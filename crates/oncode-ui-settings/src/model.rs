//! Definisi kolom halaman pengaturan: label, penjelasan, cara membaca/menulis nilainya, dan
//! validasi. Formulir dirender dari daftar ini, sehingga pencarian, penanda "ditimpa proyek",
//! dan pengujian memakai satu sumber. Logika murni: tidak bergantung pada GPUI.

use oncode_core::settings::{AiProvider, ApiFormat, AutoSave, SidebarPosition, StartupView, Theme};
use oncode_core::Settings;
use oncode_git::PullStrategy;
use oncode_lsp::ServerSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SectionId {
    Editor,
    Files,
    Appearance,
    Git,
    Terminal,
    Lsp,
    Accounts,
    Ai,
    Extensions,
    /// Tanpa kolom: hanya tombol untuk membuka `keybindings.json`.
    Keyboard,
}

impl SectionId {
    pub fn title(self) -> &'static str {
        match self {
            SectionId::Editor => "Editor",
            SectionId::Files => "Files",
            SectionId::Appearance => "Appearance",
            SectionId::Git => "Git",
            SectionId::Terminal => "Terminal",
            SectionId::Lsp => "Language Servers",
            SectionId::Accounts => "Accounts",
            SectionId::Ai => "AI",
            SectionId::Extensions => "Extensions",
            SectionId::Keyboard => "Keyboard Shortcuts",
        }
    }

    /// Nama pendek untuk id elemen.
    pub fn key(self) -> &'static str {
        match self {
            SectionId::Editor => "editor",
            SectionId::Files => "files",
            SectionId::Appearance => "appearance",
            SectionId::Git => "git",
            SectionId::Terminal => "terminal",
            SectionId::Lsp => "lsp",
            SectionId::Accounts => "accounts",
            SectionId::Ai => "ai",
            SectionId::Extensions => "extensions",
            SectionId::Keyboard => "keyboard",
        }
    }
}

/// Urutan bagian di halaman.
pub const SECTIONS: [SectionId; 10] = [
    SectionId::Editor,
    SectionId::Files,
    SectionId::Appearance,
    SectionId::Git,
    SectionId::Terminal,
    SectionId::Lsp,
    SectionId::Accounts,
    SectionId::Ai,
    SectionId::Extensions,
    SectionId::Keyboard,
];

pub struct TextSpec {
    pub placeholder: &'static str,
    pub monospace: bool,
    pub get: fn(&Settings) -> String,
    /// Mengembalikan pesan galat, atau `None` bila sah.
    pub validate: fn(&str) -> Option<&'static str>,
    pub set: fn(&mut Settings, &str),
}

pub struct NumberSpec {
    pub min: u32,
    pub max: u32,
    /// Boleh dikosongkan (nilai `None`).
    pub optional: bool,
    pub placeholder: &'static str,
    pub unit: &'static str,
    pub get: fn(&Settings) -> Option<u32>,
    /// `None` pada kolom yang tidak opsional membiarkan nilai apa adanya.
    pub set: fn(&mut Settings, Option<u32>),
}

pub struct CheckboxSpec {
    pub get: fn(&Settings) -> bool,
    pub set: fn(&mut Settings, bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChoiceOption {
    /// Nilai seperti di `settings.json`, mis. `ffOnly`.
    pub value: &'static str,
    pub label: &'static str,
}

pub struct SelectSpec {
    pub options: &'static [ChoiceOption],
    pub get: fn(&Settings) -> &'static str,
    /// Nilai yang tidak dikenal diabaikan.
    pub set: fn(&mut Settings, &str),
}

pub enum FieldKind {
    Text(TextSpec),
    Number(NumberSpec),
    Checkbox(CheckboxSpec),
    Select(SelectSpec),
}

pub struct Field {
    /// Kunci bertitik seperti di `settings.json`, mis. `editor.fontSize`.
    pub id: &'static str,
    pub section: SectionId,
    pub label: &'static str,
    pub description: &'static str,
    /// Kata tambahan untuk pencarian.
    pub keywords: &'static str,
    pub kind: FieldKind,
}

/// Nilai sebuah kolom, untuk dibandingkan dan dipindahkan antar-`Settings`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldValue {
    Text(String),
    Number(Option<u32>),
    Bool(bool),
    Choice(&'static str),
}

impl Field {
    pub fn value(&self, settings: &Settings) -> FieldValue {
        match &self.kind {
            FieldKind::Text(spec) => FieldValue::Text((spec.get)(settings)),
            FieldKind::Number(spec) => FieldValue::Number((spec.get)(settings)),
            FieldKind::Checkbox(spec) => FieldValue::Bool((spec.get)(settings)),
            FieldKind::Select(spec) => FieldValue::Choice((spec.get)(settings)),
        }
    }

    /// Menulis `value` ke `settings`; nilai berjenis lain diabaikan.
    pub fn apply(&self, settings: &mut Settings, value: &FieldValue) {
        match (&self.kind, value) {
            (FieldKind::Text(spec), FieldValue::Text(text)) => (spec.set)(settings, text),
            (FieldKind::Number(spec), FieldValue::Number(number)) => (spec.set)(settings, *number),
            (FieldKind::Checkbox(spec), FieldValue::Bool(on)) => (spec.set)(settings, *on),
            (FieldKind::Select(spec), FieldValue::Choice(choice)) => (spec.set)(settings, choice),
            _ => {}
        }
    }
}

/// Teks kosong atau hanya spasi disimpan sebagai `None`.
fn nullable(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn has_control(text: &str) -> bool {
    text.chars().any(|c| c.is_ascii_control())
}

fn validate_path(text: &str) -> Option<&'static str> {
    if has_control(text) {
        return Some("The path cannot contain control characters");
    }
    if text.chars().count() > 4096 {
        return Some("The path is too long");
    }
    None
}

fn validate_model(text: &str) -> Option<&'static str> {
    let value = text.trim();
    if value.is_empty() {
        return Some("Enter a model name, e.g. claude-opus-5-5");
    }
    if value.chars().any(char::is_whitespace) || has_control(value) {
        return Some("A model name cannot contain spaces");
    }
    if value.chars().count() > 100 {
        return Some("This is too long for a model name");
    }
    None
}

fn validate_client_id(text: &str) -> Option<&'static str> {
    let value = text.trim();
    if value.is_empty() {
        return None;
    }
    if value.chars().any(char::is_whitespace) || has_control(value) {
        return Some("A Client ID cannot contain spaces");
    }
    if value.chars().count() > 200 {
        return Some("This is too long for a Client ID");
    }
    None
}

