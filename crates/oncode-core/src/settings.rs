//! Application settings: few, typed, with sensible defaults.

use std::collections::BTreeMap;
use std::path::Path;

use oncode_git::PullStrategy;
use oncode_lsp::ServerSpec;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorSettings {
    pub font_family: String,
    pub font_size: u32,
    pub tab_size: u32,
    pub insert_spaces: bool,
    pub word_wrap: bool,
    pub line_numbers: bool,
    /// Show the blame of the cursor line at the end of that line (inline blame, like GitLens).
    pub inline_blame: bool,
    /// Format the file every time it is saved (needs the Formatters extension or a language server).
    pub format_on_save: bool,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            font_family: "\"JetBrains Mono\", \"Fira Code\", \"Cascadia Code\", Menlo, Consolas, monospace".into(),
            font_size: 14,
            tab_size: 4,
            insert_spaces: true,
            word_wrap: false,
            line_numbers: true,
            inline_blame: true,
            format_on_save: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum AutoSave {
    #[default]
    Off,
    /// Save when the editor loses focus.
    OnFocusChange,
    /// Save one second after the last keystroke.
    AfterDelay,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FilesSettings {
    pub auto_save: AutoSave,
}

/// The color theme. `dark` and `light` are the default themes (their names in `settings.json` date from
/// before there were other themes); `system` switches between them with the operating system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Theme {
    #[default]
    System,
    Dark,
    OneDark,
    Dracula,
    Nord,
    GruvboxDark,
    TokyoNight,
    CatppuccinMocha,
    Light,
    OneLight,
    SolarizedLight,
    CatppuccinLatte,
}

impl Theme {
    /// Every theme, in the order they are offered: System, the dark themes, then the light ones.
    pub const ALL: [Theme; 12] = [
        Theme::System,
        Theme::Dark,
        Theme::OneDark,
        Theme::Dracula,
        Theme::Nord,
        Theme::GruvboxDark,
        Theme::TokyoNight,
        Theme::CatppuccinMocha,
        Theme::Light,
        Theme::OneLight,
        Theme::SolarizedLight,
        Theme::CatppuccinLatte,
    ];

    /// The value in `settings.json`.
    pub const fn id(self) -> &'static str {
        match self {
            Theme::System => "system",
            Theme::Dark => "dark",
            Theme::OneDark => "oneDark",
            Theme::Dracula => "dracula",
            Theme::Nord => "nord",
            Theme::GruvboxDark => "gruvboxDark",
            Theme::TokyoNight => "tokyoNight",
            Theme::CatppuccinMocha => "catppuccinMocha",
            Theme::Light => "light",
            Theme::OneLight => "oneLight",
            Theme::SolarizedLight => "solarizedLight",
            Theme::CatppuccinLatte => "catppuccinLatte",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|theme| theme.id() == id)
    }

    /// The name shown to the user, which is also the theme's name in the UI's theme registry.
    pub const fn label(self) -> &'static str {
        match self {
            Theme::System => "System",
            Theme::Dark => "Default Dark",
            Theme::OneDark => "One Dark",
            Theme::Dracula => "Dracula",
            Theme::Nord => "Nord",
            Theme::GruvboxDark => "Gruvbox Dark",
            Theme::TokyoNight => "Tokyo Night",
            Theme::CatppuccinMocha => "Catppuccin Mocha",
            Theme::Light => "Default Light",
            Theme::OneLight => "One Light",
            Theme::SolarizedLight => "Solarized Light",
            Theme::CatppuccinLatte => "Catppuccin Latte",
        }
    }

    /// Whether this is a dark theme; `None` for System, which follows the operating system.
    pub const fn is_dark(self) -> Option<bool> {
        match self {
            Theme::System => None,
            Theme::Dark | Theme::OneDark | Theme::Dracula | Theme::Nord | Theme::GruvboxDark | Theme::TokyoNight | Theme::CatppuccinMocha => Some(true),
            Theme::Light | Theme::OneLight | Theme::SolarizedLight | Theme::CatppuccinLatte => Some(false),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum StartupView {
    /// Source Control if the folder is a repository, otherwise Explorer.
    #[default]
    Scm,
    Explorer,
}

/// The side of the window where the activity bar and sidebar are placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum SidebarPosition {
    #[default]
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkbenchSettings {
    pub theme: Theme,
    pub startup_view: StartupView,
    pub sidebar_position: SidebarPosition,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitSettings {
    /// Path to git; empty = look it up in PATH.
    pub path: Option<String>,
    /// 0 = never fetch automatically.
    pub auto_fetch_minutes: u32,
    pub pull_strategy: PullStrategy,
    pub confirm_force_push: bool,
}

impl Default for GitSettings {
    fn default() -> Self {
        Self { path: None, auto_fetch_minutes: 5, pull_strategy: PullStrategy::FfOnly, confirm_force_push: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSettings {
    /// Empty = the user's default shell.
    pub shell: Option<String>,
    /// Comma-separated font list for the terminal only; empty = editor font.
    pub font_family: Option<String>,
    pub font_size: Option<u32>,
    /// Line height as a percentage of the font size (100–200); empty = 120.
    pub line_height: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LspSettings {
    pub enabled: bool,
    /// Overrides or adds to the default server list, per language id.
    pub servers: BTreeMap<String, ServerSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ForgeSettings {
    /// OAuth app Client ID for the device flow login on github.com / gitlab.com, also the
    /// default for other servers that have no Client ID of their own. Empty = the build's default Client ID.
    pub github_client_id: Option<String>,
    pub gitlab_client_id: Option<String>,
    /// Per-server Client ID (key: origin, e.g. `https://git.example.com`). An OAuth app is only
    /// known to the server where it was registered, so each self-hosted server needs its own.
    pub client_ids: BTreeMap<String, String>,
}

/// Additional features that can be turned on or off one by one (Settings → Extensions).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionSettings {
    /// Markdown preview (`Ctrl+Shift+V`).
    pub markdown_preview: bool,
    /// External per-language formatters (Prettier, rustfmt, gofmt, ruff, dart format, …).
    pub formatters: bool,
    /// Highlighting for additional languages (Kotlin, Dart, C#, PHP, Lua, Swift, GraphQL, …).
    pub extra_languages: bool,
    /// Go to Definition and Ctrl+click without a language server: finds definitions across the whole project.
    pub symbol_search: bool,
}

impl Default for ExtensionSettings {
    fn default() -> Self {
        Self { markdown_preview: true, formatters: true, extra_languages: true, symbol_search: true }
    }
}

/// The route to Claude for "Generate Commit Message".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum AiProvider {
    /// API if there is an API key; without a key, Claude Code CLI if installed.
    #[default]
    Auto,
    /// Claude Code CLI (`claude -p`) with the user's Claude Code login; no API key.
    ClaudeCode,
    /// Claude API with an Anthropic API key.
    Api,
    /// Custom endpoint (`ai.customBaseUrl`): another provider, a gateway, or a local model.
    Custom,
}

/// Format of the custom API endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ApiFormat {
    /// OpenAI-style Chat Completions: OpenAI, OpenRouter, Groq, DeepSeek, Ollama, LM Studio, LiteLLM, …
    #[default]
    #[serde(rename = "openai")]
    OpenAi,
    /// Anthropic-style Messages API: a gateway or proxy that forwards to Claude.
    #[serde(rename = "anthropic")]
    Anthropic,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSettings {
    pub provider: AiProvider,
    /// Claude model for "Generate Commit Message". For the Claude Code CLI an alias
    /// such as `opus` or `sonnet` is also allowed.
    pub model: String,
    /// Claude API origin, e.g. an organization gateway/proxy. Empty = `https://api.anthropic.com`.
    pub base_url: Option<String>,
    /// Format of the custom API endpoint (`custom` provider).
    pub custom_format: ApiFormat,
    /// Custom endpoint address, e.g. `https://openrouter.ai/api/v1` or `http://localhost:11434/v1`.
    pub custom_base_url: Option<String>,
    /// Model on the custom endpoint, e.g. `gpt-4.1-mini` or `qwen2.5-coder`.
    pub custom_model: Option<String>,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self { provider: AiProvider::Auto, model: oncode_ai::DEFAULT_MODEL.to_string(), base_url: None, custom_format: ApiFormat::OpenAi, custom_base_url: None, custom_model: None }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub editor: EditorSettings,
    pub files: FilesSettings,
    pub workbench: WorkbenchSettings,
    pub git: GitSettings,
    pub terminal: TerminalSettings,
    pub lsp: LspSettings,
    pub forge: ForgeSettings,
    pub ai: AiSettings,
    pub extensions: ExtensionSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            editor: EditorSettings::default(),
            files: FilesSettings::default(),
            workbench: WorkbenchSettings::default(),
            git: GitSettings::default(),
            terminal: TerminalSettings::default(),
            lsp: LspSettings { enabled: true, servers: BTreeMap::new() },
            forge: ForgeSettings::default(),
            ai: AiSettings::default(),
            extensions: ExtensionSettings::default(),
        }
    }
}

/// Settings a project's `.oncode/settings.json` may not set: they choose a program to run
/// (language servers, the terminal shell, git) or where credentials and code are sent (AI,
/// forge). That file comes from the repository, and a pull request from a fork can change it,
/// so only the user's own settings decide these.
const USER_ONLY: &[&[&str]] = &[&["lsp", "servers"], &["terminal", "shell"], &["git", "path"], &["ai"], &["forge"]];

/// Removes the [`USER_ONLY`] keys from a project overlay.
fn without_user_only(mut overlay: serde_json::Value) -> serde_json::Value {
    for path in USER_ONLY {
        let Some((last, parents)) = path.split_last() else { continue };
        let parent = parents.iter().try_fold(&mut overlay, |node, key| node.get_mut(*key));
        if let Some(serde_json::Value::Object(map)) = parent {
            map.remove(*last);
        }
    }
    overlay
}

/// Merges `overlay` into `base`: objects are merged per key, other values overwrite.
pub fn merge_json(base: &mut serde_json::Value, overlay: &serde_json::Value) {
    match (base, overlay) {
        (serde_json::Value::Object(base), serde_json::Value::Object(overlay)) => {
            for (key, value) in overlay {
                merge_json(base.entry(key.clone()).or_insert(serde_json::Value::Null), value);
            }
        }
        (base, overlay) => *base = overlay.clone(),
    }
}

/// What reading a settings file found.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedSettings {
    pub settings: Settings,
    /// Why some or all of the file was not used, in words for the user: a syntax error (nothing in the file
    /// is used) or values of the wrong type (only those keys keep their default). `None` if all of it was used.
    pub problem: Option<String>,
    /// The file exists but is not readable JSON: writing the settings over it would lose what it holds.
    pub unreadable: bool,
}

impl Settings {
    /// Reads the settings. Keys missing from the file use the default value; a missing or corrupt file
    /// yields all defaults. See [`Settings::read`] for what was wrong with the file.
    pub fn load(path: &Path) -> Self {
        Self::read(path).settings
    }

    /// Reads the settings like [`Settings::load`], and says what in the file could not be used. Comments
    /// and trailing commas are accepted (people copy settings from editors that allow them), and a value
    /// of the wrong type only drops that key instead of the whole file.
    pub fn read(path: &Path) -> LoadedSettings {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return LoadedSettings { settings: Self::default(), problem: None, unreadable: false },
            Err(error) => {
                return LoadedSettings { settings: Self::default(), problem: Some(format!("{} could not be read: {error}.", file_name(path))), unreadable: true };
            }
        };
        let file = match parse_jsonc(&text) {
            Ok(file) => file,
            Err(error) => {
                let problem = format!("{} has an error on line {}, column {}: {error}. Its settings are not used until it is fixed.", file_name(path), error.line(), error.column());
                return LoadedSettings { settings: Self::default(), problem: Some(problem), unreadable: true };
            }
        };
        let (settings, rejected) = Self::default().merged_with(&file);
        let problem = (!rejected.is_empty()).then(|| format!("{} has values of the wrong type, which are ignored: {}.", file_name(path), rejected.join(", ")));
        LoadedSettings { settings, problem, unreadable: false }
    }

    /// Applies the `overlay` values onto these settings. A value that does not fit (a wrong type) is
    /// skipped, and its key is returned, so one mistake does not throw away the rest of the file.
    fn merged_with(&self, overlay: &serde_json::Value) -> (Self, Vec<String>) {
        let Ok(mut base) = serde_json::to_value(self) else { return (self.clone(), Vec::new()) };
        let mut whole = base.clone();
        merge_json(&mut whole, overlay);
        if let Ok(settings) = serde_json::from_value(whole) {
            return (settings, Vec::new());
        }
        // Something does not fit: apply the values one at a time and keep those that do.
        let mut rejected = Vec::new();
        for (path, value) in leaves(overlay) {
            let mut candidate = base.clone();
            merge_json(&mut candidate, &nested(&path, value));
            if serde_json::from_value::<Settings>(candidate.clone()).is_ok() {
                base = candidate;
            } else {
                rejected.push(path.join("."));
            }
        }
        (serde_json::from_value(base).unwrap_or_else(|_| self.clone()), rejected)
    }

    /// Writes the settings through a temporary file next to `path`, then renames it over `path`, so a reader
    /// never sees half a file. The temporary name is unique, so two saves at once cannot mix their content.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = path.with_extension(format!("json.{}-{unique}.tmp", std::process::id()));
        if let Err(error) = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, path)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(error);
        }
        Ok(())
    }

    /// Effective settings for a project: the user's settings overridden by the project's
    /// `.oncode/settings.json`, except the [`USER_ONLY`] keys. Only called for trusted folders.
    pub fn with_project(&self, project_root: &Path) -> Self {
        let project = project_root.join(".oncode").join("settings.json");
        match std::fs::read_to_string(project).ok().and_then(|t| parse_jsonc(&t).ok()) {
            Some(overlay) => self.merged_with(&without_user_only(overlay)).0,
            None => self.clone(),
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// Every value in `value` that is not an object, with the keys leading to it. An empty object has none.
fn leaves(value: &serde_json::Value) -> Vec<(Vec<String>, &serde_json::Value)> {
    fn walk<'a>(value: &'a serde_json::Value, path: &mut Vec<String>, out: &mut Vec<(Vec<String>, &'a serde_json::Value)>) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, value) in map {
                    path.push(key.clone());
                    walk(value, path, out);
                    path.pop();
                }
            }
            _ => out.push((path.clone(), value)),
        }
    }
    let mut out = Vec::new();
    walk(value, &mut Vec::new(), &mut out);
    out
}

