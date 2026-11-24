//! Tampilan tab berkas: editor kode di atas sebuah [`Buffer`].

use std::rc::Rc;

use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Editor, EditorState, InputEvent, Position, RopeExt as _, TabSize};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _};
use gpui_kit::{
    div, px, relative, App, AppContext as _, Context, Entity, FocusHandle, Focusable, Global, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription,
    Task, Window,
};
use oncode_core::settings::EditorSettings;

use super::buffer::{Buffer, BufferEvent, BufferStatus, Buffers, BuffersEvent, Reveal};
use super::language::language_for_path;
use crate::overlay::{self, Choice};
use crate::workbench::{Env, TabContent, WorkbenchEvent};

/// Nama font yang benar-benar terpasang untuk nilai `editor.fontFamily` (daftar ala CSS).
#[derive(Default)]
struct FontCache(std::collections::HashMap<String, Option<SharedString>>);

impl Global for FontCache {}

fn resolve_font(setting: &str, cx: &mut App) -> Option<SharedString> {
    if let Some(found) = cx.default_global::<FontCache>().0.get(setting) {
        return found.clone();
    }
    let installed = cx.text_system().all_font_names();
    let found = setting
        .split(',')
        .map(|name| name.trim().trim_matches(['"', '\'']).to_string())
        .filter(|name| !name.is_empty() && !matches!(name.as_str(), "monospace" | "sans-serif" | "serif"))
        .find(|name| installed.iter().any(|font| font.eq_ignore_ascii_case(name)))
        .map(SharedString::from);
    cx.default_global::<FontCache>().0.insert(setting.to_string(), found.clone());
    found
}

fn format_size(bytes: u32) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

pub struct FileView {
    env: Env,
    buffers: Entity<Buffers>,
    buffer: Entity<Buffer>,
    state: Entity<EditorState>,
    settings: EditorSettings,
    _subscriptions: Vec<Subscription>,
}

