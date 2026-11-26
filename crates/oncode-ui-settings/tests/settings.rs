//! View tests for the settings page: real controls in a headless window, a real backend,
//! and the end result checked in `settings.json` on disk.

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{div, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, TestAppContext, Window};
use oncode_core::api;
use oncode_core::settings::Theme;
use oncode_core::Settings;
use oncode_ui::testing::{TestApp, TestWindow};
use oncode_ui::Command;
use oncode_ui_settings::{SettingsPage, TEXT_DEBOUNCE};
use serde_json::{json, Value};

fn setup(cx: &mut TestAppContext) -> TestApp {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_settings::init);
    app
}

/// The content of `settings.json` on disk; `Null` if the file has never been written.
fn on_disk(app: &TestApp) -> Value {
    std::fs::read_to_string(app.core.settings_path()).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or(Value::Null)
}

fn tab_ids(window: &TestWindow, cx: &mut TestAppContext) -> Vec<String> {
    let workbench = window.workbench(cx);
    cx.update(|cx| workbench.read(cx).model().all_tabs().map(|tab| tab.id.clone()).collect())
}

fn effective(window: &TestWindow, cx: &mut TestAppContext) -> Settings {
    let workbench = window.workbench(cx);
    cx.update(|cx| workbench.read(cx).settings().clone())
}

fn open_settings(window: &TestWindow, cx: &mut TestAppContext) -> Entity<SettingsPage> {
    assert!(window.run_command(cx, "settings.open"));
    cx.run_until_parked();
    let workbench = window.workbench(cx);
    let page = cx.update(|cx| oncode_ui_settings::active_page(workbench.read(cx)).expect("settings tab active"));
    window.update(cx, |window, cx| window.render_frame(cx));
    page
}

/// Waits until all changes are saved (including the text input delay).
async fn settle(window: &TestWindow, page: &Entity<SettingsPage>, cx: &mut TestAppContext) {
    window.wait_until(cx, "settings saved", |_, cx| page.read(cx).is_settled()).await;
    cx.run_until_parked();
    window.update(cx, |window, cx| window.render_frame(cx));
}

fn exists(window: &TestWindow, cx: &mut TestAppContext, id: &'static str) -> bool {
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.try_find(id).is_some()
    })
}

/// Clicks an input box, selects all its content, then types the replacement.
fn retype(window: &TestWindow, cx: &mut TestAppContext, id: &'static str, text: &str) {
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
        window.press("secondary-a", cx);
        if text.is_empty() {
            window.press("backspace", cx);
        } else {
            window.input(text, cx);
        }
    });
    cx.run_until_parked();
}

/// Filters the page through the search box, so the target row is in the visible area.
fn search(window: &TestWindow, cx: &mut TestAppContext, query: &str) {
    retype(window, cx, "settings-search", query);
    window.update(cx, |window, cx| window.render_frame(cx));
}

fn click(window: &TestWindow, cx: &mut TestAppContext, id: &'static str) {
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    });
    cx.run_until_parked();
    // Focus listeners (e.g. an input box's `on_blur`) are invoked by GPUI after the next frame is drawn.
    window.update(cx, |window, cx| window.render_frame(cx));
    cx.run_until_parked();
}

/// Writes the user settings from outside the page, like another window.
fn save_externally(app: &TestApp, settings: Settings) {
    let core = app.core.clone();
    app.runtime.block_on(async move { api::app::settings_set(&core, settings).await }).unwrap();
}

fn write_project_settings(project: &Path, json: &str) {
    std::fs::create_dir_all(project.join(".oncode")).unwrap();
    std::fs::write(project.join(".oncode/settings.json"), json).unwrap();
}

// Counterpart of the e2e "start screen without a folder; Settings can still be opened" and the v0.2 registration test.
#[gpui_kit::test]
async fn settings_open_without_a_folder_through_the_shortcut(cx: &mut TestAppContext) {
    let app = setup(cx);
    let window = app.open(None, cx).await;

    let command = window.update(cx, |window, cx| oncode_ui::commands::get("settings.open", window, cx)).expect("settings.open registered");
    assert_eq!((command.title.as_ref(), command.category.as_deref(), command.keybindings.as_slice()), ("Open Settings", Some("Preferences"), &["Mod+,".to_string()][..]));
    assert!(tab_ids(&window, cx).is_empty());

    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press("secondary-,", cx);
    });
    cx.run_until_parked();
    assert_eq!(tab_ids(&window, cx), ["settings"]);
    assert!(exists(&window, cx, "settings-page"));
    assert!(exists(&window, cx, "st-editor.wordWrap"));

    // Opening it again activates the same tab rather than duplicating it.
    assert!(window.run_command(cx, "settings.open"));
    cx.run_until_parked();
    assert_eq!(tab_ids(&window, cx), ["settings"]);
    let workbench = window.workbench(cx);
    let tab = cx.update(|cx| workbench.read(cx).model().active().map(|(_, tab)| tab.clone())).unwrap();
    assert_eq!((tab.kind.as_str(), tab.title.as_str(), tab.description.as_deref()), ("settings", "Settings", Some("User settings")));

    // Without a folder the form works fully and there is no project marker.
    click(&window, cx, "st-editor.wordWrap");
    let page = cx.update(|cx| oncode_ui_settings::active_page(workbench.read(cx))).unwrap();
    settle(&window, &page, cx).await;
    assert_eq!(on_disk(&app)["editor"]["wordWrap"], json!(true));
    assert!(!exists(&window, cx, "st-editor.wordWrap-override"));

    // Closing the last tab returns to the empty screen.
    assert!(window.run_command(cx, "workbench.closeTab"));
    window.wait_until(cx, "tab closed", |shell, cx| shell.read(cx).workbench().read(cx).model().all_tabs().next().is_none()).await;
}

