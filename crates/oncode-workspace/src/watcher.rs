//! A watcher for file changes in the working tree and in Git directories.

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

/// Quiet period before the collected changes are sent.
const QUIET: Duration = Duration::from_millis(250);
/// Wait limit since the first change, so a continuous stream of changes is still delivered.
const MAX_WAIT: Duration = Duration::from_secs(1);
/// Above this number of paths, a delivery is replaced with `overflow: true`.
const MAX_PATHS: usize = 1000;

/// Top-level Git directory entries that are never reported.
const GIT_NOISE: &[&str] = &["objects", "logs", "hooks", "info", "worktrees", "modules", "lfs"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WatchEvent {
    /// A file or folder in the working tree changed. `paths` contains absolute paths without duplicates.
    /// If there are too many changes or the operating system lost events, `paths` is empty and
    /// `overflow` is `true`: reload everything.
    Files { paths: Vec<String>, overflow: bool },
    /// The contents of a watched Git directory changed. `git_dir` is exactly the path
    /// given by the caller.
    ///
    /// - `refs`: `refs/`, `packed-refs`, or `HEAD` changed.
    /// - `index`: `index` changed.
    /// - `state`: an operation marker changed (`MERGE_HEAD`, `rebase-merge`, `rebase-apply`,
    ///   `CHERRY_PICK_HEAD`, `REVERT_HEAD`).
    ///
    /// All three can be `false` if another file changed, e.g. `config` or `FETCH_HEAD`.
    #[serde(rename_all = "camelCase")]
    Git { git_dir: String, refs: bool, index: bool, state: bool },
}

/// A watcher for a single project folder. Watching stops when this value is dropped.
pub struct Watcher {
    tx: mpsc::Sender<Msg>,
    thread: Option<thread::JoinHandle<()>>,
}

/// How to watch the working tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TreeMode {
    /// A single recursive watch from the operating system (FSEvents, ReadDirectoryChangesW).
    Native,
    /// One watch per folder that is not ignored. Used on Linux, because inotify
    /// needs one watch per folder and folders such as `node_modules` or `target`
    /// would exhaust the watch quota.
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
    /// Watches `root` recursively along with each Git directory in `git_dirs` (which may be
    /// outside `root`, e.g. those of a worktree or submodule).
    ///
    /// Changes are collected and then sent after ±250 ms of quiet, at the latest every 1 second,
    /// from a background thread. Changes to paths ignored by gitignore are dropped.
    /// Changes inside any `.git` folder are only reported as `Git` (and only
    /// for directories in `git_dirs`); `*.lock` files and the `objects/` folder are ignored.
    /// The creation or deletion of the `.git` entry itself is reported as `Files`.
    ///
    /// For linked worktrees, changes to `refs/` and `packed-refs` in the shared Git directory
    /// are also reported as `refs: true` on that worktree's `git_dir`.
    ///
    /// Fails if `root` or any of the `git_dirs` cannot be watched, including if the operating
    /// system's watcher limit is exceeded.
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
        // Some backends report paths with their symlinks already resolved, so prefix matching
        // uses the canonical path; the paths sent to the caller still use the original `root`.
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

    /// Replaces the set of watched Git directories.
    ///
    /// Directories that fail to be watched are skipped and the first error is returned; the other
    /// directories still apply. Do not call from inside the `on_event` callback.
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
            // Dropped from inside the callback: the thread will stop by itself after the callback finishes.
            if thread.thread().id() != thread::current().id() {
                let _ = thread.join();
            }
        }
    }
}

struct GitDir {
    /// The path exactly as given by the caller; used in `WatchEvent::Git`.
    shown: String,
    dir: PathBuf,
    /// The shared Git directory, if `dir` belongs to a linked worktree.
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
    /// Paths relative to the project root.
    files: BTreeSet<PathBuf>,
    overflow: bool,
    git: BTreeMap<String, GitFlags>,
    /// Set every time a new change is recorded; used to manage the quiet period.
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
    /// The canonical project root; the prefix of all working tree event paths.
    root: PathBuf,
    /// The project root as given by the caller; the prefix of the reported paths.
    root_shown: PathBuf,
    rules: IgnoreRules,
    git_dirs: Vec<GitDir>,
    /// Paths watched for the sake of Git directories → whether recursive.
    git_watches: BTreeMap<PathBuf, bool>,
    pending: Pending,
    /// `.gitignore` changed: folders that were previously ignored may need to be watched.
    resync: bool,
    on_event: Box<dyn Fn(WatchEvent) + Send + Sync>,
}

