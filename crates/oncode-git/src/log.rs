//! Riwayat commit: format `git log`, parser, dan pembaca bertahap untuk graph.

use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStdout, Command};

use crate::error::{GitError, Result};
use crate::graph::GraphBuilder;
use crate::model::*;
use crate::repo::Repo;
use crate::runner::{check_path, check_ref, Cmd};

const FIELD: char = '\x1f';
/// Hash, induk, penulis, committer, ref, subjek; dipisah 0x1f.
pub(crate) const FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%cn%x1f%ce%x1f%ct%x1f%D%x1f%s";

fn parse_refs(decoration: &str) -> Vec<RefLabel> {
    let mut labels = Vec::new();
    for part in decoration.split(", ").map(str::trim).filter(|p| !p.is_empty()) {
        let (is_head, name) = match part.strip_prefix("HEAD -> ") {
            Some(rest) => (true, rest),
            None => (false, part),
        };
        if is_head || name == "HEAD" {
            labels.push(RefLabel { kind: RefKind::Head, name: "HEAD".into() });
            if name == "HEAD" {
                continue;
            }
        }
        if let Some(tag) = name.strip_prefix("tag: refs/tags/") {
            labels.push(RefLabel { kind: RefKind::Tag, name: tag.into() });
        } else if let Some(branch) = name.strip_prefix("refs/heads/") {
            labels.push(RefLabel { kind: RefKind::Branch, name: branch.into() });
        } else if let Some(remote) = name.strip_prefix("refs/remotes/") {
            if !remote.ends_with("/HEAD") {
                labels.push(RefLabel { kind: RefKind::Remote, name: remote.into() });
            }
        } else if name == "refs/stash" {
            labels.push(RefLabel { kind: RefKind::Stash, name: "stash".into() });
        }
    }
    labels
}

/// Mengurai satu rekaman dengan format [`FORMAT`].
pub fn parse_commit(record: &str) -> Option<Commit> {
    let record = record.trim_start_matches('\n');
    let mut fields = record.splitn(10, FIELD);
    let oid = fields.next()?.trim().to_string();
    if oid.is_empty() {
        return None;
    }
    let parents = fields.next()?.split(' ').filter(|p| !p.is_empty()).map(String::from).collect();
    let author = Signature {
        name: fields.next()?.to_string(),
        email: fields.next()?.to_string(),
        time: fields.next()?.parse::<i64>().unwrap_or(0) as Timestamp,
    };
    let committer = Signature {
        name: fields.next()?.to_string(),
        email: fields.next()?.to_string(),
        time: fields.next()?.parse::<i64>().unwrap_or(0) as Timestamp,
    };
    let refs = parse_refs(fields.next()?);
    let subject = fields.next().unwrap_or("").to_string();
    Some(Commit { oid, parents, author, committer, subject, refs })
}

pub(crate) fn parse_commits(raw: &[u8]) -> Vec<Commit> {
    raw.split(|b| *b == 0).filter_map(|r| parse_commit(&String::from_utf8_lossy(r))).collect()
}

/// Proses `git log` yang dibaca per halaman. Karena pipa menahan proses saat tidak
/// dibaca, commit yang tidak pernah dilihat tidak menimbulkan biaya.
pub struct LogStream {
    child: Child,
    reader: BufReader<ChildStdout>,
    graph: GraphBuilder,
    done: bool,
}

impl LogStream {
    /// Membaca hingga `count` commit berikutnya beserta posisi graph-nya.
    pub async fn next_page(&mut self, count: usize) -> Result<LogPage> {
        let mut rows = Vec::with_capacity(count);
        let mut buf = Vec::new();
        while !self.done && rows.len() < count {
            buf.clear();
            let read = self.reader.read_until(0, &mut buf).await.map_err(|e| GitError::io("Failed to read git log", e))?;
            if read == 0 {
                self.done = true;
                break;
            }
            if buf.last() == Some(&0) {
                buf.pop();
            }
            if let Some(commit) = parse_commit(&String::from_utf8_lossy(&buf)) {
                rows.push(self.graph.push(commit));
            }
        }
        Ok(LogPage { rows, done: self.done })
    }

    pub async fn close(mut self) {
        let _ = self.child.kill().await;
    }
}

