//! Uji integrasi: tiap operasi dijalankan terhadap repositori git sungguhan di direktori sementara.

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
    t.commit_file("lama.txt", "isi\n", "Tambah lama");
    t.commit_file("hapus.txt", "x\n", "Tambah hapus");
    t.write("README.md", "# Proyek\nberubah\n");
    t.write("baru dengan spasi.txt", "baru\n");
    t.write("tahap.txt", "tahap\n");
    t.git(&["add", "tahap.txt"]);
    t.git(&["mv", "lama.txt", "nama baru.txt"]);
    std::fs::remove_file(t.file("hapus.txt")).unwrap();

    let s = t.repo.snapshot().await.unwrap();
    assert_eq!(s.head, Head::Branch { name: "main".into() });
    assert_eq!(names(&s.staged), vec![("nama baru.txt", ChangeKind::Renamed), ("tahap.txt", ChangeKind::Added)]);
    assert_eq!(s.staged[0].orig_path.as_deref(), Some("lama.txt"));
    assert_eq!(names(&s.unstaged), vec![("README.md", ChangeKind::Modified), ("hapus.txt", ChangeKind::Deleted)]);
    assert_eq!(s.untracked, vec!["baru dengan spasi.txt"]);
    assert!(s.operation.is_none());
}

#[tokio::test]
async fn snapshot_handles_non_ascii_names_and_detached_head() {
    let t = TestRepo::with_commit().await;
    let first = t.git(&["rev-parse", "HEAD"]).trim().to_string();
    t.commit_file("catatan/résumé ünï.txt", "a\n", "Nama non-ASCII");
    t.write("catatan/résumé ünï.txt", "b\n");
    let s = t.repo.snapshot().await.unwrap();
    assert_eq!(names(&s.unstaged), vec![("catatan/résumé ünï.txt", ChangeKind::Modified)]);

    t.git(&["checkout", "-q", "--", "."]);
    t.repo.switch_detached(&first).await.unwrap();
    let s = t.repo.snapshot().await.unwrap();
    assert_eq!(s.head, Head::Detached { oid: first });
}

// ----- Staging per berkas -----

#[tokio::test]
async fn stage_unstage_and_discard_whole_files() {
    let t = TestRepo::with_commit().await;
    t.write("README.md", "# Proyek\nubah\n");
    t.write("baru.txt", "baru\n");

    t.repo.stage(&paths(&["README.md", "baru.txt"])).await.unwrap();
    let s = t.repo.snapshot().await.unwrap();
    assert_eq!(names(&s.staged), vec![("README.md", ChangeKind::Modified), ("baru.txt", ChangeKind::Added)]);
    assert!(s.unstaged.is_empty() && s.untracked.is_empty());

    t.repo.unstage(&paths(&["baru.txt"])).await.unwrap();
    let s = t.repo.snapshot().await.unwrap();
    assert_eq!(names(&s.staged), vec![("README.md", ChangeKind::Modified)]);
    assert_eq!(s.untracked, vec!["baru.txt"]);

    t.repo.unstage_all().await.unwrap();
    t.repo.discard(&paths(&["README.md"])).await.unwrap();
    assert_eq!(t.read("README.md"), "# Proyek\n");
    // Berkas untracked tidak disentuh oleh discard.
    assert_eq!(t.read("baru.txt"), "baru\n");
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
    let err = t.repo.stage(&paths(&["../di-luar.txt"])).await.unwrap_err();
    assert_eq!(err.code, GitErrorCode::InvalidInput);
    let err = t.repo.switch("--orphan=x").await.unwrap_err();
    assert_eq!(err.code, GitErrorCode::InvalidInput);
}

// ----- Staging parsial -----

fn op(a_from: u32, a_to: u32, b_from: u32, b_to: u32) -> SpliceOp {
    SpliceOp { a_from, a_to, b_from, b_to }
}

