//! The window: header, error line, the two episode panes, the footer, and the views that open
//! over them (drive selector, transfer progress, delete confirmation, theme picker, debug log).

use castoff_core::format::{format_bytes, format_date, format_duration, format_speed};
use castoff_core::{Drive, Episode};
use rune::tok::{Family, TextStyle};
use rune::{
    Color, Cursor, Cx, El, LoopSpec, ScrollSpec, Spring, Transition, Tx, col, el, icon, row, text,
};

use rune_kit::{ScrollBar, ScrollBarStyle};

use crate::icons;
use crate::keys::{self, act, click_row, click_selector, hint_action, row_key};
use crate::state::{AppView, DebugMessage, Level, Pane, State};
use crate::theme::{Colors, THEMES};

/// Below this width the focused pane takes most of the room.
const NARROW: f32 = 860.0;
/// An episode row's height, for paging.
const ROW_H: f32 = 58.0;

const fn ts(size: f32, weight: f32) -> TextStyle {
    TextStyle::new(size, weight)
}

fn label(s: impl Into<String>, style: TextStyle, color: Color) -> El {
    text(s).font(style).color(color)
}

fn mono(size: f32, weight: f32) -> TextStyle {
    TextStyle::new(size, weight).family(Family::SystemMono)
}

/// A shortcut hint: `key label`, where each `/`-separated part of the key is its own clickable
/// keycap.
#[derive(Clone, Copy)]
pub struct Hint {
    pub key: &'static str,
    pub label: &'static str,
}

const fn hint(key: &'static str, label: &'static str) -> Hint {
    Hint { key, label }
}

/// `getFooterShortcuts`.
fn footer_hints(view: AppView, debug: bool) -> Vec<Vec<Hint>> {
    match view {
        AppView::Normal => {
            let line1 = vec![hint("↑/↓", "navigate"), hint("tab", "switch list"), hint("f", "drives"), hint("r", "refresh")];
            let mut line2 = Vec::new();
            if debug {
                line2.push(hint("x", "debug"));
            }
            line2.push(hint("ctrl+t", "change theme"));
            line2.push(hint("q", "quit"));
            vec![line1, line2]
        }
        _ => vec![vec![hint("q", "quit")]],
    }
}

fn keycap(c: &Colors, key_id: String, part: &'static str) -> El {
    let (name, ctrl) = hint_action(part);
    el().key(key_id)
        .px(6.0)
        .h(20.0)
        .center()
        .radius(5.0)
        .fill(c.element)
        .border(1.0, c.border.alpha(0.6))
        .hover(c.footer_key.alpha(0.18))
        .press(c.footer_key.alpha(0.3))
        .cursor(Cursor::Pointer)
        .role("button")
        .label(part)
        .on_press_tx(move |s: &mut State, tx: &mut Tx| act(s, &name, ctrl, tx))
        .child(label(part, mono(11.5, 600.0), c.footer_key).group_color(c.list_focused))
}

/// One hint: its keycaps then its label. A single-key hint's label presses it too.
fn hint_el(c: &Colors, scope: &str, h: Hint) -> El {
    let parts: Vec<&'static str> = h.key.split('/').collect();
    let mut caps = row().gap(3.0).center_y();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            caps = caps.child(label("/", ts(11.0, 500.0), c.footer_key.alpha(0.7)));
        }
        caps = caps.child(keycap(c, format!("{scope}-key-{}-{part}", h.label.replace(' ', "-")), part));
    }
    let mut r = row().key(format!("{scope}-hint-{}", h.label.replace(' ', "-"))).gap(6.0).center_y().px(4.0).py(2.0).radius(6.0)
        .hover_group()
        .child(caps)
        .child(label(h.label, ts(12.0, 450.0), c.footer_text).group_color(c.list_focused));
    if parts.len() == 1 && !h.key.is_empty() {
        let (name, ctrl) = hint_action(h.key);
        r = r.cursor(Cursor::Pointer).on_press_tx(move |s: &mut State, tx: &mut Tx| act(s, &name, ctrl, tx));
    }
    r
}

