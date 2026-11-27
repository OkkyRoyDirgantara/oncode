//! View tests for change markers in the editor gutter and for blame.

use std::time::{Duration, Instant};

use gpui_kit::component::input::Position;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{px, Bounds, Entity, Pixels, SharedString, TestAppContext};
use oncode_ui::editor::{active_file, open_file, FileView, OpenFile};
use oncode_ui::repos;
use oncode_ui::testing::{git, TestApp, TestWindow};
use oncode_ui_diff::gutter::ChangeKind;
use oncode_ui_diff::markers::change_markers;

#[gpui_kit::test]
async fn editor_shows_changes_against_the_index_and_blame(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_diff::init);
    let project = app.git_project(&[("a.txt", "one\ntwo\nthree\n")]);
    std::fs::write(project.join("a.txt"), "one\nTWO\nthree\nfour\n").unwrap();
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    window.wait_until(cx, "repository open", |_, cx| !repos(cx).read(cx).roots().is_empty()).await;
    let path = project.join("a.txt").to_string_lossy().into_owned();
    window.update(cx, |window, cx| workbench.update(cx, |w, cx| open_file(w, OpenFile::new(path.clone()), window, cx)));
    let markers = cx.update(|cx| change_markers(workbench.read(cx)).expect("markers installed"));

    // Line 2 changed, line 4 added.
    window
        .wait_until(cx, "markers computed", |_, cx| {
            markers.read(cx).marker_ranges(&path, cx).is_some_and(|ranges| ranges.iter().map(|(_, kind)| *kind).collect::<Vec<_>>() == [ChangeKind::Modified, ChangeKind::Added])
        })
        .await;

    // Once staged, there is no longer any difference from the index.
    git(&project, &["add", "a.txt"]);
    window.wait_until(cx, "markers disappear after staging", |_, cx| markers.read(cx).marker_ranges(&path, cx).is_some_and(|ranges| ranges.is_empty())).await;

    // Blame: on by default (inline blame), can be turned off and on per file.
    assert!(cx.update(|cx| active_file(workbench.read(cx), cx).is_some()));
    window.wait_until(cx, "blame loaded", |_, cx| markers.read(cx).blame_for(&path).is_some_and(|ranges| !ranges.is_empty())).await;
    let author = cx.update(|cx| markers.read(cx).blame_for(&path).unwrap()[0].author.clone());
    assert_eq!(author, "Tester");
    // Toggle Blame shows the blame column beside the editor, and hides it again.
    assert!(window.run_command(cx, "git.toggleBlame"));
    cx.run_until_parked();
    assert!(cx.update(|cx| markers.read(cx).has_blame_column(&path)));
    let shown = |window: &TestWindow, cx: &mut TestAppContext| {
        window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.try_find("annotation-column").is_some()
        })
    };
    window.wait_until(cx, "column has the blame", |_, cx| markers.read(cx).blame_column(&path).is_some_and(|column| !column.annotations.is_empty())).await;
    assert!(shown(&window, cx), "the blame column is drawn");
    let first = cx.update(|cx| markers.read(cx).blame_column(&path).unwrap().annotations[0].text.to_string());
    assert!(first.contains("Tester") || first == "Not committed yet", "{first}");
    assert!(window.run_command(cx, "git.toggleBlame"));
    cx.run_until_parked();
    assert!(!shown(&window, cx), "the blame column is hidden");
    assert!(cx.update(|cx| markers.read(cx).blame_for(&path).is_some()), "inline blame keeps its blame");
}

/// The end-of-line hint painted in the editor, after redrawing frames until `ok` is satisfied.
async fn painted_hint(
    window: &TestWindow,
    view: &Entity<FileView>,
    cx: &mut TestAppContext,
    what: &str,
    mut ok: impl FnMut(Option<&(SharedString, Bounds<Pixels>)>) -> bool,
) -> Option<(SharedString, Bounds<Pixels>)> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        window.update(cx, |window, cx| window.render_frame(cx));
        cx.run_until_parked();
        let hint = cx.update(|cx| view.read(cx).painted_line_hint());
        if ok(hint.as_ref()) {
            return hint;
        }
        assert!(Instant::now() < deadline, "waited too long: {what} (last: {hint:?})");
        cx.executor().timer(Duration::from_millis(20)).await;
    }
}

/// Inline blame like GitLens: author, time, and a faint commit message at the end of the cursor line.
#[gpui_kit::test]
async fn inline_blame_is_drawn_at_the_end_of_the_cursor_line(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_diff::init);
    let project = app.git_project(&[("a.txt", "a\nthe second line is much longer\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    window.wait_until(cx, "repository open", |_, cx| !repos(cx).read(cx).roots().is_empty()).await;
    let path = project.join("a.txt").to_string_lossy().into_owned();
    window.update(cx, |window, cx| workbench.update(cx, |w, cx| open_file(w, OpenFile::new(path.clone()), window, cx)));
    window.wait_until(cx, "a.txt loaded", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).is_ready())).await;
    let (view, _, state) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());

    // On without any configuration: the user's own commit is written as "You".
    let (text, first) = painted_hint(&window, &view, cx, "blame of line 1", |hint| hint.is_some()).await.unwrap();
    assert!(text.starts_with("You, ") && text.ends_with(" • First commit"), "{text}");

    // Move to the longer line: the hint moves down and shifts to the right.
    window.update(cx, |window, cx| state.update(cx, |state, cx| state.set_cursor_position(Position { line: 1, character: 3 }, window, cx)));
    let (_, second) = painted_hint(&window, &view, cx, "blame of line 2", |hint| hint.is_some_and(|(_, bounds)| bounds.origin.y > first.origin.y)).await.unwrap();
    assert!(second.origin.x > first.origin.x + px(100.), "at the end of the longer line: {first:?} → {second:?}");

    // A line that was just edited is not committed yet.
    window.update(cx, |window, cx| {
        state.update(cx, |state, cx| state.focus(window, cx));
        window.input("X", cx);
    });
    let (text, _) = painted_hint(&window, &view, cx, "edited line", |hint| hint.is_some_and(|(text, _)| text == "You • Uncommitted changes")).await.unwrap();
    assert_eq!(text, "You • Uncommitted changes");

    // Turned off via the Git menu → Toggle Inline Blame.
    assert!(window.run_command(cx, "git.toggleInlineBlame"));
    painted_hint(&window, &view, cx, "inline blame turned off", |hint| hint.is_none()).await;
    assert!(!app.core.user_settings().editor.inline_blame);
}

/// A file restored with the session when the folder opens can get its editor before the repository is known;
/// it still gets its change markers and blame once the repository shows up.
#[gpui_kit::test]
async fn an_editor_opened_before_its_repository_is_known_still_gets_blame(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_diff::init);
    let project = app.git_project(&[("a.txt", "one\n")]);
    let path = project.join("a.txt").to_string_lossy().into_owned();
    // The session restores a.txt as soon as the folder is open.
    app.core.store.set_session(&project.to_string_lossy(), serde_json::json!({ "files": [path.clone()], "active": path.clone() }).to_string());
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    window.wait_until(cx, "markers installed", |_, cx| change_markers(workbench.read(cx)).is_some()).await;
    let markers = cx.update(|cx| change_markers(workbench.read(cx)).unwrap());
    window.wait_until(cx, "a.txt restored", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).path() == path)).await;
    window.wait_until(cx, "blame loaded for the restored file", |_, cx| markers.read(cx).blame_for(&path).is_some_and(|ranges| !ranges.is_empty())).await;
}
