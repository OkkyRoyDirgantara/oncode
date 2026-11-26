//! Search feature: project-wide text search and replace (per match, per file,
//! or all). Also available in restricted mode (a folder that is not yet trusted).
//!
//! - [`matcher`]: the per-line matcher (the same engine as the backend) and the replacement text.
//! - [`model`]: search result state without UI.
//! - [`SearchView`]: the sidebar view along with how the search runs.

pub mod matcher;
pub mod model;
mod replace;
mod view;

use gpui_kit::{App, AppContext as _, Entity, WeakEntity, Window};
use oncode_ui::editor::active_file;
use oncode_ui::{Command, Lucide, SidebarView, Workbench};

pub use view::{SearchOption, SearchView};

/// Id of this feature's sidebar view.
pub const SIDEBAR_ID: &str = "search";

/// Registers this feature. Called once from `app/src/main.rs` after `oncode_ui::init`.
pub fn init(cx: &mut App) {
    oncode_ui::register_feature(cx, |workbench, window, cx| {
        let env = workbench.env().clone();
        // The view is created now (not when the sidebar is first opened) so commands can
        // fill in and focus the query field at any time.
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

/// The Search view of a window.
pub fn search_view(workbench: &Workbench) -> Option<Entity<SearchView>> {
    workbench.extension::<Entity<SearchView>>()
}

/// Brings up the Search view and focuses the query field, filled with the text selected in the editor if
/// any (a single non-empty line). With `replace`, the replacement row is shown too.
fn focus_search(workbench: &WeakEntity<Workbench>, view: &Entity<SearchView>, replace: bool, window: &mut Window, cx: &mut App) {
    let Some(workbench) = workbench.upgrade() else { return };
    let selection = active_file(workbench.read(cx), cx).map(|(_, _, state)| state.read(cx).selected_value().to_string());
    workbench.update(cx, |workbench, cx| workbench.show_sidebar(SIDEBAR_ID, cx));
    view.update(cx, |view, cx| {
        // In regex mode, the selected text is searched for as is.
        let prefill = selection.and_then(|selection| model::prefill_from_selection(&selection, view.option(SearchOption::Regex)));
        view.focus_query(prefill, window, cx);
        if replace {
            view.show_replace(window, cx);
        }
    });
}
