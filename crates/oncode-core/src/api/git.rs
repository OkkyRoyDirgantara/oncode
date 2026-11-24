//! API Git. Setiap fungsi menerima `root` (akar worktree yang sudah dibuka).
//! Fungsi yang mengubah repositori selalu menjadwalkan penyiaran snapshot baru,
//! juga saat gagal, karena operasi yang gagal bisa meninggalkan keadaan baru (mis. konflik).

use std::future::Future;
use std::path::PathBuf;

use oncode_git::*;

use crate::error::{AppError, AppResult};
use crate::state::Shared;

/// Menjalankan operasi baca.
async fn read<T, Fut>(core: &Shared, root: &str, f: impl FnOnce(Repo) -> Fut) -> AppResult<T>
where
    Fut: Future<Output = oncode_git::Result<T>> + Send,
{
    f(core.repo(root)?).await.map_err(Into::into)
}

/// Menjalankan operasi tulis lalu menjadwalkan snapshot baru. `refs` juga menyiarkan perubahan ref.
async fn write<T, Fut>(core: &Shared, root: &str, refs: bool, f: impl FnOnce(Repo) -> Fut) -> AppResult<T>
where
    Fut: Future<Output = oncode_git::Result<T>> + Send,
{
    let repo = core.repo(root)?;
    let result = f(repo).await;
    core.refresh(root);
    if refs {
        core.refresh_refs(root);
    }
    result.map_err(Into::into)
}

// ----- Repositori -----

/// Membuka repositori yang memuat `path` (mis. submodule) dan mengembalikan akarnya.
pub async fn git_open(core: &Shared, window: &str, path: String) -> AppResult<String> {
    let repo = core.open_repo(window, &PathBuf::from(path)).await?;
    let root = repo.root_string();
    core.refresh(&root);
    Ok(root)
}

pub async fn git_snapshot(core: &Shared, root: String) -> AppResult<RepoSnapshot> {
    read(core, &root, |repo| async move { repo.snapshot().await }).await
}

pub async fn git_refs(core: &Shared, root: String) -> AppResult<Refs> {
    read(core, &root, |repo| async move { repo.refs().await }).await
}

pub async fn git_init(core: &Shared, path: String) -> AppResult<()> {
    // Folder yang sudah berisi repositori tidak boleh menjadi dipercaya lewat jalur ini.
    let target = PathBuf::from(&path);
    if target.join(".git").exists() && !core.store.is_trusted(&target) {
        return Err(AppError::untrusted());
    }
    let git = core.git().await?;
    git.init(&PathBuf::from(&path)).await?;
    core.store.set_trusted(&path, true);
    Ok(())
}

/// Meng-clone repositori; folder hasilnya langsung dipercaya. Mengembalikan path tujuan.
pub async fn git_clone(
    core: &Shared,
    url: String,
    parent_dir: String,
    name: String,
    recurse_submodules: bool,
    operation_id: String,
    on_progress: ProgressFn,
) -> AppResult<String> {
    if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
        return Err(AppError::invalid("Invalid destination folder name"));
    }
    let dest = PathBuf::from(&parent_dir).join(&name);
    if dest.exists() && dest.read_dir().is_ok_and(|mut d| d.next().is_some()) {
        return Err(AppError::new("alreadyExists", format!("Destination folder already exists and is not empty: {}", dest.display())));
    }
    let git = core.git().await?;
    let cancel = core.begin_operation(&operation_id);
    let result = git.clone_repo(&url, &dest, recurse_submodules, Some(cancel), Some(on_progress)).await;
    core.end_operation(&operation_id);
    result?;
    let dest = dest.to_string_lossy().into_owned();
    core.store.set_trusted(&dest, true);
    Ok(dest)
}

// ----- Staging -----

pub async fn git_stage(core: &Shared, root: String, paths: Vec<String>) -> AppResult<()> {
    write(core, &root, false, |repo| async move { repo.stage(&paths).await }).await
}

pub async fn git_stage_all(core: &Shared, root: String) -> AppResult<()> {
    write(core, &root, false, |repo| async move { repo.stage_all().await }).await
}

