//! Penyedia GitHub terhadap server HTTP tiruan.

mod common;

use common::{assert_error, header, query_value, received, repo};
use oncode_forge::{
    provider, CheckState, ForgeErrorCode, ForgeHost, ForgeKind, ForgeProvider, NewPullRequest, PrFilter, PrState,
    RepoRef,
};
use serde_json::{json, Value};
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "gho_token_rahasia_123";

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
        "head": { "ref": "fitur", "sha": "6dcb09b5b57875f334f61aebed695e2e4193db5e", "repo": { "full_name": "octo/hello" } },
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
    assert_eq!(header(request, "authorization"), Some("Bearer gho_token_rahasia_123"));
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
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "login": "budi" })))
        .mount(&server)
        .await;

    let host = ForgeHost::new(ForgeKind::GitHub, "https://GHE.Perusahaan.id/");
    let account = provider(&host, TOKEN, Some(&server.uri())).current_user().await.expect("current_user");
    assert_eq!(account.id, "github:ghe.perusahaan.id:budi");
    assert_eq!(account.host, "https://GHE.Perusahaan.id");
}

#[tokio::test]
async fn list_repos_pages_through_link_header() {
    let server = MockServer::start().await;
    let mut private_fork = repo_json("octo/rahasia");
    private_fork["private"] = json!(true);
    private_fork["fork"] = json!(true);
    private_fork["description"] = json!("Repositori privat");
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
                .set_body_json(json!([repo_json("octo/terakhir")])),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = client(&server);
    // Halaman 0 diperlakukan sebagai halaman 1.
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
    assert_eq!(secret.description, "Repositori privat");

    let last = client.list_repos("  ", 4).await.expect("list_repos");
    assert_eq!(last.next_page, None);
    assert_eq!(last.items[0].full_name, "octo/terakhir");
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
            ResponseTemplate::new(200).set_body_json(json!([repo_json("tim/oncode-docs"), repo_json("tim/lain")])),
        )
        .expect(1)
        .mount(&server)
        .await;

    let found = client(&server).list_repos(" oncode ", 1).await.expect("list_repos");
    let names: Vec<&str> = found.items.iter().map(|r| r.full_name.as_str()).collect();
    assert_eq!(names, ["octo/OnCode", "tim/oncode-docs"]);
    assert_eq!(found.next_page, None);
}

#[tokio::test]
async fn list_repos_with_query_stops_after_scan_budget() {
    let server = MockServer::start().await;
    // Setiap halaman menunjuk ke halaman berikutnya dan tidak pernah cocok.
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

    let found = client(&server).list_repos("tidak-ada", 1).await.expect("list_repos");
    assert!(found.items.is_empty());
    // Lima halaman dipindai; pemanggil melanjutkan dari halaman 6.
    assert_eq!(found.next_page, Some(6));
    assert_eq!(received(&server).await.len(), 5);
}

#[tokio::test]
async fn list_pull_requests_open_and_all() {
    let server = MockServer::start().await;
    let mut from_fork = pull_json(8, "Dari fork");
    from_fork["head"]["repo"] = json!({ "full_name": "kontributor/hello" });
    from_fork["draft"] = json!(true);
    let mut deleted_fork = pull_json(6, "Fork terhapus");
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
                .set_body_json(json!([pull_json(9, "Tambah fitur"), from_fork, deleted_fork])),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mut closed = pull_json(3, "Ditutup");
    closed["state"] = json!("closed");
    let mut merged = pull_json(2, "Sudah masuk");
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
    assert_eq!(first.title, "Tambah fitur");
    assert_eq!(first.state, PrState::Open);
    assert!(!first.draft);
    assert_eq!(first.author, "octocat");
    assert_eq!(first.author_avatar.as_deref(), Some("https://avatars.githubusercontent.com/u/1?v=4"));
    assert_eq!(first.source_branch, "fitur");
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
                    "items": [{ "number": 7, "title": "Punya saya" }, { "number": 5, "title": "Juga punya saya" }],
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
        .respond_with(ResponseTemplate::new(200).set_body_json(pull_json(7, "Punya saya")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/octo/hello/pulls/5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull_json(5, "Juga punya saya")))
        .expect(2)
        .mount(&server)
        .await;

    let client = client(&server);
    let mine = client.list_pull_requests(&hello(), PrFilter::Mine, 1).await.expect("mine");
    assert_eq!(mine.next_page, Some(2));
    // Urutan hasil pencarian dipertahankan, dan branch terisi dari detail PR.
    assert_eq!(mine.items.iter().map(|p| p.number).collect::<Vec<_>>(), [7, 5]);
    assert_eq!(mine.items[0].source_branch, "fitur");
    assert_eq!(mine.items[0].head_sha, "6dcb09b5b57875f334f61aebed695e2e4193db5e");

    let review = client.list_pull_requests(&hello(), PrFilter::ReviewRequested, 3).await.expect("review");
    assert_eq!(review.next_page, None);
    assert_eq!(review.items.iter().map(|p| p.number).collect::<Vec<_>>(), [5]);

    // Query pencarian dikirim ter-encode sebagai satu parameter `q`.
    let requests = received(&server).await;
    let search = requests.iter().find(|r| r.url.path() == "/search/issues").expect("permintaan pencarian");
    assert_eq!(query_value(search, "q").as_deref(), Some("repo:octo/hello is:pr is:open author:@me"));
}

