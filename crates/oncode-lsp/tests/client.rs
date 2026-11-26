//! LSP client tests against the in-process mock server (via channels, without a child process).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use lsp_types::request::Request;
use lsp_types::{
    ApplyWorkspaceEditParams, ApplyWorkspaceEditResponse, CompletionParams, CompletionResponse, ConfigurationItem, Position, Range, TextDocumentContentChangeEvent,
    TextDocumentIdentifier, TextDocumentPositionParams, Uri,
};
use oncode_lsp::mock::{MockOptions, MockServer};
use oncode_lsp::{Client, ClientConfig, ClientHandler, Feature, PositionEncoding, Responder, RpcError};
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedReceiver;

/// A test request that the mock server never answers.
enum Hang {}
impl Request for Hang {
    type Params = Value;
    type Result = Value;
    const METHOD: &'static str = "mock/hang";
}

enum MockDocument {}
impl Request for MockDocument {
    type Params = Value;
    type Result = Value;
    const METHOD: &'static str = "mock/document";
}

enum MockState {}
impl Request for MockState {
    type Params = Value;
    type Result = Value;
    const METHOD: &'static str = "mock/state";
}

#[derive(Default)]
struct Recorder {
    notifications: Mutex<Vec<(String, Value)>>,
    /// `Some(applied)`: answer `workspace/applyEdit` with that value; `None`: drop without answering.
    apply: Mutex<Option<bool>>,
    edits: Mutex<Vec<ApplyWorkspaceEditParams>>,
}

impl ClientHandler for Recorder {
    fn notification(&self, method: &str, params: Value) {
        self.notifications.lock().unwrap().push((method.to_string(), params));
    }

    fn apply_edit(&self, params: ApplyWorkspaceEditParams, respond: Responder<ApplyWorkspaceEditResponse>) {
        self.edits.lock().unwrap().push(params);
        if let Some(applied) = *self.apply.lock().unwrap() {
            respond.respond(ApplyWorkspaceEditResponse { applied, failure_reason: None, failed_change: None });
        }
    }

    fn configuration(&self, items: &[ConfigurationItem]) -> Vec<Value> {
        items.iter().map(|item| if item.section.as_deref() == Some("mock") { json!({ "enabled": true }) } else { Value::Null }).collect()
    }
}

struct Harness {
    client: Client,
    handler: Arc<Recorder>,
    server: Arc<Mutex<MockServer>>,
    /// All messages the client sent, in order.
    sent: Arc<Mutex<Vec<Value>>>,
}

impl Harness {
    fn sent_methods(&self) -> Vec<String> {
        self.sent.lock().unwrap().iter().filter_map(|message| message["method"].as_str().map(String::from)).collect()
    }

    fn last_sent(&self, method: &str) -> Value {
        self.sent.lock().unwrap().iter().rev().find(|message| message["method"] == method).cloned().unwrap_or_else(|| panic!("no {method} message"))
    }

    fn notifications(&self, method: &str) -> Vec<Value> {
        self.handler.notifications.lock().unwrap().iter().filter(|(name, _)| name == method).map(|(_, params)| params.clone()).collect()
    }

    /// Yields to the pump task until all messages that have been sent are processed.
    async fn settle(&self) {
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }

    async fn server_text(&self, uri: &Uri) -> Value {
        self.settle().await;
        self.client.request::<MockDocument>(json!({ "uri": uri.as_str() })).await.unwrap()
    }
}

fn config(root: Option<&str>) -> ClientConfig {
    ClientConfig { root: root.map(String::from), version: Some("1.2.3".into()), ..Default::default() }
}

