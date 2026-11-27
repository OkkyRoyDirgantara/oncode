//! Go to Symbol through the language server: `textDocument/documentSymbol` for the editor list and
//! `workspace/symbol` for the workspace list. Registered as providers in `oncode_ui::editor`, which
//! falls back to its text-pattern search when these return nothing.

use gpui_kit::{App, AppContext as _, Entity, Task, WeakEntity, Window};
use lsp_types::request::{DocumentSymbolRequest, WorkspaceSymbolRequest};
use lsp_types::{
    DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse, OneOf, PartialResultParams, Position, SymbolInformation, SymbolKind, TextDocumentIdentifier, WorkDoneProgressParams,
    WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use oncode_lsp::position::{offset_at, PositionEncoding};
use oncode_lsp::uri::{path_to_lsp_uri, uri_to_path};
use oncode_lsp::Feature;
use oncode_ui::editor::{language_for_path, Buffer, SymbolItem};
use oncode_ui::{spawn_core, Workbench};

/// How a symbol kind is shown next to its name.
pub(crate) fn kind_label(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::FILE => "file",
        SymbolKind::MODULE => "module",
        SymbolKind::NAMESPACE => "namespace",
        SymbolKind::PACKAGE => "package",
        SymbolKind::CLASS => "class",
        SymbolKind::METHOD => "method",
        SymbolKind::PROPERTY => "property",
        SymbolKind::FIELD => "field",
        SymbolKind::CONSTRUCTOR => "constructor",
        SymbolKind::ENUM => "enum",
        SymbolKind::INTERFACE => "interface",
        SymbolKind::FUNCTION => "function",
        SymbolKind::VARIABLE => "variable",
        SymbolKind::CONSTANT => "constant",
        SymbolKind::STRUCT => "struct",
        SymbolKind::EVENT => "event",
        SymbolKind::OPERATOR => "operator",
        SymbolKind::TYPE_PARAMETER => "type parameter",
        SymbolKind::ENUM_MEMBER => "enum member",
        SymbolKind::OBJECT => "object",
        SymbolKind::KEY => "key",
        _ => "symbol",
    }
}

/// Line (from 1) and column (characters, from 0) of an LSP position in `text`.
fn line_and_column(text: &str, position: Position, encoding: PositionEncoding) -> (u32, u32) {
    let offset = offset_at(text, position, encoding).min(text.len());
    let line_start = text[..offset].rfind('\n').map_or(0, |index| index + 1);
    (position.line + 1, text[line_start..offset].chars().count() as u32)
}

/// Converts a `documentSymbol` answer into list items in document order, nested symbols after their parent.
pub(crate) fn document_items(path: &str, text: &str, response: DocumentSymbolResponse, encoding: PositionEncoding) -> Vec<SymbolItem> {
    let mut items = Vec::new();
    match response {
        DocumentSymbolResponse::Flat(symbols) => {
            let mut symbols: Vec<SymbolInformation> = symbols;
            symbols.sort_by_key(|symbol| (symbol.location.range.start.line, symbol.location.range.start.character));
            for symbol in symbols {
                let (line, column) = line_and_column(text, symbol.location.range.start, encoding);
                items.push(SymbolItem { name: symbol.name, kind: kind_label(symbol.kind).into(), container: symbol.container_name, path: path.to_string(), line, column, depth: 0 });
            }
        }
        DocumentSymbolResponse::Nested(symbols) => {
            fn walk(symbols: Vec<DocumentSymbol>, parent: Option<&str>, depth: usize, path: &str, text: &str, encoding: PositionEncoding, items: &mut Vec<SymbolItem>) {
                let mut symbols = symbols;
                symbols.sort_by_key(|symbol| (symbol.selection_range.start.line, symbol.selection_range.start.character));
                for symbol in symbols {
                    let (line, column) = line_and_column(text, symbol.selection_range.start, encoding);
                    let name = symbol.name.clone();
                    items.push(SymbolItem { name: symbol.name, kind: kind_label(symbol.kind).into(), container: parent.map(String::from), path: path.to_string(), line, column, depth });
                    if let Some(children) = symbol.children {
                        walk(children, Some(&name), depth + 1, path, text, encoding, items);
                    }
                }
            }
            walk(symbols, None, 0, path, text, encoding, &mut items);
        }
    }
    items
}

/// Converts a `workspace/symbol` answer into list items. The files are not open, so the column is the
/// position's character as the server counts it (exact for most code).
pub(crate) fn workspace_items(response: WorkspaceSymbolResponse) -> Vec<SymbolItem> {
    match response {
        WorkspaceSymbolResponse::Flat(symbols) => symbols
            .into_iter()
            .map(|symbol| {
                let start = symbol.location.range.start;
                SymbolItem {
                    name: symbol.name,
                    kind: kind_label(symbol.kind).into(),
                    container: symbol.container_name,
                    path: uri_to_path(symbol.location.uri.as_str()),
                    line: start.line + 1,
                    column: start.character,
                    depth: 0,
                }
            })
            .collect(),
        WorkspaceSymbolResponse::Nested(symbols) => symbols
            .into_iter()
            .map(|symbol| {
                let (path, start) = match symbol.location {
                    OneOf::Left(location) => (uri_to_path(location.uri.as_str()), location.range.start),
                    OneOf::Right(location) => (uri_to_path(location.uri.as_str()), Position::default()),
                };
                SymbolItem { name: symbol.name, kind: kind_label(symbol.kind).into(), container: symbol.container_name, path, line: start.line + 1, column: start.character, depth: 0 }
            })
            .collect(),
    }
}

