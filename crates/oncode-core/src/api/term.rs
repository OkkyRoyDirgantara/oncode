//! Integrated terminal: one PTY per terminal, output streamed through a callback.

use oncode_terminal::SpawnOptions;

use crate::api::Callback;
use crate::error::AppResult;
use crate::state::Shared;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermEvent {
    /// PTY output as is (may be cut in the middle of a multi-byte character); the native terminal
    /// emulator parses these bytes itself. For text, pass it through [`Utf8Stream`].
    Data { data: Vec<u8> },
    Exit { code: Option<i32> },
}

/// Turns a byte stream into text without cutting multi-byte characters at chunk boundaries.
/// A separate utility for users who need text from [`TermEvent::Data`].
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
                        // Invalid byte: replace with U+FFFD and continue.
                        Some(len) => {
                            out.push('\u{FFFD}');
                            start = valid + len;
                        }
                        // Incomplete character at the end of the chunk: keep it for the next chunk.
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

/// Runs a shell in this window's project folder. `on_event` is called from the PTY reader thread.
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

/// User input (keystrokes, pastes, mouse reports) as raw bytes.
pub async fn term_write(core: &Shared, id: u32, data: &[u8]) -> AppResult<()> {
    Ok(core.terminals.write(id, data)?)
}

pub async fn term_resize(core: &Shared, id: u32, cols: u16, rows: u16) -> AppResult<()> {
    Ok(core.terminals.resize(id, cols.max(2), rows.max(1))?)
}

/// Flow control: the UI pauses PTY reading when it falls behind processing the output.
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
        // A truncated multi-byte prefix waits for its continuation.
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

        let project = t.project("project");
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
        term_write(&t.core, id, b"echo \"result-$((6 * 7))-$(pwd)\"; exit 3\n").await.unwrap();

        // Raw bytes are assembled into text with the `Utf8Stream` utility.
        let mut decoder = Utf8Stream::default();
        let mut output = String::new();
        let code = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                match rx.recv().await.expect("terminal channel closed before Exit") {
                    TermEvent::Data { data } => output.push_str(&decoder.push(&data)),
                    TermEvent::Exit { code } => return code,
                }
            }
        })
        .await
        .expect("terminal did not exit");
        assert_eq!(code, Some(3));
        assert!(output.contains(&format!("result-42-{}", project.display())), "output: {output:?}");

        // A terminal that has exited: writing fails, killing again is not an error.
        assert!(term_write(&t.core, id, b"x").await.is_err());
        term_kill(&t.core, "w1", id).await.unwrap();
        assert!(t.core.with_workspace("w1", |ws| ws.terminals.clone()).unwrap().is_empty());
    }
}
