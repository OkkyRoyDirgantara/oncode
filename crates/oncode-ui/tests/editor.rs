//! Editor view tests: opening folders and files, editing, saving, and reloading.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use oncode_ui::editor::{active_file, buffers, open_file, OpenFile};
use oncode_ui::testing::{TestApp, TestWindow};

#[gpui_kit::test]
async fn opens_edits_and_saves_a_file(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("src/main.rs", "fn main() {}\n"), ("README.md", "# Demo\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let path = project.join("src/main.rs").to_string_lossy().into_owned();

    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&path), window, cx)));
    window.wait_until(cx, "file loaded", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).is_ready())).await;

    let (_, buffer, state) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());
    assert_eq!(cx.update(|cx| state.read(cx).value().to_string()), "fn main() {}\n");
    assert!(!cx.update(|cx| buffer.read(cx).is_dirty()));

    // Typing through the real input path: the editor is already focused after the tab is opened.
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input("// note\n", cx);
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| state.read(cx).value().to_string()), "// note\nfn main() {}\n");
    assert!(cx.update(|cx| buffer.read(cx).is_dirty()), "the buffer must be marked unsaved");
    assert!(cx.update(|cx| workbench.read(cx).has_dirty_tabs(cx)));

    // Ctrl+S (file.save) through the command registry.
    assert!(window.run_command(cx, "file.save"));
    window.wait_until(cx, "saved", |_, cx| !buffer.read(cx).is_dirty()).await;
    assert_eq!(std::fs::read_to_string(project.join("src/main.rs")).unwrap(), "// note\nfn main() {}\n");
}

/// SCM/diff/graph build the path from the Git root (`C:/x/a.txt` on Windows), the explorer from the folder
/// (`C:\x\a.txt`). Both must open the same tab and buffer.
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
    assert_eq!(cx.update(|cx| workbench.read(cx).model().all_tabs().count()), 1, "one tab per file");
    let (_, buffer, _) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());
    assert_eq!(cx.update(|cx| buffer.read(cx).path().to_string()), native);
}

#[gpui_kit::test]
async fn same_file_in_two_groups_stays_in_sync(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("a.txt", "one\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let path = project.join("a.txt").to_string_lossy().into_owned();

    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&path), window, cx)));
    window.wait_until(cx, "file loaded", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).is_ready())).await;
    let (_, _, first) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());

    assert!(window.run_command(cx, "workbench.split"));
    cx.run_until_parked();
    let (_, buffer, second) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());
    assert_ne!(first, second, "each group has its own editor state");
    assert_eq!(cx.update(|cx| workbench.read(cx).model().groups.len()), 2);
    assert_eq!(cx.update(|cx| second.read(cx).value().to_string()), "one\n");

    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input("two ", cx);
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| second.read(cx).value().to_string()), "two one\n");
    assert_eq!(cx.update(|cx| first.read(cx).value().to_string()), "two one\n", "the edit is forwarded to the other group");
    assert!(cx.update(|cx| buffer.read(cx).is_dirty()));
}

#[gpui_kit::test]
async fn clean_buffer_reloads_when_the_file_changes_on_disk(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("a.txt", "old\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let file = project.join("a.txt");
    let path = file.to_string_lossy().into_owned();

    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&path), window, cx)));
    window.wait_until(cx, "file loaded", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).is_ready())).await;
    let (_, _, state) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());

    // The size changes, so the change is detected even if the mtime is coarse-grained.
    std::fs::write(&file, "new from outside\n").unwrap();
    window.wait_until(cx, "reloaded", |_, cx| state.read(cx).value().as_ref() == "new from outside\n").await;
    assert!(cx.update(|cx| buffers(workbench.read(cx)).unwrap().read(cx).dirty(cx).is_empty()));
}

