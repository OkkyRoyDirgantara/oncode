//! Integration tests: each operation runs against a real git repository in a temporary directory.

mod common;

use common::{raw_git, TestRepo};
use oncode_git::*;

fn paths(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn names(changes: &[FileChange]) -> Vec<(&str, ChangeKind)> {
    changes.iter().map(|c| (c.path.as_str(), c.kind)).collect()
}

// ----- Status -----

#[tokio::test]
async fn snapshot_of_empty_repo_is_unborn() {
    let t = TestRepo::new().await;
    let s = t.repo.snapshot().await.unwrap();
    assert_eq!(s.head, Head::Unborn { name: "main".into() });
    assert_eq!(s.head_oid, None);
    assert!(s.staged.is_empty() && s.unstaged.is_empty() && s.untracked.is_empty());
}

#[tokio::test]
async fn snapshot_reports_each_kind_of_change() {
    let t = TestRepo::with_commit().await;
    t.commit_file("old.txt", "content\n", "Add old");
    t.commit_file("delete.txt", "x\n", "Add delete");
    t.write("README.md", "# Project\nchanged\n");
    t.write("new with spaces.txt", "new\n");
    t.write("staged.txt", "staged\n");
    t.git(&["add", "staged.txt"]);
    t.git(&["mv", "old.txt", "new name.txt"]);
    std::fs::remove_file(t.file("delete.txt")).unwrap();

    let s = t.repo.snapshot().await.unwrap();
    assert_eq!(s.head, Head::Branch { name: "main".into() });
    assert_eq!(names(&s.staged), vec![("new name.txt", ChangeKind::Renamed), ("staged.txt", ChangeKind::Added)]);
    assert_eq!(s.staged[0].orig_path.as_deref(), Some("old.txt"));
    assert_eq!(names(&s.unstaged), vec![("README.md", ChangeKind::Modified), ("delete.txt", ChangeKind::Deleted)]);
    assert_eq!(s.untracked, vec!["new with spaces.txt"]);
    assert!(s.operation.is_none());
}

#[tokio::test]
async fn snapshot_handles_non_ascii_names_and_detached_head() {
    let t = TestRepo::with_commit().await;
    let first = t.git(&["rev-parse", "HEAD"]).trim().to_string();
    t.commit_file("notes/résumé ünï.txt", "a\n", "Non-ASCII name");
    t.write("notes/résumé ünï.txt", "b\n");
    let s = t.repo.snapshot().await.unwrap();
    assert_eq!(names(&s.unstaged), vec![("notes/résumé ünï.txt", ChangeKind::Modified)]);

    t.git(&["checkout", "-q", "--", "."]);
    t.repo.switch_detached(&first).await.unwrap();
    let s = t.repo.snapshot().await.unwrap();
    assert_eq!(s.head, Head::Detached { oid: first });
}

// ----- Per-file staging -----

#[tokio::test]
async fn stage_unstage_and_discard_whole_files() {
    let t = TestRepo::with_commit().await;
    t.write("README.md", "# Project\nedited\n");
    t.write("new.txt", "new\n");

    t.repo.stage(&paths(&["README.md", "new.txt"])).await.unwrap();
    let s = t.repo.snapshot().await.unwrap();
    assert_eq!(names(&s.staged), vec![("README.md", ChangeKind::Modified), ("new.txt", ChangeKind::Added)]);
    assert!(s.unstaged.is_empty() && s.untracked.is_empty());

    t.repo.unstage(&paths(&["new.txt"])).await.unwrap();
    let s = t.repo.snapshot().await.unwrap();
    assert_eq!(names(&s.staged), vec![("README.md", ChangeKind::Modified)]);
    assert_eq!(s.untracked, vec!["new.txt"]);

    t.repo.unstage_all().await.unwrap();
    t.repo.discard(&paths(&["README.md"])).await.unwrap();
    assert_eq!(t.read("README.md"), "# Project\n");
    // Untracked files are not touched by discard.
    assert_eq!(t.read("new.txt"), "new\n");
}

/// `[id]` in a path (Next.js, SvelteKit routes) is part of the name: git would read it as a glob that
/// also matches `i` and `d`, so acting on one file would touch its neighbours.
#[tokio::test]
async fn paths_with_glob_characters_only_touch_that_file() {
    let t = TestRepo::with_commit().await;
    let files = ["app/[id]/page.tsx", "app/i/page.tsx", "app/d/page.tsx"];
    for file in files {
        t.write(file, "base\n");
    }
    t.git(&["add", "-A"]);
    t.git(&["commit", "-q", "-m", "Routes"]);
    t.commit_file("app/i/page.tsx", "only i\n", "Change i");
    for file in files {
        t.write(file, &format!("{}edited\n", t.read(file)));
    }
    let target = paths(&["app/[id]/page.tsx"]);

    t.repo.stage(&target).await.unwrap();
    let s = t.repo.snapshot().await.unwrap();
    assert_eq!(names(&s.staged), vec![("app/[id]/page.tsx", ChangeKind::Modified)]);
    t.repo.unstage(&target).await.unwrap();
    assert!(t.repo.snapshot().await.unwrap().staged.is_empty());

    t.repo.discard(&target).await.unwrap();
    assert_eq!(t.read("app/[id]/page.tsx"), "base\n");
    assert_eq!(t.read("app/i/page.tsx"), "only i\nedited\n");
    assert_eq!(t.read("app/d/page.tsx"), "base\nedited\n");

    let history = t.repo.log(&LogQuery { path: Some("app/[id]/page.tsx".into()), ..Default::default() }, 10).await.unwrap();
    assert_eq!(history.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), vec!["Routes"]);
    let mut stream = t.repo.log_stream(&LogQuery { path: Some("app/[id]/page.tsx".into()), ..Default::default() }).unwrap();
    assert_eq!(stream.next_page(10).await.unwrap().rows.len(), 1);
    stream.close().await;
}

#[tokio::test]
async fn unstage_works_before_first_commit() {
    let t = TestRepo::new().await;
    t.write("a.txt", "a\n");
    t.write("b.txt", "b\n");
    t.repo.stage_all().await.unwrap();
    t.repo.unstage(&paths(&["a.txt"])).await.unwrap();
    let s = t.repo.snapshot().await.unwrap();
    assert_eq!(names(&s.staged), vec![("b.txt", ChangeKind::Added)]);
    t.repo.unstage_all().await.unwrap();
    let s = t.repo.snapshot().await.unwrap();
    assert!(s.staged.is_empty());
    assert_eq!(s.untracked, vec!["a.txt", "b.txt"]);
}

#[tokio::test]
async fn paths_outside_the_repo_are_rejected() {
    let t = TestRepo::with_commit().await;
    let err = t.repo.stage(&paths(&["../outside.txt"])).await.unwrap_err();
    assert_eq!(err.code, GitErrorCode::InvalidInput);
    let err = t.repo.switch("--orphan=x").await.unwrap_err();
    assert_eq!(err.code, GitErrorCode::InvalidInput);
}

// ----- Partial staging -----

fn op(a_from: u32, a_to: u32, b_from: u32, b_to: u32) -> SpliceOp {
    SpliceOp { a_from, a_to, b_from, b_to }
}

