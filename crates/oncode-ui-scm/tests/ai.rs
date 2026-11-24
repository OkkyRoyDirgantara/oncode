//! Uji "Generate Commit Message": Claude API tiruan di mesin lokal, kotak commit, dan prompt API key.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext};
use oncode_core::api::ai as ai_api;
use oncode_ui::repos;
use oncode_ui::testing::{git, TestApp, TestWindow};
use oncode_ui_scm::store::ScmStore;
use oncode_ui_scm::view::ScmPanel;
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SUBJECT: &str = "fix: ubah isi a.txt";

/// Claude API tiruan yang mengalirkan `SUBJECT` dalam dua potongan untuk key `sk-ant-uji`.
async fn claude() -> MockServer {
    let server = MockServer::start().await;
    let events = [
        json!({"type": "message_start", "message": {"id": "msg_1", "model": "claude-opus-5-5"}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "fix: ubah "}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "isi a.txt"}}),
        json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}}),
        json!({"type": "message_stop"}),
    ];
    let body: String = events.iter().map(|e| format!("event: {}\ndata: {}\n\n", e["type"].as_str().unwrap(), e)).collect();
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "sk-ant-uji"))
        .respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(body))
        .mount(&server)
        .await;
    server
}

/// Proyek dengan satu perubahan yang sudah di-stage, Claude API diarahkan ke `server`.
fn staged_project(app: &TestApp, server: &MockServer) -> std::path::PathBuf {
    let mut settings = app.core.user_settings();
    settings.ai.base_url = Some(server.uri());
    app.core.set_user_settings(settings).unwrap();
    let project = app.git_project(&[("a.txt", "satu\n")]);
    std::fs::write(project.join("a.txt"), "dua\n").unwrap();
    git(&project, &["add", "a.txt"]);
    project
}

/// Jumlah berkas ter-stage di satu-satunya repositori yang terbuka. Akarnya dibaca dari aplikasi:
/// di Windows git menulisnya dengan garis miring maju, berbeda dari path proyek.
fn staged_count(cx: &gpui_kit::App) -> Option<usize> {
    let repos = repos(cx).read(cx);
    let root = repos.roots().into_iter().next()?;
    repos.snapshot(&root).map(|s| s.staged.len())
}

async fn open_scm(app: &TestApp, project: &std::path::Path, cx: &mut TestAppContext) -> (TestWindow, Entity<ScmPanel>) {
    let window = app.open(Some(project), cx).await;
    window.wait_until(cx, "perubahan ter-stage terbaca", |_, cx| staged_count(cx) == Some(1)).await;
    assert!(window.run_command(cx, "scm.focus"));
    cx.run_until_parked();
    window.update(cx, |window, cx| window.render_frame(cx));
    let workbench = window.workbench(cx);
    let store = cx.update(|cx| workbench.read(cx).extension::<Entity<ScmStore>>()).expect("store Source Control");
    window.wait_until(cx, "panel Source Control", |_, cx| store.read(cx).panel().is_some()).await;
    let panel = cx.update(|cx| store.read(cx).panel()).unwrap();
    (window, panel)
}

fn click(window: &TestWindow, cx: &mut TestAppContext, id: &'static str) {
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
async fn generated_message_fills_the_commit_box_and_can_be_committed(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_scm::init);
    let server = app.runtime.block_on(claude());
    let project = staged_project(&app, &server);
    let core = app.core.clone();
    app.runtime.block_on(async move { ai_api::ai_set_api_key(&core, "sk-ant-uji".into()).await }).unwrap();
    let (window, panel) = open_scm(&app, &project, cx).await;

    click(&window, cx, "scm-ai-generate");
    window.wait_until(cx, "pesan dari Claude", |_, cx| !panel.read(cx).is_generating() && panel.read(cx).message(cx) == SUBJECT).await;

    // Diff yang di-stage terkirim.
    let requests = app.runtime.block_on(server.received_requests()).unwrap();
    assert_eq!(requests.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    let content = body["messages"][0]["content"].as_str().unwrap();
    assert!(content.contains("-satu") && content.contains("+dua"), "{content}");

    click(&window, cx, "scm-commit-button");
    window.wait_until(cx, "commit selesai", |_, cx| staged_count(cx) == Some(0)).await;
    assert_eq!(git(&project, &["log", "-1", "--format=%s"]).trim(), SUBJECT);
}

#[gpui_kit::test]
async fn missing_api_key_is_asked_for_then_generation_continues(cx: &mut TestAppContext) {
    if std::env::var_os("ANTHROPIC_API_KEY").is_some() {
        return; // Key dari lingkungan dipakai; tidak ada prompt yang bisa diuji.
    }
    let app = TestApp::new(cx);
    cx.update(oncode_ui_scm::init);
    let server = app.runtime.block_on(claude());
    let project = staged_project(&app, &server);
    let (window, panel) = open_scm(&app, &project, cx).await;

    click(&window, cx, "scm-ai-generate");
    // Prompt API key muncul; key diisi lalu pembuatan pesan diulang.
    for _ in 0..300 {
        let open = window.update(cx, |window, cx| {
            use gpui_kit::component::WindowExt as _;
            window.has_active_dialog(cx)
        });
        if open {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.run_until_parked();
    }
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input("sk-ant-uji", cx);
        window.press("enter", cx);
    });
    cx.run_until_parked();

    window.wait_until(cx, "pesan dari Claude", |_, cx| !panel.read(cx).is_generating() && panel.read(cx).message(cx) == SUBJECT).await;
    let core = app.core.clone();
    assert!(app.runtime.block_on(async move { ai_api::ai_has_api_key(&core).await }).unwrap(), "key tersimpan");
}
