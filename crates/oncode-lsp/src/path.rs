//! Looking up programs on PATH.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Looks up `command` on PATH and returns the program's path.
///
/// A command containing a path separator (absolute or relative) is checked directly without PATH.
/// On Windows, extensions from `PATHEXT` (`.EXE`, `.CMD`, ...) are tried as well.
pub fn find_on_path(command: &str) -> Option<PathBuf> {
    resolve_command(command, None)
}

/// Like [`find_on_path`], but a relative path containing a separator is resolved from `base`.
pub(crate) fn resolve_command(command: &str, base: Option<&Path>) -> Option<PathBuf> {
    resolve_in(command, base, std::env::var_os("PATH").as_deref(), &executable_extensions())
}

/// Extensions tried when looking up a program; empty outside Windows.
fn executable_extensions() -> Vec<String> {
    if cfg!(windows) {
        let raw = std::env::var("PATHEXT").unwrap_or_default();
        parse_pathext(if raw.trim().is_empty() { ".COM;.EXE;.BAT;.CMD" } else { &raw })
    } else {
        Vec::new()
    }
}

fn parse_pathext(raw: &str) -> Vec<String> {
    raw.split(';')
        .map(str::trim)
        .filter(|ext| ext.len() > 1 && ext.starts_with('.'))
        .map(str::to_string)
        .collect()
}

fn has_separator(command: &str) -> bool {
    command.contains('/') || (cfg!(windows) && command.contains('\\'))
}

fn resolve_in(command: &str, base: Option<&Path>, path_var: Option<&OsStr>, extensions: &[String]) -> Option<PathBuf> {
    if command.trim().is_empty() {
        return None;
    }
    let as_path = Path::new(command);
    if as_path.is_absolute() || has_separator(command) {
        let candidate = match base {
            Some(base) if as_path.is_relative() => base.join(as_path),
            _ => as_path.to_path_buf(),
        };
        return executable_at(&candidate, extensions);
    }
    std::env::split_paths(path_var?)
        .filter(|dir| !dir.as_os_str().is_empty())
        .find_map(|dir| executable_at(&dir.join(command), extensions))
}

/// Checks a single candidate. Without an extension list (Unix) the file must have the execute bit;
/// with an extension list (Windows) the file must end with one of those extensions.
fn executable_at(candidate: &Path, extensions: &[String]) -> Option<PathBuf> {
    if extensions.is_empty() {
        return is_executable_file(candidate).then(|| candidate.to_path_buf());
    }
    let has_known_extension = candidate
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|ext| extensions.iter().any(|known| known[1..].eq_ignore_ascii_case(ext)));
    if has_known_extension && candidate.is_file() {
        return Some(candidate.to_path_buf());
    }
    extensions.iter().find_map(|ext| {
        // The extension is appended, not replaced: `foo.bar` becomes `foo.bar.exe`.
        let mut with_ext = candidate.as_os_str().to_os_string();
        with_ext.push(ext);
        let with_ext = PathBuf::from(with_ext);
        with_ext.is_file().then_some(with_ext)
    })
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        std::fs::write(path, b"").expect("write file");
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    #[cfg(unix)]
    #[test]
    fn finds_sh_on_path() {
        let found = find_on_path("sh").expect("sh is on PATH");
        assert!(found.is_absolute());
        assert!(found.is_file());
        assert_eq!(find_on_path(&found.to_string_lossy()), Some(found));
    }

    #[test]
    fn missing_or_empty_command_is_none() {
        assert_eq!(find_on_path("oncode-no-such-command"), None);
        assert_eq!(find_on_path(""), None);
        assert_eq!(find_on_path("   "), None);
        assert_eq!(find_on_path("/no/such/program"), None);
    }

    #[cfg(unix)]
    #[test]
    fn unix_requires_the_executable_bit_and_respects_path_order() {
        let first = tempfile::tempdir().expect("tempdir");
        let second = tempfile::tempdir().expect("tempdir");
        touch(&first.path().join("tool"));
        touch(&second.path().join("tool"));
        make_executable(&second.path().join("tool"));
        std::fs::create_dir(first.path().join("folder")).expect("mkdir");
        std::fs::create_dir(second.path().join("folder")).expect("mkdir");

        let path_var = std::env::join_paths([first.path(), second.path()]).expect("join_paths");
        // A file without the execute bit in the first folder is skipped.
        assert_eq!(resolve_in("tool", None, Some(&path_var), &[]), Some(second.path().join("tool")));
        // A folder is not a program.
        assert_eq!(resolve_in("folder", None, Some(&path_var), &[]), None);
        assert_eq!(resolve_in("tool", None, None, &[]), None);

        make_executable(&first.path().join("tool"));
        assert_eq!(resolve_in("tool", None, Some(&path_var), &[]), Some(first.path().join("tool")));
    }

    #[cfg(unix)]
    #[test]
    fn paths_with_separators_skip_path_lookup() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("bin")).expect("mkdir");
        let program = dir.path().join("bin").join("tool");
        touch(&program);
        make_executable(&program);

        let empty = std::ffi::OsString::new();
        assert_eq!(resolve_in(&program.to_string_lossy(), None, Some(&empty), &[]), Some(program.clone()));
        // Relative to `base` if given.
        assert_eq!(resolve_in("./bin/tool", Some(dir.path()), Some(&empty), &[]), Some(dir.path().join("./bin/tool")));
        assert_eq!(resolve_in("bin/tool", Some(dir.path()), Some(&empty), &[]), Some(dir.path().join("bin/tool")));
        assert_eq!(resolve_in("bin/missing", Some(dir.path()), Some(&empty), &[]), None);
        // A bare name is not looked up in `base`.
        assert_eq!(resolve_in("tool", Some(&dir.path().join("bin")), Some(&empty), &[]), None);
    }

    #[test]
    fn pathext_extensions_are_appended() {
        // Linux file systems are case-sensitive, so the test files use the same spelling as PATHEXT.
        let dir = tempfile::tempdir().expect("tempdir");
        touch(&dir.path().join("gopls.EXE"));
        touch(&dir.path().join("clangd.exe"));
        touch(&dir.path().join("tsserver")); // sh script from npm: not a Windows program
        touch(&dir.path().join("tsserver.CMD"));
        touch(&dir.path().join("notes.txt"));
        let path_var = std::env::join_paths([dir.path()]).expect("join_paths");
        let exts = parse_pathext(".COM;.EXE;.BAT;.CMD");

        assert_eq!(resolve_in("gopls", None, Some(&path_var), &exts), Some(dir.path().join("gopls.EXE")));
        // An extension that is already written is matched case-insensitively.
        assert_eq!(resolve_in("clangd.exe", None, Some(&path_var), &exts), Some(dir.path().join("clangd.exe")));
        assert_eq!(resolve_in("tsserver", None, Some(&path_var), &exts), Some(dir.path().join("tsserver.CMD")));
        assert_eq!(resolve_in("notes.txt", None, Some(&path_var), &exts), None);
        assert_eq!(resolve_in("missing", None, Some(&path_var), &exts), None);
        // A path with a separator also gets the extension.
        let direct = dir.path().join("gopls");
        assert_eq!(resolve_in(&direct.to_string_lossy(), None, None, &exts), Some(dir.path().join("gopls.EXE")));
    }

    #[test]
    fn pathext_is_parsed_leniently() {
        assert_eq!(parse_pathext(".COM; .EXE;;.cmd;."), vec![".COM", ".EXE", ".cmd"]);
        assert!(parse_pathext("").is_empty());
    }
}
