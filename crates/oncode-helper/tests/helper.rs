//! Uji ujung ke ujung: git → oncode-helper → HelperServer → jawaban aplikasi.

use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use oncode_git::*;

const HELPER: &str = env!("CARGO_BIN_EXE_oncode-helper");

fn isolate() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        for (key, value) in [
            // Berkas kosong milik platform: `NUL` di Windows.
            ("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" }),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_AUTHOR_NAME", "Penguji"),
            ("GIT_AUTHOR_EMAIL", "penguji@contoh.id"),
            ("GIT_COMMITTER_NAME", "Penguji"),
            ("GIT_COMMITTER_EMAIL", "penguji@contoh.id"),
        ] {
            std::env::set_var(key, value);
        }
    });
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git").current_dir(cwd).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn commit(cwd: &Path, file: &str, content: &str, message: &str) {
    std::fs::write(cwd.join(file), content).unwrap();
    git(cwd, &["add", "-A"]);
    git(cwd, &["commit", "-q", "-m", message]);
}

/// Server helper yang mencatat setiap permintaan dan menjawab lewat `answer`.
async fn server(answer: impl Fn(&Prompt) -> Option<String> + Send + Sync + 'static) -> (Arc<HelperServer>, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let answer = Arc::new(answer);
    let prompt: PromptFn = Arc::new(move |p: Prompt| {
        let description = match &p {
            Prompt::Askpass { prompt, .. } => format!("askpass:{prompt}"),
            Prompt::EditMessage { initial, .. } => format!("edit:{initial}"),
        };
        log.lock().unwrap().push(description);
        let reply = answer(&p);
        Box::pin(async move { reply }) as PromptFuture
    });
    (HelperServer::start(HELPER.into(), prompt).await.unwrap(), seen)
}

fn run_helper(server: &HelperServer, token: &str, args: &[&str]) -> (bool, String) {
    let out = Command::new(HELPER)
        .args(args)
        .env("ONCODE_IPC_ENDPOINT", server.endpoint())
        .env("ONCODE_IPC_TOKEN", token)
        .output()
        .unwrap();
    (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned())
}

