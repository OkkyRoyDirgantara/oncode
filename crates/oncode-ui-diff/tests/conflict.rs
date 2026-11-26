//! Conflict editor view tests: a conflicting merge is resolved through a real window.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext};
use oncode_ui::testing::{git, TestApp};
use oncode_ui::{repos, Workbench};
use oncode_ui_diff::conflict_tab::{ConflictTab, ConflictView};
use oncode_ui_diff::open_conflict;

fn active_conflict(workbench: &Entity<Workbench>, cx: &gpui_kit::App) -> Option<Entity<ConflictView>> {
    workbench.read(cx).active_content()?.as_any().downcast_ref::<ConflictTab>().map(|tab| tab.view.clone())
}

/// Repository with a merge of `feature` into `main` that conflicts in `a.txt`.
fn conflicted(app: &TestApp) -> std::path::PathBuf {
    let project = app.git_project(&[("a.txt", "initial\nsame\n")]);
    git(&project, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(project.join("a.txt"), "from feature\nsame\n").unwrap();
    git(&project, &["commit", "-q", "-am", "Change on feature"]);
    git(&project, &["checkout", "-q", "main"]);
    std::fs::write(project.join("a.txt"), "from main\nsame\n").unwrap();
    git(&project, &["commit", "-q", "-am", "Change on main"]);
    let merge = std::process::Command::new("git").current_dir(&project).args(["merge", "-q", "feature"]).output().unwrap();
    assert!(!merge.status.success(), "merge must conflict");
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
    window.wait_until(cx, "conflict status", |_, cx| repos(cx).read(cx).snapshot(&root).is_some_and(|s| s.conflicted.len() == 1)).await;

    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_conflict(workbench, root.clone(), "a.txt".into(), window, cx)));
    window.wait_until(cx, "conflict loaded", |_, cx| active_conflict(&workbench, cx).is_some_and(|view| view.read(cx).is_loaded())).await;
    let view = cx.update(|cx| active_conflict(&workbench, cx).unwrap());
    assert_eq!(cx.update(|cx| view.read(cx).counts()), (1, 1), "one conflict, not yet resolved");

    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click("conflict-use-incoming", cx);
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| view.read(cx).counts().1), 0, "chosen conflict is resolved");
    assert_eq!(cx.update(|cx| view.read(cx).result().read(cx).value().to_string()), "from feature\nsame\n");

    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click("conflict-mark-resolved", cx);
    });
    window.wait_until(cx, "conflict marked as resolved", |_, cx| repos(cx).read(cx).snapshot(&root).is_some_and(|s| s.conflicted.is_empty())).await;
    assert_eq!(std::fs::read_to_string(project.join("a.txt")).unwrap(), "from feature\nsame\n");
    assert_eq!(git(&project, &["diff", "--cached", "--name-only"]).trim(), "a.txt", "result is already staged");
}
