//! Tampilan terminal: satu sesi (PTY + emulator) dan panel yang menampung beberapa sesi.
//!
//! Setiap baris layar digambar sebagai teks bergaya (`StyledText`) dengan font monospace, sehingga
//! kolom tetap sejajar. Ukuran grid dihitung dari lebar dan tinggi area yang tersedia.

use std::cell::Cell as StdCell;
use std::collections::HashSet;
use std::ops::Range;
use std::rc::Rc;

use alacritty_terminal::event::Event as TermEmuEvent;
use alacritty_terminal::grid::{Dimensions as _, Scroll};
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::CursorShape;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Selectable as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    canvas, div, px, App, AppContext as _, Bounds, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, FontStyle, FontWeight, HighlightStyle,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _, Pixels, Render, ScrollEvent,
    SharedString, StatefulInteractiveElement as _, StrikethroughStyle, Styled as _, StyledText, Subscription, Task, UnderlineStyle, WeakEntity, Window,
};
use oncode_core::api::term::{self, TermEvent};
use oncode_ui::keys::KeyPress;
use oncode_ui::overlay::{self, TextPrompt};
use oncode_ui::{spawn_core, Env, Lucide, Workbench};
use tokio::sync::mpsc;

use crate::emulator::{Emulator, GridSize};
use crate::keys::{self, KeyAction, KeyModes};
use crate::palette::{cell_style, to_hsla, Palette};

const IS_MAC: bool = cfg!(target_os = "macos");

/// Ukuran sel dan baris untuk font terminal saat ini.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Metrics {
    cell_width: Pixels,
    line_height: Pixels,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// Proses shell berakhir.
    Exited,
    /// Judul (OSC 0/2) berubah.
    TitleChanged,
}

/// Satu terminal: proses shell di PTY dan emulator yang menampilkan keluarannya.
pub struct TerminalSession {
    env: Env,
    pub(crate) emulator: Emulator,
    /// Id PTY dari `oncode-core`; `None` selama proses dimulai.
    id: Option<u32>,
    title: SharedString,
    exited: Option<Option<i32>>,
    error: Option<String>,
    input: Option<mpsc::UnboundedSender<Vec<u8>>>,
    focus_handle: FocusHandle,
    metrics: Option<Metrics>,
    /// Ukuran area terakhir yang diukur, untuk menghitung ulang grid.
    bounds: Rc<StdCell<Option<Bounds<Pixels>>>>,
    /// Sisa gulir roda (piksel) yang belum genap satu baris.
    scroll_remainder: f32,
    selecting: bool,
    font_family: SharedString,
    font_size: Pixels,
    _reader: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SessionEvent> for TerminalSession {}

impl Focusable for TerminalSession {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Keybinding aplikasi yang tetap berlaku saat terminal fokus (lihat `keys::COMMANDS_TO_SKIP_SHELL`).
fn app_bindings(window: &Window, cx: &App) -> HashSet<String> {
    keys::COMMANDS_TO_SKIP_SHELL
        .iter()
        .filter_map(|id| oncode_ui::commands::get(id, window, cx))
        .flat_map(|command| oncode_ui::commands::keybindings_for(&command, cx))
        .collect()
}

fn press_of(event: &KeyDownEvent) -> KeyPress {
    let modifiers = &event.keystroke.modifiers;
    KeyPress { control: modifiers.control, alt: modifiers.alt, shift: modifiers.shift, platform: modifiers.platform, key: event.keystroke.key.clone() }
}

impl TerminalSession {
    pub fn new(env: Env, number: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        // Tombol yang bukan milik aplikasi diteruskan ke shell, termasuk Ctrl+W, Ctrl+S, dan sejenisnya.
        oncode_ui::commands::capture_keys(
            &focus_handle,
            |press, window, cx| keys::classify_key(press, IS_MAC, &app_bindings(window, cx)) != KeyAction::App,
            cx,
        );
        let (font_family, font_size) = terminal_font(&env, cx);
        let mut this = Self {
            env,
            emulator: Emulator::new(GridSize::new(80, 24)),
            id: None,
            title: format!("Terminal {number}").into(),
            exited: None,
            error: None,
            input: None,
            focus_handle,
            metrics: None,
            bounds: Rc::new(StdCell::new(None)),
            scroll_remainder: 0.,
            selecting: false,
            font_family,
            font_size,
            _reader: Task::ready(()),
            _subscriptions: Vec::new(),
        };
        if let Some(workbench) = this.env.workbench.upgrade() {
            this._subscriptions.push(cx.subscribe_in(&workbench, window, |this, workbench, event: &oncode_ui::WorkbenchEvent, _, cx| {
                if matches!(event, oncode_ui::WorkbenchEvent::SettingsChanged) {
                    let settings = workbench.read(cx).settings();
                    let family = family_from_setting(&settings.editor.font_family);
                    let size = px(settings.terminal.font_size.unwrap_or(settings.editor.font_size).clamp(8, 40) as f32);
                    if let Some(family) = family {
                        this.font_family = family;
                    }
                    this.font_size = size;
                    this.metrics = None;
                    cx.notify();
                }
            }));
        }
        this.start(window, cx);
        this
    }

