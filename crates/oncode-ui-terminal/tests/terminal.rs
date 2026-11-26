//! Terminal view tests: a real shell on a PTY, input through the window's key path.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use oncode_ui::testing::TestApp;

/// The contents of a file written by a shell redirection: PowerShell 5 writes UTF-16 with a BOM, PowerShell and cmd
/// use CRLF, Unix shells use UTF-8 with LF.
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
    window.wait_until(cx, "shell is running", |_, cx| panel.read(cx).active_session().is_some_and(|s| s.read(cx).is_running())).await;
    assert_eq!(cx.update(|cx| workbench.read(cx).active_panel().map(|p| p.to_string())).as_deref(), Some("terminal"));

    // Typing reaches the shell through the terminal's key handler (not an application shortcut).
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input("echo ok > out.txt", cx);
        window.press("enter", cx);
    });
    let out = project.join("out.txt");
    window.wait_until(cx, "file written by the shell", |_, _| std::fs::read(&out).is_ok_and(|bytes| shell_text(&bytes).trim_end() == "ok")).await;

    // The program's output shows up in the emulator. `echo` exists in sh, cmd, and PowerShell; the
    // typed command line (`echo done-marker`) is not exactly the same as its output line.
    window.update(cx, |window, cx| {
        window.input("echo done-marker", cx);
        window.press("enter", cx);
    });
    let session = cx.update(|cx| panel.read(cx).active_session().unwrap().clone());
    window.wait_until(cx, "output shows up", |_, cx| session.read(cx).text().lines().any(|line| line.trim() == "done-marker")).await;
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
    window.wait_until(cx, "shell is running", |_, cx| panel.read(cx).active_session().is_some_and(|s| s.read(cx).is_running())).await;

    // Ctrl+W in the terminal belongs to the shell (delete word), not "Close Editor".
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press("ctrl-w", cx);
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| workbench.read(cx).model().all_tabs().count()), 1, "the editor tab is not closed");

    // An allowed shortcut (Toggle Panel) still reaches the application.
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
    window.wait_until(cx, "shell is running", |_, cx| panel.read(cx).active_session().is_some_and(|s| s.read(cx).is_running())).await;
    let session = cx.update(|cx| panel.read(cx).active_session().unwrap().clone());

    // Tab (completion) and Shift+Tab belong to the shell: focus does not move to another control (e.g. the branch button).
    for key in ["tab", "shift-tab"] {
        let focused = window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
            session.read(cx).focus_handle_is_focused(window)
        });
        assert!(focused, "{key} moved focus out of the terminal");
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
    window.wait_until(cx, "first terminal is running", |_, cx| panel.read(cx).active_session().is_some_and(|s| s.read(cx).is_running())).await;

    // Ctrl+Shift+` (Cmd on macOS) as GPUI sends it on Windows: Ctrl + "~", Shift has already become the character.
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press("secondary-~", cx);
    });
    cx.run_until_parked();
    window.wait_until(cx, "second terminal from the shortcut", |_, cx| panel.read(cx).sessions().len() == 2).await;

    // The "+" button in the panel tab bar.
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click("terminal-new", cx);
    });
    cx.run_until_parked();
    window.wait_until(cx, "third terminal from the button", |_, cx| panel.read(cx).sessions().len() == 3).await;
    window.wait_until(cx, "all terminals are running", |_, cx| panel.read(cx).sessions().iter().all(|s| s.read(cx).is_running())).await;
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
    window.wait_until(cx, "shell is running", |_, cx| panel.read(cx).active_session().is_some_and(|s| s.read(cx).is_running())).await;
    let session = cx.update(|cx| panel.read(cx).active_session().unwrap().clone());

    // Output longer than the screen, so part of it goes into the history.
    let command = if cfg!(windows) { "1..150 | ForEach-Object { \"line $_\" }" } else { "seq 1 150 | sed 's/^/line /'" };
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.input(command, cx);
        window.press("enter", cx);
    });
    window.wait_until(cx, "output finished", |_, cx| session.read(cx).text().contains("line 150")).await;
    window.wait_until(cx, "there is history", |_, cx| session.read(cx).history_size() > 0).await;
    assert_eq!(cx.update(|cx| session.read(cx).display_offset()), 0);

    // Mouse wheel up: the view moves into the history.
    window.update(cx, |window, cx| window.scroll("terminal-session", gpui_kit::ScrollDelta::Lines(gpui_kit::point(0., 5.)), cx));
    cx.run_until_parked();
    let offset = cx.update(|cx| session.read(cx).display_offset());
    assert!(offset > 0, "mouse wheel up scrolls the history (offset {offset})");

    // Wheel down returns to the newest output.
    window.update(cx, |window, cx| window.scroll("terminal-session", gpui_kit::ScrollDelta::Lines(gpui_kit::point(0., -50.)), cx));
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| session.read(cx).display_offset()), 0);

    // Keyboard: Shift+PageUp goes up one page, Ctrl+Shift+End returns to the bottom.
    let press = |cx: &mut TestAppContext, key: &str| {
        window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        });
        cx.run_until_parked();
    };
    press(cx, "shift-pageup");
    assert!(cx.update(|cx| session.read(cx).display_offset()) > 0, "Shift+PageUp scrolls the history");
    press(cx, "ctrl-shift-end");
    assert_eq!(cx.update(|cx| session.read(cx).display_offset()), 0);
    press(cx, "ctrl-shift-up");
    assert_eq!(cx.update(|cx| session.read(cx).display_offset()), 1, "Ctrl+Shift+Up goes up one line");

    // Scrollbar: a click at the top end of the track jumps to the oldest history.
    let history = cx.update(|cx| session.read(cx).history_size());
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click_with_options("terminal-scrollbar", gpui_kit::test::ClickOptions::new().with_offset(gpui_kit::point(gpui_kit::px(6.), gpui_kit::px(0.))), cx);
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| session.read(cx).display_offset()), history, "a click at the top of the scrollbar = oldest history");
}

