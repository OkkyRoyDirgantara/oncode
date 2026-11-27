//! The view of a single terminal session (PTY + emulator). A session can be shown in the panel (see
//! `panel.rs`) or as a tab in the editor area.
//!
//! The screen is drawn per cell in a `canvas` (see `grid.rs` and `paint.rs`): backgrounds at the
//! full line height, letters placed in their own columns, and line/block/powerline characters
//! drawn as shapes. The grid size is computed from the width and height of the available area.

use std::cell::Cell as StdCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use alacritty_terminal::event::Event as TermEmuEvent;
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::TermMode;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    canvas, div, px, relative, AnyElement, App, AppContext as _, Bounds, ClipboardItem, Context, ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    Font, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _, Pixels, Render, ScrollEvent, SharedString,
    Styled as _, Subscription, Task, TestSupportExt as _, UTF16Selection, WeakEntity, Window,
};
use oncode_core::api::term::{self, TermEvent};
use oncode_core::Settings;
use oncode_ui::keys::KeyPress;
use oncode_ui::{spawn_core, Env, Lucide};
use tokio::sync::mpsc;

use crate::emulator::{Emulator, FindOptions, GridSize};
use crate::flow::FlowControl;
use crate::keys::{self, KeyAction, KeyModes, ScrollKey};
use crate::layout::SessionId;
use crate::mouse::{self, MouseAction, MouseEncoding, ReportButton, ReportModifiers, WheelTarget};
use crate::paint::{self, paint_frame, CellMetrics};
use crate::palette::{to_hsla, Palette};
use crate::{font, grid};

const IS_MAC: bool = cfg!(target_os = "macos");

/// The keymap context of a terminal session; also used by the binding in `init` (see `lib.rs`).
pub(crate) const KEY_CONTEXT: &str = "Terminal";

type Metrics = CellMetrics;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// The shell process ended.
    Exited,
    /// The title (OSC 0/2) changed.
    TitleChanged,
    /// This terminal received keyboard focus.
    Focused,
}

