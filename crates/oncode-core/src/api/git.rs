//! Git API. Every function takes `root` (the root of an already opened worktree).
//! Functions that modify the repository always schedule a broadcast of a new snapshot,
//! even on failure, because a failed operation can leave a new state (e.g. a conflict).

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use oncode_git::*;

use crate::error::{AppError, AppResult};
use crate::state::Shared;

/// Runs a read operation.
async fn read<T, Fut>(core: &Shared, root: &str, f: impl FnOnce(Repo) -> Fut) -> AppResult<T>
where
    Fut: Future<Output = oncode_git::Result<T>> + Send,
{
    f(core.repo(root)?).await.map_err(Into::into)
}

/// Runs a write operation and then schedules a new snapshot. `refs` also broadcasts ref changes.
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

// ----- Repository -----

/// Opens the repository containing `path` (e.g. a submodule) and returns its root.
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
    // A folder that already contains a repository must not become trusted through this path.
    let target = PathBuf::from(&path);
    if target.join(".git").exists() && !core.store.is_trusted(&target) {
        return Err(AppError::untrusted());
    }
    let git = core.git().await?;
    git.init(&PathBuf::from(&path)).await?;
    core.store.set_trusted(&path, true);
    Ok(())
}

/// Clones a repository and returns the destination path. The result is not trusted: it opens in
/// restricted mode like any other new folder, because its Git config, language servers and
/// project settings come from whoever published it. Submodules are not cloned either, since
/// their URLs come from the same untrusted `.gitmodules`; after trusting the folder they can
/// be fetched with Update Submodules.
pub async fn git_clone(core: &Shared, url: String, parent_dir: String, name: String, operation_id: String, on_progress: ProgressFn) -> AppResult<String> {
    if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
        return Err(AppError::invalid("Invalid destination folder name"));
    }
    let dest = PathBuf::from(&parent_dir).join(&name);
    if dest.exists() && dest.read_dir().is_ok_and(|mut d| d.next().is_some()) {
        return Err(AppError::new("alreadyExists", format!("Destination folder already exists and is not empty: {}", dest.display())));
    }
    let git = core.git().await?;
    let cancel = core.begin_operation(&operation_id);
    let result = git.clone_repo(&url, &dest, false, Some(cancel), Some(on_progress)).await;
    core.end_operation(&operation_id);
    result?;
    Ok(dest.to_string_lossy().into_owned())
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

/// Discards changes: tracked files are restored to the index contents, untracked files
/// are moved to the OS trash (not permanently deleted).
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

/// Stages, unstages, or discards part of a file's changes (per hunk or per line).
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

/// The contents of a file at a given revision (e.g. `HEAD`), for change markers in the editor gutter.
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

/// The number of commits that exist only on this branch; shown before a force delete.
pub async fn git_branch_unmerged(core: &Shared, root: String, name: String) -> AppResult<u32> {
    read(core, &root, |repo| async move { repo.unmerged_count(&name).await }).await
}

pub async fn git_branch_set_upstream(core: &Shared, root: String, branch: String, upstream: Option<String>) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.set_upstream(&branch, upstream.as_deref()).await }).await
}

pub async fn git_branch_delete_remote(core: &Shared, root: String, remote: String, branch: String) -> AppResult<()> {
    write(core, &root, true, |repo| async move { repo.delete_remote_branch(&remote, &branch).await }).await
}

// ----- Merge, rebase, ongoing operations -----

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

/// Continue / Skip / Abort for a merge, rebase, cherry-pick, or revert that is currently stopped.
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

// ----- History -----

/// Starts an incremental `git log` for the Graph view; pages are fetched with `git_log_next`.
pub async fn git_log_open(core: &Shared, root: String, query: LogQuery) -> AppResult<u32> {
    let stream = core.repo(&root)?.log_stream(&query)?;
    let id = core.next_id();
    core.log_streams.lock().unwrap().insert(id, Arc::new(tokio::sync::Mutex::new(stream)));
    Ok(id)
}

pub async fn git_log_next(core: &Shared, stream_id: u32, count: u32) -> AppResult<LogPage> {
    let Some(stream) = core.log_streams.lock().unwrap().get(&stream_id).cloned() else {
        return Ok(LogPage { rows: Vec::new(), done: true });
    };
    let mut stream = stream.lock().await;
    Ok(stream.next_page(count.clamp(1, 2000) as usize).await?)
}

pub async fn git_log_close(core: &Shared, stream_id: u32) -> AppResult<()> {
    let stream = core.log_streams.lock().unwrap().remove(&stream_id);
    // A page still being read holds the stream; it is stopped when that read ends (`kill_on_drop`).
    if let Some(stream) = stream.and_then(|stream| Arc::try_unwrap(stream).ok()) {
        stream.into_inner().close().await;
    }
    Ok(())
}

