//! Terminal keys: (1) a filter that decides which keys are forwarded to the shell, which are
//! handled by the terminal itself (copy, paste, find), and which are left to the application;
//! (2) translating keys into xterm-style byte sequences; (3) the paste wrapper.
//! Pure (no GPUI) so it can be tested for both platforms.

use std::collections::HashSet;

use oncode_ui::keys::{keybinding_from_press_on, KeyPress};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Shell,
    App,
    Copy,
    Paste,
    Find,
}

/// Application commands that stay in effect while the terminal is focused. Their keybindings are taken
/// from the registry (including user overrides); other keys bound to commands outside this list
/// (e.g. Ctrl+W, Ctrl+S, Ctrl+O) are forwarded to the shell.
pub const COMMANDS_TO_SKIP_SHELL: &[&str] = &[
    "workbench.commandPalette",
    "workbench.quickOpen",
    "workbench.toggleSidebar",
    "workbench.togglePanel",
    "workbench.nextTab",
    "workbench.previousTab",
    "window.new",
    "search.focus",
    "terminal.toggle",
    "terminal.new",
    "terminal.split",
    "terminal.kill",
    "terminal.clear",
    "terminal.focusNext",
    "terminal.focus",
];

/// The canonical keybinding of a key press, in the same format as
/// `oncode_ui::keys::keybinding_from_press`, but with the platform supplied from outside.
/// `None` if it is only a modifier.
pub fn binding_of(press: &KeyPress, is_mac: bool) -> Option<String> {
    keybinding_from_press_on(press, is_mac)
}

/// - `Copy` / `Paste`: Mod+Shift+C / Mod+Shift+V; on macOS also Cmd+C / Cmd+V.
///   A plain Ctrl+C always reaches the shell as SIGINT.
/// - `Find`: Mod+F opens the terminal's find bar.
/// - `App`: the allowed application keybindings; on macOS all other Cmd combinations also belong to
///   the application because Cmd has no meaning for the shell.
/// - `Shell`: everything else.
pub fn classify_key(press: &KeyPress, is_mac: bool, app_bindings: &HashSet<String>) -> KeyAction {
    let Some(binding) = binding_of(press, is_mac) else { return KeyAction::Shell };
    if binding == "Mod+Shift+C" || (is_mac && binding == "Mod+C") {
        return KeyAction::Copy;
    }
    if binding == "Mod+Shift+V" || (is_mac && binding == "Mod+V") {
        return KeyAction::Paste;
    }
    if binding == "Mod+F" {
        return KeyAction::Find;
    }
    if app_bindings.contains(&binding) {
        return KeyAction::App;
    }
    if is_mac && press.platform {
        return KeyAction::App;
    }
    KeyAction::Shell
}

/// Scrolling the terminal history via the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollKey {
    PageUp,
    PageDown,
    LineUp,
    LineDown,
    Top,
    Bottom,
}

/// Shift+PageUp/PageDown (page), Ctrl+Shift+↑/↓ (line), Ctrl+Shift+Home/End (ends), like
/// VS Code. Only applies outside full-screen programs; the caller checks that.
pub fn scroll_key(press: &KeyPress) -> Option<ScrollKey> {
    if press.alt || press.platform || !press.shift {
        return None;
    }
    match (press.control, press.key.to_lowercase().as_str()) {
        (false, "pageup") => Some(ScrollKey::PageUp),
        (false, "pagedown") => Some(ScrollKey::PageDown),
        (true, "up") => Some(ScrollKey::LineUp),
        (true, "down") => Some(ScrollKey::LineDown),
        (true, "home") => Some(ScrollKey::Top),
        (true, "end") => Some(ScrollKey::Bottom),
        _ => None,
    }
}

/// Terminal modes that affect the key byte sequences.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyModes {
    /// DECCKM: arrow keys send `ESC O A` instead of `ESC [ A`.
    pub app_cursor: bool,
    /// LNM: Enter sends CR LF.
    pub line_feed_new_line: bool,
}

/// The xterm modifier parameter: 1 + Shift(1) + Alt(2) + Ctrl(4).
fn modifier_param(press: &KeyPress) -> u8 {
    1 + u8::from(press.shift) + 2 * u8::from(press.alt) + 4 * u8::from(press.control)
}

