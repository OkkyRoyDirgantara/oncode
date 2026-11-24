//! Logika murni untuk penanda perubahan di gutter editor dan blame: jenis penanda per baris,
//! pencarian repositori sebuah berkas, dan format tanggal blame.

use std::collections::BTreeMap;
use std::ops::Range;

use oncode_git::BlameRange;

use crate::lines::LineDiff;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

/// Repositori yang memuat `path` (yang akarnya paling dalam) dan path relatifnya (pemisah `/`).
pub fn repo_for_path(path: &str, roots: &[String]) -> Option<(String, String)> {
    let normalized = path.replace('\\', "/");
    let clean = |root: &str| root.replace('\\', "/").trim_end_matches('/').to_string();
    let mut best: Option<(&String, String)> = None;
    for root in roots {
        let base = clean(root);
        let inside = normalized == base || normalized.strip_prefix(&base).is_some_and(|rest| rest.starts_with('/'));
        if inside && best.as_ref().is_none_or(|(_, current)| base.len() > current.len()) {
            best = Some((root, base));
        }
    }
    let (root, base) = best?;
    Some((root.clone(), normalized.get(base.len() + 1..).unwrap_or_default().to_string()))
}

/// Jenis penanda per baris (nomor baris mulai dari 1) pada isi saat ini (sisi B).
pub fn change_marks(diff: &LineDiff) -> BTreeMap<u32, ChangeKind> {
    let mut marks = BTreeMap::new();
    for chunk in &diff.chunks {
        if chunk.b_len() == 0 {
            // Penghapusan murni: ditandai pada baris yang kini menempati posisinya.
            let line = (chunk.b_from + 1).min(diff.b_lines.max(1));
            marks.entry(line).or_insert(ChangeKind::Deleted);
            continue;
        }
        let kind = if chunk.a_len() == 0 { ChangeKind::Added } else { ChangeKind::Modified };
        for line in chunk.b_from + 1..=chunk.b_to {
            marks.insert(line, kind);
        }
    }
    marks
}

/// Rentang byte untuk dekorasi gutter editor. Rentang tak kosong menandai semua baris yang
/// disentuhnya; rentang kosong menandai batas baris tempat baris yang dihapus tadinya berada.
pub fn gutter_ranges(diff: &LineDiff, text: &str) -> Vec<(Range<usize>, ChangeKind)> {
    if diff.chunks.is_empty() {
        return Vec::new();
    }
    // Offset awal tiap baris nyata, ditambah panjang teks sebagai penutup.
    let mut starts: Vec<usize> = Vec::with_capacity(diff.b_lines as usize + 1);
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        starts.push(offset);
        offset += line.len();
    }
    starts.push(text.len());
    let at = |line: u32| starts.get(line as usize).copied().unwrap_or(text.len());
    diff.chunks
        .iter()
        .map(|chunk| {
            if chunk.b_len() == 0 {
                (at(chunk.b_from)..at(chunk.b_from), ChangeKind::Deleted)
            } else {
                let kind = if chunk.a_len() == 0 { ChangeKind::Added } else { ChangeKind::Modified };
                (at(chunk.b_from)..at(chunk.b_to), kind)
            }
        })
        .collect()
}

/// Rentang blame yang memuat sebuah baris (nomor mulai dari 1).
pub fn blame_at(ranges: &[BlameRange], line: u32) -> Option<&BlameRange> {
    let index = ranges.partition_point(|range| range.start_line + range.line_count <= line);
    ranges.get(index).filter(|range| line >= range.start_line)
}

/// Tanggal relatif yang ringkas. `seconds` dan `now` adalah detik sejak epoch.
pub fn format_blame_date(seconds: f64, now: f64) -> String {
    let days = ((now - seconds) / 86_400.0).floor() as i64;
    if days < 1 {
        "today".into()
    } else if days < 2 {
        "yesterday".into()
    } else if days < 30 {
        format!("{days} days ago")
    } else if days < 365 {
        format!("{} mo ago", days / 30)
    } else {
        format!("{} yr ago", days / 365)
    }
}

/// Teks kolom blame untuk sebuah rentang.
pub fn blame_label(range: &BlameRange, now: f64) -> String {
    if range.uncommitted {
        "Not committed yet".into()
    } else {
        format!("{} · {}", range.author, format_blame_date(range.time, now))
    }
}

