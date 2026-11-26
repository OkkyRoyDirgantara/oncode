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
