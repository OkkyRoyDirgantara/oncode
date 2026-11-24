//! Formatter eksternal per bahasa: daftar kandidat dan pemilihan program yang terpasang.
//!
//! Semua formatter di sini membaca isi berkas dari stdin dan menulis hasilnya ke stdout, sehingga
//! isi yang belum disimpan pun bisa diformat. Murni (tanpa menjalankan proses); pencarian program
//! diberikan pemanggil agar bisa diuji.

use std::path::{Path, PathBuf};

/// Satu formatter. `{file}` di argumen diganti path berkas (untuk memilih parser dan membaca
/// konfigurasi proyek seperti `.prettierrc` atau `rustfmt.toml`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatterSpec {
    pub name: &'static str,
    pub program: &'static str,
    pub args: &'static [&'static str],
    /// Paket npm: dicari lebih dulu di `node_modules/.bin` proyek (versi milik proyek).
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

/// Kandidat formatter untuk sebuah bahasa (id bahasa LSP), urut dari yang diutamakan.
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

/// Formatter yang siap dijalankan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedFormatter {
    pub name: &'static str,
    pub program: PathBuf,
    pub args: Vec<String>,
}

/// Memilih formatter pertama yang terpasang untuk `language`. `find` mencari program (nama di
/// PATH, atau path lengkap) dan mengembalikan lokasinya bila ada. Paket npm dicari lebih dulu di
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

/// Petunjuk pemasangan untuk bahasa yang belum punya formatter terpasang.
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

    /// Pencari program tiruan: hanya program di PATH yang namanya persis ada di `installed`.
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
        let root = Path::new("/proyek");
        let file = Path::new("/proyek/src/app.ts");
        let local = resolve("typescript", file, root, |program: &str| program.contains("node_modules").then(|| PathBuf::from(program))).unwrap();
        assert_eq!(local.name, "Prettier");
        assert!(local.program.to_string_lossy().contains("node_modules"));
        assert_eq!(local.args, ["--stdin-filepath", "/proyek/src/app.ts"]);

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
