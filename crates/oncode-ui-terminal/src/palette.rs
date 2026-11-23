//! Warna terminal: palet yang diturunkan dari tema aplikasi dan penerjemahan warna serta atribut
//! sel (16/256/truecolor, bold, dim, inverse, hidden) menjadi warna siap gambar.

use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use gpui_kit::component::Theme;
use gpui_kit::{Hsla, Rgba};

/// Palet dasar terminal untuk satu tema.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    /// Warna teks di dalam kursor blok.
    pub cursor_text: Rgb,
    /// Latar seleksi beserta alfanya.
    pub selection: (Rgb, f32),
    /// 16 warna ANSI: 0–7 normal, 8–15 terang.
    pub ansi: [Rgb; 16],
}

const fn rgb(value: u32) -> Rgb {
    Rgb { r: (value >> 16) as u8, g: (value >> 8) as u8, b: value as u8 }
}

pub fn from_hsla(color: Hsla) -> Rgb {
    let Rgba { r, g, b, .. } = color.to_rgb();
    let channel = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgb { r: channel(r), g: channel(g), b: channel(b) }
}

pub fn to_hsla(color: Rgb) -> Hsla {
    Hsla::from(Rgba { r: color.r as f32 / 255.0, g: color.g as f32 / 255.0, b: color.b as f32 / 255.0, a: 1.0 })
}

/// Campuran linear: `amount` 0 = `a`, 1 = `b`.
pub fn mix(a: Rgb, b: Rgb, amount: f32) -> Rgb {
    let channel = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * amount).round().clamp(0.0, 255.0) as u8;
    Rgb { r: channel(a.r, b.r), g: channel(a.g, b.g), b: channel(a.b, b.b) }
}

impl Palette {
    /// Palet bawaan versi 0.2 (tema gelap), dipakai bila tema tidak tersedia dan oleh uji.
    pub const fn fallback() -> Self {
        Self {
            foreground: rgb(0xd9dce3),
            background: rgb(0x1d1f27),
            cursor: rgb(0xd9dce3),
            cursor_text: rgb(0x1d1f27),
            selection: (rgb(0x4f9cff), 0.28),
            ansi: [
                rgb(0x17191f),
                rgb(0xf0616d),
                rgb(0x7cc98a),
                rgb(0xe3b556),
                rgb(0x4f9cff),
                rgb(0xc792ea),
                rgb(0x5fd0c5),
                rgb(0x9299ab),
                rgb(0x636a7d),
                rgb(0xf08a8a),
                rgb(0xa5d6a7),
                rgb(0xe3c48a),
                rgb(0x82b1ff),
                rgb(0xd58cf0),
                rgb(0x4fd0d0),
                rgb(0xd9dce3),
            ],
        }
    }

    /// Palet dari tema aplikasi; berlaku untuk tema gelap maupun terang.
    ///
    /// Seperti versi 0.2, "hitam" dan "putih" ANSI mengikuti tema (latar cekung dan teks redup),
    /// sehingga keduanya tetap terbaca di kedua tema.
    pub fn from_theme(theme: &Theme) -> Self {
        let background = from_hsla(theme.background);
        let foreground = from_hsla(theme.foreground);
        let muted = from_hsla(theme.muted_foreground);
        let selection_alpha = if theme.selection.a < 1.0 { theme.selection.a } else { 0.45 };
        Self {
            foreground,
            background,
            cursor: foreground,
            cursor_text: background,
            selection: (from_hsla(theme.selection.opacity(1.0)), selection_alpha),
            ansi: [
                from_hsla(theme.muted),
                from_hsla(theme.red),
                from_hsla(theme.green),
                from_hsla(theme.yellow),
                from_hsla(theme.blue),
                from_hsla(theme.magenta),
                from_hsla(theme.cyan),
                muted,
                mix(muted, background, 0.35),
                from_hsla(theme.red_light),
                from_hsla(theme.green_light),
                from_hsla(theme.yellow_light),
                from_hsla(theme.blue_light),
                from_hsla(theme.magenta_light),
                from_hsla(theme.cyan_light),
                foreground,
            ],
        }
    }

    /// Warna untuk indeks 0–255: 16 warna ANSI, kubus 6×6×6, lalu 24 tingkat abu-abu.
    pub fn indexed(&self, index: u8) -> Rgb {
        match index {
            0..=15 => self.ansi[index as usize],
            16..=231 => {
                let value = index - 16;
                let level = |n: u8| if n == 0 { 0 } else { 55 + n * 40 };
                Rgb { r: level(value / 36), g: level((value / 6) % 6), b: level(value % 6) }
            }
            232..=255 => {
                let gray = 8 + (index - 232) * 10;
                Rgb { r: gray, g: gray, b: gray }
            }
        }
    }

