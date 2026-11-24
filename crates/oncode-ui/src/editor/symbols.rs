//! Go to Definition dan Ctrl+klik tanpa language server (ekstensi `extensions.symbolSearch`):
//! definisi dicari di seluruh proyek lewat `oncode_core::api::symbols`.
//!
//! Setiap editor berkas mendapat penyedia ini sejak dibuat. Bila language server terhubung, hub
//! LSP menggantinya dengan penyedianya sendiri, yang kembali ke pencarian ini bila server tidak
//! menemukan apa-apa.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::component::input::{DefinitionProvider, EditorState, Rope, RopeExt as _};
use gpui_kit::{App, Entity, Result as GpuiResult, Task, Window};
use lsp_types::{LocationLink, Position, Range};
use oncode_core::api::symbols::{self as symbols_api, SymbolLocation};

use super::{is_untitled, open_file, OpenFile};
use crate::workbench::Env;

/// Hasil pencarian dipakai ulang sebentar: Ctrl+arahkan kursor memanggil penyedia berulang kali.
const CACHE_FOR: Duration = Duration::from_secs(3);

type Cache = Rc<RefCell<Option<(String, Instant, Vec<SymbolLocation>)>>>;

/// Penyedia definisi berbasis pencarian proyek untuk satu berkas.
pub struct SymbolSearchProvider {
    env: Env,
    path: String,
    cache: Cache,
}

impl SymbolSearchProvider {
    pub fn new(env: Env, path: String) -> Self {
        Self { env, path, cache: Rc::default() }
    }
}

impl DefinitionProvider for SymbolSearchProvider {
    fn definitions(&self, text: &Rope, offset: usize, _: &mut Window, cx: &mut App) -> Task<GpuiResult<Vec<LocationLink>>> {
        search_links(&self.env, &self.path, text, offset, Some(self.cache.clone()), cx)
    }
}

/// Definisi simbol di `offset` lewat pencarian proyek, sebagai `LocationLink` untuk editor.
/// Dipakai juga hub LSP bila server tidak menemukan definisi.
pub fn symbol_definition_links(env: &Env, path: &str, text: &Rope, offset: usize, cx: &mut App) -> Task<GpuiResult<Vec<LocationLink>>> {
    search_links(env, path, text, offset, None, cx)
}

fn search_links(env: &Env, path: &str, text: &Rope, offset: usize, cache: Option<Cache>, cx: &mut App) -> Task<GpuiResult<Vec<LocationLink>>> {
    let Some(word) = text.word_range(offset) else { return Task::ready(Ok(Vec::new())) };
    let symbol = text.slice(word.clone()).to_string();
    if symbol.is_empty() || symbol.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return Task::ready(Ok(Vec::new()));
    }
    let origin = Range { start: text.offset_to_position(word.start), end: text.offset_to_position(word.end) };
    if let Some((cached_symbol, at, locations)) = cache.as_ref().and_then(|cache| cache.borrow().clone()) {
        if cached_symbol == symbol && at.elapsed() < CACHE_FOR {
            return Task::ready(Ok(links(&locations, origin)));
        }
    }
    let current = (!is_untitled(path)).then(|| path.to_string());
    let (core, label, query) = (env.core.clone(), env.window.to_string(), symbol.clone());
    let task = crate::spawn_core(cx, async move { symbols_api::symbol_definitions(&core, &label, query, current).await });
    cx.spawn(async move |_| {
        // Galat pencarian (mis. tidak ada folder terbuka) berarti tidak ada definisi, bukan kegagalan.
        let locations = task.await.unwrap_or_default();
        if let Some(cache) = cache {
            *cache.borrow_mut() = Some((symbol, Instant::now(), locations.clone()));
        }
        Ok(links(&locations, origin))
    })
}

fn links(locations: &[SymbolLocation], origin: Range) -> Vec<LocationLink> {
    locations
        .iter()
        .filter_map(|location| {
            let uri = oncode_lsp::uri::path_to_lsp_uri(&location.path)?;
            let start = Position { line: location.line.saturating_sub(1), character: location.column };
            let target = Range { start, end: Position { line: start.line, character: location.column + location.length } };
            Some(LocationLink { origin_selection_range: Some(origin), target_uri: uri, target_range: target, target_selection_range: target })
        })
        .collect()
}

/// Memasang penyedia pencarian simbol pada editor yang baru dibuat, beserta pembuka berkas untuk
/// definisi di berkas lain. Hub LSP boleh menimpanya sesudah ini.
pub(crate) fn attach(state: &Entity<EditorState>, env: &Env, path: &str, cx: &mut App) {
    let provider = Rc::new(SymbolSearchProvider::new(env.clone(), path.to_string()));
    let workbench = env.workbench.clone();
    state.update(cx, |state, cx| {
        let lsp = state.lsp_mut();
        lsp.definition_provider = Some(provider);
        lsp.show_document = Some(Rc::new(move |params: &lsp_types::ShowDocumentParams, window: &mut Window, cx: &mut App| {
            if params.external == Some(true) {
                return false;
            }
            let target = oncode_lsp::uri::uri_to_path(params.uri.as_str());
            let selection = params.selection.unwrap_or_default();
            let options = OpenFile::new(target).at(selection.start.line + 1, selection.start.character, None);
            let _ = workbench.update(cx, |workbench, cx| open_file(workbench, options, window, cx));
            true
        }));
        cx.notify();
    });
}
