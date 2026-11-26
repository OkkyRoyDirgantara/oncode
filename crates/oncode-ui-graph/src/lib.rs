//! Commit graph, commit details, history operations, interactive rebase, reflog, worktree, and submodule.

pub mod actions;
pub mod format;
pub mod geometry;
pub mod graph_tab;
pub mod lists;
pub mod menus;
pub mod paths;
pub mod query;
pub mod rebase_dialog;
pub mod rebase_plan;
pub mod refs;
pub mod store;
#[cfg(test)]
mod testing;

use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{div, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window};
use oncode_core::api::git;
use oncode_ui::commands::Command;
use oncode_ui::editor::active_file;
use oncode_ui::overlay::{self, Confirm, TextPrompt};
use oncode_ui::quick_pick::{PickItem, QuickPickOptions};
use oncode_ui::{Env, Lucide, OpenOptions, SidebarView, TabContent, TabData, TabKind, ViewTab, Workbench};
use serde_json::{json, Value};

use actions::{ask, handle_git_error, pick, prompt, run_with_progress, Ctx};
use graph_tab::GraphTab;
use lists::{ReflogTab, StashTab};
use paths::{default_worktree_name, is_dirty_worktree_error, locate_in_repo};
use store::{CurrentStore, GraphStore, RevealTarget};

pub const SIDEBAR_ID: &str = "graph";

/// Registers this feature. Called once from `app/src/main.rs` after `oncode_ui::init`.
pub fn init(cx: &mut App) {
    oncode_ui::register_tab_kind(cx, "graph", GraphKind);
    oncode_ui::register_tab_kind(cx, "fileHistory", GraphKind);
    oncode_ui::register_tab_kind(cx, "reflog", ReflogKind);
    oncode_ui::register_tab_kind(cx, "stash", StashKind);
    oncode_ui::register_feature(cx, setup);
}

/// The Graph store of this window; `None` if the folder is not trusted or is not a repository.
pub fn graph_store(workbench: &Workbench) -> Option<Entity<GraphStore>> {
    workbench.extension::<Entity<GraphStore>>()
}

fn current_store(cx: &App) -> Entity<GraphStore> {
    cx.global::<CurrentStore>().0.clone()
}

fn root_param(tab: &TabData) -> String {
    tab.param_str("root").unwrap_or_default().to_string()
}

struct GraphKind;

impl TabKind for GraphKind {
    fn build(&self, tab: &TabData, _: &Env, window: &mut Window, cx: &mut App) -> Box<dyn TabContent> {
        let store = current_store(cx);
        let (root, path) = (root_param(tab), tab.param_str("path").map(String::from));
        Box::new(ViewTab(cx.new(|cx| GraphTab::new(store, root, path, window, cx))))
    }

    fn icon(&self, tab: &TabData) -> Option<Lucide> {
        Some(if tab.kind == "fileHistory" { Lucide::Clock } else { Lucide::GitGraph })
    }
}

struct ReflogKind;

impl TabKind for ReflogKind {
    fn build(&self, tab: &TabData, _: &Env, _: &mut Window, cx: &mut App) -> Box<dyn TabContent> {
        let ctx = Ctx::new(&current_store(cx), root_param(tab));
        Box::new(ViewTab(cx.new(|cx| ReflogTab::new(ctx, cx))))
    }

    fn icon(&self, _: &TabData) -> Option<Lucide> {
        Some(Lucide::Clock)
    }
}

struct StashKind;

impl TabKind for StashKind {
    fn build(&self, tab: &TabData, _: &Env, _: &mut Window, cx: &mut App) -> Box<dyn TabContent> {
        let ctx = Ctx::new(&current_store(cx), root_param(tab));
        let name = tab.param_str("name").unwrap_or_default().to_string();
        Box::new(ViewTab(cx.new(|cx| StashTab::new(ctx, name, cx))))
    }

    fn icon(&self, _: &TabData) -> Option<Lucide> {
        Some(Lucide::Archive)
    }
}

fn open_tab(store: &Entity<GraphStore>, tab: TabData, window: &mut Window, cx: &mut App) {
    let workbench = store.read(cx).env().workbench.clone();
    let _ = workbench.update(cx, |workbench, cx| {
        workbench.open_tab(tab, OpenOptions::default(), window, cx);
    });
}

/// Opens the Graph tab of a repository, optionally selecting a commit right away.
pub fn open_graph(store: &Entity<GraphStore>, root: &str, reveal: Option<RevealTarget>, window: &mut Window, cx: &mut App) {
    if let Some(target) = reveal {
        let root = root.to_string();
        store.update(cx, |store, cx| store.request_reveal(&root, target, cx));
    }
    let tab = TabData::new("graph", "Graph", json!({ "root": root })).with_id(format!("graph:{root}")).with_description(format!("Commit graph — {root}"));
    open_tab(store, tab, window, cx);
}

