//! Integration tests with a real PTY. Unix only because it uses `/bin/sh`.
#![cfg(unix)]

use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use oncode_terminal::{SpawnOptions, TermErrorCode, TerminalManager};

const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Default)]
struct Output {
    bytes: Mutex<Vec<u8>>,
    changed: Condvar,
}

impl Output {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes.lock().expect("output lock")).into_owned()
    }

    /// Waits until the output contains `needle`; panics with the output content if it does not appear.
    fn wait_for(&self, needle: &str) -> String {
        let deadline = Instant::now() + TIMEOUT;
        let mut bytes = self.bytes.lock().expect("output lock");
        loop {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if text.contains(needle) {
                return text;
            }
            let now = Instant::now();
            assert!(now < deadline, "`{needle}` did not appear in the output: {text:?}");
            bytes = self.changed.wait_timeout(bytes, deadline - now).expect("output lock").0;
        }
    }
}

struct Term {
    manager: Arc<TerminalManager>,
    id: u32,
    output: Arc<Output>,
    exit: Receiver<Option<i32>>,
}

impl Term {
    fn spawn(options: SpawnOptions) -> Self {
        Self::spawn_in(Arc::new(TerminalManager::new()), options)
    }

    fn spawn_in(manager: Arc<TerminalManager>, options: SpawnOptions) -> Self {
        let output = Arc::new(Output::default());
        let sink = Arc::clone(&output);
        let (exit_tx, exit) = mpsc::channel();
        let id = manager
            .spawn(
                options,
                move |data| {
                    sink.bytes.lock().expect("output lock").extend_from_slice(&data);
                    sink.changed.notify_all();
                },
                move |code| {
                    let _ = exit_tx.send(code);
                },
            )
            .expect("spawn terminal");
        Self { manager, id, output, exit }
    }

    fn send(&self, text: &str) {
        self.manager.write(self.id, text.as_bytes()).expect("write to terminal");
    }

    fn wait_exit(&self) -> Option<i32> {
        self.exit.recv_timeout(TIMEOUT).expect("on_exit was not called")
    }
}

fn sh(cwd: &std::path::Path) -> SpawnOptions {
    SpawnOptions {
        shell: Some("/bin/sh".into()),
        cwd: cwd.to_string_lossy().into_owned(),
        cols: 80,
        rows: 24,
        ..SpawnOptions::default()
    }
}

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

#[test]
fn echoes_command_output() {
    let dir = tempdir();
    let term = Term::spawn(sh(dir.path()));
    term.send("echo marker-$((6*7))\n");
    term.output.wait_for("marker-42");
    assert_eq!(term.manager.ids(), vec![term.id]);
}

#[test]
fn resize_is_visible_to_the_child() {
    let dir = tempdir();
    let term = Term::spawn(sh(dir.path()));
    term.send("stty size\n");
    term.output.wait_for("24 80");
    term.manager.resize(term.id, 100, 30).expect("resize");
    term.send("stty size\n");
    term.output.wait_for("30 100");
}

#[test]
fn exit_code_is_reported_and_id_removed() {
    let dir = tempdir();
    let term = Term::spawn(sh(dir.path()));
    term.send("echo before-$((1+1))\n");
    term.send("exit 3\n");
    assert_eq!(term.wait_exit(), Some(3));
    // All output was delivered before on_exit.
    assert!(term.output.text().contains("before-2"));
    assert!(term.manager.ids().is_empty());
    // on_exit only once: the sender has been dropped, there is no second message.
    assert!(term.exit.recv_timeout(Duration::from_millis(200)).is_err());
    let err = term.manager.write(term.id, b"x").expect_err("terminal already exited");
    assert_eq!(err.code, TermErrorCode::NotFound);
}

#[test]
fn cwd_is_honoured() {
    let dir = tempdir();
    let real = dir.path().canonicalize().expect("canonicalize");
    let term = Term::spawn(sh(&real));
    term.send("pwd\n");
    term.output.wait_for(&real.to_string_lossy());
}

#[test]
fn missing_cwd_is_a_spawn_error() {
    let dir = tempdir();
    let manager = TerminalManager::new();
    let options = sh(&dir.path().join("does-not-exist"));
    let err = manager.spawn(options, |_| {}, |_| {}).expect_err("cwd does not exist");
    assert_eq!(err.code, TermErrorCode::SpawnFailed);
    assert!(manager.ids().is_empty());
}

#[test]
fn missing_shell_is_a_spawn_error() {
    let dir = tempdir();
    let manager = TerminalManager::new();
    let mut options = sh(dir.path());
    options.shell = Some("/does/not/exist/shell-oncode".into());
    let err = manager.spawn(options, |_| {}, |_| {}).expect_err("shell does not exist");
    assert_eq!(err.code, TermErrorCode::SpawnFailed);
    assert!(manager.ids().is_empty());
}

#[test]
fn env_is_passed_to_the_shell() {
    let dir = tempdir();
    let mut options = sh(dir.path());
    options.env = vec![("ONCODE_TEST".into(), "value".into())];
    let term = Term::spawn(options);
    term.send("echo \"env=$ONCODE_TEST/$TERM/$COLORTERM/$TERM_PROGRAM.\"\n");
    term.output.wait_for("env=value/xterm-256color/truecolor/OnCode.");
}

