//! Image files: shown as a picture with its size and zoom; an SVG switches between its picture and its text.

use std::cell::Cell;
use std::io::Cursor;
use std::rc::Rc;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext};
use oncode_ui::editor::{active_file, active_image, buffers, file_mode, open_file, BuffersEvent, FileMode, ImageInfo, ImageView, OpenFile, Zoom};
use oncode_ui::testing::{TestApp, TestWindow};
use oncode_ui::Workbench;

const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="80"><rect width="120" height="80" fill="red"/></svg>"#;

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    let picture = image::RgbaImage::from_pixel(width, height, image::Rgba([0, 128, 255, 128]));
    picture.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
    bytes
}

fn open(window: &TestWindow, workbench: &Entity<Workbench>, file: OpenFile, cx: &mut TestAppContext) {
    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, file, window, cx)));
    cx.run_until_parked();
}

fn frames(window: &TestWindow, cx: &mut TestAppContext) {
    for _ in 0..3 {
        window.update(cx, |window, cx| window.render_frame(cx));
        cx.run_until_parked();
    }
}

/// The picture in the active tab, once it is decoded and laid out (the first frame measures the view).
async fn shown_image(window: &TestWindow, workbench: &Entity<Workbench>, cx: &mut TestAppContext) -> Entity<ImageView> {
    window.wait_until(cx, "picture decoded", |_, cx| active_image(workbench.read(cx), cx).is_some_and(|image| image.read(cx).info().is_some())).await;
    frames(window, cx);
    let image = cx.update(|cx| active_image(workbench.read(cx), cx).unwrap());
    assert!(cx.update(|cx| image.read(cx).scale().is_some()), "laid out");
    image
}

fn active_tab_mode(workbench: &Entity<Workbench>, cx: &mut TestAppContext) -> FileMode {
    cx.update(|cx| file_mode(workbench.read(cx).model().active().unwrap().1))
}

fn has(window: &TestWindow, id: &'static str, cx: &mut TestAppContext) -> bool {
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        !window.find_all(id).is_empty()
    })
}

fn click(window: &TestWindow, id: &'static str, cx: &mut TestAppContext) {
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    });
    cx.run_until_parked();
}

fn press(window: &TestWindow, key: &str, cx: &mut TestAppContext) {
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press(key, cx);
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
async fn a_png_shows_as_a_picture_with_its_size_and_zooms(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("README.md", "# Demo\n")]);
    let bytes = png(300, 200);
    std::fs::write(project.join("logo.png"), &bytes).unwrap();
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);

    open(&window, &workbench, OpenFile::new(project.join("logo.png").to_string_lossy()), cx);
    let image = shown_image(&window, &workbench, cx).await;
    assert_eq!(cx.update(|cx| image.read(cx).info()), Some(ImageInfo { width: 300, height: 200, bytes: bytes.len(), format: "PNG" }));
    assert_eq!(cx.update(|cx| (image.read(cx).zoom(), image.read(cx).scale())), (Zoom::Fit, Some(1.)), "a small picture is not enlarged");
    assert!(has(&window, "image-zoom", cx), "the zoom shows in the status bar");
    assert!(!has(&window, "editor-action-image-open-as-text", cx), "a PNG has no text to show");

    // Editing commands do not apply to a picture.
    assert!(!window.run_command(cx, "editor.find"));
    assert!(!window.run_command(cx, "editor.gotoLine"));

    // The picture has focus after opening, so the zoom keys apply.
    press(&window, "secondary-=", cx);
    assert_eq!(cx.update(|cx| image.read(cx).zoom()), Zoom::Scale(1.5));
    press(&window, "secondary--", cx);
    press(&window, "secondary--", cx);
    assert_eq!(cx.update(|cx| image.read(cx).zoom()), Zoom::Scale(0.9));
    press(&window, "secondary-0", cx);
    assert_eq!(cx.update(|cx| image.read(cx).zoom()), Zoom::Fit);

    // The zoom in the status bar switches between fitting the view and the actual size.
    click(&window, "image-zoom", cx);
    assert_eq!(cx.update(|cx| image.read(cx).zoom()), Zoom::Scale(1.));
    click(&window, "image-zoom", cx);
    assert_eq!(cx.update(|cx| image.read(cx).zoom()), Zoom::Fit);

    // A picture larger than the view is scaled down to fit.
    std::fs::write(project.join("wide.png"), png(4000, 1000)).unwrap();
    open(&window, &workbench, OpenFile::new(project.join("wide.png").to_string_lossy()), cx);
    let wide = shown_image(&window, &workbench, cx).await;
    let scale = cx.update(|cx| wide.read(cx).scale().unwrap());
    assert!(scale < 0.3 && scale > 0.1, "fits the view: {scale}");
    assert_eq!(cx.update(|cx| workbench.read(cx).model().all_tabs().count()), 2);
}

