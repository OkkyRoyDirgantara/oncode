//! Go to Definition without a language server: finds where a symbol is defined across the whole
//! project using text patterns common to many languages (`fn`, `def`, `class`, `fun`, `func`,
//! `const x =`, C/Java/Dart/C#-style methods), then sorts the candidates by confidence.
//!
//! Not as precise as a language server (it knows nothing about scope or types), but it works for any
//! language and without installing anything.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use serde::Serialize;

use crate::error::Result;
use crate::search::{search, SearchQuery};

/// Keywords that start a definition in various languages.
const DEFINITION_KEYWORDS: &[&str] = &[
    "fn", "func", "function", "def", "class", "struct", "enum", "trait", "interface", "type", "typedef", "impl", "mod", "module", "namespace", "object", "record",
    "protocol", "extension", "mixin", "fun", "val", "var", "let", "const", "static", "macro_rules!", "union", "alias", "sub", "proc",
];

/// Words at the start of a line that indicate a usage, not a definition (e.g. `return foo(x)`).
const USE_PREFIXES: &[&str] = &[
    "return", "new", "throw", "await", "else", "if", "while", "for", "switch", "case", "catch", "yield", "print", "println", "echo", "assert", "delete",
    "typeof", "not", "and", "or", "in", "is", "do", "go", "defer",
];

/// A single definition candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Definition {
    /// Path relative to the project root, with forward slashes.
    pub path: String,
    /// Line number, starting from 1.
    pub line: u32,
    /// Start of the symbol name on that line, in UTF-16 units, starting from 0.
    pub column: u32,
    /// Length of the symbol name in UTF-16 units.
    pub length: u32,
    /// Contents of the line (trimmed).
    pub preview: String,
}

/// `true` if `symbol` is a name that can be searched for (letters, digits, `_`, `$`; not starting with a digit).
pub fn is_symbol(symbol: &str) -> bool {
    let mut chars = symbol.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$') && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

/// Regex that matches lines that are candidate definitions of `symbol`.
fn definition_pattern(symbol: &str) -> String {
    let name = symbol.replace('$', r"\$");
    let keywords = DEFINITION_KEYWORDS.join("|");
    [
        // `fn name`, `class Name`, `fun Type.name`, `const name`, …
        format!(r"\b(?:{keywords})\s+(?:[A-Za-z_$][\w$]*\s*\.\s*)?{name}\b"),
        // Go: `func (r *T) Name(`.
        format!(r"\bfunc\s*\([^)]*\)\s*{name}\b"),
        // `name = function`, `name: (x) =>`, `name = async (`.
        format!(r"\b{name}\s*[:=]\s*(?:async\s+)?(?:function\b|\()"),
        // C/Java/C#/Dart/Kotlin-style methods: `Type name(`, `Future<void> name(`.
        format!(r"[\w>\]?]\s+{name}\s*(?:<[^>]*>)?\s*\("),
        // JS/TS class methods without a keyword: `name(args) {`.
        format!(r"^\s*(?:async\s+)?{name}\s*\([^)]*\)\s*\{{"),
    ]
    .join("|")
}

/// Confidence that `line` defines `symbol`: 3 a definition keyword, 2 a function assignment,
/// 1 a method shape. `None` if the line looks like a usage.
fn score(line: &str, symbol: &str) -> Option<u8> {
    let trimmed = line.trim_start();
    let first_word: String = trimmed.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    if USE_PREFIXES.contains(&first_word.as_str()) {
        return None;
    }
    let before = &trimmed[..find_word(trimmed, symbol)?];
    let words: Vec<&str> = before.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '!')).filter(|w| !w.is_empty()).collect();
    if words.iter().any(|word| DEFINITION_KEYWORDS.contains(word)) {
        return Some(3);
    }
    let after = trimmed[find_word(trimmed, symbol)? + symbol.len()..].trim_start();
    if after.starts_with('=') && !after.starts_with("==") || after.starts_with(':') {
        return Some(2);
    }
    // Method shape: a parenthesis after the name, and the line does not end as a statement.
    let end = trimmed.trim_end();
    if after.starts_with('(') || after.starts_with('<') {
        if end.ends_with(';') || end.ends_with(',') {
            return None;
        }
        return Some(1);
    }
    None
}

/// Byte position of the first occurrence of `symbol` as a whole word in `text`.
fn find_word(text: &str, symbol: &str) -> Option<usize> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let mut from = 0;
    while let Some(found) = text[from..].find(symbol) {
        let start = from + found;
        let end = start + symbol.len();
        let before_ok = text[..start].chars().next_back().is_none_or(|c| !is_word(c));
        let after_ok = text[end..].chars().next().is_none_or(|c| !is_word(c));
        if before_ok && after_ok {
            return Some(start);
        }
        from = end;
    }
    None
}

fn utf16_len(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}

