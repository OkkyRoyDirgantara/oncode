//! The GitLab provider against a mock HTTP server.

mod common;

use common::{assert_error, header, query_value, received, repo};
use oncode_forge::{
    provider, CheckState, ForgeErrorCode, ForgeHost, ForgeKind, ForgeProvider, NewPullRequest, PrFilter, PrState,
    RepoRef,
};
use serde_json::{json, Value};
use wiremock::matchers::{body_json, method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "glpat-token_secret_456";
/// The path of project `team/sub/project` after encoding.
const PROJECT: &str = "/projects/team%2Fsub%2Fproject";

fn client(server: &MockServer) -> Box<dyn ForgeProvider + Send + Sync> {
    provider(&ForgeHost::gitlab(), TOKEN, Some(&server.uri()))
}

fn proyek() -> RepoRef {
    repo("gitlab.com", "team/sub", "project")
}

fn project_json(path_with_namespace: &str) -> Value {
    json!({
        "id": 42,
        "path_with_namespace": path_with_namespace,
        "description": null,
        "visibility": "public",
        "http_url_to_repo": format!("https://gitlab.com/{path_with_namespace}.git"),
        "ssh_url_to_repo": format!("git@gitlab.com:{path_with_namespace}.git"),
        "default_branch": "main",
        "last_activity_at": "2024-05-01T12:34:56.500Z",
        "updated_at": "2020-01-01T00:00:00.000Z",
        "web_url": format!("https://gitlab.com/{path_with_namespace}"),
    })
}

fn mr_json(iid: u32, title: &str) -> Value {
    json!({
        "id": 1000 + iid,
        "iid": iid,
        "title": title,
        "state": "opened",
        "draft": false,
        "work_in_progress": false,
        "author": { "username": "alice", "name": "Alice", "avatar_url": "https://gitlab.com/uploads/alice.png" },
        "source_branch": "feature",
        "target_branch": "main",
        "sha": "8888888888888888888888888888888888888888",
        "updated_at": "2024-05-01T19:34:56.000+07:00",
        "web_url": format!("https://gitlab.com/team/sub/project/-/merge_requests/{iid}"),
        "source_project_id": 42,
        "target_project_id": 42,
        "labels": [],
        "reviewers": [],
    })
}

#[tokio::test]
async fn current_user_and_request_headers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 7,
            "username": "alice",
            "name": "Alice Smith",
            "avatar_url": "https://gitlab.com/uploads/-/system/user/avatar/7/avatar.png",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let account = client(&server).current_user().await.expect("current_user");
    assert_eq!(account.id, "gitlab:gitlab.com:alice");
    assert_eq!(account.kind, ForgeKind::GitLab);
    assert_eq!(account.host, "https://gitlab.com");
    assert_eq!(account.username, "alice");
    assert_eq!(account.display_name, "Alice Smith");
    assert_eq!(account.avatar_url.as_deref(), Some("https://gitlab.com/uploads/-/system/user/avatar/7/avatar.png"));

    let requests = received(&server).await;
    let request = &requests[0];
    assert_eq!(header(request, "authorization"), Some("Bearer glpat-token_secret_456"));
    assert_eq!(header(request, "user-agent"), Some("OnCode"));
    assert_eq!(header(request, "accept"), Some("application/json"));
    assert_eq!(header(request, "private-token"), None);
}

#[tokio::test]
async fn self_hosted_relative_avatar_becomes_absolute() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "username": "carol",
            "name": "",
            "avatar_url": "/uploads/-/system/user/avatar/3/carol.png",
        })))
        .mount(&server)
        .await;

    let host = ForgeHost::new(ForgeKind::GitLab, "https://git.example.com/");
    let account = provider(&host, TOKEN, Some(&server.uri())).current_user().await.expect("current_user");
    assert_eq!(account.id, "gitlab:git.example.com:carol");
    assert_eq!(account.host, "https://git.example.com");
    assert_eq!(account.display_name, "carol");
    assert_eq!(
        account.avatar_url.as_deref(),
        Some("https://git.example.com/uploads/-/system/user/avatar/3/carol.png")
    );
}

