//! Uji tampilan terminal: shell sungguhan di PTY, masukan lewat jalur tombol jendela.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use oncode_ui::testing::TestApp;

/// Isi berkas yang ditulis redireksi shell: PowerShell 5 menulis UTF-16 ber-BOM, PowerShell dan cmd
/// memakai CRLF, shell Unix UTF-8 dengan LF.
fn shell_text(bytes: &[u8]) -> String {
    match bytes {
        [0xFF, 0xFE, rest @ ..] => String::from_utf16_lossy(&rest.chunks_exact(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]])).collect::<Vec<_>>()),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

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
    window.wait_until(cx, "berkas ditulis shell", |_, _| std::fs::read(&out).is_ok_and(|bytes| shell_text(&bytes).trim_end() == "ok")).await;

    // Keluaran program tampil di emulator. `echo` ada di sh, cmd, dan PowerShell; baris perintah
    // yang diketik (`echo tanda-selesai`) tidak sama persis dengan baris keluarannya.
    window.update(cx, |window, cx| {
        window.input("echo tanda-selesai", cx);
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
    window.update(cx, |window, cx| window.press("secondary-j", cx));
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

#[gpui_kit::test]
async fn more_terminals_open_from_the_shortcut_and_the_panel_button(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_terminal::init);
    let project = app.project(&[("a.txt", "a\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    assert!(window.run_command(cx, "terminal.toggle"));
    let panel = cx.update(|cx| oncode_ui_terminal::panel(workbench.read(cx)).unwrap());
    window.wait_until(cx, "terminal pertama berjalan", |_, cx| panel.read(cx).active_session().is_some_and(|s| s.read(cx).is_running())).await;

    // Ctrl+Shift+` (Cmd di macOS) seperti yang dikirim GPUI di Windows: Ctrl + "~", Shift sudah menjadi karakter.
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press("secondary-~", cx);
    });
    cx.run_until_parked();
    window.wait_until(cx, "terminal kedua dari pintasan", |_, cx| panel.read(cx).sessions().len() == 2).await;

    // Tombol "+" di bilah tab panel.
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click("terminal-new", cx);
    });
    cx.run_until_parked();
    window.wait_until(cx, "terminal ketiga dari tombol", |_, cx| panel.read(cx).sessions().len() == 3).await;
    window.wait_until(cx, "semua terminal berjalan", |_, cx| panel.read(cx).sessions().iter().all(|s| s.read(cx).is_running())).await;
}

#[gpui_kit::test]
async fn mouse_wheel_scrolls_back_through_the_output(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_terminal::init);
    let project = app.project(&[("a.txt", "a\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    assert!(window.run_command(cx, "terminal.toggle"));
    let panel = cx.update(|cx| oncode_ui_terminal::panel(workbench.read(cx)).unwrap());
    window.wait_until(cx, "shell berjalan", |_, cx| panel.read(cx).active_session().is_some_and(|s| s.read(cx).is_running())).await;
    let session = cx.update(|cx| panel.read(cx).active_session().unwrap().clone());

    // Keluaran lebih panjang dari layar, sehingga sebagian masuk riwayat.
    let command = if cfg!(windows) { "1..150 | ForEach-Object { \"baris $_\" }" } else { "seq 1 150 | sed 's/^/baris /'" };
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input(command, cx);
        window.press("enter", cx);
    });
    window.wait_until(cx, "keluaran selesai", |_, cx| session.read(cx).text().contains("baris 150")).await;
    window.wait_until(cx, "ada riwayat", |_, cx| session.read(cx).history_size() > 0).await;
    assert_eq!(cx.update(|cx| session.read(cx).display_offset()), 0);

    // Roda mouse ke atas: tampilan bergeser ke riwayat.
    window.update(cx, |window, cx| window.scroll("terminal-session", gpui_kit::ScrollDelta::Lines(gpui_kit::point(0., 5.)), cx));
    cx.run_until_parked();
    let offset = cx.update(|cx| session.read(cx).display_offset());
    assert!(offset > 0, "roda mouse ke atas menggulir riwayat (offset {offset})");

    // Roda ke bawah kembali ke keluaran terbaru.
    window.update(cx, |window, cx| window.scroll("terminal-session", gpui_kit::ScrollDelta::Lines(gpui_kit::point(0., -50.)), cx));
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| session.read(cx).display_offset()), 0);

    // Keyboard: Shift+PageUp naik satu halaman, Ctrl+Shift+End kembali ke bawah.
    let press = |cx: &mut TestAppContext, key: &str| {
        window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        });
        cx.run_until_parked();
    };
    press(cx, "shift-pageup");
    assert!(cx.update(|cx| session.read(cx).display_offset()) > 0, "Shift+PageUp menggulir riwayat");
    press(cx, "ctrl-shift-end");
    assert_eq!(cx.update(|cx| session.read(cx).display_offset()), 0);
    press(cx, "ctrl-shift-up");
    assert_eq!(cx.update(|cx| session.read(cx).display_offset()), 1, "Ctrl+Shift+Up naik satu baris");

    // Scrollbar: klik di ujung atas lintasan melompat ke riwayat tertua.
    let history = cx.update(|cx| session.read(cx).history_size());
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click_with_options("terminal-scrollbar", gpui_kit::test::ClickOptions::new().with_offset(gpui_kit::point(gpui_kit::px(6.), gpui_kit::px(0.))), cx);
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| session.read(cx).display_offset()), history, "klik di atas scrollbar = riwayat tertua");
}

