//! Isi berkas untuk tampilan diff, dan staging parsial lewat sambung-potong baris.
//!
//! Staging per hunk/baris tidak memakai `git apply`. UI mengirim rentang baris dari
//! diff yang sedang ditampilkannya, backend menyusun isi baru lalu menuliskannya
//! langsung sebagai blob ke index (`hash-object` + `update-index --cacheinfo`).
//! Dengan begitu hasilnya tidak bergantung pada algoritma diff mana pun, dan sidik
//! isi kedua sisi memastikan diff yang dipakai UI belum usang.

use crate::error::{GitError, GitErrorCode, Result};
use crate::model::*;
use crate::repo::Repo;
use crate::runner::{check_path, check_ref, Cmd};

/// Batas ukuran teks yang dikirim ke tampilan diff.
pub const MAX_DIFF_BYTES: usize = 5 * 1024 * 1024;

/// Sidik isi (FNV-1a 64-bit) dalam heksadesimal. `None` (sisi tidak ada) berbeda dari isi kosong.
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

/// Memecah isi menjadi baris seperti model dokumen editor: `"a\nb\n"` → `["a", "b", ""]`.
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

/// Menghasilkan A dengan `A[a_from..a_to)` diganti `B[b_from..b_to)` untuk tiap operasi.
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

/// Menghasilkan B dengan `B[b_from..b_to)` diganti `A[a_from..a_to)` untuk tiap operasi.
pub fn splice_reverse(a: &[u8], b: &[u8], ops: &[SpliceOp]) -> Result<Vec<u8>> {
    let swapped: Vec<SpliceOp> =
        ops.iter().map(|op| SpliceOp { a_from: op.b_from, a_to: op.b_to, b_from: op.a_from, b_to: op.a_to }).collect();
    splice_forward(b, a, &swapped)
}

/// Menyamakan akhir baris `source` dengan gaya `target` (CRLF atau LF).
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
    /// Isi blob pada sebuah spesifikasi objek (`HEAD:path`, `:path`, ...), atau `None` bila tidak ada.
    pub async fn blob(&self, spec: &str) -> Result<Option<Vec<u8>>> {
        let out = self.run_raw(Cmd::new(["cat-file", "blob", "--end-of-options", spec])).await?;
        Ok((out.code == 0).then_some(out.stdout))
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

    /// Dua versi sebuah berkas untuk ditampilkan berdampingan.
    pub async fn diff_contents(&self, path: &str, orig_path: Option<&str>, source: &DiffSource) -> Result<DiffContents> {
        let path = check_path(path)?;
        let old_path = match orig_path {
            Some(p) => check_path(p)?,
            None => path,
        };
        let (a, b) = match source {
            DiffSource::Worktree => (
                side(self.blob(&format!(":{path}")).await?, "Index".into()),
                side(self.worktree_file(path).await, "Working tree".into()),
            ),
            DiffSource::Staged => (
                side(self.blob(&format!("HEAD:{old_path}")).await?, "HEAD".into()),
                side(self.blob(&format!(":{path}")).await?, "Index".into()),
            ),
            DiffSource::Commit { oid, parent } => {
                let oid = check_ref(oid)?;
                let parent = match parent {
                    Some(p) => check_ref(p)?.to_string(),
                    None => format!("{oid}^"),
                };
                (
                    side(self.blob(&format!("{parent}:{old_path}")).await?, short(&parent)),
                    side(self.blob(&format!("{oid}:{path}")).await?, short(oid)),
                )
            }
            DiffSource::Range { from, to } => {
                let (from, to) = (check_ref(from)?, check_ref(to)?);
                (
                    side(self.blob(&format!("{from}:{old_path}")).await?, short(from)),
                    side(self.blob(&format!("{to}:{path}")).await?, short(to)),
                )
            }
            DiffSource::Stash { name } => {
                let name = check_ref(name)?;
                let mut after = self.blob(&format!("{name}:{path}")).await?;
                if after.is_none() {
                    // Berkas untracked disimpan di induk ketiga stash.
                    after = self.blob(&format!("{name}^3:{path}")).await?;
                }
                (side(self.blob(&format!("{name}^1:{old_path}")).await?, "Before".into()), side(after, name.to_string()))
            }
        };
        Ok(DiffContents { path: path.to_string(), orig_path: orig_path.map(String::from), a, b })
    }

    /// Mode index sebuah berkas (mis. `100644`), bila ada di index.
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

    /// Stage, unstage, atau discard sebagian perubahan sebuah berkas.
    ///
    /// `fp_a` dan `fp_b` adalah sidik kedua sisi diff yang ditampilkan UI; bila isi
    /// sudah berubah sejak itu, operasi ditolak dengan kode `Stale`.
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
                // Sisi worktree dibersihkan dulu (filter clean, konversi akhir baris) agar setara dengan isi index.
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
                    // Berkas baru yang seluruh isinya dikeluarkan kembali menjadi untracked.
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
                let a_matched = match_eol(&a_bytes, &b_bytes);
                let content = splice_reverse(&a_matched, &b_bytes, ops)?;
                tokio::fs::write(self.root().join(path), content)
                    .await
                    .map_err(|e| GitError::io("Failed to write file", e))
            }
        }
    }
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
        // Hanya perubahan pertama.
        assert_eq!(splice_forward(a, b, &[op(1, 2, 1, 2)]).unwrap(), b"1\nTWO\n3\n4\n5\n");
        // Hanya perubahan kedua.
        assert_eq!(splice_forward(a, b, &[op(4, 5, 4, 6)]).unwrap(), b"1\n2\n3\n4\nFIVE\nSIX\n");
        // Keduanya menghasilkan B.
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
        // A tanpa newline di akhir, B menambah baris: baris maya terakhir ikut dipilih.
        assert_eq!(splice_forward(b"a", b"a\nb\n", &[op(1, 1, 1, 3)]).unwrap(), b"a\nb\n");
        // B menghapus baris terakhir beserta newline-nya.
        assert_eq!(splice_forward(b"a\nb\n", b"a", &[op(1, 3, 1, 1)]).unwrap(), b"a");
        // Berkas baru: A kosong.
        assert_eq!(splice_forward(b"", b"x\ny\n", &[op(0, 1, 0, 3)]).unwrap(), b"x\ny\n");
        // Sebagian baris berkas baru.
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
