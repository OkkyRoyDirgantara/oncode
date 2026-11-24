//! Lapisan workspace OnCode.
//!
//! Menyediakan akses berkas, watcher, pencarian teks proyek, dan indeks quick open
//! (lihat `docs/RANCANGAN.md` bagian 7.1–7.3). Crate ini tidak bergantung pada Tauri
//! sehingga bisa diuji langsung terhadap direktori sementara.

pub mod error;
pub mod fs;
mod gitignore;
pub mod index;
pub mod search;
pub mod symbols;
pub mod watcher;

pub use error::{Result, WsError, WsErrorCode};
pub use fs::{
    create_dir, create_file, is_inside, open_file, read_dir, rename, save_file, stat, trash, DirEntry, Eol, FileStat,
    OpenedFile, TextFile,
};
pub use index::{FileIndex, QuickOpenItem};
pub use search::{replace_in_files, search, FileMatches, LineMatch, ReplaceSummary, SearchQuery, SearchSummary};
pub use watcher::{WatchEvent, Watcher};
