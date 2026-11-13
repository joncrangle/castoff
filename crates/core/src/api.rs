//! The contract between the sync engine (this crate) and the UI (`crates/app`).
//! FROZEN: both sides are built against this file in parallel. Do not change signatures.
//!
//! Every `Backend` method blocks and may be slow (SQLite, `diskutil`, file copies), so the UI
//! calls them off the main thread. Errors are user-facing messages.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

/// One episode row, in either pane. On the Mac side `file_path` is the downloaded asset in the
/// Podcasts library; on the drive side it is the file on the drive.
#[derive(Clone, Debug, PartialEq)]
pub struct Episode {
    pub id: String,
    pub title: String,
    pub show_name: String,
    pub duration_secs: f64,
    /// Unix seconds; `None` when unknown (drive files carry no date).
    pub published: Option<i64>,
    /// Mac side only: an equivalent file already exists on the current drive.
    pub on_drive: bool,
    pub file_path: PathBuf,
    pub file_size: u64,
    pub selected: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Drive {
    pub id: String,
    pub name: String,
    pub bsd_name: String,
    pub mount_point: PathBuf,
    pub total_space: u64,
    /// `None` when unknown.
    pub free_space: Option<u64>,
}

/// `~/.config/castoff/castoff.jsonc`.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub theme: String,
    pub favorite_drives: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { theme: "Catppuccin".into(), favorite_drives: Vec::new() }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TransferProgress {
    pub current_file: String,
    pub files_done: usize,
    pub total_files: usize,
    pub bytes_transferred: u64,
    pub total_bytes: u64,
    /// Bytes per second.
    pub speed: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SyncOutcome {
    /// `failed` is (file, error) per episode that could not be copied.
    Complete { copied: usize, failed: Vec<(String, String)> },
    /// Nothing to copy: every requested episode is already on the drive.
    AlreadySynced,
    /// `cancel` was set; partial files are cleaned up.
    Cancelled,
}

pub trait Backend: Send + Sync + 'static {
    /// Never fails: a missing or unparsable file gives `Settings::default()`.
    fn load_settings(&self) -> Settings;
    fn save_settings(&self, settings: &Settings) -> Result<(), String>;

    /// Downloaded episodes from the macOS Podcasts library, `on_drive` and `selected` false.
    fn load_mac_episodes(&self) -> Result<Vec<Episode>, String>;

    /// Mounted external volumes that can take podcasts.
    fn scan_drives(&self) -> Result<Vec<Drive>, String>;
    /// Episodes under the drive's `podcasts` folder.
    fn scan_drive(&self, drive: &Drive) -> Result<Vec<Episode>, String>;

    /// Plans and copies `episodes` to `drive` (tagging, filename sanitising, final cleanup),
    /// calling `progress` as it goes and stopping promptly once `cancel` is true.
    fn sync(
        &self,
        episodes: &[Episode],
        drive: &Drive,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(TransferProgress),
    ) -> Result<SyncOutcome, String>;

    /// Removes these drive-side files, then any podcast folders left empty.
    fn delete_from_drive(&self, episodes: &[Episode]) -> Result<(), String>;
}

/// Sets `on_drive` on each Mac episode that has an equivalent in `drive`.
pub fn mark_on_drive(mac: &mut [Episode], drive: &[Episode]) {
    crate::matching::mark_on_drive(mac, drive)
}
