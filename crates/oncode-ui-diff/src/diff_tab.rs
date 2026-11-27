//! Diff tab: two versions of a file (side by side or unified) on top of the `Diff` component,
//! with stage/unstage/discard per change and per selected line.
//!
//! The displayed patch is built here from the two contents (see `lines.rs`), so the changes
//! on screen are exactly the ones that get translated into `SpliceOp`.

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::diff::{Diff, DiffAnnotation, DiffEvent, DiffFile, DiffLinePosition, DiffLineRange, DiffMode, DiffSide, DiffState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Disableable as _, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, App, AppContext as _, ClickEvent, Context, ElementId, Entity, FocusHandle, Focusable, Global, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _,
    Render, SharedString, Styled as _, Subscription, Task, WeakEntity, Window,
};
use oncode_core::api::git;
use oncode_core::AppError;
use oncode_git::{DiffContents, DiffSource, SpliceOp, SpliceTarget};
use oncode_ui::editor::{buffers, language_for_path, open_file, OpenFile};
use oncode_ui::overlay::{self, Confirm};
use oncode_ui::{repos, spawn_core, Env, Lucide, ReposEvent, TabContent, TabData, TabKind, ViewTab, WorkbenchEvent};
use serde_json::{json, Value};

use crate::lines::{Direction, LineDiff, LineSelection, Side};
use crate::util::{base_name, repo_file};

/// File name inside the generated patch; the real path is shown in the toolbar.
pub const PATCH_PATH: &str = "file";
/// Above this number of lines the patch no longer contains the whole file.
const FULL_CONTEXT_MAX_LINES: u32 = 40_000;
const LIMITED_CONTEXT: u32 = 200;

/// Parameters of a diff tab.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffParams {
    pub root: String,
    pub path: String,
    pub orig_path: Option<String>,
    pub source: DiffSource,
}

fn source_key(source: &DiffSource) -> String {
    match source {
        DiffSource::Worktree => "worktree".into(),
        DiffSource::Staged => "staged".into(),
        DiffSource::Commit { oid, parent } => format!("commit:{oid}:{}", parent.as_deref().unwrap_or_default()),
        DiffSource::Range { from, to } => format!("range:{from}:{to}"),
        DiffSource::Stash { name } => format!("stash:{name}"),
    }
}

fn short(rev: &str) -> &str {
    rev.get(..8).unwrap_or(rev)
}

pub fn source_label(source: &DiffSource) -> String {
    match source {
        DiffSource::Worktree => "Working Tree".into(),
        DiffSource::Staged => "Staged".into(),
        DiffSource::Commit { oid, .. } => short(oid).to_string(),
        DiffSource::Range { from, to } => format!("{}..{}", short(from), short(to)),
        DiffSource::Stash { name } => name.clone(),
    }
}

fn source_json(source: &DiffSource) -> Value {
    match source {
        DiffSource::Worktree => json!({ "kind": "worktree" }),
        DiffSource::Staged => json!({ "kind": "staged" }),
        DiffSource::Commit { oid, parent } => json!({ "kind": "commit", "oid": oid, "parent": parent }),
        DiffSource::Range { from, to } => json!({ "kind": "range", "from": from, "to": to }),
        DiffSource::Stash { name } => json!({ "kind": "stash", "name": name }),
    }
}

impl DiffParams {
    pub fn tab(&self) -> TabData {
        let label = source_label(&self.source);
        TabData::new("diff", format!("{} ({label})", base_name(&self.path)), json!({ "root": self.root, "path": self.path, "origPath": self.orig_path, "source": source_json(&self.source) }))
            .with_id(format!("diff:{}:{}:{}", self.root, source_key(&self.source), self.path))
            .with_description(format!("{} — {label}", self.path))
    }

    pub fn from_tab(tab: &TabData) -> Option<Self> {
        Some(Self {
            root: tab.param_str("root")?.to_string(),
            path: tab.param_str("path")?.to_string(),
            orig_path: tab.param_str("origPath").map(String::from),
            source: serde_json::from_value(tab.params.get("source")?.clone()).ok()?,
        })
    }
}

/// Actions available for a diff source: (label, target).
fn actions_for(source: &DiffSource) -> &'static [(&'static str, SpliceTarget)] {
    match source {
        DiffSource::Worktree => &[("Stage", SpliceTarget::Stage), ("Discard", SpliceTarget::Discard)],
        DiffSource::Staged => &[("Unstage", SpliceTarget::Unstage)],
        _ => &[],
    }
}

