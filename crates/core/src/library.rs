//! Reading the macOS Podcasts library.
//!
//! The database is only ever opened read-only (`SQLITE_OPEN_READ_ONLY` plus `query_only`), and
//! only for the duration of one query.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};
use url::Url;

use crate::api::Episode;

/// Where the Podcasts app keeps its library, relative to the home directory.
pub const DB_RELATIVE_PATH: &str =
    "Library/Group Containers/243LU875E5.groups.com.apple.podcasts/Documents/MTLibrary.sqlite";

/// Seconds between the Unix epoch (1970) and the Apple/Core Data epoch (2001-01-01).
pub const APPLE_EPOCH_OFFSET: i64 = 978_307_200;

const QUERY: &str = "
    SELECT
      e.Z_PK AS id,
      e.ZTITLE AS title,
      p.ZTITLE AS showName,
      e.ZASSETURL AS assetUrl,
      e.ZPUBDATE AS pubDate,
      e.ZDURATION AS duration
    FROM ZMTEPISODE e
    JOIN ZMTPODCAST p ON e.ZPODCASTUUID = p.ZUUID
    WHERE e.ZASSETURL IS NOT NULL
    ORDER BY e.ZPUBDATE DESC
";

/// `~/Library/Group Containers/243LU875E5.groups.com.apple.podcasts/Documents/MTLibrary.sqlite`.
pub fn default_db_path() -> PathBuf {
    std::env::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(DB_RELATIVE_PATH)
}

/// JavaScript's `decodeURIComponent`: percent-decoding that rejects malformed escapes and
/// sequences that aren't valid UTF-8.
fn decode_uri_component(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hex = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Turns an asset URL into a file path:
/// `decodeURIComponent(new URL(url).pathname)`, and when that fails (not a URL, or a bad escape)
/// the raw string with the first `file://` removed.
pub fn asset_url_to_path(asset_url: &str) -> String {
    Url::parse(asset_url)
        .ok()
        .and_then(|u| decode_uri_component(u.path()))
        .unwrap_or_else(|| asset_url.replacen("file://", "", 1))
}

/// One row of the query, before mapping.
struct Row {
    id: i64,
    title: Option<String>,
    show_name: Option<String>,
    asset_url: Option<String>,
    pub_date: Option<f64>,
    duration: Option<f64>,
}

fn row_to_episode(row: Row) -> Episode {
    let file_path = match row.asset_url.as_deref() {
        Some(url) if !url.is_empty() => PathBuf::from(asset_url_to_path(url)),
        _ => PathBuf::new(),
    };
    // A zero or missing date is "unknown".
    let published = row
        .pub_date
        .filter(|&d| d != 0.0 && d.is_finite())
        .map(|d| d.floor() as i64 + APPLE_EPOCH_OFFSET);
    let file_size = if file_path.as_os_str().is_empty() {
        0
    } else {
        fs::metadata(&file_path).map(|m| m.len()).unwrap_or(0)
    };

    Episode {
        id: row.id.to_string(),
        title: row.title.unwrap_or_else(|| "Untitled".to_string()),
        show_name: row.show_name.unwrap_or_else(|| "Unknown".to_string()),
        duration_secs: row.duration.unwrap_or(0.0),
        published,
        on_drive: false,
        file_path,
        file_size,
        selected: false,
    }
}

fn open_read_only(db_path: &Path) -> Result<Connection, rusqlite::Error> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    // The Podcasts app may be writing: wait briefly rather than fail, and refuse writes outright.
    conn.busy_timeout(Duration::from_secs(2))?;
    conn.pragma_update(None, "query_only", true)?;
    Ok(conn)
}

fn query_rows(conn: &Connection) -> Result<Vec<Row>, rusqlite::Error> {
    let mut stmt = conn.prepare(QUERY)?;
    let rows = stmt.query_map([], |r| {
        Ok(Row {
            id: r.get(0)?,
            title: r.get(1)?,
            show_name: r.get(2)?,
            asset_url: r.get(3)?,
            pub_date: r.get(4)?,
            duration: r.get(5)?,
        })
    })?;
    rows.collect()
}