// ----- Split, editor area, and drag and drop -----

use gpui_kit::{point, Entity};
use oncode_ui::testing::TestWindow;
use oncode_ui::Workbench;
use oncode_ui_terminal::{session_tab, tab_session, TerminalPanel, TerminalSession};

/// Opens the project and the first terminal in the panel, then waits for its shell to run.
async fn open_with_terminal(app: &TestApp, cx: &mut TestAppContext) -> (TestWindow, Entity<Workbench>, Entity<TerminalPanel>) {
    let project = app.project(&[("a.txt", "a\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    assert!(window.run_command(cx, "terminal.toggle"));
    let panel = cx.update(|cx| oncode_ui_terminal::panel(workbench.read(cx)).unwrap());
    window.wait_until(cx, "shell is running", |_, cx| panel.read(cx).active_session().is_some_and(|s| s.read(cx).is_running())).await;
    (window, workbench, panel)
}

/// The terminal ids in the editor-area tabs, in group order.
fn editor_terminals(workbench: &Entity<Workbench>, cx: &mut TestAppContext) -> Vec<Vec<u64>> {
    cx.update(|cx| workbench.read(cx).model().groups.iter().map(|g| g.tabs.iter().filter_map(tab_session).collect()).filter(|ids: &Vec<u64>| !ids.is_empty()).collect())
}

fn session_ids(sessions: &[Entity<TerminalSession>], cx: &mut TestAppContext) -> Vec<u64> {
    cx.update(|cx| sessions.iter().map(|s| s.read(cx).session_id()).collect())
}

/// Drags from the center of element `from` to a relative point (`fx`, `fy` in 0..1) in element `to`.
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
    window.wait_until(cx, "two terminals are running", |_, cx| panel.read(cx).visible_sessions().iter().filter(|s| s.read(cx).is_running()).count() == 2).await;
    let second = cx.update(|cx| panel.read(cx).active_session().unwrap().read(cx).session_id());
    assert_ne!(first, second);
    assert_eq!(cx.update(|cx| panel.read(cx).layout().groups.clone()), [vec![first, second]], "one group, side by side");

    // Both terminals are shown next to each other (the new one on the right), and the terminal list appears.
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        let (left, right) = (window.find(format!("terminal-pane-{first}")).bounds(), window.find(format!("terminal-pane-{second}")).bounds());
        assert!(right.origin.x >= left.origin.x + left.size.width - gpui_kit::px(2.), "side by side: {left:?} {right:?}");
        assert_eq!(left.origin.y, right.origin.y);
        assert!(window.find(format!("terminal-tab-{first}")).visible());
    });

    // The Split Terminal shortcut while the terminal is focused (Cmd+\ on macOS, which outside the
    // terminal means Split Editor; Ctrl+Shift+5 elsewhere).
    let key = if cfg!(target_os = "macos") { "cmd-\\" } else { "ctrl-shift-5" };
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.press(key, cx);
    });
    window.wait_until(cx, "third terminal from the shortcut", |_, cx| panel.read(cx).visible_sessions().len() == 3).await;
    assert_eq!(editor_terminals(&workbench, cx).len(), 0, "no tab was split");
    let third = cx.update(|cx| panel.read(cx).active_session().unwrap().read(cx).session_id());
    assert_eq!(cx.update(|cx| panel.read(cx).layout().groups.clone()), [vec![first, second, third]]);

    // Unsplit separates the active terminal from its group.
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
    window.wait_until(cx, "terminal became a tab", |_, cx| panel.read(cx).sessions().is_empty() && panel.read(cx).is_in_editor(id)).await;
    assert_eq!(editor_terminals(&workbench, cx), [[id]]);
    // The same shell process keeps running and receives typing in the editor tab.
    assert!(cx.update(|cx| session.read(cx).is_running()));
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        assert!(session.read(cx).focus_handle_is_focused(window), "the terminal tab is focused");
        window.input("echo moved > moved.txt", cx);
        window.press("enter", cx);
    });
    let out = root.join("moved.txt");
    window.wait_until(cx, "the shell in the editor tab writes a file", |_, _| std::fs::read(&out).is_ok_and(|bytes| shell_text(&bytes).trim_end() == "moved")).await;

    assert!(window.run_command(cx, "terminal.moveToPanel"));
    window.wait_until(cx, "back in the panel", |_, cx| panel.read(cx).sessions().len() == 1 && !panel.read(cx).is_in_editor(id)).await;
    assert!(editor_terminals(&workbench, cx).is_empty(), "the terminal tab is closed");
    assert!(cx.update(|cx| panel.read(cx).sessions()[0] == session && session.read(cx).is_running()), "the same terminal, still running");
}

