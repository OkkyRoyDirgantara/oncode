//! Conversion between file paths and `file://` URIs for LSP, correct for both POSIX and Windows paths.

use std::str::FromStr;

fn is_drive(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'\\' | b'/')
}

fn is_unc(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("\\\\") else { return false };
    match rest.find(['\\', '/']) {
        Some(end) => end > 0,
        None => false,
    }
}

fn is_windows_path(path: &str) -> bool {
    is_drive(path) || is_unc(path)
}

/// Same as `encodeURIComponent`: letters, digits, and `-_.!~*'()` are left alone; the rest
/// (space, `#`, `?`, `%`, non-ASCII, ...) is escaped as UTF-8 bytes.
fn encode_segment(segment: &str, out: &mut String) {
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
}

fn join_encoded<'a>(segments: impl Iterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for (index, segment) in segments.enumerate() {
        if index > 0 {
            out.push('/');
        }
        encode_segment(segment, &mut out);
    }
    out
}

/// `/home/a b/x.rs` → `file:///home/a%20b/x.rs`; `C:\x\y` → `file:///C:/x/y`;
/// `\\server\share\x` → `file://server/share/x`.
pub fn path_to_uri(path: &str) -> String {
    if is_unc(path) {
        let mut parts = path[2..].split(['\\', '/']).filter(|part| !part.is_empty());
        let host = parts.next().unwrap_or_default();
        return format!("file://{host}/{}", join_encoded(parts));
    }
    if is_drive(path) {
        let segments = path[2..].split(['\\', '/']).filter(|part| !part.is_empty());
        return format!("file:///{}/{}", &path[..2], join_encoded(segments));
    }
    // On POSIX a backslash is an ordinary file name character.
    let joined = join_encoded(path.split('/'));
    format!("file://{}{joined}", if joined.starts_with('/') { "" } else { "/" })
}

/// A `lsp_types` typed URI for a path; `None` if the result is not a valid URI.
pub fn path_to_lsp_uri(path: &str) -> Option<lsp_types::Uri> {
    lsp_types::Uri::from_str(&path_to_uri(path)).ok()
}

/// The inverse of `encode_segment`; `None` if the escape is broken or not UTF-8.
fn decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = text.get(index + 1..index + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// A broken escape is used as-is rather than failing.
fn decode_lossy(text: &str) -> String {
    decode(text).unwrap_or_else(|| text.to_string())
}

/// The inverse of [`path_to_uri`]. Windows drive letters are normalized to uppercase (servers often
/// send `file:///c%3A/...`). A URI that is not `file:` is returned as-is.
pub fn uri_to_path(uri: &str) -> String {
    let Some(prefix) = uri.get(..7) else { return uri.to_string() };
    if !prefix.eq_ignore_ascii_case("file://") {
        return uri.to_string();
    }
    let rest = &uri[7..];
    let (host, raw) = match rest.find('/') {
        Some(slash) => (&rest[..slash], &rest[slash..]),
        None => (rest, "/"),
    };
    // Drop the query and fragment; `#` and `?` inside a file name are always escaped.
    let raw = raw.split(['?', '#']).next().unwrap_or(raw);
    let decoded = raw.split('/').map(decode_lossy).collect::<Vec<_>>().join("/");
    if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
        return format!("\\\\{}{}", decode_lossy(host), decoded.replace('/', "\\"));
    }
    let bytes = decoded.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' && (bytes.len() == 3 || bytes[3] == b'/') {
        let tail = if bytes.len() == 3 { "\\".to_string() } else { decoded[3..].replace('/', "\\") };
        return format!("{}:{tail}", (bytes[1] as char).to_ascii_uppercase());
    }
    decoded
}

/// Path comparison key: Windows paths are case-insensitive and their separators are unified.
pub fn path_key(path: &str) -> String {
    if is_windows_path(path) {
        path.replace('/', "\\").to_lowercase()
    } else {
        path.to_string()
    }
}