#[gpui_kit::test]
async fn ctrl_or_alt_arrows_move_lines_and_undo_restores_them(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("list.txt", "one\ntwo\nthree\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let path = project.join("list.txt").to_string_lossy().into_owned();
    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&path), window, cx)));
    window.wait_until(cx, "file loaded", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).is_ready())).await;
    let (_, buffer, state) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());
    let text = |cx: &mut TestAppContext| cx.update(|cx| state.read(cx).value().to_string());

    let press = |cx: &mut TestAppContext, key: &str| {
        window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        });
        cx.run_until_parked();
    };
    // The cursor is on the first line after the file is opened.
    press(cx, "ctrl-down");
    assert_eq!(text(cx), "two\none\nthree\n");
    press(cx, "ctrl-down");
    assert_eq!(text(cx), "two\nthree\none\n");
    press(cx, "alt-up");
    assert_eq!(text(cx), "two\none\nthree\n");
    assert!(cx.update(|cx| buffer.read(cx).is_dirty()));

    press(cx, "secondary-z");
    assert_eq!(text(cx), "two\nthree\none\n", "undo reverts a single move");
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
    let (_, buffer, state) = cx.update(|cx| active_file(workbench.read(cx), cx).expect("the new file tab is active"));
    assert!(cx.update(|cx| buffer.read(cx).is_untitled() && buffer.read(cx).is_ready()));
    assert_eq!(cx.update(|cx| buffer.read(cx).file_name().to_string()), "Untitled-1");
    let title = cx.update(|cx| workbench.read(cx).model().active().map(|(_, tab)| tab.title.clone()));
    assert_eq!(title.as_deref(), Some("Untitled-1"));

    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input("# Notes", cx);
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| state.read(cx).value().to_string()), "# Notes");
    assert!(cx.update(|cx| buffer.read(cx).is_dirty()));

    // Ctrl+N again: the next number.
    assert!(window.run_command(cx, "file.newUntitled"));
    cx.run_until_parked();
    let second = cx.update(|cx| active_file(workbench.read(cx), cx).map(|(_, buffer, _)| buffer.read(cx).file_name().to_string()));
    assert_eq!(second.as_deref(), Some("Untitled-2"));
    // Back to the first file, then save: the Save As dialog, the file is written, the tab moves to its new path.
    assert!(window.run_command(cx, "workbench.previousTab"));
    cx.run_until_parked();
    let target = project.join("notes.md");
    let chosen = target.clone();
    assert!(window.run_command(cx, "file.save"));
    cx.run_until_parked();
    assert!(cx.did_prompt_for_new_path(), "Save As is prompted");
    cx.simulate_new_path_selection(move |_| Some(chosen));
    let target_text = target.to_string_lossy().into_owned();
    window
        .wait_until(cx, "tab moves to the saved file", |_, cx| {
            active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).path() == target_text && !buffer.read(cx).is_dirty())
        })
        .await;
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "# Notes");
    let title = cx.update(|cx| workbench.read(cx).model().active().map(|(_, tab)| tab.title.clone()));
    assert_eq!(title.as_deref(), Some("notes.md"));
    let (_, _, state) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());
    assert_eq!(cx.update(|cx| state.read(cx).value().to_string()), "# Notes", "the content moves along to its new tab");
}

/// Format Document (Shift+Alt+F) and Format on Save with a real formatter (rustfmt), if installed.
#[gpui_kit::test]
async fn shift_alt_f_and_format_on_save_use_the_languages_formatter(cx: &mut TestAppContext) {
    if rustfmt_on_path().is_none() {
        return;
    }
    let app = TestApp::new(cx);
    let project = app.project(&[("main.rs", "fn  main( ){let x=1;}\n"), ("lib.rs", "pub fn  a(){}\n")]);
    // The formatter only runs in a trusted folder.
    app.trust(&project);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let open = |name: &str, cx: &mut TestAppContext| {
        let path = project.join(name).to_string_lossy().into_owned();
        window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&path), window, cx)));
    };

    open("main.rs", cx);
    window.wait_until(cx, "main.rs loaded", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).is_ready())).await;
    let (_, buffer, state) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press("shift-alt-f", cx);
    });
    window.wait_until(cx, "formatted by rustfmt", |_, cx| state.read(cx).value().as_ref() == "fn main() {\n    let x = 1;\n}\n").await;
    assert!(cx.update(|cx| buffer.read(cx).is_dirty()), "the format result is an unsaved edit");

    // Format on Save: the file on disk is saved in a tidy state.
    let mut settings = app.core.user_settings();
    settings.editor.format_on_save = true;
    let core = app.core.clone();
    app.runtime.block_on(async move { oncode_core::api::app::settings_set(&core, settings).await }).unwrap();
    window.wait_until(cx, "new settings read", |_, cx| workbench.read(cx).settings().editor.format_on_save).await;
    open("lib.rs", cx);
    window.wait_until(cx, "lib.rs loaded", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).path().ends_with("lib.rs") && buffer.read(cx).is_ready())).await;
    cx.run_until_parked();
    assert!(window.run_command(cx, "file.save"));
    let lib = project.join("lib.rs");
    window.wait_until(cx, "saved tidily", move |_, _| std::fs::read_to_string(&lib).is_ok_and(|text| text == "pub fn a() {}\n")).await;
}

fn rustfmt_on_path() -> Option<std::path::PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).find_map(|dir| ["rustfmt", "rustfmt.exe"].iter().map(|name| dir.join(name)).find(|path| path.is_file()))
}

/// F12 (and Ctrl+click, through the same provider) without a language server: the definition is searched in the project.
#[gpui_kit::test]
async fn f12_without_a_language_server_jumps_to_the_definition_in_another_file(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("util.py", "import os\n\ndef normalize(text):\n    return text.strip()\n"), ("main.py", "from util import normalize\nvalue = normalize(' a ')\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let main = project.join("main.py").to_string_lossy().into_owned();
    // The cursor is on "normalize" on line 2.
    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&main).at(2, 10, None), window, cx)));
    window.wait_until(cx, "main.py loaded", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).is_ready())).await;
    cx.run_until_parked();

    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press("f12", cx);
    });
    window
        .wait_until(cx, "util.py open at the definition", |_, cx| {
            active_file(workbench.read(cx), cx).is_some_and(|(view, buffer, _)| buffer.read(cx).path().ends_with("util.py") && view.read(cx).cursor_info(cx).0 == 3)
        })
        .await;
    let (view, _, _) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());
    assert_eq!(cx.update(|cx| view.read(cx).cursor_info(cx)).1, 5, "the cursor is at the start of the function name (column 5)");
}

