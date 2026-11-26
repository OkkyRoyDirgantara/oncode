//! Tests of the Explorer view, the welcome screen, and restricted mode through a real (headless) window
//! with a real backend. The end result is checked on disk and through `git`.

use std::path::Path;

use gpui_kit::test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _};
use gpui_kit::component::WindowExt as _;
use gpui_kit::{
    point, App, AppContext as _, Entity, ExternalPaths, FileDropEvent, Focusable, InputEvent as _, IntoElement, MouseButton, ParentElement as _, Render, Styled as _, TestAppContext,
    Window,
};
use oncode_ui::editor::{active_file, buffers, open_file, OpenFile};
use oncode_ui::testing::{git, TestApp, TestWindow};
use oncode_ui::{Command, OpenOptions, SidebarView, TabContent, TabData, TabKind, ViewTab, Workbench};
use oncode_ui_explorer::tree::Row;
use oncode_ui_explorer::{Explorer, RevealHandler, REVEAL_LABEL};

/// A path as text in the form tabs and buffers use (`\` on Windows), so that
/// `project.join("src/app")` equals the path the explorer builds.
fn text(path: &Path) -> String {
    oncode_ui::editor::file_path(&path.to_string_lossy())
}

/// Tree rows as text: the relative path, `/` for folders, indent per level.
fn shape(explorer: &Entity<Explorer>, root: &Path, cx: &App) -> Vec<String> {
    let root = text(root);
    explorer
        .read(cx)
        .rows()
        .iter()
        .map(|row| match row {
            Row::Entry { entry, depth, .. } => {
                // The entry path uses the system separator (`\` on Windows); its text form always uses `/`.
                let relative = entry.path.strip_prefix(&root).unwrap_or(&entry.path).trim_start_matches(['/', '\\']).replace('\\', "/");
                format!("{}{relative}{}", "  ".repeat(*depth), if entry.is_dir { "/" } else { "" })
            }
            Row::Note { depth, text, .. } => format!("{}({text})", "  ".repeat(*depth)),
        })
        .collect()
}

struct Fixture {
    window: TestWindow,
    workbench: Entity<Workbench>,
    explorer: Entity<Explorer>,
}

impl Fixture {
    /// Opens a folder and waits until the contents of the tree root are loaded.
    async fn open(app: &TestApp, project: &Path, cx: &mut TestAppContext) -> Self {
        let window = app.open(Some(project), cx).await;
        let fixture = Self::current(window, cx);
        fixture.settle(cx).await;
        fixture
    }

    /// The Workbench and Explorer currently installed (replaced when the folder is trusted).
    fn current(window: TestWindow, cx: &mut TestAppContext) -> Self {
        let workbench = window.workbench(cx);
        let explorer = cx.update(|cx| oncode_ui_explorer::explorer(workbench.read(cx)).expect("explorer registered"));
        Self { window, workbench, explorer }
    }

    /// Waits until no folder is being loaded.
    async fn settle(&self, cx: &mut TestAppContext) {
        let explorer = self.explorer.clone();
        self.window.wait_until(cx, "tree loaded", move |_, cx| !explorer.read(cx).rows().is_empty() && !explorer.read(cx).is_loading()).await;
    }

    async fn wait_for_rows(&self, cx: &mut TestAppContext, what: &str, root: &Path, condition: impl Fn(&[String]) -> bool) {
        let explorer = self.explorer.clone();
        let root = root.to_path_buf();
        self.window.wait_until(cx, what, move |_, cx| condition(&shape(&explorer, &root, cx))).await;
    }

    fn click(&self, cx: &mut TestAppContext, id: &str) {
        let id = id.to_string();
        self.window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.click(gpui_kit::SharedString::from(id), cx);
        });
    }

    fn double_click(&self, cx: &mut TestAppContext, id: &str) {
        let id = id.to_string();
        self.window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.double_click(gpui_kit::SharedString::from(id), cx);
        });
    }

    fn press(&self, cx: &mut TestAppContext, key: &str) {
        self.window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        });
        cx.run_until_parked();
    }

    fn input(&self, cx: &mut TestAppContext, text: &str) {
        self.window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.input(text, cx);
        });
        cx.run_until_parked();
    }

    fn select(&self, cx: &mut TestAppContext, path: &Path) {
        let path = text(path);
        self.explorer.update(cx, |explorer, cx| explorer.select(Some(path), cx));
    }

    fn selected(&self, cx: &mut TestAppContext) -> Option<String> {
        cx.update(|cx| self.explorer.read(cx).selected().map(String::from))
    }

    /// Moves keyboard focus to the tree through the command, like Ctrl+Shift+E.
    fn focus_tree(&self, cx: &mut TestAppContext) {
        assert!(self.window.run_command(cx, "explorer.focus"));
        self.window.update(cx, |window, cx| window.render_frame(cx));
        assert!(self.window.update(cx, |window, cx| self.explorer.read(cx).tree_focused(window)), "focus is in the tree");
    }

    async fn wait_for_active_file(&self, cx: &mut TestAppContext, path: &Path) {
        let (workbench, path) = (self.workbench.clone(), text(path));
        self.window
            .wait_until(cx, "active file loaded", move |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).path() == path && buffer.read(cx).is_ready()))
            .await;
    }

    fn active_tab(&self, cx: &mut TestAppContext) -> Option<TabData> {
        cx.update(|cx| self.workbench.read(cx).model().active().map(|(_, tab)| tab.clone()))
    }

    fn tab_ids(&self, cx: &mut TestAppContext) -> Vec<String> {
        cx.update(|cx| self.workbench.read(cx).model().all_tabs().map(|tab| tab.id.clone()).collect())
    }
}

fn setup(cx: &mut TestAppContext) -> TestApp {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_explorer::init);
    app
}

// ----- v0.2 e2e scenario: the explorer opens a file; edit, save, and Git markers follow -----

