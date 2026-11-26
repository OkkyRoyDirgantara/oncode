//! The bridge from language servers to the editor: one server per language per window, document
//! sync, completion/hover/definition/code action providers for `EditorState`, and diagnostics.
//!
//! The JSON-RPC client (`oncode_lsp::Client`) is safe to use from any thread; all requests run on
//! the backend runtime via `spawn_core`, and the results are forwarded to the main thread.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::input::{CodeActionProvider, CompletionProvider, DefinitionProvider, EditorState, HoverProvider, Rope};
use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Result as GpuiResult, SharedString, Subscription, Task, WeakEntity, Window};
use lsp_types::request::{CodeActionRequest, Completion, GotoDefinition, HoverRequest};
use lsp_types::{
    CodeAction, CodeActionContext, CodeActionOrCommand, CodeActionParams, CompletionContext, CompletionParams, CompletionResponse, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverParams, LocationLink, PartialResultParams, Position, TextDocumentContentChangeEvent, TextDocumentIdentifier,
    TextDocumentPositionParams, WorkDoneProgressParams,
};
use oncode_core::api::lsp as lsp_api;
use oncode_core::api::lsp::LspEvent;
use oncode_lsp::position::{position_at, PositionEncoding};
use oncode_lsp::uri::path_to_lsp_uri;
use oncode_lsp::{Client, ClientConfig, ClientHandler, Feature};
use oncode_ui::editor::{language_for_path, Buffer, BufferEvent, BuffersEvent};
use oncode_ui::{overlay, spawn_core, Env, Workbench};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::diagnostics::Diagnostics;

/// Limit on attempts to restart a server that died on its own.
const MAX_RESTARTS: u32 = 3;

#[derive(Debug, Clone, PartialEq)]
pub enum ServerStatus {
    Starting,
    Running,
    /// The server is not installed or failed to start; the message is for the user.
    Failed(String),
    Stopped,
}

/// A message from the transport thread to the main thread.
enum Incoming {
    Notification { method: String, params: Value },
    Exited { code: Option<i32>, stderr_tail: String },
}

struct Handler(mpsc::UnboundedSender<Incoming>);

impl ClientHandler for Handler {
    fn notification(&self, method: &str, params: Value) {
        let _ = self.0.send(Incoming::Notification { method: method.to_string(), params });
    }
}

/// A single document that is already open on the server.
struct OpenDoc {
    uri: lsp_types::Uri,
    /// The last content sent to the server.
    sent: String,
    _subscriptions: Vec<Subscription>,
}

pub struct Server {
    pub language: String,
    pub status: ServerStatus,
    client: Option<Arc<Client>>,
    id: Option<u32>,
    restarts: u32,
    docs: HashMap<String, OpenDoc>,
    /// Buffers waiting for the server to be ready.
    waiting: Vec<WeakEntity<Buffer>>,
    _tasks: Vec<Task<()>>,
}

impl Server {
    pub fn client(&self) -> Option<Arc<Client>> {
        self.client.clone().filter(|client| client.is_ready())
    }

