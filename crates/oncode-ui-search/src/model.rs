//! Search state without UI: the query, results streamed by the backend, dismissed rows, and
//! the displayed text. All of it is pure functions so it can be tested without a window.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use oncode_workspace::{FileMatches, LineMatch, SearchQuery, SearchSummary};

use crate::matcher::{utf16_len, utf16_to_byte, LineMatcher, MatchKey};

/// Delay after the last keystroke before the search starts.
pub const SEARCH_DEBOUNCE_MS: u64 = 250;
pub const MAX_RESULTS: u32 = 5000;
/// Results that arrive in succession are briefly batched so the list isn't redrawn per file.
pub const FLUSH_MS: u64 = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchStatus {
    #[default]
    Idle,
    Running,
    Done,
    Failed,
}

/// Search form inputs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inputs {
    pub pattern: String,
    pub is_regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    /// Comma-separated globs, exactly as typed.
    pub include: String,
    pub exclude: String,
}

/// `"src, *.ts ,,"` → `["src", "*.ts"]`
pub fn parse_globs(text: &str) -> Vec<String> {
    text.split(',').map(str::trim).filter(|glob| !glob.is_empty()).map(String::from).collect()
}

pub fn build_query(inputs: &Inputs) -> SearchQuery {
    SearchQuery {
        pattern: inputs.pattern.clone(),
        is_regex: inputs.is_regex,
        case_sensitive: inputs.case_sensitive,
        whole_word: inputs.whole_word,
        include: parse_globs(&inputs.include),
        exclude: parse_globs(&inputs.exclude),
        max_results: MAX_RESULTS,
    }
}

/// Text selected in the editor as a query: in regex mode its special characters are escaped so
/// it is searched for as is.
pub fn prefill_from_selection(selection: &str, is_regex: bool) -> Option<String> {
    if selection.is_empty() || selection.contains(['\n', '\r']) {
        return None;
    }
    if !is_regex {
        return Some(selection.to_string());
    }
    let mut escaped = String::with_capacity(selection.len() + 4);
    for ch in selection.chars() {
        if "\\^$.*+?()[]{}|".contains(ch) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    Some(escaped)
}

pub fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

pub fn match_key(m: &LineMatch) -> MatchKey {
    MatchKey { line: m.line, column: m.column, length: m.length }
}

/// A row in the flattened result tree; indices point into [`Results::files`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    File { file: usize },
    Match { file: usize, index: usize },
}

/// Row identity that stays valid even when the list changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKey {
    File(String),
    Match { path: String, line: u32, column: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub files: usize,
    pub matches: usize,
}

/// The search results currently displayed.
#[derive(Debug, Default)]
pub struct Results {
    pub status: SearchStatus,
    pub error: Option<String>,
    pub summary: Option<SearchSummary>,
    /// Sorted by path. Files and matches the user dismissed are no longer here.
    pub files: Vec<FileMatches>,
    /// Matches dismissed from files that are still displayed; replacement must not touch them.
    pub dismissed: HashMap<String, Vec<MatchKey>>,
    pub collapsed: HashSet<String>,
    pub selected: Option<RowKey>,
    /// The query that produced `files`; used by replacement so it matches exactly what is displayed.
    pub query: Option<SearchQuery>,
    /// The inputs changed and the new search has not started yet; the displayed results no longer represent the inputs.
    pub pending: bool,
}

impl Results {
    /// Clears everything (empty query or the folder changed).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// A new search starts.
    pub fn start(&mut self, query: SearchQuery) {
        self.clear();
        self.status = SearchStatus::Running;
        self.query = Some(query);
    }

    /// Adds newly arrived files; the list stays sorted by path, whatever order the backend uses.
    pub fn merge(&mut self, incoming: Vec<FileMatches>) {
        if incoming.is_empty() {
            return;
        }
        self.files.extend(incoming);
        self.files.sort_by(|a, b| a.path.cmp(&b.path));
    }

    pub fn finish(&mut self, summary: SearchSummary) {
        self.status = SearchStatus::Done;
        self.summary = Some(summary);
    }

    pub fn fail(&mut self, message: String) {
        self.status = SearchStatus::Failed;
        self.error = Some(message);
    }

    pub fn truncated(&self) -> bool {
        self.summary.is_some_and(|summary| summary.truncated)
    }

