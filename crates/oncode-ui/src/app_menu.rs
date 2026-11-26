//! The macOS menu bar (next to the Apple logo). Its content is built from the [`menubar::MENUS`] of the
//! currently active window, so it matches the menu in the title bar on Windows and Linux; on macOS the
//! title bar menu is not shown.
//!
//! Native menus only know GPUI [`Action`]s, while OnCode commands use text ids and their own
//! shortcut system ([`commands`]). The bridge:
//! - A usable item carries a [`MenuCommand`]; its handler runs the command in the active
//!   window. An item whose `when` is currently `false` carries a [`DisabledMenuCommand`], which
//!   deliberately has no handler, so macOS shows it disabled.
//! - macOS takes the shortcut text in menus from the GPUI keymap. Each shortcut is registered with
//!   the [`MENU_ONLY_CONTEXT`] context, which never exists on any element: it shows in the menu, but
//!   is never run by GPUI. The keyboard is still handled entirely by [`commands`].
//! - If the keyboard declines a shortcut (e.g. `keys_when` is not satisfied), AppKit offers the same
//!   key to the menu; [`commands::declined_by_keyboard`] prevents the menu from running it.
//!
//! The menu is rebuilt only when its content changes ([`sync`] is called every time the active window is drawn).

use std::collections::HashSet;

use gpui_kit::{Action, AnyWindowHandle, App, KeyBinding, KeyBindingContextPredicate, Menu, MenuItem, SharedString, SystemMenuType, Window};
use serde_json::Value;

use crate::commands;
use crate::menubar::{self, MENUS};

/// A keymap context that is never installed on any element (see the module documentation).
const MENU_ONLY_CONTEXT: &str = "OnCodeNativeMenuOnly";

/// Commands that move to the application menu (the "OnCode" menu) on macOS, instead of another menu.
const APP_MENU_COMMANDS: &[&str] = &["settings.open"];

/// Runs command `id` in the active window. `key` is the shortcut shown by the menu; it is also
/// part of the action so that an old shortcut from before `keybindings.json` was changed is not used.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = oncode, no_json, no_register)]
pub struct MenuCommand {
    pub id: SharedString,
    pub key: Option<String>,
}

/// An item for a command that cannot be used right now. It has no handler (see the module documentation).
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = oncode, no_json, no_register)]
pub struct DisabledMenuCommand {
    pub id: SharedString,
    pub key: Option<String>,
}

gpui_kit::actions!(
    oncode,
    [
        /// Closes all windows (asking if there are unsaved tabs), then quits.
        Quit,
        Hide,
        HideOthers,
        ShowAll,
        /// "New Window" when no window is open.
        NewWindow,
    ]
);

/// One menu item: a command of the active window, or [`NewWindow`] when there is no window.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuEntry {
    pub id: SharedString,
    pub title: SharedString,
    pub enabled: bool,
    /// The canonical shortcut shown (see [`crate::keys::canonical_keybinding`]).
    pub key: Option<String>,
}

/// The menu bar content: extra items for the application menu, then the menus from [`MENUS`]. `None` = separator.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuModel {
    pub app: Vec<MenuEntry>,
    pub menus: Vec<(SharedString, Vec<Option<MenuEntry>>)>,
    /// No window: only "New Window" is available.
    pub windowless: bool,
}

#[derive(Default)]
struct NativeMenus {
    shown: Option<MenuModel>,
    /// (id, shortcut, windowless) already registered in the keymap to be shown by the menu.
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
    // Standard macOS shortcuts. An OnCode command using the same key still takes precedence, because
    // `commands` processes keys before the GPUI keymap.
    cx.bind_keys([KeyBinding::new("cmd-q", Quit, None), KeyBinding::new("cmd-h", Hide, None), KeyBinding::new("alt-cmd-h", HideOthers, None)]);
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() {
            show(windowless(cx), cx);
        }
    })
    .detach();
}

/// The Dock icon was clicked: open a new window if none is open, like other macOS apps.
/// Installed via `Application::on_reopen`.
pub fn reopen(cx: &mut App) {
    if cx.windows().is_empty() {
        crate::open_window(None, cx);
    }
}

/// Updates the menu bar if `window` is active and its menu content changed. Cheap if nothing changed.
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

/// The menu bar content for `window`. A menu without registered commands is not shown (e.g. Git in a
/// folder without a repository), same as the menu in the title bar.
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

/// The menu bar content when there is no window: the "New Window" shortcut is taken from the last menu.
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
    // Runs after the update in progress (possibly in the middle of `render`) has finished.
    cx.defer(move |cx| {
        if !bindings.is_empty() {
            cx.bind_keys(bindings);
        }
        // The keymap is read when the menu is assembled, so the shortcuts must be registered first.
        cx.set_menus(menus);
    });
}

/// Shortcuts that are only displayed by the menu, for both the enabled and disabled versions of the same item.
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

/// A canonical shortcut (`Mod+Shift+P`) in GPUI keymap notation (`cmd-shift-p`).
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
    // macOS replaces the name of the first menu with the application name.
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
    // Settings is given the title that is customary on macOS.
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

/// Runs a command from the menu in the active window. Global action handlers run while that window
/// is borrowed, so the command is run afterwards.
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