#[gpui_kit::test]
async fn dragging_terminals_between_the_panel_list_and_the_editor_area(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_terminal::init);
    let (window, workbench, panel) = open_with_terminal(&app, cx).await;
    let first = cx.update(|cx| panel.read(cx).active_session().unwrap().read(cx).session_id());
    assert!(window.run_command(cx, "terminal.new"));
    window.wait_until(cx, "second terminal", |_, cx| panel.read(cx).sessions().len() == 2).await;
    let second = cx.update(|cx| panel.read(cx).active_session().unwrap().read(cx).session_id());

    // From the panel list to the empty editor area: it becomes a tab, removed from the panel.
    window.update(cx, |window, cx| window.drag_to(format!("terminal-tab-{first}"), "editor-empty", cx));
    cx.run_until_parked();
    window.wait_until(cx, "terminal became a tab", |_, cx| panel.read(cx).is_in_editor(first)).await;
    assert_eq!(editor_terminals(&workbench, cx), [[first]]);
    assert_eq!(session_ids(&cx.update(|cx| panel.read(cx).sessions()), cx), [second]);

    // From an editor tab to the left edge of a panel terminal: side by side, to the left of that terminal.
    let tab = format!("tab-1-{}", session_tab(first, "").id);
    drag_to_point(&window, cx, &tab, &format!("terminal-pane-{second}"), 0.05, 0.5);
    window.wait_until(cx, "back in the panel", |_, cx| !panel.read(cx).is_in_editor(first)).await;
    assert!(editor_terminals(&workbench, cx).is_empty());
    assert_eq!(cx.update(|cx| panel.read(cx).layout().groups.clone()), [vec![first, second]]);
    assert!(cx.update(|cx| panel.read(cx).sessions().iter().all(|s| s.read(cx).is_running())), "the processes keep running");

    // The list: dropping a terminal in the empty space below the list takes it out of the split.
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
    window.wait_until(cx, "terminal in the editor area", |_, cx| panel.read(cx).editor_sessions().len() == 1).await;
    let editor = cx.update(|cx| panel.read(cx).editor_sessions()[0].clone());
    window.wait_until(cx, "the shell in the tab is running", |_, cx| editor.read(cx).is_running()).await;
    // The tab title follows the terminal's title.
    let title = cx.update(|cx| editor.read(cx).title().to_string());
    window.wait_until(cx, "tab title", |_, cx| workbench.read(cx).model().all_tabs().any(|t| t.title == title)).await;

    // "Split Editor" on a terminal tab opens a new terminal in the adjacent group, not a second view.
    assert!(window.run_command(cx, "workbench.split"));
    window.wait_until(cx, "second terminal in the editor area", |_, cx| panel.read(cx).editor_sessions().len() == 2).await;
    let tabs = editor_terminals(&workbench, cx);
    assert_eq!(tabs.len(), 2, "two groups: {tabs:?}");
    assert_ne!(tabs[0], tabs[1]);

    // Closing the tab kills its terminal; the panel terminal is untouched.
    let id = tabs[1][0];
    assert!(window.run_command(cx, "workbench.closeTab"));
    window.wait_until(cx, "the tab terminal is dead", |_, cx| panel.read(cx).session(id).is_none()).await;
    assert_eq!(panel_count(&panel, cx), 1);
    assert_eq!(cx.update(|cx| panel.read(cx).editor_sessions().len()), 1);
}

