//! A JSON-RPC/LSP client on top of a raw message transport (unframed JSON text).
//!
//! The client does not know how messages are sent: outgoing messages are placed on the channel returned by
//! [`Client::new`] (in call order), and incoming messages are handed over via
//! [`Client::receive`]. That way the same client is used on top of [`LspManager`](crate::LspManager),
//! on top of the `oncode-core` API, and on top of an in-process mock server.
//!
//! A request is sent **when the function is called**, not when its future is first polled,
//! so a `did_change` called earlier always arrives at the server earlier. The result
//! future uses the tokio timer and must be awaited inside a tokio runtime. Dropping the future
//! before it completes cancels its request (`$/cancelRequest`).

use std::collections::HashMap;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use lsp_types::notification::Notification;
use lsp_types::request::Request;
use lsp_types::{
    ApplyWorkspaceEditParams, ApplyWorkspaceEditResponse, ClientInfo, ConfigurationItem, ConfigurationParams, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, InitializeParams, InitializeResult, InitializedParams, RegistrationParams, ServerCapabilities, ServerInfo,
    ShowDocumentParams, ShowDocumentResult, TextDocumentContentChangeEvent, TextDocumentIdentifier, TextDocumentItem, TextDocumentSyncKind, UnregistrationParams, Uri,
    VersionedTextDocumentIdentifier, WorkspaceFolder,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use crate::capabilities::{client_capabilities, server_supports, Feature, SyncOptions};
use crate::position::PositionEncoding;
use crate::uri::path_to_lsp_uri;

/// JSON-RPC error codes the client uses when answering the server.
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

fn lock<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RpcError {
    #[error("The language server did not answer `{method}` in time")]
    Timeout { method: String },
    #[error("The request was cancelled")]
    Cancelled,
    #[error("The language server is not running")]
    Closed,
    #[error("The language server has not finished starting")]
    NotReady,
    /// An error sent by the server (`error` in a JSON-RPC response).
    #[error("{message}")]
    Server { code: i64, message: String },
    #[error("Unexpected answer from the language server: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// The client's name and version for `clientInfo`.
    pub name: String,
    pub version: Option<String>,
    /// Absolute path of the project folder; becomes `rootUri` and the only `workspaceFolders` entry.
    pub root: Option<String>,
    /// Timeout of each request.
    pub request_timeout: Duration,
    /// Timeout of `initialize` (a large server needs time to load the project).
    pub initialize_timeout: Duration,
    pub initialization_options: Option<Value>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            name: "OnCode".into(),
            version: None,
            root: None,
            request_timeout: Duration::from_secs(10),
            initialize_timeout: Duration::from_secs(30),
            initialization_options: None,
        }
    }
}

/// Receiver of messages from the server that are not responses to requests. Called from the transport thread,
/// so it must be fast and must not block (usually it just forwards to a channel owned by the UI).
pub trait ClientHandler: Send + Sync + 'static {
    /// A notification from the server, e.g. `textDocument/publishDiagnostics` or `window/showMessage`.
    fn notification(&self, method: &str, params: Value);

    /// `workspace/applyEdit`. The answer is sent via `respond`; if `respond` is dropped without
    /// answering, the server receives `applied: false`.
    fn apply_edit(&self, params: ApplyWorkspaceEditParams, respond: Responder<ApplyWorkspaceEditResponse>) {
        let _ = (params, respond);
    }

    /// `window/showDocument`. Without an answer, the server receives `success: false`.
    fn show_document(&self, params: ShowDocumentParams, respond: Responder<ShowDocumentResult>) {
        let _ = (params, respond);
    }

    /// `workspace/configuration`: one value per item; `null` means "no setting".
    fn configuration(&self, items: &[ConfigurationItem]) -> Vec<Value> {
        vec![Value::Null; items.len()]
    }
}

/// A pending answer to a request from the server. It always answers: if dropped without
/// [`Responder::respond`], a fallback value is sent so the server does not wait forever.
pub struct Responder<T: Serialize> {
    client: Weak<Inner>,
    id: Value,
    fallback: Option<Value>,
    _marker: PhantomData<fn(T)>,
}

