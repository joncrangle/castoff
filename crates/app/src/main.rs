//! Castoff: copies downloaded Apple Podcasts episodes to an external drive. Built on Rune.
//!
//! `cargo run -p castoff` uses the real library and drives; `-- --demo` (or `CASTOFF_DEMO=1`)
//! runs on made-up data, as every headless run does:
//! `target/debug/castoff --snap out/x.png --scene transfer`, `--list-scenes`.

mod demo;
mod icons;
mod keys;
mod logic;
mod state;
mod theme;
mod view;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use castoff_core::{Backend, LiveBackend, Settings, TransferProgress};
use rune::{AppSpec, Waker};

use crate::demo::{DemoBackend, Timing};
use crate::state::{AppView, DebugMessage, Level, Pane, State};
use crate::theme::THEMES;

fn flag(name: &str) -> bool {
    rune::tooling::extra_args().iter().any(|a| a == name)
}

fn env_on(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| matches!(v.as_str(), "1" | "true" | "yes"))
}

fn init() -> State {
    let headless = rune::tooling::headless_run().is_some();
    let backend: Arc<dyn Backend> = if headless {
        Arc::new(DemoBackend::new(Timing::RECORDED))
    } else if flag("--demo") || env_on("CASTOFF_DEMO") {
        Arc::new(DemoBackend::new(Timing::REALISTIC))
    } else {
        Arc::new(LiveBackend::new())
    };
    let mut s = State::new(backend, headless);
    s.paced = headless;
    s.debug_enabled = env_on("DEBUG") || flag("--debug");
    s
}

fn setup(s: &mut State, waker: Waker) {
    s.jobs.set_waker(waker);
}

// ---------------------------------------------------------------------------------------------
// Scenes: each starts from `init` (demo backend, jobs run inline) and builds its state by running
// the app's own logic to completion, so it renders the same on every run.

/// Starts up and applies every answer, as the first frames would.
pub fn settle(s: &mut State) {
    logic::boot(s);
    for _ in 0..16 {
        let msgs = s.jobs.drain();
        if msgs.is_empty() {
            break;
        }
        for m in msgs {
            logic::apply(s, m);
        }
    }
    // Scene snapshots are taken at once; the periodic drive check would change nothing.
    s.watch_drives = false;
}

fn with_backend(s: &mut State, b: DemoBackend) {
    s.backend = Arc::new(b);
}

fn fixed_log(s: &mut State) {
    // 2026-01-23 09:41:00 UTC onwards, so the log reads the same on every run.
    let base = 1_769_161_260_000i64;
    let lines: &[(&str, Level)] = &[
        ("Loaded settings: theme Catppuccin, 1 favourite drive", Level::Info),
        ("Loaded 37 Mac episodes", Level::Info),
        ("Found 3 drives", Level::Info),
        ("Loaded 9 drive episodes", Level::Info),
        ("Drive appeared: SANDISK CLIP (demo-clip)", Level::Info),
        ("Slow scan of /Volumes/SANDISK CLIP (2.4 s)", Level::Warn),
        ("Syncing 3 episode(s) to WALKMAN", Level::Info),
        ("Sync complete: 3 copied, 0 failed", Level::Info),
        ("Could not tag \"Insomnia Requests Hour.mp3\": ID3 frame too large", Level::Error),
        ("Drive disappeared: ECHO MINI (demo-echo)", Level::Info),
        ("Copied to clipboard", Level::Info),
    ];
    s.debug_messages = lines
        .iter()
        .enumerate()
        .map(|(i, (m, l))| DebugMessage { timestamp: base + i as i64 * 7_300, message: m.to_string(), level: *l })
        .collect();
}

fn select_some(s: &mut State, pane: Pane, rows: &[usize]) {
    for &i in rows {
        if let Some(e) = s.list_mut(pane).get_mut(i) {
            e.selected = true;
        }
    }
}

fn theme_scene(s: &mut State, name: &str) {
    with_backend(
        s,
        DemoBackend::instant().with_settings(Settings { theme: name.into(), favorite_drives: vec!["demo-walkman".into()] }),
    );
    settle(s);
    s.mac_index = 2;
    select_some(s, Pane::Mac, &[2, 5]);
}

