//! "Generate Commit Message" dengan Claude: pengelolaan API key Anthropic. Alur pembuatan pesannya
//! ada di [`ScmPanel::generate_message`](crate::view::ScmPanel::generate_message).

use gpui_kit::{App, AsyncWindowContext, Window};
use oncode_core::{api, Shared};
use oncode_ui::overlay::{self, Confirm, TextPrompt};
use oncode_ui::{core, spawn_core};

const KEY_HELP: &str = "Create a key in the Claude Console (platform.claude.com, under API keys) and paste it here. \
It is stored in your system keychain. Each time you generate a message, the staged diff and the recent commit subjects are sent to Anthropic.";

/// Meminta API key Anthropic lalu menyimpannya. `reason` ditampilkan di atas petunjuk (mis. key
/// lama ditolak). `true` bila key tersimpan.
pub(crate) async fn ask_api_key(core: Shared, reason: Option<String>, cx: &mut AsyncWindowContext) -> bool {
    let message = match reason {
        Some(reason) => format!("{reason}\n\n{KEY_HELP}"),
        None => KEY_HELP.to_string(),
    };
    let prompt = TextPrompt::new("Anthropic API key", "Save").message(message).placeholder("sk-ant-…").secret();
    let Ok(answer) = cx.update(|window, cx| overlay::prompt_text(prompt, window, cx)) else { return false };
    let Some(key) = answer.await.map(|key| key.trim().to_string()).filter(|key| !key.is_empty()) else { return false };
    let Ok(task) = cx.update(|_, cx| spawn_core(cx, async move { api::ai::ai_set_api_key(&core, key).await })) else { return false };
    let result = task.await;
    let saved = result.is_ok();
    let _ = cx.update(|window, cx| {
        if let Err(error) = result {
            overlay::show_error(&error, window, cx);
        }
    });
    saved
}

/// Command "AI: Set Anthropic API Key…".
pub(crate) fn set_api_key(window: &mut Window, cx: &mut App) {
    let core = core(cx);
    window
        .spawn(cx, async move |cx| {
            if ask_api_key(core, None, cx).await {
                let _ = cx.update(|window, cx| overlay::toast_success("The Anthropic API key was saved.", window, cx));
            }
        })
        .detach();
}

/// Command "AI: Remove Anthropic API Key".
pub(crate) fn remove_api_key(window: &mut Window, cx: &mut App) {
    let core = core(cx);
    let answer = overlay::confirm(
        Confirm::new("Remove the Anthropic API key?", "OnCode will not be able to generate commit messages until you set a key again.", "Remove").danger(),
        window,
        cx,
    );
    window
        .spawn(cx, async move |cx| {
            if !answer.await {
                return;
            }
            let Ok(task) = cx.update(|_, cx| spawn_core(cx, async move { api::ai::ai_remove_api_key(&core).await })) else { return };
            let result = task.await;
            let _ = cx.update(|window, cx| match result {
                Ok(()) => overlay::toast_success("The Anthropic API key was removed.", window, cx),
                Err(error) => overlay::show_error(&error, window, cx),
            });
        })
        .detach();
}
