//! The Pull Requests sidebar, the PR/MR detail tab, the create-PR dialog, and cloning from an account.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, TestSupportExt as _, Window,
};
use oncode_core::api::forge::{self as forge_api, ForgeContext};
use oncode_core::api::git as git_api;
use oncode_core::{CoreEvent, Shared};
use oncode_forge::{Check, NewPullRequest, PrFilter, PullRequest, PullRequestDetail};
use oncode_git::Head;
use oncode_ui::overlay::{self, TextPrompt};
use oncode_ui::quick_pick::{PickItem, QuickPickOptions};
use oncode_ui::{core_bus, repos, spawn_core, Env, Lucide, OpenOptions, Shell, Workbench};

use crate::accounts::{core_call, pick, sign_in};
use crate::clone::{repo_name_from_url, validate_clone_url, validate_folder_name};
use crate::logic::{check_label, create_problem, effective_target, pr_tab, push_need, source_branch_name, summarize_checks, PrBadge, PrTabParams, FILTERS};
use crate::util::{describe_forge_error, forge_name, open_external, pr_noun, pr_noun_plural, pr_number, relative_time};

pub const SIDEBAR_ID: &str = "pullRequests";

enum ListState {
    Loading,
    /// The folder is not a repository or has no recognized remote.
    NoRemote,
    SignedOut(ForgeContext),
    Ready { context: ForgeContext, items: Vec<PullRequest>, next_page: Option<u32> },
    Failed(String),
}

/// Sidebar list of PRs/MRs of this window's repository.
pub struct PullRequestsView {
    env: Env,
    filter: PrFilter,
    state: ListState,
    revision: u64,
    _load: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl PullRequestsView {
    pub fn new(env: Env, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&core_bus(cx), |this: &mut Self, _, event: &CoreEvent, cx| {
            if matches!(event, CoreEvent::AccountsChanged) {
                this.reload(cx);
            }
        });
        let mut this = Self { env, filter: PrFilter::Open, state: ListState::Loading, revision: 0, _load: Task::ready(()), _subscriptions: vec![subscription] };
        this.reload(cx);
        this
    }

    pub fn items(&self) -> &[PullRequest] {
        match &self.state {
            ListState::Ready { items, .. } => items,
            _ => &[],
        }
    }

    pub fn context(&self) -> Option<&ForgeContext> {
        match &self.state {
            ListState::SignedOut(context) | ListState::Ready { context, .. } => Some(context),
            _ => None,
        }
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.env.repo_root.clone() else {
            self.state = ListState::NoRemote;
            return;
        };
        self.revision += 1;
        let revision = self.revision;
        self.state = ListState::Loading;
        let (core, filter) = (self.env.core.clone(), self.filter);
        let task = spawn_core(cx, async move {
            let Some(context) = forge_api::forge_context(&core, root).await? else { return Ok(None) };
            let Some(account) = context.account.clone() else { return Ok(Some((context, None))) };
            let page = forge_api::forge_prs(&core, account.id, context.repo.clone(), filter, 1).await?;
            Ok::<_, oncode_core::AppError>(Some((context, Some(page))))
        });
        self._load = cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.revision != revision {
                    return;
                }
                this.state = match result {
                    Ok(None) => ListState::NoRemote,
                    Ok(Some((context, None))) => ListState::SignedOut(context),
                    Ok(Some((context, Some(page)))) => ListState::Ready { context, items: page.items, next_page: page.next_page },
                    Err(error) => ListState::Failed(describe_forge_error(&error, None).line()),
                };
                cx.notify();
            });
        });
        cx.notify();
    }

    fn load_more(&mut self, cx: &mut Context<Self>) {
        let ListState::Ready { context, next_page: Some(page), .. } = &self.state else { return };
        let Some(account) = context.account.clone() else { return };
        let (core, repo, filter, page, revision) = (self.env.core.clone(), context.repo.clone(), self.filter, *page, self.revision);
        let task = spawn_core(cx, async move { forge_api::forge_prs(&core, account.id, repo, filter, page).await });
        self._load = cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.revision != revision {
                    return;
                }
                if let (ListState::Ready { items, next_page, .. }, Ok(page)) = (&mut this.state, result) {
                    items.extend(page.items);
                    *next_page = page.next_page;
                }
                cx.notify();
            });
        });
    }

    fn open(&self, pr: &PullRequest, window: &mut Window, cx: &mut App) {
        let (Some(context), Some(root)) = (self.context(), self.env.repo_root.clone()) else { return };
        let Some(tab) = pr_tab(&root, context, pr.number, &pr.title) else { return };
        let _ = self.env.workbench.update(cx, |workbench, cx| {
            workbench.open_tab(tab, OpenOptions::preview(), window, cx);
        });
    }
}

