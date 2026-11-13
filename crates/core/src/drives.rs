//! Finding external drives.
//!
//! `/Volumes` is listed and each volume is described by `diskutil info -plist`. Because the UI
//! polls [`DriveScanner::scan`] every couple of seconds, a volume's `diskutil` verdict is cached
//! (keyed by name and device number, so a remount or a different disk under the same name is
//! looked at again) and only its free space is refreshed on later scans. Slow or hung volumes
//! can't stall a scan: each is probed on its own thread under a deadline, and `diskutil` itself
//! runs under a timeout.

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use plist::{Dictionary, Value};

use crate::api::Drive;

/// Volumes that are never offered as drives.
pub const EXCLUDED_NAMES: [&str; 6] = [
    "Macintosh HD",
    "Macintosh HD - Data",
    "Recovery",
    "Preboot",
    "VM",
    "Update",
];

const DISKUTIL_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a failed `diskutil` call is remembered before the volume is asked about again.
const FAILURE_RETRY: Duration = Duration::from_secs(10);

// ---- plist helpers -----------------------------------------------------------------------------

/// Parses a plist (XML or binary) whose root is a dictionary, as `diskutil -plist` prints.
pub fn parse_plist_dict(bytes: &[u8]) -> Result<Dictionary, String> {
    let value =
        Value::from_reader(Cursor::new(bytes)).map_err(|e| format!("Invalid plist: {e}"))?;
    value
        .into_dictionary()
        .ok_or_else(|| "Invalid plist output".to_string())
}

pub fn plist_string<'a>(dict: &'a Dictionary, key: &str) -> Option<&'a str> {
    dict.get(key)?.as_string()
}

pub fn plist_bool(dict: &Dictionary, key: &str) -> Option<bool> {
    dict.get(key)?.as_boolean()
}

/// Integers and reals both read as numbers, as in JavaScript.
pub fn plist_number(dict: &Dictionary, key: &str) -> Option<f64> {
    match dict.get(key)? {
        Value::Integer(i) => i
            .as_signed()
            .map(|v| v as f64)
            .or_else(|| i.as_unsigned().map(|v| v as f64)),
        Value::Real(r) => Some(*r),
        _ => None,
    }
}

// ---- classification ----------------------------------------------------------------------------

/// Why a volume is not offered as a drive, or `None` when it should be: internal system volumes,
/// Time Machine backups, and internal disks that aren't USB/Thunderbolt enclosures. (A volume that
/// doesn't say whether it's internal is let through.)
pub fn exclusion_reason(info: &Dictionary) -> Option<String> {
    let volume_name = plist_string(info, "VolumeName").unwrap_or("");
    let mount_point = plist_string(info, "MountPoint").unwrap_or("");

    if EXCLUDED_NAMES.contains(&volume_name) {
        return Some(format!("In EXCLUDED_NAMES: {volume_name}"));
    }
    if mount_point.is_empty() {
        return Some("No mount point".to_string());
    }

    let fs_type = plist_string(info, "FilesystemType").unwrap_or("");
    if fs_type == "apfs" && volume_name.to_lowercase().contains("time machine") {
        return Some("Time Machine volume".to_string());
    }

    let internal = plist_bool(info, "Internal");
    let removable = plist_bool(info, "Removable");
    let ejectable = plist_bool(info, "Ejectable");
    if internal == Some(true) && removable != Some(true) && ejectable != Some(true) {
        let bus = plist_string(info, "BusProtocol").unwrap_or("");
        if bus != "USB" && bus != "Thunderbolt" {
            return Some(format!("Internal drive with non-removable protocol: {bus}"));
        }
    }
    None
}

/// Free space from whichever key the filesystem provides. A negative result (the APFS fallback
/// subtracts two unrelated sizes) counts as unknown.
fn free_space_from_info(info: &Dictionary) -> Option<u64> {
    let free = plist_number(info, "VolumeFreeSpace")
        .or_else(|| plist_number(info, "APFSContainerFree"))
        .or_else(|| plist_number(info, "FreeSpace"))
        .or_else(|| {
            let total = plist_number(info, "APFSContainerTotalSpace")?;
            let used = plist_number(info, "APFSPhysicalStoreSize")?;
            Some(total - used)
        })?;
    (free >= 0.0).then_some(free as u64)
}