    fn named(&self, color: NamedColor) -> Rgb {
        match color {
            NamedColor::Foreground | NamedColor::BrightForeground => self.foreground,
            NamedColor::Background => self.background,
            NamedColor::Cursor => self.cursor,
            NamedColor::DimForeground => mix(self.foreground, self.background, 0.5),
            NamedColor::DimBlack
            | NamedColor::DimRed
            | NamedColor::DimGreen
            | NamedColor::DimYellow
            | NamedColor::DimBlue
            | NamedColor::DimMagenta
            | NamedColor::DimCyan
            | NamedColor::DimWhite => mix(self.ansi[color as usize - NamedColor::DimBlack as usize], self.background, 0.5),
            other => self.ansi[(other as usize).min(15)],
        }
    }

    /// Warna akhir untuk sebuah warna sel. `overrides` adalah warna yang diubah program lewat
    /// OSC 4/10/11 dan didahulukan atas palet.
    pub fn resolve(&self, color: Color, overrides: &Colors) -> Rgb {
        match color {
            Color::Spec(rgb) => rgb,
            Color::Named(named) => overrides[named].unwrap_or_else(|| self.named(named)),
            Color::Indexed(index) => overrides[index as usize].unwrap_or_else(|| self.indexed(index)),
        }
    }
}

/// Tampilan sebuah sel setelah warna dan atributnya diterjemahkan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellStyle {
    pub fg: Rgb,
    /// `None` bila sama dengan latar bawaan terminal (tidak perlu digambar).
    pub bg: Option<Rgb>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