impl<T: Serialize> Responder<T> {
    fn new(client: &Arc<Inner>, id: Value, fallback: &T) -> Self {
        Self { client: Arc::downgrade(client), id, fallback: serde_json::to_value(fallback).ok(), _marker: PhantomData }
    }

    pub fn respond(mut self, result: T) {
        self.fallback = None;
        if let Some(client) = self.client.upgrade() {
            client.send(json!({ "jsonrpc": "2.0", "id": self.id, "result": serde_json::to_value(result).unwrap_or(Value::Null) }));
        }
    }
}

impl<T: Serialize> Drop for Responder<T> {
    fn drop(&mut self) {
        if let (Some(fallback), Some(client)) = (self.fallback.take(), self.client.upgrade()) {
            client.send(json!({ "jsonrpc": "2.0", "id": self.id, "result": fallback }));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Created,
    Initializing,
    Ready,
    ShuttingDown,
}

struct Document {
    version: i32,
}

struct State {
    phase: Phase,
    capabilities: ServerCapabilities,
    server_info: Option<ServerInfo>,
    encoding: PositionEncoding,
    sync: SyncOptions,
    /// Documents currently open on the server, keyed by their URI text.
    documents: HashMap<String, Document>,
    /// The last version of each URI; not repeated even if the document is closed and then reopened.
    versions: HashMap<String, i32>,
    /// Dynamic registrations (`client/registerCapability`): id → method.
    registrations: HashMap<String, String>,
}

struct Pending {
    method: &'static str,
    tx: oneshot::Sender<Result<Value, RpcError>>,
}

struct Inner {
    config: ClientConfig,
    handler: Arc<dyn ClientHandler>,
    outgoing: mpsc::UnboundedSender<String>,
    next_id: AtomicI64,
    pending: Mutex<HashMap<i64, Pending>>,
    state: Mutex<State>,
    closed: AtomicBool,
}

impl Inner {
    fn send(&self, message: Value) -> bool {
        !self.closed.load(Ordering::SeqCst) && self.outgoing.send(message.to_string()).is_ok()
    }

    fn respond_error(&self, id: Value, code: i64, message: &str) {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }));
    }

    fn respond(&self, id: Value, result: Value) {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }
}

/// Cancels the request if its future is dropped before the answer arrives.
struct CancelGuard {
    client: Weak<Inner>,
    id: i64,
    method: &'static str,
    armed: bool,
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Some(client) = self.client.upgrade() else { return };
        let was_pending = lock(&client.pending).remove(&self.id).is_some();
        // `initialize` must not be canceled per the spec.
        if was_pending && self.method != lsp_types::request::Initialize::METHOD {
            client.send(json!({ "jsonrpc": "2.0", "method": "$/cancelRequest", "params": { "id": self.id } }));
        }
    }
}

/// The client of one language server. Cheap to clone; all clones share the same state.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

impl Client {
    /// Creates the client and its outgoing message channel. The caller drains that channel in order
    /// to the transport, and hands every message from the server to [`Client::receive`].
    pub fn new(config: ClientConfig, handler: Arc<dyn ClientHandler>) -> (Self, mpsc::UnboundedReceiver<String>) {
        let (outgoing, receiver) = mpsc::unbounded_channel();
        let inner = Inner {
            config,
            handler,
            outgoing,
            next_id: AtomicI64::new(1),
            pending: Mutex::new(HashMap::new()),
            state: Mutex::new(State {
                phase: Phase::Created,
                capabilities: ServerCapabilities::default(),
                server_info: None,
                encoding: PositionEncoding::Utf16,
                sync: SyncOptions::default(),
                documents: HashMap::new(),
                versions: HashMap::new(),
                registrations: HashMap::new(),
            }),
            closed: AtomicBool::new(false),
        };
        (Self { inner: Arc::new(inner) }, receiver)
    }

    // ----- State -----

