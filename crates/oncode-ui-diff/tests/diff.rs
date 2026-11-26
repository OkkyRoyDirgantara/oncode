//! Diff tab view tests: stage/unstage/discard per change and per line, unified view,
//! and the fallback view. The end result is checked with git, not just from the view state.

use std::path::Path;

use gpui_kit::component::diff::{DiffLineRange, DiffMode, DiffSide};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext};
use oncode_git::DiffSource;
use oncode_ui::testing::{git, TestApp, TestWindow};
use oncode_ui::{repos, ViewTab, Workbench};
use oncode_ui_diff::{open_diff, DiffRequest, DiffView};

const README: &str = "# Demo\n\nLine one\nLine two\nLine three\n";
const README_CHANGED: &str = "# Demo\n\nLine ONE\nLine two\nLine three\nLine four\n";

/// Lines that changed in the output of `git diff -U0`.
fn changed_lines(diff: &str) -> Vec<String> {
    diff.lines().filter(|line| (line.starts_with('+') || line.starts_with('-')) && !line.starts_with("+++") && !line.starts_with("---")).map(String::from).collect()
}

async fn open(app: &TestApp, project: &Path, cx: &mut TestAppContext) -> (TestWindow, Entity<Workbench>, String) {
    cx.update(|cx| oncode_ui_diff::init(cx));
    let window = app.open(Some(project), cx).await;
    let workbench = window.workbench(cx);
    let root = project.to_string_lossy().into_owned();
    window.wait_until(cx, "repository open", |_, cx| repos(cx).read(cx).snapshot(&root).is_some()).await;
    (window, workbench, root)
}

fn active_diff(workbench: &Entity<Workbench>, cx: &gpui_kit::App) -> Option<Entity<DiffView>> {
    workbench.read(cx).active_content()?.as_any().downcast_ref::<ViewTab<DiffView>>().map(|tab| tab.0.clone())
}

/// Opens a diff tab and waits until its content is loaded.
async fn open_diff_tab(window: &TestWindow, workbench: &Entity<Workbench>, root: &str, path: &str, source: DiffSource, cx: &mut TestAppContext) -> Entity<DiffView> {
    let request = DiffRequest { root: root.to_string(), path: path.to_string(), orig_path: None, source, preview: false };
    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_diff(workbench, request, window, cx)));
    window.wait_until(cx, "diff loaded", |_, cx| active_diff(workbench, cx).is_some_and(|view| view.read(cx).is_loaded())).await;
    cx.update(|cx| active_diff(workbench, cx).unwrap())
}

fn click(window: &TestWindow, cx: &mut TestAppContext, id: &str) {
    let id = id.to_string();
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click(gpui_kit::SharedString::from(id), cx);
    });
}

#[gpui_kit::test]
async fn stages_one_change_and_keeps_the_other(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.git_project(&[("README.md", README)]);
    std::fs::write(project.join("README.md"), README_CHANGED).unwrap();
    let (window, workbench, root) = open(&app, &project, cx).await;

    let view = open_diff_tab(&window, &workbench, &root, "README.md", DiffSource::Worktree, cx).await;
    assert_eq!(cx.update(|cx| view.read(cx).change_count()), 2, "two changes with action buttons");
    let tab = cx.update(|cx| workbench.read(cx).model().active().map(|(_, tab)| (tab.title.clone(), tab.kind.clone())).unwrap());
    assert_eq!(tab, ("README.md (Working Tree)".to_string(), "diff".to_string()));

    click(&window, cx, "diff-stage-0");
    window.wait_until(cx, "first change enters the index", |_, _| !git(&project, &["diff", "--cached", "--name-only"]).trim().is_empty()).await;
    assert_eq!(changed_lines(&git(&project, &["diff", "--cached", "-U0"])), ["-Line one", "+Line ONE"], "only the first change enters the index");
    assert_eq!(changed_lines(&git(&project, &["diff", "-U0"])), ["+Line four"], "the second change stays unstaged");
    window.wait_until(cx, "diff is down to one change", |_, cx| view.read(cx).change_count() == 1).await;

    // The remaining change can be committed separately.
    git(&project, &["commit", "-q", "-m", "Change line one"]);
    assert_eq!(git(&project, &["show", "HEAD:README.md"]), "# Demo\n\nLine ONE\nLine two\nLine three\n");
}

#[gpui_kit::test]
async fn unstages_one_change_then_discards_the_other_after_confirming(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.git_project(&[("README.md", README)]);
    std::fs::write(project.join("README.md"), README_CHANGED).unwrap();
    git(&project, &["add", "README.md"]);
    let (window, workbench, root) = open(&app, &project, cx).await;

    // Staged diff: remove the first change from the index.
    let staged = open_diff_tab(&window, &workbench, &root, "README.md", DiffSource::Staged, cx).await;
    assert_eq!(cx.update(|cx| staged.read(cx).change_count()), 2);
    click(&window, cx, "diff-unstage-0");
    window.wait_until(cx, "first change leaves the index", |_, _| changed_lines(&git(&project, &["diff", "-U0"])) == ["-Line one", "+Line ONE"]).await;
    assert_eq!(changed_lines(&git(&project, &["diff", "--cached", "-U0"])), ["+Line four"]);

    // Working tree diff: discard that change; canceled first, then confirmed.
    let worktree = open_diff_tab(&window, &workbench, &root, "README.md", DiffSource::Worktree, cx).await;
    window.wait_until(cx, "working tree diff", |_, cx| worktree.read(cx).change_count() == 1).await;
    click(&window, cx, "diff-discard-0");
    cx.run_until_parked();
    click(&window, cx, "cancel");
    cx.run_until_parked();
    assert!(std::fs::read_to_string(project.join("README.md")).unwrap().contains("Line ONE"), "canceled: the file is unchanged");
    click(&window, cx, "diff-discard-0");
    cx.run_until_parked();
    click(&window, cx, "ok");
    window.wait_until(cx, "change discarded", |_, _| std::fs::read_to_string(project.join("README.md")).is_ok_and(|t| !t.contains("Line ONE"))).await;
    assert_eq!(std::fs::read_to_string(project.join("README.md")).unwrap(), "# Demo\n\nLine one\nLine two\nLine three\nLine four\n", "the staged change remains");
}

#[gpui_kit::test]
async fn stages_selected_lines_and_switches_to_unified(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.git_project(&[("a.txt", "one\n")]);
    std::fs::write(project.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    let (window, workbench, root) = open(&app, &project, cx).await;
    let view = open_diff_tab(&window, &workbench, &root, "a.txt", DiffSource::Worktree, cx).await;

    // Select only the line "two" (line 2 of the new side), then stage the selection.
    cx.update(|cx| {
        let state = view.read(cx).state().clone();
        state.update(cx, |state, cx| state.set_selected_lines(Some(DiffLineRange::new(oncode_ui_diff::diff_tab::PATCH_PATH, DiffSide::Modified, 2, 2)), cx));
    });
    click(&window, cx, "diff-selected-stage");
    window.wait_until(cx, "one line enters the index", |_, _| git(&project, &["show", ":a.txt"]) == "one\ntwo\n").await;
    assert_eq!(changed_lines(&git(&project, &["diff", "-U0"])), ["+three"]);

    // Switch between side-by-side ↔ unified view.
    let before = cx.update(|cx| view.read(cx).mode(cx));
    click(&window, cx, "diff-toggle-layout");
    cx.run_until_parked();
    let after = cx.update(|cx| view.read(cx).mode(cx));
    assert_ne!(before, after);
    assert!(matches!(after, DiffMode::Unified | DiffMode::Split));
}
