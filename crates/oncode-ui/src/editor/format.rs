//! Format Document dan Format on Save.
//!
//! Formatter datang dari beberapa penyedia berprioritas: ekstensi Formatters (Prettier, rustfmt,
//! gofmt, …) lebih dulu, lalu language server. Penyedia yang tidak bisa menangani sebuah berkas
//! menjawab [`FormatOutcome::Unavailable`], dan penyedia berikutnya dicoba.

use std::rc::Rc;

use gpui_kit::component::input::{EditorState, RopeExt as _};
use gpui_kit::{App, AppContext as _, Entity, Global, Task, Window};
use oncode_core::api::format as format_api;

use super::buffer::{is_untitled, Buffer};
use super::language::language_for_path;
use crate::workbench::Workbench;
use crate::{overlay, spawn_core};

/// Hasil satu penyedia format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatOutcome {
    /// Isi baru seluruh dokumen.
    Formatted(String),
    /// Formatter berjalan, tidak ada yang berubah.
    Unchanged,
    /// Penyedia ini tidak bisa memformat berkas ini (tidak terpasang, dimatikan, tidak mendukung).
    Unavailable,
    /// Formatter gagal; pesan untuk pengguna.
    Failed(String),
}

/// Penyedia format: `None` sama dengan [`FormatOutcome::Unavailable`] tanpa perlu menunggu.
pub type FormatProvider = Rc<dyn Fn(&Entity<Buffer>, &Entity<EditorState>, &mut Window, &mut App) -> Option<Task<FormatOutcome>>>;

#[derive(Default)]
struct FormatProviders(Vec<(i32, FormatProvider)>);

impl Global for FormatProviders {}

/// Mendaftarkan penyedia format; prioritas lebih tinggi dicoba lebih dulu.
pub fn register_format_provider(priority: i32, provider: impl Fn(&Entity<Buffer>, &Entity<EditorState>, &mut Window, &mut App) -> Option<Task<FormatOutcome>> + 'static, cx: &mut App) {
    let providers = &mut cx.default_global::<FormatProviders>().0;
    providers.push((priority, Rc::new(provider)));
    providers.sort_by(|a, b| b.0.cmp(&a.0));
}

/// Prioritas penyedia ekstensi Formatters (formatter milik bahasa).
pub const EXTERNAL_PRIORITY: i32 = 100;
/// Prioritas penyedia language server.
pub const LSP_PRIORITY: i32 = 50;

/// Memformat isi buffer lewat penyedia pertama yang tersedia. Hasil `true` bila diformat atau
/// sudah rapi. `quiet`: tanpa pesan "tidak ada formatter" (dipakai Format on Save).
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

/// Mengganti seluruh isi dengan hasil format sebagai satu suntingan (bisa di-undo), kursor
/// dipertahankan pada baris dan kolom yang sama sejauh masih ada.
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

/// Penyedia ekstensi Formatters: formatter milik bahasa yang terpasang di mesin atau proyek.
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

/// Menyimpan buffer; dengan Format on Save menyala, diformat lebih dulu (gagal format tetap disimpan).
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
