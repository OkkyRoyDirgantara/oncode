//! Hot exit: unsaved changes of open files (and new, untitled files) are kept on disk while they are
//! edited and when a window closes, so they come back the next time the folder is opened.
//!
//! One JSON file per buffer under `<data dir>/backups/<folder key>/`, never in `state.json` (which is
//! rewritten whole on every change and has a size limit for sessions).

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::state::Shared;

/// The unsaved content of one buffer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BufferBackup {
    /// The buffer's path: an absolute file path, or `untitled:Untitled-N`.
    pub path: String,
    pub text: String,
    pub encoding: String,
    pub bom: bool,
    /// `"lf"` or `"crlf"`.
    pub eol: String,
    /// Modification time and size of the file on disk when the buffer was loaded or last saved; a file
    /// that changed after that is shown as a conflict when the backup is restored.
    pub mtime_ms: f64,
    pub size: u32,
}

/// A stable short name for a string: FNV-1a (64 bit). Not `DefaultHasher`, whose algorithm may change
/// between Rust versions, which would hide the backups an older build wrote.
fn key(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn folder_dir(core: &Shared, root: &str) -> PathBuf {
    core.backups_dir().join(key(root))
}

fn backup_file(core: &Shared, root: &str, path: &str) -> PathBuf {
    folder_dir(core, root).join(format!("{}.json", key(path)))
}

fn io_error(error: std::io::Error) -> AppError {
    AppError::new("io", error.to_string())
}

/// Puts the writes and deletes of backup files in the order they were asked for. Each request takes a
/// number when it is made, on the caller's thread; one that reaches the disk after a newer request for
/// the same file is dropped. The requests run on the backend's thread pool, where an older one could
/// otherwise finish last: a delayed backup still being written when the window closes and writes the
/// newest text, a backup written just before a save removes it, or one written just before all of the
/// folder's backups are cleared.
#[derive(Debug, Default)]
pub struct BackupQueue {
    last_ticket: AtomicU64,
    /// The newest request handled per backup file, and per folder for clears. Held while a file is
    /// written, so writes take turns.
    handled: tokio::sync::Mutex<HashMap<PathBuf, u64>>,
}

enum Change {
    Write(Vec<u8>),
    Delete,
    /// Removes the whole folder of backups (`target` is the folder).
    Clear,
}

impl BackupQueue {
    fn ticket(&self) -> u64 {
        self.last_ticket.fetch_add(1, Ordering::Relaxed) + 1
    }

    async fn apply(&self, target: PathBuf, ticket: u64, change: Change) -> AppResult<()> {
        let mut handled = self.handled.lock().await;
        // Dropped if a newer request for this file, or a newer clear of its folder, was handled already.
        let folder = if matches!(change, Change::Clear) { None } else { target.parent().map(Path::to_path_buf) };
        let superseded = |key: &Path| handled.get(key).is_some_and(|newest| *newest > ticket);
        if superseded(&target) || folder.as_deref().is_some_and(superseded) {
            return Ok(());
        }
        handled.insert(target.clone(), ticket);
        let file = target;
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            match change {
                // Written to a temporary file first, then renamed, so a crash in the middle never leaves half a backup.
                Change::Write(json) => {
                    if let Some(parent) = file.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    let temporary = file.with_extension("tmp");
                    std::fs::write(&temporary, json)?;
                    std::fs::rename(&temporary, &file)
                }
                Change::Delete => match std::fs::remove_file(&file) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    result => result,
                },
                Change::Clear => match std::fs::remove_dir_all(&file) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    result => result,
                },
            }
        })
        .await
        .map_err(|e| AppError::new("io", e.to_string()))?
        .map_err(io_error)
    }
}

/// Writes the backup of one buffer of the folder `root`.
///
/// Its place in the order is taken when this is called, not when the future first runs: pass the
/// returned future to the runtime as it is, without wrapping the call in an `async` block.
pub fn backup_write(core: &Shared, root: String, backup: BufferBackup) -> impl Future<Output = AppResult<()>> + Send + 'static {
    let file = backup_file(core, &root, &backup.path);
    let ticket = core.backup_queue.ticket();
    let json = serde_json::to_vec(&backup);
    let core = core.clone();
    async move {
        let json = json.map_err(|e| AppError::new("io", e.to_string()))?;
        core.backup_queue.apply(file, ticket, Change::Write(json)).await
    }
}

/// Removes the backup of one buffer (saved, reverted, or its changes discarded). A missing backup is
/// fine. Ordered with [`backup_write`] the same way.
pub fn backup_delete(core: &Shared, root: String, path: String) -> impl Future<Output = AppResult<()>> + Send + 'static {
    let file = backup_file(core, &root, &path);
    let ticket = core.backup_queue.ticket();
    let core = core.clone();
    async move { core.backup_queue.apply(file, ticket, Change::Delete).await }
}

