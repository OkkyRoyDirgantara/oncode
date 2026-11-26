//! Source Control state per window: refs, the stash list, and the busy indicator. The repository
//! snapshot itself lives in `oncode_ui::repos`; this store only adds what is not there.

use std::time::{Duration, Instant};

use gpui_kit::{App, Context, Entity, EventEmitter, SharedString, Subscription, Task, WeakEntity, Window};
use oncode_core::api;
use oncode_git::{Head, Refs, RepoSnapshot, Stash};
use oncode_ui::{repos, spawn_core, Env, ReposEvent, Workbench};

/// The Git operation currently running for this window's repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Busy {
    pub label: SharedString,
    /// 0–100, or `None` if unknown.
    pub percent: Option<u8>,
    /// For canceling through `api::app::app_cancel`.
    pub operation_id: Option<String>,
}

impl Busy {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self { label: label.into(), percent: None, operation_id: None }
    }

    /// Text for the panel and status bar, e.g. "Fetch: Receiving objects 40%".
    pub fn text(&self) -> String {
        match self.percent {
            Some(percent) => format!("{} {percent}%", self.label),
            None => format!("{}…", self.label),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScmEvent {
    /// The `git.commit` command: move focus to the commit message box.
    FocusCommit,
    /// The `git.generateCommitMessage` command: write the commit message with Claude.
    GenerateMessage,
}

pub struct ScmStore {
    env: Env,
    root: Option<String>,
    refs: Option<Refs>,
    stashes: Vec<Stash>,
    busy: Option<Busy>,
    /// HEAD, HEAD oid, and stash count from the last snapshot; a change here reloads the refs.
    seen: Option<(Head, Option<String>, u32)>,
    reloading: bool,
    reload_again: bool,
    /// Focus on the commit box was requested before its panel was built.
    pending_focus: bool,
    pending_generate: bool,
    /// This window's Source Control panel, if already built.
    panel: Option<WeakEntity<crate::view::ScmPanel>>,
    _auto_fetch: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ScmEvent> for ScmStore {}

/// Interval of the auto-fetch check; the fetch itself follows `git.autoFetchMinutes`.
const AUTO_FETCH_TICK: Duration = Duration::from_secs(30);

/// Auto-fetch conditions: enabled, window active, no operation running, a remote exists, and the interval has passed.
pub fn should_auto_fetch(minutes: u32, since_last: Duration, window_active: bool, busy: bool, has_remote: bool, operation_in_progress: bool) -> bool {
    minutes > 0 && window_active && !busy && has_remote && !operation_in_progress && since_last >= Duration::from_secs(u64::from(minutes) * 60)
}

impl ScmStore {
    pub fn new(env: Env, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let root = env.repo_root.clone();
        let subscription = cx.subscribe(&repos(cx), |this: &mut Self, _, event: &ReposEvent, cx| match event {
            ReposEvent::Changed(root) if this.root.as_ref() == Some(root) => this.on_snapshot(cx),
            ReposEvent::RefsChanged(root) if this.root.as_ref() == Some(root) => this.reload_refs(cx),
            _ => {}
        });
        let auto_fetch = cx.spawn_in(window, async move |this, cx| {
            let mut last = Instant::now();
            loop {
                cx.background_executor().timer(AUTO_FETCH_TICK).await;
                let Ok(fetch) = this.update_in(cx, |this, window, cx| {
                    let Some(root) = this.root.clone() else { return None };
                    let minutes = this.env.core.effective_settings(&this.env.window).git.auto_fetch_minutes;
                    let operation = this.snapshot(cx).is_none_or(|s| s.operation.is_some());
                    if !should_auto_fetch(minutes, last.elapsed(), window.is_window_active(), this.busy.is_some(), this.has_remote(), operation) {
                        return None;
                    }
                    last = Instant::now();
                    let core = this.env.core.clone();
                    let id = format!("auto-fetch-{}", core.next_id());
                    // Errors are ignored so they do not get in the way.
                    Some(spawn_core(cx, async move { api::git::git_fetch(&core, root, None, id, std::sync::Arc::new(|_| {})).await }))
                }) else {
                    return;
                };
                if let Some(fetch) = fetch {
                    let _ = fetch.await;
                }
            }
        });
        let mut this = Self {
            env,
            root,
            refs: None,
            stashes: Vec::new(),
            busy: None,
            seen: None,
            reloading: false,
            reload_again: false,
            pending_focus: false,
            pending_generate: false,
            panel: None,
            _auto_fetch: auto_fetch,
            _subscriptions: vec![subscription],
        };
        // A snapshot may already exist (a second window for the same repository).
        if this.snapshot(cx).is_some() {
            this.on_snapshot(cx);
        }
        this
    }

    pub fn env(&self) -> &Env {
        &self.env
    }

    /// The root of this window's repository.
    pub fn root(&self) -> Option<&str> {
        self.root.as_deref()
    }

    pub fn snapshot(&self, cx: &App) -> Option<RepoSnapshot> {
        let root = self.root.as_ref()?;
        repos(cx).read(cx).snapshot(root).cloned()
    }

    pub fn refs(&self) -> Option<&Refs> {
        self.refs.as_ref()
    }

    pub fn stashes(&self) -> &[Stash] {
        &self.stashes
    }

    pub fn busy(&self) -> Option<&Busy> {
        self.busy.as_ref()
    }

    pub fn has_remote(&self) -> bool {
        self.refs.as_ref().is_some_and(|refs| !refs.remotes.is_empty())
    }

    pub fn set_busy(&mut self, busy: Option<Busy>, cx: &mut Context<Self>) {
        if self.busy != busy {
            self.busy = busy;
            cx.notify();
        }
    }

    /// Updates the progress of the running operation, if it is still the same operation.
    pub fn set_progress(&mut self, operation_id: &str, label: SharedString, percent: Option<u8>, cx: &mut Context<Self>) {
        if let Some(busy) = &mut self.busy {
            if busy.operation_id.as_deref() == Some(operation_id) {
                busy.label = label;
                busy.percent = percent;
                cx.notify();
            }
        }
    }

    pub fn request_commit_focus(&mut self, cx: &mut Context<Self>) {
        self.pending_focus = true;
        cx.emit(ScmEvent::FocusCommit);
    }

    /// Takes a focus request that has not been served yet.
    pub fn take_commit_focus(&mut self) -> bool {
        std::mem::take(&mut self.pending_focus)
    }

    pub fn request_generate(&mut self, cx: &mut Context<Self>) {
        self.pending_generate = true;
        cx.emit(ScmEvent::GenerateMessage);
    }

    /// Takes a "Generate Commit Message" request that has not been served yet (the panel is not built yet).
    pub fn take_generate_request(&mut self) -> bool {
        std::mem::take(&mut self.pending_generate)
    }

    pub(crate) fn set_panel(&mut self, panel: WeakEntity<crate::view::ScmPanel>) {
        self.panel = Some(panel);
    }

    /// This window's Source Control panel, if built and still alive.
    pub fn panel(&self) -> Option<Entity<crate::view::ScmPanel>> {
        self.panel.as_ref()?.upgrade()
    }

    fn on_snapshot(&mut self, cx: &mut Context<Self>) {
        let Some(snapshot) = self.snapshot(cx) else { return };
        // The stash count and HEAD also determine the list that is shown.
        let seen = (snapshot.head.clone(), snapshot.head_oid.clone(), snapshot.stash_count);
        if self.seen.as_ref() != Some(&seen) {
            self.seen = Some(seen);
            self.reload_refs(cx);
        }
        cx.notify();
    }

    /// Reloads refs and stashes. Requests that arrive while loading are merged into one next load.
    pub fn reload_refs(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else { return };
        if self.reloading {
            self.reload_again = true;
            return;
        }
        self.reloading = true;
        let core = self.env.core.clone();
        let task = spawn_core(cx, async move {
            let refs = api::git::git_refs(&core, root.clone()).await.ok();
            let stashes = api::git::git_stash_list(&core, root).await.ok();
            (refs, stashes)
        });
        cx.spawn(async move |this, cx| {
            let (refs, stashes) = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(refs) = refs {
                    this.refs = Some(refs);
                }
                if let Some(stashes) = stashes {
                    this.stashes = stashes;
                }
                this.reloading = false;
                if std::mem::take(&mut this.reload_again) {
                    this.reload_refs(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// A window's Source Control store; `None` if the folder is not trusted.
pub fn store(workbench: &Workbench) -> Option<Entity<ScmStore>> {
    workbench.extension::<Entity<ScmStore>>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_fetch_needs_every_condition() {
        let five = Duration::from_secs(300);
        assert!(should_auto_fetch(5, five, true, false, true, false));
        assert!(!should_auto_fetch(0, five, true, false, true, false), "0 minutes = never");
        assert!(!should_auto_fetch(5, Duration::from_secs(299), true, false, true, false), "interval has not passed");
        assert!(!should_auto_fetch(5, five, false, false, true, false), "window is not active");
        assert!(!should_auto_fetch(5, five, true, true, true, false), "another operation is running");
        assert!(!should_auto_fetch(5, five, true, false, false, false), "no remote");
        assert!(!should_auto_fetch(5, five, true, false, true, true), "a merge/rebase is stopped");
    }

    #[test]
    fn busy_text_shows_percent_or_ellipsis() {
        assert_eq!(Busy::new("Commit").text(), "Commit…");
        assert_eq!(Busy { label: "Fetch: Receiving objects".into(), percent: Some(40), operation_id: Some("op".into()) }.text(), "Fetch: Receiving objects 40%");
    }
}
