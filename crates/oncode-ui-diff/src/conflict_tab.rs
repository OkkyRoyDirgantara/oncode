//! Tab editor konflik: Current | Result | Incoming. Hanya panel Result yang bisa disunting.
//!
//! Komponen `Diff` punya dukungan konflik bawaan (`DiffFile::parse_conflicts`), tetapi hasilnya
//! tidak bisa disunting bebas; di sini tiga editor dipakai agar panel Result tetap editor biasa.
//! Rentang tiap konflik di panel Result dijaga oleh `ConflictState`.

use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Editor, EditorState, GutterDecoration, GutterDecorationCollection, InputEditsEvent, RangeDecoration, RangeDecorationCollection, RangeDecorationStyle, Rope};
use gpui_kit::component::resizable::{h_resizable, resizable_panel};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Disableable as _, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, relative, App, AppContext as _, Context, Entity, FocusHandle, Focusable, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Task, Window,
};
use oncode_core::api::git;
use oncode_core::AppError;
use oncode_git::{ConflictDetail, ConflictKind, ConflictSide};
use oncode_ui::editor::language_for_path;
use oncode_ui::overlay::{self, Confirm};
use oncode_ui::{repos, spawn_core, Env, Lucide, TabContent, TabData, TabKind};
use serde_json::json;

use crate::conflict_state::{build_initial_result, minimal_edit, normalize, ConflictChoice, ConflictState, InitialResult};
use crate::util::base_name;

/// Tab konflik untuk sebuah berkas.
pub fn conflict_tab(root: &str, path: &str) -> TabData {
    TabData::new("conflict", format!("{} (Conflict)", base_name(path)), json!({ "root": root, "path": path })).with_id(format!("conflict:{root}:{path}")).with_description(path.to_string())
}

fn simple_text(kind: ConflictKind) -> &'static str {
    match kind {
        ConflictKind::BothModified => "Both sides changed this file and it cannot be merged as text.",
        ConflictKind::BothAdded => "Both sides added a file with this name.",
        ConflictKind::BothDeleted => "Both sides deleted (or renamed away) this file.",
        ConflictKind::AddedByUs => "This file was added only on the current side.",
        ConflictKind::AddedByThem => "This file was added only on the incoming side.",
        ConflictKind::DeletedByUs => "The current side deleted this file, the incoming side changed it.",
        ConflictKind::DeletedByThem => "The incoming side deleted this file, the current side changed it.",
    }
}

fn choice_key(choice: ConflictChoice) -> &'static str {
    match choice {
        ConflictChoice::Current => "current",
        ConflictChoice::Incoming => "incoming",
        ConflictChoice::Both => "both",
        ConflictChoice::Base => "base",
    }
}

/// Dekorasi milik editor konflik pada ketiga panel.
struct Decorations {
    result_fill: RangeDecorationCollection,
    result_gutter: GutterDecorationCollection,
    current_fill: RangeDecorationCollection,
    incoming_fill: RangeDecorationCollection,
}