pub async fn git_unstage(core: &Shared, root: String, paths: Vec<String>) -> AppResult<()> {
    write(core, &root, false, |repo| async move { repo.unstage(&paths).await }).await
}

pub async fn git_unstage_all(core: &Shared, root: String) -> AppResult<()> {
    write(core, &root, false, |repo| async move { repo.unstage_all().await }).await
}

/// Membuang perubahan: berkas terlacak dikembalikan ke isi index, berkas untracked
/// dipindahkan ke tempat sampah OS (bukan dihapus permanen).
pub async fn git_discard(core: &Shared, root: String, tracked: Vec<String>, untracked: Vec<String>) -> AppResult<()> {
    let repo = core.repo(&root)?;
    let mut to_trash = Vec::with_capacity(untracked.len());
    for path in &untracked {
        let full = repo.root().join(path);
        if !oncode_workspace::is_inside(repo.root(), &full) {
            return Err(AppError::new("invalidPath", format!("Path is outside the repository: {path}")));
        }
        to_trash.push(full);
    }
    let result = async {
        if !tracked.is_empty() {
            repo.discard(&tracked).await?;
        }
        if !to_trash.is_empty() {
            oncode_workspace::trash(&to_trash).await?;
        }
        Ok::<(), AppError>(())
    }
    .await;
    core.refresh(&root);
    result
}

pub async fn git_diff(core: &Shared, root: String, path: String, orig_path: Option<String>, source: DiffSource) -> AppResult<DiffContents> {
    read(core, &root, |repo| async move { repo.diff_contents(&path, orig_path.as_deref(), &source).await }).await
}

/// Stage, unstage, atau discard sebagian perubahan sebuah berkas (per hunk atau per baris).
pub async fn git_splice(
    core: &Shared,
    root: String,
    target: SpliceTarget,
    path: String,
    ops: Vec<SpliceOp>,
    fingerprint_a: String,
    fingerprint_b: String,
) -> AppResult<()> {
    write(core, &root, false, |repo| async move { repo.apply_splice(target, &path, &ops, &fingerprint_a, &fingerprint_b).await }).await
}

/// Isi sebuah berkas pada revisi tertentu (mis. `HEAD`), untuk penanda perubahan di gutter editor.
pub async fn git_file_at(core: &Shared, root: String, rev: String, path: String) -> AppResult<Option<String>> {
    let repo = core.repo(&root)?;
    if rev.starts_with('-') || rev.contains(':') || path.contains('\0') {
        return Err(AppError::invalid("Invalid revision or path"));
    }
    let blob = repo.blob(&format!("{rev}:{path}")).await?;
    Ok(blob.filter(|b| !oncode_git::content::is_binary(b)).map(|b| String::from_utf8_lossy(&b).into_owned()))
}

// ----- Commit -----

pub async fn git_commit(core: &Shared, root: String, message: String, options: CommitOptions) -> AppResult<String> {
    write(core, &root, true, |repo| async move { repo.commit(&message, &options).await }).await
}

pub async fn git_undo_commit(core: &Shared, root: String) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.undo_last_commit().await }).await
}

pub async fn git_last_message(core: &Shared, root: String) -> AppResult<String> {
    read(core, &root, |repo| async move { repo.last_commit_message().await }).await
}

pub async fn git_identity(core: &Shared, root: String) -> AppResult<Identity> {
    Ok(core.repo(&root)?.identity().await)
}

pub async fn git_set_identity(core: &Shared, root: String, name: String, email: String, global: bool) -> AppResult<()> {
    write(core, &root, false, |repo| async move { repo.set_identity(&name, &email, global).await }).await
}

// ----- Branch -----

pub async fn git_switch(core: &Shared, root: String, branch: String) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.switch(&branch).await }).await
}

pub async fn git_switch_detached(core: &Shared, root: String, rev: String) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.switch_detached(&rev).await }).await
}

pub async fn git_switch_tracking(core: &Shared, root: String, remote_branch: String) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.switch_tracking(&remote_branch).await }).await
}

pub async fn git_branch_create(core: &Shared, root: String, name: String, start: Option<String>, checkout: bool) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.create_branch(&name, start.as_deref(), checkout).await }).await
}

