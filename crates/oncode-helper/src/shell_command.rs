//! The `oncode` command in the terminal, like VS Code's `code`: `oncode .` opens the current folder.
//!
//! The app binary is deliberately not named `oncode` (see `app/Cargo.toml`), so what gets installed
//! is a small script that runs `oncode-app` in the background so the terminal is free right away:
//! - Windows: `<app folder>\bin\oncode.cmd`, and then that `bin` folder is added to the user's
//!   PATH. The installer calls this when installing and removing the app.
//! - macOS: `/usr/local/bin/oncode`, with administrator permission if that folder is not writable.
//! - Linux: `~/.local/bin/oncode` (the `.deb` package already installs `/usr/bin/oncode` itself).
//!
//! A new `oncode-app` process forwards the folder to an already running OnCode if there is one (see
//! `app/src/single_instance.rs`).

use std::path::Path;
use std::process::ExitCode;

/// Marker inside the script: only scripts created by OnCode may be overwritten or removed.
const MARKER: &str = "Created by OnCode";

/// `oncode-helper install-shell-command <app folder>` and `uninstall-shell-command`.
/// Messages for the user are printed to stdout, errors to stderr.
pub fn main(install: bool, app_dir: Option<&str>) -> ExitCode {
    let Some(app_dir) = app_dir else {
        eprintln!("oncode-helper: missing the application folder");
        return ExitCode::from(2);
    };
    let app_dir = Path::new(app_dir);
    let result = if install { platform::install(app_dir) } else { platform::uninstall(app_dir) };
    match result {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// Content of `oncode.cmd`. `start` makes the terminal not wait for the app to close; the app path
/// is relative to the script, so the content is the same for every install location.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_script() -> String {
    format!("@echo off\r\nrem {MARKER}: opens a folder in OnCode, e.g. `oncode .`\r\nstart \"\" \"%~dp0..\\oncode-app.exe\" %*\r\n")
}

/// Content of the `oncode` script for macOS and Linux. `nohup ... &` detaches the app from the terminal.
#[cfg_attr(not(unix), allow(dead_code))]
fn unix_script(exe: &Path) -> String {
    format!("#!/bin/sh\n# {MARKER}: opens a folder in OnCode, e.g. `oncode .`\nnohup {} \"$@\" >/dev/null 2>&1 &\n", sh_quote(&exe.to_string_lossy()))
}

#[cfg_attr(not(unix), allow(dead_code))]
fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// PATH (Windows format, separated by `;`) with `dir` at the end, or `None` if it is already there.
#[cfg_attr(not(windows), allow(dead_code))]
fn path_with(path: &str, dir: &str) -> Option<String> {
    if path.split(';').any(|entry| same_entry(entry, dir)) {
        return None;
    }
    let path = path.trim_end_matches(';');
    Some(if path.is_empty() { dir.to_string() } else { format!("{path};{dir}") })
}

/// PATH without `dir`, or `None` if it was not there. Other entries are left as they are.
#[cfg_attr(not(windows), allow(dead_code))]
fn path_without(path: &str, dir: &str) -> Option<String> {
    if !path.split(';').any(|entry| same_entry(entry, dir)) {
        return None;
    }
    Some(path.split(';').filter(|entry| !same_entry(entry, dir)).collect::<Vec<_>>().join(";"))
}

/// Windows paths are case-insensitive; a trailing slash is ignored.
fn same_entry(entry: &str, dir: &str) -> bool {
    let normalize = |s: &str| s.trim().trim_end_matches(['\\', '/']).to_lowercase();
    !entry.trim().is_empty() && normalize(entry) == normalize(dir)
}

#[cfg(windows)]
mod platform {
    use std::path::Path;

    pub fn install(app_dir: &Path) -> Result<String, String> {
        let bin = app_dir.join("bin");
        std::fs::create_dir_all(&bin).map_err(|e| format!("Cannot create {}: {e}", bin.display()))?;
        let script = bin.join("oncode.cmd");
        std::fs::write(&script, super::windows_script()).map_err(|e| format!("Cannot write {}: {e}", script.display()))?;
        user_path::update(|path| super::path_with(path, &bin.to_string_lossy()))?;
        Ok(format!("Installed the 'oncode' command ({}). Open a new terminal to use it.", script.display()))
    }

    pub fn uninstall(app_dir: &Path) -> Result<String, String> {
        let bin = app_dir.join("bin");
        user_path::update(|path| super::path_without(path, &bin.to_string_lossy()))?;
        let script = bin.join("oncode.cmd");
        match std::fs::remove_file(&script) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("Cannot remove {}: {e}", script.display())),
        }
        // Only removed if empty.
        let _ = std::fs::remove_dir(&bin);
        Ok("Removed the 'oncode' command from PATH.".to_string())
    }

    /// The user's `Path` value in the registry (`HKCU\Environment`). Read and written as is:
    /// entries like `%USERPROFILE%\bin` are not expanded and the value type is preserved.
    mod user_path {
        use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
        use windows_sys::Win32::System::Registry::{
            RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_EXPAND_SZ, REG_SZ,
            REG_VALUE_TYPE,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE};

        struct Key(HKEY);

        impl Drop for Key {
            fn drop(&mut self) {
                unsafe { RegCloseKey(self.0) };
            }
        }

        fn wide(text: &str) -> Vec<u16> {
            text.encode_utf16().chain(Some(0)).collect()
        }

        /// Modifies the user's PATH with `change` (`None` = no change), then notifies
        /// Explorer so terminals opened after this get the new PATH.
        pub fn update(change: impl FnOnce(&str) -> Option<String>) -> Result<(), String> {
            let key = open()?;
            let (path, kind) = read(&key)?;
            let Some(updated) = change(&path) else { return Ok(()) };
            write(&key, &updated, kind)?;
            let parameter = wide("Environment");
            let mut result = 0usize;
            unsafe { SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, 0, parameter.as_ptr() as isize, SMTO_ABORTIFHUNG, 5000, &mut result) };
            Ok(())
        }

        fn open() -> Result<Key, String> {
            let name = wide("Environment");
            let mut key: HKEY = std::ptr::null_mut();
            let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, name.as_ptr(), 0, KEY_QUERY_VALUE | KEY_SET_VALUE, &mut key) };
            if status != ERROR_SUCCESS {
                return Err(format!("Cannot open the user environment in the registry (error {status})"));
            }
            Ok(Key(key))
        }

        fn read(key: &Key) -> Result<(String, REG_VALUE_TYPE), String> {
            let name = wide("Path");
            let (mut kind, mut size) = (0, 0u32);
            let status = unsafe { RegQueryValueExW(key.0, name.as_ptr(), std::ptr::null(), &mut kind, std::ptr::null_mut(), &mut size) };
            if status == ERROR_FILE_NOT_FOUND {
                return Ok((String::new(), REG_EXPAND_SZ));
            }
            if status != ERROR_SUCCESS {
                return Err(format!("Cannot read the user PATH (error {status})"));
            }
            let mut buffer = vec![0u16; (size as usize).div_ceil(2) + 1];
            let mut bytes = (buffer.len() * 2) as u32;
            let status = unsafe { RegQueryValueExW(key.0, name.as_ptr(), std::ptr::null(), &mut kind, buffer.as_mut_ptr().cast(), &mut bytes) };
            if status != ERROR_SUCCESS {
                return Err(format!("Cannot read the user PATH (error {status})"));
            }
            if kind != REG_SZ && kind != REG_EXPAND_SZ {
                return Err("The user PATH in the registry is not a text value; left it unchanged".to_string());
            }
            buffer.truncate(bytes as usize / 2);
            while buffer.last() == Some(&0) {
                buffer.pop();
            }
            // Better to refuse than to write back a corrupted PATH.
            let path = String::from_utf16(&buffer).map_err(|_| "The user PATH contains invalid characters; left it unchanged".to_string())?;
            Ok((path, kind))
        }

        fn write(key: &Key, value: &str, kind: REG_VALUE_TYPE) -> Result<(), String> {
            let name = wide("Path");
            let status = if value.is_empty() {
                unsafe { RegDeleteValueW(key.0, name.as_ptr()) }
            } else {
                let data = wide(value);
                unsafe { RegSetValueExW(key.0, name.as_ptr(), 0, kind, data.as_ptr().cast(), (data.len() * 2) as u32) }
            };
            if status != ERROR_SUCCESS {
                return Err(format!("Cannot update the user PATH (error {status})"));
            }
            Ok(())
        }
    }
}