/// A named starting state for headless renders (`--scene`).
type Scene = (&'static str, fn(&mut State));

const SCENES: &[Scene] = &[
    ("default", |s| {
        settle(s);
        s.mac_index = 3;
        select_some(s, Pane::Mac, &[1, 3, 4]);
    }),
    ("drive-focused", |s| {
        settle(s);
        s.focused = Pane::Drive;
        s.drive_index = 2;
        select_some(s, Pane::Drive, &[0, 2, 3]);
        s.mac_index = 4;
    }),
    ("loading", |s| {
        s.booted = true;
        s.watch_drives = false;
        s.loading_mac = true;
        s.loading_drive = true;
    }),
    ("no-drive", |s| {
        let b = DemoBackend::instant();
        for id in ["demo-walkman", "demo-echo", "demo-clip"] {
            b.unplug(id);
        }
        with_backend(s, b);
        settle(s);
        s.mac_index = 1;
    }),
    ("empty-library", |s| {
        settle(s);
        s.mac.clear();
        s.drive_eps.clear();
    }),
    ("error", |s| {
        settle(s);
        s.error = "Could not open the Podcasts library: database is locked (MTLibrary.sqlite)".into();
    }),
    ("drive-selector", |s| {
        with_backend(
            s,
            DemoBackend::instant().with_settings(Settings { theme: "Catppuccin".into(), favorite_drives: vec!["demo-echo".into()] }),
        );
        settle(s);
        s.view = AppView::DriveSelection;
        s.drive_menu_index = 1;
    }),
    ("drive-scanning", |s| {
        settle(s);
        s.view = AppView::DriveSelection;
        s.scanning = true;
    }),
    ("transfer", |s| {
        settle(s);
        select_some(s, Pane::Mac, &[0, 1, 2, 3, 4]);
        s.view = AppView::Syncing;
        s.progress = TransferProgress {
            current_file: "Ep. 212 — The Radio Telescope in the Desert.mp3".into(),
            files_done: 2,
            total_files: 5,
            bytes_transferred: 131_400_000,
            total_bytes: 318_700_000,
            speed: 41_900_000.0,
        };
    }),
    ("confirm-delete", |s| {
        settle(s);
        s.focused = Pane::Drive;
        s.drive_index = 1;
        select_some(s, Pane::Drive, &[1, 4]);
        s.view = AppView::Confirm;
    }),
    ("theme-picker", |s| {
        settle(s);
        s.view = AppView::ThemeSelection;
        s.theme_menu_index = THEMES.iter().position(|(n, _)| *n == "Tokyo Night").unwrap_or(0);
        s.theme = "Tokyo Night".into();
        s.reveal_pending = true;
    }),
    ("debug-log", |s| {
        settle(s);
        s.debug_enabled = true;
        s.last_key = Some("F1".into());
        fixed_log(s);
        s.view = AppView::Debug;
        s.debug_menu_index = 8;
    }),
    ("debug-mode", |s| {
        settle(s);
        s.debug_enabled = true;
        s.last_key = Some("Ctrl+t".into());
    }),
    ("theme-gruvbox", |s| theme_scene(s, "Gruvbox")),
    ("theme-nord", |s| theme_scene(s, "Nord")),
    ("theme-solarized", |s| theme_scene(s, "Solarized Dark")),
    ("theme-rose-pine", |s| theme_scene(s, "Rosé Pine")),
    ("theme-system", |s| theme_scene(s, "System")),
    ("theme-zenburn", |s| theme_scene(s, "Zenburn")),
];

pub fn spec() -> AppSpec<State> {
    AppSpec {
        scenes: SCENES,
        setup: Some(setup),
        poll: Some(logic::poll),
        on_key: Some(keys::on_key),
        min_size: Some((640.0, 440.0)),
        ..AppSpec::new("Castoff", (1180.0, 760.0), init, view::view)
    }
}

fn main() {
    // Before the window: `castoff --version`, as the release tooling and package tests expect.
    if std::env::args().skip(1).any(|a| a == "--version" || a == "-V") {
        println!("castoff v{}", env!("CARGO_PKG_VERSION"));
        return;
    }
    rune::App::new(spec()).chrome(rune::Chrome { traffic_lights: Some((18.0, 21.0)), ..rune::Chrome::DEFAULT }).run();
}