#[gpui_kit::test]
async fn opens_a_file_from_the_tree_and_git_marks_follow_the_edit(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.git_project(&[
        ("README.md", "# Demo\n\nLine one\n"),
        ("src/main.rs", "fn main() {}\n"),
        ("src/util.rs", "pub fn util() {}\n"),
        (".gitignore", "target/\n"),
        ("target/build.log", "output\n"),
    ]);
    let f = Fixture::open(&app, &project, cx).await;

    let rows = cx.update(|cx| shape(&f.explorer, &project, cx));
    assert!(rows.contains(&"src/".to_string()) && rows.contains(&"README.md".to_string()), "root contents are shown: {rows:?}");
    assert!(!rows.iter().any(|row| row.contains("main.rs")), "folder contents are not loaded before opening: {rows:?}");
    let folders_first = rows.iter().position(|row| row == "src/").unwrap() < rows.iter().position(|row| row == "README.md").unwrap();
    assert!(folders_first, "folders first: {rows:?}");
    // Entries ignored by git are marked so they are drawn dimmed.
    let ignored = cx.update(|cx| f.explorer.read(cx).rows().iter().filter_map(Row::entry).find(|entry| entry.name == "target").map(|entry| entry.ignored));
    assert_eq!(ignored, Some(true));

    f.click(cx, "explorer-row-src");
    f.wait_for_rows(cx, "src contents loaded", &project, |rows| rows.contains(&"  src/main.rs".to_string())).await;

    // Single click: a preview tab.
    f.click(cx, "explorer-row-src/main.rs");
    f.wait_for_active_file(cx, &project.join("src/main.rs")).await;
    assert!(f.active_tab(cx).unwrap().preview, "a single click opens a preview");
    assert_eq!(f.selected(cx), Some(text(&project.join("src/main.rs"))));

    // Double click: a permanent tab.
    f.double_click(cx, "explorer-row-README.md");
    f.wait_for_active_file(cx, &project.join("README.md")).await;
    let tab = f.active_tab(cx).unwrap();
    assert_eq!((tab.title.as_str(), tab.preview), ("README.md", false));

    f.input(cx, "New line\n");
    let (_, buffer, state) = cx.update(|cx| active_file(f.workbench.read(cx), cx).unwrap());
    assert!(cx.update(|cx| buffer.read(cx).is_dirty()));
    assert_eq!(std::fs::read_to_string(project.join("README.md")).unwrap(), "# Demo\n\nLine one\n", "not written to disk before saving");

    assert!(f.window.run_command(cx, "file.save"));
    f.window.wait_until(cx, "saved", |_, cx| !buffer.read(cx).is_dirty()).await;
    assert_eq!(std::fs::read_to_string(project.join("README.md")).unwrap(), cx.update(|cx| state.read(cx).value().to_string()));
    assert_eq!(git(&project, &["status", "--porcelain"]).trim(), "M README.md", "git sees the change");

    let readme = text(&project.join("README.md"));
    let explorer = f.explorer.clone();
    f.window.wait_until(cx, "explorer marks the changed file", move |_, cx| explorer.read(cx).decoration(&readme, false).is_some_and(|d| d.letter == "M")).await;

    // A folder shows there are changes inside it; a new file is marked U.
    std::fs::write(project.join("src/new.rs"), "// new\n").unwrap();
    let (src, new_file) = (text(&project.join("src")), text(&project.join("src/new.rs")));
    let explorer = f.explorer.clone();
    f.window
        .wait_until(cx, "folder and new file marked", move |_, cx| {
            let explorer = explorer.read(cx);
            explorer.decoration(&src, true).is_some_and(|d| d.letter.is_empty()) && explorer.decoration(&new_file, false).is_some_and(|d| d.letter == "U")
        })
        .await;
}

// ----- Keyboard navigation -----

#[gpui_kit::test]
async fn moves_the_selection_and_expands_or_collapses_with_the_keyboard(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("docs/a.md", "a"), ("src/app/store.ts", "s"), ("src/main.ts", "m"), ("README.md", "# r\n")]);
    let f = Fixture::open(&app, &project, cx).await;
    assert_eq!(cx.update(|cx| shape(&f.explorer, &project, cx)), ["docs/", "src/", "README.md"]);

    f.focus_tree(cx);
    f.press(cx, "down");
    assert_eq!(f.selected(cx), Some(text(&project.join("docs"))));
    f.press(cx, "up");
    assert_eq!(f.selected(cx), Some(text(&project.join("docs"))), "does not go past the first row");
    f.press(cx, "down");
    assert_eq!(f.selected(cx), Some(text(&project.join("src"))));

    // Right opens a folder; right again enters its first child.
    f.press(cx, "right");
    f.wait_for_rows(cx, "src opened", &project, |rows| rows == ["docs/", "src/", "  src/app/", "  src/main.ts", "README.md"]).await;
    assert_eq!(f.selected(cx), Some(text(&project.join("src"))));
    f.press(cx, "right");
    assert_eq!(f.selected(cx), Some(text(&project.join("src/app"))));

    // Left on a closed folder moves to its parent; left on an open folder closes it.
    f.press(cx, "left");
    assert_eq!(f.selected(cx), Some(text(&project.join("src"))));
    f.press(cx, "left");
    assert_eq!(cx.update(|cx| shape(&f.explorer, &project, cx)), ["docs/", "src/", "README.md"]);

    f.press(cx, "end");
    assert_eq!(f.selected(cx), Some(text(&project.join("README.md"))));
    f.press(cx, "home");
    assert_eq!(f.selected(cx), Some(text(&project.join("docs"))));

    // Enter opens/closes a folder and opens a file as a preview.
    f.press(cx, "enter");
    f.wait_for_rows(cx, "docs opened", &project, |rows| rows.contains(&"  docs/a.md".to_string())).await;
    f.press(cx, "end");
    f.press(cx, "enter");
    f.wait_for_active_file(cx, &project.join("README.md")).await;
    assert!(f.active_tab(cx).unwrap().preview);
}

// ----- The file of the active tab is highlighted -----

#[gpui_kit::test]
async fn reveals_the_file_of_the_active_tab_by_expanding_its_ancestors(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("src/app/deep/store.ts", "s"), ("src/main.ts", "m"), ("README.md", "# r\n")]);
    let f = Fixture::open(&app, &project, cx).await;
    let store = project.join("src/app/deep/store.ts");

    let path = text(&store);
    f.window.update(cx, |window, cx| f.workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&path), window, cx)));
    f.wait_for_rows(cx, "ancestors opened", &project, |rows| rows == ["src/", "  src/app/", "    src/app/deep/", "      src/app/deep/store.ts", "  src/main.ts", "README.md"]).await;
    assert_eq!(f.selected(cx), Some(text(&store)));

    // A path outside the project does not change the tree.
    let outside = text(&app.dirs.path().join("outside.txt"));
    f.explorer.update(cx, |explorer, cx| explorer.reveal(&outside, cx));
    cx.run_until_parked();
    assert_eq!(f.selected(cx), Some(text(&store)));
}

// ----- New entries -----

#[gpui_kit::test]
async fn creates_a_nested_file_inside_the_selected_folder_reveals_and_opens_it(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("src/a.txt", "a\n"), ("README.md", "# r\n")]);
    let f = Fixture::open(&app, &project, cx).await;

    f.select(cx, &project.join("src"));
    assert!(f.window.run_command(cx, "explorer.newFile"));
    assert!(cx.update(|cx| f.explorer.read(cx).is_editing()));
    f.settle(cx).await;
    assert!(cx.update(|cx| f.explorer.read(cx).is_expanded(&text(&project.join("src")))), "the target folder is opened");

    // An existing name is rejected with a message, and Enter creates nothing.
    f.input(cx, "a.txt");
    assert!(cx.update(|cx| f.explorer.read(cx).edit_error()).unwrap().contains("already exists"));
    f.press(cx, "enter");
    assert!(cx.update(|cx| f.explorer.read(cx).is_editing()), "the input stays open while the name is invalid");

    // Escape cancels.
    f.press(cx, "escape");
    assert!(!cx.update(|cx| f.explorer.read(cx).is_editing()));

    assert!(f.window.run_command(cx, "explorer.newFile"));
    f.input(cx, "lib/new.rs");
    assert_eq!(cx.update(|cx| f.explorer.read(cx).edit_error()), None);
    f.press(cx, "enter");

    let created = project.join("src/lib/new.rs");
    f.wait_for_active_file(cx, &created).await;
    assert!(created.is_file(), "the file is created together with its intermediate folders");
    assert!(!f.active_tab(cx).unwrap().preview, "the new file is opened as a permanent tab");
    f.wait_for_rows(cx, "new file shown", &project, |rows| rows == ["src/", "  src/lib/", "    src/lib/new.rs", "  src/a.txt", "README.md"]).await;
    assert_eq!(f.selected(cx), Some(text(&created)));
}

