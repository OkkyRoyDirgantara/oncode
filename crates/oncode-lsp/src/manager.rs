//! Pengelola proses language server.
//!
//! Tiap server memakai tiga task tokio: pembaca stdout (membingkai pesan dan memanggil `on_message`),
//! pembaca stderr (menyimpan ekor keluaran), dan pengawas yang menunggu proses berakhir lalu
//! memanggil `on_exit`.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{watch, Notify};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::error::{LspError, LspErrorCode, Result};
use crate::frame::{encode_frame, FrameDecoder};
use crate::model::{install_hint_for, ServerExitInfo, ServerSpec};
use crate::path::resolve_command;

const READ_CHUNK: usize = 64 * 1024;
/// Ukuran maksimum ekor stderr yang disimpan.
const STDERR_TAIL_BYTES: usize = 4 * 1024;
/// Waktu yang diberikan kepada server untuk keluar sendiri setelah stdin ditutup, sebelum dimatikan paksa.
const STOP_GRACE: Duration = Duration::from_millis(500);
/// Setelah proses berakhir, pembaca diberi waktu ini untuk menghabiskan sisa pipa. Diperlukan
/// karena proses cucu bisa menahan pipa tetap terbuka sehingga EOF tidak pernah tiba.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

/// Mengunci mutex dan mengabaikan status *poisoned*: callback pengguna yang panik
/// tidak boleh membuat seluruh pengelola ikut macet.
fn lock<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Server {
    /// `None` setelah server diminta berhenti atau prosesnya berakhir.
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    /// Permintaan berhenti untuk task pengawas.
    stop: Notify,
    /// Menjadi `true` setelah `on_exit` dipanggil.
    done: watch::Receiver<bool>,
}

impl Server {
    /// Meminta berhenti lalu menunggu sampai `on_exit` selesai dipanggil.
    async fn stop_and_wait(&self) {
        self.stop.notify_one();
        let mut done = self.done.clone();
        // Err berarti task pengawas sudah hilang (runtime dimatikan); tidak ada lagi yang ditunggu.
        let _ = done.wait_for(|done| *done).await;
    }
}

#[derive(Default)]
struct Registry {
    next_id: AtomicU32,
    servers: Mutex<HashMap<u32, Arc<Server>>>,
}

/// Pengelola semua language server milik satu aplikasi. Aman dipakai dari banyak task di balik `Arc`.
/// Saat di-drop, semua server diminta berhenti.
#[derive(Default)]
pub struct LspManager {
    registry: Arc<Registry>,
}

