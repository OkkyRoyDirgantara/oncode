//! Pembantu bersama untuk tes integrasi dengan server HTTP tiruan.
#![allow(dead_code)]

use oncode_forge::{ForgeError, ForgeErrorCode, RepoRef};
use wiremock::{MockServer, Request};

pub fn repo(host: &str, owner: &str, name: &str) -> RepoRef {
    RepoRef { host: host.to_string(), owner: owner.to_string(), name: name.to_string() }
}

/// Semua permintaan yang diterima server tiruan sejauh ini.
pub async fn received(server: &MockServer) -> Vec<Request> {
    server.received_requests().await.unwrap_or_default()
}

/// Nilai parameter query `name` pada permintaan, sudah di-decode.
pub fn query_value(request: &Request, name: &str) -> Option<String> {
    request.url.query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.into_owned())
}

/// Nilai field `name` pada badan form-urlencoded, sudah di-decode.
pub fn form_value(request: &Request, name: &str) -> Option<String> {
    let body = String::from_utf8_lossy(&request.body).into_owned();
    let url = wiremock::http::Url::parse(&format!("http://form.invalid/?{body}")).ok()?;
    url.query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.into_owned())
}

pub fn header<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    request.headers.get(name).and_then(|value| value.to_str().ok())
}

/// Memastikan galat berkode `code` dan pesannya tidak memuat `secret`.
pub fn assert_error(err: &ForgeError, code: ForgeErrorCode, secret: &str) {
    assert_eq!(err.code, code, "pesan: {}", err.message);
    assert!(!err.message.is_empty());
    assert!(!err.message.contains(secret), "rahasia bocor di pesan galat: {}", err.message);
    assert!(!format!("{err:?}").contains(secret), "rahasia bocor di Debug galat");
}

/// Alamat lokal yang pasti tertutup: port dipesan lalu langsung dilepas.
pub fn closed_port_uri() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind port sementara");
    let port = listener.local_addr().expect("alamat lokal").port();
    drop(listener);
    format!("http://127.0.0.1:{port}")
}
