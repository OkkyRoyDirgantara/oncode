//! Indeks path berkas untuk quick open.

use std::path::Path;
use std::sync::mpsc;

use ignore::WalkState;
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use serde::Serialize;
use unicode_segmentation::UnicodeSegmentation;

use crate::gitignore::walk_builder;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickOpenItem {
    /// Path relatif terhadap akar proyek, dengan garis miring.
    pub path: String,
    /// Makin besar makin cocok; `0` untuk pola kosong.
    pub score: u32,
    /// Indeks karakter (code point Unicode) di `path` yang cocok dengan pola, urut naik;
    /// dipakai untuk penyorotan.
    pub indices: Vec<u32>,
}

/// Daftar path berkas proyek yang terurut, untuk pencocokan fuzzy.
#[derive(Debug, Clone, Default)]
pub struct FileIndex {
    /// Terurut menaik (per byte) dan tanpa duplikat.
    paths: Vec<String>,
}

impl FileIndex {
    /// Menelusuri `root` dengan menghormati gitignore; berkas tersembunyi ikut, `.git` tidak.
    /// Symlink ke berkas ikut dicatat tetapi symlink ke folder tidak ditelusuri. Memblokir.
    pub fn build(root: &Path) -> FileIndex {
        let root = &crate::fs::absolute(root).unwrap_or_else(|_| root.to_path_buf());
        let (tx, rx) = mpsc::channel::<String>();
        walk_builder(root).build_parallel().run(|| {
            let tx = tx.clone();
            Box::new(move |entry| {
                let Ok(entry) = entry else { return WalkState::Continue };
                let Some(file_type) = entry.file_type() else { return WalkState::Continue };
                if file_type.is_dir() {
                    return WalkState::Continue;
                }
                if file_type.is_symlink() && !std::fs::metadata(entry.path()).is_ok_and(|m| m.is_file()) {
                    return WalkState::Continue;
                }
                if let Ok(rel) = entry.path().strip_prefix(root) {
                    let _ = tx.send(to_slash(rel));
                }
                WalkState::Continue
            })
        });
        drop(tx);
        let mut paths: Vec<String> = rx.into_iter().filter(|p| !p.is_empty()).collect();
        paths.sort_unstable();
        paths.dedup();
        FileIndex { paths }
    }

    /// Jumlah berkas di dalam indeks.
    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// Semua path di dalam indeks, terurut menaik.
    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    /// Pencocokan fuzzy terhadap path relatif (smart case, penilaian sadar-path dari
    /// `nucleo-matcher`). Kata yang dipisah spasi harus cocok semuanya. Pola kosong
    /// mengembalikan `limit` path pertama menurut abjad.
    ///
    /// Hasil diurutkan menurut skor menurun, lalu path yang lebih pendek lebih dulu.
    pub fn query(&self, pattern: &str, limit: usize) -> Vec<QuickOpenItem> {
        if limit == 0 {
            return Vec::new();
        }
        let pattern = pattern.trim();
        if pattern.is_empty() {
            return self
                .paths
                .iter()
                .take(limit)
                .map(|path| QuickOpenItem { path: path.clone(), score: 0, indices: Vec::new() })
                .collect();
        }

        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let pattern = Pattern::new(pattern, CaseMatching::Smart, Normalization::Smart, AtomKind::Fuzzy);
        let mut buf = Vec::new();

        let mut scored: Vec<(u32, &String)> = Vec::new();
        for path in &self.paths {
            if let Some(score) = pattern.score(Utf32Str::new(path, &mut buf), &mut matcher) {
                scored.push((score, path));
            }
        }
        let order = |a: &(u32, &String), b: &(u32, &String)| {
            b.0.cmp(&a.0).then_with(|| a.1.len().cmp(&b.1.len())).then_with(|| a.1.cmp(b.1))
        };
        if scored.len() > limit {
            scored.select_nth_unstable_by(limit - 1, order);
            scored.truncate(limit);
        }
        scored.sort_unstable_by(order);

        // Indeks kecocokan hanya dihitung untuk hasil yang benar-benar dikembalikan.
        let mut raw = Vec::new();
        scored
            .into_iter()
            .map(|(score, path)| {
                raw.clear();
                pattern.indices(Utf32Str::new(path, &mut buf), &mut matcher, &mut raw);
                raw.sort_unstable();
                raw.dedup();
                QuickOpenItem { path: path.clone(), score, indices: char_indices(path, &raw) }
            })
            .collect()
    }

