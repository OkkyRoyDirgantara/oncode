//! Penyedia GitLab: gitlab.com dan instance self-hosted.

use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;

use crate::error::{ForgeError, Result};
use crate::http::{
    encode_segment, next_page_from_gitlab, non_empty, timestamp_or_zero, Api, Flavor, PER_PAGE, SCAN_PER_PAGE,
};
use crate::model::{
    Account, Check, CheckState, ForgeHost, NewPullRequest, Page, PrFilter, PrState, PullRequest, PullRequestDetail,
    RemoteRepo, RepoRef,
};
use crate::provider::ForgeProvider;

/// Batas halaman job (masing-masing 100 butir) yang diambil untuk satu pipeline.
const MAX_JOB_PAGES: u32 = 10;

#[derive(Debug)]
pub(crate) struct GitLabProvider {
    api: Api,
    host: ForgeHost,
    /// Username akun ini, diambil sekali saat pertama dibutuhkan.
    username: OnceCell<String>,
}

impl GitLabProvider {
    pub(crate) fn new(host: &ForgeHost, token: &str, api_base_override: Option<&str>) -> Self {
        let base = api_base_override.map_or_else(|| host.api_base(), str::to_string);
        Self {
            api: Api::new(Flavor::GitLab, &base, &host.hostname(), token),
            host: host.clone(),
            username: OnceCell::new(),
        }
    }

    async fn fetch_user(&self) -> Result<GlUser> {
        let (user, _) = self.api.get::<GlUser>("/user", &[]).await?;
        Ok(user)
    }

    async fn username(&self) -> Result<&str> {
        let name = self.username.get_or_try_init(|| async { self.fetch_user().await.map(|u| u.username) }).await?;
        Ok(name.as_str())
    }

    /// Avatar di instance self-hosted bisa berupa path relatif.
    fn absolute_url(&self, url: Option<String>) -> Option<String> {
        let url = non_empty(url)?;
        if url.starts_with('/') && !url.starts_with("//") {
            Some(format!("{}{url}", self.host.web_base()))
        } else {
            Some(url)
        }
    }

    fn pull_request(&self, mr: GlMergeRequest) -> PullRequest {
        let state = match mr.state.as_str() {
            "merged" => PrState::Merged,
            "closed" => PrState::Closed,
            // `opened`, dan `locked` (sedang dalam proses merge).
            _ => PrState::Open,
        };
        let is_cross_repo = match (mr.source_project_id, mr.target_project_id) {
            (Some(source), Some(target)) => source != target,
            _ => false,
        };
        let (author, author_avatar) = match mr.author {
            Some(author) => (author.username, self.absolute_url(author.avatar_url)),
            None => (String::new(), None),
        };
        PullRequest {
            number: mr.iid,
            title: mr.title,
            state,
            draft: mr.draft.or(mr.work_in_progress).unwrap_or(false),
            author,
            author_avatar,
            source_branch: mr.source_branch,
            target_branch: mr.target_branch,
            head_sha: mr.sha.unwrap_or_default(),
            updated_at: timestamp_or_zero(mr.updated_at.as_deref()),
            web_url: mr.web_url.unwrap_or_default(),
            is_cross_repo,
        }
    }

    async fn commit_count(&self, mr_path: &str) -> Result<u32> {
        let query = [("per_page", SCAN_PER_PAGE.to_string())];
        let (commits, headers) =
            self.api.get::<Vec<serde::de::IgnoredAny>>(&format!("{mr_path}/commits"), &query).await?;
        let total = headers.get("x-total").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u32>().ok());
        Ok(total.unwrap_or(commits.len() as u32))
    }
}

#[async_trait::async_trait]
impl ForgeProvider for GitLabProvider {
    async fn current_user(&self) -> Result<Account> {
        let user = self.fetch_user().await?;
        let display_name = non_empty(user.name).unwrap_or_else(|| user.username.clone());
        Ok(Account {
            id: Account::make_id(&self.host, &user.username),
            kind: self.host.kind,
            host: self.host.web_base().to_string(),
            avatar_url: self.absolute_url(user.avatar_url),
            username: user.username,
            display_name,
        })
    }

