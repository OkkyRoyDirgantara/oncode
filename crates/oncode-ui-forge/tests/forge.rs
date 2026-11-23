//! Uji tampilan GitLab terhadap server HTTP tiruan di mesin lokal: login dengan token, daftar
//! merge request di sidebar, dan tab detailnya.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use oncode_core::api::forge as forge_api;
use oncode_forge::ForgeKind;
use oncode_ui::testing::{git, TestApp};
use oncode_ui::ViewTab;
use oncode_ui_forge::views::PullRequestTab;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PROJECT: &str = "/api/v4/projects/tim%2Fproyek";

fn mr(iid: u32, title: &str, base: &str) -> serde_json::Value {
    json!({
        "id": 1000 + iid, "iid": iid, "title": title, "state": "opened", "draft": false, "work_in_progress": false,
        "author": { "username": "budi", "name": "Budi", "avatar_url": null },
        "source_branch": format!("fitur-{iid}"), "target_branch": "main", "sha": "8888888888888888888888888888888888888888",
        "updated_at": "2026-11-01T10:00:00.000Z", "web_url": format!("{base}/tim/proyek/-/merge_requests/{iid}"),
        "source_project_id": 42, "target_project_id": 42, "labels": ["bug"], "reviewers": [],
        "description": "## Ringkasan\n\nMemperbaiki *login*.", "merge_status": "can_be_merged", "has_conflicts": false, "changes_count": "2",
    })
}

async fn gitlab() -> MockServer {
    let server = MockServer::start().await;
    let base = server.uri();
    Mock::given(method("GET")).and(path("/api/v4/user")).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 7, "username": "budi", "name": "Budi Santoso", "avatar_url": null }))).mount(&server).await;
    Mock::given(method("GET")).and(path(format!("{PROJECT}/merge_requests"))).respond_with(ResponseTemplate::new(200).set_body_json(json!([mr(12, "Perbaiki login", &base), mr(13, "Tambah halaman profil", &base)]))).mount(&server).await;
    Mock::given(method("GET")).and(path(format!("{PROJECT}/merge_requests/12"))).respond_with(ResponseTemplate::new(200).set_body_json(mr(12, "Perbaiki login", &base))).mount(&server).await;
    Mock::given(method("GET")).and(path(format!("{PROJECT}/merge_requests/12/commits"))).respond_with(ResponseTemplate::new(200).insert_header("x-total", "1").set_body_json(json!([{ "id": "a" }]))).mount(&server).await;
    server
}

#[gpui_kit::test]
async fn gitlab_token_login_lists_merge_requests_and_opens_one(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_forge::init);
    let server = app.runtime.block_on(gitlab());
    let base = server.uri();

    // Login dengan token (jalur yang sama dengan dialog "Use a personal access token").
    let core = app.core.clone();
    let account = app.runtime.block_on(forge_api::forge_login_token(&core, ForgeKind::GitLab, Some(base.clone()), "glpat-uji".into())).expect("login token");
    assert_eq!(account.username, "budi");

    let project = app.git_project(&[("README.md", "# Proyek\n")]);
    git(&project, &["remote", "add", "origin", &format!("{base}/tim/proyek.git")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    window.wait_until(cx, "sidebar PR", |_, cx| oncode_ui_forge::pull_requests(workbench.read(cx)).is_some()).await;
    let list = cx.update(|cx| oncode_ui_forge::pull_requests(workbench.read(cx)).unwrap());
    window.wait_until(cx, "daftar MR dimuat", |_, cx| list.read(cx).items().len() == 2).await;
    let titles: Vec<String> = cx.update(|cx| list.read(cx).items().iter().map(|pr| pr.title.clone()).collect());
    assert_eq!(titles, ["Perbaiki login", "Tambah halaman profil"]);

    // Klik MR membuka tab detailnya.
    cx.update(|cx| workbench.update(cx, |w, cx| w.show_sidebar("pullRequests", cx)));
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click("forge-pr-12", cx);
    });
    cx.run_until_parked();
    let tab = cx.update(|cx| workbench.read(cx).active_content().and_then(|c| c.as_any().downcast_ref::<ViewTab<PullRequestTab>>().map(|t| t.0.clone())).expect("tab MR aktif"));
    window.wait_until(cx, "detail dimuat", |_, cx| tab.read(cx).detail().is_some()).await;
    let detail = cx.update(|cx| tab.read(cx).detail().cloned().unwrap());
    assert_eq!(detail.pull_request.number, 12);
    assert_eq!(detail.body, "## Ringkasan\n\nMemperbaiki *login*.");
    assert_eq!(detail.labels, ["bug"]);
}
