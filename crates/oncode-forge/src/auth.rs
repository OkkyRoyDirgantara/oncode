//! Login OAuth 2.0 *device authorization grant* (RFC 8628) untuk GitHub dan GitLab.
//!
//! Alurnya: [`device_start`] meminta kode, UI menampilkan `user_code` dan membuka
//! `verification_uri`, lalu pemanggil memanggil [`device_poll`] tiap `interval` detik
//! sampai hasilnya bukan [`DevicePoll::Pending`] / [`DevicePoll::SlowDown`]. Tidak ada
//! client secret yang terlibat.

use std::fmt;

use reqwest::header::ACCEPT;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::{ForgeError, ForgeErrorCode, Result};
use crate::http::{self, non_empty, now_epoch, Reply};
use crate::model::{ForgeHost, ForgeKind, Timestamp};

const DEVICE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
const REDACTED: &str = "***";

/// Jawaban langkah pertama device flow.
///
/// `device_code` adalah rahasia sementara: siapa pun yang memegangnya bisa mengambil
/// token setelah pengguna menyetujui login. Simpan di sisi backend; UI hanya memerlukan
/// `user_code` dan `verification_uri`. `Debug` menyamarkannya.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCode {
    pub device_code: String,
    /// Kode pendek yang diketik pengguna di browser.
    pub user_code: String,
    /// Halaman tempat pengguna memasukkan kode.
    pub verification_uri: String,
    /// Tautan yang sudah memuat kodenya (GitLab); GitHub tidak mengirimnya.
    pub verification_uri_complete: Option<String>,
    /// Jeda minimal antar-poll, dalam detik.
    pub interval: u32,
    /// Masa berlaku kode, dalam detik.
    pub expires_in: u32,
}

impl fmt::Debug for DeviceCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceCode")
            .field("device_code", &REDACTED)
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("verification_uri_complete", &self.verification_uri_complete)
            .field("interval", &self.interval)
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

/// Token milik satu akun. Hanya untuk keychain dan memori proses: sengaja tidak
/// menurunkan `specta::Type` supaya tidak pernah dikirim ke UI, dan `Debug` menyamarkan
/// isinya.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenSet {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Kapan `access_token` kedaluwarsa (detik sejak epoch); `None` bila tidak kedaluwarsa.
    #[serde(default)]
    pub expires_at: Option<Timestamp>,
}

impl TokenSet {
    /// Token yang tidak kedaluwarsa dan tanpa refresh token, misalnya Personal Access Token.
    pub fn new(access_token: impl Into<String>) -> Self {
        Self { access_token: access_token.into(), refresh_token: None, expires_at: None }
    }

    /// `true` bila token sudah atau akan kedaluwarsa dalam `margin_seconds` detik.
    /// Token tanpa masa berlaku selalu `false`.
    pub fn expires_within(&self, margin_seconds: f64) -> bool {
        self.expires_at.is_some_and(|at| at - now_epoch() <= margin_seconds)
    }
}

impl fmt::Debug for TokenSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenSet")
            .field("access_token", &REDACTED)
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| REDACTED))
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Hasil satu kali poll ke endpoint token.
#[derive(Debug, Clone, PartialEq)]
pub enum DevicePoll {
    /// Pengguna belum menyelesaikan login; poll lagi setelah `interval` detik.
    Pending,
    /// Poll terlalu cepat; tambahkan 5 detik ke jeda lalu poll lagi (RFC 8628 §3.5).
    SlowDown,
    /// Login berhasil.
    Token(TokenSet),
    /// Pengguna menolak permintaan login.
    Denied,
    /// Kode sudah kedaluwarsa; mulai lagi dari [`device_start`].
    Expired,
}

