//! Go to Definition tanpa language server: mencari tempat sebuah simbol didefinisikan di seluruh
//! proyek dengan pola teks yang umum di banyak bahasa (`fn`, `def`, `class`, `fun`, `func`,
//! `const x =`, method gaya C/Java/Dart/C#), lalu mengurutkan kandidat menurut keyakinannya.
//!
//! Tidak sepresisi language server (tidak tahu scope atau tipe), tetapi berjalan untuk bahasa apa
//! pun dan tanpa memasang apa-apa.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use serde::Serialize;

use crate::error::Result;
use crate::search::{search, SearchQuery};

/// Kata kunci yang mengawali definisi di berbagai bahasa.
const DEFINITION_KEYWORDS: &[&str] = &[
    "fn", "func", "function", "def", "class", "struct", "enum", "trait", "interface", "type", "typedef", "impl", "mod", "module", "namespace", "object", "record",
    "protocol", "extension", "mixin", "fun", "val", "var", "let", "const", "static", "macro_rules!", "union", "alias", "sub", "proc",
];

/// Kata di awal baris yang menandakan pemakaian, bukan definisi (mis. `return foo(x)`).
const USE_PREFIXES: &[&str] = &[
    "return", "new", "throw", "await", "else", "if", "while", "for", "switch", "case", "catch", "yield", "print", "println", "echo", "assert", "delete",
    "typeof", "not", "and", "or", "in", "is", "do", "go", "defer",
];

/// Satu kandidat definisi.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Definition {
    /// Path relatif terhadap akar proyek, dengan garis miring.
    pub path: String,
    /// Nomor baris, mulai dari 1.
    pub line: u32,
    /// Awal nama simbol di baris itu, dalam unit UTF-16, mulai dari 0.
    pub column: u32,
    /// Panjang nama simbol dalam unit UTF-16.
    pub length: u32,
    /// Isi baris (dipangkas).
    pub preview: String,
}

/// `true` bila `symbol` adalah nama yang bisa dicari (huruf, angka, `_`, `$`; tidak diawali angka).
pub fn is_symbol(symbol: &str) -> bool {
    let mut chars = symbol.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$') && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

/// Regex yang mencocokkan baris-baris calon definisi `symbol`.
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
        // Method gaya C/Java/C#/Dart/Kotlin: `Type name(`, `Future<void> name(`.
        format!(r"[\w>\]?]\s+{name}\s*(?:<[^>]*>)?\s*\("),
        // Method kelas JS/TS tanpa kata kunci: `name(args) {`.
        format!(r"^\s*(?:async\s+)?{name}\s*\([^)]*\)\s*\{{"),
    ]
    .join("|")
}

/// Keyakinan bahwa `line` mendefinisikan `symbol`: 3 kata kunci definisi, 2 penugasan fungsi,
/// 1 bentuk method. `None` bila baris itu tampak seperti pemakaian.
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
    // Bentuk method: tanda kurung setelah nama, dan baris tidak berakhir sebagai pernyataan.
    let end = trimmed.trim_end();
    if after.starts_with('(') || after.starts_with('<') {
        if end.ends_with(';') || end.ends_with(',') {
            return None;
        }
        return Some(1);
    }
    None
}

/// Posisi byte kemunculan `symbol` sebagai kata utuh pertama di `text`.
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

/// Mencari definisi `symbol` di bawah `root`, paling banyak `limit` kandidat, urut dari yang
/// paling meyakinkan. `current` (path relatif berkas yang sedang dibuka) didahulukan bila sama
/// yakinnya, lalu berkas berekstensi sama.
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
            // Kolom dihitung dari awal baris: `preview` bisa dipangkas di depan.
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
            ("src/main.rs", "fn main() {\n    let total = hitung(2);\n    println!(\"{}\", hitung(3));\n}\n"),
            ("src/math.rs", "pub fn hitung(x: u32) -> u32 {\n    x * 2\n}\n"),
            ("app/util.py", "import os\n\ndef bersihkan(teks):\n    return teks.strip()\n\nhasil = bersihkan(' a ')\n"),
            ("web/api.ts", "export const ambilData = async (url: string) => fetch(url);\nambilData('/x');\n"),
            ("lib/home.dart", "class HomePage extends StatelessWidget {\n  Widget build(BuildContext context) {\n    return Text('Halo');\n  }\n}\n"),
            ("android/Main.kt", "fun String.sapa(): String = \"Halo $this\"\n"),
            ("go/server.go", "func (s *Server) Mulai(port int) error {\n\treturn nil\n}\n"),
            ("java/Akun.java", "public class Akun {\n    public void simpan(String nama) {\n        validasi(nama);\n    }\n}\n"),
        ]);
        let root = dir.path();
        assert_eq!(first(root, "hitung", Some("src/main.rs")), Some(("src/math.rs".into(), 1, 7)));
        assert_eq!(first(root, "bersihkan", None), Some(("app/util.py".into(), 3, 4)));
        assert_eq!(first(root, "ambilData", None), Some(("web/api.ts".into(), 1, 13)));
        assert_eq!(first(root, "HomePage", None), Some(("lib/home.dart".into(), 1, 6)));
        assert_eq!(first(root, "build", None), Some(("lib/home.dart".into(), 2, 9)));
        assert_eq!(first(root, "sapa", None), Some(("android/Main.kt".into(), 1, 11)));
        assert_eq!(first(root, "Mulai", None), Some(("go/server.go".into(), 1, 17)));
        assert_eq!(first(root, "simpan", None), Some(("java/Akun.java".into(), 2, 16)));
        // Hanya dipanggil, tidak didefinisikan di proyek.
        assert_eq!(first(root, "validasi", None), None);
        assert_eq!(first(root, "println", None), None);
    }

    #[test]
    fn keyword_definitions_rank_first_and_the_current_file_wins_ties() {
        let dir = project(&[
            ("a.ts", "function proses() {}\n"),
            ("b.ts", "const hasil = proses();\nfunction proses() {}\n"),
            ("c.py", "def proses():\n    pass\n"),
        ]);
        let all = find_definitions(dir.path(), "proses", Some("b.ts"), 10).unwrap();
        let paths: Vec<&str> = all.iter().map(|d| d.path.as_str()).collect();
        assert_eq!(paths, ["b.ts", "a.ts", "c.py"], "berkas aktif, lalu ekstensi yang sama");
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
