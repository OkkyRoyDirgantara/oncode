//! Uji menu di bilah judul: item menjalankan command, termasuk command editor yang pintasannya
//! hanya berlaku saat editor fokus (menu memindahkan fokus ke dirinya sendiri). macOS memakai menu
//! bar sistem sebagai gantinya (lihat `app_menu.rs`).
#![cfg(not(target_os = "macos"))]

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use oncode_ui::editor::{active_file, open_file, OpenFile};
use oncode_ui::testing::{TestApp, TestWindow};

/// Membuka menu `menu` lalu mengklik item ke-`index` (pemisah ikut terhitung).
fn pick(window: &TestWindow, cx: &mut TestAppContext, menu: &'static str, index: usize) {
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click(menu, cx);
    });
    cx.run_until_parked();
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.within("popup-menu").click(index, cx);
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
async fn file_menu_creates_a_new_text_file(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("a.txt", "a\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);

    // File → New Text File (item pertama).
    pick(&window, cx, "menu-file", 0);
    let name = cx.update(|cx| active_file(workbench.read(cx), cx).map(|(_, buffer, _)| buffer.read(cx).file_name().to_string()));
    assert_eq!(name.as_deref(), Some("Untitled-1"));
}

#[gpui_kit::test]
async fn edit_menu_runs_editor_commands_on_the_active_file(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("daftar.txt", "satu\ndua\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let path = project.join("daftar.txt").to_string_lossy().into_owned();
    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&path), window, cx)));
    window.wait_until(cx, "berkas dimuat", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).is_ready())).await;
    let (_, _, state) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());

    // Edit: Find, ─, Format Document, Move Line Up, Move Line Down, …
    pick(&window, cx, "menu-edit", 4);
    assert_eq!(cx.update(|cx| state.read(cx).value().to_string()), "dua\nsatu\n");
}
