//! Fitur AI: menulis pesan commit dengan Claude dari perubahan yang di-stage.
//!
//! API key Anthropic disimpan di keychain OS (sama seperti token akun forge), dengan variabel
//! lingkungan `ANTHROPIC_API_KEY` sebagai cadangan. Diff hanya dikirim saat pengguna memintanya.

use oncode_ai::{ClaudeConfig, CommitPrompt, DEFAULT_BASE_URL, DEFAULT_MODEL};
use oncode_forge::TokenSet;
use serde::Serialize;

use super::forge::keychain;
use super::Callback;
use crate::error::{AppError, AppResult};
use crate::events::NoticeKind;
use crate::state::Shared;

/// Id API key Anthropic di keychain.
const KEY_ID: &str = "ai:anthropic";
/// Batas patch yang dikirim (± 30 ribu token); ringkasan `--stat` selalu dikirim utuh.
pub const MAX_DIFF_BYTES: usize = 120 * 1024;
/// Jumlah subjek commit terbaru sebagai contoh gaya penulisan repo.
const RECENT_SUBJECTS: u32 = 10;

async fn api_key(core: &Shared) -> AppResult<Option<String>> {
    let stored = keychain(core, |store| store.load(KEY_ID)).await?.map(|tokens| tokens.access_token);
    Ok(stored.or_else(|| std::env::var("ANTHROPIC_API_KEY").ok()).map(|key| key.trim().to_string()).filter(|key| !key.is_empty()))
}

/// Menyimpan API key Anthropic di keychain.
pub async fn ai_set_api_key(core: &Shared, key: String) -> AppResult<()> {
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err(AppError::invalid("The API key is empty"));
    }
    if key.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(AppError::invalid("An API key cannot contain spaces"));
    }
    keychain(core, move |store| store.save(KEY_ID, &TokenSet::new(key))).await?;
    if core.tokens.is_memory_only() {
        core.notify(
            NoticeKind::Warning,
            "No system keychain is available, so the API key is kept in memory only. You will need to set it again after restarting OnCode.",
        );
    }
    Ok(())
}

pub async fn ai_remove_api_key(core: &Shared) -> AppResult<()> {
    keychain(core, |store| store.delete(KEY_ID)).await
}

