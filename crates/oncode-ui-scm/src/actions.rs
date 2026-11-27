//! Git actions from the UI side: confirmation for destructive actions, progress, and errors that
//! can be acted on. Each action is an `async` flow on the main thread; backend work
//! runs on the tokio runtime through `spawn_core`.

use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{App, AsyncWindowContext, Entity, SharedString, WeakEntity, Window};
use oncode_core::api::{app, git as backend};
use oncode_core::{AppError, AppResult, Settings, Shared};
use oncode_git::{
    CommitOptions, ConflictSide, DiffSource, Head, MergeMode, OperationAction, Progress, ProgressFn, PullStrategy, PushOptions, Refs, Remote, RepoSnapshot,
    StashOptions,
};
use oncode_ui::editor::{open_file, OpenFile};
use oncode_ui::overlay::{self, Choice, Confirm};
use oncode_ui::quick_pick::{quick_pick, PickItem, QuickPickOptions};
use oncode_ui::{spawn_core, Workbench};
use oncode_ui_diff::DiffRequest;

use crate::logic::{self, plural, CheckoutTarget, FollowUp, NoticeLevel};
use crate::prompts::{self, InputPrompt, ToastAction};
use crate::store::{self, Busy, ScmStore};

/// Ids of the Git Output panel and the sidebars that error follow-ups point to.
pub const GIT_OUTPUT_PANEL: &str = "git-output";
pub const SCM_SIDEBAR: &str = "scm";
const ACCOUNTS_SIDEBAR: &str = "pullRequests";

/// A window's repository with everything needed to run actions on it.
#[derive(Clone)]
pub struct Git {
    pub core: Shared,
    /// The window label for the `oncode-core` API.
    pub label: SharedString,
    /// The repository root (worktree).
    pub root: String,
    pub store: Entity<ScmStore>,
    pub workbench: WeakEntity<Workbench>,
}

impl Git {
    pub fn from_store(store: &Entity<ScmStore>, cx: &App) -> Option<Self> {
        let state = store.read(cx);
        let env = state.env();
        Some(Self { core: env.core.clone(), label: env.window.clone(), root: state.root()?.to_string(), store: store.clone(), workbench: env.workbench.clone() })
    }

    /// This window's repository. Do not call while `Workbench` is borrowed.
    pub fn for_window(window: &Window, cx: &App) -> Option<Self> {
        let workbench = Workbench::for_window(window, cx)?;
        let store = store::store(workbench.read(cx))?;
        Self::from_store(&store, cx)
    }

    pub fn settings(&self) -> Settings {
        self.core.effective_settings(&self.label)
    }

    pub fn snapshot(&self, cx: &App) -> Option<RepoSnapshot> {
        self.store.read(cx).snapshot(cx)
    }

    /// Opens the diff tab of a file in this repository.
    pub fn open_diff(&self, path: &str, orig_path: Option<String>, source: DiffSource, preview: bool, window: &mut Window, cx: &mut App) {
        let request = DiffRequest { root: self.root.clone(), path: path.to_string(), orig_path, source, preview };
        let _ = self.workbench.update(cx, |workbench, cx| oncode_ui_diff::open_diff(workbench, request, window, cx));
    }

    pub fn open_conflict(&self, path: &str, window: &mut Window, cx: &mut App) {
        let (root, path) = (self.root.clone(), path.to_string());
        let _ = self.workbench.update(cx, |workbench, cx| oncode_ui_diff::open_conflict(workbench, root, path, window, cx));
    }

    /// Opens the working file in the editor (not a preview).
    pub fn open_file(&self, path: &str, window: &mut Window, cx: &mut App) {
        let full = logic::repo_file(&self.root, path);
        let _ = self.workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(full), window, cx));
    }

    fn show_sidebar(&self, id: &'static str, cx: &mut App) {
        let _ = self.workbench.update(cx, |workbench, cx| workbench.show_sidebar(id, cx));
    }

    fn show_panel(&self, id: &'static str, cx: &mut App) {
        let _ = self.workbench.update(cx, |workbench, cx| workbench.show_panel(id, cx));
    }
}

/// Runs an action flow on this window's main thread.
pub fn spawn(window: &mut Window, cx: &mut App, flow: impl AsyncFnOnce(&mut AsyncWindowContext) + 'static) {
    window.spawn(cx, async move |cx| flow(cx).await).detach();
}

pub type OpFuture = Pin<Box<dyn Future<Output = AppResult<()>> + Send>>;

/// A backend operation that can be run again (e.g. after an automatic stash): takes the core and the repository root.
pub type Op = Rc<dyn Fn(Shared, String) -> OpFuture>;

pub fn op<F, Fut>(f: F) -> Op
where
    F: Fn(Shared, String) -> Fut + 'static,
    Fut: Future<Output = AppResult<()>> + Send + 'static,
{
    Rc::new(move |core, root| Box::pin(f(core, root)))
}

// ----- Flow helpers -----

fn ui<R>(cx: &mut AsyncWindowContext, f: impl FnOnce(&mut Window, &mut App) -> R) -> Option<R> {
    cx.update(f).ok()
}

async fn core_call<T: Send + 'static>(cx: &mut AsyncWindowContext, future: impl Future<Output = AppResult<T>> + Send + 'static) -> AppResult<T> {
    match cx.update(|_, cx| spawn_core(cx, future)) {
        Ok(task) => task.await,
        Err(_) => Err(AppError::new("cancelled", "The window was closed")),
    }
}