/// Builds a [`Drive`] from `diskutil info` output, or says why the volume was skipped.
pub fn drive_from_info(info: &Dictionary) -> Result<Drive, String> {
    if let Some(reason) = exclusion_reason(info) {
        return Err(reason);
    }
    let mount_point = plist_string(info, "MountPoint").unwrap_or_default();

    let volume_uuid = plist_string(info, "VolumeUUID").unwrap_or("");
    let volume_name = plist_string(info, "VolumeName")
        .map(str::to_owned)
        .unwrap_or_else(|| mount_point.rsplit('/').next().unwrap_or("").to_string());
    let bsd_name = plist_string(info, "DeviceIdentifier").unwrap_or("");
    let total_space = plist_number(info, "TotalSize").unwrap_or(0.0);

    if total_space <= 0.0 {
        return Err("zero total space".to_string());
    }

    let id = [volume_uuid, bsd_name, volume_name.as_str()]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or_default()
        .to_string();

    Ok(Drive {
        id,
        name: volume_name,
        bsd_name: bsd_name.to_string(),
        mount_point: PathBuf::from(mount_point),
        total_space: total_space as u64,
        free_space: free_space_from_info(info),
    })
}

// ---- `diskutil activity` lines -----------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityKind {
    Appeared,
    Disappeared,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityEvent {
    pub kind: ActivityKind,
    pub bsd_name: String,
}

/// Parses a line of `diskutil activity` such as `***DiskAppeared ('disk4s1', 'USB', 'MS-DOS')`.
/// (The UI polls rather than streaming this; the parser is kept for a future event-driven watch.)
pub fn parse_activity_line(line: &str) -> Option<ActivityEvent> {
    let rest = line.trim_start().strip_prefix("***")?;
    let name_end = rest
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(rest.len());
    let (action, rest) = rest.split_at(name_end);
    let kind = match action {
        "DiskAppeared" | "VolumeMount" | "DiskDescriptionChanged" => ActivityKind::Appeared,
        "DiskDisappeared" | "VolumeUnmount" => ActivityKind::Disappeared,
        _ => return None,
    };
    let after_ws = rest.trim_start();
    if after_ws.len() == rest.len() {
        return None; // the action must be followed by whitespace
    }
    let after_paren = after_ws.strip_prefix("('")?;
    let end = after_paren.find('\'')?;
    let bsd_name = &after_paren[..end];
    if bsd_name.is_empty() || !after_paren[end + 1..].starts_with(',') {
        return None;
    }
    Some(ActivityEvent {
        kind,
        bsd_name: bsd_name.to_string(),
    })
}

// ---- asking diskutil ---------------------------------------------------------------------------

/// Where volume information comes from. The live implementation is [`Diskutil`]; tests supply
/// fixtures.
pub trait VolumeInfoSource: Send + Sync {
    /// The `diskutil info -plist` dictionary for the volume mounted at `mount_point`.
    fn info(&self, mount_point: &Path) -> Result<Dictionary, String>;
}

/// Runs the real `diskutil`.
pub struct Diskutil {
    pub timeout: Duration,
}

impl Default for Diskutil {
    fn default() -> Self {
        Diskutil {
            timeout: DISKUTIL_TIMEOUT,
        }
    }
}

impl VolumeInfoSource for Diskutil {
    fn info(&self, mount_point: &Path) -> Result<Dictionary, String> {
        let mut child = Command::new("diskutil")
            .arg("info")
            .arg("-plist")
            .arg(mount_point)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Could not run diskutil: {e}"))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or("Could not read diskutil output")?;

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = tx.send(stdout.read_to_end(&mut buf).map(|_| buf));
        });

        match rx.recv_timeout(self.timeout) {
            Ok(Ok(bytes)) => {
                let status = child.wait().map_err(|e| format!("diskutil: {e}"))?;
                if !status.success() {
                    return Err(format!("diskutil exited with {status}"));
                }
                parse_plist_dict(&bytes)
            }
            Ok(Err(e)) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(format!("Could not read diskutil output: {e}"))
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                Err("diskutil timed out".to_string())
            }
        }
    }
}

/// Free bytes available to unprivileged users on the volume at `path`, from `statvfs`.
pub fn statvfs_free_space(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `stat` is a plain-old-data out-parameter that statvfs fills in on success, and
    // `c_path` is a valid NUL-terminated string that outlives the call.
    unsafe {
        let mut stat: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c_path.as_ptr(), &mut stat) == 0 {
            // The field widths differ between platforms, so some conversions are no-ops.
            #[allow(clippy::useless_conversion)]
            let free = Some(u64::from(stat.f_bavail).saturating_mul(u64::from(stat.f_frsize)));
            free
        } else {
            None
        }
    }
}

