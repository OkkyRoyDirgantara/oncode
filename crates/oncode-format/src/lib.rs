//! External per-language formatters: the list of candidates and picking the installed program.
//!
//! Every formatter here reads the file contents from stdin and writes the result to stdout, so
//! unsaved content can be formatted too. Pure (no process is spawned); the program lookup is
//! supplied by the caller so it can be tested.

use std::path::{Path, PathBuf};

/// One formatter. `{file}` in the arguments is replaced with the file path (to pick the parser and
/// read project configuration such as `.prettierrc` or `rustfmt.toml`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatterSpec {
    pub name: &'static str,
    pub program: &'static str,
    pub args: &'static [&'static str],
    /// npm package: looked up first in the project's `node_modules/.bin` (the project's own version).
    pub node_package: bool,
}

const fn spec(name: &'static str, program: &'static str, args: &'static [&'static str]) -> FormatterSpec {
    FormatterSpec { name, program, args, node_package: false }
}

const fn npm(name: &'static str, program: &'static str, args: &'static [&'static str]) -> FormatterSpec {
    FormatterSpec { name, program, args, node_package: true }
}

const PRETTIER: FormatterSpec = npm("Prettier", "prettier", &["--stdin-filepath", "{file}"]);
const RUSTFMT: FormatterSpec = spec("rustfmt", "rustfmt", &["--edition", "2021", "--emit", "stdout", "--quiet"]);
const GOFMT: FormatterSpec = spec("gofmt", "gofmt", &[]);
const RUFF: FormatterSpec = spec("Ruff", "ruff", &["format", "--stdin-filename", "{file}", "-"]);
const BLACK: FormatterSpec = spec("Black", "black", &["--quiet", "--stdin-filename", "{file}", "-"]);
const DART: FormatterSpec = spec("dart format", "dart", &["format", "--stdin-name", "{file}"]);
const KTLINT: FormatterSpec = spec("ktlint", "ktlint", &["--format", "--stdin", "--log-level=none"]);
const SQL_FORMATTER: FormatterSpec = npm("sql-formatter", "sql-formatter", &[]);
const CLANG_FORMAT: FormatterSpec = spec("clang-format", "clang-format", &["--assume-filename={file}"]);
const SHFMT: FormatterSpec = spec("shfmt", "shfmt", &["-filename", "{file}"]);
const STYLUA: FormatterSpec = spec("StyLua", "stylua", &["--stdin-filepath", "{file}", "-"]);
const TAPLO: FormatterSpec = spec("Taplo", "taplo", &["fmt", "-"]);
const SWIFT_FORMAT: FormatterSpec = spec("swift-format", "swift-format", &[]);

/// Formatter candidates for a language (LSP language id), in order of preference.
pub fn formatters_for(language: &str) -> &'static [FormatterSpec] {
    match language {
        "javascript" | "javascriptreact" | "typescript" | "typescriptreact" | "json" | "css" | "scss" | "less" | "html" | "markdown" | "yaml" | "graphql"
        | "vue" => &[PRETTIER],
        "rust" => &[RUSTFMT],
        "go" => &[GOFMT],
        "python" => &[RUFF, BLACK],
        "dart" => &[DART],
        "kotlin" => &[KTLINT],
        "sql" => &[SQL_FORMATTER],
        "c" | "cpp" | "java" | "csharp" | "proto" => &[CLANG_FORMAT],
        "shellscript" => &[SHFMT],
        "lua" => &[STYLUA],
        "toml" => &[TAPLO],
        "swift" => &[SWIFT_FORMAT],
        _ => &[],
    }
}

/// A formatter that is ready to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedFormatter {
    pub name: &'static str,
    pub program: PathBuf,
    pub args: Vec<String>,
}

