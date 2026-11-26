//! Pure explorer logic: paths, tree state, flattening into rows, name validation, Git decorations,
//! and tab rewriting. It touches neither GPUI nor the backend so it is easy to test.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use oncode_git::{ChangeKind, RepoSnapshot};
use oncode_ui::{file_tab, FileMark, TabData};
use oncode_workspace::DirEntry;

/// The height of one tree row and the indent per level, in pixels.
pub const ROW_HEIGHT: f32 = 22.;
pub const INDENT: f32 = 12.;

// ----- Path -----

fn separator_of(path: &str) -> char {
    if path.contains('\\') && !path.contains('/') {
        '\\'
    } else {
        '/'
    }
}

fn is_separator(c: char) -> bool {
    c == '/' || c == '\\'
}

fn strip_trailing(path: &str) -> &str {
    let trimmed = path.trim_end_matches(is_separator);
    if trimmed.is_empty() {
        // The file system root ("/") is kept.
        let first = path.chars().next().map_or(0, char::len_utf8);
        &path[..first]
    } else {
        trimmed
    }
}

/// Joins `dir` with a forward-slash relative path, using the separator that `dir` uses.
pub fn join_path(dir: &str, relative: &str) -> String {
    let sep = separator_of(dir);
    let base = strip_trailing(dir);
    let tail: String = relative.chars().map(|c| if is_separator(c) { sep } else { c }).collect();
    if base.ends_with(sep) {
        format!("{base}{tail}")
    } else {
        format!("{base}{sep}{tail}")
    }
}

/// True if `path` equals `ancestor` or is inside it.
pub fn is_same_or_under(path: &str, ancestor: &str) -> bool {
    let base = strip_trailing(ancestor);
    if path == base || path == ancestor {
        return true;
    }
    let Some(rest) = path.strip_prefix(base) else { return false };
    rest.starts_with(is_separator) || base.ends_with(is_separator)
}

/// The folders from `root` down to the direct parent of `path` (ordered outermost to innermost). Empty if `path` is outside `root`.
pub fn ancestor_chain(root: &str, path: &str) -> Vec<String> {
    let base = strip_trailing(root);
    if path == base || !is_same_or_under(path, base) {
        return Vec::new();
    }
    let sep = separator_of(root);
    let segments: Vec<&str> = path[base.len()..].split(is_separator).filter(|s| !s.is_empty()).collect();
    let mut chain = vec![base.to_string()];
    let mut current = base.to_string();
    for segment in segments.iter().take(segments.len().saturating_sub(1)) {
        if !current.ends_with(sep) {
            current.push(sep);
        }
        current.push_str(segment);
        chain.push(current.clone());
    }
    chain
}

/// The new path for `path` after `from` is moved to `to`; `None` if `path` does not move along.
pub fn moved_path(path: &str, from: &str, to: &str) -> Option<String> {
    if path == from {
        return Some(to.to_string());
    }
    if !is_same_or_under(path, from) {
        return None;
    }
    Some(format!("{to}{}", &path[strip_trailing(from).len()..]))
}

/// True if both paths point to the same place, regardless of separator and trailing slash.
pub fn same_path(a: &str, b: &str) -> bool {
    strip_trailing(a).replace('\\', "/") == strip_trailing(b).replace('\\', "/")
}

/// True if `source` may be moved into the folder `dest_dir` by drag and drop: it must be inside
/// `root` (the project folder itself cannot be moved), must not go into itself or into a folder
/// inside itself, and there is no point moving it to the folder it is already in.
pub fn can_move_into(root: &str, source: &str, dest_dir: &str) -> bool {
    !same_path(source, root) && is_same_or_under(source, root) && !is_same_or_under(dest_dir, source) && !same_path(dir_name(source), dest_dir)
}

/// The last file or folder name of a path.
pub fn base_name(path: &str) -> &str {
    let trimmed = strip_trailing(path);
    match trimmed.rfind(is_separator) {
        Some(index) => &trimmed[index + 1..],
        None => trimmed,
    }
}

/// The parent folder of a path; empty if there is none.
pub fn dir_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches(is_separator);
    match trimmed.rfind(is_separator) {
        Some(0) => &trimmed[..1],
        Some(index) => &trimmed[..index],
        None => "",
    }
}

/// The path relative to the project folder, with forward slashes. A path outside the project is returned as is.
pub fn relative_path(root: &str, absolute: &str) -> String {
    let base = root.trim_end_matches(is_separator);
    if absolute == base {
        return String::new();
    }
    match absolute.strip_prefix(base) {
        Some(rest) if rest.starts_with(is_separator) => rest[1..].replace('\\', "/"),
        _ => absolute.to_string(),
    }
}

/// A file tab that points to `from` (or a file inside it) is rewritten to `to`; any other tab yields `None`.
pub fn rewrite_file_tab(tab: &TabData, from: &str, to: &str) -> Option<TabData> {
    if tab.kind != "file" {
        return None;
    }
    // The tab path is already in `file_path` form (`\` on Windows); `from`/`to` are normalized first.
    let (from, to) = (oncode_ui::editor::file_path(from), oncode_ui::editor::file_path(to));
    let moved = moved_path(tab.param_str("path")?, &from, &to)?;
    let mut next = file_tab(&moved);
    next.preview = tab.preview;
    let mut params = tab.params.clone();
    params["path"] = serde_json::Value::String(moved);
    next.params = params;
    Some(next)
}

// ----- Tree state -----

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DirState {
    /// The folder contents; `None` until it has ever loaded successfully.
    pub entries: Option<Vec<DirEntry>>,
    pub loading: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Entry { entry: DirEntry, depth: usize, expanded: bool, parent: String },
    /// A note below an open folder: loading, empty, or failed to read.
    Note { depth: usize, parent: String, text: String, error: bool },
}

