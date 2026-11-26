//! Client capabilities announced at `initialize`, and server capability checks.

use lsp_types::{
    ClientCapabilities, CodeActionProviderCapability, HoverProviderCapability, OneOf, SaveOptions, ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncSaveOptions,
};
use serde_json::json;

/// OnCode's client capabilities. Built as JSON to make it easy to compare against the spec;
/// the test below makes sure there are no misspelled field names.
pub(crate) fn client_capabilities_json() -> serde_json::Value {
    json!({
        "general": {
            // Order = preference. UTF-16 must be supported by all servers and is the last choice.
            "positionEncodings": ["utf-8", "utf-32", "utf-16"],
            "staleRequestSupport": { "cancel": true, "retryOnContentModified": [] }
        },
        "workspace": {
            "applyEdit": true,
            "workspaceEdit": { "documentChanges": true, "failureHandling": "abort" },
            "configuration": true,
            "workspaceFolders": true,
            "didChangeConfiguration": { "dynamicRegistration": false }
        },
        "textDocument": {
            "synchronization": { "dynamicRegistration": false, "willSave": false, "willSaveWaitUntil": false, "didSave": true },
            "completion": {
                "dynamicRegistration": false,
                "contextSupport": true,
                "completionItem": {
                    // The editor inserts text as-is; snippets are not supported.
                    "snippetSupport": false,
                    "documentationFormat": ["markdown", "plaintext"],
                    "deprecatedSupport": true,
                    "insertReplaceSupport": true,
                    "resolveSupport": { "properties": ["documentation", "detail", "additionalTextEdits"] }
                }
            },
            "hover": { "dynamicRegistration": false, "contentFormat": ["markdown", "plaintext"] },
            "signatureHelp": {
                "dynamicRegistration": false,
                "contextSupport": true,
                "signatureInformation": {
                    "documentationFormat": ["markdown", "plaintext"],
                    "parameterInformation": { "labelOffsetSupport": true },
                    "activeParameterSupport": true
                }
            },
            "definition": { "dynamicRegistration": false, "linkSupport": true },
            "references": { "dynamicRegistration": false },
            "rename": { "dynamicRegistration": false, "prepareSupport": true },
            "formatting": { "dynamicRegistration": false },
            "codeAction": {
                "dynamicRegistration": false,
                "isPreferredSupport": true,
                "dataSupport": true,
                "resolveSupport": { "properties": ["edit"] },
                "codeActionLiteralSupport": {
                    "codeActionKind": { "valueSet": ["", "quickfix", "refactor", "refactor.extract", "refactor.inline", "refactor.rewrite", "source", "source.organizeImports"] }
                }
            },
            "publishDiagnostics": { "relatedInformation": true, "versionSupport": true, "codeDescriptionSupport": true, "dataSupport": true, "tagSupport": { "valueSet": [1, 2] } }
        },
        "window": { "workDoneProgress": true, "showDocument": { "support": true } }
    })
}

pub fn client_capabilities() -> ClientCapabilities {
    serde_json::from_value(client_capabilities_json()).unwrap_or_default()
}

/// Server features the UI uses; checked before a request is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    Completion,
    CompletionResolve,
    Hover,
    SignatureHelp,
    Definition,
    References,
    Rename,
    PrepareRename,
    Formatting,
    CodeAction,
    CodeActionResolve,
}

impl Feature {
    /// The method that registers this feature via `client/registerCapability`.
    pub fn method(self) -> &'static str {
        match self {
            Self::Completion => "textDocument/completion",
            Self::CompletionResolve => "completionItem/resolve",
            Self::Hover => "textDocument/hover",
            Self::SignatureHelp => "textDocument/signatureHelp",
            Self::Definition => "textDocument/definition",
            Self::References => "textDocument/references",
            Self::Rename => "textDocument/rename",
            Self::PrepareRename => "textDocument/prepareRename",
            Self::Formatting => "textDocument/formatting",
            Self::CodeAction => "textDocument/codeAction",
            Self::CodeActionResolve => "codeAction/resolve",
        }
    }
}

fn one_of<T>(value: &Option<OneOf<bool, T>>) -> bool {
    !matches!(value, None | Some(OneOf::Left(false)))
}

/// Whether the server's static capabilities include `feature`.
pub fn server_supports(capabilities: &ServerCapabilities, feature: Feature) -> bool {
    match feature {
        Feature::Completion => capabilities.completion_provider.is_some(),
        Feature::CompletionResolve => capabilities.completion_provider.as_ref().is_some_and(|options| options.resolve_provider == Some(true)),
        Feature::Hover => !matches!(capabilities.hover_provider, None | Some(HoverProviderCapability::Simple(false))),
        Feature::SignatureHelp => capabilities.signature_help_provider.is_some(),
        Feature::Definition => one_of(&capabilities.definition_provider),
        Feature::References => one_of(&capabilities.references_provider),
        Feature::Rename => one_of(&capabilities.rename_provider),
        Feature::PrepareRename => matches!(&capabilities.rename_provider, Some(OneOf::Right(options)) if options.prepare_provider == Some(true)),
        Feature::Formatting => one_of(&capabilities.document_formatting_provider),
        Feature::CodeAction => !matches!(capabilities.code_action_provider, None | Some(CodeActionProviderCapability::Simple(false))),
        Feature::CodeActionResolve => {
            matches!(&capabilities.code_action_provider, Some(CodeActionProviderCapability::Options(options)) if options.resolve_provider == Some(true))
        }
    }
}