// Counterpart of the e2e "settings are saved to settings.json and take effect immediately".
#[gpui_kit::test]
async fn toggling_a_switch_persists_and_applies_immediately(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("README.md", "# Demo\n")]);
    let window = app.open(Some(&project), cx).await;
    let page = open_settings(&window, cx);
    assert!(!effective(&window, cx).editor.word_wrap, "initially without word wrap");
    assert_eq!(window.update(cx, |window, _| window.find("st-editor.wordWrap").checked()), Some(false));

    click(&window, cx, "st-editor.wordWrap");
    settle(&window, &page, cx).await;
    let saved = on_disk(&app);
    assert_eq!(saved["editor"]["wordWrap"], json!(true), "settings.json updated");
    assert_eq!(saved["editor"]["tabSize"], json!(4), "other settings remain intact");
    assert!(std::fs::read_to_string(app.core.settings_path()).unwrap().contains("\"wordWrap\": true"), "file format is the same as v0.2");
    window.wait_until(cx, "window settings change too", |shell, cx| shell.read(cx).workbench().read(cx).settings().editor.word_wrap).await;
    assert_eq!(window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.find("st-editor.wordWrap").checked()
    }), Some(true));

    click(&window, cx, "st-editor.wordWrap");
    settle(&window, &page, cx).await;
    assert_eq!(on_disk(&app)["editor"]["wordWrap"], json!(false), "setting restored");
    window.wait_until(cx, "window settings revert", |shell, cx| !shell.read(cx).workbench().read(cx).settings().editor.word_wrap).await;
    // Without a `.oncode/settings.json` in the project, there must be no lingering "overridden by project" marker.
    assert!(!exists(&window, cx, "st-editor.wordWrap-override"));
    assert_eq!(Settings::load(app.core.settings_path()), Settings::default());
}

#[gpui_kit::test]
async fn text_input_is_debounced_and_saved_once_with_the_last_value(cx: &mut TestAppContext) {
    let app = setup(cx);
    let window = app.open(None, cx).await;
    let page = open_settings(&window, cx);

    retype(&window, cx, "st-editor.fontFamily", "  Fira Code, monospace ");
    assert_eq!(on_disk(&app), Value::Null, "not saved while the delay is running");
    assert!(!cx.update(|cx| page.read(cx).is_settled()));
    cx.executor().advance_clock(TEXT_DEBOUNCE - Duration::from_millis(1));
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| page.read(cx).user_settings().editor.font_family.clone()), Settings::default().editor.font_family, "delay has not elapsed");

    cx.executor().advance_clock(Duration::from_millis(1));
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| page.read(cx).user_settings().editor.font_family.clone()), "Fira Code, monospace", "edge whitespace is trimmed");
    settle(&window, &page, cx).await;
    assert_eq!(on_disk(&app)["editor"]["fontFamily"], json!("Fira Code, monospace"));
    assert_eq!(effective(&window, cx).editor.font_family, "Fira Code, monospace");

    // Enter saves without waiting for the delay.
    search(&window, cx, "terminal.shell");
    retype(&window, cx, "st-terminal.shell", "/bin/zsh");
    assert_eq!(cx.update(|cx| page.read(cx).user_settings().terminal.shell.clone()), None);
    window.update(cx, |window, cx| window.press("enter", cx));
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| page.read(cx).user_settings().terminal.shell.clone()).as_deref(), Some("/bin/zsh"));
    settle(&window, &page, cx).await;
    assert_eq!(on_disk(&app)["terminal"]["shell"], json!("/bin/zsh"));

    // Emptying a field that may be empty stores null; losing focus saves without waiting for the delay.
    retype(&window, cx, "st-terminal.shell", "");
    click(&window, cx, "settings-search");
    assert_eq!(cx.update(|cx| page.read(cx).user_settings().terminal.shell.clone()), None);
    settle(&window, &page, cx).await;
    assert_eq!(on_disk(&app)["terminal"]["shell"], Value::Null);
}

