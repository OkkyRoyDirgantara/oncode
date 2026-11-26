//! Git actions from the Graph feature's menus and buttons: confirmations for destructive actions,
//! indicators for running operations, and actionable errors.

use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::button::Button;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::WindowExt as _;
use gpui_kit::{App, AsyncWindowContext, ClipboardItem, Entity, SharedString, Window};
use oncode_core::api::{app, git};
use oncode_core::{AppError, AppResult, Shared};
use oncode_git::{MergeMode, Progress, ProgressFn, PushOptions, ResetMode};
use oncode_ui::overlay::{self, Choice, Confirm, TextPrompt};
use oncode_ui::quick_pick::{quick_pick, PickItem, QuickPickOptions};
use oncode_ui::spawn_core;

use crate::format::{plural, short_rev};
use crate::menus::GitAction;
use crate::paths::validate_ref_name;
use crate::store::{head_label, snapshot, Busy, GraphStore, RevealTarget};

/// The target repository of an action, together with its window's store.
#[derive(Clone)]
pub struct Ctx {
    pub store: Entity<GraphStore>,
    pub root: String,
}

impl Ctx {
    pub fn new(store: &Entity<GraphStore>, root: impl Into<String>) -> Self {
        Self { store: store.clone(), root: root.into() }
    }

    pub fn core(&self, cx: &App) -> Shared {
        self.store.read(cx).env().core.clone()
    }
}

type OpFuture = Pin<Box<dyn Future<Output = AppResult<()>> + Send>>;
/// A backend operation that can be repeated (e.g. after an automatic stash).
pub type Op = Rc<dyn Fn(Shared, String) -> OpFuture>;

pub fn op<F, Fut>(f: F) -> Op
where
    F: Fn(Shared, String) -> Fut + 'static,
    Fut: Future<Output = AppResult<()>> + Send + 'static,
{
    Rc::new(move |core, root| Box::pin(f(core, root)))
}