/// Memulai device flow. GitHub: `POST {host}/login/device/code` dengan scope
/// `repo read:user`; GitLab: `POST {host}/oauth/authorize_device` dengan scope
/// `api read_user`. `web_base_override` mengganti origin web untuk tes.
///
/// Mengembalikan `Unsupported` bila instance tidak menyediakan device flow (GitLab
/// sebelum 17.2, atau fitur dimatikan); jalan lainnya adalah Personal Access Token.
pub async fn device_start(host: &ForgeHost, client_id: &str, web_base_override: Option<&str>) -> Result<DeviceCode> {
    let client_id = require_client_id(client_id)?;
    let server = host.hostname();
    let (path, scope) = match host.kind {
        ForgeKind::GitHub => ("/login/device/code", "repo read:user"),
        ForgeKind::GitLab => ("/oauth/authorize_device", "api read_user"),
    };
    let reply = post_form(host, web_base_override, path, &[("client_id", client_id), ("scope", scope)]).await?;

    match serde_json::from_slice::<DeviceCodeBody>(&reply.body) {
        Ok(body) if reply.status.is_success() => Ok(DeviceCode {
            device_code: body.device_code,
            user_code: body.user_code,
            verification_uri: body.verification_uri,
            verification_uri_complete: non_empty(body.verification_uri_complete),
            interval: body.interval.unwrap_or(5).max(1),
            expires_in: body.expires_in.unwrap_or(900),
        }),
        _ => {
            if let Some(error) = oauth_error(&reply) {
                return Err(error.into_forge_error(&server));
            }
            if reply.status == StatusCode::NOT_FOUND {
                return Err(ForgeError::unsupported(format!(
                    "Device code login is not available on {server} (or the client ID is not recognized). \
                     Use a personal access token instead."
                )));
            }
            if !reply.status.is_success() {
                return Err(http::status_error(&reply, &server));
            }
            Err(ForgeError::invalid_response(format!("The login response from {server} could not be understood")))
        }
    }
}

/// Satu kali poll ke endpoint token (GitHub: `{host}/login/oauth/access_token`, GitLab:
/// `{host}/oauth/token`) dengan grant type `urn:ietf:params:oauth:grant-type:device_code`.
///
/// Pemanggil yang mengatur jeda: tunggu `interval` detik antar-panggilan, dan tambah
/// 5 detik tiap menerima [`DevicePoll::SlowDown`]. Galat `Network` boleh dicoba lagi;
/// galat lain berarti alur harus diulang dari awal.
pub async fn device_poll(
    host: &ForgeHost,
    client_id: &str,
    device_code: &str,
    web_base_override: Option<&str>,
) -> Result<DevicePoll> {
    let client_id = require_client_id(client_id)?;
    if device_code.trim().is_empty() {
        return Err(ForgeError::invalid("Device code must not be empty"));
    }
    let server = host.hostname();
    let form = [("client_id", client_id), ("device_code", device_code), ("grant_type", DEVICE_GRANT_TYPE)];
    let reply = post_form(host, web_base_override, token_path(host.kind), &form).await?;

    if let Some(tokens) = token_set(&reply) {
        return Ok(DevicePoll::Token(tokens));
    }
    match oauth_error(&reply) {
        Some(error) => match error.code.as_str() {
            "authorization_pending" => Ok(DevicePoll::Pending),
            "slow_down" => Ok(DevicePoll::SlowDown),
            "access_denied" => Ok(DevicePoll::Denied),
            "expired_token" => Ok(DevicePoll::Expired),
            // Kode perangkat tidak dikenal lagi. GitLab menjawab begini setelah kode
            // dipakai atau dihapus, jadi penyebab pastinya tidak bisa dibedakan.
            "invalid_grant" | "incorrect_device_code" => Err(ForgeError::invalid(format!(
                "The login code for {server} is no longer valid (denied, already used, or expired). \
                 Start the login again."
            ))),
            _ => Err(error.into_forge_error(&server)),
        },
        None if !reply.status.is_success() => Err(http::status_error(&reply, &server)),
        None => Err(ForgeError::invalid_response(format!("The login response from {server} could not be understood"))),
    }
}

