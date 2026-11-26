//! Keyboard shortcut notation: the canonical form, display, and the `keybindings.json` parser.
//! Pure logic; the rules are the same as in version 0.2 so users' files still apply.

use std::collections::BTreeMap;

pub const IS_MAC: bool = cfg!(target_os = "macos");

const MODIFIER_ORDER: [&str; 5] = ["Mod", "Ctrl", "Meta", "Alt", "Shift"];

fn canonical_key(key: &str) -> String {
    let lower = key.to_lowercase();
    let named = match lower.as_str() {
        "arrowup" | "up" => "Up",
        "arrowdown" | "down" => "Down",
        "arrowleft" | "left" => "Left",
        "arrowright" | "right" => "Right",
        "esc" | "escape" => "Escape",
        " " | "spacebar" | "space" => "Space",
        "return" | "enter" => "Enter",
        "del" | "delete" => "Delete",
        "backquote" => "`",
        "backspace" => "Backspace",
        "tab" => "Tab",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        "home" => "Home",
        "end" => "End",
        "insert" => "Insert",
        _ => "",
    };
    if !named.is_empty() {
        return named.to_string();
    }
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => c.to_uppercase().collect(),
        (Some(c), Some(_)) => {
            // F1..F12 and other names: first letter uppercase, the rest lowercase.
            let rest: String = lower.chars().skip(1).collect();
            format!("{}{}", c.to_uppercase(), rest)
        }
        _ => String::new(),
    }
}

/// Normalizes shortcut notation: fixed modifier order, uppercase key.
/// `Mod` means Ctrl on Linux/Windows and Cmd on macOS.
pub fn canonical_keybinding(binding: &str) -> String {
    canonical_for(binding, IS_MAC)
}

fn canonical_for(binding: &str, mac: bool) -> String {
    let trimmed = binding.trim();
    let mut parts: Vec<&str> = trimmed.split('+').map(str::trim).filter(|p| !p.is_empty()).collect();
    // "Ctrl++" means the plus key.
    if trimmed.ends_with("++") || trimmed == "+" {
        parts.push("+");
    }
    let Some((key, mods)) = parts.split_last() else { return String::new() };
    let mut key = canonical_key(key);
    let mut mods: Vec<String> = mods
        .iter()
        .map(|m| match m.to_lowercase().as_str() {
            "cmd" | "meta" | "command" | "super" | "win" => if mac { "Mod" } else { "Meta" }.to_string(),
            "ctrl" | "control" => if mac { "Ctrl" } else { "Mod" }.to_string(),
            "mod" => "Mod".to_string(),
            "alt" | "option" | "opt" => "Alt".to_string(),
            "shift" => "Shift".to_string(),
            _ => m.to_string(),
        })
        .collect();
    // `Ctrl+~` and `Ctrl+Shift+`` are the same shortcut (see `unshifted`).
    if let Some(base) = unshifted(&key) {
        key = base.to_string();
        mods.push("Shift".to_string());
    }
    let mut out: Vec<&str> = MODIFIER_ORDER.iter().copied().filter(|m| mods.iter().any(|x| x == m)).collect();
    out.push(&key);
    out.join("+")
}

/// The modifiers and key of a key press, without depending on GPUI types.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyPress {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    /// Cmd on macOS, Super/Win elsewhere.
    pub platform: bool,
    pub key: String,
}

/// The base key (US layout) for symbols typed with Shift, e.g. `~` → `` ` ``.
///
/// GPUI on Windows reports Ctrl+Shift+` as `ctrl` + `~` (Shift is already "used up" turning into the
/// character), while other platforms may report `ctrl-shift` + `` ` ``. Both are normalized
/// to `Shift+<base key>` so a shortcut like `Mod+Shift+`` matches on all platforms. `+`
/// is deliberately not included: `Ctrl++` means the plus key, which on many layouts does not use Shift.
fn unshifted(key: &str) -> Option<&'static str> {
    Some(match key {
        "~" => "`",
        "!" => "1",
        "@" => "2",
        "#" => "3",
        "$" => "4",
        "%" => "5",
        "^" => "6",
        "&" => "7",
        "*" => "8",
        "(" => "9",
        ")" => "0",
        "_" => "-",
        "{" => "[",
        "}" => "]",
        "|" => "\\",
        ":" => ";",
        "\"" => "'",
        "<" => ",",
        ">" => ".",
        "?" => "/",
        _ => return None,
    })
}

/// The canonical shortcut for a key press, or `None` if it is only a modifier.
pub fn keybinding_from_press(press: &KeyPress) -> Option<String> {
    keybinding_from_press_on(press, IS_MAC)
}