impl Render for PullRequestsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let body = v_flex().id("forge-prs").size_full().gap_1().p_1().text_sm().overflow_y_scroll();
        match &self.state {
            ListState::Loading => body.child(div().p_2().child(Spinner::new())).into_any_element(),
            ListState::NoRemote => body.child(div().p_2().text_color(theme.muted_foreground).child("This folder has no GitHub or GitLab remote.")).into_any_element(),
            ListState::Failed(message) => body
                .child(div().p_2().text_color(theme.danger).child(message.clone()))
                .child(Button::new("forge-retry").outline().small().label("Try Again").on_click(cx.listener(|this, _, _, cx| this.reload(cx))))
                .into_any_element(),
            ListState::SignedOut(context) => body
                .child(div().p_2().text_color(theme.muted_foreground).child(format!(
                    "Sign in to {} to see {} for {}/{}.",
                    forge_name(context.kind),
                    pr_noun_plural(context.kind).to_lowercase(),
                    context.repo.owner,
                    context.repo.name
                )))
                .child(div().px_2().child(Button::new("forge-sidebar-signin").primary().small().icon(Icon::new(Lucide::LogIn)).label("Sign In…").on_click(|_, window, cx| sign_in(window, cx))))
                .into_any_element(),
            ListState::Ready { context, items, next_page } => {
                let kind = context.kind;
                let selected = FILTERS.iter().position(|(filter, _, _)| *filter == self.filter).unwrap_or(0);
                let now = crate::util::now_seconds();
                body.child(
                    TabBar::new("forge-filters")
                        .underline()
                        .small()
                        .selected_index(selected)
                        .children(FILTERS.iter().map(|(_, _, label)| Tab::new().label(*label)))
                        .on_click(cx.listener(|this, index: &usize, _, cx| {
                            this.filter = FILTERS[*index].0;
                            this.reload(cx);
                        })),
                )
                .when(items.is_empty(), |el| el.child(div().p_2().text_color(theme.muted_foreground).child(format!("No {}.", pr_noun_plural(kind).to_lowercase()))))
                .children(items.iter().map(|pr| {
                    let badge = PrBadge::of(pr);
                    let pr_owned = pr.clone();
                    v_flex()
                        .id(SharedString::from(format!("forge-pr-{}", pr.number)))
                        .test_support()
                        .px_2()
                        .py_1()
                        .rounded(theme.radius)
                        .hover(|s| s.bg(theme.list_hover))
                        .on_click(cx.listener(move |this, _, window, cx| this.open(&pr_owned, window, cx)))
                        .child(h_flex().gap_1().child(Icon::new(Lucide::GitPullRequest).xsmall()).child(div().flex_1().min_w_0().truncate().child(pr.title.clone())))
                        .child(div().text_xs().text_color(theme.muted_foreground).child(format!(
                            "{} · {} · {} · {}",
                            pr_number(kind, pr.number),
                            badge.label(),
                            pr.author,
                            relative_time(pr.updated_at, now)
                        )))
                }))
                .when(next_page.is_some(), |el| el.child(Button::new("forge-more").ghost().small().label("Load More").on_click(cx.listener(|this, _, _, cx| this.load_more(cx)))))
                .into_any_element()
            }
        }
    }
}

