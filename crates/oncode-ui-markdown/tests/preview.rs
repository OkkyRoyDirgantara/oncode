//! Uji ekstensi Markdown Preview: membuka pratinjau di samping, ikut berubah saat disunting, dan
//! bisa dimatikan lewat pengaturan.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use oncode_ui::editor::{active_file, open_file, OpenFile};
use oncode_ui::testing::{TestApp, TestWindow};
use oncode_ui::ViewTab;
use oncode_ui_markdown::MarkdownPreview;

async fn open_readme(app: &TestApp, cx: &mut TestAppContext) -> (TestWindow, std::path::PathBuf) {
    let project = app.project(&[("README.md", "# Judul\n\nIsi awal.\n"), ("main.rs", "fn main() {}\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let path = project.join("README.md").to_string_lossy().into_owned();
    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&path), window, cx)));
    window.wait_until(cx, "README dimuat", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).is_ready())).await;
    (window, project)
}

/// Pratinjau di tab aktif.
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
        window.press("ctrl-shift-v", cx);
    });
    cx.run_until_parked();
    let preview = preview(&window, cx).expect("tab pratinjau aktif");
    window.wait_until(cx, "isi pratinjau", |_, cx| preview.read(cx).text() == "# Judul\n\nIsi awal.\n").await;
    assert_eq!(cx.update(|cx| workbench.read(cx).model().groups.len()), 2, "pratinjau dibuka di samping editor");

    // Suntingan yang belum disimpan langsung terlihat di pratinjau.
    window.update(cx, |window, cx| editor.update(cx, |state, cx| state.set_value("# Judul baru\n", window, cx)));
    cx.update(|cx| editor.update(cx, |_, cx| cx.emit(gpui_kit::component::input::InputEvent::Change)));
    window.wait_until(cx, "pratinjau mengikuti suntingan", |_, cx| preview.read(cx).text() == "# Judul baru\n").await;
}

#[gpui_kit::test]
async fn preview_is_only_offered_for_markdown_files_with_the_extension_on(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_markdown::init);
    let mut settings = app.core.user_settings();
    settings.extensions.markdown_preview = false;
    app.core.set_user_settings(settings).unwrap();
    let (window, project) = open_readme(&app, cx).await;
    assert!(!window.run_command(cx, "markdown.showPreviewToSide"), "ekstensi dimatikan");

    let mut settings = app.core.user_settings();
    settings.extensions.markdown_preview = true;
    app.core.set_user_settings(settings).unwrap();
    let workbench = window.workbench(cx);
    let path = project.join("main.rs").to_string_lossy().into_owned();
    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&path), window, cx)));
    window.wait_until(cx, "main.rs aktif", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).path().ends_with("main.rs"))).await;
    assert!(!window.run_command(cx, "markdown.showPreview"), "bukan berkas Markdown");
}
