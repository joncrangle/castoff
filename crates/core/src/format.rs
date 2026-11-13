//! Display formatting and filename sanitising.
//!
//! The rounding (JavaScript's `toFixed`) and the sanitising rules are fixed: file names already on
//! people's drives were written with them, and a sync matches episodes by those names.

use chrono::{Datelike, Local, TimeZone};

use crate::api::Drive;

const BYTES_UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];

/// JavaScript's `Number.prototype.toFixed(1)` for finite, non-negative values.
///
/// Rust's `{:.1}` rounds exact ties to even (`1.25` -> `1.2`) where JavaScript picks the larger
/// candidate (`1.3`). The only doubles that are exact ties at one decimal are odd multiples of
/// 0.25, so those are handled explicitly.
pub(crate) fn to_fixed1(value: f64) -> String {
    let quad = value * 4.0;
    if quad.fract() == 0.0 && (quad as u64) % 2 == 1 {
        let tenths = (value * 10.0).floor() as u64 + 1;
        return format!("{}.{}", tenths / 10, tenths % 10);
    }
    format!("{value:.1}")
}

/// "1.5 GB", "320.0 MB", "0 B". Bytes show no decimals, everything above shows one.
pub fn format_bytes(bytes: u64) -> String {
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut unit = 0;
    let mut value = bytes as f64;
    while value >= 1024.0 && unit < BYTES_UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        return format!("{} {}", value.round() as u64, BYTES_UNITS[unit]);
    }
    format!("{} {}", to_fixed1(value), BYTES_UNITS[unit])
}

