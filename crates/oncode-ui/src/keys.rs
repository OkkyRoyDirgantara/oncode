//! Penulisan pintasan keyboard: bentuk kanonis, tampilan, dan pengurai `keybindings.json`.
//! Logika murni; aturannya sama dengan versi 0.2 agar berkas pengguna tetap berlaku.

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
            // F1..F12 dan nama lain: huruf pertama besar, sisanya kecil.
            let rest: String = lower.chars().skip(1).collect();
            format!("{}{}", c.to_uppercase(), rest)
        }
        _ => String::new(),
    }
}

/// Menormalkan penulisan pintasan: urutan modifier tetap, tombol huruf besar.
/// `Mod` berarti Ctrl di Linux/Windows dan Cmd di macOS.
pub fn canonical_keybinding(binding: &str) -> String {
    canonical_for(binding, IS_MAC)
}

fn canonical_for(binding: &str, mac: bool) -> String {
    let trimmed = binding.trim();
    let mut parts: Vec<&str> = trimmed.split('+').map(str::trim).filter(|p| !p.is_empty()).collect();
    // "Ctrl++" berarti tombol plus.
    if trimmed.ends_with("++") || trimmed == "+" {
        parts.push("+");
    }
    let Some((key, mods)) = parts.split_last() else { return String::new() };
    let key = canonical_key(key);
    let mods: Vec<String> = mods
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
    let mut out: Vec<&str> = MODIFIER_ORDER.iter().copied().filter(|m| mods.iter().any(|x| x == m)).collect();
    out.push(&key);
    out.join("+")
}

/// Modifier dan tombol sebuah penekanan, tanpa bergantung pada tipe GPUI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyPress {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    /// Cmd di macOS, Super/Win di tempat lain.
    pub platform: bool,
    pub key: String,
}

/// Pintasan kanonis untuk sebuah penekanan tombol, atau `None` bila hanya modifier.
pub fn keybinding_from_press(press: &KeyPress) -> Option<String> {
    press_for(press, IS_MAC)
}

fn press_for(press: &KeyPress, mac: bool) -> Option<String> {
    let lower = press.key.to_lowercase();
    if press.key.is_empty() || matches!(lower.as_str(), "shift" | "control" | "alt" | "platform" | "function" | "cmd" | "super") {
        return None;
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
    if press.shift {
        mods.push("Shift");
    }
    let key = canonical_key(&press.key);
    mods.push(&key);
    Some(mods.join("+"))
}

/// Teks pintasan untuk ditampilkan, mis. "Ctrl+Shift+P" atau "⌘⇧P".
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

/// Mengurai isi `keybindings.json`: objek `{ "<id command>": "<pintasan>" | ["<pintasan>", ...] }`.
/// Baris komentar `//` diabaikan. Daftar kosong menghapus pintasan bawaan.
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
        assert_eq!(press_for(&press, false).as_deref(), Some("Mod+Shift+P"));
        let press = KeyPress { platform: true, key: "s".into(), ..Default::default() };
        assert_eq!(press_for(&press, true).as_deref(), Some("Mod+S"));
        let press = KeyPress { key: "escape".into(), ..Default::default() };
        assert_eq!(press_for(&press, false).as_deref(), Some("Escape"));
        let press = KeyPress { control: true, key: "pagedown".into(), ..Default::default() };
        assert_eq!(press_for(&press, false).as_deref(), Some("Mod+PageDown"));
        let press = KeyPress { shift: true, key: "shift".into(), ..Default::default() };
        assert_eq!(press_for(&press, false), None);
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
        let parsed = parse_keybindings("// komentar\n{ \"file.save\": \"ctrl+alt+s\", \"view.toggleTerminal\": [], \"a.b\": [\"F5\", \"shift+f5\"] }").unwrap();
        assert_eq!(parsed["file.save"], vec![canonical_keybinding("Mod+Alt+S")]);
        assert!(parsed["view.toggleTerminal"].is_empty());
        assert_eq!(parsed["a.b"], vec!["F5".to_string(), "Shift+F5".to_string()]);
        assert!(parse_keybindings("").unwrap().is_empty());
        assert!(parse_keybindings("[]").is_err());
        assert!(parse_keybindings("{ \"a\": 1 }").is_err());
        assert!(parse_keybindings("{ nope").is_err());
    }
}
