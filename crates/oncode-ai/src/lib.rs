//! Klien Claude API untuk OnCode: menulis pesan commit dari diff yang di-stage.
//!
//! Anthropic tidak punya SDK Rust resmi, jadi Messages API (`POST /v1/messages`) dipanggil lewat
//! HTTP biasa dengan stream Server-Sent Events.
//!
//! - [`request_body`]: badan permintaan (murni, diuji).
//! - [`SseParser`]: pengurai Server-Sent Events.
//! - [`stream_commit_message`]: mengirim permintaan dan mengalirkan teks jawabannya.
//!
//! API key tidak pernah muncul di keluaran `Debug` maupun pesan galat.

use std::fmt;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

/// Model bawaan.
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
/// Alamat Claude API bawaan.
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2026-10-21";
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// Batas token keluaran. Thinking adaptif (selalu menyala di Opus 5.5) ikut dihitung, jadi
/// batasnya longgar walaupun pesan commit pendek.
const MAX_TOKENS: u32 = 16_000;

const SYSTEM_PROMPT: &str = "You write Git commit messages for staged changes.

Reply with the commit message only: no code fences, quotes, or commentary before or after it.

Match the style of the repository's recent commit subjects when they are given: language, capitalization, and whether they use prefixes such as \"feat:\" or \"fix:\". Without examples, write in English.

Write the subject line in the imperative mood, aiming for 50 characters and never more than 72. Add a body, separated from the subject by a blank line and wrapped at 72 columns, only when the change needs explaining (why it was made, or what is not obvious from the diff). Most small changes need only the subject.

Describe what the diff actually changes, and do not mention anything that is not in it.

If the user has started the message, begin your reply with exactly the text they wrote and continue from there.";

/// Akses ke Claude API.
#[derive(Clone)]
pub struct ClaudeConfig {
    pub api_key: String,
    pub model: String,
    /// Origin API tanpa `/v1`, mis. [`DEFAULT_BASE_URL`] atau gateway/proxy milik organisasi.
    pub base_url: String,
}

impl fmt::Debug for ClaudeConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClaudeConfig").field("api_key", &"***").field("model", &self.model).field("base_url", &self.base_url).finish()
    }
}

/// Bahan untuk menulis pesan commit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitPrompt {
    /// Subjek commit terbaru, terbaru lebih dulu; contoh gaya penulisan repo.
    pub recent_subjects: Vec<String>,
    /// Keluaran `git diff --cached --stat`.
    pub stat: String,
    /// Keluaran `git diff --cached` (mungkin dipotong, lihat `truncated`).
    pub patch: String,
    pub truncated: bool,
    /// Awal pesan yang sudah diketik pengguna; kosong bila belum ada.
    pub draft: String,
}

/// Kode galat yang dipakai UI untuk memilih pesan dan aksi lanjutan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AiErrorCode {
    /// API key salah atau tidak berlaku (HTTP 401/403).
    Unauthorized,
    /// Batas laju terlampaui (HTTP 429).
    RateLimited,
    /// Claude API sedang sibuk atau bermasalah (HTTP 5xx/529, atau event `overloaded_error`).
    Overloaded,
    /// Claude menolak permintaan (`stop_reason: "refusal"`), termasuk setelah fallback.
    Refused,
    /// Gagal terhubung, waktu habis, atau koneksi putus di tengah jawaban.
    Network,
    /// Permintaan ditolak API (mis. nama model salah, diff terlalu besar).
    InvalidRequest,
    /// Jawaban API tidak berbentuk seperti yang diharapkan.
    InvalidResponse,
}

/// Galat lapisan AI. `message` aman ditampilkan ke pengguna.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{message}")]
pub struct AiError {
    pub code: AiErrorCode,
    pub message: String,
}

