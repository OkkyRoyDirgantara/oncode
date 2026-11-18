//! Lapisan Git OnCode.
//!
//! Semua operasi dijalankan lewat `git` milik sistem (lihat `docs/RANCANGAN.md` bagian 3.4).
//! Crate ini tidak bergantung pada Tauri sehingga bisa diuji langsung terhadap repositori sementara.

pub mod blame;
pub mod conflict;
pub mod content;
pub mod error;
pub mod graph;
pub mod helper;
pub mod log;
pub mod model;
pub mod ops;
pub mod repo;
pub mod runner;
pub mod status;

pub use error::{GitError, GitErrorCode, Result};
pub use helper::{EditorMode, HelperServer, OpContext, Prompt, PromptFn, PromptFuture};
pub use log::LogStream;
pub use model::*;
pub use repo::Repo;
pub use runner::{Cmd, Git, LogFn, ProgressFn, MIN_GIT_VERSION};
pub use tokio_util::sync::CancellationToken;
