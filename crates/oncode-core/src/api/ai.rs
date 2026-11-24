//! Fitur AI: menulis pesan commit dengan Claude dari perubahan yang di-stage.
//!
//! Dua jalur (pengaturan `ai.provider`):
//! - Claude API dengan API key Anthropic. Key disimpan di keychain OS (sama seperti token akun
//!   forge), dengan variabel lingkungan `ANTHROPIC_API_KEY` sebagai cadangan.
//! - Claude Code CLI (`claude -p`) dengan login Claude Code pengguna, tanpa API key.
//!
//! Diff hanya dikirim saat pengguna memintanya.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use oncode_ai::claude_code::{self, ClaudeCodeStream};
use oncode_ai::{ClaudeConfig, CommitPrompt, DEFAULT_BASE_URL, DEFAULT_MODEL};
use oncode_forge::TokenSet;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};

use super::forge::keychain;
use super::Callback;
use crate::error::{AppError, AppResult};
use crate::events::NoticeKind;
use crate::settings::{AiProvider, AiSettings, ApiFormat};
use crate::state::Shared;

/// Batas waktu satu kali menulis pesan lewat Claude Code CLI (termasuk waktu start CLI).
const CLAUDE_CODE_TIMEOUT: Duration = Duration::from_secs(180);

/// Id API key Anthropic di keychain.
const KEY_ID: &str = "ai:anthropic";
/// Id API key endpoint kustom di keychain (opsional: server lokal tidak memerlukannya).
const CUSTOM_KEY_ID: &str = "ai:custom";
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
    save_key(core, KEY_ID, key).await
}

/// Menyimpan API key endpoint kustom di keychain.
pub async fn ai_set_custom_api_key(core: &Shared, key: String) -> AppResult<()> {
    save_key(core, CUSTOM_KEY_ID, key).await
}

pub async fn ai_remove_custom_api_key(core: &Shared) -> AppResult<()> {
    keychain(core, |store| store.delete(CUSTOM_KEY_ID)).await
}

async fn custom_api_key(core: &Shared) -> AppResult<Option<String>> {
    Ok(keychain(core, |store| store.load(CUSTOM_KEY_ID)).await?.map(|tokens| tokens.access_token.trim().to_string()).filter(|key| !key.is_empty()))
}

async fn save_key(core: &Shared, id: &'static str, key: String) -> AppResult<()> {
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err(AppError::invalid("The API key is empty"));
    }
    if key.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(AppError::invalid("An API key cannot contain spaces"));
    }
    keychain(core, move |store| store.save(id, &TokenSet::new(key))).await?;
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

/// Program `claude` (Claude Code CLI), bila terpasang: di PATH, lalu di lokasi pemasangan
/// bawaannya. Cadangan itu perlu bila PATH shell login tidak terbaca (aplikasi dibuka dari
/// Finder/Dock hanya mendapat PATH minimal).
pub fn claude_code_program() -> Option<PathBuf> {
    oncode_lsp::find_on_path("claude").or_else(|| {
        let home = crate::dirs::home_dir()?;
        claude_code_install_paths(&home).iter().find_map(|path| oncode_lsp::find_on_path(&path.to_string_lossy()))
    })
}

/// Lokasi pemasangan Claude Code: installer native (`~/.local/bin`) dan pemasangan lokal lama
/// (`~/.claude/local`). Di Windows ekstensi `.exe`/`.cmd` dicoba oleh `find_on_path`.
fn claude_code_install_paths(home: &Path) -> [PathBuf; 2] {
    [home.join(".local").join("bin").join("claude"), home.join(".claude").join("local").join("claude")]
}

/// Endpoint kustom yang sudah lengkap (alamat dan model terisi).
#[derive(Debug, Clone, PartialEq, Eq)]
struct CustomEndpoint {
    format: ApiFormat,
    base_url: String,
    model: String,
    key: Option<String>,
}

impl CustomEndpoint {
    fn from_settings(settings: &AiSettings, key: Option<String>) -> Option<Self> {
        let filled = |value: &Option<String>| value.as_deref().map(str::trim).filter(|value| !value.is_empty()).map(String::from);
        Some(Self { format: settings.custom_format, base_url: filled(&settings.custom_base_url)?, model: filled(&settings.custom_model)?, key })
    }
}

/// Jalur yang dipakai untuk menulis pesan commit.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Route {
    Api(String),
    ClaudeCode(PathBuf),
    Custom(CustomEndpoint),
}

