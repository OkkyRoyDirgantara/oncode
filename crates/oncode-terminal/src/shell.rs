//! Pemilihan shell bawaan pengguna.

/// Shell bawaan pengguna.
///
/// - Unix: `$SHELL` bila menunjuk berkas yang ada, lalu `/bin/bash`, lalu `/bin/sh`.
/// - Windows: `pwsh.exe` bila ada di PATH, lalu `powershell.exe`, lalu `%COMSPEC%`.
#[cfg(not(windows))]
pub fn default_shell() -> String {
    pick_unix_shell(std::env::var("SHELL").ok().as_deref(), |path| std::path::Path::new(path).is_file())
}

/// Shell bawaan pengguna.
///
/// - Unix: `$SHELL` bila menunjuk berkas yang ada, lalu `/bin/bash`, lalu `/bin/sh`.
/// - Windows: `pwsh.exe` bila ada di PATH, lalu `powershell.exe`, lalu `%COMSPEC%`.
#[cfg(windows)]
pub fn default_shell() -> String {
    pick_windows_shell(std::env::var_os("PATH").as_deref(), std::env::var("COMSPEC").ok().as_deref())
}

#[cfg(any(not(windows), test))]
fn pick_unix_shell(env_shell: Option<&str>, exists: impl Fn(&str) -> bool) -> String {
    if let Some(shell) = env_shell.map(str::trim).filter(|s| !s.is_empty()) {
        if exists(shell) {
            return shell.to_string();
        }
    }
    if exists("/bin/bash") {
        return "/bin/bash".to_string();
    }
    "/bin/sh".to_string()
}

#[cfg(any(windows, test))]
fn pick_windows_shell(path_var: Option<&std::ffi::OsStr>, comspec: Option<&str>) -> String {
    for name in ["pwsh.exe", "powershell.exe"] {
        if is_in_path(name, path_var) {
            return name.to_string();
        }
    }
    match comspec.map(str::trim).filter(|s| !s.is_empty()) {
        Some(comspec) => comspec.to_string(),
        None => "cmd.exe".to_string(),
    }
}

/// `true` bila `file_name` ada sebagai berkas di salah satu folder PATH.
#[cfg(any(windows, test))]
fn is_in_path(file_name: &str, path_var: Option<&std::ffi::OsStr>) -> bool {
    let Some(path_var) = path_var else { return false };
    std::env::split_paths(path_var)
        .filter(|dir| !dir.as_os_str().is_empty())
        .any(|dir| dir.join(file_name).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_prefers_env_shell_when_it_exists() {
        assert_eq!(pick_unix_shell(Some("/usr/bin/fish"), |_| true), "/usr/bin/fish");
        assert_eq!(pick_unix_shell(Some("  /usr/bin/zsh\n"), |_| true), "/usr/bin/zsh");
    }

    #[test]
    fn unix_falls_back_to_bash_then_sh() {
        assert_eq!(pick_unix_shell(None, |p| p == "/bin/bash"), "/bin/bash");
        assert_eq!(pick_unix_shell(Some(""), |p| p == "/bin/bash"), "/bin/bash");
        assert_eq!(pick_unix_shell(Some("/hilang/shell"), |p| p == "/bin/bash"), "/bin/bash");
        assert_eq!(pick_unix_shell(Some("/hilang/shell"), |_| false), "/bin/sh");
    }

    #[test]
    fn windows_prefers_pwsh_then_powershell_then_comspec() {
        let with_pwsh = tempfile::tempdir().expect("tempdir");
        let with_powershell = tempfile::tempdir().expect("tempdir");
        let empty = tempfile::tempdir().expect("tempdir");
        std::fs::write(with_pwsh.path().join("pwsh.exe"), b"").expect("tulis");
        std::fs::write(with_powershell.path().join("powershell.exe"), b"").expect("tulis");

        let join = |dirs: &[&std::path::Path]| std::env::join_paths(dirs).expect("join_paths");
        let comspec = Some(r"C:\Windows\System32\cmd.exe");

        let both = join(&[empty.path(), with_powershell.path(), with_pwsh.path()]);
        assert_eq!(pick_windows_shell(Some(&both), comspec), "pwsh.exe");

        let only_powershell = join(&[empty.path(), with_powershell.path()]);
        assert_eq!(pick_windows_shell(Some(&only_powershell), comspec), "powershell.exe");

        let none = join(&[empty.path()]);
        assert_eq!(pick_windows_shell(Some(&none), comspec), r"C:\Windows\System32\cmd.exe");
        assert_eq!(pick_windows_shell(Some(&none), None), "cmd.exe");
        assert_eq!(pick_windows_shell(None, Some("  ")), "cmd.exe");
    }

    #[cfg(unix)]
    #[test]
    fn default_shell_points_to_an_existing_file() {
        let shell = default_shell();
        assert!(std::path::Path::new(&shell).is_file(), "shell bawaan tidak ada: {shell}");
    }
}
