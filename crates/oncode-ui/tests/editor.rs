//! Uji tampilan editor: membuka folder dan berkas, menyunting, menyimpan, dan memuat ulang.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use oncode_ui::editor::{active_file, buffers, open_file, OpenFile};
use oncode_ui::testing::TestApp;

#[gpui_kit::test]
async fn opens_edits_and_saves_a_file(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("src/main.rs", "fn main() {}\n"), ("README.md", "# Demo\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let path = project.join("src/main.rs").to_string_lossy().into_owned();

    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&path), window, cx)));
    window.wait_until(cx, "berkas dimuat", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).is_ready())).await;

    let (_, buffer, state) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());
    assert_eq!(cx.update(|cx| state.read(cx).value().to_string()), "fn main() {}\n");
    assert!(!cx.update(|cx| buffer.read(cx).is_dirty()));

    // Mengetik lewat jalur masukan sungguhan: editor sudah fokus setelah tab dibuka.
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input("// catatan\n", cx);
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| state.read(cx).value().to_string()), "// catatan\nfn main() {}\n");
    assert!(cx.update(|cx| buffer.read(cx).is_dirty()), "buffer harus bertanda belum disimpan");
    assert!(cx.update(|cx| workbench.read(cx).has_dirty_tabs(cx)));

    // Ctrl+S (file.save) lewat registry command.
    assert!(window.run_command(cx, "file.save"));
    window.wait_until(cx, "tersimpan", |_, cx| !buffer.read(cx).is_dirty()).await;
    assert_eq!(std::fs::read_to_string(project.join("src/main.rs")).unwrap(), "// catatan\nfn main() {}\n");
}

#[gpui_kit::test]
async fn same_file_in_two_groups_stays_in_sync(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("a.txt", "satu\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let path = project.join("a.txt").to_string_lossy().into_owned();

    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&path), window, cx)));
    window.wait_until(cx, "berkas dimuat", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).is_ready())).await;
    let (_, _, first) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());

    assert!(window.run_command(cx, "workbench.split"));
    cx.run_until_parked();
    let (_, buffer, second) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());
    assert_ne!(first, second, "tiap grup punya state editor sendiri");
    assert_eq!(cx.update(|cx| workbench.read(cx).model().groups.len()), 2);
    assert_eq!(cx.update(|cx| second.read(cx).value().to_string()), "satu\n");

    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input("dua ", cx);
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| second.read(cx).value().to_string()), "dua satu\n");
    assert_eq!(cx.update(|cx| first.read(cx).value().to_string()), "dua satu\n", "suntingan diteruskan ke grup lain");
    assert!(cx.update(|cx| buffer.read(cx).is_dirty()));
}

#[gpui_kit::test]
async fn clean_buffer_reloads_when_the_file_changes_on_disk(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("a.txt", "lama\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let file = project.join("a.txt");
    let path = file.to_string_lossy().into_owned();

    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&path), window, cx)));
    window.wait_until(cx, "berkas dimuat", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).is_ready())).await;
    let (_, _, state) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());

    // Ukuran berubah, jadi perubahan terdeteksi walau mtime berbutir kasar.
    std::fs::write(&file, "baru dari luar\n").unwrap();
    window.wait_until(cx, "dimuat ulang", |_, cx| state.read(cx).value().as_ref() == "baru dari luar\n").await;
    assert!(cx.update(|cx| buffers(workbench.read(cx)).unwrap().read(cx).dirty(cx).is_empty()));
}
