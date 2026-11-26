//! View test helpers (the `test-support` feature): a test app with a real `oncode-core`
//! in a temporary directory, and a headless window containing the `Shell`.
//!
//! ```ignore
//! #[gpui_kit::test]
//! async fn example(cx: &mut TestAppContext) {
//!     let app = TestApp::new(cx);
//!     let project = app.project(&[("a.txt", "content")]);
//!     let window = app.open(Some(project.path()), cx).await;
//!     ...
//! }
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{px, size, AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, WindowBounds, WindowOptions};
use oncode_core::{Core, Dirs, FallbackStore, Shared};
use tempfile::TempDir;

use crate::{OnCode, Shell, Workbench};

/// An in-memory token store; tests do not touch the keychain.
fn memory_tokens() -> Arc<FallbackStore> {
    Arc::new(FallbackStore::new(Arc::new(oncode_forge::MemoryStore::default())))
}

static GIT_ENV: std::sync::Once = std::sync::Once::new();

/// Isolates git from the global/system configuration and sets the test identity (once per process).
pub fn isolate_git() {
    GIT_ENV.call_once(|| {
        let vars = [
            ("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" }),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_AUTHOR_NAME", "Tester"),
            ("GIT_AUTHOR_EMAIL", "tester@example.com"),
            ("GIT_COMMITTER_NAME", "Tester"),
            ("GIT_COMMITTER_EMAIL", "tester@example.com"),
            ("GIT_CONFIG_COUNT", "2"),
            ("GIT_CONFIG_KEY_0", "protocol.file.allow"),
            ("GIT_CONFIG_VALUE_0", "always"),
            ("GIT_CONFIG_KEY_1", "commit.gpgsign"),
            ("GIT_CONFIG_VALUE_1", "false"),
        ];
        for (key, value) in vars {
            std::env::set_var(key, value);
        }
    });
}

