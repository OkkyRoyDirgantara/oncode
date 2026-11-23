//! Terminal terintegrasi: satu PTY per terminal, keluaran dialirkan lewat callback.

use oncode_terminal::SpawnOptions;

use crate::api::Callback;
use crate::error::AppResult;
use crate::state::Shared;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermEvent {
    /// Keluaran PTY apa adanya (bisa terpotong di tengah karakter multi-byte); emulator terminal
    /// native mengurai byte ini sendiri. Untuk teks, lewatkan ke [`Utf8Stream`].
    Data { data: Vec<u8> },
    Exit { code: Option<i32> },
}

/// Mengubah aliran byte menjadi teks tanpa memotong karakter multi-byte di batas potongan.
/// Utilitas terpisah untuk pemakai yang butuh teks dari [`TermEvent::Data`].
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

/// Menjalankan shell di folder proyek jendela ini. `on_event` dipanggil dari thread pembaca PTY.
pub async fn term_spawn(core: &Shared, window: &str, cols: u16, rows: u16, on_event: Callback<TermEvent>) -> AppResult<u32> {
    let root = core.workspace_root(window)?;
    let settings = core.effective_settings(window);
    let options = SpawnOptions {
        shell: settings.terminal.shell.filter(|s| !s.trim().is_empty()),
        args: Vec::new(),
        cwd: root.to_string_lossy().into_owned(),
        cols: cols.max(2),
        rows: rows.max(1),
        env: Vec::new(),
    };
    let on_data = on_event.clone();
    let id = core.terminals.spawn(
        options,
        move |data| {
            if !data.is_empty() {
                on_data(TermEvent::Data { data });
            }
        },
        move |code| on_event(TermEvent::Exit { code }),
    )?;
    let _ = core.with_workspace(window, |ws| ws.terminals.push(id));
    Ok(id)
}

/// Masukan pengguna (ketikan, tempelan, laporan mouse) sebagai byte mentah.
pub async fn term_write(core: &Shared, id: u32, data: &[u8]) -> AppResult<()> {
    Ok(core.terminals.write(id, data)?)
}

pub async fn term_resize(core: &Shared, id: u32, cols: u16, rows: u16) -> AppResult<()> {
    Ok(core.terminals.resize(id, cols.max(2), rows.max(1))?)
}

/// Kendali aliran: UI menjeda pembacaan PTY saat tertinggal memproses keluaran.
pub async fn term_set_paused(core: &Shared, id: u32, paused: bool) -> AppResult<()> {
    Ok(core.terminals.set_paused(id, paused)?)
}

pub async fn term_kill(core: &Shared, window: &str, id: u32) -> AppResult<()> {
    core.terminals.kill(id);
    let _ = core.with_workspace(window, |ws| ws.terminals.retain(|t| *t != id));
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

#[cfg(all(test, unix))]
mod pty_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::api::app::workspace_open;
    use crate::testing::TestCore;

    #[tokio::test]
    async fn spawn_delivers_raw_output_and_the_exit_code() {
        let t = TestCore::new();
        let noop: Callback<TermEvent> = Arc::new(|_| {});
        assert_eq!(term_spawn(&t.core, "w1", 80, 24, noop).await.unwrap_err().code, "noWorkspace");

        let project = t.project("proyek");
        let mut settings = t.core.user_settings();
        settings.terminal.shell = Some("/bin/sh".into());
        t.core.set_user_settings(settings).unwrap();
        workspace_open(&t.core, "w1", project.to_string_lossy().into_owned()).await.unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let id = term_spawn(
            &t.core,
            "w1",
            80,
            24,
            Arc::new(move |event| {
                let _ = tx.send(event);
            }),
        )
        .await
        .unwrap();
        assert_eq!(t.core.with_workspace("w1", |ws| ws.terminals.clone()).unwrap(), vec![id]);

        term_resize(&t.core, id, 100, 30).await.unwrap();
        term_write(&t.core, id, b"echo \"hasil-$((6 * 7))-$(pwd)\"; exit 3\n").await.unwrap();

        // Byte mentah dirangkai menjadi teks dengan utilitas `Utf8Stream`.
        let mut decoder = Utf8Stream::default();
        let mut output = String::new();
        let code = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                match rx.recv().await.expect("kanal terminal tertutup sebelum Exit") {
                    TermEvent::Data { data } => output.push_str(&decoder.push(&data)),
                    TermEvent::Exit { code } => return code,
                }
            }
        })
        .await
        .expect("terminal tidak keluar");
        assert_eq!(code, Some(3));
        assert!(output.contains(&format!("hasil-42-{}", project.display())), "keluaran: {output:?}");

        // Terminal yang sudah keluar: menulis gagal, mematikan lagi bukan galat.
        assert!(term_write(&t.core, id, b"x").await.is_err());
        term_kill(&t.core, "w1", id).await.unwrap();
        assert!(t.core.with_workspace("w1", |ws| ws.terminals.clone()).unwrap().is_empty());
    }
}