/// A list of commits without a graph (file history, search).
pub async fn git_log(core: &Shared, root: String, query: LogQuery, limit: u32) -> AppResult<Vec<Commit>> {
    read(core, &root, |repo| async move { repo.log(&query, limit.clamp(1, 5000)).await }).await
}

pub async fn git_commit_detail(core: &Shared, root: String, oid: String) -> AppResult<CommitDetail> {
    read(core, &root, |repo| async move { repo.commit_detail(&oid).await }).await
}

pub async fn git_diff_files(core: &Shared, root: String, from: String, to: String) -> AppResult<Vec<CommitFile>> {
    read(core, &root, |repo| async move { repo.diff_files(&from, &to).await }).await
}

/// The common ancestor of two revisions (e.g. to show a pull request's changes).
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

// ----- Conflicts -----

pub async fn git_conflict(core: &Shared, root: String, path: String) -> AppResult<ConflictDetail> {
    read(core, &root, |repo| async move { repo.conflict_detail(&path).await }).await
}

pub async fn git_resolve_side(core: &Shared, root: String, path: String, side: ConflictSide) -> AppResult<()> {
    write(core, &root, false, |repo| async move { repo.resolve_with_side(&path, side).await }).await
}

/// Error code of [`git_resolve_write`] when the file changed after the conflict editor read it.
pub const CHANGED_ON_DISK: &str = "changedOnDisk";

/// Writes the result from the conflict editor to the file, encoded as `format` (the file's own encoding,
/// byte order mark, and line ending), and then marks it resolved. With `expected` (the working tree file
/// as the editor read it, see [`ConflictDetail::worktree`]), a file that has changed since is not written:
/// the error code is [`CHANGED_ON_DISK`].
pub async fn git_resolve_write(core: &Shared, root: String, path: String, content: String, format: TextFormat, expected: Option<Option<String>>) -> AppResult<()> {
    let repo = core.repo(&root)?;
    let full = repo.root().join(&path);
    if !oncode_workspace::is_inside(repo.root(), &full) {
        return Err(AppError::new("invalidPath", format!("Path is outside the repository: {path}")));
    }
    if let Some(expected) = expected {
        if repo.worktree_text(&path, &format.encoding).await? != expected {
            return Err(AppError::new(CHANGED_ON_DISK, format!("{path} changed on disk after the conflict editor opened it.")));
        }
    }
    let eol = if format.crlf { oncode_workspace::Eol::Crlf } else { oncode_workspace::Eol::Lf };
    oncode_workspace::save_file(&full, &content, &format.encoding, format.bom, eol).await?;
    write(core, &root, false, |repo| async move { repo.mark_resolved(&path).await }).await
}

pub async fn git_mark_resolved(core: &Shared, root: String, path: String) -> AppResult<()> {
    write(core, &root, false, |repo| async move { repo.mark_resolved(&path).await }).await
}

pub async fn git_resolve_delete(core: &Shared, root: String, path: String) -> AppResult<()> {
    write(core, &root, false, |repo| async move { repo.resolve_by_deleting(&path).await }).await
}

// ----- Worktree and submodule -----

pub async fn git_worktrees(core: &Shared, root: String) -> AppResult<Vec<Worktree>> {
    read(core, &root, |repo| async move { repo.worktrees().await }).await
}