// ----- Detail tab -----

enum DetailState {
    Loading,
    Ready { detail: Box<PullRequestDetail>, checks: Vec<Check> },
    Failed(String),
}

pub struct PullRequestTab {
    env: Env,
    params: PrTabParams,
    state: DetailState,
    checking_out: bool,
    focus_handle: FocusHandle,
    _load: Task<()>,
}

impl Focusable for PullRequestTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl PullRequestTab {
    pub fn new(env: Env, params: PrTabParams, cx: &mut Context<Self>) -> Self {
        let mut this = Self { env, params, state: DetailState::Loading, checking_out: false, focus_handle: cx.focus_handle(), _load: Task::ready(()) };
        this.reload(cx);
        this
    }

    pub fn detail(&self) -> Option<&PullRequestDetail> {
        match &self.state {
            DetailState::Ready { detail, .. } => Some(detail),
            _ => None,
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.state = DetailState::Loading;
        let (core, params) = (self.env.core.clone(), self.params.clone());
        let task = spawn_core(cx, async move {
            let detail = forge_api::forge_pr(&core, params.account_id.clone(), params.repo.clone(), params.number).await?;
            let checks = forge_api::forge_checks(&core, params.account_id, params.repo, detail.pull_request.head_sha.clone()).await.unwrap_or_default();
            Ok::<_, oncode_core::AppError>((detail, checks))
        });
        self._load = cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.state = match result {
                    Ok((detail, checks)) => DetailState::Ready { detail: Box::new(detail), checks },
                    Err(error) => DetailState::Failed(describe_forge_error(&error, Some(&this.params.repo.host)).line()),
                };
                cx.notify();
            });
        });
        cx.notify();
    }

    fn checkout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.checking_out {
            return;
        }
        self.checking_out = true;
        cx.notify();
        let (core, params) = (self.env.core.clone(), self.params.clone());
        let operation = format!("pr-checkout-{}", core.next_id());
        let task = spawn_core(cx, async move { forge_api::forge_pr_checkout(&core, params.root, params.kind, params.remote, params.number, operation, Arc::new(|_| {})).await });
        let noun = pr_noun(Some(self.params.kind));
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.checking_out = false;
                match result {
                    Ok(branch) => overlay::toast_success(format!("Switched to {branch}."), window, cx),
                    Err(error) => overlay::toast_error(format!("Could not check out the {noun}: {}", error.message), window, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for PullRequestTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let kind = Some(self.params.kind);
        let body = v_flex().id("forge-pr-tab").size_full().p_4().gap_3().overflow_y_scroll().track_focus(&self.focus_handle).bg(theme.background).text_sm();
        match &self.state {
            DetailState::Loading => body.child(Spinner::new()).into_any_element(),
            DetailState::Failed(message) => body
                .child(div().text_color(theme.danger).child(message.clone()))
                .child(Button::new("forge-pr-retry").outline().small().label("Try Again").on_click(cx.listener(|this, _, _, cx| this.reload(cx))))
                .into_any_element(),
            DetailState::Ready { detail, checks } => {
                let pr = &detail.pull_request;
                let web_url = pr.web_url.clone();
                body.child(div().text_xl().font_weight(gpui_kit::FontWeight::SEMIBOLD).child(format!("{} {}", pr.title, pr_number(kind, pr.number))))
                    .child(div().text_color(theme.muted_foreground).child(format!(
                        "{} · {} wants to merge {} into {} · {} commits, {} files",
                        PrBadge::of(pr).label(),
                        pr.author,
                        pr.source_branch,
                        pr.target_branch,
                        detail.commits,
                        detail.changed_files
                    )))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Button::new("forge-pr-checkout").primary().small().icon(Icon::new(Lucide::GitBranch)).label("Checkout").loading(self.checking_out).on_click(cx.listener(|this, _, window, cx| this.checkout(window, cx))))
                            .child(Button::new("forge-pr-browser").outline().small().icon(Icon::new(Lucide::ExternalLink)).label("Open in Browser").on_click(move |_, _, cx| {
                                open_external(&web_url, cx);
                            }))
                            .child(Button::new("forge-pr-refresh").ghost().small().icon(Icon::new(Lucide::RefreshCw)).tooltip("Refresh").on_click(cx.listener(|this, _, _, cx| this.reload(cx)))),
                    )
                    .when(!detail.labels.is_empty(), |el| el.child(div().text_xs().text_color(theme.muted_foreground).child(format!("Labels: {}", detail.labels.join(", ")))))
                    .when(!detail.reviewers.is_empty(), |el| el.child(div().text_xs().text_color(theme.muted_foreground).child(format!("Reviewers: {}", detail.reviewers.join(", ")))))
                    .child(
                        div()
                            .id("forge-pr-body")
                            .p_3()
                            .rounded(theme.radius)
                            .border_1()
                            .border_color(theme.border)
                            .child(if detail.body.trim().is_empty() { TextView::markdown("forge-pr-markdown", "*No description provided.*").into_any_element() } else { TextView::markdown("forge-pr-markdown", detail.body.clone()).into_any_element() }),
                    )
                    .child(div().font_weight(gpui_kit::FontWeight::SEMIBOLD).child(format!("Checks — {}", summarize_checks(checks))))
                    .children(checks.iter().map(|check| {
                        let url = check.url.clone();
                        h_flex()
                            .id(SharedString::from(format!("forge-check-{}", check.name)))
                            .gap_2()
                            .child(div().w(px(70.)).text_xs().text_color(theme.muted_foreground).child(check_label(check.state)))
                            .child(div().flex_1().child(check.name.clone()))
                            .when_some(url, |el, url| el.cursor_pointer().on_click(move |_, _, cx| {
                                open_external(&url, cx);
                            }))
                    }))
                    .into_any_element()
            }
        }
    }
}

