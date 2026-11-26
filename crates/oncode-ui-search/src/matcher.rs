//! Matcher for files currently open in the editor (and for the replacement text preview).
//!
//! Uses the same engine as the backend (`grep-regex`, built with the same options as
//! `oncode_workspace::search`), so results in the editor never differ from results on disk:
//! line by line, empty matches are skipped, whole word uses Unicode word boundaries, and
//! `$1`/`${name}`/`$$` in the replacement text follow the `grep-matcher` rules.
//!
//! Positions within a line are stored in bytes (what the editor uses); the backend's `LineMatch`
//! uses UTF-16 units, so the conversion helpers live here too.

use std::ops::Range;

use grep_matcher::{Captures as _, Matcher as _};
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use oncode_workspace::SearchQuery;

/// The part of the query that determines matching (without file filters and the result cap).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchOptions {
    pub pattern: String,
    pub is_regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
}

impl From<&SearchQuery> for MatchOptions {
    fn from(query: &SearchQuery) -> Self {
        Self { pattern: query.pattern.clone(), is_regex: query.is_regex, case_sensitive: query.case_sensitive, whole_word: query.whole_word }
    }
}

/// The identity of a match as reported by the backend: line starting at 1, column and length
/// in UTF-16 units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MatchKey {
    pub line: u32,
    pub column: u32,
    pub length: u32,
}

/// A single match within a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineHit {
    /// Start and end of the match within the line, in bytes.
    pub start: usize,
    pub end: usize,
    /// Byte range of each capture group; index 0 is the whole match. `None` if the group
    /// did not participate in the match.
    pub groups: Vec<Option<Range<usize>>>,
}

/// A per-line matcher for a query.
#[derive(Clone)]
pub struct LineMatcher {
    matcher: RegexMatcher,
}

impl LineMatcher {
    /// Builds the matcher; its error is the same message the backend sends.
    pub fn new(options: &MatchOptions) -> Result<Self, String> {
        if options.pattern.is_empty() {
            return Err("The search pattern is empty".into());
        }
        if !options.is_regex && options.pattern.contains(['\n', '\r']) {
            return Err("Multiline search is not supported yet".into());
        }
        // Must be exactly the same as `build_matcher` in `oncode-workspace/src/search.rs`.
        RegexMatcherBuilder::new()
            .case_insensitive(!options.case_sensitive)
            .word(options.whole_word)
            .fixed_strings(!options.is_regex)
            .multi_line(true)
            .crlf(true)
            .build(&options.pattern)
            .map(|matcher| Self { matcher })
            .map_err(|error| format!("Invalid search pattern: {error}"))
    }

    /// All non-empty matches on a single line (without the line ending), from left to right and
    /// non-overlapping.
    pub fn find(&self, line: &str) -> Vec<LineHit> {
        let mut hits = Vec::new();
        let Ok(mut captures) = self.matcher.new_captures() else { return hits };
        let _ = self.matcher.captures_iter(line.as_bytes(), &mut captures, |captures| {
            let Some(whole) = captures.get(0) else { return true };
            // Empty matches are not reported by the backend.
            if whole.is_empty() {
                return true;
            }
            let groups = (0..captures.len()).map(|index| captures.get(index).map(|m| m.start()..m.end())).collect();
            hits.push(LineHit { start: whole.start(), end: whole.end(), groups });
            true
        });
        hits
    }

    /// The index of a named capture group, if any.
    pub fn capture_index(&self, name: &str) -> Option<usize> {
        self.matcher.capture_index(name)
    }

    /// The replacement text for a single match. `interpolate` (regex mode) interprets `$1` etc.;
    /// in literal mode the replacement text is used as is.
    pub fn replacement_for(&self, line: &str, hit: &LineHit, replacement: &str, interpolate: bool) -> String {
        if !interpolate {
            return replacement.to_string();
        }
        expand_replacement(replacement, |index| hit.groups.get(index).cloned().flatten().and_then(|range| line.get(range)), |name| self.capture_index(name))
    }
}

