//! Scanning a drive's `Podcasts` folder.
//!
//! Layout on the drive: `<mount>/Podcasts/<Show_Name>/<YYYY-MM-DD> - <Episode_Title>.<ext>`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Days, Local, NaiveDate, TimeZone};

use crate::api::Episode;
use crate::fsutil::{is_audio_file, is_system_hidden_file};
use crate::matching::{DriveIndex, IndexEntry, build_expected_drive_path};

/// The folder on the drive that holds the shows.
pub const PODCASTS_DIR: &str = "Podcasts";

const UNKNOWN_SHOW: &str = "Unknown Show";
/// Guards against symlink cycles; real drives are two or three levels deep.
const MAX_DEPTH: usize = 32;

/// What a file's path under `Podcasts/` says about it.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedFile {
    /// The first path component (the show folder), or "Unknown Show" for a file in the root.
    pub show_name: String,
    /// The file name without extension and without the leading "YYYY-MM-DD - " when it has one.
    pub title: String,
    pub date: Option<NaiveDate>,
    /// The file name without its extension, date prefix included.
    pub raw_title: String,
}

/// A date prefix like "2024-01-15 - ". Months 1-12 and days 1-31 are accepted; a day past the end
/// of its month rolls into the next one, as JavaScript's `new Date("2024-02-30")` does.
fn parse_date_prefix(raw_title: &str) -> Option<(NaiveDate, &str)> {
    let bytes = raw_title.as_bytes();
    if bytes.len() < 14 || &bytes[10..13] != b" - " {
        return None;
    }
    let shape_ok = bytes[..10].iter().enumerate().all(|(i, b)| match i {
        4 | 7 => *b == b'-',
        _ => b.is_ascii_digit(),
    });
    if !shape_ok {
        return None;
    }
    let rest = &raw_title[13..];
    if rest.is_empty() || rest.contains('\n') {
        return None;
    }
    let year: i32 = raw_title[0..4].parse().ok()?;
    let month: u32 = raw_title[5..7].parse().ok()?;
    let day: u32 = raw_title[8..10].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let date =
        NaiveDate::from_ymd_opt(year, month, 1)?.checked_add_days(Days::new(u64::from(day - 1)))?;
    Some((date, rest))
}

/// Parses a path relative to the `Podcasts` folder (`Show_Name/2024-01-15 - Title.mp3`).
pub fn parse_podcast_file(relative_path: &str) -> Option<ParsedFile> {
    let normalized = relative_path.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').collect();
    let filename = *parts.last()?;
    if filename.is_empty() {
        return None;
    }
    let show_name = if parts.len() > 1 {
        parts[0].to_string()
    } else {
        UNKNOWN_SHOW.to_string()
    };

    let dot = filename.rfind('.').filter(|&i| i > 0)?;
    let raw_title = &filename[..dot];

    let (title, date) = match parse_date_prefix(raw_title) {
        Some((date, title)) => (title.to_string(), Some(date)),
        None => (raw_title.to_string(), None),
    };
    Some(ParsedFile {
        show_name,
        title,
        date,
        raw_title: raw_title.to_string(),
    })
}

/// Every file below `dir` as a `/`-separated path relative to it, in name order. Unreadable
/// directories are skipped.
fn list_files_recursive(dir: &Path, relative: &str, depth: usize, out: &mut Vec<String>) {
    let Ok(read) = fs::read_dir(dir) else { return };
    let mut names: Vec<String> = read
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    for name in names {
        let full = dir.join(&name);
        let rel = if relative.is_empty() {
            name.clone()
        } else {
            format!("{relative}/{name}")
        };
        if full.is_dir() {
            if depth < MAX_DEPTH {
                list_files_recursive(&full, &rel, depth + 1, out);
            }
        } else {
            out.push(rel);
        }
    }
}

