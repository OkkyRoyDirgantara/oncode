//! Repository status: parser for `git status --porcelain=v2 -z` and detection of unfinished operations.

use std::path::Path;

use crate::error::Result;
use crate::model::*;
use crate::repo::Repo;
use crate::runner::Cmd;

#[derive(Debug, Default, PartialEq)]
pub struct ParsedStatus {
    pub oid: Option<String>,
    pub head: Option<String>,
    pub detached: bool,
    pub upstream: Option<String>,
    pub ahead_behind: Option<(u32, u32)>,
    pub stash_count: u32,
    pub staged: Vec<FileChange>,
    pub unstaged: Vec<FileChange>,
    pub untracked: Vec<String>,
    pub conflicted: Vec<ConflictEntry>,
}

fn change_kind(code: u8) -> Option<ChangeKind> {
    Some(match code {
        b'M' => ChangeKind::Modified,
        b'T' => ChangeKind::TypeChanged,
        b'A' => ChangeKind::Added,
        b'D' => ChangeKind::Deleted,
        b'R' => ChangeKind::Renamed,
        b'C' => ChangeKind::Copied,
        _ => return None,
    })
}

fn submodule_state(field: &str) -> Option<SubmoduleState> {
    let bytes = field.as_bytes();
    if bytes.first() != Some(&b'S') || bytes.len() < 4 {
        return None;
    }
    Some(SubmoduleState { commit_changed: bytes[1] == b'C', modified: bytes[2] == b'M', untracked: bytes[3] == b'U' })
}

fn conflict_kind(xy: &str) -> ConflictKind {
    match xy {
        "DD" => ConflictKind::BothDeleted,
        "AU" => ConflictKind::AddedByUs,
        "UD" => ConflictKind::DeletedByThem,
        "UA" => ConflictKind::AddedByThem,
        "DU" => ConflictKind::DeletedByUs,
        "AA" => ConflictKind::BothAdded,
        _ => ConflictKind::BothModified,
    }
}