// ---- scanning /Volumes -------------------------------------------------------------------------

struct CacheEntry {
    dev: u64,
    /// `None` for volumes that are excluded (or whose `diskutil` call failed).
    drive: Option<Drive>,
    /// Set for failures: ask again after this.
    retry_at: Option<Instant>,
}

type Cache = Arc<Mutex<HashMap<String, CacheEntry>>>;
/// Current free bytes of a mounted volume, or `None` when unknown.
pub type FreeSpaceFn = Arc<dyn Fn(&Path) -> Option<u64> + Send + Sync>;

/// Lists the drives mounted under a volumes root (normally `/Volumes`).
pub struct DriveScanner {
    volumes_root: PathBuf,
    source: Arc<dyn VolumeInfoSource>,
    free_space: FreeSpaceFn,
    cache: Cache,
    deadline: Duration,
}

impl DriveScanner {
    /// The real thing: `/Volumes`, `diskutil`, free space from `statvfs`.
    pub fn system() -> Self {
        DriveScanner::new(
            PathBuf::from("/Volumes"),
            Arc::new(Diskutil::default()),
            Arc::new(statvfs_free_space),
        )
    }

    pub fn new(
        volumes_root: PathBuf,
        source: Arc<dyn VolumeInfoSource>,
        free_space: FreeSpaceFn,
    ) -> Self {
        DriveScanner {
            volumes_root,
            source,
            free_space,
            cache: Arc::new(Mutex::new(HashMap::new())),
            deadline: DISKUTIL_TIMEOUT + Duration::from_secs(1),
        }
    }

    /// Overrides how long a scan waits for slow volumes before giving up on them.
    pub fn with_deadline(mut self, deadline: Duration) -> Self {
        self.deadline = deadline;
        self
    }

    /// The mounted external volumes, in name order. Volumes that don't answer in time are left out
    /// of this scan (and tried again on the next).
    pub fn scan(&self) -> Result<Vec<Drive>, String> {
        let entries = std::fs::read_dir(&self.volumes_root)
            .map_err(|e| format!("Could not list {}: {e}", self.volumes_root.display()))?;
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| !n.starts_with('.') && !EXCLUDED_NAMES.contains(&n.as_str()))
            .collect();
        names.sort();

        // Forget volumes that are gone.
        if let Ok(mut cache) = self.cache.lock() {
            cache.retain(|name, _| names.contains(name));
        }

        let (tx, rx) = mpsc::channel();
        for (index, name) in names.iter().enumerate() {
            let tx = tx.clone();
            let name = name.clone();
            let mount = self.volumes_root.join(&name);
            let source = Arc::clone(&self.source);
            let free_space = Arc::clone(&self.free_space);
            let cache = Arc::clone(&self.cache);
            let spawned = thread::Builder::new()
                .name("castoff-drive-probe".into())
                .spawn(move || {
                    let _ = tx.send((
                        index,
                        probe_volume(&name, &mount, &*source, &*free_space, &cache),
                    ));
                });
            if spawned.is_err() {
                // Out of threads: skip this volume for now.
                continue;
            }
        }
        drop(tx);

        let give_up_at = Instant::now() + self.deadline;
        let mut found: Vec<(usize, Drive)> = Vec::new();
        while let Ok((index, result)) =
            rx.recv_timeout(give_up_at.saturating_duration_since(Instant::now()))
        {
            if let Some(drive) = result {
                found.push((index, drive));
            }
        }
        found.sort_by_key(|(index, _)| *index);
        Ok(found.into_iter().map(|(_, d)| d).collect())
    }

    /// The drive mounted at `mount_point`, if it qualifies. Not cached.
    pub fn drive_at(&self, mount_point: &Path) -> Option<Drive> {
        self.source
            .info(mount_point)
            .ok()
            .and_then(|info| drive_from_info(&info).ok())
    }
}