    pub fn title(&self) -> &SharedString {
        &self.title
    }

    pub fn focus_handle_is_focused(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
    }

    pub fn is_running(&self) -> bool {
        self.id.is_some() && self.exited.is_none()
    }

    pub fn has_exited(&self) -> bool {
        self.exited.is_some()
    }

    /// Isi buffer (riwayat dan layar) sebagai teks; untuk uji dan "salin semua".
    pub fn text(&self) -> String {
        self.emulator.text()
    }

    /// Memulai proses shell (juga untuk "Restart" setelah proses berakhir).
    pub fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.exited = None;
        self.error = None;
        self.emulator = Emulator::new(self.emulator.size());
        let (output_tx, mut output_rx) = mpsc::unbounded_channel::<TermEvent>();
        let (input_tx, mut input_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        self.input = Some(input_tx);
        let size = self.emulator.size();
        let (core, label) = (self.env.core.clone(), self.env.window.to_string());
        let spawn = spawn_core(cx, async move {
            let callback: oncode_core::api::Callback<TermEvent> = std::sync::Arc::new(move |event| {
                let _ = output_tx.send(event);
            });
            let id = term::term_spawn(&core, &label, size.cols as u16, size.rows as u16, callback).await?;
            // Penulis tunggal per terminal agar urutan masukan terjaga.
            let writer_core = core.clone();
            tokio::spawn(async move {
                while let Some(bytes) = input_rx.recv().await {
                    if term::term_write(&writer_core, id, &bytes).await.is_err() {
                        break;
                    }
                }
            });
            Ok::<u32, oncode_core::AppError>(id)
        });
        self._reader = cx.spawn_in(window, async move |this, cx| {
            match spawn.await {
                Ok(id) => {
                    if this.update(cx, |this, cx| {
                        this.id = Some(id);
                        cx.notify();
                    })
                    .is_err()
                    {
                        return;
                    }
                }
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.error = Some(error.message);
                        cx.notify();
                    });
                    return;
                }
            }
            // Keluaran dikumpulkan per gelombang agar keluaran deras tidak menggambar ulang per potongan.
            while let Some(first) = output_rx.recv().await {
                let mut batch = vec![first];
                while let Ok(more) = output_rx.try_recv() {
                    batch.push(more);
                }
                if this.update(cx, |this, cx| this.process(batch, cx)).is_err() {
                    return;
                }
            }
        });
    }

    fn process(&mut self, batch: Vec<TermEvent>, cx: &mut Context<Self>) {
        for event in batch {
            match event {
                TermEvent::Data { data } => self.emulator.feed(&data),
                TermEvent::Exit { code } => {
                    self.exited = Some(code);
                    let message = match code {
                        Some(code) => format!("\r\n\x1b[2m[Process exited with code {code}]\x1b[0m\r\n"),
                        None => "\r\n\x1b[2m[Process exited]\x1b[0m\r\n".to_string(),
                    };
                    self.emulator.inject(message.as_bytes());
                    cx.emit(SessionEvent::Exited);
                }
            }
        }
        for event in self.emulator.take_events() {
            match event {
                // Jawaban emulator untuk program (mis. laporan posisi kursor).
                TermEmuEvent::PtyWrite(text) => self.write(text.into_bytes()),
                TermEmuEvent::Title(title) if !title.trim().is_empty() => {
                    self.title = title.into();
                    cx.emit(SessionEvent::TitleChanged);
                }
                _ => {}
            }
        }
        cx.notify();
    }

    /// Mengirim byte ke shell.
    pub fn write(&mut self, bytes: Vec<u8>) {
        if self.exited.is_some() {
            return;
        }
        if let Some(input) = &self.input {
            let _ = input.send(bytes);
        }
    }

    /// Mengirim teks seperti diketik (dipakai command "Run Selected Text" dan uji).
    pub fn send_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.emulator.set_display_offset(0);
        self.write(text.as_bytes().to_vec());
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.emulator.clear();
        cx.notify();
    }

    pub fn kill(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.id.take() {
            let (core, label) = (self.env.core.clone(), self.env.window.to_string());
            spawn_core(cx, async move { term::term_kill(&core, &label, id).await }).detach();
        }
        self.input = None;
    }

    pub fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        match self.emulator.selection_text() {
            Some(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                true
            }
            None => false,
        }
    }

    pub fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else { return };
        let bracketed = self.emulator.mode().contains(TermMode::BRACKETED_PASTE);
        self.emulator.set_display_offset(0);
        self.write(keys::paste_bytes(&text, bracketed));
        cx.notify();
    }

    fn find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let answer = overlay::prompt_text(TextPrompt::new("Find in Terminal", "Find"), window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let Some(query) = answer.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                if !this.emulator.find(&query, false) && !this.emulator.find(&query, true) {
                    overlay::toast_info(format!("No results for \"{query}\""), window, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ----- Masukan -----

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let press = press_of(event);
        match keys::classify_key(&press, IS_MAC, &app_bindings(window, cx)) {
            KeyAction::App => return,
            KeyAction::Copy => {
                self.copy_selection(cx);
            }
            KeyAction::Paste => self.paste(cx),
            KeyAction::Find => self.find(window, cx),
            KeyAction::Shell => {
                let mode = self.emulator.mode();
                let modes = KeyModes { app_cursor: mode.contains(TermMode::APP_CURSOR), line_feed_new_line: mode.contains(TermMode::LINE_FEED_NEW_LINE) };
                let bytes = keys::key_to_bytes(&press, modes, IS_MAC).or_else(|| {
                    // Teks biasa (huruf, angka, simbol, Shift+huruf).
                    let text = event.keystroke.key_char.clone()?;
                    (!press.control && !press.platform).then(|| text.into_bytes())
                });
                let Some(bytes) = bytes else { return };
                self.emulator.clear_selection();
                self.emulator.set_display_offset(0);
                self.write(bytes);
                cx.notify();
            }
        }
        cx.stop_propagation();
    }

    /// Titik grid di bawah posisi jendela, beserta sisi sel (untuk seleksi).
    fn point_at(&self, position: gpui_kit::Point<Pixels>) -> Option<(GridPoint, Side)> {
        let bounds = self.bounds.get()?;
        let metrics = self.metrics?;
        let size = self.emulator.size();
        let x = (position.x - bounds.origin.x).max(px(0.));
        let y = (position.y - bounds.origin.y).max(px(0.));
        let column_f = x / metrics.cell_width;
        let column = (column_f as usize).min(size.cols.saturating_sub(1));
        let side = if column_f.fract() < 0.5 { Side::Left } else { Side::Right };
        let row = ((y / metrics.line_height) as usize).min(size.rows.saturating_sub(1)) as i32;
        Some((GridPoint::new(Line(row - self.emulator.display_offset() as i32), Column(column)), side))
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
        let Some((point, side)) = self.point_at(event.position) else { return };
        // Ctrl/Cmd+klik membuka tautan web.
        let open_link = if IS_MAC { event.modifiers.platform } else { event.modifiers.control };
        if open_link {
            if let Some(link) = self.emulator.link_at(point) {
                cx.open_url(&link.url);
                return;
            }
        }
        let ty = match event.click_count {
            2 => SelectionType::Semantic,
            3 => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        if event.click_count == 1 && !event.modifiers.shift {
            self.emulator.clear_selection();
        }
        self.emulator.start_selection(ty, point, side);
        self.selecting = true;
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        if let Some((point, side)) = self.point_at(event.position) {
            self.emulator.update_selection(point, side);
            cx.notify();
        }
    }

    fn on_scroll(&mut self, event: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(metrics) = self.metrics else { return };
        let delta = event.delta.pixel_delta(metrics.line_height).y;
        let total = self.scroll_remainder + f32::from(delta);
        let lines = (total / f32::from(metrics.line_height)).trunc();
        self.scroll_remainder = total - lines * f32::from(metrics.line_height);
        if lines == 0. {
            return;
        }
        let mode = self.emulator.mode();
        if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) && !mode.intersects(TermMode::MOUSE_MODE) {
            // Program layar penuh (less, vim) tanpa laporan tetikus: roda menjadi panah atas/bawah.
            let arrow: &[u8] = if lines > 0. { if mode.contains(TermMode::APP_CURSOR) { b"\x1bOA" } else { b"\x1b[A" } } else if mode.contains(TermMode::APP_CURSOR) { b"\x1bOB" } else { b"\x1b[B" };
            for _ in 0..lines.abs() as usize {
                self.write(arrow.to_vec());
            }
        } else {
            self.emulator.scroll(Scroll::Delta(lines as i32));
        }
        cx.notify();
    }

    // ----- Tata letak -----

    fn measure(&mut self, window: &mut Window) -> Metrics {
        if let Some(metrics) = self.metrics {
            return metrics;
        }
        let font = gpui_kit::font(self.font_family.clone());
        let font_id = window.text_system().resolve_font(&font);
        let cell_width = window.text_system().advance(font_id, self.font_size, 'm').map(|size| size.width).unwrap_or(self.font_size * 0.6);
        let metrics = Metrics { cell_width, line_height: (self.font_size * 1.3).round() };
        self.metrics = Some(metrics);
        metrics
    }

    /// Menyesuaikan grid dengan ukuran area; dipanggil setelah tata letak.
    fn fit(&mut self, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        let Some(metrics) = self.metrics else { return };
        let cols = ((bounds.size.width / metrics.cell_width) as usize).max(2);
        let rows = ((bounds.size.height / metrics.line_height) as usize).max(1);
        let size = GridSize::new(cols, rows);
        if size == self.emulator.size() {
            return;
        }
        self.emulator.resize(size);
        if let Some(id) = self.id {
            let core = self.env.core.clone();
            spawn_core(cx, async move { term::term_resize(&core, id, cols as u16, rows as u16).await }).detach();
        }
        cx.notify();
    }

    fn render_lines(&self, palette: &Palette, focused: bool) -> Vec<(String, Vec<(Range<usize>, HighlightStyle)>)> {
        let term = self.emulator.term();
        let content = term.renderable_content();
        let offset = content.display_offset as i32;
        let rows = term.screen_lines();
        let colors = content.colors;
        let selection = content.selection;
        let cursor = content.cursor;
        let show_cursor = cursor.shape != CursorShape::Hidden && self.exited.is_none();
        let (selection_rgb, selection_alpha) = palette.selection;
        let selection_bg = to_hsla(selection_rgb).opacity(selection_alpha);

        let mut lines: Vec<(String, Vec<(Range<usize>, HighlightStyle)>)> = (0..rows).map(|_| (String::new(), Vec::new())).collect();
        for indexed in content.display_iter {
            let row = indexed.point.line.0 + offset;
            if row < 0 || row as usize >= rows {
                continue;
            }
            let cell = indexed.cell;
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            let style = cell_style(cell, palette, colors);
            let (line, runs) = &mut lines[row as usize];
            let start = line.len();
            line.push(if cell.c == '\0' { ' ' } else { cell.c });
            line.extend(cell.zerowidth().into_iter().flatten());
            let end = line.len();

            let mut highlight = HighlightStyle { color: Some(to_hsla(style.fg)), ..Default::default() };
            if let Some(bg) = style.bg {
                highlight.background_color = Some(to_hsla(bg));
            }
            if style.bold {
                highlight.font_weight = Some(FontWeight::BOLD);
            }
            if style.italic {
                highlight.font_style = Some(FontStyle::Italic);
            }
            if style.underline {
                highlight.underline = Some(UnderlineStyle { thickness: px(1.), ..Default::default() });
            }
            if style.strikethrough {
                highlight.strikethrough = Some(StrikethroughStyle { thickness: px(1.), ..Default::default() });
            }
            if selection.is_some_and(|range| range.contains(indexed.point)) {
                highlight.background_color = Some(selection_bg);
            }
            if show_cursor && focused && cursor.shape == CursorShape::Block && indexed.point == cursor.point {
                highlight.background_color = Some(to_hsla(palette.cursor));
                highlight.color = Some(to_hsla(palette.cursor_text));
            }
            // Sel bergaya sama digabung menjadi satu potongan.
            match runs.last_mut() {
                Some((range, previous)) if range.end == start && *previous == highlight => range.end = end,
                _ => runs.push((start..end, highlight)),
            }
        }
        lines
    }
}

impl Render for TerminalSession {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let metrics = self.measure(window);
        let palette = Palette::from_theme(cx.theme());
        let focused = self.focus_handle.is_focused(window);
        let background = to_hsla(palette.background);
        let lines = self.render_lines(&palette, focused);

        // Kursor selain blok-fokus digambar sebagai lapisan di atas teks.
        let cursor_overlay = {
            let content = self.emulator.term().renderable_content();
            let cursor = content.cursor;
            let row = cursor.point.line.0 + content.display_offset as i32;
            let visible = cursor.shape != CursorShape::Hidden && self.exited.is_none() && row >= 0 && (row as usize) < self.emulator.size().rows;
            visible.then(|| {
                let left = metrics.cell_width * cursor.point.column.0 as f32;
                let top = metrics.line_height * row as f32;
                let color = to_hsla(palette.cursor);
                let base = div().absolute().left(left).top(top);
                match (cursor.shape, focused) {
                    (CursorShape::Block, true) => None,
                    (CursorShape::Beam, _) => Some(base.w(px(2.)).h(metrics.line_height).bg(color)),
                    (CursorShape::Underline, _) => Some(base.top(top + metrics.line_height - px(2.)).w(metrics.cell_width).h(px(2.)).bg(color)),
                    _ => Some(base.w(metrics.cell_width).h(metrics.line_height).border_1().border_color(color)),
                }
            })
            .flatten()
        };

        let bounds_cell = self.bounds.clone();
        let entity: WeakEntity<Self> = cx.weak_entity();
        let status = match (&self.error, self.id) {
            (Some(error), _) => Some(format!("The terminal could not start: {error}")),
            (None, None) => Some("Starting terminal…".to_string()),
            _ => None,
        };

        div()
            .id("terminal-session")
            .size_full()
            .relative()
            .overflow_hidden()
            .px_2()
            .pt_1()
            .bg(background)
            .text_color(to_hsla(palette.foreground))
            .font_family(self.font_family.clone())
            .text_size(self.font_size)
            .line_height(metrics.line_height)
            .track_focus(&self.focus_handle)
            .key_context("Terminal")
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.selecting = false))
            .on_scroll(cx.listener(Self::on_scroll))
            .child(
                div()
                    .size_full()
                    .relative()
                    .child(
                        // Mengukur area teks setelah tata letak; perubahan ukuran menghitung ulang grid.
                        canvas(
                            move |bounds, window, cx| {
                                let changed = bounds_cell.get().map(|b| b.size) != Some(bounds.size);
                                bounds_cell.set(Some(bounds));
                                if changed {
                                    let entity = entity.clone();
                                    window.defer(cx, move |_, cx| {
                                        let _ = entity.update(cx, |this, cx| this.fit(bounds, cx));
                                    });
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .children(lines.into_iter().map(|(text, runs)| div().h(metrics.line_height).whitespace_nowrap().child(StyledText::new(text).with_highlights(runs))))
                    .children(cursor_overlay),
            )
            .when_some(status, |el, status| {
                el.child(div().absolute().top_2().left_2().text_color(cx.theme().muted_foreground).font_family(cx.theme().font_family.clone()).child(status))
            })
    }
}

fn family_from_setting(setting: &str) -> Option<SharedString> {
    setting
        .split(',')
        .map(|name| name.trim().trim_matches(['"', '\'']).to_string())
        .find(|name| !name.is_empty() && !matches!(name.as_str(), "monospace" | "sans-serif" | "serif"))
        .map(SharedString::from)
}

fn terminal_font(env: &Env, cx: &App) -> (SharedString, Pixels) {
    // `Workbench` sedang dipinjam saat panel dibangun, jadi pengaturan dibaca dari core.
    let settings = env.core.effective_settings(&env.window);
    let installed = cx.text_system().all_font_names();
    let family = settings
        .editor
        .font_family
        .split(',')
        .map(|name| name.trim().trim_matches(['"', '\'']).to_string())
        .find(|name| installed.iter().any(|font| font.eq_ignore_ascii_case(name)))
        .map(SharedString::from)
        .unwrap_or_else(|| cx.theme().mono_font_family.clone());
    let size = px(settings.terminal.font_size.unwrap_or(settings.editor.font_size).clamp(8, 40) as f32);
    (family, size)
}

// ----- Panel -----

/// Panel bawah "Terminal": daftar sesi dan sesi yang aktif.
pub struct TerminalPanel {
    env: Env,
    sessions: Vec<Entity<TerminalSession>>,
    active: usize,
    next_number: usize,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for TerminalPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.sessions.get(self.active) {
            Some(session) => session.focus_handle(cx),
            None => self.focus_handle.clone(),
        }
    }
}

impl TerminalPanel {
    pub fn new(env: Env, cx: &mut Context<Self>) -> Self {
        Self { env, sessions: Vec::new(), active: 0, next_number: 1, focus_handle: cx.focus_handle(), _subscriptions: Vec::new() }
    }

    pub fn sessions(&self) -> &[Entity<TerminalSession>] {
        &self.sessions
    }

    pub fn active_session(&self) -> Option<&Entity<TerminalSession>> {
        self.sessions.get(self.active)
    }

    /// Terminal baru di folder proyek, langsung fokus.
    pub fn new_session(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<TerminalSession> {
        let number = self.next_number;
        self.next_number += 1;
        let env = self.env.clone();
        let session = cx.new(|cx| TerminalSession::new(env, number, window, cx));
        self._subscriptions.push(cx.observe(&session, |_, _, cx| cx.notify()));
        self.sessions.push(session.clone());
        self.active = self.sessions.len() - 1;
        self.focus_active(window, cx);
        cx.notify();
        session
    }

    /// Memastikan ada minimal satu terminal (saat panel dibuka pertama kali).
    pub fn ensure_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sessions.is_empty() {
            self.new_session(window, cx);
        } else {
            self.focus_active(window, cx);
        }
    }

    pub fn focus_active(&self, window: &mut Window, cx: &mut App) {
        if let Some(session) = self.sessions.get(self.active) {
            let handle = session.focus_handle(cx);
            window.defer(cx, move |window, cx| handle.focus(window, cx));
        }
    }

    pub fn kill_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sessions.is_empty() {
            return;
        }
        let session = self.sessions.remove(self.active);
        session.update(cx, |session, cx| session.kill(cx));
        self.active = self.active.min(self.sessions.len().saturating_sub(1));
        self.focus_active(window, cx);
        cx.notify();
    }

    pub fn focus_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sessions.len() > 1 {
            self.active = (self.active + 1) % self.sessions.len();
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    pub fn clear_active(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = self.sessions.get(self.active) {
            session.update(cx, |session, cx| session.clear(cx));
        }
    }

    fn select(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.active = index.min(self.sessions.len().saturating_sub(1));
        self.focus_active(window, cx);
        cx.notify();
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let active = self.sessions.get(self.active).cloned();
        let exited = active.as_ref().is_some_and(|session| session.read(cx).has_exited());
        h_flex()
            .id("terminal-panel")
            .size_full()
            .track_focus(&self.focus_handle)
            .child(
                v_flex()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .when(exited, |el| {
                        el.child(
                            h_flex()
                                .flex_none()
                                .px_2()
                                .py_1()
                                .gap_2()
                                .text_sm()
                                .border_b_1()
                                .border_color(theme.border)
                                .child(div().flex_1().text_color(theme.muted_foreground).child("The process has exited."))
                                .child(Button::new("terminal-restart").label("Restart").xsmall().outline().on_click(cx.listener(|this, _, window, cx| {
                                    if let Some(session) = this.sessions.get(this.active).cloned() {
                                        session.update(cx, |session, cx| session.start(window, cx));
                                    }
                                }))),
                        )
                    })
                    .child(div().flex_1().min_h_0().w_full().map(|el| match active {
                        Some(session) => el.child(session),
                        None => el.flex().items_center().justify_center().text_sm().text_color(theme.muted_foreground).child(
                            Button::new("terminal-start").label("New Terminal").small().outline().on_click(cx.listener(|this, _, window, cx| {
                                this.new_session(window, cx);
                            })),
                        ),
                    })),
            )
            // Daftar sesi di kanan, seperti VS Code, bila ada lebih dari satu.
            .when(self.sessions.len() > 1, |el| {
                el.child(
                    v_flex()
                        .id("terminal-list")
                        .w(px(160.))
                        .h_full()
                        .flex_none()
                        .border_l_1()
                        .border_color(theme.border)
                        .overflow_y_scroll()
                        .children(self.sessions.iter().enumerate().map(|(index, session)| {
                            let title = session.read(cx).title().clone();
                            Button::new(SharedString::from(format!("terminal-tab-{index}")))
                                .ghost()
                                .xsmall()
                                .w_full()
                                .icon(Icon::new(Lucide::SquareTerminal))
                                .label(title)
                                .selected(index == self.active)
                                .on_click(cx.listener(move |this, _, window, cx| this.select(index, window, cx)))
                        })),
                )
            })
    }
}

/// Panel terminal milik jendela ini (dibuat saat panelnya pertama kali dibuka).
pub fn panel(workbench: &Workbench) -> Option<Entity<TerminalPanel>> {
    workbench.extension::<Entity<TerminalPanel>>()
}
