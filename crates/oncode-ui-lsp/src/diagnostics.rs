//! Diagnostics dari semua language server sebuah jendela, per berkas. Dipakai editor, panel
//! Problems, dan status bar.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use gpui_kit::{Context, EventEmitter};
use lsp_types::{DiagnosticSeverity, NumberOrString};
use oncode_lsp::uri::uri_to_path;
use oncode_lsp::PositionEncoding;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

/// Satu diagnostic dalam bentuk yang ditampilkan panel Problems.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// Baris mulai, berbasis 1.
    pub line: u32,
    /// Kolom mulai berbasis 0, dalam satuan kolom server (lihat [`FileDiagnostics::encoding`]).
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub severity: Severity,
    pub message: String,
    pub source: Option<String>,
    pub code: Option<String>,
}

/// Menurut spesifikasi, tanpa severity klien bebas menafsirkan; dianggap galat seperti v0.2.
pub fn to_severity(severity: Option<DiagnosticSeverity>) -> Severity {
    match severity {
        Some(DiagnosticSeverity::WARNING) => Severity::Warning,
        Some(DiagnosticSeverity::INFORMATION) => Severity::Info,
        Some(DiagnosticSeverity::HINT) => Severity::Hint,
        _ => Severity::Error,
    }
}

pub fn convert_diagnostic(item: &lsp_types::Diagnostic) -> Problem {
    let code = match &item.code {
        Some(NumberOrString::Number(number)) => Some(number.to_string()),
        Some(NumberOrString::String(text)) if !text.is_empty() => Some(text.clone()),
        _ => None,
    };
    Problem {
        line: item.range.start.line + 1,
        column: item.range.start.character,
        end_line: item.range.end.line + 1,
        end_column: item.range.end.character,
        severity: to_severity(item.severity),
        message: item.message.clone(),
        source: item.source.clone().filter(|source| !source.is_empty()),
        code,
    }
}

/// Isi `textDocument/publishDiagnostics` yang dibaca dengan longgar: butir yang bentuknya rusak
/// dilewati, notifikasi tanpa `uri` atau daftar yang sah diabaikan seluruhnya.
pub fn parse_publish(params: &Value) -> Option<(String, Option<i32>, Vec<lsp_types::Diagnostic>)> {
    let uri = params.get("uri")?.as_str()?.to_string();
    let items = params.get("diagnostics")?.as_array()?;
    let version = params.get("version").and_then(Value::as_i64).map(|version| version as i32);
    let diagnostics = items
        .iter()
        .filter_map(|item| {
            let mut item = item.clone();
            // Bentuk lama `code: { value }` dinormalkan agar tidak menggagalkan butirnya.
            if let Some(value) = item.get("code").and_then(|code| code.get("value")).cloned() {
                item["code"] = value;
            }
            serde_json::from_value::<lsp_types::Diagnostic>(item).ok()
        })
        .collect();
    Some((uri, version, diagnostics))
}

#[derive(Debug, Clone)]
pub struct FileDiagnostics {
    pub problems: Vec<Problem>,
    /// Bentuk asli dari server, untuk editor dan konteks `textDocument/codeAction`.
    pub raw: Vec<lsp_types::Diagnostic>,
    /// Satuan kolom server yang menerbitkannya.
    pub encoding: PositionEncoding,
    pub version: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProblemCounts {
    pub errors: usize,
    pub warnings: usize,
    /// Mencakup `info` dan `hint`.
    pub infos: usize,
}

impl ProblemCounts {
    pub fn add(&mut self, severity: Severity) {
        match severity {
            Severity::Error => self.errors += 1,
            Severity::Warning => self.warnings += 1,
            Severity::Info | Severity::Hint => self.infos += 1,
        }
    }