/// Episode length: "MM:SS" below an hour, "HH:MM:SS" from an hour up.
/// Negative (and NaN) durations clamp to zero.
pub fn format_duration(secs: f64) -> String {
    let secs = if secs.is_nan() { 0.0 } else { secs.max(0.0) };
    let hours = (secs / 3600.0).floor() as u64;
    let minutes = ((secs % 3600.0) / 60.0).floor() as u64;
    let seconds = (secs % 60.0).floor() as u64;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

/// Transfer rate, as the transfer popup shows it: MiB/s with one decimal, e.g. "12.4 MB/s".
pub fn format_speed(bytes_per_sec: f64) -> String {
    let speed = if bytes_per_sec.is_finite() {
        bytes_per_sec.max(0.0)
    } else {
        0.0
    };
    format!("{} MB/s", to_fixed1(speed / 1024.0 / 1024.0))
}

/// Publication date from Unix seconds, in local time, as "YYYY-MM-DD". Zero (the "no date"
/// sentinel) and unrepresentable timestamps give "Unknown".
pub fn format_date(unix_secs: i64) -> String {
    if unix_secs == 0 {
        return "Unknown".to_string();
    }
    match Local.timestamp_opt(unix_secs, 0).single() {
        Some(dt) => format!("{}-{:02}-{:02}", dt.year(), dt.month(), dt.day()),
        None => "Unknown".to_string(),
    }
}

/// `format_date` for an optional timestamp (`None` is "unknown").
pub fn format_date_opt(unix_secs: Option<i64>) -> String {
    format_date(unix_secs.unwrap_or(0))
}

/// "WALKMAN (15.2 GB free)" or "WALKMAN (unknown free space)".
pub fn format_drive_info(drive: &Drive) -> String {
    let free = match drive.free_space {
        Some(bytes) => format!("{} free", format_bytes(bytes)),
        None => "unknown free space".to_string(),
    };
    format!("{} ({free})", drive.name)
}

/// Truncates to `max_len` characters, ending in "..." when something was cut.
pub fn truncate_string(s: &str, max_len: usize) -> String {
    if max_len == 0 {
        return String::new();
    }
    let len = s.chars().count();
    if len <= max_len {
        return s.to_string();
    }
    if max_len <= 3 {
        return ".".repeat(max_len);
    }
    let kept: String = s.chars().take(max_len - 3).collect();
    format!("{kept}...")
}

/// Progress fraction in 0..=1. Byte-based when `total_bytes` is known, else item-based.
pub fn calculate_progress(current: usize, total: usize, bytes: u64, total_bytes: u64) -> f64 {
    if total_bytes > 0 {
        return (bytes as f64 / total_bytes as f64).clamp(0.0, 1.0);
    }
    if total > 0 {
        return (current as f64 / total as f64).clamp(0.0, 1.0);
    }
    0.0
}

/// A text progress bar of `width` cells ("█" filled, "░" empty).
pub fn progress_bar(fraction: f64, width: usize) -> String {
    let p = fraction.clamp(0.0, 1.0);
    let filled = ((p * width as f64).round() as usize).min(width);
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

/// JavaScript's `\s` / `String.prototype.trim` whitespace set.
fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// Longest name kept, counted in UTF-16 code units, as the names already on drives were.
const MAX_NAME_UNITS: usize = 100;

/// Makes a show or episode name safe for FAT32/exFAT: `< > : " / \ | ? *` become `-`, `&` becomes
/// "and", apostrophes vanish, whitespace runs become `_`, and the result is capped at 100 UTF-16
/// units. Empty or all-whitespace input gives "untitled".
///
/// A result of exactly "." or ".." also gives "untitled", so a hostile show name
/// can't point the destination outside the Podcasts folder.
pub fn sanitize_filename(name: &str) -> String {
    if name.chars().all(is_js_whitespace) {
        return "untitled".to_string();
    }

    let mut out = String::with_capacity(name.len());
    let mut in_whitespace = false;
    for c in name.chars() {
        if is_js_whitespace(c) {
            if !in_whitespace {
                out.push('_');
                in_whitespace = true;
            }
            continue;
        }
        match c {
            // Removed apostrophes sit inside a whitespace run without ending it.
            '\'' => continue,
            '&' => out.push_str("and"),
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => out.push('-'),
            _ => out.push(c),
        }
        in_whitespace = false;
    }

    if out.is_empty() || out == "." || out == ".." {
        return "untitled".to_string();
    }

    let mut units = 0;
    let mut cut = out.len();
    for (idx, c) in out.char_indices() {
        units += c.len_utf16();
        if units > MAX_NAME_UNITS {
            cut = idx;
            break;
        }
    }
    out.truncate(cut);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ---- formatBytes ------------------------------------------------------------------------

    #[test]
    fn bytes_units() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1_048_576), "1.0 MB");
        assert_eq!(format_bytes(1_073_741_824), "1.0 GB");
        assert_eq!(format_bytes(1_099_511_627_776), "1.0 TB");
        assert_eq!(format_bytes(1_610_612_736), "1.5 GB");
        assert_eq!(format_bytes(0), "0 B");
    }

    #[test]
    fn bytes_15_2_gb() {
        assert_eq!(
            format_bytes((15.2 * 1024.0 * 1024.0 * 1024.0) as u64),
            "15.2 GB"
        );
    }

    #[test]
    fn bytes_rounds_ties_up_like_javascript() {
        // 1280 / 1024 = 1.25 exactly: JS toFixed(1) gives "1.3", Rust's {:.1} would give "1.2".
        assert_eq!(format_bytes(1280), "1.3 KB");
        assert_eq!(format_bytes(2304), "2.3 KB");
        assert_eq!(format_bytes(1792), "1.8 KB"); // 1.75
        // Non-ties are untouched: 1.0498 -> 1.0, 1.0518 -> 1.1.
        assert_eq!(format_bytes(1075), "1.0 KB");
        assert_eq!(format_bytes(1077), "1.1 KB");
    }

    #[test]
    fn bytes_largest_unit_caps_at_tb() {
        assert_eq!(format_bytes(2048 * 1_099_511_627_776), "2048.0 TB");
    }

    // ---- formatDuration ---------------------------------------------------------------------

    #[test]
    fn duration_formats() {
        assert_eq!(format_duration(65.0), "01:05");
        assert_eq!(format_duration(3665.0), "01:01:05");
        assert_eq!(format_duration(5.0), "00:05");
        assert_eq!(format_duration(3600.0), "01:00:00");
        assert_eq!(format_duration(0.0), "00:00");
        assert_eq!(format_duration(59.9), "00:59");
    }

    #[test]
    fn duration_clamps_negative() {
        assert_eq!(format_duration(-120.0), "00:00");
        assert_eq!(format_duration(f64::NAN), "00:00");
    }

    // ---- formatDate -------------------------------------------------------------------------

    fn local_noon(y: i32, m: u32, d: u32) -> i64 {
        Local
            .with_ymd_and_hms(y, m, d, 12, 0, 0)
            .unwrap()
            .timestamp()
    }

    #[test]
    fn date_formats_in_local_time() {
        assert_eq!(format_date(local_noon(2023, 12, 25)), "2023-12-25");
        assert_eq!(format_date(local_noon(2024, 1, 5)), "2024-01-05");
    }

    #[test]
    fn date_zero_is_unknown() {
        assert_eq!(format_date(0), "Unknown");
        assert_eq!(format_date_opt(None), "Unknown");
        assert_eq!(
            format_date_opt(Some(local_noon(2023, 12, 25))),
            "2023-12-25"
        );
    }

    #[test]
    fn date_out_of_range_is_unknown() {
        assert_eq!(format_date(i64::MAX), "Unknown");
    }

    // ---- speed / progress -------------------------------------------------------------------

    #[test]
    fn speed_is_mib_per_second() {
        assert_eq!(format_speed(0.0), "0.0 MB/s");
        assert_eq!(format_speed(1024.0 * 1024.0), "1.0 MB/s");
        assert_eq!(format_speed(12.4 * 1024.0 * 1024.0), "12.4 MB/s");
        assert_eq!(format_speed(f64::INFINITY), "0.0 MB/s");
        assert_eq!(format_speed(f64::NAN), "0.0 MB/s");
        assert_eq!(format_speed(-5.0), "0.0 MB/s");
    }

    #[test]
    fn progress_prefers_bytes_then_items() {
        assert_eq!(calculate_progress(1, 4, 50, 100), 0.5);
        assert_eq!(calculate_progress(1, 4, 0, 0), 0.25);
        assert_eq!(calculate_progress(0, 0, 0, 0), 0.0);
        assert_eq!(calculate_progress(0, 0, 500, 100), 1.0);
    }

    #[test]
    fn progress_bar_cells() {
        assert_eq!(progress_bar(0.0, 4), "░░░░");
        assert_eq!(progress_bar(0.5, 4), "██░░");
        assert_eq!(progress_bar(1.0, 4), "████");
        assert_eq!(progress_bar(2.0, 4), "████");
        assert_eq!(progress_bar(-1.0, 4), "░░░░");
    }

    // ---- sanitizeFilename -------------------------------------------------------------------

    #[test]
    fn sanitize_replaces_invalid_characters() {
        assert_eq!(
            sanitize_filename("Episode 1: The Beginning?"),
            "Episode_1-_The_Beginning-"
        );
        assert_eq!(sanitize_filename("file:name?test"), "file-name-test");
        assert_eq!(sanitize_filename("a<b>c\"d/e\\f|g*h"), "a-b-c-d-e-f-g-h");
    }

    #[test]
    fn sanitize_spaces_to_underscores() {
        assert_eq!(sanitize_filename("My Episode Name"), "My_Episode_Name");
        assert_eq!(sanitize_filename("a \t  b"), "a_b");
        assert_eq!(sanitize_filename(" a "), "_a_");
    }

    #[test]
    fn sanitize_truncates_to_100() {
        assert_eq!(sanitize_filename(&"A".repeat(150)).len(), 100);
    }

    #[test]
    fn sanitize_truncates_by_utf16_units_without_splitting_pairs() {
        // 99 ASCII + one emoji (2 units) would be 101 units: the emoji is dropped, not split.
        let name = format!("{}😀", "A".repeat(99));
        assert_eq!(sanitize_filename(&name), "A".repeat(99));
        let name = format!("{}😀", "A".repeat(98));
        assert_eq!(sanitize_filename(&name).chars().count(), 99);
    }

    #[test]
    fn sanitize_ampersand_and_apostrophe() {
        assert_eq!(sanitize_filename("Tom & Jerry"), "Tom_and_Jerry");
        assert_eq!(
            sanitize_filename("It's a wonderful life"),
            "Its_a_wonderful_life"
        );
        assert_eq!(
            sanitize_filename("Tom & Jerry's Show"),
            "Tom_and_Jerrys_Show"
        );
        // A removed apostrophe doesn't split a whitespace run.
        assert_eq!(sanitize_filename("a ' b"), "a_b");
    }

    #[test]
    fn sanitize_empty_and_whitespace() {
        assert_eq!(sanitize_filename(""), "untitled");
        assert_eq!(sanitize_filename("   "), "untitled");
        assert_eq!(sanitize_filename("\u{feff}\u{a0}"), "untitled");
        assert_eq!(sanitize_filename("'"), "untitled");
    }

    #[test]
    fn sanitize_refuses_dot_names() {
        assert_eq!(sanitize_filename("."), "untitled");
        assert_eq!(sanitize_filename(".."), "untitled");
        assert_eq!(sanitize_filename("..."), "...");
    }

    #[test]
    fn sanitize_keeps_other_punctuation() {
        assert_eq!(
            sanitize_filename("Episode: The *Best* One!"),
            "Episode-_The_-Best-_One!"
        );
        assert_eq!(
            sanitize_filename("Show: With Special/Characters?"),
            "Show-_With_Special-Characters-"
        );
    }

    // ---- misc -------------------------------------------------------------------------------

    #[test]
    fn truncate_string_cases() {
        assert_eq!(truncate_string("hello", 10), "hello");
        assert_eq!(truncate_string("hello world", 8), "hello...");
        assert_eq!(truncate_string("hello", 2), "..");
        assert_eq!(truncate_string("hello", 0), "");
    }

    fn drive(free: Option<u64>) -> Drive {
        Drive {
            id: "1".into(),
            name: "WALKMAN".into(),
            bsd_name: "disk2s1".into(),
            mount_point: PathBuf::from("/Volumes/WALKMAN"),
            total_space: 32 << 30,
            free_space: free,
        }
    }

    #[test]
    fn drive_info() {
        assert_eq!(
            format_drive_info(&drive(Some((15.2 * 1024.0 * 1024.0 * 1024.0) as u64))),
            "WALKMAN (15.2 GB free)"
        );
        assert_eq!(
            format_drive_info(&drive(Some(256 << 20))),
            "WALKMAN (256.0 MB free)"
        );
        assert_eq!(
            format_drive_info(&drive(None)),
            "WALKMAN (unknown free space)"
        );
    }
}