pub struct ConflictView {
    env: Env,
    root: String,
    path: String,
    detail: Option<ConflictDetail>,
    error: Option<String>,
    /// `None` untuk konflik non-teks (biner, atau salah satu sisi menghapus berkas).
    initial: Option<InitialResult>,
    model: ConflictState,
    current: Entity<EditorState>,
    result: Entity<EditorState>,
    incoming: Entity<EditorState>,
    decorations: Decorations,
    /// Isi panel Result yang sudah tercermin di `model`.
    synced: Rope,
    /// Isi panel Result sudah diubah tetapi belum ditandai selesai.
    touched: bool,
    saving: bool,
    focus_handle: FocusHandle,
    _load: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl ConflictView {
    pub fn new(root: String, path: String, env: Env, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let language = language_for_path(&path).highlighter;
        let editor = |window: &mut Window, cx: &mut Context<Self>| cx.new(|cx| EditorState::new(window, cx).language(language).line_number(true).searchable(true));
        let (current, result, incoming) = (editor(window, cx), editor(window, cx), editor(window, cx));
        let decorations = Decorations {
            result_fill: result.update(cx, |state, cx| state.create_range_decorations_collection(Vec::new(), cx)),
            result_gutter: result.update(cx, |state, cx| state.create_gutter_decorations_collection(Vec::new(), cx)),
            current_fill: current.update(cx, |state, cx| state.create_range_decorations_collection(Vec::new(), cx)),
            incoming_fill: incoming.update(cx, |state, cx| state.create_range_decorations_collection(Vec::new(), cx)),
        };
        let subscriptions = vec![
            cx.subscribe(&result, |this, _, event: &InputEditsEvent, cx| this.on_result_edits(event, cx)),
            // Daftar konflik repositori menentukan apakah berkas ini masih perlu diselesaikan.
            cx.observe(&repos(cx), |_, _, cx| cx.notify()),
        ];
        let mut this = Self {
            env,
            root,
            path,
            detail: None,
            error: None,
            initial: None,
            model: ConflictState::default(),
            current,
            result,
            incoming,
            decorations,
            synced: Rope::from(""),
            touched: false,
            saving: false,
            focus_handle: cx.focus_handle(),
            _load: Task::ready(()),
            _subscriptions: subscriptions,
        };
        this.load(window, cx);
        this
    }

    pub fn is_loaded(&self) -> bool {
        self.detail.is_some() || self.error.is_some()
    }

    /// Jumlah konflik dan yang belum diselesaikan.
    pub fn counts(&self) -> (usize, usize) {
        (self.model.conflicts.len(), self.model.unresolved())
    }

    pub fn is_touched(&self) -> bool {
        self.touched
    }

    /// State editor panel Result.
    pub fn result(&self) -> &Entity<EditorState> {
        &self.result
    }

    fn tab_id(&self) -> String {
        format!("conflict:{}:{}", self.root, self.path)
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (core, root, path) = (self.env.core.clone(), self.root.clone(), self.path.clone());
        let task = spawn_core(cx, async move { git::git_conflict(&core, root, path).await });
        self._load = cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| this.install(result, window, cx));
        });
    }

    fn install(&mut self, result: Result<ConflictDetail, AppError>, window: &mut Window, cx: &mut Context<Self>) {
        match result {
            Ok(detail) => {
                self.initial = (!detail.segments.is_empty()).then(|| build_initial_result(&detail.segments));
                if let Some(initial) = &self.initial {
                    self.model = ConflictState::new(initial.conflicts.clone());
                    let sides = [(&self.current, normalize(detail.current.as_deref().unwrap_or_default())), (&self.result, initial.text.clone()), (&self.incoming, normalize(detail.incoming.as_deref().unwrap_or_default()))];
                    for (state, text) in sides {
                        state.update(cx, |state, cx| state.set_value(text, window, cx));
                    }
                    self.synced = self.result.read(cx).text().clone();
                }
                self.detail = Some(detail);
                self.error = None;
                self.refresh_decorations(cx);
                if self.initial.is_some() {
                    self.reveal(0, false, cx);
                }
            }
            Err(error) => self.error = Some(error.message),
        }
        cx.notify();
    }

    /// Mencerminkan suntingan manual di panel Result ke rentang konflik.
    fn on_result_edits(&mut self, event: &InputEditsEvent, cx: &mut Context<Self>) {
        let Some((_, last)) = event.edits().last() else { return };
        // Penggantian oleh tombol pilihan sudah tercatat di model.
        if *last == self.synced {
            return;
        }
        let mut before = self.synced.clone();
        for (edit, after) in event.edits() {
            let whole = edit.start_byte == 0 && edit.old_end_byte == before.len() && before.len() > 0;
            let (start, old_end, new_end) = if whole { minimal_edit(&before.to_string(), &after.to_string()) } else { (edit.start_byte, edit.old_end_byte, edit.new_end_byte) };
            self.model.apply_edit(start, old_end, new_end);
            before = after.clone();
        }
        self.synced = before;
        self.touched = true;
        self.refresh_decorations(cx);
        cx.notify();
    }

    fn refresh_decorations(&mut self, cx: &mut Context<Self>) {
        let theme = cx.theme();
        let (unresolved, resolved, current, incoming): (Hsla, Hsla, Hsla, Hsla) = (theme.warning, theme.success, theme.success, theme.info);
        let active = self.model.active;
        let mut fills = Vec::new();
        let mut marks = Vec::new();
        for conflict in &self.model.conflicts {
            let color = if conflict.resolved { resolved } else { unresolved };
            let opacity = if conflict.index == active { 0.28 } else { 0.14 };
            fills.push(RangeDecoration::new(conflict.from..conflict.to).with_style(RangeDecorationStyle::Fill).with_color(color.opacity(opacity)));
            marks.push(GutterDecoration::new(conflict.from..conflict.to).with_color(color));
        }
        let side = |ranges: &[std::ops::Range<usize>], color: Hsla| {
            ranges.iter().enumerate().map(|(index, range)| RangeDecoration::new(range.clone()).with_style(RangeDecorationStyle::Fill).with_color(color.opacity(if index == active { 0.28 } else { 0.14 }))).collect::<Vec<_>>()
        };
        let (current_fills, incoming_fills) = match &self.initial {
            Some(initial) => (side(&initial.current_ranges, current), side(&initial.incoming_ranges, incoming)),
            None => (Vec::new(), Vec::new()),
        };
        self.decorations.result_fill.set(fills, cx);
        self.decorations.result_gutter.set(marks, cx);
        self.decorations.current_fill.set(current_fills, cx);
        self.decorations.incoming_fill.set(incoming_fills, cx);
    }

    /// Menjadikan sebuah konflik aktif dan menampilkannya di ketiga panel.
    fn reveal(&mut self, index: usize, focus: bool, cx: &mut Context<Self>) {
        let Some(initial) = &self.initial else { return };
        let Some(conflict) = self.model.conflicts.get(index) else { return };
        self.model.active = index;
        let targets = [(&self.current, initial.current_ranges[index].start), (&self.incoming, initial.incoming_ranges[index].start), (&self.result, conflict.from)];
        for (state, offset) in targets {
            state.update(cx, |state, cx| {
                let offset = offset.min(state.text().len());
                state.set_selected_range(offset..offset, cx);
            });
        }
        let _ = focus;
        self.refresh_decorations(cx);
        cx.notify();
    }

    /// Pindah ke konflik berikutnya atau sebelumnya (berputar).
    pub fn go_to(&mut self, delta: isize, cx: &mut Context<Self>) {
        let total = self.model.conflicts.len() as isize;
        if total == 0 {
            return;
        }
        let next = (self.model.active as isize + delta).rem_euclid(total) as usize;
        self.reveal(next, true, cx);
    }

    /// Mengganti isi konflik aktif dengan sisi pilihan.
    pub fn choose(&mut self, choice: ConflictChoice, window: &mut Window, cx: &mut Context<Self>) {
        let index = self.model.active;
        let Some((range, insert)) = self.model.choose(index, choice) else { return };
        let unchanged = self.synced.slice(range.clone()).to_string() == insert;
        if !unchanged {
            self.result.update(cx, |state, cx| {
                state.set_selected_range(range.clone(), cx);
                // Rentang kosong tetap berupa kursor di posisi itu, jadi teks disisipkan di sana.
                state.replace(insert.clone(), window, cx);
            });
            self.synced = self.result.read(cx).text().clone();
        }
        self.touched = true;
        self.refresh_decorations(cx);
        cx.notify();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.touched = false;
        cx.notify();
        let Some(workbench) = self.env.workbench.upgrade() else { return };
        let id = self.tab_id();
        // Ditunda: bisa dipanggil selagi `Workbench` diperbarui.
        window.defer(cx, move |window, cx| {
            workbench.update(cx, |workbench, cx| {
                let groups: Vec<u32> = workbench.model().groups.iter().filter(|group| group.contains(&id)).map(|group| group.id).collect();
                for group in groups {
                    workbench.close_tabs(group, vec![id.clone()], true, window, cx).detach();
                }
            });
        });
    }

    /// Menulis isi panel Result ke berkas dan menandai konflik selesai.
    pub fn mark_resolved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Some(detail) = &self.detail else { return };
        let left = self.model.unresolved();
        let confirmed = (left > 0).then(|| {
            let title = if left == 1 { "1 conflict is still unresolved".to_string() } else { format!("{left} conflicts are still unresolved") };
            overlay::confirm(Confirm::new(title, "Unresolved conflicts keep the Current side. Mark the file as resolved anyway?", "Mark as Resolved"), window, cx)
        });
        // Akhir baris mengikuti berkas aslinya.
        let crlf = detail.current.as_deref().or(detail.incoming.as_deref()).unwrap_or_default().contains("\r\n");
        let (core, root, path) = (self.env.core.clone(), self.root.clone(), self.path.clone());
        cx.spawn_in(window, async move |this, cx| {
            if let Some(confirmed) = confirmed {
                if !confirmed.await {
                    return;
                }
            }
            let Ok(task) = this.update(cx, |this, cx| {
                let text = this.result.read(cx).text().to_string();
                let content = if crlf { text.replace('\n', "\r\n") } else { text };
                this.saving = true;
                cx.notify();
                spawn_core(cx, async move { git::git_resolve_write(&core, root, path, content).await })
            }) else {
                return;
            };
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match result {
                    Ok(()) => this.close(window, cx),
                    Err(error) => {
                        overlay::show_error(&error, window, cx);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// Menyelesaikan konflik non-teks dengan satu sisi, atau dengan menghapus berkas.
    fn resolve_simple(&mut self, side: Option<ConflictSide>, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let (core, root, path) = (self.env.core.clone(), self.root.clone(), self.path.clone());
        let confirmed = side.is_none().then(|| {
            overlay::confirm(Confirm::new(format!("Delete {path}?"), "The conflict is resolved by deleting the file from the working tree and the index. This cannot be undone.", "Delete File").danger(), window, cx)
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Some(confirmed) = confirmed {
                if !confirmed.await {
                    return;
                }
            }
            let Ok(task) = this.update(cx, |this, cx| {
                this.saving = true;
                cx.notify();
                spawn_core(cx, async move {
                    match side {
                        Some(side) => git::git_resolve_side(&core, root, path, side).await,
                        None => git::git_resolve_delete(&core, root, path).await,
                    }
                })
            }) else {
                return;
            };
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match result {
                    Ok(()) => this.close(window, cx),
                    Err(error) => {
                        overlay::show_error(&error, window, cx);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    // ----- Tampilan -----

    fn render_centered(&self, children: Vec<gpui_kit::AnyElement>) -> gpui_kit::AnyElement {
        v_flex().id("conflict-message").size_full().track_focus(&self.focus_handle).items_center().justify_center().gap_3().p_4().children(children).into_any_element()
    }

    fn render_simple(&self, detail: &ConflictDetail, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        use ConflictKind::*;
        let kind = detail.kind;
        let keep_current = detail.current.is_some() || matches!(kind, DeletedByThem | AddedByUs) || (detail.binary && kind != DeletedByUs);
        let keep_incoming = detail.incoming.is_some() || matches!(kind, DeletedByUs | AddedByThem) || (detail.binary && kind != DeletedByThem);
        let current_exists = !matches!(kind, DeletedByUs | AddedByThem | BothDeleted);
        let incoming_exists = !matches!(kind, DeletedByThem | AddedByUs | BothDeleted);
        let description = format!("{}{}", if detail.binary { "This is a binary file. " } else { "" }, simple_text(kind));
        let saving = self.saving;
        let actions = h_flex()
            .gap_2()
            .when(keep_current && current_exists, |el| {
                el.child(Button::new("conflict-keep-current").outline().label(format!("Keep Current ({})", detail.current_label)).disabled(saving).on_click(cx.listener(|this, _, window, cx| this.resolve_simple(Some(ConflictSide::Current), window, cx))))
            })
            .when(keep_incoming && incoming_exists, |el| {
                el.child(Button::new("conflict-keep-incoming").outline().label(format!("Keep Incoming ({})", detail.incoming_label)).disabled(saving).on_click(cx.listener(|this, _, window, cx| this.resolve_simple(Some(ConflictSide::Incoming), window, cx))))
            })
            .child(Button::new("conflict-delete").danger().label("Delete File").disabled(saving).on_click(cx.listener(|this, _, window, cx| this.resolve_simple(None, window, cx))));
        self.render_centered(vec![
            div().font_weight(gpui_kit::FontWeight::SEMIBOLD).child(SharedString::from(self.path.clone())).into_any_element(),
            div().text_color(cx.theme().muted_foreground).child(SharedString::from(description)).into_any_element(),
            actions.into_any_element(),
        ])
    }

    fn render_editor(&self, detail: &ConflictDetail, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme();
        let (total, unresolved) = self.counts();
        let active = self.model.conflicts.get(self.model.active);
        let saving = self.saving;
        let counter = match (total, unresolved) {
            (0, _) => "No conflicting regions".to_string(),
            (total, 0) => format!("All {total} resolved"),
            (total, unresolved) => format!("{unresolved} of {total} unresolved"),
        };

        let toolbar = h_flex()
            .id("conflict-toolbar")
            .w_full()
            .flex_none()
            .px_3()
            .py_1()
            .gap_2()
            .text_sm()
            .border_b_1()
            .border_color(theme.border)
            .child(div().min_w_0().overflow_hidden().whitespace_nowrap().child(SharedString::from(self.path.clone())))
            .child(div().id("conflict-counter").flex_none().text_color(if unresolved > 0 { theme.warning } else { theme.success }).child(SharedString::from(counter)))
            .child(div().flex_1())
            .child(Button::new("conflict-previous").ghost().xsmall().icon(Icon::new(Lucide::ArrowUp)).tooltip("Previous conflict").disabled(total == 0).on_click(cx.listener(|this, _, _, cx| this.go_to(-1, cx))))
            .child(Button::new("conflict-next").ghost().xsmall().icon(Icon::new(Lucide::ArrowDown)).tooltip("Next conflict").disabled(total == 0).on_click(cx.listener(|this, _, _, cx| this.go_to(1, cx))))
            .child(
                Button::new("conflict-mark-resolved")
                    .primary()
                    .xsmall()
                    .icon(Icon::new(Lucide::Check))
                    .label("Mark as Resolved")
                    .disabled(saving)
                    .on_click(cx.listener(|this, _, window, cx| this.mark_resolved(window, cx))),
            );

        // Bilah konflik aktif: status dan pilihan sisinya.
        let choice_bar = active.map(|conflict| {
            let mut choices = vec![(ConflictChoice::Current, "Use Current"), (ConflictChoice::Incoming, "Use Incoming"), (ConflictChoice::Both, "Use Both")];
            if !conflict.base.is_empty() {
                choices.push((ConflictChoice::Base, "Use Base"));
            }
            let status = format!("Conflict {} of {total} · {}", conflict.index + 1, if conflict.resolved { "resolved" } else { "unresolved" });
            h_flex()
                .id("conflict-choices")
                .w_full()
                .flex_none()
                .px_3()
                .py_1()
                .gap_2()
                .text_xs()
                .border_b_1()
                .border_color(theme.border)
                .bg(theme.muted.opacity(0.3))
                .child(div().id("conflict-status").text_color(if conflict.resolved { theme.success } else { theme.warning }).child(SharedString::from(status)))
                .child(div().flex_1())
                .children(choices.into_iter().map(|(choice, label)| {
                    Button::new(SharedString::from(format!("conflict-use-{}", choice_key(choice)))).outline().xsmall().label(label).disabled(saving).on_click(cx.listener(move |this, _, window, cx| this.choose(choice, window, cx)))
                }))
        });

        let pane = |id: &'static str, title: String, color: Hsla, state: &Entity<EditorState>, readonly: bool| {
            v_flex()
                .id(id)
                .size_full()
                .child(div().w_full().flex_none().px_3().py_0p5().text_xs().overflow_hidden().whitespace_nowrap().border_b_2().border_color(color).child(SharedString::from(title)))
                .child(div().flex_1().min_h_0().w_full().child(Editor::new(state).readonly(readonly).bordered(false).p_0().h(relative(1.))))
        };
        let panes = h_resizable("conflict-panes")
            .child(resizable_panel().child(pane("conflict-pane-current", format!("Current · {}", detail.current_label), theme.success, &self.current, true)))
            .child(resizable_panel().child(pane("conflict-pane-result", "Result (editable)".to_string(), theme.primary, &self.result, false)))
            .child(resizable_panel().child(pane("conflict-pane-incoming", format!("Incoming · {}", detail.incoming_label), theme.info, &self.incoming, true)));

        v_flex().size_full().child(toolbar).children(choice_bar).child(div().flex_1().min_h_0().w_full().child(panes)).into_any_element()
    }
}

impl Focusable for ConflictView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        if self.initial.is_some() {
            self.result.focus_handle(cx)
        } else {
            self.focus_handle.clone()
        }
    }
}

impl Render for ConflictView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Selama snapshot belum ada, berkas dianggap masih konflik.
        let still_conflicted = repos(cx).read(cx).snapshot(&self.root).is_none_or(|snapshot| snapshot.conflicted.iter().any(|entry| entry.path == self.path));
        let muted = cx.theme().muted_foreground;
        let body = if let Some(error) = &self.error {
            self.render_centered(vec![div().text_color(muted).child(SharedString::from(error.clone())).into_any_element()])
        } else if let Some(detail) = self.detail.clone() {
            if !still_conflicted && !self.saving {
                self.render_centered(vec![
                    div().child(SharedString::from(format!("{} is no longer in conflict.", self.path))).into_any_element(),
                    Button::new("conflict-close").primary().label("Close").on_click(cx.listener(|this, _, window, cx| this.close(window, cx))).into_any_element(),
                ])
            } else if self.initial.is_some() {
                self.render_editor(&detail, cx)
            } else {
                self.render_simple(&detail, cx)
            }
        } else {
            self.render_centered(vec![Spinner::new().into_any_element()])
        };
        div().id("conflict-tab").size_full().bg(cx.theme().background).child(body)
    }
}

/// Isi tab untuk jenis `conflict`.
pub struct ConflictTab {
    pub view: Entity<ConflictView>,
}

impl TabContent for ConflictTab {
    fn view(&self) -> gpui_kit::AnyView {
        self.view.clone().into()
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        let handle = self.view.focus_handle(cx);
        handle.focus(window, cx);
    }

    fn is_dirty(&self, cx: &App) -> bool {
        self.view.read(cx).touched
    }

    fn can_close(&self, window: &mut Window, cx: &mut App) -> Task<bool> {
        if !self.view.read(cx).touched {
            return Task::ready(true);
        }
        overlay::confirm(
            Confirm::new(
                "Discard conflict resolution?",
                "Your choices in this conflict editor have not been applied yet. Closing the tab discards them; the file stays in conflict.",
                "Discard",
            )
            .danger(),
            window,
            cx,
        )
    }

    fn observe(&self, on_change: Rc<dyn Fn(&mut App)>, cx: &mut App) -> Option<Subscription> {
        Some(cx.observe(&self.view, move |_, cx| on_change(cx)))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub(crate) struct ConflictKindTab;

impl TabKind for ConflictKindTab {
    fn build(&self, tab: &TabData, env: &Env, window: &mut Window, cx: &mut App) -> Box<dyn TabContent> {
        let (root, path) = (tab.param_str("root").unwrap_or_default().to_string(), tab.param_str("path").unwrap_or_default().to_string());
        let env = env.clone();
        Box::new(ConflictTab { view: cx.new(|cx| ConflictView::new(root, path, env, window, cx)) })
    }

    fn icon(&self, _tab: &TabData) -> Option<Lucide> {
        Some(Lucide::GitMerge)
    }
}