impl LspManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Menjalankan proses server di `cwd` dengan stdio berupa pipa dan mengembalikan id server.
    ///
    /// Galat `ServerNotInstalled` bila perintahnya tidak ditemukan (path relatif yang memuat pemisah
    /// dihitung dari `cwd`). `on_message` dipanggil untuk tiap isi pesan lengkap (teks JSON tanpa
    /// header `Content-Length`) dari stdout server. `on_exit` dipanggil satu kali saat proses
    /// berakhir, juga setelah [`Self::stop`], dan selalu sesudah pesan terakhir.
    ///
    /// Kedua callback dijalankan di task tokio, jadi harus cepat dan tidak memblokir.
    pub async fn start(
        &self,
        spec: &ServerSpec,
        cwd: &Path,
        on_message: impl Fn(String) + Send + Sync + 'static,
        on_exit: impl FnOnce(ServerExitInfo) + Send + 'static,
    ) -> Result<u32> {
        if !cwd.is_dir() {
            return Err(LspError::spawn_failed(format!("Server working directory not found: {}", cwd.display())));
        }
        let program = resolve_command(&spec.command, Some(cwd)).ok_or_else(|| not_installed(&spec.command))?;
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| LspError::spawn_failed("Language servers can only be started from within a tokio runtime"))?;

        let mut command = Command::new(&program);
        command
            .args(&spec.args)
            .envs(spec.env.iter().filter(|(key, _)| !key.is_empty()).map(|(key, value)| (key, value)))
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // CREATE_NO_WINDOW: jangan munculkan jendela konsol untuk server di aplikasi GUI.
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);

        let mut child = command.spawn().map_err(|err| {
            LspError::spawn_failed(format!("Failed to start language server `{}`: {err}", spec.command))
        })?;
        let (Some(stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            // `child` di-drop di sini dan prosesnya dimatikan (kill_on_drop).
            return Err(LspError::spawn_failed(format!("Failed to open stdio pipes for `{}`", spec.command)));
        };

        let (done_tx, done_rx) = watch::channel(false);
        let server = Arc::new(Server {
            stdin: tokio::sync::Mutex::new(Some(stdin)),
            stop: Notify::new(),
            done: done_rx,
        });
        let id = self.registry.next_id.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        lock(&self.registry.servers).insert(id, Arc::clone(&server));

        let stdout_task = runtime.spawn(read_messages(stdout, on_message));
        let stderr_tail = Arc::new(Mutex::new(Vec::new()));
        let stderr_task = runtime.spawn(read_stderr_tail(stderr, Arc::clone(&stderr_tail)));
        let registry = Arc::downgrade(&self.registry);
        runtime.spawn(async move {
            let info = supervise(child, &server, stdout_task, stderr_task, &stderr_tail).await;
            remove(&registry, id);
            on_exit(info);
            let _ = done_tx.send(true);
        });

        Ok(id)
    }

    /// Menulis `Content-Length: N\r\n\r\n<json>` ke stdin server (N = panjang dalam byte).
    ///
    /// Future ini sebaiknya tidak dibatalkan di tengah jalan: pesan yang baru tertulis sebagian
    /// akan merusak aliran ke server.
    pub async fn send(&self, id: u32, json: &str) -> Result<()> {
        let server = lock(&self.registry.servers).get(&id).cloned().ok_or_else(|| LspError::not_found(id))?;
        let frame = encode_frame(json);
        let mut stdin = server.stdin.lock().await;
        let stdin = stdin.as_mut().ok_or_else(|| LspError::not_found(id))?;
        stdin.write_all(&frame).await.map_err(|err| LspError::io("Failed to send message to language server", err))?;
        stdin.flush().await.map_err(|err| LspError::io("Failed to send message to language server", err))
    }

    /// Menghentikan server: stdin ditutup, lalu prosesnya dimatikan bila belum keluar setelah
    /// jeda singkat. Kembali setelah `on_exit` dipanggil. Id yang tidak dikenal bukan galat.
    pub async fn stop(&self, id: u32) {
        let server = lock(&self.registry.servers).remove(&id);
        if let Some(server) = server {
            server.stop_and_wait().await;
        }
    }

    /// Menghentikan semua server secara bersamaan dan menunggu semuanya berakhir.
    pub async fn stop_all(&self) {
        let servers: Vec<Arc<Server>> = lock(&self.registry.servers).drain().map(|(_, server)| server).collect();
        for server in &servers {
            server.stop.notify_one();
        }
        for server in &servers {
            server.stop_and_wait().await;
        }
    }

    /// Id semua server yang masih berjalan, terurut naik.
    pub fn ids(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = lock(&self.registry.servers).keys().copied().collect();
        ids.sort_unstable();
        ids
    }
}

impl Drop for LspManager {
    fn drop(&mut self) {
        // Tidak bisa menunggu di sini; task pengawas menyelesaikan penghentian di latar belakang.
        for (_, server) in lock(&self.registry.servers).drain() {
            server.stop.notify_one();
        }
    }
}

fn not_installed(command: &str) -> LspError {
    let mut message = format!("Language server `{command}` was not found in PATH.");
    if let Some(hint) = install_hint_for(command) {
        message.push(' ');
        message.push_str(hint);
    }
    LspError::new(LspErrorCode::ServerNotInstalled, message)
}

fn remove(registry: &Weak<Registry>, id: u32) {
    if let Some(registry) = registry.upgrade() {
        let removed = lock(&registry.servers).remove(&id);
        drop(removed);
    }
}

async fn read_messages(mut stdout: impl AsyncRead + Unpin, on_message: impl Fn(String)) {
    let mut decoder = FrameDecoder::new();
    let mut buf = vec![0u8; READ_CHUNK];
    loop {
        match stdout.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => decoder.push(&buf[..n]).into_iter().for_each(&on_message),
        }
    }
}

async fn read_stderr_tail(mut stderr: impl AsyncRead + Unpin, tail: Arc<Mutex<Vec<u8>>>) {
    let mut buf = vec![0u8; 8 * 1024];
    loop {
        match stderr.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => push_tail(&mut lock(&tail), &buf[..n], STDERR_TAIL_BYTES),
        }
    }
}