impl Repo {
    fn log_args(&self, query: &LogQuery) -> Result<Vec<String>> {
        let mut args: Vec<String> =
            vec!["log".into(), "-z".into(), "--topo-order".into(), "--decorate=full".into(), FORMAT.into()];
        if let Some(grep) = query.grep.as_deref().filter(|g| !g.is_empty()) {
            args.extend(["--regexp-ignore-case".into(), "--fixed-strings".into(), format!("--grep={grep}")]);
        }
        if let Some(author) = query.author.as_deref().filter(|a| !a.is_empty()) {
            args.push(format!("--author={author}"));
        }
        if let Some(since) = query.since.as_deref().filter(|s| !s.is_empty()) {
            args.push(format!("--since={since}"));
        }
        if let Some(until) = query.until.as_deref().filter(|s| !s.is_empty()) {
            args.push(format!("--until={until}"));
        }
        if query.path.is_some() {
            args.push("--follow".into());
        }
        if query.all {
            args.push("--all".into());
        } else {
            args.push("--end-of-options".into());
            for rev in &query.revs {
                args.push(check_ref(rev)?.to_string());
            }
        }
        if let Some(path) = &query.path {
            args.push("--".into());
            args.push(check_path(path)?.to_string());
        }
        Ok(args)
    }

    /// Memulai `git log` bertahap untuk tampilan Graph.
    pub fn log_stream(&self, query: &LogQuery) -> Result<LogStream> {
        let args = self.log_args(query)?;
        let mut command = Command::new(self.git().info().path);
        command
            .current_dir(self.root())
            .args(["--no-pager", "-c", "color.ui=false", "-c", "core.quotepath=false"])
            .args(&args)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_PAGER", "cat")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let mut child = command.spawn().map_err(|e| GitError::io("Failed to run git log", e))?;
        let stdout = child.stdout.take().expect("stdout dipipa");
        Ok(LogStream { child, reader: BufReader::with_capacity(64 * 1024, stdout), graph: GraphBuilder::new(), done: false })
    }

    /// Daftar commit tanpa graph (dipakai untuk riwayat berkas dan pratinjau).
    pub async fn log(&self, query: &LogQuery, limit: u32) -> Result<Vec<Commit>> {
        let mut args = self.log_args(query)?;
        args.insert(1, format!("--max-count={limit}"));
        let out = self.run(Cmd::new(args).ok_codes(&[0, 128])).await?;
        // Kode 128 muncul pada repositori tanpa commit.
        Ok(parse_commits(&out.stdout))
    }

    /// Metadata, isi pesan, dan daftar berkas sebuah commit.
    pub async fn commit_detail(&self, oid: &str) -> Result<CommitDetail> {
        let oid = check_ref(oid)?;
        let meta = self
            .run(Cmd::new(["show", "-s", "--decorate=full", "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%cn%x1f%ce%x1f%ct%x1f%D%x1f%s%x00%B", "--end-of-options", oid]))
            .await?;
        let text = meta.text();
        let (head, body) = text.split_once('\0').unwrap_or((text.as_str(), ""));
        let commit = parse_commit(head).ok_or_else(|| GitError::invalid(format!("Commit not found: {oid}")))?;
        let files = self.changed_files(&commit.oid, commit.parents.len() > 1).await?;
        Ok(CommitDetail { commit, body: body.trim_end().to_string(), files })
    }

    /// Daftar berkas yang diubah sebuah commit (merge dibandingkan terhadap induk pertama).
    async fn changed_files(&self, oid: &str, merge: bool) -> Result<Vec<CommitFile>> {
        let revs: Vec<String> = if merge { vec![format!("{oid}^1"), oid.to_string()] } else { vec![oid.to_string()] };
        let base = ["diff-tree", "--root", "-r", "--no-commit-id", "-z", "-M", "--no-ext-diff"];
        let raw = self.run(Cmd::new(base).args(["--raw", "--abbrev=40", "--end-of-options"]).args(revs.clone())).await?;
        let stat = self.run(Cmd::new(base).args(["--numstat", "--end-of-options"]).args(revs)).await?;
        Ok(merge_raw_and_numstat(&raw.stdout, &stat.stdout))
    }

    /// Berkas yang berubah antara dua revisi.
    pub async fn diff_files(&self, from: &str, to: &str) -> Result<Vec<CommitFile>> {
        let (from, to) = (check_ref(from)?.to_string(), check_ref(to)?.to_string());
        let raw = self
            .run(Cmd::new(["diff", "--raw", "-z", "-M", "--no-ext-diff", "--abbrev=40", "--end-of-options", &from, &to]))
            .await?;
        let stat = self.run(Cmd::new(["diff", "--numstat", "-z", "-M", "--no-ext-diff", "--end-of-options", &from, &to])).await?;
        Ok(merge_raw_and_numstat(&raw.stdout, &stat.stdout))
    }
}

