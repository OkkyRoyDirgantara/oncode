//! File buffer: one per path per window, shared by all tabs/splits that show it.
//!
//! Each view has its own `EditorState` (separate cursor and scroll); `Buffer` keeps
//! the content of all of them the same by forwarding every edit to the other states.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::input::{EditorState, InputEditsEvent, InputEvent, Rope, RopeExt as _};
use gpui_kit::{App, AppContext as _, Context, Entity, EntityId, EventEmitter, SharedString, Subscription, Task, WeakEntity, Window};
use oncode_core::api::backup::{self as backup_api, BufferBackup};
use oncode_core::api::fs;
use oncode_core::settings::AutoSave;
use oncode_core::AppError;
use oncode_workspace::editorconfig::EditorConfig;
use oncode_workspace::{Eol, OpenedFile};

use crate::overlay::Choice;
use crate::spawn_core;
use crate::workbench::{Env, WorkbenchEvent};

/// Whether a file's modification time and size are the same as recorded earlier. The time is compared to the
/// microsecond: one that went through JSON (a hot exit backup) can come back off by the last bit, since
/// serde_json's default float parsing is not exact.
fn same_stat(mtime_ms: f64, size: u32, other_mtime_ms: f64, other_size: u32) -> bool {
    size == other_size && (mtime_ms - other_mtime_ms).abs() < 0.001
}

/// The encoding name and BOM for an `.editorconfig` `charset` that OnCode can write.
fn charset_encoding(charset: &str) -> Option<(&'static str, bool)> {
    match charset {
        "utf-8" => Some(("UTF-8", false)),
        "utf-8-bom" => Some(("UTF-8", true)),
        "latin1" => Some(("windows-1252", false)),
        _ => None,
    }
}

/// How long after the last edit the hot exit backup is written.
const BACKUP_DELAY: Duration = Duration::from_secs(1);

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
    /// The file's `.editorconfig` properties were read (again); views re-apply their indentation.
    ConfigChanged,
}

/// What a save does when the file changed on disk since it was loaded or last saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnDiskChange {
    /// Ask whether to overwrite it or reload it.
    Ask,
    /// Write nothing and mark the buffer as conflicted.
    Keep,
    /// Write without checking.
    Overwrite,
}

