//! Read-only smoke check: prints what `LiveBackend` finds on this machine. Never syncs, deletes,
//! or writes (settings are only loaded, not saved).
//!
//! `CARGO_TARGET_DIR=target/core cargo run -p castoff-core --example probe`

use std::time::Instant;

use castoff_core::drives::{
    Diskutil, VolumeInfoSource, drive_from_info, plist_number, plist_string,
};
use castoff_core::format::{format_bytes, format_date_opt, format_duration};
use castoff_core::{Backend, LiveBackend};

fn main() {
    let backend = LiveBackend::new();

    let settings = backend.load_settings();
    println!(
        "settings: theme = {:?}, favorite_drives = {:?}",
        settings.theme, settings.favorite_drives
    );

    let started = Instant::now();
    match backend.load_mac_episodes() {
        Ok(episodes) => {
            let missing = episodes.iter().filter(|e| e.file_size == 0).count();
            println!(
                "mac episodes: {} ({} with no file on disk) in {:?}",
                episodes.len(),
                missing,
                started.elapsed()
            );
            for e in episodes.iter().take(5) {
                println!(
                    "  - [{}] {} | {} | {} | {} | {} | {}",
                    e.id,
                    e.show_name,
                    e.title,
                    format_date_opt(e.published),
                    format_duration(e.duration_secs),
                    format_bytes(e.file_size),
                    e.file_path.display()
                );
            }
        }
        Err(e) => println!("mac episodes: error: {e}"),
    }

    for round in 1..=3 {
        let started = Instant::now();
        match backend.scan_drives() {
            Ok(drives) => {
                println!(
                    "drives (scan {round}, {:?}): {}",
                    started.elapsed(),
                    drives.len()
                );
                if round == 1 {
                    for d in &drives {
                        let free = d.free_space.map_or("unknown".to_string(), format_bytes);
                        println!(
                            "  - {} [{}] {} total {} free {} id {}",
                            d.name,
                            d.bsd_name,
                            d.mount_point.display(),
                            format_bytes(d.total_space),
                            free,
                            d.id
                        );
                        match backend.scan_drive(d) {
                            Ok(eps) => {
                                println!("    {} episode file(s) under Podcasts/", eps.len())
                            }
                            Err(e) => println!("    scan error: {e}"),
                        }
                    }
                }
            }
            Err(e) => println!("drives (scan {round}): error: {e}"),
        }
    }

    // No external disk to look at? Show how the real `diskutil` output for the startup volume is
    // read and why it is (correctly) not offered as a drive.
    let root = std::path::Path::new("/");
    match Diskutil::default().info(root) {
        Ok(info) => println!(
            "diskutil info /: VolumeName={:?} MountPoint={:?} TotalSize={:?} VolumeFreeSpace={:?} -> {}",
            plist_string(&info, "VolumeName"),
            plist_string(&info, "MountPoint"),
            plist_number(&info, "TotalSize"),
            plist_number(&info, "VolumeFreeSpace"),
            match drive_from_info(&info) {
                Ok(d) => format!("offered as drive {:?}", d.name),
                Err(reason) => format!("excluded ({reason})"),
            }
        ),
        Err(e) => println!("diskutil info /: error: {e}"),
    }
}
