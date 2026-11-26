//! OnCode application state without UI and without Tauri.
//!
//! - [`Core`] holds the repository registry, per-window workspaces, settings, persisted data,
//!   cancelable operations, and requests waiting for a user answer.
//! - [`api`] contains all the v0.2 "commands" as plain functions, one module per domain.
//! - Changes are broadcast as [`CoreEvent`] through the [`EventSink`] given at construction.
//!
//! All `async` functions in this crate must run on the tokio runtime given to
//! [`Core::new`] (e.g. via [`Core::spawn`]); the UI layer receives the results on its own main thread.

pub mod api;
pub mod dirs;
pub mod error;
pub mod events;
pub mod settings;
pub mod state;
pub mod store;
pub mod tokens;

#[cfg(test)]
pub(crate) mod testing;

pub use dirs::{Dirs, APP_IDENTIFIER};
pub use error::{AppError, AppResult};
pub use events::{CoreEvent, EventSink, NoticeKind, PromptKind, PromptRequest};
pub use settings::Settings;
pub use state::{root_key, Core, Shared, Workspace};
pub use store::Store;
pub use tokens::FallbackStore;
