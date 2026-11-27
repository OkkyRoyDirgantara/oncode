//! Go to Symbol in Editor (Ctrl+Shift+O) and Go to Symbol in Workspace (Ctrl+T).
//!
//! Symbols come from registered providers (the language server, registered by `oncode-ui-lsp`) first.
//! When no provider answers, a text-pattern search takes over (`oncode_workspace::outline`): the file's
//! own text for the editor list, and the project search for the workspace list. Those results are
//! labeled as text matches, since they are approximate.

use std::rc::Rc;

use gpui_kit::component::input::EditorState;
use gpui_kit::{App, Entity, Global, Task, WeakEntity, Window};
use oncode_core::api::symbols as symbols_api;

use super::buffer::Buffer;
use super::language::language_for_path;
use super::{open_file, FileView, OpenFile, Reveal};
use crate::quick_pick::{quick_pick, PickItem, QuickPickOptions};
use crate::workbench::Workbench;
use crate::{overlay, spawn_core};

/// One symbol in a Go to Symbol list.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolItem {
    pub name: String,
    /// What it is (`function`, `class`, `method`, …), shown next to the name.
    pub kind: String,
    /// The symbol it belongs to (a class for a method), if known.
    pub container: Option<String>,
    pub path: String,
    /// Starts from 1.
    pub line: u32,
    /// Characters, starts from 0.
    pub column: u32,
    /// Nesting depth in the file's outline; 0 at the top.
    pub depth: usize,
}

/// The symbols of one file. `None` (immediately, or from the task) means this provider cannot answer
/// for the file, and the next one is asked.
pub type DocumentSymbolProvider = Rc<dyn Fn(&Entity<Buffer>, &mut Window, &mut App) -> Option<Task<Option<Vec<SymbolItem>>>>>;

/// Symbols across the workspace of a window matching a query; `None` like [`DocumentSymbolProvider`].
pub type WorkspaceSymbolProvider = Rc<dyn Fn(&WeakEntity<Workbench>, String, &mut App) -> Option<Task<Option<Vec<SymbolItem>>>>>;

#[derive(Default)]
struct SymbolProviders {
    document: Vec<DocumentSymbolProvider>,
    workspace: Vec<WorkspaceSymbolProvider>,
}

impl Global for SymbolProviders {}

pub fn register_document_symbol_provider(provider: impl Fn(&Entity<Buffer>, &mut Window, &mut App) -> Option<Task<Option<Vec<SymbolItem>>>> + 'static, cx: &mut App) {
    cx.default_global::<SymbolProviders>().document.push(Rc::new(provider));
}

pub fn register_workspace_symbol_provider(provider: impl Fn(&WeakEntity<Workbench>, String, &mut App) -> Option<Task<Option<Vec<SymbolItem>>>> + 'static, cx: &mut App) {
    cx.default_global::<SymbolProviders>().workspace.push(Rc::new(provider));
}

/// The outline of the buffer's text from the text patterns, for when no provider answers.
fn text_outline(path: &str, text: &str) -> Vec<SymbolItem> {
    let language = language_for_path(path).lsp;
    oncode_workspace::outline::outline(text, language)
        .into_iter()
        .map(|symbol| SymbolItem { name: symbol.name, kind: symbol.kind, container: None, path: path.to_string(), line: symbol.line, column: symbol.column, depth: symbol.depth })
        .collect()
}

fn encode(line: u32, column: u32) -> String {
    format!("{line}:{column}")
}

fn decode(value: &str) -> Option<Reveal> {
    let (line, column) = value.split_once(':')?;
    Some(Reveal { line: line.parse().ok()?, column: column.parse().ok()?, length: None })
}

/// Go to Symbol in Editor: lists the file's symbols; moving through the list shows each one in the
/// editor, Enter goes there, and Escape returns to where the cursor was.
pub(crate) fn goto_symbol_in_editor(view: Entity<FileView>, buffer: Entity<Buffer>, state: Entity<EditorState>, window: &mut Window, cx: &mut App) {
    let providers: Vec<DocumentSymbolProvider> = cx.try_global::<SymbolProviders>().map(|p| p.document.clone()).unwrap_or_default();
    window
        .spawn(cx, async move |cx| {
            let mut found = None;
            for provider in providers {
                let Ok(Some(task)) = cx.update(|window, cx| provider(&buffer, window, cx)) else { continue };
                if let Some(items) = task.await.filter(|items| !items.is_empty()) {
                    found = Some(items);
                    break;
                }
            }
            let _ = cx.update(|window, cx| {
                let approximate = found.is_none();
                let items = found.unwrap_or_else(|| {
                    let path = buffer.read(cx).path().to_string();
                    text_outline(&path, &state.read(cx).text().to_string())
                });
                show_editor_symbols(view, state, items, approximate, window, cx);
            });
        })
        .detach();
}

