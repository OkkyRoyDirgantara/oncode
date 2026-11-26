//! OnCode workspace layer.
//!
//! Provides file access, watcher, project text search, and the quick open index
//! (see `docs/DESIGN.md` section 7.1–7.3). This crate does not depend on Tauri,
//! so it can be tested directly against temporary directories.

pub mod error;
pub mod fs;
mod gitignore;
pub mod index;
pub mod search;
pub mod symbols;
pub mod watcher;

pub use error::{Result, WsError, WsErrorCode};
pub use fs::{
    copy_into, create_dir, create_file, is_inside, open_file, read_dir, rename, save_file, stat, trash, DirEntry, Eol,
    FileStat, OpenedFile, TextFile,
};
pub use index::{FileIndex, QuickOpenItem};
pub use search::{replace_in_files, search, FileMatches, LineMatch, ReplaceSummary, SearchQuery, SearchSummary};
pub use watcher::{WatchEvent, Watcher};
