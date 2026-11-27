//! The Source Control panel: the repository header, the commit box, the operation banner, the list of changes, and stashes.

use std::ops::Range;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::component::button::{Button, ButtonVariants as _, DropdownButton};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Disableable as _, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, uniform_list, AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, InteractiveElement as _, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Task, TestSupportExt as _, UniformListScrollHandle, Window,
};
use oncode_core::api::ai::CommitSuggestion;
use oncode_core::{api, AppResult};
use oncode_git::{ChangeKind, CommitFile, ConflictEntry, ConflictSide, DiffSource, FileChange, Head, OperationAction, PushOptions, RepoSnapshot, Stash};
use oncode_ui::overlay;
use oncode_ui::{execute, spawn_core, Env, Lucide};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use crate::actions::{self, AfterCommit, CommitRequest, Git, StashRequest};
use crate::logic::{self, ChangeRow, SyncState};
use crate::rows::{build_rows, Row, Section, StashFiles, ViewState};
use crate::store::{ScmEvent, ScmStore};

const ROW_HEIGHT: f32 = 26.;

/// Contents of the "More actions" menu: (label, command id). `None` = separator.
const HEADER_MENU: &[Option<(&str, &str)>] = &[
    Some(("Pull", "git.pull")),
    Some(("Push", "git.push")),
    Some(("Fetch", "git.fetch")),
    Some(("Sync (Pull then Push)", "git.sync")),
    None,
    Some(("Switch Branch…", "git.switchBranch")),
    Some(("Create Branch…", "git.createBranch")),
    Some(("Create Tag…", "git.createTag")),
    None,
    Some(("Stash Changes…", "git.stash")),
    Some(("Undo Last Commit…", "git.undoLastCommit")),
    None,
    Some(("Show Graph", "git.showGraph")),
    Some(("Show Reflog", "git.showReflog")),
    Some(("Add Worktree…", "git.addWorktree")),
    Some(("Add Remote…", "git.addRemote")),
    None,
    Some(("Force Push…", "git.forcePush")),
];

pub struct ScmPanel {
    env: Env,
    store: Entity<ScmStore>,
    git: Option<Git>,
    message: Entity<TextareaState>,
    /// The branch name last set in the commit box placeholder.
    placeholder_for: String,
    amend: bool,
    signoff: bool,
    /// Sign commits (GPG or SSH, as configured in git).
    sign: bool,
    no_verify: bool,
    view: ViewState,
    rows: Vec<Row>,
    scroll: UniformListScrollHandle,
    /// Why Git is unavailable (not installed or too old).
    git_error: Option<String>,
    /// The "Generate Commit Message" that is running.
    generating: Option<Generation>,
    _subscriptions: Vec<Subscription>,
}

/// A commit message that Claude is writing.
struct Generation {
    /// The commit box content before it started; restored if it fails or is stopped.
    draft: String,
    _task: Task<()>,
    /// Dropped to cancel the HTTP request on the backend runtime.
    _cancel: oneshot::Sender<()>,
}

fn now_seconds() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

fn icon_button(id: impl Into<SharedString>, icon: Lucide, tooltip: impl Into<SharedString>) -> Button {
    Button::new(id.into()).ghost().xsmall().icon(Icon::new(icon)).tooltip(tooltip)
}

/// A menu item that runs `run` outside of any entity update.
fn menu_item(label: impl Into<SharedString>, run: impl Fn(&mut Window, &mut App) + 'static) -> PopupMenuItem {
    PopupMenuItem::new(label).on_click(move |_, window, cx| run(window, cx))
}

