//! OnCode terminal: a PTY manager on top of `portable-pty`.
//!
//! This crate does not depend on Tauri. The application layer wraps
//! [`TerminalManager`] in commands and forwards output through a channel.

mod error;
mod manager;
mod shell;

pub use error::{Result, TermError, TermErrorCode};
pub use manager::{SpawnOptions, TerminalManager};
pub use shell::default_shell;
