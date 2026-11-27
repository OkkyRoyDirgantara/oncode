//! Graph view tests: history, commit details, and interactive rebase through a real window.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext};
use oncode_ui::testing::{git, TestApp, TestWindow};
use oncode_ui::ViewTab;
use oncode_ui_graph::graph_tab::GraphTab;

/// A repository with three commits after the first commit: a, b, c (c is the newest).
fn history(app: &TestApp) -> std::path::PathBuf {
    let project = app.git_project(&[("README.md", "# Project\n")]);
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
    window.wait_until(cx, "graph store", |_, cx| oncode_ui_graph::graph_store(workbench.read(cx)).is_some()).await;
    assert!(window.run_command(cx, "git.showGraph"));
    cx.run_until_parked();
    let tab = cx.update(|cx| workbench.read(cx).active_content().and_then(|c| c.as_any().downcast_ref::<ViewTab<GraphTab>>().map(|t| t.0.clone())).expect("active graph tab"));
    window.wait_until(cx, "history loaded", |_, cx| !tab.read(cx).is_loading() && !tab.read(cx).rows().is_empty()).await;
    (window, tab)
}

#[gpui_kit::test]
async fn graph_lists_history_and_selects_commits(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_graph::init);
    let project = history(&app);
    let (window, tab) = open_graph(&app, &project, cx).await;

    let listed: Vec<String> = cx.update(|cx| tab.read(cx).rows().iter().map(|r| r.commit.subject.clone()).collect());
    assert_eq!(listed, ["Commit c", "Commit b", "Commit a", "First commit"]);
    // The top commit is selected automatically.
    let head = git(&project, &["rev-parse", "HEAD"]).trim().to_string();
    assert_eq!(cx.update(|cx| tab.read(cx).selected().to_vec()), [head]);

    // Clicking another row selects it.
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
    assert_eq!(subjects(&project), ["Commit c", "Commit b", "Commit a", "First commit"]);

    // Rebase from the start of history: the list holds first, a, b, c (oldest on top).
    assert!(window.run_command(cx, "git.interactiveRebase"));
    window.wait_until(cx, "rebase dialog", |_, _| true).await;
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
    // Move "Commit c" (the 4th row) up twice, right after the first commit.
    window.update(cx, |window, cx| {
        window.click("rebase-up-3", cx);
        window.render_frame(cx);
        window.click("rebase-up-2", cx);
        window.render_frame(cx);
        window.click("rebase-start", cx);
    });
    let expected = ["Commit b", "Commit a", "Commit c", "First commit"];
    // The file contents do not change, only the commit order.
    window.wait_until(cx, "rebase finished", |_, _| subjects(&project) == expected).await;
    assert_eq!(git(&project, &["status", "--porcelain"]).trim(), "");
}

/// Closing the Graph tab stops its `git log`, which otherwise waits on its pipe until the app quits.
#[gpui_kit::test]
async fn closing_the_graph_stops_its_log_stream(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_graph::init);
    let project = history(&app);
    let (window, tab) = open_graph(&app, &project, cx).await;
    assert_eq!(app.core.log_streams.lock().unwrap().len(), 1);

    drop(tab);
    assert!(window.run_command(cx, "workbench.closeTab"));
    for _ in 0..5 {
        window.update(cx, |window, cx| window.render_frame(cx));
        cx.run_until_parked();
    }
    let core = app.core.clone();
    window.wait_until(cx, "stream closed", move |_, _| core.log_streams.lock().unwrap().is_empty()).await;
}