    pub fn counts(&self) -> Counts {
        counts_of(&self.files)
    }

    pub fn file(&self, path: &str) -> Option<&FileMatches> {
        self.files.iter().find(|file| file.path == path)
    }

    /// The flattened result tree: a file row followed by its matches, except for collapsed files.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::with_capacity(self.files.len());
        for (file, matches) in self.files.iter().enumerate() {
            rows.push(Row::File { file });
            if !self.collapsed.contains(&matches.path) {
                rows.extend((0..matches.matches.len()).map(|index| Row::Match { file, index }));
            }
        }
        rows
    }

    pub fn row_key(&self, row: Row) -> Option<RowKey> {
        match row {
            Row::File { file } => self.files.get(file).map(|f| RowKey::File(f.path.clone())),
            Row::Match { file, index } => {
                let f = self.files.get(file)?;
                let m = f.matches.get(index)?;
                Some(RowKey::Match { path: f.path.clone(), line: m.line, column: m.column })
            }
        }
    }

    pub fn is_collapsed(&self, path: &str) -> bool {
        self.collapsed.contains(path)
    }

    pub fn set_collapsed(&mut self, path: &str, collapsed: bool) {
        if collapsed {
            self.collapsed.insert(path.to_string());
        } else {
            self.collapsed.remove(path);
        }
    }

    pub fn all_collapsed(&self) -> bool {
        !self.files.is_empty() && self.files.iter().all(|file| self.collapsed.contains(&file.path))
    }

    pub fn set_all_collapsed(&mut self, collapsed: bool) {
        self.collapsed = if collapsed { self.files.iter().map(|file| file.path.clone()).collect() } else { HashSet::new() };
    }

    /// Dismisses a file from the results (and from replacement).
    pub fn dismiss_file(&mut self, path: &str) {
        self.files.retain(|file| file.path != path);
        self.dismissed.remove(path);
    }

    /// Dismisses a single match; a file left with no matches disappears too.
    pub fn dismiss_match(&mut self, path: &str, key: MatchKey) {
        let Some(file) = self.files.iter_mut().find(|file| file.path == path) else { return };
        file.matches.retain(|m| !(m.line == key.line && m.column == key.column));
        if file.matches.is_empty() {
            self.dismiss_file(path);
        } else {
            self.dismissed.entry(path.to_string()).or_default().push(key);
        }
    }

    /// A match has been replaced with `inserted`: its row is removed, and other matches on
    /// the same line are shifted (their column and preview) so they still point at the right text.
    pub fn match_replaced(&mut self, path: &str, key: MatchKey, inserted: &str) {
        let Some(file) = self.files.iter_mut().find(|file| file.path == path) else { return };
        file.matches.retain(|m| !(m.line == key.line && m.column == key.column));
        let delta = utf16_len(inserted) as i64 - i64::from(key.length);
        let shift = |column: u32| u32::try_from(i64::from(column) + delta).unwrap_or(0);
        for m in file.matches.iter_mut().filter(|m| m.line == key.line) {
            // The position of the replaced match within this line's preview (may be outside its excerpt).
            let at = i64::from(m.preview_offset) + i64::from(key.column) - i64::from(m.column);
            let patched = patch_preview(&mut m.preview, at, key.length, inserted);
            if m.column > key.column {
                m.column = shift(m.column);
                if patched {
                    m.preview_offset = shift(m.preview_offset);
                }
            }
        }
        if let Some(dismissed) = self.dismissed.get_mut(path) {
            for other in dismissed.iter_mut().filter(|other| other.line == key.line && other.column > key.column) {
                other.column = shift(other.column);
            }
        }
        if file.matches.is_empty() {
            self.dismiss_file(path);
        }
    }

    /// "Replace All" is only allowed for a finished search that still represents the inputs.
    pub fn can_replace(&self, replacing: bool) -> bool {
        self.status == SearchStatus::Done && !self.pending && !replacing && !self.files.is_empty()
    }

    /// The summary line above the results; `true` if it is an error message.
    pub fn status_line(&self, replacing: bool) -> Option<(String, bool)> {
        if replacing {
            return Some(("Replacing…".into(), false));
        }
        let counts = self.counts();
        match self.status {
            SearchStatus::Idle => None,
            SearchStatus::Failed => Some((self.error.clone().unwrap_or_else(|| "The search failed.".into()), true)),
            SearchStatus::Running if counts.matches > 0 => Some((format!("{} so far…", summary_text(counts, false)), false)),
            SearchStatus::Running => Some(("Searching…".into(), false)),
            SearchStatus::Done if counts.matches == 0 => Some(("No results found.".into(), false)),
            SearchStatus::Done => Some((summary_text(counts, self.truncated()), false)),
        }
    }
}