impl Row {
    pub fn entry(&self) -> Option<&DirEntry> {
        match self {
            Row::Entry { entry, .. } => Some(entry),
            Row::Note { .. } => None,
        }
    }

    pub fn path(&self) -> Option<&str> {
        self.entry().map(|entry| entry.path.as_str())
    }

    pub fn depth(&self) -> usize {
        match self {
            Row::Entry { depth, .. } | Row::Note { depth, .. } => *depth,
        }
    }
}

/// The result of flattening the tree.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Flat {
    /// The visible rows (only the contents of open folders).
    pub rows: Vec<Row>,
    /// Open and visible folders whose contents have never been requested; the caller loads them.
    pub missing: Vec<String>,
}

/// The contents of loaded folders, the open folders, and the selected row.
///
/// Loading is run by the caller: [`TreeState::begin_load`] marks a folder as loading and
/// issues a request number, [`TreeState::finish_load`] applies its response. A response to a
/// request that has been superseded by a newer one is ignored.
#[derive(Debug, Clone, Default)]
pub struct TreeState {
    pub root: String,
    pub dirs: HashMap<String, DirState>,
    pub expanded: HashSet<String>,
    pub selected: Option<String>,
    /// The latest request per folder; an older response must not overwrite a newer one.
    latest: HashMap<String, u64>,
    counter: u64,
}

impl TreeState {
    pub fn new(root: impl Into<String>) -> Self {
        Self { root: root.into(), ..Default::default() }
    }

    /// Whether this folder is open (the root is always open).
    pub fn is_open(&self, dir: &str) -> bool {
        dir == self.root || self.expanded.contains(dir)
    }

    pub fn entries(&self, dir: &str) -> Option<&[DirEntry]> {
        self.dirs.get(dir)?.entries.as_deref()
    }

    /// The entry for a path, looked up in the contents of its parent folder.
    pub fn entry(&self, path: &str) -> Option<&DirEntry> {
        self.entries(dir_name(path))?.iter().find(|entry| entry.path == path)
    }

    pub fn is_loading(&self) -> bool {
        self.dirs.values().any(|dir| dir.loading)
    }

    /// Marks a folder as loading; the old contents stay displayed until the response arrives.
    pub fn begin_load(&mut self, path: &str) -> u64 {
        self.counter += 1;
        self.latest.insert(path.to_string(), self.counter);
        self.dirs.entry(path.to_string()).or_default().loading = true;
        self.counter
    }

    /// Applies the response to a request. The result is `true` if the folder contents are now available.
    pub fn finish_load(&mut self, path: &str, request: u64, result: Result<Vec<DirEntry>, String>) -> bool {
        if self.latest.get(path) != Some(&request) {
            return false;
        }
        self.latest.remove(path);
        // A folder that has meanwhile disappeared from its parent's contents is not revived by a late response.
        if path != self.root {
            if let Some(siblings) = self.entries(dir_name(path)) {
                if !siblings.iter().any(|entry| entry.path == path) {
                    self.dirs.remove(path);
                    return false;
                }
            }
        }
        match result {
            Ok(entries) => {
                let previous = self.dirs.get_mut(path).and_then(|dir| dir.entries.take());
                self.forget_removed(previous.as_deref(), &entries);
                self.dirs.insert(path.to_string(), DirState { entries: Some(entries), loading: false, error: None });
                true
            }
            Err(error) => {
                self.dirs.insert(path.to_string(), DirState { entries: None, loading: false, error: Some(error) });
                false
            }
        }
    }

    /// An entry that disappears from a folder takes the state below it (cache, open, selected) along with it.
    fn forget_removed(&mut self, previous: Option<&[DirEntry]>, entries: &[DirEntry]) {
        let Some(previous) = previous else { return };
        let removed: Vec<&str> = previous.iter().filter(|old| !entries.iter().any(|new| new.path == old.path)).map(|old| old.path.as_str()).collect();
        if removed.is_empty() {
            return;
        }
        let gone = |key: &str| removed.iter().any(|path| is_same_or_under(key, path));
        self.dirs.retain(|key, _| !gone(key));
        self.latest.retain(|key, _| !gone(key));
        self.expanded.retain(|key| !gone(key));
        if self.selected.as_deref().is_some_and(gone) {
            self.selected = None;
        }
    }

    pub fn expand(&mut self, path: &str) {
        self.expanded.insert(path.to_string());
    }

    pub fn collapse(&mut self, path: &str) {
        self.expanded.remove(path);
    }

    pub fn collapse_all(&mut self) {
        self.expanded.clear();
    }

    /// Refreshes `path` and all folders inside it: the cache of closed folders is dropped (loaded again
    /// when opened) and open folders are returned to be reloaded.
    pub fn reload_under(&mut self, path: &str) -> Vec<String> {
        let root = self.root.clone();
        let expanded = &self.expanded;
        let open = |key: &str| key == root || expanded.contains(key);
        self.dirs.retain(|key, _| open(key) || !is_same_or_under(key, path));
        self.latest.retain(|key, _| open(key) || !is_same_or_under(key, path));
        let mut targets: Vec<String> = std::iter::once(&root).chain(expanded.iter()).filter(|key| is_same_or_under(key, path)).cloned().collect();
        targets.sort();
        targets.dedup();
        targets
    }