fn target_key(target: SpliceTarget) -> &'static str {
    match target {
        SpliceTarget::Stage => "stage",
        SpliceTarget::Unstage => "unstage",
        SpliceTarget::Discard => "discard",
    }
}

fn target_title(target: SpliceTarget) -> &'static str {
    match target {
        SpliceTarget::Stage => "Stage this change",
        SpliceTarget::Unstage => "Unstage this change",
        SpliceTarget::Discard => "Discard this change from the working tree",
    }
}

fn target_icon(target: SpliceTarget) -> Lucide {
    match target {
        SpliceTarget::Stage => Lucide::Plus,
        SpliceTarget::Unstage => Lucide::Minus,
        SpliceTarget::Discard => Lucide::Undo2,
    }
}

/// The layout chosen last; applies to diff tabs opened afterwards.
struct PreferredMode(DiffMode);

impl Global for PreferredMode {}

/// Result of processing `DiffContents` on a background thread.
struct Prepared {
    contents: DiffContents,
    model: Arc<LineDiff>,
    files: Vec<DiffFile>,
    /// Reason the contents cannot be compared as text.
    problem: Option<String>,
}

fn prepare(contents: DiffContents, language: &'static str) -> Prepared {
    let problem = [&contents.a, &contents.b].iter().find_map(|side| {
        if side.binary {
            Some("This is a binary file; its content cannot be compared as text.".to_string())
        } else if side.too_large {
            Some("This file is too large to compare here.".to_string())
        } else {
            None
        }
    });
    if problem.is_some() {
        return Prepared { contents, model: Arc::new(LineDiff::compute("", "")), files: Vec::new(), problem };
    }
    let (a, b) = (contents.a.text.as_deref().unwrap_or_default(), contents.b.text.as_deref().unwrap_or_default());
    let model = LineDiff::compute(a, b);
    let (files, problem) = if model.chunks.is_empty() {
        (vec![DiffFile::unchanged(PATCH_PATH, b)], None)
    } else {
        let context = (model.a_lines.max(model.b_lines) > FULL_CONTEXT_MAX_LINES).then_some(LIMITED_CONTEXT);
        match DiffFile::parse(&model.unified_patch(a, b, PATCH_PATH, context)) {
            Ok(files) => (files, None),
            Err(error) => (Vec::new(), Some(format!("This diff cannot be displayed: {}", error.message()))),
        }
    };
    let files = files.into_iter().map(|file| file.with_language(language)).collect();
    Prepared { contents, model: Arc::new(model), files, problem }
}

struct Loaded {
    contents: DiffContents,
    model: Arc<LineDiff>,
    problem: Option<String>,
}

