//! One trait for all providers, plus helpers that depend on the provider kind.

use crate::error::{ForgeError, Result};
use crate::github::GitHubProvider;
use crate::gitlab::GitLabProvider;
use crate::model::{
    Account, Check, ForgeHost, ForgeKind, NewPullRequest, Page, PrFilter, PullRequest, PullRequestDetail, RemoteRepo,
    RepoRef,
};

/// Operations supported by all providers. Page numbers start at 1 (0 is treated as 1);
/// for the next page, send `Page::next_page` back as-is.
#[async_trait::async_trait]
pub trait ForgeProvider: Send + Sync {
    /// The account that owns the token. Also used to validate the token (including PATs) at login.
    async fn current_user(&self) -> Result<Account>;

    /// Repositories this account can access, most recently updated first.
    /// `query` filters by name; an empty string means no filter.
    ///
    /// GitLab searches on the server side. GitHub has no server-side search for
    /// "repositories I can access", so there the list is scanned on the client side:
    /// a single call may return more or fewer than 30 items (even zero)
    /// while `next_page` is still set.
    async fn list_repos(&self, query: &str, page: u32) -> Result<Page<RemoteRepo>>;

    /// PRs/MRs in `repo`, most recently updated first.
    async fn list_pull_requests(&self, repo: &RepoRef, filter: PrFilter, page: u32) -> Result<Page<PullRequest>>;

    /// Details of a single PR/MR.
    async fn get_pull_request(&self, repo: &RepoRef, number: u32) -> Result<PullRequestDetail>;

    /// CI checks for commit `sha`. GitHub: check runs combined with commit statuses.
    /// GitLab: jobs from the latest pipeline for that commit, or the pipeline's status if
    /// there are no jobs yet. Empty if the commit has no CI.
    async fn list_checks(&self, repo: &RepoRef, sha: &str) -> Result<Vec<Check>>;

    /// Creates a PR/MR. The source branch must already exist on the remote.
    async fn create_pull_request(&self, repo: &RepoRef, input: &NewPullRequest) -> Result<PullRequest>;

    /// The repository's default branch (to pre-fill the target branch).
    async fn default_branch(&self, repo: &RepoRef) -> Result<String>;
}

/// The Git ref to fetch for checking out PR/MR `number`: GitHub `refs/pull/{n}/head`,
/// GitLab `refs/merge-requests/{n}/head`.
pub fn pull_request_ref(kind: ForgeKind, number: u32) -> String {
    match kind {
        ForgeKind::GitHub => format!("refs/pull/{number}/head"),
        ForgeKind::GitLab => format!("refs/merge-requests/{number}/head"),
    }
}

/// The username/password pair git uses over HTTPS for this host with an OAuth token
/// or PAT: GitHub `("x-access-token", token)`, GitLab `("oauth2", token)`.
///
/// The second value is a secret: do not log it and do not pass it as a command argument.
pub fn git_credentials(kind: ForgeKind, token: &str) -> (String, String) {
    let username = match kind {
        ForgeKind::GitHub => "x-access-token",
        ForgeKind::GitLab => "oauth2",
    };
    (username.to_string(), token.to_string())
}

/// Creates a provider for `host` with `token`. `api_base_override` replaces the REST
/// base, so tests can point at a mock server.
pub fn provider(host: &ForgeHost, token: &str, api_base_override: Option<&str>) -> Box<dyn ForgeProvider + Send + Sync> {
    match host.kind {
        ForgeKind::GitHub => Box::new(GitHubProvider::new(host, token, api_base_override)),
        ForgeKind::GitLab => Box::new(GitLabProvider::new(host, token, api_base_override)),
    }
}

/// Validates PR/MR input before it is sent to the server.
pub(crate) fn validate_new_pull_request(input: &NewPullRequest) -> Result<()> {
    if input.title.trim().is_empty() {
        return Err(ForgeError::invalid("Title must not be empty"));
    }
    if input.source_branch.trim().is_empty() {
        return Err(ForgeError::invalid("Source branch must not be empty"));
    }
    if input.target_branch.trim().is_empty() {
        return Err(ForgeError::invalid("Target branch must not be empty"));
    }
    if input.source_branch.trim() == input.target_branch.trim() {
        return Err(ForgeError::invalid("Source and target branches must be different"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ForgeErrorCode;

    #[test]
    fn validates_new_pull_request() {
        let valid = NewPullRequest {
            title: "Title".into(),
            body: String::new(),
            source_branch: "feature".into(),
            target_branch: "main".into(),
            draft: false,
        };
        assert!(validate_new_pull_request(&valid).is_ok());
        for broken in [
            NewPullRequest { title: "  ".into(), ..valid.clone() },
            NewPullRequest { source_branch: String::new(), ..valid.clone() },
            NewPullRequest { target_branch: String::new(), ..valid.clone() },
            NewPullRequest { source_branch: "main".into(), ..valid.clone() },
        ] {
            let err = validate_new_pull_request(&broken).expect_err("must be rejected");
            assert_eq!(err.code, ForgeErrorCode::InvalidInput);
        }
    }

    #[test]
    fn pull_request_refs() {
        assert_eq!(pull_request_ref(ForgeKind::GitHub, 42), "refs/pull/42/head");
        assert_eq!(pull_request_ref(ForgeKind::GitLab, 7), "refs/merge-requests/7/head");
    }

    #[test]
    fn git_credentials_per_kind() {
        assert_eq!(
            git_credentials(ForgeKind::GitHub, "gho_abc"),
            ("x-access-token".to_string(), "gho_abc".to_string())
        );
        assert_eq!(git_credentials(ForgeKind::GitLab, "glpat-abc"), ("oauth2".to_string(), "glpat-abc".to_string()));
    }

    #[test]
    fn provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>(_: &T) {}
        let github = provider(&ForgeHost::github(), "token", None);
        let gitlab = provider(&ForgeHost::gitlab(), "token", None);
        assert_send_sync(&github);
        assert_send_sync(&gitlab);
    }
}
