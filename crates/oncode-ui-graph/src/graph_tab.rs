//! Graph tab (also used for the history of a file): ref panel on the left, commit list with
//! graph lanes in the middle, and commit details on the right.

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::resizable::{h_resizable, resizable_panel};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    canvas, div, fill, point, px, size, uniform_list, AnyElement, App, AppContext as _, Bounds, ClickEvent, Context, Entity, FocusHandle, Focusable, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, PathBuilder, Render, ScrollStrategy, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task,
    UniformListScrollHandle, Window,
};
use oncode_core::api::git;
use oncode_git::{ChangeKind, CommitDetail, CommitFile, DiffSource, GraphRow, LogQuery, RefKind};
use gpui_kit::TestSupportExt as _;
use oncode_ui::{spawn_core, Env, Lucide};
use oncode_ui_diff::{open_diff, DiffRequest};

use crate::actions::{run_action, Ctx};
use crate::format::{base_name, full_date, relative_date, short_oid};
use crate::geometry::{edge_segments, graph_width, lane_color_index, lane_x, max_lane, Segment, DOT_RADIUS, ROW_HEIGHT};
use crate::menus::{branch_menu, commit_menu, remote_branch_menu, stash_menu, tag_menu, CommitMenuContext, GitAction, MenuEntry};
use crate::query::{build_query, is_filtered, next_selection, order_oldest_first};
use crate::refs::{group_refs, remote_short_name};
use crate::store::{snapshot as repo_snapshot, GraphStore, StoreEvent};

const PAGE: u32 = 300;

/// A `git log` stream that a load has opened but not yet handed to its tab. It is closed when dropped,
/// so a load that is thrown away (a newer reload started, the tab closed, or the first page failed)
/// does not leave its `git log` running until the app quits.
struct OpenedStream {
    core: oncode_core::Shared,
    id: Option<u32>,
}

impl OpenedStream {
    fn id(&self) -> u32 {
        self.id.expect("the stream is not handed over yet")
    }

    /// Hands the stream over to the tab, which closes it from then on.
    fn hand_over(mut self) -> u32 {
        self.id.take().expect("the stream is handed over once")
    }
}

impl Drop for OpenedStream {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            let core = self.core.clone();
            self.core.spawn(async move { git::git_log_close(&core, id).await });
        }
    }
}

/// Graph lane color (repeats every 8 lanes).
fn lane_color(index: usize, cx: &App) -> Hsla {
    let theme = cx.theme();
    [theme.blue, theme.green, theme.magenta, theme.yellow, theme.cyan, theme.red, theme.primary, theme.muted_foreground][index % 8]
}

/// Contents of the right panel.
enum Details {
    None,
    Loading,
    Commit(Box<CommitDetail>),
    /// Comparison of two commits: [old, new] and the files that differ.
    Compare { from: String, to: String, files: Vec<CommitFile> },
    Failed(String),
}

