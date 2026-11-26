//! LSP positions (`line`, `column`) relative to text. The column unit depends on the encoding
//! agreed at `initialize`; the default is UTF-16, while the editor works with byte offsets.
//!
//! Lines are separated by `\n`; a `\r` at the end of a line does not count as line content.

use lsp_types::{Position, PositionEncodingKind, Range, TextDocumentContentChangeEvent, TextEdit};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PositionEncoding {
    Utf8,
    #[default]
    Utf16,
    Utf32,
}

impl PositionEncoding {
    /// The encoding chosen by the server; if unspecified it means UTF-16.
    pub fn from_kind(kind: Option<&PositionEncodingKind>) -> Self {
        match kind.map(PositionEncodingKind::as_str) {
            Some("utf-8") => Self::Utf8,
            Some("utf-32") => Self::Utf32,
            _ => Self::Utf16,
        }
    }

    pub fn kind(self) -> PositionEncodingKind {
        match self {
            Self::Utf8 => PositionEncodingKind::UTF8,
            Self::Utf16 => PositionEncodingKind::UTF16,
            Self::Utf32 => PositionEncodingKind::UTF32,
        }
    }

    fn width(self, ch: char) -> usize {
        match self {
            Self::Utf8 => ch.len_utf8(),
            Self::Utf16 => ch.len_utf16(),
            Self::Utf32 => 1,
        }
    }

    /// The column for byte offset `byte` within `line`. An offset in the middle of a character is rounded to
    /// the start of that character; an offset outside the line is clamped to the end of the line.
    pub fn column_of(self, line: &str, byte: usize) -> u32 {
        let line = trim_line_end(line);
        let mut column = 0usize;
        for (index, ch) in line.char_indices() {
            if index + ch.len_utf8() > byte {
                break;
            }
            column += self.width(ch);
        }
        column as u32
    }

    /// The byte offset for `column` within `line`. A column outside the line is clamped to the end of the line;
    /// a column in the middle of a character (e.g. between a surrogate pair) is rounded to the start of that character.
    pub fn byte_of(self, line: &str, column: u32) -> usize {
        let line = trim_line_end(line);
        let target = column as usize;
        let mut seen = 0usize;
        for (index, ch) in line.char_indices() {
            if seen + self.width(ch) > target {
                return index;
            }
            seen += self.width(ch);
        }
        line.len()
    }
}

