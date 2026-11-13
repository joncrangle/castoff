//! Small filesystem helpers shared by the drive scan, the sync and the delete.

use std::fs;
use std::io;
use std::path::Path;

/// Files macOS (and its tools) scatter on volumes, which are never real content.
pub const SYSTEM_HIDDEN_FILES: [&str; 9] = [
    ".DS_Store",
    ".Spotlight-V100",
    ".Trashes",
    ".fseventsd",
    ".TemporaryItems",
    ".VolumeIcon.icns",
    ".com.apple.timemachine.donotpresent",
    ".DocumentRevisions-V100",
    ".PKInstallSandboxManager",
];

pub const AUDIO_EXTENSIONS: [&str; 6] = [".mp3", ".m4a", ".wav", ".aac", ".ogg", ".flac"];

/// Suffix of the in-progress copy of a file. The sync copies to `<dest>.castoff-part` and renames
/// when complete, so an interrupted run never leaves a short file under the real name (which a
/// later run would mistake for a finished one).
pub const PARTIAL_SUFFIX: &str = ".castoff-part";

/// `.DS_Store`-style names and AppleDouble (`._*`) files.
pub fn is_system_hidden_file(name: &str) -> bool {
    SYSTEM_HIDDEN_FILES.contains(&name) || name.starts_with("._")
}

/// Whether the file name ends in a known audio extension (case-insensitive).
pub fn is_audio_file(name: &str) -> bool {
    match name.rfind('.') {
        Some(idx) => {
            let ext = name[idx..].to_lowercase();
            AUDIO_EXTENSIONS.contains(&ext.as_str())
        }
        None => false,
    }
}

/// The file name's extension including the dot (case preserved), or ".mp3" when it has none.
///
/// (Only the file name is looked at, so a dot in a folder name, as in `/a.b/file`, isn't taken
/// for an extension.)
pub fn get_extension(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match name.rfind('.') {
        Some(idx) if idx > 0 => name[idx..].to_string(),
        _ => ".mp3".to_string(),
    }
}

/// The size of a file, or 0 when it can't be read.
pub fn file_size(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Removes the system hidden files directly inside `dir`. Per-file failures (and hidden
/// directories, which `unlink` can't remove) are ignored.
pub fn cleanup_system_hidden_files(dir: &Path) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if let Some(name) = entry.file_name().to_str()
            && is_system_hidden_file(name)
        {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(())
}

/// Whether `dir` holds nothing but system hidden entries. An unreadable directory is reported as
/// *not* empty, so a directory that couldn't be looked into is never deleted.
pub fn is_dir_empty(dir: &Path) -> bool {
    match fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .all(|e| e.file_name().to_str().is_some_and(is_system_hidden_file)),
        Err(_) => false,
    }
}

/// Hidden-file cleanup on `dir`, then removal of `dir` if nothing visible is left in it.
/// Returns whether the directory was removed. All errors are swallowed.
pub fn remove_dir_if_empty(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let _ = cleanup_system_hidden_files(dir);
    if is_dir_empty(dir) {
        return fs::remove_dir_all(dir).is_ok();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn hidden_file_names() {
        assert!(is_system_hidden_file(".DS_Store"));
        assert!(is_system_hidden_file(".Trashes"));
        assert!(is_system_hidden_file("._episode.mp3"));
        assert!(!is_system_hidden_file("episode.mp3"));
        assert!(!is_system_hidden_file(".hidden"));
    }

    #[test]
    fn audio_extensions() {
        for ok in [
            "a.mp3", "a.M4A", "a.wav", "a.aac", "a.ogg", "a.flac", "x.y.mp3",
        ] {
            assert!(is_audio_file(ok), "{ok}");
        }
        for no in ["notes.txt", "mp3", "a.mp3.part", "", "a."] {
            assert!(!is_audio_file(no), "{no}");
        }
    }

    #[test]
    fn extensions_default_to_mp3() {
        assert_eq!(get_extension("/path/to/ep.m4a"), ".m4a");
        assert_eq!(get_extension("/path/to/ep.MP3"), ".MP3");
        assert_eq!(get_extension("/path/to/ep"), ".mp3");
        assert_eq!(get_extension("/some.dir/ep"), ".mp3");
        assert_eq!(get_extension("/path/.hidden"), ".mp3");
    }

    #[test]
    fn dir_emptiness_ignores_hidden_files() {
        let dir = TempDir::new().unwrap();
        assert!(is_dir_empty(dir.path()));
        fs::write(dir.path().join(".DS_Store"), "").unwrap();
        fs::write(dir.path().join("._x.mp3"), "").unwrap();
        assert!(is_dir_empty(dir.path()));
        fs::write(dir.path().join("file.txt"), "").unwrap();
        assert!(!is_dir_empty(dir.path()));
        assert!(!is_dir_empty(&dir.path().join("missing")));
    }

    #[test]
    fn cleanup_removes_only_hidden_files() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(".DS_Store"), "").unwrap();
        fs::write(dir.path().join("._ep.mp3"), "").unwrap();
        fs::write(dir.path().join("ep.mp3"), "x").unwrap();
        fs::write(dir.path().join(".keep"), "x").unwrap();
        cleanup_system_hidden_files(dir.path()).unwrap();
        let mut left: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, [".keep", "ep.mp3"]);
    }

    #[test]
    fn remove_dir_if_empty_cases() {
        let dir = TempDir::new().unwrap();

        let only_hidden = dir.path().join("hidden-only");
        fs::create_dir_all(only_hidden.join(".Trashes")).unwrap();
        fs::write(only_hidden.join(".DS_Store"), "").unwrap();
        assert!(remove_dir_if_empty(&only_hidden));
        assert!(!only_hidden.exists());

        let has_file = dir.path().join("has-file");
        fs::create_dir(&has_file).unwrap();
        fs::write(has_file.join(".DS_Store"), "").unwrap();
        fs::write(has_file.join("ep.mp3"), "x").unwrap();
        assert!(!remove_dir_if_empty(&has_file));
        assert!(has_file.join("ep.mp3").exists());
        assert!(!has_file.join(".DS_Store").exists());

        assert!(!remove_dir_if_empty(&dir.path().join("missing")));
    }
}
