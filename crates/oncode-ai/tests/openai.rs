//! Tests for the OpenAI-compatible client against a mock server (such as OpenRouter, Groq, Ollama).

use oncode_ai::openai::{stream_commit_message, OpenAiConfig};
use oncode_ai::{AiErrorCode, CommitPrompt};
use serde_json::json;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn prompt() -> CommitPrompt {
    CommitPrompt { recent_subjects: vec![], stat: " a.txt | 1 +".into(), patch: "+new\n".into(), truncated: false, draft: String::new() }
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
        .and(header("authorization", "Bearer test-key"))
        .and(body_partial_json(json!({"model": "gpt-test", "stream": true})))
        .respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(chunks(&["feat: add ", "new file"])))
        .expect(1)
        .mount(&server)
        .await;
    let config = OpenAiConfig { base_url: format!("{}/v1/", server.uri()), model: "gpt-test".into(), api_key: Some("test-key".into()) };
    let mut pieces = Vec::new();
    let message = stream_commit_message(&config, &prompt(), |piece| pieces.push(piece.to_string())).await.unwrap();
    assert_eq!(message, "feat: add new file");
    assert_eq!(pieces, ["feat: add ", "new file"]);
}

#[tokio::test]
async fn local_servers_need_no_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(chunks(&["<think>thought</think>\nfix: a"])))
        .mount(&server)
        .await;
    let config = OpenAiConfig { base_url: format!("{}/v1", server.uri()), model: "qwen".into(), api_key: None };
    assert_eq!(stream_commit_message(&config, &prompt(), |_| {}).await.unwrap(), "fix: a", "the <think> block is removed");
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
        let config = OpenAiConfig { base_url: format!("{}/v1", server.uri()), model: "gpt-x".into(), api_key: Some("test-key".into()) };
        let error = stream_commit_message(&config, &prompt(), |_| {}).await.unwrap_err();
        assert_eq!(error.code, code, "HTTP {status}");
        assert!(error.message.contains(text), "{}", error.message);
        assert!(!error.message.contains("test-key"));
        assert!(!error.message.contains("Claude"), "the message must not mention Claude for other endpoints");
    }
}
