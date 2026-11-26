//! View tests for project-wide search and replace through a real window.

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
    let project = app.git_project(&[(".gitignore", "build/\n"), ("a.txt", "apple orange\napple\n"), ("b.txt", "apple\n"), ("build/c.txt", "apple\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let view = cx.update(|cx| search_view(workbench.read(cx)).unwrap());

    // b.txt is open in the editor: its replacement must go through the editor, not overwrite the file on disk.
    let b = project.join("b.txt").to_string_lossy().into_owned();
    window.update(cx, |window, cx| workbench.update(cx, |w, cx| open_file(w, OpenFile::new(b.clone()), window, cx)));
    window.wait_until(cx, "b.txt loaded", |_, cx| buffers(workbench.read(cx)).and_then(|bs| bs.read(cx).get(&b)).is_some_and(|buf| buf.read(cx).is_ready())).await;

    // Ctrl+Shift+F, then type the query.
    assert!(window.run_command(cx, "search.focus"));
    cx.run_until_parked();
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input("apple", cx);
        window.press("enter", cx);
    });
    window.wait_until(cx, "search finished", |_, cx| view.read(cx).results().status == SearchStatus::Done).await;
    let files: Vec<(String, usize)> = cx.update(|cx| view.read(cx).results().files.iter().map(|f| (f.path.clone(), f.matches.len())).collect());
    assert_eq!(files, [("a.txt".to_string(), 2), ("b.txt".to_string(), 1)], "build/ is ignored because of .gitignore");

    // Replace all, with confirmation.
    assert!(window.run_command(cx, "search.replace"));
    cx.run_until_parked();
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click("search-replacement", cx);
        window.input("mango", cx);
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
    window.wait_until(cx, "a.txt replaced on disk", |_, _| std::fs::read_to_string(&a).is_ok_and(|t| t == "mango orange\nmango\n")).await;
    window
        .wait_until(cx, "b.txt replaced in the editor", |_, cx| {
            buffers(workbench.read(cx)).and_then(|bs| bs.read(cx).get(&b)).and_then(|buf| buf.read(cx).text(cx)).is_some_and(|text| text.to_string() == "mango\n")
        })
        .await;
    assert_eq!(std::fs::read_to_string(project.join("build/c.txt")).unwrap(), "apple\n", "the ignored file is not touched");
}
