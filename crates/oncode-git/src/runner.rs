//! Runs `git` as a child process in a controlled environment.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::error::{GitError, GitErrorCode, Result};
use crate::helper::{HelperServer, OpContext};
use crate::model::{GitInfo, GitLogEntry, Progress};

/// The minimum supported Git version.
pub const MIN_GIT_VERSION: (u32, u32) = (2, 40);

pub type ProgressFn = Arc<dyn Fn(Progress) + Send + Sync>;
pub type LogFn = Arc<dyn Fn(GitLogEntry) + Send + Sync>;

/// The git installation used by the app.
pub struct Git {
    path: PathBuf,
    version: (u32, u32, u32),
    version_text: String,
    helper: RwLock<Option<Arc<HelperServer>>>,
    logger: RwLock<Option<LogFn>>,
}

#[derive(Debug)]
pub struct Output {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl Output {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn trimmed(&self) -> String {
        self.text().trim().to_string()
    }
}

/// Makes git read pathspecs as plain file names (see [`Cmd::paths`]).
pub(crate) const LITERAL_PATHSPECS: &str = "GIT_LITERAL_PATHSPECS";

/// A single git command to run.
pub struct Cmd {
    pub(crate) args: Vec<OsString>,
    pub(crate) stdin: Option<Vec<u8>>,
    pub(crate) write: bool,
    pub(crate) log: bool,
    pub(crate) ok_codes: Vec<i32>,
    pub(crate) env: Vec<(String, OsString)>,
    pub(crate) cancel: Option<CancellationToken>,
    pub(crate) progress: Option<ProgressFn>,
    pub(crate) timeout: Option<Duration>,
    pub(crate) context: Option<OpContext>,
}

impl Cmd {
    pub fn new<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        Self {
            args: args.into_iter().map(Into::into).collect(),
            stdin: None,
            write: false,
            log: false,
            ok_codes: vec![0],
            env: Vec::new(),
            cancel: None,
            progress: None,
            timeout: None,
            context: None,
        }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn arg_if(self, cond: bool, arg: &str) -> Self {
        if cond {
            self.arg(arg)
        } else {
            self
        }
    }

    /// Appends `-- <path...>`. The paths are file names, not patterns: git would otherwise read
    /// `[ ] * ?` as a glob, so discarding `app/[id]/page.tsx` would also discard `app/i/page.tsx`.
    /// The variable is set only for commands that take paths, because git passes it on to hooks,
    /// whose own pathspecs may be meant as globs.
    pub fn paths<I, S>(mut self, paths: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.push("--".into());
        self.args.extend(paths.into_iter().map(Into::into));
        self.literal_paths()
    }

    /// Makes git match the command's paths literally; for commands that build their own `-- <path>`.
    pub(crate) fn literal_paths(self) -> Self {
        self.env(LITERAL_PATHSPECS, "1")
    }

    pub fn stdin(mut self, data: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(data.into());
        self
    }

    /// Marks the command as a write operation: it is recorded in Git Output and may lock the index.
    pub fn write(mut self) -> Self {
        self.write = true;
        self.log = true;
        self
    }

    /// Exit codes other than 0 that are treated as success.
    pub fn ok_codes(mut self, codes: &[i32]) -> Self {
        self.ok_codes = codes.to_vec();
        self
    }

    pub fn env(mut self, key: &str, value: impl Into<OsString>) -> Self {
        self.env.push((key.to_string(), value.into()));
        self
    }

    pub fn cancel(mut self, token: Option<CancellationToken>) -> Self {
        self.cancel = token;
        self
    }

    pub fn progress(mut self, progress: Option<ProgressFn>) -> Self {
        self.progress = progress;
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn context(mut self, context: OpContext) -> Self {
        self.context = Some(context);
        self
    }
}

impl Git {
    /// Locates git (an explicit path or from PATH) and checks its version.
    pub async fn discover(explicit: Option<PathBuf>) -> Result<Arc<Self>> {
        let path = explicit.unwrap_or_else(|| PathBuf::from(if cfg!(windows) { "git.exe" } else { "git" }));
        let mut command = Command::new(&path);
        command.arg("--version").stdin(Stdio::null());
        hide_window(&mut command);
        let output = command
            .output()
            .await
            .map_err(|e| GitError::new(GitErrorCode::GitNotFound, format!("Git not found: {e}")))?;
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let version = parse_version(&text)
            .ok_or_else(|| GitError::new(GitErrorCode::GitNotFound, format!("Unrecognized git version: {text}")))?;
        Ok(Arc::new(Self {
            path,
            version,
            version_text: text,
            helper: RwLock::new(None),
            logger: RwLock::new(None),
        }))
    }

