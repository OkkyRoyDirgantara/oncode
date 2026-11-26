//! Editor tab drag-and-drop tests: reordering, moving between groups, and splitting a group via its edge.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{point, Entity, TestAppContext};
use oncode_ui::testing::{TestApp, TestWindow};
use oncode_ui::{file_tab, OpenOptions, Orientation, Workbench};

/// The tab titles per group, in group order.
fn layout(workbench: &Entity<Workbench>, cx: &mut TestAppContext) -> Vec<Vec<String>> {
    cx.update(|cx| workbench.read(cx).model().groups.iter().map(|g| g.tabs.iter().map(|t| t.title.clone()).collect()).collect())
}

fn group_ids(workbench: &Entity<Workbench>, cx: &mut TestAppContext) -> Vec<u32> {
    cx.update(|cx| workbench.read(cx).model().groups.iter().map(|g| g.id).collect())
}

/// The element id of a file's tab in that group.
fn tab_element(group: u32, path: &std::path::Path) -> String {
    format!("tab-{group}-{}", file_tab(&path.to_string_lossy()).id)
}

/// Drags from the center of element `from` to a relative point (`fx`, `fy` in 0..1) in element `to`.
fn drag_to_point(window: &TestWindow, cx: &mut TestAppContext, from: &str, to: &str, fx: f32, fy: f32) {
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        let from = window.find(from.to_string()).bounds().center();
        let to = window.find(to.to_string()).bounds();
        let target = point(to.origin.x + to.size.width * fx, to.origin.y + to.size.height * fy);
        window.drag(from, target, cx);
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
async fn tabs_are_dragged_to_split_move_and_reorder(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("a.txt", "a\n"), ("b.txt", "b\n")]);
    let (a, b) = (project.join("a.txt"), project.join("b.txt"));
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    window.update(cx, |window, cx| {
        workbench.update(cx, |w, cx| {
            w.open_tab(file_tab(&a.to_string_lossy()), OpenOptions::default(), window, cx);
            w.open_tab(file_tab(&b.to_string_lossy()), OpenOptions::default(), window, cx);
        })
    });
    assert_eq!(layout(&workbench, cx), [["a.txt", "b.txt"]]);

    // The right edge of the group content: a new group on the right containing that tab.
    drag_to_point(&window, cx, &tab_element(1, &a), "group-content-1", 0.95, 0.5);
    assert_eq!(layout(&workbench, cx), [vec!["b.txt"], vec!["a.txt"]]);
    assert_eq!(cx.update(|cx| workbench.read(cx).model().orientation), Orientation::Horizontal);
    let right = group_ids(&workbench, cx)[1];
    assert_eq!(cx.update(|cx| workbench.read(cx).model().active_group), right, "the target group is active");

    // The empty space of the right group's tab bar: the tab moves to its end, the emptied left group disappears.
    drag_to_point(&window, cx, &tab_element(1, &b), &format!("tabs-end-{right}"), 0.5, 0.5);
    assert_eq!(layout(&workbench, cx), [["a.txt", "b.txt"]]);

    // Over another tab: placed before that tab.
    drag_to_point(&window, cx, &tab_element(right, &b), &tab_element(right, &a), 0.3, 0.5);
    assert_eq!(layout(&workbench, cx), [["b.txt", "a.txt"]]);

    // The bottom edge with a single group: split downwards.
    drag_to_point(&window, cx, &tab_element(right, &a), &format!("group-content-{right}"), 0.5, 0.95);
    assert_eq!(layout(&workbench, cx), [vec!["b.txt"], vec!["a.txt"]]);
    assert_eq!(cx.update(|cx| workbench.read(cx).model().orientation), Orientation::Vertical);

    // The file content is still shown in its new group (the tab content moves along, it does not disappear).
    let bottom = group_ids(&workbench, cx)[1];
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        assert!(window.find(format!("group-content-{bottom}")).visible());
    });
    assert!(cx.update(|cx| workbench.read(cx).active_content().is_some()));
}