    pub fn total(&self) -> usize {
        self.errors + self.warnings + self.infos
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticsEvent {
    /// Diagnostics berkas-berkas ini (path absolut) berganti.
    Changed(Vec<String>),
}

/// Store diagnostics satu jendela. Path absolut → diagnostics; berkas tanpa diagnostics tidak
/// punya entri.
#[derive(Default)]
pub struct Diagnostics {
    files: BTreeMap<String, FileDiagnostics>,
    /// Server mana yang menerbitkan diagnostics untuk path mana, agar bisa dihapus saat server itu berhenti.
    owners: HashMap<String, BTreeSet<String>>,
    counts: ProblemCounts,
    revision: u64,
}

impl EventEmitter<DiagnosticsEvent> for Diagnostics {}

impl Diagnostics {
    pub fn files(&self) -> &BTreeMap<String, FileDiagnostics> {
        &self.files
    }

    pub fn for_path(&self, path: &str) -> Option<&FileDiagnostics> {
        self.files.get(path)
    }

    /// Jumlah seluruh masalah, tanpa penyaring.
    pub fn counts(&self) -> ProblemCounts {
        self.counts
    }

    /// Bertambah setiap kali isi store berubah.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn recount(&mut self) {
        let mut counts = ProblemCounts::default();
        for file in self.files.values() {
            for problem in &file.problems {
                counts.add(problem.severity);
            }
        }
        self.counts = counts;
        self.revision += 1;
    }

    /// Versi tanpa `Context` dari [`Diagnostics::publish`]; mengembalikan path yang berubah.
    fn publish_inner(&mut self, owner: &str, encoding: PositionEncoding, params: &Value) -> Option<String> {
        let (uri, version, diagnostics) = parse_publish(params)?;
        let path = uri_to_path(&uri);
        if diagnostics.is_empty() {
            if let Some(paths) = self.owners.get_mut(owner) {
                paths.remove(&path);
            }
            self.files.remove(&path)?;
        } else {
            self.owners.entry(owner.to_string()).or_default().insert(path.clone());
            let problems = diagnostics.iter().map(convert_diagnostic).collect();
            self.files.insert(path.clone(), FileDiagnostics { problems, raw: diagnostics, encoding, version });
        }
        self.recount();
        Some(path)
    }

    /// Mencatat satu notifikasi `publishDiagnostics` dari server `owner`; daftar kosong menghapus
    /// entri berkasnya. Mengembalikan path berkasnya bila store berubah.
    pub fn publish(&mut self, owner: &str, encoding: PositionEncoding, params: &Value, cx: &mut Context<Self>) -> Option<String> {
        let path = self.publish_inner(owner, encoding, params)?;
        cx.emit(DiagnosticsEvent::Changed(vec![path.clone()]));
        cx.notify();
        Some(path)
    }

    fn clear_inner(&mut self, owner: Option<&str>) -> Vec<String> {
        let paths: Vec<String> = match owner {
            None => {
                self.owners.clear();
                std::mem::take(&mut self.files).into_keys().collect()
            }
            Some(owner) => {
                let paths = self.owners.remove(owner).unwrap_or_default();
                paths.into_iter().filter(|path| self.files.remove(path).is_some()).collect()
            }
        };
        if !paths.is_empty() {
            self.recount();
        }
        paths
    }

    /// Menghapus diagnostics milik satu server (server berhenti), atau semuanya bila `owner`
    /// tidak diberikan. Mengembalikan path yang entrinya terhapus.
    pub fn clear(&mut self, owner: Option<&str>, cx: &mut Context<Self>) -> Vec<String> {
        let paths = self.clear_inner(owner);
        if !paths.is_empty() {
            cx.emit(DiagnosticsEvent::Changed(paths.clone()));
            cx.notify();
        }
        paths
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn range(line: u32, character: u32, end_line: u32, end_character: u32) -> Value {
        json!({ "start": { "line": line, "character": character }, "end": { "line": end_line, "character": end_character } })
    }

    fn one(item: Value) -> Problem {
        let (_, _, diagnostics) = parse_publish(&json!({ "uri": "file:///a", "diagnostics": [item] })).unwrap();
        convert_diagnostic(&diagnostics[0])
    }

    fn paths(store: &Diagnostics) -> Vec<&str> {
        store.files().keys().map(String::as_str).collect()
    }

    fn messages(store: &Diagnostics, path: &str) -> Vec<String> {
        store.for_path(path).unwrap().problems.iter().map(|problem| problem.message.clone()).collect()
    }

    const ENC: PositionEncoding = PositionEncoding::Utf16;

    #[test]
    fn converts_lsp_diagnostics() {
        assert_eq!(to_severity(Some(DiagnosticSeverity::ERROR)), Severity::Error);
        assert_eq!(to_severity(Some(DiagnosticSeverity::WARNING)), Severity::Warning);
        assert_eq!(to_severity(Some(DiagnosticSeverity::INFORMATION)), Severity::Info);
        assert_eq!(to_severity(Some(DiagnosticSeverity::HINT)), Severity::Hint);
        assert_eq!(to_severity(None), Severity::Error);
        // Baris berbasis 1, kolom berbasis 0.
        assert_eq!(
            one(json!({ "range": range(4, 8, 5, 2), "severity": 2, "message": "unused variable", "source": "rustc", "code": "unused_variables" })),
            Problem {
                line: 5,
                column: 8,
                end_line: 6,
                end_column: 2,
                severity: Severity::Warning,
                message: "unused variable".into(),
                source: Some("rustc".into()),
                code: Some("unused_variables".into()),
            }
        );
        assert_eq!(one(json!({ "range": range(0, 0, 0, 1), "message": "x", "code": 2304 })).code.as_deref(), Some("2304"));
        assert_eq!(one(json!({ "range": range(0, 0, 0, 1), "message": "x", "code": { "value": "E0308" } })).code.as_deref(), Some("E0308"));
        assert_eq!(one(json!({ "range": range(0, 0, 0, 1), "message": "x" })).code, None);
        assert_eq!(one(json!({ "range": range(0, 0, 0, 1), "message": "x", "code": "" })).code, None);
    }

    #[test]
    fn records_publish_diagnostics_per_path_and_replaces_the_old_list() {
        let mut store = Diagnostics::default();
        let changed = store.publish_inner("rust-analyzer", ENC, &json!({ "uri": "file:///proj/src/a%20b.rs", "diagnostics": [{ "range": range(0, 0, 0, 1), "severity": 1, "message": "first" }] }));
        assert_eq!(changed.as_deref(), Some("/proj/src/a b.rs"));
        assert_eq!(paths(&store), ["/proj/src/a b.rs"]);
        assert_eq!(messages(&store, "/proj/src/a b.rs"), ["first"]);

        store.publish_inner(
            "rust-analyzer",
            ENC,
            &json!({ "uri": "file:///proj/src/a%20b.rs", "version": 3, "diagnostics": [
                { "range": range(1, 0, 1, 1), "severity": 1, "message": "second" },
                { "range": range(2, 0, 2, 1), "severity": 2, "message": "third" },
            ] }),
        );
        assert_eq!(messages(&store, "/proj/src/a b.rs"), ["second", "third"]);
        assert_eq!(store.for_path("/proj/src/a b.rs").unwrap().version, Some(3));
        assert_eq!(store.counts(), ProblemCounts { errors: 1, warnings: 1, infos: 0 });
    }

    #[test]
    fn an_empty_list_removes_the_file_entry() {
        let mut store = Diagnostics::default();
        store.publish_inner("s", ENC, &json!({ "uri": "file:///proj/a.rs", "diagnostics": [{ "range": range(0, 0, 0, 1), "message": "x" }] }));
        let before = store.revision();
        assert!(store.publish_inner("s", ENC, &json!({ "uri": "file:///proj/a.rs", "diagnostics": [] })).is_some());
        assert!(store.files().is_empty());
        assert_eq!(store.counts().total(), 0);
        // Berkas tanpa entri: store tidak berubah (tidak memicu render ulang).
        let revision = store.revision();
        assert!(revision > before);
        assert!(store.publish_inner("s", ENC, &json!({ "uri": "file:///proj/other.rs", "diagnostics": [] })).is_none());
        assert_eq!(store.revision(), revision);
    }

    #[test]
    fn a_stopped_server_only_takes_its_own_diagnostics() {
        let mut store = Diagnostics::default();
        let item = |message: &str| json!([{ "range": range(0, 0, 0, 1), "message": message }]);
        store.publish_inner("rust-analyzer", ENC, &json!({ "uri": "file:///proj/a.rs", "diagnostics": item("rust") }));
        store.publish_inner("rust-analyzer", ENC, &json!({ "uri": "file:///proj/b.rs", "diagnostics": item("rust") }));
        store.publish_inner("gopls", ENC, &json!({ "uri": "file:///proj/main.go", "diagnostics": item("go") }));
        assert_eq!(store.clear_inner(Some("rust-analyzer")), ["/proj/a.rs", "/proj/b.rs"]);
        assert_eq!(paths(&store), ["/proj/main.go"]);
        assert!(store.clear_inner(Some("rust-analyzer")).is_empty());
        assert_eq!(paths(&store), ["/proj/main.go"]);
        assert_eq!(store.clear_inner(None), ["/proj/main.go"]);
        assert!(store.files().is_empty());
        assert_eq!(store.counts(), ProblemCounts::default());
    }

    #[test]
    fn malformed_notifications_are_ignored() {
        let mut store = Diagnostics::default();
        assert!(store.publish_inner("s", ENC, &json!({ "uri": 42, "diagnostics": [] })).is_none());
        assert!(store.publish_inner("s", ENC, &json!({ "uri": "file:///a", "diagnostics": null })).is_none());
        assert!(store.publish_inner("s", ENC, &json!({ "uri": "file:///a", "diagnostics": [{ "message": "no range" }, null] })).is_none());
        assert!(store.files().is_empty());
        // Butir yang sah di antara yang rusak tetap dicatat.
        store.publish_inner("s", ENC, &json!({ "uri": "file:///a", "diagnostics": [null, { "range": range(0, 0, 0, 1), "message": "ok" }] }));
        assert_eq!(messages(&store, "/a"), ["ok"]);
    }
}
