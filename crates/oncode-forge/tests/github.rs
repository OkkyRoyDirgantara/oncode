//! The GitHub provider against a mock HTTP server.

mod common;

use common::{assert_error, header, query_value, received, repo};
use oncode_forge::{
    provider, CheckState, ForgeErrorCode, ForgeHost, ForgeKind, ForgeProvider, NewPullRequest, PrFilter, PrState,
    RepoRef, MergeInput, MergeMethod, MergeState, NewReviewComment, ReviewEvent, ReviewInput, ReviewSide,
};
use serde_json::{json, Value};
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "gho_token_secret_123";

fn client(server: &MockServer) -> Box<dyn ForgeProvider + Send + Sync> {
    provider(&ForgeHost::github(), TOKEN, Some(&server.uri()))
}

fn hello() -> RepoRef {
    repo("github.com", "octo", "hello")
}

fn repo_json(full_name: &str) -> Value {
    json!({
        "id": 1296269,
        "full_name": full_name,
        "description": null,
        "private": false,
        "fork": false,
        "clone_url": format!("https://github.com/{full_name}.git"),
        "ssh_url": format!("git@github.com:{full_name}.git"),
        "default_branch": "main",
        "updated_at": "2011-01-26T19:14:43Z",
        "html_url": format!("https://github.com/{full_name}"),
    })
}

fn pull_json(number: u32, title: &str) -> Value {
    json!({
        "number": number,
        "title": title,
        "state": "open",
        "draft": false,
        "merged_at": null,
        "user": { "login": "octocat", "avatar_url": "https://avatars.githubusercontent.com/u/1?v=4" },
        "head": { "ref": "feature", "sha": "6dcb09b5b57875f334f61aebed695e2e4193db5e", "repo": { "full_name": "octo/hello" } },
        "base": { "ref": "main", "sha": "aaaa09b5b57875f334f61aebed695e2e4193db5e", "repo": { "full_name": "octo/hello" } },
        "updated_at": "2011-01-26T19:01:12Z",
        "html_url": format!("https://github.com/octo/hello/pull/{number}"),
    })
}

#[tokio::test]
async fn current_user_and_request_headers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "login": "octocat",
            "id": 1,
            "name": "Mona Lisa Octocat",
            "avatar_url": "https://avatars.githubusercontent.com/u/1?v=4",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let account = client(&server).current_user().await.expect("current_user");
    assert_eq!(account.id, "github:github.com:octocat");
    assert_eq!(account.kind, ForgeKind::GitHub);
    assert_eq!(account.host, "https://github.com");
    assert_eq!(account.username, "octocat");
    assert_eq!(account.display_name, "Mona Lisa Octocat");
    assert_eq!(account.avatar_url.as_deref(), Some("https://avatars.githubusercontent.com/u/1?v=4"));

    let requests = received(&server).await;
    let request = &requests[0];
    assert_eq!(header(request, "authorization"), Some("Bearer gho_token_secret_123"));
    assert_eq!(header(request, "user-agent"), Some("OnCode"));
    assert_eq!(header(request, "accept"), Some("application/vnd.github+json"));
    assert_eq!(header(request, "x-github-api-version"), Some("2022-11-28"));
}

#[tokio::test]
async fn current_user_without_name_falls_back_to_login() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "login": "octocat", "name": null, "avatar_url": "" })))
        .mount(&server)
        .await;

    let account = client(&server).current_user().await.expect("current_user");
    assert_eq!(account.display_name, "octocat");
    assert_eq!(account.avatar_url, None);
}

#[tokio::test]
async fn enterprise_host_shapes_the_account() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "login": "alice" })))
        .mount(&server)
        .await;

    let host = ForgeHost::new(ForgeKind::GitHub, "https://GHE.Example.com/");
    let account = provider(&host, TOKEN, Some(&server.uri())).current_user().await.expect("current_user");
    assert_eq!(account.id, "github:ghe.example.com:alice");
    assert_eq!(account.host, "https://GHE.Example.com");
}

