//! Tests for files dropped from the OS onto the editor area: opened as tabs, or splitting a group via
//! its edge. Files outside the project and folders are rejected. Dragging from the Explorer uses the same path
//! (`DraggedEntry`) and is tested in `oncode-ui-explorer`.

use std::path::Path;

use gpui_kit::component::WindowExt as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{point, Entity, ExternalPaths, FileDropEvent, InputEvent as _, TestAppContext};
use oncode_ui::testing::{TestApp, TestWindow};
use oncode_ui::{file_tab, Orientation, Workbench};

/// The tab titles per group, in group order.
fn layout(workbench: &Entity<Workbench>, cx: &mut TestAppContext) -> Vec<Vec<String>> {
    cx.update(|cx| workbench.read(cx).model().groups.iter().map(|g| g.tabs.iter().map(|t| t.title.clone()).collect()).collect())
}

fn group_ids(workbench: &Entity<Workbench>, cx: &mut TestAppContext) -> Vec<u32> {
    cx.update(|cx| workbench.read(cx).model().groups.iter().map(|g| g.id).collect())
}

/// The element id of the empty space in a group's tab bar.
fn tabs_end(group: u32) -> String {
    format!("tabs-end-{group}")
}

/// The element id of a file's tab in that group.
fn tab_element(group: u32, path: &Path) -> String {
    format!("tab-{group}-{}", file_tab(&path.to_string_lossy()).id)
}

/// Drops `paths` from the OS at a relative point (`fx`, `fy` in 0..1) in element `to`, through the same
/// sequence of events as the platform: enter, move, then release.
fn drop_from_os(window: &TestWindow, cx: &mut TestAppContext, paths: &[&Path], to: &str, fx: f32, fy: f32) {
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        let bounds = window.find(to.to_string()).bounds();
        let position = point(bounds.origin.x + bounds.size.width * fx, bounds.origin.y + bounds.size.height * fy);
        let external = ExternalPaths(paths.iter().map(|path| path.to_path_buf()).collect());
        window.dispatch_event(FileDropEvent::Entered { position, paths: external }.to_platform_input(), cx);
        window.render_frame(cx);
        window.dispatch_event(FileDropEvent::Pending { position }.to_platform_input(), cx);
        window.render_frame(cx);
        window.dispatch_event(FileDropEvent::Submit { position }.to_platform_input(), cx);
        window.render_frame(cx);
        window.dispatch_event(FileDropEvent::Ended.to_platform_input(), cx);
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
async fn files_dropped_from_the_os_open_as_tabs_and_split_the_editor(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("a.txt", "a\n"), ("b.txt", "b\n"), ("c.txt", "c\n"), ("src/inner.txt", "i\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    assert!(layout(&workbench, cx).iter().all(Vec::is_empty), "no tabs yet");

    // An empty editor area: the file is opened.
    drop_from_os(&window, cx, &[&project.join("a.txt")], "editor-empty", 0.5, 0.5);
    assert_eq!(layout(&workbench, cx), [["a.txt"]]);
    assert!(cx.update(|cx| workbench.read(cx).active_content().is_some()), "the tab content is built");

    // Several files at once in the middle of the group content: all become tabs, the first is active at the end.
    drop_from_os(&window, cx, &[&project.join("b.txt"), &project.join("c.txt")], "group-content-1", 0.5, 0.5);
    assert_eq!(layout(&workbench, cx), [["a.txt", "b.txt", "c.txt"]]);
    assert_eq!(cx.update(|cx| workbench.read(cx).model().active().map(|(_, tab)| tab.title.clone())), Some("c.txt".into()), "the last file is active");

    // The right edge of the group content: a new group on the right containing that file.
    drop_from_os(&window, cx, &[&project.join("src/inner.txt")], "group-content-1", 0.95, 0.5);
    assert_eq!(layout(&workbench, cx), [vec!["a.txt", "b.txt", "c.txt"], vec!["inner.txt"]]);
    assert_eq!(cx.update(|cx| workbench.read(cx).model().orientation), Orientation::Horizontal);
    let right = group_ids(&workbench, cx)[1];
    assert_eq!(cx.update(|cx| workbench.read(cx).model().active_group), right, "the target group is active");

    // A file that is already open is moved to the target group (not duplicated), like dragging its tab.
    drop_from_os(&window, cx, &[&project.join("a.txt")], &tabs_end(right), 0.5, 0.5);
    assert_eq!(layout(&workbench, cx), [vec!["b.txt", "c.txt"], vec!["inner.txt", "a.txt"]]);

    // Over a tab: placed before that tab.
    drop_from_os(&window, cx, &[&project.join("c.txt")], &tab_element(right, &project.join("src/inner.txt")), 0.3, 0.5);
    assert_eq!(layout(&workbench, cx), [vec!["b.txt"], vec!["c.txt", "inner.txt", "a.txt"]]);
}

#[gpui_kit::test]
async fn files_dropped_without_an_open_folder_only_explain_why_nothing_opened(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let elsewhere = app.project(&[("x.txt", "x\n")]);
    let window = app.open(None, cx).await;
    let workbench = window.workbench(cx);

    drop_from_os(&window, cx, &[&elsewhere.join("x.txt")], "editor-empty", 0.5, 0.5);
    assert!(layout(&workbench, cx).iter().all(Vec::is_empty), "without a folder, files cannot be opened");
    assert_eq!(window.update(cx, |window, cx| window.notifications(cx).len()), 1, "the user is notified");
}

#[gpui_kit::test]
async fn files_outside_the_project_and_folders_are_not_opened(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("a.txt", "a\n"), ("src/inner.txt", "i\n")]);
    let elsewhere = app.project(&[("outside.txt", "x\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    drop_from_os(&window, cx, &[&project.join("a.txt")], "editor-empty", 0.5, 0.5);
    assert_eq!(layout(&workbench, cx), [["a.txt"]]);

    // All file access is restricted to the project folder; other files cannot be opened.
    drop_from_os(&window, cx, &[&elsewhere.join("outside.txt")], "group-content-1", 0.5, 0.5);
    assert_eq!(layout(&workbench, cx), [["a.txt"]]);

    // A folder is not opened as a tab, and does not split the group even when dropped on an edge.
    drop_from_os(&window, cx, &[&project.join("src")], "group-content-1", 0.95, 0.5);
    assert_eq!(layout(&workbench, cx), [["a.txt"]]);

    // A mix: only the valid files are opened; one that is rejected does not make the others fail.
    drop_from_os(&window, cx, &[&project.join("src"), &project.join("src/inner.txt"), &elsewhere.join("outside.txt")], "group-content-1", 0.5, 0.5);
    assert_eq!(layout(&workbench, cx), [["a.txt", "inner.txt"]]);
}