/// A single terminal: a shell process on a PTY and the emulator that displays its output.
pub struct TerminalSession {
    env: Env,
    session_id: SessionId,
    pub(crate) emulator: Emulator,
    /// The PTY id from `oncode-core`; `None` while the process is starting.
    id: Option<u32>,
    title: SharedString,
    exited: Option<Option<i32>>,
    error: Option<String>,
    input: Option<mpsc::UnboundedSender<Vec<u8>>>,
    focus_handle: FocusHandle,
    metrics: Option<Metrics>,
    /// The last measured size of the area, for recomputing the grid.
    bounds: Rc<StdCell<Option<Bounds<Pixels>>>>,
    /// The cell size when the grid was last fitted; a changed font or line height also
    /// changes the number of lines and columns, even if the area size stays the same.
    fitted: Rc<StdCell<Option<Metrics>>>,
    /// The wheel scroll remainder (pixels) that does not yet add up to a whole line.
    scroll_remainder: f32,
    selecting: bool,
    /// The mouse button pressed and reported to the program (mouse reporting), and the last cell reported.
    reported_button: Option<ReportButton>,
    reported_cell: Option<(usize, usize)>,
    /// Text an input method (IME) is still composing; drawn underlined at the cursor, sent once committed.
    preedit: Option<String>,
    /// The inline find bar, while open.
    find_bar: Option<FindBar>,
    /// The scrollbar thumb is being dragged.
    scrollbar_drag: bool,
    font: Font,
    font_size: Pixels,
    /// Line height as a multiple of the font size.
    line_height: f32,
    /// Bytes received but not yet processed; pauses PTY reading when the UI falls behind.
    flow: Arc<FlowControl>,
    _reader: Task<()>,
    /// Ends a synchronized update (DECSET 2026) the program never closes, at its deadline.
    _sync_flush: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SessionEvent> for TerminalSession {}

/// Most matches the find bar collects (the scrollback holds 10,000 lines).
const MAX_FIND_MATCHES: usize = 1000;

/// The terminal's inline find bar.
struct FindBar {
    input: Entity<InputState>,
    options: FindOptions,
    matches: Vec<std::ops::RangeInclusive<GridPoint>>,
    current: Option<usize>,
    _subscription: Subscription,
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The terminal as a text input target: committed text goes to the shell, composed (marked) text is kept
/// and drawn until the input method commits it. There is no editable text otherwise, so ranges refer to
/// the composed text only.
impl EntityInputHandler for TerminalSession {
    fn text_for_range(&mut self, range: std::ops::Range<usize>, adjusted: &mut Option<std::ops::Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        let preedit = self.preedit.as_deref()?;
        let units: Vec<u16> = preedit.encode_utf16().collect();
        let range = range.start.min(units.len())..range.end.min(units.len());
        *adjusted = Some(range.clone());
        Some(String::from_utf16_lossy(&units[range]))
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let end = self.preedit.as_deref().map_or(0, utf16_len);
        Some(UTF16Selection { range: end..end, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<std::ops::Range<usize>> {
        self.preedit.as_deref().map(|text| 0..utf16_len(text))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.preedit.take().is_some() {
            cx.notify();
        }
    }

    fn replace_text_in_range(&mut self, _: Option<std::ops::Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        self.preedit = None;
        if !text.is_empty() {
            self.emulator.clear_selection();
            self.emulator.set_display_offset(0);
            self.write(text.as_bytes().to_vec());
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(&mut self, _: Option<std::ops::Range<usize>>, text: &str, _: Option<std::ops::Range<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        self.preedit = (!text.is_empty()).then(|| text.to_string());
        cx.notify();
    }

    fn bounds_for_range(&mut self, range: std::ops::Range<usize>, _: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let cell = self.cursor_cell_bounds()?;
        let metrics = self.metrics?;
        // Each composed character takes one cell (wide characters two; close enough for placing the IME window).
        let offset = metrics.cell_width * range.start as f32;
        Some(Bounds::new(gpui_kit::point(cell.origin.x + offset, cell.origin.y), cell.size))
    }

    fn character_index_for_point(&mut self, _: gpui_kit::Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

/// Draws the text an input method is composing at the cursor: on the background, underlined.
fn paint_preedit(text: &str, cell: Bounds<Pixels>, metrics: Metrics, font: &Font, font_size: Pixels, palette: &Palette, window: &mut Window, cx: &mut App) {
    let color = to_hsla(palette.foreground);
    let run = gpui_kit::TextRun {
        len: text.len(),
        font: font.clone(),
        color,
        background_color: Some(to_hsla(palette.background)),
        underline: Some(gpui_kit::UnderlineStyle { thickness: px(1.), color: Some(color), wavy: false }),
        strikethrough: None,
    };
    let line = window.text_system().shape_line(text.to_string().into(), font_size, &[run], None);
    let _ = line.paint(cell.origin, metrics.line_height, gpui_kit::TextAlign::Left, None, window, cx);
}

impl Focusable for TerminalSession {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Application keybindings that stay in effect while the terminal is focused (see `keys::COMMANDS_TO_SKIP_SHELL`).
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
        // Keys that do not belong to the application are forwarded to the shell, including Ctrl+W, Ctrl+S, and the like.
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
            reported_button: None,
            reported_cell: None,
            preedit: None,
            find_bar: None,
            scrollbar_drag: false,
            font,
            font_size,
            line_height,
            flow: Arc::new(FlowControl::default()),
            _reader: Task::ready(()),
            _sync_flush: Task::ready(()),
            _subscriptions: Vec::new(),
        };
        let focus = cx.on_focus(&this.focus_handle, window, |this, _, cx| {
            // Focus reporting (DECSET 1004): programs like vim and tmux redraw or reload on focus.
            if this.emulator.mode().contains(TermMode::FOCUS_IN_OUT) {
                this.write(b"\x1b[I".to_vec());
            }
            cx.emit(SessionEvent::Focused);
        });
        this._subscriptions.push(focus);
        let blur = cx.on_blur(&this.focus_handle, window, |this, _, _| {
            if this.emulator.mode().contains(TermMode::FOCUS_IN_OUT) {
                this.write(b"\x1b[O".to_vec());
            }
        });
        this._subscriptions.push(blur);
        if let Some(workbench) = this.env.workbench.upgrade() {
            this._subscriptions.push(cx.subscribe_in(&workbench, window, |this, workbench, event: &oncode_ui::WorkbenchEvent, _, cx| {
                if matches!(event, oncode_ui::WorkbenchEvent::SettingsChanged) {
                    // The same rules as when the terminal was created (a font that is not installed is skipped).
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

    /// The current cell width and line height; `None` before the terminal is first drawn.
    pub fn cell_size(&self) -> Option<(Pixels, Pixels)> {
        self.metrics.map(|metrics| (metrics.cell_width, metrics.line_height))
    }

    /// The number of grid columns and lines.
    pub fn grid_size(&self) -> (usize, usize) {
        let size = self.emulator.size();
        (size.cols, size.rows)
    }

    /// The font family in use (confirmed to be installed, or the theme's monospace font).
    pub fn font_family(&self) -> &SharedString {
        &self.font.family
    }

    /// This session's id (see [`SessionId`]); not the PTY id.
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

    /// The buffer contents (history and screen) as text; for tests and "copy all".
    pub fn text(&self) -> String {
        self.emulator.text()
    }

    /// The number of lines currently scrolled up (0 = at the bottom, showing the newest output).
    pub fn display_offset(&self) -> usize {
        self.emulator.display_offset()
    }

    /// The number of history lines above the screen.
    pub fn history_size(&self) -> usize {
        self.emulator.history_size()
    }

    /// Starts the shell process (also for "Restart" after the process has ended).
    pub fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.exited = None;
        self.error = None;
        self.emulator = Emulator::new(self.emulator.size());
        let (output_tx, mut output_rx) = mpsc::unbounded_channel::<TermEvent>();
        let (input_tx, mut input_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        self.input = Some(input_tx);
        let size = self.emulator.size();
        let (core, label) = (self.env.core.clone(), self.env.window.to_string());
        let flow = Arc::new(FlowControl::default());
        self.flow = flow.clone();
        // The PTY id, once known: output can arrive before `term_spawn` returns.
        let pty_id = Arc::new(OnceLock::<u32>::new());
        let spawn = spawn_core(cx, async move {
            let (callback_core, callback_id) = (core.clone(), pty_id.clone());
            let callback: oncode_core::api::Callback<TermEvent> = Arc::new(move |event| {
                // Counted on the delivering thread, so reading pauses while the UI thread is still busy.
                if let TermEvent::Data { data } = &event {
                    if flow.written(data.len()) == Some(true) {
                        if let Some(id) = callback_id.get() {
                            let _ = callback_core.terminals.set_paused(*id, true);
                        }
                    }
                }
                let _ = output_tx.send(event);
            });
            let id = term::term_spawn(&core, &label, size.cols as u16, size.rows as u16, callback).await?;
            let _ = pty_id.set(id);
            // A single writer per terminal so the input order is preserved.
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
                        // Spawn runs async: the first layout can finish before `id` exists, so
                        // `fit` resizes the emulator without telling the PTY. Sync it now, otherwise
                        // the program inside (e.g. Claude CLI) draws for 80x24 until there is a resize.
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
            // Output is collected in waves so heavy output does not redraw per chunk.
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
        let mut processed = 0;
        for event in batch {
            match event {
                TermEvent::Data { data } => {
                    self.emulator.feed(&data);
                    processed += data.len();
                }
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
                // The emulator's reply to the program (e.g. a cursor position report).
                TermEmuEvent::PtyWrite(text) => self.write(text.into_bytes()),
                TermEmuEvent::Title(title) if !title.trim().is_empty() => {
                    self.title = title.into();
                    cx.emit(SessionEvent::TitleChanged);
                }
                _ => {}
            }
        }
        if self.flow.processed(processed) == Some(false) {
            if let Some(id) = self.id {
                let core = self.env.core.clone();
                spawn_core(cx, async move { term::term_set_paused(&core, id, false).await }).detach();
            }
        }
        self.schedule_sync_flush(cx);
        // New output can add or move matches.
        if self.find_bar.is_some() {
            self.refresh_matches(false, cx);
        }
        cx.notify();
    }

    /// While a synchronized update is open, wakes up at its deadline to end it if the program has not.
    fn schedule_sync_flush(&mut self, cx: &mut Context<Self>) {
        let Some(deadline) = self.emulator.flush_sync() else {
            self._sync_flush = Task::ready(());
            return;
        };
        let wait = deadline.saturating_duration_since(Instant::now());
        self._sync_flush = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |this, cx| {
                this.schedule_sync_flush(cx);
                cx.notify();
            });
        });
    }

    /// Sends bytes to the shell.
    pub fn write(&mut self, bytes: Vec<u8>) {
        if self.exited.is_some() {
            return;
        }
        if let Some(input) = &self.input {
            let _ = input.send(bytes);
        }
    }

    /// Sends text as if typed (used by the "Run Selected Text" command and tests).
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

    /// Opens the inline find bar (Ctrl+F), filled with the selected text if there is any.
    fn find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.emulator.selection_text().filter(|text| !text.contains('\n'));
        if self.find_bar.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find"));
            let subscription = cx.subscribe_in(&input, window, |this: &mut Self, _, event: &InputEvent, _, cx| match event {
                InputEvent::Change => this.refresh_matches(true, cx),
                InputEvent::PressEnter { shift, .. } => this.step_match(!*shift, cx),
                _ => {}
            });
            self.find_bar = Some(FindBar { input, options: FindOptions::default(), matches: Vec::new(), current: None, _subscription: subscription });
        }
        let Some(bar) = &self.find_bar else { return };
        let input = bar.input.clone();
        input.update(cx, |input, cx| {
            if let Some(text) = selected {
                input.set_value(text, window, cx);
            }
            input.focus(window, cx);
        });
        self.refresh_matches(true, cx);
        cx.notify();
    }

    /// Closes the find bar and gives the keyboard back to the terminal.
    pub fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find_bar.take().is_some() {
            self.focus_handle.focus(window, cx);
            cx.notify();
        }
    }

    /// The query in the find bar, how many matches there are, and which one is current (from 0).
    pub fn find_state(&self, cx: &App) -> Option<(String, usize, Option<usize>)> {
        let bar = self.find_bar.as_ref()?;
        Some((bar.input.read(cx).value().to_string(), bar.matches.len(), bar.current))
    }

    /// Searches the buffer again. `jump`: select the match nearest the bottom (the newest output), as a new
    /// search does; otherwise keep the current match where possible (new output arrived).
    fn refresh_matches(&mut self, jump: bool, cx: &mut Context<Self>) {
        let Some(bar) = &mut self.find_bar else { return };
        let query = bar.input.read(cx).value().to_string();
        let previous = bar.current;
        bar.matches = self.emulator.find_all(&query, bar.options, MAX_FIND_MATCHES);
        bar.current = if bar.matches.is_empty() {
            None
        } else if jump {
            Some(bar.matches.len() - 1)
        } else {
            // New output adds matches at the bottom and shifts the line numbers of the older ones (the screen
            // scrolls), so the current match keeps its place in the list, not its position on the grid.
            Some(previous.unwrap_or(usize::MAX).min(bar.matches.len() - 1))
        };
        if jump {
            match bar.current.and_then(|index| bar.matches.get(index).cloned()) {
                Some(range) => self.emulator.select_range(&range),
                None => self.emulator.clear_selection(),
            }
        }
        cx.notify();
    }

    /// Next (`forward`) or previous match, wrapping around.
    fn step_match(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(bar) = &mut self.find_bar else { return };
        let count = bar.matches.len();
        if count == 0 {
            return;
        }
        let current = bar.current.unwrap_or(0);
        let next = if forward { (current + 1) % count } else { (current + count - 1) % count };
        bar.current = Some(next);
        let range = bar.matches[next].clone();
        self.emulator.select_range(&range);
        cx.notify();
    }

    fn toggle_find_option(&mut self, change: fn(&mut FindOptions), cx: &mut Context<Self>) {
        if let Some(bar) = &mut self.find_bar {
            change(&mut bar.options);
            self.refresh_matches(true, cx);
        }
    }

    /// The cells of the visible matches, row by row, for highlighting.
    fn visible_match_spans(&self) -> Vec<grid::Span> {
        let Some(bar) = &self.find_bar else { return Vec::new() };
        let size = self.emulator.size();
        let offset = self.emulator.display_offset() as i32;
        let mut spans = Vec::new();
        for range in &bar.matches {
            let (start, end) = (*range.start(), *range.end());
            for line in start.line.0..=end.line.0 {
                let row = line + offset;
                if row < 0 || row as usize >= size.rows {
                    continue;
                }
                let from = if line == start.line.0 { start.column.0 } else { 0 };
                let to = if line == end.line.0 { end.column.0 + 1 } else { size.cols };
                spans.push(grid::Span { row: row as usize, start: from, end: to.min(size.cols), color: alacritty_terminal::vte::ansi::Rgb { r: 0, g: 0, b: 0 } });
            }
        }
        spans
    }

    fn render_find_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let bar = self.find_bar.as_ref()?;
        let count = match (bar.matches.len(), bar.current) {
            (0, _) if bar.input.read(cx).value().is_empty() => String::new(),
            (0, _) => "No results".to_string(),
            (total, Some(current)) => format!("{} of {total}{}", current + 1, if total >= MAX_FIND_MATCHES { "+" } else { "" }),
            (total, None) => total.to_string(),
        };
        let toggle = |id: &'static str, label: &'static str, tooltip: &'static str, on: bool, change: fn(&mut FindOptions), cx: &mut Context<Self>| {
            Button::new(id).xsmall().label(label).tooltip(tooltip).when(on, |b| b.primary()).when(!on, |b| b.ghost()).on_click(cx.listener(move |this, _, window, cx| {
                this.toggle_find_option(change, cx);
                // The keyboard goes back to the search text.
                if let Some(bar) = &this.find_bar {
                    bar.input.update(cx, |input, cx| input.focus(window, cx));
                }
            }))
        };
        let options = bar.options;
        let case = toggle("terminal-find-case", "Aa", "Match Case", options.case_sensitive, |o| o.case_sensitive = !o.case_sensitive, cx);
        let word = toggle("terminal-find-word", "ab", "Match Whole Word", options.whole_word, |o| o.whole_word = !o.whole_word, cx);
        let regex = toggle("terminal-find-regex", ".*", "Use Regular Expression", options.regex, |o| o.regex = !o.regex, cx);
        let theme = cx.theme();
        Some(
            h_flex()
                .id("terminal-find")
                .test_support()
                .absolute()
                .top_1()
                .right_4()
                .gap_1()
                .p_1()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .shadow_md()
                // Clicks in the bar stay here: the terminal underneath would take the focus and start a selection.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        this.close_find(window, cx);
                        cx.stop_propagation();
                    } else if event.keystroke.key == "f3" {
                        this.step_match(!event.keystroke.modifiers.shift, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(div().w(px(200.)).child(Input::new(&bar.input).xsmall()))
                .child(case)
                .child(word)
                .child(regex)
                .child(div().min_w(px(64.)).text_xs().text_color(theme.muted_foreground).child(count))
                .child(Button::new("terminal-find-previous").xsmall().ghost().icon(Icon::new(Lucide::ArrowUp)).tooltip("Previous Match (Shift+Enter)").on_click(cx.listener(|this, _, _, cx| this.step_match(false, cx))))
                .child(Button::new("terminal-find-next").xsmall().ghost().icon(Icon::new(Lucide::ArrowDown)).tooltip("Next Match (Enter)").on_click(cx.listener(|this, _, _, cx| this.step_match(true, cx))))
                .child(Button::new("terminal-find-close").xsmall().ghost().icon(Icon::new(Lucide::X)).tooltip("Close (Escape)").on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx))))
                .into_any_element(),
        )
    }

    // ----- Input -----

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let press = press_of(event);
        // Scroll the history with the keyboard (in a full-screen program those keys belong to that program).
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
                // Ordinary text (letters, digits, symbols, Shift+letter) is not written here: GPUI hands it to
                // this terminal's input handler (`replace_text_in_range`), which is also how an input method
                // (IME) composes and commits text.
                let Some(bytes) = keys::key_to_bytes(&press, modes, IS_MAC) else { return };
                self.emulator.clear_selection();
                self.emulator.set_display_offset(0);
                self.write(bytes);
                cx.notify();
            }
        }
        cx.stop_propagation();
    }