#[gpui_kit::test]
async fn invalid_input_is_shown_and_never_saved(cx: &mut TestAppContext) {
    let app = setup(cx);
    let window = app.open(None, cx).await;
    let page = open_settings(&window, cx);
    let problem = |cx: &mut TestAppContext, id: &str| cx.update(|cx| page.read(cx).problem(id));
    let shown = |cx: &mut TestAppContext, id: &str| cx.update(|cx| page.read(cx).field_input(id, cx)).unwrap();

    // Text: an empty font list is rejected, the input stays visible so it can be fixed.
    retype(&window, cx, "st-editor.fontFamily", "   ");
    assert!(problem(cx, "editor.fontFamily").unwrap().contains("at least one font"));
    assert!(exists(&window, cx, "st-editor.fontFamily-error"));
    settle(&window, &page, cx).await;
    click(&window, cx, "settings-search");
    assert_eq!(on_disk(&app), Value::Null, "an invalid value is never saved");
    assert_eq!(shown(cx, "editor.fontFamily"), "   ");
    assert!(exists(&window, cx, "st-editor.fontFamily-error"), "the message remains after focus is lost");

    // Once fixed, the message disappears and the value is saved.
    retype(&window, cx, "st-editor.fontFamily", "monospace");
    assert_eq!(problem(cx, "editor.fontFamily"), None);
    settle(&window, &page, cx).await;
    assert!(!exists(&window, cx, "st-editor.fontFamily-error"));
    assert_eq!(on_disk(&app)["editor"]["fontFamily"], json!("monospace"));

    // Numbers: non-digits cannot be typed at all.
    retype(&window, cx, "st-editor.tabSize", "x-.");
    assert_eq!(shown(cx, "editor.tabSize"), "4");
    assert_eq!(problem(cx, "editor.tabSize"), None);

    // A required field that is emptied: a message, not saved, also after focus is lost.
    retype(&window, cx, "st-editor.tabSize", "");
    assert!(problem(cx, "editor.tabSize").unwrap().contains("Enter a number"));
    click(&window, cx, "settings-search");
    settle(&window, &page, cx).await;
    assert!(problem(cx, "editor.tabSize").unwrap().contains("Enter a number"));
    assert_eq!(on_disk(&app)["editor"]["tabSize"], json!(4));

    // Out of range: a message while typing, and not saved even when the delay elapses…
    retype(&window, cx, "st-editor.tabSize", "99");
    assert!(problem(cx, "editor.tabSize").unwrap().contains("from 1 to 16"));
    assert!(exists(&window, cx, "st-editor.tabSize-error"));
    settle(&window, &page, cx).await;
    assert_eq!(on_disk(&app)["editor"]["tabSize"], json!(4));
    assert_eq!(effective(&window, cx).editor.tab_size, 4);
    // …then clamped to the nearest limit when focus is lost, and that valid value is saved.
    click(&window, cx, "settings-search");
    settle(&window, &page, cx).await;
    assert_eq!(shown(cx, "editor.tabSize"), "16");
    assert_eq!(problem(cx, "editor.tabSize"), None);
    assert!(!exists(&window, cx, "st-editor.tabSize-error"));
    assert_eq!(on_disk(&app)["editor"]["tabSize"], json!(16));

    // A valid in-range value is saved as is; a leading zero is normalized when focus is lost.
    retype(&window, cx, "st-editor.tabSize", "02");
    click(&window, cx, "settings-search");
    settle(&window, &page, cx).await;
    assert_eq!(shown(cx, "editor.tabSize"), "2");
    assert_eq!(on_disk(&app)["editor"]["tabSize"], json!(2));
    window.wait_until(cx, "window settings", |shell, cx| shell.read(cx).workbench().read(cx).settings().editor.tab_size == 2).await;
}

#[gpui_kit::test]
async fn number_steppers_and_optional_number(cx: &mut TestAppContext) {
    let app = setup(cx);
    let window = app.open(None, cx).await;
    let page = open_settings(&window, cx);

    // The + and - buttons of NumberInput.
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.within("st-editor.fontSize").click("increment", cx);
    });
    cx.run_until_parked();
    settle(&window, &page, cx).await;
    assert_eq!(on_disk(&app)["editor"]["fontSize"], json!(15));
    window.update(cx, |window, cx| {
        window.within("st-editor.fontSize").click("decrement", cx);
        window.within("st-editor.fontSize").click("decrement", cx);
    });
    cx.run_until_parked();
    settle(&window, &page, cx).await;
    assert_eq!(on_disk(&app)["editor"]["fontSize"], json!(13));
    window.wait_until(cx, "window settings", |shell, cx| shell.read(cx).workbench().read(cx).settings().editor.font_size == 13).await;

    // The terminal font size may be empty (null = follow the editor).
    search(&window, cx, "terminal.fontSize");
    retype(&window, cx, "st-terminal.fontSize", "12");
    settle(&window, &page, cx).await;
    assert_eq!(on_disk(&app)["terminal"]["fontSize"], json!(12));
    retype(&window, cx, "st-terminal.fontSize", "");
    assert_eq!(cx.update(|cx| page.read(cx).problem("terminal.fontSize")), None);
    settle(&window, &page, cx).await;
    assert_eq!(on_disk(&app)["terminal"]["fontSize"], Value::Null);
}

