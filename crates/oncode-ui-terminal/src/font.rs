//! Font terminal: memilih keluarga font yang terpasang dari pengaturan, dan menghitung ukuran sel.
//! Murni agar bisa diuji.

use oncode_core::Settings;

/// Tinggi baris bawaan, dalam persen ukuran font.
pub const DEFAULT_LINE_HEIGHT: u32 = 120;

/// Nama-nama di daftar font (dipisah koma, boleh berkutip), tanpa nama generik.
fn names(list: &str) -> impl Iterator<Item = &str> {
    list.split(',')
        .map(|name| name.trim().trim_matches(['"', '\'']).trim())
        .filter(|name| !name.is_empty() && !matches!(name.to_ascii_lowercase().as_str(), "monospace" | "sans-serif" | "serif" | "system-ui"))
}

/// Font pertama di daftar yang terpasang, dengan penulisan nama seperti yang terpasang.
pub fn pick_family(list: &str, installed: &[String]) -> Option<String> {
    names(list).find_map(|name| installed.iter().find(|font| font.eq_ignore_ascii_case(name)).cloned())
}

/// Keluarga font terminal: `terminal.fontFamily`, lalu daftar font editor. `None` berarti tidak
/// ada yang terpasang; pemanggil memakai font monospace tema.
pub fn terminal_family(settings: &Settings, installed: &[String]) -> Option<String> {
    settings.terminal.font_family.as_deref().and_then(|list| pick_family(list, installed)).or_else(|| pick_family(&settings.editor.font_family, installed))
}

pub fn font_size(settings: &Settings) -> f32 {
    settings.terminal.font_size.unwrap_or(settings.editor.font_size).clamp(8, 40) as f32
}

/// Tinggi baris sebagai kelipatan ukuran font.
pub fn line_height_factor(settings: &Settings) -> f32 {
    settings.terminal.line_height.unwrap_or(DEFAULT_LINE_HEIGHT).clamp(100, 200) as f32 / 100.
}

/// Lebar dan tinggi sel, dibulatkan ke piksel perangkat: latar sel-sel bersebelahan lalu
/// bertemu tepat di batas piksel, tanpa garis tipis di antaranya.
pub fn cell_size(advance: f32, font_size: f32, factor: f32, scale: f32) -> (f32, f32) {
    let snap = |value: f32| ((value * scale).round() / scale).max(1. / scale);
    (snap(advance), snap(font_size * factor))
}

/// Ketebalan garis tipis (box drawing, garis bawah, kursor), minimal satu piksel perangkat.
pub fn line_thickness(font_size: f32, scale: f32) -> f32 {
    (font_size * 0.07 * scale).round().max(1.) / scale
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed() -> Vec<String> {
        ["Menlo", "JetBrains Mono", "Helvetica"].map(String::from).to_vec()
    }

    #[test]
    fn picks_the_first_installed_font_ignoring_quotes_case_and_generics() {
        assert_eq!(pick_family("\"Fira Code\", 'jetbrains mono', Menlo", &installed()).as_deref(), Some("JetBrains Mono"));
        assert_eq!(pick_family("monospace, Menlo", &installed()).as_deref(), Some("Menlo"));
        assert_eq!(pick_family("Fira Code, monospace", &installed()), None);
        assert_eq!(pick_family("", &installed()), None);
    }

    #[test]
    fn terminal_font_falls_back_to_the_editor_list_when_not_installed() {
        let mut settings = Settings::default();
        settings.editor.font_family = "\"Fira Code\", Menlo, monospace".into();
        assert_eq!(terminal_family(&settings, &installed()).as_deref(), Some("Menlo"));
        settings.terminal.font_family = Some("JetBrains Mono".into());
        assert_eq!(terminal_family(&settings, &installed()).as_deref(), Some("JetBrains Mono"));
        // Font terminal yang tidak terpasang tidak membuat terminal jatuh ke font proporsional.
        settings.terminal.font_family = Some("Not Installed".into());
        assert_eq!(terminal_family(&settings, &installed()).as_deref(), Some("Menlo"));
    }

    #[test]
    fn size_and_line_height_follow_settings_within_limits() {
        let mut settings = Settings::default();
        settings.editor.font_size = 14;
        assert_eq!(font_size(&settings), 14.);
        settings.terminal.font_size = Some(100);
        assert_eq!(font_size(&settings), 40.);
        assert_eq!(line_height_factor(&settings), 1.2);
        settings.terminal.line_height = Some(150);
        assert_eq!(line_height_factor(&settings), 1.5);
        settings.terminal.line_height = Some(50);
        assert_eq!(line_height_factor(&settings), 1.);
    }

    #[test]
    fn cells_snap_to_device_pixels() {
        assert_eq!(cell_size(8.43, 14., 1.2, 1.), (8., 17.));
        assert_eq!(cell_size(8.43, 14., 1.2, 2.), (8.5, 17.));
        assert_eq!(line_thickness(14., 1.), 1.);
        assert_eq!(line_thickness(14., 2.), 1.);
        assert_eq!(line_thickness(28., 2.), 2.);
    }
}
