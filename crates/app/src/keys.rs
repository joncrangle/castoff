//! The keyboard model: keys become names ("j", "space",
//! "escape", "pageup", ...) and `act` handles a name for the current view. Clicks on footer
//! hints and rows go through `act` too.

use rune::{Key, KeyEvent, Tx};

use crate::logic;
use crate::state::{AppView, Level, Pane, State};
use crate::theme::THEMES;

/// Seconds between two refreshes: `r` is debounced.
const REFRESH_DEBOUNCE: f64 = 1.0;

/// A key event's name and whether Control was held, or None for keys the app ignores.
pub fn key_name(ev: &KeyEvent) -> Option<(String, bool)> {
    let ctrl = ev.mods.ctrl;
    let name = match &ev.key {
        Key::Up => "up".into(),
        Key::Down => "down".into(),
        Key::Left => "left".into(),
        Key::Right => "right".into(),
        Key::Tab => "tab".into(),
        Key::Escape => "escape".into(),
        Key::Enter => "return".into(),
        Key::PageUp => "pageup".into(),
        Key::PageDown => "pagedown".into(),
        Key::Home => "home".into(),
        Key::End => "end".into(),
        Key::Function(n) => format!("f{n}"),
        Key::Text(t) if t == " " => "space".into(),
        Key::Text(t) => {
            let mut chars = t.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            // A Control chord can arrive as its control character (Ctrl+T is U+0014).
            if (c as u32) >= 1 && (c as u32) <= 26 {
                return Some((((b'a' + c as u8 - 1) as char).to_string(), true));
            }
            c.to_lowercase().to_string()
        }
        // Off macOS a Control chord arrives as a command key; on macOS that is ⌘.
        Key::Command(c) => {
            if ctrl {
                c.to_string()
            } else if *c == 'q' {
                "q".into()
            } else {
                return None;
            }
        }
        _ => return None,
    };
    Some((name, ctrl))
}

/// The label the debug header shows for the last key ("Ctrl+t", "Shift+a").
fn key_label(name: &str, ev: &KeyEvent) -> String {
    let mut label = name.to_string();
    if ev.mods.cmd && !ev.mods.ctrl {
        label = format!("Cmd+{label}");
    }
    if ev.mods.ctrl {
        label = format!("Ctrl+{label}");
    }
    if ev.mods.shift {
        label = format!("Shift+{label}");
    }
    label
}

/// `AppSpec::on_key`.
pub fn on_key(s: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some((mut name, mut ctrl)) = key_name(ev) else { return false };
    // Ctrl+U and Ctrl+D page.
    if ctrl && name == "u" {
        name = "pageup".into();
        ctrl = false;
    } else if ctrl && name == "d" {
        name = "pagedown".into();
        ctrl = false;
    }
    s.last_key = Some(key_label(&name, ev));
    act(s, &name, ctrl, tx);
    true
}

/// A footer hint's key ("esc", "enter", "ctrl+t", "↑") as `act` names it.
pub fn hint_action(key: &str) -> (String, bool) {
    let mut k = key.to_lowercase();
    let ctrl = k.starts_with("ctrl+");
    if ctrl {
        k = k["ctrl+".len()..].to_string();
    }
    let k = match k.as_str() {
        "esc" => "escape".to_string(),
        "enter" => "return".to_string(),
        "↑" => "up".to_string(),
        "↓" => "down".to_string(),
        _ => k,
    };
    (k, ctrl)
}

pub fn row_key(pane: Pane, i: usize) -> String {
    match pane {
        Pane::Mac => format!("mac-row-{i}"),
        Pane::Drive => format!("drive-row-{i}"),
    }
}

pub fn reveal(s: &State, tx: &mut Tx) {
    match s.view {
        AppView::Normal => tx.scroll_into_view(row_key(s.focused, s.index(s.focused))),
        AppView::DriveSelection => tx.scroll_into_view(format!("drive-item-{}", s.drive_menu_index)),
        AppView::ThemeSelection => tx.scroll_into_view(format!("theme-item-{}", s.theme_menu_index)),
        AppView::Debug => tx.scroll_into_view(format!("debug-item-{}", s.debug_menu_index)),
        _ => {}
    }
}