fn set_busy(git: &Git, busy: Option<Busy>, cx: &mut AsyncWindowContext) {
    ui(cx, |_, cx| git.store.update(cx, |store, cx| store.set_busy(busy, cx)));
}

fn snapshot(git: &Git, cx: &mut AsyncWindowContext) -> Option<RepoSnapshot> {
    ui(cx, |_, cx| git.snapshot(cx)).flatten()
}

fn info(cx: &mut AsyncWindowContext, key: &'static str, message: impl Into<SharedString>) {
    let message = message.into();
    ui(cx, |window, cx| prompts::toast_info(key, message, window, cx));
}

async fn confirm(cx: &mut AsyncWindowContext, options: Confirm) -> bool {
    match ui(cx, |window, cx| overlay::confirm(options, window, cx)) {
        Some(answer) => answer.await,
        None => false,
    }
}

async fn choose(cx: &mut AsyncWindowContext, title: &str, message: String, choices: Vec<Choice>) -> Option<&'static str> {
    let title = title.to_string();
    ui(cx, |window, cx| overlay::choose(title, message, choices, window, cx))?.await
}

async fn input(cx: &mut AsyncWindowContext, options: InputPrompt) -> Option<String> {
    ui(cx, |window, cx| prompts::prompt_input(options, window, cx))?.await
}

async fn pick(cx: &mut AsyncWindowContext, options: QuickPickOptions) -> Option<PickItem> {
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<PickItem>>();
    let tx = std::cell::Cell::new(Some(tx));
    ui(cx, |window, cx| {
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
    })?;
    rx.await.ok().flatten()
}

fn ref_name_validator(value: &str) -> Option<String> {
    logic::validate_ref_name(value).map(String::from)
}

/// Runs a Git operation with the busy indicator and returns its raw result.
async fn run<T: Send + 'static>(git: &Git, label: &str, cx: &mut AsyncWindowContext, future: impl Future<Output = AppResult<T>> + Send + 'static) -> AppResult<T> {
    set_busy(git, Some(Busy::new(label.to_string())), cx);
    let result = core_call(cx, future).await;
    set_busy(git, None, cx);
    result
}

/// Like [`run`], but errors are shown; `None` means it failed.
async fn run_shown<T: Send + 'static>(git: &Git, label: &str, cx: &mut AsyncWindowContext, future: impl Future<Output = AppResult<T>> + Send + 'static) -> Option<T> {
    match run(git, label, cx, future).await {
        Ok(value) => Some(value),
        Err(error) => {
            handle_error(git, &error, label, None, cx);
            None
        }
    }
}

/// Runs a repeatable operation; if it fails because of local changes, offers a stash and then repeats.
async fn run_op(git: &Git, label: &str, operation: Op, cx: &mut AsyncWindowContext) -> bool {
    match run(git, label, cx, operation(git.core.clone(), git.root.clone())).await {
        Ok(()) => true,
        Err(error) => {
            handle_error(git, &error, label, Some(operation), cx);
            false
        }
    }
}

/// Runs a network operation with progress and a cancel button; returns its error so the caller can decide.
async fn run_progress<Fut>(git: &Git, label: &str, cx: &mut AsyncWindowContext, make: impl FnOnce(Shared, String, String, ProgressFn) -> Fut) -> AppResult<()>
where
    Fut: Future<Output = AppResult<()>> + Send + 'static,
{
    let operation_id = format!("scm-{}-{}", git.label, git.core.next_id());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Progress>();
    let on_progress: ProgressFn = Arc::new(move |progress| {
        let _ = tx.send(progress);
    });
    set_busy(git, Some(Busy { label: label.to_string().into(), percent: None, operation_id: Some(operation_id.clone()) }), cx);
    // Progress comes from the backend thread; it is forwarded to the store on the main thread.
    let pump = cx.spawn({
        let (store, id, label) = (git.store.downgrade(), operation_id.clone(), label.to_string());
        async move |cx| {
            while let Some(progress) = rx.recv().await {
                let text: SharedString = format!("{label}: {}", progress.phase).into();
                if store.update(cx, |store, cx| store.set_progress(&id, text, progress.percent, cx)).is_err() {
                    break;
                }
            }
        }
    });
    let result = core_call(cx, make(git.core.clone(), git.root.clone(), operation_id, on_progress)).await;
    drop(pump);
    set_busy(git, None, cx);
    result
}

/// Cancels the network operation that is running.
pub fn cancel_busy(git: &Git, cx: &mut App) {
    let Some(id) = git.store.read(cx).busy().and_then(|busy| busy.operation_id.clone()) else { return };
    let core = git.core.clone();
    spawn_core(cx, async move { app::app_cancel(&core, id).await }).detach();
}

