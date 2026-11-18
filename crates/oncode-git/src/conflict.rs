//! Resolusi konflik: tiga versi dari index digabung ulang dengan `git merge-file`
//! lalu dipecah menjadi segmen untuk editor konflik.

use crate::content::is_binary;
use crate::error::{GitError, Result};
use crate::model::*;
use crate::repo::Repo;
use crate::runner::{check_path, Cmd};
use crate::status::detect_operation;

/// Memecah keluaran `git merge-file --diff3` dengan penanda sepanjang `marker` karakter.
pub fn parse_merge_output(output: &[u8], marker: usize) -> Vec<MergeSegment> {
    #[derive(PartialEq)]
    enum State {
        Resolved,
        Current,
        Base,
        Incoming,
    }
    let starts = |line: &[u8], ch: u8| line.len() >= marker && line[..marker].iter().all(|b| *b == ch) && line.get(marker).is_none_or(|b| matches!(b, b' ' | b'\n' | b'\r'));

    let mut segments = Vec::new();
    let mut state = State::Resolved;
    let (mut resolved, mut current, mut base, mut incoming) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();

    for line in output.split_inclusive(|b| *b == b'\n') {
        match state {
            State::Resolved if starts(line, b'<') => {
                if !resolved.is_empty() {
                    segments.push(MergeSegment::Resolved { text: text(&resolved) });
                    resolved.clear();
                }
                state = State::Current;
            }
            State::Resolved => resolved.extend_from_slice(line),
            State::Current if starts(line, b'|') => state = State::Base,
            State::Current if starts(line, b'=') => state = State::Incoming,
            State::Current => current.extend_from_slice(line),
            State::Base if starts(line, b'=') => state = State::Incoming,
            State::Base => base.extend_from_slice(line),
            State::Incoming if starts(line, b'>') => {
                segments.push(MergeSegment::Conflict { current: text(&current), base: text(&base), incoming: text(&incoming) });
                current.clear();
                base.clear();
                incoming.clear();
                state = State::Resolved;
            }
            State::Incoming => incoming.extend_from_slice(line),
        }
    }
    if !resolved.is_empty() {
        segments.push(MergeSegment::Resolved { text: text(&resolved) });
    }
    segments
}

/// Panjang penanda yang tidak bertabrakan dengan isi berkas mana pun.
fn pick_marker_size(inputs: &[&[u8]]) -> usize {
    let mut size = 16;
    loop {
        let collides = inputs.iter().any(|data| {
            data.split(|b| *b == b'\n').any(|line| {
                line.len() >= size && matches!(line[0], b'<' | b'=' | b'>' | b'|') && line[..size].iter().all(|b| *b == line[0])
            })
        });
        if !collides {
            return size;
        }
        size += 8;
    }
}

fn kind_from_stages(base: bool, ours: bool, theirs: bool) -> ConflictKind {
    match (base, ours, theirs) {
        (true, true, true) => ConflictKind::BothModified,
        (false, true, true) => ConflictKind::BothAdded,
        (true, false, false) => ConflictKind::BothDeleted,
        (true, true, false) => ConflictKind::DeletedByThem,
        (true, false, true) => ConflictKind::DeletedByUs,
        (false, true, false) => ConflictKind::AddedByUs,
        (false, false, true) => ConflictKind::AddedByThem,
        (false, false, false) => ConflictKind::BothModified,
    }
}

impl Repo {
    /// Label kedua sisi konflik memakai nama branch/commit yang sebenarnya.
    async fn conflict_labels(&self) -> (String, String) {
        let head = self
            .run(Cmd::new(["rev-parse", "--abbrev-ref", "HEAD"]))
            .await
            .map(|o| o.trimmed())
            .ok()
            .filter(|h| !h.is_empty() && h != "HEAD");
        let head_label = head.unwrap_or_else(|| "HEAD".to_string());
        match detect_operation(self.git_dir()) {
            Some(Operation::Merge { incoming }) => (head_label, incoming),
            // Saat rebase, sisi "ours" adalah dasar baru dan "theirs" adalah commit milik branch yang sedang di-rebase.
            Some(Operation::Rebase { onto, head_name, stopped_at, .. }) => {
                let incoming = match stopped_at {
                    Some(sha) if !head_name.is_empty() => format!("{head_name} ({sha})"),
                    Some(sha) => sha,
                    None => head_name,
                };
                (format!("new base ({onto})"), incoming)
            }
            Some(Operation::CherryPick { commit }) => (head_label, format!("cherry-pick {commit}")),
            Some(Operation::Revert { commit }) => (head_label, format!("revert {commit}")),
            None => (head_label, "incoming".to_string()),
        }
    }