// ----- Split, area editor, dan seret-lepas -----

use gpui_kit::{point, Entity};
use oncode_ui::testing::TestWindow;
use oncode_ui::Workbench;
use oncode_ui_terminal::{session_tab, tab_session, TerminalPanel, TerminalSession};

/// Membuka proyek dan terminal pertama di panel, lalu menunggu shell-nya berjalan.
async fn open_with_terminal(app: &TestApp, cx: &mut TestAppContext) -> (TestWindow, Entity<Workbench>, Entity<TerminalPanel>) {
    let project = app.project(&[("a.txt", "a\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    assert!(window.run_command(cx, "terminal.toggle"));
    let panel = cx.update(|cx| oncode_ui_terminal::panel(workbench.read(cx)).unwrap());
    window.wait_until(cx, "shell berjalan", |_, cx| panel.read(cx).active_session().is_some_and(|s| s.read(cx).is_running())).await;
    (window, workbench, panel)
}

/// Id terminal di tab-tab area editor, menurut urutan grup.
fn editor_terminals(workbench: &Entity<Workbench>, cx: &mut TestAppContext) -> Vec<Vec<u64>> {
    cx.update(|cx| workbench.read(cx).model().groups.iter().map(|g| g.tabs.iter().filter_map(tab_session).collect()).filter(|ids: &Vec<u64>| !ids.is_empty()).collect())
}

fn session_ids(sessions: &[Entity<TerminalSession>], cx: &mut TestAppContext) -> Vec<u64> {
    cx.update(|cx| sessions.iter().map(|s| s.read(cx).session_id()).collect())
}

/// Menyeret dari tengah elemen `from` ke titik relatif (`fx`, `fy` dalam 0..1) di elemen `to`.
fn drag_to_point(window: &TestWindow, cx: &mut TestAppContext, from: &str, to: &str, fx: f32, fy: f32) {
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        let from = window.find(from.to_string()).bounds().center();
        let to = window.find(to.to_string()).bounds();
        window.drag(from, point(to.origin.x + to.size.width * fx, to.origin.y + to.size.height * fy), cx);
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
async fn split_terminal_shows_two_shells_side_by_side(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_terminal::init);
    let (window, workbench, panel) = open_with_terminal(&app, cx).await;
    let first = cx.update(|cx| panel.read(cx).active_session().unwrap().read(cx).session_id());

    assert!(window.run_command(cx, "terminal.split"));
    window.wait_until(cx, "dua terminal berjalan", |_, cx| panel.read(cx).visible_sessions().iter().filter(|s| s.read(cx).is_running()).count() == 2).await;
    let second = cx.update(|cx| panel.read(cx).active_session().unwrap().read(cx).session_id());
    assert_ne!(first, second);
    assert_eq!(cx.update(|cx| panel.read(cx).layout().groups.clone()), [vec![first, second]], "satu kelompok, berdampingan");

    // Kedua terminal tampil bersebelahan (yang baru di kanan), dan daftar terminal muncul.
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        let (left, right) = (window.find(format!("terminal-pane-{first}")).bounds(), window.find(format!("terminal-pane-{second}")).bounds());
        assert!(right.origin.x >= left.origin.x + left.size.width - gpui_kit::px(2.), "berdampingan: {left:?} {right:?}");
        assert_eq!(left.origin.y, right.origin.y);
        assert!(window.find(format!("terminal-tab-{first}")).visible());
    });

    // Pintasan Split Terminal saat terminal fokus (Cmd+\ di macOS, yang di luar terminal berarti
    // Split Editor; Ctrl+Shift+5 di tempat lain).
    let key = if cfg!(target_os = "macos") { "cmd-\\" } else { "ctrl-shift-5" };
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press(key, cx);
    });
    window.wait_until(cx, "terminal ketiga dari pintasan", |_, cx| panel.read(cx).visible_sessions().len() == 3).await;
    assert_eq!(editor_terminals(&workbench, cx).len(), 0, "tidak ada tab yang dibelah");
    let third = cx.update(|cx| panel.read(cx).active_session().unwrap().read(cx).session_id());
    assert_eq!(cx.update(|cx| panel.read(cx).layout().groups.clone()), [vec![first, second, third]]);

    // Unsplit memisahkan terminal aktif dari kelompoknya.
    assert!(window.run_command(cx, "terminal.unsplit"));
    assert_eq!(cx.update(|cx| panel.read(cx).layout().groups.clone()), [vec![first, second], vec![third]]);
}

