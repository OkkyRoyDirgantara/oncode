//! Uji tampilan LSP terhadap language server tiruan (`oncode_lsp::mock`) yang dijalankan sebagai
//! proses sungguhan lewat stdio, untuk berkas `.txt` (id bahasa `plaintext`).

use std::path::{Path, PathBuf};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext};
use oncode_core::Settings;
use oncode_lsp::ServerSpec;
use oncode_ui::editor::{active_file, open_file, OpenFile};
use oncode_ui::testing::{TestApp, TestWindow};
use oncode_ui::Workbench;
use oncode_ui_lsp::hub::ServerStatus;

/// Program server tiruan; dibangun dengan `cargo build -p oncode-lsp --features mock --example mock_lsp_server`.
fn mock_server() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let debug = exe.parent().unwrap().parent().unwrap();
    let path = debug.join("examples").join(if cfg!(windows) { "mock_lsp_server.exe" } else { "mock_lsp_server" });
    if !path.is_file() {
        let status = std::process::Command::new(env!("CARGO"))
            .args(["build", "-q", "-p", "oncode-lsp", "--features", "mock", "--example", "mock_lsp_server"])
            .env("CARGO_TARGET_DIR", debug.parent().unwrap())
            .status()
            .expect("cargo tersedia");
        assert!(status.success(), "server tiruan gagal dibangun");
    }
    path
}

fn configure(app: &TestApp) {
    let mut settings = Settings::default();
    settings.lsp.enabled = true;
    settings.lsp.servers.insert("plaintext".into(), ServerSpec { command: mock_server().to_string_lossy().into_owned(), args: Vec::new(), env: Vec::new() });
    app.core.set_user_settings(settings).unwrap();
}

async fn open_with_server(app: &TestApp, project: &Path, file: &str, cx: &mut TestAppContext) -> (TestWindow, Entity<Workbench>) {
    configure(app);
    cx.update(oncode_ui_lsp::init);
    app.trust(project);
    let window = app.open(Some(project), cx).await;
    let workbench = window.workbench(cx);
    let path = project.join(file).to_string_lossy().into_owned();
    window.update(cx, |window, cx| workbench.update(cx, |w, cx| open_file(w, OpenFile::new(path), window, cx)));
    window
        .wait_until(cx, "server berjalan", |_, cx| {
            oncode_ui_lsp::hub(workbench.read(cx)).is_some_and(|hub| hub.read(cx).server("plaintext").is_some_and(|s| s.status == ServerStatus::Running))
        })
        .await;
    (window, workbench)
}

#[gpui_kit::test]
async fn diagnostics_reach_the_store_and_the_problems_panel(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("a.txt", "ini WARN\nlalu ERROR di sini\n")]);
    let (window, workbench) = open_with_server(&app, &project, "a.txt", cx).await;
    let diagnostics = cx.update(|cx| oncode_ui_lsp::diagnostics(workbench.read(cx)).unwrap());
    window.wait_until(cx, "diagnostics tiba", |_, cx| diagnostics.read(cx).counts().total() == 2).await;
    let counts = cx.update(|cx| diagnostics.read(cx).counts());
    assert_eq!((counts.errors, counts.warnings), (1, 1));
    // Diagnostics juga terpasang di editor (garis bawah).
    let state = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap().2);
    window.wait_until(cx, "diagnostics di editor", |_, cx| state.read(cx).diagnostics().is_some_and(|set| set.len() == 2)).await;
    // Rentangnya menunjuk kata yang benar, bukan awal dokumen.
    let starts: Vec<usize> = cx.update(|cx| state.read(cx).diagnostics().unwrap().iter().map(|entry| entry.range.start).collect());
    assert_eq!(starts, [4, 14], "WARN di baris 1, ERROR di baris 2");

    // Menyunting berkas memperbarui diagnostics (dokumen tersinkron ke server).
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input("INFO ", cx);
    });
    window.wait_until(cx, "diagnostics diperbarui", |_, cx| diagnostics.read(cx).counts().total() == 3).await;

    // Status bar membuka panel Problems.
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click("status-problems", cx);
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| workbench.read(cx).active_panel().map(|p| p.to_string())).as_deref(), Some("problems"));
}

#[gpui_kit::test]
async fn go_to_definition_opens_the_other_file(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("a.txt", "pakai bantu\n"), ("b.txt", "baris pertama\ndef bantu\n")]);
    let (window, workbench) = open_with_server(&app, &project, "a.txt", cx).await;

    // Kursor di atas "bantu" lalu F12.
    let state = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap().2);
    window.update(cx, |window, cx| {
        state.update(cx, |state, cx| state.set_cursor_position(lsp_types::Position::new(0, 8), window, cx));
        window.render_frame(cx);
    });
    cx.run_until_parked();
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press("f12", cx);
    });
    let target = project.join("b.txt").to_string_lossy().into_owned();
    window.wait_until(cx, "berkas definisi terbuka", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).path() == target)).await;
}

#[gpui_kit::test]
async fn rename_changes_open_and_closed_files(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("a.txt", "pakai bantu\n"), ("b.txt", "def bantu\n")]);
    let (window, workbench) = open_with_server(&app, &project, "a.txt", cx).await;
    let state = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap().2);
    window.update(cx, |window, cx| {
        state.update(cx, |state, cx| state.set_cursor_position(lsp_types::Position::new(0, 8), window, cx));
        window.render_frame(cx);
    });
    cx.run_until_parked();
    assert!(window.run_command(cx, "editor.rename"));
    cx.run_until_parked();
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press("secondary-a", cx);
        window.input("tolong", cx);
        window.press("enter", cx);
    });
    window.wait_until(cx, "berkas tertutup diganti", |_, _| std::fs::read_to_string(project.join("b.txt")).is_ok_and(|t| t == "def tolong\n")).await;
    window.wait_until(cx, "editor diganti", |_, cx| state.read(cx).value().as_ref() == "pakai tolong\n").await;
}
