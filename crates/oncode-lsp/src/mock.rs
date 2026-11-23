//! Language server tiruan untuk pengujian: bahasa mainan yang cukup untuk melatih semua fitur klien.
//!
//! Aturan "bahasa"-nya:
//! - pengenal adalah `[A-Za-z_][A-Za-z0-9_]*`; `def nama` mendefinisikan `nama`;
//! - kata `ERROR`, `WARN`, `INFO`, dan `HINT` menghasilkan diagnostics dengan severity itu;
//! - berkas proyek adalah berkas `.txt`/`.mock` di bawah folder akar.
//!
//! Server sengaja ditulis dengan `serde_json::Value` mentah, terpisah dari tipe yang dipakai
//! klien, supaya bentuk pesan di kabel ikut teruji. Dipakai di dalam proses lewat
//! [`MockServer::handle`] dan sebagai program stdio lewat [`run_stdio`].

use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use lsp_types::{Position, Range};
use serde_json::{json, Value};

use crate::frame::{encode_frame, FrameDecoder};
use crate::position::{offset_at, position_at, PositionEncoding};
use crate::uri::{path_to_uri, uri_key, uri_to_path};

#[derive(Debug, Clone, Default)]
pub struct MockOptions {
    /// Memilih UTF-8 sebagai satuan kolom bila klien menawarkannya (bawaan: UTF-16).
    pub utf8: bool,
    /// Meminta sinkronisasi penuh, bukan bertahap.
    pub full_sync: bool,
    /// Tidak pernah menjawab `initialize`.
    pub silent_initialize: bool,
    /// Keluar dengan kode ini sebelum membaca apa pun (meniru server yang tidak terpasang benar).
    pub exit_on_start: Option<i32>,
}

impl MockOptions {
    /// `--utf8`, `--full-sync`, `--silent-initialize`, `--exit-on-start <kode>`.
    pub fn from_args(args: &[String]) -> Self {
        let mut options = Self::default();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--utf8" => options.utf8 = true,
                "--full-sync" => options.full_sync = true,
                "--silent-initialize" => options.silent_initialize = true,
                "--exit-on-start" => options.exit_on_start = args.next().and_then(|code| code.parse().ok()),
                _ => {}
            }
        }
        options
    }
}

struct Doc {
    text: String,
    version: i64,
    language: String,
}

pub struct MockServer {
    options: MockOptions,
    root: Option<PathBuf>,
    docs: BTreeMap<String, Doc>,
    encoding: PositionEncoding,
    shutdown: bool,
    exit: Option<i32>,
    next_id: i64,
    /// Permintaan server→klien yang belum dijawab: id → metode.
    awaiting: BTreeMap<i64, String>,
    /// Jawaban klien atas permintaan server, sesuai urutan tiba: `{method, result|error}`.
    answers: Vec<Value>,
    /// Metode semua pesan yang diterima dari klien, sesuai urutan.
    received: Vec<String>,
    cancelled: Vec<Value>,
}

fn is_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