#[tokio::test]
async fn list_repos_pages_through_next_page_header() {
    let server = MockServer::start().await;
    let mut internal_fork = project_json("team/internal");
    internal_fork["visibility"] = json!("internal");
    internal_fork["forked_from_project"] = json!({ "id": 1, "path_with_namespace": "origin/internal" });
    internal_fork["description"] = json!("Internal project");
    let mut empty_project = project_json("team/empty");
    empty_project["visibility"] = json!("private");
    empty_project["default_branch"] = Value::Null;
    Mock::given(method("GET"))
        .and(path("/projects"))
        .and(query_param("membership", "true"))
        .and(query_param("order_by", "last_activity_at"))
        .and(query_param("sort", "desc"))
        .and(query_param("per_page", "30"))
        .and(query_param("page", "1"))
        .and(query_param_is_missing("search"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-next-page", "2")
                .insert_header("x-total-pages", "2")
                .set_body_json(json!([project_json("team/sub/project"), internal_fork, empty_project])),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/projects"))
        .and(query_param("per_page", "30"))
        .and(query_param("page", "2"))
        .and(query_param_is_missing("search"))
        .respond_with(
            ResponseTemplate::new(200).insert_header("x-next-page", "").set_body_json(json!([project_json("team/last")])),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = client(&server);
    let first = client.list_repos("", 1).await.expect("list_repos");
    assert_eq!(first.next_page, Some(2));
    assert_eq!(first.items.len(), 3);
    let project = &first.items[0];
    assert_eq!(project.full_name, "team/sub/project");
    assert_eq!(project.description, "");
    assert!(!project.private);
    assert!(!project.fork);
    assert_eq!(project.clone_url_https, "https://gitlab.com/team/sub/project.git");
    assert_eq!(project.clone_url_ssh, "git@gitlab.com:team/sub/project.git");
    assert_eq!(project.default_branch, "main");
    assert_eq!(project.updated_at, 1_714_566_896.5);
    assert_eq!(project.web_url, "https://gitlab.com/team/sub/project");
    assert!(first.items[1].private, "internal counts as private");
    assert!(first.items[1].fork);
    assert_eq!(first.items[1].description, "Internal project");
    assert!(first.items[2].private);
    assert_eq!(first.items[2].default_branch, "");

    let last = client.list_repos("", 2).await.expect("list_repos");
    assert_eq!(last.next_page, None);
    assert_eq!(last.items[0].full_name, "team/last");
}

#[tokio::test]
async fn list_repos_searches_server_side() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/projects"))
        .and(query_param("membership", "true"))
        .and(query_param("search", "on code"))
        .and(query_param("per_page", "30"))
        .and(query_param("page", "3"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([project_json("team/oncode")])))
        .expect(1)
        .mount(&server)
        .await;

    let found = client(&server).list_repos("  on code ", 3).await.expect("list_repos");
    assert_eq!(found.items.len(), 1);
    assert_eq!(found.next_page, None);
}

#[tokio::test]
async fn list_pull_requests_sends_filter_query() {
    let server = MockServer::start().await;
    let merge_requests = format!("{PROJECT}/merge_requests");
    let mut draft = mr_json(4, "Draft: Work in progress");
    draft["draft"] = json!(true);
    let mut from_fork = mr_json(3, "From fork");
    from_fork["source_project_id"] = json!(99);
    from_fork["author"] = json!({ "username": "guest", "avatar_url": "/uploads/guest.png" });

    // Open
    Mock::given(method("GET"))
        .and(path(merge_requests.as_str()))
        .and(query_param("state", "opened"))
        .and(query_param("order_by", "updated_at"))
        .and(query_param("sort", "desc"))
        .and(query_param("per_page", "30"))
        .and(query_param("page", "1"))
        .and(query_param_is_missing("scope"))
        .and(query_param_is_missing("reviewer_username"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-next-page", "2")
                .set_body_json(json!([mr_json(5, "Add feature"), draft, from_fork])),
        )
        .expect(1)
        .mount(&server)
        .await;
    // All
    let mut merged = mr_json(2, "Already merged");
    merged["state"] = json!("merged");
    let mut closed = mr_json(1, "Closed");
    closed["state"] = json!("closed");
    Mock::given(method("GET"))
        .and(path(merge_requests.as_str()))
        .and(query_param("state", "all"))
        .and(query_param("order_by", "updated_at"))
        .and(query_param("sort", "desc"))
        .and(query_param("per_page", "30"))
        .and(query_param("page", "2"))
        .and(query_param_is_missing("scope"))
        .respond_with(ResponseTemplate::new(200).insert_header("x-next-page", "").set_body_json(json!([merged, closed])))
        .expect(1)
        .mount(&server)
        .await;
    // Mine
    Mock::given(method("GET"))
        .and(path(merge_requests.as_str()))
        .and(query_param("state", "opened"))
        .and(query_param("scope", "created_by_me"))
        .and(query_param("order_by", "updated_at"))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([mr_json(5, "Add feature")])))
        .expect(1)
        .mount(&server)
        .await;
    // ReviewRequested: the username is fetched from /user, only once.
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "username": "alice", "name": "Alice" })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(merge_requests.as_str()))
        .and(query_param("state", "opened"))
        .and(query_param("reviewer_username", "alice"))
        .and(query_param("order_by", "updated_at"))
        .and(query_param_is_missing("scope"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([mr_json(6, "Please review")])))
        .expect(2)
        .mount(&server)
        .await;

    let client = client(&server);
    let open = client.list_pull_requests(&proyek(), PrFilter::Open, 1).await.expect("open");
    assert_eq!(open.next_page, Some(2));
    assert_eq!(open.items.len(), 3);
    let first = &open.items[0];
    assert_eq!(first.number, 5);
    assert_eq!(first.title, "Add feature");
    assert_eq!(first.state, PrState::Open);
    assert!(!first.draft);
    assert_eq!(first.author, "alice");
    assert_eq!(first.author_avatar.as_deref(), Some("https://gitlab.com/uploads/alice.png"));
    assert_eq!(first.source_branch, "feature");
    assert_eq!(first.target_branch, "main");
    assert_eq!(first.head_sha, "8888888888888888888888888888888888888888");
    assert_eq!(first.updated_at, 1_714_566_896.0);
    assert_eq!(first.web_url, "https://gitlab.com/team/sub/project/-/merge_requests/5");
    assert!(!first.is_cross_repo);
    assert!(open.items[1].draft);
    assert!(open.items[2].is_cross_repo);
    assert_eq!(open.items[2].author_avatar.as_deref(), Some("https://gitlab.com/uploads/guest.png"));

    let all = client.list_pull_requests(&proyek(), PrFilter::All, 2).await.expect("all");
    assert_eq!(all.next_page, None);
    assert_eq!(all.items[0].state, PrState::Merged);
    assert_eq!(all.items[1].state, PrState::Closed);

    let mine = client.list_pull_requests(&proyek(), PrFilter::Mine, 1).await.expect("mine");
    assert_eq!(mine.items.iter().map(|p| p.number).collect::<Vec<_>>(), [5]);

    let review = client.list_pull_requests(&proyek(), PrFilter::ReviewRequested, 1).await.expect("review");
    assert_eq!(review.items.iter().map(|p| p.number).collect::<Vec<_>>(), [6]);
    // The second call uses the username that was already remembered.
    client.list_pull_requests(&proyek(), PrFilter::ReviewRequested, 1).await.expect("review again");

    // The project path is sent encoded, not as separate segments.
    let requests = received(&server).await;
    assert!(requests[0].url.as_str().contains("/projects/team%2Fsub%2Fproject/merge_requests?"), "{}", requests[0].url);
    assert_eq!(query_value(&requests[0], "state").as_deref(), Some("opened"));
}

