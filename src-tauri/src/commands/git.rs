//! Command Git. Setiap command menerima `root` (akar worktree yang sudah dibuka).
//! Command yang mengubah repositori selalu menjadwalkan penyiaran snapshot baru,
//! juga saat gagal, karena operasi yang gagal bisa meninggalkan keadaan baru (mis. konflik).

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use oncode_git::*;
use tauri::ipc::Channel;
use tauri::{State, WebviewWindow};

use crate::error::{AppError, AppResult};
use crate::state::Shared;

fn progress_fn(channel: Channel<Progress>) -> Option<ProgressFn> {
    Some(Arc::new(move |progress| {
        let _ = channel.send(progress);
    }))
}

/// Menjalankan operasi baca.
async fn read<T, Fut>(state: &Shared, root: &str, f: impl FnOnce(Repo) -> Fut) -> AppResult<T>
where
    Fut: Future<Output = oncode_git::Result<T>> + Send,
{
    f(state.repo(root)?).await.map_err(Into::into)
}

/// Menjalankan operasi tulis lalu menjadwalkan snapshot baru. `refs` juga menyiarkan perubahan ref.
async fn write<T, Fut>(state: &Shared, root: &str, refs: bool, f: impl FnOnce(Repo) -> Fut) -> AppResult<T>
where
    Fut: Future<Output = oncode_git::Result<T>> + Send,
{
    let repo = state.repo(root)?;
    let result = f(repo).await;
    state.refresh(root);
    if refs {
        state.refresh_refs(root);
    }
    result.map_err(Into::into)
}

// ----- Repositori -----

/// Membuka repositori yang memuat `path` (mis. submodule) dan mengembalikan akarnya.
#[tauri::command]
#[specta::specta]
pub async fn git_open(window: WebviewWindow, state: State<'_, Shared>, path: String) -> AppResult<String> {
    let repo = state.open_repo(window.label(), &PathBuf::from(path)).await?;
    let root = repo.root_string();
    state.refresh(&root);
    Ok(root)
}