/// Whether the "before save" step ([`super::format::before_save`]: Format on Save, `.editorconfig`
/// whitespace rules) runs for a save.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Participants {
    Run,
    /// The content is written as it is: a programmatic write such as Replace in Files, or auto-save
    /// after a delay (formatting text while the user is still typing would move it under the cursor).
    Skip,
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
    /// Held by the save that is running (see [`Buffer::save`]).
    save_turn: Arc<tokio::sync::Mutex<()>>,
    /// A hot exit backup of this buffer exists on disk, or has been asked for (see [`Buffer::write_backup`]).
    backed_up: bool,
    /// A backup to apply once the file has loaded (hot exit restore).
    pending_restore: Option<BufferBackup>,
    /// The `.editorconfig` properties for this file.
    editorconfig: EditorConfig,
    /// The line ending or encoding was changed and not saved yet (the text itself may be unchanged).
    format_changed: bool,
    _load: Task<()>,
    _auto_save: Task<()>,
    _backup: Task<()>,
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
            save_turn: Arc::default(),
            backed_up: false,
            pending_restore: None,
            editorconfig: EditorConfig::default(),
            format_changed: false,
            _load: Task::ready(()),
            _auto_save: Task::ready(()),
            _backup: Task::ready(()),
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

    /// The file starts with a byte order mark.
    pub fn has_bom(&self) -> bool {
        self.bom
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
        self.delete_backup(cx);
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
        let dirty = self.format_changed || {
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
        self.schedule_backup(window, cx);
    }

    /// The folder whose hot exit backups this buffer belongs to, if hot exit is on.
    fn hot_exit_root(&self, cx: &App) -> Option<String> {
        let enabled = self.env.workbench.upgrade().map(|w| w.read(cx).settings().files.hot_exit).unwrap_or(false);
        enabled.then(|| self.env.root_string()).flatten()
    }

    /// Keeps a backup of the unsaved content a moment after the last edit; removes it when the content
    /// is back to what is on disk.
    fn schedule_backup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.dirty {
            self._backup = Task::ready(());
            self.delete_backup(cx);
            return;
        }
        if self.hot_exit_root(cx).is_none() {
            return;
        }
        self._backup = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(BACKUP_DELAY).await;
            let Ok(write) = this.update(cx, |this, cx| this.write_backup(cx)) else { return };
            write.await;
        });
    }

    /// Writes the backup of the current content now. Resolves to `true` when it is on disk (or there is
    /// nothing to keep), `false` if hot exit is off or the write failed.
    pub fn write_backup(&mut self, cx: &mut Context<Self>) -> Task<bool> {
        if !self.dirty {
            return Task::ready(true);
        }
        let Some(root) = self.hot_exit_root(cx) else { return Task::ready(false) };
        let Some(text) = self.text(cx) else { return Task::ready(false) };
        let backup = BufferBackup {
            path: self.path.clone(),
            text: text.to_string(),
            encoding: self.encoding.clone(),
            bom: self.bom,
            eol: match self.eol {
                Eol::Lf => "lf".into(),
                Eol::Crlf => "crlf".into(),
            },
            mtime_ms: self.mtime_ms,
            size: self.size,
        };
        // Marked now rather than once written: a save or discard right after this must still remove the
        // backup, and the backend applies the two in the order they were asked for.
        self.backed_up = true;
        let task = spawn_core(cx, backup_api::backup_write(&self.env.core, root, backup));
        cx.background_spawn(async move { task.await.is_ok() })
    }

    /// Removes this buffer's backup, if it has one (saved, reverted, or its changes discarded).
    fn delete_backup(&mut self, cx: &mut Context<Self>) {
        self.delete_backup_of(self.path.clone(), cx);
    }

    fn delete_backup_of(&mut self, path: String, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.backed_up) {
            return;
        }
        let Some(root) = self.env.root_string() else { return };
        spawn_core(cx, backup_api::backup_delete(&self.env.core, root, path)).detach();
    }

    /// Hot exit restore: shows the backed-up content as unsaved changes, now or once the file has loaded.
    /// If the file changed on disk after the backup was made, the buffer is marked as conflicted, which
    /// asks the user to keep their version or reload.
    pub fn restore_backup(&mut self, backup: BufferBackup, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_ready() {
            self.pending_restore = Some(backup);
            return;
        }
        self.apply_backup(backup, window, cx);
    }

    fn apply_backup(&mut self, backup: BufferBackup, window: &mut Window, cx: &mut Context<Self>) {
        self.backed_up = true;
        if self.is_untitled() {
            self.encoding = backup.encoding.clone();
            self.bom = backup.bom;
            self.eol = if backup.eol == "crlf" { Eol::Crlf } else { Eol::Lf };
        } else if !same_stat(backup.mtime_ms, backup.size, self.mtime_ms, self.size) {
            self.conflict = true;
        }
        let states: Vec<Entity<EditorState>> = self.states.iter().filter_map(|(state, _)| state.upgrade()).collect();
        if states.is_empty() {
            self.initial = Some(backup.text.clone());
        } else {
            for state in states {
                state.update(cx, |state, cx| state.set_value(backup.text.clone(), window, cx));
            }
        }
        self.dirty = backup.text.len() != self.saved.len() || Rope::from(backup.text.as_str()) != self.saved;
        if !self.dirty {
            self.delete_backup(cx);
        }
        cx.emit(BufferEvent::Edited);
        cx.notify();
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
                    this.auto_save(window, cx).detach();
                }
            });
        });
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reload_editorconfig(cx);
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
        self.format_changed = false;
        self.revision += 1;
        self.apply_editorconfig_defaults();
        cx.emit(BufferEvent::Loaded);
        match self.pending_restore.take().filter(|_| self.is_ready()) {
            Some(backup) => self.apply_backup(backup, window, cx),
            // A reload from disk drops the unsaved changes, and with them their backup.
            None => self.delete_backup(cx),
        }
        cx.notify();
    }

    /// The `.editorconfig` properties for this file (empty until read, and for new files).
    pub fn editorconfig(&self) -> &EditorConfig {
        &self.editorconfig
    }

    /// The tab size and whether to indent with spaces: `.editorconfig` first, then the settings.
    pub fn indentation(&self, settings: &oncode_core::settings::EditorSettings) -> (u32, bool) {
        let tab_size = self.editorconfig.tab_size().unwrap_or(settings.tab_size);
        let spaces = self.editorconfig.indent_with_tabs.map(|tabs| !tabs).unwrap_or(settings.insert_spaces);
        (tab_size, spaces)
    }

    /// Reads this file's `.editorconfig` properties (again, e.g. after an `.editorconfig` changed).
    pub fn reload_editorconfig(&mut self, cx: &mut Context<Self>) {
        if self.is_untitled() {
            return;
        }
        let (core, label, path) = (self.env.core.clone(), self.env.window.to_string(), self.path.clone());
        let task = spawn_core(cx, async move { fs::fs_editorconfig(&core, &label, path).await });
        cx.spawn(async move |this, cx| {
            let config = task.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                if this.editorconfig != config {
                    this.editorconfig = config;
                    this.apply_editorconfig_defaults();
                    cx.emit(BufferEvent::ConfigChanged);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// A file without any line break (e.g. a new, empty file) cannot show which line ending it uses, and an
    /// empty one has no encoding yet: `.editorconfig` decides them.
    fn apply_editorconfig_defaults(&mut self) {
        if !self.is_ready() || self.saved.to_string().contains('\n') {
            return;
        }
        match self.editorconfig.end_of_line.as_deref() {
            Some("crlf") => self.eol = Eol::Crlf,
            Some("lf") => self.eol = Eol::Lf,
            _ => {}
        }
        if self.saved.len() == 0 {
            if let Some((encoding, bom)) = self.editorconfig.charset.as_deref().and_then(charset_encoding) {
                self.encoding = encoding.into();
                self.bom = bom;
            }
        }
    }

    /// Change End of Line Sequence: the file is written with `eol` from the next save on.
    pub fn set_eol(&mut self, eol: Eol, cx: &mut Context<Self>) {
        if self.eol == eol {
            return;
        }
        self.eol = eol;
        self.mark_format_changed(cx);
    }

    /// Save with Encoding: the file is written with `encoding` (a WHATWG label) and `bom` from the next save on.
    pub fn set_encoding(&mut self, encoding: String, bom: bool, cx: &mut Context<Self>) {
        if self.encoding == encoding && self.bom == bom {
            return;
        }
        self.encoding = encoding;
        self.bom = bom;
        self.mark_format_changed(cx);
    }

    fn mark_format_changed(&mut self, cx: &mut Context<Self>) {
        if self.is_untitled() {
            return;
        }
        self.format_changed = true;
        self.dirty = true;
        cx.emit(BufferEvent::Edited);
        cx.notify();
    }

    /// Reopen with Encoding: reads the file again, decoded with `encoding` (a WHATWG label) instead of the
    /// detected one. Unsaved changes are discarded; the caller asks first.
    pub fn reopen_with_encoding(&mut self, encoding: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_untitled() {
            return;
        }
        let (core, label, path) = (self.env.core.clone(), self.env.window.to_string(), self.path.clone());
        let task = spawn_core(cx, async move { fs::fs_open_with_encoding(&core, &label, path, encoding).await });
        self._load = cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| this.install(result, window, cx));
        });
    }

    /// Reloads from disk, discarding unsaved changes.
    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_untitled() {
            return;
        }
        self.load(window, cx);
    }

    /// Saves to disk with its original encoding and line ending. Resolves to `false` on failure or when the
    /// user cancels. A new file first asks where to save it.
    ///
    /// If the file changed on disk since it was loaded or last saved, nothing is written until the user
    /// chooses to overwrite it (or to reload it instead). The file is checked here and not only when the
    /// watcher reports a change: the watcher skips gitignored paths such as `.env`, and its report can
    /// arrive after the save.
    ///
    /// Every save path (Save, Save All, the close prompt, auto-save on focus change) runs the
    /// "before save" step first: Format on Save and the `.editorconfig` whitespace rules.
    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
        self.save_checked(OnDiskChange::Ask, Participants::Run, window, cx)
    }

    /// Saves the content exactly as it is, without the "before save" step: for programmatic writes
    /// such as Replace in Files, which stand for a file written by the backend.
    pub fn save_as_is(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
        self.save_checked(OnDiskChange::Ask, Participants::Skip, window, cx)
    }

    /// Saves even if the file changed on disk: the "Overwrite" choice.
    pub fn overwrite(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
        self.save_checked(OnDiskChange::Overwrite, Participants::Run, window, cx)
    }

    /// A save nobody asked for at that moment (auto-save): a file that changed on disk is left alone,
    /// and the buffer is marked as conflicted, which shows the banner. Auto-save after a delay skips the
    /// "before save" step, as in VS Code; auto-save on focus change runs it.
    pub(crate) fn auto_save(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
        let participants = if self.auto_save_mode(cx) == AutoSave::AfterDelay { Participants::Skip } else { Participants::Run };
        self.save_checked(OnDiskChange::Keep, participants, window, cx)
    }

    fn save_checked(&mut self, on_change: OnDiskChange, participants: Participants, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
        if self.is_untitled() {
            return self.save_untitled(window, cx);
        }
        if !self.is_ready() {
            return Task::ready(false);
        }
        let turn = self.save_turn.clone();
        cx.spawn_in(window, async move |this, cx| {
            // One save at a time: otherwise a save could take the file written by the save before it,
            // whose stat is not recorded yet, for a change made by another program.
            let _turn = turn.lock_owned().await;
            if participants == Participants::Run {
                // Outside any update of this buffer: the step reads the buffer and edits its editor state.
                let step = this.upgrade().and_then(|buffer| cx.update(|window, cx| super::format::before_save(&buffer, window, cx)).ok());
                if let Some(step) = step {
                    step.await;
                }
            }
            if on_change != OnDiskChange::Overwrite {
                let Ok(stat) = this.update(cx, |this, cx| {
                    let (core, label, path) = (this.env.core.clone(), this.env.window.to_string(), this.path.clone());
                    spawn_core(cx, async move { fs::fs_stat(&core, &label, path).await })
                }) else {
                    return false;
                };
                // A failed stat tells nothing about a change; the write reports the real problem. A file
                // that is gone is written again (the unsaved changes restore it).
                let stat = stat.await.ok().flatten();
                let Ok(changed) = this.update(cx, |this, _| this.conflict || stat.is_some_and(|stat| stat.mtime_ms != this.mtime_ms || stat.size != this.size)) else {
                    return false;
                };
                if changed {
                    let answer = this.update_in(cx, |this, window, cx| {
                        this.conflict = true;
                        cx.notify();
                        (on_change == OnDiskChange::Ask).then(|| this.ask_about_disk_change(window, cx))
                    });
                    let Ok(Some(answer)) = answer else { return false };
                    match answer.await {
                        Some("overwrite") => {}
                        Some("reload") => {
                            let _ = this.update_in(cx, |this, window, cx| this.reload(window, cx));
                            return false;
                        }
                        _ => return false,
                    }
                }
            }
            let Ok(write) = this.update_in(cx, |this, window, cx| this.write(window, cx)) else { return false };
            write.await
        })
    }

    fn ask_about_disk_change(&self, window: &mut Window, cx: &mut Context<Self>) -> Task<Option<&'static str>> {
        let name = self.file_name().to_string();
        let message = if self.dirty {
            format!("{name} was changed by another program after you opened it. Overwrite it with your version, or reload it and lose your unsaved changes?")
        } else {
            format!("{name} was changed by another program after you opened it. Overwrite it with the content shown here, or reload it?")
        };
        let choices = vec![Choice::new("cancel", "Cancel"), Choice::new("reload", "Reload from Disk"), Choice::new("overwrite", "Overwrite").danger()];
        crate::overlay::choose(format!("{name} changed on disk"), message, choices, window, cx)
    }

    /// Writes the current content, without checking the file on disk.
    fn write(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
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
                    this.format_changed = false;
                    // Edits can happen while saving.
                    this.dirty = this.primary().is_some_and(|primary| *primary.read(cx).text() != this.saved);
                    if !this.dirty {
                        this.delete_backup(cx);
                    }
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
            let Ok(task) = cx.update(|_, cx| {
                spawn_core(cx, async move {
                    // The new file's line ending and encoding come from `.editorconfig`, if it says.
                    let config = fs::fs_editorconfig(&core, &label, save_target.clone()).await.unwrap_or_default();
                    let eol = if config.end_of_line.as_deref() == Some("crlf") { Eol::Crlf } else { Eol::Lf };
                    let (encoding, bom) = config.charset.as_deref().and_then(charset_encoding).unwrap_or(("UTF-8", false));
                    fs::fs_save(&core, &label, save_target, content, encoding.into(), bom, eol).await
                })
            }) else {
                return false;
            };
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(stat) => {
                    this.saved = text;
                    this.mtime_ms = stat.mtime_ms;
                    this.size = stat.size;
                    this.dirty = this.primary().is_some_and(|primary| *primary.read(cx).text() != this.saved);
                    // The backup was kept under the untitled name.
                    this.delete_backup_of(from.clone(), cx);
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
                            // It was being edited as text; it stays that way even if the new name is an image's (e.g. an SVG).
                            if super::image::is_image(&to) {
                                super::image::set_file_mode(&mut next, super::image::FileMode::Text);
                            }
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
    /// The signature help shown for an editor changed (see [`Buffers::set_signature`]).
    SignatureChanged,
}

/// Signature help for a call at the cursor (the language server's `textDocument/signatureHelp`), shown
/// above the cursor by the editor view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignaturePopup {
    /// The signature, e.g. `fn copy(from: &str, to: &str) -> Result<()>`.
    pub label: String,
    /// The byte range of the current parameter inside `label`, if known.
    pub active: Option<std::ops::Range<usize>>,
    /// Documentation of the signature or the current parameter (plain text).
    pub documentation: Option<String>,
    /// Which overload this is and how many there are, when there is more than one.
    pub overload: Option<(usize, usize)>,
}

