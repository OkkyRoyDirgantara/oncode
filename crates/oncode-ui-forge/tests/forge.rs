//! Tests of the GitLab views against a mock HTTP server on the local machine: token login, the
//! merge request list in the sidebar, and its detail tab.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use oncode_core::api::forge as forge_api;
use oncode_forge::ForgeKind;
use oncode_ui::testing::{git, TestApp, TestWindow};
use oncode_ui::ViewTab;
use oncode_ui_forge::views::PullRequestTab;
use serde_json::json;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PROJECT: &str = "/api/v4/projects/team%2Fproject";

fn mr(iid: u32, title: &str, base: &str) -> serde_json::Value {
    json!({
        "id": 1000 + iid, "iid": iid, "title": title, "state": "opened", "draft": false, "work_in_progress": false,
        "author": { "username": "budi", "name": "Budi", "avatar_url": null },
        "source_branch": format!("feature-{iid}"), "target_branch": "main", "sha": "8888888888888888888888888888888888888888",
        "updated_at": "2026-11-01T10:00:00.000Z", "web_url": format!("{base}/team/project/-/merge_requests/{iid}"),
        "source_project_id": 42, "target_project_id": 42, "labels": ["bug"], "reviewers": [],
        "description": "## Summary\n\nFixes *login*.", "merge_status": "can_be_merged", "has_conflicts": false, "changes_count": "2",
    })
}

async fn gitlab() -> MockServer {
    let server = MockServer::start().await;
    let base = server.uri();
    Mock::given(method("GET")).and(path("/api/v4/user")).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 7, "username": "budi", "name": "Budi Santoso", "avatar_url": null }))).mount(&server).await;
    Mock::given(method("GET")).and(path(format!("{PROJECT}/merge_requests"))).respond_with(ResponseTemplate::new(200).set_body_json(json!([mr(12, "Fix login", &base), mr(13, "Add profile page", &base)]))).mount(&server).await;
    Mock::given(method("GET")).and(path(format!("{PROJECT}/merge_requests/12"))).respond_with(ResponseTemplate::new(200).set_body_json(mr(12, "Fix login", &base))).mount(&server).await;
    Mock::given(method("GET")).and(path(format!("{PROJECT}/merge_requests/12/commits"))).respond_with(ResponseTemplate::new(200).insert_header("x-total", "1").set_body_json(json!([{ "id": "a" }]))).mount(&server).await;
    server
}

#[gpui_kit::test]
async fn gitlab_token_login_lists_merge_requests_and_opens_one(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_forge::init);
    let server = app.runtime.block_on(gitlab());
    let base = server.uri();

    // Login with a token (the same path as the "Use a personal access token" dialog).
    let core = app.core.clone();
    let account = app.runtime.block_on(forge_api::forge_login_token(&core, ForgeKind::GitLab, Some(base.clone()), "glpat-test".into())).expect("login token");
    assert_eq!(account.username, "budi");

    let project = app.git_project(&[("README.md", "# Project\n")]);
    git(&project, &["remote", "add", "origin", &format!("{base}/team/project.git")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    window.wait_until(cx, "sidebar PR", |_, cx| oncode_ui_forge::pull_requests(workbench.read(cx)).is_some()).await;
    let list = cx.update(|cx| oncode_ui_forge::pull_requests(workbench.read(cx)).unwrap());
    window.wait_until(cx, "MR list loaded", |_, cx| list.read(cx).items().len() == 2).await;
    let titles: Vec<String> = cx.update(|cx| list.read(cx).items().iter().map(|pr| pr.title.clone()).collect());
    assert_eq!(titles, ["Fix login", "Add profile page"]);

    // Clicking an MR opens its detail tab.
    cx.update(|cx| workbench.update(cx, |w, cx| w.show_sidebar("pullRequests", cx)));
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click("forge-pr-12", cx);
    });
    cx.run_until_parked();
    let tab = cx.update(|cx| workbench.read(cx).active_content().and_then(|c| c.as_any().downcast_ref::<ViewTab<PullRequestTab>>().map(|t| t.0.clone())).expect("active MR tab"));
    window.wait_until(cx, "detail loaded", |_, cx| tab.read(cx).detail().is_some()).await;
    let detail = cx.update(|cx| tab.read(cx).detail().cloned().unwrap());
    assert_eq!(detail.pull_request.number, 12);
    assert_eq!(detail.body, "## Summary\n\nFixes *login*.");
    assert_eq!(detail.labels, ["bug"]);
}