#[tokio::test(flavor = "multi_thread")]
async fn askpass_prints_the_answer_and_cancel_fails() {
    let (srv, seen) = server(|p| match p {
        Prompt::Askpass { prompt, .. } if prompt.starts_with("Password") => Some("rahasia".into()),
        _ => None,
    })
    .await;
    let token = srv.register(OpContext::default());

    let (ok, out) = run_helper(&srv, &token, &["Password for 'https://contoh.id': "]);
    assert!(ok);
    assert_eq!(out, "rahasia\n");

    // Pengguna membatalkan dialog: helper keluar dengan galat sehingga git berhenti.
    let (ok, out) = run_helper(&srv, &token, &["Username for 'https://contoh.id': "]);
    assert!(!ok && out.is_empty());
    assert_eq!(seen.lock().unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_or_expired_tokens_are_refused() {
    let (srv, seen) = server(|_| Some("tidak boleh terkirim".into())).await;
    let (ok, out) = run_helper(&srv, "token-palsu", &["Password: "]);
    assert!(!ok && out.is_empty());

    let token = srv.register(OpContext::default());
    srv.unregister(&token);
    let (ok, _) = run_helper(&srv, &token, &["Password: "]);
    assert!(!ok);
    assert!(seen.lock().unwrap().is_empty(), "permintaan tanpa token sah tidak boleh sampai ke pengguna");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn socket_is_private_to_the_user() {
    use std::os::unix::fs::PermissionsExt;
    let (srv, _) = server(|_| None).await;
    let mode = std::fs::metadata(srv.endpoint()).unwrap().permissions().mode();
    assert_eq!(mode & 0o077, 0, "socket tidak boleh bisa diakses pengguna lain");
}

#[tokio::test(flavor = "multi_thread")]
async fn editor_modes_supply_accept_and_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("COMMIT_EDITMSG");
    let path = file.to_str().unwrap();
    let (srv, seen) = server(|p| match p {
        Prompt::EditMessage { initial, .. } => Some(format!("{initial} (disunting)")),
        _ => None,
    })
    .await;

    std::fs::write(&file, "Pesan awal\n# komentar git\n").unwrap();
    let token = srv.register(OpContext { editor: EditorMode::AcceptDefault, ..Default::default() });
    assert!(run_helper(&srv, &token, &["editor", path]).0);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "Pesan awal\n# komentar git\n");

    let token = srv.register(OpContext { editor: EditorMode::Supply("Dari aplikasi".into()), ..Default::default() });
    assert!(run_helper(&srv, &token, &["editor", path]).0);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "Dari aplikasi\n");

    std::fs::write(&file, "Pesan awal\n# komentar git\n").unwrap();
    let token = srv.register(OpContext { editor: EditorMode::Prompt, ..Default::default() });
    assert!(run_helper(&srv, &token, &["editor", path]).0);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "Pesan awal (disunting)\n");
    assert_eq!(seen.lock().unwrap().as_slice(), ["edit:Pesan awal"], "komentar git tidak ditampilkan ke pengguna");

    // Editor urutan tanpa daftar todo yang disiapkan harus ditolak.
    let token = srv.register(OpContext::default());
    assert!(!run_helper(&srv, &token, &["sequence", path]).0);
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_rebase_squash_reword_and_drop() {
    isolate();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q", "-b", "main"]);
    commit(root, "base.txt", "base\n", "Dasar");
    let base = git(root, &["rev-parse", "HEAD"]).trim().to_string();
    commit(root, "a.txt", "a\n", "Tambah validasi login");
    commit(root, "b.txt", "b\n", "wip");
    commit(root, "c.txt", "c\n", "perbaiki tes");
    commit(root, "d.txt", "d\n", "debug print");

    let (srv, seen) = server(|p| match p {
        Prompt::EditMessage { initial, .. } if initial.contains("wip") => Some("Validasi login lengkap".into()),
        Prompt::EditMessage { initial, .. } if initial.contains("perbaiki tes") => Some("Perbaiki tes login".into()),
        _ => None,
    })
    .await;
    let g = Git::discover(None).await.unwrap();
    g.set_helper(srv);
    let repo = Repo::open(g, root).await.unwrap();

    let mut plan = repo.rebase_plan(Some(&base)).await.unwrap();
    assert_eq!(plan.items.len(), 4);
    plan.items[1].action = RebaseAction::Squash;
    plan.items[2].action = RebaseAction::Reword;
    plan.items[3].action = RebaseAction::Drop;
    repo.rebase_interactive(Some(&base), &plan.items).await.unwrap();

    let subjects: Vec<String> = git(root, &["log", "--format=%s"]).lines().map(String::from).collect();
    assert_eq!(subjects, ["Perbaiki tes login", "Validasi login lengkap", "Dasar"]);
    assert!(root.join("a.txt").exists() && root.join("b.txt").exists() && root.join("c.txt").exists());
    assert!(!root.join("d.txt").exists(), "commit yang di-drop harus hilang");
    assert!(repo.snapshot().await.unwrap().operation.is_none());
    assert_eq!(seen.lock().unwrap().len(), 2, "satu dialog untuk squash, satu untuk reword");
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_rebase_reorder_and_cancelled_message_stops_cleanly() {
    isolate();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q", "-b", "main"]);
    commit(root, "base.txt", "base\n", "Dasar");
    let base = git(root, &["rev-parse", "HEAD"]).trim().to_string();
    commit(root, "a.txt", "a\n", "A");
    commit(root, "b.txt", "b\n", "B");
    let before = git(root, &["rev-parse", "HEAD"]);

    // Pengguna membatalkan dialog pesan.
    let (srv, _) = server(|_| None).await;
    let g = Git::discover(None).await.unwrap();
    g.set_helper(srv);
    let repo = Repo::open(g, root).await.unwrap();

    let mut plan = repo.rebase_plan(Some(&base)).await.unwrap();
    plan.items.swap(0, 1);
    repo.rebase_interactive(Some(&base), &plan.items).await.unwrap();
    let subjects: Vec<String> = git(root, &["log", "--format=%s"]).lines().map(String::from).collect();
    assert_eq!(subjects, ["A", "B", "Dasar"], "urutan commit harus tertukar");

    let mut plan = repo.rebase_plan(Some(&base)).await.unwrap();
    plan.items[1].action = RebaseAction::Reword;
    assert!(repo.rebase_interactive(Some(&base), &plan.items).await.is_err());
    // Rebase berhenti di tengah; membatalkannya mengembalikan keadaan semula.
    let snapshot = repo.snapshot().await.unwrap();
    assert!(matches!(snapshot.operation, Some(Operation::Rebase { interactive: true, .. })), "{:?}", snapshot.operation);
    repo.operation(OperationAction::Abort).await.unwrap();
    assert_ne!(git(root, &["rev-parse", "HEAD"]), before, "hasil rebase pertama tetap ada");
    assert_eq!(git(root, &["log", "--format=%s"]).lines().collect::<Vec<_>>(), ["A", "B", "Dasar"]);
}
