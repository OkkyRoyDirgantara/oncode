//! Uji ujung-ke-ujung dengan server palsu berupa skrip `sh`. Hanya untuk Unix.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use oncode_lsp::{find_on_path, LspErrorCode, LspManager, ServerExitInfo, ServerSpec};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use tokio::time::timeout;

const TIMEOUT: Duration = Duration::from_secs(10);

/// Membaca satu pesan berbingkai dari stdin, membalas dengan pesan berbingkai yang memuat
/// isi pesan tadi, lalu hidup sampai stdin ditutup.
const ECHO_SERVER: &str = r#"
len=0
while IFS= read -r line; do
  line=$(printf '%s' "$line" | tr -d '\r')
  [ -z "$line" ] && break
  case "$line" in
    Content-Length:*) len=$(printf '%s' "${line#Content-Length:}" | tr -d ' ') ;;
  esac
done
body=$(dd bs=1 count="$len" 2>/dev/null)
reply="{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"cwd\":\"$(pwd)\",\"env\":\"$ONCODE_UJI\",\"arg\":\"$1\",\"echo\":$body}}"
n=$(printf '%s' "$reply" | wc -c | tr -d ' ')
printf 'Content-Length: %s\r\n\r\n%s' "$n" "$reply"
note='{"jsonrpc":"2.0","method":"selesai"}'
printf 'Content-Type: application/vscode-jsonrpc; charset=utf-8\r\ncontent-length: %s\r\n\r\n%s' "${#note}" "$note"
cat >/dev/null
echo "stdin ditutup" >&2
exit 0
"#;

struct Running {
    id: u32,
    messages: UnboundedReceiver<String>,
    exit: UnboundedReceiver<ServerExitInfo>,
}

impl Running {
    async fn message(&mut self) -> String {
        timeout(TIMEOUT, self.messages.recv()).await.expect("pesan tidak datang").expect("kanal pesan tertutup")
    }

    async fn exit(&mut self) -> ServerExitInfo {
        timeout(TIMEOUT, self.exit.recv()).await.expect("on_exit tidak dipanggil").expect("kanal exit tertutup")
    }
}

fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).expect("tulis skrip");
    path
}

/// Skrip dijalankan lewat `sh <skrip>` supaya tidak perlu bit eksekusi (dan tidak kena ETXTBSY).
fn sh_spec(script: &Path, extra_args: &[&str]) -> ServerSpec {
    let mut args = vec![script.to_string_lossy().into_owned()];
    args.extend(extra_args.iter().map(|arg| (*arg).to_string()));
    ServerSpec { command: "sh".into(), args, env: vec![] }
}

async fn start(manager: &LspManager, spec: &ServerSpec, cwd: &Path) -> Running {
    let (message_tx, messages) = unbounded_channel();
    let (exit_tx, exit) = unbounded_channel();
    let id = manager
        .start(
            spec,
            cwd,
            move |message| {
                let _ = message_tx.send(message);
            },
            move |info| {
                let _ = exit_tx.send(info);
            },
        )
        .await
        .expect("start server");
    Running { id, messages, exit }
}