pub struct DiffView {
    env: Env,
    params: DiffParams,
    state: Entity<DiffState>,
    loaded: Option<Loaded>,
    error: Option<String>,
    /// An action (stage/unstage/discard) is in progress.
    busy: bool,
    /// Side B line to show again after the diff is reloaded because of an action.
    restore_line: Option<u32>,
    focus_handle: FocusHandle,
    _load: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl DiffView {
    pub fn new(params: DiffParams, env: Env, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mode = cx.try_global::<PreferredMode>().map_or(DiffMode::Split, |mode| mode.0);
        let state = cx.new(|cx| DiffState::new([], cx).with_mode(mode));
        let mut subscriptions = vec![cx.subscribe(&state, |_, _, event: &DiffEvent, cx| {
            if matches!(event, DiffEvent::SelectionChanged(_) | DiffEvent::SelectionEnded(_)) {
                cx.notify();
            }
        })];
        // Commit, revision range, and stash contents don't change; the others follow the repository state.
        if matches!(params.source, DiffSource::Worktree | DiffSource::Staged) {
            subscriptions.push(cx.subscribe_in(&repos(cx), window, |this, _, event: &ReposEvent, window, cx| {
                if matches!(event, ReposEvent::Changed(root) if *root == this.params.root) {
                    this.load(window, cx);
                }
            }));
        }
        if params.source == DiffSource::Worktree {
            if let Some(workbench) = env.workbench.upgrade() {
                subscriptions.push(cx.subscribe_in(&workbench, window, |this, _, event: &WorkbenchEvent, window, cx| {
                    let file = this.absolute_path().replace('\\', "/");
                    let touched = match event {
                        WorkbenchEvent::FileSaved(path) => path.replace('\\', "/") == file,
                        WorkbenchEvent::FsChanged { paths, overflow } => *overflow || paths.iter().any(|path| path.replace('\\', "/") == file),
                        _ => false,
                    };
                    if touched {
                        this.load(window, cx);
                    }
                }));
            }
        }
        let mut this = Self {
            env,
            params,
            state,
            loaded: None,
            error: None,
            busy: false,
            restore_line: None,
            focus_handle: cx.focus_handle(),
            _load: Task::ready(()),
            _subscriptions: subscriptions,
        };
        this.load(window, cx);
        this
    }

    pub fn params(&self) -> &DiffParams {
        &self.params
    }

    pub fn state(&self) -> &Entity<DiffState> {
        &self.state
    }

    /// The diff has been loaded (successfully or not).
    pub fn is_loaded(&self) -> bool {
        self.loaded.is_some() || self.error.is_some()
    }

    /// Number of changes displayed.
    pub fn change_count(&self) -> usize {
        self.loaded.as_ref().map_or(0, |loaded| loaded.model.chunks.len())
    }

    pub fn mode(&self, cx: &App) -> DiffMode {
        self.state.read(cx).mode()
    }

    fn absolute_path(&self) -> String {
        repo_file(&self.params.root, &self.params.path)
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (core, params) = (self.env.core.clone(), self.params.clone());
        let language = language_for_path(&self.params.path).highlighter;
        let task = spawn_core(cx, async move { git::git_diff(&core, params.root, params.path, params.orig_path, params.source).await });
        self._load = cx.spawn_in(window, async move |this, cx| {
            let prepared = match task.await {
                Ok(contents) => {
                    // Identical contents are not reinstalled, so scroll position and selection are kept.
                    let unchanged = this
                        .read_with(cx, |this, _| {
                            this.loaded.as_ref().is_some_and(|loaded| loaded.contents.a.fingerprint == contents.a.fingerprint && loaded.contents.b.fingerprint == contents.b.fingerprint)
                        })
                        .unwrap_or(true);
                    if unchanged {
                        let _ = this.update(cx, |this, cx| {
                            this.error = None;
                            this.restore_line = None;
                            cx.notify();
                        });
                        return;
                    }
                    Ok(cx.background_executor().spawn(async move { prepare(contents, language) }).await)
                }
                Err(error) => Err(error),
            };
            let _ = this.update(cx, |this, cx| this.install(prepared, cx));
        });
    }

    fn install(&mut self, prepared: Result<Prepared, AppError>, cx: &mut Context<Self>) {
        match prepared {
            Ok(prepared) => {
                let restore = self.restore_line.take().filter(|_| self.loaded.is_some());
                let lines = prepared.model.b_lines;
                self.state.update(cx, |state, cx| {
                    state.set_files(prepared.files, cx);
                    if let Some(line) = restore.filter(|_| lines > 0) {
                        state.scroll_to_line(DiffLinePosition::new(PATCH_PATH, DiffSide::Modified, line.clamp(1, lines) as usize), cx);
                    }
                });
                self.loaded = Some(Loaded { contents: prepared.contents, model: prepared.model, problem: prepared.problem });
                self.error = None;
            }
            Err(error) => {
                self.loaded = None;
                self.error = Some(if error.code == "notARepo" { "The repository is no longer open.".to_string() } else { error.message });
            }
        }
        cx.notify();
    }

    // ----- Actions -----

    fn direction(target: SpliceTarget) -> Direction {
        match target {
            SpliceTarget::Stage => Direction::Forward,
            SpliceTarget::Unstage | SpliceTarget::Discard => Direction::Reverse,
        }
    }

    /// Stage/unstage/discard a single change (index follows the on-screen order).
    pub fn apply_change(&mut self, index: usize, target: SpliceTarget, window: &mut Window, cx: &mut Context<Self>) {
        let Some(loaded) = &self.loaded else { return };
        let Some(chunk) = loaded.model.chunks.get(index) else { return };
        let ops = vec![loaded.model.chunk_op(chunk)];
        let anchor = chunk.b_from;
        self.apply(target, ops, anchor, window, cx);
    }

    /// The currently selected lines, in the form `LineDiff` understands.
    fn selection(&self, cx: &App) -> Option<LineSelection> {
        let model = &self.loaded.as_ref()?.model;
        let range = self.state.read(cx).selected_lines()?;
        let (start, end) = (range.start() as u32, range.end() as u32);
        Some(match (range.side(), range.end_side()) {
            (DiffSide::Modified, DiffSide::Modified) => LineSelection::Modified(start.min(end) - 1..start.max(end)),
            (DiffSide::Original, DiffSide::Original) => LineSelection::Original(start.min(end) - 1..start.max(end)),
            (from, to) => {
                let side = |side: DiffSide| if side == DiffSide::Original { Side::Original } else { Side::Modified };
                let (first, last) = (model.row_of(side(from), start - 1), model.row_of(side(to), end - 1));
                LineSelection::Rows(first.min(last), first.max(last))
            }
        })
    }

    /// Stage/unstage/discard only the selected lines.
    pub fn apply_selection(&mut self, target: SpliceTarget, window: &mut Window, cx: &mut Context<Self>) {
        let Some(loaded) = &self.loaded else { return };
        let Some(selection) = self.selection(cx) else {
            overlay::toast_info("Select lines first: click a line, or drag across the line numbers.", window, cx);
            return;
        };
        let ops = loaded.model.selection_ops(&selection, Self::direction(target));
        let anchor = self.state.read(cx).selected_lines().map_or(0, |range| range.start().min(range.end()) as u32).saturating_sub(1);
        self.apply(target, ops, anchor, window, cx);
    }

    fn apply(&mut self, target: SpliceTarget, ops: Vec<SpliceOp>, anchor: u32, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(loaded) = &self.loaded else { return };
        if loaded.contents.lfs {
            overlay::toast_info("This file is stored with Git LFS: stage or discard it as a whole from Source Control.", window, cx);
            return;
        }
        if ops.is_empty() {
            overlay::toast_info("The selection does not contain any changes.", window, cx);
            return;
        }
        // The diff shows saved content; unsaved changes must be saved first.
        if self.params.source == DiffSource::Worktree {
            let dirty = self.env.workbench.upgrade().and_then(|workbench| buffers(workbench.read(cx))).and_then(|buffers| buffers.read(cx).get(&self.absolute_path())).filter(|buffer| buffer.read(cx).is_dirty());
            if let Some(buffer) = dirty {
                let save = buffer.update(cx, |buffer, cx| buffer.save(window, cx));
                cx.spawn_in(window, async move |this, cx| {
                    let saved = save.await;
                    let _ = this.update_in(cx, |this, window, cx| {
                        if saved {
                            overlay::toast_warning("The file had unsaved changes. They were saved and the diff was refreshed; try again.", window, cx);
                            this.load(window, cx);
                        }
                    });
                })
                .detach();
                return;
            }
        }
        let confirmed = (target == SpliceTarget::Discard).then(|| {
            overlay::confirm(
                Confirm::new("Discard this change?", format!("The selected change in {} is removed from the working tree. This cannot be undone.", self.params.path), "Discard").danger(),
                window,
                cx,
            )
        });
        let (core, root, path) = (self.env.core.clone(), self.params.root.clone(), self.params.path.clone());
        let (fingerprint_a, fingerprint_b) = (loaded.contents.a.fingerprint.clone(), loaded.contents.b.fingerprint.clone());
        self.busy = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            if let Some(confirmed) = confirmed {
                if !confirmed.await {
                    let _ = this.update(cx, |this, cx| {
                        this.busy = false;
                        cx.notify();
                    });
                    return;
                }
            }
            let Ok(task) = this.update(cx, |_, cx| spawn_core(cx, async move { git::git_splice(&core, root, target, path, ops, fingerprint_a, fingerprint_b).await })) else { return };
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(()) => {}
                    Err(error) if error.code == "stale" => {
                        overlay::toast_warning("The file changed since this diff was shown. The diff was refreshed; try again.", window, cx);
                    }
                    Err(error) => overlay::show_error(&error, window, cx),
                }
                this.restore_line = Some(anchor.max(1));
                this.load(window, cx);
            });
        })
        .detach();
    }

    /// Stage/unstage/discard the whole file.
    fn apply_file(&mut self, target: SpliceTarget, window: &mut Window, cx: &mut Context<Self>) {
        let (core, root, path) = (self.env.core.clone(), self.params.root.clone(), self.params.path.clone());
        let mut paths = vec![path.clone()];
        let report = |task: Task<Result<(), AppError>>, window: &mut Window, cx: &mut Context<Self>| {
            cx.spawn_in(window, async move |this, cx| {
                if let Err(error) = task.await {
                    let _ = this.update_in(cx, |_, window, cx| overlay::show_error(&error, window, cx));
                }
            })
            .detach();
        };
        match target {
            SpliceTarget::Stage => report(spawn_core(cx, async move { git::git_stage(&core, root, paths).await }), window, cx),
            SpliceTarget::Unstage => {
                paths.extend(self.params.orig_path.clone());
                report(spawn_core(cx, async move { git::git_unstage(&core, root, paths).await }), window, cx)
            }
            SpliceTarget::Discard => {
                let untracked = repos(cx).read(cx).snapshot(&root).is_some_and(|snapshot| snapshot.untracked.contains(&path));
                let message = if untracked { "1 untracked file will be moved to the trash." } else { "Changes to 1 tracked file will be lost permanently." };
                let confirmed = overlay::confirm(Confirm::new(format!("Discard changes in {path}?"), message, "Discard").danger(), window, cx);
                cx.spawn_in(window, async move |this, cx| {
                    if !confirmed.await {
                        return;
                    }
                    let (tracked, untracked) = if untracked { (Vec::new(), paths) } else { (paths, Vec::new()) };
                    let Ok(task) = this.update(cx, |_, cx| spawn_core(cx, async move { git::git_discard(&core, root, tracked, untracked).await })) else { return };
                    if let Err(error) = task.await {
                        let _ = this.update_in(cx, |_, window, cx| overlay::show_error(&error, window, cx));
                    }
                })
                .detach();
            }
        }
    }

    pub fn set_mode(&mut self, mode: DiffMode, cx: &mut Context<Self>) {
        cx.set_global(PreferredMode(mode));
        self.state.update(cx, |state, cx| state.set_mode(mode, cx));
        cx.notify();
    }

    fn toggle_mode(&mut self, cx: &mut Context<Self>) {
        let next = if self.mode(cx) == DiffMode::Split { DiffMode::Unified } else { DiffMode::Split };
        self.set_mode(next, cx);
    }

    fn navigate(&mut self, forward: bool, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| if forward { state.next_change(cx) } else { state.previous_change(cx) });
    }

    /// Opens the working file at the first changed line.
    fn open_working_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workbench) = self.env.workbench.upgrade() else { return };
        let line = self.loaded.as_ref().and_then(|loaded| loaded.model.first_changed_line());
        let mut options = OpenFile::new(self.absolute_path());
        if let Some(line) = line {
            options = options.at(line, 0, None);
        }
        // Deferred: the click handler may run while `Workbench` is being updated.
        window.defer(cx, move |window, cx| workbench.update(cx, |workbench, cx| open_file(workbench, options, window, cx)));
    }

    /// Clicking a line selects that line; Shift+click extends the selection on the same side.
    fn select_line(&mut self, position: &DiffLinePosition, extend: bool, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            let start = state.selected_lines().filter(|range| extend && range.side() == position.side() && range.end_side() == position.side()).map(|range| range.start().min(range.end()));
            let (from, to) = match start {
                Some(start) => (start.min(position.line()), start.max(position.line())),
                None => (position.line(), position.line()),
            };
            state.set_selected_lines(Some(DiffLineRange::new(position.path().clone(), position.side(), from, to)), cx);
        });
        cx.notify();
    }

    // ----- View -----

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let params = &self.params;
        let loaded = self.loaded.as_ref();
        let lfs = loaded.is_some_and(|loaded| loaded.contents.lfs);
        let actions: &[(&str, SpliceTarget)] = if lfs { &[] } else { actions_for(&params.source) };
        let problem = loaded.is_some_and(|loaded| loaded.problem.is_some());
        let changes = self.change_count();
        let comparable = loaded.is_some() && !problem;
        let split = self.mode(cx) == DiffMode::Split;
        let title = match &params.orig_path {
            Some(orig) if *orig != params.path => format!("{orig} → {}", params.path),
            _ => params.path.clone(),
        };
        let can_open = loaded.is_some_and(|loaded| loaded.contents.b.exists) && !matches!(params.source, DiffSource::Stash { .. } | DiffSource::Range { .. });

        h_flex()
            .id("diff-toolbar")
            .w_full()
            .flex_none()
            .px_3()
            .py_1()
            .gap_2()
            .text_sm()
            .border_b_1()
            .border_color(theme.border)
            .child(div().min_w_0().overflow_hidden().whitespace_nowrap().child(SharedString::from(title)))
            .child(div().flex_none().text_color(theme.muted_foreground).child(SharedString::from(source_label(&params.source))))
            .when(lfs, |el| el.child(div().id("diff-lfs").flex_none().text_xs().text_color(theme.warning).child("Git LFS file: stage or discard it as a whole")))
            .when(comparable, |el| {
                let text = match changes {
                    0 => "no changes".to_string(),
                    1 => "1 change".to_string(),
                    count => format!("{count} changes"),
                };
                el.child(div().id("diff-change-count").flex_none().text_color(theme.muted_foreground).child(SharedString::from(text)))
            })
            .child(div().flex_1())
            .when(comparable && changes > 0, |el| {
                el.children(actions.iter().map(|(label, target)| {
                    let target = *target;
                    Button::new(SharedString::from(format!("diff-selected-{}", target_key(target))))
                        .outline()
                        .xsmall()
                        .icon(Icon::new(target_icon(target)))
                        .label(format!("{label} Selected Lines"))
                        .tooltip(format!("{label} only the selected lines"))
                        .disabled(self.busy)
                        .on_click(cx.listener(move |this, _, window, cx| this.apply_selection(target, window, cx)))
                }))
            })
            .children(actions.iter().map(|(label, target)| {
                let target = *target;
                Button::new(SharedString::from(format!("diff-file-{}", target_key(target))))
                    .outline()
                    .xsmall()
                    .label(format!("{label} File"))
                    .on_click(cx.listener(move |this, _, window, cx| this.apply_file(target, window, cx)))
            }))
            .child(
                Button::new("diff-previous")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(Lucide::ArrowUp))
                    .tooltip("Previous change (Alt+↑)")
                    .disabled(!comparable || changes == 0)
                    .on_click(cx.listener(|this, _, _, cx| this.navigate(false, cx))),
            )
            .child(
                Button::new("diff-next")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(Lucide::ArrowDown))
                    .tooltip("Next change (Alt+↓)")
                    .disabled(!comparable || changes == 0)
                    .on_click(cx.listener(|this, _, _, cx| this.navigate(true, cx))),
            )
            .child(
                Button::new("diff-toggle-layout")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(if split { Lucide::Rows2 } else { Lucide::Columns2 }))
                    .tooltip(if split { "Switch to unified view" } else { "Switch to side-by-side view" })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_mode(cx))),
            )
            .when(can_open, |el| {
                el.child(Button::new("diff-open-file").ghost().xsmall().icon(Icon::new(Lucide::FileText)).tooltip("Open file").on_click(cx.listener(|this, _, window, cx| this.open_working_file(window, cx))))
            })
    }

    fn render_message(&self, message: impl Into<SharedString>, detail: Option<String>, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        v_flex()
            .id("diff-message")
            .size_full()
            .track_focus(&self.focus_handle)
            .items_center()
            .justify_center()
            .gap_2()
            .text_color(cx.theme().muted_foreground)
            .child(div().child(message.into()))
            .when_some(detail, |el, detail| el.child(div().text_xs().child(SharedString::from(detail))))
            .into_any_element()
    }

    fn render_body(&self, loaded: &Loaded, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme();
        let (a, b) = (&loaded.contents.a, &loaded.contents.b);
        if let Some(problem) = &loaded.problem {
            let side = |side: &oncode_git::DiffSide| if side.exists { format!("{}: {} bytes", side.label, side.size) } else { format!("{}: absent", side.label) };
            return self.render_message(problem.clone(), Some(format!("{} · {}", side(a), side(b))), cx);
        }
        let model = loaded.model.clone();
        let total = model.chunks.len();
        // A Git LFS file is shown with its real content, but its index holds the pointer: no per-change actions.
        let actions: &[(&str, SpliceTarget)] = if loaded.contents.lfs { &[] } else { actions_for(&self.params.source) };
        let split = self.mode(cx) == DiffMode::Split;
        let busy = self.busy;

        // Action bar below the last line of each change.
        let annotations: Vec<DiffAnnotation> = if actions.is_empty() {
            Vec::new()
        } else {
            model
                .chunks
                .iter()
                .enumerate()
                .map(|(index, chunk)| {
                    let (side, line) = model.chunk_anchor(chunk);
                    let side = if side == Side::Original { DiffSide::Original } else { DiffSide::Modified };
                    DiffAnnotation::line(ElementId::NamedInteger("diff-change".into(), index as u64), DiffLinePosition::new(PATCH_PATH, side, line as usize))
                })
                .collect()
        };
        let weak: WeakEntity<Self> = cx.weak_entity();
        let click_target = weak.clone();
        let diff = Diff::new(&self.state)
            .header_visible(false)
            .annotations(annotations)
            .render_annotation(move |annotation, _, cx| {
                let index = match annotation.id() {
                    ElementId::NamedInteger(_, index) => *index as usize,
                    _ => 0,
                };
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().text_xs().text_color(cx.theme().muted_foreground).child(SharedString::from(format!("Change {} of {total}", index + 1))))
                    .children(actions.iter().map(|(label, target)| {
                        let (target, weak) = (*target, weak.clone());
                        let button = Button::new(SharedString::from(format!("diff-{}-{index}", target_key(target)))).xsmall().label(*label).tooltip(target_title(target)).disabled(busy);
                        let button = if target == SpliceTarget::Discard { button.danger() } else { button.outline() };
                        button.on_click(move |_, window, cx| {
                            let _ = weak.update(cx, |this, cx| this.apply_change(index, target, window, cx));
                        })
                    }))
            })
            .on_line_click(move |position: &DiffLinePosition, event: &ClickEvent, _, cx| {
                let extend = event.modifiers().shift;
                let _ = click_target.update(cx, |this, cx| this.select_line(position, extend, cx));
            })
            .size_full();

        let identical = total == 0 && a.exists == b.exists;
        v_flex()
            .size_full()
            .when(split && total > 0, |el| {
                let label = |text: String| div().flex_1().min_w_0().px_3().py_0p5().overflow_hidden().whitespace_nowrap().child(SharedString::from(text));
                el.child(
                    h_flex()
                        .id("diff-labels")
                        .w_full()
                        .flex_none()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .border_b_1()
                        .border_color(theme.border)
                        .child(label(if a.exists { a.label.clone() } else { format!("{} — file does not exist", a.label) }))
                        .child(label(if b.exists { b.label.clone() } else { format!("{} — file deleted", b.label) })),
                )
            })
            .when(identical, |el| {
                el.child(div().id("diff-identical").w_full().flex_none().px_3().py_1().text_xs().text_color(theme.muted_foreground).border_b_1().border_color(theme.border).child("Both versions are identical."))
            })
            .child(div().flex_1().min_h_0().w_full().child(diff))
            .into_any_element()
    }
}