/// Connects the client to the mock server: each outgoing message is processed by the server and its replies
/// are handed straight to the client, in order.
fn connect(options: MockOptions, root: Option<&str>) -> Harness {
    let handler = Arc::new(Recorder::default());
    let (client, mut outgoing) = Client::new(config(root), handler.clone());
    let server = Arc::new(Mutex::new(MockServer::new(options)));
    let sent = Arc::new(Mutex::new(Vec::new()));
    let (pump_client, pump_server, pump_sent) = (client.clone(), server.clone(), sent.clone());
    tokio::spawn(async move {
        while let Some(message) = outgoing.recv().await {
            pump_sent.lock().unwrap().push(serde_json::from_str::<Value>(&message).expect("the client sends valid JSON"));
            let replies = pump_server.lock().unwrap().handle(&message);
            for reply in replies {
                pump_client.receive(&reply);
            }
        }
    });
    Harness { client, handler, server, sent }
}

async fn ready(options: MockOptions) -> Harness {
    let harness = connect(options, None);
    harness.client.initialize().await.expect("initialize");
    harness.settle().await;
    harness
}

fn uri(path: &str) -> Uri {
    oncode_lsp::uri::path_to_lsp_uri(path).unwrap()
}

fn pos(line: u32, character: u32) -> Position {
    Position { line, character }
}

fn change(start: (u32, u32), end: (u32, u32), text: &str) -> TextDocumentContentChangeEvent {
    TextDocumentContentChangeEvent { range: Some(Range { start: pos(start.0, start.1), end: pos(end.0, end.1) }), range_length: None, text: text.into() }
}

fn next(outgoing: &mut UnboundedReceiver<String>) -> Value {
    serde_json::from_str(&outgoing.try_recv().expect("the client has not sent a message yet")).unwrap()
}

#[tokio::test]
async fn handshake_sends_initialize_then_initialized_and_stores_capabilities() {
    let harness = connect(MockOptions::default(), Some("/project/example app"));
    assert!(!harness.client.is_ready());
    assert!(!harness.client.supports(Feature::Hover), "no feature is considered present before initialize");
    // Ordinary requests are rejected before the handshake finishes.
    assert_eq!(harness.client.request::<Hang>(Value::Null).await.unwrap_err(), RpcError::NotReady);
    assert_eq!(harness.client.did_open(&uri("/project/a.txt"), "plaintext", String::new()).unwrap_err(), RpcError::NotReady);

    let result = harness.client.initialize().await.unwrap();
    assert_eq!(result.server_info.unwrap().name, "mock-lsp");
    assert!(harness.client.is_ready());
    assert_eq!(harness.client.encoding(), PositionEncoding::Utf16);
    assert_eq!(harness.client.server_info().unwrap().version.as_deref(), Some("1.0"));
    harness.settle().await;
    assert_eq!(&harness.sent_methods()[..2], ["initialize", "initialized"]);

    let initialize = harness.sent.lock().unwrap()[0].clone();
    assert_eq!(initialize["jsonrpc"], "2.0");
    assert_eq!(initialize["params"]["rootUri"], "file:///project/example%20app");
    assert_eq!(initialize["params"]["workspaceFolders"][0]["name"], "example app");
    assert_eq!(initialize["params"]["clientInfo"], json!({ "name": "OnCode", "version": "1.2.3" }));
    assert_eq!(initialize["params"]["processId"], std::process::id());
    assert_eq!(initialize["params"]["capabilities"]["textDocument"]["completion"]["completionItem"]["snippetSupport"], false);
    assert_eq!(initialize["params"]["capabilities"]["workspace"]["applyEdit"], true);

    // The handshake happens only once.
    assert!(matches!(harness.client.initialize().await, Err(RpcError::Invalid(_))));
}

#[tokio::test]
async fn capabilities_drive_feature_checks_and_trigger_characters() {
    let harness = ready(MockOptions::default()).await;
    for feature in [Feature::Completion, Feature::CompletionResolve, Feature::Hover, Feature::SignatureHelp, Feature::Definition, Feature::References, Feature::Rename, Feature::PrepareRename, Feature::Formatting, Feature::CodeAction] {
        assert!(harness.client.supports(feature), "{feature:?}");
    }
    assert!(!harness.client.supports(Feature::CodeActionResolve));
    assert_eq!(harness.client.completion_trigger_characters(), ["."]);
    assert_eq!(harness.client.signature_trigger_characters(), ["(", ","]);
    assert!(harness.client.capabilities().hover_provider.is_some());
}