    /// Reacts to outside changes: open folders that contain a changed path are returned
    /// to be reloaded; closed folders that were loaded before just have their cache dropped.
    pub fn fs_changes<'a>(&mut self, paths: impl IntoIterator<Item = &'a str>, overflow: bool) -> Vec<String> {
        if overflow {
            let root = self.root.clone();
            return self.reload_under(&root);
        }
        let mut reload: Vec<String> = Vec::new();
        for path in paths {
            // Its parent's contents changed; the path itself may be a folder whose contents changed.
            for dir in [dir_name(path), path] {
                if !self.dirs.contains_key(dir) {
                    continue;
                }
                if self.is_open(dir) {
                    if !reload.iter().any(|known| known == dir) {
                        reload.push(dir.to_string());
                    }
                } else {
                    self.dirs.remove(dir);
                    self.latest.remove(dir);
                }
            }
        }
        reload
    }

    /// After a rename: the open and selected state below `from` moves along to `to`.
    /// The cache below `from` is dropped because its entries still hold the old path.
    pub fn move_state(&mut self, from: &str, to: &str) {
        self.dirs.retain(|key, _| !is_same_or_under(key, from));
        self.latest.retain(|key, _| !is_same_or_under(key, from));
        self.expanded = self.expanded.iter().map(|key| moved_path(key, from, to).unwrap_or_else(|| key.clone())).collect();
        if let Some(moved) = self.selected.as_deref().and_then(|selected| moved_path(selected, from, to)) {
            self.selected = Some(moved);
        }
    }

    /// Flattens the tree into a list of visible rows.
    pub fn flatten(&self) -> Flat {
        let mut flat = Flat::default();
        self.visit(&self.root, 0, &mut flat);
        flat
    }

    fn visit(&self, dir: &str, depth: usize, flat: &mut Flat) {
        let note = |text: &str, error: bool| Row::Note { depth, parent: dir.to_string(), text: text.to_string(), error };
        let Some(state) = self.dirs.get(dir) else {
            flat.missing.push(dir.to_string());
            flat.rows.push(note("Loading…", false));
            return;
        };
        let Some(entries) = &state.entries else {
            flat.rows.push(match &state.error {
                Some(error) => note(error, true),
                None => note("Loading…", false),
            });
            return;
        };
        if entries.is_empty() {
            flat.rows.push(note("Empty", false));
            return;
        }
        for entry in entries {
            let open = entry.is_dir && self.expanded.contains(&entry.path);
            flat.rows.push(Row::Entry { entry: entry.clone(), depth, expanded: open, parent: dir.to_string() });
            if open {
                self.visit(&entry.path, depth + 1, flat);
            }
        }
    }
}

// ----- Name validation -----

#[derive(Debug, Clone, Copy, Default)]
pub struct NameOptions<'a> {
    /// Allow `a/b/c.txt`: the intermediate folders are created at the same time.
    pub nested: bool,
    /// The current name (rename): not considered a clash with itself.
    pub current: Option<&'a str>,
}

/// Checks a new file/folder name. Returns an error message, or `None` if valid.
pub fn validate_entry_name(input: &str, siblings: Option<&[DirEntry]>, options: NameOptions) -> Option<String> {
    if input.trim().is_empty() {
        return Some("Enter a name.".into());
    }
    if input.contains('\0') {
        return Some("The name contains an invalid character.".into());
    }
    if input.contains('\\') {
        return Some("Backslashes are not allowed in names.".into());
    }
    if !options.nested && input.contains('/') {
        return Some("Names cannot contain \"/\".".into());
    }
    let segments: Vec<&str> = input.split('/').collect();
    for segment in &segments {
        if segment.is_empty() {
            return Some(if input.starts_with('/') { "Enter a path relative to this folder." } else { "Path segments cannot be empty." }.into());
        }
        if *segment == "." || *segment == ".." {
            return Some("\".\" and \"..\" cannot be used as names.".into());
        }
        if *segment != segment.trim() {
            return Some("Names cannot start or end with a space.".into());
        }
    }
    let first = segments[0];
    let clash = siblings.and_then(|siblings| siblings.iter().find(|sibling| sibling.name == first));
    if let Some(clash) = clash {
        if Some(first) != options.current {
            if segments.len() == 1 {
                return Some(format!("A file or folder named \"{first}\" already exists in this location."));
            }
            if !clash.is_dir {
                return Some(format!("\"{first}\" is a file, so nothing can be created inside it."));
            }
        }
    }
    None
}

// ----- Git decorations -----

/// The status caption for the tooltip.
pub fn mark_title(mark: FileMark) -> &'static str {
    match mark {
        FileMark::Modified => "Modified",
        FileMark::Added => "Added",
        FileMark::Untracked => "Untracked",
        FileMark::Deleted => "Deleted",
        FileMark::Renamed => "Renamed",
        FileMark::Conflicted => "Conflict",
    }
}

fn kind_mark(kind: &ChangeKind) -> FileMark {
    match kind {
        ChangeKind::Added | ChangeKind::Copied => FileMark::Added,
        ChangeKind::Modified | ChangeKind::TypeChanged => FileMark::Modified,
        ChangeKind::Deleted => FileMark::Deleted,
        ChangeKind::Renamed => FileMark::Renamed,
    }
}

/// If one file has several statuses (e.g. staged and unstaged), the highest-ranked one is shown.
fn file_rank(mark: FileMark) -> u8 {
    match mark {
        FileMark::Modified => 1,
        FileMark::Renamed => 2,
        FileMark::Added => 3,
        FileMark::Untracked => 4,
        FileMark::Deleted => 5,
        FileMark::Conflicted => 6,
    }
}

/// Folder color: a conflict beats a change, a change beats a new file.
fn folder_mark(mark: FileMark) -> FileMark {
    match mark {
        FileMark::Conflicted => FileMark::Conflicted,
        FileMark::Modified | FileMark::Renamed | FileMark::Deleted => FileMark::Modified,
        FileMark::Added => FileMark::Added,
        FileMark::Untracked => FileMark::Untracked,
    }
}

fn folder_rank(mark: FileMark) -> u8 {
    match mark {
        FileMark::Untracked => 1,
        FileMark::Added => 2,
        FileMark::Modified | FileMark::Renamed | FileMark::Deleted => 3,
        FileMark::Conflicted => 4,
    }
}

fn slashes(path: &str) -> Cow<'_, str> {
    if path.contains('\\') {
        Cow::Owned(path.replace('\\', "/"))
    } else {
        Cow::Borrowed(path)
    }
}

