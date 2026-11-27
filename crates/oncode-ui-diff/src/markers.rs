//! Change markers in the file editor gutter (compared against the index content) and blame.
//!
//! Markers are recomputed (with a delay) every time the file is edited or the repository state
//! changes. Blame for the cursor line is shown at the end of that line (inline blame, like GitLens)
//! through the editor's end-of-line hint, and also in the status bar.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::input::{EditorState, GutterDecoration, GutterDecorationCollection};
use gpui_kit::component::{h_flex, ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::{div, App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, StatefulInteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _, Subscription, Task, TestSupportExt as _, WeakEntity, Window};
use oncode_core::api::git as git_api;
use oncode_git::{BlameRange, DiffSource};
use oncode_ui::editor::{active_file, active_text_file, buffers, Buffer, BufferEvent, Buffers, BuffersEvent};
use oncode_ui::{overlay, repos, spawn_core, Env, Lucide, ReposEvent, StatusAlign, StatusItem, Workbench, WorkbenchEvent};

use crate::gutter::{blame_at, blame_label, blame_summary, gutter_ranges, inline_blame_text, repo_for_path, ChangeKind};
use crate::lines::LineDiff;

/// Delay before markers are recomputed after an edit.
const DEBOUNCE: Duration = Duration::from_millis(250);
/// Delay before blame is reloaded after an edit (inline blame is hidden in the meantime).
const BLAME_DEBOUNCE: Duration = Duration::from_millis(600);

struct Tracked {
    buffer: WeakEntity<Buffer>,
    root: String,
    relative: String,
    /// Index content (the comparison side); `None` = new/untracked file (all lines "added").
    base: Option<Option<String>>,
    collections: Vec<(WeakEntity<EditorState>, GutterDecorationCollection)>,
    _debounce: Task<()>,
    _load: Task<()>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MarkersEvent {
    BlameChanged,
}

/// Change markers and blame for all file editors in a window.
pub struct ChangeMarkers {
    env: Env,
    buffers: Option<WeakEntity<Buffers>>,
    files: HashMap<String, Tracked>,
    /// Blame per path for files whose blame is currently shown.
    blame: HashMap<String, Vec<BlameRange>>,
    /// Files edited after their blame was loaded; their line numbers may have shifted,
    /// so their blame is not shown until reloaded.
    stale_blame: HashSet<String>,
    blame_tasks: HashMap<String, Task<()>>,
    /// The `user.email` per repository, used to write "You" in inline blame. `None` while
    /// still loading (inline blame waits so it doesn't switch from a name to "You").
    identities: HashMap<String, Option<Option<String>>>,
    /// Latest value of `editor.inlineBlame`.
    inline_blame: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<MarkersEvent> for ChangeMarkers {}

impl ChangeMarkers {
    pub fn new(env: Env, buffers: Option<Entity<oncode_ui::editor::Buffers>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = vec![cx.subscribe(&repos(cx), |this: &mut Self, _, event: &ReposEvent, cx| {
            if let ReposEvent::Changed(root) = event {
                let paths: Vec<String> = this.files.iter().filter(|(_, t)| &t.root == root).map(|(p, _)| p.clone()).collect();
                for path in paths {
                    this.reload_base(&path, cx);
                }
            }
        })];
        // `Workbench` is being built when this is called, so the buffer registry is passed in
        // from outside and settings are read from core.
        if let Some(buffers) = &buffers {
            subscriptions.push(cx.subscribe_in(buffers, window, |this: &mut Self, _, event: &BuffersEvent, _, cx| {
                if let BuffersEvent::EditorCreated { buffer, state } = event {
                    this.attach(buffer.clone(), state.clone(), cx);
                }
            }));
            // Inline blame: a hint at the end of the cursor line.
            let markers = cx.weak_entity();
            let provider: oncode_ui::editor::LineHintProvider = Rc::new(move |path, row, cx| markers.upgrade()?.read(cx).inline_hint(path, row));
            buffers.update(cx, |buffers, cx| buffers.set_line_hint_provider("git.blame", provider, cx));
        }
        if let Some(workbench) = env.workbench.upgrade() {
            subscriptions.push(cx.subscribe(&workbench, |this: &mut Self, workbench, event: &WorkbenchEvent, cx| {
                if matches!(event, WorkbenchEvent::SettingsChanged) {
                    let on = workbench.read(cx).settings().editor.inline_blame;
                    this.set_inline_blame(on, cx);
                }
            }));
        }
        let inline_blame = env.core.effective_settings(&env.window).editor.inline_blame;
        Self {
            env,
            buffers: buffers.map(|buffers| buffers.downgrade()),
            files: HashMap::new(),
            blame: HashMap::new(),
            stale_blame: HashSet::new(),
            blame_tasks: HashMap::new(),
            identities: HashMap::new(),
            inline_blame,
            _subscriptions: subscriptions,
        }
    }

    fn attach(&mut self, buffer: Entity<Buffer>, state: Entity<EditorState>, cx: &mut Context<Self>) {
        let path = buffer.read(cx).path().to_string();
        let roots = repos(cx).read(cx).roots();
        let Some((root, relative)) = repo_for_path(&path, &roots) else { return };
        let collection = state.update(cx, |state, cx| state.create_gutter_decorations_collection(Vec::new(), cx));
        if !self.identities.contains_key(&root) {
            self.load_identity(root.clone(), cx);
        }
        if !self.files.contains_key(&path) {
            let subscription = cx.subscribe(&buffer, |this: &mut Self, buffer, event: &BufferEvent, cx| {
                let path = buffer.read(cx).path().to_string();
                this.schedule(&path, cx);
                if !this.blame.contains_key(&path) {
                    return;
                }
                match event {
                    // Line numbers may shift: hide it first, reload after the delay.
                    BufferEvent::Edited => {
                        if this.stale_blame.insert(path.clone()) {
                            this.refresh_hints(cx);
                        }
                        this.load_blame(&path, BLAME_DEBOUNCE, cx);
                    }
                    BufferEvent::Loaded | BufferEvent::Saved => this.load_blame(&path, Duration::ZERO, cx),
                }
            });
            let release = cx.observe_release(&buffer, |this: &mut Self, buffer, _| {
                let path = buffer.path().to_string();
                this.files.remove(&path);
                this.blame.remove(&path);
                this.stale_blame.remove(&path);
                this.blame_tasks.remove(&path);
            });
            self._subscriptions.extend([subscription, release]);
            self.files.insert(
                path.clone(),
                Tracked { buffer: buffer.downgrade(), root, relative, base: None, collections: Vec::new(), _debounce: Task::ready(()), _load: Task::ready(()) },
            );
            self.reload_base(&path, cx);
        }
        if let Some(tracked) = self.files.get_mut(&path) {
            tracked.collections.retain(|(state, _)| state.upgrade().is_some());
            tracked.collections.push((state.downgrade(), collection));
        }
        if self.inline_blame && !self.blame.contains_key(&path) && !self.blame_tasks.contains_key(&path) {
            self.load_blame(&path, Duration::ZERO, cx);
        }
        self.schedule(&path, cx);
    }

    /// Applies `editor.inlineBlame`: turning on loads blame for all open files,
    /// turning off hides it.
    fn set_inline_blame(&mut self, on: bool, cx: &mut Context<Self>) {
        if on == self.inline_blame {
            return;
        }
        self.inline_blame = on;
        if on {
            let paths: Vec<String> = self.files.keys().filter(|path| !self.blame.contains_key(*path)).cloned().collect();
            for path in paths {
                self.load_blame(&path, Duration::ZERO, cx);
            }
        } else {
            self.blame.clear();
            self.stale_blame.clear();
            self.blame_tasks.clear();
            self.blame_changed(cx);
        }
    }

    /// Reloads this file's index content, then recomputes the markers.
    fn reload_base(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(tracked) = self.files.get_mut(path) else { return };
        let (core, root, relative) = (self.env.core.clone(), tracked.root.clone(), tracked.relative.clone());
        let task = spawn_core(cx, async move { git_api::git_diff(&core, root, relative, None, DiffSource::Worktree).await });
        let key = path.to_string();
        tracked._load = cx.spawn(async move |this, cx| {
            let Ok(contents) = task.await else { return };
            let _ = this.update(cx, |this, cx| {
                if let Some(tracked) = this.files.get_mut(&key) {
                    tracked.base = Some(if contents.a.exists { contents.a.text } else { None });
                }
                this.schedule(&key, cx);
            });
        });
    }

    fn schedule(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(tracked) = self.files.get_mut(path) else { return };
        let key = path.to_string();
        tracked._debounce = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| this.recompute(&key, cx));
        });
    }

    /// Computes the markers in the background, then applies them to all editors of this file.
    fn recompute(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(tracked) = self.files.get_mut(path) else { return };
        let Some(base) = tracked.base.clone() else { return };
        let Some(buffer) = tracked.buffer.upgrade() else { return };
        if buffer.read(cx).is_large() {
            return;
        }
        let Some(text) = buffer.read(cx).text(cx).map(|rope| rope.to_string()) else { return };
        let theme = cx.theme();
        let colors = (theme.green, theme.blue, theme.red);
        let key = path.to_string();
        let task = cx.background_spawn(async move {
            let diff = LineDiff::compute(base.as_deref().unwrap_or(""), &text);
            gutter_ranges(&diff, &text)
        });
        tracked._debounce = cx.spawn(async move |this, cx| {
            let ranges = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(tracked) = this.files.get_mut(&key) else { return };
                let decorations: Vec<GutterDecoration> = ranges
                    .into_iter()
                    .map(|(range, kind)| {
                        let color = match kind {
                            ChangeKind::Added => colors.0,
                            ChangeKind::Modified => colors.1,
                            ChangeKind::Deleted => colors.2,
                        };
                        GutterDecoration::new(range).with_color(color)
                    })
                    .collect();
                tracked.collections.retain(|(state, _)| state.upgrade().is_some());
                for (_, collection) in &tracked.collections {
                    collection.set(decorations.clone(), cx);
                }
            });
        });
    }

    /// The markers applied to a file (for tests).
    pub fn marker_ranges(&self, path: &str, cx: &App) -> Option<Vec<(std::ops::Range<usize>, ChangeKind)>> {
        let tracked = self.files.get(path)?;
        let base = tracked.base.clone()?;
        let text = tracked.buffer.upgrade()?.read(cx).text(cx)?.to_string();
        let diff = LineDiff::compute(base.as_deref().unwrap_or(""), &text);
        Some(gutter_ranges(&diff, &text))
    }

    // ----- Blame -----

    pub fn blame_for(&self, path: &str) -> Option<&[BlameRange]> {
        self.blame.get(path).map(Vec::as_slice)
    }

    /// Blame of a line (starting at 1), unless that file's blame hasn't caught up with the latest edit.
    pub fn line_blame(&self, path: &str, line: u32) -> Option<&BlameRange> {
        if self.stale_blame.contains(path) {
            return None;
        }
        blame_at(self.blame.get(path)?, line)
    }

    /// Inline blame text for line `row` (starting at 0).
    pub fn inline_hint(&self, path: &str, row: u32) -> Option<gpui_kit::SharedString> {
        let range = self.line_blame(path, row + 1)?;
        let me = match self.files.get(path).and_then(|tracked| self.identities.get(&tracked.root)) {
            Some(None) => return None,
            Some(Some(email)) => email.as_deref(),
            None => None,
        };
        Some(inline_blame_text(range, now(), me).into())
    }

    /// Turns this file's blame on or off. Returns `false` if the file is not part of a repository.
    pub fn toggle_blame(&mut self, path: &str, cx: &mut Context<Self>) -> bool {
        if !self.files.contains_key(path) {
            return false;
        }
        if self.blame.contains_key(path) || self.blame_tasks.contains_key(path) {
            self.blame.remove(path);
            self.stale_blame.remove(path);
            self.blame_tasks.remove(path);
            self.blame_changed(cx);
        } else {
            self.load_blame(path, Duration::ZERO, cx);
        }
        true
    }

    /// Loads this file's blame (with the current buffer content) after `delay`. A newer
    /// request replaces one that is still pending.
    fn load_blame(&mut self, path: &str, delay: Duration, cx: &mut Context<Self>) {
        if !self.files.contains_key(path) {
            return;
        }
        let key = path.to_string();
        let task = cx.spawn({
            let key = key.clone();
            async move |this, cx| {
                if !delay.is_zero() {
                    cx.background_executor().timer(delay).await;
                }
                // The content is read after the delay so it includes the latest edit.
                let Ok(Some(request)) = this.update(cx, |this, cx| {
                    let tracked = this.files.get(&key)?;
                    let contents = tracked.buffer.upgrade().and_then(|b| b.read(cx).text(cx)).map(|rope| rope.to_string());
                    let (core, root, relative) = (this.env.core.clone(), tracked.root.clone(), tracked.relative.clone());
                    Some(spawn_core(cx, async move { git_api::git_blame(&core, root, relative, contents).await }))
                }) else {
                    return;
                };
                let result = request.await;
                let _ = this.update(cx, |this, cx| {
                    this.blame_tasks.remove(&key);
                    // Untracked or new file: empty blame, still marked as active.
                    this.blame.insert(key.clone(), result.unwrap_or_default());
                    this.stale_blame.remove(&key);
                    this.blame_changed(cx);
                });
            }
        });
        self.blame_tasks.insert(key, task);
    }

    /// The user's email in this repository, for "You".
    fn load_identity(&mut self, root: String, cx: &mut Context<Self>) {
        self.identities.insert(root.clone(), None);
        let core = self.env.core.clone();
        let task = spawn_core(cx, {
            let root = root.clone();
            async move { git_api::git_identity(&core, root).await }
        });
        cx.spawn(async move |this, cx| {
            let email = task.await.ok().and_then(|identity| identity.email);
            let _ = this.update(cx, |this, cx| {
                this.identities.insert(root, Some(email));
                this.refresh_hints(cx);
            });
        })
        .detach();
    }

    fn blame_changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(MarkersEvent::BlameChanged);
        cx.notify();
        self.refresh_hints(cx);
    }

    /// Asks the editor view to redraw inline blame.
    fn refresh_hints(&self, cx: &mut Context<Self>) {
        if let Some(buffers) = self.buffers.as_ref().and_then(WeakEntity::upgrade) {
            buffers.update(cx, |buffers, cx| buffers.refresh_line_hints(cx));
        }
    }
}