#[tokio::test]
async fn get_pull_request_merged_with_reviewers() {
    let server = MockServer::start().await;
    let mut pull = pull_json(12, "Perbaiki login");
    pull["state"] = json!("closed");
    pull["merged"] = json!(true);
    pull["merged_at"] = json!("2011-01-26T19:01:12Z");
    pull["body"] = json!("## Ringkasan\n\nMemperbaiki *login*.");
    pull["requested_reviewers"] = json!([{ "login": "siti" }]);
    pull["requested_teams"] = json!([{ "name": "Tim Inti", "slug": "tim-inti" }]);
    pull["labels"] = json!([{ "name": "bug" }, { "name": "prioritas" }]);
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
            { "user": { "login": "agus" }, "state": "APPROVED" },
            // Balasan penulis sendiri dan review ganda tidak menambah reviewer.
            { "user": { "login": "octocat" }, "state": "COMMENTED" },
            { "user": { "login": "agus" }, "state": "COMMENTED" },
            { "user": null, "state": "COMMENTED" },
        ])))
        .expect(1)
        .mount(&server)
        .await;

    let detail = client(&server).get_pull_request(&hello(), 12).await.expect("detail");
    assert_eq!(detail.pull_request.number, 12);
    assert_eq!(detail.pull_request.state, PrState::Merged);
    assert!(!detail.pull_request.draft);
    assert_eq!(detail.body, "## Ringkasan\n\nMemperbaiki *login*.");
    assert_eq!(detail.reviewers, ["siti", "Tim Inti", "agus"]);
    assert_eq!(detail.labels, ["bug", "prioritas"]);
    assert_eq!(detail.mergeable, None);
    assert_eq!(detail.commits, 3);
    assert_eq!(detail.changed_files, 5);
    assert_eq!(detail.additions, 100);
    assert_eq!(detail.deletions, 42);
}

#[tokio::test]
async fn get_pull_request_draft() {
    let server = MockServer::start().await;
    let mut pull = pull_json(13, "Masih dikerjakan");
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
                        run("antre", "queued", Value::Null),
                        run("jalan", "in_progress", Value::Null),
                        run("lulus", "completed", json!("success")),
                        run("gagal", "completed", json!("failure")),
                        run("habis-waktu", "completed", json!("timed_out")),
                    ],
                })),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mut without_html_url = run("netral", "completed", json!("neutral"));
    without_html_url["html_url"] = Value::Null;
    Mock::given(method("GET"))
        .and(path("/repos/octo/hello/commits/abc123/check-runs"))
        .and(query_param("per_page", "100"))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "total_count": 9,
            "check_runs": [
                run("batal", "completed", json!("cancelled")),
                run("lewati", "completed", json!("skipped")),
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
                { "context": "ci/tunggu", "state": "pending", "target_url": null },
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
            ("antre", CheckState::Pending),
            ("jalan", CheckState::Running),
            ("lulus", CheckState::Success),
            ("gagal", CheckState::Failure),
            ("habis-waktu", CheckState::Failure),
            ("batal", CheckState::Cancelled),
            ("lewati", CheckState::Skipped),
            ("netral", CheckState::Neutral),
            ("ci/jenkins", CheckState::Success),
            ("ci/lint", CheckState::Failure),
            ("ci/deploy", CheckState::Failure),
            ("ci/tunggu", CheckState::Pending),
        ]
    );
    assert_eq!(checks[0].url.as_deref(), Some("https://github.com/octo/hello/runs/antre"));
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
    let err = client.list_checks(&hello(), "  ").await.expect_err("sha kosong");
    assert_error(&err, ForgeErrorCode::InvalidInput, TOKEN);
}

