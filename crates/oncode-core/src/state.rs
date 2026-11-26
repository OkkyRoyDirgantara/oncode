//! Application state: the git installation, open repositories, per-window workspaces,
//! cancelable operations, and requests waiting for a user answer.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use oncode_forge::{Account, ForgeHost, TokenStore};
use oncode_git::{CancellationToken, CredentialRequest, Git, GitErrorCode, HelperServer, LogStream, Prompt, PromptFn, PromptFuture, Repo};
use oncode_lsp::LspManager;
use oncode_terminal::TerminalManager;
use oncode_workspace::{FileIndex, WatchEvent, Watcher};
use tokio::runtime::Handle;
use tokio::sync::{oneshot, OnceCell};
use tokio::task::JoinHandle;

use crate::dirs::Dirs;
use crate::error::{AppError, AppResult};
use crate::events::{CoreEvent, EventSink, NoticeKind, PromptKind, PromptRequest};
use crate::settings::Settings;
use crate::store::Store;
use crate::tokens::FallbackStore;

pub type Shared = Arc<Core>;

/// Environment variable holding the absolute path to `oncode-helper`; checked before the app's binary folder.
pub const HELPER_ENV: &str = "ONCODE_HELPER";

/// Comparison form of a repository root: `/` separators, no trailing `/`, and on Windows
/// case-insensitive. Git reports the root as `C:/project`, while Windows paths
/// are written `C:\project`; both point to the same repository.
pub fn root_key(root: &str) -> String {
    // On Linux/macOS `\` is a valid name character, so it is only treated as `/` on Windows.
    if cfg!(windows) {
        root.replace('\\', "/").trim_end_matches('/').to_lowercase()
    } else {
        root.trim_end_matches('/').to_string()
    }
}

/// Registry entry for `root`: matched exactly, or via [`root_key`].
fn find_repo(repos: &HashMap<String, Arc<RepoEntry>>, root: &str) -> Option<(String, Arc<RepoEntry>)> {
    if let Some(entry) = repos.get(root) {
        return Some((root.to_string(), entry.clone()));
    }
    let key = root_key(root);
    repos.iter().find(|(candidate, _)| root_key(candidate) == key).map(|(candidate, entry)| (candidate.clone(), entry.clone()))
}

pub struct RepoEntry {
    pub repo: Repo,
    scheduled: AtomicBool,
    dirty: AtomicBool,
}

/// One window opens one folder.
pub struct Workspace {
    pub root: PathBuf,
    pub trusted: bool,
    watcher: Option<Watcher>,
    pub index: Arc<RwLock<FileIndex>>,
    /// Roots of the repositories opened from this window.
    pub repos: Vec<String>,
    pub terminals: Vec<u32>,
    pub servers: Vec<u32>,
}

/// Application state without UI. Windows are identified by a string label chosen by the UI layer.
pub struct Core {
    runtime: Handle,
    events: EventSink,
    pub store: Store,
    settings_path: PathBuf,
    settings: RwLock<Settings>,
    git: OnceCell<Result<Arc<Git>, AppError>>,
    repos: Mutex<HashMap<String, Arc<RepoEntry>>>,
    workspaces: Mutex<HashMap<String, Workspace>>,
    pub log_streams: tokio::sync::Mutex<HashMap<u32, LogStream>>,
    next_id: AtomicU32,
    operations: Mutex<HashMap<String, CancellationToken>>,
    prompts: Mutex<HashMap<String, oneshot::Sender<Option<String>>>>,
    pub terminals: Arc<TerminalManager>,
    pub lsp: Arc<LspManager>,
    pub tokens: Arc<FallbackStore>,
    /// Device flow logins currently waiting, keyed by user code.
    pub pending_logins: Mutex<HashMap<String, PendingLogin>>,
}

/// One device flow login waiting for approval in the browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingLogin {
    pub host: oncode_forge::ForgeHost,
    pub device_code: String,
    /// The Client ID used when starting; polling must use the same one.
    pub client_id: String,
}

impl Core {
    /// `runtime` is the tokio runtime where all background work and `api::*` functions run;
    /// `events` receives every [`CoreEvent`] (called from any thread).
    pub fn new(runtime: Handle, dirs: Dirs, tokens: Arc<FallbackStore>, events: EventSink) -> Shared {
        let settings_path = dirs.settings_path();
        Arc::new(Self {
            runtime,
            events,
            store: Store::load(dirs.state_path()),
            settings: RwLock::new(Settings::load(&settings_path)),
            settings_path,
            git: OnceCell::new(),
            repos: Mutex::new(HashMap::new()),
            workspaces: Mutex::new(HashMap::new()),
            log_streams: tokio::sync::Mutex::new(HashMap::new()),
            next_id: AtomicU32::new(1),
            operations: Mutex::new(HashMap::new()),
            prompts: Mutex::new(HashMap::new()),
            terminals: Arc::new(TerminalManager::new()),
            lsp: Arc::new(LspManager::new()),
            tokens,
            pending_logins: Mutex::new(HashMap::new()),
        })
    }

