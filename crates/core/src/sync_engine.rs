//! Planning and running a sync, and deleting from the drive.
//!
//! A sync is: build the drive index, plan what's missing, copy each file with chunked progress
//! (then ID3-tag it), then tidy the drive. Worth knowing:
//!
//! * a file that fails to copy is recorded and the sync carries on;
//! * files are copied to `<name>.castoff-part` and renamed once complete (and tagged), so a crash
//!   or a cancel can never leave a short file under a real name;
//! * the final cleanup removes hidden files and any show folders left empty.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::api::{Episode, SyncOutcome, TransferProgress};
use crate::drive_scan::{PODCASTS_DIR, build_drive_index};
use crate::format::{format_date_opt, sanitize_filename};
use crate::fsutil::{
    PARTIAL_SUFFIX, file_size, get_extension, is_system_hidden_file, remove_dir_if_empty,
};
use crate::matching::{DriveIndex, IndexEntry, MatchProbe, match_episode};
use crate::tagging::{PodcastTags, is_taggable, write_tags};

/// Copy buffer: 1 MiB.
pub const DEFAULT_CHUNK_SIZE: usize = 1024 * 1024;
/// Chunk-level progress is passed on at most this often.
pub const DEFAULT_PROGRESS_INTERVAL: Duration = Duration::from_millis(50);

/// Knobs for the copy loop (the defaults are right for the app; tests shrink them).
#[derive(Clone, Debug)]
pub struct ExecOptions {
    pub chunk_size: usize,
    pub progress_interval: Duration,
}

impl Default for ExecOptions {
    fn default() -> Self {
        ExecOptions {
            chunk_size: DEFAULT_CHUNK_SIZE,
            progress_interval: DEFAULT_PROGRESS_INTERVAL,
        }
    }
}

/// One file to copy.
#[derive(Clone, Debug, PartialEq)]
pub struct CopyItem {
    pub title: String,
    pub show_name: String,
    pub published: Option<i64>,
    pub source: PathBuf,
    pub dest: PathBuf,
    /// Source size at planning time.
    pub size: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SyncPlan {
    pub to_copy: Vec<CopyItem>,
    pub total_bytes: u64,
}

impl SyncPlan {
    pub fn total_files(&self) -> usize {
        self.to_copy.len()
    }
}

/// `<drive>/Podcasts/<Show_Name>/<YYYY-MM-DD> - <Episode_Title><ext>`. An unknown publication date
/// gives "Unknown" in place of the date.
pub fn format_dest_path(
    drive_root: &Path,
    show_name: &str,
    title: &str,
    published: Option<i64>,
    extension: &str,
) -> PathBuf {
    let file_name = format!(
        "{} - {}{}",
        format_date_opt(published),
        sanitize_filename(title),
        extension
    );
    drive_root
        .join(PODCASTS_DIR)
        .join(sanitize_filename(show_name))
        .join(file_name)
}

/// The index with only the paths kept, so a sync matches by expected path alone (never by size or
/// duration).
fn path_only(index: DriveIndex) -> DriveIndex {
    index
        .into_iter()
        .map(|(key, entry)| {
            (
                key,
                IndexEntry {
                    path: entry.path,
                    size: None,
                    duration: None,
                },
            )
        })
        .collect()
}

/// Works out what needs copying. Episodes are grouped by show (shows in order of first
/// appearance), then skipped if already flagged `on_drive`, without a source file, matched in the
/// drive index, or already present at their destination with content. Two episodes that map to the
/// same destination are copied once.
pub fn create_plan(episodes: &[Episode], drive_root: &Path, index: &DriveIndex) -> SyncPlan {
    let mut shows: Vec<&str> = Vec::new();
    for ep in episodes {
        if !shows.contains(&ep.show_name.as_str()) {
            shows.push(&ep.show_name);
        }
    }

    let mut plan = SyncPlan::default();
    let mut seen_dests: HashSet<PathBuf> = HashSet::new();
    for show in shows {
        for ep in episodes.iter().filter(|e| e.show_name == show) {
            if ep.on_drive || ep.file_path.as_os_str().is_empty() {
                continue;
            }
            if match_episode(show, MatchProbe::from(ep), index) {
                continue;
            }
            let ext = get_extension(&ep.file_path.to_string_lossy());
            let dest = format_dest_path(drive_root, show, &ep.title, ep.published, &ext);
            if file_size(&dest) > 0 || !seen_dests.insert(dest.clone()) {
                continue;
            }
            let size = file_size(&ep.file_path);
            plan.total_bytes += size;
            plan.to_copy.push(CopyItem {
                title: ep.title.clone(),
                show_name: show.to_string(),
                published: ep.published,
                source: ep.file_path.clone(),
                dest,
                size,
            });
        }
    }
    plan
}

/// Keeps the transfer state the UI sees: `files_done` is the index of the
/// file in progress (all of them once complete), `speed` is bytes since the copy began over the
/// seconds since it began.
struct Reporter<'a> {
    state: TransferProgress,
    started: Option<Instant>,
    sink: &'a mut dyn FnMut(TransferProgress),
}

impl<'a> Reporter<'a> {
    fn new(sink: &'a mut dyn FnMut(TransferProgress)) -> Self {
        Reporter {
            state: TransferProgress::default(),
            started: None,
            sink,
        }
    }

    fn emit(&mut self) {
        (self.sink)(self.state.clone());
    }

    fn preparing(&mut self) {
        self.state = TransferProgress {
            current_file: "Preparing...".into(),
            ..Default::default()
        };
        self.emit();
    }

    fn planned(&mut self, plan: &SyncPlan) {
        self.state.total_files = plan.total_files();
        self.state.total_bytes = plan.total_bytes;
        self.emit();
    }

    fn speed(&self) -> f64 {
        let Some(started) = self.started else {
            return 0.0;
        };
        let secs = started.elapsed().as_secs_f64();
        let speed = self.state.bytes_transferred as f64 / secs;
        if speed.is_finite() { speed } else { 0.0 }
    }