#[tokio::test]
async fn stage_one_hunk_then_unstage_and_discard_lines() {
    let t = TestRepo::with_commit().await;
    t.commit_file("f.txt", "1\n2\n3\n4\n5\n6\n", "Base");
    t.write("f.txt", "1\nTWO\n3\n4\n5\nSIX\nSEVEN\n");

    // Stage only the first change (line 2).
    let d = t.repo.diff_contents("f.txt", None, &DiffSource::Worktree).await.unwrap();
    assert_eq!(d.a.text.as_deref(), Some("1\n2\n3\n4\n5\n6\n"));
    t.repo.apply_splice(SpliceTarget::Stage, "f.txt", &[op(1, 2, 1, 2)], &d.a.fingerprint, &d.b.fingerprint).await.unwrap();
    assert_eq!(t.index("f.txt"), "1\nTWO\n3\n4\n5\n6\n");
    assert_eq!(t.read("f.txt"), "1\nTWO\n3\n4\n5\nSIX\nSEVEN\n", "working tree must not change");

    // The old fingerprint is now stale.
    let err = t
        .repo
        .apply_splice(SpliceTarget::Stage, "f.txt", &[op(5, 6, 5, 7)], &d.a.fingerprint, &d.b.fingerprint)
        .await
        .unwrap_err();
    assert_eq!(err.code, GitErrorCode::Stale);

    // Unstage that change again.
    let d = t.repo.diff_contents("f.txt", None, &DiffSource::Staged).await.unwrap();
    t.repo.apply_splice(SpliceTarget::Unstage, "f.txt", &[op(1, 2, 1, 2)], &d.a.fingerprint, &d.b.fingerprint).await.unwrap();
    assert_eq!(t.index("f.txt"), "1\n2\n3\n4\n5\n6\n");

    // Discard only the second change from the working tree.
    let d = t.repo.diff_contents("f.txt", None, &DiffSource::Worktree).await.unwrap();
    t.repo.apply_splice(SpliceTarget::Discard, "f.txt", &[op(5, 6, 5, 7)], &d.a.fingerprint, &d.b.fingerprint).await.unwrap();
    assert_eq!(t.read("f.txt"), "1\nTWO\n3\n4\n5\n6\n");
}

#[tokio::test]
async fn stage_part_of_an_untracked_file_and_unstage_it_again() {
    let t = TestRepo::with_commit().await;
    t.write("new.txt", "one\ntwo\nthree\n");

    let d = t.repo.diff_contents("new.txt", None, &DiffSource::Worktree).await.unwrap();
    assert!(!d.a.exists);
    // Only the line "two".
    t.repo.apply_splice(SpliceTarget::Stage, "new.txt", &[op(0, 0, 1, 2)], &d.a.fingerprint, &d.b.fingerprint).await.unwrap();
    assert_eq!(t.index("new.txt"), "two\n");
    let s = t.repo.snapshot().await.unwrap();
    assert_eq!(names(&s.staged), vec![("new.txt", ChangeKind::Added)]);
    assert_eq!(names(&s.unstaged), vec![("new.txt", ChangeKind::Modified)]);

    // Unstaging all of its content makes the file untracked again.
    let d = t.repo.diff_contents("new.txt", None, &DiffSource::Staged).await.unwrap();
    assert!(!d.a.exists);
    t.repo.apply_splice(SpliceTarget::Unstage, "new.txt", &[op(0, 1, 0, 2)], &d.a.fingerprint, &d.b.fingerprint).await.unwrap();
    let s = t.repo.snapshot().await.unwrap();
    assert!(s.staged.is_empty());
    assert_eq!(s.untracked, vec!["new.txt"]);
}

#[tokio::test]
async fn partial_staging_handles_missing_final_newline() {
    let t = TestRepo::with_commit().await;
    t.write("f.txt", "a\nb");
    t.git(&["add", "f.txt"]);
    t.git(&["commit", "-q", "-m", "No trailing newline"]);
    t.write("f.txt", "A\nb\nc\n");

    let d = t.repo.diff_contents("f.txt", None, &DiffSource::Worktree).await.unwrap();
    // Only the addition at the end (line "b" without newline → "b\n" + "c"), not the change to the first line.
    t.repo.apply_splice(SpliceTarget::Stage, "f.txt", &[op(2, 2, 2, 4)], &d.a.fingerprint, &d.b.fingerprint).await.unwrap();
    assert_eq!(t.index("f.txt"), "a\nb\nc\n");
}

#[tokio::test]
async fn partial_staging_keeps_executable_mode() {
    let t = TestRepo::with_commit().await;
    t.write("run.sh", "#!/bin/sh\necho one\n");
    t.git(&["add", "run.sh"]);
    t.git(&["update-index", "--chmod=+x", "run.sh"]);
    t.git(&["commit", "-q", "-m", "Script"]);
    t.write("run.sh", "#!/bin/sh\necho one\necho two\n");
    let d = t.repo.diff_contents("run.sh", None, &DiffSource::Worktree).await.unwrap();
    t.repo.apply_splice(SpliceTarget::Stage, "run.sh", &[op(2, 2, 2, 3)], &d.a.fingerprint, &d.b.fingerprint).await.unwrap();
    assert!(t.git(&["ls-files", "-s", "run.sh"]).starts_with("100755 "));
    assert_eq!(t.index("run.sh"), "#!/bin/sh\necho one\necho two\n");
}

#[cfg(unix)]
#[tokio::test]
async fn discarding_part_of_a_file_keeps_its_permissions_and_leaves_no_temp_file() {
    use std::os::unix::fs::PermissionsExt;
    let t = TestRepo::with_commit().await;
    t.commit_file("run.sh", "#!/bin/sh\necho one\n", "Script");
    std::fs::set_permissions(t.file("run.sh"), std::fs::Permissions::from_mode(0o750)).unwrap();
    t.write("run.sh", "#!/bin/sh\necho ONE\necho two\n");
    let d = t.repo.diff_contents("run.sh", None, &DiffSource::Worktree).await.unwrap();
    // Discard only the added line.
    t.repo.apply_splice(SpliceTarget::Discard, "run.sh", &[op(2, 2, 2, 3)], &d.a.fingerprint, &d.b.fingerprint).await.unwrap();
    assert_eq!(t.read("run.sh"), "#!/bin/sh\necho ONE\n");
    assert_eq!(std::fs::metadata(t.file("run.sh")).unwrap().permissions().mode() & 0o777, 0o750);
    let leftovers: Vec<_> = std::fs::read_dir(t.path()).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).collect();
    assert!(leftovers.is_empty(), "temporary file left behind: {leftovers:?}");
}

#[cfg(unix)]
#[tokio::test]
async fn part_of_a_symlink_cannot_be_discarded() {
    let t = TestRepo::with_commit().await;
    t.write("target.txt", "keep me\n");
    std::os::unix::fs::symlink("target.txt", t.file("link")).unwrap();
    t.git(&["add", "target.txt", "link"]);
    t.git(&["commit", "-q", "-m", "Link"]);
    std::fs::remove_file(t.file("link")).unwrap();
    std::os::unix::fs::symlink("elsewhere.txt", t.file("link")).unwrap();
    let d = t.repo.diff_contents("link", None, &DiffSource::Worktree).await.unwrap();
    let err = t
        .repo
        .apply_splice(SpliceTarget::Discard, "link", &[op(0, 1, 0, 1)], &d.a.fingerprint, &d.b.fingerprint)
        .await
        .unwrap_err();
    assert_eq!(err.code, GitErrorCode::InvalidInput);
    assert_eq!(std::fs::read_link(t.file("link")).unwrap(), std::path::Path::new("elsewhere.txt"));
    assert_eq!(t.read("target.txt"), "keep me\n");
}

#[tokio::test]
async fn binary_files_cannot_be_partially_staged() {
    let t = TestRepo::with_commit().await;
    std::fs::write(t.file("img.bin"), [0u8, 1, 2, 0, 255]).unwrap();
    let d = t.repo.diff_contents("img.bin", None, &DiffSource::Worktree).await.unwrap();
    assert!(d.b.binary && d.b.text.is_none());
    let err = t
        .repo
        .apply_splice(SpliceTarget::Stage, "img.bin", &[op(0, 1, 0, 1)], &d.a.fingerprint, &d.b.fingerprint)
        .await
        .unwrap_err();
    assert_eq!(err.code, GitErrorCode::InvalidInput);
}