    pub fn next_id(&self) -> u32 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    // ----- Runtime and events -----

    pub fn runtime(&self) -> &Handle {
        &self.runtime
    }

    /// Runs `fut` on the core's tokio runtime; may be called from any thread.
    pub fn spawn<F>(&self, fut: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.runtime.spawn(fut)
    }

    /// Broadcasts an event to the UI.
    pub fn emit(&self, event: CoreEvent) {
        (self.events)(event);
    }

    /// Shows a notification in all windows.
    pub fn notify(&self, kind: NoticeKind, message: impl Into<String>) {
        self.emit(CoreEvent::Notice { kind, message: message.into() });
    }

    /// Called when the app exits: kills all terminals and language servers.
    pub async fn shutdown(&self) {
        self.terminals.kill_all();
        self.lsp.stop_all().await;
    }

    // ----- Settings -----

    pub fn settings_path(&self) -> &Path {
        &self.settings_path
    }

    pub fn keybindings_path(&self) -> PathBuf {
        self.settings_path.with_file_name("keybindings.json")
    }

    pub fn user_settings(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }

    pub fn set_user_settings(&self, settings: Settings) -> std::io::Result<()> {
        settings.save(&self.settings_path)?;
        *self.settings.write().unwrap() = settings;
        Ok(())
    }

    /// Effective settings for a window (project settings only apply to trusted folders).
    pub fn effective_settings(&self, label: &str) -> Settings {
        let user = self.user_settings();
        match self.workspaces.lock().unwrap().get(label) {
            Some(ws) if ws.trusted => user.with_project(&ws.root),
            _ => user,
        }
    }

    // ----- Git -----

    /// The git installation; looked up once, the first time it is needed.
    pub async fn git(self: &Arc<Self>) -> AppResult<Arc<Git>> {
        self.git.get_or_init(|| self.init_git()).await.clone()
    }

    async fn init_git(self: &Arc<Self>) -> AppResult<Arc<Git>> {
        let explicit = self.user_settings().git.path.filter(|p| !p.trim().is_empty()).map(PathBuf::from);
        let git = Git::discover(explicit).await?;
        if !git.is_supported() {
            let (major, minor) = oncode_git::MIN_GIT_VERSION;
            return Err(AppError::new(
                "gitNotFound",
                format!("Git {} is too old; OnCode requires Git {major}.{minor} or newer", git.info().version),
            ));
        }

        let events = self.events.clone();
        git.set_logger(Arc::new(move |entry| events(CoreEvent::GitOutput { entry })));

        if let Some(helper) = helper_path() {
            // Weak reference: `Git` is stored inside the core, so a strong reference would form a cycle.
            let core = Arc::downgrade(self);
            let prompt: PromptFn = Arc::new(move |prompt: Prompt| {
                let core = core.upgrade();
                Box::pin(async move {
                    match core {
                        Some(core) => core.answer_prompt(prompt).await,
                        None => None,
                    }
                }) as PromptFuture
            });
            match HelperServer::start(helper, prompt).await {
                Ok(server) => git.set_helper(server),
                Err(e) => {
                    eprintln!("oncode: helper server failed to start: {e}");
                    self.notify(NoticeKind::Error, format!("Credential prompts and interactive rebase are unavailable: the helper server failed to start ({e})"));
                }
            }
        } else {
            eprintln!("oncode: oncode-helper not found; credential prompts and interactive rebase are unavailable");
            self.notify(NoticeKind::Warning, "oncode-helper was not found next to the application, so credential prompts and interactive rebase are unavailable. Reinstall OnCode or run `cargo build -p oncode-helper` when building from source.");
        }
        Ok(git)
    }

    /// The repository already opened for `root`.
    pub fn repo(&self, root: &str) -> AppResult<Repo> {
        find_repo(&self.repos.lock().unwrap(), root)
            .map(|(_, entry)| entry.repo.clone())
            .ok_or_else(|| AppError::new("notARepo", format!("Repository is not open: {root}")))
    }

    /// Opens the repository containing `path` for window `label`. The folder must be trusted.
    pub async fn open_repo(self: &Arc<Self>, label: &str, path: &Path) -> AppResult<Repo> {
        if !self.store.is_trusted(path) {
            return Err(AppError::untrusted());
        }
        let git = self.git().await?;
        let repo = Repo::open(git, path).await?;
        let root = repo.root_string();
        let repo = {
            let mut repos = self.repos.lock().unwrap();
            repos
                .entry(root.clone())
                .or_insert_with(|| Arc::new(RepoEntry { repo, scheduled: AtomicBool::new(false), dirty: AtomicBool::new(false) }))
                .repo
                .clone()
        };
        if let Some(ws) = self.workspaces.lock().unwrap().get_mut(label) {
            if !ws.repos.contains(&root) {
                ws.repos.push(root.clone());
            }
            let dirs = self.git_dirs(&ws.repos);
            if let Some(watcher) = &ws.watcher {
                let _ = watcher.set_git_dirs(&dirs);
            }
        }
        Ok(repo)
    }

