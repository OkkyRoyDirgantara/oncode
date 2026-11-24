//! Perkakas bersama uji unit: core dengan direktori sementara, penampung event, dan git yang
//! terisolasi dari konfigurasi pengguna (pendekatan yang sama dengan uji `oncode-git`).

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

/// Mengisolasi git dari konfigurasi global/sistem dan menetapkan identitas uji.
pub fn isolate_git() {
    ENV.call_once(|| {
        let vars = [
            // Berkas kosong milik platform: `NUL` di Windows.
            ("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" }),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_AUTHOR_NAME", "Penguji"),
            ("GIT_AUTHOR_EMAIL", "penguji@contoh.id"),
            ("GIT_COMMITTER_NAME", "Penguji"),
            ("GIT_COMMITTER_EMAIL", "penguji@contoh.id"),
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

/// Menjalankan git langsung (di luar lapisan yang diuji) dan mengembalikan stdout.
pub fn git(cwd: &Path, args: &[&str]) -> String {
    isolate_git();
    let out = Command::new("git").current_dir(cwd).args(args).output().unwrap();
    assert!(out.status.success(), "git {:?} gagal: {}{}", args, String::from_utf8_lossy(&out.stderr), String::from_utf8_lossy(&out.stdout));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Binary `oncode-helper` hasil `cargo build -p oncode-helper`: satu folder di atas `deps/` tempat binary uji berada.
pub fn built_helper() -> Option<PathBuf> {
    let name = if cfg!(windows) { "oncode-helper.exe" } else { "oncode-helper" };
    let path = std::env::current_exe().ok()?.parent()?.parent()?.join(name);
    path.is_file().then_some(path)
}

/// Core di atas runtime uji, dengan direktori konfigurasi/data sementara dan token di memori.
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

    /// Folder proyek kosong di dalam direktori sementara (path kanonik, seperti hasil `workspace_open`).
    pub fn project(&self, name: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        canonical(&path)
    }

    /// Repositori dengan satu commit berisi `README.md`, sudah ditandai dipercaya.
    pub fn trusted_repo(&self, name: &str) -> PathBuf {
        let path = self.project(name);
        git(&path, &["init", "-q", "-b", "main"]);
        std::fs::write(path.join("README.md"), "# Proyek\n").unwrap();
        git(&path, &["add", "-A"]);
        git(&path, &["commit", "-q", "-m", "Commit pertama"]);
        self.core.store.set_trusted(&path.to_string_lossy(), true);
        path
    }
}

/// Menunggu event pertama yang cocok dengan `pick`; event lain dilewati. Panik setelah 20 detik.
pub async fn recv<T>(events: &mut UnboundedReceiver<CoreEvent>, mut pick: impl FnMut(CoreEvent) -> Option<T>) -> T {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let event = events.recv().await.expect("penampung event tertutup");
            if let Some(value) = pick(event) {
                return value;
            }
        }
    })
    .await
    .expect("event yang ditunggu tidak datang")
}

/// Path kanonik seperti yang dipakai `workspace_open`: di Windows tanpa awalan `\\?\`.
fn canonical(path: &std::path::Path) -> PathBuf {
    let path = std::fs::canonicalize(path).unwrap();
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => path,
    }
}
