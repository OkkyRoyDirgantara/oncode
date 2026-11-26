//! Link detection in terminal output. Only `http://` and `https://` are recognized and opened.

use std::ops::Range;

/// A link that is safe to open in the system browser.
pub fn is_web_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    ["http://", "https://"].iter().any(|scheme| lower.len() > scheme.len() && lower.starts_with(scheme))
}

fn ends_url(c: char) -> bool {
    c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"' | '`' | '\'' | '│' | '\0')
}

/// The ranges (character indices, exclusive end) of all web links in `chars`.
///
/// Sentence-ending punctuation at the end is not included, and a closing bracket is only included if
/// its pair is inside the link (e.g. a Wikipedia link).
pub fn find_urls(chars: &[char]) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let rest = &chars[index..];
        let scheme_len = ["https://", "http://"].iter().find_map(|scheme| {
            let scheme: Vec<char> = scheme.chars().collect();
            (rest.len() > scheme.len() && rest[..scheme.len()].iter().zip(&scheme).all(|(a, b)| a.eq_ignore_ascii_case(b))).then_some(scheme.len())
        });
        // A scheme in the middle of a word (e.g. "xhttp://") is not the start of a link.
        let at_boundary = index == 0 || !chars[index - 1].is_alphanumeric();
        let Some(scheme_len) = scheme_len.filter(|_| at_boundary) else {
            index += 1;
            continue;
        };
        let mut end = index + scheme_len;
        while end < chars.len() && !ends_url(chars[end]) {
            end += 1;
        }
        loop {
            let Some(&last) = chars[index..end].last() else { break };
            let closer = |open: char, close: char| {
                last == close && chars[index..end].iter().filter(|c| **c == close).count() > chars[index..end].iter().filter(|c| **c == open).count()
            };
            let trailing = matches!(last, '.' | ',' | ';' | ':' | '!' | '?') || closer('(', ')') || closer('[', ']') || closer('{', '}');
            if !trailing || end <= index + scheme_len {
                break;
            }
            end -= 1;
        }
        if end > index + scheme_len {
            found.push(index..end);
        }
        index = end.max(index + 1);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(text: &str) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        find_urls(&chars).into_iter().map(|range| chars[range].iter().collect()).collect()
    }

    #[test]
    fn finds_http_and_https_links() {
        assert_eq!(urls("see https://example.com/a?b=1#c and http://localhost:3000/"), ["https://example.com/a?b=1#c", "http://localhost:3000/"]);
        assert_eq!(urls("HTTPS://EXAMPLE.COM"), ["HTTPS://EXAMPLE.COM"]);
        assert!(urls("no links here, ftp://x.y or file:///etc/passwd").is_empty());
        assert!(urls("https:// alone").is_empty());
    }

    #[test]
    fn trailing_punctuation_and_unbalanced_brackets_are_excluded() {
        assert_eq!(urls("Open https://example.com/docs."), ["https://example.com/docs"]);
        assert_eq!(urls("(https://example.com/x)"), ["https://example.com/x"]);
        assert_eq!(urls("https://en.wikipedia.org/wiki/Rust_(programming_language)"), ["https://en.wikipedia.org/wiki/Rust_(programming_language)"]);
        assert_eq!(urls("<https://example.com>, \"https://a.b/c\""), ["https://example.com", "https://a.b/c"]);
        assert_eq!(urls("[link](https://example.com/a),"), ["https://example.com/a"]);
    }

    #[test]
    fn scheme_inside_a_word_is_not_a_link() {
        assert!(urls("xhttps://example.com").is_empty());
        assert_eq!(urls("url=https://example.com"), ["https://example.com"]);
    }

    #[test]
    fn only_web_schemes_are_openable() {
        assert!(is_web_url("https://example.com"));
        assert!(is_web_url("HTTP://example.com"));
        assert!(!is_web_url("https://"));
        assert!(!is_web_url("file:///etc/passwd"));
        assert!(!is_web_url("javascript:alert(1)"));
        assert!(!is_web_url(" https://example.com"));
    }
}
