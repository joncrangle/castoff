//! The app's logic: startup, refresh, drive scanning and watching, sync and
//! delete. Every backend call runs through `Jobs`, never on the UI thread; answers come back as
//! `Msg`s that `poll` applies.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use castoff_core::{Drive, Episode, SyncOutcome, TransferProgress, mark_on_drive};
use rune::Tx;

use crate::state::{AppView, Level, Msg, Pane, ScanReason, State};
use crate::theme::is_theme;

/// How often the drive list is re-read.
pub const DRIVE_WATCH_SECS: f64 = 2.0;

/// Startup (`initialize`): settings, then the Mac library, then drives (auto-selecting one and
/// scanning it).
pub fn boot(s: &mut State) {
    if s.booted {
        return;
    }
    s.booted = true;
    s.loading_mac = true;
    s.loading_drive = true;
    s.scanning = true;
    s.mac_generation += 1;
    let generation = s.mac_generation;
    s.jobs.spawn(&s.backend, move |b, emit| {
        emit.send(Msg::Settings(b.load_settings()));
        emit.send(Msg::MacLoaded { generation, result: b.load_mac_episodes() });
        emit.send(Msg::DrivesScanned { reason: ScanReason::Full, result: b.scan_drives() });
    });
}

/// `scanForDrives`: the drive selector's scan.
pub fn scan_for_drives(s: &mut State) {
    s.scanning = true;
    s.jobs.spawn(&s.backend, |b, emit| {
        emit.send(Msg::DrivesScanned { reason: ScanReason::Full, result: b.scan_drives() });
    });
}

/// `loadDrivePodcasts`.
pub fn load_drive(s: &mut State, drive: Drive) {
    s.loading_drive = true;
    s.jobs.spawn(&s.backend, move |b, emit| {
        let result = b.scan_drive(&drive);
        emit.send(Msg::DriveLoaded { drive_id: drive.id.clone(), result });
    });
}

pub fn select_drive(s: &mut State, drive: Drive) {
    let changed = s.current_drive.as_ref().is_none_or(|d| d.id != drive.id);
    s.current_drive = Some(drive.clone());
    if changed {
        s.drive_eps.clear();
        s.drive_index = 0;
        unmark(&mut s.mac);
    }
    load_drive(s, drive);
}

/// `refreshData`.
pub fn refresh(s: &mut State) {
    s.loading_mac = true;
    s.loading_drive = true;
    s.error.clear();
    s.mac_generation += 1;
    let generation = s.mac_generation;
    let drive = s.current_drive.clone();
    s.jobs.spawn(&s.backend, move |b, emit| {
        emit.send(Msg::MacLoaded { generation, result: b.load_mac_episodes() });
        if let Some(drive) = drive {
            let result = b.scan_drive(&drive);
            emit.send(Msg::DriveLoaded { drive_id: drive.id.clone(), result });
        }
    });
    if s.current_drive.is_none() {
        s.loading_drive = false;
    }
}

/// `startSync`.
pub fn start_sync(s: &mut State, episodes: Vec<Episode>) {
    let Some(drive) = s.current_drive.clone() else {
        s.error = "No drive selected".into();
        s.view = AppView::Normal;
        return;
    };
    if episodes.is_empty() {
        return;
    }
    s.progress = TransferProgress { current_file: "Preparing...".into(), ..Default::default() };
    s.view = AppView::Syncing;
    let cancel = Arc::new(AtomicBool::new(false));
    s.sync_cancel = Some(cancel.clone());
    s.log(format!("Syncing {} episode(s) to {}", episodes.len(), drive.name), Level::Info);
    s.jobs.spawn(&s.backend, move |b, emit| {
        let mut progress = |p: TransferProgress| emit.send(Msg::SyncProgress(p));
        let result = b.sync(&episodes, &drive, &cancel, &mut progress);
        emit.send(Msg::SyncDone(result));
    });
}

/// `cancelSync`.
pub fn cancel_sync(s: &mut State) {
    if let Some(cancel) = &s.sync_cancel {
        cancel.store(true, Ordering::SeqCst);
        s.log("Cancelling sync...", Level::Info);
    }
}

/// `deleteSelectedFromDrive`.
pub fn delete_selected(s: &mut State) {
    let selected = s.selected(Pane::Drive);
    s.view = AppView::Normal;
    if selected.is_empty() {
        return;
    }
    s.loading_drive = true;
    s.log(format!("Deleting {} file(s) from drive", selected.len()), Level::Info);
    s.jobs.spawn(&s.backend, move |b, emit| {
        emit.send(Msg::Deleted(b.delete_from_drive(&selected)));
    });
}