    /// Tiga versi berkas konflik beserta hasil penggabungan ulangnya.
    pub async fn conflict_detail(&self, path: &str) -> Result<ConflictDetail> {
        let path = check_path(path)?;
        let base = self.blob(&format!(":1:{path}")).await?;
        let current = self.blob(&format!(":2:{path}")).await?;
        let incoming = self.blob(&format!(":3:{path}")).await?;
        if base.is_none() && current.is_none() && incoming.is_none() {
            return Err(GitError::invalid(format!("{path} is not in conflict")));
        }
        let kind = kind_from_stages(base.is_some(), current.is_some(), incoming.is_some());
        let binary = [&base, &current, &incoming].iter().any(|side| side.as_deref().is_some_and(is_binary));
        let (current_label, incoming_label) = self.conflict_labels().await;

        let mut segments = Vec::new();
        if !binary {
            if let (Some(cur), Some(inc)) = (&current, &incoming) {
                let base_bytes = base.clone().unwrap_or_default();
                let marker = pick_marker_size(&[cur, &base_bytes, inc]);
                let dir = tempfile::tempdir().map_err(|e| GitError::io("Failed to create temporary directory", e))?;
                let files = [("current", cur.as_slice()), ("base", base_bytes.as_slice()), ("incoming", inc.as_slice())];
                for (name, data) in files {
                    tokio::fs::write(dir.path().join(name), data).await.map_err(|e| GitError::io("Failed to write temporary file", e))?;
                }
                let out = self
                    .run_raw(
                        Cmd::new(["merge-file", "-p", "--diff3", &format!("--marker-size={marker}")])
                            .arg(dir.path().join("current").into_os_string())
                            .arg(dir.path().join("base").into_os_string())
                            .arg(dir.path().join("incoming").into_os_string()),
                    )
                    .await?;
                if out.code < 0 || out.code > 127 {
                    return Err(GitError::from_output(&["merge-file".into()], out.code, "", &out.stderr));
                }
                segments = parse_merge_output(&out.stdout, marker);
            }
        }

        let text = |side: &Option<Vec<u8>>| side.as_ref().filter(|_| !binary).map(|b| String::from_utf8_lossy(b).into_owned());
        Ok(ConflictDetail {
            path: path.to_string(),
            kind,
            binary,
            current_label,
            incoming_label,
            base: text(&base),
            current: text(&current),
            incoming: text(&incoming),
            segments,
        })
    }

    /// Menyelesaikan konflik dengan memakai satu sisi seutuhnya. Bila sisi itu
    /// menghapus berkas, berkasnya dihapus.
    pub async fn resolve_with_side(&self, path: &str, side: ConflictSide) -> Result<()> {
        let path = check_path(path)?;
        let (stage, flag) = match side {
            ConflictSide::Current => (2, "--ours"),
            ConflictSide::Incoming => (3, "--theirs"),
        };
        let exists = self.blob(&format!(":{stage}:{path}")).await?.is_some();
        let _guard = self.lock().await;
        if exists {
            self.run(Cmd::new(["checkout", flag]).paths([path]).write()).await?;
            self.run(Cmd::new(["add"]).paths([path]).write()).await.map(drop)
        } else {
            self.run(Cmd::new(["rm", "-f", "-q"]).paths([path]).write()).await.map(drop)
        }
    }

    /// Menandai konflik selesai dengan isi berkas di working tree saat ini.
    pub async fn mark_resolved(&self, path: &str) -> Result<()> {
        let path = check_path(path)?;
        let _guard = self.lock().await;
        self.run(Cmd::new(["add"]).paths([path]).write()).await.map(drop)
    }

    /// Menyelesaikan konflik dengan menghapus berkas.
    pub async fn resolve_by_deleting(&self, path: &str) -> Result<()> {
        let path = check_path(path)?;
        let _guard = self.lock().await;
        self.run(Cmd::new(["rm", "-f", "-q"]).paths([path]).write()).await.map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_diff3_output_into_segments() {
        let m = 16;
        let (lt, bar, eq, gt) = ("<".repeat(m), "|".repeat(m), "=".repeat(m), ">".repeat(m));
        let output = format!("top\n{lt} current\nours\n{bar} base\norig\n{eq}\ntheirs\n{gt} incoming\nbottom\n");
        let segments = parse_merge_output(output.as_bytes(), m);
        assert_eq!(
            segments,
            vec![
                MergeSegment::Resolved { text: "top\n".into() },
                MergeSegment::Conflict { current: "ours\n".into(), base: "orig\n".into(), incoming: "theirs\n".into() },
                MergeSegment::Resolved { text: "bottom\n".into() },
            ]
        );
    }

    #[test]
    fn ignores_shorter_marker_lookalikes_in_content() {
        let m = 16;
        let output = format!("<<<<<<< not a marker\n=======\n{} a\nx\n{}\ny\n{} b\n", "<".repeat(m), "=".repeat(m), ">".repeat(m));
        let segments = parse_merge_output(output.as_bytes(), m);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0], MergeSegment::Resolved { text: "<<<<<<< not a marker\n=======\n".into() });
        assert_eq!(segments[1], MergeSegment::Conflict { current: "x\n".into(), base: String::new(), incoming: "y\n".into() });
    }

    #[test]
    fn marker_size_grows_on_collision() {
        let long = "=".repeat(20);
        assert_eq!(pick_marker_size(&[b"plain\n", b"", b"x"]), 16);
        assert_eq!(pick_marker_size(&[long.as_bytes()]), 24);
    }
}
