//! A single OnCode process per user, like VS Code.
//!
//! `oncode .` in a terminal runs `oncode-app` again. If OnCode is already running, the new process
//! only sends its arguments and working folder over a local socket and then finishes, before loading the GPU,
//! fonts, or the runtime. The existing process opens a window for that folder or focuses the window that
//! already has it open. Without this, every `oncode .` adds a whole extra process along with its memory.
//!
//! The endpoint is per user: a named pipe on Windows, a Unix socket in `XDG_RUNTIME_DIR` or the temporary
//! folder elsewhere. Debug builds use their own endpoint so they are not forwarded to an installed
//! OnCode.

use std::io::{Read, Write};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt as _, AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _, BufReader};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// Time limit for waiting for the main process's reply; past that this process runs on its own.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);
/// The maximum size of a single request, so a faulty client cannot exhaust memory.
const MAX_REQUEST: u64 = 256 * 1024;

/// The command-line arguments and working folder of the forwarding process.
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

/// Off when run with `ONCODE_DEV_SCRIPT` (debug builds): that script must run in this process.
pub fn enabled() -> bool {
    !(cfg!(debug_assertions) && std::env::var_os("ONCODE_DEV_SCRIPT").is_some())
}

pub fn endpoint() -> String {
    transport::endpoint(if cfg!(debug_assertions) { "-dev" } else { "" })
}

/// Forwards `request` to the OnCode process that is already running. `true` if it was accepted; `false` if
/// there is no other process or it does not reply, and this process runs as usual.
///
/// On macOS and Linux it does not create threads, so it is safe to call before `shell_env`.
pub fn forward(endpoint: &str, request: &Request) -> bool {
    let Some(mut stream) = transport::connect(endpoint) else { return false };
    if stream.write_all(request.to_line().as_bytes()).and_then(|()| stream.flush()).is_err() {
        return false;
    }
    transport::read_reply(stream)
}

/// Becomes the main process: requests from later processes arrive on the returned receiver.
/// `None` if the endpoint cannot be opened (e.g. held by another process); the application keeps running.
pub fn listen(endpoint: &str, runtime: &tokio::runtime::Runtime) -> Option<UnboundedReceiver<Request>> {
    let _context = runtime.enter();
    let (sender, receiver) = unbounded_channel();
    transport::listen(endpoint, sender).then_some(receiver)
}

/// Reads a single request and replies `ok` once the request has entered the queue.
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
        // `XDG_RUNTIME_DIR` and macOS's `TMPDIR` belong to this user only; `/tmp` is shared,
        // so the file name contains the uid and its owner is checked before connecting.
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
                // Another process still replies: leave it. Otherwise it is a leftover from a process that stopped abruptly.
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
        // All pipe instances are serving other clients: retry for a moment.
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

    /// A pipe opened as a file has no read timeout, so the reply is awaited on another
    /// thread. On Windows `shell_env` does nothing, so this thread does not interfere.
    pub fn read_reply(stream: File) -> bool {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(is_ok_reply(stream));
        });
        receiver.recv_timeout(REPLY_TIMEOUT).unwrap_or(false)
    }

    pub fn listen(endpoint: &str, sender: UnboundedSender<Request>) -> bool {
        // `first_pipe_instance` fails if another process already holds this name.
        let Ok(first) = ServerOptions::new().first_pipe_instance(true).reject_remote_clients(true).create(endpoint) else { return false };
        let endpoint = endpoint.to_string();
        tokio::spawn(async move {
            let mut pipe = first;
            loop {
                if pipe.connect().await.is_err() {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                // The next instance is created before serving this client, so there is always one waiting.
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
        let request = Request { cwd: r"C:\code".to_string(), args: vec!["oncode-app".to_string(), ".".to_string(), "folder \"odd\"\n".to_string()] };
        let line = request.to_line();
        assert_eq!(line.matches('\n').count(), 1);
        assert_eq!(Request::parse(&line), Some(request));
        assert_eq!(Request::parse(r#"{"cwd":"/a","args":[1]}"#), None);
    }

    #[test]
    fn second_process_hands_its_request_to_the_first() {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        let endpoint = test_endpoint();
        let request = Request { cwd: "/project".to_string(), args: vec!["oncode-app".to_string(), ".".to_string()] };

        // There is no main process yet: the request cannot be forwarded.
        assert!(!forward(&endpoint, &request));

        let mut requests = listen(&endpoint, &runtime).expect("endpoint is open");
        // The same endpoint cannot be held by two processes.
        assert!(listen(&endpoint, &runtime).is_none());

        for _ in 0..3 {
            assert!(forward(&endpoint, &request));
            assert_eq!(requests.try_recv().ok(), Some(request.clone()));
        }
        #[cfg(unix)]
        let _ = std::fs::remove_file(&endpoint);
    }
}