/// Translates a Git error code into a message with next-action buttons.
pub fn handle_error(git: &Git, error: &AppError, label: &str, retry: Option<Op>, cx: &mut AsyncWindowContext) {
    let Some(notice) = logic::error_notice(&error.code, &error.message, label, retry.is_some()) else { return };
    let mut actions: Vec<ToastAction> = notice.follow_ups.iter().map(|follow_up| follow_up_action(git, *follow_up, label, retry.clone())).collect();
    let with_details = !matches!(error.code.as_str(), "noUpstream" | "nothingToCommit" | "stale");
    if let Some(detail) = error.detail.clone().filter(|detail| with_details && !detail.trim().is_empty()) {
        let (title, message) = (label.to_string(), error.message.clone());
        actions.push(ToastAction::new("details", "Details", move |window, cx| {
            let text = if detail.trim() == message.trim() { detail.trim().to_string() } else { format!("{message}\n\n{}", detail.trim()) };
            overlay::choose(title.clone(), text, vec![Choice::new("ok", "Close").primary()], window, cx).detach();
        }));
    }
    let (git, key) = (git.clone(), error.code.clone());
    ui(cx, move |window, cx| {
        if notice.reveal_scm {
            git.show_sidebar(SCM_SIDEBAR, cx);
        }
        prompts::toast(notice.level, &key, notice.message, actions, window, cx);
    });
}

fn follow_up_action(git: &Git, follow_up: FollowUp, label: &str, retry: Option<Op>) -> ToastAction {
    let (git, label) = (git.clone(), label.to_string());
    ToastAction::new(follow_up.slug(), follow_up.label(), move |window, cx| {
        let (git, label, retry) = (git.clone(), label.clone(), retry.clone());
        match follow_up {
            FollowUp::PullThenPush => spawn(window, cx, async move |cx| {
                if pull(&git, cx).await {
                    push(&git, PushOptions::default(), cx).await;
                }
            }),
            FollowUp::ForcePush => spawn(window, cx, async move |cx| {
                push(&git, PushOptions { force_with_lease: true, ..Default::default() }, cx).await;
            }),
            FollowUp::PublishBranch => spawn(window, cx, async move |cx| {
                push(&git, PushOptions { set_upstream: true, ..Default::default() }, cx).await;
            }),
            FollowUp::StashAndContinue => spawn(window, cx, async move |cx| {
                let Some(retry) = retry else { return };
                let options = StashOptions { message: Some(format!("Auto-stash before {}", label.to_lowercase())), ..Default::default() };
                let (core, root) = (git.core.clone(), git.root.clone());
                if run_shown(&git, "Stash", cx, async move { backend::git_stash_push(&core, root, options).await }).await.is_some() && run_op(&git, &label, retry, cx).await {
                    info(cx, "stashed", "Your changes were stashed. Use Stashes → Pop to restore them.");
                }
            }),
            FollowUp::ShowOutput => git.show_panel(GIT_OUTPUT_PANEL, cx),
            FollowUp::Accounts => git.show_sidebar(ACCOUNTS_SIDEBAR, cx),
        }
    })
}

// ----- Staging -----

pub async fn stage(git: &Git, paths: Vec<String>, cx: &mut AsyncWindowContext) -> bool {
    let (core, root) = (git.core.clone(), git.root.clone());
    run_shown(git, "Stage", cx, async move { backend::git_stage(&core, root, paths).await }).await.is_some()
}

pub async fn stage_all(git: &Git, cx: &mut AsyncWindowContext) -> bool {
    let (core, root) = (git.core.clone(), git.root.clone());
    run_shown(git, "Stage all", cx, async move { backend::git_stage_all(&core, root).await }).await.is_some()
}

pub async fn unstage(git: &Git, paths: Vec<String>, cx: &mut AsyncWindowContext) -> bool {
    let (core, root) = (git.core.clone(), git.root.clone());
    run_shown(git, "Unstage", cx, async move { backend::git_unstage(&core, root, paths).await }).await.is_some()
}

pub async fn unstage_all(git: &Git, cx: &mut AsyncWindowContext) -> bool {
    let (core, root) = (git.core.clone(), git.root.clone());
    run_shown(git, "Unstage all", cx, async move { backend::git_unstage_all(&core, root).await }).await.is_some()
}

/// Discards changes after a confirmation that names what will be lost.
pub async fn discard(git: &Git, tracked: Vec<String>, untracked: Vec<String>, cx: &mut AsyncWindowContext) -> bool {
    let Some((title, message)) = logic::discard_prompt(&tracked, &untracked) else { return false };
    if !confirm(cx, Confirm::new(title, message, "Discard").danger()).await {
        return false;
    }
    let (core, root) = (git.core.clone(), git.root.clone());
    run_shown(git, "Discard", cx, async move { backend::git_discard(&core, root, tracked, untracked).await }).await.is_some()
}

/// Discards all working tree changes (tracked and untracked).
pub async fn discard_all(git: &Git, cx: &mut AsyncWindowContext) -> bool {
    let Some(snapshot) = snapshot(git, cx) else { return false };
    let tracked = snapshot.unstaged.iter().map(|change| change.path.clone()).collect();
    discard(git, tracked, snapshot.untracked.clone(), cx).await
}

// ----- Commit -----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterCommit {
    Push,
    Sync,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitRequest {
    pub message: String,
    pub amend: bool,
    pub signoff: bool,
    pub no_verify: bool,
    /// Sign the commit (`-S`, with the GPG or SSH key from git's configuration).
    pub sign: bool,
    pub then: Option<AfterCommit>,
}

fn valid_email(value: &str) -> bool {
    let value = value.trim();
    !value.chars().any(char::is_whitespace) && value.split_once('@').is_some_and(|(user, host)| !user.is_empty() && !host.is_empty())
}