fn validate_font_family(text: &str) -> Option<&'static str> {
    if text.trim().is_empty() {
        return Some("Enter at least one font name");
    }
    if has_control(text) || text.contains([';', '{', '}']) {
        return Some("A font list cannot contain ; { }");
    }
    if text.chars().count() > 500 {
        return Some("The font list is too long");
    }
    None
}

pub fn validate_server_command(text: &str) -> Option<&'static str> {
    if text.trim().is_empty() {
        return Some("Enter a command");
    }
    validate_path(text)
}

const AUTO_SAVE: &[ChoiceOption] =
    &[ChoiceOption { value: "off", label: "Off" }, ChoiceOption { value: "onFocusChange", label: "On focus change" }, ChoiceOption { value: "afterDelay", label: "After delay" }];
const THEME: &[ChoiceOption] = &[ChoiceOption { value: "system", label: "System" }, ChoiceOption { value: "dark", label: "Dark" }, ChoiceOption { value: "light", label: "Light" }];
const STARTUP_VIEW: &[ChoiceOption] = &[ChoiceOption { value: "scm", label: "Source Control" }, ChoiceOption { value: "explorer", label: "Explorer" }];
const SIDEBAR_POSITION: &[ChoiceOption] = &[ChoiceOption { value: "left", label: "Left" }, ChoiceOption { value: "right", label: "Right" }];
const PULL_STRATEGY: &[ChoiceOption] =
    &[ChoiceOption { value: "ffOnly", label: "Fast-forward only" }, ChoiceOption { value: "merge", label: "Merge" }, ChoiceOption { value: "rebase", label: "Rebase" }];

const AI_PROVIDER: &[ChoiceOption] = &[
    ChoiceOption { value: "auto", label: "Automatic" },
    ChoiceOption { value: "claudeCode", label: "Claude Code CLI" },
    ChoiceOption { value: "api", label: "Anthropic API" },
    ChoiceOption { value: "custom", label: "Custom endpoint" },
];

const API_FORMAT: &[ChoiceOption] = &[ChoiceOption { value: "openai", label: "OpenAI-compatible" }, ChoiceOption { value: "anthropic", label: "Anthropic Messages" }];

fn validate_endpoint(text: &str) -> Option<&'static str> {
    let value = text.trim();
    if value.is_empty() {
        return None;
    }
    if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Some("An address cannot contain spaces");
    }
    if !(value.starts_with("https://") || value.starts_with("http://")) {
        return Some("Start the address with https:// or http://");
    }
    if value.len() > 2048 {
        return Some("This address is too long");
    }
    None
}

fn validate_custom_model(text: &str) -> Option<&'static str> {
    let value = text.trim();
    if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Some("A model name cannot contain spaces");
    }
    if value.chars().count() > 200 {
        return Some("This is too long for a model name");
    }
    None
}

