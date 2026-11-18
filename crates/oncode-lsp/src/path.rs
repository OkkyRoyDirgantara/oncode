//! Pencarian program di PATH.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Mencari `command` di PATH dan mengembalikan path programnya.
///
/// Perintah yang memuat pemisah path (absolut maupun relatif) diperiksa langsung tanpa PATH.
/// Di Windows ekstensi dari `PATHEXT` (`.EXE`, `.CMD`, ...) ikut dicoba.
pub fn find_on_path(command: &str) -> Option<PathBuf> {
    resolve_command(command, None)
}

/// Seperti [`find_on_path`], tetapi path relatif yang memuat pemisah dihitung dari `base`.
pub(crate) fn resolve_command(command: &str, base: Option<&Path>) -> Option<PathBuf> {
    resolve_in(command, base, std::env::var_os("PATH").as_deref(), &executable_extensions())
}

/// Ekstensi yang dicoba saat mencari program; kosong di luar Windows.
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

/// Memeriksa satu kandidat. Tanpa daftar ekstensi (Unix) berkas harus punya bit eksekusi;
/// dengan daftar ekstensi (Windows) berkas harus berakhiran salah satu ekstensi itu.
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
        // Ekstensi ditambahkan, bukan mengganti: `foo.bar` menjadi `foo.bar.exe`.
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
        std::fs::write(path, b"").expect("tulis berkas");
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    #[cfg(unix)]
    #[test]
    fn finds_sh_on_path() {
        let found = find_on_path("sh").expect("sh ada di PATH");
        assert!(found.is_absolute());
        assert!(found.is_file());
        assert_eq!(find_on_path(&found.to_string_lossy()), Some(found));
    }

    #[test]
    fn missing_or_empty_command_is_none() {
        assert_eq!(find_on_path("oncode-perintah-yang-tidak-ada"), None);
        assert_eq!(find_on_path(""), None);
        assert_eq!(find_on_path("   "), None);
        assert_eq!(find_on_path("/tidak/ada/program"), None);
    }

    #[cfg(unix)]
    #[test]
    fn unix_requires_the_executable_bit_and_respects_path_order() {
        let first = tempfile::tempdir().expect("tempdir");
        let second = tempfile::tempdir().expect("tempdir");
        touch(&first.path().join("alat"));
        touch(&second.path().join("alat"));
        make_executable(&second.path().join("alat"));
        std::fs::create_dir(first.path().join("folder")).expect("mkdir");
        std::fs::create_dir(second.path().join("folder")).expect("mkdir");

        let path_var = std::env::join_paths([first.path(), second.path()]).expect("join_paths");
        // Berkas tanpa bit eksekusi di folder pertama dilewati.
        assert_eq!(resolve_in("alat", None, Some(&path_var), &[]), Some(second.path().join("alat")));
        // Folder bukan program.
        assert_eq!(resolve_in("folder", None, Some(&path_var), &[]), None);
        assert_eq!(resolve_in("alat", None, None, &[]), None);

        make_executable(&first.path().join("alat"));
        assert_eq!(resolve_in("alat", None, Some(&path_var), &[]), Some(first.path().join("alat")));
    }

    #[cfg(unix)]
    #[test]
    fn paths_with_separators_skip_path_lookup() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("bin")).expect("mkdir");
        let program = dir.path().join("bin").join("alat");
        touch(&program);
        make_executable(&program);

        let empty = std::ffi::OsString::new();
        assert_eq!(resolve_in(&program.to_string_lossy(), None, Some(&empty), &[]), Some(program.clone()));
        // Relatif terhadap `base` bila diberikan.
        assert_eq!(resolve_in("./bin/alat", Some(dir.path()), Some(&empty), &[]), Some(dir.path().join("./bin/alat")));
        assert_eq!(resolve_in("bin/alat", Some(dir.path()), Some(&empty), &[]), Some(dir.path().join("bin/alat")));
        assert_eq!(resolve_in("bin/hilang", Some(dir.path()), Some(&empty), &[]), None);
        // Nama polos tidak dicari di `base`.
        assert_eq!(resolve_in("alat", Some(&dir.path().join("bin")), Some(&empty), &[]), None);
    }

    #[test]
    fn pathext_extensions_are_appended() {
        // Sistem berkas Linux peka huruf besar, jadi berkas uji memakai ejaan yang sama dengan PATHEXT.
        let dir = tempfile::tempdir().expect("tempdir");
        touch(&dir.path().join("gopls.EXE"));
        touch(&dir.path().join("clangd.exe"));
        touch(&dir.path().join("tsserver")); // skrip sh dari npm: bukan program Windows
        touch(&dir.path().join("tsserver.CMD"));
        touch(&dir.path().join("catatan.txt"));
        let path_var = std::env::join_paths([dir.path()]).expect("join_paths");
        let exts = parse_pathext(".COM;.EXE;.BAT;.CMD");

        assert_eq!(resolve_in("gopls", None, Some(&path_var), &exts), Some(dir.path().join("gopls.EXE")));
        // Ekstensi yang sudah ditulis dicocokkan tanpa membedakan huruf besar.
        assert_eq!(resolve_in("clangd.exe", None, Some(&path_var), &exts), Some(dir.path().join("clangd.exe")));
        assert_eq!(resolve_in("tsserver", None, Some(&path_var), &exts), Some(dir.path().join("tsserver.CMD")));
        assert_eq!(resolve_in("catatan.txt", None, Some(&path_var), &exts), None);
        assert_eq!(resolve_in("hilang", None, Some(&path_var), &exts), None);
        // Path dengan pemisah juga mendapat ekstensi.
        let direct = dir.path().join("gopls");
        assert_eq!(resolve_in(&direct.to_string_lossy(), None, None, &exts), Some(dir.path().join("gopls.EXE")));
    }

    #[test]
    fn pathext_is_parsed_leniently() {
        assert_eq!(parse_pathext(".COM; .EXE;;.cmd;."), vec![".COM", ".EXE", ".cmd"]);
        assert!(parse_pathext("").is_empty());
    }
}
