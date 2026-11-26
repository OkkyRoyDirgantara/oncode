//! The bottom "Terminal" panel: groups of terminals shown side by side (split), the terminal list on
//! the right, and drag and drop between the panel and the editor area.
//!
//! All of this window's terminals are owned by this panel, including those shown as tabs in the editor
//! area (tab kind [`TAB_KIND`]). A terminal is killed as soon as it is no longer in the panel or in the
//! editor area (e.g. its tab is closed). Calls to `Workbench` from here are always deferred
//! (`window.defer`): opening a terminal tab builds its content through this panel, which must not
//! be borrowed at that moment.

use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui_kit::component::button::Button;
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::resizable::{h_resizable, resizable_panel};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, relative, AnyElement, App, AppContext as _, Context, DragMoveEvent, Entity, FocusHandle, Focusable, Global, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, TestSupportExt as _, WeakEntity, Window,
};
use oncode_ui::model::{drop_zone, DropZone, SplitSide};
use oncode_ui::{DraggedTab, Env, Lucide, OpenOptions, TabContent, TabData, TabDragPreview, TabKind, ViewTab, Workbench, WorkbenchEvent};
use serde_json::json;

use crate::layout::{Layout, PanelDrop, SessionId};
use crate::view::{SessionEvent, TerminalSession};

/// The terminal tab kind in the editor area.
pub const TAB_KIND: &str = "terminal";

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

/// An id for a new terminal (unique across the whole application, so its tab id is unique too).
pub fn fresh_session_id() -> SessionId {
    NEXT_SESSION.fetch_add(1, Ordering::Relaxed)
}

/// An editor-area tab for a terminal. A terminal that does not exist yet is created when its tab is built.
pub fn session_tab(id: SessionId, title: &str) -> TabData {
    TabData::new(TAB_KIND, title, json!({ "session": id }))
}

/// The terminal id of a terminal tab.
pub fn tab_session(tab: &TabData) -> Option<SessionId> {
    if tab.kind != TAB_KIND {
        return None;
    }
    tab.params.get("session")?.as_u64()
}

/// What is being dragged is a terminal (from the panel list or an editor-area tab).
fn is_terminal_drag(value: &dyn Any) -> bool {
    value.downcast_ref::<DraggedTab>().is_some_and(|dragged| dragged.tab.kind == TAB_KIND)
}

pub struct TerminalPanel {
    env: Env,
    /// All of this window's terminals, in the panel and in the editor area.
    sessions: HashMap<SessionId, Entity<TerminalSession>>,
    /// The layout of the terminals in the panel.
    layout: Layout,
    /// Terminals shown as tabs in the editor area.
    in_editor: Vec<SessionId>,
    /// The terminal that received focus last (in the panel or the editor area): the target of commands
    /// like Kill and Clear.
    last_focused: Option<SessionId>,
    next_number: usize,
    /// The drop zone while a terminal is dragged over a panel terminal.
    pane_drop: Option<(SessionId, DropZone)>,
    focus_handle: FocusHandle,
    session_subscriptions: HashMap<SessionId, Vec<Subscription>>,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for TerminalPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.active_session() {
            Some(session) => session.focus_handle(cx),
            None => self.focus_handle.clone(),
        }
    }
}

impl TerminalPanel {
    pub fn new(env: Env, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        if let Some(workbench) = env.workbench.upgrade() {
            subscriptions.push(cx.subscribe_in(&workbench, window, |this, workbench, event: &WorkbenchEvent, window, cx| {
                if matches!(event, WorkbenchEvent::TabsChanged) {
                    this.sync_editor_tabs(workbench, window, cx);
                }
            }));
        }
        Self {
            env,
            sessions: HashMap::new(),
            layout: Layout::default(),
            in_editor: Vec::new(),
            last_focused: None,
            next_number: 1,
            pane_drop: None,
            focus_handle: cx.focus_handle(),
            session_subscriptions: HashMap::new(),
            _subscriptions: subscriptions,
        }
    }

    // ----- State -----

    /// The terminals in the panel, in list order.
    pub fn sessions(&self) -> Vec<Entity<TerminalSession>> {
        self.layout.order().iter().filter_map(|id| self.sessions.get(id).cloned()).collect()
    }

