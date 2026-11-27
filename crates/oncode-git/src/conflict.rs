//! Conflict resolution: the three versions from the index are re-merged with `git merge-file`
//! and then split into segments for the conflict editor. If the file in the working tree was already
//! edited (conflicts resolved by hand, by rerere or by a merge driver), the segments come from its
//! conflict markers instead, so that work is kept.
//!
//! Text is decoded the way the editor reads files: a byte order mark, then UTF-8, then a guess.

use encoding_rs::{Encoding, UTF_8};

use crate::content::is_binary;
use crate::error::{GitError, Result};
use crate::model::*;
use crate::repo::Repo;
use crate::runner::{check_path, Cmd};
use crate::status::detect_operation;

/// Splits `git merge-file --diff3` output whose markers are `marker` characters long.
pub fn parse_merge_output(output: &[u8], marker: usize) -> Vec<MergeSegment> {
    parse_merge_text(&String::from_utf8_lossy(output), marker)
}

/// Splits text with conflict markers that are `marker` characters long (git writes 7 unless the
/// `conflict-marker-size` attribute says otherwise). The base section (`|||||||`) is optional.
pub fn parse_merge_text(output: &str, marker: usize) -> Vec<MergeSegment> {
    let output = output.as_bytes();
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

/// A marker length that does not collide with the content of any file.
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

/// The encoding of a conflicted file, judged from all of its versions: a byte order mark, then UTF-8 if
/// every version is valid UTF-8, otherwise a guess from the bytes (as the editor does when it opens a file).
fn detect_encoding(sides: &[&[u8]]) -> (&'static Encoding, bool) {
    if let Some((encoding, _)) = sides.first().and_then(|bytes| Encoding::for_bom(bytes)) {
        return (encoding, true);
    }
    if sides.iter().all(|bytes| std::str::from_utf8(bytes).is_ok()) {
        return (UTF_8, false);
    }
    let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    for bytes in sides.iter().filter(|bytes| std::str::from_utf8(bytes).is_err()) {
        detector.feed(bytes, false);
    }
    detector.feed(b"", true);
    (detector.guess(None, chardetng::Utf8Detection::Deny), false)
}

/// Decodes `bytes` as `encoding`, without the byte order mark.
fn decode_as(encoding: &'static Encoding, bytes: &[u8]) -> String {
    let bytes = Encoding::for_bom(bytes).map_or(bytes, |(_, length)| &bytes[length..]);
    encoding.decode_without_bom_handling(bytes).0.into_owned()
}

/// More lines end with `\r\n` than with a bare `\n`.
fn mostly_crlf(text: &str) -> bool {
    let crlf = text.matches("\r\n").count();
    crlf > text.matches('\n').count() - crlf
}

/// Nothing in `marked` (the working tree) was resolved yet: taking the current side of every conflict gives
/// the same text in both, and so does taking the incoming side (line endings aside). The conflicts
/// themselves may be cut differently: git joins conflicts that are close together unless
/// `merge.conflictStyle` is diff3, and leaves the base out.
fn same_conflicts(fresh: &[MergeSegment], marked: &[MergeSegment]) -> bool {
    let side = |segments: &[MergeSegment], current_side: bool| -> String {
        let text: String = segments
            .iter()
            .map(|segment| match segment {
                MergeSegment::Resolved { text } => text.as_str(),
                MergeSegment::Conflict { current, incoming, .. } => if current_side { current.as_str() } else { incoming.as_str() },
            })
            .collect();
        text.replace("\r\n", "\n")
    };
    side(fresh, true) == side(marked, true) && side(fresh, false) == side(marked, false)
}

impl Repo {
    /// Labels for both sides of the conflict, using the actual branch/commit names.
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
            // During a rebase, the "ours" side is the new base and "theirs" is the commit from the branch being rebased.
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
            Some(Operation::Bisect { .. }) | None => (head_label, "incoming".to_string()),
        }
    }

    /// The three versions of a conflicted file together with the result of re-merging them, or with the
    /// conflicts still marked in the working tree if that file was already edited.
    pub async fn conflict_detail(&self, path: &str) -> Result<ConflictDetail> {
        let path = check_path(path)?;
        let base = self.blob(&format!(":1:{path}")).await?;
        let current = self.blob(&format!(":2:{path}")).await?;
        let incoming = self.blob(&format!(":3:{path}")).await?;
        if base.is_none() && current.is_none() && incoming.is_none() {
            return Err(GitError::invalid(format!("{path} is not in conflict")));
        }
        let kind = kind_from_stages(base.is_some(), current.is_some(), incoming.is_some());
        let tree = tokio::fs::read(self.root().join(path)).await.ok().filter(|bytes| !is_binary(bytes));
        let binary = [&base, &current, &incoming].iter().any(|side| side.as_deref().is_some_and(is_binary));
        let (current_label, incoming_label) = self.conflict_labels().await;

        let sides: Vec<&[u8]> = [&tree, &current, &incoming, &base].into_iter().flatten().map(Vec::as_slice).collect();
        let (encoding, bom) = detect_encoding(&sides);
        let decode = |bytes: &[u8]| decode_as(encoding, bytes);
        let worktree = tree.as_deref().filter(|_| !binary).map(decode);
        let reference = worktree.clone().or_else(|| current.as_deref().map(decode)).unwrap_or_default();
        let format = TextFormat { encoding: encoding.name().to_string(), bom, crlf: mostly_crlf(&reference) };

        let mut segments = Vec::new();
        let mut from_worktree = false;
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
                segments = parse_merge_text(&decode(&out.stdout), marker);
                if let Some(tree) = &worktree {
                    let marked = parse_merge_text(tree, self.conflict_marker_size(path).await);
                    if !same_conflicts(&segments, &marked) {
                        segments = if marked.is_empty() { vec![MergeSegment::Resolved { text: String::new() }] } else { marked };
                        from_worktree = true;
                    }
                }
            }
        }

        let text = |side: &Option<Vec<u8>>| side.as_deref().filter(|_| !binary).map(decode);
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
            format,
            worktree,
            from_worktree,
        })
    }

    /// The file in the working tree decoded as `encoding` (a WHATWG label); `None` if it is missing or binary.
    pub async fn worktree_text(&self, path: &str, encoding: &str) -> Result<Option<String>> {
        let path = check_path(path)?;
        let encoding = Encoding::for_label(encoding.as_bytes()).unwrap_or(UTF_8);
        Ok(tokio::fs::read(self.root().join(path)).await.ok().filter(|bytes| !is_binary(bytes)).map(|bytes| decode_as(encoding, &bytes)))
    }

    /// The length of the conflict markers git writes for `path`: 7, unless the `conflict-marker-size`
    /// attribute sets another.
    async fn conflict_marker_size(&self, path: &str) -> usize {
        let out = self.run(Cmd::new(["check-attr", "conflict-marker-size"]).paths([path])).await;
        out.ok().and_then(|out| out.trimmed().rsplit(": ").next().and_then(|size| size.parse().ok())).filter(|size| *size > 0).unwrap_or(7)
    }

    /// Resolves a conflict by taking one side entirely. If that side
    /// deleted the file, the file is deleted.
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

    /// Marks a conflict as resolved using the file's current working tree content.
    pub async fn mark_resolved(&self, path: &str) -> Result<()> {
        let path = check_path(path)?;
        let _guard = self.lock().await;
        self.run(Cmd::new(["add"]).paths([path]).write()).await.map(drop)
    }

    /// Resolves a conflict by deleting the file.
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
