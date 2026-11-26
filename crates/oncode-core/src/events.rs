//! Events broadcast by `oncode-core` to the UI through [`EventSink`].

use std::sync::Arc;

use oncode_git::{GitLogEntry, RepoSnapshot};
use serde::Serialize;

use crate::settings::Settings;

/// Event receiver. Called from any thread (tokio runtime, file watcher), so it
/// must be fast and must not block; the UI usually just forwards it to the main thread.
pub type EventSink = Arc<dyn Fn(CoreEvent) + Send + Sync>;

#[derive(Debug, Clone, PartialEq)]
pub enum CoreEvent {
    /// The state of a repository changed; the UI can simply redraw from this snapshot.
    RepoChanged { root: String, snapshot: RepoSnapshot },
    /// A branch, tag, or HEAD changed; the ref list and graph need to be reloaded.
    RefsChanged { root: String },
    /// Files in the project folder of window `window` changed from outside the app.
    FsChanged {
        /// Label of the target window.
        window: String,
        paths: Vec<String>,
        /// Too many changes to itemize; reload everything.
        overflow: bool,
    },
    /// A single git command run by the app, for the Git Output panel.
    GitOutput { entry: GitLogEntry },
    /// Git needs a user answer; answered through `api::app::prompt_respond`.
    Prompt {
        /// The window whose project folder contains the requesting repository; `None` = show in any window.
        window: Option<String>,
        request: PromptRequest,
    },
    /// User-level settings changed. The effective per-window settings (with project
    /// overrides) are fetched through `api::app::settings_get`.
    SettingsChanged { settings: Settings },
    /// The list of GitHub/GitLab accounts changed (login or logout).
    AccountsChanged,
    /// A notification from the backend that the UI shows as a toast.
    Notice { kind: NoticeKind, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PromptKind {
    /// Username or plain text.
    Text,
    /// Password, token, or passphrase: the input is hidden.
    Secret,
    /// Yes/no question (e.g. SSH host fingerprint).
    Confirm,
    /// Multi-line commit message.
    Message,
}

/// Payload of [`CoreEvent::Prompt`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptRequest {
    pub id: String,
    pub kind: PromptKind,
    /// Question text from git/ssh.
    pub prompt: String,
    /// Initial content (for commit messages).
    pub initial: String,
    pub repo: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NoticeKind {
    Warning,
    Error,
}