#[gpui_kit::test]
async fn creates_a_folder_at_the_project_root_from_the_toolbar(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("README.md", "# r\n")]);
    let f = Fixture::open(&app, &project, cx).await;

    f.click(cx, "explorer-new-folder");
    assert!(cx.update(|cx| f.explorer.read(cx).is_editing()));
    f.input(cx, "docs");
    f.press(cx, "enter");
    f.wait_for_rows(cx, "new folder shown", &project, |rows| rows == ["docs/", "README.md"]).await;
    assert!(project.join("docs").is_dir());
    assert_eq!(f.selected(cx), Some(text(&project.join("docs"))));
    assert!(f.tab_ids(cx).is_empty(), "a new folder does not open a tab");
}

// ----- Rename -----

#[gpui_kit::test]
async fn renames_a_file_with_f2_and_keeps_unsaved_content_on_the_new_path(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("notes.txt", "one\n"), ("other.txt", "other\n")]);
    let f = Fixture::open(&app, &project, cx).await;

    f.double_click(cx, "explorer-row-notes.txt");
    f.wait_for_active_file(cx, &project.join("notes.txt")).await;
    f.input(cx, "two ");
    assert!(cx.update(|cx| f.workbench.read(cx).has_dirty_tabs(cx)));

    f.focus_tree(cx);
    f.press(cx, "f2");
    assert!(cx.update(|cx| f.explorer.read(cx).is_editing()), "F2 opens the name input");
    // The name without the extension is selected, so typing replaces only that part.
    f.input(cx, "memo");
    f.press(cx, "enter");

    let renamed = project.join("memo.txt");
    f.wait_for_rows(cx, "new name shown", &project, |rows| rows == ["memo.txt", "other.txt"]).await;
    assert!(renamed.is_file() && !project.join("notes.txt").exists(), "the file is renamed on disk");
    assert_eq!(f.selected(cx), Some(text(&renamed)));
    assert_eq!(f.tab_ids(cx), [format!("file:{}", text(&renamed))]);
    assert_eq!(f.active_tab(cx).unwrap().title, "memo.txt");

    // Unsaved contents move along, and saving writes to the new path.
    f.wait_for_active_file(cx, &renamed).await;
    let (_, buffer, state) = cx.update(|cx| active_file(f.workbench.read(cx), cx).unwrap());
    assert_eq!(cx.update(|cx| state.read(cx).value().to_string()), "two one\n");
    assert!(cx.update(|cx| buffer.read(cx).is_dirty()));
    assert_eq!(std::fs::read_to_string(&renamed).unwrap(), "one\n");
    buffer.update_in_window(&f.window, cx);
    f.window.wait_until(cx, "saved", |_, cx| !buffer.read(cx).is_dirty()).await;
    assert_eq!(std::fs::read_to_string(&renamed).unwrap(), "two one\n");
    assert!(!project.join("notes.txt").exists(), "the old path is not written again");
}

/// Saves a buffer through the test window.
trait SaveInWindow {
    fn update_in_window(&self, window: &TestWindow, cx: &mut TestAppContext);
}

impl SaveInWindow for Entity<oncode_ui::editor::Buffer> {
    fn update_in_window(&self, window: &TestWindow, cx: &mut TestAppContext) {
        window.update(cx, |window, cx| self.update(cx, |buffer, cx| buffer.save(window, cx)).detach());
    }
}

#[gpui_kit::test]
async fn renaming_a_folder_rewrites_open_tabs_and_keeps_it_expanded(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("src/deep/a.ts", "a\n"), ("src/b.ts", "b\n"), ("other.ts", "o\n")]);
    let f = Fixture::open(&app, &project, cx).await;

    for file in ["src/deep/a.ts", "src/b.ts", "other.ts"] {
        let path = text(&project.join(file));
        f.window.update(cx, |window, cx| f.workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&path), window, cx)));
        f.wait_for_active_file(cx, &project.join(file)).await;
        // Wait for the reveal to finish (ancestor folders loaded and opened) before the next file:
        // an unfinished reveal is superseded by the new active file.
        let explorer = f.explorer.clone();
        f.window.wait_until(cx, "file selected in the tree", move |_, cx| explorer.read(cx).selected() == Some(path.as_str())).await;
    }
    f.wait_for_rows(cx, "tree opened", &project, |rows| rows == ["src/", "  src/deep/", "    src/deep/a.ts", "  src/b.ts", "other.ts"]).await;

    f.select(cx, &project.join("src"));
    f.focus_tree(cx);
    f.press(cx, "f2");
    // A folder's name is selected entirely.
    f.input(cx, "lib");
    f.press(cx, "enter");

    f.wait_for_rows(cx, "folder loaded at the new path", &project, |rows| rows == ["lib/", "  lib/deep/", "    lib/deep/a.ts", "  lib/b.ts", "other.ts"]).await;
    assert!(project.join("lib/deep/a.ts").is_file() && !project.join("src").exists());
    assert_eq!(f.selected(cx), Some(text(&project.join("lib"))));
    let mut ids = f.tab_ids(cx);
    ids.sort();
    let mut expected: Vec<String> = ["lib/b.ts", "lib/deep/a.ts", "other.ts"].iter().map(|file| format!("file:{}", text(&project.join(file)))).collect();
    expected.sort();
    assert_eq!(ids, expected, "file tabs inside the folder are rewritten, other tabs are not");

    // A rewritten tab can still be loaded from its new path.
    let path = text(&project.join("lib/b.ts"));
    f.window.update(cx, |window, cx| f.workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&path), window, cx)));
    f.wait_for_active_file(cx, &project.join("lib/b.ts")).await;
    let (_, _, state) = cx.update(|cx| active_file(f.workbench.read(cx), cx).unwrap());
    assert_eq!(cx.update(|cx| state.read(cx).value().to_string()), "b\n");
    assert!(cx.update(|cx| buffers(f.workbench.read(cx)).unwrap().read(cx).dirty(cx).is_empty()));
}

// ----- Outside changes -----

