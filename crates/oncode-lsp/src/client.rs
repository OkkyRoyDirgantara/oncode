//! Klien JSON-RPC/LSP di atas transport pesan mentah (teks JSON tanpa bingkai).
//!
//! Klien tidak tahu cara pesan dikirim: pesan keluar ditaruh di kanal yang dikembalikan
//! [`Client::new`] (urutannya = urutan pemanggilan), dan pesan masuk diserahkan lewat
//! [`Client::receive`]. Dengan begitu klien yang sama dipakai di atas [`LspManager`](crate::LspManager),
//! di atas API `oncode-core`, dan di atas server tiruan di dalam proses.
//!
//! Permintaan dikirim **saat fungsi dipanggil**, bukan saat future-nya pertama kali di-poll,
//! sehingga `did_change` yang dipanggil lebih dulu selalu tiba lebih dulu di server. Future
//! hasilnya memakai pewaktu tokio dan harus ditunggu di dalam runtime tokio. Membuang future
//! sebelum selesai membatalkan permintaannya (`$/cancelRequest`).

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

/// Kode galat JSON-RPC yang dipakai klien saat menjawab server.
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
    /// Galat yang dikirim server (`error` pada jawaban JSON-RPC).
    #[error("{message}")]
    Server { code: i64, message: String },
    #[error("Unexpected answer from the language server: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Nama dan versi klien untuk `clientInfo`.
    pub name: String,
    pub version: Option<String>,
    /// Path absolut folder proyek; menjadi `rootUri` dan satu-satunya `workspaceFolders`.
    pub root: Option<String>,
    /// Batas waktu tiap permintaan.
    pub request_timeout: Duration,
    /// Batas waktu `initialize` (server besar perlu waktu untuk memuat proyek).
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

/// Penerima pesan dari server yang bukan jawaban permintaan. Dipanggil dari thread transport,
/// jadi harus cepat dan tidak memblokir (biasanya hanya meneruskan ke kanal milik UI).
pub trait ClientHandler: Send + Sync + 'static {
    /// Notifikasi dari server, mis. `textDocument/publishDiagnostics` atau `window/showMessage`.
    fn notification(&self, method: &str, params: Value);

    /// `workspace/applyEdit`. Jawaban dikirim lewat `respond`; bila `respond` dibuang tanpa
    /// dijawab, server menerima `applied: false`.
    fn apply_edit(&self, params: ApplyWorkspaceEditParams, respond: Responder<ApplyWorkspaceEditResponse>) {
        let _ = (params, respond);
    }

    /// `window/showDocument`. Tanpa jawaban, server menerima `success: false`.
    fn show_document(&self, params: ShowDocumentParams, respond: Responder<ShowDocumentResult>) {
        let _ = (params, respond);
    }

    /// `workspace/configuration`: satu nilai per butir; `null` berarti "tidak ada pengaturan".
    fn configuration(&self, items: &[ConfigurationItem]) -> Vec<Value> {
        vec![Value::Null; items.len()]
    }
}

/// Jawaban tertunda untuk permintaan dari server. Selalu menjawab: bila dibuang tanpa
/// [`Responder::respond`], nilai cadangan yang dikirim agar server tidak menunggu selamanya.
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
    /// Dokumen yang sedang terbuka di server, menurut teks URI-nya.
    documents: HashMap<String, Document>,
    /// Versi terakhir tiap URI; tidak diulang walau dokumen ditutup lalu dibuka lagi.
    versions: HashMap<String, i32>,
    /// Pendaftaran dinamis (`client/registerCapability`): id → metode.
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

/// Membatalkan permintaan bila future-nya dibuang sebelum jawaban tiba.
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
        // `initialize` tidak boleh dibatalkan menurut spesifikasi.
        if was_pending && self.method != lsp_types::request::Initialize::METHOD {
            client.send(json!({ "jsonrpc": "2.0", "method": "$/cancelRequest", "params": { "id": self.id } }));
        }
    }
}

/// Klien satu language server. Murah di-clone; semua clone berbagi keadaan yang sama.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

impl Client {
    /// Membuat klien dan kanal pesan keluarnya. Pemanggil menguras kanal itu secara berurutan
    /// ke transport, dan menyerahkan setiap pesan dari server ke [`Client::receive`].
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

    // ----- Keadaan -----

    /// `initialize` sudah dijawab dan klien belum ditutup.
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

    /// Satuan kolom yang disepakati dengan server.
    pub fn encoding(&self) -> PositionEncoding {
        lock(&self.inner.state).encoding
    }

    pub fn sync_options(&self) -> SyncOptions {
        lock(&self.inner.state).sync
    }

    /// Server mendukung `feature`, lewat kemampuan statis atau pendaftaran dinamis.
    pub fn supports(&self, feature: Feature) -> bool {
        let state = lock(&self.inner.state);
        state.phase == Phase::Ready && (server_supports(&state.capabilities, feature) || state.registrations.values().any(|method| method == feature.method()))
    }