#[tokio::test]
async fn the_server_may_choose_utf8_positions() {
    let harness = ready(MockOptions { utf8: true, ..Default::default() }).await;
    assert_eq!(harness.client.encoding(), PositionEncoding::Utf8);
}

#[tokio::test]
async fn responses_are_matched_to_requests_by_id() {
    let (client, mut outgoing) = Client::new(config(None), Arc::new(Recorder::default()));
    let initialize = client.initialize();
    let request = next(&mut outgoing);
    assert_eq!(request["method"], "initialize");
    client.receive(&json!({ "jsonrpc": "2.0", "id": request["id"], "result": { "capabilities": {} } }).to_string());
    initialize.await.unwrap();
    assert_eq!(next(&mut outgoing)["method"], "initialized");

    let first = client.request::<Hang>(json!("one"));
    let second = client.request::<Hang>(json!("two"));
    let (a, b) = (next(&mut outgoing), next(&mut outgoing));
    assert_eq!((a["params"].clone(), b["params"].clone()), (json!("one"), json!("two")));
    assert_ne!(a["id"], b["id"]);
    assert_eq!(client.pending_methods(), ["mock/hang", "mock/hang"]);

    // Answered in reverse order; broken messages and unknown ids are ignored.
    client.receive("not json");
    client.receive(&json!({ "jsonrpc": "2.0", "id": 9999, "result": "stray" }).to_string());
    client.receive(&json!({ "jsonrpc": "2.0", "id": b["id"], "result": "answer two" }).to_string());
    client.receive(&json!({ "jsonrpc": "2.0", "id": a["id"], "result": "answer one" }).to_string());
    assert_eq!(first.await.unwrap(), json!("answer one"));
    assert_eq!(second.await.unwrap(), json!("answer two"));
    assert!(client.pending_methods().is_empty());
}

#[tokio::test]
async fn server_errors_and_malformed_results_are_reported() {
    let harness = ready(MockOptions::default()).await;
    enum Unknown {}
    impl Request for Unknown {
        type Params = Value;
        type Result = Value;
        const METHOD: &'static str = "mock/unknown";
    }
    let error = harness.client.request::<Unknown>(Value::Null).await.unwrap_err();
    assert_eq!(error, RpcError::Server { code: -32601, message: "Method not found: mock/unknown".into() });

    // A result that does not match the requested type.
    enum WrongShape {}
    impl Request for WrongShape {
        type Params = Value;
        type Result = Vec<u32>;
        const METHOD: &'static str = "mock/state";
    }
    assert!(matches!(harness.client.request::<WrongShape>(Value::Null).await, Err(RpcError::Invalid(_))));
}

#[tokio::test(start_paused = true)]
async fn requests_time_out_and_are_cancelled_on_the_server() {
    let harness = ready(MockOptions::default()).await;
    let started = tokio::time::Instant::now();
    let error = harness.client.request_with_timeout::<Hang>(Value::Null, Duration::from_secs(3)).await.unwrap_err();
    assert_eq!(error, RpcError::Timeout { method: "mock/hang".into() });
    assert_eq!(started.elapsed(), Duration::from_secs(3));
    assert!(harness.client.pending_methods().is_empty());

    let hang_id = harness.last_sent("mock/hang")["id"].clone();
    harness.settle().await;
    assert_eq!(harness.last_sent("$/cancelRequest")["params"]["id"], hang_id);
    // A late answer does not disturb other requests.
    harness.client.receive(&json!({ "jsonrpc": "2.0", "id": hang_id, "result": "late" }).to_string());
    let state = harness.client.request::<MockState>(Value::Null).await.unwrap();
    assert_eq!(state["cancelled"], json!([hang_id]));
}

