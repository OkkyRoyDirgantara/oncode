//! Small utilities for the forge feature: naming, relative time, safe URLs, and error messages.

use gpui_kit::App;
use oncode_core::AppError;
use oncode_forge::{ForgeHost, ForgeKind, RepoRef};

/// The provider name for display.
pub fn forge_name(kind: Option<ForgeKind>) -> &'static str {
    match kind {
        Some(ForgeKind::GitHub) => "GitHub",
        Some(ForgeKind::GitLab) => "GitLab",
        None => "the Git host",
    }
}

/// GitLab calls it a merge request; GitHub (and unknown hosts) a pull request.
pub fn pr_noun(kind: Option<ForgeKind>) -> &'static str {
    if kind == Some(ForgeKind::GitLab) {
        "Merge Request"
    } else {
        "Pull Request"
    }
}

/// The plural form of [`pr_noun`].
pub fn pr_noun_plural(kind: Option<ForgeKind>) -> &'static str {
    if kind == Some(ForgeKind::GitLab) {
        "Merge Requests"
    } else {
        "Pull Requests"
    }
}

/// Abbreviation: PR or MR.
pub fn pr_short(kind: Option<ForgeKind>) -> &'static str {
    if kind == Some(ForgeKind::GitLab) {
        "MR"
    } else {
        "PR"
    }
}

/// Number prefix: `#12` on GitHub, `!12` on GitLab.
pub fn pr_number(kind: Option<ForgeKind>, number: u32) -> String {
    format!("{}{number}", if kind == Some(ForgeKind::GitLab) { "!" } else { "#" })
}

/// Host name from an account origin (e.g. `https://github.com` → `github.com`). The port is kept.
pub fn host_name(origin: &str) -> String {
    let without_scheme = origin.split_once("://").map_or(origin, |(_, rest)| rest);
    without_scheme.split('/').next().unwrap_or_default().to_lowercase()
}

/// The server to log in to: the origin the user entered, or that provider's public service if empty.
pub fn forge_host(kind: ForgeKind, host: Option<&str>) -> ForgeHost {
    match host.filter(|h| !h.trim().is_empty()) {
        Some(host) => ForgeHost::new(kind, host),
        None if kind == ForgeKind::GitHub => ForgeHost::github(),
        None => ForgeHost::gitlab(),
    }
}

/// The `host` argument for login: `None` for github.com/gitlab.com, otherwise the full origin.
pub fn login_host_for(kind: ForgeKind, host: Option<&str>) -> Option<String> {
    let host = host.filter(|h| !h.is_empty())?;
    let name = host_name(host);
    if (kind == ForgeKind::GitHub && name == "github.com") || (kind == ForgeKind::GitLab && name == "gitlab.com") {
        return None;
    }
    Some(if host.contains("://") { host.trim_end_matches('/').to_string() } else { format!("https://{name}") })
}

/// Length of the `scheme://` prefix if `value` starts with a URL scheme (`[a-z][a-z0-9+.-]*://`).
fn scheme_len(value: &str) -> Option<usize> {
    let end = value.find("://")?;
    let scheme = &value[..end];
    let mut chars = scheme.chars();
    let first = chars.next()?;
    (first.is_ascii_alphabetic() && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))).then_some(end)
}

/// Normalizes self-hosted host input into an origin (with a path if the instance lives under a
/// subpath). Returns `None` if it is not a valid http(s) URL.
pub fn normalize_host(input: &str) -> Option<String> {
    let raw = input.trim();
    if raw.is_empty() || raw.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let (scheme, rest) = match scheme_len(raw) {
        Some(end) => (raw[..end].to_ascii_lowercase(), &raw[end + 3..]),
        None => ("https".to_string(), raw),
    };
    if scheme != "https" && scheme != "http" {
        return None;
    }
    // Credentials in the URL are never accepted; the token is asked for in a separate field.
    if rest.contains(['?', '#', '@', '\\']) {
        return None;
    }
    let (authority, path) = match rest.find('/') {
        Some(slash) => (&rest[..slash], &rest[slash..]),
        None => (rest, ""),
    };
    let (host, port) = if authority.starts_with('[') {
        let close = authority.find(']')?;
        let inside = &authority[1..close];
        if inside.is_empty() || !inside.chars().all(|c| c.is_ascii_hexdigit() || matches!(c, ':' | '.')) {
            return None;
        }
        match &authority[close + 1..] {
            "" => (&authority[..=close], None),
            tail => (&authority[..=close], Some(tail.strip_prefix(':')?)),
        }
    } else {
        let (host, port) = match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        };
        if host.is_empty() || host.starts_with('.') || !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_')) {
            return None;
        }
        (host, port)
    };
    let port = match port {
        None | Some("") => None,
        Some(port) => Some(port.parse::<u16>().ok()?),
    };
    // The scheme's default port is omitted, like browser URL normalization does.
    let port = port.filter(|p| !((scheme == "https" && *p == 443) || (scheme == "http" && *p == 80)));
    let host = host.to_ascii_lowercase();
    let path = path.trim_end_matches('/');
    Some(match port {
        Some(port) => format!("{scheme}://{host}:{port}{path}"),
        None => format!("{scheme}://{host}{path}"),
    })
}