static NEXT_OPERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn new_operation_id() -> String {
    format!("op-graph-{}", NEXT_OPERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

fn set_busy(ctx: &Ctx, busy: Option<Busy>, cx: &mut AsyncWindowContext) {
    let _ = cx.update(|_, cx| ctx.store.update(cx, |store, cx| store.set_busy(&ctx.root, busy, cx)));
}

fn window_gone() -> AppError {
    AppError::new("cancelled", "The window was closed")
}

/// Runs an operation with a busy indicator and returns its raw result.
pub async fn run_raw(ctx: &Ctx, label: &str, operation: &Op, cx: &mut AsyncWindowContext) -> AppResult<()> {
    set_busy(ctx, Some(Busy { label: label.to_string().into(), operation_id: None }), cx);
    let task = cx.update(|_, cx| spawn_core(cx, operation(ctx.core(cx), ctx.root.clone())));
    let result = match task {
        Ok(task) => task.await,
        Err(_) => Err(window_gone()),
    };
    set_busy(ctx, None, cx);
    result
}

/// Runs an operation; an error is shown (with a follow-up offer) and the result is `false`.
/// With `retry`, a "local changes would be overwritten" error offers to stash and then repeat.
pub async fn run(ctx: &Ctx, label: &str, operation: Op, retry: bool, cx: &mut AsyncWindowContext) -> bool {
    match run_raw(ctx, label, &operation, cx).await {
        Ok(()) => true,
        Err(error) => {
            let (ctx, label, retry) = (ctx.clone(), label.to_string(), retry.then_some(operation));
            let _ = cx.update(|window, cx| handle_git_error(&ctx, &error, &label, retry, window, cx));
            false
        }
    }
}

/// Runs a network operation with progress and a cancellation id.
pub async fn run_with_progress<F, Fut>(ctx: &Ctx, label: &str, operation: F, cx: &mut AsyncWindowContext) -> bool
where
    F: FnOnce(Shared, String, String, ProgressFn) -> Fut,
    Fut: Future<Output = AppResult<()>> + Send + 'static,
{
    let operation_id = new_operation_id();
    set_busy(ctx, Some(Busy { label: label.to_string().into(), operation_id: Some(operation_id.clone()) }), cx);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Progress>();
    let on_progress: ProgressFn = Arc::new(move |progress| {
        let _ = tx.send(progress);
    });
    // Progress arrives from the backend thread; it is forwarded to the busy indicator on the main thread.
    let (progress_ctx, progress_label, progress_id) = (ctx.clone(), label.to_string(), operation_id.clone());
    let forward = cx.spawn(async move |cx| {
        while let Some(progress) = rx.recv().await {
            let text = match progress.percent {
                Some(percent) => format!("{progress_label}: {} {percent}%", progress.phase),
                None => format!("{progress_label}: {}", progress.phase),
            };
            let updated = cx.update(|_, cx| {
                progress_ctx.store.update(cx, |store, cx| {
                    // The operation has finished (or been replaced): do not revive its indicator.
                    if store.busy(&progress_ctx.root).is_some_and(|b| b.operation_id.as_deref() == Some(progress_id.as_str())) {
                        store.set_busy(&progress_ctx.root, Some(Busy { label: text.into(), operation_id: Some(progress_id.clone()) }), cx);
                    }
                })
            });
            if updated.is_err() {
                break;
            }
        }
    });
    let task = cx.update(|_, cx| spawn_core(cx, operation(ctx.core(cx), ctx.root.clone(), operation_id, on_progress)));
    let result = match task {
        Ok(task) => task.await,
        Err(_) => Err(window_gone()),
    };
    drop(forward);
    set_busy(ctx, None, cx);
    match result {
        Ok(()) => true,
        Err(error) => {
            let (ctx, label) = (ctx.clone(), label.to_string());
            let _ = cx.update(|window, cx| handle_git_error(&ctx, &error, &label, None, window, cx));
            false
        }
    }
}

/// Cancels the network operation currently running on this repository.
pub fn cancel_busy(ctx: &Ctx, cx: &mut App) {
    let Some(id) = ctx.store.read(cx).busy(&ctx.root).and_then(|b| b.operation_id.clone()) else { return };
    let core = ctx.core(cx);
    spawn_core(cx, async move { app::app_cancel(&core, id).await }).detach();
}

fn with_detail(message: String, error: &AppError) -> String {
    match error.detail.as_deref().map(str::trim) {
        Some(detail) if !detail.is_empty() && detail != error.message.trim() => format!("{message}\n{detail}"),
        _ => message,
    }
}

fn show_sidebar(ctx: &Ctx, id: &str, cx: &mut App) {
    if let Some(workbench) = ctx.store.read(cx).env().workbench.upgrade() {
        workbench.update(cx, |workbench, cx| workbench.show_sidebar(id, cx));
    }
}

/// Translates a Git error code into a message and, where applicable, a follow-up action button.
pub fn handle_git_error(ctx: &Ctx, error: &AppError, label: &str, retry: Option<Op>, window: &mut Window, cx: &mut App) {
    match error.code.as_str() {
        "cancelled" => {}
        "pushRejected" => overlay::toast_error(
            with_detail(format!("{label} was rejected because the remote has commits you don't have locally. Pull first, then push again."), error),
            window,
            cx,
        ),
        "noUpstream" => overlay::toast_error("This branch is not published yet.", window, cx),
        "dirtyWorkTree" => {
            let message = with_detail(format!("{label} would overwrite your local changes."), error);
            let Some(retry) = retry else { return overlay::toast_error(message, window, cx) };
            let (ctx, label) = (ctx.clone(), label.to_string());
            window.push_notification(
                Notification::error(message).autohide(false).action(move |_, _, cx| {
                    let (ctx, label, retry) = (ctx.clone(), label.clone(), retry.clone());
                    Button::new("stash-and-continue").label("Stash and continue").on_click(cx.listener(move |this, _, window, cx| {
                        this.dismiss(window, cx);
                        stash_and_retry(ctx.clone(), label.clone(), retry.clone(), window, cx);
                    }))
                }),
                cx,
            );
        }
        "conflict" => {
            show_sidebar(ctx, "scm", cx);
            overlay::toast_warning(format!("{label} stopped because of conflicts. Resolve them in Source Control, then continue."), window, cx);
        }
        "indexLocked" => overlay::toast_error("Another Git process seems to be running in this repository (index.lock exists). Try again in a moment.", window, cx),
        "hookFailed" => {
            if let Some(workbench) = ctx.store.read(cx).env().workbench.upgrade() {
                workbench.update(cx, |workbench, cx| workbench.show_panel("git-output", cx));
            }
            overlay::toast_error(with_detail(format!("{label} was stopped by a Git hook."), error), window, cx);
        }
        "authFailed" => overlay::toast_error(with_detail(format!("{label} failed: authentication was rejected."), error), window, cx),
        "nothingToCommit" => overlay::toast_info("There is nothing to commit.", window, cx),
        "stale" => overlay::toast_warning(error.message.clone(), window, cx),
        _ => overlay::toast_error(with_detail(format!("{label} failed: {}", error.message), error), window, cx),
    }
}

fn stash_and_retry(ctx: Ctx, label: String, retry: Op, window: &mut Window, cx: &mut App) {
    window
        .spawn(cx, async move |cx| {
            let message = format!("Auto-stash before {}", label.to_lowercase());
            let stash = op(move |core, root| {
                let options = oncode_git::StashOptions { message: Some(message.clone()), include_untracked: false, staged_only: false, paths: Vec::new() };
                async move { git::git_stash_push(&core, root, options).await }
            });
            if run(&ctx, "Stash", stash, false, cx).await && run(&ctx, &label, retry, false, cx).await {
                let _ = cx.update(|window, cx| overlay::toast_info("Your changes were stashed. Use Stashes → Pop to restore them.", window, cx));
            }
        })
        .detach();
}

// ----- Dialogs from async tasks -----

pub async fn ask(cx: &mut AsyncWindowContext, options: Confirm) -> bool {
    match cx.update(|window, cx| overlay::confirm(options, window, cx)) {
        Ok(task) => task.await,
        Err(_) => false,
    }
}

pub async fn prompt(cx: &mut AsyncWindowContext, options: TextPrompt) -> Option<String> {
    cx.update(|window, cx| overlay::prompt_text(options, window, cx)).ok()?.await
}

pub async fn choose(cx: &mut AsyncWindowContext, title: String, message: String, choices: Vec<Choice>) -> Option<&'static str> {
    cx.update(|window, cx| overlay::choose(title, message, choices, window, cx)).ok()?.await
}

pub async fn pick(cx: &mut AsyncWindowContext, options: QuickPickOptions) -> Option<PickItem> {
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<PickItem>>();
    let tx = std::cell::Cell::new(Some(tx));
    cx.update(|window, cx| {
        quick_pick(
            options,
            move |item, _, _| {
                if let Some(tx) = tx.take() {
                    let _ = tx.send(item);
                }
            },
            window,
            cx,
        )
    })
    .ok()?;
    rx.await.ok().flatten()
}

fn info(cx: &mut AsyncWindowContext, message: impl Into<SharedString>) {
    let message = message.into();
    let _ = cx.update(|window, cx| overlay::toast_info(message, window, cx));
}

fn error(cx: &mut AsyncWindowContext, message: impl Into<SharedString>) {
    let message = message.into();
    let _ = cx.update(|window, cx| overlay::toast_error(message, window, cx));
}

/// Asks for a ref name; an invalid name is rejected with a message before git runs.
async fn prompt_ref_name(cx: &mut AsyncWindowContext, options: TextPrompt) -> Option<String> {
    let value = prompt(cx, options).await?;
    let name = value.trim().to_string();
    if let Some(problem) = validate_ref_name(&name) {
        error(cx, format!("{problem}: \"{name}\""));
        return None;
    }
    Some(name)
}

pub fn copy_text(text: &str, what: &str, window: &mut Window, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
    overlay::toast_info(format!("{what} copied to clipboard."), window, cx);
}

// ----- Checkout and branch -----

#[derive(Debug, Clone, PartialEq)]
pub enum CheckoutTarget {
    Branch(String),
    Remote(String),
    Detached(String),
}

pub async fn checkout(ctx: &Ctx, target: CheckoutTarget, cx: &mut AsyncWindowContext) -> bool {
    let operation = match target {
        CheckoutTarget::Branch(name) => op(move |core, root| {
            let name = name.clone();
            async move { git::git_switch(&core, root, name).await }
        }),
        CheckoutTarget::Detached(rev) => op(move |core, root| {
            let rev = rev.clone();
            async move { git::git_switch_detached(&core, root, rev).await }
        }),
        CheckoutTarget::Remote(name) => {
            // If a local branch with the same name already exists, just switch to it.
            let local = name.split_once('/').map(|(_, rest)| rest.to_string()).unwrap_or_default();
            let exists = cx.update(|_, cx| ctx.store.read(cx).refs(&ctx.root).is_some_and(|refs| refs.branches.iter().any(|b| b.name == local))).unwrap_or(false);
            if exists {
                op(move |core, root| {
                    let local = local.clone();
                    async move { git::git_switch(&core, root, local).await }
                })
            } else {
                op(move |core, root| {
                    let name = name.clone();
                    async move { git::git_switch_tracking(&core, root, name).await }
                })
            }
        }
    };
    run(ctx, "Checkout", operation, true, cx).await
}

pub async fn create_branch(ctx: &Ctx, start: Option<String>, cx: &mut AsyncWindowContext) {
    let title = match &start {
        Some(start) => format!("Create branch at {}", short_rev(start)),
        None => "Create branch".to_string(),
    };
    let Some(branch) = prompt_ref_name(cx, TextPrompt::new(title, "OK").message("Branch name").placeholder("feature/my-change")).await else { return };
    let operation = op(move |core, root| {
        let (branch, start) = (branch.clone(), start.clone());
        async move { git::git_branch_create(&core, root, branch, start, true).await }
    });
    run(ctx, "Create branch", operation, true, cx).await;
}

pub async fn rename_branch(ctx: &Ctx, name: String, cx: &mut AsyncWindowContext) {
    let Some(next) = prompt_ref_name(cx, TextPrompt::new(format!("Rename branch {name}"), "OK").message("New name").initial(name.clone())).await else { return };
    if next == name {
        return;
    }
    let operation = op(move |core, root| {
        let (name, next) = (name.clone(), next.clone());
        async move { git::git_branch_rename(&core, root, name, next).await }
    });
    run(ctx, "Rename branch", operation, false, cx).await;
}

/// Confirmation text for deleting a branch that has not been merged.
pub fn unmerged_branch_message(count: u32) -> String {
    let (exist, them) = if count == 1 { ("exists", "it") } else { ("exist", "them") };
    format!(
        "{} on this branch {exist} nowhere else. Deleting the branch makes {them} unreachable (recoverable only through the reflog for a limited time).",
        plural(count as usize, "commit")
    )
}

pub async fn delete_branch(ctx: &Ctx, name: String, cx: &mut AsyncWindowContext) {
    let delete = |force: bool| {
        let name = name.clone();
        op(move |core, root| {
            let name = name.clone();
            async move { git::git_branch_delete(&core, root, name, force).await }
        })
    };
    let error = match run_raw(ctx, "Delete branch", &delete(false), cx).await {
        Ok(()) => return,
        Err(error) => error,
    };
    if error.code != "branchNotMerged" {
        let ctx = ctx.clone();
        let _ = cx.update(|window, cx| handle_git_error(&ctx, &error, "Delete branch", None, window, cx));
        return;
    }
    let count = {
        let (root, branch) = (ctx.root.clone(), name.clone());
        match cx.update(|_, cx| {
            let core = ctx.core(cx);
            spawn_core(cx, async move { git::git_branch_unmerged(&core, root, branch).await })
        }) {
            Ok(task) => task.await.unwrap_or(0),
            Err(_) => return,
        }
    };
    let confirmed = ask(cx, Confirm::new(format!("Delete unmerged branch {name}?"), unmerged_branch_message(count), "Delete Anyway").danger()).await;
    if confirmed {
        run(ctx, "Delete branch", delete(true), false, cx).await;
    }
}

pub async fn delete_remote_branch(ctx: &Ctx, full_name: String, cx: &mut AsyncWindowContext) {
    let Some((remote, branch)) = full_name.split_once('/').map(|(r, b)| (r.to_string(), b.to_string())) else { return };
    let confirmed = ask(cx, Confirm::new(format!("Delete {branch} on {remote}?"), "This deletes the branch on the server for everyone.", "Delete Remote Branch").danger()).await;
    if !confirmed {
        return;
    }
    let operation = op(move |core, root| {
        let (remote, branch) = (remote.clone(), branch.clone());
        async move { git::git_branch_delete_remote(&core, root, remote, branch).await }
    });
    run(ctx, "Delete remote branch", operation, false, cx).await;
}

/// Picks an upstream for a local branch from the list of remote branches.
pub async fn set_upstream(ctx: &Ctx, branch: String, cx: &mut AsyncWindowContext) {
    const REMOVE: &str = "\0remove";
    let items = cx
        .update(|_, cx| {
            let store = ctx.store.read(cx);
            let Some(refs) = store.refs(&ctx.root) else { return Vec::new() };
            let current = refs.branches.iter().find(|b| b.name == branch).and_then(|b| b.upstream.clone());
            let mut items: Vec<PickItem> = refs
                .remote_branches
                .iter()
                .filter(|b| !b.name.ends_with("/HEAD"))
                .map(|b| {
                    let item = PickItem::new(b.name.clone(), b.name.clone());
                    if Some(&b.name) == current.as_ref() {
                        item.description("current upstream")
                    } else {
                        item
                    }
                })
                .collect();
            if let Some(current) = current {
                items.push(PickItem::new("Remove upstream", REMOVE).description(current));
            }
            items
        })
        .unwrap_or_default();
    if items.is_empty() {
        return info(cx, "There are no remote branches yet. Push the branch to publish it.");
    }
    let Some(picked) = pick(cx, QuickPickOptions::new(format!("Upstream for {branch}"), items)).await else { return };
    let upstream = (picked.value != REMOVE).then_some(picked.value);
    let operation = op(move |core, root| {
        let (branch, upstream) = (branch.clone(), upstream.clone());
        async move { git::git_branch_set_upstream(&core, root, branch, upstream).await }
    });
    run(ctx, "Set upstream", operation, false, cx).await;
}

// ----- Push -----

pub async fn push(ctx: &Ctx, branch: Option<String>, set_upstream: bool, cx: &mut AsyncWindowContext) -> bool {
    let Ok((snapshot, remotes)) = cx.update(|_, cx| {
        let remotes: Vec<(String, String)> =
            ctx.store.read(cx).refs(&ctx.root).map(|refs| refs.remotes.iter().map(|r| (r.name.clone(), if r.push_url.is_empty() { r.fetch_url.clone() } else { r.push_url.clone() })).collect()).unwrap_or_default();
        (snapshot(&ctx.root, cx), remotes)
    }) else {
        return false;
    };
    let mut options = PushOptions { remote: None, branch, set_upstream, force_with_lease: false, tags: false };
    if let Some(snapshot) = &snapshot {
        if matches!(snapshot.head, oncode_git::Head::Detached { .. }) && options.branch.is_none() {
            info(cx, "You are not on a branch. Create a branch before pushing.");
            return false;
        }
        if snapshot.upstream.is_none() && !options.set_upstream && options.branch.is_none() {
            options.set_upstream = true;
        }
    }
    if options.set_upstream {
        if remotes.is_empty() {
            error(cx, "This repository has no remote yet. Add one with “Git: Add Remote…”.");
            return false;
        }
        if remotes.len() > 1 {
            let items = remotes.iter().map(|(name, url)| PickItem::new(name.clone(), name.clone()).description(url.clone())).collect();
            let Some(picked) = pick(cx, QuickPickOptions::new("Publish to which remote?", items)).await else { return false };
            options.remote = Some(picked.value);
        }
    }
    let label = if options.set_upstream { "Publish" } else { "Push" };
    run_with_progress(ctx, label, move |core, root, id, progress| async move { git::git_push(&core, root, options, id, progress).await }, cx).await
}

pub async fn fetch(ctx: &Ctx, cx: &mut AsyncWindowContext) -> bool {
    run_with_progress(ctx, "Fetch", move |core, root, id, progress| async move { git::git_fetch(&core, root, None, id, progress).await }, cx).await
}

// ----- Merge, rebase -----

pub async fn merge(ctx: &Ctx, rev: String, cx: &mut AsyncWindowContext) {
    let current = cx.update(|_, cx| head_label(snapshot(&ctx.root, cx).as_ref())).unwrap_or_default();
    let answer = choose(
        cx,
        format!("Merge {rev} into {current}?"),
        "Merge fast-forwards when possible. “No Fast-Forward” always creates a merge commit; “Squash” stages the combined changes without committing.".to_string(),
        vec![Choice::new("cancel", "Cancel"), Choice::new("squash", "Squash"), Choice::new("noFf", "No Fast-Forward"), Choice::new("merge", "Merge").primary()],
    )
    .await;
    let mode = match answer {
        Some("merge") => MergeMode::Default,
        Some("noFf") => MergeMode::NoFf,
        Some("squash") => MergeMode::Squash,
        _ => return,
    };
    let operation = op(move |core, root| {
        let rev = rev.clone();
        async move { git::git_merge(&core, root, rev, mode).await }
    });
    run(ctx, "Merge", operation, true, cx).await;
}

pub async fn rebase_onto(ctx: &Ctx, onto: String, cx: &mut AsyncWindowContext) {
    let current = cx.update(|_, cx| head_label(snapshot(&ctx.root, cx).as_ref())).unwrap_or_default();
    let confirmed = ask(
        cx,
        Confirm::new(
            format!("Rebase {current} onto {onto}?"),
            "Your commits are re-created on top of the target. If they were already pushed, you will need to force push afterwards.",
            "Rebase",
        ),
    )
    .await;
    if !confirmed {
        return;
    }
    let operation = op(move |core, root| {
        let onto = onto.clone();
        async move { git::git_rebase(&core, root, onto).await }
    });
    run(ctx, "Rebase", operation, true, cx).await;
}

// ----- Cherry-pick, revert, reset, tag -----

pub async fn cherry_pick(ctx: &Ctx, oids: Vec<String>, cx: &mut AsyncWindowContext) -> bool {
    let operation = op(move |core, root| {
        let oids = oids.clone();
        async move { git::git_cherry_pick(&core, root, oids).await }
    });
    run(ctx, "Cherry-pick", operation, true, cx).await
}

pub async fn revert(ctx: &Ctx, oid: String, subject: String, cx: &mut AsyncWindowContext) {
    let message = format!("A new commit is created that undoes:\n{}  {subject}", crate::format::short_oid(&oid));
    if !ask(cx, Confirm::new("Revert commit?", message, "Revert")).await {
        return;
    }
    let operation = op(move |core, root| {
        let oid = oid.clone();
        async move { git::git_revert(&core, root, oid).await }
    });
    run(ctx, "Revert", operation, true, cx).await;
}

pub fn reset_mode_name(mode: ResetMode) -> &'static str {
    match mode {
        ResetMode::Soft => "soft",
        ResetMode::Mixed => "mixed",
        ResetMode::Hard => "hard",
    }
}

