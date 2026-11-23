//! Uji tampilan editor konflik: merge yang bentrok diselesaikan lewat jendela sungguhan.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext};
use oncode_ui::testing::{git, TestApp};
use oncode_ui::{repos, Workbench};
use oncode_ui_diff::conflict_tab::{ConflictTab, ConflictView};
use oncode_ui_diff::open_conflict;

fn active_conflict(workbench: &Entity<Workbench>, cx: &gpui_kit::App) -> Option<Entity<ConflictView>> {
    workbench.read(cx).active_content()?.as_any().downcast_ref::<ConflictTab>().map(|tab| tab.view.clone())
}

/// Repositori dengan merge `fitur` ke `main` yang bentrok di `a.txt`.
fn conflicted(app: &TestApp) -> std::path::PathBuf {
    let project = app.git_project(&[("a.txt", "awal\nsama\n")]);
    git(&project, &["checkout", "-q", "-b", "fitur"]);
    std::fs::write(project.join("a.txt"), "dari fitur\nsama\n").unwrap();
    git(&project, &["commit", "-q", "-am", "Ubah di fitur"]);
    git(&project, &["checkout", "-q", "main"]);
    std::fs::write(project.join("a.txt"), "dari main\nsama\n").unwrap();
    git(&project, &["commit", "-q", "-am", "Ubah di main"]);
    let merge = std::process::Command::new("git").current_dir(&project).args(["merge", "-q", "fitur"]).output().unwrap();
    assert!(!merge.status.success(), "merge harus bentrok");
    project
}

#[gpui_kit::test]
async fn resolves_a_conflict_with_the_incoming_side(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_diff::init);
    let project = conflicted(&app);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let root = project.to_string_lossy().into_owned();
    window.wait_until(cx, "status konflik", |_, cx| repos(cx).read(cx).snapshot(&root).is_some_and(|s| s.conflicted.len() == 1)).await;

    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_conflict(workbench, root.clone(), "a.txt".into(), window, cx)));
    window.wait_until(cx, "konflik dimuat", |_, cx| active_conflict(&workbench, cx).is_some_and(|view| view.read(cx).is_loaded())).await;
    let view = cx.update(|cx| active_conflict(&workbench, cx).unwrap());
    assert_eq!(cx.update(|cx| view.read(cx).counts()), (1, 1), "satu konflik, belum selesai");

    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click("conflict-use-incoming", cx);
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| view.read(cx).counts().1), 0, "konflik terpilih selesai");
    assert_eq!(cx.update(|cx| view.read(cx).result().read(cx).value().to_string()), "dari fitur\nsama\n");

    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click("conflict-mark-resolved", cx);
    });
    window.wait_until(cx, "konflik ditandai selesai", |_, cx| repos(cx).read(cx).snapshot(&root).is_some_and(|s| s.conflicted.is_empty())).await;
    assert_eq!(std::fs::read_to_string(project.join("a.txt")).unwrap(), "dari fitur\nsama\n");
    assert_eq!(git(&project, &["diff", "--cached", "--name-only"]).trim(), "a.txt", "hasil sudah di-stage");
}