/// Makes sure `user.name` and `user.email` are set; if not, asks for them and saves them globally.
async fn ensure_identity(git: &Git, cx: &mut AsyncWindowContext) -> bool {
    let (core, root) = (git.core.clone(), git.root.clone());
    let identity = match core_call(cx, async move { backend::git_identity(&core, root).await }).await {
        Ok(identity) => identity,
        Err(error) => {
            handle_error(git, &error, "Commit", None, cx);
            return false;
        }
    };
    let filled = |value: &Option<String>| value.as_ref().is_some_and(|v| !v.trim().is_empty());
    if filled(&identity.name) && filled(&identity.email) {
        return true;
    }
    const TITLE: &str = "Git needs to know who you are";
    let Some(name) = input(cx, InputPrompt::new(TITLE, "Your name (user.name)").value(identity.name.unwrap_or_default())).await else { return false };
    let email = InputPrompt::new(TITLE, "Your email (user.email)")
        .value(identity.email.unwrap_or_default())
        .validate(|value| (!valid_email(value)).then(|| "Enter an email address".to_string()));
    let Some(email) = input(cx, email).await else { return false };
    let (core, root, name, email) = (git.core.clone(), git.root.clone(), name.trim().to_string(), email.trim().to_string());
    run_shown(git, "Set identity", cx, async move { backend::git_set_identity(&core, root, name, email, true).await }).await.is_some()
}

pub async fn commit(git: &Git, request: CommitRequest, cx: &mut AsyncWindowContext) -> bool {
    let Some(snapshot) = snapshot(git, cx) else { return false };
    if request.message.trim().is_empty() {
        info(cx, "commit-message", "Enter a commit message first.");
        return false;
    }
    if !ensure_identity(git, cx).await {
        return false;
    }
    if snapshot.staged.is_empty() && !request.amend {
        let count = snapshot.unstaged.len() + snapshot.untracked.len();
        if count == 0 {
            info(cx, "nothing-to-commit", "There are no changes to commit.");
            return false;
        }
        let message = format!("Stage all {} and commit them?", plural(count, "changed file"));
        if !confirm(cx, Confirm::new("No staged changes", message, "Stage All and Commit")).await || !stage_all(git, cx).await {
            return false;
        }
    }
    let (core, root, message) = (git.core.clone(), git.root.clone(), request.message.clone());
    let options = CommitOptions { amend: request.amend, signoff: request.signoff, no_verify: request.no_verify, gpg_sign: request.sign, ..Default::default() };
    if run_shown(git, "Commit", cx, async move { backend::git_commit(&core, root, message, options).await }).await.is_none() {
        return false;
    }
    match request.then {
        Some(AfterCommit::Push) => {
            push(git, PushOptions::default(), cx).await;
        }
        Some(AfterCommit::Sync) => {
            sync(git, cx).await;
        }
        None => {}
    }
    true
}

pub async fn undo_last_commit(git: &Git, cx: &mut AsyncWindowContext) {
    let message = "The commit is removed from the branch and its changes return to the staging area. Nothing is lost.";
    if confirm(cx, Confirm::new("Undo last commit?", message, "Undo Commit")).await {
        let (core, root) = (git.core.clone(), git.root.clone());
        run_shown(git, "Undo commit", cx, async move { backend::git_undo_commit(&core, root).await }).await;
    }
}

// ----- Remote -----

pub async fn fetch(git: &Git, remote: Option<String>, cx: &mut AsyncWindowContext) -> bool {
    let result = run_progress(git, "Fetch", cx, |core, root, id, progress| async move { backend::git_fetch(&core, root, remote, id, progress).await }).await;
    match result {
        Ok(()) => true,
        Err(error) => {
            handle_error(git, &error, "Fetch", None, cx);
            false
        }
    }
}

async fn pull_with(git: &Git, strategy: PullStrategy, cx: &mut AsyncWindowContext) -> AppResult<()> {
    run_progress(git, "Pull", cx, |core, root, id, progress| async move { backend::git_pull(&core, root, strategy, id, progress).await }).await
}

pub async fn pull(git: &Git, cx: &mut AsyncWindowContext) -> bool {
    let strategy = git.settings().git.pull_strategy;
    let error = match pull_with(git, strategy, cx).await {
        Ok(()) => return true,
        Err(error) => error,
    };
    if error.code == "cancelled" {
        return false;
    }
    // ff-only failed because the history diverged: offer another way.
    if strategy == PullStrategy::FfOnly && logic::is_diverged(&error.message, error.detail.as_deref()) {
        let message = "Your branch and its upstream both have new commits, so a fast-forward is not possible. How do you want to combine them?".to_string();
        let choices = vec![Choice::new("cancel", "Cancel"), Choice::new("merge", "Merge"), Choice::new("rebase", "Rebase").primary()];
        let strategy = match choose(cx, "Branches have diverged", message, choices).await {
            Some("merge") => PullStrategy::Merge,
            Some("rebase") => PullStrategy::Rebase,
            _ => return false,
        };
        return match pull_with(git, strategy, cx).await {
            Ok(()) => true,
            Err(error) => {
                handle_error(git, &error, "Pull", None, cx);
                false
            }
        };
    }
    handle_error(git, &error, "Pull", None, cx);
    false
}

/// The list of remotes: from the store if already loaded, otherwise straight from the backend.
async fn remotes(git: &Git, cx: &mut AsyncWindowContext) -> Vec<Remote> {
    if let Some(refs) = refs(git, cx).await {
        return refs.remotes;
    }
    Vec::new()
}

