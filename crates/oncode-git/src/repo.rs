//! A handle to a single repository worktree.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{Mutex, MutexGuard};

use crate::error::{GitError, GitErrorCode, Result};
use crate::runner::{Cmd, Git, Output};

#[derive(Clone)]
pub struct Repo {
    inner: Arc<Inner>,
}

struct Inner {
    git: Arc<Git>,
    root: PathBuf,
    git_dir: PathBuf,
    common_dir: PathBuf,
    /// Write operation queue: only one runs at a time.
    write_lock: Mutex<()>,
}

impl Repo {
    /// Opens the repository that contains `path`.
    pub async fn open(git: Arc<Git>, path: &Path) -> Result<Self> {
        let out = git
            .run(
                path,
                Cmd::new(["rev-parse", "--path-format=absolute", "--show-toplevel", "--absolute-git-dir", "--git-common-dir"]),
            )
            .await?;
        let text = out.text();
        let mut lines = text.lines();
        let (Some(root), Some(git_dir), Some(common_dir)) = (lines.next(), lines.next(), lines.next()) else {
            return Err(GitError::new(GitErrorCode::NotARepo, "Not a git repository with a working tree"));
        };
        Ok(Self {
            inner: Arc::new(Inner {
                git,
                root: PathBuf::from(root),
                git_dir: PathBuf::from(git_dir),
                common_dir: PathBuf::from(common_dir),
                write_lock: Mutex::new(()),
            }),
        })
    }

    /// The root of the worktree that contains `path`, or `None` if it is not a repository.
    pub async fn find_root(git: &Git, path: &Path) -> Option<PathBuf> {
        let out = git.run(path, Cmd::new(["rev-parse", "--path-format=absolute", "--show-toplevel"])).await.ok()?;
        let root = out.trimmed();
        (!root.is_empty()).then(|| PathBuf::from(root))
    }

    pub fn git(&self) -> &Arc<Git> {
        &self.inner.git
    }

    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    /// The git directory of this worktree (holds HEAD, index, operation markers).
    pub fn git_dir(&self) -> &Path {
        &self.inner.git_dir
    }

    /// The shared git directory (holds refs and objects).
    pub fn common_dir(&self) -> &Path {
        &self.inner.common_dir
    }

    pub fn root_string(&self) -> String {
        self.inner.root.to_string_lossy().into_owned()
    }

    pub(crate) async fn run(&self, cmd: Cmd) -> Result<Output> {
        self.inner.git.run(&self.inner.root, cmd).await
    }

    pub(crate) async fn run_raw(&self, cmd: Cmd) -> Result<Output> {
        self.inner.git.run_raw(&self.inner.root, cmd).await
    }

    /// Takes a turn for a write operation. Do not call it nested.
    pub(crate) async fn lock(&self) -> MutexGuard<'_, ()> {
        self.inner.write_lock.lock().await
    }

    /// Waits for the write operation currently running to finish.
    pub async fn wait_idle(&self) {
        drop(self.inner.write_lock.lock().await);
    }

    pub fn is_busy(&self) -> bool {
        self.inner.write_lock.try_lock().is_err()
    }
}