/// Runs git directly (outside the layer under test) and returns stdout; panics on failure.
pub fn git(cwd: &Path, args: &[&str]) -> String {
    isolate_git();
    let out = std::process::Command::new("git").current_dir(cwd).args(args).output().expect("git is available");
    assert!(out.status.success(), "git {args:?} failed: {}{}", String::from_utf8_lossy(&out.stderr), String::from_utf8_lossy(&out.stdout));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub struct TestApp {
    pub core: Shared,
    pub runtime: Arc<tokio::runtime::Runtime>,
    /// Temporary config and data directory; removed when the test finishes.
    pub dirs: TempDir,
    projects: std::cell::RefCell<Vec<TempDir>>,
}

pub struct TestWindow {
    pub handle: AnyWindowHandle,
    pub shell: Entity<Shell>,
}

impl TestApp {
    /// Sets up GPUI, the components, `oncode-core`, and the base features. Other features are registered
    /// by the caller via `cx.update(|cx| feature_crate::init(cx))` before opening a window.
    pub fn new(cx: &mut TestAppContext) -> Self {
        isolate_git();
        // `oncode-helper` (credentials and interactive rebase) lives in `target/<profile>/`, searched
        // from the test binary's folder upwards: the test binary is in `deps/` or, with Cargo's new
        // build-dir layout, in `build/<crate>/<hash>/out/`.
        if std::env::var_os("ONCODE_HELPER").is_none() {
            let name = if cfg!(windows) { "oncode-helper.exe" } else { "oncode-helper" };
            let helper = std::env::current_exe().ok().and_then(|exe| exe.ancestors().skip(1).map(|dir| dir.join(name)).find(|path| path.is_file()));
            if let Some(helper) = helper {
                std::env::set_var("ONCODE_HELPER", helper);
            }
        }
        let runtime = Arc::new(tokio::runtime::Builder::new_multi_thread().enable_all().worker_threads(2).build().expect("tokio runtime"));
        let dirs = tempfile::tempdir().expect("temporary directory");
        let (events_tx, events_rx) = tokio::sync::mpsc::unbounded_channel();
        let core = Core::new(
            runtime.handle().clone(),
            Dirs::under(dirs.path()),
            memory_tokens(),
            Arc::new(move |event| {
                let _ = events_tx.send(event);
            }),
        );
        // Backend work runs on real tokio threads, so the test executor must be willing to wait.
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::init(OnCode { core: core.clone(), runtime: runtime.clone(), events: Some(events_rx) }, cx);
        });
        Self { core, runtime, dirs, projects: Default::default() }
    }

    /// A temporary project folder containing these files (relative path, content).
    pub fn project(&self, files: &[(&str, &str)]) -> PathBuf {
        let dir = tempfile::tempdir().expect("project directory");
        for (rel, content) in files {
            let full = dir.path().join(rel);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(full, content).unwrap();
        }
        let path = canonical(dir.path());
        self.projects.borrow_mut().push(dir);
        path
    }

    /// A project folder that is a git repository (branch `main`) with one commit containing these
    /// files, already trusted. Git is isolated from the user's configuration.
    pub fn git_project(&self, files: &[(&str, &str)]) -> PathBuf {
        let path = self.project(files);
        git(&path, &["init", "-q", "-b", "main"]);
        // The identity in the repository config, like a user who has already set up git.
        git(&path, &["config", "user.name", "Tester"]);
        git(&path, &["config", "user.email", "tester@example.com"]);
        git(&path, &["add", "-A"]);
        git(&path, &["commit", "-q", "--allow-empty", "-m", "First commit"]);
        self.trust(&path);
        path
    }

    /// Revokes trust in a folder (restricted mode).
    pub fn trust_revoke(&self, path: &Path) {
        self.core.store.set_trusted(&path.to_string_lossy(), false);
    }

    /// Marks a folder as trusted (as if the user had approved it).
    pub fn trust(&self, path: &Path) {
        self.core.store.set_trusted(&path.to_string_lossy(), true);
    }

    /// Opens a headless window; if `folder` is given, waits until the folder is open.
    pub async fn open(&self, folder: Option<&Path>, cx: &mut TestAppContext) -> TestWindow {
        let folder_buf = folder.map(Path::to_path_buf);
        let (handle, shell) = cx.update(|cx| {
            gpui_kit::open_window(
                WindowOptions { window_bounds: Some(WindowBounds::Windowed(Bounds::new(Default::default(), size(px(1200.), px(800.))))), ..Default::default() },
                cx,
                |window, cx| cx.new(|cx| Shell::new(folder_buf, window, cx)),
            )
            .expect("test window")
        });
        let window = TestWindow { handle, shell };
        // The window is active like in the real app: GPUI only calls blur listeners
        // (`on_blur`) and some other behaviors when the window is active.
        window.update(cx, |window, _| window.activate_window());
        cx.run_until_parked();
        if folder.is_some() {
            window.wait_until(cx, "folder open", |shell, cx| shell.read(cx).workspace().is_some()).await;
        }
        window
    }
}

impl TestWindow {
    pub fn workbench(&self, cx: &mut TestAppContext) -> Entity<Workbench> {
        cx.update(|cx| self.shell.read(cx).workbench().clone())
    }

    /// Waits until `condition` is true (at most 10 seconds), running pending tasks meanwhile.
    pub async fn wait_until(&self, cx: &mut TestAppContext, what: &str, mut condition: impl FnMut(&Entity<Shell>, &gpui_kit::App) -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            if cx.update(|cx| condition(&self.shell, cx)) {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "waited too long: {what}");
            // Give the backend (tokio) threads time to finish their work.
            cx.executor().timer(Duration::from_millis(20)).await;
        }
    }

    /// Runs `f` with this window (for `find`, `click`, `input`, `press`).
    pub fn update<R>(&self, cx: &mut TestAppContext, f: impl FnOnce(&mut gpui_kit::Window, &mut gpui_kit::App) -> R) -> R {
        cx.update_window(self.handle, |_, window, cx| f(window, cx)).expect("window still exists")
    }

    /// Runs a command as if from the command palette.
    pub fn run_command(&self, cx: &mut TestAppContext, id: &str) -> bool {
        self.update(cx, |window, cx| crate::execute_now(id, &serde_json::Value::Null, window, cx))
    }
}

/// The canonical path as used by `workspace_open`: on Windows without the `\\?\` prefix, which Git
/// (e.g. `git http-backend`) and many other programs do not understand.
pub fn canonical(path: &std::path::Path) -> PathBuf {
    let path = std::fs::canonicalize(path).unwrap();
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => path,
    }
}