    pub fn encoding(&self) -> PositionEncoding {
        self.client.as_ref().map(|client| client.encoding()).unwrap_or(PositionEncoding::Utf16)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum HubEvent {
    StatusChanged(String),
}

/// All of a single window's language servers.
pub struct LspHub {
    env: Env,
    servers: HashMap<String, Server>,
    diagnostics: Entity<Diagnostics>,
    /// Languages that have already been told "server is not installed", so it is not repeated.
    warned: HashSet<String>,
    /// Editor state per path, to show diagnostics and apply edits.
    editors: HashMap<String, Vec<WeakEntity<EditorState>>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<HubEvent> for LspHub {}

impl LspHub {
    pub fn new(env: Env, buffers: Entity<oncode_ui::editor::Buffers>, diagnostics: Entity<Diagnostics>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.subscribe_in(&buffers, window, |this: &mut Self, _, event: &BuffersEvent, window, cx| {
                if let BuffersEvent::EditorCreated { buffer, state } = event {
                    this.attach(buffer.clone(), state.clone(), window, cx);
                }
            }),
            cx.subscribe(&diagnostics, |this: &mut Self, _, event: &crate::diagnostics::DiagnosticsEvent, cx| {
                let crate::diagnostics::DiagnosticsEvent::Changed(paths) = event;
                for path in paths {
                    this.show_diagnostics(path, cx);
                }
            }),
        ];
        Self { env, servers: HashMap::new(), diagnostics, warned: HashSet::new(), editors: HashMap::new(), _subscriptions: subscriptions }
    }

    pub fn diagnostics(&self) -> &Entity<Diagnostics> {
        &self.diagnostics
    }

    pub fn server(&self, language: &str) -> Option<&Server> {
        self.servers.get(language)
    }

    pub fn servers(&self) -> impl Iterator<Item = &Server> {
        self.servers.values()
    }

    /// The server for this file, if it is already running.
    pub fn server_for_path(&self, path: &str) -> Option<&Server> {
        self.servers.get(language_for_path(path).lsp).filter(|server| server.status == ServerStatus::Running)
    }

    fn enabled(&self) -> bool {
        self.env.trusted && self.env.core.effective_settings(&self.env.window).lsp.enabled
    }

    /// Connects a file editor: installs the providers and opens its document on the server.
    fn attach(&mut self, buffer: Entity<Buffer>, state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled() {
            return;
        }
        let path = buffer.read(cx).path().to_string();
        let language = language_for_path(&path).lsp.to_string();
        if language == "plaintext" && !self.env.core.effective_settings(&self.env.window).lsp.servers.contains_key("plaintext") {
            return;
        }
        let Some(uri) = path_to_lsp_uri(&path) else { return };
        self.editors.entry(path.clone()).or_default().push(state.downgrade());
        let provider = Rc::new(Provider { hub: cx.weak_entity(), language: language.clone(), path: path.clone(), uri, buffer: buffer.downgrade() });
        let open_path = path.clone();
        let workbench = self.env.workbench.clone();
        state.update(cx, |state, cx| {
            let lsp = state.lsp_mut();
            lsp.completion_provider = Some(provider.clone());
            lsp.hover_provider = Some(provider.clone());
            lsp.definition_provider = Some(provider.clone());
            lsp.code_action_providers = vec![provider.clone()];
            // A definition in another file is opened in an editor tab.
            // A definition in the same file is handled by that editor itself; another file is opened afterwards
            // (the editor is being updated while this handler is called).
            lsp.show_document = Some(oncode_ui::editor::show_document_handler(workbench.clone(), open_path.clone()));
            cx.notify();
        });
        self.show_diagnostics(&path, cx);
        self.open_doc(&language, buffer, window, cx);
    }

    fn open_doc(&mut self, language: &str, buffer: Entity<Buffer>, window: &mut Window, cx: &mut Context<Self>) {
        let (path, ready, large) = {
            let buffer = buffer.read(cx);
            (buffer.path().to_string(), buffer.is_ready(), buffer.is_large())
        };
        if large {
            return;
        }
        if !self.servers.contains_key(language) {
            self.start(language, window, cx);
        }
        let Some(server) = self.servers.get_mut(language) else { return };
        if server.docs.contains_key(&path) {
            return;
        }
        let Some(client) = server.client() else {
            if !server.waiting.iter().any(|waiting| waiting.upgrade().is_some_and(|b| b == buffer)) {
                server.waiting.push(buffer.downgrade());
            }
            return;
        };
        if !ready {
            // Opened once the content has loaded.
            let language = language.to_string();
            let subscription = cx.subscribe_in(&buffer, window, move |this: &mut Self, buffer, event: &BufferEvent, window, cx| {
                if matches!(event, BufferEvent::Loaded) {
                    this.open_doc(&language, buffer.clone(), window, cx);
                }
            });
            self._subscriptions.push(subscription);
            return;
        }
        let Some(uri) = path_to_lsp_uri(&path) else { return };
        let text = buffer.read(cx).text(cx).map(|rope| rope.to_string()).unwrap_or_default();
        let language_id = language_for_path(&path).lsp;
        if client.did_open(&uri, language_id, text.clone()).is_err() {
            return;
        }
        let language_key = language.to_string();
        let subscriptions = vec![
            cx.subscribe(&buffer, {
                let language = language_key.clone();
                move |this: &mut Self, buffer, event: &BufferEvent, cx| match event {
                    BufferEvent::Edited | BufferEvent::Loaded => this.sync(&language, &buffer, cx),
                    BufferEvent::Saved => this.saved(&language, &buffer, cx),
                }
            }),
            cx.observe_release(&buffer, {
                let (language, path) = (language_key.clone(), path.clone());
                move |this: &mut Self, _, _| this.close_doc(&language, &path)
            }),
        ];
        server.docs.insert(path, OpenDoc { uri, sent: text, _subscriptions: subscriptions });
    }

    /// Sends a buffer's latest content if it differs from what was last sent.
    fn sync(&mut self, language: &str, buffer: &Entity<Buffer>, cx: &App) {
        let path = buffer.read(cx).path().to_string();
        let Some(text) = buffer.read(cx).text(cx).map(|rope| rope.to_string()) else { return };
        self.sync_text(language, &path, text);
    }

    fn sync_text(&mut self, language: &str, path: &str, text: String) {
        let Some(server) = self.servers.get_mut(language) else { return };
        let Some(client) = server.client.clone() else { return };
        let Some(doc) = server.docs.get_mut(path) else { return };
        if doc.sent == text {
            return;
        }
        // The whole content is sent as a single change: always valid, also for incremental servers.
        let change = TextDocumentContentChangeEvent { range: None, range_length: None, text: text.clone() };
        let full = text.clone();
        if client.did_change(&doc.uri, vec![change], move || full).is_ok() {
            doc.sent = text;
        }
    }

    fn saved(&mut self, language: &str, buffer: &Entity<Buffer>, cx: &App) {
        self.sync(language, buffer, cx);
        let path = buffer.read(cx).path().to_string();
        let Some(server) = self.servers.get(language) else { return };
        let (Some(client), Some(doc)) = (server.client.clone(), server.docs.get(&path)) else { return };
        let text = doc.sent.clone();
        let _ = client.did_save(&doc.uri, move || text);
    }

    fn close_doc(&mut self, language: &str, path: &str) {
        self.editors.remove(path);
        let Some(server) = self.servers.get_mut(language) else { return };
        if let Some(doc) = server.docs.remove(path) {
            if let Some(client) = &server.client {
                let _ = client.did_close(&doc.uri);
            }
        }
    }

    /// Makes sure the content sent to the server equals the editor's content before a request.
    fn ensure_synced(&mut self, language: &str, path: &str, text: &Rope) {
        self.sync_text(language, path, text.to_string());
    }

    // ----- Server lifecycle -----

    fn start(&mut self, language: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (incoming_tx, mut incoming_rx) = mpsc::unbounded_channel::<Incoming>();
        let config = ClientConfig { name: "OnCode".into(), version: Some(env!("CARGO_PKG_VERSION").into()), root: self.env.root_string(), ..Default::default() };
        let (client, mut outgoing) = Client::new(config, Arc::new(Handler(incoming_tx.clone())));
        let client = Arc::new(client);
        let (core, label, language_key) = (self.env.core.clone(), self.env.window.to_string(), language.to_string());
        let transport_client = client.clone();
        let exit_tx = incoming_tx.clone();
        let start = spawn_core(cx, async move {
            let on_event: oncode_core::api::Callback<LspEvent> = Arc::new(move |event| match event {
                LspEvent::Message { json } => transport_client.receive(&json),
                LspEvent::Exit { code, stderr_tail } => {
                    transport_client.close();
                    let _ = exit_tx.send(Incoming::Exited { code, stderr_tail });
                }
            });
            let id = lsp_api::lsp_start(&core, &label, language_key, on_event).await?;
            // A single writer: outgoing messages are sent in order.
            let writer_core = core.clone();
            tokio::spawn(async move {
                while let Some(json) = outgoing.recv().await {
                    if lsp_api::lsp_send(&writer_core, id, json).await.is_err() {
                        break;
                    }
                }
            });
            Ok::<u32, oncode_core::AppError>(id)
        });
        let init_client = client.clone();
        let language_owned = language.to_string();
        let lifecycle = cx.spawn_in(window, async move |this, cx| {
            let id = match start.await {
                Ok(id) => id,
                Err(error) => {
                    let _ = this.update_in(cx, |this, window, cx| this.failed(&language_owned, error.message, error.code == "serverNotInstalled" || error.code == "notFound", window, cx));
                    return;
                }
            };
            let _ = this.update(cx, |this, _| {
                if let Some(server) = this.servers.get_mut(&language_owned) {
                    server.id = Some(id);
                }
            });
            let initialize = init_client.initialize();
            let Ok(task) = cx.update(|_, cx| spawn_core(cx, initialize)) else { return };
            match task.await {
                Ok(_) => {
                    let _ = this.update_in(cx, |this, window, cx| this.running(&language_owned, window, cx));
                }
                Err(error) => {
                    let _ = this.update_in(cx, |this, window, cx| this.failed(&language_owned, format!("The language server did not start: {error}"), false, window, cx));
                }
            }
        });
        let language_owned = language.to_string();
        let pump = cx.spawn_in(window, async move |this, cx| {
            while let Some(message) = incoming_rx.recv().await {
                let mut batch = vec![message];
                while let Ok(more) = incoming_rx.try_recv() {
                    batch.push(more);
                }
                if this.update_in(cx, |this, window, cx| this.handle(&language_owned, batch, window, cx)).is_err() {
                    return;
                }
            }
        });
        let restarts = self.servers.get(language).map_or(0, |server| server.restarts);
        self.servers.insert(
            language.to_string(),
            Server { language: language.to_string(), status: ServerStatus::Starting, client: Some(client), id: None, restarts, docs: HashMap::new(), waiting: Vec::new(), _tasks: vec![lifecycle, pump] },
        );
        cx.emit(HubEvent::StatusChanged(language.to_string()));
        cx.notify();
    }

    fn running(&mut self, language: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(server) = self.servers.get_mut(language) else { return };
        server.status = ServerStatus::Running;
        let waiting: Vec<Entity<Buffer>> = server.waiting.drain(..).filter_map(|buffer| buffer.upgrade()).collect();
        for buffer in waiting {
            self.open_doc(language, buffer, window, cx);
        }
        cx.emit(HubEvent::StatusChanged(language.to_string()));
        cx.notify();
    }

    fn failed(&mut self, language: &str, message: String, not_installed: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(server) = self.servers.get_mut(language) {
            server.status = ServerStatus::Failed(message.clone());
            server.client = None;
        }
        if self.warned.insert(language.to_string()) {
            let text = if not_installed { format!("No language server is installed for {language}. {message}") } else { message };
            overlay::toast_warning(text, window, cx);
        }
        cx.emit(HubEvent::StatusChanged(language.to_string()));
        cx.notify();
    }

    fn handle(&mut self, language: &str, batch: Vec<Incoming>, window: &mut Window, cx: &mut Context<Self>) {
        let encoding = self.servers.get(language).map(Server::encoding).unwrap_or(PositionEncoding::Utf16);
        for message in batch {
            match message {
                Incoming::Notification { method, params } => match method.as_str() {
                    "textDocument/publishDiagnostics" => {
                        self.diagnostics.update(cx, |diagnostics, cx| diagnostics.publish(language, encoding, &params, cx));
                    }
                    "window/showMessage" => {
                        let text = params.get("message").and_then(Value::as_str).unwrap_or_default().to_string();
                        let kind = params.get("type").and_then(Value::as_u64).unwrap_or(3);
                        if !text.is_empty() {
                            if kind == 1 {
                                overlay::toast_error(text, window, cx);
                            } else if kind == 2 {
                                overlay::toast_warning(text, window, cx);
                            } else {
                                overlay::toast_info(text, window, cx);
                            }
                        }
                    }
                    _ => {}
                },
                Incoming::Exited { code, stderr_tail } => self.exited(language, code, stderr_tail, window, cx),
            }
        }
    }

    /// The server stopped on its own: its diagnostics are removed, then it is restarted with a delay (limited).
    fn exited(&mut self, language: &str, code: Option<i32>, stderr_tail: String, window: &mut Window, cx: &mut Context<Self>) {
        self.diagnostics.update(cx, |diagnostics, cx| diagnostics.clear(Some(language), cx));
        let Some(server) = self.servers.get_mut(language) else { return };
        if server.status == ServerStatus::Stopped {
            return;
        }
        let was_running = server.status == ServerStatus::Running;
        let reopen: Vec<String> = server.docs.keys().cloned().collect();
        server.client = None;
        server.docs.clear();
        server.restarts += 1;
        let restarts = server.restarts;
        if !was_running || restarts > MAX_RESTARTS {
            let detail = stderr_tail.lines().last().unwrap_or_default().to_string();
            let message = format!("The {language} language server stopped (exit code {}). {detail}", code.map_or("unknown".to_string(), |c| c.to_string()));
            self.failed(language, message, false, window, cx);
            return;
        }
        server.status = ServerStatus::Starting;
        let language = language.to_string();
        let delay = Duration::from_millis(500 * u64::from(restarts));
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.start(&language, window, cx);
                // The documents that were open are reopened as soon as the server is ready.
                let buffers = Workbench::for_window(window, cx).and_then(|w| oncode_ui::editor::buffers(w.read(cx)));
                if let (Some(buffers), Some(server)) = (buffers, this.servers.get_mut(&language)) {
                    for path in &reopen {
                        if let Some(buffer) = buffers.read(cx).get(path) {
                            server.waiting.push(buffer.downgrade());
                        }
                    }
                }
            });
        })
        .detach();
    }

    /// Stops and then restarts a language's server (the "Restart Language Server" command).
    pub fn restart(&mut self, language: &str, window: &mut Window, cx: &mut Context<Self>) {
        let reopen: Vec<String> = self.servers.get(language).map(|server| server.docs.keys().cloned().collect()).unwrap_or_default();
        self.stop(language, cx);
        self.servers.remove(language);
        self.warned.remove(language);
        self.start(language, window, cx);
        let buffers = Workbench::for_window(window, cx).and_then(|w| oncode_ui::editor::buffers(w.read(cx)));
        if let (Some(buffers), Some(server)) = (buffers, self.servers.get_mut(language)) {
            for path in &reopen {
                if let Some(buffer) = buffers.read(cx).get(path) {
                    server.waiting.push(buffer.downgrade());
                }
            }
        }
    }

    fn stop(&mut self, language: &str, cx: &mut Context<Self>) {
        let Some(server) = self.servers.get_mut(language) else { return };
        server.status = ServerStatus::Stopped;
        let (client, id) = (server.client.take(), server.id.take());
        let (core, label) = (self.env.core.clone(), self.env.window.to_string());
        spawn_core(cx, async move {
            if let Some(client) = client {
                client.shutdown(Duration::from_secs(2)).await;
            }
            if let Some(id) = id {
                let _ = lsp_api::lsp_stop(&core, &label, id).await;
            }
        })
        .detach();
        self.diagnostics.update(cx, |diagnostics, cx| diagnostics.clear(Some(language), cx));
    }

    /// Shows a file's diagnostics in all of its editors.
    fn show_diagnostics(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(states) = self.editors.get_mut(path) else { return };
        states.retain(|state| state.upgrade().is_some());
        let items: Vec<gpui_kit::component::highlighter::Diagnostic> = self
            .diagnostics
            .read(cx)
            .for_path(path)
            .map(|file| {
                file.raw
                    .iter()
                    .map(|item| {
                        let severity = match item.severity {
                            Some(lsp_types::DiagnosticSeverity::WARNING) => gpui_kit::component::highlighter::DiagnosticSeverity::Warning,
                            Some(lsp_types::DiagnosticSeverity::INFORMATION) => gpui_kit::component::highlighter::DiagnosticSeverity::Info,
                            Some(lsp_types::DiagnosticSeverity::HINT) => gpui_kit::component::highlighter::DiagnosticSeverity::Hint,
                            _ => gpui_kit::component::highlighter::DiagnosticSeverity::Error,
                        };
                        gpui_kit::component::highlighter::Diagnostic::new(item.range.start..item.range.end, item.message.clone()).with_severity(severity)
                    })
                    .collect()
            })
            .unwrap_or_default();
        for state in states.iter().filter_map(WeakEntity::upgrade) {
            let items = items.clone();
            state.update(cx, |state, cx| {
                // The diagnostics set keeps its own copy of the text to convert positions
                // into offsets; that copy is first synced with the editor's current content.
                let text = state.text().clone();
                let mut items = items;
                items.sort_by(|a, b| (a.range.start.line, a.range.start.character).cmp(&(b.range.start.line, b.range.start.character)));
                if let Some(set) = state.diagnostics_mut() {
                    set.reset(&text);
                    set.extend(items);
                }
                cx.notify();
            });
        }
    }

    /// The client and cursor position for a request on a document.
    pub(crate) fn request_context(&mut self, language: &str, path: &str, text: &Rope, offset: usize) -> Option<(Arc<Client>, Position)> {
        self.ensure_synced(language, path, text);
        let server = self.servers.get(language)?;
        let client = server.client()?;
        let position = position_at(&text.to_string(), offset, client.encoding());
        Some((client, position))
    }
}