// ----- Commits and history -----

#[tokio::test]
async fn commit_amend_and_undo() {
    let t = TestRepo::new().await;
    t.write("a.txt", "a\n");
    t.repo.stage_all().await.unwrap();
    let first = t.repo.commit("First\n\nMessage body.", &CommitOptions::default()).await.unwrap();
    assert_eq!(first.len(), 40);
    assert_eq!(t.repo.last_commit_message().await.unwrap(), "First\n\nMessage body.");

    let err = t.repo.commit("Empty", &CommitOptions::default()).await.unwrap_err();
    assert_eq!(err.code, GitErrorCode::NothingToCommit);
    assert_eq!(t.repo.commit("  ", &CommitOptions::default()).await.unwrap_err().code, GitErrorCode::InvalidInput);

    t.write("b.txt", "b\n");
    t.repo.stage_all().await.unwrap();
    let amended = t.repo.commit("First (amended)", &CommitOptions { amend: true, signoff: true, ..Default::default() }).await.unwrap();
    assert_ne!(amended, first);
    let message = t.repo.last_commit_message().await.unwrap();
    assert!(message.starts_with("First (amended)") && message.contains("Signed-off-by: Tester"));
    assert_eq!(t.git(&["rev-list", "--count", "HEAD"]).trim(), "1");

    t.write("c.txt", "c\n");
    t.repo.stage_all().await.unwrap();
    t.repo.commit("Second", &CommitOptions::default()).await.unwrap();
    t.repo.undo_last_commit().await.unwrap();
    let s = t.repo.snapshot().await.unwrap();
    assert_eq!(s.head_oid.as_deref(), Some(amended.as_str()));
    assert_eq!(names(&s.staged), vec![("c.txt", ChangeKind::Added)]);
}

#[tokio::test]
async fn commit_detail_lists_files_for_root_normal_and_merge_commits() {
    let t = TestRepo::with_commit().await;
    let root = t.git(&["rev-parse", "HEAD"]).trim().to_string();
    let detail = t.repo.commit_detail(&root).await.unwrap();
    assert_eq!(detail.commit.subject, "First commit");
    assert_eq!(detail.files.len(), 1);
    assert_eq!((detail.files[0].path.as_str(), detail.files[0].kind, detail.files[0].additions), ("README.md", ChangeKind::Added, Some(1)));

    t.commit_file("a.txt", "1\n2\n", "Add a");
    t.git(&["mv", "a.txt", "b.txt"]);
    t.write("README.md", "# Project\nline\n");
    t.git(&["add", "-A"]);
    t.git(&["commit", "-q", "-m", "Rename and modify\n\nBody."]);
    let head = t.git(&["rev-parse", "HEAD"]).trim().to_string();
    let detail = t.repo.commit_detail(&head).await.unwrap();
    assert_eq!(detail.body, "Rename and modify\n\nBody.");
    let renamed = detail.files.iter().find(|f| f.path == "b.txt").unwrap();
    assert_eq!((renamed.kind, renamed.orig_path.as_deref()), (ChangeKind::Renamed, Some("a.txt")));
    let readme = detail.files.iter().find(|f| f.path == "README.md").unwrap();
    assert_eq!((readme.additions, readme.deletions), (Some(1), Some(0)));

    // A merge commit is compared against its first parent.
    t.git(&["switch", "-q", "-c", "feature"]);
    t.commit_file("feature.txt", "f\n", "Feature");
    t.git(&["switch", "-q", "main"]);
    t.commit_file("main.txt", "u\n", "Main");
    t.git(&["merge", "-q", "--no-ff", "--no-edit", "feature"]);
    let merge = t.git(&["rev-parse", "HEAD"]).trim().to_string();
    let detail = t.repo.commit_detail(&merge).await.unwrap();
    assert_eq!(detail.commit.parents.len(), 2);
    assert_eq!(detail.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["feature.txt"]);

    let d = t.repo.diff_contents("b.txt", Some("a.txt"), &DiffSource::Commit { oid: head, parent: None }).await.unwrap();
    assert_eq!(d.a.text.as_deref(), Some("1\n2\n"));
    assert_eq!(d.b.text.as_deref(), Some("1\n2\n"));
}

#[tokio::test]
async fn log_stream_pages_through_history_with_graph() {
    let t = TestRepo::with_commit().await;
    for i in 0..7 {
        t.commit_file("n.txt", &format!("{i}\n"), &format!("Commit {i}"));
    }
    t.git(&["switch", "-q", "-c", "side", "HEAD~3"]);
    t.commit_file("side.txt", "s\n", "On side");
    t.git(&["switch", "-q", "main"]);

    let mut stream = t.repo.log_stream(&LogQuery { all: true, ..Default::default() }).unwrap();
    let first = stream.next_page(4).await.unwrap();
    assert_eq!(first.rows.len(), 4);
    assert!(!first.done);
    let rest = stream.next_page(100).await.unwrap();
    assert!(rest.done);
    let all: Vec<_> = first.rows.iter().chain(rest.rows.iter()).collect();
    assert_eq!(all.len(), 9);
    assert!(all.iter().any(|r| r.column == 1), "branch side must be in the second lane");
    let tip = all.iter().find(|r| r.commit.subject == "Commit 6").unwrap();
    assert!(tip.commit.refs.iter().any(|r| r.kind == RefKind::Head));
    assert!(tip.commit.refs.iter().any(|r| r.kind == RefKind::Branch && r.name == "main"));
    stream.close().await;

    let found = t.repo.log(&LogQuery { grep: Some("on SIDE".into()), all: true, ..Default::default() }, 10).await.unwrap();
    assert_eq!(found.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), vec!["On side"]);
    let history = t.repo.log(&LogQuery { path: Some("n.txt".into()), ..Default::default() }, 3).await.unwrap();
    assert_eq!(history.len(), 3);
}

/// A search lists commits whose parents are mostly not listed: following them would open a lane for each
/// and widen the graph with every row, so a filtered list has no lanes at all.
#[tokio::test]
async fn a_filtered_log_has_no_lanes() {
    let t = TestRepo::with_commit().await;
    for i in 0..6 {
        t.commit_file("n.txt", &format!("{i}\n"), &format!("{} {i}", if i % 2 == 0 { "fix" } else { "feat" }));
    }
    let mut stream = t.repo.log_stream(&LogQuery { grep: Some("fix".into()), all: true, ..Default::default() }).unwrap();
    let page = stream.next_page(100).await.unwrap();
    assert_eq!(page.rows.len(), 3);
    assert!(page.rows.iter().all(|row| row.column == 0 && row.edges.is_empty()), "{:?}", page.rows.iter().map(|r| (r.column, r.edges.len())).collect::<Vec<_>>());
    stream.close().await;
}

/// Line history (`git log -L`): only the commits that changed the chosen lines, and the parser keeps just
/// the subject even when an older Git prints the patch anyway.
#[tokio::test]
async fn line_history_lists_only_commits_touching_the_lines() {
    let t = TestRepo::with_commit().await;
    t.commit_file("f.txt", "a\nb\nc\n", "Add f");
    t.commit_file("f.txt", "a\nB\nc\n", "Change line 2");
    t.commit_file("f.txt", "A\nB\nc\n", "Change line 1");
    let query = LogQuery { path: Some("f.txt".into()), lines: Some((2, 2)), ..Default::default() };
    let found = t.repo.log(&query, 10).await.unwrap();
    assert_eq!(found.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["Change line 2", "Add f"]);
    assert!(query.is_filtered());
    let mut stream = t.repo.log_stream(&query).unwrap();
    let page = stream.next_page(10).await.unwrap();
    assert_eq!(page.rows.len(), 2);
    stream.close().await;
}

