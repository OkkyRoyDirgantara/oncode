//! Tampilan sidebar Explorer: pohon berkas yang dimuat per folder dan divirtualisasi, beserta
//! operasi berkasnya (buat, ganti nama, hapus ke tempat sampah, salin path).
//!
//! State pohon ada di [`TreeState`] (logika murni). Tampilan ini hanya menjalankan pemuatan:
//! setiap kali pohon berubah, folder terbuka yang belum punya isi diminta ke backend, dan
//! permintaan "tampilkan path ini" dilanjutkan sampai semua leluhurnya termuat.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, rgb, uniform_list, AnyElement, App, AppContext as _, ClickEvent, ClipboardItem, Context, Entity, FocusHandle, Focusable, Global, Hsla, InteractiveElement as _,
    IntoElement, KeyDownEvent, MouseButton, ParentElement as _, Render, ScrollStrategy, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task,
    TestSupportExt as _, UniformListScrollHandle, WeakEntity, Window,
};
use oncode_core::api::fs;
use oncode_core::AppError;
use oncode_ui::editor::{buffers, open_file, OpenFile};
use oncode_ui::overlay::{self, Confirm};
use oncode_ui::{repos, spawn_core, Env, FileMark, Lucide, WorkbenchEvent};
use oncode_workspace::DirEntry;
use serde_json::Value;

use crate::restricted;
use crate::tree::{
    ancestor_chain, base_name, build_decorations, decoration_for, dir_name, is_same_or_under, join_path, moved_path, relative_path, rewrite_file_tab, validate_entry_name,
    EntryDecoration, GitDecorations, NameOptions, Row, TreeState, INDENT, ROW_HEIGHT,
};

/// Jeda pengumpulan event perubahan berkas yang beruntun menjadi satu pemuatan ulang.
const FS_DEBOUNCE: Duration = Duration::from_millis(150);

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

/// Masukan nama di dalam pohon, untuk entri baru atau ganti nama.
struct Edit {
    kind: EditKind,
    /// Folder tempat entri dibuat, atau induk entri yang diganti namanya.
    parent: String,
    input: Entity<InputState>,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

/// Baris yang digambar: baris pohon, masukan nama, atau pesan validasinya.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Display {
    Tree(usize),
    Edit { depth: usize },
    EditError { depth: usize },
}

/// Folder yang terbuka dan baris terpilih per jendela dan folder proyek. `Workbench` (dan
/// tampilan ini) dibuat ulang saat status kepercayaan berubah; pohonnya tidak boleh ikut menciut.
#[derive(Default)]
struct TreeMemory(HashMap<(String, String), (HashSet<String>, Option<String>)>);

impl Global for TreeMemory {}

pub struct Explorer {
    env: Env,
    /// Folder proyek; `None` bila jendela belum membuka folder.
    root: Option<String>,
    tree: TreeState,
    rows: Vec<Row>,
    display: Vec<Display>,
    decorations: GitDecorations,
    /// Berkas di tab aktif; barisnya disorot.
    active_path: Option<String>,
    /// Path yang diminta tampil; dilanjutkan setiap kali isi sebuah folder datang.
    pending_reveal: Option<String>,
    edit: Option<Edit>,
    /// Sasaran menu konteks: entri di bawah penunjuk, atau `None` untuk latar pohon.
    menu_target: Option<String>,
    focus_handle: FocusHandle,
    scroll: UniformListScrollHandle,
    fs_paths: HashSet<String>,
    fs_overflow: bool,
    _fs_debounce: Task<()>,
    _subscriptions: Vec<Subscription>,
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

    // ----- Keadaan untuk fitur lain dan pengujian -----

    pub fn root(&self) -> Option<&str> {
        self.root.as_deref()
    }

    /// Baris pohon yang terlihat, dari atas ke bawah.
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn selected(&self) -> Option<&str> {
        self.tree.selected.as_deref()
    }

    pub fn is_expanded(&self, path: &str) -> bool {
        self.tree.expanded.contains(path)
    }

    /// Ada folder yang sedang dimuat.
    pub fn is_loading(&self) -> bool {
        self.tree.is_loading()
    }

    /// Masukan nama (entri baru atau ganti nama) sedang terbuka.
    pub fn is_editing(&self) -> bool {
        self.edit.is_some()
    }

    /// Pesan validasi masukan nama, bila ada.
    pub fn edit_error(&self) -> Option<SharedString> {
        self.edit.as_ref()?.error.clone()
    }

    /// Dekorasi Git untuk sebuah path absolut.
    pub fn decoration(&self, path: &str, is_dir: bool) -> Option<EntryDecoration> {
        decoration_for(&self.decorations, path, is_dir)
    }

    /// Fokus keyboard berada di pohon (bukan di masukan nama).
    pub fn tree_focused(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
    }

    /// Entri yang sedang terpilih, dicari di isi folder induknya.
    pub fn selected_entry(&self) -> Option<&DirEntry> {
        self.tree.entry(self.tree.selected.as_deref()?)
    }

    // ----- Pemuatan -----

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

