//! OnCode native UI framework (GPUI): window, tabs, panels, commands, keybindings.
//!
//! Feature crates (`oncode-ui-*`) depend on this crate and register themselves through
//! [`register_feature`]. Key rules to avoid double entity borrows:
//! see `docs/DEVELOPMENT.md`, section "Entity rules".

pub mod app;
pub mod app_menu;
pub mod commands;
pub mod editor;
pub mod file_icons;
pub mod keys;
pub mod menubar;
pub mod model;
pub mod overlay;
pub mod quick_pick;
pub mod repos;
pub mod shell;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod workbench;

pub use app::{core, core_bus, spawn_core, CoreBus, OnCode};
pub use commands::{execute, execute_now, Command, CommandRegistry};
pub use model::{OpenOptions, Orientation, SplitSide, TabData};
pub use repos::{repos, FileMark, Repos, ReposEvent};
pub use shell::{file_tab, Shell};
pub use workbench::{
    register_feature, register_tab_kind, DraggedEntry, DraggedTab, EditorAction, Env, PanelView, SidebarView, StatusAlign, StatusItem, TabContent, TabDragPreview, TabKind, ViewTab,
    Workbench, WorkbenchEvent,
};

/// The full Lucide icon set (~1,800 icons) from the bundled asset pack.
pub use gpui_kit::assets::IconName as Lucide;

/// Sets up everything global. Called once after `gpui_kit::init`.
pub fn init(app: OnCode, cx: &mut gpui_kit::App) {
    app::install(app, cx);
    commands::install(cx);
    workbench::install(cx);
    repos::install(cx);
    editor::install(cx);
    app_menu::install(cx);
}

/// Opens a new window, optionally straight away with a folder.
pub fn open_window(folder: Option<std::path::PathBuf>, cx: &mut gpui_kit::App) {
    if let Err(error) = try_open_window(folder, cx) {
        eprintln!("oncode: failed to open a window: {error}");
    }
}

/// Folder from another OnCode process (`oncode .` in a terminal, see `app/src/single_instance.rs`):
/// a window that already has that folder open is focused, otherwise a new window is opened (without
/// a folder if there is none), like VS Code's `code`.
pub fn open_from_command_line(folder: Option<std::path::PathBuf>, cx: &mut gpui_kit::App) {
    cx.activate(true);
    match folder.as_deref().and_then(|folder| shell::window_with_folder(folder, cx)) {
        Some(handle) => {
            let _ = handle.update(cx, |_, window, _| window.activate_window());
        }
        None => open_window(folder, cx),
    }
}

/// Like [`open_window`], but the failure is returned to the caller (e.g. to be written to a log
/// when the first window fails to open, since GUI apps on Windows have no console).
pub fn try_open_window(folder: Option<std::path::PathBuf>, cx: &mut gpui_kit::App) -> gpui_kit::Result<()> {
    use gpui_kit::{px, size, AppContext as _, WindowBounds, WindowOptions};
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(1280.), px(800.)), cx)),
        window_min_size: Some(size(px(640.), px(400.))),
        app_id: Some(oncode_core::APP_IDENTIFIER.into()),
        ..gpui_kit::component::TitleBar::window_options()
    };
    gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| Shell::new(folder, window, cx)))?;
    Ok(())
}
