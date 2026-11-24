//! Menu bar macOS (di samping logo Apple). Isinya dibangun dari [`menubar::MENUS`] milik jendela
//! yang sedang aktif, jadi sama dengan menu di bilah judul Windows dan Linux; di macOS menu di
//! bilah judul tidak ditampilkan.
//!
//! Menu native hanya mengenal GPUI [`Action`], sedangkan command OnCode memakai id teks dan
//! sistem pintasannya sendiri ([`commands`]). Jembatannya:
//! - Item yang bisa dipakai membawa [`MenuCommand`]; penangannya menjalankan command di jendela
//!   aktif. Item yang `when`-nya sedang `false` membawa [`DisabledMenuCommand`], yang sengaja
//!   tidak punya penangan, sehingga macOS menampilkannya nonaktif.
//! - macOS mengambil teks pintasan di menu dari keymap GPUI. Setiap pintasan didaftarkan dengan
//!   konteks [`MENU_ONLY_CONTEXT`] yang tidak pernah ada di elemen mana pun: tampil di menu, tetapi
//!   tidak pernah dijalankan GPUI. Keyboard tetap sepenuhnya diurus [`commands`].
//! - Bila keyboard menolak sebuah pintasan (mis. `keys_when` tidak terpenuhi), AppKit menawarkan
//!   tombol yang sama ke menu; [`commands::declined_by_keyboard`] mencegah menu menjalankannya.
//!
//! Menu disusun ulang hanya bila isinya berubah ([`sync`] dipanggil setiap jendela aktif digambar).

use std::collections::HashSet;

use gpui_kit::{Action, AnyWindowHandle, App, KeyBinding, KeyBindingContextPredicate, Menu, MenuItem, SharedString, SystemMenuType, Window};
use serde_json::Value;

use crate::commands;
use crate::menubar::{self, MENUS};

/// Konteks keymap yang tidak pernah dipasang di elemen mana pun (lihat dokumentasi modul).
const MENU_ONLY_CONTEXT: &str = "OnCodeNativeMenuOnly";

/// Command yang di macOS pindah ke menu aplikasi (menu "OnCode"), bukan di menu lain.
const APP_MENU_COMMANDS: &[&str] = &["settings.open"];

/// Menjalankan command `id` di jendela aktif. `key` adalah pintasan yang ditampilkan menu; ikut
/// menjadi bagian action agar pintasan lama dari sebelum `keybindings.json` diubah tidak terpakai.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = oncode, no_json, no_register)]
pub struct MenuCommand {
    pub id: SharedString,
    pub key: Option<String>,
}

/// Item untuk command yang sedang tidak bisa dipakai. Tidak punya penangan (lihat dokumentasi modul).
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = oncode, no_json, no_register)]
pub struct DisabledMenuCommand {
    pub id: SharedString,
    pub key: Option<String>,
}

gpui_kit::actions!(
    oncode,
    [
        /// Menutup semua jendela (bertanya bila ada tab belum disimpan), lalu keluar.
        Quit,
        Hide,
        HideOthers,
        ShowAll,
        /// "New Window" saat tidak ada jendela yang terbuka.
        NewWindow,
    ]
);

/// Satu item menu: command jendela aktif, atau [`NewWindow`] saat tidak ada jendela.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuEntry {
    pub id: SharedString,
    pub title: SharedString,
    pub enabled: bool,
    /// Pintasan kanonis yang ditampilkan (lihat [`crate::keys::canonical_keybinding`]).
    pub key: Option<String>,
}

/// Isi menu bar: item tambahan untuk menu aplikasi, lalu menu-menu dari [`MENUS`]. `None` = pemisah.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuModel {
    pub app: Vec<MenuEntry>,
    pub menus: Vec<(SharedString, Vec<Option<MenuEntry>>)>,
    /// Tidak ada jendela: hanya "New Window" yang tersedia.
    pub windowless: bool,
}

#[derive(Default)]
struct NativeMenus {
    shown: Option<MenuModel>,
    /// (id, pintasan, tanpa jendela) yang sudah didaftarkan ke keymap untuk ditampilkan menu.
    bound: HashSet<(SharedString, String, bool)>,
}

impl gpui_kit::Global for NativeMenus {}

pub(crate) fn install(cx: &mut App) {
    cx.on_action(|action: &MenuCommand, cx| run(action.id.clone(), cx));
    cx.on_action(|_: &Quit, cx| crate::shell::quit(cx));
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &NewWindow, cx| crate::open_window(None, cx));
    if !cfg!(target_os = "macos") {
        return;
    }
    // Pintasan standar macOS. Command OnCode yang memakai tombol sama tetap didahulukan, karena
    // `commands` memproses tombol sebelum keymap GPUI.
    cx.bind_keys([KeyBinding::new("cmd-q", Quit, None), KeyBinding::new("cmd-h", Hide, None), KeyBinding::new("alt-cmd-h", HideOthers, None)]);
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() {
            show(windowless(cx), cx);
        }
    })
    .detach();
}

