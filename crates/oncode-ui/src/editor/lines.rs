//! Operasi baris editor yang murni (tanpa GPUI): memindahkan baris ke atas atau ke bawah.

use std::ops::Range;

/// Hasil memindahkan baris: bagian teks yang diganti, penggantinya, dan pilihan sesudahnya.
/// Semua offset dalam byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMove {
    pub replace: Range<usize>,
    pub text: String,
    pub selection: Range<usize>,
}

/// Memindahkan baris-baris yang tercakup `selection` satu baris ke atas (`up`) atau ke bawah.
/// Pilihan ikut berpindah bersama barisnya. `None` bila sudah di baris pertama/terakhir.
///
/// Pilihan yang berakhir tepat di awal sebuah baris tidak mengikutkan baris itu, sama seperti
/// VS Code (memilih satu baris penuh dengan Shift+↓ tidak ikut memindahkan baris berikutnya).
pub fn move_lines(text: &str, selection: Range<usize>, up: bool) -> Option<LineMove> {
    let selection = selection.start.min(text.len())..selection.end.min(text.len());
    let starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(index, _)| index + 1)).collect();
    let line_of = |offset: usize| starts.partition_point(|&start| start <= offset) - 1;
    // Akhir baris tanpa karakter `\n`-nya.
    let line_end = |line: usize| if line + 1 < starts.len() { starts[line + 1] - 1 } else { text.len() };

    let first = line_of(selection.start);
    let mut last = line_of(selection.end);
    if selection.end > selection.start && last > first && starts[last] == selection.end {
        last -= 1;
    }
    let block = &text[starts[first]..line_end(last)];
    if up {
        let above = first.checked_sub(1)?;
        let neighbour = &text[starts[above]..line_end(above)];
        let shift = neighbour.len() + 1;
        Some(LineMove {
            replace: starts[above]..line_end(last),
            text: format!("{block}\n{neighbour}"),
            selection: selection.start - shift..selection.end - shift,
        })
    } else {
        let below = last + 1;
        if below >= starts.len() {
            return None;
        }
        let neighbour = &text[starts[below]..line_end(below)];
        let shift = neighbour.len() + 1;
        Some(LineMove {
            replace: starts[first]..line_end(below),
            text: format!("{neighbour}\n{block}"),
            selection: selection.start + shift..selection.end + shift,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Menerapkan hasil pemindahan ke teks.
    fn apply(text: &str, change: &LineMove) -> String {
        format!("{}{}{}", &text[..change.replace.start], change.text, &text[change.replace.end..])
    }

    #[test]
    fn moves_the_cursor_line_up_and_down_with_the_cursor() {
        let text = "satu\ndua\ntiga\n";
        // Kursor di "dua", kolom 1.
        let up = move_lines(text, 6..6, true).unwrap();
        assert_eq!(apply(text, &up), "dua\nsatu\ntiga\n");
        assert_eq!(up.selection, 1..1, "kursor tetap di kolom yang sama di baris barunya");

        let down = move_lines(text, 6..6, false).unwrap();
        assert_eq!(apply(text, &down), "satu\ntiga\ndua\n");
        assert_eq!(&apply(text, &down)[down.selection.start - 1..down.selection.start + 2], "dua");
    }

    #[test]
    fn stops_at_the_first_and_last_line() {
        assert_eq!(move_lines("a\nb", 0..0, true), None);
        assert_eq!(move_lines("a\nb", 3..3, false), None);
        assert_eq!(move_lines("", 0..0, false), None);
    }

    #[test]
    fn moves_every_selected_line_as_one_block() {
        let text = "a\nb\nc\nd";
        // Pilihan dari tengah "b" sampai tengah "c".
        let change = move_lines(text, 2..5, false).unwrap();
        assert_eq!(apply(text, &change), "a\nd\nb\nc");
        assert_eq!(&apply(text, &change)[change.selection.clone()], "b\nc");

        let change = move_lines(text, 2..5, true).unwrap();
        assert_eq!(apply(text, &change), "b\nc\na\nd");
    }

    #[test]
    fn a_selection_ending_at_a_line_start_leaves_that_line() {
        let text = "a\nb\nc";
        // Baris "b" dipilih penuh termasuk `\n`-nya (berakhir di awal "c").
        let change = move_lines(text, 2..4, true).unwrap();
        assert_eq!(apply(text, &change), "b\na\nc");
    }

    #[test]
    fn last_line_without_newline_and_multibyte_text() {
        let text = "é1\nü2";
        let change = move_lines(text, 5..5, true).unwrap();
        assert_eq!(apply(text, &change), "ü2\né1");
        let change = move_lines(text, 0..0, false).unwrap();
        assert_eq!(apply(text, &change), "ü2\né1");
        assert!(apply(text, &change).is_char_boundary(change.selection.start));
    }
}
