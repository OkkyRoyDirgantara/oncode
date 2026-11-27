//! Tests of the LSP views against a mock language server (`oncode_lsp::mock`) run as a real
//! process over stdio, for `.txt` files (language id `plaintext`).

use std::path::{Path, PathBuf};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext};
use oncode_core::Settings;
use oncode_lsp::ServerSpec;
use oncode_ui::editor::{active_file, open_file, OpenFile};
use oncode_ui::testing::{TestApp, TestWindow};
use oncode_ui::Workbench;
use oncode_ui_lsp::hub::ServerStatus;

/// The mock server program; built with `cargo build -p oncode-lsp --features mock --example mock_lsp_server`.
/// Looked up in `examples/` from the test binary's folder upwards, because the test binary's location
/// differs between Cargo versions (`target/<profile>/deps/` or `target/<profile>/build/<crate>/<hash>/out/`).
fn mock_server() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let relative = std::path::Path::new("examples").join(if cfg!(windows) { "mock_lsp_server.exe" } else { "mock_lsp_server" });
    let find = || exe.ancestors().skip(1).map(|dir| dir.join(&relative)).find(|path| path.is_file());
    if let Some(path) = find() {
        return path;
    }
    // Not built yet (e.g. just `cargo test -p oncode-ui-lsp`): build into the same target directory.
    let status = std::process::Command::new(env!("CARGO"))
        .args(["build", "-q", "-p", "oncode-lsp", "--features", "mock", "--example", "mock_lsp_server"])
        .status()
        .expect("cargo is available");
    assert!(status.success(), "mock server failed to build");
    find().unwrap_or_else(|| panic!("mock server not found above {}", exe.display()))
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
        .wait_until(cx, "server is running", |_, cx| {
            oncode_ui_lsp::hub(workbench.read(cx)).is_some_and(|hub| hub.read(cx).server("plaintext").is_some_and(|s| s.status == ServerStatus::Running))
        })
        .await;
    (window, workbench)
}

#[gpui_kit::test]
async fn diagnostics_reach_the_store_and_the_problems_panel(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("a.txt", "see WARN\nthen ERROR here\n")]);
    let (window, workbench) = open_with_server(&app, &project, "a.txt", cx).await;
    let diagnostics = cx.update(|cx| oncode_ui_lsp::diagnostics(workbench.read(cx)).unwrap());
    window.wait_until(cx, "diagnostics arrived", |_, cx| diagnostics.read(cx).counts().total() == 2).await;
    let counts = cx.update(|cx| diagnostics.read(cx).counts());
    assert_eq!((counts.errors, counts.warnings), (1, 1));
    // Diagnostics are also attached to the editor (underlines).
    let state = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap().2);
    window.wait_until(cx, "diagnostics in the editor", |_, cx| state.read(cx).diagnostics().is_some_and(|set| set.len() == 2)).await;
    // The range points at the right word, not the start of the document.
    let starts: Vec<usize> = cx.update(|cx| state.read(cx).diagnostics().unwrap().iter().map(|entry| entry.range.start).collect());
    assert_eq!(starts, [4, 14], "WARN on line 1, ERROR on line 2");

    // Editing the file updates the diagnostics (the document is synced to the server).
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input("INFO ", cx);
    });
    window.wait_until(cx, "diagnostics updated", |_, cx| diagnostics.read(cx).counts().total() == 3).await;

    // The status bar opens the Problems panel.
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
    let project = app.project(&[("a.txt", "use helper\n"), ("b.txt", "first line\ndef helper\n")]);
    let (window, workbench) = open_with_server(&app, &project, "a.txt", cx).await;

    // Cursor on "helper", then F12.
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
    window.wait_until(cx, "definition file is open", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).path() == target)).await;
}

#[gpui_kit::test]
async fn rename_changes_open_and_closed_files(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("a.txt", "use helper\n"), ("b.txt", "def helper\n")]);
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
        window.input("renamed", cx);
        window.press("enter", cx);
    });
    window.wait_until(cx, "closed file is renamed", |_, _| std::fs::read_to_string(project.join("b.txt")).is_ok_and(|t| t == "def renamed\n")).await;
    window.wait_until(cx, "editor is renamed", |_, cx| state.read(cx).value().as_ref() == "use renamed\n").await;
}

