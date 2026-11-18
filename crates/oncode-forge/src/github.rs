//! Penyedia GitHub: github.com, GitHub Enterprise Cloud, dan GitHub Enterprise Server.

use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};

use crate::error::{ForgeError, Result};
use crate::http::{
    encode_segment, next_page_from_link, non_empty, timestamp_or_zero, Api, Flavor, PER_PAGE, SCAN_PER_PAGE,
};
use crate::model::{
    Account, Check, CheckState, ForgeHost, NewPullRequest, Page, PrFilter, PrState, PullRequest, PullRequestDetail,
    RemoteRepo, RepoRef,
};
use crate::provider::ForgeProvider;

/// Batas halaman (masing-masing 100 butir) yang dipindai dalam satu panggilan.
const MAX_SCAN_PAGES: u32 = 5;
/// Batas halaman check run yang diambil untuk satu commit.
const MAX_CHECK_PAGES: u32 = 10;
/// Jumlah permintaan detail PR yang berjalan bersamaan saat melengkapi hasil pencarian.
const HYDRATE_CONCURRENCY: usize = 6;

#[derive(Debug)]
pub(crate) struct GitHubProvider {
    api: Api,
    host: ForgeHost,
}

impl GitHubProvider {
    pub(crate) fn new(host: &ForgeHost, token: &str, api_base_override: Option<&str>) -> Self {
        let base = api_base_override.map_or_else(|| host.api_base(), str::to_string);
        Self { api: Api::new(Flavor::GitHub, &base, &host.hostname(), token), host: host.clone() }
    }

    async fn fetch_pull(&self, repo: &RepoRef, number: u32) -> Result<GhPull> {
        let path = format!("{}/pulls/{number}", repo_path(repo)?);
        let (pull, _) = self.api.get::<GhPull>(&path, &[]).await?;
        Ok(pull)
    }

    /// PR lewat API pencarian. Hasil pencarian tidak memuat branch dan SHA, jadi tiap
    /// butir dilengkapi dengan `GET /pulls/{n}`.
    async fn search_pulls(&self, repo: &RepoRef, qualifier: &str, page: u32) -> Result<Page<PullRequest>> {
        // Validasi lebih dulu supaya pemilik/nama yang kosong tidak sampai ke query.
        repo_path(repo)?;
        let q = format!("repo:{}/{} is:pr is:open {qualifier}", repo.owner, repo.name);
        let query = [
            ("q", q),
            ("sort", "updated".to_string()),
            ("order", "desc".to_string()),
            ("per_page", PER_PAGE.to_string()),
            ("page", page.to_string()),
        ];
        let (found, headers) = self.api.get::<GhSearch>("/search/issues", &query).await?;
        let next_page = next_page_from_link(&headers);

        let pulls: Vec<Result<GhPull>> = stream::iter(found.items.into_iter().map(|item| item.number))
            .map(|number| self.fetch_pull(repo, number))
            .buffered(HYDRATE_CONCURRENCY)
            .collect()
            .await;
        let mut items = Vec::with_capacity(pulls.len());
        for pull in pulls {
            items.push(PullRequest::from(pull?));
        }
        Ok(Page::new(items, next_page))
    }

    async fn check_runs(&self, commit_path: &str) -> Result<Vec<Check>> {
        let path = format!("{commit_path}/check-runs");
        let mut checks = Vec::new();
        let mut next = Some(1);
        let mut fetched = 0;
        while let Some(page) = next {
            if fetched >= MAX_CHECK_PAGES {
                break;
            }
            let query = [("per_page", SCAN_PER_PAGE.to_string()), ("page", page.to_string())];
            let (runs, headers) = self.api.get::<GhCheckRuns>(&path, &query).await?;
            fetched += 1;
            checks.extend(runs.check_runs.into_iter().map(|run| Check {
                state: check_run_state(&run.status, run.conclusion.as_deref()),
                name: run.name,
                url: non_empty(run.html_url).or(non_empty(run.details_url)),
            }));
            next = next_page_from_link(&headers);
        }
        Ok(checks)
    }

    async fn commit_statuses(&self, commit_path: &str) -> Result<Vec<Check>> {
        // Status gabungan hanya memuat status terbaru untuk tiap konteks.
        let path = format!("{commit_path}/status");
        let query = [("per_page", SCAN_PER_PAGE.to_string())];
        let (combined, _) = self.api.get::<GhCombinedStatus>(&path, &query).await?;
        Ok(combined
            .statuses
            .into_iter()
            .map(|status| Check {
                state: commit_status_state(&status.state),
                name: status.context,
                url: non_empty(status.target_url),
            })
            .collect())
    }
}

