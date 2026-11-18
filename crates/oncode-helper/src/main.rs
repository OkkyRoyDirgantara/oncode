//! Program pembantu yang dijalankan git atas nama OnCode.
//!
//! - `oncode-helper <prompt>`           : askpass (GIT_ASKPASS / SSH_ASKPASS); jawaban dicetak ke stdout.
//! - `oncode-helper editor <berkas>`    : GIT_EDITOR; aplikasi mengisi berkas pesan.
//! - `oncode-helper sequence <berkas>`  : GIT_SEQUENCE_EDITOR; aplikasi mengisi daftar todo rebase.
//!
//! Permintaan diteruskan ke aplikasi lewat socket lokal (`ONCODE_IPC_ENDPOINT`)
//! dengan token sekali pakai (`ONCODE_IPC_TOKEN`). Kode keluar 0 berarti dijawab,
//! selain itu git menganggap pengguna membatalkan.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::ExitCode;

fn absolute(path: &str) -> String {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        path.to_string()
    } else {
        std::env::current_dir().map(|d| d.join(p).to_string_lossy().into_owned()).unwrap_or_else(|_| path.to_string())
    }
}

#[cfg(unix)]
fn connect(endpoint: &str) -> std::io::Result<impl Read + Write> {
    std::os::unix::net::UnixStream::connect(endpoint)
}

#[cfg(windows)]
fn connect(endpoint: &str) -> std::io::Result<impl Read + Write> {
    // Named pipe dibuka seperti berkas; bila semua instance sibuk, coba lagi sebentar.
    const ERROR_PIPE_BUSY: i32 = 231;
    let mut attempts = 0;
    loop {
        match std::fs::OpenOptions::new().read(true).write(true).open(endpoint) {
            Ok(file) => return Ok(file),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && attempts < 50 => {
                attempts += 1;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => return Err(e),
        }
    }
}

fn request(kind: &str, text: &str) -> Option<String> {
    let endpoint = std::env::var("ONCODE_IPC_ENDPOINT").ok()?;
    let token = std::env::var("ONCODE_IPC_TOKEN").ok()?;
    let mut stream = connect(&endpoint).ok()?;
    let mut line = serde_json::json!({ "token": token, "kind": kind, "text": text }).to_string();
    line.push('\n');
    stream.write_all(line.as_bytes()).ok()?;
    stream.flush().ok()?;
    let mut response = String::new();
    BufReader::new(stream).read_line(&mut response).ok()?;
    let value: serde_json::Value = serde_json::from_str(&response).ok()?;
    if value.get("ok")?.as_bool()? {
        Some(value.get("value")?.as_str()?.to_string())
    } else {
        None
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (kind, text) = match args.first().map(String::as_str) {
        Some("editor") => ("editor", args.get(1).map(|p| absolute(p))),
        Some("sequence") => ("sequence", args.get(1).map(|p| absolute(p))),
        Some(prompt) => ("askpass", Some(prompt.to_string())),
        None => ("askpass", None),
    };
    let Some(text) = text else {
        eprintln!("oncode-helper: missing argument");
        return ExitCode::from(2);
    };
    match request(kind, &text) {
        Some(value) => {
            if kind == "askpass" {
                println!("{value}");
            }
            ExitCode::SUCCESS
        }
        None => ExitCode::FAILURE,
    }
}