#[test]
fn args_are_passed_to_the_shell() {
    let dir = tempdir();
    let mut options = sh(dir.path());
    options.args = vec!["-c".into(), "echo args-$((40+2)); exit 5".into()];
    let term = Term::spawn(options);
    assert_eq!(term.wait_exit(), Some(5));
    assert!(term.output.text().contains("args-42"));
}

#[test]
fn paused_terminal_holds_output_until_resumed() {
    let dir = tempdir();
    let term = Term::spawn(sh(dir.path()));
    term.send("echo start-$((1+1))\n");
    term.output.wait_for("start-2");

    term.manager.set_paused(term.id, true).expect("pause");
    term.send("echo hold-$((20+3))\n");
    std::thread::sleep(Duration::from_millis(400));
    assert!(!term.output.text().contains("hold-23"), "output leaked while paused");

    term.manager.set_paused(term.id, false).expect("resume");
    term.output.wait_for("hold-23");
}

#[test]
fn exit_while_paused_delivers_output_after_resume() {
    let dir = tempdir();
    let term = Term::spawn(sh(dir.path()));
    term.send("echo start-$((1+1))\n");
    term.output.wait_for("start-2");

    term.manager.set_paused(term.id, true).expect("pause");
    term.send("echo final-$((30+3)); exit 4\n");
    assert!(term.exit.recv_timeout(Duration::from_millis(600)).is_err(), "on_exit came before the output");

    // The process has exited, but the remaining output is only delivered after resuming.
    assert_eq!(term.manager.ids(), vec![term.id]);
    term.manager.set_paused(term.id, false).expect("resume");
    assert_eq!(term.wait_exit(), Some(4));
    assert!(term.output.text().contains("final-33"));
}

#[test]
fn kill_triggers_on_exit() {
    let dir = tempdir();
    let term = Term::spawn(sh(dir.path()));
    term.send("echo ready-$((1+1))\n");
    term.output.wait_for("ready-2");
    term.manager.kill(term.id);
    assert!(term.manager.ids().is_empty());
    term.wait_exit();
    // No output after on_exit.
    let before = term.output.text();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(term.output.text(), before);
    // A second kill and an unknown id are fine.
    term.manager.kill(term.id);
    term.manager.kill(9999);
}

#[test]
fn kill_stops_a_child_that_ignores_sighup() {
    let dir = tempdir();
    let term = Term::spawn(sh(dir.path()));
    term.send("trap '' HUP; echo immune-$((1+1))\n");
    term.output.wait_for("immune-2");
    let started = Instant::now();
    term.manager.kill(term.id);
    assert_eq!(term.wait_exit(), None, "the process should have been stopped by SIGKILL");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn kill_while_paused_still_exits() {
    let dir = tempdir();
    let term = Term::spawn(sh(dir.path()));
    term.send("echo ready-$((1+1))\n");
    term.output.wait_for("ready-2");
    term.manager.set_paused(term.id, true).expect("pause");
    term.manager.kill(term.id);
    term.wait_exit();
}

#[test]
fn exit_is_reported_even_if_a_background_job_keeps_the_pty_open() {
    let dir = tempdir();
    let term = Term::spawn(sh(dir.path()));
    term.send("sleep 3 &\n");
    term.send("exit 0\n");
    let started = Instant::now();
    assert_eq!(term.wait_exit(), Some(0));
    assert!(started.elapsed() < Duration::from_millis(2500), "on_exit waited for the background process");
}

#[test]
fn unknown_id_is_not_found() {
    let manager = TerminalManager::new();
    assert_eq!(manager.write(42, b"x").expect_err("write").code, TermErrorCode::NotFound);
    assert_eq!(manager.resize(42, 80, 24).expect_err("resize").code, TermErrorCode::NotFound);
    assert_eq!(manager.set_paused(42, true).expect_err("set_paused").code, TermErrorCode::NotFound);
    manager.kill(42);
    manager.kill_all();
    assert!(manager.ids().is_empty());
}

#[test]
fn kill_all_and_drop_stop_every_terminal() {
    let dir = tempdir();
    let manager = Arc::new(TerminalManager::new());
    let a = Term::spawn_in(Arc::clone(&manager), sh(dir.path()));
    let b = Term::spawn_in(Arc::clone(&manager), sh(dir.path()));
    assert_ne!(a.id, b.id);
    assert_eq!(manager.ids(), vec![a.id.min(b.id), a.id.max(b.id)]);
    manager.kill_all();
    assert!(manager.ids().is_empty());
    a.wait_exit();
    b.wait_exit();

    // Dropping the manager stops the remaining terminals.
    let c = Term::spawn(sh(dir.path()));
    let Term { manager: owned, exit, .. } = c;
    drop(owned);
    exit.recv_timeout(TIMEOUT).expect("on_exit after drop");
}

#[test]
fn large_output_arrives_complete() {
    let dir = tempdir();
    let term = Term::spawn(sh(dir.path()));
    // Over 200 KiB: exceeds a single read chunk and the PTY buffer.
    term.send("i=0; while [ $i -lt 4000 ]; do echo \"line-$i-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\"; i=$((i+1)); done; echo done-$((2*4))\n");
    let text = term.output.wait_for("done-8");
    assert!(text.contains("line-0-x"));
    assert!(text.contains("line-3999-x"));
}