#[tokio::test]
async fn list_repos_pages_through_link_header() {
    let server = MockServer::start().await;
    let mut private_fork = repo_json("octo/secret");
    private_fork["private"] = json!(true);
    private_fork["fork"] = json!(true);
    private_fork["description"] = json!("Private repository");
    Mock::given(method("GET"))
        .and(path("/user/repos"))
        .and(query_param("sort", "updated"))
        .and(query_param("direction", "desc"))
        .and(query_param("per_page", "30"))
        .and(query_param("page", "1"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header(
                    "link",
                    "<https://api.github.com/user/repos?sort=updated&direction=desc&per_page=30&page=2>; rel=\"next\", \
                     <https://api.github.com/user/repos?sort=updated&direction=desc&per_page=30&page=4>; rel=\"last\"",
                )
                .set_body_json(json!([repo_json("octo/hello"), private_fork])),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/user/repos"))
        .and(query_param("per_page", "30"))
        .and(query_param("page", "4"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("link", "<https://api.github.com/user/repos?per_page=30&page=3>; rel=\"prev\"")
                .set_body_json(json!([repo_json("octo/last")])),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = client(&server);
    // Page 0 is treated as page 1.
    let first = client.list_repos("", 0).await.expect("list_repos");
    assert_eq!(first.next_page, Some(2));
    assert_eq!(first.items.len(), 2);
    let hello = &first.items[0];
    assert_eq!(hello.full_name, "octo/hello");
    assert_eq!(hello.description, "");
    assert!(!hello.private);
    assert!(!hello.fork);
    assert_eq!(hello.clone_url_https, "https://github.com/octo/hello.git");
    assert_eq!(hello.clone_url_ssh, "git@github.com:octo/hello.git");
    assert_eq!(hello.default_branch, "main");
    assert_eq!(hello.updated_at, 1_296_069_283.0);
    assert_eq!(hello.web_url, "https://github.com/octo/hello");
    let secret = &first.items[1];
    assert!(secret.private);
    assert!(secret.fork);
    assert_eq!(secret.description, "Private repository");

    let last = client.list_repos("  ", 4).await.expect("list_repos");
    assert_eq!(last.next_page, None);
    assert_eq!(last.items[0].full_name, "octo/last");
}

#[tokio::test]
async fn list_repos_with_query_scans_and_filters() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user/repos"))
        .and(query_param("sort", "updated"))
        .and(query_param("per_page", "100"))
        .and(query_param("page", "1"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("link", "<https://api.github.com/user/repos?per_page=100&page=2>; rel=\"next\"")
                .set_body_json(json!([repo_json("octo/OnCode"), repo_json("octo/hello")])),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/user/repos"))
        .and(query_param("per_page", "100"))
        .and(query_param("page", "2"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([repo_json("team/oncode-docs"), repo_json("team/other")])),
        )
        .expect(1)
        .mount(&server)
        .await;

    let found = client(&server).list_repos(" oncode ", 1).await.expect("list_repos");
    let names: Vec<&str> = found.items.iter().map(|r| r.full_name.as_str()).collect();
    assert_eq!(names, ["octo/OnCode", "team/oncode-docs"]);
    assert_eq!(found.next_page, None);
}

#[tokio::test]
async fn list_repos_with_query_stops_after_scan_budget() {
    let server = MockServer::start().await;
    // Every page points to the next page and never matches.
    for page in 1..=6u32 {
        Mock::given(method("GET"))
            .and(path("/user/repos"))
            .and(query_param("per_page", "100"))
            .and(query_param("page", page.to_string()))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header(
                        "link",
                        format!("<https://api.github.com/user/repos?per_page=100&page={}>; rel=\"next\"", page + 1),
                    )
                    .set_body_json(json!([repo_json("octo/hello")])),
            )
            .mount(&server)
            .await;
    }

    let found = client(&server).list_repos("does-not-exist", 1).await.expect("list_repos");
    assert!(found.items.is_empty());
    // Five pages are scanned; the caller continues from page 6.
    assert_eq!(found.next_page, Some(6));
    assert_eq!(received(&server).await.len(), 5);
}

#[tokio::test]
async fn list_pull_requests_open_and_all() {
    let server = MockServer::start().await;
    let mut from_fork = pull_json(8, "From fork");
    from_fork["head"]["repo"] = json!({ "full_name": "contributor/hello" });
    from_fork["draft"] = json!(true);
    let mut deleted_fork = pull_json(6, "Deleted fork");
    deleted_fork["head"]["repo"] = Value::Null;
    deleted_fork["user"] = Value::Null;
    Mock::given(method("GET"))
        .and(path("/repos/octo/hello/pulls"))
        .and(query_param("state", "open"))
        .and(query_param("sort", "updated"))
        .and(query_param("direction", "desc"))
        .and(query_param("per_page", "30"))
        .and(query_param("page", "1"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("link", "<https://api.github.com/repositories/1/pulls?state=open&page=2>; rel=\"next\"")
                .set_body_json(json!([pull_json(9, "Add feature"), from_fork, deleted_fork])),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mut closed = pull_json(3, "Closed");
    closed["state"] = json!("closed");
    let mut merged = pull_json(2, "Already merged");
    merged["state"] = json!("closed");
    merged["merged_at"] = json!("2011-01-26T19:01:12Z");
    Mock::given(method("GET"))
        .and(path("/repos/octo/hello/pulls"))
        .and(query_param("state", "all"))
        .and(query_param("sort", "updated"))
        .and(query_param("direction", "desc"))
        .and(query_param("per_page", "30"))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([closed, merged])))
        .expect(1)
        .mount(&server)
        .await;

    let client = client(&server);
    let open = client.list_pull_requests(&hello(), PrFilter::Open, 1).await.expect("open");
    assert_eq!(open.next_page, Some(2));
    assert_eq!(open.items.len(), 3);
    let first = &open.items[0];
    assert_eq!(first.number, 9);
    assert_eq!(first.title, "Add feature");
    assert_eq!(first.state, PrState::Open);
    assert!(!first.draft);
    assert_eq!(first.author, "octocat");
    assert_eq!(first.author_avatar.as_deref(), Some("https://avatars.githubusercontent.com/u/1?v=4"));
    assert_eq!(first.source_branch, "feature");
    assert_eq!(first.target_branch, "main");
    assert_eq!(first.head_sha, "6dcb09b5b57875f334f61aebed695e2e4193db5e");
    assert_eq!(first.updated_at, 1_296_068_472.0);
    assert_eq!(first.web_url, "https://github.com/octo/hello/pull/9");
    assert!(!first.is_cross_repo);
    assert!(open.items[1].is_cross_repo);
    assert!(open.items[1].draft);
    assert!(open.items[2].is_cross_repo);
    assert_eq!(open.items[2].author, "ghost");

    let all = client.list_pull_requests(&hello(), PrFilter::All, 2).await.expect("all");
    assert_eq!(all.next_page, None);
    assert_eq!(all.items[0].state, PrState::Closed);
    assert_eq!(all.items[1].state, PrState::Merged);
}

#[tokio::test]
async fn list_pull_requests_mine_and_review_requested_use_search() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/issues"))
        .and(query_param("q", "repo:octo/hello is:pr is:open author:@me"))
        .and(query_param("sort", "updated"))
        .and(query_param("order", "desc"))
        .and(query_param("per_page", "30"))
        .and(query_param("page", "1"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("link", "<https://api.github.com/search/issues?q=x&page=2>; rel=\"next\"")
                .set_body_json(json!({
                    "total_count": 31,
                    "incomplete_results": false,
                    "items": [{ "number": 7, "title": "Mine" }, { "number": 5, "title": "Also mine" }],
                })),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/search/issues"))
        .and(query_param("q", "repo:octo/hello is:pr is:open review-requested:@me"))
        .and(query_param("sort", "updated"))
        .and(query_param("order", "desc"))
        .and(query_param("per_page", "30"))
        .and(query_param("page", "3"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "total_count": 1, "items": [{ "number": 5 }] })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/octo/hello/pulls/7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull_json(7, "Mine")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/octo/hello/pulls/5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull_json(5, "Also mine")))
        .expect(2)
        .mount(&server)
        .await;

    let client = client(&server);
    let mine = client.list_pull_requests(&hello(), PrFilter::Mine, 1).await.expect("mine");
    assert_eq!(mine.next_page, Some(2));
    // The order of the search results is preserved, and the branch is filled from the PR detail.
    assert_eq!(mine.items.iter().map(|p| p.number).collect::<Vec<_>>(), [7, 5]);
    assert_eq!(mine.items[0].source_branch, "feature");
    assert_eq!(mine.items[0].head_sha, "6dcb09b5b57875f334f61aebed695e2e4193db5e");

    let review = client.list_pull_requests(&hello(), PrFilter::ReviewRequested, 3).await.expect("review");
    assert_eq!(review.next_page, None);
    assert_eq!(review.items.iter().map(|p| p.number).collect::<Vec<_>>(), [5]);

    // The search query is sent encoded as a single `q` parameter.
    let requests = received(&server).await;
    let search = requests.iter().find(|r| r.url.path() == "/search/issues").expect("search request");
    assert_eq!(query_value(search, "q").as_deref(), Some("repo:octo/hello is:pr is:open author:@me"));
}

#[tokio::test]
async fn get_pull_request_merged_with_reviewers() {
    let server = MockServer::start().await;
    let mut pull = pull_json(12, "Fix login");
    pull["state"] = json!("closed");
    pull["merged"] = json!(true);
    pull["merged_at"] = json!("2011-01-26T19:01:12Z");
    pull["body"] = json!("## Summary\n\nFixes *login*.");
    pull["requested_reviewers"] = json!([{ "login": "carol" }]);
    pull["requested_teams"] = json!([{ "name": "Core Team", "slug": "core-team" }]);
    pull["labels"] = json!([{ "name": "bug" }, { "name": "priority" }]);
    pull["mergeable"] = Value::Null;
    pull["commits"] = json!(3);
    pull["changed_files"] = json!(5);
    pull["additions"] = json!(100);
    pull["deletions"] = json!(42);
    Mock::given(method("GET"))
        .and(path("/repos/octo/hello/pulls/12"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/octo/hello/pulls/12/reviews"))
        .and(query_param("per_page", "100"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "user": { "login": "dave" }, "state": "APPROVED" },
            // The author's own replies and duplicate reviews do not add reviewers.
            { "user": { "login": "octocat" }, "state": "COMMENTED" },
            { "user": { "login": "dave" }, "state": "COMMENTED" },
            { "user": null, "state": "COMMENTED" },
        ])))
        .expect(1)
        .mount(&server)
        .await;

    let detail = client(&server).get_pull_request(&hello(), 12).await.expect("detail");
    assert_eq!(detail.pull_request.number, 12);
    assert_eq!(detail.pull_request.state, PrState::Merged);
    assert!(!detail.pull_request.draft);
    assert_eq!(detail.body, "## Summary\n\nFixes *login*.");
    assert_eq!(detail.reviewers, ["carol", "Core Team", "dave"]);
    assert_eq!(detail.labels, ["bug", "priority"]);
    assert_eq!(detail.mergeable, None);
    assert_eq!(detail.commits, 3);
    assert_eq!(detail.changed_files, 5);
    assert_eq!(detail.additions, 100);
    assert_eq!(detail.deletions, 42);
}

#[tokio::test]
async fn get_pull_request_draft() {
    let server = MockServer::start().await;
    let mut pull = pull_json(13, "Work in progress");
    pull["draft"] = json!(true);
    pull["body"] = Value::Null;
    pull["mergeable"] = json!(false);
    Mock::given(method("GET"))
        .and(path("/repos/octo/hello/pulls/13"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/octo/hello/pulls/13/reviews"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    let detail = client(&server).get_pull_request(&hello(), 13).await.expect("detail");
    assert_eq!(detail.pull_request.state, PrState::Open);
    assert!(detail.pull_request.draft);
    assert_eq!(detail.body, "");
    assert!(detail.reviewers.is_empty());
    assert!(detail.labels.is_empty());
    assert_eq!(detail.mergeable, Some(false));
    assert_eq!(detail.commits, 0);
}

#[tokio::test]
async fn list_checks_merges_check_runs_and_statuses() {
    let server = MockServer::start().await;
    let run = |name: &str, status: &str, conclusion: Value| {
        json!({
            "name": name,
            "status": status,
            "conclusion": conclusion,
            "html_url": format!("https://github.com/octo/hello/runs/{name}"),
            "details_url": "https://ci.example.com/detail",
        })
    };
    Mock::given(method("GET"))
        .and(path("/repos/octo/hello/commits/abc123/check-runs"))
        .and(query_param("per_page", "100"))
        .and(query_param("page", "1"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header(
                    "link",
                    "<https://api.github.com/repos/octo/hello/commits/abc123/check-runs?per_page=100&page=2>; rel=\"next\"",
                )
                .set_body_json(json!({
                    "total_count": 9,
                    "check_runs": [
                        run("queued", "queued", Value::Null),
                        run("running", "in_progress", Value::Null),
                        run("passed", "completed", json!("success")),
                        run("failed", "completed", json!("failure")),
                        run("timed-out", "completed", json!("timed_out")),
                    ],
                })),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mut without_html_url = run("neutral", "completed", json!("neutral"));
    without_html_url["html_url"] = Value::Null;
    Mock::given(method("GET"))
        .and(path("/repos/octo/hello/commits/abc123/check-runs"))
        .and(query_param("per_page", "100"))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "total_count": 9,
            "check_runs": [
                run("canceled", "completed", json!("cancelled")),
                run("skipped", "completed", json!("skipped")),
                without_html_url,
            ],
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/octo/hello/commits/abc123/status"))
        .and(query_param("per_page", "100"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "state": "failure",
            "statuses": [
                { "context": "ci/jenkins", "state": "success", "target_url": "https://jenkins.example.com/1" },
                { "context": "ci/lint", "state": "failure", "target_url": null },
                { "context": "ci/deploy", "state": "error", "target_url": "" },
                { "context": "ci/waiting", "state": "pending", "target_url": null },
            ],
        })))
        .expect(1)
        .mount(&server)
        .await;

    let checks = client(&server).list_checks(&hello(), "abc123").await.expect("checks");
    let states: Vec<(&str, CheckState)> = checks.iter().map(|c| (c.name.as_str(), c.state)).collect();
    assert_eq!(
        states,
        [
            ("queued", CheckState::Pending),
            ("running", CheckState::Running),
            ("passed", CheckState::Success),
            ("failed", CheckState::Failure),
            ("timed-out", CheckState::Failure),
            ("canceled", CheckState::Cancelled),
            ("skipped", CheckState::Skipped),
            ("neutral", CheckState::Neutral),
            ("ci/jenkins", CheckState::Success),
            ("ci/lint", CheckState::Failure),
            ("ci/deploy", CheckState::Failure),
            ("ci/waiting", CheckState::Pending),
        ]
    );
    assert_eq!(checks[0].url.as_deref(), Some("https://github.com/octo/hello/runs/queued"));
    assert_eq!(checks[7].url.as_deref(), Some("https://ci.example.com/detail"));
    assert_eq!(checks[8].url.as_deref(), Some("https://jenkins.example.com/1"));
    assert_eq!(checks[9].url, None);
    assert_eq!(checks[10].url, None);
}

#[tokio::test]
async fn list_checks_empty_when_commit_has_no_ci() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/octo/hello/commits/abc123/check-runs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "total_count": 0, "check_runs": [] })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/octo/hello/commits/abc123/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "state": "pending", "statuses": [] })))
        .mount(&server)
        .await;

    let client = client(&server);
    assert!(client.list_checks(&hello(), "abc123").await.expect("checks").is_empty());
    let err = client.list_checks(&hello(), "  ").await.expect_err("empty sha");
    assert_error(&err, ForgeErrorCode::InvalidInput, TOKEN);
}