/// Mengurai `git diff --raw -z`: `:mode mode oid oid STATUS\0path\0[path2\0]`.
pub(crate) fn parse_raw(raw: &[u8]) -> Vec<CommitFile> {
    let mut files = Vec::new();
    let mut parts = raw.split(|b| *b == 0).map(|p| String::from_utf8_lossy(p).into_owned());
    while let Some(meta) = parts.next() {
        if !meta.starts_with(':') {
            continue;
        }
        let status = meta.split(' ').nth(4).unwrap_or("M").to_string();
        let Some(first) = parts.next() else { break };
        let code = status.chars().next().unwrap_or('M');
        let (path, orig_path) = if matches!(code, 'R' | 'C') {
            match parts.next() {
                Some(second) => (second, Some(first)),
                None => (first, None),
            }
        } else {
            (first, None)
        };
        let kind = match code {
            'A' => ChangeKind::Added,
            'D' => ChangeKind::Deleted,
            'R' => ChangeKind::Renamed,
            'C' => ChangeKind::Copied,
            'T' => ChangeKind::TypeChanged,
            _ => ChangeKind::Modified,
        };
        files.push(CommitFile { path, orig_path, kind, additions: None, deletions: None });
    }
    files
}

/// Mengurai `git diff --numstat -z` menjadi (path, tambah, hapus).
pub(crate) fn parse_numstat(raw: &[u8]) -> Vec<(String, Option<u32>, Option<u32>)> {
    let mut stats = Vec::new();
    let mut parts = raw.split(|b| *b == 0).map(|p| String::from_utf8_lossy(p).into_owned());
    while let Some(entry) = parts.next() {
        let mut fields = entry.splitn(3, '\t');
        let (Some(add), Some(del), Some(path)) = (fields.next(), fields.next(), fields.next()) else { continue };
        let (add, del) = (add.parse().ok(), del.parse().ok());
        if path.is_empty() {
            // Rename: dua rekaman berikutnya adalah path lama dan path baru.
            let _old = parts.next();
            if let Some(new) = parts.next() {
                stats.push((new, add, del));
            }
        } else {
            stats.push((path.to_string(), add, del));
        }
    }
    stats
}

pub(crate) fn merge_raw_and_numstat(raw: &[u8], numstat: &[u8]) -> Vec<CommitFile> {
    let mut files = parse_raw(raw);
    for (path, add, del) in parse_numstat(numstat) {
        if let Some(file) = files.iter_mut().find(|f| f.path == path) {
            file.additions = add;
            file.deletions = del;
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commit_record() {
        let record = "abc123\x1fp1 p2\x1fAna\x1fana@x.id\x1f1700000000\x1fBudi\x1fbudi@x.id\x1f1700000100\x1fHEAD -> refs/heads/main, tag: refs/tags/v1.0, refs/remotes/origin/main, refs/remotes/origin/HEAD\x1fSubjek dengan \x1f pemisah";
        let c = parse_commit(record).unwrap();
        assert_eq!(c.oid, "abc123");
        assert_eq!(c.parents, vec!["p1", "p2"]);
        assert_eq!(c.author.name, "Ana");
        assert_eq!(c.author.time, 1_700_000_000.0);
        assert_eq!(c.committer.email, "budi@x.id");
        assert_eq!(c.subject, "Subjek dengan \x1f pemisah");
        assert_eq!(
            c.refs.iter().map(|r| (r.kind, r.name.as_str())).collect::<Vec<_>>(),
            vec![
                (RefKind::Head, "HEAD"),
                (RefKind::Branch, "main"),
                (RefKind::Tag, "v1.0"),
                (RefKind::Remote, "origin/main")
            ]
        );
    }

    #[test]
    fn parses_root_commit_and_detached_head() {
        let c = parse_commit("abc\x1f\x1fA\x1fa@x\x1f1\x1fA\x1fa@x\x1f1\x1fHEAD\x1froot").unwrap();
        assert!(c.parents.is_empty());
        assert_eq!(c.refs, vec![RefLabel { kind: RefKind::Head, name: "HEAD".into() }]);
    }

    #[test]
    fn parses_raw_with_rename() {
        let raw = b":100644 100644 aaa bbb M\0src/a.rs\0:100644 100644 ccc ddd R090\0old.rs\0new.rs\0:000000 100644 000 eee A\0added file.txt\0";
        let files = parse_raw(raw);
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].kind, ChangeKind::Modified);
        assert_eq!((files[1].path.as_str(), files[1].orig_path.as_deref()), ("new.rs", Some("old.rs")));
        assert_eq!(files[2].path, "added file.txt");
    }

    #[test]
    fn parses_numstat_with_rename_and_binary() {
        let raw = b"3\t1\tsrc/a.rs\x005\t2\t\0old.rs\0new.rs\0-\t-\timg.png\0";
        let stats = parse_numstat(raw);
        assert_eq!(stats, vec![("src/a.rs".to_string(), Some(3), Some(1)), ("new.rs".to_string(), Some(5), Some(2)), ("img.png".to_string(), None, None)]);
    }
}