/// Like [`keybinding_from_press`], with the platform chosen by the caller (for cross-platform tests).
pub fn keybinding_from_press_on(press: &KeyPress, mac: bool) -> Option<String> {
    let lower = press.key.to_lowercase();
    if press.key.is_empty() || matches!(lower.as_str(), "shift" | "control" | "alt" | "platform" | "function" | "cmd" | "super") {
        return None;
    }
    let mut key = canonical_key(&press.key);
    let mut shift = press.shift;
    if let Some(base) = unshifted(&key) {
        key = base.to_string();
        shift = true;
    }
    let mut mods: Vec<&str> = Vec::new();
    let (primary, secondary_ctrl, meta) = if mac { (press.platform, press.control, false) } else { (press.control, false, press.platform) };
    if primary {
        mods.push("Mod");
    }
    if secondary_ctrl {
        mods.push("Ctrl");
    }
    if meta {
        mods.push("Meta");
    }
    if press.alt {
        mods.push("Alt");
    }
    if shift {
        mods.push("Shift");
    }
    mods.push(&key);
    Some(mods.join("+"))
}

/// The shortcut text to display, e.g. "Ctrl+Shift+P" or "⌘⇧P".
pub fn format_keybinding(binding: &str) -> String {
    format_for(binding, IS_MAC)
}

fn format_for(binding: &str, mac: bool) -> String {
    let canonical = canonical_for(binding, mac);
    let mut parts: Vec<&str> = canonical.split('+').filter(|p| !p.is_empty()).collect();
    if canonical.ends_with("++") || canonical == "+" {
        parts.push("+");
    }
    let arrow = |p: &str| match p {
        "Up" => Some("↑"),
        "Down" => Some("↓"),
        "Left" => Some("←"),
        "Right" => Some("→"),
        _ => None,
    };
    if mac {
        parts
            .iter()
            .map(|p| match *p {
                "Mod" => "⌘",
                "Ctrl" => "⌃",
                "Alt" => "⌥",
                "Shift" => "⇧",
                "Enter" => "↩",
                other => arrow(other).unwrap_or(other),
            })
            .collect::<Vec<_>>()
            .join("")
    } else {
        parts
            .iter()
            .map(|p| match *p {
                "Mod" => "Ctrl",
                other => arrow(other).unwrap_or(other),
            })
            .collect::<Vec<_>>()
            .join("+")
    }
}

