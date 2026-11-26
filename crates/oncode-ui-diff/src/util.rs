//! Small helpers for repository file paths.

/// Absolute path of a repository file, using the OS's separator.
pub fn repo_file(root: &str, path: &str) -> String {
    let separator = if root.contains('\\') && !root.contains('/') { "\\" } else { "/" };
    format!("{}{separator}{}", root.trim_end_matches(['/', '\\']), path.replace('/', separator))
}

/// File name from a path.
pub fn base_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_with_the_separator_of_the_root() {
        assert_eq!(repo_file("/p/app/", "src/a.rs"), "/p/app/src/a.rs");
        assert_eq!(repo_file("C:\\code\\app", "src/a.rs"), "C:\\code\\app\\src\\a.rs");
        assert_eq!(base_name("src/a.rs"), "a.rs");
        assert_eq!(base_name("a.rs"), "a.rs");
    }
}
