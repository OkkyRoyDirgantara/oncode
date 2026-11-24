//! API perintah, satu modul per domain. Nama fungsi berpola `<domain>_<aksi>`, sama dengan
//! command IPC di v0.2. Jendela pemanggil dikenali lewat label string (`window`).
//!
//! Semua fungsi di sini harus dijalankan di runtime tokio milik [`Core`](crate::Core).

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

/// Pengganti `Channel<T>` v0.2: dipanggil untuk tiap kiriman, dari thread mana pun.
pub type Callback<T> = Arc<dyn Fn(T) + Send + Sync>;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::testing::TestCore;

    /// UI menjalankan fungsi API lewat `Core::spawn`, jadi future-nya harus `Send`.
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

        // Pola pemakaian dari UI: future memiliki `Shared` sendiri dan dijalankan di runtime core.
        let owned = t.core.clone();
        let settings = t.core.spawn(async move { super::app::settings_get(&owned, "w").await }).await.unwrap().unwrap();
        assert_eq!(settings, crate::Settings::default());
    }
}
