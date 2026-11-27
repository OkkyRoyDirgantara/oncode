//! Go to Symbol without a language server: the definitions in one file (an outline) and a symbol
//! search across the project, found with the same kind of text patterns as `symbols` (Go to
//! Definition without a language server). Approximate: it knows nothing about scope or types.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, OnceLock};

use regex::Regex;
use serde::Serialize;

use crate::error::Result;
use crate::search::{search, SearchQuery};

/// One definition found in a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutlineSymbol {
    pub name: String,
    /// What it is, from the keyword that defines it: `function`, `class`, `struct`, `method`, `heading`, …
    pub kind: String,
    /// Line number, starting from 1.
    pub line: u32,
    /// Start of the name on that line, in characters, starting from 0.
    pub column: u32,
    /// Nesting depth, from indentation (Markdown: heading level); 0 at the top.
    pub depth: usize,
}

/// A definition found by the project-wide search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSymbol {
    /// Path relative to the project root, with forward slashes.
    pub path: String,
    pub name: String,
    pub kind: String,
    pub line: u32,
    pub column: u32,
}

/// Modifiers that may come before a definition keyword.
const MODIFIERS: &str = r#"(?:(?:pub(?:\([^)]*\))?|export|default|declare|async|static|public|private|protected|internal|abstract|final|sealed|open|override|virtual|inline|unsafe|extern(?:\s+"[^"]*")?|data|suspend|partial|readonly)\s+)*"#;

/// Definition keywords and the kind each one shows.
const KEYWORDS: &[(&str, &str)] = &[
    ("fn", "function"),
    ("func", "function"),
    ("function", "function"),
    ("def", "function"),
    ("fun", "function"),
    ("sub", "function"),
    ("proc", "function"),
    ("macro_rules!", "macro"),
    ("class", "class"),
    ("struct", "struct"),
    ("enum", "enum"),
    ("union", "struct"),
    ("trait", "interface"),
    ("interface", "interface"),
    ("protocol", "interface"),
    ("type", "type"),
    ("typedef", "type"),
    ("impl", "impl"),
    ("mod", "module"),
    ("module", "module"),
    ("namespace", "module"),
    ("object", "class"),
    ("record", "class"),
    ("extension", "extension"),
    ("mixin", "class"),
];

/// Words that start a statement, never a method definition (`if (x) {`, `return foo(x)`).
const NOT_A_NAME: &[&str] = &[
    "if", "for", "while", "switch", "catch", "return", "new", "throw", "await", "else", "do", "try", "with", "elif", "match", "when", "function", "typeof", "sizeof", "assert", "yield",
];

fn keyword_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let keywords = KEYWORDS.iter().map(|(k, _)| regex::escape(k)).collect::<Vec<_>>().join("|");
        Regex::new(&format!(r"^\s*{MODIFIERS}(?P<kw>{keywords})(?:\s+|\s*<[^>]*>\s*)(?:[A-Za-z_$][\w$]*\s*\.\s*)?(?P<name>[A-Za-z_$][\w$]*)")).unwrap()
    })
}

/// `impl Trait for Type` / `impl Type` (Rust): the rest of the line up to `{` is the name.
fn impl_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*(?:unsafe\s+)?impl(?:\s*<[^>]*>)?\s+(?P<name>[^{;]+?)\s*(?:\{|where\b|$)").unwrap())
}

/// Go methods: `func (r *Server) Start(`.
fn go_method_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*func\s*\([^)]*\)\s*(?P<name>[A-Za-z_]\w*)").unwrap())
}

/// Functions assigned to a name: `const load = async (x) =>`, `let f = function`.
fn assigned_function_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*(?:export\s+)?(?:const|let|var)\s+(?P<name>[A-Za-z_$][\w$]*)\s*(?::[^=]+)?=\s*(?:async\s+)?(?:function\b|\([^)]*\)\s*(?::[^=]+)?=>|[A-Za-z_$][\w$]*\s*=>)").unwrap())
}

/// Methods inside a class, with or without a return type: `  save(name) {`, `  public void save(String n) {`,
/// `  Widget build(BuildContext c) {`, `  async load(): Promise<void> {`.
fn method_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            r"^\s+{MODIFIERS}(?:(?:get|set|synchronized)\s+)?(?:[\w<>\[\],.?]+\s+)?(?P<name>[A-Za-z_$][\w$]*)\s*(?:<[^>]*>)?\s*\([^;]*\)\s*(?::\s*[^{{=;]+)?(?:throws\s+[\w., ]+)?\{{\s*$"
        ))
        .unwrap()
    })
}

