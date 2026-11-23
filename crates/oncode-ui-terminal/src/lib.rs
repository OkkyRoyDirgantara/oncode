//! Terminal terintegrasi: panel bawah `terminal`, tombol status bar, dan command-nya.
//! Emulasi oleh `alacritty_terminal`; tampilannya digambar sendiri (lihat `view.rs`).

pub mod emulator;
pub mod flow;
pub mod keys;
pub mod links;
pub mod mouse;
pub mod palette;
pub mod view;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Icon, Sizable as _};
use gpui_kit::{App, AppContext as _, Context, Entity, IntoElement, KeyBinding, NoAction, Render, Window};
use oncode_ui::commands::Command;
use oncode_ui::{Lucide, PanelView, StatusAlign, StatusItem, Workbench};
use serde_json::Value;

pub use view::{panel, SessionEvent, TerminalPanel, TerminalSession};

/// Id panel bawah terminal.
pub const PANEL_ID: &str = "terminal";

/// Mendaftarkan fitur ini. Dipanggil sekali dari `app/src/main.rs` setelah `oncode_ui::init`.
pub fn init(cx: &mut App) {
    // Root gpui-kit mengikat Tab/Shift+Tab ke perpindahan fokus, dan binding aksi dijalankan
    // sebelum penangan `keydown`. Di dalam terminal kedua tombol itu milik shell (pelengkapan,
    // Shift+Tab mundur), jadi binding Root dinonaktifkan untuk konteks terminal.
    cx.bind_keys([KeyBinding::new("tab", NoAction, Some(view::KEY_CONTEXT)), KeyBinding::new("shift-tab", NoAction, Some(view::KEY_CONTEXT))]);
    oncode_ui::register_feature(cx, setup);
}

fn setup(workbench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let env = workbench.env().clone();
    // Shell berjalan di folder proyek; tanpa folder tidak ada terminal (sama dengan v0.2).
    if env.root.is_none() {
        return;
    }
    let terminal = cx.new(|cx| TerminalPanel::new(env.clone(), cx));
    workbench.set_extension(terminal.clone());
    workbench.register_panel(PanelView::new(PANEL_ID, "Terminal", 10, {
        let terminal = terminal.clone();
        move |_, _, _| terminal.clone().into()
    }));
    workbench.register_status(StatusItem::new("terminal.toggle", StatusAlign::Left, 30, |_, _, cx| cx.new(|_| TerminalButton).into()), window, cx);

    let with_panel = |f: fn(&mut Workbench, Entity<TerminalPanel>, &mut Window, &mut Context<Workbench>)| {
        let weak = cx.weak_entity();
        move |_: &Value, window: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |workbench, cx| {
                if let Some(terminal) = panel(workbench) {
                    f(workbench, terminal, window, cx);
                }
            });
        }
    };
    workbench.register_command(Command::new("terminal.toggle", "Toggle Terminal", with_panel(toggle)).category("Terminal").key("Mod+`"));
    workbench.register_command(
        Command::new("terminal.new", "New Terminal", with_panel(|workbench, terminal, window, cx| {
            workbench.show_panel(PANEL_ID, cx);
            terminal.update(cx, |terminal, cx| terminal.new_session(window, cx));
        }))
        .category("Terminal")
        .key("Mod+Shift+`"),
    );
    workbench.register_command(
        Command::new("terminal.focus", "Focus Terminal", with_panel(|workbench, terminal, window, cx| {
            workbench.show_panel(PANEL_ID, cx);
            terminal.update(cx, |terminal, cx| terminal.ensure_session(window, cx));
        }))
        .category("Terminal"),
    );
    let has_session = {
        let terminal = terminal.clone();
        move |_: &Window, cx: &App| !terminal.read(cx).sessions().is_empty()
    };
    workbench.register_command(
        Command::new("terminal.kill", "Kill Terminal", with_panel(|_, terminal, window, cx| terminal.update(cx, |terminal, cx| terminal.kill_active(window, cx))))
            .category("Terminal")
            .when(has_session.clone()),
    );
    workbench.register_command(
        Command::new("terminal.clear", "Clear Terminal", with_panel(|_, terminal, _, cx| terminal.update(cx, |terminal, cx| terminal.clear_active(cx))))
            .category("Terminal")
            .when(has_session.clone()),
    );
    workbench.register_command(
        Command::new("terminal.focusNext", "Focus Next Terminal", with_panel(|workbench, terminal, window, cx| {
            workbench.show_panel(PANEL_ID, cx);
            terminal.update(cx, |terminal, cx| terminal.focus_next(window, cx));
        }))
        .category("Terminal")
        .when(has_session),
    );
}

/// Menampilkan terminal (membuat sesi pertama bila belum ada), atau menyembunyikan panelnya bila
/// terminal sudah terlihat dan fokus.
fn toggle(workbench: &mut Workbench, terminal: Entity<TerminalPanel>, window: &mut Window, cx: &mut Context<Workbench>) {
    let showing = workbench.active_panel().is_some_and(|id| id.as_ref() == PANEL_ID);
    let focused = terminal.read(cx).active_session().is_some_and(|session| session.read(cx).focus_handle_is_focused(window));
    if showing && focused {
        workbench.hide_panel(window, cx);
        return;
    }
    workbench.show_panel(PANEL_ID, cx);
    terminal.update(cx, |terminal, cx| terminal.ensure_session(window, cx));
}

/// Tombol "Terminal" di status bar.
struct TerminalButton;

impl Render for TerminalButton {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Button::new("status-terminal").ghost().xsmall().icon(Icon::new(Lucide::SquareTerminal)).label("Terminal").tooltip("Toggle Terminal").on_click(|_, window, cx| {
            oncode_ui::execute("terminal.toggle", Value::Null, window, cx);
        })
    }
}
