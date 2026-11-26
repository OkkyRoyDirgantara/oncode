//! OnCode LSP: language server process manager and its protocol client.
//!
//! - [`LspManager`] runs the processes and frames JSON-RPC messages over stdio
//!   (`Content-Length: N\r\n\r\n<json>`).
//! - [`Client`] is a typed JSON-RPC/LSP client on top of a raw message transport: request–response
//!   correlation, timeouts, cancellation, requests from the server, the `initialize`
//!   handshake, and document synchronization.
//! - [`position`] and [`uri`] translate positions and paths between the editor and the protocol.
//!
//! Does not depend on the UI.

mod capabilities;
mod client;
mod error;
mod frame;
mod manager;
#[cfg(any(test, feature = "mock"))]
pub mod mock;
mod model;
mod path;
pub mod position;
pub mod uri;

pub use capabilities::{client_capabilities, server_supports, Feature, SyncOptions};
pub use client::{Client, ClientConfig, ClientHandler, Responder, RpcError};
pub use error::{LspError, LspErrorCode, Result};
pub use frame::FrameDecoder;
pub use manager::LspManager;
pub use model::{default_servers, ServerDefault, ServerExitInfo, ServerSpec};
pub use path::find_on_path;
pub use position::PositionEncoding;
