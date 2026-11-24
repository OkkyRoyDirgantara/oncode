//! PATH dari shell login pengguna, untuk aplikasi yang dibuka dari Finder, Dock, atau launcher.
//!
//! Aplikasi GUI di macOS mewarisi PATH minimal dari launchd (`/usr/bin:/bin:/usr/sbin:/sbin`),
//! bukan PATH yang disusun `~/.zprofile`/`~/.zshrc`. Tanpa langkah ini program di `~/.local/bin`,
//! `/opt/homebrew/bin`, `~/.cargo/bin`, dst. (Claude Code CLI, language server, formatter) tidak
//! ditemukan. PATH shell login dibaca sekali saat mulai lalu digabung ke PATH proses, sehingga
//! pencarian program dan semua proses anak ikut memakainya.

/// Menggabungkan PATH shell login ke PATH proses.
///
/// Harus dipanggil paling awal di `main`, sebelum runtime atau thread lain dibuat: mengubah
/// variabel lingkungan tidak aman selama thread lain bisa membacanya.
#[cfg(unix)]
pub fn import_login_path() {
    use std::io::IsTerminal as _;

    // Dibuka dari terminal: PATH sudah milik shell pengguna (termasuk venv dan sejenisnya).
    if std::io::stdin().is_terminal() {
        return;
    }
    let Some(shell) = login_shell() else { return };
    let Some(login) = unix::read_login_path(&shell) else { return };
    let current = std::env::var_os("PATH").unwrap_or_default();
    if let Some(merged) = merge(std::ffi::OsStr::new(&login), &current) {
        std::env::set_var("PATH", merged);
    }
}

#[cfg(not(unix))]
pub fn import_login_path() {}

#[cfg(unix)]
fn login_shell() -> Option<String> {
    let shell = std::env::var("SHELL").ok().map(|shell| shell.trim().to_string()).filter(|shell| !shell.is_empty());
    // zsh adalah shell bawaan macOS bila launchd tidak memberi `SHELL`.
    shell.or_else(|| cfg!(target_os = "macos").then(|| "/bin/zsh".to_string()))
}

#[cfg(unix)]
mod unix {
    use std::io::Read as _;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    /// Batas waktu shell login; berkas rc yang macet (mis. `exec tmux`) tidak boleh menahan aplikasi.
    const TIMEOUT: Duration = Duration::from_secs(3);

    /// Menjalankan `$SHELL -l -i` (login dan interaktif: `~/.zprofile` dan `~/.zshrc` sama-sama
    /// dibaca, seperti terminal biasa) dan mengambil PATH-nya. `None` bila gagal atau terlalu lama.
    pub(super) fn read_login_path(shell: &str) -> Option<String> {
        let script = format!("echo {}; /usr/bin/env -0", super::MARKER);
        let mut command = Command::new(shell);
        command.args(["-l", "-i", "-c", &script]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
        // Dari home, agar alat per folder (direnv, `.nvmrc`) tidak ikut memengaruhi hasil.
        if let Some(home) = std::env::var_os("HOME") {
            command.current_dir(home);
        }
        let mut child = command.spawn().ok()?;
        let mut stdout = child.stdout.take()?;
        let (tx, rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut output = Vec::new();
            let _ = stdout.read_to_end(&mut output);
            let _ = tx.send(output);
        });
        let output = rx.recv_timeout(TIMEOUT);
        let _ = child.kill();
        let _ = child.wait();
        // Thread pembaca hanya ditunggu bila sudah selesai; setelah batas waktu ia dibiarkan
        // (pipa bisa tetap terbuka oleh proses latar yang dijalankan berkas rc).
        let output = output.ok()?;
        let _ = reader.join();
        super::parse_path(&String::from_utf8_lossy(&output))
    }
}

/// Penanda awal keluaran `env`: berkas rc boleh mencetak apa saja sebelumnya.
#[cfg(any(unix, test))]
const MARKER: &str = "__ONCODE_SHELL_ENV__";

/// Nilai PATH dari keluaran `echo MARKER; env -0`.
#[cfg(any(unix, test))]
fn parse_path(output: &str) -> Option<String> {
    let (_, env) = output.split_once(MARKER)?;
    env.trim_start_matches(['\r', '\n'])
        .split('\0')
        .find_map(|entry| entry.strip_prefix("PATH="))
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(String::from)
}

/// PATH shell login lebih dulu (urutan pilihan pengguna), lalu folder PATH proses yang belum ada.
#[cfg(any(unix, test))]
fn merge(login: &std::ffi::OsStr, current: &std::ffi::OsStr) -> Option<std::ffi::OsString> {
    let mut seen = std::collections::HashSet::new();
    let dirs: Vec<std::path::PathBuf> = std::env::split_paths(login)
        .chain(std::env::split_paths(current))
        .filter(|dir| !dir.as_os_str().is_empty())
        .filter(|dir| seen.insert(dir.clone()))
        .collect();
    if dirs.is_empty() {
        return None;
    }
    std::env::join_paths(dirs).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn parses_path_after_the_marker_and_ignores_rc_noise() {
        let output = format!("Selamat datang!\nPATH=/palsu\n{MARKER}\nHOME=/Users/a\0PATH=/Users/a/.local/bin:/usr/bin\0SHELL=/bin/zsh\0");
        assert_eq!(parse_path(&output).as_deref(), Some("/Users/a/.local/bin:/usr/bin"));
    }

    #[test]
    fn missing_marker_or_path_gives_none() {
        assert_eq!(parse_path("PATH=/usr/bin\0"), None);
        assert_eq!(parse_path(&format!("{MARKER}\nHOME=/a\0")), None);
        assert_eq!(parse_path(&format!("{MARKER}\nPATH=\0")), None);
    }

    #[cfg(unix)]
    #[test]
    fn login_path_comes_first_without_duplicates() {
        let merged = merge(OsStr::new("/Users/a/.local/bin:/opt/homebrew/bin:/usr/bin"), OsStr::new("/usr/bin:/bin:/usr/sbin:/sbin"));
        assert_eq!(merged.as_deref(), Some(OsStr::new("/Users/a/.local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin")));
        assert_eq!(merge(OsStr::new(""), OsStr::new("")), None);
    }

    /// Shell sungguhan: hasilnya memuat folder PATH sistem yang selalu ada.
    #[cfg(unix)]
    #[test]
    fn reads_path_from_a_real_login_shell() {
        let path = unix::read_login_path("/bin/sh").expect("PATH dari /bin/sh -l -i");
        assert!(std::env::split_paths(OsStr::new(&path)).any(|dir| dir == std::path::Path::new("/bin") || dir == std::path::Path::new("/usr/bin")), "{path}");
    }
}