    /// The text an input method is composing, if any.
    pub fn preedit(&self) -> Option<&str> {
        self.preedit.as_deref()
    }

    /// The bounds of the cursor cell in window coordinates (where the IME window and composed text go).
    fn cursor_cell_bounds(&self) -> Option<Bounds<Pixels>> {
        let bounds = self.bounds.get()?;
        let metrics = self.metrics?;
        let point = self.emulator.cursor_point();
        let row = (point.line.0 + self.emulator.display_offset() as i32).max(0) as f32;
        let origin = gpui_kit::point(bounds.origin.x + metrics.cell_width * point.column.0 as f32, bounds.origin.y + metrics.line_height * row);
        Some(Bounds::new(origin, gpui_kit::size(metrics.cell_width, metrics.line_height)))
    }

    /// The grid point under a window position, along with the cell side (for selection).
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

    /// The encoding of mouse reports, when the program asked for the mouse and Shift is not held (Shift
    /// keeps selecting text locally, as in other terminals).
    fn mouse_reporting(&self, shift: bool) -> Option<MouseEncoding> {
        let mode = self.emulator.mode();
        if shift || !mode.intersects(TermMode::MOUSE_MODE) {
            return None;
        }
        Some(if mode.contains(TermMode::SGR_MOUSE) {
            MouseEncoding::Sgr
        } else if mode.contains(TermMode::UTF8_MOUSE) {
            MouseEncoding::Utf8
        } else {
            MouseEncoding::Normal
        })
    }

