//! Tests for the Markdown Preview extension: opening the preview to the side, following edits, and
//! being switchable off via settings.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use oncode_ui::editor::{active_file, open_file, OpenFile};
use oncode_ui::testing::{TestApp, TestWindow};
use oncode_ui::ViewTab;
use oncode_ui_markdown::MarkdownPreview;

async fn open_readme(app: &TestApp, cx: &mut TestAppContext) -> (TestWindow, std::path::PathBuf) {
    let project = app.project(&[("README.md", "# Title\n\nInitial content.\n"), ("main.rs", "fn main() {}\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let path = project.join("README.md").to_string_lossy().into_owned();
    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&path), window, cx)));
    window.wait_until(cx, "README loaded", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).is_ready())).await;
    (window, project)
}

/// The preview in the active tab.
fn preview(window: &TestWindow, cx: &mut TestAppContext) -> Option<gpui_kit::Entity<MarkdownPreview>> {
    let workbench = window.workbench(cx);
    cx.update(|cx| workbench.read(cx).active_content().and_then(|content| content.as_any().downcast_ref::<ViewTab<MarkdownPreview>>().map(|tab| tab.0.clone())))
}

#[gpui_kit::test]
async fn ctrl_shift_v_previews_beside_the_editor_and_follows_edits(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_markdown::init);
    let (window, _project) = open_readme(&app, cx).await;
    let workbench = window.workbench(cx);
    let (_, _, editor) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());

    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press("secondary-shift-v", cx);
    });
    cx.run_until_parked();
    let preview = preview(&window, cx).expect("preview tab is active");
    window.wait_until(cx, "preview content", |_, cx| preview.read(cx).text() == "# Title\n\nInitial content.\n").await;
    assert_eq!(cx.update(|cx| workbench.read(cx).model().groups.len()), 2, "preview opens beside the editor");

    // An unsaved edit shows up in the preview right away.
    window.update(cx, |window, cx| editor.update(cx, |state, cx| state.set_value("# New title\n", window, cx)));
    cx.update(|cx| editor.update(cx, |_, cx| cx.emit(gpui_kit::component::input::InputEvent::Change)));
    window.wait_until(cx, "preview follows the edit", |_, cx| preview.read(cx).text() == "# New title\n").await;
}

const README: &str = "# Title\n\nInitial content.\n";

/// Opens the preview beside the active README, then closes the README editor tab (answering the unsaved-changes dialog
/// with "Don't Save" if there is one). The preview keeps the buffer alive while no editor shows it.
async fn close_the_editor_but_keep_the_preview(window: &TestWindow, cx: &mut TestAppContext) -> gpui_kit::Entity<MarkdownPreview> {
    let workbench = window.workbench(cx);
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press("secondary-shift-v", cx);
    });
    cx.run_until_parked();
    let preview = preview(window, cx).expect("preview tab is active");
    window.wait_until(cx, "preview content", |_, cx| !preview.read(cx).text().is_empty()).await;

    assert!(window.run_command(cx, "markdown.showSource"));
    cx.run_until_parked();
    assert!(window.run_command(cx, "workbench.closeTab"));
    cx.run_until_parked();
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        if !window.find_all("choice-discard").is_empty() {
            window.click("choice-discard", cx);
        }
    });
    cx.run_until_parked();
    window.wait_until(cx, "README editor closed", |_, cx| !workbench.read(cx).model().all_tabs().any(|tab| tab.title == "README.md")).await;
    // As in the running app, frames are drawn after the tab is gone, which releases the closed editor's view.
    for _ in 0..5 {
        window.update(cx, |window, cx| window.render_frame(cx));
        cx.run_until_parked();
    }
    // The window keeps the last focused input (the closed editor's state) alive until another input takes focus,
    // e.g. when the user opens quick open or the command palette to open the file again.
    assert!(window.run_command(cx, "workbench.commandPalette"));
    cx.run_until_parked();
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press("escape", cx);
    });
    cx.run_until_parked();
    preview
}

/// Opens README.md again and returns its buffer and editor state once loaded.
async fn reopen_readme(window: &TestWindow, project: &std::path::Path, cx: &mut TestAppContext) -> (gpui_kit::Entity<oncode_ui::editor::Buffer>, gpui_kit::Entity<gpui_kit::component::input::EditorState>) {
    let workbench = window.workbench(cx);
    let path = project.join("README.md").to_string_lossy().into_owned();
    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&path), window, cx)));
    window.wait_until(cx, "README is open again", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).is_ready())).await;
    let (_, buffer, editor) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());
    (buffer, editor)
}

/// The preview holds the buffer, so it outlives the editor tab. Opening the file again must show the file,
/// not an empty editor (which a later save would write to disk).
#[gpui_kit::test]
async fn reopening_a_file_the_preview_keeps_alive_shows_its_content(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_markdown::init);
    let (window, project) = open_readme(&app, cx).await;
    let _preview = close_the_editor_but_keep_the_preview(&window, cx).await;

    let (buffer, editor) = reopen_readme(&window, &project, cx).await;
    cx.update(|cx| {
        assert_eq!(editor.read(cx).text().to_string(), README, "the editor shows the file, not an empty buffer");
        assert!(!buffer.read(cx).is_dirty());
    });
    assert_eq!(std::fs::read_to_string(project.join("README.md")).unwrap(), README);
}

