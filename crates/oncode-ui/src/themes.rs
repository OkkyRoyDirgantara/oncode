//! Bundled color themes. Each one is a small palette, turned at startup into gpui-component's theme format
//! (a `ThemeSet`) and added to its `ThemeRegistry`, next to the two default themes. `workbench.theme` picks
//! one ([`ThemeSetting`] lists them all), or System, which switches between the two default themes.
//!
//! The palettes follow the published colors of open-source themes, all MIT licensed; each names its source.
//! Colors that are not given here are derived by gpui-component (button hovers, links, chart colors, …).

use gpui_kit::component::{Theme, ThemeMode, ThemeRegistry};
use gpui_kit::{App, Window};
use oncode_core::settings::Theme as ThemeSetting;
use oncode_core::Settings;
use serde_json::{json, Map, Value};

/// Syntax colors, as tree-sitter captures (see gpui-component's highlighter).
struct Syntax {
    keyword: &'static str,
    function: &'static str,
    kind: &'static str,
    string: &'static str,
    number: &'static str,
    constant: &'static str,
    property: &'static str,
    variable: &'static str,
    attribute: &'static str,
    tag: &'static str,
    operator: &'static str,
    punctuation: &'static str,
    escape: &'static str,
    /// `self`, `this` and the like.
    special: &'static str,
    /// Headings in Markdown.
    title: &'static str,
}

