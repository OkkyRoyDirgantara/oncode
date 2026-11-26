//! Parses a Git remote URL into the host, owner, and repository name.

use crate::model::RepoRef;

/// Parses a remote URL into a [`RepoRef`].
///
/// Recognized forms: `https://host/owner/name(.git)`, `http://…`,
/// `ssh://git@host[:port]/owner/name.git`, `git://…`, and the scp form
/// `git@host:owner/name.git`; a `user@` and a trailing slash are allowed. The port is dropped
/// and the hostname is lowercased. For GitLab subgroups, `owner` contains the whole
/// namespace path (`group/subgroup`).
///
/// Returns `None` for local paths and `file://` URLs.
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
            // The scp form: [user@]host:path. The colon must appear before the first
            // slash; otherwise this is a local path.
            let (left, path) = url.split_once(':')?;
            if left.contains(['/', '\\']) {
                return None;
            }
            // A single letter before the colon is a Windows drive (`C:\repo`), not a host.
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

/// Extracts the hostname from `[user[:pass]@]host[:port]`.
fn host_from_authority(authority: &str) -> Option<String> {
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = if host_port.starts_with('[') {
        // IPv6 literal: the square brackets are kept.
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
            ("http://git.local/team/project.git", expect("git.local", "team", "project")),
            ("  https://gitlab.com/team/project.git\n", expect("gitlab.com", "team", "project")),
            // user@ and user:pass@ in HTTPS
            ("https://alice@github.com/octocat/hello-world.git", expect("github.com", "octocat", "hello-world")),
            ("https://alice:secret@gitlab.com/team/project.git", expect("gitlab.com", "team", "project")),
            // The port in HTTPS is dropped
            ("https://git.example.com:8443/team/project.git", expect("git.example.com", "team", "project")),
            // Uppercase host
            ("https://GitHub.COM/Octocat/Hello-World.git", expect("github.com", "Octocat", "Hello-World")),
            ("git@GITLAB.com:Team/Project.git", expect("gitlab.com", "Team", "Project")),
            // The scp form
            ("git@github.com:octocat/hello-world.git", expect("github.com", "octocat", "hello-world")),
            ("git@github.com:octocat/hello-world", expect("github.com", "octocat", "hello-world")),
            ("git@github.com:/octocat/hello-world.git", expect("github.com", "octocat", "hello-world")),
            ("github.com:octocat/hello-world.git", expect("github.com", "octocat", "hello-world")),
            ("org-1234@github.com:octocat/hello-world.git", expect("github.com", "octocat", "hello-world")),
            // ssh://, with and without a port
            ("ssh://git@github.com/octocat/hello-world.git", expect("github.com", "octocat", "hello-world")),
            ("ssh://git@git.example.com:2222/team/project.git", expect("git.example.com", "team", "project")),
            ("ssh://git.example.com:2222/team/project", expect("git.example.com", "team", "project")),
            ("git://github.com/octocat/hello-world.git", expect("github.com", "octocat", "hello-world")),
            // GitLab subgroups
            ("https://gitlab.com/group/subgroup/project.git", expect("gitlab.com", "group/subgroup", "project")),
            ("git@gitlab.com:group/subgroup/more/project.git", expect("gitlab.com", "group/subgroup/more", "project")),
            ("ssh://git@gitlab.com:22/group/subgroup/project/", expect("gitlab.com", "group/subgroup", "project")),
            // A name containing a dot
            ("https://github.com/octocat/site.github.io.git", expect("github.com", "octocat", "site.github.io")),
            // IPv6
            ("ssh://git@[::1]:2222/team/project.git", expect("[::1]", "team", "project")),
            // Not a forge remote
            ("/home/alice/repo", None),
            ("/home/alice/repo.git", None),
            ("./repo", None),
            ("../team/repo.git", None),
            ("repo", None),
            ("team/repo", None),
            ("C:\\Users\\alice\\repo", None),
            ("C:/Users/alice/repo", None),
            ("file:///home/alice/repo.git", None),
            ("file://server/share/team/repo.git", None),
            ("", None),
            ("   ", None),
            // Incomplete
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
