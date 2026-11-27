//! `.editorconfig` support (<https://editorconfig.org>): the properties that apply to a file, from the
//! `.editorconfig` files in its folder and the folders above it, up to one marked `root = true` or the
//! workspace root.
//!
//! Supported properties: `indent_style`, `indent_size`, `tab_width`, `end_of_line`, `charset`,
//! `trim_trailing_whitespace`, `insert_final_newline`. Section globs support `*`, `**`, `?`, `[…]`, and
//! `{a,b}` (numeric ranges `{1..3}` are not supported).

use std::path::Path;

use globset::GlobBuilder;
use serde::Serialize;

/// The properties that apply to one file; `None` = not set by any `.editorconfig`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorConfig {
    /// `true` for tabs, `false` for spaces.
    pub indent_with_tabs: Option<bool>,
    /// Spaces per indentation level.
    pub indent_size: Option<u32>,
    /// Width of a tab character.
    pub tab_width: Option<u32>,
    /// `"lf"` or `"crlf"` (`cr` alone is not supported and ignored).
    pub end_of_line: Option<String>,
    /// `utf-8`, `utf-8-bom`, `latin1`, `utf-16le`, or `utf-16be`.
    pub charset: Option<String>,
    pub trim_trailing_whitespace: Option<bool>,
    pub insert_final_newline: Option<bool>,
}

impl EditorConfig {
    /// The tab size to use: `indent_size`, or `tab_width` when indenting with tabs (or `indent_size = tab`).
    pub fn tab_size(&self) -> Option<u32> {
        match (self.indent_with_tabs, self.indent_size, self.tab_width) {
            (Some(true), _, Some(width)) => Some(width),
            (_, Some(size), _) => Some(size),
            (_, None, width) => width,
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// One parsed `.editorconfig` file.
#[derive(Debug, Default, PartialEq)]
struct ConfigFile {
    root: bool,
    /// Section glob and its properties (keys and values lowercased, as the spec says), in file order.
    sections: Vec<(String, Vec<(String, String)>)>,
}

fn parse(text: &str) -> ConfigFile {
    let mut file = ConfigFile::default();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            file.sections.push((section.to_string(), Vec::new()));
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim().to_lowercase(), value.trim().to_lowercase());
        match file.sections.last_mut() {
            Some((_, properties)) => properties.push((key, value)),
            // Before any section: only `root` means something.
            None if key == "root" => file.root = value == "true",
            None => {}
        }
    }
    file
}

/// Whether a section glob matches a file, given the file's path relative to the `.editorconfig`'s
/// folder with `/` separators. A glob without `/` matches the file name in any folder below.
fn section_matches(glob: &str, relative: &str) -> bool {
    let pattern = if glob.contains('/') { glob.trim_start_matches('/').to_string() } else { format!("**/{glob}") };
    // In EditorConfig `**` matches across folders anywhere (`docs/**.md`); globset only understands it as
    // a whole path component, so `**x` becomes `**/*x`.
    let pattern = expand_double_star(&pattern);
    GlobBuilder::new(&pattern).literal_separator(true).backslash_escape(true).build().is_ok_and(|glob| glob.compile_matcher().is_match(relative))
}

fn expand_double_star(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len() + 4);
    let mut rest = pattern;
    while let Some(at) = rest.find("**") {
        out.push_str(&rest[..at]);
        out.push_str("**");
        rest = &rest[at + 2..];
        if !rest.is_empty() && !rest.starts_with('/') {
            out.push_str("/*");
        }
    }
    out.push_str(rest);
    out
}

fn apply(config: &mut EditorConfig, key: &str, value: &str) {
    let number = || value.parse::<u32>().ok().filter(|n| (1..=32).contains(n));
    let flag = || match value {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    };
    // `unset` removes a value set by an earlier section or file.
    let unset = value == "unset";
    match key {
        "indent_style" => config.indent_with_tabs = if unset { None } else { matches!(value, "tab" | "space").then_some(value == "tab").or(config.indent_with_tabs) },
        "indent_size" if value == "tab" => config.indent_with_tabs = Some(true),
        "indent_size" => config.indent_size = if unset { None } else { number().or(config.indent_size) },
        "tab_width" => config.tab_width = if unset { None } else { number().or(config.tab_width) },
        "end_of_line" => config.end_of_line = if unset { None } else { matches!(value, "lf" | "crlf").then(|| value.to_string()).or(config.end_of_line.take()) },
        "charset" => {
            config.charset = if unset { None } else { matches!(value, "utf-8" | "utf-8-bom" | "latin1" | "utf-16le" | "utf-16be").then(|| value.to_string()).or(config.charset.take()) }
        }
        "trim_trailing_whitespace" => config.trim_trailing_whitespace = if unset { None } else { flag().or(config.trim_trailing_whitespace) },
        "insert_final_newline" => config.insert_final_newline = if unset { None } else { flag().or(config.insert_final_newline) },
        _ => {}
    }
}

