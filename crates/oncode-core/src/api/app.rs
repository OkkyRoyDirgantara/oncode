//! Aplikasi, workspace, pengaturan, dan sesi.

use std::path::{Path, PathBuf};

use oncode_git::GitInfo;
use serde::Serialize;

use crate::error::{AppError, AppResult};
use crate::events::CoreEvent;
use crate::settings::Settings;
use crate::state::Shared;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    /// `linux`, `windows`, atau `macos`.
    pub platform: String,
    pub git: Option<GitInfo>,
    /// Alasan fitur Git tidak tersedia (git tidak ada atau terlalu lama).
    pub git_error: Option<String>,
    pub recent: Vec<String>,
    pub settings_path: String,
    /// Berkas penimpaan pintasan keyboard milik pengguna.
    pub keybindings_path: String,
    pub home_dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceInfo {
    pub root: String,
    pub name: String,
    /// `false` = mode terbatas: Git, LSP, dan pengaturan proyek dinonaktifkan.
    pub trusted: bool,
    /// Akar repositori yang memuat folder ini, bila ada dan folder dipercaya.
    pub repo_root: Option<String>,
    pub settings: Settings,
}

pub async fn app_info(core: &Shared) -> AppResult<AppInfo> {
    let (git, git_error) = match core.git().await {
        Ok(git) => (Some(git.info()), None),
        Err(e) => (None, Some(e.message)),
    };
    Ok(AppInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        platform: std::env::consts::OS.to_string(),
        git,
        git_error,
        recent: core.store.recent().into_iter().filter(|p| Path::new(p).is_dir()).collect(),
        settings_path: core.settings_path().to_string_lossy().into_owned(),
        keybindings_path: core.keybindings_path().to_string_lossy().into_owned(),
        home_dir: crate::dirs::home_dir().map(|p| p.to_string_lossy().into_owned()),
    })
}

async fn workspace_info(core: &Shared, label: &str) -> AppResult<WorkspaceInfo> {
    let root = core.workspace_root(label)?;
    let trusted = core.workspace_trusted(label);
    let mut repo_root = None;
    if trusted {
        if let Ok(repo) = core.open_repo(label, &root).await {
            repo_root = Some(repo.root_string());
            core.refresh(&repo.root_string());
        }
    }
    Ok(WorkspaceInfo {
        name: root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.to_string_lossy().into_owned()),
        root: root.to_string_lossy().into_owned(),
        trusted,
        repo_root,
        settings: core.effective_settings(label),
    })
}

/// Membuka folder di jendela ini. Judul jendela (`<name> — OnCode`) diatur lapisan UI dari `name`.
pub async fn workspace_open(core: &Shared, window: &str, path: String) -> AppResult<WorkspaceInfo> {
    let root = std::fs::canonicalize(&path).map_err(|e| AppError::new("notFound", format!("Cannot open folder: {e}")))?;
    if !root.is_dir() {
        return Err(AppError::new("notFound", format!("Not a folder: {path}")));
    }
    let root = strip_verbatim(root);
    let shared = core.clone();
    let build_root = root.clone();
    let build_label = window.to_string();
    // Membangun indeks berkas bisa lama pada proyek besar; jangan menahan thread async.
    core.runtime()
        .spawn_blocking(move || shared.set_workspace(&build_label, build_root))
        .await
        .map_err(|e| AppError::new("io", e.to_string()))?;
    core.store.add_recent(&root.to_string_lossy());
    workspace_info(core, window).await
}

/// Windows: `canonicalize` menghasilkan path `\\?\C:\...` yang tidak dipahami banyak program.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => path,
    }
}

/// Memercayai (atau berhenti memercayai) folder jendela ini, lalu memuat ulang workspace.
pub async fn workspace_set_trusted(core: &Shared, window: &str, trusted: bool) -> AppResult<WorkspaceInfo> {
    let root = core.workspace_root(window)?;
    core.store.set_trusted(&root.to_string_lossy(), trusted);
    workspace_open(core, window, root.to_string_lossy().into_owned()).await
}

/// Menutup folder jendela ini; juga dipanggil UI saat jendelanya ditutup.
pub async fn workspace_close(core: &Shared, window: &str) -> AppResult<()> {
    core.close_workspace(window);
    Ok(())
}

/// Menandai folder sebagai dipercaya tanpa membukanya (dipakai setelah clone/init dari aplikasi).
pub async fn workspace_trust_path(core: &Shared, path: String) -> AppResult<()> {
    let root = std::fs::canonicalize(&path).map(strip_verbatim).unwrap_or_else(|_| PathBuf::from(&path));
    core.store.set_trusted(&root.to_string_lossy(), true);
    Ok(())
}

