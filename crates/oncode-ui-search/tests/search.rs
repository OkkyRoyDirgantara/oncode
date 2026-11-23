//! Uji tampilan pencarian dan penggantian di seluruh proyek lewat jendela sungguhan.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use oncode_ui::editor::{buffers, open_file, OpenFile};
use oncode_ui::testing::TestApp;
use oncode_ui_search::model::SearchStatus;
use oncode_ui_search::search_view;

#[gpui_kit::test]
async fn search_respects_gitignore_then_replace_all_updates_closed_and_open_files(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_search::init);
    let project = app.git_project(&[(".gitignore", "build/\n"), ("a.txt", "apel jeruk\napel\n"), ("b.txt", "apel\n"), ("build/c.txt", "apel\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let view = cx.update(|cx| search_view(workbench.read(cx)).unwrap());

    // b.txt terbuka di editor: penggantiannya harus lewat editor, bukan menimpa berkas di disk.
    let b = project.join("b.txt").to_string_lossy().into_owned();
    window.update(cx, |window, cx| workbench.update(cx, |w, cx| open_file(w, OpenFile::new(b.clone()), window, cx)));
    window.wait_until(cx, "b.txt dimuat", |_, cx| buffers(workbench.read(cx)).and_then(|bs| bs.read(cx).get(&b)).is_some_and(|buf| buf.read(cx).is_ready())).await;

    // Ctrl+Shift+F lalu mengetik kueri.
    assert!(window.run_command(cx, "search.focus"));
    cx.run_until_parked();
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input("apel", cx);
        window.press("enter", cx);
    });
    window.wait_until(cx, "pencarian selesai", |_, cx| view.read(cx).results().status == SearchStatus::Done).await;
    let files: Vec<(String, usize)> = cx.update(|cx| view.read(cx).results().files.iter().map(|f| (f.path.clone(), f.matches.len())).collect());
    assert_eq!(files, [("a.txt".to_string(), 2), ("b.txt".to_string(), 1)], "build/ diabaikan karena .gitignore");

    // Ganti semua, dengan konfirmasi.
    assert!(window.run_command(cx, "search.replace"));
    cx.run_until_parked();
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click("search-replacement", cx);
        window.input("mangga", cx);
    });
    cx.run_until_parked();
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click("search-replace-all", cx);
    });
    cx.run_until_parked();
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click("ok", cx);
    });
    let a = project.join("a.txt");
    window.wait_until(cx, "a.txt diganti di disk", |_, _| std::fs::read_to_string(&a).is_ok_and(|t| t == "mangga jeruk\nmangga\n")).await;
    window
        .wait_until(cx, "b.txt diganti di editor", |_, cx| {
            buffers(workbench.read(cx)).and_then(|bs| bs.read(cx).get(&b)).and_then(|buf| buf.read(cx).text(cx)).is_some_and(|text| text.to_string() == "mangga\n")
        })
        .await;
    assert_eq!(std::fs::read_to_string(project.join("build/c.txt")).unwrap(), "apel\n", "berkas yang diabaikan tidak disentuh");
}