#[gpui_kit::test]
async fn select_persists_and_reopening_shows_persisted_values(cx: &mut TestAppContext) {
    let app = setup(cx);
    let window = app.open(None, cx).await;
    let page = open_settings(&window, cx);

    search(&window, cx, "git.pullStrategy");
    assert_eq!(window.update(cx, |window, _| window.find("st-git.pullStrategy").value().map(str::to_owned)).as_deref(), Some("Fast-forward only"));
    window.update(cx, |window, cx| {
        window.within("st-git.pullStrategy").click("input", cx);
        window.press("down", cx);
        window.press("down", cx);
        window.press("enter", cx);
    });
    window.wait_until(cx, "choice saved", |_, cx| page.read(cx).user_settings().git.pull_strategy == oncode_git::PullStrategy::Rebase).await;
    settle(&window, &page, cx).await;
    assert_eq!(on_disk(&app)["git"]["pullStrategy"], json!("rebase"));
    assert_eq!(effective(&window, cx).git.pull_strategy, oncode_git::PullStrategy::Rebase);

    search(&window, cx, "git.autoFetchMinutes");
    retype(&window, cx, "st-git.autoFetchMinutes", "30");
    search(&window, cx, "forge.githubClientId");
    retype(&window, cx, "st-forge.githubClientId", "Iv1.0123456789abcdef");
    search(&window, cx, "workbench.theme");
    window.update(cx, |window, cx| {
        window.within("st-workbench.theme").click("input", cx);
        window.press("down", cx);
        window.press("enter", cx);
    });
    window.wait_until(cx, "theme saved", |_, cx| page.read(cx).user_settings().workbench.theme == Theme::Dark).await;
    settle(&window, &page, cx).await;
    drop(page);

    // Close and open again: the controls show the stored values.
    assert!(window.run_command(cx, "workbench.closeTab"));
    window.wait_until(cx, "tab closed", |shell, cx| shell.read(cx).workbench().read(cx).model().all_tabs().next().is_none()).await;
    let page = open_settings(&window, cx);
    let shown = |cx: &mut TestAppContext, id: &str| cx.update(|cx| page.read(cx).field_input(id, cx)).unwrap();
    assert_eq!(shown(cx, "git.autoFetchMinutes"), "30");
    assert_eq!(shown(cx, "forge.githubClientId"), "Iv1.0123456789abcdef");
    assert_eq!(window.update(cx, |window, _| window.find("st-workbench.theme").value().map(str::to_owned)).as_deref(), Some("Dark"));
    search(&window, cx, "git.pullStrategy");
    assert_eq!(window.update(cx, |window, _| window.find("st-git.pullStrategy").value().map(str::to_owned)).as_deref(), Some("Rebase"));

    let mut expected = Settings::default();
    expected.git.pull_strategy = oncode_git::PullStrategy::Rebase;
    expected.git.auto_fetch_minutes = 30;
    expected.forge.github_client_id = Some("Iv1.0123456789abcdef".into());
    expected.workbench.theme = Theme::Dark;
    assert_eq!(Settings::load(app.core.settings_path()), expected);
    assert_eq!(effective(&window, cx), expected);
}

#[gpui_kit::test]
async fn pending_edit_is_saved_when_the_tab_closes(cx: &mut TestAppContext) {
    let app = setup(cx);
    let window = app.open(None, cx).await;
    let page = open_settings(&window, cx);

    retype(&window, cx, "st-editor.tabSize", "3");
    assert!(!cx.update(|cx| page.read(cx).is_settled()));
    drop(page);
    assert!(window.run_command(cx, "workbench.closeTab"));
    let path = app.core.settings_path().to_path_buf();
    window.wait_until(cx, "pending edit is saved too", |_, _| Settings::load(&path).editor.tab_size == 3).await;
    window.wait_until(cx, "window settings", |shell, cx| shell.read(cx).workbench().read(cx).settings().editor.tab_size == 3).await;
}

#[gpui_kit::test]
async fn external_change_updates_controls_but_not_the_field_being_typed(cx: &mut TestAppContext) {
    let app = setup(cx);
    let window = app.open(None, cx).await;
    let page = open_settings(&window, cx);
    let shown = |cx: &mut TestAppContext, id: &str| cx.update(|cx| page.read(cx).field_input(id, cx)).unwrap();

    retype(&window, cx, "st-editor.fontFamily", "Fira Co");

    // Another window changes the theme, tab size, a switch, AND the font being typed here.
    let mut other = Settings::default();
    other.workbench.theme = Theme::Light;
    other.editor.tab_size = 8;
    other.editor.line_numbers = false;
    other.editor.font_family = "Other Window Font".into();
    save_externally(&app, other.clone());
    window.wait_until(cx, "external change received", |_, cx| page.read(cx).user_settings().editor.tab_size == 8).await;
    window.update(cx, |window, cx| window.render_frame(cx));

    assert_eq!(shown(cx, "editor.tabSize"), "8");
    assert_eq!(window.update(cx, |window, _| window.find("st-editor.lineNumbers").checked()), Some(false));
    assert_eq!(window.update(cx, |window, _| window.find("st-workbench.theme").value().map(str::to_owned)).as_deref(), Some("Light"));
    assert_eq!(shown(cx, "editor.fontFamily"), "Fira Co", "the field being typed is not overwritten");

    // Typing continues; what is saved contains the external changes plus the field being typed.
    window.update(cx, |window, cx| window.input("de", cx));
    cx.run_until_parked();
    settle(&window, &page, cx).await;
    other.editor.font_family = "Fira Code".into();
    assert_eq!(Settings::load(app.core.settings_path()), other);
    assert_eq!(shown(cx, "editor.fontFamily"), "Fira Code");

    // A field that is not being edited follows the external value right away.
    other.editor.font_family = "Yet Another".into();
    other.editor.font_size = 19;
    click(&window, cx, "settings-search");
    save_externally(&app, other.clone());
    window.wait_until(cx, "second external change", |_, cx| page.read(cx).user_settings().editor.font_size == 19).await;
    assert_eq!(shown(cx, "editor.fontFamily"), "Yet Another");
    assert_eq!(shown(cx, "editor.fontSize"), "19");
    assert_eq!(Settings::load(app.core.settings_path()), other, "the page does not re-save external changes");
}

