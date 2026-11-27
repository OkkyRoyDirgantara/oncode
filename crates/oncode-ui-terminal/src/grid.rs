//! The terminal screen contents, ready to draw, in cell units: the background per cell, runs of text with
//! the same style, characters drawn by us (see `glyphs.rs`), the selection, and the cursor.
//!
//! Each text run is placed at its own column and each of its letters is forced to be one cell wide
//! when drawn, so letters from a fallback font (icons, emoji, other scripts) do not shift the
//! columns after them. The background is drawn per cell at the full line height, not just behind the letters.
//! Pure (no GPUI) so it can be tested.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions as _;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::Term;
use alacritty_terminal::vte::ansi::{CursorShape, Rgb};

use crate::glyphs;
use crate::palette::{cell_style, Palette};

/// A run of text on one line: adjacent cells with the same style.
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    pub row: usize,
    pub column: usize,
    pub text: String,
    /// The width of each character in cells: 1, or 2 for a wide character (which is always alone).
    pub cell_width: usize,
    /// The number of cells occupied by the whole run.
    pub cells: usize,
    pub fg: Rgb,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

impl TextRun {
    fn same_style(&self, other: &TextRun) -> bool {
        (self.fg, self.bold, self.italic, self.underline, self.strikethrough) == (other.fg, other.bold, other.italic, other.underline, other.strikethrough)
    }
}

/// A character drawn by us.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    pub row: usize,
    pub column: usize,
    pub c: char,
    pub fg: Rgb,
}

/// Adjacent cells on one line (`start..end`) with the same color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub row: usize,
    pub start: usize,
    pub end: usize,
    pub color: Rgb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub row: usize,
    pub column: usize,
    /// 2 if the cursor is on a wide character.
    pub cells: usize,
    /// `Block` only while the terminal is focused (its background and letter color are already in the
    /// `Frame`); without focus it becomes `HollowBlock`.
    pub shape: CursorShape,
    pub color: Rgb,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Frame {
    pub backgrounds: Vec<Span>,
    /// Selected cells; drawn with the palette's selection color (transparent) over the background.
    pub selections: Vec<Span>,
    /// Find bar matches; drawn like the selection, in the match color.
    pub matches: Vec<Span>,
    pub runs: Vec<TextRun>,
    pub glyphs: Vec<Glyph>,
    pub cursor: Option<Cursor>,
}

/// Adds a cell to the last span if it is contiguous and the same color, or opens a new span.
fn extend(spans: &mut Vec<Span>, row: usize, column: usize, cells: usize, color: Rgb) {
    match spans.last_mut() {
        Some(last) if last.row == row && last.end == column && last.color == color => last.end = column + cells,
        _ => spans.push(Span { row, start: column, end: column + cells, color }),
    }
}

