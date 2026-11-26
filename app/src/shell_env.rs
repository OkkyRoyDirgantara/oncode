//! The PATH from the user's login shell, for applications launched from Finder, the Dock, or a launcher.
//!
//! GUI applications on macOS inherit a minimal PATH from launchd (`/usr/bin:/bin:/usr/sbin:/sbin`),
//! not the PATH assembled by `~/.zprofile`/`~/.zshrc`. Without this step, programs in `~/.local/bin`,
//! `/opt/homebrew/bin`, `~/.cargo/bin`, etc. (Claude Code CLI, language servers, formatters) are not
//! found. The login shell's PATH is read once at startup and then merged into the process PATH, so
//! program lookup and all child processes use it too.

/// Merges the login shell's PATH into the process PATH.
///
/// Must be called first thing in `main`, before a runtime or any other thread is created: changing
/// an environment variable is not safe while other threads can read it.
#[cfg(unix)]
pub fn import_login_path() {
    use std::io::IsTerminal as _;

    // Launched from a terminal: the PATH is already the user's shell PATH (including venvs and the like).
    if std::io::stdin().is_terminal() {
        return;
    }
    let Some(shell) = login_shell() else { return };
    let Some(login) = unix::read_login_path(&shell) else { return };
    let current = std::env::var_os("PATH").unwrap_or_default();
    if let Some(merged) = merge(std::ffi::OsStr::new(&login), &current) {
        std::env::set_var("PATH", merged);
    }
}

#[cfg(not(unix))]
pub fn import_login_path() {}

#[cfg(unix)]
fn login_shell() -> Option<String> {
    let shell = std::env::var("SHELL").ok().map(|shell| shell.trim().to_string()).filter(|shell| !shell.is_empty());
    // zsh is macOS's default shell when launchd does not provide `SHELL`.
    shell.or_else(|| cfg!(target_os = "macos").then(|| "/bin/zsh".to_string()))
}

#[cfg(unix)]
mod unix {
    use std::io::Read as _;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    /// Time limit for the login shell; a stuck rc file (e.g. `exec tmux`) must not hold up the application.
    const TIMEOUT: Duration = Duration::from_secs(3);

    /// Runs `$SHELL -l -i` (login and interactive: both `~/.zprofile` and `~/.zshrc` are
    /// read, like a regular terminal) and takes its PATH. `None` if it fails or takes too long.
    pub(super) fn read_login_path(shell: &str) -> Option<String> {
        let script = format!("echo {}; /usr/bin/env -0", super::MARKER);
        let mut command = Command::new(shell);
        command.args(["-l", "-i", "-c", &script]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
        // From home, so per-folder tools (direnv, `.nvmrc`) do not affect the result.
        if let Some(home) = std::env::var_os("HOME") {
            command.current_dir(home);
        }
        let mut child = command.spawn().ok()?;
        let mut stdout = child.stdout.take()?;
        let (tx, rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut output = Vec::new();
            let _ = stdout.read_to_end(&mut output);
            let _ = tx.send(output);
        });
        let output = rx.recv_timeout(TIMEOUT);
        let _ = child.kill();
        let _ = child.wait();
        // The reader thread is only waited for if it has finished; after the time limit it is left alone
        // (the pipe may stay open because of a background process started by the rc file).
        let output = output.ok()?;
        let _ = reader.join();
        super::parse_path(&String::from_utf8_lossy(&output))
    }
}

/// The marker for the start of the `env` output: the rc file may print anything before it.
#[cfg(any(unix, test))]
const MARKER: &str = "__ONCODE_SHELL_ENV__";

/// The PATH value from the output of `echo MARKER; env -0`.
#[cfg(any(unix, test))]
fn parse_path(output: &str) -> Option<String> {
    let (_, env) = output.split_once(MARKER)?;
    env.trim_start_matches(['\r', '\n'])
        .split('\0')
        .find_map(|entry| entry.strip_prefix("PATH="))
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(String::from)
}

/// The login shell's PATH first (the user's preferred order), then the process PATH folders that are not yet present.
#[cfg(any(unix, test))]
fn merge(login: &std::ffi::OsStr, current: &std::ffi::OsStr) -> Option<std::ffi::OsString> {
    let mut seen = std::collections::HashSet::new();
    let dirs: Vec<std::path::PathBuf> = std::env::split_paths(login)
        .chain(std::env::split_paths(current))
        .filter(|dir| !dir.as_os_str().is_empty())
        .filter(|dir| seen.insert(dir.clone()))
        .collect();
    if dirs.is_empty() {
        return None;
    }
    std::env::join_paths(dirs).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn parses_path_after_the_marker_and_ignores_rc_noise() {
        let output = format!("Welcome!\nPATH=/fake\n{MARKER}\nHOME=/Users/a\0PATH=/Users/a/.local/bin:/usr/bin\0SHELL=/bin/zsh\0");
        assert_eq!(parse_path(&output).as_deref(), Some("/Users/a/.local/bin:/usr/bin"));
    }

    #[test]
    fn missing_marker_or_path_gives_none() {
        assert_eq!(parse_path("PATH=/usr/bin\0"), None);
        assert_eq!(parse_path(&format!("{MARKER}\nHOME=/a\0")), None);
        assert_eq!(parse_path(&format!("{MARKER}\nPATH=\0")), None);
    }

    #[cfg(unix)]
    #[test]
    fn login_path_comes_first_without_duplicates() {
        let merged = merge(OsStr::new("/Users/a/.local/bin:/opt/homebrew/bin:/usr/bin"), OsStr::new("/usr/bin:/bin:/usr/sbin:/sbin"));
        assert_eq!(merged.as_deref(), Some(OsStr::new("/Users/a/.local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin")));
        assert_eq!(merge(OsStr::new(""), OsStr::new("")), None);
    }

    /// A real shell: the result contains a system PATH folder that is always present.
    #[cfg(unix)]
    #[test]
    fn reads_path_from_a_real_login_shell() {
        let path = unix::read_login_path("/bin/sh").expect("PATH from /bin/sh -l -i");
        assert!(std::env::split_paths(OsStr::new(&path)).any(|dir| dir == std::path::Path::new("/bin") || dir == std::path::Path::new("/usr/bin")), "{path}");
    }
}
