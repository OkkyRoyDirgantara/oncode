//! Pure editor line operations (no GPUI): moving lines up or down.

use std::ops::Range;

/// Result of moving lines: the text range that is replaced, its replacement, and the selection afterwards.
/// All offsets are in bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMove {
    pub replace: Range<usize>,
    pub text: String,
    pub selection: Range<usize>,
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