    /// Karakter yang memicu completion otomatis (mis. `.`).
    pub fn completion_trigger_characters(&self) -> Vec<String> {
        lock(&self.inner.state).capabilities.completion_provider.as_ref().and_then(|options| options.trigger_characters.clone()).unwrap_or_default()
    }

    /// Karakter yang memicu dan memicu ulang signature help (mis. `(` dan `,`).
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

    // ----- Pesan masuk -----

    /// Memproses satu pesan dari server (teks JSON tanpa header). Pesan yang rusak diabaikan.
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
        // Jawaban untuk permintaan yang sudah kedaluwarsa atau dibatalkan tidak punya entri lagi.
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
            // Tidak ada yang perlu dilakukan selain mengakui: progres ditampilkan (bila perlu) dari
            // notifikasi `$/progress`, dan permintaan penyegaran dijawab pada permintaan berikutnya.
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

    /// Transport terputus atau server berhenti: semua permintaan yang menunggu gagal dengan
    /// [`RpcError::Closed`], dan tidak ada lagi pesan yang dikirim atau diterima.
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

    // ----- Permintaan dan notifikasi -----

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
                // Pengirim hilang tanpa jawaban: klien ditutup.
                Ok(Err(_)) => {
                    guard.armed = false;
                    Err(RpcError::Closed)
                }
                // `guard` dibuang di sini dan mengirim `$/cancelRequest`.
                Err(_) => Err(RpcError::Timeout { method: R::METHOD.to_string() }),
            }
        }
    }

    /// Mengirim permintaan bertipe dan mengembalikan future jawabannya (lihat catatan modul).
    pub fn request<R>(&self, params: R::Params) -> impl Future<Output = Result<R::Result, RpcError>> + Send + 'static
    where
        R: Request,
        R::Params: Serialize,
        R::Result: DeserializeOwned + Send + 'static,
    {
        self.request_inner::<R>(params, self.inner.config.request_timeout, true)
    }

    /// Seperti [`Client::request`] dengan batas waktu sendiri.
    pub fn request_with_timeout<R>(&self, params: R::Params, timeout: Duration) -> impl Future<Output = Result<R::Result, RpcError>> + Send + 'static
    where
        R: Request,
        R::Params: Serialize,
        R::Result: DeserializeOwned + Send + 'static,
    {
        self.request_inner::<R>(params, timeout, true)
    }

    /// Mengirim notifikasi. Hasil `false` bila klien sudah ditutup.
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

    // ----- Siklus hidup -----

    fn workspace_folders(&self) -> Option<Vec<WorkspaceFolder>> {
        let root = self.inner.config.root.as_ref()?;
        let uri = path_to_lsp_uri(root)?;
        let name = root.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(root).to_string();
        Some(vec![WorkspaceFolder { uri, name }])
    }

    /// Jabat tangan: `initialize`, menyimpan kemampuan server, lalu `initialized`.
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

    /// Penutupan yang rapi: `shutdown`, lalu `exit`. Galat `shutdown` (mis. batas waktu) tidak
    /// menghalangi `exit`. Setelah ini klien tertutup.
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

    // ----- Sinkronisasi dokumen -----

    fn document_ready(&self) -> Result<(), RpcError> {
        if self.is_closed() {
            return Err(RpcError::Closed);
        }
        if lock(&self.inner.state).phase != Phase::Ready {
            return Err(RpcError::NotReady);
        }
        Ok(())
    }

    /// Dokumen ini sedang terbuka di server.
    pub fn is_open(&self, uri: &Uri) -> bool {
        lock(&self.inner.state).documents.contains_key(uri.as_str())
    }

    /// Versi dokumen yang terakhir dikirim ke server; `None` bila tidak terbuka.
    pub fn version(&self, uri: &Uri) -> Option<i32> {
        lock(&self.inner.state).documents.get(uri.as_str()).map(|document| document.version)
    }

    /// `textDocument/didOpen`. Mengembalikan versi dokumen; dokumen yang sudah terbuka tidak
    /// dibuka lagi (versinya yang sekarang dikembalikan).
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

    /// `textDocument/didChange`. `changes` adalah perubahan bertahap (tiap butir mengacu ke
    /// dokumen setelah butir sebelumnya, posisinya dalam [`Client::encoding`]); `full_text` hanya
    /// dipanggil bila server meminta sinkronisasi penuh. Mengembalikan versi baru, atau `None`
    /// bila server tidak menginginkan perubahan atau tidak ada yang berubah.
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

    /// `textDocument/didSave`; `text` hanya dipanggil bila server meminta isinya disertakan.
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

    /// `textDocument/didClose`. Dokumen yang tidak terbuka diabaikan.
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

/// Metode permintaan yang sedang menunggu jawaban, terurut menurut id (untuk diagnosis dan uji).
impl Client {
    pub fn pending_methods(&self) -> Vec<&'static str> {
        let pending = lock(&self.inner.pending);
        let mut entries: Vec<(i64, &'static str)> = pending.iter().map(|(id, pending)| (*id, pending.method)).collect();
        entries.sort_unstable();
        entries.into_iter().map(|(_, method)| method).collect()
    }
}
