//! Uji klien OpenAI-compatible terhadap server tiruan (seperti OpenRouter, Groq, Ollama).

use oncode_ai::openai::{stream_commit_message, OpenAiConfig};
use oncode_ai::{AiErrorCode, CommitPrompt};
use serde_json::json;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn prompt() -> CommitPrompt {
    CommitPrompt { recent_subjects: vec![], stat: " a.txt | 1 +".into(), patch: "+baru\n".into(), truncated: false, draft: String::new() }
}

fn chunks(pieces: &[&str]) -> String {
    let mut body: String = pieces.iter().map(|piece| format!("data: {}\n\n", json!({"choices": [{"index": 0, "delta": {"content": piece}}]}))).collect();
    body.push_str(&format!("data: {}\n\n", json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]})));
    body.push_str("data: [DONE]\n\n");
    body
}

#[tokio::test]
async fn streams_from_an_openai_compatible_endpoint_with_a_bearer_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Bearer kunci-uji"))
        .and(body_partial_json(json!({"model": "gpt-uji", "stream": true})))
        .respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(chunks(&["feat: tambah ", "berkas baru"])))
        .expect(1)
        .mount(&server)
        .await;
    let config = OpenAiConfig { base_url: format!("{}/v1/", server.uri()), model: "gpt-uji".into(), api_key: Some("kunci-uji".into()) };
    let mut pieces = Vec::new();
    let message = stream_commit_message(&config, &prompt(), |piece| pieces.push(piece.to_string())).await.unwrap();
    assert_eq!(message, "feat: tambah berkas baru");
    assert_eq!(pieces, ["feat: tambah ", "berkas baru"]);
}

#[tokio::test]
async fn local_servers_need_no_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(chunks(&["<think>pikir</think>\nfix: a"])))
        .mount(&server)
        .await;
    let config = OpenAiConfig { base_url: format!("{}/v1", server.uri()), model: "qwen".into(), api_key: None };
    assert_eq!(stream_commit_message(&config, &prompt(), |_| {}).await.unwrap(), "fix: a", "blok <think> dibuang");
    let request = &server.received_requests().await.unwrap()[0];
    assert!(request.headers.get("authorization").is_none());
}

#[tokio::test]
async fn http_errors_are_reported_neutrally() {
    for (status, body, code, text) in [
        (401, json!({"error": {"message": "Incorrect API key provided"}}), AiErrorCode::Unauthorized, "Incorrect API key"),
        (404, json!({"error": {"message": "The model `gpt-x` does not exist"}}), AiErrorCode::InvalidRequest, "gpt-x"),
        (429, json!({"error": {"message": "slow down"}}), AiErrorCode::RateLimited, "rate limit"),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/v1/chat/completions")).respond_with(ResponseTemplate::new(status).set_body_json(body)).mount(&server).await;
        let config = OpenAiConfig { base_url: format!("{}/v1", server.uri()), model: "gpt-x".into(), api_key: Some("kunci-uji".into()) };
        let error = stream_commit_message(&config, &prompt(), |_| {}).await.unwrap_err();
        assert_eq!(error.code, code, "HTTP {status}");
        assert!(error.message.contains(text), "{}", error.message);
        assert!(!error.message.contains("kunci-uji"));
        assert!(!error.message.contains("Claude"), "pesan tidak menyebut Claude untuk endpoint lain");
    }
}
