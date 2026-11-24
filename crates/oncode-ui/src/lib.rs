//! Kerangka UI native OnCode (GPUI): jendela, tab, panel, command, keybinding.
//!
//! Crate fitur (`oncode-ui-*`) bergantung pada crate ini dan mendaftarkan diri lewat
//! [`register_feature`]. Aturan penting agar tidak terjadi peminjaman ganda entitas:
//! lihat `docs/PENGEMBANGAN.md` bagian "Aturan entitas".

pub mod app;
pub mod commands;
pub mod editor;
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
pub use model::{OpenOptions, Orientation, TabData};
pub use repos::{repos, FileMark, Repos, ReposEvent};
pub use shell::{file_tab, Shell};
pub use workbench::{
    register_feature, register_tab_kind, Env, PanelView, SidebarView, StatusAlign, StatusItem, TabContent, TabKind, ViewTab, Workbench, WorkbenchEvent,
};

/// Ikon Lucide lengkap (± 1.800 ikon) dari paket aset bawaan.
pub use gpui_kit::assets::IconName as Lucide;

/// Menyiapkan semua yang bersifat global. Dipanggil sekali setelah `gpui_kit::init`.
pub fn init(app: OnCode, cx: &mut gpui_kit::App) {
    app::install(app, cx);
    commands::install(cx);
    workbench::install(cx);
    repos::install(cx);
    editor::install(cx);
}

/// Membuka jendela baru, opsional langsung dengan sebuah folder.
pub fn open_window(folder: Option<std::path::PathBuf>, cx: &mut gpui_kit::App) {
    if let Err(error) = try_open_window(folder, cx) {
        eprintln!("oncode: failed to open a window: {error}");
    }
}

/// Seperti [`open_window`], tetapi kegagalannya dikembalikan ke pemanggil (mis. untuk dicatat ke log
/// saat jendela pertama gagal dibuka, karena aplikasi GUI di Windows tidak punya konsol).
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
