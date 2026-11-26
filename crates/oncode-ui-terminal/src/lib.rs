//! Integrated terminal: the bottom `terminal` panel, the status bar button, and its commands.
//! Emulation is done by `alacritty_terminal`; the rendering is custom-drawn (see `view.rs`).

pub mod emulator;
pub mod flow;
pub mod font;
pub mod glyphs;
pub mod grid;
pub mod keys;
pub mod layout;
pub mod links;
pub mod mouse;
pub mod paint;
pub mod palette;
pub mod panel;
pub mod view;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{h_flex, Icon, Sizable as _};
use gpui_kit::{AnyElement, App, AppContext as _, Context, Entity, IntoElement, KeyBinding, NoAction, ParentElement as _, Render, Styled as _, Window};
use oncode_ui::commands::Command;
use oncode_ui::{Lucide, OpenOptions, PanelView, StatusAlign, StatusItem, Workbench};
use serde_json::{json, Value};

pub use layout::{Layout, PanelDrop, SessionId};
pub use panel::{fresh_session_id, panel, session_tab, tab_session, TerminalPanel, TAB_KIND};
pub use view::{SessionEvent, TerminalSession};

/// Id of the terminal bottom panel.
pub const PANEL_ID: &str = "terminal";

/// Registers this feature. Called once from `app/src/main.rs` after `oncode_ui::init`.
pub fn init(cx: &mut App) {
    // The gpui-kit Root binds Tab/Shift+Tab to focus movement, and action bindings run before the
    // `keydown` handler. Inside the terminal both keys belong to the shell (completion,
    // Shift+Tab goes backwards), so the Root binding is disabled for the terminal context.
    cx.bind_keys([KeyBinding::new("tab", NoAction, Some(view::KEY_CONTEXT)), KeyBinding::new("shift-tab", NoAction, Some(view::KEY_CONTEXT))]);
    oncode_ui::register_tab_kind(cx, TAB_KIND, panel::TerminalKind);
    oncode_ui::register_feature(cx, setup);
}

/// Command argument from a button in the panel bar: the target is the panel's active terminal, not
/// the terminal that was focused last (which may well be in the editor area).
fn panel_only(args: &Value) -> bool {
    args.get("panel").and_then(Value::as_bool).unwrap_or(false)
}