    pub fn info(&self) -> GitInfo {
        GitInfo {
            path: self.path.to_string_lossy().into_owned(),
            version: format!("{}.{}.{}", self.version.0, self.version.1, self.version.2),
            supported: self.is_supported(),
        }
    }

    pub fn version(&self) -> (u32, u32, u32) {
        self.version
    }

    pub fn version_text(&self) -> &str {
        &self.version_text
    }

    pub fn is_supported(&self) -> bool {
        (self.version.0, self.version.1) >= MIN_GIT_VERSION
    }

    pub fn set_helper(&self, helper: Arc<HelperServer>) {
        *self.helper.write().unwrap() = Some(helper);
    }

    pub fn set_logger(&self, logger: LogFn) {
        *self.logger.write().unwrap() = Some(logger);
    }

    /// Runs a command; an exit code outside `ok_codes` becomes an error.
    pub async fn run(&self, cwd: &Path, cmd: Cmd) -> Result<Output> {
        let ok_codes = cmd.ok_codes.clone();
        let display: Vec<String> = cmd.args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        let output = self.run_raw(cwd, cmd).await?;
        if ok_codes.contains(&output.code) {
            Ok(output)
        } else {
            Err(GitError::from_output(&display, output.code, &output.text(), &output.stderr))
        }
    }

    /// Runs a command and returns the output whatever its exit code.
    pub async fn run_raw(&self, cwd: &Path, cmd: Cmd) -> Result<Output> {
        let started = Instant::now();
        let display: Vec<String> = cmd.args.iter().map(|a| a.to_string_lossy().into_owned()).collect();

        let helper = self.helper.read().unwrap().clone();
        let mut command = Command::new(&self.path);
        command.current_dir(cwd).args(["--no-pager", "-c", "color.ui=false", "-c", "core.quotepath=false"]);
        if let Some(server) = &helper {
            // Logged-in forge accounts answer through git's credential protocol, which passes the
            // protocol and host as separate fields; the askpass prompt text can be spoofed by the URL.
            // `-c` comes last in git's config order, so the user's own helpers are still asked first,
            // and git passes it on to the git processes it starts (submodule clones).
            command.arg("-c").arg(credential_helper_config(server.helper_path()));
        }
        command
            .args(&cmd.args)
            .env("LC_ALL", "en_US.UTF-8")
            .env("LANG", "en_US.UTF-8")
            .env("LANGUAGE", "en")
            .env("GIT_PAGER", "cat")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(if cmd.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .kill_on_drop(true);
        if !cmd.write {
            command.env("GIT_OPTIONAL_LOCKS", "0");
        }
        hide_window(&mut command);

        let mut token = None;
        match &helper {
            Some(server) => {
                let helper_path = server.helper_path().to_string_lossy().replace('\\', "/");
                let registered = server.register(cmd.context.clone().unwrap_or_else(|| OpContext {
                    repo: Some(cwd.to_path_buf()),
                    ..OpContext::default()
                }));
                command
                    .env("GIT_ASKPASS", server.helper_path())
                    .env("SSH_ASKPASS", server.helper_path())
                    .env("SSH_ASKPASS_REQUIRE", "force")
                    .env("GIT_EDITOR", format!("\"{helper_path}\" editor"))
                    .env("GIT_SEQUENCE_EDITOR", format!("\"{helper_path}\" sequence"))
                    .env("ONCODE_IPC_ENDPOINT", server.endpoint())
                    .env("ONCODE_IPC_TOKEN", &registered);
                token = Some(registered);
            }
            None => {
                // Without a helper nothing can answer the editor; accept the default content.
                command.env("GIT_EDITOR", "true").env("GIT_SEQUENCE_EDITOR", "true");
            }
        }
        for (key, value) in &cmd.env {
            command.env(key, value);
        }

        let result = self.execute(command, &cmd).await;
        if let (Some(server), Some(token)) = (&helper, &token) {
            server.unregister(token);
        }

        let logger = self.logger.read().unwrap().clone();
        if let Some(logger) = logger {
            let (code, text) = match &result {
                Ok(out) => (out.code, log_text(&String::from_utf8_lossy(&out.stdout), &out.stderr)),
                Err(e) => (-1, e.message.clone()),
            };
            if cmd.log || code != 0 && cmd.write {
                logger(GitLogEntry {
                    command: format!("git {}", shell_join(&display)),
                    cwd: cwd.to_string_lossy().into_owned(),
                    duration_ms: started.elapsed().as_millis().min(u32::MAX as u128) as u32,
                    exit_code: code,
                    output: text,
                });
            }
        }
        result
    }

    async fn execute(&self, mut command: Command, cmd: &Cmd) -> Result<Output> {
        let mut child = command.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                GitError::new(GitErrorCode::GitNotFound, "Git not found")
            } else {
                GitError::io("Failed to run git", e)
            }
        })?;