pub async fn git_branch_rename(core: &Shared, root: String, name: String, new_name: String) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.rename_branch(&name, &new_name).await }).await
}

pub async fn git_branch_delete(core: &Shared, root: String, name: String, force: bool) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.delete_branch(&name, force).await }).await
}

/// Jumlah commit yang hanya ada di branch ini; ditampilkan sebelum menghapus paksa.
pub async fn git_branch_unmerged(core: &Shared, root: String, name: String) -> AppResult<u32> {
    read(core, &root, |repo| async move { repo.unmerged_count(&name).await }).await
}

pub async fn git_branch_set_upstream(core: &Shared, root: String, branch: String, upstream: Option<String>) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.set_upstream(&branch, upstream.as_deref()).await }).await
}

pub async fn git_branch_delete_remote(core: &Shared, root: String, remote: String, branch: String) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.delete_remote_branch(&remote, &branch).await }).await
}

// ----- Merge, rebase, operasi berlanjut -----

pub async fn git_merge(core: &Shared, root: String, rev: String, mode: MergeMode) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.merge(&rev, mode).await }).await
}

pub async fn git_rebase(core: &Shared, root: String, onto: String) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.rebase(&onto).await }).await
}

pub async fn git_rebase_plan(core: &Shared, root: String, base: Option<String>) -> AppResult<RebasePlan> {
    read(core, &root, |repo| async move { repo.rebase_plan(base.as_deref()).await }).await
}

pub async fn git_rebase_interactive(core: &Shared, root: String, base: Option<String>, items: Vec<RebaseTodoItem>) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.rebase_interactive(base.as_deref(), &items).await }).await
}

/// Continue / Skip / Abort untuk merge, rebase, cherry-pick, atau revert yang sedang berhenti.
pub async fn git_operation(core: &Shared, root: String, action: OperationAction) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.operation(action).await }).await
}

// ----- Remote -----

pub async fn git_fetch(
    core: &Shared,
    root: String,
    remote: Option<String>,
    operation_id: String,
    on_progress: ProgressFn,
) -> AppResult<()> {
    let cancel = core.begin_operation(&operation_id);
    let result = write(core, &root, true, |repo| async move { repo.fetch(remote.as_deref(), Some(cancel), Some(on_progress)).await }).await;
    core.end_operation(&operation_id);
    result
}

pub async fn git_pull(
    core: &Shared,
    root: String,
    strategy: PullStrategy,
    operation_id: String,
    on_progress: ProgressFn,
) -> AppResult<()> {
    let cancel = core.begin_operation(&operation_id);
    let result = write(core, &root, true, |repo| async move { repo.pull(strategy, Some(cancel), Some(on_progress)).await }).await;
    core.end_operation(&operation_id);
    result
}

pub async fn git_push(
    core: &Shared,
    root: String,
    options: PushOptions,
    operation_id: String,
    on_progress: ProgressFn,
) -> AppResult<()> {
    let cancel = core.begin_operation(&operation_id);
    let result = write(core, &root, true, |repo| async move { repo.push(&options, Some(cancel), Some(on_progress)).await }).await;
    core.end_operation(&operation_id);
    result
}

pub async fn git_remote_add(core: &Shared, root: String, name: String, url: String) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.remote_add(&name, &url).await }).await
}

pub async fn git_remote_remove(core: &Shared, root: String, name: String) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.remote_remove(&name).await }).await
}

pub async fn git_remote_rename(core: &Shared, root: String, name: String, new_name: String) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.remote_rename(&name, &new_name).await }).await
}

pub async fn git_remote_set_url(core: &Shared, root: String, name: String, url: String) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.remote_set_url(&name, &url).await }).await
}

// ----- Riwayat -----

/// Memulai `git log` bertahap untuk tampilan Graph; halaman diambil dengan `git_log_next`.
pub async fn git_log_open(core: &Shared, root: String, query: LogQuery) -> AppResult<u32> {
    let stream = core.repo(&root)?.log_stream(&query)?;
    let id = core.next_id();
    core.log_streams.lock().await.insert(id, stream);
    Ok(id)
}

