//! Penanda perubahan di gutter editor berkas (dibandingkan dengan isi index) dan blame.
//!
//! Penanda dihitung ulang (dengan jeda) setiap kali berkas disunting atau keadaan repositori
//! berubah. Blame ditampilkan untuk baris kursor di status bar, karena komponen editor tidak
//! menyediakan anotasi per baris di gutter.

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::component::input::{EditorState, GutterDecoration, GutterDecorationCollection};
use gpui_kit::component::{h_flex, ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::{div, App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, StatefulInteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _, Subscription, Task, TestSupportExt as _, WeakEntity, Window};
use oncode_core::api::git as git_api;
use oncode_git::{BlameRange, DiffSource};
use oncode_ui::editor::{active_file, buffers, Buffer, BufferEvent, BuffersEvent};
use oncode_ui::{overlay, repos, spawn_core, Env, Lucide, ReposEvent, StatusAlign, StatusItem, Workbench, WorkbenchEvent};

use crate::gutter::{blame_at, blame_label, blame_summary, gutter_ranges, repo_for_path, ChangeKind};
use crate::lines::LineDiff;

/// Jeda sebelum penanda dihitung ulang setelah suntingan.
const DEBOUNCE: Duration = Duration::from_millis(250);

struct Tracked {
    buffer: WeakEntity<Buffer>,
    root: String,
    relative: String,
    /// Isi index (sisi pembanding); `None` = berkas baru/untracked (semua baris "ditambah").
    base: Option<Option<String>>,
    collections: Vec<(WeakEntity<EditorState>, GutterDecorationCollection)>,
    _debounce: Task<()>,
    _load: Task<()>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MarkersEvent {
    BlameChanged,
}

/// Penanda perubahan dan blame untuk semua editor berkas satu jendela.
pub struct ChangeMarkers {
    env: Env,
    files: HashMap<String, Tracked>,
    /// Blame per path untuk berkas yang blame-nya sedang ditampilkan.
    blame: HashMap<String, Vec<BlameRange>>,
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
        // `Workbench` sedang dibangun saat ini dipanggil, jadi registry buffer diberikan dari luar.
        if let Some(buffers) = buffers {
            subscriptions.push(cx.subscribe_in(&buffers, window, |this: &mut Self, _, event: &BuffersEvent, _, cx| {
                if let BuffersEvent::EditorCreated { buffer, state } = event {
                    this.attach(buffer.clone(), state.clone(), cx);
                }
            }));
        }
        Self { env, files: HashMap::new(), blame: HashMap::new(), _subscriptions: subscriptions }
    }

    fn attach(&mut self, buffer: Entity<Buffer>, state: Entity<EditorState>, cx: &mut Context<Self>) {
        let path = buffer.read(cx).path().to_string();
        let roots = repos(cx).read(cx).roots();
        let Some((root, relative)) = repo_for_path(&path, &roots) else { return };
        let collection = state.update(cx, |state, cx| state.create_gutter_decorations_collection(Vec::new(), cx));
        if !self.files.contains_key(&path) {
            let subscription = cx.subscribe(&buffer, |this: &mut Self, buffer, event: &BufferEvent, cx| {
                if matches!(event, BufferEvent::Edited | BufferEvent::Loaded | BufferEvent::Saved) {
                    let path = buffer.read(cx).path().to_string();
                    this.schedule(&path, cx);
                    if this.blame.contains_key(&path) && !matches!(event, BufferEvent::Edited) {
                        this.load_blame(&path, cx);
                    }
                }
            });
            let release = cx.observe_release(&buffer, |this: &mut Self, buffer, _| {
                let path = buffer.path().to_string();
                this.files.remove(&path);
                this.blame.remove(&path);
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
        if self.inline_blame(cx) {
            self.load_blame(&path, cx);
        }
        self.schedule(&path, cx);
    }

    fn inline_blame(&self, cx: &App) -> bool {
        self.env.workbench.upgrade().is_some_and(|w| w.read(cx).settings().editor.inline_blame)
    }

    /// Memuat ulang isi index berkas ini lalu menghitung ulang penanda.
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

    /// Menghitung penanda di latar lalu memasangnya di semua editor berkas ini.
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

    /// Jumlah penanda yang terpasang pada sebuah berkas (untuk uji).
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

    /// Menyalakan atau mematikan blame berkas ini. Hasil `false` bila berkas bukan bagian repositori.
    pub fn toggle_blame(&mut self, path: &str, cx: &mut Context<Self>) -> bool {
        if !self.files.contains_key(path) {
            return false;
        }
        if self.blame.remove(path).is_some() {
            cx.emit(MarkersEvent::BlameChanged);
            cx.notify();
        } else {
            self.load_blame(path, cx);
        }
        true
    }

    fn load_blame(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(tracked) = self.files.get(path) else { return };
        let contents = tracked.buffer.upgrade().and_then(|b| b.read(cx).text(cx)).map(|rope| rope.to_string());
        let (core, root, relative, key) = (self.env.core.clone(), tracked.root.clone(), tracked.relative.clone(), path.to_string());
        let task = spawn_core(cx, async move { git_api::git_blame(&core, root, relative, contents).await });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                // Berkas untracked atau baru: blame kosong, tetap ditandai aktif.
                this.blame.insert(key, result.unwrap_or_default());
                cx.emit(MarkersEvent::BlameChanged);
                cx.notify();
            });
        })
        .detach();
    }
}

/// Item status bar: blame baris kursor pada berkas aktif yang blame-nya dinyalakan.
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
                    this._state = active_file(workbench.read(cx), cx).map(|(_, _, state)| cx.observe(&state, |_, _, cx| cx.notify()));
                    cx.notify();
                }
            }));
        }
        Self { markers, workbench, _subscriptions: subscriptions, _state: None }
    }
}

impl Render for BlameStatus {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some((view, buffer, _)) = self.workbench.upgrade().and_then(|w| active_file(w.read(cx), cx)) else { return div().into_any_element() };
        let path = buffer.read(cx).path().to_string();
        let Some(ranges) = self.markers.read(cx).blame_for(&path) else { return div().into_any_element() };
        let (line, _, _) = view.read(cx).cursor_info(cx);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
        let Some(range) = blame_at(ranges, line) else { return div().into_any_element() };
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

/// Mendaftarkan penanda perubahan, status blame, dan command `git.toggleBlame` di sebuah jendela.
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
}

/// Penanda perubahan jendela ini.
pub fn change_markers(workbench: &Workbench) -> Option<Entity<ChangeMarkers>> {
    workbench.extension::<Entity<ChangeMarkers>>()
}