#[async_trait::async_trait]
impl ForgeProvider for GitHubProvider {
    async fn current_user(&self) -> Result<Account> {
        let (user, _) = self.api.get::<GhUser>("/user", &[]).await?;
        let display_name = non_empty(user.name).unwrap_or_else(|| user.login.clone());
        Ok(Account {
            id: Account::make_id(&self.host, &user.login),
            kind: self.host.kind,
            host: self.host.web_base().to_string(),
            username: user.login,
            display_name,
            avatar_url: non_empty(user.avatar_url),
        })
    }

    async fn list_repos(&self, query: &str, page: u32) -> Result<Page<RemoteRepo>> {
        let needle = query.trim().to_lowercase();
        let page = page.max(1);
        let page_query = |per_page: u32, page: u32| {
            [
                ("sort", "updated".to_string()),
                ("direction", "desc".to_string()),
                ("per_page", per_page.to_string()),
                ("page", page.to_string()),
            ]
        };

        if needle.is_empty() {
            let (repos, headers) = self.api.get::<Vec<GhRepo>>("/user/repos", &page_query(PER_PAGE, page)).await?;
            return Ok(Page::new(repos.into_iter().map(RemoteRepo::from).collect(), next_page_from_link(&headers)));
        }

        // Pindai halaman berukuran 100 sampai cukup banyak yang cocok. Halaman tidak pernah
        // dipotong di tengah, sehingga `next_page` selalu aman dipakai untuk melanjutkan.
        let mut items = Vec::new();
        let mut next = Some(page);
        let mut scanned = 0;
        while let Some(current) = next {
            if scanned >= MAX_SCAN_PAGES || items.len() >= PER_PAGE as usize {
                break;
            }
            let (repos, headers) =
                self.api.get::<Vec<GhRepo>>("/user/repos", &page_query(SCAN_PER_PAGE, current)).await?;
            scanned += 1;
            items.extend(
                repos.into_iter().filter(|repo| repo.full_name.to_lowercase().contains(&needle)).map(RemoteRepo::from),
            );
            next = next_page_from_link(&headers);
        }
        Ok(Page::new(items, next))
    }

    async fn list_pull_requests(&self, repo: &RepoRef, filter: PrFilter, page: u32) -> Result<Page<PullRequest>> {
        let page = page.max(1);
        let state = match filter {
            PrFilter::Open => "open",
            PrFilter::All => "all",
            // Endpoint daftar PR tidak bisa menyaring per penulis atau reviewer.
            PrFilter::Mine => return self.search_pulls(repo, "author:@me", page).await,
            PrFilter::ReviewRequested => return self.search_pulls(repo, "review-requested:@me", page).await,
        };
        let path = format!("{}/pulls", repo_path(repo)?);
        let query = [
            ("state", state.to_string()),
            ("sort", "updated".to_string()),
            ("direction", "desc".to_string()),
            ("per_page", PER_PAGE.to_string()),
            ("page", page.to_string()),
        ];
        let (pulls, headers) = self.api.get::<Vec<GhPull>>(&path, &query).await?;
        Ok(Page::new(pulls.into_iter().map(PullRequest::from).collect(), next_page_from_link(&headers)))
    }

    async fn get_pull_request(&self, repo: &RepoRef, number: u32) -> Result<PullRequestDetail> {
        // `requested_reviewers` hanya memuat yang belum memberi review, jadi daftar review
        // diambil juga supaya reviewer yang sudah selesai tetap muncul.
        let reviews_path = format!("{}/pulls/{number}/reviews", repo_path(repo)?);
        let reviews_query = [("per_page", SCAN_PER_PAGE.to_string())];
        let (pull, (reviews, _)) = tokio::try_join!(
            self.fetch_pull(repo, number),
            self.api.get::<Vec<GhReview>>(&reviews_path, &reviews_query),
        )?;

        let author = pull.user.as_ref().map(|u| u.login.clone());
        let mut reviewers: Vec<String> = Vec::new();
        let requested = pull.requested_reviewers.iter().map(|u| u.login.clone());
        let teams = pull.requested_teams.iter().map(|t| t.name.clone());
        let reviewed = reviews.into_iter().filter_map(|r| r.user).map(|u| u.login);
        for name in requested.chain(teams).chain(reviewed) {
            if Some(&name) != author.as_ref() && !reviewers.contains(&name) {
                reviewers.push(name);
            }
        }

        Ok(PullRequestDetail {
            body: pull.body.clone().unwrap_or_default(),
            reviewers,
            labels: pull.labels.iter().map(|l| l.name.clone()).collect(),
            mergeable: pull.mergeable,
            commits: pull.commits.unwrap_or(0),
            changed_files: pull.changed_files.unwrap_or(0),
            additions: pull.additions.unwrap_or(0),
            deletions: pull.deletions.unwrap_or(0),
            pull_request: PullRequest::from(pull),
        })
    }