struct Palette {
    /// The theme it is; its label is the name in the theme registry.
    theme: ThemeSetting,
    author: &'static str,
    url: &'static str,
    /// The editor, active tab, and dialogs.
    bg: &'static str,
    /// The sidebar, tab bar, and lists.
    panel: &'static str,
    /// The title bar and status bar.
    chrome: &'static str,
    /// Menus and popups.
    popover: &'static str,
    border: &'static str,
    fg: &'static str,
    muted: &'static str,
    hover: &'static str,
    /// The selected row in a list.
    active: &'static str,
    /// Selected text.
    selection: &'static str,
    /// The line with the cursor.
    line: &'static str,
    /// Primary buttons, focus rings, and the selection border in lists.
    accent: &'static str,
    /// Text on the accent color.
    on_accent: &'static str,
    link: &'static str,
    cursor: &'static str,
    red: &'static str,
    green: &'static str,
    yellow: &'static str,
    blue: &'static str,
    magenta: &'static str,
    cyan: &'static str,
    /// The terminal's bright red, green, yellow, blue, magenta, and cyan.
    bright: [&'static str; 6],
    comment: &'static str,
    italic_comments: bool,
    /// Line numbers, and the one of the current line.
    gutter: &'static str,
    gutter_active: &'static str,
    syntax: Syntax,
}

const PALETTES: &[Palette] = &[
    Palette {
        theme: ThemeSetting::OneDark,
        author: "Atom One Dark (GitHub, MIT)",
        url: "https://github.com/atom/atom/tree/master/packages/one-dark-syntax",
        bg: "#282c34",
        panel: "#21252b",
        chrome: "#21252b",
        popover: "#21252b",
        border: "#181a1f",
        fg: "#abb2bf",
        muted: "#7f848e",
        hover: "#2c313a",
        active: "#3a3f4b",
        selection: "#3e4451",
        line: "#2c313c",
        accent: "#4d78cc",
        on_accent: "#ffffff",
        link: "#61afef",
        cursor: "#528bff",
        red: "#e06c75",
        green: "#98c379",
        yellow: "#e5c07b",
        blue: "#61afef",
        magenta: "#c678dd",
        cyan: "#56b6c2",
        bright: ["#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2"],
        comment: "#5c6370",
        italic_comments: true,
        gutter: "#495162",
        gutter_active: "#abb2bf",
        syntax: Syntax {
            keyword: "#c678dd",
            function: "#61afef",
            kind: "#e5c07b",
            string: "#98c379",
            number: "#d19a66",
            constant: "#d19a66",
            property: "#e06c75",
            variable: "#abb2bf",
            attribute: "#d19a66",
            tag: "#e06c75",
            operator: "#56b6c2",
            punctuation: "#abb2bf",
            escape: "#56b6c2",
            special: "#e5c07b",
            title: "#e06c75",
        },
    },
    Palette {
        theme: ThemeSetting::Dracula,
        author: "Dracula Theme (MIT)",
        url: "https://draculatheme.com",
        bg: "#282a36",
        panel: "#21222c",
        chrome: "#191a21",
        popover: "#21222c",
        border: "#191a21",
        fg: "#f8f8f2",
        muted: "#9ea8c7",
        hover: "#343746",
        active: "#44475a",
        selection: "#44475a",
        line: "#343746",
        accent: "#bd93f9",
        on_accent: "#282a36",
        link: "#8be9fd",
        cursor: "#f8f8f2",
        red: "#ff5555",
        green: "#50fa7b",
        yellow: "#f1fa8c",
        blue: "#bd93f9",
        magenta: "#ff79c6",
        cyan: "#8be9fd",
        bright: ["#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df", "#a4ffff"],
        comment: "#6272a4",
        italic_comments: false,
        gutter: "#6272a4",
        gutter_active: "#f8f8f2",
        syntax: Syntax {
            keyword: "#ff79c6",
            function: "#50fa7b",
            kind: "#8be9fd",
            string: "#f1fa8c",
            number: "#bd93f9",
            constant: "#bd93f9",
            property: "#66d9ef",
            variable: "#f8f8f2",
            attribute: "#50fa7b",
            tag: "#ff79c6",
            operator: "#ff79c6",
            punctuation: "#f8f8f2",
            escape: "#ff79c6",
            special: "#bd93f9",
            title: "#bd93f9",
        },
    },
    Palette {
        theme: ThemeSetting::Nord,
        author: "Arctic Ice Studio and Sven Greb (MIT)",
        url: "https://www.nordtheme.com",
        bg: "#2e3440",
        panel: "#2e3440",
        chrome: "#2e3440",
        popover: "#3b4252",
        border: "#3b4252",
        fg: "#d8dee9",
        muted: "#8b95a8",
        hover: "#3b4252",
        active: "#434c5e",
        selection: "#434c5e",
        line: "#3b4252",
        accent: "#88c0d0",
        on_accent: "#2e3440",
        link: "#88c0d0",
        cursor: "#d8dee9",
        red: "#bf616a",
        green: "#a3be8c",
        yellow: "#ebcb8b",
        blue: "#81a1c1",
        magenta: "#b48ead",
        cyan: "#88c0d0",
        bright: ["#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#8fbcbb"],
        comment: "#616e88",
        italic_comments: false,
        gutter: "#4c566a",
        gutter_active: "#d8dee9",
        syntax: Syntax {
            keyword: "#81a1c1",
            function: "#88c0d0",
            kind: "#8fbcbb",
            string: "#a3be8c",
            number: "#b48ead",
            constant: "#81a1c1",
            property: "#d8dee9",
            variable: "#d8dee9",
            attribute: "#8fbcbb",
            tag: "#81a1c1",
            operator: "#81a1c1",
            punctuation: "#eceff4",
            escape: "#ebcb8b",
            special: "#81a1c1",
            title: "#88c0d0",
        },
    },
    Palette {
        theme: ThemeSetting::GruvboxDark,
        author: "Pavel Pertsev (MIT)",
        url: "https://github.com/morhetz/gruvbox",
        bg: "#282828",
        panel: "#32302f",
        chrome: "#1d2021",
        popover: "#32302f",
        border: "#3c3836",
        fg: "#ebdbb2",
        muted: "#a89984",
        hover: "#3c3836",
        active: "#504945",
        selection: "#504945",
        line: "#32302f",
        accent: "#d79921",
        on_accent: "#282828",
        link: "#83a598",
        cursor: "#ebdbb2",
        red: "#cc241d",
        green: "#98971a",
        yellow: "#d79921",
        blue: "#458588",
        magenta: "#b16286",
        cyan: "#689d6a",
        bright: ["#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b", "#8ec07c"],
        comment: "#928374",
        italic_comments: true,
        gutter: "#7c6f64",
        gutter_active: "#fabd2f",
        syntax: Syntax {
            keyword: "#fb4934",
            function: "#b8bb26",
            kind: "#fabd2f",
            string: "#b8bb26",
            number: "#d3869b",
            constant: "#d3869b",
            property: "#83a598",
            variable: "#ebdbb2",
            attribute: "#8ec07c",
            tag: "#83a598",
            operator: "#fe8019",
            punctuation: "#a89984",
            escape: "#fe8019",
            special: "#fe8019",
            title: "#fabd2f",
        },
    },
    Palette {
        theme: ThemeSetting::TokyoNight,
        author: "enkia (MIT)",
        url: "https://github.com/enkia/tokyo-night-vscode-theme",
        bg: "#1a1b26",
        panel: "#16161e",
        chrome: "#16161e",
        popover: "#16161e",
        border: "#101014",
        fg: "#c0caf5",
        muted: "#787c99",
        hover: "#1f2233",
        active: "#2a2f45",
        selection: "#283457",
        line: "#1e202e",
        accent: "#3d59a1",
        on_accent: "#ffffff",
        link: "#7aa2f7",
        cursor: "#c0caf5",
        red: "#f7768e",
        green: "#9ece6a",
        yellow: "#e0af68",
        blue: "#7aa2f7",
        magenta: "#bb9af7",
        cyan: "#7dcfff",
        bright: ["#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff"],
        comment: "#565f89",
        italic_comments: true,
        gutter: "#363b54",
        gutter_active: "#737aa2",
        syntax: Syntax {
            keyword: "#bb9af7",
            function: "#7aa2f7",
            kind: "#2ac3de",
            string: "#9ece6a",
            number: "#ff9e64",
            constant: "#ff9e64",
            property: "#73daca",
            variable: "#c0caf5",
            attribute: "#e0af68",
            tag: "#f7768e",
            operator: "#89ddff",
            punctuation: "#a9b1d6",
            escape: "#89ddff",
            special: "#f7768e",
            title: "#7aa2f7",
        },
    },
    Palette {
        theme: ThemeSetting::CatppuccinMocha,
        author: "Catppuccin (MIT)",
        url: "https://catppuccin.com",
        bg: "#1e1e2e",
        panel: "#181825",
        chrome: "#11111b",
        popover: "#181825",
        border: "#313244",
        fg: "#cdd6f4",
        muted: "#9399b2",
        hover: "#313244",
        active: "#45475a",
        selection: "#585b70",
        line: "#2a2b3c",
        accent: "#cba6f7",
        on_accent: "#11111b",
        link: "#89b4fa",
        cursor: "#f5e0dc",
        red: "#f38ba8",
        green: "#a6e3a1",
        yellow: "#f9e2af",
        blue: "#89b4fa",
        magenta: "#f5c2e7",
        cyan: "#94e2d5",
        bright: ["#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5"],
        comment: "#9399b2",
        italic_comments: true,
        gutter: "#45475a",
        gutter_active: "#b4befe",
        syntax: Syntax {
            keyword: "#cba6f7",
            function: "#89b4fa",
            kind: "#f9e2af",
            string: "#a6e3a1",
            number: "#fab387",
            constant: "#fab387",
            property: "#b4befe",
            variable: "#cdd6f4",
            attribute: "#f9e2af",
            tag: "#89b4fa",
            operator: "#89dceb",
            punctuation: "#9399b2",
            escape: "#f5c2e7",
            special: "#f38ba8",
            title: "#89b4fa",
        },
    },
    Palette {
        theme: ThemeSetting::OneLight,
        author: "Atom One Light (GitHub, MIT)",
        url: "https://github.com/atom/atom/tree/master/packages/one-light-syntax",
        bg: "#fafafa",
        panel: "#f0f0f1",
        chrome: "#eaeaeb",
        popover: "#ffffff",
        border: "#dbdbdc",
        fg: "#383a42",
        muted: "#696c77",
        hover: "#e5e5e6",
        active: "#dbdbdc",
        selection: "#e5e5e6",
        line: "#f2f2f2",
        accent: "#526fff",
        on_accent: "#ffffff",
        link: "#4078f2",
        cursor: "#526fff",
        red: "#e45649",
        green: "#50a14f",
        yellow: "#c18401",
        blue: "#4078f2",
        magenta: "#a626a4",
        cyan: "#0184bc",
        bright: ["#e45649", "#50a14f", "#c18401", "#4078f2", "#a626a4", "#0184bc"],
        comment: "#a0a1a7",
        italic_comments: true,
        gutter: "#9d9d9f",
        gutter_active: "#383a42",
        syntax: Syntax {
            keyword: "#a626a4",
            function: "#4078f2",
            kind: "#c18401",
            string: "#50a14f",
            number: "#986801",
            constant: "#986801",
            property: "#e45649",
            variable: "#383a42",
            attribute: "#986801",
            tag: "#e45649",
            operator: "#0184bc",
            punctuation: "#383a42",
            escape: "#0184bc",
            special: "#e45649",
            title: "#e45649",
        },
    },
    Palette {
        theme: ThemeSetting::SolarizedLight,
        author: "Ethan Schoonover (MIT)",
        url: "https://ethanschoonover.com/solarized/",
        bg: "#fdf6e3",
        panel: "#eee8d5",
        chrome: "#eee8d5",
        popover: "#fdf6e3",
        border: "#ddd6c1",
        fg: "#586e75",
        muted: "#839496",
        hover: "#e4ddc8",
        active: "#d9d2bd",
        selection: "#eee8d5",
        line: "#f5efdc",
        accent: "#268bd2",
        on_accent: "#fdf6e3",
        link: "#268bd2",
        cursor: "#657b83",
        red: "#dc322f",
        green: "#859900",
        yellow: "#b58900",
        blue: "#268bd2",
        magenta: "#d33682",
        cyan: "#2aa198",
        bright: ["#cb4b16", "#859900", "#b58900", "#268bd2", "#6c71c4", "#2aa198"],
        comment: "#93a1a1",
        italic_comments: true,
        gutter: "#93a1a1",
        gutter_active: "#586e75",
        syntax: Syntax {
            keyword: "#859900",
            function: "#268bd2",
            kind: "#b58900",
            string: "#2aa198",
            number: "#d33682",
            constant: "#cb4b16",
            property: "#268bd2",
            variable: "#657b83",
            attribute: "#b58900",
            tag: "#268bd2",
            operator: "#859900",
            punctuation: "#657b83",
            escape: "#dc322f",
            special: "#268bd2",
            title: "#cb4b16",
        },
    },
    Palette {
        theme: ThemeSetting::CatppuccinLatte,
        author: "Catppuccin (MIT)",
        url: "https://catppuccin.com",
        bg: "#eff1f5",
        panel: "#e6e9ef",
        chrome: "#dce0e8",
        popover: "#eff1f5",
        border: "#ccd0da",
        fg: "#4c4f69",
        muted: "#6c6f85",
        hover: "#dce0e8",
        active: "#ccd0da",
        selection: "#ccd0da",
        line: "#e6e9ef",
        accent: "#8839ef",
        on_accent: "#eff1f5",
        link: "#1e66f5",
        cursor: "#dc8a78",
        red: "#d20f39",
        green: "#40a02b",
        yellow: "#df8e1d",
        blue: "#1e66f5",
        magenta: "#ea76cb",
        cyan: "#179299",
        bright: ["#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb", "#179299"],
        comment: "#7c7f93",
        italic_comments: true,
        gutter: "#8c8fa1",
        gutter_active: "#7287fd",
        syntax: Syntax {
            keyword: "#8839ef",
            function: "#1e66f5",
            kind: "#df8e1d",
            string: "#40a02b",
            number: "#fe640b",
            constant: "#fe640b",
            property: "#7287fd",
            variable: "#4c4f69",
            attribute: "#df8e1d",
            tag: "#1e66f5",
            operator: "#04a5e5",
            punctuation: "#7c7f93",
            escape: "#ea76cb",
            special: "#d20f39",
            title: "#1e66f5",
        },
    },
];

/// Adds the bundled themes to gpui-component's registry. Called once, after `gpui_kit::init`.
pub(crate) fn install(cx: &mut App) {
    let registry = ThemeRegistry::global_mut(cx);
    for palette in PALETTES {
        if let Err(error) = registry.load_themes_from_str(&theme_set(palette).to_string()) {
            eprintln!("oncode: theme {} could not be loaded: {error}", palette.theme.label());
        }
    }
}

/// Applies the theme from the settings.
pub fn apply(settings: &Settings, window: &mut Window, cx: &mut App) {
    show(settings.workbench.theme, window, cx);
}

/// Shows a theme in every window: from the settings, or while it is highlighted in the theme picker.
pub fn show(theme: ThemeSetting, window: &mut Window, cx: &mut App) {
    if !cx.has_global::<Theme>() {
        Theme::change(ThemeMode::Light, None, cx);
    }
    let registry = ThemeRegistry::global(cx);
    let (mut light, mut dark) = (registry.default_light_theme().clone(), registry.default_dark_theme().clone());
    let mode = match theme.is_dark() {
        None => None,
        Some(is_dark) => {
            let config = registry.themes().get(theme.label()).filter(|config| config.mode.is_dark() == is_dark).cloned();
            if let Some(config) = config {
                if is_dark {
                    dark = config;
                } else {
                    light = config;
                }
            }
            Some(if is_dark { ThemeMode::Dark } else { ThemeMode::Light })
        }
    };
    let global = Theme::global_mut(cx);
    global.light_theme = light;
    global.dark_theme = dark;
    match mode {
        Some(mode) => Theme::change(mode, Some(window), cx),
        None => Theme::sync_system_appearance(Some(window), cx),
    }
}

/// The palette in gpui-component's theme file format.
fn theme_set(p: &Palette) -> Value {
    let dark = p.theme.is_dark() == Some(true);
    let [red_bright, green_bright, yellow_bright, blue_bright, magenta_bright, cyan_bright] = p.bright;
    let transparent = "#00000000";
    let tint = |color: &str| format!("{color}26");
    let colors: Map<String, Value> = [
        ("background", p.bg.into()),
        ("foreground", p.fg.into()),
        ("border", p.border.into()),
        ("input.border", p.border.into()),
        ("muted.background", p.hover.into()),
        ("muted.foreground", p.muted.into()),
        ("primary.background", p.accent.into()),
        ("primary.foreground", p.on_accent.into()),
        ("secondary.background", p.hover.into()),
        ("secondary.hover.background", p.active.into()),
        ("secondary.active.background", p.active.into()),
        ("secondary.foreground", p.fg.into()),
        ("accent.background", p.hover.into()),
        ("accent.foreground", p.fg.into()),
        ("accordion.background", p.bg.into()),
        ("group_box.background", p.panel.into()),
        ("group_box.foreground", p.fg.into()),
        ("popover.background", p.popover.into()),
        ("popover.foreground", p.fg.into()),
        ("list.background", p.panel.into()),
        ("list.active.background", p.active.into()),
        ("list.active.border", p.accent.into()),
        ("list.hover.background", p.hover.into()),
        ("list.even.background", p.panel.into()),
        ("list.head.background", p.panel.into()),
        ("list.highlight.foreground", p.link.into()),
        ("table.background", p.bg.into()),
        ("table.active.background", p.active.into()),
        ("table.active.border", p.accent.into()),
        ("table.even.background", p.panel.into()),
        ("table.head.background", p.panel.into()),
        ("table.head.foreground", p.muted.into()),
        ("table.hover.background", p.hover.into()),
        ("table.row.border", p.border.into()),
        ("link", p.link.into()),
        ("ring", p.accent.into()),
        ("caret", p.cursor.into()),
        ("selection.background", p.selection.into()),
        ("drag.border", p.accent.into()),
        ("drop_target.background", format!("{}40", p.accent).into()),
        ("sidebar.background", p.panel.into()),
        ("sidebar.foreground", p.fg.into()),
        ("sidebar.border", p.border.into()),
        ("sidebar.accent.background", p.hover.into()),
        ("sidebar.accent.foreground", p.fg.into()),
        ("sidebar.primary.background", p.accent.into()),
        ("sidebar.primary.foreground", p.on_accent.into()),
        ("skeleton.background", p.hover.into()),
        ("switch.background", p.active.into()),
        ("progress.bar.background", p.accent.into()),
        ("slider.background", p.accent.into()),
        ("slider.thumb.background", p.bg.into()),
        ("tab.background", transparent.into()),
        ("tab.foreground", p.muted.into()),
        ("tab.active.background", p.bg.into()),
        ("tab.active.foreground", p.fg.into()),
        ("tab_bar.background", p.panel.into()),
        ("tab_bar.segmented.background", p.panel.into()),
        ("title_bar.background", p.chrome.into()),
        ("title_bar.border", p.border.into()),
        ("status_bar.background", p.chrome.into()),
        ("status_bar.border", p.border.into()),
        ("scrollbar.background", transparent.into()),
        ("scrollbar.thumb.background", format!("{}66", p.muted).into()),
        ("scrollbar.thumb.hover.background", format!("{}99", p.muted).into()),
        ("overlay", (if dark { "#00000066" } else { "#0000001a" }).into()),
        ("window.border", p.border.into()),
        ("base.red", p.red.into()),
        ("base.red.light", red_bright.into()),
        ("base.green", p.green.into()),
        ("base.green.light", green_bright.into()),
        ("base.yellow", p.yellow.into()),
        ("base.yellow.light", yellow_bright.into()),
        ("base.blue", p.blue.into()),
        ("base.blue.light", blue_bright.into()),
        ("base.magenta", p.magenta.into()),
        ("base.magenta.light", magenta_bright.into()),
        ("base.cyan", p.cyan.into()),
        ("base.cyan.light", cyan_bright.into()),
    ]
    .into_iter()
    .map(|(key, value): (&str, String)| (key.to_string(), Value::String(value)))
    .collect();
    let s = &p.syntax;
    let color = |color: &str| json!({ "color": color });
    let comment = if p.italic_comments { json!({ "color": p.comment, "font_style": "italic" }) } else { color(p.comment) };
    let mut syntax = Map::new();
    for (capture, style) in [
        ("attribute", color(s.attribute)),
        ("boolean", color(s.constant)),
        ("comment", comment.clone()),
        ("comment_doc", comment),
        ("constant", color(s.constant)),
        ("constructor", color(s.kind)),
        ("embedded", color(p.fg)),
        ("emphasis", json!({ "font_style": "italic" })),
        ("emphasis.strong", json!({ "font_weight": 700 })),
        ("enum", color(s.kind)),
        ("function", color(s.function)),
        ("hint", color(p.comment)),
        ("keyword", color(s.keyword)),
        ("label", color(s.tag)),
        ("link_text", color(p.link)),
        ("link_uri", json!({ "color": p.cyan, "font_style": "italic" })),
        ("number", color(s.number)),
        ("operator", color(s.operator)),
        ("predictive", color(p.comment)),
        ("preproc", color(s.keyword)),
        ("property", color(s.property)),
        ("punctuation", color(s.punctuation)),
        ("punctuation.bracket", color(s.punctuation)),
        ("punctuation.delimiter", color(s.punctuation)),
        ("punctuation.list_marker", color(s.keyword)),
        ("punctuation.special", color(s.keyword)),
        ("string", color(s.string)),
        ("string.escape", color(s.escape)),
        ("string.regex", color(s.escape)),
        ("string.special", color(s.string)),
        ("string.special.symbol", color(s.constant)),
        ("tag", color(s.tag)),
        ("tag.doctype", color(s.keyword)),
        ("text.code.span", color(s.string)),
        ("text.literal", color(s.string)),
        ("title", json!({ "color": s.title, "font_weight": 700 })),
        ("type", color(s.kind)),
        ("variable", color(s.variable)),
        ("variable.special", color(s.special)),
        ("variant", color(s.constant)),
    ] {
        syntax.insert(capture.into(), style);
    }
    let highlight = json!({
        "editor.foreground": p.fg,
        "editor.background": p.bg,
        "editor.active_line.background": p.line,
        "editor.line_number": p.gutter,
        "editor.active_line_number": p.gutter_active,
        "editor.invisible": format!("{}66", p.gutter),
        "created": p.green,
        "deleted": p.red,
        "modified": p.yellow,
        "error": p.red,
        "error.background": tint(p.red),
        "error.border": p.red,
        "warning": p.yellow,
        "warning.background": tint(p.yellow),
        "warning.border": p.yellow,
        "info": p.blue,
        "info.background": tint(p.blue),
        "info.border": p.blue,
        "success": p.green,
        "success.background": tint(p.green),
        "success.border": p.green,
        "hint": p.comment,
        "hint.background": tint(p.comment),
        "hint.border": p.comment,
        "syntax": syntax,
    });
    let name = p.theme.label();
    json!({
        "name": name,
        "author": p.author,
        "url": p.url,
        "themes": [{ "name": name, "mode": if dark { "dark" } else { "light" }, "colors": colors, "highlight": highlight }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::component::ThemeSet;

    /// Every key is one gpui-component knows: a misspelled key would be dropped silently and its color
    /// replaced by the default theme's.
    #[test]
    fn every_bundled_theme_parses_without_losing_keys() {
        for palette in PALETTES {
            let name = palette.theme.label();
            let written = theme_set(palette);
            let parsed: ThemeSet = serde_json::from_value(written.clone()).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(parsed.themes.len(), 1);
            assert_eq!(Some(parsed.themes[0].mode.is_dark()), palette.theme.is_dark(), "{name}");
            let reread = serde_json::to_value(&parsed.themes[0]).unwrap();
            for section in ["colors", "highlight"] {
                for key in written["themes"][0][section].as_object().unwrap().keys() {
                    assert!(!reread[section][key].is_null(), "{name}: {section}.{key} is not a known key");
                }
            }
            for capture in written["themes"][0]["highlight"]["syntax"].as_object().unwrap().keys() {
                assert!(!reread["highlight"]["syntax"][capture].is_null(), "{name}: syntax.{capture} is not a known capture");
            }
        }
    }

    /// Every theme in the settings has colors: a bundled palette, or one of gpui-component's defaults.
    #[test]
    fn every_theme_setting_has_one_palette_or_is_a_default() {
        for theme in ThemeSetting::ALL {
            let palettes = PALETTES.iter().filter(|palette| palette.theme == theme).count();
            let builtin = matches!(theme, ThemeSetting::System | ThemeSetting::Dark | ThemeSetting::Light);
            assert_eq!(palettes, usize::from(!builtin), "{theme:?}");
        }
    }
}
