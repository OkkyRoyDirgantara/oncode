//! Project text search and replace across many files.

use std::borrow::Cow;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use grep_matcher::{Captures, Matcher};
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkMatch};
use ignore::WalkState;
use serde::{Deserialize, Serialize};

use crate::error::{Result, WsError};
use crate::fs::{absolute, is_inside, write_atomic};
use crate::gitignore::walk_builder;
use crate::index::to_slash;

/// Maximum length of `LineMatch::preview`, in characters.
const PREVIEW_MAX_CHARS: usize = 400;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    pub pattern: String,
    /// `false`: `pattern` is searched for literally. `true`: the `regex` crate's syntax.
    pub is_regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    /// Globs of files to include; empty means all. `*.rs` matches at any depth,
    /// `src` covers the entire contents of the `src` folder, and globs containing a slash (`src/*.ts`)
    /// are matched from the project root.
    pub include: Vec<String>,
    /// Globs of files or folders to exclude, with the same rules.
    pub exclude: Vec<String>,
    /// Maximum number of matches; `0` means unlimited.
    pub max_results: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineMatch {
    /// Line number, starting from 1.
    pub line: u32,
    /// Start of the match within the line, in UTF-16 units, starting from 0.
    pub column: u32,
    /// Length of the match in UTF-16 units.
    pub length: u32,
    /// The line contents without the line ending, trimmed to at most 400 characters around
    /// the match. A match longer than that is truncated too.
    pub preview: String,
    /// Start of the match within `preview`, in UTF-16 units.
    pub preview_offset: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMatches {
    /// Path relative to the project root, with forward slashes.
    pub path: String,
    /// Sorted by line, then column.
    pub matches: Vec<LineMatch>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchSummary {
    pub files: u32,
    pub matches: u32,
    /// The search stopped before finishing: `max_results` was reached or it was canceled.
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceSummary {
    /// Number of files changed.
    pub files: u32,
    pub replacements: u32,
}

/// Searches for `query` in all files under `root`. Blocking; the walk runs in parallel.
///
/// Honors gitignore, skips binary files and the `.git` folder. `on_file` is called once
/// for each file that contains matches, from a worker thread and in no particular order.
/// The search stops early if `cancel` is set or `max_results` is reached; in both
/// cases `truncated` is `true`. Matches never cross lines, and
/// empty matches (e.g. regex `a*` on a line without `a`) are not reported.
pub fn search(
    root: &Path,
    query: &SearchQuery,
    cancel: &AtomicBool,
    on_file: &(dyn Fn(FileMatches) + Sync),
) -> Result<SearchSummary> {
    let matcher = build_matcher(query)?;
    let filter = GlobFilter::new(&query.include, &query.exclude)?;
    let root = &absolute(root)?;
    let progress = Progress {
        cancel,
        limit: if query.max_results == 0 { u32::MAX } else { query.max_results },
        matches: AtomicU32::new(0),
        files: AtomicU32::new(0),
        truncated: AtomicBool::new(false),
    };

    walk_builder(root).build_parallel().run(|| {
        let matcher = &matcher;
        let filter = &filter;
        let progress = &progress;
        let mut searcher = SearcherBuilder::new()
            .line_terminator(grep_matcher::LineTerminator::crlf())
            .binary_detection(BinaryDetection::quit(0))
            .line_number(true)
            .build();
        Box::new(move |entry| {
            if progress.should_stop() {
                return WalkState::Quit;
            }
            let Ok(entry) = entry else { return WalkState::Continue };
            let Ok(rel) = entry.path().strip_prefix(root) else { return WalkState::Continue };
            if rel.as_os_str().is_empty() {
                return WalkState::Continue;
            }
            let rel = to_slash(rel);
            let Some(file_type) = entry.file_type() else { return WalkState::Continue };
            if file_type.is_dir() {
                return if filter.is_excluded(&rel) { WalkState::Skip } else { WalkState::Continue };
            }
            if !file_type.is_file() || !filter.accepts_file(&rel) {
                return WalkState::Continue;
            }

            let mut collector = Collector { matcher, progress, matches: Vec::new() };
            // Files that cannot be read are skipped; matches already collected are still sent.
            let _ = searcher.search_path(matcher, entry.path(), &mut collector);
            if !collector.matches.is_empty() {
                progress.files.fetch_add(1, Ordering::Relaxed);
                on_file(FileMatches { path: rel, matches: collector.matches });
            }
            if progress.should_stop() {
                WalkState::Quit
            } else {
                WalkState::Continue
            }
        })
    });

    let cancelled = cancel.load(Ordering::Relaxed);
    Ok(SearchSummary {
        files: progress.files.load(Ordering::Relaxed),
        matches: progress.matches.load(Ordering::Relaxed),
        truncated: cancelled || progress.truncated.load(Ordering::Relaxed),
    })
}

/// Replaces every match of `query` in the given files with `replacement`.
///
/// `files` contains paths relative to `root`; all of them must be inside `root`, otherwise
/// an `InvalidPath` error is returned before any file is changed. `replacement` is
/// literal text if `!is_regex`; if `is_regex`, `$1`, `${name}`, and `$$` are interpreted.
/// Only UTF-8 files are processed; binary files, files in another encoding, or ones that no longer
/// exist are skipped. Each file is written atomically; `include`, `exclude`, and `max_results`
/// in `query` are not used. An I/O error stops the process; files already written
/// before it stay changed.
pub fn replace_in_files(root: &Path, query: &SearchQuery, replacement: &str, files: &[String]) -> Result<ReplaceSummary> {
    let matcher = build_matcher(query)?;
    let mut targets = Vec::with_capacity(files.len());
    for file in files {
        let path = root.join(file);
        if !is_inside(root, &path) {
            return Err(WsError::invalid_path(format!("File is outside the project: {file}")));
        }
        targets.push(path);
    }

    let mut summary = ReplaceSummary { files: 0, replacements: 0 };
    for path in &targets {
        let count = replace_in_file(&matcher, path, replacement, query.is_regex)?;
        if count > 0 {
            summary.files = summary.files.saturating_add(1);
            summary.replacements = summary.replacements.saturating_add(count);
        }
    }
    Ok(summary)
}

fn replace_in_file(matcher: &RegexMatcher, path: &Path, replacement: &str, interpolate: bool) -> Result<u32> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(WsError::io(path, err)),
    };
    if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
        return Ok(0);
    }
    const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
    let (bom, body) = if bytes.starts_with(BOM) { bytes.split_at(BOM.len()) } else { (&[][..], &bytes[..]) };

    let mut caps = matcher.new_captures().map_err(|e| WsError::invalid_pattern(e.to_string()))?;
    let mut out = Vec::with_capacity(bytes.len() + 64);
    out.extend_from_slice(bom);
    let mut count = 0u32;
    // Line by line, same as the search, so `^` and `$` behave identically.
    for line in body.split_inclusive(|b| *b == b'\n') {
        let content = trim_line_terminator(line);
        let terminator = &line[content.len()..];
        matcher
            .replace_with_captures(content, &mut caps, &mut out, |caps, dst| {
                if caps.get(0).is_none_or(|m| m.is_empty()) {
                    return true;
                }
                count = count.saturating_add(1);
                if interpolate {
                    caps.interpolate(|name| matcher.capture_index(name), content, replacement.as_bytes(), dst);
                } else {
                    dst.extend_from_slice(replacement.as_bytes());
                }
                true
            })
            .map_err(|e| WsError::invalid_pattern(e.to_string()))?;
        out.extend_from_slice(terminator);
    }
    if count > 0 {
        write_atomic(path, &out)?;
    }
    Ok(count)
}

