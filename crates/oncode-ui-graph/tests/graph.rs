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
    // The watcher may report the commits above while the first load runs; the reload that follows closes
    // the first load's stream in the background.
    let core = app.core.clone();
    window.wait_until(cx, "one stream", move |_, _| core.log_streams.lock().unwrap().len() == 1).await;

    drop(tab);
    assert!(window.run_command(cx, "workbench.closeTab"));
    for _ in 0..5 {
        window.update(cx, |window, cx| window.render_frame(cx));
        cx.run_until_parked();
    }
    let core = app.core.clone();
    window.wait_until(cx, "stream closed", move |_, _| core.log_streams.lock().unwrap().is_empty()).await;
}

/// A reload that starts while the previous one is still opening its `git log` (refs changed during loading)
/// closes the stream the discarded load opened, instead of leaving that `git log` running.
#[gpui_kit::test]
async fn a_reload_during_loading_leaves_no_log_stream_open(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_graph::init);
    let project = history(&app);
    let (window, tab) = open_graph(&app, &project, cx).await;

    tab.update(cx, |tab, cx| {
        tab.reload(cx);
        tab.reload(cx);
    });
    window.wait_until(cx, "history reloaded", |_, cx| !tab.read(cx).is_loading() && !tab.read(cx).rows().is_empty()).await;
    let core = app.core.clone();
    window.wait_until(cx, "one stream left", move |_, _| core.log_streams.lock().unwrap().len() == 1).await;
}

/// "File History" from a Source Control row passes `[root, path]`; the history is of that file, not of the
/// file in the active editor (there is none here).
#[gpui_kit::test]
async fn file_history_shows_the_file_passed_as_argument(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_graph::init);
    let project = history(&app);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    window.wait_until(cx, "graph store", |_, cx| oncode_ui_graph::graph_store(workbench.read(cx)).is_some()).await;

    let root = project.to_string_lossy().to_string();
    window.update(cx, |window, cx| oncode_ui::commands::execute_now("git.fileHistory", &serde_json::json!([root, "b.txt"]), window, cx));
    cx.run_until_parked();
    let tab = cx.update(|cx| workbench.read(cx).active_content().and_then(|c| c.as_any().downcast_ref::<ViewTab<GraphTab>>().map(|t| t.0.clone())).expect("file history tab"));
    window.wait_until(cx, "history loaded", |_, cx| !tab.read(cx).is_loading() && !tab.read(cx).rows().is_empty()).await;
    let listed: Vec<String> = cx.update(|cx| tab.read(cx).rows().iter().map(|r| r.commit.subject.clone()).collect());
    assert_eq!(listed, ["Commit b"]);
}

fn listed(tab: &Entity<GraphTab>, cx: &mut TestAppContext) -> Vec<String> {
    cx.update(|cx| tab.read(cx).rows().iter().map(|r| r.commit.subject.clone()).collect())
}

/// The search box finds a commit by (part of) its hash and selects it in the full history; the author box
/// and the date filter narrow the list.
#[gpui_kit::test]
async fn graph_search_by_hash_author_and_date(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_graph::init);
    let project = history(&app);
    std::fs::write(project.join("d.txt"), "d\n").unwrap();
    git(&project, &["add", "-A"]);
    git(&project, &["commit", "-q", "--author", "Someone Else <else@example.com>", "-m", "Commit d"]);
    let (window, tab) = open_graph(&app, &project, cx).await;

    let a = git(&project, &["rev-parse", "HEAD~3"]).trim().to_string();
    let search = cx.update(|cx| tab.read(cx).search_input().clone());
    window.update(cx, |window, cx| search.update(cx, |input, cx| input.set_value(a[..8].to_string(), window, cx)));
    tab.update(cx, |tab, cx| tab.reload(cx));
    window.wait_until(cx, "commit a selected", |_, cx| tab.read(cx).selected() == [a.clone()]).await;
    assert_eq!(listed(&tab, cx).len(), 5, "the whole history stays listed");

    window.update(cx, |window, cx| search.update(cx, |input, cx| input.set_value(String::new(), window, cx)));
    let author = cx.update(|cx| tab.read(cx).author_input().clone());
    window.update(cx, |window, cx| author.update(cx, |input, cx| input.set_value("Someone".to_string(), window, cx)));
    tab.update(cx, |tab, cx| tab.reload(cx));
    window.wait_until(cx, "only the other author's commit", |_, cx| !tab.read(cx).is_loading() && tab.read(cx).rows().len() == 1).await;
    assert_eq!(listed(&tab, cx), ["Commit d"]);

    window.update(cx, |window, cx| author.update(cx, |input, cx| input.set_value(String::new(), window, cx)));
    tab.update(cx, |tab, cx| tab.set_since(Some("2099-01-01".into()), cx));
    window.wait_until(cx, "nothing after a future date", |_, cx| !tab.read(cx).is_loading() && tab.read(cx).rows().is_empty()).await;
    tab.update(cx, |tab, cx| tab.set_since(None, cx));
    window.wait_until(cx, "everything again", |_, cx| !tab.read(cx).is_loading() && tab.read(cx).rows().len() == 5).await;
}