/// Replaces `old_len` UTF-16 units starting at position `at` in `preview` with `inserted`. Returns
/// `false` if that range is not (entirely) inside the preview.
fn patch_preview(preview: &mut String, at: i64, old_len: u32, inserted: &str) -> bool {
    let Ok(at) = usize::try_from(at) else { return false };
    let end = at + old_len as usize;
    if end > utf16_len(preview) {
        return false;
    }
    let range = utf16_to_byte(preview, at)..utf16_to_byte(preview, end);
    preview.replace_range(range, inserted);
    true
}

pub fn counts_of(files: &[FileMatches]) -> Counts {
    Counts { files: files.len(), matches: files.iter().map(|file| file.matches.len()).sum() }
}

/// "3 results in 2 files", with a note if the backend stopped before finishing.
pub fn summary_text(counts: Counts, truncated: bool) -> String {
    format!("{} in {}{}", plural(counts.matches, "result"), plural(counts.files, "file"), if truncated { " — results truncated" } else { "" })
}

/// The confirmation prompt for the results currently displayed: `(title, message)`.
pub fn confirmation_text(counts: Counts, replacement: &str, truncated: bool) -> (String, String) {
    let what = if replacement.is_empty() {
        "Each occurrence will be deleted (the replacement is empty).".to_string()
    } else {
        format!("Each occurrence will be replaced with \"{replacement}\".")
    };
    let scope = if truncated { " The results were truncated: every occurrence in the listed files is replaced, including ones not shown." } else { "" };
    (
        format!("Replace {} across {}?", plural(counts.matches, "occurrence"), plural(counts.files, "file")),
        format!("{what}{scope} Files that are not open in the editor are changed on disk immediately."),
    )
}

/// The outcome of a replacement.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplaceOutcome {
    pub replacements: usize,
    pub files: usize,
    /// Files changed in the editor but not saved (already contained other changes
    /// beforehand, or failed to save).
    pub unsaved: usize,
    pub skipped_read_only: Vec<String>,
    pub errors: Vec<String>,
}

impl ReplaceOutcome {
    /// Records the result of one file.
    pub fn count(&mut self, replacements: usize) -> bool {
        if replacements == 0 {
            return false;
        }
        self.replacements += replacements;
        self.files += 1;
        true
    }
}

/// Notifications after a replacement.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub success: Option<String>,
    pub info: Option<String>,
    pub warning: Option<String>,
    /// `(message, details)`.
    pub error: Option<(String, String)>,
}

pub fn report(outcome: &ReplaceOutcome) -> Report {
    let mut report = Report::default();
    if outcome.replacements > 0 {
        let unsaved = if outcome.unsaved > 0 {
            format!(" {} {} changed in the editor and left unsaved.", plural(outcome.unsaved, "open file"), if outcome.unsaved == 1 { "was" } else { "were" })
        } else {
            String::new()
        };
        report.success = Some(format!("Replaced {} across {}.{unsaved}", plural(outcome.replacements, "occurrence"), plural(outcome.files, "file")));
    } else if outcome.errors.is_empty() {
        report.info = Some("Nothing was replaced.".into());
    }
    if !outcome.skipped_read_only.is_empty() {
        report.warning = Some(format!("Skipped {}.", plural(outcome.skipped_read_only.len(), "read-only file")));
    }
    if !outcome.errors.is_empty() {
        report.error = Some((format!("{} failed", plural(outcome.errors.len(), "replacement")), outcome.errors.join("\n")));
    }
    report
}

/// The preview of a match, split for display: the text before, the match, and the text after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewParts<'a> {
    pub before: &'a str,
    pub hit: &'a str,
    pub after: &'a str,
}