pub static FIELDS: [Field; 30] = [
    Field {
        id: "editor.fontFamily",
        section: SectionId::Editor,
        label: "Font family",
        description: "Comma-separated font list for the editor and terminal; the first installed font is used.",
        keywords: "typeface monospace",
        kind: FieldKind::Text(TextSpec {
            placeholder: "",
            monospace: true,
            get: |s| s.editor.font_family.clone(),
            validate: validate_font_family,
            set: |s, text| s.editor.font_family = text.trim().to_string(),
        }),
    },
    Field {
        id: "editor.fontSize",
        section: SectionId::Editor,
        label: "Font size",
        description: "Editor font size in pixels.",
        keywords: "",
        kind: FieldKind::Number(NumberSpec {
            min: 8,
            max: 40,
            optional: false,
            placeholder: "",
            unit: "px",
            get: |s| Some(s.editor.font_size),
            set: |s, value| s.editor.font_size = value.unwrap_or(s.editor.font_size),
        }),
    },
    Field {
        id: "editor.tabSize",
        section: SectionId::Editor,
        label: "Tab size",
        description: "Number of columns a tab occupies, and the indent width when inserting spaces.",
        keywords: "indent indentation",
        kind: FieldKind::Number(NumberSpec {
            min: 1,
            max: 16,
            optional: false,
            placeholder: "",
            unit: "",
            get: |s| Some(s.editor.tab_size),
            set: |s, value| s.editor.tab_size = value.unwrap_or(s.editor.tab_size),
        }),
    },
    Field {
        id: "editor.insertSpaces",
        section: SectionId::Editor,
        label: "Insert spaces",
        description: "Indent with spaces when pressing Tab instead of inserting a tab character.",
        keywords: "indent tabs",
        kind: FieldKind::Checkbox(CheckboxSpec { get: |s| s.editor.insert_spaces, set: |s, value| s.editor.insert_spaces = value }),
    },
    Field {
        id: "editor.wordWrap",
        section: SectionId::Editor,
        label: "Word wrap",
        description: "Wrap long lines at the edge of the editor instead of scrolling horizontally.",
        keywords: "",
        kind: FieldKind::Checkbox(CheckboxSpec { get: |s| s.editor.word_wrap, set: |s, value| s.editor.word_wrap = value }),
    },
    Field {
        id: "editor.lineNumbers",
        section: SectionId::Editor,
        label: "Line numbers",
        description: "Show line numbers in the editor gutter.",
        keywords: "gutter",
        kind: FieldKind::Checkbox(CheckboxSpec { get: |s| s.editor.line_numbers, set: |s, value| s.editor.line_numbers = value }),
    },
    Field {
        id: "editor.inlineBlame",
        section: SectionId::Editor,
        label: "Inline blame",
        description: "Show who last changed the current line, and when, at the end of that line.",
        keywords: "git annotation author",
        kind: FieldKind::Checkbox(CheckboxSpec { get: |s| s.editor.inline_blame, set: |s, value| s.editor.inline_blame = value }),
    },
    Field {
        id: "editor.formatOnSave",
        section: SectionId::Editor,
        label: "Format on save",
        description: "Format the file every time you save it, with the language's formatter or language server.",
        keywords: "prettier format beautify",
        kind: FieldKind::Checkbox(CheckboxSpec { get: |s| s.editor.format_on_save, set: |s, value| s.editor.format_on_save = value }),
    },
    Field {
        id: "files.autoSave",
        section: SectionId::Files,
        label: "Auto save",
        description: "Save changed files automatically: never, when the window loses focus, or one second after you stop typing.",
        keywords: "",
        kind: FieldKind::Select(SelectSpec {
            options: AUTO_SAVE,
            get: |s| match s.files.auto_save {
                AutoSave::Off => "off",
                AutoSave::OnFocusChange => "onFocusChange",
                AutoSave::AfterDelay => "afterDelay",
            },
            set: |s, value| {
                s.files.auto_save = match value {
                    "off" => AutoSave::Off,
                    "onFocusChange" => AutoSave::OnFocusChange,
                    "afterDelay" => AutoSave::AfterDelay,
                    _ => return,
                }
            },
        }),
    },
    Field {
        id: "workbench.theme",
        section: SectionId::Appearance,
        label: "Theme",
        description: "Colour theme. System follows the light or dark setting of your operating system.",
        keywords: "color colour dark light",
        kind: FieldKind::Select(SelectSpec {
            options: THEME,
            get: |s| match s.workbench.theme {
                Theme::System => "system",
                Theme::Dark => "dark",
                Theme::Light => "light",
            },
            set: |s, value| {
                s.workbench.theme = match value {
                    "system" => Theme::System,
                    "dark" => Theme::Dark,
                    "light" => Theme::Light,
                    _ => return,
                }
            },
        }),
    },
    Field {
        id: "workbench.startupView",
        section: SectionId::Appearance,
        label: "Startup view",
        description: "Sidebar view shown when a folder is opened. Source Control applies only when the folder is a Git repository.",
        keywords: "sidebar scm explorer",
        kind: FieldKind::Select(SelectSpec {
            options: STARTUP_VIEW,
            get: |s| match s.workbench.startup_view {
                StartupView::Scm => "scm",
                StartupView::Explorer => "explorer",
            },
            set: |s, value| {
                s.workbench.startup_view = match value {
                    "scm" => StartupView::Scm,
                    "explorer" => StartupView::Explorer,
                    _ => return,
                }
            },
        }),
    },
    Field {
        id: "workbench.sidebarPosition",
        section: SectionId::Appearance,
        label: "Sidebar position",
        description: "Side of the window for the activity bar and sidebar (Explorer, Source Control, Search, …). Also available as \"View: Move Sidebar Left/Right\".",
        keywords: "sidebar left right activity bar layout side",
        kind: FieldKind::Select(SelectSpec {
            options: SIDEBAR_POSITION,
            get: |s| match s.workbench.sidebar_position {
                SidebarPosition::Left => "left",
                SidebarPosition::Right => "right",
            },
            set: |s, value| {
                s.workbench.sidebar_position = match value {
                    "left" => SidebarPosition::Left,
                    "right" => SidebarPosition::Right,
                    _ => return,
                }
            },
        }),
    },
    Field {
        id: "git.path",
        section: SectionId::Git,
        label: "Path to git",
        description: "Full path of the git program. Leave empty to find git on PATH. Takes effect after restarting OnCode.",
        keywords: "executable binary",
        kind: FieldKind::Text(TextSpec {
            placeholder: "Found on PATH",
            monospace: true,
            get: |s| s.git.path.clone().unwrap_or_default(),
            validate: validate_path,
            set: |s, text| s.git.path = nullable(text),
        }),
    },
    Field {
        id: "git.autoFetchMinutes",
        section: SectionId::Git,
        label: "Auto-fetch interval",
        description: "Fetch from the remotes in the background every this many minutes. 0 never fetches automatically.",
        keywords: "",
        kind: FieldKind::Number(NumberSpec {
            min: 0,
            max: 1440,
            optional: false,
            placeholder: "",
            unit: "minutes",
            get: |s| Some(s.git.auto_fetch_minutes),
            set: |s, value| s.git.auto_fetch_minutes = value.unwrap_or(s.git.auto_fetch_minutes),
        }),
    },
    Field {
        id: "git.pullStrategy",
        section: SectionId::Git,
        label: "Pull strategy",
        description: "How Pull combines remote commits with yours. Fast-forward only never creates a merge commit and stops if the branches have diverged.",
        keywords: "merge rebase ff-only",
        kind: FieldKind::Select(SelectSpec {
            options: PULL_STRATEGY,
            get: |s| match s.git.pull_strategy {
                PullStrategy::FfOnly => "ffOnly",
                PullStrategy::Merge => "merge",
                PullStrategy::Rebase => "rebase",
            },
            set: |s, value| {
                s.git.pull_strategy = match value {
                    "ffOnly" => PullStrategy::FfOnly,
                    "merge" => PullStrategy::Merge,
                    "rebase" => PullStrategy::Rebase,
                    _ => return,
                }
            },
        }),
    },
    Field {
        id: "git.confirmForcePush",
        section: SectionId::Git,
        label: "Confirm force push",
        description: "Ask before a force push (with lease) overwrites a branch on the remote.",
        keywords: "",
        kind: FieldKind::Checkbox(CheckboxSpec { get: |s| s.git.confirm_force_push, set: |s, value| s.git.confirm_force_push = value }),
    },
    Field {
        id: "terminal.shell",
        section: SectionId::Terminal,
        label: "Shell",
        description: "Program started in new terminals. Leave empty to use your default shell.",
        keywords: "",
        kind: FieldKind::Text(TextSpec {
            placeholder: "Default shell",
            monospace: true,
            get: |s| s.terminal.shell.clone().unwrap_or_default(),
            validate: validate_path,
            set: |s, text| s.terminal.shell = nullable(text),
        }),
    },
    Field {
        id: "terminal.fontSize",
        section: SectionId::Terminal,
        label: "Font size",
        description: "Terminal font size in pixels. Leave empty to use the editor font size.",
        keywords: "",
        kind: FieldKind::Number(NumberSpec {
            min: 8,
            max: 40,
            optional: true,
            placeholder: "Editor size",
            unit: "px",
            get: |s| s.terminal.font_size,
            set: |s, value| s.terminal.font_size = value,
        }),
    },
    Field {
        id: "lsp.enabled",
        section: SectionId::Lsp,
        label: "Enable language servers",
        description: "Start a language server for completion, diagnostics and navigation when a file of its language is opened (trusted folders only).",
        keywords: "lsp intellisense completion diagnostics",
        kind: FieldKind::Checkbox(CheckboxSpec { get: |s| s.lsp.enabled, set: |s, value| s.lsp.enabled = value }),
    },
    Field {
        id: "forge.githubClientId",
        section: SectionId::Accounts,
        label: "GitHub OAuth Client ID",
        description: "Client ID of a GitHub OAuth app with device flow enabled, used for github.com and for servers without their own Client ID. GitHub Enterprise servers ask for theirs when you sign in; browser sign-in also works without one by creating a token.",
        keywords: "login sign in device flow",
        kind: FieldKind::Text(TextSpec {
            placeholder: "Not set",
            monospace: true,
            get: |s| s.forge.github_client_id.clone().unwrap_or_default(),
            validate: validate_client_id,
            set: |s, text| s.forge.github_client_id = nullable(text),
        }),
    },
    Field {
        id: "forge.gitlabClientId",
        section: SectionId::Accounts,
        label: "GitLab OAuth Client ID",
        description: "Application ID of a GitLab OAuth application, used for gitlab.com and for servers without their own Client ID. Self-hosted servers ask for theirs when you sign in; browser sign-in also works without one by creating a token.",
        keywords: "login sign in device flow",
        kind: FieldKind::Text(TextSpec {
            placeholder: "Not set",
            monospace: true,
            get: |s| s.forge.gitlab_client_id.clone().unwrap_or_default(),
            validate: validate_client_id,
            set: |s, text| s.forge.gitlab_client_id = nullable(text),
        }),
    },
    Field {
        id: "ai.provider",
        section: SectionId::Ai,
        label: "Provider",
        description: "How \"Generate Commit Message\" reaches an AI model. Claude Code CLI uses your Claude Code sign-in (including a Claude subscription) and needs no API key; Anthropic API needs a key, set with \"AI: Set Anthropic API Key…\". Automatic uses the API when a key is set, otherwise Claude Code when it is installed. Custom endpoint sends to the address below (another provider, a gateway, or a local model). The staged diff is sent to the chosen service.",
        keywords: "ai claude code cli anthropic api key subscription login",
        kind: FieldKind::Select(SelectSpec {
            options: AI_PROVIDER,
            get: |s| match s.ai.provider {
                AiProvider::Auto => "auto",
                AiProvider::ClaudeCode => "claudeCode",
                AiProvider::Api => "api",
                AiProvider::Custom => "custom",
            },
            set: |s, value| {
                s.ai.provider = match value {
                    "auto" => AiProvider::Auto,
                    "claudeCode" => AiProvider::ClaudeCode,
                    "api" => AiProvider::Api,
                    "custom" => AiProvider::Custom,
                    _ => return,
                }
            },
        }),
    },
    Field {
        id: "ai.model",
        section: SectionId::Ai,
        label: "Claude model",
        description: "Model used by \"Generate Commit Message\" in Source Control, e.g. claude-opus-5-5. With Claude Code CLI, aliases such as opus or sonnet work too.",
        keywords: "ai claude anthropic commit message generate llm",
        kind: FieldKind::Text(TextSpec {
            placeholder: "claude-opus-5-5",
            monospace: true,
            get: |s| s.ai.model.clone(),
            validate: validate_model,
            set: |s, text| s.ai.model = text.trim().to_string(),
        }),
    },
    Field {
        id: "ai.customFormat",
        section: SectionId::Ai,
        label: "Custom endpoint format",
        description: "API format of the custom endpoint. OpenAI-compatible fits OpenAI, OpenRouter, Groq, DeepSeek, Ollama, LM Studio and LiteLLM; Anthropic Messages fits gateways in front of Claude.",
        keywords: "ai provider openai openrouter ollama lm studio groq deepseek gateway",
        kind: FieldKind::Select(SelectSpec {
            options: API_FORMAT,
            get: |s| match s.ai.custom_format {
                ApiFormat::OpenAi => "openai",
                ApiFormat::Anthropic => "anthropic",
            },
            set: |s, value| {
                s.ai.custom_format = match value {
                    "openai" => ApiFormat::OpenAi,
                    "anthropic" => ApiFormat::Anthropic,
                    _ => return,
                }
            },
        }),
    },
    Field {
        id: "ai.customBaseUrl",
        section: SectionId::Ai,
        label: "Custom endpoint address",
        description: "Base URL of the custom endpoint, up to /v1 for OpenAI-compatible ones, e.g. https://openrouter.ai/api/v1 or http://localhost:11434/v1 (Ollama). Its API key, if it needs one, is set with \"AI: Set Custom Endpoint API Key…\".",
        keywords: "ai provider domain url host openai openrouter ollama lm studio gateway",
        kind: FieldKind::Text(TextSpec {
            placeholder: "https://openrouter.ai/api/v1",
            monospace: true,
            get: |s| s.ai.custom_base_url.clone().unwrap_or_default(),
            validate: validate_endpoint,
            set: |s, text| s.ai.custom_base_url = nullable(text),
        }),
    },
    Field {
        id: "ai.customModel",
        section: SectionId::Ai,
        label: "Custom endpoint model",
        description: "Model name at the custom endpoint, e.g. gpt-4.1-mini, deepseek-chat, or qwen2.5-coder for Ollama.",
        keywords: "ai provider model openai ollama",
        kind: FieldKind::Text(TextSpec {
            placeholder: "Not set",
            monospace: true,
            get: |s| s.ai.custom_model.clone().unwrap_or_default(),
            validate: validate_custom_model,
            set: |s, text| s.ai.custom_model = nullable(text),
        }),
    },
    Field {
        id: "extensions.markdownPreview",
        section: SectionId::Extensions,
        label: "Markdown Preview",
        description: "Show a live preview of Markdown files (Ctrl+Shift+V, or Ctrl+K V to open it beside the editor).",
        keywords: "md readme render",
        kind: FieldKind::Checkbox(CheckboxSpec { get: |s| s.extensions.markdown_preview, set: |s, value| s.extensions.markdown_preview = value }),
    },
    Field {
        id: "extensions.formatters",
        section: SectionId::Extensions,
        label: "Formatters",
        description: "Format Document uses each language's own formatter when it is installed: Prettier, rustfmt, gofmt, ruff or black, dart format, ktlint, sql-formatter, clang-format. Otherwise the language server formats.",
        keywords: "prettier format beautify rustfmt gofmt black ruff",
        kind: FieldKind::Checkbox(CheckboxSpec { get: |s| s.extensions.formatters, set: |s, value| s.extensions.formatters = value }),
    },
    Field {
        id: "extensions.extraLanguages",
        section: SectionId::Extensions,
        label: "Extra languages",
        description: "Syntax highlighting for Kotlin, Dart (Flutter), C#, PHP, Lua, Swift, Scala, GraphQL, Protocol Buffers, Makefile and more. Applies to files opened after the change.",
        keywords: "syntax highlight kotlin flutter dart csharp php lua swift graphql",
        kind: FieldKind::Checkbox(CheckboxSpec { get: |s| s.extensions.extra_languages, set: |s, value| s.extensions.extra_languages = value }),
    },
    Field {
        id: "extensions.symbolSearch",
        section: SectionId::Extensions,
        label: "Go to Definition without a language server",
        description: "When no language server answers, Ctrl+click and F12 search the project for the definition of the symbol (functions, classes, variables).",
        keywords: "ctrl click definition jump function symbol navigation",
        kind: FieldKind::Checkbox(CheckboxSpec { get: |s| s.extensions.symbol_search, set: |s, value| s.extensions.symbol_search = value }),
    },
];