/// Umur commit untuk inline blame, dengan satuan sampai menit ("3 hours ago").
pub fn format_blame_age(seconds: f64, now: f64) -> String {
    let elapsed = (now - seconds).max(0.0);
    let ago = |count: i64, unit: &str| if count == 1 { format!("1 {unit} ago") } else { format!("{count} {unit}s ago") };
    let days = (elapsed / 86_400.0).floor() as i64;
    if elapsed < 60.0 {
        "just now".into()
    } else if elapsed < 3_600.0 {
        ago((elapsed / 60.0) as i64, "minute")
    } else if days < 1 {
        ago((elapsed / 3_600.0) as i64, "hour")
    } else if days < 30 {
        ago(days, "day")
    } else if days < 365 {
        ago(days / 30, "month")
    } else {
        ago(days / 365, "year")
    }
}

/// Panjang maksimal ringkasan commit di inline blame (karakter).
const INLINE_SUMMARY_CHARS: usize = 50;

/// Teks inline blame di ujung baris kursor, seperti GitLens:
/// "Ana, 3 days ago • Ringkasan". Penulis dengan email `me` ditulis "You".
pub fn inline_blame_text(range: &BlameRange, now: f64, me: Option<&str>) -> String {
    if range.uncommitted {
        return "You • Uncommitted changes".into();
    }
    let mine = me.is_some_and(|email| !email.is_empty() && email.eq_ignore_ascii_case(&range.author_email));
    let author = if mine { "You" } else { range.author.as_str() };
    let summary = range.summary.trim();
    let summary = if summary.chars().count() > INLINE_SUMMARY_CHARS {
        format!("{}…", summary.chars().take(INLINE_SUMMARY_CHARS - 1).collect::<String>().trim_end())
    } else {
        summary.to_string()
    };
    format!("{author}, {} • {summary}", format_blame_age(range.time, now))
}

