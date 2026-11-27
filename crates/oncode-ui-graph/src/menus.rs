//! Context menus for commits and refs as plain data: labels, disabled state, and actions.
//! The view turns them into a `PopupMenu`; their actions are run by `actions::run_action`.

use oncode_git::{BisectMark, Branch, Commit, GraphRow, Head, Operation, RefKind, RemoteBranch, RepoSnapshot, ResetMode, Stash, Tag};

use crate::format::short_oid;
use crate::refs::is_ancestor_of_head;

/// An action triggered by a menu item (or button) on a specific repository.
#[derive(Debug, Clone, PartialEq)]
pub enum GitAction {
    CheckoutBranch(String),
    /// The full name of a remote branch, e.g. `origin/main`.
    CheckoutRemote(String),
    CheckoutDetached(String),
    CreateBranch(String),
    CreateTag(String),
    CherryPick(Vec<String>),
    Revert { oid: String, subject: String },
    Reset { rev: String, mode: ResetMode },
    /// `None` = from the repository's first commit.
    InteractiveRebase { base: Option<String> },
    Merge(String),
    RebaseOnto(String),
    RenameBranch(String),
    DeleteBranch(String),
    /// `branch: None` = the active branch.
    Push { branch: Option<String>, set_upstream: bool },
    SetUpstream(String),
    DeleteRemoteBranch(String),
    DeleteTag(String),
    PushTag(String),
    ShowStash(String),
    StashApply { name: String, pop: bool },
    StashBranch(String),
    StashDrop { name: String, message: String },
    /// Select this commit together with HEAD to see the list of files that differ.
    CompareWithHead(String),
    /// The changes on a branch since it split from the current branch (merge base to branch).
    CompareWithCurrent(String),
    ShowInGraph { oid: String, label: String },
    Copy { text: String, what: &'static str },
    /// Start a bisect with this commit as the bad one; the good one is asked for.
    BisectStart { bad: String },
    /// Mark a commit (or, with `None`, the one being tested) during a bisect.
    BisectMark { mark: BisectMark, oid: Option<String> },
    BisectReset,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MenuEntry {
    Item { label: String, disabled: bool, danger: bool, action: GitAction },
    Separator,
}

impl MenuEntry {
    fn new(label: impl Into<String>, action: GitAction) -> Self {
        Self::Item { label: label.into(), disabled: false, danger: false, action }
    }

    fn disabled(self, value: bool) -> Self {
        match self {
            Self::Item { label, danger, action, .. } => Self::Item { label, disabled: value, danger, action },
            other => other,
        }
    }

    fn danger(self) -> Self {
        match self {
            Self::Item { label, disabled, action, .. } => Self::Item { label, disabled, danger: true, action },
            other => other,
        }
    }

    pub fn label(&self) -> Option<&str> {
        match self {
            Self::Item { label, .. } => Some(label),
            Self::Separator => None,
        }
    }
}

/// The item labeled `label`, for tests and for running actions from the keyboard.
pub fn find<'a>(entries: &'a [MenuEntry], label: &str) -> Option<(&'a GitAction, bool)> {
    entries.iter().find_map(|entry| match entry {
        MenuEntry::Item { label: l, disabled, action, .. } if l == label => Some((action, *disabled)),
        _ => None,
    })
}

pub struct CommitMenuContext<'a> {
    pub snapshot: Option<&'a RepoSnapshot>,
    pub commit: &'a Commit,
    /// The rows already loaded, to check whether this commit is an ancestor of HEAD.
    pub rows: &'a [GraphRow],
    /// The list is filtered, so ancestry cannot be inferred from the list.
    pub filtered: bool,
    /// Other selected commits that are cherry-picked along with it (ordered oldest first).
    pub selection: Option<&'a [String]>,
}