/// Bisect from start to finish: the snapshot shows it in progress, each mark checks out the next commit,
/// the first bad commit is found and shown, and reset returns to the branch.
#[tokio::test]
async fn bisect_finds_the_first_bad_commit() {
    let t = TestRepo::with_commit().await;
    for i in 1..=6 {
        t.commit_file("n.txt", &format!("{i}\n"), &format!("Step {i}"));
    }
    // "Step 4" introduced the problem (n >= 4).
    let good = t.git(&["rev-parse", "HEAD~5"]).trim().to_string();
    let message = t.repo.bisect_start("HEAD", &good).await.unwrap();
    assert!(message.contains("Bisecting:"), "{message}");
    let snapshot = t.repo.snapshot().await.unwrap();
    assert!(matches!(&snapshot.operation, Some(Operation::Bisect { original, first_bad: None }) if original == "main"), "{:?}", snapshot.operation);
    let labels: Vec<String> = t.repo.log(&LogQuery { all: true, ..Default::default() }, 20).await.unwrap().iter().flat_map(|c| c.refs.iter().filter(|r| r.kind == RefKind::Bisect).map(|r| r.name.clone())).collect();
    assert!(labels.contains(&"bisect: bad".to_string()) && labels.contains(&"bisect: good".to_string()), "{labels:?}");

    for _ in 0..10 {
        let value: u32 = std::fs::read_to_string(t.path().join("n.txt")).unwrap().trim().parse().unwrap();
        let mark = if value >= 4 { BisectMark::Bad } else { BisectMark::Good };
        let message = t.repo.bisect_mark(mark, None).await.unwrap();
        if message.contains("is the first bad commit") {
            break;
        }
    }
    let snapshot = t.repo.snapshot().await.unwrap();
    match &snapshot.operation {
        Some(Operation::Bisect { first_bad: Some(found), .. }) => assert!(found.ends_with("Step 4"), "{found}"),
        other => panic!("bisect not finished: {other:?}"),
    }

    t.repo.bisect_reset().await.unwrap();
    let snapshot = t.repo.snapshot().await.unwrap();
    assert_eq!(snapshot.operation, None);
    assert_eq!(snapshot.head, Head::Branch { name: "main".into() });
}

/// A Git LFS pointer is shown through git's filters (here a stand-in smudge filter instead of git-lfs), and the
/// diff says so, since its lines cannot be staged one by one.
#[tokio::test]
async fn lfs_pointers_are_shown_through_the_smudge_filter() {
    let t = TestRepo::with_commit().await;
    t.git(&["config", "filter.lfs.smudge", "tr a-z A-Z"]);
    t.git(&["config", "filter.lfs.clean", "cat"]);
    std::fs::write(t.path().join(".gitattributes"), "*.bin filter=lfs\n").unwrap();
    let pointer = |size: u32| format!("version https://git-lfs.github.com/spec/v1\noid sha256:abc{size}\nsize {size}\n");
    t.commit_file("data.bin", &pointer(3), "Add data");
    t.commit_file("data.bin", &pointer(4), "Change data");
    let head = t.git(&["rev-parse", "HEAD"]).trim().to_string();
    let contents = t.repo.diff_contents("data.bin", None, &DiffSource::Commit { oid: head, parent: None }).await.unwrap();
    assert!(contents.lfs);
    assert!(contents.b.text.as_deref().unwrap().starts_with("VERSION HTTPS"), "{:?}", contents.b.text);
    // An ordinary text file is shown as it is.
    let head = t.git(&["rev-parse", "HEAD~1"]).trim().to_string();
    let plain = t.repo.diff_contents(".gitattributes", None, &DiffSource::Commit { oid: head, parent: None }).await.unwrap();
    assert!(!plain.lfs);
}

#[tokio::test]
async fn log_of_empty_repo_is_empty() {
    let t = TestRepo::new().await;
    assert!(t.repo.log(&LogQuery::default(), 10).await.unwrap().is_empty());
    assert!(t.repo.reflog(10).await.unwrap().is_empty());
    assert_eq!(t.repo.last_commit_message().await.unwrap(), "");
}

// ----- Branch -----

#[tokio::test]
async fn branch_lifecycle() {
    let t = TestRepo::with_commit().await;
    t.repo.create_branch("feature/login", None, true).await.unwrap();
    t.commit_file("login.txt", "x\n", "Login");
    t.repo.switch("main").await.unwrap();
    t.repo.create_branch("other", Some("feature/login"), false).await.unwrap();

    let refs = t.repo.refs().await.unwrap();
    let branch_names: Vec<_> = refs.branches.iter().map(|b| (b.name.as_str(), b.is_head)).collect();
    assert_eq!(branch_names, vec![("feature/login", false), ("main", true), ("other", false)]);
    assert_eq!(refs.branches[0].subject, "Login");

    t.repo.rename_branch("other", "other-2").await.unwrap();
    assert_eq!(t.repo.unmerged_count("feature/login").await.unwrap(), 1);
    let err = t.repo.delete_branch("feature/login", false).await.unwrap_err();
    assert_eq!(err.code, GitErrorCode::BranchNotMerged);
    t.repo.delete_branch("feature/login", true).await.unwrap();
    t.repo.delete_branch("other-2", true).await.unwrap();
    assert_eq!(t.repo.refs().await.unwrap().branches.len(), 1);
}

#[tokio::test]
async fn switching_with_conflicting_local_changes_is_reported_as_dirty() {
    let t = TestRepo::with_commit().await;
    t.repo.create_branch("other", None, true).await.unwrap();
    t.commit_file("README.md", "# Other\n", "Change on other");
    t.repo.switch("main").await.unwrap();
    t.write("README.md", "# Local\n");
    let err = t.repo.switch("other").await.unwrap_err();
    assert_eq!(err.code, GitErrorCode::DirtyWorkTree);
    assert_eq!(t.read("README.md"), "# Local\n", "local changes must not be lost");
}

// ----- Merge and conflicts -----

async fn conflicted_merge() -> TestRepo {
    let t = TestRepo::with_commit().await;
    t.commit_file("config.toml", "timeout = 30\nretries = 3\nlog = \"info\"\n", "Configuration");
    t.git(&["switch", "-q", "-c", "feature"]);
    t.commit_file("config.toml", "timeout = 60\nretries = 5\nlog = \"info\"\n", "Feature changes retries");
    t.git(&["switch", "-q", "main"]);
    t.commit_file("config.toml", "timeout = 30\nretries = 4\nlog = \"debug\"\n", "Main changes retries");
    t
}

#[tokio::test]
async fn merge_conflict_is_detected_and_resolved_through_editor_flow() {
    let t = conflicted_merge().await;
    let err = t.repo.merge("feature", MergeMode::Default).await.unwrap_err();
    assert_eq!(err.code, GitErrorCode::Conflict);

    let s = t.repo.snapshot().await.unwrap();
    assert_eq!(s.operation, Some(Operation::Merge { incoming: "feature".into() }));
    assert_eq!(s.conflicted, vec![ConflictEntry { path: "config.toml".into(), kind: ConflictKind::BothModified }]);

    let detail = t.repo.conflict_detail("config.toml").await.unwrap();
    assert_eq!((detail.current_label.as_str(), detail.incoming_label.as_str()), ("main", "feature"));
    assert_eq!(detail.base.as_deref(), Some("timeout = 30\nretries = 3\nlog = \"info\"\n"));
    let conflicts: Vec<_> = detail.segments.iter().filter(|s| matches!(s, MergeSegment::Conflict { .. })).collect();
    assert_eq!(conflicts.len(), 1);
    let joined: String = detail
        .segments
        .iter()
        .map(|s| match s {
            MergeSegment::Resolved { text } => text.clone(),
            MergeSegment::Conflict { incoming, .. } => incoming.clone(),
        })
        .collect();
    assert!(joined.contains("retries = 5"));

    // The conflict editor writes the result and then marks it resolved.
    t.write("config.toml", "timeout = 60\nretries = 5\nlog = \"debug\"\n");
    t.repo.mark_resolved("config.toml").await.unwrap();
    assert!(t.repo.snapshot().await.unwrap().conflicted.is_empty());
    t.repo.operation(OperationAction::Continue).await.unwrap();

    let s = t.repo.snapshot().await.unwrap();
    assert!(s.operation.is_none());
    assert_eq!(t.git(&["rev-list", "--parents", "-n", "1", "HEAD"]).split_whitespace().count(), 3);
}

