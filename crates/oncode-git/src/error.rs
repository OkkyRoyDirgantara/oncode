use serde::Serialize;

/// Error code the UI uses to choose the message and follow-up action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GitErrorCode {
    GitNotFound,
    NotARepo,
    AuthFailed,
    PushRejected,
    NoUpstream,
    DirtyWorkTree,
    Conflict,
    IndexLocked,
    HookFailed,
    BranchNotMerged,
    NothingToCommit,
    Stale,
    Cancelled,
    Timeout,
    InvalidInput,
    Unknown,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct GitError {
    pub code: GitErrorCode,
    pub message: String,
    /// Usually git's stderr verbatim.
    pub detail: Option<String>,
}

pub type Result<T> = std::result::Result<T, GitError>;

impl GitError {
    pub fn new(code: GitErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), detail: None }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(GitErrorCode::InvalidInput, message)
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn io(context: &str, err: std::io::Error) -> Self {
        Self::new(GitErrorCode::Unknown, format!("{context}: {err}"))
    }

    /// Maps git's stderr and exit code to an error code.
    pub fn from_output(args: &[String], code: i32, stdout: &str, stderr: &str) -> Self {
        let text = format!("{stderr}\n{stdout}");
        let lower = text.to_lowercase();
        let has = |needle: &str| lower.contains(needle);

        let kind = if has("not a git repository") {
            GitErrorCode::NotARepo
        } else if has("authentication failed")
            || has("could not read username")
            || has("could not read password")
            || has("permission denied (publickey")
            || has("terminal prompts disabled")
            || has("invalid username or password")
            || has("host key verification failed")
            || has("http basic: access denied")
        {
            GitErrorCode::AuthFailed
        } else if has("index.lock") || has("unable to create") && has(".lock") {
            GitErrorCode::IndexLocked
        } else if has("[rejected]")
            || has("non-fast-forward")
            || has("fetch first")
            || has("stale info")
            || has("failed to push some refs")
        {
            GitErrorCode::PushRejected
        } else if has("has no upstream branch")
            || has("no tracking information")
            || has("no upstream configured")
        {
            GitErrorCode::NoUpstream
        } else if has("conflict")
            || has("fix conflicts")
            || has("unmerged files")
            || has("needs merge")
            || has("could not apply")
            || has("resolve all conflicts")
        {
            GitErrorCode::Conflict
        } else if has("would be overwritten")
            || has("please commit your changes or stash them")
            || has("you have unstaged changes")
            || has("your index contains uncommitted changes")
            || has("contains modified or untracked files")
            || has("local changes to the following files")
        {
            GitErrorCode::DirtyWorkTree
        } else if has("is not fully merged") {
            GitErrorCode::BranchNotMerged
        } else if has("nothing to commit") || has("nothing added to commit") || has("no changes added to commit") {
            GitErrorCode::NothingToCommit
        } else if has("hook") && (has("pre-commit") || has("commit-msg") || has("pre-push") || has("was ignored") || has("hook declined") || has("pre-rebase") || has("pre-merge-commit")) {
            GitErrorCode::HookFailed
        } else {
            GitErrorCode::Unknown
        };

        let first_line = stderr
            .lines()
            .chain(stdout.lines())
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("Git failed without a message");
        let message = first_line
            .trim_start_matches("fatal: ")
            .trim_start_matches("error: ")
            .to_string();
        let sub = args.iter().find(|a| !a.starts_with('-')).cloned().unwrap_or_default();
        GitError {
            code: kind,
            message: if message.is_empty() { format!("git {sub} failed (exit code {code})") } else { message },
            detail: Some(text.trim().to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(stderr: &str) -> GitErrorCode {
        GitError::from_output(&["push".into()], 1, "", stderr).code
    }

    #[test]
    fn classifies_common_failures() {
        assert_eq!(classify("fatal: not a git repository (or any of the parent directories): .git"), GitErrorCode::NotARepo);
        assert_eq!(classify("remote: Invalid username or password.\nfatal: Authentication failed for 'https://x/'"), GitErrorCode::AuthFailed);
        assert_eq!(classify("git@github.com: Permission denied (publickey)."), GitErrorCode::AuthFailed);
        assert_eq!(classify(" ! [rejected]        main -> main (fetch first)\nerror: failed to push some refs to 'x'"), GitErrorCode::PushRejected);
        assert_eq!(classify("fatal: The current branch topic has no upstream branch."), GitErrorCode::NoUpstream);
        assert_eq!(classify("error: Your local changes to the following files would be overwritten by checkout:"), GitErrorCode::DirtyWorkTree);
        assert_eq!(classify("fatal: Unable to create '/r/.git/index.lock': File exists."), GitErrorCode::IndexLocked);
        assert_eq!(classify("error: The branch 'x' is not fully merged."), GitErrorCode::BranchNotMerged);
        assert_eq!(classify("CONFLICT (content): Merge conflict in a.txt"), GitErrorCode::Conflict);
        assert_eq!(classify("something odd"), GitErrorCode::Unknown);
    }

    #[test]
    fn message_strips_prefix() {
        let e = GitError::from_output(&["switch".into()], 128, "", "fatal: invalid reference: nope\n");
        assert_eq!(e.message, "invalid reference: nope");
    }
}