pub async fn git_log_next(core: &Shared, stream_id: u32, count: u32) -> AppResult<LogPage> {
    let mut streams = core.log_streams.lock().await;
    let Some(stream) = streams.get_mut(&stream_id) else {
        return Ok(LogPage { rows: Vec::new(), done: true });
    };
    Ok(stream.next_page(count.clamp(1, 2000) as usize).await?)
}

pub async fn git_log_close(core: &Shared, stream_id: u32) -> AppResult<()> {
    if let Some(stream) = core.log_streams.lock().await.remove(&stream_id) {
        stream.close().await;
    }
    Ok(())
}

/// Daftar commit tanpa graph (riwayat berkas, pencarian).
pub async fn git_log(core: &Shared, root: String, query: LogQuery, limit: u32) -> AppResult<Vec<Commit>> {
    read(core, &root, |repo| async move { repo.log(&query, limit.clamp(1, 5000)).await }).await
}

pub async fn git_commit_detail(core: &Shared, root: String, oid: String) -> AppResult<CommitDetail> {
    read(core, &root, |repo| async move { repo.commit_detail(&oid).await }).await
}

pub async fn git_diff_files(core: &Shared, root: String, from: String, to: String) -> AppResult<Vec<CommitFile>> {
    read(core, &root, |repo| async move { repo.diff_files(&from, &to).await }).await
}

/// Leluhur bersama dua revisi (mis. untuk menampilkan perubahan sebuah pull request).
pub async fn git_merge_base(core: &Shared, root: String, a: String, b: String) -> AppResult<Option<String>> {
    read(core, &root, |repo| async move { repo.merge_base(&a, &b).await }).await
}

pub async fn git_reflog(core: &Shared, root: String, limit: u32) -> AppResult<Vec<ReflogEntry>> {
    read(core, &root, |repo| async move { repo.reflog(limit.clamp(1, 2000)).await }).await
}

pub async fn git_blame(core: &Shared, root: String, path: String, contents: Option<String>) -> AppResult<Vec<BlameRange>> {
    read(core, &root, |repo| async move { repo.blame(&path, contents.map(String::into_bytes)).await }).await
}

// ----- Stash -----

pub async fn git_stash_list(core: &Shared, root: String) -> AppResult<Vec<Stash>> {
    read(core, &root, |repo| async move { repo.stash_list().await }).await
}

pub async fn git_stash_push(core: &Shared, root: String, options: StashOptions) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.stash_push(&options).await }).await
}

pub async fn git_stash_apply(core: &Shared, root: String, name: String, pop: bool) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.stash_apply(&name, pop).await }).await
}

pub async fn git_stash_drop(core: &Shared, root: String, name: String) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.stash_drop(&name).await }).await
}

pub async fn git_stash_branch(core: &Shared, root: String, name: String, branch: String) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.stash_branch(&name, &branch).await }).await
}

pub async fn git_stash_files(core: &Shared, root: String, name: String) -> AppResult<Vec<CommitFile>> {
    read(core, &root, |repo| async move { repo.stash_files(&name).await }).await
}

// ----- Tag -----

pub async fn git_tag_create(core: &Shared, root: String, name: String, rev: Option<String>, message: Option<String>) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.tag_create(&name, rev.as_deref(), message.as_deref()).await }).await
}

pub async fn git_tag_delete(core: &Shared, root: String, name: String) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.tag_delete(&name).await }).await
}

pub async fn git_tag_push(core: &Shared, root: String, remote: String, name: String) -> AppResult<()> {
    write(core, &root, false, |repo| async move { repo.tag_push(&remote, &name).await }).await
}

pub async fn git_tag_delete_remote(core: &Shared, root: String, remote: String, name: String) -> AppResult<()> {
    write(core, &root, false, |repo| async move { repo.tag_delete_remote(&remote, &name).await }).await
}

// ----- Cherry-pick, revert, reset -----

pub async fn git_cherry_pick(core: &Shared, root: String, oids: Vec<String>) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.cherry_pick(&oids).await }).await
}

