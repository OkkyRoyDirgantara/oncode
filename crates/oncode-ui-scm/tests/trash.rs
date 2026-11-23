//! Uji membuang berkas untracked lewat tombol baris: berkas dipindah ke tempat sampah, tidak
//! dihapus permanen. Biner uji tersendiri karena `XDG_DATA_HOME` diarahkan ke folder sementara,
//! supaya tempat sampah pengguna tidak tersentuh.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use oncode_ui::repos;
use oncode_ui::testing::TestApp;

#[gpui_kit::test]
async fn untracked_file_goes_to_the_trash_after_confirming(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_DATA_HOME", data.path());
    let app = TestApp::new(cx);
    cx.update(oncode_ui_scm::init);
    let project = app.git_project(&[("a.txt", "a\n")]);
    std::fs::write(project.join("sampah.txt"), "tidak perlu\n").unwrap();
    let window = app.open(Some(&project), cx).await;
    let root = project.to_string_lossy().into_owned();
    window.wait_until(cx, "berkas untracked terbaca", move |_, cx| repos(cx).read(cx).snapshot(&root).is_some_and(|s| s.untracked.len() == 1)).await;
    assert!(window.run_command(cx, "scm.focus"));
    cx.run_until_parked();

    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click("scm-discard-sampah.txt", cx);
    });
    cx.run_until_parked();
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click("ok", cx);
    });
    let gone = project.join("sampah.txt");
    window.wait_until(cx, "berkas dipindah", |_, _| !gone.exists()).await;
    let trashed = data.path().join("Trash/files/sampah.txt");
    assert_eq!(std::fs::read_to_string(trashed).unwrap(), "tidak perlu\n", "isinya bisa dipulihkan dari tempat sampah");
    assert!(project.join("a.txt").exists());
}
