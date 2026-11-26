//! The Explorer sidebar view: a file tree loaded per folder and virtualized, together with its
//! file operations (create, rename, move to trash, copy path).
//!
//! The tree state lives in [`TreeState`] (pure logic). This view only runs the loading:
//! each time the tree changes, open folders that have no contents yet are requested from the backend, and
//! a "show this path" request is continued until all its ancestors are loaded.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, rgb, uniform_list, AnyElement, App, AppContext as _, Bounds, ClickEvent, ClipboardItem, Context, DragMoveEvent, Entity, ExternalPaths, FocusHandle, Focusable, Global,
    Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, ParentElement as _, Pixels, Point, Render, ScrollStrategy, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, TestSupportExt as _, UniformListScrollHandle, WeakEntity, Window,
};
use oncode_core::api::fs;
use oncode_core::AppError;
use oncode_ui::editor::{buffers, file_path, is_untitled, open_file, OpenFile};
use oncode_ui::file_icons::{file_icon, folder_icon};
use oncode_ui::overlay::{self, Confirm};
use oncode_ui::{repos, spawn_core, DraggedEntry, Env, FileMark, Lucide, TabDragPreview, WorkbenchEvent};
use oncode_workspace::DirEntry;
use serde_json::Value;

use crate::restricted;
use crate::tree::{
    ancestor_chain, base_name, build_decorations, can_move_into, decoration_for, dir_name, is_same_or_under, join_path, moved_path, relative_path, rewrite_file_tab, same_path,
    validate_entry_name, EntryDecoration, GitDecorations, NameOptions, Row, TreeState, INDENT, ROW_HEIGHT,
};

/// The delay for collecting consecutive file change events into a single reload.
const FS_DEBOUNCE: Duration = Duration::from_millis(150);

/// How long the pointer must rest over a closed folder, while dragging, before that folder is opened.
const HOVER_EXPAND: Duration = Duration::from_millis(600);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Folder,
}

#[derive(Debug, Clone, PartialEq)]
enum EditKind {
    Create(EntryKind),
    Rename(DirEntry),
}

/// The name input inside the tree, for a new entry or a rename.
struct Edit {
    kind: EditKind,
    /// The folder where the entry is created, or the parent of the entry being renamed.
    parent: String,
    input: Entity<InputState>,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

/// A drawn row: a tree row, the name input, or its validation message.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Display {
    Tree(usize),
    Edit { depth: usize },
    EditError { depth: usize },
}

/// The open folders and the selected row per window and project folder. `Workbench` (and
/// this view) is recreated when the trust status changes; the tree must not collapse along with it.
#[derive(Default)]
struct TreeMemory(HashMap<(String, String), (HashSet<String>, Option<String>)>);

impl Global for TreeMemory {}

/// The label of the menu item and command that reveal a file in the OS file manager, per platform.
pub const REVEAL_LABEL: &str = if cfg!(target_os = "macos") {
    "Reveal in Finder"
} else if cfg!(windows) {
    "Reveal in File Explorer"
} else {
    "Open Containing Folder"
};

/// The command palette label for the file in the active tab.
pub const REVEAL_ACTIVE_LABEL: &str = if cfg!(target_os = "macos") {
    "Reveal Active File in Finder"
} else if cfg!(windows) {
    "Reveal Active File in File Explorer"
} else {
    "Open Active File's Containing Folder"
};

/// The opener of the OS file manager. The default is `App::reveal_path`; tests install a replacement through this global
/// because the GPUI test platform does not implement `reveal_path` (calling it directly panics).
#[derive(Clone)]
pub struct RevealHandler(pub RevealFn);

/// A function that opens `path` in the OS file manager.
pub type RevealFn = Rc<dyn Fn(&Path, &mut App)>;

impl Global for RevealHandler {}

pub struct Explorer {
    env: Env,
    /// The project folder; `None` if the window has not opened a folder.
    root: Option<String>,
    tree: TreeState,
    rows: Vec<Row>,
    display: Vec<Display>,
    decorations: GitDecorations,
    /// The file in the active tab; its row is highlighted.
    active_path: Option<String>,
    /// The path requested to be shown; continued each time the contents of a folder arrive.
    pending_reveal: Option<String>,
    edit: Option<Edit>,
    /// The context menu target: the entry under the pointer, or `None` for the tree background.
    menu_target: Option<String>,
    /// The folder that would receive what is being dragged if dropped now; highlighted while dragging.
    drop_target: Option<String>,
    /// A closed folder under the pointer while dragging; opened if the pointer rests over it.
    hover_folder: Option<String>,
    _hover_expand: Task<()>,
    focus_handle: FocusHandle,
    scroll: UniformListScrollHandle,
    fs_paths: HashSet<String>,
    fs_overflow: bool,
    _fs_debounce: Task<()>,
    _subscriptions: Vec<Subscription>,
}

/// The place in the tree that a point indicates while dragging.
struct DropPoint {
    /// The receiving folder: the folder under the pointer, the parent folder of a file, or the project root
    /// if the pointer is on the tree background.
    dir: String,
    /// The closed folder exactly under the pointer.
    collapsed: Option<String>,
}

impl Focusable for Explorer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn mark_color(mark: FileMark, dark: bool) -> Hsla {
    let hex = match (mark, dark) {
        (FileMark::Added, true) => 0x7cc98a,
        (FileMark::Modified, true) => 0xe3b556,
        (FileMark::Deleted, true) => 0xf0616d,
        (FileMark::Renamed, true) => 0x6cb6ff,
        (FileMark::Untracked, true) => 0x5fc9a8,
        (FileMark::Conflicted, true) => 0xff8f5a,
        (FileMark::Added, false) => 0x2c8a45,
        (FileMark::Modified, false) => 0xa8710a,
        (FileMark::Deleted, false) => 0xc9303f,
        (FileMark::Renamed, false) => 0x1f6fe0,
        (FileMark::Untracked, false) => 0x1f8f72,
        (FileMark::Conflicted, false) => 0xc75a1c,
    };
    rgb(hex).into()
}

fn with_context(mut error: AppError, context: String) -> AppError {
    error.message = format!("{context}: {}", error.message);
    error
}

