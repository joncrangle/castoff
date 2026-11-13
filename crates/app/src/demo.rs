//! A `Backend` with made-up but plausible data: a few dozen episodes across several shows, three
//! drives, a sync that copies over a couple of seconds and honours `cancel`, and settings and
//! deletes that live in memory. `--demo` (or `CASTOFF_DEMO=1`) runs the app on it, and every
//! headless scene and test does, so nothing touches the real disk or settings file.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::Duration;

use castoff_core::{Backend, Drive, Episode, Settings, SyncOutcome, TransferProgress};

const MB: u64 = 1024 * 1024;
const GB: u64 = 1024 * MB;
/// 2026-01-23 09:00 UTC: the newest episode's date, so dates never depend on today.
const NEWEST: i64 = 1_769_158_800;
const DAY: i64 = 86_400;

/// (show, [(title, minutes)]), newest first within each show.
const SHOWS: &[(&str, &[(&str, u32)])] = &[
    ("The Long Signal", &[
        ("Ep. 212 — The Radio Telescope in the Desert", 71),
        ("Ep. 211 — Listening for Pulsars", 64),
        ("Ep. 210 — Why the Night Sky Is Dark", 58),
        ("Ep. 209 — A Short History of Static", 66),
        ("Ep. 208 — The Wow! Signal, Revisited", 75),
        ("Ep. 207 — Antennas All the Way Down", 52),
    ]),
    ("Tiny Histories", &[
        ("The Paperclip", 24),
        ("The Shipping Container", 31),
        ("The Pencil Eraser", 22),
        ("The Bicycle Bell", 19),
        ("The Zip Fastener", 27),
        ("The Teabag", 21),
        ("The Safety Pin", 23),
    ]),
    ("Kitchen Table Science", &[
        ("Why Bread Rises (and Sometimes Doesn’t)", 46),
        ("The Physics of a Perfect Pancake", 41),
        ("Salt, Ice and the Freezing Point", 38),
        ("Caramel Is Just Controlled Burning", 44),
        ("What Your Kettle Knows About Altitude", 36),
    ]),
    ("Field Notes with Ada Okafor", &[
        ("142: Mapping Wetlands from a Canoe", 89),
        ("141: The Return of the Beavers", 94),
        ("140: Counting Moths at Midnight", 82),
        ("139: How Lichens Tell Time", 77),
        ("138: A Year in One Hedgerow", 101),
        ("137: Rewilding a Parking Lot", 85),
    ]),
    ("The Margin", &[
        ("Interest Rates, Explained with Pizza", 33),
        ("Who Actually Sets the Price of Coffee?", 29),
        ("The Strange Economics of Concert Tickets", 35),
        ("Inflation’s Long Tail", 31),
        ("Why Everything Is a Subscription Now", 28),
    ]),
    ("Night Shift Radio", &[
        ("Live from the 3 a.m. Diner", 118),
        ("Truckers, Bakers and Lighthouse Keepers", 124),
        ("The Night Nurse Special", 109),
        ("Insomnia Requests Hour", 96),
    ]),
    ("Build Log", &[
        ("#58 Shipping a Desktop App in a Weekend", 54),
        ("#57 The Case for Boring Technology", 61),
        ("#56 Debugging by Bisecting Your Assumptions", 49),
        ("#55 GPUs for People Who Draw Rectangles", 57),
    ]),
];

/// Which mac episodes (show index, episode index) start out on which drive.
const WALKMAN_HAS: &[(usize, usize)] = &[(0, 1), (0, 2), (1, 0), (1, 3), (2, 2), (3, 1), (4, 0), (5, 1), (6, 2)];
const ECHO_HAS: &[(usize, usize)] = &[(1, 1), (3, 0), (4, 3)];

#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// Before each load or scan returns.
    pub latency: Duration,
    /// One sync step; a sync takes about `sync_steps` of them, however many files.
    pub sync_step: Duration,
    pub sync_steps: u32,
}

