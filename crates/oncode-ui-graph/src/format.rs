//! Format tanggal dan hash untuk daftar commit.

use oncode_git::Timestamp;

/// Detik Unix saat ini.
pub fn now_seconds() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// Waktu relatif ringkas dari detik Unix, mis. "5m ago", "3d ago", "2y ago".
pub fn relative_date_at(seconds: Timestamp, now: f64) -> String {
    let diff = (now - seconds).floor();
    if !diff.is_finite() {
        return String::new();
    }
    let diff = diff as i64;
    if diff < 60 {
        return "just now".into();
    }
    if diff < 3600 {
        return format!("{}m ago", diff / 60);
    }
    if diff < 86400 {
        return format!("{}h ago", diff / 3600);
    }
    let days = diff / 86400;
    if days < 30 {
        return format!("{days}d ago");
    }
    if days < 365 {
        return format!("{}mo ago", days / 30);
    }
    format!("{}y ago", days / 365)
}

pub fn relative_date(seconds: Timestamp) -> String {
    relative_date_at(seconds, now_seconds())
}

/// Tanggal dan jam lengkap dalam zona waktu setempat, mis. "Nov 17, 2026, 2:05 PM".
pub fn full_date(seconds: Timestamp) -> String {
    use chrono::TimeZone as _;
    if !seconds.is_finite() {
        return String::new();
    }
    match chrono::Local.timestamp_opt(seconds as i64, 0).single() {
        Some(time) => time.format("%b %-d, %Y, %-I:%M %p").to_string(),
        None => String::new(),
    }
}

pub fn short_oid(oid: &str) -> &str {
    oid.get(..8).unwrap_or(oid)
}

/// Nama revisi untuk judul dialog: hash penuh dipendekkan, nama ref dibiarkan.
pub fn short_rev(rev: &str) -> &str {
    if rev.len() >= 40 {
        short_oid(rev)
    } else {
        rev
    }
}

pub fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// Nama terakhir sebuah path (`/` atau `\`).
pub fn base_name(path: &str) -> &str {
    path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: f64 = 60.0;
    const HOUR: f64 = 3600.0;
    const DAY: f64 = 86400.0;

    #[test]
    fn relative_date_formats_each_range() {
        let now = 1_795_000_000.0;
        let ago = |seconds: f64| relative_date_at(now - seconds, now);
        let cases = [
            (0.0, "just now"),
            (59.0, "just now"),
            (MIN, "1m ago"),
            (59.0 * MIN + 59.0, "59m ago"),
            (HOUR, "1h ago"),
            (23.0 * HOUR + 59.0 * MIN, "23h ago"),
            (DAY, "1d ago"),
            (29.0 * DAY, "29d ago"),
            (30.0 * DAY, "1mo ago"),
            (364.0 * DAY, "12mo ago"),
            (365.0 * DAY, "1y ago"),
            (800.0 * DAY, "2y ago"),
        ];
        for (seconds, expected) in cases {
            assert_eq!(ago(seconds), expected, "{seconds} detik lalu");
        }
    }

    #[test]
    fn future_timestamps_are_just_now_and_nan_is_empty() {
        let now = 1_795_000_000.0;
        assert_eq!(relative_date_at(now + 500.0, now), "just now");
        assert_eq!(relative_date_at(f64::NAN, now), "");
        assert_eq!(full_date(f64::NAN), "");
    }

    #[test]
    fn full_date_has_month_year_and_time() {
        let text = full_date(1_700_000_000.0);
        assert!(text.contains("2023"), "{text}");
        assert!(text.ends_with("AM") || text.ends_with("PM"), "{text}");
    }

    #[test]
    fn short_oid_and_plural() {
        assert_eq!(short_oid("0123456789abcdef0123456789abcdef01234567"), "01234567");
        assert_eq!(short_oid("abc"), "abc");
        assert_eq!(short_rev("0123456789abcdef0123456789abcdef01234567"), "01234567");
        assert_eq!(short_rev("feature/login"), "feature/login");
        assert_eq!(plural(1, "commit"), "1 commit");
        assert_eq!(plural(0, "commit"), "0 commits");
        assert_eq!(plural(3, "changed file"), "3 changed files");
    }

    #[test]
    fn base_name_handles_both_separators() {
        assert_eq!(base_name("/work/oncode/"), "oncode");
        assert_eq!(base_name("C:\\work\\oncode"), "oncode");
        assert_eq!(base_name("a.txt"), "a.txt");
    }
}
