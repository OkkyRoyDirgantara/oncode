//! Parse URL remote Git menjadi host, pemilik, dan nama repositori.

use crate::model::RepoRef;

/// Mem-parse URL remote menjadi [`RepoRef`].
///
/// Bentuk yang dikenali: `https://host/owner/name(.git)`, `http://…`,
/// `ssh://git@host[:port]/owner/name.git`, `git://…`, dan bentuk scp
/// `git@host:owner/name.git`; `user@` dan garis miring di akhir boleh ada. Port dibuang
/// dan nama host dijadikan huruf kecil. Untuk subgrup GitLab, `owner` memuat seluruh
/// jalur namespace (`grup/subgrup`).
///
/// Mengembalikan `None` untuk path lokal dan URL `file://`.
pub fn parse_remote_url(url: &str) -> Option<RepoRef> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    let (authority, path) = match url.split_once("://") {
        Some((scheme, rest)) => {
            let scheme = scheme.to_ascii_lowercase();
            if !matches!(scheme.as_str(), "https" | "http" | "ssh" | "git" | "git+ssh" | "ssh+git" | "git+https") {
                return None;
            }
            rest.split_once('/')?
        }
        None => {
            // Bentuk scp: [user@]host:path. Titik dua harus muncul sebelum garis miring
            // pertama; kalau tidak, ini path lokal.
            let (left, path) = url.split_once(':')?;
            if left.contains(['/', '\\']) {
                return None;
            }
            // Satu huruf sebelum titik dua adalah drive Windows (`C:\repo`), bukan host.
            if left.len() < 2 {
                return None;
            }
            (left, path)
        }
    };

    let host = host_from_authority(authority)?;

    let path = path.split(['?', '#']).next().unwrap_or_default();
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path).trim_end_matches('/');

    let mut segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segments.len() < 2 || segments.iter().any(|s| *s == "." || *s == ".." || s.contains('\\')) {
        return None;
    }
    let name = segments.pop()?.to_string();
    Some(RepoRef { host, owner: segments.join("/"), name })
}

/// Mengambil nama host dari `[user[:pass]@]host[:port]`.
fn host_from_authority(authority: &str) -> Option<String> {
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = if host_port.starts_with('[') {
        // IPv6 literal: kurung siku dipertahankan.
        let end = host_port.find(']')?;
        &host_port[..=end]
    } else {
        host_port.split(':').next().unwrap_or_default()
    };
    if host.is_empty() {
        return None;
    }
    let valid = host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '[' | ']' | ':'));
    if !valid {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (host, owner, name)
    type Parts = Option<(String, String, String)>;

    fn parsed(url: &str) -> Parts {
        parse_remote_url(url).map(|r| (r.host, r.owner, r.name))
    }

    fn expect(host: &str, owner: &str, name: &str) -> Parts {
        Some((host.to_string(), owner.to_string(), name.to_string()))
    }

    #[test]
    fn parses_remote_urls() {
        let table: &[(&str, Parts)] = &[
            // HTTPS
            ("https://github.com/octocat/hello-world.git", expect("github.com", "octocat", "hello-world")),
            ("https://github.com/octocat/hello-world", expect("github.com", "octocat", "hello-world")),
            ("https://github.com/octocat/hello-world/", expect("github.com", "octocat", "hello-world")),
            ("https://github.com/octocat/hello-world.git/", expect("github.com", "octocat", "hello-world")),
            ("http://git.lokal/tim/proyek.git", expect("git.lokal", "tim", "proyek")),
            ("  https://gitlab.com/tim/proyek.git\n", expect("gitlab.com", "tim", "proyek")),
            // user@ dan user:pass@ di HTTPS
            ("https://budi@github.com/octocat/hello-world.git", expect("github.com", "octocat", "hello-world")),
            ("https://budi:rahasia@gitlab.com/tim/proyek.git", expect("gitlab.com", "tim", "proyek")),
            // Port di HTTPS dibuang
            ("https://git.perusahaan.id:8443/tim/proyek.git", expect("git.perusahaan.id", "tim", "proyek")),
            // Host huruf besar
            ("https://GitHub.COM/Octocat/Hello-World.git", expect("github.com", "Octocat", "Hello-World")),
            ("git@GITLAB.com:Tim/Proyek.git", expect("gitlab.com", "Tim", "Proyek")),
            // Bentuk scp
            ("git@github.com:octocat/hello-world.git", expect("github.com", "octocat", "hello-world")),
            ("git@github.com:octocat/hello-world", expect("github.com", "octocat", "hello-world")),
            ("git@github.com:/octocat/hello-world.git", expect("github.com", "octocat", "hello-world")),
            ("github.com:octocat/hello-world.git", expect("github.com", "octocat", "hello-world")),
            ("org-1234@github.com:octocat/hello-world.git", expect("github.com", "octocat", "hello-world")),
            // ssh://, dengan dan tanpa port
            ("ssh://git@github.com/octocat/hello-world.git", expect("github.com", "octocat", "hello-world")),
            ("ssh://git@git.perusahaan.id:2222/tim/proyek.git", expect("git.perusahaan.id", "tim", "proyek")),
            ("ssh://git.perusahaan.id:2222/tim/proyek", expect("git.perusahaan.id", "tim", "proyek")),
            ("git://github.com/octocat/hello-world.git", expect("github.com", "octocat", "hello-world")),
            // Subgrup GitLab
            ("https://gitlab.com/grup/subgrup/proyek.git", expect("gitlab.com", "grup/subgrup", "proyek")),
            ("git@gitlab.com:grup/subgrup/lagi/proyek.git", expect("gitlab.com", "grup/subgrup/lagi", "proyek")),
            ("ssh://git@gitlab.com:22/grup/subgrup/proyek/", expect("gitlab.com", "grup/subgrup", "proyek")),
            // Nama yang memuat titik
            ("https://github.com/octocat/situs.github.io.git", expect("github.com", "octocat", "situs.github.io")),
            // IPv6
            ("ssh://git@[::1]:2222/tim/proyek.git", expect("[::1]", "tim", "proyek")),
            // Bukan remote forge
            ("/home/budi/repo", None),
            ("/home/budi/repo.git", None),
            ("./repo", None),
            ("../tim/repo.git", None),
            ("repo", None),
            ("tim/repo", None),
            ("C:\\Users\\budi\\repo", None),
            ("C:/Users/budi/repo", None),
            ("file:///home/budi/repo.git", None),
            ("file://server/share/tim/repo.git", None),
            ("", None),
            ("   ", None),
            // Kurang lengkap
            ("https://github.com/octocat", None),
            ("https://github.com/", None),
            ("https://github.com", None),
            ("git@github.com:hello-world.git", None),
            ("https:///octocat/hello-world", None),
        ];
        for (url, expected) in table {
            assert_eq!(&parsed(url), expected, "url: {url:?}");
        }
    }
}
