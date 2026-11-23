//! Laporan mouse untuk program TUI (protokol X10/normal, UTF-8, dan SGR). Logika murni.

/// Tombol atau gerakan yang dilaporkan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Press(ReportButton),
    Release(ReportButton),
    /// Gerakan; `Some` bila sebuah tombol sedang ditekan.
    Motion(Option<ReportButton>),
    WheelUp,
    WheelDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportButton {
    Left,
    Middle,
    Right,
}

impl ReportButton {
    fn code(self) -> u8 {
        match self {
            ReportButton::Left => 0,
            ReportButton::Middle => 1,
            ReportButton::Right => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReportModifiers {
    pub shift: bool,
    pub alt: bool,
    pub control: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEncoding {
    /// `CSI M Cb Cx Cy`, satu byte per koordinat (maksimum kolom/baris 223).
    Normal,
    /// Seperti `Normal`, tetapi koordinat besar ditulis sebagai UTF-8.
    Utf8,
    /// `CSI < Cb ; Cx ; Cy M|m`.
    Sgr,
}

/// Urutan byte laporan mouse untuk sel (`column`, `line`) berbasis nol, atau `None` bila
/// posisinya tidak bisa dinyatakan dalam pengodean itu.
pub fn mouse_report(action: MouseAction, column: usize, line: usize, modifiers: ReportModifiers, encoding: MouseEncoding) -> Option<Vec<u8>> {
    let sgr = encoding == MouseEncoding::Sgr;
    let (mut code, release) = match action {
        MouseAction::Press(button) => (button.code(), false),
        // Protokol lama tidak menyebut tombol mana yang dilepas.
        MouseAction::Release(button) => (if sgr { button.code() } else { 3 }, true),
        MouseAction::Motion(button) => (32 + button.map_or(3, ReportButton::code), false),
        MouseAction::WheelUp => (64, false),
        MouseAction::WheelDown => (65, false),
    };
    if modifiers.shift {
        code += 4;
    }
    if modifiers.alt {
        code += 8;
    }
    if modifiers.control {
        code += 16;
    }
    let (x, y) = (column + 1, line + 1);
    match encoding {
        MouseEncoding::Sgr => Some(format!("\x1b[<{code};{x};{y}{}", if release { 'm' } else { 'M' }).into_bytes()),
        MouseEncoding::Normal => {
            if x > 223 || y > 223 {
                return None;
            }
            Some(vec![0x1b, b'[', b'M', 32 + code, 32 + x as u8, 32 + y as u8])
        }
        MouseEncoding::Utf8 => {
            if x > 2015 || y > 2015 {
                return None;
            }
            let mut bytes = vec![0x1b, b'[', b'M', 32 + code];
            for value in [x, y] {
                let ch = char::from_u32(32 + value as u32)?;
                let mut buffer = [0u8; 4];
                bytes.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
            }
            Some(bytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: ReportModifiers = ReportModifiers { shift: false, alt: false, control: false };

    fn sgr(action: MouseAction, column: usize, line: usize) -> String {
        String::from_utf8(mouse_report(action, column, line, NONE, MouseEncoding::Sgr).unwrap()).unwrap()
    }

    #[test]
    fn sgr_press_release_and_wheel() {
        assert_eq!(sgr(MouseAction::Press(ReportButton::Left), 0, 0), "\x1b[<0;1;1M");
        assert_eq!(sgr(MouseAction::Release(ReportButton::Left), 9, 4), "\x1b[<0;10;5m");
        assert_eq!(sgr(MouseAction::Press(ReportButton::Right), 300, 50), "\x1b[<2;301;51M");
        assert_eq!(sgr(MouseAction::WheelUp, 2, 3), "\x1b[<64;3;4M");
        assert_eq!(sgr(MouseAction::WheelDown, 2, 3), "\x1b[<65;3;4M");
        assert_eq!(sgr(MouseAction::Motion(Some(ReportButton::Left)), 1, 1), "\x1b[<32;2;2M");
        assert_eq!(sgr(MouseAction::Motion(None), 1, 1), "\x1b[<35;2;2M");
    }

    #[test]
    fn modifiers_are_added_to_the_button_code() {
        let mods = ReportModifiers { shift: true, alt: true, control: true };
        let bytes = mouse_report(MouseAction::Press(ReportButton::Middle), 0, 0, mods, MouseEncoding::Sgr).unwrap();
        assert_eq!(String::from_utf8(bytes).unwrap(), "\x1b[<29;1;1M");
    }

    #[test]
    fn normal_encoding_uses_single_bytes_and_a_generic_release() {
        assert_eq!(mouse_report(MouseAction::Press(ReportButton::Left), 0, 0, NONE, MouseEncoding::Normal).unwrap(), b"\x1b[M !!");
        assert_eq!(mouse_report(MouseAction::Release(ReportButton::Right), 1, 2, NONE, MouseEncoding::Normal).unwrap(), b"\x1b[M#\"#");
        assert_eq!(mouse_report(MouseAction::Press(ReportButton::Left), 222, 0, NONE, MouseEncoding::Normal).unwrap()[4], 255);
        assert_eq!(mouse_report(MouseAction::Press(ReportButton::Left), 223, 0, NONE, MouseEncoding::Normal), None);
    }

    #[test]
    fn utf8_encoding_extends_the_coordinate_range() {
        let bytes = mouse_report(MouseAction::Press(ReportButton::Left), 299, 0, NONE, MouseEncoding::Utf8).unwrap();
        assert_eq!(&bytes[..4], b"\x1b[M ");
        assert_eq!(std::str::from_utf8(&bytes[4..]).unwrap().chars().map(|c| c as u32).collect::<Vec<_>>(), [332, 33]);
        assert_eq!(mouse_report(MouseAction::Press(ReportButton::Left), 3000, 0, NONE, MouseEncoding::Utf8), None);
    }
}