/// Ikon di Dock diklik: buka jendela baru bila tidak ada yang terbuka, seperti aplikasi macOS lain.
/// Dipasang lewat `Application::on_reopen`.
pub fn reopen(cx: &mut App) {
    if cx.windows().is_empty() {
        crate::open_window(None, cx);
    }
}

/// Memperbarui menu bar bila `window` sedang aktif dan isi menunya berubah. Murah bila tidak berubah.
pub(crate) fn sync(window: &Window, cx: &mut App) {
    if !window.is_window_active() {
        return;
    }
    let model = model(window, cx);
    if cx.try_global::<NativeMenus>().and_then(|state| state.shown.as_ref()) == Some(&model) {
        return;
    }
    show(model, cx);
}

/// Isi menu bar untuk `window`. Menu tanpa command terdaftar tidak ditampilkan (mis. Git di folder
/// tanpa repositori), sama seperti menu di bilah judul.
pub fn model(window: &Window, cx: &App) -> MenuModel {
    let entry = |command: &commands::Command| MenuEntry {
        id: command.id.clone(),
        title: command.title.clone(),
        enabled: command.when.as_ref().is_none_or(|when| when(window, cx)),
        key: commands::keybindings_for(command, cx).into_iter().next(),
    };
    let app = APP_MENU_COMMANDS.iter().filter_map(|id| commands::get(id, window, cx)).map(|command| entry(&command)).collect();
    let menus = MENUS
        .iter()
        .filter_map(|(title, items)| {
            let items: Vec<Option<&str>> = items.iter().copied().filter(|item| item.is_none_or(|id| !APP_MENU_COMMANDS.contains(&id))).collect();
            let entries: Vec<Option<MenuEntry>> = menubar::entries(&items, window, cx).iter().map(|item| item.as_ref().map(entry)).collect();
            (!entries.is_empty()).then(|| (SharedString::from(*title), entries))
        })
        .collect();
    MenuModel { app, menus, windowless: false }
}

/// Isi menu bar saat tidak ada jendela: pintasan "New Window" diambil dari menu terakhir.
fn windowless(cx: &App) -> MenuModel {
    let key = cx
        .try_global::<NativeMenus>()
        .and_then(|state| state.shown.as_ref())
        .and_then(|model| model.menus.iter().flat_map(|(_, items)| items.iter().flatten()).find(|item| item.id == "window.new"))
        .and_then(|item| item.key.clone());
    let new_window = MenuEntry { id: "window.new".into(), title: "New Window".into(), enabled: true, key };
    MenuModel { app: Vec::new(), menus: vec![("File".into(), vec![Some(new_window)])], windowless: true }
}

fn show(model: MenuModel, cx: &mut App) {
    let state = cx.default_global::<NativeMenus>();
    let mut missing = Vec::new();
    for item in model.app.iter().chain(model.menus.iter().flat_map(|(_, items)| items.iter().flatten())) {
        if let Some(key) = &item.key {
            if state.bound.insert((item.id.clone(), key.clone(), model.windowless)) {
                missing.push((item.id.clone(), key.clone(), model.windowless));
            }
        }
    }
    let menus = build(&model);
    state.shown = Some(model);
    let bindings: Vec<KeyBinding> = missing.into_iter().flat_map(|(id, key, windowless)| display_bindings(id, key, windowless, cx)).collect();
    // Dijalankan setelah pembaruan yang sedang berjalan (bisa jadi di tengah `render`) selesai.
    cx.defer(move |cx| {
        if !bindings.is_empty() {
            cx.bind_keys(bindings);
        }
        // Keymap dibaca saat menu disusun, jadi pintasan harus terdaftar lebih dulu.
        cx.set_menus(menus);
    });
}

/// Pintasan yang hanya ditampilkan menu, untuk versi aktif dan nonaktif item yang sama.
fn display_bindings(id: SharedString, key: String, windowless: bool, cx: &App) -> Vec<KeyBinding> {
    let Some(keystroke) = gpui_keystroke(&key) else { return Vec::new() };
    let Ok(context) = KeyBindingContextPredicate::parse(MENU_ONLY_CONTEXT) else { return Vec::new() };
    let context: std::rc::Rc<KeyBindingContextPredicate> = context.into();
    let actions: Vec<Box<dyn Action>> = if windowless {
        vec![Box::new(NewWindow)]
    } else {
        vec![Box::new(MenuCommand { id: id.clone(), key: Some(key.clone()) }), Box::new(DisabledMenuCommand { id, key: Some(key) })]
    };
    actions.into_iter().filter_map(|action| KeyBinding::load(&keystroke, action, Some(context.clone()), false, None, cx).ok()).collect()
}

