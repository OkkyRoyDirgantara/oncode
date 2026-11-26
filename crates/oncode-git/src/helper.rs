//! Server for `oncode-helper`.
//!
//! Git runs `oncode-helper` when it needs credentials (credential helper or askpass) or an
//! editor (commit message, rebase todo list). The helper contacts this server over a
//! local socket, carrying the one-time token belonging to that `git` invocation.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

/// A request the app (or the user) must answer.
#[derive(Debug, Clone)]
pub enum Prompt {
    /// Git asks for an HTTP(S) login through the credential helper protocol. The answer is
    /// [`CredentialRequest::reply`]; `None` lets git fall back to askpass.
    Credential(CredentialRequest),
    /// Username, password, passphrase, or SSH host confirmation.
    Askpass { prompt: String, repo: Option<PathBuf> },
    /// Git asks for a message (commit, merge, squash, reword).
    EditMessage { initial: String, repo: Option<PathBuf> },
}

/// What git asks a credential helper for (`git credential` protocol, operation `get`): a login
/// for one HTTP(S) origin. Git parses the remote URL itself and passes each part as its own
/// field, so a user name in the URL cannot pose as the host the way it can in the askpass
/// prompt text (`Password for 'https://github.com'x@evil.com': `).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CredentialRequest {
    /// `https` or `http`.
    pub protocol: String,
    /// The host name, with `:port` if the URL has one.
    pub host: String,
    /// The user name from the URL, if it has one.
    pub username: Option<String>,
}

impl CredentialRequest {
    /// Reads git's `key=value` lines. Other keys (`path`, `wwwauth[]`, `capability[]`, …) are ignored.
    pub fn parse(text: &str) -> Self {
        let mut request = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            match key {
                "protocol" => request.protocol = value.to_string(),
                "host" => request.host = value.to_string(),
                "username" => request.username = Some(value.to_string()),
                _ => {}
            }
        }
        request
    }

    /// The host name without the port, in lowercase (`[::1]` keeps its brackets).
    pub fn hostname(&self) -> String {
        let host = self.host.as_str();
        let name = if host.starts_with('[') {
            host.find(']').map_or(host, |end| &host[..=end])
        } else {
            host.split(':').next().unwrap_or_default()
        };
        name.to_ascii_lowercase()
    }

    /// The answer in git's format. The user name is only included when the URL has none, so
    /// git keeps the one in the URL. `None` if a value contains a line break or NUL, which
    /// would let it add fields of its own.
    pub fn reply(&self, username: &str, password: &str) -> Option<String> {
        if [username, password].iter().any(|value| value.contains(['\n', '\r', '\0'])) {
            return None;
        }
        let mut reply = String::new();
        if self.username.is_none() {
            reply.push_str(&format!("username={username}\n"));
        }
        reply.push_str(&format!("password={password}\n"));
        Some(reply)
    }
}

pub type PromptFuture = Pin<Box<dyn Future<Output = Option<String>> + Send>>;
/// Returns `None` if the user cancels.
pub type PromptFn = Arc<dyn Fn(Prompt) -> PromptFuture + Send + Sync>;

/// How to answer editor requests for a single git invocation.
#[derive(Debug, Clone, Default)]
pub enum EditorMode {
    /// Ask the user.
    #[default]
    Prompt,
    /// Accept the default content from git.
    AcceptDefault,
    /// Use this text.
    Supply(String),
}

/// Context of a single git invocation, looked up by token.
#[derive(Debug, Clone, Default)]
pub struct OpContext {
    pub repo: Option<PathBuf>,
    /// Contents of the todo file for an interactive rebase.
    pub todo: Option<String>,
    pub editor: EditorMode,
}

#[derive(Debug, Deserialize)]
struct Request {
    token: String,
    kind: String,
    text: String,
}

#[derive(Debug, Serialize)]
struct Response {
    ok: bool,
    value: String,
}

pub struct HelperServer {
    endpoint: String,
    helper_path: PathBuf,
    contexts: Mutex<HashMap<String, OpContext>>,
    prompt: PromptFn,
}