#[gpui_kit::test]
async fn search_box_hides_rows_and_sections_that_do_not_match(cx: &mut TestAppContext) {
    let app = setup(cx);
    let window = app.open(None, cx).await;
    let page = open_settings(&window, cx);
    window.wait_until(cx, "server table loaded", |_, cx| page.read(cx).servers().is_some()).await;
    let visible = |cx: &mut TestAppContext, ids: &[&'static str]| ids.iter().map(|id| exists(&window, cx, id)).collect::<Vec<_>>();

    assert_eq!(visible(cx, &["section-editor", "section-files", "section-appearance", "section-git", "section-terminal", "section-lsp", "section-accounts", "section-ai", "section-extensions", "section-keyboard"]), [true; 10]);
    assert!(!exists(&window, cx, "settings-no-match"));

    search(&window, cx, "font size");
    assert_eq!(visible(cx, &["row-editor.fontSize", "row-terminal.fontSize", "row-editor.tabSize", "row-editor.fontFamily"]), [true, true, false, false]);
    assert_eq!(visible(cx, &["section-editor", "section-terminal", "section-git", "section-lsp", "section-keyboard"]), [true, true, false, false, false]);

    search(&window, cx, "rebase");
    assert_eq!(visible(cx, &["row-git.pullStrategy", "row-git.path", "section-editor"]), [true, false, false]);

    // Server table rows are filtered by language/command too.
    search(&window, cx, "rust-analyzer");
    assert_eq!(visible(cx, &["row-lsp.servers", "server-rust", "server-go", "row-lsp.enabled"]), [true, true, false, false]);
    search(&window, cx, "language servers");
    assert_eq!(visible(cx, &["row-lsp.enabled", "row-lsp.servers", "server-rust", "server-go"]), [true; 4]);

    search(&window, cx, "keyboard shortcuts");
    assert_eq!(visible(cx, &["row-keybindings", "section-keyboard", "section-editor", "section-lsp"]), [true, true, false, false]);

    search(&window, cx, "zzz-nothing");
    assert_eq!(visible(cx, &["settings-no-match", "section-editor", "section-keyboard"]), [true, false, false]);

    search(&window, cx, "");
    assert_eq!(visible(cx, &["settings-no-match", "section-editor", "section-keyboard", "row-editor.tabSize"]), [false, true, true, true]);
    let _ = app;
}

#[gpui_kit::test]
async fn project_override_marker_follows_effective_settings(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("a.txt", "a\n")]);
    write_project_settings(&project, r#"{ "editor": { "tabSize": 2 }, "git": { "pullStrategy": "rebase" }, "lsp": { "servers": { "rust": { "command": "ra-project" } } } }"#);

    // Restricted mode: project settings are ignored, so there is no marker.
    let window = app.open(Some(&project), cx).await;
    let page = open_settings(&window, cx);
    assert!(!exists(&window, cx, "st-editor.tabSize-override"));
    drop(page);

    app.trust(&project);
    window.update(cx, |window, _| window.remove_window());
    cx.run_until_parked();
    let window = app.open(Some(&project), cx).await;
    assert_eq!(effective(&window, cx).editor.tab_size, 2);
    let page = open_settings(&window, cx);
    window.wait_until(cx, "server table loaded", |_, cx| page.read(cx).servers().is_some()).await;

    assert!(exists(&window, cx, "st-editor.tabSize-override"));
    assert!(!exists(&window, cx, "st-editor.fontSize-override"));
    // The control still shows the user's value, not the project's effective value.
    assert_eq!(cx.update(|cx| page.read(cx).field_input("editor.tabSize", cx)).as_deref(), Some("4"));
    search(&window, cx, "git.pullStrategy");
    assert!(exists(&window, cx, "st-git.pullStrategy-override"));
    assert_eq!(window.update(cx, |window, _| window.find("st-git.pullStrategy").value().map(str::to_owned)).as_deref(), Some("Fast-forward only"));
    // A project cannot choose language servers, so its `rust` server is not in effect.
    assert!(effective(&window, cx).lsp.servers.is_empty());
    let rust = cx.update(|cx| page.read(cx).servers().and_then(|servers| servers.iter().find(|s| s.language == "rust").cloned())).expect("rust row");
    assert_ne!(rust.command, "ra-project");

    // The marker disappears when the user's value is made equal to the project's value.
    search(&window, cx, "editor.tabSize");
    retype(&window, cx, "st-editor.tabSize", "2");
    settle(&window, &page, cx).await;
    assert_eq!(on_disk(&app)["editor"]["tabSize"], json!(2));
    window.wait_until(cx, "marker disappears", |_, _| true).await;
    assert!(!exists(&window, cx, "st-editor.tabSize-override"));
    // The project's own settings are never written from this page.
    assert!(std::fs::read_to_string(project.join(".oncode/settings.json")).unwrap().contains("\"tabSize\": 2 }"));
}