/// Two conflicting regions with `between` unchanged lines between them.
async fn two_conflicts(between: &str) -> TestRepo {
    let t = TestRepo::with_commit().await;
    t.commit_file("list.txt", &format!("a = 1\n{between}b = 1\n"), "Base");
    t.git(&["switch", "-q", "-c", "feature"]);
    t.commit_file("list.txt", &format!("a = 3\n{between}b = 3\n"), "Feature");
    t.git(&["switch", "-q", "main"]);
    t.commit_file("list.txt", &format!("a = 2\n{between}b = 2\n"), "Main");
    t.repo.merge("feature", MergeMode::Default).await.unwrap_err();
    t
}

fn conflicts(segments: &[MergeSegment]) -> Vec<(&str, &str)> {
    segments
        .iter()
        .filter_map(|s| match s {
            MergeSegment::Conflict { current, incoming, .. } => Some((current.as_str(), incoming.as_str())),
            MergeSegment::Resolved { .. } => None,
        })
        .collect()
}

#[tokio::test]
async fn conflict_detail_keeps_what_was_already_resolved_in_the_working_tree() {
    let between = "k1\nk2\nk3\nk4\n";
    let t = two_conflicts(between).await;
    let fresh = t.repo.conflict_detail("list.txt").await.unwrap();
    assert!(!fresh.from_worktree, "git's own markers match a fresh merge");
    assert_eq!(conflicts(&fresh.segments), vec![("a = 2\n", "a = 3\n"), ("b = 2\n", "b = 3\n")]);
    assert!(fresh.segments.iter().any(|s| matches!(s, MergeSegment::Conflict { base, .. } if base == "a = 1\n")), "a fresh merge has the base");
    assert_eq!(fresh.worktree.as_deref(), Some(t.read("list.txt").as_str()));

    // The first conflict is resolved by hand; the editor starts from that.
    let tree = t.read("list.txt");
    let end = tree.find(">>>>>>> feature\n").unwrap() + ">>>>>>> feature\n".len();
    t.write("list.txt", &format!("a = 23\n{}", &tree[end..]));
    let detail = t.repo.conflict_detail("list.txt").await.unwrap();
    assert!(detail.from_worktree);
    assert_eq!(conflicts(&detail.segments), vec![("b = 2\n", "b = 3\n")]);
    assert_eq!(detail.segments[0], MergeSegment::Resolved { text: format!("a = 23\n{between}") });

    // Without markers left, there is nothing to choose and the file is kept as it is.
    t.write("list.txt", "a = 23\nb = 23\n");
    let detail = t.repo.conflict_detail("list.txt").await.unwrap();
    assert_eq!(detail.segments, vec![MergeSegment::Resolved { text: "a = 23\nb = 23\n".into() }]);
}

/// git joins conflicts that are close together (and leaves out the base); that alone is no edit.
#[tokio::test]
async fn conflicts_git_joined_still_come_from_a_fresh_merge() {
    let t = two_conflicts("keep\n").await;
    assert_eq!(t.read("list.txt").matches("<<<<<<<").count(), 1, "git wrote one joined conflict");
    let detail = t.repo.conflict_detail("list.txt").await.unwrap();
    assert!(!detail.from_worktree);
    assert_eq!(conflicts(&detail.segments).len(), 2);
}

/// A Windows-1252 file with CRLF line endings is read as such, so it can be written back the same way.
#[tokio::test]
async fn conflict_detail_reads_the_files_own_encoding_and_line_endings() {
    let t = TestRepo::with_commit().await;
    let write = |text: &str| std::fs::write(t.file("menu.txt"), text.replace('\n', "\r\n").chars().map(|c| if c == 'é' { 0xE9u8 } else { c as u8 }).collect::<Vec<u8>>()).unwrap();
    write("caf\u{e9}\nprix = 1\n");
    t.git(&["add", "-A"]);
    t.git(&["commit", "-q", "-m", "Base"]);
    t.git(&["switch", "-q", "-c", "feature"]);
    write("caf\u{e9}\nprix = 3 \u{e9}\n");
    t.git(&["commit", "-q", "-am", "Feature"]);
    t.git(&["switch", "-q", "main"]);
    write("caf\u{e9}\nprix = 2 \u{e9}\n");
    t.git(&["commit", "-q", "-am", "Main"]);
    t.repo.merge("feature", MergeMode::Default).await.unwrap_err();

    let detail = t.repo.conflict_detail("menu.txt").await.unwrap();
    assert_eq!(detail.format, TextFormat { encoding: "windows-1252".into(), bom: false, crlf: true });
    assert_eq!(detail.current.as_deref(), Some("caf\u{e9}\r\nprix = 2 \u{e9}\r\n"));
    assert_eq!(conflicts(&detail.segments), vec![("prix = 2 \u{e9}\r\n", "prix = 3 \u{e9}\r\n")]);
    assert_eq!(detail.segments[0], MergeSegment::Resolved { text: "caf\u{e9}\r\n".into() });
    assert_eq!(t.repo.worktree_text("menu.txt", &detail.format.encoding).await.unwrap(), detail.worktree);
}

#[tokio::test]
async fn resolving_a_conflict_with_glob_characters_leaves_other_files_alone() {
    let t = TestRepo::with_commit().await;
    t.write("app/i/page.tsx", "i\n");
    t.write("app/d/page.tsx", "d\n");
    t.commit_file("app/[id]/page.tsx", "base\n", "Routes");
    t.git(&["switch", "-q", "-c", "feature"]);
    t.commit_file("app/[id]/page.tsx", "feature\n", "Feature");
    t.git(&["switch", "-q", "main"]);
    t.commit_file("app/[id]/page.tsx", "main\n", "Main");
    t.repo.merge("feature", MergeMode::Default).await.unwrap_err();

    t.repo.resolve_by_deleting("app/[id]/page.tsx").await.unwrap();
    assert!(!t.file("app/[id]/page.tsx").exists());
    assert_eq!((t.read("app/i/page.tsx"), t.read("app/d/page.tsx")), ("i\n".to_string(), "d\n".to_string()));
    let s = t.repo.snapshot().await.unwrap();
    assert!(s.conflicted.is_empty());
    assert_eq!(names(&s.staged), vec![("app/[id]/page.tsx", ChangeKind::Deleted)]);
}

#[tokio::test]
async fn merge_can_be_aborted_and_resolved_by_side() {
    let t = conflicted_merge().await;
    let before = t.git(&["rev-parse", "HEAD"]);
    t.repo.merge("feature", MergeMode::Default).await.unwrap_err();
    t.repo.operation(OperationAction::Abort).await.unwrap();
    assert!(t.repo.snapshot().await.unwrap().operation.is_none());
    assert_eq!(t.git(&["rev-parse", "HEAD"]), before);

    t.repo.merge("feature", MergeMode::Default).await.unwrap_err();
    t.repo.resolve_with_side("config.toml", ConflictSide::Incoming).await.unwrap();
    assert_eq!(t.read("config.toml"), "timeout = 60\nretries = 5\nlog = \"info\"\n");
    assert!(t.repo.snapshot().await.unwrap().conflicted.is_empty());
}

