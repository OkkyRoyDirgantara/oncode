//! The search box in the middle of the title bar, like VS Code's command center. It shows the open
//! folder's name and opens Go to File; without a folder it shows "Search" and opens the command palette.

use gpui_kit::component::button::Button;
use gpui_kit::component::{h_flex, ActiveTheme as _, Icon, Sizable as _, TITLE_BAR_HEIGHT};
use gpui_kit::{div, px, App, Decorations, IntoElement, ParentElement as _, Pixels, Styled as _, Window};
use serde_json::Value;

use crate::{commands, keys, Lucide};

/// The title bar content: the menus (if any) on the left and the search box in the middle of the window.
pub fn title_bar_content(menus: Option<gpui_kit::AnyElement>, folder: Option<&str>, window: &Window, cx: &App) -> impl IntoElement {
    let (left, right) = balance(window);
    // Both sides grow alike from their bases, which puts the box in the middle of the window. A long menu row
    // keeps its width (flex items do not shrink below their content), so the box then moves right instead of
    // covering the menus.
    h_flex()
        .flex_1()
        .h_full()
        .gap_3()
        .child(h_flex().h_full().flex_grow(1.).flex_shrink(1.).flex_basis(left).children(menus))
        .child(command_center(folder, window, cx))
        .child(div().h_full().flex_grow(1.).flex_shrink(1.).flex_basis(right))
}

/// Extra width for the left and right side. `TitleBar` pads its left edge (80 px for the traffic lights on
/// macOS, 12 px elsewhere) and draws the window buttons to the right of its children; the lighter side gets
/// the difference, so the box is centered in the window rather than between the padding and the buttons.
fn balance(window: &Window) -> (Pixels, Pixels) {
    let padding = if cfg!(target_os = "macos") { 80. } else { 12. };
    // The same conditions as `TitleBar`'s own window buttons.
    let buttons = if cfg!(target_os = "macos") || (cfg!(target_os = "linux") && !matches!(window.window_decorations(), Decorations::Client { .. })) {
        0
    } else {
        let supported = window.window_controls();
        1 + usize::from(supported.minimize) + usize::from(supported.maximize)
    };
    let buttons = buttons as f32 * TITLE_BAR_HEIGHT.as_f32();
    (px((buttons - padding).max(0.)), px((padding - buttons).max(0.)))
}

fn command_center(folder: Option<&str>, window: &Window, cx: &App) -> impl IntoElement {
    let (command, label, description) = match folder {
        Some(name) => ("workbench.quickOpen", name.to_string(), format!("Search files in {name}")),
        None => ("workbench.commandPalette", "Search".to_string(), "Search commands".to_string()),
    };
    let shortcut = commands::get(command, window, cx).and_then(|command| commands::keybindings_for(&command, cx).into_iter().next());
    let tooltip = match shortcut {
        Some(shortcut) => format!("{description} ({})", keys::format_keybinding(&shortcut)),
        None => description.clone(),
    };
    // About 38% of the window, like VS Code, within 200–600 px; it may shrink further when the window is narrow.
    let width = (window.viewport_size().width.as_f32() * 0.38).clamp(200., 600.);
    Button::new("command-center")
        .outline()
        .xsmall()
        .icon(Icon::new(Lucide::Search))
        .label(label)
        .accessibility_label(description)
        .tooltip(tooltip)
        .w(px(width))
        .min_w(px(120.))
        .flex_shrink(1.)
        .text_color(cx.theme().muted_foreground)
        .on_click(move |_, window, cx| commands::execute(command, Value::Null, window, cx))
}
