//! End-to-end tests: git → oncode-helper → HelperServer → the app's answer.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use oncode_git::*;

const HELPER: &str = env!("CARGO_BIN_EXE_oncode-helper");

fn isolate() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        for (key, value) in [
            // The platform's empty file: `NUL` on Windows.
            ("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" }),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_AUTHOR_NAME", "Tester"),
            ("GIT_AUTHOR_EMAIL", "tester@example.com"),
            ("GIT_COMMITTER_NAME", "Tester"),
            ("GIT_COMMITTER_EMAIL", "tester@example.com"),
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

/// A helper server that records every request and answers through `answer`.
async fn server(answer: impl Fn(&Prompt) -> Option<String> + Send + Sync + 'static) -> (Arc<HelperServer>, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let answer = Arc::new(answer);
    let prompt: PromptFn = Arc::new(move |p: Prompt| {
        let description = match &p {
            Prompt::Credential(r) => format!("credential:{} {} {}", r.protocol, r.host, r.username.as_deref().unwrap_or("-")),
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
        Prompt::Askpass { prompt, .. } if prompt.starts_with("Password") => Some("secret".into()),
        _ => None,
    })
    .await;
    let token = srv.register(OpContext::default());

    let (ok, out) = run_helper(&srv, &token, &["Password for 'https://example.com': "]);
    assert!(ok);
    assert_eq!(out, "secret\n");

    // The user cancels the dialog: the helper exits with an error so git stops.
    let (ok, out) = run_helper(&srv, &token, &["Username for 'https://example.com': "]);
    assert!(!ok && out.is_empty());
    assert_eq!(seen.lock().unwrap().len(), 2);
}

fn base64(input: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            out.push(if i <= chunk.len() { TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// An HTTP server on 127.0.0.1 that answers every request with 401 Basic and records the
/// `Authorization` headers it receives.
fn unauthorized_server() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 && !line.trim_end().is_empty() {
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("authorization") {
                        log.lock().unwrap().push(value.trim().to_string());
                    }
                }
                line.clear();
            }
            let _ = stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"test\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
    });
    (port, seen)
}

/// The start of a recorded credential request for the test server on `port`: the real host,
/// whatever user name git passes along.
fn credential_for(port: u16) -> String {
    format!("credential:http 127.0.0.1:{port} ")
}

/// The `.gitmodules` attack from issue #2: the user name `github.com'x` makes the askpass prompt
/// read `Password for 'http://github.com'x@127.0.0.1:…': `. The credential helper gets the real
/// host as its own field, and its answer goes to that host.
///
/// Git 2.40.0 loses the URL's user name after the first 401 and asks the helper without one, so
/// the reply carries its own; newer versions pass it on, and the reply keeps it.
#[tokio::test(flavor = "multi_thread")]
async fn credential_helper_gets_the_real_host_and_its_answer_reaches_the_server() {
    isolate();
    let (port, authorization) = unauthorized_server();
    let (srv, seen) = server(|p| match p {
        Prompt::Credential(request) if request.hostname() == "127.0.0.1" => request.reply("unused", "SECRET"),
        _ => None,
    })
    .await;
    let g = Git::discover(None).await.unwrap();
    g.set_helper(srv);
    let dir = tempfile::tempdir().unwrap();

    let url = format!("http://github.com'x@127.0.0.1:{port}/owner/repo.git");
    assert!(g.run(dir.path(), Cmd::new(["ls-remote", "--end-of-options", &url])).await.is_err(), "the server rejects every login");
    let first = seen.lock().unwrap().first().cloned().unwrap_or_default();
    let user = match first.strip_prefix(&credential_for(port)) {
        Some("github.com'x") => "github.com'x",
        Some("-") => "unused",
        _ => panic!("the helper gets the real host and the URL's user name, if any: {first:?}"),
    };
    // curl first tries the URL's user name with an empty password, then git asks for credentials.
    let expected = format!("Basic {}", base64(format!("{user}:SECRET").as_bytes()));
    let received = authorization.lock().unwrap().clone();
    assert!(received.contains(&expected), "the password from the app reaches the server as {user:?}: {received:?}");
}