    /// `initialize` has been answered and the client has not been closed.
    pub fn is_ready(&self) -> bool {
        !self.is_closed() && lock(&self.inner.state).phase == Phase::Ready
    }

    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::SeqCst)
    }

    pub fn capabilities(&self) -> ServerCapabilities {
        lock(&self.inner.state).capabilities.clone()
    }

    pub fn server_info(&self) -> Option<ServerInfo> {
        lock(&self.inner.state).server_info.clone()
    }

    /// The column unit agreed with the server.
    pub fn encoding(&self) -> PositionEncoding {
        lock(&self.inner.state).encoding
    }

    pub fn sync_options(&self) -> SyncOptions {
        lock(&self.inner.state).sync
    }

    /// The server supports `feature`, via static capabilities or dynamic registration.
    pub fn supports(&self, feature: Feature) -> bool {
        let state = lock(&self.inner.state);
        state.phase == Phase::Ready && (server_supports(&state.capabilities, feature) || state.registrations.values().any(|method| method == feature.method()))
    }

    /// Characters that trigger completion automatically (e.g. `.`).
    pub fn completion_trigger_characters(&self) -> Vec<String> {
        lock(&self.inner.state).capabilities.completion_provider.as_ref().and_then(|options| options.trigger_characters.clone()).unwrap_or_default()
    }

    /// Characters that trigger and re-trigger signature help (e.g. `(` and `,`).
    pub fn signature_trigger_characters(&self) -> Vec<String> {
        let state = lock(&self.inner.state);
        let Some(options) = state.capabilities.signature_help_provider.as_ref() else { return Vec::new() };
        let mut characters = options.trigger_characters.clone().unwrap_or_default();
        for character in options.retrigger_characters.clone().unwrap_or_default() {
            if !characters.contains(&character) {
                characters.push(character);
            }
        }
        characters
    }

    // ----- Incoming messages -----

    /// Processes a single message from the server (JSON text without the header). A broken message is ignored.
    pub fn receive(&self, json: &str) {
        if self.is_closed() {
            return;
        }
        let Ok(Value::Object(mut message)) = serde_json::from_str::<Value>(json) else { return };
        let id = message.remove("id").filter(|id| !id.is_null());
        let method = match message.remove("method") {
            Some(Value::String(method)) => Some(method),
            _ => None,
        };
        match (method, id) {
            (Some(method), Some(id)) => self.on_request(id, &method, message.remove("params").unwrap_or(Value::Null)),
            (Some(method), None) => self.inner.handler.notification(&method, message.remove("params").unwrap_or(Value::Null)),
            (None, Some(id)) => self.on_response(&id, message),
            (None, None) => {}
        }
    }

    fn on_response(&self, id: &Value, mut message: serde_json::Map<String, Value>) {
        let Some(id) = id.as_i64() else { return };
        // A response to a request that has expired or been canceled no longer has an entry.
        let Some(pending) = lock(&self.inner.pending).remove(&id) else { return };
        let result = match message.remove("error") {
            Some(error) if !error.is_null() => Err(RpcError::Server {
                code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                message: error.get("message").and_then(Value::as_str).unwrap_or("Unknown language server error").to_string(),
            }),
            _ => Ok(message.remove("result").unwrap_or(Value::Null)),
        };
        let _ = pending.tx.send(result);
    }

    fn on_request(&self, id: Value, method: &str, params: Value) {
        let inner = &self.inner;
        fn parse<T: DeserializeOwned>(inner: &Inner, id: &Value, params: Value) -> Option<T> {
            match serde_json::from_value(params) {
                Ok(value) => Some(value),
                Err(error) => {
                    inner.respond_error(id.clone(), INVALID_PARAMS, &error.to_string());
                    None
                }
            }
        }
        match method {
            "workspace/configuration" => {
                let Some(params) = parse::<ConfigurationParams>(inner, &id, params) else { return };
                let mut values = inner.handler.configuration(&params.items);
                values.resize(params.items.len(), Value::Null);
                inner.respond(id, Value::Array(values));
            }
            "client/registerCapability" => {
                let Some(params) = parse::<RegistrationParams>(inner, &id, params) else { return };
                let mut state = lock(&inner.state);
                for registration in params.registrations {
                    state.registrations.insert(registration.id, registration.method);
                }
                drop(state);
                inner.respond(id, Value::Null);
            }
            "client/unregisterCapability" => {
                let Some(params) = parse::<UnregistrationParams>(inner, &id, params) else { return };
                let mut state = lock(&inner.state);
                for unregistration in params.unregisterations {
                    state.registrations.remove(&unregistration.id);
                }
                drop(state);
                inner.respond(id, Value::Null);
            }
            "workspace/applyEdit" => {
                let Some(params) = parse::<ApplyWorkspaceEditParams>(inner, &id, params) else { return };
                let fallback = ApplyWorkspaceEditResponse { applied: false, failure_reason: Some("The edit was not applied".into()), failed_change: None };
                inner.handler.apply_edit(params, Responder::new(inner, id, &fallback));
            }
            "window/showDocument" => {
                let Some(params) = parse::<ShowDocumentParams>(inner, &id, params) else { return };
                inner.handler.show_document(params, Responder::new(inner, id, &ShowDocumentResult { success: false }));
            }
            "workspace/workspaceFolders" => {
                let folders = self.workspace_folders();
                inner.respond(id, serde_json::to_value(folders).unwrap_or(Value::Null));
            }
            // Nothing to do except acknowledge: progress is shown (if needed) from the
            // `$/progress` notification, and refresh requests are answered on the next request.
            "window/workDoneProgress/create"
            | "window/showMessageRequest"
            | "workspace/semanticTokens/refresh"
            | "workspace/inlayHint/refresh"
            | "workspace/inlineValue/refresh"
            | "workspace/codeLens/refresh"
            | "workspace/diagnostic/refresh" => inner.respond(id, Value::Null),
            _ => inner.respond_error(id, METHOD_NOT_FOUND, &format!("Unsupported request: {method}")),
        }
    }

    /// The transport disconnected or the server stopped: all waiting requests fail with
    /// [`RpcError::Closed`], and no more messages are sent or received.
    pub fn close(&self) {
        if self.inner.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let pending: Vec<Pending> = lock(&self.inner.pending).drain().map(|(_, pending)| pending).collect();
        for pending in pending {
            let _ = pending.tx.send(Err(RpcError::Closed));
        }
        lock(&self.inner.state).documents.clear();
    }

    // ----- Requests and notifications -----

    fn start(&self, method: &'static str, params: Value, require_ready: bool) -> Result<(CancelGuard, oneshot::Receiver<Result<Value, RpcError>>), RpcError> {
        if self.is_closed() {
            return Err(RpcError::Closed);
        }
        if require_ready && lock(&self.inner.state).phase != Phase::Ready {
            return Err(RpcError::NotReady);
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        lock(&self.inner.pending).insert(id, Pending { method, tx });
        let mut message = json!({ "jsonrpc": "2.0", "id": id, "method": method });
        if !params.is_null() {
            message["params"] = params;
        }
        if !self.inner.send(message) {
            lock(&self.inner.pending).remove(&id);
            return Err(RpcError::Closed);
        }
        Ok((CancelGuard { client: Arc::downgrade(&self.inner), id, method, armed: true }, rx))
    }

    fn request_inner<R>(&self, params: R::Params, timeout: Duration, require_ready: bool) -> impl Future<Output = Result<R::Result, RpcError>> + Send + 'static
    where
        R: Request,
        R::Params: Serialize,
        R::Result: DeserializeOwned + Send + 'static,
    {
        let started = serde_json::to_value(params).map_err(|error| RpcError::Invalid(error.to_string())).and_then(|params| self.start(R::METHOD, params, require_ready));
        async move {
            let (mut guard, rx) = started?;
            let outcome = tokio::time::timeout(timeout, rx).await;
            match outcome {
                Ok(Ok(result)) => {
                    guard.armed = false;
                    serde_json::from_value(result?).map_err(|error| RpcError::Invalid(format!("{}: {error}", R::METHOD)))
                }
                // The sender vanished without an answer: the client was closed.
                Ok(Err(_)) => {
                    guard.armed = false;
                    Err(RpcError::Closed)
                }
                // `guard` is dropped here and sends `$/cancelRequest`.
                Err(_) => Err(RpcError::Timeout { method: R::METHOD.to_string() }),
            }
        }
    }

    /// Sends a typed request and returns the future of its answer (see the module notes).
    pub fn request<R>(&self, params: R::Params) -> impl Future<Output = Result<R::Result, RpcError>> + Send + 'static
    where
        R: Request,
        R::Params: Serialize,
        R::Result: DeserializeOwned + Send + 'static,
    {
        self.request_inner::<R>(params, self.inner.config.request_timeout, true)
    }

    /// Like [`Client::request`] with its own timeout.
    pub fn request_with_timeout<R>(&self, params: R::Params, timeout: Duration) -> impl Future<Output = Result<R::Result, RpcError>> + Send + 'static
    where
        R: Request,
        R::Params: Serialize,
        R::Result: DeserializeOwned + Send + 'static,
    {
        self.request_inner::<R>(params, timeout, true)
    }

    /// Sends a notification. The result is `false` if the client has been closed.
    pub fn notify<N>(&self, params: N::Params) -> bool
    where
        N: Notification,
        N::Params: Serialize,
    {
        let Ok(params) = serde_json::to_value(params) else { return false };
        let mut message = json!({ "jsonrpc": "2.0", "method": N::METHOD });
        if !params.is_null() {
            message["params"] = params;
        }
        self.inner.send(message)
    }

    // ----- Lifecycle -----

    fn workspace_folders(&self) -> Option<Vec<WorkspaceFolder>> {
        let root = self.inner.config.root.as_ref()?;
        let uri = path_to_lsp_uri(root)?;
        let name = root.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(root).to_string();
        Some(vec![WorkspaceFolder { uri, name }])
    }

    /// The handshake: `initialize`, storing the server's capabilities, then `initialized`.
    pub fn initialize(&self) -> impl Future<Output = Result<InitializeResult, RpcError>> + Send + 'static {
        let config = &self.inner.config;
        let folders = self.workspace_folders();
        #[allow(deprecated)]
        let params = InitializeParams {
            process_id: Some(std::process::id()),
            root_path: config.root.clone(),
            root_uri: folders.as_ref().and_then(|folders| folders.first()).map(|folder| folder.uri.clone()),
            initialization_options: config.initialization_options.clone(),
            capabilities: client_capabilities(),
            workspace_folders: folders,
            client_info: Some(ClientInfo { name: config.name.clone(), version: config.version.clone() }),
            ..Default::default()
        };
        let already_started = {
            let mut state = lock(&self.inner.state);
            let started = state.phase != Phase::Created;
            if !started {
                state.phase = Phase::Initializing;
            }
            started
        };
        let request = (!already_started).then(|| self.request_inner::<lsp_types::request::Initialize>(params, config.initialize_timeout, false));
        let client = self.clone();
        async move {
            let Some(request) = request else { return Err(RpcError::Invalid("initialize was already sent".into())) };
            let result = request.await?;
            {
                let mut state = lock(&client.inner.state);
                state.encoding = PositionEncoding::from_kind(result.capabilities.position_encoding.as_ref());
                state.sync = SyncOptions::from_capabilities(&result.capabilities);
                state.capabilities = result.capabilities.clone();
                state.server_info = result.server_info.clone();
                state.phase = Phase::Ready;
            }
            if !client.notify::<lsp_types::notification::Initialized>(InitializedParams {}) {
                return Err(RpcError::Closed);
            }
            Ok(result)
        }
    }

    /// A clean shutdown: `shutdown`, then `exit`. A `shutdown` error (e.g. a timeout) does not
    /// prevent `exit`. After this the client is closed.
    pub fn shutdown(&self, timeout: Duration) -> impl Future<Output = ()> + Send + 'static {
        let was_ready = {
            let mut state = lock(&self.inner.state);
            let ready = state.phase == Phase::Ready;
            state.phase = Phase::ShuttingDown;
            ready
        };
        let request = was_ready.then(|| self.request_inner::<lsp_types::request::Shutdown>((), timeout, false));
        let client = self.clone();
        async move {
            if let Some(request) = request {
                let _ = request.await;
            }
            client.notify::<lsp_types::notification::Exit>(());
            client.close();
        }
    }

    // ----- Document synchronization -----

    fn document_ready(&self) -> Result<(), RpcError> {
        if self.is_closed() {
            return Err(RpcError::Closed);
        }
        if lock(&self.inner.state).phase != Phase::Ready {
            return Err(RpcError::NotReady);
        }
        Ok(())
    }

    /// This document is currently open on the server.
    pub fn is_open(&self, uri: &Uri) -> bool {
        lock(&self.inner.state).documents.contains_key(uri.as_str())
    }

    /// The document version last sent to the server; `None` if it is not open.
    pub fn version(&self, uri: &Uri) -> Option<i32> {
        lock(&self.inner.state).documents.get(uri.as_str()).map(|document| document.version)
    }

    /// `textDocument/didOpen`. Returns the document version; a document that is already open is not
    /// opened again (its current version is returned).
    pub fn did_open(&self, uri: &Uri, language_id: &str, text: String) -> Result<i32, RpcError> {
        self.document_ready()?;
        let (version, send) = {
            let mut state = lock(&self.inner.state);
            if let Some(document) = state.documents.get(uri.as_str()) {
                return Ok(document.version);
            }
            let version = state.versions.get(uri.as_str()).copied().unwrap_or(0) + 1;
            state.versions.insert(uri.as_str().to_string(), version);
            state.documents.insert(uri.as_str().to_string(), Document { version });
            (version, state.sync.open_close)
        };
        if send {
            self.notify::<lsp_types::notification::DidOpenTextDocument>(DidOpenTextDocumentParams {
                text_document: TextDocumentItem { uri: uri.clone(), language_id: language_id.to_string(), version, text },
            });
        }
        Ok(version)
    }

    /// `textDocument/didChange`. `changes` are incremental changes (each item refers to the
    /// document after the previous item, with positions in [`Client::encoding`]); `full_text` is only
    /// called if the server asks for full synchronization. Returns the new version, or `None`
    /// if the server does not want changes or nothing changed.
    pub fn did_change(&self, uri: &Uri, changes: Vec<TextDocumentContentChangeEvent>, full_text: impl FnOnce() -> String) -> Result<Option<i32>, RpcError> {
        self.document_ready()?;
        if changes.is_empty() {
            return Ok(None);
        }
        let (version, kind) = {
            let mut state = lock(&self.inner.state);
            let kind = state.sync.change;
            if kind == TextDocumentSyncKind::NONE {
                return Ok(None);
            }
            let Some(document) = state.documents.get_mut(uri.as_str()) else { return Err(RpcError::Invalid(format!("document is not open: {}", uri.as_str()))) };
            document.version += 1;
            let version = document.version;
            state.versions.insert(uri.as_str().to_string(), version);
            (version, kind)
        };
        let content_changes = if kind == TextDocumentSyncKind::INCREMENTAL { changes } else { vec![TextDocumentContentChangeEvent { range: None, range_length: None, text: full_text() }] };
        self.notify::<lsp_types::notification::DidChangeTextDocument>(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier { uri: uri.clone(), version },
            content_changes,
        });
        Ok(Some(version))
    }

    /// `textDocument/didSave`; `text` is only called if the server asks for the contents to be included.
    pub fn did_save(&self, uri: &Uri, text: impl FnOnce() -> String) -> Result<(), RpcError> {
        self.document_ready()?;
        let save = {
            let state = lock(&self.inner.state);
            if !state.documents.contains_key(uri.as_str()) {
                return Ok(());
            }
            state.sync.save
        };
        if let Some(include_text) = save {
            self.notify::<lsp_types::notification::DidSaveTextDocument>(DidSaveTextDocumentParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                text: include_text.then(text),
            });
        }
        Ok(())
    }

    /// `textDocument/didClose`. A document that is not open is ignored.
    pub fn did_close(&self, uri: &Uri) -> Result<(), RpcError> {
        self.document_ready()?;
        let send = {
            let mut state = lock(&self.inner.state);
            if state.documents.remove(uri.as_str()).is_none() {
                return Ok(());
            }
            state.sync.open_close
        };
        if send {
            self.notify::<lsp_types::notification::DidCloseTextDocument>(DidCloseTextDocumentParams { text_document: TextDocumentIdentifier { uri: uri.clone() } });
        }
        Ok(())
    }
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").field("ready", &self.is_ready()).field("closed", &self.is_closed()).finish()
    }
}

/// Request methods currently awaiting an answer, ordered by id (for diagnosis and tests).
impl Client {
    pub fn pending_methods(&self) -> Vec<&'static str> {
        let pending = lock(&self.inner.pending);
        let mut entries: Vec<(i64, &'static str)> = pending.iter().map(|(id, pending)| (*id, pending.method)).collect();
        entries.sort_unstable();
        entries.into_iter().map(|(_, method)| method).collect()
    }
}