        if let Some(data) = cmd.stdin.clone() {
            if let Some(mut stdin) = child.stdin.take() {
                tokio::spawn(async move {
                    let _ = stdin.write_all(&data).await;
                    let _ = stdin.shutdown().await;
                });
            }
        }

        let mut stdout = child.stdout.take().expect("stdout is piped");
        let mut stderr = child.stderr.take().expect("stderr is piped");
        let out_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = stdout.read_to_end(&mut buf).await;
            buf
        });
        let progress = cmd.progress.clone();
        let err_task = tokio::spawn(async move {
            let mut all = Vec::new();
            let mut pending = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                match stderr.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        all.extend_from_slice(&chunk[..n]);
                        if let Some(progress) = &progress {
                            pending.extend_from_slice(&chunk[..n]);
                            while let Some(pos) = pending.iter().position(|b| *b == b'\r' || *b == b'\n') {
                                let line: Vec<u8> = pending.drain(..=pos).collect();
                                if let Some(p) = parse_progress(&String::from_utf8_lossy(&line)) {
                                    progress(p);
                                }
                            }
                        }
                    }
                }
            }
            String::from_utf8_lossy(&all).into_owned()
        });

        let cancel = cmd.cancel.clone().unwrap_or_default();
        let timeout = cmd.timeout;
        let status = tokio::select! {
            status = child.wait() => status.map_err(|e| GitError::io("Failed to wait for git", e))?,
            _ = cancel.cancelled() => {
                let _ = child.kill().await;
                return Err(GitError::new(GitErrorCode::Cancelled, "Operation cancelled"));
            }
            _ = async {
                match timeout {
                    Some(t) => tokio::time::sleep(t).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                let _ = child.kill().await;
                return Err(GitError::new(GitErrorCode::Timeout, "Git did not finish within the time limit"));
            }
        };

        let stdout = out_task.await.unwrap_or_default();
        let stderr = clean_stderr(&err_task.await.unwrap_or_default());
        Ok(Output { code: status.code().unwrap_or(-1), stdout, stderr })
    }
}

/// `credential.helper=!'<helper>' credential`: git runs it through the shell and appends the
/// operation (`get`, `store`, `erase`).
fn credential_helper_config(helper: &Path) -> String {
    let path = helper.to_string_lossy().replace('\\', "/");
    format!("credential.helper=!{} credential", sh_quote(&path))
}

/// Quotes `value` as a single word for `sh`.
fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

#[cfg(windows)]
fn hide_window(command: &mut Command) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_window(_command: &mut Command) {}

/// Parses "git version 2.53.0" (including suffixes such as ".windows.1").
pub fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let rest = text.trim().strip_prefix("git version ")?;
    let mut parts = rest.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty());
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    Some((major, minor, patch))
}

/// Parses a git progress line, e.g. `Receiving objects:  45% (450/1000), 1.2 MiB | 500 KiB/s`.
pub fn parse_progress(line: &str) -> Option<Progress> {
    let line = line.trim().trim_start_matches("remote: ").trim();
    let (phase, rest) = line.split_once(':')?;
    let phase = phase.trim();
    if phase.is_empty() || phase.len() > 40 || !rest.contains('(') && !rest.contains('%') {
        return None;
    }
    let percent = rest.split('%').next().and_then(|p| p.trim().parse::<u8>().ok()).filter(|_| rest.contains('%'));
    Some(Progress { phase: phase.to_string(), percent, text: line.to_string() })
}

