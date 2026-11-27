//! GitLab provider: gitlab.com and self-hosted instances.

use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;

use crate::error::{ForgeError, Result};
use crate::http::{
    encode_segment, next_page_from_gitlab, non_empty, timestamp_or_zero, Api, Flavor, PER_PAGE, SCAN_PER_PAGE,
};
use crate::model::{
    Account, Check, CheckState, ForgeHost, MergeInput, MergeMethod, MergeOutcome, MergeState, NewPullRequest, Page, PrFilter, PrState, PullRequest, PullRequestDetail,
    RemoteRepo, RepoRef, ReviewComment, ReviewEvent, ReviewInput, ReviewSide,
};
use crate::provider::ForgeProvider;

/// Limit on job pages (100 items each) fetched for a single pipeline.
const MAX_JOB_PAGES: u32 = 10;

#[derive(Debug)]
pub(crate) struct GitLabProvider {
    api: Api,
    host: ForgeHost,
    /// This account's username, fetched once when first needed.
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

    /// Avatars on self-hosted instances can be relative paths.
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
            // `opened`, and `locked` (a merge is in progress).
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
            // `reviewer_username` has been available since GitLab 13.8, older than
            // `scope=reviews_for_me`, so it is safe for old self-hosted instances.
            PrFilter::ReviewRequested => params.push(("reviewer_username", self.username().await?.to_string())),
        }
        let (mrs, headers) = self.api.get::<Vec<GlMergeRequest>>(&path, &params).await?;
        Ok(Page::new(mrs.into_iter().map(|mr| self.pull_request(mr)).collect(), next_page_from_gitlab(&headers)))
    }

    async fn get_pull_request(&self, repo: &RepoRef, number: u32) -> Result<PullRequestDetail> {
        let path = format!("{}/merge_requests/{number}", project_path(repo)?);
        // The commit count is not in the MR response, so it is taken from its commit list.
        let main = async { tokio::try_join!(self.api.get::<GlMergeRequest>(&path, &[]), self.commit_count(&path)) };
        // The allowed merge methods; not essential, so a failure is ignored.
        let settings = async { self.api.get::<GlProject>(&project_path(repo)?, &[]).await.map(|(found, _)| found) };
        let (main, settings) = tokio::join!(main, settings);
        let ((mr, _), commits) = main?;
        let merge_methods = gitlab_merge_methods(settings.ok().as_ref());
        let merge_state = gitlab_merge_state(&mr);

        let mergeable = if mr.has_conflicts == Some(true) {
            Some(false)
        } else {
            match mr.merge_status.as_deref() {
                Some("can_be_merged") => Some(true),
                Some("cannot_be_merged") => Some(false),
                // `unchecked`, `checking`, `cannot_be_merged_recheck`: still being computed.
                _ => None,
            }
        };
        // `changes_count` is a string, capped at "1000+".
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
                // The object form appears when `with_labels_details` is enabled.
                serde_json::Value::Object(map) => map.get("name").and_then(|n| n.as_str()).map(str::to_string),
                _ => None,
            })
            .collect();

        Ok(PullRequestDetail {
            merge_state,
            merge_methods,
            body: mr.description.clone().unwrap_or_default(),
            reviewers: mr.reviewers.iter().map(|r| r.username.clone()).collect(),
            labels,
            mergeable,
            commits,
            changed_files,
            // GitLab does not provide the number of added/deleted lines via REST.
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
            // A pipeline with no visible jobs (for example one that only triggers another pipeline).
            return Ok(vec![Check {
                name: format!("Pipeline #{}", pipeline.id),
                state: job_state(&pipeline.status, false),
                url: non_empty(pipeline.web_url),
            }]);
        }

        // The API sorts from the newest job; reversed to follow the pipeline order.
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

    async fn merge_pull_request(&self, repo: &RepoRef, number: u32, input: &MergeInput) -> Result<MergeOutcome> {
        if input.method == MergeMethod::Rebase {
            return Err(ForgeError::invalid("GitLab merges with the project's own merge method; choose Merge or Squash"));
        }
        let squash = input.method == MergeMethod::Squash;
        let message = match (input.title.as_deref(), input.message.as_deref()) {
            (Some(title), Some(body)) if !body.trim().is_empty() => Some(format!("{title}\n\n{body}")),
            (Some(title), _) if !title.trim().is_empty() => Some(title.to_string()),
            _ => None,
        };
        let body = GlMerge {
            squash,
            sha: input.sha.as_deref(),
            should_remove_source_branch: input.delete_branch,
            merge_commit_message: if squash { None } else { message.as_deref() },
            squash_commit_message: if squash { message.as_deref() } else { None },
        };
        let path = format!("{}/merge_requests/{number}/merge", project_path(repo)?);
        let _: serde_json::Value = self.api.put(&path, &body).await?;
        // The branch is removed by GitLab as part of the merge (`should_remove_source_branch`).
        Ok(MergeOutcome::default())
    }

    async fn list_review_comments(&self, repo: &RepoRef, number: u32) -> Result<Vec<ReviewComment>> {
        let path = format!("{}/merge_requests/{number}/discussions", project_path(repo)?);
        let mut comments = Vec::new();
        for page in 1..=10u32 {
            let (batch, headers) = self.api.get::<Vec<GlDiscussion>>(&path, &[("per_page", SCAN_PER_PAGE.to_string()), ("page", page.to_string())]).await?;
            let more = next_page_from_gitlab(&headers).is_some();
            for discussion in batch {
                for (index, note) in discussion.notes.into_iter().filter(|note| !note.system).enumerate() {
                    // Only comments on lines of the changes; general comments are not line comments.
                    let Some(position) = note.position else { continue };
                    let (path, line, side) = match (position.new_line, position.old_line) {
                        (Some(line), _) => (position.new_path.unwrap_or_default(), Some(line), ReviewSide::Right),
                        (None, Some(line)) => (position.old_path.unwrap_or_default(), Some(line), ReviewSide::Left),
                        (None, None) => (position.new_path.unwrap_or_default(), None, ReviewSide::Right),
                    };
                    comments.push(ReviewComment {
                        id: note.id.to_string(),
                        thread: discussion.id.clone(),
                        path,
                        line,
                        side,
                        body: note.body,
                        author: note.author.map(|a| a.username).unwrap_or_default(),
                        created_at: timestamp_or_zero(note.created_at.as_deref()),
                        is_reply: index > 0,
                    });
                }
            }
            if !more {
                break;
            }
        }
        Ok(comments)
    }

    async fn submit_review(&self, repo: &RepoRef, number: u32, input: &ReviewInput) -> Result<()> {
        if input.event != ReviewEvent::Approve && input.body.trim().is_empty() && input.comments.is_empty() {
            return Err(ForgeError::invalid("Write a comment first"));
        }
        let mr_path = format!("{}/merge_requests/{number}", project_path(repo)?);
        if !input.comments.is_empty() {
            // Line comments are placed with the MR's diff refs.
            let (mr, _) = self.api.get::<GlMergeRequest>(&mr_path, &[]).await?;
            let refs = mr.diff_refs.ok_or_else(|| ForgeError::invalid_response("The merge request has no diff yet"))?;
            for comment in &input.comments {
                let mut position = serde_json::json!({
                    "position_type": "text",
                    "base_sha": refs.base_sha,
                    "start_sha": refs.start_sha,
                    "head_sha": refs.head_sha,
                    "new_path": comment.path,
                    "old_path": comment.path,
                });
                let key = if comment.side == ReviewSide::Left { "old_line" } else { "new_line" };
                position[key] = serde_json::json!(comment.line);
                let _: serde_json::Value = self.api.post(&format!("{mr_path}/discussions"), &serde_json::json!({ "body": comment.body, "position": position })).await?;
            }
        }
        if !input.body.trim().is_empty() {
            let _: serde_json::Value = self.api.post(&format!("{mr_path}/notes"), &serde_json::json!({ "body": input.body })).await?;
        }
        if input.event == ReviewEvent::Approve {
            let _: serde_json::Value = self.api.post(&format!("{mr_path}/approve"), &serde_json::json!({})).await?;
        }
        Ok(())
    }

    async fn reply_to_review_comment(&self, repo: &RepoRef, number: u32, thread: &str, body: &str) -> Result<()> {
        if body.trim().is_empty() {
            return Err(ForgeError::invalid("Write a reply first"));
        }
        let path = format!("{}/merge_requests/{number}/discussions/{}/notes", project_path(repo)?, encode_segment(thread));
        let _: serde_json::Value = self.api.post(&path, &serde_json::json!({ "body": body })).await?;
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct GlDiscussion {
    id: String,
    #[serde(default)]
    notes: Vec<GlNote>,
}

#[derive(Debug, Deserialize)]
struct GlNote {
    id: u64,
    #[serde(default)]
    body: String,
    #[serde(default)]
    author: Option<GlActor>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    system: bool,
    #[serde(default)]
    position: Option<GlPosition>,
}

#[derive(Debug, Deserialize)]
struct GlPosition {
    #[serde(default)]
    new_path: Option<String>,
    #[serde(default)]
    old_path: Option<String>,
    #[serde(default)]
    new_line: Option<u32>,
    #[serde(default)]
    old_line: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct GlDiffRefs {
    base_sha: String,
    start_sha: String,
    head_sha: String,
}

#[derive(Debug, Serialize)]
struct GlMerge<'a> {
    squash: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha: Option<&'a str>,
    should_remove_source_branch: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    merge_commit_message: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    squash_commit_message: Option<&'a str>,
}

/// GitLab merges with the project's merge method; squashing is allowed unless the project forbids it.
fn gitlab_merge_methods(project: Option<&GlProject>) -> Vec<MergeMethod> {
    match project.and_then(|p| p.squash_option.as_deref()) {
        Some("never") => vec![MergeMethod::Merge],
        Some("always") => vec![MergeMethod::Squash],
        _ => vec![MergeMethod::Merge, MergeMethod::Squash],
    }
}

/// GitLab's `detailed_merge_status` (or the older `merge_status`).
fn gitlab_merge_state(mr: &GlMergeRequest) -> MergeState {
    merge_state_from(mr.has_conflicts, mr.detailed_merge_status.as_deref(), mr.merge_status.as_deref())
}

fn merge_state_from(has_conflicts: Option<bool>, detailed: Option<&str>, merge_status: Option<&str>) -> MergeState {
    if has_conflicts == Some(true) {
        return MergeState::Conflicts;
    }
    match detailed {
        Some("mergeable") => MergeState::Clean,
        Some("conflict") | Some("broken_status") => MergeState::Conflicts,
        Some("need_rebase") => MergeState::Behind,
        Some("draft_status") => MergeState::Draft,
        Some("checking") | Some("unchecked") | Some("preparing") | Some("approvals_syncing") => MergeState::Checking,
        // Everything else GitLab refuses to merge, including `ci_must_pass` and `ci_still_running` (the
        // project requires a successful pipeline first). A failing pipeline that is not required leaves
        // the status `mergeable`.
        Some(_) => MergeState::Blocked,
        None => match merge_status {
            Some("can_be_merged") => MergeState::Clean,
            Some("cannot_be_merged") => MergeState::Conflicts,
            _ => MergeState::Checking,
        },
    }
}

/// `/projects/{id}` with the project path encoded (`group%2Fproject`).
fn project_path(repo: &RepoRef) -> Result<String> {
    if repo.owner.trim().is_empty() || repo.name.trim().is_empty() {
        return Err(ForgeError::invalid("Repository owner and name must not be empty"));
    }
    Ok(format!("/projects/{}", encode_segment(&repo.full_name())))
}

/// GitLab has no draft parameter when creating an MR; the draft status is determined by
/// the title prefix.
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

/// Maps a job or pipeline status to a [`CheckState`].
fn job_state(status: &str, allow_failure: bool) -> CheckState {
    match status {
        "success" => CheckState::Success,
        // A job that is allowed to fail shows up as a warning, not a failure.
        "failed" if allow_failure => CheckState::Neutral,
        "failed" => CheckState::Failure,
        "running" | "canceling" => CheckState::Running,
        "canceled" | "cancelled" => CheckState::Cancelled,
        "skipped" => CheckState::Skipped,
        // Waiting to be run manually.
        "manual" => CheckState::Neutral,
        // `created`, `pending`, `waiting_for_resource`, `preparing`, `scheduled`, and
        // new statuses not yet recognized.
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
    #[serde(default)]
    squash_option: Option<String>,
}

impl From<GlProject> for RemoteRepo {
    fn from(project: GlProject) -> Self {
        let updated = project.last_activity_at.as_deref().or(project.updated_at.as_deref());
        RemoteRepo {
            updated_at: timestamp_or_zero(updated),
            full_name: project.path_with_namespace,
            description: project.description.unwrap_or_default(),
            // `internal` is visible to all logged-in users, but is not public.
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
    detailed_merge_status: Option<String>,
    #[serde(default)]
    diff_refs: Option<GlDiffRefs>,
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
        let repo = RepoRef { host: "gitlab.com".into(), owner: "group/subgroup".into(), name: "project".into() };
        assert_eq!(project_path(&repo).expect("path"), "/projects/group%2Fsubgroup%2Fproject");
        let empty = RepoRef { host: "gitlab.com".into(), owner: String::new(), name: "project".into() };
        assert!(project_path(&empty).is_err());
    }

    #[test]
    fn merge_states() {
        assert_eq!(merge_state_from(None, Some("mergeable"), None), MergeState::Clean);
        assert_eq!(merge_state_from(Some(true), Some("mergeable"), None), MergeState::Conflicts);
        assert_eq!(merge_state_from(None, Some("need_rebase"), None), MergeState::Behind);
        assert_eq!(merge_state_from(None, Some("checking"), None), MergeState::Checking);
        // A required pipeline that has not passed yet: GitLab refuses the merge.
        assert_eq!(merge_state_from(None, Some("ci_must_pass"), None), MergeState::Blocked);
        assert_eq!(merge_state_from(None, Some("ci_still_running"), None), MergeState::Blocked);
        assert_eq!(merge_state_from(None, Some("not_approved"), None), MergeState::Blocked);
        assert_eq!(merge_state_from(None, None, Some("can_be_merged")), MergeState::Clean);
        assert_eq!(merge_state_from(None, None, None), MergeState::Checking);
    }

    #[test]
    fn draft_title_prefix() {
        assert_eq!(merge_request_title("Add feature", true), "Draft: Add feature");
        assert_eq!(merge_request_title("Add feature", false), "Add feature");
        assert_eq!(merge_request_title("Draft: Add feature", true), "Draft: Add feature");
        assert_eq!(merge_request_title("draft: add feature", true), "draft: add feature");
        assert_eq!(merge_request_title("[Draft] Add feature", true), "[Draft] Add feature");
        assert_eq!(merge_request_title("(draft) Add feature", true), "(draft) Add feature");
        // "Drafting" is not a draft prefix.
        assert_eq!(merge_request_title("Drafting again", true), "Draft: Drafting again");
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