pub fn commit_menu(context: &CommitMenuContext) -> Vec<MenuEntry> {
    let CommitMenuContext { snapshot, commit, rows, filtered, selection } = context;
    let head_oid = snapshot.and_then(|s| s.head_oid.as_deref());
    let current_branch = match snapshot.map(|s| &s.head) {
        Some(Head::Branch { name }) => Some(name.as_str()),
        _ => None,
    };
    let oid = commit.oid.clone();
    let is_head = Some(oid.as_str()) == head_oid;
    let ancestor = if *filtered { None } else { is_ancestor_of_head(rows, head_oid, &oid) };
    let picks: Vec<String> = match selection {
        Some(selection) if selection.len() > 1 && selection.contains(&oid) => selection.to_vec(),
        _ => vec![oid.clone()],
    };

    let mut entries: Vec<MenuEntry> = commit
        .refs
        .iter()
        .filter(|r| r.kind == RefKind::Branch && Some(r.name.as_str()) != current_branch)
        .map(|b| MenuEntry::new(format!("Checkout Branch {}", b.name), GitAction::CheckoutBranch(b.name.clone())))
        .collect();
    let cherry_label = if picks.len() > 1 { format!("Cherry-pick {} Commits", picks.len()) } else { "Cherry-pick".to_string() };
    let cherry_disabled = picks.len() == 1 && is_head;
    let reset = |mode| GitAction::Reset { rev: oid.clone(), mode };
    entries.extend([
        MenuEntry::new("Checkout (detached)", GitAction::CheckoutDetached(oid.clone())).disabled(is_head && current_branch.is_none()),
        MenuEntry::Separator,
        MenuEntry::new("Create Branch Here…", GitAction::CreateBranch(oid.clone())),
        MenuEntry::new("Create Tag Here…", GitAction::CreateTag(oid.clone())),
        MenuEntry::Separator,
        MenuEntry::new(cherry_label, GitAction::CherryPick(picks)).disabled(cherry_disabled),
        MenuEntry::new("Revert…", GitAction::Revert { oid: oid.clone(), subject: commit.subject.clone() }),
        MenuEntry::Separator,
        MenuEntry::new("Reset Current Branch to Here (Soft)…", reset(ResetMode::Soft)).disabled(is_head),
        MenuEntry::new("Reset Current Branch to Here (Mixed)…", reset(ResetMode::Mixed)).disabled(is_head),
        MenuEntry::new("Reset Current Branch to Here (Hard)…", reset(ResetMode::Hard)).danger(),
        MenuEntry::Separator,
        // Rebase rearranges the commits AFTER this commit; it only makes sense for ancestors of HEAD.
        MenuEntry::new("Interactive Rebase from Here…", GitAction::InteractiveRebase { base: Some(oid.clone()) }).disabled(is_head || ancestor == Some(false)),
        MenuEntry::Separator,
        MenuEntry::new("Compare with HEAD", GitAction::CompareWithHead(oid.clone())).disabled(is_head || head_oid.is_none()),
        MenuEntry::Separator,
    ]);
    // Bisect: start here (this commit is bad), or, while bisecting, mark this commit.
    let bisecting = matches!(snapshot.and_then(|s| s.operation.as_ref()), Some(Operation::Bisect { .. }));
    if bisecting {
        let mark = |mark| GitAction::BisectMark { mark, oid: Some(oid.clone()) };
        entries.extend([
            MenuEntry::new("Bisect: Mark as Good", mark(BisectMark::Good)),
            MenuEntry::new("Bisect: Mark as Bad", mark(BisectMark::Bad)),
            MenuEntry::new("Bisect: Skip", mark(BisectMark::Skip)),
            MenuEntry::new("Stop Bisect", GitAction::BisectReset),
        ]);
    } else {
        entries.push(MenuEntry::new("Start Bisect (This Commit Is Bad)…", GitAction::BisectStart { bad: oid.clone() }).disabled(snapshot.is_some_and(|s| s.operation.is_some())));
    }
    entries.extend([
        MenuEntry::Separator,
        MenuEntry::new("Copy Hash", GitAction::Copy { text: oid.clone(), what: "Hash" }),
        MenuEntry::new("Copy Subject", GitAction::Copy { text: commit.subject.clone(), what: "Subject" }),
    ]);
    entries
}

pub fn branch_menu(branch: &Branch) -> Vec<MenuEntry> {
    let name = branch.name.clone();
    let elsewhere = branch.worktree.is_some() && !branch.is_head;
    let published = branch.upstream.is_some() && !branch.upstream_gone;
    let push = if branch.is_head { GitAction::Push { branch: None, set_upstream: false } } else { GitAction::Push { branch: Some(name.clone()), set_upstream: !published } };
    vec![
        MenuEntry::new("Checkout", GitAction::CheckoutBranch(name.clone())).disabled(branch.is_head || elsewhere),
        MenuEntry::new("Merge into Current…", GitAction::Merge(name.clone())).disabled(branch.is_head),
        MenuEntry::new("Rebase Current onto…", GitAction::RebaseOnto(name.clone())).disabled(branch.is_head),
        MenuEntry::new("Compare with Current Branch", GitAction::CompareWithCurrent(name.clone())).disabled(branch.is_head),
        MenuEntry::Separator,
        MenuEntry::new("Rename…", GitAction::RenameBranch(name.clone())),
        MenuEntry::new("Delete…", GitAction::DeleteBranch(name.clone())).danger().disabled(branch.is_head || elsewhere),
        MenuEntry::Separator,
        MenuEntry::new(if published { "Push" } else { "Publish" }, push),
        MenuEntry::new("Set Upstream…", GitAction::SetUpstream(name.clone())),
        MenuEntry::Separator,
        MenuEntry::new("Copy Name", GitAction::Copy { text: name, what: "Branch name" }),
    ]
}