#[tokio::test]
async fn get_pull_request_merged_with_details() {
    let server = MockServer::start().await;
    let mut mr = mr_json(12, "Fix login");
    mr["state"] = json!("merged");
    mr["description"] = json!("## Summary\n\nFixes *login*.");
    mr["reviewers"] = json!([{ "username": "carol" }, { "username": "dave" }]);
    mr["labels"] = json!(["bug", "priority"]);
    mr["merge_status"] = json!("can_be_merged");
    mr["has_conflicts"] = json!(false);
    mr["changes_count"] = json!("5");
    Mock::given(method("GET"))
        .and(path(format!("{PROJECT}/merge_requests/12")))
        .respond_with(ResponseTemplate::new(200).set_body_json(mr))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{PROJECT}/merge_requests/12/commits")))
        .and(query_param("per_page", "100"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-total", "3")
                .set_body_json(json!([{ "id": "a" }, { "id": "b" }, { "id": "c" }])),
        )
        .expect(1)
        .mount(&server)
        .await;

    let detail = client(&server).get_pull_request(&proyek(), 12).await.expect("detail");
    assert_eq!(detail.pull_request.number, 12);
    assert_eq!(detail.pull_request.state, PrState::Merged);
    assert!(!detail.pull_request.draft);
    assert_eq!(detail.body, "## Summary\n\nFixes *login*.");
    assert_eq!(detail.reviewers, ["carol", "dave"]);
    assert_eq!(detail.labels, ["bug", "priority"]);
    assert_eq!(detail.mergeable, Some(true));
    assert_eq!(detail.commits, 3);
    assert_eq!(detail.changed_files, 5);
    // GitLab does not provide line counts.
    assert_eq!(detail.additions, 0);
    assert_eq!(detail.deletions, 0);
}

#[tokio::test]
async fn get_pull_request_draft_with_conflicts() {
    let server = MockServer::start().await;
    let mut mr = mr_json(13, "Draft: Work in progress");
    // Old instances only send `work_in_progress`.
    mr.as_object_mut().expect("object").remove("draft");
    mr["work_in_progress"] = json!(true);
    mr["description"] = Value::Null;
    mr["merge_status"] = json!("cannot_be_merged");
    mr["has_conflicts"] = json!(true);
    mr["changes_count"] = json!("1000+");
    mr["labels"] = json!([{ "name": "backend", "color": "#ff0000" }]);
    Mock::given(method("GET"))
        .and(path(format!("{PROJECT}/merge_requests/13")))
        .respond_with(ResponseTemplate::new(200).set_body_json(mr))
        .mount(&server)
        .await;
    // Without the `x-total` header, the count is computed from the response body.
    Mock::given(method("GET"))
        .and(path(format!("{PROJECT}/merge_requests/13/commits")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "id": "a" }, { "id": "b" }])))
        .mount(&server)
        .await;

    let detail = client(&server).get_pull_request(&proyek(), 13).await.expect("detail");
    assert_eq!(detail.pull_request.state, PrState::Open);
    assert!(detail.pull_request.draft);
    assert_eq!(detail.body, "");
    assert_eq!(detail.mergeable, Some(false));
    assert_eq!(detail.commits, 2);
    assert_eq!(detail.changed_files, 1000);
    assert_eq!(detail.labels, ["backend"]);
}