/// Splits the preview; leading indentation is dropped so the match is seen sooner.
pub fn split_preview(m: &LineMatch) -> PreviewParts<'_> {
    let preview = m.preview.as_str();
    let start = utf16_to_byte(preview, m.preview_offset as usize);
    let end = utf16_to_byte(preview, (m.preview_offset + m.length) as usize).max(start);
    let lead = (preview.len() - preview.trim_start().len()).min(start);
    PreviewParts { before: &preview[lead..start], hit: &preview[start..end], after: &preview[end..] }
}

/// The text of one result row along with the byte range of the match and (if any) its replacement text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowText {
    pub text: String,
    pub hit: Range<usize>,
    pub inserted: Option<Range<usize>>,
}

/// Builds the result row text: `before` + match + replacement text (in replace mode) + `after`.
pub fn row_text(m: &LineMatch, replacement: Option<&str>) -> RowText {
    let parts = split_preview(m);
    let mut text = String::with_capacity(m.preview.len() + replacement.map_or(0, str::len));
    text.push_str(parts.before);
    let hit = text.len()..text.len() + parts.hit.len();
    text.push_str(parts.hit);
    let inserted = replacement.filter(|replacement| !replacement.is_empty()).map(|replacement| {
        let range = text.len()..text.len() + replacement.len();
        text.push_str(replacement);
        range
    });
    text.push_str(parts.after);
    RowText { text, hit, inserted }
}

/// The replacement text for a single match (with `$1` etc. filled in when it can be computed from the preview).
pub fn replacement_preview(matcher: Option<&LineMatcher>, interpolate: bool, replacement: &str, m: &LineMatch) -> String {
    if !interpolate {
        return replacement.to_string();
    }
    let Some(matcher) = matcher else { return replacement.to_string() };
    let preview = m.preview.as_str();
    let start = utf16_to_byte(preview, m.preview_offset as usize);
    let end = utf16_to_byte(preview, (m.preview_offset + m.length) as usize);
    match matcher.find(preview).into_iter().find(|hit| hit.start == start && hit.end == end) {
        Some(hit) => matcher.replacement_for(preview, &hit, replacement, true),
        None => replacement.to_string(),
    }
}