#[tokio::test]
async fn create_pull_request_sends_expected_body() {
    let server = MockServer::start().await;
    let mut created = pull_json(21, "Add export feature");
    created["draft"] = json!(true);
    Mock::given(method("POST"))
        .and(path("/repos/octo/hello/pulls"))
        .and(body_json(json!({
            "title": "Add export feature",
            "head": "feature",
            "base": "main",
            "body": "Description **Markdown**.",
            "draft": true,
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(created))
        .expect(1)
        .mount(&server)
        .await;

    let input = NewPullRequest {
        title: " Add export feature ".into(),
        body: "Description **Markdown**.".into(),
        source_branch: "feature".into(),
        target_branch: "main".into(),
        draft: true,
    };
    let pull = client(&server).create_pull_request(&hello(), &input).await.expect("create");
    assert_eq!(pull.number, 21);
    assert!(pull.draft);
    assert_eq!(pull.web_url, "https://github.com/octo/hello/pull/21");

    let requests = received(&server).await;
    assert_eq!(header(&requests[0], "content-type"), Some("application/json"));
    assert_eq!(header(&requests[0], "authorization"), Some("Bearer gho_token_secret_123"));
}

#[tokio::test]
async fn create_pull_request_rejects_invalid_input_and_maps_422() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/octo/hello/pulls"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "message": "Validation Failed",
            "errors": [{ "resource": "PullRequest", "code": "custom", "message": "A pull request already exists for octo:feature." }],
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client(&server);
    let mut input = NewPullRequest {
        title: String::new(),
        body: String::new(),
        source_branch: "feature".into(),
        target_branch: "main".into(),
        draft: false,
    };
    // An empty title is rejected before any request is sent.
    let err = client.create_pull_request(&hello(), &input).await.expect_err("empty title");
    assert_error(&err, ForgeErrorCode::InvalidInput, TOKEN);
    assert!(received(&server).await.is_empty());

    input.title = "Title".into();
    let err = client.create_pull_request(&hello(), &input).await.expect_err("422");
    assert_error(&err, ForgeErrorCode::InvalidInput, TOKEN);
    assert!(err.message.contains("A pull request already exists for octo:feature."), "{}", err.message);
}

#[tokio::test]
async fn default_branch_reads_repo() {
    let server = MockServer::start().await;
    let mut repo_body = repo_json("octo/hello");
    repo_body["default_branch"] = json!("trunk");
    Mock::given(method("GET"))
        .and(path("/repos/octo/hello"))
        .respond_with(ResponseTemplate::new(200).set_body_json(repo_body))
        .expect(1)
        .mount(&server)
        .await;

    assert_eq!(client(&server).default_branch(&hello()).await.expect("default_branch"), "trunk");
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
            body: json!({ "message": "Bad credentials" }),
            expected: ForgeErrorCode::Unauthorized,
        },
        Case {
            status: 403,
            headers: &[("x-ratelimit-remaining", "4999")],
            body: json!({ "message": "Resource not accessible by integration" }),
            expected: ForgeErrorCode::Forbidden,
        },
        Case {
            status: 403,
            headers: &[("x-ratelimit-remaining", "0"), ("x-ratelimit-reset", "4102444800")],
            body: json!({ "message": "API rate limit exceeded for user ID 1." }),
            expected: ForgeErrorCode::RateLimited,
        },
        Case {
            status: 403,
            headers: &[("retry-after", "60")],
            body: json!({ "message": "You have exceeded a secondary rate limit." }),
            expected: ForgeErrorCode::RateLimited,
        },
        Case { status: 404, headers: &[], body: json!({ "message": "Not Found" }), expected: ForgeErrorCode::NotFound },
        Case {
            status: 429,
            headers: &[("retry-after", "30")],
            body: json!({ "message": "Too Many Requests" }),
            expected: ForgeErrorCode::RateLimited,
        },
        Case { status: 405, headers: &[], body: json!({ "message": "Pull Request is not mergeable" }), expected: ForgeErrorCode::InvalidInput },
        Case { status: 422, headers: &[], body: json!({ "message": "Validation Failed" }), expected: ForgeErrorCode::InvalidInput },
        Case { status: 500, headers: &[], body: json!({ "message": "Server Error" }), expected: ForgeErrorCode::Network },
        Case { status: 503, headers: &[], body: json!("unavailable"), expected: ForgeErrorCode::Network },
    ];

    for case in cases {
        let server = MockServer::start().await;
        let mut response = ResponseTemplate::new(case.status).set_body_json(&case.body);
        for (name, value) in case.headers {
            response = response.insert_header(*name, *value);
        }
        Mock::given(method("GET")).and(path("/user")).respond_with(response).mount(&server).await;

        let err = client(&server).current_user().await.expect_err("must fail");
        assert_eq!(err.code, case.expected, "status {} header {:?}: {}", case.status, case.headers, err.message);
        assert_error(&err, case.expected, TOKEN);
    }
}

