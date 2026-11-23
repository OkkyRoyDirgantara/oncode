//! Pembantu kecil untuk path berkas repositori.

/// Path absolut sebuah berkas repositori, dengan pemisah milik OS.
pub fn repo_file(root: &str, path: &str) -> String {
    let separator = if root.contains('\\') && !root.contains('/') { "\\" } else { "/" };
    format!("{}{separator}{}", root.trim_end_matches(['/', '\\']), path.replace('/', separator))
}

/// Nama berkas dari sebuah path.
pub fn base_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_with_the_separator_of_the_root() {
        assert_eq!(repo_file("/p/app/", "src/a.rs"), "/p/app/src/a.rs");
        assert_eq!(repo_file("C:\\kode\\app", "src/a.rs"), "C:\\kode\\app\\src\\a.rs");
        assert_eq!(base_name("src/a.rs"), "a.rs");
        assert_eq!(base_name("a.rs"), "a.rs");
    }
}