#[tokio::test]
async fn stage_one_hunk_then_unstage_and_discard_lines() {
    let t = TestRepo::with_commit().await;
    t.commit_file("f.txt", "1\n2\n3\n4\n5\n6\n", "Dasar");
    t.write("f.txt", "1\nDUA\n3\n4\n5\nENAM\nTUJUH\n");

    // Stage hanya perubahan pertama (baris 2).
    let d = t.repo.diff_contents("f.txt", None, &DiffSource::Worktree).await.unwrap();
    assert_eq!(d.a.text.as_deref(), Some("1\n2\n3\n4\n5\n6\n"));
    t.repo.apply_splice(SpliceTarget::Stage, "f.txt", &[op(1, 2, 1, 2)], &d.a.fingerprint, &d.b.fingerprint).await.unwrap();
    assert_eq!(t.index("f.txt"), "1\nDUA\n3\n4\n5\n6\n");
    assert_eq!(t.read("f.txt"), "1\nDUA\n3\n4\n5\nENAM\nTUJUH\n", "working tree tidak boleh berubah");

    // Sidik lama kini usang.
    let err = t
        .repo
        .apply_splice(SpliceTarget::Stage, "f.txt", &[op(5, 6, 5, 7)], &d.a.fingerprint, &d.b.fingerprint)
        .await
        .unwrap_err();
    assert_eq!(err.code, GitErrorCode::Stale);

    // Unstage kembali perubahan itu.
    let d = t.repo.diff_contents("f.txt", None, &DiffSource::Staged).await.unwrap();
    t.repo.apply_splice(SpliceTarget::Unstage, "f.txt", &[op(1, 2, 1, 2)], &d.a.fingerprint, &d.b.fingerprint).await.unwrap();
    assert_eq!(t.index("f.txt"), "1\n2\n3\n4\n5\n6\n");

    // Discard hanya perubahan kedua dari working tree.
    let d = t.repo.diff_contents("f.txt", None, &DiffSource::Worktree).await.unwrap();
    t.repo.apply_splice(SpliceTarget::Discard, "f.txt", &[op(5, 6, 5, 7)], &d.a.fingerprint, &d.b.fingerprint).await.unwrap();
    assert_eq!(t.read("f.txt"), "1\nDUA\n3\n4\n5\n6\n");
}

#[tokio::test]
async fn stage_part_of_an_untracked_file_and_unstage_it_again() {
    let t = TestRepo::with_commit().await;
    t.write("baru.txt", "satu\ndua\ntiga\n");

    let d = t.repo.diff_contents("baru.txt", None, &DiffSource::Worktree).await.unwrap();
    assert!(!d.a.exists);
    // Hanya baris "dua".
    t.repo.apply_splice(SpliceTarget::Stage, "baru.txt", &[op(0, 0, 1, 2)], &d.a.fingerprint, &d.b.fingerprint).await.unwrap();
    assert_eq!(t.index("baru.txt"), "dua\n");
    let s = t.repo.snapshot().await.unwrap();
    assert_eq!(names(&s.staged), vec![("baru.txt", ChangeKind::Added)]);
    assert_eq!(names(&s.unstaged), vec![("baru.txt", ChangeKind::Modified)]);

    // Mengeluarkan seluruh isinya membuat berkas kembali untracked.
    let d = t.repo.diff_contents("baru.txt", None, &DiffSource::Staged).await.unwrap();
    assert!(!d.a.exists);
    t.repo.apply_splice(SpliceTarget::Unstage, "baru.txt", &[op(0, 1, 0, 2)], &d.a.fingerprint, &d.b.fingerprint).await.unwrap();
    let s = t.repo.snapshot().await.unwrap();
    assert!(s.staged.is_empty());
    assert_eq!(s.untracked, vec!["baru.txt"]);
}

#[tokio::test]
async fn partial_staging_handles_missing_final_newline() {
    let t = TestRepo::with_commit().await;
    t.write("f.txt", "a\nb");
    t.git(&["add", "f.txt"]);
    t.git(&["commit", "-q", "-m", "Tanpa newline di akhir"]);
    t.write("f.txt", "A\nb\nc\n");

    let d = t.repo.diff_contents("f.txt", None, &DiffSource::Worktree).await.unwrap();
    // Hanya penambahan di akhir (baris "b" tanpa newline → "b\n" + "c"), bukan perubahan baris pertama.
    t.repo.apply_splice(SpliceTarget::Stage, "f.txt", &[op(2, 2, 2, 4)], &d.a.fingerprint, &d.b.fingerprint).await.unwrap();
    assert_eq!(t.index("f.txt"), "a\nb\nc\n");
}