/// Position for the editor: `(line starting at 1, column in characters, length in characters)`.
/// The backend reports UTF-16 units; the difference is computed from the part of the line that is in the preview.
pub fn reveal_position(m: &LineMatch) -> (u32, u32, u32) {
    let preview = m.preview.as_str();
    let start = utf16_to_byte(preview, m.preview_offset as usize);
    let end = utf16_to_byte(preview, (m.preview_offset + m.length) as usize).max(start);
    let surrogates = |text: &str| (utf16_len(text) - text.chars().count()) as u32;
    let column = m.column.saturating_sub(surrogates(&preview[..start]));
    let length = m.length.saturating_sub(surrogates(&preview[start..end]));
    (m.line, column, length)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matcher::MatchOptions;

    fn m(line: u32, column: u32, preview: &str, length: u32) -> LineMatch {
        LineMatch { line, column, length, preview: preview.into(), preview_offset: column }
    }

    fn file(path: &str, matches: Vec<LineMatch>) -> FileMatches {
        FileMatches { path: path.into(), matches }
    }

    fn done(files: Vec<FileMatches>) -> Results {
        let mut results = Results::default();
        results.start(build_query(&Inputs { pattern: "foo".into(), ..Default::default() }));
        let counts = counts_of(&files);
        results.merge(files);
        results.finish(SearchSummary { files: counts.files as u32, matches: counts.matches as u32, truncated: false });
        results
    }

    #[test]
    fn splits_comma_separated_globs_and_drops_blanks() {
        assert_eq!(parse_globs("src, *.ts ,,"), ["src", "*.ts"]);
        assert_eq!(parse_globs("   "), Vec::<String>::new());
    }

    #[test]
    fn carries_the_options_and_the_result_cap() {
        let query = build_query(&Inputs { pattern: "foo".into(), is_regex: true, case_sensitive: false, whole_word: true, include: "src".into(), exclude: "dist, *.min.js".into() });
        assert_eq!(
            query,
            SearchQuery {
                pattern: "foo".into(),
                is_regex: true,
                case_sensitive: false,
                whole_word: true,
                include: vec!["src".into()],
                exclude: vec!["dist".into(), "*.min.js".into()],
                max_results: MAX_RESULTS,
            }
        );
    }

    #[test]
    fn words_the_summary_with_singular_and_plural_forms() {
        let one = [file("a.ts", vec![m(1, 0, "foo", 3)])];
        let many = [file("a.ts", vec![m(1, 0, "foo", 3), m(2, 0, "foo", 3)]), file("b.ts", vec![m(1, 0, "foo", 3)])];
        assert_eq!(summary_text(counts_of(&one), false), "1 result in 1 file");
        assert_eq!(summary_text(counts_of(&many), false), "3 results in 2 files");
        assert_eq!(summary_text(counts_of(&many), true), "3 results in 2 files — results truncated");
    }

    #[test]
    fn keeps_files_sorted_by_path_however_the_backend_orders_them_and_flattens_them_into_rows() {
        let mut results = Results::default();
        results.start(build_query(&Inputs { pattern: "x".into(), ..Default::default() }));
        results.merge(vec![file("src/z.ts", vec![m(1, 0, "x", 1)])]);
        results.merge(vec![file("a.ts", vec![m(1, 0, "x", 1), m(4, 2, "  x", 1)]), file("src/b.ts", vec![m(1, 0, "x", 1)])]);
        assert_eq!(results.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["a.ts", "src/b.ts", "src/z.ts"]);

        results.set_collapsed("src/b.ts", true);
        assert_eq!(
            results.rows(),
            [Row::File { file: 0 }, Row::Match { file: 0, index: 0 }, Row::Match { file: 0, index: 1 }, Row::File { file: 1 }, Row::File { file: 2 }, Row::Match { file: 2, index: 0 }]
        );
        assert_eq!(results.row_key(Row::Match { file: 0, index: 1 }), Some(RowKey::Match { path: "a.ts".into(), line: 4, column: 2 }));
        assert_eq!(results.row_key(Row::File { file: 1 }), Some(RowKey::File("src/b.ts".into())));

        assert!(!results.all_collapsed());
        results.set_all_collapsed(true);
        assert!(results.all_collapsed());
        assert_eq!(results.rows().len(), 3);
        results.set_all_collapsed(false);
        assert_eq!(results.rows().len(), 7);
    }

    #[test]
    fn status_line_follows_the_run() {
        let mut results = Results::default();
        assert_eq!(results.status_line(false), None);
        results.start(build_query(&Inputs { pattern: "foo".into(), ..Default::default() }));
        assert_eq!(results.status_line(false), Some(("Searching…".into(), false)));
        results.merge(vec![file("a.ts", vec![m(1, 0, "foo", 3)])]);
        assert_eq!(results.status_line(false), Some(("1 result in 1 file so far…".into(), false)));
        results.finish(SearchSummary { files: 1, matches: 1, truncated: true });
        assert_eq!(results.status_line(false), Some(("1 result in 1 file — results truncated".into(), false)));
        assert_eq!(results.status_line(true), Some(("Replacing…".into(), false)));

        let mut empty = Results::default();
        empty.start(build_query(&Inputs { pattern: "foo".into(), ..Default::default() }));
        empty.finish(SearchSummary { files: 0, matches: 0, truncated: false });
        assert_eq!(empty.status_line(false), Some(("No results found.".into(), false)));

        let mut failed = Results::default();
        failed.start(build_query(&Inputs { pattern: "(".into(), is_regex: true, ..Default::default() }));
        failed.fail("Invalid search pattern: unclosed group".into());
        assert_eq!(failed.status_line(false), Some(("Invalid search pattern: unclosed group".into(), true)));
    }

    #[test]
    fn removes_dismissed_matches_and_files_from_the_list_and_the_summary() {
        let mut results = done(vec![file("a.ts", vec![m(1, 0, "foo", 3), m(2, 4, "let foo", 3)]), file("b.ts", vec![m(1, 0, "foo", 3)])]);
        results.dismiss_match("a.ts", MatchKey { line: 1, column: 0, length: 3 });
        assert_eq!(results.counts(), Counts { files: 2, matches: 2 });
        assert_eq!(results.dismissed["a.ts"], [MatchKey { line: 1, column: 0, length: 3 }]);

        // A file's last match takes the file with it, including the dismissed record.
        results.dismiss_match("a.ts", MatchKey { line: 2, column: 4, length: 3 });
        assert_eq!(results.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["b.ts"]);
        assert!(!results.dismissed.contains_key("a.ts"));

        results.dismiss_file("b.ts");
        assert_eq!(results.counts(), Counts { files: 0, matches: 0 });
        assert_eq!(results.status_line(false), Some(("No results found.".into(), false)));
        assert!(!results.can_replace(false));
    }

    #[test]
    fn replace_is_only_offered_for_a_finished_search_that_matches_the_inputs() {
        let mut results = Results::default();
        results.start(build_query(&Inputs { pattern: "foo".into(), ..Default::default() }));
        results.merge(vec![file("a.ts", vec![m(1, 0, "foo", 3)])]);
        assert!(!results.can_replace(false), "the search is still running");
        results.finish(SearchSummary { files: 1, matches: 1, truncated: false });
        assert!(results.can_replace(false));
        assert!(!results.can_replace(true), "a replacement is in progress");
        results.pending = true;
        assert!(!results.can_replace(false), "the inputs have changed");
    }

    #[test]
    fn asks_with_the_counts_of_what_is_still_listed() {
        let files = [file("a.ts", vec![m(1, 0, "foo", 3), m(2, 0, "foo", 3)]), file("b.ts", vec![m(1, 0, "foo", 3)])];
        let (title, message) = confirmation_text(counts_of(&files), "bar", false);
        assert_eq!(title, "Replace 3 occurrences across 2 files?");
        assert_eq!(message, "Each occurrence will be replaced with \"bar\". Files that are not open in the editor are changed on disk immediately.");

        let (title, message) = confirmation_text(counts_of(&files[1..]), "", true);
        assert_eq!(title, "Replace 1 occurrence across 1 file?");
        assert!(message.starts_with("Each occurrence will be deleted (the replacement is empty). The results were truncated:"), "{message}");
    }

    #[test]
    fn reports_the_totals_and_what_was_left_out() {
        let outcome = ReplaceOutcome { replacements: 3, files: 2, ..Default::default() };
        assert_eq!(report(&outcome).success.as_deref(), Some("Replaced 3 occurrences across 2 files."));

        let outcome = ReplaceOutcome { replacements: 1, files: 1, unsaved: 1, ..Default::default() };
        assert_eq!(report(&outcome).success.as_deref(), Some("Replaced 1 occurrence across 1 file. 1 open file was changed in the editor and left unsaved."));

        let outcome = ReplaceOutcome { replacements: 4, files: 2, unsaved: 2, skipped_read_only: vec!["a".into()], ..Default::default() };
        let text = report(&outcome);
        assert_eq!(text.success.as_deref(), Some("Replaced 4 occurrences across 2 files. 2 open files were changed in the editor and left unsaved."));
        assert_eq!(text.warning.as_deref(), Some("Skipped 1 read-only file."));

        assert_eq!(report(&ReplaceOutcome::default()).info.as_deref(), Some("Nothing was replaced."));
        let failed = report(&ReplaceOutcome { errors: vec!["a.ts: denied".into(), "b.ts: gone".into()], ..Default::default() });
        assert_eq!(failed.info, None);
        assert_eq!(failed.error, Some(("2 replacements failed".into(), "a.ts: denied\nb.ts: gone".into())));
    }

    #[test]
    fn shows_the_match_without_the_leading_indentation() {
        let hit = m(2, 4, "    let foo = 1;", 3);
        let hit = LineMatch { column: 8, preview_offset: 8, ..hit };
        assert_eq!(split_preview(&hit), PreviewParts { before: "let ", hit: "foo", after: " = 1;" });
        // A match inside the indentation: the indentation isn't dropped past the start of the match.
        let spaces = m(1, 2, "    x", 2);
        assert_eq!(split_preview(&spaces), PreviewParts { before: "", hit: "  ", after: "x" });
        // Offsets in UTF-16 units, text in UTF-8.
        let astral = m(1, 3, "😀 foo é", 3);
        assert_eq!(split_preview(&astral), PreviewParts { before: "😀 ", hit: "foo", after: " é" });
    }

    #[test]
    fn previews_the_replacement_next_to_each_match_expanding_capture_references() {
        let options = MatchOptions { pattern: "let (\\w+)".into(), is_regex: true, case_sensitive: true, whole_word: false };
        let matcher = LineMatcher::new(&options).unwrap();
        let hit = m(1, 2, "  let alpha = 1;", 9);
        assert_eq!(replacement_preview(Some(&matcher), true, "const $1", &hit), "const alpha");
        // Literal mode: the replacement text as is.
        assert_eq!(replacement_preview(Some(&matcher), false, "const $1", &hit), "const $1");
        // The match is no longer found in the preview (e.g. truncated): the replacement text as is.
        let cut = m(1, 2, "  let", 9);
        assert_eq!(replacement_preview(Some(&matcher), true, "const $1", &cut), "const $1");

        let row = row_text(&hit, Some("const alpha"));
        assert_eq!(row.text, "let alphaconst alpha = 1;");
        assert_eq!(&row.text[row.hit.clone()], "let alpha");
        assert_eq!(&row.text[row.inserted.clone().unwrap()], "const alpha");
        let plain = row_text(&hit, None);
        assert_eq!((plain.text.as_str(), plain.inserted), ("let alpha = 1;", None));
        // Empty replacement (deleting): only the match is marked.
        assert_eq!(row_text(&hit, Some("")).inserted, None);
    }

    #[test]
    fn converts_utf16_columns_to_editor_characters() {
        assert_eq!(reveal_position(&m(2, 4, "    println!(\"hello\");", 7)), (2, 4, 7));
        // An astral character before the match counts as two units for the backend, one character for the editor.
        assert_eq!(reveal_position(&m(1, 7, "a 😀😀 foo", 3)), (1, 5, 3));
        assert_eq!(reveal_position(&m(1, 0, "😀x", 3)), (1, 0, 2));
    }

    #[test]
    fn seeds_the_query_from_a_single_line_selection() {
        assert_eq!(prefill_from_selection("a.b(c)", false).as_deref(), Some("a.b(c)"));
        assert_eq!(prefill_from_selection("a.b(c)", true).as_deref(), Some("a\\.b\\(c\\)"));
        assert_eq!(prefill_from_selection("", false), None);
        assert_eq!(prefill_from_selection("two\nlines", false), None);
    }

    #[test]
    fn a_replaced_match_leaves_the_rest_of_its_line_pointing_at_the_right_text() {
        let line = "foo = foo + foo;";
        let mut results = done(vec![file("a.ts", vec![m(1, 0, line, 3), m(1, 6, line, 3), m(1, 12, line, 3), m(2, 0, "foo", 3)])]);
        results.dismiss_match("a.ts", MatchKey { line: 1, column: 12, length: 3 });

        results.match_replaced("a.ts", MatchKey { line: 1, column: 6, length: 3 }, "quux");
        let matches = &results.files[0].matches;
        assert_eq!(matches.iter().map(|m| (m.line, m.column)).collect::<Vec<_>>(), [(1, 0), (2, 0)]);
        assert_eq!(matches[0].preview, "foo = quux + foo;");
        assert_eq!(matches[0].preview_offset, 0);
        // A dismissed match to the right shifts too, so it is still skipped by the next replacement.
        assert_eq!(results.dismissed["a.ts"], [MatchKey { line: 1, column: 13, length: 3 }]);

        results.match_replaced("a.ts", MatchKey { line: 1, column: 0, length: 3 }, "");
        assert_eq!(results.dismissed["a.ts"], [MatchKey { line: 1, column: 10, length: 3 }]);
        results.match_replaced("a.ts", MatchKey { line: 2, column: 0, length: 3 }, "x");
        assert!(results.files.is_empty(), "a file with no matches left disappears too");
    }

    #[test]
    fn a_replaced_match_shifts_later_matches_and_their_previews() {
        let line = "a😀 foo foo";
        let mut results = done(vec![file("a.ts", vec![m(1, 4, line, 3), m(1, 8, line, 3)])]);
        results.match_replaced("a.ts", MatchKey { line: 1, column: 4, length: 3 }, "é");
        let rest = &results.files[0].matches[0];
        assert_eq!((rest.column, rest.preview_offset, rest.preview.as_str()), (6, 6, "a😀 é foo"));
        assert_eq!(split_preview(rest).hit, "foo");
    }
}
