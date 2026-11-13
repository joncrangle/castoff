//! Matching Mac episodes to files on the drive.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::api::Episode;
use crate::format::sanitize_filename;

/// One file on the drive, keyed in a [`DriveIndex`] by its expected `Show/Title.mp3` path.
/// `size` and `duration` are only consulted by the fallback matches.
#[derive(Clone, Debug, PartialEq)]
pub struct IndexEntry {
    pub path: PathBuf,
    pub size: Option<u64>,
    pub duration: Option<f64>,
}

/// The drive's files keyed by `build_expected_drive_path(show, title)`.
pub type DriveIndex = HashMap<String, IndexEntry>;

/// The parts of a Mac episode the matcher looks at.
#[derive(Clone, Copy, Debug, Default)]
pub struct MatchProbe<'a> {
    pub title: &'a str,
    pub file_size: Option<u64>,
    pub duration: Option<f64>,
}

impl<'a> From<&'a Episode> for MatchProbe<'a> {
    fn from(ep: &'a Episode) -> Self {
        MatchProbe {
            title: &ep.title,
            file_size: Some(ep.file_size),
            duration: Some(ep.duration_secs),
        }
    }
}

/// The index key for an episode: `Sanitized_Show/Sanitized_Title.mp3`. The extension is always
/// `.mp3` on both sides, so an `.m4a` on the drive matches its Mac episode too.
pub fn build_expected_drive_path(show_name: &str, title: &str) -> String {
    format!(
        "{}/{}.mp3",
        sanitize_filename(show_name),
        sanitize_filename(title)
    )
}

/// Whether the episode is already on the drive: by expected path first, then (same show only) by
/// equal file size, then by duration within 2%. Zero and missing sizes/durations never match.
pub fn match_episode(show_name: &str, episode: MatchProbe<'_>, index: &DriveIndex) -> bool {
    if index.contains_key(&build_expected_drive_path(show_name, episode.title)) {
        return true;
    }

    let show_prefix = format!("{}/", sanitize_filename(show_name));
    let ep_size = episode.file_size.filter(|&s| s != 0);
    let ep_duration = episode.duration.filter(|&d| d != 0.0 && !d.is_nan());

    for (key, info) in index {
        if !key.starts_with(&show_prefix) {
            continue;
        }
        if let (Some(a), Some(b)) = (ep_size, info.size.filter(|&s| s != 0))
            && a == b
        {
            return true;
        }
        if let (Some(a), Some(b)) = (
            ep_duration,
            info.duration.filter(|&d| d != 0.0 && !d.is_nan()),
        ) {
            let tolerance = a * 0.02;
            if (a - b).abs() <= tolerance {
                return true;
            }
        }
    }
    false
}

fn episode_key(show_name: &str, title: &str) -> String {
    format!(
        "{}/{}",
        sanitize_filename(show_name),
        sanitize_filename(title)
    )
}

