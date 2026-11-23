//! Uji tampilan Graph: riwayat, detail commit, dan interactive rebase lewat jendela sungguhan.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext};
use oncode_ui::testing::{git, TestApp, TestWindow};
use oncode_ui::ViewTab;
use oncode_ui_graph::graph_tab::GraphTab;

/// Repositori dengan tiga commit setelah commit pertama: a, b, c (c paling baru).
fn history(app: &TestApp) -> std::path::PathBuf {
    let project = app.git_project(&[("README.md", "# Proyek\n")]);
    for name in ["a", "b", "c"] {
        std::fs::write(project.join(format!("{name}.txt")), format!("{name}\n")).unwrap();
        git(&project, &["add", "-A"]);
        git(&project, &["commit", "-q", "-m", &format!("Commit {name}")]);
    }
    project
}

fn subjects(project: &std::path::Path) -> Vec<String> {
    git(project, &["log", "--format=%s"]).lines().map(String::from).collect()
}

async fn open_graph(app: &TestApp, project: &std::path::Path, cx: &mut TestAppContext) -> (TestWindow, Entity<GraphTab>) {
    let window = app.open(Some(project), cx).await;
    let workbench = window.workbench(cx);
    window.wait_until(cx, "store graph", |_, cx| oncode_ui_graph::graph_store(workbench.read(cx)).is_some()).await;
    assert!(window.run_command(cx, "git.showGraph"));
    cx.run_until_parked();
    let tab = cx.update(|cx| workbench.read(cx).active_content().and_then(|c| c.as_any().downcast_ref::<ViewTab<GraphTab>>().map(|t| t.0.clone())).expect("tab graph aktif"));
    window.wait_until(cx, "riwayat dimuat", |_, cx| !tab.read(cx).is_loading() && !tab.read(cx).rows().is_empty()).await;
    (window, tab)
}

#[gpui_kit::test]
async fn graph_lists_history_and_selects_commits(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_graph::init);
    let project = history(&app);
    let (window, tab) = open_graph(&app, &project, cx).await;

    let listed: Vec<String> = cx.update(|cx| tab.read(cx).rows().iter().map(|r| r.commit.subject.clone()).collect());
    assert_eq!(listed, ["Commit c", "Commit b", "Commit a", "Commit pertama"]);
    // Commit teratas terpilih otomatis.
    let head = git(&project, &["rev-parse", "HEAD"]).trim().to_string();
    assert_eq!(cx.update(|cx| tab.read(cx).selected().to_vec()), [head]);

    // Klik baris lain memilihnya.
    let b = git(&project, &["rev-parse", "HEAD~1"]).trim().to_string();
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click(format!("graph-row-{b}"), cx);
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| tab.read(cx).selected().to_vec()), [b]);
}

#[gpui_kit::test]
async fn interactive_rebase_reorders_commits(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_graph::init);
    let project = history(&app);
    let (window, _tab) = open_graph(&app, &project, cx).await;
    assert_eq!(subjects(&project), ["Commit c", "Commit b", "Commit a", "Commit pertama"]);

    // Rebase dari awal riwayat: daftar berisi pertama, a, b, c (tertua di atas).
    assert!(window.run_command(cx, "git.interactiveRebase"));
    window.wait_until(cx, "dialog rebase", |_, _| true).await;
    for _ in 0..200 {
        let ready = window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.try_find("rebase-start").is_some()
        });
        if ready {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.run_until_parked();
    }
    // Pindahkan "Commit c" (baris ke-4) ke atas dua kali, tepat setelah commit pertama.
    window.update(cx, |window, cx| {
        window.click("rebase-up-3", cx);
        window.render_frame(cx);
        window.click("rebase-up-2", cx);
        window.render_frame(cx);
        window.click("rebase-start", cx);
    });
    let expected = ["Commit b", "Commit a", "Commit c", "Commit pertama"];
    // Isi berkas tidak berubah, hanya urutan commit.
    window.wait_until(cx, "rebase selesai", |_, _| subjects(&project) == expected).await;
    assert_eq!(git(&project, &["status", "--porcelain"]).trim(), "");
}
