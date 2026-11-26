//! File buffer: one per path per window, shared by all tabs/splits that show it.
//!
//! Each view has its own `EditorState` (separate cursor and scroll); `Buffer` keeps
//! the content of all of them the same by forwarding every edit to the other states.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::input::{EditorState, InputEditsEvent, InputEvent, Rope, RopeExt as _};
use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, SharedString, Subscription, Task, WeakEntity, Window};
use oncode_core::api::fs;
use oncode_core::settings::AutoSave;
use oncode_core::AppError;
use oncode_workspace::{Eol, OpenedFile};

use crate::spawn_core;
use crate::workbench::{Env, WorkbenchEvent};

/// Size limit for heavy features (highlighting, folding, LSP).
pub const LARGE_FILE_BYTES: u32 = 5 * 1024 * 1024;

/// Prefix of the path of a new file that has never been saved (`untitled:Untitled-1`).
pub const UNTITLED_PREFIX: &str = "untitled:";

/// The file path form used by tabs and buffers. On Windows the separator is normalized to
/// `\`, so `C:/x/a.rs` (a root from Git, used by SCM/diff/graph) and `C:\x\a.rs` (the explorer)
/// open the same tab and buffer. `untitled:` paths and paths on other OSes are left unchanged.
pub fn file_path(path: &str) -> String {
    if cfg!(windows) && !is_untitled(path) {
        path.replace('/', "\\")
    } else {
        path.to_string()
    }
}

/// `true` for a new file that has no place on disk yet.
pub fn is_untitled(path: &str) -> bool {
    path.starts_with(UNTITLED_PREFIX)
}