/// Rentang byte semua pengenal di `text`.
fn words(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (index, ch) in text.char_indices() {
        match (is_word_char(ch), start) {
            (true, None) if !ch.is_ascii_digit() => start = Some(index),
            (false, Some(begin)) => {
                out.push((begin, index));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(begin) = start {
        out.push((begin, text.len()));
    }
    out
}

/// Pengenal yang memuat `offset` atau berakhir tepat di sana.
fn word_at(text: &str, offset: usize) -> Option<(usize, usize)> {
    words(text).into_iter().find(|(start, end)| *start <= offset && offset <= *end)
}

/// Rentang nama yang didefinisikan dengan `def nama`.
fn definitions(text: &str) -> Vec<(usize, usize)> {
    let all = words(text);
    all.windows(2).filter(|pair| &text[pair[0].0..pair[0].1] == "def" && text[pair[0].1..pair[1].0].chars().all(|ch| ch == ' ' || ch == '\t')).map(|pair| pair[1]).collect()
}

fn collect_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 4 || out.len() >= 200 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = entries.filter_map(|entry| entry.ok().map(|entry| entry.path())).collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_string();
        if name.starts_with('.') || name == "node_modules" || name == "target" {
            continue;
        }
        if path.is_dir() {
            collect_files(&path, depth + 1, out);
        } else if matches!(path.extension().and_then(|ext| ext.to_str()), Some("txt" | "mock")) && path.metadata().is_ok_and(|meta| meta.len() <= 256 * 1024) {
            out.push(path);
        }
    }
}

impl MockServer {
    pub fn new(options: MockOptions) -> Self {
        Self {
            options,
            root: None,
            docs: BTreeMap::new(),
            encoding: PositionEncoding::Utf16,
            shutdown: false,
            exit: None,
            next_id: 0,
            awaiting: BTreeMap::new(),
            answers: Vec::new(),
            received: Vec::new(),
            cancelled: Vec::new(),
        }
    }

    /// Kode keluar bila server memutuskan berhenti (`exit`, atau `mock/crash`).
    pub fn exit_code(&self) -> Option<i32> {
        self.exit
    }

    /// Isi dokumen yang sedang terbuka menurut server.
    pub fn document(&self, uri: &str) -> Option<&str> {
        let key = uri_key(uri);
        self.docs.iter().find(|(open, _)| uri_key(open) == key).map(|(_, doc)| doc.text.as_str())
    }

    fn doc(&self, uri: &str) -> Option<(&String, &Doc)> {
        let key = uri_key(uri);
        self.docs.iter().find(|(open, _)| uri_key(open) == key)
    }

    /// Semua berkas proyek sebagai `(uri, isi)`: dokumen terbuka (isi dari memori) lalu berkas di disk.
    fn workspace(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self.docs.iter().map(|(uri, doc)| (uri.clone(), doc.text.clone())).collect();
        let Some(root) = &self.root else { return out };
        let mut files = Vec::new();
        collect_files(root, 0, &mut files);
        for file in files {
            let uri = path_to_uri(&file.to_string_lossy());
            if self.doc(&uri).is_some() {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&file) {
                out.push((uri, text.replace("\r\n", "\n")));
            }
        }
        out.sort();
        out
    }

    fn range(&self, text: &str, span: (usize, usize)) -> Value {
        let range = Range { start: position_at(text, span.0, self.encoding), end: position_at(text, span.1, self.encoding) };
        json!(range)
    }

    fn diagnostics_for(&self, text: &str) -> Vec<Value> {
        words(text)
            .into_iter()
            .filter_map(|span| {
                let (severity, message, code) = match &text[span.0..span.1] {
                    "ERROR" => (1, "Unexpected ERROR", "E001"),
                    "WARN" => (2, "Suspicious WARN", "W001"),
                    "INFO" => (3, "Note INFO", "I001"),
                    "HINT" => (4, "Consider HINT", "H001"),
                    _ => return None,
                };
                Some(json!({ "range": self.range(text, span), "severity": severity, "message": message, "source": "mock", "code": code }))
            })
            .collect()
    }

    fn publish(&self, uri: &str, text: &str, version: Option<i64>) -> Value {
        let mut params = json!({ "uri": uri, "diagnostics": self.diagnostics_for(text) });
        if let Some(version) = version {
            params["version"] = json!(version);
        }
        json!({ "jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": params })
    }

    fn ask(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        self.awaiting.insert(self.next_id, method.to_string());
        json!({ "jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params })
    }

    /// Posisi permintaan sebagai `(uri terbuka, isi, offset byte)`.
    fn locate(&self, params: &Value) -> Option<(String, String, usize)> {
        let uri = params["textDocument"]["uri"].as_str()?;
        let (uri, doc) = self.doc(uri)?;
        let position: Position = serde_json::from_value(params["position"].clone()).ok()?;
        Some((uri.clone(), doc.text.clone(), offset_at(&doc.text, position, self.encoding)))
    }

    fn occurrences(&self, name: &str, declarations_only: bool) -> Vec<Value> {
        let mut out = Vec::new();
        for (uri, text) in self.workspace() {
            let spans = if declarations_only { definitions(&text) } else { words(&text) };
            for span in spans.into_iter().filter(|span| &text[span.0..span.1] == name) {
                out.push(json!({ "uri": uri, "range": self.range(&text, span) }));
            }
        }
        out
    }

    fn completion(&self, params: &Value) -> Value {
        let Some((_, text, offset)) = self.locate(params) else { return Value::Null };
        let start = text[..offset].char_indices().rev().take_while(|(_, ch)| is_word_char(*ch)).last().map_or(offset, |(index, _)| index);
        let prefix = &text[start..offset];
        let mut names: BTreeMap<String, bool> = BTreeMap::new();
        for (_, source) in self.workspace() {
            let defined = definitions(&source);
            let current = source == text;
            for span in words(&source) {
                let name = &source[span.0..span.1];
                // Kata yang sedang diketik bukan saran.
                if name == "def" || (current && span.1 == offset && span.0 == start) {
                    continue;
                }
                *names.entry(name.to_string()).or_default() |= defined.contains(&span);
            }
        }
        let items: Vec<Value> = names
            .into_iter()
            .filter(|(name, _)| name.starts_with(prefix))
            .map(|(name, defined)| json!({ "label": name, "kind": if defined { 3 } else { 1 }, "detail": if defined { "definition" } else { "word" }, "data": { "name": name } }))
            .collect();
        json!({ "isIncomplete": false, "items": items })
    }

    fn hover(&self, params: &Value) -> Value {
        let Some((_, text, offset)) = self.locate(params) else { return Value::Null };
        let Some(span) = word_at(&text, offset) else { return Value::Null };
        let name = &text[span.0..span.1];
        let declared = self.occurrences(name, true).len();
        json!({ "contents": { "kind": "markdown", "value": format!("**{name}**\n\n{declared} definition(s)") }, "range": self.range(&text, span) })
    }

    fn signature_help(&self, params: &Value) -> Value {
        let Some((_, text, offset)) = self.locate(params) else { return Value::Null };
        let line_start = text[..offset].rfind('\n').map_or(0, |index| index + 1);
        let before = &text[line_start..offset];
        let Some(open) = before.rfind('(') else { return Value::Null };
        if before[open..].contains(')') {
            return Value::Null;
        }
        let Some(span) = word_at(before, open).filter(|span| span.1 == open) else { return Value::Null };
        let name = &before[span.0..span.1];
        let active = before[open..].matches(',').count();
        json!({
            "signatures": [{ "label": format!("{name}(a, b)"), "documentation": format!("Calls {name}"), "parameters": [{ "label": "a" }, { "label": "b" }] }],
            "activeSignature": 0,
            "activeParameter": active
        })
    }

    fn rename(&self, params: &Value) -> Value {
        let Some((_, text, offset)) = self.locate(params) else { return Value::Null };
        let Some(span) = word_at(&text, offset) else { return Value::Null };
        let name = text[span.0..span.1].to_string();
        let new_name = params["newName"].as_str().unwrap_or_default();
        let mut changes: Vec<Value> = Vec::new();
        for (uri, source) in self.workspace() {
            let edits: Vec<Value> = words(&source).into_iter().filter(|span| source[span.0..span.1] == name).map(|span| json!({ "range": self.range(&source, span), "newText": new_name })).collect();
            if edits.is_empty() {
                continue;
            }
            let version = self.doc(&uri).map(|(_, doc)| json!(doc.version)).unwrap_or(Value::Null);
            changes.push(json!({ "textDocument": { "uri": uri, "version": version }, "edits": edits }));
        }
        json!({ "documentChanges": changes })
    }

    /// Membuang spasi di akhir baris.
    fn formatting(&self, params: &Value) -> Value {
        let Some((_, doc)) = params["textDocument"]["uri"].as_str().and_then(|uri| self.doc(uri)) else { return Value::Null };
        let mut edits = Vec::new();
        let mut start = 0usize;
        for line in doc.text.split('\n') {
            let trimmed = line.trim_end_matches([' ', '\t']);
            if trimmed.len() < line.len() {
                edits.push(json!({ "range": self.range(&doc.text, (start + trimmed.len(), start + line.len())), "newText": "" }));
            }
            start += line.len() + 1;
        }
        json!(edits)
    }

    fn code_actions(&self, params: &Value) -> Value {
        let Some(uri) = params["textDocument"]["uri"].as_str() else { return Value::Null };
        let actions: Vec<Value> = params["context"]["diagnostics"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|diagnostic| diagnostic["code"] == "E001")
            .map(|diagnostic| {
                json!({
                    "title": "Replace ERROR with OK",
                    "kind": "quickfix",
                    "diagnostics": [diagnostic],
                    "edit": { "changes": { uri: [{ "range": diagnostic["range"], "newText": "OK" }] } }
                })
            })
            .collect();
        json!(actions)
    }

    fn capabilities(&self) -> Value {
        json!({
            "positionEncoding": self.encoding.kind(),
            "textDocumentSync": { "openClose": true, "change": if self.options.full_sync { 1 } else { 2 }, "save": { "includeText": false } },
            "completionProvider": { "resolveProvider": true, "triggerCharacters": ["."] },
            "hoverProvider": true,
            "signatureHelpProvider": { "triggerCharacters": ["(", ","] },
            "definitionProvider": true,
            "referencesProvider": true,
            "renameProvider": { "prepareProvider": true },
            "documentFormattingProvider": true,
            "codeActionProvider": { "codeActionKinds": ["quickfix"] }
        })
    }

    fn request(&mut self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
        Ok(match method {
            "initialize" => {
                self.root = params["rootUri"].as_str().map(|uri| PathBuf::from(uri_to_path(uri)));
                let offered: Vec<&str> = params["capabilities"]["general"]["positionEncodings"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                self.encoding = if self.options.utf8 && offered.contains(&"utf-8") { PositionEncoding::Utf8 } else { PositionEncoding::Utf16 };
                json!({ "capabilities": self.capabilities(), "serverInfo": { "name": "mock-lsp", "version": "1.0" } })
            }
            "shutdown" => {
                self.shutdown = true;
                Value::Null
            }
            "textDocument/completion" => self.completion(params),
            "completionItem/resolve" => {
                let mut item = params.clone();
                let name = item["data"]["name"].as_str().unwrap_or_default().to_string();
                item["documentation"] = json!({ "kind": "markdown", "value": format!("Documentation for `{name}`") });
                item
            }
            "textDocument/hover" => self.hover(params),
            "textDocument/signatureHelp" => self.signature_help(params),
            "textDocument/definition" => match self.locate(params).and_then(|(_, text, offset)| word_at(&text, offset).map(|span| text[span.0..span.1].to_string())) {
                Some(name) => json!(self.occurrences(&name, true)),
                None => Value::Null,
            },
            "textDocument/references" => match self.locate(params).and_then(|(_, text, offset)| word_at(&text, offset).map(|span| text[span.0..span.1].to_string())) {
                Some(name) => {
                    let declarations = if params["context"]["includeDeclaration"] == false { self.occurrences(&name, true) } else { Vec::new() };
                    json!(self.occurrences(&name, false).into_iter().filter(|location| !declarations.contains(location)).collect::<Vec<_>>())
                }
                None => Value::Null,
            },
            "textDocument/prepareRename" => match self.locate(params).and_then(|(_, text, offset)| word_at(&text, offset).map(|span| self.range(&text, span))) {
                Some(range) => range,
                None => Value::Null,
            },
            "textDocument/rename" => {
                let new_name = params["newName"].as_str().unwrap_or_default();
                if new_name.is_empty() || !new_name.chars().all(is_word_char) {
                    return Err((-32602, format!("`{new_name}` is not a valid name")));
                }
                self.rename(params)
            }
            "textDocument/formatting" => self.formatting(params),
            "textDocument/codeAction" => self.code_actions(params),
            // Kait uji: keadaan server dan isi dokumen menurut server.
            "mock/document" => match params["uri"].as_str().and_then(|uri| self.doc(uri)) {
                Some((_, doc)) => json!({ "text": doc.text, "version": doc.version, "languageId": doc.language }),
                None => Value::Null,
            },
            "mock/state" => json!({ "answers": self.answers, "received": self.received, "cancelled": self.cancelled, "open": self.docs.keys().collect::<Vec<_>>() }),
            // Tidak pernah dijawab; untuk menguji batas waktu dan pembatalan.
            "mock/hang" => return Err((0, String::new())),
            _ => return Err((-32601, format!("Method not found: {method}"))),
        })
    }

    fn notification(&mut self, method: &str, params: &Value, out: &mut Vec<Value>) {
        match method {
            "initialized" => {
                out.push(self.ask("window/workDoneProgress/create", json!({ "token": "mock-progress" })));
                out.push(self.ask("workspace/configuration", json!({ "items": [{ "section": "mock" }, { "section": "mock.other" }] })));
                out.push(self.ask("client/registerCapability", json!({ "registrations": [{ "id": "mock-watch", "method": "workspace/didChangeWatchedFiles", "registerOptions": {} }] })));
                // Seperti server sungguhan, diagnostics seluruh proyek diterbitkan setelah siap.
                for (uri, text) in self.workspace() {
                    if !self.diagnostics_for(&text).is_empty() {
                        out.push(self.publish(&uri, &text, None));
                    }
                }
            }
            "textDocument/didOpen" => {
                let item = &params["textDocument"];
                let (Some(uri), Some(text)) = (item["uri"].as_str(), item["text"].as_str()) else { return };
                let doc = Doc { text: text.to_string(), version: item["version"].as_i64().unwrap_or(0), language: item["languageId"].as_str().unwrap_or_default().to_string() };
                out.push(self.publish(uri, &doc.text, Some(doc.version)));
                self.docs.insert(uri.to_string(), doc);
            }
            "textDocument/didChange" => {
                let Some(uri) = params["textDocument"]["uri"].as_str().and_then(|uri| self.doc(uri).map(|(uri, _)| uri.clone())) else { return };
                let Ok(changes) = serde_json::from_value::<Vec<lsp_types::TextDocumentContentChangeEvent>>(params["contentChanges"].clone()) else { return };
                let encoding = self.encoding;
                if let Some(doc) = self.docs.get_mut(&uri) {
                    crate::position::apply_content_changes(&mut doc.text, &changes, encoding);
                    doc.version = params["textDocument"]["version"].as_i64().unwrap_or(doc.version);
                }
                if let Some(doc) = self.docs.get(&uri) {
                    out.push(self.publish(&uri, &doc.text, Some(doc.version)));
                }
            }
            "textDocument/didClose" => {
                let Some(uri) = params["textDocument"]["uri"].as_str().and_then(|uri| self.doc(uri).map(|(uri, _)| uri.clone())) else { return };
                self.docs.remove(&uri);
                // Kembali ke isi di disk, seperti server yang menganalisis seluruh proyek.
                let on_disk = std::fs::read_to_string(uri_to_path(&uri)).unwrap_or_default();
                out.push(self.publish(&uri, &on_disk, None));
            }
            "$/cancelRequest" => self.cancelled.push(params["id"].clone()),
            "exit" => self.exit = Some(if self.shutdown { 0 } else { 1 }),
            // Kait uji: server mati mendadak, atau meminta klien menerapkan suntingan.
            "mock/crash" => self.exit = Some(params["code"].as_i64().unwrap_or(1) as i32),
            "mock/applyEdit" => out.push(self.ask("workspace/applyEdit", params.clone())),
            _ => {}
        }
    }

    /// Memproses satu pesan dari klien dan mengembalikan pesan-pesan balasan, sesuai urutan.
    pub fn handle(&mut self, message: &str) -> Vec<String> {
        let Ok(message) = serde_json::from_str::<Value>(message) else { return Vec::new() };
        let mut out: Vec<Value> = Vec::new();
        let id = message.get("id").filter(|id| !id.is_null()).cloned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match (message.get("method").and_then(Value::as_str), id) {
            (Some(method), Some(id)) => {
                self.received.push(method.to_string());
                if method == "initialize" && self.options.silent_initialize {
                    return Vec::new();
                }
                match self.request(method, &params) {
                    Ok(result) => out.push(json!({ "jsonrpc": "2.0", "id": id, "result": result })),
                    Err((0, _)) => {}
                    Err((code, message)) => out.push(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })),
                }
            }
            (Some(method), None) => {
                self.received.push(method.to_string());
                self.notification(method, &params, &mut out);
            }
            (None, Some(id)) => {
                if let Some(method) = id.as_i64().and_then(|id| self.awaiting.remove(&id)) {
                    let mut answer = json!({ "method": method });
                    for key in ["result", "error"] {
                        if let Some(value) = message.get(key) {
                            answer[key] = value.clone();
                        }
                    }
                    self.answers.push(answer);
                }
            }
            (None, None) => {}
        }
        out.into_iter().map(|value| value.to_string()).collect()
    }
}

/// Menjalankan server tiruan di atas stdin/stdout sampai `exit` atau stdin ditutup.
pub fn run_stdio(args: &[String]) -> i32 {
    let options = MockOptions::from_args(args);
    if let Some(code) = options.exit_on_start {
        eprintln!("mock-lsp: exiting on start with code {code}");
        return code;
    }
    let mut server = MockServer::new(options);
    let mut decoder = FrameDecoder::new();
    let (stdin, stdout) = (std::io::stdin(), std::io::stdout());
    let (mut stdin, mut stdout) = (stdin.lock(), stdout.lock());
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = match stdin.read(&mut buffer) {
            Ok(0) | Err(_) => return 0,
            Ok(read) => read,
        };
        for message in decoder.push(&buffer[..read]) {
            for reply in server.handle(&message) {
                if stdout.write_all(&encode_frame(&reply)).and_then(|()| stdout.flush()).is_err() {
                    return 1;
                }
            }
            if let Some(code) = server.exit_code() {
                eprintln!("mock-lsp: exiting with code {code}");
                return code;
            }
        }
    }
}
