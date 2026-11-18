//! Aplikasi, jendela, workspace, pengaturan, dan sesi.

use std::path::{Path, PathBuf};

use oncode_git::GitInfo;
use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_specta::Event;

use crate::error::{AppError, AppResult};
use crate::events::SettingsChanged;
use crate::settings::Settings;
use crate::state::Shared;

#[derive(Debug, Clone, Serialize, Type)]
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

#[derive(Debug, Clone, Serialize, Type)]
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

#[tauri::command]
#[specta::specta]
pub async fn app_info(app: AppHandle, state: State<'_, Shared>) -> AppResult<AppInfo> {
    let (git, git_error) = match state.git().await {
        Ok(git) => (Some(git.info()), None),
        Err(e) => (None, Some(e.message)),
    };
    Ok(AppInfo {
        version: app.package_info().version.to_string(),
        platform: std::env::consts::OS.to_string(),
        git,
        git_error,
        recent: state.store.recent().into_iter().filter(|p| Path::new(p).is_dir()).collect(),
        settings_path: state.settings_path().to_string_lossy().into_owned(),
        keybindings_path: state.keybindings_path().to_string_lossy().into_owned(),
        home_dir: app.path().home_dir().ok().map(|p| p.to_string_lossy().into_owned()),
    })
}

async fn workspace_info(state: &Shared, label: &str) -> AppResult<WorkspaceInfo> {
    let root = state.workspace_root(label)?;
    let trusted = state.workspace_trusted(label);
    let mut repo_root = None;
    if trusted {
        if let Ok(repo) = state.open_repo(label, &root).await {
            repo_root = Some(repo.root_string());
            state.refresh(&repo.root_string());
        }
    }
    Ok(WorkspaceInfo {
        name: root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.to_string_lossy().into_owned()),
        root: root.to_string_lossy().into_owned(),
        trusted,
        repo_root,
        settings: state.effective_settings(label),
    })
}