#[tokio::test]
async fn delete_modify_conflict_can_keep_or_delete() {
    let t = TestRepo::with_commit().await;
    t.commit_file("f.txt", "a\n", "Add f");
    t.git(&["switch", "-q", "-c", "remove"]);
    t.git(&["rm", "-q", "f.txt"]);
    t.git(&["commit", "-q", "-m", "Remove f"]);
    t.git(&["switch", "-q", "main"]);
    t.commit_file("f.txt", "a\nb\n", "Modify f");

    t.repo.merge("remove", MergeMode::Default).await.unwrap_err();
    let s = t.repo.snapshot().await.unwrap();
    assert_eq!(s.conflicted[0].kind, ConflictKind::DeletedByThem);
    let detail = t.repo.conflict_detail("f.txt").await.unwrap();
    assert_eq!(detail.kind, ConflictKind::DeletedByThem);
    assert!(detail.incoming.is_none() && detail.segments.is_empty());

    // Choosing the incoming side means deleting the file.
    t.repo.resolve_with_side("f.txt", ConflictSide::Incoming).await.unwrap();
    assert!(!t.file("f.txt").exists());
    assert!(t.repo.snapshot().await.unwrap().conflicted.is_empty());
}

#[tokio::test]
async fn fast_forward_only_merge_is_refused_when_diverged() {
    let t = conflicted_merge().await;
    assert!(t.repo.merge("feature", MergeMode::FfOnly).await.is_err());
    assert!(t.repo.snapshot().await.unwrap().operation.is_none());
}

// ----- Rebase -----

#[tokio::test]
async fn rebase_conflict_reports_progress_and_can_be_aborted() {
    let t = conflicted_merge().await;
    t.git(&["switch", "-q", "feature"]);
    t.commit_file("other.txt", "x\n", "Second commit on feature");
    let before = t.git(&["rev-parse", "HEAD"]);

    let err = t.repo.rebase("main").await.unwrap_err();
    assert_eq!(err.code, GitErrorCode::Conflict);
    let s = t.repo.snapshot().await.unwrap();
    match s.operation {
        Some(Operation::Rebase { step, total, head_name, interactive, .. }) => {
            assert_eq!((step, total, head_name.as_str(), interactive), (1, 2, "feature", false));
        }
        other => panic!("expected a rebase, got {other:?}"),
    }
    let detail = t.repo.conflict_detail("config.toml").await.unwrap();
    assert!(detail.current_label.starts_with("new base"));
    assert!(detail.incoming_label.starts_with("feature"));

    t.repo.operation(OperationAction::Abort).await.unwrap();
    assert_eq!(t.git(&["rev-parse", "HEAD"]), before);
    assert_eq!(t.repo.snapshot().await.unwrap().head, Head::Branch { name: "feature".into() });
}

#[tokio::test]
async fn rebase_continue_after_resolving() {
    let t = conflicted_merge().await;
    t.git(&["switch", "-q", "feature"]);
    t.repo.rebase("main").await.unwrap_err();
    t.write("config.toml", "timeout = 60\nretries = 5\nlog = \"debug\"\n");
    t.repo.mark_resolved("config.toml").await.unwrap();
    t.repo.operation(OperationAction::Continue).await.unwrap();
    assert!(t.repo.snapshot().await.unwrap().operation.is_none());
    assert_eq!(t.git(&["log", "-1", "--format=%s"]).trim(), "Feature changes retries");
    assert_eq!(t.git(&["merge-base", "HEAD", "main"]), t.git(&["rev-parse", "main"]));
}

#[tokio::test]
async fn merge_base_finds_common_ancestor() {
    let t = conflicted_merge().await;
    let base = t.git(&["merge-base", "main", "feature"]).trim().to_string();
    assert_eq!(t.repo.merge_base("main", "feature").await.unwrap().as_deref(), Some(base.as_str()));
    // History with no common ancestor.
    t.git(&["switch", "-q", "--orphan", "unrelated"]);
    t.commit_file("x.txt", "x\n", "Another root");
    assert_eq!(t.repo.merge_base("main", "unrelated").await.unwrap(), None);
    assert!(t.repo.merge_base("main", "does-not-exist").await.is_err());
}

#[tokio::test]
async fn rebase_plan_lists_commits_oldest_first() {
    let t = TestRepo::with_commit().await;
    let base = t.git(&["rev-parse", "HEAD"]).trim().to_string();
    t.commit_file("a.txt", "a\n", "A");
    t.commit_file("b.txt", "b\n", "B");
    let plan = t.repo.rebase_plan(Some(&base)).await.unwrap();
    assert_eq!(plan.items.iter().map(|i| i.subject.as_str()).collect::<Vec<_>>(), vec!["A", "B"]);
    assert_eq!(plan.published, 0);
    let plan = t.repo.rebase_plan(None).await.unwrap();
    assert_eq!(plan.items.len(), 3);
}

// ----- Stash -----

#[tokio::test]
async fn stash_roundtrip() {
    let t = TestRepo::with_commit().await;
    t.write("README.md", "# Changed\n");
    t.write("new.txt", "new\n");
    t.repo.stash_push(&StashOptions { message: Some("work in progress".into()), include_untracked: true, ..Default::default() }).await.unwrap();
    assert_eq!(t.read("README.md"), "# Project\n");
    assert!(!t.file("new.txt").exists());

    let list = t.repo.stash_list().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "stash@{0}");
    assert!(list[0].message.contains("work in progress"));
    assert_eq!(t.repo.snapshot().await.unwrap().stash_count, 1);

    let mut files: Vec<_> = t.repo.stash_files("stash@{0}").await.unwrap().into_iter().map(|f| f.path).collect();
    files.sort();
    assert_eq!(files, vec!["README.md", "new.txt"]);
    let d = t.repo.diff_contents("new.txt", None, &DiffSource::Stash { name: "stash@{0}".into() }).await.unwrap();
    assert_eq!(d.b.text.as_deref(), Some("new\n"));

    t.repo.stash_apply("stash@{0}", false).await.unwrap();
    assert_eq!(t.read("README.md"), "# Changed\n");
    assert_eq!(t.repo.stash_list().await.unwrap().len(), 1);
    t.repo.stash_drop("stash@{0}").await.unwrap();
    assert!(t.repo.stash_list().await.unwrap().is_empty());
}

#[tokio::test]
async fn stash_only_selected_paths_and_pop() {
    let t = TestRepo::with_commit().await;
    t.commit_file("a.txt", "a\n", "A");
    t.write("a.txt", "A2\n");
    t.write("README.md", "# R2\n");
    t.repo.stash_push(&StashOptions { paths: paths(&["a.txt"]), ..Default::default() }).await.unwrap();
    assert_eq!(t.read("a.txt"), "a\n");
    assert_eq!(t.read("README.md"), "# R2\n");
    t.repo.stash_apply("stash@{0}", true).await.unwrap();
    assert_eq!(t.read("a.txt"), "A2\n");
    assert!(t.repo.stash_list().await.unwrap().is_empty());
}

// ----- Tag, cherry-pick, revert, reset -----