    fn start_clock(&mut self) {
        self.started = Some(Instant::now());
    }

    fn working_on(&mut self, label: String, index: usize, bytes: u64) {
        self.state.current_file = label;
        self.state.files_done = index;
        self.state.bytes_transferred = bytes;
        self.state.speed = self.speed();
        self.emit();
    }

    fn complete(&mut self) {
        self.state.current_file = String::new();
        self.state.files_done = self.state.total_files;
        self.state.bytes_transferred = self.state.total_bytes;
        self.state.speed = self.speed();
        self.emit();
    }

    fn finalizing(&mut self) {
        self.state.current_file = "Finalizing drive...".into();
        self.emit();
    }
}

fn partial_path(dest: &Path) -> PathBuf {
    let mut name = dest.as_os_str().to_owned();
    name.push(PARTIAL_SUFFIX);
    PathBuf::from(name)
}

enum CopyEnd {
    Done,
    Cancelled,
}

/// Copies one item to its `.castoff-part` file in `chunk_size` pieces, syncing each to disk (so
/// progress means bytes on the drive) and checking `cancel` before every chunk. On cancel or error
/// the partial file is removed.
fn copy_to_partial(
    item: &CopyItem,
    partial: &Path,
    cancel: &AtomicBool,
    opts: &ExecOptions,
    bytes_done: &mut u64,
    on_progress: &mut dyn FnMut(u64),
) -> std::io::Result<CopyEnd> {
    let mut src = File::open(&item.source)?;
    let mut out = File::create(partial)?;
    let mut buf = vec![0u8; opts.chunk_size.max(1)];
    let mut last_emit: Option<Instant> = None;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(CopyEnd::Cancelled);
        }
        let n = match src.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        out.write_all(&buf[..n])?;
        out.sync_data()?;
        *bytes_done += n as u64;
        if last_emit.is_none_or(|t| t.elapsed() >= opts.progress_interval) {
            last_emit = Some(Instant::now());
            on_progress(*bytes_done);
        }
    }
    out.flush()?;
    Ok(CopyEnd::Done)
}

enum RunEnd {
    Finished {
        copied: usize,
        failed: Vec<(String, String)>,
    },
    Cancelled,
}

fn execute_plan(
    plan: &SyncPlan,
    cancel: &AtomicBool,
    opts: &ExecOptions,
    reporter: &mut Reporter<'_>,
) -> RunEnd {
    reporter.start_clock();
    let mut bytes_done: u64 = 0;
    let mut planned_so_far: u64 = 0;
    let mut copied = 0;
    let mut failed: Vec<(String, String)> = Vec::new();

    for (index, item) in plan.to_copy.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return RunEnd::Cancelled;
        }
        planned_so_far += item.size;
        reporter.working_on(item.title.clone(), index, bytes_done);

        let partial = partial_path(&item.dest);
        let prepared = match item.dest.parent() {
            Some(dir) => fs::create_dir_all(dir),
            None => Ok(()),
        };
        let copy_result = prepared.and_then(|()| {
            let title = item.title.clone();
            copy_to_partial(
                item,
                &partial,
                cancel,
                opts,
                &mut bytes_done,
                &mut |bytes| {
                    reporter.working_on(title.clone(), index, bytes);
                },
            )
        });

        match copy_result {
            Ok(CopyEnd::Cancelled) => {
                let _ = fs::remove_file(&partial);
                return RunEnd::Cancelled;
            }
            Ok(CopyEnd::Done) => {
                reporter.working_on(format!("Tagging: {}", item.title), index, bytes_done);
                if is_taggable(&item.dest) {
                    // Tagging is best effort: an untagged episode is still a good episode.
                    let tags =
                        PodcastTags::for_episode(&item.title, "", &item.show_name, item.published);
                    let _ = write_tags(&partial, &tags);
                }
                match fs::rename(&partial, &item.dest) {
                    Ok(()) => copied += 1,
                    Err(e) => {
                        let _ = fs::remove_file(&partial);
                        failed.push((item.title.clone(), e.to_string()));
                    }
                }
            }
            Err(e) => {
                let _ = fs::remove_file(&partial);
                failed.push((item.title.clone(), e.to_string()));
                // Keep the byte counter in step with the plan after a short or skipped file.
                bytes_done = bytes_done.max(planned_so_far);
            }
        }
    }
    reporter.complete();
    RunEnd::Finished { copied, failed }
}

/// Tidies the drive after a sync: in each show folder, removes stale partial copies and system
/// hidden files, then removes the folder if nothing visible is left.
pub fn cleanup_drive(drive_root: &Path) {
    let Ok(entries) = fs::read_dir(drive_root.join(PODCASTS_DIR)) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if is_system_hidden_file(&name) {
            continue;
        }
        let show_dir = entry.path();
        if !show_dir.is_dir() {
            continue;
        }
        if let Ok(files) = fs::read_dir(&show_dir) {
            for file in files.filter_map(Result::ok) {
                if file.file_name().to_string_lossy().ends_with(PARTIAL_SUFFIX) {
                    let _ = fs::remove_file(file.path());
                }
            }
        }
        remove_dir_if_empty(&show_dir);
    }
}

