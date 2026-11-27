//! Pure editor line operations (no GPUI): moving, copying, deleting, and commenting lines.

use std::ops::Range;

/// Result of moving lines: the text range that is replaced, its replacement, and the selection afterwards.
/// All offsets are in bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMove {
    pub replace: Range<usize>,
    pub text: String,
    pub selection: Range<usize>,
}

/// The lines of a text: the start offset of each line.
struct Lines<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> Lines<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, starts: std::iter::once(0).chain(text.match_indices('\n').map(|(index, _)| index + 1)).collect() }
    }

    fn line_of(&self, offset: usize) -> usize {
        self.starts.partition_point(|&start| start <= offset) - 1
    }

    fn start(&self, line: usize) -> usize {
        self.starts[line]
    }

    /// End of a line, excluding its `\n` (and a `\r` before it, so CRLF files keep their line endings).
    fn end(&self, line: usize) -> usize {
        let end = if line + 1 < self.starts.len() { self.starts[line + 1] - 1 } else { self.text.len() };
        if end > self.starts[line] && self.text.as_bytes()[end - 1] == b'\r' {
            end - 1
        } else {
            end
        }
    }

    fn count(&self) -> usize {
        self.starts.len()
    }

    /// The first and last line touched by a selection. A selection ending exactly at the start of a
    /// line does not include that line (selecting one full line with Shift+↓ does not include the next).
    fn covered(&self, selection: &Range<usize>) -> (usize, usize) {
        let first = self.line_of(selection.start);
        let mut last = self.line_of(selection.end);
        if selection.end > selection.start && last > first && self.starts[last] == selection.end {
            last -= 1;
        }
        (first, last)
    }

    /// The line break used by the text (`\r\n` if its first line ends with one).
    fn newline(&self) -> &'static str {
        if self.count() > 1 && self.end(0) + 1 < self.starts[1] {
            "\r\n"
        } else {
            "\n"
        }
    }
}

fn clamp(text: &str, selection: Range<usize>) -> Range<usize> {
    selection.start.min(text.len())..selection.end.min(text.len())
}

/// Copies the lines covered by `selection` and inserts the copy below them (`down`) or above them.
/// The selection moves to the copy below, or stays on the copy above (which is where the original was),
/// as in VS Code's Copy Line Down/Up.
pub fn copy_lines(text: &str, selection: Range<usize>, down: bool) -> LineMove {
    let selection = clamp(text, selection);
    let lines = Lines::new(text);
    let (first, last) = lines.covered(&selection);
    let (start, end) = (lines.start(first), lines.end(last));
    let block = &text[start..end];
    let newline = lines.newline();
    let shift = if down { block.len() + newline.len() } else { 0 };
    LineMove { replace: start..end, text: format!("{block}{newline}{block}"), selection: selection.start + shift..selection.end + shift }
}

/// Deletes the lines covered by `selection`, including their line break. The cursor goes to the start
/// of the line that takes their place.
pub fn delete_lines(text: &str, selection: Range<usize>) -> LineMove {
    let selection = clamp(text, selection);
    let lines = Lines::new(text);
    let (first, last) = lines.covered(&selection);
    let start = lines.start(first);
    let (replace, cursor) = if last + 1 < lines.count() {
        // Up to the start of the next line: the line break goes with the deleted lines.
        (start..lines.start(last + 1), start)
    } else if first > 0 {
        // The last line: the line break before it goes instead, and the cursor ends on the line above.
        let previous_end = lines.end(first - 1);
        (previous_end..text.len(), lines.start(first - 1))
    } else {
        (0..text.len(), 0)
    };
    LineMove { replace, text: String::new(), selection: cursor..cursor }
}

/// Comment tokens of a language: a line comment (`//`) and/or a block comment (`/*`, `*/`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CommentTokens {
    pub line: Option<&'static str>,
    pub block: Option<(&'static str, &'static str)>,
}

/// One edit inside a block of text: `remove` bytes at `at`, then insert `insert`.
struct Edit {
    at: usize,
    remove: usize,
    insert: String,
}

