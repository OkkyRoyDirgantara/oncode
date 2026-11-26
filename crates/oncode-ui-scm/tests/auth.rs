//! Tests of the HTTP credentials dialog: git asks for a username and password through `oncode-helper`, the app
//! shows a dialog, and the answer goes back to git. Server: a local `git http-backend` with Basic auth.

mod common;

use std::path::Path;

use common::GitHttpServer;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use oncode_ui::repos;
use oncode_ui::testing::{git, TestApp, TestWindow};

fn remote_head(server: &GitHttpServer) -> Option<String> {
    let output = std::process::Command::new("git").current_dir(server.bare()).args(["rev-parse", "--verify", "-q", "main"]).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Waits for a dialog to open, then answers it. `None` presses Escape (cancels).
async fn answer_prompt(window: &TestWindow, cx: &mut TestAppContext, text: Option<&str>) {
    for _ in 0..500 {
        let open = window.update(cx, |window, cx| {
            use gpui_kit::component::WindowExt as _;
            window.has_active_dialog(cx)
        });
        if open {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.run_until_parked();
    }
    cx.run_until_parked();
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        match text {
            Some(text) => {
                window.input(text, cx);
                window.press("enter", cx);
            }
            None => window.click("cancel", cx),
        }
    });
    cx.run_until_parked();
    // The next dialog only opens after the answer reaches git.
    for _ in 0..25 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.run_until_parked();
    }
}

async fn setup(app: &TestApp, server: &GitHttpServer, cx: &mut TestAppContext) -> (TestWindow, std::path::PathBuf) {
    cx.update(oncode_ui_scm::init);
    let project = app.git_project(&[("a.txt", "a\n")]);
    git(&project, &["remote", "add", "origin", &server.url()]);
    let window = app.open(Some(&project), cx).await;
    let root = project.to_string_lossy().into_owned();
    window.wait_until(cx, "snapshot", move |_, cx| repos(cx).read(cx).snapshot(&root).is_some()).await;
    (window, project)
}

fn head(project: &Path) -> String {
    git(project, &["rev-parse", "HEAD"]).trim().to_string()
}

#[gpui_kit::test]
async fn push_asks_for_username_and_password(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let server = GitHttpServer::start("budi", "secret");
    let (window, project) = setup(&app, &server, cx).await;

    assert!(window.run_command(cx, "git.push"));
    answer_prompt(&window, cx, Some("budi")).await;
    answer_prompt(&window, cx, Some("secret")).await;
    let expected = head(&project);
    window.wait_until(cx, "push succeeded", |_, _| remote_head(&server).as_deref() == Some(expected.as_str())).await;
}

#[gpui_kit::test]
async fn wrong_password_or_cancel_leaves_the_remote_untouched(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let server = GitHttpServer::start("budi", "secret");
    let (window, _project) = setup(&app, &server, cx).await;

    // Wrong password: git gives up, the remote stays empty.
    assert!(window.run_command(cx, "git.push"));
    answer_prompt(&window, cx, Some("budi")).await;
    answer_prompt(&window, cx, Some("wrong")).await;
    window.wait_until(cx, "credentials rejected", |_, _| server.rejected.load(std::sync::atomic::Ordering::SeqCst) >= 2).await;
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.run_until_parked();
    }
    assert_eq!(remote_head(&server), None);

    // Canceled at the username dialog: the push stops without sending anything.
    assert!(window.run_command(cx, "git.push"));
    answer_prompt(&window, cx, None).await;
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.run_until_parked();
    }
    assert_eq!(remote_head(&server), None);
    let busy = window.update(cx, |window, cx| {
        use gpui_kit::component::WindowExt as _;
        window.has_active_dialog(cx)
    });
    assert!(!busy, "no dialog is left behind");
}
