//! Menerapkan `WorkspaceEdit` (rename, code action): berkas yang terbuka disunting lewat editornya
//! sehingga bisa di-undo; berkas yang tertutup ditulis langsung ke disk dengan encoding aslinya.

use std::collections::BTreeMap;

use gpui_kit::{App, Task, WeakEntity, Window};
use lsp_types::{DocumentChangeOperation, DocumentChanges, OneOf, TextEdit, WorkspaceEdit};
use oncode_core::api::fs;
use oncode_lsp::position::{apply_text_edits, PositionEncoding};
use oncode_lsp::uri::uri_to_path;
use oncode_ui::{overlay, spawn_core, Workbench};
use oncode_workspace::OpenedFile;

use crate::hub::{encoding_for, LspHub};

/// Suntingan per path absolut.
pub fn edits_by_path(edit: WorkspaceEdit) -> BTreeMap<String, Vec<TextEdit>> {
    let mut out: BTreeMap<String, Vec<TextEdit>> = BTreeMap::new();
    if let Some(changes) = edit.changes {
        for (uri, edits) in changes {
            out.entry(uri_to_path(uri.as_str())).or_default().extend(edits);
        }
    }
    let documents = match edit.document_changes {
        Some(DocumentChanges::Edits(edits)) => edits,
        Some(DocumentChanges::Operations(operations)) => operations
            .into_iter()
            .filter_map(|operation| match operation {
                DocumentChangeOperation::Edit(edit) => Some(edit),
                // Membuat/mengganti nama/menghapus berkas tidak didukung (sama dengan v0.2).
                DocumentChangeOperation::Op(_) => None,
            })
            .collect(),
        None => Vec::new(),
    };
    for document in documents {
        let edits = document.edits.into_iter().map(|edit| match edit {
            OneOf::Left(edit) => edit,
            OneOf::Right(annotated) => annotated.text_edit,
        });
        out.entry(uri_to_path(document.text_document.uri.as_str())).or_default().extend(edits);
    }
    out
}

/// Menerapkan suntingan lalu menampilkan ringkasannya. Hasil: jumlah berkas yang berubah.
pub fn apply_workspace_edit(hub: WeakEntity<LspHub>, edit: WorkspaceEdit, window: &mut Window, cx: &mut App) -> Task<usize> {
    let per_path = edits_by_path(edit);
    let Some(workbench) = Workbench::for_window(window, cx) else { return Task::ready(0) };
    let env = workbench.read(cx).env().clone();
    let buffers = oncode_ui::editor::buffers(workbench.read(cx));
    let mut changed = 0usize;
    let mut closed: Vec<(String, Vec<TextEdit>, PositionEncoding)> = Vec::new();
    for (path, edits) in per_path {
        if edits.is_empty() {
            continue;
        }
        let encoding = hub.upgrade().map(|hub| encoding_for(hub.read(cx), &path)).unwrap_or(PositionEncoding::Utf16);
        let open = buffers.as_ref().and_then(|buffers| buffers.read(cx).get(&path)).and_then(|buffer| buffer.read(cx).primary());
        match open {
            Some(state) => {
                state.update(cx, |state, cx| {
                    if encoding == PositionEncoding::Utf16 {
                        // Urutan dari belakang agar posisi suntingan berikutnya tetap sah.
                        let mut edits = edits.clone();
                        edits.sort_by(|a, b| b.range.start.cmp(&a.range.start));
                        state.apply_lsp_edits(&edits, window, cx);
                    } else {
                        let text = apply_text_edits(&state.text().to_string(), &edits, encoding);
                        state.set_value(text, window, cx);
                    }
                });
                changed += 1;
            }
            None => closed.push((path, edits, encoding)),
        }
    }
    if closed.is_empty() {
        overlay::toast_info(format!("Changed {changed} file{}.", if changed == 1 { "" } else { "s" }), window, cx);
        return Task::ready(changed);
    }
    let (core, label) = (env.core.clone(), env.window.to_string());
    let task = spawn_core(cx, async move {
        let mut written = 0usize;
        let mut failed = Vec::new();
        for (path, edits, encoding) in closed {
            match fs::fs_open(&core, &label, path.clone()).await {
                Ok(OpenedFile::Text(file)) => {
                    let text = apply_text_edits(&file.text, &edits, encoding);
                    match fs::fs_save(&core, &label, path.clone(), text, file.encoding, file.bom, file.eol).await {
                        Ok(_) => written += 1,
                        Err(error) => failed.push(format!("{path}: {}", error.message)),
                    }
                }
                Ok(_) => failed.push(format!("{path}: not a text file")),
                Err(error) => failed.push(format!("{path}: {}", error.message)),
            }
        }
        (written, failed)
    });
    window.spawn(cx, async move |cx| {
        let (written, failed) = task.await;
        let total = changed + written;
        let _ = cx.update(|window, cx| {
            if failed.is_empty() {
                overlay::toast_info(format!("Changed {total} file{}.", if total == 1 { "" } else { "s" }), window, cx);
            } else {
                overlay::toast_error(format!("Changed {total} files; could not change {}:\n{}", failed.len(), failed.join("\n")), window, cx);
            }
        });
        total
    })
}