/// A provider for a single file editor.
struct Provider {
    hub: WeakEntity<LspHub>,
    language: String,
    path: String,
    uri: lsp_types::Uri,
    buffer: WeakEntity<Buffer>,
}

impl Provider {
    fn context(&self, text: &Rope, offset: usize, feature: Feature, cx: &mut App) -> Option<(Arc<Client>, Position)> {
        let (client, position) = self.hub.update(cx, |hub, _| hub.request_context(&self.language, &self.path, text, offset)).ok().flatten()?;
        client.supports(feature).then_some((client, position))
    }

    fn position_params(&self, position: Position) -> TextDocumentPositionParams {
        TextDocumentPositionParams { text_document: TextDocumentIdentifier { uri: self.uri.clone() }, position }
    }
}

fn to_gpui<T>(result: Result<T, oncode_lsp::RpcError>) -> GpuiResult<T> {
    result.map_err(|error| anyhow::anyhow!(error.to_string()))
}

impl CompletionProvider for Provider {
    fn completions(&self, text: &Rope, offset: usize, trigger: CompletionContext, _: &mut Window, cx: &mut App) -> Task<GpuiResult<CompletionResponse>> {
        let Some((client, position)) = self.context(text, offset, Feature::Completion, cx) else { return Task::ready(Ok(CompletionResponse::Array(Vec::new()))) };
        let params = CompletionParams {
            text_document_position: self.position_params(position),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: Some(trigger),
        };
        let request = client.request::<Completion>(params);
        let task = spawn_core(cx, request);
        cx.background_spawn(async move { to_gpui(task.await).map(|response| response.unwrap_or(CompletionResponse::Array(Vec::new()))) })
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, cx: &mut App) -> bool {
        let Some(hub) = self.hub.upgrade() else { return false };
        let Some(client) = hub.read(cx).server(&self.language).and_then(Server::client) else { return false };
        let triggers = client.completion_trigger_characters();
        new_text.chars().last().is_some_and(|c| c.is_alphanumeric() || c == '_' || triggers.iter().any(|t| t.ends_with(c)))
    }
}