    async fn list_checks(&self, repo: &RepoRef, sha: &str) -> Result<Vec<Check>> {
        let sha = sha.trim();
        if sha.is_empty() {
            return Err(ForgeError::invalid("Commit SHA must not be empty"));
        }
        let commit_path = format!("{}/commits/{}", repo_path(repo)?, encode_segment(sha));
        let (mut checks, statuses) =
            tokio::try_join!(self.check_runs(&commit_path), self.commit_statuses(&commit_path))?;
        checks.extend(statuses);
        Ok(checks)
    }

    async fn create_pull_request(&self, repo: &RepoRef, input: &NewPullRequest) -> Result<PullRequest> {
        crate::provider::validate_new_pull_request(input)?;
        let path = format!("{}/pulls", repo_path(repo)?);
        let body = GhNewPull {
            title: input.title.trim(),
            head: input.source_branch.trim(),
            base: input.target_branch.trim(),
            body: &input.body,
            draft: input.draft,
        };
        let pull: GhPull = self.api.post(&path, &body).await?;
        Ok(PullRequest::from(pull))
    }

    async fn default_branch(&self, repo: &RepoRef) -> Result<String> {
        let (found, _) = self.api.get::<GhRepo>(&repo_path(repo)?, &[]).await?;
        non_empty(found.default_branch)
            .ok_or_else(|| ForgeError::invalid_response("This repository has no default branch yet"))
    }
}

fn repo_path(repo: &RepoRef) -> Result<String> {
    if repo.owner.trim().is_empty() || repo.name.trim().is_empty() {
        return Err(ForgeError::invalid("Repository owner and name must not be empty"));
    }
    Ok(format!("/repos/{}/{}", encode_segment(&repo.owner), encode_segment(&repo.name)))
}

/// Memetakan `status` dan `conclusion` check run ke [`CheckState`].
fn check_run_state(status: &str, conclusion: Option<&str>) -> CheckState {
    match status {
        "completed" => match conclusion.unwrap_or_default() {
            "success" => CheckState::Success,
            "failure" | "timed_out" | "action_required" | "startup_failure" => CheckState::Failure,
            "cancelled" => CheckState::Cancelled,
            "skipped" => CheckState::Skipped,
            _ => CheckState::Neutral,
        },
        "in_progress" => CheckState::Running,
        _ => CheckState::Pending,
    }
}

/// Memetakan `state` commit status ke [`CheckState`].
fn commit_status_state(state: &str) -> CheckState {
    match state {
        "success" => CheckState::Success,
        "failure" | "error" => CheckState::Failure,
        _ => CheckState::Pending,
    }
}

