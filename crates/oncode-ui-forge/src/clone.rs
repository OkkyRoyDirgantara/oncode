//! Fungsi murni untuk dialog clone: nama folder dari URL dan validasi masukan.

/// Panjang awalan `skema://` bila `value` diawali skema URL (`[a-z][a-z0-9+.-]*://`).
fn scheme(value: &str) -> Option<&str> {
    let end = value.find("://")?;
    let scheme = &value[..end];
    let mut chars = scheme.chars();
    let first = chars.next()?;
    (first.is_ascii_alphabetic() && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))).then_some(scheme)
}

fn strip_git_suffix(value: &str) -> &str {
    match value.len().checked_sub(4) {
        Some(cut) if value.is_char_boundary(cut) && value[cut..].eq_ignore_ascii_case(".git") => &value[..cut],
        _ => value,
    }
}

/// Nama folder bawaan untuk sebuah URL clone, seperti yang dipilih `git clone`: segmen terakhir
/// tanpa garis miring penutup dan tanpa akhiran `.git`. Mendukung bentuk https/ssh/git/file
/// maupun bentuk scp (`git@host:owner/repo.git`). Kosong bila tidak ada nama yang bisa ditebak.
pub fn repo_name_from_url(url: &str) -> String {
    let value = url.trim();
    // Query dan fragmen bukan bagian dari nama.
    let value = value.split(['?', '#']).next().unwrap_or_default();
    let value = value.trim_end_matches(['/', '\\']);
    let value = strip_git_suffix(value).trim_end_matches(['/', '\\']);
    // Sisa yang hanya berupa host (mis. "https://example.com") bukan nama repositori.
    if let Some(scheme) = scheme(value) {
        if !value[scheme.len() + 3..].contains('/') {
            return String::new();
        }
    }
    let name = value.rsplit(['/', '\\', ':']).next().unwrap_or_default();
    if validate_folder_name(name).is_none() {
        name.to_string()
    } else {
        String::new()
    }
}

/// Pesan galat untuk nama folder tujuan, atau `None` bila sah.
pub fn validate_folder_name(name: &str) -> Option<&'static str> {
    if name.trim().is_empty() {
        return Some("Enter a folder name");
    }
    if name != name.trim() {
        return Some("The name cannot start or end with a space");
    }
    if name.contains(['/', '\\']) {
        return Some("The name cannot contain a path separator");
    }
    if name == "." || name == ".." {
        return Some("This name is reserved");
    }
    if name.chars().any(|c| c.is_control()) {
        return Some("The name cannot contain control characters");
    }
    if name.contains(['<', '>', ':', '"', '|', '?', '*']) {
        return Some("The name cannot contain any of < > : \" | ? *");
    }
    if name.starts_with('-') {
        return Some("The name cannot start with a dash");
    }
    if name.ends_with('.') {
        return Some("The name cannot end with a dot");
    }
    if name.chars().count() > 255 {
        return Some("The name is too long");
    }
    None
}

/// Bentuk scp: `[user@]host:path`.
fn is_scp_like(value: &str) -> bool {
    let Some((head, path)) = value.split_once(':') else { return false };
    if path.is_empty() || head.contains(['/', '\\']) {
        return false;
    }
    match head.split_once('@') {
        // user@host: keduanya tidak kosong.
        Some((user, host)) => !user.is_empty() && !host.is_empty(),
        // host tanpa user harus berupa nama bertitik, agar `C:\...` tidak dianggap scp.
        None => head.char_indices().any(|(index, c)| c == '.' && index > 0 && index + 1 < head.len()),
    }
}

/// Path lokal absolut: `/…`, `C:\…`, atau `~/…`.
fn is_local_absolute(value: &str) -> bool {
    let bytes = value.as_bytes();
    value.starts_with('/') || value.starts_with("~/") || (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'/' | b'\\'))
}

/// Pesan galat untuk URL clone, atau `None` bila bentuknya masuk akal. Git sendiri yang
/// memeriksa sisanya.
pub fn validate_clone_url(url: &str) -> Option<&'static str> {
    let value = url.trim();
    if value.is_empty() {
        return Some("Enter a repository URL");
    }
    if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Some("The URL cannot contain spaces");
    }
    if value.starts_with('-') {
        return Some("The URL cannot start with a dash");
    }
    // Bentuk `transport::alamat` (mis. `ext::`) menjalankan program pembantu; tidak pernah diterima dari sini.
    if let Some(end) = value.find("::") {
        let transport = &value[..end];
        if !transport.is_empty() && transport.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-')) {
            return Some("This kind of URL is not supported");
        }
    }
    if let Some(scheme) = scheme(value) {
        let allowed = ["https", "http", "ssh", "git", "file"].iter().any(|s| scheme.eq_ignore_ascii_case(s));
        return if allowed { None } else { Some("Use an https://, ssh:// or git:// URL") };
    }
    if is_scp_like(value) || is_local_absolute(value) {
        return None;
    }
    Some("Enter a full URL, for example https://github.com/owner/repo.git")
}