/// Decides whether one `/Volumes` entry is a drive, using and filling the cache.
fn probe_volume(
    name: &str,
    mount: &Path,
    source: &dyn VolumeInfoSource,
    free_space: &(dyn Fn(&Path) -> Option<u64> + Send + Sync),
    cache: &Cache,
) -> Option<Drive> {
    let meta = std::fs::metadata(mount).ok()?;
    if !meta.is_dir() {
        return None;
    }
    let dev = meta.dev();

    let cached = cache.lock().ok().and_then(|c| {
        let entry = c.get(name)?;
        let fresh = entry.dev == dev && entry.retry_at.is_none_or(|t| Instant::now() < t);
        fresh.then(|| entry.drive.clone())
    });

    let drive = match cached {
        Some(drive) => drive,
        None => {
            let (drive, retry_at) = match source.info(mount) {
                Ok(info) => (drive_from_info(&info).ok(), None),
                Err(_) => (None, Some(Instant::now() + FAILURE_RETRY)),
            };
            if let Ok(mut c) = cache.lock() {
                c.insert(
                    name.to_string(),
                    CacheEntry {
                        dev,
                        drive: drive.clone(),
                        retry_at,
                    },
                );
            }
            drive
        }
    };

    drive.map(|mut d| {
        if let Some(free) = free_space(mount) {
            d.free_space = Some(free);
        }
        d
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    const WALKMAN_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>BusProtocol</key>
	<string>USB</string>
	<key>DeviceIdentifier</key>
	<string>disk4s1</string>
	<key>Ejectable</key>
	<true/>
	<key>Internal</key>
	<false/>
	<key>MountPoint</key>
	<string>/Volumes/WALKMAN</string>
	<key>Removable</key>
	<true/>
	<key>TotalSize</key>
	<integer>32212254720</integer>
	<key>VolumeFreeSpace</key>
	<integer>16106127360</integer>
	<key>VolumeName</key>
	<string>WALKMAN</string>
</dict>
</plist>"#;

    fn dict(entries: &[(&str, Value)]) -> Dictionary {
        let mut d = Dictionary::new();
        for (k, v) in entries {
            d.insert((*k).to_string(), v.clone());
        }
        d
    }

    fn s(v: &str) -> Value {
        Value::String(v.to_string())
    }
    fn b(v: bool) -> Value {
        Value::Boolean(v)
    }
    fn n(v: u64) -> Value {
        Value::Integer(v.into())
    }

    /// An external USB disk.
    fn usb_info(name: &str, mount: &str) -> Dictionary {
        dict(&[
            ("VolumeName", s(name)),
            ("MountPoint", s(mount)),
            ("DeviceIdentifier", s("disk4s1")),
            ("VolumeUUID", s("UUID-1")),
            ("Internal", b(false)),
            ("Removable", b(true)),
            ("Ejectable", b(true)),
            ("BusProtocol", s("USB")),
            ("TotalSize", n(32_000_000_000)),
            ("VolumeFreeSpace", n(16_000_000_000)),
        ])
    }

    // ---- plist parsing ----------------------------------------------------------------------

    #[test]
    fn parses_real_diskutil_output() {
        let result = parse_plist_dict(WALKMAN_PLIST.as_bytes()).unwrap();
        assert_eq!(plist_string(&result, "BusProtocol"), Some("USB"));
        assert_eq!(plist_string(&result, "VolumeName"), Some("WALKMAN"));
        assert_eq!(
            plist_string(&result, "MountPoint"),
            Some("/Volumes/WALKMAN")
        );
        assert_eq!(plist_bool(&result, "Internal"), Some(false));
        assert_eq!(plist_bool(&result, "Removable"), Some(true));
        assert_eq!(plist_bool(&result, "Ejectable"), Some(true));
        assert_eq!(plist_number(&result, "TotalSize"), Some(32_212_254_720.0));
        assert_eq!(
            plist_number(&result, "VolumeFreeSpace"),
            Some(16_106_127_360.0)
        );
    }

    fn wrap(body: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\">\n<dict>\n{body}\n</dict>\n</plist>"
        )
    }

    #[test]
    fn parses_strings_integers_booleans() {
        let xml = wrap(
            "<key>VolumeName</key><string>WALKMAN</string>
             <key>TotalSize</key><integer>32000000000</integer>
             <key>FreeSpace</key><integer>16000000000</integer>
             <key>Internal</key><false/>
             <key>Removable</key><true/>
             <key>Ratio</key><real>0.5</real>",
        );
        let d = parse_plist_dict(xml.as_bytes()).unwrap();
        assert_eq!(plist_string(&d, "VolumeName"), Some("WALKMAN"));
        assert_eq!(plist_number(&d, "TotalSize"), Some(32_000_000_000.0));
        assert_eq!(plist_number(&d, "FreeSpace"), Some(16_000_000_000.0));
        assert_eq!(plist_bool(&d, "Internal"), Some(false));
        assert_eq!(plist_bool(&d, "Removable"), Some(true));
        assert_eq!(plist_number(&d, "Ratio"), Some(0.5));
    }

    #[test]
    fn parses_nested_dicts_and_arrays() {
        let xml = wrap(
            "<key>DriveInfo</key><dict><key>Name</key><string>USB Drive</string>
               <key>Size</key><integer>1000000</integer></dict>
             <key>Volumes</key><array><string>WALKMAN</string><string>SANSA</string></array>",
        );
        let d = parse_plist_dict(xml.as_bytes()).unwrap();
        let info = d.get("DriveInfo").unwrap().as_dictionary().unwrap();
        assert_eq!(plist_string(info, "Name"), Some("USB Drive"));
        assert_eq!(plist_number(info, "Size"), Some(1_000_000.0));
        let volumes = d.get("Volumes").unwrap().as_array().unwrap();
        assert_eq!(volumes.len(), 2);
        assert_eq!(volumes[0].as_string(), Some("WALKMAN"));
        assert_eq!(volumes[1].as_string(), Some("SANSA"));
    }

    #[test]
    fn decodes_xml_entities() {
        let xml = wrap("<key>Name</key><string>Tom &amp; Jerry&apos;s Drive</string>");
        let d = parse_plist_dict(xml.as_bytes()).unwrap();
        assert_eq!(plist_string(&d, "Name"), Some("Tom & Jerry's Drive"));
    }

    #[test]
    fn rejects_garbage_and_non_dictionary_roots() {
        assert!(parse_plist_dict(b"not a plist").is_err());
        let array_root = "<?xml version=\"1.0\"?><plist version=\"1.0\"><array><string>x</string></array></plist>";
        assert_eq!(
            parse_plist_dict(array_root.as_bytes()).unwrap_err(),
            "Invalid plist output"
        );
    }

    #[test]
    fn plist_getters_return_none_for_the_wrong_type_or_a_missing_key() {
        let d = dict(&[("S", s("hello")), ("N", n(42)), ("B", b(true))]);
        assert_eq!(plist_string(&d, "S"), Some("hello"));
        assert_eq!(plist_string(&d, "N"), None);
        assert_eq!(plist_string(&d, "Missing"), None);
        assert_eq!(plist_number(&d, "N"), Some(42.0));
        assert_eq!(plist_number(&d, "S"), None);
        assert_eq!(plist_number(&d, "Missing"), None);
        assert_eq!(plist_bool(&d, "B"), Some(true));
        assert_eq!(plist_bool(&d, "S"), None);
        assert_eq!(plist_bool(&d, "Missing"), None);
    }

    // ---- classification ---------------------------------------------------------------------

    #[test]
    fn external_usb_disk_becomes_a_drive() {
        let drive = drive_from_info(&parse_plist_dict(WALKMAN_PLIST.as_bytes()).unwrap()).unwrap();
        assert_eq!(drive.id, "disk4s1"); // no VolumeUUID in this fixture
        assert_eq!(drive.name, "WALKMAN");
        assert_eq!(drive.bsd_name, "disk4s1");
        assert_eq!(drive.mount_point, PathBuf::from("/Volumes/WALKMAN"));
        assert_eq!(drive.total_space, 32_212_254_720);
        assert_eq!(drive.free_space, Some(16_106_127_360));
    }

    #[test]
    fn id_prefers_uuid_then_bsd_name_then_volume_name() {
        let mut info = usb_info("X", "/Volumes/X");
        assert_eq!(drive_from_info(&info).unwrap().id, "UUID-1");
        info.remove("VolumeUUID");
        assert_eq!(drive_from_info(&info).unwrap().id, "disk4s1");
        info.remove("DeviceIdentifier");
        assert_eq!(drive_from_info(&info).unwrap().id, "X");
    }

    #[test]
    fn volume_name_defaults_to_the_mount_point_leaf() {
        let mut info = usb_info("X", "/Volumes/Leaf Name");
        info.remove("VolumeName");
        assert_eq!(drive_from_info(&info).unwrap().name, "Leaf Name");
    }

    #[test]
    fn excludes_system_volumes_by_name() {
        for name in EXCLUDED_NAMES {
            let reason = drive_from_info(&usb_info(name, "/Volumes/x")).unwrap_err();
            assert!(reason.contains("EXCLUDED_NAMES"), "{name}: {reason}");
        }
    }

    #[test]
    fn excludes_volumes_without_a_mount_point() {
        let mut info = usb_info("X", "");
        assert_eq!(drive_from_info(&info).unwrap_err(), "No mount point");
        info.remove("MountPoint");
        assert_eq!(drive_from_info(&info).unwrap_err(), "No mount point");
    }

    #[test]
    fn excludes_apfs_time_machine_volumes_only() {
        let mut info = usb_info("Time Machine Backups", "/Volumes/tm");
        info.insert("FilesystemType".into(), s("apfs"));
        assert_eq!(drive_from_info(&info).unwrap_err(), "Time Machine volume");
        info.insert("FilesystemType".into(), s("hfs"));
        assert!(drive_from_info(&info).is_ok());
    }

    #[test]
    fn internal_disks_are_excluded_unless_usb_thunderbolt_removable_or_ejectable() {
        let internal = |extra: &[(&str, Value)]| {
            let mut info = dict(&[
                ("VolumeName", s("Data")),
                ("MountPoint", s("/Volumes/Data")),
                ("Internal", b(true)),
                ("TotalSize", n(100)),
            ]);
            for (k, v) in extra {
                info.insert((*k).to_string(), v.clone());
            }
            info
        };
        assert!(drive_from_info(&internal(&[("BusProtocol", s("PCI-Express"))])).is_err());
        assert!(
            drive_from_info(&internal(&[
                ("Removable", b(false)),
                ("Ejectable", b(false))
            ]))
            .is_err()
        );
        assert!(drive_from_info(&internal(&[("BusProtocol", s("USB"))])).is_ok());
        assert!(drive_from_info(&internal(&[("BusProtocol", s("Thunderbolt"))])).is_ok());
        assert!(drive_from_info(&internal(&[("Removable", b(true))])).is_ok());
        assert!(drive_from_info(&internal(&[("Ejectable", b(true))])).is_ok());

        // Unknown internal-ness is let through.
        let mut unknown = internal(&[]);
        unknown.remove("Internal");
        assert!(drive_from_info(&unknown).is_ok());
    }

    #[test]
    fn excludes_zero_total_space() {
        let mut info = usb_info("X", "/Volumes/X");
        info.insert("TotalSize".into(), n(0));
        assert_eq!(drive_from_info(&info).unwrap_err(), "zero total space");
        info.remove("TotalSize");
        assert!(drive_from_info(&info).is_err());
    }

    #[test]
    fn free_space_falls_back_through_the_known_keys() {
        let mut info = usb_info("X", "/Volumes/X");
        assert_eq!(
            drive_from_info(&info).unwrap().free_space,
            Some(16_000_000_000)
        );

        info.remove("VolumeFreeSpace");
        info.insert("APFSContainerFree".into(), n(5));
        assert_eq!(drive_from_info(&info).unwrap().free_space, Some(5));

        info.remove("APFSContainerFree");
        info.insert("FreeSpace".into(), n(6));
        assert_eq!(drive_from_info(&info).unwrap().free_space, Some(6));

        info.remove("FreeSpace");
        info.insert("APFSContainerTotalSpace".into(), n(100));
        info.insert("APFSPhysicalStoreSize".into(), n(30));
        assert_eq!(drive_from_info(&info).unwrap().free_space, Some(70));

        // Negative (nonsense) and absent both mean unknown.
        info.insert("APFSPhysicalStoreSize".into(), n(300));
        assert_eq!(drive_from_info(&info).unwrap().free_space, None);
        info.remove("APFSContainerTotalSpace");
        assert_eq!(drive_from_info(&info).unwrap().free_space, None);
    }

    // ---- diskutil activity ------------------------------------------------------------------

    fn activity(line: &str) -> Option<(ActivityKind, String)> {
        parse_activity_line(line).map(|e| (e.kind, e.bsd_name))
    }

    #[test]
    fn activity_lines() {
        let tail = "('disk4s1', 'USB_DRIVE', 'MS-DOS FAT32')";
        let appeared = Some((ActivityKind::Appeared, "disk4s1".to_string()));
        let disappeared = Some((ActivityKind::Disappeared, "disk4s1".to_string()));
        assert_eq!(
            activity(&format!("***DiskAppeared          {tail}")),
            appeared
        );
        assert_eq!(
            activity(&format!("***DiskDescriptionChanged {tail}")),
            appeared
        );
        assert_eq!(
            activity(&format!("***VolumeMount           {tail}")),
            appeared
        );
        assert_eq!(
            activity(&format!("***DiskDisappeared       {tail}")),
            disappeared
        );
        assert_eq!(
            activity(&format!("***VolumeUnmount         {tail}")),
            disappeared
        );
        assert_eq!(activity(&format!("***DiskAttributeChanged  {tail}")), None);
    }

    #[test]
    fn activity_line_with_leading_spaces_and_empty_fields() {
        assert_eq!(
            activity("  ***DiskAppeared ('disk2', '', '')"),
            Some((ActivityKind::Appeared, "disk2".to_string()))
        );
    }

    #[test]
    fn activity_line_rejects_malformed_input() {
        assert_eq!(activity(""), None);
        assert_eq!(activity("DiskAppeared ('disk2', '')"), None);
        assert_eq!(activity("***DiskAppeared('disk2', '')"), None);
        assert_eq!(activity("***DiskAppeared ('', 'x')"), None);
        assert_eq!(activity("***DiskAppeared ('disk2' 'x')"), None);
    }

    // ---- scanning /Volumes ------------------------------------------------------------------

    struct FakeSource {
        infos: HashMap<String, Result<Dictionary, String>>,
        calls: AtomicUsize,
    }

    impl FakeSource {
        fn new(infos: Vec<(&str, Result<Dictionary, String>)>) -> Arc<Self> {
            Arc::new(FakeSource {
                infos: infos.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
                calls: AtomicUsize::new(0),
            })
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl VolumeInfoSource for FakeSource {
        fn info(&self, mount_point: &Path) -> Result<Dictionary, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let name = mount_point
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            self.infos
                .get(&name)
                .cloned()
                .unwrap_or_else(|| Err("unknown volume".into()))
        }
    }

    fn no_free_space() -> FreeSpaceFn {
        Arc::new(|_| None)
    }

    fn volumes(names: &[&str]) -> TempDir {
        let root = TempDir::new().unwrap();
        for name in names {
            fs::create_dir(root.path().join(name)).unwrap();
        }
        root
    }

    #[test]
    fn scan_lists_qualifying_volumes_in_name_order() {
        let root = volumes(&[
            "WALKMAN",
            "Backup",
            "Macintosh HD",
            ".hidden",
            "Excluded By Info",
        ]);
        fs::write(root.path().join("a-file"), "").unwrap();
        let source = FakeSource::new(vec![
            ("WALKMAN", Ok(usb_info("WALKMAN", "/Volumes/WALKMAN"))),
            ("Backup", Ok(usb_info("Backup", "/Volumes/Backup"))),
            ("Excluded By Info", Ok(usb_info("Recovery", "/Volumes/x"))),
        ]);
        let scanner = DriveScanner::new(root.path().into(), source.clone(), no_free_space());

        let names: Vec<String> = scanner
            .scan()
            .unwrap()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(names, ["Backup", "WALKMAN"]);
        // Never asked about the excluded name, the hidden entry or the plain file.
        assert_eq!(source.calls(), 3);
    }

    #[test]
    fn scan_with_no_volumes_is_empty() {
        let root = volumes(&[]);
        let scanner =
            DriveScanner::new(root.path().into(), FakeSource::new(vec![]), no_free_space());
        assert!(scanner.scan().unwrap().is_empty());
    }

    #[test]
    fn scan_of_a_missing_root_is_an_error() {
        let root = TempDir::new().unwrap();
        let scanner = DriveScanner::new(
            root.path().join("nope"),
            FakeSource::new(vec![]),
            no_free_space(),
        );
        assert!(scanner.scan().unwrap_err().contains("Could not list"));
    }

    #[test]
    fn repeated_scans_reuse_the_cached_verdicts() {
        let root = volumes(&["USB", "Weird"]);
        let source = FakeSource::new(vec![
            ("USB", Ok(usb_info("USB", "/Volumes/USB"))),
            ("Weird", Ok(usb_info("Recovery", "/Volumes/Weird"))), // excluded by info
        ]);
        let scanner = DriveScanner::new(root.path().into(), source.clone(), no_free_space());
        for _ in 0..5 {
            assert_eq!(scanner.scan().unwrap().len(), 1);
        }
        assert_eq!(
            source.calls(),
            2,
            "diskutil is asked once per volume, not once per poll"
        );
    }

    #[test]
    fn free_space_is_refreshed_on_every_scan() {
        let root = volumes(&["USB"]);
        let source = FakeSource::new(vec![("USB", Ok(usb_info("USB", "/Volumes/USB")))]);
        let free = Arc::new(AtomicUsize::new(100));
        let free_for_fn = Arc::clone(&free);
        let scanner = DriveScanner::new(
            root.path().into(),
            source.clone(),
            Arc::new(move |_| Some(free_for_fn.load(Ordering::SeqCst) as u64)),
        );
        assert_eq!(scanner.scan().unwrap()[0].free_space, Some(100));
        free.store(42, Ordering::SeqCst);
        assert_eq!(scanner.scan().unwrap()[0].free_space, Some(42));
        assert_eq!(source.calls(), 1);
    }

    #[test]
    fn diskutil_free_space_is_used_when_statvfs_cant_answer() {
        let root = volumes(&["USB"]);
        let source = FakeSource::new(vec![("USB", Ok(usb_info("USB", "/Volumes/USB")))]);
        let scanner = DriveScanner::new(root.path().into(), source, no_free_space());
        assert_eq!(scanner.scan().unwrap()[0].free_space, Some(16_000_000_000));
    }

    #[test]
    fn a_failed_diskutil_call_is_skipped_then_retried_later() {
        let root = volumes(&["Flaky"]);
        let source = FakeSource::new(vec![("Flaky", Err("diskutil timed out".into()))]);
        let scanner = DriveScanner::new(root.path().into(), source.clone(), no_free_space());
        assert!(scanner.scan().unwrap().is_empty());
        assert!(scanner.scan().unwrap().is_empty());
        assert_eq!(source.calls(), 1, "failures are remembered for a while");

        // Expire the failure by hand and the volume is asked about again.
        scanner
            .cache
            .lock()
            .unwrap()
            .get_mut("Flaky")
            .unwrap()
            .retry_at = Some(Instant::now());
        scanner.scan().unwrap();
        assert_eq!(source.calls(), 2);
    }

    #[test]
    fn a_new_device_under_the_same_name_is_looked_at_again() {
        let root = volumes(&["USB"]);
        let source = FakeSource::new(vec![("USB", Ok(usb_info("USB", "/Volumes/USB")))]);
        let scanner = DriveScanner::new(root.path().into(), source.clone(), no_free_space());
        scanner.scan().unwrap();
        scanner.cache.lock().unwrap().get_mut("USB").unwrap().dev ^= 1;
        scanner.scan().unwrap();
        assert_eq!(source.calls(), 2);
    }

    #[test]
    fn unmounted_volumes_leave_the_cache() {
        let root = volumes(&["USB"]);
        let source = FakeSource::new(vec![("USB", Ok(usb_info("USB", "/Volumes/USB")))]);
        let scanner = DriveScanner::new(root.path().into(), source.clone(), no_free_space());
        scanner.scan().unwrap();
        fs::remove_dir(root.path().join("USB")).unwrap();
        assert!(scanner.scan().unwrap().is_empty());
        assert!(scanner.cache.lock().unwrap().is_empty());
        fs::create_dir(root.path().join("USB")).unwrap();
        assert_eq!(scanner.scan().unwrap().len(), 1);
        assert_eq!(source.calls(), 2);
    }

    #[test]
    fn a_hung_volume_cannot_stall_the_scan() {
        struct Hang;
        impl VolumeInfoSource for Hang {
            fn info(&self, mount_point: &Path) -> Result<Dictionary, String> {
                if mount_point.ends_with("Hung") {
                    thread::sleep(Duration::from_secs(30));
                }
                Ok(usb_info(
                    &mount_point.file_name().unwrap().to_string_lossy(),
                    "/Volumes/x",
                ))
            }
        }
        let root = volumes(&["Fine", "Hung"]);
        let scanner = DriveScanner::new(root.path().into(), Arc::new(Hang), no_free_space())
            .with_deadline(Duration::from_millis(300));
        let started = Instant::now();
        let names: Vec<String> = scanner
            .scan()
            .unwrap()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "took {:?}",
            started.elapsed()
        );
        assert_eq!(names, ["Fine"]);
    }

    #[test]
    fn drive_at_answers_for_one_mount_point() {
        let source = FakeSource::new(vec![("USB", Ok(usb_info("USB", "/Volumes/USB")))]);
        let scanner = DriveScanner::new(PathBuf::from("/nonexistent"), source, no_free_space());
        assert_eq!(
            scanner.drive_at(Path::new("/Volumes/USB")).unwrap().name,
            "USB"
        );
        assert!(scanner.drive_at(Path::new("/Volumes/Other")).is_none());
    }

    #[test]
    fn statvfs_reports_free_space_for_a_real_directory() {
        let dir = TempDir::new().unwrap();
        assert!(statvfs_free_space(dir.path()).is_some_and(|b| b > 0));
        assert!(statvfs_free_space(&dir.path().join("missing")).is_none());
    }
}