impl Focusable for DiffView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        if self.loaded.as_ref().is_some_and(|loaded| loaded.problem.is_none()) {
            self.state.focus_handle(cx)
        } else {
            self.focus_handle.clone()
        }
    }
}

impl Render for DiffView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match (&self.error, &self.loaded) {
            (Some(error), _) => self.render_message(error.clone(), None, cx),
            (None, None) => v_flex().size_full().track_focus(&self.focus_handle).items_center().justify_center().child(Spinner::new()).into_any_element(),
            (None, Some(loaded)) => self.render_body(loaded, cx),
        };
        v_flex()
            .id("diff-tab")
            .size_full()
            .bg(cx.theme().background)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                let keystroke = &event.keystroke;
                if keystroke.modifiers.alt && matches!(keystroke.key.as_str(), "down" | "up") {
                    this.navigate(keystroke.key == "down", cx);
                    cx.stop_propagation();
                }
            }))
            .child(self.render_toolbar(cx))
            .child(div().flex_1().min_h_0().w_full().child(body))
    }
}

pub(crate) struct DiffKind;

impl TabKind for DiffKind {
    fn build(&self, tab: &TabData, env: &Env, window: &mut Window, cx: &mut App) -> Box<dyn TabContent> {
        let params = DiffParams::from_tab(tab).unwrap_or(DiffParams { root: String::new(), path: tab.title.clone(), orig_path: None, source: DiffSource::Worktree });
        let env = env.clone();
        Box::new(ViewTab(cx.new(|cx| DiffView::new(params, env, window, cx))))
    }

    fn icon(&self, _tab: &TabData) -> Option<Lucide> {
        Some(Lucide::GitCompare)
    }
}
