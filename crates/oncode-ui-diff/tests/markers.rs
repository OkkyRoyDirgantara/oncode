//! Uji tampilan penanda perubahan di gutter editor dan blame.

use gpui_kit::TestAppContext;
use oncode_ui::editor::{active_file, open_file, OpenFile};
use oncode_ui::repos;
use oncode_ui::testing::{git, TestApp};
use oncode_ui_diff::gutter::ChangeKind;
use oncode_ui_diff::markers::change_markers;

#[gpui_kit::test]
async fn editor_shows_changes_against_the_index_and_blame(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_diff::init);
    let project = app.git_project(&[("a.txt", "satu\ndua\ntiga\n")]);
    std::fs::write(project.join("a.txt"), "satu\nDUA\ntiga\nempat\n").unwrap();
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let root = project.to_string_lossy().into_owned();
    window.wait_until(cx, "repositori terbuka", |_, cx| repos(cx).read(cx).snapshot(&root).is_some()).await;
    let path = project.join("a.txt").to_string_lossy().into_owned();
    window.update(cx, |window, cx| workbench.update(cx, |w, cx| open_file(w, OpenFile::new(path.clone()), window, cx)));
    let markers = cx.update(|cx| change_markers(workbench.read(cx)).expect("penanda terpasang"));

    // Baris 2 diubah, baris 4 ditambah.
    window
        .wait_until(cx, "penanda dihitung", |_, cx| {
            markers.read(cx).marker_ranges(&path, cx).is_some_and(|ranges| ranges.iter().map(|(_, kind)| *kind).collect::<Vec<_>>() == [ChangeKind::Modified, ChangeKind::Added])
        })
        .await;

    // Setelah di-stage, tidak ada lagi perbedaan dengan index.
    git(&project, &["add", "a.txt"]);
    window.wait_until(cx, "penanda hilang setelah stage", |_, cx| markers.read(cx).marker_ranges(&path, cx).is_some_and(|ranges| ranges.is_empty())).await;

    // Blame: dinyalakan lewat command, lalu tersedia untuk berkas aktif.
    assert!(cx.update(|cx| active_file(workbench.read(cx), cx).is_some()));
    assert!(window.run_command(cx, "git.toggleBlame"));
    window.wait_until(cx, "blame dimuat", |_, cx| markers.read(cx).blame_for(&path).is_some_and(|ranges| !ranges.is_empty())).await;
    let author = cx.update(|cx| markers.read(cx).blame_for(&path).unwrap()[0].author.clone());
    assert_eq!(author, "Penguji");
    assert!(window.run_command(cx, "git.toggleBlame"));
    cx.run_until_parked();
    assert!(cx.update(|cx| markers.read(cx).blame_for(&path).is_none()), "blame dimatikan lagi");
}
