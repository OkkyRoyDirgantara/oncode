//! Tampilan satu sesi terminal (PTY + emulator). Sesi bisa tampil di panel (lihat `panel.rs`)
//! atau sebagai tab di area editor.
//!
//! Layar digambar per sel di sebuah `canvas` (lihat `grid.rs` dan `paint.rs`): latar setinggi
//! baris penuh, huruf ditaruh di kolomnya masing-masing, dan karakter garis/blok/powerline
//! digambar sebagai bentuk. Ukuran grid dihitung dari lebar dan tinggi area yang tersedia.

use std::cell::Cell as StdCell;
use std::collections::HashSet;
use std::rc::Rc;

use alacritty_terminal::event::Event as TermEmuEvent;
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::TermMode;
use gpui_kit::component::button::Button;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    canvas, div, px, relative, App, Bounds, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, Font, InteractiveElement as _, IntoElement, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _, Pixels, Render, ScrollEvent, SharedString, Styled as _, Subscription, Task,
    TestSupportExt as _, WeakEntity, Window,
};
use oncode_core::api::term::{self, TermEvent};
use oncode_core::Settings;
use oncode_ui::keys::KeyPress;
use oncode_ui::overlay::{self, TextPrompt};
use oncode_ui::{spawn_core, Env};
use tokio::sync::mpsc;

use crate::emulator::{Emulator, GridSize};
use crate::keys::{self, KeyAction, KeyModes, ScrollKey};
use crate::layout::SessionId;
use crate::mouse::{self, MouseAction, MouseEncoding, ReportModifiers, WheelTarget};
use crate::paint::{self, paint_frame, CellMetrics};
use crate::palette::{to_hsla, Palette};
use crate::{font, grid};

const IS_MAC: bool = cfg!(target_os = "macos");

/// Konteks keymap sesi terminal; dipakai juga oleh binding di `init` (lihat `lib.rs`).
pub(crate) const KEY_CONTEXT: &str = "Terminal";

type Metrics = CellMetrics;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// Proses shell berakhir.
    Exited,
    /// Judul (OSC 0/2) berubah.
    TitleChanged,
    /// Terminal ini menerima fokus keyboard.
    Focused,
}

/// Satu terminal: proses shell di PTY dan emulator yang menampilkan keluarannya.
pub struct TerminalSession {
    env: Env,
    session_id: SessionId,
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
    /// Ukuran sel saat grid terakhir disesuaikan; font atau tinggi baris yang berubah juga
    /// mengubah jumlah baris dan kolom, walau ukuran area tetap.
    fitted: Rc<StdCell<Option<Metrics>>>,
    /// Sisa gulir roda (piksel) yang belum genap satu baris.
    scroll_remainder: f32,
    selecting: bool,
    /// Ibu jari scrollbar sedang diseret.
    scrollbar_drag: bool,
    font: Font,
    font_size: Pixels,
    /// Tinggi baris sebagai kelipatan ukuran font.
    line_height: f32,
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
    pub fn new(env: Env, session_id: SessionId, number: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        // Tombol yang bukan milik aplikasi diteruskan ke shell, termasuk Ctrl+W, Ctrl+S, dan sejenisnya.
        oncode_ui::commands::capture_keys(
            &focus_handle,
            |press, window, cx| keys::classify_key(press, IS_MAC, &app_bindings(window, cx)) != KeyAction::App,
            cx,
        );
        let (font, font_size, line_height) = terminal_font(&env.core.effective_settings(&env.window), cx);
        let mut this = Self {
            env,
            session_id,
            emulator: Emulator::new(GridSize::new(80, 24)),
            id: None,
            title: format!("Terminal {number}").into(),
            exited: None,
            error: None,
            input: None,
            focus_handle,
            metrics: None,
            bounds: Rc::new(StdCell::new(None)),
            fitted: Rc::new(StdCell::new(None)),
            scroll_remainder: 0.,
            selecting: false,
            scrollbar_drag: false,
            font,
            font_size,
            line_height,
            _reader: Task::ready(()),
            _subscriptions: Vec::new(),
        };
        let focus = cx.on_focus(&this.focus_handle, window, |_, _, cx| cx.emit(SessionEvent::Focused));
        this._subscriptions.push(focus);
        if let Some(workbench) = this.env.workbench.upgrade() {
            this._subscriptions.push(cx.subscribe_in(&workbench, window, |this, workbench, event: &oncode_ui::WorkbenchEvent, _, cx| {
                if matches!(event, oncode_ui::WorkbenchEvent::SettingsChanged) {
                    // Aturan yang sama dengan saat terminal dibuat (font yang tidak terpasang dilewati).
                    let (font, font_size, line_height) = terminal_font(workbench.read(cx).settings(), cx);
                    if (&font, font_size, line_height) != (&this.font, this.font_size, this.line_height) {
                        (this.font, this.font_size, this.line_height) = (font, font_size, line_height);
                        this.metrics = None;
                        cx.notify();
                    }
                }
            }));
        }
        this.start(window, cx);
        this
    }

