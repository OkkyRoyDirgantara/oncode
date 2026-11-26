//! OAuth 2.0 *device authorization grant* login (RFC 8628) for GitHub and GitLab.
//!
//! The flow: [`device_start`] requests a code, the UI shows the `user_code` and opens
//! the `verification_uri`, then the caller calls [`device_poll`] every `interval` seconds
//! until the result is no longer [`DevicePoll::Pending`] / [`DevicePoll::SlowDown`]. No
//! client secret is involved.

use std::fmt;

use reqwest::header::ACCEPT;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::error::{ForgeError, ForgeErrorCode, Result};
use crate::http::{self, non_empty, now_epoch, Reply};
use crate::model::{ForgeHost, ForgeKind, Timestamp};

const DEVICE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
const REDACTED: &str = "***";

/// Response of the first step of the device flow.
///
/// `device_code` is a temporary secret: whoever holds it can fetch the token once the
/// user approves the login. Keep it on the backend side; the UI only needs
/// `user_code` and `verification_uri`. `Debug` redacts it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCode {
    pub device_code: String,
    /// Short code the user types in the browser.
    pub user_code: String,
    /// Page where the user enters the code.
    pub verification_uri: String,
    /// Link that already contains the code (GitLab); GitHub does not send it.
    pub verification_uri_complete: Option<String>,
    /// Minimum delay between polls, in seconds.
    pub interval: u32,
    /// Lifetime of the code, in seconds.
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

/// Token belonging to one account. Only for the keychain and process memory, never sent
/// to the UI; `Debug` redacts its contents.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenSet {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// When `access_token` expires (seconds since epoch); `None` if it does not expire.
    #[serde(default)]
    pub expires_at: Option<Timestamp>,
}

impl TokenSet {
    /// A token that does not expire and has no refresh token, such as a Personal Access Token.
    pub fn new(access_token: impl Into<String>) -> Self {
        Self { access_token: access_token.into(), refresh_token: None, expires_at: None }
    }

    /// `true` if the token has expired or will expire within `margin_seconds` seconds.
    /// A token without an expiry is always `false`.
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

/// Result of a single poll of the token endpoint.
#[derive(Debug, Clone, PartialEq)]
pub enum DevicePoll {
    /// The user has not finished logging in; poll again after `interval` seconds.
    Pending,
    /// Polled too fast; add 5 seconds to the delay and poll again (RFC 8628 §3.5).
    SlowDown,
    /// Login succeeded.
    Token(TokenSet),
    /// The user denied the login request.
    Denied,
    /// The code has expired; start over from [`device_start`].
    Expired,
}

/// Starts the device flow. GitHub: `POST {host}/login/device/code` with scope
/// `repo read:user`; GitLab: `POST {host}/oauth/authorize_device` with scope
/// `api read_user`. `web_base_override` replaces the web origin for tests.
///
/// Returns `Unsupported` if the instance does not offer the device flow (GitLab
/// before 17.2, or the feature is disabled); the alternative is a Personal Access Token.
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

/// A single poll of the token endpoint (GitHub: `{host}/login/oauth/access_token`, GitLab:
/// `{host}/oauth/token`) with grant type `urn:ietf:params:oauth:grant-type:device_code`.
///
/// The caller controls the delay: wait `interval` seconds between calls, and add
/// 5 seconds each time [`DevicePoll::SlowDown`] is received. A `Network` error may be
/// retried; any other error means the flow must be restarted from the beginning.
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
            // The device code is no longer recognized. GitLab answers this way after the code
            // is used or deleted, so the exact cause cannot be told apart.
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

/// Exchanges a refresh token for a new token. GitLab access tokens have an expiry;
/// GitHub OAuth app tokens do not expire, so for GitHub the result is
/// `Unsupported`.
///
/// A GitLab refresh token can only be used once: store the new [`TokenSet`] right
/// after this call succeeds. `Unauthorized` means the refresh token is no longer
/// valid and the user must log in again.
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
    // Without `Accept: application/json`, GitHub answers in form-urlencoded format.
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
    /// Remaining lifetime in seconds.
    #[serde(default)]
    expires_in: Option<f64>,
}

/// Reads the token from a successful response. Parse error details are deliberately dropped so that the
/// response body (which may contain a token) never ends up in an error message.
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
        // Computed from the local clock, because it is later compared against the local clock too.
        expires_at: body.expires_in.filter(|seconds| *seconds > 0.0).map(|seconds| now_epoch() + seconds),
    })
}

/// OAuth error (`{"error": "...", "error_description": "..."}`). GitHub sends it
/// with HTTP 200, GitLab with 400/401.
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
            access_token: "gho_access_secret".into(),
            refresh_token: Some("refresh_secret".into()),
            expires_at: Some(1_700_000_000.0),
        };
        let debug = format!("{tokens:?}");
        assert!(!debug.contains("gho_access_secret"), "{debug}");
        assert!(!debug.contains("refresh_secret"), "{debug}");
        assert!(debug.contains("***"));
        assert!(debug.contains("1700000000"));

        let pretty = format!("{:#?}", DevicePoll::Token(tokens));
        assert!(!pretty.contains("secret"), "{pretty}");
    }

    #[test]
    fn device_code_debug_is_redacted() {
        let code = DeviceCode {
            device_code: "device_code_secret".into(),
            user_code: "WDJB-MJHT".into(),
            verification_uri: "https://github.com/login/device".into(),
            verification_uri_complete: None,
            interval: 5,
            expires_in: 900,
        };
        let debug = format!("{code:?}");
        assert!(!debug.contains("device_code_secret"), "{debug}");
        assert!(debug.contains("WDJB-MJHT"));
    }

    #[test]
    fn token_set_json_round_trip() {
        let tokens = TokenSet { access_token: "a".into(), refresh_token: Some("r".into()), expires_at: Some(12.5) };
        let json = serde_json::to_string(&tokens).expect("json");
        assert_eq!(json, r#"{"accessToken":"a","refreshToken":"r","expiresAt":12.5}"#);
        assert_eq!(serde_json::from_str::<TokenSet>(&json).expect("json"), tokens);
        // Optional fields may be absent.
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
        let err = refresh_token(&ForgeHost::github(), "client", "refresh", None).await.expect_err("must fail");
        assert_eq!(err.code, ForgeErrorCode::Unsupported);
    }
}
