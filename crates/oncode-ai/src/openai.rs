//! Custom OpenAI-compatible endpoints (Chat Completions): OpenAI, OpenRouter, Groq,
//! DeepSeek, local Ollama/LM Studio, LiteLLM, and similar gateways. `POST {base}/chat/completions`
//! with `stream: true`; `base` usually ends in `/v1`.

use std::fmt;
use std::time::Duration;

use serde_json::{json, Value};

use crate::{clean_message, user_message, AiError, AiErrorCode, CommitPrompt, SseParser, SYSTEM_PROMPT};

/// Access to an OpenAI-compatible endpoint.
#[derive(Clone)]
pub struct OpenAiConfig {
    /// E.g. `https://api.openai.com/v1` or `http://localhost:11434/v1`.
    pub base_url: String,
    pub model: String,
    /// Sent as `Authorization: Bearer`; empty for local servers without a key.
    pub api_key: Option<String>,
}

impl fmt::Debug for OpenAiConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiConfig").field("base_url", &self.base_url).field("model", &self.model).field("api_key", &self.api_key.as_ref().map(|_| "***")).finish()
    }
}

/// Body of the Chat Completions request. No sampling parameters or token limit: their names and
/// support differ between providers.
pub fn request_body(config: &OpenAiConfig, prompt: &CommitPrompt) -> Value {
    json!({
        "model": config.model,
        "stream": true,
        "messages": [
            { "role": "system", "content": SYSTEM_PROMPT },
            { "role": "user", "content": user_message(prompt) },
        ],
    })
}

/// Error from the endpoint's HTTP status (neutral message: the provider could be anyone).
fn status_error(status: u16, detail: Option<&str>, retry_after: Option<u64>) -> AiError {
    let detail = detail.map(str::trim).filter(|d| !d.is_empty());
    match status {
        401 | 403 => AiError::new(
            AiErrorCode::Unauthorized,
            format!("The endpoint rejected the API key{}", detail.map(|d| format!(": {d}")).unwrap_or_else(|| ".".into())),
        ),
        429 => AiError::new(
            AiErrorCode::RateLimited,
            match retry_after {
                Some(seconds) => format!("The endpoint's rate limit was reached. Try again in {seconds} seconds."),
                None => "The endpoint's rate limit was reached. Try again in a moment.".to_string(),
            },
        ),
        500.. => AiError::new(AiErrorCode::Overloaded, format!("The endpoint had a problem (HTTP {status}){}", detail.map(|d| format!(": {d}")).unwrap_or_else(|| ". Try again in a moment.".into()))),
        _ => AiError::new(
            AiErrorCode::InvalidRequest,
            match detail {
                Some(detail) => format!("The endpoint rejected the request: {detail}"),
                None => format!("The endpoint rejected the request (HTTP {status}). Check the base URL and model."),
            },
        ),
    }
}

fn transport_error(err: reqwest::Error) -> AiError {
    let err = err.without_url();
    let message = if err.is_timeout() {
        "Timed out while waiting for the endpoint. Check that it is running and reachable.".to_string()
    } else if err.is_connect() {
        "Could not connect to the endpoint. Check the base URL and that the server is running.".to_string()
    } else {
        format!("The request to the endpoint failed: {err}")
    };
    AiError::new(AiErrorCode::Network, message)
}