/// `true` bila ada API key, di keychain atau di `ANTHROPIC_API_KEY`.
pub async fn ai_has_api_key(core: &Shared) -> AppResult<bool> {
    Ok(api_key(core).await?.is_some())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitSuggestion {
    pub message: String,
    /// Diff terlalu panjang dan hanya sebagian yang dikirim.
    pub truncated: bool,
}

/// Menulis pesan commit untuk perubahan yang di-stage. `draft` adalah awal pesan yang sudah
/// diketik pengguna (boleh kosong); jawaban diawali teks itu. `on_text` menerima tiap potongan
/// teks begitu tiba, dari thread mana pun.
///
/// Galat `noApiKey` berarti belum ada API key (lihat [`ai_set_api_key`]); `nothingStaged` berarti
/// tidak ada yang di-stage; `unauthorized` berarti key ditolak.
pub async fn ai_commit_message(core: &Shared, root: String, draft: String, on_text: Callback<String>) -> AppResult<CommitSuggestion> {
    let Some(api_key) = api_key(core).await? else {
        return Err(AppError::new("noApiKey", "Set an Anthropic API key to generate commit messages"));
    };
    let repo = core.repo(&root)?;
    let diff = repo.staged_diff(MAX_DIFF_BYTES).await?;
    if diff.stat.trim().is_empty() {
        return Err(AppError::new("nothingStaged", "Stage the changes you want to commit first"));
    }
    let recent_subjects = repo.recent_subjects(RECENT_SUBJECTS).await?;
    let settings = core.user_settings().ai;
    let config = ClaudeConfig {
        api_key,
        model: Some(settings.model.trim()).filter(|model| !model.is_empty()).unwrap_or(DEFAULT_MODEL).to_string(),
        base_url: settings.base_url.filter(|url| !url.trim().is_empty()).unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
    };
    let prompt = CommitPrompt { recent_subjects, stat: diff.stat, patch: diff.patch, truncated: diff.truncated, draft };
    let message = oncode_ai::stream_commit_message(&config, &prompt, |piece| on_text(piece.to_string())).await?;
    Ok(CommitSuggestion { message, truncated: diff.truncated })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::api::git::git_open;
    use crate::testing::{git, TestCore};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn sse(events: &[serde_json::Value]) -> String {
        events.iter().map(|e| format!("event: {}\ndata: {}\n\n", e["type"].as_str().unwrap(), e)).collect()
    }

    #[tokio::test]
    async fn api_key_is_validated_stored_and_removed() {
        let t = TestCore::new();
        assert_eq!(ai_set_api_key(&t.core, "  ".into()).await.unwrap_err().code, "invalidInput");
        assert_eq!(ai_set_api_key(&t.core, "sk ant".into()).await.unwrap_err().code, "invalidInput");
        ai_set_api_key(&t.core, " sk-ant-uji \n".into()).await.unwrap();
        assert!(ai_has_api_key(&t.core).await.unwrap());
        assert_eq!(api_key(&t.core).await.unwrap().as_deref(), Some("sk-ant-uji"));
        ai_remove_api_key(&t.core).await.unwrap();
        if std::env::var_os("ANTHROPIC_API_KEY").is_none() {
            assert!(!ai_has_api_key(&t.core).await.unwrap());
        }
    }

    #[tokio::test]
    async fn commit_message_needs_a_key_and_staged_changes() {
        let t = TestCore::new();
        let project = t.trusted_repo("proyek");
        let root = git_open(&t.core, "w1", project.to_string_lossy().into_owned()).await.unwrap();
        if std::env::var_os("ANTHROPIC_API_KEY").is_none() {
            let error = ai_commit_message(&t.core, root.clone(), String::new(), Arc::new(|_| {})).await.unwrap_err();
            assert_eq!(error.code, "noApiKey");
        }
        ai_set_api_key(&t.core, "sk-ant-uji".into()).await.unwrap();
        let error = ai_commit_message(&t.core, root, String::new(), Arc::new(|_| {})).await.unwrap_err();
        assert_eq!(error.code, "nothingStaged");
        assert_eq!(ai_commit_message(&t.core, "/tidak/dibuka".into(), String::new(), Arc::new(|_| {})).await.unwrap_err().code, "notARepo");
    }

    #[tokio::test]
    async fn commit_message_streams_from_claude_with_the_staged_diff() {
        let server = MockServer::start().await;
        let body = sse(&[
            serde_json::json!({"type": "message_start", "message": {"id": "msg_1"}}),
            serde_json::json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "feat: tambah "}}),
            serde_json::json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "berkas catatan"}}),
            serde_json::json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}}),
            serde_json::json!({"type": "message_stop"}),
        ]);
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("x-api-key", "sk-ant-uji"))
            .respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(body))
            .expect(1)
            .mount(&server)
            .await;

        let t = TestCore::new();
        let mut settings = t.core.user_settings();
        settings.ai.base_url = Some(server.uri());
        settings.ai.model = "claude-sonnet-5-5".into();
        t.core.set_user_settings(settings).unwrap();
        ai_set_api_key(&t.core, "sk-ant-uji".into()).await.unwrap();

        let project = t.trusted_repo("proyek");
        std::fs::write(project.join("a.txt"), "a\n").unwrap();
        git(&project, &["add", "a.txt"]);
        git(&project, &["commit", "-q", "-m", "feat: mulai proyek"]);
        std::fs::write(project.join("catatan.md"), "# Catatan\n").unwrap();
        git(&project, &["add", "catatan.md"]);
        let root = git_open(&t.core, "w1", project.to_string_lossy().into_owned()).await.unwrap();

        let pieces = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = pieces.clone();
        let suggestion = ai_commit_message(&t.core, root, "feat:".into(), Arc::new(move |piece| sink.lock().unwrap().push(piece))).await.unwrap();
        assert_eq!(suggestion, CommitSuggestion { message: "feat: tambah berkas catatan".into(), truncated: false });
        assert_eq!(*pieces.lock().unwrap(), ["feat: tambah ", "berkas catatan"]);

        // Permintaan memuat model dari pengaturan, diff yang di-stage, contoh subjek, dan draf.
        let requests = server.received_requests().await.unwrap();
        let request: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(request["model"], "claude-sonnet-5-5");
        let content = request["messages"][0]["content"].as_str().unwrap();
        assert!(content.contains("+# Catatan"), "{content}");
        assert!(content.contains("- feat: mulai proyek"), "{content}");
        assert!(content.contains("<draft>\nfeat:\n</draft>"), "{content}");
    }
}
