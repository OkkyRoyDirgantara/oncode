//! Device flow dan pembaruan token terhadap server HTTP tiruan.

mod common;

use std::time::{SystemTime, UNIX_EPOCH};

use common::{assert_error, form_value, header, received};
use oncode_forge::{device_poll, device_start, refresh_token, DevicePoll, ForgeErrorCode, ForgeHost, ForgeKind};
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const GITHUB_CLIENT: &str = "Iv1.klien_github";
const GITLAB_CLIENT: &str = "klien_gitlab_0123";
const DEVICE_CODE: &str = "kode_perangkat_rahasia_3584d83530557fdd";
const ACCESS_TOKEN: &str = "token_akses_rahasia_e72e16c7";
const REFRESH_TOKEN: &str = "token_refresh_rahasia_803c1fd4";

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).expect("jam sistem").as_secs_f64()
}

fn token_path(kind: ForgeKind) -> &'static str {
    match kind {
        ForgeKind::GitHub => "/login/oauth/access_token",
        ForgeKind::GitLab => "/oauth/token",
    }
}

fn host(kind: ForgeKind) -> ForgeHost {
    match kind {
        ForgeKind::GitHub => ForgeHost::github(),
        ForgeKind::GitLab => ForgeHost::gitlab(),
    }
}

fn client_id(kind: ForgeKind) -> &'static str {
    match kind {
        ForgeKind::GitHub => GITHUB_CLIENT,
        ForgeKind::GitLab => GITLAB_CLIENT,
    }
}

/// GitHub mengirim galat OAuth dengan HTTP 200, GitLab dengan HTTP 400.
fn error_status(kind: ForgeKind) -> u16 {
    match kind {
        ForgeKind::GitHub => 200,
        ForgeKind::GitLab => 400,
    }
}

/// Menjalankan satu poll terhadap server yang menjawab `status` + `body`.
async fn poll_with(kind: ForgeKind, status: u16, body: Value) -> (oncode_forge::Result<DevicePoll>, MockServer) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(token_path(kind)))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .expect(1)
        .mount(&server)
        .await;
    let result = device_poll(&host(kind), client_id(kind), DEVICE_CODE, Some(&server.uri())).await;
    (result, server)
}

#[tokio::test]
async fn github_device_start() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/login/device/code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "device_code": DEVICE_CODE,
            "user_code": "WDJB-MJHT",
            "verification_uri": "https://github.com/login/device",
            "expires_in": 900,
            "interval": 5,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let code = device_start(&ForgeHost::github(), GITHUB_CLIENT, Some(&server.uri())).await.expect("device_start");
    assert_eq!(code.device_code, DEVICE_CODE);
    assert_eq!(code.user_code, "WDJB-MJHT");
    assert_eq!(code.verification_uri, "https://github.com/login/device");
    assert_eq!(code.verification_uri_complete, None);
    assert_eq!(code.interval, 5);
    assert_eq!(code.expires_in, 900);
    assert!(!format!("{code:?}").contains(DEVICE_CODE));

    let requests = received(&server).await;
    let request = &requests[0];
    assert_eq!(header(request, "accept"), Some("application/json"));
    assert_eq!(header(request, "content-type"), Some("application/x-www-form-urlencoded"));
    assert_eq!(header(request, "user-agent"), Some("OnCode"));
    assert_eq!(header(request, "authorization"), None);
    assert_eq!(form_value(request, "client_id").as_deref(), Some(GITHUB_CLIENT));
    assert_eq!(form_value(request, "scope").as_deref(), Some("repo read:user"));
}

#[tokio::test]
async fn gitlab_device_start() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/authorize_device"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "device_code": DEVICE_CODE,
            "user_code": "0A44L90H",
            "verification_uri": "https://gitlab.com/oauth/device",
            "verification_uri_complete": "https://gitlab.com/oauth/device?user_code=0A44L90H",
            "expires_in": 300,
            "interval": 5,
        })))
        .expect(1)
        .mount(&server)
        .await;

    // Garis miring di akhir override tidak menggandakan pemisah path.
    let base = format!("{}/", server.uri());
    let code = device_start(&ForgeHost::gitlab(), GITLAB_CLIENT, Some(&base)).await.expect("device_start");
    assert_eq!(code.device_code, DEVICE_CODE);
    assert_eq!(code.user_code, "0A44L90H");
    assert_eq!(code.verification_uri, "https://gitlab.com/oauth/device");
    assert_eq!(code.verification_uri_complete.as_deref(), Some("https://gitlab.com/oauth/device?user_code=0A44L90H"));
    assert_eq!(code.interval, 5);
    assert_eq!(code.expires_in, 300);

    let requests = received(&server).await;
    assert_eq!(form_value(&requests[0], "client_id").as_deref(), Some(GITLAB_CLIENT));
    assert_eq!(form_value(&requests[0], "scope").as_deref(), Some("api read_user"));
}