impl ScmPanel {
    pub fn new(env: Env, store: Entity<ScmStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let git = Git::from_store(&store, cx);
        let message = cx.new(|cx| TextareaState::new(window, cx).auto_grow(3, 10).placeholder("Message (Ctrl+Enter to commit)"));
        let subscriptions = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.subscribe_in(&store, window, |this: &mut Self, _, event: &ScmEvent, window, cx| match event {
                ScmEvent::FocusCommit => this.focus_commit(window, cx),
                ScmEvent::GenerateMessage => {
                    this.store.update(cx, |store, _| store.take_generate_request());
                    if this.generating.is_none() {
                        this.generate_message(window, cx);
                    }
                }
            }),
            cx.subscribe_in(&message, window, |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { secondary: true, .. } => this.submit(None, window, cx),
                // The subject length hint follows typing.
                InputEvent::Change => cx.notify(),
                _ => {}
            }),
        ];
        let core = env.core.clone();
        let info = spawn_core(cx, async move { api::app::app_info(&core).await });
        cx.spawn(async move |this, cx| {
            let error = info.await.ok().and_then(|info| if info.git.is_none() { Some(info.git_error.unwrap_or_else(|| "git was not found".into())) } else { None });
            let _ = this.update(cx, |this, cx| {
                this.git_error = error;
                cx.notify();
            });
        })
        .detach();
        let mut this = Self {
            env,
            store,
            git,
            message,
            placeholder_for: String::new(),
            amend: false,
            signoff: false,
            sign: false,
            no_verify: false,
            view: ViewState::default(),
            rows: Vec::new(),
            scroll: UniformListScrollHandle::new(),
            git_error: None,
            generating: None,
            _subscriptions: subscriptions,
        };
        // The `git.commit` command can arrive before this panel is built.
        if this.store.update(cx, |store, _| store.take_commit_focus()) {
            cx.defer_in(window, |this, window, cx| this.focus_commit(window, cx));
        }
        let weak = cx.weak_entity();
        this.store.update(cx, |store, _| store.set_panel(weak));
        if this.store.update(cx, |store, _| store.take_generate_request()) {
            cx.defer_in(window, |this, window, cx| this.generate_message(window, cx));
        }
        this.sync_placeholder(window, cx);
        this
    }

    /// The current commit message text.
    pub fn message(&self, cx: &App) -> String {
        self.message.read(cx).value().to_string()
    }

    fn focus_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.store.update(cx, |store, _| store.take_commit_focus());
        self.message.update(cx, |state, cx| state.focus(window, cx));
    }

    fn sync_placeholder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(branch) = self.store.read(cx).snapshot(cx).map(|s| logic::head_label(&s)) else { return };
        if branch != self.placeholder_for {
            let key = if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" };
            let text = format!("Message ({key}+Enter to commit on {branch})");
            self.message.update(cx, |state, cx| state.set_placeholder(text, window, cx));
            self.placeholder_for = branch;
        }
    }

    fn busy(&self, cx: &App) -> bool {
        self.store.read(cx).busy().is_some()
    }

    /// Runs an action flow on this panel's repository.
    fn run(&self, window: &mut Window, cx: &mut App, flow: impl AsyncFnOnce(Git, &mut gpui_kit::AsyncWindowContext) + 'static) {
        let Some(git) = self.git.clone() else { return };
        actions::spawn(window, cx, async move |cx| flow(git, cx).await);
    }

    fn submit(&mut self, then: Option<AfterCommit>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(git) = self.git.clone() else { return };
        if self.busy(cx) {
            return;
        }
        let request = CommitRequest { message: self.message(cx), amend: self.amend, signoff: self.signoff, no_verify: self.no_verify, sign: self.sign, then };
        cx.spawn_in(window, async move |this, cx| {
            if actions::commit(&git, request, cx).await {
                // The commit box is cleared after a successful commit.
                let _ = this.update_in(cx, |this, window, cx| {
                    this.message.update(cx, |state, cx| state.set_value("", window, cx));
                    this.amend = false;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// `true` while Claude is writing the commit message.
    pub fn is_generating(&self) -> bool {
        self.generating.is_some()
    }

    /// "Generate Commit Message": Claude writes a message from the staged changes, and the text
    /// streams into the commit box. What was already typed in the box becomes the start of the message. Calling it again
    /// while it runs means stop.
    pub fn generate_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.generating.is_some() {
            self.stop_generating(window, cx);
            return;
        }
        let Some(git) = self.git.clone() else { return };
        if self.store.read(cx).snapshot(cx).is_some_and(|snapshot| snapshot.staged.is_empty()) {
            overlay::toast_info("Stage the changes you want to commit first.", window, cx);
            return;
        }
        let draft = self.message(cx);
        let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
        let (text_tx, mut text_rx) = mpsc::unbounded_channel::<String>();
        let (core, root, prompt_draft) = (git.core.clone(), git.root.clone(), draft.clone());
        let request = spawn_core(cx, async move {
            let on_text: api::Callback<String> = Arc::new(move |piece| {
                let _ = text_tx.send(piece);
            });
            tokio::select! {
                result = api::ai::ai_commit_message(&core, root, prompt_draft, on_text) => Some(result),
                // The sender is dropped when stopped: the HTTP request is canceled too.
                _ = cancel_rx => None,
            }
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            // The channel closes when the request finishes or is canceled.
            let mut text = String::new();
            while let Some(first) = text_rx.recv().await {
                text.push_str(&first);
                while let Ok(more) = text_rx.try_recv() {
                    text.push_str(&more);
                }
                let shown = text.clone();
                if this.update_in(cx, |this, window, cx| this.message.update(cx, |state, cx| state.set_value(shown, window, cx))).is_err() {
                    return;
                }
            }
            let result = request.await;
            let _ = this.update_in(cx, |this, window, cx| this.finish_generating(result, window, cx));
        });
        self.generating = Some(Generation { draft, _task: task, _cancel: cancel_tx });
        cx.notify();
    }

    fn stop_generating(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(generation) = self.generating.take() {
            self.message.update(cx, |state, cx| state.set_value(generation.draft, window, cx));
            cx.notify();
        }
    }

    fn finish_generating(&mut self, result: Option<AppResult<CommitSuggestion>>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(generation) = self.generating.take() else { return };
        cx.notify();
        let error = match result {
            None => return,
            Some(Ok(suggestion)) => {
                self.message.update(cx, |state, cx| state.set_value(suggestion.message, window, cx));
                if suggestion.truncated {
                    overlay::toast_info("The staged diff is long, so only part of it was sent. Check the message before committing.", window, cx);
                }
                return;
            }
            Some(Err(error)) => error,
        };
        // Failed: the commit box goes back to how it was before it started.
        self.message.update(cx, |state, cx| state.set_value(generation.draft, window, cx));
        match error.code.as_str() {
            // No key yet, or the key was rejected: ask for the matching key, then try once more.
            "noApiKey" | "unauthorized" | "customUnauthorized" => {
                let kind = if error.code == "customUnauthorized" { crate::ai::KeyKind::Custom } else { crate::ai::KeyKind::Anthropic };
                let reason = (error.code != "noApiKey").then(|| error.message.clone());
                let core = self.env.core.clone();
                cx.spawn_in(window, async move |this, cx| {
                    if crate::ai::ask_api_key(core, kind, reason, cx).await {
                        let _ = this.update_in(cx, |this, window, cx| this.generate_message(window, cx));
                    }
                })
                .detach();
            }
            _ => overlay::show_error(&error, window, cx),
        }
    }

    fn set_amend(&mut self, amend: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.amend = amend;
        cx.notify();
        let Some(git) = self.git.clone() else { return };
        if !amend || !self.message(cx).trim().is_empty() {
            return;
        }
        // Amend with an empty box: fill it with the last commit message.
        let (core, root) = (git.core.clone(), git.root.clone());
        let task = spawn_core(cx, async move { api::git::git_last_message(&core, root).await });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(last) = task.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                if this.amend && this.message(cx).trim().is_empty() {
                    this.message.update(cx, |state, cx| state.set_value(last.trim_end().to_string(), window, cx));
                }
            });
        })
        .detach();
    }

    fn toggle_section(&mut self, section: Section, cx: &mut Context<Self>) {
        self.view.toggle(section);
        cx.notify();
    }

    /// Opens or closes the file list of a stash; its files are loaded the first time it is opened.
    fn toggle_stash(&mut self, stash: &Stash, cx: &mut Context<Self>) {
        if self.view.expanded.remove(&stash.oid).is_some() {
            cx.notify();
            return;
        }
        let Some(git) = self.git.clone() else { return };
        self.view.expanded.insert(stash.oid.clone(), StashFiles::Loading);
        cx.notify();
        let (oid, name) = (stash.oid.clone(), stash.name.clone());
        let (core, root) = (git.core.clone(), git.root.clone());
        let task = spawn_core(cx, async move { api::git::git_stash_files(&core, root, name).await });
        cx.spawn(async move |this, cx| {
            let files = match task.await {
                Ok(files) => StashFiles::Loaded(files),
                Err(error) => StashFiles::Failed(error.message),
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(slot) = this.view.expanded.get_mut(&oid) {
                    *slot = files;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    // ----- Rendering -----

    fn render_empty(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let body = v_flex().id("scm-empty").test_support().size_full().p_3().gap_2().text_sm().text_color(theme.muted_foreground);
        if let Some(error) = &self.git_error {
            return body.child(SharedString::from(format!("Git is unavailable: {error}."))).into_any_element();
        }
        if self.env.repo_root.is_some() {
            return body.child("Loading repository…").into_any_element();
        }
        body.child("This folder is not a Git repository.")
            .child(Button::new("scm-init").primary().small().label("Initialize Repository").on_click(|_, window, cx| execute("git.init", Value::Null, window, cx)))
            .child(Button::new("scm-clone").outline().small().label("Clone Repository…").on_click(|_, window, cx| execute("git.clone", Value::Null, window, cx)))
            .into_any_element()
    }

    fn render_header(&self, snapshot: &RepoSnapshot, sync: &SyncState, busy: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let ahead_behind = match sync {
            SyncState::Sync { ahead, behind, .. } if *ahead > 0 || *behind > 0 => Some(format!("{behind}↓ {ahead}↑")),
            _ => None,
        };
        h_flex()
            .w_full()
            .flex_none()
            .px_3()
            .gap_1()
            .child(Icon::new(if snapshot.operation.is_some() { Lucide::GitMerge } else { Lucide::GitBranch }).small())
            .child(div().id("scm-branch").test_support().flex_1().min_w_0().truncate().text_sm().child(SharedString::from(logic::branch_status_label(snapshot))))
            .when_some(ahead_behind, |el, text| el.child(div().flex_none().text_xs().text_color(theme.muted_foreground).child(SharedString::from(text))))
            .child(icon_button("scm-fetch", Lucide::RefreshCw, "Fetch").loading(busy).on_click(cx.listener(|this, _, window, cx| {
                this.run(window, cx, async |git, cx| {
                    actions::fetch(&git, None, cx).await;
                });
            })))
            .child(icon_button("scm-more", Lucide::Ellipsis, "More actions").dropdown_menu(|mut menu: PopupMenu, _, _| {
                for entry in HEADER_MENU {
                    menu = match *entry {
                        Some((label, command)) => menu.item(menu_item(label, move |window, cx| execute(command, Value::Null, window, cx))),
                        None => menu.separator(),
                    };
                }
                menu
            }))
    }

    fn render_commit_box(&self, snapshot: &RepoSnapshot, busy: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let subject_length = self.message(cx).lines().next().map_or(0, |line| line.chars().count());
        let disabled = busy || snapshot.operation.is_some();
        let (signoff, no_verify, sign) = (self.signoff, self.no_verify, self.sign);
        let this = cx.entity().downgrade();
        let menu = move |menu: PopupMenu, _: &mut Window, _: &mut Context<PopupMenu>| {
            let submit = |then: Option<AfterCommit>| {
                let this = this.clone();
                move |window: &mut Window, cx: &mut App| {
                    let _ = this.update(cx, |this, cx| this.submit(then, window, cx));
                }
            };
            let toggle = |change: fn(&mut ScmPanel)| {
                let this = this.clone();
                move |_: &mut Window, cx: &mut App| {
                    let _ = this.update(cx, |this, cx| {
                        change(this);
                        cx.notify();
                    });
                }
            };
            menu.item(menu_item("Commit", submit(None)))
                .item(menu_item("Commit & Push", submit(Some(AfterCommit::Push))))
                .item(menu_item("Commit & Sync", submit(Some(AfterCommit::Sync))))
                .separator()
                .item(menu_item("Add Signed-off-by", toggle(|this| this.signoff = !this.signoff)).checked(signoff))
                .item(menu_item("Skip Hooks (--no-verify)", toggle(|this| this.no_verify = !this.no_verify)).checked(no_verify))
                // The key and program come from git's configuration (`user.signingkey`, `gpg.format`); a passphrase
                // must be asked by a graphical pinentry or an agent, since git runs without a terminal here.
                .item(menu_item("Sign Commit (GPG/SSH)", toggle(|this| this.sign = !this.sign)).checked(sign))
        };
        v_flex()
            .w_full()
            .flex_none()
            .px_3()
            .gap_1p5()
            .child(div().id("scm-message").test_support().w_full().child(Textarea::new(&self.message).aria_label("Commit message")))
            .when(subject_length > 50, |el| {
                el.child(
                    div()
                        .id("scm-subject-hint")
                        .test_support()
                        .text_xs()
                        .text_color(if subject_length > 72 { theme.danger } else { theme.muted_foreground })
                        .child(SharedString::from(format!("Subject is {subject_length} characters (aim for 50, keep under 72)"))),
                )
            })
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        div().flex_1().min_w_0().child(
                            DropdownButton::new("scm-commit")
                                .primary()
                                .small()
                                .menu_accessibility_label("More commit actions")
                                .button(
                                    Button::new("scm-commit-button")
                                        .icon(Icon::new(Lucide::Check))
                                        .label(logic::commit_label(self.amend, snapshot.staged.len()))
                                        .disabled(disabled)
                                        .on_click(cx.listener(|this, _, window, cx| this.submit(None, window, cx))),
                                )
                                .dropdown_menu(menu),
                        ),
                    )
                    .child({
                        let generating = self.generating.is_some();
                        let (icon, tooltip) =
                            if generating { (Lucide::CircleStop, "Stop generating") } else { (Lucide::Sparkles, "Generate commit message with AI") };
                        icon_button("scm-ai-generate", icon, tooltip)
                            .disabled(!generating && (disabled || snapshot.staged.is_empty()))
                            .on_click(cx.listener(|this, _, window, cx| this.generate_message(window, cx)))
                    })
                    .child(
                        Checkbox::new("scm-amend")
                            .label("Amend")
                            .tooltip("Replace the last commit with the staged changes and this message")
                            .checked(self.amend)
                            .disabled(matches!(snapshot.head, Head::Unborn { .. }))
                            .on_click(cx.listener(|this, checked: &bool, window, cx| this.set_amend(*checked, window, cx))),
                    ),
            )
    }

    fn render_busy(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let busy = self.store.read(cx).busy()?.clone();
        let theme = cx.theme();
        Some(
            v_flex()
                .id("scm-busy")
                .test_support()
                .w_full()
                .flex_none()
                .px_3()
                .gap_1()
                .child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(div().flex_1().min_w_0().truncate().child(SharedString::from(busy.text())))
                        .when(busy.operation_id.is_some(), |el| {
                            el.child(Button::new("scm-cancel").outline().xsmall().label("Cancel").on_click(cx.listener(|this, _, _, cx| {
                                if let Some(git) = &this.git {
                                    actions::cancel_busy(git, cx);
                                }
                            })))
                        }),
                )
                .when_some(busy.percent, |el, percent| el.child(Progress::new("scm-progress").value(f32::from(percent))))
                .into_any_element(),
        )
    }

    fn render_operation(&self, snapshot: &RepoSnapshot, busy: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let text = logic::operation_text(snapshot.operation.as_ref()?);
        // A bisect has Good / Bad / Skip / Stop instead of Continue / Skip / Abort (unless a conflict stopped it).
        let (bisect, found) = match snapshot.operation.as_ref() {
            Some(oncode_git::Operation::Bisect { first_bad, .. }) => (true, first_bad.is_some()),
            _ => (false, false),
        };
        let theme = cx.theme();
        let conflicts = snapshot.conflicted.len();
        let action = |id: &'static str, label: &'static str, action: OperationAction, cx: &Context<Self>| {
            Button::new(id).small().label(label).disabled(busy).on_click(cx.listener(move |this, _, window, cx| {
                this.run(window, cx, async move |git, cx| actions::operation(&git, action, cx).await);
            }))
        };
        Some(
            v_flex()
                .id("scm-operation")
                .test_support()
                .flex_none()
                .mx_3()
                .p_2()
                .gap_1()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.warning)
                .text_sm()
                .child(div().font_weight(gpui_kit::FontWeight::SEMIBOLD).child(SharedString::from(text.title)))
                .child(SharedString::from(text.detail))
                .when(!bisect || conflicts > 0, |el| el.child(div().text_xs().text_color(theme.muted_foreground).child(SharedString::from(logic::conflicts_left(conflicts)))))
                .map(|el| {
                    if bisect && conflicts == 0 {
                        let mark = |id: &'static str, label: &'static str, mark: oncode_git::BisectMark, cx: &Context<Self>| {
                            Button::new(id).small().label(label).disabled(busy || found).on_click(cx.listener(move |this, _, window, cx| {
                                this.run(window, cx, async move |git, cx| actions::bisect_mark(&git, mark, cx).await);
                            }))
                        };
                        el.child(
                            h_flex()
                                .gap_1p5()
                                .pt_1()
                                .child(mark("scm-bisect-good", "Good", oncode_git::BisectMark::Good, cx).outline())
                                .child(mark("scm-bisect-bad", "Bad", oncode_git::BisectMark::Bad, cx).outline())
                                .child(mark("scm-bisect-skip", "Skip", oncode_git::BisectMark::Skip, cx).outline())
                                .child(Button::new("scm-bisect-reset").small().label(if found { "Finish" } else { "Stop" }).when(found, |b| b.primary()).when(!found, |b| b.outline()).disabled(busy).on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.run(window, cx, async move |git, cx| actions::bisect_reset(&git, cx).await);
                                    },
                                ))),
                        )
                    } else {
                        el.child(
                            h_flex()
                                .gap_1p5()
                                .pt_1()
                                .child(action("scm-op-continue", "Continue", OperationAction::Continue, cx).primary().disabled(busy || conflicts > 0))
                                .when(text.can_skip, |el| el.child(action("scm-op-skip", "Skip", OperationAction::Skip, cx).outline()))
                                .child(action("scm-op-abort", "Abort", OperationAction::Abort, cx).outline()),
                        )
                    }
                })
                .into_any_element(),
        )
    }

    fn render_sync(&self, sync: &SyncState, busy: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let button = match sync {
            SyncState::Publish => Button::new("scm-publish").outline().small().w_full().icon(Icon::new(Lucide::CloudUpload)).label("Publish Branch").disabled(busy).on_click(cx.listener(
                |this, _, window, cx| {
                    this.run(window, cx, async |git, cx| {
                        actions::push(&git, PushOptions { set_upstream: true, ..Default::default() }, cx).await;
                    });
                },
            )),
            SyncState::Sync { upstream, ahead, behind } if *ahead > 0 || *behind > 0 => {
                let mut label = "Sync Changes".to_string();
                if *behind > 0 {
                    label.push_str(&format!(" {behind}↓"));
                }
                if *ahead > 0 {
                    label.push_str(&format!(" {ahead}↑"));
                }
                Button::new("scm-sync")
                    .outline()
                    .small()
                    .w_full()
                    .icon(Icon::new(Lucide::RefreshCw))
                    .label(label)
                    .tooltip(format!("Pull {behind} and push {ahead} commit(s) with {upstream}"))
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.run(window, cx, async |git, cx| {
                            actions::sync(&git, cx).await;
                        });
                    }))
            }
            _ => return None,
        };
        Some(div().w_full().flex_none().px_3().child(button).into_any_element())
    }

    fn render_rows(&mut self, range: Range<usize>, _: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let rows: Vec<Row> = range.filter_map(|index| self.rows.get(index).cloned()).collect();
        rows.into_iter().map(|row| self.render_row(row, cx)).collect()
    }

    fn render_row(&self, row: Row, cx: &mut Context<Self>) -> AnyElement {
        match row {
            Row::Header { section, count, open } => self.render_section(section, count, open, cx),
            Row::Conflict(entry) => self.render_conflict(entry, cx),
            Row::Staged(change) => self.render_staged(change, cx),
            Row::Change(change) => self.render_change(change, cx),
            Row::Clean => self.render_note("No changes. The working tree is clean.".into(), 22., cx),
            Row::Stash { stash, expanded } => self.render_stash(stash, expanded, cx),
            Row::StashFile { stash, file } => self.render_stash_file(stash, file, cx),
            Row::StashNote(text) => self.render_note(text.into(), 42., cx),
        }
    }

    fn render_note(&self, text: SharedString, indent: f32, cx: &mut Context<Self>) -> AnyElement {
        h_flex().h(px(ROW_HEIGHT)).w_full().pl(px(indent)).pr_2().text_sm().text_color(cx.theme().muted_foreground).child(div().truncate().child(text)).into_any_element()
    }

    fn render_section(&self, section: Section, count: usize, open: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let mut actions = h_flex().flex_none().gap_0p5();
        match section {
            Section::Staged => {
                actions = actions.child(icon_button("scm-unstage-all", Lucide::Minus, "Unstage all").on_click(cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.run(window, cx, async |git, cx| {
                        actions::unstage_all(&git, cx).await;
                    });
                })));
            }
            Section::Changes if count > 0 => {
                actions = actions
                    .child(icon_button("scm-stash-all", Lucide::Archive, "Stash all changes…").on_click(cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.run(window, cx, async |git, cx| actions::stash_push(&git, StashRequest::default(), cx).await);
                    })))
                    .child(icon_button("scm-discard-all", Lucide::Undo2, "Discard all changes…").on_click(cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.run(window, cx, async |git, cx| {
                            actions::discard_all(&git, cx).await;
                        });
                    })))
                    .child(icon_button("scm-stage-all", Lucide::Plus, "Stage all").on_click(cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.run(window, cx, async |git, cx| {
                            actions::stage_all(&git, cx).await;
                        });
                    })));
            }
            _ => {}
        }
        h_flex()
            .id(SharedString::from(format!("scm-section-{}", section.slug())))
            .test_support()
            .h(px(ROW_HEIGHT))
            .w_full()
            .px_2()
            .gap_1()
            .text_xs()
            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
            .hover(|style| style.bg(theme.list_hover))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_section(section, cx)))
            .child(Icon::new(if open { Lucide::ChevronDown } else { Lucide::ChevronRight }).small())
            .child(div().flex_1().min_w_0().truncate().child(section.title().to_uppercase()))
            .child(actions)
            .child(div().flex_none().px_1p5().rounded_full().bg(theme.muted).text_color(theme.muted_foreground).child(count.to_string()))
            .into_any_element()
    }

    /// The skeleton of a file row: the name, a dimmed folder, action buttons (shown on hover), and the status letter.
    #[allow(clippy::too_many_arguments)]
    fn file_row(
        &self,
        id: String,
        path: &str,
        tooltip: String,
        letter: &'static str,
        letter_color: gpui_kit::Hsla,
        deleted: bool,
        indent: f32,
        actions: Vec<Button>,
        cx: &mut Context<Self>,
    ) -> impl StatefulInteractiveElement + ParentElement + Styled + IntoElement + 'static {
        let theme = cx.theme();
        let (name, dir) = logic::split_path(path);
        h_flex()
            .id(SharedString::from(id))
            .test_support()
            .group("scm-row")
            .h(px(ROW_HEIGHT))
            .w_full()
            .pl(px(indent))
            .pr_2()
            .gap_1p5()
            .text_sm()
            .hover(|style| style.bg(theme.list_hover))
            .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1p5()
                    .child(oncode_ui::file_icons::file_icon(path).when(deleted, |el| el.opacity(0.5)))
                    .child(div().flex_none().max_w_full().truncate().when(deleted, |el| el.line_through()).child(SharedString::from(name.to_string())))
                    .when(!dir.is_empty(), |el| el.child(div().flex_1().min_w_0().truncate().text_xs().text_color(theme.muted_foreground).child(SharedString::from(dir.to_string())))),
            )
            .child(h_flex().flex_none().gap_0p5().opacity(0.).group_hover("scm-row", |style| style.opacity(1.)).children(actions))
            .child(div().flex_none().w(px(14.)).text_center().text_xs().font_weight(gpui_kit::FontWeight::SEMIBOLD).text_color(letter_color).child(letter))
    }

    /// The same context menu items for all file rows.
    fn common_menu(menu: PopupMenu, git: &Git, path: &str, deleted: bool) -> PopupMenu {
        let (open_git, open_path) = (git.clone(), path.to_string());
        let (history_root, history_path) = (git.root.clone(), path.to_string());
        let copy_path = path.to_string();
        menu.separator()
            .item(menu_item("Open File", move |window, cx| open_git.open_file(&open_path, window, cx)).disabled(deleted))
            .item(menu_item("File History", move |window, cx| execute("git.fileHistory", json!([history_root, history_path]), window, cx)))
            .item(menu_item("Copy Path", move |_, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy_path.clone()))))
    }

    fn render_conflict(&self, entry: ConflictEntry, cx: &mut Context<Self>) -> AnyElement {
        let Some(git) = self.git.clone() else { return div().into_any_element() };
        let path = entry.path.clone();
        let resolve = {
            let path = path.clone();
            icon_button(format!("scm-resolve-{path}"), Lucide::Plus, "Mark as resolved (stage current file content)").on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                let path = path.clone();
                this.run(window, cx, async move |git, cx| actions::mark_resolved(&git, path, cx).await);
            }))
        };
        let tooltip = format!("{path} • {}", logic::conflict_title(entry.kind));
        let open = {
            let (git, path) = (git.clone(), path.clone());
            move |window: &mut Window, cx: &mut App| git.open_conflict(&path, window, cx)
        };
        let click = open.clone();
        self.file_row(format!("scm-conflict-{path}"), &path, tooltip, "!", cx.theme().danger, false, 22., vec![resolve], cx)
            .on_click(move |_, window, cx| click(window, cx))
            .context_menu(move |menu, _, _| {
                // `None` = mark as resolved with the current file content.
                let resolve = |side: Option<ConflictSide>| {
                    let (git, path) = (git.clone(), path.clone());
                    move |window: &mut Window, cx: &mut App| {
                        let (git, path) = (git.clone(), path.clone());
                        actions::spawn(window, cx, async move |cx| match side {
                            Some(side) => actions::resolve_with_side(&git, path, side, cx).await,
                            None => actions::mark_resolved(&git, path, cx).await,
                        });
                    }
                };
                let menu = menu
                    .item(menu_item("Open Conflict Editor", open.clone()))
                    .item(menu_item("Accept Current (ours)", resolve(Some(ConflictSide::Current))))
                    .item(menu_item("Accept Incoming (theirs)", resolve(Some(ConflictSide::Incoming))))
                    .item(menu_item("Mark as Resolved", resolve(None)));
                Self::common_menu(menu, &git, &path, false)
            })
            .into_any_element()
    }

    fn render_staged(&self, change: FileChange, cx: &mut Context<Self>) -> AnyElement {
        let Some(git) = self.git.clone() else { return div().into_any_element() };
        let path = change.path.clone();
        let deleted = change.kind == ChangeKind::Deleted;
        // A rename is unstaged together with its old path.
        let paths: Vec<String> = std::iter::once(path.clone()).chain(change.orig_path.clone()).collect();
        let unstage = {
            let paths = paths.clone();
            icon_button(format!("scm-unstage-{path}"), Lucide::Minus, "Unstage").on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                let paths = paths.clone();
                this.run(window, cx, async move |git, cx| {
                    actions::unstage(&git, paths, cx).await;
                });
            }))
        };
        let tooltip = format!("{}{path} • {} (staged)", change.orig_path.as_ref().map(|orig| format!("{orig} → ")).unwrap_or_default(), logic::kind_title(change.kind));
        let open = {
            let (git, path, orig) = (git.clone(), path.clone(), change.orig_path.clone());
            move |preview: bool, window: &mut Window, cx: &mut App| git.open_diff(&path, orig.clone(), DiffSource::Staged, preview, window, cx)
        };
        let click = open.clone();
        self.file_row(format!("scm-staged-{path}"), &path, tooltip, logic::kind_letter(change.kind), kind_color(change.kind, false, cx), deleted, 22., vec![unstage], cx)
            .on_click(move |_, window, cx| click(true, window, cx))
            .context_menu(move |menu, _, _| {
                let open = open.clone();
                let (unstage_git, unstage_paths) = (git.clone(), paths.clone());
                let menu = menu.item(menu_item("Open Changes", move |window, cx| open(false, window, cx))).item(menu_item("Unstage", move |window, cx| {
                    let (git, paths) = (unstage_git.clone(), unstage_paths.clone());
                    actions::spawn(window, cx, async move |cx| {
                        actions::unstage(&git, paths, cx).await;
                    });
                }));
                Self::common_menu(menu, &git, &path, deleted)
            })
            .into_any_element()
    }

    fn render_change(&self, change: ChangeRow, cx: &mut Context<Self>) -> AnyElement {
        let Some(git) = self.git.clone() else { return div().into_any_element() };
        let path = change.path.clone();
        let untracked = change.untracked;
        let deleted = change.kind == ChangeKind::Deleted;
        let discard_label = if untracked { "Delete (move to trash)…" } else { "Discard changes…" };
        // Untracked files are moved to the trash; tracked ones are restored to the index content.
        let discard_flow = move |git: Git, path: String, window: &mut Window, cx: &mut App| {
            actions::spawn(window, cx, async move |cx| {
                let (tracked, untracked) = if untracked { (Vec::new(), vec![path]) } else { (vec![path], Vec::new()) };
                actions::discard(&git, tracked, untracked, cx).await;
            });
        };
        let stage_flow = |git: Git, path: String, window: &mut Window, cx: &mut App| {
            actions::spawn(window, cx, async move |cx| {
                actions::stage(&git, vec![path], cx).await;
            });
        };
        let mut buttons = Vec::new();
        if !deleted {
            let (git, path) = (git.clone(), path.clone());
            buttons.push(icon_button(format!("scm-open-{path}"), Lucide::FileText, "Open file").on_click(move |_, window, cx| {
                cx.stop_propagation();
                git.open_file(&path, window, cx);
            }));
        }
        buttons.push({
            let (git, path) = (git.clone(), path.clone());
            icon_button(format!("scm-discard-{path}"), Lucide::Undo2, discard_label).on_click(move |_, window, cx| {
                cx.stop_propagation();
                discard_flow(git.clone(), path.clone(), window, cx);
            })
        });
        buttons.push({
            let (git, path) = (git.clone(), path.clone());
            icon_button(format!("scm-stage-{path}"), Lucide::Plus, "Stage").on_click(move |_, window, cx| {
                cx.stop_propagation();
                stage_flow(git.clone(), path.clone(), window, cx);
            })
        });
        let title = if untracked { "Untracked" } else { logic::kind_title(change.kind) };
        let tooltip = format!("{}{path} • {title}", change.orig_path.as_ref().map(|orig| format!("{orig} → ")).unwrap_or_default());
        let letter = if untracked { "U" } else { logic::kind_letter(change.kind) };
        let open = {
            let (git, path, orig) = (git.clone(), path.clone(), change.orig_path.clone());
            move |preview: bool, window: &mut Window, cx: &mut App| git.open_diff(&path, orig.clone(), DiffSource::Worktree, preview, window, cx)
        };
        let click = open.clone();
        self.file_row(format!("scm-change-{path}"), &path, tooltip, letter, kind_color(change.kind, untracked, cx), deleted, 22., buttons, cx)
            .on_click(move |_, window, cx| click(true, window, cx))
            .context_menu(move |menu, _, _| {
                let open = open.clone();
                let (stage_git, stage_path) = (git.clone(), path.clone());
                let (discard_git, discard_path) = (git.clone(), path.clone());
                let (stash_git, stash_path) = (git.clone(), path.clone());
                let menu = menu
                    .item(menu_item("Open Changes", move |window, cx| open(false, window, cx)))
                    .item(menu_item("Stage", move |window, cx| stage_flow(stage_git.clone(), stage_path.clone(), window, cx)))
                    .item(menu_item(if untracked { "Delete (Move to Trash)…" } else { "Discard Changes…" }, move |window, cx| {
                        discard_flow(discard_git.clone(), discard_path.clone(), window, cx)
                    }))
                    .item(
                        menu_item("Stash This File…", move |window, cx| {
                            let (git, path) = (stash_git.clone(), stash_path.clone());
                            actions::spawn(window, cx, async move |cx| {
                                actions::stash_push(&git, StashRequest { paths: vec![path], include_untracked: Some(false), staged_only: false }, cx).await;
                            });
                        })
                        .disabled(untracked),
                    );
                Self::common_menu(menu, &git, &path, deleted)
            })
            .into_any_element()
    }

    fn render_stash(&self, stash: Stash, expanded: bool, cx: &mut Context<Self>) -> AnyElement {
        let Some(git) = self.git.clone() else { return div().into_any_element() };
        let theme = cx.theme();
        let name = stash.name.clone();
        let pop_flow = |git: Git, name: String, pop: bool, window: &mut Window, cx: &mut App| {
            actions::spawn(window, cx, async move |cx| {
                actions::stash_apply(&git, name, pop, cx).await;
            });
        };
        let drop_flow = |git: Git, name: String, message: String, window: &mut Window, cx: &mut App| {
            actions::spawn(window, cx, async move |cx| actions::stash_drop(&git, name, message, cx).await);
        };
        let pop = {
            let (git, name) = (git.clone(), name.clone());
            icon_button(format!("scm-stash-pop-{name}"), Lucide::ArrowUp, "Pop (apply and drop)").on_click(move |_, window, cx| {
                cx.stop_propagation();
                pop_flow(git.clone(), name.clone(), true, window, cx);
            })
        };
        let drop = {
            let (git, name, message) = (git.clone(), name.clone(), stash.message.clone());
            icon_button(format!("scm-stash-drop-{name}"), Lucide::X, "Drop…").on_click(move |_, window, cx| {
                cx.stop_propagation();
                drop_flow(git.clone(), name.clone(), message.clone(), window, cx);
            })
        };
        let tooltip = format!("{name}: {}", stash.message);
        let toggle = stash.clone();
        let menu_stash = stash.clone();
        let this = cx.entity().downgrade();
        h_flex()
            .id(SharedString::from(format!("scm-stash-{name}")))
            .test_support()
            .group("scm-row")
            .h(px(ROW_HEIGHT))
            .w_full()
            .pl(px(22.))
            .pr_2()
            .gap_1p5()
            .text_sm()
            .hover(|style| style.bg(theme.list_hover))
            .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_stash(&toggle, cx)))
            .child(Icon::new(if expanded { Lucide::ChevronDown } else { Lucide::Archive }).small().text_color(theme.muted_foreground))
            .child(div().flex_1().min_w_0().truncate().child(SharedString::from(stash.message.clone())))
            .child(h_flex().flex_none().gap_0p5().opacity(0.).group_hover("scm-row", |style| style.opacity(1.)).child(pop).child(drop))
            .child(div().flex_none().text_xs().text_color(theme.muted_foreground).child(SharedString::from(logic::relative_time(stash.time, now_seconds()))))
            .context_menu(move |menu, _, _| {
                let stash = menu_stash.clone();
                let show = {
                    let (this, stash) = (this.clone(), stash.clone());
                    move |_: &mut Window, cx: &mut App| {
                        let _ = this.update(cx, |this, cx| {
                            if !this.view.expanded.contains_key(&stash.oid) {
                                this.toggle_stash(&stash, cx);
                            }
                        });
                    }
                };
                let apply = |pop: bool| {
                    let (git, name) = (git.clone(), stash.name.clone());
                    move |window: &mut Window, cx: &mut App| pop_flow(git.clone(), name.clone(), pop, window, cx)
                };
                let (branch_git, branch_name) = (git.clone(), stash.name.clone());
                let (drop_git, drop_name, drop_message) = (git.clone(), stash.name.clone(), stash.message.clone());
                menu.item(menu_item("Show Changes", show))
                    .item(menu_item("Apply", apply(false)))
                    .item(menu_item("Pop (apply and drop)", apply(true)))
                    .item(menu_item("Create Branch from Stash…", move |window, cx| {
                        let (git, name) = (branch_git.clone(), branch_name.clone());
                        actions::spawn(window, cx, async move |cx| actions::stash_branch(&git, name, cx).await);
                    }))
                    .separator()
                    .item(menu_item("Drop…", move |window, cx| drop_flow(drop_git.clone(), drop_name.clone(), drop_message.clone(), window, cx)))
            })
            .into_any_element()
    }

    fn render_stash_file(&self, stash: String, file: CommitFile, cx: &mut Context<Self>) -> AnyElement {
        let Some(git) = self.git.clone() else { return div().into_any_element() };
        let path = file.path.clone();
        let tooltip = format!("{}{path} • {}", file.orig_path.as_ref().map(|orig| format!("{orig} → ")).unwrap_or_default(), logic::kind_title(file.kind));
        let id = format!("scm-stash-file-{stash}-{path}");
        self.file_row(id, &path, tooltip, logic::kind_letter(file.kind), kind_color(file.kind, false, cx), file.kind == ChangeKind::Deleted, 42., Vec::new(), cx)
            .on_click(move |_, window, cx| git.open_diff(&path, file.orig_path.clone(), DiffSource::Stash { name: stash.clone() }, true, window, cx))
            .into_any_element()
    }
}