/// Title, message, and button label of the reset confirmation; the message states what is lost.
pub fn reset_confirmation(current: &str, rev: &str, mode: ResetMode, changed_files: usize) -> Confirm {
    let short = short_rev(rev);
    let message = match mode {
        ResetMode::Hard => format!(
            "{current} moves to {short} and ALL uncommitted changes in tracked files are discarded{}\n\nCommits that become unreachable can still be found in the reflog (Git: Show Reflog) for a limited time; discarded uncommitted changes cannot be recovered.",
            if changed_files > 0 { format!(" ({} currently changed).", plural(changed_files, "file")) } else { ".".to_string() }
        ),
        ResetMode::Soft => format!("{current} moves to {short}. The changes from the commits in between stay staged. Nothing is lost."),
        ResetMode::Mixed => format!("{current} moves to {short}. The changes from the commits in between stay in your files, unstaged. Nothing is lost."),
    };
    let confirm = Confirm::new(format!("Reset {current} to {short} ({})?", reset_mode_name(mode)), message, if mode == ResetMode::Hard { "Reset and Discard" } else { "Reset" });
    if mode == ResetMode::Hard {
        confirm.danger()
    } else {
        confirm
    }
}

pub async fn reset(ctx: &Ctx, rev: String, mode: ResetMode, cx: &mut AsyncWindowContext) {
    let snapshot = cx.update(|_, cx| snapshot(&ctx.root, cx)).ok().flatten();
    let changed = snapshot
        .as_ref()
        .map(|s| s.staged.iter().chain(s.unstaged.iter()).map(|c| c.path.as_str()).collect::<std::collections::HashSet<_>>().len())
        .unwrap_or(0);
    let current = head_label(snapshot.as_ref());
    if !ask(cx, reset_confirmation(&current, &rev, mode, changed)).await {
        return;
    }
    let operation = op(move |core, root| {
        let rev = rev.clone();
        async move { git::git_reset(&core, root, rev, mode).await }
    });
    run(ctx, "Reset", operation, false, cx).await;
}