impl Explorer {
    pub fn new(env: Env, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let root = env.root_string();
        let mut tree = TreeState::new(root.clone().unwrap_or_default());
        if let Some(root) = &root {
            let key = (env.window.to_string(), root.clone());
            if let Some((expanded, selected)) = cx.default_global::<TreeMemory>().0.get(&key) {
                tree.expanded = expanded.clone();
                tree.selected = selected.clone();
            }
        }

        let mut subscriptions = vec![cx.observe(&repos(cx), |this: &mut Self, _, cx| this.sync_decorations(cx))];
        if let Some(workbench) = env.workbench.upgrade() {
            subscriptions.push(cx.subscribe_in(&workbench, window, |this: &mut Self, workbench, event: &WorkbenchEvent, _, cx| match event {
                WorkbenchEvent::ActiveTabChanged | WorkbenchEvent::TabsChanged => {
                    let active = workbench.read(cx).model().active().filter(|(_, tab)| tab.kind == "file").and_then(|(_, tab)| tab.param_str("path")).map(String::from);
                    this.set_active_path(active, cx);
                }
                WorkbenchEvent::FsChanged { paths, overflow } => this.queue_fs_change(paths, *overflow, cx),
                _ => {}
            }));
        }

        let mut this = Self {
            env,
            root,
            tree,
            rows: Vec::new(),
            display: Vec::new(),
            decorations: GitDecorations::default(),
            active_path: None,
            pending_reveal: None,
            edit: None,
            menu_target: None,
            drop_target: None,
            hover_folder: None,
            _hover_expand: Task::ready(()),
            focus_handle: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            fs_paths: HashSet::new(),
            fs_overflow: false,
            _fs_debounce: Task::ready(()),
            _subscriptions: subscriptions,
        };
        this.sync_decorations(cx);
        if this.root.is_some() {
            this.tree_changed(cx);
        }
        this
    }

    // ----- State for other features and tests -----

    pub fn root(&self) -> Option<&str> {
        self.root.as_deref()
    }

    /// The visible tree rows, from top to bottom.
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn selected(&self) -> Option<&str> {
        self.tree.selected.as_deref()
    }

    pub fn is_expanded(&self, path: &str) -> bool {
        self.tree.expanded.contains(path)
    }

    /// A folder is being loaded.
    pub fn is_loading(&self) -> bool {
        self.tree.is_loading()
    }

    /// The name input (new entry or rename) is open.
    pub fn is_editing(&self) -> bool {
        self.edit.is_some()
    }

    /// The validation message of the name input, if any.
    pub fn edit_error(&self) -> Option<SharedString> {
        self.edit.as_ref()?.error.clone()
    }

    /// The Git decoration for an absolute path.
    pub fn decoration(&self, path: &str, is_dir: bool) -> Option<EntryDecoration> {
        decoration_for(&self.decorations, path, is_dir)
    }

    /// Keyboard focus is in the tree (not in the name input).
    pub fn tree_focused(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
    }

    /// The currently selected entry, looked up in the contents of its parent folder.
    pub fn selected_entry(&self) -> Option<&DirEntry> {
        self.tree.entry(self.tree.selected.as_deref()?)
    }

    // ----- Loading -----

    fn start_load(&mut self, dir: &str, cx: &mut Context<Self>) {
        let request = self.tree.begin_load(dir);
        let (core, label, path) = (self.env.core.clone(), self.env.window.to_string(), dir.to_string());
        let task = spawn_core(cx, async move { fs::fs_read_dir(&core, &label, path).await });
        let dir = dir.to_string();
        cx.spawn(async move |this, cx| {
            let result = task.await.map_err(|error| error.message);
            let _ = this.update(cx, |this, cx| {
                this.tree.finish_load(&dir, request, result);
                this.tree_changed(cx);
            });
        })
        .detach();
    }

    /// Called after every tree change: continues the show request, requests the contents of
    /// open folders that are not loaded yet, then rebuilds the rows.
    fn tree_changed(&mut self, cx: &mut Context<Self>) {
        if self.root.is_none() {
            return;
        }
        let revealed = self.advance_reveal(cx);
        let flat = self.tree.flatten();
        for dir in &flat.missing {
            self.start_load(dir, cx);
        }
        self.rows = flat.rows;
        self.rebuild_display();
        if let Some(path) = revealed {
            self.scroll_to_path(&path);
        }
        self.remember(cx);
        cx.notify();
    }

    fn remember(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else { return };
        let state = (self.tree.expanded.clone(), self.tree.selected.clone());
        cx.default_global::<TreeMemory>().0.insert((self.env.window.to_string(), root), state);
    }

    /// Builds the drawn rows: the tree rows plus the name input in its place.
    fn rebuild_display(&mut self) {
        let mut display: Vec<Display> = (0..self.rows.len()).map(Display::Tree).collect();
        if let Some(edit) = &self.edit {
            let has_error = edit.error.is_some();
            match &edit.kind {
                EditKind::Rename(entry) => {
                    if let Some(index) = self.rows.iter().position(|row| row.path() == Some(entry.path.as_str())) {
                        let depth = self.rows[index].depth();
                        display[index] = Display::Edit { depth };
                        if has_error {
                            display.insert(index + 1, Display::EditError { depth });
                        }
                    }
                }
                EditKind::Create(_) => {
                    // Right below the target folder; at the very top for the project root.
                    let (at, depth) = match self.rows.iter().position(|row| row.path() == Some(edit.parent.as_str())) {
                        Some(index) => (index + 1, self.rows[index].depth() + 1),
                        None => (0, 0),
                    };
                    // The "Empty" note does not need to appear next to the name input.
                    if matches!(self.rows.get(at), Some(Row::Note { parent, error: false, .. }) if *parent == edit.parent) {
                        display.remove(at);
                    }
                    display.insert(at, Display::Edit { depth });
                    if has_error {
                        display.insert(at + 1, Display::EditError { depth });
                    }
                }
            }
        }
        self.display = display;
    }

    fn scroll_to_path(&mut self, path: &str) {
        let index = self.display.iter().position(|display| matches!(display, Display::Tree(row) if self.rows[*row].path() == Some(path)));
        if let Some(index) = index {
            self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
        }
    }

    fn scroll_to_edit(&mut self) {
        if let Some(index) = self.display.iter().position(|display| matches!(display, Display::Edit { .. })) {
            self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
        }
    }

    /// Continues the show request. Returns its path if it is now selected and visible.
    fn advance_reveal(&mut self, cx: &mut Context<Self>) -> Option<String> {
        let path = self.pending_reveal.clone()?;
        let chain = ancestor_chain(&self.tree.root, &path);
        if chain.is_empty() {
            // Outside the project.
            self.pending_reveal = None;
            return None;
        }
        for (index, dir) in chain.iter().enumerate() {
            let next = chain.get(index + 1).map_or(path.as_str(), String::as_str);
            match self.tree.dirs.get(dir) {
                None => {
                    self.start_load(dir, cx);
                    return None;
                }
                // Contents that are being reloaded cannot be trusted yet; wait for the response.
                Some(state) if state.loading => return None,
                Some(state) => {
                    if !state.entries.as_ref().is_some_and(|entries| entries.iter().any(|entry| entry.path == next)) {
                        // Already deleted, unreadable, or hidden (e.g. inside `.git`).
                        self.pending_reveal = None;
                        return None;
                    }
                }
            }
        }
        for dir in chain.iter().skip(1) {
            self.tree.expand(dir);
        }
        self.tree.selected = Some(path.clone());
        self.pending_reveal = None;
        Some(path)
    }

    /// Opens all ancestor folders of `path`, selects it, and scrolls to it. Does
    /// nothing if `path` is not in the tree (outside the project or already deleted).
    pub fn reveal(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.root.is_none() {
            return;
        }
        self.pending_reveal = Some(path.to_string());
        self.tree_changed(cx);
    }