    async fn list_repos(&self, query: &str, page: u32) -> Result<Page<RemoteRepo>> {
        let mut params = vec![
            ("membership", "true".to_string()),
            ("order_by", "last_activity_at".to_string()),
            ("sort", "desc".to_string()),
            ("per_page", PER_PAGE.to_string()),
            ("page", page.max(1).to_string()),
        ];
        let needle = query.trim();
        if !needle.is_empty() {
            params.push(("search", needle.to_string()));
        }
        let (projects, headers) = self.api.get::<Vec<GlProject>>("/projects", &params).await?;
        Ok(Page::new(projects.into_iter().map(RemoteRepo::from).collect(), next_page_from_gitlab(&headers)))
    }

    async fn list_pull_requests(&self, repo: &RepoRef, filter: PrFilter, page: u32) -> Result<Page<PullRequest>> {
        let path = format!("{}/merge_requests", project_path(repo)?);
        let mut params = vec![
            ("state", if filter == PrFilter::All { "all" } else { "opened" }.to_string()),
            ("order_by", "updated_at".to_string()),
            ("sort", "desc".to_string()),
            ("per_page", PER_PAGE.to_string()),
            ("page", page.max(1).to_string()),
        ];
        match filter {
            PrFilter::Open | PrFilter::All => {}
            PrFilter::Mine => params.push(("scope", "created_by_me".to_string())),
            // `reviewer_username` tersedia sejak GitLab 13.8, lebih lama daripada
            // `scope=reviews_for_me`, sehingga aman untuk instance self-hosted lama.
            PrFilter::ReviewRequested => params.push(("reviewer_username", self.username().await?.to_string())),
        }
        let (mrs, headers) = self.api.get::<Vec<GlMergeRequest>>(&path, &params).await?;
        Ok(Page::new(mrs.into_iter().map(|mr| self.pull_request(mr)).collect(), next_page_from_gitlab(&headers)))
    }

    async fn get_pull_request(&self, repo: &RepoRef, number: u32) -> Result<PullRequestDetail> {
        let path = format!("{}/merge_requests/{number}", project_path(repo)?);
        // Jumlah commit tidak ada di jawaban MR, jadi diambil dari daftar commit-nya.
        let ((mr, _), commits) =
            tokio::try_join!(self.api.get::<GlMergeRequest>(&path, &[]), self.commit_count(&path))?;

        let mergeable = if mr.has_conflicts == Some(true) {
            Some(false)
        } else {
            match mr.merge_status.as_deref() {
                Some("can_be_merged") => Some(true),
                Some("cannot_be_merged") => Some(false),
                // `unchecked`, `checking`, `cannot_be_merged_recheck`: masih dihitung.
                _ => None,
            }
        };
        // `changes_count` berupa string, dan dibatasi sampai "1000+".
        let changed_files = mr
            .changes_count
            .as_deref()
            .map(|count| count.chars().take_while(char::is_ascii_digit).collect::<String>())
            .and_then(|digits| digits.parse::<u32>().ok())
            .unwrap_or(0);
        let labels = mr
            .labels
            .iter()
            .filter_map(|label| match label {
                serde_json::Value::String(name) => Some(name.clone()),
                // Bentuk objek muncul bila `with_labels_details` aktif.
                serde_json::Value::Object(map) => map.get("name").and_then(|n| n.as_str()).map(str::to_string),
                _ => None,
            })
            .collect();

        Ok(PullRequestDetail {
            body: mr.description.clone().unwrap_or_default(),
            reviewers: mr.reviewers.iter().map(|r| r.username.clone()).collect(),
            labels,
            mergeable,
            commits,
            changed_files,
            // GitLab tidak menyediakan jumlah baris yang ditambah/dihapus lewat REST.
            additions: 0,
            deletions: 0,
            pull_request: self.pull_request(mr),
        })
    }