    fn report_mouse(&mut self, action: MouseAction, cell: (usize, usize), modifiers: &gpui_kit::Modifiers, encoding: MouseEncoding) {
        let modifiers = ReportModifiers { shift: false, alt: modifiers.alt, control: modifiers.control };
        if let Some(bytes) = mouse::mouse_report(action, cell.0, cell.1, modifiers, encoding) {
            self.write(bytes);
        }
        self.reported_cell = Some(cell);
    }

    /// A middle or right button press: reported to a program that asked for the mouse, ignored otherwise.
    fn on_other_mouse_down(&mut self, button: ReportButton, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
        let (Some(encoding), Some(cell)) = (self.mouse_reporting(event.modifiers.shift), self.screen_cell(event.position)) else { return };
        self.report_mouse(MouseAction::Press(button), cell, &event.modifiers, encoding);
        self.reported_button = Some(button);
        cx.stop_propagation();
    }

    fn on_mouse_up_button(&mut self, button: ReportButton, event: &gpui_kit::MouseUpEvent) {
        if self.reported_button == Some(button) {
            self.reported_button = None;
            if let (Some(encoding), Some(cell)) = (self.mouse_reporting(false), self.screen_cell(event.position)) {
                self.report_mouse(MouseAction::Release(button), cell, &event.modifiers, encoding);
            }
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
        let Some((point, side)) = self.point_at(event.position) else { return };
        // Ctrl/Cmd+click opens a web link.
        let open_link = if IS_MAC { event.modifiers.platform } else { event.modifiers.control };
        if open_link {
            if let Some(link) = self.emulator.link_at(point) {
                cx.open_url(&link.url);
                return;
            }
        }
        // A program that asked for the mouse (htop, lazygit, vim with mouse=a) gets the click.
        if let (Some(encoding), Some(cell)) = (self.mouse_reporting(event.modifiers.shift), self.screen_cell(event.position)) {
            self.report_mouse(MouseAction::Press(ReportButton::Left), cell, &event.modifiers, encoding);
            self.reported_button = Some(ReportButton::Left);
            return;
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
        // Mouse reporting: motion with a button held (1002) or any motion (1003), once per cell.
        if let Some(encoding) = self.mouse_reporting(event.modifiers.shift) {
            let mode = self.emulator.mode();
            let report = match self.reported_button {
                Some(_) => mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION),
                None => mode.contains(TermMode::MOUSE_MOTION),
            };
            if let Some(cell) = self.screen_cell(event.position).filter(|cell| report && Some(*cell) != self.reported_cell) {
                self.report_mouse(MouseAction::Motion(self.reported_button), cell, &event.modifiers, encoding);
            }
            if self.reported_button.is_some() {
                return;
            }
        }
        if !self.selecting || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        if let Some((point, side)) = self.point_at(event.position) {
            self.emulator.update_selection(point, side);
            cx.notify();
        }
    }

    /// The screen cell (column, line) under a window position, zero-based.
    fn screen_cell(&self, position: gpui_kit::Point<Pixels>) -> Option<(usize, usize)> {
        let (point, _) = self.point_at(position)?;
        Some((point.column.0, (point.line.0 + self.emulator.display_offset() as i32).max(0) as usize))
    }

    /// Scrolls the history to the pointer position on the scrollbar track.
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
        // Some platforms turn Shift+wheel into a horizontal scroll.
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
                // A program that captures the mouse (vim `mouse=a`, htop, lazygit) receives the wheel itself.
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
                // A full-screen program (less, man) without mouse reporting: the wheel becomes up/down arrows.
                let arrow: &[u8] = if lines > 0. { if mode.contains(TermMode::APP_CURSOR) { b"\x1bOA" } else { b"\x1b[A" } } else if mode.contains(TermMode::APP_CURSOR) { b"\x1bOB" } else { b"\x1b[B" };
                for _ in 0..lines.abs() as usize {
                    self.write(arrow.to_vec());
                }
            }
            WheelTarget::History => self.emulator.scroll(Scroll::Delta(lines as i32)),
        }
        cx.notify();
    }

