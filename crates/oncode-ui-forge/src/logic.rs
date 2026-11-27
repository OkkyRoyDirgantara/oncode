//! Pure logic behind the forge views: the PR tab, the create-PR dialog, and the CI summary.

use oncode_core::api::forge::ForgeContext;
use oncode_forge::{Check, CheckState, ForgeKind, MergeState, PrFilter, PrState, PullRequest, RepoRef};
use oncode_git::{ChangeKind, Refs, Upstream};
use oncode_ui::TabData;
use serde::{Deserialize, Serialize};

use crate::util::{pr_noun, pr_number, shorten};

/// Kind of the PR/MR detail tab (same as v0.2).
pub const PR_TAB_KIND: &str = "pullRequest";

/// PR list filters with their labels, in display order.
pub const FILTERS: [(PrFilter, &str, &str); 4] =
    [(PrFilter::Open, "open", "Open"), (PrFilter::Mine, "mine", "Created by me"), (PrFilter::ReviewRequested, "reviewRequested", "Review requested"), (PrFilter::All, "all", "All")];

/// Parameters of the PR/MR detail tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrTabParams {
    pub root: String,
    pub account_id: String,
    pub repo: RepoRef,
    pub kind: ForgeKind,
    pub remote: String,
    pub number: u32,
}

/// The detail tab for a PR/MR; `None` if there is no account for that host yet.
pub fn pr_tab(root: &str, context: &ForgeContext, number: u32, title: &str) -> Option<TabData> {
    let account = context.account.as_ref()?;
    let kind = context.kind.unwrap_or(account.kind);
    let params = PrTabParams { root: root.to_string(), account_id: account.id.clone(), repo: context.repo.clone(), kind, remote: context.remote.clone(), number };
    let label = pr_number(Some(kind), number);
    Some(
        TabData::new(PR_TAB_KIND, format!("{label} {}", shorten(title, 40)), serde_json::to_value(&params).ok()?)
            .with_id(format!("pullRequest:{}:{}/{}/{}:{number}", account.id, context.repo.host, context.repo.owner, context.repo.name))
            .with_description(format!("{} {label} · {}/{}\n{title}", pr_noun(Some(kind)), context.repo.owner, context.repo.name)),
    )
}

/// PR state for the icon and badge: draft only applies while it is still open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrBadge {
    Open,
    Draft,
    Merged,
    Closed,
}

impl PrBadge {
    pub fn of(pr: &PullRequest) -> Self {
        match pr.state {
            PrState::Merged => Self::Merged,
            PrState::Closed => Self::Closed,
            PrState::Open if pr.draft => Self::Draft,
            PrState::Open => Self::Open,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Draft => "Draft",
            Self::Merged => "Merged",
            Self::Closed => "Closed",
        }
    }
}

pub fn check_label(state: CheckState) -> &'static str {
    match state {
        CheckState::Pending => "Pending",
        CheckState::Running => "Running",
        CheckState::Success => "Passed",
        CheckState::Failure => "Failed",
        CheckState::Cancelled => "Cancelled",
        CheckState::Skipped => "Skipped",
        CheckState::Neutral => "Neutral",
    }
}

/// Summary of CI checks, e.g. "3 passed, 1 failed".
pub fn summarize_checks(checks: &[Check]) -> String {
    let count = |states: &[CheckState]| checks.iter().filter(|c| states.contains(&c.state)).count();
    let failed = count(&[CheckState::Failure]);
    let running = count(&[CheckState::Pending, CheckState::Running]);
    let passed = count(&[CheckState::Success]);
    let other = checks.len() - failed - running - passed;
    let mut parts = Vec::new();
    if failed > 0 {
        parts.push(format!("{failed} failed"));
    }
    if running > 0 {
        parts.push(format!("{running} in progress"));
    }
    if passed > 0 {
        parts.push(format!("{passed} passed"));
    }
    if other > 0 {
        parts.push(format!("{other} skipped or cancelled"));
    }
    parts.join(", ")
}

/// Merge state sentence for a PR that is still open.
/// Whether Merge is offered: not when the forge has said it would refuse (conflicts, a draft, unmet
/// rules or reviews, a source branch that must be updated first), nor while it is still checking.
/// `Unknown` is left to the forge to decide. Failing checks (`Unstable`) do not stop a merge.
pub fn can_try_merge(state: MergeState) -> bool {
    matches!(state, MergeState::Clean | MergeState::Unstable | MergeState::Unknown)
}

pub fn mergeable_text(mergeable: Option<bool>) -> &'static str {
    match mergeable {
        Some(true) => "No conflicts with the target branch",
        Some(false) => "Conflicts with the target branch",
        None => "Mergeability not known yet",
    }
}

