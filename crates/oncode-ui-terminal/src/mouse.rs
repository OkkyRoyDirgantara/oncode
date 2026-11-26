//! Mouse reporting for TUI programs (the X10/normal, UTF-8, and SGR protocols). Pure logic.

/// The button or motion being reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Press(ReportButton),
    Release(ReportButton),
    /// Motion; `Some` if a button is being held down.
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
    /// `CSI M Cb Cx Cy`, one byte per coordinate (maximum column/line 223).
    Normal,
    /// Like `Normal`, but large coordinates are written as UTF-8.
    Utf8,
    /// `CSI < Cb ; Cx ; Cy M|m`.
    Sgr,
}

/// Where the mouse wheel scroll is forwarded to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelTarget {
    /// The terminal's history (scrollback) is scrolled.
    History,
    /// A full-screen program without mouse reporting (less, man): the wheel becomes up/down arrows.
    Arrows,
    /// The program asked for mouse reporting (vim `mouse=a`, htop, lazygit): the wheel is reported to the program.
    Report,
}

/// Decides where the wheel goes. Shift+wheel always scrolls the terminal history (a way out of a
/// program that captures the mouse), except on the alternate screen, which has no history.
pub fn wheel_target(mouse_mode: bool, alt_screen: bool, alternate_scroll: bool, shift: bool) -> WheelTarget {
    if mouse_mode && !shift {
        WheelTarget::Report
    } else if alt_screen && alternate_scroll && !mouse_mode {
        WheelTarget::Arrows
    } else {
        WheelTarget::History
    }
}

/// The scroll position (`display_offset`) from a point on the scrollbar: `fraction` 0 = top (the
/// oldest history), 1 = bottom (the newest output).
pub fn offset_at_fraction(fraction: f32, history: usize) -> usize {
    let fraction = if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { 1.0 };
    ((1.0 - fraction) * history as f32).round() as usize
}

/// The scrollbar thumb's placement as a fraction of the track height: (top, height).
pub fn scrollbar_thumb(rows: usize, history: usize, offset: usize) -> (f32, f32) {
    let total = (rows + history).max(1) as f32;
    let height = rows as f32 / total;
    let top = history.saturating_sub(offset.min(history)) as f32 / total;
    (top, height)
}

/// The mouse report byte sequence for the zero-based cell (`column`, `line`), or `None` if the
/// position cannot be expressed in that encoding.
pub fn mouse_report(action: MouseAction, column: usize, line: usize, modifiers: ReportModifiers, encoding: MouseEncoding) -> Option<Vec<u8>> {
    let sgr = encoding == MouseEncoding::Sgr;
    let (mut code, release) = match action {
        MouseAction::Press(button) => (button.code(), false),
        // The old protocol does not say which button was released.
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

    #[test]
    fn wheel_goes_to_the_program_history_or_arrows() {
        // A plain shell: the terminal history.
        assert_eq!(wheel_target(false, false, true, false), WheelTarget::History);
        // less/man: alternate screen without a mouse, the wheel becomes arrows.
        assert_eq!(wheel_target(false, true, true, false), WheelTarget::Arrows);
        // vim mouse=a, htop: reported to the program, also on the main screen.
        assert_eq!(wheel_target(true, true, true, false), WheelTarget::Report);
        assert_eq!(wheel_target(true, false, true, false), WheelTarget::Report);
        // Shift+wheel on the main screen: still the terminal history.
        assert_eq!(wheel_target(true, false, true, true), WheelTarget::History);
    }

    #[test]
    fn scrollbar_maps_between_offset_and_position() {
        assert_eq!(offset_at_fraction(0.0, 100), 100, "top = oldest history");
        assert_eq!(offset_at_fraction(1.0, 100), 0, "bottom = newest");
        assert_eq!(offset_at_fraction(0.5, 100), 50);
        assert_eq!(offset_at_fraction(-3.0, 100), 100);
        assert_eq!(offset_at_fraction(f32::NAN, 100), 0);
        // 20 screen lines, 80 history.
        assert_eq!(scrollbar_thumb(20, 80, 0), (0.8, 0.2));
        assert_eq!(scrollbar_thumb(20, 80, 80), (0.0, 0.2));
        assert_eq!(scrollbar_thumb(20, 0, 0), (0.0, 1.0));
    }

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