pub fn open_reflog(store: &Entity<GraphStore>, root: &str, window: &mut Window, cx: &mut App) {
    let tab = TabData::new("reflog", "Reflog", json!({ "root": root })).with_id(format!("reflog:{root}")).with_description(format!("Reflog — {root}"));
    open_tab(store, tab, window, cx);
}

pub fn open_stash(store: &Entity<GraphStore>, root: &str, name: &str, window: &mut Window, cx: &mut App) {
    let tab = TabData::new("stash", name.to_string(), json!({ "root": root, "name": name })).with_id(format!("stash:{root}:{name}")).with_description(format!("Stash {name}"));
    open_tab(store, tab, window, cx);
}

/// Opens the history of a file (a repository-relative path).
pub fn open_file_history(store: &Entity<GraphStore>, root: &str, path: &str, window: &mut Window, cx: &mut App) {
    let title = format!("History: {}", format::base_name(path));
    let tab = TabData::new("fileHistory", title, json!({ "root": root, "path": path })).with_id(format!("fileHistory:{root}:{path}")).with_description(format!("File history — {path}"));
    open_tab(store, tab, window, cx);
}

/// The Graph tab currently active in this window.
pub(crate) fn active_tab(window: &Window, cx: &App) -> Option<Entity<GraphTab>> {
    let workbench = Workbench::for_window(window, cx)?;
    let content = workbench.read(cx).active_content()?;
    content.as_any().downcast_ref::<ViewTab<GraphTab>>().map(|tab| tab.0.clone())
}

fn setup(workbench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let env = workbench.env().clone();
    if !env.trusted || env.repo_root.is_none() {
        return;
    }
    let store = cx.new(|cx| GraphStore::new(env.clone(), cx));
    workbench.set_extension(store.clone());
    workbench.on_before_build_tabs({
        let store = store.clone();
        move |cx| cx.set_global(CurrentStore(store.clone()))
    });
    workbench.register_sidebar(SidebarView::new(SIDEBAR_ID, "Graph", Lucide::GitGraph, 40, {
        let store = store.clone();
        move |_, window, cx| {
            let store = store.clone();
            cx.new(|cx| GraphSidebar::new(store, window, cx)).into()
        }
    }));
    let _ = window;

    let with_root = |f: fn(Entity<GraphStore>, String, &mut Window, &mut App)| {
        let store = store.clone();
        move |_: &Value, window: &mut Window, cx: &mut App| {
            let Some(root) = store.read(cx).default_root().map(String::from) else { return };
            f(store.clone(), root, window, cx);
        }
    };
    let commands = [
        Command::new("git.showGraph", "Show Graph", with_root(|store, root, window, cx| open_graph(&store, &root, None, window, cx))).key("Mod+Shift+H"),
        Command::new("git.showReflog", "Show Reflog", with_root(|store, root, window, cx| open_reflog(&store, &root, window, cx))),
        Command::new("git.interactiveRebase", "Interactive Rebase…", with_root(|store, root, window, cx| rebase_dialog::open(Ctx::new(&store, root), None, window, cx))),
        Command::new("git.fileHistoryActive", "Show History of Active File", with_root(file_history_of_active)),
        Command::new("git.fileHistory", "Show File History…", with_root(file_history_of_active)),
        Command::new("git.showStash", "Show Stash…", with_root(|store, root, window, cx| {
            let stashes = store.read(cx).stashes(&root).to_vec();
            if stashes.is_empty() {
                overlay::toast_info("There are no stashes.", window, cx);
                return;
            }
            let items = stashes.iter().map(|s| PickItem::new(s.name.clone(), s.name.clone()).description(s.message.clone())).collect();
            oncode_ui::quick_pick::quick_pick(
                QuickPickOptions::new("Show which stash?", items),
                move |item, window, cx| {
                    if let Some(item) = item {
                        open_stash(&store, &root, &item.value, window, cx);
                    }
                },
                window,
                cx,
            );
        })),
        Command::new("git.addWorktree", "Add Worktree…", with_root(|store, root, window, cx| add_worktree(Ctx::new(&store, root), window, cx))),
        Command::new("git.removeWorktree", "Remove Worktree…", with_root(|store, root, window, cx| remove_worktree(Ctx::new(&store, root), window, cx))),
        Command::new("git.updateSubmodules", "Update Submodules", with_root(|store, root, window, cx| {
            let ctx = Ctx::new(&store, root);
            window
                .spawn(cx, async move |cx| {
                    let done = run_with_progress(&ctx, "Update submodules", |core, root, id, progress| async move { git::git_submodule_update(&core, root, Vec::new(), true, id, progress).await }, cx).await;
                    if done {
                        let _ = cx.update(|_, cx| ctx.store.update(cx, |store, cx| store.reload(&ctx.root, cx)));
                    }
                })
                .detach();
        })),
    ];
    for command in commands {
        workbench.register_command(command.category("Git"));
    }
}