/// Pintasan kanonis (`Mod+Shift+P`) dalam penulisan keymap GPUI (`cmd-shift-p`).
pub fn gpui_keystroke(binding: &str) -> Option<String> {
    let mut parts: Vec<&str> = binding.split('+').filter(|part| !part.is_empty()).collect();
    if binding.ends_with("++") || binding == "+" {
        parts.push("+");
    }
    let (key, modifiers) = parts.split_last()?;
    let mut out: Vec<String> = Vec::new();
    for modifier in modifiers {
        out.push(
            match *modifier {
                "Mod" if crate::keys::IS_MAC => "cmd",
                "Mod" => "ctrl",
                "Ctrl" => "ctrl",
                "Meta" => "cmd",
                "Alt" => "alt",
                "Shift" => "shift",
                _ => return None,
            }
            .to_string(),
        );
    }
    out.push(key.to_lowercase());
    Some(out.join("-"))
}

fn build(model: &MenuModel) -> Vec<Menu> {
    let mut app: Vec<MenuItem> = model.app.iter().map(|item| menu_item(item, model.windowless)).collect();
    if !app.is_empty() {
        app.push(MenuItem::separator());
    }
    app.extend([
        MenuItem::os_submenu("Services", SystemMenuType::Services),
        MenuItem::separator(),
        MenuItem::action("Hide OnCode", Hide),
        MenuItem::action("Hide Others", HideOthers),
        MenuItem::action("Show All", ShowAll),
        MenuItem::separator(),
        MenuItem::action("Quit OnCode", Quit),
    ]);
    // Nama menu pertama diganti macOS dengan nama aplikasi.
    let mut menus = vec![Menu::new("OnCode").items(app)];
    menus.extend(model.menus.iter().map(|(title, items)| {
        Menu::new(title.clone()).items(items.iter().map(|item| match item {
            Some(item) => menu_item(item, model.windowless),
            None => MenuItem::separator(),
        }))
    }));
    menus
}

fn menu_item(item: &MenuEntry, windowless: bool) -> MenuItem {
    // Pengaturan diberi judul yang lazim di macOS.
    let title = if item.id == "settings.open" { SharedString::from("Settings…") } else { item.title.clone() };
    if windowless {
        return MenuItem::action(title, NewWindow);
    }
    let (id, key) = (item.id.clone(), item.key.clone());
    if item.enabled {
        MenuItem::action(title, MenuCommand { id, key })
    } else {
        MenuItem::action(title, DisabledMenuCommand { id, key })
    }
}

/// Menjalankan command dari menu di jendela aktif. Penangan action global berjalan selagi jendela
/// itu dipinjam, jadi command dijalankan setelahnya.
fn run(id: SharedString, cx: &mut App) {
    let Some(handle) = cx.active_window() else { return };
    cx.defer(move |cx| run_in(handle, &id, cx));
}

fn run_in(handle: AnyWindowHandle, id: &str, cx: &mut App) {
    let _ = handle.update(cx, |_, window, cx| {
        let Some(command) = commands::get(id, window, cx) else { return };
        if commands::declined_by_keyboard(&command, cx) {
            return;
        }
        commands::execute_now(id, &Value::Null, window, cx);
    });
}

#[cfg(test)]
mod tests {
    use super::gpui_keystroke;

    #[test]
    fn canonical_shortcuts_become_gpui_keystrokes() {
        let primary = if crate::keys::IS_MAC { "cmd" } else { "ctrl" };
        assert_eq!(gpui_keystroke("Mod+Shift+P").as_deref(), Some(format!("{primary}-shift-p").as_str()));
        assert_eq!(gpui_keystroke("Alt+Shift+F").as_deref(), Some("alt-shift-f"));
        assert_eq!(gpui_keystroke("Ctrl+Up").as_deref(), Some("ctrl-up"));
        assert_eq!(gpui_keystroke("F12").as_deref(), Some("f12"));
        assert_eq!(gpui_keystroke("Mod+,").as_deref(), Some(format!("{primary}-,").as_str()));
        assert_eq!(gpui_keystroke("Mod++").as_deref(), Some(format!("{primary}-+").as_str()));
        assert_eq!(gpui_keystroke("Hyper+K"), None);
        assert_eq!(gpui_keystroke(""), None);
    }
}