// ----- Creating a PR -----

/// The "Create Pull Request" dialog for the current branch.
pub struct CreatePullRequest {
    env: Env,
    context: ForgeContext,
    title: Entity<InputState>,
    body: Entity<TextareaState>,
    target: Entity<InputState>,
    draft: bool,
    default_branch: Option<String>,
    busy: bool,
    problem: Option<&'static str>,
}

impl CreatePullRequest {
    fn source(&self, cx: &App) -> Option<(String, crate::logic::PushNeed)> {
        let root = self.env.repo_root.as_ref()?;
        let snapshot = repos(cx).read(cx).snapshot(root)?.clone();
        let Head::Branch { name } = &snapshot.head else { return None };
        Some((source_branch_name(name, snapshot.upstream.as_ref(), None), push_need(snapshot.upstream.as_ref())))
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((source, need)) = self.source(cx) else { return };
        let title = self.title.read(cx).value().to_string();
        let chosen = self.target.read(cx).value().trim().to_string();
        let target = effective_target(Some(chosen.as_str()).filter(|c| !c.is_empty()), self.default_branch.as_deref(), &source);
        self.problem = create_problem(&title, &target, &source, self.default_branch.is_some(), !chosen.is_empty());
        if self.problem.is_some() || self.busy {
            cx.notify();
            return;
        }
        let Some(account) = self.context.account.clone() else { return };
        self.busy = true;
        cx.notify();
        let input = NewPullRequest { title: title.trim().to_string(), body: self.body.read(cx).value().to_string(), source_branch: source, target_branch: target, draft: self.draft };
        let (core, root, remote, repo) = (self.env.core.clone(), self.env.repo_root.clone().unwrap_or_default(), self.context.remote.clone(), self.context.repo.clone());
        let publish = need != crate::logic::PushNeed::None;
        let task = spawn_core(cx, async move {
            if publish {
                let options = oncode_git::PushOptions { remote: Some(remote), set_upstream: true, ..Default::default() };
                let operation = format!("pr-push-{}", core.next_id());
                git_api::git_push(&core, root, options, operation, Arc::new(|_| {})).await?;
            }
            forge_api::forge_pr_create(&core, account.id, repo, input).await
        });
        let noun = pr_noun(self.context.kind);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(pr) => {
                        window.close_dialog(cx);
                        overlay::toast_success(format!("Created {noun} {}.", pr_number(this.context.kind, pr.number)), window, cx);
                        if let (Some(tab), Some(root)) = (pr_tab(this.env.repo_root.as_deref().unwrap_or_default(), &this.context, pr.number, &pr.title), this.env.repo_root.clone()) {
                            let _ = root;
                            let _ = this.env.workbench.update(cx, |workbench, cx| workbench.open_tab(tab, OpenOptions::default(), window, cx));
                        }
                    }
                    Err(error) => overlay::toast_error(describe_forge_error(&error, Some(&this.context.repo.host)).line(), window, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for CreatePullRequest {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let noun = pr_noun(self.context.kind);
        let (source, need) = self.source(cx).unwrap_or_else(|| ("(no branch)".into(), crate::logic::PushNeed::None));
        v_flex()
            .id("forge-create")
            .gap_2()
            .text_sm()
            .child(div().text_color(theme.muted_foreground).child(format!("From {source} into:")))
            .child(Input::new(&self.target).id("forge-create-target").small())
            .child(Input::new(&self.title).id("forge-create-title"))
            .child(Textarea::new(&self.body))
            .child(Checkbox::new("forge-create-draft").label("Create as draft").checked(self.draft).on_change(cx.listener(|this, value: &bool, _, cx| {
                this.draft = *value;
                cx.notify();
            })))
            .when_some(need.notice(&self.context.remote), |el, notice| el.child(div().text_xs().text_color(theme.muted_foreground).child(notice)))
            .when_some(self.problem, |el, problem| el.child(div().id("forge-create-problem").text_color(theme.danger).child(problem)))
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(Button::new("forge-create-cancel").outline().small().label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(Button::new("forge-create-submit").primary().small().loading(self.busy).label(need.submit_label(noun)).on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))),
            )
    }
}