#[gpui_kit::test]
async fn reset_per_field_and_reset_all_with_confirmation(cx: &mut TestAppContext) {
    let app = setup(cx);
    let mut changed = Settings::default();
    changed.editor.font_size = 18;
    changed.editor.word_wrap = true;
    changed.workbench.theme = Theme::Dark;
    changed.git.path = Some("/opt/git".into());
    changed.lsp.servers.insert("rust".into(), oncode_lsp::ServerSpec { command: "ra-custom".into(), ..Default::default() });
    app.core.set_user_settings(changed.clone()).unwrap();
    let window = app.open(None, cx).await;
    let page = open_settings(&window, cx);

    // The reset button only exists on fields that differ from the default.
    assert!(exists(&window, cx, "st-editor.fontSize-reset"));
    assert!(exists(&window, cx, "st-editor.wordWrap-reset"));
    assert!(!exists(&window, cx, "st-editor.tabSize-reset"));

    // The button appears as soon as the value is changed; reset discards invalid input currently displayed.
    retype(&window, cx, "st-editor.tabSize", "8");
    settle(&window, &page, cx).await;
    assert!(exists(&window, cx, "st-editor.tabSize-reset"));
    retype(&window, cx, "st-editor.tabSize", "");
    assert!(exists(&window, cx, "st-editor.tabSize-error"));
    click(&window, cx, "st-editor.tabSize-reset");
    settle(&window, &page, cx).await;
    assert_eq!(cx.update(|cx| page.read(cx).field_input("editor.tabSize", cx)).as_deref(), Some("4"));
    assert!(!exists(&window, cx, "st-editor.tabSize-error"));
    assert!(!exists(&window, cx, "st-editor.tabSize-reset"));

    // Resetting one field only restores that field.
    click(&window, cx, "st-editor.fontSize-reset");
    settle(&window, &page, cx).await;
    let mut expected = changed.clone();
    expected.editor.font_size = 14;
    assert_eq!(Settings::load(app.core.settings_path()), expected);
    assert_eq!(cx.update(|cx| page.read(cx).field_input("editor.fontSize", cx)).as_deref(), Some("14"));
    assert!(!exists(&window, cx, "st-editor.fontSize-reset"));
    assert!(exists(&window, cx, "st-editor.wordWrap-reset"));
    click(&window, cx, "st-editor.wordWrap-reset");
    settle(&window, &page, cx).await;
    expected.editor.word_wrap = false;
    assert_eq!(Settings::load(app.core.settings_path()), expected);
    assert_eq!(window.update(cx, |window, _| window.find("st-editor.wordWrap").checked()), Some(false));

    // Reset to defaults asks for confirmation; canceling changes nothing.
    click(&window, cx, "settings-reset-all");
    assert!(exists(&window, cx, "ok"), "confirmation dialog open");
    click(&window, cx, "cancel");
    settle(&window, &page, cx).await;
    assert_eq!(Settings::load(app.core.settings_path()), expected);

    // After confirming: exactly the defaults, including server overrides, and all controls follow.
    retype(&window, cx, "st-editor.tabSize", "");
    click(&window, cx, "settings-reset-all");
    click(&window, cx, "ok");
    window.wait_until(cx, "back to defaults", |_, cx| page.read(cx).user_settings() == &Settings::default()).await;
    settle(&window, &page, cx).await;
    assert_eq!(Settings::load(app.core.settings_path()), Settings::default());
    assert_eq!(effective(&window, cx), Settings::default());
    assert_eq!(cx.update(|cx| page.read(cx).field_input("editor.tabSize", cx)).as_deref(), Some("4"));
    assert_eq!(cx.update(|cx| page.read(cx).field_input("git.path", cx)).as_deref(), Some(""));
    assert!(!exists(&window, cx, "st-editor.tabSize-error"));
    assert_eq!(window.update(cx, |window, _| window.find("st-workbench.theme").value().map(str::to_owned)).as_deref(), Some("System"));

    // Everything is already default: the button is disabled, clicking it doesn't open a dialog.
    click(&window, cx, "settings-reset-all");
    assert!(!exists(&window, cx, "ok"));
}