#[tokio::test]
async fn partial_staging_keeps_executable_mode() {
    let t = TestRepo::with_commit().await;
    t.write("run.sh", "#!/bin/sh\necho satu\n");
    t.git(&["add", "run.sh"]);
    t.git(&["update-index", "--chmod=+x", "run.sh"]);
    t.git(&["commit", "-q", "-m", "Skrip"]);
    t.write("run.sh", "#!/bin/sh\necho satu\necho dua\n");
    let d = t.repo.diff_contents("run.sh", None, &DiffSource::Worktree).await.unwrap();
    t.repo.apply_splice(SpliceTarget::Stage, "run.sh", &[op(2, 2, 2, 3)], &d.a.fingerprint, &d.b.fingerprint).await.unwrap();
    assert!(t.git(&["ls-files", "-s", "run.sh"]).starts_with("100755 "));
    assert_eq!(t.index("run.sh"), "#!/bin/sh\necho satu\necho dua\n");
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

// ----- Commit dan riwayat -----

#[tokio::test]
async fn commit_amend_and_undo() {
    let t = TestRepo::new().await;
    t.write("a.txt", "a\n");
    t.repo.stage_all().await.unwrap();
    let first = t.repo.commit("Pertama\n\nBadan pesan.", &CommitOptions::default()).await.unwrap();
    assert_eq!(first.len(), 40);
    assert_eq!(t.repo.last_commit_message().await.unwrap(), "Pertama\n\nBadan pesan.");

    let err = t.repo.commit("Kosong", &CommitOptions::default()).await.unwrap_err();
    assert_eq!(err.code, GitErrorCode::NothingToCommit);
    assert_eq!(t.repo.commit("  ", &CommitOptions::default()).await.unwrap_err().code, GitErrorCode::InvalidInput);

    t.write("b.txt", "b\n");
    t.repo.stage_all().await.unwrap();
    let amended = t.repo.commit("Pertama (diubah)", &CommitOptions { amend: true, signoff: true, ..Default::default() }).await.unwrap();
    assert_ne!(amended, first);
    let message = t.repo.last_commit_message().await.unwrap();
    assert!(message.starts_with("Pertama (diubah)") && message.contains("Signed-off-by: Penguji"));
    assert_eq!(t.git(&["rev-list", "--count", "HEAD"]).trim(), "1");

    t.write("c.txt", "c\n");
    t.repo.stage_all().await.unwrap();
    t.repo.commit("Kedua", &CommitOptions::default()).await.unwrap();
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
    assert_eq!(detail.commit.subject, "Commit pertama");
    assert_eq!(detail.files.len(), 1);
    assert_eq!((detail.files[0].path.as_str(), detail.files[0].kind, detail.files[0].additions), ("README.md", ChangeKind::Added, Some(1)));

    t.commit_file("a.txt", "1\n2\n", "Tambah a");
    t.git(&["mv", "a.txt", "b.txt"]);
    t.write("README.md", "# Proyek\nbaris\n");
    t.git(&["add", "-A"]);
    t.git(&["commit", "-q", "-m", "Rename dan ubah\n\nBadan."]);
    let head = t.git(&["rev-parse", "HEAD"]).trim().to_string();
    let detail = t.repo.commit_detail(&head).await.unwrap();
    assert_eq!(detail.body, "Rename dan ubah\n\nBadan.");
    let renamed = detail.files.iter().find(|f| f.path == "b.txt").unwrap();
    assert_eq!((renamed.kind, renamed.orig_path.as_deref()), (ChangeKind::Renamed, Some("a.txt")));
    let readme = detail.files.iter().find(|f| f.path == "README.md").unwrap();
    assert_eq!((readme.additions, readme.deletions), (Some(1), Some(0)));

    // Merge commit dibandingkan terhadap induk pertama.
    t.git(&["switch", "-q", "-c", "fitur"]);
    t.commit_file("fitur.txt", "f\n", "Fitur");
    t.git(&["switch", "-q", "main"]);
    t.commit_file("utama.txt", "u\n", "Utama");
    t.git(&["merge", "-q", "--no-ff", "--no-edit", "fitur"]);
    let merge = t.git(&["rev-parse", "HEAD"]).trim().to_string();
    let detail = t.repo.commit_detail(&merge).await.unwrap();
    assert_eq!(detail.commit.parents.len(), 2);
    assert_eq!(detail.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["fitur.txt"]);

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
    t.git(&["switch", "-q", "-c", "sisi", "HEAD~3"]);
    t.commit_file("sisi.txt", "s\n", "Di sisi");
    t.git(&["switch", "-q", "main"]);

    let mut stream = t.repo.log_stream(&LogQuery { all: true, ..Default::default() }).unwrap();
    let first = stream.next_page(4).await.unwrap();
    assert_eq!(first.rows.len(), 4);
    assert!(!first.done);
    let rest = stream.next_page(100).await.unwrap();
    assert!(rest.done);
    let all: Vec<_> = first.rows.iter().chain(rest.rows.iter()).collect();
    assert_eq!(all.len(), 9);
    assert!(all.iter().any(|r| r.column == 1), "branch sisi harus berada di jalur kedua");
    let tip = all.iter().find(|r| r.commit.subject == "Commit 6").unwrap();
    assert!(tip.commit.refs.iter().any(|r| r.kind == RefKind::Head));
    assert!(tip.commit.refs.iter().any(|r| r.kind == RefKind::Branch && r.name == "main"));
    stream.close().await;

    let found = t.repo.log(&LogQuery { grep: Some("di SISI".into()), all: true, ..Default::default() }, 10).await.unwrap();
    assert_eq!(found.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), vec!["Di sisi"]);
    let history = t.repo.log(&LogQuery { path: Some("n.txt".into()), ..Default::default() }, 3).await.unwrap();
    assert_eq!(history.len(), 3);
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
    t.repo.create_branch("fitur/login", None, true).await.unwrap();
    t.commit_file("login.txt", "x\n", "Login");
    t.repo.switch("main").await.unwrap();
    t.repo.create_branch("lain", Some("fitur/login"), false).await.unwrap();

    let refs = t.repo.refs().await.unwrap();
    let branch_names: Vec<_> = refs.branches.iter().map(|b| (b.name.as_str(), b.is_head)).collect();
    assert_eq!(branch_names, vec![("fitur/login", false), ("lain", false), ("main", true)]);
    assert_eq!(refs.branches[0].subject, "Login");

    t.repo.rename_branch("lain", "lain-2").await.unwrap();
    assert_eq!(t.repo.unmerged_count("fitur/login").await.unwrap(), 1);
    let err = t.repo.delete_branch("fitur/login", false).await.unwrap_err();
    assert_eq!(err.code, GitErrorCode::BranchNotMerged);
    t.repo.delete_branch("fitur/login", true).await.unwrap();
    t.repo.delete_branch("lain-2", true).await.unwrap();
    assert_eq!(t.repo.refs().await.unwrap().branches.len(), 1);
}

