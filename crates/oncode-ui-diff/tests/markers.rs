//! Uji tampilan penanda perubahan di gutter editor dan blame.

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
    let project = app.git_project(&[("a.txt", "satu\ndua\ntiga\n")]);
    std::fs::write(project.join("a.txt"), "satu\nDUA\ntiga\nempat\n").unwrap();
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    window.wait_until(cx, "repositori terbuka", |_, cx| !repos(cx).read(cx).roots().is_empty()).await;
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

    // Blame: menyala bawaan (inline blame), bisa dimatikan dan dinyalakan per berkas.
    assert!(cx.update(|cx| active_file(workbench.read(cx), cx).is_some()));
    window.wait_until(cx, "blame dimuat", |_, cx| markers.read(cx).blame_for(&path).is_some_and(|ranges| !ranges.is_empty())).await;
    let author = cx.update(|cx| markers.read(cx).blame_for(&path).unwrap()[0].author.clone());
    assert_eq!(author, "Penguji");
    assert!(window.run_command(cx, "git.toggleBlame"));
    cx.run_until_parked();
    assert!(cx.update(|cx| markers.read(cx).blame_for(&path).is_none()), "blame dimatikan");
    assert!(window.run_command(cx, "git.toggleBlame"));
    window.wait_until(cx, "blame dinyalakan lagi", |_, cx| markers.read(cx).blame_for(&path).is_some_and(|ranges| !ranges.is_empty())).await;
}

/// Hint ujung baris yang tergambar di editor, setelah frame digambar ulang sampai `ok` terpenuhi.
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
        assert!(Instant::now() < deadline, "menunggu terlalu lama: {what} (terakhir: {hint:?})");
        cx.executor().timer(Duration::from_millis(20)).await;
    }
}

/// Inline blame seperti GitLens: penulis, waktu, dan pesan commit samar di ujung baris kursor.
#[gpui_kit::test]
async fn inline_blame_is_drawn_at_the_end_of_the_cursor_line(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_diff::init);
    let project = app.git_project(&[("a.txt", "a\nbaris kedua yang jauh lebih panjang\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    window.wait_until(cx, "repositori terbuka", |_, cx| !repos(cx).read(cx).roots().is_empty()).await;
    let path = project.join("a.txt").to_string_lossy().into_owned();
    window.update(cx, |window, cx| workbench.update(cx, |w, cx| open_file(w, OpenFile::new(path.clone()), window, cx)));
    window.wait_until(cx, "a.txt dimuat", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).is_ready())).await;
    let (view, _, state) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());

    // Menyala tanpa diatur: commit milik pengguna sendiri ditulis "You".
    let (text, first) = painted_hint(&window, &view, cx, "blame baris 1", |hint| hint.is_some()).await.unwrap();
    assert!(text.starts_with("You, ") && text.ends_with(" • Commit pertama"), "{text}");

    // Pindah ke baris yang lebih panjang: hint ikut turun dan bergeser ke kanan.
    window.update(cx, |window, cx| state.update(cx, |state, cx| state.set_cursor_position(Position { line: 1, character: 3 }, window, cx)));
    let (_, second) = painted_hint(&window, &view, cx, "blame baris 2", |hint| hint.is_some_and(|(_, bounds)| bounds.origin.y > first.origin.y)).await.unwrap();
    assert!(second.origin.x > first.origin.x + px(100.), "di ujung baris yang lebih panjang: {first:?} → {second:?}");

    // Baris yang baru disunting belum di-commit.
    window.update(cx, |window, cx| {
        state.update(cx, |state, cx| state.focus(window, cx));
        window.input("X", cx);
    });
    let (text, _) = painted_hint(&window, &view, cx, "baris yang disunting", |hint| hint.is_some_and(|(text, _)| text == "You • Uncommitted changes")).await.unwrap();
    assert_eq!(text, "You • Uncommitted changes");

    // Dimatikan lewat menu Git → Toggle Inline Blame.
    assert!(window.run_command(cx, "git.toggleInlineBlame"));
    painted_hint(&window, &view, cx, "inline blame dimatikan", |hint| hint.is_none()).await;
    assert!(!app.core.user_settings().editor.inline_blame);
}
