//! The real [`Backend`]: a thin layer that points the engine's path-taking functions at this Mac's
//! Podcasts library, `/Volumes`, and the shared settings file.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::api::*;
use crate::drive_scan::scan_drive_episodes;
use crate::drives::{DriveScanner, FreeSpaceFn, VolumeInfoSource};
use crate::library::{default_db_path, load_episodes};
use crate::settings::{default_settings_path, load_settings_from, save_settings_to};
use crate::sync_engine::{ExecOptions, delete_files, run_sync};

/// Where a [`LiveBackend`] reads and writes. [`LiveConfig::system`] is this Mac's real locations;
/// tests point everything at temp directories and fixtures.
pub struct LiveConfig {
    pub settings_path: PathBuf,
    pub db_path: PathBuf,
    pub volumes_root: PathBuf,
    /// Describes a mounted volume (`diskutil info`, or a fixture).
    pub volume_info: Arc<dyn VolumeInfoSource>,
    /// Current free bytes of a mounted volume (`statvfs`, or a fixture); `None` falls back to the
    /// figure `diskutil` gave.
    pub free_space: FreeSpaceFn,
    pub exec: ExecOptions,
}

impl LiveConfig {
    pub fn system() -> Self {
        LiveConfig {
            settings_path: default_settings_path(),
            db_path: default_db_path(),
            volumes_root: PathBuf::from("/Volumes"),
            volume_info: Arc::new(crate::drives::Diskutil::default()),
            free_space: Arc::new(crate::drives::statvfs_free_space),
            exec: ExecOptions::default(),
        }
    }
}

pub struct LiveBackend {
    settings_path: PathBuf,
    db_path: PathBuf,
    exec: ExecOptions,
    scanner: DriveScanner,
}

impl LiveBackend {
    pub fn new() -> Self {
        LiveBackend::with_config(LiveConfig::system())
    }

    pub fn with_config(config: LiveConfig) -> Self {
        LiveBackend {
            scanner: DriveScanner::new(config.volumes_root, config.volume_info, config.free_space),
            settings_path: config.settings_path,
            db_path: config.db_path,
            exec: config.exec,
        }
    }
}

impl Default for LiveBackend {
    fn default() -> Self {
        LiveBackend::new()
    }
}

impl Backend for LiveBackend {
    fn load_settings(&self) -> Settings {
        load_settings_from(&self.settings_path)
    }

    fn save_settings(&self, settings: &Settings) -> Result<(), String> {
        save_settings_to(&self.settings_path, settings)
    }

    fn load_mac_episodes(&self) -> Result<Vec<Episode>, String> {
        load_episodes(&self.db_path)
    }

    fn scan_drives(&self) -> Result<Vec<Drive>, String> {
        self.scanner.scan()
    }

    fn scan_drive(&self, drive: &Drive) -> Result<Vec<Episode>, String> {
        Ok(scan_drive_episodes(&drive.mount_point))
    }

    fn sync(
        &self,
        episodes: &[Episode],
        drive: &Drive,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(TransferProgress),
    ) -> Result<SyncOutcome, String> {
        run_sync(episodes, &drive.mount_point, cancel, &self.exec, progress)
    }

