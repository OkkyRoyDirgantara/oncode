//! Perkakas bersama uji integrasi: repositori sementara yang terisolasi dari konfigurasi pengguna.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Once};

use oncode_git::{Git, Repo};
use tempfile::TempDir;

static ENV: Once = Once::new();

/// Mengisolasi git dari konfigurasi global/sistem dan menetapkan identitas uji.
pub fn isolate() {
    ENV.call_once(|| {
        let vars = [
            // Berkas kosong milik platform: `NUL` di Windows.
            ("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" }),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_AUTHOR_NAME", "Penguji"),
            ("GIT_AUTHOR_EMAIL", "penguji@contoh.id"),
            ("GIT_COMMITTER_NAME", "Penguji"),
            ("GIT_COMMITTER_EMAIL", "penguji@contoh.id"),
            // Submodule dari path lokal memerlukan izin protokol file.
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

pub struct TestRepo {
    pub dir: TempDir,
    pub repo: Repo,
    pub git: Arc<Git>,
}

impl TestRepo {
    /// Repositori kosong dengan branch `main`.
    pub async fn new() -> Self {
        isolate();
        let dir = tempfile::tempdir().unwrap();
        let git = Git::discover(None).await.unwrap();
        raw_git(dir.path(), &["init", "-q", "-b", "main"]);
        let repo = Repo::open(git.clone(), dir.path()).await.unwrap();
        Self { dir, repo, git }
    }

    /// Repositori dengan satu commit berisi `README.md`.
    pub async fn with_commit() -> Self {
        let t = Self::new().await;
        t.write("README.md", "# Proyek\n");
        t.git(&["add", "-A"]);
        t.git(&["commit", "-q", "-m", "Commit pertama"]);
        t
    }

    pub fn path(&self) -> &Path {
        self.repo.root()
    }

    pub fn file(&self, rel: &str) -> PathBuf {
        self.path().join(rel)
    }

    pub fn write(&self, rel: &str, content: &str) {
        let full = self.file(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(full, content).unwrap();
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.file(rel)).unwrap()
    }

    /// Menjalankan git langsung (di luar lapisan yang diuji) dan mengembalikan stdout.
    pub fn git(&self, args: &[&str]) -> String {
        raw_git(self.path(), args)
    }

    /// Menulis berkas lalu membuat commit berisi seluruh perubahan.
    pub fn commit_file(&self, rel: &str, content: &str, message: &str) -> String {
        self.write(rel, content);
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    /// Isi berkas di index.
    pub fn index(&self, rel: &str) -> String {
        self.git(&["show", &format!(":{rel}")])
    }
}

pub fn raw_git(cwd: &Path, args: &[&str]) -> String {
    isolate();
    let out = Command::new("git").current_dir(cwd).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "git {:?} gagal: {}{}",
        args,
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Sama seperti [`raw_git`] tetapi tidak panik bila git gagal.
pub fn try_git(cwd: &Path, args: &[&str]) -> bool {
    isolate();
    Command::new("git").current_dir(cwd).args(args).output().map(|o| o.status.success()).unwrap_or(false)
}