    /// Dipanggil setelah setiap perubahan pohon: melanjutkan permintaan tampil, meminta isi
    /// folder terbuka yang belum dimuat, lalu menyusun ulang baris.
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

    /// Menyusun baris yang digambar: baris pohon ditambah masukan nama pada tempatnya.
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
                    // Tepat di bawah folder tujuan; di paling atas untuk akar proyek.
                    let (at, depth) = match self.rows.iter().position(|row| row.path() == Some(edit.parent.as_str())) {
                        Some(index) => (index + 1, self.rows[index].depth() + 1),
                        None => (0, 0),
                    };
                    // Keterangan "Empty" tidak perlu tampil di samping masukan nama.
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

    /// Melanjutkan permintaan tampil. Mengembalikan path-nya bila kini sudah terpilih dan terlihat.
    fn advance_reveal(&mut self, cx: &mut Context<Self>) -> Option<String> {
        let path = self.pending_reveal.clone()?;
        let chain = ancestor_chain(&self.tree.root, &path);
        if chain.is_empty() {
            // Di luar proyek.
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
                // Isi yang sedang dimuat ulang belum bisa dipercaya; tunggu jawabannya.
                Some(state) if state.loading => return None,
                Some(state) => {
                    if !state.entries.as_ref().is_some_and(|entries| entries.iter().any(|entry| entry.path == next)) {
                        // Sudah dihapus, tidak terbaca, atau disembunyikan (mis. di dalam `.git`).
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

    /// Membuka semua folder leluhur `path`, memilihnya, dan menggulir ke sana. Tidak berbuat
    /// apa-apa bila `path` tidak ada di pohon (di luar proyek atau sudah dihapus).
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
                // Akar yang dilaporkan backend bisa ditulis sedikit berbeda (mis. garis miring penutup).
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

    // ----- Perubahan dari luar -----

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

    // ----- Pohon -----

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

    /// Memuat ulang akar dan setiap folder yang terbuka; cache folder lain dibuang. Bentangan
    /// dan pilihan dipertahankan.
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
            // Klik kedua dari klik ganda tidak menutup lagi folder yang baru dibuka.
            if clicks <= 1 {
                self.toggle(path, cx);
            }
        } else {
            // Klik tunggal membuka pratinjau; klik ganda menjadikannya tab tetap.
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

    // ----- Entri baru dan ganti nama -----

    /// Folder tujuan untuk berkas/folder baru: folder terpilih, folder dari berkas terpilih, atau akar proyek.
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
                // Untuk berkas, hanya nama tanpa ekstensi yang terpilih; `.gitignore` dan folder terpilih seluruhnya.
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
            // Event dari masukan yang sudah digantikan diabaikan.
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
        // Fokus dipindah setelah pembaruan ini selesai, saat masukan sudah menjadi bagian tampilan.
        window.defer(cx, move |window, cx| {
            input.update(cx, |state, cx| {
                state.focus(window, cx);
                if stem_end > 0 {
                    state.set_selected_range(0..stem_end, cx);
                }
            });
        });
    }

    /// Membuka masukan nama untuk berkas atau folder baru di `dir` (bawaan: mengikuti pilihan).
    pub fn new_entry(&mut self, kind: EntryKind, dir: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else { return };
        let Some(parent) = dir.or_else(|| self.target_directory()) else { return };
        // Folder tujuan dibuka lebih dulu agar terlihat di mana entri baru akan muncul.
        for dir in ancestor_chain(&root, &parent).into_iter().skip(1) {
            self.tree.expand(&dir);
        }
        if parent != root {
            self.tree.expand(&parent);
        }
        self.start_edit(EditKind::Create(kind), parent, window, cx);
    }

    /// Membuka masukan nama untuk mengganti nama entri terpilih.
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
                // Folder antara (`a/b/c.txt`) mungkin sudah pernah dimuat; cache-nya kini usang.
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
                // Induk dimuat ulang; folder di bawah nama baru menyusul setelah nama itu ada di isi induknya.
                this.start_load(&parent, cx);
                this.tree_changed(cx);
            });
        })
        .detach();
    }

    /// Memindahkan buffer dan tab berkas dari `from` (berkas, atau folder beserta isinya) ke `to`.
    fn apply_rename(&mut self, from: &str, to: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workbench) = self.env.workbench.upgrade() else { return };
        // Buffer dipindah lebih dulu: isi yang belum disimpan ikut ke path baru sebelum tab dipasang ulang.
        // Buffer dan state editornya dipegang sampai tampilan baru menautkan diri.
        let mut held = Vec::new();
        if let Some(registry) = buffers(workbench.read(cx)) {
            for buffer in registry.read(cx).all() {
                let old = buffer.read(cx).path().to_string();
                let Some(new) = moved_path(&old, from, to) else { continue };
                registry.update(cx, |registry, cx| registry.rename(&old, &new, cx));
                if !buffer.read(cx).is_ready() {
                    // Pemuatan dari path lama tidak akan berhasil lagi.
                    buffer.update(cx, |buffer, cx| buffer.reload(window, cx));
                }
                let state = buffer.read(cx).primary();
                held.push((buffer, state));
            }
        }
        workbench.update(cx, |workbench, cx| workbench.update_tabs(|tab| rewrite_file_tab(tab, from, to), window, cx));
        drop(held);
    }

    // ----- Hapus dan salin path -----

    /// Memindahkan entri terpilih ke tempat sampah OS setelah konfirmasi.
    pub fn delete_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.selected_entry().cloned() else { return };
        self.delete_entry(entry, window, cx);
    }

    fn delete_entry(&mut self, entry: DirEntry, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workbench) = self.env.workbench.upgrade() else { return };
        // Berkas terbuka di dalam entri ini yang punya perubahan belum disimpan ikut hilang.
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
                // Memuat ulang induk sekaligus membuang keadaan pohon di bawah entri yang hilang.
                this.start_load(dir_name(&entry.path), cx);
                this.tree_changed(cx);
            });
        })
        .detach();
    }

    /// Menyalin path entri terpilih ke papan klip.
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

    // ----- Menu konteks -----

    fn build_menu(this: &WeakEntity<Self>, mut menu: PopupMenu, cx: &App) -> PopupMenu {
        let Some(explorer) = this.upgrade() else { return menu };
        let (root, entry) = {
            let explorer = explorer.read(cx);
            (explorer.root.clone(), explorer.menu_target.as_deref().and_then(|path| explorer.tree.entry(path)).cloned())
        };
        let Some(root) = root else { return menu };
        // Aksi dijalankan setelah menu tertutup, di luar pembaruan menu itu sendiri.
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
                // Di dalam folder yang diklik; untuk berkas, di folder induknya.
                let dir = if entry.is_dir { entry.path.clone() } else { dir_name(&entry.path).to_string() };
                let (new_file_dir, new_folder_dir) = (dir.clone(), dir);
                let (rename, delete) = (entry.clone(), entry.clone());
                let (copy, copy_relative) = (entry.path.clone(), entry.path.clone());
                menu.item(item("New File…", Box::new(move |this, window, cx| this.new_entry(EntryKind::File, Some(new_file_dir.clone()), window, cx))))
                    .item(item("New Folder…", Box::new(move |this, window, cx| this.new_entry(EntryKind::Folder, Some(new_folder_dir.clone()), window, cx))))
                    .separator()
                    .item(item("Rename…", Box::new(move |this, window, cx| this.rename_entry(rename.clone(), window, cx))))
                    .item(item("Delete (Move to Trash)…", Box::new(move |this, window, cx| this.delete_entry(delete.clone(), window, cx))))
                    .separator()
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

    // ----- Tampilan -----

    fn row_id(&self, prefix: &str, path: &str) -> SharedString {
        let relative = self.root.as_deref().map_or_else(|| path.to_string(), |root| relative_path(root, path));
        SharedString::from(format!("{prefix}-{relative}"))
    }

    fn row_padding(depth: usize) -> gpui_kit::Pixels {
        px(6. + depth as f32 * INDENT)
    }

    fn render_entry(&self, entry: &DirEntry, depth: usize, expanded: bool, window: &Window, cx: &mut Context<Self>) -> AnyElement {
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

        let icon = match (entry.is_dir, expanded) {
            (true, true) => Lucide::FolderOpen,
            (true, false) => Lucide::Folder,
            (false, _) => Lucide::File,
        };
        let (path, is_dir) = (entry.path.clone(), entry.is_dir);
        let menu_path = entry.path.clone();
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
            .tooltip(move |window, cx| Tooltip::new(title.clone()).build(window, cx))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| this.on_row_click(&path, is_dir, event.click_count(), window, cx)))
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
            .child(Icon::new(icon).small().flex_none().text_color(tint.unwrap_or(theme.muted_foreground)))
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

    fn render_note(&self, index: usize, depth: usize, text: &str, error: bool, cx: &mut Context<Self>) -> AnyElement {
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
            // Escape membatalkan sebelum masukan sempat menanganinya.
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

    fn render_rows(&mut self, range: Range<usize>, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        range
            .map(|index| match self.display.get(index).copied() {
                Some(Display::Tree(row)) => match &self.rows[row] {
                    Row::Entry { entry, depth, expanded, .. } => self.render_entry(entry, *depth, *expanded, window, cx),
                    Row::Note { depth, text, error, .. } => self.render_note(index, *depth, text, *error, cx),
                },
                Some(Display::Edit { depth }) => self.render_edit(depth, cx),
                Some(Display::EditError { depth }) => {
                    let text = self.edit.as_ref().and_then(|edit| edit.error.clone()).unwrap_or_default();
                    self.render_note(index, depth, &text, true, cx)
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
        let body: AnyElement = if self.edit.is_some() || root_state.is_some_and(|state| state.entries.as_ref().is_some_and(|entries| !entries.is_empty())) {
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

        let this = cx.entity().downgrade();
        div()
            .id("explorer-tree")
            .test_support()
            .track_focus(&self.focus_handle)
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .on_key_down(cx.listener(Self::on_key_down))
            // Menu konteks dibangun dari sasaran ini; baris di bawah penunjuk mengisinya kembali.
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