/// Fills in `$1`, `$name`, `${name}`, and `$$` in the replacement text, following `interpolate` from
/// the `grep-matcher` crate: a name without braces consumes as many letters/digits/underscores as
/// possible (`$1a` is a group named "1a"), and a group that doesn't exist or didn't participate in
/// the match becomes empty text.
pub fn expand_replacement<'a>(template: &str, group: impl Fn(usize) -> Option<&'a str>, name_to_index: impl Fn(&str) -> Option<usize>) -> String {
    let bytes = template.as_bytes();
    let mut out = String::with_capacity(template.len());
    let mut at = 0;
    while let Some(found) = template[at..].find('$') {
        let dollar = at + found;
        out.push_str(&template[at..dollar]);
        if bytes.get(dollar + 1) == Some(&b'$') {
            out.push('$');
            at = dollar + 2;
            continue;
        }
        let braced = bytes.get(dollar + 1) == Some(&b'{');
        let name_start = dollar + if braced { 2 } else { 1 };
        let mut name_end = name_start;
        while bytes.get(name_end).is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_') {
            name_end += 1;
        }
        if name_end == name_start || (braced && bytes.get(name_end) != Some(&b'}')) {
            out.push('$');
            at = dollar + 1;
            continue;
        }
        let name = &template[name_start..name_end];
        at = name_end + usize::from(braced);
        let index = match name.parse::<u32>() {
            Ok(number) => Some(number as usize),
            Err(_) => name_to_index(name),
        };
        if let Some(value) = index.and_then(&group) {
            out.push_str(value);
        }
    }
    out.push_str(&template[at..]);
    out
}

/// Length of text in UTF-16 units.
pub fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// Byte offset for position `units` (UTF-16 units) within `text`; clamped to the end of the text
/// and never splits a character.
pub fn utf16_to_byte(text: &str, units: usize) -> usize {
    let mut seen = 0;
    for (index, ch) in text.char_indices() {
        if seen >= units {
            return index;
        }
        seen += ch.len_utf16();
    }
    text.len()
}

/// Which matches get replaced.
#[derive(Debug, Clone, Copy)]
pub enum Pick<'a> {
    All,
    /// All except those the user dismissed.
    Except(&'a [MatchKey]),
    /// Only this one match.
    Only(&'a MatchKey),
}

impl Pick<'_> {
    fn wants(&self, key: &MatchKey) -> bool {
        match self {
            Pick::All => true,
            Pick::Except(dismissed) => !dismissed.contains(key),
            Pick::Only(only) => *only == key,
        }
    }
}

/// A single replacement in a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    /// Line number, starting at 1.
    pub line: u32,
    /// Start and length of the match within the line, in UTF-16 units (like `LineMatch`).
    pub column: u32,
    pub length: u32,
    /// The byte range being replaced within the whole document.
    pub range: Range<usize>,
    pub insert: String,
}

/// Edits that replace matches in `text`, ordered from the start of the document. Lines are split on
/// `\n` (with a trailing `\r` dropped), the same as the backend.
pub fn compute_edits(matcher: &LineMatcher, text: &str, replacement: &str, interpolate: bool, pick: Pick<'_>) -> Vec<TextEdit> {
    let mut edits = Vec::new();
    let mut offset = 0;
    for (index, raw) in text.split('\n').enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let number = u32::try_from(index + 1).unwrap_or(u32::MAX);
        if let Pick::Only(only) = pick {
            if only.line != number {
                offset += raw.len() + 1;
                continue;
            }
        }
        // UTF-16 offsets are computed incrementally so a long line isn't rescanned for every match.
        let (mut seen_byte, mut seen_units) = (0, 0);
        for hit in matcher.find(line) {
            seen_units += utf16_len(&line[seen_byte..hit.start]);
            seen_byte = hit.start;
            let key = MatchKey { line: number, column: saturate(seen_units), length: saturate(utf16_len(&line[hit.start..hit.end])) };
            if !pick.wants(&key) {
                continue;
            }
            edits.push(TextEdit {
                line: number,
                column: key.column,
                length: key.length,
                range: offset + hit.start..offset + hit.end,
                insert: matcher.replacement_for(line, &hit, replacement, interpolate),
            });
        }
        offset += raw.len() + 1;
    }
    edits
}

/// Applies edits (ordered like the output of [`compute_edits`]) to `text`.
pub fn apply_edits(text: &str, edits: &[TextEdit]) -> String {
    let grow: usize = edits.iter().map(|edit| edit.insert.len()).sum();
    let mut out = String::with_capacity(text.len() + grow);
    let mut at = 0;
    for edit in edits {
        out.push_str(&text[at..edit.range.start]);
        out.push_str(&edit.insert);
        at = edit.range.end;
    }
    out.push_str(&text[at..]);
    out
}

