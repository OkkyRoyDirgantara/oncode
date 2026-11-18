//! Terminal terintegrasi: satu PTY per terminal, keluaran dialirkan lewat channel.

use std::sync::Mutex;

use oncode_terminal::SpawnOptions;
use serde::Serialize;
use specta::Type;
use tauri::ipc::Channel;
use tauri::{State, WebviewWindow};

use crate::error::AppResult;
use crate::state::Shared;

#[derive(Debug, Clone, Serialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TermEvent {
    Data { data: String },
    Exit { code: Option<i32> },
}

/// Mengubah aliran byte menjadi teks tanpa memotong karakter multi-byte di batas potongan.
#[derive(Default)]
pub struct Utf8Stream {
    pending: Vec<u8>,
}

impl Utf8Stream {
    pub fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let mut out = String::with_capacity(self.pending.len());
        let mut start = 0;
        loop {
            match std::str::from_utf8(&self.pending[start..]) {
                Ok(text) => {
                    out.push_str(text);
                    start = self.pending.len();
                    break;
                }
                Err(e) => {
                    let valid = start + e.valid_up_to();
                    out.push_str(&String::from_utf8_lossy(&self.pending[start..valid]));
                    match e.error_len() {
                        // Byte tidak sah: ganti dengan U+FFFD lalu lanjutkan.
                        Some(len) => {
                            out.push('\u{FFFD}');
                            start = valid + len;
                        }
                        // Karakter belum lengkap di akhir potongan: simpan untuk potongan berikutnya.
                        None => {
                            start = valid;
                            break;
                        }
                    }
                }
            }
        }
        self.pending.drain(..start);
        out
    }
}

#[tauri::command]
#[specta::specta]
pub async fn term_spawn(window: WebviewWindow, state: State<'_, Shared>, cols: u16, rows: u16, on_event: Channel<TermEvent>) -> AppResult<u32> {
    let label = window.label().to_string();
    let root = state.workspace_root(&label)?;
    let settings = state.effective_settings(&label);
    let options = SpawnOptions {
        shell: settings.terminal.shell.filter(|s| !s.trim().is_empty()),
        args: Vec::new(),
        cwd: root.to_string_lossy().into_owned(),
        cols: cols.max(2),
        rows: rows.max(1),
        env: Vec::new(),
    };
    let decoder = Mutex::new(Utf8Stream::default());
    let data_channel = on_event.clone();
    let id = state.terminals.spawn(
        options,
        move |bytes| {
            let data = decoder.lock().unwrap().push(&bytes);
            if !data.is_empty() {
                let _ = data_channel.send(TermEvent::Data { data });
            }
        },
        move |code| {
            let _ = on_event.send(TermEvent::Exit { code });
        },
    )?;
    let _ = state.with_workspace(&label, |ws| ws.terminals.push(id));
    Ok(id)
}

#[tauri::command]
#[specta::specta]
pub async fn term_write(state: State<'_, Shared>, id: u32, data: String) -> AppResult<()> {
    Ok(state.terminals.write(id, data.as_bytes())?)
}

#[tauri::command]
#[specta::specta]
pub async fn term_resize(state: State<'_, Shared>, id: u32, cols: u16, rows: u16) -> AppResult<()> {
    Ok(state.terminals.resize(id, cols.max(2), rows.max(1))?)
}

/// Kendali aliran: UI menjeda pembacaan PTY saat tertinggal memproses keluaran.
#[tauri::command]
#[specta::specta]
pub async fn term_set_paused(state: State<'_, Shared>, id: u32, paused: bool) -> AppResult<()> {
    Ok(state.terminals.set_paused(id, paused)?)
}

#[tauri::command]
#[specta::specta]
pub async fn term_kill(window: WebviewWindow, state: State<'_, Shared>, id: u32) -> AppResult<()> {
    state.terminals.kill(id);
    let _ = state.with_workspace(window.label(), |ws| ws.terminals.retain(|t| *t != id));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_split_multibyte_characters_intact() {
        let mut s = Utf8Stream::default();
        let bytes = "aé€😀z".as_bytes();
        let mut out = String::new();
        for chunk in bytes.chunks(1) {
            out.push_str(&s.push(chunk));
        }
        assert_eq!(out, "aé€😀z");
    }

    #[test]
    fn replaces_invalid_bytes_and_continues() {
        let mut s = Utf8Stream::default();
        assert_eq!(s.push(&[b'a', 0xFF, b'b']), "a\u{FFFD}b");
        // Awalan multi-byte yang terpotong menunggu lanjutan.
        assert_eq!(s.push(&[0xE2, 0x82]), "");
        assert_eq!(s.push(&[0xAC, b'!']), "€!");
    }
}
