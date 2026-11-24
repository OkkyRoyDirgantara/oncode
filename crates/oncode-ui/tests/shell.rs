//! Uji tampilan kerangka jendela: command palette, quick open, pintasan keyboard, sesi.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use oncode_ui::editor::{active_file, open_file, OpenFile};
use oncode_ui::testing::TestApp;

fn tab_titles(workbench: &gpui_kit::Entity<oncode_ui::Workbench>, cx: &mut TestAppContext) -> Vec<String> {
    cx.update(|cx| workbench.read(cx).model().all_tabs().map(|t| t.title.clone()).collect())
}

#[gpui_kit::test]
async fn quick_open_finds_and_opens_a_file(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("src/main.rs", "fn main() {}\n"), ("src/lib.rs", "pub fn f() {}\n"), ("README.md", "# Demo\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);

    assert!(window.run_command(cx, "workbench.quickOpen"));
    cx.run_until_parked();
    // Enter langsung setelah mengetik, sebelum pencarian di backend selesai (daftar masih berisi
    // hasil kueri kosong): pilihan menunggu hasil untuk "lib", bukan diabaikan.
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input("lib", cx);
    });
    cx.run_until_parked();
    window.update(cx, |window, cx| window.press("enter", cx));
    let workbench_for_wait = workbench.clone();
    window.wait_until(cx, "lib.rs terbuka", move |_, cx| workbench_for_wait.read(cx).model().all_tabs().any(|tab| tab.title == "lib.rs")).await;
    assert_eq!(tab_titles(&workbench, cx), ["lib.rs"]);
}

/// Hasil quick open tampil dan bisa dibedakan dari status "mencari" (id untuk uji).
#[gpui_kit::test]
async fn quick_open_lists_matching_files(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("src/main.rs", "fn main() {}\n"), ("src/lib.rs", "pub fn f() {}\n")]);
    let window = app.open(Some(&project), cx).await;
    assert!(window.run_command(cx, "workbench.quickOpen"));
    cx.run_until_parked();
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input("zzz-tidak-ada", cx);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let (items, empty) = window.update(cx, |window, cx| {
            window.render_frame(cx);
            (window.find_all(("quick-pick-item", 0usize)).len(), window.find_all("quick-pick-empty").len())
        });
        if empty > 0 {
            assert_eq!(items, 0, "tidak ada berkas yang cocok");
            break;
        }
        assert!(std::time::Instant::now() < deadline, "status kosong tidak tampil");
        window.wait_until(cx, "backend", |_, _| true).await;
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[gpui_kit::test]
async fn command_palette_runs_the_selected_command(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("a.txt", "a\n"), ("b.txt", "b\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    for name in ["a.txt", "b.txt"] {
        let path = project.join(name).to_string_lossy().into_owned();
        window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(path), window, cx)));
    }
    cx.run_until_parked();
    assert_eq!(tab_titles(&workbench, cx).len(), 2);

    assert!(window.run_command(cx, "workbench.commandPalette"));
    cx.run_until_parked();
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input("close all editors", cx);
    });
    cx.run_until_parked();
    window.update(cx, |window, cx| window.press("enter", cx));
    cx.run_until_parked();
    assert!(tab_titles(&workbench, cx).is_empty(), "command terpilih harus dijalankan");
}

#[gpui_kit::test]
async fn keybindings_json_overrides_default_shortcuts(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let window = app.open(None, cx).await;
    let workbench = window.workbench(cx);
    let visible = |cx: &mut TestAppContext| cx.update(|cx| workbench.read(cx).is_sidebar_visible());
    assert!(visible(cx));

    // Pintasan bawaan Mod+B.
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press("secondary-b", cx);
    });
    cx.run_until_parked();
    assert!(!visible(cx), "Mod+B menyembunyikan sidebar");

    // Penimpaan pengguna: pintasan bawaan tidak lagi berlaku, yang baru berlaku.
    std::fs::create_dir_all(app.core.keybindings_path().parent().unwrap()).unwrap();
    std::fs::write(app.core.keybindings_path(), "// milik pengguna\n{ \"workbench.toggleSidebar\": \"Mod+Alt+B\" }\n").unwrap();
    assert!(window.run_command(cx, "keybindings.reload"));
    for _ in 0..100 {
        let applied = window.update(cx, |window, cx| {
            oncode_ui::commands::get("workbench.toggleSidebar", window, cx).is_some_and(|c| oncode_ui::commands::keybindings_for(&c, cx) == ["Mod+Alt+B".to_string()])
        });
        if applied {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.run_until_parked();
    }
    window.update(cx, |window, cx| window.press("secondary-b", cx));
    cx.run_until_parked();
    assert!(!visible(cx), "pintasan lama tidak lagi berlaku");
    window.update(cx, |window, cx| window.press("secondary-alt-b", cx));
    cx.run_until_parked();
    assert!(visible(cx), "pintasan baru berlaku");
}