/// Removes the `<think>…</think>` blocks that some reasoning models write (e.g. through Ollama).
fn strip_thinking(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</think>") {
            Some(end) => rest = &rest[start + end + "</think>".len()..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

#[derive(Debug, Default)]
struct StreamState {
    text: String,
    finished: bool,
}

/// A single Chat Completions SSE event; returns the new text chunk if there is one.
fn apply_data(state: &mut StreamState, data: &str) -> Result<Option<String>, AiError> {
    let data = data.trim();
    if data.is_empty() {
        return Ok(None);
    }
    if data == "[DONE]" {
        state.finished = true;
        return Ok(None);
    }
    let value: Value = serde_json::from_str(data).map_err(|_| AiError::new(AiErrorCode::InvalidResponse, "The endpoint sent an answer OnCode could not read"))?;
    if let Some(error) = value.get("error") {
        let message = error["message"].as_str().or_else(|| error.as_str()).unwrap_or("unknown error");
        return Err(AiError::new(AiErrorCode::InvalidRequest, format!("The endpoint reported an error: {message}")));
    }
    let choice = &value["choices"][0];
    if choice["finish_reason"].as_str().is_some() {
        state.finished = true;
    }
    let piece = choice["delta"]["content"].as_str().unwrap_or_default();
    if piece.is_empty() {
        return Ok(None);
    }
    state.text.push_str(piece);
    Ok(Some(piece.to_string()))
}

fn finish(state: StreamState) -> Result<String, AiError> {
    if !state.finished && state.text.is_empty() {
        return Err(AiError::new(AiErrorCode::Network, "The connection closed before the answer was complete. Try again."));
    }
    let message = clean_message(&strip_thinking(&state.text));
    if message.is_empty() {
        return Err(AiError::new(AiErrorCode::InvalidResponse, "The endpoint returned an empty message. Try again."));
    }
    Ok(message)
}

/// Writes a commit message through an OpenAI-compatible endpoint; `on_text` receives each chunk.
pub async fn stream_commit_message(config: &OpenAiConfig, prompt: &CommitPrompt, mut on_text: impl FnMut(&str)) -> Result<String, AiError> {
    let client = reqwest::Client::builder()
        .user_agent("OnCode")
        .connect_timeout(Duration::from_secs(10))
        // A local model can take a long time before the first token (loading the model into memory).
        .read_timeout(Duration::from_secs(300))
        .build()
        .map_err(transport_error)?;
    let url = format!("{}/chat/completions", config.base_url.trim().trim_end_matches('/'));
    let mut request = client.post(url).json(&request_body(config, prompt));
    if let Some(key) = config.api_key.as_deref().map(str::trim).filter(|key| !key.is_empty()) {
        request = request.bearer_auth(key);
    }
    let mut response = request.send().await.map_err(transport_error)?;
    let status = response.status();
    if !status.is_success() {
        let retry_after = response.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok());
        let body = response.bytes().await.unwrap_or_default();
        let value: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let message = value["error"]["message"].as_str().or_else(|| value["error"].as_str()).or_else(|| value["message"].as_str());
        return Err(status_error(status.as_u16(), message, retry_after));
    }
    let mut parser = SseParser::default();
    let mut state = StreamState::default();
    while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
        for event in parser.push(&chunk) {
            if let Some(piece) = apply_data(&mut state, &event.data)? {
                on_text(&piece);
            }
        }
    }
    finish(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt() -> CommitPrompt {
        CommitPrompt { recent_subjects: vec!["fix: a".into()], stat: "1 file".into(), patch: "+b".into(), truncated: false, draft: String::new() }
    }

    #[test]
    fn body_has_system_and_user_messages_and_no_vendor_specific_options() {
        let config = OpenAiConfig { base_url: "http://localhost:11434/v1".into(), model: "qwen2.5-coder".into(), api_key: None };
        let body = request_body(&config, &prompt());
        assert_eq!((body["model"].as_str(), body["stream"].as_bool()), (Some("qwen2.5-coder"), Some(true)));
        assert_eq!(body["messages"][0]["role"], "system");
        assert!(body["messages"][1]["content"].as_str().unwrap().contains("+b"));
        for absent in ["temperature", "max_tokens", "max_completion_tokens", "fallbacks", "output_config"] {
            assert!(body.get(absent).is_none(), "{absent}");
        }
        assert!(!format!("{:?}", OpenAiConfig { api_key: Some("sk-secret".into()), ..config }).contains("secret"));
    }

    #[test]
    fn stream_chunks_become_text_until_done() {
        let mut state = StreamState::default();
        let chunks = [
            r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":""}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"content":"fix: repair "}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"reasoning_content":"thinking"}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"content":"login"},"finish_reason":"stop"}]}"#,
            "[DONE]",
        ];
        let pieces: Vec<String> = chunks.iter().filter_map(|data| apply_data(&mut state, data).unwrap()).collect();
        assert_eq!(pieces, ["fix: repair ", "login"]);
        assert_eq!(finish(state).unwrap(), "fix: repair login");
    }

    #[test]
    fn thinking_blocks_and_errors() {
        assert_eq!(strip_thinking("<think>hmm\n</think>\nfix: a"), "\nfix: a");
        assert_eq!(strip_thinking("fix: a<think>rest"), "fix: a");
        let mut state = StreamState::default();
        let error = apply_data(&mut state, r#"{"error":{"message":"model not found"}}"#).unwrap_err();
        assert!(error.message.contains("model not found"));
        assert_eq!(finish(StreamState::default()).unwrap_err().code, AiErrorCode::Network);
    }
}