    fn git_dirs(&self, roots: &[String]) -> Vec<PathBuf> {
        let repos = self.repos.lock().unwrap();
        let mut dirs: Vec<PathBuf> = Vec::new();
        for root in roots {
            if let Some(entry) = repos.get(root) {
                for dir in [entry.repo.git_dir(), entry.repo.common_dir()] {
                    if !dirs.iter().any(|d| d == dir) {
                        dirs.push(dir.to_path_buf());
                    }
                }
            }
        }
        dirs
    }

    /// Schedules broadcasting a new snapshot for `root`. Consecutive requests are coalesced.
    pub fn refresh(self: &Arc<Self>, root: &str) {
        // Broadcast with the root as registered (from Git), whatever form the caller gave.
        let Some((root, entry)) = find_repo(&self.repos.lock().unwrap(), root) else { return };
        entry.dirty.store(true, Ordering::SeqCst);
        if entry.scheduled.swap(true, Ordering::SeqCst) {
            return;
        }
        let state = self.clone();
        self.spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(60)).await;
                entry.dirty.store(false, Ordering::SeqCst);
                // While the app itself is writing, wait until it finishes so no intermediate state is broadcast.
                entry.repo.wait_idle().await;
                match entry.repo.snapshot().await {
                    Ok(snapshot) => state.emit(CoreEvent::RepoChanged { root: root.clone(), snapshot }),
                    Err(e) if e.code == GitErrorCode::NotARepo => {
                        state.repos.lock().unwrap().remove(&root);
                        break;
                    }
                    Err(_) => {}
                }
                if !entry.dirty.load(Ordering::SeqCst) {
                    entry.scheduled.store(false, Ordering::SeqCst);
                    // A request that arrives exactly between the two checks above.
                    if entry.dirty.load(Ordering::SeqCst) && !entry.scheduled.swap(true, Ordering::SeqCst) {
                        continue;
                    }
                    break;
                }
            }
        });
    }

    pub fn refresh_refs(&self, root: &str) {
        self.emit(CoreEvent::RefsChanged { root: root.to_string() });
    }

    // ----- Workspace -----

    pub fn workspace_root(&self, label: &str) -> AppResult<PathBuf> {
        self.workspaces
            .lock()
            .unwrap()
            .get(label)
            .map(|ws| ws.root.clone())
            .ok_or_else(|| AppError::new("noWorkspace", "No folder is open in this window"))
    }

    pub fn workspace_trusted(&self, label: &str) -> bool {
        self.workspaces.lock().unwrap().get(label).is_some_and(|ws| ws.trusted)
    }

    pub fn with_workspace<T>(&self, label: &str, f: impl FnOnce(&mut Workspace) -> T) -> AppResult<T> {
        let mut workspaces = self.workspaces.lock().unwrap();
        let ws = workspaces
            .get_mut(label)
            .ok_or_else(|| AppError::new("noWorkspace", "No folder is open in this window"))?;
        Ok(f(ws))
    }

    /// Ensures `path` is inside this window's project folder (or is an app settings file).
    pub fn resolve(&self, label: &str, path: &str) -> AppResult<PathBuf> {
        let candidate = PathBuf::from(path);
        if candidate == self.settings_path || candidate == self.keybindings_path() {
            return Ok(candidate);
        }
        let root = self.workspace_root(label)?;
        let full = if candidate.is_absolute() { candidate } else { root.join(candidate) };
        if oncode_workspace::is_inside(&root, &full) {
            Ok(full)
        } else {
            Err(AppError::new("invalidPath", format!("Path is outside the project folder: {path}")))
        }
    }

    /// Sets a window's project folder: starts watching files and building the index.
    /// Building the index can take a while; call from a thread that may block.
    pub fn set_workspace(self: &Arc<Self>, label: &str, root: PathBuf) {
        self.close_workspace(label);
        let trusted = self.store.is_trusted(&root);
        let index = Arc::new(RwLock::new(FileIndex::build(&root)));

        // Weak reference: the watcher is stored inside the core, so a strong reference would form a cycle.
        let core = Arc::downgrade(self);
        let watch_label = label.to_string();
        let watch_root = root.clone();
        let watch_index = index.clone();
        let watcher = Watcher::start(&root, &[], move |event| match event {
            WatchEvent::Files { paths, overflow } => {
                if overflow {
                    *watch_index.write().unwrap() = FileIndex::build(&watch_root);
                } else {
                    let mut index = watch_index.write().unwrap();
                    for path in &paths {
                        let full = Path::new(path);
                        let Ok(rel) = full.strip_prefix(&watch_root) else { continue };
                        let rel = rel.to_string_lossy().replace('\\', "/");
                        if full.is_file() {
                            index.add(&rel);
                        } else if !full.exists() {
                            index.remove(&rel);
                        }
                    }
                }
                let Some(state) = core.upgrade() else { return };
                state.emit(CoreEvent::FsChanged { window: watch_label.clone(), paths, overflow });
                state.refresh_workspace(&watch_label, false);
            }
            WatchEvent::Git { refs, .. } => {
                if let Some(state) = core.upgrade() {
                    state.refresh_workspace(&watch_label, refs);
                }
            }
        })
        .map_err(|e| eprintln!("oncode: failed to start the file watcher: {}", e.message))
        .ok();

        self.workspaces.lock().unwrap().insert(
            label.to_string(),
            Workspace { root, trusted, watcher, index, repos: Vec::new(), terminals: Vec::new(), servers: Vec::new() },
        );
    }

    fn refresh_workspace(self: &Arc<Self>, label: &str, refs: bool) {
        let roots = self.workspaces.lock().unwrap().get(label).map(|ws| ws.repos.clone()).unwrap_or_default();
        for root in roots {
            self.refresh(&root);
            if refs {
                self.refresh_refs(&root);
            }
        }
    }

    /// Stops the window's watcher, terminals, and language servers.
    /// The UI calls this when the window is closed.
    pub fn close_workspace(self: &Arc<Self>, label: &str) {
        let Some(ws) = self.workspaces.lock().unwrap().remove(label) else { return };
        for id in &ws.terminals {
            self.terminals.kill(*id);
        }
        let lsp = self.lsp.clone();
        let servers = ws.servers.clone();
        self.spawn(async move {
            for id in servers {
                lsp.stop(id).await;
            }
        });
        // Repositories no longer used by any window are released.
        let still_used: Vec<String> = self.workspaces.lock().unwrap().values().flat_map(|w| w.repos.clone()).collect();
        let mut repos = self.repos.lock().unwrap();
        for root in ws.repos {
            if !still_used.contains(&root) {
                repos.remove(&root);
            }
        }
    }

    /// Labels of all windows that currently have a folder open.
    pub fn workspace_labels(&self) -> Vec<String> {
        self.workspaces.lock().unwrap().keys().cloned().collect()
    }

    /// Label of the window whose project folder contains `path`.
    fn window_for(&self, path: &Path) -> Option<String> {
        self.workspaces.lock().unwrap().iter().find(|(_, ws)| path.starts_with(&ws.root)).map(|(label, _)| label.clone())
    }

    // ----- Cancelable operations -----

    pub fn begin_operation(&self, id: &str) -> CancellationToken {
        let token = CancellationToken::new();
        self.operations.lock().unwrap().insert(id.to_string(), token.clone());
        token
    }

    pub fn end_operation(&self, id: &str) {
        self.operations.lock().unwrap().remove(id);
    }

    pub fn cancel_operation(&self, id: &str) {
        if let Some(token) = self.operations.lock().unwrap().get(id) {
            token.cancel();
        }
    }

    // ----- Requests to the user -----

    /// The user's answer to a [`CoreEvent::Prompt`]; `None` means canceled.
    pub fn respond_prompt(&self, id: &str, value: Option<String>) {
        if let Some(tx) = self.prompts.lock().unwrap().remove(id) {
            let _ = tx.send(value);
        }
    }

    /// Broadcasts a [`CoreEvent::Prompt`] and then waits for its answer (at most 10 minutes).
    pub(crate) async fn ask_user(&self, kind: PromptKind, prompt: String, initial: String, repo: Option<PathBuf>) -> Option<String> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let (tx, rx) = oneshot::channel();
        self.prompts.lock().unwrap().insert(id.clone(), tx);
        let request = PromptRequest { id: id.clone(), kind, prompt, initial, repo: repo.as_ref().map(|p| p.to_string_lossy().into_owned()) };
        let window = repo.as_deref().and_then(|p| self.window_for(p));
        self.emit(CoreEvent::Prompt { window, request });
        let answer = tokio::time::timeout(Duration::from_secs(600), rx).await.ok().and_then(Result::ok).flatten();
        self.prompts.lock().unwrap().remove(&id);
        answer
    }

    /// Answers a request from git. A logged-in account answers the credential helper for its own
    /// host; askpass prompts always go to the user, because their text cannot tell the real host
    /// apart from a user name in the URL (`Password for 'https://github.com'x@evil.com': `).
    pub(crate) async fn answer_prompt(&self, prompt: Prompt) -> Option<String> {
        match prompt {
            Prompt::Credential(request) => self.account_credential(&request).await,
            Prompt::Askpass { prompt, repo } => self.ask_user(classify_prompt(&prompt), prompt, String::new(), repo).await,
            Prompt::EditMessage { initial, repo } => self.ask_user(PromptKind::Message, String::new(), initial, repo).await,
        }
    }

    /// The login of the GitHub/GitLab account on the requested host, in git's credential format.
    async fn account_credential(&self, request: &CredentialRequest) -> Option<String> {
        let account = self.store.accounts().into_iter().find(|a| account_serves(a, request))?;
        let store = self.tokens.clone();
        let id = account.id.clone();
        // Keychain access can block, so it runs on its own thread.
        let tokens = self.runtime.spawn_blocking(move || store.load(&id)).await.ok()?.ok().flatten()?;
        let (username, password) = oncode_forge::git_credentials(account.kind, &tokens.access_token);
        request.reply(&username, &password)
    }
}

