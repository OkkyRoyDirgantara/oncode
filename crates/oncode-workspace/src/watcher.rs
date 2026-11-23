//! Pemantau perubahan berkas di working tree dan di direktori Git.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use notify::event::ModifyKind;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use serde::Serialize;

use crate::error::{Result, WsError};
use crate::fs::{absolute, is_temp_name};
use crate::gitignore::{read_common_dir, IgnoreRules};

/// Masa tenang sebelum perubahan yang terkumpul dikirim.
const QUIET: Duration = Duration::from_millis(250);
/// Batas tunggu sejak perubahan pertama, agar aliran perubahan yang terus-menerus tetap terkirim.
const MAX_WAIT: Duration = Duration::from_secs(1);
/// Di atas jumlah path ini, satu kiriman diganti dengan `overflow: true`.
const MAX_PATHS: usize = 1000;

/// Entri tingkat atas direktori Git yang tidak pernah dilaporkan.
const GIT_NOISE: &[&str] = &["objects", "logs", "hooks", "info", "worktrees", "modules", "lfs"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WatchEvent {
    /// Berkas atau folder di working tree berubah. `paths` berisi path absolut tanpa duplikat.
    /// Bila terlalu banyak perubahan atau sistem operasi kehilangan event, `paths` kosong dan
    /// `overflow` bernilai `true`: muat ulang semuanya.
    Files { paths: Vec<String>, overflow: bool },
    /// Isi direktori Git yang dipantau berubah. `git_dir` sama persis dengan path yang
    /// diberikan pemanggil.
    ///
    /// - `refs`: `refs/`, `packed-refs`, atau `HEAD` berubah.
    /// - `index`: `index` berubah.
    /// - `state`: penanda operasi berubah (`MERGE_HEAD`, `rebase-merge`, `rebase-apply`,
    ///   `CHERRY_PICK_HEAD`, `REVERT_HEAD`).
    ///
    /// Ketiganya bisa `false` bila yang berubah berkas lain, mis. `config` atau `FETCH_HEAD`.
    #[serde(rename_all = "camelCase")]
    Git { git_dir: String, refs: bool, index: bool, state: bool },
}

/// Pemantau satu folder proyek. Pemantauan berhenti saat nilai ini di-drop.
pub struct Watcher {
    tx: mpsc::Sender<Msg>,
    thread: Option<thread::JoinHandle<()>>,
}

/// Cara memantau working tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TreeMode {
    /// Satu pemantauan rekursif dari sistem operasi (FSEvents, ReadDirectoryChangesW).
    Native,
    /// Satu pemantauan per folder yang tidak diabaikan. Dipakai di Linux, karena inotify
    /// membutuhkan satu watch per folder dan folder seperti `node_modules` atau `target`
    /// akan menghabiskan jatah watch.
    PerDirectory,
}

impl TreeMode {
    fn for_platform() -> Self {
        if cfg!(target_os = "linux") {
            TreeMode::PerDirectory
        } else {
            TreeMode::Native
        }
    }
}

enum Msg {
    Notify(notify::Result<notify::Event>),
    SetGitDirs(Vec<PathBuf>, mpsc::Sender<Result<()>>),
    Stop,
}

impl Watcher {
    /// Memantau `root` secara rekursif beserta tiap direktori Git di `git_dirs` (boleh berada
    /// di luar `root`, mis. milik worktree atau submodule).
    ///
    /// Perubahan dikumpulkan lalu dikirim setelah ±250 ms tenang, paling lambat tiap 1 detik,
    /// dari sebuah thread latar. Perubahan pada path yang diabaikan gitignore dibuang.
    /// Perubahan di dalam folder `.git` mana pun hanya dilaporkan sebagai `Git` (dan hanya
    /// untuk direktori di `git_dirs`); berkas `*.lock` dan folder `objects/` diabaikan.
    /// Dibuat atau dihapusnya entri `.git` itu sendiri dilaporkan sebagai `Files`.
    ///
    /// Untuk worktree tertaut, perubahan `refs/` dan `packed-refs` di direktori Git bersama
    /// ikut dilaporkan sebagai `refs: true` pada `git_dir` worktree tersebut.
    ///
    /// Gagal bila `root` atau salah satu `git_dirs` tidak dapat dipantau, termasuk bila batas
    /// watcher sistem operasi terlampaui.
    pub fn start(
        root: &Path,
        git_dirs: &[PathBuf],
        on_event: impl Fn(WatchEvent) + Send + Sync + 'static,
    ) -> Result<Watcher> {
        Self::start_with(root, git_dirs, Box::new(on_event), TreeMode::for_platform())
    }