#[gpui_kit::test]
async fn language_server_override_customize_edit_and_reset(cx: &mut TestAppContext) {
    let app = setup(cx);
    let window = app.open(None, cx).await;
    let page = open_settings(&window, cx);
    window.wait_until(cx, "server table loaded", |_, cx| page.read(cx).servers().is_some()).await;
    let builtin = cx.update(|cx| page.read(cx).servers().unwrap().iter().find(|info| info.language == "typescript").cloned()).expect("built-in typescript server");
    assert!(!builtin.install_hint.is_empty());

    // Customize creates an override from the built-in server.
    search(&window, cx, "typescript");
    assert!(!exists(&window, cx, "server-typescript-command"));
    click(&window, cx, "server-typescript-customize");
    settle(&window, &page, cx).await;
    assert_eq!(on_disk(&app)["lsp"]["servers"], json!({ "typescript": { "command": builtin.command, "args": builtin.args, "env": [] } }));
    assert!(exists(&window, cx, "server-typescript-command"));
    assert!(!exists(&window, cx, "server-typescript-customize"));

    // The command and arguments are stored in lsp.servers[language]; quoted arguments are split correctly.
    retype(&window, cx, "server-typescript-command", "/opt/node/bin/tsls");
    settle(&window, &page, cx).await;
    retype(&window, cx, "server-typescript-args", r#"--stdio --log "/tmp/my logs/ts.log""#);
    settle(&window, &page, cx).await;
    assert_eq!(on_disk(&app)["lsp"]["servers"]["typescript"], json!({ "command": "/opt/node/bin/tsls", "args": ["--stdio", "--log", "/tmp/my logs/ts.log"], "env": [] }));
    // The table is reloaded and shows the server now in effect.
    window.wait_until(cx, "table loads the override", |_, cx| page.read(cx).servers().is_some_and(|list| list.iter().any(|info| info.command == "/opt/node/bin/tsls" && !info.installed))).await;

    // Invalid: an unclosed quote or an empty command; nothing is saved.
    retype(&window, cx, "server-typescript-args", r#"--log "unclosed"#);
    assert!(exists(&window, cx, "server-typescript-args-error"));
    retype(&window, cx, "server-typescript-command", "  ");
    assert!(exists(&window, cx, "server-typescript-command-error"));
    settle(&window, &page, cx).await;
    assert_eq!(on_disk(&app)["lsp"]["servers"]["typescript"]["command"], json!("/opt/node/bin/tsls"));
    assert_eq!(on_disk(&app)["lsp"]["servers"]["typescript"]["args"], json!(["--stdio", "--log", "/tmp/my logs/ts.log"]));

    // Reset removes the override and goes back to the built-in server.
    click(&window, cx, "server-typescript-reset");
    settle(&window, &page, cx).await;
    assert_eq!(Settings::load(app.core.settings_path()), Settings::default());
    assert!(exists(&window, cx, "server-typescript-customize"));
    assert!(!exists(&window, cx, "server-typescript-command"));
}

#[gpui_kit::test]
async fn existing_override_keeps_env_when_the_command_changes(cx: &mut TestAppContext) {
    let app = setup(cx);
    let mut user = Settings::default();
    user.lsp.servers.insert("rust".into(), oncode_lsp::ServerSpec { command: "ra".into(), args: vec!["--x".into()], env: vec![("RA_LOG".into(), "info".into())] });
    app.core.set_user_settings(user).unwrap();
    let window = app.open(None, cx).await;
    let page = open_settings(&window, cx);
    window.wait_until(cx, "server table loaded", |_, cx| page.read(cx).servers().is_some()).await;

    search(&window, cx, "rust");
    retype(&window, cx, "server-rust-command", "ra-nightly");
    settle(&window, &page, cx).await;
    assert_eq!(on_disk(&app)["lsp"]["servers"]["rust"], json!({ "command": "ra-nightly", "args": ["--x"], "env": [["RA_LOG", "info"]] }));
}

#[gpui_kit::test]
async fn links_open_settings_json_and_keybindings_json(cx: &mut TestAppContext) {
    let app = setup(cx);
    let window = app.open(None, cx).await;
    let _page = open_settings(&window, cx);
    let settings_path = app.core.settings_path().to_string_lossy().into_owned();
    let keybindings_path = app.core.keybindings_path().to_string_lossy().into_owned();
    assert!(!app.core.settings_path().exists() && !app.core.keybindings_path().exists());

    // Open settings.json: a file that doesn't exist is created first, then opened in the editor.
    click(&window, cx, "settings-open-json");
    let id = format!("file:{settings_path}");
    window.wait_until(cx, "settings.json open", |shell, cx| shell.read(cx).workbench().read(cx).model().all_tabs().any(|tab| tab.id == id)).await;
    assert_eq!(Settings::load(app.core.settings_path()), Settings::default());
    assert!(app.core.settings_path().exists());

    // Keyboard Shortcuts: the button runs the framework's keybindings.openJson command.
    assert!(window.run_command(cx, "settings.open"));
    cx.run_until_parked();
    search(&window, cx, "keyboard shortcuts");
    click(&window, cx, "settings-open-keybindings");
    let id = format!("file:{keybindings_path}");
    window.wait_until(cx, "keybindings.json open", |shell, cx| shell.read(cx).workbench().read(cx).model().all_tabs().any(|tab| tab.id == id)).await;
    assert!(std::fs::read_to_string(app.core.keybindings_path()).unwrap().starts_with("// User keyboard shortcuts."));

    // The settings.openJson command is also available from the palette, without a folder.
    assert!(window.run_command(cx, "workbench.closeAllTabs"));
    window.wait_until(cx, "all tabs closed", |shell, cx| shell.read(cx).workbench().read(cx).model().all_tabs().next().is_none()).await;
    assert!(window.run_command(cx, "settings.openJson"));
    let id = format!("file:{settings_path}");
    window.wait_until(cx, "settings.json open via command", |shell, cx| shell.read(cx).workbench().read(cx).model().all_tabs().any(|tab| tab.id == id)).await;
    // Shortcuts overridden through keybindings.json also apply to the settings commands.
    std::fs::write(app.core.keybindings_path(), r#"{ "settings.open": "Mod+Alt+S" }"#).unwrap();
    assert!(window.run_command(cx, "keybindings.reload"));
    for _ in 0..100 {
        let applied = window.update(cx, |window, cx| oncode_ui::commands::get("settings.open", window, cx).is_some_and(|c| oncode_ui::commands::keybindings_for(&c, cx) == ["Mod+Alt+S".to_string()]));
        if applied {
            break;
        }
        window.wait_until(cx, "backend", |_, _| true).await;
        std::thread::sleep(Duration::from_millis(10));
    }
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press("secondary-,", cx);
    });
    cx.run_until_parked();
    assert!(!tab_ids(&window, cx).contains(&"settings".to_string()), "the default shortcut no longer applies");
    window.update(cx, |window, cx| window.press("secondary-alt-s", cx));
    cx.run_until_parked();
    assert!(tab_ids(&window, cx).contains(&"settings".to_string()), "the user's shortcut opens settings");
}

#[gpui_kit::test]
async fn git_error_is_shown_and_the_path_can_be_fixed_without_a_folder(cx: &mut TestAppContext) {
    let app = setup(cx);
    // A wrong git path stored from a previous session; git has not been looked up yet in this process.
    let mut broken = Settings::default();
    broken.git.path = Some(app.dirs.path().join("does-not-exist/git").to_string_lossy().into_owned());
    app.core.set_user_settings(broken.clone()).unwrap();

    let window = app.open(None, cx).await;
    let page = open_settings(&window, cx);
    window.wait_until(cx, "app info loaded", |_, cx| page.read(cx).app_info().is_some()).await;
    let info = cx.update(|cx| page.read(cx).app_info().cloned()).unwrap();
    assert!(info.git.is_none(), "git is not found at the wrong path");
    assert!(info.git_error.as_deref().is_some_and(|error| !error.is_empty()));

    search(&window, cx, "git.path");
    assert!(exists(&window, cx, "st-git-status"), "the reason Git is unavailable is shown in the path row");
    assert_eq!(cx.update(|cx| page.read(cx).field_input("git.path", cx)), broken.git.path);

    // Emptying the path fixes it (takes effect after the app restarts).
    retype(&window, cx, "st-git.path", "");
    settle(&window, &page, cx).await;
    assert_eq!(on_disk(&app)["git"]["path"], Value::Null);
    assert_eq!(Settings::load(app.core.settings_path()), Settings::default());
}

#[gpui_kit::test]
async fn git_version_is_shown_when_git_is_found(cx: &mut TestAppContext) {
    let app = setup(cx);
    let window = app.open(None, cx).await;
    let page = open_settings(&window, cx);
    window.wait_until(cx, "app info loaded", |_, cx| page.read(cx).app_info().is_some()).await;
    let info = cx.update(|cx| page.read(cx).app_info().cloned()).unwrap();
    assert!(info.git.is_some(), "git is available on the test machine: {:?}", info.git_error);
    search(&window, cx, "git.path");
    assert!(exists(&window, cx, "st-git-status"));
    let _ = app;
}

struct FakeAccounts;

impl Render for FakeAccounts {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::base::TestSupportExt as _;
        div().id("fake-accounts").child("Signed in as octocat").test_support()
    }
}