fn hints_line(c: &Colors, scope: &str, line: &[Hint]) -> El {
    let mut r = row().gap(8.0).center().flex_wrap();
    for (i, h) in line.iter().enumerate() {
        if i > 0 {
            r = r.child(label("•", ts(11.0, 600.0), c.footer_separator.alpha(0.8)));
        }
        r = r.child(hint_el(c, scope, *h));
    }
    r
}

fn hints_block(c: &Colors, scope: &str, lines: &[Vec<Hint>]) -> El {
    let mut b = col().key(format!("{scope}-hints")).gap(6.0).center();
    for line in lines.iter().filter(|l| !l.is_empty()) {
        b = b.child(hints_line(c, scope, line));
    }
    b
}

/// Three dots breathing in turn.
fn spinner(key: &str, color: Color) -> El {
    let mut r = row().key(key.to_string()).gap(4.0).center_y();
    for i in 0..3 {
        r = r.child(
            el().key(format!("dot{i}"))
                .size(6.0, 6.0)
                .radius(3.0)
                .fill(color)
                .looping(LoopSpec::opacity(0.9, f64::from(i) * 0.15, 0.25, 1.0)),
        );
    }
    r
}

pub fn view(s: &State, cx: &mut Cx<State>) -> El {
    let c = Colors::of(&s.theme);
    let size = cx.layout_size("root").map_or((1100.0, 700.0), |[w, h]| (w, h));
    let inset = cx.titlebar_inset();

    let mut root = col()
        .key("root")
        .w_full()
        .h_full()
        .fill(c.background)
        .child(header(s, &c, size, inset));
    if !s.error.is_empty() {
        root = root.child(error_line(s, &c));
    }
    root = root
        .child(panes(s, &c, size.0))
        .child(col().key("footer").w_full().pt(10.0).pb(14.0).px(16.0).child(hints_block(
            &c,
            "footer",
            &footer_hints(s.view, s.debug_enabled),
        )));
    match s.view {
        AppView::Normal => {}
        AppView::DriveSelection => root = overlay(root, &c, "drives", drive_selector(s, &c, size), true),
        AppView::Syncing => root = overlay(root, &c, "transfer", transfer(s, &c), false),
        AppView::Confirm => root = overlay(root, &c, "confirm", confirm(s, &c), true),
        AppView::ThemeSelection => root = overlay(root, &c, "themes", theme_picker(s, &c, size), true),
        AppView::Debug => root = overlay(root, &c, "debug", debug_log(s, &c, size), true),
    }
    root
}

fn header(s: &State, c: &Colors, size: (f32, f32), inset: f32) -> El {
    let lead = if inset > 0.0 { inset + 18.0 } else { 20.0 };
    let mut left = col().key("header-left").grow(1.0).min_w(0.0).pl(lead).gap(2.0).justify_center();
    if s.debug_enabled {
        left = left
            .cursor(Cursor::Pointer)
            .on_press_tx(|s: &mut State, tx: &mut Tx| act(s, "f1", false, tx))
            .child(label("DEBUG MODE", ts(11.0, 700.0).tracking(0.6), c.status_debug))
            .child(
                label(
                    format!(
                        "Key: {}  ·  Size: {}×{}",
                        s.last_key.as_deref().unwrap_or("—"),
                        size.0.round(),
                        size.1.round()
                    ),
                    mono(11.0, 500.0),
                    c.status_debug.alpha(0.85),
                )
                .truncate(),
            );
    }

    let logo = row()
        .key("logo")
        .gap(8.0)
        .center_y()
        .child(
            el().size(26.0, 26.0)
                .radius(7.0)
                .center()
                .gradient(c.status_app, c.accent)
                .child(icon(&icons::SAILBOAT, 16.0).color(c.background)),
        )
        .child(label("Castoff", ts(20.0, 760.0).tracking(-0.2), c.status_app));

    let drive_text = match &s.current_drive {
        Some(d) => format!("Drive: {} › podcasts", d.name),
        None => "No drives detected".to_string(),
    };
    let chip = row()
        .key("drive-chip")
        .gap(7.0)
        .center_y()
        .px(10.0)
        .h(30.0)
        .radius(8.0)
        .min_w(0.0)
        .border(1.0, c.border.alpha(0.7))
        .hover(c.hover)
        .press(c.press)
        .cursor(Cursor::Pointer)
        .role("button")
        .label("Choose drive")
        .no_window_drag()
        .on_press_tx(|s: &mut State, tx: &mut Tx| act(s, "f", false, tx))
        .child(icon(&icons::DRIVE, 15.0).color(c.status_drive))
        .child(label(drive_text, ts(12.5, 550.0), c.status_drive).truncate());
    // No `min_w(0)`: the chip keeps its width in a narrow window and the title moves left of
    // centre to make room, instead of sliding under it.
    let right = row().key("header-right").grow(1.0).pr(16.0).justify_end().center_y().child(chip);

    row()
        .key("header")
        .w_full()
        .h(56.0)
        .center_y()
        .gap(12.0)
        .window_drag()
        .child(left)
        .child(logo)
        .child(right)
}

