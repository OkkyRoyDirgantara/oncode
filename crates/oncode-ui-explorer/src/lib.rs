//! File explorer, welcome screen, and restricted mode.
//!
//! - [`Explorer`]: the `explorer` sidebar view (a file tree with Git markers and file operations).
//! - [`Welcome`]: the contents of the editor area when there are no tabs (the welcome screen or shortcut hints).
//! - Restricted mode: a banner at the top of the Explorer and a status bar item while the folder is not trusted.
//! - The initial sidebar view follows the `workbench.startupView` setting.

mod explorer;
mod restricted;
pub mod tree;
mod welcome;

use std::collections::HashMap;

use gpui_kit::{App, AppContext as _, Context, Entity, Global, SharedString, WeakEntity, Window};
use oncode_core::settings::StartupView;
use oncode_ui::{Command, Lucide, SidebarView, StatusAlign, StatusItem, Workbench, WorkbenchEvent};
use serde_json::Value;

pub use explorer::{EntryKind, Explorer, RevealHandler, REVEAL_ACTIVE_LABEL, REVEAL_LABEL};
pub use restricted::{RestrictedStatus, RESTRICTED_MESSAGE};
pub use welcome::Welcome;

/// Id of the Explorer sidebar view.
pub const SIDEBAR_ID: &str = "explorer";

/// The last sidebar view per window: (project folder, has a repository, view id).
/// `Workbench` is recreated when the trust status changes; the user's choice must not be lost.
#[derive(Default)]
struct SidebarMemory(HashMap<String, (String, bool, SharedString)>);

impl Global for SidebarMemory {}

/// Registers this feature. Called once from `app/src/main.rs` after `oncode_ui::init`.
pub fn init(cx: &mut App) {
    cx.set_global(SidebarMemory::default());
    oncode_ui::register_feature(cx, setup);
}

/// The Explorer of a window.
pub fn explorer(workbench: &Workbench) -> Option<Entity<Explorer>> {
    workbench.extension::<Entity<Explorer>>()
}

/// The welcome screen (or shortcut hints) of a window.
pub fn welcome(workbench: &Workbench) -> Option<Entity<Welcome>> {
    workbench.extension::<Entity<Welcome>>()
}

/// The sidebar view chosen when a folder is opened.
///
/// A new folder, or a repository that has just become available (the folder was just trusted):
/// Source Control if the folder is a repository and the setting asks for it, otherwise Explorer.
/// The same folder reloaded without that change: the previous view is kept (`None`).
pub fn startup_sidebar(previous: Option<(&str, bool)>, root: &str, has_repo: bool, setting: StartupView) -> Option<&'static str> {
    let same_root = previous.is_some_and(|(previous_root, _)| previous_root == root);
    let repo_appeared = has_repo && !previous.is_some_and(|(_, had_repo)| had_repo);
    if same_root && !repo_appeared {
        return None;
    }
    if has_repo && setting == StartupView::Scm {
        Some("scm")
    } else if same_root {
        None
    } else {
        Some(SIDEBAR_ID)
    }
}

fn setup(workbench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let env = workbench.env().clone();

    let explorer = cx.new(|cx| Explorer::new(env.clone(), window, cx));
    workbench.set_extension(explorer.clone());
    workbench.register_sidebar(SidebarView::new(SIDEBAR_ID, "Explorer", Lucide::Files, 10, {
        let explorer = explorer.clone();
        move |_, _, _| explorer.clone().into()
    }));

    let welcome = cx.new(|cx| Welcome::new(env.clone(), cx));
    workbench.set_extension(welcome.clone());
    workbench.set_empty_view(welcome.into());

    if env.root.is_some() && !env.trusted {
        workbench.register_status(StatusItem::new("workspace.restricted", StatusAlign::Left, 0, |_, _, cx| cx.new(|_| RestrictedStatus).into()), window, cx);
    }

    register_commands(workbench, &explorer, cx.weak_entity());
    choose_startup_sidebar(workbench, window, cx);
}

/// Chooses the initial sidebar view, then records the user's subsequent choices.
fn choose_startup_sidebar(workbench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let env = workbench.env().clone();
    let label = env.window.to_string();
    let Some(root) = env.root_string() else {
        cx.default_global::<SidebarMemory>().0.remove(&label);
        return;
    };
    let has_repo = env.repo_root.is_some();
    let previous = cx.default_global::<SidebarMemory>().0.get(&label).cloned();
    let chosen = startup_sidebar(previous.as_ref().map(|(root, had_repo, _)| (root.as_str(), *had_repo)), &root, has_repo, workbench.settings().workbench.startup_view);
    let wanted: SharedString = match chosen {
        Some(id) => id.into(),
        None => previous.map(|(_, _, sidebar)| sidebar).unwrap_or_else(|| SIDEBAR_ID.into()),
    };

    // Other features (e.g. Source Control) are registered after this one; the view is chosen once all are registered.
    cx.defer_in(window, {
        let (label, root) = (label.clone(), root.clone());
        move |workbench, _, cx| {
            workbench.show_sidebar(&wanted, cx);
            let current = workbench.selected_sidebar().cloned().unwrap_or_else(|| SIDEBAR_ID.into());
            cx.default_global::<SidebarMemory>().0.insert(label, (root, has_repo, current));
        }
    });
    if let Some(entity) = env.workbench.upgrade() {
        cx.subscribe(&entity, move |workbench, _, event: &WorkbenchEvent, cx| {
            if matches!(event, WorkbenchEvent::LayoutChanged) {
                if let Some(current) = workbench.selected_sidebar().cloned() {
                    cx.default_global::<SidebarMemory>().0.insert(label.clone(), (root.clone(), has_repo, current));
                }
            }
        })
        .detach();
    }
}