    fn start_with(
        root: &Path,
        git_dirs: &[PathBuf],
        on_event: Box<dyn Fn(WatchEvent) + Send + Sync>,
        mode: TreeMode,
    ) -> Result<Watcher> {
        let root_shown = absolute(root)?;
        // Sebagian backend melaporkan path yang sudah diselesaikan symlink-nya, jadi pencocokan
        // awalan memakai path kanonis; path yang dikirim ke pemanggil tetap memakai `root` asli.
        let root_real = std::fs::canonicalize(&root_shown).map_err(|e| WsError::io(&root_shown, e))?;
        if !root_real.is_dir() {
            return Err(WsError::invalid_path(format!("Not a folder: {}", root_shown.display())));
        }

        let (tx, rx) = mpsc::channel();
        let handler_tx = tx.clone();
        let notify = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let _ = handler_tx.send(Msg::Notify(res));
        })
        .map_err(|e| watch_error(&root_shown, &e))?;

        let mut worker = Worker {
            notify,
            mode,
            rules: IgnoreRules::new(&root_real),
            root: root_real,
            root_shown,
            git_dirs: Vec::new(),
            git_watches: BTreeMap::new(),
            pending: Pending::default(),
            resync: false,
            on_event,
        };
        worker.watch_root()?;
        worker.apply_git_dirs(git_dirs)?;

        let thread = thread::Builder::new()
            .name("oncode-watcher".to_string())
            .spawn(move || worker.run(rx))
            .map_err(|e| WsError::other(format!("Failed to start the watcher thread: {e}")))?;
        Ok(Watcher { tx, thread: Some(thread) })
    }

    /// Mengganti himpunan direktori Git yang dipantau.
    ///
    /// Direktori yang gagal dipantau dilewati dan galat pertama dikembalikan; direktori lain
    /// tetap berlaku. Jangan dipanggil dari dalam callback `on_event`.
    pub fn set_git_dirs(&self, git_dirs: &[PathBuf]) -> Result<()> {
        if self.thread.as_ref().is_some_and(|t| t.thread().id() == thread::current().id()) {
            return Err(WsError::other("set_git_dirs must not be called from the watcher callback"));
        }
        let stopped = || WsError::other("Watcher has stopped");
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx.send(Msg::SetGitDirs(git_dirs.to_vec(), reply_tx)).map_err(|_| stopped())?;
        reply_rx.recv().map_err(|_| stopped())?
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Stop);
        if let Some(thread) = self.thread.take() {
            // Di-drop dari dalam callback: thread akan berhenti sendiri setelah callback selesai.
            if thread.thread().id() != thread::current().id() {
                let _ = thread.join();
            }
        }
    }
}

struct GitDir {
    /// Path persis seperti yang diberikan pemanggil; dipakai di `WatchEvent::Git`.
    shown: String,
    dir: PathBuf,
    /// Direktori Git bersama, bila `dir` milik worktree tertaut.
    common: Option<PathBuf>,
}

#[derive(Default, Clone, Copy)]
struct GitFlags {
    refs: bool,
    index: bool,
    state: bool,
}

#[derive(Default)]
struct Pending {
    /// Path relatif terhadap akar proyek.
    files: BTreeSet<PathBuf>,
    overflow: bool,
    git: BTreeMap<String, GitFlags>,
    /// Disetel tiap kali ada perubahan baru yang tercatat; dipakai untuk mengatur masa tenang.
    touched: bool,
}

impl Pending {
    fn add_file(&mut self, rel: PathBuf) {
        self.touched = true;
        if self.overflow {
            return;
        }
        self.files.insert(rel);
        if self.files.len() > MAX_PATHS {
            self.set_overflow();
        }
    }

    fn set_overflow(&mut self) {
        self.touched = true;
        self.overflow = true;
        self.files.clear();
    }
}

struct Worker {
    notify: RecommendedWatcher,
    mode: TreeMode,
    /// Akar proyek kanonis; awalan semua path event working tree.
    root: PathBuf,
    /// Akar proyek seperti yang diberikan pemanggil; awalan path yang dilaporkan.
    root_shown: PathBuf,
    rules: IgnoreRules,
    git_dirs: Vec<GitDir>,
    /// Path yang dipantau demi direktori Git → apakah rekursif.
    git_watches: BTreeMap<PathBuf, bool>,
    pending: Pending,
    /// `.gitignore` berubah: folder yang tadinya diabaikan mungkin perlu dipantau.
    resync: bool,
    on_event: Box<dyn Fn(WatchEvent) + Send + Sync>,
}

impl Worker {
    fn run(mut self, rx: mpsc::Receiver<Msg>) {
        // Waktu perubahan pertama dan terakhir dalam kumpulan yang belum dikirim.
        let mut window: Option<(Instant, Instant)> = None;
        loop {
            let msg = match window {
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                Some((first, last)) => {
                    let deadline = (last + QUIET).min(first + MAX_WAIT);
                    let now = Instant::now();
                    // Diperiksa lebih dulu: `recv_timeout` tetap mengembalikan pesan yang sudah
                    // mengantre, sehingga banjir event bisa menunda pengiriman tanpa batas.
                    if now >= deadline {
                        Err(RecvTimeoutError::Timeout)
                    } else {
                        rx.recv_timeout(deadline - now)
                    }
                }
            };
            match msg {
                Ok(Msg::Notify(result)) => {
                    self.on_notify(result);
                    if std::mem::take(&mut self.pending.touched) {
                        let now = Instant::now();
                        window = Some((window.map_or(now, |(first, _)| first), now));
                    }
                }
                Ok(Msg::SetGitDirs(dirs, reply)) => {
                    let _ = reply.send(self.apply_git_dirs(&dirs));
                }
                Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {
                    self.flush();
                    window = None;
                }
            }
        }
    }

