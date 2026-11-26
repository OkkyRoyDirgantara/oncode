//! Settings feature: the `settings` tab (a form over user-level settings) and the commands
//! to open it. The tab can be opened without a project folder, so the Git path can be fixed when
//! Git is not found.
//!
//! The Accounts section provides a slot for views from other crates (the account list and sign-in owned
//! by the forge feature): see [`register_accounts_view`].

pub mod model;
mod page;

use std::rc::Rc;

use gpui_kit::{AnyView, App, AppContext as _, Global, Window};
use oncode_core::api;
use oncode_ui::editor::{open_file, OpenFile};
use oncode_ui::{overlay, spawn_core, Command, Env, Lucide, OpenOptions, TabContent, TabData, TabKind, ViewTab, Workbench};
use serde_json::json;

pub use page::{SettingsPage, TEXT_DEBOUNCE};

/// Kind and id of the settings tab. The id is fixed, so the tab never opens twice in one group.
pub const SETTINGS_TAB: &str = "settings";

type AccountsBuilder = Rc<dyn Fn(&Env, &mut Window, &mut App) -> AnyView>;

#[derive(Default)]
struct AccountsSlot(Option<AccountsBuilder>);

impl Global for AccountsSlot {}

/// Registers the view inserted in the Accounts section of the settings page (account list,
/// sign-in and sign-out buttons). The last registration replaces the previous one.
///
/// `builder` is called once for every settings tab that is opened, **after** `Workbench`
/// has finished building the tab, so it may read `Workbench` through `env.workbench`.
pub fn register_accounts_view(cx: &mut App, builder: impl Fn(&Env, &mut Window, &mut App) -> AnyView + 'static) {
    cx.default_global::<AccountsSlot>().0 = Some(Rc::new(builder));
}

pub(crate) fn accounts_builder(cx: &App) -> Option<AccountsBuilder> {
    cx.try_global::<AccountsSlot>().and_then(|slot| slot.0.clone())
}

/// The settings tab for [`Workbench::open_tab`].
pub fn settings_tab() -> TabData {
    TabData::new(SETTINGS_TAB, "Settings", json!({})).with_id(SETTINGS_TAB).with_description("User settings")
}

/// Opens (or activates) the settings tab in this window. Don't call it while `Workbench`
/// is borrowed; from there use `oncode_ui::execute("settings.open", ..)`.
pub fn open_settings(window: &mut Window, cx: &mut App) {
    if let Some(workbench) = Workbench::for_window(window, cx) {
        workbench.update(cx, |workbench, cx| {
            workbench.open_tab(settings_tab(), OpenOptions::default(), window, cx);
        });
    }
}

/// Opens the user's `settings.json` in the editor. A file that has never been written is first created from
/// the settings currently in effect, so there is something to edit.
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

/// The settings page in the active tab, if that tab is a settings tab.
pub fn active_page(workbench: &Workbench) -> Option<gpui_kit::Entity<SettingsPage>> {
    workbench.active_content()?.as_any().downcast_ref::<ViewTab<SettingsPage>>().map(|tab| tab.0.clone())
}

/// Registers this feature. Called once from `app/src/main.rs` after `oncode_ui::init`.
pub fn init(cx: &mut App) {
    cx.default_global::<AccountsSlot>();
    oncode_ui::register_tab_kind(cx, SETTINGS_TAB, SettingsKind);
    // No folder or trust requirement: settings must always be openable.
    oncode_ui::register_feature(cx, |workbench, _, _| {
        workbench.register_command(Command::new("settings.open", "Open Settings", |_, window, cx| open_settings(window, cx)).category("Preferences").key("Mod+,"));
        workbench.register_command(Command::new("settings.openJson", "Open settings.json", |_, window, cx| open_settings_json(window, cx)).category("Preferences"));
    });
}