fn error_line(s: &State, c: &Colors) -> El {
    row()
        .key("error")
        .mx(16.0)
        .mb(8.0)
        .px(12.0)
        .py(8.0)
        .gap(8.0)
        .center_y()
        .radius(8.0)
        .fill(c.error.alpha(0.12))
        .border(1.0, c.error.alpha(0.35))
        .transition(Transition::opacity())
        .child(icon(&icons::WARNING, 15.0).color(c.error))
        .child(label(s.error.clone(), ts(12.5, 500.0), c.error).grow(1.0).min_w(0.0).truncate())
}

fn panes(s: &State, c: &Colors, width: f32) -> El {
    let (mac_grow, drive_grow) = if width <= NARROW {
        if s.focused == Pane::Mac { (7.0, 3.0) } else { (3.0, 7.0) }
    } else {
        (1.0, 1.0)
    };
    row()
        .key("panes")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .px(16.0)
        .gap(12.0)
        .child(pane(s, c, Pane::Mac).grow(mac_grow))
        .child(pane(s, c, Pane::Drive).grow(drive_grow))
}

fn pane(s: &State, c: &Colors, p: Pane) -> El {
    let focused = s.focused == p;
    let (key, title, glyph) = match p {
        Pane::Mac => ("mac", "Apple Podcasts", &icons::LAPTOP),
        Pane::Drive => ("drive", "Drive Podcasts", &icons::DRIVE),
    };
    let list = s.list(p);
    let loading = match p {
        Pane::Mac => s.loading_mac,
        Pane::Drive => s.loading_drive,
    };
    let selected = list.iter().filter(|e| e.selected).count();
    let mut counts = if loading { String::new() } else { format!("{} episodes", list.len()) };
    if selected > 0 {
        counts = format!("{counts} · {selected} selected");
    }

    let head = row()
        .key("head")
        .w_full()
        .h(46.0)
        .px(14.0)
        .gap(8.0)
        .center_y()
        .child(
            row()
                .key("title")
                .gap(6.0)
                .center_y()
                .px(9.0)
                .h(24.0)
                .min_w(0.0)
                .radius(6.0)
                .fill(c.header_bg)
                .child(icon(glyph, 14.0).color(c.header_text))
                .child(label(title, ts(12.5, 650.0), c.header_text).min_w(0.0).truncate()),
        )
        .child(
            label(counts, ts(12.0, 500.0).features("'tnum' 1"), if selected > 0 { c.marker_selected } else { c.text_dim })
                .key("counts")
                .grow(1.0)
                .min_w(0.0)
                .align(rune::Align::End)
                .truncate(),
        );

    let body = if loading {
        centered(
            &format!("{key}-loading"),
            col().gap(12.0).center().child(spinner("spinner", c.accent)).child(label("Loading...", ts(13.0, 500.0), c.text)),
        )
    } else if list.is_empty() {
        let hint = match p {
            Pane::Mac => "Download episodes in Apple Podcasts, then press r to refresh.",
            Pane::Drive if s.current_drive.is_none() => "No drive selected. Press f to choose one.",
            Pane::Drive => "Select episodes on the left and press s to sync them here.",
        };
        centered(
            &format!("{key}-empty"),
            // Full width, so the lines wrap inside a narrow pane instead of running past it.
            col()
                .w_full()
                .gap(6.0)
                .center()
                .child(icon(glyph, 26.0).color(c.text_dim))
                .child(label("No podcasts found", ts(13.5, 600.0), c.text).w_full().wrap().align(rune::Align::Center))
                .child(label(hint, ts(12.0, 450.0), c.text_secondary).w_full().wrap().align(rune::Align::Center)),
        )
    } else {
        // Rows lay out at once: text re-truncates for the final width immediately, so a row that
        // glided there on the pane's spring would run its title over the size column on the way.
        let mut rows = col().key(format!("{key}-rows")).w_full().px(8.0).py(6.0).motion(Spring::NONE);
        let cursor = s.index(p);
        for (i, ep) in list.iter().enumerate() {
            rows = rows.child(episode_row(c, p, i, ep, focused && i == cursor, focused));
        }
        col()
            .key(format!("{key}-scroll"))
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .scroll_edge_fade(14.0)
            .scroll_margin(8.0)
            .on_layout(move |s: &mut State, r: [f32; 4]| {
                if s.focused == p {
                    s.page_rows = ((r[3] / ROW_H).floor() as usize).max(1);
                }
            })
            .child(rows)
            .scroll_bar_styled(scroll_bar_style(c))
    };

    let hints = match p {
        Pane::Mac => [hint("space", "select"), hint("a", "all"), hint("esc", "deselect"), hint("s", "sync")],
        Pane::Drive => [hint("space", "select"), hint("a", "all"), hint("esc", "deselect"), hint("d", "delete")],
    };
    let mut foot = col()
        .key("pane-foot")
        .w_full()
        .h(40.0)
        .center()
        .motion(Spring::NONE)
        .opacity(if focused { 1.0 } else { 0.0 })
        .child(hints_line(c, key, &hints));
    if !focused {
        foot = foot.inert();
    }

    col()
        .key(format!("pane-{key}"))
        .role("region")
        .label(title)
        .min_w(0.0)
        .min_h(0.0)
        .radius(12.0)
        .fill(if focused { c.surface } else { c.background })
        .border(1.0, if focused { c.border_focused } else { c.border })
        .shadow(if focused { 6.0 } else { 0.0 }, if focused { 22.0 } else { 0.0 }, c.shadow.alpha(if focused { 0.28 } else { 0.0 }))
        .motion(Spring::new(0.32, 0.0))
        .on_press(move |s: &mut State| {
            if s.view == AppView::Normal {
                s.focused = p;
            }
        })
        .child(head)
        .child(el().key("rule").w_full().h(1.0).fill(c.border.alpha(0.6)))
        .child(body)
        .child(foot)
}

