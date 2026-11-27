//! File contents for the diff view, and partial staging through line splicing.
//!
//! Per-hunk/per-line staging does not use `git apply`. The UI sends line ranges from
//! the diff it is currently showing, and the backend builds the new content and writes it
//! directly as a blob into the index (`hash-object` + `update-index --cacheinfo`).
//! This way the result does not depend on any particular diff algorithm, and the
//! content fingerprints of both sides ensure the diff the UI is using is not stale.

use crate::error::{GitError, GitErrorCode, Result};
use crate::model::*;
use crate::repo::Repo;
use crate::runner::{check_path, check_ref, Cmd};

/// Size limit for text sent to the diff view.
pub const MAX_DIFF_BYTES: usize = 5 * 1024 * 1024;

/// Content fingerprint (64-bit FNV-1a) in hexadecimal. `None` (side absent) differs from empty content.
pub fn fingerprint(data: Option<&[u8]>) -> String {
    let Some(data) = data else { return "-".into() };
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in data {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

pub fn is_binary(data: &[u8]) -> bool {
    data.iter().take(8000).any(|b| *b == 0)
}

/// A Git LFS pointer file (the text stored in git instead of the content).
fn is_lfs_pointer(bytes: &[u8]) -> bool {
    bytes.len() < 1024 && bytes.starts_with(b"version https://git-lfs.github.com/spec/v1\n")
}

fn side(data: Option<Vec<u8>>, label: String) -> DiffSide {
    let fingerprint = fingerprint(data.as_deref());
    match data {
        None => DiffSide { text: None, exists: false, binary: false, too_large: false, size: 0, fingerprint, label },
        Some(bytes) => {
            let binary = is_binary(&bytes);
            let too_large = bytes.len() > MAX_DIFF_BYTES;
            DiffSide {
                text: (!binary && !too_large).then(|| String::from_utf8_lossy(&bytes).into_owned()),
                exists: true,
                binary,
                too_large,
                size: bytes.len().min(u32::MAX as usize) as u32,
                fingerprint,
                label,
            }
        }
    }
}

/// Splits content into lines like the editor's document model:`"a\nb\n"` → `["a", "b", ""]`.
fn split_lines(data: &[u8]) -> Vec<&[u8]> {
    data.split(|b| *b == b'\n').collect()
}

fn check_ops(ops: &[SpliceOp], a_len: usize, b_len: usize) -> Result<()> {
    let mut last_a = 0u32;
    let mut last_b = 0u32;
    for op in ops {
        let ordered = op.a_from <= op.a_to && op.b_from <= op.b_to && op.a_from >= last_a && op.b_from >= last_b;
        let in_bounds = op.a_to as usize <= a_len && op.b_to as usize <= b_len;
        if !ordered || !in_bounds {
            return Err(GitError::new(GitErrorCode::Stale, "Line range does not match the file contents; reload the diff"));
        }
        last_a = op.a_to;
        last_b = op.b_to;
    }
    Ok(())
}

/// Produces A with `A[a_from..a_to)` replaced by `B[b_from..b_to)` for each operation.
pub fn splice_forward(a: &[u8], b: &[u8], ops: &[SpliceOp]) -> Result<Vec<u8>> {
    let (a_lines, b_lines) = (split_lines(a), split_lines(b));
    check_ops(ops, a_lines.len(), b_lines.len())?;
    let mut out: Vec<&[u8]> = Vec::with_capacity(a_lines.len());
    let mut cursor = 0usize;
    for op in ops {
        out.extend_from_slice(&a_lines[cursor..op.a_from as usize]);
        out.extend_from_slice(&b_lines[op.b_from as usize..op.b_to as usize]);
        cursor = op.a_to as usize;
    }
    out.extend_from_slice(&a_lines[cursor..]);
    Ok(out.join(&b'\n'))
}

/// Produces B with `B[b_from..b_to)` replaced by `A[a_from..a_to)` for each operation.
pub fn splice_reverse(a: &[u8], b: &[u8], ops: &[SpliceOp]) -> Result<Vec<u8>> {
    let swapped: Vec<SpliceOp> =
        ops.iter().map(|op| SpliceOp { a_from: op.b_from, a_to: op.b_to, b_from: op.a_from, b_to: op.a_to }).collect();
    splice_forward(b, a, &swapped)
}

/// Makes the line endings of `source` match the style of `target` (CRLF or LF).
fn match_eol(source: &[u8], target: &[u8]) -> Vec<u8> {
    let target_crlf = target.windows(2).any(|w| w == b"\r\n");
    let source_crlf = source.windows(2).any(|w| w == b"\r\n");
    if target_crlf == source_crlf || source.is_empty() {
        return source.to_vec();
    }
    if target_crlf {
        let mut out = Vec::with_capacity(source.len() + source.len() / 20);
        for (i, byte) in source.iter().enumerate() {
            if *byte == b'\n' && (i == 0 || source[i - 1] != b'\r') {
                out.push(b'\r');
            }
            out.push(*byte);
        }
        out
    } else {
        let mut out = Vec::with_capacity(source.len());
        for (i, byte) in source.iter().enumerate() {
            if *byte == b'\r' && source.get(i + 1) == Some(&b'\n') {
                continue;
            }
            out.push(*byte);
        }
        out
    }
}

impl Repo {
    /// The blob content at an object spec (`HEAD:path`, `:path`, ...), or `None` if it does not exist.
    pub async fn blob(&self, spec: &str) -> Result<Option<Vec<u8>>> {
        let out = self.run_raw(Cmd::new(["cat-file", "blob", "--end-of-options", spec])).await?;
        Ok((out.code == 0).then_some(out.stdout))
    }

    /// The blob for showing in a diff: a Git LFS pointer is replaced by the file's real content through git's
    /// filters (`cat-file --filters`), when git-lfs is installed. The flag says whether that happened.
    async fn display_blob(&self, spec: &str) -> Result<(Option<Vec<u8>>, bool)> {
        let raw = self.blob(spec).await?;
        let Some(pointer) = raw.as_ref().filter(|bytes| is_lfs_pointer(bytes)) else { return Ok((raw, false)) };
        let out = self.run_raw(Cmd::new(["cat-file", "--filters", "--end-of-options", spec])).await?;
        if out.code == 0 && !out.stdout.is_empty() && out.stdout != *pointer {
            return Ok((Some(out.stdout), true));
        }
        Ok((raw, false))
    }

    async fn worktree_file(&self, path: &str) -> Option<Vec<u8>> {
        let full = self.root().join(path);
        match tokio::fs::symlink_metadata(&full).await {
            Ok(meta) if meta.is_file() => tokio::fs::read(&full).await.ok(),
            Ok(meta) if meta.file_type().is_symlink() => {
                tokio::fs::read_link(&full).await.ok().map(|t| t.to_string_lossy().into_owned().into_bytes())
            }
            _ => None,
        }
    }

    /// Two versions of a file to display side by side.
    pub async fn diff_contents(&self, path: &str, orig_path: Option<&str>, source: &DiffSource) -> Result<DiffContents> {
        let path = check_path(path)?;
        let old_path = match orig_path {
            Some(p) => check_path(p)?,
            None => path,
        };
        let mut lfs = false;
        let mut shown = async |spec: String| -> Result<Option<Vec<u8>>> {
            let (data, smudged) = self.display_blob(&spec).await?;
            lfs |= smudged;
            Ok(data)
        };
        let (a, b) = match source {
            DiffSource::Worktree => (side(shown(format!(":{path}")).await?, "Index".into()), side(self.worktree_file(path).await, "Working tree".into())),
            DiffSource::Staged => (side(shown(format!("HEAD:{old_path}")).await?, "HEAD".into()), side(shown(format!(":{path}")).await?, "Index".into())),
            DiffSource::Commit { oid, parent } => {
                let oid = check_ref(oid)?;
                let parent = match parent {
                    Some(p) => check_ref(p)?.to_string(),
                    None => format!("{oid}^"),
                };
                (side(shown(format!("{parent}:{old_path}")).await?, short(&parent)), side(shown(format!("{oid}:{path}")).await?, short(oid)))
            }
            DiffSource::Range { from, to } => {
                let (from, to) = (check_ref(from)?, check_ref(to)?);
                (side(shown(format!("{from}:{old_path}")).await?, short(from)), side(shown(format!("{to}:{path}")).await?, short(to)))
            }
            DiffSource::Stash { name } => {
                let name = check_ref(name)?;
                let mut after = shown(format!("{name}:{path}")).await?;
                if after.is_none() {
                    // Untracked files are stored in the stash's third parent.
                    after = shown(format!("{name}^3:{path}")).await?;
                }
                (side(shown(format!("{name}^1:{old_path}")).await?, "Before".into()), side(after, name.to_string()))
            }
        };
        drop(shown);
        Ok(DiffContents { path: path.to_string(), orig_path: orig_path.map(String::from), a, b, lfs })
    }

    /// The index mode of a file (e.g. `100644`), if it is in the index.
    async fn index_mode(&self, path: &str) -> Result<Option<String>> {
        let out = self.run(Cmd::new(["ls-files", "-s", "-z"]).paths([path])).await?;
        let text = out.text();
        Ok(text.split('\0').find(|r| !r.is_empty()).and_then(|r| r.split(' ').next()).map(String::from))
    }

    async fn worktree_mode(&self, path: &str) -> String {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = tokio::fs::metadata(self.root().join(path)).await {
                if meta.permissions().mode() & 0o111 != 0 {
                    return "100755".into();
                }
            }
        }
        let _ = path;
        "100644".into()
    }

    async fn write_index_blob(&self, path: &str, mode: &str, content: Vec<u8>) -> Result<()> {
        let oid = self.run(Cmd::new(["hash-object", "-w", "--stdin"]).stdin(content).write()).await?.trimmed();
        self.run(Cmd::new(["update-index", "--add", "--cacheinfo", &format!("{mode},{oid},{path}")]).write()).await?;
        Ok(())
    }

    /// Stages, unstages, or discards part of a file's changes.
    ///
    /// `fp_a` and `fp_b` are the fingerprints of both diff sides shown by the UI; if the
    /// content has changed since then, the operation is rejected with code `Stale`.
    pub async fn apply_splice(&self, target: SpliceTarget, path: &str, ops: &[SpliceOp], fp_a: &str, fp_b: &str) -> Result<()> {
        let path = check_path(path)?;
        let _guard = self.lock().await;
        let stale = || GitError::new(GitErrorCode::Stale, "The file changed since the diff was shown; reload and try again");

        let (a_spec, b_blob_spec) = match target {
            SpliceTarget::Stage | SpliceTarget::Discard => (format!(":{path}"), None),
            SpliceTarget::Unstage => (format!("HEAD:{path}"), Some(format!(":{path}"))),
        };
        let a = self.blob(&a_spec).await?;
        let b = match &b_blob_spec {
            Some(spec) => self.blob(spec).await?,
            None => self.worktree_file(path).await,
        };
        if fingerprint(a.as_deref()) != fp_a || fingerprint(b.as_deref()) != fp_b {
            return Err(stale());
        }
        if a.as_deref().is_some_and(is_binary) || b.as_deref().is_some_and(is_binary) {
            return Err(GitError::invalid("Binary files can only be staged as a whole file"));
        }
        let a_bytes = a.clone().unwrap_or_default();
        let b_bytes = b.clone().unwrap_or_default();

        match target {
            SpliceTarget::Stage => {
                // The worktree side is cleaned first (clean filter, line-ending conversion) so it is equivalent to the index content.
                let b_clean = if b.is_some() {
                    let oid = self.run(Cmd::new(["hash-object", "-w"]).paths([path]).write()).await?.trimmed();
                    self.blob(&oid).await?.ok_or_else(stale)?
                } else {
                    Vec::new()
                };
                let content = splice_forward(&a_bytes, &b_clean, ops)?;
                if content.is_empty() && b.is_none() {
                    self.run(Cmd::new(["update-index", "--force-remove"]).paths([path]).write()).await?;
                    return Ok(());
                }
                let mode = match self.index_mode(path).await? {
                    Some(mode) => mode,
                    None => self.worktree_mode(path).await,
                };
                self.write_index_blob(path, &mode, content).await
            }
            SpliceTarget::Unstage => {
                let content = splice_reverse(&a_bytes, &b_bytes, ops)?;
                if a.is_none() && content.is_empty() {
                    // A new file whose entire content is unstaged becomes untracked again.
                    self.run(Cmd::new(["update-index", "--force-remove"]).paths([path]).write()).await?;
                    return Ok(());
                }
                let mode = match self.index_mode(path).await? {
                    Some(mode) => mode,
                    None => {
                        let out = self.run(Cmd::new(["ls-tree", "-z", "HEAD"]).paths([path])).await?;
                        out.text().split(' ').next().filter(|m| !m.is_empty()).unwrap_or("100644").to_string()
                    }
                };
                self.write_index_blob(path, &mode, content).await
            }
            SpliceTarget::Discard => {
                if b.is_none() {
                    return Err(GitError::invalid("The file is not in the working tree; restore the whole file"));
                }
                let full = self.root().join(path);
                // A link's "content" is its target path; writing it would clobber the file the link points to.
                if tokio::fs::symlink_metadata(&full).await.is_ok_and(|meta| meta.file_type().is_symlink()) {
                    return Err(GitError::invalid("Part of a symbolic link cannot be discarded; restore the whole file"));
                }
                let a_matched = match_eol(&a_bytes, &b_bytes);
                let content = splice_reverse(&a_matched, &b_bytes, ops)?;
                replace_file(full, content).await.map_err(|e| GitError::io("Failed to write file", e))
            }
        }
    }
}

/// Replaces a working tree file in one step: the content goes to a temporary file next to
/// it, which is then renamed over it. Readers (the file watcher, an open editor) never see
/// a truncated file, a failed write leaves the original intact, and the permissions stay.
async fn replace_file(path: std::path::PathBuf, content: Vec<u8>) -> std::io::Result<()> {
    tokio::task::spawn_blocking(move || {
        use std::io::Write;
        let permissions = std::fs::metadata(&path)?.permissions();
        if permissions.readonly() {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "the file is read-only"));
        }
        let dir = path.parent().unwrap_or(std::path::Path::new("."));
        // Same name pattern as the workspace's atomic saves, which the file watcher ignores.
        let temp = dir.join(format!(".oncode-{}.tmp", uuid::Uuid::new_v4().simple()));
        let written = (|| {
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;
            file.write_all(&content)?;
            file.sync_all()?;
            drop(file);
            std::fs::set_permissions(&temp, permissions)?;
            std::fs::rename(&temp, &path)
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        written
    })
    .await
    .map_err(std::io::Error::other)?
}

fn short(rev: &str) -> String {
    if rev.len() >= 40 && rev.chars().take(40).all(|c| c.is_ascii_hexdigit()) {
        format!("{}{}", &rev[..8], &rev[40..])
    } else {
        rev.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(a_from: u32, a_to: u32, b_from: u32, b_to: u32) -> SpliceOp {
        SpliceOp { a_from, a_to, b_from, b_to }
    }

    #[test]
    fn forward_replaces_selected_ranges_only() {
        let a = b"1\n2\n3\n4\n5\n";
        let b = b"1\nTWO\n3\n4\nFIVE\nSIX\n";
        // Only the first change.
        assert_eq!(splice_forward(a, b, &[op(1, 2, 1, 2)]).unwrap(), b"1\nTWO\n3\n4\n5\n");
        // Only the second change.
        assert_eq!(splice_forward(a, b, &[op(4, 5, 4, 6)]).unwrap(), b"1\n2\n3\n4\nFIVE\nSIX\n");
        // Both produce B.
        assert_eq!(splice_forward(a, b, &[op(1, 2, 1, 2), op(4, 5, 4, 6)]).unwrap(), b);
    }

    #[test]
    fn reverse_restores_selected_ranges_only() {
        let a = b"1\n2\n3\n";
        let b = b"1\nTWO\n3\nFOUR\n";
        assert_eq!(splice_reverse(a, b, &[op(1, 2, 1, 2)]).unwrap(), b"1\n2\n3\nFOUR\n");
        assert_eq!(splice_reverse(a, b, &[op(3, 3, 3, 4)]).unwrap(), b"1\nTWO\n3\n");
    }

    #[test]
    fn handles_missing_trailing_newline_like_the_editor_model() {
        // A has no trailing newline, B adds a line: the last virtual line is selected too.
        assert_eq!(splice_forward(b"a", b"a\nb\n", &[op(1, 1, 1, 3)]).unwrap(), b"a\nb\n");
        // B removes the last line along with its newline.
        assert_eq!(splice_forward(b"a\nb\n", b"a", &[op(1, 3, 1, 1)]).unwrap(), b"a");
        // New file: A is empty.
        assert_eq!(splice_forward(b"", b"x\ny\n", &[op(0, 1, 0, 3)]).unwrap(), b"x\ny\n");
        // Some of the lines of a new file.
        assert_eq!(splice_forward(b"", b"x\ny\n", &[op(0, 0, 1, 2)]).unwrap(), b"y\n");
    }

    #[test]
    fn rejects_out_of_range_or_unordered_ops() {
        assert_eq!(splice_forward(b"a\n", b"b\n", &[op(0, 9, 0, 1)]).unwrap_err().code, GitErrorCode::Stale);
        assert!(splice_forward(b"a\nb\n", b"c\nd\n", &[op(1, 2, 1, 2), op(0, 1, 0, 1)]).is_err());
    }

    #[test]
    fn preserves_crlf_lines() {
        let a = b"1\r\n2\r\n";
        let b = b"1\r\nTWO\r\n";
        assert_eq!(splice_forward(a, b, &[op(1, 2, 1, 2)]).unwrap(), b"1\r\nTWO\r\n");
        assert_eq!(match_eol(b"x\ny\n", b"a\r\nb\r\n"), b"x\r\ny\r\n");
        assert_eq!(match_eol(b"x\r\ny\r\n", b"a\nb\n"), b"x\ny\n");
    }

    #[test]
    fn fingerprint_distinguishes_missing_from_empty() {
        assert_ne!(fingerprint(None), fingerprint(Some(b"")));
        assert_ne!(fingerprint(Some(b"a")), fingerprint(Some(b"b")));
        assert_eq!(fingerprint(Some(b"abc")), fingerprint(Some(b"abc")));
    }
}