pub fn field_by_id(id: &str) -> Option<&'static Field> {
    FIELDS.iter().find(|field| field.id == id)
}

/// Mengurai isian angka: nilai yang sah, atau pesan galat.
pub fn parse_number(spec: &NumberSpec, text: &str) -> Result<Option<u32>, String> {
    let raw = text.trim();
    if raw.is_empty() {
        return if spec.optional { Ok(None) } else { Err("Enter a number".to_string()) };
    }
    if !raw.chars().all(|c| c.is_ascii_digit()) {
        return Err("Enter a whole number".to_string());
    }
    // Deretan digit yang terlalu panjang untuk `u32` pasti di luar rentang.
    match raw.parse::<u32>() {
        Ok(value) if (spec.min..=spec.max).contains(&value) => Ok(Some(value)),
        _ => Err(format!("Enter a value from {} to {}", spec.min, spec.max)),
    }
}

/// Nilai kolom teks atau angka sebagai teks, untuk kotak isian. Jenis lain menghasilkan teks kosong.
pub fn field_text(field: &Field, settings: &Settings) -> String {
    match &field.kind {
        FieldKind::Text(spec) => (spec.get)(settings),
        FieldKind::Number(spec) => (spec.get)(settings).map(|n| n.to_string()).unwrap_or_default(),
        _ => String::new(),
    }
}

