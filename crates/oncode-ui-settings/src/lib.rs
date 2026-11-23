//! Fitur pengaturan: tab `settings` (formulir di atas pengaturan tingkat pengguna) dan command
//! untuk membukanya. Tab bisa dibuka tanpa folder proyek, agar path Git bisa diperbaiki saat
//! Git tidak ditemukan.
//!
//! Bagian Accounts menyediakan tempat untuk tampilan dari crate lain (daftar akun dan login milik
//! fitur forge): lihat [`register_accounts_view`].

pub mod model;
mod page;

use std::rc::Rc;

use gpui_kit::{AnyView, App, AppContext as _, Global, Window};
use oncode_core::api;
use oncode_ui::editor::{open_file, OpenFile};
use oncode_ui::{overlay, spawn_core, Command, Env, Lucide, OpenOptions, TabContent, TabData, TabKind, ViewTab, Workbench};
use serde_json::json;

pub use page::{SettingsPage, TEXT_DEBOUNCE};

/// Jenis dan id tab pengaturan. Id tetap, sehingga tab tidak pernah terbuka dua kali dalam satu grup.
pub const SETTINGS_TAB: &str = "settings";

type AccountsBuilder = Rc<dyn Fn(&Env, &mut Window, &mut App) -> AnyView>;

#[derive(Default)]
struct AccountsSlot(Option<AccountsBuilder>);

impl Global for AccountsSlot {}

/// Mendaftarkan tampilan yang disisipkan di bagian Accounts halaman pengaturan (daftar akun,
/// tombol masuk dan keluar). Pendaftaran terakhir menggantikan yang sebelumnya.
///
/// `builder` dipanggil sekali untuk setiap tab pengaturan yang dibuka, **setelah** `Workbench`
/// selesai membangun tab, jadi boleh membaca `Workbench` lewat `env.workbench`.
pub fn register_accounts_view(cx: &mut App, builder: impl Fn(&Env, &mut Window, &mut App) -> AnyView + 'static) {
    cx.default_global::<AccountsSlot>().0 = Some(Rc::new(builder));
}

pub(crate) fn accounts_builder(cx: &App) -> Option<AccountsBuilder> {
    cx.try_global::<AccountsSlot>().and_then(|slot| slot.0.clone())
}

/// Tab pengaturan untuk [`Workbench::open_tab`].
pub fn settings_tab() -> TabData {
    TabData::new(SETTINGS_TAB, "Settings", json!({})).with_id(SETTINGS_TAB).with_description("User settings")
}

/// Membuka (atau mengaktifkan) tab pengaturan di jendela ini. Jangan dipanggil selagi `Workbench`
/// dipinjam; dari sana pakai `oncode_ui::execute("settings.open", ..)`.
pub fn open_settings(window: &mut Window, cx: &mut App) {
    if let Some(workbench) = Workbench::for_window(window, cx) {
        workbench.update(cx, |workbench, cx| {
            workbench.open_tab(settings_tab(), OpenOptions::default(), window, cx);
        });
    }
}

/// Membuka `settings.json` pengguna di editor. Berkas yang belum pernah ditulis dibuat dulu dari
/// pengaturan yang sedang berlaku, supaya ada yang bisa disunting.
pub fn open_settings_json(window: &mut Window, cx: &mut App) {
    let core = oncode_ui::core(cx);
    let task = spawn_core(cx, async move {
        let path = core.settings_path().to_path_buf();
        if !path.exists() {
            api::app::settings_set(&core, core.user_settings()).await?;
        }
        Ok::<_, oncode_core::AppError>(path.to_string_lossy().into_owned())
    });
    window
        .spawn(cx, async move |cx| {
            let result = task.await;
            let _ = cx.update(|window, cx| match result {
                Ok(path) => {
                    if let Some(workbench) = Workbench::for_window(window, cx) {
                        workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(path), window, cx));
                    }
                }
                Err(error) => overlay::show_error(&error, window, cx),
            });
        })
        .detach();
}

struct SettingsKind;

impl TabKind for SettingsKind {
    fn build(&self, _: &TabData, env: &Env, window: &mut Window, cx: &mut App) -> Box<dyn TabContent> {
        Box::new(ViewTab(cx.new(|cx| SettingsPage::new(env.clone(), window, cx))))
    }

    fn icon(&self, _: &TabData) -> Option<Lucide> {
        Some(Lucide::Settings)
    }
}

/// Halaman pengaturan di tab aktif, bila tab itu tab pengaturan.
pub fn active_page(workbench: &Workbench) -> Option<gpui_kit::Entity<SettingsPage>> {
    workbench.active_content()?.as_any().downcast_ref::<ViewTab<SettingsPage>>().map(|tab| tab.0.clone())
}

/// Mendaftarkan fitur ini. Dipanggil sekali dari `app/src/main.rs` setelah `oncode_ui::init`.
pub fn init(cx: &mut App) {
    cx.default_global::<AccountsSlot>();
    oncode_ui::register_tab_kind(cx, SETTINGS_TAB, SettingsKind);
    // Tanpa syarat folder atau kepercayaan: pengaturan harus selalu bisa dibuka.
    oncode_ui::register_feature(cx, |workbench, _, _| {
        workbench.register_command(Command::new("settings.open", "Open Settings", |_, window, cx| open_settings(window, cx)).category("Preferences").key("Mod+,"));
        workbench.register_command(Command::new("settings.openJson", "Open settings.json", |_, window, cx| open_settings_json(window, cx)).category("Preferences"));
    });
}