/// `handleAction`.
pub fn act(s: &mut State, key: &str, ctrl: bool, tx: &mut Tx) {
    if key == "q" || (ctrl && key == "c") {
        s.quit_requested = true;
        if let Some(cancel) = &s.sync_cancel {
            cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        tx.quit();
        return;
    }
    match s.view {
        AppView::ThemeSelection => theme_selection(s, key, tx),
        AppView::Syncing => {
            if key == "escape" {
                logic::cancel_sync(s);
            }
        }
        AppView::Normal => normal(s, key, ctrl, tx),
        AppView::DriveSelection => drive_selection(s, key, ctrl, tx),
        AppView::Confirm => match key {
            "return" | "enter" | "y" => logic::delete_selected(s),
            "escape" | "n" => s.view = AppView::Normal,
            _ => {}
        },
        AppView::Debug => debug(s, key, tx),
    }
    reveal(s, tx);
}

fn move_cursor(s: &mut State, delta: isize) {
    let pane = s.focused;
    let len = s.list(pane).len();
    let i = if len == 0 { 0 } else { (s.index(pane) as isize + delta).clamp(0, len as isize - 1) as usize };
    s.set_index(pane, i);
}

fn normal(s: &mut State, key: &str, ctrl: bool, tx: &mut Tx) {
    if ctrl && key == "t" {
        s.theme_menu_index = THEMES.iter().position(|(n, _)| *n == s.saved_theme).unwrap_or(0);
        s.view = AppView::ThemeSelection;
        return;
    }
    let pane = s.focused;
    match key {
        "f1" => open_debug(s),
        "x" if s.debug_enabled => open_debug(s),
        "up" | "k" => move_cursor(s, -1),
        "down" | "j" => move_cursor(s, 1),
        "pageup" => move_cursor(s, -(s.page_rows.max(1) as isize)),
        "pagedown" => move_cursor(s, s.page_rows.max(1) as isize),
        "home" => move_cursor(s, isize::MIN / 2),
        "end" => move_cursor(s, isize::MAX / 2),
        "left" | "h" => s.focused = Pane::Mac,
        "right" | "l" => s.focused = Pane::Drive,
        "tab" => s.focused = if pane == Pane::Mac { Pane::Drive } else { Pane::Mac },
        "f" if !ctrl => {
            s.drive_menu_index = 0;
            s.view = AppView::DriveSelection;
            logic::scan_for_drives(s);
        }
        "r" if !ctrl => {
            let now = tx.clock();
            if now - s.last_refresh >= REFRESH_DEBOUNCE {
                s.last_refresh = now;
                logic::refresh(s);
            }
        }
        "s" if !ctrl => {
            if pane == Pane::Mac {
                let selected = s.selected(Pane::Mac);
                if !selected.is_empty() {
                    logic::start_sync(s, selected);
                }
            }
        }
        "d" if !ctrl => {
            if pane == Pane::Drive && s.drive_eps.iter().any(|e| e.selected) {
                s.view = AppView::Confirm;
            }
        }
        "space" => toggle_selection(s, pane, s.index(pane)),
        "a" if !ctrl => toggle_all(s, pane),
        "escape" => clear_selection(s, pane),
        _ => {}
    }
}

fn open_debug(s: &mut State) {
    s.debug_menu_index = s.debug_messages.len().saturating_sub(1);
    s.view = AppView::Debug;
}

pub fn toggle_selection(s: &mut State, pane: Pane, i: usize) {
    if let Some(e) = s.list_mut(pane).get_mut(i) {
        e.selected = !e.selected;
    }
}

pub fn toggle_all(s: &mut State, pane: Pane) {
    let list = s.list_mut(pane);
    let all = list.iter().all(|e| e.selected);
    for e in list.iter_mut() {
        e.selected = !all;
    }
}

pub fn clear_selection(s: &mut State, pane: Pane) {
    for e in s.list_mut(pane).iter_mut() {
        e.selected = false;
    }
}

/// A click on an episode row (`onItemClick`): the cursor row of the focused pane toggles its
/// selection; any other row takes focus and the cursor.
pub fn click_row(s: &mut State, pane: Pane, i: usize) {
    if s.view != AppView::Normal {
        return;
    }
    if s.focused == pane && s.index(pane) == i {
        toggle_selection(s, pane, i);
    } else {
        s.focused = pane;
        s.set_index(pane, i);
    }
}

fn theme_selection(s: &mut State, key: &str, _tx: &mut Tx) {
    match key {
        "escape" => {
            s.theme = s.saved_theme.clone();
            s.view = AppView::Normal;
        }
        "up" | "k" => s.theme_menu_index = s.theme_menu_index.saturating_sub(1),
        "down" | "j" => s.theme_menu_index = (s.theme_menu_index + 1).min(THEMES.len() - 1),
        "return" | "enter" => {
            let name = THEMES[s.theme_menu_index].0;
            logic::save_theme(s, name);
            s.view = AppView::Normal;
            return;
        }
        _ => {}
    }
    if s.view == AppView::ThemeSelection {
        s.theme = THEMES[s.theme_menu_index].0.to_string();
    }
}

fn drive_selection(s: &mut State, key: &str, ctrl: bool, _tx: &mut Tx) {
    match key {
        "escape" => s.view = AppView::Normal,
        "up" | "k" => s.drive_menu_index = s.drive_menu_index.saturating_sub(1),
        "down" | "j" => s.drive_menu_index = (s.drive_menu_index + 1).min(s.drives.len().saturating_sub(1)),
        "return" | "enter" => {
            if let Some(drive) = s.drives.get(s.drive_menu_index).cloned() {
                s.view = AppView::Normal;
                logic::select_drive(s, drive);
            }
        }
        "f" if ctrl => {
            if let Some(drive) = s.drives.get(s.drive_menu_index) {
                let id = drive.id.clone();
                logic::toggle_favorite(s, &id);
            }
        }
        _ => {}
    }
}

fn debug(s: &mut State, key: &str, tx: &mut Tx) {
    match key {
        "escape" => s.view = AppView::Normal,
        "up" | "k" => s.debug_menu_index = s.debug_menu_index.saturating_sub(1),
        "down" | "j" => s.debug_menu_index = (s.debug_menu_index + 1).min(s.debug_messages.len().saturating_sub(1)),
        "return" | "enter" => {
            if let Some(m) = s.debug_messages.get(s.debug_menu_index) {
                tx.copy(m.message.clone());
                s.log("Copied to clipboard", Level::Info);
            }
        }
        _ => {}
    }
}

/// A click on a selector row (`Selector.handleInteraction`): the current row chooses, another
/// row moves there first.
pub fn click_selector(s: &mut State, i: usize, tx: &mut Tx) {
    let current = match s.view {
        AppView::DriveSelection => s.drive_menu_index,
        AppView::ThemeSelection => s.theme_menu_index,
        AppView::Debug => s.debug_menu_index,
        _ => return,
    };
    if current == i {
        act(s, "return", false, tx);
        return;
    }
    match s.view {
        AppView::DriveSelection => s.drive_menu_index = i,
        AppView::ThemeSelection => {
            s.theme_menu_index = i;
            s.theme = THEMES[i].0.to_string();
        }
        AppView::Debug => s.debug_menu_index = i,
        _ => {}
    }
}