/// Seconds since the epoch.
fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// Status bar item: blame of the cursor line in the active file whose blame is turned on.
pub struct BlameStatus {
    markers: Entity<ChangeMarkers>,
    workbench: WeakEntity<Workbench>,
    _subscriptions: Vec<Subscription>,
    _state: Option<Subscription>,
}

impl BlameStatus {
    pub fn new(markers: Entity<ChangeMarkers>, workbench: WeakEntity<Workbench>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = vec![cx.observe(&markers, |_, _, cx| cx.notify())];
        if let Some(entity) = workbench.upgrade() {
            subscriptions.push(cx.subscribe_in(&entity, window, |this: &mut Self, workbench, event: &WorkbenchEvent, _, cx| {
                if matches!(event, WorkbenchEvent::ActiveTabChanged | WorkbenchEvent::TabsChanged) {
                    this._state = active_text_file(workbench.read(cx), cx).map(|(_, _, state)| cx.observe(&state, |_, _, cx| cx.notify()));
                    cx.notify();
                }
            }));
        }
        Self { markers, workbench, _subscriptions: subscriptions, _state: None }
    }
}

impl Render for BlameStatus {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some((view, buffer, _)) = self.workbench.upgrade().and_then(|w| active_text_file(w.read(cx), cx)) else { return div().into_any_element() };
        let path = buffer.read(cx).path().to_string();
        let (line, _, _) = view.read(cx).cursor_info(cx);
        let now = now();
        let Some(range) = self.markers.read(cx).line_blame(&path, line) else { return div().into_any_element() };
        let theme = cx.theme();
        let summary = blame_summary(range, now);
        h_flex()
            .id("status-blame")
            .test_support()
            .gap_1()
            .text_color(theme.muted_foreground)
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(summary.clone()).build(window, cx))
            .child(Icon::new(Lucide::GitCommitHorizontal).xsmall())
            .child(blame_label(range, now))
            .into_any_element()
    }
}

