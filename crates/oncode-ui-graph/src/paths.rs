//! Pemetaan path absolut editor ke path relatif repositori, dan nama folder worktree.

use oncode_core::AppError;

fn is_windows_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/')) || path.starts_with("\\\\")
}

/// Path `absolute` relatif terhadap akar repositori, dengan garis miring biasa.
/// Mengembalikan `None` bila berkas berada di luar repositori (atau adalah akarnya sendiri).
pub fn repo_relative_path(root: &str, absolute: &str) -> Option<String> {
    let windows = is_windows_path(root);
    let base = root.replace('\\', "/").trim_end_matches('/').to_string();
    let full = absolute.replace('\\', "/");
    // Sistem berkas Windows tidak membedakan huruf besar dan kecil.
    let (prefix, candidate) = if windows { (base.to_lowercase(), full.to_lowercase()) } else { (base.clone(), full.clone()) };
    if !candidate.starts_with(&format!("{prefix}/")) {
        return None;
    }
    let relative = full.get(base.len() + 1..)?.trim_start_matches('/');
    (!relative.is_empty()).then(|| relative.to_string())
}

/// Repositori yang memuat `absolute` (yang akarnya paling dalam), beserta path relatifnya.
pub fn locate_in_repo<'a>(roots: impl IntoIterator<Item = &'a str>, absolute: &str) -> Option<(String, String)> {
    let mut best: Option<(String, String)> = None;
    for root in roots {
        if let Some(path) = repo_relative_path(root, absolute) {
            if best.as_ref().is_none_or(|(current, _)| root.len() > current.len()) {
                best = Some((root.to_string(), path));
            }
        }
    }
    best
}

/// Menggabungkan folder dan nama memakai pemisah yang dipakai folder itu.
pub fn join_path(dir: &str, name: &str) -> String {
    let sep = if dir.contains('\\') && !dir.contains('/') { '\\' } else { '/' };
    format!("{}{sep}{name}", dir.trim_end_matches(['/', '\\']))
}

/// Path absolut sebuah berkas repositori, dengan pemisah milik akar repositori.
pub fn repo_file(root: &str, path: &str) -> String {
    let sep = if root.contains('\\') && !root.contains('/') { "\\" } else { "/" };
    format!("{}{sep}{}", root.trim_end_matches(['/', '\\']), path.replace('/', sep))
}

/// Nama folder bawaan untuk worktree baru: `<folder-repo>-<branch>` tanpa garis miring.
pub fn default_worktree_name(root: &str, branch: &str) -> String {
    let flat: Vec<&str> = branch.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    format!("{}-{}", crate::format::base_name(root), flat.join("-"))
}

/// Galat `git worktree remove` karena worktree masih berisi perubahan atau berkas untracked.
pub fn is_dirty_worktree_error(error: &AppError) -> bool {
    if error.code == "dirtyWorkTree" {
        return true;
    }
    let text = format!("{} {}", error.message, error.detail.as_deref().unwrap_or("")).to_lowercase();
    ["modified or untracked", "contains modified", "untracked files", "use --force"].iter().any(|needle| text.contains(needle))
}

/// Galat untuk nama folder yang tidak sah, atau `None`.
pub fn validate_folder_name(value: &str) -> Option<&'static str> {
    let name = value.trim();
    if name.is_empty() {
        return Some("Enter a folder name");
    }
    if name.contains(['/', '\\']) {
        return Some("Enter a single folder name, not a path");
    }
    if name == "." || name == ".." {
        return Some("This is not a valid folder name");
    }
    None
}

