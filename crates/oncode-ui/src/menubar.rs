//! Menu di bilah judul (File, Edit, View, Go, Git, Terminal, Help). Setiap item menjalankan
//! command dari registry jendela, jadi label dan pintasannya selalu sama dengan command palette.
//! Item yang command-nya tidak terdaftar di jendela ini disembunyikan (mis. Git di folder tanpa
//! repositori); item yang belum bisa dipakai sekarang ditampilkan nonaktif.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{h_flex, ActiveTheme as _, Sizable as _};
use gpui_kit::{div, px, App, InteractiveElement as _, IntoElement, ParentElement as _, SharedString, Styled as _, Window};
use serde_json::Value;

use crate::commands::{self, Command};
use crate::keys;

/// Menu dan isinya: id command, `None` = pemisah.
pub const MENUS: &[(&str, &[Option<&str>])] = &[
    (
        "File",
        &[
            Some("file.newUntitled"),
            Some("window.new"),
            None,
            Some("file.openFolder"),
            Some("file.openRecent"),
            Some("git.clone"),
            None,
            Some("file.save"),
            Some("file.saveAll"),
            None,
            Some("settings.open"),
            Some("keybindings.openJson"),
            None,
            Some("file.closeFolder"),
        ],
    ),
    (
        "Edit",
        &[
            Some("editor.find"),
            Some("search.focus"),
            Some("search.replace"),
            None,
            Some("editor.format"),
            Some("editor.moveLineUp"),
            Some("editor.moveLineDown"),
            None,
            Some("editor.rename"),
            Some("editor.copyPath"),
        ],
    ),
    (
        "View",
        &[
            Some("workbench.commandPalette"),
            None,
            Some("explorer.focus"),
            Some("scm.focus"),
            Some("git.showGraph"),
            None,
            Some("workbench.toggleSidebar"),
            Some("workbench.togglePanel"),
            Some("terminal.toggle"),
            None,
            Some("workbench.split"),
            Some("workbench.toggleSplitDirection"),
            None,
            Some("markdown.showPreviewToSide"),
        ],
    ),
    (
        "Go",
        &[
            Some("workbench.quickOpen"),
            Some("editor.gotoLine"),
            None,
            Some("editor.goToDefinition"),
            Some("editor.findReferences"),
            None,
            Some("workbench.nextTab"),
            Some("workbench.previousTab"),
        ],
    ),
    (
        "Git",
        &[
            Some("git.commit"),
            Some("git.generateCommitMessage"),
            None,
            Some("git.pull"),
            Some("git.push"),
            Some("git.fetch"),
            Some("git.sync"),
            None,
            Some("git.switchBranch"),
            Some("git.createBranch"),
            Some("git.stash"),
            None,
            Some("git.init"),
            Some("forge.signIn"),
        ],
    ),
    ("Terminal", &[Some("terminal.new"), Some("terminal.toggle"), Some("terminal.focusNext"), None, Some("terminal.clear"), Some("terminal.kill")]),
    ("Help", &[Some("workbench.commandPalette"), Some("keybindings.openJson"), Some("settings.openJson")]),
];

/// Command terdaftar dari sebuah menu, dengan pemisah di antara kelompok yang tidak kosong.
fn entries(items: &[Option<&str>], window: &Window, cx: &App) -> Vec<Option<Command>> {
    let mut out: Vec<Option<Command>> = Vec::new();
    let mut separator = false;
    for item in items {
        match item {
            None => separator = true,
            Some(id) => {
                let Some(command) = commands::get(id, window, cx) else { continue };
                if separator && !out.is_empty() {
                    out.push(None);
                }
                separator = false;
                out.push(Some(command));
            }
        }
    }
    out
}

fn build(mut menu: PopupMenu, items: &[Option<&str>], window: &Window, cx: &App) -> PopupMenu {
    let muted = cx.theme().muted_foreground;
    for entry in entries(items, window, cx) {
        let Some(command) = entry else {
            menu = menu.separator();
            continue;
        };
        let enabled = command.when.as_ref().is_none_or(|when| when(window, cx));
        let hint: SharedString = commands::keybindings_for(&command, cx).first().map(|binding| keys::format_keybinding(binding)).unwrap_or_default().into();
        let (title, id) = (command.title.clone(), command.id.clone());
        let item = PopupMenuItem::element(move |_, _| {
            h_flex().w_full().gap_6().justify_between().child(div().child(title.clone())).child(div().text_xs().text_color(muted).child(hint.clone()))
        })
        .disabled(!enabled)
        .on_click(move |_, window, cx| commands::execute(id.clone(), Value::Null, window, cx));
        menu = menu.item(item);
    }
    menu.min_w(px(260.))
}

/// Deretan tombol menu untuk bilah judul. Menu tanpa satu pun command terdaftar tidak ditampilkan.
pub fn menu_bar(window: &Window, cx: &App) -> impl IntoElement {
    h_flex().id("menu-bar").gap_0p5().children(MENUS.iter().filter(|(_, items)| !entries(items, window, cx).is_empty()).map(|(title, items)| {
        Button::new(SharedString::from(format!("menu-{}", title.to_lowercase())))
            .ghost()
            .xsmall()
            .label(*title)
            .dropdown_menu(move |menu: PopupMenu, window, cx| build(menu, items, window, cx))
    }))
}