/// Parses the output of `git status --porcelain=v2 -z --branch --show-stash`.
pub fn parse_status(raw: &[u8]) -> ParsedStatus {
    let mut status = ParsedStatus::default();
    let mut records = raw.split(|b| *b == 0).filter(|r| !r.is_empty());

    while let Some(record) = records.next() {
        let text = String::from_utf8_lossy(record);
        let Some((tag, rest)) = text.split_once(' ') else { continue };
        match tag {
            "#" => {
                let (key, value) = rest.split_once(' ').unwrap_or((rest, ""));
                match key {
                    "branch.oid" => status.oid = (value != "(initial)").then(|| value.to_string()),
                    "branch.head" => {
                        if value == "(detached)" {
                            status.detached = true;
                        } else {
                            status.head = Some(value.to_string());
                        }
                    }
                    "branch.upstream" => status.upstream = Some(value.to_string()),
                    "branch.ab" => {
                        let mut parts = value.split(' ');
                        let ahead = parts.next().and_then(|p| p.trim_start_matches('+').parse().ok());
                        let behind = parts.next().and_then(|p| p.trim_start_matches('-').parse().ok());
                        if let (Some(a), Some(b)) = (ahead, behind) {
                            status.ahead_behind = Some((a, b));
                        }
                    }
                    "stash" => status.stash_count = value.parse().unwrap_or(0),
                    _ => {}
                }
            }
            "1" | "2" => {
                // 1 XY sub mH mI mW hH hI path
                // 2 XY sub mH mI mW hH hI Xscore path  (then the next record = original path)
                let field_count = if tag == "1" { 8 } else { 9 };
                let mut fields = rest.splitn(field_count, ' ');
                let xy = fields.next().unwrap_or("..").as_bytes().to_vec();
                let sub = fields.next().unwrap_or("N...").to_string();
                let path = fields.last().unwrap_or("").to_string();
                let orig_path = if tag == "2" {
                    records.next().map(|r| String::from_utf8_lossy(r).into_owned())
                } else {
                    None
                };
                let submodule = submodule_state(&sub);
                let (x, y) = (xy.first().copied().unwrap_or(b'.'), xy.get(1).copied().unwrap_or(b'.'));
                if let Some(kind) = change_kind(x) {
                    let renamed = matches!(kind, ChangeKind::Renamed | ChangeKind::Copied);
                    status.staged.push(FileChange {
                        path: path.clone(),
                        orig_path: if renamed { orig_path.clone() } else { None },
                        kind,
                        submodule: submodule.clone(),
                    });
                }
                if let Some(kind) = change_kind(y) {
                    let renamed = matches!(kind, ChangeKind::Renamed | ChangeKind::Copied);
                    status.unstaged.push(FileChange {
                        path,
                        orig_path: if renamed { orig_path } else { None },
                        kind,
                        submodule,
                    });
                }
            }
            "u" => {
                // u XY sub m1 m2 m3 mW h1 h2 h3 path
                let mut fields = rest.splitn(10, ' ');
                let xy = fields.next().unwrap_or("UU").to_string();
                let path = fields.last().unwrap_or("").to_string();
                status.conflicted.push(ConflictEntry { path, kind: conflict_kind(&xy) });
            }
            "?" => status.untracked.push(rest.to_string()),
            _ => {}
        }
    }
    status
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Modern Git always creates the `interactive` marker, so a rebase is considered interactive
/// if its step list contains commands other than `pick`.
fn has_non_pick_steps(base: &Path) -> bool {
    ["done", "git-rebase-todo"].iter().any(|name| {
        std::fs::read_to_string(base.join(name)).is_ok_and(|text| {
            text.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .any(|l| !matches!(l.split_whitespace().next(), Some("pick" | "p")))
        })
    })
}

fn short(oid: &str) -> String {
    oid.chars().take(8).collect()
}

/// Detects an unfinished merge, rebase, cherry-pick, or revert from the contents of the git directory.
pub fn detect_operation(git_dir: &Path) -> Option<Operation> {
    for (dir, interactive_default) in [("rebase-merge", true), ("rebase-apply", false)] {
        let base = git_dir.join(dir);
        if !base.is_dir() {
            continue;
        }
        // `rebase-apply` is also used by `git am`; only a rebase has an `onto` file.
        let Some(onto) = read_trimmed(&base.join("onto")) else { continue };
        let number = |name: &str| read_trimmed(&base.join(name)).and_then(|s| s.parse::<u32>().ok());
        let (step, total) = if dir == "rebase-merge" {
            (number("msgnum").unwrap_or(0), number("end").unwrap_or(0))
        } else {
            (number("next").unwrap_or(0), number("last").unwrap_or(0))
        };
        let head_name = read_trimmed(&base.join("head-name"))
            .map(|n| n.trim_start_matches("refs/heads/").to_string())
            .unwrap_or_default();
        return Some(Operation::Rebase {
            interactive: interactive_default && has_non_pick_steps(&base),
            step,
            total,
            onto: short(&onto),
            head_name,
            stopped_at: read_trimmed(&base.join("stopped-sha")).map(|s| short(&s)),
        });
    }
    if let Some(head) = read_trimmed(&git_dir.join("MERGE_HEAD")) {
        let incoming = read_trimmed(&git_dir.join("MERGE_MSG"))
            .and_then(|msg| {
                let first = msg.lines().next()?.to_string();
                let start = first.find('\'')? + 1;
                let end = first[start..].find('\'')? + start;
                Some(first[start..end].to_string())
            })
            .unwrap_or_else(|| short(head.lines().next().unwrap_or("")));
        return Some(Operation::Merge { incoming });
    }
    if let Some(head) = read_trimmed(&git_dir.join("CHERRY_PICK_HEAD")) {
        return Some(Operation::CherryPick { commit: short(&head) });
    }
    if let Some(head) = read_trimmed(&git_dir.join("REVERT_HEAD")) {
        return Some(Operation::Revert { commit: short(&head) });
    }
    // Last: a cherry-pick (or merge) can stop in the middle of a bisect, and it is the one to finish first.
    if let Some(start) = read_trimmed(&git_dir.join("BISECT_START")) {
        let original = start.trim_start_matches("refs/heads/").to_string();
        let original = if original.len() == 40 && original.chars().all(|c| c.is_ascii_hexdigit()) { short(&original) } else { original };
        let first_bad = read_trimmed(&git_dir.join("BISECT_LOG")).and_then(|log| {
            let line = log.lines().rev().find_map(|line| line.strip_prefix("# first bad commit: "))?.to_string();
            // `[<full id>] <subject>`
            let (id, subject) = line.strip_prefix('[')?.split_once("] ")?;
            Some(format!("{} {subject}", short(id)))
        });
        return Some(Operation::Bisect { original, first_bad });
    }
    None
}

impl Repo {
    /// A complete picture of the repository's current state.
    pub async fn snapshot(&self) -> Result<RepoSnapshot> {
        let out = self
            .run(Cmd::new([
                "status",
                "--porcelain=v2",
                "-z",
                "--branch",
                "--show-stash",
                "--untracked-files=all",
            ]))
            .await?;
        let parsed = parse_status(&out.stdout);

        let head = if parsed.detached {
            Head::Detached { oid: parsed.oid.clone().unwrap_or_default() }
        } else {
            let name = parsed.head.clone().unwrap_or_default();
            if parsed.oid.is_none() {
                Head::Unborn { name }
            } else {
                Head::Branch { name }
            }
        };
        let upstream = parsed.upstream.clone().map(|name| {
            let (ahead, behind) = parsed.ahead_behind.unwrap_or((0, 0));
            Upstream { name, ahead, behind, exists: parsed.ahead_behind.is_some() }
        });

        Ok(RepoSnapshot {
            root: self.root_string(),
            head,
            head_oid: parsed.oid,
            upstream,
            operation: detect_operation(self.git_dir()),
            staged: parsed.staged,
            unstaged: parsed.unstaged,
            untracked: parsed.untracked,
            conflicted: parsed.conflicted,
            stash_count: parsed.stash_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(parts: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for p in parts {
            out.extend_from_slice(p.as_bytes());
            out.push(0);
        }
        out
    }

    #[test]
    fn parses_branch_headers() {
        let raw = rec(&[
            "# branch.oid 1234567890abcdef",
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +2 -1",
            "# stash 3",
        ]);
        let s = parse_status(&raw);
        assert_eq!(s.oid.as_deref(), Some("1234567890abcdef"));
        assert_eq!(s.head.as_deref(), Some("main"));
        assert_eq!(s.upstream.as_deref(), Some("origin/main"));
        assert_eq!(s.ahead_behind, Some((2, 1)));
        assert_eq!(s.stash_count, 3);
    }

    #[test]
    fn parses_initial_and_detached() {
        let s = parse_status(&rec(&["# branch.oid (initial)", "# branch.head main"]));
        assert_eq!(s.oid, None);
        assert!(!s.detached);
        let s = parse_status(&rec(&["# branch.oid abc", "# branch.head (detached)"]));
        assert!(s.detached);
        assert_eq!(s.head, None);
    }

    #[test]
    fn parses_ordinary_entries_with_spaces() {
        let raw = rec(&[
            "1 M. N... 100644 100644 100644 aaa bbb src/staged file.rs",
            "1 .M N... 100644 100644 100644 aaa aaa src/unstaged.rs",
            "1 MM N... 100644 100644 100644 aaa bbb both.rs",
            "1 A. N... 000000 100644 100644 000 bbb new.rs",
            "1 .D N... 100644 100644 000000 aaa aaa gone.rs",
            "? untracked dir/file.txt",
        ]);
        let s = parse_status(&raw);
        assert_eq!(
            s.staged.iter().map(|f| (f.path.as_str(), f.kind)).collect::<Vec<_>>(),
            vec![
                ("src/staged file.rs", ChangeKind::Modified),
                ("both.rs", ChangeKind::Modified),
                ("new.rs", ChangeKind::Added)
            ]
        );
        assert_eq!(
            s.unstaged.iter().map(|f| (f.path.as_str(), f.kind)).collect::<Vec<_>>(),
            vec![
                ("src/unstaged.rs", ChangeKind::Modified),
                ("both.rs", ChangeKind::Modified),
                ("gone.rs", ChangeKind::Deleted)
            ]
        );
        assert_eq!(s.untracked, vec!["untracked dir/file.txt"]);
    }

    #[test]
    fn parses_rename_with_original_path() {
        let raw = rec(&[
            "2 R. N... 100644 100644 100644 aaa aaa R100 new name.rs",
            "old name.rs",
            "1 .M N... 100644 100644 100644 aaa aaa after.rs",
        ]);
        let s = parse_status(&raw);
        assert_eq!(s.staged.len(), 1);
        assert_eq!(s.staged[0].path, "new name.rs");
        assert_eq!(s.staged[0].orig_path.as_deref(), Some("old name.rs"));
        assert_eq!(s.staged[0].kind, ChangeKind::Renamed);
        assert_eq!(s.unstaged[0].path, "after.rs");
    }

    #[test]
    fn parses_unmerged_and_submodule() {
        let raw = rec(&[
            "u UU N... 100644 100644 100644 100644 a b c conflict file.txt",
            "u DU N... 100644 000000 100644 100644 a b c deleted.txt",
            "1 .M SCM. 160000 160000 160000 aaa aaa vendor/lib",
        ]);
        let s = parse_status(&raw);
        assert_eq!(s.conflicted[0], ConflictEntry { path: "conflict file.txt".into(), kind: ConflictKind::BothModified });
        assert_eq!(s.conflicted[1].kind, ConflictKind::DeletedByUs);
        let sub = s.unstaged[0].submodule.clone().unwrap();
        assert!(sub.commit_changed && sub.modified && !sub.untracked);
    }
}
