//! The per-window command registry. Every action is registered here so it can be run from the
//! command palette, menus, and keyboard.
//!
//! A command handler receives `(&Value, &mut Window, &mut App)` and is **not** called while the
//! `Workbench` entity is borrowed, so it may update any entity.

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
    /// Default shortcuts in canonical form (see [`keys::canonical_keybinding`]).
    pub keybindings: Vec<String>,
    /// The command is only active (and only captures shortcuts) when this returns true.
    pub when: Option<WhenFn>,
    /// An extra condition specific to keyboard shortcuts, e.g. "the editor is focused". Menus and the
    /// command palette only use `when`, so the command can still be picked from there.
    pub keys_when: Option<WhenFn>,
    /// Not shown in the command palette.
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

    /// Adds a default shortcut, e.g. `"Mod+Shift+P"`; `Mod` = Ctrl on Linux/Windows, Cmd on macOS.
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

    /// This command's keyboard shortcuts only apply if `when` also returns true.
    pub fn keys_when(mut self, when: impl Fn(&Window, &App) -> bool + 'static) -> Self {
        self.keys_when = Some(Rc::new(when));
        self
    }

    pub fn hidden(mut self) -> Self {
        self.hidden = true;
        self
    }

    /// The full title for the command palette, e.g. "Git: Commit".
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

    /// All commands in registration order.
    pub fn all(&self) -> Vec<Command> {
        self.order.iter().filter_map(|id| self.commands.get(id).cloned()).collect()
    }
}

/// Shortcut overrides from `keybindings.json`; applies to all windows.
#[derive(Default)]
struct Overrides(BTreeMap<String, Vec<String>>);

impl Global for Overrides {}

#[derive(Default)]
struct Registries(HashMap<WindowId, Rc<RefCell<CommandRegistry>>>);

impl Global for Registries {}

/// A shortcut filter: called before a command is run via a keyboard shortcut.
/// Returning `true` holds that command back, so the key is passed on to the focused element.
/// Used by the terminal so Ctrl+W, Ctrl+S, and the like reach the shell.
pub type KeyShield = Rc<dyn Fn(&KeyPress, &Command, &Window, &App) -> bool>;

#[derive(Default)]
struct Shields(Vec<KeyShield>);

impl Global for Shields {}

/// Registers a shortcut filter (see [`KeyShield`]); applies to all windows.
pub fn register_key_shield(cx: &mut App, shield: impl Fn(&KeyPress, &Command, &Window, &App) -> bool + 'static) {
    cx.default_global::<Shields>().0.push(Rc::new(shield));
}

pub(crate) fn install(cx: &mut App) {
    cx.set_global(Overrides::default());
    cx.set_global(Registries::default());
    // Runs before any other key mechanism, like a capture-phase `keydown` handler in the web version.
    cx.intercept_keystrokes(|event, window, cx| {
        let keystroke = &event.keystroke;
        let press = KeyPress {
            control: keystroke.modifiers.control,
            alt: keystroke.modifiers.alt,
            shift: keystroke.modifiers.shift,
            platform: keystroke.modifiers.platform,
            key: keystroke.key.clone(),
        };
        let Some(binding) = keys::keybinding_from_press(&press) else { return };
        // The focused element (e.g. the terminal) may hold a key back for itself.
        let (command, bound) = match_keybinding(&binding, window, cx);
        let command = command.filter(|command| {
            if key_captured(&press, window, cx) {
                return false;
            }
            let shields = cx.try_global::<Shields>().map(|shields| shields.0.clone()).unwrap_or_default();
            !shields.iter().any(|shield| shield(&press, command, window, cx))
        });
        let Some(command) = command else {
            if bound {
                decline(binding, cx);
            }
            return;
        };
        cx.stop_propagation();
        (command.run)(&Value::Null, window, cx);
    })
    .detach();
}

/// A shortcut that the keyboard just declined even though it is bound to a command (e.g. `keys_when`
/// is not satisfied, or the terminal held the key). On macOS, AppKit then offers the same key to the
/// menu bar; the menu checks this via [`declined_by_keyboard`] so the keyboard's decision still
/// applies. Cleared as soon as the event being processed finishes.
#[derive(Default)]
struct Declined {
    binding: Option<String>,
    serial: u64,
}