#[derive(Debug, Clone, PartialEq)]
pub enum BufferStatus {
    Loading,
    Ready,
    Binary { size: u32 },
    TooLarge { size: u32 },
    Error(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum BufferEvent {
    /// The content was loaded (or reloaded) from disk.
    Loaded,
    /// The content was edited.
    Edited,
    Saved,
}

pub struct Buffer {
    path: String,
    env: Env,
    status: BufferStatus,
    states: Vec<(WeakEntity<EditorState>, Vec<Subscription>)>,
    /// Content from disk waiting for the first `EditorState`.
    initial: Option<String>,
    /// Content as of the last load or save; the comparison baseline for the "unsaved" marker.
    saved: Rope,
    dirty: bool,
    encoding: String,
    bom: bool,
    eol: Eol,
    mtime_ms: f64,
    size: u32,
    read_only: bool,
    /// The file changed on disk while the buffer holds unsaved changes.
    conflict: bool,
    /// Incremented every time the content is reloaded from disk.
    revision: u64,
    _load: Task<()>,
    _auto_save: Task<()>,
}

impl EventEmitter<BufferEvent> for Buffer {}

impl Buffer {
    pub fn open(path: String, env: Env, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            path,
            env,
            status: BufferStatus::Loading,
            states: Vec::new(),
            initial: None,
            saved: Rope::from(""),
            dirty: false,
            encoding: "UTF-8".into(),
            bom: false,
            eol: Eol::Lf,
            mtime_ms: 0.0,
            size: 0,
            read_only: false,
            conflict: false,
            revision: 0,
            _load: Task::ready(()),
            _auto_save: Task::ready(()),
        };
        if is_untitled(&this.path) {
            // A new file: ready right away, empty, without reading the disk.
            this.status = BufferStatus::Ready;
            this.initial = Some(String::new());
            this.revision = 1;
        } else {
            this.load(window, cx);
        }
        this
    }

    /// A new file that has never been saved.
    pub fn is_untitled(&self) -> bool {
        is_untitled(&self.path)
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn file_name(&self) -> &str {
        if let Some(name) = self.path.strip_prefix(UNTITLED_PREFIX) {
            return name;
        }
        self.path.rsplit(['/', '\\']).next().unwrap_or(&self.path)
    }

    /// The file was renamed; only called by [`Buffers::rename`] so the registry stays consistent.
    fn set_path(&mut self, path: String, cx: &mut Context<Self>) {
        self.path = path;
        cx.notify();
    }

    pub fn status(&self) -> &BufferStatus {
        &self.status
    }

    pub fn is_ready(&self) -> bool {
        self.status == BufferStatus::Ready
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn has_conflict(&self) -> bool {
        self.conflict
    }

    pub fn encoding(&self) -> &str {
        &self.encoding
    }

    pub fn eol(&self) -> Eol {
        self.eol
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub fn is_large(&self) -> bool {
        self.size > LARGE_FILE_BYTES
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The first `EditorState` that is still alive; its content is the buffer content.
    pub fn primary(&self) -> Option<Entity<EditorState>> {
        self.states.iter().find_map(|(state, _)| state.upgrade())
    }

    /// The current content, or `None` if the buffer is not ready.
    pub fn text(&self, cx: &App) -> Option<Rope> {
        if !self.is_ready() {
            return None;
        }
        match self.primary() {
            Some(state) => Some(state.read(cx).text().clone()),
            None => self.initial.as_deref().map(Rope::from),
        }
    }

    /// Links a view's `EditorState`: its content is synced and its edits are forwarded.
    pub fn attach(&mut self, state: &Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) {
        self.states.retain(|(state, _)| state.upgrade().is_some());
        if self.is_ready() {
            let text = match self.primary() {
                Some(primary) => primary.read(cx).text().to_string(),
                None => self.initial.take().unwrap_or_default(),
            };
            state.update(cx, |state, cx| state.set_value(text, window, cx));
        }
        let subscriptions = vec![
            cx.subscribe_in(state, window, |this, state, event: &InputEditsEvent, window, cx| this.on_edits(state, event, window, cx)),
            cx.subscribe_in(state, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.on_changed(window, cx);
                }
            }),
            // The buffer can outlive its views (a Markdown preview holds it): when the last `EditorState` goes away,
            // its content moves back into the buffer, so a view opened later does not start empty.
            cx.observe_release(state, |this, released, _| this.keep_content_of(released.text())),
        ];
        self.states.push((state.downgrade(), subscriptions));
    }

    /// An `EditorState` was released. If it was the last one, the content it held becomes the buffer's own,
    /// to be handed to the next view (see [`Buffer::attach`]).
    fn keep_content_of(&mut self, text: &Rope) {
        if self.is_ready() && self.primary().is_none() {
            self.initial = Some(text.to_string());
        }
    }

    /// "Don't Save" on the tab of the view with `view`: unless another view still shows this buffer, it goes back
    /// to the last loaded or saved content. A buffer can outlive its views (a Markdown preview holds it), and would
    /// otherwise keep the discarded changes for the next view that opens the file.
    pub fn discard_changes(&mut self, view: &Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) {
        let shared = self.states.iter().filter_map(|(state, _)| state.upgrade()).any(|state| state != *view);
        if !self.is_ready() || shared {
            return;
        }
        let saved = self.saved.to_string();
        // `set_value` does not emit an event, so the dirty marker and the other observers are updated here.
        view.update(cx, |state, cx| state.set_value(saved, window, cx));
        self.dirty = false;
        cx.emit(BufferEvent::Edited);
        cx.notify();
    }

    /// Forwards edits from one state to the other states of this buffer.
    fn on_edits(&mut self, source: &Entity<EditorState>, event: &InputEditsEvent, window: &mut Window, cx: &mut Context<Self>) {
        let target = source.read(cx).text().clone();
        let others: Vec<Entity<EditorState>> = self.states.iter().filter_map(|(state, _)| state.upgrade()).filter(|state| state != source).collect();
        for other in others {
            // A state whose content is already the same is left alone; this also stops the echo,
            // because a forwarded edit emits an event from the receiving state.
            if *other.read(cx).text() == target {
                continue;
            }
            other.update(cx, |state, cx| {
                for (edit, after) in event.edits() {
                    // This state still holds the document from before the edit, so the old positions are computed from it.
                    let before = state.text().clone();
                    if edit.old_end_byte > before.len() {
                        break;
                    }
                    let range = lsp_types::Range { start: before.offset_to_position(edit.start_byte), end: before.offset_to_position(edit.old_end_byte) };
                    let new_text = after.slice(edit.start_byte..edit.new_end_byte).to_string();
                    state.apply_lsp_edits(&vec![lsp_types::TextEdit { range, new_text }], window, cx);
                }
                // Safety net: if per-edit forwarding misses, sync the whole content.
                if *state.text() != target {
                    state.set_value(target.to_string(), window, cx);
                }
                cx.notify();
            });
        }
    }

    fn on_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_ready() {
            return;
        }
        let Some(primary) = self.primary() else { return };
        let dirty = {
            let text = primary.read(cx).text();
            // Compare the whole content only if its length equals that of the saved content.
            text.len() != self.saved.len() || *text != self.saved
        };
        let changed = dirty != self.dirty;
        self.dirty = dirty;
        cx.emit(BufferEvent::Edited);
        if changed {
            cx.notify();
        }
        self.schedule_auto_save(window, cx);
    }

    fn auto_save_mode(&self, cx: &App) -> AutoSave {
        self.env.workbench.upgrade().map(|w| w.read(cx).settings().files.auto_save).unwrap_or_default()
    }

    fn schedule_auto_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A new file is not auto-saved: saving it means choosing a location.
        if self.auto_save_mode(cx) != AutoSave::AfterDelay || !self.dirty || self.conflict || self.is_untitled() {
            return;
        }
        self._auto_save = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.dirty && !this.conflict {
                    this.save(window, cx).detach();
                }
            });
        });
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (core, label, path) = (self.env.core.clone(), self.env.window.to_string(), self.path.clone());
        let task = spawn_core(cx, async move { fs::fs_open(&core, &label, path).await });
        self._load = cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| this.install(result, window, cx));
        });
    }

    fn install(&mut self, result: Result<OpenedFile, AppError>, window: &mut Window, cx: &mut Context<Self>) {
        match result {
            Ok(OpenedFile::Text(file)) => {
                self.saved = Rope::from(file.text.as_str());
                self.encoding = file.encoding;
                self.bom = file.bom;
                self.eol = file.eol;
                self.mtime_ms = file.stat.mtime_ms;
                self.size = file.stat.size;
                self.read_only = file.stat.read_only;
                self.status = BufferStatus::Ready;
                let states: Vec<Entity<EditorState>> = self.states.iter().filter_map(|(state, _)| state.upgrade()).collect();
                if states.is_empty() {
                    self.initial = Some(file.text);
                } else {
                    // `set_value` does not emit an event, so it is not forwarded as an edit.
                    for state in states {
                        state.update(cx, |state, cx| state.set_value(file.text.clone(), window, cx));
                    }
                }
            }
            Ok(OpenedFile::Binary { size }) => {
                self.status = BufferStatus::Binary { size };
                self.size = size;
            }
            Ok(OpenedFile::TooLarge { size }) => {
                self.status = BufferStatus::TooLarge { size };
                self.size = size;
            }
            Err(error) => self.status = BufferStatus::Error(error.message),
        }
        self.dirty = false;
        self.conflict = false;
        self.revision += 1;
        cx.emit(BufferEvent::Loaded);
        cx.notify();
    }

    /// Reloads from disk, discarding unsaved changes.
    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_untitled() {
            return;
        }
        self.load(window, cx);
    }

    /// Saves to disk with its original encoding and line ending. Resolves to `false` on failure.
    /// A new file first asks where to save it.
    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
        if self.is_untitled() {
            return self.save_untitled(window, cx);
        }
        let Some(text) = self.text(cx) else { return Task::ready(false) };
        let (core, label, path) = (self.env.core.clone(), self.env.window.to_string(), self.path.clone());
        let (encoding, bom, eol) = (self.encoding.clone(), self.bom, self.eol);
        let content = text.to_string();
        let task = spawn_core(cx, async move { fs::fs_save(&core, &label, path, content, encoding, bom, eol).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(stat) => {
                    this.saved = text;
                    this.mtime_ms = stat.mtime_ms;
                    this.size = stat.size;
                    this.conflict = false;
                    // Edits can happen while saving.
                    this.dirty = this.primary().is_some_and(|primary| *primary.read(cx).text() != this.saved);
                    cx.emit(BufferEvent::Saved);
                    cx.notify();
                    let saved = WorkbenchEvent::FileSaved(this.path.clone());
                    if let Some(workbench) = this.env.workbench.upgrade() {
                        // Deferred: saving can be triggered from inside a `Workbench` update.
                        window.defer(cx, move |_, cx| workbench.update(cx, |workbench, cx| workbench.broadcast(saved, cx)));
                    }
                    true
                }
                Err(error) => {
                    let mut error = error;
                    error.message = format!("Could not save {}: {}", this.file_name(), error.message);
                    crate::overlay::show_error(&error, window, cx);
                    false
                }
            })
            .unwrap_or(false)
        })
    }

    /// "Save As" for a new file: pick a path inside the project folder, save, then the buffer and
    /// its tab move to that path (the language and highlighting follow the new file name).
    fn save_untitled(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
        let Some(text) = self.text(cx) else { return Task::ready(false) };
        let Some(root) = self.env.root.clone() else {
            crate::overlay::toast_info("Open a folder to save new files.", window, cx);
            return Task::ready(false);
        };
        let suggested = format!("{}.txt", self.file_name());
        let chosen = cx.prompt_for_new_path(&root, Some(&suggested));
        let (core, label, from) = (self.env.core.clone(), self.env.window.to_string(), self.path.clone());
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(target))) = chosen.await else { return false };
            let target = target.to_string_lossy().into_owned();
            let (save_target, content) = (target.clone(), text.to_string());
            let Ok(task) = cx.update(|_, cx| spawn_core(cx, async move { fs::fs_save(&core, &label, save_target, content, "UTF-8".into(), false, Eol::Lf).await })) else {
                return false;
            };
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(stat) => {
                    this.saved = text;
                    this.mtime_ms = stat.mtime_ms;
                    this.size = stat.size;
                    this.dirty = this.primary().is_some_and(|primary| *primary.read(cx).text() != this.saved);
                    this.move_to_saved_path(from, target, window, cx);
                    true
                }
                Err(mut error) => {
                    if error.code == "invalidPath" {
                        error.message = "New files can only be saved inside the open folder.".into();
                    }
                    crate::overlay::show_error(&error, window, cx);
                    false
                }
            })
            .unwrap_or(false)
        })
    }

    /// Moves the buffer of a new file and its tab to the path where it was saved.
    fn move_to_saved_path(&mut self, from: String, to: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workbench) = self.env.workbench.upgrade() else { return };
        // The buffer and its editor state are held until the new tab links itself to this buffer.
        let held = (cx.entity(), self.primary());
        window.defer(cx, move |window, cx| {
            if let Some(registry) = super::buffers(workbench.read(cx)) {
                registry.update(cx, |registry, cx| registry.rename(&from, &to, cx));
            }
            workbench.update(cx, |workbench, cx| {
                workbench.update_tabs(
                    |tab| {
                        (tab.kind == "file" && tab.param_str("path") == Some(from.as_str())).then(|| {
                            let mut next = crate::shell::file_tab(&to);
                            next.preview = false;
                            next
                        })
                    },
                    window,
                    cx,
                );
                workbench.broadcast(WorkbenchEvent::FileSaved(to.clone()), cx);
            });
            drop(held);
        });
    }

    /// Reacts to a file change from outside: a clean buffer is reloaded, a dirty buffer is marked as conflicted.
    pub fn check_external_change(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<ExternalChange> {
        if self.status == BufferStatus::Loading || self.is_untitled() {
            return Task::ready(ExternalChange::Unchanged);
        }
        let (core, label, path) = (self.env.core.clone(), self.env.window.to_string(), self.path.clone());
        let task = spawn_core(cx, async move { fs::fs_stat(&core, &label, path).await });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(stat) = task.await else { return ExternalChange::Unchanged };
            this.update_in(cx, |this, window, cx| {
                let Some(stat) = stat else { return ExternalChange::Deleted };
                if stat.mtime_ms == this.mtime_ms && stat.size == this.size {
                    return ExternalChange::Unchanged;
                }
                if !this.is_ready() || !this.dirty {
                    this.reload(window, cx);
                    return ExternalChange::Reloaded;
                }
                this.mtime_ms = stat.mtime_ms;
                this.size = stat.size;
                this.conflict = true;
                cx.notify();
                ExternalChange::Conflict
            })
            .unwrap_or(ExternalChange::Unchanged)
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalChange {
    Unchanged,
    Reloaded,
    Conflict,
    Deleted,
}

/// The position to show when a file is opened.
#[derive(Debug, Clone, PartialEq)]
pub struct Reveal {
    /// Line number starting at 1.
    pub line: u32,
    /// Column starting at 0, in characters.
    pub column: u32,
    /// Length of the selection in characters (e.g. a search match).
    pub length: Option<u32>,
}

#[derive(Debug, Clone)]
pub enum BuffersEvent {
    /// A new buffer was created (not necessarily finished loading).
    Opened(Entity<Buffer>),
    /// A view created an `EditorState` for this buffer (where LSP installs its providers).
    EditorCreated { buffer: Entity<Buffer>, state: Entity<EditorState> },
    /// There is a pending position for this path; taken with [`Buffers::take_reveal`].
    Reveal(String),
    /// The end-of-line hint content changed; the editor view repaints its hint.
    LineHintsChanged,
}

/// A provider of faint text at the end of the cursor line (e.g. inline blame), like the "after"
/// decoration in VS Code. Takes the file path and the line (starting at 0); it is called during
/// render, so it must be cheap and must not update entities.
pub type LineHintProvider = Rc<dyn Fn(&str, u32, &App) -> Option<SharedString>>;

/// The buffer registry of one window. A buffer is released when no tab holds it any more.
pub struct Buffers {
    env: Env,
    buffers: HashMap<String, WeakEntity<Buffer>>,
    reveals: HashMap<String, Reveal>,
    line_hints: Vec<(SharedString, LineHintProvider)>,
}

impl EventEmitter<BuffersEvent> for Buffers {}

impl Buffers {
    pub fn new(env: Env) -> Self {
        Self { env, buffers: HashMap::new(), reveals: HashMap::new(), line_hints: Vec::new() }
    }

    /// Installs an end-of-line hint provider; the same id replaces the old one.
    pub fn set_line_hint_provider(&mut self, id: impl Into<SharedString>, provider: LineHintProvider, cx: &mut Context<Self>) {
        let id = id.into();
        self.line_hints.retain(|(existing, _)| *existing != id);
        self.line_hints.push((id, provider));
        cx.emit(BuffersEvent::LineHintsChanged);
    }

    /// The hint for a line (starting at 0): from the first provider that has content.
    pub fn line_hint(&self, path: &str, row: u32, cx: &App) -> Option<SharedString> {
        self.line_hints.iter().find_map(|(_, provider)| provider(path, row, cx))
    }

    /// Called by a provider when its hint content changes.
    pub fn refresh_line_hints(&mut self, cx: &mut Context<Self>) {
        cx.emit(BuffersEvent::LineHintsChanged);
    }

    /// The path for the next new file: `untitled:Untitled-N` with the smallest N that is not open yet.
    pub fn next_untitled_path(&self) -> String {
        (1..)
            .map(|n| format!("{UNTITLED_PREFIX}Untitled-{n}"))
            .find(|path| self.get(path).is_none())
            .expect("there is always an unused number")
    }

    pub fn get(&self, path: &str) -> Option<Entity<Buffer>> {
        self.buffers.get(&file_path(path))?.upgrade()
    }

    pub fn open(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) -> Entity<Buffer> {
        if let Some(existing) = self.get(path) {
            return existing;
        }
        self.buffers.retain(|_, buffer| buffer.upgrade().is_some());
        let env = self.env.clone();
        let path = file_path(path);
        let path_owned = path.clone();
        let buffer = cx.new(|cx| Buffer::open(path_owned, env, window, cx));
        self.buffers.insert(path, buffer.downgrade());
        cx.emit(BuffersEvent::Opened(buffer.clone()));
        buffer
    }

    /// File `from` was renamed to `to`: its buffer moves along with its unsaved content.
    /// Resolves to `false` if there is no buffer for `from`. Its tab is rewritten by the caller
    /// (`Workbench::update_tabs`); new views use this buffer as long as the caller holds it.
    pub fn rename(&mut self, from: &str, to: &str, cx: &mut Context<Self>) -> bool {
        let Some(buffer) = self.get(from) else { return false };
        let (from, to) = (file_path(from), file_path(to));
        let (from, to) = (from.as_str(), to.as_str());
        self.buffers.remove(from);
        buffer.update(cx, |buffer, cx| buffer.set_path(to.to_string(), cx));
        self.buffers.insert(to.to_string(), buffer.downgrade());
        if let Some(reveal) = self.reveals.remove(from) {
            self.reveals.insert(to.to_string(), reveal);
        }
        true
    }

    /// All buffers that are still in use.
    pub fn all(&self) -> Vec<Entity<Buffer>> {
        self.buffers.values().filter_map(WeakEntity::upgrade).collect()
    }

    pub fn dirty(&self, cx: &App) -> Vec<Entity<Buffer>> {
        self.all().into_iter().filter(|buffer| buffer.read(cx).is_dirty()).collect()
    }

    pub(crate) fn editor_created(&mut self, buffer: Entity<Buffer>, state: Entity<EditorState>, cx: &mut Context<Self>) {
        cx.emit(BuffersEvent::EditorCreated { buffer, state });
    }

    pub fn set_reveal(&mut self, path: &str, reveal: Reveal, cx: &mut Context<Self>) {
        let path = file_path(path);
        self.reveals.insert(path.clone(), reveal);
        cx.emit(BuffersEvent::Reveal(path));
    }

    pub fn take_reveal(&mut self, path: &str) -> Option<Reveal> {
        self.reveals.remove(&file_path(path))
    }
}