#[gpui_kit::test]
async fn session_restores_pinned_file_tabs(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("a.txt", "a\n"), ("b.txt", "b\n"), ("c.txt", "c\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let path = |name: &str| project.join(name).to_string_lossy().into_owned();

    window.update(cx, |window, cx| {
        workbench.update(cx, |workbench, cx| {
            open_file(workbench, OpenFile::new(path("a.txt")), window, cx);
            open_file(workbench, OpenFile::new(path("b.txt")), window, cx);
            // Tab pratinjau tidak ikut disimpan.
            open_file(workbench, OpenFile::new(path("c.txt")).preview(), window, cx);
            open_file(workbench, OpenFile::new(path("a.txt")), window, cx);
        })
    });
    cx.run_until_parked();

    // Menutup folder menyimpan sesi; membukanya lagi memulihkan tab.
    window.update(cx, |window, cx| window.shell_close_folder(cx));
    window.wait_until(cx, "folder tertutup", |shell, cx| shell.read(cx).workspace().is_none()).await;
    let shell = window.shell.clone();
    window.update(cx, |window, cx| shell.update(cx, |shell, cx| shell.open_folder(project.clone(), window, cx)));
    window.wait_until(cx, "folder terbuka lagi", |shell, cx| shell.read(cx).workspace().is_some()).await;
    let workbench = window.workbench(cx);
    window.wait_until(cx, "tab dipulihkan", |_, cx| workbench.read(cx).model().all_tabs().count() == 2).await;
    assert_eq!(tab_titles(&workbench, cx), ["a.txt", "b.txt"]);
    window.wait_until(cx, "tab aktif dipulihkan", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).file_name() == "a.txt")).await;
}

trait ShellWindowExt {
    fn shell_close_folder(&mut self, cx: &mut gpui_kit::App);
}

impl ShellWindowExt for gpui_kit::Window {
    fn shell_close_folder(&mut self, cx: &mut gpui_kit::App) {
        if let Some(shell) = oncode_ui::Shell::for_window(self, cx) {
            shell.update(cx, |shell, cx| shell.close_folder(self, cx));
        }
    }
}

/// Enter yang ditekan selagi pencarian async masih berjalan tidak hilang: item pertama dari hasil
/// kueri terakhir dipilih begitu hasilnya tiba.
#[gpui_kit::test]
async fn enter_during_a_pending_search_picks_the_first_result_when_it_arrives(cx: &mut TestAppContext) {
    use gpui_kit::AppContext as _;
    use oncode_ui::quick_pick::{quick_pick, PickItem, QuickPickOptions};
    use std::cell::RefCell;
    use std::rc::Rc;

    let app = TestApp::new(cx);
    let project = app.project(&[("a.txt", "a
")]);
    let window = app.open(Some(&project), cx).await;
    let dialog_open = |window: &oncode_ui::testing::TestWindow, cx: &mut TestAppContext| {
        window.update(cx, |window, cx| {
            use gpui_kit::component::WindowExt as _;
            window.has_active_dialog(cx)
        })
    };
    // Setiap pencarian untuk kueri "x" (bisa lebih dari sekali) menunggu sampai uji mengirim hasilnya.
    let (send, receive) = tokio::sync::watch::channel::<Option<Vec<PickItem>>>(None);
    let _keep_open = receive.clone();
    let picked: Rc<RefCell<Option<Option<String>>>> = Rc::default();
    window.update(cx, |window, cx| {
        let search = move |query: String, cx: &mut gpui_kit::App| -> gpui_kit::Task<Vec<PickItem>> {
            if query != "x" {
                return gpui_kit::Task::ready(Vec::new());
            }
            let mut receive = receive.clone();
            cx.background_spawn(async move {
                let items = receive.wait_for(Option::is_some).await.ok().and_then(|items| items.clone());
                items.unwrap_or_default()
            })
        };
        let picked = picked.clone();
        quick_pick(QuickPickOptions::with_search("Cari", search), move |item, _, _| *picked.borrow_mut() = Some(item.map(|i| i.value)), window, cx);
    });
    cx.run_until_parked();
    assert!(dialog_open(&window, cx), "quick pick terbuka");
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input("x", cx);
    });
    cx.run_until_parked();
    assert!(dialog_open(&window, cx), "masih terbuka setelah mengetik");
    window.update(cx, |window, cx| window.press("enter", cx));
    cx.run_until_parked();
    assert!(picked.borrow().is_none(), "belum ada hasil: belum memilih apa pun");
    assert!(dialog_open(&window, cx), "quick pick tetap terbuka menunggu hasil");

    send.send(Some(vec![PickItem::new("satu", "1"), PickItem::new("dua", "2")])).unwrap();
    let picked_for_wait = picked.clone();
    window.wait_until(cx, "item pertama dipilih", move |_, _| picked_for_wait.borrow().is_some()).await;
    assert_eq!(picked.borrow().clone(), Some(Some("1".to_string())));
}