fn register_commands(workbench: &mut Workbench, explorer: &Entity<Explorer>, weak_workbench: WeakEntity<Workbench>) {
    let has_root = workbench.env().root.is_some();
    let weak = explorer.downgrade();

    // Runs `f` on the Explorer; `reveal` shows its sidebar first.
    let run = |reveal: bool, f: fn(&mut Explorer, &mut Window, &mut Context<Explorer>)| {
        let (weak, weak_workbench) = (weak.clone(), weak_workbench.clone());
        move |_: &Value, window: &mut Window, cx: &mut App| {
            if reveal {
                let _ = weak_workbench.update(cx, |workbench, cx| workbench.show_sidebar(SIDEBAR_ID, cx));
            }
            let _ = weak.update(cx, |explorer, cx| f(explorer, window, cx));
        }
    };
    let has_selection = {
        let weak = weak.clone();
        move |_: &Window, cx: &App| weak.upgrade().is_some_and(|explorer| explorer.read(cx).selected_entry().is_some())
    };
    // F2 and Delete only apply while keyboard focus is in the tree.
    let tree_action = {
        let weak = weak.clone();
        move |window: &Window, cx: &App| {
            weak.upgrade().is_some_and(|explorer| {
                let explorer = explorer.read(cx);
                explorer.tree_focused(window) && explorer.selected_entry().is_some()
            })
        }
    };

    workbench.register_command(Command::new("explorer.focus", "Focus on Explorer", run(true, |explorer, window, cx| explorer.focus(window, cx))).category("View").key("Mod+Shift+E"));
    workbench.register_command(
        Command::new("explorer.newFile", "New File…", run(true, |explorer, window, cx| explorer.new_entry(EntryKind::File, None, window, cx))).category("Explorer").when(move |_, _| has_root),
    );
    workbench.register_command(
        Command::new("explorer.newFolder", "New Folder…", run(true, |explorer, window, cx| explorer.new_entry(EntryKind::Folder, None, window, cx)))
            .category("Explorer")
            .when(move |_, _| has_root),
    );
    workbench.register_command(
        Command::new("explorer.rename", "Rename…", run(true, |explorer, window, cx| explorer.rename_selected(window, cx))).category("Explorer").key("F2").when(tree_action.clone()),
    );
    workbench.register_command(
        Command::new("explorer.delete", "Delete (Move to Trash)…", run(true, |explorer, window, cx| explorer.delete_selected(window, cx)))
            .category("Explorer")
            .key("Delete")
            .when(tree_action),
    );
    workbench.register_command(
        Command::new("explorer.copyPath", "Copy Path", run(false, |explorer, _, cx| explorer.copy_selected_path(false, cx))).category("Explorer").when(has_selection.clone()),
    );
    workbench.register_command(
        Command::new("explorer.copyRelativePath", "Copy Relative Path", run(false, |explorer, _, cx| explorer.copy_selected_path(true, cx))).category("Explorer").when(has_selection),
    );
    // The file in the active tab; disabled for non-file tabs and `untitled:` files.
    let has_active_file = {
        let weak = weak.clone();
        move |_: &Window, cx: &App| weak.upgrade().is_some_and(|explorer| explorer.read(cx).active_file().is_some())
    };
    workbench.register_command(
        Command::new("explorer.revealActiveFile", REVEAL_ACTIVE_LABEL, run(false, |explorer, _, cx| explorer.reveal_active_in_file_manager(cx))).category("Explorer").when(has_active_file),
    );
    workbench.register_command(Command::new("explorer.collapseAll", "Collapse All", run(false, |explorer, _, cx| explorer.collapse_all(cx))).category("Explorer").when(move |_, _| has_root));
    workbench.register_command(Command::new("explorer.refresh", "Refresh", run(false, |explorer, _, cx| explorer.refresh(cx))).category("Explorer").when(move |_, _| has_root));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_folder_starts_on_source_control_only_when_it_is_a_repository_and_the_setting_says_so() {
        assert_eq!(startup_sidebar(None, "/ws", true, StartupView::Scm), Some("scm"));
        assert_eq!(startup_sidebar(None, "/ws", true, StartupView::Explorer), Some("explorer"));
        assert_eq!(startup_sidebar(None, "/ws", false, StartupView::Scm), Some("explorer"));
        // Another folder in the same window is treated like a new folder.
        assert_eq!(startup_sidebar(Some(("/old", true)), "/ws", false, StartupView::Scm), Some("explorer"));
        assert_eq!(startup_sidebar(Some(("/old", false)), "/ws", true, StartupView::Scm), Some("scm"));
    }

    #[test]
    fn trusting_a_repository_folder_switches_to_source_control() {
        assert_eq!(startup_sidebar(Some(("/ws", false)), "/ws", true, StartupView::Scm), Some("scm"));
        // The setting chooses Explorer: the view currently in use is not replaced.
        assert_eq!(startup_sidebar(Some(("/ws", false)), "/ws", true, StartupView::Explorer), None);
    }

    #[test]
    fn reloading_the_same_folder_keeps_the_current_view() {
        assert_eq!(startup_sidebar(Some(("/ws", true)), "/ws", true, StartupView::Scm), None);
        assert_eq!(startup_sidebar(Some(("/ws", false)), "/ws", false, StartupView::Scm), None);
        assert_eq!(startup_sidebar(Some(("/ws", true)), "/ws", false, StartupView::Scm), None);
    }
}
