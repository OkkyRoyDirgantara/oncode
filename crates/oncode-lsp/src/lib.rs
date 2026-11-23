//! LSP OnCode: pengelola proses language server dan klien protokolnya.
//!
//! - [`LspManager`] menjalankan proses dan membingkai pesan JSON-RPC lewat stdio
//!   (`Content-Length: N\r\n\r\n<json>`).
//! - [`Client`] adalah klien JSON-RPC/LSP bertipe di atas transport pesan mentah: korelasi
//!   permintaan–jawaban, batas waktu, pembatalan, permintaan dari server, jabat tangan
//!   `initialize`, dan sinkronisasi dokumen.
//! - [`position`] dan [`uri`] menerjemahkan posisi dan path antara editor dan protokol.
//!
//! Tidak bergantung pada UI.

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