/// Menggabungkan folder induk dan nama untuk ditampilkan.
pub fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        return name.to_string();
    }
    let separator = if parent.contains('\\') && !parent.contains('/') { '\\' } else { '/' };
    format!("{}{separator}{name}", parent.trim_end_matches(['/', '\\']))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_name_follows_git_clone() {
        let cases = [
            ("https://github.com/owner/repo.git", "repo"),
            ("https://github.com/owner/repo", "repo"),
            ("https://github.com/owner/repo/", "repo"),
            ("https://github.com/owner/repo.git/", "repo"),
            ("https://github.com/owner/repo.GIT", "repo"),
            ("  https://github.com/owner/repo.git  ", "repo"),
            ("https://gitlab.com/group/sub/group/project.git", "project"),
            ("https://user@host.example:8443/scm/my.repo.git", "my.repo"),
            ("https://github.com/owner/repo.git?ref=main#readme", "repo"),
            // Bentuk scp.
            ("git@github.com:owner/repo.git", "repo"),
            ("git@github.com:owner/repo", "repo"),
            ("git@github.com:repo.git", "repo"),
            ("git@gitlab.com:group/sub/project.git/", "project"),
            ("ssh://git@github.com/owner/repo.git", "repo"),
            ("ssh://git@host.example:2222/owner/repo.git", "repo"),
            ("git://host.example/pub/thing.git", "thing"),
            ("file:///home/me/src/local.git", "local"),
            ("/home/me/src/local-repo/", "local-repo"),
            // Seperti git: menunjuk ke direktori `.git` berarti repositori induknya.
            ("/home/me/src/local-repo/.git", "local-repo"),
            ("C:\\repos\\win-repo", "win-repo"),
        ];
        for (url, expected) in cases {
            assert_eq!(repo_name_from_url(url), expected, "{url}");
        }
    }

    #[test]
    fn repo_name_is_not_guessed_from_a_bare_host() {
        for url in ["", "   ", "https://github.com", "https://github.com/", "ssh://git@host.example:2222", "https://host/.."] {
            assert_eq!(repo_name_from_url(url), "", "{url:?}");
        }
    }

    #[test]
    fn folder_names_are_validated() {
        for name in ["repo", "my.repo", "repo-2", "Proyek Baru", ".dotfiles", "a"] {
            assert_eq!(validate_folder_name(name), None, "{name:?}");
        }
        let long = "x".repeat(256);
        let cases = [
            ("", "Enter a folder name"),
            ("   ", "Enter a folder name"),
            ("a/b", "path separator"),
            ("a\\b", "path separator"),
            ("/abs", "path separator"),
            ("..", "reserved"),
            (".", "reserved"),
            (" repo", "space"),
            ("repo ", "space"),
            ("re\tpo", "control"),
            ("re\u{0}po", "control"),
            ("re:po", "cannot contain"),
            ("re\"po", "cannot contain"),
            ("re*po", "cannot contain"),
            ("-rf", "dash"),
            ("repo.", "dot"),
            (long.as_str(), "too long"),
        ];
        for (name, message) in cases {
            let problem = validate_folder_name(name).unwrap_or_else(|| panic!("{name:?} harus ditolak"));
            assert!(problem.contains(message), "{name:?}: {problem}");
        }
        assert_eq!(validate_folder_name(&"x".repeat(255)), None);
    }

    #[test]
    fn clone_urls_are_validated() {
        for url in [
            "https://github.com/owner/repo.git",
            "http://git.internal/owner/repo",
            "ssh://git@github.com/owner/repo.git",
            "git://host.example/pub/thing.git",
            "file:///home/me/src/local.git",
            "git@github.com:owner/repo.git",
            "github.com:owner/repo.git",
            "/home/me/src/local-repo",
            "C:\\repos\\win-repo",
            "~/src/repo",
        ] {
            assert_eq!(validate_clone_url(url), None, "{url}");
        }
        let cases = [
            ("", "Enter a repository URL"),
            ("https://github.com/owner/my repo", "spaces"),
            ("--upload-pack=touch /tmp/pwned", "spaces"),
            ("--upload-pack=x", "dash"),
            ("-u", "dash"),
            // Transport pembantu menjalankan program; tidak boleh lolos.
            ("ext::sh -c touch% /tmp/pwned", "spaces"),
            ("ext::sh", "not supported"),
            ("fd::17", "not supported"),
            ("ftp://host/repo.git", "https"),
            ("javascript://x/y", "https"),
            ("owner/repo", "full URL"),
            ("repo", "full URL"),
        ];
        for (url, message) in cases {
            let problem = validate_clone_url(url).unwrap_or_else(|| panic!("{url:?} harus ditolak"));
            assert!(problem.contains(message), "{url:?}: {problem}");
        }
    }

    #[test]
    fn join_path_uses_the_separator_of_the_parent() {
        assert_eq!(join_path("/home/me/src", "repo"), "/home/me/src/repo");
        assert_eq!(join_path("/home/me/src/", "repo"), "/home/me/src/repo");
        assert_eq!(join_path("C:\\Users\\me", "repo"), "C:\\Users\\me\\repo");
        assert_eq!(join_path("", "repo"), "repo");
    }
}