#[gpui_kit::test]
async fn follows_files_created_and_removed_outside_the_app(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("src/app/store.ts", "s"), ("src/a.ts", "a"), ("README.md", "# r\n")]);
    let f = Fixture::open(&app, &project, cx).await;

    f.click(cx, "explorer-row-src");
    f.wait_for_rows(cx, "src opened", &project, |rows| rows.contains(&"  src/a.ts".to_string())).await;
    f.click(cx, "explorer-row-src/app");
    f.wait_for_rows(cx, "app opened", &project, |rows| rows.contains(&"    src/app/store.ts".to_string())).await;
    f.select(cx, &project.join("src/app/store.ts"));

    std::fs::write(project.join("src/new.ts"), "b").unwrap();
    f.wait_for_rows(cx, "file from outside shown", &project, |rows| rows == ["src/", "  src/app/", "    src/app/store.ts", "  src/a.ts", "  src/new.ts", "README.md"]).await;
    assert_eq!(f.selected(cx), Some(text(&project.join("src/app/store.ts"))), "expansion and selection are kept");

    // A folder that disappears takes the expansion and selection below it away.
    std::fs::remove_dir_all(project.join("src/app")).unwrap();
    f.wait_for_rows(cx, "deleted folder gone", &project, |rows| rows == ["src/", "  src/a.ts", "  src/new.ts", "README.md"]).await;
    assert_eq!(f.selected(cx), None);
    assert!(!cx.update(|cx| f.explorer.read(cx).is_expanded(&text(&project.join("src/app")))));

    // Refresh and Collapse All through commands.
    assert!(f.window.run_command(cx, "explorer.refresh"));
    f.settle(cx).await;
    assert!(f.window.run_command(cx, "explorer.collapseAll"));
    assert_eq!(cx.update(|cx| shape(&f.explorer, &project, cx)), ["src/", "README.md"]);
}

// ----- Copy path and the context menu -----

#[gpui_kit::test]
async fn copies_the_absolute_and_relative_path_of_the_selection(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("src/a.ts", "a")]);
    let f = Fixture::open(&app, &project, cx).await;
    assert!(!f.window.run_command(cx, "explorer.copyPath"), "without a selection the command is disabled");

    f.click(cx, "explorer-row-src");
    f.wait_for_rows(cx, "src opened", &project, |rows| rows.contains(&"  src/a.ts".to_string())).await;
    f.select(cx, &project.join("src/a.ts"));

    assert!(f.window.run_command(cx, "explorer.copyRelativePath"));
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some("src/a.ts".to_string()));
    assert!(f.window.run_command(cx, "explorer.copyPath"));
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some(text(&project.join("src/a.ts"))));
}

#[gpui_kit::test]
async fn the_context_menu_acts_on_the_row_under_the_pointer(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("src/a.ts", "a"), ("README.md", "# r\n")]);
    let f = Fixture::open(&app, &project, cx).await;

    f.window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.right_click("explorer-row-README.md", cx);
    });
    cx.run_until_parked();
    assert_eq!(f.selected(cx), Some(text(&project.join("README.md"))), "a right click selects its row");

    let item = f.window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.find_by_label("Copy Relative Path").bounds().center()
    });
    f.window.update(cx, |window, cx| {
        window.pointer_move(item, None, cx);
        window.pointer_down(item, MouseButton::Left, cx);
        window.pointer_up(item, MouseButton::Left, cx);
    });
    cx.run_until_parked();
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some("README.md".to_string()));

    // The menu for a folder offers creating inside it, not "Open".
    f.window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.right_click("explorer-row-src", cx);
    });
    cx.run_until_parked();
    let (has_new_file, has_open) = f.window.update(cx, |window, cx| {
        window.render_frame(cx);
        (window.try_find_by_label("New File…").is_some(), window.try_find_by_label("Open").is_some())
    });
    assert!(has_new_file && !has_open);
}

// ----- Reveal in File Explorer / Finder -----

/// Installs a replacement for the file manager opener (the GPUI test platform does not implement it) and
/// returns the list of paths that were requested to be opened.
fn capture_reveals(cx: &mut TestAppContext) -> std::rc::Rc<std::cell::RefCell<Vec<String>>> {
    let revealed = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let sink = revealed.clone();
    cx.update(|cx| cx.set_global(RevealHandler(std::rc::Rc::new(move |path: &Path, _: &mut App| sink.borrow_mut().push(text(path))))));
    revealed
}

impl Fixture {
    /// Right-clicks row `id`, then picks the menu item labeled `label`.
    fn context_menu(&self, cx: &mut TestAppContext, id: &str, label: &str) {
        let id = id.to_string();
        self.window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.right_click(gpui_kit::SharedString::from(id), cx);
        });
        cx.run_until_parked();
        let item = self.window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.find_by_label(label).bounds().center()
        });
        self.window.update(cx, |window, cx| {
            window.pointer_move(item, None, cx);
            window.pointer_down(item, MouseButton::Left, cx);
            window.pointer_up(item, MouseButton::Left, cx);
        });
        cx.run_until_parked();
    }
}

#[gpui_kit::test]
async fn the_context_menu_reveals_a_file_or_a_folder_in_the_system_file_manager(cx: &mut TestAppContext) {
    let app = setup(cx);
    // A new folder is not trusted yet: reveal must still work in restricted mode.
    let project = app.project(&[("src/a.ts", "a"), ("README.md", "# r\n")]);
    let f = Fixture::open(&app, &project, cx).await;
    assert!(!cx.update(|cx| f.workbench.read(cx).env().trusted));
    let revealed = capture_reveals(cx);

    f.context_menu(cx, "explorer-row-README.md", REVEAL_LABEL);
    assert_eq!(*revealed.borrow(), [text(&project.join("README.md"))]);

    f.context_menu(cx, "explorer-row-src", REVEAL_LABEL);
    assert_eq!(*revealed.borrow(), [text(&project.join("README.md")), text(&project.join("src"))], "a folder can be revealed too");

    // The tree background is not a file or folder: there is no reveal item.
    f.window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.right_click("explorer-tree", cx);
    });
    cx.run_until_parked();
    let (has_background_menu, has_reveal) = f.window.update(cx, |window, cx| {
        window.render_frame(cx);
        (window.try_find_by_label("Refresh").is_some(), window.try_find_by_label(REVEAL_LABEL).is_some())
    });
    assert!(has_background_menu, "the tree background menu is open");
    assert!(!has_reveal);
}

#[gpui_kit::test]
async fn the_command_reveals_the_active_file_and_is_off_without_one(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("src/a.ts", "a")]);
    let f = Fixture::open(&app, &project, cx).await;
    let revealed = capture_reveals(cx);
    assert!(!f.window.run_command(cx, "explorer.revealActiveFile"), "without an active file tab the command is disabled");
    assert!(revealed.borrow().is_empty());

    f.click(cx, "explorer-row-src");
    f.wait_for_rows(cx, "src opened", &project, |rows| rows.contains(&"  src/a.ts".to_string())).await;
    f.open_and_reveal(cx, &project.join("src/a.ts")).await;
    assert!(f.window.run_command(cx, "explorer.revealActiveFile"));
    assert_eq!(*revealed.borrow(), [text(&project.join("src/a.ts"))]);
}

// ----- v0.2 e2e scenario: welcome screen without a folder; Settings can still be opened -----

struct FakeSettings {
    focus_handle: gpui_kit::FocusHandle,
}

