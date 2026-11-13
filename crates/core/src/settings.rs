//! The settings file, `~/.config/castoff/castoff.jsonc`.
//!
//! The file is JSON with comments. Loading never fails (a missing or unparsable file gives the
//! defaults); saving rewrites the file as plain pretty-printed JSON with the `$schema` line
//! first, keeping every key this crate doesn't know about.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::api::Settings;

/// The schema named in the file, for editor completion and validation.
pub const SCHEMA_URL: &str =
    "https://raw.githubusercontent.com/joncrangle/castoff/main/schemas/castoff.schema.json";

const THEME_KEY: &str = "theme";
const FAVORITES_KEY: &str = "favoriteDrives";
const SCHEMA_KEY: &str = "$schema";

/// `~/.config/castoff/castoff.jsonc`.
pub fn default_settings_path() -> PathBuf {
    let home = std::env::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".config").join("castoff").join("castoff.jsonc")
}

/// Removes `//` and `/* */` comments, and trailing commas, outside of string literals.
pub fn strip_jsonc(text: &str) -> String {
    let without_comments = strip_comments(text);
    strip_trailing_commas(&without_comments)
}

fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(escaped) = chars.next() {
                    out.push(escaped);
                }
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = '\0';
                for next in chars.by_ref() {
                    if prev == '*' && next == '/' {
                        break;
                    }
                    prev = next;
                }
                out.push(' ');
            }
            _ => out.push(c),
        }
    }
    out
}

fn strip_trailing_commas(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(&escaped) = chars.get(i + 1) {
                    out.push(escaped);
                    i += 1;
                }
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
            out.push(c);
        } else if c == ',' {
            let next = chars[i + 1..].iter().find(|ch| !ch.is_whitespace());
            if !matches!(next, Some('}') | Some(']')) {
                out.push(c);
            }
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// Parses the file's text into its top-level object. `None` unless it holds a JSON(C) object.
fn parse_object(text: &str) -> Option<Map<String, Value>> {
    match serde_json::from_str::<Value>(&strip_jsonc(text)) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    }
}

fn settings_from_object(map: &Map<String, Value>) -> Settings {
    let defaults = Settings::default();
    let theme = match map.get(THEME_KEY) {
        Some(Value::String(t)) if !t.is_empty() => t.clone(),
        _ => defaults.theme,
    };
    let favorite_drives = match map.get(FAVORITES_KEY) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    };
    Settings {
        theme,
        favorite_drives,
    }
}

/// Loads settings from `path`. Never fails: anything unreadable gives `Settings::default()`.
pub fn load_settings_from(path: &Path) -> Settings {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| parse_object(&text))
        .map(|map| settings_from_object(&map))
        .unwrap_or_default()
}