pub async fn git_revert(core: &Shared, root: String, oid: String) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.revert(&oid).await }).await
}

pub async fn git_reset(core: &Shared, root: String, rev: String, mode: ResetMode) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.reset(&rev, mode).await }).await
}

// ----- Konflik -----

pub async fn git_conflict(core: &Shared, root: String, path: String) -> AppResult<ConflictDetail> {
    read(core, &root, |repo| async move { repo.conflict_detail(&path).await }).await
}

pub async fn git_resolve_side(core: &Shared, root: String, path: String, side: ConflictSide) -> AppResult<()> {
    write(core, &root, false, |repo| async move { repo.resolve_with_side(&path, side).await }).await
}

/// Menulis hasil dari editor konflik ke berkas lalu menandainya selesai.
pub async fn git_resolve_write(core: &Shared, root: String, path: String, content: String) -> AppResult<()> {
    let repo = core.repo(&root)?;
    let full = repo.root().join(&path);
    if !oncode_workspace::is_inside(repo.root(), &full) {
        return Err(AppError::new("invalidPath", format!("Path is outside the repository: {path}")));
    }
    tokio::fs::write(&full, content).await?;
    write(core, &root, false, |repo| async move { repo.mark_resolved(&path).await }).await
}

pub async fn git_mark_resolved(core: &Shared, root: String, path: String) -> AppResult<()> {
    write(core, &root, false, |repo| async move { repo.mark_resolved(&path).await }).await
}

pub async fn git_resolve_delete(core: &Shared, root: String, path: String) -> AppResult<()> {
    write(core, &root, false, |repo| async move { repo.resolve_by_deleting(&path).await }).await
}

// ----- Worktree dan submodule -----

pub async fn git_worktrees(core: &Shared, root: String) -> AppResult<Vec<Worktree>> {
    read(core, &root, |repo| async move { repo.worktrees().await }).await
}

/// Menambah worktree; folder baru ikut dipercaya karena berasal dari repositori yang sudah dipercaya.
pub async fn git_worktree_add(core: &Shared, root: String, path: String, new_branch: Option<String>, rev: Option<String>) -> AppResult<()> {
    let target = PathBuf::from(&path);
    let result = write(core, &root, true, |repo| async move { repo.worktree_add(&target, new_branch.as_deref(), rev.as_deref()).await }).await;
    if result.is_ok() {
        core.store.set_trusted(&path, true);
    }
    result
}

pub async fn git_worktree_remove(core: &Shared, root: String, path: String, force: bool) -> AppResult<()> {
    let target = PathBuf::from(path);
    write(core, &root, true, |repo| async move { repo.worktree_remove(&target, force).await }).await
}

pub async fn git_worktree_prune(core: &Shared, root: String) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.worktree_prune().await }).await
}

pub async fn git_submodules(core: &Shared, root: String) -> AppResult<Vec<Submodule>> {
    read(core, &root, |repo| async move { repo.submodules().await }).await
}

pub async fn git_submodule_update(
    core: &Shared,
    root: String,
    paths: Vec<String>,
    init: bool,
    operation_id: String,
    on_progress: ProgressFn,
) -> AppResult<()> {
    let cancel = core.begin_operation(&operation_id);
    let result =
        write(core, &root, false, |repo| async move { repo.submodule_update(&paths, init, Some(cancel), Some(on_progress)).await }).await;
    core.end_operation(&operation_id);
    result
}

