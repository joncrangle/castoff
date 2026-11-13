//! The app driven as a person would, through `rune::Runtime`: keys, clicks and frames, against
//! the demo backend. Most tests run jobs inline (deterministic); the sync tests run them on
//! threads, as the window does, and wait on the real clock for the answers.

use std::sync::Arc;
use std::time::{Duration, Instant};

use castoff_core::{Backend, Settings};
use rune::{Env, Key, KeyEvent, Mods, Runtime};

use crate::demo::{DemoBackend, Timing};
use crate::keys;
use crate::logic;
use crate::state::{AppView, Pane, State};
use crate::view::view;

const DT: f64 = 1.0 / 60.0;

fn runtime(backend: Arc<DemoBackend>, inline: bool) -> Runtime<State> {
    let s = State::new(backend as Arc<dyn Backend>, inline);
    let mut rt = Runtime::new(s, view, (1180.0, 760.0), Env::new(true, 1.0));
    rt.poll = Some(logic::poll);
    rt.on_key = Some(keys::on_key);
    rt.frame(0.0);
    for _ in 0..3 {
        rt.frame(DT);
    }
    rt
}

fn demo() -> Arc<DemoBackend> {
    Arc::new(DemoBackend::instant())
}

fn started() -> (Runtime<State>, Arc<DemoBackend>) {
    let b = demo();
    (runtime(b.clone(), true), b)
}

fn press(rt: &mut Runtime<State>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    rt.frame(DT);
    rt.frame(DT);
}

fn ch(rt: &mut Runtime<State>, c: &str) {
    press(rt, Key::Text(c.into()), Mods::default());
}

fn ctrl(rt: &mut Runtime<State>, c: &str) {
    press(rt, Key::Text(c.into()), Mods { ctrl: true, ..Mods::default() });
}

fn key(rt: &mut Runtime<State>, k: Key) {
    press(rt, k, Mods::default());
}

fn click(rt: &mut Runtime<State>, k: &str) {
    let (x, y) = rt.find(k).unwrap_or_else(|| panic!("no element keyed {k}"));
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(DT);
    rt.frame(DT);
}