fn save(s: &mut State, what: &'static str) {
    let settings = s.settings();
    s.jobs.spawn(&s.backend, move |b, emit| {
        emit.send(Msg::Saved { what, result: b.save_settings(&settings) });
    });
}

/// `saveTheme`.
pub fn save_theme(s: &mut State, name: &str) {
    s.theme = name.into();
    s.saved_theme = name.into();
    save(s, "theme");
}

/// `toggleFavoriteDrive`.
pub fn toggle_favorite(s: &mut State, drive_id: &str) {
    if let Some(i) = s.favorites.iter().position(|f| f == drive_id) {
        s.favorites.remove(i);
    } else {
        s.favorites.push(drive_id.to_string());
    }
    save(s, "favorites");
}

fn unmark(mac: &mut [Episode]) {
    for e in mac.iter_mut() {
        e.on_drive = false;
    }
}

fn remark(s: &mut State) {
    unmark(&mut s.mac);
    if s.current_drive.is_some() {
        mark_on_drive(&mut s.mac, &s.drive_eps);
    }
}

/// Favourites first, otherwise in scan order (a stable sort).
fn sort_favorites_first(s: &State, drives: &mut [Drive]) {
    drives.sort_by_key(|d| !s.is_favorite(&d.id));
}

fn clamp(i: usize, len: usize) -> usize {
    if len == 0 { 0 } else { i.min(len - 1) }
}

/// Applies one answer from a job.
pub fn apply(s: &mut State, msg: Msg) {
    match msg {
        Msg::Settings(settings) => {
            if is_theme(&settings.theme) {
                s.theme = settings.theme.clone();
                s.saved_theme = settings.theme;
            }
            s.favorites = settings.favorite_drives;
        }
        Msg::MacLoaded { generation, result } => {
            if generation != s.mac_generation {
                return;
            }
            s.loading_mac = false;
            match result {
                Ok(episodes) => {
                    s.log(format!("Loaded {} Mac episodes", episodes.len()), Level::Info);
                    s.mac = episodes;
                    s.mac_index = clamp(s.mac_index, s.mac.len());
                    remark(s);
                }
                Err(e) => {
                    s.error = e.clone();
                    s.log(e, Level::Error);
                }
            }
        }
        Msg::DrivesScanned { reason: ScanReason::Full, result } => {
            s.scanning = false;
            match result {
                Ok(mut drives) => {
                    sort_favorites_first(s, &mut drives);
                    s.log(format!("Found {} drives", drives.len()), Level::Info);
                    s.drives = drives;
                    s.drive_menu_index = 0;
                    if s.current_drive.is_none() {
                        let pick = s.drives.iter().find(|d| s.is_favorite(&d.id)).or(s.drives.first()).cloned();
                        match pick {
                            Some(drive) => select_drive(s, drive),
                            None => s.loading_drive = false,
                        }
                    } else if let Some(cur) = &s.current_drive
                        && let Some(fresh) = s.drives.iter().find(|d| d.id == cur.id)
                    {
                        // Keep the header's free space current.
                        s.current_drive = Some(fresh.clone());
                    }
                }
                Err(e) => {
                    s.error = e.clone();
                    s.log(e, Level::Error);
                    s.drives.clear();
                    s.loading_drive = false;
                }
            }
        }
        Msg::DrivesScanned { reason: ScanReason::Watch, result } => {
            s.watch_in_flight = false;
            match result {
                Ok(now) => watch_answer(s, now),
                Err(e) => s.log(format!("Drive listener error: {e}"), Level::Error),
            }
        }
        Msg::DriveLoaded { drive_id, result } => {
            if s.current_drive.as_ref().is_none_or(|d| d.id != drive_id) {
                return;
            }
            s.loading_drive = false;
            match result {
                Ok(episodes) => {
                    s.log(format!("Loaded {} drive episodes", episodes.len()), Level::Info);
                    s.drive_eps = episodes;
                    s.drive_index = clamp(s.drive_index, s.drive_eps.len());
                    remark(s);
                }
                Err(e) => {
                    s.error = e.clone();
                    s.log(e, Level::Error);
                    s.drive_eps.clear();
                    remark(s);
                }
            }
        }
        Msg::SyncProgress(p) => {
            if s.view == AppView::Syncing {
                s.progress = p;
            }
        }
        Msg::SyncDone(result) => {
            s.sync_cancel = None;
            s.view = AppView::Normal;
            match result {
                Ok(SyncOutcome::Complete { copied, failed }) => {
                    s.log(format!("Sync complete: {copied} copied, {} failed", failed.len()), Level::Info);
                    for (file, err) in &failed {
                        s.log(format!("{file}: {err}"), Level::Error);
                    }
                    if let Some((file, err)) = failed.first() {
                        s.error = format!("{} file(s) failed to copy ({file}: {err})", failed.len());
                    }
                    after_sync(s);
                }
                Ok(SyncOutcome::AlreadySynced) => {
                    s.log("All episodes already synced", Level::Info);
                    after_sync(s);
                }
                Ok(SyncOutcome::Cancelled) => {
                    s.log("Sync cancelled by user", Level::Info);
                    s.error.clear();
                }
                Err(e) => {
                    s.log(format!("Sync failed: {e}"), Level::Error);
                    s.error = e;
                }
            }
        }
        Msg::Deleted(result) => {
            if let Err(e) = result {
                s.error = e.clone();
                s.log(e, Level::Error);
            }
            match s.current_drive.clone() {
                Some(drive) => load_drive(s, drive),
                None => s.loading_drive = false,
            }
            for e in s.drive_eps.iter_mut() {
                e.selected = false;
            }
        }
        Msg::Saved { what, result } => match result {
            Ok(()) => s.log(format!("Saved {what}"), Level::Info),
            Err(e) => s.log(format!("Failed to save {what}: {e}"), Level::Error),
        },
    }
}

