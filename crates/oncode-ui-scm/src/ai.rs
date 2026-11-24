//! "Generate Commit Message" dengan AI: pengelolaan API key (Anthropic dan endpoint kustom). Alur
//! pembuatan pesannya ada di [`ScmPanel::generate_message`](crate::view::ScmPanel::generate_message).

use gpui_kit::{App, AsyncWindowContext, Window};
use oncode_core::{api, AppResult, Shared};
use oncode_ui::overlay::{self, Confirm, TextPrompt};
use oncode_ui::{core, spawn_core};

/// Key mana yang diatur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyKind {
    /// API key Anthropic (provider Anthropic API / Otomatis).
    Anthropic,
    /// API key endpoint kustom (provider Custom endpoint).
    Custom,
}

impl KeyKind {
    fn title(self) -> &'static str {
        match self {
            KeyKind::Anthropic => "Anthropic API key",
            KeyKind::Custom => "Custom endpoint API key",
        }
    }

    /// Nama di tengah kalimat.
    fn noun(self) -> &'static str {
        match self {
            KeyKind::Anthropic => "Anthropic API key",
            KeyKind::Custom => "custom endpoint API key",
        }
    }

    fn help(self) -> &'static str {
        match self {
            KeyKind::Anthropic => {
                "Create a key in the Claude Console (platform.claude.com, under API keys) and paste it here. \
It is stored in your system keychain. Each time you generate a message, the staged diff and the recent commit subjects are sent to Anthropic.\n\n\
No key? Install Claude Code and sign in (a Claude subscription works): OnCode then uses it instead (Settings → AI → Provider)."
            }
            KeyKind::Custom => {
                "The key of the endpoint set in Settings → AI (sent as `Authorization: Bearer` for OpenAI-compatible endpoints, `x-api-key` for Anthropic ones). \
It is stored in your system keychain. Each time you generate a message, the staged diff and the recent commit subjects are sent to that endpoint."
            }
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            KeyKind::Anthropic => "sk-ant-…",
            KeyKind::Custom => "API key",
        }
    }
}

async fn save(core: Shared, kind: KeyKind, key: String) -> AppResult<()> {
    match kind {
        KeyKind::Anthropic => api::ai::ai_set_api_key(&core, key).await,
        KeyKind::Custom => api::ai::ai_set_custom_api_key(&core, key).await,
    }
}

async fn remove(core: Shared, kind: KeyKind) -> AppResult<()> {
    match kind {
        KeyKind::Anthropic => api::ai::ai_remove_api_key(&core).await,
        KeyKind::Custom => api::ai::ai_remove_custom_api_key(&core).await,
    }
}

/// Meminta API key lalu menyimpannya. `reason` ditampilkan di atas petunjuk (mis. key lama
/// ditolak). `true` bila key tersimpan.
pub(crate) async fn ask_api_key(core: Shared, kind: KeyKind, reason: Option<String>, cx: &mut AsyncWindowContext) -> bool {
    let message = match reason {
        Some(reason) => format!("{reason}\n\n{}", kind.help()),
        None => kind.help().to_string(),
    };
    let prompt = TextPrompt::new(kind.title(), "Save").message(message).placeholder(kind.placeholder()).secret();
    let Ok(answer) = cx.update(|window, cx| overlay::prompt_text(prompt, window, cx)) else { return false };
    let Some(key) = answer.await.map(|key| key.trim().to_string()).filter(|key| !key.is_empty()) else { return false };
    let Ok(task) = cx.update(|_, cx| spawn_core(cx, save(core, kind, key))) else { return false };
    let result = task.await;
    let saved = result.is_ok();
    let _ = cx.update(|window, cx| {
        if let Err(error) = result {
            overlay::show_error(&error, window, cx);
        }
    });
    saved
}

/// Command "AI: Set Anthropic API Key…" / "AI: Set Custom Endpoint API Key…".
pub(crate) fn set_api_key(kind: KeyKind, window: &mut Window, cx: &mut App) {
    let core = core(cx);
    window
        .spawn(cx, async move |cx| {
            if ask_api_key(core, kind, None, cx).await {
                let _ = cx.update(|window, cx| overlay::toast_success(format!("The {} was saved.", kind.noun()), window, cx));
            }
        })
        .detach();
}

/// Command "AI: Remove Anthropic API Key" / "AI: Remove Custom Endpoint API Key".
pub(crate) fn remove_api_key(kind: KeyKind, window: &mut Window, cx: &mut App) {
    let core = core(cx);
    let what = kind.noun();
    let answer = overlay::confirm(
        Confirm::new(format!("Remove the {what}?"), "OnCode will not be able to use it to generate commit messages until you set it again.", "Remove").danger(),
        window,
        cx,
    );
    window
        .spawn(cx, async move |cx| {
            if !answer.await {
                return;
            }
            let Ok(task) = cx.update(|_, cx| spawn_core(cx, remove(core, kind))) else { return };
            let result = task.await;
            let _ = cx.update(|window, cx| match result {
                Ok(()) => overlay::toast_success(format!("The {what} was removed."), window, cx),
                Err(error) => overlay::show_error(&error, window, cx),
            });
        })
        .detach();
}