/// Builds the visible screen contents (taking the scroll position into account). `show_cursor`: the
/// process is still running.
pub fn build<T: EventListener>(term: &Term<T>, palette: &Palette, focused: bool, show_cursor: bool) -> Frame {
    let content = term.renderable_content();
    let offset = content.display_offset as i32;
    let rows = term.screen_lines();
    let colors = content.colors;
    let selection = content.selection;
    let cursor = content.cursor;
    let cursor_row = cursor.point.line.0 + offset;
    let cursor_visible = show_cursor && cursor.shape != CursorShape::Hidden && cursor_row >= 0 && (cursor_row as usize) < rows;
    let block_cursor = cursor_visible && focused && cursor.shape == CursorShape::Block;

    let mut frame = Frame::default();
    let mut cursor_cells = 1;
    for indexed in content.display_iter {
        let row = indexed.point.line.0 + offset;
        if row < 0 || row as usize >= rows {
            continue;
        }
        let (row, column) = (row as usize, indexed.point.column.0);
        let cell = indexed.cell;
        // The right half of a wide character is already handled together with its left half.
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        let cells = if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
        let at_cursor = cursor_visible && indexed.point == cursor.point;
        if at_cursor {
            cursor_cells = cells;
        }

        let style = cell_style(cell, palette, colors);
        let (mut fg, mut bg) = (style.fg, style.bg);
        if at_cursor && block_cursor {
            fg = palette.cursor_text;
            bg = Some(palette.cursor);
        }
        if let Some(bg) = bg {
            extend(&mut frame.backgrounds, row, column, cells, bg);
        }
        if selection.is_some_and(|range| range.contains(indexed.point)) {
            extend(&mut frame.selections, row, column, cells, palette.selection.0);
        }

        let c = cell.c;
        let blank = matches!(c, ' ' | '\0') && cell.zerowidth().is_none_or(|marks| marks.is_empty());
        if cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) || (blank && !style.underline && !style.strikethrough) {
            continue;
        }
        if glyphs::is_custom(c) {
            frame.glyphs.push(Glyph { row, column, c, fg });
            continue;
        }
        let mut text = String::from(if c == '\0' { ' ' } else { c });
        text.extend(cell.zerowidth().into_iter().flatten());
        let run = TextRun { row, column, text, cell_width: cells, cells, fg, bold: style.bold, italic: style.italic, underline: style.underline, strikethrough: style.strikethrough };
        match frame.runs.last_mut() {
            Some(last)
                if cells == 1
                    && last.cell_width == 1
                    && last.row == row
                    && last.column + last.cells == column
                    && last.same_style(&run) =>
            {
                last.text.push_str(&run.text);
                last.cells += 1;
            }
            _ => frame.runs.push(run),
        }
    }

    if cursor_visible {
        let shape = if !focused && cursor.shape == CursorShape::Block { CursorShape::HollowBlock } else { cursor.shape };
        frame.cursor = Some(Cursor { row: cursor_row as usize, column: cursor.point.column.0, cells: cursor_cells, shape, color: palette.cursor });
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emulator::{Emulator, GridSize};

    const P: Palette = Palette::fallback();

    fn frame(bytes: &str, focused: bool) -> Frame {
        let mut emulator = Emulator::new(GridSize::new(20, 4));
        emulator.feed(bytes.as_bytes());
        build(emulator.term(), &P, focused, true)
    }

    fn texts(frame: &Frame) -> Vec<(usize, usize, &str)> {
        frame.runs.iter().map(|run| (run.row, run.column, run.text.as_str())).collect()
    }

    #[test]
    fn plain_text_is_one_run_per_style_and_skips_blanks() {
        let f = frame("hello world", true);
        assert_eq!(texts(&f), [(0, 0, "hello"), (0, 6, "world")]);
        // `->` stays two characters in one run; its ligature is turned off when drawn.
        assert_eq!(texts(&frame("a -> b", true)), [(0, 0, "a"), (0, 2, "->"), (0, 5, "b")]);
    }

    #[test]
    fn style_changes_split_runs() {
        let f = frame("ab\x1b[1mcd\x1b[0;31mef", true);
        assert_eq!(texts(&f), [(0, 0, "ab"), (0, 2, "cd"), (0, 4, "ef")]);
        assert!(f.runs[1].bold);
        assert_eq!(f.runs[2].fg, P.ansi[1]);
    }

    #[test]
    fn backgrounds_cover_whole_cells_including_spaces() {
        let f = frame("\x1b[41mA B\x1b[0m C", true);
        let red: Vec<Span> = f.backgrounds.iter().copied().filter(|s| s.color == P.ansi[1]).collect();
        assert_eq!(red, [Span { row: 0, start: 0, end: 3, color: P.ansi[1] }], "spaces with a background are colored too");
        assert_eq!(texts(&f), [(0, 0, "A"), (0, 2, "B"), (0, 4, "C")]);
    }

    #[test]
    fn wide_characters_take_two_cells_and_stand_alone() {
        let f = frame("漢字a", true);
        assert_eq!(texts(&f), [(0, 0, "漢"), (0, 2, "字"), (0, 4, "a")]);
        assert_eq!(f.runs[0].cell_width, 2);
        assert_eq!(f.runs[2].cell_width, 1);
    }

    #[test]
    fn box_drawing_and_powerline_are_drawn_as_shapes() {
        let f = frame("┌─┐ x \u{E0B0}", true);
        assert_eq!(f.glyphs.iter().map(|g| (g.column, g.c)).collect::<Vec<_>>(), [(0, '┌'), (1, '─'), (2, '┐'), (6, '\u{E0B0}')]);
        assert_eq!(texts(&f), [(0, 4, "x")]);
    }

    #[test]
    fn focused_block_cursor_inverts_its_cell() {
        let f = frame("ab", true);
        let cursor = f.cursor.unwrap();
        assert_eq!((cursor.row, cursor.column, cursor.shape), (0, 2, CursorShape::Block));
        assert!(f.backgrounds.contains(&Span { row: 0, start: 2, end: 3, color: P.cursor }));

        let unfocused = frame("ab", false);
        assert_eq!(unfocused.cursor.unwrap().shape, CursorShape::HollowBlock);
        assert!(unfocused.backgrounds.is_empty());
    }

    #[test]
    fn cursor_on_a_letter_uses_the_cursor_text_color() {
        let f = frame("ab\x1b[D", true);
        assert_eq!(f.cursor.unwrap().column, 1);
        let b = f.runs.iter().find(|run| run.column == 1).unwrap();
        assert_eq!((b.text.as_str(), b.fg), ("b", P.cursor_text));
    }

    #[test]
    fn hidden_cursor_and_exited_process_show_no_cursor() {
        assert!(frame("ab\x1b[?25l", true).cursor.is_none());
        let mut emulator = Emulator::new(GridSize::new(20, 4));
        emulator.feed(b"ab");
        assert!(build(emulator.term(), &P, true, false).cursor.is_none());
    }

    #[test]
    fn underlined_spaces_are_kept_so_the_line_is_drawn() {
        let f = frame("\x1b[4ma b\x1b[0m", true);
        assert_eq!(texts(&f), [(0, 0, "a b")]);
        assert!(f.runs[0].underline);
    }
}
