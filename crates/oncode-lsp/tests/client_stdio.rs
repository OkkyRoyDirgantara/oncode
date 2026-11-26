//! End-to-end test: the LSP client on top of `LspManager` against the mock server as a real
//! stdio process (`examples/mock_lsp_server.rs`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lsp_types::request::Request;
use lsp_types::{GotoDefinitionParams, GotoDefinitionResponse, Position, TextDocumentIdentifier, TextDocumentPositionParams};
use oncode_lsp::uri::{path_to_lsp_uri, uri_to_path};
use oncode_lsp::{Client, ClientConfig, ClientHandler, LspManager, RpcError, ServerExitInfo, ServerSpec};
use serde_json::{json, Value};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::time::timeout;

const TIMEOUT: Duration = Duration::from_secs(20);

/// The example binary is built by `cargo test` into `target/<profile>/examples/`. It is looked up from the test
/// binary's folder upward, because the location of the test binary differs between Cargo versions: `target/<profile>/deps/` or
/// `target/<profile>/build/<crate>/<hash>/out/` (the new build-dir layout).
fn mock_server() -> PathBuf {
    let exe = std::env::current_exe().expect("test path");
    let name = if cfg!(windows) { "mock_lsp_server.exe" } else { "mock_lsp_server" };
    let relative = Path::new("examples").join(name);
    exe.ancestors()
        .skip(1)
        .map(|dir| dir.join(&relative))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("mock server has not been built (cargo test -p oncode-lsp): {} not found above {}", relative.display(), exe.display()))
}

struct Forward(UnboundedSender<(String, Value)>);

impl ClientHandler for Forward {
    fn notification(&self, method: &str, params: Value) {
        let _ = self.0.send((method.to_string(), params));
    }
}

struct Session {
    client: Client,
    id: u32,
    notifications: UnboundedReceiver<(String, Value)>,
    exit: UnboundedReceiver<ServerExitInfo>,
}

impl Session {
    async fn notification(&mut self, method: &str) -> Value {
        loop {
            let (name, params) = timeout(TIMEOUT, self.notifications.recv()).await.expect("notification did not arrive").expect("channel closed");
            if name == method {
                return params;
            }
        }
    }

    async fn exit(&mut self) -> ServerExitInfo {
        timeout(TIMEOUT, self.exit.recv()).await.expect("on_exit was not called").expect("exit channel closed")
    }
}

/// Wires the client to the server process: outgoing messages are written in order to the server's stdin,
/// messages from stdout are handed to the client, and the client is closed when the process ends.
async fn start(manager: &Arc<LspManager>, args: &[&str], root: &Path) -> Session {
    let (notification_tx, notifications) = unbounded_channel();
    let (exit_tx, exit) = unbounded_channel();
    let config = ClientConfig { root: Some(root.to_string_lossy().into_owned()), ..Default::default() };
    let (client, mut outgoing) = Client::new(config, Arc::new(Forward(notification_tx)));
    let spec = ServerSpec { command: mock_server().to_string_lossy().into_owned(), args: args.iter().map(|arg| arg.to_string()).collect(), env: vec![] };
    let (on_message, on_exit) = (client.clone(), client.clone());
    let id = manager
        .start(&spec, root, move |json| on_message.receive(&json), move |info| {
            on_exit.close();
            let _ = exit_tx.send(info);
        })
        .await
        .expect("mock server runs");
    let writer = manager.clone();
    tokio::spawn(async move {
        while let Some(message) = outgoing.recv().await {
            if writer.send(id, &message).await.is_err() {
                break;
            }
        }
    });
    Session { client, id, notifications, exit }
}

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temporary directory");
    std::fs::write(dir.path().join("library.txt"), "def append\ndef remove\n").unwrap();
    std::fs::write(dir.path().join("main.txt"), "append ERROR\nremove WARN\n").unwrap();
    dir
}

enum MockState {}
impl Request for MockState {
    type Params = Value;
    type Result = Value;
    const METHOD: &'static str = "mock/state";
}

