//! Tampilan diff dengan staging per perubahan/baris, editor konflik, penanda perubahan di
//! editor, dan blame. Lihat docs/PENGEMBANGAN.md.

pub mod conflict_state;
pub mod conflict_tab;
pub mod diff_tab;
pub mod gutter;
pub mod lines;
pub mod markers;
mod util;

use gpui_kit::{App, Context, Window};
use oncode_git::DiffSource;
use oncode_ui::{OpenOptions, Workbench};

pub use diff_tab::{DiffParams, DiffView};

/// Diff satu berkas yang diminta fitur lain (panel Source Control, graph, stash).
#[derive(Debug, Clone, PartialEq)]
pub struct DiffRequest {
    /// Akar repositori (worktree).
    pub root: String,
    /// Path relatif terhadap akar repositori.
    pub path: String,
    /// Path lama bila berkas diganti nama.
    pub orig_path: Option<String>,
    pub source: DiffSource,
    /// Buka sebagai tab pratinjau.
    pub preview: bool,
}

/// Mendaftarkan fitur ini. Dipanggil sekali dari `app/src/main.rs` setelah `oncode_ui::init`.
pub fn init(cx: &mut App) {
    oncode_ui::register_tab_kind(cx, "diff", diff_tab::DiffKind);
    oncode_ui::register_tab_kind(cx, "conflict", conflict_tab::ConflictKindTab);
    oncode_ui::register_feature(cx, markers::setup);
}

/// Membuka tab diff untuk satu berkas.
pub fn open_diff(workbench: &mut Workbench, request: DiffRequest, window: &mut Window, cx: &mut Context<Workbench>) {
    let params = DiffParams { root: request.root, path: request.path, orig_path: request.orig_path, source: request.source };
    workbench.open_tab(params.tab(), OpenOptions { preview: request.preview, ..Default::default() }, window, cx);
}

/// Membuka editor konflik untuk satu berkas yang sedang konflik.
pub fn open_conflict(workbench: &mut Workbench, root: String, path: String, window: &mut Window, cx: &mut Context<Workbench>) {
    workbench.open_tab(conflict_tab::conflict_tab(&root, &path), OpenOptions::default(), window, cx);
}