/// Picks a quick pick item by typing, then Enter.
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

/// Waits for the dialog's input box, then types and presses Enter.
async fn answer(window: &TestWindow, cx: &mut TestAppContext, text: &str) {
    for _ in 0..300 {
        let open = window.update(cx, |window, cx| {
            use gpui_kit::component::WindowExt as _;
            window.render_frame(cx);
            window.has_active_dialog(cx) && window.try_find("choice-token").is_none()
        });
        if open {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.run_until_parked();
    }
    cx.run_until_parked();
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input(text, cx);
        window.press("enter", cx);
    });
    cx.run_until_parked();
}

async fn click_when_ready(window: &TestWindow, cx: &mut TestAppContext, id: &'static str) {
    for _ in 0..300 {
        let found = window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
        });
        if found {
            window.update(cx, |window, cx| window.click(id, cx));
            cx.run_until_parked();
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.run_until_parked();
    }
    panic!("button {id} did not appear");
}

/// Sign In → GitLab (self-hosted) → server address → via browser, until the
/// "no Client ID yet" choice dialog shows.
async fn start_browser_sign_in(window: &TestWindow, cx: &mut TestAppContext, base: &str) {
    assert!(window.run_command(cx, "forge.signIn"));
    pick(window, cx, "self-hosted").await;
    answer(window, cx, base).await;
    pick(window, cx, "browser").await;
}

async fn wait_for_account(app: &TestApp, window: &TestWindow, cx: &mut TestAppContext, username: &'static str) {
    let core = app.core.clone();
    window
        .wait_until(cx, "account saved", |_, _| {
            let core = core.clone();
            app.runtime.block_on(async move { forge_api::forge_accounts(&core).await }).is_ok_and(|accounts| accounts.iter().any(|a| a.username == username))
        })
        .await;
}

#[gpui_kit::test]
async fn self_hosted_browser_sign_in_without_client_id_creates_a_token(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_forge::init);
    let server = app.runtime.block_on(gitlab());
    let base = server.uri();
    let window = app.open(None, cx).await;

    // Without a Client ID for this server: the token creation page opens with the name and scopes filled in.
    start_browser_sign_in(&window, cx, &base).await;
    click_when_ready(&window, cx, "choice-token").await;
    assert_eq!(cx.opened_url(), Some(format!("{base}/-/user_settings/personal_access_tokens?name=OnCode&scopes=api,read_user")));
    answer(&window, cx, "glpat-test").await;

    wait_for_account(&app, &window, cx, "budi").await;
    assert!(app.core.user_settings().forge.client_ids.is_empty(), "the token path does not save a Client ID");
}

#[gpui_kit::test]
async fn self_hosted_browser_sign_in_saves_the_client_id_of_the_server(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_forge::init);
    let server = app.runtime.block_on(async {
        let server = MockServer::start().await;
        let base = server.uri();
        Mock::given(method("POST"))
            .and(path("/oauth/authorize_device"))
            .and(body_string_contains("client_id=server-client"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "device_code": "dev-1", "user_code": "WXYZ-0001", "verification_uri": format!("{base}/oauth/device"), "expires_in": 300, "interval": 1 })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .and(body_string_contains("client_id=server-client"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "access_token": "token-server", "refresh_token": "fresh", "expires_in": 7200, "token_type": "Bearer" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v4/user"))
            .and(header("authorization", "Bearer token-server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 9, "username": "sari", "name": "Sari", "avatar_url": null })))
            .mount(&server)
            .await;
        server
    });
    let base = server.uri();
    let window = app.open(None, cx).await;

    // The server's OAuth application Client ID is entered during login, then the device flow runs.
    start_browser_sign_in(&window, cx, &base).await;
    click_when_ready(&window, cx, "choice-client-id").await;
    answer(&window, cx, "server-client").await;

    wait_for_account(&app, &window, cx, "sari").await;
    assert_eq!(cx.opened_url(), Some(format!("{base}/oauth/device")), "verification page opened in the browser");
    assert_eq!(app.core.user_settings().forge.client_ids.get(&base).map(String::as_str), Some("server-client"));
}
