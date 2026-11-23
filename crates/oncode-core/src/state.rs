//! State aplikasi: instalasi git, repositori terbuka, workspace per jendela,
//! operasi yang bisa dibatalkan, dan permintaan yang menunggu jawaban pengguna.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use oncode_forge::TokenStore;
use oncode_git::{CancellationToken, Git, GitErrorCode, HelperServer, LogStream, Prompt, PromptFn, PromptFuture, Repo};
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

/// Variabel lingkungan berisi path absolut ke `oncode-helper`; diperiksa sebelum folder binary aplikasi.
pub const HELPER_ENV: &str = "ONCODE_HELPER";

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

/// State aplikasi tanpa UI. Jendela dikenali lewat label string yang ditentukan lapisan UI.
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
    /// Login device flow yang sedang menunggu: kode pengguna → (host, device code).
    pub pending_logins: Mutex<HashMap<String, (oncode_forge::ForgeHost, String)>>,
}

impl Core {
    /// `runtime` adalah runtime tokio tempat semua pekerjaan latar dan fungsi `api::*` dijalankan;
    /// `events` menerima setiap [`CoreEvent`] (dipanggil dari thread mana pun).
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

    // ----- Runtime dan event -----

    pub fn runtime(&self) -> &Handle {
        &self.runtime
    }

    /// Menjalankan `fut` di runtime tokio milik core; boleh dipanggil dari thread mana pun.
    pub fn spawn<F>(&self, fut: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.runtime.spawn(fut)
    }

    /// Menyiarkan event ke UI.
    pub fn emit(&self, event: CoreEvent) {
        (self.events)(event);
    }

    /// Menampilkan pemberitahuan di semua jendela.
    pub fn notify(&self, kind: NoticeKind, message: impl Into<String>) {
        self.emit(CoreEvent::Notice { kind, message: message.into() });
    }