    fn flush(&mut self) {
        if std::mem::take(&mut self.resync) && self.mode == TreeMode::PerDirectory {
            let root = self.root.clone();
            if self.watch_tree(&root, false).is_err() {
                self.pending.set_overflow();
            }
        }
        let pending = std::mem::take(&mut self.pending);
        if pending.overflow {
            (self.on_event)(WatchEvent::Files { paths: Vec::new(), overflow: true });
        } else if !pending.files.is_empty() {
            let paths = pending.files.iter().map(|rel| self.root_shown.join(rel).to_string_lossy().into_owned()).collect();
            (self.on_event)(WatchEvent::Files { paths, overflow: false });
        }
        for (git_dir, flags) in pending.git {
            (self.on_event)(WatchEvent::Git { git_dir, refs: flags.refs, index: flags.index, state: flags.state });
        }
    }

    fn on_notify(&mut self, result: notify::Result<notify::Event>) {
        let event = match result {
            Ok(event) => event,
            // Backend kehilangan jejak (mis. batas inotify tercapai): minta pemuatan ulang.
            Err(_) => return self.pending.set_overflow(),
        };
        if event.need_rescan() {
            self.pending.set_overflow();
        }
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        // Entri dibuat, dihapus, atau berganti nama (bukan sekadar isinya berubah).
        let structural = !matches!(event.kind, EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Metadata(_)));
        for path in &event.paths {
            self.on_path(path, structural);
        }
    }

    fn on_path(&mut self, path: &Path, structural: bool) {
        if self.on_git_path(path) {
            return;
        }
        let Ok(rel) = path.strip_prefix(&self.root) else { return };
        let Some(name) = rel.file_name() else { return };

        if let Some(position) = rel.components().position(|c| c.as_os_str() == ".git") {
            // Isi `.git` bukan bagian working tree. Entri `.git` itu sendiri dilaporkan saat
            // dibuat atau dihapus, agar repositori baru (`git init`) bisa dikenali.
            if structural && position + 1 == rel.components().count() {
                self.pending.add_file(rel.to_path_buf());
            }
            return;
        }
        if is_temp_name(&name.to_string_lossy()) {
            return;
        }

        if name == ".gitignore" {
            let parent_ignored = path.parent().is_some_and(|p| p != self.root && self.rules.is_ignored(p, true));
            if !parent_ignored {
                self.rules.invalidate();
                self.resync = true;
            }
        }
        let is_dir = std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir());
        if self.rules.is_ignored(path, is_dir) {
            return;
        }
        self.pending.add_file(rel.to_path_buf());

        if self.mode == TreeMode::PerDirectory && is_dir && structural {
            // Folder baru (atau dipindahkan ke sini): pantau, dan laporkan isinya karena berkas
            // yang dibuat sebelum watch terpasang tidak menghasilkan event.
            match self.watch_tree(path, true) {
                Ok(found) => found.into_iter().for_each(|rel| self.pending.add_file(rel)),
                Err(_) => self.pending.set_overflow(),
            }
        }
    }

    /// Mencatat perubahan di dalam direktori Git yang dipantau. `false` bila `path` bukan
    /// bagian dari direktori Git mana pun.
    fn on_git_path(&mut self, path: &Path) -> bool {
        let mut base: Option<&PathBuf> = None;
        for git in &self.git_dirs {
            for candidate in std::iter::once(&git.dir).chain(git.common.as_ref()) {
                let longer = base.is_none_or(|b| candidate.as_os_str().len() > b.as_os_str().len());
                if longer && path.starts_with(candidate) {
                    base = Some(candidate);
                }
            }
        }
        let Some(base) = base.cloned() else { return false };
        let Ok(rel) = path.strip_prefix(&base) else { return false };
        let mut components = rel.components();
        // Direktori Git itu sendiri (dibuat/dihapus) ditangani sebagai entri working tree.
        let Some(first) = components.next() else { return false };
        let first = first.as_os_str().to_string_lossy();
        let first = first.as_ref();
        let nested = components.next().is_some();

        if rel.file_name().is_some_and(|n| n.to_string_lossy().ends_with(".lock")) {
            return true;
        }
        let in_refs = matches!(first, "refs" | "reftable");
        let in_rebase = matches!(first, "rebase-merge" | "rebase-apply");
        if GIT_NOISE.contains(&first) || (nested && !in_refs && !in_rebase) {
            return true;
        }
        let shared_refs = in_refs || (!nested && first == "packed-refs");
        let flags = GitFlags {
            refs: shared_refs || (!nested && first == "HEAD"),
            index: !nested && first == "index",
            state: in_rebase || (!nested && matches!(first, "MERGE_HEAD" | "CHERRY_PICK_HEAD" | "REVERT_HEAD")),
        };

        for git in &self.git_dirs {
            if git.dir == base {
                let entry = self.pending.git.entry(git.shown.clone()).or_default();
                entry.refs |= flags.refs;
                entry.index |= flags.index;
                entry.state |= flags.state;
                self.pending.touched = true;
            } else if git.common.as_ref() == Some(&base) && shared_refs {
                // Worktree tertaut berbagi ref dengan repositori induknya; `HEAD`, `index`,
                // dan penanda operasi di direktori bersama adalah milik worktree utama.
                self.pending.git.entry(git.shown.clone()).or_default().refs = true;
                self.pending.touched = true;
            }
        }
        true
    }

    fn watch_root(&mut self) -> Result<()> {
        let root = self.root.clone();
        match self.mode {
            TreeMode::Native => self.notify.watch(&root, RecursiveMode::Recursive),
            TreeMode::PerDirectory => self.watch_tree(&root, false).map(|_| ()),
        }
        .map_err(|e| watch_error(&self.root_shown, &e))
    }

    /// Memasang watch pada `top` dan semua folder di bawahnya yang tidak diabaikan.
    /// Bila `collect`, mengembalikan entri yang ditemukan (relatif terhadap akar proyek).
    ///
    /// Gagal bila `top` sendiri tidak dapat dipantau atau batas watcher terlampaui; subfolder
    /// lain yang gagal (mis. sudah terhapus) dilewati.
    fn watch_tree(&mut self, top: &Path, collect: bool) -> notify::Result<Vec<PathBuf>> {
        let mut found = Vec::new();
        let mut stack = vec![top.to_path_buf()];
        let mut is_top = true;
        while let Some(dir) = stack.pop() {
            // Watch dipasang sebelum isi folder dibaca, supaya tidak ada celah di antaranya.
            let watched = self.notify.watch(&dir, RecursiveMode::NonRecursive);
            if let Err(err) = watched {
                if is_top || matches!(err.kind, notify::ErrorKind::MaxFilesWatch) {
                    return Err(err);
                }
                continue;
            }
            is_top = false;
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let name = entry.file_name();
                if name == ".git" || is_temp_name(&name.to_string_lossy()) {
                    continue;
                }
                let Ok(file_type) = entry.file_type() else { continue };
                let path = dir.join(&name);
                if self.rules.is_entry_ignored(&dir, &path, file_type.is_dir()) {
                    continue;
                }
                if collect && found.len() <= MAX_PATHS {
                    if let Ok(rel) = path.strip_prefix(&self.root) {
                        found.push(rel.to_path_buf());
                    }
                }
                if file_type.is_dir() {
                    stack.push(path);
                }
            }
        }
        Ok(found)
    }

    /// Benar bila `path` adalah folder working tree, yang watch-nya tidak boleh dilepas saat
    /// himpunan direktori Git berubah.
    fn is_tree_path(&self, path: &Path) -> bool {
        path.strip_prefix(&self.root).is_ok_and(|rel| rel.components().all(|c| c.as_os_str() != ".git"))
    }

    fn apply_git_dirs(&mut self, dirs: &[PathBuf]) -> Result<()> {
        let mut first_error: Option<WsError> = None;
        let mut git_dirs = Vec::with_capacity(dirs.len());
        let mut wanted: BTreeMap<PathBuf, bool> = BTreeMap::new();
        for dir in dirs {
            let real = match std::fs::canonicalize(dir) {
                Ok(real) if real.is_dir() => real,
                Ok(_) => {
                    first_error.get_or_insert_with(|| WsError::invalid_path(format!("Not a Git directory: {}", dir.display())));
                    continue;
                }
                Err(err) => {
                    first_error.get_or_insert_with(|| WsError::io(dir, err));
                    continue;
                }
            };
            let common = read_common_dir(&real).and_then(|c| std::fs::canonicalize(c).ok()).filter(|c| *c != real);
            // Berkas kunci (`HEAD`, `index`, `packed-refs`, penanda operasi) adalah anak langsung;
            // hanya `refs/` yang perlu dipantau sampai ke dalam. `objects/` tidak dipantau.
            for base in std::iter::once(&real).chain(common.as_ref()) {
                wanted.entry(base.clone()).or_insert(false);
                for sub in ["refs", "reftable"] {
                    let path = base.join(sub);
                    if path.is_dir() {
                        wanted.insert(path, true);
                    }
                }
            }
            git_dirs.push(GitDir { shown: dir.to_string_lossy().into_owned(), dir: real, common });
        }

        let stale: Vec<PathBuf> = self.git_watches.keys().filter(|p| !wanted.contains_key(*p)).cloned().collect();
        for path in stale {
            self.git_watches.remove(&path);
            if !self.is_tree_path(&path) {
                let _ = self.notify.unwatch(&path);
            }
        }
        for (path, recursive) in wanted {
            if self.git_watches.contains_key(&path) {
                continue;
            }
            let mode = if recursive { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
            match self.notify.watch(&path, mode) {
                Ok(()) => {
                    self.git_watches.insert(path, recursive);
                }
                Err(err) => {
                    first_error.get_or_insert_with(|| watch_error(&path, &err));
                }
            }
        }
        self.git_dirs = git_dirs;
        match first_error {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }
}

fn watch_error(path: &Path, err: &notify::Error) -> WsError {
    match &err.kind {
        notify::ErrorKind::PathNotFound => WsError::not_found(path),
        notify::ErrorKind::Io(io) => WsError::io(path, std::io::Error::new(io.kind(), io.to_string())),
        notify::ErrorKind::MaxFilesWatch => WsError::other(
            "The operating system's file watcher limit was reached; file changes cannot be watched. \
             On Linux, increase fs.inotify.max_user_watches.",
        ),
        _ => WsError::other(format!("Failed to watch {}: {err}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::WsErrorCode;
    use std::fs;
    use std::process::Command;
    use std::sync::{Arc, Mutex};

    const TIMEOUT: Duration = Duration::from_secs(3);
    /// Cukup lama untuk memastikan sebuah event memang tidak datang (di atas masa tenang).
    const SETTLE: Duration = Duration::from_millis(700);

    #[derive(Clone, Default)]
    struct Events(Arc<Mutex<Vec<WatchEvent>>>);

    impl Events {
        fn sink(&self) -> Box<dyn Fn(WatchEvent) + Send + Sync> {
            let events = self.0.clone();
            Box::new(move |event| events.lock().unwrap().push(event))
        }

        fn all(&self) -> Vec<WatchEvent> {
            self.0.lock().unwrap().clone()
        }

        fn clear(&self) {
            self.0.lock().unwrap().clear();
        }

        /// Semua path yang pernah dilaporkan lewat `Files`.
        fn paths(&self) -> Vec<String> {
            self.all()
                .into_iter()
                .flat_map(|e| match e {
                    WatchEvent::Files { paths, .. } => paths,
                    WatchEvent::Git { .. } => Vec::new(),
                })
                .collect()
        }

        fn has_path(&self, path: &Path) -> bool {
            self.paths().iter().any(|p| Path::new(p) == path)
        }

        /// Gabungan flag semua event `Git` untuk `git_dir`; `None` bila belum ada.
        fn git(&self, git_dir: &Path) -> Option<(bool, bool, bool)> {
            let mut merged = None;
            for event in self.all() {
                if let WatchEvent::Git { git_dir: dir, refs, index, state } = event {
                    if Path::new(&dir) == git_dir {
                        let (r, i, s) = merged.unwrap_or((false, false, false));
                        merged = Some((r || refs, i || index, s || state));
                    }
                }
            }
            merged
        }

        fn wait(&self, what: &str, check: impl Fn(&Events) -> bool) {
            let start = Instant::now();
            while start.elapsed() < TIMEOUT {
                if check(self) {
                    return;
                }
                thread::sleep(Duration::from_millis(20));
            }
            panic!("tidak terjadi dalam {TIMEOUT:?}: {what}; event: {:#?}", self.all());
        }

        fn wait_path(&self, path: &Path) {
            self.wait(&format!("event Files untuk {}", path.display()), |e| e.has_path(path));
        }
    }

    fn start(root: &Path, git_dirs: &[PathBuf], mode: TreeMode) -> (Watcher, Events) {
        let events = Events::default();
        let watcher = Watcher::start_with(root, git_dirs, events.sink(), mode).unwrap();
        (watcher, events)
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn git(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(dir)
            .args(["-c", "user.name=Tes", "-c", "user.email=tes@example.com", "-c", "init.defaultBranch=main"])
            .args(["-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null"])
            .args(args)
            // Konfigurasi Git milik mesin yang menjalankan tes tidak boleh ikut berpengaruh.
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" })
            .output()
            .expect("git harus terpasang untuk tes ini");
        assert!(output.status.success(), "git {args:?} gagal: {}", String::from_utf8_lossy(&output.stderr));
    }

    /// Folder sementara dengan path kanonis, agar path yang dilaporkan mudah dibandingkan.
    fn temp_root() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        (tmp, root)
    }

    const MODES: [TreeMode; 2] = [TreeMode::PerDirectory, TreeMode::Native];

    #[test]
    fn reports_created_and_modified_files() {
        for mode in MODES {
            let (_tmp, root) = temp_root();
            write(&root.join("src/existing.rs"), "v1");
            let (_watcher, events) = start(&root, &[], mode);

            let created = root.join("baru.txt");
            write(&created, "halo");
            events.wait_path(&created);

            events.clear();
            let existing = root.join("src/existing.rs");
            write(&existing, "v2");
            events.wait_path(&existing);
            assert!(events.all().iter().all(|e| matches!(e, WatchEvent::Files { overflow: false, .. })), "{mode:?}");

            events.clear();
            fs::remove_file(&created).unwrap();
            events.wait_path(&created);
        }
    }

    #[test]
    fn rapid_changes_are_batched_and_deduplicated() {
        let (_tmp, root) = temp_root();
        let (_watcher, events) = start(&root, &[], TreeMode::for_platform());
        let file = root.join("a.txt");
        for i in 0..20 {
            write(&file, &format!("isi {i}"));
        }
        write(&root.join("b.txt"), "x");
        events.wait_path(&root.join("b.txt"));
        thread::sleep(SETTLE);

        // 21 penulisan menjadi segelintir kiriman (biasanya satu), tiap path sekali per kiriman.
        let all = events.all();
        assert!(all.len() <= 3, "{all:#?}");
        let expected = vec![file.to_string_lossy().into_owned(), root.join("b.txt").to_string_lossy().into_owned()];
        assert_eq!(all[0], WatchEvent::Files { paths: expected.clone(), overflow: false });
        for event in &all {
            let WatchEvent::Files { paths, overflow: false } = event else { panic!("{event:?}") };
            assert!(paths.iter().all(|p| expected.contains(p)) && paths.len() <= 2, "{all:#?}");
        }
    }

    #[test]
    fn ignored_paths_produce_no_events() {
        for mode in MODES {
            let (_tmp, root) = temp_root();
            write(&root.join(".gitignore"), "target/\n*.log\n");
            write(&root.join("target/debug/old"), "");
            write(&root.join("src/.gitignore"), "/generated\n");
            write(&root.join("src/lib.rs"), "");
            let (_watcher, events) = start(&root, &[], mode);

            write(&root.join("target/debug/new.bin"), "x");
            write(&root.join("target/fresh/deep/file"), "x");
            write(&root.join("debug.log"), "x");
            write(&root.join("src/trace.log"), "x");
            write(&root.join("src/generated/out.rs"), "x");
            let visible = root.join("src/visible.rs");
            write(&visible, "x");
            events.wait_path(&visible);
            thread::sleep(SETTLE);

            let paths = events.paths();
            assert_eq!(paths, [visible.to_string_lossy().into_owned()], "{mode:?}");
        }
    }

    #[test]
    fn gitignore_changes_take_effect() {
        let (_tmp, root) = temp_root();
        write(&root.join("cache/old"), "");
        let (_watcher, events) = start(&root, &[], TreeMode::for_platform());

        // Sebelum diabaikan, perubahan di `cache/` dilaporkan.
        write(&root.join("cache/a"), "1");
        events.wait_path(&root.join("cache/a"));

        write(&root.join(".gitignore"), "cache/\n");
        events.wait_path(&root.join(".gitignore"));
        events.clear();
        write(&root.join("cache/b"), "1");
        write(&root.join("marker1"), "1");
        events.wait_path(&root.join("marker1"));
        thread::sleep(SETTLE);
        assert!(!events.has_path(&root.join("cache/b")), "{:#?}", events.all());

        // Setelah aturan dicabut, folder yang tadinya diabaikan kembali dipantau.
        write(&root.join("build/x"), "1");
        write(&root.join(".gitignore"), "build/\n");
        events.wait_path(&root.join(".gitignore"));
        thread::sleep(SETTLE);
        events.clear();
        write(&root.join("cache/c"), "1");
        events.wait_path(&root.join("cache/c"));
        assert!(!events.has_path(&root.join("build/x")));
    }

    #[test]
    fn new_directories_are_watched_and_their_contents_reported() {
        for mode in MODES {
            let (_tmp, root) = temp_root();
            let (_watcher, events) = start(&root, &[], mode);

            // Folder dan isinya dibuat sekaligus, sebelum watch folder baru sempat terpasang.
            write(&root.join("pkg/src/deep/mod.rs"), "x");
            write(&root.join("pkg/README.md"), "x");
            events.wait_path(&root.join("pkg"));
            if mode == TreeMode::PerDirectory {
                events.wait_path(&root.join("pkg/src/deep/mod.rs"));
                events.wait_path(&root.join("pkg/README.md"));
            }

            thread::sleep(SETTLE);
            events.clear();
            let later = root.join("pkg/src/deep/later.rs");
            write(&later, "x");
            events.wait_path(&later);

            // Folder yang dipindahkan tetap terpantau di lokasi barunya.
            events.clear();
            fs::rename(root.join("pkg"), root.join("crate")).unwrap();
            events.wait_path(&root.join("crate"));
            thread::sleep(SETTLE);
            events.clear();
            let moved = root.join("crate/src/deep/after-move.rs");
            write(&moved, "x");
            events.wait_path(&moved);
            assert!(events.paths().iter().all(|p| !p.contains("/pkg/")), "{mode:?}: {:#?}", events.all());
        }
    }

    #[test]
    fn too_many_changes_overflow() {
        let (_tmp, root) = temp_root();
        let (_watcher, events) = start(&root, &[], TreeMode::for_platform());
        for i in 0..(MAX_PATHS + 200) {
            fs::write(root.join(format!("f{i}.txt")), "x").unwrap();
        }
        events.wait("event overflow", |e| {
            e.all().iter().any(|ev| matches!(ev, WatchEvent::Files { paths, overflow: true } if paths.is_empty()))
        });
    }

    #[test]
    fn git_index_refs_and_state_changes() {
        for mode in MODES {
            let (_tmp, root) = temp_root();
            git(&root, &["init", "-q"]);
            let git_dir = root.join(".git");
            let (_watcher, events) = start(&root, std::slice::from_ref(&git_dir), mode);

            write(&git_dir.join("index"), "indeks palsu");
            events.wait("Git{index}", |e| e.git(&git_dir) == Some((false, true, false)));

            events.clear();
            write(&git_dir.join("refs/heads/x"), "0000000000000000000000000000000000000000\n");
            events.wait("Git{refs} dari refs/heads/x", |e| e.git(&git_dir) == Some((true, false, false)));

            // Folder ref baru (branch bernama `fitur/a`) ikut terpantau.
            events.clear();
            write(&git_dir.join("refs/heads/fitur/a"), "0000000000000000000000000000000000000000\n");
            events.wait("Git{refs} dari refs/heads/fitur/a", |e| e.git(&git_dir) == Some((true, false, false)));

            events.clear();
            write(&git_dir.join("packed-refs"), "# pack-refs\n");
            events.wait("Git{refs} dari packed-refs", |e| e.git(&git_dir) == Some((true, false, false)));

            events.clear();
            write(&git_dir.join("HEAD"), "ref: refs/heads/lain\n");
            events.wait("Git{refs} dari HEAD", |e| e.git(&git_dir) == Some((true, false, false)));

            events.clear();
            write(&git_dir.join("MERGE_HEAD"), "0000000000000000000000000000000000000000\n");
            events.wait("Git{state} dari MERGE_HEAD", |e| e.git(&git_dir) == Some((false, false, true)));

            events.clear();
            fs::create_dir(git_dir.join("rebase-merge")).unwrap();
            events.wait("Git{state} dari rebase-merge", |e| e.git(&git_dir) == Some((false, false, true)));

            // Berkas lain di direktori Git: event tanpa flag.
            events.clear();
            write(&git_dir.join("FETCH_HEAD"), "x\n");
            events.wait("Git tanpa flag", |e| e.git(&git_dir) == Some((false, false, false)));

            // Tidak ada satu pun yang bocor sebagai perubahan working tree.
            thread::sleep(SETTLE);
            assert_eq!(events.paths(), Vec::<String>::new(), "{mode:?}");
        }
    }

    #[test]
    fn git_lock_files_and_objects_are_ignored() {
        for mode in MODES {
            let (_tmp, root) = temp_root();
            git(&root, &["init", "-q"]);
            let git_dir = root.join(".git");
            let (_watcher, events) = start(&root, std::slice::from_ref(&git_dir), mode);

            write(&git_dir.join("index.lock"), "x");
            write(&git_dir.join("HEAD.lock"), "x");
            write(&git_dir.join("refs/heads/main.lock"), "x");
            write(&git_dir.join("objects/ab/cdef0123"), "x");
            write(&git_dir.join("objects/pack/pack-1.pack"), "x");
            write(&git_dir.join("logs/HEAD"), "x");
            let marker = root.join("marker.txt");
            write(&marker, "x");
            events.wait_path(&marker);
            thread::sleep(SETTLE);

            assert_eq!(events.git(&git_dir), None, "{mode:?}: {:#?}", events.all());
            assert_eq!(events.paths(), [marker.to_string_lossy().into_owned()], "{mode:?}");
        }
    }

    #[test]
    fn real_git_commands_are_classified() {
        let (_tmp, root) = temp_root();
        git(&root, &["init", "-q"]);
        let git_dir = root.join(".git");
        write(&root.join("a.txt"), "satu\n");
        let (_watcher, events) = start(&root, std::slice::from_ref(&git_dir), TreeMode::for_platform());

        git(&root, &["add", "a.txt"]);
        events.wait("git add mengubah index", |e| e.git(&git_dir).is_some_and(|(_, index, _)| index));

        events.clear();
        git(&root, &["commit", "-q", "-m", "awal"]);
        events.wait("git commit mengubah refs", |e| e.git(&git_dir).is_some_and(|(refs, _, _)| refs));

        thread::sleep(SETTLE);
        events.clear();
        git(&root, &["branch", "fitur"]);
        events.wait("git branch mengubah refs", |e| e.git(&git_dir) == Some((true, false, false)));
        thread::sleep(SETTLE);
        assert_eq!(events.paths(), Vec::<String>::new());
    }

    #[test]
    fn unwatched_git_dirs_are_silent_until_set() {
        let (_tmp, root) = temp_root();
        git(&root, &["init", "-q"]);
        let git_dir = root.join(".git");
        let (watcher, events) = start(&root, &[], TreeMode::for_platform());

        write(&git_dir.join("index"), "1");
        let marker = root.join("marker1");
        write(&marker, "x");
        events.wait_path(&marker);
        thread::sleep(SETTLE);
        assert_eq!(events.all().len(), 1, "{:#?}", events.all());

        watcher.set_git_dirs(std::slice::from_ref(&git_dir)).unwrap();
        events.clear();
        write(&git_dir.join("index"), "2");
        events.wait("Git{index} setelah set_git_dirs", |e| e.git(&git_dir) == Some((false, true, false)));

        watcher.set_git_dirs(&[]).unwrap();
        thread::sleep(SETTLE);
        events.clear();
        write(&git_dir.join("index"), "3");
        write(&git_dir.join("refs/heads/y"), "3");
        let marker = root.join("marker2");
        write(&marker, "x");
        events.wait_path(&marker);
        thread::sleep(SETTLE);
        assert_eq!(events.git(&git_dir), None, "{:#?}", events.all());

        let err = watcher.set_git_dirs(&[root.join("tidak-ada")]).unwrap_err();
        assert_eq!(err.code, WsErrorCode::NotFound);
    }

    #[test]
    fn git_dir_outside_root_is_watched() {
        let (_tmp, top) = temp_root();
        let root = top.join("work");
        fs::create_dir(&root).unwrap();
        let git_dir = top.join("store.git");
        git(&root, &["init", "-q", "--separate-git-dir", git_dir.to_str().unwrap()]);
        let (_watcher, events) = start(&root, std::slice::from_ref(&git_dir), TreeMode::for_platform());

        write(&git_dir.join("index"), "x");
        write(&git_dir.join("refs/heads/x"), "x");
        events.wait("Git dari direktori di luar root", |e| e.git(&git_dir) == Some((true, true, false)));
        // Berkas penunjuk `.git` di working tree tidak dilaporkan saat isinya tidak berubah.
        assert_eq!(events.paths(), Vec::<String>::new());
    }

    #[test]
    fn linked_worktree_sees_shared_refs() {
        let (_tmp, top) = temp_root();
        let main = top.join("main");
        fs::create_dir(&main).unwrap();
        git(&main, &["init", "-q"]);
        git(&main, &["commit", "-q", "--allow-empty", "-m", "awal"]);
        let linked = top.join("linked");
        git(&main, &["worktree", "add", "-q", "-b", "topik", linked.to_str().unwrap()]);
        let linked_git_dir = main.join(".git/worktrees/linked");
        assert!(linked_git_dir.join("commondir").is_file());

        let (_watcher, events) = start(&linked, std::slice::from_ref(&linked_git_dir), TreeMode::for_platform());

        // Ref hidup di direktori Git bersama.
        git(&main, &["branch", "baru"]);
        events.wait("refs bersama", |e| e.git(&linked_git_dir) == Some((true, false, false)));

        // `index` milik worktree utama bukan urusan worktree tertaut.
        thread::sleep(SETTLE);
        events.clear();
        write(&main.join(".git/index"), "x");
        write(&main.join(".git/MERGE_HEAD"), "x");
        write(&linked_git_dir.join("index"), "x");
        events.wait("index milik worktree tertaut", |e| e.git(&linked_git_dir).is_some());
        thread::sleep(SETTLE);
        assert_eq!(events.git(&linked_git_dir), Some((false, true, false)));
    }

    #[test]
    fn creating_and_removing_dot_git_is_reported_as_files() {
        let (_tmp, root) = temp_root();
        let (_watcher, events) = start(&root, &[], TreeMode::for_platform());

        git(&root, &["init", "-q"]);
        events.wait_path(&root.join(".git"));
        thread::sleep(SETTLE);
        assert_eq!(events.paths(), [root.join(".git").to_string_lossy().into_owned()]);

        events.clear();
        fs::remove_dir_all(root.join(".git")).unwrap();
        events.wait_path(&root.join(".git"));
    }

    #[cfg(unix)]
    #[test]
    fn reported_paths_use_the_root_as_given() {
        let (_tmp, top) = temp_root();
        let real = top.join("real");
        fs::create_dir(&real).unwrap();
        git(&real, &["init", "-q"]);
        let link = top.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let git_dir = link.join(".git");
        let (_watcher, events) = start(&link, std::slice::from_ref(&git_dir), TreeMode::for_platform());
        write(&real.join("a.txt"), "x");
        events.wait_path(&link.join("a.txt"));
        write(&real.join(".git/index"), "x");
        events.wait("Git dengan path asli pemanggil", |e| e.git(&git_dir) == Some((false, true, false)));
    }

    #[test]
    fn dropping_stops_events() {
        let (_tmp, root) = temp_root();
        let (watcher, events) = start(&root, &[], TreeMode::for_platform());
        write(&root.join("a.txt"), "x");
        events.wait_path(&root.join("a.txt"));
        drop(watcher);
        events.clear();
        write(&root.join("b.txt"), "x");
        thread::sleep(SETTLE);
        assert_eq!(events.all(), []);
    }

    #[test]
    fn start_fails_for_missing_paths() {
        let (_tmp, root) = temp_root();
        let err = Watcher::start(&root.join("tidak-ada"), &[], |_| {}).err().unwrap();
        assert_eq!(err.code, WsErrorCode::NotFound);
        let err = Watcher::start(&root, &[root.join("tidak-ada/.git")], |_| {}).err().unwrap();
        assert_eq!(err.code, WsErrorCode::NotFound);
        write(&root.join("berkas"), "x");
        let err = Watcher::start(&root.join("berkas"), &[], |_| {}).err().unwrap();
        assert_eq!(err.code, WsErrorCode::InvalidPath);
    }

    #[test]
    fn events_serialize_with_kind_tag() {
        let files = WatchEvent::Files { paths: vec!["/a".into()], overflow: false };
        assert_eq!(
            serde_json::to_value(&files).unwrap(),
            serde_json::json!({ "kind": "files", "paths": ["/a"], "overflow": false })
        );
        let git = WatchEvent::Git { git_dir: "/r/.git".into(), refs: true, index: false, state: false };
        assert_eq!(
            serde_json::to_value(&git).unwrap(),
            serde_json::json!({ "kind": "git", "gitDir": "/r/.git", "refs": true, "index": false, "state": false })
        );
    }

    #[test]
    fn watcher_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Watcher>();
    }
}
