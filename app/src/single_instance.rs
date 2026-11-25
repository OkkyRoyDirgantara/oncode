//! Satu proses OnCode per pengguna, seperti VS Code.
//!
//! `oncode .` di terminal menjalankan `oncode-app` lagi. Bila OnCode sudah berjalan, proses baru
//! hanya mengirim argumen dan folder kerjanya lewat socket lokal lalu selesai, sebelum memuat GPU,
//! font, atau runtime. Proses lama membuka jendela untuk folder itu atau memfokuskan jendela yang
//! sudah membukanya. Tanpa ini setiap `oncode .` menambah satu proses penuh beserta memorinya.
//!
//! Endpoint-nya per pengguna: named pipe di Windows, socket Unix di `XDG_RUNTIME_DIR` atau folder
//! sementara di tempat lain. Build debug memakai endpoint sendiri agar tidak diteruskan ke OnCode
//! yang terpasang.

use std::io::{Read, Write};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt as _, AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _, BufReader};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// Batas waktu menunggu jawaban proses utama; lewat dari itu proses ini berjalan sendiri.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);
/// Ukuran maksimal satu permintaan, agar klien yang salah tidak menghabiskan memori.
const MAX_REQUEST: u64 = 256 * 1024;

/// Argumen baris perintah dan folder kerja proses yang meneruskan.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub cwd: String,
    pub args: Vec<String>,
}

impl Request {
    fn to_line(&self) -> String {
        let mut line = json!({ "cwd": self.cwd, "args": self.args }).to_string();
        line.push('\n');
        line
    }

    fn parse(line: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(line).ok()?;
        let cwd = value.get("cwd")?.as_str()?.to_string();
        let args = value.get("args")?.as_array()?.iter().map(|arg| arg.as_str().map(String::from)).collect::<Option<Vec<_>>>()?;
        Some(Self { cwd, args })
    }
}

/// Mati saat dijalankan dengan `ONCODE_DEV_SCRIPT` (build debug): skrip itu harus berjalan di proses ini.
pub fn enabled() -> bool {
    !(cfg!(debug_assertions) && std::env::var_os("ONCODE_DEV_SCRIPT").is_some())
}

pub fn endpoint() -> String {
    transport::endpoint(if cfg!(debug_assertions) { "-dev" } else { "" })
}

/// Meneruskan `request` ke proses OnCode yang sudah berjalan. `true` bila diterima; `false` bila
/// tidak ada proses lain atau proses itu tidak menjawab, dan proses ini berjalan seperti biasa.
///
/// Di macOS dan Linux tidak membuat thread, sehingga aman dipanggil sebelum `shell_env`.
pub fn forward(endpoint: &str, request: &Request) -> bool {
    let Some(mut stream) = transport::connect(endpoint) else { return false };
    if stream.write_all(request.to_line().as_bytes()).and_then(|()| stream.flush()).is_err() {
        return false;
    }
    transport::read_reply(stream)
}

/// Menjadi proses utama: permintaan dari proses berikutnya masuk ke penerima yang dikembalikan.
/// `None` bila endpoint tidak bisa dibuka (mis. dipegang proses lain); aplikasi tetap berjalan.
pub fn listen(endpoint: &str, runtime: &tokio::runtime::Runtime) -> Option<UnboundedReceiver<Request>> {
    let _context = runtime.enter();
    let (sender, receiver) = unbounded_channel();
    transport::listen(endpoint, sender).then_some(receiver)
}

/// Membaca satu permintaan lalu menjawab `ok` setelah permintaan masuk antrean.
async fn handle<S: AsyncRead + AsyncWrite + Unpin>(stream: S, sender: UnboundedSender<Request>) {
    let (read, mut write) = tokio::io::split(stream);
    let mut line = String::new();
    if BufReader::new(read.take(MAX_REQUEST)).read_line(&mut line).await.is_err() {
        return;
    }
    let Some(request) = Request::parse(&line) else { return };
    if sender.send(request).is_ok() {
        let _ = write.write_all(b"ok\n").await;
        let _ = write.shutdown().await;
    }
}

fn is_ok_reply(mut stream: impl Read) -> bool {
    let mut reply = [0u8; 3];
    stream.read_exact(&mut reply).is_ok() && &reply == b"ok\n"
}

#[cfg(unix)]
mod transport {
    use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;

    use tokio::net::UnixListener;

    use super::*;