/// Folder yang diminta lewat baris perintah: argumen pertama yang merupakan direktori.
/// `args[0]` adalah nama program; dipakai saat aplikasi dijalankan dan saat instance kedua
/// meneruskan argumennya.
pub fn folder_argument(args: &[String], cwd: &str) -> Option<String> {
    args.iter().skip(1).filter(|a| !a.starts_with('-')).find_map(|arg| {
        let path = Path::new(arg);
        let full = if path.is_absolute() { path.to_path_buf() } else { Path::new(cwd).join(path) };
        full.is_dir().then(|| full.to_string_lossy().into_owned())
    })
}

pub async fn recent_remove(core: &Shared, path: String) -> AppResult<()> {
    core.store.remove_recent(&path);
    Ok(())
}

/// Membatalkan operasi panjang (clone, fetch, push, pencarian) berdasarkan id-nya.
pub async fn app_cancel(core: &Shared, operation_id: String) -> AppResult<()> {
    core.cancel_operation(&operation_id);
    Ok(())
}

/// Jawaban pengguna atas [`CoreEvent::Prompt`]; `None` berarti dibatalkan.
pub async fn prompt_respond(core: &Shared, id: String, value: Option<String>) -> AppResult<()> {
    core.respond_prompt(&id, value);
    Ok(())
}

pub async fn settings_get(core: &Shared, window: &str) -> AppResult<Settings> {
    Ok(core.effective_settings(window))
}

/// Pengaturan tingkat pengguna (tanpa penimpaan proyek), untuk halaman pengaturan.
pub async fn settings_get_user(core: &Shared) -> AppResult<Settings> {
    Ok(core.user_settings())
}

/// Menyimpan pengaturan pengguna lalu menyiarkan [`CoreEvent::SettingsChanged`] satu kali;
/// tiap jendela mengambil pengaturan efektifnya sendiri lewat [`settings_get`].
pub async fn settings_set(core: &Shared, settings: Settings) -> AppResult<()> {
    core.set_user_settings(settings)?;
    core.emit(CoreEvent::SettingsChanged { settings: core.user_settings() });
    Ok(())
}

/// Nilai bawaan semua pengaturan (untuk "Reset to defaults").
pub async fn settings_defaults() -> AppResult<Settings> {
    Ok(Settings::default())
}

/// Isi `keybindings.json` pengguna: objek JSON `{ "<id command>": "<pintasan>" | ["..."] }`.
/// Berkas yang belum ada menghasilkan `{}`.
pub async fn keybindings_get(core: &Shared) -> AppResult<String> {
    match std::fs::read_to_string(core.keybindings_path()) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok("{}".to_string()),
        Err(e) => Err(e.into()),
    }
}

/// Membuat `keybindings.json` (bila belum ada) agar bisa dibuka di editor, dan mengembalikan path-nya.
pub async fn keybindings_ensure(core: &Shared) -> AppResult<String> {
    let path = core.keybindings_path();
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, "// User keyboard shortcuts. Example:\n// { \"file.save\": \"Mod+S\", \"workbench.togglePanel\": [] }\n{}\n")?;
    }
    Ok(path.to_string_lossy().into_owned())
}

/// Sesi tersimpan (tab terbuka, tata letak) untuk sebuah proyek, sebagai JSON milik lapisan UI.
pub async fn session_get(core: &Shared, root: String) -> AppResult<Option<String>> {
    Ok(core.store.session(&root))
}

