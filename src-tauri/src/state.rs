//! State aplikasi: instalasi git, repositori terbuka, workspace per jendela,
//! operasi yang bisa dibatalkan, dan permintaan yang menunggu jawaban pengguna.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use oncode_forge::TokenStore;
use crate::tokens::FallbackStore;
use oncode_git::{CancellationToken, Git, GitErrorCode, HelperServer, LogStream, Prompt, PromptFn, PromptFuture, Repo};
use oncode_lsp::LspManager;
use oncode_terminal::TerminalManager;
use oncode_workspace::{FileIndex, WatchEvent, Watcher};
use tauri::{AppHandle, Manager};
use tauri_specta::Event;
use tokio::sync::{oneshot, OnceCell};

use crate::error::{AppError, AppResult};
use crate::events::{FsChanged, GitOutput, Notice, NoticeKind, PromptKind, PromptRequest, RefsChanged, RepoChanged};
use crate::settings::Settings;
use crate::store::Store;

pub type Shared = Arc<AppState>;

pub struct RepoEntry {
    pub repo: Repo,
    scheduled: AtomicBool,
    dirty: AtomicBool,
}

/// Satu jendela membuka satu folder.
pub struct Workspace {
    pub root: PathBuf,
    pub trusted: bool,
    watcher: Option<Watcher>,
    pub index: Arc<RwLock<FileIndex>>,
    /// Akar repositori yang dibuka dari jendela ini.
    pub repos: Vec<String>,
    pub terminals: Vec<u32>,
    pub servers: Vec<u32>,
}

pub struct AppState {
    pub app: AppHandle,
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
    /// Login device flow yang sedang menunggu: kode pengguna → (host, device code).
    pub pending_logins: Mutex<HashMap<String, (oncode_forge::ForgeHost, String)>>,
}

impl AppState {
    pub fn new(app: AppHandle, config_dir: PathBuf, data_dir: PathBuf, tokens: Arc<FallbackStore>) -> Self {
        let settings_path = config_dir.join("settings.json");
        Self {
            app,
            store: Store::load(data_dir.join("state.json")),
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
        }
    }

    pub fn next_id(&self) -> u32 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    // ----- Pengaturan -----

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

    /// Pengaturan efektif untuk sebuah jendela (pengaturan proyek hanya berlaku pada folder yang dipercaya).
    pub fn effective_settings(&self, label: &str) -> Settings {
        let user = self.user_settings();
        match self.workspaces.lock().unwrap().get(label) {
            Some(ws) if ws.trusted => user.with_project(&ws.root),
            _ => user,
        }
    }

    // ----- Git -----

    /// Instalasi git; dicari sekali saat pertama dibutuhkan.
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

        let app = self.app.clone();
        git.set_logger(Arc::new(move |entry| {
            let _ = GitOutput { entry }.emit(&app);
        }));

