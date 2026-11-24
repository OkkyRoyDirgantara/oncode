//! Uji klien Claude terhadap server HTTP tiruan: header, badan permintaan, stream SSE, dan galat.

use oncode_ai::{stream_commit_message, AiErrorCode, ClaudeConfig, CommitPrompt};
use serde_json::json;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn config(server: &MockServer, model: &str) -> ClaudeConfig {
    ClaudeConfig { api_key: "kunci-uji".into(), model: model.into(), base_url: format!("{}/", server.uri()) }
}

fn prompt() -> CommitPrompt {
    CommitPrompt { recent_subjects: vec!["fix: perbaiki login".into()], stat: " a.txt | 1 +".into(), patch: "+baru\n".into(), truncated: false, draft: String::new() }
}

/// Badan stream SSE dari daftar objek event.
fn sse(events: &[serde_json::Value]) -> String {
    events.iter().map(|e| format!("event: {}\ndata: {}\n\n", e["type"].as_str().unwrap(), e)).collect()
}

fn text_stream(pieces: &[&str], stop_reason: &str) -> String {
    let mut events = vec![
        json!({"type": "message_start", "message": {"id": "msg_1", "model": "claude-opus-5-5"}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": ""}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}),
    ];
    for piece in pieces {
        events.push(json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": piece}}));
    }
    events.push(json!({"type": "content_block_stop", "index": 1}));
    events.push(json!({"type": "message_delta", "delta": {"stop_reason": stop_reason}, "usage": {"output_tokens": 20}}));
    events.push(json!({"type": "message_stop"}));
    sse(&events)
}

fn stream_response(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(body)
}

#[tokio::test]
async fn streams_the_message_with_the_required_headers_and_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "kunci-uji"))
        .and(header("anthropic-version", "2026-10-21"))
        .and(header("anthropic-beta", "server-side-fallback-2026-07-01"))
        .and(body_partial_json(json!({"model": "claude-opus-5-5", "stream": true, "fallbacks": "default", "output_config": {"effort": "low"}})))
        .respond_with(stream_response(text_stream(&["fix: perbaiki ", "batas waktu login"], "end_turn")))
        .expect(1)
        .mount(&server)
        .await;

    let mut pieces = Vec::new();
    let message = stream_commit_message(&config(&server, "claude-opus-5-5"), &prompt(), |piece| pieces.push(piece.to_string())).await.unwrap();
    assert_eq!(message, "fix: perbaiki batas waktu login");
    assert_eq!(pieces, ["fix: perbaiki ", "batas waktu login"]);

    // Isi pesan pengguna sampai ke API.
    let requests = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    let content = body["messages"][0]["content"].as_str().unwrap();
    assert!(content.contains("- fix: perbaiki login") && content.contains("+baru"));
}

#[tokio::test]
async fn models_without_server_fallbacks_get_neither_the_header_nor_the_field() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(stream_response(text_stream(&["Fix login"], "end_turn")))
        .mount(&server)
        .await;
    let message = stream_commit_message(&config(&server, "claude-haiku-5-5"), &prompt(), |_| {}).await.unwrap();
    assert_eq!(message, "Fix login");
    let request = &server.received_requests().await.unwrap()[0];
    assert!(request.headers.get("anthropic-beta").is_none());
    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert!(body.get("fallbacks").is_none());
}

#[tokio::test]
async fn http_errors_become_typed_errors() {
    let cases = [
        (401, json!({"type": "error", "error": {"type": "authentication_error", "message": "invalid x-api-key"}}), None, AiErrorCode::Unauthorized),
        (429, json!({"type": "error", "error": {"type": "rate_limit_error", "message": "slow down"}}), Some("7"), AiErrorCode::RateLimited),
        (529, json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}), None, AiErrorCode::Overloaded),
        (404, json!({"type": "error", "error": {"type": "not_found_error", "message": "model: claude-x not found"}}), None, AiErrorCode::InvalidRequest),
    ];
    for (status, body, retry_after, code) in cases {
        let server = MockServer::start().await;
        let mut response = ResponseTemplate::new(status).set_body_json(body);
        if let Some(seconds) = retry_after {
            response = response.insert_header("retry-after", seconds);
        }
        Mock::given(method("POST")).and(path("/v1/messages")).respond_with(response).mount(&server).await;
        let error = stream_commit_message(&config(&server, "claude-opus-5-5"), &prompt(), |_| {}).await.unwrap_err();
        assert_eq!(error.code, code, "HTTP {status}: {}", error.message);
        assert!(!error.message.contains("kunci-uji"), "API key tidak boleh muncul di pesan galat");
        if status == 429 {
            assert!(error.message.contains("7 seconds"), "{}", error.message);
        }
    }
}

#[tokio::test]
async fn errors_inside_the_stream_and_refusals_stop_the_message() {
    // Event `error` di tengah stream.
    let server = MockServer::start().await;
    let body = sse(&[
        json!({"type": "message_start", "message": {"id": "msg_1"}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Fix"}}),
        json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}),
    ]);
    Mock::given(method("POST")).and(path("/v1/messages")).respond_with(stream_response(body)).mount(&server).await;
    let error = stream_commit_message(&config(&server, "claude-opus-5-5"), &prompt(), |_| {}).await.unwrap_err();
    assert_eq!(error.code, AiErrorCode::Overloaded);

    // Seluruh rantai fallback menolak: teks parsial tidak dipakai.
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/v1/messages")).respond_with(stream_response(text_stream(&["Sebagian"], "refusal"))).mount(&server).await;
    let error = stream_commit_message(&config(&server, "claude-opus-5-5"), &prompt(), |_| {}).await.unwrap_err();
    assert_eq!(error.code, AiErrorCode::Refused);
}