/// `true` if git asks for a login on `account`'s host with its scheme, so an `http://` remote never
/// receives the token of an `https://` account. Like [`oncode_forge::ForgeHost::matches`], the port
/// is not compared.
fn account_serves(account: &Account, request: &CredentialRequest) -> bool {
    let Some((scheme, _)) = account.host.split_once("://") else { return false };
    let hostname = ForgeHost { kind: account.kind, host: account.host.clone() }.hostname();
    !hostname.is_empty() && scheme.eq_ignore_ascii_case(&request.protocol) && hostname == request.hostname()
}

/// `oncode-helper`: the path from `ONCODE_HELPER` if set, otherwise next to the app binary
/// (both during development and after bundling).
pub(crate) fn helper_path() -> Option<PathBuf> {
    resolve_helper(std::env::var_os(HELPER_ENV).map(PathBuf::from), std::env::current_exe().ok())
}

fn resolve_helper(explicit: Option<PathBuf>, exe: Option<PathBuf>) -> Option<PathBuf> {
    // The override must be an absolute path to an existing file; otherwise fall back to the default location.
    if let Some(path) = explicit.filter(|p| p.is_absolute() && p.is_file()) {
        return Some(path);
    }
    let name = if cfg!(windows) { "oncode-helper.exe" } else { "oncode-helper" };
    let candidate = exe?.parent()?.join(name);
    candidate.is_file().then_some(candidate)
}

