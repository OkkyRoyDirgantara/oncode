//! Registry command per jendela. Setiap aksi terdaftar di sini agar bisa dijalankan dari
//! command palette, menu, dan keyboard.
//!
//! Penangan command menerima `(&Value, &mut Window, &mut App)` dan **tidak** dipanggil selagi
//! entitas `Workbench` dipinjam, sehingga boleh memperbarui entitas mana pun.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use gpui_kit::{App, FocusHandle, Global, SharedString, WeakFocusHandle, Window, WindowId};
use serde_json::Value;

use crate::keys::{self, KeyPress};

pub type RunFn = Rc<dyn Fn(&Value, &mut Window, &mut App)>;
pub type WhenFn = Rc<dyn Fn(&Window, &App) -> bool>;

#[derive(Clone)]
pub struct Command {
    pub id: SharedString,
    pub title: SharedString,
    pub category: Option<SharedString>,
    /// Pintasan bawaan dalam bentuk kanonis (lihat [`keys::canonical_keybinding`]).
    pub keybindings: Vec<String>,
    /// Command hanya aktif (dan hanya menangkap pintasan) bila ini mengembalikan true.
    pub when: Option<WhenFn>,
    /// Syarat tambahan khusus pintasan keyboard, mis. "editor sedang fokus". Menu dan command
    /// palette hanya memakai `when`, sehingga command tetap bisa dipilih dari sana.
    pub keys_when: Option<WhenFn>,
    /// Tidak ditampilkan di command palette.
    pub hidden: bool,
    pub run: RunFn,
}

impl Command {
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>, run: impl Fn(&Value, &mut Window, &mut App) + 'static) -> Self {
        Self { id: id.into(), title: title.into(), category: None, keybindings: Vec::new(), when: None, keys_when: None, hidden: false, run: Rc::new(run) }
    }

    pub fn category(mut self, category: impl Into<SharedString>) -> Self {
        self.category = Some(category.into());
        self
    }

    /// Menambah pintasan bawaan, mis. `"Mod+Shift+P"`; `Mod` = Ctrl di Linux/Windows, Cmd di macOS.
    pub fn key(mut self, binding: &str) -> Self {
        let canonical = keys::canonical_keybinding(binding);
        if !canonical.is_empty() {
            self.keybindings.push(canonical);
        }
        self
    }

    pub fn when(mut self, when: impl Fn(&Window, &App) -> bool + 'static) -> Self {
        self.when = Some(Rc::new(when));
        self
    }

    /// Pintasan keyboard command ini hanya berlaku bila `when` juga mengembalikan true.
    pub fn keys_when(mut self, when: impl Fn(&Window, &App) -> bool + 'static) -> Self {
        self.keys_when = Some(Rc::new(when));
        self
    }

    pub fn hidden(mut self) -> Self {
        self.hidden = true;
        self
    }

    /// Judul lengkap untuk command palette, mis. "Git: Commit".
    pub fn label(&self) -> String {
        match &self.category {
            Some(category) => format!("{category}: {}", self.title),
            None => self.title.to_string(),
        }
    }
}

#[derive(Default)]
pub struct CommandRegistry {
    commands: BTreeMap<SharedString, Command>,
    order: Vec<SharedString>,
}

impl CommandRegistry {
    pub fn register(&mut self, command: Command) {
        if !self.commands.contains_key(&command.id) {
            self.order.push(command.id.clone());
        }
        self.commands.insert(command.id.clone(), command);
    }

    pub fn get(&self, id: &str) -> Option<&Command> {
        self.commands.get(id)
    }

    /// Semua command menurut urutan pendaftaran.
    pub fn all(&self) -> Vec<Command> {
        self.order.iter().filter_map(|id| self.commands.get(id).cloned()).collect()
    }
}

/// Penimpaan pintasan dari `keybindings.json`; berlaku untuk semua jendela.
#[derive(Default)]
struct Overrides(BTreeMap<String, Vec<String>>);

impl Global for Overrides {}

#[derive(Default)]
struct Registries(HashMap<WindowId, Rc<RefCell<CommandRegistry>>>);

impl Global for Registries {}

/// Penyaring pintasan: dipanggil sebelum sebuah command dijalankan lewat pintasan keyboard.
/// Hasil `true` menahan command itu, sehingga tombolnya diteruskan ke elemen yang sedang fokus.
/// Dipakai terminal agar Ctrl+W, Ctrl+S, dan sejenisnya sampai ke shell.
pub type KeyShield = Rc<dyn Fn(&KeyPress, &Command, &Window, &App) -> bool>;

#[derive(Default)]
struct Shields(Vec<KeyShield>);

impl Global for Shields {}

/// Mendaftarkan penyaring pintasan (lihat [`KeyShield`]); berlaku untuk semua jendela.
pub fn register_key_shield(cx: &mut App, shield: impl Fn(&KeyPress, &Command, &Window, &App) -> bool + 'static) {
    cx.default_global::<Shields>().0.push(Rc::new(shield));
}

