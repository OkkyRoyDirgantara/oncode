//! A terminal emulator on top of `alacritty_terminal`: grid, scrollback, selection, search, and
//! link detection. It does not depend on GPUI, so it can be tested without a window.

use std::cell::RefCell;
use std::ops::RangeInclusive;
use std::rc::Rc;
use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Boundary, Column, Direction, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::search::RegexSearch;
use alacritty_terminal::term::{Config, Osc52, Term, TermMode};
use alacritty_terminal::vte::ansi::Processor;

use crate::links::{find_urls, is_web_url};

/// Number of scrollback lines, same as version 0.2.
pub const SCROLLBACK: usize = 10_000;
/// A logical line (a wrapped line) longer than this is not checked for links.
const MAX_LINK_ROWS: i32 = 40;

/// A collector of events from the emulator (replies for the PTY, the bell, and the like).
#[derive(Clone, Default)]
pub struct Events(Rc<RefCell<Vec<Event>>>);

impl EventListener for Events {
    fn send_event(&self, event: Event) {
        self.0.borrow_mut().push(event);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridSize {
    pub cols: usize,
    pub rows: usize,
}

impl GridSize {
    pub const DEFAULT: GridSize = GridSize { cols: 80, rows: 24 };

    pub fn new(cols: usize, rows: usize) -> Self {
        Self { cols: cols.max(2), rows: rows.max(1) }
    }
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// The link under a cell along with the cells it covers (per line, inclusive columns).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub url: String,
    pub spans: Vec<(Line, RangeInclusive<usize>)>,
}

impl Link {
    pub fn contains(&self, point: Point) -> bool {
        self.spans.iter().any(|(line, columns)| *line == point.line && columns.contains(&point.column.0))
    }
}

pub struct Emulator {
    term: Term<Events>,
    events: Events,
    processor: Processor,
}

impl Emulator {
    pub fn new(size: GridSize) -> Self {
        let events = Events::default();
        // OSC 52 is disabled: program output must not write to the clipboard (same as version 0.2).
        let config = Config { scrolling_history: SCROLLBACK, osc52: Osc52::Disabled, ..Config::default() };
        let term = Term::new(config, &size, events.clone());
        Self { term, events, processor: Processor::new() }
    }

    pub fn term(&self) -> &Term<Events> {
        &self.term
    }

    /// Processes PTY output.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.term, bytes);
    }

    /// Writes to the screen from the application side (exit message, clearing). Uses its own parser
    /// so that a PTY escape sequence cut off in the middle does not swallow its text.
    pub fn inject(&mut self, bytes: &[u8]) {
        let mut processor: Processor = Processor::new();
        processor.advance(&mut self.term, bytes);
    }

    /// A synchronized update (DECSET 2026) that the program never closes is ended after its
    /// deadline. Returns the deadline still pending, if any.
    pub fn flush_sync(&mut self) -> Option<Instant> {
        let deadline = self.processor.sync_timeout().sync_timeout()?;
        if Instant::now() >= deadline {
            self.processor.stop_sync(&mut self.term);
            return None;
        }
        Some(deadline)
    }

    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut *self.events.0.borrow_mut())
    }

    pub fn size(&self) -> GridSize {
        GridSize { cols: self.term.columns(), rows: self.term.screen_lines() }
    }

    pub fn resize(&mut self, size: GridSize) {
        if size != self.size() {
            self.term.resize(size);
        }
    }

    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    pub fn history_size(&self) -> usize {
        self.term.history_size()
    }

    pub fn scroll(&mut self, scroll: Scroll) {
        self.term.scroll_display(scroll);
    }

    /// Scrolls so that `offset` lines of history are visible above the screen (0 = bottom).
    pub fn set_display_offset(&mut self, offset: usize) {
        let current = self.display_offset() as i32;
        let target = offset.min(self.history_size()) as i32;
        if target != current {
            self.term.scroll_display(Scroll::Delta(target - current));
        }
    }

    pub fn cursor_point(&self) -> Point {
        self.term.grid().cursor.point
    }

    pub fn cell(&self, line: i32, column: usize) -> &Cell {
        &self.term.grid()[Line(line)][Column(column)]
    }

    /// The contents of a line (0 = the top line of the screen, negative = history) without trailing spaces.
    pub fn line_text(&self, line: i32) -> String {
        let row = &self.term.grid()[Line(line)];
        let mut text = String::new();
        for column in 0..self.term.columns() {
            let cell = &row[Column(column)];
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            text.push(cell.c);
            text.extend(cell.zerowidth().into_iter().flatten());
        }
        text.trim_end().to_string()
    }

    /// The entire buffer contents (history and screen), one grid line per text line.
    pub fn text(&self) -> String {
        let top = self.term.topmost_line().0;
        let bottom = self.term.bottommost_line().0;
        let mut lines: Vec<String> = (top..=bottom).map(|line| self.line_text(line)).collect();
        while lines.last().is_some_and(|line| line.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    }

    /// Clears the history and the screen, leaving the cursor line (the prompt) on the top line.
    pub fn clear(&mut self) {
        if self.mode().contains(TermMode::ALT_SCREEN) {
            return;
        }
        let row = self.cursor_point().line.0;
        if row > 0 {
            self.inject(format!("\x1b[{row}S\x1b[{row}A").as_bytes());
        }
        self.term.grid_mut().clear_history();
        self.term.selection = None;
    }

    // ----- Selection -----

    pub fn start_selection(&mut self, ty: SelectionType, point: Point, side: Side) {
        self.term.selection = Some(Selection::new(ty, point, side));
    }

    pub fn update_selection(&mut self, point: Point, side: Side) {
        if let Some(selection) = self.term.selection.as_mut() {
            selection.update(point, side);
        }
    }

    pub fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    pub fn has_selection(&self) -> bool {
        self.term.selection.as_ref().is_some_and(|selection| selection.to_range(&self.term).is_some())
    }

    /// The selected text; `None` if there is none or it is empty.
    pub fn selection_text(&self) -> Option<String> {
        self.term.selection_to_string().filter(|text| !text.is_empty())
    }

    // ----- Search -----

    /// Searches for `query` (literal text, case-insensitive) forward or backward from the current
    /// selection, wrapping around at the end of the buffer. The match is selected and scrolled into view.
    pub fn find(&mut self, query: &str, forward: bool) -> bool {
        if query.is_empty() {
            return false;
        }
        let Ok(mut regex) = RegexSearch::new(&format!("(?i){}", escape_regex(query))) else { return false };
        let selected = self.term.selection.as_ref().and_then(|selection| selection.to_range(&self.term));
        let viewport_top = Line(-(self.display_offset() as i32));
        let (origin, direction, side) = match (selected, forward) {
            (Some(range), true) => (range.end.add(&self.term, Boundary::None, 1), Direction::Right, Side::Left),
            (Some(range), false) => (range.start.sub(&self.term, Boundary::None, 1), Direction::Left, Side::Right),
            (None, true) => (Point::new(viewport_top, Column(0)), Direction::Right, Side::Left),
            (None, false) => (Point::new(Line(viewport_top.0 + self.term.bottommost_line().0), self.term.last_column()), Direction::Left, Side::Right),
        };
        let Some(found) = self.term.search_next(&mut regex, origin, direction, side, None) else { return false };
        let (start, end) = (*found.start(), *found.end());
        let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
        selection.update(end, Side::Right);
        self.term.selection = Some(selection);
        self.term.scroll_to_point(start);
        true
    }

    // ----- Links -----

    /// The web link under `point`: an OSC 8 hyperlink, or a URL detected in its text
    /// (including one that wraps onto the next line).
    pub fn link_at(&self, point: Point) -> Option<Link> {
        let grid = self.term.grid();
        let last_column = self.term.last_column();
        if point.line < self.term.topmost_line() || point.line > self.term.bottommost_line() || point.column > last_column {
            return None;
        }
        if let Some(hyperlink) = grid[point].hyperlink() {
            if !is_web_url(hyperlink.uri()) {
                return None;
            }
            let same = |column: usize| grid[point.line][Column(column)].hyperlink().as_ref() == Some(&hyperlink);
            let mut start = point.column.0;
            while start > 0 && same(start - 1) {
                start -= 1;
            }
            let mut end = point.column.0;
            while end < last_column.0 && same(end + 1) {
                end += 1;
            }
            return Some(Link { url: hyperlink.uri().to_string(), spans: vec![(point.line, start..=end)] });
        }

        // Logical line: grid lines joined by the wrap marker in the last column.
        let wraps = |line: Line| grid[line][last_column].flags.contains(Flags::WRAPLINE);
        let mut first = point.line;
        while first > self.term.topmost_line() && point.line.0 - first.0 < MAX_LINK_ROWS && wraps(Line(first.0 - 1)) {
            first.0 -= 1;
        }
        let mut last = point.line;
        while last < self.term.bottommost_line() && last.0 - point.line.0 < MAX_LINK_ROWS && wraps(last) {
            last.0 += 1;
        }

        let mut chars: Vec<char> = Vec::new();
        let mut points: Vec<Point> = Vec::new();
        let mut target = None;
        for line in first.0..=last.0 {
            for column in 0..=last_column.0 {
                let cell = &grid[Line(line)][Column(column)];
                let here = Line(line) == point.line && column == point.column.0;
                if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                    // The spacer belongs to the wide character on its left.
                    if here && cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                        target = chars.len().checked_sub(1);
                    }
                    continue;
                }
                if here {
                    target = Some(chars.len());
                }
                chars.push(cell.c);
                points.push(Point::new(Line(line), Column(column)));
            }
        }
        let target = target?;
        let range = find_urls(&chars).into_iter().find(|range| range.contains(&target))?;
        let url: String = chars[range.clone()].iter().collect();
        let mut spans: Vec<(Line, RangeInclusive<usize>)> = Vec::new();
        for index in range {
            let at = points[index];
            // A wide character occupies two cells.
            let width = if grid[at].flags.contains(Flags::WIDE_CHAR) { 1 } else { 0 };
            let end = (at.column.0 + width).min(last_column.0);
            match spans.last_mut() {
                Some((line, columns)) if *line == at.line => *columns = *columns.start()..=end,
                _ => spans.push((at.line, at.column.0..=end)),
            }
        }
        Some(Link { url, spans })
    }
}