#[gpui_kit::test]
async fn an_svg_shows_as_a_picture_and_switches_to_text_and_back(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("icon.svg", SVG)]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let path = project.join("icon.svg").to_string_lossy().into_owned();
    // Language servers and change markers attach to an editor when it is created.
    let editors = Rc::new(Cell::new(0));
    let _subscription = cx.update(|cx| {
        let editors = editors.clone();
        cx.subscribe(&buffers(workbench.read(cx)).unwrap(), move |_, event: &BuffersEvent, _| {
            if matches!(event, BuffersEvent::EditorCreated { .. }) {
                editors.set(editors.get() + 1);
            }
        })
    });

    open(&window, &workbench, OpenFile::new(&path), cx);
    let image = shown_image(&window, &workbench, cx).await;
    assert_eq!(cx.update(|cx| image.read(cx).info()), Some(ImageInfo { width: 120, height: 80, bytes: SVG.len(), format: "SVG" }));
    assert_eq!(editors.get(), 0, "a picture starts no language server");
    assert!(has(&window, "editor-action-image-open-as-text", cx));
    assert!(!has(&window, "editor-action-image-open-as-image", cx));

    // "Open as Text" shows the source in the same tab.
    click(&window, "editor-action-image-open-as-text", cx);
    assert_eq!(active_tab_mode(&workbench, cx), FileMode::Text);
    let (_, buffer, state) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());
    assert!(cx.update(|cx| active_image(workbench.read(cx), cx).is_none()));
    assert_eq!(cx.update(|cx| state.read(cx).value().to_string()), SVG);
    assert_eq!(editors.get(), 1, "the text gets its language server");
    assert!(has(&window, "editor-action-image-open-as-image", cx), "and a button back to the picture");
    assert_eq!(cx.update(|cx| workbench.read(cx).model().all_tabs().count()), 1, "no second tab");

    // Opening the file again (e.g. from the Explorer) keeps showing the text.
    open(&window, &workbench, OpenFile::new(&path), cx);
    assert_eq!(active_tab_mode(&workbench, cx), FileMode::Text);

    // An unsaved edit shows in the picture.
    let edited = SVG.replace(r#"width="120""#, r#"width="240""#);
    window.update(cx, |window, cx| state.update(cx, |state, cx| state.set_value(edited.clone(), window, cx)));
    cx.update(|cx| state.update(cx, |_, cx| cx.emit(gpui_kit::component::input::InputEvent::Change)));
    cx.run_until_parked();
    assert!(cx.update(|cx| buffer.read(cx).is_dirty()));

    assert!(window.run_command(cx, "image.openAsImage"));
    cx.run_until_parked();
    assert_eq!(active_tab_mode(&workbench, cx), FileMode::Image);
    let image = shown_image(&window, &workbench, cx).await;
    assert_eq!(cx.update(|cx| image.read(cx).info().map(|info| info.width)), Some(240), "drawn from the unsaved text");
    assert!(cx.update(|cx| workbench.read(cx).has_dirty_tabs(cx)), "the edit is still there, unsaved");

    // Ctrl+S still saves while the picture is shown.
    assert!(window.run_command(cx, "file.save"));
    window.wait_until(cx, "saved", |_, cx| !buffer.read(cx).is_dirty()).await;
    assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);

    // Zoomed in far, the SVG is rasterized again with more pixels so it stays sharp.
    assert_eq!(cx.update(|cx| image.read(cx).svg_raster_scale()), Some(1.));
    for _ in 0..5 {
        assert!(window.run_command(cx, "image.zoomIn"));
    }
    assert_eq!(cx.update(|cx| image.read(cx).zoom()), Zoom::Scale(7.));
    window.wait_until(cx, "rasterized again", |_, cx| image.read(cx).svg_raster_scale().is_some_and(|scale| scale >= 4.)).await;
}