pub fn remote_branch_menu(branch: &RemoteBranch) -> Vec<MenuEntry> {
    let name = branch.name.clone();
    vec![
        MenuEntry::new("Checkout (tracking)", GitAction::CheckoutRemote(name.clone())),
        MenuEntry::new("Merge into Current…", GitAction::Merge(name.clone())),
        MenuEntry::new("Rebase Current onto…", GitAction::RebaseOnto(name.clone())),
        MenuEntry::new("Compare with Current Branch", GitAction::CompareWithCurrent(name.clone())),
        MenuEntry::Separator,
        MenuEntry::new("Delete on Remote…", GitAction::DeleteRemoteBranch(name.clone())).danger(),
        MenuEntry::Separator,
        MenuEntry::new("Copy Name", GitAction::Copy { text: name, what: "Branch name" }),
    ]
}

/// `remote`: the repository's first remote, for the offer to push the tag.
pub fn tag_menu(tag: &Tag, remote: Option<&str>) -> Vec<MenuEntry> {
    let name = tag.name.clone();
    let mut entries = vec![MenuEntry::new("Checkout (detached)", GitAction::CheckoutDetached(name.clone()))];
    if let Some(remote) = remote {
        entries.push(MenuEntry::new(format!("Push to {remote}"), GitAction::PushTag(name.clone())));
    }
    entries.extend([
        MenuEntry::new("Delete…", GitAction::DeleteTag(name.clone())).danger(),
        MenuEntry::Separator,
        MenuEntry::new("Copy Name", GitAction::Copy { text: name, what: "Tag name" }),
    ]);
    entries
}

pub fn stash_menu(stash: &Stash) -> Vec<MenuEntry> {
    let name = stash.name.clone();
    vec![
        MenuEntry::new("Show Changes", GitAction::ShowStash(name.clone())),
        MenuEntry::new("Apply", GitAction::StashApply { name: name.clone(), pop: false }),
        MenuEntry::new("Pop (apply and drop)", GitAction::StashApply { name: name.clone(), pop: true }),
        MenuEntry::new("Create Branch from Stash…", GitAction::StashBranch(name.clone())),
        MenuEntry::Separator,
        MenuEntry::new("Drop…", GitAction::StashDrop { name, message: stash.message.clone() }).danger(),
    ]
}