    pub fn title(&self) -> &SharedString {
        &self.title
    }

    /// Lebar sel dan tinggi baris saat ini; `None` sebelum terminal pertama kali digambar.
    pub fn cell_size(&self) -> Option<(Pixels, Pixels)> {
        self.metrics.map(|metrics| (metrics.cell_width, metrics.line_height))
    }

    /// Jumlah kolom dan baris grid.
    pub fn grid_size(&self) -> (usize, usize) {
        let size = self.emulator.size();
        (size.cols, size.rows)
    }

    /// Keluarga font yang dipakai (sudah dipastikan terpasang, atau font monospace tema).
    pub fn font_family(&self) -> &SharedString {
        &self.font.family
    }

    /// Id sesi ini (lihat [`SessionId`]); bukan id PTY.
    pub fn session_id(&self) -> SessionId {
        self.session_id
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

    /// Jumlah baris yang sedang digulir ke atas (0 = di bawah, menampilkan keluaran terbaru).
    pub fn display_offset(&self) -> usize {
        self.emulator.display_offset()
    }

    /// Jumlah baris riwayat di atas layar.
    pub fn history_size(&self) -> usize {
        self.emulator.history_size()
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
                        // Spawn berjalan async: tata letak pertama bisa selesai sebelum `id` ada, sehingga
                        // `fit` mengubah emulator tanpa memberi tahu PTY. Selaraskan sekarang, kalau tidak
                        // program di dalamnya (mis. Claude CLI) menggambar untuk 80x24 sampai ada resize.
                        let current = this.emulator.size();
                        if current != size {
                            this.resize_pty(id, current, cx);
                        }
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
        // Gulir riwayat dengan keyboard (di program layar penuh tombolnya milik program itu).
        if !self.emulator.mode().contains(TermMode::ALT_SCREEN) {
            if let Some(key) = keys::scroll_key(&press) {
                self.emulator.scroll(match key {
                    ScrollKey::PageUp => Scroll::PageUp,
                    ScrollKey::PageDown => Scroll::PageDown,
                    ScrollKey::LineUp => Scroll::Delta(1),
                    ScrollKey::LineDown => Scroll::Delta(-1),
                    ScrollKey::Top => Scroll::Top,
                    ScrollKey::Bottom => Scroll::Bottom,
                });
                cx.notify();
                cx.stop_propagation();
                return;
            }
        }
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
        if self.scrollbar_drag {
            if event.pressed_button == Some(MouseButton::Left) {
                self.scroll_to_pointer(event.position.y, cx);
            } else {
                self.scrollbar_drag = false;
            }
            return;
        }
        if !self.selecting || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        if let Some((point, side)) = self.point_at(event.position) {
            self.emulator.update_selection(point, side);
            cx.notify();
        }
    }

    /// Sel layar (kolom, baris) di bawah posisi jendela, berbasis nol.
    fn screen_cell(&self, position: gpui_kit::Point<Pixels>) -> Option<(usize, usize)> {
        let (point, _) = self.point_at(position)?;
        Some((point.column.0, (point.line.0 + self.emulator.display_offset() as i32).max(0) as usize))
    }

    /// Menggulir riwayat ke posisi pointer di lintasan scrollbar.
    fn scroll_to_pointer(&mut self, y: Pixels, cx: &mut Context<Self>) {
        let Some(bounds) = self.bounds.get() else { return };
        let fraction = f32::from(y - bounds.origin.y) / f32::from(bounds.size.height).max(1.0);
        let offset = mouse::offset_at_fraction(fraction, self.emulator.history_size());
        if offset != self.emulator.display_offset() {
            self.emulator.set_display_offset(offset);
            cx.notify();
        }
    }

    fn on_scroll(&mut self, event: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(metrics) = self.metrics else { return };
        let pixels = event.delta.pixel_delta(metrics.line_height);
        // Sebagian platform mengubah Shift+roda menjadi gulir horizontal.
        let delta = if event.modifiers.shift && pixels.y == px(0.) { pixels.x } else { pixels.y };
        let total = self.scroll_remainder + f32::from(delta);
        let lines = (total / f32::from(metrics.line_height)).trunc();
        self.scroll_remainder = total - lines * f32::from(metrics.line_height);
        if lines == 0. {
            return;
        }
        let mode = self.emulator.mode();
        let target = mouse::wheel_target(
            mode.intersects(TermMode::MOUSE_MODE),
            mode.contains(TermMode::ALT_SCREEN),
            mode.contains(TermMode::ALTERNATE_SCROLL),
            event.modifiers.shift,
        );
        match target {
            WheelTarget::Report => {
                // Program yang menangkap mouse (vim `mouse=a`, htop, lazygit) menerima rodanya sendiri.
                let Some((column, row)) = self.screen_cell(event.position) else { return };
                let encoding = if mode.contains(TermMode::SGR_MOUSE) {
                    MouseEncoding::Sgr
                } else if mode.contains(TermMode::UTF8_MOUSE) {
                    MouseEncoding::Utf8
                } else {
                    MouseEncoding::Normal
                };
                let action = if lines > 0. { MouseAction::WheelUp } else { MouseAction::WheelDown };
                let modifiers = ReportModifiers { shift: false, alt: event.modifiers.alt, control: event.modifiers.control };
                for _ in 0..(lines.abs() as usize).min(10) {
                    if let Some(bytes) = mouse::mouse_report(action, column, row, modifiers, encoding) {
                        self.write(bytes);
                    }
                }
            }
            WheelTarget::Arrows => {
                // Program layar penuh (less, man) tanpa laporan mouse: roda menjadi panah atas/bawah.
                let arrow: &[u8] = if lines > 0. { if mode.contains(TermMode::APP_CURSOR) { b"\x1bOA" } else { b"\x1b[A" } } else if mode.contains(TermMode::APP_CURSOR) { b"\x1bOB" } else { b"\x1b[B" };
                for _ in 0..lines.abs() as usize {
                    self.write(arrow.to_vec());
                }
            }
            WheelTarget::History => self.emulator.scroll(Scroll::Delta(lines as i32)),
        }
        cx.notify();
    }

    // ----- Tata letak -----

    fn measure(&mut self, window: &mut Window) -> Metrics {
        let scale = window.scale_factor();
        if let Some(metrics) = self.metrics.filter(|metrics| metrics.scale == scale) {
            return metrics;
        }
        let font_id = window.text_system().resolve_font(&self.font);
        let advance = window.text_system().advance(font_id, self.font_size, 'm').map(|size| size.width).unwrap_or(self.font_size * 0.6);
        let (width, height) = font::cell_size(f32::from(advance), f32::from(self.font_size), self.line_height, scale);
        let metrics = Metrics { cell_width: px(width), line_height: px(height), line: font::line_thickness(f32::from(self.font_size), scale), scale };
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
            self.resize_pty(id, size, cx);
        }
        cx.notify();
    }