/// Frames until `done`, on the real clock (threaded jobs answer in their own time).
fn until(rt: &mut Runtime<State>, what: &str, done: impl Fn(&State) -> bool) {
    let start = Instant::now();
    while !done(&rt.state) {
        assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        rt.frame(DT);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn startup_loads_the_library_finds_drives_and_picks_the_favourite() {
    let b = Arc::new(DemoBackend::instant().with_settings(Settings {
        theme: "Nord".into(),
        favorite_drives: vec!["demo-echo".into()],
    }));
    let rt = runtime(b, true);
    let s = &rt.state;
    assert!(!s.loading_mac && !s.loading_drive && !s.scanning);
    assert_eq!(s.mac.len(), 37);
    assert_eq!(s.theme, "Nord");
    assert_eq!(s.drives[0].id, "demo-echo", "favourites sort first");
    assert_eq!(s.current_drive.as_ref().unwrap().id, "demo-echo");
    assert_eq!(s.drive_eps.len(), 3);
    assert!(rt.find("mac-row-0").is_some() && rt.find("drive-row-2").is_some());
}

#[test]
fn without_a_favourite_the_first_drive_is_chosen() {
    let (rt, _) = started();
    assert_eq!(rt.state.current_drive.as_ref().unwrap().name, "WALKMAN");
    assert_eq!(rt.state.drive_eps.len(), 9);
}

#[test]
fn keys_move_the_cursor_and_switch_panes() {
    let (mut rt, _) = started();
    ch(&mut rt, "j");
    ch(&mut rt, "j");
    key(&mut rt, Key::Down);
    assert_eq!(rt.state.mac_index, 3);
    ch(&mut rt, "k");
    key(&mut rt, Key::Up);
    assert_eq!(rt.state.mac_index, 1);
    for _ in 0..5 {
        ch(&mut rt, "k");
    }
    assert_eq!(rt.state.mac_index, 0, "stops at the top");

    ch(&mut rt, "l");
    assert_eq!(rt.state.focused, Pane::Drive);
    ch(&mut rt, "j");
    assert_eq!((rt.state.drive_index, rt.state.mac_index), (1, 0), "moves the focused pane only");
    ch(&mut rt, "h");
    assert_eq!(rt.state.focused, Pane::Mac);
    key(&mut rt, Key::Right);
    assert_eq!(rt.state.focused, Pane::Drive);
    key(&mut rt, Key::Left);
    assert_eq!(rt.state.focused, Pane::Mac);
    key(&mut rt, Key::Tab);
    assert_eq!(rt.state.focused, Pane::Drive);
    key(&mut rt, Key::Tab);
    assert_eq!(rt.state.focused, Pane::Mac);
}

#[test]
fn ctrl_d_and_ctrl_u_page_and_the_cursor_stays_in_view() {
    let (mut rt, _) = started();
    let page = rt.state.page_rows;
    assert!(page >= 3, "page from the list's height, got {page}");
    ctrl(&mut rt, "d");
    assert_eq!(rt.state.mac_index, page);
    ctrl(&mut rt, "d");
    ctrl(&mut rt, "d");
    ctrl(&mut rt, "d");
    ctrl(&mut rt, "d");
    ctrl(&mut rt, "d");
    assert_eq!(rt.state.mac_index, 36, "stops at the end");
    for _ in 0..60 {
        rt.frame(DT);
    }
    let row = rt.presented("mac-row-36").unwrap().0;
    let list = rt.presented("mac-scroll").unwrap().0;
    assert!(row[1] >= list[1] - 0.5 && row[1] + row[3] <= list[1] + list[3] + 0.5, "row {row:?} inside {list:?}");
    ctrl(&mut rt, "u");
    assert_eq!(rt.state.mac_index, 36 - page);
    // The page keys do too.
    key(&mut rt, Key::PageUp);
    assert_eq!(rt.state.mac_index, 36 - 2 * page);
}

#[test]
fn space_a_and_escape_select() {
    let (mut rt, _) = started();
    ch(&mut rt, " ");
    assert!(rt.state.mac[0].selected);
    ch(&mut rt, " ");
    assert!(!rt.state.mac[0].selected);
    ch(&mut rt, "a");
    assert!(rt.state.mac.iter().all(|e| e.selected));
    ch(&mut rt, "a");
    assert!(rt.state.mac.iter().all(|e| !e.selected), "a toggles all off when all are on");
    ch(&mut rt, "j");
    ch(&mut rt, " ");
    ch(&mut rt, "j");
    ch(&mut rt, " ");
    assert_eq!(rt.state.selected(Pane::Mac).len(), 2);
    key(&mut rt, Key::Escape);
    assert!(rt.state.selected(Pane::Mac).is_empty());
    // On the drive side the same keys select drive files.
    key(&mut rt, Key::Tab);
    ch(&mut rt, "a");
    assert!(rt.state.drive_eps.iter().all(|e| e.selected));
    assert!(rt.state.mac.iter().all(|e| !e.selected));
}

#[test]
fn clicks_move_the_cursor_select_and_focus_panes() {
    let (mut rt, _) = started();
    click(&mut rt, "mac-row-2");
    assert_eq!(rt.state.mac_index, 2);
    assert!(!rt.state.mac[2].selected, "the first click moves the cursor");
    click(&mut rt, "mac-row-2");
    assert!(rt.state.mac[2].selected, "a click on the cursor row selects it");
    click(&mut rt, "drive-row-1");
    assert_eq!((rt.state.focused, rt.state.drive_index), (Pane::Drive, 1));
    assert!(!rt.state.drive_eps[1].selected);
    // The pane itself, away from rows, focuses.
    click(&mut rt, "pane-mac/head");
    assert_eq!(rt.state.focused, Pane::Mac);
    // The round check selects in one click.
    click(&mut rt, "drive-row-4/check");
    assert_eq!(rt.state.focused, Pane::Drive);
    assert!(rt.state.drive_eps[4].selected);
}

#[test]
fn footer_hints_are_buttons() {
    let (mut rt, _) = started();
    click(&mut rt, "footer-key-switch-list-tab");
    assert_eq!(rt.state.focused, Pane::Drive);
    click(&mut rt, "footer-hint-navigate/footer-key-navigate-↓");
    assert_eq!(rt.state.drive_index, 1);
    // The focused pane's own hints.
    click(&mut rt, "drive-key-all-a");
    assert!(rt.state.drive_eps.iter().all(|e| e.selected));
    click(&mut rt, "drive-key-deselect-esc");
    assert!(rt.state.drive_eps.iter().all(|e| !e.selected));
    click(&mut rt, "footer-key-change-theme-ctrl+t");
    assert_eq!(rt.state.view, AppView::ThemeSelection);
}

#[test]
fn the_drive_selector_scans_favourites_and_switches_drives() {
    let (mut rt, b) = started();
    ch(&mut rt, "f");
    assert_eq!(rt.state.view, AppView::DriveSelection);
    assert!(!rt.state.scanning, "the scan answered");
    assert!(rt.find("drive-item-2").is_some());
    ch(&mut rt, "j");
    assert_eq!(rt.state.drive_menu_index, 1);
    ctrl(&mut rt, "f");
    assert!(rt.state.is_favorite("demo-echo"));
    assert_eq!(b.saved_settings().favorite_drives, vec!["demo-echo".to_string()], "saved through the backend");
    key(&mut rt, Key::Enter);
    assert_eq!(rt.state.view, AppView::Normal);
    assert_eq!(rt.state.current_drive.as_ref().unwrap().id, "demo-echo");
    assert_eq!(rt.state.drive_eps.len(), 3);

    // Reopened, favourites sort first; a click moves, a second click chooses.
    ch(&mut rt, "f");
    assert_eq!(rt.state.drives[0].id, "demo-echo");
    click(&mut rt, "drive-item-2");
    assert_eq!(rt.state.drive_menu_index, 2);
    click(&mut rt, "drive-item-2");
    assert_eq!(rt.state.view, AppView::Normal);
    assert_eq!(rt.state.current_drive.as_ref().unwrap().id, "demo-clip");
    assert!(rt.state.drive_eps.is_empty());

    ch(&mut rt, "f");
    key(&mut rt, Key::Escape);
    assert_eq!(rt.state.view, AppView::Normal);
}

#[test]
fn the_theme_picker_previews_reverts_and_saves() {
    let (mut rt, b) = started();
    ctrl(&mut rt, "t");
    assert_eq!(rt.state.view, AppView::ThemeSelection);
    assert_eq!(rt.state.theme_menu_index, 1, "starts on the saved theme");
    ch(&mut rt, "j");
    assert_eq!(rt.state.theme, "Cyberdream", "previews live");
    key(&mut rt, Key::Escape);
    assert_eq!((rt.state.view, rt.state.theme.as_str()), (AppView::Normal, "Catppuccin"), "escape reverts");

    ctrl(&mut rt, "t");
    key(&mut rt, Key::Down);
    key(&mut rt, Key::Down);
    key(&mut rt, Key::Enter);
    assert_eq!((rt.state.theme.as_str(), rt.state.saved_theme.as_str()), ("Dracula", "Dracula"));
    assert_eq!(b.saved_settings().theme, "Dracula");

    // A click outside closes it like escape.
    ctrl(&mut rt, "t");
    click(&mut rt, "theme-item-8");
    assert_eq!(rt.state.theme, "Nord");
    rt.pointer_move(10.0, 700.0);
    rt.pointer_down(10.0, 700.0);
    rt.pointer_up(10.0, 700.0);
    rt.frame(DT);
    rt.frame(DT);
    assert_eq!((rt.state.view, rt.state.theme.as_str()), (AppView::Normal, "Dracula"));
}

#[test]
fn the_debug_log_opens_copies_and_closes() {
    let (mut rt, _) = started();
    let n = rt.state.debug_messages.len();
    assert!(n > 0, "startup logs");
    key(&mut rt, Key::Function(1));
    assert_eq!(rt.state.view, AppView::Debug);
    assert_eq!(rt.state.debug_menu_index, n - 1, "opens on the newest line");
    ch(&mut rt, "k");
    key(&mut rt, Key::Enter);
    assert_eq!(rt.state.debug_messages.last().unwrap().message, "Copied to clipboard");
    key(&mut rt, Key::Escape);
    assert_eq!(rt.state.view, AppView::Normal);
    // `x` opens it only in debug mode.
    ch(&mut rt, "x");
    assert_eq!(rt.state.view, AppView::Normal);
    rt.state.debug_enabled = true;
    ch(&mut rt, "x");
    assert_eq!(rt.state.view, AppView::Debug);
}

#[test]
fn delete_asks_first_then_removes_files_and_rescans() {
    let (mut rt, b) = started();
    key(&mut rt, Key::Tab);
    ch(&mut rt, "d");
    assert_eq!(rt.state.view, AppView::Normal, "nothing selected, nothing to confirm");
    ch(&mut rt, " ");
    ch(&mut rt, "j");
    ch(&mut rt, " ");
    ch(&mut rt, "d");
    assert_eq!(rt.state.view, AppView::Confirm);
    ch(&mut rt, "n");
    assert_eq!(rt.state.view, AppView::Normal);
    assert_eq!(b.files_on("demo-walkman").len(), 9);
    ch(&mut rt, "d");
    ch(&mut rt, "y");
    assert_eq!(rt.state.view, AppView::Normal);
    assert_eq!(b.files_on("demo-walkman").len(), 7);
    assert_eq!(rt.state.drive_eps.len(), 7, "rescanned");
    assert!(rt.state.drive_eps.iter().all(|e| !e.selected));
    // Enter confirms, and the dialog's buttons work too.
    ch(&mut rt, " ");
    ch(&mut rt, "d");
    click(&mut rt, "no");
    assert_eq!(rt.state.view, AppView::Normal);
    ch(&mut rt, "d");
    key(&mut rt, Key::Enter);
    assert_eq!(rt.state.drive_eps.len(), 6);
}

#[test]
fn sync_needs_a_selection_on_the_mac_side() {
    let (mut rt, _) = started();
    ch(&mut rt, "s");
    assert_eq!(rt.state.view, AppView::Normal);
    key(&mut rt, Key::Tab);
    ch(&mut rt, "a");
    ch(&mut rt, "s");
    assert_eq!(rt.state.view, AppView::Normal, "s does nothing in the drive pane");
}

fn threaded(step_ms: u64) -> (Runtime<State>, Arc<DemoBackend>) {
    let b = Arc::new(DemoBackend::new(Timing {
        latency: Duration::ZERO,
        sync_step: Duration::from_millis(step_ms),
        sync_steps: 40,
    }));
    let mut rt = runtime(b.clone(), false);
    until(&mut rt, "startup", |s| !s.loading_mac && !s.loading_drive && !s.drive_eps.is_empty());
    (rt, b)
}

#[test]
fn sync_shows_progress_then_copies_and_rescans() {
    let (mut rt, b) = threaded(15);
    // Rows 0 and 2 are not on WALKMAN.
    ch(&mut rt, " ");
    ch(&mut rt, "j");
    ch(&mut rt, "j");
    ch(&mut rt, " ");
    ch(&mut rt, "s");
    assert_eq!(rt.state.view, AppView::Syncing);
    assert!(rt.find("transfer-panel").is_some());
    until(&mut rt, "progress", |s| s.progress.total_files == 2 && s.progress.bytes_transferred > 0);
    assert!(rt.state.progress.total_bytes > 0 && rt.state.progress.speed > 0.0);
    until(&mut rt, "the sync to finish", |s| s.view == AppView::Normal);
    until(&mut rt, "the rescan", |s| s.drive_eps.len() == 11);
    assert_eq!(b.files_on("demo-walkman").len(), 11);
    assert!(rt.state.mac.iter().all(|e| !e.selected), "selection clears after a sync");
    assert!(rt.state.error.is_empty());
}

#[test]
fn escape_cancels_a_sync() {
    let (mut rt, b) = threaded(40);
    ch(&mut rt, "a");
    ch(&mut rt, "s");
    assert_eq!(rt.state.view, AppView::Syncing);
    until(&mut rt, "progress", |s| s.progress.bytes_transferred > 0);
    // Other keys do nothing while syncing.
    ch(&mut rt, "j");
    ch(&mut rt, "f");
    assert_eq!(rt.state.view, AppView::Syncing);
    key(&mut rt, Key::Escape);
    until(&mut rt, "the cancel", |s| s.view == AppView::Normal);
    assert_eq!(b.files_on("demo-walkman").len(), 9, "nothing copied");
    assert!(rt.state.debug_messages.iter().any(|m| m.message == "Sync cancelled by user"));
    assert!(rt.state.error.is_empty());
    assert!(rt.state.mac.iter().all(|e| e.selected), "a cancelled sync keeps the selection");
}

#[test]
fn drives_appearing_and_disappearing_are_noticed() {
    let (mut rt, b) = started();
    assert_eq!(rt.state.current_drive.as_ref().unwrap().id, "demo-walkman");
    b.unplug("demo-walkman");
    for _ in 0..150 {
        rt.frame(DT);
    }
    assert!(rt.state.current_drive.is_none(), "the current drive went away");
    assert!(rt.state.drive_eps.is_empty());
    assert_eq!(rt.state.drives.len(), 2);
    assert!(rt.state.mac.iter().all(|e| !e.on_drive));
    b.plug("demo-walkman");
    for _ in 0..150 {
        rt.frame(DT);
    }
    assert_eq!(rt.state.drives.len(), 3);
    assert_eq!(rt.state.current_drive.as_ref().unwrap().id, "demo-walkman", "a drive appearing is selected when none is");
    assert_eq!(rt.state.drive_eps.len(), 9);
}

#[test]
fn refresh_reloads_and_is_debounced() {
    let (mut rt, _) = started();
    rt.state.mac.clear();
    rt.state.error = "old".into();
    ch(&mut rt, "r");
    assert_eq!(rt.state.mac.len(), 37);
    assert!(rt.state.error.is_empty());
    rt.state.mac.clear();
    ch(&mut rt, "r");
    assert!(rt.state.mac.is_empty(), "a second r within a second is ignored");
    for _ in 0..70 {
        rt.frame(DT);
    }
    ch(&mut rt, "r");
    assert_eq!(rt.state.mac.len(), 37);
}

#[test]
fn q_and_ctrl_c_quit_from_any_view() {
    let (mut rt, _) = started();
    ch(&mut rt, "f");
    ch(&mut rt, "q");
    assert!(rt.state.quit_requested);
    let (mut rt, _) = started();
    ctrl(&mut rt, "c");
    assert!(rt.state.quit_requested);
}

#[test]
fn keys_map_to_action_names() {
    let ev = |key, mods| KeyEvent { key, mods };
    let c = Mods { ctrl: true, ..Mods::default() };
    assert_eq!(keys::key_name(&ev(Key::Text(" ".into()), Mods::default())), Some(("space".into(), false)));
    assert_eq!(keys::key_name(&ev(Key::Text("\u{14}".into()), c)), Some(("t".into(), true)));
    assert_eq!(keys::key_name(&ev(Key::Command('t'), c)), Some(("t".into(), true)));
    assert_eq!(keys::key_name(&ev(Key::Function(1), Mods::default())), Some(("f1".into(), false)));
    assert_eq!(keys::hint_action("ctrl+t"), ("t".into(), true));
    assert_eq!(keys::hint_action("esc"), ("escape".into(), false));
    assert_eq!(keys::hint_action("↓"), ("down".into(), false));
}

#[test]
fn every_scene_builds_and_renders_its_view() {
    for (name, scene) in crate::spec().scenes {
        let mut s = State::new(Arc::new(DemoBackend::instant()), true);
        scene(&mut s);
        let mut rt = Runtime::new(s, view, (1180.0, 760.0), Env::new(true, 1.0));
        rt.poll = Some(logic::poll);
        rt.frame(0.0);
        rt.frame(DT);
        assert!(rt.find("header").is_some(), "{name}");
    }
}