#[test]
fn sh_is_on_path() {
    assert!(find_on_path("sh").is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn round_trip_with_a_fake_server() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cwd = dir.path().canonicalize().expect("canonicalize");
    let script = write_script(&cwd, "echo.sh", ECHO_SERVER);
    let mut spec = sh_spec(&script, &["argumen-satu"]);
    spec.env = vec![("ONCODE_UJI".into(), "nilai-env".into())];

    let manager = LspManager::new();
    let mut server = start(&manager, &spec, &cwd).await;
    assert_eq!(manager.ids(), vec![server.id]);

    // Isi pesan memuat karakter multi-byte: Content-Length harus dihitung dalam byte.
    let request = r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":"héllo 日本"}"#;
    manager.send(server.id, request).await.expect("kirim pesan");

    let reply = server.message().await;
    let json: serde_json::Value = serde_json::from_str(&reply).expect("balasan berupa JSON");
    assert_eq!(json["id"], 1);
    assert_eq!(json["result"]["echo"]["params"], "héllo 日本");
    assert_eq!(json["result"]["cwd"], cwd.to_string_lossy().as_ref());
    assert_eq!(json["result"]["env"], "nilai-env");
    assert_eq!(json["result"]["arg"], "argumen-satu");
    assert_eq!(server.message().await, r#"{"jsonrpc":"2.0","method":"selesai"}"#);

    // stop() menutup stdin; server keluar sendiri dengan kode 0 dan on_exit sudah dipanggil saat stop() kembali.
    manager.stop(server.id).await;
    let info = server.exit.try_recv().expect("on_exit dipanggil sebelum stop() kembali");
    assert_eq!(info.code, Some(0));
    assert!(info.stderr_tail.contains("stdin ditutup"), "stderr: {:?}", info.stderr_tail);
    assert!(manager.ids().is_empty());

    // Setelah berhenti: kirim → NotFound, stop lagi tidak apa-apa, on_exit tidak terulang.
    let err = manager.send(server.id, "{}").await.expect_err("server sudah berhenti");
    assert_eq!(err.code, LspErrorCode::NotFound);
    manager.stop(server.id).await;
    assert!(server.exit.recv().await.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_kills_a_server_that_ignores_stdin_eof() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = write_script(dir.path(), "bandel.sh", "echo siap >&2\nexec sleep 30\n");
    let manager = LspManager::new();
    let mut server = start(&manager, &sh_spec(&script, &[]), dir.path()).await;

    let started = Instant::now();
    manager.stop(server.id).await;
    assert!(started.elapsed() < Duration::from_secs(5), "stop() terlalu lama: {:?}", started.elapsed());
    let info = server.exit().await;
    assert_eq!(info.code, None, "proses seharusnya dimatikan oleh sinyal");
    assert!(manager.ids().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn server_that_exits_immediately_reports_stderr_tail() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = write_script(dir.path(), "gagal.sh", "echo 'boom: konfigurasi rusak' >&2\nexit 7\n");
    let manager = LspManager::new();
    let mut server = start(&manager, &sh_spec(&script, &[]), dir.path()).await;

    let info = server.exit().await;
    assert_eq!(info.code, Some(7));
    assert_eq!(info.stderr_tail.trim(), "boom: konfigurasi rusak");
    assert!(manager.ids().is_empty());
    let err = manager.send(server.id, "{}").await.expect_err("server sudah mati");
    assert_eq!(err.code, LspErrorCode::NotFound);
}

#[tokio::test(flavor = "multi_thread")]
async fn stderr_tail_is_capped_at_4_kib() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = write_script(
        dir.path(),
        "cerewet.sh",
        "i=0\nwhile [ $i -lt 400 ]; do echo \"baris stderr nomor $i é\" >&2; i=$((i+1)); done\necho AKHIR >&2\nexit 2\n",
    );
    let manager = LspManager::new();
    let mut server = start(&manager, &sh_spec(&script, &[]), dir.path()).await;

    let info = server.exit().await;
    assert_eq!(info.code, Some(2));
    assert!(info.stderr_tail.len() <= 4096, "ekor stderr {} byte", info.stderr_tail.len());
    assert!(info.stderr_tail.len() > 3000);
    assert!(info.stderr_tail.ends_with("AKHIR\n"));
    assert!(!info.stderr_tail.contains("nomor 0 "));
    assert!(!info.stderr_tail.contains('\u{fffd}'));
}

#[tokio::test(flavor = "multi_thread")]
async fn messages_before_exit_are_delivered_before_on_exit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = write_script(
        dir.path(),
        "pamit.sh",
        "printf 'Content-Length: 7\\r\\n\\r\\n{\"n\":1}Content-Length: 7\\r\\n\\r\\n{\"n\":2}'\nexit 0\n",
    );
    let manager = LspManager::new();
    let mut server = start(&manager, &sh_spec(&script, &[]), dir.path()).await;

    let info = server.exit().await;
    assert_eq!(info.code, Some(0));
    assert_eq!(server.messages.try_recv().ok().as_deref(), Some("{\"n\":1}"));
    assert_eq!(server.messages.try_recv().ok().as_deref(), Some("{\"n\":2}"));
}

#[tokio::test(flavor = "multi_thread")]
async fn exit_is_reported_even_if_a_grandchild_keeps_the_pipes_open() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = write_script(dir.path(), "cucu.sh", "sleep 4 &\necho 'induk keluar' >&2\nexit 9\n");
    let manager = LspManager::new();
    let mut server = start(&manager, &sh_spec(&script, &[]), dir.path()).await;

    let started = Instant::now();
    let info = server.exit().await;
    assert!(started.elapsed() < Duration::from_millis(3500), "on_exit menunggu proses cucu");
    assert_eq!(info.code, Some(9));
    assert!(info.stderr_tail.contains("induk keluar"));
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_command_is_server_not_installed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = LspManager::new();
    for command in ["oncode-server-yang-tidak-ada", "./bin/tidak-ada", "/tidak/ada/server"] {
        let spec = ServerSpec { command: command.into(), args: vec![], env: vec![] };
        let err = manager.start(&spec, dir.path(), |_| {}, |_| {}).await.expect_err("perintah tidak ada");
        assert_eq!(err.code, LspErrorCode::ServerNotInstalled, "perintah {command}");
        assert!(err.message.contains(command));
    }
    assert!(manager.ids().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_cwd_is_spawn_failed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = LspManager::new();
    let spec = ServerSpec { command: "sh".into(), args: vec![], env: vec![] };
    let err = manager.start(&spec, &dir.path().join("hilang"), |_| {}, |_| {}).await.expect_err("cwd tidak ada");
    assert_eq!(err.code, LspErrorCode::SpawnFailed);
}

#[tokio::test(flavor = "multi_thread")]
async fn relative_command_is_resolved_against_cwd() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join("bin")).expect("mkdir");
    // Symlink ke `sh` asli: tidak ada berkas program yang ditulis, jadi tidak mungkin kena ETXTBSY.
    let real_sh = find_on_path("sh").expect("sh ada di PATH");
    std::os::unix::fs::symlink(real_sh, dir.path().join("bin").join("server")).expect("symlink");
    let script = write_script(dir.path(), "relatif.sh", "echo 'dari bin' >&2\nexit 3\n");

    let manager = LspManager::new();
    let spec = ServerSpec {
        command: "./bin/server".into(),
        args: vec![script.to_string_lossy().into_owned()],
        env: vec![],
    };
    let mut server = start(&manager, &spec, dir.path()).await;
    let info = server.exit().await;
    assert_eq!(info.code, Some(3));
    assert!(info.stderr_tail.contains("dari bin"));
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_all_and_unknown_ids() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = write_script(dir.path(), "diam.sh", "cat >/dev/null\n");
    let manager = LspManager::new();
    let spec = sh_spec(&script, &[]);
    let mut a = start(&manager, &spec, dir.path()).await;
    let mut b = start(&manager, &spec, dir.path()).await;
    assert_ne!(a.id, b.id);
    assert_eq!(manager.ids(), vec![a.id.min(b.id), a.id.max(b.id)]);

    manager.stop(4242).await;
    assert_eq!(manager.send(4242, "{}").await.expect_err("id asing").code, LspErrorCode::NotFound);

    manager.stop_all().await;
    assert!(manager.ids().is_empty());
    assert_eq!(a.exit.try_recv().expect("on_exit a").code, Some(0));
    assert_eq!(b.exit.try_recv().expect("on_exit b").code, Some(0));
    manager.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_manager_stops_servers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = write_script(dir.path(), "diam.sh", "cat >/dev/null\n");
    let manager = LspManager::new();
    let mut server = start(&manager, &sh_spec(&script, &[]), dir.path()).await;
    drop(manager);
    assert_eq!(server.exit().await.code, Some(0));
}