#[gpui_kit::test]
async fn accounts_section_runs_the_sign_in_command_or_embeds_a_registered_view(cx: &mut TestAppContext) {
    let app = setup(cx);
    let runs = Rc::new(Cell::new(0));
    // Stand-in for the forge feature: only its sign-in command.
    cx.update(|cx| {
        let runs = runs.clone();
        oncode_ui::register_feature(cx, move |workbench, _, _| {
            let runs = runs.clone();
            workbench.register_command(Command::new("forge.signIn", "Sign In to GitHub or GitLab…", move |_, _, _| runs.set(runs.get() + 1)).category("Accounts"));
        });
    });
    let window = app.open(None, cx).await;
    let _page = open_settings(&window, cx);

    // Without a registered view: the Client ID fields and a button that runs forge.signIn.
    search(&window, cx, "accounts");
    assert!(exists(&window, cx, "st-forge.githubClientId"));
    assert!(exists(&window, cx, "st-forge.gitlabClientId"));
    assert!(!exists(&window, cx, "fake-accounts"));
    click(&window, cx, "settings-sign-in");
    assert_eq!(runs.get(), 1);

    // With a registered view: that view is inserted and the stand-in button disappears.
    cx.update(|cx| {
        oncode_ui_settings::register_accounts_view(cx, |env, _, cx| {
            // The builder may read `Workbench`: it is called after the tab has finished building.
            let _ = env.workbench.upgrade().map(|workbench| workbench.read(cx).settings().clone());
            cx.new(|_| FakeAccounts).into()
        })
    });
    assert!(window.run_command(cx, "workbench.closeTab"));
    window.wait_until(cx, "tab closed", |shell, cx| shell.read(cx).workbench().read(cx).model().all_tabs().next().is_none()).await;
    let _page = open_settings(&window, cx);
    search(&window, cx, "accounts");
    assert!(exists(&window, cx, "fake-accounts"));
    assert!(!exists(&window, cx, "settings-sign-in"));
    assert!(exists(&window, cx, "st-forge.githubClientId"), "the Client ID fields are still there");
    let _ = app;
}

#[gpui_kit::test]
async fn sign_in_button_is_absent_when_the_forge_command_is_not_registered(cx: &mut TestAppContext) {
    let app = setup(cx);
    let window = app.open(None, cx).await;
    let _page = open_settings(&window, cx);
    search(&window, cx, "accounts");
    assert!(exists(&window, cx, "st-forge.githubClientId"));
    assert!(!exists(&window, cx, "settings-sign-in"));
    assert!(!exists(&window, cx, "row-accounts"));
    let _ = app;
}