fn build_matcher(query: &SearchQuery) -> Result<RegexMatcher> {
    if query.pattern.is_empty() {
        return Err(WsError::invalid_pattern("Search pattern is empty"));
    }
    if !query.is_regex && query.pattern.contains(['\n', '\r']) {
        return Err(WsError::invalid_pattern("Multiline search is not supported yet"));
    }
    RegexMatcherBuilder::new()
        .case_insensitive(!query.case_sensitive)
        .word(query.whole_word)
        .fixed_strings(!query.is_regex)
        // `^` and `$` apply per line; a match never contains `\r` or `\n`.
        .multi_line(true)
        .crlf(true)
        .build(&query.pattern)
        .map_err(|err| WsError::invalid_pattern(format!("Invalid search pattern: {err}")))
}

/// An include/exclude glob filter against relative paths.
struct GlobFilter {
    include: Option<GlobSet>,
    exclude: Option<GlobSet>,
}

impl GlobFilter {
    fn new(include: &[String], exclude: &[String]) -> Result<Self> {
        Ok(Self { include: build_glob_set(include)?, exclude: build_glob_set(exclude)? })
    }

    fn is_excluded(&self, rel: &str) -> bool {
        self.exclude.as_ref().is_some_and(|set| set.is_match(rel))
    }

    fn accepts_file(&self, rel: &str) -> bool {
        self.include.as_ref().is_none_or(|set| set.is_match(rel)) && !self.is_excluded(rel)
    }
}