/// Turns text into a literal regex pattern.
fn escape_regex(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        if matches!(c, '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$' | '#' | '&' | '-' | '~') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::term::cell::Flags;
    use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};

    use super::*;
    use crate::palette::{cell_style, Palette};

    fn emulator(cols: usize, rows: usize, output: &str) -> Emulator {
        let mut emulator = Emulator::new(GridSize::new(cols, rows));
        emulator.feed(output.as_bytes());
        emulator
    }

    #[test]
    fn plain_output_lands_in_the_grid() {
        let e = emulator(20, 4, "hello\r\nworld");
        assert_eq!(e.line_text(0), "hello");
        assert_eq!(e.line_text(1), "world");
        assert_eq!(e.text(), "hello\nworld");
        assert_eq!(e.cursor_point(), Point::new(Line(1), Column(5)));
    }

    #[test]
    fn sgr_colors_and_attributes_reach_the_cells() {
        // Like `printf '\033[31mred\033[0m \033[1;4;38;5;110mblue\033[0m \033[48;2;1;2;3;7mx\033[0m'`.
        let e = emulator(40, 4, "\x1b[31mred\x1b[0m \x1b[1;4;38;5;110mblue\x1b[0m \x1b[48;2;1;2;3;7mx\x1b[0m");
        assert_eq!(e.line_text(0), "red blue x");
        let red = e.cell(0, 0);
        assert_eq!(red.c, 'r');
        assert_eq!(red.fg, Color::Named(NamedColor::Red));
        assert_eq!(e.cell(0, 3).fg, Color::Named(NamedColor::Foreground), "SGR 0 restores the default color");

        let blue = e.cell(0, 4);
        assert_eq!(blue.fg, Color::Indexed(110));
        assert!(blue.flags.contains(Flags::BOLD | Flags::UNDERLINE));

        let inverse = e.cell(0, 9);
        assert_eq!(inverse.bg, Color::Spec(Rgb { r: 1, g: 2, b: 3 }));
        assert!(inverse.flags.contains(Flags::INVERSE));

        // And down to the ready-to-draw colors.
        let palette = Palette::fallback();
        let overrides = e.term().colors();
        assert_eq!(cell_style(red, &palette, overrides).fg, palette.ansi[1]);
        let styled = cell_style(blue, &palette, overrides);
        assert_eq!(styled.fg, Rgb { r: 0x87, g: 0xaf, b: 0xd7 });
        assert!(styled.bold && styled.underline);
        let styled = cell_style(inverse, &palette, overrides);
        assert_eq!(styled.fg, Rgb { r: 1, g: 2, b: 3 });
        assert_eq!(styled.bg, Some(palette.foreground));
    }

    #[test]
    fn wide_characters_take_two_cells() {
        let e = emulator(10, 2, "a漢b");
        assert!(e.cell(0, 1).flags.contains(Flags::WIDE_CHAR));
        assert!(e.cell(0, 2).flags.contains(Flags::WIDE_CHAR_SPACER));
        assert_eq!(e.cell(0, 3).c, 'b');
        assert_eq!(e.line_text(0), "a漢b");
    }

    #[test]
    fn modes_follow_the_program() {
        let mut e = emulator(20, 4, "");
        assert!(!e.mode().contains(TermMode::APP_CURSOR));
        e.feed(b"\x1b[?1h\x1b[?2004h\x1b[?1000h\x1b[?1006h");
        assert!(e.mode().contains(TermMode::APP_CURSOR | TermMode::BRACKETED_PASTE | TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE));
        e.feed(b"\x1b[?1049h");
        assert!(e.mode().contains(TermMode::ALT_SCREEN));
        e.feed(b"\x1b[?1049l\x1b[?25l");
        assert!(!e.mode().contains(TermMode::ALT_SCREEN | TermMode::SHOW_CURSOR));
        e.feed(b"\x1b[6 q");
        assert_eq!(e.term().cursor_style().shape, CursorShape::Beam);
        e.feed(b"\x1b[2 q");
        assert_eq!(e.term().cursor_style().shape, CursorShape::Block);
    }

    #[test]
    fn device_queries_are_answered_through_events() {
        let mut e = emulator(20, 4, "\x1b[6n");
        let replies: Vec<String> = e
            .take_events()
            .into_iter()
            .filter_map(|event| match event {
                Event::PtyWrite(text) => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(replies, ["\x1b[1;1R"]);
        assert!(e.take_events().is_empty());
    }

    #[test]
    fn output_beyond_the_screen_goes_to_scrollback() {
        let mut e = emulator(10, 3, "");
        for n in 1..=10 {
            e.feed(format!("line {n}\r\n").as_bytes());
        }
        assert_eq!(e.history_size(), 8);
        assert_eq!(e.line_text(0), "line 9");
        assert_eq!(e.line_text(-8), "line 1");
        e.scroll(Scroll::Delta(3));
        assert_eq!(e.display_offset(), 3);
        e.set_display_offset(100);
        assert_eq!(e.display_offset(), 8);
        e.scroll(Scroll::Bottom);
        assert_eq!(e.display_offset(), 0);
    }

    #[test]
    fn resize_reflows_and_keeps_content() {
        let mut e = emulator(10, 3, "0123456789abcdef");
        assert_eq!(e.line_text(0), "0123456789");
        assert_eq!(e.line_text(1), "abcdef");
        e.resize(GridSize::new(20, 3));
        assert_eq!(e.size(), GridSize { cols: 20, rows: 3 });
        assert_eq!(e.line_text(0), "0123456789abcdef");
    }

    #[test]
    fn clear_keeps_only_the_prompt_line() {
        let mut e = emulator(20, 4, "");
        for n in 1..=9 {
            e.feed(format!("output {n}\r\n").as_bytes());
        }
        e.feed(b"$ ");
        e.clear();
        assert_eq!(e.history_size(), 0);
        assert_eq!(e.text(), "$");
        assert_eq!(e.cursor_point(), Point::new(Line(0), Column(2)));
    }

    #[test]
    fn injected_text_is_not_swallowed_by_a_pending_escape_sequence() {
        let mut e = emulator(40, 4, "abc\x1b[3");
        e.inject(b"\r\n[Process exited with code 0]");
        assert_eq!(e.line_text(1), "[Process exited with code 0]");
    }

    #[test]
    fn selection_by_cell_word_and_line() {
        let mut e = emulator(30, 4, "cargo build --release\r\nfirst two");
        e.start_selection(SelectionType::Simple, Point::new(Line(0), Column(0)), Side::Left);
        assert!(!e.has_selection(), "a click without dragging does not select anything yet");
        e.update_selection(Point::new(Line(0), Column(4)), Side::Right);
        assert_eq!(e.selection_text().as_deref(), Some("cargo"));

        e.start_selection(SelectionType::Semantic, Point::new(Line(0), Column(8)), Side::Left);
        assert_eq!(e.selection_text().as_deref(), Some("build"));

        e.start_selection(SelectionType::Lines, Point::new(Line(1), Column(3)), Side::Left);
        assert_eq!(e.selection_text().as_deref(), Some("first two\n"));

        // A selection across lines includes the line ending.
        e.start_selection(SelectionType::Simple, Point::new(Line(0), Column(6)), Side::Left);
        e.update_selection(Point::new(Line(1), Column(4)), Side::Right);
        assert_eq!(e.selection_text().as_deref(), Some("build --release\nfirst"));

        e.clear_selection();
        assert_eq!(e.selection_text(), None);
    }

    #[test]
    fn find_walks_matches_in_both_directions_and_wraps() {
        let mut e = emulator(30, 4, "");
        for n in 1..=12 {
            e.feed(format!("line {n} Error (x.y)\r\n").as_bytes());
        }
        assert!(!e.find("not found", true));
        assert!(!e.find("", true));

        // Without a selection, a forward search starts from the top of the view.
        assert!(e.find("error", true));
        let first = e.term().selection.as_ref().unwrap().to_range(e.term()).unwrap();
        assert_eq!(e.selection_text().as_deref(), Some("Error"), "case-insensitive");
        assert!(e.find("error", true));
        let second = e.term().selection.as_ref().unwrap().to_range(e.term()).unwrap();
        assert_eq!(second.start.line.0, first.start.line.0 + 1);
        assert!(e.find("error", false));
        assert_eq!(e.term().selection.as_ref().unwrap().to_range(e.term()).unwrap(), first);

        // Going backward from the topmost match in the buffer wraps to the bottommost, and vice versa.
        e.clear_selection();
        e.scroll(Scroll::Top);
        assert!(e.find("line 1 ", true));
        assert_eq!(e.term().selection.as_ref().unwrap().to_range(e.term()).unwrap().start.line, e.term().topmost_line());
        assert!(e.find("error", false));
        assert_eq!(e.selection_text().as_deref(), Some("Error"));
        let wrapped = e.term().selection.as_ref().unwrap().to_range(e.term()).unwrap();
        assert_eq!(wrapped.start.line, Line(2), "the last match is on line 12");
        assert!(e.display_offset() <= 1, "the view follows the match until it is visible");

        // Regex special characters are searched for literally.
        assert!(e.find("(x.y)", true));
        assert_eq!(e.selection_text().as_deref(), Some("(x.y)"));
        assert!(!e.find("x+y", true));
    }

    #[test]
    fn find_scrolls_history_matches_into_view() {
        let mut e = emulator(20, 3, "needle\r\n");
        for n in 1..=20 {
            e.feed(format!("hay {n}\r\n").as_bytes());
        }
        assert_eq!(e.display_offset(), 0);
        assert!(e.find("needle", false));
        assert_eq!(e.display_offset(), e.history_size());
    }

    #[test]
    fn detects_links_under_a_cell() {
        let e = emulator(60, 4, "check https://example.com/docs. then\r\nno link");
        let link = e.link_at(Point::new(Line(0), Column(10))).unwrap();
        assert_eq!(link.url, "https://example.com/docs");
        assert_eq!(link.spans, vec![(Line(0), 6..=29)]);
        assert!(link.contains(Point::new(Line(0), Column(6))) && !link.contains(Point::new(Line(0), Column(30))));
        assert_eq!(e.link_at(Point::new(Line(0), Column(2))), None);
        assert_eq!(e.link_at(Point::new(Line(0), Column(30))), None, "the sentence-ending period is not part of the link");
        assert_eq!(e.link_at(Point::new(Line(1), Column(3))), None);
    }

    #[test]
    fn links_continue_across_wrapped_rows() {
        let e = emulator(20, 4, "x https://example.com/a/very/long/path?q=1 y");
        let link = e.link_at(Point::new(Line(1), Column(2))).unwrap();
        assert_eq!(link.url, "https://example.com/a/very/long/path?q=1");
        assert_eq!(link.spans, vec![(Line(0), 2..=19), (Line(1), 0..=19), (Line(2), 0..=1)]);
        assert_eq!(e.link_at(Point::new(Line(0), Column(5))).unwrap().url, link.url);
    }

    #[test]
    fn osc8_hyperlinks_open_only_web_targets() {
        let e = emulator(40, 4, "\x1b]8;;https://example.com/x\x1b\\text\x1b]8;;\x1b\\ \x1b]8;;file:///etc/passwd\x1b\\local\x1b]8;;\x1b\\");
        let link = e.link_at(Point::new(Line(0), Column(1))).unwrap();
        assert_eq!(link.url, "https://example.com/x");
        assert_eq!(link.spans, vec![(Line(0), 0..=3)]);
        assert_eq!(e.link_at(Point::new(Line(0), Column(6))), None, "schemes other than http/https are not opened");
    }

    #[test]
    fn synchronized_updates_are_flushed_after_their_timeout() {
        let mut e = emulator(20, 4, "\x1b[?2026hheld");
        assert_eq!(e.line_text(0), "", "content is held back during a synchronized update");
        assert!(e.flush_sync().is_some());
        e.feed(b"\x1b[?2026l");
        assert_eq!(e.line_text(0), "held");
        assert!(e.flush_sync().is_none());
    }
}