#[tokio::test]
async fn switching_with_conflicting_local_changes_is_reported_as_dirty() {
    let t = TestRepo::with_commit().await;
    t.repo.create_branch("lain", None, true).await.unwrap();
    t.commit_file("README.md", "# Lain\n", "Ubah di lain");
    t.repo.switch("main").await.unwrap();
    t.write("README.md", "# Lokal\n");
    let err = t.repo.switch("lain").await.unwrap_err();
    assert_eq!(err.code, GitErrorCode::DirtyWorkTree);
    assert_eq!(t.read("README.md"), "# Lokal\n", "perubahan lokal tidak boleh hilang");
}

// ----- Merge dan konflik -----

async fn conflicted_merge() -> TestRepo {
    let t = TestRepo::with_commit().await;
    t.commit_file("config.toml", "timeout = 30\nretries = 3\nlog = \"info\"\n", "Konfigurasi");
    t.git(&["switch", "-q", "-c", "fitur"]);
    t.commit_file("config.toml", "timeout = 60\nretries = 5\nlog = \"info\"\n", "Fitur ubah retries");
    t.git(&["switch", "-q", "main"]);
    t.commit_file("config.toml", "timeout = 30\nretries = 4\nlog = \"debug\"\n", "Main ubah retries");
    t
}

#[tokio::test]
async fn merge_conflict_is_detected_and_resolved_through_editor_flow() {
    let t = conflicted_merge().await;
    let err = t.repo.merge("fitur", MergeMode::Default).await.unwrap_err();
    assert_eq!(err.code, GitErrorCode::Conflict);

    let s = t.repo.snapshot().await.unwrap();
    assert_eq!(s.operation, Some(Operation::Merge { incoming: "fitur".into() }));
    assert_eq!(s.conflicted, vec![ConflictEntry { path: "config.toml".into(), kind: ConflictKind::BothModified }]);

    let detail = t.repo.conflict_detail("config.toml").await.unwrap();
    assert_eq!((detail.current_label.as_str(), detail.incoming_label.as_str()), ("main", "fitur"));
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

    // Editor konflik menulis hasil lalu menandainya selesai.
    t.write("config.toml", "timeout = 60\nretries = 5\nlog = \"debug\"\n");
    t.repo.mark_resolved("config.toml").await.unwrap();
    assert!(t.repo.snapshot().await.unwrap().conflicted.is_empty());
    t.repo.operation(OperationAction::Continue).await.unwrap();

    let s = t.repo.snapshot().await.unwrap();
    assert!(s.operation.is_none());
    assert_eq!(t.git(&["rev-list", "--parents", "-n", "1", "HEAD"]).split_whitespace().count(), 3);
}

