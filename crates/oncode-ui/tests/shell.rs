//! View tests for the window framework: command palette, quick open, keyboard shortcuts, session.

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
    // Enter right after typing, before the backend search finishes (the list still holds
    // the results of the empty query): the pick waits for the results for "lib", instead of being ignored.
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input("lib", cx);
    });
    cx.run_until_parked();
    window.update(cx, |window, cx| window.press("enter", cx));
    let workbench_for_wait = workbench.clone();
    window.wait_until(cx, "lib.rs open", move |_, cx| workbench_for_wait.read(cx).model().all_tabs().any(|tab| tab.title == "lib.rs")).await;
    assert_eq!(tab_titles(&workbench, cx), ["lib.rs"]);
}

/// Quick open results are shown and can be told apart from the "searching" state (ids for tests).
#[gpui_kit::test]
async fn quick_open_lists_matching_files(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("src/main.rs", "fn main() {}\n"), ("src/lib.rs", "pub fn f() {}\n")]);
    let window = app.open(Some(&project), cx).await;
    assert!(window.run_command(cx, "workbench.quickOpen"));
    cx.run_until_parked();
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input("zzz-does-not-exist", cx);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let (items, empty) = window.update(cx, |window, cx| {
            window.render_frame(cx);
            (window.find_all(("quick-pick-item", 0usize)).len(), window.find_all("quick-pick-empty").len())
        });
        if empty > 0 {
            assert_eq!(items, 0, "no file matches");
            break;
        }
        assert!(std::time::Instant::now() < deadline, "the empty state is not shown");
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
    assert!(tab_titles(&workbench, cx).is_empty(), "the picked command must be run");
}

#[gpui_kit::test]
async fn keybindings_json_overrides_default_shortcuts(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let window = app.open(None, cx).await;
    let workbench = window.workbench(cx);
    let visible = |cx: &mut TestAppContext| cx.update(|cx| workbench.read(cx).is_sidebar_visible());
    assert!(visible(cx));

    // The default shortcut Mod+B.
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press("secondary-b", cx);
    });
    cx.run_until_parked();
    assert!(!visible(cx), "Mod+B hides the sidebar");

    // A user override: the default shortcut no longer applies, the new one does.
    std::fs::create_dir_all(app.core.keybindings_path().parent().unwrap()).unwrap();
    std::fs::write(app.core.keybindings_path(), "// the user's own\n{ \"workbench.toggleSidebar\": \"Mod+Alt+B\" }\n").unwrap();
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
    assert!(!visible(cx), "the old shortcut no longer applies");
    window.update(cx, |window, cx| window.press("secondary-alt-b", cx));
    cx.run_until_parked();
    assert!(visible(cx), "the new shortcut applies");
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
            // A preview tab is not saved.
            open_file(workbench, OpenFile::new(path("c.txt")).preview(), window, cx);
            open_file(workbench, OpenFile::new(path("a.txt")), window, cx);
        })
    });
    cx.run_until_parked();

    // Closing the folder saves the session; opening it again restores the tabs.
    window.update(cx, |window, cx| window.shell_close_folder(cx));
    window.wait_until(cx, "folder closed", |shell, cx| shell.read(cx).workspace().is_none()).await;
    let shell = window.shell.clone();
    window.update(cx, |window, cx| shell.update(cx, |shell, cx| shell.open_folder(project.clone(), window, cx)));
    window.wait_until(cx, "folder open again", |shell, cx| shell.read(cx).workspace().is_some()).await;
    let workbench = window.workbench(cx);
    window.wait_until(cx, "tabs restored", |_, cx| workbench.read(cx).model().all_tabs().count() == 2).await;
    assert_eq!(tab_titles(&workbench, cx), ["a.txt", "b.txt"]);
    window.wait_until(cx, "active tab restored", |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).file_name() == "a.txt")).await;
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

/// An Enter pressed while an async search is still running is not lost: the first item of the latest
/// query's results is picked as soon as they arrive.
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
    // Every search for the query "x" (possibly more than once) waits until the test sends its results.
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
        quick_pick(QuickPickOptions::with_search("Search", search), move |item, _, _| *picked.borrow_mut() = Some(item.map(|i| i.value)), window, cx);
    });
    cx.run_until_parked();
    assert!(dialog_open(&window, cx), "the quick pick is open");
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input("x", cx);
    });
    cx.run_until_parked();
    assert!(dialog_open(&window, cx), "still open after typing");
    window.update(cx, |window, cx| window.press("enter", cx));
    cx.run_until_parked();
    assert!(picked.borrow().is_none(), "no results yet: nothing has been picked");
    assert!(dialog_open(&window, cx), "the quick pick stays open waiting for results");

    send.send(Some(vec![PickItem::new("one", "1"), PickItem::new("two", "2")])).unwrap();
    let picked_for_wait = picked.clone();
    window.wait_until(cx, "first item picked", move |_, _| picked_for_wait.borrow().is_some()).await;
    assert_eq!(picked.borrow().clone(), Some(Some("1".to_string())));
}

/// The folder each window has open, in the order of `cx.windows()`.
fn window_folders(cx: &mut TestAppContext) -> Vec<Option<String>> {
    use gpui_kit::AppContext as _;
    let windows = cx.update(|cx| cx.windows());
    windows
        .into_iter()
        .map(|handle| {
            cx.update_window(handle, |_, window, cx| oncode_ui::Shell::for_window(window, cx).and_then(|shell| shell.read(cx).workspace().map(|w| w.root.clone())))
                .ok()
                .flatten()
        })
        .collect()
}

/// `oncode <folder>` while OnCode is already running: the window that already has that folder open is reused,
/// another folder gets a new window.
#[gpui_kit::test]
async fn command_line_folder_reuses_its_window_or_opens_a_new_one(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let first = app.project(&[("a.txt", "a\n")]);
    let second = app.project(&[("b.txt", "b\n")]);
    app.open(Some(&first), cx).await;

    // Another path for the same folder (`<folder>/.`, like `oncode .`) is still recognized.
    cx.update(|cx| oncode_ui::open_from_command_line(Some(first.join(".")), cx));
    cx.run_until_parked();
    assert_eq!(window_folders(cx).len(), 1);

    cx.update(|cx| oncode_ui::open_from_command_line(Some(second.clone()), cx));
    let wanted = second.to_string_lossy().into_owned();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !window_folders(cx).contains(&Some(wanted.clone())) {
        assert!(std::time::Instant::now() < deadline, "the second folder did not open: {:?}", window_folders(cx));
        cx.run_until_parked();
        cx.executor().timer(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(window_folders(cx).len(), 2);

    cx.update(|cx| oncode_ui::open_from_command_line(Some(second.clone()), cx));
    cx.run_until_parked();
    assert_eq!(window_folders(cx).len(), 2);

    // Without a folder: a new empty window.
    cx.update(|cx| oncode_ui::open_from_command_line(None, cx));
    cx.run_until_parked();
    assert_eq!(window_folders(cx).len(), 3);
}