        if let Some(helper) = helper_path() {
            let app = self.app.clone();
            let prompt: PromptFn = Arc::new(move |prompt: Prompt| {
                let state = app.state::<Shared>().inner().clone();
                Box::pin(async move { state.answer_prompt(prompt).await }) as PromptFuture
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
            self.notify(NoticeKind::Warning, "oncode-helper was not found next to the application, so credential prompts and interactive rebase are unavailable. Reinstall OnCode or run `pnpm helper` when building from source.");
        }
        Ok(git)
    }

    /// Menampilkan pemberitahuan di semua jendela.
    pub fn notify(&self, kind: NoticeKind, message: impl Into<String>) {
        let _ = Notice { kind, message: message.into() }.emit(&self.app);
    }

    /// Repositori yang sudah dibuka untuk `root`.
    pub fn repo(&self, root: &str) -> AppResult<Repo> {
        self.repos
            .lock()
            .unwrap()
            .get(root)
            .map(|e| e.repo.clone())
            .ok_or_else(|| AppError::new("notARepo", format!("Repository is not open: {root}")))
    }

    /// Membuka repositori yang memuat `path` untuk jendela `label`. Folder harus dipercaya.
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

    /// Menjadwalkan penyiaran snapshot baru untuk `root`. Permintaan beruntun digabung.
    pub fn refresh(self: &Arc<Self>, root: &str) {
        let Some(entry) = self.repos.lock().unwrap().get(root).cloned() else { return };
        entry.dirty.store(true, Ordering::SeqCst);
        if entry.scheduled.swap(true, Ordering::SeqCst) {
            return;
        }
        let state = self.clone();
        let root = root.to_string();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(60)).await;
                entry.dirty.store(false, Ordering::SeqCst);
                // Selama aplikasi sendiri sedang menulis, tunggu sampai selesai agar tidak menyiarkan keadaan antara.
                entry.repo.wait_idle().await;
                match entry.repo.snapshot().await {
                    Ok(snapshot) => {
                        let _ = RepoChanged { root: root.clone(), snapshot }.emit(&state.app);
                    }
                    Err(e) if e.code == GitErrorCode::NotARepo => {
                        state.repos.lock().unwrap().remove(&root);
                        break;
                    }
                    Err(_) => {}
                }
                if !entry.dirty.load(Ordering::SeqCst) {
                    entry.scheduled.store(false, Ordering::SeqCst);
                    // Permintaan yang datang tepat di antara dua pemeriksaan di atas.
                    if entry.dirty.load(Ordering::SeqCst) && !entry.scheduled.swap(true, Ordering::SeqCst) {
                        continue;
                    }
                    break;
                }
            }
        });
    }

    pub fn refresh_refs(&self, root: &str) {
        let _ = RefsChanged { root: root.to_string() }.emit(&self.app);
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

    /// Memastikan `path` berada di dalam folder proyek jendela ini (atau berkas pengaturan aplikasi).
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

    /// Menetapkan folder proyek sebuah jendela: mulai memantau berkas dan membangun indeks.
    pub fn set_workspace(self: &Arc<Self>, label: &str, root: PathBuf) {
        self.close_workspace(label);
        let trusted = self.store.is_trusted(&root);
        let index = Arc::new(RwLock::new(FileIndex::build(&root)));

        let state = self.clone();
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
                let _ = FsChanged { paths, overflow }.emit_to(&state.app, watch_label.as_str());
                state.refresh_workspace(&watch_label, false);
            }
            WatchEvent::Git { refs, .. } => state.refresh_workspace(&watch_label, refs),
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

    /// Menghentikan pemantau, terminal, dan language server milik jendela.
    pub fn close_workspace(self: &Arc<Self>, label: &str) {
        let Some(ws) = self.workspaces.lock().unwrap().remove(label) else { return };
        for id in &ws.terminals {
            self.terminals.kill(*id);
        }
        let lsp = self.lsp.clone();
        let servers = ws.servers.clone();
        tauri::async_runtime::spawn(async move {
            for id in servers {
                lsp.stop(id).await;
            }
        });
        // Repositori yang tidak lagi dipakai jendela mana pun dilepas.
        let still_used: Vec<String> = self.workspaces.lock().unwrap().values().flat_map(|w| w.repos.clone()).collect();
        let mut repos = self.repos.lock().unwrap();
        for root in ws.repos {
            if !still_used.contains(&root) {
                repos.remove(&root);
            }
        }
    }

    /// Label jendela yang folder proyeknya memuat `path`.
    fn window_for(&self, path: &Path) -> Option<String> {
        self.workspaces.lock().unwrap().iter().find(|(_, ws)| path.starts_with(&ws.root)).map(|(label, _)| label.clone())
    }

    // ----- Operasi yang bisa dibatalkan -----

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

    // ----- Permintaan ke pengguna -----

    pub fn respond_prompt(&self, id: &str, value: Option<String>) {
        if let Some(tx) = self.prompts.lock().unwrap().remove(id) {
            let _ = tx.send(value);
        }
    }

    async fn ask_user(&self, kind: PromptKind, prompt: String, initial: String, repo: Option<PathBuf>) -> Option<String> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let (tx, rx) = oneshot::channel();
        self.prompts.lock().unwrap().insert(id.clone(), tx);
        let request = PromptRequest { id: id.clone(), kind, prompt, initial, repo: repo.as_ref().map(|p| p.to_string_lossy().into_owned()) };
        let target = repo.as_deref().and_then(|p| self.window_for(p));
        let sent = match target {
            Some(label) => request.emit_to(&self.app, label.as_str()),
            None => request.emit(&self.app),
        };
        if sent.is_err() {
            self.prompts.lock().unwrap().remove(&id);
            return None;
        }
        let answer = tokio::time::timeout(Duration::from_secs(600), rx).await.ok().and_then(Result::ok).flatten();
        self.prompts.lock().unwrap().remove(&id);
        answer
    }

    /// Menjawab permintaan dari git: kredensial akun yang sudah login dipakai otomatis, selain itu tanya pengguna.
    async fn answer_prompt(&self, prompt: Prompt) -> Option<String> {
        match prompt {
            Prompt::Askpass { prompt, repo } => {
                if let Some(answer) = self.account_credential(&prompt).await {
                    return Some(answer);
                }
                self.ask_user(classify_prompt(&prompt), prompt, String::new(), repo).await
            }
            Prompt::EditMessage { initial, repo } => self.ask_user(PromptKind::Message, String::new(), initial, repo).await,
        }
    }

    /// Username/password untuk host yang cocok dengan akun GitHub/GitLab yang sudah login.
    async fn account_credential(&self, prompt: &str) -> Option<String> {
        let wants_username = prompt.starts_with("Username");
        if !wants_username && !prompt.starts_with("Password") {
            return None;
        }
        let host = prompt_host(prompt)?;
        let host = host.split(':').next().unwrap_or(&host).to_string();
        let account = self.store.accounts().into_iter().find(|a| {
            let account_host = host_of(&a.host);
            account_host.split(':').next().unwrap_or(&account_host).eq_ignore_ascii_case(&host)
        })?;
        let store = self.tokens.clone();
        let id = account.id.clone();
        // Akses keychain bisa memblokir, jadi dijalankan di thread tersendiri.
        let tokens = tokio::task::spawn_blocking(move || store.load(&id)).await.ok()?.ok().flatten()?;
        let (username, password) = oncode_forge::git_credentials(account.kind, &tokens.access_token);
        Some(if wants_username { username } else { password })
    }
}