fn saturate(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    //! The expectations here follow the backend tests (`crates/oncode-workspace/src/search.rs`) and
    //! the v0.2 matcher tests (`matcher.test.ts`): matches and replacement text must be the same.

    use super::*;

    fn literal(pattern: &str) -> MatchOptions {
        MatchOptions { pattern: pattern.into(), is_regex: false, case_sensitive: false, whole_word: false }
    }

    fn regex(pattern: &str) -> MatchOptions {
        MatchOptions { pattern: pattern.into(), is_regex: true, case_sensitive: true, whole_word: false }
    }

    fn sensitive(mut options: MatchOptions) -> MatchOptions {
        options.case_sensitive = true;
        options
    }

    fn insensitive(mut options: MatchOptions) -> MatchOptions {
        options.case_sensitive = false;
        options
    }

    fn word(mut options: MatchOptions) -> MatchOptions {
        options.whole_word = true;
        options
    }

    fn matcher_of(options: &MatchOptions) -> LineMatcher {
        LineMatcher::new(options).unwrap_or_else(|reason| panic!("cannot be built: {reason}"))
    }

    /// `[column, length]` of each match on a line, in UTF-16 units.
    fn spans(options: MatchOptions, line: &str) -> Vec<[usize; 2]> {
        matcher_of(&options).find(line).iter().map(|hit| [utf16_len(&line[..hit.start]), utf16_len(&line[hit.start..hit.end])]).collect()
    }

    fn columns(options: MatchOptions, line: &str) -> Vec<usize> {
        spans(options, line).into_iter().map(|[start, _]| start).collect()
    }

    /// File content after all matches are replaced, like `replace_in_files` in the backend.
    fn replaced(options: MatchOptions, replacement: &str, text: &str) -> String {
        let edits = compute_edits(&matcher_of(&options), text, replacement, options.is_regex, Pick::All);
        apply_edits(text, &edits)
    }

    #[test]
    fn literal_search_does_not_interpret_regex_syntax() {
        assert_eq!(spans(literal("a.b("), "let x = a.b(c);"), [[8, 4]]);
        assert_eq!(spans(literal("a.b("), "let y = aXb(c);"), [] as [[usize; 2]; 0]);
        assert_eq!(columns(literal("[x]{2}$^|\\d/"), "see [x]{2}$^|\\d/ here"), [4]);
    }

    #[test]
    fn literal_search_is_case_insensitive_unless_asked_otherwise() {
        for line in ["Hello", "hello", "HELLO"] {
            assert_eq!(columns(literal("hello"), line), [0]);
        }
        assert_eq!(columns(sensitive(literal("hello")), "Hello"), [] as [usize; 0]);
        assert_eq!(columns(sensitive(literal("hello")), "hello"), [0]);
        // Unicode case folding, not just ASCII.
        assert_eq!(columns(literal("ÉCOLE"), "une école"), [4]);
    }

    #[test]
    fn finds_several_non_overlapping_matches_on_one_line() {
        assert_eq!(columns(literal("aa"), "aaaaa aa"), [0, 2, 6]);
    }

    #[test]
    fn whole_word_matches_the_backend_on_its_own_fixture() {
        let line = "cat concat cat_x (cat) cats";
        assert_eq!(columns(literal("cat"), line).len(), 5);
        assert_eq!(columns(word(literal("cat")), line), [0, 18]);
        assert_eq!(columns(word(regex("ca.")), line), [0, 18]);
    }

    #[test]
    fn whole_word_uses_unicode_word_characters() {
        assert_eq!(columns(word(literal("cat")), "écat caté cat"), [10]);
        assert_eq!(columns(word(literal("cat")), "猫cat cat٣ cat"), [10]);
        // An astral character (two UTF-16 units) before the match.
        assert_eq!(columns(word(literal("cat")), "𝒳cat 😀cat"), [8]);
    }

    #[test]
    fn whole_word_only_constrains_the_two_ends_of_the_match() {
        assert_eq!(spans(word(literal("a.b")), "x a.b y a.bc"), [[2, 3]]);
        // A "half" boundary only looks at the neighbours: there must be no word character right outside
        // the match, whatever the first and last characters of the match itself are.
        assert_eq!(spans(word(literal("(cat)")), "x (cat) y"), [[2, 5]]);
        assert_eq!(spans(word(literal("(cat)")), "a(cat) (cat)b"), [] as [[usize; 2]; 0]);
    }

    #[test]
    fn whole_word_backtracks_into_the_pattern_to_end_on_a_word_boundary() {
        assert_eq!(spans(word(regex("\\w+?")), "abc de"), [[0, 3], [4, 2]]);
    }

    #[test]
    fn regex_anchors_per_line_and_never_reports_empty_matches() {
        assert_eq!(columns(regex("^foo"), "foo bar"), [0]);
        assert_eq!(columns(regex("^foo"), "bar foo"), [] as [usize; 0]);
        assert_eq!(columns(regex("foo$"), "bar foo"), [4]);
        assert_eq!(spans(regex("foo\\s*$"), "bar foo  "), [[4, 5]]);
        assert_eq!(spans(regex("a*"), "bab"), [[1, 1]]);
        assert_eq!(spans(regex("^"), "ccc"), [] as [[usize; 2]; 0]);
    }

    #[test]
    fn regex_honours_case_sensitivity() {
        assert_eq!(columns(regex("h[a-z]llo"), "Hello hello"), [6]);
        assert_eq!(columns(insensitive(regex("h[a-z]llo")), "Hello hello HELLO"), [0, 6, 12]);
    }

    #[test]
    fn regex_classes_have_their_unicode_meaning() {
        assert_eq!(spans(regex("\\w+"), "héllo wörld_1"), [[0, 5], [6, 7]]);
        assert_eq!(spans(regex("\\d+"), "a ٣٤ b 12"), [[2, 2], [7, 2]]);
        assert_eq!(spans(regex("\\bé\\w+"), "été école"), [[0, 3], [4, 5]]);
        assert_eq!(spans(regex("\\W+"), "é—é"), [[1, 1]]);
        assert_eq!(spans(regex("[\\w-]+"), "naïve-ish !"), [[0, 9]]);
        assert_eq!(spans(regex("a\\sb"), "a\u{a0}b a\u{3000}b"), [[0, 3], [4, 3]]);
        assert_eq!(spans(regex("\\S+"), "ab  cd"), [[0, 2], [4, 2]]);
        assert_eq!(spans(regex("\\Bb"), "ab b"), [[1, 1]]);
    }

    #[test]
    fn reports_utf16_offsets_and_treats_an_astral_character_as_one_character() {
        let line = "héllo 😀 wörld 😀😀!";
        assert_eq!(spans(literal("wörld"), line), [[9, 5]]);
        assert_eq!(spans(regex("😀."), line), [[6, 3], [15, 4]]);
        assert_eq!(spans(regex("."), "😀a"), [[0, 2], [2, 1]]);
        assert_eq!(spans(regex("[^a]"), "a😀"), [[1, 2]]);
        assert_eq!(spans(regex("\\w$"), "x𝒳"), [[1, 2]]);
        assert_eq!(spans(regex("[^a]$"), "a😀"), [[1, 2]]);
        assert_eq!(spans(word(regex("cat")), "cat𝒳 cat"), [[6, 3]]);
        // An empty match before an astral character doesn't split that character.
        assert_eq!(spans(regex("x*"), "😀x"), [[2, 1]]);
    }

    #[test]
    fn regex_syntax_of_the_backend_is_available_unchanged() {
        assert_eq!(spans(regex("(?P<year>\\d{4})-(?<month>\\d\\d)"), "on 2026-11-17"), [[3, 7]]);
        assert_eq!(spans(regex("\\x41\\x{1F600}\\u{42}\\U0001F600"), "A😀B😀"), [[0, 6]]);
        assert_eq!(spans(regex("\\pL+"), "ab1γδ"), [[0, 2], [3, 2]]);
        assert_eq!(spans(regex("\\p{Greek}+"), "abγδ"), [[2, 2]]);
        assert_eq!(spans(regex("\\P{L}+"), "a12b"), [[1, 2]]);
        assert_eq!(spans(regex("[]a]+"), "x]a]y"), [[1, 3]]);
        assert_eq!(spans(regex("a]}"), "a]} a]"), [[0, 3]]);
        assert_eq!(spans(regex("[\\-\\]\\\\^]+"), "a-]\\^b"), [[1, 4]]);
        assert_eq!(spans(regex("a{2,3}?"), "aaaa"), [[0, 2], [2, 2]]);
        assert_eq!(spans(regex("x/y"), "x/y"), [[0, 3]]);
        // Version 0.2 rejected the following constructs because they can't be translated to a JS `RegExp`;
        // here the engine is the same as the backend's, so all of them work in the editor too.
        assert_eq!(spans(regex("(?i)abc"), "xABC"), [[1, 3]]);
        assert_eq!(spans(regex("[[:alpha:]]+"), "ab1"), [[0, 2]]);
        assert_eq!(spans(regex("[a-z&&[^b]]+"), "abc"), [[0, 1], [2, 1]]);
    }

    #[test]
    fn invalid_patterns_are_refused_with_a_reason() {
        assert!(LineMatcher::new(&literal("")).is_err());
        assert!(LineMatcher::new(&literal("a\nb")).is_err());
        let reason = LineMatcher::new(&regex("(unclosed")).err().unwrap();
        assert!(reason.starts_with("Invalid search pattern:"), "{reason}");
        assert!(LineMatcher::new(&regex("a**")).is_err() || !spans(regex("a**"), "aa").is_empty());
        assert!(LineMatcher::new(&regex("(a)\\1")).is_err(), "backreferences are not supported by the regex crate");
    }

    /// Mock match: numbered groups 0..=3, then `val` (index 4) and `none` (index 5, did not participate in the match).
    fn expand(template: &str) -> String {
        let groups = [Some("whole"), Some("one"), None, Some("three"), Some("V"), None];
        expand_replacement(
            template,
            |index| groups.get(index).copied().flatten(),
            |name| match name {
                "val" => Some(4),
                "none" => Some(5),
                _ => None,
            },
        )
    }

    #[test]
    fn expands_numbered_named_and_braced_references() {
        assert_eq!(expand("$1-$3-$0"), "one-three-whole");
        assert_eq!(expand("${1}x ${val}y $val"), "onex Vy V");
    }

    #[test]
    fn turns_double_dollar_into_a_dollar_and_keeps_a_dollar_that_starts_no_reference() {
        assert_eq!(expand("$$1 costs $ 5 and $"), "$1 costs $ 5 and $");
        assert_eq!(expand("${1"), "${1");
        assert_eq!(expand("${a-b}"), "${a-b}");
        assert_eq!(expand("$-"), "$-");
    }

    #[test]
    fn takes_the_longest_name_so_dollar_1a_is_a_group_called_1a() {
        assert_eq!(expand("$1a"), "");
        assert_eq!(expand("${1}a"), "onea");
        assert_eq!(expand("$val_x"), "");
    }

    #[test]
    fn uses_an_empty_string_for_groups_that_are_missing_or_did_not_participate() {
        assert_eq!(expand("[$2][$9][$nope][$none][$constructor]"), "[][][][][]");
        // Non-ASCII text around a reference isn't truncated.
        assert_eq!(expand("é$1ü→$val"), "éoneü→V");
    }

    #[test]
    fn replaces_literally_without_interpreting_references_and_leaves_other_lines_alone() {
        let text = "foo(1) and Foo(2)\nnothing\nfoo(3)";
        assert_eq!(replaced(sensitive(literal("foo(")), "$1bar[", text), "$1bar[1) and Foo(2)\nnothing\n$1bar[3)");
    }

    #[test]
    fn replacing_follows_case_and_whole_word_options() {
        assert_eq!(replaced(word(literal("cat")), "dog", "Cat cats CAT"), "dog cats dog");
    }

    #[test]
    fn expands_capture_references_in_regex_mode() {
        let text = "let alpha = 1;\nlet beta = 22;\nconst gamma = 3;";
        assert_eq!(
            replaced(regex("let (\\w+) = (?P<val>\\d+);"), "const ${1}: u32 = $val; // $$", text),
            "const alpha: u32 = 1; // $\nconst beta: u32 = 22; // $\nconst gamma = 3;"
        );
    }

    #[test]
    fn applies_line_anchors_per_line_and_leaves_empty_lines_untouched() {
        assert_eq!(replaced(regex("\\s+$"), "", "a  \nb\n\nc "), "a\nb\n\nc");
        assert_eq!(replaced(regex("^foo"), "bar", "foo foo\nfoo"), "bar foo\nbar");
        // CRLF line endings are preserved and `$` matches before `\r\n`.
        assert_eq!(replaced(regex("foo$"), "bar", "a foo\r\nfoo\r\n"), "a bar\r\nbar\r\n");
    }

    #[test]
    fn never_matches_across_lines() {
        assert_eq!(replaced(regex("bar\\s+bar"), "X", "foo bar\nbar foo"), "foo bar\nbar foo");
    }

    #[test]
    fn keeps_offsets_right_when_several_replacements_on_a_line_change_its_length() {
        assert_eq!(replaced(literal("a"), "bbb", "a-a-a 😀 a"), "bbb-bbb-bbb 😀 bbb");
        assert_eq!(replaced(regex("(\\w)(\\w)"), "$2$1", "abcd 😀 éf"), "badc 😀 fé");
    }

    #[test]
    fn reports_edits_as_one_based_lines_with_utf16_ranges_and_can_leave_chosen_matches_out() {
        let text = "x 😀 foo foo\n\nfoo";
        let matcher = matcher_of(&literal("foo"));
        let edits = compute_edits(&matcher, text, "bar", false, Pick::All);
        let brief: Vec<(u32, u32, u32, &str)> = edits.iter().map(|e| (e.line, e.column, e.column + e.length, e.insert.as_str())).collect();
        assert_eq!(brief, [(1, 5, 8, "bar"), (1, 9, 12, "bar"), (3, 0, 3, "bar")]);
        assert_eq!(edits.iter().map(|e| &text[e.range.clone()]).collect::<Vec<_>>(), ["foo", "foo", "foo"]);

        let dismissed = [MatchKey { line: 1, column: 5, length: 3 }];
        let kept = compute_edits(&matcher, text, "bar", false, Pick::Except(&dismissed));
        assert_eq!(apply_edits(text, &kept), "x 😀 foo bar\n\nbar");

        let only = MatchKey { line: 1, column: 9, length: 3 };
        let one = compute_edits(&matcher, text, "bar", false, Pick::Only(&only));
        assert_eq!(apply_edits(text, &one), "x 😀 foo bar\n\nfoo");
        // A match that is no longer at that position is not replaced.
        let gone = MatchKey { line: 2, column: 0, length: 3 };
        assert!(compute_edits(&matcher, text, "bar", false, Pick::Only(&gone)).is_empty());
    }

    #[test]
    fn utf16_positions_convert_to_byte_offsets_without_splitting_characters() {
        let text = "é😀x";
        assert_eq!(utf16_len(text), 4);
        assert_eq!(utf16_to_byte(text, 0), 0);
        assert_eq!(utf16_to_byte(text, 1), 2);
        assert_eq!(utf16_to_byte(text, 3), 6);
        assert_eq!(utf16_to_byte(text, 4), 7);
        assert_eq!(utf16_to_byte(text, 99), 7);
        // A position in the middle of a surrogate pair is rounded up to the end of its character.
        assert_eq!(utf16_to_byte(text, 2), 6);
    }

    /// Replacement in the editor must produce the same content as the backend's replacement on disk.
    #[test]
    fn replacing_in_memory_matches_the_backend_byte_for_byte() {
        let cases: [(MatchOptions, &str, &str); 6] = [
            (literal("needle"), "thread", "one needle\nNEEDLE two needle\n"),
            (word(literal("cat")), "dog", "cat concat cat_x (cat) cats\r\ncat\r\n"),
            (regex("let (\\w+) = (?P<val>\\d+);"), "const ${1}: u32 = $val; // $$ $9", "let alpha = 1;\nlet beta = 22;\n"),
            (regex("\\s+$"), "", "a  \nb\n\nc \n"),
            (insensitive(regex("(é)(\\w+)")), "$2$1-$0", "été École 😀 émoji\n"),
            (regex("a*"), "-", "bab\nccc\n"),
        ];
        for (options, replacement, text) in cases {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("a.txt"), text).unwrap();
            let query = SearchQuery {
                pattern: options.pattern.clone(),
                is_regex: options.is_regex,
                case_sensitive: options.case_sensitive,
                whole_word: options.whole_word,
                include: Vec::new(),
                exclude: Vec::new(),
                max_results: 0,
            };
            let summary = oncode_workspace::replace_in_files(dir.path(), &query, replacement, &["a.txt".to_string()]).unwrap();
            let on_disk = std::fs::read_to_string(dir.path().join("a.txt")).unwrap();

            let edits = compute_edits(&matcher_of(&options), text, replacement, options.is_regex, Pick::All);
            assert_eq!(apply_edits(text, &edits), on_disk, "pattern {:?}", options.pattern);
            assert_eq!(edits.len() as u32, summary.replacements, "pattern {:?}", options.pattern);
        }
    }
}