impl AiError {
    pub fn new(code: AiErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

/// `true` untuk model yang menerima `fallbacks` server-side: bila Claude menolak karena
/// pengaman (`stop_reason: "refusal"`), API meneruskan permintaan ke model fallback dalam
/// panggilan yang sama.
pub fn supports_server_fallbacks(model: &str) -> bool {
    matches!(model, "claude-opus-5-5" | "claude-opus-5" | "claude-sonnet-5-5" | "claude-fable-5-1" | "claude-fable-5-2")
}

/// Isi pesan pengguna: contoh subjek, stat, diff, dan draf.
fn user_message(prompt: &CommitPrompt) -> String {
    let mut text = String::new();
    if !prompt.recent_subjects.is_empty() {
        text.push_str("Recent commit subjects in this repository, newest first:\n");
        for subject in &prompt.recent_subjects {
            text.push_str("- ");
            text.push_str(subject);
            text.push('\n');
        }
        text.push('\n');
    }
    text.push_str("Summary of the staged changes (git diff --cached --stat):\n");
    text.push_str(prompt.stat.trim_end());
    text.push_str("\n\nStaged diff:\n<diff>\n");
    text.push_str(prompt.patch.trim_end());
    text.push_str("\n</diff>\n");
    if prompt.truncated {
        text.push_str("\nThe diff above was cut off because it is long. Use the summary for the files it does not show.\n");
    }
    if prompt.draft.trim().is_empty() {
        text.push_str("\nWrite the commit message.");
    } else {
        text.push_str("\nThe user has started the message. Begin your reply with exactly this text and continue it:\n<draft>\n");
        text.push_str(&prompt.draft);
        text.push_str("\n</draft>");
    }
    text
}

/// Badan permintaan `POST /v1/messages`. Thinking tidak diatur (adaptif secara bawaan) dan
/// effort `low` karena tugasnya ringkas; parameter sampling tidak dikirim.
pub fn request_body(config: &ClaudeConfig, prompt: &CommitPrompt) -> Value {
    let mut body = json!({
        "model": config.model,
        "max_tokens": MAX_TOKENS,
        "stream": true,
        "output_config": { "effort": "low" },
        "system": SYSTEM_PROMPT,
        "messages": [{ "role": "user", "content": user_message(prompt) }],
    });
    if supports_server_fallbacks(&config.model) {
        body["fallbacks"] = json!("default");
    }
    body
}

/// Satu event Server-Sent Events.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
}

/// Pengurai Server-Sent Events bertahap: potongan byte boleh terbelah di mana saja, termasuk
/// di tengah karakter UTF-8 multi-byte.
#[derive(Debug, Default)]
pub struct SseParser {
    buffer: Vec<u8>,
    event: String,
    data: Vec<String>,
}

impl SseParser {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some(end) = self.buffer.iter().position(|&b| b == b'\n') {
            let raw: Vec<u8> = self.buffer.drain(..=end).collect();
            let line = String::from_utf8_lossy(&raw[..raw.len() - 1]);
            let line = line.strip_suffix('\r').unwrap_or(&line);
            if line.is_empty() {
                if !self.event.is_empty() || !self.data.is_empty() {
                    events.push(SseEvent { event: std::mem::take(&mut self.event), data: self.data.join("\n") });
                    self.data.clear();
                }
                continue;
            }
            if line.starts_with(':') {
                continue;
            }
            let (field, value) = match line.split_once(':') {
                Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
                None => (line, ""),
            };
            match field {
                "event" => self.event = value.to_string(),
                "data" => self.data.push(value.to_string()),
                _ => {}
            }
        }
        events
    }
}

/// Keadaan jawaban yang sedang dialirkan.
#[derive(Debug, Default)]
struct StreamState {
    text: String,
    stop_reason: Option<String>,
}

/// Menerapkan satu event; mengembalikan potongan teks baru bila ada. Hanya `text_delta` yang
/// diambil: thinking, `ping`, dan blok `fallback` diabaikan. Dengan fallback server-side, teks
/// yang sudah diterima tidak pernah dibatalkan; model fallback melanjutkannya.
fn apply_event(state: &mut StreamState, event: &SseEvent) -> Result<Option<String>, AiError> {
    if event.data.trim().is_empty() {
        return Ok(None);
    }
    let value: Value =
        serde_json::from_str(&event.data).map_err(|_| AiError::new(AiErrorCode::InvalidResponse, "The Claude API sent an answer OnCode could not read"))?;
    match value["type"].as_str() {
        Some("content_block_delta") if value["delta"]["type"] == "text_delta" => {
            let piece = value["delta"]["text"].as_str().unwrap_or_default();
            if piece.is_empty() {
                return Ok(None);
            }
            state.text.push_str(piece);
            Ok(Some(piece.to_string()))
        }
        Some("message_delta") => {
            if let Some(reason) = value["delta"]["stop_reason"].as_str() {
                state.stop_reason = Some(reason.to_string());
            }
            Ok(None)
        }
        Some("error") => Err(api_error(None, value["error"]["type"].as_str(), value["error"]["message"].as_str(), None)),
        _ => Ok(None),
    }
}