async fn refs(git: &Git, cx: &mut AsyncWindowContext) -> Option<Refs> {
    if let Some(refs) = ui(cx, |_, cx| git.store.read(cx).refs().cloned()).flatten() {
        return Some(refs);
    }
    let (core, root) = (git.core.clone(), git.root.clone());
    core_call(cx, async move { backend::git_refs(&core, root).await }).await.ok()
}

pub async fn push(git: &Git, mut options: PushOptions, cx: &mut AsyncWindowContext) -> bool {
    let snapshot = snapshot(git, cx);
    if snapshot.as_ref().is_some_and(|s| matches!(s.head, Head::Detached { .. })) && options.branch.is_none() {
        info(cx, "detached", "You are not on a branch. Create a branch before pushing.");
        return false;
    }
    if snapshot.as_ref().is_some_and(|s| s.upstream.is_none()) && !options.set_upstream && options.branch.is_none() {
        options.set_upstream = true;
    }
    if options.set_upstream && options.remote.is_none() {
        let remotes = remotes(git, cx).await;
        if remotes.is_empty() {
            let add = {
                let git = git.clone();
                ToastAction::new("add-remote", "Add remote…", move |window, cx| {
                    let git = git.clone();
                    spawn(window, cx, async move |cx| add_remote(&git, cx).await);
                })
            };
            ui(cx, |window, cx| prompts::toast(NoticeLevel::Error, "no-remote", "This repository has no remote yet.", vec![add], window, cx));
            return false;
        }
        if remotes.len() > 1 {
            let items = remotes.iter().map(|r| PickItem::new(r.name.clone(), r.name.clone()).description(if r.push_url.is_empty() { r.fetch_url.clone() } else { r.push_url.clone() })).collect();
            let Some(picked) = pick(cx, QuickPickOptions::new("Publish to which remote?", items)).await else { return false };
            options.remote = Some(picked.value);
        }
    }
    if options.force_with_lease && git.settings().git.confirm_force_push {
        let target = snapshot.as_ref().map(|s| s.upstream.as_ref().map_or_else(|| logic::head_label(s), |u| u.name.clone())).unwrap_or_default();
        let message = format!(
            "This overwrites {target} on the remote with your local branch. Commits that exist only on the remote will be removed from that branch.\n\nThe push is refused if someone else pushed since your last fetch (--force-with-lease)."
        );
        if !confirm(cx, Confirm::new("Force push?", message, "Force Push").danger()).await {
            return false;
        }
    }
    let label = if options.set_upstream { "Publish" } else { "Push" };
    let result = run_progress(git, label, cx, |core, root, id, progress| async move { backend::git_push(&core, root, options, id, progress).await }).await;
    match result {
        Ok(()) => true,
        Err(error) => {
            handle_error(git, &error, label, None, cx);
            false
        }
    }
}

/// Pull, then push.
pub async fn sync(git: &Git, cx: &mut AsyncWindowContext) -> bool {
    let published = snapshot(git, cx).is_some_and(|s| s.upstream.is_some_and(|u| u.exists));
    if !published {
        return push(git, PushOptions { set_upstream: true, ..Default::default() }, cx).await;
    }
    if !pull(git, cx).await {
        return false;
    }
    // A push without new commits is harmless ("Everything up-to-date").
    push(git, PushOptions::default(), cx).await
}

pub async fn add_remote(git: &Git, cx: &mut AsyncWindowContext) {
    let Some(name) = input(cx, InputPrompt::new("Add remote", "Remote name").value("origin").validate(ref_name_validator)).await else { return };
    let Some(url) = input(cx, InputPrompt::new("Add remote", "Remote URL").placeholder("https://example.com/owner/repo.git")).await else { return };
    let (core, root, name, url) = (git.core.clone(), git.root.clone(), name.trim().to_string(), url.trim().to_string());
    run_shown(git, "Add remote", cx, async move { backend::git_remote_add(&core, root, name, url).await }).await;
}

/// Picks one of the remotes; returns it right away if there is only one.
async fn pick_remote(git: &Git, placeholder: &str, cx: &mut AsyncWindowContext) -> Option<Remote> {
    let remotes = remotes(git, cx).await;
    match remotes.len() {
        0 => {
            info(cx, "no-remote", "This repository has no remote yet.");
            None
        }
        1 => remotes.into_iter().next(),
        _ => {
            let items = remotes.iter().map(|r| PickItem::new(r.name.clone(), r.name.clone()).description(r.fetch_url.clone())).collect();
            let picked = pick(cx, QuickPickOptions::new(placeholder.to_string(), items)).await?;
            remotes.into_iter().find(|r| r.name == picked.value)
        }
    }
}

pub async fn remove_remote(git: &Git, name: Option<String>, cx: &mut AsyncWindowContext) {
    let name = match name {
        Some(name) => name,
        None => match pick_remote(git, "Remove which remote?", cx).await {
            Some(remote) => remote.name,
            None => return,
        },
    };
    let message = "Its remote-tracking branches are removed locally. Nothing changes on the server.";
    if confirm(cx, Confirm::new(format!("Remove remote {name}?"), message, "Remove").danger()).await {
        let (core, root) = (git.core.clone(), git.root.clone());
        run_shown(git, "Remove remote", cx, async move { backend::git_remote_remove(&core, root, name).await }).await;
    }
}

