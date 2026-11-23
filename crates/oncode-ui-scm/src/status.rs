//! Item status bar: branch aktif (membuka pemilih branch) dan sinkronisasi (publish, pull+push, progres).

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::{div, Context, Entity, IntoElement, Render, Styled as _, Subscription, Window};
use oncode_git::PushOptions;
use oncode_ui::Lucide;

use crate::actions::{self, Git};
use crate::logic::{self, SyncState};
use crate::store::ScmStore;

/// Nama branch (atau commit bila detached) beserta operasi yang sedang berlangsung.
pub struct BranchStatus {
    store: Entity<ScmStore>,
    _subscription: Subscription,
}

impl BranchStatus {
    pub fn new(store: Entity<ScmStore>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&store, |_, _, cx| cx.notify());
        Self { store, _subscription: subscription }
    }
}

impl Render for BranchStatus {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(snapshot) = self.store.read(cx).snapshot(cx) else { return div().into_any_element() };
        let store = self.store.clone();
        Button::new("status-branch")
            .ghost()
            .xsmall()
            .icon(Icon::new(if snapshot.operation.is_some() { Lucide::GitMerge } else { Lucide::GitBranch }))
            .label(logic::branch_status_label(&snapshot))
            .tooltip("Switch branch")
            .on_click(move |_, window, cx| {
                let Some(git) = Git::from_store(&store, cx) else { return };
                actions::spawn(window, cx, async move |cx| actions::pick_branch(&git, cx).await);
            })
            .into_any_element()
    }
}

/// Jumlah commit di belakang/di depan upstream; saat operasi berjalan menampilkan progresnya.
pub struct SyncStatus {
    store: Entity<ScmStore>,
    _subscription: Subscription,
}

impl SyncStatus {
    pub fn new(store: Entity<ScmStore>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&store, |_, _, cx| cx.notify());
        Self { store, _subscription: subscription }
    }
}

impl Render for SyncStatus {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.store.read(cx);
        let Some(snapshot) = state.snapshot(cx) else { return div().into_any_element() };
        let store = self.store.clone();
        if let Some(busy) = state.busy() {
            let text = match busy.percent {
                Some(percent) => format!("{} {percent}%", busy.label),
                None => busy.label.to_string(),
            };
            let cancellable = busy.operation_id.is_some();
            return Button::new("status-sync")
                .ghost()
                .xsmall()
                .loading(true)
                .label(text)
                .tooltip(if cancellable { "Click to cancel" } else { "Git operation in progress" })
                .on_click(move |_, _, cx| {
                    if let Some(git) = Git::from_store(&store, cx) {
                        actions::cancel_busy(&git, cx);
                    }
                })
                .max_w(gpui_kit::px(280.))
                .text_color(cx.theme().muted_foreground)
                .into_any_element();
        }
        match logic::sync_state(&snapshot, state.has_remote()) {
            SyncState::Hidden => div().into_any_element(),
            SyncState::Publish => Button::new("status-sync")
                .ghost()
                .xsmall()
                .icon(Icon::new(Lucide::CloudUpload))
                .tooltip("Publish branch")
                .on_click(move |_, window, cx| {
                    let Some(git) = Git::from_store(&store, cx) else { return };
                    actions::spawn(window, cx, async move |cx| {
                        actions::push(&git, PushOptions { set_upstream: true, ..Default::default() }, cx).await;
                    });
                })
                .into_any_element(),
            SyncState::Sync { upstream, ahead, behind } => Button::new("status-sync")
                .ghost()
                .xsmall()
                .icon(Icon::new(Lucide::RefreshCw))
                .label(format!("{behind}↓ {ahead}↑"))
                .tooltip(format!("Sync with {upstream}: pull {behind}, push {ahead}"))
                .on_click(move |_, window, cx| {
                    let Some(git) = Git::from_store(&store, cx) else { return };
                    actions::spawn(window, cx, async move |cx| {
                        actions::sync(&git, cx).await;
                    });
                })
                .into_any_element(),
        }
    }
}