/// How documents are synchronized with the server, derived from `textDocumentSync`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncOptions {
    pub open_close: bool,
    pub change: TextDocumentSyncKind,
    /// `None`: the server does not want `didSave`. `Some(true)`: include the file contents.
    pub save: Option<bool>,
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self { open_close: false, change: TextDocumentSyncKind::NONE, save: None }
    }
}

impl SyncOptions {
    pub fn from_capabilities(capabilities: &ServerCapabilities) -> Self {
        match &capabilities.text_document_sync {
            None => Self::default(),
            // The short form: open/close and save (without contents) are sent as long as the kind is not `None`.
            Some(TextDocumentSyncCapability::Kind(kind)) => {
                let active = *kind != TextDocumentSyncKind::NONE;
                Self { open_close: active, change: *kind, save: active.then_some(false) }
            }
            Some(TextDocumentSyncCapability::Options(options)) => Self {
                open_close: options.open_close.unwrap_or(false),
                change: options.change.unwrap_or(TextDocumentSyncKind::NONE),
                save: match &options.save {
                    None | Some(TextDocumentSyncSaveOptions::Supported(false)) => None,
                    Some(TextDocumentSyncSaveOptions::Supported(true)) => Some(false),
                    Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions { include_text })) => Some(include_text.unwrap_or(false)),
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drops `null` values so the re-serialized result can be compared with the source JSON.
    fn strip_nulls(value: serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => serde_json::Value::Object(map.into_iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k, strip_nulls(v))).collect()),
            serde_json::Value::Array(items) => serde_json::Value::Array(items.into_iter().map(strip_nulls).collect()),
            other => other,
        }
    }

    #[test]
    fn client_capabilities_match_the_typed_schema() {
        let source = client_capabilities_json();
        let typed: ClientCapabilities = serde_json::from_value(source.clone()).expect("client capabilities must match the lsp-types schema");
        // Misspelled fields will be lost when re-serialized.
        assert_eq!(strip_nulls(serde_json::to_value(&typed).unwrap()), source);
        assert_eq!(typed.text_document.unwrap().completion.unwrap().completion_item.unwrap().snippet_support, Some(false));
    }

    fn capabilities(json: serde_json::Value) -> ServerCapabilities {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn features_follow_static_capabilities() {
        let none = ServerCapabilities::default();
        for feature in [Feature::Completion, Feature::Hover, Feature::Definition, Feature::References, Feature::Rename, Feature::Formatting, Feature::CodeAction, Feature::SignatureHelp] {
            assert!(!server_supports(&none, feature), "{feature:?}");
        }
        let full = capabilities(json!({
            "completionProvider": { "resolveProvider": true, "triggerCharacters": ["."] },
            "hoverProvider": true,
            "signatureHelpProvider": { "triggerCharacters": ["("] },
            "definitionProvider": true,
            "referencesProvider": { "workDoneProgress": false },
            "renameProvider": { "prepareProvider": true },
            "documentFormattingProvider": true,
            "codeActionProvider": { "resolveProvider": true }
        }));
        for feature in [
            Feature::Completion,
            Feature::CompletionResolve,
            Feature::Hover,
            Feature::SignatureHelp,
            Feature::Definition,
            Feature::References,
            Feature::Rename,
            Feature::PrepareRename,
            Feature::Formatting,
            Feature::CodeAction,
            Feature::CodeActionResolve,
        ] {
            assert!(server_supports(&full, feature), "{feature:?}");
        }
        let partial = capabilities(json!({ "completionProvider": {}, "hoverProvider": false, "definitionProvider": false, "renameProvider": true, "codeActionProvider": true }));
        assert!(server_supports(&partial, Feature::Completion));
        assert!(!server_supports(&partial, Feature::CompletionResolve));
        assert!(!server_supports(&partial, Feature::Hover));
        assert!(!server_supports(&partial, Feature::Definition));
        assert!(server_supports(&partial, Feature::Rename));
        assert!(!server_supports(&partial, Feature::PrepareRename));
        assert!(server_supports(&partial, Feature::CodeAction));
        assert!(!server_supports(&partial, Feature::CodeActionResolve));
    }

    #[test]
    fn sync_options_cover_both_capability_shapes() {
        assert_eq!(SyncOptions::from_capabilities(&ServerCapabilities::default()), SyncOptions::default());
        let short = SyncOptions::from_capabilities(&capabilities(json!({ "textDocumentSync": 2 })));
        assert_eq!(short, SyncOptions { open_close: true, change: TextDocumentSyncKind::INCREMENTAL, save: Some(false) });
        let off = SyncOptions::from_capabilities(&capabilities(json!({ "textDocumentSync": 0 })));
        assert_eq!(off, SyncOptions { open_close: false, change: TextDocumentSyncKind::NONE, save: None });
        let long = SyncOptions::from_capabilities(&capabilities(json!({ "textDocumentSync": { "openClose": true, "change": 1, "save": { "includeText": true } } })));
        assert_eq!(long, SyncOptions { open_close: true, change: TextDocumentSyncKind::FULL, save: Some(true) });
        let no_save = SyncOptions::from_capabilities(&capabilities(json!({ "textDocumentSync": { "openClose": true, "change": 2, "save": false } })));
        assert_eq!(no_save.save, None);
    }
}