    pub fn endpoint(suffix: &str) -> String {
        // `XDG_RUNTIME_DIR` dan `TMPDIR` macOS hanya milik pengguna ini; `/tmp` dipakai bersama,
        // karena itu nama berkas memuat uid dan pemiliknya diperiksa sebelum dihubungi.
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|dir| dir.is_dir()).unwrap_or_else(std::env::temp_dir);
        let uid = unsafe { libc::getuid() };
        dir.join(format!("oncode-{uid}{suffix}.sock")).to_string_lossy().into_owned()
    }

    pub fn connect(endpoint: &str) -> Option<UnixStream> {
        let meta = std::fs::symlink_metadata(endpoint).ok()?;
        if !meta.file_type().is_socket() || meta.uid() != unsafe { libc::getuid() } {
            return None;
        }
        let stream = UnixStream::connect(endpoint).ok()?;
        stream.set_read_timeout(Some(REPLY_TIMEOUT)).ok()?;
        stream.set_write_timeout(Some(REPLY_TIMEOUT)).ok()?;
        Some(stream)
    }

    pub fn read_reply(stream: UnixStream) -> bool {
        is_ok_reply(stream)
    }

    pub fn listen(endpoint: &str, sender: UnboundedSender<Request>) -> bool {
        let listener = match UnixListener::bind(endpoint) {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                // Proses lain masih menjawab: biarkan. Selain itu sisa proses yang berhenti mendadak.
                if UnixStream::connect(endpoint).is_ok() {
                    return false;
                }
                let _ = std::fs::remove_file(endpoint);
                match UnixListener::bind(endpoint) {
                    Ok(listener) => listener,
                    Err(_) => return false,
                }
            }
            Err(_) => return false,
        };
        if std::fs::set_permissions(endpoint, std::fs::Permissions::from_mode(0o600)).is_err() {
            return false;
        }
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        tokio::spawn(handle(stream, sender.clone()));
                    }
                    Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
                }
            }
        });
        true
    }
}

#[cfg(windows)]
mod transport {
    use std::fs::File;

    use tokio::net::windows::named_pipe::ServerOptions;

    use super::*;

    pub fn endpoint(suffix: &str) -> String {
        let user: String = std::env::var("USERNAME").unwrap_or_default().chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
        format!(r"\\.\pipe\oncode-{user}{suffix}")
    }

    pub fn connect(endpoint: &str) -> Option<File> {
        // Semua instance pipe sedang melayani klien lain: coba lagi sebentar.
        const ERROR_PIPE_BUSY: i32 = 231;
        for _ in 0..20 {
            match std::fs::OpenOptions::new().read(true).write(true).open(endpoint) {
                Ok(file) => return Some(file),
                Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => return None,
            }
        }
        None
    }

    /// Pipe yang dibuka sebagai berkas tidak punya batas waktu baca, jadi jawabannya ditunggu di
    /// thread lain. Di Windows `shell_env` tidak berbuat apa-apa, jadi thread ini tidak mengganggu.
    pub fn read_reply(stream: File) -> bool {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(is_ok_reply(stream));
        });
        receiver.recv_timeout(REPLY_TIMEOUT).unwrap_or(false)
    }

    pub fn listen(endpoint: &str, sender: UnboundedSender<Request>) -> bool {
        // `first_pipe_instance` gagal bila proses lain sudah memegang nama ini.
        let Ok(first) = ServerOptions::new().first_pipe_instance(true).reject_remote_clients(true).create(endpoint) else { return false };
        let endpoint = endpoint.to_string();
        tokio::spawn(async move {
            let mut pipe = first;
            loop {
                if pipe.connect().await.is_err() {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                // Instance berikutnya dibuat sebelum melayani klien ini, agar selalu ada yang menunggu.
                let Ok(next) = ServerOptions::new().reject_remote_clients(true).create(&endpoint) else { break };
                let connected = std::mem::replace(&mut pipe, next);
                tokio::spawn(handle(connected, sender.clone()));
            }
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_endpoint() -> String {
        let id = format!("{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        transport::endpoint(&format!("-test-{id}"))
    }

    #[test]
    fn requests_round_trip_as_json_lines() {
        let request = Request { cwd: r"C:\kode".to_string(), args: vec!["oncode-app".to_string(), ".".to_string(), "folder \"aneh\"\n".to_string()] };
        let line = request.to_line();
        assert_eq!(line.matches('\n').count(), 1);
        assert_eq!(Request::parse(&line), Some(request));
        assert_eq!(Request::parse(r#"{"cwd":"/a","args":[1]}"#), None);
    }

    #[test]
    fn second_process_hands_its_request_to_the_first() {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        let endpoint = test_endpoint();
        let request = Request { cwd: "/proyek".to_string(), args: vec!["oncode-app".to_string(), ".".to_string()] };

        // Belum ada proses utama: permintaan tidak bisa diteruskan.
        assert!(!forward(&endpoint, &request));

        let mut requests = listen(&endpoint, &runtime).expect("endpoint terbuka");
        // Endpoint yang sama tidak bisa dipegang dua proses.
        assert!(listen(&endpoint, &runtime).is_none());

        for _ in 0..3 {
            assert!(forward(&endpoint, &request));
            assert_eq!(requests.try_recv().ok(), Some(request.clone()));
        }
        #[cfg(unix)]
        let _ = std::fs::remove_file(&endpoint);
    }
}