pub struct GraphTab {
    ctx: Ctx,
    env: Env,
    /// `Some(path)`: the history of a single file (a repository-relative path).
    path: Option<String>,
    search: Entity<InputState>,
    query: LogQuery,
    rows: Vec<GraphRow>,
    /// The rightmost lane used by `rows`; kept up to date as pages arrive, so drawing does not scan every row.
    lanes: i32,
    done: bool,
    stream: Option<u32>,
    loading: bool,
    error: Option<String>,
    /// Incremented each time the query restarts; results of the old query are discarded.
    revision: u64,
    selected: Vec<String>,
    details: Details,
    /// A commit to select as soon as it appears (from the ref panel or a "Show in Graph" request).
    hunt: Option<String>,
    scroll: UniformListScrollHandle,
    focus_handle: FocusHandle,
    _search_debounce: Task<()>,
    _load: Task<()>,
    _details: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for GraphTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl GraphTab {
    pub fn new(store: Entity<GraphStore>, root: String, path: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let env = store.read(cx).env().clone();
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(if path.is_some() { "Filter by message" } else { "Search commits by message" }));
        let subscriptions = vec![
            cx.subscribe_in(&search, window, |this: &mut Self, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change | InputEvent::PressEnter { .. }) {
                    let immediate = matches!(event, InputEvent::PressEnter { .. });
                    this.schedule_search(immediate, window, cx);
                }
            }),
            cx.subscribe(&store, |this: &mut Self, store, event: &StoreEvent, cx| match event {
                StoreEvent::RefsChanged(root) if *root == this.ctx.root => this.reload(cx),
                StoreEvent::RevealRequested(root) if *root == this.ctx.root => {
                    if let Some(target) = store.update(cx, |store, _| store.take_reveal(root)) {
                        this.reveal(&target.oid, cx);
                    }
                }
                _ => cx.notify(),
            }),
            // A closed tab stops its `git log`; otherwise the process waits on its pipe until the app quits.
            cx.on_release(|this: &mut Self, cx| {
                if let Some(stream) = this.stream.take() {
                    let core = this.env.core.clone();
                    spawn_core(cx, async move { git::git_log_close(&core, stream).await }).detach();
                }
            }),
        ];
        store.update(cx, |store, cx| store.ensure(&root, cx));
        let mut this = Self {
            ctx: Ctx::new(&store, root),
            env,
            path,
            search,
            query: LogQuery::default(),
            rows: Vec::new(),
            lanes: -1,
            done: false,
            stream: None,
            loading: false,
            error: None,
            revision: 0,
            selected: Vec::new(),
            details: Details::None,
            hunt: None,
            scroll: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            _search_debounce: Task::ready(()),
            _load: Task::ready(()),
            _details: Task::ready(()),
            _subscriptions: subscriptions,
        };
        if let Some(target) = store.update(cx, |store, _| store.take_reveal(&this.ctx.root)) {
            this.hunt = Some(target.oid);
        }
        this.reload(cx);
        this
    }

    pub fn rows(&self) -> &[GraphRow] {
        &self.rows
    }

    pub fn selected(&self) -> &[String] {
        &self.selected
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    fn schedule_search(&mut self, immediate: bool, window: &mut Window, cx: &mut Context<Self>) {
        self._search_debounce = cx.spawn_in(window, async move |this, cx| {
            if !immediate {
                cx.background_executor().timer(std::time::Duration::from_millis(300)).await;
            }
            let _ = this.update(cx, |this, cx| this.reload(cx));
        });
    }

    /// Restarts the list with the current query.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let grep = self.search.read(cx).value().to_string();
        self.query = build_query("all", &grep, "", self.path.as_deref().unwrap_or(""));
        self.revision += 1;
        let revision = self.revision;
        if let Some(stream) = self.stream.take() {
            let core = self.env.core.clone();
            spawn_core(cx, async move { git::git_log_close(&core, stream).await }).detach();
        }
        self.loading = true;
        self.error = None;
        let (core, root, query) = (self.env.core.clone(), self.ctx.root.clone(), self.query.clone());
        // A reload that starts before this one finishes drops this task's result (or the task), which
        // closes the stream it opened.
        let task = spawn_core(cx, async move {
            let id = git::git_log_open(&core, root, query).await?;
            let stream = OpenedStream { core: core.clone(), id: Some(id) };
            let page = git::git_log_next(&core, stream.id(), PAGE).await?;
            Ok::<_, oncode_core::AppError>((stream, page))
        });
        self._load = cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.revision != revision {
                    return;
                }
                this.loading = false;
                match result {
                    Ok((stream, page)) => {
                        this.stream = Some(stream.hand_over());
                        this.lanes = max_lane(page.rows.iter());
                        this.rows = page.rows;
                        this.done = page.done;
                        // The selection is kept if its commit still exists.
                        this.selected.retain(|oid| this.rows.iter().any(|row| &row.commit.oid == oid));
                        if this.selected.is_empty() {
                            if let Some(first) = this.rows.first() {
                                this.selected = vec![first.commit.oid.clone()];
                            }
                        }
                        this.after_rows_changed(cx);
                        this.load_details(cx);
                    }
                    Err(error) => {
                        this.rows.clear();
                        this.lanes = -1;
                        this.error = Some(error.message);
                    }
                }
                cx.notify();
            });
        });
        cx.notify();
    }

    fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.loading || self.done {
            return;
        }
        let Some(stream) = self.stream else { return };
        self.loading = true;
        let revision = self.revision;
        let core = self.env.core.clone();
        let task = spawn_core(cx, async move { git::git_log_next(&core, stream, PAGE).await });
        self._load = cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.revision != revision {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(page) => {
                        this.lanes = this.lanes.max(max_lane(page.rows.iter()));
                        this.rows.extend(page.rows);
                        this.done = page.done;
                        this.after_rows_changed(cx);
                    }
                    Err(error) => this.error = Some(error.message),
                }
                cx.notify();
            });
        });
    }

    /// If a commit is being hunted, select it once visible; otherwise load the next page.
    fn after_rows_changed(&mut self, cx: &mut Context<Self>) {
        let Some(oid) = self.hunt.clone() else { return };
        if let Some(index) = self.rows.iter().position(|row| row.commit.oid == oid) {
            self.hunt = None;
            self.selected = vec![oid];
            self.scroll.scroll_to_item(index, ScrollStrategy::Center);
            self.load_details(cx);
        } else if !self.done {
            self.load_more(cx);
        } else {
            self.hunt = None;
        }
    }

    /// Selects a commit, loading the next page if it is not visible yet.
    pub fn reveal(&mut self, oid: &str, cx: &mut Context<Self>) {
        self.hunt = Some(oid.to_string());
        self.after_rows_changed(cx);
        cx.notify();
    }

    fn select(&mut self, oid: &str, additive: bool, cx: &mut Context<Self>) {
        self.selected = next_selection(&self.selected, oid, additive);
        self.load_details(cx);
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        let current = self.selected.last().and_then(|oid| self.rows.iter().position(|row| &row.commit.oid == oid)).unwrap_or(0);
        let next = (current as isize + delta).clamp(0, self.rows.len() as isize - 1) as usize;
        self.selected = vec![self.rows[next].commit.oid.clone()];
        self.scroll.scroll_to_item(next, ScrollStrategy::Nearest);
        if next + 20 > self.rows.len() {
            self.load_more(cx);
        }
        self.load_details(cx);
        cx.notify();
    }

    fn load_details(&mut self, cx: &mut Context<Self>) {
        let (core, root) = (self.env.core.clone(), self.ctx.root.clone());
        match self.selected.as_slice() {
            [] => {
                self.details = Details::None;
                self._details = Task::ready(());
            }
            [oid] => {
                self.details = Details::Loading;
                let oid = oid.clone();
                let task = spawn_core(cx, async move { git::git_commit_detail(&core, root, oid).await });
                self._details = cx.spawn(async move |this, cx| {
                    let result = task.await;
                    let _ = this.update(cx, |this, cx| {
                        this.details = match result {
                            Ok(detail) => Details::Commit(Box::new(detail)),
                            Err(error) => Details::Failed(error.message),
                        };
                        cx.notify();
                    });
                });
            }
            _ => {
                let ordered = order_oldest_first(&self.rows, &self.selected);
                let (from, to) = (ordered[0].clone(), ordered[ordered.len() - 1].clone());
                self.details = Details::Loading;
                let (from_task, to_task) = (from.clone(), to.clone());
                let task = spawn_core(cx, async move { git::git_diff_files(&core, root, from_task, to_task).await });
                self._details = cx.spawn(async move |this, cx| {
                    let result = task.await;
                    let _ = this.update(cx, |this, cx| {
                        this.details = match result {
                            Ok(files) => Details::Compare { from, to, files },
                            Err(error) => Details::Failed(error.message),
                        };
                        cx.notify();
                    });
                });
            }
        }
    }


    // ----- View -----

    fn render_refs(&self, cx: &mut Context<Self>) -> AnyElement {
        let store = self.ctx.store.read(cx);
        let refs = store.refs(&self.ctx.root).cloned();
        let stashes = store.stashes(&self.ctx.root).to_vec();
        let remote = store.first_remote(&self.ctx.root);
        let groups = group_refs(refs.as_ref(), &stashes, "");
        let theme = cx.theme();
        let heading = |text: &str| div().px_2().pt_2().pb_1().text_xs().text_color(theme.muted_foreground).child(text.to_uppercase());
        let entity = cx.entity();
        let ctx = self.ctx.clone();
        let item = move |id: String, icon: Lucide, label: String, bold: bool, oid: String, menu: Vec<MenuEntry>, cx: &App| {
            let entity = entity.clone();
            let ctx = ctx.clone();
            h_flex()
                .id(SharedString::from(id))
                .w_full()
                .px_2()
                .py_0p5()
                .gap_1p5()
                .text_sm()
                .rounded(cx.theme().radius)
                .hover(|s| s.bg(cx.theme().list_hover))
                .when(bold, |el| el.font_weight(gpui_kit::FontWeight::SEMIBOLD))
                .child(Icon::new(icon).xsmall().text_color(cx.theme().muted_foreground))
                .child(div().flex_1().min_w_0().truncate().child(label))
                .on_click(move |_, _, cx| entity.update(cx, |this, cx| this.reveal(&oid, cx)))
                .context_menu(move |popup, _, _| graph_menu(&ctx, menu.clone(), popup))
        };
        v_flex()
            .id("graph-refs")
            .size_full()
            .overflow_y_scroll()
            .pb_2()
            .child(heading("Branches"))
            .children(groups.branches.iter().map(|branch| item(format!("ref-branch-{}", branch.name), Lucide::GitBranch, branch.name.clone(), branch.is_head, branch.oid.clone(), branch_menu(branch), cx)))
            .when(!groups.remotes.is_empty(), |el| el.child(heading("Remotes")))
            .children(groups.remotes.iter().flat_map(|group| {
                group.branches.iter().map(|branch| {
                    item(format!("ref-remote-{}", branch.name), Lucide::Cloud, format!("{}/{}", group.remote, remote_short_name(branch)), false, branch.oid.clone(), remote_branch_menu(branch), cx)
                })
            }))
            .when(!groups.tags.is_empty(), |el| el.child(heading("Tags")))
            .children(groups.tags.iter().map(|tag| item(format!("ref-tag-{}", tag.name), Lucide::Tag, tag.name.clone(), false, tag.oid.clone(), tag_menu(tag, remote.as_deref()), cx)))
            .when(!groups.stashes.is_empty(), |el| el.child(heading("Stashes")))
            .children(groups.stashes.iter().map(|stash| item(format!("ref-stash-{}", stash.name), Lucide::Archive, format!("{} {}", stash.name, stash.message), false, stash.oid.clone(), stash_menu(stash), cx)))
            .into_any_element()
    }

    fn render_rows(&mut self, range: Range<usize>, _: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if range.end + 40 > self.rows.len() {
            self.load_more(cx);
        }
        let width = graph_width(self.lanes);
        let snapshot = repo_snapshot(&self.ctx.root, cx);
        let head_oid = snapshot.as_ref().and_then(|s| s.head_oid.clone());
        let lane_colors: Rc<[Hsla]> = (0..8).map(|i| lane_color(i, cx)).collect();
        let entity = cx.entity();
        range
            .filter_map(|index| {
                let row = self.rows.get(index)?;
                let theme = cx.theme();
                let selected = self.selected.contains(&row.commit.oid);
                let is_head = head_oid.as_deref() == Some(row.commit.oid.as_str());
                let oid = row.commit.oid.clone();
                let menu_oid = oid.clone();
                let (entity, menu_entity) = (entity.clone(), entity.clone());
                let lane_colors = lane_colors.clone();
                let graph_row = row.clone();
                let graph = canvas(
                    |_, _, _| {},
                    move |bounds: Bounds<gpui_kit::Pixels>, _, window, _| paint_row(&graph_row, bounds, &lane_colors, window),
                )
                .w(px(width))
                .h(px(ROW_HEIGHT))
                .flex_none();
                let has_branch = row.commit.refs.iter().any(|label| label.kind == RefKind::Branch);
                let badges = row.commit.refs.iter().filter(|label| !(label.kind == RefKind::Head && has_branch)).take(3).map(|label| {
                    let color = match label.kind {
                        RefKind::Head | RefKind::Branch => theme.primary,
                        RefKind::Remote => theme.blue,
                        RefKind::Tag => theme.yellow,
                        RefKind::Stash => theme.magenta,
                    };
                    div().flex_none().px_1().rounded(px(3.)).border_1().border_color(color).text_color(color).text_xs().child(label.name.clone())
                });
                let menu_ctx = self.ctx.clone();
                Some(
                    h_flex()
                        .id(SharedString::from(format!("graph-row-{oid}")))
                        .test_support()
                        .w_full()
                        .h(px(ROW_HEIGHT))
                        .pr_2()
                        .gap_2()
                        .text_sm()
                        .when(selected, |el| el.bg(theme.list_active))
                        .when(!selected, |el| el.hover(|s| s.bg(theme.list_hover)))
                        .on_click(move |event: &ClickEvent, _, cx| {
                            let additive = event.modifiers().control || event.modifiers().platform;
                            entity.update(cx, |this, cx| this.select(&oid, additive, cx));
                        })
                        // The rows and the selection are read when the menu opens, not copied into every row.
                        .context_menu(move |popup, _, cx| {
                            let this = menu_entity.read(cx);
                            let Some(row) = this.rows.iter().find(|r| r.commit.oid == menu_oid) else { return popup };
                            let selection = order_oldest_first(&this.rows, &this.selected);
                            let snapshot = repo_snapshot(&menu_ctx.root, cx);
                            let entries = commit_menu(&CommitMenuContext { snapshot: snapshot.as_ref(), commit: &row.commit, rows: &this.rows, filtered: is_filtered(&this.query), selection: Some(&selection) });
                            graph_menu(&menu_ctx, entries, popup)
                        })
                        .child(graph)
                        .children(badges)
                        .child(div().flex_1().min_w_0().truncate().when(is_head, |el| el.font_weight(gpui_kit::FontWeight::SEMIBOLD)).child(row.commit.subject.clone()))
                        .child(div().flex_none().w(px(110.)).truncate().text_color(theme.muted_foreground).child(row.commit.author.name.clone()))
                        .child(div().flex_none().w(px(80.)).truncate().text_color(theme.muted_foreground).child(relative_date(row.commit.author.time)))
                        .child(div().flex_none().w(px(64.)).font_family(theme.mono_font_family.clone()).text_xs().text_color(theme.muted_foreground).child(short_oid(&row.commit.oid).to_string()))
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn render_file(&self, file: &CommitFile, source: DiffSource, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (letter, color) = match file.kind {
            ChangeKind::Added => ("A", theme.green),
            ChangeKind::Deleted => ("D", theme.red),
            ChangeKind::Renamed | ChangeKind::Copied => ("R", theme.blue),
            _ => ("M", theme.yellow),
        };
        let stats = match (file.additions, file.deletions) {
            (Some(add), Some(del)) => format!("+{add} −{del}"),
            _ => String::new(),
        };
        let (workbench, root) = (self.env.workbench.clone(), self.ctx.root.clone());
        let file_owned = file.clone();
        h_flex()
            .id(SharedString::from(format!("graph-file-{}", file.path)))
            .w_full()
            .px_2()
            .py_0p5()
            .gap_2()
            .text_sm()
            .rounded(theme.radius)
            .hover(|s| s.bg(theme.list_hover))
            .on_click(move |_, window, cx| open_file_diff(&workbench, root.clone(), &file_owned, source.clone(), window, cx))
            .child(div().w(px(12.)).text_color(color).child(letter))
            .child(div().flex_none().child(base_name(&file.path).to_string()))
            .child(div().flex_1().min_w_0().truncate().text_xs().text_color(theme.muted_foreground).child(file.path.clone()))
            .child(div().flex_none().text_xs().text_color(theme.muted_foreground).child(stats))
            .into_any_element()
    }

    fn render_details(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let body = v_flex().id("graph-details").size_full().overflow_y_scroll().p_3().gap_2().text_sm();
        match &self.details {
            Details::None => body.text_color(theme.muted_foreground).child("Select a commit to see its details.").into_any_element(),
            Details::Loading => body.child(Spinner::new()).into_any_element(),
            Details::Failed(message) => body.text_color(theme.danger).child(message.clone()).into_any_element(),
            Details::Commit(detail) => {
                let commit = &detail.commit;
                let source = DiffSource::Commit { oid: commit.oid.clone(), parent: None };
                let files: Vec<AnyElement> = detail.files.iter().map(|file| self.render_file(file, source.clone(), cx)).collect();
                let theme = cx.theme();
                body.child(div().text_base().font_weight(gpui_kit::FontWeight::SEMIBOLD).child(commit.subject.clone()))
                    .when_some(message_body(&commit.subject, &detail.body), |el, body| el.child(div().whitespace_normal().text_color(theme.foreground).child(body)))
                    .child(
                        v_flex()
                            .gap_0p5()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("{} <{}>", commit.author.name, commit.author.email))
                            .child(full_date(commit.author.time))
                            .child(div().font_family(theme.mono_font_family.clone()).child(commit.oid.clone()))
                            .when(!commit.parents.is_empty(), |el| el.child(format!("Parents: {}", commit.parents.iter().map(|p| short_oid(p).to_string()).collect::<Vec<_>>().join(", ")))),
                    )
                    .child(div().pt_2().text_xs().text_color(theme.muted_foreground).child(format!("{} CHANGED", detail.files.len())))
                    .children(files)
                    .into_any_element()
            }
            Details::Compare { from, to, files } => {
                let source = DiffSource::Range { from: from.clone(), to: to.clone() };
                let rows: Vec<AnyElement> = files.iter().map(|file| self.render_file(file, source.clone(), cx)).collect();
                let theme = cx.theme();
                body.child(div().font_weight(gpui_kit::FontWeight::SEMIBOLD).child(format!("Comparing {} → {}", short_oid(from), short_oid(to))))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(format!("{} files differ", files.len())))
                    .children(rows)
                    .into_any_element()
            }
        }
    }
}

/// Context menu item; its action is run through `dispatch`.
fn graph_menu(ctx: &Ctx, entries: Vec<MenuEntry>, mut menu: PopupMenu) -> PopupMenu {
    for entry in entries {
        menu = match entry {
            MenuEntry::Separator => menu.separator(),
            MenuEntry::Item { label, disabled, action, .. } => {
                let ctx = ctx.clone();
                menu.item(PopupMenuItem::new(label).disabled(disabled).on_click(move |_, window, cx| dispatch(&ctx, action.clone(), window, cx)))
            }
        };
    }
    menu
}

/// Opens the diff of one file of a commit (or range).
fn open_file_diff(workbench: &gpui_kit::WeakEntity<oncode_ui::Workbench>, root: String, file: &CommitFile, source: DiffSource, window: &mut Window, cx: &mut App) {
    let request = DiffRequest { root, path: file.path.clone(), orig_path: file.orig_path.clone(), source, preview: true };
    let _ = workbench.update(cx, |workbench, cx| open_diff(workbench, request, window, cx));
}

/// Runs a menu action; "Compare with HEAD" is handled here because it needs the Graph tab.
fn dispatch(ctx: &Ctx, action: GitAction, window: &mut Window, cx: &mut App) {
    if let GitAction::CompareWithHead(oid) = &action {
        let head = repo_snapshot(&ctx.root, cx).and_then(|s| s.head_oid);
        if let (Some(head), Some(tab)) = (head, crate::active_tab(window, cx)) {
            let oid = oid.clone();
            tab.update(cx, |tab, cx| {
                tab.selected = vec![oid, head];
                tab.load_details(cx);
                cx.notify();
            });
        }
        return;
    }
    run_action(ctx, action, window, cx);
}

fn paint_row(row: &GraphRow, bounds: Bounds<gpui_kit::Pixels>, colors: &[Hsla], window: &mut Window) {
    let origin = bounds.origin;
    let at = |x: f32, y: f32| point(origin.x + px(x), origin.y + px(y));
    for edge in &row.edges {
        let mut builder = PathBuilder::stroke(px(1.5));
        for segment in edge_segments(edge, ROW_HEIGHT) {
            match segment {
                Segment::MoveTo(x, y) => builder.move_to(at(x, y)),
                Segment::LineTo(x, y) => builder.line_to(at(x, y)),
                Segment::QuadTo { ctrl, to } => builder.curve_to(at(to.0, to.1), at(ctrl.0, ctrl.1)),
            }
        }
        if let Ok(path) = builder.build() {
            window.paint_path(path, colors[lane_color_index(edge.color as i32)]);
        }
    }
    let center = at(lane_x(row.column), ROW_HEIGHT / 2.);
    let radius = px(DOT_RADIUS);
    let dot = Bounds::new(point(center.x - radius, center.y - radius), size(radius * 2., radius * 2.));
    let color = colors[lane_color_index(row.color as i32)];
    if row.commit.parents.len() > 1 {
        // Merge commit: an empty circle.
        window.paint_quad(fill(dot, gpui_kit::transparent_black()).corner_radii(radius).border_widths(px(1.5)).border_color(color));
    } else {
        window.paint_quad(fill(dot, color).corner_radii(radius));
    }
}

impl Render for GraphTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let count = self.rows.len();
        let status: SharedString = match (&self.error, self.loading, count) {
            (Some(error), _, _) => error.clone().into(),
            (None, true, 0) => "Loading history…".into(),
            (None, false, 0) => "No commits match.".into(),
            _ => format!("{count}{} commits", if self.done { "" } else { "+" }).into(),
        };
        let list = uniform_list("graph-list", count, cx.processor(Self::render_rows)).track_scroll(&self.scroll).size_full();
        let middle = v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .w_full()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .when_some(self.path.clone(), |el, path| el.child(div().flex_none().text_sm().child(format!("History: {path}"))))
                    .child(div().flex_1().child(Input::new(&self.search).small().prefix(Icon::new(Lucide::Search).xsmall())))
                    .child(div().flex_none().text_xs().text_color(theme.muted_foreground).child(status)),
            )
            .child(div().flex_1().min_h_0().w_full().child(list));
        let refs = if self.path.is_none() { Some(self.render_refs(cx)) } else { None };
        let details = self.render_details(cx);
        let mut group = h_resizable("graph-layout");
        if let Some(refs) = refs {
            group = group.child(resizable_panel().size(px(220.)).child(refs));
        }
        div()
            .id("graph-tab")
            .size_full()
            .track_focus(&self.focus_handle)
            .bg(cx.theme().background)
            .on_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, _, cx| match event.keystroke.key.as_str() {
                "down" | "j" => this.move_selection(1, cx),
                "up" | "k" => this.move_selection(-1, cx),
                _ => {}
            }))
            .child(group.child(resizable_panel().child(middle)).child(resizable_panel().size(px(340.)).child(details)))
    }
}

