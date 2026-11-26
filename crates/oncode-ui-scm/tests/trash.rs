//! Tests of discarding an untracked file with the row button: the file is moved to the trash, not
//! deleted permanently. A separate test binary because `XDG_DATA_HOME` is pointed at a temporary folder,
//! so the user's trash is not touched.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use oncode_ui::repos;
use oncode_ui::testing::TestApp;

/// Only on OSes with a freedesktop trash (Linux/BSD), which can be pointed at a temporary
/// folder. On Windows and macOS the file would go into the user's real Recycle Bin/Trash.
#[gpui_kit::test]
#[cfg_attr(any(windows, target_os = "macos"), ignore = "the user's real trash cannot be redirected on this OS")]
async fn untracked_file_goes_to_the_trash_after_confirming(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_DATA_HOME", data.path());
    let app = TestApp::new(cx);
    cx.update(oncode_ui_scm::init);
    let project = app.git_project(&[("a.txt", "a\n")]);
    std::fs::write(project.join("trash.txt"), "not needed\n").unwrap();
    let window = app.open(Some(&project), cx).await;
    let root = project.to_string_lossy().into_owned();
    window.wait_until(cx, "untracked file read", move |_, cx| repos(cx).read(cx).snapshot(&root).is_some_and(|s| s.untracked.len() == 1)).await;
    assert!(window.run_command(cx, "scm.focus"));
    cx.run_until_parked();

    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click("scm-discard-trash.txt", cx);
    });
    cx.run_until_parked();
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click("ok", cx);
    });
    let gone = project.join("trash.txt");
    window.wait_until(cx, "file moved", |_, _| !gone.exists()).await;
    let trashed = data.path().join("Trash/files/trash.txt");
    assert_eq!(std::fs::read_to_string(trashed).unwrap(), "not needed\n", "its content can be restored from the trash");
    assert!(project.join("a.txt").exists());
}