/// Picks the first installed formatter for `language`. `find` looks up a program (a name on
/// PATH, or a full path) and returns its location if present. npm packages are looked up first in
/// `<root>/node_modules/.bin`.
pub fn resolve(language: &str, file: &Path, root: &Path, find: impl Fn(&str) -> Option<PathBuf>) -> Option<ResolvedFormatter> {
    formatters_for(language).iter().find_map(|spec| {
        let local = spec.node_package.then(|| root.join("node_modules").join(".bin").join(spec.program)).and_then(|path| find(&path.to_string_lossy()));
        let program = local.or_else(|| find(spec.program))?;
        let file = file.to_string_lossy();
        let args = spec.args.iter().map(|arg| arg.replace("{file}", &file)).collect();
        Some(ResolvedFormatter { name: spec.name, program, args })
    })
}

/// Installation hint for a language that has no formatter installed.
pub fn install_hint(language: &str) -> Option<&'static str> {
    let first = formatters_for(language).first()?;
    Some(match first.program {
        "prettier" => "Install Prettier with `npm install --save-dev prettier` in the project (or `npm install -g prettier`).",
        "rustfmt" => "Install rustfmt with `rustup component add rustfmt`.",
        "gofmt" => "gofmt comes with the Go toolchain.",
        "ruff" => "Install Ruff with `pip install ruff` (or Black with `pip install black`).",
        "dart" => "dart format comes with the Dart or Flutter SDK.",
        "ktlint" => "Install ktlint from github.com/pinterest/ktlint (e.g. `brew install ktlint`).",
        "sql-formatter" => "Install sql-formatter with `npm install -g sql-formatter`.",
        "clang-format" => "Install clang-format (part of LLVM), e.g. `brew install clang-format`.",
        "shfmt" => "Install shfmt, e.g. `brew install shfmt` or `go install mvdan.cc/sh/v3/cmd/shfmt@latest`.",
        "stylua" => "Install StyLua with `cargo install stylua`.",
        "taplo" => "Install Taplo with `cargo install taplo-cli`.",
        "swift-format" => "Install swift-format (it comes with recent Swift toolchains).",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fake program finder: only programs on PATH whose exact name is in `installed` are found.
    fn finder(installed: &'static [&'static str]) -> impl Fn(&str) -> Option<PathBuf> {
        move |program: &str| installed.contains(&program).then(|| PathBuf::from(program))
    }

    #[test]
    fn every_language_the_user_asked_for_has_a_formatter() {
        for language in ["javascript", "typescriptreact", "json", "css", "html", "markdown", "yaml", "rust", "go", "python", "dart", "kotlin", "sql", "java", "c", "csharp", "lua"] {
            assert!(!formatters_for(language).is_empty(), "{language}");
            assert!(install_hint(language).is_some(), "{language}");
        }
        assert!(formatters_for("plaintext").is_empty());
    }

    #[test]
    fn the_projects_own_prettier_wins_over_a_global_one() {
        let root = Path::new("/project");
        let file = Path::new("/project/src/app.ts");
        let local = resolve("typescript", file, root, |program: &str| program.contains("node_modules").then(|| PathBuf::from(program))).unwrap();
        assert_eq!(local.name, "Prettier");
        assert!(local.program.to_string_lossy().contains("node_modules"));
        assert_eq!(local.args, ["--stdin-filepath", "/project/src/app.ts"]);

        let global = resolve("typescript", file, root, finder(&["prettier"])).unwrap();
        assert_eq!(global.program, PathBuf::from("prettier"));
    }

    #[test]
    fn falls_back_to_the_next_installed_formatter() {
        let resolved = resolve("python", Path::new("/p/a.py"), Path::new("/p"), finder(&["black"])).unwrap();
        assert_eq!((resolved.name, resolved.args.as_slice()), ("Black", &["--quiet".to_string(), "--stdin-filename".into(), "/p/a.py".into(), "-".into()][..]));
        assert_eq!(resolve("python", Path::new("/p/a.py"), Path::new("/p"), finder(&[])), None);
        assert_eq!(resolve("plaintext", Path::new("/p/a.txt"), Path::new("/p"), finder(&["prettier"])), None);
    }

    #[test]
    fn file_placeholder_is_filled_inside_arguments() {
        let resolved = resolve("cpp", Path::new("/p/main.cpp"), Path::new("/p"), finder(&["clang-format"])).unwrap();
        assert_eq!(resolved.args, ["--assume-filename=/p/main.cpp"]);
    }
}
