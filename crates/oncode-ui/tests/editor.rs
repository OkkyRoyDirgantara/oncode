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

/// SCM/diff/graph membangun path dari root Git (`C:/x/a.txt` di Windows), explorer dari folder
/// (`C:\x\a.txt`). Keduanya harus membuka tab dan buffer yang sama.
#[gpui_kit::test]
async fn the_same_file_opened_with_either_separator_shares_one_tab(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("src/a.txt", "a\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let native = project.join("src").join("a.txt").to_string_lossy().into_owned();
    let slashed = native.replace('\\', "/");
    for path in [native.clone(), slashed] {
        window.update(cx, |window, cx| workbench.update(cx, |w, cx| open_file(w, OpenFile::new(path), window, cx)));
        cx.run_until_parked();
    }
    assert_eq!(cx.update(|cx| workbench.read(cx).model().all_tabs().count()), 1, "satu tab untuk satu berkas");
    let (_, buffer, _) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());
    assert_eq!(cx.update(|cx| buffer.read(cx).path().to_string()), native);
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

#[gpui_kit::test]
async fn ctrl_or_alt_arrows_move_lines_and_undo_restores_them(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("daftar.txt", "satu\ndua\ntiga\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let path = project.join("daftar.txt").to_string_lossy().into_owned();
    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&path), window, cx)));
    window.wait_until(cx, "berkas dimuat", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).is_ready())).await;
    let (_, buffer, state) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());
    let text = |cx: &mut TestAppContext| cx.update(|cx| state.read(cx).value().to_string());

    let press = |cx: &mut TestAppContext, key: &str| {
        window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        });
        cx.run_until_parked();
    };
    // Kursor di baris pertama setelah berkas dibuka.
    press(cx, "ctrl-down");
    assert_eq!(text(cx), "dua\nsatu\ntiga\n");
    press(cx, "ctrl-down");
    assert_eq!(text(cx), "dua\ntiga\nsatu\n");
    press(cx, "alt-up");
    assert_eq!(text(cx), "dua\nsatu\ntiga\n");
    assert!(cx.update(|cx| buffer.read(cx).is_dirty()));

    press(cx, "secondary-z");
    assert_eq!(text(cx), "dua\ntiga\nsatu\n", "undo membatalkan satu pemindahan");
}

#[gpui_kit::test]
async fn ctrl_n_opens_an_untitled_file_that_save_as_moves_into_the_project(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("a.txt", "a\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);

    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press("secondary-n", cx);
    });
    cx.run_until_parked();
    let (_, buffer, state) = cx.update(|cx| active_file(workbench.read(cx), cx).expect("tab berkas baru aktif"));
    assert!(cx.update(|cx| buffer.read(cx).is_untitled() && buffer.read(cx).is_ready()));
    assert_eq!(cx.update(|cx| buffer.read(cx).file_name().to_string()), "Untitled-1");
    let title = cx.update(|cx| workbench.read(cx).model().active().map(|(_, tab)| tab.title.clone()));
    assert_eq!(title.as_deref(), Some("Untitled-1"));

    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input("# Catatan", cx);
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| state.read(cx).value().to_string()), "# Catatan");
    assert!(cx.update(|cx| buffer.read(cx).is_dirty()));

    // Ctrl+N lagi: nomor berikutnya.
    assert!(window.run_command(cx, "file.newUntitled"));
    cx.run_until_parked();
    let second = cx.update(|cx| active_file(workbench.read(cx), cx).map(|(_, buffer, _)| buffer.read(cx).file_name().to_string()));
    assert_eq!(second.as_deref(), Some("Untitled-2"));
    // Kembali ke berkas pertama lalu simpan: dialog Save As, berkas ditulis, tab pindah ke path barunya.
    assert!(window.run_command(cx, "workbench.previousTab"));
    cx.run_until_parked();
    let target = project.join("catatan.md");
    let chosen = target.clone();
    assert!(window.run_command(cx, "file.save"));
    cx.run_until_parked();
    assert!(cx.did_prompt_for_new_path(), "Save As ditanyakan");
    cx.simulate_new_path_selection(move |_| Some(chosen));
    let target_text = target.to_string_lossy().into_owned();
    window
        .wait_until(cx, "tab pindah ke berkas yang disimpan", |_, cx| {
            active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).path() == target_text && !buffer.read(cx).is_dirty())
        })
        .await;
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "# Catatan");
    let title = cx.update(|cx| workbench.read(cx).model().active().map(|(_, tab)| tab.title.clone()));
    assert_eq!(title.as_deref(), Some("catatan.md"));
    let (_, _, state) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());
    assert_eq!(cx.update(|cx| state.read(cx).value().to_string()), "# Catatan", "isi ikut pindah ke tab barunya");
}