#[tokio::test]
async fn rate_limit_message_mentions_wait_time() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "30").set_body_json(json!({})))
        .mount(&server)
        .await;

    let err = client(&server).current_user().await.expect_err("429");
    assert_eq!(err.code, ForgeErrorCode::RateLimited);
    assert!(err.message.contains("30 seconds"), "{}", err.message);
}

#[tokio::test]
async fn error_message_never_echoes_the_token() {
    let server = MockServer::start().await;
    // A server that (mistakenly) echoes the credential in its error message.
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(
            ResponseTemplate::new(401).set_body_json(json!({ "message": format!("Bad credentials: Bearer {TOKEN}") })),
        )
        .mount(&server)
        .await;

    let err = client(&server).current_user().await.expect_err("401");
    assert_error(&err, ForgeErrorCode::Unauthorized, TOKEN);
}

#[tokio::test]
async fn malformed_json_is_invalid_response() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>not json</html>"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/octo/hello"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "name": "other shape" })))
        .mount(&server)
        .await;

    let client = client(&server);
    let err = client.current_user().await.expect_err("not json");
    assert_error(&err, ForgeErrorCode::InvalidResponse, TOKEN);
    let err = client.default_branch(&hello()).await.expect_err("wrong shape");
    assert_error(&err, ForgeErrorCode::InvalidResponse, TOKEN);
}

