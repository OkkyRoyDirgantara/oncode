//! State aplikasi OnCode tanpa UI dan tanpa Tauri.
//!
//! - [`Core`] memegang registry repositori, workspace per jendela, pengaturan, data tersimpan,
//!   operasi yang bisa dibatalkan, dan permintaan yang menunggu jawaban pengguna.
//! - [`api`] memuat seluruh "command" v0.2 sebagai fungsi biasa, satu modul per domain.
//! - Perubahan disiarkan sebagai [`CoreEvent`] lewat [`EventSink`] yang diberikan saat konstruksi.
//!
//! Semua fungsi `async` di crate ini harus dijalankan di runtime tokio yang diberikan ke
//! [`Core::new`] (mis. lewat [`Core::spawn`]); lapisan UI menerima hasilnya di thread utamanya sendiri.

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
