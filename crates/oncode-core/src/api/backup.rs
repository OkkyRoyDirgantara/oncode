//! Hot exit: unsaved changes of open files (and new, untitled files) are kept on disk while they are
//! edited and when a window closes, so they come back the next time the folder is opened.
//!
//! One JSON file per buffer under `<data dir>/backups/<folder key>/`, never in `state.json` (which is
//! rewritten whole on every change and has a size limit for sessions).

use std::path::{Path, PathBuf};

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

/// Writes the backup of one buffer of the folder `root`. Written to a temporary file first, then
/// renamed, so a crash in the middle never leaves half a backup.
pub async fn backup_write(core: &Shared, root: String, backup: BufferBackup) -> AppResult<()> {
    let file = backup_file(core, &root, &backup.path);
    let json = serde_json::to_vec(&backup).map_err(|e| AppError::new("io", e.to_string()))?;
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = file.with_extension("tmp");
        std::fs::write(&temporary, json)?;
        std::fs::rename(&temporary, &file)
    })
    .await
    .map_err(|e| AppError::new("io", e.to_string()))?
    .map_err(io_error)
}

/// Removes the backup of one buffer (saved, reverted, or its changes discarded). A missing backup is fine.
pub async fn backup_delete(core: &Shared, root: String, path: String) -> AppResult<()> {
    let file = backup_file(core, &root, &path);
    match tokio::fs::remove_file(&file).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(error)),
    }
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
    }
}