    async fn list_checks(&self, repo: &RepoRef, sha: &str) -> Result<Vec<Check>> {
        let sha = sha.trim();
        if sha.is_empty() {
            return Err(ForgeError::invalid("Commit SHA must not be empty"));
        }
        let project = project_path(repo)?;
        let query = [
            ("sha", sha.to_string()),
            ("order_by", "id".to_string()),
            ("sort", "desc".to_string()),
            ("per_page", "1".to_string()),
        ];
        let (pipelines, _) = self.api.get::<Vec<GlPipeline>>(&format!("{project}/pipelines"), &query).await?;
        let Some(pipeline) = pipelines.into_iter().next() else {
            return Ok(Vec::new());
        };

        let jobs_path = format!("{project}/pipelines/{}/jobs", pipeline.id);
        let mut jobs: Vec<GlJob> = Vec::new();
        let mut next = Some(1);
        let mut fetched = 0;
        while let Some(page) = next {
            if fetched >= MAX_JOB_PAGES {
                break;
            }
            let query = [("per_page", SCAN_PER_PAGE.to_string()), ("page", page.to_string())];
            let (batch, headers) = self.api.get::<Vec<GlJob>>(&jobs_path, &query).await?;
            fetched += 1;
            jobs.extend(batch);
            next = next_page_from_gitlab(&headers);
        }

        if jobs.is_empty() {
            // Pipeline tanpa job yang terlihat (misalnya hanya memicu pipeline lain).
            return Ok(vec![Check {
                name: format!("Pipeline #{}", pipeline.id),
                state: job_state(&pipeline.status, false),
                url: non_empty(pipeline.web_url),
            }]);
        }

        // API mengurutkan dari job terbaru; dibalik agar mengikuti urutan pipeline.
        jobs.sort_by_key(|job| job.id);
        Ok(jobs
            .into_iter()
            .map(|job| Check {
                state: job_state(&job.status, job.allow_failure),
                name: job.name,
                url: non_empty(job.web_url),
            })
            .collect())
    }

    async fn create_pull_request(&self, repo: &RepoRef, input: &NewPullRequest) -> Result<PullRequest> {
        crate::provider::validate_new_pull_request(input)?;
        let path = format!("{}/merge_requests", project_path(repo)?);
        let title = merge_request_title(&input.title, input.draft);
        let body = GlNewMergeRequest {
            source_branch: input.source_branch.trim(),
            target_branch: input.target_branch.trim(),
            title: &title,
            description: &input.body,
        };
        let mr: GlMergeRequest = self.api.post(&path, &body).await?;
        Ok(self.pull_request(mr))
    }

    async fn default_branch(&self, repo: &RepoRef) -> Result<String> {
        let (project, _) = self.api.get::<GlProject>(&project_path(repo)?, &[]).await?;
        non_empty(project.default_branch)
            .ok_or_else(|| ForgeError::invalid_response("This repository has no default branch yet"))
    }
}

/// `/projects/{id}` dengan jalur proyek di-encode (`grup%2Fproyek`).
fn project_path(repo: &RepoRef) -> Result<String> {
    if repo.owner.trim().is_empty() || repo.name.trim().is_empty() {
        return Err(ForgeError::invalid("Repository owner and name must not be empty"));
    }
    Ok(format!("/projects/{}", encode_segment(&repo.full_name())))
}

/// GitLab tidak punya parameter draft saat membuat MR; status draft ditentukan oleh
/// awalan judul.
fn merge_request_title(title: &str, draft: bool) -> String {
    let title = title.trim();
    if !draft || has_draft_prefix(title) {
        return title.to_string();
    }
    format!("Draft: {title}")
}

fn has_draft_prefix(title: &str) -> bool {
    let lower = title.trim_start().to_lowercase();
    ["draft:", "[draft]", "(draft)"].iter().any(|prefix| lower.starts_with(prefix))
}

/// Memetakan status job atau pipeline ke [`CheckState`].
fn job_state(status: &str, allow_failure: bool) -> CheckState {
    match status {
        "success" => CheckState::Success,
        // Job yang boleh gagal tampil sebagai peringatan, bukan kegagalan.
        "failed" if allow_failure => CheckState::Neutral,
        "failed" => CheckState::Failure,
        "running" | "canceling" => CheckState::Running,
        "canceled" | "cancelled" => CheckState::Cancelled,
        "skipped" => CheckState::Skipped,
        // Menunggu dijalankan secara manual.
        "manual" => CheckState::Neutral,
        // `created`, `pending`, `waiting_for_resource`, `preparing`, `scheduled`, dan
        // status baru yang belum dikenal.
        _ => CheckState::Pending,
    }
}

