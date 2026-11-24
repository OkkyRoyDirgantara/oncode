//! Uji tampilan remote, stash, dan branch terhadap remote git lokal (repositori bare).

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

/// Menunggu dialog lalu mengklik tombolnya (tombol dialog muncul setelah tugas latar selesai).
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
    panic!("tombol {id} tidak muncul");
}

/// Menunggu kotak masukan dialog lalu mengetik dan menekan Enter.
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

    // Publish: branch belum punya upstream, jadi push memasang upstream.
    assert!(window.run_command(cx, "git.push"));
    let head = git(&project, &["rev-parse", "HEAD"]).trim().to_string();
    window.wait_until(cx, "main ada di remote", |_, _| std::process::Command::new("git").current_dir(&bare).args(["rev-parse", "main"]).output().is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == head)).await;
    let snapshot = snapshot_of(&project);
    window.wait_until(cx, "upstream terpasang", move |_, cx| snapshot(cx).is_some_and(|s| s.upstream.is_some_and(|u| u.exists))).await;

    // Orang lain mendorong commit; di sini juga ada commit baru: riwayat bercabang.
    let other = tempfile::tempdir().unwrap();
    git(other.path(), &["clone", "-q", &bare.to_string_lossy(), "."]);
    std::fs::write(other.path().join("b.txt"), "b\n").unwrap();
    git(other.path(), &["add", "-A"]);
    git(other.path(), &["commit", "-q", "-m", "Dari rekan"]);
    git(other.path(), &["push", "-q"]);
    std::fs::write(project.join("c.txt"), "c\n").unwrap();
    git(&project, &["add", "-A"]);
    git(&project, &["commit", "-q", "-m", "Dari saya"]);

    // Sync: pull fast-forward gagal, pengguna memilih Rebase, lalu push.
    assert!(window.run_command(cx, "git.sync"));
    click_when_ready(&window, cx, "choice-rebase").await;
    window
        .wait_until(cx, "remote berisi kedua commit", |_, _| {
            std::process::Command::new("git").current_dir(&bare).args(["log", "--format=%s", "main"]).output().is_ok_and(|o| String::from_utf8_lossy(&o.stdout).lines().take(2).collect::<Vec<_>>() == ["Dari saya", "Dari rekan"])
        })
        .await;
    assert_eq!(git(&project, &["status", "--porcelain"]).trim(), "");
}

#[gpui_kit::test]
async fn stash_and_pop_then_create_a_branch(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_scm::init);
    let project = app.git_project(&[("a.txt", "a\n")]);
    std::fs::write(project.join("a.txt"), "diubah\n").unwrap();
    let window = open(&app, &project, cx).await;
    let snapshot = snapshot_of(&project);
    window.wait_until(cx, "perubahan terbaca", |_, cx| snapshot(cx).is_some_and(|s| s.unstaged.len() == 1)).await;

    assert!(window.run_command(cx, "git.stash"));
    answer(&window, cx, "simpan dulu").await;
    window.wait_until(cx, "perubahan tersimpan di stash", |_, _| std::fs::read_to_string(project.join("a.txt")).is_ok_and(|t| t == "a\n")).await;
    assert!(git(&project, &["stash", "list"]).contains("simpan dulu"));

    assert!(window.run_command(cx, "git.stashPop"));
    window.wait_until(cx, "stash dikembalikan", |_, _| std::fs::read_to_string(project.join("a.txt")).is_ok_and(|t| t == "diubah\n")).await;
    assert_eq!(git(&project, &["stash", "list"]).trim(), "");

    assert!(window.run_command(cx, "git.createBranch"));
    answer(&window, cx, "fitur/baru").await;
    window.wait_until(cx, "pindah ke branch baru", |_, _| git(&project, &["branch", "--show-current"]).trim() == "fitur/baru").await;
}