    // ----- Layout -----

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

    /// Fits the grid to the area size; called after layout.
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

    /// Tells the PTY (and the program inside via SIGWINCH/ConPTY) the new grid size.
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
        let mut frame = grid::build(self.emulator.term(), &palette, focused, self.exited.is_none());
        frame.matches = self.visible_match_spans();
        let find_bar = self.render_find_bar(cx);
        let (font, font_size) = (self.font.clone(), self.font_size);
        let (input_focus, input_entity) = (self.focus_handle.clone(), cx.entity());
        let preedit = self.preedit.clone().zip(self.cursor_cell_bounds());

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
            .on_mouse_down(MouseButton::Middle, cx.listener(|this, event, window, cx| this.on_other_mouse_down(ReportButton::Middle, event, window, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, event, window, cx| this.on_other_mouse_down(ReportButton::Right, event, window, cx)))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event, _, _| {
                    this.selecting = false;
                    this.scrollbar_drag = false;
                    this.on_mouse_up_button(ReportButton::Left, event);
                }),
            )
            .on_mouse_up(MouseButton::Middle, cx.listener(|this, event, _, _| this.on_mouse_up_button(ReportButton::Middle, event)))
            .on_mouse_up(MouseButton::Right, cx.listener(|this, event, _, _| this.on_mouse_up_button(ReportButton::Right, event)))
            .on_scroll(cx.listener(Self::on_scroll))
            .child(
                div()
                    .size_full()
                    .relative()
                    .child(
                        // Measures the text area after layout (a size change recomputes the
                        // grid), then draws the screen contents per cell.
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
                            move |bounds, _, window, cx| {
                                paint_frame(&frame, bounds.origin, metrics, &font, font_size, &palette, window, cx);
                                // Typed text and input methods reach the terminal through this handler.
                                window.handle_input(&input_focus, ElementInputHandler::new(bounds, input_entity.clone()), cx);
                                if let Some((text, cell)) = preedit.as_ref() {
                                    paint_preedit(text, *cell, metrics, &font, font_size, &palette, window, cx);
                                }
                            },
                        )
                        .absolute()
                        .size_full(),
                    ),
            )
            .when(history > 0, |el| {
                // The history scrollbar: click to jump, drag its thumb to scroll.
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
            })
            .children(find_bar);

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

/// The terminal's font, size, and line height according to the settings. A font that is not installed is
/// skipped; if none is installed, the theme's monospace font is used.
fn terminal_font(settings: &Settings, cx: &App) -> (Font, Pixels, f32) {
    let installed = cx.text_system().all_font_names();
    let family = font::terminal_family(settings, &installed).map(SharedString::from).unwrap_or_else(|| cx.theme().mono_font_family.clone());
    (paint::terminal_font(family), px(font::font_size(settings)), font::line_height_factor(settings))
}