/// Mengembalikan satu kolom ke nilai bawaannya.
pub fn apply_default(field: &Field, settings: &mut Settings, defaults: &Settings) {
    field.apply(settings, &field.value(defaults));
}

/// Nilai kolom ini berbeda dari nilai bawaan.
pub fn differs_from_default(field: &Field, settings: &Settings, defaults: &Settings) -> bool {
    field.value(settings) != field.value(defaults)
}

/// Nilai bawaan sebuah kolom sebagai teks untuk tooltip.
pub fn default_label(field: &Field, defaults: &Settings) -> String {
    match &field.kind {
        FieldKind::Checkbox(spec) => if (spec.get)(defaults) { "on" } else { "off" }.to_string(),
        FieldKind::Select(spec) => {
            let value = (spec.get)(defaults);
            spec.options.iter().find(|o| o.value == value).map(|o| o.label).unwrap_or(value).to_string()
        }
        FieldKind::Number(spec) => (spec.get)(defaults).map(|n| n.to_string()).unwrap_or_else(|| "empty".to_string()),
        FieldKind::Text(spec) => {
            let text = (spec.get)(defaults);
            if text.is_empty() {
                "empty".to_string()
            } else {
                text
            }
        }
    }
}

/// Jumlah hal yang berbeda dari bawaan: kolom formulir ditambah penimpaan language server.
pub fn count_non_default(settings: &Settings, defaults: &Settings) -> usize {
    let fields = FIELDS.iter().filter(|field| differs_from_default(field, settings, defaults)).count();
    let languages: std::collections::BTreeSet<&String> = settings.lsp.servers.keys().chain(defaults.lsp.servers.keys()).collect();
    let servers = languages.into_iter().filter(|language| settings.lsp.servers.get(*language) != defaults.lsp.servers.get(*language)).count();
    fields + servers
}

#[derive(Clone, Copy)]
pub struct OverrideContext<'a> {
    /// Folder proyek dipercaya (pengaturan proyek hanya berlaku di folder yang dipercaya).
    pub trusted: bool,
    /// Pengaturan pengguna yang tersimpan.
    pub user: &'a Settings,
    /// Pengaturan efektif jendela ini (pengguna + proyek).
    pub effective: Option<&'a Settings>,
}

/// Kolom ini sedang ditimpa oleh `.oncode/settings.json` proyek.
pub fn is_overridden(field: &Field, context: OverrideContext) -> bool {
    match context.effective {
        Some(effective) if context.trusted => field.value(context.user) != field.value(effective),
        _ => false,
    }
}

/// Server untuk bahasa ini ditimpa oleh pengaturan proyek.
pub fn is_server_overridden(language: &str, context: OverrideContext) -> bool {
    match context.effective {
        Some(effective) if context.trusted => context.user.lsp.servers.get(language) != effective.lsp.servers.get(language),
        _ => false,
    }
}

/// Kueri pencarian cocok bila setiap katanya muncul di salah satu teks.
pub fn matches_query(query: &str, texts: &[&str]) -> bool {
    let query = query.to_lowercase();
    let mut words = query.split_whitespace().peekable();
    if words.peek().is_none() {
        return true;
    }
    let haystack = texts.join("\n").to_lowercase();
    words.all(|word| haystack.contains(word))
}

pub fn field_matches(field: &Field, query: &str) -> bool {
    let options = match &field.kind {
        FieldKind::Select(spec) => spec.options.iter().map(|o| o.label).collect::<Vec<_>>().join(" "),
        _ => String::new(),
    };
    matches_query(query, &[field.label, field.description, field.id, field.section.title(), field.keywords, &options])
}