/// `{"a": {"b": value}}` for the path `["a", "b"]`.
fn nested(path: &[String], value: &serde_json::Value) -> serde_json::Value {
    path.iter().rev().fold(value.clone(), |inner, key| serde_json::Value::Object([(key.clone(), inner)].into_iter().collect()))
}

/// Parses JSON that may contain `//` and `/* */` comments and trailing commas (JSONC, as in VS Code's
/// settings). The error positions refer to the original text: comments are blanked out, not removed.
pub fn parse_jsonc(text: &str) -> Result<serde_json::Value, serde_json::Error> {
    serde_json::from_str(&strip_jsonc(text))
}

/// Replaces comments with spaces (keeping line breaks) and drops commas that are followed only by
/// whitespace or comments before a closing `}` or `]`. Strings are left alone.
fn strip_jsonc(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    // Index in `out` of a comma that is so far followed only by whitespace and comments.
    let mut pending_comma: Option<usize> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' => {
                pending_comma = None;
                out.push(c);
                i += 1;
                while i < chars.len() {
                    out.push(chars[i]);
                    match chars[i] {
                        '\\' if i + 1 < chars.len() => {
                            out.push(chars[i + 1]);
                            i += 2;
                        }
                        '"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
            }
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    out.push(' ');
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                out.extend([' ', ' ']);
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    out.push(if chars[i] == '\n' { '\n' } else { ' ' });
                    i += 1;
                }
                if i < chars.len() {
                    out.extend([' ', ' ']);
                    i += 2;
                }
            }
            ',' => {
                pending_comma = Some(out.len());
                out.push(c);
                i += 1;
            }
            '}' | ']' => {
                if let Some(comma) = pending_comma.take() {
                    out[comma] = ' ';
                }
                out.push(c);
                i += 1;
            }
            c if c.is_whitespace() => {
                out.push(c);
                i += 1;
            }
            _ => {
                pending_comma = None;
                out.push(c);
                i += 1;
            }
        }
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_partial_file_falls_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(Settings::load(&path), Settings::default());
        std::fs::write(&path, r#"{ "editor": { "fontSize": 18 } }"#).unwrap();
        let s = Settings::load(&path);
        assert_eq!(s.editor.font_size, 18);
        assert_eq!(s.editor.tab_size, 4);
        assert!(s.lsp.enabled);
        // A wrongly typed value only loses that value, not the rest of the file.
        std::fs::write(&path, r#"{ "editor": { "fontSize": "large", "tabSize": 2 }, "workbench": { "theme": "dark" } }"#).unwrap();
        let loaded = Settings::read(&path);
        assert_eq!(loaded.settings.editor.font_size, Settings::default().editor.font_size);
        assert_eq!(loaded.settings.editor.tab_size, 2);
        assert_eq!(loaded.settings.workbench.theme, Theme::Dark);
        assert!(!loaded.unreadable);
        assert!(loaded.problem.as_deref().is_some_and(|p| p.contains("editor.fontSize") && !p.contains("tabSize")), "{loaded:?}");
        // A syntax error: nothing in the file can be used, and it must not be written over unnoticed.
        std::fs::write(&path, "{ \"editor\": { \"fontSize\": 18 }\n\"x\"").unwrap();
        let loaded = Settings::read(&path);
        assert_eq!(loaded.settings, Settings::default());
        assert!(loaded.unreadable);
        assert!(loaded.problem.as_deref().is_some_and(|p| p.contains("line 2")), "{loaded:?}");
    }

    /// Settings copied from editors that allow comments and trailing commas (VS Code) are read.
    #[test]
    fn comments_and_trailing_commas_are_accepted() {
        let text = r#"{
            // The font.
            "editor": { "fontSize": 18, /* inline */ "fontFamily": "Fira // Code, /* not a comment */", },
            "terminal": { "shell": "C:\\bin\\sh\"x\",]" },
            "lsp": { "servers": {}, },
        }"#;
        let value = parse_jsonc(text).unwrap();
        assert_eq!(value["editor"]["fontSize"], 18);
        assert_eq!(value["editor"]["fontFamily"], "Fira // Code, /* not a comment */");
        assert_eq!(value["terminal"]["shell"], "C:\\bin\\sh\"x\",]");
        // An error position still points into the original text.
        let error = parse_jsonc("{\n  // note\n  \"a\": x\n}").unwrap_err();
        assert_eq!(error.line(), 3);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg/settings.json");
        let mut s = Settings::default();
        s.git.pull_strategy = PullStrategy::Rebase;
        s.workbench.theme = Theme::Dracula;
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path), s);
    }

    /// `id` is the spelling serde writes, so the settings page and `settings.json` agree; the values from
    /// before there were other themes (`system`, `dark`, `light`) still mean the same.
    #[test]
    fn theme_ids_match_serde_and_old_values_still_load() {
        for theme in Theme::ALL {
            assert_eq!(serde_json::to_value(theme).unwrap(), theme.id(), "{theme:?}");
            assert_eq!(Theme::from_id(theme.id()), Some(theme));
        }
        assert_eq!(Theme::from_id("solarized"), None);
        let labels: std::collections::BTreeSet<&str> = Theme::ALL.iter().map(|theme| theme.label()).collect();
        assert_eq!(labels.len(), Theme::ALL.len());

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        for (old, theme) in [("system", Theme::System), ("dark", Theme::Dark), ("light", Theme::Light)] {
            std::fs::write(&path, format!(r#"{{ "workbench": {{ "theme": "{old}" }} }}"#)).unwrap();
            assert_eq!(Settings::load(&path).workbench.theme, theme);
        }
    }

    #[test]
    fn project_settings_override_user_settings() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".oncode")).unwrap();
        std::fs::write(
            dir.path().join(".oncode/settings.json"),
            r#"{ "editor": { "tabSize": 2 }, "git": { "autoFetchMinutes": 0 }, "terminal": { "fontSize": 12 }, "lsp": { "enabled": false } }"#,
        )
        .unwrap();
        let mut user = Settings::default();
        user.editor.font_size = 16;
        let effective = user.with_project(dir.path());
        assert_eq!(effective.editor.tab_size, 2);
        assert_eq!(effective.editor.font_size, 16, "keys that are not overridden stay from the user");
        assert_eq!(effective.git.auto_fetch_minutes, 0);
        assert_eq!(effective.terminal.font_size, Some(12));
        assert!(!effective.lsp.enabled);
    }

    /// A repository (or a pull request from a fork) must not choose the programs OnCode runs.
    #[test]
    fn project_settings_cannot_choose_programs_or_credential_targets() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".oncode")).unwrap();
        std::fs::write(
            dir.path().join(".oncode/settings.json"),
            r#"{
                "editor": { "tabSize": 2 },
                "lsp": { "servers": { "rust": { "command": "./evil.sh", "args": ["-x"], "env": [["NODE_OPTIONS", "--require ./evil.js"]] } } },
                "terminal": { "shell": "./evil.sh", "fontSize": 12 },
                "git": { "path": "./evil.sh", "pullStrategy": "rebase" },
                "ai": { "baseUrl": "https://evil.example", "customBaseUrl": "https://evil.example/v1" },
                "forge": { "githubClientId": "evil", "clientIds": { "https://github.com": "evil" } }
            }"#,
        )
        .unwrap();
        let mut user = Settings::default();
        user.lsp.servers.insert("go".into(), ServerSpec { command: "gopls-user".into(), args: Vec::new(), env: Vec::new() });
        user.terminal.shell = Some("/bin/zsh".into());

        let effective = user.with_project(dir.path());
        assert_eq!(effective.lsp.servers, user.lsp.servers, "the user's own servers stay, the project's are ignored");
        assert_eq!(effective.terminal.shell.as_deref(), Some("/bin/zsh"));
        assert_eq!(effective.git.path, None);
        assert_eq!(effective.ai, user.ai);
        assert_eq!(effective.forge, user.forge);
        // The rest of the same file still applies.
        assert_eq!(effective.editor.tab_size, 2);
        assert_eq!(effective.terminal.font_size, Some(12));
        assert_eq!(effective.git.pull_strategy, PullStrategy::Rebase);
    }
}
