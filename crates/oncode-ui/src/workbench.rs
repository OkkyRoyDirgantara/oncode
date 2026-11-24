//! Jendela utama: activity bar, sidebar, grup editor bertab, panel bawah, dan status bar.
//!
//! Fitur mendaftarkan diri lewat [`register_feature`] (per jendela) dan [`register_tab_kind`]
//! (global). Semua tampilan fitur adalah entitas tersendiri (`AnyView`), sehingga boleh
//! membaca `Workbench` di `render` mereka. Yang **tidak** boleh: membaca atau memperbarui
//! entitas `Workbench` dari dalam fungsi yang dipanggil `Workbench` sendiri (pembangun tampilan,
//! `TabKind::build`, `TabContent::can_close`), karena saat itu entitasnya sedang dipinjam.

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
    div, px, AnyElement, AnyView, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, Global, InteractiveElement as _,
    IntoElement, MouseButton, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, TestSupportExt as _, WeakEntity,
    Window,
};
use oncode_core::api::app::WorkspaceInfo;
use oncode_core::settings::SidebarPosition;
use oncode_core::{Settings, Shared};

use crate::commands::{self, Command, CommandRegistry};
use crate::model::{Model, OpenOptions, Orientation, TabData};
use crate::Lucide;

/// Konteks sebuah jendela, diberikan ke semua pembangun tampilan. Tidak berubah selama
/// `Workbench` hidup: membuka folder lain atau mengubah kepercayaan membuat `Workbench` baru.
#[derive(Clone)]
pub struct Env {
    pub workbench: WeakEntity<Workbench>,
    pub core: Shared,
    /// Label jendela untuk API `oncode-core`.
    pub window: SharedString,
    /// Folder proyek; `None` bila jendela belum membuka folder.
    pub root: Option<PathBuf>,
    /// Folder dipercaya: Git, LSP, dan pengaturan proyek aktif.
    pub trusted: bool,
    /// Akar repositori yang memuat folder ini, bila ada dan folder dipercaya.
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

/// Isi sebuah tab yang sudah dibangun.
pub trait TabContent: 'static {
    fn view(&self) -> AnyView;
    /// Memindahkan fokus keyboard ke isi tab.
    fn focus(&self, window: &mut Window, cx: &mut App);
    /// Tab punya perubahan yang belum disimpan.
    fn is_dirty(&self, _cx: &App) -> bool {
        false
    }
    /// Dipanggil sebelum tab ditutup; hasil `false` membatalkan.
    fn can_close(&self, _window: &mut Window, _cx: &mut App) -> Task<bool> {
        Task::ready(true)
    }
    /// Memanggil `on_change` saat keadaan yang memengaruhi bilah tab (mis. dirty) berubah.
    fn observe(&self, _on_change: Rc<dyn Fn(&mut App)>, _cx: &mut App) -> Option<Subscription> {
        None
    }
    /// Untuk fitur yang perlu mengenali isi tab miliknya sendiri (`downcast_ref`).
    fn as_any(&self) -> &dyn std::any::Any;
}

/// `TabContent` untuk tampilan biasa yang bisa difokus.
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

/// Pembangun isi tab untuk satu jenis (`TabData::kind`).
pub trait TabKind: 'static {
    fn build(&self, tab: &TabData, env: &Env, window: &mut Window, cx: &mut App) -> Box<dyn TabContent>;
    fn icon(&self, _tab: &TabData) -> Option<Lucide> {
        None
    }
}

type BuildView = Rc<dyn Fn(&Env, &mut Window, &mut App) -> AnyView>;
type BadgeFn = Rc<dyn Fn(&App) -> Option<usize>>;

