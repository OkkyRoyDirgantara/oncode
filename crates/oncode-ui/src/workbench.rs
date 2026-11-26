//! The main window: activity bar, sidebar, tabbed editor groups, bottom panel, and status bar.
//!
//! Features register themselves through [`register_feature`] (per window) and [`register_tab_kind`]
//! (global). All feature views are entities of their own (`AnyView`), so they may
//! read the `Workbench` in their `render`. What they must **not** do: read or update the
//! `Workbench` entity from inside functions called by the `Workbench` itself (view builders,
//! `TabKind::build`, `TabContent::can_close`), because the entity is borrowed at that point.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::resizable::{h_resizable, resizable_panel, v_resizable};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Selectable as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, relative, AnyElement, AnyView, App, AppContext as _, Bounds, Context, DragMoveEvent, Entity, EventEmitter, ExternalPaths, FocusHandle, Focusable, Global,
    InteractiveElement, IntoElement, MouseButton, ParentElement as _, Pixels, Point, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task,
    TestSupportExt as _, WeakEntity, Window,
};
use oncode_core::api::app::WorkspaceInfo;
use oncode_core::settings::SidebarPosition;
use oncode_core::{Settings, Shared};

use crate::commands::{self, Command, CommandRegistry};
use crate::model::{self, DropZone, Model, OpenOptions, Orientation, SplitSide, TabData};
use crate::overlay;
use crate::shell::file_tab;
use crate::Lucide;

/// The context of a window, given to all view builders. It does not change while the
/// `Workbench` lives: opening another folder or changing trust creates a new `Workbench`.
#[derive(Clone)]
pub struct Env {
    pub workbench: WeakEntity<Workbench>,
    pub core: Shared,
    /// The window label for the `oncode-core` API.
    pub window: SharedString,
    /// The project folder; `None` if the window has not opened a folder.
    pub root: Option<PathBuf>,
    /// The folder is trusted: Git, LSP, and project settings are active.
    pub trusted: bool,
    /// The root of the repository containing this folder, if there is one and the folder is trusted.
    pub repo_root: Option<String>,
}

impl Env {
    pub fn label(&self) -> &str {
        &self.window
    }

    pub fn root_string(&self) -> Option<String> {
        self.root.as_ref().map(|p| p.to_string_lossy().into_owned())
    }
}

/// The content of a tab that has been built.
pub trait TabContent: 'static {
    fn view(&self) -> AnyView;
    /// Moves keyboard focus to the tab content.
    fn focus(&self, window: &mut Window, cx: &mut App);
    /// The tab has unsaved changes.
    fn is_dirty(&self, _cx: &App) -> bool {
        false
    }
    /// Called before the tab is closed; a result of `false` cancels.
    fn can_close(&self, _window: &mut Window, _cx: &mut App) -> Task<bool> {
        Task::ready(true)
    }
    /// Calls `on_change` when state affecting the tab bar (e.g. dirty) changes.
    fn observe(&self, _on_change: Rc<dyn Fn(&mut App)>, _cx: &mut App) -> Option<Subscription> {
        None
    }
    /// For features that need to recognize their own tab content (`downcast_ref`).
    fn as_any(&self) -> &dyn std::any::Any;
}

/// A `TabContent` for an ordinary focusable view.
pub struct ViewTab<V>(pub Entity<V>);

impl<V: Render + Focusable> TabContent for ViewTab<V> {
    fn view(&self) -> AnyView {
        self.0.clone().into()
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        let handle = self.0.focus_handle(cx);
        handle.focus(window, cx);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// The tab content builder for one kind (`TabData::kind`).
pub trait TabKind: 'static {
    fn build(&self, tab: &TabData, env: &Env, window: &mut Window, cx: &mut App) -> Box<dyn TabContent>;
    fn icon(&self, _tab: &TabData) -> Option<Lucide> {
        None
    }
    /// A colored image (an asset path, e.g. a language logo) as the tab icon; takes precedence over [`TabKind::icon`].
    fn image(&self, _tab: &TabData) -> Option<SharedString> {
        None
    }
    /// The tab opened in the adjacent group by "Split Editor". Default: the same tab (two views
    /// of the same content). Kinds whose content may only be shown once (e.g. a terminal) return
    /// a new tab, or `None` if they cannot be split.
    fn split_copy(&self, tab: &TabData) -> Option<TabData> {
        Some(tab.clone())
    }
}

/// A tab being dragged. Other features may create it to drag something into the editor area
/// as a tab (e.g. a terminal from the panel), with `from: None`; the dropped tab is opened there.
#[derive(Debug, Clone)]
pub struct DraggedTab {
    pub tab: TabData,
    /// The source group in the editor area; `None` if it came from outside the editor area.
    pub from: Option<u32>,
}

/// A file or folder being dragged from a file list (e.g. the Explorer). A file
/// dropped onto the editor area is opened as a tab there, just like a dragged tab; a folder is not
/// accepted. Files from outside the application (dragged from the OS) arrive as `ExternalPaths` and are treated the same.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraggedEntry {
    /// Absolute path.
    pub path: String,
    pub is_dir: bool,
}

/// True if what is being dragged may be dropped onto the editor area: a tab, a file from a file list, or
/// a file from the OS (its type, folder or not, is only checked on drop).
fn accepts_drop(value: &dyn std::any::Any) -> bool {
    match value.downcast_ref::<DraggedEntry>() {
        Some(entry) => !entry.is_dir,
        None => value.is::<DraggedTab>() || value.is::<ExternalPaths>(),
    }
}

/// A small image that follows the pointer while a tab is dragged.
pub struct TabDragPreview {
    title: SharedString,
    icon: Option<Lucide>,
}

impl TabDragPreview {
    pub fn new(title: impl Into<SharedString>, icon: Option<Lucide>) -> Self {
        Self { title: title.into(), icon }
    }
}

impl Render for TabDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .gap_1p5()
            .px_3()
            .py_1()
            .text_sm()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.drag_border)
            .bg(theme.tab_active)
            .text_color(theme.tab_active_foreground)
            .opacity(0.9)
            .shadow_md()
            .when_some(self.icon, |el, icon| el.child(Icon::new(icon).small()))
            .child(self.title.clone())
    }
}

/// Where a tab is dropped in a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TabDrop {
    /// Before the n-th tab in the tab bar.
    Before(usize),
    /// At the end of the tab bar, or in the middle of the group content.
    End,
    /// A new group on this group's side.
    Split(SplitSide),
}

type BuildView = Rc<dyn Fn(&Env, &mut Window, &mut App) -> AnyView>;
type BadgeFn = Rc<dyn Fn(&App) -> Option<usize>>;

/// A view in the sidebar, selected through the activity bar.
#[derive(Clone)]
pub struct SidebarView {
    pub id: SharedString,
    pub title: SharedString,
    pub icon: Lucide,
    pub order: i32,
    pub build: BuildView,
    /// A small number on the icon (e.g. the number of changes).
    pub badge: Option<BadgeFn>,
}

