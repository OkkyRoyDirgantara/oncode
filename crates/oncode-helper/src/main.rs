//! Helper program that git runs on behalf of OnCode.
//!
//! - `oncode-helper <prompt>`           : askpass (GIT_ASKPASS / SSH_ASKPASS); the answer is printed to stdout.
//! - `oncode-helper editor <file>`      : GIT_EDITOR; the app fills in the message file.
//! - `oncode-helper sequence <file>`    : GIT_SEQUENCE_EDITOR; the app fills in the rebase todo list.
//!
//! Requests are forwarded to the app over a local socket (`ONCODE_IPC_ENDPOINT`)
//! with a one-time token (`ONCODE_IPC_TOKEN`). Exit code 0 means it was answered,
//! otherwise git treats it as the user canceling.
//!
//! Outside git, the helper also installs and removes the `oncode` command in the terminal
//! (`install-shell-command` / `uninstall-shell-command <app folder>`, see [`shell_command`]);
//! called by the Windows installer and the command palette.

mod shell_command;

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
    // A named pipe is opened like a file; if all instances are busy, retry briefly.
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
    match args.first().map(String::as_str) {
        Some("install-shell-command") => return shell_command::main(true, args.get(1).map(String::as_str)),
        Some("uninstall-shell-command") => return shell_command::main(false, args.get(1).map(String::as_str)),
        _ => {}
    }
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
