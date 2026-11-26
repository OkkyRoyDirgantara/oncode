//! Tests of "Generate Commit Message": a mock Claude API on the local machine, the commit box, and the API key prompt.

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

const SUBJECT: &str = "fix: change contents of a.txt";

/// A mock Claude API that streams `SUBJECT` in two pieces for the key `sk-ant-test`.
async fn claude() -> MockServer {
    let server = MockServer::start().await;
    let events = [
        json!({"type": "message_start", "message": {"id": "msg_1", "model": "claude-opus-5-5"}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "fix: change "}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "contents of a.txt"}}),
        json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}}),
        json!({"type": "message_stop"}),
    ];
    let body: String = events.iter().map(|e| format!("event: {}\ndata: {}\n\n", e["type"].as_str().unwrap(), e)).collect();
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "sk-ant-test"))
        .respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(body))
        .mount(&server)
        .await;
    server
}

/// A project with one change that is already staged, with the Claude API pointed at `server`.
fn staged_project(app: &TestApp, server: &MockServer) -> std::path::PathBuf {
    let mut settings = app.core.user_settings();
    settings.ai.base_url = Some(server.uri());
    // Always through the mock API: with "Automatic", the Claude Code installed on the test machine would be used.
    settings.ai.provider = oncode_core::settings::AiProvider::Api;
    app.core.set_user_settings(settings).unwrap();
    let project = app.git_project(&[("a.txt", "one\n")]);
    std::fs::write(project.join("a.txt"), "two\n").unwrap();
    git(&project, &["add", "a.txt"]);
    project
}

/// The number of staged files in the only repository that is open. The root is read from the app:
/// on Windows git writes it with forward slashes, unlike the project path.
fn staged_count(cx: &gpui_kit::App) -> Option<usize> {
    let repos = repos(cx).read(cx);
    let root = repos.roots().into_iter().next()?;
    repos.snapshot(&root).map(|s| s.staged.len())
}

async fn open_scm(app: &TestApp, project: &std::path::Path, cx: &mut TestAppContext) -> (TestWindow, Entity<ScmPanel>) {
    let window = app.open(Some(project), cx).await;
    window.wait_until(cx, "staged change read", |_, cx| staged_count(cx) == Some(1)).await;
    assert!(window.run_command(cx, "scm.focus"));
    cx.run_until_parked();
    window.update(cx, |window, cx| window.render_frame(cx));
    let workbench = window.workbench(cx);
    let store = cx.update(|cx| workbench.read(cx).extension::<Entity<ScmStore>>()).expect("Source Control store");
    window.wait_until(cx, "Source Control panel", |_, cx| store.read(cx).panel().is_some()).await;
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
    app.runtime.block_on(async move { ai_api::ai_set_api_key(&core, "sk-ant-test".into()).await }).unwrap();
    let (window, panel) = open_scm(&app, &project, cx).await;

    click(&window, cx, "scm-ai-generate");
    window.wait_until(cx, "message from Claude", |_, cx| !panel.read(cx).is_generating() && panel.read(cx).message(cx) == SUBJECT).await;

    // The staged diff was sent.
    let requests = app.runtime.block_on(server.received_requests()).unwrap();
    assert_eq!(requests.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    let content = body["messages"][0]["content"].as_str().unwrap();
    assert!(content.contains("-one") && content.contains("+two"), "{content}");

    click(&window, cx, "scm-commit-button");
    window.wait_until(cx, "commit finished", |_, cx| staged_count(cx) == Some(0)).await;
    assert_eq!(git(&project, &["log", "-1", "--format=%s"]).trim(), SUBJECT);
}

#[gpui_kit::test]
async fn missing_api_key_is_asked_for_then_generation_continues(cx: &mut TestAppContext) {
    if std::env::var_os("ANTHROPIC_API_KEY").is_some() {
        return; // The key from the environment is used; there is no prompt to test.
    }
    let app = TestApp::new(cx);
    cx.update(oncode_ui_scm::init);
    let server = app.runtime.block_on(claude());
    let project = staged_project(&app, &server);
    let (window, panel) = open_scm(&app, &project, cx).await;

    click(&window, cx, "scm-ai-generate");
    // The API key prompt appears; the key is entered, then message generation is repeated.
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
        window.input("sk-ant-test", cx);
        window.press("enter", cx);
    });
    cx.run_until_parked();

    window.wait_until(cx, "message from Claude", |_, cx| !panel.read(cx).is_generating() && panel.read(cx).message(cx) == SUBJECT).await;
    let core = app.core.clone();
    assert!(app.runtime.block_on(async move { ai_api::ai_has_api_key(&core).await }).unwrap(), "key saved");
}

/// A custom OpenAI-compatible endpoint (e.g. OpenRouter): a rejected key asks for that endpoint's key,
/// then the message is generated again.
#[gpui_kit::test]
async fn custom_endpoint_asks_for_its_own_key_when_rejected(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_scm::init);
    let server = app.runtime.block_on(async {
        let server = MockServer::start().await;
        let chunk = |text: &str| format!("data: {}\n\n", json!({"choices": [{"index": 0, "delta": {"content": text}}]}));
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header("authorization", "Bearer custom-key"))
            .respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(format!("{}{}data: [DONE]\n\n", chunk("fix: change "), chunk("contents of a.txt"))))
            .mount(&server)
            .await;
        Mock::given(method("POST")).and(path("/v1/chat/completions")).respond_with(ResponseTemplate::new(401).set_body_json(json!({"error": {"message": "missing key"}}))).mount(&server).await;
        server
    });
    let project = staged_project(&app, &server);
    let mut settings = app.core.user_settings();
    settings.ai.provider = oncode_core::settings::AiProvider::Custom;
    settings.ai.custom_base_url = Some(format!("{}/v1", server.uri()));
    settings.ai.custom_model = Some("custom-model".into());
    app.core.set_user_settings(settings).unwrap();
    let (window, panel) = open_scm(&app, &project, cx).await;

    click(&window, cx, "scm-ai-generate");
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
        window.input("custom-key", cx);
        window.press("enter", cx);
    });
    cx.run_until_parked();
    window.wait_until(cx, "message from the custom endpoint", |_, cx| !panel.read(cx).is_generating() && panel.read(cx).message(cx) == SUBJECT).await;
    let core = app.core.clone();
    assert!(!app.runtime.block_on(async move { ai_api::ai_has_api_key(&core).await }).unwrap() || std::env::var_os("ANTHROPIC_API_KEY").is_some(), "the Anthropic key is untouched");
}