#[tokio::test]
async fn tags_lightweight_and_annotated() {
    let t = TestRepo::with_commit().await;
    let head = t.git(&["rev-parse", "HEAD"]).trim().to_string();
    t.repo.tag_create("v1.0.0", None, Some("First release")).await.unwrap();
    t.repo.tag_create("lightweight", Some(&head), None).await.unwrap();
    let tags = t.repo.refs().await.unwrap().tags;
    let annotated = tags.iter().find(|t| t.name == "v1.0.0").unwrap();
    assert!(annotated.annotated);
    assert_eq!(annotated.message, "First release");
    assert_eq!(annotated.oid, head, "annotated tag must be peeled to the commit");
    let light = tags.iter().find(|t| t.name == "lightweight").unwrap();
    assert!(!light.annotated && light.oid == head);
    t.repo.tag_delete("lightweight").await.unwrap();
    assert_eq!(t.repo.refs().await.unwrap().tags.len(), 1);
}

#[tokio::test]
async fn cherry_pick_revert_and_reset() {
    let t = TestRepo::with_commit().await;
    t.git(&["switch", "-q", "-c", "side"]);
    let picked = t.commit_file("picked.txt", "p\n", "To be picked");
    t.git(&["switch", "-q", "main"]);

    t.repo.cherry_pick(&[picked.clone()]).await.unwrap();
    assert_eq!(t.read("picked.txt"), "p\n");
    assert_eq!(t.git(&["log", "-1", "--format=%s"]).trim(), "To be picked");

    let head = t.git(&["rev-parse", "HEAD"]).trim().to_string();
    t.repo.revert(&head).await.unwrap();
    assert!(!t.file("picked.txt").exists());
    assert!(t.git(&["log", "-1", "--format=%s"]).starts_with("Revert"));

    t.write("dirty.txt", "k\n");
    t.repo.stage_all().await.unwrap();
    t.repo.reset(&head, ResetMode::Mixed).await.unwrap();
    assert_eq!(t.git(&["rev-parse", "HEAD"]).trim(), head);
    assert!(t.file("picked.txt").exists() == false, "reset mixed does not change the working tree");
    t.repo.reset(&head, ResetMode::Hard).await.unwrap();
    assert_eq!(t.read("picked.txt"), "p\n");

    let reflog = t.repo.reflog(20).await.unwrap();
    assert!(reflog.len() >= 4);
    assert_eq!(reflog[0].selector, "HEAD@{0}");
    assert!(reflog.iter().any(|e| e.message.contains("cherry-pick")));
}

#[tokio::test]
async fn cherry_pick_conflict_can_be_aborted() {
    let t = conflicted_merge().await;
    let incoming = t.git(&["rev-parse", "feature"]).trim().to_string();
    let err = t.repo.cherry_pick(&[incoming]).await.unwrap_err();
    assert_eq!(err.code, GitErrorCode::Conflict);
    assert!(matches!(t.repo.snapshot().await.unwrap().operation, Some(Operation::CherryPick { .. })));
    t.repo.operation(OperationAction::Abort).await.unwrap();
    assert!(t.repo.snapshot().await.unwrap().operation.is_none());
}

// ----- Worktree and submodule -----

#[tokio::test]
async fn worktree_add_list_remove() {
    let t = TestRepo::with_commit().await;
    let extra = tempfile::tempdir().unwrap();
    let wt_path = extra.path().join("wt feature");
    t.repo.worktree_add(&wt_path, Some("feature"), None).await.unwrap();

    let list = t.repo.worktrees().await.unwrap();
    assert_eq!(list.len(), 2);
    assert!(list[0].is_main && list[0].is_current);
    assert_eq!(list[1].branch.as_deref(), Some("feature"));
    let refs = t.repo.refs().await.unwrap();
    let fitur = refs.branches.iter().find(|b| b.name == "feature").unwrap();
    assert!(fitur.worktree.as_deref().is_some_and(|p| p.ends_with("wt feature")));

    // A worktree has its own git directory for HEAD and operation markers.
    let wt = Repo::open(t.git.clone(), &wt_path).await.unwrap();
    assert_ne!(wt.git_dir(), wt.common_dir());
    assert_eq!(wt.snapshot().await.unwrap().head, Head::Branch { name: "feature".into() });

    std::fs::write(wt_path.join("dirty.txt"), "x").unwrap();
    assert!(t.repo.worktree_remove(&wt_path, false).await.is_err());
    t.repo.worktree_remove(&wt_path, true).await.unwrap();
    assert_eq!(t.repo.worktrees().await.unwrap().len(), 1);
}

#[tokio::test]
async fn submodule_status_and_update() {
    let lib = TestRepo::with_commit().await;
    let t = TestRepo::with_commit().await;
    assert!(t.repo.submodules().await.unwrap().is_empty());
    t.git(&["submodule", "add", "-q", lib.path().to_str().unwrap(), "vendor/lib"]);
    t.git(&["commit", "-q", "-m", "Add submodule"]);

    let subs = t.repo.submodules().await.unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!((subs[0].path.as_str(), subs[0].status), ("vendor/lib", SubmoduleStatus::UpToDate));

    // Fresh clone: the submodule is not initialized until it is updated.
    let clone_dir = tempfile::tempdir().unwrap();
    raw_git(clone_dir.path(), &["clone", "-q", t.path().to_str().unwrap(), "copy"]);
    let clone = Repo::open(t.git.clone(), &clone_dir.path().join("copy")).await.unwrap();
    assert_eq!(clone.submodules().await.unwrap()[0].status, SubmoduleStatus::Uninitialized);
    clone.submodule_update(&[], true, None, None).await.unwrap();
    assert_eq!(clone.submodules().await.unwrap()[0].status, SubmoduleStatus::UpToDate);
    assert!(clone.root().join("vendor/lib/README.md").exists());

    // A new commit inside the submodule shows up as a change in the superproject.
    let sub_path = clone.root().join("vendor/lib");
    std::fs::write(sub_path.join("new.txt"), "x").unwrap();
    raw_git(&sub_path, &["add", "-A"]);
    raw_git(&sub_path, &["commit", "-q", "-m", "Inside submodule"]);
    assert_eq!(clone.submodules().await.unwrap()[0].status, SubmoduleStatus::Modified);
    let s = clone.snapshot().await.unwrap();
    assert!(s.unstaged[0].submodule.as_ref().is_some_and(|m| m.commit_changed));
}

// ----- Blame -----

#[tokio::test]
async fn blame_attributes_lines_and_marks_unsaved_changes() {
    let t = TestRepo::with_commit().await;
    let first = t.commit_file("f.txt", "one\ntwo\nthree\n", "Initial");
    let second = t.commit_file("f.txt", "one\nTWO\nthree\n", "Change line two");

    let ranges = t.repo.blame("f.txt", None).await.unwrap();
    let owner = |line: u32| ranges.iter().find(|r| line >= r.start_line && line < r.start_line + r.line_count).unwrap();
    assert_eq!(owner(1).oid, first);
    assert_eq!(owner(2).oid, second);
    assert_eq!(owner(2).summary, "Change line two");
    assert_eq!(owner(3).oid, first);
    assert_eq!(owner(1).author, "Tester");

    // Unsaved buffer content: the new line is marked as uncommitted.
    let ranges = t.repo.blame("f.txt", Some(b"one\nTWO\ninserted\nthree\n".to_vec())).await.unwrap();
    let inserted = ranges.iter().find(|r| r.start_line == 3).unwrap();
    assert!(inserted.uncommitted);
    assert_eq!(ranges.iter().map(|r| r.line_count).sum::<u32>(), 4);
}

// ----- Remote -----

