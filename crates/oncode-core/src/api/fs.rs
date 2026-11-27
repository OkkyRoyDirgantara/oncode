//! Files, search, and quick open. All paths are confined to the calling window's project folder.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use oncode_workspace::{DirEntry, Eol, FileMatches, FileStat, OpenedFile, QuickOpenItem, ReplaceSummary, SearchQuery, SearchSummary};
use serde::Serialize;

use crate::api::Callback;
use crate::error::{AppError, AppResult};
use crate::state::Shared;

/// Size limit for files opened in the editor.
const MAX_OPEN_BYTES: u64 = 50 * 1024 * 1024;

pub async fn fs_read_dir(core: &Shared, window: &str, dir: String) -> AppResult<Vec<DirEntry>> {
    let root = core.workspace_root(window)?;
    let dir = core.resolve(window, &dir)?;
    Ok(oncode_workspace::read_dir(&root, &dir).await?)
}

pub async fn fs_open(core: &Shared, window: &str, path: String) -> AppResult<OpenedFile> {
    let path = core.resolve(window, &path)?;
    Ok(oncode_workspace::open_file(&path, MAX_OPEN_BYTES).await?)
}

/// The raw bytes of a file (e.g. an image to show), within the same size limit as [`fs_open`].
pub async fn fs_read_bytes(core: &Shared, window: &str, path: String) -> AppResult<Vec<u8>> {
    let path = core.resolve(window, &path)?;
    Ok(oncode_workspace::read_bytes(&path, MAX_OPEN_BYTES).await?)
}

pub async fn fs_save(
    core: &Shared,
    window: &str,
    path: String,
    text: String,
    encoding: String,
    bom: bool,
    eol: Eol,
) -> AppResult<FileStat> {
    let path = core.resolve(window, &path)?;
    Ok(oncode_workspace::save_file(&path, &text, &encoding, bom, eol).await?)
}

pub async fn fs_stat(core: &Shared, window: &str, path: String) -> AppResult<Option<FileStat>> {
    let path = core.resolve(window, &path)?;
    Ok(oncode_workspace::stat(&path).await?)
}

pub async fn fs_create_file(core: &Shared, window: &str, path: String) -> AppResult<()> {
    let path = core.resolve(window, &path)?;
    Ok(oncode_workspace::create_file(&path).await?)
}

pub async fn fs_create_dir(core: &Shared, window: &str, path: String) -> AppResult<()> {
    let path = core.resolve(window, &path)?;
    Ok(oncode_workspace::create_dir(&path).await?)
}

pub async fn fs_rename(core: &Shared, window: &str, from: String, to: String) -> AppResult<()> {
    let from = core.resolve(window, &from)?;
    let to = core.resolve(window, &to)?;
    Ok(oncode_workspace::rename(&from, &to).await?)
}

/// Copies files or folders from anywhere (the result of a drag-and-drop from the OS) into the folder `dest_dir`
/// in the project. Only the destination is confined to the project: the sources are chosen by the user via
/// that gesture. Does not overwrite; returns the paths of the copies.
pub async fn fs_copy_into(core: &Shared, window: &str, sources: Vec<String>, dest_dir: String) -> AppResult<Vec<String>> {
    let dest_dir = core.resolve(window, &dest_dir)?;
    let sources: Vec<PathBuf> = sources.into_iter().map(PathBuf::from).collect();
    let copied = oncode_workspace::copy_into(&sources, &dest_dir).await?;
    Ok(copied.into_iter().map(|path| path.to_string_lossy().into_owned()).collect())
}

/// Moves files or folders to the OS trash.
pub async fn fs_trash(core: &Shared, window: &str, paths: Vec<String>) -> AppResult<()> {
    let root = core.workspace_root(window)?;
    let mut resolved: Vec<PathBuf> = Vec::with_capacity(paths.len());
    for path in &paths {
        let full = core.resolve(window, path)?;
        if full == root {
            return Err(AppError::invalid("The project folder cannot be deleted from here"));
        }
        resolved.push(full);
    }
    Ok(oncode_workspace::trash(&resolved).await?)
}

