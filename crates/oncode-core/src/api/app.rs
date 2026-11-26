//! App, workspace, settings, and sessions.

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
    /// `linux`, `windows`, or `macos`.
    pub platform: String,
    pub git: Option<GitInfo>,
    /// Why Git features are unavailable (git is missing or too old).
    pub git_error: Option<String>,
    pub recent: Vec<String>,
    pub settings_path: String,
    /// The user's keyboard shortcut override file.
    pub keybindings_path: String,
    pub home_dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceInfo {
    pub root: String,
    pub name: String,
    /// `false` = restricted mode: Git, LSP, and project settings are disabled.
    pub trusted: bool,
    /// Root of the repository containing this folder, if any and the folder is trusted.
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

/// Opens a folder in this window. The window title (`<name> — OnCode`) is set by the UI layer from `name`.
pub async fn workspace_open(core: &Shared, window: &str, path: String) -> AppResult<WorkspaceInfo> {
    let root = std::fs::canonicalize(&path).map_err(|e| AppError::new("notFound", format!("Cannot open folder: {e}")))?;
    if !root.is_dir() {
        return Err(AppError::new("notFound", format!("Not a folder: {path}")));
    }
    let root = strip_verbatim(root);
    let shared = core.clone();
    let build_root = root.clone();
    let build_label = window.to_string();
    // Building the file index can take a while on a large project; don't hold up the async thread.
    core.runtime()
        .spawn_blocking(move || shared.set_workspace(&build_label, build_root))
        .await
        .map_err(|e| AppError::new("io", e.to_string()))?;
    core.store.add_recent(&root.to_string_lossy());
    workspace_info(core, window).await
}

/// Windows: `canonicalize` produces a `\\?\C:\...` path that many programs do not understand.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => path,
    }
}

/// Trusts (or stops trusting) this window's folder, then reloads the workspace.
pub async fn workspace_set_trusted(core: &Shared, window: &str, trusted: bool) -> AppResult<WorkspaceInfo> {
    let root = core.workspace_root(window)?;
    core.store.set_trusted(&root.to_string_lossy(), trusted);
    workspace_open(core, window, root.to_string_lossy().into_owned()).await
}

/// Closes this window's folder; also called by the UI when the window is closed.
pub async fn workspace_close(core: &Shared, window: &str) -> AppResult<()> {
    core.close_workspace(window);
    Ok(())
}

/// Marks a folder as trusted without opening it (used after clone/init from the app).
pub async fn workspace_trust_path(core: &Shared, path: String) -> AppResult<()> {
    let root = std::fs::canonicalize(&path).map(strip_verbatim).unwrap_or_else(|_| PathBuf::from(&path));
    core.store.set_trusted(&root.to_string_lossy(), true);
    Ok(())
}

/// The folder requested on the command line: the first argument that is a directory.
/// `args[0]` is the program name; used when the app starts and when a second instance
/// forwards its arguments.
pub fn folder_argument(args: &[String], cwd: &str) -> Option<String> {
    args.iter().skip(1).filter(|a| !a.starts_with('-')).find_map(|arg| {
        let path = Path::new(arg);
        let full = if path.is_absolute() { path.to_path_buf() } else { Path::new(cwd).join(path) };
        full.is_dir().then(|| full.to_string_lossy().into_owned())
    })
}

/// Installs (`install`) or removes the `oncode` command in the terminal, returning a message for
/// the user. Done by `oncode-helper` because the Windows installer uses the same step before the
/// app itself has been copied.
pub async fn shell_command_set(install: bool) -> AppResult<String> {
    let helper = crate::state::helper_path().ok_or_else(|| {
        AppError::new("notFound", "oncode-helper was not found next to the application. Reinstall OnCode or run `cargo build -p oncode-helper` when building from source.")
    })?;
    let app_dir = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)).ok_or_else(|| AppError::new("io", "Cannot find the application folder"))?;
    let output = super::hidden_command(&helper)
        .arg(if install { "install-shell-command" } else { "uninstall-shell-command" })
        .arg(&app_dir)
        .output()
        .await
        .map_err(|e| AppError::new("io", format!("Cannot run oncode-helper: {e}")))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(AppError::new("shellCommand", if stderr.is_empty() { "Changing the 'oncode' command failed".to_string() } else { stderr }))
}

pub async fn recent_remove(core: &Shared, path: String) -> AppResult<()> {
    core.store.remove_recent(&path);
    Ok(())
}

/// Cancels a long operation (clone, fetch, push, search) by its id.
pub async fn app_cancel(core: &Shared, operation_id: String) -> AppResult<()> {
    core.cancel_operation(&operation_id);
    Ok(())
}

/// The user's answer to a [`CoreEvent::Prompt`]; `None` means canceled.
pub async fn prompt_respond(core: &Shared, id: String, value: Option<String>) -> AppResult<()> {
    core.respond_prompt(&id, value);
    Ok(())
}

pub async fn settings_get(core: &Shared, window: &str) -> AppResult<Settings> {
    Ok(core.effective_settings(window))
}