#[tokio::test]
async fn create_pull_request_sends_expected_body() {
    let server = MockServer::start().await;
    let mut created = pull_json(21, "Tambah fitur ekspor");
    created["draft"] = json!(true);
    Mock::given(method("POST"))
        .and(path("/repos/octo/hello/pulls"))
        .and(body_json(json!({
            "title": "Tambah fitur ekspor",
            "head": "fitur",
            "base": "main",
            "body": "Deskripsi **Markdown**.",
            "draft": true,
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(created))
        .expect(1)
        .mount(&server)
        .await;

    let input = NewPullRequest {
        title: " Tambah fitur ekspor ".into(),
        body: "Deskripsi **Markdown**.".into(),
        source_branch: "fitur".into(),
        target_branch: "main".into(),
        draft: true,
    };
    let pull = client(&server).create_pull_request(&hello(), &input).await.expect("create");
    assert_eq!(pull.number, 21);
    assert!(pull.draft);
    assert_eq!(pull.web_url, "https://github.com/octo/hello/pull/21");

    let requests = received(&server).await;
    assert_eq!(header(&requests[0], "content-type"), Some("application/json"));
    assert_eq!(header(&requests[0], "authorization"), Some("Bearer gho_token_rahasia_123"));
}

#[tokio::test]
async fn create_pull_request_rejects_invalid_input_and_maps_422() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/octo/hello/pulls"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "message": "Validation Failed",
            "errors": [{ "resource": "PullRequest", "code": "custom", "message": "A pull request already exists for octo:fitur." }],
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client(&server);
    let mut input = NewPullRequest {
        title: String::new(),
        body: String::new(),
        source_branch: "fitur".into(),
        target_branch: "main".into(),
        draft: false,
    };
    // Judul kosong ditolak sebelum ada permintaan yang dikirim.
    let err = client.create_pull_request(&hello(), &input).await.expect_err("judul kosong");
    assert_error(&err, ForgeErrorCode::InvalidInput, TOKEN);
    assert!(received(&server).await.is_empty());

    input.title = "Judul".into();
    let err = client.create_pull_request(&hello(), &input).await.expect_err("422");
    assert_error(&err, ForgeErrorCode::InvalidInput, TOKEN);
    assert!(err.message.contains("A pull request already exists for octo:fitur."), "{}", err.message);
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
        Case { status: 422, headers: &[], body: json!({ "message": "Validation Failed" }), expected: ForgeErrorCode::InvalidInput },
        Case { status: 500, headers: &[], body: json!({ "message": "Server Error" }), expected: ForgeErrorCode::Network },
        Case { status: 503, headers: &[], body: json!("tidak tersedia"), expected: ForgeErrorCode::Network },
    ];

    for case in cases {
        let server = MockServer::start().await;
        let mut response = ResponseTemplate::new(case.status).set_body_json(&case.body);
        for (name, value) in case.headers {
            response = response.insert_header(*name, *value);
        }
        Mock::given(method("GET")).and(path("/user")).respond_with(response).mount(&server).await;

        let err = client(&server).current_user().await.expect_err("harus gagal");
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
    // Server yang (secara keliru) menggemakan kredensial di pesan galatnya.
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
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>bukan json</html>"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/octo/hello"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "nama": "bentuk lain" })))
        .mount(&server)
        .await;

    let client = client(&server);
    let err = client.current_user().await.expect_err("bukan json");
    assert_error(&err, ForgeErrorCode::InvalidResponse, TOKEN);
    let err = client.default_branch(&hello()).await.expect_err("bentuk salah");
    assert_error(&err, ForgeErrorCode::InvalidResponse, TOKEN);
}

#[tokio::test]
async fn unreachable_server_is_network_error() {
    let uri = common::closed_port_uri();
    let err = provider(&ForgeHost::github(), TOKEN, Some(&uri)).current_user().await.expect_err("tidak terhubung");
    assert_error(&err, ForgeErrorCode::Network, TOKEN);
}

#[tokio::test]
async fn empty_repo_ref_is_rejected_before_any_request() {
    let server = MockServer::start().await;
    let client = client(&server);
    let broken = repo("github.com", "", "hello");

    let err = client.default_branch(&broken).await.expect_err("pemilik kosong");
    assert_error(&err, ForgeErrorCode::InvalidInput, TOKEN);
    let err = client.list_pull_requests(&broken, PrFilter::Mine, 1).await.expect_err("pemilik kosong");
    assert_error(&err, ForgeErrorCode::InvalidInput, TOKEN);
    assert!(received(&server).await.is_empty());
}