/// The `owner/name` of a repository.
pub fn repo_slug(repo: &RepoRef) -> String {
    format!("{}/{}", repo.owner, repo.name)
}

/// An absolute http(s) URL, or `None` for other schemes (`javascript:`, `file:`, relative, …).
pub fn safe_external_url(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let end = scheme_len(trimmed)?;
    let scheme = trimmed[..end].to_ascii_lowercase();
    if scheme != "https" && scheme != "http" {
        return None;
    }
    let rest = &trimmed[end + 3..];
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or_default();
    if host.is_empty() || host.starts_with(':') || trimmed.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    Some(format!("{scheme}://{rest}"))
}

/// Image URLs that may be loaded: `https:` only.
pub fn safe_image_url(value: &str) -> Option<String> {
    safe_external_url(value).filter(|url| url.starts_with("https://"))
}

/// Opens a link in the system browser. Schemes other than http(s) are ignored.
pub fn open_external(value: &str, cx: &mut App) -> bool {
    match safe_external_url(value) {
        Some(url) => {
            cx.open_url(&url);
            true
        }
        None => false,
    }
}

/// Seconds since the Unix epoch, now.
pub fn now_seconds() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

const UNITS: [(f64, f64, &str); 6] = [
    (60.0, 1.0, "second"),
    (3600.0, 60.0, "minute"),
    (86400.0, 3600.0, "hour"),
    (86400.0 * 30.0, 86400.0, "day"),
    (86400.0 * 365.0, 86400.0 * 30.0, "month"),
    (f64::INFINITY, 86400.0 * 365.0, "year"),
];

/// Short relative time from a Unix timestamp (seconds), e.g. "3 hours ago".
pub fn relative_time(unix_seconds: f64, now_seconds: f64) -> String {
    if !unix_seconds.is_finite() || unix_seconds <= 0.0 {
        return String::new();
    }
    let diff = (now_seconds - unix_seconds).round();
    if diff < 45.0 {
        return if diff < -45.0 { "in the future" } else { "just now" }.to_string();
    }
    for (limit, seconds, name) in UNITS {
        if diff < limit {
            let n = (diff / seconds).round().max(1.0) as u64;
            return format!("{n} {name}{} ago", if n == 1 { "" } else { "s" });
        }
    }
    String::new()
}

/// Shortens text to at most `max` characters, with an ellipsis.
pub fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", head.trim_end())
}

/// An error message for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorText {
    /// The main sentence (English, written here).
    pub title: String,
    /// The original message from the backend/provider, if it adds information.
    pub detail: Option<String>,
}

impl ErrorText {
    pub fn plain(title: impl Into<String>) -> Self {
        Self { title: title.into(), detail: None }
    }

    /// Detail worth showing (not empty and different from the title).
    pub fn extra(&self) -> Option<&str> {
        self.detail.as_deref().filter(|d| !d.trim().is_empty() && *d != self.title)
    }

    /// One text for a toast: the title, then the detail on the next line.
    pub fn line(&self) -> String {
        match self.extra() {
            Some(detail) => format!("{}\n{detail}", self.title),
            None => self.title.clone(),
        }
    }

    /// One line: the title with the detail in parentheses.
    pub fn inline(&self) -> String {
        match self.extra() {
            Some(detail) => format!("{} ({detail})", self.title),
            None => self.title.clone(),
        }
    }
}