/// Opens the "Create Pull Request" dialog for this window's repository.
pub fn create_pull_request(window: &mut Window, cx: &mut App) {
    let Some(workbench) = Workbench::for_window(window, cx) else { return };
    let env = workbench.read(cx).env().clone();
    let Some(root) = env.repo_root.clone() else { return };
    let core = env.core.clone();
    window
        .spawn(cx, async move |cx| {
            let context_core = core.clone();
            let context = match core_call(cx, async move { forge_api::forge_context(&context_core, root).await }).await {
                Ok(Some(context)) => context,
                Ok(None) => {
                    let _ = cx.update(|window, cx| overlay::toast_info("This repository has no GitHub or GitLab remote.", window, cx));
                    return;
                }
                Err(error) => {
                    let _ = cx.update(|window, cx| overlay::show_error(&error, window, cx));
                    return;
                }
            };
            let Some(account) = context.account.clone() else {
                let _ = cx.update(|window, cx| {
                    overlay::toast_info(format!("Sign in to {} first.", forge_name(context.kind)), window, cx);
                    sign_in(window, cx);
                });
                return;
            };
            let (default_core, repo) = (core.clone(), context.repo.clone());
            let default_branch = core_call(cx, async move { forge_api::forge_default_branch(&default_core, account.id, repo).await }).await.ok();
            let _ = cx.update(|window, cx| {
                let subject = Workbench::for_window(window, cx).and_then(|_| None::<String>);
                let title = cx.new(|cx| InputState::new(window, cx).placeholder("Title").default_value(subject.unwrap_or_default()));
                let body = cx.new(|cx| TextareaState::new(window, cx).auto_grow(5, 12).placeholder("Description (Markdown)"));
                let target = cx.new(|cx| InputState::new(window, cx).placeholder(default_branch.clone().map_or("Target branch".to_string(), |b| format!("Target branch (default: {b})"))));
                let noun = pr_noun(context.kind);
                let dialog = cx.new(|_| CreatePullRequest { env, context, title: title.clone(), body, target, draft: false, default_branch, busy: false, problem: None });
                window.open_dialog(cx, move |d, _, _| d.title(format!("Create {noun}")).w(px(560.)).child(dialog.clone()));
                window.defer(cx, move |window, cx| title.update(cx, |state, cx| state.focus(window, cx)));
            });
        })
        .detach();
}