#[derive(Debug, Deserialize)]
struct GhUser {
    login: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    avatar_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhActor {
    login: String,
    #[serde(default)]
    avatar_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhRepo {
    full_name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    private: bool,
    #[serde(default)]
    fork: bool,
    #[serde(default)]
    clone_url: Option<String>,
    #[serde(default)]
    ssh_url: Option<String>,
    #[serde(default)]
    default_branch: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
}

impl From<GhRepo> for RemoteRepo {
    fn from(repo: GhRepo) -> Self {
        RemoteRepo {
            full_name: repo.full_name,
            description: repo.description.unwrap_or_default(),
            private: repo.private,
            fork: repo.fork,
            clone_url_https: repo.clone_url.unwrap_or_default(),
            clone_url_ssh: repo.ssh_url.unwrap_or_default(),
            default_branch: repo.default_branch.unwrap_or_default(),
            updated_at: timestamp_or_zero(repo.updated_at.as_deref()),
            web_url: repo.html_url.unwrap_or_default(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct GhBranchRepo {
    full_name: String,
}

#[derive(Debug, Deserialize)]
struct GhBranch {
    #[serde(rename = "ref")]
    name: String,
    sha: String,
    #[serde(default)]
    repo: Option<GhBranchRepo>,
}

#[derive(Debug, Deserialize)]
struct GhLabel {
    name: String,
}

#[derive(Debug, Deserialize)]
struct GhTeam {
    name: String,
}

#[derive(Debug, Deserialize)]
struct GhPull {
    number: u32,
    title: String,
    state: String,
    #[serde(default)]
    draft: Option<bool>,
    #[serde(default)]
    merged_at: Option<String>,
    #[serde(default)]
    merged: Option<bool>,
    #[serde(default)]
    user: Option<GhActor>,
    head: GhBranch,
    base: GhBranch,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    // Hanya ada di jawaban detail.
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    requested_reviewers: Vec<GhActor>,
    #[serde(default)]
    requested_teams: Vec<GhTeam>,
    #[serde(default)]
    labels: Vec<GhLabel>,
    #[serde(default)]
    mergeable: Option<bool>,
    #[serde(default)]
    commits: Option<u32>,
    #[serde(default)]
    changed_files: Option<u32>,
    #[serde(default)]
    additions: Option<u32>,
    #[serde(default)]
    deletions: Option<u32>,
}

impl From<GhPull> for PullRequest {
    fn from(pull: GhPull) -> Self {
        let state = if pull.merged_at.is_some() || pull.merged == Some(true) {
            PrState::Merged
        } else if pull.state == "open" {
            PrState::Open
        } else {
            PrState::Closed
        };
        // Repositori sumber bisa `null` bila fork-nya sudah dihapus.
        let is_cross_repo = match (&pull.head.repo, &pull.base.repo) {
            (Some(head), Some(base)) => head.full_name != base.full_name,
            (None, _) => true,
            (Some(_), None) => false,
        };
        let (author, author_avatar) = match pull.user {
            Some(user) => (user.login, non_empty(user.avatar_url)),
            // Akun yang sudah dihapus.
            None => ("ghost".to_string(), None),
        };
        PullRequest {
            number: pull.number,
            title: pull.title,
            state,
            draft: pull.draft.unwrap_or(false),
            author,
            author_avatar,
            source_branch: pull.head.name,
            target_branch: pull.base.name,
            head_sha: pull.head.sha,
            updated_at: timestamp_or_zero(pull.updated_at.as_deref()),
            web_url: pull.html_url.unwrap_or_default(),
            is_cross_repo,
        }
    }
}

#[derive(Debug, Deserialize)]
struct GhSearch {
    #[serde(default)]
    items: Vec<GhSearchItem>,
}

#[derive(Debug, Deserialize)]
struct GhSearchItem {
    number: u32,
}

#[derive(Debug, Deserialize)]
struct GhReview {
    #[serde(default)]
    user: Option<GhActor>,
}

#[derive(Debug, Deserialize)]
struct GhCheckRuns {
    #[serde(default)]
    check_runs: Vec<GhCheckRun>,
}

#[derive(Debug, Deserialize)]
struct GhCheckRun {
    name: String,
    status: String,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    details_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhCombinedStatus {
    #[serde(default)]
    statuses: Vec<GhStatus>,
}

#[derive(Debug, Deserialize)]
struct GhStatus {
    context: String,
    state: String,
    #[serde(default)]
    target_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct GhNewPull<'a> {
    title: &'a str,
    head: &'a str,
    base: &'a str,
    body: &'a str,
    draft: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_run_states() {
        assert_eq!(check_run_state("queued", None), CheckState::Pending);
        assert_eq!(check_run_state("waiting", None), CheckState::Pending);
        assert_eq!(check_run_state("in_progress", None), CheckState::Running);
        assert_eq!(check_run_state("completed", Some("success")), CheckState::Success);
        assert_eq!(check_run_state("completed", Some("failure")), CheckState::Failure);
        assert_eq!(check_run_state("completed", Some("timed_out")), CheckState::Failure);
        assert_eq!(check_run_state("completed", Some("action_required")), CheckState::Failure);
        assert_eq!(check_run_state("completed", Some("cancelled")), CheckState::Cancelled);
        assert_eq!(check_run_state("completed", Some("skipped")), CheckState::Skipped);
        assert_eq!(check_run_state("completed", Some("neutral")), CheckState::Neutral);
        assert_eq!(check_run_state("completed", Some("stale")), CheckState::Neutral);
        assert_eq!(check_run_state("completed", None), CheckState::Neutral);
    }

    #[test]
    fn commit_status_states() {
        assert_eq!(commit_status_state("success"), CheckState::Success);
        assert_eq!(commit_status_state("failure"), CheckState::Failure);
        assert_eq!(commit_status_state("error"), CheckState::Failure);
        assert_eq!(commit_status_state("pending"), CheckState::Pending);
    }

    #[test]
    fn repo_path_rejects_empty_parts() {
        let repo = RepoRef { host: "github.com".into(), owner: " ".into(), name: "x".into() };
        assert!(repo_path(&repo).is_err());
        let repo = RepoRef { host: "github.com".into(), owner: "octo".into(), name: "hello world".into() };
        assert_eq!(repo_path(&repo).expect("path"), "/repos/octo/hello%20world");
    }
}