pub async fn rename_remote(git: &Git, cx: &mut AsyncWindowContext) {
    let Some(remote) = pick_remote(git, "Rename which remote?", cx).await else { return };
    let prompt = InputPrompt::new(format!("Rename remote {}", remote.name), "New name").value(remote.name.clone()).validate(ref_name_validator);
    let Some(new_name) = input(cx, prompt).await else { return };
    let new_name = new_name.trim().to_string();
    if new_name != remote.name {
        let (core, root) = (git.core.clone(), git.root.clone());
        run_shown(git, "Rename remote", cx, async move { backend::git_remote_rename(&core, root, remote.name, new_name).await }).await;
    }
}

pub async fn set_remote_url(git: &Git, cx: &mut AsyncWindowContext) {
    let Some(remote) = pick_remote(git, "Change the URL of which remote?", cx).await else { return };
    let prompt = InputPrompt::new(format!("Change URL of {}", remote.name), "Remote URL").value(remote.fetch_url.clone());
    let Some(url) = input(cx, prompt).await else { return };
    let url = url.trim().to_string();
    if url != remote.fetch_url {
        let (core, root) = (git.core.clone(), git.root.clone());
        run_shown(git, "Change remote URL", cx, async move { backend::git_remote_set_url(&core, root, remote.name, url).await }).await;
    }
}

// ----- Branch -----

pub async fn create_branch(git: &Git, start: Option<String>, cx: &mut AsyncWindowContext) {
    let title = match &start {
        Some(start) if start.len() >= 40 => format!("Create branch at {}", &start[..8]),
        Some(start) => format!("Create branch at {start}"),
        None => "Create branch".to_string(),
    };
    let prompt = InputPrompt::new(title, "Branch name").placeholder("feature/my-change").validate(ref_name_validator);
    let Some(name) = input(cx, prompt).await else { return };
    let name = name.trim().to_string();
    let operation = op(move |core, root| {
        let (name, start) = (name.clone(), start.clone());
        async move { backend::git_branch_create(&core, root, name, start, true).await }
    });
    run_op(git, "Create branch", operation, cx).await;
}

pub async fn checkout(git: &Git, target: CheckoutTarget, cx: &mut AsyncWindowContext) -> bool {
    let operation = match target {
        CheckoutTarget::Create => {
            create_branch(git, None, cx).await;
            return true;
        }
        CheckoutTarget::Branch(name) => op(move |core, root| {
            let name = name.clone();
            async move { backend::git_switch(&core, root, name).await }
        }),
        CheckoutTarget::Remote(name) => {
            // If a local branch with the same name already exists, just switch to it.
            let local = logic::local_name_of_remote(&name).to_string();
            let exists = refs(git, cx).await.is_some_and(|refs| refs.branches.iter().any(|b| b.name == local));
            op(move |core, root| {
                let (name, local) = (name.clone(), local.clone());
                async move {
                    if exists {
                        backend::git_switch(&core, root, local).await
                    } else {
                        backend::git_switch_tracking(&core, root, name).await
                    }
                }
            })
        }
        CheckoutTarget::Detached(name) => op(move |core, root| {
            let name = name.clone();
            async move { backend::git_switch_detached(&core, root, name).await }
        }),
    };
    run_op(git, "Checkout", operation, cx).await
}

/// The branch pick list for switching (from the status bar).
pub async fn pick_branch(git: &Git, cx: &mut AsyncWindowContext) {
    let Some(refs) = refs(git, cx).await else { return };
    let items = logic::branch_pick_items(&refs)
        .into_iter()
        .map(|(label, description, target)| {
            let item = PickItem::new(label, target.encode());
            if description.is_empty() {
                item
            } else {
                item.description(description)
            }
        })
        .collect();
    let Some(picked) = pick(cx, QuickPickOptions::new("Switch to branch, remote branch or tag", items)).await else { return };
    if let Some(target) = CheckoutTarget::decode(&picked.value) {
        checkout(git, target, cx).await;
    }
}

/// Picks a branch, remote branch, or tag other than the current branch.
async fn pick_ref(git: &Git, placeholder: &'static str, cx: &mut AsyncWindowContext) -> Option<String> {
    let refs = refs(git, cx).await?;
    let items = logic::ref_pick_items(&refs).into_iter().map(|(name, kind)| PickItem::new(name.clone(), name).description(kind)).collect();
    pick(cx, QuickPickOptions::new(placeholder, items)).await.map(|item| item.value)
}

// ----- Merge, rebase, continuing operations -----

pub async fn merge(git: &Git, rev: Option<String>, mode: MergeMode, cx: &mut AsyncWindowContext) {
    let rev = match rev {
        Some(rev) => rev,
        None => match pick_ref(git, "Merge which branch into the current branch?", cx).await {
            Some(rev) => rev,
            None => return,
        },
    };
    let current = snapshot(git, cx).map(|s| logic::head_label(&s)).unwrap_or_default();
    if !confirm(cx, Confirm::new(format!("Merge {rev} into {current}?"), "", "Merge")).await {
        return;
    }
    let operation = op(move |core, root| {
        let rev = rev.clone();
        async move { backend::git_merge(&core, root, rev, mode).await }
    });
    run_op(git, "Merge", operation, cx).await;
}