/// Tampilan di sidebar, dipilih lewat activity bar.
#[derive(Clone)]
pub struct SidebarView {
    pub id: SharedString,
    pub title: SharedString,
    pub icon: Lucide,
    pub order: i32,
    pub build: BuildView,
    /// Angka kecil pada ikon (mis. jumlah perubahan).
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

/// Tampilan di panel bawah.
#[derive(Clone)]
pub struct PanelView {
    pub id: SharedString,
    pub title: SharedString,
    pub order: i32,
    pub build: BuildView,
    pub badge: Option<BadgeFn>,
    /// Aksi di sisi kanan bilah tab panel.
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StatusAlign {
    Left,
    Right,
}

/// Item status bar; berupa tampilan tersendiri agar bisa mengamati state yang dibutuhkannya.
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
    /// Tab aktif (atau grup aktif) berganti.
    ActiveTabChanged,
    /// Daftar tab berubah (buka, tutup, ganti nama).
    TabsChanged,
    /// Sidebar atau panel bawah ditampilkan, disembunyikan, atau diganti.
    LayoutChanged,
    /// Pengaturan efektif jendela ini berubah; baca lewat [`Workbench::settings`].
    SettingsChanged,
    /// Berkas di folder proyek berubah di disk (path absolut). `overflow`: terlalu banyak untuk dirinci.
    FsChanged { paths: Vec<String>, overflow: bool },
    /// Sebuah berkas baru saja disimpan dari editor (path absolut).
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

/// Mendaftarkan fitur. `setup` dipanggil untuk setiap `Workbench` baru (setiap jendela, dan
/// setiap kali folder dibuka), saat entitasnya sedang dibuat.
pub fn register_feature(cx: &mut App, setup: impl Fn(&mut Workbench, &mut Window, &mut Context<Workbench>) + 'static) {
    cx.global_mut::<Features>().0.push(Rc::new(setup));
}

/// Mendaftarkan pembangun isi tab untuk sebuah jenis tab.
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
    panel_built: HashMap<SharedString, AnyView>,
    active_panel: Option<SharedString>,
    panel_visible: bool,
    status_items: Vec<(StatusItem, AnyView)>,
    /// Ditampilkan di area editor saat tidak ada tab.
    empty_view: Option<AnyView>,
    focus_handle: FocusHandle,
    window_id: gpui_kit::WindowId,
    /// State per jendela milik fitur (mis. registry buffer), dicari menurut tipenya.
    extensions: HashMap<std::any::TypeId, Box<dyn std::any::Any>>,
    /// Dipanggil tepat sebelum isi tab dibangun, agar `TabKind` global tahu jendela mana yang membangun.
    before_build: Vec<Rc<dyn Fn(&mut App)>>,
}

impl EventEmitter<WorkbenchEvent> for Workbench {}

impl Focusable for Workbench {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Workbench {
    /// Membuat `Workbench` untuk jendela ini dan menjalankan pendaftaran semua fitur.
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
            panel_built: HashMap::new(),
            active_panel: None,
            panel_visible: false,
            status_items: Vec::new(),
            empty_view: None,
            focus_handle: cx.focus_handle(),
            window_id,
            extensions: HashMap::new(),
            before_build: Vec::new(),
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

    /// `Workbench` milik sebuah jendela.
    pub fn for_window(window: &Window, cx: &App) -> Option<Entity<Workbench>> {
        cx.try_global::<Windows>()?.0.get(&window.window_handle().window_id())?.upgrade()
    }

    pub fn env(&self) -> &Env {
        &self.env
    }

    pub fn model(&self) -> &Model {
        &self.model
    }

    /// Pengaturan efektif jendela ini (pengaturan proyek sudah diterapkan bila folder dipercaya).
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

    /// Menyiarkan event ke semua pelanggan `Workbench` (dipakai `Shell` dan editor).
    pub fn broadcast(&mut self, event: WorkbenchEvent, cx: &mut Context<Self>) {
        cx.emit(event);
    }

    /// Menyimpan state per jendela milik sebuah fitur (biasanya `Entity<..>`), dicari menurut tipenya.
    pub fn set_extension<T: 'static>(&mut self, value: T) {
        self.extensions.insert(std::any::TypeId::of::<T>(), Box::new(value));
    }

    pub fn extension<T: Clone + 'static>(&self) -> Option<T> {
        self.extensions.get(&std::any::TypeId::of::<T>()).and_then(|value| value.downcast_ref::<T>()).cloned()
    }

    /// Mendaftarkan fungsi yang dipanggil tepat sebelum `TabKind::build`. `TabKind` terdaftar global,
    /// jadi fitur memakai ini untuk menunjuk state per jendela yang harus dipakai pembangunnya.
    pub fn on_before_build_tabs(&mut self, f: impl Fn(&mut App) + 'static) {
        self.before_build.push(Rc::new(f));
    }

    /// Isi tab yang sedang aktif, bila sudah dibangun.
    pub fn active_content(&self) -> Option<&dyn TabContent> {
        let (group, tab) = self.model.active()?;
        self.slots.get(&(group.id, tab.id.clone())).map(|slot| slot.content.as_ref())
    }