impl Focusable for FakeSettings {
    fn focus_handle(&self, _: &App) -> gpui_kit::FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for FakeSettings {
    fn render(&mut self, _: &mut Window, _: &mut gpui_kit::Context<Self>) -> impl IntoElement {
        gpui_kit::div().size_full().child("Settings")
    }
}

struct FakeSettingsKind;

impl TabKind for FakeSettingsKind {
    fn build(&self, _: &TabData, _: &oncode_ui::Env, _: &mut Window, cx: &mut App) -> Box<dyn TabContent> {
        Box::new(ViewTab(cx.new(|cx| FakeSettings { focus_handle: cx.focus_handle() })))
    }
}

/// A stand-in for the settings crate: a `settings.open` command that opens a tab.
fn register_fake_settings(cx: &mut App) {
    oncode_ui::register_tab_kind(cx, "settings", FakeSettingsKind);
    oncode_ui::register_feature(cx, |workbench, _, cx| {
        let weak = cx.weak_entity();
        workbench.register_command(Command::new("settings.open", "Open Settings", move |_, window, cx| {
            let _ = weak.update(cx, |workbench, cx| {
                workbench.open_tab(TabData::new("settings", "Settings", serde_json::Value::Null), OpenOptions::default(), window, cx);
            });
        }));
    });
}

#[gpui_kit::test]
async fn welcome_screen_without_a_folder_still_opens_settings(cx: &mut TestAppContext) {
    let app = setup(cx);
    cx.update(register_fake_settings);
    let window = app.open(None, cx).await;
    let workbench = window.workbench(cx);
    let welcome = cx.update(|cx| oncode_ui_explorer::welcome(workbench.read(cx)).unwrap());
    window.wait_until(cx, "app info loaded", |_, cx| welcome.read(cx).info().is_some()).await;

    let visible = |cx: &mut TestAppContext, id: &'static str| {
        window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some_and(|element| element.visible())
        })
    };
    assert!(visible(cx, "welcome-title"), "the welcome screen is shown");
    assert!(visible(cx, "welcome-open-folder") && visible(cx, "welcome-clone") && visible(cx, "welcome-settings"));
    assert!(!visible(cx, "welcome-git-problem"), "git is available on the test machine");
    assert!(visible(cx, "explorer-open-folder"), "the sidebar offers opening a folder");
    // Without the Source Control crate, `git.clone` is not registered and its button is disabled.
    assert_eq!(window.update(cx, |window, _| window.find("welcome-clone").disabled()), Some(true));

    window.update(cx, |window, cx| window.click("welcome-settings", cx));
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| workbench.read(cx).model().active().map(|(_, tab)| tab.kind.clone())), Some("settings".to_string()));
    assert!(!visible(cx, "welcome-title"), "the Settings tab replaces the welcome screen");

    // Closing the last tab returns to the welcome screen.
    assert!(window.run_command(cx, "workbench.closeTab"));
    cx.run_until_parked();
    assert!(visible(cx, "welcome-title"));
}

#[gpui_kit::test]
async fn welcome_lists_recent_folders_removes_one_and_opens_another(cx: &mut TestAppContext) {
    let app = setup(cx);
    let first = app.project(&[("a.txt", "a")]);
    let second = app.project(&[("b.txt", "b")]);
    app.core.store.add_recent(&text(&first));
    app.core.store.add_recent(&text(&second));

    let window = app.open(None, cx).await;
    let workbench = window.workbench(cx);
    let welcome = cx.update(|cx| oncode_ui_explorer::welcome(workbench.read(cx)).unwrap());
    window.wait_until(cx, "recent folders loaded", |_, cx| welcome.read(cx).info().is_some_and(|info| info.recent.len() == 2)).await;

    let remove = format!("welcome-recent-remove-{}", text(&first));
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click(gpui_kit::SharedString::from(remove), cx);
    });
    let core = app.core.clone();
    let kept = text(&second);
    window.wait_until(cx, "folder removed from the list", move |_, _| core.store.recent() == [kept.clone()]).await;
    assert!(cx.update(|cx| workbench.read(cx).env().root.is_none()), "removing from the list does not open the folder");

    let open = format!("welcome-recent-{}", text(&second));
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click(gpui_kit::SharedString::from(open), cx);
    });
    window.wait_until(cx, "recent folder opened", |shell, cx| shell.read(cx).workspace().is_some_and(|workspace| workspace.root == text(&second))).await;

    // A folder is open without tabs: shortcut hints, not the welcome screen.
    let (hint, title) = window.update(cx, |window, cx| {
        window.render_frame(cx);
        (window.try_find("editor-hint").is_some(), window.try_find("welcome-title").is_some())
    });
    assert!(hint && !title);
}

// ----- v0.2 e2e scenario: a new folder opens in restricted mode until trusted -----

struct FakeScm {
    focus_handle: gpui_kit::FocusHandle,
}

impl Render for FakeScm {
    fn render(&mut self, _: &mut Window, _: &mut gpui_kit::Context<Self>) -> impl IntoElement {
        gpui_kit::InteractiveElement::track_focus(gpui_kit::div(), &self.focus_handle).child("Source Control")
    }
}

/// A stand-in for the Source Control crate: like the real one, it is only registered if the folder is trusted.
fn register_fake_scm(cx: &mut App) {
    oncode_ui::register_feature(cx, |workbench, _, _| {
        if !workbench.env().trusted {
            return;
        }
        workbench.register_sidebar(SidebarView::new("scm", "Source Control", oncode_ui::Lucide::GitBranch, 30, |_, _, cx| cx.new(|cx| FakeScm { focus_handle: cx.focus_handle() }).into()));
    });
}

#[gpui_kit::test]
async fn an_untrusted_folder_opens_in_restricted_mode_until_trusted(cx: &mut TestAppContext) {
    let app = setup(cx);
    cx.update(register_fake_scm);
    // An untrusted git repository.
    let project = app.project(&[("README.md", "# Demo\n"), ("src/main.rs", "fn main() {}\n")]);
    git(&project, &["init", "-q", "-b", "main"]);
    git(&project, &["add", "-A"]);
    git(&project, &["commit", "-q", "-m", "First commit"]);

    let f = Fixture::open(&app, &project, cx).await;
    let env = cx.update(|cx| f.workbench.read(cx).env().clone());
    assert!(!env.trusted && env.repo_root.is_none(), "a new folder is not trusted yet");
    let (banner, status) = f.window.update(cx, |window, cx| {
        window.render_frame(cx);
        (window.try_find("restricted-banner").is_some_and(|e| e.visible()), window.try_find("status-restricted").is_some_and(|e| e.visible()))
    });
    assert!(banner && status, "the banner in the Explorer and the status bar item are shown");
    assert_eq!(cx.update(|cx| f.workbench.read(cx).active_sidebar().map(|id| id.to_string())), Some("explorer".to_string()));
    assert!(cx.update(|cx| oncode_ui::repos(cx).read(cx).roots().is_empty()), "Git is not run in restricted mode");

    // The tree remains usable; its expansion must persist after the folder is trusted.
    f.click(cx, "explorer-row-src");
    f.wait_for_rows(cx, "src opened", &project, |rows| rows.contains(&"  src/main.rs".to_string())).await;

    f.click(cx, "restricted-trust");
    f.window.wait_until(cx, "folder trusted", |shell, cx| shell.read(cx).workspace().is_some_and(|workspace| workspace.trusted && workspace.repo_root.is_some())).await;
    assert!(app.core.store.is_trusted(&project));

    let f = Fixture::current(f.window, cx);
    f.settle(cx).await;
    let (banner, status) = f.window.update(cx, |window, cx| {
        window.render_frame(cx);
        (window.try_find("restricted-banner").is_some(), window.try_find("status-restricted").is_some())
    });
    assert!(!banner && !status, "the restricted mode notices are gone");
    assert_eq!(cx.update(|cx| f.workbench.read(cx).active_sidebar().map(|id| id.to_string())), Some("scm".to_string()), "Source Control becomes the active view after the folder is trusted");
    f.wait_for_rows(cx, "expansion kept", &project, |rows| rows.contains(&"  src/main.rs".to_string())).await;
    assert!(!f.window.run_command(cx, "workspace.trust"), "the trust command is no longer active");
}