#[tokio::test]
async fn dropping_a_request_cancels_it() {
    let harness = ready(MockOptions::default()).await;
    let request = harness.client.request::<Hang>(Value::Null);
    assert_eq!(harness.client.pending_methods(), ["mock/hang"]);
    drop(request);
    assert!(harness.client.pending_methods().is_empty());
    harness.settle().await;
    let state = harness.client.request::<MockState>(Value::Null).await.unwrap();
    assert_eq!(state["cancelled"].as_array().unwrap().len(), 1);
    // A request that finished (`mock/state` above) is not canceled.
    assert_eq!(harness.sent_methods().iter().filter(|method| *method == "$/cancelRequest").count(), 1);
}

#[tokio::test]
async fn requests_from_the_server_are_answered() {
    let harness = ready(MockOptions::default()).await;
    // After `initialized`, the mock server asks for progress, configuration, and capability registration.
    let state = harness.client.request::<MockState>(Value::Null).await.unwrap();
    assert_eq!(
        state["answers"],
        json!([
            { "method": "window/workDoneProgress/create", "result": null },
            { "method": "workspace/configuration", "result": [{ "enabled": true }, null] },
            { "method": "client/registerCapability", "result": null },
        ])
    );

    // An unknown request is answered with an error, not ignored.
    let (client, mut outgoing) = Client::new(config(None), Arc::new(Recorder::default()));
    client.receive(&json!({ "jsonrpc": "2.0", "id": "abc", "method": "odd/unknownMethod", "params": {} }).to_string());
    let reply = next(&mut outgoing);
    assert_eq!((reply["id"].clone(), reply["error"]["code"].clone()), (json!("abc"), json!(-32601)));
    client.receive(&json!({ "jsonrpc": "2.0", "id": 5, "method": "workspace/configuration", "params": { "items": "wrong" } }).to_string());
    assert_eq!(next(&mut outgoing)["error"]["code"], -32602);
    client.receive(&json!({ "jsonrpc": "2.0", "id": 6, "method": "workspace/workspaceFolders" }).to_string());
    assert_eq!(next(&mut outgoing), json!({ "jsonrpc": "2.0", "id": 6, "result": null }));
}

#[tokio::test]
async fn dynamic_registrations_extend_the_supported_features() {
    let (client, mut outgoing) = Client::new(config(None), Arc::new(Recorder::default()));
    let initialize = client.initialize();
    let request = next(&mut outgoing);
    client.receive(&json!({ "jsonrpc": "2.0", "id": request["id"], "result": { "capabilities": {} } }).to_string());
    initialize.await.unwrap();
    assert!(!client.supports(Feature::Formatting));

    client.receive(&json!({ "jsonrpc": "2.0", "id": 1, "method": "client/registerCapability", "params": { "registrations": [{ "id": "fmt", "method": "textDocument/formatting" }] } }).to_string());
    assert!(client.supports(Feature::Formatting));
    client.receive(&json!({ "jsonrpc": "2.0", "id": 2, "method": "client/unregisterCapability", "params": { "unregisterations": [{ "id": "fmt", "method": "textDocument/formatting" }] } }).to_string());
    assert!(!client.supports(Feature::Formatting));
}