/// Writes `settings` to `path`, merging them into whatever the file already holds so unknown keys
/// (and the existing `$schema` value) survive. Comments are not preserved.
///
/// If the file exists but can't be parsed it is copied to `<name>.bak` before being replaced,
/// rather than silently losing the user's hand-written content.
pub fn save_settings_to(path: &Path, settings: &Settings) -> Result<(), String> {
    let fail = |what: &str, e: std::io::Error| format!("Failed to save settings: {what}: {e}");

    let existing_text = fs::read_to_string(path).ok();
    let existing = existing_text.as_deref().and_then(parse_object);
    if existing.is_none()
        && let Some(text) = &existing_text
        && !text.trim().is_empty()
    {
        let mut backup = path.as_os_str().to_owned();
        backup.push(".bak");
        fs::write(&backup, text)
            .map_err(|e| fail("could not back up the unparsable settings file", e))?;
    }
    let existing = existing.unwrap_or_default();

    let mut merged = Map::new();
    merged.insert(
        SCHEMA_KEY.to_string(),
        existing
            .get(SCHEMA_KEY)
            .cloned()
            .unwrap_or_else(|| Value::String(SCHEMA_URL.to_string())),
    );
    for (key, value) in &existing {
        if key != SCHEMA_KEY {
            merged.insert(key.clone(), value.clone());
        }
    }
    merged.insert(THEME_KEY.to_string(), Value::String(settings.theme.clone()));
    merged.insert(
        FAVORITES_KEY.to_string(),
        Value::Array(
            settings
                .favorite_drives
                .iter()
                .cloned()
                .map(Value::String)
                .collect(),
        ),
    );

    let json = serde_json::to_string_pretty(&Value::Object(merged))
        .map_err(|e| format!("Failed to save settings: {e}"))?;

    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(dir).map_err(|e| fail("could not create the settings directory", e))?;

    // Write beside the target, then rename, so a crash can't leave a half-written file.
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp-{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, json).map_err(|e| fail("could not write the settings file", e))?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        fail("could not replace the settings file", e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn settings(theme: &str, favs: &[&str]) -> Settings {
        Settings {
            theme: theme.into(),
            favorite_drives: favs.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn loads_jsonc_with_comments() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("castoff.jsonc");
        fs::write(
            &path,
            r#"
            // This is a test comment
            {
                "theme": "Nord", /* Another comment */
                "favoriteDrives": ["/Volumes/USB"]
            }
            "#,
        )
        .unwrap();
        assert_eq!(
            load_settings_from(&path),
            settings("Nord", &["/Volumes/USB"])
        );
    }

    #[test]
    fn comment_markers_inside_strings_survive() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("s.jsonc");
        fs::write(
            &path,
            r#"{ "theme": "a // b /* c */ \" // d", "favoriteDrives": ["http://x"] // tail
            }"#,
        )
        .unwrap();
        assert_eq!(
            load_settings_from(&path),
            settings("a // b /* c */ \" // d", &["http://x"])
        );
    }

    #[test]
    fn trailing_commas_are_tolerated() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("s.jsonc");
        fs::write(
            &path,
            r#"{ "theme": "Nord", "favoriteDrives": ["a", "b",], }"#,
        )
        .unwrap();
        assert_eq!(load_settings_from(&path), settings("Nord", &["a", "b"]));
    }

    #[test]
    fn missing_or_broken_file_gives_defaults() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("nope.jsonc");
        assert_eq!(load_settings_from(&missing), Settings::default());

        let broken = dir.path().join("broken.jsonc");
        fs::write(&broken, "{ not json").unwrap();
        assert_eq!(load_settings_from(&broken), Settings::default());

        let array = dir.path().join("array.jsonc");
        fs::write(&array, "[1, 2]").unwrap();
        assert_eq!(load_settings_from(&array), Settings::default());
    }

    #[test]
    fn partial_files_fill_in_defaults() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("s.jsonc");
        fs::write(&path, r#"{ "favoriteDrives": ["x", 5, null] }"#).unwrap();
        assert_eq!(load_settings_from(&path), settings("Catppuccin", &["x"]));
        fs::write(&path, r#"{ "theme": "Nord" }"#).unwrap();
        assert_eq!(load_settings_from(&path), settings("Nord", &[]));
        fs::write(&path, r#"{ "theme": 4, "favoriteDrives": "x" }"#).unwrap();
        assert_eq!(load_settings_from(&path), Settings::default());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("castoff.jsonc");
        let s = settings("Dracula", &["UUID-1", "UUID-2"]);
        save_settings_to(&path, &s).unwrap();
        assert_eq!(load_settings_from(&path), s);
    }

    #[test]
    fn save_writes_the_schema_line_first_in_the_tui_format() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("castoff.jsonc");
        save_settings_to(&path, &settings("Nord", &["a"])).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let expected = format!(
            "{{\n  \"$schema\": \"{SCHEMA_URL}\",\n  \"theme\": \"Nord\",\n  \"favoriteDrives\": [\n    \"a\"\n  ]\n}}"
        );
        assert_eq!(text, expected);

        save_settings_to(&path, &settings("Nord", &[])).unwrap();
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("\"favoriteDrives\": []")
        );
    }

    #[test]
    fn save_preserves_unknown_keys_order_and_existing_schema() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("castoff.jsonc");
        fs::write(
            &path,
            r#"// my config
            {
                "zebra": {"deep": [1, 2, {"x": null}]},
                "theme": "Nord", /* keep */
                "$schema": "https://example.test/custom.json",
                "alpha": true,
                "favoriteDrives": ["old"]
            }"#,
        )
        .unwrap();

        save_settings_to(&path, &settings("Catppuccin", &["new"])).unwrap();

        let saved: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let obj = saved.as_object().unwrap();
        let keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["$schema", "zebra", "theme", "alpha", "favoriteDrives"]
        );
        assert_eq!(obj["$schema"], "https://example.test/custom.json");
        assert_eq!(obj["zebra"]["deep"][2]["x"], Value::Null);
        assert_eq!(obj["alpha"], true);
        assert_eq!(obj["theme"], "Catppuccin");
        assert_eq!(obj["favoriteDrives"], serde_json::json!(["new"]));
        // No leftover temp file.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn saving_over_an_unparsable_file_keeps_a_backup() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("castoff.jsonc");
        fs::write(&path, "{ this is: not json").unwrap();
        save_settings_to(&path, &settings("Nord", &[])).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("castoff.jsonc.bak")).unwrap(),
            "{ this is: not json"
        );
        assert_eq!(load_settings_from(&path).theme, "Nord");
    }

    #[test]
    fn saving_into_an_unwritable_place_reports_an_error() {
        let dir = TempDir::new().unwrap();
        // The "directory" is really a file.
        let blocker = dir.path().join("blocker");
        fs::write(&blocker, "x").unwrap();
        let err =
            save_settings_to(&blocker.join("castoff.jsonc"), &Settings::default()).unwrap_err();
        assert!(err.starts_with("Failed to save settings"), "{err}");
    }
}