pub fn change_letter(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Added => "A",
        ChangeKind::Modified => "M",
        ChangeKind::Deleted => "D",
        ChangeKind::Renamed => "R",
        ChangeKind::Copied => "C",
        ChangeKind::TypeChanged => "T",
    }
}

/// "1 commit" / "2 commits".
pub fn plural(count: u32, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}

/// Comparison ref for changed files: `remote/target` if it exists, otherwise the local
/// branch `target`.
pub fn base_ref_for(remote: &str, target: &str, refs: Option<&Refs>) -> Option<String> {
    let refs = refs?;
    let tracking = format!("{remote}/{target}");
    if refs.remote_branches.iter().any(|b| b.name == tracking) {
        return Some(tracking);
    }
    refs.branches.iter().any(|b| b.name == target).then(|| target.to_string())
}

/// What must be done before the PR can be created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushNeed {
    None,
    /// The branch does not exist on the remote yet.
    Publish,
    /// There are local commits that have not been pushed.
    Push { ahead: u32 },
}

pub fn push_need(upstream: Option<&Upstream>) -> PushNeed {
    match upstream {
        Some(upstream) if upstream.exists => {
            if upstream.ahead > 0 {
                PushNeed::Push { ahead: upstream.ahead }
            } else {
                PushNeed::None
            }
        }
        _ => PushNeed::Publish,
    }
}

impl PushNeed {
    /// Notice in the dialog about the push that will be done first.
    pub fn notice(self, remote: &str) -> Option<String> {
        match self {
            Self::None => None,
            Self::Publish => Some(format!("This branch has not been published yet. It will be pushed to {remote} first.")),
            Self::Push { ahead: 1 } => Some("This branch has 1 commit that is not on the remote yet. It will be pushed first.".to_string()),
            Self::Push { ahead } => Some(format!("This branch has {ahead} commits that are not on the remote yet. They will be pushed first.")),
        }
    }

    /// Label of the submit button.
    pub fn submit_label(self, noun: &str) -> String {
        match self {
            Self::None => format!("Create {noun}"),
            Self::Publish => format!("Publish & Create {noun}"),
            Self::Push { .. } => format!("Push & Create {noun}"),
        }
    }
}

/// Branch name on the remote: the upstream name without the remote prefix, or the local branch
/// name if it has not been published.
pub fn source_branch_name(local_branch: &str, upstream: Option<&Upstream>, refs: Option<&Refs>) -> String {
    let Some(upstream) = upstream.filter(|u| u.exists) else { return local_branch.to_string() };
    // Longest remote name first, so `origin-fork/x` is not cut as `origin`.
    let mut remotes: Vec<&str> = refs.map(|r| r.remotes.iter().map(|remote| remote.name.as_str()).collect()).unwrap_or_default();
    remotes.sort_by_key(|name| std::cmp::Reverse(name.len()));
    for remote in remotes {
        if let Some(rest) = upstream.name.strip_prefix(remote).and_then(|rest| rest.strip_prefix('/')) {
            return rest.to_string();
        }
    }
    match upstream.name.split_once('/') {
        Some((_, rest)) => rest.to_string(),
        None => upstream.name.clone(),
    }
}

/// Target branches that can be chosen: the branches of `remote`, without the remote prefix and
/// without `HEAD`, plus `extra` (the default branch from the forge).
pub fn target_branch_options(refs: Option<&Refs>, remote: &str, extra: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut add = |name: &str| {
        if !name.is_empty() && name != "HEAD" && !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
    };
    for branch in refs.map(|r| r.remote_branches.as_slice()).unwrap_or_default() {
        if branch.remote != remote {
            continue;
        }
        add(branch.name.strip_prefix(remote).and_then(|rest| rest.strip_prefix('/')).unwrap_or(&branch.name));
    }
    for name in extra {
        add(name);
    }
    names.sort_by_key(|name| name.to_lowercase());
    names
}

/// The effective target branch: the user's choice, or the default branch if it differs from the source.
/// Without a known default branch, the target must be chosen explicitly; no silent guessing.
pub fn effective_target(chosen: Option<&str>, default_branch: Option<&str>, source: &str) -> String {
    match chosen {
        Some(chosen) => chosen.to_string(),
        None => default_branch.filter(|name| !name.is_empty() && *name != source).unwrap_or_default().to_string(),
    }
}