/// Memilih jalur menurut pengaturan, key yang tersedia, dan CLI yang terpasang.
fn choose_route(provider: AiProvider, key: Option<String>, claude: Option<PathBuf>, custom: Option<CustomEndpoint>) -> AppResult<Route> {
    let no_key = || AppError::new("noApiKey", "Set an Anthropic API key (or install Claude Code and sign in) to generate commit messages");
    let no_cli = || {
        AppError::new(
            "claudeCodeNotFound",
            "Claude Code (the `claude` command) was not found on PATH. Install it and sign in, or choose Anthropic API in Settings → AI → Provider.",
        )
    };
    match provider {
        AiProvider::Api => key.map(Route::Api).ok_or_else(no_key),
        AiProvider::ClaudeCode => claude.map(Route::ClaudeCode).ok_or_else(no_cli),
        AiProvider::Custom => custom.map(Route::Custom).ok_or_else(|| {
            AppError::new("customEndpointIncomplete", "Set the base URL and model of the custom endpoint in Settings → AI to generate commit messages")
        }),
        AiProvider::Auto => match (key, claude) {
            (Some(key), _) => Ok(Route::Api(key)),
            (None, Some(claude)) => Ok(Route::ClaudeCode(claude)),
            (None, None) => Err(no_key()),
        },
    }
}

/// Galat endpoint kustom: key yang ditolak diberi kode sendiri agar UI meminta key endpoint itu,
/// bukan key Anthropic.
fn custom_error(error: oncode_ai::AiError) -> AppError {
    if error.code == oncode_ai::AiErrorCode::Unauthorized {
        let reason = error.message.trim_end_matches('.');
        return AppError::new("customUnauthorized", format!("{reason}. Set it with \"AI: Set Custom Endpoint API Key…\"."));
    }
    error.into()
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
    let settings = core.user_settings().ai;
    let key = if matches!(settings.provider, AiProvider::Api | AiProvider::Auto) { api_key(core).await? } else { None };
    let claude = if matches!(settings.provider, AiProvider::ClaudeCode | AiProvider::Auto) { claude_code_program() } else { None };
    let custom = if settings.provider == AiProvider::Custom { CustomEndpoint::from_settings(&settings, custom_api_key(core).await?) } else { None };
    let route = choose_route(settings.provider, key, claude, custom)?;
    let repo = core.repo(&root)?;
    let diff = repo.staged_diff(MAX_DIFF_BYTES).await?;
    if diff.stat.trim().is_empty() {
        return Err(AppError::new("nothingStaged", "Stage the changes you want to commit first"));
    }
    let recent_subjects = repo.recent_subjects(RECENT_SUBJECTS).await?;
    let model = Some(settings.model.trim()).filter(|model| !model.is_empty()).unwrap_or(DEFAULT_MODEL).to_string();
    let prompt = CommitPrompt { recent_subjects, stat: diff.stat, patch: diff.patch, truncated: diff.truncated, draft };
    let message = match route {
        Route::Api(api_key) => {
            let base_url = settings.base_url.filter(|url| !url.trim().is_empty()).unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
            let config = ClaudeConfig { api_key, model, base_url };
            oncode_ai::stream_commit_message(&config, &prompt, |piece| on_text(piece.to_string())).await?
        }
        Route::ClaudeCode(program) => {
            // System prompt lewat berkas: argumen panjang bermultibaris rawan di Windows (shim `.cmd`).
            let prompt_file = std::env::temp_dir().join(format!("oncode-commit-prompt-{}.txt", uuid::Uuid::new_v4()));
            std::fs::write(&prompt_file, claude_code::system_prompt())?;
            let args = claude_code::args(&prompt_file.to_string_lossy(), Some(&model));
            let result = run_claude_code(&program, &args, claude_code::input(&prompt), |piece| on_text(piece)).await;
            let _ = std::fs::remove_file(&prompt_file);
            result?
        }
        Route::Custom(endpoint) => match endpoint.format {
            ApiFormat::OpenAi => {
                let config = oncode_ai::openai::OpenAiConfig { base_url: endpoint.base_url, model: endpoint.model, api_key: endpoint.key };
                oncode_ai::openai::stream_commit_message(&config, &prompt, |piece| on_text(piece.to_string())).await.map_err(custom_error)?
            }
            ApiFormat::Anthropic => {
                let config = ClaudeConfig { api_key: endpoint.key.unwrap_or_default(), model: endpoint.model, base_url: endpoint.base_url };
                oncode_ai::stream_commit_message(&config, &prompt, |piece| on_text(piece.to_string())).await.map_err(custom_error)?
            }
        },
    };
    Ok(CommitSuggestion { message, truncated: diff.truncated })
}

