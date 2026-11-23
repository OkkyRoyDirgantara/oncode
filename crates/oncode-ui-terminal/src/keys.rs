//! Tombol terminal: (1) penyaring yang menentukan tombol mana yang diteruskan ke shell, mana yang
//! ditangani terminal sendiri (salin, tempel, cari), dan mana yang dibiarkan untuk aplikasi;
//! (2) penerjemahan tombol menjadi urutan byte ala xterm; (3) pembungkus tempelan.
//! Murni (tanpa GPUI) agar bisa diuji untuk kedua platform.

use std::collections::HashSet;

use oncode_ui::keys::{canonical_keybinding, KeyPress};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Shell,
    App,
    Copy,
    Paste,
    Find,
}

/// Command aplikasi yang tetap berlaku saat terminal fokus. Keybinding-nya diambil dari registry
/// (termasuk penimpaan pengguna); tombol lain yang terikat ke command di luar daftar ini
/// (mis. Ctrl+W, Ctrl+S, Ctrl+O) diteruskan ke shell.
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
    "terminal.kill",
    "terminal.clear",
    "terminal.focusNext",
    "terminal.focus",
];

/// Keybinding kanonis sebuah penekanan, dengan format yang sama seperti
/// `oncode_ui::keys::keybinding_from_press`, tetapi platformnya diberikan dari luar.
/// `None` bila hanya modifier.
pub fn binding_of(press: &KeyPress, is_mac: bool) -> Option<String> {
    let lower = press.key.to_lowercase();
    if press.key.is_empty() || matches!(lower.as_str(), "shift" | "control" | "alt" | "platform" | "function" | "cmd" | "super") {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    let (primary, secondary_ctrl, meta) = if is_mac { (press.platform, press.control, false) } else { (press.control, false, press.platform) };
    if primary {
        parts.push("Mod".into());
    }
    if secondary_ctrl {
        parts.push("Ctrl".into());
    }
    if meta {
        parts.push("Meta".into());
    }
    if press.alt {
        parts.push("Alt".into());
    }
    if press.shift {
        parts.push("Shift".into());
    }
    parts.push(if press.key == "+" { "+".into() } else { canonical_keybinding(&press.key) });
    Some(parts.join("+"))
}

/// - `Copy` / `Paste`: Mod+Shift+C / Mod+Shift+V; di macOS juga Cmd+C / Cmd+V.
///   Ctrl+C polos selalu sampai ke shell sebagai SIGINT.
/// - `Find`: Mod+F membuka bilah cari terminal.
/// - `App`: keybinding aplikasi yang diizinkan; di macOS semua kombinasi Cmd lain juga milik aplikasi
///   karena Cmd tidak punya arti bagi shell.
/// - `Shell`: sisanya.
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

/// Mode terminal yang memengaruhi urutan byte tombol.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyModes {
    /// DECCKM: tombol panah mengirim `ESC O A` alih-alih `ESC [ A`.
    pub app_cursor: bool,
    /// LNM: Enter mengirim CR LF.
    pub line_feed_new_line: bool,
}

/// Parameter modifier xterm: 1 + Shift(1) + Alt(2) + Ctrl(4).
fn modifier_param(press: &KeyPress) -> u8 {
    1 + u8::from(press.shift) + 2 * u8::from(press.alt) + 4 * u8::from(press.control)
}

/// Kode kontrol untuk Ctrl+<tombol>, mengikuti xterm.
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

/// Urutan `CSI 1 ; m X` atau bentuk pendeknya untuk tombol kursor dan F1–F4.
fn cursor_sequence(final_byte: char, press: &KeyPress, ss3_when_plain: bool) -> Vec<u8> {
    let modifier = modifier_param(press);
    if modifier == 1 {
        return if ss3_when_plain { format!("\x1bO{final_byte}") } else { format!("\x1b[{final_byte}") }.into_bytes();
    }
    format!("\x1b[1;{modifier}{final_byte}").into_bytes()
}

/// Urutan `CSI n ~` (dengan modifier: `CSI n ; m ~`).
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

/// Byte yang dikirim ke PTY untuk sebuah tombol, atau `None` bila tombol itu menghasilkan teks biasa
/// (yang datang lewat jalur masukan teks/IME) atau tidak berarti bagi terminal.
///
/// `key` memakai penamaan GPUI: huruf kecil (`"a"`), nama (`"enter"`, `"pageup"`, `"f5"`), atau simbol.
pub fn key_to_bytes(press: &KeyPress, modes: KeyModes, is_mac: bool) -> Option<Vec<u8>> {
    // Cmd (macOS) dan Super tidak punya arti bagi shell.
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
        // Alt+<karakter> = ESC lalu karakternya. Di macOS Option menyusun karakter khusus,
        // jadi dibiarkan lewat jalur masukan teks.
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
        // Shift+<karakter> menghasilkan teks biasa.
        _ => return None,
    };
    Some(bytes)
}

/// Menyiapkan teks tempelan untuk dikirim ke PTY.
///
/// Dalam mode bracketed paste teksnya dibungkus `ESC [ 200 ~` … `ESC [ 201 ~`, dan ESC serta ETX
/// dibuang agar isi tempelan tidak bisa menutup bungkusnya sendiri. Tanpa mode itu, akhir baris
/// dijadikan CR seperti ketikan.
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

    /// Penekanan tiruan dari notasi seperti "Ctrl+Shift+c" (bagian terakhir adalah `key` ala GPUI).
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

    // Keybinding bawaan command yang tetap berlaku di terminal.
    fn app_bindings() -> HashSet<String> {
        ["Mod+`", "Mod+~", "Mod+J", "Mod+Shift+P", "F1", "Mod+P", "Mod+B"].into_iter().map(String::from).collect()
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
        // Sama dengan kerangka: Super di Linux/Windows ditulis `Meta`, bukan `Mod`.
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
        assert_eq!(binding_of(&key("Ctrl+~"), false).as_deref(), Some("Mod+~"));
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
        // Dengan modifier selalu bentuk CSI, juga dalam mode aplikasi.
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
        assert_eq!(bytes("F2").as_deref(), Some("\x1bOQ"), "nama tombol tidak peka huruf besar");
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
        assert_eq!(key_to_bytes(&key("Alt+b"), KeyModes::default(), true), None, "Option menyusun karakter di macOS");
        // Tombol bernama tetap diterjemahkan di macOS.
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
        // Isi tempelan tidak bisa menutup bungkusnya atau mengirim Ctrl+C.
        assert_eq!(paste_bytes("a\x1b[201~b\x03c", true), b"\x1b[200~a[201~bc\x1b[201~");
    }
}
