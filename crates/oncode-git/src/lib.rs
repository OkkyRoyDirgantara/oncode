//! The OnCode Git layer.
//!
//! All operations run through the system `git` (see `docs/DESIGN.md` section 3.4).
//! This crate does not depend on Tauri, so it can be tested directly against a temporary repository.

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
pub use helper::{CredentialRequest, EditorMode, HelperServer, OpContext, Prompt, PromptFn, PromptFuture};
pub use log::LogStream;
pub use model::*;
pub use repo::Repo;
pub use runner::{Cmd, Git, LogFn, ProgressFn, MIN_GIT_VERSION};
pub use tokio_util::sync::CancellationToken;