impl HoverProvider for Provider {
    fn hover(&self, text: &Rope, offset: usize, _: &mut Window, cx: &mut App) -> Task<GpuiResult<Option<Hover>>> {
        let Some((client, position)) = self.context(text, offset, Feature::Hover, cx) else { return Task::ready(Ok(None)) };
        let params = HoverParams { text_document_position_params: self.position_params(position), work_done_progress_params: WorkDoneProgressParams::default() };
        let task = spawn_core(cx, client.request::<HoverRequest>(params));
        cx.background_spawn(async move { to_gpui(task.await) })
    }
}

impl DefinitionProvider for Provider {
    fn definitions(&self, text: &Rope, offset: usize, window: &mut Window, cx: &mut App) -> Task<GpuiResult<Vec<LocationLink>>> {
        // Without a running server, or if the server finds nothing: search the project.
        let env = oncode_ui::Workbench::for_window(window, cx).map(|workbench| workbench.read(cx).env().clone());
        let Some((client, position)) = self.context(text, offset, Feature::Definition, cx) else {
            return match env {
                Some(env) => oncode_ui::editor::symbol_definition_links(&env, &self.path, text, offset, cx),
                None => Task::ready(Ok(Vec::new())),
            };
        };
        let params = GotoDefinitionParams {
            text_document_position_params: self.position_params(position),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };
        let task = spawn_core(cx, client.request::<GotoDefinition>(params));
        let (path, text) = (self.path.clone(), text.clone());
        cx.spawn(async move |cx| {
            let links = to_gpui(task.await).map(definition_links).unwrap_or_default();
            match env {
                Some(env) if links.is_empty() => cx.update(|cx| oncode_ui::editor::symbol_definition_links(&env, &path, &text, offset, cx)).await,
                _ => Ok(links),
            }
        })
    }
}