    /// Id tampilan sidebar yang terpilih, terlepas dari terlihat atau tidak.
    pub fn selected_sidebar(&self) -> Option<&SharedString> {
        self.active_sidebar.as_ref()
    }

    // ----- Pendaftaran -----

    pub fn register_command(&mut self, command: Command) {
        self.commands.borrow_mut().register(command);
    }

    pub fn register_sidebar(&mut self, view: SidebarView) {
        self.sidebar_views.retain(|v| v.id != view.id);
        self.sidebar_views.push(view);
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

    /// Tampilan area editor saat tidak ada tab terbuka (mis. layar awal).
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
                // Disimpan ke pengaturan pengguna; semua jendela mengikuti lewat `SettingsChanged`.
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
                if let Some(tab) = this.model.active().map(|(_, t)| t.clone()) {
                    this.open_tab(tab, OpenOptions::to_side(), window, cx);
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

    // ----- Tab -----

    /// Membuka tab (atau mengaktifkannya bila sudah ada) dan mengembalikan id-nya.
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

    /// Mengubah tab di semua grup (mis. setelah berkas diganti nama).
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

    /// Menutup tab setelah isinya mengizinkan (kecuali `force`). Hasil `false` berarti dibatalkan.
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

    /// Menutup tab yang cocok di semua grup tanpa konfirmasi (mis. berkasnya sudah dihapus).
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

    /// Ada tab dengan perubahan yang belum disimpan.
    pub fn has_dirty_tabs(&self, cx: &App) -> bool {
        self.slots.values().any(|slot| slot.content.is_dirty(cx))
    }

    /// Menyamakan isi tab yang sudah dibangun dengan model.
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

    // ----- Sidebar dan panel -----

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

    // ----- Tampilan -----

    /// Activity bar dan sidebar berada di kanan jendela.
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
            // Garis pemisah selalu di sisi yang menghadap editor.
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
        h_flex()
            .id(SharedString::from(format!("tabs-{group}")))
            .w_full()
            .h(px(34.))
            .flex_none()
            .bg(theme.tab_bar)
            .border_b_1()
            .border_color(theme.border)
            .overflow_x_scroll()
            .children(model_group.tabs.iter().map(|tab| {
                let selected = model_group.active.as_deref() == Some(tab.id.as_str());
                let dirty = self.slots.get(&(group, tab.id.clone())).is_some_and(|slot| slot.content.is_dirty(cx));
                let icon = kinds.get(&tab.kind).and_then(|kind| kind.icon(tab));
                let id = tab.id.clone();
                let close_id = tab.id.clone();
                let pin_id = tab.id.clone();
                let menu_id = tab.id.clone();
                let weak = cx.weak_entity();
                h_flex()
                    .id(SharedString::from(format!("tab-{group}-{}", tab.id)))
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
                    .when_some(icon, |el, icon| el.child(Icon::new(icon).small()))
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
                                    // Titik: ada perubahan yang belum disimpan.
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
            .into_any_element()
    }

    fn render_group(&self, group: u32, cx: &mut Context<Self>) -> AnyElement {
        let content = self.model.group(group).and_then(|g| g.active.clone()).and_then(|id| self.slots.get(&(group, id))).map(|slot| slot.content.view());
        v_flex()
            .id(SharedString::from(format!("group-{group}")))
            .size_full()
            .overflow_hidden()
            .capture_any_mouse_down(cx.listener(move |this, _, _, cx| this.activate_group(group, cx)))
            .child(self.render_tab_strip(group, cx))
            .child(div().flex_1().min_h_0().w_full().overflow_hidden().when_some(content, |el, view| el.child(view)))
            .into_any_element()
    }

    fn render_editor_area(&self, cx: &mut Context<Self>) -> AnyElement {
        let has_tabs = self.model.all_tabs().next().is_some();
        if !has_tabs {
            return div()
                .id("editor-empty")
                .size_full()
                .bg(cx.theme().background)
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
                // Kiri: activity bar | sidebar | editor. Kanan: editor | sidebar | activity bar.
                // Id berbeda per sisi agar ukuran yang tersimpan tidak tertukar antar panel.
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

/// Melepas pendaftaran jendela saat `Workbench` dilepas.
fn release(workbench: &mut Workbench, cx: &mut App) {
    // `Workbench` pengganti (folder lain di jendela yang sama) mungkin sudah terdaftar; jangan dilepas.
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
