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
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input("lib", cx);
    });
    // Pencarian berkas berjalan di backend; tunggu sampai hasilnya tampil lalu pilih.
    window.wait_until(cx, "hasil quick open", |_, _| true).await;
    for _ in 0..50 {
        window.update(cx, |window, cx| window.press("enter", cx));
        cx.run_until_parked();
        if !tab_titles(&workbench, cx).is_empty() {
            break;
        }
        window.wait_until(cx, "backend", |_, _| true).await;
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(tab_titles(&workbench, cx), ["lib.rs"]);
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