#[tokio::test(flavor = "multi_thread")]
async fn full_session_over_stdio() {
    let dir = project();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let manager = Arc::new(LspManager::new());
    let mut session = start(&manager, &[], &root).await;

    let result = timeout(TIMEOUT, session.client.initialize()).await.expect("initialize on time").expect("initialize");
    assert_eq!(result.server_info.unwrap().name, "mock-lsp");
    assert!(session.client.is_ready());

    // Project diagnostics are published after `initialized`, also for files that have not been opened.
    let utama = root.join("main.txt");
    let published = session.notification("textDocument/publishDiagnostics").await;
    assert_eq!(uri_to_path(published["uri"].as_str().unwrap()), utama.to_string_lossy());
    let severities: Vec<i64> = published["diagnostics"].as_array().unwrap().iter().map(|d| d["severity"].as_i64().unwrap()).collect();
    assert_eq!(severities, [1, 2]);

    // Open the document, then go-to-definition into another file that only exists on disk.
    let uri = path_to_lsp_uri(&utama.to_string_lossy()).unwrap();
    session.client.did_open(&uri, "plaintext", std::fs::read_to_string(&utama).unwrap()).unwrap();
    assert_eq!(session.notification("textDocument/publishDiagnostics").await["version"], 1);
    let params = GotoDefinitionParams {
        text_document_position_params: TextDocumentPositionParams { text_document: TextDocumentIdentifier { uri: uri.clone() }, position: Position { line: 1, character: 2 } },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };
    let Some(GotoDefinitionResponse::Array(locations)) = session.client.request::<lsp_types::request::GotoDefinition>(params).await.unwrap() else { panic!("definition location") };
    assert_eq!(locations.len(), 1);
    assert_eq!(uri_to_path(locations[0].uri.as_str()), root.join("library.txt").to_string_lossy());
    assert_eq!((locations[0].range.start.line, locations[0].range.start.character), (1, 4));

    // The server receives the client's answers to its requests over the same pipe.
    let state = session.client.request::<MockState>(Value::Null).await.unwrap();
    assert_eq!(state["answers"].as_array().unwrap().len(), 3);
    assert_eq!(state["open"], json!([uri.as_str()]));

    // Clean shutdown: shutdown + exit, the process exits with code 0 and the client is closed.
    session.client.shutdown(Duration::from_secs(5)).await;
    let info = session.exit().await;
    assert_eq!(info.code, Some(0));
    assert!(session.client.is_closed());
    assert!(manager.ids().is_empty());
    assert!(manager.send(session.id, "{}").await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_crash_fails_pending_requests() {
    let dir = project();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let manager = Arc::new(LspManager::new());
    let mut session = start(&manager, &[], &root).await;
    session.client.initialize().await.unwrap();

    enum Hang {}
    impl Request for Hang {
        type Params = Value;
        type Result = Value;
        const METHOD: &'static str = "mock/hang";
    }
    enum Crash {}
    impl lsp_types::notification::Notification for Crash {
        type Params = Value;
        const METHOD: &'static str = "mock/crash";
    }
    let pending = session.client.request::<Hang>(Value::Null);
    session.client.notify::<Crash>(json!({ "code": 7 }));
    assert_eq!(timeout(TIMEOUT, pending).await.expect("request fails when the server dies").unwrap_err(), RpcError::Closed);
    let info = session.exit().await;
    assert_eq!(info.code, Some(7));
    assert!(info.stderr_tail.contains("exiting with code 7"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_exits_on_start_never_answers_initialize() {
    let dir = project();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let manager = Arc::new(LspManager::new());
    let mut session = start(&manager, &["--exit-on-start", "3"], &root).await;
    let seen = Arc::new(Mutex::new(None));
    let result = timeout(TIMEOUT, session.client.initialize()).await.expect("initialize ends when the process exits");
    *seen.lock().unwrap() = result.as_ref().err().cloned();
    assert_eq!(seen.lock().unwrap().clone(), Some(RpcError::Closed));
    let info = session.exit().await;
    assert_eq!(info.code, Some(3));
    assert!(info.stderr_tail.contains("exiting on start"));
}