/// A commit row in the interactive rebase dialog can be dragged onto another row to move it there.
#[gpui_kit::test]
async fn interactive_rebase_rows_can_be_dragged(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_graph::init);
    let project = history(&app);
    let (window, _tab) = open_graph(&app, &project, cx).await;
    assert!(window.run_command(cx, "git.interactiveRebase"));
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
    // Drag "Commit c" (the last row) onto "Commit a" (the second row).
    window.update(cx, |window, cx| {
        window.drag_to("rebase-item-3", "rebase-item-1", cx);
        window.render_frame(cx);
        window.click("rebase-start", cx);
    });
    let expected = ["Commit b", "Commit a", "Commit c", "First commit"];
    window.wait_until(cx, "rebase finished", |_, _| subjects(&project) == expected).await;
}

/// Show Line History lists the commits that changed the cursor's line; the editor's line is mapped to the
/// committed file first (here an unsaved line was added above it).
#[gpui_kit::test]
async fn line_history_of_the_cursor_line(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_graph::init);
    let project = app.git_project(&[("README.md", "# Project\n")]);
    for (content, message) in [("a\nb\nc\n", "Add f"), ("a\nB\nc\n", "Change line 2"), ("A\nB\nc\n", "Change line 1")] {
        std::fs::write(project.join("f.txt"), content).unwrap();
        git(&project, &["add", "-A"]);
        git(&project, &["commit", "-q", "-m", message]);
    }
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    window.wait_until(cx, "graph store", |_, cx| oncode_ui_graph::graph_store(workbench.read(cx)).is_some()).await;
    let path = project.join("f.txt").to_string_lossy().into_owned();
    window.update(cx, |window, cx| workbench.update(cx, |w, cx| oncode_ui::editor::open_file(w, oncode_ui::editor::OpenFile::new(&path), window, cx)));
    window.wait_until(cx, "f.txt loaded", |_, cx| oncode_ui::editor::active_file(workbench.read(cx), cx).is_some_and(|(_, b, _)| b.read(cx).is_ready())).await;
    let state = cx.update(|cx| oncode_ui::editor::active_file(workbench.read(cx), cx).unwrap().2);
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input("new\n", cx);
    });
    cx.run_until_parked();
    // The cursor is now on the editor's line 2, which is "A" (line 1 at HEAD); move it to line 3 ("B", line 2 at HEAD).
    cx.update(|cx| state.update(cx, |state, cx| state.set_selected_range(6..6, cx)));
    assert!(window.run_command(cx, "git.lineHistory"));
    window
        .wait_until(cx, "line history tab", |_, cx| {
            workbench.read(cx).active_content().and_then(|c| c.as_any().downcast_ref::<ViewTab<GraphTab>>().map(|t| t.0.clone())).is_some_and(|tab| !tab.read(cx).is_loading() && !tab.read(cx).rows().is_empty())
        })
        .await;
    let tab = cx.update(|cx| workbench.read(cx).active_content().and_then(|c| c.as_any().downcast_ref::<ViewTab<GraphTab>>().map(|t| t.0.clone())).unwrap());
    assert_eq!(listed(&tab, cx), ["Change line 2", "Add f"]);
}

/// Clicks a dialog button once it is shown.
async fn click_when_shown(window: &TestWindow, cx: &mut TestAppContext, id: &str) {
    for _ in 0..300 {
        let shown = window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.try_find(id.to_string()).is_some()
        });
        if shown {
            window.update(cx, |window, cx| window.click(id.to_string(), cx));
            cx.run_until_parked();
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.run_until_parked();
    }
    panic!("{id} never appeared");
}

/// Dropping a branch onto another branch in the ref panel merges it there (switching to the target first).
#[gpui_kit::test]
async fn dragging_a_branch_onto_another_merges_it(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_graph::init);
    let project = history(&app);
    git(&project, &["switch", "-q", "-c", "feature"]);
    std::fs::write(project.join("f.txt"), "f\n").unwrap();
    git(&project, &["add", "-A"]);
    git(&project, &["commit", "-q", "-m", "Feature work"]);
    let (window, _tab) = open_graph(&app, &project, cx).await;
    window.wait_until(cx, "refs loaded", |_, _| true).await;
    for _ in 0..200 {
        let ready = window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.try_find("ref-branch-main").is_some() && window.try_find("ref-branch-feature").is_some()
        });
        if ready {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.run_until_parked();
    }
    window.update(cx, |window, cx| window.drag_to("ref-branch-feature", "ref-branch-main", cx));
    cx.run_until_parked();
    click_when_shown(&window, cx, "choice-merge").await;
    // The merge dialog itself (fast-forward when possible).
    click_when_shown(&window, cx, "choice-merge").await;
    window.wait_until(cx, "main has the feature commit", |_, _| subjects(&project).first().map(String::as_str) == Some("Feature work")).await;
    assert_eq!(git(&project, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(), "main");
}

/// Compare with Current Branch lists only what the branch changed since it split off, not what the current
/// branch changed meanwhile.
#[gpui_kit::test]
async fn compare_a_branch_with_the_current_branch(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_graph::init);
    let project = history(&app);
    git(&project, &["switch", "-q", "-c", "feature", "HEAD~1"]);
    std::fs::write(project.join("g.txt"), "g\n").unwrap();
    git(&project, &["add", "-A"]);
    git(&project, &["commit", "-q", "-m", "Feature g"]);
    git(&project, &["switch", "-q", "main"]);
    let (window, tab) = open_graph(&app, &project, cx).await;
    tab.update(cx, |tab, cx| tab.compare_with_current("feature".into(), cx));
    window.wait_until(cx, "comparison loaded", |_, cx| tab.read(cx).compared_files().is_some()).await;
    assert_eq!(cx.update(|cx| tab.read(cx).compared_files()).unwrap(), ["g.txt"]);
}