fn show_editor_symbols(view: Entity<FileView>, state: Entity<EditorState>, items: Vec<SymbolItem>, approximate: bool, window: &mut Window, cx: &mut App) {
    if items.is_empty() {
        overlay::toast_info("No symbols found in this file.", window, cx);
        return;
    }
    let original = state.read(cx).selected_range();
    let picks = items
        .iter()
        .map(|item| {
            let label = format!("{}{}", "  ".repeat(item.depth.min(8)), item.name);
            let kind = if approximate { format!("{} (text match)", item.kind) } else { item.kind.clone() };
            let description = match &item.container {
                Some(container) => format!("{kind} · {container}"),
                None => kind,
            };
            PickItem::new(label, encode(item.line, item.column)).description(description).hint(format!("{}", item.line))
        })
        .collect();
    let preview = view.downgrade();
    let options = QuickPickOptions::new("Go to symbol in editor", picks).on_highlight(move |item, window, cx| {
        if let Some(reveal) = decode(&item.value) {
            let _ = preview.update(cx, |view, cx| view.preview_reveal(&reveal, window, cx));
        }
    });
    let target = view.downgrade();
    quick_pick(
        options,
        move |item, window, cx| {
            let _ = target.update(cx, |view, cx| match item.as_ref().and_then(|item| decode(&item.value)) {
                Some(reveal) => view.reveal(&reveal, window, cx),
                None => view.restore_selection(original.clone(), window, cx),
            });
        },
        window,
        cx,
    );
}

/// Most symbols the workspace list shows.
const WORKSPACE_LIMIT: usize = 200;

/// Go to Symbol in Workspace: searches symbols across the project as you type.
pub(crate) fn goto_symbol_in_workspace(workbench: &Entity<Workbench>, window: &mut Window, cx: &mut App) {
    let providers: Vec<WorkspaceSymbolProvider> = cx.try_global::<SymbolProviders>().map(|p| p.workspace.clone()).unwrap_or_default();
    let env = workbench.read(cx).env().clone();
    let root = env.root_string();
    let weak = workbench.downgrade();
    let search_workbench = weak.clone();
    let options = QuickPickOptions::with_search("Go to symbol in workspace (type part of a name)", move |query, cx| {
        let query = query.trim().to_string();
        if query.is_empty() {
            return Task::ready(Vec::new());
        }
        let providers = providers.clone();
        let (core, label) = (env.core.clone(), env.window.to_string());
        let (root, weak) = (root.clone(), search_workbench.clone());
        let pending: Vec<Task<Option<Vec<SymbolItem>>>> = providers.iter().filter_map(|provider| provider(&weak, query.clone(), cx)).collect();
        let text_search = spawn_core(cx, async move { symbols_api::symbol_search(&core, &label, query).await });
        cx.spawn(async move |_| {
            let mut found = None;
            for task in pending {
                if let Some(items) = task.await.filter(|items| !items.is_empty()) {
                    found = Some(items);
                    break;
                }
            }
            let (items, approximate) = match found {
                Some(items) => (items, false),
                None => {
                    let located = text_search.await.unwrap_or_default();
                    let items = located
                        .into_iter()
                        .map(|s| SymbolItem { name: s.name, kind: s.kind, container: None, path: s.path, line: s.line, column: s.column, depth: 0 })
                        .collect();
                    (items, true)
                }
            };
            items
                .into_iter()
                .take(WORKSPACE_LIMIT)
                .map(|item| {
                    let relative = super::relative_to(root.as_deref(), &item.path);
                    let kind = if approximate { format!("{} (text match)", item.kind) } else { item.kind.clone() };
                    let description = match &item.container {
                        Some(container) => format!("{container} · {relative}"),
                        None => relative,
                    };
                    PickItem::new(item.name.clone(), format!("{}\u{0}{}", item.path, encode(item.line, item.column))).description(description).hint(kind)
                })
                .collect()
        })
    })
    .empty_text("No symbols found");
    quick_pick(
        options,
        move |item, window, cx| {
            let Some(item) = item else { return };
            let Some((path, position)) = item.value.split_once('\u{0}') else { return };
            let Some(reveal) = decode(position) else { return };
            let options = OpenFile::new(path).at(reveal.line, reveal.column, None);
            let _ = weak.update(cx, |workbench, cx| open_file(workbench, options, window, cx));
        },
        window,
        cx,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_round_trip_and_the_text_outline_uses_the_language() {
        assert_eq!(decode(&encode(12, 4)), Some(Reveal { line: 12, column: 4, length: None }));
        assert_eq!(decode("x"), None);
        let items = text_outline("/p/notes.md", "# Title\n## Part\n");
        assert_eq!(items.iter().map(|i| (i.name.as_str(), i.depth)).collect::<Vec<_>>(), [("Title", 0), ("Part", 1)]);
    }
}