fn setup(workbench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let env = workbench.env().clone();
    // The shell runs in the project folder; without a folder there is no terminal (same as v0.2).
    if env.root.is_none() {
        return;
    }
    let terminal = cx.new(|cx| TerminalPanel::new(env.clone(), window, cx));
    workbench.set_extension(terminal.clone());
    workbench.on_before_build_tabs({
        let weak = terminal.downgrade();
        move |cx| cx.set_global(panel::CurrentPanel(weak.clone()))
    });
    workbench.register_panel(
        PanelView::new(PANEL_ID, "Terminal", 10, {
            let terminal = terminal.clone();
            move |_, _, _| terminal.clone().into()
        })
        .actions(panel_actions),
    );
    workbench.register_status(StatusItem::new("terminal.toggle", StatusAlign::Left, 30, |_, _, cx| cx.new(|_| TerminalButton).into()), window, cx);

    type Handler = fn(&mut Workbench, Entity<TerminalPanel>, &Value, &mut Window, &mut Context<Workbench>);
    let with_panel = |f: Handler| {
        let weak = cx.weak_entity();
        move |args: &Value, window: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |workbench, cx| {
                if let Some(terminal) = panel(workbench) {
                    f(workbench, terminal, args, window, cx);
                }
            });
        }
    };
    let has_session = {
        let terminal = terminal.clone();
        move |_: &Window, cx: &App| terminal.read(cx).has_sessions()
    };
    let in_panel = {
        let terminal = terminal.clone();
        move |_: &Window, cx: &App| !terminal.read(cx).layout().is_empty()
    };
    let in_editor = {
        let terminal = terminal.clone();
        move |_: &Window, cx: &App| !terminal.read(cx).editor_sessions().is_empty()
    };
    let terminal_focused = {
        let terminal = terminal.clone();
        move |window: &Window, cx: &App| terminal.read(cx).focused(window, cx).is_some()
    };

    workbench.register_command(Command::new("terminal.toggle", "Toggle Terminal", with_panel(toggle)).category("Terminal").key("Mod+`"));
    workbench.register_command(
        Command::new("terminal.new", "New Terminal", with_panel(|workbench, terminal, _, window, cx| {
            workbench.show_panel(PANEL_ID, cx);
            terminal.update(cx, |terminal, cx| terminal.new_session(window, cx));
        }))
        .category("Terminal")
        .key("Mod+Shift+`"),
    );
    workbench.register_command(
        Command::new("terminal.split", "Split Terminal", with_panel(split))
            .category("Terminal")
            // Like VS Code: Cmd+\ on macOS (overrides "Split Editor" while the terminal is focused),
            // Ctrl+Shift+5 elsewhere (Ctrl+\ belongs to the shell).
            .key(if cfg!(target_os = "macos") { "Mod+\\" } else { "Ctrl+Shift+5" })
            .keys_when(terminal_focused),
    );
    workbench.register_command(
        Command::new("terminal.newInEditor", "Create New Terminal in Editor Area", with_panel(|workbench, _, _, window, cx| {
            workbench.open_tab(session_tab(fresh_session_id(), "Terminal"), OpenOptions::default(), window, cx);
        }))
        .category("Terminal"),
    );
    workbench.register_command(
        Command::new("terminal.moveToEditor", "Move Terminal into Editor Area", with_panel(|_, terminal, args, window, cx| {
            terminal.update(cx, |terminal, cx| {
                // The panel's active terminal; from the palette, the focused one if it is in the panel.
                let id = terminal.focused(window, cx).filter(|id| !panel_only(args) && terminal.layout().contains(*id)).or(terminal.layout().active);
                if let Some(id) = id {
                    terminal.move_to_editor(id, window, cx);
                }
            });
        }))
        .category("Terminal")
        .when(in_panel.clone()),
    );
    workbench.register_command(
        Command::new("terminal.moveToPanel", "Move Terminal into Panel", with_panel(|workbench, terminal, _, window, cx| {
            // The focused editor-area terminal, the one focused last, or the active tab if it is a terminal.
            let active_tab = workbench.model().active().and_then(|(_, tab)| tab_session(tab));
            let target = {
                let terminal = terminal.read(cx);
                terminal.target(false, window, cx).filter(|id| terminal.is_in_editor(*id)).or(active_tab.filter(|id| terminal.is_in_editor(*id)))
            };
            let Some(id) = target else { return };
            workbench.show_panel(PANEL_ID, cx);
            terminal.update(cx, |terminal, cx| terminal.move_to_panel(id, PanelDrop::Alone(None), window, cx));
        }))
        .category("Terminal")
        .when(in_editor),
    );
    workbench.register_command(
        Command::new("terminal.unsplit", "Unsplit Terminal", with_panel(|_, terminal, _, window, cx| {
            terminal.update(cx, |terminal, cx| {
                if let Some(id) = terminal.layout().active {
                    terminal.unsplit(id, window, cx);
                }
            });
        }))
        .category("Terminal")
        .when({
            let terminal = terminal.clone();
            move |_, cx| terminal.read(cx).layout().active_group().len() > 1
        }),
    );
    workbench.register_command(
        Command::new("terminal.focus", "Focus Terminal", with_panel(|workbench, terminal, _, window, cx| {
            workbench.show_panel(PANEL_ID, cx);
            terminal.update(cx, |terminal, cx| terminal.ensure_session(window, cx));
        }))
        .category("Terminal"),
    );
    workbench.register_command(
        Command::new("terminal.kill", "Kill Terminal", with_panel(|_, terminal, args, window, cx| terminal.update(cx, |terminal, cx| terminal.kill_active(panel_only(args), window, cx))))
            .category("Terminal")
            .when(has_session.clone()),
    );
    workbench.register_command(
        Command::new("terminal.clear", "Clear Terminal", with_panel(|_, terminal, args, window, cx| terminal.update(cx, |terminal, cx| terminal.clear_active(panel_only(args), window, cx))))
            .category("Terminal")
            .when(has_session),
    );
    workbench.register_command(
        Command::new("terminal.focusNext", "Focus Next Terminal", with_panel(|workbench, terminal, _, window, cx| {
            workbench.show_panel(PANEL_ID, cx);
            terminal.update(cx, |terminal, cx| terminal.focus_next(window, cx));
        }))
        .category("Terminal")
        .when(in_panel),
    );
}