/// A whole sync: index the drive, plan, copy, clean up.
///
/// `progress` sees `"Preparing..."`, then the plan's totals, then per-file / per-chunk / tagging
/// updates, a final "complete" update and `"Finalizing drive..."`. `cancel` is checked before
/// every chunk; cancelling removes the partial file, skips the cleanup, and keeps files already
/// finished.
pub fn run_sync(
    episodes: &[Episode],
    drive_root: &Path,
    cancel: &AtomicBool,
    opts: &ExecOptions,
    progress: &mut dyn FnMut(TransferProgress),
) -> Result<SyncOutcome, String> {
    if !drive_root.is_dir() {
        return Err(format!(
            "{} is not available. Is the drive still connected?",
            drive_root.display()
        ));
    }
    let mut reporter = Reporter::new(progress);
    reporter.preparing();

    let index = path_only(build_drive_index(drive_root));
    let plan = create_plan(episodes, drive_root, &index);
    if plan.to_copy.is_empty() {
        return Ok(SyncOutcome::AlreadySynced);
    }
    if cancel.load(Ordering::Relaxed) {
        return Ok(SyncOutcome::Cancelled);
    }
    reporter.planned(&plan);

    match execute_plan(&plan, cancel, opts, &mut reporter) {
        RunEnd::Cancelled => Ok(SyncOutcome::Cancelled),
        RunEnd::Finished { copied, failed } => {
            reporter.finalizing();
            cleanup_drive(drive_root);
            Ok(SyncOutcome::Complete { copied, failed })
        }
    }
}

/// A "Podcasts" folder that sits directly on a drive, which deleting episodes must not remove.
fn is_podcasts_root(dir: &Path) -> bool {
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned());
    name(dir).as_deref() == Some(PODCASTS_DIR)
        && dir.parent().and_then(name).as_deref() != Some(PODCASTS_DIR)
}

