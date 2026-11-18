//! Berkas, pencarian, dan quick open. Semua path dibatasi ke folder proyek jendela pemanggil.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use oncode_workspace::{DirEntry, Eol, FileMatches, FileStat, OpenedFile, QuickOpenItem, ReplaceSummary, SearchQuery, SearchSummary};
use serde::Serialize;
use specta::Type;
use tauri::ipc::Channel;
use tauri::{State, WebviewWindow};

use crate::error::{AppError, AppResult};
use crate::state::Shared;

/// Batas ukuran berkas yang dibuka di editor.
const MAX_OPEN_BYTES: u64 = 50 * 1024 * 1024;

#[tauri::command]
#[specta::specta]
pub async fn fs_read_dir(window: WebviewWindow, state: State<'_, Shared>, dir: String) -> AppResult<Vec<DirEntry>> {
    let root = state.workspace_root(window.label())?;
    let dir = state.resolve(window.label(), &dir)?;
    Ok(oncode_workspace::read_dir(&root, &dir).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn fs_open(window: WebviewWindow, state: State<'_, Shared>, path: String) -> AppResult<OpenedFile> {
    let path = state.resolve(window.label(), &path)?;
    Ok(oncode_workspace::open_file(&path, MAX_OPEN_BYTES).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn fs_save(
    window: WebviewWindow,
    state: State<'_, Shared>,
    path: String,
    text: String,
    encoding: String,
    bom: bool,
    eol: Eol,
) -> AppResult<FileStat> {
    let path = state.resolve(window.label(), &path)?;
    Ok(oncode_workspace::save_file(&path, &text, &encoding, bom, eol).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn fs_stat(window: WebviewWindow, state: State<'_, Shared>, path: String) -> AppResult<Option<FileStat>> {
    let path = state.resolve(window.label(), &path)?;
    Ok(oncode_workspace::stat(&path).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn fs_create_file(window: WebviewWindow, state: State<'_, Shared>, path: String) -> AppResult<()> {
    let path = state.resolve(window.label(), &path)?;
    Ok(oncode_workspace::create_file(&path).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn fs_create_dir(window: WebviewWindow, state: State<'_, Shared>, path: String) -> AppResult<()> {
    let path = state.resolve(window.label(), &path)?;
    Ok(oncode_workspace::create_dir(&path).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn fs_rename(window: WebviewWindow, state: State<'_, Shared>, from: String, to: String) -> AppResult<()> {
    let from = state.resolve(window.label(), &from)?;
    let to = state.resolve(window.label(), &to)?;
    Ok(oncode_workspace::rename(&from, &to).await?)
}

/// Memindahkan berkas atau folder ke tempat sampah OS.
#[tauri::command]
#[specta::specta]
pub async fn fs_trash(window: WebviewWindow, state: State<'_, Shared>, paths: Vec<String>) -> AppResult<()> {
    let root = state.workspace_root(window.label())?;
    let mut resolved: Vec<PathBuf> = Vec::with_capacity(paths.len());
    for path in &paths {
        let full = state.resolve(window.label(), path)?;
        if full == root {
            return Err(AppError::invalid("The project folder cannot be deleted from here"));
        }
        resolved.push(full);
    }
    Ok(oncode_workspace::trash(&resolved).await?)
}

#[tauri::command]
#[specta::specta]
pub async fn quick_open(window: WebviewWindow, state: State<'_, Shared>, pattern: String, limit: u32) -> AppResult<Vec<QuickOpenItem>> {
    let index = state.with_workspace(window.label(), |ws| ws.index.clone())?;
    let items = index.read().unwrap().query(&pattern, limit.min(200) as usize);
    Ok(items)
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SearchEvent {
    Matches { file: FileMatches },
    Done { summary: SearchSummary },
    Failed { message: String },
}

/// Pencarian teks di seluruh proyek. Hasil dialirkan lewat `on_event`; batalkan dengan `app_cancel(operation_id)`.
#[tauri::command]
#[specta::specta]
pub async fn search_start(
    window: WebviewWindow,
    state: State<'_, Shared>,
    query: SearchQuery,
    operation_id: String,
    on_event: Channel<SearchEvent>,
) -> AppResult<()> {
    let root = state.workspace_root(window.label())?;
    let token = state.begin_operation(&operation_id);
    let cancel = Arc::new(AtomicBool::new(false));
    let watcher_flag = cancel.clone();
    let watcher_token = token.clone();
    let watch = tokio::spawn(async move {
        watcher_token.cancelled().await;
        watcher_flag.store(true, Ordering::SeqCst);
    });

    let sender = on_event.clone();
    let result = tokio::task::spawn_blocking(move || {
        oncode_workspace::search(&root, &query, &cancel, &|file| {
            let _ = sender.send(SearchEvent::Matches { file });
        })
    })
    .await;
    watch.abort();
    state.end_operation(&operation_id);

    let event = match result {
        Ok(Ok(summary)) => SearchEvent::Done { summary },
        Ok(Err(e)) => SearchEvent::Failed { message: e.message },
        Err(e) => SearchEvent::Failed { message: e.to_string() },
    };
    let _ = on_event.send(event);
    Ok(())
}

/// Mengganti semua kecocokan di berkas yang diberikan (yang tidak sedang terbuka di editor).
#[tauri::command]
#[specta::specta]
pub async fn search_replace(
    window: WebviewWindow,
    state: State<'_, Shared>,
    query: SearchQuery,
    replacement: String,
    files: Vec<String>,
) -> AppResult<ReplaceSummary> {
    let root = state.workspace_root(window.label())?;
    for file in &files {
        state.resolve(window.label(), file)?;
    }
    tokio::task::spawn_blocking(move || oncode_workspace::replace_in_files(&root, &query, &replacement, &files))
        .await
        .map_err(|e| AppError::new("io", e.to_string()))?
        .map_err(Into::into)
}