/// Membuang pagar kode dan spasi di tepi, bila model tetap menambahkannya.
fn clean_message(text: &str) -> String {
    let trimmed = text.trim();
    let unfenced = trimmed
        .strip_prefix("```")
        .and_then(|rest| rest.strip_suffix("```"))
        .map(|inner| inner.split_once('\n').map_or("", |(_, body)| body))
        .unwrap_or(trimmed);
    unfenced.trim().to_string()
}

fn finish(state: StreamState) -> Result<String, AiError> {
    match state.stop_reason.as_deref() {
        // Seluruh rantai (termasuk fallback) menolak: teks parsial dibuang.
        Some("refusal") => Err(AiError::new(AiErrorCode::Refused, "Claude declined to write a message for these changes. Write it yourself or try again.")),
        None => Err(AiError::new(AiErrorCode::Network, "The connection closed before the answer was complete. Try again.")),
        Some(_) => {
            let message = clean_message(&state.text);
            if message.is_empty() {
                return Err(AiError::new(AiErrorCode::InvalidResponse, "Claude returned an empty message. Try again."));
            }
            Ok(message)
        }
    }
}

/// Galat dari status HTTP dan/atau objek `error` API.
fn api_error(status: Option<u16>, kind: Option<&str>, detail: Option<&str>, retry_after: Option<u64>) -> AiError {
    let detail = detail.map(str::trim).filter(|d| !d.is_empty());
    match (status, kind) {
        (Some(401), _) | (_, Some("authentication_error")) => {
            AiError::new(AiErrorCode::Unauthorized, "The Anthropic API key was rejected. Set a new key with \"AI: Set Anthropic API Key…\".")
        }
        (Some(403), _) | (_, Some("permission_error")) => AiError::new(
            AiErrorCode::Unauthorized,
            format!("This Anthropic API key is not allowed to use the model{}", detail.map(|d| format!(": {d}")).unwrap_or_else(|| ".".into())),
        ),
        (Some(429), _) | (_, Some("rate_limit_error")) => AiError::new(
            AiErrorCode::RateLimited,
            match retry_after {
                Some(seconds) => format!("The Claude API rate limit was reached. Try again in {seconds} seconds."),
                None => "The Claude API rate limit was reached. Try again in a moment.".to_string(),
            },
        ),
        (Some(529), _) | (_, Some("overloaded_error")) => AiError::new(AiErrorCode::Overloaded, "Claude is overloaded right now. Try again in a moment."),
        (Some(status), _) if status >= 500 => AiError::new(AiErrorCode::Overloaded, format!("The Claude API had a problem (HTTP {status}). Try again in a moment.")),
        (_, Some("api_error")) => AiError::new(AiErrorCode::Overloaded, "The Claude API had a problem. Try again in a moment."),
        _ => AiError::new(
            AiErrorCode::InvalidRequest,
            match detail {
                Some(detail) => format!("The Claude API rejected the request: {detail}"),
                None => format!("The Claude API rejected the request (HTTP {}).", status.unwrap_or(400)),
            },
        ),
    }
}

fn transport_error(err: reqwest::Error) -> AiError {
    let err = err.without_url();
    let message = if err.is_timeout() {
        "Timed out while waiting for the Claude API. Check your connection and try again.".to_string()
    } else if err.is_connect() {
        "Could not connect to the Claude API. Check your internet connection.".to_string()
    } else {
        format!("The request to the Claude API failed: {err}")
    };
    AiError::new(AiErrorCode::Network, message)
}