/// Why the PR form cannot be submitted yet. `default_loaded`: the forge has answered with the default branch.
pub fn create_problem(title: &str, target: &str, source: &str, default_loaded: bool, target_chosen: bool) -> Option<&'static str> {
    if title.trim().is_empty() {
        return Some("Enter a title.");
    }
    if !default_loaded && !target_chosen {
        return None;
    }
    if target.is_empty() {
        return Some("Choose a target branch.");
    }
    if target == source {
        return Some("The source and target branch are the same.");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use oncode_forge::Account;
    use oncode_git::{Branch, Remote, RemoteBranch};

    #[test]
    fn merge_is_offered_only_when_the_forge_may_accept_it() {
        let offered: Vec<MergeState> = [MergeState::Clean, MergeState::Blocked, MergeState::Behind, MergeState::Conflicts, MergeState::Unstable, MergeState::Draft, MergeState::Checking, MergeState::Unknown]
            .into_iter()
            .filter(|state| can_try_merge(*state))
            .collect();
        assert_eq!(offered, [MergeState::Clean, MergeState::Unstable, MergeState::Unknown]);
    }

    fn refs() -> Refs {
        let branch = |name: &str| Branch {
            name: name.into(),
            oid: "1".repeat(40),
            is_head: false,
            upstream: None,
            ahead: 0,
            behind: 0,
            upstream_gone: false,
            worktree: None,
            subject: "Add login form".into(),
            time: 0.0,
        };
        let remote_branch = |remote: &str, name: &str| RemoteBranch { name: format!("{remote}/{name}"), remote: remote.into(), oid: "2".repeat(40) };
        let remote = |name: &str| Remote { name: name.into(), fetch_url: format!("https://github.com/{name}/widgets.git"), push_url: String::new() };
        Refs {
            branches: vec![branch("main"), branch("feature/login")],
            remote_branches: vec![remote_branch("origin", "HEAD"), remote_branch("origin", "main"), remote_branch("origin", "develop"), remote_branch("fork", "main"), remote_branch("fork", "topic")],
            tags: Vec::new(),
            remotes: vec![remote("origin"), remote("fork"), remote("origin-fork")],
        }
    }

    fn upstream(name: &str, ahead: u32, exists: bool) -> Upstream {
        Upstream { name: name.into(), ahead, behind: 0, exists }
    }

    fn check(name: &str, state: CheckState) -> Check {
        Check { name: name.into(), state, url: None }
    }

    #[test]
    fn push_need_depends_on_the_upstream() {
        assert_eq!(push_need(None), PushNeed::Publish);
        assert_eq!(push_need(Some(&upstream("origin/x", 0, false))), PushNeed::Publish);
        assert_eq!(push_need(Some(&upstream("origin/x", 2, true))), PushNeed::Push { ahead: 2 });
        assert_eq!(push_need(Some(&upstream("origin/x", 0, true))), PushNeed::None);
        assert_eq!(PushNeed::None.notice("origin"), None);
        assert!(PushNeed::Publish.notice("origin").unwrap().contains("has not been published yet. It will be pushed to origin first."));
        assert!(PushNeed::Push { ahead: 1 }.notice("origin").unwrap().contains("1 commit that is not on the remote yet. It will"));
        assert!(PushNeed::Push { ahead: 3 }.notice("origin").unwrap().contains("3 commits that are not on the remote yet. They will"));
        assert_eq!(PushNeed::Publish.submit_label("Pull Request"), "Publish & Create Pull Request");
        assert_eq!(PushNeed::Push { ahead: 1 }.submit_label("Merge Request"), "Push & Create Merge Request");
        assert_eq!(PushNeed::None.submit_label("Pull Request"), "Create Pull Request");
    }

    #[test]
    fn source_branch_uses_the_name_on_the_remote() {
        let refs = refs();
        assert_eq!(source_branch_name("local", None, Some(&refs)), "local");
        assert_eq!(source_branch_name("local", Some(&upstream("origin/local", 0, false)), Some(&refs)), "local");
        assert_eq!(source_branch_name("local", Some(&upstream("origin/feature/login", 0, true)), Some(&refs)), "feature/login");
        // The remote with the longest matching name wins.
        assert_eq!(source_branch_name("local", Some(&upstream("origin-fork/topic", 0, true)), Some(&refs)), "topic");
        // Without a ref list: cut at the first slash.
        assert_eq!(source_branch_name("local", Some(&upstream("upstream/a/b", 0, true)), None), "a/b");
        assert_eq!(source_branch_name("local", Some(&upstream("no-slash", 0, true)), None), "no-slash");
    }

    #[test]
    fn target_options_come_from_one_remote_without_head() {
        let refs = refs();
        assert_eq!(target_branch_options(Some(&refs), "origin", &[]), ["develop", "main"]);
        assert_eq!(target_branch_options(Some(&refs), "fork", &["main", "", "release"]), ["main", "release", "topic"]);
        assert_eq!(target_branch_options(None, "origin", &["main"]), ["main"]);
        assert!(target_branch_options(Some(&refs), "does-not-exist", &[]).is_empty());
    }

    #[test]
    fn target_defaults_to_the_default_branch_unless_it_is_the_source() {
        assert_eq!(effective_target(None, Some("main"), "feature"), "main");
        assert_eq!(effective_target(None, Some("main"), "main"), "");
        assert_eq!(effective_target(None, None, "feature"), "");
        assert_eq!(effective_target(None, Some(""), "feature"), "");
        assert_eq!(effective_target(Some("develop"), Some("main"), "feature"), "develop");
    }

    #[test]
    fn create_form_reports_what_is_missing() {
        assert_eq!(create_problem("  ", "main", "feature", true, false), Some("Enter a title."));
        // While the default branch is loading, there is no complaint about the target yet.
        assert_eq!(create_problem("Title", "", "feature", false, false), None);
        assert_eq!(create_problem("Title", "", "feature", true, false), Some("Choose a target branch."));
        assert_eq!(create_problem("Title", "feature", "feature", true, true), Some("The source and target branch are the same."));
        assert_eq!(create_problem("Title", "main", "feature", true, false), None);
    }

    #[test]
    fn base_ref_prefers_the_tracking_branch() {
        let refs = refs();
        assert_eq!(base_ref_for("origin", "main", Some(&refs)).as_deref(), Some("origin/main"));
        assert_eq!(base_ref_for("origin", "feature/login", Some(&refs)).as_deref(), Some("feature/login"));
        assert_eq!(base_ref_for("origin", "release", Some(&refs)), None);
        assert_eq!(base_ref_for("upstream", "main", Some(&refs)).as_deref(), Some("main"));
        assert_eq!(base_ref_for("origin", "main", None), None);
    }

    #[test]
    fn checks_are_summarised() {
        assert_eq!(summarize_checks(&[]), "");
        let checks = [check("build", CheckState::Success), check("lint", CheckState::Failure), check("deploy", CheckState::Running)];
        assert_eq!(summarize_checks(&checks), "1 failed, 1 in progress, 1 passed");
        assert_eq!(summarize_checks(&[check("a", CheckState::Skipped), check("b", CheckState::Success)]), "1 passed, 1 skipped or cancelled");
        assert_eq!(check_label(CheckState::Success), "Passed");
        assert_eq!(check_label(CheckState::Failure), "Failed");
    }

    #[test]
    fn badge_shows_draft_only_while_open() {
        let pr = |state: PrState, draft: bool| PullRequest {
            number: 1,
            title: "x".into(),
            state,
            draft,
            author: "ana".into(),
            author_avatar: None,
            source_branch: "a".into(),
            target_branch: "b".into(),
            head_sha: "0".repeat(40),
            updated_at: 0.0,
            web_url: String::new(),
            is_cross_repo: false,
        };
        assert_eq!(PrBadge::of(&pr(PrState::Open, false)), PrBadge::Open);
        assert_eq!(PrBadge::of(&pr(PrState::Open, true)), PrBadge::Draft);
        assert_eq!(PrBadge::of(&pr(PrState::Merged, true)), PrBadge::Merged);
        assert_eq!(PrBadge::of(&pr(PrState::Closed, false)).label(), "Closed");
        assert_eq!(plural(1, "commit"), "1 commit");
        assert_eq!(plural(3, "file"), "3 files");
        assert_eq!(mergeable_text(Some(false)), "Conflicts with the target branch");
    }

    #[test]
    fn pr_tab_carries_everything_needed_to_reload_it() {
        let account = Account { id: "gitlab:git.example.com:budi".into(), kind: ForgeKind::GitLab, host: "https://git.example.com".into(), username: "budi".into(), display_name: "Budi".into(), avatar_url: None };
        let repo = RepoRef { host: "git.example.com".into(), owner: "team".into(), name: "project".into() };
        let mut context = ForgeContext { remote: "origin".into(), repo: repo.clone(), kind: None, account: Some(account), web_url: "https://git.example.com/team/project".into() };
        let tab = pr_tab("/repo", &context, 5, "Add validation to the login form with an extremely long title").unwrap();
        assert_eq!(tab.kind, PR_TAB_KIND);
        assert_eq!(tab.id, "pullRequest:gitlab:git.example.com:budi:git.example.com/team/project:5");
        assert_eq!(tab.title, "!5 Add validation to the login form with a…");
        assert_eq!(tab.description.as_deref(), Some("Merge Request !5 · team/project\nAdd validation to the login form with an extremely long title"));
        let params: PrTabParams = serde_json::from_value(tab.params.clone()).unwrap();
        // The forge kind is taken from the account if the host is not recognized.
        assert_eq!(params, PrTabParams { root: "/repo".into(), account_id: "gitlab:git.example.com:budi".into(), repo, kind: ForgeKind::GitLab, remote: "origin".into(), number: 5 });
        assert_eq!(tab.params["accountId"], "gitlab:git.example.com:budi");
        context.account = None;
        assert!(pr_tab("/repo", &context, 5, "x").is_none());
    }
}