#[gpui_kit::test]
async fn terminal_moves_to_the_editor_area_and_back_without_restarting(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_terminal::init);
    let (window, workbench, panel) = open_with_terminal(&app, cx).await;
    let session = cx.update(|cx| panel.read(cx).active_session().unwrap().clone());
    let id = cx.update(|cx| session.read(cx).session_id());
    let root = cx.update(|cx| workbench.read(cx).env().root.clone().unwrap());

    assert!(window.run_command(cx, "terminal.moveToEditor"));
    window.wait_until(cx, "terminal jadi tab", |_, cx| panel.read(cx).sessions().is_empty() && panel.read(cx).is_in_editor(id)).await;
    assert_eq!(editor_terminals(&workbench, cx), [[id]]);
    // Proses shell yang sama tetap berjalan dan menerima ketikan di tab editor.
    assert!(cx.update(|cx| session.read(cx).is_running()));
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        assert!(session.read(cx).focus_handle_is_focused(window), "tab terminal fokus");
        window.input("echo moved > moved.txt", cx);
        window.press("enter", cx);
    });
    let out = root.join("moved.txt");
    window.wait_until(cx, "shell di tab editor menulis berkas", |_, _| std::fs::read(&out).is_ok_and(|bytes| shell_text(&bytes).trim_end() == "moved")).await;

    assert!(window.run_command(cx, "terminal.moveToPanel"));
    window.wait_until(cx, "kembali ke panel", |_, cx| panel.read(cx).sessions().len() == 1 && !panel.read(cx).is_in_editor(id)).await;
    assert!(editor_terminals(&workbench, cx).is_empty(), "tab terminal ditutup");
    assert!(cx.update(|cx| panel.read(cx).sessions()[0] == session && session.read(cx).is_running()), "terminal yang sama, masih berjalan");
}