/// A label for a run of lines in the annotation column to the left of the editor (e.g. the blame column).
#[derive(Debug, Clone, PartialEq)]
pub struct LineAnnotation {
    /// First line, from 1.
    pub start_line: u32,
    pub line_count: u32,
    /// Shown once, at the first visible line of the run.
    pub text: SharedString,
    /// Passed to the column's click handler (e.g. a commit id).
    pub id: SharedString,
}

/// The annotation column of one file: its runs of lines (sorted, not overlapping) and what a click does.
#[derive(Clone)]
pub struct AnnotationColumn {
    pub annotations: Rc<Vec<LineAnnotation>>,
    pub on_click: Rc<dyn Fn(&str, &mut Window, &mut App)>,
}

/// A provider of the annotation column for a file path; `None` = no column for that file. Called during
/// render, so it must be cheap and must not update entities.
pub type AnnotationProvider = Rc<dyn Fn(&str, &App) -> Option<AnnotationColumn>>;

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
    /// Signature help per editor state.
    signatures: HashMap<EntityId, SignaturePopup>,
    /// Hot exit backups waiting for their buffer to be opened (see [`Buffers::restore_backups`]).
    pending_backups: HashMap<String, BufferBackup>,
    annotation_providers: Vec<(SharedString, AnnotationProvider)>,
}