/// An audio file under `Podcasts/`, with everything the drive scan and the index need from it.
#[derive(Clone, Debug)]
pub struct DriveFile {
    pub parsed: ParsedFile,
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

fn collect_drive_files(drive_root: &Path) -> Vec<DriveFile> {
    let podcasts_dir = drive_root.join(PODCASTS_DIR);
    if !podcasts_dir.exists() {
        return Vec::new();
    }
    let mut files = Vec::new();
    list_files_recursive(&podcasts_dir, "", 0, &mut files);

    files
        .into_iter()
        .filter_map(|rel| {
            let filename = rel.rsplit('/').next().unwrap_or(&rel);
            if is_system_hidden_file(filename) || !is_audio_file(filename) {
                return None;
            }
            let parsed = parse_podcast_file(&rel)?;
            let path = podcasts_dir.join(&rel);
            let meta = fs::metadata(&path).ok();
            Some(DriveFile {
                parsed,
                size: meta.as_ref().map_or(0, |m| m.len()),
                modified: meta.and_then(|m| m.modified().ok()),
                path,
            })
        })
        .collect()
}

/// Whether the drive has a `Podcasts` folder at its root.
pub fn has_podcasts_folder(drive_root: &Path) -> bool {
    drive_root.join(PODCASTS_DIR).exists()
}

/// The drive's index for matching: expected `Show/Title.mp3` key to file. Empty when there is no
/// `Podcasts` folder.
pub fn build_drive_index(drive_root: &Path) -> DriveIndex {
    collect_drive_files(drive_root)
        .into_iter()
        .map(|f| {
            let key = build_expected_drive_path(&f.parsed.show_name, &f.parsed.title);
            (
                key,
                IndexEntry {
                    path: f.path,
                    size: Some(f.size),
                    duration: None,
                },
            )
        })
        .collect()
}

/// Local noon on `date` as Unix seconds. Noon (not midnight) so the local date is the same date
/// in every timezone and across DST changes; UTC midnight would show as the previous day anywhere
/// west of Greenwich.
fn local_noon(date: NaiveDate) -> Option<i64> {
    let noon = date.and_hms_opt(12, 0, 0)?;
    Local
        .from_local_datetime(&noon)
        .earliest()
        .map(|dt| dt.timestamp())
}

fn unix_secs(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

/// Episodes on the drive, grouped by show in order of first appearance (like `scanDrive` +
/// `mapPodcastsToEpisodes`). Show and episode names have their `_` turned back into spaces.
///
/// `published` is the date in the file name, else the file's modification time, else now.
/// `id` is the file path, which is unique where `Show-Title` is not.
pub fn scan_drive_episodes(drive_root: &Path) -> Vec<Episode> {
    let mut shows: Vec<String> = Vec::new();
    let mut by_show: HashMap<String, Vec<Episode>> = HashMap::new();

    for file in collect_drive_files(drive_root) {
        let show_dir = file.parsed.show_name.clone();
        let published = file
            .parsed
            .date
            .and_then(local_noon)
            .or_else(|| file.modified.map(unix_secs))
            .unwrap_or_else(|| unix_secs(SystemTime::now()));

        let episode = Episode {
            id: file.path.to_string_lossy().into_owned(),
            title: file.parsed.title.replace('_', " "),
            show_name: show_dir.replace('_', " "),
            duration_secs: 0.0,
            published: Some(published),
            on_drive: true,
            file_path: file.path,
            file_size: file.size,
            selected: false,
        };
        if !by_show.contains_key(&show_dir) {
            shows.push(show_dir.clone());
        }
        by_show.entry(show_dir).or_default().push(episode);
    }

    shows
        .into_iter()
        .flat_map(|show| by_show.remove(&show).unwrap_or_default())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::format_date;
    use tempfile::TempDir;

    fn write(path: &Path, bytes: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![0u8; bytes]).unwrap();
    }

    /// A drive with two dated episodes, a hidden file, a non-audio file, and a file with
    /// an invalid date prefix.
    fn drive_fixture() -> TempDir {
        let usb = TempDir::new().unwrap();
        let show = usb.path().join("Podcasts/Tech_Talk");
        write(&show.join("2024-01-15 - Episode_1.mp3"), 100);
        write(&show.join("2024-01-22 - Episode_2.mp3"), 200);
        write(&show.join(".DS_Store"), 0);
        write(&show.join("notes.txt"), 0);
        write(&show.join("InvalidDate - Episode_3.mp3"), 300);
        usb
    }

    #[test]
    fn parses_dated_file_names() {
        let p = parse_podcast_file("Tech_Talk/2024-01-15 - Episode_1.mp3").unwrap();
        assert_eq!(p.show_name, "Tech_Talk");
        assert_eq!(p.title, "Episode_1");
        assert_eq!(p.raw_title, "2024-01-15 - Episode_1");
        assert_eq!(p.date, NaiveDate::from_ymd_opt(2024, 1, 15));
    }

    #[test]
    fn undated_names_keep_the_whole_stem() {
        let p = parse_podcast_file("Tech_Talk/InvalidDate - Episode_3.mp3").unwrap();
        assert_eq!(p.title, "InvalidDate - Episode_3");
        assert_eq!(p.date, None);
        // Wrong shapes: no spaces around the dash, 2-digit year, impossible month/day.
        for name in [
            "2024-01-15-Episode.mp3",
            "24-01-15 - Episode.mp3",
            "2024-13-01 - Episode.mp3",
            "2024-01-00 - Episode.mp3",
            "2024-01-15 - .mp3",
        ] {
            let p = parse_podcast_file(&format!("Show/{name}")).unwrap();
            assert_eq!(p.date, None, "{name}");
            assert_eq!(p.title, &name[..name.len() - 4], "{name}");
        }
    }

    #[test]
    fn day_past_month_end_rolls_over() {
        let p = parse_podcast_file("Show/2024-02-30 - Ep.mp3").unwrap();
        assert_eq!(p.date, NaiveDate::from_ymd_opt(2024, 3, 1));
        assert_eq!(p.title, "Ep");
    }

    #[test]
    fn root_level_files_belong_to_unknown_show() {
        let p = parse_podcast_file("2024-01-15 - Loose.mp3").unwrap();
        assert_eq!(p.show_name, "Unknown Show");
        assert_eq!(p.title, "Loose");
    }

    #[test]
    fn extension_less_and_dotfile_names_are_not_episodes() {
        assert!(parse_podcast_file("Show/noext").is_none());
        assert!(parse_podcast_file("Show/.mp3").is_none());
        assert!(parse_podcast_file("Show/").is_none());
    }

    #[test]
    fn nested_folders_use_the_first_component_as_show() {
        let p = parse_podcast_file("Show/Season_1/Ep.mp3").unwrap();
        assert_eq!(p.show_name, "Show");
        assert_eq!(p.title, "Ep");
    }

    #[test]
    fn scan_detects_episodes_dates_and_filters_other_files() {
        let usb = drive_fixture();
        let episodes = scan_drive_episodes(usb.path());
        assert_eq!(episodes.len(), 3);
        assert!(
            episodes
                .iter()
                .all(|e| e.show_name == "Tech Talk" && e.on_drive && !e.selected)
        );

        let ep1 = episodes
            .iter()
            .find(|e| e.title.contains("Episode 1"))
            .unwrap();
        assert_eq!(format_date(ep1.published.unwrap()), "2024-01-15");
        assert_eq!(ep1.file_size, 100);
        assert_eq!(ep1.duration_secs, 0.0);
        assert_eq!(
            ep1.file_path,
            usb.path()
                .join("Podcasts/Tech_Talk/2024-01-15 - Episode_1.mp3")
        );

        let ep2 = episodes
            .iter()
            .find(|e| e.title.contains("Episode 2"))
            .unwrap();
        assert_eq!(format_date(ep2.published.unwrap()), "2024-01-22");
        assert_eq!(ep2.file_size, 200);

        // Invalid date prefix: the stem is the title and the date falls back to the file's mtime.
        let ep3 = episodes
            .iter()
            .find(|e| e.title == "InvalidDate - Episode 3")
            .unwrap();
        let published = ep3.published.unwrap();
        let now = unix_secs(SystemTime::now());
        assert!(
            (now - published).abs() < 3600,
            "mtime of a just-written file"
        );
    }

    #[test]
    fn episode_ids_are_unique_paths() {
        let usb = TempDir::new().unwrap();
        let show = usb.path().join("Podcasts/Show");
        write(&show.join("2024-01-01 - Same.mp3"), 1);
        write(&show.join("2024-02-01 - Same.mp3"), 1);
        let episodes = scan_drive_episodes(usb.path());
        assert_eq!(episodes.len(), 2);
        assert_ne!(episodes[0].id, episodes[1].id);
    }

    #[test]
    fn scan_groups_by_show_in_first_appearance_order() {
        let usb = TempDir::new().unwrap();
        let root = usb.path().join("Podcasts");
        write(&root.join("B_Show/2024-01-01 - b1.mp3"), 1);
        write(&root.join("A_Show/2024-01-01 - a1.mp3"), 1);
        write(&root.join("A_Show/2024-01-02 - a2.mp3"), 1);
        write(&root.join("2024-01-01 - loose.mp3"), 1);
        let shows: Vec<(String, String)> = scan_drive_episodes(usb.path())
            .into_iter()
            .map(|e| (e.show_name, e.title))
            .collect();
        // Entries sort by name: "2024..." (root file), "A_Show", "B_Show".
        assert_eq!(
            shows,
            [
                ("Unknown Show".to_string(), "loose".to_string()),
                ("A Show".to_string(), "a1".to_string()),
                ("A Show".to_string(), "a2".to_string()),
                ("B Show".to_string(), "b1".to_string()),
            ]
        );
    }

    #[test]
    fn scan_finds_nested_files_and_all_audio_types() {
        let usb = TempDir::new().unwrap();
        let root = usb.path().join("Podcasts/Show");
        for name in [
            "a.mp3", "b.M4A", "c.wav", "d.aac", "e.ogg", "f.flac", "g.txt", "._h.mp3",
        ] {
            write(&root.join(name), 1);
        }
        write(&root.join("Season_1/i.mp3"), 1);
        let mut titles: Vec<String> = scan_drive_episodes(usb.path())
            .into_iter()
            .map(|e| e.title)
            .collect();
        titles.sort();
        assert_eq!(titles, ["a", "b", "c", "d", "e", "f", "i"]);
    }

    #[test]
    fn scan_without_podcasts_folder_is_empty() {
        let usb = TempDir::new().unwrap();
        assert!(scan_drive_episodes(usb.path()).is_empty());
        assert!(build_drive_index(usb.path()).is_empty());
        assert!(!has_podcasts_folder(usb.path()));
        fs::create_dir(usb.path().join("Podcasts")).unwrap();
        assert!(has_podcasts_folder(usb.path()));
        assert!(scan_drive_episodes(usb.path()).is_empty());
    }

    #[test]
    fn index_uses_expected_drive_keys() {
        let usb = drive_fixture();
        let index = build_drive_index(usb.path());
        assert_eq!(index.len(), 3);
        assert!(index.contains_key("Tech_Talk/Episode_1.mp3"));
        assert!(index.contains_key("Tech_Talk/Episode_2.mp3"));
        assert!(index.contains_key("Tech_Talk/InvalidDate_-_Episode_3.mp3"));

        let entry = &index["Tech_Talk/Episode_2.mp3"];
        assert_eq!(entry.size, Some(200));
        assert_eq!(
            entry.path,
            usb.path()
                .join("Podcasts/Tech_Talk/2024-01-22 - Episode_2.mp3")
        );
    }

    #[test]
    fn index_keys_ignore_the_extension() {
        let usb = TempDir::new().unwrap();
        write(&usb.path().join("Podcasts/Show/2024-01-01 - Ep.m4a"), 1);
        assert!(build_drive_index(usb.path()).contains_key("Show/Ep.mp3"));
    }
}
