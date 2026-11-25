//! Perintah `oncode` di terminal, seperti `code` milik VS Code: `oncode .` membuka folder saat ini.
//!
//! Binary aplikasi sengaja tidak bernama `oncode` (lihat `app/Cargo.toml`), jadi yang dipasang
//! adalah skrip kecil yang menjalankan `oncode-app` di latar belakang agar terminal langsung bebas:
//! - Windows: `<folder aplikasi>\bin\oncode.cmd`, lalu folder `bin` itu ditambahkan ke PATH
//!   pengguna. Installer memanggil ini saat memasang dan melepas aplikasi.
//! - macOS: `/usr/local/bin/oncode`, dengan izin administrator bila folder itu tidak bisa ditulis.
//! - Linux: `~/.local/bin/oncode` (paket `.deb` sudah memasang `/usr/bin/oncode` sendiri).
//!
//! Proses `oncode-app` yang baru meneruskan folder ke OnCode yang sudah berjalan bila ada (lihat
//! `app/src/single_instance.rs`).

use std::path::Path;
use std::process::ExitCode;

/// Penanda di dalam skrip: hanya skrip buatan OnCode yang boleh ditimpa atau dihapus.
const MARKER: &str = "Created by OnCode";

/// `oncode-helper install-shell-command <folder aplikasi>` dan `uninstall-shell-command`.
/// Pesan untuk pengguna dicetak ke stdout, galat ke stderr.
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

/// Isi `oncode.cmd`. `start` membuat terminal tidak menunggu aplikasi ditutup; path aplikasi
/// relatif terhadap skrip, sehingga isinya sama untuk semua lokasi pemasangan.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_script() -> String {
    format!("@echo off\r\nrem {MARKER}: opens a folder in OnCode, e.g. `oncode .`\r\nstart \"\" \"%~dp0..\\oncode-app.exe\" %*\r\n")
}

/// Isi skrip `oncode` untuk macOS dan Linux. `nohup ... &` melepas aplikasi dari terminal.
#[cfg_attr(not(unix), allow(dead_code))]
fn unix_script(exe: &Path) -> String {
    format!("#!/bin/sh\n# {MARKER}: opens a folder in OnCode, e.g. `oncode .`\nnohup {} \"$@\" >/dev/null 2>&1 &\n", sh_quote(&exe.to_string_lossy()))
}

#[cfg_attr(not(unix), allow(dead_code))]
fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// PATH (format Windows, dipisah `;`) dengan `dir` di ujungnya, atau `None` bila sudah ada.
#[cfg_attr(not(windows), allow(dead_code))]
fn path_with(path: &str, dir: &str) -> Option<String> {
    if path.split(';').any(|entry| same_entry(entry, dir)) {
        return None;
    }
    let path = path.trim_end_matches(';');
    Some(if path.is_empty() { dir.to_string() } else { format!("{path};{dir}") })
}

/// PATH tanpa `dir`, atau `None` bila memang tidak ada. Entri lain dibiarkan apa adanya.
#[cfg_attr(not(windows), allow(dead_code))]
fn path_without(path: &str, dir: &str) -> Option<String> {
    if !path.split(';').any(|entry| same_entry(entry, dir)) {
        return None;
    }
    Some(path.split(';').filter(|entry| !same_entry(entry, dir)).collect::<Vec<_>>().join(";"))
}

/// Path Windows tidak peka huruf besar-kecil; garis miring di akhir diabaikan.
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
        // Hanya terhapus bila kosong.
        let _ = std::fs::remove_dir(&bin);
        Ok("Removed the 'oncode' command from PATH.".to_string())
    }

    /// Nilai `Path` milik pengguna di registri (`HKCU\Environment`). Dibaca dan ditulis apa adanya:
    /// entri seperti `%USERPROFILE%\bin` tidak dikembangkan dan jenis nilainya dipertahankan.
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

        /// Mengubah PATH pengguna dengan `change` (`None` = tidak ada perubahan), lalu memberi tahu
        /// Explorer agar terminal yang dibuka setelah ini mendapat PATH baru.
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
            // Lebih baik menolak daripada menulis kembali PATH yang rusak.
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

    /// Lokasi skrip. `/usr/local/bin` ada di PATH bawaan macOS; `~/.local/bin` di kebanyakan distro Linux.
    fn target() -> Result<PathBuf, String> {
        if cfg!(target_os = "macos") {
            return Ok(PathBuf::from("/usr/local/bin/oncode"));
        }
        let home = std::env::var_os("HOME").filter(|home| !home.is_empty()).ok_or("HOME is not set")?;
        Ok(PathBuf::from(home).join(".local/bin/oncode"))
    }

    /// Binary yang dijalankan skrip. AppImage berjalan dari folder sementara yang hilang saat
    /// aplikasi ditutup, jadi yang dipakai berkas `.AppImage`-nya sendiri.
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
        // Tautan lama dihapus dulu agar penulisan tidak mengikuti tautan ke berkas lain.
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

    /// Menjalankan perintah shell sebagai administrator lewat dialog izin macOS.
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
        // Sudah ada, termasuk dengan huruf atau garis miring akhir yang berbeda.
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