pub async fn push_tag(ctx: &Ctx, name: String, cx: &mut AsyncWindowContext) -> bool {
    let Some(remote) = cx.update(|_, cx| ctx.store.read(cx).first_remote(&ctx.root)).ok().flatten() else {
        error(cx, "This repository has no remote yet.");
        return false;
    };
    let (tag, target) = (name.clone(), remote.clone());
    let operation = op(move |core, root| {
        let (remote, name) = (target.clone(), tag.clone());
        async move { git::git_tag_push(&core, root, remote, name).await }
    });
    let pushed = run(ctx, "Push tag", operation, false, cx).await;
    if pushed {
        let _ = cx.update(|window, cx| overlay::toast_success(format!("Tag {name} pushed to {remote}."), window, cx));
    }
    pushed
}

pub async fn create_tag(ctx: &Ctx, rev: Option<String>, cx: &mut AsyncWindowContext) {
    let Some(name) = prompt_ref_name(cx, TextPrompt::new("Create tag", "OK").message("Tag name").placeholder("v1.0.0")).await else { return };
    let Some(message) = prompt(cx, TextPrompt::new(format!("Create tag {name}"), "Create Tag").message("Message (leave empty for a lightweight tag)")).await else { return };
    let message = (!message.trim().is_empty()).then(|| message.trim().to_string());
    let tag = name.clone();
    let operation = op(move |core, root| {
        let (name, rev, message) = (tag.clone(), rev.clone(), message.clone());
        async move { git::git_tag_create(&core, root, name, rev, message).await }
    });
    if !run(ctx, "Create tag", operation, false, cx).await {
        return;
    }
    let remote = cx.update(|_, cx| ctx.store.read(cx).first_remote(&ctx.root)).ok().flatten();
    let ctx = ctx.clone();
    let _ = cx.update(|window, cx| match remote {
        Some(remote) => {
            let label: SharedString = format!("Push to {remote}").into();
            window.push_notification(
                Notification::success(format!("Tag {name} created.")).action(move |_, _, cx| {
                    let (ctx, name) = (ctx.clone(), name.clone());
                    Button::new("push-tag").label(label.clone()).on_click(cx.listener(move |this, _, window, cx| {
                        this.dismiss(window, cx);
                        let (ctx, name) = (ctx.clone(), name.clone());
                        window
                            .spawn(cx, async move |cx| {
                                push_tag(&ctx, name, cx).await;
                            })
                            .detach();
                    }))
                }),
                cx,
            );
        }
        None => overlay::toast_success(format!("Tag {name} created."), window, cx),
    });
}

