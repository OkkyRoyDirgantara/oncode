//! Lokasi direktori konfigurasi dan data aplikasi.
//!
//! Di v0.2 lokasi ini ditentukan Tauri dari identifier aplikasi; di sini diturunkan dengan
//! aturan yang sama agar `settings.json`, `keybindings.json`, dan `state.json` lama tetap terbaca.

use std::path::PathBuf;

/// Identifier aplikasi v0.2 (`tauri.conf.json`); menjadi nama folder di direktori konfigurasi dan data.
pub const APP_IDENTIFIER: &str = "dev.oncode.editor";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    /// Memuat `settings.json` dan `keybindings.json`.
    pub config_dir: PathBuf,
    /// Memuat `state.json`.
    pub data_dir: PathBuf,
}

impl Dirs {
    /// Lokasi bawaan, sama persis dengan yang dipakai versi Tauri:
    ///
    /// | OS | `config_dir` | `data_dir` |
    /// |---|---|---|
    /// | Linux | `$XDG_CONFIG_HOME` atau `~/.config` | `$XDG_DATA_HOME` atau `~/.local/share` |
    /// | Windows | `%APPDATA%` | `%APPDATA%` |
    /// | macOS | `~/Library/Application Support` | `~/Library/Application Support` |
    ///
    /// masing-masing ditambah `dev.oncode.editor`. Tidak ada variabel lingkungan khusus OnCode;
    /// di Linux `XDG_CONFIG_HOME`/`XDG_DATA_HOME` (harus path absolut) tetap dihormati seperti dulu.
    /// `None` bila OS tidak bisa memberi tahu folder home pengguna.
    pub fn default_for_app() -> Option<Dirs> {
        Self::from_bases(dirs::config_dir(), dirs::data_dir())
    }

    /// Kedua direktori di bawah satu folder (`<base>/config` dan `<base>/data`); untuk uji.
    pub fn under(base: impl Into<PathBuf>) -> Dirs {
        let base = base.into();
        Dirs { config_dir: base.join("config"), data_dir: base.join("data") }
    }

    fn from_bases(config_base: Option<PathBuf>, data_base: Option<PathBuf>) -> Option<Dirs> {
        Some(Dirs { config_dir: config_base?.join(APP_IDENTIFIER), data_dir: data_base?.join(APP_IDENTIFIER) })
    }

    pub fn settings_path(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }

    pub fn keybindings_path(&self) -> PathBuf {
        self.config_dir.join("keybindings.json")
    }

    pub fn state_path(&self) -> PathBuf {
        self.data_dir.join("state.json")
    }
}

/// Folder home pengguna.
pub fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_the_v02_identifier_to_both_bases() {
        let dirs = Dirs::from_bases(Some(PathBuf::from("/home/a/.config")), Some(PathBuf::from("/home/a/.local/share"))).unwrap();
        assert_eq!(dirs.config_dir, PathBuf::from("/home/a/.config/dev.oncode.editor"));
        assert_eq!(dirs.data_dir, PathBuf::from("/home/a/.local/share/dev.oncode.editor"));
        assert_eq!(dirs.settings_path(), PathBuf::from("/home/a/.config/dev.oncode.editor/settings.json"));
        assert_eq!(dirs.keybindings_path(), PathBuf::from("/home/a/.config/dev.oncode.editor/keybindings.json"));
        assert_eq!(dirs.state_path(), PathBuf::from("/home/a/.local/share/dev.oncode.editor/state.json"));
        // Tanpa folder dasar (home tidak diketahui) tidak ada lokasi bawaan.
        assert_eq!(Dirs::from_bases(None, Some(PathBuf::from("/x"))), None);
        assert_eq!(Dirs::from_bases(Some(PathBuf::from("/x")), None), None);
    }

    #[test]
    fn explicit_dirs_override_the_defaults() {
        let dirs = Dirs::under("/tmp/uji");
        assert_eq!(dirs.config_dir, PathBuf::from("/tmp/uji/config"));
        assert_eq!(dirs.data_dir, PathBuf::from("/tmp/uji/data"));
    }

    /// Nilai lingkungan hanya dibaca, tidak diubah, supaya aman dijalankan paralel dengan uji lain.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_defaults_follow_xdg_like_the_tauri_version() {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
        let xdg = |key: &str, fallback: &str| {
            std::env::var_os(key).map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home.join(fallback))
        };
        let dirs = Dirs::default_for_app().expect("lokasi bawaan");
        assert_eq!(dirs.config_dir, xdg("XDG_CONFIG_HOME", ".config").join("dev.oncode.editor"));
        assert_eq!(dirs.data_dir, xdg("XDG_DATA_HOME", ".local/share").join("dev.oncode.editor"));
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn config_and_data_share_one_folder_on_windows_and_macos() {
        let dirs = Dirs::default_for_app().expect("lokasi bawaan");
        assert_eq!(dirs.config_dir, dirs.data_dir);
        assert!(dirs.config_dir.ends_with("dev.oncode.editor"));
    }
}