    /// Memberi tahu PTY (dan program di dalamnya lewat SIGWINCH/ConPTY) ukuran grid yang baru.
    fn resize_pty(&self, id: u32, size: GridSize, cx: &mut Context<Self>) {
        let core = self.env.core.clone();
        let (cols, rows) = (size.cols as u16, size.rows as u16);
        spawn_core(cx, async move { term::term_resize(&core, id, cols, rows).await }).detach();
    }
}

impl Render for TerminalSession {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let metrics = self.measure(window);
        let palette = Palette::from_theme(cx.theme());
        let focused = self.focus_handle.is_focused(window);
        let background = to_hsla(palette.background);
        let frame = grid::build(self.emulator.term(), &palette, focused, self.exited.is_none());
        let (font, font_size) = (self.font.clone(), self.font_size);

        let bounds_cell = self.bounds.clone();
        let fitted = self.fitted.clone();
        let entity: WeakEntity<Self> = cx.weak_entity();
        let (rows, history, offset) = (self.emulator.size().rows, self.emulator.history_size(), self.emulator.display_offset());
        let thumb = cx.theme().muted_foreground.opacity(if offset > 0 { 0.55 } else { 0.3 });
        let thumb_hover = cx.theme().muted_foreground.opacity(0.7);
        let status = match (&self.error, self.id) {
            (Some(error), _) => Some(format!("The terminal could not start: {error}")),
            (None, None) => Some("Starting terminal…".to_string()),
            _ => None,
        };