#[tokio::test]
async fn device_start_defaults_missing_interval() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/login/device/code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "device_code": DEVICE_CODE,
            "user_code": "WDJB-MJHT",
            "verification_uri": "https://github.com/login/device",
        })))
        .mount(&server)
        .await;

    let code = device_start(&ForgeHost::github(), GITHUB_CLIENT, Some(&server.uri())).await.expect("device_start");
    assert_eq!(code.interval, 5);
    assert_eq!(code.expires_in, 900);
}

#[tokio::test]
async fn device_start_failures() {
    // Instance yang belum punya endpoint device flow.
    let server = MockServer::start().await;
    let err = device_start(&ForgeHost::gitlab(), GITLAB_CLIENT, Some(&server.uri())).await.expect_err("404");
    assert_error(&err, ForgeErrorCode::Unsupported, GITLAB_CLIENT);

    // GitHub: device flow dimatikan untuk aplikasi ini (dijawab dengan HTTP 200).
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/login/device/code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": "device_flow_disabled",
            "error_description": "Device Flow must be explicitly enabled for this App",
        })))
        .mount(&server)
        .await;
    let err = device_start(&ForgeHost::github(), GITHUB_CLIENT, Some(&server.uri())).await.expect_err("dimatikan");
    assert_eq!(err.code, ForgeErrorCode::Unsupported, "{}", err.message);

    // GitLab: client ID tidak dikenal.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/authorize_device"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": "invalid_client",
            "error_description": "Client authentication failed due to unknown client.",
        })))
        .mount(&server)
        .await;
    let err = device_start(&ForgeHost::gitlab(), GITLAB_CLIENT, Some(&server.uri())).await.expect_err("klien salah");
    assert_eq!(err.code, ForgeErrorCode::InvalidInput, "{}", err.message);

    // Jawaban sukses yang bentuknya salah.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/login/device/code"))
        .respond_with(ResponseTemplate::new(200).set_body_string("device_code=abc&user_code=WDJB"))
        .mount(&server)
        .await;
    let err = device_start(&ForgeHost::github(), GITHUB_CLIENT, Some(&server.uri())).await.expect_err("bukan json");
    assert_eq!(err.code, ForgeErrorCode::InvalidResponse, "{}", err.message);

    // Client ID kosong ditolak tanpa permintaan apa pun; server mati adalah galat jaringan.
    let err = device_start(&ForgeHost::github(), "  ", None).await.expect_err("client id kosong");
    assert_eq!(err.code, ForgeErrorCode::InvalidInput);
    let uri = common::closed_port_uri();
    let err = device_start(&ForgeHost::github(), GITHUB_CLIENT, Some(&uri)).await.expect_err("tidak terhubung");
    assert_eq!(err.code, ForgeErrorCode::Network, "{}", err.message);
}

#[tokio::test]
async fn device_poll_sends_device_code_grant() {
    for kind in [ForgeKind::GitHub, ForgeKind::GitLab] {
        let body = json!({ "error": "authorization_pending", "error_description": "The authorization request is still pending." });
        let (result, server) = poll_with(kind, error_status(kind), body).await;
        assert_eq!(result.expect("poll"), DevicePoll::Pending, "{kind:?}");

        let requests = received(&server).await;
        let request = &requests[0];
        assert_eq!(header(request, "accept"), Some("application/json"));
        assert_eq!(header(request, "content-type"), Some("application/x-www-form-urlencoded"));
        assert_eq!(form_value(request, "client_id").as_deref(), Some(client_id(kind)));
        assert_eq!(form_value(request, "device_code").as_deref(), Some(DEVICE_CODE));
        assert_eq!(
            form_value(request, "grant_type").as_deref(),
            Some("urn:ietf:params:oauth:grant-type:device_code")
        );
        // Kode perangkat hanya ada di badan permintaan, tidak di URL.
        assert!(!request.url.as_str().contains(DEVICE_CODE));
    }
}