    fn set_active_path(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        if self.active_path == path {
            return;
        }
        self.active_path = path.clone();
        match path {
            Some(path) => self.reveal(&path, cx),
            None => cx.notify(),
        }
    }

    fn sync_decorations(&mut self, cx: &mut Context<Self>) {
        let store = repos(cx);
        let store = store.read(cx);
        let snapshot = self.env.repo_root.as_deref().and_then(|repo_root| {
            store.snapshot(repo_root).or_else(|| {
                // The root reported by the backend may be written slightly differently (e.g. a trailing slash).
                let root = self.root.as_deref()?;
                store.roots().into_iter().filter(|candidate| is_same_or_under(root, candidate)).max_by_key(String::len).and_then(|candidate| store.snapshot(&candidate))
            })
        });
        let decorations = build_decorations(snapshot);
        if decorations != self.decorations {
            self.decorations = decorations;
            cx.notify();
        }
    }

    // ----- Outside changes -----

    fn queue_fs_change(&mut self, paths: &[String], overflow: bool, cx: &mut Context<Self>) {
        if self.root.is_none() {
            return;
        }
        self.fs_paths.extend(paths.iter().cloned());
        self.fs_overflow |= overflow;
        self._fs_debounce = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FS_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| {
                let paths = std::mem::take(&mut this.fs_paths);
                let overflow = std::mem::take(&mut this.fs_overflow);
                let targets = this.tree.fs_changes(paths.iter().map(String::as_str), overflow);
                for dir in targets {
                    this.start_load(&dir, cx);
                }
                this.tree_changed(cx);
            });
        });
    }

    // ----- Tree -----

    pub fn select(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        if self.tree.selected == path {
            return;
        }
        self.tree.selected = path.clone();
        if let Some(path) = path {
            self.scroll_to_path(&path);
        }
        self.remember(cx);
        cx.notify();
    }

    pub fn expand(&mut self, path: &str, cx: &mut Context<Self>) {
        self.tree.expand(path);
        self.tree_changed(cx);
    }

    pub fn collapse(&mut self, path: &str, cx: &mut Context<Self>) {
        self.tree.collapse(path);
        self.tree_changed(cx);
    }

    pub fn toggle(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.tree.expanded.contains(path) {
            self.collapse(path, cx);
        } else {
            self.expand(path, cx);
        }
    }

    pub fn collapse_all(&mut self, cx: &mut Context<Self>) {
        self.tree.collapse_all();
        self.tree_changed(cx);
    }

    /// Reloads the root and every open folder; the cache of other folders is dropped. The expansion
    /// and selection are kept.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else { return };
        self.reload_under(&root, cx);
    }

    fn reload_under(&mut self, path: &str, cx: &mut Context<Self>) {
        for dir in self.tree.reload_under(path) {
            self.start_load(&dir, cx);
        }
        self.tree_changed(cx);
    }

    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    fn open(&mut self, path: &str, preview: bool, to_side: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workbench) = self.env.workbench.upgrade() else { return };
        let mut options = OpenFile::new(path);
        options.preview = preview;
        options.to_side = to_side;
        workbench.update(cx, |workbench, cx| open_file(workbench, options, window, cx));
    }

    fn on_row_click(&mut self, path: &str, is_dir: bool, clicks: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.select(Some(path.to_string()), cx);
        if is_dir {
            self.focus_handle.focus(window, cx);
            // The second click of a double click does not close the folder that was just opened.
            if clicks <= 1 {
                self.toggle(path, cx);
            }
        } else {
            // A single click opens a preview; a double click makes it a permanent tab.
            self.open(path, clicks <= 1, false, window, cx);
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.platform || modifiers.alt || self.edit.is_some() {
            return;
        }
        let Some(root) = self.root.clone() else { return };
        let entries: Vec<(&DirEntry, bool, &str)> = self
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Entry { entry, expanded, parent, .. } => Some((entry, *expanded, parent.as_str())),
                Row::Note { .. } => None,
            })
            .collect();
        let index = self.tree.selected.as_deref().and_then(|selected| entries.iter().position(|(entry, _, _)| entry.path == selected));
        let current = index.map(|index| (entries[index].0.clone(), entries[index].1, entries[index].2.to_string()));
        let at = |to: usize| entries.get(to.min(entries.len().saturating_sub(1))).map(|(entry, _, _)| entry.path.clone());

        enum Step {
            Select(Option<String>),
            Expand(String),
            Collapse(String),
            Toggle(String),
            Open(String),
            Nothing,
        }
        let step = match event.keystroke.key.as_str() {
            "down" => Step::Select(at(index.map_or(0, |index| index + 1))),
            "up" => Step::Select(at(index.map_or(0, |index| index.saturating_sub(1)))),
            "home" => Step::Select(at(0)),
            "end" => Step::Select(at(usize::MAX)),
            "right" => match &current {
                Some((entry, false, _)) if entry.is_dir => Step::Expand(entry.path.clone()),
                Some((entry, true, _)) if entry.is_dir && index.and_then(|index| entries.get(index + 1)).is_some_and(|(_, _, parent)| *parent == entry.path) => {
                    Step::Select(at(index.unwrap_or(0) + 1))
                }
                _ => Step::Nothing,
            },
            "left" => match &current {
                Some((entry, true, _)) if entry.is_dir => Step::Collapse(entry.path.clone()),
                Some((_, _, parent)) if *parent != root => Step::Select(Some(parent.clone())),
                _ => Step::Nothing,
            },
            "enter" => match &current {
                Some((entry, _, _)) if entry.is_dir => Step::Toggle(entry.path.clone()),
                Some((entry, _, _)) => Step::Open(entry.path.clone()),
                None => Step::Nothing,
            },
            _ => return,
        };
        cx.stop_propagation();
        match step {
            Step::Select(Some(path)) => self.select(Some(path), cx),
            Step::Expand(path) => self.expand(&path, cx),
            Step::Collapse(path) => self.collapse(&path, cx),
            Step::Toggle(path) => self.toggle(&path, cx),
            Step::Open(path) => self.open(&path, true, false, window, cx),
            Step::Select(None) | Step::Nothing => {}
        }
    }

    // ----- New entries and rename -----

    /// The target folder for a new file/folder: the selected folder, the folder of the selected file, or the project root.
    fn target_directory(&self) -> Option<String> {
        let root = self.root.clone()?;
        Some(match self.selected_entry() {
            Some(entry) if entry.is_dir => entry.path.clone(),
            Some(entry) => dir_name(&entry.path).to_string(),
            None => root,
        })
    }

    fn start_edit(&mut self, kind: EditKind, parent: String, window: &mut Window, cx: &mut Context<Self>) {
        let (initial, stem_end) = match &kind {
            EditKind::Rename(entry) => {
                // For a file, only the name without the extension is selected; `.gitignore` and folders are selected entirely.
                let stem_end = match entry.name.rfind('.') {
                    Some(dot) if !entry.is_dir && dot > 0 => dot,
                    _ => entry.name.len(),
                };
                (entry.name.clone(), stem_end)
            }
            EditKind::Create(_) => (String::new(), 0),
        };
        let placeholder = match &kind {
            EditKind::Create(EntryKind::File) => "name.ext or folder/name.ext",
            EditKind::Create(EntryKind::Folder) => "name or parent/name",
            EditKind::Rename(_) => "",
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(initial));
        let subscription = cx.subscribe_in(&input, window, |this: &mut Self, input, event: &InputEvent, window, cx| {
            // Events from an input that has been replaced are ignored.
            if this.edit.as_ref().is_none_or(|edit| edit.input != *input) {
                return;
            }
            match event {
                InputEvent::Change => this.validate_edit(cx),
                InputEvent::PressEnter { .. } => this.commit_edit(window, cx),
                InputEvent::Blur => this.cancel_edit(false, window, cx),
                InputEvent::Focus => {}
            }
        });
        self.edit = Some(Edit { kind, parent, input: input.clone(), error: None, _subscriptions: vec![subscription] });
        self.tree_changed(cx);
        self.scroll_to_edit();
        // Focus is moved after this update finishes, once the input is part of the view.
        window.defer(cx, move |window, cx| {
            input.update(cx, |state, cx| {
                state.focus(window, cx);
                if stem_end > 0 {
                    state.set_selected_range(0..stem_end, cx);
                }
            });
        });
    }

    /// Opens the name input for a new file or folder in `dir` (default: follows the selection).
    pub fn new_entry(&mut self, kind: EntryKind, dir: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else { return };
        let Some(parent) = dir.or_else(|| self.target_directory()) else { return };
        // The target folder is opened first so it is visible where the new entry will appear.
        for dir in ancestor_chain(&root, &parent).into_iter().skip(1) {
            self.tree.expand(&dir);
        }
        if parent != root {
            self.tree.expand(&parent);
        }
        self.start_edit(EditKind::Create(kind), parent, window, cx);
    }

    /// Opens the name input to rename the selected entry.
    pub fn rename_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.selected_entry().cloned() else { return };
        self.rename_entry(entry, window, cx);
    }

    fn rename_entry(&mut self, entry: DirEntry, window: &mut Window, cx: &mut Context<Self>) {
        let parent = dir_name(&entry.path).to_string();
        self.start_edit(EditKind::Rename(entry), parent, window, cx);
    }

    fn edit_problem(&self, edit: &Edit, cx: &App) -> Option<String> {
        let value = edit.input.read(cx).value();
        let siblings = self.tree.entries(&edit.parent);
        match &edit.kind {
            EditKind::Create(_) => validate_entry_name(&value, siblings, NameOptions { nested: true, current: None }),
            EditKind::Rename(entry) => validate_entry_name(&value, siblings, NameOptions { nested: false, current: Some(&entry.name) }),
        }
    }

    fn validate_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = &self.edit else { return };
        let error = self.edit_problem(edit, cx).map(SharedString::from);
        if let Some(edit) = &mut self.edit {
            if edit.error != error {
                edit.error = error;
                self.rebuild_display();
                self.scroll_to_edit();
                cx.notify();
            }
        }
    }

    fn cancel_edit(&mut self, refocus: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit.take().is_none() {
            return;
        }
        self.rebuild_display();
        if refocus {
            self.focus_handle.focus(window, cx);
        }
        cx.notify();
    }

    fn commit_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.validate_edit(cx);
        let Some(edit) = &self.edit else { return };
        if edit.error.is_some() {
            return;
        }
        let name = edit.input.read(cx).value().to_string();
        let Some(edit) = self.edit.take() else { return };
        self.rebuild_display();
        self.focus_handle.focus(window, cx);
        cx.notify();
        match edit.kind {
            EditKind::Create(kind) => self.create(kind, edit.parent, name, window, cx),
            EditKind::Rename(entry) if entry.name == name => {}
            EditKind::Rename(entry) => self.rename(entry, edit.parent, name, window, cx),
        }
    }

    fn create(&mut self, kind: EntryKind, parent: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let path = join_path(&parent, &name);
        let (core, label, target) = (self.env.core.clone(), self.env.window.to_string(), path.clone());
        let task = spawn_core(cx, async move {
            match kind {
                EntryKind::File => fs::fs_create_file(&core, &label, target).await,
                EntryKind::Folder => fs::fs_create_dir(&core, &label, target).await,
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Err(error) = result {
                    overlay::show_error(&with_context(error, format!("Could not create {name}")), window, cx);
                    return;
                }
                this.start_load(&parent, cx);
                // The intermediate folder (`a/b/c.txt`) may have been loaded before; its cache is now stale.
                if let Some((first, _)) = name.split_once('/') {
                    let between = join_path(&parent, first);
                    for dir in this.tree.reload_under(&between) {
                        this.start_load(&dir, cx);
                    }
                }
                this.reveal(&path, cx);
                if kind == EntryKind::File {
                    this.open(&path, false, false, window, cx);
                }
            });
        })
        .detach();
    }

    fn rename(&mut self, entry: DirEntry, parent: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let to = join_path(&parent, &name);
        let (core, label, from, target) = (self.env.core.clone(), self.env.window.to_string(), entry.path.clone(), to.clone());
        let task = spawn_core(cx, async move { fs::fs_rename(&core, &label, from, target).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Err(error) = result {
                    overlay::show_error(&with_context(error, format!("Could not rename {}", entry.name)), window, cx);
                    return;
                }
                this.apply_rename(&entry.path, &to, window, cx);
                this.tree.move_state(&entry.path, &to);
                this.tree.selected = Some(to.clone());
                // The parent is reloaded; the folder under the new name follows once that name is in its parent's contents.
                this.start_load(&parent, cx);
                this.tree_changed(cx);
            });
        })
        .detach();
    }

    /// Moves buffers and file tabs from `from` (a file, or a folder with its contents) to `to`.
    fn apply_rename(&mut self, from: &str, to: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workbench) = self.env.workbench.upgrade() else { return };
        // Buffers are moved first: unsaved contents follow to the new path before tabs are reattached.
        // The buffers and their editor state are held until the new view attaches itself.
        let mut held = Vec::new();
        if let Some(registry) = buffers(workbench.read(cx)) {
            for buffer in registry.read(cx).all() {
                let old = buffer.read(cx).path().to_string();
                let Some(new) = moved_path(&old, from, to) else { continue };
                registry.update(cx, |registry, cx| registry.rename(&old, &new, cx));
                if !buffer.read(cx).is_ready() {
                    // Loading from the old path would no longer succeed.
                    buffer.update(cx, |buffer, cx| buffer.reload(window, cx));
                }
                let state = buffer.read(cx).primary();
                held.push((buffer, state));
            }
        }
        workbench.update(cx, |workbench, cx| workbench.update_tabs(|tab| rewrite_file_tab(tab, from, to), window, cx));
        drop(held);
    }

    // ----- Delete and copy path -----

    /// Moves the selected entry to the OS trash after confirmation.
    pub fn delete_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.selected_entry().cloned() else { return };
        self.delete_entry(entry, window, cx);
    }

    fn delete_entry(&mut self, entry: DirEntry, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workbench) = self.env.workbench.upgrade() else { return };
        // Open files inside this entry that have unsaved changes are lost too.
        let dirty: Vec<String> = {
            let workbench = workbench.read(cx);
            let open: Vec<&str> = workbench.model().all_tabs().filter(|tab| tab.kind == "file").filter_map(|tab| tab.param_str("path")).collect();
            buffers(workbench)
                .map(|registry| registry.read(cx).dirty(cx))
                .unwrap_or_default()
                .iter()
                .map(|buffer| buffer.read(cx).path().to_string())
                .filter(|path| is_same_or_under(path, &entry.path) && open.contains(&path.as_str()))
                .collect()
        };
        let unsaved = match dirty.as_slice() {
            [] => String::new(),
            [only] => format!(" \"{}\" has unsaved changes that will be lost.", base_name(only)),
            many => format!(" {} open files inside it have unsaved changes that will be lost.", many.len()),
        };
        let title = format!("Delete {} \"{}\"?", if entry.is_dir { "folder" } else { "file" }, entry.name);
        let subject = if entry.is_dir { format!("\"{}\" and everything inside it", entry.name) } else { format!("\"{}\"", entry.name) };
        let confirmed = overlay::confirm(Confirm::new(title, format!("{subject} will be moved to the system trash.{unsaved}"), "Move to Trash").danger(), window, cx);

        let (core, label) = (self.env.core.clone(), self.env.window.to_string());
        cx.spawn_in(window, async move |this, cx| {
            if !confirmed.await {
                return;
            }
            let target = entry.path.clone();
            let Ok(task) = cx.update(|_, cx| spawn_core(cx, async move { fs::fs_trash(&core, &label, vec![target]).await })) else { return };
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Err(error) = result {
                    overlay::show_error(&with_context(error, format!("Could not delete {}", entry.name)), window, cx);
                    return;
                }
                let inside = entry.path.clone();
                workbench.update(cx, |workbench, cx| {
                    workbench.force_close_where(|tab| tab.kind == "file" && tab.param_str("path").is_some_and(|path| is_same_or_under(path, &inside)), window, cx);
                });
                // Reloading the parent also drops the tree state below the entry that disappeared.
                this.start_load(dir_name(&entry.path), cx);
                this.tree_changed(cx);
            });
        })
        .detach();
    }

    /// Copies the path of the selected entry to the clipboard.
    pub fn copy_selected_path(&mut self, relative: bool, cx: &mut Context<Self>) {
        if let Some(path) = self.tree.selected.clone() {
            self.copy_path(&path, relative, cx);
        }
    }

    fn copy_path(&self, path: &str, relative: bool, cx: &mut Context<Self>) {
        let text = match (&self.root, relative) {
            (Some(root), true) => relative_path(root, path),
            _ => path.to_string(),
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    /// Opens `path` in the OS file manager: the file selected in its parent folder (Windows/macOS), or
    /// its parent folder opened (Linux). It only asks the OS to show that path and does not run any contents of the
    /// project folder, so it is also safe in restricted mode. An `untitled:` file is not on disk yet: ignored.
    pub fn reveal_in_file_manager(&self, path: &str, cx: &mut Context<Self>) {
        if is_untitled(path) {
            return;
        }
        let path = PathBuf::from(file_path(path));
        match cx.try_global::<RevealHandler>().cloned() {
            Some(RevealHandler(handler)) => handler(&path, cx),
            None => cx.reveal_path(&path),
        }
    }

    /// The file in the active tab that is already on disk; `None` for non-file tabs and `untitled:` files.
    pub fn active_file(&self) -> Option<&str> {
        self.active_path.as_deref().filter(|path| !is_untitled(path))
    }

    pub fn reveal_active_in_file_manager(&self, cx: &mut Context<Self>) {
        if let Some(path) = self.active_file().map(String::from) {
            self.reveal_in_file_manager(&path, cx);
        }
    }

    // ----- Drag and drop -----

    /// Whether the row list is drawn: the root has contents, or the name input is open. Otherwise only
    /// a single message is shown (loading, empty, or error).
    fn list_shown(&self) -> bool {
        let has_entries = self.root.as_deref().and_then(|root| self.tree.dirs.get(root)).is_some_and(|state| state.entries.as_ref().is_some_and(|entries| !entries.is_empty()));
        self.edit.is_some() || has_entries
    }

    /// The folder currently highlighted as the drop target, while something is being dragged over the tree.
    pub fn drop_target(&self) -> Option<&str> {
        self.drop_target.as_deref()
    }

    /// The place in the tree indicated by `position` (window coordinates). Rows are mapped through the position and
    /// the list scroll, because rows are virtualized and all are `ROW_HEIGHT` tall. `None` if outside the
    /// list or over the name input.
    fn drop_point(&self, position: Point<Pixels>) -> Option<DropPoint> {
        let root = self.root.clone()?;
        // An empty (or still loading) project: the list is not drawn, only a message. The whole tree is the root.
        if !self.list_shown() {
            return Some(DropPoint { dir: root, collapsed: None });
        }
        let (bounds, offset) = {
            let state = self.scroll.0.borrow();
            (state.base_handle.bounds(), state.base_handle.offset())
        };
        if !bounds.contains(&position) {
            return None;
        }
        let index = f32::from(position.y - bounds.origin.y - offset.y) / ROW_HEIGHT;
        let display = (index >= 0.).then(|| index as usize).and_then(|index| self.display.get(index).copied());
        let row = match display {
            // Below the last row: the tree background, meaning the project root.
            None => return Some(DropPoint { dir: root, collapsed: None }),
            Some(Display::Tree(row)) => &self.rows[row],
            Some(Display::Edit { .. } | Display::EditError { .. }) => return None,
        };
        Some(match row {
            Row::Entry { entry, expanded, .. } if entry.is_dir => DropPoint { dir: entry.path.clone(), collapsed: (!*expanded).then(|| entry.path.clone()) },
            // Dropped on a file: goes into the folder that file is in.
            Row::Entry { entry, .. } => DropPoint { dir: dir_name(&entry.path).to_string(), collapsed: None },
            Row::Note { parent, .. } => DropPoint { dir: parent.clone(), collapsed: None },
        })
    }

    /// Highlights the target folder while something is dragged over the tree, and opens a closed folder if
    /// the pointer rests over it. `dragged` is empty for files from the OS, which may always be copied.
    fn track_drop_target(&mut self, dragged: Option<&DraggedEntry>, bounds: Bounds<Pixels>, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else { return };
        let point = if bounds.contains(&position) { self.drop_point(position) } else { None };
        let target = point.as_ref().map(|point| point.dir.clone()).filter(|dir| dragged.is_none_or(|dragged| can_move_into(&root, &dragged.path, dir)));
        let folder = point.and_then(|point| point.collapsed);
        if target != self.drop_target {
            self.drop_target = target;
            cx.notify();
        }
        if folder != self.hover_folder {
            self.hover_folder = folder.clone();
            // Replacing the previous task cancels it.
            self._hover_expand = match folder {
                Some(dir) => cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(HOVER_EXPAND).await;
                    let _ = this.update(cx, |this, cx| {
                        if this.hover_folder.as_deref() == Some(dir.as_str()) && cx.has_active_drag() {
                            this.hover_folder = None;
                            this.expand(&dir, cx);
                        }
                    });
                }),
                None => Task::ready(()),
            };
        }
    }

    /// Clears the highlight and stops the automatic opening, after a drop.
    fn end_drop(&mut self, cx: &mut Context<Self>) {
        self.drop_target = None;
        self.hover_folder = None;
        self._hover_expand = Task::ready(());
        cx.notify();
    }

    /// The target folder that should be highlighted now: something is being dragged and the pointer is over the tree.
    fn active_drop_target(&self, cx: &App) -> Option<&str> {
        self.drop_target.as_deref().filter(|_| cx.has_active_drag())
    }

    /// An entry from this tree is dropped: it is moved into the folder under the pointer.
    fn drop_entry(&mut self, dragged: &DraggedEntry, window: &mut Window, cx: &mut Context<Self>) {
        let position = window.mouse_position();
        self.end_drop(cx);
        let Some(root) = self.root.clone() else { return };
        let Some(point) = self.drop_point(position) else { return };
        if can_move_into(&root, &dragged.path, &point.dir) {
            self.move_entry(dragged.path.clone(), point.dir, window, cx);
        }
    }

    /// Files from the OS are dropped: they are copied into the folder under the pointer.
    fn drop_external(&mut self, dropped: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        let position = window.mouse_position();
        self.end_drop(cx);
        let Some(point) = self.drop_point(position) else { return };
        let sources = dropped.paths().iter().map(|path| path.to_string_lossy().into_owned()).collect();
        self.copy_in(sources, point.dir, window, cx);
    }

    /// Moves `from` (a file or folder) into the folder `to_dir`. Open buffers and tabs move
    /// along, just like a rename.
    fn move_entry(&mut self, from: String, to_dir: String, window: &mut Window, cx: &mut Context<Self>) {
        let name = base_name(&from).to_string();
        let to = join_path(&to_dir, &name);
        let from_dir = dir_name(&from).to_string();
        let (core, label, source, target) = (self.env.core.clone(), self.env.window.to_string(), from.clone(), to.clone());
        let task = spawn_core(cx, async move { fs::fs_rename(&core, &label, source, target).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Err(error) = result {
                    overlay::show_error(&with_context(error, format!("Could not move {name}")), window, cx);
                    return;
                }
                // The active file that moves along is already known at its new location. Without this, the rewritten
                // tab makes the tree reveal that file and shift the selection away from the dragged entry.
                // Tab paths use the `file_path` form; `from`/`to` are normalized first, as in `rewrite_file_tab`.
                let (from_tab, to_tab) = (oncode_ui::editor::file_path(&from), oncode_ui::editor::file_path(&to));
                if let Some(active) = this.active_path.as_deref().and_then(|active| moved_path(active, &from_tab, &to_tab)) {
                    this.active_path = Some(active);
                }
                this.apply_rename(&from, &to, window, cx);
                this.tree.move_state(&from, &to);
                // Both folders' contents changed. Loading starts before `reveal`, so the old contents of the target
                // folder are not trusted; the result is then opened, selected, and scrolled into view.
                this.start_load(&from_dir, cx);
                if !same_path(&from_dir, &to_dir) {
                    this.start_load(&to_dir, cx);
                }
                this.reveal(&to, cx);
            });
        })
        .detach();
    }

    /// Copies files and folders from outside the project (dragged from the OS) into `dest_dir`. It does not overwrite:
    /// a name that already exists makes the whole copy rejected with a message.
    fn copy_in(&mut self, sources: Vec<String>, dest_dir: String, window: &mut Window, cx: &mut Context<Self>) {
        if sources.is_empty() {
            return;
        }
        let (core, label, dir) = (self.env.core.clone(), self.env.window.to_string(), dest_dir.clone());
        let task = spawn_core(cx, async move { fs::fs_copy_into(&core, &label, sources, dir).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Err(error) => overlay::show_error(&with_context(error, "Could not copy the dropped files".to_string()), window, cx),
                Ok(copied) => {
                    this.start_load(&dest_dir, cx);
                    if let Some(first) = copied.first() {
                        this.reveal(first, cx);
                    }
                }
            });
        })
        .detach();
    }

    // ----- Context menu -----

    fn build_menu(this: &WeakEntity<Self>, mut menu: PopupMenu, cx: &App) -> PopupMenu {
        let Some(explorer) = this.upgrade() else { return menu };
        let (root, entry) = {
            let explorer = explorer.read(cx);
            (explorer.root.clone(), explorer.menu_target.as_deref().and_then(|path| explorer.tree.entry(path)).cloned())
        };
        let Some(root) = root else { return menu };
        // The action runs after the menu closes, outside the menu's own update.
        let item = |label: &str, run: Box<dyn Fn(&mut Self, &mut Window, &mut Context<Self>)>| {
            let this = this.clone();
            let run: std::rc::Rc<dyn Fn(&mut Self, &mut Window, &mut Context<Self>)> = std::rc::Rc::from(run);
            PopupMenuItem::new(label.to_string()).on_click(move |_, window, cx| {
                let (this, run) = (this.clone(), run.clone());
                window.defer(cx, move |window, cx| {
                    let _ = this.update(cx, |this, cx| run(this, window, cx));
                });
            })
        };
        match entry {
            Some(entry) => {
                if !entry.is_dir {
                    let (a, b) = (entry.path.clone(), entry.path.clone());
                    menu = menu
                        .item(item("Open", Box::new(move |this, window, cx| this.open(&a, false, false, window, cx))))
                        .item(item("Open to the Side", Box::new(move |this, window, cx| this.open(&b, false, true, window, cx))))
                        .separator();
                }
                // Inside the clicked folder; for a file, in its parent folder.
                let dir = if entry.is_dir { entry.path.clone() } else { dir_name(&entry.path).to_string() };
                let (new_file_dir, new_folder_dir) = (dir.clone(), dir);
                let (rename, delete) = (entry.clone(), entry.clone());
                let (copy, copy_relative, reveal) = (entry.path.clone(), entry.path.clone(), entry.path.clone());
                menu.item(item("New File…", Box::new(move |this, window, cx| this.new_entry(EntryKind::File, Some(new_file_dir.clone()), window, cx))))
                    .item(item("New Folder…", Box::new(move |this, window, cx| this.new_entry(EntryKind::Folder, Some(new_folder_dir.clone()), window, cx))))
                    .separator()
                    .item(item("Rename…", Box::new(move |this, window, cx| this.rename_entry(rename.clone(), window, cx))))
                    .item(item("Delete (Move to Trash)…", Box::new(move |this, window, cx| this.delete_entry(delete.clone(), window, cx))))
                    .separator()
                    .item(item(REVEAL_LABEL, Box::new(move |this, _, cx| this.reveal_in_file_manager(&reveal, cx))))
                    .item(item("Copy Path", Box::new(move |this, _, cx| this.copy_path(&copy, false, cx))))
                    .item(item("Copy Relative Path", Box::new(move |this, _, cx| this.copy_path(&copy_relative, true, cx))))
            }
            None => {
                let (new_file_dir, new_folder_dir) = (root.clone(), root);
                menu.item(item("New File…", Box::new(move |this, window, cx| this.new_entry(EntryKind::File, Some(new_file_dir.clone()), window, cx))))
                    .item(item("New Folder…", Box::new(move |this, window, cx| this.new_entry(EntryKind::Folder, Some(new_folder_dir.clone()), window, cx))))
                    .separator()
                    .item(item("Refresh", Box::new(|this, _, cx| this.refresh(cx))))
                    .item(item("Collapse All", Box::new(|this, _, cx| this.collapse_all(cx))))
            }
        }
    }

    // ----- View -----

    fn row_id(&self, prefix: &str, path: &str) -> SharedString {
        let relative = self.root.as_deref().map_or_else(|| path.to_string(), |root| relative_path(root, path));
        SharedString::from(format!("{prefix}-{relative}"))
    }

    fn row_padding(depth: usize) -> gpui_kit::Pixels {
        px(6. + depth as f32 * INDENT)
    }

    fn render_entry(&self, entry: &DirEntry, depth: usize, expanded: bool, dropping: bool, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let decoration = decoration_for(&self.decorations, &entry.path, entry.is_dir);
        let mark = decoration.as_ref().map(|decoration| mark_color(decoration.status, theme.is_dark()));
        let tint = if entry.ignored { Some(theme.muted_foreground.opacity(0.6)) } else { mark };
        let selected = self.tree.selected.as_deref() == Some(entry.path.as_str());
        let active = self.active_path.as_deref() == Some(entry.path.as_str());
        let focused = self.focus_handle.is_focused(window);

        let relative = self.root.as_deref().map_or_else(|| entry.path.clone(), |root| relative_path(root, &entry.path));
        let mut title = vec![relative];
        if entry.is_symlink {
            title.push("symbolic link".into());
        }
        if entry.ignored {
            title.push("ignored".into());
        }
        if let Some(decoration) = &decoration {
            title.push(decoration.title.clone());
        }
        let title = SharedString::from(title.join(" • "));

        // Language / file type logo, and special folder icons (src, tests, node_modules, …).
        let icon = if entry.is_dir { folder_icon(&entry.path) } else { file_icon(&entry.path) };
        let ignored = entry.ignored;
        let (path, is_dir) = (entry.path.clone(), entry.is_dir);
        let menu_path = entry.path.clone();
        let dragged = DraggedEntry { path: entry.path.clone(), is_dir: entry.is_dir };
        h_flex()
            .id(self.row_id("explorer-row", &entry.path))
            .test_support()
            .w_full()
            .h(px(ROW_HEIGHT))
            .flex_none()
            .gap_1()
            .pl(Self::row_padding(depth))
            .pr_2()
            .text_sm()
            .when(selected && focused, |el| el.bg(theme.list_active))
            .when(selected && !focused, |el| el.bg(theme.list_active.opacity(0.6)))
            .when(active && !selected, |el| el.bg(theme.list_hover))
            .when(!selected && !active, |el| el.hover(|style| style.bg(theme.list_hover)))
            // The drop target folder and its contents are highlighted, overriding the selection color.
            .when(dropping, |el| el.bg(theme.drop_target))
            .tooltip(move |window, cx| Tooltip::new(title.clone()).build(window, cx))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| this.on_row_click(&path, is_dir, event.click_count(), window, cx)))
            // Drag to another folder to move, or to the editor area to open (files).
            .on_drag(dragged, |dragged, _, _, cx| {
                cx.stop_propagation();
                let icon = if dragged.is_dir { Lucide::Folder } else { Lucide::FileText };
                cx.new(|_| TabDragPreview::new(base_name(&dragged.path).to_string(), Some(icon)))
            })
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, cx| {
                    this.menu_target = Some(menu_path.clone());
                    this.select(Some(menu_path.clone()), cx);
                }),
            )
            .child(div().flex_none().w(px(14.)).text_color(theme.muted_foreground).when(entry.is_dir, |el| {
                el.child(Icon::new(if expanded { Lucide::ChevronDown } else { Lucide::ChevronRight }).xsmall())
            }))
            .child(icon.when(ignored, |el| el.opacity(0.5)))
            .child(div().flex_1().min_w_0().truncate().when_some(tint, |el, tint| el.text_color(tint)).child(SharedString::from(entry.name.clone())))
            .when_some(decoration, |el, decoration| {
                let color = mark_color(decoration.status, theme.is_dark());
                if decoration.letter.is_empty() {
                    el.child(div().flex_none().mx_1().size(px(6.)).rounded_full().bg(color.opacity(0.85)))
                } else {
                    el.child(div().flex_none().text_xs().text_color(color).child(decoration.letter))
                }
            })
            .into_any_element()
    }

    fn render_note(&self, index: usize, depth: usize, text: &str, error: bool, dropping: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        div()
            .id(("explorer-note", index))
            .w_full()
            .h(px(ROW_HEIGHT))
            .flex_none()
            .pl(Self::row_padding(depth) + px(20.))
            .pr_2()
            .text_xs()
            .line_height(px(ROW_HEIGHT))
            .truncate()
            .when(dropping, |el| el.bg(theme.drop_target))
            .when(error, |el| el.text_color(theme.danger))
            .when(!error, |el| el.italic().text_color(theme.muted_foreground))
            .child(SharedString::from(text.to_string()))
            .into_any_element()
    }

    fn render_edit(&self, depth: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let Some(edit) = &self.edit else { return div().h(px(ROW_HEIGHT)).into_any_element() };
        let (icon, expanded_twist) = match &edit.kind {
            EditKind::Create(EntryKind::Folder) => (Lucide::Folder, true),
            EditKind::Create(EntryKind::File) => (Lucide::File, false),
            EditKind::Rename(entry) if entry.is_dir => (Lucide::Folder, true),
            EditKind::Rename(_) => (Lucide::File, false),
        };
        h_flex()
            .id("explorer-edit")
            .w_full()
            .h(px(ROW_HEIGHT))
            .flex_none()
            .gap_1()
            .pl(Self::row_padding(depth))
            .pr_2()
            .text_sm()
            // Escape cancels before the input gets a chance to handle it.
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.cancel_edit(true, window, cx);
                }
            }))
            .child(div().flex_none().w(px(14.)).text_color(theme.muted_foreground).when(expanded_twist, |el| el.child(Icon::new(Lucide::ChevronRight).xsmall())))
            .child(Icon::new(icon).small().flex_none().text_color(theme.muted_foreground))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(ROW_HEIGHT - 2.))
                    .overflow_hidden()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(if edit.error.is_some() { theme.danger } else { theme.ring })
                    .bg(theme.background)
                    .child(Input::new(&edit.input).id("explorer-edit-input").xsmall().appearance(false)),
            )
            .into_any_element()
    }

    /// The drop target folder whose rows are highlighted: that folder and its contents. The project root is not highlighted
    /// on a row, but on the whole tree (see `render_tree`).
    fn highlighted_folder(&self, cx: &App) -> Option<String> {
        let target = self.active_drop_target(cx)?;
        (!self.root.as_deref().is_some_and(|root| same_path(root, target))).then(|| target.to_string())
    }

    fn render_rows(&mut self, range: Range<usize>, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let target = self.highlighted_folder(cx);
        let inside = |path: &str| target.as_deref().is_some_and(|dir| is_same_or_under(path, dir));
        range
            .map(|index| match self.display.get(index).copied() {
                Some(Display::Tree(row)) => match &self.rows[row] {
                    Row::Entry { entry, depth, expanded, .. } => self.render_entry(entry, *depth, *expanded, inside(&entry.path), window, cx),
                    Row::Note { depth, text, error, parent } => self.render_note(index, *depth, text, *error, inside(parent), cx),
                },
                Some(Display::Edit { depth }) => self.render_edit(depth, cx),
                Some(Display::EditError { depth }) => {
                    let text = self.edit.as_ref().and_then(|edit| edit.error.clone()).unwrap_or_default();
                    self.render_note(index, depth, &text, true, false, cx)
                }
                None => div().h(px(ROW_HEIGHT)).into_any_element(),
            })
            .collect()
    }

    fn render_toolbar(&self, root: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let name = base_name(root).to_string();
        let button = |id: &'static str, icon: Lucide, tooltip: &'static str| Button::new(id).ghost().xsmall().icon(Icon::new(icon)).tooltip(tooltip);
        h_flex()
            .id("explorer-toolbar")
            .w_full()
            .h(px(28.))
            .flex_none()
            .pl_3()
            .pr_1()
            .gap_0p5()
            .border_b_1()
            .border_color(theme.border)
            .child(div().flex_1().min_w_0().truncate().text_xs().font_weight(gpui_kit::FontWeight::SEMIBOLD).child(SharedString::from(name)))
            .child(button("explorer-new-file", Lucide::FilePlus, "New File…").on_click(cx.listener(|this, _, window, cx| this.new_entry(EntryKind::File, None, window, cx))))
            .child(button("explorer-new-folder", Lucide::FolderPlus, "New Folder…").on_click(cx.listener(|this, _, window, cx| this.new_entry(EntryKind::Folder, None, window, cx))))
            .child(button("explorer-refresh", Lucide::RefreshCw, "Refresh").on_click(cx.listener(|this, _, _, cx| this.refresh(cx))))
            .child(button("explorer-collapse-all", Lucide::ChevronsDownUp, "Collapse All").on_click(cx.listener(|this, _, _, cx| this.collapse_all(cx))))
    }

    fn render_tree(&self, root: &str, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let message = |el: gpui_kit::Div| el.px_3().py_3().text_sm().text_color(theme.muted_foreground);
        let root_state = self.tree.dirs.get(root);
        let body: AnyElement = if self.list_shown() {
            uniform_list("explorer-rows", self.display.len(), cx.processor(|this, range: Range<usize>, window, cx| this.render_rows(range, window, cx)))
                .track_scroll(&self.scroll)
                .size_full()
                .into_any_element()
        } else if let Some(error) = root_state.filter(|state| state.entries.is_none()).and_then(|state| state.error.clone()) {
            v_flex()
                .px_3()
                .py_3()
                .gap_2()
                .items_start()
                .text_sm()
                .child(div().id("explorer-error").test_support().text_color(theme.danger).child(SharedString::from(error)))
                .child(Button::new("explorer-retry").label("Retry").outline().small().on_click(cx.listener(|this, _, _, cx| this.refresh(cx))))
                .into_any_element()
        } else if root_state.is_some_and(|state| state.entries.is_some()) {
            message(div()).child("This folder is empty.").into_any_element()
        } else {
            message(div()).child("Loading…").into_any_element()
        };

        // Dropped on the project root (the tree background, or a file row at the root): the whole tree is highlighted.
        let root_target = self.active_drop_target(cx).is_some_and(|dir| same_path(root, dir));
        let this = cx.entity().downgrade();
        div()
            .id("explorer-tree")
            .test_support()
            .track_focus(&self.focus_handle)
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .when(root_target, |el| el.bg(theme.drop_target))
            .on_key_down(cx.listener(Self::on_key_down))
            // Drag and drop is handled in one place: the row under the pointer is computed from its position,
            // so the empty background below the last row (the project root) is handled too.
            .on_drag_move::<DraggedEntry>(cx.listener(|this, event: &DragMoveEvent<DraggedEntry>, _, cx| {
                let dragged = event.drag(cx).clone();
                this.track_drop_target(Some(&dragged), event.bounds, event.event.position, cx);
            }))
            .on_drag_move::<ExternalPaths>(cx.listener(|this, event: &DragMoveEvent<ExternalPaths>, _, cx| this.track_drop_target(None, event.bounds, event.event.position, cx)))
            .on_drop(cx.listener(|this, dragged: &DraggedEntry, window, cx| this.drop_entry(dragged, window, cx)))
            .on_drop(cx.listener(|this, dropped: &ExternalPaths, window, cx| this.drop_external(dropped, window, cx)))
            // The context menu is built from this target; the row under the pointer sets it again.
            .capture_any_mouse_down(cx.listener(|this, _, _, _| this.menu_target = None))
            .on_any_mouse_down(cx.listener(|this, _, window, cx| {
                if this.edit.is_none() {
                    this.focus_handle.focus(window, cx);
                }
            }))
            .child(body)
            .context_menu(move |menu, _, cx| Self::build_menu(&this, menu, cx))
            .into_any_element()
    }
}

impl Render for Explorer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(root) = self.root.clone() else {
            return v_flex()
                .id("explorer-empty")
                .size_full()
                .px_3()
                .py_4()
                .gap_3()
                .items_start()
                .text_sm()
                .child(div().text_color(cx.theme().muted_foreground).child("No folder opened"))
                .child(Button::new("explorer-open-folder").label("Open Folder").primary().small().on_click(|_, window, cx| {
                    oncode_ui::execute("file.openFolder", Value::Null, window, cx);
                }))
                .into_any_element();
        };
        v_flex()
            .id("explorer")
            .size_full()
            .when(!self.env.trusted, |el| el.child(restricted::banner(cx)))
            .child(self.render_toolbar(&root, cx))
            .child(self.render_tree(&root, cx))
            .into_any_element()
    }
}