#[tokio::test]
async fn get_pull_request_mergeability_unknown_while_checking() {
    let server = MockServer::start().await;
    let mut mr = mr_json(14, "Newly created");
    mr["merge_status"] = json!("checking");
    mr["changes_count"] = Value::Null;
    Mock::given(method("GET"))
        .and(path(format!("{PROJECT}/merge_requests/14")))
        .respond_with(ResponseTemplate::new(200).set_body_json(mr))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{PROJECT}/merge_requests/14/commits")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    let detail = client(&server).get_pull_request(&proyek(), 14).await.expect("detail");
    assert_eq!(detail.mergeable, None);
    assert_eq!(detail.changed_files, 0);
    assert_eq!(detail.commits, 0);
}

#[tokio::test]
async fn list_checks_maps_latest_pipeline_jobs() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("{PROJECT}/pipelines")))
        .and(query_param("sha", "abc123"))
        .and(query_param("order_by", "id"))
        .and(query_param("sort", "desc"))
        .and(query_param("per_page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": 77, "status": "failed", "web_url": "https://gitlab.com/team/sub/project/-/pipelines/77" },
        ])))
        .expect(1)
        .mount(&server)
        .await;
    let job = |id: u32, name: &str, status: &str, allow_failure: bool| {
        json!({
            "id": id,
            "name": name,
            "status": status,
            "allow_failure": allow_failure,
            "web_url": format!("https://gitlab.com/team/sub/project/-/jobs/{id}"),
        })
    };
    // The API sorts from the largest id; the final result must be in ascending order.
    Mock::given(method("GET"))
        .and(path(format!("{PROJECT}/pipelines/77/jobs")))
        .and(query_param("per_page", "100"))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).insert_header("x-next-page", "2").set_body_json(json!([
            job(10, "manual", "manual", false),
            job(9, "skipped", "skipped", false),
            job(8, "canceled", "canceled", false),
            job(7, "allowed-to-fail", "failed", true),
            job(6, "failed", "failed", false),
        ])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{PROJECT}/pipelines/77/jobs")))
        .and(query_param("per_page", "100"))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).insert_header("x-next-page", "").set_body_json(json!([
            job(5, "passed", "success", false),
            job(4, "running", "running", false),
            job(3, "waiting-for-resource", "waiting_for_resource", false),
            job(2, "queued", "pending", false),
            job(1, "created", "created", false),
        ])))
        .expect(1)
        .mount(&server)
        .await;

    let checks = client(&server).list_checks(&proyek(), "abc123").await.expect("checks");
    let states: Vec<(&str, CheckState)> = checks.iter().map(|c| (c.name.as_str(), c.state)).collect();
    assert_eq!(
        states,
        [
            ("created", CheckState::Pending),
            ("queued", CheckState::Pending),
            ("waiting-for-resource", CheckState::Pending),
            ("running", CheckState::Running),
            ("passed", CheckState::Success),
            ("failed", CheckState::Failure),
            ("allowed-to-fail", CheckState::Neutral),
            ("canceled", CheckState::Cancelled),
            ("skipped", CheckState::Skipped),
            ("manual", CheckState::Neutral),
        ]
    );
    assert_eq!(checks[0].url.as_deref(), Some("https://gitlab.com/team/sub/project/-/jobs/1"));
}