/// Two URIs point to the same file even if they are escaped differently.
pub fn uri_key(uri: &str) -> String {
    path_key(&uri_to_path(uri))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_paths() {
        assert_eq!(path_to_uri("/home/user/project/src/main.rs"), "file:///home/user/project/src/main.rs");
        assert_eq!(path_to_uri("/"), "file:///");
    }

    #[test]
    fn windows_drive_letters_with_either_separator() {
        assert_eq!(path_to_uri("C:\\x\\y"), "file:///C:/x/y");
        assert_eq!(path_to_uri("C:/x/y"), "file:///C:/x/y");
        assert_eq!(path_to_uri("d:\\Projects\\app\\main.go"), "file:///d:/Projects/app/main.go");
        assert_eq!(path_to_uri("C:\\"), "file:///C:/");
    }

    #[test]
    fn unc_paths() {
        assert_eq!(path_to_uri("\\\\server\\share\\dir\\a b.ts"), "file://server/share/dir/a%20b.ts");
    }

    #[test]
    fn escapes_spaces_uri_characters_and_non_ascii() {
        assert_eq!(path_to_uri("/home/a b/c.rs"), "file:///home/a%20b/c.rs");
        assert_eq!(path_to_uri("/tmp/what?#100%.txt"), "file:///tmp/what%3F%23100%25.txt");
        assert_eq!(path_to_uri("/home/é/日本語.py"), "file:///home/%C3%A9/%E6%97%A5%E6%9C%AC%E8%AA%9E.py");
        assert_eq!(path_to_uri("C:\\Users\\Dé v\\a#b.ts"), "file:///C:/Users/D%C3%A9%20v/a%23b.ts");
    }

    #[test]
    fn backslash_is_a_file_name_character_on_posix() {
        assert_eq!(path_to_uri("/tmp/a\\b.txt"), "file:///tmp/a%5Cb.txt");
        assert_eq!(uri_to_path("file:///tmp/a%5Cb.txt"), "/tmp/a\\b.txt");
    }

    #[test]
    fn round_trips_posix_paths() {
        for path in ["/home/user/main.rs", "/home/a b/c d.rs", "/tmp/what?#100%.txt", "/home/é/日本語.py", "/srv/@types/node/index.d.ts", "/x/[id]/(group)/+page.ts", "/"] {
            assert_eq!(uri_to_path(&path_to_uri(path)), path, "{path}");
            assert!(path_to_lsp_uri(path).is_some(), "{path}");
        }
    }

    #[test]
    fn round_trips_windows_paths() {
        for path in ["C:\\x\\y", "C:\\Users\\Dé v\\a#b?.ts", "D:\\a b\\c.go", "\\\\server\\share\\dir\\a b.ts"] {
            assert_eq!(uri_to_path(&path_to_uri(path)), path, "{path}");
        }
        // Separators and the drive letter are normalized.
        assert_eq!(uri_to_path(&path_to_uri("c:/x/y")), "C:\\x\\y");
    }

    #[test]
    fn accepts_the_forms_servers_send() {
        assert_eq!(uri_to_path("file:///c%3A/Users/me/a.ts"), "C:\\Users\\me\\a.ts");
        assert_eq!(uri_to_path("file:///C:/Users/me/a.ts"), "C:\\Users\\me\\a.ts");
        assert_eq!(uri_to_path("file:///srv/%40types/node/index.d.ts"), "/srv/@types/node/index.d.ts");
        assert_eq!(uri_to_path("file:///srv/@types/node/index.d.ts"), "/srv/@types/node/index.d.ts");
        assert_eq!(uri_to_path("file://localhost/home/user/a.rs"), "/home/user/a.rs");
    }

    #[test]
    fn drops_query_and_fragment_and_survives_broken_escapes() {
        assert_eq!(uri_to_path("file:///home/a.rs#L10"), "/home/a.rs");
        assert_eq!(uri_to_path("file:///home/a.rs?x=1"), "/home/a.rs");
        assert_eq!(uri_to_path("file:///home/100%zz.txt"), "/home/100%zz.txt");
    }

    #[test]
    fn non_file_uris_are_returned_unchanged() {
        assert_eq!(uri_to_path("untitled:Untitled-1"), "untitled:Untitled-1");
        assert_eq!(uri_to_path("jdt://contents/rt.jar/java.lang/String.class"), "jdt://contents/rt.jar/java.lang/String.class");
    }

    #[test]
    fn keys_ignore_escaping_differences() {
        assert_eq!(uri_key("file:///srv/%40types/a.ts"), uri_key("file:///srv/@types/a.ts"));
        assert_eq!(uri_key("file:///c%3A/Users/Me/a.ts"), uri_key(&path_to_uri("C:\\Users\\me\\A.ts")));
    }

    #[test]
    fn posix_keys_are_case_sensitive() {
        assert_ne!(path_key("/home/A.rs"), path_key("/home/a.rs"));
        assert_eq!(path_key("C:/X/y"), path_key("c:\\x\\Y"));
    }
}
