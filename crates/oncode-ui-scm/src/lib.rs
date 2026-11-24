//! Panel Source Control, commit, branch, remote, stash, dan Git Output.

pub mod actions;
mod ai;
pub mod logic;
pub mod output;
pub mod prompts;
mod register;
pub mod rows;
pub mod status;
pub mod store;
pub mod view;

use gpui_kit::App;

/// Mendaftarkan fitur ini. Dipanggil sekali dari `app/src/main.rs` setelah `oncode_ui::init`.
pub fn init(cx: &mut App) {
    output::install(cx);
    oncode_ui::register_feature(cx, register::setup);
}