    /// Menambahkan satu berkas; tidak berpengaruh bila sudah ada.
    pub fn add(&mut self, rel_path: &str) {
        let path = clean(rel_path);
        if path.is_empty() {
            return;
        }
        if let Err(pos) = self.paths.binary_search(&path) {
            self.paths.insert(pos, path);
        }
    }

    /// Menghapus satu berkas; bila `rel_path` adalah folder, semua berkas di bawahnya ikut dihapus.
    pub fn remove(&mut self, rel_path: &str) {
        let path = clean(rel_path);
        if path.is_empty() {
            return;
        }
        if let Ok(pos) = self.paths.binary_search(&path) {
            self.paths.remove(pos);
        }
        let prefix = format!("{path}/");
        let start = self.paths.partition_point(|p| p.as_str() < prefix.as_str());
        let len = self.paths[start..].iter().take_while(|p| p.starts_with(&prefix)).count();
        self.paths.drain(start..start + len);
    }
}

/// `nucleo-matcher` mengindeks per gugus grafem untuk teks non-ASCII; ubah ke indeks code point.
fn char_indices(path: &str, grapheme_indices: &[u32]) -> Vec<u32> {
    if path.is_ascii() {
        return grapheme_indices.to_vec();
    }
    let mut starts = Vec::new();
    let mut chars = 0u32;
    for grapheme in path.graphemes(true) {
        starts.push(chars);
        chars += grapheme.chars().count() as u32;
    }
    grapheme_indices.iter().filter_map(|&i| starts.get(i as usize).copied()).collect()
}

/// Path relatif dengan garis miring sebagai pemisah.
pub(crate) fn to_slash(rel: &Path) -> String {
    let mut out = String::new();
    for component in rel.components() {
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(&component.as_os_str().to_string_lossy());
    }
    out
}

