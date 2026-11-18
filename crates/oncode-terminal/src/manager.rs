//! Pengelola PTY: menjalankan shell, meneruskan masukan/keluaran, dan mengatur siklus hidupnya.
//!
//! Tiap terminal memakai dua thread:
//! - *pembaca* membaca PTY per potongan hingga 64 KiB dan memanggil `on_data`;
//! - *penunggu* menunggu proses anak keluar, memastikan sisa keluaran terkirim, lalu memanggil `on_exit`.
//!
//! Keterbatasan di Unix: bila shell sudah keluar tetapi proses latar belakangnya masih memegang PTY,
//! thread pembaca baru berakhir (dan PTY baru dibebaskan) saat proses itu menulis sesuatu atau selesai.
//! `on_exit` tidak ikut tertunda; keluaran yang datang setelahnya dibuang.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, Child, ChildKiller, CommandBuilder, ExitStatus, MasterPty, PtySize};
use serde::Deserialize;
use specta::Type;

use crate::error::{Result, TermError};
use crate::shell::default_shell;

/// Ukuran maksimum satu potongan keluaran yang diteruskan ke `on_data`.
const READ_CHUNK: usize = 64 * 1024;
/// Setelah proses anak keluar, pembaca dianggap selesai bila tidak ada keluaran baru selama ini.
/// Diperlukan karena proses latar belakang bisa menahan PTY tetap terbuka sehingga EOF tidak pernah tiba.
const DRAIN_IDLE: Duration = Duration::from_millis(250);
/// Jeda antara SIGHUP dan SIGKILL saat terminal ditutup paksa.
#[cfg(unix)]
const KILL_GRACE: Duration = Duration::from_millis(300);

const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

/// Di Windows pembaca harus terus menguras pipa keluaran setelah terminal ditutup, sebab
/// `ClosePseudoConsole` pada versi Windows lama menunggu sampai keluaran habis dibaca.
/// Di Unix pembaca langsung berhenti agar PTY segera dibebaskan.
const DRAIN_AFTER_CLOSE: bool = cfg!(windows);

/// Opsi untuk [`TerminalManager::spawn`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SpawnOptions {
    /// Program shell. `None` (atau string kosong) berarti [`default_shell`].
    #[serde(default)]
    pub shell: Option<String>,
    /// Argumen tambahan untuk shell.
    #[serde(default)]
    pub args: Vec<String>,
    /// Folder kerja awal. String kosong berarti folder home pengguna.
    pub cwd: String,
    /// Lebar terminal dalam kolom. `0` berarti 80.
    pub cols: u16,
    /// Tinggi terminal dalam baris. `0` berarti 24.
    pub rows: u16,
    /// Variabel lingkungan tambahan. `TERM=xterm-256color`, `COLORTERM=truecolor` dan
    /// `TERM_PROGRAM=OnCode` selalu dipasang lebih dulu; entri di sini boleh menimpanya.
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

/// Mengunci mutex dan mengabaikan status *poisoned*: callback pengguna yang panik
/// tidak boleh membuat seluruh pengelola terminal ikut macet.
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

/// Kode keluar proses; `None` bila proses dihentikan oleh sinyal atau statusnya tidak terbaca.
fn exit_code(status: std::io::Result<ExitStatus>) -> Option<i32> {
    let status = status.ok()?;
    if status.signal().is_some() {
        return None;
    }
    // Kode keluar Windows (mis. 0xC000013A) sengaja dibungkus ke i32.
    Some(status.exit_code() as i32)
}

#[derive(Default)]
struct Flow {
    /// UI meminta pembacaan dijeda.
    paused: bool,
    /// Terminal ditutup lewat `kill`; sisa keluaran dibuang.
    closed: bool,
}

#[derive(Default)]
struct ReaderProgress {
    /// Jumlah potongan yang sudah diteruskan ke `on_data`.
    chunks: u64,
    /// Thread pembaca sudah berakhir.
    done: bool,
}

struct Session {
    /// `None` setelah proses anak keluar (PTY sudah dibebaskan).
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    /// `None` setelah proses anak keluar.
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    #[cfg(unix)]
    pid: Option<u32>,
    flow: Mutex<Flow>,
    flow_cv: Condvar,
    /// `true` setelah proses anak selesai ditunggu. Sinyal hanya dikirim sambil memegang kunci ini
    /// dan hanya bila nilainya `false`, supaya tidak mengenai PID yang sudah dipakai proses lain.
    exited: Mutex<bool>,
    exited_cv: Condvar,
    reader: Mutex<ReaderProgress>,
    reader_cv: Condvar,
    /// `true` sejak `on_exit` akan dipanggil. Dipegang selama `on_data` berjalan sehingga
    /// tidak ada keluaran yang dikirim setelah `on_exit`.
    delivery_closed: Mutex<bool>,
}

impl Session {
    /// Menunggu sampai tidak dijeda. Mengembalikan `false` bila terminal sudah ditutup.
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