/// The editor list's provider: the symbols of the buffer's file from its language server.
pub(crate) fn document_symbol_provider(buffer: &Entity<Buffer>, window: &mut Window, cx: &mut App) -> Option<Task<Option<Vec<SymbolItem>>>> {
    let workbench = Workbench::for_window(window, cx)?;
    let hub = crate::hub(workbench.read(cx))?;
    let path = buffer.read(cx).path().to_string();
    let language = language_for_path(&path).lsp.to_string();
    let text = buffer.read(cx).text(cx)?;
    let (client, _) = hub.update(cx, |hub, _| hub.request_context(&language, &path, &text, 0))?;
    if !client.supports(Feature::DocumentSymbol) {
        return None;
    }
    let params = DocumentSymbolParams { text_document: TextDocumentIdentifier { uri: path_to_lsp_uri(&path)? }, work_done_progress_params: WorkDoneProgressParams::default(), partial_result_params: PartialResultParams::default() };
    let encoding = client.encoding();
    let task = spawn_core(cx, client.request::<DocumentSymbolRequest>(params));
    let text = text.to_string();
    Some(cx.background_spawn(async move {
        let response = task.await.ok().flatten()?;
        Some(document_items(&path, &text, response, encoding))
    }))
}

/// The workspace list's provider: asks every running language server that supports it, and merges the answers.
pub(crate) fn workspace_symbol_provider(workbench: &WeakEntity<Workbench>, query: String, cx: &mut App) -> Option<Task<Option<Vec<SymbolItem>>>> {
    let workbench = workbench.upgrade()?;
    let hub = crate::hub(workbench.read(cx))?;
    let clients: Vec<_> = hub.read(cx).servers().filter_map(|server| server.client()).filter(|client| client.supports(Feature::WorkspaceSymbol)).collect();
    if clients.is_empty() {
        return None;
    }
    let tasks: Vec<_> = clients
        .into_iter()
        .map(|client| {
            let params = WorkspaceSymbolParams { query: query.clone(), work_done_progress_params: WorkDoneProgressParams::default(), partial_result_params: PartialResultParams::default() };
            spawn_core(cx, async move { client.request::<WorkspaceSymbolRequest>(params).await })
        })
        .collect();
    Some(cx.background_spawn(async move {
        let mut items = Vec::new();
        for task in tasks {
            if let Ok(Some(response)) = task.await {
                items.extend(workspace_items(response));
            }
        }
        Some(items)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Location, Range};

    fn range(line: u32, character: u32) -> Range {
        Range { start: Position { line, character }, end: Position { line, character: character + 1 } }
    }

    #[allow(deprecated)]
    fn symbol(name: &str, kind: SymbolKind, line: u32, character: u32, children: Option<Vec<DocumentSymbol>>) -> DocumentSymbol {
        DocumentSymbol { name: name.into(), detail: None, kind, tags: None, deprecated: None, range: range(line, character), selection_range: range(line, character), children }
    }

    #[test]
    fn nested_document_symbols_are_flattened_in_order_with_depth() {
        let text = "class Ä {\n  fn run() {}\n}\nfn main() {}\n";
        let response = DocumentSymbolResponse::Nested(vec![
            symbol("main", SymbolKind::FUNCTION, 3, 3, None),
            symbol("Ä", SymbolKind::CLASS, 0, 6, Some(vec![symbol("run", SymbolKind::METHOD, 1, 5, None)])),
        ]);
        let items = document_items("/p/a.rs", text, response, PositionEncoding::Utf16);
        let shown: Vec<(&str, &str, u32, u32, usize, Option<&str>)> = items.iter().map(|i| (i.name.as_str(), i.kind.as_str(), i.line, i.column, i.depth, i.container.as_deref())).collect();
        assert_eq!(shown, [("Ä", "class", 1, 6, 0, None), ("run", "method", 2, 5, 1, Some("Ä")), ("main", "function", 4, 3, 0, None)]);
    }

    #[test]
    #[allow(deprecated)]
    fn workspace_symbols_carry_their_file() {
        let uri = path_to_lsp_uri("/p/src/lib.rs").unwrap();
        let response = WorkspaceSymbolResponse::Flat(vec![SymbolInformation {
            name: "load".into(),
            kind: SymbolKind::FUNCTION,
            tags: None,
            deprecated: None,
            location: Location { uri, range: range(9, 4) },
            container_name: Some("store".into()),
        }]);
        let items = workspace_items(response);
        assert_eq!((items[0].path.as_str(), items[0].line, items[0].column, items[0].container.as_deref()), ("/p/src/lib.rs", 10, 4, Some("store")));
    }
}