/// Format Document (Shift+Alt+F) dan Format on Save dengan formatter sungguhan (rustfmt), bila terpasang.
#[gpui_kit::test]
async fn shift_alt_f_and_format_on_save_use_the_languages_formatter(cx: &mut TestAppContext) {
    if rustfmt_on_path().is_none() {
        return;
    }
    let app = TestApp::new(cx);
    let project = app.project(&[("main.rs", "fn  main( ){let x=1;}\n"), ("lib.rs", "pub fn  a(){}\n")]);
    // Formatter hanya berjalan di folder yang dipercaya.
    app.trust(&project);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let open = |name: &str, cx: &mut TestAppContext| {
        let path = project.join(name).to_string_lossy().into_owned();
        window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&path), window, cx)));
    };

    open("main.rs", cx);
    window.wait_until(cx, "main.rs dimuat", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).is_ready())).await;
    let (_, buffer, state) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press("shift-alt-f", cx);
    });
    window.wait_until(cx, "diformat rustfmt", |_, cx| state.read(cx).value().as_ref() == "fn main() {\n    let x = 1;\n}\n").await;
    assert!(cx.update(|cx| buffer.read(cx).is_dirty()), "hasil format adalah suntingan yang belum disimpan");

    // Format on Save: berkas di disk tersimpan dalam keadaan rapi.
    let mut settings = app.core.user_settings();
    settings.editor.format_on_save = true;
    let core = app.core.clone();
    app.runtime.block_on(async move { oncode_core::api::app::settings_set(&core, settings).await }).unwrap();
    window.wait_until(cx, "pengaturan baru terbaca", |_, cx| workbench.read(cx).settings().editor.format_on_save).await;
    open("lib.rs", cx);
    window.wait_until(cx, "lib.rs dimuat", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).path().ends_with("lib.rs") && buffer.read(cx).is_ready())).await;
    cx.run_until_parked();
    assert!(window.run_command(cx, "file.save"));
    let lib = project.join("lib.rs");
    window.wait_until(cx, "tersimpan rapi", move |_, _| std::fs::read_to_string(&lib).is_ok_and(|text| text == "pub fn a() {}\n")).await;
}

fn rustfmt_on_path() -> Option<std::path::PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).find_map(|dir| ["rustfmt", "rustfmt.exe"].iter().map(|name| dir.join(name)).find(|path| path.is_file()))
}

/// F12 (dan Ctrl+klik, lewat penyedia yang sama) tanpa language server: definisi dicari di proyek.
#[gpui_kit::test]
async fn f12_without_a_language_server_jumps_to_the_definition_in_another_file(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("util.py", "import os\n\ndef bersihkan(teks):\n    return teks.strip()\n"), ("main.py", "from util import bersihkan\nhasil = bersihkan(' a ')\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let main = project.join("main.py").to_string_lossy().into_owned();
    // Kursor di "bersihkan" pada baris 2.
    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&main).at(2, 10, None), window, cx)));
    window.wait_until(cx, "main.py dimuat", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).is_ready())).await;
    cx.run_until_parked();

    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press("f12", cx);
    });
    window
        .wait_until(cx, "util.py terbuka di definisi", |_, cx| {
            active_file(workbench.read(cx), cx).is_some_and(|(view, buffer, _)| buffer.read(cx).path().ends_with("util.py") && view.read(cx).cursor_info(cx).0 == 3)
        })
        .await;
    let (view, _, _) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());
    assert_eq!(cx.update(|cx| view.read(cx).cursor_info(cx)).1, 5, "kursor di awal nama fungsi (kolom 5)");
}

/// Definisi di berkas yang sama (Ctrl+klik/F12 pada fungsi di berkas itu sendiri) tidak boleh
/// membuat aplikasi crash: kursor pindah ke definisinya di editor yang sama.
#[gpui_kit::test]
async fn going_to_a_definition_in_the_same_file_moves_the_cursor(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("main.py", "def hitung(x):\n    return x * 2\n\nhasil = hitung(3)\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let main = project.join("main.py").to_string_lossy().into_owned();
    // Kursor di "hitung" pada baris 4.
    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&main).at(4, 9, None), window, cx)));
    window.wait_until(cx, "main.py dimuat", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).is_ready())).await;
    cx.run_until_parked();

    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press("f12", cx);
    });
    window.wait_until(cx, "kursor di definisi", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(view, _, _)| view.read(cx).cursor_info(cx).0 == 1)).await;
    let (view, buffer, _) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());
    assert!(cx.update(|cx| buffer.read(cx).path().ends_with("main.py")));
    // Nama fungsinya dipilih (kolom 5–10), kursor di akhir pilihan.
    assert_eq!(cx.update(|cx| view.read(cx).cursor_info(cx)), (1, 11, 6), "\"hitung\" di baris 1 terpilih");
    assert_eq!(cx.update(|cx| workbench.read(cx).model().all_tabs().count()), 1, "tidak membuka tab baru");
}