#[gpui_kit::test]
async fn startup_view_setting_keeps_the_explorer_for_a_repository(cx: &mut TestAppContext) {
    let app = setup(cx);
    cx.update(register_fake_scm);
    let mut settings = app.core.user_settings();
    settings.workbench.startup_view = oncode_core::settings::StartupView::Explorer;
    app.core.set_user_settings(settings).unwrap();

    let project = app.git_project(&[("a.txt", "a\n")]);
    let f = Fixture::open(&app, &project, cx).await;
    assert!(cx.update(|cx| f.workbench.read(cx).env().repo_root.is_some()));
    assert_eq!(cx.update(|cx| f.workbench.read(cx).active_sidebar().map(|id| id.to_string())), Some("explorer".to_string()));
}

#[gpui_kit::test]
async fn a_trusted_repository_starts_on_source_control_by_default(cx: &mut TestAppContext) {
    let app = setup(cx);
    cx.update(register_fake_scm);
    let project = app.git_project(&[("a.txt", "a\n")]);
    let f = Fixture::open(&app, &project, cx).await;
    assert_eq!(cx.update(|cx| f.workbench.read(cx).active_sidebar().map(|id| id.to_string())), Some("scm".to_string()));

    // Explorer can still be invoked and used through a command.
    f.focus_tree(cx);
    assert_eq!(cx.update(|cx| f.workbench.read(cx).active_sidebar().map(|id| id.to_string())), Some("explorer".to_string()));
}

// ----- Drag and drop -----

impl Fixture {
    /// Drags from the center of element `from` to a relative point (`fx`, `fy` in 0..1) in element `to`, then releases.
    fn drag(&self, cx: &mut TestAppContext, from: &str, to: &str, fx: f32, fy: f32) {
        let (from, to) = (from.to_string(), to.to_string());
        self.window.update(cx, |window, cx| {
            window.render_frame(cx);
            let start = window.find(from).bounds().center();
            let area = window.find(to).bounds();
            window.drag(start, point(area.origin.x + area.size.width * fx, area.origin.y + area.size.height * fy), cx);
        });
        cx.run_until_parked();
    }

    /// Drops files from the OS onto a relative point in element `to`, with the platform event sequence.
    fn drop_from_os(&self, cx: &mut TestAppContext, paths: &[&Path], to: &str, fx: f32, fy: f32) {
        let to = to.to_string();
        self.window.update(cx, |window, cx| {
            window.render_frame(cx);
            let area = window.find(to).bounds();
            let position = point(area.origin.x + area.size.width * fx, area.origin.y + area.size.height * fy);
            let external = ExternalPaths(paths.iter().map(|path| path.to_path_buf()).collect());
            window.dispatch_event(FileDropEvent::Entered { position, paths: external }.to_platform_input(), cx);
            window.render_frame(cx);
            window.dispatch_event(FileDropEvent::Pending { position }.to_platform_input(), cx);
            window.render_frame(cx);
            window.dispatch_event(FileDropEvent::Submit { position }.to_platform_input(), cx);
            window.render_frame(cx);
            window.dispatch_event(FileDropEvent::Ended.to_platform_input(), cx);
        });
        cx.run_until_parked();
    }

    /// Opens a file in a permanent tab and waits until it is selected in the tree.
    async fn open_and_reveal(&self, cx: &mut TestAppContext, path: &Path) {
        let text = text(path);
        self.window.update(cx, |window, cx| self.workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&text), window, cx)));
        self.wait_for_active_file(cx, path).await;
        let explorer = self.explorer.clone();
        self.window.wait_until(cx, "file selected in the tree", move |_, cx| explorer.read(cx).selected() == Some(text.as_str())).await;
    }

    /// The number of notifications (toasts) currently shown.
    fn notifications(&self, cx: &mut TestAppContext) -> usize {
        self.window.update(cx, |window, cx| window.notifications(cx).len())
    }

    /// Waits until exactly `count` notifications are shown (errors from backend work arrive later).
    async fn wait_for_notifications(&self, cx: &mut TestAppContext, count: usize) {
        for _ in 0..500 {
            cx.run_until_parked();
            if self.notifications(cx) == count {
                return;
            }
            cx.executor().timer(std::time::Duration::from_millis(20)).await;
        }
        panic!("waiting for {count} notifications, {} shown", self.notifications(cx));
    }

    fn is_expanded(&self, cx: &mut TestAppContext, path: &Path) -> bool {
        let path = text(path);
        cx.update(|cx| self.explorer.read(cx).is_expanded(&path))
    }

    /// The folder currently highlighted as the drop target.
    fn drop_target(&self, cx: &mut TestAppContext) -> Option<String> {
        cx.update(|cx| self.explorer.read(cx).drop_target().map(String::from))
    }

    /// The current tree rows, in the `shape` form.
    fn rows(&self, root: &Path, cx: &mut TestAppContext) -> Vec<String> {
        let explorer = self.explorer.clone();
        cx.update(|cx| shape(&explorer, root, cx))
    }

    /// The tab titles per editor group, in group order.
    fn editor_layout(&self, cx: &mut TestAppContext) -> Vec<Vec<String>> {
        cx.update(|cx| self.workbench.read(cx).model().groups.iter().map(|group| group.tabs.iter().map(|tab| tab.title.clone()).collect()).collect())
    }
}