#[tokio::test]
async fn merge_can_be_aborted_and_resolved_by_side() {
    let t = conflicted_merge().await;
    let before = t.git(&["rev-parse", "HEAD"]);
    t.repo.merge("fitur", MergeMode::Default).await.unwrap_err();
    t.repo.operation(OperationAction::Abort).await.unwrap();
    assert!(t.repo.snapshot().await.unwrap().operation.is_none());
    assert_eq!(t.git(&["rev-parse", "HEAD"]), before);

    t.repo.merge("fitur", MergeMode::Default).await.unwrap_err();
    t.repo.resolve_with_side("config.toml", ConflictSide::Incoming).await.unwrap();
    assert_eq!(t.read("config.toml"), "timeout = 60\nretries = 5\nlog = \"info\"\n");
    assert!(t.repo.snapshot().await.unwrap().conflicted.is_empty());
}

#[tokio::test]
async fn delete_modify_conflict_can_keep_or_delete() {
    let t = TestRepo::with_commit().await;
    t.commit_file("f.txt", "a\n", "Tambah f");
    t.git(&["switch", "-q", "-c", "hapus"]);
    t.git(&["rm", "-q", "f.txt"]);
    t.git(&["commit", "-q", "-m", "Hapus f"]);
    t.git(&["switch", "-q", "main"]);
    t.commit_file("f.txt", "a\nb\n", "Ubah f");

    t.repo.merge("hapus", MergeMode::Default).await.unwrap_err();
    let s = t.repo.snapshot().await.unwrap();
    assert_eq!(s.conflicted[0].kind, ConflictKind::DeletedByThem);
    let detail = t.repo.conflict_detail("f.txt").await.unwrap();
    assert_eq!(detail.kind, ConflictKind::DeletedByThem);
    assert!(detail.incoming.is_none() && detail.segments.is_empty());

    // Memilih sisi incoming berarti menghapus berkas.
    t.repo.resolve_with_side("f.txt", ConflictSide::Incoming).await.unwrap();
    assert!(!t.file("f.txt").exists());
    assert!(t.repo.snapshot().await.unwrap().conflicted.is_empty());
}

#[tokio::test]
async fn fast_forward_only_merge_is_refused_when_diverged() {
    let t = conflicted_merge().await;
    assert!(t.repo.merge("fitur", MergeMode::FfOnly).await.is_err());
    assert!(t.repo.snapshot().await.unwrap().operation.is_none());
}

// ----- Rebase -----

#[tokio::test]
async fn rebase_conflict_reports_progress_and_can_be_aborted() {
    let t = conflicted_merge().await;
    t.git(&["switch", "-q", "fitur"]);
    t.commit_file("lain.txt", "x\n", "Commit kedua di fitur");
    let before = t.git(&["rev-parse", "HEAD"]);

    let err = t.repo.rebase("main").await.unwrap_err();
    assert_eq!(err.code, GitErrorCode::Conflict);
    let s = t.repo.snapshot().await.unwrap();
    match s.operation {
        Some(Operation::Rebase { step, total, head_name, interactive, .. }) => {
            assert_eq!((step, total, head_name.as_str(), interactive), (1, 2, "fitur", false));
        }
        other => panic!("seharusnya rebase, ternyata {other:?}"),
    }
    let detail = t.repo.conflict_detail("config.toml").await.unwrap();
    assert!(detail.current_label.starts_with("new base"));
    assert!(detail.incoming_label.starts_with("fitur"));

    t.repo.operation(OperationAction::Abort).await.unwrap();
    assert_eq!(t.git(&["rev-parse", "HEAD"]), before);
    assert_eq!(t.repo.snapshot().await.unwrap().head, Head::Branch { name: "fitur".into() });
}

