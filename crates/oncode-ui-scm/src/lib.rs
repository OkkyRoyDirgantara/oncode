//! The Source Control panel, commit, branch, remote, stash, and Git Output.

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

/// Registers this feature. Called once from `app/src/main.rs` after `oncode_ui::init`.
pub fn init(cx: &mut App) {
    output::install(cx);
    oncode_ui::register_feature(cx, register::setup);
}