impl Global for Declined {}

fn decline(binding: String, cx: &mut App) {
    let declined = cx.default_global::<Declined>();
    declined.serial += 1;
    declined.binding = Some(binding);
    let serial = declined.serial;
    // A foreground task only runs after this event has finished processing, including by the menu bar.
    cx.spawn(async move |cx| {
        cx.update(|cx| {
            let declined = cx.default_global::<Declined>();
            if declined.serial == serial {
                declined.binding = None;
            }
        })
    })
    .detach();
}

/// Whether one of `command`'s shortcuts was just declined by the keyboard (see [`Declined`]).
pub fn declined_by_keyboard(command: &Command, cx: &App) -> bool {
    let Some(binding) = cx.try_global::<Declined>().and_then(|declined| declined.binding.as_ref()) else { return false };
    keybindings_for(command, cx).iter().any(|b| b == binding)
}

/// A key capture: if the element with this `FocusHandle` is focused and its function returns `true`,
/// the application shortcut is not run and the key is passed on to that element.
pub type CaptureFn = Rc<dyn Fn(&KeyPress, &Window, &App) -> bool>;

#[derive(Default)]
struct Captures(Vec<(WeakFocusHandle, CaptureFn)>);

impl Global for Captures {}

/// Registers a key capture for an element (released automatically when its `FocusHandle` is gone).
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

/// All commands of this window.
pub fn all(window: &Window, cx: &App) -> Vec<Command> {
    registry_for(window, cx).map(|r| r.borrow().all()).unwrap_or_default()
}

pub fn get(id: &str, window: &Window, cx: &App) -> Option<Command> {
    registry_for(window, cx).and_then(|r| r.borrow().get(id).cloned())
}

/// The shortcuts that apply to a command (user overrides take precedence).
pub fn keybindings_for(command: &Command, cx: &App) -> Vec<String> {
    match cx.try_global::<Overrides>().and_then(|o| o.0.get(command.id.as_ref())) {
        Some(custom) => custom.clone(),
        None => command.keybindings.clone(),
    }
}

/// Applies the content of `keybindings.json`. An empty list removes the default shortcuts.
pub fn set_keybinding_overrides(overrides: BTreeMap<String, Vec<String>>, cx: &mut App) {
    cx.set_global(Overrides(overrides));
    cx.refresh_windows();
}

/// The active command bound to this shortcut; the one registered last wins.
pub fn command_for_keybinding(binding: &str, window: &Window, cx: &App) -> Option<Command> {
    match_keybinding(binding, window, cx).0
}

/// Like [`command_for_keybinding`], plus whether this shortcut is bound to any command
/// (including inactive ones).
fn match_keybinding(binding: &str, window: &Window, cx: &App) -> (Option<Command>, bool) {
    let (mut found, mut bound) = (None, false);
    for command in all(window, cx) {
        if !keybindings_for(&command, cx).iter().any(|b| b == binding) {
            continue;
        }
        bound = true;
        if command.when.as_ref().is_some_and(|when| !when(window, cx)) || command.keys_when.as_ref().is_some_and(|when| !when(window, cx)) {
            continue;
        }
        found = Some(command);
    }
    (found, bound)
}

/// Runs a command right now. Do not call it while the `Workbench` entity is borrowed
/// (e.g. from inside `workbench.update(..)`); use [`execute`] for that.
pub fn execute_now(id: &str, args: &Value, window: &mut Window, cx: &mut App) -> bool {
    let Some(command) = get(id, window, cx) else { return false };
    if command.when.as_ref().is_some_and(|when| !when(window, cx)) {
        return false;
    }
    (command.run)(args, window, cx);
    true
}

/// Schedules a command after the update in progress finishes. Safe to call from anywhere.
pub fn execute(id: impl Into<SharedString>, args: Value, window: &mut Window, cx: &mut App) {
    let id = id.into();
    window.defer(cx, move |window, cx| {
        execute_now(&id, &args, window, cx);
    });
}