fn trim_line_end(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

/// The byte range of line number `line` (without its line separator); `None` if the line does not exist.
fn line_span(text: &str, line: u32) -> Option<(usize, usize)> {
    let mut start = 0usize;
    for _ in 0..line {
        start += text[start..].find('\n')? + 1;
    }
    let end = text[start..].find('\n').map_or(text.len(), |offset| start + offset);
    Some((start, end))
}

/// The byte offset for a position. A line outside the document is clamped to the end of the document.
pub fn offset_at(text: &str, position: Position, encoding: PositionEncoding) -> usize {
    match line_span(text, position.line) {
        Some((start, end)) => start + encoding.byte_of(&text[start..end], position.character),
        None => text.len(),
    }
}

/// The position for a byte offset (clamped to the document length).
pub fn position_at(text: &str, offset: usize, encoding: PositionEncoding) -> Position {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &text[..offset];
    let line = before.bytes().filter(|byte| *byte == b'\n').count() as u32;
    let start = before.rfind('\n').map_or(0, |index| index + 1);
    let end = text[start..].find('\n').map_or(text.len(), |index| start + index);
    Position { line, character: encoding.column_of(&text[start..end], offset - start) }
}

/// The byte range for an LSP range; reversed ends are swapped.
pub fn byte_range(text: &str, range: Range, encoding: PositionEncoding) -> std::ops::Range<usize> {
    let (start, end) = (offset_at(text, range.start, encoding), offset_at(text, range.end, encoding));
    start.min(end)..start.max(end)
}

/// Edits already mapped to byte offsets of the original document, sorted ascending by position.
/// Edits at the same position keep their original order (per the LSP spec).
pub fn resolve_text_edits(text: &str, edits: &[TextEdit], encoding: PositionEncoding) -> Vec<(std::ops::Range<usize>, String)> {
    let mut resolved: Vec<(std::ops::Range<usize>, String)> = edits.iter().map(|edit| (byte_range(text, edit.range, encoding), edit.new_text.clone())).collect();
    resolved.sort_by_key(|(range, _)| range.start);
    resolved
}

/// Applies a set of `TextEdit`s (all of their positions refer to the original `text`) and returns
/// the result. An edit that overlaps a previous edit is skipped.
pub fn apply_text_edits(text: &str, edits: &[TextEdit], encoding: PositionEncoding) -> String {
    let resolved = resolve_text_edits(text, edits, encoding);
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    for (range, new_text) in resolved {
        if range.start < cursor {
            continue;
        }
        out.push_str(&text[cursor..range.start]);
        out.push_str(&new_text);
        cursor = range.end;
    }
    out.push_str(&text[cursor..]);
    out
}

/// Applies the contents of `didChange` in order: each change refers to the document after
/// the previous change; a change without a range replaces the entire contents.
pub fn apply_content_changes(text: &mut String, changes: &[TextDocumentContentChangeEvent], encoding: PositionEncoding) {
    for change in changes {
        match change.range {
            Some(range) => {
                let range = byte_range(text, range, encoding);
                text.replace_range(range, &change.text);
            }
            None => *text = change.text.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    fn edit(start: (u32, u32), end: (u32, u32), text: &str) -> TextEdit {
        TextEdit { range: Range { start: pos(start.0, start.1), end: pos(end.0, end.1) }, new_text: text.into() }
    }

    #[test]
    fn columns_count_code_units_of_the_negotiated_encoding() {
        // "a" (1 byte), "é" (2 bytes, 1 UTF-16 unit), "😀" (4 bytes, 2 UTF-16 units), "z".
        let line = "aé😀z";
        assert_eq!(PositionEncoding::Utf8.column_of(line, 7), 7);
        assert_eq!(PositionEncoding::Utf16.column_of(line, 7), 4);
        assert_eq!(PositionEncoding::Utf32.column_of(line, 7), 3);
        assert_eq!(PositionEncoding::Utf16.column_of(line, 3), 2);
        // In the middle of a character: rounded to the start of that character; outside the line: clamped.
        assert_eq!(PositionEncoding::Utf16.column_of(line, 5), 2);
        assert_eq!(PositionEncoding::Utf16.column_of(line, 99), 5);

        assert_eq!(PositionEncoding::Utf16.byte_of(line, 2), 3);
        assert_eq!(PositionEncoding::Utf16.byte_of(line, 4), 7);
        // Column 3 falls between a surrogate pair.
        assert_eq!(PositionEncoding::Utf16.byte_of(line, 3), 3);
        assert_eq!(PositionEncoding::Utf32.byte_of(line, 3), 7);
        assert_eq!(PositionEncoding::Utf8.byte_of(line, 3), 3);
        assert_eq!(PositionEncoding::Utf16.byte_of(line, 99), line.len());
    }

    #[test]
    fn line_terminators_are_not_part_of_the_line() {
        assert_eq!(PositionEncoding::Utf16.byte_of("ab\r\n", 9), 2);
        assert_eq!(PositionEncoding::Utf16.column_of("ab\r", 3), 2);
    }

    #[test]
    fn encoding_negotiation_defaults_to_utf16() {
        assert_eq!(PositionEncoding::from_kind(None), PositionEncoding::Utf16);
        assert_eq!(PositionEncoding::from_kind(Some(&PositionEncodingKind::UTF8)), PositionEncoding::Utf8);
        assert_eq!(PositionEncoding::from_kind(Some(&PositionEncodingKind::UTF32)), PositionEncoding::Utf32);
        assert_eq!(PositionEncoding::from_kind(Some(&PositionEncodingKind::new("odd"))), PositionEncoding::Utf16);
        assert_eq!(PositionEncoding::Utf32.kind(), PositionEncodingKind::UTF32);
    }

    #[test]
    fn offsets_and_positions_round_trip() {
        let text = "item\ntwo 😀 done\r\n\nfinal";
        let enc = PositionEncoding::Utf16;
        assert_eq!(offset_at(text, pos(0, 0), enc), 0);
        assert_eq!(offset_at(text, pos(1, 4), enc), 9);
        assert_eq!(offset_at(text, pos(1, 6), enc), 13);
        assert_eq!(&text[offset_at(text, pos(1, 7), enc)..][..4], "done");
        // A column past the end of the line is clamped before `\r\n`; a line outside the document goes to the end of the document.
        assert_eq!(offset_at(text, pos(1, 99), enc), 18);
        assert_eq!(offset_at(text, pos(2, 5), enc), 20);
        assert_eq!(offset_at(text, pos(9, 0), enc), text.len());

        for offset in [0, 4, 5, 9, 13, 18, 20, 21, text.len()] {
            assert_eq!(offset_at(text, position_at(text, offset, enc), enc), offset, "offset {offset}");
        }
        assert_eq!(position_at(text, 13, enc), pos(1, 6));
        assert_eq!(position_at(text, 13, PositionEncoding::Utf32), pos(1, 5));
        assert_eq!(position_at(text, text.len() + 10, enc), pos(3, 5));
    }

    #[test]
    fn text_edits_refer_to_the_original_document() {
        let text = "let a = 1;\nlet b = a;\n";
        let edits = [edit((1, 8), (1, 9), "value"), edit((0, 4), (0, 5), "value"), edit((0, 0), (0, 0), "// result\n")];
        assert_eq!(apply_text_edits(text, &edits, PositionEncoding::Utf16), "// result\nlet value = 1;\nlet b = value;\n");
    }

    #[test]
    fn inserts_at_the_same_position_keep_their_order_and_overlaps_are_skipped() {
        let text = "x";
        let edits = [edit((0, 0), (0, 0), "a"), edit((0, 0), (0, 0), "b"), edit((0, 0), (0, 1), "y")];
        assert_eq!(apply_text_edits(text, &edits, PositionEncoding::Utf16), "aby");
        let overlapping = [edit((0, 0), (0, 3), "AAA"), edit((0, 1), (0, 2), "b")];
        assert_eq!(apply_text_edits("abcd", &overlapping, PositionEncoding::Utf16), "AAAd");
    }

    #[test]
    fn utf16_ranges_respect_surrogate_pairs() {
        let text = "a😀b";
        assert_eq!(apply_text_edits(text, &[edit((0, 1), (0, 3), "-")], PositionEncoding::Utf16), "a-b");
        assert_eq!(apply_text_edits(text, &[edit((0, 1), (0, 2), "-")], PositionEncoding::Utf32), "a-b");
    }

    #[test]
    fn content_changes_apply_in_sequence() {
        let mut text = String::from("item\ntwo\n");
        let changes = [
            TextDocumentContentChangeEvent { range: Some(Range { start: pos(0, 4), end: pos(0, 4) }), range_length: None, text: "!".into() },
            // Refers to the document after the first change.
            TextDocumentContentChangeEvent { range: Some(Range { start: pos(0, 0), end: pos(0, 5) }), range_length: None, text: "ITEM".into() },
            TextDocumentContentChangeEvent { range: Some(Range { start: pos(1, 3), end: pos(2, 0) }), range_length: None, text: "".into() },
        ];
        apply_content_changes(&mut text, &changes, PositionEncoding::Utf16);
        assert_eq!(text, "ITEM\ntwo");
        apply_content_changes(&mut text, &[TextDocumentContentChangeEvent { range: None, range_length: None, text: "new".into() }], PositionEncoding::Utf16);
        assert_eq!(text, "new");
    }
}