/// Deletes these drive-side files, then removes any parent folders left empty (after clearing
/// their system hidden files). Per-file failures are ignored.
///
/// This refuses paths that aren't audio files inside a folder named "Podcasts", so
/// a caller handing over Mac-side episodes by mistake can't delete the Podcasts library; those are
/// reported as an error after the legitimate deletions have been made.
pub fn delete_files(episodes: &[Episode]) -> Result<(), String> {
    let mut parents: Vec<PathBuf> = Vec::new();
    let mut refused: Vec<String> = Vec::new();

    for ep in episodes {
        let path = &ep.file_path;
        let in_podcasts_folder = path.components().any(|c| c.as_os_str() == PODCASTS_DIR);
        let is_audio = path
            .file_name()
            .is_some_and(|n| crate::fsutil::is_audio_file(&n.to_string_lossy()));
        if !in_podcasts_folder || !is_audio {
            refused.push(path.display().to_string());
            continue;
        }
        let is_file = fs::symlink_metadata(path)
            .map(|m| !m.is_dir())
            .unwrap_or(false);
        if is_file {
            let _ = fs::remove_file(path);
        }
        if let Some(parent) = path.parent()
            && !parents.iter().any(|p| p == parent)
        {
            parents.push(parent.to_path_buf());
        }
    }

    for dir in parents {
        if !is_podcasts_root(&dir) {
            remove_dir_if_empty(&dir);
        }
    }

    if refused.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Refused to delete {} file(s) that are not audio files in a Podcasts folder: {}",
            refused.len(),
            refused.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::format_date;
    use chrono::{Local, TimeZone};
    use id3::{Tag, TagLike};
    use tempfile::TempDir;

    fn local_noon(y: i32, m: u32, d: u32) -> i64 {
        Local
            .with_ymd_and_hms(y, m, d, 12, 0, 0)
            .unwrap()
            .timestamp()
    }

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    /// A Mac-side episode backed by a real file of `size` bytes under `mac`.
    fn mac_episode(
        mac: &Path,
        show: &str,
        title: &str,
        day: u32,
        ext: &str,
        size: usize,
    ) -> Episode {
        let file = mac.join(format!(
            "{}-{}{ext}",
            show.replace(' ', "_"),
            title.replace(' ', "_")
        ));
        // Distinct, non-ID3 content so copies can be compared byte for byte.
        let bytes: Vec<u8> = (0..size).map(|i| (i % 251) as u8 ^ (day as u8)).collect();
        write(&file, &bytes);
        Episode {
            id: format!("{show}/{title}"),
            title: title.into(),
            show_name: show.into(),
            duration_secs: 100.0,
            published: Some(local_noon(2024, 1, day)),
            on_drive: false,
            file_path: file,
            file_size: size as u64,
            selected: false,
        }
    }

    fn tiny_chunks() -> ExecOptions {
        ExecOptions {
            chunk_size: 1000,
            progress_interval: Duration::ZERO,
        }
    }

    fn run(
        episodes: &[Episode],
        drive: &Path,
        cancel: &AtomicBool,
        opts: &ExecOptions,
    ) -> (Result<SyncOutcome, String>, Vec<TransferProgress>) {
        let mut events = Vec::new();
        let result = run_sync(episodes, drive, cancel, opts, &mut |p| events.push(p));
        (result, events)
    }

    fn files_under(dir: &Path) -> Vec<String> {
        fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
            let Ok(read) = fs::read_dir(dir) else { return };
            for e in read.filter_map(Result::ok) {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, base, out);
                } else {
                    out.push(p.strip_prefix(base).unwrap().to_string_lossy().into_owned());
                }
            }
        }
        let mut out = Vec::new();
        walk(dir, dir, &mut out);
        out.sort();
        out
    }

    // ---- formatDestPath ---------------------------------------------------------------------

    #[test]
    fn dest_path_has_a_date_prefix() {
        let result = format_dest_path(
            Path::new("/Volumes/USB"),
            "My Podcast",
            "Episode Title",
            Some(local_noon(2024, 3, 15)),
            ".mp3",
        );
        assert_eq!(
            result,
            PathBuf::from("/Volumes/USB/Podcasts/My_Podcast/2024-03-15 - Episode_Title.mp3")
        );
    }

    #[test]
    fn dest_path_sanitises_show_and_title() {
        let result = format_dest_path(
            Path::new("/Volumes/USB"),
            "Show: With Special/Characters?",
            "Episode: The *Best* One!",
            Some(local_noon(2024, 1, 1)),
            ".mp3",
        )
        .to_string_lossy()
        .into_owned();
        assert!(
            result.contains("Show-_With_Special-Characters-"),
            "{result}"
        );
        assert!(result.contains("Episode-_The_-Best-_One!"), "{result}");
    }

    #[test]
    fn dest_path_keeps_the_extension() {
        let d = Some(local_noon(2024, 1, 1));
        let mp3 = format_dest_path(Path::new("/V"), "Show", "Ep", d, ".mp3");
        let m4a = format_dest_path(Path::new("/V"), "Show", "Ep", d, ".m4a");
        assert!(mp3.to_string_lossy().ends_with(".mp3"));
        assert!(m4a.to_string_lossy().ends_with(".m4a"));
    }

    #[test]
    fn dest_path_with_unknown_date_says_unknown() {
        let p = format_dest_path(Path::new("/V"), "Show", "Ep", None, ".mp3");
        assert_eq!(p, PathBuf::from("/V/Podcasts/Show/Unknown - Ep.mp3"));
    }

    // ---- createPlan -------------------------------------------------------------------------

    fn three_show_episodes(mac: &Path) -> Vec<Episode> {
        let mut eps = vec![
            mac_episode(
                mac,
                "Tech Talk",
                "Episode 1: Getting Started",
                15,
                ".mp3",
                5000,
            ),
            mac_episode(mac, "Tech Talk", "Episode 2: Deep Dive", 22, ".mp3", 100),
            mac_episode(mac, "Tech Talk", "Episode 3: Q&A Session", 29, ".mp3", 3000),
            mac_episode(
                mac,
                "Science Daily",
                "The Universe Explained",
                31,
                ".mp3",
                7000,
            ),
        ];
        eps[1].on_drive = true; // already synced: must be skipped
        eps
    }

    #[test]
    fn plan_has_unsynced_episodes_only() {
        let mac = TempDir::new().unwrap();
        let drive = TempDir::new().unwrap();
        let plan = create_plan(
            &three_show_episodes(mac.path()),
            drive.path(),
            &DriveIndex::new(),
        );
        let titles: Vec<&str> = plan.to_copy.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "Episode 1: Getting Started",
                "Episode 3: Q&A Session",
                "The Universe Explained"
            ]
        );
        assert_eq!(plan.total_files(), 3);
        assert_eq!(plan.total_bytes, 15_000);
    }

    #[test]
    fn plan_destinations_carry_show_folder_and_date() {
        let mac = TempDir::new().unwrap();
        let drive = TempDir::new().unwrap();
        let plan = create_plan(
            &three_show_episodes(mac.path()),
            drive.path(),
            &DriveIndex::new(),
        );
        let ep1 = &plan.to_copy[0];
        assert_eq!(
            ep1.dest,
            drive
                .path()
                .join("Podcasts/Tech_Talk/2024-01-15 - Episode_1-_Getting_Started.mp3")
        );
        assert_eq!(
            ep1.source,
            mac.path().join("Tech_Talk-Episode_1:_Getting_Started.mp3")
        );
        assert_eq!(ep1.size, 5000);
        assert_eq!(ep1.show_name, "Tech Talk");
    }

    #[test]
    fn plan_skips_episodes_matched_in_the_index() {
        let mac = TempDir::new().unwrap();
        let drive = TempDir::new().unwrap();
        let mut index = DriveIndex::new();
        index.insert(
            "Tech_Talk/Episode_1-_Getting_Started.mp3".into(),
            IndexEntry {
                path: "/Volumes/USB/x.mp3".into(),
                size: None,
                duration: None,
            },
        );
        let plan = create_plan(&three_show_episodes(mac.path()), drive.path(), &index);
        // Episode 1 on the drive, Episode 2 flagged: two left.
        assert_eq!(plan.total_files(), 2);
        assert!(
            plan.to_copy
                .iter()
                .all(|i| i.title != "Episode 1: Getting Started")
        );
    }

    #[test]
    fn plan_for_nothing_is_empty() {
        let drive = TempDir::new().unwrap();
        let plan = create_plan(&[], drive.path(), &DriveIndex::new());
        assert!(plan.to_copy.is_empty());
        assert_eq!((plan.total_files(), plan.total_bytes), (0, 0));
    }

    #[test]
    fn plan_skips_destinations_that_already_have_content_but_not_empty_ones() {
        let mac = TempDir::new().unwrap();
        let drive = TempDir::new().unwrap();
        let eps = vec![
            mac_episode(mac.path(), "Show", "Has content", 1, ".mp3", 10),
            mac_episode(mac.path(), "Show", "Is empty", 2, ".mp3", 10),
        ];
        write(
            &drive
                .path()
                .join("Podcasts/Show/2024-01-01 - Has_content.mp3"),
            b"data",
        );
        write(
            &drive.path().join("Podcasts/Show/2024-01-02 - Is_empty.mp3"),
            b"",
        );
        let plan = create_plan(&eps, drive.path(), &DriveIndex::new());
        assert_eq!(plan.to_copy.len(), 1);
        assert_eq!(plan.to_copy[0].title, "Is empty");
    }

    #[test]
    fn plan_ignores_episodes_without_a_source_path_and_dedupes_destinations() {
        let mac = TempDir::new().unwrap();
        let drive = TempDir::new().unwrap();
        let a = mac_episode(mac.path(), "Show", "Same", 1, ".mp3", 10);
        let mut b = a.clone();
        b.id = "other".into();
        let mut blank = a.clone();
        blank.file_path = PathBuf::new();
        blank.title = "Blank".into();
        let plan = create_plan(&[a, b, blank], drive.path(), &DriveIndex::new());
        assert_eq!(plan.to_copy.len(), 1);
        assert_eq!(plan.total_bytes, 10);
    }

    #[test]
    fn plan_groups_by_show_in_first_appearance_order() {
        let mac = TempDir::new().unwrap();
        let drive = TempDir::new().unwrap();
        let eps = vec![
            mac_episode(mac.path(), "B", "b1", 1, ".mp3", 1),
            mac_episode(mac.path(), "A", "a1", 2, ".mp3", 1),
            mac_episode(mac.path(), "B", "b2", 3, ".mp3", 1),
        ];
        let plan = create_plan(&eps, drive.path(), &DriveIndex::new());
        let order: Vec<&str> = plan.to_copy.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(order, ["b1", "b2", "a1"]);
    }

    #[test]
    fn plan_keeps_each_files_extension_and_uses_mp3_when_missing() {
        let mac = TempDir::new().unwrap();
        let drive = TempDir::new().unwrap();
        let m4a = mac_episode(mac.path(), "Show", "A", 1, ".m4a", 1);
        let bare = mac_episode(mac.path(), "Show", "B", 2, "", 1);
        let plan = create_plan(&[m4a, bare], drive.path(), &DriveIndex::new());
        assert!(plan.to_copy[0].dest.to_string_lossy().ends_with("A.m4a"));
        assert!(plan.to_copy[1].dest.to_string_lossy().ends_with("B.mp3"));
    }

    // ---- running a sync ---------------------------------------------------------------------

    #[test]
    fn syncs_files_with_progress_in_the_tuis_sequence() {
        let mac = TempDir::new().unwrap();
        let drive = TempDir::new().unwrap();
        let eps = vec![
            mac_episode(mac.path(), "Tech Talk", "Ep One", 15, ".m4a", 2500),
            mac_episode(mac.path(), "Science", "Ep Two", 16, ".m4a", 1000),
        ];
        let cancel = AtomicBool::new(false);
        let (result, events) = run(&eps, drive.path(), &cancel, &tiny_chunks());

        assert_eq!(
            result.unwrap(),
            SyncOutcome::Complete {
                copied: 2,
                failed: vec![]
            }
        );

        // Contents are identical (.m4a so no tag is added) and land where planned.
        let dest1 = drive
            .path()
            .join("Podcasts/Tech_Talk/2024-01-15 - Ep_One.m4a");
        let dest2 = drive
            .path()
            .join("Podcasts/Science/2024-01-16 - Ep_Two.m4a");
        assert_eq!(
            fs::read(&dest1).unwrap(),
            fs::read(&eps[0].file_path).unwrap()
        );
        assert_eq!(
            fs::read(&dest2).unwrap(),
            fs::read(&eps[1].file_path).unwrap()
        );
        assert_eq!(
            files_under(drive.path()).len(),
            2,
            "no partial files left behind"
        );

        let first = &events[0];
        assert_eq!(first.current_file, "Preparing...");
        assert_eq!(
            (first.total_files, first.total_bytes, first.speed),
            (0, 0, 0.0)
        );

        let planned = &events[1];
        assert_eq!((planned.total_files, planned.total_bytes), (2, 3500));
        assert_eq!(planned.current_file, "Preparing...");

        // File 1: initial, 3 chunks (1000+1000+500), tagging; file 2: initial, 1 chunk, tagging.
        let labels: Vec<(&str, usize, u64)> = events[2..events.len() - 2]
            .iter()
            .map(|e| (e.current_file.as_str(), e.files_done, e.bytes_transferred))
            .collect();
        assert_eq!(
            labels,
            [
                ("Ep One", 0, 0),
                ("Ep One", 0, 1000),
                ("Ep One", 0, 2000),
                ("Ep One", 0, 2500),
                ("Tagging: Ep One", 0, 2500),
                ("Ep Two", 1, 2500),
                ("Ep Two", 1, 3500),
                ("Tagging: Ep Two", 1, 3500),
            ]
        );

        let complete = &events[events.len() - 2];
        assert_eq!(complete.current_file, "");
        assert_eq!(complete.files_done, 2);
        assert_eq!(
            (complete.bytes_transferred, complete.total_bytes),
            (3500, 3500)
        );

        let finalizing = events.last().unwrap();
        assert_eq!(finalizing.current_file, "Finalizing drive...");
        assert_eq!(finalizing.files_done, 2);
        assert_eq!(finalizing.bytes_transferred, 3500);

        assert!(events.iter().all(|e| e.speed.is_finite() && e.speed >= 0.0));
    }

    #[test]
    fn speed_is_bytes_over_seconds_since_the_copy_began() {
        let mac = TempDir::new().unwrap();
        let drive = TempDir::new().unwrap();
        let eps = vec![mac_episode(mac.path(), "Show", "Slow", 1, ".m4a", 4000)];
        let cancel = AtomicBool::new(false);
        let mut events = Vec::new();
        run_sync(&eps, drive.path(), &cancel, &tiny_chunks(), &mut |p| {
            // Make the copy take measurable time: every callback costs 20ms.
            std::thread::sleep(Duration::from_millis(20));
            events.push(p);
        })
        .unwrap();

        // Before the copy begins there is no speed.
        assert_eq!(events[0].speed, 0.0);
        assert_eq!(events[1].speed, 0.0);
        // The second chunk (2000 bytes) is reported after two earlier 20ms callbacks, so the rate
        // is positive and at most 2000 bytes / 40ms.
        let chunk = events.iter().find(|p| p.bytes_transferred == 2000).unwrap();
        assert!(
            chunk.speed > 0.0 && chunk.speed <= 2000.0 / 0.04,
            "{}",
            chunk.speed
        );
        // All 4000 bytes after at least six callbacks: at most 4000 bytes / 120ms.
        let complete = events.iter().find(|p| p.current_file.is_empty()).unwrap();
        assert!(
            complete.speed > 0.0 && complete.speed <= 4000.0 / 0.12,
            "{}",
            complete.speed
        );
    }

    #[test]
    fn mp3_copies_are_tagged_and_audio_is_preserved() {
        let mac = TempDir::new().unwrap();
        let drive = TempDir::new().unwrap();
        let ep = mac_episode(mac.path(), "Tech Talk", "Ep 1", 15, ".mp3", 3000);
        let original = fs::read(&ep.file_path).unwrap();
        let cancel = AtomicBool::new(false);
        let (result, _) = run(
            std::slice::from_ref(&ep),
            drive.path(),
            &cancel,
            &tiny_chunks(),
        );
        assert_eq!(
            result.unwrap(),
            SyncOutcome::Complete {
                copied: 1,
                failed: vec![]
            }
        );

        let dest = drive
            .path()
            .join("Podcasts/Tech_Talk/2024-01-15 - Ep_1.mp3");
        let tag = Tag::read_from_path(&dest).unwrap();
        assert_eq!(tag.title(), Some("Ep 1"));
        assert_eq!(tag.album(), Some("Tech Talk"));
        assert_eq!(tag.genre(), Some("Podcast"));
        assert_eq!(tag.year(), Some(2024));
        assert!(
            tag.comments()
                .next()
                .unwrap()
                .text
                .starts_with("Published: 2024-01-1")
        );
        let bytes = fs::read(&dest).unwrap();
        assert!(
            bytes.ends_with(&original),
            "audio payload preserved after the tag"
        );
        // The Mac original is untouched.
        assert_eq!(fs::read(&ep.file_path).unwrap(), original);
    }

    #[test]
    fn non_mp3_files_are_not_tagged() {
        let mac = TempDir::new().unwrap();
        let drive = TempDir::new().unwrap();
        let ep = mac_episode(mac.path(), "Show", "Ep", 15, ".m4a", 500);
        let cancel = AtomicBool::new(false);
        run(
            std::slice::from_ref(&ep),
            drive.path(),
            &cancel,
            &tiny_chunks(),
        )
        .0
        .unwrap();
        let copied = fs::read(drive.path().join("Podcasts/Show/2024-01-15 - Ep.m4a")).unwrap();
        assert_eq!(copied, fs::read(&ep.file_path).unwrap());
    }

    #[test]
    fn already_synced_when_everything_is_on_the_drive() {
        let mac = TempDir::new().unwrap();
        let drive = TempDir::new().unwrap();
        let ep = mac_episode(mac.path(), "Tech Talk", "Ep 1", 15, ".mp3", 100);
        // Already on the drive under a different date prefix: the path-based match catches it.
        write(
            &drive
                .path()
                .join("Podcasts/Tech_Talk/2023-05-05 - Ep_1.mp3"),
            b"old",
        );
        let cancel = AtomicBool::new(false);
        let (result, events) = run(&[ep], drive.path(), &cancel, &tiny_chunks());
        assert_eq!(result.unwrap(), SyncOutcome::AlreadySynced);
        assert_eq!(events.len(), 1, "only the Preparing... update");
        assert_eq!(
            files_under(drive.path()),
            ["Podcasts/Tech_Talk/2023-05-05 - Ep_1.mp3"]
        );
    }

    #[test]
    fn syncing_nothing_is_already_synced() {
        let drive = TempDir::new().unwrap();
        let cancel = AtomicBool::new(false);
        assert_eq!(
            run(&[], drive.path(), &cancel, &tiny_chunks()).0.unwrap(),
            SyncOutcome::AlreadySynced
        );
    }

    #[test]
    fn a_sync_matches_by_path_only_not_by_size() {
        // A different episode with the same size sits in the same show on the drive. A sync
        // drops sizes from the index, so it is still copied.
        let mac = TempDir::new().unwrap();
        let drive = TempDir::new().unwrap();
        let ep = mac_episode(mac.path(), "Show", "New one", 15, ".m4a", 300);
        write(
            &drive.path().join("Podcasts/Show/2024-01-01 - Other.m4a"),
            &[0u8; 300],
        );
        let cancel = AtomicBool::new(false);
        let (result, _) = run(&[ep], drive.path(), &cancel, &tiny_chunks());
        assert_eq!(
            result.unwrap(),
            SyncOutcome::Complete {
                copied: 1,
                failed: vec![]
            }
        );
    }

    #[test]
    fn running_twice_copies_nothing_the_second_time() {
        let mac = TempDir::new().unwrap();
        let drive = TempDir::new().unwrap();
        let eps = vec![mac_episode(mac.path(), "Show", "Ep", 15, ".mp3", 500)];
        let cancel = AtomicBool::new(false);
        assert!(matches!(
            run(&eps, drive.path(), &cancel, &tiny_chunks()).0.unwrap(),
            SyncOutcome::Complete { copied: 1, .. }
        ));
        assert_eq!(
            run(&eps, drive.path(), &cancel, &tiny_chunks()).0.unwrap(),
            SyncOutcome::AlreadySynced
        );
    }

    #[test]
    fn a_missing_drive_is_an_error_and_writes_nothing() {
        let mac = TempDir::new().unwrap();
        let eps = vec![mac_episode(mac.path(), "Show", "Ep", 15, ".mp3", 10)];
        let gone = mac.path().join("not-mounted");
        let cancel = AtomicBool::new(false);
        let (result, events) = run(&eps, &gone, &cancel, &tiny_chunks());
        assert!(result.unwrap_err().contains("not available"));
        assert!(events.is_empty());
        assert!(!gone.exists());
    }

    // ---- failures ---------------------------------------------------------------------------

    #[test]
    fn a_failing_file_is_reported_and_the_rest_still_copy() {
        let mac = TempDir::new().unwrap();
        let drive = TempDir::new().unwrap();
        let good1 = mac_episode(mac.path(), "Show", "Good one", 1, ".m4a", 1500);
        let mut bad = mac_episode(mac.path(), "Show", "Vanished", 2, ".m4a", 1000);
        fs::remove_file(&bad.file_path).unwrap(); // deleted between listing and syncing
        bad.file_size = 1000;
        let good2 = mac_episode(mac.path(), "Show", "Good two", 3, ".m4a", 700);

        let cancel = AtomicBool::new(false);
        let (result, events) = run(&[good1, bad, good2], drive.path(), &cancel, &tiny_chunks());
        match result.unwrap() {
            SyncOutcome::Complete { copied, failed } => {
                assert_eq!(copied, 2);
                assert_eq!(failed.len(), 1);
                assert_eq!(failed[0].0, "Vanished");
                assert!(!failed[0].1.is_empty());
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            files_under(drive.path()),
            [
                "Podcasts/Show/2024-01-01 - Good_one.m4a",
                "Podcasts/Show/2024-01-03 - Good_two.m4a"
            ]
        );
        // The last event still reaches the totals.
        let complete = events.iter().find(|e| e.current_file.is_empty()).unwrap();
        assert_eq!(complete.bytes_transferred, complete.total_bytes);
    }

    #[test]
    fn a_destination_that_cannot_be_created_is_a_per_file_failure() {
        let mac = TempDir::new().unwrap();
        let drive = TempDir::new().unwrap();
        // "Podcasts" is a file, so nothing can be created beneath it.
        write(&drive.path().join("Podcasts"), b"in the way");
        let eps = vec![mac_episode(mac.path(), "Show", "Ep", 1, ".m4a", 10)];
        let cancel = AtomicBool::new(false);
        let (result, _) = run(&eps, drive.path(), &cancel, &tiny_chunks());
        match result.unwrap() {
            SyncOutcome::Complete { copied: 0, failed } => assert_eq!(failed.len(), 1),
            other => panic!("unexpected {other:?}"),
        }
    }

    // ---- cancellation -----------------------------------------------------------------------

    #[test]
    fn cancelling_mid_copy_stops_promptly_and_removes_the_partial_file() {
        let mac = TempDir::new().unwrap();
        let drive = TempDir::new().unwrap();
        let first = mac_episode(mac.path(), "Show", "First", 1, ".m4a", 3000);
        let big = mac_episode(mac.path(), "Show", "Big", 2, ".m4a", 100_000);
        let third = mac_episode(mac.path(), "Show", "Third", 3, ".m4a", 3000);

        let cancel = AtomicBool::new(false);
        let mut events = Vec::new();
        let mut chunks_seen_of_big = 0;
        let result = run_sync(
            &[first, big, third],
            drive.path(),
            &cancel,
            &tiny_chunks(),
            &mut |p| {
                if p.current_file == "Big" && p.bytes_transferred > 3000 {
                    chunks_seen_of_big += 1;
                    if chunks_seen_of_big == 3 {
                        cancel.store(true, Ordering::SeqCst);
                    }
                }
                events.push(p);
            },
        );

        assert_eq!(result.unwrap(), SyncOutcome::Cancelled);
        // Stopped within a chunk of the flag: 3000 + 3 chunks, nowhere near the 100 000 bytes.
        let max_bytes = events.iter().map(|e| e.bytes_transferred).max().unwrap();
        assert!(
            max_bytes <= 3000 + 3 * 1000,
            "copied on to {max_bytes} after cancel"
        );
        // First file finished and stays; the partial of the second is gone; the third never began.
        assert_eq!(
            files_under(drive.path()),
            ["Podcasts/Show/2024-01-01 - First.m4a"]
        );
        assert!(events.iter().all(|e| e.current_file != "Third"));
        assert!(
            events
                .iter()
                .all(|e| e.current_file != "Finalizing drive..."),
            "no cleanup step after a cancel"
        );
    }

    #[test]
    fn cancelling_before_the_copy_starts_writes_nothing() {
        let mac = TempDir::new().unwrap();
        let drive = TempDir::new().unwrap();
        let eps = vec![mac_episode(mac.path(), "Show", "Ep", 1, ".m4a", 5000)];
        let cancel = AtomicBool::new(true);
        let (result, events) = run(&eps, drive.path(), &cancel, &tiny_chunks());
        assert_eq!(result.unwrap(), SyncOutcome::Cancelled);
        assert!(files_under(drive.path()).is_empty());
        assert!(events.iter().all(|e| e.bytes_transferred == 0));
    }

    #[test]
    fn cancelling_during_the_very_first_chunk_leaves_no_files_or_folders_with_content() {
        let mac = TempDir::new().unwrap();
        let drive = TempDir::new().unwrap();
        let eps = vec![mac_episode(mac.path(), "Show", "Ep", 1, ".m4a", 50_000)];
        let cancel = AtomicBool::new(false);
        let result = run_sync(&eps, drive.path(), &cancel, &tiny_chunks(), &mut |p| {
            if p.current_file == "Ep" && p.bytes_transferred == 1000 {
                cancel.store(true, Ordering::SeqCst);
            }
        });
        assert_eq!(result.unwrap(), SyncOutcome::Cancelled);
        assert!(
            files_under(drive.path()).is_empty(),
            "{:?}",
            files_under(drive.path())
        );
    }

    #[test]
    fn a_cancelled_sync_can_be_resumed() {
        let mac = TempDir::new().unwrap();
        let drive = TempDir::new().unwrap();
        let eps = vec![
            mac_episode(mac.path(), "Show", "One", 1, ".m4a", 4000),
            mac_episode(mac.path(), "Show", "Two", 2, ".m4a", 4000),
        ];
        let cancel = AtomicBool::new(false);
        let _ = run_sync(&eps, drive.path(), &cancel, &tiny_chunks(), &mut |p| {
            if p.current_file == "Two" && p.bytes_transferred > 4000 {
                cancel.store(true, Ordering::SeqCst);
            }
        });
        assert_eq!(
            files_under(drive.path()),
            ["Podcasts/Show/2024-01-01 - One.m4a"]
        );

        let cancel = AtomicBool::new(false);
        let (result, _) = run(&eps, drive.path(), &cancel, &tiny_chunks());
        assert_eq!(
            result.unwrap(),
            SyncOutcome::Complete {
                copied: 1,
                failed: vec![]
            }
        );
        assert_eq!(files_under(drive.path()).len(), 2);
    }

    // ---- cleanup ----------------------------------------------------------------------------

    #[test]
    fn cleanup_removes_hidden_files_stale_partials_and_empty_show_folders() {
        let drive = TempDir::new().unwrap();
        let root = drive.path().join("Podcasts");
        write(&root.join("Keeps/ep.mp3"), b"x");
        write(&root.join("Keeps/.DS_Store"), b"");
        write(&root.join("Keeps/._ep.mp3"), b"");
        write(&root.join("OnlyHidden/.DS_Store"), b"");
        write(&root.join("OnlyHidden/._junk"), b"");
        write(
            &root.join("OnlyPartial/2024-01-01 - Ep.mp3.castoff-part"),
            b"half",
        );
        fs::create_dir_all(root.join("Empty")).unwrap();
        write(&root.join(".DS_Store"), b"");
        write(&root.join("loose.mp3"), b"x");

        cleanup_drive(drive.path());

        assert_eq!(
            files_under(drive.path()),
            [
                "Podcasts/.DS_Store",
                "Podcasts/Keeps/ep.mp3",
                "Podcasts/loose.mp3"
            ]
        );
        assert!(!root.join("OnlyHidden").exists());
        assert!(!root.join("OnlyPartial").exists());
        assert!(!root.join("Empty").exists());
        assert!(root.exists());
    }

    #[test]
    fn cleanup_without_a_podcasts_folder_does_nothing() {
        let drive = TempDir::new().unwrap();
        cleanup_drive(drive.path());
        assert!(files_under(drive.path()).is_empty());
    }

    #[test]
    fn a_finished_sync_tidies_the_drive() {
        let mac = TempDir::new().unwrap();
        let drive = TempDir::new().unwrap();
        write(&drive.path().join("Podcasts/Old_Show/.DS_Store"), b"");
        write(&drive.path().join("Podcasts/New_Show/._ep"), b"");
        let eps = vec![mac_episode(mac.path(), "New Show", "Ep", 1, ".m4a", 10)];
        let cancel = AtomicBool::new(false);
        run(&eps, drive.path(), &cancel, &tiny_chunks()).0.unwrap();
        assert!(!drive.path().join("Podcasts/Old_Show").exists());
        assert_eq!(
            files_under(drive.path()),
            ["Podcasts/New_Show/2024-01-01 - Ep.m4a"]
        );
    }

    // ---- delete -----------------------------------------------------------------------------

    fn drive_episode(path: &Path) -> Episode {
        Episode {
            id: path.display().to_string(),
            title: "t".into(),
            show_name: "s".into(),
            duration_secs: 0.0,
            published: None,
            on_drive: true,
            file_path: path.to_path_buf(),
            file_size: 1,
            selected: true,
        }
    }

    #[test]
    fn delete_removes_files_and_emptied_folders_only() {
        let drive = TempDir::new().unwrap();
        let root = drive.path().join("Podcasts");
        let a1 = root.join("Show_A/2024-01-01 - a1.mp3");
        let a2 = root.join("Show_A/2024-01-02 - a2.mp3");
        let b1 = root.join("Show_B/2024-01-01 - b1.mp3");
        let c1 = root.join("Show_C/2024-01-01 - c1.mp3");
        for p in [&a1, &a2, &b1, &c1] {
            write(p, b"x");
        }
        write(&root.join("Show_B/.DS_Store"), b"");
        write(&root.join("Show_B/._b1.mp3"), b"");

        delete_files(&[drive_episode(&a1), drive_episode(&b1)]).unwrap();

        assert_eq!(
            files_under(drive.path()),
            [
                "Podcasts/Show_A/2024-01-02 - a2.mp3",
                "Podcasts/Show_C/2024-01-01 - c1.mp3"
            ]
        );
        assert!(
            !root.join("Show_B").exists(),
            "emptied folder (hidden files only) is removed"
        );
        assert!(root.join("Show_A").exists());
    }

    #[test]
    fn delete_tolerates_missing_files() {
        let drive = TempDir::new().unwrap();
        let root = drive.path().join("Podcasts");
        let real = root.join("Show/2024-01-01 - real.mp3");
        write(&real, b"x");
        let ghost = root.join("Show/2024-01-01 - ghost.mp3");
        delete_files(&[drive_episode(&ghost), drive_episode(&real)]).unwrap();
        assert!(files_under(drive.path()).is_empty());
        assert!(root.exists(), "the Podcasts folder itself stays");
    }

    #[test]
    fn delete_never_removes_the_podcasts_folder_itself() {
        let drive = TempDir::new().unwrap();
        let loose = drive.path().join("Podcasts/loose.mp3");
        write(&loose, b"x");
        delete_files(&[drive_episode(&loose)]).unwrap();
        assert!(drive.path().join("Podcasts").is_dir());
    }

    #[test]
    fn delete_refuses_paths_outside_a_podcasts_folder_or_that_are_not_audio() {
        let dir = TempDir::new().unwrap();
        let mac_file = dir.path().join("Library/Cache/ep.mp3");
        let not_audio = dir.path().join("Podcasts/Show/notes.txt");
        let good = dir.path().join("Podcasts/Show/ep.mp3");
        for p in [&mac_file, &not_audio, &good] {
            write(p, b"x");
        }
        let err = delete_files(&[
            drive_episode(&mac_file),
            drive_episode(&not_audio),
            drive_episode(&good),
        ])
        .unwrap_err();
        assert!(err.contains("Refused to delete 2"), "{err}");
        assert!(mac_file.exists() && not_audio.exists());
        assert!(!good.exists(), "legitimate deletions still happen");
    }

    #[test]
    fn delete_does_not_follow_a_directory_given_as_a_file() {
        let dir = TempDir::new().unwrap();
        let folder = dir.path().join("Podcasts/Show/ep.mp3"); // a directory named like a file
        write(&folder.join("inner.txt"), b"x");
        delete_files(&[drive_episode(&folder)]).unwrap();
        assert!(folder.join("inner.txt").exists());
    }

    #[test]
    fn format_date_agrees_with_the_planned_destination() {
        // The date in a destination name is the local date shown by format_date.
        let ts = local_noon(2024, 12, 31);
        let dest = format_dest_path(Path::new("/V"), "S", "T", Some(ts), ".mp3");
        assert!(dest.to_string_lossy().contains(&format_date(ts)));
    }
}
