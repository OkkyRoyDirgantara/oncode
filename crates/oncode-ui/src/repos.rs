//! The state of open repositories, shared by the explorer, editor, status bar, and
//! Source Control panel. Fed by the `RepoChanged` event of `oncode-core`.

use std::collections::HashMap;

use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Global, Subscription, Window};
use oncode_core::api::git;
use oncode_core::{root_key, CoreEvent};
use oncode_git::RepoSnapshot;

use crate::workbench::{register_feature, Workbench};
use crate::{core_bus, spawn_core};

#[derive(Debug, Clone, PartialEq)]
pub enum ReposEvent {
    /// This repository's snapshot was replaced.
    Changed(String),
    /// A branch, tag, or HEAD changed; the ref list and graph need to be reloaded.
    RefsChanged(String),
}

/// The Git status of a file, for the markers in the explorer and tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileMark {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

impl FileMark {
    /// The marker letter, as in VS Code.
    pub fn letter(self) -> &'static str {
        match self {
            Self::Modified => "M",
            Self::Added => "A",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Untracked => "U",
            Self::Conflicted => "!",
        }
    }
}

#[derive(Default)]
pub struct Repos {
    snapshots: HashMap<String, RepoSnapshot>,
    /// Markers per absolute path (with `/` separators), recomputed for every snapshot.
    marks: HashMap<String, HashMap<String, FileMark>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ReposEvent> for Repos {}

struct ReposGlobal(Entity<Repos>);

impl Global for ReposGlobal {}

/// The global repository store.
pub fn repos(cx: &App) -> Entity<Repos> {
    cx.global::<ReposGlobal>().0.clone()
}

fn join(root: &str, rel: &str) -> String {
    format!("{}/{}", root.trim_end_matches(['/', '\\']).replace('\\', "/"), rel)
}

/// Markers for all changed files in a snapshot. Conflicts win over everything else;
/// working tree changes win over staged ones.
pub fn marks_for(snapshot: &RepoSnapshot) -> HashMap<String, FileMark> {
    use oncode_git::ChangeKind;
    let kind_mark = |kind: &ChangeKind| match kind {
        ChangeKind::Added => FileMark::Added,
        ChangeKind::Deleted => FileMark::Deleted,
        ChangeKind::Renamed | ChangeKind::Copied => FileMark::Renamed,
        _ => FileMark::Modified,
    };
    let mut marks = HashMap::new();
    for change in &snapshot.staged {
        marks.insert(join(&snapshot.root, &change.path), kind_mark(&change.kind));
    }
    for change in &snapshot.unstaged {
        marks.insert(join(&snapshot.root, &change.path), kind_mark(&change.kind));
    }
    for path in &snapshot.untracked {
        marks.insert(join(&snapshot.root, path.trim_end_matches('/')), FileMark::Untracked);
    }
    for entry in &snapshot.conflicted {
        marks.insert(join(&snapshot.root, &entry.path), FileMark::Conflicted);
    }
    marks
}

impl Repos {
    /// The repository snapshot with this root; `C:\project` and `C:/project` are treated as the same.
    pub fn snapshot(&self, root: &str) -> Option<&RepoSnapshot> {
        self.snapshots.get(root).or_else(|| {
            let key = root_key(root);
            self.snapshots.iter().find(|(candidate, _)| root_key(candidate) == key).map(|(_, snapshot)| snapshot)
        })
    }

    pub fn roots(&self) -> Vec<String> {
        let mut roots: Vec<String> = self.snapshots.keys().cloned().collect();
        roots.sort();
        roots
    }

    /// The Git marker for an absolute path.
    pub fn mark(&self, path: &str) -> Option<FileMark> {
        let path = path.replace('\\', "/");
        self.marks.values().find_map(|marks| marks.get(&path).copied())
    }

    /// The marker for a folder: there are changes inside it.
    pub fn folder_mark(&self, dir: &str) -> Option<FileMark> {
        let prefix = format!("{}/", dir.replace('\\', "/").trim_end_matches('/'));
        let mut found = None;
        for marks in self.marks.values() {
            for (path, mark) in marks {
                if path.starts_with(&prefix) {
                    if *mark == FileMark::Conflicted {
                        return Some(FileMark::Conflicted);
                    }
                    found = Some(FileMark::Modified);
                }
            }
        }
        found
    }

    /// The number of changed files (for the badge in the activity bar).
    pub fn change_count(&self, root: &str) -> usize {
        self.snapshot(root).map_or(0, |s| s.staged.len() + s.unstaged.len() + s.untracked.len() + s.conflicted.len())
    }

    fn set(&mut self, root: String, snapshot: RepoSnapshot, cx: &mut Context<Self>) {
        if self.snapshots.get(&root) == Some(&snapshot) {
            return;
        }
        self.marks.insert(root.clone(), marks_for(&snapshot));
        self.snapshots.insert(root.clone(), snapshot);
        cx.emit(ReposEvent::Changed(root));
        cx.notify();
    }
}

pub(crate) fn install(cx: &mut App) {
    let bus = core_bus(cx);
    let store = cx.new(|cx| {
        let subscription = cx.subscribe(&bus, |this: &mut Repos, _, event: &CoreEvent, cx| match event {
            CoreEvent::RepoChanged { root, snapshot } => this.set(root.clone(), snapshot.clone(), cx),
            CoreEvent::RefsChanged { root } => cx.emit(ReposEvent::RefsChanged(root.clone())),
            _ => {}
        });
        Repos { _subscriptions: vec![subscription], ..Default::default() }
    });
    cx.set_global(ReposGlobal(store));
    register_feature(cx, setup);
}

/// Opens the project folder's repository (if there is one and it is trusted) so its snapshots start flowing.
fn setup(workbench: &mut Workbench, _: &mut Window, cx: &mut Context<Workbench>) {
    let env = workbench.env().clone();
    let Some(repo_root) = env.repo_root.clone() else { return };
    let (core, label) = (env.core.clone(), env.window.to_string());
    spawn_core(cx, async move {
        if let Err(error) = git::git_open(&core, &label, repo_root).await {
            eprintln!("oncode: failed to open the repository: {}", error.message);
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use oncode_core::root_key;

    #[test]
    fn roots_match_across_separators_and_trailing_slashes() {
        assert_eq!(root_key("/home/ana/project/"), "/home/ana/project");
        assert_ne!(root_key("/a/b"), root_key("/a/bc"));
        // On Windows `\` is the same as `/`, and case is not distinguished.
        assert_eq!(root_key(r"C:\Users\ana\project") == root_key("C:/Users/ana/project"), cfg!(windows));
        assert_eq!(root_key(r"c:\Project") == root_key("C:/project"), cfg!(windows));
    }
}