    /// The terminals shown as tabs in the editor area.
    pub fn editor_sessions(&self) -> Vec<Entity<TerminalSession>> {
        self.in_editor.iter().filter_map(|id| self.sessions.get(id).cloned()).collect()
    }

    /// There are terminals, in the panel or in the editor area.
    pub fn has_sessions(&self) -> bool {
        !self.sessions.is_empty()
    }

    pub fn session(&self, id: SessionId) -> Option<&Entity<TerminalSession>> {
        self.sessions.get(&id)
    }

    /// The active terminal in the panel.
    pub fn active_session(&self) -> Option<&Entity<TerminalSession>> {
        self.layout.active.and_then(|id| self.sessions.get(&id))
    }

    /// The terminals currently shown side by side in the panel (the active terminal's group).
    pub fn visible_sessions(&self) -> Vec<Entity<TerminalSession>> {
        self.layout.active_group().iter().filter_map(|id| self.sessions.get(id).cloned()).collect()
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn is_in_editor(&self, id: SessionId) -> bool {
        self.in_editor.contains(&id)
    }

    /// The terminal that is focused.
    pub fn focused(&self, window: &Window, cx: &App) -> Option<SessionId> {
        self.sessions.iter().find(|(_, session)| session.read(cx).focus_handle_is_focused(window)).map(|(id, _)| *id)
    }

    /// The target of terminal commands: the focused one, the one focused last, then the one active in the panel.
    /// `panel_only`: only the panel's terminal (the buttons in the panel bar).
    pub fn target(&self, panel_only: bool, window: &Window, cx: &App) -> Option<SessionId> {
        if panel_only {
            return self.layout.active;
        }
        self.focused(window, cx).or(self.last_focused.filter(|id| self.sessions.contains_key(id))).or(self.layout.active)
    }

    // ----- Creating and killing -----

    fn create_session(&mut self, id: SessionId, window: &mut Window, cx: &mut Context<Self>) -> Entity<TerminalSession> {
        let number = self.next_number;
        self.next_number += 1;
        let env = self.env.clone();
        let session = cx.new(|cx| TerminalSession::new(env, id, number, window, cx));
        let subscriptions = vec![
            cx.observe(&session, |_, _, cx| cx.notify()),
            cx.subscribe_in(&session, window, move |this, _, event: &SessionEvent, window, cx| this.on_session_event(id, event, window, cx)),
        ];
        self.session_subscriptions.insert(id, subscriptions);
        self.sessions.insert(id, session.clone());
        session
    }

    /// The terminal with this id; created if it does not exist yet (a new terminal tab in the editor area).
    pub fn session_or_create(&mut self, id: SessionId, window: &mut Window, cx: &mut Context<Self>) -> Entity<TerminalSession> {
        match self.sessions.get(&id) {
            Some(session) => session.clone(),
            None => self.create_session(id, window, cx),
        }
    }

    /// A new terminal in the project folder as its own group in the panel, focused right away.
    pub fn new_session(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<TerminalSession> {
        let id = fresh_session_id();
        let session = self.create_session(id, window, cx);
        self.layout.place(id, PanelDrop::Alone(None));
        self.focus_active(window, cx);
        cx.notify();
        session
    }

    /// A new terminal beside the panel terminal `beside` (default: the active one), like VS Code.
    pub fn split(&mut self, beside: Option<SessionId>, window: &mut Window, cx: &mut Context<Self>) -> Entity<TerminalSession> {
        let Some(target) = beside.filter(|id| self.layout.contains(*id)).or(self.layout.active) else {
            return self.new_session(window, cx);
        };
        let id = fresh_session_id();
        let session = self.create_session(id, window, cx);
        self.layout.place(id, PanelDrop::Beside(target, true));
        self.focus_active(window, cx);
        cx.notify();
        session
    }

    /// Takes a terminal out of its split group.
    pub fn unsplit(&mut self, id: SessionId, window: &mut Window, cx: &mut Context<Self>) {
        self.layout.unsplit(id);
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Makes sure there is at least one terminal in the panel (when the panel is opened).
    pub fn ensure_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.layout.is_empty() {
            self.new_session(window, cx);
        } else {
            self.focus_active(window, cx);
        }
    }

    pub fn focus_active(&self, window: &mut Window, cx: &mut App) {
        if let Some(session) = self.active_session() {
            let handle = session.focus_handle(cx);
            window.defer(cx, move |window, cx| handle.focus(window, cx));
        }
    }

    /// Kills a terminal. For a terminal in the editor area its tab is closed (which kills it).
    pub fn kill(&mut self, id: SessionId, window: &mut Window, cx: &mut Context<Self>) {
        if self.in_editor.contains(&id) {
            let tab_id = session_tab(id, "").id;
            self.with_workbench(window, cx, move |workbench, window, cx| workbench.force_close_where(|tab| tab.id == tab_id, window, cx));
            return;
        }
        self.layout.remove(id);
        self.drop_session(id, cx);
        self.focus_active(window, cx);
        cx.notify();
    }

    fn drop_session(&mut self, id: SessionId, cx: &mut Context<Self>) {
        if let Some(session) = self.sessions.remove(&id) {
            session.update(cx, |session, cx| session.kill(cx));
        }
        self.session_subscriptions.remove(&id);
        if self.last_focused == Some(id) {
            self.last_focused = None;
        }
        if self.pane_drop.is_some_and(|(pane, _)| pane == id) {
            self.pane_drop = None;
        }
    }

    pub fn kill_active(&mut self, panel_only: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.target(panel_only, window, cx) {
            self.kill(id, window, cx);
        }
    }

    pub fn clear_active(&mut self, panel_only: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(session) = self.target(panel_only, window, cx).and_then(|id| self.sessions.get(&id)) {
            session.update(cx, |session, cx| session.clear(cx));
        }
    }

    pub fn focus_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(next) = self.layout.cycle(1) {
            self.layout.active = Some(next);
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    pub fn select(&mut self, id: SessionId, window: &mut Window, cx: &mut Context<Self>) {
        if self.layout.contains(id) {
            self.layout.active = Some(id);
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    // ----- Panel and editor area -----

    /// Runs `f` on the `Workbench` after the update that is in progress has finished.
    fn with_workbench(&self, window: &mut Window, cx: &mut App, f: impl FnOnce(&mut Workbench, &mut Window, &mut Context<Workbench>) + 'static) {
        let Some(workbench) = self.env.workbench.upgrade() else { return };
        window.defer(cx, move |window, cx| workbench.update(cx, |workbench, cx| f(workbench, window, cx)));
    }

    /// Moves a panel terminal to the editor area as a tab in the active group.
    pub fn move_to_editor(&mut self, id: SessionId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.sessions.get(&id) else { return };
        let tab = session_tab(id, session.read(cx).title());
        // The panel removes it by itself once its tab is open (`sync_editor_tabs`).
        self.with_workbench(window, cx, move |workbench, window, cx| {
            workbench.open_tab(tab, OpenOptions::default(), window, cx);
        });
    }

    /// Moves a terminal (from the editor area, or within the panel) to this place in the panel.
    pub fn move_to_panel(&mut self, id: SessionId, place: PanelDrop, window: &mut Window, cx: &mut Context<Self>) {
        if !self.sessions.contains_key(&id) {
            return;
        }
        self.pane_drop = None;
        self.layout.place(id, place);
        if self.in_editor.contains(&id) {
            self.in_editor.retain(|x| *x != id);
            let tab_id = session_tab(id, "").id;
            self.with_workbench(window, cx, move |workbench, window, cx| workbench.force_close_where(|tab| tab.id == tab_id, window, cx));
        }
        // Deferred after the tab closing, which focuses the next editor tab.
        self.focus_active(window, cx);
        cx.notify();
    }

    fn drop_in_panel(&mut self, dragged: &DraggedTab, place: PanelDrop, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = tab_session(&dragged.tab) {
            self.move_to_panel(id, place, window, cx);
        }
    }

    /// Syncs the panel with the terminal tabs open in the editor area: a terminal that moved to the
    /// editor area is removed from the panel, a terminal that is nowhere is killed, and the tab title
    /// follows the terminal's title.
    fn sync_editor_tabs(&mut self, workbench: &Entity<Workbench>, window: &mut Window, cx: &mut Context<Self>) {
        let tabs: Vec<(SessionId, String)> = workbench.read(cx).model().all_tabs().filter_map(|tab| Some((tab_session(tab)?, tab.title.clone()))).collect();
        self.in_editor = tabs.iter().map(|(id, _)| *id).collect();
        self.in_editor.dedup();
        for id in self.in_editor.clone() {
            self.layout.remove(id);
        }
        let orphans: Vec<SessionId> = self.sessions.keys().copied().filter(|id| !self.layout.contains(*id) && !self.in_editor.contains(id)).collect();
        for id in orphans {
            self.drop_session(id, cx);
        }
        let renames: Vec<(String, String)> = tabs
            .iter()
            .filter_map(|(id, title)| {
                let current = self.sessions.get(id)?.read(cx).title().to_string();
                (current != *title).then(|| (session_tab(*id, "").id, current))
            })
            .collect();
        self.rename_tabs(renames, window, cx);
        cx.notify();
    }

    fn rename_tabs(&self, renames: Vec<(String, String)>, window: &mut Window, cx: &mut App) {
        if renames.is_empty() {
            return;
        }
        self.with_workbench(window, cx, move |workbench, window, cx| {
            workbench.update_tabs(
                |tab| renames.iter().find(|(id, title)| *id == tab.id && *title != tab.title).map(|(_, title)| TabData { title: title.clone(), ..tab.clone() }),
                window,
                cx,
            );
        });
    }

    fn on_session_event(&mut self, id: SessionId, event: &SessionEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            SessionEvent::Focused => {
                self.last_focused = Some(id);
                if self.layout.contains(id) && self.layout.active != Some(id) {
                    self.layout.active = Some(id);
                }
            }
            SessionEvent::TitleChanged => {
                if let Some(session) = self.sessions.get(&id).filter(|_| self.in_editor.contains(&id)) {
                    let title = session.read(cx).title().to_string();
                    self.rename_tabs(vec![(session_tab(id, "").id, title)], window, cx);
                }
            }
            SessionEvent::Exited => {}
        }
        cx.notify();
    }

    /// Computes the drop zone while a terminal is dragged over the panel terminal `pane`: the left/right
    /// edge splits on that side, the middle makes it its own terminal.
    fn track_pane_drop(&mut self, pane: SessionId, event: &DragMoveEvent<DraggedTab>, cx: &mut Context<Self>) {
        let (bounds, position) = (event.bounds, event.event.position);
        let dragged = tab_session(&event.drag(cx).tab);
        let zone = if dragged.is_some() && dragged != Some(pane) && bounds.contains(&position) {
            let x = (position.x - bounds.origin.x) / bounds.size.width;
            Some((pane, drop_zone(x, 0.5, true, false)))
        } else if self.pane_drop.is_some_and(|(p, _)| p == pane) {
            None
        } else {
            return;
        };
        if zone != self.pane_drop {
            self.pane_drop = zone;
            cx.notify();
        }
    }

    // ----- Views -----

    fn render_pane(&self, id: SessionId, split: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let session = self.sessions.get(&id).cloned();
        let zone = self.pane_drop.filter(|(pane, _)| *pane == id && cx.has_active_drag()).map(|(_, zone)| zone);
        let overlay = zone.map(|zone| {
            let base = div().absolute().bg(theme.drop_target);
            match zone {
                DropZone::Split(SplitSide::Left) => base.top_0().left_0().h_full().w(relative(0.5)),
                DropZone::Split(_) => base.top_0().right_0().h_full().w(relative(0.5)),
                DropZone::Center => base.top_0().left_0().size_full(),
            }
        });
        // The top line marking the active terminal; always present (transparent) so the grid size does not change.
        let marker = if self.layout.active == Some(id) { theme.primary } else { theme.primary.opacity(0.) };
        div()
            .id(SharedString::from(format!("terminal-pane-{id}")))
            .test_support()
            .relative()
            .size_full()
            .when(split, |el| el.border_t_2().border_color(marker))
            .on_drag_move::<DraggedTab>(cx.listener(move |this, event: &DragMoveEvent<DraggedTab>, _, cx| this.track_pane_drop(id, event, cx)))
            .can_drop(|value, _, _| is_terminal_drag(value))
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                let zone = this.pane_drop.filter(|(pane, _)| *pane == id).map(|(_, zone)| zone);
                this.pane_drop = None;
                if tab_session(&dragged.tab) == Some(id) {
                    cx.notify();
                    return;
                }
                let place = match zone {
                    Some(DropZone::Split(SplitSide::Left)) => PanelDrop::Beside(id, false),
                    Some(DropZone::Split(_)) => PanelDrop::Beside(id, true),
                    _ => PanelDrop::Alone(None),
                };
                this.drop_in_panel(dragged, place, window, cx);
            }))
            .when_some(session, |el, session| el.child(session))
            .children(overlay)
            .into_any_element()
    }

    fn render_entry(&self, id: SessionId, glyph: Option<&'static str>, cx: &mut Context<Self>) -> AnyElement {
        let Some(session) = self.sessions.get(&id) else { return div().into_any_element() };
        let theme = cx.theme();
        let title = session.read(cx).title().clone();
        let selected = self.layout.active == Some(id);
        let in_split = self.layout.group_of(id).is_some_and(|group| self.layout.groups[group].len() > 1);
        let weak = cx.weak_entity();
        h_flex()
            .id(SharedString::from(format!("terminal-tab-{id}")))
            .test_support()
            .flex_none()
            .h(px(24.))
            .mx_1()
            .px_1p5()
            .gap_1()
            .rounded(theme.radius)
            .text_sm()
            .map(|el| if selected { el.bg(theme.list_active).text_color(theme.foreground) } else { el.text_color(theme.muted_foreground).hover(|style| style.bg(theme.list_hover)) })
            .when_some(glyph, |el, glyph| el.child(div().flex_none().w(px(10.)).text_color(theme.muted_foreground).child(glyph)))
            .child(Icon::new(Lucide::SquareTerminal).small())
            .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(title.clone()))
            .on_click(cx.listener(move |this, _, window, cx| this.select(id, window, cx)))
            // Drag to the editor area to make it a tab, or onto another terminal to split.
            .on_drag(DraggedTab { tab: session_tab(id, &title), from: None }, |dragged, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| TabDragPreview::new(dragged.tab.title.clone(), Some(Lucide::SquareTerminal)))
            })
            .can_drop(|value, _, _| is_terminal_drag(value))
            .drag_over::<DraggedTab>(|style, _, _, cx| style.bg(cx.theme().drop_target))
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| this.drop_in_panel(dragged, PanelDrop::Alone(Some(id)), window, cx)))
            .context_menu(move |menu, _, _| {
                let action = |f: fn(&mut TerminalPanel, SessionId, &mut Window, &mut Context<TerminalPanel>)| {
                    let weak = weak.clone();
                    move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                        let _ = weak.update(cx, |this, cx| f(this, id, window, cx));
                    }
                };
                let menu = menu.item(PopupMenuItem::new("Split Terminal").on_click(action(|this, id, window, cx| {
                    this.split(Some(id), window, cx);
                })));
                let menu = if in_split { menu.item(PopupMenuItem::new("Unsplit Terminal").on_click(action(TerminalPanel::unsplit))) } else { menu };
                menu.item(PopupMenuItem::new("Move Terminal into Editor Area").on_click(action(TerminalPanel::move_to_editor)))
                    .separator()
                    .item(PopupMenuItem::new("Kill Terminal").on_click(action(TerminalPanel::kill)))
            })
            .into_any_element()
    }

    fn render_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut entries = Vec::new();
        for group in &self.layout.groups {
            for (index, id) in group.iter().enumerate() {
                // A tree line like VS Code for terminals shown side by side.
                let glyph = match (group.len(), index) {
                    (1, _) => None,
                    (_, 0) => Some("┌"),
                    (len, index) if index + 1 == len => Some("└"),
                    _ => Some("├"),
                };
                entries.push(self.render_entry(*id, glyph, cx));
            }
        }
        let theme = cx.theme();
        v_flex()
            .id("terminal-list")
            .w(px(180.))
            .h_full()
            .flex_none()
            .py_1()
            .gap_0p5()
            .border_l_1()
            .border_color(theme.border)
            .overflow_y_scroll()
            .children(entries)
            // The empty space below the list: a terminal dropped here becomes its own terminal.
            .child(
                div()
                    .id("terminal-list-end")
                    .test_support()
                    .flex_1()
                    .w_full()
                    .min_h(px(24.))
                    .can_drop(|value, _, _| is_terminal_drag(value))
                    .drag_over::<DraggedTab>(|style, _, _, cx| style.bg(cx.theme().drop_target))
                    .on_drop(cx.listener(|this, dragged: &DraggedTab, window, cx| this.drop_in_panel(dragged, PanelDrop::Alone(None), window, cx))),
            )
            .into_any_element()
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let visible: Vec<SessionId> = self.layout.active_group().to_vec();
        let content = match visible.as_slice() {
            [] => div()
                .id("terminal-empty")
                .test_support()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .can_drop(|value, _, _| is_terminal_drag(value))
                .drag_over::<DraggedTab>(|style, _, _, cx| style.bg(cx.theme().drop_target))
                .on_drop(cx.listener(|this, dragged: &DraggedTab, window, cx| this.drop_in_panel(dragged, PanelDrop::Alone(None), window, cx)))
                .child(Button::new("terminal-start").label("New Terminal").small().outline().on_click(cx.listener(|this, _, window, cx| {
                    this.new_session(window, cx);
                })))
                .into_any_element(),
            [only] => self.render_pane(*only, false, cx),
            _ => {
                // The id follows the group layout: sizes are divided evenly again when the contents change.
                let key = visible.iter().map(|id| id.to_string()).collect::<Vec<_>>().join("-");
                h_resizable(SharedString::from(format!("terminal-split-{key}")))
                    .children(visible.iter().map(|id| resizable_panel().child(self.render_pane(*id, true, cx))))
                    .into_any_element()
            }
        };
        let list = (self.layout.order().len() > 1).then(|| self.render_list(cx));
        h_flex()
            .id("terminal-panel")
            .size_full()
            .track_focus(&self.focus_handle)
            .child(div().flex_1().h_full().min_w_0().child(content))
            .children(list)
    }
}

