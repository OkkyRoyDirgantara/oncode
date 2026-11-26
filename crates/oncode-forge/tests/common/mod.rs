//! Shared helpers for integration tests with a mock HTTP server.
#![allow(dead_code)]

use oncode_forge::{ForgeError, ForgeErrorCode, RepoRef};
use wiremock::{MockServer, Request};

pub fn repo(host: &str, owner: &str, name: &str) -> RepoRef {
    RepoRef { host: host.to_string(), owner: owner.to_string(), name: name.to_string() }
}

/// All requests the mock server has received so far.
pub async fn received(server: &MockServer) -> Vec<Request> {
    server.received_requests().await.unwrap_or_default()
}

/// The value of query parameter `name` on the request, already decoded.
pub fn query_value(request: &Request, name: &str) -> Option<String> {
    request.url.query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.into_owned())
}

/// The value of field `name` in a form-urlencoded body, already decoded.
pub fn form_value(request: &Request, name: &str) -> Option<String> {
    let body = String::from_utf8_lossy(&request.body).into_owned();
    let url = wiremock::http::Url::parse(&format!("http://form.invalid/?{body}")).ok()?;
    url.query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.into_owned())
}

pub fn header<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    request.headers.get(name).and_then(|value| value.to_str().ok())
}

/// Ensures the error has code `code` and its message does not contain `secret`.
pub fn assert_error(err: &ForgeError, code: ForgeErrorCode, secret: &str) {
    assert_eq!(err.code, code, "message: {}", err.message);
    assert!(!err.message.is_empty());
    assert!(!err.message.contains(secret), "secret leaked in error message: {}", err.message);
    assert!(!format!("{err:?}").contains(secret), "secret leaked in error Debug");
}

/// A local address that is guaranteed to be closed: a port is reserved and then released right away.
pub fn closed_port_uri() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind temporary port");
    let port = listener.local_addr().expect("local address").port();
    drop(listener);
    format!("http://127.0.0.1:{port}")
}