/// Aturan dasar nama ref Git, agar galat muncul sebelum perintah dijalankan.
pub fn validate_ref_name(value: &str) -> Option<&'static str> {
    let name = value.trim();
    if name.is_empty() {
        return Some("Enter a name");
    }
    if name.starts_with('-') {
        return Some("Names cannot start with a dash");
    }
    let bad_char = name.chars().any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c));
    let bad_shape = name.contains("..") || name.contains("@{") || name.contains("//") || name.starts_with('/') || name.ends_with('/') || name.ends_with('.') || name.ends_with(".lock");
    if bad_char || bad_shape {
        return Some("This is not a valid Git ref name");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_relative_path_maps_files_inside_the_repository() {
        let cases = [
            ("/work/repo", "/work/repo/src/main.rs", "src/main.rs"),
            ("/work/repo/", "/work/repo/README.md", "README.md"),
            ("/work/repo", "/work/repo/a/b/c.txt", "a/b/c.txt"),
            ("C:\\work\\repo", "C:\\work\\repo\\src\\app.ts", "src/app.ts"),
            // Windows tidak membedakan huruf besar/kecil; huruf pada hasil mengikuti path berkas.
            ("C:\\Work\\Repo", "c:\\work\\repo\\Src\\App.ts", "Src/App.ts"),
            ("C:/work/repo", "C:\\work\\repo\\x.ts", "x.ts"),
        ];
        for (root, absolute, expected) in cases {
            assert_eq!(repo_relative_path(root, absolute).as_deref(), Some(expected), "{root} + {absolute}");
        }
    }

    #[test]
    fn repo_relative_path_rejects_files_outside() {
        let cases = [
            ("/work/repo", "/work/other/file.ts"),
            // Awalan nama yang sama bukan berarti berada di dalam repositori.
            ("/work/repo", "/work/repo-backup/file.ts"),
            ("/work/repo", "/work/repo"),
            ("/work/repo", "/work/repo/"),
            ("/work/repo", "relative/file.ts"),
            // Di Unix huruf besar/kecil berbeda berarti folder lain.
            ("/work/repo", "/Work/Repo/file.ts"),
        ];
        for (root, absolute) in cases {
            assert_eq!(repo_relative_path(root, absolute), None, "{root} + {absolute}");
        }
    }

    #[test]
    fn locate_prefers_the_deepest_repository() {
        let roots = ["/work/repo", "/work/repo/vendor/lib"];
        assert_eq!(locate_in_repo(roots, "/work/repo/vendor/lib/src/a.c"), Some(("/work/repo/vendor/lib".into(), "src/a.c".into())));
        assert_eq!(locate_in_repo(roots, "/work/repo/src/a.c"), Some(("/work/repo".into(), "src/a.c".into())));
        assert_eq!(locate_in_repo(roots, "/elsewhere/a.c"), None);
    }

    #[test]
    fn join_path_uses_the_separator_of_the_parent() {
        assert_eq!(join_path("/home/ana/code", "repo-feature"), "/home/ana/code/repo-feature");
        assert_eq!(join_path("/home/ana/code/", "repo-feature"), "/home/ana/code/repo-feature");
        assert_eq!(join_path("C:\\code", "repo-feature"), "C:\\code\\repo-feature");
        assert_eq!(repo_file("/work/repo/", "src/a.rs"), "/work/repo/src/a.rs");
        assert_eq!(repo_file("C:\\work\\repo", "src/a.rs"), "C:\\work\\repo\\src\\a.rs");
    }

    #[test]
    fn default_worktree_name_flattens_slashes() {
        assert_eq!(default_worktree_name("/work/oncode", "main"), "oncode-main");
        assert_eq!(default_worktree_name("/work/oncode/", "feature/login/form"), "oncode-feature-login-form");
        assert_eq!(default_worktree_name("C:\\work\\oncode", "fix/typo"), "oncode-fix-typo");
    }

    #[test]
    fn dirty_worktree_errors_are_recognised() {
        let error = |code: &str, message: &str, detail: Option<&str>| AppError { code: code.into(), message: message.into(), detail: detail.map(String::from) };
        assert!(is_dirty_worktree_error(&error("dirtyWorkTree", "x", None)));
        assert!(is_dirty_worktree_error(&error("unknown", "'/w/x' contains modified or untracked files, use --force to delete it", None)));
        assert!(is_dirty_worktree_error(&error("unknown", "git failed", Some("fatal: '/w/x' contains modified or untracked files, use --force to delete it"))));
        assert!(!is_dirty_worktree_error(&error("unknown", "'/w/x' is not a working tree", None)));
        assert!(!is_dirty_worktree_error(&error("indexLocked", "index.lock exists", None)));
    }

    #[test]
    fn folder_and_ref_names_are_validated() {
        assert_eq!(validate_folder_name("  "), Some("Enter a folder name"));
        assert_eq!(validate_folder_name("a/b"), Some("Enter a single folder name, not a path"));
        assert_eq!(validate_folder_name(".."), Some("This is not a valid folder name"));
        assert_eq!(validate_folder_name("repo-main"), None);

        for good in ["main", "feature/login", "v1.0.0", "fix_typo-2"] {
            assert_eq!(validate_ref_name(good), None, "{good}");
        }
        assert_eq!(validate_ref_name(" "), Some("Enter a name"));
        assert_eq!(validate_ref_name("-x"), Some("Names cannot start with a dash"));
        for bad in ["a b", "a..b", "a~1", "a^", "a:b", "a?", "a*", "a[", "a\\b", "a@{1}", "a//b", "/a", "a/", "a.", "a.lock", "a\tb"] {
            assert_eq!(validate_ref_name(bad), Some("This is not a valid Git ref name"), "{bad}");
        }
    }
}