#[tauri::command]
#[specta::specta]
pub async fn git_snapshot(state: State<'_, Shared>, root: String) -> AppResult<RepoSnapshot> {
    read(&state, &root, |repo| async move { repo.snapshot().await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_refs(state: State<'_, Shared>, root: String) -> AppResult<Refs> {
    read(&state, &root, |repo| async move { repo.refs().await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_init(state: State<'_, Shared>, path: String) -> AppResult<()> {
    // Folder yang sudah berisi repositori tidak boleh menjadi dipercaya lewat jalur ini.
    let target = PathBuf::from(&path);
    if target.join(".git").exists() && !state.store.is_trusted(&target) {
        return Err(AppError::untrusted());
    }
    let git = state.git().await?;
    git.init(&PathBuf::from(&path)).await?;
    state.store.set_trusted(&path, true);
    Ok(())
}

/// Meng-clone repositori; folder hasilnya langsung dipercaya. Mengembalikan path tujuan.
#[tauri::command]
#[specta::specta]
pub async fn git_clone(
    state: State<'_, Shared>,
    url: String,
    parent_dir: String,
    name: String,
    recurse_submodules: bool,
    operation_id: String,
    on_progress: Channel<Progress>,
) -> AppResult<String> {
    if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
        return Err(AppError::invalid("Invalid destination folder name"));
    }
    let dest = PathBuf::from(&parent_dir).join(&name);
    if dest.exists() && dest.read_dir().is_ok_and(|mut d| d.next().is_some()) {
        return Err(AppError::new("alreadyExists", format!("Destination folder already exists and is not empty: {}", dest.display())));
    }
    let git = state.git().await?;
    let cancel = state.begin_operation(&operation_id);
    let result = git.clone_repo(&url, &dest, recurse_submodules, Some(cancel), progress_fn(on_progress)).await;
    state.end_operation(&operation_id);
    result?;
    let dest = dest.to_string_lossy().into_owned();
    state.store.set_trusted(&dest, true);
    Ok(dest)
}

// ----- Staging -----

#[tauri::command]
#[specta::specta]
pub async fn git_stage(state: State<'_, Shared>, root: String, paths: Vec<String>) -> AppResult<()> {
    write(&state, &root, false, |repo| async move { repo.stage(&paths).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_stage_all(state: State<'_, Shared>, root: String) -> AppResult<()> {
    write(&state, &root, false, |repo| async move { repo.stage_all().await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_unstage(state: State<'_, Shared>, root: String, paths: Vec<String>) -> AppResult<()> {
    write(&state, &root, false, |repo| async move { repo.unstage(&paths).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_unstage_all(state: State<'_, Shared>, root: String) -> AppResult<()> {
    write(&state, &root, false, |repo| async move { repo.unstage_all().await }).await
}

/// Membuang perubahan: berkas terlacak dikembalikan ke isi index, berkas untracked
/// dipindahkan ke tempat sampah OS (bukan dihapus permanen).
#[tauri::command]
#[specta::specta]
pub async fn git_discard(state: State<'_, Shared>, root: String, tracked: Vec<String>, untracked: Vec<String>) -> AppResult<()> {
    let repo = state.repo(&root)?;
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
    state.refresh(&root);
    result
}

#[tauri::command]
#[specta::specta]
pub async fn git_diff(state: State<'_, Shared>, root: String, path: String, orig_path: Option<String>, source: DiffSource) -> AppResult<DiffContents> {
    read(&state, &root, |repo| async move { repo.diff_contents(&path, orig_path.as_deref(), &source).await }).await
}

/// Stage, unstage, atau discard sebagian perubahan sebuah berkas (per hunk atau per baris).
#[tauri::command]
#[specta::specta]
pub async fn git_splice(
    state: State<'_, Shared>,
    root: String,
    target: SpliceTarget,
    path: String,
    ops: Vec<SpliceOp>,
    fingerprint_a: String,
    fingerprint_b: String,
) -> AppResult<()> {
    write(&state, &root, false, |repo| async move { repo.apply_splice(target, &path, &ops, &fingerprint_a, &fingerprint_b).await }).await
}

/// Isi sebuah berkas pada revisi tertentu (mis. `HEAD`), untuk penanda perubahan di gutter editor.
#[tauri::command]
#[specta::specta]
pub async fn git_file_at(state: State<'_, Shared>, root: String, rev: String, path: String) -> AppResult<Option<String>> {
    let repo = state.repo(&root)?;
    if rev.starts_with('-') || rev.contains(':') || path.contains('\0') {
        return Err(AppError::invalid("Invalid revision or path"));
    }
    let blob = repo.blob(&format!("{rev}:{path}")).await?;
    Ok(blob.filter(|b| !oncode_git::content::is_binary(b)).map(|b| String::from_utf8_lossy(&b).into_owned()))
}

// ----- Commit -----

#[tauri::command]
#[specta::specta]
pub async fn git_commit(state: State<'_, Shared>, root: String, message: String, options: CommitOptions) -> AppResult<String> {
    write(&state, &root, true, |repo| async move { repo.commit(&message, &options).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_undo_commit(state: State<'_, Shared>, root: String) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.undo_last_commit().await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_last_message(state: State<'_, Shared>, root: String) -> AppResult<String> {
    read(&state, &root, |repo| async move { repo.last_commit_message().await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_identity(state: State<'_, Shared>, root: String) -> AppResult<Identity> {
    Ok(state.repo(&root)?.identity().await)
}

#[tauri::command]
#[specta::specta]
pub async fn git_set_identity(state: State<'_, Shared>, root: String, name: String, email: String, global: bool) -> AppResult<()> {
    write(&state, &root, false, |repo| async move { repo.set_identity(&name, &email, global).await }).await
}

// ----- Branch -----

#[tauri::command]
#[specta::specta]
pub async fn git_switch(state: State<'_, Shared>, root: String, branch: String) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.switch(&branch).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_switch_detached(state: State<'_, Shared>, root: String, rev: String) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.switch_detached(&rev).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_switch_tracking(state: State<'_, Shared>, root: String, remote_branch: String) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.switch_tracking(&remote_branch).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_branch_create(state: State<'_, Shared>, root: String, name: String, start: Option<String>, checkout: bool) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.create_branch(&name, start.as_deref(), checkout).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_branch_rename(state: State<'_, Shared>, root: String, name: String, new_name: String) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.rename_branch(&name, &new_name).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_branch_delete(state: State<'_, Shared>, root: String, name: String, force: bool) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.delete_branch(&name, force).await }).await
}

/// Jumlah commit yang hanya ada di branch ini; ditampilkan sebelum menghapus paksa.
#[tauri::command]
#[specta::specta]
pub async fn git_branch_unmerged(state: State<'_, Shared>, root: String, name: String) -> AppResult<u32> {
    read(&state, &root, |repo| async move { repo.unmerged_count(&name).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_branch_set_upstream(state: State<'_, Shared>, root: String, branch: String, upstream: Option<String>) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.set_upstream(&branch, upstream.as_deref()).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_branch_delete_remote(state: State<'_, Shared>, root: String, remote: String, branch: String) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.delete_remote_branch(&remote, &branch).await }).await
}

// ----- Merge, rebase, operasi berlanjut -----

#[tauri::command]
#[specta::specta]
pub async fn git_merge(state: State<'_, Shared>, root: String, rev: String, mode: MergeMode) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.merge(&rev, mode).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_rebase(state: State<'_, Shared>, root: String, onto: String) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.rebase(&onto).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_rebase_plan(state: State<'_, Shared>, root: String, base: Option<String>) -> AppResult<RebasePlan> {
    read(&state, &root, |repo| async move { repo.rebase_plan(base.as_deref()).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_rebase_interactive(state: State<'_, Shared>, root: String, base: Option<String>, items: Vec<RebaseTodoItem>) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.rebase_interactive(base.as_deref(), &items).await }).await
}

/// Continue / Skip / Abort untuk merge, rebase, cherry-pick, atau revert yang sedang berhenti.
#[tauri::command]
#[specta::specta]
pub async fn git_operation(state: State<'_, Shared>, root: String, action: OperationAction) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.operation(action).await }).await
}

// ----- Remote -----

#[tauri::command]
#[specta::specta]
pub async fn git_fetch(
    state: State<'_, Shared>,
    root: String,
    remote: Option<String>,
    operation_id: String,
    on_progress: Channel<Progress>,
) -> AppResult<()> {
    let cancel = state.begin_operation(&operation_id);
    let result = write(&state, &root, true, |repo| async move { repo.fetch(remote.as_deref(), Some(cancel), progress_fn(on_progress)).await }).await;
    state.end_operation(&operation_id);
    result
}

#[tauri::command]
#[specta::specta]
pub async fn git_pull(
    state: State<'_, Shared>,
    root: String,
    strategy: PullStrategy,
    operation_id: String,
    on_progress: Channel<Progress>,
) -> AppResult<()> {
    let cancel = state.begin_operation(&operation_id);
    let result = write(&state, &root, true, |repo| async move { repo.pull(strategy, Some(cancel), progress_fn(on_progress)).await }).await;
    state.end_operation(&operation_id);
    result
}

#[tauri::command]
#[specta::specta]
pub async fn git_push(
    state: State<'_, Shared>,
    root: String,
    options: PushOptions,
    operation_id: String,
    on_progress: Channel<Progress>,
) -> AppResult<()> {
    let cancel = state.begin_operation(&operation_id);
    let result = write(&state, &root, true, |repo| async move { repo.push(&options, Some(cancel), progress_fn(on_progress)).await }).await;
    state.end_operation(&operation_id);
    result
}

#[tauri::command]
#[specta::specta]
pub async fn git_remote_add(state: State<'_, Shared>, root: String, name: String, url: String) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.remote_add(&name, &url).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_remote_remove(state: State<'_, Shared>, root: String, name: String) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.remote_remove(&name).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_remote_rename(state: State<'_, Shared>, root: String, name: String, new_name: String) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.remote_rename(&name, &new_name).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_remote_set_url(state: State<'_, Shared>, root: String, name: String, url: String) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.remote_set_url(&name, &url).await }).await
}

// ----- Riwayat -----

/// Memulai `git log` bertahap untuk tampilan Graph; halaman diambil dengan `git_log_next`.
#[tauri::command]
#[specta::specta]
pub async fn git_log_open(state: State<'_, Shared>, root: String, query: LogQuery) -> AppResult<u32> {
    let stream = state.repo(&root)?.log_stream(&query)?;
    let id = state.next_id();
    state.log_streams.lock().await.insert(id, stream);
    Ok(id)
}

#[tauri::command]
#[specta::specta]
pub async fn git_log_next(state: State<'_, Shared>, stream_id: u32, count: u32) -> AppResult<LogPage> {
    let mut streams = state.log_streams.lock().await;
    let Some(stream) = streams.get_mut(&stream_id) else {
        return Ok(LogPage { rows: Vec::new(), done: true });
    };
    Ok(stream.next_page(count.clamp(1, 2000) as usize).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn git_log_close(state: State<'_, Shared>, stream_id: u32) -> AppResult<()> {
    if let Some(stream) = state.log_streams.lock().await.remove(&stream_id) {
        stream.close().await;
    }
    Ok(())
}

/// Daftar commit tanpa graph (riwayat berkas, pencarian).
#[tauri::command]
#[specta::specta]
pub async fn git_log(state: State<'_, Shared>, root: String, query: LogQuery, limit: u32) -> AppResult<Vec<Commit>> {
    read(&state, &root, |repo| async move { repo.log(&query, limit.clamp(1, 5000)).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_commit_detail(state: State<'_, Shared>, root: String, oid: String) -> AppResult<CommitDetail> {
    read(&state, &root, |repo| async move { repo.commit_detail(&oid).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_diff_files(state: State<'_, Shared>, root: String, from: String, to: String) -> AppResult<Vec<CommitFile>> {
    read(&state, &root, |repo| async move { repo.diff_files(&from, &to).await }).await
}

/// Leluhur bersama dua revisi (mis. untuk menampilkan perubahan sebuah pull request).
#[tauri::command]
#[specta::specta]
pub async fn git_merge_base(state: State<'_, Shared>, root: String, a: String, b: String) -> AppResult<Option<String>> {
    read(&state, &root, |repo| async move { repo.merge_base(&a, &b).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_reflog(state: State<'_, Shared>, root: String, limit: u32) -> AppResult<Vec<ReflogEntry>> {
    read(&state, &root, |repo| async move { repo.reflog(limit.clamp(1, 2000)).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_blame(state: State<'_, Shared>, root: String, path: String, contents: Option<String>) -> AppResult<Vec<BlameRange>> {
    read(&state, &root, |repo| async move { repo.blame(&path, contents.map(String::into_bytes)).await }).await
}

// ----- Stash -----

#[tauri::command]
#[specta::specta]
pub async fn git_stash_list(state: State<'_, Shared>, root: String) -> AppResult<Vec<Stash>> {
    read(&state, &root, |repo| async move { repo.stash_list().await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_stash_push(state: State<'_, Shared>, root: String, options: StashOptions) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.stash_push(&options).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_stash_apply(state: State<'_, Shared>, root: String, name: String, pop: bool) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.stash_apply(&name, pop).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_stash_drop(state: State<'_, Shared>, root: String, name: String) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.stash_drop(&name).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_stash_branch(state: State<'_, Shared>, root: String, name: String, branch: String) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.stash_branch(&name, &branch).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_stash_files(state: State<'_, Shared>, root: String, name: String) -> AppResult<Vec<CommitFile>> {
    read(&state, &root, |repo| async move { repo.stash_files(&name).await }).await
}

// ----- Tag -----

#[tauri::command]
#[specta::specta]
pub async fn git_tag_create(state: State<'_, Shared>, root: String, name: String, rev: Option<String>, message: Option<String>) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.tag_create(&name, rev.as_deref(), message.as_deref()).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_tag_delete(state: State<'_, Shared>, root: String, name: String) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.tag_delete(&name).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_tag_push(state: State<'_, Shared>, root: String, remote: String, name: String) -> AppResult<()> {
    write(&state, &root, false, |repo| async move { repo.tag_push(&remote, &name).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_tag_delete_remote(state: State<'_, Shared>, root: String, remote: String, name: String) -> AppResult<()> {
    write(&state, &root, false, |repo| async move { repo.tag_delete_remote(&remote, &name).await }).await
}

// ----- Cherry-pick, revert, reset -----

#[tauri::command]
#[specta::specta]
pub async fn git_cherry_pick(state: State<'_, Shared>, root: String, oids: Vec<String>) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.cherry_pick(&oids).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_revert(state: State<'_, Shared>, root: String, oid: String) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.revert(&oid).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_reset(state: State<'_, Shared>, root: String, rev: String, mode: ResetMode) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.reset(&rev, mode).await }).await
}

// ----- Konflik -----

#[tauri::command]
#[specta::specta]
pub async fn git_conflict(state: State<'_, Shared>, root: String, path: String) -> AppResult<ConflictDetail> {
    read(&state, &root, |repo| async move { repo.conflict_detail(&path).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_resolve_side(state: State<'_, Shared>, root: String, path: String, side: ConflictSide) -> AppResult<()> {
    write(&state, &root, false, |repo| async move { repo.resolve_with_side(&path, side).await }).await
}

/// Menulis hasil dari editor konflik ke berkas lalu menandainya selesai.
#[tauri::command]
#[specta::specta]
pub async fn git_resolve_write(state: State<'_, Shared>, root: String, path: String, content: String) -> AppResult<()> {
    let repo = state.repo(&root)?;
    let full = repo.root().join(&path);
    if !oncode_workspace::is_inside(repo.root(), &full) {
        return Err(AppError::new("invalidPath", format!("Path is outside the repository: {path}")));
    }
    tokio::fs::write(&full, content).await?;
    write(&state, &root, false, |repo| async move { repo.mark_resolved(&path).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_mark_resolved(state: State<'_, Shared>, root: String, path: String) -> AppResult<()> {
    write(&state, &root, false, |repo| async move { repo.mark_resolved(&path).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_resolve_delete(state: State<'_, Shared>, root: String, path: String) -> AppResult<()> {
    write(&state, &root, false, |repo| async move { repo.resolve_by_deleting(&path).await }).await
}

// ----- Worktree dan submodule -----

#[tauri::command]
#[specta::specta]
pub async fn git_worktrees(state: State<'_, Shared>, root: String) -> AppResult<Vec<Worktree>> {
    read(&state, &root, |repo| async move { repo.worktrees().await }).await
}

/// Menambah worktree; folder baru ikut dipercaya karena berasal dari repositori yang sudah dipercaya.
#[tauri::command]
#[specta::specta]
pub async fn git_worktree_add(state: State<'_, Shared>, root: String, path: String, new_branch: Option<String>, rev: Option<String>) -> AppResult<()> {
    let target = PathBuf::from(&path);
    let result = write(&state, &root, true, |repo| async move { repo.worktree_add(&target, new_branch.as_deref(), rev.as_deref()).await }).await;
    if result.is_ok() {
        state.store.set_trusted(&path, true);
    }
    result
}

#[tauri::command]
#[specta::specta]
pub async fn git_worktree_remove(state: State<'_, Shared>, root: String, path: String, force: bool) -> AppResult<()> {
    let target = PathBuf::from(path);
    write(&state, &root, true, |repo| async move { repo.worktree_remove(&target, force).await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_worktree_prune(state: State<'_, Shared>, root: String) -> AppResult<()> {
    write(&state, &root, true, |repo| async move { repo.worktree_prune().await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_submodules(state: State<'_, Shared>, root: String) -> AppResult<Vec<Submodule>> {
    read(&state, &root, |repo| async move { repo.submodules().await }).await
}

#[tauri::command]
#[specta::specta]
pub async fn git_submodule_update(
    state: State<'_, Shared>,
    root: String,
    paths: Vec<String>,
    init: bool,
    operation_id: String,
    on_progress: Channel<Progress>,
) -> AppResult<()> {
    let cancel = state.begin_operation(&operation_id);
    let result =
        write(&state, &root, false, |repo| async move { repo.submodule_update(&paths, init, Some(cancel), progress_fn(on_progress)).await }).await;
    state.end_operation(&operation_id);
    result
}

#[tauri::command]
#[specta::specta]
pub async fn git_submodule_sync(state: State<'_, Shared>, root: String) -> AppResult<()> {
    write(&state, &root, false, |repo| async move { repo.submodule_sync().await }).await
}
