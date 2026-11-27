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
/// `current` and `incoming` are the whole files shown beside the result. The merged parts between
/// conflicts can hold changes from the other side (or edits made in the working tree), so a conflict's
/// place in those files is found by comparing each file with the result built from its own side.
pub fn build_initial_result(segments: &[MergeSegment], current: &str, incoming: &str) -> InitialResult {
    let mut result = InitialResult::default();
    // The result with every conflict taken from one side, and the place of each conflict in it.
    let (mut ours, mut theirs) = (String::new(), String::new());
    let (mut ours_spans, mut theirs_spans) = (Vec::new(), Vec::new());
    for segment in segments {
        match segment {
            MergeSegment::Resolved { text } => {
                let part = normalize(text);
                ours.push_str(&part);
                theirs.push_str(&part);
                result.text.push_str(&part);
            }
            MergeSegment::Conflict { current, base, incoming } => {
                let (current, incoming) = (normalize(current), normalize(incoming));
                ours_spans.push(ours.len()..ours.len() + current.len());
                ours.push_str(&current);
                theirs_spans.push(theirs.len()..theirs.len() + incoming.len());
                theirs.push_str(&incoming);
                let from = result.text.len();
                result.text.push_str(&current);
                result.conflicts.push(ConflictRange { index: result.conflicts.len(), from, to: from + current.len(), resolved: false, current, incoming, base: normalize(base) });
            }
        }
    }
    result.current_ranges = map_spans(&ours, &normalize(current), &ours_spans);
    result.incoming_ranges = map_spans(&theirs, &normalize(incoming), &theirs_spans);
    result
}

