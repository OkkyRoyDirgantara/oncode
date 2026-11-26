//! Locations of the app's config and data directories.
//!
//! In v0.2 these locations were determined by Tauri from the app identifier; here they are derived
//! with the same rules so the old `settings.json`, `keybindings.json`, and `state.json` stay readable.

use std::path::PathBuf;

/// The v0.2 app identifier (`tauri.conf.json`); becomes the folder name in the config and data directories.
pub const APP_IDENTIFIER: &str = "dev.oncode.editor";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    /// Holds `settings.json` and `keybindings.json`.
    pub config_dir: PathBuf,
    /// Holds `state.json`.
    pub data_dir: PathBuf,
}

impl Dirs {
    /// Default locations, exactly the same as what the Tauri version used:
    ///
    /// | OS | `config_dir` | `data_dir` |
    /// |---|---|---|
    /// | Linux | `$XDG_CONFIG_HOME` or `~/.config` | `$XDG_DATA_HOME` or `~/.local/share` |
    /// | Windows | `%APPDATA%` | `%APPDATA%` |
    /// | macOS | `~/Library/Application Support` | `~/Library/Application Support` |
    ///
    /// each with `dev.oncode.editor` appended. There are no OnCode-specific environment variables;
    /// on Linux `XDG_CONFIG_HOME`/`XDG_DATA_HOME` (must be absolute paths) are still honored as before.
    /// `None` if the OS cannot tell the user's home folder.
    pub fn default_for_app() -> Option<Dirs> {
        Self::from_bases(dirs::config_dir(), dirs::data_dir())
    }

    /// Both directories under one folder (`<base>/config` and `<base>/data`); for tests.
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

/// The user's home folder.
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
        // Without base folders (home unknown) there is no default location.
        assert_eq!(Dirs::from_bases(None, Some(PathBuf::from("/x"))), None);
        assert_eq!(Dirs::from_bases(Some(PathBuf::from("/x")), None), None);
    }

    #[test]
    fn explicit_dirs_override_the_defaults() {
        let dirs = Dirs::under("/tmp/test");
        assert_eq!(dirs.config_dir, PathBuf::from("/tmp/test/config"));
        assert_eq!(dirs.data_dir, PathBuf::from("/tmp/test/data"));
    }

    /// Environment values are only read, never changed, so it is safe to run in parallel with other tests.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_defaults_follow_xdg_like_the_tauri_version() {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
        let xdg = |key: &str, fallback: &str| {
            std::env::var_os(key).map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home.join(fallback))
        };
        let dirs = Dirs::default_for_app().expect("default location");
        assert_eq!(dirs.config_dir, xdg("XDG_CONFIG_HOME", ".config").join("dev.oncode.editor"));
        assert_eq!(dirs.data_dir, xdg("XDG_DATA_HOME", ".local/share").join("dev.oncode.editor"));
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn config_and_data_share_one_folder_on_windows_and_macos() {
        let dirs = Dirs::default_for_app().expect("default location");
        assert_eq!(dirs.config_dir, dirs.data_dir);
        assert!(dirs.config_dir.ends_with("dev.oncode.editor"));
    }

    /// The window's app_id is APP_IDENTIFIER. The dock on Linux finds OnCode's name and icon by matching
    /// it to the installed `.desktop` entry's StartupWMClass.
    #[test]
    fn linux_desktop_entry_matches_the_window_app_id() {
        let entry = include_str!("../../../app/packaging/linux/oncode.desktop");
        let expected = format!("StartupWMClass={APP_IDENTIFIER}");
        assert!(entry.lines().any(|line| line == expected), "app/packaging/linux/oncode.desktop needs `{expected}`");
    }
}
