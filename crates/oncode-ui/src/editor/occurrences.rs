//! Finding occurrences for the multi-cursor commands (pure, no GPUI): Add Selection to Next Find Match
//! (Ctrl+D) and Select All Occurrences (Ctrl+Shift+L).

use std::ops::Range;

/// What is searched for. Started from an empty selection (the word under the cursor), only whole words
/// match; started from a selection, the selected text matches anywhere. Both are case-sensitive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Needle {
    pub text: String,
    pub whole_word: bool,
}

/// The state of a run of Ctrl+D presses in one editor: what is searched for, the first selection (which
/// the editor keeps as its active selection), and the last occurrence added. A press whose active
/// selection is no longer `first` (the user clicked or typed) starts a new run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tracker {
    pub needle: Needle,
    pub first: Range<usize>,
    pub last: Range<usize>,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The word around `offset` (letters, digits, and `_`), or `None` if there is no word character on
/// either side of it.
pub fn word_at(text: &str, offset: usize) -> Option<Range<usize>> {
    let offset = offset.min(text.len());
    let start = text[..offset].char_indices().rev().take_while(|(_, c)| is_word_char(*c)).last().map_or(offset, |(i, _)| i);
    let end = text[offset..].char_indices().take_while(|(_, c)| is_word_char(*c)).last().map_or(offset, |(i, c)| offset + i + c.len_utf8());
    (start < end).then_some(start..end)
}

fn whole_word_at(text: &str, range: &Range<usize>) -> bool {
    let before = text[..range.start].chars().next_back().is_none_or(|c| !is_word_char(c));
    let after = text[range.end..].chars().next().is_none_or(|c| !is_word_char(c));
    before && after
}

/// Occurrences of `needle` starting in `text[from..to]`, in order.
fn matches_in<'a>(text: &'a str, needle: &'a Needle, from: usize, to: usize) -> impl Iterator<Item = Range<usize>> + 'a {
    let length = needle.text.len();
    text[from..].match_indices(needle.text.as_str()).map(move |(i, _)| from + i..from + i + length).take_while(move |range| range.start < to).filter(move |range| !needle.whole_word || whole_word_at(text, range))
}

/// The next occurrence after `after`, wrapping around to the start of the text; `None` when there is no
/// other occurrence (wrapping reaches `stop`, the start of the run's first selection).
pub fn find_next(text: &str, needle: &Needle, after: usize, stop: usize) -> Option<Range<usize>> {
    if needle.text.is_empty() {
        return None;
    }
    let after = after.min(text.len());
    if after < stop {
        // Already wrapped around: only what is left before the first selection.
        return matches_in(text, needle, after, stop).next();
    }
    matches_in(text, needle, after, text.len()).next().or_else(|| matches_in(text, needle, 0, stop).next())
}

/// Every occurrence, at most `limit`.
pub fn find_all(text: &str, needle: &Needle, limit: usize) -> Vec<Range<usize>> {
    if needle.text.is_empty() {
        return Vec::new();
    }
    matches_in(text, needle, 0, text.len()).take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str) -> Needle {
        Needle { text: text.into(), whole_word: true }
    }

    #[test]
    fn finds_the_word_around_an_offset() {
        let text = "let foo_bar = föö;";
        assert_eq!(word_at(text, 6), Some(4..11));
        assert_eq!(word_at(text, 4), Some(4..11), "at the start of the word");
        assert_eq!(word_at(text, 11), Some(4..11), "at the end of the word");
        assert_eq!(&text[word_at(text, 15).unwrap()], "föö");
        assert_eq!(word_at("a = b", 2), None);
    }

    #[test]
    fn next_occurrence_wraps_and_stops_at_the_first_selection() {
        let text = "foo foobar foo\nfoo";
        let needle = word("foo");
        // First selection 0..3: the next whole word is at 11, then 15, then none (wrapping reaches 0).
        assert_eq!(find_next(text, &needle, 3, 0), Some(11..14));
        assert_eq!(find_next(text, &needle, 14, 0), Some(15..18));
        assert_eq!(find_next(text, &needle, 18, 0), None);
        // Starting in the middle wraps to the beginning, then stops before the first selection.
        assert_eq!(find_next(text, &needle, 18, 11), Some(0..3));
        assert_eq!(find_next(text, &needle, 3, 11), None);
        // Not whole-word: "foo" inside "foobar" matches too.
        let any = Needle { text: "foo".into(), whole_word: false };
        assert_eq!(find_next(text, &any, 3, 0), Some(4..7));
    }

    #[test]
    fn finds_all_occurrences_up_to_a_limit() {
        let text = "a.b a_b a b";
        assert_eq!(find_all(text, &word("a"), 10), [0..1, 8..9]);
        assert_eq!(find_all(text, &word("a"), 1), [0..1]);
        assert_eq!(find_all(text, &Needle { text: "Case".into(), whole_word: false }, 10), Vec::<Range<usize>>::new());
        assert!(find_all(text, &word(""), 10).is_empty());
    }
}