/// `oncode-helper` berada di samping binary aplikasi, baik saat pengembangan maupun setelah dibundel.
fn helper_path() -> Option<PathBuf> {
    let name = if cfg!(windows) { "oncode-helper.exe" } else { "oncode-helper" };
    let exe = std::env::current_exe().ok()?;
    let candidate = exe.parent()?.join(name);
    candidate.is_file().then_some(candidate)
}

fn host_of(origin: &str) -> String {
    origin.split("://").nth(1).unwrap_or(origin).split('/').next().unwrap_or("").to_string()
}

/// Mengambil nama host dari prompt git seperti `Password for 'https://user@github.com': `.
pub fn prompt_host(prompt: &str) -> Option<String> {
    let start = prompt.find('\'')? + 1;
    let end = prompt[start..].find('\'')? + start;
    let url = &prompt[start..end];
    let rest = url.split("://").nth(1)?;
    let authority = rest.split('/').next()?;
    let host = authority.rsplit('@').next()?;
    (!host.is_empty()).then(|| host.to_string())
}

/// Menentukan jenis dialog dari teks prompt git/ssh.
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

    #[test]
    fn extracts_host_from_git_prompts() {
        assert_eq!(prompt_host("Username for 'https://github.com': ").as_deref(), Some("github.com"));
        assert_eq!(prompt_host("Password for 'https://ana@gitlab.contoh.id:8443': ").as_deref(), Some("gitlab.contoh.id:8443"));
        assert_eq!(prompt_host("Password for 'https://x@github.com/a/b.git': ").as_deref(), Some("github.com"));
        assert_eq!(prompt_host("Enter passphrase for key '/home/a/.ssh/id_ed25519': "), None);
    }

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

    #[test]
    fn host_of_strips_scheme_and_path() {
        assert_eq!(host_of("https://github.com"), "github.com");
        assert_eq!(host_of("https://git.contoh.id/sub"), "git.contoh.id");
    }
}
