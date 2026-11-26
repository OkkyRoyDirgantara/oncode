//! Data structures sent to the UI. All fields use camelCase on the TypeScript side.

use serde::{Deserialize, Serialize};

pub use crate::remote::parse_remote_url;

/// Seconds since the Unix epoch. `f64` so it is safe to export as a `number` in TypeScript.
pub type Timestamp = f64;

/// The kind of Git hosting provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ForgeKind {
    #[serde(rename = "github")]
    GitHub,
    #[serde(rename = "gitlab")]
    GitLab,
}

impl ForgeKind {
    /// A stable short name: `"github"` or `"gitlab"`. Used in the account id.
    pub fn as_str(self) -> &'static str {
        match self {
            ForgeKind::GitHub => "github",
            ForgeKind::GitLab => "gitlab",
        }
    }
}

/// One forge instance. `host` is the web origin WITHOUT a trailing slash, for example
/// `"https://github.com"`, `"https://gitlab.com"`, or `"https://git.example.com"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeHost {
    pub kind: ForgeKind,
    pub host: String,
}

impl ForgeHost {
    /// Creates a host from user input: whitespace and trailing slashes are dropped, and
    /// `https://` is added if no scheme is given.
    pub fn new(kind: ForgeKind, host: &str) -> Self {
        let trimmed = host.trim().trim_end_matches('/');
        let host = if trimmed.contains("://") { trimmed.to_string() } else { format!("https://{trimmed}") };
        Self { kind, host }
    }

    /// github.com.
    pub fn github() -> Self {
        Self { kind: ForgeKind::GitHub, host: "https://github.com".to_string() }
    }

    /// gitlab.com.
    pub fn gitlab() -> Self {
        Self { kind: ForgeKind::GitLab, host: "https://gitlab.com".to_string() }
    }

    /// The web origin without a trailing slash.
    pub fn web_base(&self) -> &str {
        self.host.trim().trim_end_matches('/')
    }

    /// The hostname only, lowercase, without scheme, port, or path. It has the same form as
    /// [`RepoRef::host`], so the two can be compared directly.
    pub fn hostname(&self) -> String {
        let rest = match self.host.split_once("://") {
            Some((_, rest)) => rest,
            None => self.host.as_str(),
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let authority = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
        let host = if authority.starts_with('[') {
            match authority.find(']') {
                Some(end) => &authority[..=end],
                None => authority,
            }
        } else {
            authority.split(':').next().unwrap_or_default()
        };
        host.trim().to_ascii_lowercase()
    }

    /// REST base: github.com → `https://api.github.com`; GitHub Enterprise Cloud with
    /// data residency (`*.ghe.com`) → `https://api.{host}`; GitHub Enterprise Server →
    /// `{host}/api/v3`; GitLab → `{host}/api/v4`.
    pub fn api_base(&self) -> String {
        let base = self.web_base();
        match self.kind {
            ForgeKind::GitHub => {
                let hostname = self.hostname();
                if hostname == "github.com" || hostname == "www.github.com" {
                    "https://api.github.com".to_string()
                } else if hostname.ends_with(".ghe.com") {
                    format!("https://api.{hostname}")
                } else {
                    format!("{base}/api/v3")
                }
            }
            ForgeKind::GitLab => format!("{base}/api/v4"),
        }
    }

    /// `true` if `repo` is on this host (hostname comparison, without port).
    pub fn matches(&self, repo: &RepoRef) -> bool {
        self.hostname() == repo.host.to_ascii_lowercase()
    }

    /// `true` for public services (github.com, gitlab.com), not self-run instances.
    pub fn is_public(&self) -> bool {
        let hostname = self.hostname();
        match self.kind {
            ForgeKind::GitHub => hostname == "github.com" || hostname == "www.github.com",
            ForgeKind::GitLab => hostname == "gitlab.com" || hostname == "www.gitlab.com",
        }
    }

    /// Web page for creating a Personal Access Token, with the name and scopes OnCode
    /// needs already filled in. A browser login route that does not require an OAuth app.
    pub fn token_creation_url(&self) -> String {
        let base = self.web_base();
        match self.kind {
            ForgeKind::GitHub => format!("{base}/settings/tokens/new?description=OnCode&scopes=repo,read:user"),
            ForgeKind::GitLab => format!("{base}/-/user_settings/personal_access_tokens?name=OnCode&scopes=api,read_user"),
        }
    }
}

/// A repository on a forge, the result of parsing a remote URL.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoRef {
    /// The hostname only, lowercase, for example `"github.com"`.
    pub host: String,
    /// Owner or namespace. May contain `/` for GitLab subgroups.
    pub owner: String,
    /// Repository name without the `.git` suffix.
    pub name: String,
}