/// Menukar refresh token dengan token baru. Token akses GitLab punya masa berlaku;
/// token aplikasi OAuth GitHub tidak kedaluwarsa, sehingga untuk GitHub hasilnya
/// `Unsupported`.
///
/// Refresh token GitLab hanya bisa dipakai sekali: simpan [`TokenSet`] yang baru segera
/// setelah panggilan ini berhasil. `Unauthorized` berarti refresh token sudah tidak
/// berlaku dan pengguna harus login ulang.
pub async fn refresh_token(
    host: &ForgeHost,
    client_id: &str,
    refresh_token: &str,
    web_base_override: Option<&str>,
) -> Result<TokenSet> {
    if host.kind == ForgeKind::GitHub {
        return Err(ForgeError::unsupported("GitHub tokens do not expire, so there is nothing to refresh"));
    }
    let client_id = require_client_id(client_id)?;
    if refresh_token.trim().is_empty() {
        return Err(ForgeError::invalid("Refresh token must not be empty"));
    }
    let server = host.hostname();
    let form = [("client_id", client_id), ("refresh_token", refresh_token), ("grant_type", "refresh_token")];
    let reply = post_form(host, web_base_override, token_path(host.kind), &form).await?;

    if let Some(tokens) = token_set(&reply) {
        return Ok(tokens);
    }
    match oauth_error(&reply) {
        Some(error) if error.code == "invalid_grant" => Err(ForgeError::new(
            ForgeErrorCode::Unauthorized,
            format!("The login session for {server} could not be refreshed. Please log in again."),
        )),
        Some(error) => Err(error.into_forge_error(&server)),
        None if !reply.status.is_success() => Err(http::status_error(&reply, &server)),
        None => Err(ForgeError::invalid_response(format!("The login response from {server} could not be understood"))),
    }
}

fn token_path(kind: ForgeKind) -> &'static str {
    match kind {
        ForgeKind::GitHub => "/login/oauth/access_token",
        ForgeKind::GitLab => "/oauth/token",
    }
}

fn require_client_id(client_id: &str) -> Result<&str> {
    let client_id = client_id.trim();
    if client_id.is_empty() {
        return Err(ForgeError::invalid("OAuth client ID is not configured"));
    }
    Ok(client_id)
}

async fn post_form(
    host: &ForgeHost,
    web_base_override: Option<&str>,
    path: &str,
    form: &[(&str, &str)],
) -> Result<Reply> {
    let base = web_base_override.unwrap_or_else(|| host.web_base()).trim_end_matches('/');
    // Tanpa `Accept: application/json`, GitHub menjawab dalam bentuk form-urlencoded.
    let request = http::client()?.post(format!("{base}{path}")).header(ACCEPT, "application/json").form(form);
    http::send(request, &host.hostname()).await
}

#[derive(Deserialize)]
struct DeviceCodeBody {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    #[serde(default)]
    interval: Option<u32>,
    #[serde(default)]
    expires_in: Option<u32>,
}

#[derive(Deserialize)]
struct TokenBody {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    /// Sisa masa berlaku dalam detik.
    #[serde(default)]
    expires_in: Option<f64>,
}

/// Membaca token dari jawaban sukses. Rincian galat parse sengaja dibuang supaya isi
/// jawaban (yang bisa memuat token) tidak pernah masuk ke pesan galat.
fn token_set(reply: &Reply) -> Option<TokenSet> {
    if !reply.status.is_success() {
        return None;
    }
    let body: TokenBody = serde_json::from_slice(&reply.body).ok()?;
    if body.access_token.trim().is_empty() {
        return None;
    }
    Some(TokenSet {
        access_token: body.access_token,
        refresh_token: non_empty(body.refresh_token),
        // Dihitung dari jam lokal, karena nanti dibandingkan dengan jam lokal juga.
        expires_at: body.expires_in.filter(|seconds| *seconds > 0.0).map(|seconds| now_epoch() + seconds),
    })
}

/// Galat OAuth (`{"error": "...", "error_description": "..."}`). GitHub mengirimnya
/// dengan HTTP 200, GitLab dengan 400/401.
struct OAuthError {
    code: String,
    description: Option<String>,
}

