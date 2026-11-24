//! Uji seret-lepas tab editor: urutan ulang, pindah grup, dan membelah grup lewat tepinya.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{point, Entity, TestAppContext};
use oncode_ui::testing::{TestApp, TestWindow};
use oncode_ui::{file_tab, OpenOptions, Orientation, Workbench};

/// Judul tab per grup, menurut urutan grup.
fn layout(workbench: &Entity<Workbench>, cx: &mut TestAppContext) -> Vec<Vec<String>> {
    cx.update(|cx| workbench.read(cx).model().groups.iter().map(|g| g.tabs.iter().map(|t| t.title.clone()).collect()).collect())
}

fn group_ids(workbench: &Entity<Workbench>, cx: &mut TestAppContext) -> Vec<u32> {
    cx.update(|cx| workbench.read(cx).model().groups.iter().map(|g| g.id).collect())
}

/// Id elemen tab sebuah berkas di grup itu.
fn tab_element(group: u32, path: &std::path::Path) -> String {
    format!("tab-{group}-{}", file_tab(&path.to_string_lossy()).id)
}

/// Menyeret dari tengah elemen `from` ke titik relatif (`fx`, `fy` dalam 0..1) di elemen `to`.
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

    // Tepi kanan isi grup: grup baru di kanan berisi tab itu.
    drag_to_point(&window, cx, &tab_element(1, &a), "group-content-1", 0.95, 0.5);
    assert_eq!(layout(&workbench, cx), [vec!["b.txt"], vec!["a.txt"]]);
    assert_eq!(cx.update(|cx| workbench.read(cx).model().orientation), Orientation::Horizontal);
    let right = group_ids(&workbench, cx)[1];
    assert_eq!(cx.update(|cx| workbench.read(cx).model().active_group), right, "grup tujuan aktif");

    // Ruang kosong bilah tab grup kanan: tab pindah ke ujungnya, grup kiri yang kosong hilang.
    drag_to_point(&window, cx, &tab_element(1, &b), &format!("tabs-end-{right}"), 0.5, 0.5);
    assert_eq!(layout(&workbench, cx), [["a.txt", "b.txt"]]);

    // Di atas tab lain: ditaruh sebelum tab itu.
    drag_to_point(&window, cx, &tab_element(right, &b), &tab_element(right, &a), 0.3, 0.5);
    assert_eq!(layout(&workbench, cx), [["b.txt", "a.txt"]]);

    // Tepi bawah dengan satu grup: belah ke bawah.
    drag_to_point(&window, cx, &tab_element(right, &a), &format!("group-content-{right}"), 0.5, 0.95);
    assert_eq!(layout(&workbench, cx), [vec!["b.txt"], vec!["a.txt"]]);
    assert_eq!(cx.update(|cx| workbench.read(cx).model().orientation), Orientation::Vertical);

    // Isi berkas tetap tampil di grup barunya (isi tab ikut pindah, tidak hilang).
    let bottom = group_ids(&workbench, cx)[1];
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        assert!(window.find(format!("group-content-{bottom}")).visible());
    });
    assert!(cx.update(|cx| workbench.read(cx).active_content().is_some()));
}
