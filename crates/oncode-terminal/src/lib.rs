//! Terminal OnCode: pengelola PTY di atas `portable-pty`.
//!
//! Crate ini tidak bergantung pada Tauri. Lapisan aplikasi membungkus
//! [`TerminalManager`] dalam command dan meneruskan keluaran lewat channel.

mod error;
mod manager;
mod shell;

pub use error::{Result, TermError, TermErrorCode};
pub use manager::{SpawnOptions, TerminalManager};
pub use shell::default_shell;
