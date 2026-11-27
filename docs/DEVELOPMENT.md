# Development Guide

OnCode is a native application: Rust + [GPUI](https://github.com/zed-industries/zed) via [`gpui-kit`](https://gpui-kit.com) (the `gpui-component` components). Its design is in [DESIGN.md](DESIGN.md). The Tauri version (v0.2) is kept under the git tag `tauri-v0.2`; its design is in [archive/DESIGN-v0.2-tauri.md](archive/DESIGN-v0.2-tauri.md).

## Prerequisites

| Requirement | Notes |
|---|---|
| Rust (stable, ≥ 1.88) | `rustup` |
| Git ≥ 2.40 | Used by the app at runtime and by the tests |
| C compiler | For the tree-sitter grammars |
| GPUI system libraries | See below |

**Linux (Debian/Ubuntu):**

```bash
sudo apt install build-essential pkg-config libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev \
  libxcb1-dev libx11-dev libfontconfig-dev libfreetype-dev libssl-dev libvulkan1
```

A Vulkan driver is required at runtime. Without a GPU, Mesa lavapipe (`mesa-vulkan-drivers`) can be used: `VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.json`.

**Windows:** Microsoft C++ Build Tools. **macOS:** Xcode with the Command Line Tools.

### Without root access (`.dev-sysroot`)

If the packages above cannot be installed, the `-dev` packages and the C compiler can be downloaded with `apt-get download` and extracted into `.dev-sysroot/` inside the project (not committed). Activate it before running `cargo`:

```bash
source .dev-sysroot/env.sh
```

Libraries that exist only in the sysroot (e.g. `libxkbcommon-x11.so.0`) must be findable when the app runs: `LD_LIBRARY_PATH=.dev-sysroot/runlib`.

## Everyday commands

| Goal | Command |
|---|---|
| Run | `cargo build -p oncode-helper && cargo run -p oncode -- [folder]` |
| Test one crate | `cargo test -p oncode-ui-scm` |
| All tests | `cargo test --workspace` |
| Quick check | `cargo check -p oncode` |
| `.deb` package | `cargo install cargo-packager --locked`, then `cargo packager --release -f deb` in the `app/` folder (output in `target/packages/`) |
| AppImage | `cargo packager --release -f appimage` in `app/`. On a machine without `libxkbcommon-x11` installed, `linuxdeploy` needs an `LD_LIBRARY_PATH` that includes that library |
| NSIS installer from Linux | Requires `makensis` (package `nsis`). `cargo packager --release -f nsis --target x86_64-pc-windows-gnu` in `app/`, with `CARGO_TARGET_DIR` pointing at the Windows build folder: the tool ignores `--binaries-dir`. The Windows binary must be built with `--config "$ONCODE_WIN_PATCH"` (see `.dev-sysroot/env-win.sh`) |
| DMG from Linux (unofficial) | See the comments in `.dev-sysroot/env-mac.sh`: zig + macOS SDK, a copy of `gpui-pre-apple` with shaders at app start, linking with `rust-lld`, then `.dev-sysroot/mac/make-dmg.sh`. Official releases are still built on macOS (`release.yml`) |
| `oncode` command | The NSIS installer runs `oncode-helper install-shell-command` in `preinstall-section` (`app/Cargo.toml`) before the files are copied, and `uninstall-shell-command` on uninstall. The `.deb` package ships `app/packaging/linux/oncode` as `/usr/bin/oncode`; that file must stay executable in git. Debug builds use their own single-process endpoint (`app/src/single_instance.rs`), so they do not collide with an installed OnCode |
| Release | Push a `v*` tag; `.github/workflows/release.yml` builds deb/AppImage, NSIS, and DMG (Apple Silicon and Intel, signed ad-hoc), then uploads them as a draft release. To try the installers first: Actions → Release → **Run workflow**; they become artifacts of that run |

`oncode-helper` must sit next to the `oncode-app` binary (in `target/debug/`); without it the credential dialog and interactive rebase are unavailable.

The main binary is deliberately not named `oncode`: on Windows with an Intel GPU, a 6-letter exe name makes the driver load the shader cache of Chrome/Edge into the app's memory (± 150 MB). The explanation is in `app/Cargo.toml`.

The LSP tests use a mock language server (`crates/oncode-lsp/src/mock.rs`, program `mock_lsp_server`); the tests build it themselves if it does not exist yet. To try it in the app, point `lsp.servers.<language>.command` in `settings.json` to `target/debug/examples/mock_lsp_server` (the words `ERROR`, `WARN`, `INFO`, `HINT` produce diagnostics; `def name` creates a definition).

## Crate map

| Crate | Contents |
|---|---|
| `app` (binary `oncode-app`) | `main`: tokio runtime, `Core`, then `init` of each UI crate |
| `crates/oncode-core` | UI-free application state and the entire API (`oncode_core::api::{app, fs, git, term, lsp, forge}`) |
| `crates/oncode-git`, `-workspace`, `-terminal`, `-lsp`, `-forge`, `-helper` | Backends; do not depend on the UI |
| `crates/oncode-ui` | UI framework: `Workbench`, `Shell`, commands, keybindings, quick pick, dialogs, editor and buffers, repository store |
| `crates/oncode-ui-<feature>` | One feature per crate: `explorer`, `search`, `diff`, `scm`, `graph`, `terminal`, `lsp`, `forge`, `settings` |

Feature crates depend only on `oncode-ui`, `oncode-core`, and the backend crates they need (exception: `scm` and `graph` use `oncode-ui-diff` to open diff tabs).

## Key rules

### 1. Don't invent GPUI APIs

GPUI and `gpui-component` are still pre-1.0. Before using a method, **look up its signature in the source** (`~/.cargo/registry/src/*/gpui-component-0.10.0`, `gpui-base-0.10.0`, `gpui-pre-0.3.12`). Per-component documentation: `https://gpui-kit.com/component/<name>.md` (e.g. `editor.md`, `diff.md`, `tree.md`, `list.md`, `virtual-list.md`, `menu.md`, `dialog.md`). Always import through `gpui_kit::...`, not `gpui::...`.

### 2. Entity rules (double borrow = panic)

A GPUI entity that is being `update`d must not be read or `update`d again from inside that update. Consequences:

- **Builders called by `Workbench`** (`SidebarView::build`, `PanelView::build`, `StatusItem::build`, `TabKind::build`, `TabContent::can_close`, the `register_feature` functions) run while `Workbench` is borrowed. Inside them, **do not** call `workbench.read(cx)` / `workbench.update(cx, ..)`. Use data from `Env`, or defer with `cx.defer_in(window, ..)` / `window.defer(cx, ..)`.
- **`render` of a feature view** runs outside the `Workbench` borrow, so `workbench.read(cx)` is fine.
- **Command handlers** (`Command::new(.., |args, window, cx| ..)`) run outside any borrow and may update any entity.
- From inside `workbench.update(..)`, run other commands with `oncode_ui::execute(id, args, window, cx)` (deferred), not `execute_now`.
- Event handlers (`cx.subscribe`, `cx.observe`) run after the update has finished, so they are safe.

### 3. Backend work does not run on the main thread

The backend crates use tokio; GPUI has its own executor. All `oncode_core::api::*` calls are run through `oncode_ui::spawn_core`:

```rust
let (core, root) = (env.core.clone(), root.clone());
let task = spawn_core(cx, async move { api::git::git_stage(&core, root, paths).await });
cx.spawn_in(window, async move |this, cx| {
    let result = task.await;
    let _ = this.update_in(cx, |this, window, cx| {
        if let Err(error) = result {
            oncode_ui::overlay::show_error(&error, window, cx);
        }
    });
})
.detach();
```

Do not call blocking I/O (files, processes, network) directly from `render` or event handlers.

### 4. Language

All UI text, user-facing messages, code comments, and documents are written in **English**. Command ids are the same as in v0.2 so that users' `keybindings.json` keeps working.

## Adding a feature

Each feature crate exports `pub fn init(cx: &mut App)`, which is called from `app/src/main.rs`.

```rust
pub fn init(cx: &mut App) {
    // Tab kind (global).
    oncode_ui::register_tab_kind(cx, "graph", GraphKind);
    // Per window: called for every new `Workbench` (also when a folder is opened or trusted).
    oncode_ui::register_feature(cx, |workbench, window, cx| {
        let env = workbench.env().clone();          // core, window label, root, trusted, repo_root
        if !env.trusted { return; }                 // restricted mode: Git/LSP features are not registered
        workbench.register_sidebar(SidebarView::new("scm", "Source Control", Lucide::GitBranch, 20, |env, window, cx| {
            cx.new(|cx| ScmPanel::new(env.clone(), window, cx)).into()
        }).badge(|cx| /* Option<usize> */ None));
        workbench.register_panel(PanelView::new("git-output", "Git Output", 30, |env, window, cx| ..));
        workbench.register_status(StatusItem::new("git.branch", StatusAlign::Left, 10, |env, window, cx| ..), window, cx);
        workbench.register_command(Command::new("git.commit", "Commit", |_, window, cx| ..).category("Git").key("Mod+Enter").when(|_, cx| ..));
    });
}
```

- **Sidebar, panel, status bar:** a view is its own entity (`AnyView`) that observes the state it needs. Ordering: a smaller `order` comes first. Explorer 10, Search 20, Source Control 30, Graph 40, Pull Requests 50; panels: Terminal 10, Problems 20, Git Output 30.
- **Tabs:** `workbench.open_tab(TabData::new(kind, title, json!({..})), OpenOptions::default(), window, cx)`. A tab's content is built by `TabKind::build` and implements `TabContent` (use `ViewTab(entity)` for simple views). Tabs can be dragged (`DraggedTab`); other features may drag a `DraggedTab { from: None, .. }` onto the editor area to open it as a tab (used by the terminal). A tab kind whose content must not appear twice overrides `TabKind::split_copy` (terminal: "Split Editor" opens a new terminal). To change one tab's data in place use `workbench.update_tab(group, id, ..)`; a kind whose content depends on that data overrides `TabKind::rebuild` (a file tab switching between an image's picture and its text). Reopening an open tab keeps the parameters the opener does not mention.
- **Per-window state owned by a feature:** `workbench.set_extension(entity)` and `workbench.extension::<Entity<T>>()`.
- **Files:** `oncode_ui::editor::open_file(workbench, OpenFile::new(path).at(line, column, len), window, cx)`; buffers via `oncode_ui::editor::buffers(workbench)`; the active file via `active_file(workbench, cx)`. An image file's tab may show a picture instead of the editor (`active_image`); features that work on the editor itself (language servers, blame, editing commands) use `active_text_file`, which skips pictures. `Buffer::save` asks before writing over a file that changed on disk; `Buffer::overwrite` is the "Overwrite" choice.
- **Rebuilding the workbench:** work that changes what the workbench is built from (the folder's trust, a new repository) goes through `Shell::rebuild_workspace(|core| async { .. }, window, cx)`, which asks about unsaved changes first and brings the tabs back through the session.
- **Repositories:** `oncode_ui::repos(cx)` (a snapshot per root, file markers); subscribe to `ReposEvent`.
- **Events:** `WorkbenchEvent` (tabs, layout, settings, `FsChanged`, `FileSaved`) via `cx.subscribe_in(&workbench, window, ..)`; other backend events via `oncode_ui::core_bus(cx)` (`CoreEvent`).
- **Dialogs and toasts:** `oncode_ui::overlay::{confirm, choose, prompt_text, toast_*, show_error}`; choosing from a list: `oncode_ui::quick_pick::quick_pick`. Destructive actions always go through `confirm(..danger())`, which says what will be lost.
- **Icons:** `Icon::new(oncode_ui::Lucide::GitBranch)` (all of Lucide is available).

## Testing

Three levels, all `cargo test`:

1. **Pure logic** (`#[test]`): translating choices, parsers, models. Separate logic from the view so it can be tested without a window.
2. **Backend** (`#[tokio::test]` in the backend crates and `oncode-core`): against real git in a temporary repository.
3. **View** (`#[gpui_kit::test]`): a headless window, simulated input, a real backend. See `crates/oncode-ui/tests/editor.rs`.

```rust
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use oncode_ui::testing::{git, TestApp};

#[gpui_kit::test]
async fn stages_a_file(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(|cx| oncode_ui_scm::init(cx));                 // register the feature under test before opening the window
    let project = app.git_project(&[("a.txt", "one\n")]);    // temporary git repo, already trusted
    std::fs::write(project.join("a.txt"), "two\n").unwrap();
    let window = app.open(Some(&project), cx).await;
    window.wait_until(cx, "git status", |_, cx| ..).await;   // waits for the backend (a real tokio thread)
    window.update(cx, |window, cx| { window.render_frame(cx); window.click("scm-stage-a.txt", cx); });
    window.wait_until(cx, "staged", |_, cx| ..).await;
    assert_eq!(git(&project, &["diff", "--cached", "--name-only"]).trim(), "a.txt");
}
```

Notes on view tests:

- Don't `use gpui_kit::*;` in test modules (it shadows `#[test]`). An element that gets clicked needs `.id("...")`; a custom element that `window.find(..)` should locate needs `.test_support()` (`gpui_kit::test::TestSupportExt`).
- `window.input("text", cx)` types into the focused element; `window.press("enter", cx)` presses a named key; `window.run_command(cx, "id")` runs a command.
- Don't wait with `cx.executor().timer(..)` outside `wait_until`; GPUI test time does not advance on its own.
- Check the final result outside the app (file contents, `git`), not just view state.

### Seeing the UI headlessly

Debug builds accept `ONCODE_DEV_SCRIPT` (see `app/src/dev.rs`): a JSON list of steps (`wait`, `open`, `command`, `type`, `key`) that runs after the window opens. With `Xvfb` and lavapipe, the result can be captured with `xwd`. Use a temporary `XDG_CONFIG_HOME`/`XDG_DATA_HOME` so the user's settings are not touched.