#[tokio::test]
async fn unreachable_server_is_network_error() {
    let uri = common::closed_port_uri();
    let err = provider(&ForgeHost::github(), TOKEN, Some(&uri)).current_user().await.expect_err("not connected");
    assert_error(&err, ForgeErrorCode::Network, TOKEN);
}

#[tokio::test]
async fn empty_repo_ref_is_rejected_before_any_request() {
    let server = MockServer::start().await;
    let client = client(&server);
    let broken = repo("github.com", "", "hello");

    let err = client.default_branch(&broken).await.expect_err("empty owner");
    assert_error(&err, ForgeErrorCode::InvalidInput, TOKEN);
    let err = client.list_pull_requests(&broken, PrFilter::Mine, 1).await.expect_err("empty owner");
    assert_error(&err, ForgeErrorCode::InvalidInput, TOKEN);
    assert!(received(&server).await.is_empty());
}

/// Merge: the chosen method and the head commit seen are sent; the source branch is deleted afterwards when it
/// is in the same repository. A PR that cannot be merged (405) is reported clearly.
#[tokio::test]
async fn merge_pull_request_sends_the_method_and_deletes_the_branch() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/repos/octo/hello/pulls/7")).respond_with(ResponseTemplate::new(200).set_body_json(pull_json(7, "Feature"))).mount(&server).await;
    Mock::given(method("PUT"))
        .and(path("/repos/octo/hello/pulls/7/merge"))
        .and(body_json(json!({ "merge_method": "squash", "sha": "6dcb09b5b57875f334f61aebed695e2e4193db5e", "commit_title": "Feature (#7)" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "merged": true, "sha": "abc" })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE")).and(path("/repos/octo/hello/git/refs/heads/feature")).respond_with(ResponseTemplate::new(204)).expect(1).mount(&server).await;

    let input = MergeInput { method: MergeMethod::Squash, sha: Some("6dcb09b5b57875f334f61aebed695e2e4193db5e".into()), title: Some("Feature (#7)".into()), message: None, delete_branch: true };
    let outcome = client(&server).merge_pull_request(&hello(), 7, &input).await.expect("merged");
    assert_eq!(outcome.branch_kept, None);

    // Merged, but the branch cannot be deleted (no permission): still a merge, with the reason.
    let kept = MockServer::start().await;
    Mock::given(method("GET")).and(path("/repos/octo/hello/pulls/7")).respond_with(ResponseTemplate::new(200).set_body_json(pull_json(7, "Feature"))).mount(&kept).await;
    Mock::given(method("PUT")).and(path("/repos/octo/hello/pulls/7/merge")).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "merged": true, "sha": "abc" }))).expect(1).mount(&kept).await;
    Mock::given(method("DELETE"))
        .and(path("/repos/octo/hello/git/refs/heads/feature"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({ "message": "Resource not accessible by integration" })))
        .mount(&kept)
        .await;
    let outcome = client(&kept).merge_pull_request(&hello(), 7, &input).await.expect("merged although the branch stays");
    assert!(outcome.branch_kept.is_some_and(|reason| !reason.is_empty()));

    let refused = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/repos/octo/hello/pulls/8/merge"))
        .respond_with(ResponseTemplate::new(405).set_body_json(json!({ "message": "Pull Request is not mergeable" })))
        .mount(&refused)
        .await;
    let input = MergeInput { method: MergeMethod::Merge, sha: None, title: None, message: None, delete_branch: false };
    let err = client(&refused).merge_pull_request(&hello(), 8, &input).await.expect_err("405");
    assert_error(&err, ForgeErrorCode::InvalidInput, TOKEN);
    assert!(err.message.contains("not mergeable"), "{}", err.message);
}