/// A definition in the same file (Ctrl+click/F12 on a function in that very file) must not
/// crash the app: the cursor moves to its definition in the same editor.
#[gpui_kit::test]
async fn going_to_a_definition_in_the_same_file_moves_the_cursor(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("main.py", "def double(x):\n    return x * 2\n\nvalue = double(3)\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let main = project.join("main.py").to_string_lossy().into_owned();
    // The cursor is on "double" on line 4.
    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&main).at(4, 9, None), window, cx)));
    window.wait_until(cx, "main.py loaded", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).is_ready())).await;
    cx.run_until_parked();

    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press("f12", cx);
    });
    window.wait_until(cx, "cursor at the definition", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(view, _, _)| view.read(cx).cursor_info(cx).0 == 1)).await;
    let (view, buffer, _) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());
    assert!(cx.update(|cx| buffer.read(cx).path().ends_with("main.py")));
    // The function name is selected (columns 5–10), the cursor at the end of the selection.
    assert_eq!(cx.update(|cx| view.read(cx).cursor_info(cx)), (1, 11, 6), "\"double\" on line 1 is selected");
    assert_eq!(cx.update(|cx| workbench.read(cx).model().all_tabs().count()), 1, "no new tab is opened");
}

fn has_element(window: &TestWindow, id: &'static str, cx: &mut TestAppContext) -> bool {
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        !window.find_all(id).is_empty()
    })
}

/// Waits for an element of a dialog; dialogs are not part of the shell's state that `wait_until` looks at.
fn wait_for_element(window: &TestWindow, id: &'static str, cx: &mut TestAppContext) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !has_element(window, id, cx) {
        assert!(std::time::Instant::now() < deadline, "waited too long for {id}");
        // The backend (tokio) threads answer in the meantime.
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.run_until_parked();
    }
}

fn click_element(window: &TestWindow, id: &'static str, cx: &mut TestAppContext) {
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    });
    cx.run_until_parked();
}

/// Saving must not silently replace what another program wrote. `.env` is gitignored, so the watcher never
/// reports it: the save itself has to notice.
#[gpui_kit::test]
async fn saving_a_file_changed_on_disk_asks_before_overwriting(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[(".gitignore", ".env\n"), (".env", "A=1\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let file = project.join(".env");
    let path = file.to_string_lossy().into_owned();

    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&path), window, cx)));
    window.wait_until(cx, "file loaded", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).is_ready())).await;
    let (_, buffer, state) = cx.update(|cx| active_file(workbench.read(cx), cx).unwrap());
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input("B=2\n", cx);
    });
    cx.run_until_parked();
    assert!(cx.update(|cx| buffer.read(cx).is_dirty()));

    // Another program rewrites the file; Ctrl+S asks, and Cancel writes nothing.
    std::fs::write(&file, "A=1\nFROM_SCRIPT=1\n").unwrap();
    assert!(window.run_command(cx, "file.save"));
    wait_for_element(&window, "choice-overwrite", cx);
    click_element(&window, "choice-cancel", cx);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "A=1\nFROM_SCRIPT=1\n");
    assert!(cx.update(|cx| buffer.read(cx).is_dirty() && buffer.read(cx).has_conflict()));
    assert!(has_element(&window, "file-conflict-overwrite", cx), "the banner offers the choice again");

    // Overwrite from the banner writes this version.
    click_element(&window, "file-conflict-overwrite", cx);
    window.wait_until(cx, "overwritten", |_, cx| !buffer.read(cx).is_dirty()).await;
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "B=2\nA=1\n");
    assert!(!cx.update(|cx| buffer.read(cx).has_conflict()));

    // Saving again right away is not mistaken for a change from outside.
    window.update(cx, |window, cx| window.input("C=3\n", cx));
    cx.run_until_parked();
    assert!(window.run_command(cx, "file.save"));
    window.wait_until(cx, "saved", |_, cx| !buffer.read(cx).is_dirty()).await;
    assert!(!has_element(&window, "choice-overwrite", cx));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "B=2\nC=3\nA=1\n");

    // Reload takes the other program's version and drops the unsaved edit.
    window.update(cx, |window, cx| window.input("D=4\n", cx));
    cx.run_until_parked();
    std::fs::write(&file, "FROM_SCRIPT=2\n").unwrap();
    assert!(window.run_command(cx, "file.save"));
    wait_for_element(&window, "choice-reload", cx);
    click_element(&window, "choice-reload", cx);
    window.wait_until(cx, "reloaded", |_, cx| state.read(cx).value().as_ref() == "FROM_SCRIPT=2\n").await;
    assert!(!cx.update(|cx| buffer.read(cx).is_dirty() || buffer.read(cx).has_conflict()));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "FROM_SCRIPT=2\n");
}