/// Context menu for a reflog entry (or any commit known only by its hash).
pub fn reflog_menu(oid: &str) -> Vec<MenuEntry> {
    let oid = oid.to_string();
    vec![
        MenuEntry::new("Create Branch Here…", GitAction::CreateBranch(oid.clone())),
        MenuEntry::new("Checkout (detached)", GitAction::CheckoutDetached(oid.clone())),
        MenuEntry::Separator,
        MenuEntry::new("Reset Current Branch to Here (Mixed)…", GitAction::Reset { rev: oid.clone(), mode: ResetMode::Mixed }),
        MenuEntry::new("Reset Current Branch to Here (Hard)…", GitAction::Reset { rev: oid.clone(), mode: ResetMode::Hard }).danger(),
        MenuEntry::Separator,
        MenuEntry::new(format!("Copy Hash ({})", short_oid(&oid)), GitAction::Copy { text: oid.clone(), what: "Hash" }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{branch, commit, linear_rows, oid, snapshot};
    use oncode_git::RefLabel;

    fn labels(entries: &[MenuEntry]) -> Vec<&str> {
        entries.iter().filter_map(MenuEntry::label).collect()
    }

    fn is_disabled(entries: &[MenuEntry], label: &str) -> bool {
        find(entries, label).unwrap_or_else(|| panic!("no item {label}")).1
    }

    #[test]
    fn commit_menu_offers_the_actions_for_the_clicked_commit() {
        let rows = linear_rows(0, 5);
        let snapshot = snapshot();
        let mut target = commit(3);
        target.refs = vec![RefLabel { kind: RefKind::Branch, name: "feature".into() }, RefLabel { kind: RefKind::Branch, name: "main".into() }, RefLabel { kind: RefKind::Tag, name: "v1".into() }];
        let menu = commit_menu(&CommitMenuContext { snapshot: Some(&snapshot), commit: &target, rows: &rows, filtered: false, selection: None });
        assert_eq!(
            labels(&menu),
            [
                // The active branch (main) is not offered for checkout.
                "Checkout Branch feature",
                "Checkout (detached)",
                "Create Branch Here…",
                "Create Tag Here…",
                "Cherry-pick",
                "Revert…",
                "Reset Current Branch to Here (Soft)…",
                "Reset Current Branch to Here (Mixed)…",
                "Reset Current Branch to Here (Hard)…",
                "Interactive Rebase from Here…",
                "Compare with HEAD",
                "Start Bisect (This Commit Is Bad)…",
                "Copy Hash",
                "Copy Subject",
            ]
        );
        assert!(labels(&menu).iter().all(|l| !is_disabled(&menu, l)), "nothing is disabled for an ancestor of HEAD");
        assert_eq!(find(&menu, "Checkout Branch feature").unwrap().0, &GitAction::CheckoutBranch("feature".into()));
        assert_eq!(find(&menu, "Cherry-pick").unwrap().0, &GitAction::CherryPick(vec![oid(3)]));
        assert_eq!(find(&menu, "Revert…").unwrap().0, &GitAction::Revert { oid: oid(3), subject: "Commit 3".into() });
        assert_eq!(find(&menu, "Reset Current Branch to Here (Hard)…").unwrap().0, &GitAction::Reset { rev: oid(3), mode: ResetMode::Hard });
        assert_eq!(find(&menu, "Interactive Rebase from Here…").unwrap().0, &GitAction::InteractiveRebase { base: Some(oid(3)) });
        assert_eq!(find(&menu, "Copy Subject").unwrap().0, &GitAction::Copy { text: "Commit 3".into(), what: "Subject" });
        assert!(menu.iter().any(|e| matches!(e, MenuEntry::Item { label, danger: true, .. } if label.contains("(Hard)"))));
    }

    #[test]
    fn head_commit_disables_rebase_and_soft_mixed_reset() {
        let rows = linear_rows(0, 5);
        let snapshot = snapshot();
        let head = commit(0);
        let menu = commit_menu(&CommitMenuContext { snapshot: Some(&snapshot), commit: &head, rows: &rows, filtered: false, selection: None });
        for label in ["Cherry-pick", "Reset Current Branch to Here (Soft)…", "Reset Current Branch to Here (Mixed)…", "Interactive Rebase from Here…", "Compare with HEAD"] {
            assert!(is_disabled(&menu, label), "{label}");
        }
        // A hard reset to HEAD is still useful (it discards changes), and so is a detached checkout from a branch.
        assert!(!is_disabled(&menu, "Reset Current Branch to Here (Hard)…"));
        assert!(!is_disabled(&menu, "Checkout (detached)"));

        let mut detached = snapshot.clone();
        detached.head = Head::Detached { oid: oid(0) };
        let menu = commit_menu(&CommitMenuContext { snapshot: Some(&detached), commit: &head, rows: &rows, filtered: false, selection: None });
        assert!(is_disabled(&menu, "Checkout (detached)"));
    }

    #[test]
    fn rebase_is_disabled_for_commits_that_are_not_ancestors_unless_the_list_is_filtered() {
        use crate::testing::row;
        // HEAD = 2 → 3 → 4; commit 1 is on another branch.
        let rows = vec![row(2, &[3]), row(1, &[4]), row(3, &[4]), row(4, &[])];
        let mut snapshot = snapshot();
        snapshot.head_oid = Some(oid(2));
        let side = rows[1].commit.clone();
        let menu = commit_menu(&CommitMenuContext { snapshot: Some(&snapshot), commit: &side, rows: &rows, filtered: false, selection: None });
        assert!(is_disabled(&menu, "Interactive Rebase from Here…"));
        let menu = commit_menu(&CommitMenuContext { snapshot: Some(&snapshot), commit: &side, rows: &rows, filtered: true, selection: None });
        assert!(!is_disabled(&menu, "Interactive Rebase from Here…"), "filtered list: cannot be inferred, so it is allowed");
    }

    #[test]
    fn cherry_pick_takes_the_whole_selection_when_the_commit_is_part_of_it() {
        let rows = linear_rows(0, 6);
        let snapshot = snapshot();
        let selection = vec![oid(4), oid(2)];
        let menu = commit_menu(&CommitMenuContext { snapshot: Some(&snapshot), commit: &rows[2].commit, rows: &rows, filtered: false, selection: Some(&selection) });
        assert_eq!(find(&menu, "Cherry-pick 2 Commits").unwrap().0, &GitAction::CherryPick(vec![oid(4), oid(2)]));
        // A commit outside the selection: only that commit.
        let menu = commit_menu(&CommitMenuContext { snapshot: Some(&snapshot), commit: &rows[5].commit, rows: &rows, filtered: false, selection: Some(&selection) });
        assert_eq!(find(&menu, "Cherry-pick").unwrap().0, &GitAction::CherryPick(vec![oid(5)]));
    }

    #[test]
    fn branch_menu_depends_on_head_worktree_and_upstream() {
        let mut current = branch("main");
        current.is_head = true;
        current.upstream = Some("origin/main".into());
        let menu = branch_menu(&current);
        assert_eq!(labels(&menu), ["Checkout", "Merge into Current…", "Rebase Current onto…", "Compare with Current Branch", "Rename…", "Delete…", "Push", "Set Upstream…", "Copy Name"]);
        for label in ["Checkout", "Merge into Current…", "Rebase Current onto…", "Compare with Current Branch", "Delete…"] {
            assert!(is_disabled(&menu, label), "{label}");
        }
        assert_eq!(find(&menu, "Push").unwrap().0, &GitAction::Push { branch: None, set_upstream: false });

        let other = branch("feature/login");
        let menu = branch_menu(&other);
        assert!(labels(&menu).iter().all(|l| !is_disabled(&menu, l)));
        assert_eq!(find(&menu, "Publish").unwrap().0, &GitAction::Push { branch: Some("feature/login".into()), set_upstream: true });

        let mut gone = branch("old");
        gone.upstream = Some("origin/old".into());
        gone.upstream_gone = true;
        assert_eq!(find(&branch_menu(&gone), "Publish").unwrap().0, &GitAction::Push { branch: Some("old".into()), set_upstream: true });

        let mut tracked = branch("next");
        tracked.upstream = Some("origin/next".into());
        assert_eq!(find(&branch_menu(&tracked), "Push").unwrap().0, &GitAction::Push { branch: Some("next".into()), set_upstream: false });

        let mut elsewhere = branch("busy");
        elsewhere.worktree = Some("/work/repo-busy".into());
        let menu = branch_menu(&elsewhere);
        assert!(is_disabled(&menu, "Checkout") && is_disabled(&menu, "Delete…"));
        assert!(!is_disabled(&menu, "Merge into Current…"));
    }

    #[test]
    fn remote_tag_stash_and_reflog_menus() {
        let remote = RemoteBranch { name: "origin/feature/x".into(), remote: "origin".into(), oid: oid(1) };
        let menu = remote_branch_menu(&remote);
        assert_eq!(labels(&menu), ["Checkout (tracking)", "Merge into Current…", "Rebase Current onto…", "Compare with Current Branch", "Delete on Remote…", "Copy Name"]);
        assert_eq!(find(&menu, "Delete on Remote…").unwrap().0, &GitAction::DeleteRemoteBranch("origin/feature/x".into()));

        let tag = Tag { name: "v1.0.0".into(), oid: oid(2), annotated: false, message: String::new() };
        assert_eq!(labels(&tag_menu(&tag, None)), ["Checkout (detached)", "Delete…", "Copy Name"]);
        let menu = tag_menu(&tag, Some("origin"));
        assert_eq!(labels(&menu), ["Checkout (detached)", "Push to origin", "Delete…", "Copy Name"]);
        assert_eq!(find(&menu, "Push to origin").unwrap().0, &GitAction::PushTag("v1.0.0".into()));

        let stash = Stash { index: 0, name: "stash@{0}".into(), oid: oid(3), message: "WIP on main".into(), time: 0.0 };
        let menu = stash_menu(&stash);
        assert_eq!(labels(&menu), ["Show Changes", "Apply", "Pop (apply and drop)", "Create Branch from Stash…", "Drop…"]);
        assert_eq!(find(&menu, "Drop…").unwrap().0, &GitAction::StashDrop { name: "stash@{0}".into(), message: "WIP on main".into() });

        let menu = reflog_menu(&oid(9));
        assert_eq!(
            labels(&menu),
            ["Create Branch Here…", "Checkout (detached)", "Reset Current Branch to Here (Mixed)…", "Reset Current Branch to Here (Hard)…", &format!("Copy Hash ({})", &oid(9)[..8])]
        );
    }
}