fn file_history_of_active(store: Entity<GraphStore>, root: String, window: &mut Window, cx: &mut App) {
    let path = Workbench::for_window(window, cx).and_then(|w| active_file(w.read(cx), cx)).map(|(_, buffer, _)| buffer.read(cx).path().to_string());
    let Some(path) = path else {
        overlay::toast_info("Open a file first.", window, cx);
        return;
    };
    match locate_in_repo([root.as_str()], &path) {
        Some((root, relative)) => open_file_history(&store, &root, &relative, window, cx),
        None => overlay::toast_info("This file is not part of an open Git repository.", window, cx),
    }
}

/// New worktree: pick a branch (or type a new branch name), then the target folder next to the repository.
fn add_worktree(ctx: Ctx, window: &mut Window, cx: &mut App) {
    let branches: Vec<String> = ctx.store.read(cx).refs(&ctx.root).map(|refs| refs.branches.iter().filter(|b| !b.is_head && b.worktree.is_none()).map(|b| b.name.clone()).collect()).unwrap_or_default();
    window
        .spawn(cx, async move |cx| {
            let items = branches.iter().map(|b| PickItem::new(b.clone(), b.clone()).description("existing branch")).collect();
            let options = QuickPickOptions::new("Branch for the new worktree (type a new name to create one)", items).allow_custom(|text| Some(PickItem::new(format!("Create branch \"{text}\""), format!("new:{text}"))));
            let Some(picked) = pick(cx, options).await else { return };
            let (branch, new_branch) = match picked.value.strip_prefix("new:") {
                Some(name) => (name.to_string(), Some(name.to_string())),
                None => (picked.value.clone(), None),
            };
            if let Some(problem) = paths::validate_ref_name(&branch) {
                let _ = cx.update(|window, cx| overlay::toast_error(problem, window, cx));
                return;
            }
            let parent = PathBuf::from(&ctx.root).parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from(&ctx.root));
            let suggested = parent.join(default_worktree_name(&ctx.root, &branch)).to_string_lossy().into_owned();
            let Some(path) = prompt(cx, TextPrompt::new("Worktree folder", "Create Worktree").initial(suggested)).await else { return };
            let path = path.trim().to_string();
            let rev = if new_branch.is_some() { None } else { Some(branch.clone()) };
            let Ok(core) = cx.update(|_, cx| ctx.core(cx)) else { return };
            let root = ctx.root.clone();
            let task_path = path.clone();
            let Ok(task) = cx.update(|_, cx| oncode_ui::spawn_core(cx, async move { git::git_worktree_add(&core, root, task_path, new_branch, rev).await })) else { return };
            let result = task.await;
            let open = match result {
                Ok(()) => ask(cx, Confirm::new("Worktree created", format!("Open {path} in a new window?"), "Open")).await,
                Err(error) => {
                    let _ = cx.update(|window, cx| handle_git_error(&ctx, &error, "Add worktree", None, window, cx));
                    false
                }
            };
            let _ = cx.update(|_, cx| {
                ctx.store.update(cx, |store, cx| store.reload(&ctx.root, cx));
                if open {
                    oncode_ui::open_window(Some(PathBuf::from(&path)), cx);
                }
            });
        })
        .detach();
}