/// The control code for Ctrl+<key>, following xterm.
fn control_code(key: &str) -> Option<u8> {
    let mut chars = key.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return match key {
            "space" => Some(0x00),
            _ => None,
        };
    };
    match c.to_ascii_lowercase() {
        c @ 'a'..='z' => Some(c as u8 - b'a' + 1),
        '@' | ' ' | '2' => Some(0x00),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '/' | '7' | '-' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

/// The `CSI 1 ; m X` sequence or its short form for cursor keys and F1–F4.
fn cursor_sequence(final_byte: char, press: &KeyPress, ss3_when_plain: bool) -> Vec<u8> {
    let modifier = modifier_param(press);
    if modifier == 1 {
        return if ss3_when_plain { format!("\x1bO{final_byte}") } else { format!("\x1b[{final_byte}") }.into_bytes();
    }
    format!("\x1b[1;{modifier}{final_byte}").into_bytes()
}

/// The `CSI n ~` sequence (with a modifier: `CSI n ; m ~`).
fn tilde_sequence(number: u8, press: &KeyPress) -> Vec<u8> {
    let modifier = modifier_param(press);
    if modifier == 1 {
        return format!("\x1b[{number}~").into_bytes();
    }
    format!("\x1b[{number};{modifier}~").into_bytes()
}

fn with_alt_prefix(press: &KeyPress, mut bytes: Vec<u8>) -> Vec<u8> {
    if press.alt {
        bytes.insert(0, 0x1b);
    }
    bytes
}

/// The bytes sent to the PTY for a key, or `None` if the key produces ordinary text
/// (which arrives through the text input/IME path) or means nothing to the terminal.
///
/// `key` uses GPUI naming: lowercase letters (`"a"`), names (`"enter"`, `"pageup"`, `"f5"`), or symbols.
pub fn key_to_bytes(press: &KeyPress, modes: KeyModes, is_mac: bool) -> Option<Vec<u8>> {
    // Cmd (macOS) and Super have no meaning for the shell.
    if press.platform {
        return None;
    }
    let key = press.key.to_lowercase();
    let plain = !press.shift && !press.alt && !press.control;
    let bytes = match key.as_str() {
        "enter" => with_alt_prefix(press, if modes.line_feed_new_line { b"\r\n".to_vec() } else { b"\r".to_vec() }),
        "tab" if press.shift => b"\x1b[Z".to_vec(),
        "tab" => with_alt_prefix(press, b"\t".to_vec()),
        "backspace" => with_alt_prefix(press, if press.control { vec![0x08] } else { vec![0x7f] }),
        "escape" => with_alt_prefix(press, vec![0x1b]),
        "up" => cursor_sequence('A', press, modes.app_cursor),
        "down" => cursor_sequence('B', press, modes.app_cursor),
        "right" => cursor_sequence('C', press, modes.app_cursor),
        "left" => cursor_sequence('D', press, modes.app_cursor),
        "home" => cursor_sequence('H', press, modes.app_cursor),
        "end" => cursor_sequence('F', press, modes.app_cursor),
        "insert" => tilde_sequence(2, press),
        "delete" => tilde_sequence(3, press),
        "pageup" => tilde_sequence(5, press),
        "pagedown" => tilde_sequence(6, press),
        "f1" => cursor_sequence('P', press, true),
        "f2" => cursor_sequence('Q', press, true),
        "f3" => cursor_sequence('R', press, true),
        "f4" => cursor_sequence('S', press, true),
        "f5" => tilde_sequence(15, press),
        "f6" => tilde_sequence(17, press),
        "f7" => tilde_sequence(18, press),
        "f8" => tilde_sequence(19, press),
        "f9" => tilde_sequence(20, press),
        "f10" => tilde_sequence(21, press),
        "f11" => tilde_sequence(23, press),
        "f12" => tilde_sequence(24, press),
        "f13" => tilde_sequence(25, press),
        "f14" => tilde_sequence(26, press),
        "f15" => tilde_sequence(28, press),
        "f16" => tilde_sequence(29, press),
        "f17" => tilde_sequence(31, press),
        "f18" => tilde_sequence(32, press),
        "f19" => tilde_sequence(33, press),
        "f20" => tilde_sequence(34, press),
        _ if plain => return None,
        _ if press.control => with_alt_prefix(press, vec![control_code(&key)?]),
        // Alt+<character> = ESC followed by the character. On macOS Option composes special characters,
        // so it is left to the text input path.
        _ if press.alt && !is_mac => {
            let text = if key == "space" {
                " ".to_string()
            } else if press.key.chars().count() == 1 {
                if press.shift {
                    press.key.to_uppercase()
                } else {
                    press.key.clone()
                }
            } else {
                return None;
            };
            let mut bytes = vec![0x1b];
            bytes.extend_from_slice(text.as_bytes());
            bytes
        }
        // Shift+<character> produces ordinary text.
        _ => return None,
    };
    Some(bytes)
}

/// Prepares pasted text to be sent to the PTY.
///
/// In bracketed paste mode the text is wrapped in `ESC [ 200 ~` … `ESC [ 201 ~`, and ESC and ETX
/// are dropped so the pasted content cannot close its own wrapper. Without that mode, line endings
/// are turned into CR like typed input.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        let clean: String = text.chars().filter(|c| !matches!(c, '\x1b' | '\x03')).collect();
        let mut bytes = b"\x1b[200~".to_vec();
        bytes.extend_from_slice(clean.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
        bytes
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake key press from a notation like "Ctrl+Shift+c" (the last part is the GPUI-style `key`).
    fn key(spec: &str) -> KeyPress {
        let mut parts: Vec<&str> = spec.split('+').collect();
        let key = parts.pop().unwrap();
        KeyPress {
            control: parts.contains(&"Ctrl"),
            alt: parts.contains(&"Alt"),
            shift: parts.contains(&"Shift"),
            platform: parts.contains(&"Meta"),
            key: key.to_string(),
        }
    }

    // The default keybindings of the commands that stay in effect in the terminal.
    fn app_bindings() -> HashSet<String> {
        ["Mod+`", "Mod+Shift+`", "Mod+J", "Mod+Shift+P", "F1", "Mod+P", "Mod+B"].into_iter().map(String::from).collect()
    }

    fn linux(spec: &str) -> KeyAction {
        classify_key(&key(spec), false, &app_bindings())
    }

    fn mac(spec: &str) -> KeyAction {
        classify_key(&key(spec), true, &app_bindings())
    }

    #[test]
    fn binding_maps_ctrl_to_mod_on_linux_and_cmd_to_mod_on_macos() {
        assert_eq!(binding_of(&key("Ctrl+p"), false).as_deref(), Some("Mod+P"));
        assert_eq!(binding_of(&key("Meta+p"), true).as_deref(), Some("Mod+P"));
        assert_eq!(binding_of(&key("Ctrl+p"), true).as_deref(), Some("Ctrl+P"));
        // Same as the framework: Super on Linux/Windows is written `Meta`, not `Mod`.
        assert_eq!(binding_of(&key("Meta+p"), false).as_deref(), Some("Meta+P"));
    }

    #[test]
    fn binding_matches_the_framework_format() {
        for (spec, mac) in [("Ctrl+Shift+c", false), ("Ctrl+`", false), ("Ctrl+Alt+s", false), ("Ctrl+pagedown", false), ("f1", false), ("escape", false), ("Meta+s", true)] {
            let press = key(spec);
            if mac == oncode_ui::keys::IS_MAC {
                assert_eq!(binding_of(&press, mac), oncode_ui::keys::keybinding_from_press(&press), "{spec}");
            }
        }
        assert_eq!(binding_of(&key("Ctrl+Shift+c"), false).as_deref(), Some("Mod+Shift+C"));
        // Shifted symbols are normalized: Windows sends Ctrl+Shift+` as Ctrl + "~".
        assert_eq!(binding_of(&key("Ctrl+~"), false).as_deref(), Some("Mod+Shift+`"));
        assert_eq!(binding_of(&key("Ctrl+Alt+s"), false).as_deref(), Some("Mod+Alt+S"));
    }

    #[test]
    fn binding_is_none_for_modifier_only_presses() {
        assert_eq!(binding_of(&KeyPress { control: true, key: "control".into(), ..Default::default() }, false), None);
        assert_eq!(binding_of(&KeyPress { shift: true, key: "shift".into(), ..Default::default() }, true), None);
    }

    #[test]
    fn linux_plain_ctrl_c_reaches_the_shell_and_ctrl_shift_c_v_copy_and_paste() {
        assert_eq!(linux("Ctrl+c"), KeyAction::Shell);
        assert_eq!(linux("Ctrl+v"), KeyAction::Shell);
        assert_eq!(linux("Ctrl+Shift+c"), KeyAction::Copy);
        assert_eq!(linux("Ctrl+Shift+v"), KeyAction::Paste);
    }

    #[test]
    fn linux_allowed_app_keybindings_are_left_to_the_app() {
        for spec in ["Ctrl+`", "Ctrl+~", "Ctrl+j", "Ctrl+Shift+p", "Ctrl+p", "Ctrl+b", "f1"] {
            assert_eq!(linux(spec), KeyAction::App, "{spec}");
        }
    }

    #[test]
    fn linux_readline_and_ordinary_keys_go_to_the_shell() {
        for spec in ["Ctrl+w", "Ctrl+s", "Ctrl+o", "Ctrl+g", "Ctrl+r", "Ctrl+d", "Ctrl+l", "Ctrl+a", "Alt+b", "a", "Shift+a", "enter", "tab", "escape", "f12", "f2", "up"] {
            assert_eq!(linux(spec), KeyAction::Shell, "{spec}");
        }
    }

    #[test]
    fn linux_ctrl_f_opens_find_and_super_is_not_mod() {
        assert_eq!(linux("Ctrl+f"), KeyAction::Find);
        assert_eq!(linux("Meta+c"), KeyAction::Shell);
        assert_eq!(linux("Meta+p"), KeyAction::Shell);
    }

    #[test]
    fn modifier_only_presses_always_belong_to_the_shell() {
        let press = KeyPress { control: true, key: "control".into(), ..Default::default() };
        assert_eq!(classify_key(&press, false, &app_bindings()), KeyAction::Shell);
    }

    #[test]
    fn macos_cmd_c_v_copy_and_paste_while_ctrl_c_stays_sigint() {
        assert_eq!(mac("Meta+c"), KeyAction::Copy);
        assert_eq!(mac("Meta+v"), KeyAction::Paste);
        assert_eq!(mac("Meta+Shift+c"), KeyAction::Copy);
        assert_eq!(mac("Meta+Shift+v"), KeyAction::Paste);
        assert_eq!(mac("Ctrl+c"), KeyAction::Shell);
        assert_eq!(mac("Ctrl+v"), KeyAction::Shell);
    }

    #[test]
    fn macos_app_keybindings_use_cmd_and_ctrl_letters_go_to_the_shell() {
        for spec in ["Meta+`", "Meta+j", "Meta+Shift+p", "Meta+p", "Meta+b"] {
            assert_eq!(mac(spec), KeyAction::App, "{spec}");
        }
        for spec in ["Ctrl+p", "Ctrl+b", "Ctrl+j", "Ctrl+w", "Ctrl+r", "Ctrl+`"] {
            assert_eq!(mac(spec), KeyAction::Shell, "{spec}");
        }
    }

    #[test]
    fn macos_other_cmd_combinations_belong_to_the_app() {
        assert_eq!(mac("Meta+w"), KeyAction::App);
        assert_eq!(mac("Meta+s"), KeyAction::App);
        assert_eq!(mac("Meta+q"), KeyAction::App);
        assert_eq!(mac("Meta+f"), KeyAction::Find);
    }

    #[test]
    fn macos_ordinary_and_option_keys_go_to_the_shell() {
        for spec in ["a", "enter", "Alt+b", "f12"] {
            assert_eq!(mac(spec), KeyAction::Shell, "{spec}");
        }
    }

    fn bytes(spec: &str) -> Option<String> {
        key_to_bytes(&key(spec), KeyModes::default(), false).map(|b| String::from_utf8(b).unwrap())
    }

    fn app_cursor(spec: &str) -> Option<String> {
        key_to_bytes(&key(spec), KeyModes { app_cursor: true, ..Default::default() }, false).map(|b| String::from_utf8(b).unwrap())
    }

    #[test]
    fn scroll_keys_match_vs_code() {
        assert_eq!(scroll_key(&key("Shift+pageup")), Some(ScrollKey::PageUp));
        assert_eq!(scroll_key(&key("Shift+pagedown")), Some(ScrollKey::PageDown));
        assert_eq!(scroll_key(&key("Ctrl+Shift+up")), Some(ScrollKey::LineUp));
        assert_eq!(scroll_key(&key("Ctrl+Shift+down")), Some(ScrollKey::LineDown));
        assert_eq!(scroll_key(&key("Ctrl+Shift+home")), Some(ScrollKey::Top));
        assert_eq!(scroll_key(&key("Ctrl+Shift+end")), Some(ScrollKey::Bottom));
        // Without Shift they still belong to the shell (PageUp for command history, etc.).
        for spec in ["pageup", "Ctrl+up", "up", "Shift+up", "Alt+Shift+pageup"] {
            assert_eq!(scroll_key(&key(spec)), None, "{spec}");
        }
    }

    #[test]
    fn printable_keys_are_left_to_text_input() {
        for spec in ["a", "Shift+a", "1", "/", "space", "Shift+space", "é"] {
            assert_eq!(bytes(spec), None, "{spec}");
        }
    }

    #[test]
    fn editing_keys() {
        assert_eq!(bytes("enter").as_deref(), Some("\r"));
        assert_eq!(bytes("Shift+enter").as_deref(), Some("\r"));
        assert_eq!(bytes("Alt+enter").as_deref(), Some("\x1b\r"));
        assert_eq!(bytes("tab").as_deref(), Some("\t"));
        assert_eq!(bytes("Shift+tab").as_deref(), Some("\x1b[Z"));
        assert_eq!(bytes("backspace").as_deref(), Some("\x7f"));
        assert_eq!(bytes("Ctrl+backspace").as_deref(), Some("\x08"));
        assert_eq!(bytes("Alt+backspace").as_deref(), Some("\x1b\x7f"));
        assert_eq!(bytes("escape").as_deref(), Some("\x1b"));
        assert_eq!(bytes("delete").as_deref(), Some("\x1b[3~"));
        assert_eq!(bytes("insert").as_deref(), Some("\x1b[2~"));
        let lnm = KeyModes { line_feed_new_line: true, ..Default::default() };
        assert_eq!(key_to_bytes(&key("enter"), lnm, false).as_deref(), Some(&b"\r\n"[..]));
    }

    #[test]
    fn arrows_home_end_follow_application_cursor_mode() {
        assert_eq!(bytes("up").as_deref(), Some("\x1b[A"));
        assert_eq!(bytes("down").as_deref(), Some("\x1b[B"));
        assert_eq!(bytes("right").as_deref(), Some("\x1b[C"));
        assert_eq!(bytes("left").as_deref(), Some("\x1b[D"));
        assert_eq!(bytes("home").as_deref(), Some("\x1b[H"));
        assert_eq!(bytes("end").as_deref(), Some("\x1b[F"));
        assert_eq!(app_cursor("up").as_deref(), Some("\x1bOA"));
        assert_eq!(app_cursor("left").as_deref(), Some("\x1bOD"));
        assert_eq!(app_cursor("home").as_deref(), Some("\x1bOH"));
        assert_eq!(app_cursor("end").as_deref(), Some("\x1bOF"));
        // With a modifier it is always the CSI form, also in application mode.
        assert_eq!(app_cursor("Ctrl+left").as_deref(), Some("\x1b[1;5D"));
    }

    #[test]
    fn modified_cursor_and_paging_keys_carry_the_xterm_modifier_parameter() {
        assert_eq!(bytes("Shift+up").as_deref(), Some("\x1b[1;2A"));
        assert_eq!(bytes("Alt+up").as_deref(), Some("\x1b[1;3A"));
        assert_eq!(bytes("Ctrl+right").as_deref(), Some("\x1b[1;5C"));
        assert_eq!(bytes("Ctrl+Shift+left").as_deref(), Some("\x1b[1;6D"));
        assert_eq!(bytes("Ctrl+Alt+Shift+end").as_deref(), Some("\x1b[1;8F"));
        assert_eq!(bytes("pageup").as_deref(), Some("\x1b[5~"));
        assert_eq!(bytes("pagedown").as_deref(), Some("\x1b[6~"));
        assert_eq!(bytes("Ctrl+pageup").as_deref(), Some("\x1b[5;5~"));
        assert_eq!(bytes("Ctrl+delete").as_deref(), Some("\x1b[3;5~"));
    }

    #[test]
    fn function_keys() {
        assert_eq!(bytes("f1").as_deref(), Some("\x1bOP"));
        assert_eq!(bytes("f4").as_deref(), Some("\x1bOS"));
        assert_eq!(bytes("f5").as_deref(), Some("\x1b[15~"));
        assert_eq!(bytes("f6").as_deref(), Some("\x1b[17~"));
        assert_eq!(bytes("f10").as_deref(), Some("\x1b[21~"));
        assert_eq!(bytes("f11").as_deref(), Some("\x1b[23~"));
        assert_eq!(bytes("f12").as_deref(), Some("\x1b[24~"));
        assert_eq!(bytes("Shift+f1").as_deref(), Some("\x1b[1;2P"));
        assert_eq!(bytes("Ctrl+f5").as_deref(), Some("\x1b[15;5~"));
        assert_eq!(bytes("F2").as_deref(), Some("\x1bOQ"), "key names are case-insensitive");
    }

    #[test]
    fn ctrl_combinations_send_control_codes() {
        assert_eq!(bytes("Ctrl+a").as_deref(), Some("\x01"));
        assert_eq!(bytes("Ctrl+c").as_deref(), Some("\x03"));
        assert_eq!(bytes("Ctrl+d").as_deref(), Some("\x04"));
        assert_eq!(bytes("Ctrl+z").as_deref(), Some("\x1a"));
        assert_eq!(bytes("Ctrl+Shift+a").as_deref(), Some("\x01"));
        assert_eq!(bytes("Ctrl+[").as_deref(), Some("\x1b"));
        assert_eq!(bytes("Ctrl+\\").as_deref(), Some("\x1c"));
        assert_eq!(bytes("Ctrl+]").as_deref(), Some("\x1d"));
        assert_eq!(bytes("Ctrl+^").as_deref(), Some("\x1e"));
        assert_eq!(bytes("Ctrl+_").as_deref(), Some("\x1f"));
        assert_eq!(bytes("Ctrl+/").as_deref(), Some("\x1f"));
        assert_eq!(bytes("Ctrl+space").as_deref(), Some("\0"));
        assert_eq!(bytes("Ctrl+@").as_deref(), Some("\0"));
        assert_eq!(bytes("Ctrl+?").as_deref(), Some("\x7f"));
        assert_eq!(bytes("Ctrl+Alt+c").as_deref(), Some("\x1b\x03"));
        assert_eq!(bytes("Ctrl+1"), None);
    }

    #[test]
    fn alt_combinations_prefix_escape_except_on_macos() {
        assert_eq!(bytes("Alt+b").as_deref(), Some("\x1bb"));
        assert_eq!(bytes("Alt+Shift+b").as_deref(), Some("\x1bB"));
        assert_eq!(bytes("Alt+.").as_deref(), Some("\x1b."));
        assert_eq!(bytes("Alt+space").as_deref(), Some("\x1b "));
        assert_eq!(key_to_bytes(&key("Alt+b"), KeyModes::default(), true), None, "Option composes characters on macOS");
        // Named keys are still translated on macOS.
        assert_eq!(key_to_bytes(&key("Alt+left"), KeyModes::default(), true).as_deref(), Some(&b"\x1b[1;3D"[..]));
    }

    #[test]
    fn platform_modifier_never_reaches_the_shell() {
        assert_eq!(bytes("Meta+c"), None);
        assert_eq!(bytes("Meta+up"), None);
    }

    #[test]
    fn paste_is_bracketed_only_when_the_program_asked_for_it() {
        assert_eq!(paste_bytes("ls\npwd\r\n", false), b"ls\rpwd\r");
        assert_eq!(paste_bytes("ls\npwd", true), b"\x1b[200~ls\npwd\x1b[201~");
        // The pasted content cannot close its wrapper or send Ctrl+C.
        assert_eq!(paste_bytes("a\x1b[201~b\x03c", true), b"\x1b[200~a[201~bc\x1b[201~");
    }
}