/// Without a matching account, git moves on to askpass, which only the user answers.
#[tokio::test(flavor = "multi_thread")]
async fn credential_helper_without_an_answer_falls_back_to_askpass() {
    isolate();
    let (port, authorization) = unauthorized_server();
    let (srv, seen) = server(|_| None).await;
    let g = Git::discover(None).await.unwrap();
    g.set_helper(srv);
    let dir = tempfile::tempdir().unwrap();

    let url = format!("http://github.com'x@127.0.0.1:{port}/owner/repo.git");
    assert!(g.run(dir.path(), Cmd::new(["ls-remote", "--end-of-options", &url])).await.is_err());
    let seen = seen.lock().unwrap().clone();
    let [credential, askpass] = seen.as_slice() else { panic!("one credential request, then askpass: {seen:?}") };
    assert!(credential.starts_with(&credential_for(port)), "the helper gets the real host: {credential:?}");
    // The prompt text depends on the git version: newer ones write the user name's `'` as `%27`,
    // and 2.40.0 has lost the user name and asks for one.
    assert!(askpass.starts_with("askpass:") && askpass.contains(&format!("127.0.0.1:{port}")), "{askpass:?}");
    // Only curl's first try with the URL's user name and an empty password.
    let empty = format!("Basic {}", base64(b"github.com'x:"));
    let received = authorization.lock().unwrap().clone();
    assert!(received.iter().all(|header| *header == empty), "no password is sent when nobody answers: {received:?}");
}