/// The status letter color, like the Git markers in the explorer.
fn kind_color(kind: ChangeKind, untracked: bool, cx: &App) -> gpui_kit::Hsla {
    let theme = cx.theme();
    if untracked {
        return theme.success;
    }
    match kind {
        ChangeKind::Added => theme.success,
        ChangeKind::Deleted => theme.danger,
        ChangeKind::Renamed | ChangeKind::Copied => theme.info,
        ChangeKind::Modified | ChangeKind::TypeChanged => theme.warning,
    }
}

impl Render for ScmPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(snapshot) = self.store.read(cx).snapshot(cx) else {
            return self.render_empty(cx);
        };
        self.sync_placeholder(window, cx);
        let (stashes, has_remote, busy) = {
            let store = self.store.read(cx);
            (store.stashes().to_vec(), store.has_remote(), store.busy().is_some())
        };
        self.view.retain_stashes(&stashes);
        self.rows = build_rows(&snapshot, &stashes, &self.view);
        let sync = logic::sync_state(&snapshot, has_remote);

        v_flex()
            .id("scm-panel")
            .size_full()
            .gap_2()
            .child(self.render_header(&snapshot, &sync, busy, cx))
            .child(self.render_commit_box(&snapshot, busy, cx))
            .children(self.render_busy(cx))
            .children(self.render_operation(&snapshot, busy, cx))
            .children(self.render_sync(&sync, busy, cx))
            .child(div().flex_1().min_h_0().w_full().child(uniform_list("scm-rows", self.rows.len(), cx.processor(Self::render_rows)).track_scroll(&self.scroll).size_full()))
            .into_any_element()
    }
}
