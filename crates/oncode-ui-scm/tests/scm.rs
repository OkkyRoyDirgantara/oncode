//! Uji tampilan panel Source Control lewat jendela sungguhan dan git sungguhan.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use oncode_ui::testing::{git, TestApp, TestWindow};
use oncode_ui::{repos, Workbench};

fn changed(window: &TestWindow, cx: &mut TestAppContext, project: &std::path::Path) -> impl Fn(&gpui_kit::App) -> Option<(usize, usize, usize)> {
    let _ = (window, cx);
    let root = project.to_string_lossy().into_owned();
    move |cx| repos(cx).read(cx).snapshot(&root).map(|s| (s.staged.len(), s.unstaged.len(), s.untracked.len()))
}

async fn open_scm(app: &TestApp, project: &std::path::Path, cx: &mut TestAppContext) -> TestWindow {
    let window = app.open(Some(project), cx).await;
    let state = changed(&window, cx, project);
    window.wait_until(cx, "snapshot repositori", move |_, cx| state(cx).is_some()).await;
    assert!(window.run_command(cx, "scm.focus"));
    cx.run_until_parked();
    window.update(cx, |window, cx| window.render_frame(cx));
    window
}

fn workbench_sidebar(window: &TestWindow, cx: &mut TestAppContext) -> Option<String> {
    let workbench: gpui_kit::Entity<Workbench> = window.workbench(cx);
    cx.update(|cx| workbench.read(cx).active_sidebar().map(|s| s.to_string()))
}

#[gpui_kit::test]
async fn stages_a_file_with_the_row_button_and_commits(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_scm::init);
    let project = app.git_project(&[("a.txt", "satu\n")]);
    std::fs::write(project.join("a.txt"), "dua\n").unwrap();
    std::fs::write(project.join("baru.txt"), "baru\n").unwrap();
    let window = open_scm(&app, &project, cx).await;
    assert_eq!(workbench_sidebar(&window, cx).as_deref(), Some("scm"));
    let state = changed(&window, cx, &project);
    window.wait_until(cx, "perubahan terbaca", |_, cx| state(cx) == Some((0, 1, 1))).await;

    // Tombol stage di baris berkas (muncul saat kursor di atas baris).
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click("scm-stage-a.txt", cx);
    });
    window.wait_until(cx, "a.txt ter-stage", |_, cx| state(cx) == Some((1, 0, 1))).await;
    assert_eq!(git(&project, &["diff", "--cached", "--name-only"]).trim(), "a.txt");

    // Pesan commit lalu tombol Commit.
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click("scm-message", cx);
        window.input("Ubah a", cx);
    });
    cx.run_until_parked();
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click("scm-commit-button", cx);
    });
    window.wait_until(cx, "commit selesai", |_, cx| state(cx) == Some((0, 0, 1))).await;
    assert_eq!(git(&project, &["log", "-1", "--format=%s"]).trim(), "Ubah a");
    assert_eq!(git(&project, &["show", "HEAD:a.txt"]), "dua\n");
    // Berkas untracked tidak ikut ter-commit.
    assert_eq!(git(&project, &["status", "--porcelain"]).trim(), "?? baru.txt");
}

#[gpui_kit::test]
async fn stage_all_then_unstage_all(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_scm::init);
    let project = app.git_project(&[("a.txt", "satu\n")]);
    std::fs::write(project.join("a.txt"), "dua\n").unwrap();
    std::fs::write(project.join("b.txt"), "b\n").unwrap();
    let window = open_scm(&app, &project, cx).await;
    let state = changed(&window, cx, &project);
    window.wait_until(cx, "perubahan terbaca", |_, cx| state(cx) == Some((0, 1, 1))).await;

    assert!(window.run_command(cx, "git.stageAll"));
    window.wait_until(cx, "semua ter-stage", |_, cx| state(cx) == Some((2, 0, 0))).await;
    assert!(window.run_command(cx, "git.unstageAll"));
    window.wait_until(cx, "semua kembali", |_, cx| state(cx) == Some((0, 1, 1))).await;
}

#[gpui_kit::test]
async fn restricted_folder_has_no_source_control(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_scm::init);
    let project = app.git_project(&[("a.txt", "satu\n")]);
    app.trust_revoke(&project);
    let window = app.open(Some(&project), cx).await;
    assert!(!window.run_command(cx, "git.stageAll"), "command Git tidak ada di mode terbatas");
    assert!(!window.run_command(cx, "scm.focus"));
}
