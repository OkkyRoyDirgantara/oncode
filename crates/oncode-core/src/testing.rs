//! Shared unit test tooling: a core with temporary directories, an event collector, and git
//! isolated from the user's configuration (the same approach as the `oncode-git` tests).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Once};
use std::time::Duration;

use oncode_forge::MemoryStore;
use tempfile::TempDir;
use tokio::runtime::Handle;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use crate::dirs::Dirs;
use crate::events::CoreEvent;
use crate::state::{Core, Shared};
use crate::tokens::FallbackStore;

static ENV: Once = Once::new();

/// Isolates git from the global/system configuration and sets the test identity.
pub fn isolate_git() {
    ENV.call_once(|| {
        let vars = [
            // The platform's empty file: `NUL` on Windows.
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

/// Runs git directly (outside the layer under test) and returns stdout.
pub fn git(cwd: &Path, args: &[&str]) -> String {
    isolate_git();
    let out = Command::new("git").current_dir(cwd).args(args).output().unwrap();
    assert!(out.status.success(), "git {:?} failed: {}{}", args, String::from_utf8_lossy(&out.stderr), String::from_utf8_lossy(&out.stdout));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The `oncode-helper` binary built by `cargo build -p oncode-helper` (in `target/<profile>/`), searched for
/// from the test binary's folder upward: the test binary is in `target/<profile>/deps/` or, with
/// Cargo's new build-dir layout, in `target/<profile>/build/<crate>/<hash>/out/`.
pub fn built_helper() -> Option<PathBuf> {
    let name = if cfg!(windows) { "oncode-helper.exe" } else { "oncode-helper" };
    std::env::current_exe().ok()?.ancestors().skip(1).map(|dir| dir.join(name)).find(|path| path.is_file())
}

/// A core on top of the test runtime, with temporary config/data directories and in-memory tokens.
pub struct TestCore {
    pub core: Shared,
    pub events: UnboundedReceiver<CoreEvent>,
    pub dir: TempDir,
}

impl TestCore {
    pub fn new() -> Self {
        isolate_git();
        let dir = tempfile::tempdir().unwrap();
        let (tx, events) = unbounded_channel();
        let tokens = Arc::new(FallbackStore::new(Arc::new(MemoryStore::new())));
        let core = Core::new(
            Handle::current(),
            Dirs::under(dir.path().join("app")),
            tokens,
            Arc::new(move |event| {
                let _ = tx.send(event);
            }),
        );
        Self { core, events, dir }
    }

    /// An empty project folder inside the temporary directory (canonical path, like the result of `workspace_open`).
    pub fn project(&self, name: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        canonical(&path)
    }

    /// A repository with one commit containing `README.md`, already marked as trusted.
    pub fn trusted_repo(&self, name: &str) -> PathBuf {
        let path = self.project(name);
        git(&path, &["init", "-q", "-b", "main"]);
        std::fs::write(path.join("README.md"), "# Project\n").unwrap();
        git(&path, &["add", "-A"]);
        git(&path, &["commit", "-q", "-m", "First commit"]);
        self.core.store.set_trusted(&path.to_string_lossy(), true);
        path
    }
}

/// Waits for the first event that matches `pick`; other events are skipped. Panics after 20 seconds.
pub async fn recv<T>(events: &mut UnboundedReceiver<CoreEvent>, mut pick: impl FnMut(CoreEvent) -> Option<T>) -> T {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let event = events.recv().await.expect("event collector closed");
            if let Some(value) = pick(event) {
                return value;
            }
        }
    })
    .await
    .expect("the awaited event never arrived")
}

/// Canonical path as used by `workspace_open`: on Windows without the `\\?\` prefix.
fn canonical(path: &std::path::Path) -> PathBuf {
    let path = std::fs::canonicalize(path).unwrap();
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => path,
    }
}