impl EventEmitter<BuffersEvent> for Buffers {}

impl Buffers {
    pub fn new(env: Env) -> Self {
        Self { env, buffers: HashMap::new(), reveals: HashMap::new(), line_hints: Vec::new(), signatures: HashMap::new(), pending_backups: HashMap::new(), annotation_providers: Vec::new() }
    }

    /// Hot exit restore: each backup is applied to its buffer when that buffer is opened (right away if
    /// it is open already).
    pub fn restore_backups(&mut self, backups: Vec<BufferBackup>, window: &mut Window, cx: &mut Context<Self>) {
        for backup in backups {
            let path = file_path(&backup.path);
            match self.get(&path) {
                Some(buffer) => buffer.update(cx, |buffer, cx| buffer.restore_backup(backup, window, cx)),
                None => {
                    self.pending_backups.insert(path, backup);
                }
            }
        }
    }

    /// Writes the backups of all buffers with unsaved changes now (before a window closes). Resolves to
    /// `true` if every one is on disk.
    pub fn flush_backups(&self, cx: &mut Context<Self>) -> Task<bool> {
        let writes: Vec<Task<bool>> = self.dirty(cx).into_iter().map(|buffer| buffer.update(cx, |buffer, cx| buffer.write_backup(cx))).collect();
        cx.spawn(async move |_, _| {
            let mut all = true;
            for write in writes {
                all &= write.await;
            }
            all
        })
    }