/// Parses the content of `keybindings.json`: an object `{ "<command id>": "<shortcut>" | ["<shortcut>", ...] }`.
/// `//` comment lines are ignored. An empty list removes the default shortcuts.
pub fn parse_keybindings(text: &str) -> Result<BTreeMap<String, Vec<String>>, String> {
    let stripped: String = text.lines().filter(|line| !line.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
    let source = if stripped.trim().is_empty() { "{}" } else { stripped.as_str() };
    let data: serde_json::Value = serde_json::from_str(source).map_err(|e| e.to_string())?;
    let serde_json::Value::Object(map) = data else {
        return Err("The file must contain a JSON object".into());
    };
    let mut overrides = BTreeMap::new();
    for (id, value) in map {
        let list: Vec<String> = match value {
            serde_json::Value::String(s) => vec![s],
            serde_json::Value::Array(items) => {
                let mut out = Vec::new();
                for item in items {
                    match item {
                        serde_json::Value::String(s) => out.push(s),
                        _ => return Err(format!("The value for \"{id}\" must be a string or a list of strings")),
                    }
                }
                out
            }
            _ => return Err(format!("The value for \"{id}\" must be a string or a list of strings")),
        };
        overrides.insert(id, list.iter().map(|b| canonical_keybinding(b)).filter(|b| !b.is_empty()).collect());
    }
    Ok(overrides)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_orders_modifiers_and_uppercases_key() {
        assert_eq!(canonical_for("shift+ctrl+p", false), "Mod+Shift+P");
        assert_eq!(canonical_for("Mod+Shift+P", false), "Mod+Shift+P");
        assert_eq!(canonical_for("Alt+ArrowUp", false), "Alt+Up");
        assert_eq!(canonical_for("esc", false), "Escape");
        assert_eq!(canonical_for("Mod+`", false), "Mod+`");
        assert_eq!(canonical_for("f2", false), "F2");
        assert_eq!(canonical_for("Mod+PageDown", false), "Mod+PageDown");
        assert_eq!(canonical_for("", false), "");
    }

    #[test]
    fn canonical_handles_plus_key_and_platforms() {
        assert_eq!(canonical_for("Ctrl++", false), "Mod++");
        assert_eq!(canonical_for("Cmd+S", true), "Mod+S");
        assert_eq!(canonical_for("Ctrl+S", true), "Ctrl+S");
        assert_eq!(canonical_for("Cmd+S", false), "Meta+S");
    }

    #[test]
    fn press_maps_to_canonical_binding() {
        let press = KeyPress { control: true, shift: true, key: "p".into(), ..Default::default() };
        assert_eq!(keybinding_from_press_on(&press, false).as_deref(), Some("Mod+Shift+P"));
        let press = KeyPress { platform: true, key: "s".into(), ..Default::default() };
        assert_eq!(keybinding_from_press_on(&press, true).as_deref(), Some("Mod+S"));
        let press = KeyPress { key: "escape".into(), ..Default::default() };
        assert_eq!(keybinding_from_press_on(&press, false).as_deref(), Some("Escape"));
        let press = KeyPress { control: true, key: "pagedown".into(), ..Default::default() };
        assert_eq!(keybinding_from_press_on(&press, false).as_deref(), Some("Mod+PageDown"));
        let press = KeyPress { shift: true, key: "shift".into(), ..Default::default() };
        assert_eq!(keybinding_from_press_on(&press, false), None);
    }

    #[test]
    fn shifted_symbols_match_their_base_key_with_shift() {
        // Windows: Ctrl+Shift+` arrives as ctrl + "~" without Shift; other platforms may send ctrl-shift + "`".
        let windows = KeyPress { control: true, key: "~".into(), ..Default::default() };
        let other = KeyPress { control: true, shift: true, key: "`".into(), ..Default::default() };
        assert_eq!(keybinding_from_press_on(&windows, false).as_deref(), Some("Mod+Shift+`"));
        assert_eq!(keybinding_from_press_on(&other, false).as_deref(), Some("Mod+Shift+`"));
        assert_eq!(canonical_for("Ctrl+Shift+`", false), "Mod+Shift+`");
        assert_eq!(canonical_for("Ctrl+~", false), "Mod+Shift+`");
        assert_eq!(canonical_for("Ctrl+Shift+~", false), "Mod+Shift+`");
        // Shift+digit and other symbols.
        let bang = KeyPress { control: true, key: "!".into(), ..Default::default() };
        assert_eq!(keybinding_from_press_on(&bang, false).as_deref(), Some("Mod+Shift+1"));
        assert_eq!(canonical_for("Ctrl+Shift+1", false), "Mod+Shift+1");
        assert_eq!(canonical_for("Ctrl+?", false), "Mod+Shift+/");
        // Without Shift it stays the base key; `+` is not changed.
        assert_eq!(canonical_for("Mod+`", false), "Mod+`");
        assert_eq!(canonical_for("Ctrl++", false), "Mod++");
        let plus = KeyPress { control: true, key: "+".into(), ..Default::default() };
        assert_eq!(keybinding_from_press_on(&plus, false).as_deref(), Some("Mod++"));
    }

    #[test]
    fn formats_for_display() {
        assert_eq!(format_for("Mod+Shift+P", false), "Ctrl+Shift+P");
        assert_eq!(format_for("Mod+Shift+P", true), "⌘⇧P");
        assert_eq!(format_for("Alt+Up", false), "Alt+↑");
        assert_eq!(format_for("Ctrl++", false), "Ctrl++");
    }

    #[test]
    fn parses_keybindings_file() {
        let parsed = parse_keybindings("// comment\n{ \"file.save\": \"ctrl+alt+s\", \"view.toggleTerminal\": [], \"a.b\": [\"F5\", \"shift+f5\"] }").unwrap();
        // `ctrl` is `Mod` on Windows/Linux, but a separate Control key on macOS (`Mod` = Cmd).
        assert_eq!(parsed["file.save"], vec![canonical_keybinding("Ctrl+Alt+S")]);
        assert_eq!(canonical_for("ctrl+alt+s", false), "Mod+Alt+S");
        assert_eq!(canonical_for("ctrl+alt+s", true), "Ctrl+Alt+S");
        assert!(parsed["view.toggleTerminal"].is_empty());
        assert_eq!(parsed["a.b"], vec!["F5".to_string(), "Shift+F5".to_string()]);
        assert!(parse_keybindings("").unwrap().is_empty());
        assert!(parse_keybindings("[]").is_err());
        assert!(parse_keybindings("{ \"a\": 1 }").is_err());
        assert!(parse_keybindings("{ nope").is_err());
    }
}
