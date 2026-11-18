//! Language server: backend hanya mengelola proses dan meneruskan pesan JSON-RPC apa adanya.

use oncode_lsp::ServerSpec;
use serde::Serialize;
use specta::Type;
use tauri::ipc::Channel;
use tauri::{State, WebviewWindow};

use crate::error::{AppError, AppResult};
use crate::settings::Settings;
use crate::state::Shared;

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LspServerInfo {
    pub language: String,
    pub command: String,
    pub args: Vec<String>,
    pub installed: bool,
    /// Petunjuk singkat cara memasang server bila belum ada.
    pub install_hint: String,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LspEvent {
    Message { json: String },
    #[serde(rename_all = "camelCase")]
    Exit { code: Option<i32>, stderr_tail: String },
}

/// Server untuk sebuah bahasa: pengaturan pengguna lebih diutamakan daripada daftar bawaan.
fn server_for(settings: &Settings, language: &str) -> Option<(ServerSpec, String)> {
    let default = oncode_lsp::default_servers().into_iter().find(|d| d.language == language);
    match settings.lsp.servers.get(language) {
        Some(spec) => Some((spec.clone(), default.map(|d| d.install_hint).unwrap_or_default())),
        None => default.map(|d| (d.spec, d.install_hint)),
    }
}

#[tauri::command]
#[specta::specta]
pub async fn lsp_servers(window: WebviewWindow, state: State<'_, Shared>) -> AppResult<Vec<LspServerInfo>> {
    let settings = state.effective_settings(window.label());
    let mut languages: Vec<String> = oncode_lsp::default_servers().into_iter().map(|d| d.language).collect();
    for language in settings.lsp.servers.keys() {
        if !languages.contains(language) {
            languages.push(language.clone());
        }
    }
    Ok(languages
        .into_iter()
        .filter_map(|language| {
            let (spec, install_hint) = server_for(&settings, &language)?;
            Some(LspServerInfo {
                installed: oncode_lsp::find_on_path(&spec.command).is_some(),
                language,
                command: spec.command,
                args: spec.args,
                install_hint,
            })
        })
        .collect())
}

/// Memulai language server untuk `language` di folder proyek. Hanya pada folder yang dipercaya.
#[tauri::command]
#[specta::specta]
pub async fn lsp_start(window: WebviewWindow, state: State<'_, Shared>, language: String, on_event: Channel<LspEvent>) -> AppResult<u32> {
    let label = window.label().to_string();
    if !state.workspace_trusted(&label) {
        return Err(AppError::untrusted());
    }
    let settings = state.effective_settings(&label);
    if !settings.lsp.enabled {
        return Err(AppError::new("disabled", "LSP is disabled in settings"));
    }
    let (spec, _) = server_for(&settings, &language)
        .ok_or_else(|| AppError::new("notFound", format!("No language server available for language {language}")))?;
    let root = state.workspace_root(&label)?;
    let messages = on_event.clone();
    let id = state
        .lsp
        .start(
            &spec,
            &root,
            move |json| {
                let _ = messages.send(LspEvent::Message { json });
            },
            move |exit| {
                let _ = on_event.send(LspEvent::Exit { code: exit.code, stderr_tail: exit.stderr_tail });
            },
        )
        .await?;
    let _ = state.with_workspace(&label, |ws| ws.servers.push(id));
    Ok(id)
}

#[tauri::command]
#[specta::specta]
pub async fn lsp_send(state: State<'_, Shared>, id: u32, json: String) -> AppResult<()> {
    Ok(state.lsp.send(id, &json).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn lsp_stop(window: WebviewWindow, state: State<'_, Shared>, id: u32) -> AppResult<()> {
    state.lsp.stop(id).await;
    let _ = state.with_workspace(window.label(), |ws| ws.servers.retain(|s| *s != id));
    Ok(())
}
