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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Theme {
    #[default]
    System,
    Dark,
    Light,
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

impl Settings {
    /// Reads the settings. Keys missing from the file use the default value;
    /// a missing or corrupt file yields all defaults.
    pub fn load(path: &Path) -> Self {
        let Some(file) = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else {
            return Self::default();
        };
        Self::default().merged_with(&file)
    }

    /// Applies the `overlay` values onto these settings; if the result is invalid, these settings are kept.
    fn merged_with(&self, overlay: &serde_json::Value) -> Self {
        let Ok(mut base) = serde_json::to_value(self) else { return self.clone() };
        merge_json(&mut base, overlay);
        serde_json::from_value(base).unwrap_or_else(|_| self.clone())
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)
    }

    /// Effective settings for a project: the user's settings overridden by the project's
    /// `.oncode/settings.json`. Only called for trusted folders.
    pub fn with_project(&self, project_root: &Path) -> Self {
        let project = project_root.join(".oncode").join("settings.json");
        match std::fs::read_to_string(project).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) {
            Some(overlay) => self.merged_with(&overlay),
            None => self.clone(),
        }
    }
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
        // A wrongly typed value does not corrupt the other settings: the file is ignored entirely.
        std::fs::write(&path, r#"{ "editor": { "fontSize": "large" } }"#).unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg/settings.json");
        let mut s = Settings::default();
        s.git.pull_strategy = PullStrategy::Rebase;
        s.workbench.theme = Theme::Light;
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path), s);
    }

    #[test]
    fn project_settings_override_user_settings() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".oncode")).unwrap();
        std::fs::write(
            dir.path().join(".oncode/settings.json"),
            r#"{ "editor": { "tabSize": 2 }, "lsp": { "servers": { "rust": { "command": "ra-custom" } } } }"#,
        )
        .unwrap();
        let mut user = Settings::default();
        user.editor.font_size = 16;
        let effective = user.with_project(dir.path());
        assert_eq!(effective.editor.tab_size, 2);
        assert_eq!(effective.editor.font_size, 16, "keys that are not overridden stay from the user");
        assert_eq!(effective.lsp.servers["rust"].command, "ra-custom");
    }
}