/// `clone --recurse-submodules` clones each submodule in a child git process; the helper set
/// with `-c` must reach it too.
#[tokio::test(flavor = "multi_thread")]
async fn credential_helper_reaches_submodule_clones() {
    isolate();
    let (port, _) = unauthorized_server();
    let (srv, seen) = server(|_| None).await;
    let g = Git::discover(None).await.unwrap();
    g.set_helper(srv);

    let dir = tempfile::tempdir().unwrap();
    let parent = dir.path().join("super");
    std::fs::create_dir(&parent).unwrap();
    git(&parent, &["init", "-q", "-b", "main"]);
    let gitmodules = format!("[submodule \"sub\"]\n\tpath = sub\n\turl = http://github.com'x@127.0.0.1:{port}/owner/repo.git\n");
    std::fs::write(parent.join(".gitmodules"), gitmodules).unwrap();
    git(&parent, &["update-index", "--add", "--cacheinfo", "160000,1111111111111111111111111111111111111111,sub"]);
    git(&parent, &["add", ".gitmodules"]);
    git(&parent, &["commit", "-q", "-m", "Add submodule"]);

    // The submodule cannot be fetched; whether that fails the whole clone depends on the git version.
    let dest = dir.path().join("dest");
    let _ = g.clone_repo(parent.to_str().unwrap(), &dest, true, None, None).await;
    assert!(dest.join(".gitmodules").exists(), "the superproject itself was cloned");
    let requests = seen.lock().unwrap().clone();
    assert!(requests.iter().any(|request| request.starts_with(&credential_for(port))), "requests: {requests:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_or_expired_tokens_are_refused() {
    let (srv, seen) = server(|_| Some("must not be sent".into())).await;
    let (ok, out) = run_helper(&srv, "fake-token", &["Password: "]);
    assert!(!ok && out.is_empty());

    let token = srv.register(OpContext::default());
    srv.unregister(&token);
    let (ok, _) = run_helper(&srv, &token, &["Password: "]);
    assert!(!ok);
    assert!(seen.lock().unwrap().is_empty(), "a request without a valid token must not reach the user");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn socket_is_private_to_the_user() {
    use std::os::unix::fs::PermissionsExt;
    let (srv, _) = server(|_| None).await;
    let mode = std::fs::metadata(srv.endpoint()).unwrap().permissions().mode();
    assert_eq!(mode & 0o077, 0, "the socket must not be accessible to other users");
}

#[tokio::test(flavor = "multi_thread")]
async fn editor_modes_supply_accept_and_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("COMMIT_EDITMSG");
    let path = file.to_str().unwrap();
    let (srv, seen) = server(|p| match p {
        Prompt::EditMessage { initial, .. } => Some(format!("{initial} (edited)")),
        _ => None,
    })
    .await;

    std::fs::write(&file, "Initial message\n# git comment\n").unwrap();
    let token = srv.register(OpContext { editor: EditorMode::AcceptDefault, ..Default::default() });
    assert!(run_helper(&srv, &token, &["editor", path]).0);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "Initial message\n# git comment\n");

    let token = srv.register(OpContext { editor: EditorMode::Supply("From the app".into()), ..Default::default() });
    assert!(run_helper(&srv, &token, &["editor", path]).0);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "From the app\n");

    std::fs::write(&file, "Initial message\n# git comment\n").unwrap();
    let token = srv.register(OpContext { editor: EditorMode::Prompt, ..Default::default() });
    assert!(run_helper(&srv, &token, &["editor", path]).0);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "Initial message (edited)\n");
    assert_eq!(seen.lock().unwrap().as_slice(), ["edit:Initial message"], "git comments are not shown to the user");

    // The sequence editor without a prepared todo list must be refused.
    let token = srv.register(OpContext::default());
    assert!(!run_helper(&srv, &token, &["sequence", path]).0);
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_rebase_squash_reword_and_drop() {
    isolate();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q", "-b", "main"]);
    commit(root, "base.txt", "base\n", "Base");
    let base = git(root, &["rev-parse", "HEAD"]).trim().to_string();
    commit(root, "a.txt", "a\n", "Add login validation");
    commit(root, "b.txt", "b\n", "wip");
    commit(root, "c.txt", "c\n", "fix tests");
    commit(root, "d.txt", "d\n", "debug print");

    let (srv, seen) = server(|p| match p {
        Prompt::EditMessage { initial, .. } if initial.contains("wip") => Some("Complete login validation".into()),
        Prompt::EditMessage { initial, .. } if initial.contains("fix tests") => Some("Fix login tests".into()),
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
    assert_eq!(subjects, ["Fix login tests", "Complete login validation", "Base"]);
    assert!(root.join("a.txt").exists() && root.join("b.txt").exists() && root.join("c.txt").exists());
    assert!(!root.join("d.txt").exists(), "the dropped commit must be gone");
    assert!(repo.snapshot().await.unwrap().operation.is_none());
    assert_eq!(seen.lock().unwrap().len(), 2, "one dialog for squash, one for reword");
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_rebase_reorder_and_cancelled_message_stops_cleanly() {
    isolate();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q", "-b", "main"]);
    commit(root, "base.txt", "base\n", "Base");
    let base = git(root, &["rev-parse", "HEAD"]).trim().to_string();
    commit(root, "a.txt", "a\n", "A");
    commit(root, "b.txt", "b\n", "B");
    let before = git(root, &["rev-parse", "HEAD"]);

    // The user cancels the message dialog.
    let (srv, _) = server(|_| None).await;
    let g = Git::discover(None).await.unwrap();
    g.set_helper(srv);
    let repo = Repo::open(g, root).await.unwrap();

    let mut plan = repo.rebase_plan(Some(&base)).await.unwrap();
    plan.items.swap(0, 1);
    repo.rebase_interactive(Some(&base), &plan.items).await.unwrap();
    let subjects: Vec<String> = git(root, &["log", "--format=%s"]).lines().map(String::from).collect();
    assert_eq!(subjects, ["A", "B", "Base"], "the commit order must be swapped");

    let mut plan = repo.rebase_plan(Some(&base)).await.unwrap();
    plan.items[1].action = RebaseAction::Reword;
    assert!(repo.rebase_interactive(Some(&base), &plan.items).await.is_err());
    // The rebase stops midway; aborting it restores the original state.
    let snapshot = repo.snapshot().await.unwrap();
    assert!(matches!(snapshot.operation, Some(Operation::Rebase { interactive: true, .. })), "{:?}", snapshot.operation);
    repo.operation(OperationAction::Abort).await.unwrap();
    assert_ne!(git(root, &["rev-parse", "HEAD"]), before, "the result of the first rebase remains");
    assert_eq!(git(root, &["log", "--format=%s"]).lines().collect::<Vec<_>>(), ["A", "B", "Base"]);
}