#[gpui_kit::test]
async fn an_svg_picture_beside_its_text_follows_edits(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("icon.svg", SVG)]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let path = project.join("icon.svg").to_string_lossy().into_owned();

    // Going to a position (as a search result does) opens the text.
    open(&window, &workbench, OpenFile::new(&path).at(1, 0, None), cx);
    assert_eq!(active_tab_mode(&workbench, cx), FileMode::Text);
    let (_, _, text) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());

    // The split copy shows the picture; the first group keeps the text.
    assert!(window.run_command(cx, "workbench.split"));
    cx.run_until_parked();
    assert!(window.run_command(cx, "image.openAsImage"));
    cx.run_until_parked();
    let image = shown_image(&window, &workbench, cx).await;
    let modes = cx.update(|cx| workbench.read(cx).model().groups.iter().map(|group| file_mode(group.active_tab().unwrap())).collect::<Vec<_>>());
    assert_eq!(modes, [FileMode::Text, FileMode::Image]);

    let edited = SVG.replace(r#"height="80""#, r#"height="60""#);
    window.update(cx, |window, cx| text.update(cx, |state, cx| state.set_value(edited, window, cx)));
    cx.update(|cx| text.update(cx, |_, cx| cx.emit(gpui_kit::component::input::InputEvent::Change)));
    window.wait_until(cx, "picture follows the edit", |_, cx| image.read(cx).info().is_some_and(|info| info.height == 60)).await;
}

#[gpui_kit::test]
async fn a_picture_reloads_when_the_file_changes_and_explains_a_broken_file(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[]);
    let file = project.join("photo.png");
    std::fs::write(&file, png(40, 30)).unwrap();
    std::fs::write(project.join("broken.png"), b"\x89PNG\r\n\x1a\nnot really a picture").unwrap();
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);

    open(&window, &workbench, OpenFile::new(file.to_string_lossy()), cx);
    let image = shown_image(&window, &workbench, cx).await;
    assert_eq!(cx.update(|cx| image.read(cx).info().map(|info| (info.width, info.height))), Some((40, 30)));

    std::fs::write(&file, png(64, 48)).unwrap();
    window.wait_until(cx, "reloaded", |_, cx| image.read(cx).info().is_some_and(|info| info.width == 64)).await;

    open(&window, &workbench, OpenFile::new(project.join("broken.png").to_string_lossy()), cx);
    window
        .wait_until(cx, "error shown", |_, cx| active_image(workbench.read(cx), cx).is_some_and(|image| image.read(cx).error().is_some_and(|error| error.starts_with("broken.png cannot be shown as an image"))))
        .await;
}

/// Files that are not images open in the editor as before.
#[gpui_kit::test]
async fn other_files_still_open_as_text(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("main.rs", "fn main() {}\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    open(&window, &workbench, OpenFile::new(project.join("main.rs").to_string_lossy()), cx);
    window.wait_until(cx, "loaded", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).is_ready())).await;
    assert!(cx.update(|cx| active_image(workbench.read(cx), cx).is_none()));
    assert!(!has(&window, "editor-action-image-open-as-text", cx));
    assert!(!window.run_command(cx, "image.zoomIn"));
}
