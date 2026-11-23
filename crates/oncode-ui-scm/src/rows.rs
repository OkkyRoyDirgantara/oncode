//! Daftar perubahan diratakan menjadi baris-baris berketinggian sama, supaya bisa ditampilkan
//! dengan daftar virtual walaupun repositori punya ribuan berkas yang berubah.

use std::collections::HashMap;

use oncode_git::{CommitFile, ConflictEntry, FileChange, RepoSnapshot, Stash};

use crate::logic::{change_rows, ChangeRow};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Conflicts,
    Staged,
    Changes,
    Stashes,
}

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Self::Conflicts => "Merge Conflicts",
            Self::Staged => "Staged Changes",
            Self::Changes => "Changes",
            Self::Stashes => "Stashes",
        }
    }

    /// Bagian id elemen kepala kelompok (`scm-section-<slug>`).
    pub fn slug(self) -> &'static str {
        match self {
            Self::Conflicts => "conflicts",
            Self::Staged => "staged",
            Self::Changes => "changes",
            Self::Stashes => "stashes",
        }
    }

    /// Kelompok stash tertutup sampai dibuka; yang lain terbuka.
    pub fn open_by_default(self) -> bool {
        self != Self::Stashes
    }
}

/// Berkas-berkas sebuah stash yang dimuat saat barisnya dibuka.
#[derive(Debug, Clone, PartialEq)]
pub enum StashFiles {
    Loading,
    Loaded(Vec<CommitFile>),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Header { section: Section, count: usize, open: bool },
    Conflict(ConflictEntry),
    Staged(FileChange),
    Change(ChangeRow),
    /// "No changes. The working tree is clean."
    Clean,
    Stash { stash: Stash, expanded: bool },
    StashFile { stash: String, file: CommitFile },
    /// Keterangan di bawah stash yang dibuka (sedang memuat, kosong, atau galat).
    StashNote(String),
}

/// Kelompok mana yang terbuka dan stash mana yang sedang dibuka.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ViewState {
    toggled: HashMap<Section, bool>,
    /// Oid stash yang dibuka beserta berkasnya.
    pub expanded: HashMap<String, StashFiles>,
}

impl ViewState {
    pub fn is_open(&self, section: Section) -> bool {
        self.toggled.get(&section).copied().unwrap_or_else(|| section.open_by_default())
    }

    pub fn toggle(&mut self, section: Section) {
        let open = self.is_open(section);
        self.toggled.insert(section, !open);
    }

    pub fn set_open(&mut self, section: Section, open: bool) {
        self.toggled.insert(section, open);
    }

    /// Melupakan stash yang sudah tidak ada (oid-nya berubah setiap kali daftar stash bergeser).
    pub fn retain_stashes(&mut self, stashes: &[Stash]) {
        self.expanded.retain(|oid, _| stashes.iter().any(|stash| &stash.oid == oid));
    }
}