impl Worker {
    fn run(mut self, rx: mpsc::Receiver<Msg>) {
        // The times of the first and last change in the batch not yet sent.
        let mut window: Option<(Instant, Instant)> = None;
        loop {
            let msg = match window {
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                Some((first, last)) => {
                    let deadline = (last + QUIET).min(first + MAX_WAIT);
                    let now = Instant::now();
                    // Checked first: `recv_timeout` still returns messages that are already
                    // queued, so a flood of events could delay delivery indefinitely.
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
            // The backend lost track (e.g. the inotify limit was reached): ask for a reload.
            Err(_) => return self.pending.set_overflow(),
        };
        if event.need_rescan() {
            self.pending.set_overflow();
        }
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        // An entry was created, deleted, or renamed (not merely its contents changing). On Windows
        // content changes arrive as `Modify(Any)`, also for folders whose contents changed.
        let structural = !matches!(event.kind, EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Metadata(_) | ModifyKind::Any));
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
            // The contents of `.git` are not part of the working tree. The `.git` entry itself is reported when
            // created or deleted, so a new repository (`git init`) can be recognized.
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
        // "Folder contents changed" (Windows) carries no information: the files inside are reported on their own.
        if is_dir && !structural {
            return;
        }
        if self.rules.is_ignored(path, is_dir) {
            return;
        }
        self.pending.add_file(rel.to_path_buf());

        if self.mode == TreeMode::PerDirectory && is_dir && structural {
            // A new folder (or one moved here): watch it, and report its contents because files
            // created before the watch was installed produce no events.
            match self.watch_tree(path, true) {
                Ok(found) => found.into_iter().for_each(|rel| self.pending.add_file(rel)),
                Err(_) => self.pending.set_overflow(),
            }
        }
    }

    /// Records a change inside a watched Git directory. `false` if `path` is not
    /// part of any Git directory.
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
        // The Git directory itself (created/deleted) is handled as a working tree entry.
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
                // A linked worktree shares refs with its parent repository; `HEAD`, `index`,
                // and operation markers in the shared directory belong to the main worktree.
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

    /// Installs a watch on `top` and all folders beneath it that are not ignored.
    /// If `collect`, returns the entries found (relative to the project root).
    ///
    /// Fails if `top` itself cannot be watched or the watcher limit is exceeded; other
    /// subfolders that fail (e.g. already deleted) are skipped.
    fn watch_tree(&mut self, top: &Path, collect: bool) -> notify::Result<Vec<PathBuf>> {
        let mut found = Vec::new();
        let mut stack = vec![top.to_path_buf()];
        let mut is_top = true;
        while let Some(dir) = stack.pop() {
            // The watch is installed before the folder contents are read, so there is no gap in between.
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

    /// True if `path` is a working tree folder, whose watch must not be released when
    /// the set of Git directories changes.
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
            // The key files (`HEAD`, `index`, `packed-refs`, operation markers) are direct children;
            // only `refs/` needs to be watched all the way down. `objects/` is not watched.
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
    /// Long enough to be sure an event really is not coming (above the quiet period).
    const SETTLE: Duration = Duration::from_millis(700);
    /// Backends that can deliver events late (see [`Events::settle`]).
    const DELAYED_EVENTS: bool = !cfg!(target_os = "linux");

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

        /// Discards the events of the previous step, including those that are still to come. FSEvents (macOS) and
        /// ReadDirectoryChangesW (Windows, when busy) can deliver events with a delay, also for
        /// changes made just before watching started (e.g. `git init` during test setup).
        /// inotify (Linux) does not, so there it is enough to just clear.
        fn settle(&self) {
            if DELAYED_EVENTS {
                thread::sleep(SETTLE);
            }
            self.clear();
        }

        /// All paths ever reported through `Files`.
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

        /// The merged flags of all `Git` events for `git_dir`; `None` if there are none yet.
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
            panic!("did not happen within {TIMEOUT:?}: {what}; events: {:#?}", self.all());
        }

        fn wait_path(&self, path: &Path) {
            self.wait(&format!("Files event for {}", path.display()), |e| e.has_path(path));
        }
    }

    fn start(root: &Path, git_dirs: &[PathBuf], mode: TreeMode) -> (Watcher, Events) {
        let events = Events::default();
        let watcher = Watcher::start_with(root, git_dirs, events.sink(), mode).unwrap();
        // Trailing events from the test setup (files and `git init` before watching) are discarded.
        if DELAYED_EVENTS {
            events.settle();
        }
        (watcher, events)
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn git(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(dir)
            .args(["-c", "user.name=Test", "-c", "user.email=test@example.com", "-c", "init.defaultBranch=main"])
            .args(["-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null"])
            .args(args)
            // The Git configuration of the machine running the tests must not have any influence.
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" })
            .output()
            .expect("git must be installed for this test");
        assert!(output.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    /// A temporary folder with a canonical path, so the reported paths are easy to compare.
    fn temp_root() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        // Like the root from `workspace_open`: without the Windows `\\?\` prefix, which Git rejects.
        let root = match root.to_string_lossy().strip_prefix(r"\\?\") {
            Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
            _ => root,
        };
        (tmp, root)
    }

    /// The modes used on this platform (see [`TreeMode::for_platform`]), plus `Native` on Linux.
    /// `PerDirectory` is only used on Linux; on Windows a per-folder ReadDirectoryChangesW locks
    /// the folder (it cannot be renamed), and on macOS a per-folder FSEvents reports old contents.
    const MODES: &[TreeMode] = if cfg!(target_os = "linux") { &[TreeMode::PerDirectory, TreeMode::Native] } else { &[TreeMode::Native] };

    #[test]
    fn reports_created_and_modified_files() {
        for &mode in MODES {
            let (_tmp, root) = temp_root();
            write(&root.join("src/existing.rs"), "v1");
            let (_watcher, events) = start(&root, &[], mode);

            let created = root.join("new.txt");
            write(&created, "hello");
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
            write(&file, &format!("content {i}"));
        }
        write(&root.join("b.txt"), "x");
        events.wait_path(&root.join("b.txt"));
        thread::sleep(SETTLE);

        // 21 writes become a handful of deliveries (usually one), each path once per delivery.
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
        for &mode in MODES {
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
            let visible = root.join("src").join("visible.rs");
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

        // Before it is ignored, changes in `cache/` are reported.
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

        // After the rule is revoked, the folder that was previously ignored is watched again.
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
        for &mode in MODES {
            let (_tmp, root) = temp_root();
            let (_watcher, events) = start(&root, &[], mode);

            // The folder and its contents are created at once, before the new folder's watch could be installed.
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

            // A moved folder stays watched at its new location.
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
        events.wait("overflow event", |e| {
            e.all().iter().any(|ev| matches!(ev, WatchEvent::Files { paths, overflow: true } if paths.is_empty()))
        });
    }

    #[test]
    fn git_index_refs_and_state_changes() {
        for &mode in MODES {
            let (_tmp, root) = temp_root();
            git(&root, &["init", "-q"]);
            let git_dir = root.join(".git");
            let (_watcher, events) = start(&root, std::slice::from_ref(&git_dir), mode);

            write(&git_dir.join("index"), "fake index");
            events.wait("Git{index}", |e| e.git(&git_dir) == Some((false, true, false)));

            events.settle();
            write(&git_dir.join("refs/heads/x"), "0000000000000000000000000000000000000000\n");
            events.wait("Git{refs} from refs/heads/x", |e| e.git(&git_dir) == Some((true, false, false)));

            // A new ref folder (a branch named `feature/a`) is watched too.
            events.settle();
            write(&git_dir.join("refs/heads/feature/a"), "0000000000000000000000000000000000000000\n");
            events.wait("Git{refs} from refs/heads/feature/a", |e| e.git(&git_dir) == Some((true, false, false)));

            events.settle();
            write(&git_dir.join("packed-refs"), "# pack-refs\n");
            events.wait("Git{refs} from packed-refs", |e| e.git(&git_dir) == Some((true, false, false)));

            events.settle();
            write(&git_dir.join("HEAD"), "ref: refs/heads/other\n");
            events.wait("Git{refs} from HEAD", |e| e.git(&git_dir) == Some((true, false, false)));

            events.settle();
            write(&git_dir.join("MERGE_HEAD"), "0000000000000000000000000000000000000000\n");
            events.wait("Git{state} from MERGE_HEAD", |e| e.git(&git_dir) == Some((false, false, true)));

            events.settle();
            fs::create_dir(git_dir.join("rebase-merge")).unwrap();
            events.wait("Git{state} from rebase-merge", |e| e.git(&git_dir) == Some((false, false, true)));

            // Another file in the Git directory: an event without flags.
            events.settle();
            write(&git_dir.join("FETCH_HEAD"), "x\n");
            events.wait("Git without flags", |e| e.git(&git_dir) == Some((false, false, false)));

            // None of it leaks as a working tree change.
            thread::sleep(SETTLE);
            assert_eq!(events.paths(), Vec::<String>::new(), "{mode:?}");
        }
    }

    #[test]
    fn git_lock_files_and_objects_are_ignored() {
        for &mode in MODES {
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
        write(&root.join("a.txt"), "one\n");
        let (_watcher, events) = start(&root, std::slice::from_ref(&git_dir), TreeMode::for_platform());

        git(&root, &["add", "a.txt"]);
        events.wait("git add changes the index", |e| e.git(&git_dir).is_some_and(|(_, index, _)| index));

        events.clear();
        git(&root, &["commit", "-q", "-m", "initial"]);
        events.wait("git commit changes refs", |e| e.git(&git_dir).is_some_and(|(refs, _, _)| refs));

        thread::sleep(SETTLE);
        events.clear();
        git(&root, &["branch", "feature"]);
        events.wait("git branch changes refs", |e| e.git(&git_dir) == Some((true, false, false)));
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
        // Trailing events from `git init` now also count as Git events; discard them first.
        events.settle();
        write(&git_dir.join("index"), "2");
        events.wait("Git{index} after set_git_dirs", |e| e.git(&git_dir) == Some((false, true, false)));

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

        let err = watcher.set_git_dirs(&[root.join("does-not-exist")]).unwrap_err();
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
        events.wait("Git from a directory outside root", |e| e.git(&git_dir) == Some((true, true, false)));
        // The `.git` pointer file in the working tree is not reported when its contents do not change.
        assert_eq!(events.paths(), Vec::<String>::new());
    }

    #[test]
    fn linked_worktree_sees_shared_refs() {
        let (_tmp, top) = temp_root();
        let main = top.join("main");
        fs::create_dir(&main).unwrap();
        git(&main, &["init", "-q"]);
        git(&main, &["commit", "-q", "--allow-empty", "-m", "initial"]);
        let linked = top.join("linked");
        git(&main, &["worktree", "add", "-q", "-b", "topic", linked.to_str().unwrap()]);
        let linked_git_dir = main.join(".git/worktrees/linked");
        assert!(linked_git_dir.join("commondir").is_file());

        let (_watcher, events) = start(&linked, std::slice::from_ref(&linked_git_dir), TreeMode::for_platform());

        // Refs live in the shared Git directory.
        git(&main, &["branch", "new"]);
        events.wait("shared refs", |e| e.git(&linked_git_dir) == Some((true, false, false)));

        // The main worktree's `index` is none of the linked worktree's business.
        thread::sleep(SETTLE);
        events.clear();
        write(&main.join(".git/index"), "x");
        write(&main.join(".git/MERGE_HEAD"), "x");
        write(&linked_git_dir.join("index"), "x");
        events.wait("index of the linked worktree", |e| e.git(&linked_git_dir).is_some());
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
        events.wait("Git with the caller's original path", |e| e.git(&git_dir) == Some((false, true, false)));
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
        let err = Watcher::start(&root.join("does-not-exist"), &[], |_| {}).err().unwrap();
        assert_eq!(err.code, WsErrorCode::NotFound);
        let err = Watcher::start(&root, &[root.join("does-not-exist/.git")], |_| {}).err().unwrap();
        assert_eq!(err.code, WsErrorCode::NotFound);
        write(&root.join("file"), "x");
        let err = Watcher::start(&root.join("file"), &[], |_| {}).err().unwrap();
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