pub async fn delete_tag(ctx: &Ctx, name: String, cx: &mut AsyncWindowContext) {
    let remote = cx.update(|_, cx| ctx.store.read(cx).first_remote(&ctx.root)).ok().flatten();
    let mut choices = vec![Choice::new("cancel", "Cancel")];
    if let Some(remote) = &remote {
        choices.push(Choice::new("both", format!("Delete Locally and on {remote}")).danger());
    }
    choices.push(Choice::new("local", "Delete Locally").danger());
    let message = match &remote {
        Some(remote) => format!("The tag is removed from this repository. You can also delete it on {remote}."),
        None => "The tag is removed from this repository. The commit it points to is kept.".to_string(),
    };
    let answer = choose(cx, format!("Delete tag {name}?"), message, choices).await;
    if !matches!(answer, Some("local") | Some("both")) {
        return;
    }
    let tag = name.clone();
    let local = op(move |core, root| {
        let name = tag.clone();
        async move { git::git_tag_delete(&core, root, name).await }
    });
    if !run(ctx, "Delete tag", local, false, cx).await {
        return;
    }
    if let (Some("both"), Some(remote)) = (answer, remote) {
        let operation = op(move |core, root| {
            let (remote, name) = (remote.clone(), name.clone());
            async move { git::git_tag_delete_remote(&core, root, remote, name).await }
        });
        run(ctx, "Delete remote tag", operation, false, cx).await;
    }
}