/// Translates a forge error code into an actionable message. Never contains the
/// token. `host` is the origin or host name of the account concerned.
pub fn describe_forge_error(error: &AppError, host: Option<&str>) -> ErrorText {
    let host = host.map(host_name).filter(|h| !h.is_empty()).unwrap_or_else(|| "the server".to_string());
    let message = error.message.clone();
    let title = match error.code.as_str() {
        "unauthorized" => format!("Sign-in for {host} is missing or has expired. Sign in again."),
        "forbidden" => format!("This account is not allowed to do that on {host}. The token may be missing a required scope."),
        "notFound" => format!("Not found on {host}, or this account has no access to it."),
        "rateLimited" => format!("The API rate limit of {host} was reached. Try again in a few minutes."),
        "network" => format!("Could not reach {host}. Check your connection and try again."),
        "keychain" => "The system keychain is unavailable or refused access, so the sign-in could not be stored.".to_string(),
        "unsupported" => "This is not supported by the host or not configured.".to_string(),
        "invalidInput" => format!("The request was rejected by {host}."),
        "invalidResponse" => format!("{host} sent an unexpected answer."),
        _ => return ErrorText { title: message, detail: error.detail.clone() },
    };
    ErrorText { title, detail: Some(message) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_provider() {
        assert_eq!(forge_name(Some(ForgeKind::GitHub)), "GitHub");
        assert_eq!(forge_name(Some(ForgeKind::GitLab)), "GitLab");
        assert_eq!(forge_name(None), "the Git host");
        assert_eq!(pr_noun(Some(ForgeKind::GitLab)), "Merge Request");
        assert_eq!(pr_noun(None), "Pull Request");
        assert_eq!(pr_noun_plural(Some(ForgeKind::GitLab)), "Merge Requests");
        assert_eq!(pr_noun_plural(Some(ForgeKind::GitHub)), "Pull Requests");
        assert_eq!((pr_short(Some(ForgeKind::GitHub)), pr_short(Some(ForgeKind::GitLab))), ("PR", "MR"));
        assert_eq!((pr_number(Some(ForgeKind::GitHub), 12), pr_number(Some(ForgeKind::GitLab), 12)), ("#12".to_string(), "!12".to_string()));
    }

    #[test]
    fn host_name_drops_scheme_and_path() {
        assert_eq!(host_name("https://GitHub.com"), "github.com");
        assert_eq!(host_name("http://127.0.0.1:8080/gitlab"), "127.0.0.1:8080");
        assert_eq!(host_name("git.example.com"), "git.example.com");
    }

    #[test]
    fn login_host_is_none_for_the_public_instances() {
        assert_eq!(login_host_for(ForgeKind::GitHub, Some("github.com")), None);
        assert_eq!(login_host_for(ForgeKind::GitLab, Some("https://gitlab.com/")), None);
        assert_eq!(login_host_for(ForgeKind::GitHub, None), None);
        assert_eq!(login_host_for(ForgeKind::GitHub, Some("")), None);
        // github.com as GitLab is not GitLab's public instance.
        assert_eq!(login_host_for(ForgeKind::GitLab, Some("github.com")).as_deref(), Some("https://github.com"));
        assert_eq!(login_host_for(ForgeKind::GitLab, Some("git.example.com")).as_deref(), Some("https://git.example.com"));
        assert_eq!(login_host_for(ForgeKind::GitLab, Some("http://127.0.0.1:8080/")).as_deref(), Some("http://127.0.0.1:8080"));
    }

    #[test]
    fn normalize_host_accepts_http_origins_only() {
        assert_eq!(normalize_host("git.example.com").as_deref(), Some("https://git.example.com"));
        assert_eq!(normalize_host("  https://Git.Example.com/  ").as_deref(), Some("https://git.example.com"));
        assert_eq!(normalize_host("http://127.0.0.1:8080").as_deref(), Some("http://127.0.0.1:8080"));
        assert_eq!(normalize_host("https://example.com/gitlab/").as_deref(), Some("https://example.com/gitlab"));
        assert_eq!(normalize_host("https://example.com:443").as_deref(), Some("https://example.com"));
        assert_eq!(normalize_host("HTTPS://example.com:8443").as_deref(), Some("https://example.com:8443"));
        assert_eq!(normalize_host("https://[::1]:8443").as_deref(), Some("https://[::1]:8443"));
        for bad in ["", "   ", "git example com", "ftp://example.com", "ssh://git@example.com", "https://user:pw@example.com", "https://example.com/?q=1", "https://example.com/#x", "https://", "https://:8080", "https://example.com:port", "javascript://x"] {
            assert_eq!(normalize_host(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn only_absolute_http_urls_are_safe() {
        assert_eq!(safe_external_url("https://example.com/a?b=1#c").as_deref(), Some("https://example.com/a?b=1#c"));
        assert_eq!(safe_external_url("  HTTP://example.com  ").as_deref(), Some("http://example.com"));
        for bad in ["", "example.com", "/relative", "//example.com", "javascript:alert(1)", "javascript://example.com/%0aalert(1)", "data:text/html,x", "file:///etc/passwd", "vbscript:x", "https://", "https:///path", "https://example.com/a b"] {
            assert_eq!(safe_external_url(bad), None, "{bad:?}");
        }
        assert_eq!(safe_image_url("https://example.com/a.png").as_deref(), Some("https://example.com/a.png"));
        assert_eq!(safe_image_url("http://example.com/a.png"), None);
        assert_eq!(safe_image_url("data:image/png;base64,AAAA"), None);
    }

    #[test]
    fn relative_time_picks_a_readable_unit() {
        let now = 1_800_000_000.0;
        assert_eq!(relative_time(0.0, now), "");
        assert_eq!(relative_time(f64::NAN, now), "");
        assert_eq!(relative_time(now - 10.0, now), "just now");
        assert_eq!(relative_time(now + 10.0, now), "just now");
        assert_eq!(relative_time(now + 600.0, now), "in the future");
        assert_eq!(relative_time(now - 50.0, now), "50 seconds ago");
        assert_eq!(relative_time(now - 60.0, now), "1 minute ago");
        assert_eq!(relative_time(now - 3.0 * 3600.0, now), "3 hours ago");
        assert_eq!(relative_time(now - 86400.0, now), "1 day ago");
        assert_eq!(relative_time(now - 45.0 * 86400.0, now), "2 months ago");
        assert_eq!(relative_time(now - 800.0 * 86400.0, now), "2 years ago");
    }

    #[test]
    fn shorten_keeps_short_text_and_adds_an_ellipsis() {
        assert_eq!(shorten("short", 40), "short");
        assert_eq!(shorten("one four two", 9), "one four…");
        assert_eq!(shorten("héllo wörld", 6), "héllo…");
    }

    #[test]
    fn errors_become_actionable_messages() {
        let error = |code: &str| AppError::new(code, "original message");
        let text = describe_forge_error(&error("unauthorized"), Some("https://git.example.com"));
        assert_eq!(text.title, "Sign-in for git.example.com is missing or has expired. Sign in again.");
        assert_eq!(text.detail.as_deref(), Some("original message"));
        assert_eq!(text.line(), "Sign-in for git.example.com is missing or has expired. Sign in again.\noriginal message");
        assert!(describe_forge_error(&error("network"), None).title.starts_with("Could not reach the server."));
        assert!(describe_forge_error(&error("rateLimited"), Some("github.com")).title.contains("rate limit of github.com"));
        assert!(describe_forge_error(&error("keychain"), None).title.contains("keychain"));
        assert!(describe_forge_error(&error("forbidden"), None).title.contains("scope"));
        assert!(describe_forge_error(&error("notFound"), None).title.starts_with("Not found on the server"));
        assert!(describe_forge_error(&error("invalidInput"), None).title.contains("rejected"));
        assert!(describe_forge_error(&error("invalidResponse"), None).title.contains("unexpected answer"));
        assert!(describe_forge_error(&error("unsupported"), None).title.contains("not supported"));
        // An unknown code: the backend message as is, without a duplicate detail.
        let other = describe_forge_error(&error("io"), None);
        assert_eq!((other.title.as_str(), other.extra()), ("original message", None));
        assert_eq!(other.inline(), "original message");
        assert_eq!(text.inline(), "Sign-in for git.example.com is missing or has expired. Sign in again. (original message)");
    }
}