#[cfg(unix)]
mod platform {
    use std::io::ErrorKind;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::{Path, PathBuf};

    pub fn install(app_dir: &Path) -> Result<String, String> {
        let target = target()?;
        let script = super::unix_script(&executable(app_dir));
        if let Ok(existing) = std::fs::symlink_metadata(&target) {
            if existing.is_dir() || !is_ours(&target) {
                return Err(format!("{} already exists and was not created by OnCode; remove it first", target.display()));
            }
        }
        if let Err(error) = write(&target, &script) {
            if error.kind() != ErrorKind::PermissionDenied || !cfg!(target_os = "macos") {
                return Err(format!("Cannot write {}: {error}", target.display()));
            }
            let temp = std::env::temp_dir().join(format!("oncode-shell-command-{}", std::process::id()));
            std::fs::write(&temp, &script).map_err(|e| format!("Cannot write {}: {e}", temp.display()))?;
            let dir = target.parent().unwrap_or(Path::new("/"));
            let (dir, temp_q, target_q) = (quote(dir), quote(&temp), quote(&target));
            let result = as_administrator(&format!("mkdir -p {dir} && cp {temp_q} {target_q} && chmod 755 {target_q}"));
            let _ = std::fs::remove_file(&temp);
            result?;
        }
        let mut message = format!("Installed the 'oncode' command at {}.", target.display());
        let dir = target.parent().unwrap_or(Path::new("/"));
        if !std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|entry| entry == dir)) {
            message.push_str(&format!(" Add {} to PATH to use it.", dir.display()));
        }
        Ok(message)
    }

    pub fn uninstall(_app_dir: &Path) -> Result<String, String> {
        let target = target()?;
        if std::fs::symlink_metadata(&target).is_err() {
            return Ok("The 'oncode' command is not installed.".to_string());
        }
        if !is_ours(&target) {
            return Err(format!("{} was not created by OnCode; left it in place", target.display()));
        }
        match std::fs::remove_file(&target) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::PermissionDenied && cfg!(target_os = "macos") => as_administrator(&format!("rm -f {}", quote(&target)))?,
            Err(error) => return Err(format!("Cannot remove {}: {error}", target.display())),
        }
        Ok(format!("Removed the 'oncode' command ({}).", target.display()))
    }

    /// Script location. `/usr/local/bin` is on the default macOS PATH; `~/.local/bin` on most Linux distros.
    fn target() -> Result<PathBuf, String> {
        if cfg!(target_os = "macos") {
            return Ok(PathBuf::from("/usr/local/bin/oncode"));
        }
        let home = std::env::var_os("HOME").filter(|home| !home.is_empty()).ok_or("HOME is not set")?;
        Ok(PathBuf::from(home).join(".local/bin/oncode"))
    }

    /// The binary the script runs. An AppImage runs from a temporary folder that disappears when the
    /// app is closed, so the `.AppImage` file itself is used.
    fn executable(app_dir: &Path) -> PathBuf {
        std::env::var_os("APPIMAGE").map(PathBuf::from).filter(|path| path.is_file()).unwrap_or_else(|| app_dir.join("oncode-app"))
    }

    fn is_ours(path: &Path) -> bool {
        std::fs::read(path).is_ok_and(|content| String::from_utf8_lossy(&content).contains(super::MARKER))
    }

    fn write(target: &Path, script: &str) -> std::io::Result<()> {
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // The old link is removed first so the write does not follow the link to another file.
        match std::fs::remove_file(target) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        std::fs::write(target, script)?;
        std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o755))
    }

    fn quote(path: &Path) -> String {
        super::sh_quote(&path.to_string_lossy())
    }

    /// Runs a shell command as administrator through the macOS permission dialog.
    fn as_administrator(command: &str) -> Result<(), String> {
        let escaped = command.replace('\\', "\\\\").replace('"', "\\\"");
        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(format!("do shell script \"{escaped}\" with administrator privileges"))
            .output()
            .map_err(|e| format!("Cannot run osascript: {e}"))?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("-128") {
            return Err("Cancelled".to_string());
        }
        Err(format!("Administrator command failed: {}", stderr.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_the_folder_to_path_once() {
        assert_eq!(path_with(r"C:\Windows;%USERPROFILE%\bin", r"C:\OnCode\bin").as_deref(), Some(r"C:\Windows;%USERPROFILE%\bin;C:\OnCode\bin"));
        assert_eq!(path_with(r"C:\Windows;", r"C:\OnCode\bin").as_deref(), Some(r"C:\Windows;C:\OnCode\bin"));
        assert_eq!(path_with("", r"C:\OnCode\bin").as_deref(), Some(r"C:\OnCode\bin"));
        // Already present, including with a different letter case or trailing slash.
        assert_eq!(path_with(r"C:\Windows;c:\oncode\BIN\", r"C:\OnCode\bin"), None);
    }

    #[test]
    fn removes_only_the_folder_from_path() {
        assert_eq!(path_without(r"C:\Windows;C:\OnCode\bin;%USERPROFILE%\bin", r"C:\OnCode\bin").as_deref(), Some(r"C:\Windows;%USERPROFILE%\bin"));
        assert_eq!(path_without(r"c:\oncode\bin\", r"C:\OnCode\bin").as_deref(), Some(""));
        assert_eq!(path_without(r"C:\Windows;C:\OnCode\binary", r"C:\OnCode\bin"), None);
    }

    #[test]
    fn scripts_run_the_app_in_the_background() {
        let cmd = windows_script();
        assert!(cmd.contains(MARKER));
        assert!(cmd.contains(r#"start "" "%~dp0..\oncode-app.exe" %*"#));

        let sh = unix_script(Path::new("/opt/On Code's/oncode-app"));
        assert!(sh.starts_with("#!/bin/sh\n"));
        assert!(sh.contains(MARKER));
        assert!(sh.contains(r#"nohup '/opt/On Code'\''s/oncode-app' "$@" >/dev/null 2>&1 &"#));
    }
}