pub async fn quick_open(core: &Shared, window: &str, pattern: String, limit: u32) -> AppResult<Vec<QuickOpenItem>> {
    let index = core.with_workspace(window, |ws| ws.index.clone())?;
    let items = index.read().unwrap().query(&pattern, limit.min(200) as usize);
    Ok(items)
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SearchEvent {
    Matches { file: FileMatches },
    Done { summary: SearchSummary },
    Failed { message: String },
}

/// Text search across the whole project. Results are streamed through `on_event`; cancel with `app_cancel(operation_id)`.
pub async fn search_start(
    core: &Shared,
    window: &str,
    query: SearchQuery,
    operation_id: String,
    on_event: Callback<SearchEvent>,
) -> AppResult<()> {
    let root = core.workspace_root(window)?;
    let token = core.begin_operation(&operation_id);
    let cancel = Arc::new(AtomicBool::new(false));
    let watcher_flag = cancel.clone();
    let watcher_token = token.clone();
    let watch = core.spawn(async move {
        watcher_token.cancelled().await;
        watcher_flag.store(true, Ordering::SeqCst);
    });

    let sender = on_event.clone();
    let result = core
        .runtime()
        .spawn_blocking(move || {
            oncode_workspace::search(&root, &query, &cancel, &|file| {
                sender(SearchEvent::Matches { file });
            })
        })
        .await;
    watch.abort();
    core.end_operation(&operation_id);

    let event = match result {
        Ok(Ok(summary)) => SearchEvent::Done { summary },
        Ok(Err(e)) => SearchEvent::Failed { message: e.message },
        Err(e) => SearchEvent::Failed { message: e.to_string() },
    };
    on_event(event);
    Ok(())
}

/// Replaces all matches in the given files (those not currently open in the editor).
pub async fn search_replace(
    core: &Shared,
    window: &str,
    query: SearchQuery,
    replacement: String,
    files: Vec<String>,
) -> AppResult<ReplaceSummary> {
    let root = core.workspace_root(window)?;
    for file in &files {
        core.resolve(window, file)?;
    }
    core.runtime()
        .spawn_blocking(move || oncode_workspace::replace_in_files(&root, &query, &replacement, &files))
        .await
        .map_err(|e| AppError::new("io", e.to_string()))?
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::api::app::{app_cancel, workspace_open};
    use crate::events::CoreEvent;
    use crate::testing::{recv, TestCore};
    use oncode_workspace::TextFile;

    async fn open(t: &TestCore, name: &str) -> PathBuf {
        let project = t.project(name);
        workspace_open(&t.core, "w1", project.to_string_lossy().into_owned()).await.unwrap();
        project
    }

    fn query(pattern: &str) -> SearchQuery {
        SearchQuery {
            pattern: pattern.into(),
            is_regex: false,
            case_sensitive: true,
            whole_word: false,
            include: Vec::new(),
            exclude: Vec::new(),
            max_results: 0,
        }
    }

    #[tokio::test]
    async fn files_are_created_saved_and_read_inside_the_project() {
        let t = TestCore::new();
        assert_eq!(fs_read_dir(&t.core, "w1", ".".into()).await.unwrap_err().code, "noWorkspace");
        let project = open(&t, "project").await;

        fs_create_dir(&t.core, "w1", "src".into()).await.unwrap();
        fs_create_file(&t.core, "w1", "src/a.txt".into()).await.unwrap();
        let stat = fs_save(&t.core, "w1", "src/a.txt".into(), "hello\nworld\n".into(), "UTF-8".into(), false, Eol::Crlf).await.unwrap();
        assert_eq!(std::fs::read(project.join("src/a.txt")).unwrap(), b"hello\r\nworld\r\n");
        assert_eq!(fs_stat(&t.core, "w1", "src/a.txt".into()).await.unwrap(), Some(stat));
        assert_eq!(fs_stat(&t.core, "w1", "src/does-not-exist".into()).await.unwrap(), None);

        // An absolute path inside the project is also accepted.
        let absolute = project.join("src/a.txt").to_string_lossy().into_owned();
        match fs_open(&t.core, "w1", absolute).await.unwrap() {
            OpenedFile::Text(TextFile { text, eol, bom, .. }) => assert_eq!((text.as_str(), eol, bom), ("hello\nworld\n", Eol::Crlf, false)),
            other => panic!("expected a text file: {other:?}"),
        }

        fs_rename(&t.core, "w1", "src/a.txt".into(), "src/b.txt".into()).await.unwrap();
        let names: Vec<String> = fs_read_dir(&t.core, "w1", "src".into()).await.unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["b.txt"]);
    }

    #[tokio::test]
    async fn paths_outside_the_project_are_refused() {
        let t = TestCore::new();
        let project = open(&t, "project").await;
        let outside = t.dir.path().join("outside.txt");
        std::fs::write(&outside, "secret").unwrap();
        let outside = outside.to_string_lossy().into_owned();

        assert_eq!(fs_open(&t.core, "w1", outside.clone()).await.unwrap_err().code, "invalidPath");
        assert_eq!(fs_open(&t.core, "w1", "../outside.txt".into()).await.unwrap_err().code, "invalidPath");
        assert_eq!(fs_save(&t.core, "w1", outside.clone(), "x".into(), "UTF-8".into(), false, Eol::Lf).await.unwrap_err().code, "invalidPath");
        assert_eq!(fs_create_file(&t.core, "w1", "../new.txt".into()).await.unwrap_err().code, "invalidPath");
        assert_eq!(fs_rename(&t.core, "w1", "a.txt".into(), outside.clone()).await.unwrap_err().code, "invalidPath");
        assert_eq!(fs_trash(&t.core, "w1", vec![outside.clone()]).await.unwrap_err().code, "invalidPath");
        assert_eq!(search_replace(&t.core, "w1", query("a"), "b".into(), vec![outside]).await.unwrap_err().code, "invalidPath");
        // The project folder itself cannot be thrown away.
        let err = fs_trash(&t.core, "w1", vec![project.to_string_lossy().into_owned()]).await.unwrap_err();
        assert_eq!(err.code, "invalidInput");
        assert_eq!(std::fs::read_to_string(t.dir.path().join("outside.txt")).unwrap(), "secret");
    }

    #[tokio::test]
    async fn dropped_files_are_copied_from_anywhere_but_only_into_the_project() {
        let t = TestCore::new();
        let project = open(&t, "project").await;
        fs_create_dir(&t.core, "w1", "src".into()).await.unwrap();
        let outside = t.dir.path().join("outside");
        std::fs::create_dir_all(outside.join("assets")).unwrap();
        std::fs::write(outside.join("assets/logo.txt"), "logo").unwrap();
        std::fs::write(outside.join("notes.txt"), "notes").unwrap();
        let outside_path = |name: &str| outside.join(name).to_string_lossy().into_owned();

        // The destination may be relative or absolute; the sources are outside the project.
        let copied = fs_copy_into(&t.core, "w1", vec![outside_path("notes.txt"), outside_path("assets")], "src".into()).await.unwrap();
        // Compared as `Path`: `join("src/x")` uses mixed separators on Windows.
        let copied: Vec<PathBuf> = copied.into_iter().map(PathBuf::from).collect();
        assert_eq!(copied, [project.join("src").join("notes.txt"), project.join("src").join("assets")]);
        assert_eq!(std::fs::read(project.join("src/assets/logo.txt")).unwrap(), b"logo");
        assert_eq!(std::fs::read(outside.join("notes.txt")).unwrap(), b"notes", "the source is untouched");

        // Does not overwrite an existing copy.
        let err = fs_copy_into(&t.core, "w1", vec![outside_path("notes.txt")], project.join("src").to_string_lossy().into_owned()).await.unwrap_err();
        assert_eq!(err.code, "alreadyExists");

        // A destination outside the project is refused, and nothing is copied.
        let elsewhere = t.dir.path().join("other");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let err = fs_copy_into(&t.core, "w1", vec![outside_path("notes.txt")], elsewhere.to_string_lossy().into_owned()).await.unwrap_err();
        assert_eq!(err.code, "invalidPath");
        assert!(!elsewhere.join("notes.txt").exists());
        assert_eq!(fs_copy_into(&t.core, "w1", vec![outside_path("notes.txt")], "../other".into()).await.unwrap_err().code, "invalidPath");
    }

    #[tokio::test]
    async fn settings_files_can_be_opened_from_any_project() {
        let t = TestCore::new();
        open(&t, "project").await;
        let path = crate::api::app::keybindings_ensure(&t.core).await.unwrap();
        assert!(matches!(fs_open(&t.core, "w1", path).await.unwrap(), OpenedFile::Text(_)));
    }

    #[tokio::test]
    async fn search_streams_matches_then_a_summary_and_replace_rewrites_files() {
        let t = TestCore::new();
        let project = t.project("project");
        std::fs::write(project.join("a.txt"), "one needle\ntwo\n").unwrap();
        std::fs::write(project.join("b.txt"), "needle needle\n").unwrap();
        std::fs::write(project.join("c.txt"), "empty\n").unwrap();
        workspace_open(&t.core, "w1", project.to_string_lossy().into_owned()).await.unwrap();

        let events: Arc<Mutex<Vec<SearchEvent>>> = Arc::default();
        let sink = events.clone();
        search_start(&t.core, "w1", query("needle"), "search-1".into(), Arc::new(move |e| sink.lock().unwrap().push(e))).await.unwrap();
        let mut events = std::mem::take(&mut *events.lock().unwrap());
        // The last message is always the summary; before it, one message per matching file.
        match events.pop() {
            Some(SearchEvent::Done { summary }) => assert_eq!((summary.files, summary.matches, summary.truncated), (2, 3, false)),
            other => panic!("expected Done: {other:?}"),
        }
        let mut files: Vec<(String, usize)> = events
            .into_iter()
            .map(|e| match e {
                SearchEvent::Matches { file } => (file.path, file.matches.len()),
                other => panic!("expected Matches: {other:?}"),
            })
            .collect();
        files.sort();
        assert_eq!(files, [("a.txt".to_string(), 1), ("b.txt".to_string(), 2)]);

        // A finished operation is no longer registered: canceling it has no effect.
        app_cancel(&t.core, "search-1".into()).await.unwrap();

        let summary = search_replace(&t.core, "w1", query("needle"), "thread".into(), vec!["b.txt".into()]).await.unwrap();
        assert_eq!((summary.files, summary.replacements), (1, 2));
        assert_eq!(std::fs::read_to_string(project.join("b.txt")).unwrap(), "thread thread\n");
        assert_eq!(std::fs::read_to_string(project.join("a.txt")).unwrap(), "one needle\ntwo\n");
    }

    #[tokio::test]
    async fn quick_open_uses_the_index_and_follows_external_changes() {
        let mut t = TestCore::new();
        let project = t.project("project");
        std::fs::create_dir(project.join("src")).unwrap();
        std::fs::write(project.join("src/main.rs"), "").unwrap();
        workspace_open(&t.core, "w1", project.to_string_lossy().into_owned()).await.unwrap();

        let paths = |items: Vec<QuickOpenItem>| items.into_iter().map(|i| i.path).collect::<Vec<_>>();
        assert_eq!(paths(quick_open(&t.core, "w1", "main".into(), 10).await.unwrap()), ["src/main.rs"]);
        assert_eq!(quick_open(&t.core, "other", "main".into(), 10).await.unwrap_err().code, "noWorkspace");

        // A file created from outside the app: the watcher broadcasts `FsChanged` to the window
        // that owns the folder and the index is updated as well.
        let added = project.join("src/extra.rs");
        std::fs::write(&added, "").unwrap();
        let window = recv(&mut t.events, |e| match e {
            CoreEvent::FsChanged { window, paths, overflow } if overflow || paths.iter().any(|p| std::path::Path::new(p) == added) => Some(window),
            _ => None,
        })
        .await;
        assert_eq!(window, "w1");
        assert_eq!(paths(quick_open(&t.core, "w1", "extra".into(), 10).await.unwrap()), ["src/extra.rs"]);
    }
}