impl SidebarView {
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>, icon: Lucide, order: i32, build: impl Fn(&Env, &mut Window, &mut App) -> AnyView + 'static) -> Self {
        Self { id: id.into(), title: title.into(), icon, order, build: Rc::new(build), badge: None }
    }

    pub fn badge(mut self, badge: impl Fn(&App) -> Option<usize> + 'static) -> Self {
        self.badge = Some(Rc::new(badge));
        self
    }
}

/// A view in the bottom panel.
#[derive(Clone)]
pub struct PanelView {
    pub id: SharedString,
    pub title: SharedString,
    pub order: i32,
    pub build: BuildView,
    pub badge: Option<BadgeFn>,
    /// Actions on the right side of the panel's tab bar.
    pub actions: Option<Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>>,
}

impl PanelView {
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>, order: i32, build: impl Fn(&Env, &mut Window, &mut App) -> AnyView + 'static) -> Self {
        Self { id: id.into(), title: title.into(), order, build: Rc::new(build), badge: None, actions: None }
    }

    pub fn badge(mut self, badge: impl Fn(&App) -> Option<usize> + 'static) -> Self {
        self.badge = Some(Rc::new(badge));
        self
    }

    pub fn actions(mut self, actions: impl Fn(&mut Window, &mut App) -> AnyElement + 'static) -> Self {
        self.actions = Some(Rc::new(actions));
        self
    }
}

/// A button at the right end of the tab bar, for that group's active tab (e.g. "Open Preview" for Markdown).
#[derive(Clone)]
pub struct EditorAction {
    pub id: SharedString,
    pub icon: Lucide,
    pub tooltip: SharedString,
    /// The command to run; that button's group is already active when the command runs.
    pub command: SharedString,
    /// Shown if it returns true for that group's active tab. Receives the window's effective
    /// settings; do not read the `Workbench` entity here (it is called during its render).
    pub when: Rc<dyn Fn(&TabData, &Settings) -> bool>,
}

