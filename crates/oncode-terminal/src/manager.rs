//! PTY manager: runs shells, forwards input/output, and manages their lifecycle.
//!
//! Each terminal uses two threads:
//! - the *reader* reads the PTY in chunks of up to 64 KiB and calls `on_data`;
//! - the *waiter* waits for the child process to exit, makes sure the remaining output is delivered, then calls `on_exit`.
//!
//! Limitation on Unix: if the shell has exited but its background processes still hold the PTY,
//! the reader thread only ends (and the PTY is only released) when such a process writes something or finishes.
//! `on_exit` is not delayed by this; output that arrives afterwards is discarded.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, Child, ChildKiller, CommandBuilder, ExitStatus, MasterPty, PtySize};
use serde::Deserialize;

use crate::error::{Result, TermError};
use crate::shell::default_shell;

/// Maximum size of a single output chunk forwarded to `on_data`.
const READ_CHUNK: usize = 64 * 1024;
/// After the child process exits, the reader is considered finished if there is no new output for this long.
/// Needed because a background process can keep the PTY open so EOF never arrives.
const DRAIN_IDLE: Duration = Duration::from_millis(250);
/// Delay between SIGHUP and SIGKILL when a terminal is force-closed.
#[cfg(unix)]
const KILL_GRACE: Duration = Duration::from_millis(300);

const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

/// On Windows the reader must keep draining the output pipe after the terminal is closed, because
/// `ClosePseudoConsole` on older Windows versions waits until the output has been read.
/// On Unix the reader stops immediately so the PTY is released right away.
const DRAIN_AFTER_CLOSE: bool = cfg!(windows);

/// Options for [`TerminalManager::spawn`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnOptions {
    /// The shell program. `None` (or an empty string) means [`default_shell`].
    #[serde(default)]
    pub shell: Option<String>,
    /// Extra arguments for the shell.
    #[serde(default)]
    pub args: Vec<String>,
    /// Initial working folder. An empty string means the user's home folder.
    pub cwd: String,
    /// Terminal width in columns. `0` means 80.
    pub cols: u16,
    /// Terminal height in rows. `0` means 24.
    pub rows: u16,
    /// Extra environment variables. `TERM=xterm-256color`, `COLORTERM=truecolor` and
    /// `TERM_PROGRAM=OnCode` are always set first; entries here may override them.
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

/// Locks a mutex and ignores the *poisoned* state: a user callback that panics
/// must not make the whole terminal manager hang.
fn lock<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        cols: if cols == 0 { DEFAULT_COLS } else { cols },
        rows: if rows == 0 { DEFAULT_ROWS } else { rows },
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// The process exit code; `None` if the process was stopped by a signal or its status could not be read.
fn exit_code(status: std::io::Result<ExitStatus>) -> Option<i32> {
    let status = status.ok()?;
    if status.signal().is_some() {
        return None;
    }
    // Windows exit codes (e.g. 0xC000013A) are deliberately wrapped into an i32.
    Some(status.exit_code() as i32)
}

#[derive(Default)]
struct Flow {
    /// The UI asked for reading to be paused.
    paused: bool,
    /// The terminal was closed through `kill`; remaining output is discarded.
    closed: bool,
}

#[derive(Default)]
struct ReaderProgress {
    /// Number of chunks already forwarded to `on_data`.
    chunks: u64,
    /// The reader thread has ended.
    done: bool,
}

struct Session {
    /// `None` after the child process exits (the PTY has been released).
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    /// `None` after the child process exits.
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    #[cfg(unix)]
    pid: Option<u32>,
    flow: Mutex<Flow>,
    flow_cv: Condvar,
    /// `true` once the child process has been waited on. Signals are only sent while holding this lock
    /// and only if its value is `false`, so they never hit a PID already reused by another process.
    exited: Mutex<bool>,
    exited_cv: Condvar,
    reader: Mutex<ReaderProgress>,
    reader_cv: Condvar,
    /// `true` from the point `on_exit` is about to be called. Held while `on_data` runs so
    /// no output is delivered after `on_exit`.
    delivery_closed: Mutex<bool>,
}

impl Session {
    /// Waits until not paused. Returns `false` if the terminal has been closed.
    fn wait_until_resumed(&self) -> bool {
        let mut flow = lock(&self.flow);
        while flow.paused && !flow.closed {
            flow = self.flow_cv.wait(flow).unwrap_or_else(PoisonError::into_inner);
        }
        !flow.closed
    }

