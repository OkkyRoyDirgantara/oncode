//! Shared tooling for integration tests: a temporary repository isolated from the user's configuration.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Once};

use oncode_git::{Git, Repo};
use tempfile::TempDir;

static ENV: Once = Once::new();

/// Isolates git from the global/system configuration and sets the test identity.
pub fn isolate() {
    ENV.call_once(|| {
        let vars = [
            // The platform's empty file: `NUL` on Windows.
            ("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" }),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_AUTHOR_NAME", "Tester"),
            ("GIT_AUTHOR_EMAIL", "tester@example.com"),
            ("GIT_COMMITTER_NAME", "Tester"),
            ("GIT_COMMITTER_EMAIL", "tester@example.com"),
            // A submodule from a local path requires file protocol permission.
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
    /// An empty repository with a `main` branch.
    pub async fn new() -> Self {
        isolate();
        let dir = tempfile::tempdir().unwrap();
        let git = Git::discover(None).await.unwrap();
        raw_git(dir.path(), &["init", "-q", "-b", "main"]);
        let repo = Repo::open(git.clone(), dir.path()).await.unwrap();
        Self { dir, repo, git }
    }

    /// A repository with one commit containing `README.md`.
    pub async fn with_commit() -> Self {
        let t = Self::new().await;
        t.write("README.md", "# Project\n");
        t.git(&["add", "-A"]);
        t.git(&["commit", "-q", "-m", "First commit"]);
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

    /// Runs git directly (outside the layer under test) and returns stdout.
    pub fn git(&self, args: &[&str]) -> String {
        raw_git(self.path(), args)
    }

    /// Writes a file and then creates a commit containing all changes.
    pub fn commit_file(&self, rel: &str, content: &str, message: &str) -> String {
        self.write(rel, content);
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    /// The content of a file in the index.
    pub fn index(&self, rel: &str) -> String {
        self.git(&["show", &format!(":{rel}")])
    }
}

pub fn raw_git(cwd: &Path, args: &[&str]) -> String {
    isolate();
    let out = Command::new("git").current_dir(cwd).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "git {:?} failed: {}{}",
        args,
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Same as [`raw_git`] but does not panic if git fails.
pub fn try_git(cwd: &Path, args: &[&str]) -> bool {
    isolate();
    Command::new("git").current_dir(cwd).args(args).output().map(|o| o.status.success()).unwrap_or(false)
}