#[gpui_kit::test]
async fn dragging_terminals_between_the_panel_list_and_the_editor_area(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_terminal::init);
    let (window, workbench, panel) = open_with_terminal(&app, cx).await;
    let first = cx.update(|cx| panel.read(cx).active_session().unwrap().read(cx).session_id());
    assert!(window.run_command(cx, "terminal.new"));
    window.wait_until(cx, "terminal kedua", |_, cx| panel.read(cx).sessions().len() == 2).await;
    let second = cx.update(|cx| panel.read(cx).active_session().unwrap().read(cx).session_id());

    // Dari daftar panel ke area editor yang kosong: menjadi tab, dicabut dari panel.
    window.update(cx, |window, cx| window.drag_to(format!("terminal-tab-{first}"), "editor-empty", cx));
    cx.run_until_parked();
    window.wait_until(cx, "terminal jadi tab", |_, cx| panel.read(cx).is_in_editor(first)).await;
    assert_eq!(editor_terminals(&workbench, cx), [[first]]);
    assert_eq!(session_ids(&cx.update(|cx| panel.read(cx).sessions()), cx), [second]);

    // Dari tab editor ke tepi kiri terminal panel: berdampingan di kiri terminal itu.
    let tab = format!("tab-1-{}", session_tab(first, "").id);
    drag_to_point(&window, cx, &tab, &format!("terminal-pane-{second}"), 0.05, 0.5);
    window.wait_until(cx, "kembali ke panel", |_, cx| !panel.read(cx).is_in_editor(first)).await;
    assert!(editor_terminals(&workbench, cx).is_empty());
    assert_eq!(cx.update(|cx| panel.read(cx).layout().groups.clone()), [vec![first, second]]);
    assert!(cx.update(|cx| panel.read(cx).sessions().iter().all(|s| s.read(cx).is_running())), "proses tetap berjalan");

    // Daftar: menjatuhkan terminal di ruang kosong di bawah daftar mengeluarkannya dari split.
    window.update(cx, |window, cx| window.drag_to(format!("terminal-tab-{first}"), "terminal-list-end", cx));
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| panel.read(cx).layout().groups.clone()), [vec![second], vec![first]]);
}

#[gpui_kit::test]
async fn closing_an_editor_terminal_kills_it_and_split_editor_opens_a_new_one(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_terminal::init);
    let (window, workbench, panel) = open_with_terminal(&app, cx).await;

    assert!(window.run_command(cx, "terminal.newInEditor"));
    window.wait_until(cx, "terminal di area editor", |_, cx| panel.read(cx).editor_sessions().len() == 1).await;
    let editor = cx.update(|cx| panel.read(cx).editor_sessions()[0].clone());
    window.wait_until(cx, "shell di tab berjalan", |_, cx| editor.read(cx).is_running()).await;
    // Judul tab mengikuti judul terminal.
    let title = cx.update(|cx| editor.read(cx).title().to_string());
    window.wait_until(cx, "judul tab", |_, cx| workbench.read(cx).model().all_tabs().any(|t| t.title == title)).await;

    // "Split Editor" pada tab terminal membuka terminal baru di grup sebelah, bukan tampilan kedua.
    assert!(window.run_command(cx, "workbench.split"));
    window.wait_until(cx, "terminal kedua di area editor", |_, cx| panel.read(cx).editor_sessions().len() == 2).await;
    let tabs = editor_terminals(&workbench, cx);
    assert_eq!(tabs.len(), 2, "dua grup: {tabs:?}");
    assert_ne!(tabs[0], tabs[1]);

    // Menutup tab mematikan terminalnya; terminal panel tidak tersentuh.
    let id = tabs[1][0];
    assert!(window.run_command(cx, "workbench.closeTab"));
    window.wait_until(cx, "terminal tab mati", |_, cx| panel.read(cx).session(id).is_none()).await;
    assert_eq!(panel_count(&panel, cx), 1);
    assert_eq!(cx.update(|cx| panel.read(cx).editor_sessions().len()), 1);
}

fn panel_count(panel: &Entity<TerminalPanel>, cx: &mut TestAppContext) -> usize {
    cx.update(|cx| panel.read(cx).sessions().len())
}

// ----- Font dan tampilan -----

