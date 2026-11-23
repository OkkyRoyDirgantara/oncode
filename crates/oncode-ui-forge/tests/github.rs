//! Uji tampilan login GitHub dengan device flow (server GitHub Enterprise tiruan di mesin lokal),
//! lalu push ke remote HTTP memakai token akun itu tanpa dialog kredensial.

#[path = "../../oncode-ui-scm/tests/common/mod.rs"]
mod common;

use common::GitHttpServer;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use oncode_core::api::forge as forge_api;
use oncode_core::Settings;
use oncode_ui::repos;
use oncode_ui::testing::{git, TestApp, TestWindow};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "gho_tokenuji123";

async fn github() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/login/device/code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "device_code": "dev-1", "user_code": "ABCD-1234", "verification_uri": "http://127.0.0.1/login/device", "expires_in": 900, "interval": 1 })))
        .mount(&server)
        .await;
    // Jawaban pertama "belum disetujui", lalu token.
    Mock::given(method("POST")).and(path("/login/oauth/access_token")).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "error": "authorization_pending" }))).up_to_n_times(1).mount(&server).await;
    Mock::given(method("POST")).and(path("/login/oauth/access_token")).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "access_token": TOKEN, "token_type": "bearer", "scope": "repo,read:user" }))).mount(&server).await;
    Mock::given(method("GET")).and(path("/api/v3/user")).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "login": "ana", "name": "Ana", "avatar_url": "" }))).mount(&server).await;
    server
}

/// Memilih butir quick pick dengan mengetik lalu Enter.
async fn pick(window: &TestWindow, cx: &mut TestAppContext, text: &str) {
    for _ in 0..300 {
        let ready = window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.try_find("quick-pick-input").is_some()
        });
        if ready {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.run_until_parked();
    }
    window.update(cx, |window, cx| window.input(text, cx));
    cx.run_until_parked();
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
async fn device_flow_login_then_push_with_the_account_token(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_forge::init);
    let server = app.runtime.block_on(github());
    let mut settings = Settings::default();
    settings.forge.github_client_id = Some("klien-uji".into());
    app.core.set_user_settings(settings).unwrap();
    let window = app.open(None, cx).await;

    // Sign In → GitHub Enterprise → alamat server → lewat browser.
    assert!(window.run_command(cx, "forge.signIn"));
    pick(&window, cx, "Enterprise").await;
    for _ in 0..100 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.run_until_parked();
    }
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input(&server.uri(), cx);
        window.press("enter", cx);
    });
    cx.run_until_parked();
    pick(&window, cx, "browser").await;

    // Kode perangkat tampil; polling berjalan sampai server memberi token.
    let core = app.core.clone();
    window
        .wait_until(cx, "akun tersimpan", |_, _| {
            let core = core.clone();
            app.runtime.block_on(async move { forge_api::forge_accounts(&core).await }).is_ok_and(|accounts| accounts.iter().any(|a| a.username == "ana"))
        })
        .await;
    assert_eq!(cx.opened_url().as_deref(), Some("http://127.0.0.1/login/device"), "halaman verifikasi dibuka di browser");

    // Push ke remote di host yang sama memakai token akun: tidak ada dialog kredensial.
    let git_server = GitHttpServer::start("x-access-token", TOKEN);
    let project = app.git_project(&[("a.txt", "a\n")]);
    git(&project, &["remote", "add", "origin", &git_server.url()]);
    let window = app.open(Some(&project), cx).await;
    cx.update(oncode_ui_scm::init);
    let root = project.to_string_lossy().into_owned();
    window.wait_until(cx, "snapshot", move |_, cx| repos(cx).read(cx).snapshot(&root).is_some()).await;
    let _ = window;
    let push_core = app.core.clone();
    let push_root = project.to_string_lossy().into_owned();
    let result = app.runtime.block_on(async move {
        oncode_core::api::git::git_push(&push_core, push_root, oncode_git::PushOptions { set_upstream: true, ..Default::default() }, "uji".into(), std::sync::Arc::new(|_| {})).await
    });
    assert!(result.is_ok(), "push memakai token akun: {result:?}");
    let pushed = std::process::Command::new("git").current_dir(git_server.bare()).args(["rev-parse", "main"]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&pushed.stdout).trim(), git(&project, &["rev-parse", "HEAD"]).trim());
}