/// Keeps the last line of each run of progress updates (`\r`).
fn clean_stderr(raw: &str) -> String {
    raw.split('\n')
        .map(|line| line.rsplit('\r').find(|part| !part.trim().is_empty()).unwrap_or("").trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Most stdout kept in a Git Output entry; enough for a summary such as bisect's verdict or a merge's
/// file list, without copying a large listing into the panel.
const LOG_STDOUT_LIMIT: usize = 16 * 1024;

/// The text of a Git Output entry: what git printed to stdout (results such as "is the first bad
/// commit" or a commit summary) followed by stderr (progress and messages).
fn log_text(stdout: &str, stderr: &str) -> String {
    let mut out = stdout.trim();
    let mut cut = false;
    if out.len() > LOG_STDOUT_LIMIT {
        let mut end = LOG_STDOUT_LIMIT;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out = &out[..end];
        cut = true;
    }
    let mut text = out.to_string();
    if cut {
        text.push_str("\n…");
    }
    let err = stderr.trim();
    if !err.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(err);
    }
    text
}

fn shell_join(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.is_empty() || a.chars().any(|c| c.is_whitespace() || "\"'$`\\*?[]{}()<>|&;#~".contains(c)) {
                format!("'{}'", a.replace('\'', "'\\''"))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Rejects ref/revision names that could be read as an option or contain control characters.
pub fn check_ref(value: &str) -> Result<&str> {
    if value.is_empty() || value.starts_with('-') || value.chars().any(|c| c.is_control()) {
        return Err(GitError::invalid(format!("Invalid ref name: {value:?}")));
    }
    Ok(value)
}

/// Rejects paths that escape the repository: absolute, rooted (`/x` or `\x`, which on Windows
/// `is_absolute` does not count because they have no drive), drive-prefixed (`C:x`), or using `..`.
pub fn check_path(value: &str) -> Result<&str> {
    use std::path::Component;
    let path = Path::new(value);
    if value.is_empty()
        || value.contains('\0')
        || path.is_absolute()
        || path.components().any(|c| matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
    {
        return Err(GitError::invalid(format!("Invalid path: {value:?}")));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_text_keeps_stdout_before_stderr_and_caps_it() {
        assert_eq!(log_text("abc1234 is the first bad commit\n", ""), "abc1234 is the first bad commit");
        assert_eq!(log_text("[main 1a2b3c4] Fix\n", "warning: x\n"), "[main 1a2b3c4] Fix\nwarning: x");
        assert_eq!(log_text("", "  fatal: no\n"), "fatal: no");
        let long = "é".repeat(LOG_STDOUT_LIMIT);
        let text = log_text(&long, "");
        assert!(text.len() <= LOG_STDOUT_LIMIT + "\n…".len());
        assert!(text.ends_with("\n…"));
    }

    #[test]
    fn parses_versions() {
        assert_eq!(parse_version("git version 2.53.0"), Some((2, 53, 0)));
        assert_eq!(parse_version("git version 2.45.1.windows.1"), Some((2, 45, 1)));
        assert_eq!(parse_version("git version 2.39.5 (Apple Git-154)"), Some((2, 39, 5)));
        assert_eq!(parse_version("nope"), None);
    }

    #[test]
    fn parses_progress_lines() {
        let p = parse_progress("Receiving objects:  45% (450/1000), 1.20 MiB | 500.00 KiB/s").unwrap();
        assert_eq!(p.phase, "Receiving objects");
        assert_eq!(p.percent, Some(45));
        let p = parse_progress("remote: Counting objects: 100% (10/10), done.").unwrap();
        assert_eq!(p.phase, "Counting objects");
        assert_eq!(p.percent, Some(100));
        assert!(parse_progress("fatal: repository not found").is_none());
        assert!(parse_progress("To github.com:a/b.git").is_none());
    }

    #[test]
    fn stderr_keeps_last_progress_update() {
        let raw = "Counting: 10% (1/10)\rCounting: 100% (10/10), done.\nfatal: boom\n";
        assert_eq!(clean_stderr(raw), "Counting: 100% (10/10), done.\nfatal: boom");
    }

    #[test]
    fn credential_helper_path_is_one_shell_word() {
        assert_eq!(
            credential_helper_config(Path::new("/opt/On Code/oncode-helper")),
            "credential.helper=!'/opt/On Code/oncode-helper' credential"
        );
        assert_eq!(
            credential_helper_config(Path::new("/home/o'brien/$x/oncode-helper")),
            r"credential.helper=!'/home/o'\''brien/$x/oncode-helper' credential"
        );
        if cfg!(windows) {
            assert_eq!(
                credential_helper_config(Path::new(r"C:\Program Files\OnCode\oncode-helper.exe")),
                "credential.helper=!'C:/Program Files/OnCode/oncode-helper.exe' credential"
            );
        }
    }

    #[test]
    fn rejects_option_like_refs_and_escaping_paths() {
        assert!(check_ref("--upload-pack=x").is_err());
        assert!(check_ref("feature/x").is_ok());
        assert!(check_path("../etc/passwd").is_err());
        assert!(check_path("/abs").is_err());
        assert!(check_path("src/a b.rs").is_ok());
        assert!(check_path("src/../../outside").is_err());
        if cfg!(windows) {
            assert!(check_path(r"\Windows\System32").is_err());
            assert!(check_path("C:outside.txt").is_err());
            assert!(check_path(r"C:\outside.txt").is_err());
            assert!(check_path(r"src\a.rs").is_ok());
        }
    }
}