/// This window's terminal panel.
pub fn panel(workbench: &Workbench) -> Option<Entity<TerminalPanel>> {
    workbench.extension::<Entity<TerminalPanel>>()
}

/// The panel of the window that is currently building tabs; filled in via `Workbench::on_before_build_tabs`.
pub(crate) struct CurrentPanel(pub WeakEntity<TerminalPanel>);

impl Global for CurrentPanel {}

/// The content of a terminal tab in the editor area: the same terminal as the one in the panel (its
/// shell process keeps running when it is moved).
pub(crate) struct TerminalKind;

impl TabKind for TerminalKind {
    fn build(&self, tab: &TabData, _: &Env, window: &mut Window, cx: &mut App) -> Box<dyn TabContent> {
        let panel = cx.try_global::<CurrentPanel>().and_then(|current| current.0.upgrade());
        let id = tab_session(tab).unwrap_or_else(fresh_session_id);
        match panel {
            Some(panel) => Box::new(ViewTab(panel.update(cx, |panel, cx| panel.session_or_create(id, window, cx)))),
            None => Box::new(ViewTab(cx.new(|cx| Unavailable { focus_handle: cx.focus_handle() }))),
        }
    }

    fn icon(&self, _: &TabData) -> Option<Lucide> {
        Some(Lucide::SquareTerminal)
    }

    /// "Split Editor" on a terminal opens a new terminal next to it (a single terminal cannot be
    /// shown in two places).
    fn split_copy(&self, _: &TabData) -> Option<TabData> {
        Some(session_tab(fresh_session_id(), "Terminal"))
    }
}

/// A terminal tab in a window without a folder (there are no terminals there).
struct Unavailable {
    focus_handle: FocusHandle,
}

impl Focusable for Unavailable {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Unavailable {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .track_focus(&self.focus_handle)
            .flex()
            .items_center()
            .justify_center()
            .text_color(cx.theme().muted_foreground)
            .child("Open a folder to use the terminal.")
    }
}