// ----- Stash -----

pub async fn stash_apply(ctx: &Ctx, name: String, pop: bool, cx: &mut AsyncWindowContext) -> bool {
    let operation = op(move |core, root| {
        let name = name.clone();
        async move { git::git_stash_apply(&core, root, name, pop).await }
    });
    run(ctx, if pop { "Pop stash" } else { "Apply stash" }, operation, false, cx).await
}

pub async fn stash_drop(ctx: &Ctx, name: String, message: String, cx: &mut AsyncWindowContext) {
    let confirmed = ask(cx, Confirm::new(format!("Drop {name}?"), format!("{message}\n\nThe stashed changes are deleted permanently."), "Drop Stash").danger()).await;
    if !confirmed {
        return;
    }
    let operation = op(move |core, root| {
        let name = name.clone();
        async move { git::git_stash_drop(&core, root, name).await }
    });
    run(ctx, "Drop stash", operation, false, cx).await;
}

pub async fn stash_branch(ctx: &Ctx, name: String, cx: &mut AsyncWindowContext) {
    let Some(branch) = prompt_ref_name(cx, TextPrompt::new(format!("Create branch from {name}"), "OK").message("Branch name")).await else { return };
    let operation = op(move |core, root| {
        let (name, branch) = (name.clone(), branch.clone());
        async move { git::git_stash_branch(&core, root, name, branch).await }
    });
    run(ctx, "Stash to branch", operation, false, cx).await;
}