pub async fn session_set(core: &Shared, root: String, json: String) -> AppResult<()> {
    if json.len() > 512 * 1024 {
        return Err(AppError::invalid("Session data is too large"));
    }
    core.store.set_session(&root, json);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{git, recv, TestCore};

    #[test]
    fn strips_windows_verbatim_prefix() {
        assert_eq!(strip_verbatim(PathBuf::from(r"\\?\C:\kode")), PathBuf::from(r"C:\kode"));
        assert_eq!(strip_verbatim(PathBuf::from(r"\\?\UNC\server\share")), PathBuf::from(r"\\?\UNC\server\share"));
        assert_eq!(strip_verbatim(PathBuf::from("/home/a")), PathBuf::from("/home/a"));
    }

    #[test]
    fn picks_first_directory_argument() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("proyek");
        std::fs::create_dir(&sub).unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let args = vec!["oncode".to_string(), "--flag".to_string(), "tidak-ada".to_string(), "proyek".to_string()];
        assert_eq!(folder_argument(&args, &cwd), Some(sub.to_string_lossy().into_owned()));
        assert_eq!(folder_argument(&["oncode".to_string()], &cwd), None);
    }

    #[tokio::test]
    async fn opening_a_folder_reports_trust_and_records_it_as_recent() {
        let t = TestCore::new();
        let project = t.project("proyek");
        git(&project, &["init", "-q", "-b", "main"]);
        let path = project.to_string_lossy().into_owned();

        // Mode terbatas: repositori tidak dibuka walaupun folder berisi `.git`.
        let info = workspace_open(&t.core, "w1", path.clone()).await.unwrap();
        assert_eq!((info.root.as_str(), info.name.as_str(), info.trusted, info.repo_root.as_deref()), (path.as_str(), "proyek", false, None));
        assert_eq!(app_info(&t.core).await.unwrap().recent, vec![path.clone()]);

        let info = workspace_set_trusted(&t.core, "w1", true).await.unwrap();
        assert!(info.trusted);
        assert_eq!(info.repo_root.as_deref(), Some(path.as_str()));

        let info = workspace_set_trusted(&t.core, "w1", false).await.unwrap();
        assert_eq!((info.trusted, info.repo_root), (false, None));

        workspace_close(&t.core, "w1").await.unwrap();
        assert_eq!(workspace_set_trusted(&t.core, "w1", true).await.unwrap_err().code, "noWorkspace");

        recent_remove(&t.core, path).await.unwrap();
        assert!(app_info(&t.core).await.unwrap().recent.is_empty());
        assert_eq!(workspace_open(&t.core, "w1", t.dir.path().join("tidak-ada").to_string_lossy().into_owned()).await.unwrap_err().code, "notFound");
    }

    #[tokio::test]
    async fn app_info_reports_paths_and_git() {
        let t = TestCore::new();
        let info = app_info(&t.core).await.unwrap();
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(info.platform, std::env::consts::OS);
        assert!(info.git.is_some(), "git tersedia di mesin uji: {:?}", info.git_error);
        assert_eq!(Path::new(&info.settings_path), t.dir.path().join("app/config/settings.json"));
        assert_eq!(Path::new(&info.keybindings_path), t.dir.path().join("app/config/keybindings.json"));
    }

    #[tokio::test]
    async fn trust_path_marks_a_folder_without_opening_it() {
        let t = TestCore::new();
        let project = t.project("klon");
        workspace_trust_path(&t.core, project.to_string_lossy().into_owned()).await.unwrap();
        assert!(t.core.store.is_trusted(&project));
        assert!(t.core.workspace_root("w1").is_err());
    }

    #[tokio::test]
    async fn saving_settings_persists_and_broadcasts() {
        let mut t = TestCore::new();
        assert_eq!(settings_get_user(&t.core).await.unwrap(), settings_defaults().await.unwrap());
        let mut settings = Settings::default();
        settings.editor.font_size = 19;
        settings_set(&t.core, settings.clone()).await.unwrap();

        let broadcast = recv(&mut t.events, |e| match e {
            CoreEvent::SettingsChanged { settings } => Some(settings),
            _ => None,
        })
        .await;
        assert_eq!(broadcast, settings);
        assert_eq!(settings_get(&t.core, "w1").await.unwrap(), settings);
        assert_eq!(Settings::load(t.core.settings_path()), settings, "format settings.json di disk tetap sama");
    }

    #[tokio::test]
    async fn keybindings_file_is_created_on_demand() {
        let t = TestCore::new();
        assert_eq!(keybindings_get(&t.core).await.unwrap(), "{}");
        let path = keybindings_ensure(&t.core).await.unwrap();
        assert_eq!(Path::new(&path), t.core.keybindings_path());
        let text = keybindings_get(&t.core).await.unwrap();
        assert!(text.starts_with("// User keyboard shortcuts.") && text.ends_with("{}\n"));
        // Berkas yang sudah ada tidak ditimpa.
        std::fs::write(&path, r#"{ "file.save": "Mod+S" }"#).unwrap();
        keybindings_ensure(&t.core).await.unwrap();
        assert_eq!(keybindings_get(&t.core).await.unwrap(), r#"{ "file.save": "Mod+S" }"#);
    }

    #[tokio::test]
    async fn sessions_are_stored_per_root_with_a_size_limit() {
        let t = TestCore::new();
        assert_eq!(session_get(&t.core, "/a".into()).await.unwrap(), None);
        session_set(&t.core, "/a".into(), "{\"tabs\":[]}".into()).await.unwrap();
        assert_eq!(session_get(&t.core, "/a".into()).await.unwrap().as_deref(), Some("{\"tabs\":[]}"));
        let err = session_set(&t.core, "/a".into(), "x".repeat(512 * 1024 + 1)).await.unwrap_err();
        assert_eq!(err.code, "invalidInput");
    }

    #[tokio::test]
    async fn cancel_and_prompt_respond_reach_the_core() {
        let t = TestCore::new();
        let token = t.core.begin_operation("op");
        app_cancel(&t.core, "op".into()).await.unwrap();
        assert!(token.is_cancelled());
        // Id permintaan yang tidak dikenal bukan galat.
        prompt_respond(&t.core, "tidak-ada".into(), None).await.unwrap();
    }
}