fn centered(key: &str, content: El) -> El {
    col().key(key.to_string()).w_full().grow(1.0).min_h(0.0).center().padding(16.0).child(content)
}

/// An episode's second line: "show • date • duration".
pub fn describe(ep: &Episode) -> String {
    let mut parts = vec![ep.show_name.clone()];
    if let Some(p) = ep.published.filter(|p| *p != 0) {
        parts.push(format_date(p));
    }
    if ep.duration_secs > 0.0 {
        parts.push(format_duration(ep.duration_secs));
    }
    parts.join(" • ")
}

fn episode_row(c: &Colors, p: Pane, i: usize, ep: &Episode, cursor: bool, focused: bool) -> El {
    let sel = ep.selected;
    // Title and description colours by selection and cursor.
    let title_color = match (sel, cursor) {
        (true, true) => c.list_focused_selected,
        (true, false) => c.list_selected,
        (false, true) => c.list_focused,
        _ => c.text,
    };
    let desc_color = match (sel, cursor) {
        (true, true) => c.list_focused_selected,
        (true, false) => c.list_selected,
        (false, true) => c.list_focused,
        _ => c.text_secondary,
    };
    let marker = if sel {
        c.marker_selected
    } else if cursor {
        c.marker_cursor
    } else {
        Color::TRANSPARENT
    };

    let mut title = row().gap(6.0).center_y().min_w(0.0);
    if ep.on_drive {
        title = title.child(icon(&icons::CHECK, 14.0).color(c.success).key("on-drive"));
    }
    title = title.child(label(ep.title.clone(), ts(13.5, 600.0), title_color).min_w(0.0).truncate());

    let check = el()
        .key("check")
        .size(18.0, 18.0)
        .radius(9.0)
        .center()
        .fill(if sel { c.marker_selected } else { Color::TRANSPARENT })
        .border(1.5, if sel { c.marker_selected } else { c.text_dim.alpha(0.55) })
        .hover(c.marker_selected.alpha(0.25))
        .cursor(Cursor::Pointer)
        .role("checkbox")
        .checked(sel)
        .label("Select")
        .on_press(move |s: &mut State| {
            if s.view == AppView::Normal {
                s.focused = p;
                s.set_index(p, i);
                keys::toggle_selection(s, p, i);
            }
        });
    let check = if sel { check.child(icon(&icons::CHECK, 13.0).color(c.background)) } else { check };

    let bg = if cursor {
        c.list_focused.alpha(0.13)
    } else if sel {
        c.marker_selected.alpha(0.07)
    } else {
        Color::TRANSPARENT
    };
    let _ = focused;
    row()
        .key(row_key(p, i))
        .w_full()
        .h(ROW_H)
        .pl(6.0)
        .pr(10.0)
        .gap(10.0)
        .center_y()
        .radius(9.0)
        .fill(bg)
        .hover(c.hover)
        .press(c.press)
        .cursor(Cursor::Pointer)
        .role("row")
        .label(ep.title.clone())
        .on_press(move |s: &mut State| click_row(s, p, i))
        .child(el().key("marker").w(if sel { 4.0 } else { 3.0 }).h(34.0).radius(2.0).fill(marker))
        .child(
            col()
                .key("text")
                .grow(1.0)
                .min_w(0.0)
                .gap(3.0)
                .child(title)
                .child(label(describe(ep), ts(12.0, 450.0), desc_color).min_w(0.0).truncate()),
        )
        .child(label(format_bytes(ep.file_size), ts(11.5, 500.0).features("'tnum' 1"), c.text_dim).key("size"))
        .child(check)
}