fn remove_worktree(ctx: Ctx, window: &mut Window, cx: &mut App) {
    let worktrees: Vec<_> = ctx.store.read(cx).data(&ctx.root).map(|d| d.worktrees.iter().filter(|w| !w.is_main && !w.is_current).cloned().collect()).unwrap_or_default();
    if worktrees.is_empty() {
        overlay::toast_info("There are no other worktrees.", window, cx);
        return;
    }
    window
        .spawn(cx, async move |cx| {
            let items = worktrees.iter().map(|w| PickItem::new(format::base_name(&w.path).to_string(), w.path.clone()).description(w.branch.clone().unwrap_or_else(|| "detached".into()))).collect();
            let Some(picked) = pick(cx, QuickPickOptions::new("Remove which worktree?", items)).await else { return };
            let message = format!("The folder {} is deleted. Its branch is kept.", picked.value);
            if !ask(cx, Confirm::new("Remove worktree?", message, "Remove").danger()).await {
                return;
            }
            let mut force = false;
            loop {
                let Ok(core) = cx.update(|_, cx| ctx.core(cx)) else { return };
                let (root, path) = (ctx.root.clone(), picked.value.clone());
                let Ok(task) = cx.update(|_, cx| oncode_ui::spawn_core(cx, async move { git::git_worktree_remove(&core, root, path, force).await })) else { return };
                match task.await {
                    Ok(()) => break,
                    Err(error) if !force && is_dirty_worktree_error(&error) => {
                        let message = "The worktree has uncommitted or untracked changes. Removing it anyway deletes them permanently.";
                        if !ask(cx, Confirm::new("Worktree has changes", message, "Remove Anyway").danger()).await {
                            return;
                        }
                        force = true;
                    }
                    Err(error) => {
                        let _ = cx.update(|window, cx| handle_git_error(&ctx, &error, "Remove worktree", None, window, cx));
                        return;
                    }
                }
            }
            let _ = cx.update(|_, cx| ctx.store.update(cx, |store, cx| store.reload(&ctx.root, cx)));
        })
        .detach();
}

/// Graph sidebar: shortcuts to Graph, Reflog, and file history, plus the list of worktrees and submodules.
struct GraphSidebar {
    store: Entity<GraphStore>,
    _subscription: Subscription,
}

impl GraphSidebar {
    fn new(store: Entity<GraphStore>, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&store, |_, _, cx| cx.notify());
        Self { store, _subscription: subscription }
    }
}

impl Render for GraphSidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let Some(root) = store.default_root().map(String::from) else { return div().into_any_element() };
        let data = store.data(&root).cloned().unwrap_or_default();
        let theme = cx.theme();
        let command = |id: &'static str, icon: Lucide, label: &'static str| {
            Button::new(SharedString::from(format!("graph-sidebar-{id}"))).ghost().small().w_full().icon(Icon::new(icon)).label(label).on_click(move |_, window, cx| oncode_ui::execute(id, Value::Null, window, cx))
        };
        let heading = |text: &str| div().px_2().pt_3().pb_1().text_xs().text_color(theme.muted_foreground).child(text.to_uppercase());
        v_flex()
            .id("graph-sidebar")
            .size_full()
            .p_1()
            .gap_0p5()
            .overflow_y_scroll()
            .child(command("git.showGraph", Lucide::GitGraph, "Show Graph"))
            .child(command("git.showReflog", Lucide::Clock, "Show Reflog"))
            .child(command("git.fileHistoryActive", Lucide::FileClock, "History of Active File"))
            .child(command("git.interactiveRebase", Lucide::ListOrdered, "Interactive Rebase…"))
            .child(heading("Worktrees"))
            .children(data.worktrees.iter().map(|worktree| {
                let path = worktree.path.clone();
                h_flex()
                    .id(SharedString::from(format!("worktree-{}", worktree.path)))
                    .px_2()
                    .py_0p5()
                    .gap_1p5()
                    .text_sm()
                    .rounded(theme.radius)
                    .hover(|s| s.bg(theme.list_hover))
                    .child(Icon::new(Lucide::FolderGit2).xsmall())
                    .child(div().flex_1().min_w_0().truncate().when(worktree.is_current, |el| el.font_weight(gpui_kit::FontWeight::SEMIBOLD)).child(format::base_name(&worktree.path).to_string()))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(worktree.branch.clone().unwrap_or_else(|| "detached".into())))
                    .when(!worktree.is_current, |el| el.on_click(move |_, _, cx| oncode_ui::open_window(Some(PathBuf::from(&path)), cx)))
            }))
            .child(command("git.addWorktree", Lucide::Plus, "Add Worktree…"))
            .when(!data.submodules.is_empty(), |el| {
                el.child(heading("Submodules"))
                    .children(data.submodules.iter().map(|submodule| {
                        h_flex()
                            .px_2()
                            .py_0p5()
                            .gap_1p5()
                            .text_sm()
                            .child(Icon::new(Lucide::Package).xsmall())
                            .child(div().flex_1().min_w_0().truncate().child(submodule.path.clone()))
                            .child(div().text_xs().text_color(theme.muted_foreground).child(format!("{:?}", submodule.status).to_lowercase()))
                    }))
                    .child(command("git.updateSubmodules", Lucide::RefreshCw, "Update Submodules"))
            })
            .into_any_element()
    }
}