/// The properties for `file`, reading `.editorconfig` files from its folder upwards. The search stops at
/// a file with `root = true`, at `stop` (the workspace root, if the file is inside it), or after 32 folders.
pub fn resolve(file: &Path, stop: Option<&Path>) -> EditorConfig {
    let mut found: Vec<(std::path::PathBuf, ConfigFile)> = Vec::new();
    let mut dir = file.parent();
    let stop = stop.filter(|stop| file.starts_with(stop));
    for _ in 0..32 {
        let Some(current) = dir else { break };
        if let Ok(text) = std::fs::read_to_string(current.join(".editorconfig")) {
            let parsed = parse(&text);
            let root = parsed.root;
            found.push((current.to_path_buf(), parsed));
            if root {
                break;
            }
        }
        if stop.is_some_and(|stop| current == stop) {
            break;
        }
        dir = current.parent();
    }
    let mut config = EditorConfig::default();
    // The file closest to the file wins, so the outermost is applied first.
    for (dir, parsed) in found.iter().rev() {
        let Ok(relative) = file.strip_prefix(dir) else { continue };
        let relative = relative.to_string_lossy().replace('\\', "/");
        for (glob, properties) in &parsed.sections {
            if section_matches(glob, &relative) {
                for (key, value) in properties {
                    apply(&mut config, key, value);
                }
            }
        }
    }
    config
}

/// The text after applying `trim_trailing_whitespace` and `insert_final_newline` (`false` removes final
/// newlines). `None` if nothing changes. `newline` is the file's line break.
pub fn apply_whitespace_rules(text: &str, config: &EditorConfig, newline: &str) -> Option<String> {
    let mut out = text.to_string();
    if config.trim_trailing_whitespace == Some(true) {
        let ends_with_newline = out.ends_with('\n');
        let mut trimmed = String::with_capacity(out.len());
        let mut lines = out.split('\n').peekable();
        while let Some(line) = lines.next() {
            let (body, cr) = match line.strip_suffix('\r') {
                Some(body) => (body, "\r"),
                None => (line, ""),
            };
            trimmed.push_str(body.trim_end_matches([' ', '\t']));
            trimmed.push_str(cr);
            if lines.peek().is_some() {
                trimmed.push('\n');
            }
        }
        debug_assert_eq!(trimmed.ends_with('\n'), ends_with_newline);
        out = trimmed;
    }
    match config.insert_final_newline {
        Some(true) if !out.is_empty() && !out.ends_with('\n') => out.push_str(newline),
        Some(false) => {
            while out.ends_with('\n') {
                out.pop();
                if out.ends_with('\r') {
                    out.pop();
                }
            }
        }
        _ => {}
    }
    (out != text).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in files {
            let full = dir.path().join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, text).unwrap();
        }
        dir
    }

    #[test]
    fn sections_apply_in_order_and_nested_files_win() {
        let dir = project(&[
            (".editorconfig", "root = true\n\n[*]\nindent_style = space\nindent_size = 4\nend_of_line = lf\ninsert_final_newline = true\n\n[*.go]\nindent_style = tab\ntab_width = 8\n\n[Makefile]\nindent_style = tab\n\n[docs/**.md]\ntrim_trailing_whitespace = false\n"),
            ("web/.editorconfig", "[*.{js,ts}]\nindent_size = 2\nTrim_Trailing_Whitespace = TRUE\n"),
        ]);
        let root = dir.path();
        let go = resolve(&root.join("cmd/main.go"), Some(root));
        assert_eq!((go.indent_with_tabs, go.tab_size()), (Some(true), Some(8)));
        let js = resolve(&root.join("web/src/app.ts"), Some(root));
        assert_eq!((js.indent_with_tabs, js.tab_size(), js.trim_trailing_whitespace), (Some(false), Some(2), Some(true)));
        assert_eq!(js.end_of_line.as_deref(), Some("lf"));
        let make = resolve(&root.join("Makefile"), Some(root));
        assert_eq!(make.indent_with_tabs, Some(true));
        let doc = resolve(&root.join("docs/guide/intro.md"), Some(root));
        assert_eq!(doc.trim_trailing_whitespace, Some(false));
        let py = resolve(&root.join("tool.py"), Some(root));
        assert_eq!((py.tab_size(), py.insert_final_newline), (Some(4), Some(true)));
    }

    #[test]
    fn root_stops_the_search_and_unset_removes_a_value() {
        let dir = project(&[(".editorconfig", "[*]\nindent_size = 8\ncharset = latin1\n"), ("app/.editorconfig", "root = true\n[*]\ncharset = unset\nindent_size = 3\n")]);
        let config = resolve(&dir.path().join("app/x.txt"), None);
        assert_eq!((config.indent_size, config.charset), (Some(3), None));
        // Without the inner file, the outer one applies.
        let outer = resolve(&dir.path().join("other/x.txt"), None);
        assert_eq!((outer.indent_size, outer.charset.as_deref()), (Some(8), Some("latin1")));
        assert_eq!(expand_double_star("docs/**.md"), "docs/**/*.md");
        assert_eq!(expand_double_star("**/x"), "**/x");
        assert_eq!(expand_double_star("a/**"), "a/**");
    }

    #[test]
    fn whitespace_rules() {
        let trim = EditorConfig { trim_trailing_whitespace: Some(true), insert_final_newline: Some(true), ..Default::default() };
        assert_eq!(apply_whitespace_rules("a  \nb\t\r\nc", &trim, "\n").as_deref(), Some("a\nb\r\nc\n"));
        assert_eq!(apply_whitespace_rules("a\n", &trim, "\n"), None);
        let no_final = EditorConfig { insert_final_newline: Some(false), ..Default::default() };
        assert_eq!(apply_whitespace_rules("a\r\n\r\n", &no_final, "\r\n").as_deref(), Some("a"));
        assert_eq!(apply_whitespace_rules("", &trim, "\n"), None);
    }
}