fn after_sync(s: &mut State) {
    if let Some(drive) = s.current_drive.clone() {
        load_drive(s, drive);
    }
    for e in s.mac.iter_mut() {
        e.selected = false;
    }
}

/// Drives that appeared or disappeared, from the difference between two scans.
fn watch_answer(s: &mut State, now: Vec<Drive>) {
    let gone: Vec<Drive> = s.drives.iter().filter(|d| !now.iter().any(|n| n.id == d.id)).cloned().collect();
    let new: Vec<Drive> = now.iter().filter(|n| !s.drives.iter().any(|d| d.id == n.id)).cloned().collect();
    for drive in gone {
        s.drives.retain(|d| d.id != drive.id);
        s.log(format!("Drive disappeared: {} ({})", drive.name, drive.id), Level::Info);
        if s.current_drive.as_ref().is_some_and(|d| d.id == drive.id) {
            s.current_drive = None;
            s.drive_eps.clear();
            s.drive_index = 0;
            s.loading_drive = false;
            unmark(&mut s.mac);
        }
    }
    for drive in new {
        s.drives.push(drive.clone());
        let mut drives = std::mem::take(&mut s.drives);
        sort_favorites_first(s, &mut drives);
        s.drives = drives;
        s.log(format!("Drive appeared: {} ({})", drive.name, drive.id), Level::Info);
        if s.current_drive.is_none() {
            select_drive(s, drive);
        }
    }
    s.drive_menu_index = clamp(s.drive_menu_index, s.drives.len());
}

/// `AppSpec::poll`: apply answers, and re-read the drive list every couple of seconds.
pub fn poll(s: &mut State, clock: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
    if !s.booted {
        boot(s);
        s.next_drive_watch = clock + DRIVE_WATCH_SECS;
        changed = true;
    }
    // An answer can start more work (a drive scan selects a drive and loads it); inline jobs
    // answer at once, so keep going until the queue is empty.
    if s.paced {
        s.backlog.extend(s.jobs.drain());
        while let Some(msg) = s.backlog.pop_front() {
            let step = matches!(msg, Msg::SyncProgress(_));
            apply(s, msg);
            s.backlog.extend(s.jobs.drain());
            changed = true;
            if step {
                break;
            }
        }
        if !s.backlog.is_empty() {
            tx.wake_after(Duration::from_millis(1));
        }
    } else {
        for _ in 0..16 {
            let msgs = s.jobs.drain();
            if msgs.is_empty() {
                break;
            }
            for msg in msgs {
                apply(s, msg);
            }
            changed = true;
        }
    }
    if s.reveal_pending {
        s.reveal_pending = false;
        crate::keys::reveal(s, tx);
    }
    if s.watch_drives && s.booted {
        if clock >= s.next_drive_watch && !s.watch_in_flight && !s.scanning {
            s.watch_in_flight = true;
            s.next_drive_watch = clock + DRIVE_WATCH_SECS;
            s.jobs.spawn(&s.backend, |b, emit| {
                emit.send(Msg::DrivesScanned { reason: ScanReason::Watch, result: b.scan_drives() });
            });
            // An inline job has answered already.
            for msg in s.jobs.drain() {
                apply(s, msg);
                changed = true;
            }
        }
        tx.wake_after(Duration::from_secs_f64((s.next_drive_watch - clock).max(0.05)));
    }
    changed
}