/// Removes all backups of the folder `root`: with hot exit off they are not restored, and kept they
/// could come back with outdated content once it is turned on again. Ordered with [`backup_write`]
/// the same way: a write asked for before this cannot bring a backup back afterwards.
pub fn backup_clear(core: &Shared, root: String) -> impl Future<Output = AppResult<()>> + Send + 'static {
    let dir = folder_dir(core, &root);
    let ticket = core.backup_queue.ticket();
    let core = core.clone();
    async move { core.backup_queue.apply(dir, ticket, Change::Clear).await }
}

/// All backups of the folder `root`. Unreadable files are skipped.
pub async fn backup_list(core: &Shared, root: String) -> AppResult<Vec<BufferBackup>> {
    let dir = folder_dir(core, &root);
    tokio::task::spawn_blocking(move || read_all(&dir)).await.map_err(|e| AppError::new("io", e.to_string()))
}

fn read_all(dir: &Path) -> Vec<BufferBackup> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut backups: Vec<BufferBackup> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| std::fs::read(entry.path()).ok())
        .filter_map(|bytes| serde_json::from_slice(&bytes).ok())
        .collect();
    backups.sort_by(|a, b| a.path.cmp(&b.path));
    backups
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestCore;

    fn backup(path: &str, text: &str) -> BufferBackup {
        BufferBackup { path: path.into(), text: text.into(), encoding: "UTF-8".into(), bom: false, eol: "lf".into(), mtime_ms: 1.0, size: 2 }
    }

    #[test]
    fn keys_are_stable() {
        assert_eq!(key(""), "cbf29ce484222325");
        assert_eq!(key("a"), "af63dc4c8601ec8c");
    }

    #[tokio::test]
    async fn backups_are_kept_per_folder_and_replaced_and_deleted() {
        let t = TestCore::new();
        backup_write(&t.core, "/a".into(), backup("/a/x.txt", "one")).await.unwrap();
        backup_write(&t.core, "/a".into(), backup("untitled:Untitled-1", "new")).await.unwrap();
        backup_write(&t.core, "/b".into(), backup("/b/y.txt", "other")).await.unwrap();
        backup_write(&t.core, "/a".into(), backup("/a/x.txt", "two")).await.unwrap();

        let listed = backup_list(&t.core, "/a".into()).await.unwrap();
        assert_eq!(listed.iter().map(|b| (b.path.as_str(), b.text.as_str())).collect::<Vec<_>>(), [("/a/x.txt", "two"), ("untitled:Untitled-1", "new")]);

        backup_delete(&t.core, "/a".into(), "/a/x.txt".into()).await.unwrap();
        backup_delete(&t.core, "/a".into(), "/a/missing.txt".into()).await.unwrap();
        assert_eq!(backup_list(&t.core, "/a".into()).await.unwrap().len(), 1);
        assert_eq!(backup_list(&t.core, "/c".into()).await.unwrap(), Vec::new());

        backup_clear(&t.core, "/a".into()).await.unwrap();
        backup_clear(&t.core, "/never".into()).await.unwrap();
        assert!(backup_list(&t.core, "/a".into()).await.unwrap().is_empty());
        assert_eq!(backup_list(&t.core, "/b".into()).await.unwrap().len(), 1, "other folders keep theirs");
    }

    #[tokio::test]
    async fn a_write_asked_for_before_a_clear_does_not_bring_the_backup_back() {
        let t = TestCore::new();
        backup_write(&t.core, "/a".into(), backup("/a/x.txt", "kept")).await.unwrap();
        let late = backup_write(&t.core, "/a".into(), backup("/a/y.txt", "late"));
        backup_clear(&t.core, "/a".into()).await.unwrap();
        late.await.unwrap();
        assert!(backup_list(&t.core, "/a".into()).await.unwrap().is_empty());

        // A write asked for after the clear is kept.
        backup_write(&t.core, "/a".into(), backup("/a/x.txt", "new")).await.unwrap();
        assert_eq!(backup_list(&t.core, "/a".into()).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn an_older_request_that_runs_last_does_not_win() {
        async fn texts(core: &Shared) -> Vec<String> {
            backup_list(core, "/a".into()).await.unwrap().into_iter().map(|b| b.text).collect()
        }
        let t = TestCore::new();

        let older = backup_write(&t.core, "/a".into(), backup("/a/x.txt", "old"));
        let newer = backup_write(&t.core, "/a".into(), backup("/a/x.txt", "new"));
        newer.await.unwrap();
        older.await.unwrap();
        assert_eq!(texts(&t.core).await, ["new"]);

        // Saved while the last backup was still on its way: the backup stays removed.
        let write = backup_write(&t.core, "/a".into(), backup("/a/x.txt", "newest"));
        let delete = backup_delete(&t.core, "/a".into(), "/a/x.txt".into());
        delete.await.unwrap();
        write.await.unwrap();
        assert!(texts(&t.core).await.is_empty());

        // Concurrent writes of one file take turns and the last one asked for is kept.
        let writes: Vec<_> = (0..20).map(|i| tokio::spawn(backup_write(&t.core, "/a".into(), backup("/a/x.txt", &i.to_string())))).collect();
        for write in writes {
            write.await.unwrap().unwrap();
        }
        assert_eq!(texts(&t.core).await, ["19"]);
    }
}