impl Timing {
    /// No waiting at all: what headless runs use, so a scene or script never races a thread.
    pub const INSTANT: Timing = Timing { latency: Duration::ZERO, sync_step: Duration::ZERO, sync_steps: 24 };
    /// Headless recordings: no waiting, and enough steps that a sync paced at one a frame lasts
    /// about three seconds at 60 fps.
    pub const RECORDED: Timing = Timing { latency: Duration::ZERO, sync_step: Duration::ZERO, sync_steps: 180 };
    /// What a person sees with `--demo`: short loads and a sync of about two and a half seconds.
    pub const REALISTIC: Timing =
        Timing { latency: Duration::from_millis(450), sync_step: Duration::from_millis(30), sync_steps: 84 };
}

struct Data {
    settings: Settings,
    mac: Vec<Episode>,
    drives: Vec<Drive>,
    /// Drive id → its files.
    files: HashMap<String, Vec<Episode>>,
    unplugged: Vec<Drive>,
}

pub struct DemoBackend {
    data: Mutex<Data>,
    timing: Timing,
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if matches!(c, '/' | ':' | '\\' | '?' | '*' | '"' | '<' | '>' | '|') { '_' } else { c }).collect()
}

fn mac_episodes() -> Vec<Episode> {
    let mut out = Vec::new();
    let mut n = 0u64;
    for (si, (show, eps)) in SHOWS.iter().enumerate() {
        for (ei, (title, minutes)) in eps.iter().enumerate() {
            n += 1;
            let secs = f64::from(*minutes) * 60.0 + ((n * 37) % 60) as f64;
            // Roughly 128 kbps, with a little variety.
            let size = (secs as u64) * 16_000 + (n * 7_919 % 900) * 1_000;
            out.push(Episode {
                id: format!("mac-{si}-{ei}"),
                title: title.to_string(),
                show_name: show.to_string(),
                duration_secs: secs,
                published: Some(NEWEST - (si as i64 * 2 + ei as i64 * (5 + si as i64)) * DAY - (n as i64 % 7) * 3_600),
                on_drive: false,
                file_path: PathBuf::from(format!(
                    "/Users/demo/Library/Group Containers/243LU875E5.groups.com.apple.podcasts/Library/Cache/{si}{ei}-demo.mp3"
                )),
                file_size: size,
                selected: false,
            });
        }
    }
    // Newest first, as the Podcasts library lists them.
    out.sort_by_key(|e| std::cmp::Reverse(e.published));
    out
}

fn drive_copy(ep: &Episode, drive: &Drive) -> Episode {
    let path = drive.mount_point.join("podcasts").join(sanitize(&ep.show_name)).join(format!("{}.mp3", sanitize(&ep.title)));
    Episode {
        id: path.display().to_string(),
        title: ep.title.clone(),
        show_name: ep.show_name.clone(),
        duration_secs: ep.duration_secs,
        published: None,
        on_drive: false,
        file_path: path,
        file_size: ep.file_size,
        selected: false,
    }
}

pub fn demo_drives() -> Vec<Drive> {
    vec![
        Drive {
            id: "demo-walkman".into(),
            name: "WALKMAN".into(),
            bsd_name: "disk4s1".into(),
            mount_point: PathBuf::from("/Volumes/WALKMAN"),
            total_space: 64 * GB,
            free_space: Some(21 * GB + 410 * MB),
        },
        Drive {
            id: "demo-echo".into(),
            name: "ECHO MINI".into(),
            bsd_name: "disk5s1".into(),
            mount_point: PathBuf::from("/Volumes/ECHO MINI"),
            total_space: 32 * GB,
            free_space: Some(9 * GB + 120 * MB),
        },
        Drive {
            id: "demo-clip".into(),
            name: "SANDISK CLIP".into(),
            bsd_name: "disk6s1".into(),
            mount_point: PathBuf::from("/Volumes/SANDISK CLIP"),
            total_space: 16 * GB,
            free_space: None,
        },
    ]
}

impl DemoBackend {
    pub fn new(timing: Timing) -> Self {
        let mac = mac_episodes();
        let drives = demo_drives();
        let pick = |has: &[(usize, usize)], drive: &Drive| -> Vec<Episode> {
            has.iter()
                .filter_map(|(s, e)| mac.iter().find(|m| m.id == format!("mac-{s}-{e}")))
                .map(|m| drive_copy(m, drive))
                .collect()
        };
        let mut files = HashMap::new();
        files.insert(drives[0].id.clone(), pick(WALKMAN_HAS, &drives[0]));
        files.insert(drives[1].id.clone(), pick(ECHO_HAS, &drives[1]));
        files.insert(drives[2].id.clone(), Vec::new());
        DemoBackend {
            data: Mutex::new(Data { settings: Settings::default(), mac, drives, files, unplugged: Vec::new() }),
            timing,
        }
    }