/// Menambahkan `bytes` ke `tail` sambil hanya menyimpan `max` byte terakhir.
fn push_tail(tail: &mut Vec<u8>, bytes: &[u8], max: usize) {
    if bytes.len() >= max {
        tail.clear();
        tail.extend_from_slice(&bytes[bytes.len() - max..]);
        return;
    }
    let overflow = (tail.len() + bytes.len()).saturating_sub(max);
    tail.drain(..overflow);
    tail.extend_from_slice(bytes);
}

/// Mengubah ekor stderr menjadi teks paling banyak `max` byte tanpa memotong karakter UTF-8.
fn tail_to_string(tail: &[u8], max: usize) -> String {
    // Buang byte lanjutan UTF-8 yang terpotong di awal ekor.
    let start = tail.iter().take(3).take_while(|byte| (**byte & 0xC0) == 0x80).count();
    let text = String::from_utf8_lossy(&tail[start..]);
    // Byte tak sah menjadi U+FFFD (3 byte), jadi hasilnya bisa melebihi batas.
    let mut cut = text.len().saturating_sub(max);
    while !text.is_char_boundary(cut) {
        cut += 1;
    }
    text[cut..].to_string()
}

/// Menunggu proses berakhir (sendiri atau karena diminta berhenti), lalu mengumpulkan sisa keluaran.
async fn supervise(
    mut child: Child,
    server: &Server,
    stdout_task: JoinHandle<()>,
    stderr_task: JoinHandle<()>,
    stderr_tail: &Mutex<Vec<u8>>,
) -> ServerExitInfo {
    let status = tokio::select! {
        status = child.wait() => status,
        _ = server.stop.notified() => {
            // Menutup stdin memberi server kesempatan keluar dengan rapi. Bila sedang ada
            // pengiriman yang tertahan, langsung saja dimatikan.
            let closed_stdin = match server.stdin.try_lock() {
                Ok(mut stdin) => {
                    stdin.take();
                    true
                }
                Err(_) => false,
            };
            let graceful = if closed_stdin { timeout(STOP_GRACE, child.wait()).await.ok() } else { None };
            match graceful {
                Some(status) => status,
                None => {
                    let _ = child.start_kill();
                    child.wait().await
                }
            }
        }
    };

    if let Ok(mut stdin) = server.stdin.try_lock() {
        stdin.take();
    }
    tokio::join!(finish_reader(stdout_task), finish_reader(stderr_task));

    ServerExitInfo {
        code: status.ok().and_then(|status| status.code()),
        stderr_tail: tail_to_string(&lock(stderr_tail), STDERR_TAIL_BYTES),
    }
}

/// Menunggu task pembaca selesai; bila pipanya ditahan proses lain, task dibatalkan.
async fn finish_reader(mut task: JoinHandle<()>) {
    if timeout(DRAIN_TIMEOUT, &mut task).await.is_err() {
        task.abort();
        let _ = task.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_keeps_only_the_last_bytes() {
        let mut tail = Vec::new();
        push_tail(&mut tail, b"abcdef", 8);
        assert_eq!(tail, b"abcdef");
        push_tail(&mut tail, b"ghij", 8);
        assert_eq!(tail, b"cdefghij");
        push_tail(&mut tail, b"0123456789ABCDEF", 8);
        assert_eq!(tail, b"89ABCDEF");
        push_tail(&mut tail, b"", 8);
        assert_eq!(tail, b"89ABCDEF");
    }

    #[test]
    fn tail_string_never_splits_a_character() {
        // "é" = C3 A9; ekor dimulai di tengah karakter.
        let bytes = "ééé".as_bytes();
        assert_eq!(tail_to_string(&bytes[1..], 16), "éé");
        assert_eq!(tail_to_string(bytes, 16), "ééé");
        // Byte tak sah membengkak menjadi U+FFFD; hasil tetap dalam batas dan utuh.
        let text = tail_to_string(&[0xff, 0xff, 0xff, b'a'], 4);
        assert!(text.len() <= 4);
        assert!(text.ends_with('a'));
        assert_eq!(tail_to_string(b"", 4), "");
    }

    #[test]
    fn not_installed_message_carries_the_install_hint() {
        let err = not_installed("gopls");
        assert_eq!(err.code, LspErrorCode::ServerNotInstalled);
        assert!(err.message.contains("gopls"));
        assert!(err.message.contains("go install"));
        let err = not_installed("server-buatan-sendiri");
        assert!(err.message.ends_with("was not found in PATH."));
    }

    #[test]
    fn manager_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<LspManager>();
    }
}