/// Anotasi blame untuk baris kursor (status bar).
pub fn blame_summary(range: &BlameRange, now: f64) -> String {
    if range.uncommitted {
        "Not committed yet".into()
    } else {
        format!("{}, {} • {}", range.author, format_blame_date(range.time, now), range.summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(a: &str, b: &str) -> Vec<(u32, ChangeKind)> {
        change_marks(&LineDiff::compute(a, b)).into_iter().collect()
    }

    #[test]
    fn distinguishes_added_modified_and_deleted_lines() {
        use ChangeKind::*;
        assert_eq!(marks("1\n2\n3\n", "1\n2\n3\n"), []);
        assert_eq!(marks("1\n2\n3\n", "1\nDUA\n3\n"), [(2, Modified)]);
        assert_eq!(marks("1\n3\n", "1\n2a\n2b\n3\n"), [(2, Added), (3, Added)]);
        // Baris "2" dihapus: penanda di baris yang kini menempati posisinya.
        assert_eq!(marks("1\n2\n3\n", "1\n3\n"), [(2, Deleted)]);
    }

    #[test]
    fn marks_additions_at_the_end_of_the_file() {
        use ChangeKind::*;
        assert_eq!(marks("a\n", "a\nb\n"), [(2, Added)]);
        // Tanpa newline di akhir, baris terakhir lama ikut berubah (kini diakhiri newline).
        assert_eq!(marks("a", "a\nb"), [(1, Modified), (2, Modified)]);
        // Penghapusan di akhir berkas ditandai pada baris terakhir.
        assert_eq!(marks("a\nb\n", "a\n"), [(1, Deleted)]);
    }

    #[test]
    fn gutter_ranges_cover_whole_lines_and_deletion_boundaries() {
        use ChangeKind::*;
        let (a, b) = ("1\n2\n3\n4\n", "1\nDUA\n4\nbaru");
        let ranges = gutter_ranges(&LineDiff::compute(a, b), b);
        // "DUA" menggantikan "2" dan "3"; "baru" ditambahkan tanpa newline di akhir.
        assert_eq!(ranges, [(2..6, Modified), (8..12, Added)]);
        let (a, b) = ("1\n2\n3\n", "1\n3\n");
        assert_eq!(gutter_ranges(&LineDiff::compute(a, b), b), [(2..2, Deleted)]);
        let (a, b) = ("1\n2\n", "1\n");
        assert_eq!(gutter_ranges(&LineDiff::compute(a, b), b), [(2..2, Deleted)]);
        assert!(gutter_ranges(&LineDiff::compute(a, a), a).is_empty());
    }

    #[test]
    fn repo_for_path_picks_the_deepest_repository() {
        let roots = vec!["/p/app".to_string(), "/p/app/vendor/lib".to_string()];
        assert_eq!(repo_for_path("/p/app/src/main.rs", &roots), Some(("/p/app".into(), "src/main.rs".into())));
        assert_eq!(repo_for_path("/p/app/vendor/lib/x.c", &roots), Some(("/p/app/vendor/lib".into(), "x.c".into())));
        assert_eq!(repo_for_path("/p/app-lain/x", &roots), None);
        assert_eq!(repo_for_path("C:\\kode\\app\\src\\a.ts", &["C:\\kode\\app".to_string()]), Some(("C:\\kode\\app".into(), "src/a.ts".into())));
    }

    #[test]
    fn blame_dates_use_sensible_units() {
        let now = 1_795_000_000.0;
        let ago = |days: f64| now - days * 86_400.0;
        assert_eq!(format_blame_date(ago(0.2), now), "today");
        assert_eq!(format_blame_date(ago(1.5), now), "yesterday");
        assert_eq!(format_blame_date(ago(12.0), now), "12 days ago");
        assert_eq!(format_blame_date(ago(95.0), now), "3 mo ago");
        assert_eq!(format_blame_date(ago(800.0), now), "2 yr ago");
    }

    #[test]
    fn finds_the_blame_range_of_a_line() {
        let range = |start_line: u32, line_count: u32, author: &str| BlameRange {
            oid: "a".repeat(40),
            start_line,
            line_count,
            author: author.into(),
            author_email: String::new(),
            time: 0.0,
            summary: "Ringkasan".into(),
            uncommitted: false,
        };
        let ranges = vec![range(1, 2, "Ana"), range(3, 1, "Budi"), range(5, 3, "Ana")];
        assert_eq!(blame_at(&ranges, 2).map(|r| r.author.as_str()), Some("Ana"));
        assert_eq!(blame_at(&ranges, 3).map(|r| r.author.as_str()), Some("Budi"));
        assert!(blame_at(&ranges, 4).is_none());
        assert_eq!(blame_at(&ranges, 7).map(|r| r.start_line), Some(5));
        assert!(blame_at(&ranges, 8).is_none());
        assert_eq!(blame_label(&ranges[0], 86_400.0 * 3.0), "Ana · 3 days ago");
        assert_eq!(blame_summary(&ranges[1], 0.0), "Budi, today • Ringkasan");
        assert_eq!(blame_label(&BlameRange { uncommitted: true, ..ranges[0].clone() }, 0.0), "Not committed yet");
    }

    #[test]
    fn inline_blame_reads_like_gitlens() {
        let now = 1_795_000_000.0;
        let range = BlameRange {
            oid: "a".repeat(40),
            start_line: 1,
            line_count: 1,
            author: "Ana".into(),
            author_email: "ana@contoh.id".into(),
            time: now - 3.0 * 86_400.0,
            summary: "  fix: crash saat ctrl+klik  ".into(),
            uncommitted: false,
        };
        assert_eq!(inline_blame_text(&range, now, None), "Ana, 3 days ago • fix: crash saat ctrl+klik");
        assert_eq!(inline_blame_text(&range, now, Some("ANA@contoh.id")), "You, 3 days ago • fix: crash saat ctrl+klik");
        assert_eq!(inline_blame_text(&range, now, Some("")), "Ana, 3 days ago • fix: crash saat ctrl+klik");
        assert_eq!(inline_blame_text(&BlameRange { uncommitted: true, ..range.clone() }, now, None), "You • Uncommitted changes");
        let long = BlameRange { summary: "x".repeat(80), ..range };
        let text = inline_blame_text(&long, now, None);
        assert!(text.ends_with(&format!("{}…", "x".repeat(49))), "{text}");
    }

    #[test]
    fn blame_ages_go_down_to_minutes() {
        let now = 1_795_000_000.0;
        assert_eq!(format_blame_age(now - 20.0, now), "just now");
        assert_eq!(format_blame_age(now + 5.0, now), "just now");
        assert_eq!(format_blame_age(now - 60.0, now), "1 minute ago");
        assert_eq!(format_blame_age(now - 5.0 * 60.0, now), "5 minutes ago");
        assert_eq!(format_blame_age(now - 3.5 * 3_600.0, now), "3 hours ago");
        assert_eq!(format_blame_age(now - 1.2 * 86_400.0, now), "1 day ago");
        assert_eq!(format_blame_age(now - 95.0 * 86_400.0, now), "3 months ago");
        assert_eq!(format_blame_age(now - 800.0 * 86_400.0, now), "2 years ago");
    }
}