pub async fn rebase_onto(git: &Git, onto: Option<String>, cx: &mut AsyncWindowContext) {
    let onto = match onto {
        Some(onto) => onto,
        None => match pick_ref(git, "Rebase the current branch onto…", cx).await {
            Some(onto) => onto,
            None => return,
        },
    };
    let current = snapshot(git, cx).map(|s| logic::head_label(&s)).unwrap_or_default();
    let message = "Your commits are re-created on top of the target. If they were already pushed, you will need to force push afterwards.";
    if !confirm(cx, Confirm::new(format!("Rebase {current} onto {onto}?"), message, "Rebase")).await {
        return;
    }
    let operation = op(move |core, root| {
        let onto = onto.clone();
        async move { backend::git_rebase(&core, root, onto).await }
    });
    run_op(git, "Rebase", operation, cx).await;
}

/// Continue / Skip / Abort for an operation that is stopped.
pub async fn operation(git: &Git, action: OperationAction, cx: &mut AsyncWindowContext) {
    let snapshot = snapshot(git, cx);
    let pretty = logic::operation_name(snapshot.as_ref().and_then(|s| s.operation.as_ref()));
    if action == OperationAction::Abort {
        let message = format!("The repository returns to the state before the {pretty} started. Conflict resolutions made so far are discarded.");
        if !confirm(cx, Confirm::new(format!("Abort {pretty}?"), message, "Abort").danger()).await {
            return;
        }
    }
    let conflicts = snapshot.as_ref().map_or(0, |s| s.conflicted.len());
    if action == OperationAction::Continue && conflicts > 0 {
        let message = format!("Resolve the remaining {} before continuing.", plural(conflicts, "conflict"));
        ui(cx, |window, cx| prompts::toast(NoticeLevel::Warning, "conflicts-left", message, Vec::new(), window, cx));
        return;
    }
    let verb = match action {
        OperationAction::Continue => "Continue",
        OperationAction::Skip => "Skip",
        OperationAction::Abort => "Abort",
    };
    let (core, root) = (git.core.clone(), git.root.clone());
    run_shown(git, &format!("{verb} {pretty}"), cx, async move { backend::git_operation(&core, root, action).await }).await;
}

/// Good / Bad / Skip during a bisect: git checks out the next commit to test, or reports the first bad one.
pub async fn bisect_mark(git: &Git, mark: oncode_git::BisectMark, cx: &mut AsyncWindowContext) {
    let label = match mark {
        oncode_git::BisectMark::Good => "Mark good",
        oncode_git::BisectMark::Bad => "Mark bad",
        oncode_git::BisectMark::Skip => "Skip commit",
    };
    let (core, root) = (git.core.clone(), git.root.clone());
    if let Some(message) = run_shown(git, label, cx, async move { backend::git_bisect_mark(&core, root, mark, None).await }).await {
        let summary = bisect_summary(&message);
        ui(cx, |window, cx| prompts::toast(NoticeLevel::Info, "bisect-step", summary, Vec::new(), window, cx));
    }
}

/// Ends the bisect and returns to where it started.
pub async fn bisect_reset(git: &Git, cx: &mut AsyncWindowContext) {
    let (core, root) = (git.core.clone(), git.root.clone());
    run_shown(git, "Stop bisect", cx, async move { backend::git_bisect_reset(&core, root).await }).await;
}

/// The line of git's bisect output worth showing: the steps left, or which commit is the first bad one.
pub fn bisect_summary(message: &str) -> String {
    message.lines().map(str::trim).find(|line| line.starts_with("Bisecting:") || oncode_git::status::bisect_found(line)).or_else(|| message.lines().next()).unwrap_or("").to_string()
}

// ----- Tag -----

pub async fn create_tag(git: &Git, rev: Option<String>, cx: &mut AsyncWindowContext) {
    let Some(name) = input(cx, InputPrompt::new("Create tag", "Tag name").placeholder("v1.0.0").validate(ref_name_validator)).await else { return };
    let name = name.trim().to_string();
    let prompt = InputPrompt::new(format!("Create tag {name}"), "Message (leave empty for a lightweight tag)").allow_empty().ok("Create Tag");
    let Some(message) = input(cx, prompt).await else { return };
    let message = Some(message.trim().to_string()).filter(|m| !m.is_empty());
    let (core, root, tag) = (git.core.clone(), git.root.clone(), name.clone());
    if run_shown(git, "Create tag", cx, async move { backend::git_tag_create(&core, root, tag, rev, message).await }).await.is_none() {
        return;
    }
    let Some(remote) = remotes(git, cx).await.into_iter().next() else { return };
    let push = {
        let (git, remote, tag) = (git.clone(), remote.name.clone(), name.clone());
        ToastAction::new("push-tag", format!("Push to {}", remote), move |window, cx| {
            let (git, remote, tag) = (git.clone(), remote.clone(), tag.clone());
            spawn(window, cx, async move |cx| {
                let (core, root) = (git.core.clone(), git.root.clone());
                run_shown(&git, "Push tag", cx, async move { backend::git_tag_push(&core, root, remote, tag).await }).await;
            });
        })
    };
    ui(cx, |window, cx| prompts::toast(NoticeLevel::Info, "tag-created", format!("Tag {name} created."), vec![push], window, cx));
}

// ----- Stash -----

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StashRequest {
    pub paths: Vec<String>,
    /// `None` = ask if there are untracked files.
    pub include_untracked: Option<bool>,
    /// Only changes that are already staged (`git stash push --staged`).
    pub staged_only: bool,
}