/// User-level settings (without project overrides), for the settings page.
pub async fn settings_get_user(core: &Shared) -> AppResult<Settings> {
    Ok(core.user_settings())
}

/// Saves the user's settings and then broadcasts [`CoreEvent::SettingsChanged`] once;
/// each window fetches its own effective settings through [`settings_get`].
pub async fn settings_set(core: &Shared, settings: Settings) -> AppResult<()> {
    core.set_user_settings(settings)?;
    core.emit(CoreEvent::SettingsChanged { settings: core.user_settings() });
    Ok(())
}

/// Default values of all settings (for "Reset to defaults").
pub async fn settings_defaults() -> AppResult<Settings> {
    Ok(Settings::default())
}

/// Contents of the user's `keybindings.json`: a JSON object `{ "<command id>": "<shortcut>" | ["..."] }`.
/// A file that does not exist yet yields `{}`.
pub async fn keybindings_get(core: &Shared) -> AppResult<String> {
    match std::fs::read_to_string(core.keybindings_path()) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok("{}".to_string()),
        Err(e) => Err(e.into()),
    }
}

/// Creates `keybindings.json` (if it does not exist yet) so it can be opened in the editor, and returns its path.
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

/// The saved session (open tabs, layout) for a project, as JSON owned by the UI layer.
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
        assert_eq!(strip_verbatim(PathBuf::from(r"\\?\C:\code")), PathBuf::from(r"C:\code"));
        assert_eq!(strip_verbatim(PathBuf::from(r"\\?\UNC\server\share")), PathBuf::from(r"\\?\UNC\server\share"));
        assert_eq!(strip_verbatim(PathBuf::from("/home/a")), PathBuf::from("/home/a"));
    }

    #[test]
    fn picks_first_directory_argument() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("project");
        std::fs::create_dir(&sub).unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let args = vec!["oncode".to_string(), "--flag".to_string(), "does-not-exist".to_string(), "project".to_string()];
        assert_eq!(folder_argument(&args, &cwd), Some(sub.to_string_lossy().into_owned()));
        assert_eq!(folder_argument(&["oncode".to_string()], &cwd), None);
    }

    #[tokio::test]
    async fn opening_a_folder_reports_trust_and_records_it_as_recent() {
        let t = TestCore::new();
        let project = t.project("project");
        git(&project, &["init", "-q", "-b", "main"]);
        let path = project.to_string_lossy().into_owned();

        // Restricted mode: the repository is not opened even though the folder contains `.git`.
        let info = workspace_open(&t.core, "w1", path.clone()).await.unwrap();
        assert_eq!((info.root.as_str(), info.name.as_str(), info.trusted, info.repo_root.as_deref()), (path.as_str(), "project", false, None));
        assert_eq!(app_info(&t.core).await.unwrap().recent, vec![path.clone()]);

        let info = workspace_set_trusted(&t.core, "w1", true).await.unwrap();
        assert!(info.trusted);
        // The repository root comes from Git (`C:/...` on Windows); compared as paths.
        assert_eq!(info.repo_root.as_deref().map(std::path::Path::new), Some(project.as_path()));

        let info = workspace_set_trusted(&t.core, "w1", false).await.unwrap();
        assert_eq!((info.trusted, info.repo_root), (false, None));

        workspace_close(&t.core, "w1").await.unwrap();
        assert_eq!(workspace_set_trusted(&t.core, "w1", true).await.unwrap_err().code, "noWorkspace");

        recent_remove(&t.core, path).await.unwrap();
        assert!(app_info(&t.core).await.unwrap().recent.is_empty());
        assert_eq!(workspace_open(&t.core, "w1", t.dir.path().join("does-not-exist").to_string_lossy().into_owned()).await.unwrap_err().code, "notFound");
    }

    #[tokio::test]
    async fn app_info_reports_paths_and_git() {
        let t = TestCore::new();
        let info = app_info(&t.core).await.unwrap();
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(info.platform, std::env::consts::OS);
        assert!(info.git.is_some(), "git is available on the test machine: {:?}", info.git_error);
        assert_eq!(Path::new(&info.settings_path), t.dir.path().join("app/config/settings.json"));
        assert_eq!(Path::new(&info.keybindings_path), t.dir.path().join("app/config/keybindings.json"));
    }

    #[tokio::test]
    async fn trust_path_marks_a_folder_without_opening_it() {
        let t = TestCore::new();
        let project = t.project("clone");
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
        assert_eq!(Settings::load(t.core.settings_path()), settings, "the settings.json format on disk stays the same");
    }

    #[tokio::test]
    async fn keybindings_file_is_created_on_demand() {
        let t = TestCore::new();
        assert_eq!(keybindings_get(&t.core).await.unwrap(), "{}");
        let path = keybindings_ensure(&t.core).await.unwrap();
        assert_eq!(Path::new(&path), t.core.keybindings_path());
        let text = keybindings_get(&t.core).await.unwrap();
        assert!(text.starts_with("// User keyboard shortcuts.") && text.ends_with("{}\n"));
        // An existing file is not overwritten.
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
        // An unknown request id is not an error.
        prompt_respond(&t.core, "does-not-exist".into(), None).await.unwrap();
    }
}