impl EditorAction {
    pub fn new(
        id: impl Into<SharedString>,
        icon: Lucide,
        tooltip: impl Into<SharedString>,
        command: impl Into<SharedString>,
        when: impl Fn(&TabData, &Settings) -> bool + 'static,
    ) -> Self {
        Self { id: id.into(), icon, tooltip: tooltip.into(), command: command.into(), when: Rc::new(when) }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StatusAlign {
    Left,
    Right,
}

/// A status bar item; it is a view of its own so it can observe the state it needs.
#[derive(Clone)]
pub struct StatusItem {
    pub id: SharedString,
    pub align: StatusAlign,
    pub order: i32,
    pub build: BuildView,
}

impl StatusItem {
    pub fn new(id: impl Into<SharedString>, align: StatusAlign, order: i32, build: impl Fn(&Env, &mut Window, &mut App) -> AnyView + 'static) -> Self {
        Self { id: id.into(), align, order, build: Rc::new(build) }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum WorkbenchEvent {
    /// The active tab (or active group) changed.
    ActiveTabChanged,
    /// The list of tabs changed (open, close, rename).
    TabsChanged,
    /// The sidebar or bottom panel was shown, hidden, or switched.
    LayoutChanged,
    /// This window's effective settings changed; read them via [`Workbench::settings`].
    SettingsChanged,
    /// Files in the project folder changed on disk (absolute paths). `overflow`: too many to list in detail.
    FsChanged { paths: Vec<String>, overflow: bool },
    /// A file was just saved from the editor (absolute path).
    FileSaved(String),
}

type FeatureFn = Rc<dyn Fn(&mut Workbench, &mut Window, &mut Context<Workbench>)>;

#[derive(Default)]
struct Features(Vec<FeatureFn>);

impl Global for Features {}

#[derive(Default)]
struct TabKinds(HashMap<String, Rc<dyn TabKind>>);

impl Global for TabKinds {}

#[derive(Default)]
struct Windows(HashMap<gpui_kit::WindowId, WeakEntity<Workbench>>);

impl Global for Windows {}

pub(crate) fn install(cx: &mut App) {
    cx.set_global(Features::default());
    cx.set_global(TabKinds::default());
    cx.set_global(Windows::default());
}

/// Registers a feature. `setup` is called for every new `Workbench` (every window, and
/// every time a folder is opened), while its entity is being created.
pub fn register_feature(cx: &mut App, setup: impl Fn(&mut Workbench, &mut Window, &mut Context<Workbench>) + 'static) {
    cx.global_mut::<Features>().0.push(Rc::new(setup));
}

/// Registers the tab content builder for a tab kind.
pub fn register_tab_kind(cx: &mut App, kind: &str, builder: impl TabKind) {
    cx.global_mut::<TabKinds>().0.insert(kind.to_string(), Rc::new(builder));
}

struct Slot {
    content: Box<dyn TabContent>,
    _subscription: Option<Subscription>,
}

pub struct Workbench {
    env: Env,
    settings: Settings,
    commands: Rc<RefCell<CommandRegistry>>,
    model: Model,
    slots: HashMap<(u32, String), Slot>,
    sidebar_views: Vec<SidebarView>,
    sidebar_built: HashMap<SharedString, AnyView>,
    active_sidebar: Option<SharedString>,
    sidebar_visible: bool,
    panel_views: Vec<PanelView>,
    editor_actions: Vec<EditorAction>,
    panel_built: HashMap<SharedString, AnyView>,
    active_panel: Option<SharedString>,
    panel_visible: bool,
    status_items: Vec<(StatusItem, AnyView)>,
    /// Shown in the editor area when there are no tabs.
    empty_view: Option<AnyView>,
    focus_handle: FocusHandle,
    window_id: gpui_kit::WindowId,
    /// Per-window state owned by features (e.g. the buffer registry), looked up by its type.
    extensions: HashMap<std::any::TypeId, Box<dyn std::any::Any>>,
    /// Called right before tab content is built, so the global `TabKind` knows which window is building.
    before_build: Vec<Rc<dyn Fn(&mut App)>>,
    /// The drop zone over the group content while a tab is dragged: (group, zone).
    drop_zone: Option<(u32, DropZone)>,
}

impl EventEmitter<WorkbenchEvent> for Workbench {}

impl Focusable for Workbench {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Workbench {
    /// Creates the `Workbench` for this window and runs the registration of all features.
    pub fn new(core: Shared, label: impl Into<SharedString>, workspace: Option<&WorkspaceInfo>, settings: Settings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let env = Env {
            workbench: cx.weak_entity(),
            core,
            window: label.into(),
            root: workspace.map(|w| PathBuf::from(&w.root)),
            trusted: workspace.is_some_and(|w| w.trusted),
            repo_root: workspace.and_then(|w| w.repo_root.clone()),
        };
        let commands = Rc::new(RefCell::new(CommandRegistry::default()));
        let window_id = window.window_handle().window_id();
        commands::attach(window, commands.clone(), cx);
        let weak_self = cx.weak_entity();
        cx.global_mut::<Windows>().0.insert(window_id, weak_self);

        cx.on_release(release).detach();
        let mut this = Self {
            env,
            settings,
            commands,
            model: Model::default(),
            slots: HashMap::new(),
            sidebar_views: Vec::new(),
            sidebar_built: HashMap::new(),
            active_sidebar: None,
            sidebar_visible: true,
            panel_views: Vec::new(),
            editor_actions: Vec::new(),
            panel_built: HashMap::new(),
            active_panel: None,
            panel_visible: false,
            status_items: Vec::new(),
            empty_view: None,
            focus_handle: cx.focus_handle(),
            window_id,
            extensions: HashMap::new(),
            before_build: Vec::new(),
            drop_zone: None,
        };
        this.register_core_commands(cx);
        let features = cx.global::<Features>().0.clone();
        for feature in features {
            feature(&mut this, window, cx);
        }
        if this.active_sidebar.is_none() {
            this.active_sidebar = this.sorted_sidebar().first().map(|v| v.id.clone());
        }
        if this.active_panel.is_none() {
            this.active_panel = this.sorted_panels().first().map(|v| v.id.clone());
        }
        this
    }

    /// The `Workbench` of a window.
    pub fn for_window(window: &Window, cx: &App) -> Option<Entity<Workbench>> {
        cx.try_global::<Windows>()?.0.get(&window.window_handle().window_id())?.upgrade()
    }

    pub fn env(&self) -> &Env {
        &self.env
    }

    pub fn model(&self) -> &Model {
        &self.model
    }

    /// This window's effective settings (project settings are already applied if the folder is trusted).
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn set_settings(&mut self, settings: Settings, cx: &mut Context<Self>) {
        if self.settings != settings {
            self.settings = settings;
            cx.emit(WorkbenchEvent::SettingsChanged);
            cx.notify();
        }
    }

    /// Broadcasts an event to all `Workbench` subscribers (used by `Shell` and the editor).
    pub fn broadcast(&mut self, event: WorkbenchEvent, cx: &mut Context<Self>) {
        cx.emit(event);
    }

    /// Stores per-window state owned by a feature (usually an `Entity<..>`), looked up by its type.
    pub fn set_extension<T: 'static>(&mut self, value: T) {
        self.extensions.insert(std::any::TypeId::of::<T>(), Box::new(value));
    }

    pub fn extension<T: Clone + 'static>(&self) -> Option<T> {
        self.extensions.get(&std::any::TypeId::of::<T>()).and_then(|value| value.downcast_ref::<T>()).cloned()
    }

    /// Registers a function called right before `TabKind::build`. `TabKind`s are registered globally,
    /// so features use this to point at the per-window state their builder must use.
    pub fn on_before_build_tabs(&mut self, f: impl Fn(&mut App) + 'static) {
        self.before_build.push(Rc::new(f));
    }

    /// The content of the active tab, if it has been built.
    pub fn active_content(&self) -> Option<&dyn TabContent> {
        let (group, tab) = self.model.active()?;
        self.slots.get(&(group.id, tab.id.clone())).map(|slot| slot.content.as_ref())
    }

    /// The id of the selected sidebar view, whether or not it is visible.
    pub fn selected_sidebar(&self) -> Option<&SharedString> {
        self.active_sidebar.as_ref()
    }

    // ----- Registration -----

    pub fn register_command(&mut self, command: Command) {
        self.commands.borrow_mut().register(command);
    }

    pub fn register_sidebar(&mut self, view: SidebarView) {
        self.sidebar_views.retain(|v| v.id != view.id);
        self.sidebar_views.push(view);
    }

    /// Adds an editor action button (see [`EditorAction`]); the same id replaces the old one.
    pub fn register_editor_action(&mut self, action: EditorAction) {
        self.editor_actions.retain(|existing| existing.id != action.id);
        self.editor_actions.push(action);
    }

    pub fn register_panel(&mut self, view: PanelView) {
        self.panel_views.retain(|v| v.id != view.id);
        self.panel_views.push(view);
    }

    pub fn register_status(&mut self, item: StatusItem, window: &mut Window, cx: &mut Context<Self>) {
        let view = (item.build)(&self.env, window, cx);
        self.status_items.retain(|(i, _)| i.id != item.id);
        self.status_items.push((item, view));
    }

    /// The editor area view when no tab is open (e.g. the welcome screen).
    pub fn set_empty_view(&mut self, view: AnyView) {
        self.empty_view = Some(view);
    }

    fn register_core_commands(&mut self, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        let with = move |f: fn(&mut Workbench, &mut Window, &mut Context<Workbench>)| {
            let weak = weak.clone();
            move |_: &serde_json::Value, window: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |this, cx| f(this, window, cx));
            }
        };
        self.register_command(Command::new("workbench.toggleSidebar", "Toggle Sidebar", with(|this, _, cx| this.toggle_sidebar(cx))).category("View").key("Mod+B"));
        self.register_command(
            Command::new("workbench.toggleSidebarPosition", "Move Sidebar Left/Right", |_, _, cx| {
                // Saved to the user settings; all windows follow via `SettingsChanged`.
                let core = crate::core(cx);
                let mut settings = core.user_settings();
                settings.workbench.sidebar_position = match settings.workbench.sidebar_position {
                    SidebarPosition::Left => SidebarPosition::Right,
                    SidebarPosition::Right => SidebarPosition::Left,
                };
                crate::spawn_core(cx, async move { oncode_core::api::app::settings_set(&core, settings).await }).detach();
            })
            .category("View"),
        );
        self.register_command(Command::new("workbench.togglePanel", "Toggle Panel", with(|this, window, cx| this.toggle_panel(window, cx))).category("View").key("Mod+J"));
        self.register_command(
            Command::new("workbench.closeTab", "Close Editor", with(|this, window, cx| {
                if let Some((group, tab)) = this.model.active().map(|(g, t)| (g.id, t.id.clone())) {
                    this.close_tabs(group, vec![tab], false, window, cx).detach();
                }
            }))
            .category("View")
            .key("Mod+W"),
        );
        self.register_command(
            Command::new("workbench.closeAllTabs", "Close All Editors", with(|this, window, cx| {
                this.close_all_tabs(false, window, cx).detach();
            }))
            .category("View"),
        );
        self.register_command(Command::new("workbench.nextTab", "Next Editor", with(|this, window, cx| this.cycle_tab(1, window, cx))).category("View").key("Mod+PageDown"));
        self.register_command(Command::new("workbench.previousTab", "Previous Editor", with(|this, window, cx| this.cycle_tab(-1, window, cx))).category("View").key("Mod+PageUp"));
        self.register_command(
            Command::new("workbench.split", "Split Editor", with(|this, window, cx| {
                let Some(tab) = this.model.active().map(|(_, t)| t.clone()) else { return };
                let kind = cx.global::<TabKinds>().0.get(&tab.kind).cloned();
                let copy = match kind {
                    Some(kind) => kind.split_copy(&tab),
                    None => Some(tab),
                };
                if let Some(copy) = copy {
                    this.open_tab(copy, OpenOptions::to_side(), window, cx);
                }
            }))
            .category("View")
            .key("Mod+\\"),
        );
        self.register_command(
            Command::new("workbench.toggleSplitDirection", "Toggle Split Direction", with(|this, _, cx| {
                this.model.toggle_orientation();
                cx.emit(WorkbenchEvent::LayoutChanged);
                cx.notify();
            }))
            .category("View"),
        );
    }

    // ----- Tabs -----

    /// Opens a tab (or activates it if it already exists) and returns its id.
    pub fn open_tab(&mut self, tab: TabData, options: OpenOptions, window: &mut Window, cx: &mut Context<Self>) -> String {
        let id = tab.id.clone();
        let background = options.background;
        self.model.open(tab, &options);
        self.sync_slots(window, cx);
        if !background {
            self.focus_active(window, cx);
        }
        cx.emit(WorkbenchEvent::TabsChanged);
        cx.emit(WorkbenchEvent::ActiveTabChanged);
        cx.notify();
        id
    }

    pub fn activate_tab(&mut self, group: u32, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.model.activate(group, id);
        self.focus_active(window, cx);
        cx.emit(WorkbenchEvent::ActiveTabChanged);
        cx.notify();
    }

    fn activate_group(&mut self, group: u32, cx: &mut Context<Self>) {
        if self.model.active_group != group {
            self.model.activate_group(group);
            cx.emit(WorkbenchEvent::ActiveTabChanged);
            cx.notify();
        }
    }

    pub fn pin_tab(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.model.pin(id) {
            cx.emit(WorkbenchEvent::TabsChanged);
            cx.notify();
        }
    }

    /// Changes tabs in all groups (e.g. after a file is renamed).
    pub fn update_tabs(&mut self, change: impl FnMut(&TabData) -> Option<TabData>, window: &mut Window, cx: &mut Context<Self>) {
        self.model.update_tabs(change);
        self.sync_slots(window, cx);
        cx.emit(WorkbenchEvent::TabsChanged);
        cx.notify();
    }

    fn cycle_tab(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        self.model.cycle(delta);
        self.focus_active(window, cx);
        cx.emit(WorkbenchEvent::ActiveTabChanged);
        cx.notify();
    }

    /// Closes tabs once their content allows it (unless `force`). A result of `false` means canceled.
    pub fn close_tabs(&mut self, group: u32, ids: Vec<String>, force: bool, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
        cx.spawn_in(window, async move |this, cx| {
            let mut closing: Vec<String> = Vec::new();
            let mut allowed = true;
            for id in ids {
                let check = this.update_in(cx, |this, window, cx| {
                    if !this.model.group(group).is_some_and(|g| g.contains(&id)) {
                        return None;
                    }
                    let elsewhere = this.model.groups.iter().any(|g| g.id != group && g.contains(&id));
                    if force || elsewhere {
                        return Some(Task::ready(true));
                    }
                    Some(match this.slots.get(&(group, id.clone())) {
                        Some(slot) => slot.content.can_close(window, cx),
                        None => Task::ready(true),
                    })
                });
                match check {
                    Ok(Some(task)) => {
                        if task.await {
                            closing.push(id);
                        } else {
                            allowed = false;
                            break;
                        }
                    }
                    Ok(None) => {}
                    Err(_) => return false,
                }
            }
            let _ = this.update_in(cx, |this, window, cx| this.remove_tabs(group, &closing, window, cx));
            allowed
        })
    }

    pub fn close_all_tabs(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
        let groups: Vec<(u32, Vec<String>)> = self.model.groups.iter().map(|g| (g.id, g.tabs.iter().map(|t| t.id.clone()).collect())).collect();
        cx.spawn_in(window, async move |this, cx| {
            for (group, ids) in groups {
                let Ok(task) = this.update_in(cx, |this, window, cx| this.close_tabs(group, ids, force, window, cx)) else { return false };
                if !task.await {
                    return false;
                }
            }
            true
        })
    }

    /// Closes matching tabs in all groups without confirmation (e.g. their file was deleted).
    pub fn force_close_where(&mut self, matches: impl Fn(&TabData) -> bool, window: &mut Window, cx: &mut Context<Self>) {
        let targets: Vec<(u32, Vec<String>)> =
            self.model.groups.iter().map(|g| (g.id, g.tabs.iter().filter(|t| matches(t)).map(|t| t.id.clone()).collect::<Vec<_>>())).filter(|(_, ids)| !ids.is_empty()).collect();
        for (group, ids) in targets {
            self.remove_tabs(group, &ids, window, cx);
        }
    }

    fn remove_tabs(&mut self, group: u32, ids: &[String], window: &mut Window, cx: &mut Context<Self>) {
        if ids.is_empty() {
            return;
        }
        self.model.remove(group, ids);
        self.sync_slots(window, cx);
        self.focus_active(window, cx);
        cx.emit(WorkbenchEvent::TabsChanged);
        cx.emit(WorkbenchEvent::ActiveTabChanged);
        cx.notify();
    }

    /// There is a tab with unsaved changes.
    pub fn has_dirty_tabs(&self, cx: &App) -> bool {
        self.slots.values().any(|slot| slot.content.is_dirty(cx))
    }

    // ----- Drag and drop -----

    /// Moves the dropped tab to group `group`. The content of an already built tab moves along
    /// (it is not rebuilt), so its state (e.g. scroll position) is kept.
    fn drop_tab(&mut self, dragged: &DraggedTab, group: u32, place: TabDrop, window: &mut Window, cx: &mut Context<Self>) {
        self.drop_zone = None;
        let tab = dragged.tab.clone();
        let id = tab.id.clone();
        let (target, source) = match place {
            TabDrop::Before(index) => (group, self.model.move_tab(tab, dragged.from, group, Some(index))),
            TabDrop::End => (group, self.model.move_tab(tab, dragged.from, group, None)),
            TabDrop::Split(side) => self.model.split_tab(tab, dragged.from, group, side),
        };
        if let Some(source) = source {
            if let Some(slot) = self.slots.remove(&(source, id.clone())) {
                self.slots.entry((target, id)).or_insert(slot);
            }
        }
        self.sync_slots(window, cx);
        self.focus_active(window, cx);
        cx.emit(WorkbenchEvent::TabsChanged);
        cx.emit(WorkbenchEvent::ActiveTabChanged);
        cx.emit(WorkbenchEvent::LayoutChanged);
        cx.notify();
    }

    /// Opens files dropped onto group `group`, from the Explorer or from the OS. The first is opened
    /// where it was dropped (`place`), the rest follow as tabs in that group. Folders are skipped. Files
    /// outside the project folder are rejected: all file access is restricted to the project.
    fn drop_files(&mut self, paths: Vec<String>, group: u32, place: TabDrop, window: &mut Window, cx: &mut Context<Self>) {
        self.drop_zone = None;
        let Some(root) = self.env.root.clone() else {
            overlay::toast_info("Open a folder first to work with files.", window, cx);
            cx.notify();
            return;
        };
        let (mut files, mut outside, mut folders) = (Vec::new(), Vec::new(), 0);
        for path in paths {
            let target = std::path::Path::new(&path);
            if target.is_dir() {
                folders += 1;
            } else if oncode_workspace::is_inside(&root, target) {
                files.push(path);
            } else {
                outside.push(target.file_name().map_or_else(|| path.clone(), |name| name.to_string_lossy().into_owned()));
            }
        }
        if !outside.is_empty() {
            overlay::toast_error(format!("Only files inside the project folder can be opened: {}", outside.join(", ")), window, cx);
        } else if files.is_empty() && folders > 0 {
            overlay::toast_info("Folders cannot be opened in the editor. Drop files instead.", window, cx);
        }
        let mut files = files.into_iter();
        let Some(first) = files.next() else {
            cx.notify();
            return;
        };
        self.drop_tab(&DraggedTab { tab: file_tab(&first), from: None }, group, place, window, cx);
        for path in files {
            let options = OpenOptions { group: Some(self.model.active_group), ..Default::default() };
            self.open_tab(file_tab(&path), options, window, cx);
        }
    }

    /// The drop place in group `group`'s content by the zone currently highlighted: an edge splits, the center goes into the group.
    fn zone_place(&self, group: u32) -> TabDrop {
        match self.drop_zone.filter(|(g, _)| *g == group).map(|(_, zone)| zone) {
            Some(DropZone::Split(side)) => TabDrop::Split(side),
            _ => TabDrop::End,
        }
    }

    /// Makes `el` accept tabs, files from the Explorer, and files from the OS dropped onto group
    /// `group`. The drop place is decided by `place`, which is called on drop. `tint` colors
    /// `el` while something is dragged over it; the group content does not use it because it already has a zone highlight.
    fn accept_drops<E: InteractiveElement>(&self, el: E, group: u32, tint: bool, place: impl Fn(&Self) -> TabDrop + Clone + 'static, cx: &Context<Self>) -> E {
        let (place_tab, place_entry) = (place.clone(), place.clone());
        let el = el.can_drop(|value, _, _| accepts_drop(value));
        let el = if tint {
            el.drag_over::<DraggedTab>(|style, _, _, cx| style.bg(cx.theme().drop_target))
                .drag_over::<DraggedEntry>(|style, _, _, cx| style.bg(cx.theme().drop_target))
                .drag_over::<ExternalPaths>(|style, _, _, cx| style.bg(cx.theme().drop_target))
        } else {
            el
        };
        el.on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                let place = place_tab(this);
                this.drop_tab(dragged, group, place, window, cx);
            }))
            .on_drop(cx.listener(move |this, dragged: &DraggedEntry, window, cx| {
                let place = place_entry(this);
                this.drop_files(vec![dragged.path.clone()], group, place, window, cx);
            }))
            .on_drop(cx.listener(move |this, dropped: &ExternalPaths, window, cx| {
                let place = place(this);
                let paths = dropped.paths().iter().map(|path| path.to_string_lossy().into_owned()).collect();
                this.drop_files(paths, group, place, window, cx);
            }))
    }

    /// A tab standing in for something being dragged into the editor area, to compute the drop zone;
    /// `None` if nothing can be opened (e.g. a folder).
    fn drag_stand_in(value: &dyn std::any::Any) -> Option<DraggedTab> {
        if let Some(dragged) = value.downcast_ref::<DraggedTab>() {
            return Some(dragged.clone());
        }
        let path = match (value.downcast_ref::<DraggedEntry>(), value.downcast_ref::<ExternalPaths>()) {
            (Some(entry), _) if !entry.is_dir => entry.path.clone(),
            (None, Some(external)) => external.paths().first()?.to_string_lossy().into_owned(),
            _ => return None,
        };
        Some(DraggedTab { tab: file_tab(&path), from: None })
    }

    /// Computes the drop zone while something is dragged over group `group`'s content.
    fn track_drop_zone(&mut self, group: u32, dragged: Option<DraggedTab>, bounds: Bounds<Pixels>, position: Point<Pixels>, cx: &mut Context<Self>) {
        let zone = if let Some(dragged) = dragged.filter(|_| bounds.contains(&position)) {
            let (horizontal, vertical) = self.model.split_axes(group, &dragged.tab.id, dragged.from);
            let x = (position.x - bounds.origin.x) / bounds.size.width;
            let y = (position.y - bounds.origin.y) / bounds.size.height;
            Some((group, model::drop_zone(x, y, horizontal, vertical)))
        } else if self.drop_zone.is_some_and(|(g, _)| g == group) {
            None
        } else {
            return;
        };
        if zone != self.drop_zone {
            self.drop_zone = zone;
            cx.notify();
        }
    }

    /// Syncs the content of already built tabs with the model.
    fn sync_slots(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let wanted: Vec<(u32, TabData)> = self.model.groups.iter().flat_map(|g| g.tabs.iter().map(move |t| (g.id, t.clone()))).collect();
        self.slots.retain(|(group, id), _| wanted.iter().any(|(g, t)| g == group && &t.id == id));
        for (group, tab) in wanted {
            let key = (group, tab.id.clone());
            if self.slots.contains_key(&key) {
                continue;
            }
            let kind = cx.global::<TabKinds>().0.get(&tab.kind).cloned();
            for prepare in self.before_build.clone() {
                prepare(cx);
            }
            let content: Box<dyn TabContent> = match kind {
                Some(kind) => kind.build(&tab, &self.env, window, cx),
                None => Box::new(ViewTab(cx.new(|cx| MissingTab { kind: tab.kind.clone().into(), focus_handle: cx.focus_handle() }))),
            };
            let weak = cx.weak_entity();
            let subscription = content.observe(
                Rc::new(move |cx: &mut App| {
                    let _ = weak.update(cx, |_, cx| cx.notify());
                }),
                cx,
            );
            self.slots.insert(key, Slot { content, _subscription: subscription });
        }
    }

    fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((group, tab)) = self.model.active().map(|(g, t)| (g.id, t.id.clone())) else {
            self.focus_handle.focus(window, cx);
            return;
        };
        if let Some(slot) = self.slots.get(&(group, tab)) {
            slot.content.focus(window, cx);
        }
    }

    // ----- Sidebar and panel -----

    fn sorted_sidebar(&self) -> Vec<SidebarView> {
        let mut views = self.sidebar_views.clone();
        views.sort_by_key(|v| v.order);
        views
    }

    fn sorted_panels(&self) -> Vec<PanelView> {
        let mut views = self.panel_views.clone();
        views.sort_by_key(|v| v.order);
        views
    }

    pub fn is_sidebar_visible(&self) -> bool {
        self.sidebar_visible
    }

    pub fn is_panel_visible(&self) -> bool {
        self.panel_visible
    }

    pub fn active_sidebar(&self) -> Option<&SharedString> {
        self.sidebar_visible.then_some(self.active_sidebar.as_ref()).flatten()
    }

    pub fn active_panel(&self) -> Option<&SharedString> {
        self.panel_visible.then_some(self.active_panel.as_ref()).flatten()
    }

    pub fn show_sidebar(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.sidebar_views.iter().any(|v| v.id.as_ref() == id) {
            self.active_sidebar = Some(SharedString::from(id.to_string()));
            self.sidebar_visible = true;
            cx.emit(WorkbenchEvent::LayoutChanged);
            cx.notify();
        }
    }

    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_visible = !self.sidebar_visible;
        cx.emit(WorkbenchEvent::LayoutChanged);
        cx.notify();
    }

    pub fn show_panel(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.panel_views.iter().any(|v| v.id.as_ref() == id) {
            self.active_panel = Some(SharedString::from(id.to_string()));
            self.panel_visible = true;
            cx.emit(WorkbenchEvent::LayoutChanged);
            cx.notify();
        }
    }

    pub fn toggle_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.panel_visible = !self.panel_visible;
        if !self.panel_visible {
            self.focus_active(window, cx);
        }
        cx.emit(WorkbenchEvent::LayoutChanged);
        cx.notify();
    }

    pub fn hide_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.panel_visible {
            self.toggle_panel(window, cx);
        }
    }

    fn sidebar_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyView> {
        let id = self.active_sidebar.clone()?;
        if let Some(view) = self.sidebar_built.get(&id) {
            return Some(view.clone());
        }
        let build = self.sidebar_views.iter().find(|v| v.id == id)?.build.clone();
        let view = build(&self.env, window, cx);
        self.sidebar_built.insert(id, view.clone());
        Some(view)
    }

    fn panel_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyView> {
        let id = self.active_panel.clone()?;
        if let Some(view) = self.panel_built.get(&id) {
            return Some(view.clone());
        }
        let build = self.panel_views.iter().find(|v| v.id == id)?.build.clone();
        let view = build(&self.env, window, cx);
        self.panel_built.insert(id, view.clone());
        Some(view)
    }

    // ----- Views -----

    /// The activity bar and sidebar are on the right of the window.
    pub fn sidebar_on_right(&self) -> bool {
        self.settings.workbench.sidebar_position == SidebarPosition::Right
    }

    fn render_activity_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let active = self.active_sidebar().cloned();
        let on_right = self.sidebar_on_right();
        v_flex()
            .id("activity-bar")
            .test_support()
            .w(px(44.))
            .h_full()
            .flex_none()
            .items_center()
            .py_1()
            .gap_1()
            .bg(theme.sidebar)
            // The divider line is always on the side facing the editor.
            .map(|el| if on_right { el.border_l_1() } else { el.border_r_1() })
            .border_color(theme.border)
            .children(self.sorted_sidebar().into_iter().map(|view| {
                let selected = active.as_ref() == Some(&view.id);
                let badge = view.badge.as_ref().and_then(|badge| badge(cx)).filter(|n| *n > 0);
                let id = view.id.clone();
                div()
                    .relative()
                    .child(
                        Button::new(SharedString::from(format!("activity-{}", view.id)))
                            .ghost()
                            .icon(Icon::new(view.icon))
                            .tooltip(view.title.clone())
                            .selected(selected)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.sidebar_visible && this.active_sidebar.as_ref() == Some(&id) {
                                    this.toggle_sidebar(cx);
                                } else {
                                    this.show_sidebar(&id.clone(), cx);
                                }
                            })),
                    )
                    .when_some(badge, |el, count| {
                        el.child(
                            div()
                                .absolute()
                                .top_0()
                                .right_0()
                                .px_1()
                                .rounded_full()
                                .bg(cx.theme().primary)
                                .text_color(cx.theme().primary_foreground)
                                .text_size(px(10.))
                                .child(if count > 99 { "99+".to_string() } else { count.to_string() }),
                        )
                    })
            }))
            .child(div().flex_1())
            .child(
                Button::new("activity-toggle-panel")
                    .ghost()
                    .icon(Icon::new(Lucide::PanelBottom))
                    .tooltip("Toggle Panel")
                    .selected(self.panel_visible)
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_panel(window, cx))),
            )
            .child(Button::new("activity-settings").ghost().icon(Icon::new(Lucide::Settings)).tooltip("Settings").on_click(|_, window, cx| {
                commands::execute("settings.open", serde_json::Value::Null, window, cx);
            }))
    }

    fn render_tab_strip(&self, group: u32, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let Some(model_group) = self.model.group(group) else { return div().into_any_element() };
        let active_group = self.model.active_group == group;
        let kinds = &cx.global::<TabKinds>().0;
        // Action buttons for this group's active tab, outside the scrollable tab area.
        let active_tab = model_group.active.as_deref().and_then(|id| model_group.tabs.iter().find(|tab| tab.id == id));
        let actions: Vec<EditorAction> = active_tab.map(|tab| self.editor_actions.iter().filter(|action| (action.when)(tab, &self.settings)).cloned().collect()).unwrap_or_default();
        let action_bar = h_flex().flex_none().h_full().px_1().gap_0p5().children(actions.into_iter().map(|action| {
            let command = action.command.clone();
            Button::new(SharedString::from(format!("editor-action-{}", action.id))).ghost().xsmall().icon(Icon::new(action.icon)).tooltip(action.tooltip.clone()).on_click(
                move |_, window, cx| {
                    commands::execute(command.clone(), serde_json::Value::Null, window, cx);
                },
            )
        }));
        let tabs = h_flex()
            .id(SharedString::from(format!("tabs-{group}")))
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_x_scroll()
            .children(model_group.tabs.iter().enumerate().map(|(index, tab)| {
                let selected = model_group.active.as_deref() == Some(tab.id.as_str());
                let dirty = self.slots.get(&(group, tab.id.clone())).is_some_and(|slot| slot.content.is_dirty(cx));
                let icon = kinds.get(&tab.kind).and_then(|kind| kind.icon(tab));
                let image = kinds.get(&tab.kind).and_then(|kind| kind.image(tab));
                let id = tab.id.clone();
                let close_id = tab.id.clone();
                let pin_id = tab.id.clone();
                let menu_id = tab.id.clone();
                let weak = cx.weak_entity();
                h_flex()
                    .id(SharedString::from(format!("tab-{group}-{}", tab.id)))
                    .test_support()
                    .group("tab")
                    .h_full()
                    .flex_none()
                    .gap_1p5()
                    .pl_3()
                    .pr_1p5()
                    .border_r_1()
                    .border_color(theme.border)
                    .text_sm()
                    .when(selected, |el| el.bg(theme.tab_active).text_color(theme.tab_active_foreground))
                    .when(!selected, |el| el.text_color(theme.tab_foreground).hover(|s| s.bg(theme.tab_active.opacity(0.5))))
                    .when(selected && active_group, |el| el.border_t_2().border_color(theme.primary))
                    .when_some(tab.description.clone(), |el, description| {
                        el.tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(description.clone()).build(window, cx))
                    })
                    .on_click(cx.listener(move |this, event: &gpui_kit::ClickEvent, window, cx| {
                        if event.click_count() >= 2 {
                            this.pin_tab(&pin_id, cx);
                        }
                        this.activate_tab(group, &id, window, cx);
                    }))
                    .on_mouse_down(MouseButton::Middle, {
                        let id = tab.id.clone();
                        cx.listener(move |this, _, window, cx| {
                            this.close_tabs(group, vec![id.clone()], false, window, cx).detach();
                        })
                    })
                    // Drag to move to another position, to another group, or to a group edge (split).
                    .on_drag(DraggedTab { tab: tab.clone(), from: Some(group) }, move |dragged, _, _, cx| {
                        cx.stop_propagation();
                        cx.new(|_| TabDragPreview::new(dragged.tab.title.clone(), icon))
                    })
                    .map(|el| self.accept_drops(el, group, true, move |_| TabDrop::Before(index), cx))
                    .map(|el| match (image, icon) {
                        (Some(image), _) => el.child(gpui_kit::img(image).size(px(16.)).flex_none()),
                        (None, Some(icon)) => el.child(Icon::new(icon).small()),
                        (None, None) => el,
                    })
                    .child(div().when(tab.preview, |el| el.italic()).child(SharedString::from(tab.title.clone())))
                    .child(
                        div()
                            .id(SharedString::from(format!("tab-close-{group}-{}", tab.id)))
                            .flex_none()
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(theme.radius)
                            .text_color(theme.muted_foreground)
                            .hover(|s| s.bg(theme.secondary_hover).text_color(theme.foreground))
                            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(if dirty { "Unsaved changes; click to close" } else { "Close" }).build(window, cx))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_tabs(group, vec![close_id.clone()], false, window, cx).detach();
                            }))
                            .map(|el| {
                                if dirty {
                                    // A dot: there are unsaved changes.
                                    el.child(div().size(px(8.)).rounded_full().bg(theme.foreground))
                                } else {
                                    el.child(Icon::new(gpui_kit::component::IconName::Close).size(px(12.)))
                                }
                            }),
                    )
                    .context_menu(move |menu, _, _| {
                        let (a, b, c) = (weak.clone(), weak.clone(), weak.clone());
                        let (id_a, id_b) = (menu_id.clone(), menu_id.clone());
                        menu.item(gpui_kit::component::menu::PopupMenuItem::new("Close").on_click(move |_, window, cx| {
                            let _ = a.update(cx, |this, cx| this.close_tabs(group, vec![id_a.clone()], false, window, cx).detach());
                        }))
                        .item(gpui_kit::component::menu::PopupMenuItem::new("Close Others").on_click(move |_, window, cx| {
                            let _ = b.update(cx, |this, cx| {
                                let others: Vec<String> = this.model.group(group).map(|g| g.tabs.iter().filter(|t| t.id != id_b).map(|t| t.id.clone()).collect()).unwrap_or_default();
                                this.close_tabs(group, others, false, window, cx).detach();
                            });
                        }))
                        .item(gpui_kit::component::menu::PopupMenuItem::new("Close All").on_click(move |_, window, cx| {
                            let _ = c.update(cx, |this, cx| this.close_all_tabs(false, window, cx).detach());
                        }))
                    })
            }))
            // The empty space after the last tab: dropping a tab here puts it at the very end.
            .child(
                div()
                    .id(SharedString::from(format!("tabs-end-{group}")))
                    .test_support()
                    .flex_1()
                    .min_w(px(32.))
                    .h_full()
                    .map(|el| self.accept_drops(el, group, true, |_| TabDrop::End, cx)),
            );
        h_flex()
            .w_full()
            .h(px(34.))
            .flex_none()
            .bg(theme.tab_bar)
            .border_b_1()
            .border_color(theme.border)
            .child(tabs)
            .child(action_bar)
            .into_any_element()
    }

    fn render_group(&self, group: u32, cx: &mut Context<Self>) -> AnyElement {
        let content = self.model.group(group).and_then(|g| g.active.clone()).and_then(|id| self.slots.get(&(group, id))).map(|slot| slot.content.view());
        // Highlight of where the tab will land: half the side for a split, the whole area to go into the group.
        let zone = self.drop_zone.filter(|(g, _)| *g == group && cx.has_active_drag()).map(|(_, zone)| zone);
        let overlay = zone.map(|zone| {
            let base = div().absolute().bg(cx.theme().drop_target);
            match zone {
                DropZone::Center => base.top_0().left_0().size_full(),
                DropZone::Split(SplitSide::Left) => base.top_0().left_0().h_full().w(relative(0.5)),
                DropZone::Split(SplitSide::Right) => base.top_0().right_0().h_full().w(relative(0.5)),
                DropZone::Split(SplitSide::Up) => base.top_0().left_0().w_full().h(relative(0.5)),
                DropZone::Split(SplitSide::Down) => base.bottom_0().left_0().w_full().h(relative(0.5)),
            }
        });
        v_flex()
            .id(SharedString::from(format!("group-{group}")))
            .size_full()
            .overflow_hidden()
            .capture_any_mouse_down(cx.listener(move |this, _, _, cx| this.activate_group(group, cx)))
            .child(self.render_tab_strip(group, cx))
            .child(
                div()
                    .id(SharedString::from(format!("group-content-{group}")))
                    .test_support()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .on_drag_move::<DraggedTab>(cx.listener(move |this, event: &DragMoveEvent<DraggedTab>, _, cx| {
                        this.track_drop_zone(group, Self::drag_stand_in(event.drag(cx)), event.bounds, event.event.position, cx)
                    }))
                    .on_drag_move::<DraggedEntry>(cx.listener(move |this, event: &DragMoveEvent<DraggedEntry>, _, cx| {
                        this.track_drop_zone(group, Self::drag_stand_in(event.drag(cx)), event.bounds, event.event.position, cx)
                    }))
                    .on_drag_move::<ExternalPaths>(cx.listener(move |this, event: &DragMoveEvent<ExternalPaths>, _, cx| {
                        this.track_drop_zone(group, Self::drag_stand_in(event.drag(cx)), event.bounds, event.event.position, cx)
                    }))
                    .map(|el| self.accept_drops(el, group, false, move |this| this.zone_place(group), cx))
                    .when_some(content, |el, view| el.child(view))
                    .children(overlay),
            )
            .into_any_element()
    }

    fn render_editor_area(&self, cx: &mut Context<Self>) -> AnyElement {
        let has_tabs = self.model.all_tabs().next().is_some();
        if !has_tabs {
            let group = self.model.active_group;
            return div()
                .id("editor-empty")
                .test_support()
                .size_full()
                .bg(cx.theme().background)
                .map(|el| self.accept_drops(el, group, true, |_| TabDrop::End, cx))
                .map(|el| match &self.empty_view {
                    Some(view) => el.child(view.clone()),
                    None => el.flex().items_center().justify_center().text_color(cx.theme().muted_foreground).child("No file is open"),
                })
                .into_any_element();
        }
        let groups: Vec<u32> = self.model.groups.iter().map(|g| g.id).collect();
        if groups.len() == 1 {
            return self.render_group(groups[0], cx);
        }
        let container = match self.model.orientation {
            Orientation::Horizontal => h_resizable("editor-groups"),
            Orientation::Vertical => v_resizable("editor-groups"),
        };
        container.children(groups.into_iter().map(|group| resizable_panel().child(self.render_group(group, cx)))).into_any_element()
    }

    fn render_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let view = self.panel_view(window, cx);
        let theme = cx.theme();
        let active = self.active_panel.clone();
        let actions = self.panel_views.iter().find(|v| Some(&v.id) == active.as_ref()).and_then(|v| v.actions.clone());
        v_flex()
            .id("bottom-panel")
            .size_full()
            .bg(theme.background)
            .border_t_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .w_full()
                    .h(px(32.))
                    .flex_none()
                    .px_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .children(self.sorted_panels().into_iter().map(|panel| {
                        let selected = active.as_ref() == Some(&panel.id);
                        let badge = panel.badge.as_ref().and_then(|badge| badge(cx)).filter(|n| *n > 0);
                        let id = panel.id.clone();
                        let label = match badge {
                            Some(count) => format!("{} {count}", panel.title.to_uppercase()),
                            None => panel.title.to_uppercase(),
                        };
                        Button::new(SharedString::from(format!("panel-tab-{}", panel.id)))
                            .ghost()
                            .xsmall()
                            .label(label)
                            .selected(selected)
                            .on_click(cx.listener(move |this, _, _, cx| this.show_panel(&id.clone(), cx)))
                    }))
                    .child(div().flex_1())
                    .when_some(actions, |el, actions| el.child(actions(window, cx)))
                    .child(
                        Button::new("panel-close")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(Lucide::X))
                            .tooltip("Close Panel")
                            .on_click(cx.listener(|this, _, window, cx| this.hide_panel(window, cx))),
                    ),
            )
            .child(div().flex_1().min_h_0().w_full().overflow_hidden().when_some(view, |el, view| el.child(view)))
            .into_any_element()
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let mut items: Vec<&(StatusItem, AnyView)> = self.status_items.iter().collect();
        items.sort_by_key(|(item, _)| item.order);
        let side = |align: StatusAlign| items.iter().filter(move |(item, _)| item.align == align).map(|(_, view)| view.clone()).collect::<Vec<_>>();
        h_flex()
            .id("status-bar")
            .w_full()
            .h(px(24.))
            .flex_none()
            .px_2()
            .gap_2()
            .text_xs()
            .bg(theme.status_bar)
            .border_t_1()
            .border_color(theme.status_bar_border)
            .children(side(StatusAlign::Left))
            .child(div().flex_1())
            .children(side(StatusAlign::Right))
    }
}

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = if self.sidebar_visible { self.sidebar_view(window, cx) } else { None };
        let sidebar_title = self.active_sidebar.as_ref().and_then(|id| self.sidebar_views.iter().find(|v| &v.id == id)).map(|v| v.title.to_uppercase());
        let panel_visible = self.panel_visible && !self.panel_views.is_empty();
        let panel = if panel_visible { Some(self.render_panel(window, cx)) } else { None };
        let editor_area = self.render_editor_area(cx);
        let theme = cx.theme();

        let center = v_resizable("workbench-center")
            .child(resizable_panel().child(editor_area))
            .child(resizable_panel().size(px(260.)).visible(panel_visible).child(panel.unwrap_or_else(|| div().into_any_element())));

        let on_right = self.sidebar_on_right();
        let sidebar_el = v_flex()
            .id("sidebar")
            .size_full()
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground)
            .map(|el| if on_right { el.border_l_1() } else { el.border_r_1() })
            .border_color(theme.border)
            .when_some(sidebar_title, |el, title| {
                el.child(h_flex().h(px(34.)).flex_none().px_3().text_xs().text_color(theme.muted_foreground).child(SharedString::from(title)))
            })
            .child(div().flex_1().min_h_0().w_full().overflow_hidden().when_some(sidebar.clone(), |el, view| el.child(view)));

        v_flex()
            .id("workbench")
            .size_full()
            .track_focus(&self.focus_handle)
            .bg(theme.background)
            .text_color(theme.foreground)
            .child({
                // Left: activity bar | sidebar | editor. Right: editor | sidebar | activity bar.
                // A different id per side so the saved sizes do not get mixed up between panels.
                let sidebar_panel = resizable_panel().size(px(280.)).visible(sidebar.is_some()).child(sidebar_el);
                let main = if on_right {
                    h_resizable("workbench-main-right").child(resizable_panel().child(center)).child(sidebar_panel)
                } else {
                    h_resizable("workbench-main").child(sidebar_panel).child(resizable_panel().child(center))
                };
                let main = div().flex_1().h_full().min_w_0().child(main);
                let row = h_flex().flex_1().min_h_0().w_full().items_start();
                if on_right {
                    row.child(main).child(self.render_activity_bar(cx))
                } else {
                    row.child(self.render_activity_bar(cx)).child(main)
                }
            })
            .child(self.render_status_bar(cx))
    }
}

/// Releases the window registration when the `Workbench` is released.
fn release(workbench: &mut Workbench, cx: &mut App) {
    // A replacement `Workbench` (another folder in the same window) may already be registered; do not release it.
    commands::detach(workbench.window_id, &workbench.commands, cx);
    if let Some(windows) = cx.try_global::<Windows>() {
        if windows.0.get(&workbench.window_id).is_some_and(|w| w.upgrade().is_none()) {
            cx.global_mut::<Windows>().0.remove(&workbench.window_id);
        }
    }
}

struct MissingTab {
    kind: SharedString,
    focus_handle: FocusHandle,
}

impl Focusable for MissingTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MissingTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .track_focus(&self.focus_handle)
            .flex()
            .items_center()
            .justify_center()
            .text_color(cx.theme().muted_foreground)
            .child(SharedString::from(format!("This tab type is not available: {}", self.kind)))
    }
}