pub(crate) fn install(cx: &mut App) {
    cx.set_global(Overrides::default());
    cx.set_global(Registries::default());
    // Dijalankan sebelum mekanisme tombol lain, seperti penangan `keydown` fase capture di versi web.
    cx.intercept_keystrokes(|event, window, cx| {
        let keystroke = &event.keystroke;
        let press = KeyPress {
            control: keystroke.modifiers.control,
            alt: keystroke.modifiers.alt,
            shift: keystroke.modifiers.shift,
            platform: keystroke.modifiers.platform,
            key: keystroke.key.clone(),
        };
        // Elemen yang fokus (mis. terminal) boleh menahan tombol untuk dirinya sendiri.
        if key_captured(&press, window, cx) {
            return;
        }
        let Some(binding) = keys::keybinding_from_press(&press) else { return };
        let Some(command) = command_for_keybinding(&binding, window, cx) else { return };
        let shields = cx.try_global::<Shields>().map(|shields| shields.0.clone()).unwrap_or_default();
        if shields.iter().any(|shield| shield(&press, &command, window, cx)) {
            return;
        }
        cx.stop_propagation();
        (command.run)(&Value::Null, window, cx);
    })
    .detach();
}

/// Penahan tombol: bila elemen dengan `FocusHandle` ini fokus dan fungsinya mengembalikan `true`,
/// pintasan aplikasi tidak dijalankan dan tombolnya diteruskan ke elemen itu.
pub type CaptureFn = Rc<dyn Fn(&KeyPress, &Window, &App) -> bool>;

#[derive(Default)]
struct Captures(Vec<(WeakFocusHandle, CaptureFn)>);

impl Global for Captures {}

/// Mendaftarkan penahan tombol untuk sebuah elemen (dilepas otomatis saat `FocusHandle`-nya hilang).
pub fn capture_keys(handle: &FocusHandle, capture: impl Fn(&KeyPress, &Window, &App) -> bool + 'static, cx: &mut App) {
    let captures = &mut cx.default_global::<Captures>().0;
    captures.retain(|(handle, _)| handle.upgrade().is_some());
    captures.push((handle.downgrade(), Rc::new(capture)));
}

fn key_captured(press: &KeyPress, window: &Window, cx: &App) -> bool {
    let Some(captures) = cx.try_global::<Captures>() else { return false };
    captures.0.iter().any(|(handle, capture)| handle.upgrade().is_some_and(|handle| handle.is_focused(window)) && capture(press, window, cx))
}

pub(crate) fn attach(window: &Window, registry: Rc<RefCell<CommandRegistry>>, cx: &mut App) {
    cx.global_mut::<Registries>().0.insert(window.window_handle().window_id(), registry);
}

pub(crate) fn detach(window_id: WindowId, registry: &Rc<RefCell<CommandRegistry>>, cx: &mut App) {
    let registries = &mut cx.global_mut::<Registries>().0;
    if registries.get(&window_id).is_some_and(|current| Rc::ptr_eq(current, registry)) {
        registries.remove(&window_id);
    }
}

fn registry_for(window: &Window, cx: &App) -> Option<Rc<RefCell<CommandRegistry>>> {
    cx.try_global::<Registries>()?.0.get(&window.window_handle().window_id()).cloned()
}

/// Semua command jendela ini.
pub fn all(window: &Window, cx: &App) -> Vec<Command> {
    registry_for(window, cx).map(|r| r.borrow().all()).unwrap_or_default()
}

pub fn get(id: &str, window: &Window, cx: &App) -> Option<Command> {
    registry_for(window, cx).and_then(|r| r.borrow().get(id).cloned())
}

/// Pintasan yang berlaku untuk sebuah command (penimpaan pengguna didahulukan).
pub fn keybindings_for(command: &Command, cx: &App) -> Vec<String> {
    match cx.try_global::<Overrides>().and_then(|o| o.0.get(command.id.as_ref())) {
        Some(custom) => custom.clone(),
        None => command.keybindings.clone(),
    }
}

/// Menerapkan isi `keybindings.json`. Daftar kosong menghapus pintasan bawaan.
pub fn set_keybinding_overrides(overrides: BTreeMap<String, Vec<String>>, cx: &mut App) {
    cx.set_global(Overrides(overrides));
    cx.refresh_windows();
}

/// Command aktif yang terikat pada pintasan ini; yang didaftarkan terakhir menang.
pub fn command_for_keybinding(binding: &str, window: &Window, cx: &App) -> Option<Command> {
    let mut found = None;
    for command in all(window, cx) {
        if !keybindings_for(&command, cx).iter().any(|b| b == binding) {
            continue;
        }
        if command.when.as_ref().is_some_and(|when| !when(window, cx)) || command.keys_when.as_ref().is_some_and(|when| !when(window, cx)) {
            continue;
        }
        found = Some(command);
    }
    found
}

/// Menjalankan command sekarang juga. Jangan dipanggil selagi entitas `Workbench` dipinjam
/// (mis. dari dalam `workbench.update(..)`); pakai [`execute`] untuk itu.
pub fn execute_now(id: &str, args: &Value, window: &mut Window, cx: &mut App) -> bool {
    let Some(command) = get(id, window, cx) else { return false };
    if command.when.as_ref().is_some_and(|when| !when(window, cx)) {
        return false;
    }
    (command.run)(args, window, cx);
    true
}

/// Menjadwalkan command setelah pembaruan yang sedang berjalan selesai. Aman dipanggil dari mana saja.
pub fn execute(id: impl Into<SharedString>, args: Value, window: &mut Window, cx: &mut App) {
    let id = id.into();
    window.defer(cx, move |window, cx| {
        execute_now(&id, &args, window, cx);
    });
}