#[tokio::test]
async fn device_poll_pending_states() {
    for kind in [ForgeKind::GitHub, ForgeKind::GitLab] {
        let (result, _server) = poll_with(kind, error_status(kind), json!({ "error": "authorization_pending" })).await;
        assert_eq!(result.expect("poll"), DevicePoll::Pending, "{kind:?}");

        let slow_down = json!({ "error": "slow_down", "error_description": "Too many requests.", "interval": 10 });
        let (result, _server) = poll_with(kind, error_status(kind), slow_down).await;
        assert_eq!(result.expect("poll"), DevicePoll::SlowDown, "{kind:?}");
    }
}

#[tokio::test]
async fn device_poll_terminal_states() {
    for kind in [ForgeKind::GitHub, ForgeKind::GitLab] {
        let denied = json!({ "error": "access_denied", "error_description": "The user has denied your application access." });
        let (result, _server) = poll_with(kind, error_status(kind), denied).await;
        assert_eq!(result.expect("poll"), DevicePoll::Denied, "{kind:?}");

        let expired = json!({ "error": "expired_token", "error_description": "The device code has expired." });
        let (result, _server) = poll_with(kind, error_status(kind), expired).await;
        assert_eq!(result.expect("poll"), DevicePoll::Expired, "{kind:?}");
    }
}

#[tokio::test]
async fn github_device_poll_success() {
    let body = json!({ "access_token": ACCESS_TOKEN, "token_type": "bearer", "scope": "repo,read:user" });
    let (result, _server) = poll_with(ForgeKind::GitHub, 200, body).await;
    let poll = result.expect("poll");
    assert!(!format!("{poll:?}").contains(ACCESS_TOKEN), "token bocor di Debug");
    let DevicePoll::Token(tokens) = poll else { panic!("seharusnya Token") };
    assert_eq!(tokens.access_token, ACCESS_TOKEN);
    // Token aplikasi OAuth GitHub tidak kedaluwarsa dan tidak punya refresh token.
    assert_eq!(tokens.refresh_token, None);
    assert_eq!(tokens.expires_at, None);
    assert!(!tokens.expires_within(3600.0));
}

#[tokio::test]
async fn gitlab_device_poll_success() {
    let before = now();
    let body = json!({
        "access_token": ACCESS_TOKEN,
        "token_type": "Bearer",
        "expires_in": 7200,
        "refresh_token": REFRESH_TOKEN,
        "scope": "api read_user",
        "created_at": 1593096829,
    });
    let (result, _server) = poll_with(ForgeKind::GitLab, 200, body).await;
    let after = now();
    let DevicePoll::Token(tokens) = result.expect("poll") else { panic!("seharusnya Token") };
    assert_eq!(tokens.access_token, ACCESS_TOKEN);
    assert_eq!(tokens.refresh_token.as_deref(), Some(REFRESH_TOKEN));
    // Masa berlaku dihitung dari jam lokal, bukan dari `created_at` server.
    let expires_at = tokens.expires_at.expect("expires_at");
    assert!(expires_at >= before + 7200.0 && expires_at <= after + 7200.0, "{expires_at}");
    assert!(tokens.expires_within(7300.0));
    assert!(!tokens.expires_within(60.0));
    let debug = format!("{tokens:?}");
    assert!(!debug.contains(ACCESS_TOKEN) && !debug.contains(REFRESH_TOKEN), "{debug}");
}