/// Sets `on_drive` on every Mac episode: true when a drive episode has the same sanitised
/// `show/title`, false otherwise (existing values are overwritten).
pub fn mark_on_drive(mac: &mut [Episode], drive: &[Episode]) {
    let on_drive: HashSet<String> = drive
        .iter()
        .map(|e| episode_key(&e.show_name, &e.title))
        .collect();
    for episode in mac {
        episode.on_drive = on_drive.contains(&episode_key(&episode.show_name, &episode.title));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, size: Option<u64>, duration: Option<f64>) -> IndexEntry {
        IndexEntry {
            path: PathBuf::from(path),
            size,
            duration,
        }
    }

    fn episode(show: &str, title: &str) -> Episode {
        Episode {
            id: "1".into(),
            title: title.into(),
            show_name: show.into(),
            duration_secs: 3600.0,
            published: Some(1_672_531_200),
            on_drive: false,
            file_path: PathBuf::from("/path/to/ep1.mp3"),
            file_size: 1000,
            selected: false,
        }
    }

    #[test]
    fn expected_path_format() {
        assert_eq!(
            build_expected_drive_path("Tech Talk", "Episode 1: Intro"),
            "Tech_Talk/Episode_1-_Intro.mp3"
        );
    }

    #[test]
    fn matches_by_path_size_or_duration_within_same_show() {
        let index: DriveIndex = [
            (
                "Tech_Talk/Episode_1.mp3",
                entry("/path/ep1.mp3", Some(1000), Some(100.0)),
            ),
            (
                "Tech_Talk/Episode_2_Renamed.mp3",
                entry("/path/ep2.mp3", Some(2000), Some(200.0)),
            ),
            (
                "Tech_Talk/Episode_3_Renamed.mp3",
                entry("/path/ep3.mp3", Some(3000), Some(300.0)),
            ),
            // Same size/duration as Episode 2 but a different show.
            (
                "Other_Show/Episode_1.mp3",
                entry("/path/other.mp3", Some(2000), Some(200.0)),
            ),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();

        // 1. Path match.
        assert!(match_episode(
            "Tech Talk",
            MatchProbe {
                title: "Episode 1",
                ..Default::default()
            },
            &index
        ));
        // 2. Size match, same show.
        assert!(match_episode(
            "Tech Talk",
            MatchProbe {
                title: "Episode 2",
                file_size: Some(2000),
                duration: None
            },
            &index
        ));
        // 3. Duration within 2% (303 vs 300 is 1%).
        assert!(match_episode(
            "Tech Talk",
            MatchProbe {
                title: "Episode 3",
                file_size: None,
                duration: Some(303.0)
            },
            &index
        ));
        // 4. Same size/duration but a different show must not match.
        assert!(!match_episode(
            "Other Show",
            MatchProbe {
                title: "Episode 4",
                file_size: Some(1000),
                duration: Some(100.0)
            },
            &index
        ));
        // 5. Outside the duration tolerance.
        assert!(!match_episode(
            "Tech Talk",
            MatchProbe {
                title: "Episode 5",
                file_size: None,
                duration: Some(310.0)
            },
            &index
        ));
    }

    #[test]
    fn zero_size_and_duration_never_match() {
        let index: DriveIndex = [(
            "Show/Other.mp3".to_string(),
            entry("/p", Some(0), Some(0.0)),
        )]
        .into_iter()
        .collect();
        assert!(!match_episode(
            "Show",
            MatchProbe {
                title: "X",
                file_size: Some(0),
                duration: Some(0.0)
            },
            &index
        ));
        // Path-only entries (what a sync passes) never size/duration-match.
        let index: DriveIndex = [("Show/Other.mp3".to_string(), entry("/p", None, None))]
            .into_iter()
            .collect();
        assert!(!match_episode(
            "Show",
            MatchProbe {
                title: "X",
                file_size: Some(10),
                duration: Some(10.0)
            },
            &index
        ));
    }

    #[test]
    fn marks_episodes_that_match() {
        let mut mac = vec![episode("Show A", "Ep 1"), episode("Show A", "Ep 2")];
        let drive = vec![episode("Show A", "Ep 1")];
        mark_on_drive(&mut mac, &drive);
        assert!(mac[0].on_drive);
        assert!(!mac[1].on_drive);
    }

    #[test]
    fn marking_overwrites_existing_flags() {
        let mut mac = vec![episode("Show A", "Ep 1")];
        mac[0].on_drive = true;
        mark_on_drive(&mut mac, &[]);
        assert!(!mac[0].on_drive);
    }

    #[test]
    fn marking_tolerates_mismatched_sanitisation() {
        // Mac "Show & Tell" and the drive's "Show and Tell" both sanitise to "Show_and_Tell".
        let mut mac = vec![episode("Show & Tell", "Ep 1")];
        let drive = vec![episode("Show and Tell", "Ep 1")];
        mark_on_drive(&mut mac, &drive);
        assert!(mac[0].on_drive);
    }

    #[test]
    fn marking_through_the_public_api() {
        let mut mac = vec![episode("Show A", "Ep 1")];
        crate::api::mark_on_drive(&mut mac, &[episode("Show A", "Ep 1")]);
        assert!(mac[0].on_drive);
    }
}