/// Moves byte ranges of `from` to the matching places in `to`, through a line diff of the two texts.
/// Text on unchanged lines maps exactly; a position on changed lines goes to the start (range starts)
/// or the end (range ends) of the lines that replaced them.
fn map_spans(from: &str, to: &str, spans: &[Range<usize>]) -> Vec<Range<usize>> {
    if from == to {
        return spans.to_vec();
    }
    let diff = similar::TextDiff::configure().timeout(std::time::Duration::from_millis(500)).diff_lines(from, to);
    let line_starts = |text: &str| -> Vec<usize> { std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect() };
    let (old_starts, new_starts) = (line_starts(from), line_starts(to));
    let new_offset = |line: usize| new_starts.get(line).copied().unwrap_or(to.len()).min(to.len());
    let ops: Vec<_> = diff.ops().iter().map(|op| op.as_tag_tuple()).collect();
    let map = |pos: usize, end: bool| -> usize {
        if pos >= from.len() {
            return to.len();
        }
        let line = old_starts.partition_point(|start| *start <= pos) - 1;
        let column = pos - old_starts[line];
        let Some((tag, old, new)) = ops.iter().find(|(_, old, _)| old.contains(&line)) else { return to.len() };
        if *tag == similar::DiffTag::Equal {
            return (new_offset(new.start + line - old.start) + column).min(to.len());
        }
        // Inside changed lines; a position at the start of the first of them is just before the change.
        if !end || (line == old.start && column == 0) {
            new_offset(new.start)
        } else {
            new_offset(new.end)
        }
    };
    spans
        .iter()
        .map(|span| {
            let start = map(span.start, false);
            if span.is_empty() {
                return start..start;
            }
            start..map(span.end, true).max(start)
        })
        .collect()
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
    /// ends at `new_end`. An edit of a conflict's content means the user has already decided it.
    /// `line_break_before(pos)` tells whether the document before the edit has a line break just
    /// before `pos`: typing at the end of a conflict that ends with a line break is typing on the
    /// next line, which leaves the conflict alone.
    pub fn apply_edit(&mut self, start: usize, old_end: usize, new_end: usize, line_break_before: impl Fn(usize) -> bool) {
        for conflict in &mut self.conflicts {
            let insertion = start == old_end;
            let empty = conflict.from == conflict.to;
            // Text typed at the end of the conflict's last line, or into an empty conflict, becomes part of it.
            let at_end = insertion && start == conflict.to && (empty || !line_break_before(start));
            let touched = if insertion { (conflict.from <= start && start < conflict.to) || at_end } else { start < conflict.to && old_end > conflict.from };
            let from = if at_end && empty { start } else { map_position(conflict.from, true, start, old_end, new_end) };
            let to = if at_end { new_end } else { map_position(conflict.to, false, start, old_end, new_end) }.max(from);
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
            let initial = build_initial_result(&segments(), CURRENT_FILE, INCOMING_FILE);
            (Self { text: initial.text.clone(), state: ConflictState::new(initial.conflicts.clone()) }, initial)
        }

        fn choose(&mut self, index: usize, choice: ConflictChoice) {
            let (range, insert) = self.state.choose(index, choice).unwrap();
            self.text.replace_range(range, &insert);
        }

        fn edit(&mut self, range: Range<usize>, insert: &str) {
            let text = self.text.clone();
            self.state.apply_edit(range.start, range.end, range.start + insert.len(), |pos| pos > 0 && text.as_bytes()[pos - 1] == b'\n');
            self.text.replace_range(range, insert);
        }

        fn slice(&self, index: usize) -> &str {
            let conflict = &self.state.conflicts[index];
            &self.text[conflict.from..conflict.to]
        }
    }

    const CURRENT_FILE: &str = "timeout = 30\nretries = 4\nlog = \"debug\"\na\nb\n";
    const INCOMING_FILE: &str = "timeout = 60\nretries = 5\nlog = \"info\"\n";

    #[test]
    fn builds_the_initial_document_with_the_current_side_and_records_positions() {
        let initial = build_initial_result(&segments(), CURRENT_FILE, INCOMING_FILE);
        assert_eq!(initial.text, "timeout = 60\nretries = 4\nlog = \"debug\"\na\nb\n");
        let slices: Vec<&str> = initial.conflicts.iter().map(|c| &initial.text[c.from..c.to]).collect();
        assert_eq!(slices, ["retries = 4\n", "a\nb\n"]);
        // The merged parts hold changes from the other side (`timeout = 60` came from incoming, `log` from
        // current): the ranges are still found in each side's own file.
        let current: Vec<&str> = initial.current_ranges.iter().map(|r| &CURRENT_FILE[r.clone()]).collect();
        assert_eq!(current, ["retries = 4\n", "a\nb\n"]);
        assert_eq!(initial.current_ranges[0].start, "timeout = 30\n".len());
        let incoming: Vec<&str> = initial.incoming_ranges.iter().map(|r| &INCOMING_FILE[r.clone()]).collect();
        assert_eq!(incoming, ["retries = 5\n", ""]);
        assert_eq!(initial.incoming_ranges[0].start, "timeout = 60\n".len());
        assert_eq!(initial.incoming_ranges[1].start, INCOMING_FILE.len());
    }

    /// Lines added before a conflict on the other side shift the conflict in the result, not in this side.
    #[test]
    fn side_ranges_follow_lines_the_other_side_added() {
        let segments = vec![
            MergeSegment::Resolved { text: "one\ntwo\nthree\n".into() },
            MergeSegment::Conflict { current: "ours\n".into(), base: "x\n".into(), incoming: "theirs\n".into() },
            MergeSegment::Resolved { text: "end\n".into() },
        ];
        let current = "one\nours\nend\n";
        let incoming = "one\ntwo\nthree\ntheirs\nend\n";
        let initial = build_initial_result(&segments, current, incoming);
        assert_eq!(&current[initial.current_ranges[0].clone()], "ours\n");
        assert_eq!(&incoming[initial.incoming_ranges[0].clone()], "theirs\n");
    }

    #[test]
    fn normalizes_crlf_so_positions_match_the_editor_document() {
        let initial = build_initial_result(
            &[MergeSegment::Resolved { text: "x\r\n".into() }, MergeSegment::Conflict { current: "ours\r\n".into(), base: String::new(), incoming: "theirs\r\n".into() }],
            "x\r\nours\r\n",
            "x\r\ntheirs\r\n",
        );
        assert_eq!(initial.text, "x\nours\n");
        assert_eq!((initial.conflicts[0].from, initial.conflicts[0].to), (2, 7));
        assert_eq!((initial.current_ranges[0].clone(), initial.incoming_ranges[0].clone()), (2..7, 2..9));
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

    /// A conflict ends with its line break, so its end is the start of the next line: typing there is
    /// typing on that line, not deciding the conflict.
    #[test]
    fn typing_on_the_line_after_a_conflict_leaves_it_unresolved() {
        let (mut doc, _) = Doc::new();
        let first = doc.state.conflicts[0].clone();
        doc.edit(first.to..first.to, "# note ");
        assert!(!doc.state.conflicts[0].resolved);
        assert_eq!(doc.slice(0), "retries = 4\n");
        assert!(doc.text.contains("retries = 4\n# note log"));
        // At the end of its last line, before the line break, the text is part of the conflict.
        let first = doc.state.conflicts[0].clone();
        doc.edit(first.to - 1..first.to - 1, "0");
        assert!(doc.state.conflicts[0].resolved);
        assert_eq!(doc.slice(0), "retries = 40\n");
        // Typing into an empty conflict (a side that deleted its lines) decides it.
        doc.choose(1, ConflictChoice::Incoming);
        let second = doc.state.conflicts[1].clone();
        doc.state.conflicts[1].resolved = false;
        doc.edit(second.from..second.from, "c\n");
        assert!(doc.state.conflicts[1].resolved);
        assert_eq!(doc.slice(1), "c\n");
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