    fn set_paused(&self, paused: bool) {
        lock(&self.flow).paused = paused;
        self.flow_cv.notify_all();
    }

    /// Closes the terminal: discards remaining output and stops the child process.
    /// `on_exit` is still called by the waiter thread once the process has actually ended.
    fn terminate(self: &Arc<Self>) {
        lock(&self.flow).closed = true;
        self.flow_cv.notify_all();
        self.reader_cv.notify_all();

        {
            let exited = lock(&self.exited);
            if *exited {
                return;
            }
            // Unix: SIGHUP (like closing a terminal window). Windows: TerminateProcess.
            let _ = lock(&self.killer).kill();
        }

        #[cfg(unix)]
        {
            let session = Arc::clone(self);
            let spawned = thread::Builder::new()
                .name("oncode-pty-kill".into())
                .spawn(move || session.force_kill_after(KILL_GRACE));
            if spawned.is_err() {
                self.force_kill_after(Duration::ZERO);
            }
        }
    }

    /// Sends SIGKILL if the process is still alive after `grace` (e.g. it ignores SIGHUP).
    #[cfg(unix)]
    fn force_kill_after(&self, grace: Duration) {
        let exited = lock(&self.exited);
        let (exited, _) = self
            .exited_cv
            .wait_timeout_while(exited, grace, |exited| !*exited)
            .unwrap_or_else(PoisonError::into_inner);
        if *exited {
            return;
        }
        // PID 0 or negative means "process group" to kill(2); never send a signal to it.
        let pid = self.pid.and_then(|pid| libc::pid_t::try_from(pid).ok()).filter(|pid| *pid > 0);
        if let Some(pid) = pid {
            // SAFETY: kill(2) does not touch memory; the `exited` lock guarantees the process has not been reaped.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }

    /// Called by the waiter thread after the child process exits: waits for the reader to finish the remaining output.
    fn wait_for_drain(&self) {
        let mut progress = lock(&self.reader);
        let mut last_chunks = progress.chunks;
        let mut last_change = Instant::now();
        loop {
            if progress.done {
                return;
            }
            let (paused, closed) = {
                let flow = lock(&self.flow);
                (flow.paused, flow.closed)
            };
            if closed {
                return;
            }
            if paused || progress.chunks != last_chunks {
                // While paused the UI is not ready to receive; do not count it as idle.
                last_chunks = progress.chunks;
                last_change = Instant::now();
            } else if last_change.elapsed() >= DRAIN_IDLE {
                return;
            }
            progress = self
                .reader_cv
                .wait_timeout(progress, DRAIN_IDLE)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// Marks the reader as finished even if `on_data` panics.
struct ReaderDoneGuard(Arc<Session>);

impl Drop for ReaderDoneGuard {
    fn drop(&mut self) {
        lock(&self.0.reader).done = true;
        self.0.reader_cv.notify_all();
    }
}

fn reader_loop(mut reader: Box<dyn Read + Send>, session: Arc<Session>, on_data: impl Fn(Vec<u8>)) {
    let _done = ReaderDoneGuard(Arc::clone(&session));
    let mut buf = vec![0u8; READ_CHUNK];
    loop {
        if !session.wait_until_resumed() && !DRAIN_AFTER_CLOSE {
            break;
        }
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        // A chunk read exactly while paused is held until resumed.
        if !session.wait_until_resumed() {
            if DRAIN_AFTER_CLOSE {
                continue;
            }
            break;
        }
        {
            let closed = lock(&session.delivery_closed);
            if *closed {
                if DRAIN_AFTER_CLOSE {
                    continue;
                }
                break;
            }
            on_data(buf[..n].to_vec());
        }
        lock(&session.reader).chunks += 1;
        session.reader_cv.notify_all();
    }
}

fn wait_loop(
    mut child: Box<dyn Child + Send + Sync>,
    session: Arc<Session>,
    registry: Weak<Registry>,
    id: u32,
    on_exit: impl FnOnce(Option<i32>),
) {
    let code = exit_code(child.wait());
    *lock(&session.exited) = true;
    session.exited_cv.notify_all();

    // Release the PTY. On Windows this is what makes the reader receive EOF.
    let master = lock(&session.master).take();
    drop(master);

    // The terminal stays registered while remaining output is undelivered so the UI can still
    // resume (`set_paused(false)`) a terminal that is paused.
    session.wait_for_drain();
    if let Some(registry) = registry.upgrade() {
        let removed = lock(&registry.sessions).remove(&id);
        drop(removed);
    }
    *lock(&session.delivery_closed) = true;
    on_exit(code);

    let writer = lock(&session.writer).take();
    drop(writer);
}

#[derive(Default)]
struct Registry {
    next_id: AtomicU32,
    sessions: Mutex<HashMap<u32, Arc<Session>>>,
}

impl Registry {
    fn get(&self, id: u32) -> Result<Arc<Session>> {
        lock(&self.sessions).get(&id).cloned().ok_or_else(|| TermError::not_found(id))
    }
}

/// Manager of all terminals owned by one application. Safe to use from many threads behind an `Arc`.
/// When dropped, all terminals are stopped.
#[derive(Default)]
pub struct TerminalManager {
    registry: Arc<Registry>,
}

impl TerminalManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs a shell in a new PTY and returns the id of its terminal.
    ///
    /// `on_data` receives the raw output bytes from a dedicated reader thread, in chunks of up to 64 KiB
    /// (a chunk can be cut in the middle of a UTF-8 character). `on_exit` is called exactly once with
    /// the exit code once the process ends, after all output has been delivered; its value is `None` if
    /// the process was stopped by a signal. When `on_exit` is called, the id is no longer in [`Self::ids`].
    pub fn spawn(
        &self,
        options: SpawnOptions,
        on_data: impl Fn(Vec<u8>) + Send + 'static,
        on_exit: impl FnOnce(Option<i32>) + Send + 'static,
    ) -> Result<u32> {
        let shell = options
            .shell
            .as_deref()
            .map(str::trim)
            .filter(|shell| !shell.is_empty())
            .map(str::to_string)
            .unwrap_or_else(default_shell);

        // portable-pty silently uses the home folder if cwd does not exist; better to fail clearly.
        if !options.cwd.is_empty() && !Path::new(&options.cwd).is_dir() {
            return Err(TermError::spawn_failed(format!(
                "Terminal working directory not found: {}",
                options.cwd
            )));
        }

        let mut command = CommandBuilder::new(&shell);
        command.args(&options.args);
        if !options.cwd.is_empty() {
            command.cwd(&options.cwd);
        }
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        command.env("TERM_PROGRAM", "OnCode");
        for (key, value) in &options.env {
            if !key.is_empty() {
                command.env(key, value);
            }
        }

        let pair = native_pty_system()
            .openpty(pty_size(options.cols, options.rows))
            .map_err(|err| TermError::spawn_failed(format!("Failed to create PTY: {err:#}")))?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|err| TermError::spawn_failed(format!("Failed to open PTY reader: {err:#}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|err| TermError::spawn_failed(format!("Failed to open PTY writer: {err:#}")))?;
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|err| TermError::spawn_failed(format!("Failed to start shell `{shell}`: {err:#}")))?;
        // The slave side must be closed in this process so EOF arrives when the child process exits.
        drop(pair.slave);

        let session = Arc::new(Session {
            master: Mutex::new(Some(pair.master)),
            writer: Mutex::new(Some(writer)),
            killer: Mutex::new(child.clone_killer()),
            #[cfg(unix)]
            pid: child.process_id(),
            flow: Mutex::new(Flow::default()),
            flow_cv: Condvar::new(),
            exited: Mutex::new(false),
            exited_cv: Condvar::new(),
            reader: Mutex::new(ReaderProgress::default()),
            reader_cv: Condvar::new(),
            delivery_closed: Mutex::new(false),
        });

        let id = self.registry.next_id.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        lock(&self.registry.sessions).insert(id, Arc::clone(&session));

        let reader_session = Arc::clone(&session);
        let reader_thread = thread::Builder::new()
            .name(format!("oncode-pty-read-{id}"))
            .spawn(move || reader_loop(reader, reader_session, on_data));

        let waiter_session = Arc::clone(&session);
        let registry = Arc::downgrade(&self.registry);
        let waiter_thread = reader_thread.and_then(|_| {
            thread::Builder::new()
                .name(format!("oncode-pty-wait-{id}"))
                .spawn(move || wait_loop(child, waiter_session, registry, id, on_exit))
        });

        if let Err(err) = waiter_thread {
            // Without the waiter thread nothing calls `on_exit`; cancel everything.
            let removed = lock(&self.registry.sessions).remove(&id);
            drop(removed);
            session.terminate();
            return Err(TermError::spawn_failed(format!("Failed to create terminal thread: {err}")));
        }

        Ok(id)
    }

    /// Writes input (user keystrokes, pasted text) to the terminal.
    pub fn write(&self, id: u32, data: &[u8]) -> Result<()> {
        let session = self.registry.get(id)?;
        let mut writer = lock(&session.writer);
        let writer = writer.as_mut().ok_or_else(|| TermError::not_found(id))?;
        writer
            .write_all(data)
            .and_then(|()| writer.flush())
            .map_err(|err| TermError::io("Failed to write to terminal", err))
    }

    /// Resizes the terminal; the process inside receives SIGWINCH.
    pub fn resize(&self, id: u32, cols: u16, rows: u16) -> Result<()> {
        let session = self.registry.get(id)?;
        let master = lock(&session.master);
        let master = master.as_ref().ok_or_else(|| TermError::not_found(id))?;
        master
            .resize(pty_size(cols, rows))
            .map_err(|err| TermError::io("Failed to resize terminal", format!("{err:#}")))
    }

    /// Flow control: while paused the reader thread stops reading the PTY so the child process
    /// blocks when writing output. Held-back output is delivered after resuming.
    pub fn set_paused(&self, id: u32, paused: bool) -> Result<()> {
        self.registry.get(id)?.set_paused(paused);
        Ok(())
    }

    /// Stops the child process and releases the PTY. An unknown id is not an error.
    ///
    /// The id disappears from [`Self::ids`] immediately; `on_exit` follows once the process has actually ended.
    /// On Unix the process is sent SIGHUP, then SIGKILL if it is still alive after a short delay.
    /// Remaining undelivered output is discarded.
    pub fn kill(&self, id: u32) {
        let session = lock(&self.registry.sessions).remove(&id);
        if let Some(session) = session {
            session.terminate();
        }
    }

    /// Stops all terminals.
    pub fn kill_all(&self) {
        let sessions: Vec<Arc<Session>> = lock(&self.registry.sessions).drain().map(|(_, session)| session).collect();
        for session in sessions {
            session.terminate();
        }
    }

    /// The ids of all terminals that are still alive, in ascending order.
    pub fn ids(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = lock(&self.registry.sessions).keys().copied().collect();
        ids.sort_unstable();
        ids
    }
}

impl Drop for TerminalManager {
    fn drop(&mut self) {
        self.kill_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_options_deserialize_from_camel_case_with_defaults() {
        let options: SpawnOptions =
            serde_json::from_str(r#"{"cwd":"/tmp","cols":120,"rows":40}"#).expect("minimal options");
        assert_eq!(
            options,
            SpawnOptions { shell: None, args: vec![], cwd: "/tmp".into(), cols: 120, rows: 40, env: vec![] }
        );

        let options: SpawnOptions = serde_json::from_str(
            r#"{"shell":"/bin/zsh","args":["-l"],"cwd":"/x","cols":80,"rows":24,"env":[["A","1"],["B","2"]]}"#,
        )
        .expect("full options");
        assert_eq!(options.shell.as_deref(), Some("/bin/zsh"));
        assert_eq!(options.args, vec!["-l".to_string()]);
        assert_eq!(options.env, vec![("A".into(), "1".into()), ("B".into(), "2".into())]);
    }

    #[test]
    fn zero_size_falls_back_to_default() {
        let size = pty_size(0, 0);
        assert_eq!((size.cols, size.rows), (DEFAULT_COLS, DEFAULT_ROWS));
        let size = pty_size(132, 50);
        assert_eq!((size.cols, size.rows), (132, 50));
    }

    #[test]
    fn exit_code_maps_signals_to_none() {
        assert_eq!(exit_code(Ok(ExitStatus::with_exit_code(0))), Some(0));
        assert_eq!(exit_code(Ok(ExitStatus::with_exit_code(3))), Some(3));
        assert_eq!(exit_code(Ok(ExitStatus::with_signal("Hangup"))), None);
        assert_eq!(exit_code(Err(std::io::Error::other("failed"))), None);
    }

    #[test]
    fn manager_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<TerminalManager>();
    }
}