    /// Shows (or with `None` hides) signature help in the editor whose state is `state`.
    pub fn set_signature(&mut self, state: EntityId, popup: Option<SignaturePopup>, cx: &mut Context<Self>) {
        let changed = match popup {
            Some(popup) => self.signatures.insert(state, popup.clone()).as_ref() != Some(&popup),
            None => self.signatures.remove(&state).is_some(),
        };
        if changed {
            cx.emit(BuffersEvent::SignatureChanged);
        }
    }

    /// The signature help shown in the editor whose state is `state`.
    pub fn signature(&self, state: EntityId) -> Option<&SignaturePopup> {
        self.signatures.get(&state)
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

    /// Installs an annotation column provider; the same id replaces the old one. A provider tells about
    /// changes through [`Buffers::refresh_line_hints`].
    pub fn set_annotation_provider(&mut self, id: impl Into<SharedString>, provider: AnnotationProvider, cx: &mut Context<Self>) {
        let id = id.into();
        self.annotation_providers.retain(|(existing, _)| *existing != id);
        self.annotation_providers.push((id, provider));
        cx.emit(BuffersEvent::LineHintsChanged);
    }

    /// The annotation column for a file, from the first provider that has one.
    pub fn annotation_column(&self, path: &str, cx: &App) -> Option<AnnotationColumn> {
        self.annotation_providers.iter().find_map(|(_, provider)| provider(path, cx))
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
        if let Some(backup) = self.pending_backups.remove(&path) {
            buffer.update(cx, |buffer, cx| buffer.restore_backup(backup, window, cx));
        }
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

#[cfg(test)]
mod tests {
    use super::same_stat;

    /// A modification time read back from a hot exit backup can differ in its last bit.
    #[test]
    fn stats_match_despite_float_rounding_but_not_real_changes() {
        let time = 1795762287027.6243_f64;
        assert!(same_stat(1795762287027.6245, 2, time, 2));
        assert!(!same_stat(time + 1.0, 2, time, 2), "a millisecond later is a change");
        assert!(!same_stat(time, 3, time, 2), "another size is a change");
    }
}