#[tokio::test]
async fn list_checks_without_pipeline_or_jobs() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("{PROJECT}/pipelines")))
        .and(query_param("sha", "no-ci"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{PROJECT}/pipelines")))
        .and(query_param("sha", "no-jobs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": 78, "status": "running", "web_url": "https://gitlab.com/team/sub/project/-/pipelines/78" },
        ])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{PROJECT}/pipelines/78/jobs")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    let client = client(&server);
    assert!(client.list_checks(&proyek(), "no-ci").await.expect("checks").is_empty());

    // A pipeline without jobs is reported as a single check with the pipeline's status.
    let checks = client.list_checks(&proyek(), "no-jobs").await.expect("checks");
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].name, "Pipeline #78");
    assert_eq!(checks[0].state, CheckState::Running);
    assert_eq!(checks[0].url.as_deref(), Some("https://gitlab.com/team/sub/project/-/pipelines/78"));
}

#[tokio::test]
async fn create_pull_request_draft_prefixes_title() {
    let server = MockServer::start().await;
    let mut created = mr_json(21, "Draft: Add export feature");
    created["draft"] = json!(true);
    Mock::given(method("POST"))
        .and(path(format!("{PROJECT}/merge_requests")))
        .and(body_json(json!({
            "source_branch": "feature",
            "target_branch": "main",
            "title": "Draft: Add export feature",
            "description": "Description **Markdown**.",
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(created))
        .expect(1)
        .mount(&server)
        .await;

    let input = NewPullRequest {
        title: "Add export feature".into(),
        body: "Description **Markdown**.".into(),
        source_branch: "feature".into(),
        target_branch: "main".into(),
        draft: true,
    };
    let pull = client(&server).create_pull_request(&proyek(), &input).await.expect("create");
    assert_eq!(pull.number, 21);
    assert!(pull.draft);
    assert_eq!(pull.title, "Draft: Add export feature");

    let requests = received(&server).await;
    assert_eq!(header(&requests[0], "content-type"), Some("application/json"));
    assert_eq!(header(&requests[0], "authorization"), Some("Bearer glpat-token_secret_456"));
}

#[tokio::test]
async fn create_pull_request_non_draft_and_already_prefixed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("{PROJECT}/merge_requests")))
        .and(body_json(json!({
            "source_branch": "feature",
            "target_branch": "main",
            "title": "Add export feature",
            "description": "",
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(mr_json(22, "Add export feature")))
        .expect(1)
        .mount(&server)
        .await;
    // A title that already has a draft prefix is not given a double prefix.
    Mock::given(method("POST"))
        .and(path(format!("{PROJECT}/merge_requests")))
        .and(body_json(json!({
            "source_branch": "feature",
            "target_branch": "main",
            "title": "[Draft] Add export feature",
            "description": "",
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(mr_json(23, "[Draft] Add export feature")))
        .expect(1)
        .mount(&server)
        .await;

    let client = client(&server);
    let mut input = NewPullRequest {
        title: "Add export feature".into(),
        body: String::new(),
        source_branch: "feature".into(),
        target_branch: "main".into(),
        draft: false,
    };
    assert_eq!(client.create_pull_request(&proyek(), &input).await.expect("create").number, 22);

    input.title = "[Draft] Add export feature".into();
    input.draft = true;
    assert_eq!(client.create_pull_request(&proyek(), &input).await.expect("create").number, 23);
}

#[tokio::test]
async fn create_pull_request_conflict_is_invalid_input() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("{PROJECT}/merge_requests")))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({
            "message": ["Another open merge request already exists for this source branch: !5"],
        })))
        .mount(&server)
        .await;

    let input = NewPullRequest {
        title: "Title".into(),
        body: String::new(),
        source_branch: "feature".into(),
        target_branch: "main".into(),
        draft: false,
    };
    let err = client(&server).create_pull_request(&proyek(), &input).await.expect_err("409");
    assert_error(&err, ForgeErrorCode::InvalidInput, TOKEN);
    assert!(err.message.contains("Another open merge request already exists"), "{}", err.message);
}

#[tokio::test]
async fn default_branch_reads_project() {
    let server = MockServer::start().await;
    let mut project = project_json("team/sub/project");
    project["default_branch"] = json!("develop");
    Mock::given(method("GET"))
        .and(path(PROJECT))
        .respond_with(ResponseTemplate::new(200).set_body_json(project))
        .expect(1)
        .mount(&server)
        .await;
    let mut empty = project_json("team/empty");
    empty["default_branch"] = Value::Null;
    Mock::given(method("GET"))
        .and(path("/projects/team%2Fempty"))
        .respond_with(ResponseTemplate::new(200).set_body_json(empty))
        .mount(&server)
        .await;

    let client = client(&server);
    assert_eq!(client.default_branch(&proyek()).await.expect("default_branch"), "develop");
    let err = client.default_branch(&repo("gitlab.com", "team", "empty")).await.expect_err("empty repo");
    assert_error(&err, ForgeErrorCode::InvalidResponse, TOKEN);
}

#[tokio::test]
async fn http_status_maps_to_error_code() {
    struct Case {
        status: u16,
        headers: &'static [(&'static str, &'static str)],
        body: Value,
        expected: ForgeErrorCode,
    }
    let cases = [
        Case {
            status: 401,
            headers: &[],
            body: json!({ "message": "401 Unauthorized" }),
            expected: ForgeErrorCode::Unauthorized,
        },
        Case {
            status: 401,
            headers: &[],
            body: json!({ "error": "invalid_token", "error_description": "Token is expired. You can either do re-authorization or token refresh." }),
            expected: ForgeErrorCode::Unauthorized,
        },
        Case {
            status: 403,
            headers: &[],
            body: json!({ "message": "403 Forbidden" }),
            expected: ForgeErrorCode::Forbidden,
        },
        Case {
            status: 403,
            headers: &[("ratelimit-remaining", "0"), ("ratelimit-reset", "4102444800")],
            body: json!({ "message": "403 Forbidden" }),
            expected: ForgeErrorCode::RateLimited,
        },
        Case {
            status: 404,
            headers: &[],
            body: json!({ "message": "404 Project Not Found" }),
            expected: ForgeErrorCode::NotFound,
        },
        Case {
            status: 429,
            headers: &[("retry-after", "60"), ("ratelimit-remaining", "0")],
            body: json!({ "message": { "error": "Too many requests" } }),
            expected: ForgeErrorCode::RateLimited,
        },
        Case { status: 400, headers: &[], body: json!({ "message": "400 Bad request" }), expected: ForgeErrorCode::InvalidInput },
        Case { status: 502, headers: &[], body: json!("GitLab is not responding"), expected: ForgeErrorCode::Network },
    ];

    for case in cases {
        let server = MockServer::start().await;
        let mut response = ResponseTemplate::new(case.status).set_body_json(&case.body);
        for (name, value) in case.headers {
            response = response.insert_header(*name, *value);
        }
        Mock::given(method("GET")).and(path(PROJECT)).respond_with(response).mount(&server).await;

        let err = client(&server).default_branch(&proyek()).await.expect_err("must fail");
        assert_eq!(err.code, case.expected, "status {} header {:?}: {}", case.status, case.headers, err.message);
        assert_error(&err, case.expected, TOKEN);
    }
}

#[tokio::test]
async fn malformed_json_and_unreachable_server() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>log in first</html>"))
        .mount(&server)
        .await;

    let err = client(&server).current_user().await.expect_err("not json");
    assert_error(&err, ForgeErrorCode::InvalidResponse, TOKEN);

    let uri = common::closed_port_uri();
    let err = provider(&ForgeHost::gitlab(), TOKEN, Some(&uri)).current_user().await.expect_err("not connected");
    assert_error(&err, ForgeErrorCode::Network, TOKEN);
}
