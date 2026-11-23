//! Language server: backend hanya mengelola proses dan meneruskan pesan JSON-RPC apa adanya.

use oncode_lsp::ServerSpec;
use serde::Serialize;

use crate::api::Callback;
use crate::error::{AppError, AppResult};
use crate::settings::Settings;
use crate::state::Shared;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LspServerInfo {
    pub language: String,
    pub command: String,
    pub args: Vec<String>,
    pub installed: bool,
    /// Petunjuk singkat cara memasang server bila belum ada.
    pub install_hint: String,
}

#[derive(Debug, Clone, Serialize)]
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

pub async fn lsp_servers(core: &Shared, window: &str) -> AppResult<Vec<LspServerInfo>> {
    let settings = core.effective_settings(window);
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
pub async fn lsp_start(core: &Shared, window: &str, language: String, on_event: Callback<LspEvent>) -> AppResult<u32> {
    if !core.workspace_trusted(window) {
        return Err(AppError::untrusted());
    }
    let settings = core.effective_settings(window);
    if !settings.lsp.enabled {
        return Err(AppError::new("disabled", "LSP is disabled in settings"));
    }
    let (spec, _) = server_for(&settings, &language)
        .ok_or_else(|| AppError::new("notFound", format!("No language server available for language {language}")))?;
    let root = core.workspace_root(window)?;
    let messages = on_event.clone();
    let id = core
        .lsp
        .start(
            &spec,
            &root,
            move |json| {
                messages(LspEvent::Message { json });
            },
            move |exit| {
                on_event(LspEvent::Exit { code: exit.code, stderr_tail: exit.stderr_tail });
            },
        )
        .await?;
    let _ = core.with_workspace(window, |ws| ws.servers.push(id));
    Ok(id)
}

pub async fn lsp_send(core: &Shared, id: u32, json: String) -> AppResult<()> {
    Ok(core.lsp.send(id, &json).await?)
}

pub async fn lsp_stop(core: &Shared, window: &str, id: u32) -> AppResult<()> {
    core.lsp.stop(id).await;
    let _ = core.with_workspace(window, |ws| ws.servers.retain(|s| *s != id));
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::api::app::workspace_open;
    use crate::testing::TestCore;

    fn noop() -> Callback<LspEvent> {
        Arc::new(|_| {})
    }

    #[tokio::test]
    async fn start_requires_a_trusted_folder_enabled_lsp_and_a_known_language() {
        let t = TestCore::new();
        let project = t.project("proyek");
        let path = project.to_string_lossy().into_owned();
        workspace_open(&t.core, "w1", path.clone()).await.unwrap();
        assert_eq!(lsp_start(&t.core, "w1", "rust".into(), noop()).await.unwrap_err().code, "untrusted");
        // Jendela tanpa folder juga dianggap tidak dipercaya.
        assert_eq!(lsp_start(&t.core, "lain", "rust".into(), noop()).await.unwrap_err().code, "untrusted");

        t.core.store.set_trusted(&path, true);
        workspace_open(&t.core, "w1", path.clone()).await.unwrap();
        assert_eq!(lsp_start(&t.core, "w1", "bahasa-tak-dikenal".into(), noop()).await.unwrap_err().code, "notFound");

        let mut settings = t.core.user_settings();
        settings.lsp.enabled = false;
        t.core.set_user_settings(settings).unwrap();
        assert_eq!(lsp_start(&t.core, "w1", "rust".into(), noop()).await.unwrap_err().code, "disabled");
    }

    #[tokio::test]
    async fn user_servers_override_and_extend_the_defaults() {
        let t = TestCore::new();
        let mut settings = t.core.user_settings();
        settings.lsp.servers.insert("rust".into(), ServerSpec { command: "ra-uji-tidak-terpasang".into(), args: vec!["--stdio".into()], env: Vec::new() });
        settings.lsp.servers.insert("bahasa-uji".into(), ServerSpec { command: "server-uji-tidak-terpasang".into(), ..Default::default() });
        t.core.set_user_settings(settings).unwrap();

        let servers = lsp_servers(&t.core, "w1").await.unwrap();
        let rust = servers.iter().find(|s| s.language == "rust").unwrap();
        assert_eq!((rust.command.as_str(), rust.args.as_slice(), rust.installed), ("ra-uji-tidak-terpasang", &["--stdio".to_string()][..], false));
        assert!(!rust.install_hint.is_empty(), "petunjuk pemasangan tetap dari daftar bawaan");
        let custom = servers.iter().find(|s| s.language == "bahasa-uji").unwrap();
        assert_eq!((custom.command.as_str(), custom.install_hint.as_str()), ("server-uji-tidak-terpasang", ""));
        assert!(servers.len() > oncode_lsp::default_servers().len());
    }

    #[cfg(unix)]
    async fn next(rx: &mut tokio::sync::mpsc::UnboundedReceiver<LspEvent>) -> LspEvent {
        tokio::time::timeout(std::time::Duration::from_secs(20), rx.recv()).await.expect("server tidak menjawab").expect("kanal tertutup")
    }

    /// `cat` memantulkan setiap pesan berbingkai apa adanya, jadi cukup sebagai server tiruan.
    #[cfg(unix)]
    #[tokio::test]
    async fn messages_round_trip_through_a_running_server() {
        let t = TestCore::new();
        let project = t.project("proyek");
        let path = project.to_string_lossy().into_owned();
        t.core.store.set_trusted(&path, true);
        let mut settings = t.core.user_settings();
        settings.lsp.servers.insert("gema".into(), ServerSpec { command: "cat".into(), ..Default::default() });
        t.core.set_user_settings(settings).unwrap();
        workspace_open(&t.core, "w1", path).await.unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let id = lsp_start(
            &t.core,
            "w1",
            "gema".into(),
            Arc::new(move |event| {
                let _ = tx.send(event);
            }),
        )
        .await
        .unwrap();
        assert_eq!(t.core.with_workspace("w1", |ws| ws.servers.clone()).unwrap(), vec![id]);

        let json = r#"{"jsonrpc":"2.0","id":1,"method":"uji"}"#;
        lsp_send(&t.core, id, json.into()).await.unwrap();
        assert!(matches!(next(&mut rx).await, LspEvent::Message { json: echoed } if echoed == json));

        lsp_stop(&t.core, "w1", id).await.unwrap();
        assert!(matches!(next(&mut rx).await, LspEvent::Exit { .. }));
        assert!(t.core.with_workspace("w1", |ws| ws.servers.clone()).unwrap().is_empty());
        assert_eq!(lsp_send(&t.core, id, json.into()).await.unwrap_err().code, "notFound");
    }
}