/// Uji integrasi dengan rust-analyzer sungguhan: kirim `initialize`, terima balasan berisi `capabilities`.
/// Dilewati bila rust-analyzer tidak terpasang.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "memerlukan rust-analyzer terpasang; jalankan dengan --ignored"]
async fn rust_analyzer_answers_initialize() {
    let command = find_on_path("rust-analyzer").or_else(|| {
        let home = std::env::var_os("HOME")?;
        find_on_path(&Path::new(&home).join(".cargo/bin/rust-analyzer").to_string_lossy())
    });
    let Some(command) = command else {
        eprintln!("rust-analyzer tidak terpasang; uji dilewati");
        return;
    };

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().canonicalize().expect("canonicalize");
    std::fs::create_dir(root.join("src")).expect("mkdir");
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"contoh\"\nversion = \"0.1.0\"\nedition = \"2021\"\n")
        .expect("tulis Cargo.toml");
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("tulis main.rs");

    let manager = LspManager::new();
    let spec = ServerSpec { command: command.to_string_lossy().into_owned(), args: vec![], env: vec![] };
    let mut server = start(&manager, &spec, &root).await;

    let initialize = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "processId": std::process::id(),
            "rootUri": format!("file://{}", root.display()),
            "capabilities": {},
        },
    });
    let started = Instant::now();
    manager.send(server.id, &initialize.to_string()).await.expect("kirim initialize");

    let response = loop {
        let message = timeout(Duration::from_secs(60), server.messages.recv())
            .await
            .expect("rust-analyzer tidak membalas initialize");
        let Some(message) = message else {
            // Mis. proxy rustup tanpa komponen rust-analyzer: prosesnya langsung keluar.
            let info = server.exit().await;
            panic!("rust-analyzer keluar sebelum membalas (kode {:?}): {}", info.code, info.stderr_tail.trim());
        };
        let json: serde_json::Value = serde_json::from_str(&message).expect("pesan berupa JSON");
        if json["id"] == 1 {
            break json;
        }
    };
    eprintln!("initialize dibalas dalam {:?}", started.elapsed());
    assert!(response["result"]["capabilities"].is_object(), "balasan: {response}");

    manager.send(server.id, r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#).await.expect("initialized");
    manager.send(server.id, r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#).await.expect("shutdown");
    manager.send(server.id, r#"{"jsonrpc":"2.0","method":"exit"}"#).await.expect("exit");
    manager.stop(server.id).await;
    let info = server.exit().await;
    eprintln!("rust-analyzer keluar: {info:?}");
    assert!(manager.ids().is_empty());
}
