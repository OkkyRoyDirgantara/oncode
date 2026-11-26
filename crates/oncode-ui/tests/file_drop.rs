//! Uji berkas yang dijatuhkan dari OS ke area editor: dibuka sebagai tab, atau membelah grup lewat
//! tepinya. Berkas di luar proyek dan folder ditolak. Seret dari Explorer memakai jalur yang sama
//! (`DraggedEntry`) dan diuji di `oncode-ui-explorer`.

use std::path::Path;

use gpui_kit::component::WindowExt as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{point, Entity, ExternalPaths, FileDropEvent, InputEvent as _, TestAppContext};
use oncode_ui::testing::{TestApp, TestWindow};
use oncode_ui::{file_tab, Orientation, Workbench};

/// Judul tab per grup, menurut urutan grup.
fn layout(workbench: &Entity<Workbench>, cx: &mut TestAppContext) -> Vec<Vec<String>> {
    cx.update(|cx| workbench.read(cx).model().groups.iter().map(|g| g.tabs.iter().map(|t| t.title.clone()).collect()).collect())
}

fn group_ids(workbench: &Entity<Workbench>, cx: &mut TestAppContext) -> Vec<u32> {
    cx.update(|cx| workbench.read(cx).model().groups.iter().map(|g| g.id).collect())
}

/// Id elemen ruang kosong di bilah tab sebuah grup.
fn tabs_end(group: u32) -> String {
    format!("tabs-end-{group}")
}

/// Id elemen tab sebuah berkas di grup itu.
fn tab_element(group: u32, path: &Path) -> String {
    format!("tab-{group}-{}", file_tab(&path.to_string_lossy()).id)
}

/// Menjatuhkan `paths` dari OS ke titik relatif (`fx`, `fy` dalam 0..1) di elemen `to`, lewat urutan
/// peristiwa yang sama dengan platform: masuk, bergerak, lalu lepas.
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
    assert!(layout(&workbench, cx).iter().all(Vec::is_empty), "belum ada tab");

    // Area editor kosong: berkasnya dibuka.
    drop_from_os(&window, cx, &[&project.join("a.txt")], "editor-empty", 0.5, 0.5);
    assert_eq!(layout(&workbench, cx), [["a.txt"]]);
    assert!(cx.update(|cx| workbench.read(cx).active_content().is_some()), "isi tab dibangun");

    // Beberapa berkas sekaligus di tengah isi grup: semuanya jadi tab, yang pertama aktif di ujung.
    drop_from_os(&window, cx, &[&project.join("b.txt"), &project.join("c.txt")], "group-content-1", 0.5, 0.5);
    assert_eq!(layout(&workbench, cx), [["a.txt", "b.txt", "c.txt"]]);
    assert_eq!(cx.update(|cx| workbench.read(cx).model().active().map(|(_, tab)| tab.title.clone())), Some("c.txt".into()), "berkas terakhir aktif");

    // Tepi kanan isi grup: grup baru di kanan berisi berkas itu.
    drop_from_os(&window, cx, &[&project.join("src/inner.txt")], "group-content-1", 0.95, 0.5);
    assert_eq!(layout(&workbench, cx), [vec!["a.txt", "b.txt", "c.txt"], vec!["inner.txt"]]);
    assert_eq!(cx.update(|cx| workbench.read(cx).model().orientation), Orientation::Horizontal);
    let right = group_ids(&workbench, cx)[1];
    assert_eq!(cx.update(|cx| workbench.read(cx).model().active_group), right, "grup tujuan aktif");

    // Berkas yang sudah terbuka dipindah ke grup tujuan (tidak digandakan), seperti menyeret tabnya.
    drop_from_os(&window, cx, &[&project.join("a.txt")], &tabs_end(right), 0.5, 0.5);
    assert_eq!(layout(&workbench, cx), [vec!["b.txt", "c.txt"], vec!["inner.txt", "a.txt"]]);

    // Di atas sebuah tab: ditaruh sebelum tab itu.
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
    assert!(layout(&workbench, cx).iter().all(Vec::is_empty), "tanpa folder, berkas tidak bisa dibuka");
    assert_eq!(window.update(cx, |window, cx| window.notifications(cx).len()), 1, "pengguna diberi tahu");
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

    // Semua akses berkas dibatasi ke folder proyek; berkas lain tidak bisa dibuka.
    drop_from_os(&window, cx, &[&elsewhere.join("outside.txt")], "group-content-1", 0.5, 0.5);
    assert_eq!(layout(&workbench, cx), [["a.txt"]]);

    // Folder tidak dibuka sebagai tab, dan tidak membelah grup walau dijatuhkan di tepi.
    drop_from_os(&window, cx, &[&project.join("src")], "group-content-1", 0.95, 0.5);
    assert_eq!(layout(&workbench, cx), [["a.txt"]]);

    // Campuran: hanya berkas yang sah yang dibuka; satu yang ditolak tidak menggagalkan yang lain.
    drop_from_os(&window, cx, &[&project.join("src"), &project.join("src/inner.txt"), &elsewhere.join("outside.txt")], "group-content-1", 0.5, 0.5);
    assert_eq!(layout(&workbench, cx), [["a.txt", "inner.txt"]]);
}
