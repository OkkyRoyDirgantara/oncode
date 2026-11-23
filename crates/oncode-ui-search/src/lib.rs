//! Fitur Search: pencarian teks di seluruh proyek dan penggantian (per kecocokan, per berkas,
//! atau semuanya). Tersedia juga di mode terbatas (folder yang belum dipercaya).
//!
//! - [`matcher`]: pencocok per baris (mesin yang sama dengan backend) dan teks pengganti.
//! - [`model`]: state hasil pencarian tanpa UI.
//! - [`SearchView`]: tampilan sidebar beserta jalannya pencarian.

pub mod matcher;
pub mod model;
mod replace;
mod view;

use gpui_kit::{App, AppContext as _, Entity, WeakEntity, Window};
use oncode_ui::editor::active_file;
use oncode_ui::{Command, Lucide, SidebarView, Workbench};

pub use view::{SearchOption, SearchView};

/// Id tampilan sidebar fitur ini.
pub const SIDEBAR_ID: &str = "search";

/// Mendaftarkan fitur ini. Dipanggil sekali dari `app/src/main.rs` setelah `oncode_ui::init`.
pub fn init(cx: &mut App) {
    oncode_ui::register_feature(cx, |workbench, window, cx| {
        let env = workbench.env().clone();
        // Tampilan dibuat sekarang (bukan saat sidebar pertama kali dibuka) agar command bisa
        // mengisi dan memfokuskan kolom kueri kapan pun.
        let view = cx.new(|cx| SearchView::new(env, window, cx));
        workbench.set_extension(view.clone());
        workbench.register_sidebar(SidebarView::new(SIDEBAR_ID, "Search", Lucide::Search, 20, {
            let view = view.clone();
            move |_, _, _| view.clone().into()
        }));

        let weak = cx.weak_entity();
        workbench.register_command(
            Command::new("search.focus", "Find in Files", {
                let (weak, view) = (weak.clone(), view.clone());
                move |_, window, cx| focus_search(&weak, &view, false, window, cx)
            })
            .category("Search")
            .key("Mod+Shift+F"),
        );
        workbench.register_command(Command::new("search.replace", "Replace in Files", move |_, window, cx| focus_search(&weak, &view, true, window, cx)).category("Search"));
    });
}

/// Tampilan Search milik sebuah jendela.
pub fn search_view(workbench: &Workbench) -> Option<Entity<SearchView>> {
    workbench.extension::<Entity<SearchView>>()
}

/// Memunculkan tampilan Search dan memfokuskan kolom kueri, diisi teks terpilih di editor bila
/// ada (satu baris, tidak kosong). Dengan `replace`, baris pengganti ikut ditampilkan.
fn focus_search(workbench: &WeakEntity<Workbench>, view: &Entity<SearchView>, replace: bool, window: &mut Window, cx: &mut App) {
    let Some(workbench) = workbench.upgrade() else { return };
    let selection = active_file(workbench.read(cx), cx).map(|(_, _, state)| state.read(cx).selected_value().to_string());
    workbench.update(cx, |workbench, cx| workbench.show_sidebar(SIDEBAR_ID, cx));
    view.update(cx, |view, cx| {
        // Pada mode regex, teks terpilih dicari apa adanya.
        let prefill = selection.and_then(|selection| model::prefill_from_selection(&selection, view.option(SearchOption::Regex)));
        view.focus_query(prefill, window, cx);
        if replace {
            view.show_replace(window, cx);
        }
    });
}