#[tokio::test]
async fn apply_edit_goes_through_the_callback_and_always_gets_an_answer() {
    let harness = ready(MockOptions::default()).await;
    let edit = json!({ "label": "Test", "edit": { "changes": { "file:///project/a.txt": [{ "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } }, "newText": "x" }] } } });

    *harness.handler.apply.lock().unwrap() = Some(true);
    harness.client.notify::<MockApplyEdit>(edit.clone());
    harness.settle().await;
    // The handler does not answer: the client sends the fallback answer.
    *harness.handler.apply.lock().unwrap() = None;
    harness.client.notify::<MockApplyEdit>(edit);
    harness.settle().await;

    let state = harness.client.request::<MockState>(Value::Null).await.unwrap();
    let answers: Vec<&Value> = state["answers"].as_array().unwrap().iter().filter(|answer| answer["method"] == "workspace/applyEdit").collect();
    assert_eq!(answers.len(), 2);
    assert_eq!(answers[0]["result"]["applied"], true);
    assert_eq!(answers[1]["result"]["applied"], false);
    assert!(answers[1]["result"]["failureReason"].is_string());
    let received = harness.handler.edits.lock().unwrap();
    assert_eq!(received.len(), 2);
    assert_eq!(received[0].label.as_deref(), Some("Test"));
}

enum MockApplyEdit {}
impl lsp_types::notification::Notification for MockApplyEdit {
    type Params = Value;
    const METHOD: &'static str = "mock/applyEdit";
}

#[tokio::test]
async fn documents_are_synced_incrementally_with_utf16_positions_and_versions() {
    let harness = ready(MockOptions::default()).await;
    let file = uri("/project/src/a b.txt");
    assert_eq!(file.as_str(), "file:///project/src/a%20b.txt");
    assert_eq!(harness.client.version(&file), None);

    assert_eq!(harness.client.did_open(&file, "plaintext", "let 😀 = ERROR;\nline two\n".into()).unwrap(), 1);
    assert!(harness.client.is_open(&file));
    // Opening again sends nothing.
    assert_eq!(harness.client.did_open(&file, "plaintext", "other".into()).unwrap(), 1);
    harness.settle().await;
    let open = harness.last_sent("textDocument/didOpen");
    assert_eq!(open["params"]["textDocument"], json!({ "uri": file.as_str(), "languageId": "plaintext", "version": 1, "text": "let 😀 = ERROR;\nline two\n" }));
    assert_eq!(harness.sent_methods().iter().filter(|method| *method == "textDocument/didOpen").count(), 1);

    // Diagnostics for version 1; the emoji counts as two UTF-16 units.
    let published = harness.notifications("textDocument/publishDiagnostics");
    assert_eq!(published.last().unwrap()["version"], 1);
    assert_eq!(published.last().unwrap()["diagnostics"][0]["range"], json!({ "start": { "line": 0, "character": 9 }, "end": { "line": 0, "character": 14 } }));

    // Two changes in one send: the second refers to the document after the first.
    let version = harness.client.did_change(&file, vec![change((0, 9), (0, 14), "1"), change((1, 0), (1, 4), "LINE")], || unreachable!("the server asked for incremental changes")).unwrap();
    assert_eq!(version, Some(2));
    harness.settle().await;
    let sent = harness.last_sent("textDocument/didChange");
    assert_eq!(sent["params"]["textDocument"], json!({ "uri": file.as_str(), "version": 2 }));
    assert_eq!(sent["params"]["contentChanges"].as_array().unwrap().len(), 2);
    assert_eq!(harness.server_text(&file).await, json!({ "text": "let 😀 = 1;\nLINE two\n", "version": 2, "languageId": "plaintext" }));
    assert!(harness.notifications("textDocument/publishDiagnostics").last().unwrap()["diagnostics"].as_array().unwrap().is_empty());

    assert_eq!(harness.client.did_change(&file, Vec::new(), String::new).unwrap(), None, "no changes means nothing is sent");
    assert_eq!(harness.client.version(&file), Some(2));

    harness.client.did_save(&file, || unreachable!("the server does not ask for contents on save")).unwrap();
    harness.settle().await;
    assert_eq!(harness.last_sent("textDocument/didSave")["params"], json!({ "textDocument": { "uri": file.as_str() } }));

    harness.client.did_close(&file).unwrap();
    assert!(!harness.client.is_open(&file));
    assert_eq!(harness.server_text(&file).await, Value::Null);
    harness.client.did_close(&file).unwrap();
    harness.settle().await;
    assert_eq!(harness.sent_methods().iter().filter(|method| *method == "textDocument/didClose").count(), 1);
    assert!(matches!(harness.client.did_change(&file, vec![change((0, 0), (0, 0), "x")], String::new), Err(RpcError::Invalid(_))));

    // The version is not repeated after the document is reopened.
    assert_eq!(harness.client.did_open(&file, "plaintext", "new".into()).unwrap(), 3);
}

#[tokio::test]
async fn full_sync_servers_receive_the_whole_document() {
    let harness = ready(MockOptions { full_sync: true, ..Default::default() }).await;
    let file = uri("/project/a.txt");
    harness.client.did_open(&file, "plaintext", "one".into()).unwrap();
    harness.client.did_change(&file, vec![change((0, 3), (0, 3), " two")], || "one two".to_string()).unwrap();
    harness.settle().await;
    let sent = harness.last_sent("textDocument/didChange");
    assert_eq!(sent["params"]["contentChanges"], json!([{ "text": "one two" }]));
    assert_eq!(harness.server_text(&file).await["text"], "one two");
}

#[tokio::test]
async fn typed_requests_round_trip() {
    let harness = ready(MockOptions::default()).await;
    let file = uri("/project/a.txt");
    harness.client.did_open(&file, "plaintext", "def target\ndef task\nta".into()).unwrap();
    let params = CompletionParams {
        text_document_position: TextDocumentPositionParams { text_document: TextDocumentIdentifier { uri: file.clone() }, position: pos(2, 2) },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
        context: None,
    };
    let Some(CompletionResponse::List(list)) = harness.client.request::<lsp_types::request::Completion>(params).await.unwrap() else { panic!("daftar completion") };
    let labels: Vec<&str> = list.items.iter().map(|item| item.label.as_str()).collect();
    assert_eq!(labels, ["target", "task"]);

    let resolved = harness.client.request::<lsp_types::request::ResolveCompletionItem>(list.items[0].clone()).await.unwrap();
    assert!(matches!(resolved.documentation, Some(lsp_types::Documentation::MarkupContent(content)) if content.value.contains("target")));
}

#[tokio::test]
async fn closing_fails_pending_requests_and_stops_all_traffic() {
    let harness = ready(MockOptions::default()).await;
    let pending = harness.client.request::<Hang>(Value::Null);
    harness.settle().await;
    let before = harness.sent.lock().unwrap().len();
    harness.client.close();
    assert_eq!(pending.await.unwrap_err(), RpcError::Closed);
    assert!(harness.client.is_closed() && !harness.client.is_ready());
    assert_eq!(harness.client.request::<Hang>(Value::Null).await.unwrap_err(), RpcError::Closed);
    assert_eq!(harness.client.did_open(&uri("/a.txt"), "plaintext", String::new()).unwrap_err(), RpcError::Closed);
    assert!(!harness.client.notify::<MockApplyEdit>(Value::Null));
    harness.client.receive(&json!({ "jsonrpc": "2.0", "method": "window/logMessage", "params": { "type": 3, "message": "late" } }).to_string());
    assert!(harness.notifications("window/logMessage").is_empty());
    harness.settle().await;
    assert_eq!(harness.sent.lock().unwrap().len(), before, "no messages (including cancellation) after closing");
}

#[tokio::test]
async fn shutdown_sends_shutdown_then_exit() {
    let harness = ready(MockOptions::default()).await;
    harness.client.shutdown(Duration::from_secs(2)).await;
    assert!(harness.client.is_closed());
    for _ in 0..50 {
        if harness.server.lock().unwrap().exit_code().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(harness.server.lock().unwrap().exit_code(), Some(0));
    let methods = harness.sent_methods();
    assert_eq!(&methods[methods.len() - 2..], ["shutdown", "exit"]);
    assert!(harness.last_sent("shutdown").get("params").is_none(), "a request without parameters carries no `params`");
}

#[tokio::test(start_paused = true)]
async fn initialize_that_is_never_answered_times_out() {
    let handler = Arc::new(Recorder::default());
    let (client, mut outgoing) = Client::new(ClientConfig { initialize_timeout: Duration::from_secs(5), ..Default::default() }, handler);
    let error = client.initialize().await.unwrap_err();
    assert_eq!(error, RpcError::Timeout { method: "initialize".into() });
    assert!(!client.is_ready());
    assert_eq!(next(&mut outgoing)["method"], "initialize");
    assert!(outgoing.try_recv().is_err(), "`initialize` is not canceled with $/cancelRequest");
}