impl RepoRef {
    /// `"owner/name"`, the form both providers use as the full name.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

/// An account logged in to a forge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// `"{kind}:{hostname}:{username}"`, for example `"github:github.com:octocat"`. Also used
    /// as the token key in the keychain.
    pub id: String,
    pub kind: ForgeKind,
    /// The instance's web origin, same as [`ForgeHost::host`].
    pub host: String,
    pub username: String,
    /// Display name; same as `username` if the user did not set one.
    pub display_name: String,
    pub avatar_url: Option<String>,
}

impl Account {
    /// Builds the account id from its parts.
    pub fn make_id(host: &ForgeHost, username: &str) -> String {
        format!("{}:{}:{}", host.kind.as_str(), host.hostname(), username)
    }

    /// The forge host this account lives on.
    pub fn forge_host(&self) -> ForgeHost {
        ForgeHost { kind: self.kind, host: self.host.clone() }
    }
}

/// A repository on the user's account, for the clone dialog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteRepo {
    /// `"owner/name"`; on GitLab it contains the whole subgroup path.
    pub full_name: String,
    pub description: String,
    /// On GitLab, `internal` visibility also counts as private.
    pub private: bool,
    pub fork: bool,
    pub clone_url_https: String,
    pub clone_url_ssh: String,
    /// Empty if the repository has no commits yet.
    pub default_branch: String,
    pub updated_at: Timestamp,
    pub web_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

/// A pull request (GitHub) or merge request (GitLab).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    /// The PR number on GitHub, the MR `iid` on GitLab.
    pub number: u32,
    pub title: String,
    pub state: PrState,
    pub draft: bool,
    pub author: String,
    pub author_avatar: Option<String>,
    pub source_branch: String,
    pub target_branch: String,
    pub head_sha: String,
    pub updated_at: Timestamp,
    pub web_url: String,
    /// `true` if the source branch is in another repository (a fork).
    pub is_cross_repo: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestDetail {
    pub pull_request: PullRequest,
    /// Description in raw Markdown; the UI renders and sanitizes it.
    pub body: String,
    pub reviewers: Vec<String>,
    pub labels: Vec<String>,
    /// `None` while the provider is still computing whether it can be merged.
    pub mergeable: Option<bool>,
    pub commits: u32,
    pub changed_files: u32,
    /// GitLab does not provide line counts via REST, so the value is always 0 there.
    pub additions: u32,
    /// See the note on `additions`.
    pub deletions: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckState {
    Pending,
    Running,
    Success,
    Failure,
    Cancelled,
    Skipped,
    Neutral,
}

/// A single CI check on a commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub name: String,
    pub state: CheckState,
    pub url: Option<String>,
}

/// Filter for the PR/MR list in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PrFilter {
    /// Everything that is still open.
    Open,
    /// Still open and created by this account.
    Mine,
    /// Still open and awaiting review from this account.
    ReviewRequested,
    /// Open, closed, and already merged.
    All,
}

/// Input for creating a PR/MR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewPullRequest {
    pub title: String,
    pub body: String,
    /// On GitHub, a PR from a fork is written `"owner:branch"`.
    pub source_branch: String,
    pub target_branch: String,
    pub draft: bool,
}

/// One page of results. `next_page` is set if there is a next page; its value
/// is sent back as-is as the `page` argument.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_page: Option<u32>,
}