#[tokio::test]
async fn rebase_continue_after_resolving() {
    let t = conflicted_merge().await;
    t.git(&["switch", "-q", "fitur"]);
    t.repo.rebase("main").await.unwrap_err();
    t.write("config.toml", "timeout = 60\nretries = 5\nlog = \"debug\"\n");
    t.repo.mark_resolved("config.toml").await.unwrap();
    t.repo.operation(OperationAction::Continue).await.unwrap();
    assert!(t.repo.snapshot().await.unwrap().operation.is_none());
    assert_eq!(t.git(&["log", "-1", "--format=%s"]).trim(), "Fitur ubah retries");
    assert_eq!(t.git(&["merge-base", "HEAD", "main"]), t.git(&["rev-parse", "main"]));
}

#[tokio::test]
async fn merge_base_finds_common_ancestor() {
    let t = conflicted_merge().await;
    let base = t.git(&["merge-base", "main", "fitur"]).trim().to_string();
    assert_eq!(t.repo.merge_base("main", "fitur").await.unwrap().as_deref(), Some(base.as_str()));
    // Riwayat tanpa leluhur bersama.
    t.git(&["switch", "-q", "--orphan", "lepas"]);
    t.commit_file("x.txt", "x\n", "Akar lain");
    assert_eq!(t.repo.merge_base("main", "lepas").await.unwrap(), None);
    assert!(t.repo.merge_base("main", "tidak-ada").await.is_err());
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
    t.write("README.md", "# Ubah\n");
    t.write("baru.txt", "baru\n");
    t.repo.stash_push(&StashOptions { message: Some("kerja setengah jadi".into()), include_untracked: true, ..Default::default() }).await.unwrap();
    assert_eq!(t.read("README.md"), "# Proyek\n");
    assert!(!t.file("baru.txt").exists());

    let list = t.repo.stash_list().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "stash@{0}");
    assert!(list[0].message.contains("kerja setengah jadi"));
    assert_eq!(t.repo.snapshot().await.unwrap().stash_count, 1);

    let mut files: Vec<_> = t.repo.stash_files("stash@{0}").await.unwrap().into_iter().map(|f| f.path).collect();
    files.sort();
    assert_eq!(files, vec!["README.md", "baru.txt"]);
    let d = t.repo.diff_contents("baru.txt", None, &DiffSource::Stash { name: "stash@{0}".into() }).await.unwrap();
    assert_eq!(d.b.text.as_deref(), Some("baru\n"));

    t.repo.stash_apply("stash@{0}", false).await.unwrap();
    assert_eq!(t.read("README.md"), "# Ubah\n");
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
    t.repo.tag_create("v1.0.0", None, Some("Rilis pertama")).await.unwrap();
    t.repo.tag_create("ringan", Some(&head), None).await.unwrap();
    let tags = t.repo.refs().await.unwrap().tags;
    let annotated = tags.iter().find(|t| t.name == "v1.0.0").unwrap();
    assert!(annotated.annotated);
    assert_eq!(annotated.message, "Rilis pertama");
    assert_eq!(annotated.oid, head, "annotated tag harus di-peel ke commit");
    let light = tags.iter().find(|t| t.name == "ringan").unwrap();
    assert!(!light.annotated && light.oid == head);
    t.repo.tag_delete("ringan").await.unwrap();
    assert_eq!(t.repo.refs().await.unwrap().tags.len(), 1);
}

#[tokio::test]
async fn cherry_pick_revert_and_reset() {
    let t = TestRepo::with_commit().await;
    t.git(&["switch", "-q", "-c", "sisi"]);
    let picked = t.commit_file("petik.txt", "p\n", "Untuk dipetik");
    t.git(&["switch", "-q", "main"]);

    t.repo.cherry_pick(&[picked.clone()]).await.unwrap();
    assert_eq!(t.read("petik.txt"), "p\n");
    assert_eq!(t.git(&["log", "-1", "--format=%s"]).trim(), "Untuk dipetik");

    let head = t.git(&["rev-parse", "HEAD"]).trim().to_string();
    t.repo.revert(&head).await.unwrap();
    assert!(!t.file("petik.txt").exists());
    assert!(t.git(&["log", "-1", "--format=%s"]).starts_with("Revert"));

    t.write("kotor.txt", "k\n");
    t.repo.stage_all().await.unwrap();
    t.repo.reset(&head, ResetMode::Mixed).await.unwrap();
    assert_eq!(t.git(&["rev-parse", "HEAD"]).trim(), head);
    assert!(t.file("petik.txt").exists() == false, "reset mixed tidak mengubah working tree");
    t.repo.reset(&head, ResetMode::Hard).await.unwrap();
    assert_eq!(t.read("petik.txt"), "p\n");

    let reflog = t.repo.reflog(20).await.unwrap();
    assert!(reflog.len() >= 4);
    assert_eq!(reflog[0].selector, "HEAD@{0}");
    assert!(reflog.iter().any(|e| e.message.contains("cherry-pick")));
}