/// Memecah baris argumen seperti shell sederhana: spasi memisah, kutip tunggal/ganda mengelompokkan.
pub fn split_args(text: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match quote {
            Some(open) => {
                if ch == open {
                    quote = None;
                } else if ch == '\\' && open == '"' && matches!(chars.peek(), Some('"' | '\\')) {
                    current.extend(chars.next());
                } else {
                    current.push(ch);
                }
            }
            None if ch == '"' || ch == '\'' => {
                quote = Some(ch);
                started = true;
            }
            None if ch.is_whitespace() => {
                if started {
                    args.push(std::mem::take(&mut current));
                }
                current.clear();
                started = false;
            }
            None => {
                current.push(ch);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return Err("A quote is not closed".to_string());
    }
    if started {
        args.push(current);
    }
    Ok(args)
}

/// Kebalikan [`split_args`]: argumen yang memuat spasi atau kutip dibungkus kutip ganda.
pub fn join_args(args: &[String]) -> String {
    args.iter()
        .map(|arg| {
            if arg.is_empty() || arg.chars().any(|c| c.is_whitespace() || c == '"' || c == '\'') {
                let mut quoted = String::with_capacity(arg.len() + 2);
                quoted.push('"');
                for c in arg.chars() {
                    if c == '"' || c == '\\' {
                        quoted.push('\\');
                    }
                    quoted.push(c);
                }
                quoted.push('"');
                quoted
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Menetapkan atau menghapus (`spec = None`) penimpaan server untuk satu bahasa.
pub fn set_server_override(settings: &mut Settings, language: &str, spec: Option<ServerSpec>) {
    match spec {
        Some(spec) => {
            settings.lsp.servers.insert(language.to_string(), spec);
        }
        None => {
            settings.lsp.servers.remove(language);
        }
    }
}

/// Perintah lengkap sebuah server seperti ditampilkan di tabel.
pub fn command_line(command: &str, args: &[String]) -> String {
    let joined = join_args(args);
    if joined.is_empty() {
        command.to_string()
    } else {
        format!("{command} {joined}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Settings {
        Settings::default()
    }

    fn field(id: &str) -> &'static Field {
        field_by_id(id).unwrap_or_else(|| panic!("kolom tidak dikenal: {id}"))
    }

    fn number(id: &str) -> &'static NumberSpec {
        match &field(id).kind {
            FieldKind::Number(spec) => spec,
            _ => panic!("{id} bukan kolom angka"),
        }
    }

    fn text(id: &str) -> &'static TextSpec {
        match &field(id).kind {
            FieldKind::Text(spec) => spec,
            _ => panic!("{id} bukan kolom teks"),
        }
    }

    fn json_at(settings: &Settings, id: &str) -> serde_json::Value {
        let (group, key) = id.split_once('.').unwrap();
        serde_json::to_value(settings).unwrap()[group][key].clone()
    }

    fn spec(command: &str, args: &[&str]) -> ServerSpec {
        ServerSpec { command: command.into(), args: args.iter().map(|a| a.to_string()).collect(), env: Vec::new() }
    }

    #[test]
    fn field_list_covers_every_setting_with_a_description() {
        let ids: Vec<&str> = FIELDS.iter().map(|f| f.id).collect();
        assert_eq!(
            ids,
            [
                "editor.fontFamily",
                "editor.fontSize",
                "editor.tabSize",
                "editor.insertSpaces",
                "editor.wordWrap",
                "editor.lineNumbers",
                "editor.inlineBlame",
                "editor.formatOnSave",
                "files.autoSave",
                "workbench.theme",
                "workbench.startupView",
                "workbench.sidebarPosition",
                "git.path",
                "git.autoFetchMinutes",
                "git.pullStrategy",
                "git.confirmForcePush",
                "terminal.shell",
                "terminal.fontSize",
                "lsp.enabled",
                "forge.githubClientId",
                "forge.gitlabClientId",
                "ai.provider",
                "ai.model",
                "ai.customFormat",
                "ai.customBaseUrl",
                "ai.customModel",
                "extensions.markdownPreview",
                "extensions.formatters",
                "extensions.extraLanguages",
                "extensions.symbolSearch",
            ]
        );
        for f in &FIELDS {
            assert!(f.description.len() > 10, "{}", f.id);
            assert!(SECTIONS.contains(&f.section), "{}", f.id);
        }
        let unique: std::collections::BTreeSet<&str> = ids.iter().copied().collect();
        assert_eq!(unique.len(), FIELDS.len());
    }

    /// Id kolom menunjuk ke jalur yang sama di `settings.json`, dan `set` hanya mengubah jalur itu.
    #[test]
    fn field_ids_point_at_the_same_path_in_settings_json() {
        for f in &FIELDS {
            let before = defaults();
            let current = json_at(&before, f.id);
            // `get` membaca jalur yang sama (null ditampilkan sebagai teks kosong pada kolom teks).
            let shown = match f.value(&before) {
                FieldValue::Text(text) if text.is_empty() => serde_json::Value::Null,
                FieldValue::Text(text) => text.into(),
                FieldValue::Number(n) => serde_json::to_value(n).unwrap(),
                FieldValue::Bool(b) => b.into(),
                FieldValue::Choice(c) => c.into(),
            };
            assert_eq!(shown, current, "{}", f.id);

            let mut after = before.clone();
            match &f.kind {
                FieldKind::Checkbox(spec) => (spec.set)(&mut after, !(spec.get)(&before)),
                FieldKind::Select(spec) => (spec.set)(&mut after, spec.options.iter().find(|o| o.value != (spec.get)(&before)).unwrap().value),
                FieldKind::Number(spec) => (spec.set)(&mut after, Some(spec.min + 1)),
                FieldKind::Text(spec) => (spec.set)(&mut after, "changed-value"),
            }
            assert_ne!(json_at(&after, f.id), current, "{} tidak berubah", f.id);
            // Hanya jalur itu yang berubah.
            let (group, key) = f.id.split_once('.').unwrap();
            let mut expected = serde_json::to_value(&before).unwrap();
            expected[group][key] = json_at(&after, f.id);
            assert_eq!(serde_json::to_value(&after).unwrap(), expected, "{}", f.id);
        }
    }

    #[test]
    fn select_options_match_the_backend_enums() {
        let values = |id: &str| match &field(id).kind {
            FieldKind::Select(spec) => spec.options.iter().map(|o| format!("{}={}", o.value, o.label)).collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        assert_eq!(values("files.autoSave"), ["off=Off", "onFocusChange=On focus change", "afterDelay=After delay"]);
        assert_eq!(values("workbench.theme"), ["system=System", "dark=Dark", "light=Light"]);
        assert_eq!(values("workbench.startupView"), ["scm=Source Control", "explorer=Explorer"]);
        assert_eq!(values("workbench.sidebarPosition"), ["left=Left", "right=Right"]);
        assert_eq!(values("git.pullStrategy"), ["ffOnly=Fast-forward only", "merge=Merge", "rebase=Rebase"]);
        assert_eq!(values("ai.provider"), ["auto=Automatic", "claudeCode=Claude Code CLI", "api=Anthropic API", "custom=Custom endpoint"]);
        assert_eq!(values("ai.customFormat"), ["openai=OpenAI-compatible", "anthropic=Anthropic Messages"]);

        // Setiap opsi tersimpan dengan ejaan yang sama seperti serde di backend, dan terbaca kembali.
        for f in &FIELDS {
            let FieldKind::Select(spec) = &f.kind else { continue };
            for option in spec.options {
                let mut settings = defaults();
                (spec.set)(&mut settings, option.value);
                assert_eq!(json_at(&settings, f.id), serde_json::Value::from(option.value), "{}", f.id);
                assert_eq!((spec.get)(&settings), option.value, "{}", f.id);
            }
            let mut settings = defaults();
            (spec.set)(&mut settings, "tidak-dikenal");
            assert_eq!(settings, defaults(), "{}: nilai tak dikenal diabaikan", f.id);
        }
    }

    #[test]
    fn nullable_text_fields_store_null_and_trim() {
        for id in ["git.path", "terminal.shell", "forge.githubClientId", "forge.gitlabClientId"] {
            let spec = text(id);
            let read = |input: &str| {
                let mut settings = defaults();
                (spec.set)(&mut settings, "sebelumnya");
                (spec.set)(&mut settings, input);
                json_at(&settings, id)
            };
            assert_eq!(read("   "), serde_json::Value::Null, "{id}");
            assert_eq!(read(""), serde_json::Value::Null, "{id}");
            assert_eq!(read("  value  "), serde_json::Value::from("value"), "{id}");
        }
        assert!(field("git.path").description.contains("after restarting"));
    }

    #[test]
    fn text_validation() {
        let check = |id: &str, input: &str| (text(id).validate)(input);
        assert_eq!(check("editor.fontFamily", "Fira Code, monospace"), None);
        assert!(check("editor.fontFamily", "  ").unwrap().contains("at least one font"));
        assert!(check("editor.fontFamily", "x; } body { display:none").unwrap().contains("cannot contain"));
        assert!(check("editor.fontFamily", &"x".repeat(501)).unwrap().contains("too long"));
        assert_eq!(check("git.path", "/usr/bin/git"), None);
        assert_eq!(check("git.path", ""), None);
        assert!(check("git.path", "/usr/bin/git\n--exec").unwrap().contains("control characters"));
        assert!(check("git.path", &"x".repeat(4097)).unwrap().contains("too long"));
        assert_eq!(check("forge.githubClientId", "Iv1.0123456789abcdef"), None);
        assert_eq!(check("forge.githubClientId", ""), None);
        assert!(check("forge.githubClientId", "two words").unwrap().contains("cannot contain spaces"));
        assert!(check("forge.gitlabClientId", &"x".repeat(201)).unwrap().contains("too long"));
        assert_eq!(check("ai.model", "claude-opus-5-5"), None);
        assert!(check("ai.model", "  ").unwrap().contains("Enter a model"));
        assert!(check("ai.model", "claude opus").unwrap().contains("cannot contain spaces"));
        assert_eq!(check("ai.customBaseUrl", "http://localhost:11434/v1"), None);
        assert_eq!(check("ai.customBaseUrl", ""), None);
        assert!(check("ai.customBaseUrl", "openrouter.ai/api/v1").unwrap().contains("https://"));
        assert!(check("ai.customModel", "gpt 4").unwrap().contains("cannot contain spaces"));
        assert_eq!(validate_server_command("rust-analyzer"), None);
        assert_eq!(validate_server_command("  "), Some("Enter a command"));
        assert!(validate_server_command("a\tb").unwrap().contains("control characters"));
    }

    #[test]
    fn parse_number_accepts_whole_numbers_in_range() {
        assert_eq!(parse_number(number("editor.fontSize"), "8"), Ok(Some(8)));
        assert_eq!(parse_number(number("editor.fontSize"), " 40 "), Ok(Some(40)));
        assert_eq!(parse_number(number("editor.fontSize"), "014"), Ok(Some(14)));
        assert_eq!(parse_number(number("editor.tabSize"), "1"), Ok(Some(1)));
        assert_eq!(parse_number(number("editor.tabSize"), "16"), Ok(Some(16)));
        assert_eq!(parse_number(number("git.autoFetchMinutes"), "0"), Ok(Some(0)));
    }

    #[test]
    fn parse_number_rejects_everything_else() {
        let cases = [
            ("editor.fontSize", "7", "from 8 to 40"),
            ("editor.fontSize", "41", "from 8 to 40"),
            ("editor.tabSize", "0", "from 1 to 16"),
            ("editor.tabSize", "17", "from 1 to 16"),
            ("editor.fontSize", "14.5", "whole number"),
            ("editor.fontSize", "-14", "whole number"),
            ("editor.fontSize", "1e1", "whole number"),
            ("editor.fontSize", "abc", "whole number"),
            ("editor.fontSize", "", "Enter a number"),
            ("git.autoFetchMinutes", "1441", "from 0 to 1440"),
            ("git.autoFetchMinutes", "99999999999999999999", "from 0 to 1440"),
            ("terminal.fontSize", "7", "from 8 to 40"),
            ("terminal.fontSize", "200", "from 8 to 40"),
        ];
        for (id, input, message) in cases {
            let error = parse_number(number(id), input).expect_err(&format!("{id} = {input:?} harus ditolak"));
            assert!(error.contains(message), "{id} = {input:?}: {error}");
        }
    }

    #[test]
    fn optional_number_may_be_empty() {
        assert_eq!(parse_number(number("terminal.fontSize"), ""), Ok(None));
        assert_eq!(parse_number(number("terminal.fontSize"), "  "), Ok(None));
        assert_eq!(parse_number(number("terminal.fontSize"), "12"), Ok(Some(12)));
        // Kolom wajib mengabaikan `None` alih-alih menulis nilai kosong.
        let mut settings = defaults();
        (number("editor.fontSize").set)(&mut settings, None);
        assert_eq!(settings, defaults());
    }

    fn project_effective() -> Settings {
        let mut effective = defaults();
        effective.editor.tab_size = 2;
        effective.lsp.servers.insert("rust".into(), spec("ra-custom", &[]));
        effective
    }

    #[test]
    fn override_marker_only_for_differing_fields_in_trusted_folders() {
        let user = defaults();
        let effective = project_effective();
        assert!(is_overridden(field("editor.tabSize"), OverrideContext { trusted: true, user: &user, effective: Some(&effective) }));
        assert!(!is_overridden(field("editor.fontSize"), OverrideContext { trusted: true, user: &user, effective: Some(&effective) }));
        assert!(!is_overridden(field("editor.tabSize"), OverrideContext { trusted: false, user: &user, effective: Some(&effective) }));
        // Tanpa pengaturan efektif (belum ada jendela) tidak ada penanda.
        assert!(!is_overridden(field("editor.tabSize"), OverrideContext { trusted: true, user: &user, effective: None }));

        let mut same = defaults();
        same.editor.tab_size = 2;
        assert!(!is_overridden(field("editor.tabSize"), OverrideContext { trusted: true, user: &same, effective: Some(&effective) }));
        let marked: Vec<&str> =
            FIELDS.iter().filter(|f| is_overridden(f, OverrideContext { trusted: true, user: &user, effective: Some(&effective) })).map(|f| f.id).collect();
        assert_eq!(marked, ["editor.tabSize"]);
    }

    #[test]
    fn override_marker_per_language_server() {
        let user = defaults();
        let effective = project_effective();
        let trusted = OverrideContext { trusted: true, user: &user, effective: Some(&effective) };
        assert!(is_server_overridden("rust", trusted));
        assert!(!is_server_overridden("go", trusted));
        assert!(!is_server_overridden("rust", OverrideContext { trusted: false, ..trusted }));
        // `args: []` dan args yang tidak ditulis di berkas dianggap sama.
        let mut user = defaults();
        set_server_override(&mut user, "rust", serde_json::from_str(r#"{ "command": "ra-custom" }"#).ok());
        assert!(!is_server_overridden("rust", OverrideContext { trusted: true, user: &user, effective: Some(&effective) }));
    }

    #[test]
    fn search_matches_every_word_across_label_description_id_section_and_options() {
        assert!(matches_query("", &["x"]));
        assert!(matches_query("   ", &["x"]));
        assert!(matches_query("font size", &["Font size"]));
        assert!(!matches_query("font zzz", &["Font size"]));
        let ids = |query: &str| FIELDS.iter().filter(|f| field_matches(f, query)).map(|f| f.id).collect::<Vec<_>>();
        assert_eq!(ids("font size"), ["editor.fontSize", "terminal.fontSize"]);
        assert_eq!(ids("TERMINAL"), ["editor.fontFamily", "terminal.shell", "terminal.fontSize"]);
        assert_eq!(ids("rebase"), ["git.pullStrategy"]);
        assert_eq!(ids("git.path"), ["git.path"]);
        assert_eq!(ids("accounts"), ["forge.githubClientId", "forge.gitlabClientId"]);
        assert!(ids("no such thing").is_empty());
    }

    #[test]
    fn split_args_handles_quotes_and_backslashes() {
        let ok = |args: &[&str]| Ok(args.iter().map(|a| a.to_string()).collect::<Vec<_>>());
        assert_eq!(split_args(""), ok(&[]));
        assert_eq!(split_args("  --stdio  "), ok(&["--stdio"]));
        assert_eq!(split_args(r#"--log "C:\\Program Files\\x" --flag"#), ok(&["--log", r"C:\Program Files\x", "--flag"]));
        assert_eq!(split_args(r#"--name 'two words' """#), ok(&["--name", "two words", ""]));
        assert_eq!(split_args(r"C:\tools\ls.exe"), ok(&[r"C:\tools\ls.exe"]));
        assert_eq!(split_args(r#"--bad "unclosed"#), Err("A quote is not closed".to_string()));
    }

    #[test]
    fn join_args_is_the_inverse_of_split_args() {
        let cases: [&[&str]; 5] = [&[], &["--stdio"], &["--log", r"C:\Program Files\x"], &["a b", "", r#"say "hi""#, "it's"], &["--x=1", "-y"]];
        for args in cases {
            let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            assert_eq!(split_args(&join_args(&args)), Ok(args.clone()), "{args:?}");
        }
        assert_eq!(join_args(&["--stdio".to_string(), "two words".to_string()]), r#"--stdio "two words""#);
        assert_eq!(command_line("rust-analyzer", &[]), "rust-analyzer");
        assert_eq!(command_line("tsls", &["--stdio".to_string()]), "tsls --stdio");
    }

    #[test]
    fn set_server_override_adds_and_removes() {
        let mut settings = defaults();
        set_server_override(&mut settings, "rust", Some(spec("ra", &["--x"])));
        assert_eq!(settings.lsp.servers.len(), 1);
        assert_eq!(settings.lsp.servers["rust"], spec("ra", &["--x"]));
        set_server_override(&mut settings, "rust", None);
        assert_eq!(settings, defaults());
    }

    fn changed() -> Settings {
        let mut settings = defaults();
        settings.editor.font_size = 20;
        settings.editor.word_wrap = true;
        settings.git.path = Some("/opt/git".into());
        settings.git.pull_strategy = PullStrategy::Rebase;
        settings.terminal.font_size = Some(11);
        settings.lsp.servers.insert("rust".into(), spec("ra", &[]));
        settings.lsp.servers.insert("go".into(), spec("gopls", &[]));
        settings
    }

    #[test]
    fn differs_from_default_and_apply_default_work_per_field() {
        let (changed, defaults) = (changed(), defaults());
        let differing: Vec<&str> = FIELDS.iter().filter(|f| differs_from_default(f, &changed, &defaults)).map(|f| f.id).collect();
        assert_eq!(differing, ["editor.fontSize", "editor.wordWrap", "git.path", "git.pullStrategy", "terminal.fontSize"]);
        assert!(FIELDS.iter().all(|f| !differs_from_default(f, &defaults, &defaults)));

        let mut current = changed.clone();
        for f in &FIELDS {
            apply_default(f, &mut current, &defaults);
            assert!(!differs_from_default(f, &current, &defaults), "{}", f.id);
        }
        // Yang tersisa hanya penimpaan server, yang bukan kolom formulir.
        let mut expected = defaults.clone();
        expected.lsp = changed.lsp.clone();
        assert_eq!(current, expected);
    }

    /// `apply_default` mengikuti nilai bawaan yang diberikan, bukan konstanta di modul ini.
    #[test]
    fn apply_default_follows_the_given_defaults() {
        let mut other = defaults();
        other.editor.tab_size = 2;
        other.git.path = Some("/usr/local/bin/git".into());
        other.terminal.font_size = Some(13);
        let apply = |id: &str| {
            let mut settings = defaults();
            apply_default(field(id), &mut settings, &other);
            settings
        };
        assert_eq!(apply("editor.tabSize").editor.tab_size, 2);
        assert_eq!(apply("git.path").git.path.as_deref(), Some("/usr/local/bin/git"));
        assert_eq!(apply("terminal.fontSize").terminal.font_size, Some(13));
    }

    #[test]
    fn default_labels() {
        let defaults = defaults();
        assert_eq!(default_label(field("editor.fontSize"), &defaults), "14");
        assert_eq!(default_label(field("terminal.fontSize"), &defaults), "empty");
        assert_eq!(default_label(field("git.path"), &defaults), "empty");
        assert_eq!(default_label(field("editor.insertSpaces"), &defaults), "on");
        assert_eq!(default_label(field("editor.wordWrap"), &defaults), "off");
        assert_eq!(default_label(field("git.pullStrategy"), &defaults), "Fast-forward only");
    }

    #[test]
    fn count_non_default_counts_fields_and_server_overrides() {
        assert_eq!(count_non_default(&defaults(), &defaults()), 0);
        assert_eq!(count_non_default(&changed(), &defaults()), 7);
    }

    #[test]
    fn field_text_shows_numbers_and_empty_values() {
        let mut settings = defaults();
        assert_eq!(field_text(field("editor.fontSize"), &settings), "14");
        assert_eq!(field_text(field("terminal.fontSize"), &settings), "");
        assert_eq!(field_text(field("git.path"), &settings), "");
        settings.terminal.font_size = Some(12);
        settings.git.path = Some("/opt/git".into());
        assert_eq!(field_text(field("terminal.fontSize"), &settings), "12");
        assert_eq!(field_text(field("git.path"), &settings), "/opt/git");
    }
}