    fn delete_from_drive(&self, episodes: &[Episode]) -> Result<(), String> {
        delete_files(episodes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plist::{Dictionary, Value};
    use rusqlite::Connection;
    use std::fs;
    use std::path::Path;
    use std::time::Duration;
    use tempfile::TempDir;

    struct OneUsbDisk(PathBuf);

    impl VolumeInfoSource for OneUsbDisk {
        fn info(&self, mount_point: &Path) -> Result<Dictionary, String> {
            let mut d = Dictionary::new();
            d.insert("VolumeName".into(), Value::String("WALKMAN".into()));
            d.insert(
                "MountPoint".into(),
                Value::String(self.0.to_string_lossy().into_owned()),
            );
            d.insert("DeviceIdentifier".into(), Value::String("disk9s1".into()));
            d.insert("Internal".into(), Value::Boolean(false));
            d.insert("Removable".into(), Value::Boolean(true));
            d.insert("TotalSize".into(), Value::Integer(8_000_000_000u64.into()));
            d.insert(
                "VolumeFreeSpace".into(),
                Value::Integer(4_000_000_000u64.into()),
            );
            assert!(mount_point.starts_with(self.0.parent().unwrap()));
            Ok(d)
        }
    }

    /// Everything pointed at a scratch directory: never the real library, settings or volumes.
    struct Sandbox {
        dir: TempDir,
    }

    impl Sandbox {
        fn new() -> Self {
            let dir = TempDir::new().unwrap();
            fs::create_dir_all(dir.path().join("Volumes/WALKMAN")).unwrap();
            Sandbox { dir }
        }
        fn path(&self, p: &str) -> PathBuf {
            self.dir.path().join(p)
        }
        fn backend(&self) -> LiveBackend {
            LiveBackend::with_config(LiveConfig {
                settings_path: self.path("config/castoff.jsonc"),
                db_path: self.path("MTLibrary.sqlite"),
                volumes_root: self.path("Volumes"),
                volume_info: Arc::new(OneUsbDisk(self.path("Volumes/WALKMAN"))),
                free_space: Arc::new(|_| None),
                exec: ExecOptions {
                    chunk_size: 512,
                    progress_interval: Duration::ZERO,
                },
            })
        }
    }

    #[test]
    fn settings_round_trip_through_the_backend() {
        let sb = Sandbox::new();
        let backend = sb.backend();
        assert_eq!(backend.load_settings(), Settings::default());
        let s = Settings {
            theme: "Nord".into(),
            favorite_drives: vec!["disk9s1".into()],
        };
        backend.save_settings(&s).unwrap();
        assert_eq!(backend.load_settings(), s);
        assert!(
            fs::read_to_string(sb.path("config/castoff.jsonc"))
                .unwrap()
                .contains("$schema")
        );
    }

    #[test]
    fn missing_library_is_a_user_facing_error() {
        let sb = Sandbox::new();
        let err = sb.backend().load_mac_episodes().unwrap_err();
        assert!(err.contains("not found"), "{err}");
    }

    #[test]
    fn end_to_end_library_drive_sync_delete() {
        let sb = Sandbox::new();

        // A Podcasts library with two downloaded episodes.
        let audio_dir = sb.path("assets");
        fs::create_dir_all(&audio_dir).unwrap();
        let ep1_file = audio_dir.join("one.mp3");
        let ep2_file = audio_dir.join("two.mp3");
        fs::write(&ep1_file, vec![1u8; 3000]).unwrap();
        fs::write(&ep2_file, vec![2u8; 2000]).unwrap();
        let conn = Connection::open(sb.path("MTLibrary.sqlite")).unwrap();
        conn.execute_batch(&format!(
            "CREATE TABLE ZMTPODCAST (Z_PK INTEGER PRIMARY KEY, ZUUID TEXT, ZTITLE TEXT);
             CREATE TABLE ZMTEPISODE (Z_PK INTEGER PRIMARY KEY, ZPODCASTUUID TEXT, ZTITLE TEXT,
                                      ZASSETURL TEXT, ZPUBDATE REAL, ZDURATION REAL);
             INSERT INTO ZMTPODCAST (ZUUID, ZTITLE) VALUES ('P1', 'Tech Talk');
             INSERT INTO ZMTEPISODE (Z_PK, ZPODCASTUUID, ZTITLE, ZASSETURL, ZPUBDATE, ZDURATION) VALUES
               (1, 'P1', 'Episode One', 'file://{}', 726000000, 60),
               (2, 'P1', 'Episode Two', 'file://{}', 726100000, 90);",
            ep1_file.display(),
            ep2_file.display()
        ))
        .unwrap();
        drop(conn);

        let backend = sb.backend();
        let mut mac = backend.load_mac_episodes().unwrap();
        assert_eq!(mac.len(), 2);
        assert_eq!(mac[0].title, "Episode Two", "newest first");
        assert_eq!((mac[0].file_size, mac[1].file_size), (2000, 3000));

        // The one connected disk is found, and nothing else.
        let drives = backend.scan_drives().unwrap();
        assert_eq!(drives.len(), 1);
        let drive = drives[0].clone();
        assert_eq!(drive.name, "WALKMAN");
        assert_eq!(drive.free_space, Some(4_000_000_000));
        assert_eq!(drive.mount_point, sb.path("Volumes/WALKMAN"));

        // Nothing on the drive yet.
        let on_drive = backend.scan_drive(&drive).unwrap();
        assert!(on_drive.is_empty());
        mark_on_drive(&mut mac, &on_drive);
        assert!(mac.iter().all(|e| !e.on_drive));

        // Sync both.
        let cancel = AtomicBool::new(false);
        let mut last = TransferProgress::default();
        let outcome = backend
            .sync(&mac, &drive, &cancel, &mut |p| last = p)
            .unwrap();
        assert_eq!(
            outcome,
            SyncOutcome::Complete {
                copied: 2,
                failed: vec![]
            }
        );
        assert_eq!(last.current_file, "Finalizing drive...");
        assert_eq!(
            (
                last.files_done,
                last.total_files,
                last.bytes_transferred,
                last.total_bytes
            ),
            (2, 2, 5000, 5000)
        );

        // They show up on the drive and match their Mac originals.
        let on_drive = backend.scan_drive(&drive).unwrap();
        assert_eq!(on_drive.len(), 2);
        assert!(on_drive.iter().all(|e| e.show_name == "Tech Talk"));
        mark_on_drive(&mut mac, &on_drive);
        assert!(mac.iter().all(|e| e.on_drive));

        // A second sync has nothing to do.
        let outcome = backend.sync(&mac, &drive, &cancel, &mut |_| {}).unwrap();
        assert_eq!(outcome, SyncOutcome::AlreadySynced);

        // Delete one from the drive; its Mac twin stops being "on drive" after a re-scan.
        let victim = on_drive
            .iter()
            .find(|e| e.title == "Episode One")
            .unwrap()
            .clone();
        backend.delete_from_drive(&[victim]).unwrap();
        let on_drive = backend.scan_drive(&drive).unwrap();
        assert_eq!(on_drive.len(), 1);
        mark_on_drive(&mut mac, &on_drive);
        let flags: Vec<(&str, bool)> = mac.iter().map(|e| (e.title.as_str(), e.on_drive)).collect();
        assert_eq!(flags, [("Episode Two", true), ("Episode One", false)]);

        // Deleting the last one removes the now-empty show folder too.
        backend.delete_from_drive(&on_drive).unwrap();
        assert!(!sb.path("Volumes/WALKMAN/Podcasts/Tech_Talk").exists());

        // The originals never changed.
        assert_eq!(fs::read(&ep1_file).unwrap(), vec![1u8; 3000]);
    }

    #[test]
    fn backend_is_send_sync_and_default_constructs() {
        fn assert_backend<T: Backend + Default>() {}
        assert_backend::<LiveBackend>();
    }
}