#[tokio::test]
async fn device_poll_failures() {
    // GitLab menjawab `invalid_grant` untuk kode yang sudah dihapus atau dipakai.
    let body = json!({ "error": "invalid_grant", "error_description": "The provided authorization grant is invalid." });
    let (result, _server) = poll_with(ForgeKind::GitLab, 400, body).await;
    assert_error(&result.expect_err("invalid_grant"), ForgeErrorCode::InvalidInput, DEVICE_CODE);

    let (result, _server) = poll_with(ForgeKind::GitHub, 200, json!({ "error": "incorrect_device_code" })).await;
    assert_error(&result.expect_err("incorrect_device_code"), ForgeErrorCode::InvalidInput, DEVICE_CODE);

    let (result, _server) = poll_with(ForgeKind::GitHub, 200, json!({ "error": "device_flow_disabled" })).await;
    assert_error(&result.expect_err("device_flow_disabled"), ForgeErrorCode::Unsupported, DEVICE_CODE);

    let (result, _server) = poll_with(ForgeKind::GitHub, 200, json!({ "error": "incorrect_client_credentials" })).await;
    assert_error(&result.expect_err("incorrect_client_credentials"), ForgeErrorCode::InvalidInput, DEVICE_CODE);

    let (result, _server) = poll_with(ForgeKind::GitLab, 503, json!({ "message": "Service Unavailable" })).await;
    assert_error(&result.expect_err("503"), ForgeErrorCode::Network, DEVICE_CODE);

    let (result, _server) = poll_with(ForgeKind::GitLab, 429, json!({ "message": "Retry later" })).await;
    assert_error(&result.expect_err("429"), ForgeErrorCode::RateLimited, DEVICE_CODE);

    // Jawaban sukses yang tidak terbaca: isi jawabannya tidak boleh ikut ke pesan galat.
    let odd = json!({ "access_token": ACCESS_TOKEN, "expires_in": "dua jam" });
    let (result, _server) = poll_with(ForgeKind::GitLab, 200, odd).await;
    let err = result.expect_err("bentuk salah");
    assert_error(&err, ForgeErrorCode::InvalidResponse, ACCESS_TOKEN);

    let err = device_poll(&ForgeHost::github(), GITHUB_CLIENT, " ", None).await.expect_err("kode kosong");
    assert_eq!(err.code, ForgeErrorCode::InvalidInput);
    let uri = common::closed_port_uri();
    let err = device_poll(&ForgeHost::gitlab(), GITLAB_CLIENT, DEVICE_CODE, Some(&uri)).await.expect_err("mati");
    assert_error(&err, ForgeErrorCode::Network, DEVICE_CODE);
}

#[tokio::test]
async fn gitlab_refresh_token_exchanges_for_new_tokens() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "token_akses_baru_c97d1fe5",
            "token_type": "bearer",
            "expires_in": 7200,
            "refresh_token": "token_refresh_baru_9f3a",
            "created_at": 1628711391,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let before = now();
    let tokens = refresh_token(&ForgeHost::gitlab(), GITLAB_CLIENT, REFRESH_TOKEN, Some(&server.uri()))
        .await
        .expect("refresh_token");
    assert_eq!(tokens.access_token, "token_akses_baru_c97d1fe5");
    assert_eq!(tokens.refresh_token.as_deref(), Some("token_refresh_baru_9f3a"));
    let expires_at = tokens.expires_at.expect("expires_at");
    assert!(expires_at >= before + 7200.0 && expires_at <= now() + 7200.0, "{expires_at}");

    let requests = received(&server).await;
    let request = &requests[0];
    assert_eq!(header(request, "accept"), Some("application/json"));
    assert_eq!(form_value(request, "grant_type").as_deref(), Some("refresh_token"));
    assert_eq!(form_value(request, "refresh_token").as_deref(), Some(REFRESH_TOKEN));
    assert_eq!(form_value(request, "client_id").as_deref(), Some(GITLAB_CLIENT));
    assert!(!request.url.as_str().contains(REFRESH_TOKEN));
}

#[tokio::test]
async fn gitlab_refresh_token_failures() {
    // Refresh token sudah dicabut atau sudah pernah dipakai: pengguna harus login ulang.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": "invalid_grant",
            "error_description": "The provided authorization grant is invalid, expired, revoked, does not match the redirection URI used in the authorization request, or was issued to another client.",
        })))
        .mount(&server)
        .await;
    let err = refresh_token(&ForgeHost::gitlab(), GITLAB_CLIENT, REFRESH_TOKEN, Some(&server.uri()))
        .await
        .expect_err("invalid_grant");
    assert_error(&err, ForgeErrorCode::Unauthorized, REFRESH_TOKEN);

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
        .mount(&server)
        .await;
    let err = refresh_token(&ForgeHost::gitlab(), GITLAB_CLIENT, REFRESH_TOKEN, Some(&server.uri()))
        .await
        .expect_err("500");
    assert_error(&err, ForgeErrorCode::Network, REFRESH_TOKEN);

    let err = refresh_token(&ForgeHost::gitlab(), GITLAB_CLIENT, "", None).await.expect_err("kosong");
    assert_eq!(err.code, ForgeErrorCode::InvalidInput);
}

#[tokio::test]
async fn github_refresh_token_is_unsupported() {
    // Tidak ada permintaan yang dikirim: server tiruan tanpa mock akan menjawab 404.
    let server = MockServer::start().await;
    let err = refresh_token(&ForgeHost::github(), GITHUB_CLIENT, REFRESH_TOKEN, Some(&server.uri()))
        .await
        .expect_err("github");
    assert_error(&err, ForgeErrorCode::Unsupported, REFRESH_TOKEN);
    assert!(received(&server).await.is_empty());
}
