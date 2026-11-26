//! Berkas, pencarian, dan quick open. Semua path dibatasi ke folder proyek jendela pemanggil.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use oncode_workspace::{DirEntry, Eol, FileMatches, FileStat, OpenedFile, QuickOpenItem, ReplaceSummary, SearchQuery, SearchSummary};
use serde::Serialize;

use crate::api::Callback;
use crate::error::{AppError, AppResult};
use crate::state::Shared;

/// Batas ukuran berkas yang dibuka di editor.
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

/// Menyalin berkas atau folder dari mana pun (hasil seret-lepas dari OS) ke dalam folder `dest_dir`
/// di proyek. Hanya tujuannya yang dibatasi ke proyek: sumbernya dipilih pengguna lewat gestur
/// itu. Tidak menimpa; mengembalikan path hasil salinan.
pub async fn fs_copy_into(core: &Shared, window: &str, sources: Vec<String>, dest_dir: String) -> AppResult<Vec<String>> {
    let dest_dir = core.resolve(window, &dest_dir)?;
    let sources: Vec<PathBuf> = sources.into_iter().map(PathBuf::from).collect();
    let copied = oncode_workspace::copy_into(&sources, &dest_dir).await?;
    Ok(copied.into_iter().map(|path| path.to_string_lossy().into_owned()).collect())
}

/// Memindahkan berkas atau folder ke tempat sampah OS.
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

/// Pencarian teks di seluruh proyek. Hasil dialirkan lewat `on_event`; batalkan dengan `app_cancel(operation_id)`.
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

