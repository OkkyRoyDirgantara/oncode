//! Uji autentikasi SSH sungguhan terhadap `sshd` pribadi di port acak: konfirmasi sidik jari host
//! dan passphrase kunci diminta lewat dialog aplikasi (ssh → oncode-helper sebagai SSH_ASKPASS).
//! `~/.ssh` pengguna tidak disentuh: ssh memakai berkas konfigurasi sementara lewat GIT_SSH_COMMAND.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use oncode_ui::repos;
use oncode_ui::testing::{git, TestApp, TestWindow};

const SSHD: &str = "/usr/sbin/sshd";

/// `GIT_SSH_COMMAND` berlaku untuk seluruh proses uji, jadi uji SSH dijalankan satu per satu.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
const PASSPHRASE: &str = "frasa-rahasia-kunci";

struct Sshd {
    port: u16,
    child: Child,
    ssh_dir: PathBuf,
    _dir: tempfile::TempDir,
}

impl Drop for Sshd {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn start_sshd() -> Option<Sshd> {
    if !Path::new(SSHD).exists() || cfg!(windows) {
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let base = oncode_ui::testing::canonical(dir.path());
    let (server, ssh_dir) = (base.join("sshd"), base.join("ssh"));
    std::fs::create_dir_all(&server).unwrap();
    std::fs::create_dir_all(&ssh_dir).unwrap();
    let keygen = |args: &[&str]| assert!(Command::new("ssh-keygen").args(args).status().unwrap().success());
    keygen(&["-q", "-t", "ed25519", "-N", "", "-f", &server.join("host_key").to_string_lossy()]);
    keygen(&["-q", "-t", "ed25519", "-N", PASSPHRASE, "-f", &ssh_dir.join("id_uji").to_string_lossy()]);
    std::fs::copy(ssh_dir.join("id_uji.pub"), server.join("authorized_keys")).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let config = [
        format!("Port {port}"),
        "ListenAddress 127.0.0.1".into(),
        format!("HostKey {}", server.join("host_key").display()),
        format!("PidFile {}", server.join("sshd.pid").display()),
        format!("AuthorizedKeysFile {}", server.join("authorized_keys").display()),
        "PasswordAuthentication no".into(),
        "KbdInteractiveAuthentication no".into(),
        "UsePAM no".into(),
        "StrictModes no".into(),
    ]
    .join("\n");
    std::fs::write(server.join("sshd_config"), config + "\n").unwrap();
    let child = Command::new(SSHD).args(["-D", "-e", "-f"]).arg(server.join("sshd_config")).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().ok()?;
    for _ in 0..50 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    std::fs::write(
        ssh_dir.join("config"),
        format!(
            "Host *\n  UserKnownHostsFile {}\n  IdentityFile {}\n  IdentitiesOnly yes\n  IdentityAgent none\n",
            ssh_dir.join("known_hosts").display(),
            ssh_dir.join("id_uji").display()
        ),
    )
    .unwrap();
    Some(Sshd { port, child, ssh_dir, _dir: dir })
}

fn user() -> String {
    std::env::var("USER").or_else(|_| std::env::var("LOGNAME")).unwrap_or_else(|_| "root".into())
}

fn remote_head(bare: &Path) -> Option<String> {
    let output = Command::new("git").current_dir(bare).args(["rev-parse", "--verify", "-q", "main"]).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Menunggu dialog lalu mengklik tombol (`ok`/`cancel`) atau mengetik jawaban.
async fn answer(window: &TestWindow, cx: &mut TestAppContext, action: Result<&str, &'static str>) {
    for _ in 0..750 {
        let open = window.update(cx, |window, cx| {
            use gpui_kit::component::WindowExt as _;
            window.has_active_dialog(cx)
        });
        if open {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.run_until_parked();
    }
    cx.run_until_parked();
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        match action {
            Ok(text) => {
                window.input(text, cx);
                window.press("enter", cx);
            }
            Err(button) => window.click(button, cx),
        }
    });
    for _ in 0..25 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.run_until_parked();
    }
}

async fn setup(app: &TestApp, sshd: &Sshd, cx: &mut TestAppContext) -> (TestWindow, PathBuf, tempfile::TempDir) {
    std::env::set_var("GIT_SSH_COMMAND", format!("ssh -F {}", sshd.ssh_dir.join("config").display()));
    cx.update(oncode_ui_scm::init);
    let bare_dir = tempfile::tempdir().unwrap();
    let bare = std::fs::canonicalize(bare_dir.path()).unwrap();
    git(&bare, &["init", "-q", "--bare", "-b", "main"]);
    let project = app.git_project(&[("a.txt", "a\n")]);
    git(&project, &["remote", "add", "origin", &format!("ssh://{}@127.0.0.1:{}{}", user(), sshd.port, bare.display())]);
    let window = app.open(Some(&project), cx).await;
    let root = project.to_string_lossy().into_owned();
    window.wait_until(cx, "snapshot", move |_, cx| repos(cx).read(cx).snapshot(&root).is_some()).await;
    (window, bare, bare_dir)
}

#[gpui_kit::test]
async fn ssh_push_confirms_the_host_and_asks_for_the_passphrase(cx: &mut TestAppContext) {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(sshd) = start_sshd() else { return eprintln!("sshd tidak tersedia; uji dilewati") };
    let app = TestApp::new(cx);
    let (window, bare, _bare_dir) = setup(&app, &sshd, cx).await;

    assert!(window.run_command(cx, "git.push"));
    answer(&window, cx, Err("ok")).await; // sidik jari host disetujui
    answer(&window, cx, Ok(PASSPHRASE)).await;
    window.wait_until(cx, "push lewat SSH berhasil", |_, _| remote_head(&bare).is_some()).await;
    let known = std::fs::read_to_string(sshd.ssh_dir.join("known_hosts")).unwrap_or_default();
    assert!(known.contains(&format!("[127.0.0.1]:{}", sshd.port)), "host yang disetujui dicatat");
}

#[gpui_kit::test]
async fn rejecting_an_unknown_host_stops_the_push(cx: &mut TestAppContext) {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(sshd) = start_sshd() else { return eprintln!("sshd tidak tersedia; uji dilewati") };
    let app = TestApp::new(cx);
    let (window, bare, _bare_dir) = setup(&app, &sshd, cx).await;

    assert!(window.run_command(cx, "git.push"));
    answer(&window, cx, Err("cancel")).await; // sidik jari host ditolak
    for _ in 0..100 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.run_until_parked();
    }
    assert_eq!(remote_head(&bare), None);
    let known = std::fs::read_to_string(sshd.ssh_dir.join("known_hosts")).unwrap_or_default();
    assert!(!known.contains(&format!("[127.0.0.1]:{}", sshd.port)), "host yang ditolak tidak dicatat");
}