#[derive(Debug, Deserialize)]
struct GlUser {
    username: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    avatar_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GlActor {
    username: String,
    #[serde(default)]
    avatar_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GlProject {
    path_with_namespace: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    forked_from_project: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    http_url_to_repo: Option<String>,
    #[serde(default)]
    ssh_url_to_repo: Option<String>,
    #[serde(default)]
    default_branch: Option<String>,
    #[serde(default)]
    last_activity_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    web_url: Option<String>,
}

impl From<GlProject> for RemoteRepo {
    fn from(project: GlProject) -> Self {
        let updated = project.last_activity_at.as_deref().or(project.updated_at.as_deref());
        RemoteRepo {
            updated_at: timestamp_or_zero(updated),
            full_name: project.path_with_namespace,
            description: project.description.unwrap_or_default(),
            // `internal` terlihat oleh semua pengguna yang login, tetapi bukan publik.
            private: project.visibility.as_deref().is_some_and(|v| v != "public"),
            fork: project.forked_from_project.is_some(),
            clone_url_https: project.http_url_to_repo.unwrap_or_default(),
            clone_url_ssh: project.ssh_url_to_repo.unwrap_or_default(),
            default_branch: project.default_branch.unwrap_or_default(),
            web_url: project.web_url.unwrap_or_default(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct GlMergeRequest {
    iid: u32,
    title: String,
    state: String,
    #[serde(default)]
    draft: Option<bool>,
    #[serde(default)]
    work_in_progress: Option<bool>,
    #[serde(default)]
    author: Option<GlActor>,
    source_branch: String,
    target_branch: String,
    #[serde(default)]
    sha: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    web_url: Option<String>,
    #[serde(default)]
    source_project_id: Option<u64>,
    #[serde(default)]
    target_project_id: Option<u64>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    reviewers: Vec<GlActor>,
    #[serde(default)]
    labels: Vec<serde_json::Value>,
    #[serde(default)]
    merge_status: Option<String>,
    #[serde(default)]
    has_conflicts: Option<bool>,
    #[serde(default)]
    changes_count: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GlPipeline {
    id: u64,
    status: String,
    #[serde(default)]
    web_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GlJob {
    id: u64,
    name: String,
    status: String,
    #[serde(default)]
    allow_failure: bool,
    #[serde(default)]
    web_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct GlNewMergeRequest<'a> {
    source_branch: &'a str,
    target_branch: &'a str,
    title: &'a str,
    description: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_path_is_url_encoded() {
        let repo = RepoRef { host: "gitlab.com".into(), owner: "grup/subgrup".into(), name: "proyek".into() };
        assert_eq!(project_path(&repo).expect("path"), "/projects/grup%2Fsubgrup%2Fproyek");
        let empty = RepoRef { host: "gitlab.com".into(), owner: String::new(), name: "proyek".into() };
        assert!(project_path(&empty).is_err());
    }

    #[test]
    fn draft_title_prefix() {
        assert_eq!(merge_request_title("Tambah fitur", true), "Draft: Tambah fitur");
        assert_eq!(merge_request_title("Tambah fitur", false), "Tambah fitur");
        assert_eq!(merge_request_title("Draft: Tambah fitur", true), "Draft: Tambah fitur");
        assert_eq!(merge_request_title("draft: tambah fitur", true), "draft: tambah fitur");
        assert_eq!(merge_request_title("[Draft] Tambah fitur", true), "[Draft] Tambah fitur");
        assert_eq!(merge_request_title("(draft) Tambah fitur", true), "(draft) Tambah fitur");
        // "Drafting" bukan awalan draft.
        assert_eq!(merge_request_title("Drafting ulang", true), "Draft: Drafting ulang");
    }

    #[test]
    fn job_states() {
        assert_eq!(job_state("created", false), CheckState::Pending);
        assert_eq!(job_state("pending", false), CheckState::Pending);
        assert_eq!(job_state("waiting_for_resource", false), CheckState::Pending);
        assert_eq!(job_state("preparing", false), CheckState::Pending);
        assert_eq!(job_state("scheduled", false), CheckState::Pending);
        assert_eq!(job_state("running", false), CheckState::Running);
        assert_eq!(job_state("success", false), CheckState::Success);
        assert_eq!(job_state("failed", false), CheckState::Failure);
        assert_eq!(job_state("failed", true), CheckState::Neutral);
        assert_eq!(job_state("canceled", false), CheckState::Cancelled);
        assert_eq!(job_state("skipped", false), CheckState::Skipped);
        assert_eq!(job_state("manual", false), CheckState::Neutral);
    }
}