/// Mengganti semua kecocokan di berkas yang diberikan (yang tidak sedang terbuka di editor).
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
        let project = open(&t, "proyek").await;

        fs_create_dir(&t.core, "w1", "src".into()).await.unwrap();
        fs_create_file(&t.core, "w1", "src/a.txt".into()).await.unwrap();
        let stat = fs_save(&t.core, "w1", "src/a.txt".into(), "halo\ndunia\n".into(), "UTF-8".into(), false, Eol::Crlf).await.unwrap();
        assert_eq!(std::fs::read(project.join("src/a.txt")).unwrap(), b"halo\r\ndunia\r\n");
        assert_eq!(fs_stat(&t.core, "w1", "src/a.txt".into()).await.unwrap(), Some(stat));
        assert_eq!(fs_stat(&t.core, "w1", "src/tidak-ada".into()).await.unwrap(), None);

        // Path absolut di dalam proyek juga diterima.
        let absolute = project.join("src/a.txt").to_string_lossy().into_owned();
        match fs_open(&t.core, "w1", absolute).await.unwrap() {
            OpenedFile::Text(TextFile { text, eol, bom, .. }) => assert_eq!((text.as_str(), eol, bom), ("halo\ndunia\n", Eol::Crlf, false)),
            other => panic!("seharusnya berkas teks: {other:?}"),
        }

        fs_rename(&t.core, "w1", "src/a.txt".into(), "src/b.txt".into()).await.unwrap();
        let names: Vec<String> = fs_read_dir(&t.core, "w1", "src".into()).await.unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["b.txt"]);
    }

    #[tokio::test]
    async fn paths_outside_the_project_are_refused() {
        let t = TestCore::new();
        let project = open(&t, "proyek").await;
        let outside = t.dir.path().join("luar.txt");
        std::fs::write(&outside, "rahasia").unwrap();
        let outside = outside.to_string_lossy().into_owned();

        assert_eq!(fs_open(&t.core, "w1", outside.clone()).await.unwrap_err().code, "invalidPath");
        assert_eq!(fs_open(&t.core, "w1", "../luar.txt".into()).await.unwrap_err().code, "invalidPath");
        assert_eq!(fs_save(&t.core, "w1", outside.clone(), "x".into(), "UTF-8".into(), false, Eol::Lf).await.unwrap_err().code, "invalidPath");
        assert_eq!(fs_create_file(&t.core, "w1", "../baru.txt".into()).await.unwrap_err().code, "invalidPath");
        assert_eq!(fs_rename(&t.core, "w1", "a.txt".into(), outside.clone()).await.unwrap_err().code, "invalidPath");
        assert_eq!(fs_trash(&t.core, "w1", vec![outside.clone()]).await.unwrap_err().code, "invalidPath");
        assert_eq!(search_replace(&t.core, "w1", query("a"), "b".into(), vec![outside]).await.unwrap_err().code, "invalidPath");
        // Folder proyek sendiri tidak bisa dibuang.
        let err = fs_trash(&t.core, "w1", vec![project.to_string_lossy().into_owned()]).await.unwrap_err();
        assert_eq!(err.code, "invalidInput");
        assert_eq!(std::fs::read_to_string(t.dir.path().join("luar.txt")).unwrap(), "rahasia");
    }

    #[tokio::test]
    async fn dropped_files_are_copied_from_anywhere_but_only_into_the_project() {
        let t = TestCore::new();
        let project = open(&t, "proyek").await;
        fs_create_dir(&t.core, "w1", "src".into()).await.unwrap();
        let outside = t.dir.path().join("luar");
        std::fs::create_dir_all(outside.join("assets")).unwrap();
        std::fs::write(outside.join("assets/logo.txt"), "logo").unwrap();
        std::fs::write(outside.join("catatan.txt"), "catatan").unwrap();
        let outside_path = |name: &str| outside.join(name).to_string_lossy().into_owned();

        // Tujuan boleh relatif maupun absolut; sumbernya di luar proyek.
        let copied = fs_copy_into(&t.core, "w1", vec![outside_path("catatan.txt"), outside_path("assets")], "src".into()).await.unwrap();
        // Dibandingkan sebagai `Path`: `join("src/x")` memakai pemisah campuran di Windows.
        let copied: Vec<PathBuf> = copied.into_iter().map(PathBuf::from).collect();
        assert_eq!(copied, [project.join("src").join("catatan.txt"), project.join("src").join("assets")]);
        assert_eq!(std::fs::read(project.join("src/assets/logo.txt")).unwrap(), b"logo");
        assert_eq!(std::fs::read(outside.join("catatan.txt")).unwrap(), b"catatan", "sumber tidak disentuh");

        // Tidak menimpa salinan yang sudah ada.
        let err = fs_copy_into(&t.core, "w1", vec![outside_path("catatan.txt")], project.join("src").to_string_lossy().into_owned()).await.unwrap_err();
        assert_eq!(err.code, "alreadyExists");

        // Tujuan di luar proyek ditolak, dan tidak ada yang disalin.
        let elsewhere = t.dir.path().join("lain");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let err = fs_copy_into(&t.core, "w1", vec![outside_path("catatan.txt")], elsewhere.to_string_lossy().into_owned()).await.unwrap_err();
        assert_eq!(err.code, "invalidPath");
        assert!(!elsewhere.join("catatan.txt").exists());
        assert_eq!(fs_copy_into(&t.core, "w1", vec![outside_path("catatan.txt")], "../lain".into()).await.unwrap_err().code, "invalidPath");
    }

    #[tokio::test]
    async fn settings_files_can_be_opened_from_any_project() {
        let t = TestCore::new();
        open(&t, "proyek").await;
        let path = crate::api::app::keybindings_ensure(&t.core).await.unwrap();
        assert!(matches!(fs_open(&t.core, "w1", path).await.unwrap(), OpenedFile::Text(_)));
    }

    #[tokio::test]
    async fn search_streams_matches_then_a_summary_and_replace_rewrites_files() {
        let t = TestCore::new();
        let project = t.project("proyek");
        std::fs::write(project.join("a.txt"), "satu jarum\ndua\n").unwrap();
        std::fs::write(project.join("b.txt"), "jarum jarum\n").unwrap();
        std::fs::write(project.join("c.txt"), "kosong\n").unwrap();
        workspace_open(&t.core, "w1", project.to_string_lossy().into_owned()).await.unwrap();

        let events: Arc<Mutex<Vec<SearchEvent>>> = Arc::default();
        let sink = events.clone();
        search_start(&t.core, "w1", query("jarum"), "cari-1".into(), Arc::new(move |e| sink.lock().unwrap().push(e))).await.unwrap();
        let mut events = std::mem::take(&mut *events.lock().unwrap());
        // Kiriman terakhir selalu ringkasan; sebelumnya satu kiriman per berkas yang cocok.
        match events.pop() {
            Some(SearchEvent::Done { summary }) => assert_eq!((summary.files, summary.matches, summary.truncated), (2, 3, false)),
            other => panic!("seharusnya Done: {other:?}"),
        }
        let mut files: Vec<(String, usize)> = events
            .into_iter()
            .map(|e| match e {
                SearchEvent::Matches { file } => (file.path, file.matches.len()),
                other => panic!("seharusnya Matches: {other:?}"),
            })
            .collect();
        files.sort();
        assert_eq!(files, [("a.txt".to_string(), 1), ("b.txt".to_string(), 2)]);

        // Operasi yang sudah selesai tidak lagi terdaftar: membatalkannya tidak berpengaruh.
        app_cancel(&t.core, "cari-1".into()).await.unwrap();

        let summary = search_replace(&t.core, "w1", query("jarum"), "benang".into(), vec!["b.txt".into()]).await.unwrap();
        assert_eq!((summary.files, summary.replacements), (1, 2));
        assert_eq!(std::fs::read_to_string(project.join("b.txt")).unwrap(), "benang benang\n");
        assert_eq!(std::fs::read_to_string(project.join("a.txt")).unwrap(), "satu jarum\ndua\n");
    }

    #[tokio::test]
    async fn quick_open_uses_the_index_and_follows_external_changes() {
        let mut t = TestCore::new();
        let project = t.project("proyek");
        std::fs::create_dir(project.join("src")).unwrap();
        std::fs::write(project.join("src/utama.rs"), "").unwrap();
        workspace_open(&t.core, "w1", project.to_string_lossy().into_owned()).await.unwrap();

        let paths = |items: Vec<QuickOpenItem>| items.into_iter().map(|i| i.path).collect::<Vec<_>>();
        assert_eq!(paths(quick_open(&t.core, "w1", "utama".into(), 10).await.unwrap()), ["src/utama.rs"]);
        assert_eq!(quick_open(&t.core, "lain", "utama".into(), 10).await.unwrap_err().code, "noWorkspace");

        // Berkas yang dibuat dari luar aplikasi: pemantau menyiarkan `FsChanged` ke jendela
        // pemilik folder dan indeks ikut diperbarui.
        let added = project.join("src/tambahan.rs");
        std::fs::write(&added, "").unwrap();
        let window = recv(&mut t.events, |e| match e {
            CoreEvent::FsChanged { window, paths, overflow } if overflow || paths.iter().any(|p| std::path::Path::new(p) == added) => Some(window),
            _ => None,
        })
        .await;
        assert_eq!(window, "w1");
        assert_eq!(paths(quick_open(&t.core, "w1", "tambahan".into(), 10).await.unwrap()), ["src/tambahan.rs"]);
    }
}
