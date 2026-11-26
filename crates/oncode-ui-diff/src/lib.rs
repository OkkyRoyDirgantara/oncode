//! Diff view with per-change/line staging, conflict editor, change markers in the
//! editor, and blame. See docs/DEVELOPMENT.md.

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

/// Diff of a single file requested by another feature (Source Control panel, graph, stash).
#[derive(Debug, Clone, PartialEq)]
pub struct DiffRequest {
    /// Repository root (worktree).
    pub root: String,
    /// Path relative to the repository root.
    pub path: String,
    /// Old path if the file was renamed.
    pub orig_path: Option<String>,
    pub source: DiffSource,
    /// Open as a preview tab.
    pub preview: bool,
}

/// Registers this feature. Called once from `app/src/main.rs` after `oncode_ui::init`.
pub fn init(cx: &mut App) {
    oncode_ui::register_tab_kind(cx, "diff", diff_tab::DiffKind);
    oncode_ui::register_tab_kind(cx, "conflict", conflict_tab::ConflictKindTab);
    oncode_ui::register_feature(cx, markers::setup);
}

/// Opens a diff tab for a single file.
pub fn open_diff(workbench: &mut Workbench, request: DiffRequest, window: &mut Window, cx: &mut Context<Workbench>) {
    let params = DiffParams { root: request.root, path: request.path, orig_path: request.orig_path, source: request.source };
    workbench.open_tab(params.tab(), OpenOptions { preview: request.preview, ..Default::default() }, window, cx);
}

/// Opens the conflict editor for a single file that is in conflict.
pub fn open_conflict(workbench: &mut Workbench, root: String, path: String, window: &mut Window, cx: &mut Context<Workbench>) {
    workbench.open_tab(conflict_tab::conflict_tab(&root, &path), OpenOptions::default(), window, cx);
}