    /// Dipanggil saat aplikasi keluar: mematikan semua terminal dan language server.
    pub async fn shutdown(&self) {
        self.terminals.kill_all();
        self.lsp.stop_all().await;
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

        let events = self.events.clone();
        git.set_logger(Arc::new(move |entry| events(CoreEvent::GitOutput { entry })));

        if let Some(helper) = helper_path() {
            // Referensi lemah: `Git` disimpan di dalam core, jadi referensi kuat akan membentuk siklus.
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
        self.spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(60)).await;
                entry.dirty.store(false, Ordering::SeqCst);
                // Selama aplikasi sendiri sedang menulis, tunggu sampai selesai agar tidak menyiarkan keadaan antara.
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
    /// Membangun indeks bisa lama; panggil dari thread yang boleh memblokir.
    pub fn set_workspace(self: &Arc<Self>, label: &str, root: PathBuf) {
        self.close_workspace(label);
        let trusted = self.store.is_trusted(&root);
        let index = Arc::new(RwLock::new(FileIndex::build(&root)));

        // Referensi lemah: pemantau disimpan di dalam core, jadi referensi kuat akan membentuk siklus.
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

    /// Menghentikan pemantau, terminal, dan language server milik jendela.
    /// UI memanggilnya saat jendela ditutup.
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
        // Repositori yang tidak lagi dipakai jendela mana pun dilepas.
        let still_used: Vec<String> = self.workspaces.lock().unwrap().values().flat_map(|w| w.repos.clone()).collect();
        let mut repos = self.repos.lock().unwrap();
        for root in ws.repos {
            if !still_used.contains(&root) {
                repos.remove(&root);
            }
        }
    }

    /// Label semua jendela yang sedang membuka folder.
    pub fn workspace_labels(&self) -> Vec<String> {
        self.workspaces.lock().unwrap().keys().cloned().collect()
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

    /// Jawaban pengguna atas [`CoreEvent::Prompt`]; `None` berarti dibatalkan.
    pub fn respond_prompt(&self, id: &str, value: Option<String>) {
        if let Some(tx) = self.prompts.lock().unwrap().remove(id) {
            let _ = tx.send(value);
        }
    }

    /// Menyiarkan [`CoreEvent::Prompt`] lalu menunggu jawabannya (paling lama 10 menit).
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

    /// Menjawab permintaan dari git: kredensial akun yang sudah login dipakai otomatis, selain itu tanya pengguna.
    pub(crate) async fn answer_prompt(&self, prompt: Prompt) -> Option<String> {
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
        let tokens = self.runtime.spawn_blocking(move || store.load(&id)).await.ok()?.ok().flatten()?;
        let (username, password) = oncode_forge::git_credentials(account.kind, &tokens.access_token);
        Some(if wants_username { username } else { password })
    }
}

/// `oncode-helper`: path dari `ONCODE_HELPER` bila diisi, selain itu di samping binary aplikasi
/// (baik saat pengembangan maupun setelah dibundel).
fn helper_path() -> Option<PathBuf> {
    resolve_helper(std::env::var_os(HELPER_ENV).map(PathBuf::from), std::env::current_exe().ok())
}

fn resolve_helper(explicit: Option<PathBuf>, exe: Option<PathBuf>) -> Option<PathBuf> {
    // Penimpaan harus path absolut ke berkas yang ada; bila tidak, kembali ke lokasi bawaan.
    if let Some(path) = explicit.filter(|p| p.is_absolute() && p.is_file()) {
        return Some(path);
    }
    let name = if cfg!(windows) { "oncode-helper.exe" } else { "oncode-helper" };
    let candidate = exe?.parent()?.join(name);
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
    use crate::testing::{git, isolate_git, recv, TestCore};
    use oncode_forge::{Account, ForgeKind, TokenSet};

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

    #[test]
    fn helper_override_is_checked_before_the_exe_folder() {
        let dir = tempfile::tempdir().unwrap();
        let name = if cfg!(windows) { "oncode-helper.exe" } else { "oncode-helper" };
        let beside = dir.path().join(name);
        let exe = dir.path().join("oncode");
        let elsewhere = dir.path().join("lain").join("helper-uji");
        std::fs::create_dir(dir.path().join("lain")).unwrap();

        // Belum ada helper di mana pun.
        assert_eq!(resolve_helper(None, Some(exe.clone())), None);
        assert_eq!(resolve_helper(Some(elsewhere.clone()), Some(exe.clone())), None);
        assert_eq!(resolve_helper(None, None), None);

        std::fs::write(&beside, "").unwrap();
        assert_eq!(resolve_helper(None, Some(exe.clone())), Some(beside.clone()));
        // Penimpaan yang menunjuk berkas yang tidak ada atau path relatif diabaikan.
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
        // Seperti thread pemantau berkas: bukan thread milik runtime.
        let handle = std::thread::spawn(move || core.spawn(async { 21 * 2 })).join().unwrap();
        assert_eq!(handle.await.unwrap(), 42);
        // `runtime()` memberi handle yang sama untuk pekerjaan yang memblokir.
        assert_eq!(t.core.runtime().spawn_blocking(|| 7).await.unwrap(), 7);
    }

    #[tokio::test]
    async fn ask_user_emits_a_prompt_and_respond_prompt_resolves_it() {
        let mut t = TestCore::new();
        let core = t.core.clone();
        let asking = t.core.spawn(async move { core.ask_user(PromptKind::Secret, "Password for 'https://x@contoh.id': ".into(), String::new(), None).await });
        let (window, request) = recv(&mut t.events, |e| match e {
            CoreEvent::Prompt { window, request } => Some((window, request)),
            _ => None,
        })
        .await;
        assert_eq!(window, None, "tanpa repositori, permintaan tidak ditujukan ke jendela tertentu");
        assert_eq!(request.kind, PromptKind::Secret);
        assert_eq!(request.prompt, "Password for 'https://x@contoh.id': ");
        assert_eq!(request.repo, None);

        // Id yang tidak dikenal diabaikan dan tidak menyelesaikan permintaan yang sedang menunggu.
        t.core.respond_prompt("tidak-ada", Some("salah".into()));
        t.core.respond_prompt(&request.id, Some("rahasia".into()));
        assert_eq!(asking.await.unwrap().as_deref(), Some("rahasia"));
        assert!(t.core.prompts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn prompts_target_the_window_that_owns_the_repository_and_can_be_cancelled() {
        let mut t = TestCore::new();
        let project = t.project("proyek");
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

        // Pengguna menutup dialog.
        t.core.respond_prompt(&request.id, None);
        assert_eq!(asking.await.unwrap(), None);
    }

    #[tokio::test]
    async fn logged_in_account_answers_credential_prompts_without_asking() {
        let mut t = TestCore::new();
        let account = Account {
            id: "github:github.com:ana".into(),
            kind: ForgeKind::GitHub,
            host: "https://github.com".into(),
            username: "ana".into(),
            display_name: "Ana".into(),
            avatar_url: None,
        };
        t.core.tokens.save(&account.id, &TokenSet { access_token: "tok-123".into(), refresh_token: None, expires_at: None }).unwrap();
        t.core.store.upsert_account(account, true);

        let ask = |text: &str| t.core.answer_prompt(Prompt::Askpass { prompt: text.into(), repo: None });
        assert_eq!(ask("Username for 'https://github.com': ").await.as_deref(), Some("x-access-token"));
        assert_eq!(ask("Password for 'https://x-access-token@GitHub.com': ").await.as_deref(), Some("tok-123"));
        assert!(t.events.try_recv().is_err(), "tidak ada dialog untuk host yang akunnya sudah login");

        // Host lain tetap ditanyakan ke pengguna.
        let core = t.core.clone();
        let asking = t.core.spawn(async move { core.answer_prompt(Prompt::Askpass { prompt: "Username for 'https://git.contoh.id': ".into(), repo: None }).await });
        let request = recv(&mut t.events, |e| match e {
            CoreEvent::Prompt { request, .. } => Some(request),
            _ => None,
        })
        .await;
        assert_eq!(request.kind, PromptKind::Text);
        t.core.respond_prompt(&request.id, Some("budi".into()));
        assert_eq!(asking.await.unwrap().as_deref(), Some("budi"));
    }

    /// Jalur lengkap git → `oncode-helper` → core → UI, dengan helper ditemukan lewat `ONCODE_HELPER`.
    /// Jalankan dengan `cargo build -p oncode-helper && cargo test -p oncode-core -- --ignored`.
    #[tokio::test]
    #[ignore = "butuh binary oncode-helper: cargo build -p oncode-helper"]
    async fn git_credential_prompt_reaches_the_ui_through_the_helper() {
        use wiremock::{matchers::any, Mock, MockServer, ResponseTemplate};

        let helper = crate::testing::built_helper().expect("oncode-helper belum dibangun: cargo build -p oncode-helper");
        std::env::set_var(HELPER_ENV, &helper);
        // Server HTTP yang selalu meminta kredensial.
        let server = MockServer::start().await;
        Mock::given(any()).respond_with(ResponseTemplate::new(401).insert_header("WWW-Authenticate", "Basic realm=\"uji\"")).mount(&server).await;

        let mut t = TestCore::new();
        let project = t.trusted_repo("proyek");
        git(&project, &["remote", "add", "origin", &format!("{}/tim/proyek.git", server.uri())]);
        t.core.set_workspace("w1", project.clone());
        let repo = t.core.open_repo("w1", &project).await.unwrap();

        let fetching = t.core.spawn(async move { repo.fetch(Some("origin"), None, None).await });
        let next_prompt = |e| match e {
            CoreEvent::Prompt { window, request } => Some((window, request)),
            CoreEvent::Notice { message, .. } => panic!("helper tidak aktif: {message}"),
            _ => None,
        };
        let (window, request) = recv(&mut t.events, next_prompt).await;
        assert_eq!(window.as_deref(), Some("w1"), "permintaan ditujukan ke jendela pemilik repositori");
        assert_eq!(request.kind, PromptKind::Text);
        assert!(request.prompt.starts_with("Username for 'http://127.0.0.1"), "prompt: {}", request.prompt);
        t.core.respond_prompt(&request.id, Some("ana".into()));

        let (_, request) = recv(&mut t.events, next_prompt).await;
        assert_eq!(request.kind, PromptKind::Secret);
        assert!(request.prompt.starts_with("Password for 'http://ana@127.0.0.1"), "prompt: {}", request.prompt);
        // Pengguna membatalkan: git berhenti dengan galat, bukan menunggu selamanya.
        t.core.respond_prompt(&request.id, None);
        assert!(fetching.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn untrusted_folder_is_refused_by_open_repo() {
        isolate_git();
        let t = TestCore::new();
        let project = t.project("proyek");
        git(&project, &["init", "-q", "-b", "main"]);
        t.core.set_workspace("w1", project.clone());

        assert_eq!(t.core.open_repo("w1", &project).await.err().map(|e| e.code).as_deref(), Some("untrusted"));
        assert!(t.core.repo(&project.to_string_lossy()).is_err(), "repositori tidak boleh terdaftar");

        t.core.store.set_trusted(&project.to_string_lossy(), true);
        let repo = t.core.open_repo("w1", &project).await.expect("folder sudah dipercaya");
        assert_eq!(t.core.repo(&repo.root_string()).unwrap().root(), repo.root());
        assert_eq!(t.core.with_workspace("w1", |ws| ws.repos.clone()).unwrap(), vec![repo.root_string()]);
    }

    #[tokio::test]
    async fn refresh_broadcasts_a_snapshot_and_coalesces_bursts() {
        isolate_git();
        let mut t = TestCore::new();
        let project = t.project("proyek");
        git(&project, &["init", "-q", "-b", "main"]);
        t.core.store.set_trusted(&project.to_string_lossy(), true);
        // Tanpa workspace: tidak ada pemantau berkas yang ikut memicu refresh.
        let root = t.core.open_repo("w1", &project).await.unwrap().root_string();
        std::fs::write(project.join("baru.txt"), "isi\n").unwrap();

        for _ in 0..20 {
            t.core.refresh(&root);
        }
        let (event_root, snapshot) = recv(&mut t.events, |e| match e {
            CoreEvent::RepoChanged { root, snapshot } => Some((root, snapshot)),
            _ => None,
        })
        .await;
        assert_eq!(event_root, root);
        assert_eq!(snapshot.untracked, vec!["baru.txt".to_string()]);
        // Dua puluh permintaan beruntun menghasilkan satu siaran, bukan dua puluh.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let mut extra = 0;
        while let Ok(event) = t.events.try_recv() {
            extra += matches!(event, CoreEvent::RepoChanged { .. }) as usize;
        }
        assert_eq!(extra, 0);

        // Repositori yang tidak dibuka diabaikan tanpa galat.
        t.core.refresh("/tidak/ada");
    }

    #[tokio::test]
    async fn closing_the_last_window_releases_its_repositories() {
        isolate_git();
        let t = TestCore::new();
        let project = t.project("proyek");
        git(&project, &["init", "-q", "-b", "main"]);
        t.core.store.set_trusted(&project.to_string_lossy(), true);
        t.core.set_workspace("w1", project.clone());
        t.core.set_workspace("w2", project.clone());
        let root = t.core.open_repo("w1", &project).await.unwrap().root_string();
        t.core.open_repo("w2", &project).await.unwrap();
        assert!(t.core.workspace_trusted("w1"));

        t.core.close_workspace("w1");
        assert!(t.core.repo(&root).is_ok(), "masih dipakai jendela w2");
        assert_eq!(t.core.workspace_labels(), vec!["w2".to_string()]);
        t.core.close_workspace("w2");
        assert_eq!(t.core.repo(&root).err().map(|e| e.code).as_deref(), Some("notARepo"));
        assert_eq!(t.core.workspace_root("w2").unwrap_err().code, "noWorkspace");
    }

    #[tokio::test]
    async fn resolve_confines_paths_to_the_project_folder() {
        let t = TestCore::new();
        let project = t.project("proyek");
        assert_eq!(t.core.resolve("w1", "a.txt").unwrap_err().code, "noWorkspace");
        t.core.set_workspace("w1", project.clone());

        assert_eq!(t.core.resolve("w1", "src/a.txt").unwrap(), project.join("src/a.txt"));
        assert_eq!(t.core.resolve("w1", &project.join("b.txt").to_string_lossy()).unwrap(), project.join("b.txt"));
        assert_eq!(t.core.resolve("w1", "../luar.txt").unwrap_err().code, "invalidPath");
        assert_eq!(t.core.resolve("w1", "/etc/passwd").unwrap_err().code, "invalidPath");
        // Berkas pengaturan aplikasi boleh dibuka dari jendela mana pun.
        let settings = t.core.settings_path().to_string_lossy().into_owned();
        assert_eq!(t.core.resolve("w1", &settings).unwrap(), t.core.settings_path());
        let keys = t.core.keybindings_path();
        assert_eq!(t.core.resolve("tanpa-folder", &keys.to_string_lossy()).unwrap(), keys);
    }

    #[tokio::test]
    async fn operations_can_be_cancelled_by_id() {
        let t = TestCore::new();
        let token = t.core.begin_operation("op-1");
        t.core.cancel_operation("lain");
        assert!(!token.is_cancelled());
        t.core.cancel_operation("op-1");
        assert!(token.is_cancelled());
        t.core.end_operation("op-1");
        assert!(t.core.operations.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn project_settings_apply_only_to_trusted_folders() {
        let t = TestCore::new();
        let project = t.project("proyek");
        std::fs::create_dir_all(project.join(".oncode")).unwrap();
        std::fs::write(project.join(".oncode/settings.json"), r#"{ "editor": { "tabSize": 2 } }"#).unwrap();

        t.core.set_workspace("w1", project.clone());
        assert_eq!(t.core.effective_settings("w1").editor.tab_size, 4, "mode terbatas mengabaikan pengaturan proyek");
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