pub async fn stash_push(git: &Git, request: StashRequest, cx: &mut AsyncWindowContext) {
    let Some(message) = input(cx, InputPrompt::new("Stash changes", "Message (optional)").allow_empty().ok("Stash")).await else { return };
    let mut include_untracked = request.include_untracked.unwrap_or(false);
    let untracked = snapshot(git, cx).map_or(0, |s| s.untracked.len());
    if request.include_untracked.is_none() && untracked > 0 && request.paths.is_empty() && !request.staged_only {
        let detail = format!("{} can be stashed too.", plural(untracked, "untracked file"));
        let choices = vec![Choice::new("cancel", "Cancel"), Choice::new("no", "Tracked Only"), Choice::new("yes", "Include Untracked").primary()];
        include_untracked = match choose(cx, "Include untracked files?", detail, choices).await {
            Some("yes") => true,
            Some("no") => false,
            _ => return,
        };
    }
    let options = StashOptions { message: Some(message.trim().to_string()).filter(|m| !m.is_empty()), include_untracked, staged_only: request.staged_only, paths: request.paths };
    let (core, root) = (git.core.clone(), git.root.clone());
    run_shown(git, "Stash", cx, async move { backend::git_stash_push(&core, root, options).await }).await;
}

pub async fn stash_apply(git: &Git, name: String, pop: bool, cx: &mut AsyncWindowContext) -> bool {
    let (core, root) = (git.core.clone(), git.root.clone());
    run_shown(git, if pop { "Pop stash" } else { "Apply stash" }, cx, async move { backend::git_stash_apply(&core, root, name, pop).await }).await.is_some()
}

/// Pops the latest stash, or says so if there is none.
pub async fn stash_pop_latest(git: &Git, cx: &mut AsyncWindowContext) {
    let (core, root) = (git.core.clone(), git.root.clone());
    let latest = core_call(cx, async move { backend::git_stash_list(&core, root).await }).await.ok().and_then(|stashes| stashes.into_iter().next());
    match latest {
        Some(stash) => {
            stash_apply(git, stash.name, true, cx).await;
        }
        None => info(cx, "no-stash", "There are no stashes."),
    }
}

pub async fn stash_drop(git: &Git, name: String, message: String, cx: &mut AsyncWindowContext) {
    let detail = format!("{message}\n\nThe stashed changes are deleted permanently.");
    if confirm(cx, Confirm::new(format!("Drop {name}?"), detail, "Drop Stash").danger()).await {
        let (core, root) = (git.core.clone(), git.root.clone());
        run_shown(git, "Drop stash", cx, async move { backend::git_stash_drop(&core, root, name).await }).await;
    }
}

pub async fn stash_branch(git: &Git, name: String, cx: &mut AsyncWindowContext) {
    let prompt = InputPrompt::new(format!("Create branch from {name}"), "Branch name").validate(ref_name_validator);
    let Some(branch) = input(cx, prompt).await else { return };
    let (core, root, branch) = (git.core.clone(), git.root.clone(), branch.trim().to_string());
    run_shown(git, "Stash to branch", cx, async move { backend::git_stash_branch(&core, root, name, branch).await }).await;
}

/// Shows the files of a stash; picking one opens its diff.
pub async fn show_stash(git: &Git, name: String, cx: &mut AsyncWindowContext) {
    let (core, root, stash) = (git.core.clone(), git.root.clone(), name.clone());
    let files = match core_call(cx, async move { backend::git_stash_files(&core, root, stash).await }).await {
        Ok(files) => files,
        Err(error) => {
            handle_error(git, &error, "Show stash", None, cx);
            return;
        }
    };
    let items = files
        .iter()
        .map(|file| {
            let (base, dir) = logic::split_path(&file.path);
            PickItem::new(base.to_string(), file.path.clone()).description(dir.to_string()).hint(logic::kind_letter(file.kind))
        })
        .collect();
    let options = QuickPickOptions::new(format!("Files changed in {name}"), items).empty_text("This stash has no changed files");
    let Some(picked) = pick(cx, options).await else { return };
    let orig_path = files.iter().find(|file| file.path == picked.value).and_then(|file| file.orig_path.clone());
    ui(cx, |window, cx| git.open_diff(&picked.value, orig_path, DiffSource::Stash { name }, false, window, cx));
}

// ----- Conflicts -----

pub async fn resolve_with_side(git: &Git, path: String, side: ConflictSide, cx: &mut AsyncWindowContext) {
    let (core, root) = (git.core.clone(), git.root.clone());
    run_shown(git, "Resolve conflict", cx, async move { backend::git_resolve_side(&core, root, path, side).await }).await;
}

pub async fn mark_resolved(git: &Git, path: String, cx: &mut AsyncWindowContext) {
    let (core, root) = (git.core.clone(), git.root.clone());
    run_shown(git, "Mark resolved", cx, async move { backend::git_mark_resolved(&core, root, path).await }).await;
}

#[cfg(test)]
mod tests {
    use super::valid_email;

    #[test]
    fn email_needs_a_user_and_a_host() {
        assert!(valid_email("ana@example.com"));
        assert!(valid_email("  ana@example.com  "));
        for bad in ["", "ana", "ana@", "@example.com", "ana budi@example.com"] {
            assert!(!valid_email(bad), "{bad:?}");
        }
    }
}