#[derive(Deserialize)]
struct OAuthErrorBody {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

fn oauth_error(reply: &Reply) -> Option<OAuthError> {
    let body: OAuthErrorBody = serde_json::from_slice(&reply.body).ok()?;
    let code = body.error.trim().to_string();
    if code.is_empty() {
        return None;
    }
    Some(OAuthError { code, description: non_empty(body.error_description) })
}

impl OAuthError {
    fn into_forge_error(self, server: &str) -> ForgeError {
        let detail = match &self.description {
            Some(description) => format!("{}: {}", self.code, description.trim()),
            None => self.code.clone(),
        };
        match self.code.as_str() {
            "device_flow_disabled" | "unsupported_grant_type" | "unauthorized_client" => ForgeError::unsupported(format!(
                "Device code login is not enabled for this app on {server} ({detail})"
            )),
            "invalid_client" | "incorrect_client_credentials" => {
                ForgeError::invalid(format!("OAuth client ID is not recognized by {server} ({detail})"))
            }
            "invalid_scope" | "invalid_request" | "invalid_grant" => {
                ForgeError::invalid(format!("Login request rejected by {server} ({detail})"))
            }
            _ => ForgeError::invalid_response(format!("Login to {server} failed ({detail})")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_set_debug_is_redacted() {
        let tokens = TokenSet {
            access_token: "gho_akses_rahasia".into(),
            refresh_token: Some("refresh_rahasia".into()),
            expires_at: Some(1_700_000_000.0),
        };
        let debug = format!("{tokens:?}");
        assert!(!debug.contains("gho_akses_rahasia"), "{debug}");
        assert!(!debug.contains("refresh_rahasia"), "{debug}");
        assert!(debug.contains("***"));
        assert!(debug.contains("1700000000"));

        let pretty = format!("{:#?}", DevicePoll::Token(tokens));
        assert!(!pretty.contains("rahasia"), "{pretty}");
    }

    #[test]
    fn device_code_debug_is_redacted() {
        let code = DeviceCode {
            device_code: "kode_perangkat_rahasia".into(),
            user_code: "WDJB-MJHT".into(),
            verification_uri: "https://github.com/login/device".into(),
            verification_uri_complete: None,
            interval: 5,
            expires_in: 900,
        };
        let debug = format!("{code:?}");
        assert!(!debug.contains("kode_perangkat_rahasia"), "{debug}");
        assert!(debug.contains("WDJB-MJHT"));
    }

    #[test]
    fn token_set_json_round_trip() {
        let tokens = TokenSet { access_token: "a".into(), refresh_token: Some("r".into()), expires_at: Some(12.5) };
        let json = serde_json::to_string(&tokens).expect("json");
        assert_eq!(json, r#"{"accessToken":"a","refreshToken":"r","expiresAt":12.5}"#);
        assert_eq!(serde_json::from_str::<TokenSet>(&json).expect("json"), tokens);
        // Field opsional boleh tidak ada.
        let minimal: TokenSet = serde_json::from_str(r#"{"accessToken":"a"}"#).expect("json");
        assert_eq!(minimal, TokenSet::new("a"));
    }

    #[test]
    fn expiry_margin() {
        assert!(!TokenSet::new("pat").expires_within(3600.0));
        let soon = TokenSet { access_token: "a".into(), refresh_token: None, expires_at: Some(now_epoch() + 30.0) };
        assert!(soon.expires_within(60.0));
        assert!(!soon.expires_within(5.0));
        let past = TokenSet { access_token: "a".into(), refresh_token: None, expires_at: Some(now_epoch() - 1.0) };
        assert!(past.expires_within(0.0));
    }

    #[tokio::test]
    async fn github_refresh_is_unsupported() {
        let err = refresh_token(&ForgeHost::github(), "client", "refresh", None).await.expect_err("harus gagal");
        assert_eq!(err.code, ForgeErrorCode::Unsupported);
    }
}