/// Maps an offset of the text to where it ends up after `edits` (sorted by position, not overlapping).
/// `stick_left`: an offset exactly at an insertion point stays before the inserted text (the start of a
/// selection that should include it).
fn map_offset(offset: usize, edits: &[Edit], stick_left: bool) -> usize {
    let mut delta: isize = 0;
    for edit in edits {
        if offset < edit.at || (offset == edit.at && stick_left && edit.remove == 0) {
            break;
        }
        if offset < edit.at + edit.remove {
            // Inside removed text: it moves to where the removal happened.
            return (edit.at as isize + delta) as usize;
        }
        delta += edit.insert.len() as isize - edit.remove as isize;
    }
    (offset as isize + delta) as usize
}

fn apply_edits(text: &str, start: usize, end: usize, edits: &[Edit]) -> String {
    let mut out = String::with_capacity(end - start + edits.len() * 4);
    let mut at = start;
    for edit in edits {
        out.push_str(&text[at..edit.at]);
        out.push_str(&edit.insert);
        at = edit.at + edit.remove;
    }
    out.push_str(&text[at..end]);
    out
}

fn leading_whitespace(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

/// Toggle Line Comment (Ctrl+/): if every non-blank line covered by `selection` starts with the line
/// comment, it is removed (with one space after it); otherwise each non-blank line gets the comment
/// and a space, at the smallest indentation of the block. Blank lines are left alone, unless all lines
/// are blank. A language without a line comment toggles a block comment around the lines instead.
/// `None` when the language has no comments at all.
pub fn toggle_line_comment(text: &str, selection: Range<usize>, tokens: CommentTokens) -> Option<LineMove> {
    let selection = clamp(text, selection);
    let lines = Lines::new(text);
    let (first, last) = lines.covered(&selection);
    let Some(token) = tokens.line else {
        let (open, close) = tokens.block?;
        // Around the text of the lines, after the first line's indentation.
        let start = lines.start(first);
        let start = start + leading_whitespace(&text[start..lines.end(first)]);
        let end = lines.end(last).max(start);
        return Some(toggle_block_range(text, start..end, open, close, Some(selection)));
    };
    let line_text = |line: usize| &text[lines.start(line)..lines.end(line)];
    let non_blank: Vec<usize> = (first..=last).filter(|&line| !line_text(line).trim().is_empty()).collect();
    let commented = !non_blank.is_empty() && non_blank.iter().all(|&line| line_text(line).trim_start().starts_with(token));
    let mut edits = Vec::new();
    if commented {
        for &line in &non_blank {
            let at = lines.start(line) + leading_whitespace(line_text(line));
            let after = &text[at + token.len()..lines.end(line)];
            let remove = token.len() + usize::from(after.starts_with(' '));
            edits.push(Edit { at, remove, insert: String::new() });
        }
    } else {
        let targets: Vec<usize> = if non_blank.is_empty() { (first..=last).collect() } else { non_blank };
        let indent = targets.iter().map(|&line| leading_whitespace(line_text(line))).min().unwrap_or(0);
        for line in targets {
            edits.push(Edit { at: lines.start(line) + indent.min(line_text(line).len()), remove: 0, insert: format!("{token} ") });
        }
    }
    let (start, end) = (lines.start(first), lines.end(last));
    let block = apply_edits(text, start, end, &edits);
    let non_empty = selection.start < selection.end;
    let new_start = map_offset(selection.start, &edits, non_empty) - start;
    let new_end = map_offset(selection.end, &edits, false) - start;
    Some(LineMove { replace: start..end, text: block, selection: start + new_start..start + new_end })
}

/// Toggle Block Comment (Shift+Alt+A): wraps the selection in the block comment, or unwraps it if it
/// is already wrapped (the markers inside the selection or right around it). An empty selection
/// inserts an empty comment with the cursor inside. `None` when the language has no block comment.
pub fn toggle_block_comment(text: &str, selection: Range<usize>, tokens: CommentTokens) -> Option<LineMove> {
    let (open, close) = tokens.block?;
    let selection = clamp(text, selection);
    if selection.is_empty() {
        let at = selection.start;
        let inner = at + open.len() + 1;
        return Some(LineMove { replace: at..at, text: format!("{open}  {close}"), selection: inner..inner });
    }
    Some(toggle_block_range(text, selection, open, close, None))
}

/// Wraps or unwraps `range` in a block comment. The new selection covers the commented text (without
/// the markers), or `keep` mapped through the change.
fn toggle_block_range(text: &str, range: Range<usize>, open: &'static str, close: &'static str, keep: Option<Range<usize>>) -> LineMove {
    let inner = &text[range.clone()];
    let trimmed_start = range.start + (inner.len() - inner.trim_start().len());
    let trimmed_end = range.start + inner.trim_end().len();
    let trimmed = &text[trimmed_start..trimmed_end.max(trimmed_start)];
    let mut edits = Vec::new();
    if trimmed.len() >= open.len() + close.len() && trimmed.starts_with(open) && trimmed.ends_with(close) {
        // The markers are inside the range.
        let after_open = trimmed_start + open.len();
        let before_close = trimmed_end - close.len();
        let open_remove = open.len() + usize::from(text[after_open..before_close].starts_with(' '));
        let close_space = usize::from(before_close > after_open && text[..before_close].ends_with(' ') && before_close - 1 >= trimmed_start + open_remove);
        edits.push(Edit { at: trimmed_start, remove: open_remove, insert: String::new() });
        edits.push(Edit { at: before_close - close_space, remove: close.len() + close_space, insert: String::new() });
    } else if let (Some(open_at), Some(close_len)) = (marker_before(text, range.start, open), marker_after(text, range.end, close)) {
        // The markers are right around the range (the selection a previous toggle left).
        edits.push(Edit { at: open_at, remove: range.start - open_at, insert: String::new() });
        edits.push(Edit { at: range.end, remove: close_len, insert: String::new() });
    } else {
        edits.push(Edit { at: range.start, remove: 0, insert: format!("{open} ") });
        edits.push(Edit { at: range.end, remove: 0, insert: format!(" {close}") });
    }
    let (start, end) = (edits[0].at.min(range.start), (edits[1].at + edits[1].remove).max(range.end));
    let block = apply_edits(text, start, end, &edits);
    let selection = match keep {
        Some(keep) => map_offset(keep.start, &edits, false)..map_offset(keep.end, &edits, false),
        None => {
            let wrapped = edits[0].remove == 0;
            if wrapped {
                range.start + edits[0].insert.len()..range.end + edits[0].insert.len()
            } else {
                map_offset(range.start, &edits, false)..map_offset(range.end, &edits, false)
            }
        }
    };
    LineMove { replace: start..end, text: block, selection }
}

/// Where `open` (and one optional space after it) starts, when it ends exactly at `at`.
fn marker_before(text: &str, at: usize, open: &str) -> Option<usize> {
    let before = &text[..at];
    let before_space = before.strip_suffix(' ').unwrap_or(before);
    before_space.ends_with(open).then(|| before_space.len() - open.len())
}

/// The length of one optional space and `close` starting exactly at `at`.
fn marker_after(text: &str, at: usize, close: &str) -> Option<usize> {
    let after = &text[at..];
    let space = usize::from(after.starts_with(' '));
    after[space..].starts_with(close).then_some(space + close.len())
}

/// Moves the lines covered by `selection` one line up (`up`) or down.
/// The selection moves along with its lines. `None` if already at the first/last line.
///
/// A selection ending exactly at the start of a line does not include that line, same as
/// VS Code (selecting one full line with Shift+↓ does not also move the next line).
pub fn move_lines(text: &str, selection: Range<usize>, up: bool) -> Option<LineMove> {
    let selection = selection.start.min(text.len())..selection.end.min(text.len());
    let starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(index, _)| index + 1)).collect();
    let line_of = |offset: usize| starts.partition_point(|&start| start <= offset) - 1;
    // End of a line, excluding its `\n` character.
    let line_end = |line: usize| if line + 1 < starts.len() { starts[line + 1] - 1 } else { text.len() };

    let first = line_of(selection.start);
    let mut last = line_of(selection.end);
    if selection.end > selection.start && last > first && starts[last] == selection.end {
        last -= 1;
    }
    let block = &text[starts[first]..line_end(last)];
    if up {
        let above = first.checked_sub(1)?;
        let neighbour = &text[starts[above]..line_end(above)];
        let shift = neighbour.len() + 1;
        Some(LineMove {
            replace: starts[above]..line_end(last),
            text: format!("{block}\n{neighbour}"),
            selection: selection.start - shift..selection.end - shift,
        })
    } else {
        let below = last + 1;
        if below >= starts.len() {
            return None;
        }
        let neighbour = &text[starts[below]..line_end(below)];
        let shift = neighbour.len() + 1;
        Some(LineMove {
            replace: starts[first]..line_end(below),
            text: format!("{neighbour}\n{block}"),
            selection: selection.start + shift..selection.end + shift,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUST: CommentTokens = CommentTokens { line: Some("//"), block: Some(("/*", "*/")) };
    const HTML: CommentTokens = CommentTokens { line: None, block: Some(("<!--", "-->")) };

    #[test]
    fn line_comment_toggles_at_the_smallest_indent_and_skips_blank_lines() {
        let text = "fn a() {\n    let x = 1;\n\n        let y = 2;\n}\n";
        let start = text.find("    let x").unwrap();
        let end = text.find("}\n").unwrap();
        let change = toggle_line_comment(text, start..end, RUST).unwrap();
        let commented = apply(text, &change);
        assert_eq!(commented, "fn a() {\n    // let x = 1;\n\n    //     let y = 2;\n}\n");
        // Toggling again with the new selection gives the original back.
        let back = toggle_line_comment(&commented, change.selection.clone(), RUST).unwrap();
        assert_eq!(apply(&commented, &back), text);
    }

    #[test]
    fn line_comment_with_a_cursor_and_mixed_lines() {
        let text = "a\n// b\n";
        // Cursor on "a": one line, commented.
        let change = toggle_line_comment(text, 1..1, RUST).unwrap();
        assert_eq!(apply(text, &change), "// a\n// b\n");
        assert_eq!(change.selection, 4..4, "the cursor stays after the same character");
        // Both lines, one not commented yet: both get commented.
        let change = toggle_line_comment(text, 0..text.len() - 1, RUST).unwrap();
        assert_eq!(apply(text, &change), "// a\n// // b\n");
        // A comment without a space after the token is removed too.
        let change = toggle_line_comment("//x\n", 0..0, RUST).unwrap();
        assert_eq!(apply("//x\n", &change), "x\n");
    }

    #[test]
    fn line_comment_keeps_crlf_and_handles_blank_only_lines() {
        let text = "a\r\nb\r\n";
        let change = toggle_line_comment(text, 0..4, RUST).unwrap();
        assert_eq!(apply(text, &change), "// a\r\n// b\r\n");
        let change = toggle_line_comment("\n", 0..0, RUST).unwrap();
        assert_eq!(apply("\n", &change), "// \n");
        assert_eq!(toggle_line_comment("a", 0..0, CommentTokens::default()), None);
    }

    #[test]
    fn line_comment_without_a_line_token_uses_the_block_comment() {
        let text = "  <p>hi</p>\n";
        let change = toggle_line_comment(text, 3..3, HTML).unwrap();
        let commented = apply(text, &change);
        assert_eq!(commented, "  <!-- <p>hi</p> -->\n");
        let back = toggle_line_comment(&commented, change.selection.clone(), HTML).unwrap();
        assert_eq!(apply(&commented, &back), text);
    }

    #[test]
    fn block_comment_wraps_unwraps_and_inserts_an_empty_comment() {
        let text = "let x = a + b;";
        let start = text.find('a').unwrap();
        let change = toggle_block_comment(text, start..start + 5, RUST).unwrap();
        let wrapped = apply(text, &change);
        assert_eq!(wrapped, "let x = /* a + b */;");
        assert_eq!(&wrapped[change.selection.clone()], "a + b");
        // The selection left by the toggle (inside the markers) unwraps.
        let back = toggle_block_comment(&wrapped, change.selection.clone(), RUST).unwrap();
        assert_eq!(apply(&wrapped, &back), text);
        // A selection that includes the markers unwraps too.
        let all = wrapped.find("/*").unwrap()..wrapped.find("*/").unwrap() + 2;
        let back = toggle_block_comment(&wrapped, all, RUST).unwrap();
        assert_eq!(apply(&wrapped, &back), text);
        // An empty selection inserts an empty comment with the cursor inside.
        let change = toggle_block_comment("x", 1..1, RUST).unwrap();
        assert_eq!(apply("x", &change), "x/*  */");
        assert_eq!(change.selection, 4..4);
        assert_eq!(toggle_block_comment("x", 0..1, CommentTokens { line: Some("#"), block: None }), None);
    }

    #[test]
    fn copies_lines_down_and_up() {
        let text = "one\ntwo\nthree";
        let down = copy_lines(text, 5..5, true);
        assert_eq!(apply(text, &down), "one\ntwo\ntwo\nthree");
        assert_eq!(down.selection, 9..9, "the cursor moves to the copy below");
        let up = copy_lines(text, 5..5, false);
        assert_eq!(apply(text, &up), "one\ntwo\ntwo\nthree");
        assert_eq!(up.selection, 5..5, "the cursor stays on the upper copy");
        // The last line without a trailing newline, and a selection across two lines.
        let last = copy_lines(text, 9..10, true);
        assert_eq!(apply(text, &last), "one\ntwo\nthree\nthree");
        let block = copy_lines(text, 0..6, true);
        assert_eq!(apply(text, &block), "one\ntwo\none\ntwo\nthree");
        let crlf = copy_lines("a\r\nb", 0..0, true);
        assert_eq!(apply("a\r\nb", &crlf), "a\r\na\r\nb");
    }

    #[test]
    fn deletes_lines_with_their_line_break() {
        let text = "one\ntwo\nthree";
        let middle = delete_lines(text, 5..5);
        assert_eq!(apply(text, &middle), "one\nthree");
        assert_eq!(middle.selection, 4..4);
        let last = delete_lines(text, 10..10);
        assert_eq!(apply(text, &last), "one\ntwo");
        assert_eq!(last.selection, 4..4, "the cursor goes to the line above");
        let all = delete_lines("only", 2..2);
        assert_eq!(apply("only", &all), "");
        // A full-line selection (ending at the start of the next line) deletes just that line.
        let full = delete_lines(text, 0..4);
        assert_eq!(apply(text, &full), "two\nthree");
    }

    /// Applies the move result to the text.
    fn apply(text: &str, change: &LineMove) -> String {
        format!("{}{}{}", &text[..change.replace.start], change.text, &text[change.replace.end..])
    }

    #[test]
    fn moves_the_cursor_line_up_and_down_with_the_cursor() {
        let text = "one\ntwo\nthree\n";
        // Cursor on "two", column 1.
        let up = move_lines(text, 5..5, true).unwrap();
        assert_eq!(apply(text, &up), "two\none\nthree\n");
        assert_eq!(up.selection, 1..1, "the cursor stays in the same column on its new line");

        let down = move_lines(text, 5..5, false).unwrap();
        assert_eq!(apply(text, &down), "one\nthree\ntwo\n");
        assert_eq!(&apply(text, &down)[down.selection.start - 1..down.selection.start + 2], "two");
    }

    #[test]
    fn stops_at_the_first_and_last_line() {
        assert_eq!(move_lines("a\nb", 0..0, true), None);
        assert_eq!(move_lines("a\nb", 3..3, false), None);
        assert_eq!(move_lines("", 0..0, false), None);
    }

    #[test]
    fn moves_every_selected_line_as_one_block() {
        let text = "a\nb\nc\nd";
        // Selection from the middle of "b" to the middle of "c".
        let change = move_lines(text, 2..5, false).unwrap();
        assert_eq!(apply(text, &change), "a\nd\nb\nc");
        assert_eq!(&apply(text, &change)[change.selection.clone()], "b\nc");

        let change = move_lines(text, 2..5, true).unwrap();
        assert_eq!(apply(text, &change), "b\nc\na\nd");
    }

    #[test]
    fn a_selection_ending_at_a_line_start_leaves_that_line() {
        let text = "a\nb\nc";
        // Line "b" is fully selected including its `\n` (ending at the start of "c").
        let change = move_lines(text, 2..4, true).unwrap();
        assert_eq!(apply(text, &change), "b\na\nc");
    }

    #[test]
    fn last_line_without_newline_and_multibyte_text() {
        let text = "é1\nü2";
        let change = move_lines(text, 5..5, true).unwrap();
        assert_eq!(apply(text, &change), "ü2\né1");
        let change = move_lines(text, 0..0, false).unwrap();
        assert_eq!(apply(text, &change), "ü2\né1");
        assert!(apply(text, &change).is_char_boundary(change.selection.start));
    }
}