/// Downloaded episodes from the Podcasts database at `db_path`, newest first, with the size of each
/// asset file (0 when it can't be found). `on_drive` and `selected` are false.
///
/// This lists every episode that has an asset URL, whether or not the file still
/// exists.
pub fn load_episodes(db_path: &Path) -> Result<Vec<Episode>, String> {
    if !db_path.exists() {
        return Err(format!(
            "Podcasts library not found at {}",
            db_path.display()
        ));
    }
    let rows = open_read_only(db_path)
        .and_then(|conn| query_rows(&conn))
        .map_err(|e| {
            format!(
                "Could not read the Podcasts library at {}: {e}. \
                 If macOS blocked access, give this app Full Disk Access in System Settings > \
                 Privacy & Security.",
                db_path.display()
            )
        })?;
    Ok(rows.into_iter().map(row_to_episode).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ---- asset URL -> path ------------------------------------------------------------------

    #[test]
    fn decodes_file_urls() {
        assert_eq!(
            asset_url_to_path("file:///Users/user/My%20Documents/file.txt"),
            "/Users/user/My Documents/file.txt"
        );
        assert_eq!(
            asset_url_to_path("file:///a/b%20c/d%E2%9C%93.mp3"),
            "/a/b c/d✓.mp3"
        );
    }

    #[test]
    fn malformed_escapes_fall_back_to_the_raw_string() {
        assert_eq!(asset_url_to_path("file:///foo%2"), "/foo%2");
        // Not valid UTF-8 once decoded.
        assert_eq!(asset_url_to_path("file:///foo%FF.mp3"), "/foo%FF.mp3");
    }

    #[test]
    fn non_urls_fall_back_to_the_raw_string() {
        assert_eq!(
            asset_url_to_path("/Users/user/file.mp3"),
            "/Users/user/file.mp3"
        );
        assert_eq!(asset_url_to_path("file://%"), "%");
    }

    #[test]
    fn plus_signs_and_literal_percent_encoded_slashes() {
        assert_eq!(asset_url_to_path("file:///a+b/c%2Fd.mp3"), "/a+b/c/d.mp3");
    }

    #[test]
    fn database_path_points_at_the_podcasts_group_container() {
        let path = default_db_path();
        let text = path.to_string_lossy();
        assert!(text.contains("243LU875E5.groups.com.apple.podcasts"));
        assert!(text.ends_with("MTLibrary.sqlite"));
    }

    // ---- fixture database -------------------------------------------------------------------

    fn make_db(dir: &Path) -> PathBuf {
        let path = dir.join("MTLibrary.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE ZMTPODCAST (Z_PK INTEGER PRIMARY KEY, ZUUID TEXT, ZTITLE TEXT);
             CREATE TABLE ZMTEPISODE (Z_PK INTEGER PRIMARY KEY, ZPODCASTUUID TEXT, ZTITLE TEXT,
                                      ZASSETURL TEXT, ZPUBDATE REAL, ZDURATION REAL);
             INSERT INTO ZMTPODCAST (ZUUID, ZTITLE) VALUES ('P1', 'Tech Talk'), ('P2', NULL);",
        )
        .unwrap();
        path
    }

    fn insert(db: &Path, sql: &str) {
        Connection::open(db).unwrap().execute_batch(sql).unwrap();
    }

    #[test]
    fn missing_database_is_an_error_naming_the_path() {
        let dir = TempDir::new().unwrap();
        let err = load_episodes(&dir.path().join("nope.sqlite")).unwrap_err();
        assert!(err.contains("not found"), "{err}");
        assert!(err.contains("nope.sqlite"), "{err}");
    }

    #[test]
    fn loads_rows_newest_first_with_mapped_fields() {
        let dir = TempDir::new().unwrap();
        let db = make_db(dir.path());
        let audio = dir.path().join("My Episode.mp3");
        fs::write(&audio, vec![0u8; 1234]).unwrap();
        let url = format!("file://{}", audio.display()).replace(' ', "%20");
        insert(
            &db,
            &format!(
                "INSERT INTO ZMTEPISODE (Z_PK, ZPODCASTUUID, ZTITLE, ZASSETURL, ZPUBDATE, ZDURATION)
                 VALUES (7, 'P1', 'Older', '{url}', 700000000.5, 100.5),
                        (9, 'P1', 'Newer', 'file:///definitely/missing/new.mp3', 700000100.0, 3600);"
            ),
        );

        let episodes = load_episodes(&db).unwrap();
        assert_eq!(episodes.len(), 2);

        assert_eq!(episodes[0].id, "9");
        assert_eq!(episodes[0].title, "Newer");
        assert_eq!(episodes[0].show_name, "Tech Talk");
        assert_eq!(
            episodes[0].published,
            Some(700_000_100 + APPLE_EPOCH_OFFSET)
        );
        assert_eq!(episodes[0].duration_secs, 3600.0);
        assert_eq!(
            episodes[0].file_path,
            PathBuf::from("/definitely/missing/new.mp3")
        );
        assert_eq!(episodes[0].file_size, 0);

        assert_eq!(episodes[1].id, "7");
        assert_eq!(
            episodes[1].published,
            Some(700_000_000 + APPLE_EPOCH_OFFSET)
        );
        assert_eq!(episodes[1].duration_secs, 100.5);
        assert_eq!(episodes[1].file_path, audio);
        assert_eq!(episodes[1].file_size, 1234);

        for e in &episodes {
            assert!(!e.on_drive && !e.selected);
        }
    }

    #[test]
    fn skips_rows_without_an_asset_url_or_a_matching_podcast() {
        let dir = TempDir::new().unwrap();
        let db = make_db(dir.path());
        insert(
            &db,
            "INSERT INTO ZMTEPISODE (ZPODCASTUUID, ZTITLE, ZASSETURL, ZPUBDATE, ZDURATION) VALUES
               ('P1', 'No asset', NULL, 1, 1),
               ('GHOST', 'Orphan', 'file:///x.mp3', 1, 1),
               ('P1', 'Kept', 'file:///x.mp3', 1, 1);",
        );
        let episodes = load_episodes(&db).unwrap();
        assert_eq!(episodes.len(), 1);
        assert_eq!(episodes[0].title, "Kept");
    }

    #[test]
    fn null_columns_get_the_tuis_defaults() {
        let dir = TempDir::new().unwrap();
        let db = make_db(dir.path());
        insert(
            &db,
            "INSERT INTO ZMTEPISODE (ZPODCASTUUID, ZTITLE, ZASSETURL, ZPUBDATE, ZDURATION) VALUES
               ('P2', NULL, 'file:///x.mp3', NULL, NULL),
               ('P1', 'Zero date', 'file:///y.mp3', 0, 5),
               ('P1', 'Empty url', '', 5, 5);",
        );
        let episodes = load_episodes(&db).unwrap();
        let by_title = |t: &str| episodes.iter().find(|e| e.title == t).unwrap();

        let blank = by_title("Untitled");
        assert_eq!(blank.show_name, "Unknown");
        assert_eq!(blank.published, None);
        assert_eq!(blank.duration_secs, 0.0);

        assert_eq!(by_title("Zero date").published, None);
        assert_eq!(by_title("Empty url").file_path, PathBuf::new());
    }

    #[test]
    fn integer_columns_and_percent_encoded_paths_work() {
        let dir = TempDir::new().unwrap();
        let db = make_db(dir.path());
        insert(
            &db,
            "INSERT INTO ZMTEPISODE (ZPODCASTUUID, ZTITLE, ZASSETURL, ZPUBDATE, ZDURATION) VALUES
               ('P1', 'Ints', 'file:///Users/me/Pod%20casts/a%26b.mp3', 800000000, 61);",
        );
        let e = &load_episodes(&db).unwrap()[0];
        assert_eq!(e.published, Some(800_000_000 + APPLE_EPOCH_OFFSET));
        assert_eq!(e.duration_secs, 61.0);
        assert_eq!(e.file_path, PathBuf::from("/Users/me/Pod casts/a&b.mp3"));
    }

    #[test]
    fn a_database_without_the_expected_tables_is_a_clear_error() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("other.sqlite");
        Connection::open(&db)
            .unwrap()
            .execute_batch("CREATE TABLE x (a)")
            .unwrap();
        let err = load_episodes(&db).unwrap_err();
        assert!(err.contains("Could not read the Podcasts library"), "{err}");
    }

    #[test]
    fn a_file_that_is_not_a_database_is_a_clear_error() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("junk.sqlite");
        fs::write(
            &db,
            "this is not sqlite at all, just text padding padding padding",
        )
        .unwrap();
        assert!(
            load_episodes(&db)
                .unwrap_err()
                .contains("Could not read the Podcasts library")
        );
    }

    #[test]
    fn loading_never_modifies_the_database() {
        let dir = TempDir::new().unwrap();
        let db = make_db(dir.path());
        insert(
            &db,
            "INSERT INTO ZMTEPISODE (ZPODCASTUUID, ZTITLE, ZASSETURL) VALUES ('P1', 'A', 'file:///a.mp3');",
        );
        let before = fs::read(&db).unwrap();
        let mtime = fs::metadata(&db).unwrap().modified().unwrap();
        load_episodes(&db).unwrap();
        assert_eq!(fs::read(&db).unwrap(), before);
        assert_eq!(fs::metadata(&db).unwrap().modified().unwrap(), mtime);
        // A read-only connection can't write even if asked to.
        let conn = open_read_only(&db).unwrap();
        assert!(conn.execute("DELETE FROM ZMTEPISODE", []).is_err());
    }

    #[test]
    fn works_on_a_read_only_directory_without_creating_files() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let db = make_db(dir.path());
        insert(
            &db,
            "INSERT INTO ZMTEPISODE (ZPODCASTUUID, ZTITLE, ZASSETURL) VALUES ('P1', 'A', 'file:///a.mp3');",
        );
        let before: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).unwrap();
        let result = load_episodes(&db);
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(result.unwrap().len(), 1);
        let after: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(before.len(), after.len());
    }
}