#[gpui_kit::test]
async fn font_settings_apply_to_open_terminals_and_line_drawing_renders(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_terminal::init);
    let (window, workbench, panel) = open_with_terminal(&app, cx).await;
    let session = cx.update(|cx| panel.read(cx).active_session().unwrap().clone());
    window.update(cx, |window, cx| window.render_frame(cx));
    let font_size = cx.update(|cx| workbench.read(cx).settings().editor.font_size) as f32;
    let (_, height) = cx.update(|cx| session.read(cx).cell_size()).expect("sudah digambar");
    assert!((f32::from(height) - font_size * 1.2).abs() <= 0.5, "tinggi baris bawaan 120%: {height:?}");
    cx.run_until_parked();
    let rows_before = cx.update(|cx| session.read(cx).grid_size().1);

    // Pengaturan baru berlaku untuk terminal yang sudah terbuka. Font yang tidak terpasang tidak
    // dipakai (sebelumnya terminal pindah ke font itu setelah pengaturan berubah).
    let mut settings = app.core.user_settings();
    settings.terminal.line_height = Some(150);
    settings.terminal.font_family = Some("Font Yang Tidak Terpasang".into());
    let core = app.core.clone();
    app.runtime.block_on(async move { oncode_core::api::app::settings_set(&core, settings).await }).unwrap();
    window.wait_until(cx, "pengaturan baru terbaca", |_, cx| workbench.read(cx).settings().terminal.line_height == Some(150)).await;
    window.update(cx, |window, cx| window.render_frame(cx));
    let (_, height) = cx.update(|cx| session.read(cx).cell_size()).unwrap();
    assert!((f32::from(height) - font_size * 1.5).abs() <= 0.5, "tinggi baris 150%: {height:?}");
    // Jumlah baris grid ikut menyesuaikan walau ukuran panel tidak berubah.
    window.wait_until(cx, "grid disesuaikan", |_, cx| session.read(cx).grid_size().1 < rows_before).await;
    assert_ne!(cx.update(|cx| session.read(cx).font_family().to_string()), "Font Yang Tidak Terpasang");

    // Garis kotak, blok, dan pemisah powerline digambar sebagai bentuk tanpa galat.
    window.update(cx, |window, cx| {
        window.input("echo ╭─┬─╮ ║ ▌█▒ \u{E0B0}\u{E0B4}", cx);
        window.render_frame(cx);
    });
    if cfg!(unix) {
        window.wait_until(cx, "karakter garis tampil", |_, cx| session.read(cx).text().contains("╭─┬─╮")).await;
    }
    window.update(cx, |window, cx| window.render_frame(cx));
}

// ----- Ukuran PTY -----

/// Terminal dibuat sebelum tata letak pertama, jadi PTY lahir 80x24. Ukuran sebenarnya baru diketahui
/// setelah layout, dan spawn berjalan async sehingga id PTY bisa belum ada saat itu. Shell tetap harus
/// melihat ukuran yang sama dengan grid; kalau tidak, program layar penuh (mis. Claude CLI) menggambar
/// untuk 80x24 di area yang jauh lebih lebar sampai ada resize manual.
#[gpui_kit::test]
async fn shell_sees_the_same_size_as_the_grid_after_the_first_layout(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_terminal::init);
    let project = app.project(&[("a.txt", "a\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);

    assert!(window.run_command(cx, "terminal.toggle"));
    let panel = cx.update(|cx| oncode_ui_terminal::panel(workbench.read(cx)).unwrap());
    window.wait_until(cx, "shell berjalan", |_, cx| panel.read(cx).active_session().is_some_and(|s| s.read(cx).is_running())).await;
    let session = cx.update(|cx| panel.read(cx).active_session().unwrap().clone());
    window.update(cx, |window, cx| window.render_frame(cx));
    cx.run_until_parked();

    let (cols, rows) = cx.update(|cx| session.read(cx).grid_size());
    assert_ne!((cols, rows), (80, 24), "uji ini hanya berarti bila panel tidak kebetulan berukuran 80x24");

    // Windows: `default_shell` selalu memilih pwsh atau powershell lebih dulu daripada cmd.
    let command = if cfg!(windows) { r#"Write-Output "$([Console]::WindowWidth) $([Console]::WindowHeight)" > size.txt"# } else { "stty size | awk '{print $2, $1}' > size.txt" };
    window.update(cx, |window, cx| {
        window.input(command, cx);
        window.press("enter", cx);
    });
    let out = project.join("size.txt");
    window.wait_until(cx, "shell menulis ukuran PTY", |_, _| std::fs::read(&out).is_ok_and(|bytes| !shell_text(&bytes).trim().is_empty())).await;
    let seen = shell_text(&std::fs::read(&out).unwrap());
    assert_eq!(seen.trim(), format!("{cols} {rows}"), "ukuran PTY menurut shell berbeda dari grid emulator");
}