fn build_glob_set(globs: &[String]) -> Result<Option<GlobSet>> {
    let mut builder = GlobSetBuilder::new();
    let mut any = false;
    for raw in globs {
        let glob = raw.trim();
        let glob = glob.strip_prefix("./").unwrap_or(glob);
        let anchored = glob.starts_with('/');
        let glob = glob.trim_matches('/');
        if glob.is_empty() {
            continue;
        }
        // Without a slash, the glob matches at any depth; each glob also covers the contents of
        // the folder it matches.
        let base = if anchored || glob.contains('/') { glob.to_string() } else { format!("**/{glob}") };
        for pattern in [base.clone(), format!("{base}/**")] {
            let compiled = GlobBuilder::new(&pattern)
                .literal_separator(true)
                .build()
                .map_err(|err| WsError::invalid_pattern(format!("Invalid glob \"{raw}\": {}", err.kind())))?;
            builder.add(compiled);
        }
        any = true;
    }
    if !any {
        return Ok(None);
    }
    builder.build().map(Some).map_err(|err| WsError::invalid_pattern(format!("Invalid glob: {err}")))
}

/// State shared between worker threads.
struct Progress<'a> {
    cancel: &'a AtomicBool,
    limit: u32,
    matches: AtomicU32,
    files: AtomicU32,
    truncated: AtomicBool,
}

impl Progress<'_> {
    fn should_stop(&self) -> bool {
        self.cancel.load(Ordering::Relaxed) || self.truncated.load(Ordering::Relaxed)
    }

    /// Reserves a slot for one match; `false` if the limit has already been reached.
    fn reserve(&self) -> bool {
        let mut current = self.matches.load(Ordering::Relaxed);
        loop {
            if current >= self.limit {
                self.truncated.store(true, Ordering::Relaxed);
                return false;
            }
            match self.matches.compare_exchange_weak(current, current + 1, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => return true,
                Err(actual) => current = actual,
            }
        }
    }
}

struct Collector<'a> {
    matcher: &'a RegexMatcher,
    progress: &'a Progress<'a>,
    matches: Vec<LineMatch>,
}

