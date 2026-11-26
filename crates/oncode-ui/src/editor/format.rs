//! Format Document and Format on Save.
//!
//! Formatters come from several prioritized providers: the Formatters extension (Prettier, rustfmt,
//! gofmt, …) first, then the language server. A provider that cannot handle a file
//! answers [`FormatOutcome::Unavailable`], and the next provider is tried.

use std::rc::Rc;

use gpui_kit::component::input::{EditorState, RopeExt as _};
use gpui_kit::{App, AppContext as _, Entity, Global, Task, Window};
use oncode_core::api::format as format_api;

use super::buffer::{is_untitled, Buffer};
use super::language::language_for_path;
use crate::workbench::Workbench;
use crate::{overlay, spawn_core};

/// The result of one format provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatOutcome {
    /// The new content of the whole document.
    Formatted(String),
    /// The formatter ran, nothing changed.
    Unchanged,
    /// This provider cannot format this file (not installed, turned off, unsupported).
    Unavailable,
    /// The formatter failed; message for the user.
    Failed(String),
}

/// A format provider: `None` is the same as [`FormatOutcome::Unavailable`] without having to wait.
pub type FormatProvider = Rc<dyn Fn(&Entity<Buffer>, &Entity<EditorState>, &mut Window, &mut App) -> Option<Task<FormatOutcome>>>;

#[derive(Default)]
struct FormatProviders(Vec<(i32, FormatProvider)>);

impl Global for FormatProviders {}

/// Registers a format provider; a higher priority is tried first.
pub fn register_format_provider(priority: i32, provider: impl Fn(&Entity<Buffer>, &Entity<EditorState>, &mut Window, &mut App) -> Option<Task<FormatOutcome>> + 'static, cx: &mut App) {
    let providers = &mut cx.default_global::<FormatProviders>().0;
    providers.push((priority, Rc::new(provider)));
    providers.sort_by(|a, b| b.0.cmp(&a.0));
}

/// Priority of the Formatters extension provider (the language's own formatter).
pub const EXTERNAL_PRIORITY: i32 = 100;
/// Priority of the language server provider.
pub const LSP_PRIORITY: i32 = 50;

/// Formats the buffer content through the first available provider. Resolves to `true` if it was
/// formatted or already tidy. `quiet`: no "no formatter" message (used by Format on Save).
pub fn format_buffer(buffer: &Entity<Buffer>, state: &Entity<EditorState>, quiet: bool, window: &mut Window, cx: &mut App) -> Task<bool> {
    let providers: Vec<FormatProvider> = cx.try_global::<FormatProviders>().map(|p| p.0.iter().map(|(_, provider)| provider.clone()).collect()).unwrap_or_default();
    let (buffer, state) = (buffer.clone(), state.clone());
    window.spawn(cx, async move |cx| {
        for provider in providers {
            let Ok(task) = cx.update(|window, cx| provider(&buffer, &state, window, cx)) else { return false };
            let Some(task) = task else { continue };
            match task.await {
                FormatOutcome::Unavailable => continue,
                FormatOutcome::Unchanged => return true,
                FormatOutcome::Formatted(text) => {
                    let _ = cx.update(|window, cx| apply_formatted(&state, text, window, cx));
                    return true;
                }
                FormatOutcome::Failed(message) => {
                    let _ = cx.update(|window, cx| overlay::toast_error(message, window, cx));
                    return false;
                }
            }
        }
        if !quiet {
            let _ = cx.update(|window, cx| {
                let language = language_for_path(buffer.read(cx).path());
                let hint = format_api::formatter_install_hint(language.lsp).map(|hint| format!(" {hint}")).unwrap_or_default();
                overlay::toast_info(format!("No formatter is available for {}.{hint}", language.name), window, cx);
            });
        }
        false
    })
}

/// Replaces the whole content with the formatted result as a single edit (undoable), keeping the
/// cursor at the same line and column as far as they still exist.
fn apply_formatted(state: &Entity<EditorState>, formatted: String, window: &mut Window, cx: &mut App) {
    state.update(cx, |state, cx| {
        let text = state.text().clone();
        if text.to_string() == formatted {
            return;
        }
        let cursor = state.cursor_position();
        let range = lsp_types::Range { start: text.offset_to_position(0), end: text.offset_to_position(text.len()) };
        state.apply_lsp_edits(&vec![lsp_types::TextEdit { range, new_text: formatted }], window, cx);
        state.set_cursor_position(cursor, window, cx);
    });
}

/// The Formatters extension provider: the language's own formatter installed on the machine or in the project.
pub(crate) fn external_provider(buffer: &Entity<Buffer>, _: &Entity<EditorState>, window: &mut Window, cx: &mut App) -> Option<Task<FormatOutcome>> {
    let workbench = Workbench::for_window(window, cx)?;
    let (env, enabled) = {
        let workbench = workbench.read(cx);
        (workbench.env().clone(), workbench.settings().extensions.formatters)
    };
    let path = buffer.read(cx).path().to_string();
    if !enabled || !env.trusted || is_untitled(&path) {
        return None;
    }
    let language = language_for_path(&path).lsp.to_string();
    let text = buffer.read(cx).text(cx)?.to_string();
    let (core, label) = (env.core.clone(), env.window.to_string());
    let task = spawn_core(cx, async move { format_api::format_text(&core, &label, path, language, text).await });
    Some(cx.background_spawn(async move {
        match task.await {
            Ok(Some(formatted)) => FormatOutcome::Formatted(formatted.text),
            Ok(None) => FormatOutcome::Unavailable,
            Err(error) => FormatOutcome::Failed(error.message),
        }
    }))
}

/// Saves the buffer; with Format on Save on, it is formatted first (a failed format is still saved).
pub fn save_with_format(buffer: &Entity<Buffer>, state: &Entity<EditorState>, window: &mut Window, cx: &mut App) {
    let format_on_save = Workbench::for_window(window, cx).is_some_and(|w| w.read(cx).settings().editor.format_on_save);
    if !format_on_save || buffer.read(cx).is_untitled() {
        buffer.update(cx, |buffer, cx| buffer.save(window, cx)).detach();
        return;
    }
    let format = format_buffer(buffer, state, true, window, cx);
    let buffer = buffer.clone();
    window
        .spawn(cx, async move |cx| {
            format.await;
            let _ = buffer.update_in(cx, |buffer, window, cx| buffer.save(window, cx).detach());
        })
        .detach();
}