/// Finds definitions of `symbol` under `root`, at most `limit` candidates, ordered from the
/// most convincing. `current` (the relative path of the file currently open) is preferred when
/// equally confident, then files with the same extension.
pub fn find_definitions(root: &Path, symbol: &str, current: Option<&str>, limit: usize) -> Result<Vec<Definition>> {
    if !is_symbol(symbol) {
        return Ok(Vec::new());
    }
    let query = SearchQuery {
        pattern: definition_pattern(symbol),
        is_regex: true,
        case_sensitive: true,
        whole_word: false,
        include: Vec::new(),
        exclude: Vec::new(),
        max_results: 500,
    };
    let found: Mutex<Vec<(u8, Definition)>> = Mutex::new(Vec::new());
    search(root, &query, &AtomicBool::new(false), &|file| {
        let mut found = found.lock().unwrap();
        for line in file.matches {
            let Some(rank) = score(&line.preview, symbol) else { continue };
            let Some(start) = find_word(&line.preview, symbol) else { continue };
            // The column is counted from the start of the line: `preview` may be trimmed at the front.
            let lead = line.column.saturating_sub(line.preview_offset);
            let column = lead + utf16_len(&line.preview[..start]);
            found.push((rank, Definition { path: file.path.clone(), line: line.line, column, length: utf16_len(symbol), preview: line.preview.trim().to_string() }));
        }
    })?;
    let extension = |path: &str| Path::new(path).extension().map(|e| e.to_ascii_lowercase());
    let current_extension = current.and_then(extension);
    let mut found = found.into_inner().unwrap();
    found.sort_by(|(rank_a, a), (rank_b, b)| {
        let place = |d: &Definition| (Some(d.path.as_str()) != current, extension(&d.path) != current_extension);
        rank_b.cmp(rank_a).then_with(|| place(a).cmp(&place(b))).then_with(|| a.path.cmp(&b.path)).then_with(|| a.line.cmp(&b.line))
    });
    Ok(found.into_iter().map(|(_, definition)| definition).take(limit).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in files {
            let full = dir.path().join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, text).unwrap();
        }
        dir
    }

    fn first(root: &Path, symbol: &str, current: Option<&str>) -> Option<(String, u32, u32)> {
        find_definitions(root, symbol, current, 10).unwrap().into_iter().next().map(|d| (d.path, d.line, d.column))
    }

    #[test]
    fn finds_definitions_across_languages_and_skips_call_sites() {
        let dir = project(&[
            ("src/main.rs", "fn main() {\n    let total = count(2);\n    println!(\"{}\", count(3));\n}\n"),
            ("src/math.rs", "pub fn count(x: u32) -> u32 {\n    x * 2\n}\n"),
            ("app/util.py", "import os\n\ndef normalize(text):\n    return text.strip()\n\nresult = normalize(' a ')\n"),
            ("web/api.ts", "export const fetchData = async (url: string) => fetch(url);\nfetchData('/x');\n"),
            ("lib/home.dart", "class HomePage extends StatelessWidget {\n  Widget build(BuildContext context) {\n    return Text('Hello');\n  }\n}\n"),
            ("android/Main.kt", "fun String.greet(): String = \"Hello $this\"\n"),
            ("go/server.go", "func (s *Server) Start(port int) error {\n\treturn nil\n}\n"),
            ("java/Account.java", "public class Account {\n    public void save(String name) {\n        validate(name);\n    }\n}\n"),
        ]);
        let root = dir.path();
        assert_eq!(first(root, "count", Some("src/main.rs")), Some(("src/math.rs".into(), 1, 7)));
        assert_eq!(first(root, "normalize", None), Some(("app/util.py".into(), 3, 4)));
        assert_eq!(first(root, "fetchData", None), Some(("web/api.ts".into(), 1, 13)));
        assert_eq!(first(root, "HomePage", None), Some(("lib/home.dart".into(), 1, 6)));
        assert_eq!(first(root, "build", None), Some(("lib/home.dart".into(), 2, 9)));
        assert_eq!(first(root, "greet", None), Some(("android/Main.kt".into(), 1, 11)));
        assert_eq!(first(root, "Start", None), Some(("go/server.go".into(), 1, 17)));
        assert_eq!(first(root, "save", None), Some(("java/Account.java".into(), 2, 16)));
        // Only called, not defined in the project.
        assert_eq!(first(root, "validate", None), None);
        assert_eq!(first(root, "println", None), None);
    }

    #[test]
    fn keyword_definitions_rank_first_and_the_current_file_wins_ties() {
        let dir = project(&[
            ("a.ts", "function process() {}\n"),
            ("b.ts", "const result = process();\nfunction process() {}\n"),
            ("c.py", "def process():\n    pass\n"),
        ]);
        let all = find_definitions(dir.path(), "process", Some("b.ts"), 10).unwrap();
        let paths: Vec<&str> = all.iter().map(|d| d.path.as_str()).collect();
        assert_eq!(paths, ["b.ts", "a.ts", "c.py"], "current file, then the same extension");
        assert_eq!(all[0].line, 2);
    }

    #[test]
    fn rejects_non_symbols_and_handles_unicode_columns() {
        let dir = project(&[("x.js", "const é = 1; function hëllo() {}\n")]);
        assert!(find_definitions(dir.path(), "a b", None, 10).unwrap().is_empty());
        assert!(find_definitions(dir.path(), "1abc", None, 10).unwrap().is_empty());
        assert_eq!(first(dir.path(), "hëllo", None), Some(("x.js".into(), 1, 22)));
        assert!(is_symbol("$state") && is_symbol("_x1") && !is_symbol("") && !is_symbol("a-b"));
    }
}
