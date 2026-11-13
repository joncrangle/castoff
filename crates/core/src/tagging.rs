//! ID3 tagging of copied episodes.
//!
//! This *replaces* the file's ID3v2 tag with one holding only the
//! frames below; the audio is untouched.

use std::path::Path;

use chrono::{DateTime, Datelike, Local, TimeZone, Utc};
use id3::{Tag, TagLike, Version, frame::Comment};

/// What gets written. Empty fields are skipped.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PodcastTags {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub comment: String,
    pub year: Option<i32>,
}

impl PodcastTags {
    /// The tags written for a synced episode: title, album = show, genre "Podcast", the
    /// year (local time) and a "Published: YYYY-MM-DD" comment (UTC date).
    ///
    /// The year and comment are left out when the publication date is unknown.
    pub fn for_episode(title: &str, author: &str, show_name: &str, published: Option<i64>) -> Self {
        let published = published.filter(|&p| p != 0);
        let year = published
            .and_then(|p| Local.timestamp_opt(p, 0).single())
            .map(|dt| dt.year());
        let comment = published
            .and_then(|p| DateTime::<Utc>::from_timestamp(p, 0))
            .map(|dt| format!("Published: {}", dt.format("%Y-%m-%d")))
            .unwrap_or_default();
        PodcastTags {
            title: title.to_string(),
            artist: author.to_string(),
            album: show_name.to_string(),
            genre: "Podcast".to_string(),
            comment,
            year,
        }
    }
}

/// Whether this file should be tagged: ID3 only belongs on MP3s. (Prepending an ID3 header
/// to an `.m4a` would break it.)
pub fn is_taggable(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("mp3"))
        || path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.to_ascii_lowercase().ends_with(".mp3.castoff-part"))
}

/// Writes `tags` into the file as an ID3v2.3 tag, replacing any existing ID3v2 tag.
pub fn write_tags(path: &Path, tags: &PodcastTags) -> Result<(), String> {
    let mut tag = Tag::new();
    if !tags.title.is_empty() {
        tag.set_title(tags.title.as_str());
    }
    if !tags.artist.is_empty() {
        tag.set_artist(tags.artist.as_str());
    }
    if !tags.album.is_empty() {
        tag.set_album(tags.album.as_str());
    }
    if !tags.genre.is_empty() {
        tag.set_genre(tags.genre.as_str());
    }
    if let Some(year) = tags.year {
        tag.set_year(year);
    }
    if !tags.comment.is_empty() {
        tag.add_frame(Comment {
            lang: "eng".to_string(),
            description: String::new(),
            text: tags.comment.clone(),
        });
    }
    tag.write_to_path(path, Version::Id3v23)
        .map_err(|e| format!("Failed to write tags to {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn writes_id3_tags_to_an_empty_file() {
        // Tag a zero-byte file and see it grow.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("temp-test-tag.mp3");
        fs::write(&path, []).unwrap();
        write_tags(
            &path,
            &PodcastTags {
                title: "Test Episode".into(),
                artist: "Test Artist".into(),
                album: "Test Album".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(fs::metadata(&path).unwrap().len() > 0);

        let tag = Tag::read_from_path(&path).unwrap();
        assert_eq!(tag.title(), Some("Test Episode"));
        assert_eq!(tag.artist(), Some("Test Artist"));
        assert_eq!(tag.album(), Some("Test Album"));
    }

    #[test]
    fn writes_every_field_and_keeps_the_audio() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("ep.mp3");
        let audio: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
        fs::write(&path, &audio).unwrap();

        let tags = PodcastTags {
            title: "Episode: One".into(),
            artist: "Host A".into(),
            album: "Tech Talk".into(),
            genre: "Podcast".into(),
            comment: "Published: 2024-01-15".into(),
            year: Some(2024),
        };
        write_tags(&path, &tags).unwrap();

        let tag = Tag::read_from_path(&path).unwrap();
        assert_eq!(tag.title(), Some("Episode: One"));
        assert_eq!(tag.artist(), Some("Host A"));
        assert_eq!(tag.album(), Some("Tech Talk"));
        assert_eq!(tag.genre(), Some("Podcast"));
        assert_eq!(tag.year(), Some(2024));
        let comment = tag.comments().next().unwrap();
        assert_eq!(comment.text, "Published: 2024-01-15");
        assert_eq!(comment.lang, "eng");

        let bytes = fs::read(&path).unwrap();
        assert!(bytes.starts_with(b"ID3\x03"), "ID3v2.3 header");
        assert!(bytes.ends_with(&audio), "audio payload untouched");
    }

    #[test]
    fn replaces_an_existing_tag_instead_of_stacking_a_second() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("ep.mp3");
        let audio = vec![0xFFu8; 2000];
        fs::write(&path, &audio).unwrap();

        let first = PodcastTags {
            title: "Old title".into(),
            artist: "Old artist".into(),
            ..Default::default()
        };
        write_tags(&path, &first).unwrap();
        let second = PodcastTags {
            title: "New title".into(),
            ..Default::default()
        };
        write_tags(&path, &second).unwrap();

        let tag = Tag::read_from_path(&path).unwrap();
        assert_eq!(tag.title(), Some("New title"));
        assert_eq!(tag.artist(), None, "the old tag's other frames are gone");
        let bytes = fs::read(&path).unwrap();
        assert!(bytes.ends_with(&audio));
        assert_eq!(bytes.windows(3).filter(|w| *w == b"ID3").count(), 1);
    }

    #[test]
    fn failing_to_open_the_file_is_an_error() {
        let dir = TempDir::new().unwrap();
        let err = write_tags(&dir.path().join("missing.mp3"), &PodcastTags::default()).unwrap_err();
        assert!(err.contains("Failed to write tags"), "{err}");
    }

    #[test]
    fn episode_tags_match_the_tuis() {
        // 2024-01-15 12:00 UTC.
        let published = Utc
            .with_ymd_and_hms(2024, 1, 15, 12, 0, 0)
            .unwrap()
            .timestamp();
        let tags = PodcastTags::for_episode("Ep 1", "", "Tech Talk", Some(published));
        assert_eq!(tags.title, "Ep 1");
        assert_eq!(tags.album, "Tech Talk");
        assert_eq!(tags.genre, "Podcast");
        assert_eq!(tags.artist, "");
        assert_eq!(tags.comment, "Published: 2024-01-15");
        assert_eq!(tags.year, Some(2024));
    }

    #[test]
    fn unknown_publication_date_leaves_year_and_comment_out() {
        for unknown in [None, Some(0)] {
            let tags = PodcastTags::for_episode("Ep", "", "Show", unknown);
            assert_eq!(tags.year, None);
            assert_eq!(tags.comment, "");
        }
    }

    #[test]
    fn only_mp3s_are_taggable() {
        assert!(is_taggable(Path::new("/x/a.mp3")));
        assert!(is_taggable(Path::new("/x/a.MP3")));
        assert!(is_taggable(Path::new("/x/a.mp3.castoff-part")));
        assert!(!is_taggable(Path::new("/x/a.m4a")));
        assert!(!is_taggable(Path::new("/x/a.m4a.castoff-part")));
        assert!(!is_taggable(Path::new("/x/mp3")));
    }
}