#[gpui_kit::test]
async fn dragging_a_file_onto_a_folder_moves_it_and_its_open_tab_follows(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("a.txt", "one\n"), ("docs/readme.md", "# r\n"), ("src/main.rs", "fn main() {}\n")]);
    let f = Fixture::open(&app, &project, cx).await;

    // Unsaved contents move along to the new path, as with a rename.
    f.double_click(cx, "explorer-row-a.txt");
    f.wait_for_active_file(cx, &project.join("a.txt")).await;
    f.input(cx, "two ");
    assert!(cx.update(|cx| f.workbench.read(cx).has_dirty_tabs(cx)));

    f.drag(cx, "explorer-row-a.txt", "explorer-row-docs", 0.5, 0.5);
    let moved = project.join("docs/a.txt");
    f.wait_for_rows(cx, "file moved and target folder opened", &project, |rows| rows == ["docs/", "  docs/a.txt", "  docs/readme.md", "src/"]).await;
    assert!(moved.is_file() && !project.join("a.txt").exists(), "the file is moved on disk");
    assert_eq!(f.selected(cx), Some(text(&moved)), "the moved file is selected");
    assert_eq!(f.tab_ids(cx), [format!("file:{}", text(&moved))]);

    f.wait_for_active_file(cx, &moved).await;
    let (_, buffer, state) = cx.update(|cx| active_file(f.workbench.read(cx), cx).unwrap());
    assert_eq!(cx.update(|cx| state.read(cx).value().to_string()), "two one\n");
    assert!(cx.update(|cx| buffer.read(cx).is_dirty()));
    assert_eq!(std::fs::read_to_string(&moved).unwrap(), "one\n", "the file on disk has not changed yet");
    assert_eq!(f.drop_target(cx), None, "the highlight is gone after the drop");
}

#[gpui_kit::test]
async fn dropping_on_a_file_uses_its_folder_and_on_the_background_uses_the_project_root(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("src/a.ts", "a\n"), ("src/b.ts", "b\n"), ("lib/c.ts", "c\n"), ("top.txt", "t\n")]);
    let f = Fixture::open(&app, &project, cx).await;
    f.click(cx, "explorer-row-src");
    f.wait_for_rows(cx, "src opened", &project, |rows| rows.contains(&"  src/a.ts".to_string())).await;
    f.click(cx, "explorer-row-lib");
    f.wait_for_rows(cx, "lib opened", &project, |rows| rows == ["lib/", "  lib/c.ts", "src/", "  src/a.ts", "  src/b.ts", "top.txt"]).await;

    // On top of a file: goes into the folder that file is in.
    f.drag(cx, "explorer-row-src/a.ts", "explorer-row-lib/c.ts", 0.5, 0.5);
    f.wait_for_rows(cx, "a.ts moved to lib", &project, |rows| rows == ["lib/", "  lib/a.ts", "  lib/c.ts", "src/", "  src/b.ts", "top.txt"]).await;
    assert!(project.join("lib/a.ts").is_file() && !project.join("src/a.ts").exists());

    // On the empty background below the last row: the project root.
    f.drag(cx, "explorer-row-lib/a.ts", "explorer-tree", 0.5, 0.95);
    f.wait_for_rows(cx, "a.ts moved to the root", &project, |rows| rows == ["lib/", "  lib/c.ts", "src/", "  src/b.ts", "a.ts", "top.txt"]).await;
    assert!(project.join("a.ts").is_file() && !project.join("lib/a.ts").exists());

    // A file at the root dropped on another file at the root: its place does not change, so nothing happens.
    f.drag(cx, "explorer-row-top.txt", "explorer-row-a.ts", 0.5, 0.5);
    assert!(project.join("top.txt").is_file() && project.join("a.ts").is_file());
}

#[gpui_kit::test]
async fn a_folder_moves_with_its_contents_and_the_tabs_of_files_inside_it(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("src/deep/a.ts", "a\n"), ("src/b.ts", "b\n"), ("dest/keep.txt", "k\n")]);
    let f = Fixture::open(&app, &project, cx).await;
    f.open_and_reveal(cx, &project.join("src/deep/a.ts")).await;
    f.wait_for_rows(cx, "tree opened", &project, |rows| rows == ["dest/", "src/", "  src/deep/", "    src/deep/a.ts", "  src/b.ts"]).await;

    f.drag(cx, "explorer-row-src", "explorer-row-dest", 0.5, 0.5);
    f.wait_for_rows(cx, "folder moved with its contents", &project, |rows| {
        rows == ["dest/", "  dest/src/", "    dest/src/deep/", "      dest/src/deep/a.ts", "    dest/src/b.ts", "  dest/keep.txt"]
    })
    .await;
    assert!(project.join("dest/src/deep/a.ts").is_file() && !project.join("src").exists());
    assert_eq!(f.selected(cx), Some(text(&project.join("dest/src"))));
    assert_eq!(f.tab_ids(cx), [format!("file:{}", text(&project.join("dest/src/deep/a.ts")))], "file tabs inside the folder are rewritten");
    // A folder that was already open stays open at its new location.
    assert!(f.is_expanded(cx, &project.join("dest/src/deep")));
}

#[gpui_kit::test]
async fn drops_that_would_change_nothing_or_overwrite_something_are_ignored(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("src/a.ts", "in src\n"), ("src/inner/b.ts", "b\n"), ("other/a.ts", "in other\n")]);
    let f = Fixture::open(&app, &project, cx).await;
    f.click(cx, "explorer-row-src");
    f.wait_for_rows(cx, "src opened", &project, |rows| rows.contains(&"  src/a.ts".to_string())).await;
    f.click(cx, "explorer-row-src/inner");
    let initial = ["other/", "src/", "  src/inner/", "    src/inner/b.ts", "  src/a.ts"];
    f.wait_for_rows(cx, "inner opened", &project, |rows| rows == initial).await;
    let assert_unchanged = |cx: &mut TestAppContext| {
        assert_eq!(f.rows(&project, cx), initial);
        assert_eq!(std::fs::read_to_string(project.join("src/a.ts")).unwrap(), "in src\n");
        assert_eq!(std::fs::read_to_string(project.join("other/a.ts")).unwrap(), "in other\n");
        assert!(project.join("src/inner/b.ts").is_file());
    };

    // Into itself, into its contents, and into the folder it is already in. Dragging onto the same row
    // must move a few pixels (fy 0.9), otherwise it is just a click that collapses the folder.
    f.drag(cx, "explorer-row-src", "explorer-row-src", 0.5, 0.9);
    f.drag(cx, "explorer-row-src", "explorer-row-src/inner", 0.5, 0.5);
    f.drag(cx, "explorer-row-src", "explorer-row-src/inner/b.ts", 0.5, 0.5);
    f.drag(cx, "explorer-row-src/a.ts", "explorer-row-src", 0.5, 0.5);
    f.drag(cx, "explorer-row-src/a.ts", "explorer-row-src/a.ts", 0.5, 0.9);
    assert_unchanged(cx);
    assert_eq!(f.notifications(cx), 0, "nothing was attempted, so nothing failed");

    // A name that already exists at the destination: rejected with a message, no overwrite.
    f.drag(cx, "explorer-row-src/a.ts", "explorer-row-other", 0.5, 0.5);
    f.wait_for_notifications(cx, 1).await;
    assert!(!project.join("other/src").exists());
    assert_unchanged(cx);
}