/// A modal over the window: a scrim and a centred panel. `outside` lets a click on the scrim
/// close it.
fn overlay(root: El, c: &Colors, key: &str, panel: El, outside: bool) -> El {
    let panel = panel.modal_tx(outside, |s: &mut State, tx: &mut Tx| {
        // Escape is handled by `on_key`; this is a click outside.
        let k = if s.view == AppView::Confirm { "n" } else { "escape" };
        act(s, k, false, tx);
    });
    root.child(
        el().key(format!("{key}-scrim"))
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .fill(c.scrim)
            .transition(Transition::opacity())
            .a11y_hidden(),
    )
    .child(
        el().key(format!("{key}-layer"))
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .padding(24.0)
            .center()
            .child(panel),
    )
}

fn panel(c: &Colors, key: &str, title: &str, glyph: &'static rune::Icon, width: f32) -> El {
    col()
        .key(format!("{key}-panel"))
        .role("dialog")
        .label(title.to_string())
        .w_full()
        .max_w(width)
        .radius(14.0)
        .fill(c.popup_bg)
        .border(1.0, c.popup_border.alpha(0.85))
        .shadow(18.0, 48.0, c.shadow)
        .shadow(2.0, 6.0, c.shadow.alpha(0.3))
        .transition(Transition::scale(0.96).combined(Transition::opacity()))
        .child(
            row()
                .key("title-row")
                .w_full()
                .pt(16.0)
                .pb(4.0)
                .center()
                .child(
                    row()
                        .key("title")
                        .gap(7.0)
                        .center_y()
                        .px(10.0)
                        .h(26.0)
                        .radius(7.0)
                        .fill(c.header_bg)
                        .child(icon(glyph, 14.0).color(c.header_text))
                        .child(label(title.to_string(), ts(13.0, 650.0), c.header_text)),
                ),
        )
}

struct Item {
    name: String,
    description: Option<String>,
    color: Option<Color>,
    badge: Option<El>,
}