/// Determines the dialog kind from the text of a git/ssh prompt.
pub fn classify_prompt(prompt: &str) -> PromptKind {
    let lower = prompt.to_lowercase();
    if lower.contains("(yes/no") {
        PromptKind::Confirm
    } else if lower.contains("password") || lower.contains("passphrase") || lower.contains("token") || lower.contains("pin") {
        PromptKind::Secret
    } else {
        PromptKind::Text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{git, isolate_git, recv, TestCore};
    use oncode_forge::{ForgeKind, TokenSet};

    #[test]
    fn classifies_prompt_kinds() {
        assert_eq!(classify_prompt("Username for 'https://github.com': "), PromptKind::Text);
        assert_eq!(classify_prompt("Password for 'https://a@github.com': "), PromptKind::Secret);
        assert_eq!(classify_prompt("Enter passphrase for key '/home/a/.ssh/id_ed25519': "), PromptKind::Secret);
        assert_eq!(
            classify_prompt("The authenticity of host 'x' can't be established.\nAre you sure you want to continue connecting (yes/no/[fingerprint])? "),
            PromptKind::Confirm
        );
    }

    fn account(kind: ForgeKind, host: &str) -> Account {
        let id = format!("{}:{}:ana", kind.as_str(), ForgeHost { kind, host: host.into() }.hostname());
        Account { id, kind, host: host.into(), username: "ana".into(), display_name: "Ana".into(), avatar_url: None }
    }

    fn request(protocol: &str, host: &str, username: Option<&str>) -> CredentialRequest {
        CredentialRequest { protocol: protocol.into(), host: host.into(), username: username.map(String::from) }
    }

    #[test]
    fn account_serves_only_its_own_scheme_and_host() {
        let github = account(ForgeKind::GitHub, "https://github.com");
        assert!(account_serves(&github, &request("https", "github.com", None)));
        assert!(account_serves(&github, &request("https", "GitHub.com", Some("x-access-token"))));
        // Like `ForgeHost::matches`, the port is not compared.
        assert!(account_serves(&github, &request("https", "github.com:443", None)));

        // The user name of `https://github.com'x@evil.com/` arrives as its own field, never as the host.
        assert!(!account_serves(&github, &request("https", "evil.com", Some("github.com'x"))));
        assert!(!account_serves(&github, &request("https", "evil.com", Some("https://github.com"))));
        // Plain HTTP never receives the token of an HTTPS account.
        assert!(!account_serves(&github, &request("http", "github.com", None)));
        // Look-alike hosts.
        assert!(!account_serves(&github, &request("https", "github.com.evil.com", None)));
        assert!(!account_serves(&github, &request("https", "evil-github.com", None)));
        assert!(!account_serves(&github, &request("https", "", None)));
        assert!(!account_serves(&github, &request("ssh", "github.com", None)));

        let gitlab = account(ForgeKind::GitLab, "https://gitlab.example.com:8443/sub");
        assert!(account_serves(&gitlab, &request("https", "gitlab.example.com:8443", None)));
        assert!(!account_serves(&gitlab, &request("https", "example.com", None)));
        // An instance served over plain HTTP matches only HTTP.
        let local = account(ForgeKind::GitLab, "http://127.0.0.1:8080");
        assert!(account_serves(&local, &request("http", "127.0.0.1:9000", None)));
        assert!(!account_serves(&local, &request("https", "127.0.0.1:8080", None)));
    }

    #[test]
    fn helper_override_is_checked_before_the_exe_folder() {
        let dir = tempfile::tempdir().unwrap();
        let name = if cfg!(windows) { "oncode-helper.exe" } else { "oncode-helper" };
        let beside = dir.path().join(name);
        let exe = dir.path().join("oncode");
        let elsewhere = dir.path().join("other").join("test-helper");
        std::fs::create_dir(dir.path().join("other")).unwrap();

        // There is no helper anywhere yet.
        assert_eq!(resolve_helper(None, Some(exe.clone())), None);
        assert_eq!(resolve_helper(Some(elsewhere.clone()), Some(exe.clone())), None);
        assert_eq!(resolve_helper(None, None), None);

        std::fs::write(&beside, "").unwrap();
        assert_eq!(resolve_helper(None, Some(exe.clone())), Some(beside.clone()));
        // An override pointing to a nonexistent file or a relative path is ignored.
        assert_eq!(resolve_helper(Some(elsewhere.clone()), Some(exe.clone())), Some(beside.clone()));
        assert_eq!(resolve_helper(Some(PathBuf::from(name)), Some(exe.clone())), Some(beside.clone()));

        std::fs::write(&elsewhere, "").unwrap();
        assert_eq!(resolve_helper(Some(elsewhere.clone()), Some(exe)), Some(elsewhere.clone()));
        assert_eq!(resolve_helper(Some(elsewhere.clone()), None), Some(elsewhere));
    }

    #[tokio::test]
    async fn spawn_runs_on_the_given_runtime_from_any_thread() {
        let t = TestCore::new();
        let core = t.core.clone();
        // Like the file watcher thread: not a thread owned by the runtime.
        let handle = std::thread::spawn(move || core.spawn(async { 21 * 2 })).join().unwrap();
        assert_eq!(handle.await.unwrap(), 42);
        // `runtime()` gives the same handle for blocking work.
        assert_eq!(t.core.runtime().spawn_blocking(|| 7).await.unwrap(), 7);
    }

    #[tokio::test]
    async fn ask_user_emits_a_prompt_and_respond_prompt_resolves_it() {
        let mut t = TestCore::new();
        let core = t.core.clone();
        let asking = t.core.spawn(async move { core.ask_user(PromptKind::Secret, "Password for 'https://x@example.com': ".into(), String::new(), None).await });
        let (window, request) = recv(&mut t.events, |e| match e {
            CoreEvent::Prompt { window, request } => Some((window, request)),
            _ => None,
        })
        .await;
        assert_eq!(window, None, "without a repository, the request is not targeted at a specific window");
        assert_eq!(request.kind, PromptKind::Secret);
        assert_eq!(request.prompt, "Password for 'https://x@example.com': ");
        assert_eq!(request.repo, None);

        // An unknown id is ignored and does not resolve the pending request.
        t.core.respond_prompt("does-not-exist", Some("wrong".into()));
        t.core.respond_prompt(&request.id, Some("secret".into()));
        assert_eq!(asking.await.unwrap().as_deref(), Some("secret"));
        assert!(t.core.prompts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn prompts_target_the_window_that_owns_the_repository_and_can_be_cancelled() {
        let mut t = TestCore::new();
        let project = t.project("project");
        t.core.set_workspace("w7", project.clone());

        let core = t.core.clone();
        let repo = project.join("sub");
        let asking = t.core.spawn(async move { core.answer_prompt(Prompt::EditMessage { initial: "Merge branch 'a'".into(), repo: Some(repo) }).await });
        let (window, request) = recv(&mut t.events, |e| match e {
            CoreEvent::Prompt { window, request } => Some((window, request)),
            _ => None,
        })
        .await;
        assert_eq!(window.as_deref(), Some("w7"));
        assert_eq!(request.kind, PromptKind::Message);
        assert_eq!(request.initial, "Merge branch 'a'");
        assert_eq!(request.repo.as_deref(), Some(project.join("sub").to_string_lossy().as_ref()));

        // The user closes the dialog.
        t.core.respond_prompt(&request.id, None);
        assert_eq!(asking.await.unwrap(), None);
    }

    #[tokio::test]
    async fn logged_in_account_answers_the_credential_helper_for_its_host_only() {
        let mut t = TestCore::new();
        let github = account(ForgeKind::GitHub, "https://github.com");
        t.core.tokens.save(&github.id, &TokenSet { access_token: "tok-123".into(), refresh_token: None, expires_at: None }).unwrap();
        t.core.store.upsert_account(github, true);

        let ask = |protocol: &str, host: &str, username: Option<&str>| t.core.answer_prompt(Prompt::Credential(request(protocol, host, username)));
        assert_eq!(ask("https", "github.com", None).await.as_deref(), Some("username=x-access-token\npassword=tok-123\n"));
        // A user name in the URL is kept; only the password is filled in.
        assert_eq!(ask("https", "GitHub.com", Some("ana")).await.as_deref(), Some("password=tok-123\n"));
        // `.gitmodules` with `http://github.com'x@evil.com/...`: the token must not leave.
        assert_eq!(ask("http", "evil.com", Some("github.com'x")).await, None);
        assert_eq!(ask("https", "evil.com", Some("github.com'x")).await, None);
        assert_eq!(ask("http", "github.com", None).await, None);
        assert!(t.events.try_recv().is_err(), "the credential helper never opens a dialog; git falls back to askpass");

        // Askpass is never answered from an account, even when its text names the account's host:
        // git builds it from the URL, so `Password for 'https://github.com'x@evil.com': ` asks for evil.com.
        let core = t.core.clone();
        let spoofed = "Password for 'https://github.com'x@evil.com': ";
        let asking = t.core.spawn(async move { core.answer_prompt(Prompt::Askpass { prompt: spoofed.into(), repo: None }).await });
        let shown = recv(&mut t.events, |e| match e {
            CoreEvent::Prompt { request, .. } => Some(request),
            _ => None,
        })
        .await;
        assert_eq!((shown.kind, shown.prompt.as_str()), (PromptKind::Secret, spoofed));
        t.core.respond_prompt(&shown.id, None);
        assert_eq!(asking.await.unwrap(), None);
    }

    /// The full path git → `oncode-helper` → core → UI, with the helper found via `ONCODE_HELPER`.
    /// Run with `cargo build -p oncode-helper && cargo test -p oncode-core -- --ignored`.
    #[tokio::test]
    #[ignore = "needs the oncode-helper binary: cargo build -p oncode-helper"]
    async fn git_credential_prompt_reaches_the_ui_through_the_helper() {
        use wiremock::{matchers::any, Mock, MockServer, ResponseTemplate};

        let helper = crate::testing::built_helper().expect("oncode-helper has not been built: cargo build -p oncode-helper");
        std::env::set_var(HELPER_ENV, &helper);
        // An HTTP server that always asks for credentials.
        let server = MockServer::start().await;
        Mock::given(any()).respond_with(ResponseTemplate::new(401).insert_header("WWW-Authenticate", "Basic realm=\"test\"")).mount(&server).await;

        let mut t = TestCore::new();
        let project = t.trusted_repo("project");
        git(&project, &["remote", "add", "origin", &format!("{}/team/project.git", server.uri())]);
        t.core.set_workspace("w1", project.clone());
        let repo = t.core.open_repo("w1", &project).await.unwrap();

        let fetching = t.core.spawn(async move { repo.fetch(Some("origin"), None, None).await });
        let next_prompt = |e| match e {
            CoreEvent::Prompt { window, request } => Some((window, request)),
            CoreEvent::Notice { message, .. } => panic!("helper is not active: {message}"),
            _ => None,
        };
        let (window, request) = recv(&mut t.events, next_prompt).await;
        assert_eq!(window.as_deref(), Some("w1"), "the request is targeted at the window that owns the repository");
        assert_eq!(request.kind, PromptKind::Text);
        assert!(request.prompt.starts_with("Username for 'http://127.0.0.1"), "prompt: {}", request.prompt);
        t.core.respond_prompt(&request.id, Some("ana".into()));

        let (_, request) = recv(&mut t.events, next_prompt).await;
        assert_eq!(request.kind, PromptKind::Secret);
        assert!(request.prompt.starts_with("Password for 'http://ana@127.0.0.1"), "prompt: {}", request.prompt);
        // The user cancels: git stops with an error instead of waiting forever.
        t.core.respond_prompt(&request.id, None);
        assert!(fetching.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn untrusted_folder_is_refused_by_open_repo() {
        isolate_git();
        let t = TestCore::new();
        let project = t.project("project");
        git(&project, &["init", "-q", "-b", "main"]);
        t.core.set_workspace("w1", project.clone());

        assert_eq!(t.core.open_repo("w1", &project).await.err().map(|e| e.code).as_deref(), Some("untrusted"));
        assert!(t.core.repo(&project.to_string_lossy()).is_err(), "the repository must not be registered");

        t.core.store.set_trusted(&project.to_string_lossy(), true);
        let repo = t.core.open_repo("w1", &project).await.expect("the folder is now trusted");
        assert_eq!(t.core.repo(&repo.root_string()).unwrap().root(), repo.root());
        assert_eq!(t.core.with_workspace("w1", |ws| ws.repos.clone()).unwrap(), vec![repo.root_string()]);
    }

    #[tokio::test]
    async fn refresh_broadcasts_a_snapshot_and_coalesces_bursts() {
        isolate_git();
        let mut t = TestCore::new();
        let project = t.project("project");
        git(&project, &["init", "-q", "-b", "main"]);
        t.core.store.set_trusted(&project.to_string_lossy(), true);
        // Without a workspace: no file watcher helps trigger a refresh.
        let root = t.core.open_repo("w1", &project).await.unwrap().root_string();
        std::fs::write(project.join("new.txt"), "content\n").unwrap();

        for _ in 0..20 {
            t.core.refresh(&root);
        }
        let (event_root, snapshot) = recv(&mut t.events, |e| match e {
            CoreEvent::RepoChanged { root, snapshot } => Some((root, snapshot)),
            _ => None,
        })
        .await;
        assert_eq!(event_root, root);
        assert_eq!(snapshot.untracked, vec!["new.txt".to_string()]);
        // Twenty consecutive requests produce one broadcast, not twenty.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let mut extra = 0;
        while let Ok(event) = t.events.try_recv() {
            extra += matches!(event, CoreEvent::RepoChanged { .. }) as usize;
        }
        assert_eq!(extra, 0);

        // A repository that is not open is ignored without an error.
        t.core.refresh("/does/not/exist");
    }

    #[tokio::test]
    async fn closing_the_last_window_releases_its_repositories() {
        isolate_git();
        let t = TestCore::new();
        let project = t.project("project");
        git(&project, &["init", "-q", "-b", "main"]);
        t.core.store.set_trusted(&project.to_string_lossy(), true);
        t.core.set_workspace("w1", project.clone());
        t.core.set_workspace("w2", project.clone());
        let root = t.core.open_repo("w1", &project).await.unwrap().root_string();
        t.core.open_repo("w2", &project).await.unwrap();
        assert!(t.core.workspace_trusted("w1"));

        t.core.close_workspace("w1");
        assert!(t.core.repo(&root).is_ok(), "still used by window w2");
        assert_eq!(t.core.workspace_labels(), vec!["w2".to_string()]);
        t.core.close_workspace("w2");
        assert_eq!(t.core.repo(&root).err().map(|e| e.code).as_deref(), Some("notARepo"));
        assert_eq!(t.core.workspace_root("w2").unwrap_err().code, "noWorkspace");
    }

    #[tokio::test]
    async fn resolve_confines_paths_to_the_project_folder() {
        let t = TestCore::new();
        let project = t.project("project");
        assert_eq!(t.core.resolve("w1", "a.txt").unwrap_err().code, "noWorkspace");
        t.core.set_workspace("w1", project.clone());

        assert_eq!(t.core.resolve("w1", "src/a.txt").unwrap(), project.join("src/a.txt"));
        assert_eq!(t.core.resolve("w1", &project.join("b.txt").to_string_lossy()).unwrap(), project.join("b.txt"));
        assert_eq!(t.core.resolve("w1", "../outside.txt").unwrap_err().code, "invalidPath");
        assert_eq!(t.core.resolve("w1", "/etc/passwd").unwrap_err().code, "invalidPath");
        // The app settings files may be opened from any window.
        let settings = t.core.settings_path().to_string_lossy().into_owned();
        assert_eq!(t.core.resolve("w1", &settings).unwrap(), t.core.settings_path());
        let keys = t.core.keybindings_path();
        assert_eq!(t.core.resolve("no-folder", &keys.to_string_lossy()).unwrap(), keys);
    }

    #[tokio::test]
    async fn operations_can_be_cancelled_by_id() {
        let t = TestCore::new();
        let token = t.core.begin_operation("op-1");
        t.core.cancel_operation("other");
        assert!(!token.is_cancelled());
        t.core.cancel_operation("op-1");
        assert!(token.is_cancelled());
        t.core.end_operation("op-1");
        assert!(t.core.operations.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn project_settings_apply_only_to_trusted_folders() {
        let t = TestCore::new();
        let project = t.project("project");
        std::fs::create_dir_all(project.join(".oncode")).unwrap();
        std::fs::write(project.join(".oncode/settings.json"), r#"{ "editor": { "tabSize": 2 } }"#).unwrap();

        t.core.set_workspace("w1", project.clone());
        assert_eq!(t.core.effective_settings("w1").editor.tab_size, 4, "restricted mode ignores project settings");
        t.core.store.set_trusted(&project.to_string_lossy(), true);
        t.core.set_workspace("w1", project.clone());
        assert_eq!(t.core.effective_settings("w1").editor.tab_size, 2);
        assert_eq!(t.core.user_settings().editor.tab_size, 4);

        let mut user = t.core.user_settings();
        user.editor.font_size = 17;
        t.core.set_user_settings(user.clone()).unwrap();
        assert_eq!(Settings::load(t.core.settings_path()), user);
        assert_eq!(t.core.effective_settings("w1").editor.font_size, 17);
    }
}