impl<T> Page<T> {
    pub fn new(items: Vec<T>, next_page: Option<u32>) -> Self {
        Self { items, next_page }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_base_per_host() {
        assert_eq!(ForgeHost::github().api_base(), "https://api.github.com");
        assert_eq!(ForgeHost::new(ForgeKind::GitHub, "https://GitHub.com/").api_base(), "https://api.github.com");
        assert_eq!(
            ForgeHost::new(ForgeKind::GitHub, "https://ghe.example.com").api_base(),
            "https://ghe.example.com/api/v3"
        );
        assert_eq!(
            ForgeHost::new(ForgeKind::GitHub, "https://octocorp.ghe.com").api_base(),
            "https://api.octocorp.ghe.com"
        );
        assert_eq!(ForgeHost::gitlab().api_base(), "https://gitlab.com/api/v4");
        assert_eq!(
            ForgeHost::new(ForgeKind::GitLab, "https://git.example.com/").api_base(),
            "https://git.example.com/api/v4"
        );
        // A GitLab instance under a sub-path is kept intact.
        assert_eq!(
            ForgeHost::new(ForgeKind::GitLab, "https://example.com/gitlab").api_base(),
            "https://example.com/gitlab/api/v4"
        );
    }

    #[test]
    fn host_normalization() {
        let host = ForgeHost::new(ForgeKind::GitLab, "  git.example.com/ ");
        assert_eq!(host.host, "https://git.example.com");
        assert_eq!(host.hostname(), "git.example.com");
        assert_eq!(ForgeHost::new(ForgeKind::GitLab, "http://Git.Local:8080/").hostname(), "git.local");
        assert_eq!(ForgeHost::github().hostname(), "github.com");
        assert_eq!(ForgeHost::new(ForgeKind::GitLab, "https://[::1]:8443").hostname(), "[::1]");
    }

    #[test]
    fn token_creation_url_prefills_name_and_scopes() {
        assert_eq!(ForgeHost::github().token_creation_url(), "https://github.com/settings/tokens/new?description=OnCode&scopes=repo,read:user");
        assert_eq!(
            ForgeHost::new(ForgeKind::GitLab, "git.example.com/").token_creation_url(),
            "https://git.example.com/-/user_settings/personal_access_tokens?name=OnCode&scopes=api,read_user"
        );
        // An instance under a sub-path keeps using its base path.
        assert_eq!(
            ForgeHost::new(ForgeKind::GitLab, "https://example.com/gitlab").token_creation_url(),
            "https://example.com/gitlab/-/user_settings/personal_access_tokens?name=OnCode&scopes=api,read_user"
        );
    }

    #[test]
    fn only_github_com_and_gitlab_com_are_public() {
        assert!(ForgeHost::github().is_public());
        assert!(ForgeHost::gitlab().is_public());
        assert!(!ForgeHost::new(ForgeKind::GitLab, "git.example.com").is_public());
        assert!(!ForgeHost::new(ForgeKind::GitHub, "ghe.example.com").is_public());
        // A kind that does not match its host is not a public service.
        assert!(!ForgeHost::new(ForgeKind::GitLab, "github.com").is_public());
    }

    #[test]
    fn host_matches_repo() {
        let repo = RepoRef { host: "github.com".into(), owner: "octo".into(), name: "hello".into() };
        assert!(ForgeHost::github().matches(&repo));
        assert!(!ForgeHost::gitlab().matches(&repo));
        assert_eq!(repo.full_name(), "octo/hello");
    }

    #[test]
    fn account_id_format() {
        assert_eq!(Account::make_id(&ForgeHost::github(), "octocat"), "github:github.com:octocat");
        assert_eq!(
            Account::make_id(&ForgeHost::new(ForgeKind::GitLab, "https://Git.Example.com:8443"), "alice"),
            "gitlab:git.example.com:alice"
        );
    }

    #[test]
    fn serde_names() {
        assert_eq!(serde_json::to_value(ForgeKind::GitHub).expect("json"), serde_json::json!("github"));
        assert_eq!(serde_json::to_value(ForgeKind::GitLab).expect("json"), serde_json::json!("gitlab"));
        assert_eq!(
            serde_json::from_value::<PrFilter>(serde_json::json!("reviewRequested")).expect("json"),
            PrFilter::ReviewRequested
        );
        let input: NewPullRequest = serde_json::from_value(serde_json::json!({
            "title": "Title", "body": "", "sourceBranch": "feature", "targetBranch": "main", "draft": true
        }))
        .expect("json");
        assert_eq!(input.source_branch, "feature");
        let page = Page::new(vec![1u32, 2], Some(2));
        assert_eq!(serde_json::to_value(&page).expect("json"), serde_json::json!({ "items": [1, 2], "nextPage": 2 }));
    }
}