/// A list where the current row has a heavy marker, hover tints, a click
/// on another row moves there and a click on the current row chooses it.
fn selector(c: &Colors, key: &str, items: Vec<Item>, current: usize, max_h: f32, mono_rows: bool) -> El {
    let mut rows = col().key(format!("{key}-items")).w_full().gap(2.0).py(4.0);
    for (i, item) in items.into_iter().enumerate() {
        let on = i == current;
        let name_color = if on { c.list_focused } else { item.color.unwrap_or(c.text) };
        let style = if mono_rows { mono(12.0, 500.0) } else { ts(13.5, if on { 650.0 } else { 550.0 }) };
        let mut text_col = col()
            .grow(1.0)
            .min_w(0.0)
            .gap(2.0)
            .child(label(item.name, style, name_color).group_color(c.list_focused).min_w(0.0).truncate());
        if let Some(d) = item.description {
            text_col = text_col.child(
                label(d, ts(11.5, 450.0), if on { c.list_focused.alpha(0.85) } else { c.text_secondary })
                    .min_w(0.0)
                    .truncate(),
            );
        }
        let mut r = row()
            .key(format!("{key}-item-{i}"))
            .w_full()
            .min_h(if mono_rows { 30.0 } else { 40.0 })
            .py(5.0)
            .pl(6.0)
            .pr(10.0)
            .gap(10.0)
            .center_y()
            .radius(8.0)
            .fill(if on { c.list_focused.alpha(0.12) } else { Color::TRANSPARENT })
            .hover(c.hover)
            .press(c.press)
            .hover_group()
            .cursor(Cursor::Pointer)
            .role("option")
            .checked(on)
            .on_press_tx(move |s: &mut State, tx: &mut Tx| click_selector(s, i, tx))
            .child(
                el().key("marker")
                    .w(if on { 4.0 } else { 2.0 })
                    .h(if mono_rows { 18.0 } else { 26.0 })
                    .radius(2.0)
                    .fill(if on { c.list_focused } else { c.border })
                    .group_color(c.list_focused),
            )
            .child(text_col);
        if let Some(b) = item.badge {
            r = r.child(b);
        }
        rows = rows.child(r);
    }
    col()
        .key(format!("{key}-scroll"))
        .w_full()
        .max_h(max_h)
        .min_h(0.0)
        .px(10.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .scroll_edge_fade(12.0)
        .scroll_margin(6.0)
        .child(rows)
        .scroll_bar_styled(scroll_bar_style(c))
}

/// The thumb on a list that overflows. It follows the system "Show scroll bars" setting: always
/// shown with a mouse, and shown while scrolling or under the pointer with a trackpad.
fn scroll_bar_style(c: &Colors) -> ScrollBarStyle {
    ScrollBarStyle::neutral(true).thumb(c.text_dim).thumb_hover(c.text_secondary)
}

fn selector_footer(c: &Colors, key: &str, enter: &'static str, extra: Option<Hint>) -> El {
    let mut line = vec![hint("↑/↓", "navigate"), hint("enter", enter), hint("esc", "close")];
    if let Some(e) = extra {
        line.push(e);
    }
    col().key(format!("{key}-foot")).w_full().pt(10.0).pb(14.0).px(12.0).center().child(hints_line(c, key, &line))
}

/// `formatDriveInfo`: "WALKMAN (21.4 GB free)".
pub fn drive_info(d: &Drive) -> String {
    match d.free_space {
        Some(free) => format!("{} ({} free)", d.name, format_bytes(free)),
        None => format!("{} (unknown free space)", d.name),
    }
}

fn drive_selector(s: &State, c: &Colors, size: (f32, f32)) -> El {
    let p = panel(c, "drives", "Select Drive", &icons::DRIVE, 560.0);
    if s.scanning {
        return p.child(
            row().key("scanning").w_full().py(28.0).gap(12.0).center().child(spinner("spinner", c.accent)).child(label(
                "Scanning for drives...",
                ts(13.0, 500.0),
                c.text,
            )),
        );
    }
    if s.drives.is_empty() {
        return p
            .child(col().key("empty").w_full().py(24.0).center().child(label("No external drives found", ts(13.0, 500.0), c.text)))
            .child(selector_footer(c, "drives", "select", Some(hint("ctrl+f", "favorite"))));
    }
    let items = s
        .drives
        .iter()
        .map(|d| {
            let used = d.free_space.map(|f| 1.0 - f as f32 / d.total_space.max(1) as f32);
            let description = format!(
                "{} · {}{}",
                d.mount_point.display(),
                format_bytes(d.total_space),
                if s.current_drive.as_ref().is_some_and(|x| x.id == d.id) { " · current" } else { "" }
            );
            let mut badge = row().gap(8.0).center_y();
            if let Some(u) = used {
                badge = badge.child(
                    el().key("usage").w(54.0).h(5.0).radius(2.5).fill(c.progress_empty).child(
                        el().w_frac(u.clamp(0.02, 1.0)).h(5.0).radius(2.5).fill(if u > 0.9 { c.warning } else { c.progress }),
                    ),
                );
            }
            if s.is_favorite(&d.id) {
                badge = badge.child(
                    row()
                        .key("fav")
                        .gap(4.0)
                        .px(7.0)
                        .h(20.0)
                        .center_y()
                        .radius(10.0)
                        .fill(c.footer_key.alpha(0.16))
                        .child(icon(&icons::STAR, 11.0).color(c.footer_key))
                        .child(label("Favourite", ts(11.0, 600.0), c.footer_key)),
                );
            }
            Item { name: drive_info(d), description: Some(description), color: None, badge: Some(badge) }
        })
        .collect();
    p.child(selector(c, "drive", items, s.drive_menu_index, (size.1 * 0.6).max(160.0), false))
        .child(selector_footer(c, "drives", "select", Some(hint("ctrl+f", "favorite"))))
}

fn theme_picker(s: &State, c: &Colors, size: (f32, f32)) -> El {
    let items = THEMES
        .iter()
        .map(|(name, pal)| {
            let k = Colors::from_palette(pal);
            let mut sw = row().key("swatches").gap(3.0).center_y();
            for col_ in [k.background, k.accent, k.marker_selected, k.footer_key, k.success] {
                sw = sw.child(el().size(12.0, 12.0).radius(6.0).fill(col_).border(1.0, c.border.alpha(0.6)));
            }
            let saved = *name == s.saved_theme;
            let badge = if saved {
                row().gap(8.0).center_y().child(label("current", ts(11.0, 600.0), c.text_dim)).child(sw)
            } else {
                sw
            };
            Item { name: name.to_string(), description: None, color: None, badge: Some(badge) }
        })
        .collect();
    panel(c, "themes", "Select Theme", &icons::PALETTE, 440.0)
        .child(selector(c, "theme", items, s.theme_menu_index, (size.1 * 0.62).max(160.0), false))
        .child(selector_footer(c, "themes", "select", None))
}

fn level_color(c: &Colors, level: Level) -> Color {
    match level {
        Level::Error => c.error,
        Level::Warn => c.warning,
        Level::Info => c.text_secondary,
    }
}

pub fn format_timestamp(ms: i64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_millis_opt(ms) {
        chrono::LocalResult::Single(t) => t.format("%H:%M:%S").to_string(),
        _ => "--:--:--".into(),
    }
}

fn debug_log(s: &State, c: &Colors, size: (f32, f32)) -> El {
    let w = (size.0 * 0.85).max(320.0);
    let h = (size.1 * 0.85).max(240.0);
    let mut p = panel(c, "debug", "Debug Log", &icons::LOG, w).h(h);
    if s.debug_messages.is_empty() {
        p = p.child(
            col().key("empty").w_full().grow(1.0).center().child(label("No debug messages", ts(13.0, 500.0), c.text_secondary)),
        );
    } else {
        let items = s
            .debug_messages
            .iter()
            .map(|m: &DebugMessage| Item {
                name: format!("[{}] {}", format_timestamp(m.timestamp), m.message),
                description: None,
                color: Some(level_color(c, m.level)),
                badge: None,
            })
            .collect();
        p = p.child(selector(c, "debug", items, s.debug_menu_index, h, true).grow(1.0));
    }
    p.child(selector_footer(c, "debug", "copy", None).key("debug-foot"))
}

/// `calculateProgress`: bytes when known, else files.
pub fn progress_fraction(s: &State) -> f32 {
    let p = &s.progress;
    if p.total_bytes > 0 {
        (p.bytes_transferred as f64 / p.total_bytes as f64).clamp(0.0, 1.0) as f32
    } else if p.total_files > 0 {
        (p.files_done as f64 / p.total_files as f64).clamp(0.0, 1.0) as f32
    } else {
        0.0
    }
}

fn transfer(s: &State, c: &Colors) -> El {
    let p = &s.progress;
    let frac = progress_fraction(s);
    let pct = (frac * 100.0).round() as u32;
    let mut bar_row = row()
        .key("bar-row")
        .w_full()
        .gap(12.0)
        .center_y()
        .child(
            el().key("track").grow(1.0).h(10.0).radius(5.0).fill(c.progress_empty).child(
                el().key("fill")
                    .w_frac(frac.max(0.0))
                    .h(10.0)
                    .radius(5.0)
                    .gradient(c.progress, c.progress.mix(c.accent, 0.35))
                    .motion(Spring::new(0.35, 0.0)),
            ),
        )
        .child(label(format!("{pct}%"), ts(13.0, 650.0).features("'tnum' 1"), c.accent).key("pct").w(40.0));
    if frac < 1.0 {
        bar_row = bar_row.child(spinner("spinner", c.accent));
    }
    let sep = || label("•", ts(11.0, 600.0), c.footer_separator);
    let stats = row()
        .key("stats")
        .gap(8.0)
        .center()
        .flex_wrap()
        .child(label(format!("{}/{} files", p.files_done, p.total_files), ts(12.5, 500.0).features("'tnum' 1"), c.text_secondary))
        .child(sep())
        .child(label(format_speed(p.speed), ts(12.5, 500.0).features("'tnum' 1"), c.text_secondary))
        .child(sep())
        .child(label(
            format!("{} / {}", format_bytes(p.bytes_transferred), format_bytes(p.total_bytes)),
            ts(12.5, 500.0).features("'tnum' 1"),
            c.text_secondary,
        ));
    let file = if p.current_file.is_empty() { "Preparing..." } else { p.current_file.as_str() };
    panel(c, "transfer", "Syncing", &icons::SYNC, 520.0)
        .child(
            col()
                .key("body")
                .w_full()
                .px(24.0)
                .pt(14.0)
                .gap(14.0)
                .child(
                    row()
                        .key("file")
                        .w_full()
                        .gap(6.0)
                        .center()
                        .child(label("Transferring:", ts(13.0, 500.0), c.text))
                        .child(label(file.to_string(), ts(13.0, 600.0), c.accent).min_w(0.0).truncate_middle()),
                )
                .child(bar_row)
                .child(stats),
        )
        .child(col().key("transfer-foot").w_full().pt(12.0).pb(16.0).center().child(hints_line(c, "transfer", &[hint("esc", "cancel")])))
}

fn confirm(s: &State, c: &Colors) -> El {
    let n = s.drive_eps.iter().filter(|e| e.selected).count();
    let button = |key: &'static str, text_: &'static str, caps: &'static str, fill: Color, ink: Color, act_key: &'static str| {
        row()
            .key(key)
            .gap(10.0)
            .center()
            .px(14.0)
            .h(34.0)
            .radius(8.0)
            .fill(fill)
            .hover(Color::WHITE.alpha(0.06))
            .press(Color::BLACK.alpha(0.12))
            .cursor(Cursor::Pointer)
            .role("button")
            .label(text_)
            .on_press_tx(move |s: &mut State, tx: &mut Tx| act(s, act_key, false, tx))
            .child(label(text_, ts(13.0, 650.0), ink))
            .child(label(caps, mono(11.0, 600.0), ink.alpha(0.7)))
    };
    panel(c, "confirm", "Confirm Delete", &icons::TRASH, 440.0)
        .child(
            col()
                .key("body")
                .w_full()
                .px(24.0)
                .pt(12.0)
                .pb(18.0)
                .gap(18.0)
                .center()
                .child(label(format!("Are you sure you want to delete {n} file(s)?"), ts(14.0, 550.0), c.text))
                .child(
                    row()
                        .key("buttons")
                        .gap(10.0)
                        .center()
                        .child(button("no", "Cancel", "n/esc", c.element, c.text, "n"))
                        .child(button("yes", "Delete", "y/enter", c.error, c.background, "y")),
                ),
        )
}