/// The detail carries GitHub's merge state and the merge methods the repository allows.
#[tokio::test]
async fn pull_request_detail_has_merge_state_and_methods() {
    let server = MockServer::start().await;
    let mut pull = pull_json(9, "Feature");
    pull["mergeable"] = json!(true);
    pull["mergeable_state"] = json!("unstable");
    Mock::given(method("GET")).and(path("/repos/octo/hello/pulls/9")).respond_with(ResponseTemplate::new(200).set_body_json(pull)).mount(&server).await;
    Mock::given(method("GET")).and(path("/repos/octo/hello/pulls/9/reviews")).respond_with(ResponseTemplate::new(200).set_body_json(json!([]))).mount(&server).await;
    let mut settings = repo_json("octo/hello");
    settings["allow_merge_commit"] = json!(false);
    settings["allow_squash_merge"] = json!(true);
    settings["allow_rebase_merge"] = json!(true);
    Mock::given(method("GET")).and(path("/repos/octo/hello")).respond_with(ResponseTemplate::new(200).set_body_json(settings)).mount(&server).await;
    let detail = client(&server).get_pull_request(&hello(), 9).await.expect("detail");
    assert_eq!(detail.merge_state, MergeState::Unstable);
    assert_eq!(detail.merge_methods, [MergeMethod::Squash, MergeMethod::Rebase]);
}