impl Sink for Collector<'_> {
    type Error = io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> std::result::Result<bool, io::Error> {
        for (line_number, line) in (mat.line_number().unwrap_or(1)..).zip(mat.lines()) {
            if self.progress.cancel.load(Ordering::Relaxed) {
                return Ok(false);
            }
            let line = trim_line_terminator(line);
            let number = u32::try_from(line_number).unwrap_or(u32::MAX);
            let mut cursor = Utf16Cursor::default();
            let mut room = true;
            let progress = self.progress;
            let matches = &mut self.matches;
            let _ = self.matcher.find_iter(line, |m| {
                if m.is_empty() {
                    return true;
                }
                if !progress.reserve() {
                    room = false;
                    return false;
                }
                matches.push(line_match(line, m.start(), m.end(), number, &mut cursor));
                true
            });
            if !room {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn trim_line_terminator(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

/// Computes UTF-16 offsets incrementally so a long line with many matches
/// is not rescanned from the start for each match.
#[derive(Default)]
struct Utf16Cursor {
    byte: usize,
    units: usize,
}

impl Utf16Cursor {
    fn advance_to(&mut self, line: &[u8], byte: usize) -> usize {
        if byte > self.byte {
            self.units += utf16_len(&line[self.byte..byte]);
            self.byte = byte;
        }
        self.units
    }
}

fn utf16_len(bytes: &[u8]) -> usize {
    String::from_utf8_lossy(bytes).chars().map(char::len_utf16).sum()
}

fn line_match(line: &[u8], start: usize, end: usize, number: u32, cursor: &mut Utf16Cursor) -> LineMatch {
    let column = cursor.advance_to(line, start);
    let length = utf16_len(&line[start..end]);

    let matched = String::from_utf8_lossy(&line[start..end]);
    let matched: Cow<'_, str> = match matched.char_indices().nth(PREVIEW_MAX_CHARS) {
        Some((cut, _)) => Cow::Owned(matched[..cut].to_string()),
        None => matched,
    };
    let room = PREVIEW_MAX_CHARS - matched.chars().count();

    // A byte window large enough for `room` characters, snapped to UTF-8 character boundaries.
    let mut before_start = start.saturating_sub(room * 4);
    while before_start < start && is_continuation(line[before_start]) {
        before_start += 1;
    }
    let mut after_end = line.len().min(end + room * 4);
    while after_end > end && after_end < line.len() && is_continuation(line[after_end]) {
        after_end -= 1;
    }
    let before = String::from_utf8_lossy(&line[before_start..start]);
    let after = String::from_utf8_lossy(&line[end..after_end]);
    let before_chars = before.chars().count();
    let after_chars = after.chars().count();

    // Most of the room goes to the text after the match; the rest to the text before it.
    let tail = after_chars.min(room - before_chars.min(room / 4));
    let lead = before_chars.min(room - tail);
    let lead_text = match before.char_indices().nth(before_chars - lead) {
        Some((at, _)) => &before[at..],
        None => "",
    };
    let tail_text = match after.char_indices().nth(tail) {
        Some((at, _)) => &after[..at],
        None => &after[..],
    };

    let mut preview = String::with_capacity(lead_text.len() + matched.len() + tail_text.len());
    preview.push_str(lead_text);
    preview.push_str(&matched);
    preview.push_str(tail_text);
    LineMatch {
        line: number,
        column: saturate(column),
        length: saturate(length),
        preview,
        preview_offset: saturate(lead_text.chars().map(char::len_utf16).sum()),
    }
}

fn is_continuation(byte: u8) -> bool {
    byte & 0b1100_0000 == 0b1000_0000
}

fn saturate(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::WsErrorCode;
    use std::fs;
    use std::sync::Mutex;

    fn write(path: &Path, bytes: impl AsRef<[u8]>) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn literal(pattern: &str) -> SearchQuery {
        SearchQuery {
            pattern: pattern.to_string(),
            is_regex: false,
            case_sensitive: false,
            whole_word: false,
            include: Vec::new(),
            exclude: Vec::new(),
            max_results: 0,
        }
    }

    fn regex(pattern: &str) -> SearchQuery {
        SearchQuery { is_regex: true, case_sensitive: true, ..literal(pattern) }
    }

    /// Runs a search and returns the results sorted by path.
    fn run(root: &Path, query: &SearchQuery) -> (Vec<FileMatches>, SearchSummary) {
        let found = Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);
        let summary = search(root, query, &cancel, &|file| found.lock().unwrap().push(file)).unwrap();
        let mut found = found.into_inner().unwrap();
        found.sort_by(|a, b| a.path.cmp(&b.path));
        (found, summary)
    }

    /// `(path, line, column)` for each match.
    fn hits(root: &Path, query: &SearchQuery) -> Vec<(String, u32, u32)> {
        let (files, _) = run(root, query);
        files.iter().flat_map(|f| f.matches.iter().map(|m| (f.path.clone(), m.line, m.column))).collect()
    }

    fn files_of(root: &Path, query: &SearchQuery) -> Vec<String> {
        run(root, query).0.into_iter().map(|f| f.path).collect()
    }

    #[test]
    fn literal_search_does_not_interpret_regex_syntax() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("a.txt"), "let x = a.b(c);\nlet y = aXb(c);\n");

        assert_eq!(hits(root, &literal("a.b(")), [("a.txt".to_string(), 1, 8)]);
        let (files, summary) = run(root, &regex(r"a.b\("));
        assert_eq!(summary, SearchSummary { files: 1, matches: 2, truncated: false });
        assert_eq!(files[0].matches.iter().map(|m| m.line).collect::<Vec<_>>(), [1, 2]);

        let first = &files[0].matches[0];
        assert_eq!((first.column, first.length, first.preview_offset), (8, 4, 8));
        assert_eq!(first.preview, "let x = a.b(c);");
    }

    #[test]
    fn regex_anchors_work_per_line_and_never_cross_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("a.txt"), "foo bar\nbar foo\r\nfoo\n");

        assert_eq!(hits(root, &regex("^foo")), [("a.txt".to_string(), 1, 0), ("a.txt".to_string(), 3, 0)]);
        // `$` matches before both `\r\n` and `\n`.
        assert_eq!(hits(root, &regex("foo$")), [("a.txt".to_string(), 2, 4), ("a.txt".to_string(), 3, 0)]);
        // `\s` never consumes the line ending.
        assert_eq!(hits(root, &regex(r"bar\s+bar")), []);
        let (files, _) = run(root, &regex(r"foo\s*$"));
        assert_eq!(files[0].matches.iter().map(|m| (m.line, m.length)).collect::<Vec<_>>(), [(2, 3), (3, 3)]);
        assert_eq!(files[0].matches[0].preview, "bar foo");
    }

    #[test]
    fn empty_matches_are_not_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("a.txt"), "bab\nccc\n");
        assert_eq!(hits(root, &regex("a*")), [("a.txt".to_string(), 1, 1)]);
        assert_eq!(hits(root, &regex("^")), []);
    }

    #[test]
    fn case_sensitivity() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("a.txt"), "Hello\nhello\nHELLO\n");

        assert_eq!(hits(root, &literal("hello")).len(), 3);
        let sensitive = SearchQuery { case_sensitive: true, ..literal("hello") };
        assert_eq!(hits(root, &sensitive), [("a.txt".to_string(), 2, 0)]);
        let insensitive_regex = SearchQuery { case_sensitive: false, ..regex("h[a-z]llo") };
        assert_eq!(hits(root, &insensitive_regex).len(), 3);
        assert_eq!(hits(root, &regex("h[a-z]llo")).len(), 1);
    }

    #[test]
    fn whole_word() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("a.txt"), "cat concat cat_x (cat) cats\n");

        assert_eq!(hits(root, &literal("cat")).len(), 5);
        let word = SearchQuery { whole_word: true, ..literal("cat") };
        assert_eq!(hits(root, &word), [("a.txt".to_string(), 1, 0), ("a.txt".to_string(), 1, 18)]);
        let word_regex = SearchQuery { whole_word: true, ..regex("ca.") };
        assert_eq!(hits(root, &word_regex), [("a.txt".to_string(), 1, 0), ("a.txt".to_string(), 1, 18)]);
    }

    #[test]
    fn several_matches_on_one_line() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("a.txt"), "ab ab ab\n");
        let (files, summary) = run(root, &literal("ab"));
        assert_eq!(summary.matches, 3);
        assert_eq!(files[0].matches.iter().map(|m| m.column).collect::<Vec<_>>(), [0, 3, 6]);
        assert!(files[0].matches.iter().all(|m| m.line == 1 && m.preview == "ab ab ab" && m.preview_offset == m.column));
    }

    fn glob_tree(root: &Path) {
        write(&root.join("src/main.rs"), "needle\n");
        write(&root.join("src/util/helper.rs"), "needle\n");
        write(&root.join("src/app.ts"), "needle\n");
        write(&root.join("web/src/index.ts"), "needle\n");
        write(&root.join("web/dist/bundle.js"), "needle\n");
        write(&root.join("README.md"), "needle\n");
    }

    #[test]
    fn include_globs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        glob_tree(root);
        let with = |include: &[&str]| SearchQuery { include: include.iter().map(|s| s.to_string()).collect(), ..literal("needle") };

        assert_eq!(files_of(root, &literal("needle")).len(), 6);
        assert_eq!(files_of(root, &with(&["*.rs"])), ["src/main.rs", "src/util/helper.rs"]);
        assert_eq!(files_of(root, &with(&["*.rs", "*.md"])), ["README.md", "src/main.rs", "src/util/helper.rs"]);
        // A folder name covers its contents, at any depth.
        assert_eq!(
            files_of(root, &with(&["src"])),
            ["src/app.ts", "src/main.rs", "src/util/helper.rs", "web/src/index.ts"]
        );
        // A glob containing a slash is matched from the root; `*` does not cross separators.
        assert_eq!(files_of(root, &with(&["src/*.ts"])), ["src/app.ts"]);
        assert_eq!(files_of(root, &with(&["./src/*.rs"])), ["src/main.rs"]);
        assert_eq!(files_of(root, &with(&["/src"])), ["src/app.ts", "src/main.rs", "src/util/helper.rs"]);
        assert_eq!(files_of(root, &with(&["**/*.ts"])), ["src/app.ts", "web/src/index.ts"]);
        assert_eq!(files_of(root, &with(&["web/"])), ["web/dist/bundle.js", "web/src/index.ts"]);
        assert_eq!(files_of(root, &with(&["*.{js,md}"])), ["README.md", "web/dist/bundle.js"]);
        // Empty globs are ignored.
        assert_eq!(files_of(root, &with(&["", "  "])).len(), 6);
    }

    #[test]
    fn exclude_globs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        glob_tree(root);
        let without = |exclude: &[&str]| SearchQuery { exclude: exclude.iter().map(|s| s.to_string()).collect(), ..literal("needle") };

        assert_eq!(files_of(root, &without(&["*.ts", "dist"])), ["README.md", "src/main.rs", "src/util/helper.rs"]);
        assert_eq!(files_of(root, &without(&["src"])), ["README.md", "web/dist/bundle.js"]);
        assert_eq!(
            files_of(root, &without(&["src/util"])),
            ["README.md", "src/app.ts", "src/main.rs", "web/dist/bundle.js", "web/src/index.ts"]
        );
        // Exclude wins over include.
        let both = SearchQuery { include: vec!["*.rs".into()], exclude: vec!["util".into()], ..literal("needle") };
        assert_eq!(files_of(root, &both), ["src/main.rs"]);
    }

    #[test]
    fn invalid_patterns_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let try_search = |query: &SearchQuery| search(tmp.path(), query, &cancel, &|_| {}).unwrap_err().code;
        assert_eq!(try_search(&regex("(unclosed")), WsErrorCode::InvalidPattern);
        assert_eq!(try_search(&literal("")), WsErrorCode::InvalidPattern);
        assert_eq!(try_search(&literal("a\nb")), WsErrorCode::InvalidPattern);
        assert_eq!(try_search(&regex(r"a\nb")), WsErrorCode::InvalidPattern);
        let bad_glob = SearchQuery { include: vec!["src/[".into()], ..literal("x") };
        assert_eq!(try_search(&bad_glob), WsErrorCode::InvalidPattern);
        // A literal pattern may contain regex special characters.
        search(tmp.path(), &literal("(unclosed"), &cancel, &|_| {}).unwrap();
    }

    #[test]
    fn respects_gitignore_and_skips_git_and_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join(".git/HEAD"), "needle\n");
        write(&root.join(".git/info/exclude"), "excluded.txt\n");
        write(&root.join(".gitignore"), "target/\n*.log\n");
        write(&root.join("src/.gitignore"), "generated/\n");
        write(&root.join("src/main.rs"), "needle\n");
        write(&root.join("src/generated/out.rs"), "needle\n");
        write(&root.join("target/debug/out.txt"), "needle\n");
        write(&root.join("debug.log"), "needle\n");
        write(&root.join("excluded.txt"), "needle\n");
        write(&root.join(".hidden/config"), "needle\n");
        write(&root.join("image.bin"), b"needle\0\x01\x02\nneedle\n");

        assert_eq!(files_of(root, &literal("needle")), [".hidden/config", "src/main.rs"]);
    }

    #[test]
    fn column_and_length_are_utf16_offsets() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // "é" = 2 byte / 1 unit; "😀" = 4 byte / 2 unit; "日本" = 6 byte / 2 unit.
        write(&root.join("a.txt"), "ascii\néé 😀 日本 target 😀 target\n");
        let (files, _) = run(root, &literal("target"));
        let matches = &files[0].matches;
        assert_eq!(matches.len(), 2);
        assert_eq!((matches[0].line, matches[0].column, matches[0].length), (2, 9, 6));
        assert_eq!((matches[1].line, matches[1].column, matches[1].length), (2, 19, 6));
        assert_eq!(matches[0].preview, "éé 😀 日本 target 😀 target");
        assert_eq!(matches[0].preview_offset, 9);
        let units: Vec<u16> = matches[1].preview.encode_utf16().collect();
        let start = matches[1].preview_offset as usize;
        assert_eq!(String::from_utf16(&units[start..start + matches[1].length as usize]).unwrap(), "target");

        // A match containing non-BMP characters: the length is in UTF-16 units.
        let (files, _) = run(root, &regex("😀 日"));
        assert_eq!((files[0].matches[0].column, files[0].matches[0].length), (3, 4));
    }

    #[test]
    fn long_lines_are_trimmed_around_the_match() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let line = format!("{}NEEDLE{}", "é".repeat(1000), "x".repeat(1000));
        write(&root.join("long.txt"), format!("{line}\nNEEDLE{}\n{}NEEDLE\n", "y".repeat(1000), "z".repeat(1000)));
        let (files, _) = run(root, &SearchQuery { case_sensitive: true, ..literal("NEEDLE") });
        let matches = &files[0].matches;

        let middle = &matches[0];
        assert_eq!((middle.column, middle.length), (1000, 6));
        assert_eq!(middle.preview.chars().count(), 400);
        // A quarter of the room for the text before the match.
        assert_eq!(middle.preview_offset, 98);
        assert!(middle.preview.starts_with("ééé"));
        assert_eq!(&middle.preview[98 * 2..98 * 2 + 6], "NEEDLE");
        assert!(middle.preview.ends_with("xxx"));

        let at_start = &matches[1];
        assert_eq!((at_start.column, at_start.preview_offset), (0, 0));
        assert_eq!(at_start.preview.chars().count(), 400);

        // A match at the end of the line: all the room is used for the preceding text.
        let at_end = &matches[2];
        assert_eq!((at_end.column, at_end.preview_offset), (1000, 394));
        assert_eq!(at_end.preview.chars().count(), 400);
        assert!(at_end.preview.ends_with("zNEEDLE"));

        // A match longer than the preview.
        let (files, _) = run(root, &regex("x+"));
        let huge = &files[0].matches[0];
        assert_eq!((huge.length, huge.preview_offset), (1000, 0));
        assert_eq!(huge.preview, "x".repeat(400));
    }

    #[test]
    fn searches_utf16_and_latin1_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut utf16 = vec![0xFF, 0xFE];
        for unit in "one\r\ntwo é needle\r\n".encode_utf16() {
            utf16.extend_from_slice(&unit.to_le_bytes());
        }
        write(&root.join("utf16.txt"), utf16);
        write(&root.join("bom.txt"), b"\xEF\xBB\xBFneedle at the start\n");
        write(&root.join("latin1.txt"), b"caf\xe9 needle\n");

        let (files, _) = run(root, &literal("needle"));
        let view: Vec<(&str, u32, u32)> = files.iter().map(|f| (f.path.as_str(), f.matches[0].line, f.matches[0].column)).collect();
        assert_eq!(view, [("bom.txt", 1, 0), ("latin1.txt", 1, 5), ("utf16.txt", 2, 6)]);
        assert_eq!(files[1].matches[0].preview, "caf\u{FFFD} needle");
        assert_eq!(files[2].matches[0].preview, "two é needle");
    }

    #[test]
    fn max_results_truncates() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for i in 0..20 {
            write(&root.join(format!("f{i}.txt")), "needle needle\nneedle\n");
        }
        let (files, summary) = run(root, &SearchQuery { max_results: 7, ..literal("needle") });
        assert_eq!(summary.matches, 7);
        assert!(summary.truncated);
        assert_eq!(files.iter().map(|f| f.matches.len()).sum::<usize>(), 7);
        assert_eq!(summary.files as usize, files.len());

        // Exactly as many as the limit: not truncated.
        let (_, summary) = run(root, &SearchQuery { max_results: 60, ..literal("needle") });
        assert_eq!(summary, SearchSummary { files: 20, matches: 60, truncated: false });
        let (_, summary) = run(root, &SearchQuery { max_results: 59, ..literal("needle") });
        assert_eq!((summary.matches, summary.truncated), (59, true));
        // Zero means unlimited.
        let (_, summary) = run(root, &literal("needle"));
        assert_eq!(summary, SearchSummary { files: 20, matches: 60, truncated: false });
    }

    #[test]
    fn cancel_stops_the_search() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for i in 0..400 {
            write(&root.join(format!("d{}/f{i}.txt", i % 20)), "needle\n");
        }

        let cancel = AtomicBool::new(true);
        let summary = search(root, &literal("needle"), &cancel, &|_| panic!("no results expected")).unwrap();
        assert_eq!(summary, SearchSummary { files: 0, matches: 0, truncated: true });

        // Canceled from the callback after the first result.
        let cancel = AtomicBool::new(false);
        let summary = search(root, &literal("needle"), &cancel, &|_| cancel.store(true, Ordering::Relaxed)).unwrap();
        assert!(summary.truncated);
        assert!(summary.files >= 1 && summary.files < 400, "{summary:?}");
    }

    #[test]
    fn query_deserializes_from_camel_case() {
        let query: SearchQuery = serde_json::from_value(serde_json::json!({
            "pattern": "x", "isRegex": true, "caseSensitive": false, "wholeWord": true,
            "include": ["*.rs"], "exclude": [], "maxResults": 100
        }))
        .unwrap();
        assert_eq!(query, SearchQuery { whole_word: true, include: vec!["*.rs".into()], max_results: 100, ..SearchQuery { case_sensitive: false, ..regex("x") } });

        let line = LineMatch { line: 1, column: 2, length: 3, preview: "p".into(), preview_offset: 2 };
        assert_eq!(
            serde_json::to_value(&line).unwrap(),
            serde_json::json!({ "line": 1, "column": 2, "length": 3, "preview": "p", "previewOffset": 2 })
        );
    }

    // ---- replace_in_files ----

    #[test]
    fn replace_literal() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("a.txt"), "foo(1) and Foo(2)\nnothing\nfoo(3)\r\n");
        write(&root.join("b.txt"), "none here\n");
        write(&root.join("sub/c.txt"), "foo(x)");

        let query = SearchQuery { case_sensitive: true, ..literal("foo(") };
        let files = ["a.txt".to_string(), "b.txt".to_string(), "sub/c.txt".to_string(), "missing.txt".to_string()];
        let summary = replace_in_files(root, &query, "$1bar[", &files).unwrap();
        assert_eq!(summary, ReplaceSummary { files: 2, replacements: 3 });
        // `$1` is not interpreted in literal mode; line endings are preserved.
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "$1bar[1) and Foo(2)\nnothing\n$1bar[3)\r\n");
        assert_eq!(fs::read_to_string(root.join("b.txt")).unwrap(), "none here\n");
        assert_eq!(fs::read_to_string(root.join("sub/c.txt")).unwrap(), "$1bar[x)");

        // Case-insensitivity and whole word follow the query.
        write(&root.join("d.txt"), "Cat cats CAT\n");
        let query = SearchQuery { whole_word: true, ..literal("cat") };
        let summary = replace_in_files(root, &query, "dog", &["d.txt".to_string()]).unwrap();
        assert_eq!(summary, ReplaceSummary { files: 1, replacements: 2 });
        assert_eq!(fs::read_to_string(root.join("d.txt")).unwrap(), "dog cats dog\n");
    }

    #[test]
    fn replace_regex_with_captures() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("a.rs"), "let alpha = 1;\nlet beta = 22;\r\nconst gamma = 3;\n");

        let query = regex(r"let (\w+) = (?P<val>\d+);");
        let summary = replace_in_files(root, &query, "const ${1}: u32 = $val; // $$", &["a.rs".to_string()]).unwrap();
        assert_eq!(summary, ReplaceSummary { files: 1, replacements: 2 });
        assert_eq!(
            fs::read_to_string(root.join("a.rs")).unwrap(),
            "const alpha: u32 = 1; // $\nconst beta: u32 = 22; // $\r\nconst gamma = 3;\n"
        );

        // Line anchors are the same as in the search; blank lines are untouched.
        write(&root.join("b.txt"), "a  \nb\n\nc \r\n");
        let summary = replace_in_files(root, &regex(r"\s+$"), "", &["b.txt".to_string()]).unwrap();
        assert_eq!(summary, ReplaceSummary { files: 1, replacements: 2 });
        assert_eq!(fs::read_to_string(root.join("b.txt")).unwrap(), "a\nb\n\nc\r\n");
    }

    #[test]
    fn replace_skips_non_utf8_and_keeps_bom() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("latin1.txt"), b"caf\xe9 foo\n");
        write(&root.join("binary.bin"), b"foo\0foo\n");
        write(&root.join("bom.txt"), b"\xEF\xBB\xBFfoo\n");
        let files: Vec<String> = ["latin1.txt", "binary.bin", "bom.txt"].iter().map(|s| s.to_string()).collect();

        let summary = replace_in_files(root, &regex("^foo"), "bar", &files).unwrap();
        assert_eq!(summary, ReplaceSummary { files: 1, replacements: 1 });
        assert_eq!(fs::read(root.join("latin1.txt")).unwrap(), b"caf\xe9 foo\n");
        assert_eq!(fs::read(root.join("binary.bin")).unwrap(), b"foo\0foo\n");
        assert_eq!(fs::read(root.join("bom.txt")).unwrap(), b"\xEF\xBB\xBFbar\n");
    }

    #[test]
    fn replace_rejects_paths_outside_root_before_touching_anything() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("proj");
        write(&root.join("a.txt"), "foo\n");
        write(&tmp.path().join("outside.txt"), "foo\n");

        let err = replace_in_files(&root, &literal("foo"), "bar", &["a.txt".to_string(), "../outside.txt".to_string()]).unwrap_err();
        assert_eq!(err.code, WsErrorCode::InvalidPath);
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "foo\n");
        assert_eq!(fs::read_to_string(tmp.path().join("outside.txt")).unwrap(), "foo\n");

        let absolute = tmp.path().join("outside.txt").to_string_lossy().into_owned();
        let err = replace_in_files(&root, &literal("foo"), "bar", &[absolute]).unwrap_err();
        assert_eq!(err.code, WsErrorCode::InvalidPath);
    }

    #[cfg(unix)]
    #[test]
    fn replace_keeps_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("run.sh"), "echo foo\n");
        fs::set_permissions(root.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        replace_in_files(root, &literal("foo"), "bar", &["run.sh".to_string()]).unwrap();
        assert_eq!(fs::read_to_string(root.join("run.sh")).unwrap(), "echo bar\n");
        assert_eq!(fs::metadata(root.join("run.sh")).unwrap().permissions().mode() & 0o777, 0o755);
    }
}