fn heading_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^(?P<level>#{1,6})\s+(?P<name>.+?)\s*#*\s*$").unwrap())
}

fn indent_width(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').map(|c| if c == '\t' { 4 } else { 1 }).sum()
}

fn char_column(line: &str, byte: usize) -> u32 {
    line[..byte].chars().count() as u32
}

/// The definition on one line, if any: name, kind, and the name's byte offset.
fn definition_on(line: &str) -> Option<(String, &'static str, usize)> {
    if let Some(found) = impl_regex().captures(line) {
        let name = found.name("name")?;
        return Some((name.as_str().trim().to_string(), "impl", name.start()));
    }
    if let Some(found) = go_method_regex().captures(line) {
        let name = found.name("name")?;
        return Some((name.as_str().to_string(), "method", name.start()));
    }
    if let Some(found) = keyword_regex().captures(line) {
        let keyword = found.name("kw")?.as_str();
        let name = found.name("name")?;
        let kind = KEYWORDS.iter().find(|(k, _)| *k == keyword).map_or("symbol", |(_, kind)| kind);
        return Some((name.as_str().to_string(), kind, name.start()));
    }
    if let Some(found) = assigned_function_regex().captures(line) {
        let name = found.name("name")?;
        return Some((name.as_str().to_string(), "function", name.start()));
    }
    if let Some(found) = method_regex().captures(line) {
        let name = found.name("name")?;
        if NOT_A_NAME.contains(&name.as_str()) {
            return None;
        }
        return Some((name.as_str().to_string(), "method", name.start()));
    }
    None
}

/// The definitions in a file's text, in order. `language` is the LSP language id (Markdown files list
/// their headings).
pub fn outline(text: &str, language: &str) -> Vec<OutlineSymbol> {
    let mut symbols = Vec::new();
    if language == "markdown" {
        let mut in_code = false;
        for (index, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
                in_code = !in_code;
                continue;
            }
            if in_code {
                continue;
            }
            if let Some(found) = heading_regex().captures(line) {
                let (level, name) = (found.name("level").unwrap(), found.name("name").unwrap());
                symbols.push(OutlineSymbol { name: name.as_str().to_string(), kind: "heading".into(), line: index as u32 + 1, column: char_column(line, name.start()), depth: level.as_str().len() - 1 });
            }
        }
        return symbols;
    }
    // Enclosing definitions by indentation: (indent, depth).
    let mut stack: Vec<usize> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        // Comments (`//`, `#`, `/* … */` and its `*` lines) and attributes (`#[…]`) define nothing.
        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with('*') || trimmed.starts_with("/*") {
            continue;
        }
        let Some((name, kind, at)) = definition_on(line) else { continue };
        let indent = indent_width(line);
        while stack.last().is_some_and(|&outer| outer >= indent) {
            stack.pop();
        }
        symbols.push(OutlineSymbol { name, kind: kind.into(), line: index as u32 + 1, column: char_column(line, at), depth: stack.len() });
        stack.push(indent);
    }
    symbols
}