/// Membuka folder di jendela ini.
#[tauri::command]
#[specta::specta]
pub async fn workspace_open(window: WebviewWindow, state: State<'_, Shared>, path: String) -> AppResult<WorkspaceInfo> {
    let root = std::fs::canonicalize(&path).map_err(|e| AppError::new("notFound", format!("Cannot open folder: {e}")))?;
    if !root.is_dir() {
        return Err(AppError::new("notFound", format!("Not a folder: {path}")));
    }
    let root = strip_verbatim(root);
    let shared = state.inner().clone();
    let label = window.label().to_string();
    let build_root = root.clone();
    let build_label = label.clone();
    // Membangun indeks berkas bisa lama pada proyek besar; jangan menahan thread async.
    tokio::task::spawn_blocking(move || shared.set_workspace(&build_label, build_root))
        .await
        .map_err(|e| AppError::new("io", e.to_string()))?;
    state.store.add_recent(&root.to_string_lossy());
    let info = workspace_info(state.inner(), &label).await?;
    let _ = window.set_title(&format!("{} — OnCode", info.name));
    Ok(info)
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
#[tauri::command]
#[specta::specta]
pub async fn workspace_set_trusted(window: WebviewWindow, state: State<'_, Shared>, trusted: bool) -> AppResult<WorkspaceInfo> {
    let root = state.workspace_root(window.label())?;
    state.store.set_trusted(&root.to_string_lossy(), trusted);
    workspace_open(window, state, root.to_string_lossy().into_owned()).await
}

#[tauri::command]
#[specta::specta]
pub async fn workspace_close(window: WebviewWindow, state: State<'_, Shared>) -> AppResult<()> {
    state.close_workspace(window.label());
    let _ = window.set_title("OnCode");
    Ok(())
}

/// Menandai folder sebagai dipercaya tanpa membukanya (dipakai setelah clone/init dari aplikasi).
#[tauri::command]
#[specta::specta]
pub async fn workspace_trust_path(state: State<'_, Shared>, path: String) -> AppResult<()> {
    let root = std::fs::canonicalize(&path).map(strip_verbatim).unwrap_or_else(|_| PathBuf::from(&path));
    state.store.set_trusted(&root.to_string_lossy(), true);
    Ok(())
}

/// Membuat jendela baru; `folder` langsung dibuka di sana.
pub fn open_window(app: &AppHandle, folder: Option<&str>) -> tauri::Result<()> {
    let label = format!("w{}", uuid::Uuid::new_v4().simple());
    let url = match folder {
        Some(folder) => format!("index.html?folder={}", encode_component(folder)),
        None => "index.html".to_string(),
    };
    WebviewWindowBuilder::new(app, label, WebviewUrl::App(url.into()))
        .title("OnCode")
        .inner_size(1280.0, 800.0)
        .min_inner_size(720.0, 480.0)
        .build()?;
    Ok(())
}

/// Percent-encoding untuk komponen URL (setara `encodeURIComponent`).
fn encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[tauri::command]
#[specta::specta]
pub async fn window_open(app: AppHandle, folder: Option<String>) -> AppResult<()> {
    open_window(&app, folder.as_deref()).map_err(Into::into)
}

#[tauri::command]
#[specta::specta]
pub async fn recent_remove(state: State<'_, Shared>, path: String) -> AppResult<()> {
    state.store.remove_recent(&path);
    Ok(())
}

/// Membatalkan operasi panjang (clone, fetch, push, pencarian) berdasarkan id-nya.
#[tauri::command]
#[specta::specta]
pub async fn app_cancel(state: State<'_, Shared>, operation_id: String) -> AppResult<()> {
    state.cancel_operation(&operation_id);
    Ok(())
}

/// Jawaban pengguna atas `PromptRequest`; `None` berarti dibatalkan.
#[tauri::command]
#[specta::specta]
pub async fn prompt_respond(state: State<'_, Shared>, id: String, value: Option<String>) -> AppResult<()> {
    state.respond_prompt(&id, value);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn settings_get(window: WebviewWindow, state: State<'_, Shared>) -> AppResult<Settings> {
    Ok(state.effective_settings(window.label()))
}

/// Pengaturan tingkat pengguna (tanpa penimpaan proyek), untuk halaman pengaturan.
#[tauri::command]
#[specta::specta]
pub async fn settings_get_user(state: State<'_, Shared>) -> AppResult<Settings> {
    Ok(state.user_settings())
}

#[tauri::command]
#[specta::specta]
pub async fn settings_set(app: AppHandle, state: State<'_, Shared>, settings: Settings) -> AppResult<()> {
    state.set_user_settings(settings)?;
    for (label, _) in app.webview_windows() {
        let _ = SettingsChanged { settings: state.effective_settings(&label) }.emit_to(&app, label.as_str());
    }
    Ok(())
}

/// Nilai bawaan semua pengaturan (untuk "Reset to defaults").
#[tauri::command]
#[specta::specta]
pub async fn settings_defaults() -> AppResult<Settings> {
    Ok(Settings::default())
}

/// Isi `keybindings.json` pengguna: objek JSON `{ "<id command>": "<pintasan>" | ["..."] }`.
/// Berkas yang belum ada menghasilkan `{}`.
#[tauri::command]
#[specta::specta]
pub async fn keybindings_get(state: State<'_, Shared>) -> AppResult<String> {
    match std::fs::read_to_string(state.keybindings_path()) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok("{}".to_string()),
        Err(e) => Err(e.into()),
    }
}

/// Membuat `keybindings.json` (bila belum ada) agar bisa dibuka di editor, dan mengembalikan path-nya.
#[tauri::command]
#[specta::specta]
pub async fn keybindings_ensure(state: State<'_, Shared>) -> AppResult<String> {
    let path = state.keybindings_path();
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, "// User keyboard shortcuts. Example:\n// { \"file.save\": \"Mod+S\", \"workbench.togglePanel\": [] }\n{}\n")?;
    }
    Ok(path.to_string_lossy().into_owned())
}

/// Sesi tersimpan (tab terbuka, tata letak) untuk sebuah proyek, sebagai JSON milik frontend.
#[tauri::command]
#[specta::specta]
pub async fn session_get(state: State<'_, Shared>, root: String) -> AppResult<Option<String>> {
    Ok(state.store.session(&root))
}

#[tauri::command]
#[specta::specta]
pub async fn session_set(state: State<'_, Shared>, root: String, json: String) -> AppResult<()> {
    if json.len() > 512 * 1024 {
        return Err(AppError::invalid("Session data is too large"));
    }
    state.store.set_session(&root, json);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_paths_for_urls() {
        assert_eq!(encode_component("/home/a/proyek saya"), "%2Fhome%2Fa%2Fproyek%20saya");
        assert_eq!(encode_component("C:\\kode\\é"), "C%3A%5Ckode%5C%C3%A9");
    }

    #[test]
    fn strips_windows_verbatim_prefix() {
        assert_eq!(strip_verbatim(PathBuf::from(r"\\?\C:\kode")), PathBuf::from(r"C:\kode"));
        assert_eq!(strip_verbatim(PathBuf::from(r"\\?\UNC\server\share")), PathBuf::from(r"\\?\UNC\server\share"));
        assert_eq!(strip_verbatim(PathBuf::from("/home/a")), PathBuf::from("/home/a"));
    }
}