/// Git status per absolute path. Keys use forward slashes on all platforms.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GitDecorations {
    /// The status per file.
    pub files: HashMap<String, FileMark>,
    /// The combined status per folder that contains changes.
    pub folders: HashMap<String, FileMark>,
}

/// Maps a repository snapshot to absolute paths. Paths in the snapshot are relative to the
/// repository root (`snapshot.root`), which is not necessarily the same as the project folder.
pub fn build_decorations(snapshot: Option<&RepoSnapshot>) -> GitDecorations {
    let mut decorations = GitDecorations::default();
    let Some(snapshot) = snapshot else { return decorations };
    let root = slashes(&snapshot.root).into_owned();

    let files = &mut decorations.files;
    let mut mark = |relative: &str, status: FileMark| {
        let clean = relative.trim_end_matches('/');
        if clean.is_empty() {
            return;
        }
        let absolute = join_path(&root, clean);
        match files.get(&absolute) {
            Some(previous) if file_rank(status) <= file_rank(*previous) => {}
            _ => {
                files.insert(absolute, status);
            }
        }
    };
    for change in &snapshot.staged {
        mark(&change.path, kind_mark(&change.kind));
    }
    for change in &snapshot.unstaged {
        mark(&change.path, kind_mark(&change.kind));
    }
    for path in &snapshot.untracked {
        mark(path, FileMark::Untracked);
    }
    for conflict in &snapshot.conflicted {
        mark(&conflict.path, FileMark::Conflicted);
    }

    for (path, status) in &decorations.files {
        let inherited = folder_mark(*status);
        // All folders between the repository root and the file are colored too.
        for folder in ancestor_chain(&root, path).into_iter().skip(1) {
            match decorations.folders.get(&folder) {
                Some(previous) if folder_rank(inherited) <= folder_rank(*previous) => {}
                _ => {
                    decorations.folders.insert(folder, inherited);
                }
            }
        }
    }
    decorations
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntryDecoration {
    pub status: FileMark,
    /// The status letter; empty for folders (color and dot only).
    pub letter: &'static str,
    pub title: String,
}

pub fn decoration_for(decorations: &GitDecorations, path: &str, is_dir: bool) -> Option<EntryDecoration> {
    let path = slashes(path);
    let own = decorations.files.get(path.as_ref()).copied();
    if !is_dir {
        return own.map(|status| EntryDecoration { status, letter: status.letter(), title: mark_title(status).to_string() });
    }
    // Folder: the combination of its contents, or its own status (e.g. a submodule or an untracked folder).
    let status = decorations.folders.get(path.as_ref()).copied().or(own)?;
    Some(EntryDecoration { status, letter: "", title: format!("Contains {} files", mark_title(status).to_lowercase()) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use oncode_git::{ConflictEntry, ConflictKind, FileChange, Head};
    use serde_json::json;

    fn dir(path: &str) -> DirEntry {
        DirEntry { name: base_name(path).to_string(), path: path.to_string(), is_dir: true, is_symlink: false, ignored: false }
    }

    fn file(path: &str) -> DirEntry {
        DirEntry { is_dir: false, ..dir(path) }
    }

    fn loaded(entries: Vec<DirEntry>) -> DirState {
        DirState { entries: Some(entries), loading: false, error: None }
    }

    fn tree(dirs: Vec<(&str, DirState)>, expanded: &[&str]) -> TreeState {
        let mut tree = TreeState::new("/ws");
        tree.dirs = dirs.into_iter().map(|(path, state)| (path.to_string(), state)).collect();
        tree.expanded = expanded.iter().map(|path| path.to_string()).collect();
        tree
    }

    fn shape(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| {
                let text = match row {
                    Row::Entry { entry, expanded, .. } => format!("{}{}", entry.name, if !entry.is_dir { "" } else if *expanded { "/-" } else { "/+" }),
                    Row::Note { text, .. } => format!("({text})"),
                };
                format!("{}{text}", "  ".repeat(row.depth()))
            })
            .collect()
    }

    // ----- Path -----

    #[test]
    fn joins_with_the_separator_the_directory_uses() {
        assert_eq!(join_path("/ws", "a/b.txt"), "/ws/a/b.txt");
        assert_eq!(join_path("/ws/", "a"), "/ws/a");
        assert_eq!(join_path("/", "a"), "/a");
        assert_eq!(join_path("C:\\ws", "a/b.txt"), "C:\\ws\\a\\b.txt");
    }

    #[test]
    fn tells_descendants_from_look_alike_siblings() {
        assert!(is_same_or_under("/ws/src", "/ws/src"));
        assert!(is_same_or_under("/ws/src/a.ts", "/ws/src"));
        assert!(!is_same_or_under("/ws/src2/a.ts", "/ws/src"));
        assert!(!is_same_or_under("/ws", "/ws/src"));
        assert!(is_same_or_under("/anything", "/"));
    }

    #[test]
    fn lists_the_folders_between_the_root_and_a_path() {
        assert_eq!(ancestor_chain("/ws", "/ws/a/b/c.txt"), ["/ws", "/ws/a", "/ws/a/b"]);
        assert_eq!(ancestor_chain("/ws", "/ws/c.txt"), ["/ws"]);
        assert!(ancestor_chain("/ws", "/ws").is_empty());
        assert!(ancestor_chain("/ws", "/elsewhere/c.txt").is_empty());
    }

    #[test]
    fn splits_names_parents_and_relative_paths() {
        assert_eq!(base_name("/ws/src/a.ts"), "a.ts");
        assert_eq!(base_name("/ws/src/"), "src");
        assert_eq!(base_name("C:\\ws\\a.ts"), "a.ts");
        assert_eq!(dir_name("/ws/src/a.ts"), "/ws/src");
        assert_eq!(dir_name("/a"), "/");
        assert_eq!(dir_name("a"), "");
        assert_eq!(relative_path("/ws", "/ws/src/a.ts"), "src/a.ts");
        assert_eq!(relative_path("/ws/", "/ws"), "");
        assert_eq!(relative_path("C:\\ws", "C:\\ws\\src\\a.ts"), "src/a.ts");
        assert_eq!(relative_path("/ws", "/ws2/a.ts"), "/ws2/a.ts");
    }

    // ----- Flattening -----

    fn sample() -> Vec<(&'static str, DirState)> {
        vec![
            ("/ws", loaded(vec![dir("/ws/docs"), dir("/ws/src"), file("/ws/README.md")])),
            ("/ws/src", loaded(vec![dir("/ws/src/app"), file("/ws/src/main.ts")])),
            ("/ws/src/app", loaded(vec![file("/ws/src/app/store.ts")])),
            ("/ws/docs", loaded(vec![file("/ws/docs/hidden-until-expanded.md")])),
        ]
    }

    #[test]
    fn shows_only_the_root_listing_when_nothing_is_expanded() {
        assert_eq!(shape(&tree(sample(), &[]).flatten().rows), ["docs/+", "src/+", "README.md"]);
    }

    #[test]
    fn nests_the_children_of_expanded_folders_in_place_with_depth() {
        let rows = tree(sample(), &["/ws/src", "/ws/src/app"]).flatten().rows;
        assert_eq!(shape(&rows), ["docs/+", "src/-", "  app/-", "    store.ts", "  main.ts", "README.md"]);
        let store = rows.iter().find(|row| row.path() == Some("/ws/src/app/store.ts")).unwrap();
        assert!(matches!(store, Row::Entry { depth: 2, parent, .. } if parent == "/ws/src/app"));
    }

    #[test]
    fn ignores_the_expanded_flag_of_a_folder_whose_ancestor_is_collapsed() {
        let flat = tree(sample(), &["/ws/src/app"]).flatten();
        assert_eq!(shape(&flat.rows), ["docs/+", "src/+", "README.md"]);
        assert!(flat.missing.is_empty());
    }

    #[test]
    fn puts_a_note_under_folders_that_are_loading_empty_or_unreadable() {
        let states = vec![
            ("/ws", loaded(vec![dir("/ws/empty"), dir("/ws/locked"), dir("/ws/pending"), dir("/ws/unknown")])),
            ("/ws/empty", loaded(vec![])),
            ("/ws/locked", DirState { entries: None, loading: false, error: Some("Permission denied".into()) }),
            ("/ws/pending", DirState { entries: None, loading: true, error: None }),
        ];
        let flat = tree(states, &["/ws/empty", "/ws/locked", "/ws/pending", "/ws/unknown"]).flatten();
        assert_eq!(shape(&flat.rows), ["empty/-", "  (Empty)", "locked/-", "  (Permission denied)", "pending/-", "  (Loading…)", "unknown/-", "  (Loading…)"]);
        let tones: Vec<bool> = flat.rows.iter().filter_map(|row| if let Row::Note { error, .. } = row { Some(*error) } else { None }).collect();
        assert_eq!(tones, [false, true, false, false]);
        // Only folders that have never been requested need to be loaded by the caller.
        assert_eq!(flat.missing, ["/ws/unknown"]);
    }

    #[test]
    fn keeps_showing_the_previous_listing_while_a_folder_reloads() {
        let states = vec![("/ws", DirState { entries: Some(vec![file("/ws/a.txt")]), loading: true, error: None })];
        assert_eq!(shape(&tree(states, &[]).flatten().rows), ["a.txt"]);
    }

    #[test]
    fn an_unloaded_root_is_reported_missing() {
        let flat = TreeState::new("/ws").flatten();
        assert_eq!(shape(&flat.rows), ["(Loading…)"]);
        assert_eq!(flat.missing, ["/ws"]);
    }

    // ----- Loading -----

    #[test]
    fn a_newer_request_wins_over_a_late_answer() {
        let mut tree = TreeState::new("/ws");
        let first = tree.begin_load("/ws");
        let second = tree.begin_load("/ws");
        assert!(!tree.finish_load("/ws", first, Ok(vec![file("/ws/old.txt")])));
        assert!(tree.dirs["/ws"].loading && tree.dirs["/ws"].entries.is_none());
        assert!(tree.finish_load("/ws", second, Ok(vec![file("/ws/new.txt")])));
        assert_eq!(shape(&tree.flatten().rows), ["new.txt"]);
        assert!(!tree.is_loading());
    }

    #[test]
    fn a_failed_load_keeps_the_error_for_the_folder() {
        let mut tree = tree(vec![("/ws", loaded(vec![dir("/ws/locked")]))], &["/ws/locked"]);
        let request = tree.begin_load("/ws/locked");
        assert!(!tree.finish_load("/ws/locked", request, Err("Permission denied".into())));
        assert_eq!(shape(&tree.flatten().rows), ["locked/-", "  (Permission denied)"]);
        assert!(tree.flatten().missing.is_empty(), "a failed folder is not requested over and over");
    }

    #[test]
    fn drops_expansion_and_selection_under_an_entry_that_disappeared() {
        let mut tree = tree(sample(), &["/ws/src", "/ws/src/app"]);
        tree.selected = Some("/ws/src/app/store.ts".into());
        let request = tree.begin_load("/ws/src");
        assert!(tree.finish_load("/ws/src", request, Ok(vec![file("/ws/src/main.ts")])));
        assert!(!tree.dirs.contains_key("/ws/src/app"));
        assert!(!tree.expanded.contains("/ws/src/app") && tree.expanded.contains("/ws/src"));
        assert_eq!(tree.selected, None);
    }

    #[test]
    fn a_late_answer_does_not_revive_a_folder_that_left_its_parent() {
        let mut tree = tree(sample(), &["/ws/src"]);
        let late = tree.begin_load("/ws/src");
        let parent = tree.begin_load("/ws");
        assert!(tree.finish_load("/ws", parent, Ok(vec![file("/ws/README.md")])));
        assert!(!tree.finish_load("/ws/src", late, Ok(vec![file("/ws/src/main.ts")])));
        assert!(!tree.dirs.contains_key("/ws/src"));
        assert_eq!(shape(&tree.flatten().rows), ["README.md"]);
    }

    // ----- Outside changes -----

    #[test]
    fn reloads_only_the_open_folder_that_contains_the_changed_path() {
        let mut tree = tree(sample(), &["/ws/src"]);
        tree.selected = Some("/ws/src/main.ts".into());
        assert_eq!(tree.fs_changes(["/ws/src/new.ts"], false), ["/ws/src"]);
        assert!(tree.expanded.contains("/ws/src"));
        assert_eq!(tree.selected.as_deref(), Some("/ws/src/main.ts"));
    }

    #[test]
    fn coalesces_a_burst_into_one_reload_per_folder() {
        let mut tree = tree(sample(), &["/ws/src"]);
        let mut targets = tree.fs_changes(["/ws/src/a.ts", "/ws/src/b.ts", "/ws/README.md", "/ws/src/a.ts"], false);
        targets.sort();
        assert_eq!(targets, ["/ws", "/ws/src"]);
    }

    #[test]
    fn forgets_a_cached_but_collapsed_folder_instead_of_reloading_it() {
        let mut tree = tree(sample(), &[]);
        assert!(tree.fs_changes(["/ws/docs/out.bin"], false).is_empty());
        assert!(!tree.dirs.contains_key("/ws/docs"));
        assert!(tree.dirs.contains_key("/ws/src"), "other folders are untouched");
    }

    #[test]
    fn reloads_the_root_and_every_expanded_folder_on_overflow() {
        let mut tree = tree(sample(), &["/ws/src"]);
        assert_eq!(tree.fs_changes([], true), ["/ws", "/ws/src"]);
        assert!(!tree.dirs.contains_key("/ws/docs") && !tree.dirs.contains_key("/ws/src/app"), "the cache of closed folders is dropped");
    }

    #[test]
    fn reload_under_keeps_folders_outside_the_subtree() {
        let mut tree = tree(sample(), &["/ws/src"]);
        assert_eq!(tree.reload_under("/ws/src"), ["/ws/src"]);
        assert!(tree.dirs.contains_key("/ws/docs"));
        assert!(!tree.dirs.contains_key("/ws/src/app"));
    }

    // ----- Rename -----

    #[test]
    fn move_state_carries_expansion_and_selection_to_the_new_path() {
        let mut tree = tree(sample(), &["/ws/src", "/ws/src/app", "/ws/docs"]);
        tree.selected = Some("/ws/src/app/store.ts".into());
        tree.move_state("/ws/src", "/ws/lib");
        let mut expanded: Vec<&str> = tree.expanded.iter().map(String::as_str).collect();
        expanded.sort();
        assert_eq!(expanded, ["/ws/docs", "/ws/lib", "/ws/lib/app"]);
        assert_eq!(tree.selected.as_deref(), Some("/ws/lib/app/store.ts"));
        assert!(!tree.dirs.contains_key("/ws/src") && !tree.dirs.contains_key("/ws/src/app"));
        assert!(tree.dirs.contains_key("/ws/docs"));
    }

    #[test]
    fn moves_a_path_only_when_it_is_the_renamed_entry_or_inside_it() {
        assert_eq!(moved_path("/ws/a.ts", "/ws/a.ts", "/ws/b.ts").as_deref(), Some("/ws/b.ts"));
        assert_eq!(moved_path("/ws/src/deep/a.ts", "/ws/src", "/ws/lib").as_deref(), Some("/ws/lib/deep/a.ts"));
        assert_eq!(moved_path("/ws/src2/a.ts", "/ws/src", "/ws/lib"), None);
        assert_eq!(moved_path("/ws/a.ts.bak", "/ws/a.ts", "/ws/b.ts"), None);
    }

    /// A path in the form tabs use (`\` on Windows).
    fn tab_path(path: &str) -> String {
        oncode_ui::editor::file_path(path)
    }

    #[test]
    fn rewrites_id_title_description_and_path_of_a_renamed_file_tab() {
        let mut tab = file_tab("/ws/a.ts");
        tab.preview = true;
        let next = rewrite_file_tab(&tab, "/ws/a.ts", "/ws/b.ts").unwrap();
        let moved = tab_path("/ws/b.ts");
        assert_eq!(next.id, format!("file:{moved}"));
        assert_eq!(next.kind, "file");
        assert_eq!(next.title, "b.ts");
        assert_eq!(next.description.as_deref(), Some(moved.as_str()));
        assert_eq!(next.params, json!({ "path": moved }));
        assert!(next.preview);
    }

    #[test]
    fn rewrites_tabs_of_files_under_a_renamed_folder_and_keeps_their_file_names() {
        let next = rewrite_file_tab(&file_tab("/ws/src/deep/a.ts"), "/ws/src", "/ws/lib").unwrap();
        let moved = tab_path("/ws/lib/deep/a.ts");
        assert_eq!((next.id.as_str(), next.title.as_str()), (format!("file:{moved}").as_str(), "a.ts"));
        assert_eq!(next.param_str("path"), Some(moved.as_str()));
    }

    #[test]
    fn leaves_unrelated_tabs_and_other_tab_kinds_alone() {
        assert_eq!(rewrite_file_tab(&file_tab("/ws/other.ts"), "/ws/src", "/ws/lib"), None);
        let diff = TabData::new("diff", "a.ts", json!({ "path": "/ws/src/a.ts" }));
        assert_eq!(rewrite_file_tab(&diff, "/ws/src", "/ws/lib"), None);
    }

    // ----- Name validation -----

    fn siblings() -> Vec<DirEntry> {
        vec![dir("/ws/src"), file("/ws/README.md")]
    }

    fn check(input: &str, options: NameOptions) -> Option<String> {
        validate_entry_name(input, Some(&siblings()), options)
    }

    const NESTED: NameOptions = NameOptions { nested: true, current: None };

    #[test]
    fn accepts_ordinary_names() {
        for name in ["notes.txt", ".gitignore", "with space.txt"] {
            assert_eq!(check(name, NameOptions::default()), None);
        }
        assert_eq!(validate_entry_name("anything", None, NameOptions::default()), None);
    }

    #[test]
    fn rejects_empty_names_and_stray_whitespace() {
        assert_eq!(check("", NameOptions::default()).unwrap(), "Enter a name.");
        assert_eq!(check("   ", NameOptions::default()).unwrap(), "Enter a name.");
        assert!(check(" a.txt", NameOptions::default()).unwrap().contains("space"));
        assert!(check("a.txt ", NameOptions::default()).unwrap().contains("space"));
    }

    #[test]
    fn rejects_separators_unless_nested_creation_is_allowed() {
        assert!(check("a/b.txt", NameOptions::default()).unwrap().contains("cannot contain"));
        assert_eq!(check("a/b.txt", NESTED), None);
        assert!(check("a\\b.txt", NESTED).unwrap().contains("Backslashes"));
    }

    #[test]
    fn rejects_traversal_and_empty_segments_even_when_nesting_is_allowed() {
        for name in ["..", ".", "../x", "a/../b", "a/./b"] {
            assert!(check(name, NESTED).unwrap().contains("cannot be used"), "{name}");
        }
        assert!(check("/abs.txt", NESTED).unwrap().contains("relative"));
        assert!(check("a//b", NESTED).unwrap().contains("empty"));
        assert!(check("a/", NESTED).unwrap().contains("empty"));
        // Two dots inside an ordinary name are not traversal.
        assert_eq!(check("a..b", NameOptions::default()), None);
    }

    #[test]
    fn rejects_a_name_that_a_loaded_sibling_already_has() {
        assert!(check("README.md", NameOptions::default()).unwrap().contains("already exists"));
        assert!(check("src", NameOptions::default()).unwrap().contains("already exists"));
        // Linux file systems are case-sensitive; the final decision is made in the backend.
        assert_eq!(check("readme.md", NameOptions::default()), None);
    }

    #[test]
    fn lets_a_nested_path_go_through_an_existing_folder_but_not_through_a_file() {
        assert_eq!(check("src/new.ts", NESTED), None);
        assert!(check("README.md/new.ts", NESTED).unwrap().contains("is a file"));
    }

    #[test]
    fn does_not_treat_the_current_name_as_a_clash_when_renaming() {
        let renaming = NameOptions { nested: false, current: Some("README.md") };
        assert_eq!(check("README.md", renaming), None);
        assert!(check("src", renaming).unwrap().contains("already exists"));
    }

    // ----- Git decorations -----

    fn snapshot(root: &str) -> RepoSnapshot {
        RepoSnapshot {
            root: root.into(),
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

    fn change(path: &str, kind: ChangeKind) -> FileChange {
        FileChange { path: path.into(), orig_path: None, kind, submodule: None }
    }

    fn conflict(path: &str) -> ConflictEntry {
        ConflictEntry { path: path.into(), kind: ConflictKind::BothModified }
    }

    #[test]
    fn maps_each_change_list_to_a_letter_and_status() {
        let mut s = snapshot("/ws");
        s.staged = vec![change("new.ts", ChangeKind::Added), FileChange { orig_path: Some("old.ts".into()), ..change("moved.ts", ChangeKind::Renamed) }];
        s.unstaged = vec![change("edited.ts", ChangeKind::Modified), change("gone.ts", ChangeKind::Deleted)];
        s.untracked = vec!["scratch.txt".into()];
        s.conflicted = vec![conflict("both.ts")];
        let d = build_decorations(Some(&s));
        let of = |path: &str| decoration_for(&d, path, false).map(|decoration| (decoration.letter, decoration.status));
        assert_eq!(of("/ws/new.ts"), Some(("A", FileMark::Added)));
        assert_eq!(of("/ws/moved.ts"), Some(("R", FileMark::Renamed)));
        assert_eq!(of("/ws/edited.ts"), Some(("M", FileMark::Modified)));
        assert_eq!(of("/ws/gone.ts"), Some(("D", FileMark::Deleted)));
        assert_eq!(of("/ws/scratch.txt"), Some(("U", FileMark::Untracked)));
        assert_eq!(of("/ws/both.ts"), Some((FileMark::Conflicted.letter(), FileMark::Conflicted)));
        assert_eq!(of("/ws/clean.ts"), None);
        assert_eq!(of("/ws/old.ts"), None);
    }

    #[test]
    fn resolves_snapshot_paths_against_the_repository_root_not_the_workspace_folder() {
        // The project folder /repo/packages/app is inside the repository /repo.
        let mut s = snapshot("/repo");
        s.unstaged = vec![change("packages/app/src/main.ts", ChangeKind::Modified)];
        s.untracked = vec!["packages/other/x.ts".into()];
        let d = build_decorations(Some(&s));
        assert_eq!(decoration_for(&d, "/repo/packages/app/src/main.ts", false).unwrap().letter, "M");
        let folder = decoration_for(&d, "/repo/packages/app/src", true).unwrap();
        assert_eq!((folder.letter, folder.status), ("", FileMark::Modified));
        assert_eq!(folder.title, "Contains modified files");
        assert_eq!(decoration_for(&d, "/repo/packages/app", true).unwrap().status, FileMark::Modified);
        // The same path relative to the project folder must not match too.
        assert_eq!(decoration_for(&d, "/repo/packages/app/packages/app/src/main.ts", false), None);
        assert_eq!(decoration_for(&d, "/repo/src/main.ts", false), None);
    }

    #[test]
    fn colours_every_ancestor_folder_never_the_repository_root_or_unrelated_folders() {
        let mut s = snapshot("/ws");
        s.untracked = vec!["a/b/c/new.txt".into()];
        let d = build_decorations(Some(&s));
        let mut folders: Vec<&str> = d.folders.keys().map(String::as_str).collect();
        folders.sort();
        assert_eq!(folders, ["/ws/a", "/ws/a/b", "/ws/a/b/c"]);
        let folder = decoration_for(&d, "/ws/a/b", true).unwrap();
        assert_eq!((folder.status, folder.letter), (FileMark::Untracked, ""));
        assert_eq!(decoration_for(&d, "/ws/a/other", true), None);
        assert_eq!(decoration_for(&d, "/ws", true), None);
    }

    #[test]
    fn lets_the_most_significant_change_win_on_a_folder() {
        let mut s = snapshot("/ws");
        s.staged = vec![change("pkg/added/new.ts", ChangeKind::Added)];
        s.unstaged = vec![change("pkg/changed/gone.ts", ChangeKind::Deleted)];
        s.untracked = vec!["pkg/added/scratch.txt".into(), "pkg/fresh/a.txt".into()];
        s.conflicted = vec![conflict("top/x.ts"), conflict("top/sub/y.ts")];
        let d = build_decorations(Some(&s));
        let folder = |path: &str| decoration_for(&d, path, true).map(|decoration| decoration.status);
        assert_eq!(folder("/ws/pkg/fresh"), Some(FileMark::Untracked));
        assert_eq!(folder("/ws/pkg/added"), Some(FileMark::Added));
        assert_eq!(folder("/ws/pkg/changed"), Some(FileMark::Modified));
        assert_eq!(folder("/ws/pkg"), Some(FileMark::Modified));
        assert_eq!(folder("/ws/top"), Some(FileMark::Conflicted));
    }

    #[test]
    fn shows_one_status_for_a_file_that_is_both_staged_and_changed_again() {
        let mut s = snapshot("/ws");
        s.staged = vec![change("new.ts", ChangeKind::Added), change("kept.ts", ChangeKind::Modified)];
        s.unstaged = vec![change("new.ts", ChangeKind::Modified), change("kept.ts", ChangeKind::Deleted)];
        let d = build_decorations(Some(&s));
        assert_eq!(decoration_for(&d, "/ws/new.ts", false).unwrap().letter, "A");
        assert_eq!(decoration_for(&d, "/ws/kept.ts", false).unwrap().letter, "D");
    }

    #[test]
    fn an_untracked_folder_is_decorated_itself() {
        let mut s = snapshot("/ws");
        s.untracked = vec!["fresh/".into()];
        let d = build_decorations(Some(&s));
        assert_eq!(decoration_for(&d, "/ws/fresh", true).unwrap().status, FileMark::Untracked);
    }

    #[test]
    fn has_nothing_to_show_without_a_repository() {
        let d = build_decorations(None);
        assert!(d.files.is_empty() && d.folders.is_empty());
    }

    #[test]
    fn an_entry_can_be_moved_into_any_other_folder_of_the_project() {
        // Into another folder, including the project root, for both files and folders.
        assert!(can_move_into("/ws", "/ws/a.txt", "/ws/src"));
        assert!(can_move_into("/ws", "/ws/src/a.txt", "/ws"));
        assert!(can_move_into("/ws", "/ws/src/deep/a.txt", "/ws/other"));
        assert!(can_move_into("/ws", "/ws/src", "/ws/other/inner"));
        // A shared name prefix does not mean it is inside.
        assert!(can_move_into("/ws", "/ws/src", "/ws/src-old"));
        assert!(can_move_into("/ws/", "/ws/a.txt", "/ws/src/"));
    }

    #[test]
    fn an_entry_cannot_be_dropped_where_nothing_would_change_or_into_itself() {
        // The folder it is already in: nothing changes.
        assert!(!can_move_into("/ws", "/ws/a.txt", "/ws"));
        assert!(!can_move_into("/ws", "/ws/a.txt", "/ws/"));
        assert!(!can_move_into("/ws", "/ws/src/a.txt", "/ws/src"));
        // Into itself or its contents.
        assert!(!can_move_into("/ws", "/ws/src", "/ws/src"));
        assert!(!can_move_into("/ws", "/ws/src", "/ws/src/deep"));
        // The project folder itself, and anything outside it, cannot be moved.
        assert!(!can_move_into("/ws", "/ws", "/ws/src"));
        assert!(!can_move_into("/ws", "/elsewhere/a.txt", "/ws/src"));
    }

    #[test]
    fn moves_are_judged_the_same_with_windows_separators() {
        assert!(can_move_into("C:\\ws", "C:\\ws\\src\\a.txt", "C:\\ws"));
        assert!(!can_move_into("C:\\ws", "C:\\ws\\src\\a.txt", "C:\\ws\\src"));
        assert!(!can_move_into("C:\\ws", "C:\\ws\\src", "C:\\ws\\src\\deep"));
        assert!(!can_move_into("C:\\ws", "C:\\ws\\a.txt", "C:\\ws\\"));
    }

    #[test]
    fn matches_windows_paths_regardless_of_separator() {
        let mut s = snapshot("C:/ws");
        s.unstaged = vec![change("src/main.ts", ChangeKind::Modified)];
        let d = build_decorations(Some(&s));
        assert_eq!(decoration_for(&d, "C:\\ws\\src\\main.ts", false).unwrap().letter, "M");
        assert_eq!(decoration_for(&d, "C:\\ws\\src", true).unwrap().status, FileMark::Modified);
    }
}