#[tokio::test]
async fn cherry_pick_conflict_can_be_aborted() {
    let t = conflicted_merge().await;
    let incoming = t.git(&["rev-parse", "fitur"]).trim().to_string();
    let err = t.repo.cherry_pick(&[incoming]).await.unwrap_err();
    assert_eq!(err.code, GitErrorCode::Conflict);
    assert!(matches!(t.repo.snapshot().await.unwrap().operation, Some(Operation::CherryPick { .. })));
    t.repo.operation(OperationAction::Abort).await.unwrap();
    assert!(t.repo.snapshot().await.unwrap().operation.is_none());
}

// ----- Worktree dan submodule -----

#[tokio::test]
async fn worktree_add_list_remove() {
    let t = TestRepo::with_commit().await;
    let extra = tempfile::tempdir().unwrap();
    let wt_path = extra.path().join("wt fitur");
    t.repo.worktree_add(&wt_path, Some("fitur"), None).await.unwrap();

    let list = t.repo.worktrees().await.unwrap();
    assert_eq!(list.len(), 2);
    assert!(list[0].is_main && list[0].is_current);
    assert_eq!(list[1].branch.as_deref(), Some("fitur"));
    let refs = t.repo.refs().await.unwrap();
    let fitur = refs.branches.iter().find(|b| b.name == "fitur").unwrap();
    assert!(fitur.worktree.as_deref().is_some_and(|p| p.ends_with("wt fitur")));

    // Worktree memiliki direktori git sendiri untuk HEAD dan penanda operasi.
    let wt = Repo::open(t.git.clone(), &wt_path).await.unwrap();
    assert_ne!(wt.git_dir(), wt.common_dir());
    assert_eq!(wt.snapshot().await.unwrap().head, Head::Branch { name: "fitur".into() });

    std::fs::write(wt_path.join("kotor.txt"), "x").unwrap();
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
    t.git(&["commit", "-q", "-m", "Tambah submodule"]);

    let subs = t.repo.submodules().await.unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!((subs[0].path.as_str(), subs[0].status), ("vendor/lib", SubmoduleStatus::UpToDate));

    // Clone baru: submodule belum diinisialisasi sampai di-update.
    let clone_dir = tempfile::tempdir().unwrap();
    raw_git(clone_dir.path(), &["clone", "-q", t.path().to_str().unwrap(), "salinan"]);
    let clone = Repo::open(t.git.clone(), &clone_dir.path().join("salinan")).await.unwrap();
    assert_eq!(clone.submodules().await.unwrap()[0].status, SubmoduleStatus::Uninitialized);
    clone.submodule_update(&[], true, None, None).await.unwrap();
    assert_eq!(clone.submodules().await.unwrap()[0].status, SubmoduleStatus::UpToDate);
    assert!(clone.root().join("vendor/lib/README.md").exists());

    // Commit baru di dalam submodule terlihat sebagai perubahan di superproject.
    let sub_path = clone.root().join("vendor/lib");
    std::fs::write(sub_path.join("baru.txt"), "x").unwrap();
    raw_git(&sub_path, &["add", "-A"]);
    raw_git(&sub_path, &["commit", "-q", "-m", "Di dalam submodule"]);
    assert_eq!(clone.submodules().await.unwrap()[0].status, SubmoduleStatus::Modified);
    let s = clone.snapshot().await.unwrap();
    assert!(s.unstaged[0].submodule.as_ref().is_some_and(|m| m.commit_changed));
}

// ----- Blame -----