        let grid = div()
            .id("terminal-session")
            .test_support()
            .w_full()
            .flex_1()
            .min_h_0()
            .relative()
            .overflow_hidden()
            .px_2()
            .pt_1()
            .bg(background)
            .text_color(to_hsla(palette.foreground))
            .track_focus(&self.focus_handle)
            .key_context(KEY_CONTEXT)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.selecting = false;
                    this.scrollbar_drag = false;
                }),
            )
            .on_scroll(cx.listener(Self::on_scroll))
            .child(
                div()
                    .size_full()
                    .relative()
                    .child(
                        // Mengukur area teks setelah tata letak (perubahan ukuran menghitung ulang
                        // grid), lalu menggambar isi layar per sel.
                        canvas(
                            move |bounds, window, cx| {
                                let changed = bounds_cell.get().map(|b| b.size) != Some(bounds.size) || fitted.get() != Some(metrics);
                                bounds_cell.set(Some(bounds));
                                fitted.set(Some(metrics));
                                if changed {
                                    let entity = entity.clone();
                                    window.defer(cx, move |_, cx| {
                                        let _ = entity.update(cx, |this, cx| this.fit(bounds, cx));
                                    });
                                }
                            },
                            move |bounds, _, window, cx| paint_frame(&frame, bounds.origin, metrics, &font, font_size, &palette, window, cx),
                        )
                        .absolute()
                        .size_full(),
                    ),
            )
            .when(history > 0, |el| {
                // Scrollbar riwayat: klik untuk melompat, seret ibu jarinya untuk menggulir.
                let (top, height) = mouse::scrollbar_thumb(rows, history, offset);
                el.child(
                    div()
                        .id("terminal-scrollbar")
                        .test_support()
                        .group("terminal-scrollbar")
                        .absolute()
                        .top_0()
                        .right_0()
                        .w(px(12.))
                        .h_full()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.scrollbar_drag = true;
                                this.scroll_to_pointer(event.position.y, cx);
                            }),
                        )
                        .child(
                            div()
                                .absolute()
                                .right(px(3.))
                                .w(px(6.))
                                .min_h(px(16.))
                                .top(relative(top))
                                .h(relative(height))
                                .rounded_full()
                                .bg(thumb)
                                .group_hover("terminal-scrollbar", |style| style.bg(thumb_hover)),
                        ),
                )
            })
            .when_some(status, |el, status| {
                el.child(div().absolute().top_2().left_2().text_color(cx.theme().muted_foreground).font_family(cx.theme().font_family.clone()).child(status))
            });

        let theme = cx.theme();
        v_flex()
            .size_full()
            .bg(background)
            .when(self.exited.is_some(), |el| {
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
                        .child(Button::new("terminal-restart").label("Restart").xsmall().outline().on_click(cx.listener(|this, _, window, cx| this.start(window, cx)))),
                )
            })
            .child(grid)
    }
}

/// Font, ukuran, dan tinggi baris terminal menurut pengaturan. Font yang tidak terpasang dilewati;
/// bila tidak ada yang terpasang, font monospace tema dipakai.
fn terminal_font(settings: &Settings, cx: &App) -> (Font, Pixels, f32) {
    let installed = cx.text_system().all_font_names();
    let family = font::terminal_family(settings, &installed).map(SharedString::from).unwrap_or_else(|| cx.theme().mono_font_family.clone());
    (paint::terminal_font(family), px(font::font_size(settings)), font::line_height_factor(settings))
}
