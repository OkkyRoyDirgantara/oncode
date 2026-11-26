//! Per-window state of the Graph feature: the refs, stashes, worktrees, and submodules of each repository,
//! "show this commit in Graph" requests, and the indicator of the running operation.

use std::collections::HashMap;

use gpui_kit::{App, Context, Entity, EventEmitter, Global, SharedString, Subscription, Task};
use oncode_core::api::git;
use oncode_git::{Refs, RepoSnapshot, Stash, Submodule, Worktree};
use oncode_ui::{repos, spawn_core, Env, ReposEvent};

/// A request to show a commit in the Graph tab (from the sidebar, reflog, file history).
#[derive(Debug, Clone, PartialEq)]
pub struct RevealTarget {
    pub oid: String,
    /// The name mentioned in messages, e.g. a branch name or a short hash.
    pub label: String,
    /// Load the next pages until the commit is found. If false, the commit is only selected.
    pub hunt: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RepoData {
    /// `None` until the first load finishes.
    pub refs: Option<Refs>,
    pub stashes: Vec<Stash>,
    pub worktrees: Vec<Worktree>,
    pub submodules: Vec<Submodule>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StoreEvent {
    /// The ref data of this repository finished reloading.
    Loaded(String),
    /// A branch, tag, HEAD, or stash changed: the commit list needs to be reloaded.
    RefsChanged(String),
    /// There is a new `RevealTarget` request for this repository; take it with `take_reveal`.
    RevealRequested(String),
    /// The running-operation indicator changed.
    BusyChanged(String),
}

/// What decides whether refs need reloading when the snapshot changes.
#[derive(Clone, PartialEq)]
struct HeadKey {
    head: oncode_git::Head,
    head_oid: Option<String>,
    stash_count: u32,
}

impl HeadKey {
    fn of(snapshot: &RepoSnapshot) -> Self {
        Self { head: snapshot.head.clone(), head_oid: snapshot.head_oid.clone(), stash_count: snapshot.stash_count }
    }
}

/// A Git operation currently running on a repository.
#[derive(Debug, Clone, PartialEq)]
pub struct Busy {
    pub label: SharedString,
    /// For canceling through `api::app::app_cancel`.
    pub operation_id: Option<String>,
}

struct Loading {
    /// Another change arrived while loading: load once more when finished.
    again: bool,
    _task: Task<()>,
}

pub struct GraphStore {
    env: Env,
    repos: HashMap<String, RepoData>,
    heads: HashMap<String, HeadKey>,
    loading: HashMap<String, Loading>,
    reveals: HashMap<String, RevealTarget>,
    busy: HashMap<String, Busy>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<StoreEvent> for GraphStore {}

/// The store of the window that is currently building tabs; set through `Workbench::on_before_build_tabs`.
pub(crate) struct CurrentStore(pub Entity<GraphStore>);

impl Global for CurrentStore {}

impl GraphStore {
    pub fn new(env: Env, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&repos(cx), |this: &mut Self, repos, event: &ReposEvent, cx| match event {
            ReposEvent::Changed(root) => {
                // The stash count and HEAD also determine the list that is shown.
                let key = repos.read(cx).snapshot(root).map(HeadKey::of);
                let changed = key.is_some() && this.heads.get(root) != key.as_ref();
                if let Some(key) = key {
                    this.heads.insert(root.clone(), key);
                }
                if changed && this.repos.contains_key(root) {
                    this.refs_changed(root, cx);
                }
                cx.notify();
            }
            ReposEvent::RefsChanged(root) => {
                if this.repos.contains_key(root) {
                    this.refs_changed(root, cx);
                }
            }
        });
        let mut this = Self {
            env: env.clone(),
            repos: HashMap::new(),
            heads: HashMap::new(),
            loading: HashMap::new(),
            reveals: HashMap::new(),
            busy: HashMap::new(),
            _subscriptions: vec![subscription],
        };
        if let Some(root) = env.repo_root.clone() {
            this.ensure(&root, cx);
        }
        this
    }

    pub fn env(&self) -> &Env {
        &self.env
    }

    /// The repository root of this window's project folder.
    pub fn default_root(&self) -> Option<&str> {
        self.env.repo_root.as_deref()
    }

    /// All repositories known to this store (the project root and opened submodules).
    pub fn roots(&self) -> Vec<String> {
        let mut roots: Vec<String> = self.repos.keys().cloned().collect();
        roots.sort();
        roots
    }

    pub fn data(&self, root: &str) -> Option<&RepoData> {
        self.repos.get(root)
    }

    pub fn refs(&self, root: &str) -> Option<&Refs> {
        self.repos.get(root).and_then(|data| data.refs.as_ref())
    }

    pub fn stashes(&self, root: &str) -> &[Stash] {
        self.repos.get(root).map(|data| data.stashes.as_slice()).unwrap_or_default()
    }

    /// The repository's first remote (the default destination for tags).
    pub fn first_remote(&self, root: &str) -> Option<String> {
        self.refs(root).and_then(|refs| refs.remotes.first()).map(|remote| remote.name.clone())
    }

    /// Starts tracking a repository and loads its data if not loaded yet.
    pub fn ensure(&mut self, root: &str, cx: &mut Context<Self>) {
        if !self.repos.contains_key(root) {
            self.repos.insert(root.to_string(), RepoData::default());
            if let Some(snapshot) = repos(cx).read(cx).snapshot(root) {
                self.heads.insert(root.to_string(), HeadKey::of(snapshot));
            }
            self.reload(root, cx);
        }
    }

    fn refs_changed(&mut self, root: &str, cx: &mut Context<Self>) {
        self.reload(root, cx);
        cx.emit(StoreEvent::RefsChanged(root.to_string()));
    }

    /// Reloads refs, stashes, worktrees, and submodules. Requests that arrive while loading are merged.
    pub fn reload(&mut self, root: &str, cx: &mut Context<Self>) {
        if let Some(loading) = self.loading.get_mut(root) {
            loading.again = true;
            return;
        }
        let (core, target) = (self.env.core.clone(), root.to_string());
        let load = spawn_core(cx, async move {
            let refs = git::git_refs(&core, target.clone()).await.ok();
            let stashes = git::git_stash_list(&core, target.clone()).await.unwrap_or_default();
            let worktrees = git::git_worktrees(&core, target.clone()).await.unwrap_or_default();
            let submodules = git::git_submodules(&core, target).await.unwrap_or_default();
            (refs, stashes, worktrees, submodules)
        });
        let root_key = root.to_string();
        let task = cx.spawn(async move |this, cx| {
            let (refs, stashes, worktrees, submodules) = load.await;
            let _ = this.update(cx, |this, cx| {
                let again = this.loading.remove(&root_key).is_some_and(|l| l.again);
                if let Some(data) = this.repos.get_mut(&root_key) {
                    // If loading the refs fails, the old refs are kept.
                    if refs.is_some() {
                        data.refs = refs;
                    } else if data.refs.is_none() {
                        data.refs = Some(Refs { branches: Vec::new(), remote_branches: Vec::new(), tags: Vec::new(), remotes: Vec::new() });
                    }
                    data.stashes = stashes;
                    data.worktrees = worktrees;
                    data.submodules = submodules;
                }
                cx.emit(StoreEvent::Loaded(root_key.clone()));
                cx.notify();
                if again {
                    this.reload(&root_key, cx);
                }
            });
        });
        self.loading.insert(root.to_string(), Loading { again: false, _task: task });
    }

    // ----- Reveal requests -----

    /// Leaves a request; the Graph tab for `root` picks it up when ready.
    pub fn request_reveal(&mut self, root: &str, target: RevealTarget, cx: &mut Context<Self>) {
        self.reveals.insert(root.to_string(), target);
        cx.emit(StoreEvent::RevealRequested(root.to_string()));
    }

    pub fn take_reveal(&mut self, root: &str) -> Option<RevealTarget> {
        self.reveals.remove(root)
    }

    pub fn pending_reveal(&self, root: &str) -> Option<&RevealTarget> {
        self.reveals.get(root)
    }

    // ----- Running operations -----

    pub fn busy(&self, root: &str) -> Option<&Busy> {
        self.busy.get(root)
    }

    pub fn set_busy(&mut self, root: &str, busy: Option<Busy>, cx: &mut Context<Self>) {
        match busy {
            Some(busy) => self.busy.insert(root.to_string(), busy),
            None => self.busy.remove(root),
        };
        cx.emit(StoreEvent::BusyChanged(root.to_string()));
        cx.notify();
    }
}

/// A repository snapshot from the global `oncode_ui::repos` store.
pub fn snapshot(root: &str, cx: &App) -> Option<RepoSnapshot> {
    repos(cx).read(cx).snapshot(root).cloned()
}

/// A short name for the current HEAD.
pub fn head_label(snapshot: Option<&RepoSnapshot>) -> String {
    match snapshot.map(|s| &s.head) {
        Some(oncode_git::Head::Branch { name }) | Some(oncode_git::Head::Unborn { name }) => name.clone(),
        Some(oncode_git::Head::Detached { oid }) => crate::format::short_oid(oid).to_string(),
        None => String::new(),
    }
}

/// The number of changed files (each path is counted once).
pub fn change_count(snapshot: Option<&RepoSnapshot>) -> usize {
    let Some(snapshot) = snapshot else { return 0 };
    let mut paths: std::collections::HashSet<&str> = std::collections::HashSet::new();
    paths.extend(snapshot.staged.iter().map(|c| c.path.as_str()));
    paths.extend(snapshot.unstaged.iter().map(|c| c.path.as_str()));
    paths.extend(snapshot.untracked.iter().map(|p| p.as_str()));
    paths.extend(snapshot.conflicted.iter().map(|c| c.path.as_str()));
    paths.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{oid, snapshot as fake_snapshot};
    use oncode_git::{ChangeKind, FileChange, Head};

    #[test]
    fn head_label_names_the_branch_or_shortens_the_hash() {
        let mut snapshot = fake_snapshot();
        assert_eq!(head_label(Some(&snapshot)), "main");
        snapshot.head = Head::Unborn { name: "trunk".into() };
        assert_eq!(head_label(Some(&snapshot)), "trunk");
        snapshot.head = Head::Detached { oid: oid(7) };
        assert_eq!(head_label(Some(&snapshot)), &oid(7)[..8]);
        assert_eq!(head_label(None), "");
    }

    #[test]
    fn change_count_counts_each_path_once() {
        let mut snapshot = fake_snapshot();
        assert_eq!(change_count(Some(&snapshot)), 0);
        let change = |path: &str| FileChange { path: path.into(), orig_path: None, kind: ChangeKind::Modified, submodule: None };
        snapshot.staged = vec![change("a.txt"), change("b.txt")];
        snapshot.unstaged = vec![change("a.txt")];
        snapshot.untracked = vec!["c.txt".into()];
        assert_eq!(change_count(Some(&snapshot)), 3);
        assert_eq!(change_count(None), 0);
    }
}
