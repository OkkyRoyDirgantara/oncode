//! Conflict editor model: the range of each conflict in the result document, the side choice, and how
//! ranges shift as the result document is edited. Pure state; the view lives in `conflict_tab.rs`.
//!
//! All positions are byte offsets into text whose line endings have been normalized to `\n`.

use std::ops::Range;

use oncode_git::MergeSegment;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictRange {
    pub index: usize,
    pub from: usize,
    pub to: usize,
    pub resolved: bool,
    pub current: String,
    pub incoming: String,
    pub base: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictChoice {
    Current,
    Incoming,
    Both,
    Base,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InitialResult {
    /// Initial content of the Result panel: the already-merged parts, with the current side for each conflict.
    pub text: String,
    pub conflicts: Vec<ConflictRange>,
    /// Range of each conflict in the current-side and incoming-side files, for highlighting.
    pub current_ranges: Vec<Range<usize>>,
    pub incoming_ranges: Vec<Range<usize>>,
}

/// The editor uses `\n` as the line separator; lengths are computed after normalization.
pub fn normalize(text: &str) -> String {
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text.to_string()
    }
}

/// Builds the initial result document and the conflict positions from three-way merge segments.
pub fn build_initial_result(segments: &[MergeSegment]) -> InitialResult {
    let mut result = InitialResult::default();
    let (mut current_offset, mut incoming_offset) = (0usize, 0usize);
    for segment in segments {
        match segment {
            MergeSegment::Resolved { text } => {
                let part = normalize(text);
                current_offset += part.len();
                incoming_offset += part.len();
                result.text.push_str(&part);
            }
            MergeSegment::Conflict { current, base, incoming } => {
                let (current, incoming) = (normalize(current), normalize(incoming));
                result.current_ranges.push(current_offset..current_offset + current.len());
                result.incoming_ranges.push(incoming_offset..incoming_offset + incoming.len());
                current_offset += current.len();
                incoming_offset += incoming.len();
                let from = result.text.len();
                result.text.push_str(&current);
                result.conflicts.push(ConflictRange { index: result.conflicts.len(), from, to: from + current.len(), resolved: false, current, incoming, base: normalize(base) });
            }
        }
    }
    result
}

pub fn choice_text(conflict: &ConflictRange, choice: ConflictChoice) -> String {
    match choice {
        ConflictChoice::Current => conflict.current.clone(),
        ConflictChoice::Incoming => conflict.incoming.clone(),
        ConflictChoice::Base => conflict.base.clone(),
        ConflictChoice::Both => {
            let first = &conflict.current;
            // Make sure the two sides don't merge into a single line.
            if !first.is_empty() && !first.ends_with('\n') && !conflict.incoming.is_empty() {
                format!("{first}\n{}", conflict.incoming)
            } else {
                format!("{first}{}", conflict.incoming)
            }
        }
    }
}

/// Position `pos` after `[start, old_end)` is replaced by text of length `new_end - start`.
/// `after`: a position exactly at the insertion point moves to after the inserted text.
fn map_position(pos: usize, after: bool, start: usize, old_end: usize, new_end: usize) -> usize {
    if pos < start {
        pos
    } else if pos > old_end {
        pos - old_end + new_end
    } else if old_end > pos {
        if pos == start || !after {
            start
        } else {
            new_end
        }
    } else if old_end == start && !after {
        start
    } else {
        new_end
    }
}

/// The smallest edit that turns `before` into `after`: `(start, old_end, new_end)`.
/// Used when the editor only reports "the whole document was replaced" (e.g. after IME composition).
pub fn minimal_edit(before: &str, after: &str) -> (usize, usize, usize) {
    let (old, new) = (before.as_bytes(), after.as_bytes());
    let mut prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    while !before.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let limit = old.len().min(new.len()) - prefix;
    let mut suffix = old.iter().rev().zip(new.iter().rev()).take(limit).take_while(|(a, b)| a == b).count();
    while !before.is_char_boundary(old.len() - suffix) {
        suffix -= 1;
    }
    (prefix, old.len() - suffix, new.len() - suffix)
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConflictState {
    pub conflicts: Vec<ConflictRange>,
    /// The conflict currently highlighted and targeted by the choice buttons.
    pub active: usize,
}

impl ConflictState {
    pub fn new(conflicts: Vec<ConflictRange>) -> Self {
        Self { conflicts, active: 0 }
    }

    pub fn unresolved(&self) -> usize {
        self.conflicts.iter().filter(|c| !c.resolved).count()
    }

    /// Shifts the ranges after a manual edit: `[start, old_end)` is replaced by text that
    /// ends at `new_end`. An edit inside (or touching) a conflict means the user has
    /// already decided its content.
    pub fn apply_edit(&mut self, start: usize, old_end: usize, new_end: usize) {
        for conflict in &mut self.conflicts {
            let touched = start <= conflict.to && old_end >= conflict.from;
            let from = map_position(conflict.from, true, start, old_end, new_end);
            let to = map_position(conflict.to, false, start, old_end, new_end).max(from);
            conflict.from = from;
            conflict.to = to;
            conflict.resolved = conflict.resolved || touched;
        }
    }

    /// Replaces the content of a conflict with the chosen side and marks it resolved.
    /// Returns the old range and its replacement text, to be applied to the result document.
    pub fn choose(&mut self, index: usize, choice: ConflictChoice) -> Option<(Range<usize>, String)> {
        let conflict = self.conflicts.iter().find(|c| c.index == index)?.clone();
        let insert = choice_text(&conflict, choice);
        let (start, old_end, new_end) = (conflict.from, conflict.to, conflict.from + insert.len());
        for other in &mut self.conflicts {
            if other.index == index {
                other.to = new_end;
                other.resolved = true;
            } else {
                let from = map_position(other.from, true, start, old_end, new_end);
                other.to = map_position(other.to, false, start, old_end, new_end).max(from);
                other.from = from;
            }
        }
        self.active = index;
        Some((start..old_end, insert))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segments() -> Vec<MergeSegment> {
        vec![
            MergeSegment::Resolved { text: "timeout = 60\n".into() },
            MergeSegment::Conflict { current: "retries = 4\n".into(), base: "retries = 3\n".into(), incoming: "retries = 5\n".into() },
            MergeSegment::Resolved { text: "log = \"debug\"\n".into() },
            MergeSegment::Conflict { current: "a\nb\n".into(), base: String::new(), incoming: String::new() },
        ]
    }

    /// Mock result document: text and state updated together, as in the view.
    struct Doc {
        text: String,
        state: ConflictState,
    }

    impl Doc {
        fn new() -> (Self, InitialResult) {
            let initial = build_initial_result(&segments());
            (Self { text: initial.text.clone(), state: ConflictState::new(initial.conflicts.clone()) }, initial)
        }

        fn choose(&mut self, index: usize, choice: ConflictChoice) {
            let (range, insert) = self.state.choose(index, choice).unwrap();
            self.text.replace_range(range, &insert);
        }

        fn edit(&mut self, range: Range<usize>, insert: &str) {
            self.state.apply_edit(range.start, range.end, range.start + insert.len());
            self.text.replace_range(range, insert);
        }

        fn slice(&self, index: usize) -> &str {
            let conflict = &self.state.conflicts[index];
            &self.text[conflict.from..conflict.to]
        }
    }

    #[test]
    fn builds_the_initial_document_with_the_current_side_and_records_positions() {
        let initial = build_initial_result(&segments());
        assert_eq!(initial.text, "timeout = 60\nretries = 4\nlog = \"debug\"\na\nb\n");
        let slices: Vec<&str> = initial.conflicts.iter().map(|c| &initial.text[c.from..c.to]).collect();
        assert_eq!(slices, ["retries = 4\n", "a\nb\n"]);
        // The incoming side is shorter in the second conflict; its range is computed on the incoming file.
        let incoming_file = "timeout = 60\nretries = 5\nlog = \"debug\"\n";
        let incoming: Vec<&str> = initial.incoming_ranges.iter().map(|r| &incoming_file[r.clone()]).collect();
        assert_eq!(incoming, ["retries = 5\n", ""]);
        let current_file = "timeout = 60\nretries = 4\nlog = \"debug\"\na\nb\n";
        let current: Vec<&str> = initial.current_ranges.iter().map(|r| &current_file[r.clone()]).collect();
        assert_eq!(current, ["retries = 4\n", "a\nb\n"]);
    }

    #[test]
    fn normalizes_crlf_so_positions_match_the_editor_document() {
        let initial = build_initial_result(&[
            MergeSegment::Resolved { text: "x\r\n".into() },
            MergeSegment::Conflict { current: "ours\r\n".into(), base: String::new(), incoming: "theirs\r\n".into() },
        ]);
        assert_eq!(initial.text, "x\nours\n");
        assert_eq!((initial.conflicts[0].from, initial.conflicts[0].to), (2, 7));
    }

    #[test]
    fn choosing_a_side_replaces_the_content_marks_it_resolved_and_shifts_later_conflicts() {
        let (mut doc, _) = Doc::new();
        assert_eq!(doc.state.unresolved(), 2);
        doc.choose(0, ConflictChoice::Incoming);
        assert_eq!(doc.text, "timeout = 60\nretries = 5\nlog = \"debug\"\na\nb\n");
        doc.choose(0, ConflictChoice::Both);
        assert_eq!(doc.text, "timeout = 60\nretries = 4\nretries = 5\nlog = \"debug\"\na\nb\n");
        assert!(doc.state.conflicts[0].resolved);
        assert_eq!(doc.slice(1), "a\nb\n");
        assert_eq!(doc.state.unresolved(), 1);

        // Empty incoming side (lines deleted): the range becomes empty but can still be re-chosen.
        doc.choose(1, ConflictChoice::Incoming);
        assert_eq!(doc.text, "timeout = 60\nretries = 4\nretries = 5\nlog = \"debug\"\n");
        assert_eq!(doc.state.active, 1);
        doc.choose(1, ConflictChoice::Current);
        assert!(doc.text.ends_with("log = \"debug\"\na\nb\n"));
        assert_eq!(doc.state.unresolved(), 0);
    }

    #[test]
    fn manual_edits_inside_a_conflict_resolve_it_and_edits_outside_do_not() {
        let (mut doc, initial) = Doc::new();
        doc.edit(0..0, "# comment\n");
        assert_eq!(doc.state.unresolved(), 2);
        let shifted = doc.state.conflicts[0].clone();
        assert_eq!(shifted.from, initial.conflicts[0].from + "# comment\n".len());
        assert_eq!(doc.slice(0), "retries = 4\n");

        doc.edit(shifted.from + 10..shifted.from + 11, "9");
        assert!(doc.state.conflicts[0].resolved);
        assert_eq!(doc.slice(0), "retries = 9\n");
        assert_eq!(doc.slice(1), "a\nb\n");
        assert_eq!(doc.state.unresolved(), 1);
    }

    #[test]
    fn deleting_across_a_conflict_boundary_clamps_the_range() {
        let (mut doc, initial) = Doc::new();
        let first = initial.conflicts[0].clone();
        // Delete from the middle of the first conflict through to the text after it.
        doc.edit(first.from + 4..first.to + 3, "");
        assert_eq!(doc.slice(0), "retr");
        assert!(doc.state.conflicts[0].resolved);
        assert_eq!(doc.slice(1), "a\nb\n");
        // Deleting the whole second conflict leaves an empty range at its position.
        let second = doc.state.conflicts[1].clone();
        doc.edit(second.from - 1..second.to, "");
        assert_eq!(doc.slice(1), "");
        assert_eq!(doc.state.conflicts[1].from, doc.text.len());
    }

    #[test]
    fn minimal_edit_finds_the_changed_span() {
        assert_eq!(minimal_edit("abc def", "abc XY def"), (4, 4, 7));
        assert_eq!(minimal_edit("abc def", "abc"), (3, 7, 3));
        assert_eq!(minimal_edit("aaa", "aaaa"), (3, 3, 4));
        assert_eq!(minimal_edit("same", "same"), (4, 4, 4));
        // Doesn't cut in the middle of a multibyte character.
        assert_eq!(minimal_edit("é", "è"), (0, 2, 2));
    }

    #[test]
    fn both_never_joins_the_two_sides_on_one_line() {
        let conflict = ConflictRange { index: 0, from: 0, to: 0, resolved: false, current: "no newline".into(), incoming: "second\n".into(), base: String::new() };
        assert_eq!(choice_text(&conflict, ConflictChoice::Both), "no newline\nsecond\n");
        assert_eq!(choice_text(&ConflictRange { current: "a\n".into(), ..conflict.clone() }, ConflictChoice::Both), "a\nsecond\n");
        assert_eq!(choice_text(&ConflictRange { base: "origin\n".into(), ..conflict }, ConflictChoice::Base), "origin\n");
    }
}