impl FileView {
    pub fn new(path: &str, env: Env, buffers: Entity<Buffers>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let buffer = buffers.update(cx, |buffers, cx| buffers.open(path, window, cx));
        let settings = EditorSettings::default();
        // Pengaturan efektif dibaca dari core: `Workbench` sedang dipinjam saat tab dibangun.
        let extra_languages = env.core.effective_settings(&env.window).extensions.extra_languages;
        let highlighter = language_for_path(path).highlighter_with(extra_languages);
        let state = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(highlighter)
                .line_number(settings.line_numbers)
                .indent_guides(true)
                .folding(true)
                .soft_wrap(settings.word_wrap)
                .tab_size(TabSize { tab_size: settings.tab_size as usize, hard_tabs: !settings.insert_spaces })
        });
        buffer.update(cx, |buffer, cx| buffer.attach(&state, window, cx));
        // Ctrl+klik/F12 tanpa language server; hub LSP menimpanya saat `editor_created` bila ada server.
        super::symbols::attach(&state, &env, path, cx);
        buffers.update(cx, |buffers, cx| buffers.editor_created(buffer.clone(), state.clone(), cx));

        let mut subscriptions = vec![
            cx.observe(&buffer, |_, _, cx| cx.notify()),
            cx.subscribe_in(&buffer, window, |this, _, event: &BufferEvent, window, cx| {
                if matches!(event, BufferEvent::Loaded) {
                    this.apply_reveal(window, cx);
                }
            }),
            cx.subscribe_in(&buffers, window, |this, _, event: &BuffersEvent, window, cx| {
                if matches!(event, BuffersEvent::Reveal(path) if path == this.buffer.read(cx).path()) {
                    this.apply_reveal(window, cx);
                }
            }),
            // Mulai menyunting menjadikan tab pratinjau sebagai tab tetap.
            cx.subscribe_in(&state, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    let id = format!("file:{}", this.buffer.read(cx).path());
                    if let Some(workbench) = this.env.workbench.upgrade() {
                        window.defer(cx, move |_, cx| workbench.update(cx, |workbench, cx| workbench.pin_tab(&id, cx)));
                    }
                }
                cx.notify();
            }),
        ];
        if let Some(workbench) = env.workbench.upgrade() {
            subscriptions.push(cx.subscribe_in(&workbench, window, |this, workbench, event: &WorkbenchEvent, window, cx| {
                if matches!(event, WorkbenchEvent::SettingsChanged) {
                    let settings = workbench.read(cx).settings().editor.clone();
                    this.apply_settings(settings, window, cx);
                }
            }));
        }

        let mut this = Self { env, buffers, buffer, state, settings, _subscriptions: subscriptions };
        // `Workbench` sedang dipinjam saat tab dibangun; pengaturan dibaca setelahnya.
        cx.defer_in(window, |this, window, cx| {
            if let Some(workbench) = this.env.workbench.upgrade() {
                let settings = workbench.read(cx).settings().editor.clone();
                this.apply_settings(settings, window, cx);
            }
            this.apply_reveal(window, cx);
        });
        this.apply_reveal(window, cx);
        this
    }

    pub fn buffer(&self) -> &Entity<Buffer> {
        &self.buffer
    }

    pub fn state(&self) -> &Entity<EditorState> {
        &self.state
    }

    fn apply_settings(&mut self, settings: EditorSettings, window: &mut Window, cx: &mut Context<Self>) {
        if settings == self.settings {
            return;
        }
        self.state.update(cx, |state, cx| {
            state.set_line_number(settings.line_numbers, window, cx);
            state.set_soft_wrap(settings.word_wrap, window, cx);
            state.set_tab_size(TabSize { tab_size: settings.tab_size as usize, hard_tabs: !settings.insert_spaces }, cx);
        });
        self.settings = settings;
        cx.notify();
    }

    /// Melompat ke posisi yang menunggu untuk berkas ini, bila ada dan isinya sudah dimuat.
    fn apply_reveal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.buffer.read(cx).is_ready() {
            return;
        }
        let path = self.buffer.read(cx).path().to_string();
        let Some(reveal) = self.buffers.update(cx, |buffers, _| buffers.take_reveal(&path)) else { return };
        self.reveal(&reveal, window, cx);
    }

    pub fn reveal(&mut self, reveal: &Reveal, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            let line = reveal.line.saturating_sub(1);
            let text = state.text().clone();
            let line_start = text.line_start_offset(line as usize);
            let line_text = text.slice_line(line as usize).to_string();
            // Kolom dihitung dalam karakter; editor memakai offset byte.
            let byte_at = |chars: u32| line_text.char_indices().nth(chars as usize).map_or(line_text.len(), |(index, _)| index);
            let start = line_start + byte_at(reveal.column);
            state.set_cursor_position(text.offset_to_position(start), window, cx);
            if let Some(length) = reveal.length.filter(|length| *length > 0) {
                let end = line_start + byte_at(reveal.column + length);
                state.set_selected_range(start..end, cx);
            }
            state.focus(window, cx);
        });
    }

    /// Posisi kursor (baris dan kolom mulai dari 1) dan jumlah karakter terpilih.
    pub fn cursor_info(&self, cx: &App) -> (u32, u32, usize) {
        let state = self.state.read(cx);
        let Position { line, character } = state.cursor_position();
        let range = state.selected_range();
        let selected = if range.is_empty() { 0 } else { state.text().slice(range).chars().count() };
        (line + 1, character + 1, selected)
    }

    fn render_message(&self, message: impl Into<SharedString>, action: Option<Button>, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .text_color(cx.theme().muted_foreground)
            .child(div().child(message.into()))
            .children(action)
            .into_any_element()
    }
}

impl Focusable for FileView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.state.focus_handle(cx)
    }
}