fn clean(rel_path: &str) -> String {
    let path = rel_path.replace('\\', "/");
    let path = path.trim_start_matches("./").trim_matches('/');
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn index_of(paths: &[&str]) -> FileIndex {
        let mut index = FileIndex::default();
        for path in paths {
            index.add(path);
        }
        index
    }

    fn paths(items: &[QuickOpenItem]) -> Vec<&str> {
        items.iter().map(|i| i.path.as_str()).collect()
    }

    #[test]
    fn build_respects_gitignore_and_skips_git() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join(".git/HEAD"), "ref: refs/heads/main\n");
        write(&root.join(".git/info/exclude"), "local-notes.md\n");
        write(&root.join(".gitignore"), "target/\n*.log\n");
        write(&root.join(".env"), "");
        write(&root.join("Cargo.toml"), "");
        write(&root.join("local-notes.md"), "");
        write(&root.join("debug.log"), "");
        write(&root.join("src/main.rs"), "");
        write(&root.join("src/util/mod.rs"), "");
        write(&root.join("src/util/.gitignore"), "generated.rs\n");
        write(&root.join("src/util/generated.rs"), "");
        write(&root.join("target/debug/app"), "");
        fs::create_dir_all(root.join("empty-dir")).unwrap();

        let index = FileIndex::build(root);
        assert_eq!(
            index.paths(),
            [".env", ".gitignore", "Cargo.toml", "src/main.rs", "src/util/.gitignore", "src/util/mod.rs"]
        );
        assert_eq!(index.len(), 6);
        assert!(!index.is_empty());
    }

    #[test]
    fn empty_pattern_lists_first_paths_alphabetically() {
        let index = index_of(&["src/main.rs", "Cargo.toml", "README.md", "src/lib.rs"]);
        let items = index.query("", 3);
        assert_eq!(paths(&items), ["Cargo.toml", "README.md", "src/lib.rs"]);
        assert!(items.iter().all(|i| i.score == 0 && i.indices.is_empty()));
        assert_eq!(index.query("  ", 10).len(), 4);
        assert!(index.query("", 0).is_empty());
    }

    #[test]
    fn fuzzy_query_finds_and_ranks() {
        let index = index_of(&[
            "src/main.rs",
            "src/domain/manager_settings.rs",
            "docs/manual.md",
            "src/lib.rs",
            "tests/main_test.rs",
            "Cargo.toml",
        ]);

        let items = index.query("mnrs", 10);
        assert_eq!(items[0].path, "src/main.rs");
        assert_eq!(items[0].indices, [4, 7, 9, 10]);
        assert!(items[0].score > 0);
        assert!(!paths(&items).contains(&"Cargo.toml"));
        assert!(!paths(&items).contains(&"docs/manual.md"));
        // Skor menurun.
        assert!(items.windows(2).all(|w| w[0].score >= w[1].score));

        let items = index.query("main", 10);
        assert_eq!(paths(&items)[..2], ["src/main.rs", "tests/main_test.rs"]);
        assert_eq!(items[0].indices, [4, 5, 6, 7]);

        // Kecocokan pada nama berkas mengalahkan kecocokan yang tersebar.
        let items = index.query("lib", 10);
        assert_eq!(items[0].path, "src/lib.rs");

        // Batas jumlah hasil mempertahankan yang terbaik.
        let top = index.query("rs", 2);
        assert_eq!(top.len(), 2);
        assert_eq!(paths(&top), paths(&index.query("rs", 10))[..2]);

        assert!(index.query("zzzz", 10).is_empty());
    }

    #[test]
    fn query_is_smart_case_and_space_separated() {
        let index = index_of(&["README.md", "src/readme_gen.rs", "docs/Intro.md"]);
        assert_eq!(index.query("readme", 10).len(), 2);
        // Huruf besar pada pola membuat pencocokan peka huruf.
        assert_eq!(paths(&index.query("README", 10)), ["README.md"]);
        // Dua kata: keduanya harus cocok.
        assert_eq!(paths(&index.query("src gen", 10)), ["src/readme_gen.rs"]);
        assert_eq!(paths(&index.query("docs/intro", 10)), ["docs/Intro.md"]);
    }

    #[test]
    fn equal_scores_prefer_shorter_paths() {
        let index = index_of(&["b/x.rs", "a/x.rs", "abc/x.rs"]);
        let items = index.query("x.rs", 10);
        assert_eq!(items[0].score, items[1].score);
        assert_eq!(paths(&items)[..2], ["a/x.rs", "b/x.rs"]);
    }

    #[test]
    fn indices_are_char_offsets_for_non_ascii_paths() {
        let index = index_of(&["dokumen/café/menu.txt"]);
        let items = index.query("menu", 10);
        assert_eq!(items.len(), 1);
        let chars: Vec<char> = items[0].path.chars().collect();
        let matched: String = items[0].indices.iter().map(|&i| chars[i as usize]).collect();
        assert_eq!(matched, "menu");
        assert_eq!(items[0].indices, [13, 14, 15, 16]);
    }

    #[test]
    fn add_and_remove_keep_the_index_sorted() {
        let mut index = index_of(&["src/b.rs", "src/a.rs"]);
        index.add("src/a.rs");
        index.add("./src/c.rs");
        index.add("win\\style\\path.rs");
        assert_eq!(index.paths(), ["src/a.rs", "src/b.rs", "src/c.rs", "win/style/path.rs"]);
        assert_eq!(paths(&index.query("path", 5)), ["win/style/path.rs"]);

        index.remove("src/b.rs");
        assert_eq!(index.paths(), ["src/a.rs", "src/c.rs", "win/style/path.rs"]);
        index.remove("tidak/ada.rs");
        assert_eq!(index.len(), 3);
    }

    #[test]
    fn remove_drops_everything_under_a_directory() {
        let mut index = index_of(&["src", "src-old/x.rs", "src.bak", "src/a.rs", "src/deep/b.rs", "srcx/y.rs", "z.rs"]);
        index.remove("src/");
        assert_eq!(index.paths(), ["src-old/x.rs", "src.bak", "srcx/y.rs", "z.rs"]);
        index.remove("srcx");
        assert_eq!(index.paths(), ["src-old/x.rs", "src.bak", "z.rs"]);
    }

    #[test]
    fn item_serializes_as_camel_case() {
        let item = QuickOpenItem { path: "a/b".into(), score: 7, indices: vec![0, 2] };
        assert_eq!(serde_json::to_value(&item).unwrap(), serde_json::json!({ "path": "a/b", "score": 7, "indices": [0, 2] }));
    }
}
