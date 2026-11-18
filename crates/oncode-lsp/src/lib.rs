//! LSP OnCode: pengelola proses language server.
//!
//! Crate ini hanya menjalankan proses dan membingkai pesan JSON-RPC lewat stdio
//! (`Content-Length: N\r\n\r\n<json>`); isi protokol LSP ditangani frontend.
//! Tidak bergantung pada Tauri.

mod error;
mod frame;
mod manager;
mod model;
mod path;

pub use error::{LspError, LspErrorCode, Result};
pub use frame::FrameDecoder;
pub use manager::LspManager;
pub use model::{default_servers, ServerDefault, ServerExitInfo, ServerSpec};
pub use path::find_on_path;
