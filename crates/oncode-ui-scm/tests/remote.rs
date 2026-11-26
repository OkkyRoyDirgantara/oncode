//! Tests of the remote, stash, and branch views against a local git remote (a bare repository).

use std::path::{Path, PathBuf};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use oncode_ui::repos;
use oncode_ui::testing::{git, TestApp, TestWindow};

fn bare_remote() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = oncode_ui::testing::canonical(dir.path());
    git(&path, &["init", "-q", "--bare", "-b", "main"]);
    (dir, path)
}

fn snapshot_of(project: &Path) -> impl Fn(&gpui_kit::App) -> Option<oncode_git::RepoSnapshot> {
    let root = project.to_string_lossy().into_owned();
    move |cx| repos(cx).read(cx).snapshot(&root).cloned()
}

async fn open(app: &TestApp, project: &Path, cx: &mut TestAppContext) -> TestWindow {
    let window = app.open(Some(project), cx).await;
    let snapshot = snapshot_of(project);
    window.wait_until(cx, "snapshot", move |_, cx| snapshot(cx).is_some()).await;
    window
}

/// Waits for a dialog, then clicks its button (the dialog button appears after a background task finishes).
async fn click_when_ready(window: &TestWindow, cx: &mut TestAppContext, id: &'static str) {
    for _ in 0..300 {
        let found = window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
        });
        if found {
            window.update(cx, |window, cx| window.click(id, cx));
            cx.run_until_parked();
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.run_until_parked();
    }
    panic!("button {id} did not appear");
}

/// Waits for the dialog's input box, then types and presses Enter.
async fn answer(window: &TestWindow, cx: &mut TestAppContext, text: &str) {
    for _ in 0..300 {
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
        if !text.is_empty() {
            window.input(text, cx);
        }
        window.press("enter", cx);
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
async fn publishes_then_syncs_with_a_diverged_remote(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_scm::init);
    let (_bare_dir, bare) = bare_remote();
    let project = app.git_project(&[("a.txt", "a\n")]);
    git(&project, &["remote", "add", "origin", &bare.to_string_lossy()]);
    let window = open(&app, &project, cx).await;

    // Publish: the branch has no upstream yet, so the push sets the upstream.
    assert!(window.run_command(cx, "git.push"));
    let head = git(&project, &["rev-parse", "HEAD"]).trim().to_string();
    window.wait_until(cx, "main exists on the remote", |_, _| std::process::Command::new("git").current_dir(&bare).args(["rev-parse", "main"]).output().is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == head)).await;
    let snapshot = snapshot_of(&project);
    window.wait_until(cx, "upstream set", move |_, cx| snapshot(cx).is_some_and(|s| s.upstream.is_some_and(|u| u.exists))).await;

    // Someone else pushes a commit; there is a new commit here too: the history diverges.
    let other = tempfile::tempdir().unwrap();
    git(other.path(), &["clone", "-q", &bare.to_string_lossy(), "."]);
    std::fs::write(other.path().join("b.txt"), "b\n").unwrap();
    git(other.path(), &["add", "-A"]);
    git(other.path(), &["commit", "-q", "-m", "From a colleague"]);
    git(other.path(), &["push", "-q"]);
    std::fs::write(project.join("c.txt"), "c\n").unwrap();
    git(&project, &["add", "-A"]);
    git(&project, &["commit", "-q", "-m", "From me"]);

    // Sync: the fast-forward pull fails, the user picks Rebase, then push.
    assert!(window.run_command(cx, "git.sync"));
    click_when_ready(&window, cx, "choice-rebase").await;
    window
        .wait_until(cx, "remote has both commits", |_, _| {
            std::process::Command::new("git").current_dir(&bare).args(["log", "--format=%s", "main"]).output().is_ok_and(|o| String::from_utf8_lossy(&o.stdout).lines().take(2).collect::<Vec<_>>() == ["From me", "From a colleague"])
        })
        .await;
    assert_eq!(git(&project, &["status", "--porcelain"]).trim(), "");
}

#[gpui_kit::test]
async fn stash_and_pop_then_create_a_branch(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_scm::init);
    let project = app.git_project(&[("a.txt", "a\n")]);
    std::fs::write(project.join("a.txt"), "changed\n").unwrap();
    let window = open(&app, &project, cx).await;
    let snapshot = snapshot_of(&project);
    window.wait_until(cx, "change read", |_, cx| snapshot(cx).is_some_and(|s| s.unstaged.len() == 1)).await;

    assert!(window.run_command(cx, "git.stash"));
    answer(&window, cx, "save for later").await;
    window.wait_until(cx, "change saved in the stash", |_, _| std::fs::read_to_string(project.join("a.txt")).is_ok_and(|t| t == "a\n")).await;
    assert!(git(&project, &["stash", "list"]).contains("save for later"));

    assert!(window.run_command(cx, "git.stashPop"));
    // `git stash pop` restores the file first, then removes the stash entry: wait for both. While
    // the pop runs, `git stash list` can fail briefly ("bad revision 'refs/stash'"), so
    // a failure counts as not finished yet, not as an error.
    let stash_empty = || {
        std::process::Command::new("git").current_dir(&project).args(["stash", "list"]).output().is_ok_and(|out| out.status.success() && out.stdout.trim_ascii().is_empty())
    };
    window
        .wait_until(cx, "stash restored and removed", |_, _| std::fs::read_to_string(project.join("a.txt")).is_ok_and(|t| t == "changed\n") && stash_empty())
        .await;

    assert!(window.run_command(cx, "git.createBranch"));
    answer(&window, cx, "feature/new").await;
    window.wait_until(cx, "switched to the new branch", |_, _| git(&project, &["branch", "--show-current"]).trim() == "feature/new").await;
}
