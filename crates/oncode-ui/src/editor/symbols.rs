//! Go to Definition and Ctrl+click without a language server (the `extensions.symbolSearch` extension):
//! definitions are searched across the whole project through `oncode_core::api::symbols`.
//!
//! Every file editor gets this provider from the moment it is created. When a language server is
//! connected, the LSP hub replaces it with its own provider, which falls back to this search when
//! the server finds nothing.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::component::input::{DefinitionProvider, EditorState, Rope, RopeExt as _};
use gpui_kit::{App, Entity, Result as GpuiResult, Task, WeakEntity, Window};
use lsp_types::{LocationLink, Position, Range};
use oncode_core::api::symbols::{self as symbols_api, SymbolLocation};

use super::{is_untitled, open_file, OpenFile};
use crate::workbench::{Env, Workbench};

/// Search results are reused briefly: Ctrl+hover calls the provider repeatedly.
const CACHE_FOR: Duration = Duration::from_secs(3);

type Cache = Rc<RefCell<Option<(String, Instant, Vec<SymbolLocation>)>>>;

/// A project-search-based definition provider for one file.
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

/// Definitions of the symbol at `offset` via project search, as `LocationLink`s for the editor.
/// Also used by the LSP hub when the server finds no definition.
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
        // A search error (e.g. no folder open) means no definitions, not a failure.
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

/// Attaches the symbol search provider to a newly created editor, along with a file opener for
/// definitions in other files. The LSP hub may override it afterwards.
pub(crate) fn attach(state: &Entity<EditorState>, env: &Env, path: &str, cx: &mut App) {
    let provider = Rc::new(SymbolSearchProvider::new(env.clone(), path.to_string()));
    let show_document = show_document_handler(env.workbench.clone(), path.to_string());
    state.update(cx, |state, cx| {
        let lsp = state.lsp_mut();
        lsp.definition_provider = Some(provider);
        lsp.show_document = Some(show_document);
        cx.notify();
    });
}

/// `true` if both paths point to the same file. On Windows it is case-insensitive and ignores
/// the path separator (language servers may write `c:/…` for `C:\…`).
pub fn same_path(a: &str, b: &str) -> bool {
    let normalize = |path: &str| {
        let path = path.replace('\\', "/");
        let path = path.trim_end_matches('/');
        if cfg!(windows) {
            path.to_lowercase()
        } else {
            path.to_string()
        }
    };
    normalize(a) == normalize(b)
}

/// The `show_document` handler for the editor of file `own_path` (Ctrl+click / F12 to a definition).
///
/// The editor calls it while it is itself being updated, so this handler must not touch that
/// editor: a definition in the same file is handed back to the editor (`false`, the editor moves
/// the cursor itself), and other files are opened after that update has finished.
pub fn show_document_handler(workbench: WeakEntity<Workbench>, own_path: String) -> Rc<dyn Fn(&lsp_types::ShowDocumentParams, &mut Window, &mut App) -> bool> {
    Rc::new(move |params: &lsp_types::ShowDocumentParams, window: &mut Window, cx: &mut App| {
        if params.external == Some(true) {
            return false;
        }
        let target = oncode_lsp::uri::uri_to_path(params.uri.as_str());
        if same_path(&target, &own_path) {
            return false;
        }
        let selection = params.selection.unwrap_or_default();
        let options = OpenFile::new(target).at(selection.start.line + 1, selection.start.character, None);
        let workbench = workbench.clone();
        window.defer(cx, move |window, cx| {
            let _ = workbench.update(cx, |workbench, cx| open_file(workbench, options, window, cx));
        });
        true
    })
}