/// Adds a worktree; the new folder is also trusted because it comes from an already trusted repository.
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

    /// Opens a repository without a workspace (so without a file watcher): every `RepoChanged`
    /// that arrives is certain to come from an API call. The first broadcast caused by `git_open` is discarded.
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
        let project = t.trusted_repo("project");
        let root = open(&mut t, &project).await;
        // The root comes from Git (`C:/...` on Windows); compared as paths.
        assert_eq!(Path::new(&root), project);
        std::fs::write(project.join("new.txt"), "content\n").unwrap();

        git_stage(&t.core, root.clone(), vec!["new.txt".into()]).await.unwrap();
        let snapshot = next_snapshot(&mut t.events, &root).await;
        assert_eq!(snapshot.staged.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["new.txt"]);
        assert!(snapshot.untracked.is_empty() && snapshot.unstaged.is_empty());
        assert_eq!(snapshot, git_snapshot(&t.core, root.clone()).await.unwrap());

        // A commit changes refs: `RefsChanged` is broadcast immediately, the clean snapshot follows.
        let oid = git_commit(&t.core, root.clone(), "Add file".into(), CommitOptions::default()).await.unwrap();
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
        assert!(refs_changed, "RefsChanged must precede the new snapshot");
        assert!(snapshot.staged.is_empty());
        assert_eq!(snapshot.head_oid.as_deref(), Some(oid.as_str()));
        assert_eq!(git_last_message(&t.core, root).await.unwrap().trim(), "Add file");
    }

    #[tokio::test]
    async fn failed_write_still_broadcasts_a_snapshot() {
        let mut t = TestCore::new();
        let project = t.trusted_repo("project");
        let root = open(&mut t, &project).await;

        let err = git_switch(&t.core, root.clone(), "does-not-exist".into()).await.unwrap_err();
        assert_ne!(err.code, "untrusted");
        let snapshot = next_snapshot(&mut t.events, &root).await;
        assert_eq!(snapshot.head, Head::Branch { name: "main".into() });
    }

    #[tokio::test]
    async fn git_output_is_forwarded_to_the_event_sink() {
        let mut t = TestCore::new();
        let project = t.trusted_repo("project");
        let root = git_open(&t.core, "w1", text(&project)).await.unwrap();
        git_branch_create(&t.core, root, "feature".into(), None, false).await.unwrap();
        let entry = recv(&mut t.events, |e| match e {
            CoreEvent::GitOutput { entry } if entry.command.contains("feature") => Some(entry),
            _ => None,
        })
        .await;
        assert_eq!(entry.exit_code, 0);
        assert_eq!(Path::new(&entry.cwd), project);
    }

    #[tokio::test]
    async fn untrusted_folder_is_refused_by_the_git_api() {
        let t = TestCore::new();
        let project = t.project("project");
        git(&project, &["init", "-q", "-b", "main"]);
        let path = text(&project);

        assert_eq!(git_open(&t.core, "w1", path.clone()).await.unwrap_err().code, "untrusted");
        // A folder that already contains a repository does not become trusted through `git_init`.
        assert_eq!(git_init(&t.core, path.clone()).await.unwrap_err().code, "untrusted");
        assert!(!t.core.store.is_trusted(&project));
        // Without a successful `git_open`, no git operation can run on that folder.
        assert_eq!(git_stage_all(&t.core, path.clone()).await.unwrap_err().code, "notARepo");
        assert_eq!(git_snapshot(&t.core, path.clone()).await.unwrap_err().code, "notARepo");
        assert_eq!(git_fetch(&t.core, path, None, "op".into(), Arc::new(|_| {})).await.unwrap_err().code, "notARepo");

        // A new folder initialized from the app is trusted right away.
        let fresh = t.project("new");
        git_init(&t.core, text(&fresh)).await.unwrap();
        assert!(t.core.store.is_trusted(&fresh));
        assert_eq!(Path::new(&git_open(&t.core, "w1", text(&fresh)).await.unwrap()), fresh);
    }

    #[tokio::test]
    async fn clone_validates_the_destination_and_leaves_the_result_untrusted() {
        let t = TestCore::new();
        let source = t.trusted_repo("source");
        let parent = t.project("target");
        let clone = |name: &str| git_clone(&t.core, text(&source), text(&parent), name.to_string(), "op-clone".into(), Arc::new(|_| {}));

        for name in ["", ".", "..", "a/b", "a\\b"] {
            assert_eq!(clone(name).await.unwrap_err().code, "invalidInput", "name {name:?}");
        }
        std::fs::create_dir(parent.join("full")).unwrap();
        std::fs::write(parent.join("full/file"), "").unwrap();
        assert_eq!(clone("full").await.unwrap_err().code, "alreadyExists");

        let dest = clone("clone").await.unwrap();
        assert_eq!(Path::new(&dest), parent.join("clone"));
        assert_eq!(std::fs::read_to_string(parent.join("clone/README.md")).unwrap(), "# Project\n");
        // A cloned repository opens in restricted mode like any other new folder.
        assert!(!t.core.store.is_trusted(&parent.join("clone")));
        assert!(!t.core.store.is_trusted(&parent));
        t.core.set_workspace("w1", parent.join("clone"));
        assert_eq!(git_open(&t.core, "w1", dest).await.unwrap_err().code, "untrusted");
    }

    /// Submodule URLs come from the untrusted `.gitmodules`, so a clone does not fetch them.
    #[tokio::test]
    async fn clone_does_not_fetch_submodules() {
        let t = TestCore::new();
        let library = t.trusted_repo("library");
        let source = t.trusted_repo("source");
        git(&source, &["submodule", "add", "-q", &text(&library), "lib"]);
        git(&source, &["commit", "-q", "-m", "Add submodule"]);
        let parent = t.project("target");

        git_clone(&t.core, text(&source), text(&parent), "clone".into(), "op-clone".into(), Arc::new(|_| {})).await.unwrap();
        assert!(parent.join("clone/.gitmodules").exists());
        assert!(parent.join("clone/lib").is_dir(), "git still creates the empty submodule folder");
        assert!(!parent.join("clone/lib/README.md").exists(), "the submodule is not fetched");
    }

    /// The conflict editor's result is written in the file's own encoding and line endings, and not over a
    /// file that changed after the editor read it.
    #[tokio::test]
    async fn resolving_a_conflict_keeps_the_encoding_and_refuses_a_changed_file() {
        let mut t = TestCore::new();
        let project = t.trusted_repo("project");
        let root = open(&mut t, &project).await;
        let latin1 = |text: &str| text.replace('\n', "\r\n").chars().map(|c| if c == '\u{e9}' { 0xE9u8 } else { c as u8 }).collect::<Vec<u8>>();
        let commit = |text: &str, message: &str| {
            std::fs::write(project.join("menu.txt"), latin1(text)).unwrap();
            git(&project, &["add", "-A"]);
            git(&project, &["commit", "-q", "-m", message]);
        };
        commit("caf\u{e9}\nprix = 1\n", "Base");
        git(&project, &["switch", "-q", "-c", "feature"]);
        commit("caf\u{e9}\nprix = 3\n", "Feature");
        git(&project, &["switch", "-q", "main"]);
        commit("caf\u{e9}\nprix = 2\n", "Main");
        let merged = std::process::Command::new("git").current_dir(&project).args(["merge", "-q", "feature"]).output().unwrap();
        assert!(!merged.status.success(), "the merge stops at the conflict");

        let detail = git_conflict(&t.core, root.clone(), "menu.txt".into()).await.unwrap();
        let write = |expected: Option<Option<String>>| git_resolve_write(&t.core, root.clone(), "menu.txt".into(), "caf\u{e9}\nprix = 23\n".into(), detail.format.clone(), expected);

        // Edited elsewhere after the conflict editor read it: nothing is written.
        let edited = latin1("caf\u{e9}\nprix = 4\n");
        std::fs::write(project.join("menu.txt"), &edited).unwrap();
        assert_eq!(write(Some(detail.worktree.clone())).await.unwrap_err().code, CHANGED_ON_DISK);
        assert_eq!(std::fs::read(project.join("menu.txt")).unwrap(), edited);

        // Overwriting writes Windows-1252 with CRLF, like the file was.
        write(None).await.unwrap();
        assert_eq!(std::fs::read(project.join("menu.txt")).unwrap(), latin1("caf\u{e9}\nprix = 23\n"));
        assert!(git(&project, &["diff", "--name-only", "--diff-filter=U"]).is_empty(), "marked resolved");
    }

    #[tokio::test]
    async fn revisions_and_paths_are_validated() {
        let mut t = TestCore::new();
        let project = t.trusted_repo("project");
        let root = open(&mut t, &project).await;

        let file_at = |rev: &str, path: &str| git_file_at(&t.core, root.clone(), rev.to_string(), path.to_string());
        assert_eq!(file_at("HEAD", "README.md").await.unwrap().as_deref(), Some("# Project\n"));
        assert_eq!(file_at("HEAD", "does-not-exist.txt").await.unwrap(), None);
        assert_eq!(file_at("--output=x", "README.md").await.unwrap_err().code, "invalidInput");
        assert_eq!(file_at("HEAD:README.md", "x").await.unwrap_err().code, "invalidInput");

        let outside = t.dir.path().join("outside.txt");
        let err = git_resolve_write(&t.core, root.clone(), "../outside.txt".into(), "x".into(), TextFormat::default(), None).await.unwrap_err();
        assert_eq!(err.code, "invalidPath");
        assert!(!outside.exists(), "a file outside the repository must not be written");
        let err = git_discard(&t.core, root.clone(), Vec::new(), vec!["../outside.txt".into()]).await.unwrap_err();
        assert_eq!(err.code, "invalidPath");
    }

    #[tokio::test]
    async fn log_streams_are_paged_by_id() {
        let mut t = TestCore::new();
        let project = t.trusted_repo("project");
        let root = open(&mut t, &project).await;

        let id = git_log_open(&t.core, root.clone(), LogQuery::default()).await.unwrap();
        let page = git_log_next(&t.core, id, 50).await.unwrap();
        assert_eq!(page.rows.len(), 1);
        git_log_close(&t.core, id).await.unwrap();
        // A stream that has been closed (or never existed) yields an empty page that is done.
        assert_eq!(git_log_next(&t.core, id, 50).await.unwrap(), LogPage { rows: Vec::new(), done: true });
        assert!(t.core.log_streams.lock().unwrap().is_empty());

        // A `limit` of 0 is raised to 1.
        let commits = git_log(&t.core, root, LogQuery::default(), 0).await.unwrap();
        assert_eq!(commits.len(), 1);
    }
}