/// Closing with "Don't Save" discards the changes even though the preview keeps the buffer alive: reopening the
/// file shows the saved content, and the buffer is not dirty (a dirty empty buffer would be auto-saved over the file).
#[gpui_kit::test]
async fn dont_save_discards_changes_even_when_the_preview_keeps_the_buffer(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_markdown::init);
    let (window, project) = open_readme(&app, cx).await;
    let workbench = window.workbench(cx);
    let (_, buffer, editor) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());
    window.update(cx, |window, cx| editor.update(cx, |state, cx| state.set_value("# Edited, not saved\n", window, cx)));
    cx.update(|cx| editor.update(cx, |_, cx| cx.emit(gpui_kit::component::input::InputEvent::Change)));
    window.wait_until(cx, "the buffer is dirty", |_, cx| buffer.read(cx).is_dirty()).await;

    let preview = close_the_editor_but_keep_the_preview(&window, cx).await;
    window.wait_until(cx, "preview follows the discard", |_, cx| preview.read(cx).text() == README).await;

    let (buffer, editor) = reopen_readme(&window, &project, cx).await;
    cx.update(|cx| {
        assert_eq!(editor.read(cx).text().to_string(), README, "the discarded changes are gone");
        assert!(!buffer.read(cx).is_dirty());
    });
    assert_eq!(std::fs::read_to_string(project.join("README.md")).unwrap(), README, "nothing was written to disk");
}

#[gpui_kit::test]
async fn preview_is_only_offered_for_markdown_files_with_the_extension_on(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_markdown::init);
    let mut settings = app.core.user_settings();
    settings.extensions.markdown_preview = false;
    app.core.set_user_settings(settings).unwrap();
    let (window, project) = open_readme(&app, cx).await;
    assert!(!window.run_command(cx, "markdown.showPreviewToSide"), "extension is turned off");

    let mut settings = app.core.user_settings();
    settings.extensions.markdown_preview = true;
    app.core.set_user_settings(settings).unwrap();
    let workbench = window.workbench(cx);
    let path = project.join("main.rs").to_string_lossy().into_owned();
    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&path), window, cx)));
    window.wait_until(cx, "main.rs is active", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).path().ends_with("main.rs"))).await;
    assert!(!window.run_command(cx, "markdown.showPreview"), "not a Markdown file");
}

/// The Markdown content is actually painted in the preview (not just stored in the view).
#[gpui_kit::test]
async fn preview_paints_the_rendered_markdown(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_markdown::init);
    let (window, _project) = open_readme(&app, cx).await;
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press("secondary-shift-v", cx);
    });
    cx.run_until_parked();
    let preview = preview(&window, cx).expect("preview tab is active");
    window.wait_until(cx, "preview content", |_, cx| !preview.read(cx).text().is_empty()).await;
    for _ in 0..20 {
        window.update(cx, |window, cx| window.render_frame(cx));
        cx.run_until_parked();
    }
    let painted: Vec<String> = window.update(cx, |window, _| gpui_kit::base::test_support::texts(window).iter().map(|t| t.text().to_string()).collect());
    println!("painted text: {painted:?}");
    assert!(painted.iter().any(|t| t.contains("Title")), "Markdown heading is painted");
    assert!(painted.iter().any(|t| t.contains("Initial content.")), "paragraph is painted");
    assert!(!painted.iter().any(|t| t.contains("# Title")), "the # marker is not shown raw");
}

/// Tab bar buttons: the preview icon on a Markdown file, and "Open Source" on the preview tab.
#[gpui_kit::test]
async fn tab_bar_buttons_open_the_preview_and_go_back_to_the_source(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_markdown::init);
    let (window, project) = open_readme(&app, cx).await;
    let workbench = window.workbench(cx);
    let click = |cx: &mut TestAppContext, id: &'static str| {
        window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        cx.run_until_parked();
    };

    click(cx, "editor-action-markdown-preview");
    let preview = preview(&window, cx).expect("the button opens the preview");
    window.wait_until(cx, "preview content", |_, cx| preview.read(cx).text().contains("Title")).await;
    assert_eq!(cx.update(|cx| workbench.read(cx).model().groups.len()), 2, "beside the editor");

    // The preview tab has a button back to its Markdown file (the tab that is already open).
    click(cx, "editor-action-markdown-source");
    let active = cx.update(|cx| active_file(workbench.read(cx), cx).map(|(_, buffer, _)| buffer.read(cx).path().to_string()));
    assert!(active.is_some_and(|path| path.ends_with("README.md")));
    assert_eq!(cx.update(|cx| workbench.read(cx).model().groups.len()), 2, "does not open a new group");

    // Files other than Markdown have no preview button.
    let path = project.join("main.rs").to_string_lossy().into_owned();
    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&path), window, cx)));
    window.wait_until(cx, "main.rs is active", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).path().ends_with("main.rs"))).await;
    let has_button = window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.find_all("editor-action-markdown-preview").len()
    });
    assert_eq!(has_button, 0, "main.rs replaces README as the active tab: no preview button");
}