impl Render for FileView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let buffer = self.buffer.read(cx);
        let status = buffer.status().clone();
        let conflict = buffer.has_conflict();
        let read_only = buffer.is_read_only();
        let name = buffer.file_name().to_string();
        let font = resolve_font(&self.settings.font_family, cx).unwrap_or_else(|| cx.theme().mono_font_family.clone());
        let font_size = px(self.settings.font_size.clamp(8, 40) as f32);

        let body = match status {
            BufferStatus::Loading => v_flex().size_full().items_center().justify_center().child(Spinner::new()).into_any_element(),
            BufferStatus::Binary { size } => self.render_message(format!("{name} is a binary file ({}) and cannot be shown in the editor.", format_size(size)), None, cx),
            BufferStatus::TooLarge { size } => self.render_message(format!("{name} is too large to open ({}).", format_size(size)), None, cx),
            BufferStatus::Error(message) => {
                let retry = Button::new("file-retry").label("Try Again").outline().on_click(cx.listener(|this, _, window, cx| {
                    this.buffer.update(cx, |buffer, cx| buffer.reload(window, cx));
                }));
                self.render_message(format!("Could not open {name}: {message}"), Some(retry), cx)
            }
            BufferStatus::Ready => Editor::new(&self.state).readonly(read_only).bordered(false).p_0().h(relative(1.)).font_family(font).text_size(font_size).into_any_element(),
        };

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .children(conflict.then(|| {
                h_flex()
                    .w_full()
                    .flex_none()
                    .px_3()
                    .py_1()
                    .gap_2()
                    .text_sm()
                    .bg(cx.theme().warning.opacity(0.15))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(div().flex_1().child("This file changed on disk while you have unsaved changes."))
                    .child(Button::new("file-conflict-reload").label("Reload from Disk").xsmall().outline().on_click(cx.listener(|this, _, window, cx| {
                        this.buffer.update(cx, |buffer, cx| buffer.reload(window, cx));
                    })))
                    .child(Button::new("file-conflict-overwrite").label("Overwrite").xsmall().outline().on_click(cx.listener(|this, _, window, cx| {
                        this.buffer.update(cx, |buffer, cx| buffer.save(window, cx)).detach();
                    })))
            }))
            .child(div().flex_1().min_h_0().w_full().child(body))
    }
}

/// Isi tab untuk jenis `file`.
pub struct FileTab {
    pub view: Entity<FileView>,
    pub buffer: Entity<Buffer>,
}

impl TabContent for FileTab {
    fn view(&self) -> gpui_kit::AnyView {
        self.view.clone().into()
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        let state = self.view.read(cx).state.clone();
        state.update(cx, |state, cx| state.focus(window, cx));
    }

    fn is_dirty(&self, cx: &App) -> bool {
        self.buffer.read(cx).is_dirty()
    }

    fn can_close(&self, window: &mut Window, cx: &mut App) -> Task<bool> {
        if !self.buffer.read(cx).is_dirty() {
            return Task::ready(true);
        }
        let name = self.buffer.read(cx).file_name().to_string();
        let answer = overlay::choose(
            "Unsaved changes",
            format!("Save the changes you made to {name}? Your changes will be lost if you don't save them."),
            vec![Choice::new("cancel", "Cancel"), Choice::new("discard", "Don't Save").danger(), Choice::new("save", "Save").primary()],
            window,
            cx,
        );
        let buffer = self.buffer.clone();
        window.spawn(cx, async move |cx| match answer.await {
            Some("save") => match buffer.update_in(cx, |buffer, window, cx| buffer.save(window, cx)) {
                Ok(task) => task.await,
                Err(_) => false,
            },
            Some("discard") => true,
            _ => false,
        })
    }

    fn observe(&self, on_change: Rc<dyn Fn(&mut App)>, cx: &mut App) -> Option<Subscription> {
        Some(cx.observe(&self.buffer, move |_, cx| on_change(cx)))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