pub async fn git_submodule_sync(core: &Shared, root: String) -> AppResult<()> {
    write(core, &root, false, |repo| async move { repo.submodule_sync().await }).await
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use super::*;
    use crate::events::CoreEvent;
    use crate::testing::{git, recv, TestCore};
    use tokio::sync::mpsc::UnboundedReceiver;

    fn text(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    /// Membuka repositori tanpa workspace (jadi tanpa pemantau berkas): setiap `RepoChanged`
    /// yang datang pasti berasal dari pemanggilan API. Siaran pertama akibat `git_open` dibuang.
    async fn open(t: &mut TestCore, project: &Path) -> String {
        let root = git_open(&t.core, "w1", text(project)).await.unwrap();
        next_snapshot(&mut t.events, &root).await;
        root
    }

    async fn next_snapshot(events: &mut UnboundedReceiver<CoreEvent>, root: &str) -> RepoSnapshot {
        recv(events, |e| match e {
            CoreEvent::RepoChanged { root: changed, snapshot } if changed == root => Some(snapshot),
            _ => None,
        })
        .await
    }

    #[tokio::test]
    async fn write_through_git_api_broadcasts_repo_changed() {
        let mut t = TestCore::new();
        let project = t.trusted_repo("proyek");
        let root = open(&mut t, &project).await;
        // Root berasal dari Git (`C:/...` di Windows); dibandingkan sebagai path.
        assert_eq!(Path::new(&root), project);
        std::fs::write(project.join("baru.txt"), "isi\n").unwrap();

        git_stage(&t.core, root.clone(), vec!["baru.txt".into()]).await.unwrap();
        let snapshot = next_snapshot(&mut t.events, &root).await;
        assert_eq!(snapshot.staged.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["baru.txt"]);
        assert!(snapshot.untracked.is_empty() && snapshot.unstaged.is_empty());
        assert_eq!(snapshot, git_snapshot(&t.core, root.clone()).await.unwrap());

        // Commit mengubah ref: `RefsChanged` disiarkan langsung, snapshot bersih menyusul.
        let oid = git_commit(&t.core, root.clone(), "Tambah berkas".into(), CommitOptions::default()).await.unwrap();
        assert_eq!(oid, git(&project, &["rev-parse", "HEAD"]).trim());
        let mut refs_changed = false;
        let snapshot = recv(&mut t.events, |e| match e {
            CoreEvent::RefsChanged { root: changed } if changed == root => {
                refs_changed = true;
                None
            }
            CoreEvent::RepoChanged { root: changed, snapshot } if changed == root => Some(snapshot),
            _ => None,
        })
        .await;
        assert!(refs_changed, "RefsChanged harus mendahului snapshot baru");
        assert!(snapshot.staged.is_empty());
        assert_eq!(snapshot.head_oid.as_deref(), Some(oid.as_str()));
        assert_eq!(git_last_message(&t.core, root).await.unwrap().trim(), "Tambah berkas");
    }

    #[tokio::test]
    async fn failed_write_still_broadcasts_a_snapshot() {
        let mut t = TestCore::new();
        let project = t.trusted_repo("proyek");
        let root = open(&mut t, &project).await;

        let err = git_switch(&t.core, root.clone(), "tidak-ada".into()).await.unwrap_err();
        assert_ne!(err.code, "untrusted");
        let snapshot = next_snapshot(&mut t.events, &root).await;
        assert_eq!(snapshot.head, Head::Branch { name: "main".into() });
    }

    #[tokio::test]
    async fn git_output_is_forwarded_to_the_event_sink() {
        let mut t = TestCore::new();
        let project = t.trusted_repo("proyek");
        let root = git_open(&t.core, "w1", text(&project)).await.unwrap();
        git_branch_create(&t.core, root, "fitur".into(), None, false).await.unwrap();
        let entry = recv(&mut t.events, |e| match e {
            CoreEvent::GitOutput { entry } if entry.command.contains("fitur") => Some(entry),
            _ => None,
        })
        .await;
        assert_eq!(entry.exit_code, 0);
        assert_eq!(Path::new(&entry.cwd), project);
    }

    #[tokio::test]
    async fn untrusted_folder_is_refused_by_the_git_api() {
        let t = TestCore::new();
        let project = t.project("proyek");
        git(&project, &["init", "-q", "-b", "main"]);
        let path = text(&project);

        assert_eq!(git_open(&t.core, "w1", path.clone()).await.unwrap_err().code, "untrusted");
        // Folder yang sudah berisi repositori tidak menjadi dipercaya lewat `git_init`.
        assert_eq!(git_init(&t.core, path.clone()).await.unwrap_err().code, "untrusted");
        assert!(!t.core.store.is_trusted(&project));
        // Tanpa `git_open` yang berhasil, tidak ada operasi git yang bisa dijalankan pada folder itu.
        assert_eq!(git_stage_all(&t.core, path.clone()).await.unwrap_err().code, "notARepo");
        assert_eq!(git_snapshot(&t.core, path.clone()).await.unwrap_err().code, "notARepo");
        assert_eq!(git_fetch(&t.core, path, None, "op".into(), Arc::new(|_| {})).await.unwrap_err().code, "notARepo");

        // Folder baru yang di-init dari aplikasi langsung dipercaya.
        let fresh = t.project("baru");
        git_init(&t.core, text(&fresh)).await.unwrap();
        assert!(t.core.store.is_trusted(&fresh));
        assert_eq!(Path::new(&git_open(&t.core, "w1", text(&fresh)).await.unwrap()), fresh);
    }

    #[tokio::test]
    async fn clone_validates_the_destination_and_trusts_the_result() {
        let t = TestCore::new();
        let source = t.trusted_repo("sumber");
        let parent = t.project("tujuan");
        let clone = |name: &str| git_clone(&t.core, text(&source), text(&parent), name.to_string(), false, "op-clone".into(), Arc::new(|_| {}));

        for name in ["", ".", "..", "a/b", "a\\b"] {
            assert_eq!(clone(name).await.unwrap_err().code, "invalidInput", "nama {name:?}");
        }
        std::fs::create_dir(parent.join("penuh")).unwrap();
        std::fs::write(parent.join("penuh/berkas"), "").unwrap();
        assert_eq!(clone("penuh").await.unwrap_err().code, "alreadyExists");

        let dest = clone("klon").await.unwrap();
        assert_eq!(Path::new(&dest), parent.join("klon"));
        assert_eq!(std::fs::read_to_string(parent.join("klon/README.md")).unwrap(), "# Proyek\n");
        assert!(t.core.store.is_trusted(&parent.join("klon")));
        assert!(!t.core.store.is_trusted(&parent));
    }

    #[tokio::test]
    async fn revisions_and_paths_are_validated() {
        let mut t = TestCore::new();
        let project = t.trusted_repo("proyek");
        let root = open(&mut t, &project).await;

        let file_at = |rev: &str, path: &str| git_file_at(&t.core, root.clone(), rev.to_string(), path.to_string());
        assert_eq!(file_at("HEAD", "README.md").await.unwrap().as_deref(), Some("# Proyek\n"));
        assert_eq!(file_at("HEAD", "tidak-ada.txt").await.unwrap(), None);
        assert_eq!(file_at("--output=x", "README.md").await.unwrap_err().code, "invalidInput");
        assert_eq!(file_at("HEAD:README.md", "x").await.unwrap_err().code, "invalidInput");

        let outside = t.dir.path().join("luar.txt");
        let err = git_resolve_write(&t.core, root.clone(), "../luar.txt".into(), "x".into()).await.unwrap_err();
        assert_eq!(err.code, "invalidPath");
        assert!(!outside.exists(), "berkas di luar repositori tidak boleh ditulis");
        let err = git_discard(&t.core, root.clone(), Vec::new(), vec!["../luar.txt".into()]).await.unwrap_err();
        assert_eq!(err.code, "invalidPath");
    }

    #[tokio::test]
    async fn log_streams_are_paged_by_id() {
        let mut t = TestCore::new();
        let project = t.trusted_repo("proyek");
        let root = open(&mut t, &project).await;

        let id = git_log_open(&t.core, root.clone(), LogQuery::default()).await.unwrap();
        let page = git_log_next(&t.core, id, 50).await.unwrap();
        assert_eq!(page.rows.len(), 1);
        git_log_close(&t.core, id).await.unwrap();
        // Aliran yang sudah ditutup (atau tidak pernah ada) menghasilkan halaman kosong yang selesai.
        assert_eq!(git_log_next(&t.core, id, 50).await.unwrap(), LogPage { rows: Vec::new(), done: true });
        assert!(t.core.log_streams.lock().await.is_empty());

        // `limit` 0 dinaikkan menjadi 1.
        let commits = git_log(&t.core, root, LogQuery::default(), 0).await.unwrap();
        assert_eq!(commits.len(), 1);
    }
}