/// Every form of `textDocument/definition` answer is turned into a `LocationLink`.
pub fn definition_links(response: Option<GotoDefinitionResponse>) -> Vec<LocationLink> {
    match response {
        None => Vec::new(),
        Some(GotoDefinitionResponse::Scalar(location)) => vec![link_of(location)],
        Some(GotoDefinitionResponse::Array(locations)) => locations.into_iter().map(link_of).collect(),
        Some(GotoDefinitionResponse::Link(links)) => links,
    }
}

fn link_of(location: lsp_types::Location) -> LocationLink {
    LocationLink { origin_selection_range: None, target_uri: location.uri, target_range: location.range, target_selection_range: location.range }
}

impl CodeActionProvider for Provider {
    fn id(&self) -> SharedString {
        format!("lsp:{}", self.language).into()
    }

    fn code_actions(&self, state: Entity<EditorState>, range: Range<usize>, _: &mut Window, cx: &mut App) -> Task<GpuiResult<Vec<CodeAction>>> {
        let text = state.read(cx).text().clone();
        let Some((client, start)) = self.context(&text, range.start, Feature::CodeAction, cx) else { return Task::ready(Ok(Vec::new())) };
        let end = position_at(&text.to_string(), range.end, client.encoding());
        let diagnostics = self
            .hub
            .upgrade()
            .and_then(|hub| hub.read(cx).diagnostics.read(cx).for_path(&self.path).map(|file| file.raw.clone()))
            .unwrap_or_default()
            .into_iter()
            .filter(|item| item.range.start <= end && item.range.end >= start)
            .collect();
        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri: self.uri.clone() },
            range: lsp_types::Range { start, end },
            context: CodeActionContext { diagnostics, only: None, trigger_kind: None },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };
        let task = spawn_core(cx, client.request::<CodeActionRequest>(params));
        cx.background_spawn(async move {
            let actions = to_gpui(task.await)?.unwrap_or_default();
            Ok(actions
                .into_iter()
                .filter_map(|action| match action {
                    CodeActionOrCommand::CodeAction(action) => Some(action),
                    CodeActionOrCommand::Command(_) => None,
                })
                .collect())
        })
    }

    fn perform_code_action(&self, state: Entity<EditorState>, action: CodeAction, _push_to_history: bool, window: &mut Window, cx: &mut App) -> Task<GpuiResult<()>> {
        let Some(edit) = action.edit else { return Task::ready(Ok(())) };
        let _ = state;
        let summary = crate::edits::apply_workspace_edit(self.hub.clone(), edit, window, cx);
        cx.background_spawn(async move {
            summary.await;
            Ok(())
        })
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        let _ = (&self.buffer, &self.path);
    }
}

/// The server's position encoding for a file (UTF-16 if there is no server).
pub fn encoding_for(hub: &LspHub, path: &str) -> PositionEncoding {
    hub.server_for_path(path).map(Server::encoding).unwrap_or(PositionEncoding::Utf16)
}