// ----- Clone -----

/// Clone: from a URL or from the repository list of a signed-in account; then offers to open it.
pub fn clone_repository(core: Shared, window: &mut Window, cx: &mut App) {
    window
        .spawn(cx, async move |cx| {
            let list_core = core.clone();
            let accounts = core_call(cx, async move { forge_api::forge_accounts(&list_core).await }).await.unwrap_or_default();
            let mut items = vec![PickItem::new("Clone from URL…", "url")];
            items.extend(accounts.iter().map(|a| PickItem::new(format!("Clone from {} ({})", forge_name(Some(a.kind)), a.username), format!("account:{}", a.id))));
            let source = if accounts.is_empty() { Some(PickItem::new("", "url")) } else { pick(cx, QuickPickOptions::new("Clone a repository", items)).await };
            let Some(source) = source else { return };
            let url = match source.value.strip_prefix("account:") {
                Some(account_id) => {
                    let (search_core, account) = (core.clone(), account_id.to_string());
                    let options = QuickPickOptions::with_search("Search your repositories", move |query, cx| {
                        let (core, account) = (search_core.clone(), account.clone());
                        spawn_core(cx, async move {
                            let page = forge_api::forge_repos(&core, account, query, 1).await.map(|p| p.items).unwrap_or_default();
                            page.into_iter().map(|repo| PickItem::new(repo.full_name.clone(), repo.clone_url_https.clone()).description(repo.description)).collect()
                        })
                    })
                    .empty_text("No repositories found");
                    match pick(cx, options).await {
                        Some(item) => item.value,
                        None => return,
                    }
                }
                None => {
                    let Ok(answer) = cx.update(|window, cx| overlay::prompt_text(TextPrompt::new("Clone Repository", "Next").placeholder("https://github.com/owner/repo.git"), window, cx)) else { return };
                    match answer.await.map(|u| u.trim().to_string()).filter(|u| !u.is_empty()) {
                        Some(url) => url,
                        None => return,
                    }
                }
            };
            if let Some(problem) = validate_clone_url(&url) {
                let _ = cx.update(|window, cx| overlay::toast_error(problem, window, cx));
                return;
            }
            let Ok(paths) = cx.update(|_, cx| cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Clone Into".into()) })) else { return };
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(parent) = paths.into_iter().next() else { return };
            let Ok(answer) = cx.update(|window, cx| overlay::prompt_text(TextPrompt::new("Folder name", "Clone").initial(repo_name_from_url(&url)), window, cx)) else { return };
            let Some(name) = answer.await.map(|n| n.trim().to_string()) else { return };
            if let Some(problem) = validate_folder_name(&name) {
                let _ = cx.update(|window, cx| overlay::toast_error(problem, window, cx));
                return;
            }
            let _ = cx.update(|window, cx| overlay::toast_info(format!("Cloning {url}…"), window, cx));
            let (clone_core, parent_text) = (core.clone(), parent.to_string_lossy().into_owned());
            let operation = format!("clone-{}", core.next_id());
            let result = core_call(cx, async move { git_api::git_clone(&clone_core, url, parent_text, name, true, operation, Arc::new(|_| {})).await }).await;
            let _ = cx.update(|window, cx| match result {
                Ok(dest) => {
                    if let Some(shell) = Shell::for_window(window, cx) {
                        shell.update(cx, |shell, cx| shell.open_folder(PathBuf::from(dest), window, cx));
                    }
                }
                Err(error) => overlay::show_error(&error, window, cx),
            });
        })
        .detach();
}