/// Waits for an element (a quick pick's input or item), rendering meanwhile.
async fn wait_for(window: &TestWindow, cx: &mut TestAppContext, id: impl Into<gpui_kit::ElementId> + Clone) {
    for _ in 0..300 {
        let found = window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.try_find(id.clone()).is_some()
        });
        if found {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.run_until_parked();
    }
    panic!("element never appeared");
}

fn key(window: &TestWindow, cx: &mut TestAppContext, keystroke: &str) {
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press(keystroke, cx);
    });
    cx.run_until_parked();
}

/// Go to Symbol in Editor and in Workspace use the server's `documentSymbol` and `workspace/symbol`
/// (the mock server knows only `def name`; the text patterns would not find these in a `.txt` file).
#[gpui_kit::test]
async fn go_to_symbol_uses_the_language_server(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("a.txt", "intro\ndef alpha\ndef beta\n"), ("b.txt", "x\ny\ndef gamma\n")]);
    let (window, workbench) = open_with_server(&app, &project, "a.txt", cx).await;
    let view = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap().0);

    key(&window, cx, "secondary-shift-o");
    wait_for(&window, cx, ("quick-pick-item", 1usize)).await;
    key(&window, cx, "down");
    key(&window, cx, "enter");
    window.wait_until(cx, "cursor on beta", |_, cx| view.read(cx).cursor_info(cx).0 == 3).await;

    assert!(window.run_command(cx, "workbench.gotoSymbol"));
    wait_for(&window, cx, "quick-pick-input").await;
    window.update(cx, |window, cx| window.input("gam", cx));
    wait_for(&window, cx, ("quick-pick-item", 0usize)).await;
    key(&window, cx, "enter");
    let target = project.join("b.txt").to_string_lossy().into_owned();
    window
        .wait_until(cx, "b.txt open at gamma", |_, cx| {
            active_file(workbench.read(cx), cx).is_some_and(|(view, buffer, _)| buffer.read(cx).path() == target && buffer.read(cx).is_ready() && view.read(cx).cursor_info(cx).0 == 3)
        })
        .await;
}

/// Typing `(` shows the signature with the first parameter; `,` moves to the next one; leaving the call or
/// Escape closes it; Ctrl+Shift+Space shows it again.
#[gpui_kit::test]
async fn signature_help_follows_the_call_being_typed(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("a.txt", "\n")]);
    let (window, workbench) = open_with_server(&app, &project, "a.txt", cx).await;
    let state = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap().2);
    let buffers = cx.update(|cx| oncode_ui::editor::buffers(workbench.read(cx)).unwrap());
    let id = state.entity_id();
    let active = move |cx: &gpui_kit::App| buffers.read(cx).signature(id).map(|popup| popup.label[popup.active.clone().unwrap_or(0..0)].to_string());
    let typing = |window: &TestWindow, cx: &mut TestAppContext, text: &str| {
        window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.input(text, cx);
        });
        cx.run_until_parked();
    };

    typing(&window, cx, "copy(");
    let shown = active.clone();
    window.wait_until(cx, "signature shown", move |_, cx| shown(cx).as_deref() == Some("a")).await;
    typing(&window, cx, "x,");
    let moved = active.clone();
    window.wait_until(cx, "second parameter", move |_, cx| moved(cx).as_deref() == Some("b")).await;
    typing(&window, cx, ")");
    let closed = active.clone();
    window.wait_until(cx, "closed after the call", move |_, cx| closed(cx).is_none()).await;

    // Back inside the call: Ctrl+Shift+Space shows it again, Escape closes it.
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press("left", cx);
    });
    cx.run_until_parked();
    key(&window, cx, "secondary-shift-space");
    let again = active.clone();
    window.wait_until(cx, "shown on request", move |_, cx| again(cx).as_deref() == Some("b")).await;
    key(&window, cx, "escape");
    assert_eq!(cx.update(|cx| active(cx)), None, "Escape closes it");
}