/// Review comments: read with their conversation, then a review with a line comment and a reply are sent.
#[tokio::test]
async fn review_comments_are_read_submitted_and_replied_to() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/octo/hello/pulls/5/comments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": 10, "path": "src/a.rs", "line": 4, "side": "RIGHT", "body": "Why?", "user": { "login": "carol" }, "created_at": "2026-01-01T00:00:00Z" },
            { "id": 11, "path": "src/a.rs", "line": 4, "side": "RIGHT", "body": "Because.", "user": { "login": "octocat" }, "in_reply_to_id": 10 },
            { "id": 12, "path": "src/b.rs", "line": null, "side": "LEFT", "body": "Old", "user": { "login": "carol" } },
        ])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/octo/hello/pulls/5/reviews"))
        .and(body_json(json!({ "commit_id": "abc", "body": "Looks good", "event": "APPROVE", "comments": [{ "path": "src/a.rs", "line": 7, "side": "RIGHT", "body": "nit" }] })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 1 })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/octo/hello/pulls/5/comments/10/replies"))
        .and(body_json(json!({ "body": "Thanks" })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": 13 })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client(&server);
    let comments = client.list_review_comments(&hello(), 5).await.expect("comments");
    let shown: Vec<(&str, &str, Option<u32>, bool)> = comments.iter().map(|c| (c.id.as_str(), c.thread.as_str(), c.line, c.is_reply)).collect();
    assert_eq!(shown, [("10", "10", Some(4), false), ("11", "10", Some(4), true), ("12", "12", None, false)]);
    assert_eq!(comments[2].side, ReviewSide::Left);

    let input = ReviewInput {
        event: ReviewEvent::Approve,
        body: "Looks good".into(),
        comments: vec![NewReviewComment { path: "src/a.rs".into(), line: 7, side: ReviewSide::Right, body: "nit".into() }],
        commit_id: "abc".into(),
    };
    client.submit_review(&hello(), 5, &input).await.expect("review");
    client.reply_to_review_comment(&hello(), 5, "10", "Thanks").await.expect("reply");
}
