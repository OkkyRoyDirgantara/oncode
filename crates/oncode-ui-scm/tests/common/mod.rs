//! Server git smart-HTTP kecil untuk uji: `git http-backend` lewat CGI, dengan Basic auth.
//! Hanya mendengarkan di 127.0.0.1; tidak ada permintaan ke internet.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

pub struct GitHttpServer {
    pub port: u16,
    pub root: PathBuf,
    /// Jumlah permintaan yang ditolak karena kredensial salah atau tidak ada.
    pub rejected: Arc<AtomicUsize>,
    _dir: tempfile::TempDir,
}

fn base64(input: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

impl GitHttpServer {
    /// Server dengan satu repositori bare kosong `proyek.git` (branch bawaan `main`).
    pub fn start(user: &str, password: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = oncode_ui::testing::canonical(dir.path());
        let bare = root.join("proyek.git");
        std::fs::create_dir_all(&bare).unwrap();
        oncode_ui::testing::git(&bare, &["init", "-q", "--bare", "-b", "main"]);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let expected = format!("Basic {}", base64(format!("{user}:{password}").as_bytes()));
        let rejected = Arc::new(AtomicUsize::new(0));
        let (serve_root, serve_rejected, user) = (root.clone(), rejected.clone(), user.to_string());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (root, expected, rejected, user) = (serve_root.clone(), expected.clone(), serve_rejected.clone(), user.clone());
                std::thread::spawn(move || {
                    let _ = handle(stream, &root, &expected, &user, &rejected);
                });
            }
        });
        Self { port, root, rejected, _dir: dir }
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/proyek.git", self.port)
    }

    pub fn bare(&self) -> PathBuf {
        self.root.join("proyek.git")
    }
}

fn handle(stream: TcpStream, root: &PathBuf, expected: &str, user: &str, rejected: &AtomicUsize) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
    let mut headers: Vec<(String, String)> = Vec::new();
    loop {
        let mut header = String::new();
        reader.read_line(&mut header)?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            headers.push((name.trim().to_lowercase(), value.trim().to_string()));
        }
    }
    let header = |name: &str| headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
    let mut body = Vec::new();
    if let Some(length) = header("content-length").and_then(|l| l.parse::<usize>().ok()) {
        body.resize(length, 0);
        reader.read_exact(&mut body)?;
    } else if header("transfer-encoding").is_some_and(|t| t.contains("chunked")) {
        loop {
            let mut size = String::new();
            reader.read_line(&mut size)?;
            let size = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
            if size == 0 {
                let mut end = String::new();
                reader.read_line(&mut end)?;
                break;
            }
            let mut chunk = vec![0; size];
            reader.read_exact(&mut chunk)?;
            body.extend_from_slice(&chunk);
            let mut crlf = [0; 2];
            reader.read_exact(&mut crlf)?;
        }
    }
    let mut out = stream;
    if header("authorization").as_deref() != Some(expected) {
        rejected.fetch_add(1, Ordering::SeqCst);
        out.write_all(b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"git\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
        return Ok(());
    }
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let mut command = Command::new("git");
    command
        .arg("http-backend")
        .env("GIT_PROJECT_ROOT", root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("PATH_INFO", path)
        .env("QUERY_STRING", query)
        .env("REQUEST_METHOD", &method)
        .env("REMOTE_USER", user)
        .env("REMOTE_ADDR", "127.0.0.1")
        .env("CONTENT_LENGTH", body.len().to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(content_type) = header("content-type") {
        command.env("CONTENT_TYPE", content_type);
    }
    if let Some(encoding) = header("content-encoding") {
        command.env("HTTP_CONTENT_ENCODING", encoding);
    }
    let mut child = command.spawn()?;
    child.stdin.take().unwrap().write_all(&body)?;
    let output = child.wait_with_output()?;
    let raw = output.stdout;
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").map(|i| (i, 4)).or_else(|| raw.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2)));
    let (head, content) = match split {
        Some((index, skip)) => (String::from_utf8_lossy(&raw[..index]).to_string(), &raw[index + skip..]),
        None => (String::new(), &raw[..]),
    };
    let mut status = "200 OK".to_string();
    let mut response = Vec::new();
    for header in head.lines() {
        if let Some(value) = header.strip_prefix("Status:") {
            status = value.trim().to_string();
        } else if !header.trim().is_empty() {
            response.extend_from_slice(header.trim_end().as_bytes());
            response.extend_from_slice(b"\r\n");
        }
    }
    out.write_all(format!("HTTP/1.1 {status}\r\n").as_bytes())?;
    out.write_all(&response)?;
    out.write_all(format!("Content-Length: {}\r\nConnection: close\r\n\r\n", content.len()).as_bytes())?;
    out.write_all(content)?;
    out.flush()
}
