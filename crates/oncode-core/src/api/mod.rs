//! Command API, one module per domain. Function names follow the pattern `<domain>_<action>`, same as
//! the IPC commands in v0.2. The calling window is identified by a string label (`window`).
//!
//! All functions here must run on the tokio runtime owned by [`Core`](crate::Core).

use std::sync::Arc;

pub mod ai;
pub mod app;
pub mod forge;
pub mod format;
pub mod fs;
pub mod git;
pub mod lsp;
pub mod symbols;
pub mod term;

/// Replacement for v0.2's `Channel<T>`: called for each message sent, from any thread.
pub type Callback<T> = Arc<dyn Fn(T) + Send + Sync>;

/// Helper programs (formatter, Claude Code) without a console window: on Windows a GUI app that
/// runs a console program briefly flashes a black window.
pub(crate) fn hidden_command(program: &std::path::Path) -> tokio::process::Command {
    #[allow(unused_mut)]
    let mut command = tokio::process::Command::new(program);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    command
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::testing::TestCore;

    /// The UI runs API functions through `Core::spawn`, so their futures must be `Send`.
    #[tokio::test]
    async fn api_futures_can_be_spawned_on_the_runtime() {
        fn assert_send<T: Send>(value: T) -> T {
            value
        }
        let t = TestCore::new();
        let core = &t.core;
        drop(assert_send(super::app::workspace_open(core, "w", String::new())));
        drop(assert_send(super::fs::search_start(core, "w", serde_json::from_str(r#"{"pattern":"x","isRegex":false,"caseSensitive":false,"wholeWord":false,"include":[],"exclude":[],"maxResults":0}"#).unwrap(), String::new(), Arc::new(|_| {}))));
        drop(assert_send(super::git::git_stage(core, String::new(), Vec::new())));
        drop(assert_send(super::git::git_fetch(core, String::new(), None, String::new(), Arc::new(|_| {}))));
        drop(assert_send(super::term::term_spawn(core, "w", 80, 24, Arc::new(|_| {}))));
        drop(assert_send(super::term::term_write(core, 1, b"x")));
        drop(assert_send(super::lsp::lsp_start(core, "w", String::new(), Arc::new(|_| {}))));
        drop(assert_send(super::forge::forge_login_poll(core, String::new())));
        drop(assert_send(super::ai::ai_commit_message(core, String::new(), String::new(), Arc::new(|_| {}))));
        drop(assert_send(super::forge::forge_pr_checkout(core, String::new(), oncode_forge::ForgeKind::GitHub, String::new(), 1, String::new(), Arc::new(|_| {}))));

        // Usage pattern from the UI: the future owns its own `Shared` and runs on the core's runtime.
        let owned = t.core.clone();
        let settings = t.core.spawn(async move { super::app::settings_get(&owned, "w").await }).await.unwrap().unwrap();
        assert_eq!(settings, crate::Settings::default());
    }
}
