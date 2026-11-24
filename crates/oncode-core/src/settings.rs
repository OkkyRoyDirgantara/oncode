//! Pengaturan aplikasi: sedikit, bertipe, dengan nilai bawaan yang masuk akal.

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
    /// Tampilkan anotasi blame di baris kursor.
    pub inline_blame: bool,
    /// Format berkas setiap kali disimpan (butuh ekstensi Formatters atau language server).
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
            inline_blame: false,
            format_on_save: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum AutoSave {
    #[default]
    Off,
    /// Simpan saat editor kehilangan fokus.
    OnFocusChange,
    /// Simpan satu detik setelah ketikan terakhir.
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
    /// Source Control bila folder adalah repositori, selain itu Explorer.
    #[default]
    Scm,
    Explorer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkbenchSettings {
    pub theme: Theme,
    pub startup_view: StartupView,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitSettings {
    /// Path ke git; kosong = cari di PATH.
    pub path: Option<String>,
    /// 0 = tidak pernah fetch otomatis.
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
    /// Kosong = shell bawaan pengguna.
    pub shell: Option<String>,
    pub font_size: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LspSettings {
    pub enabled: bool,
    /// Menimpa atau menambah daftar server bawaan, per id bahasa.
    pub servers: BTreeMap<String, ServerSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ForgeSettings {
    /// Client ID aplikasi OAuth untuk login device flow di github.com / gitlab.com, sekaligus
    /// bawaan untuk server lain yang belum punya Client ID sendiri. Kosong = Client ID bawaan build.
    pub github_client_id: Option<String>,
    pub gitlab_client_id: Option<String>,
    /// Client ID per server (kunci: origin, mis. `https://git.example.com`). Aplikasi OAuth hanya
    /// dikenal oleh server tempat ia didaftarkan, jadi tiap server self-hosted perlu miliknya sendiri.
    pub client_ids: BTreeMap<String, String>,
}

/// Fitur tambahan yang bisa dinyalakan atau dimatikan satu per satu (Settings → Extensions).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionSettings {
    /// Pratinjau Markdown (`Ctrl+Shift+V`).
    pub markdown_preview: bool,
    /// Formatter eksternal per bahasa (Prettier, rustfmt, gofmt, ruff, dart format, …).
    pub formatters: bool,
    /// Highlight untuk bahasa tambahan (Kotlin, Dart, C#, PHP, Lua, Swift, GraphQL, …).
    pub extra_languages: bool,
    /// Go to Definition dan Ctrl+klik tanpa language server: mencari definisi di seluruh proyek.
    pub symbol_search: bool,
}

impl Default for ExtensionSettings {
    fn default() -> Self {
        Self { markdown_preview: true, formatters: true, extra_languages: true, symbol_search: true }
    }
}

/// Jalur ke Claude untuk "Generate Commit Message".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum AiProvider {
    /// API bila ada API key; tanpa key, Claude Code CLI bila terpasang.
    #[default]
    Auto,
    /// Claude Code CLI (`claude -p`) dengan login Claude Code pengguna; tanpa API key.
    ClaudeCode,
    /// Claude API dengan API key Anthropic.
    Api,
    /// Endpoint kustom (`ai.customBaseUrl`): penyedia lain, gateway, atau model lokal.
    Custom,
}

/// Format API endpoint kustom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ApiFormat {
    /// Chat Completions ala OpenAI: OpenAI, OpenRouter, Groq, DeepSeek, Ollama, LM Studio, LiteLLM, …
    #[default]
    #[serde(rename = "openai")]
    OpenAi,
    /// Messages API ala Anthropic: gateway atau proxy yang meneruskan ke Claude.
    #[serde(rename = "anthropic")]
    Anthropic,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSettings {
    pub provider: AiProvider,
    /// Model Claude untuk "Generate Commit Message". Untuk Claude Code CLI boleh juga alias
    /// seperti `opus` atau `sonnet`.
    pub model: String,
    /// Origin Claude API, mis. gateway/proxy organisasi. Kosong = `https://api.anthropic.com`.
    pub base_url: Option<String>,
    /// Format API endpoint kustom (provider `custom`).
    pub custom_format: ApiFormat,
    /// Alamat endpoint kustom, mis. `https://openrouter.ai/api/v1` atau `http://localhost:11434/v1`.
    pub custom_base_url: Option<String>,
    /// Model di endpoint kustom, mis. `gpt-4.1-mini` atau `qwen2.5-coder`.
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

/// Menggabungkan `overlay` ke `base`: objek digabung per kunci, nilai lain menimpa.
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
    /// Membaca pengaturan. Kunci yang tidak ada di berkas memakai nilai bawaan;
    /// berkas hilang atau rusak menghasilkan nilai bawaan seluruhnya.
    pub fn load(path: &Path) -> Self {
        let Some(file) = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else {
            return Self::default();
        };
        Self::default().merged_with(&file)
    }

    /// Nilai `overlay` ditimpakan ke pengaturan ini; bila hasilnya tidak sah, pengaturan ini dipertahankan.
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

    /// Pengaturan efektif untuk sebuah proyek: pengaturan pengguna ditimpa
    /// `.oncode/settings.json` milik proyek. Hanya dipanggil untuk folder yang dipercaya.
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
        // Nilai bertipe salah tidak merusak pengaturan lain: berkas diabaikan seluruhnya.
        std::fs::write(&path, r#"{ "editor": { "fontSize": "besar" } }"#).unwrap();
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
        assert_eq!(effective.editor.font_size, 16, "kunci yang tidak ditimpa tetap dari pengguna");
        assert_eq!(effective.lsp.servers["rust"].command, "ra-custom");
    }
}