/// Menerjemahkan warna dan atribut sebuah sel.
///
/// - Bold dengan warna ANSI 0–7 digambar dengan varian terangnya (bawaan xterm.js).
/// - Dim mencampur teks separuh ke arah latarnya.
/// - Inverse menukar teks dan latar; hidden menyamakan teks dengan latar.
pub fn cell_style(cell: &Cell, palette: &Palette, overrides: &Colors) -> CellStyle {
    let flags = cell.flags;
    let bold = flags.contains(Flags::BOLD);
    let fg_color = match cell.fg {
        Color::Named(named) if bold && (named as usize) < 8 => Color::Indexed(named as u8 + 8),
        Color::Indexed(index) if bold && index < 8 => Color::Indexed(index + 8),
        other => other,
    };
    let mut fg = palette.resolve(fg_color, overrides);
    let mut bg = palette.resolve(cell.bg, overrides);
    let default_bg = matches!(cell.bg, Color::Named(NamedColor::Background));
    let inverse = flags.contains(Flags::INVERSE);
    if inverse {
        std::mem::swap(&mut fg, &mut bg);
    }
    if flags.contains(Flags::DIM) {
        fg = mix(fg, bg, 0.5);
    }
    if flags.contains(Flags::HIDDEN) {
        fg = bg;
    }
    CellStyle {
        fg,
        bg: (inverse || !default_bg).then_some(bg),
        bold,
        italic: flags.contains(Flags::ITALIC),
        underline: flags.intersects(Flags::ALL_UNDERLINES),
        strikethrough: flags.contains(Flags::STRIKEOUT),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: Palette = Palette::fallback();

    fn cell(fg: Color, bg: Color, flags: Flags) -> Cell {
        Cell { c: 'x', fg, bg, flags, extra: None }
    }

    fn plain(fg: Color) -> CellStyle {
        cell_style(&cell(fg, Color::Named(NamedColor::Background), Flags::empty()), &P, &Colors::default())
    }

    #[test]
    fn ansi_colors_come_from_the_palette() {
        assert_eq!(plain(Color::Named(NamedColor::Red)).fg, P.ansi[1]);
        assert_eq!(plain(Color::Named(NamedColor::BrightCyan)).fg, P.ansi[14]);
        assert_eq!(plain(Color::Indexed(4)).fg, P.ansi[4]);
        assert_eq!(plain(Color::Named(NamedColor::Foreground)).fg, P.foreground);
        assert_eq!(plain(Color::Named(NamedColor::Foreground)).bg, None, "latar bawaan tidak digambar");
    }

    #[test]
    fn color_cube_and_grayscale_ramp_follow_xterm() {
        assert_eq!(P.indexed(16), rgb(0x000000));
        assert_eq!(P.indexed(21), rgb(0x0000ff));
        assert_eq!(P.indexed(196), rgb(0xff0000));
        assert_eq!(P.indexed(46), rgb(0x00ff00));
        assert_eq!(P.indexed(110), rgb(0x87afd7));
        assert_eq!(P.indexed(231), rgb(0xffffff));
        assert_eq!(P.indexed(232), rgb(0x080808));
        assert_eq!(P.indexed(244), rgb(0x808080));
        assert_eq!(P.indexed(255), rgb(0xeeeeee));
    }

    #[test]
    fn truecolor_passes_through() {
        let style = cell_style(&cell(Color::Spec(rgb(0x123456)), Color::Spec(rgb(0xabcdef)), Flags::empty()), &P, &Colors::default());
        assert_eq!(style.fg, rgb(0x123456));
        assert_eq!(style.bg, Some(rgb(0xabcdef)));
    }

    #[test]
    fn bold_brightens_the_first_eight_colors_only() {
        let bold = |fg| cell_style(&cell(fg, Color::Named(NamedColor::Background), Flags::BOLD), &P, &Colors::default());
        assert_eq!(bold(Color::Named(NamedColor::Red)).fg, P.ansi[9]);
        assert_eq!(bold(Color::Indexed(2)).fg, P.ansi[10]);
        assert_eq!(bold(Color::Indexed(9)).fg, P.ansi[9]);
        assert_eq!(bold(Color::Spec(rgb(0x102030))).fg, rgb(0x102030));
        assert_eq!(bold(Color::Named(NamedColor::Foreground)).fg, P.foreground);
        assert!(bold(Color::Indexed(2)).bold);
    }

    #[test]
    fn inverse_swaps_and_always_paints_the_background() {
        let style = cell_style(&cell(Color::Named(NamedColor::Green), Color::Named(NamedColor::Background), Flags::INVERSE), &P, &Colors::default());
        assert_eq!(style.fg, P.background);
        assert_eq!(style.bg, Some(P.ansi[2]));
    }

    #[test]
    fn dim_blends_toward_the_background_and_hidden_matches_it() {
        let dim = cell_style(&cell(Color::Spec(rgb(0xffffff)), Color::Spec(rgb(0x000000)), Flags::DIM), &P, &Colors::default());
        assert_eq!(dim.fg, rgb(0x808080));
        let hidden = cell_style(&cell(Color::Named(NamedColor::Red), Color::Named(NamedColor::Background), Flags::HIDDEN), &P, &Colors::default());
        assert_eq!(hidden.fg, P.background);
    }

    #[test]
    fn attributes_are_reported() {
        let style = cell_style(
            &cell(Color::Named(NamedColor::Foreground), Color::Named(NamedColor::Background), Flags::ITALIC | Flags::UNDERCURL | Flags::STRIKEOUT),
            &P,
            &Colors::default(),
        );
        assert!(style.italic && style.underline && style.strikethrough && !style.bold);
    }

    #[test]
    fn program_overrides_win_over_the_palette() {
        let mut overrides = Colors::default();
        overrides[1] = Some(rgb(0x010203));
        overrides[NamedColor::Background] = Some(rgb(0x0a0b0c));
        assert_eq!(P.resolve(Color::Named(NamedColor::Red), &overrides), rgb(0x010203));
        assert_eq!(P.resolve(Color::Indexed(1), &overrides), rgb(0x010203));
        assert_eq!(P.resolve(Color::Named(NamedColor::Background), &overrides), rgb(0x0a0b0c));
        assert_eq!(P.resolve(Color::Named(NamedColor::Blue), &overrides), P.ansi[4]);
    }

    #[test]
    fn dim_named_colors_are_derived() {
        assert_eq!(P.resolve(Color::Named(NamedColor::DimRed), &Colors::default()), mix(P.ansi[1], P.background, 0.5));
        assert_eq!(P.resolve(Color::Named(NamedColor::BrightForeground), &Colors::default()), P.foreground);
    }

    #[test]
    fn hsla_round_trip_is_stable() {
        for color in [rgb(0x000000), rgb(0xffffff), rgb(0x4f9cff), rgb(0x7cc98a)] {
            let back = from_hsla(to_hsla(color));
            let close = |a: u8, b: u8| a.abs_diff(b) <= 1;
            assert!(close(back.r, color.r) && close(back.g, color.g) && close(back.b, color.b), "{color:?} -> {back:?}");
        }
    }
}