#[gpui_kit::test]
async fn hovering_while_dragging_highlights_the_destination_and_opens_a_collapsed_folder(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("a.txt", "a\n"), ("docs/readme.md", "# r\n"), ("docs/guide/intro.md", "i\n")]);
    let f = Fixture::open(&app, &project, cx).await;
    assert_eq!(f.rows(&project, cx), ["docs/", "a.txt"]);

    let center = |cx: &mut TestAppContext, id: &str| {
        let id = id.to_string();
        f.window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.find(id).bounds().center()
        })
    };
    let (a_txt, docs) = (center(cx, "explorer-row-a.txt"), center(cx, "explorer-row-docs"));
    f.window.update(cx, |window, cx| {
        window.pointer_move(a_txt, None, cx);
        window.pointer_down(a_txt, MouseButton::Left, cx);
        window.pointer_move(point(a_txt.x + gpui_kit::px(8.), a_txt.y), Some(MouseButton::Left), cx);
        window.pointer_move(docs, Some(MouseButton::Left), cx);
    });
    cx.run_until_parked();
    assert_eq!(f.drop_target(cx), Some(text(&project.join("docs"))), "the folder under the pointer is highlighted");
    assert!(!f.is_expanded(cx, &project.join("docs")));

    // Resting briefly over a closed folder opens it, without releasing.
    let (explorer, docs_path) = (f.explorer.clone(), text(&project.join("docs")));
    f.window.wait_until(cx, "folder opened by itself", move |_, cx| explorer.read(cx).is_expanded(&docs_path)).await;
    f.wait_for_rows(cx, "folder contents shown", &project, |rows| rows == ["docs/", "  docs/guide/", "  docs/readme.md", "a.txt"]).await;

    // The file's own source folder is not a meaningful target: not highlighted.
    let a_txt = center(cx, "explorer-row-a.txt");
    f.window.update(cx, |window, cx| window.pointer_move(a_txt, Some(MouseButton::Left), cx));
    cx.run_until_parked();
    assert_eq!(f.drop_target(cx), None);

    // Over a file inside a folder: the folder that file is in is highlighted.
    let readme = center(cx, "explorer-row-docs/readme.md");
    f.window.update(cx, |window, cx| window.pointer_move(readme, Some(MouseButton::Left), cx));
    cx.run_until_parked();
    assert_eq!(f.drop_target(cx), Some(text(&project.join("docs"))));

    // Released outside the tree (on a toolbar button): nothing changes, and the highlight disappears.
    let toolbar = center(cx, "explorer-new-file");
    f.window.update(cx, |window, cx| {
        window.pointer_move(toolbar, Some(MouseButton::Left), cx);
        window.pointer_up(toolbar, MouseButton::Left, cx);
    });
    cx.run_until_parked();
    assert_eq!(f.drop_target(cx), None);
    assert!(project.join("a.txt").is_file() && !project.join("docs/a.txt").exists());
}

#[gpui_kit::test]
async fn dragging_a_file_to_the_editor_opens_it_and_edges_split_the_editor(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("a.txt", "a\n"), ("b.txt", "b\n"), ("src/c.txt", "c\n")]);
    let f = Fixture::open(&app, &project, cx).await;

    // Empty editor area: a file from the Explorer opens as a permanent tab.
    f.drag(cx, "explorer-row-a.txt", "editor-empty", 0.5, 0.5);
    assert_eq!(f.editor_layout(cx), [["a.txt"]]);
    assert!(cx.update(|cx| f.workbench.read(cx).model().all_tabs().all(|tab| !tab.preview)), "not a preview tab");

    // The right edge of the group contents: a new group on the right.
    f.drag(cx, "explorer-row-b.txt", "group-content-1", 0.95, 0.5);
    assert_eq!(f.editor_layout(cx), [vec!["a.txt"], vec!["b.txt"]]);
    f.wait_for_active_file(cx, &project.join("b.txt")).await;

    // A folder cannot be dropped onto the editor.
    f.drag(cx, "explorer-row-src", "group-content-1", 0.5, 0.5);
    assert_eq!(f.editor_layout(cx), [vec!["a.txt"], vec!["b.txt"]]);
}

#[gpui_kit::test]
async fn files_dropped_from_the_os_are_copied_into_the_folder_under_the_pointer(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("a.txt", "a\n"), ("docs/readme.md", "# r\n")]);
    let outside = app.project(&[("logo.png", "png"), ("assets/site.css", "css")]);
    let f = Fixture::open(&app, &project, cx).await;

    // On top of a folder: copied into it, the folder is opened, and the result is selected.
    f.drop_from_os(cx, &[&outside.join("logo.png")], "explorer-row-docs", 0.5, 0.5);
    f.wait_for_rows(cx, "copy shown in docs", &project, |rows| rows == ["docs/", "  docs/logo.png", "  docs/readme.md", "a.txt"]).await;
    assert_eq!(std::fs::read(project.join("docs/logo.png")).unwrap(), b"png");
    assert_eq!(f.selected(cx), Some(text(&project.join("docs/logo.png"))));
    assert!(outside.join("logo.png").is_file(), "the source is untouched");

    // A folder from the OS, dropped on a file at the root: copied to the root with its contents.
    f.drop_from_os(cx, &[&outside.join("assets")], "explorer-row-a.txt", 0.5, 0.5);
    f.wait_for_rows(cx, "folder copied to the root", &project, |rows| rows.contains(&"assets/".to_string())).await;
    assert_eq!(std::fs::read(project.join("assets/site.css")).unwrap(), b"css");

    // On the empty background: the project root.
    f.drop_from_os(cx, &[&outside.join("logo.png")], "explorer-tree", 0.5, 0.95);
    f.wait_for_rows(cx, "copy at the root shown", &project, |rows| rows.contains(&"logo.png".to_string())).await;
    assert_eq!(std::fs::read(project.join("logo.png")).unwrap(), b"png");

    // A name that already exists: rejected with a message, no overwrite.
    std::fs::write(project.join("docs/logo.png"), "changed").unwrap();
    f.drop_from_os(cx, &[&outside.join("logo.png")], "explorer-row-docs", 0.5, 0.5);
    f.wait_for_notifications(cx, 1).await;
    assert_eq!(std::fs::read(project.join("docs/logo.png")).unwrap(), b"changed");
}

#[gpui_kit::test]
async fn files_can_be_dropped_into_an_empty_project(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[]);
    let outside = app.project(&[("hello.txt", "hello\n")]);
    let window = app.open(Some(&project), cx).await;
    let f = Fixture::current(window, cx);
    let explorer = f.explorer.clone();
    f.window.wait_until(cx, "empty tree loaded", move |_, cx| !explorer.read(cx).is_loading()).await;
    assert!(cx.update(|cx| f.explorer.read(cx).rows().iter().all(|row| row.entry().is_none())), "only the 'empty' note, no entries");

    // The list is not drawn for an empty folder, but the tree still accepts a drop as the root.
    f.drop_from_os(cx, &[&outside.join("hello.txt")], "explorer-tree", 0.5, 0.5);
    f.wait_for_rows(cx, "copy shown", &project, |rows| rows == ["hello.txt"]).await;
    assert_eq!(std::fs::read_to_string(project.join("hello.txt")).unwrap(), "hello\n");
}