// ----- Dispatcher -----

/// Runs the action of a menu item. `CompareWithHead` is handled by the Graph tab itself.
pub fn run_action(ctx: &Ctx, action: GitAction, window: &mut Window, cx: &mut App) {
    match action {
        GitAction::Copy { text, what } => return copy_text(&text, what, window, cx),
        GitAction::ShowStash(name) => return crate::open_stash(&ctx.store, &ctx.root, &name, window, cx),
        GitAction::ShowInGraph { oid, label } => return crate::open_graph(&ctx.store, &ctx.root, Some(RevealTarget { oid, label, hunt: true }), window, cx),
        GitAction::InteractiveRebase { base } => return crate::rebase_dialog::open(ctx.clone(), base, window, cx),
        GitAction::CompareWithHead(_) => return,
        _ => {}
    }
    let ctx = ctx.clone();
    window
        .spawn(cx, async move |cx| match action {
            GitAction::CheckoutBranch(name) => {
                checkout(&ctx, CheckoutTarget::Branch(name), cx).await;
            }
            GitAction::CheckoutRemote(name) => {
                checkout(&ctx, CheckoutTarget::Remote(name), cx).await;
            }
            GitAction::CheckoutDetached(rev) => {
                checkout(&ctx, CheckoutTarget::Detached(rev), cx).await;
            }
            GitAction::CreateBranch(start) => create_branch(&ctx, Some(start), cx).await,
            GitAction::CreateTag(rev) => create_tag(&ctx, Some(rev), cx).await,
            GitAction::CherryPick(oids) => {
                cherry_pick(&ctx, oids, cx).await;
            }
            GitAction::Revert { oid, subject } => revert(&ctx, oid, subject, cx).await,
            GitAction::Reset { rev, mode } => reset(&ctx, rev, mode, cx).await,
            GitAction::Merge(rev) => merge(&ctx, rev, cx).await,
            GitAction::RebaseOnto(onto) => rebase_onto(&ctx, onto, cx).await,
            GitAction::RenameBranch(name) => rename_branch(&ctx, name, cx).await,
            GitAction::DeleteBranch(name) => delete_branch(&ctx, name, cx).await,
            GitAction::Push { branch, set_upstream } => {
                push(&ctx, branch, set_upstream, cx).await;
            }
            GitAction::SetUpstream(branch) => set_upstream(&ctx, branch, cx).await,
            GitAction::DeleteRemoteBranch(name) => delete_remote_branch(&ctx, name, cx).await,
            GitAction::DeleteTag(name) => delete_tag(&ctx, name, cx).await,
            GitAction::PushTag(name) => {
                push_tag(&ctx, name, cx).await;
            }
            GitAction::StashApply { name, pop } => {
                stash_apply(&ctx, name, pop, cx).await;
            }
            GitAction::StashBranch(name) => stash_branch(&ctx, name, cx).await,
            GitAction::StashDrop { name, message } => stash_drop(&ctx, name, message, cx).await,
            GitAction::Copy { .. } | GitAction::ShowStash(_) | GitAction::ShowInGraph { .. } | GitAction::InteractiveRebase { .. } | GitAction::CompareWithHead(_) => {}
        })
        .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::oid;

    #[test]
    fn reset_confirmation_says_what_is_lost() {
        let hard = reset_confirmation("main", &oid(3), ResetMode::Hard, 2);
        assert_eq!(hard.title.as_ref(), format!("Reset main to {} (hard)?", &oid(3)[..8]));
        assert!(hard.message.contains("ALL uncommitted changes in tracked files are discarded (2 files currently changed)."), "{}", hard.message);
        assert!(hard.message.contains("cannot be recovered"));
        assert_eq!(hard.ok.as_ref(), "Reset and Discard");
        assert!(hard.danger);

        let clean = reset_confirmation("main", "v1.0.0", ResetMode::Hard, 0);
        assert!(clean.message.starts_with("main moves to v1.0.0 and ALL uncommitted changes in tracked files are discarded.\n"), "{}", clean.message);

        let soft = reset_confirmation("main", &oid(3), ResetMode::Soft, 5);
        assert!(soft.message.ends_with("stay staged. Nothing is lost."));
        assert_eq!((soft.ok.as_ref(), soft.danger), ("Reset", false));
        let mixed = reset_confirmation("main", &oid(3), ResetMode::Mixed, 5);
        assert!(mixed.message.ends_with("stay in your files, unstaged. Nothing is lost."));
        assert!(mixed.title.ends_with("(mixed)?"));
    }

    #[test]
    fn unmerged_branch_message_agrees_in_number() {
        assert!(unmerged_branch_message(1).starts_with("1 commit on this branch exists nowhere else. Deleting the branch makes it unreachable"));
        assert!(unmerged_branch_message(3).starts_with("3 commits on this branch exist nowhere else. Deleting the branch makes them unreachable"));
    }
}