fn panel_count(panel: &Entity<TerminalPanel>, cx: &mut TestAppContext) -> usize {
    cx.update(|cx| panel.read(cx).sessions().len())
}

// ----- Font and view -----

#[gpui_kit::test]
async fn font_settings_apply_to_open_terminals_and_line_drawing_renders(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_terminal::init);
    let (window, workbench, panel) = open_with_terminal(&app, cx).await;
    let session = cx.update(|cx| panel.read(cx).active_session().unwrap().clone());
    window.update(cx, |window, cx| window.render_frame(cx));
    let font_size = cx.update(|cx| workbench.read(cx).settings().editor.font_size) as f32;
    let (_, height) = cx.update(|cx| session.read(cx).cell_size()).expect("already drawn");
    assert!((f32::from(height) - font_size * 1.2).abs() <= 0.5, "default line height 120%: {height:?}");
    cx.run_until_parked();
    let rows_before = cx.update(|cx| session.read(cx).grid_size().1);

    // New settings apply to terminals that are already open. A font that is not installed is not
    // used (previously the terminal switched to that font after the settings changed).
    let mut settings = app.core.user_settings();
    settings.terminal.line_height = Some(150);
    settings.terminal.font_family = Some("Font Not Installed".into());
    let core = app.core.clone();
    app.runtime.block_on(async move { oncode_core::api::app::settings_set(&core, settings).await }).unwrap();
    window.wait_until(cx, "new settings are read", |_, cx| workbench.read(cx).settings().terminal.line_height == Some(150)).await;
    window.update(cx, |window, cx| window.render_frame(cx));
    let (_, height) = cx.update(|cx| session.read(cx).cell_size()).unwrap();
    assert!((f32::from(height) - font_size * 1.5).abs() <= 0.5, "line height 150%: {height:?}");
    // The number of grid lines adjusts too even though the panel size does not change.
    window.wait_until(cx, "grid is fitted", |_, cx| session.read(cx).grid_size().1 < rows_before).await;
    assert_ne!(cx.update(|cx| session.read(cx).font_family().to_string()), "Font Not Installed");

    // Box drawing, blocks, and powerline separators are drawn as shapes without errors.
    window.update(cx, |window, cx| {
        window.input("echo ╭─┬─╮ ║ ▌█▒ \u{E0B0}\u{E0B4}", cx);
        window.render_frame(cx);
    });
    if cfg!(unix) {
        window.wait_until(cx, "line characters show up", |_, cx| session.read(cx).text().contains("╭─┬─╮")).await;
    }
    window.update(cx, |window, cx| window.render_frame(cx));
}

// ----- PTY size -----

/// The terminal is created before the first layout, so the PTY is born 80x24. The real size is only known
/// after layout, and spawn runs async so the PTY id may not exist yet at that point. The shell must still
/// see the same size as the grid; otherwise a full-screen program (e.g. Claude CLI) draws
/// for 80x24 in a much wider area until there is a manual resize.
#[gpui_kit::test]
async fn shell_sees_the_same_size_as_the_grid_after_the_first_layout(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_terminal::init);
    let project = app.project(&[("a.txt", "a\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);

    assert!(window.run_command(cx, "terminal.toggle"));
    let panel = cx.update(|cx| oncode_ui_terminal::panel(workbench.read(cx)).unwrap());
    window.wait_until(cx, "shell is running", |_, cx| panel.read(cx).active_session().is_some_and(|s| s.read(cx).is_running())).await;
    let session = cx.update(|cx| panel.read(cx).active_session().unwrap().clone());
    window.update(cx, |window, cx| window.render_frame(cx));
    cx.run_until_parked();

    let (cols, rows) = cx.update(|cx| session.read(cx).grid_size());
    assert_ne!((cols, rows), (80, 24), "this test only means something if the panel does not happen to be 80x24");

    // Windows: `default_shell` always picks pwsh or powershell before cmd.
    let command = if cfg!(windows) { r#"Write-Output "$([Console]::WindowWidth) $([Console]::WindowHeight)" > size.txt"# } else { "stty size | awk '{print $2, $1}' > size.txt" };
    window.update(cx, |window, cx| {
        window.input(command, cx);
        window.press("enter", cx);
    });
    let out = project.join("size.txt");
    window.wait_until(cx, "the shell writes the PTY size", |_, _| std::fs::read(&out).is_ok_and(|bytes| !shell_text(&bytes).trim().is_empty())).await;
    let seen = shell_text(&std::fs::read(&out).unwrap());
    assert_eq!(seen.trim(), format!("{cols} {rows}"), "the PTY size according to the shell differs from the emulator grid");
}