    pub fn instant() -> Self {
        Self::new(Timing::INSTANT)
    }

    /// Settings as a test or scene wants them to load.
    pub fn with_settings(self, settings: Settings) -> Self {
        self.lock().settings = settings;
        self
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Data> {
        self.data.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn wait(&self) {
        if !self.timing.latency.is_zero() {
            sleep(self.timing.latency);
        }
    }

    /// Takes a drive out, as if it were ejected; `plug` puts it back.
    pub fn unplug(&self, id: &str) {
        let mut d = self.lock();
        if let Some(i) = d.drives.iter().position(|x| x.id == id) {
            let drive = d.drives.remove(i);
            d.unplugged.push(drive);
        }
    }

    #[cfg_attr(not(test), allow(dead_code, reason = "test hooks"))]
    pub fn plug(&self, id: &str) {
        let mut d = self.lock();
        if let Some(i) = d.unplugged.iter().position(|x| x.id == id) {
            let drive = d.unplugged.remove(i);
            d.drives.push(drive);
        }
    }

    #[cfg_attr(not(test), allow(dead_code, reason = "test hooks"))]
    pub fn saved_settings(&self) -> Settings {
        self.lock().settings.clone()
    }

    #[cfg_attr(not(test), allow(dead_code, reason = "test hooks"))]
    pub fn files_on(&self, drive_id: &str) -> Vec<Episode> {
        self.lock().files.get(drive_id).cloned().unwrap_or_default()
    }
}

impl Backend for DemoBackend {
    fn load_settings(&self) -> Settings {
        self.lock().settings.clone()
    }

    fn save_settings(&self, settings: &Settings) -> Result<(), String> {
        self.lock().settings = settings.clone();
        Ok(())
    }

    fn load_mac_episodes(&self) -> Result<Vec<Episode>, String> {
        self.wait();
        Ok(self.lock().mac.clone())
    }

    fn scan_drives(&self) -> Result<Vec<Drive>, String> {
        self.wait();
        Ok(self.lock().drives.clone())
    }

    fn scan_drive(&self, drive: &Drive) -> Result<Vec<Episode>, String> {
        self.wait();
        let d = self.lock();
        if !d.drives.iter().any(|x| x.id == drive.id) {
            return Err(format!("{} is not mounted", drive.name));
        }
        let mut files = d.files.get(&drive.id).cloned().unwrap_or_default();
        files.sort_by(|a, b| (&a.show_name, &a.title).cmp(&(&b.show_name, &b.title)));
        Ok(files)
    }

    fn sync(
        &self,
        episodes: &[Episode],
        drive: &Drive,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(TransferProgress),
    ) -> Result<SyncOutcome, String> {
        let todo: Vec<Episode> = {
            let d = self.lock();
            if !d.drives.iter().any(|x| x.id == drive.id) {
                return Err(format!("{} is not mounted", drive.name));
            }
            let have = d.files.get(&drive.id).cloned().unwrap_or_default();
            episodes
                .iter()
                .filter(|e| !have.iter().any(|h| h.show_name == e.show_name && h.title == e.title))
                .cloned()
                .collect()
        };
        if todo.is_empty() {
            return Ok(SyncOutcome::AlreadySynced);
        }
        let total_bytes: u64 = todo.iter().map(|e| e.file_size).sum();
        let total_files = todo.len();
        let steps = self.timing.sync_steps.max(total_files as u32);
        let per_file = (steps as usize / total_files).max(1);
        // The speed a real copy of this size would show over the simulated time.
        let seconds = (self.timing.sync_step.as_secs_f64() * f64::from(steps)).max(2.5);
        let speed = total_bytes as f64 / seconds;
        let mut done_bytes = 0u64;
        let mut copied = Vec::new();
        for (i, ep) in todo.iter().enumerate() {
            let name = format!("{}.mp3", sanitize(&ep.title));
            for step in 0..per_file {
                if cancel.load(Ordering::SeqCst) {
                    return Ok(SyncOutcome::Cancelled);
                }
                let part = ep.file_size * step as u64 / per_file as u64;
                progress(TransferProgress {
                    current_file: name.clone(),
                    files_done: i,
                    total_files,
                    bytes_transferred: done_bytes + part,
                    total_bytes,
                    speed,
                });
                if !self.timing.sync_step.is_zero() {
                    sleep(self.timing.sync_step);
                }
            }
            done_bytes += ep.file_size;
            copied.push(ep.clone());
        }
        if cancel.load(Ordering::SeqCst) {
            return Ok(SyncOutcome::Cancelled);
        }
        progress(TransferProgress {
            current_file: "Finalizing drive...".into(),
            files_done: total_files,
            total_files,
            bytes_transferred: total_bytes,
            total_bytes,
            speed,
        });
        let mut d = self.lock();
        let new: Vec<Episode> = copied.iter().map(|e| drive_copy(e, drive)).collect();
        d.files.entry(drive.id.clone()).or_default().extend(new);
        if let Some(x) = d.drives.iter_mut().find(|x| x.id == drive.id)
            && let Some(free) = x.free_space.as_mut()
        {
            *free = free.saturating_sub(total_bytes);
        }
        Ok(SyncOutcome::Complete { copied: copied.len(), failed: Vec::new() })
    }

    fn delete_from_drive(&self, episodes: &[Episode]) -> Result<(), String> {
        self.wait();
        let mut d = self.lock();
        let mut freed = HashMap::<String, u64>::new();
        for (id, files) in d.files.iter_mut() {
            files.retain(|f| {
                let gone = episodes.iter().any(|e| e.file_path == f.file_path);
                if gone {
                    *freed.entry(id.clone()).or_default() += f.file_size;
                }
                !gone
            });
        }
        for (id, bytes) in freed {
            if let Some(x) = d.drives.iter_mut().find(|x| x.id == id)
                && let Some(free) = x.free_space.as_mut()
            {
                *free += bytes;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_data_is_plausible() {
        let b = DemoBackend::instant();
        let mac = b.load_mac_episodes().unwrap();
        assert!(mac.len() >= 30, "a few dozen episodes");
        let shows: std::collections::HashSet<_> = mac.iter().map(|e| e.show_name.clone()).collect();
        assert!(shows.len() >= 6);
        assert!(mac.windows(2).all(|w| w[0].published >= w[1].published), "newest first");
        let drives = b.scan_drives().unwrap();
        assert_eq!(drives.len(), 3);
        assert_eq!(b.scan_drive(&drives[0]).unwrap().len(), WALKMAN_HAS.len());
    }

    #[test]
    fn sync_copies_reports_progress_and_skips_what_is_there() {
        let b = DemoBackend::instant();
        let mac = b.load_mac_episodes().unwrap();
        let drive = b.scan_drives().unwrap()[2].clone();
        let mut seen = Vec::new();
        let out = b.sync(&mac[..3], &drive, &AtomicBool::new(false), &mut |p| seen.push(p)).unwrap();
        assert_eq!(out, SyncOutcome::Complete { copied: 3, failed: vec![] });
        assert!(seen.len() > 3);
        assert_eq!(seen.last().unwrap().files_done, 3);
        assert_eq!(b.scan_drive(&drive).unwrap().len(), 3);
        let again = b.sync(&mac[..3], &drive, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!(again, SyncOutcome::AlreadySynced);
    }

    #[test]
    fn a_cancelled_sync_copies_nothing() {
        let b = DemoBackend::instant();
        let mac = b.load_mac_episodes().unwrap();
        let drive = b.scan_drives().unwrap()[2].clone();
        let out = b.sync(&mac[..3], &drive, &AtomicBool::new(true), &mut |_| {}).unwrap();
        assert_eq!(out, SyncOutcome::Cancelled);
        assert!(b.scan_drive(&drive).unwrap().is_empty());
    }

    #[test]
    fn delete_removes_files_and_frees_space() {
        let b = DemoBackend::instant();
        let drive = b.scan_drives().unwrap()[0].clone();
        let files = b.scan_drive(&drive).unwrap();
        b.delete_from_drive(&files[..2]).unwrap();
        assert_eq!(b.scan_drive(&drive).unwrap().len(), files.len() - 2);
        let after = b.scan_drives().unwrap()[0].free_space.unwrap();
        assert_eq!(after, drive.free_space.unwrap() + files[0].file_size + files[1].file_size);
    }
}