#[tokio::test]
async fn blame_attributes_lines_and_marks_unsaved_changes() {
    let t = TestRepo::with_commit().await;
    let first = t.commit_file("f.txt", "satu\ndua\ntiga\n", "Awal");
    let second = t.commit_file("f.txt", "satu\nDUA\ntiga\n", "Ubah baris dua");

    let ranges = t.repo.blame("f.txt", None).await.unwrap();
    let owner = |line: u32| ranges.iter().find(|r| line >= r.start_line && line < r.start_line + r.line_count).unwrap();
    assert_eq!(owner(1).oid, first);
    assert_eq!(owner(2).oid, second);
    assert_eq!(owner(2).summary, "Ubah baris dua");
    assert_eq!(owner(3).oid, first);
    assert_eq!(owner(1).author, "Penguji");

    // Isi buffer yang belum disimpan: baris baru ditandai belum di-commit.
    let ranges = t.repo.blame("f.txt", Some(b"satu\nDUA\nsisipan\ntiga\n".to_vec())).await.unwrap();
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

    // Penulis pertama: clone repositori kosong, commit, lalu publish branch.
    let git = Git::discover(None).await.unwrap();
    let a_path = base.path().join("a");
    git.clone_repo(&remote_url, &a_path, false, None, None).await.unwrap();
    let a = Repo::open(git.clone(), &a_path).await.unwrap();
    std::fs::write(a_path.join("a.txt"), "a\n").unwrap();
    a.stage_all().await.unwrap();
    a.commit("Dari A", &CommitOptions::default()).await.unwrap();

    let s = a.snapshot().await.unwrap();
    assert!(s.upstream.as_ref().is_none_or(|u| !u.exists), "branch belum dipublikasikan");
    a.push(&PushOptions { set_upstream: true, ..Default::default() }, None, None).await.unwrap();
    let s = a.snapshot().await.unwrap();
    let up = s.upstream.unwrap();
    assert_eq!((up.name.as_str(), up.ahead, up.behind, up.exists), ("origin/main", 0, 0, true));

    // Penulis kedua: clone, commit, push.
    let b_path = base.path().join("b");
    git.clone_repo(&remote_url, &b_path, false, None, None).await.unwrap();
    let b = Repo::open(git.clone(), &b_path).await.unwrap();
    std::fs::write(b_path.join("b.txt"), "b\n").unwrap();
    b.stage_all().await.unwrap();
    b.commit("Dari B", &CommitOptions::default()).await.unwrap();
    b.push(&PushOptions::default(), None, None).await.unwrap();

    // A tertinggal: fetch memperlihatkan behind, push ditolak setelah commit lokal, pull menyelesaikan.
    a.fetch(None, None, None).await.unwrap();
    assert_eq!(a.snapshot().await.unwrap().upstream.unwrap().behind, 1);
    std::fs::write(a_path.join("a2.txt"), "a2\n").unwrap();
    a.stage_all().await.unwrap();
    a.commit("A lagi", &CommitOptions::default()).await.unwrap();
    let err = a.push(&PushOptions::default(), None, None).await.unwrap_err();
    assert_eq!(err.code, GitErrorCode::PushRejected);

    assert!(a.pull(PullStrategy::FfOnly, None, None).await.is_err(), "ff-only harus gagal saat riwayat bercabang");
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

    // Tag: push lalu hapus di remote.
    a.tag_create("v1", None, None).await.unwrap();
    a.tag_push("origin", "v1").await.unwrap();
    assert!(raw_git(&base.path().join("remote.git"), &["tag"]).contains("v1"));
    a.tag_delete_remote("origin", "v1").await.unwrap();
    assert!(!raw_git(&base.path().join("remote.git"), &["tag"]).contains("v1"));

    // Branch remote: checkout melacak, lalu hapus di remote.
    a.create_branch("topik", None, true).await.unwrap();
    a.push(&PushOptions { set_upstream: true, ..Default::default() }, None, None).await.unwrap();
    b.fetch(Some("origin"), None, None).await.unwrap();
    b.switch_tracking("origin/topik").await.unwrap();
    assert_eq!(b.snapshot().await.unwrap().upstream.unwrap().name, "origin/topik");
    a.delete_remote_branch("origin", "topik").await.unwrap();
    b.fetch(None, None, None).await.unwrap();
    let topik = b.refs().await.unwrap().branches.into_iter().find(|br| br.name == "topik").unwrap();
    assert!(topik.upstream_gone);
}

#[tokio::test]
async fn remotes_can_be_managed() {
    let t = TestRepo::with_commit().await;
    t.repo.remote_add("origin", "https://contoh.id/a/b.git").await.unwrap();
    t.repo.remote_set_url("origin", "git@contoh.id:a/b.git").await.unwrap();
    t.repo.remote_rename("origin", "hulu").await.unwrap();
    let remotes = t.repo.remotes().await.unwrap();
    assert_eq!(remotes, vec![Remote { name: "hulu".into(), fetch_url: "git@contoh.id:a/b.git".into(), push_url: "git@contoh.id:a/b.git".into() }]);
    t.repo.remote_remove("hulu").await.unwrap();
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