/// Definitions across the project whose name contains `query` (ignoring case), at most `limit`,
/// those whose name starts with the query first.
pub fn find_workspace_symbols(root: &Path, query: &str, limit: usize) -> Result<Vec<WorkspaceSymbol>> {
    let query = query.trim();
    if query.is_empty() || !query.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$') {
        return Ok(Vec::new());
    }
    let q = regex::escape(query);
    let keywords = KEYWORDS.iter().map(|(k, _)| regex::escape(k)).collect::<Vec<_>>().join("|");
    let pattern = [
        format!(r"\b(?:{keywords})\s+(?:[A-Za-z_$][\w$]*\s*\.\s*)?[\w$]*{q}"),
        format!(r"\bfunc\s*\([^)]*\)\s*\w*{q}"),
        format!(r"\b(?:const|let|var)\s+[\w$]*{q}[\w$]*\s*(?::[^=]+)?=\s*(?:async\s+)?(?:function\b|\()"),
        format!(r"^\s+(?:[\w<>\[\],.?]+\s+)*[\w$]*{q}[\w$]*\s*\([^;]*\)\s*(?::\s*[^{{=;]+)?\{{\s*$"),
    ]
    .join("|");
    let search_query = SearchQuery { pattern, is_regex: true, case_sensitive: false, whole_word: false, include: Vec::new(), exclude: Vec::new(), max_results: 2000 };
    let lower = query.to_lowercase();
    let found: Mutex<Vec<WorkspaceSymbol>> = Mutex::new(Vec::new());
    search(root, &search_query, &AtomicBool::new(false), &|file| {
        let mut found = found.lock().unwrap();
        for line in file.matches {
            // `preview` may be trimmed at the front; the full line is rebuilt from the column offset for the regexes.
            let lead = line.column.saturating_sub(line.preview_offset) as usize;
            let text = format!("{}{}", " ".repeat(lead.min(200)), line.preview);
            let Some((name, kind, at)) = definition_on(&text) else { continue };
            if !name.to_lowercase().contains(&lower) {
                continue;
            }
            found.push(WorkspaceSymbol { path: file.path.clone(), name, kind: kind.into(), line: line.line, column: char_column(&text, at) });
        }
    })?;
    let mut found = found.into_inner().unwrap();
    found.sort_by(|a, b| {
        let rank = |s: &WorkspaceSymbol| (!s.name.to_lowercase().starts_with(&lower), s.name.len());
        rank(a).cmp(&rank(b)).then_with(|| a.path.cmp(&b.path)).then_with(|| a.line.cmp(&b.line))
    });
    found.truncate(limit);
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(text: &str, language: &str) -> Vec<(String, String, u32, usize)> {
        outline(text, language).into_iter().map(|s| (s.name, s.kind, s.line, s.depth)).collect()
    }

    fn item(name: &str, kind: &str, line: u32, depth: usize) -> (String, String, u32, usize) {
        (name.into(), kind.into(), line, depth)
    }

    #[test]
    fn rust_outline_with_nesting() {
        let text = "pub struct Point {\n    x: i32,\n}\n\nimpl Display for Point {\n    fn fmt(&self) {\n        if x {\n        }\n    }\n}\n\npub(crate) async fn main() {}\n";
        assert_eq!(
            names(text, "rust"),
            [item("Point", "struct", 1, 0), item("Display for Point", "impl", 5, 0), item("fmt", "function", 6, 1), item("main", "function", 12, 0)]
        );
        let symbols = outline(text, "rust");
        assert_eq!(symbols[2].column, 7, "the column of the name");
    }

    #[test]
    fn python_javascript_go_and_java() {
        let py = "class Repo:\n    def save(self):\n        return 1\n\ndef main():\n    pass\n";
        assert_eq!(names(py, "python"), [item("Repo", "class", 1, 0), item("save", "function", 2, 1), item("main", "function", 5, 0)]);

        let js = "export const load = async (url) => fetch(url);\nclass Store {\n  constructor() {\n  }\n  async get(key) {\n    if (key) {\n    }\n  }\n}\n";
        assert_eq!(names(js, "javascript"), [item("load", "function", 1, 0), item("Store", "class", 2, 0), item("constructor", "method", 3, 1), item("get", "method", 5, 1)]);

        let go = "type Server struct {\n}\n\nfunc (s *Server) Start(port int) error {\n\treturn nil\n}\n";
        assert_eq!(names(go, "go"), [item("Server", "type", 1, 0), item("Start", "method", 4, 0)]);

        let java = "public class Account {\n    public void save(String name) {\n        validate(name);\n    }\n}\n";
        assert_eq!(names(java, "java"), [item("Account", "class", 1, 0), item("save", "method", 2, 1)]);
    }

    #[test]
    fn markdown_headings_outside_code_blocks() {
        let md = "# Title\n\nText\n\n## Part one ##\n```sh\n# not a heading\n```\n### Detail\n";
        assert_eq!(names(md, "markdown"), [item("Title", "heading", 1, 0), item("Part one", "heading", 5, 1), item("Detail", "heading", 9, 2)]);
    }

    #[test]
    fn workspace_search_finds_definitions_by_part_of_the_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/user.rs"), "pub struct UserStore;\nfn load_user() {}\nfn main() { load_user(); }\n").unwrap();
        std::fs::write(dir.path().join("app.py"), "def user_name():\n    pass\n").unwrap();
        let found = find_workspace_symbols(dir.path(), "user", 10).unwrap();
        let names: Vec<(&str, &str, u32)> = found.iter().map(|s| (s.name.as_str(), s.path.as_str(), s.line)).collect();
        assert_eq!(names, [("user_name", "app.py", 1), ("UserStore", "src/user.rs", 1), ("load_user", "src/user.rs", 2)]);
        assert!(find_workspace_symbols(dir.path(), "a b", 10).unwrap().is_empty());
    }
}
