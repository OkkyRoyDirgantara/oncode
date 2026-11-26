//! Test helpers for pure logic: fake data builders.

use oncode_git::{Branch, Commit, Edge, EdgeKind, GraphRow, Head, RepoSnapshot, Signature};

pub const ROOT: &str = "/work/repo";

/// A 40-character hash that is easy to recognize: `oid(3)` starts with `0003`.
pub fn oid(n: usize) -> String {
    format!("{n:04x}{}", "a".repeat(36))
}

pub fn commit(n: usize) -> Commit {
    let who = Signature { name: "Ana".into(), email: "ana@example.com".into(), time: 1_700_000_000.0 - n as f64 * 60.0 };
    Commit { oid: oid(n), parents: vec![oid(n + 1)], author: who.clone(), committer: who, subject: format!("Commit {n}"), refs: Vec::new() }
}

pub fn row(n: usize, parents: &[usize]) -> GraphRow {
    let mut commit = commit(n);
    commit.parents = parents.iter().map(|p| oid(*p)).collect();
    GraphRow { commit, column: 0, color: 0, edges: Vec::new() }
}

/// Straight rows in lane 0: commits `from` … `from + count - 1` (the newest first).
pub fn linear_rows(from: usize, count: usize) -> Vec<GraphRow> {
    (0..count)
        .map(|i| {
            let mut edges = Vec::new();
            if from + i > 0 {
                edges.push(Edge { from: 0, to: 0, color: 0, kind: EdgeKind::Top });
            }
            edges.push(Edge { from: 0, to: 0, color: 0, kind: EdgeKind::Bottom });
            GraphRow { commit: commit(from + i), column: 0, color: 0, edges }
        })
        .collect()
}

pub fn branch(name: &str) -> Branch {
    Branch { name: name.into(), oid: oid(0), is_head: false, upstream: None, ahead: 0, behind: 0, upstream_gone: false, worktree: None, subject: String::new(), time: 0.0 }
}

pub fn snapshot() -> RepoSnapshot {
    RepoSnapshot {
        root: ROOT.into(),
        head: Head::Branch { name: "main".into() },
        head_oid: Some(oid(0)),
        upstream: None,
        operation: None,
        staged: Vec::new(),
        unstaged: Vec::new(),
        untracked: Vec::new(),
        conflicted: Vec::new(),
        stash_count: 0,
    }
}