/// Menjalankan Claude Code CLI dari folder sementara (bukan folder proyek: mode `-p` melewati
/// dialog kepercayaan folder Claude Code) dan mengalirkan teks jawabannya.
async fn run_claude_code(program: &Path, args: &[String], input: String, on_text: impl Fn(String)) -> AppResult<String> {
    let failed = |detail: String| AppError::new("claudeCodeFailed", format!("Claude Code could not write the message: {detail}"));
    let mut child = super::hidden_command(program)
        .args(args)
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| failed(error.to_string()))?;
    let mut stdin = child.stdin.take().expect("stdin dipipa");
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(input.as_bytes()).await;
    });
    let stderr = child.stderr.take().expect("stderr dipipa");
    let errors = tokio::spawn(async move {
        let mut bytes = Vec::new();
        let _ = stderr.take(16 * 1024).read_to_end(&mut bytes).await;
        String::from_utf8_lossy(&bytes).into_owned()
    });
    let mut lines = BufReader::new(child.stdout.take().expect("stdout dipipa")).lines();
    let mut stream = ClaudeCodeStream::default();
    let read = async {
        while let Some(line) = lines.next_line().await? {
            if let Some(piece) = stream.push_line(&line) {
                on_text(piece);
            }
        }
        child.wait().await
    };
    let status = match tokio::time::timeout(CLAUDE_CODE_TIMEOUT, read).await {
        Ok(status) => status.map_err(|error| failed(error.to_string()))?,
        Err(_) => return Err(failed(format!("it took longer than {} seconds", CLAUDE_CODE_TIMEOUT.as_secs()))),
    };
    let _ = writer.await;
    let stderr = errors.await.unwrap_or_default();
    Ok(stream.finish(status.success(), &stderr)?)
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

    #[test]
    fn route_follows_the_provider_setting() {
        let key = || Some("sk-ant".to_string());
        let cli = || Some(PathBuf::from("claude"));
        // Otomatis: key didahulukan, tanpa key pakai Claude Code.
        assert_eq!(choose_route(AiProvider::Auto, key(), cli(), None).unwrap(), Route::Api("sk-ant".into()));
        assert_eq!(choose_route(AiProvider::Auto, None, cli(), None).unwrap(), Route::ClaudeCode("claude".into()));
        assert_eq!(choose_route(AiProvider::Auto, None, None, None).unwrap_err().code, "noApiKey");
        // Pilihan tegas tidak beralih ke jalur lain.
        assert_eq!(choose_route(AiProvider::ClaudeCode, key(), cli(), None).unwrap(), Route::ClaudeCode("claude".into()));
        assert_eq!(choose_route(AiProvider::ClaudeCode, key(), None, None).unwrap_err().code, "claudeCodeNotFound");
        assert_eq!(choose_route(AiProvider::Api, None, cli(), None).unwrap_err().code, "noApiKey");
        assert_eq!(choose_route(AiProvider::Custom, key(), cli(), None).unwrap_err().code, "customEndpointIncomplete");
    }

    #[test]
    fn custom_endpoint_needs_both_address_and_model() {
        let mut settings = AiSettings { provider: AiProvider::Custom, custom_base_url: Some(" http://localhost:11434/v1 ".into()), ..AiSettings::default() };
        assert_eq!(CustomEndpoint::from_settings(&settings, None), None, "model belum diisi");
        settings.custom_model = Some("qwen2.5-coder".into());
        let endpoint = CustomEndpoint::from_settings(&settings, None).unwrap();
        assert_eq!((endpoint.format, endpoint.base_url.as_str(), endpoint.model.as_str()), (ApiFormat::OpenAi, "http://localhost:11434/v1", "qwen2.5-coder"));
    }

    #[tokio::test]
    async fn custom_openai_compatible_endpoint_writes_the_message() {
        use wiremock::matchers::body_partial_json;
        let server = MockServer::start().await;
        let chunk = |text: &str| format!("data: {}\n\n", serde_json::json!({"choices": [{"index": 0, "delta": {"content": text}}]}));
        let body = format!("{}{}data: [DONE]\n\n", chunk("docs: tambah "), chunk("catatan"));
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header("authorization", "Bearer kunci-kustom"))
            .and(body_partial_json(serde_json::json!({"model": "model-kustom"})))
            .respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(body))
            .expect(1)
            .mount(&server)
            .await;
        let t = TestCore::new();
        let mut settings = t.core.user_settings();
        settings.ai.provider = AiProvider::Custom;
        settings.ai.custom_base_url = Some(format!("{}/v1", server.uri()));
        settings.ai.custom_model = Some("model-kustom".into());
        t.core.set_user_settings(settings).unwrap();
        ai_set_custom_api_key(&t.core, "kunci-kustom".into()).await.unwrap();

        let project = t.trusted_repo("proyek");
        std::fs::write(project.join("catatan.md"), "# Catatan\n").unwrap();
        git(&project, &["add", "catatan.md"]);
        let root = git_open(&t.core, "w1", project.to_string_lossy().into_owned()).await.unwrap();
        let suggestion = ai_commit_message(&t.core, root.clone(), String::new(), Arc::new(|_| {})).await.unwrap();
        assert_eq!(suggestion.message, "docs: tambah catatan");

        // Key yang ditolak endpoint meminta key endpoint itu, bukan key Anthropic.
        server.reset().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({"error": {"message": "invalid key"}}))).mount(&server).await;
        let error = ai_commit_message(&t.core, root, String::new(), Arc::new(|_| {})).await.unwrap_err();
        assert_eq!(error.code, "customUnauthorized", "{}", error.message);
        ai_remove_custom_api_key(&t.core).await.unwrap();
        assert_eq!(custom_api_key(&t.core).await.unwrap(), None);
    }

    /// Proses tiruan yang mencetak keluaran stream-json Claude Code dari sebuah berkas.
    fn fake_claude(lines: &[serde_json::Value]) -> (tempfile::TempDir, PathBuf, Vec<String>) {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("keluaran.jsonl");
        std::fs::write(&output, lines.iter().map(|line| format!("{line}\n")).collect::<String>()).unwrap();
        let output = output.to_string_lossy().into_owned();
        if cfg!(windows) {
            (dir, PathBuf::from("cmd"), vec!["/C".into(), "type".into(), output])
        } else {
            (dir, PathBuf::from("cat"), vec![output])
        }
    }

    #[tokio::test]
    async fn claude_code_output_is_streamed_and_its_errors_reported() {
        let delta = |text: &str| serde_json::json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}}});
        let (_dir, program, args) = fake_claude(&[
            serde_json::json!({"type": "system", "subtype": "init"}),
            delta("feat: tambah "),
            delta("catatan"),
            serde_json::json!({"type": "result", "subtype": "success", "is_error": false, "result": "feat: tambah catatan", "stop_reason": "end_turn"}),
        ]);
        let pieces = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = pieces.clone();
        let message = run_claude_code(&program, &args, "diff".into(), move |piece| sink.lock().unwrap().push(piece)).await.unwrap();
        assert_eq!(message, "feat: tambah catatan");
        assert_eq!(*pieces.lock().unwrap(), ["feat: tambah ", "catatan"]);

        let (_dir, program, args) = fake_claude(&[serde_json::json!({"type": "result", "subtype": "success", "is_error": true, "result": "Not logged in · Please run /login"})]);
        let error = run_claude_code(&program, &args, String::new(), |_| {}).await.unwrap_err();
        assert_eq!(error.code, "claudeCodeFailed");
        assert!(error.message.contains("Please run /login"), "{}", error.message);

        let error = run_claude_code(Path::new("program-yang-tidak-ada-xyz"), &[], String::new(), |_| {}).await.unwrap_err();
        assert_eq!(error.code, "claudeCodeFailed");
    }

    /// Ujung-ke-ujung dengan Claude Code sungguhan (memakai kuota pengguna): jalankan manual dengan
    /// `cargo test -p oncode-core real_claude_code -- --ignored`.
    #[tokio::test]
    #[ignore = "memanggil Claude Code CLI sungguhan dan memakai kuotanya"]
    async fn real_claude_code_writes_a_commit_message() {
        if claude_code_program().is_none() {
            return;
        }
        let t = TestCore::new();
        let mut settings = t.core.user_settings();
        settings.ai.provider = AiProvider::ClaudeCode;
        settings.ai.model = "haiku".into();
        t.core.set_user_settings(settings).unwrap();
        let project = t.trusted_repo("proyek");
        std::fs::write(project.join("README.md"), "# Proyek

Cara memasang: jalankan `cargo build`.
").unwrap();
        git(&project, &["add", "README.md"]);
        let root = git_open(&t.core, "w1", project.to_string_lossy().into_owned()).await.unwrap();

        let pieces = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = pieces.clone();
        let suggestion = ai_commit_message(&t.core, root, String::new(), Arc::new(move |piece| sink.lock().unwrap().push(piece))).await.unwrap();
        println!("pesan: {:?} ({} potongan)", suggestion.message, pieces.lock().unwrap().len());
        assert!(!suggestion.message.is_empty() && !suggestion.message.contains("```"));
        assert!(suggestion.message.lines().next().unwrap().chars().count() <= 72, "subjek terlalu panjang");
        assert!(!pieces.lock().unwrap().is_empty(), "teks dialirkan");
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
        // Jalur API secara tegas: dengan "Otomatis", Claude Code yang terpasang di mesin uji akan dipakai.
        let mut settings = t.core.user_settings();
        settings.ai.provider = AiProvider::Api;
        t.core.set_user_settings(settings).unwrap();
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
