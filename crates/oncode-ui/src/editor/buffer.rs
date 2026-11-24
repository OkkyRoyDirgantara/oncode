//! Buffer berkas: satu per path per jendela, dipakai bersama oleh semua tab/split yang menampilkannya.
//!
//! Setiap tampilan punya `EditorState` sendiri (kursor dan gulir terpisah); `Buffer` menjaga
//! isi semuanya tetap sama dengan meneruskan setiap suntingan ke state lain.

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

/// Batas ukuran untuk fitur berat (highlight, lipat, LSP).
pub const LARGE_FILE_BYTES: u32 = 5 * 1024 * 1024;

/// Awalan path berkas baru yang belum pernah disimpan (`untitled:Untitled-1`).
pub const UNTITLED_PREFIX: &str = "untitled:";

/// `true` untuk berkas baru yang belum punya tempat di disk.
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
    /// Isi dimuat (atau dimuat ulang) dari disk.
    Loaded,
    /// Isi disunting.
    Edited,
    Saved,
}

pub struct Buffer {
    path: String,
    env: Env,
    status: BufferStatus,
    states: Vec<(WeakEntity<EditorState>, Vec<Subscription>)>,
    /// Isi dari disk yang menunggu `EditorState` pertama.
    initial: Option<String>,
    /// Isi saat terakhir dimuat atau disimpan; pembanding untuk tanda "belum disimpan".
    saved: Rope,
    dirty: bool,
    encoding: String,
    bom: bool,
    eol: Eol,
    mtime_ms: f64,
    size: u32,
    read_only: bool,
    /// Berkas berubah di disk sementara buffer berisi perubahan yang belum disimpan.
    conflict: bool,
    /// Bertambah setiap kali isi dimuat ulang dari disk.
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
            // Berkas baru: langsung siap, kosong, tanpa membaca disk.
            this.status = BufferStatus::Ready;
            this.initial = Some(String::new());
            this.revision = 1;
        } else {
            this.load(window, cx);
        }
        this
    }

    /// Berkas baru yang belum pernah disimpan.
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

    /// Berkasnya diganti nama; hanya dipanggil [`Buffers::rename`] agar registry tetap selaras.
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

    /// `EditorState` pertama yang masih hidup; isinya adalah isi buffer.
    pub fn primary(&self) -> Option<Entity<EditorState>> {
        self.states.iter().find_map(|(state, _)| state.upgrade())
    }

    /// Isi terkini, atau `None` bila buffer belum siap.
    pub fn text(&self, cx: &App) -> Option<Rope> {
        if !self.is_ready() {
            return None;
        }
        match self.primary() {
            Some(state) => Some(state.read(cx).text().clone()),
            None => self.initial.as_deref().map(Rope::from),
        }
    }

    /// Menautkan `EditorState` sebuah tampilan: isinya disamakan dan suntingannya diteruskan.
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
        ];
        self.states.push((state.downgrade(), subscriptions));
    }

    /// Meneruskan suntingan dari satu state ke state lain milik buffer ini.
    fn on_edits(&mut self, source: &Entity<EditorState>, event: &InputEditsEvent, window: &mut Window, cx: &mut Context<Self>) {
        let target = source.read(cx).text().clone();
        let others: Vec<Entity<EditorState>> = self.states.iter().filter_map(|(state, _)| state.upgrade()).filter(|state| state != source).collect();
        for other in others {
            // State yang isinya sudah sama tidak disentuh; ini juga menghentikan gema, karena
            // suntingan yang diteruskan memancarkan event dari state penerimanya.
            if *other.read(cx).text() == target {
                continue;
            }
            other.update(cx, |state, cx| {
                for (edit, after) in event.edits() {
                    // State ini masih berisi dokumen sebelum suntingan, jadi posisi lama dihitung darinya.
                    let before = state.text().clone();
                    if edit.old_end_byte > before.len() {
                        break;
                    }
                    let range = lsp_types::Range { start: before.offset_to_position(edit.start_byte), end: before.offset_to_position(edit.old_end_byte) };
                    let new_text = after.slice(edit.start_byte..edit.new_end_byte).to_string();
                    state.apply_lsp_edits(&vec![lsp_types::TextEdit { range, new_text }], window, cx);
                }
                // Jaring pengaman: bila penerusan per suntingan meleset, samakan seluruh isi.
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
            // Membandingkan seluruh isi hanya bila panjangnya sama dengan isi tersimpan.
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
        // Berkas baru tidak disimpan otomatis: menyimpannya berarti memilih tempat.
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
                    // `set_value` tidak memancarkan event, jadi tidak diteruskan sebagai suntingan.
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

    /// Memuat ulang dari disk, membuang perubahan yang belum disimpan.
    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_untitled() {
            return;
        }
        self.load(window, cx);
    }

    /// Menyimpan ke disk dengan encoding dan akhir baris aslinya. Hasil `false` bila gagal.
    /// Berkas baru lebih dulu menanyakan tempat penyimpanannya.
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
                    // Suntingan bisa terjadi selagi menyimpan.
                    this.dirty = this.primary().is_some_and(|primary| *primary.read(cx).text() != this.saved);
                    cx.emit(BufferEvent::Saved);
                    cx.notify();
                    let saved = WorkbenchEvent::FileSaved(this.path.clone());
                    if let Some(workbench) = this.env.workbench.upgrade() {
                        // Ditunda: penyimpanan bisa dipicu dari dalam pembaruan `Workbench`.
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

    /// "Save As" untuk berkas baru: pilih path di dalam folder proyek, simpan, lalu buffer dan
    /// tabnya pindah ke path itu (bahasa dan highlight mengikuti nama berkas barunya).
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

    /// Memindahkan buffer berkas baru dan tabnya ke path tempat ia disimpan.
    fn move_to_saved_path(&mut self, from: String, to: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workbench) = self.env.workbench.upgrade() else { return };
        // Buffer dan state editornya dipegang sampai tab baru menautkan diri ke buffer ini.
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

    /// Menanggapi perubahan berkas dari luar: buffer bersih dimuat ulang, buffer kotor ditandai konflik.
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

/// Posisi yang harus ditampilkan saat sebuah berkas dibuka.
#[derive(Debug, Clone, PartialEq)]
pub struct Reveal {
    /// Nomor baris mulai dari 1.
    pub line: u32,
    /// Kolom mulai dari 0, dalam karakter.
    pub column: u32,
    /// Panjang pilihan dalam karakter (mis. kecocokan pencarian).
    pub length: Option<u32>,
}

#[derive(Debug, Clone)]
pub enum BuffersEvent {
    /// Buffer baru dibuat (belum tentu selesai dimuat).
    Opened(Entity<Buffer>),
    /// Sebuah tampilan membuat `EditorState` untuk buffer ini (tempat LSP memasang penyedianya).
    EditorCreated { buffer: Entity<Buffer>, state: Entity<EditorState> },
    /// Ada posisi yang menunggu untuk path ini; diambil dengan [`Buffers::take_reveal`].
    Reveal(String),
    /// Isi hint ujung baris berubah; tampilan editor menggambar ulang hint-nya.
    LineHintsChanged,
}

/// Penyedia teks samar di ujung baris kursor (mis. inline blame), seperti dekorasi "after"
/// di VS Code. Menerima path berkas dan baris (mulai dari 0); dipanggil saat render, jadi
/// harus murah dan tidak boleh memperbarui entitas.
pub type LineHintProvider = Rc<dyn Fn(&str, u32, &App) -> Option<SharedString>>;

/// Registry buffer milik satu jendela. Buffer dilepas saat tidak ada lagi tab yang memegangnya.
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

    /// Memasang penyedia hint ujung baris; id yang sama menggantikan yang lama.
    pub fn set_line_hint_provider(&mut self, id: impl Into<SharedString>, provider: LineHintProvider, cx: &mut Context<Self>) {
        let id = id.into();
        self.line_hints.retain(|(existing, _)| *existing != id);
        self.line_hints.push((id, provider));
        cx.emit(BuffersEvent::LineHintsChanged);
    }

    /// Hint untuk sebuah baris (mulai dari 0): dari penyedia pertama yang punya isi.
    pub fn line_hint(&self, path: &str, row: u32, cx: &App) -> Option<SharedString> {
        self.line_hints.iter().find_map(|(_, provider)| provider(path, row, cx))
    }

    /// Dipanggil penyedia saat isi hint-nya berubah.
    pub fn refresh_line_hints(&mut self, cx: &mut Context<Self>) {
        cx.emit(BuffersEvent::LineHintsChanged);
    }

    /// Path untuk berkas baru berikutnya: `untitled:Untitled-N` dengan N terkecil yang belum terbuka.
    pub fn next_untitled_path(&self) -> String {
        (1..)
            .map(|n| format!("{UNTITLED_PREFIX}Untitled-{n}"))
            .find(|path| self.get(path).is_none())
            .expect("selalu ada nomor yang belum dipakai")
    }

    pub fn get(&self, path: &str) -> Option<Entity<Buffer>> {
        self.buffers.get(path)?.upgrade()
    }

    pub fn open(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) -> Entity<Buffer> {
        if let Some(existing) = self.get(path) {
            return existing;
        }
        self.buffers.retain(|_, buffer| buffer.upgrade().is_some());
        let env = self.env.clone();
        let path_owned = path.to_string();
        let buffer = cx.new(|cx| Buffer::open(path_owned, env, window, cx));
        self.buffers.insert(path.to_string(), buffer.downgrade());
        cx.emit(BuffersEvent::Opened(buffer.clone()));
        buffer
    }

    /// Berkas `from` diganti nama menjadi `to`: buffernya ikut pindah beserta isi yang belum
    /// disimpan. Hasil `false` bila tidak ada buffer untuk `from`. Tab-nya ditulis ulang pemanggil
    /// (`Workbench::update_tabs`); tampilan baru memakai buffer ini selama pemanggil memegangnya.
    pub fn rename(&mut self, from: &str, to: &str, cx: &mut Context<Self>) -> bool {
        let Some(buffer) = self.get(from) else { return false };
        self.buffers.remove(from);
        buffer.update(cx, |buffer, cx| buffer.set_path(to.to_string(), cx));
        self.buffers.insert(to.to_string(), buffer.downgrade());
        if let Some(reveal) = self.reveals.remove(from) {
            self.reveals.insert(to.to_string(), reveal);
        }
        true
    }

    /// Semua buffer yang masih dipakai.
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
        self.reveals.insert(path.to_string(), reveal);
        cx.emit(BuffersEvent::Reveal(path.to_string()));
    }

    pub fn take_reveal(&mut self, path: &str) -> Option<Reveal> {
        self.reveals.remove(path)
    }
}