    /// Menutup terminal: membuang sisa keluaran dan menghentikan proses anak.
    /// `on_exit` tetap dipanggil oleh thread penunggu begitu proses benar-benar berakhir.
    fn terminate(self: &Arc<Self>) {
        lock(&self.flow).closed = true;
        self.flow_cv.notify_all();
        self.reader_cv.notify_all();

        {
            let exited = lock(&self.exited);
            if *exited {
                return;
            }
            // Unix: SIGHUP (seperti menutup jendela terminal). Windows: TerminateProcess.
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

    /// Mengirim SIGKILL bila proses masih hidup setelah `grace` (mis. mengabaikan SIGHUP).
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
        // PID 0 atau negatif berarti "grup proses" bagi kill(2); jangan pernah dikirimi sinyal.
        let pid = self.pid.and_then(|pid| libc::pid_t::try_from(pid).ok()).filter(|pid| *pid > 0);
        if let Some(pid) = pid {
            // SAFETY: kill(2) tidak menyentuh memori; kunci `exited` menjamin proses belum di-reap.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }

    /// Dipanggil thread penunggu setelah proses anak keluar: menunggu pembaca menghabiskan sisa keluaran.
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
                // Selama dijeda UI belum siap menerima; jangan dihitung sebagai diam.
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

/// Menandai pembaca selesai walaupun `on_data` panik.
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
        // Potongan yang terbaca tepat saat dijeda ditahan sampai dilanjutkan.
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

    // Membebaskan PTY. Di Windows inilah yang membuat pembaca menerima EOF.
    let master = lock(&session.master).take();
    drop(master);

    // Terminal tetap terdaftar selama sisa keluaran belum terkirim supaya UI masih bisa
    // melanjutkan (`set_paused(false)`) terminal yang sedang dijeda.
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

/// Pengelola semua terminal milik satu aplikasi. Aman dipakai dari banyak thread di balik `Arc`.
/// Saat di-drop, semua terminal dihentikan.
#[derive(Default)]
pub struct TerminalManager {
    registry: Arc<Registry>,
}

impl TerminalManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Menjalankan shell di PTY baru dan mengembalikan id terminalnya.
    ///
    /// `on_data` menerima byte mentah keluaran dari thread pembaca khusus, per potongan hingga 64 KiB
    /// (potongan bisa terputus di tengah karakter UTF-8). `on_exit` dipanggil tepat satu kali dengan
    /// kode keluar begitu proses berakhir, setelah semua keluaran terkirim; nilainya `None` bila
    /// proses dihentikan oleh sinyal. Saat `on_exit` dipanggil, id sudah tidak ada di [`Self::ids`].
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

        // portable-pty diam-diam memakai folder home bila cwd tidak ada; lebih baik gagal dengan jelas.
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
        // Sisi slave harus ditutup di proses ini agar EOF tiba saat proses anak keluar.
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
            // Tanpa thread penunggu tidak ada yang memanggil `on_exit`; batalkan seluruhnya.
            let removed = lock(&self.registry.sessions).remove(&id);
            drop(removed);
            session.terminate();
            return Err(TermError::spawn_failed(format!("Failed to create terminal thread: {err}")));
        }

        Ok(id)
    }

    /// Menulis masukan (ketikan pengguna, tempelan) ke terminal.
    pub fn write(&self, id: u32, data: &[u8]) -> Result<()> {
        let session = self.registry.get(id)?;
        let mut writer = lock(&session.writer);
        let writer = writer.as_mut().ok_or_else(|| TermError::not_found(id))?;
        writer
            .write_all(data)
            .and_then(|()| writer.flush())
            .map_err(|err| TermError::io("Failed to write to terminal", err))
    }

    /// Mengubah ukuran terminal; proses di dalamnya menerima SIGWINCH.
    pub fn resize(&self, id: u32, cols: u16, rows: u16) -> Result<()> {
        let session = self.registry.get(id)?;
        let master = lock(&session.master);
        let master = master.as_ref().ok_or_else(|| TermError::not_found(id))?;
        master
            .resize(pty_size(cols, rows))
            .map_err(|err| TermError::io("Failed to resize terminal", format!("{err:#}")))
    }

    /// Kendali aliran: selama dijeda thread pembaca berhenti membaca PTY sehingga proses anak
    /// tertahan saat menulis keluaran. Keluaran yang tertahan dikirim setelah dilanjutkan.
    pub fn set_paused(&self, id: u32, paused: bool) -> Result<()> {
        self.registry.get(id)?.set_paused(paused);
        Ok(())
    }

    /// Menghentikan proses anak dan membebaskan PTY. Id yang tidak dikenal bukan galat.
    ///
    /// Id langsung hilang dari [`Self::ids`]; `on_exit` menyusul begitu proses benar-benar berakhir.
    /// Di Unix proses dikirimi SIGHUP, lalu SIGKILL bila masih hidup setelah jeda singkat.
    /// Sisa keluaran yang belum terkirim dibuang.
    pub fn kill(&self, id: u32) {
        let session = lock(&self.registry.sessions).remove(&id);
        if let Some(session) = session {
            session.terminate();
        }
    }

    /// Menghentikan semua terminal.
    pub fn kill_all(&self) {
        let sessions: Vec<Arc<Session>> = lock(&self.registry.sessions).drain().map(|(_, session)| session).collect();
        for session in sessions {
            session.terminate();
        }
    }

    /// Id semua terminal yang masih hidup, terurut naik.
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
            serde_json::from_str(r#"{"cwd":"/tmp","cols":120,"rows":40}"#).expect("opsi minimal");
        assert_eq!(
            options,
            SpawnOptions { shell: None, args: vec![], cwd: "/tmp".into(), cols: 120, rows: 40, env: vec![] }
        );

        let options: SpawnOptions = serde_json::from_str(
            r#"{"shell":"/bin/zsh","args":["-l"],"cwd":"/x","cols":80,"rows":24,"env":[["A","1"],["B","2"]]}"#,
        )
        .expect("opsi lengkap");
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
        assert_eq!(exit_code(Err(std::io::Error::other("gagal"))), None);
    }

    #[test]
    fn manager_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<TerminalManager>();
    }
}