/// Shows the terminal (creating the first session if there is none), or hides its panel if the
/// terminal is already visible and focused.
fn toggle(workbench: &mut Workbench, terminal: Entity<TerminalPanel>, _: &Value, window: &mut Window, cx: &mut Context<Workbench>) {
    let showing = workbench.active_panel().is_some_and(|id| id.as_ref() == PANEL_ID);
    let focused = terminal.read(cx).visible_sessions().iter().any(|session| session.read(cx).focus_handle_is_focused(window));
    if showing && focused {
        workbench.hide_panel(window, cx);
        return;
    }
    workbench.show_panel(PANEL_ID, cx);
    terminal.update(cx, |terminal, cx| terminal.ensure_session(window, cx));
}

/// Splits the focused (or active) terminal: in the panel, a new terminal beside it; in the editor
/// area, a new terminal tab in the adjacent group.
fn split(workbench: &mut Workbench, terminal: Entity<TerminalPanel>, args: &Value, window: &mut Window, cx: &mut Context<Workbench>) {
    let target = terminal.read(cx).target(panel_only(args), window, cx);
    if let Some(id) = target.filter(|id| terminal.read(cx).is_in_editor(*id)) {
        let tab_id = session_tab(id, "").id;
        if let Some(group) = workbench.model().find_tab(&tab_id) {
            workbench.activate_tab(group, &tab_id, window, cx);
        }
        workbench.open_tab(session_tab(fresh_session_id(), "Terminal"), OpenOptions::to_side(), window, cx);
        return;
    }
    workbench.show_panel(PANEL_ID, cx);
    terminal.update(cx, |terminal, cx| terminal.split(target, window, cx));
}

/// Buttons in the panel's tab bar: new terminal, split, move to the editor area, and kill.
fn panel_actions(_: &mut Window, _: &mut App) -> AnyElement {
    let button = |id: &'static str, icon: Lucide, tooltip: &'static str, command: &'static str| {
        Button::new(id).ghost().xsmall().icon(Icon::new(icon)).tooltip(tooltip).on_click(move |_, window, cx| {
            oncode_ui::execute(command, json!({ "panel": true }), window, cx);
        })
    };
    h_flex()
        .gap_0p5()
        .child(button("terminal-new", Lucide::Plus, "New Terminal", "terminal.new"))
        .child(button("terminal-split", Lucide::Columns2, "Split Terminal", "terminal.split"))
        .child(button("terminal-move-to-editor", Lucide::SquareArrowOutUpRight, "Move Terminal into Editor Area", "terminal.moveToEditor"))
        .child(button("terminal-kill", Lucide::Trash, "Kill Terminal", "terminal.kill"))
        .into_any_element()
}

/// The "Terminal" button in the status bar.
struct TerminalButton;

impl Render for TerminalButton {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Button::new("status-terminal").ghost().xsmall().icon(Icon::new(Lucide::SquareTerminal)).label("Terminal").tooltip("Toggle Terminal").on_click(|_, window, cx| {
            oncode_ui::execute("terminal.toggle", Value::Null, window, cx);
        })
    }
}