#[tokio::test]
async fn clone_push_fetch_and_pull_against_local_remote() {
    common::isolate();
    let base = tempfile::tempdir().unwrap();
    raw_git(base.path(), &["init", "-q", "--bare", "-b", "main", "remote.git"]);
    let remote_url = base.path().join("remote.git").to_string_lossy().into_owned();

    // First author: clone the empty repository, commit, then publish the branch.
    let git = Git::discover(None).await.unwrap();
    let a_path = base.path().join("a");
    git.clone_repo(&remote_url, &a_path, false, None, None).await.unwrap();
    let a = Repo::open(git.clone(), &a_path).await.unwrap();
    std::fs::write(a_path.join("a.txt"), "a\n").unwrap();
    a.stage_all().await.unwrap();
    a.commit("From A", &CommitOptions::default()).await.unwrap();

    let s = a.snapshot().await.unwrap();
    assert!(s.upstream.as_ref().is_none_or(|u| !u.exists), "branch is not published yet");
    a.push(&PushOptions { set_upstream: true, ..Default::default() }, None, None).await.unwrap();
    let s = a.snapshot().await.unwrap();
    let up = s.upstream.unwrap();
    assert_eq!((up.name.as_str(), up.ahead, up.behind, up.exists), ("origin/main", 0, 0, true));

    // Second author: clone, commit, push.
    let b_path = base.path().join("b");
    git.clone_repo(&remote_url, &b_path, false, None, None).await.unwrap();
    let b = Repo::open(git.clone(), &b_path).await.unwrap();
    std::fs::write(b_path.join("b.txt"), "b\n").unwrap();
    b.stage_all().await.unwrap();
    b.commit("From B", &CommitOptions::default()).await.unwrap();
    b.push(&PushOptions::default(), None, None).await.unwrap();

    // A is behind: fetch shows behind, push is rejected after a local commit, pull resolves it.
    a.fetch(None, None, None).await.unwrap();
    assert_eq!(a.snapshot().await.unwrap().upstream.unwrap().behind, 1);
    std::fs::write(a_path.join("a2.txt"), "a2\n").unwrap();
    a.stage_all().await.unwrap();
    a.commit("A again", &CommitOptions::default()).await.unwrap();
    let err = a.push(&PushOptions::default(), None, None).await.unwrap_err();
    assert_eq!(err.code, GitErrorCode::PushRejected);

    assert!(a.pull(PullStrategy::FfOnly, None, None).await.is_err(), "ff-only must fail when history has diverged");
    a.pull(PullStrategy::Rebase, None, None).await.unwrap();
    let up = a.snapshot().await.unwrap().upstream.unwrap();
    assert_eq!((up.ahead, up.behind), (1, 0));
    a.push(&PushOptions::default(), None, None).await.unwrap();

    let refs = a.refs().await.unwrap();
    assert_eq!(refs.remotes.len(), 1);
    assert_eq!(refs.remotes[0].name, "origin");
    assert_eq!(refs.remote_branches.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), vec!["origin/main"]);
    let main = refs.branches.iter().find(|b| b.name == "main").unwrap();
    assert_eq!(main.upstream.as_deref(), Some("origin/main"));

    // Tag: push, then delete on the remote.
    a.tag_create("v1", None, None).await.unwrap();
    a.tag_push("origin", "v1").await.unwrap();
    assert!(raw_git(&base.path().join("remote.git"), &["tag"]).contains("v1"));
    a.tag_delete_remote("origin", "v1").await.unwrap();
    assert!(!raw_git(&base.path().join("remote.git"), &["tag"]).contains("v1"));

    // Remote branch: tracking checkout, then delete on the remote.
    a.create_branch("topic", None, true).await.unwrap();
    a.push(&PushOptions { set_upstream: true, ..Default::default() }, None, None).await.unwrap();
    b.fetch(Some("origin"), None, None).await.unwrap();
    b.switch_tracking("origin/topic").await.unwrap();
    assert_eq!(b.snapshot().await.unwrap().upstream.unwrap().name, "origin/topic");
    a.delete_remote_branch("origin", "topic").await.unwrap();
    b.fetch(None, None, None).await.unwrap();
    let topik = b.refs().await.unwrap().branches.into_iter().find(|br| br.name == "topic").unwrap();
    assert!(topik.upstream_gone);
}

#[tokio::test]
async fn remotes_can_be_managed() {
    let t = TestRepo::with_commit().await;
    t.repo.remote_add("origin", "https://example.com/a/b.git").await.unwrap();
    t.repo.remote_set_url("origin", "git@example.com:a/b.git").await.unwrap();
    t.repo.remote_rename("origin", "upstream").await.unwrap();
    let remotes = t.repo.remotes().await.unwrap();
    assert_eq!(remotes, vec![Remote { name: "upstream".into(), fetch_url: "git@example.com:a/b.git".into(), push_url: "git@example.com:a/b.git".into() }]);
    t.repo.remote_remove("upstream").await.unwrap();
    assert!(t.repo.remotes().await.unwrap().is_empty());
}

#[tokio::test]
async fn opening_a_non_repository_fails_with_not_a_repo() {
    common::isolate();
    let dir = tempfile::tempdir().unwrap();
    let git = Git::discover(None).await.unwrap();
    let err = Repo::open(git.clone(), dir.path()).await.err().unwrap();
    assert_eq!(err.code, GitErrorCode::NotARepo);
    assert!(Repo::find_root(&git, dir.path()).await.is_none());
    git.init(dir.path()).await.unwrap();
    std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
    let root = Repo::find_root(&git, &dir.path().join("a/b")).await.unwrap();
    assert_eq!(root.canonicalize().unwrap(), dir.path().canonicalize().unwrap());
    assert!(git.is_supported());
}

// ----- Commit message material -----

#[tokio::test]
async fn staged_diff_covers_only_the_index_and_is_cut_at_the_limit() {
    let t = TestRepo::with_commit().await;
    t.write("README.md", "# Project\nnot staged\n");
    t.write("new.txt", "line é new\n");
    t.write("other.txt", "other\n");
    t.git(&["add", "new.txt", "other.txt"]);
    // The user's external diff tool is not used.
    t.git(&["config", "diff.external", "no-such-command"]);

    let diff = t.repo.staged_diff(100_000).await.unwrap();
    assert!(diff.stat.contains("new.txt") && diff.stat.contains("other.txt"), "{}", diff.stat);
    assert!(diff.stat.contains("2 files changed"), "{}", diff.stat);
    assert!(diff.patch.contains("+line é new") && diff.patch.contains("+other"));
    assert!(!diff.patch.contains("not staged"), "unstaged changes must not be included");
    assert!(!diff.truncated);

    // Cut at a character boundary: never in the middle of "é".
    let cut_at = diff.patch.find('é').unwrap() + 1;
    let cut = t.repo.staged_diff(cut_at).await.unwrap();
    assert!(cut.truncated);
    assert!(cut.patch.len() < cut_at && diff.patch.starts_with(&cut.patch));
    assert_eq!(cut.stat, diff.stat, "stat stays intact");

    t.git(&["reset", "-q"]);
    let empty = t.repo.staged_diff(100_000).await.unwrap();
    assert!(empty.stat.is_empty() && empty.patch.is_empty());
}

#[tokio::test]
async fn staged_diff_and_subjects_work_before_the_first_commit() {
    let t = TestRepo::new().await;
    assert!(t.repo.recent_subjects(10).await.unwrap().is_empty());
    t.write("a.txt", "a\n");
    t.git(&["add", "a.txt"]);
    let diff = t.repo.staged_diff(100_000).await.unwrap();
    assert!(diff.patch.contains("+a"), "{}", diff.patch);
}

#[tokio::test]
async fn recent_subjects_are_newest_first() {
    let t = TestRepo::with_commit().await;
    t.commit_file("a.txt", "a\n", "feat: add a\n\nMessage body.");
    t.commit_file("b.txt", "b\n", "fix: correct b");
    let subjects = t.repo.recent_subjects(2).await.unwrap();
    assert_eq!(subjects, ["fix: correct b", "feat: add a"]);
}