/// Registers the change markers, the blame status, and the `git.toggleBlame` command in a window.
pub(crate) fn setup(workbench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let env = workbench.env().clone();
    if !env.trusted || env.repo_root.is_none() {
        return;
    }
    let file_buffers = buffers(workbench);
    let markers = cx.new(|cx| ChangeMarkers::new(env.clone(), file_buffers, window, cx));
    workbench.set_extension(markers.clone());
    workbench.register_status(
        StatusItem::new("git.blame", StatusAlign::Right, 5, {
            let markers = markers.clone();
            move |env, window, cx| {
                let (markers, workbench) = (markers.clone(), env.workbench.clone());
                cx.new(|cx| BlameStatus::new(markers, workbench, window, cx)).into()
            }
        }),
        window,
        cx,
    );
    let weak = cx.weak_entity();
    workbench.register_command(
        oncode_ui::commands::Command::new("git.toggleBlame", "Toggle Blame", move |_, window, cx| {
            let Some(workbench) = weak.upgrade() else { return };
            let Some((_, buffer, _)) = active_file(workbench.read(cx), cx) else { return };
            let path = buffer.read(cx).path().to_string();
            let shown = markers.update(cx, |markers, cx| markers.toggle_blame(&path, cx));
            if !shown {
                overlay::toast_info("This file is not part of an open Git repository.", window, cx);
            }
        })
        .category("Git")
        .when({
            let weak = cx.weak_entity();
            move |_, cx| weak.upgrade().is_some_and(|w| active_file(w.read(cx), cx).is_some())
        }),
    );
    // Global toggle for `editor.inlineBlame`; all windows follow through `SettingsChanged`.
    workbench.register_command(
        oncode_ui::commands::Command::new("git.toggleInlineBlame", "Toggle Inline Blame", move |_, _, cx| {
            let core = oncode_ui::core(cx);
            let mut settings = core.user_settings();
            settings.editor.inline_blame = !core.effective_settings(&env.window).editor.inline_blame;
            spawn_core(cx, async move { oncode_core::api::app::settings_set(&core, settings).await }).detach();
        })
        .category("Git"),
    );
}

/// This window's change markers.
pub fn change_markers(workbench: &Workbench) -> Option<Entity<ChangeMarkers>> {
    workbench.extension::<Entity<ChangeMarkers>>()
}
