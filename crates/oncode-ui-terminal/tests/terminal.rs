//! Uji tampilan terminal: shell sungguhan di PTY, masukan lewat jalur tombol jendela.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use oncode_ui::testing::TestApp;

#[gpui_kit::test]
async fn terminal_runs_a_command_in_the_project_folder(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_terminal::init);
    let project = app.project(&[("a.txt", "a\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);

    assert!(window.run_command(cx, "terminal.toggle"));
    cx.run_until_parked();
    let panel = cx.update(|cx| oncode_ui_terminal::panel(workbench.read(cx)).unwrap());
    window.wait_until(cx, "shell berjalan", |_, cx| panel.read(cx).active_session().is_some_and(|s| s.read(cx).is_running())).await;
    assert_eq!(cx.update(|cx| workbench.read(cx).active_panel().map(|p| p.to_string())).as_deref(), Some("terminal"));

    // Ketikan sampai ke shell lewat penangan tombol terminal (bukan pintasan aplikasi).
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input("echo ok > out.txt", cx);
        window.press("enter", cx);
    });
    let out = project.join("out.txt");
    window.wait_until(cx, "berkas ditulis shell", |_, _| std::fs::read_to_string(&out).is_ok_and(|text| text == "ok\n")).await;

    // Keluaran program tampil di emulator, termasuk warna ANSI yang diurai.
    window.update(cx, |window, cx| {
        window.input("printf 'tanda-%s\\n' selesai", cx);
        window.press("enter", cx);
    });
    let session = cx.update(|cx| panel.read(cx).active_session().unwrap().clone());
    window.wait_until(cx, "keluaran tampil", |_, cx| session.read(cx).text().lines().any(|line| line.trim() == "tanda-selesai")).await;
}

#[gpui_kit::test]
async fn shell_keys_are_not_taken_by_app_shortcuts(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_terminal::init);
    let project = app.project(&[("a.txt", "a\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let path = project.join("a.txt").to_string_lossy().into_owned();
    window.update(cx, |window, cx| workbench.update(cx, |w, cx| oncode_ui::editor::open_file(w, oncode_ui::editor::OpenFile::new(path), window, cx)));
    assert!(window.run_command(cx, "terminal.toggle"));
    let panel = cx.update(|cx| oncode_ui_terminal::panel(workbench.read(cx)).unwrap());
    window.wait_until(cx, "shell berjalan", |_, cx| panel.read(cx).active_session().is_some_and(|s| s.read(cx).is_running())).await;

    // Ctrl+W di terminal milik shell (hapus kata), bukan "Close Editor".
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press("ctrl-w", cx);
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| workbench.read(cx).model().all_tabs().count()), 1, "tab editor tidak tertutup");

    // Pintasan yang diizinkan (Toggle Panel) tetap sampai ke aplikasi.
    window.update(cx, |window, cx| window.press("ctrl-j", cx));
    cx.run_until_parked();
    assert!(cx.update(|cx| !workbench.read(cx).is_panel_visible()));
}

#[gpui_kit::test]
async fn tab_and_shift_tab_stay_in_the_terminal(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_terminal::init);
    let project = app.project(&[("a.txt", "a\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    assert!(window.run_command(cx, "terminal.toggle"));
    let panel = cx.update(|cx| oncode_ui_terminal::panel(workbench.read(cx)).unwrap());
    window.wait_until(cx, "shell berjalan", |_, cx| panel.read(cx).active_session().is_some_and(|s| s.read(cx).is_running())).await;
    let session = cx.update(|cx| panel.read(cx).active_session().unwrap().clone());

    // Tab (pelengkapan) dan Shift+Tab milik shell: fokus tidak pindah ke kontrol lain (mis. tombol branch).
    for key in ["tab", "shift-tab"] {
        let focused = window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
            session.read(cx).focus_handle_is_focused(window)
        });
        assert!(focused, "{key} memindahkan fokus keluar dari terminal");
    }
}