/// The commit message body without the subject line (already shown as the title); `None` if empty.
pub fn message_body(subject: &str, body: &str) -> Option<String> {
    let trimmed = body.trim();
    let rest = trimmed.strip_prefix(subject.trim()).map(str::trim).unwrap_or(trimmed);
    (!rest.is_empty()).then(|| rest.to_string())
}

#[cfg(test)]
mod tests {
    use super::message_body;

    #[test]
    fn body_without_repeated_subject() {
        assert_eq!(message_body("Title", "Title"), None);
        assert_eq!(message_body("Title", "Title\n\nLong explanation"), Some("Long explanation".to_string()));
        assert_eq!(message_body("Title", "Body only"), Some("Body only".to_string()));
        assert_eq!(message_body("Title", "  "), None);
    }

    /// Drawing the visible rows costs the same however long the loaded history is (the list used to be
    /// copied for every visible row, so scrolling far down a big repository froze the view).
    #[gpui_kit::test]
    async fn drawing_rows_does_not_depend_on_the_length_of_the_history(cx: &mut gpui_kit::TestAppContext) {
        use std::time::{Duration, Instant};

        use gpui_kit::test::TestWindowExt as _;
        use oncode_ui::testing::TestApp;
        use oncode_ui::ViewTab;

        use super::GraphTab;

        let app = TestApp::new(cx);
        cx.update(crate::init);
        let project = app.git_project(&[("README.md", "# Project\n")]);
        let window = app.open(Some(&project), cx).await;
        let workbench = window.workbench(cx);
        window.wait_until(cx, "graph store", |_, cx| crate::graph_store(workbench.read(cx)).is_some()).await;
        assert!(window.run_command(cx, "git.showGraph"));
        cx.run_until_parked();
        let tab = cx.update(|cx| workbench.read(cx).active_content().and_then(|c| c.as_any().downcast_ref::<ViewTab<GraphTab>>().map(|t| t.0.clone())).expect("graph tab"));
        window.wait_until(cx, "history loaded", |_, cx| !tab.read(cx).is_loading() && !tab.read(cx).rows().is_empty()).await;

        // The time to draw a few frames with `count` more rows loaded below the visible ones.
        let draw_with = |count: usize, cx: &mut gpui_kit::TestAppContext| {
            tab.update(cx, |tab, _| {
                tab.rows.truncate(1);
                tab.rows.extend(crate::testing::linear_rows(1, count));
                tab.done = true;
            });
            let started = Instant::now();
            for _ in 0..5 {
                tab.update(cx, |_, cx| cx.notify());
                window.update(cx, |window, cx| window.render_frame(cx));
            }
            started.elapsed()
        };
        draw_with(50, cx);
        let short = draw_with(50, cx);
        let long = draw_with(30_000, cx);
        assert!(long < short * 4 + Duration::from_millis(300), "50 rows: {short:?}, 30000 rows: {long:?}");
    }
}