impl HelperServer {
    /// Opens the local socket and starts accepting connections from the helper.
    pub async fn start(helper_path: PathBuf, prompt: PromptFn) -> std::io::Result<Arc<Self>> {
        let id = format!("oncode-{}-{}", std::process::id(), &uuid::Uuid::new_v4().simple().to_string()[..12]);
        let (endpoint, listener) = transport::bind(&id)?;
        let server = Arc::new(Self { endpoint, helper_path, contexts: Mutex::new(HashMap::new()), prompt });
        transport::serve(listener, server.clone());
        Ok(server)
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn helper_path(&self) -> &Path {
        &self.helper_path
    }

    /// Registers a context and returns a one-time token.
    pub fn register(&self, ctx: OpContext) -> String {
        let token = uuid::Uuid::new_v4().simple().to_string();
        self.contexts.lock().unwrap().insert(token.clone(), ctx);
        token
    }

    pub fn unregister(&self, token: &str) {
        self.contexts.lock().unwrap().remove(token);
    }

    async fn handle<S: AsyncRead + AsyncWrite + Unpin>(self: Arc<Self>, stream: S) {
        let (read, mut write) = tokio::io::split(stream);
        let mut line = String::new();
        if BufReader::new(read).read_line(&mut line).await.is_err() {
            return;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => self.answer(request).await,
            Err(_) => None,
        };
        let response = match response {
            Some(value) => Response { ok: true, value },
            None => Response { ok: false, value: String::new() },
        };
        let mut out = serde_json::to_string(&response).unwrap_or_default();
        out.push('\n');
        let _ = write.write_all(out.as_bytes()).await;
        let _ = write.shutdown().await;
    }

    async fn answer(&self, request: Request) -> Option<String> {
        let ctx = self.contexts.lock().unwrap().get(&request.token).cloned()?;
        match request.kind.as_str() {
            "credential" => (self.prompt)(Prompt::Credential(CredentialRequest::parse(&request.text))).await,
            "askpass" => (self.prompt)(Prompt::Askpass { prompt: request.text, repo: ctx.repo }).await,
            "sequence" => {
                let todo = ctx.todo?;
                tokio::fs::write(&request.text, todo).await.ok()?;
                Some(String::new())
            }
            "editor" => match ctx.editor {
                EditorMode::AcceptDefault => Some(String::new()),
                EditorMode::Supply(text) => {
                    tokio::fs::write(&request.text, ensure_newline(text)).await.ok()?;
                    Some(String::new())
                }
                EditorMode::Prompt => {
                    let raw = tokio::fs::read(&request.text).await.ok()?;
                    let initial = strip_comments(&String::from_utf8_lossy(&raw));
                    let message = (self.prompt)(Prompt::EditMessage { initial, repo: ctx.repo }).await?;
                    tokio::fs::write(&request.text, ensure_newline(message)).await.ok()?;
                    Some(String::new())
                }
            },
            _ => None,
        }
    }
}

fn ensure_newline(mut text: String) -> String {
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// Removes the comment lines (`#`) that git adds to the message template.
pub fn strip_comments(text: &str) -> String {
    let kept: Vec<&str> = text.lines().filter(|l| !l.starts_with('#')).collect();
    let joined = kept.join("\n");
    joined.trim_end().to_string()
}

#[cfg(unix)]
mod transport {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tokio::net::UnixListener;

    pub fn bind(id: &str) -> std::io::Result<(String, UnixListener)> {
        let dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .unwrap_or_else(std::env::temp_dir);
        let path = dir.join(format!("{id}.sock"));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        Ok((path.to_string_lossy().into_owned(), listener))
    }

    pub fn serve(listener: UnixListener, server: Arc<HelperServer>) {
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        tokio::spawn(server.clone().handle(stream));
                    }
                    Err(_) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
                }
            }
        });
    }
}

#[cfg(windows)]
mod transport {
    use super::*;
    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

    pub struct Listener {
        name: String,
        first: NamedPipeServer,
    }

    pub fn bind(id: &str) -> std::io::Result<(String, Listener)> {
        let name = format!(r"\\.\pipe\{id}");
        let first = ServerOptions::new().first_pipe_instance(true).reject_remote_clients(true).create(&name)?;
        Ok((name.clone(), Listener { name, first }))
    }

    pub fn serve(listener: Listener, server: Arc<HelperServer>) {
        tokio::spawn(async move {
            let Listener { name, first } = listener;
            let mut pipe = first;
            loop {
                if pipe.connect().await.is_err() {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                let next = match ServerOptions::new().reject_remote_clients(true).create(&name) {
                    Ok(next) => next,
                    Err(_) => break,
                };
                let connected = std::mem::replace(&mut pipe, next);
                tokio::spawn(server.clone().handle(connected));
            }
        });
    }
}

impl Drop for HelperServer {
    fn drop(&mut self) {
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.endpoint);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_git_comment_lines() {
        let text = "Subject\n\nMessage body\n# Please enter the commit message\n# Lines starting with '#'\n";
        assert_eq!(strip_comments(text), "Subject\n\nMessage body");
    }

    #[test]
    fn credential_request_keeps_the_user_name_apart_from_the_host() {
        // What git sends for `http://github.com'x@127.0.0.1:48731/owner/repo.git`.
        let text = "protocol=http\nhost=127.0.0.1:48731\nusername=github.com'x\nwwwauth[]=Basic realm=\"evil\"\n";
        let request = CredentialRequest::parse(text);
        assert_eq!(request.protocol, "http");
        assert_eq!(request.host, "127.0.0.1:48731");
        assert_eq!(request.username.as_deref(), Some("github.com'x"));
        assert_eq!(request.hostname(), "127.0.0.1");

        let request = CredentialRequest::parse("protocol=https\r\nhost=GitHub.com\r\n\r\n");
        assert_eq!((request.hostname().as_str(), request.username), ("github.com", None));
        assert_eq!(CredentialRequest::parse("protocol=https\nhost=[::1]:8443\n").hostname(), "[::1]");
        assert_eq!(CredentialRequest::parse("protocol=https\n").hostname(), "");
    }

    #[test]
    fn credential_reply_fills_only_what_the_url_lacks() {
        let without_user = CredentialRequest { protocol: "https".into(), host: "github.com".into(), username: None };
        assert_eq!(without_user.reply("x-access-token", "tok").as_deref(), Some("username=x-access-token\npassword=tok\n"));
        let with_user = CredentialRequest { username: Some("ana".into()), ..without_user.clone() };
        assert_eq!(with_user.reply("x-access-token", "tok").as_deref(), Some("password=tok\n"));
        // A value with a line break could add fields of its own (for example a second `host=`).
        assert_eq!(without_user.reply("x", "tok\nhost=evil.com"), None);
        assert_eq!(without_user.reply("x\r", "tok"), None);
        assert_eq!(without_user.reply("x", "tok\0"), None);
    }
}