/// Menyusun baris-baris panel dari snapshot, daftar stash, dan keadaan tampilan.
pub fn build_rows(snapshot: &RepoSnapshot, stashes: &[Stash], view: &ViewState) -> Vec<Row> {
    let mut rows = Vec::new();
    let section = |rows: &mut Vec<Row>, section: Section, count: usize| {
        let open = view.is_open(section);
        rows.push(Row::Header { section, count, open });
        open
    };

    if !snapshot.conflicted.is_empty() && section(&mut rows, Section::Conflicts, snapshot.conflicted.len()) {
        rows.extend(snapshot.conflicted.iter().cloned().map(Row::Conflict));
    }
    if !snapshot.staged.is_empty() && section(&mut rows, Section::Staged, snapshot.staged.len()) {
        rows.extend(snapshot.staged.iter().cloned().map(Row::Staged));
    }
    let changes = change_rows(snapshot);
    if section(&mut rows, Section::Changes, changes.len()) {
        if changes.is_empty() && snapshot.staged.is_empty() && snapshot.conflicted.is_empty() {
            rows.push(Row::Clean);
        }
        rows.extend(changes.into_iter().map(Row::Change));
    }
    if !stashes.is_empty() && section(&mut rows, Section::Stashes, stashes.len()) {
        for stash in stashes {
            let files = view.expanded.get(&stash.oid);
            rows.push(Row::Stash { stash: stash.clone(), expanded: files.is_some() });
            match files {
                None => {}
                Some(StashFiles::Loading) => rows.push(Row::StashNote("Loading…".into())),
                Some(StashFiles::Failed(message)) => rows.push(Row::StashNote(message.clone())),
                Some(StashFiles::Loaded(files)) if files.is_empty() => rows.push(Row::StashNote("No changed files".into())),
                Some(StashFiles::Loaded(files)) => rows.extend(files.iter().cloned().map(|file| Row::StashFile { stash: stash.name.clone(), file })),
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use oncode_git::{ChangeKind, ConflictKind, Head};

    fn snapshot() -> RepoSnapshot {
        RepoSnapshot {
            root: "/repo".into(),
            head: Head::Branch { name: "main".into() },
            head_oid: None,
            upstream: None,
            operation: None,
            staged: Vec::new(),
            unstaged: Vec::new(),
            untracked: Vec::new(),
            conflicted: Vec::new(),
            stash_count: 0,
        }
    }

    fn change(path: &str) -> FileChange {
        FileChange { path: path.into(), orig_path: None, kind: ChangeKind::Modified, submodule: None }
    }

    fn stash(index: u32) -> Stash {
        Stash { index, name: format!("stash@{{{index}}}"), oid: format!("oid{index}"), message: format!("pesan {index}"), time: 0.0 }
    }

    fn kinds(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Header { section, count, open } => format!("{}:{count}:{}", section.slug(), if *open { "open" } else { "closed" }),
                Row::Conflict(c) => format!("conflict {}", c.path),
                Row::Staged(c) => format!("staged {}", c.path),
                Row::Change(c) => format!("change {}{}", c.path, if c.untracked { " (U)" } else { "" }),
                Row::Clean => "clean".into(),
                Row::Stash { stash, expanded } => format!("stash {}{}", stash.name, if *expanded { " +" } else { "" }),
                Row::StashFile { stash, file } => format!("stash-file {stash} {}", file.path),
                Row::StashNote(text) => format!("note {text}"),
            })
            .collect()
    }

    #[test]
    fn clean_tree_shows_only_the_changes_group_with_a_note() {
        assert_eq!(kinds(&build_rows(&snapshot(), &[], &ViewState::default())), ["changes:0:open", "clean"]);
    }

    #[test]
    fn groups_appear_in_order_and_only_when_they_have_files() {
        let mut s = snapshot();
        s.conflicted = vec![ConflictEntry { path: "c.txt".into(), kind: ConflictKind::BothModified }];
        s.staged = vec![change("b.txt")];
        s.unstaged = vec![change("a.txt")];
        s.untracked = vec!["baru.txt".into()];
        let rows = build_rows(&s, &[stash(0)], &ViewState::default());
        assert_eq!(kinds(&rows), [
            "conflicts:1:open",
            "conflict c.txt",
            "staged:1:open",
            "staged b.txt",
            "changes:2:open",
            "change a.txt",
            "change baru.txt (U)",
            "stashes:1:closed",
        ]);

        // Hanya ada yang ter-stage: kelompok Changes tetap ada tetapi tanpa catatan "clean".
        let mut s = snapshot();
        s.staged = vec![change("b.txt")];
        assert_eq!(kinds(&build_rows(&s, &[], &ViewState::default())), ["staged:1:open", "staged b.txt", "changes:0:open"]);
    }

    #[test]
    fn collapsing_a_group_hides_its_rows() {
        let mut s = snapshot();
        s.staged = vec![change("b.txt")];
        s.unstaged = vec![change("a.txt")];
        let mut view = ViewState::default();
        view.toggle(Section::Staged);
        assert_eq!(kinds(&build_rows(&s, &[], &view)), ["staged:1:closed", "changes:1:open", "change a.txt"]);
        view.toggle(Section::Staged);
        view.toggle(Section::Changes);
        assert_eq!(kinds(&build_rows(&s, &[], &view)), ["staged:1:open", "staged b.txt", "changes:1:closed"]);
    }

    #[test]
    fn expanded_stash_lists_its_files() {
        let stashes = [stash(0), stash(1)];
        let mut view = ViewState::default();
        view.toggle(Section::Stashes);
        assert_eq!(kinds(&build_rows(&snapshot(), &stashes, &view))[2..], ["stashes:2:open", "stash stash@{0}", "stash stash@{1}"]);

        view.expanded.insert("oid1".into(), StashFiles::Loading);
        assert_eq!(kinds(&build_rows(&snapshot(), &stashes, &view))[3..], ["stash stash@{0}", "stash stash@{1} +", "note Loading…"]);

        let file = CommitFile { path: "src/a.rs".into(), orig_path: None, kind: ChangeKind::Modified, additions: Some(1), deletions: Some(0) };
        view.expanded.insert("oid1".into(), StashFiles::Loaded(vec![file]));
        assert_eq!(kinds(&build_rows(&snapshot(), &stashes, &view))[4..], ["stash stash@{1} +", "stash-file stash@{1} src/a.rs"]);

        view.expanded.insert("oid0".into(), StashFiles::Loaded(Vec::new()));
        view.expanded.insert("oid1".into(), StashFiles::Failed("gagal".into()));
        assert_eq!(kinds(&build_rows(&snapshot(), &stashes, &view))[3..], ["stash stash@{0} +", "note No changed files", "stash stash@{1} +", "note gagal"]);

        // Stash yang sudah hilang dilupakan.
        view.retain_stashes(&stashes[..1]);
        assert_eq!(view.expanded.keys().collect::<Vec<_>>(), ["oid0"]);
    }
}