/// Menulis pesan commit. `on_text` menerima tiap potongan teks begitu tiba; hasil akhirnya pesan
/// lengkap yang sudah dirapikan.
pub async fn stream_commit_message(config: &ClaudeConfig, prompt: &CommitPrompt, mut on_text: impl FnMut(&str)) -> Result<String, AiError> {
    let client = reqwest::Client::builder()
        .user_agent("OnCode")
        .connect_timeout(Duration::from_secs(10))
        // Stream mengirim `ping` secara berkala; jeda selama ini berarti koneksi macet.
        .read_timeout(Duration::from_secs(120))
        .build()
        .map_err(transport_error)?;
    let url = format!("{}/v1/messages", config.base_url.trim().trim_end_matches('/'));
    let mut request = client
        .post(url)
        .header("x-api-key", &config.api_key)
        .header("anthropic-version", API_VERSION)
        .json(&request_body(config, prompt));
    if supports_server_fallbacks(&config.model) {
        request = request.header("anthropic-beta", FALLBACK_BETA);
    }
    let mut response = request.send().await.map_err(transport_error)?;
    let status = response.status();
    if !status.is_success() {
        let retry_after = response.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok());
        let body = response.bytes().await.unwrap_or_default();
        let value: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        return Err(api_error(Some(status.as_u16()), value["error"]["type"].as_str(), value["error"]["message"].as_str(), retry_after));
    }
    let mut parser = SseParser::default();
    let mut state = StreamState::default();
    while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
        for event in parser.push(&chunk) {
            if let Some(piece) = apply_event(&mut state, &event)? {
                on_text(&piece);
            }
        }
    }
    finish(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(model: &str) -> ClaudeConfig {
        ClaudeConfig { api_key: "sk-ant-rahasia".into(), model: model.into(), base_url: DEFAULT_BASE_URL.into() }
    }

    fn prompt() -> CommitPrompt {
        CommitPrompt {
            recent_subjects: vec!["fix: perbaiki login".into(), "feat: tambah halaman profil".into()],
            stat: " src/a.rs | 2 +-\n 1 file changed".into(),
            patch: "diff --git a/src/a.rs b/src/a.rs\n-lama\n+baru\n".into(),
            truncated: false,
            draft: String::new(),
        }
    }

    #[test]
    fn debug_hides_the_api_key() {
        assert!(!format!("{:?}", config(DEFAULT_MODEL)).contains("rahasia"));
    }

    #[test]
    fn request_uses_low_effort_streaming_and_no_sampling_or_thinking_settings() {
        let body = request_body(&config("claude-opus-5-5"), &prompt());
        assert_eq!(body["model"], "claude-opus-5-5");
        assert_eq!(body["stream"], true);
        assert_eq!(body["max_tokens"], 16_000);
        assert_eq!(body["output_config"]["effort"], "low");
        assert_eq!(body["fallbacks"], "default");
        assert_eq!(body["messages"][0]["role"], "user");
        for absent in ["temperature", "top_p", "top_k", "thinking"] {
            assert!(body.get(absent).is_none(), "{absent} tidak boleh dikirim");
        }
        assert!(body["system"].as_str().unwrap().contains("commit message only"));
    }

    #[test]
    fn fallbacks_are_sent_only_to_models_that_accept_them() {
        for model in ["claude-opus-5-5", "claude-sonnet-5-5", "claude-fable-5-2"] {
            assert!(supports_server_fallbacks(model), "{model}");
        }
        for model in ["claude-haiku-5-5", "claude-opus-4-8", "model-lain"] {
            assert!(!supports_server_fallbacks(model), "{model}");
            assert!(request_body(&config(model), &prompt()).get("fallbacks").is_none(), "{model}");
        }
    }

    #[test]
    fn user_message_carries_style_examples_diff_and_draft() {
        let text = user_message(&prompt());
        assert!(text.contains("- fix: perbaiki login\n- feat: tambah halaman profil"));
        assert!(text.contains("1 file changed"));
        assert!(text.contains("<diff>\ndiff --git a/src/a.rs b/src/a.rs\n-lama\n+baru\n</diff>"));
        assert!(text.ends_with("Write the commit message."));
        assert!(!text.contains("cut off"));

        let mut with_draft = prompt();
        with_draft.draft = "fix: perbaiki".into();
        with_draft.truncated = true;
        with_draft.recent_subjects.clear();
        let text = user_message(&with_draft);
        assert!(text.contains("<draft>\nfix: perbaiki\n</draft>"));
        assert!(text.contains("cut off"));
        assert!(!text.contains("Recent commit subjects"));
    }

    #[test]
    fn sse_parser_handles_events_split_across_chunks() {
        let stream = "event: message_start\ndata: {\"type\":\"message_start\"}\n\n: komentar\nevent: content_block_delta\r\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Perbaiki é\"}}\r\n\r\n";
        let bytes = stream.as_bytes();
        let mut parser = SseParser::default();
        let mut events = Vec::new();
        // Satu byte per potongan: memotong baris, CRLF, dan karakter multi-byte.
        for byte in bytes {
            events.extend(parser.push(std::slice::from_ref(byte)));
        }
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event, "message_start");
        assert_eq!(events[1].event, "content_block_delta");
        assert!(events[1].data.contains("Perbaiki é"));
    }

    fn event(data: Value) -> SseEvent {
        SseEvent { event: data["type"].as_str().unwrap_or_default().to_string(), data: data.to_string() }
    }

    #[test]
    fn only_text_deltas_become_text() {
        let mut state = StreamState::default();
        let events = [
            json!({"type": "message_start", "message": {"model": "claude-opus-5-5"}}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": ""}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "rahasia"}}),
            json!({"type": "ping"}),
            json!({"type": "content_block_start", "index": 1, "content_block": {"type": "fallback", "from": {"model": "a"}, "to": {"model": "b"}}}),
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "text_delta", "text": "Fix login"}}),
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "text_delta", "text": " timeout"}}),
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}}),
        ];
        let pieces: Vec<String> = events.iter().filter_map(|e| apply_event(&mut state, &event(e.clone())).unwrap()).collect();
        assert_eq!(pieces, ["Fix login", " timeout"]);
        assert_eq!(finish(state).unwrap(), "Fix login timeout");
    }

    #[test]
    fn refusal_and_incomplete_streams_are_errors() {
        let refused = StreamState { text: "Sebagian".into(), stop_reason: Some("refusal".into()) };
        assert_eq!(finish(refused).unwrap_err().code, AiErrorCode::Refused);
        let cut = StreamState { text: "Sebagian".into(), stop_reason: None };
        assert_eq!(finish(cut).unwrap_err().code, AiErrorCode::Network);
        let empty = StreamState { text: "  ".into(), stop_reason: Some("end_turn".into()) };
        assert_eq!(finish(empty).unwrap_err().code, AiErrorCode::InvalidResponse);
        let mut state = StreamState::default();
        let error = apply_event(&mut state, &event(json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}))).unwrap_err();
        assert_eq!(error.code, AiErrorCode::Overloaded);
    }

    #[test]
    fn code_fences_around_the_message_are_removed() {
        assert_eq!(clean_message("```\nFix login\n\nBody\n```"), "Fix login\n\nBody");
        assert_eq!(clean_message("```text\nFix login\n```\n"), "Fix login");
        assert_eq!(clean_message("  Fix login \n"), "Fix login");
    }

    #[test]
    fn http_errors_map_to_codes() {
        assert_eq!(api_error(Some(401), Some("authentication_error"), None, None).code, AiErrorCode::Unauthorized);
        let limited = api_error(Some(429), Some("rate_limit_error"), None, Some(12));
        assert_eq!(limited.code, AiErrorCode::RateLimited);
        assert!(limited.message.contains("12 seconds"));
        assert_eq!(api_error(Some(529), None, None, None).code, AiErrorCode::Overloaded);
        assert_eq!(api_error(Some(500), Some("api_error"), None, None).code, AiErrorCode::Overloaded);
        let invalid = api_error(Some(404), Some("not_found_error"), Some("model: claude-x not found"), None);
        assert_eq!(invalid.code, AiErrorCode::InvalidRequest);
        assert!(invalid.message.contains("claude-x"));
    }
}
