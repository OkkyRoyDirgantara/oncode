//! Uji menu bar macOS (`app_menu`): isinya mengikuti command jendela aktif, item menjalankan
//! command, dan pintasan yang baru ditolak keyboard tidak dijalankan lagi lewat menu. Isi menu dan
//! action-nya tidak bergantung pada platform, jadi diuji di semua OS.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use oncode_ui::app_menu::{self, DisabledMenuCommand, MenuCommand, MenuEntry, MenuModel};
use oncode_ui::commands::Command;
use oncode_ui::editor::{active_file, open_file, OpenFile};
use oncode_ui::keys::canonical_keybinding;
use oncode_ui::testing::{TestApp, TestWindow};

fn model(window: &TestWindow, cx: &mut TestAppContext) -> MenuModel {
    window.update(cx, |window, cx| app_menu::model(window, cx))
}

fn entry<'a>(model: &'a MenuModel, menu: &str, id: &str) -> Option<&'a MenuEntry> {
    model.menus.iter().find(|(title, _)| title == menu)?.1.iter().flatten().find(|item| item.id == id)
}

fn menu_command(id: &str) -> MenuCommand {
    MenuCommand { id: id.to_string().into(), key: None }
}

#[gpui_kit::test]
async fn menu_follows_the_commands_of_the_active_window(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("a.txt", "a\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);

    let before = model(&window, cx);
    assert_eq!(before.menus.first().map(|(title, _)| title.as_ref()), Some("File"));
    let save = entry(&before, "File", "file.save").expect("File → Save");
    assert_eq!((save.title.as_ref(), save.enabled, save.key.clone()), ("Save", false, Some(canonical_keybinding("Mod+S"))), "belum ada berkas terbuka");

    // Item yang bisa dipakai punya penangan; yang nonaktif tidak, sehingga macOS menampilkannya abu-abu.
    cx.update(|cx| {
        assert!(cx.is_action_available(&menu_command("file.newUntitled")));
        assert!(!cx.is_action_available(&DisabledMenuCommand { id: "file.save".into(), key: None }));
    });

    // File → New Text File lewat menu bar.
    cx.update(|cx| cx.dispatch_action(&menu_command("file.newUntitled")));
    cx.run_until_parked();
    let name = cx.update(|cx| active_file(workbench.read(cx), cx).map(|(_, buffer, _)| buffer.read(cx).file_name().to_string()));
    assert_eq!(name.as_deref(), Some("Untitled-1"));
    assert!(entry(&model(&window, cx), "File", "file.save").is_some_and(|save| save.enabled), "Save aktif setelah ada berkas");
}

#[gpui_kit::test]
async fn settings_moves_to_the_application_menu(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let window = app.open(None, cx).await;
    let workbench = window.workbench(cx);
    cx.update(|cx| workbench.update(cx, |workbench, _| workbench.register_command(Command::new("settings.open", "Open Settings", |_, _, _| {}).key("Mod+,"))));

    let model = model(&window, cx);
    assert_eq!(model.app.iter().map(|item| item.id.as_ref()).collect::<Vec<_>>(), ["settings.open"]);
    assert!(entry(&model, "File", "settings.open").is_none(), "tidak muncul dua kali");
    let file = &model.menus.iter().find(|(title, _)| title == "File").unwrap().1;
    assert!(file.windows(2).all(|pair| pair[0].is_some() || pair[1].is_some()), "tidak ada pemisah ganda");
}

#[gpui_kit::test]
async fn a_shortcut_declined_by_the_keyboard_is_not_run_again_by_the_menu(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("daftar.txt", "satu\ndua\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let path = project.join("daftar.txt").to_string_lossy().into_owned();
    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&path), window, cx)));
    window.wait_until(cx, "berkas dimuat", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).is_ready())).await;
    let (_, _, state) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());
    let text = |cx: &mut TestAppContext| cx.update(|cx| state.read(cx).value().to_string());

    // Alt+Down hanya memindahkan baris saat editor fokus. Di macOS, tombol yang ditolak lalu
    // ditawarkan AppKit ke menu bar dalam event yang sama.
    window.update(cx, |window, cx| window.blur(cx));
    let handle = window.handle;
    cx.update(|cx| {
        let _ = handle.update(cx, |_, window, cx| window.press("alt-down", cx));
        cx.dispatch_action(&menu_command("editor.moveLineDown"));
    });
    cx.run_until_parked();
    assert_eq!(text(cx), "satu\ndua\n");

    // Dipilih dari menu (bukan lewat tombol): berlaku untuk berkas aktif walau editor tidak fokus.
    cx.update(|cx| cx.dispatch_action(&menu_command("editor.moveLineDown")));
    cx.run_until_parked();
    assert_eq!(text(cx), "dua\nsatu\n");
}
