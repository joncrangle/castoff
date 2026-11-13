//! The few icons the window draws, as SVG bodies on a 16-point grid (tinted with `.color`).

use rune::Icon;

pub static CHECK: Icon = Icon {
    name: "pa-check",
    view: 16.0,
    body: r#"<path d="M3.5 8.4L6.6 11.4L12.5 4.8" stroke="currentColor" stroke-width="1.9" fill="none" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

pub static STAR: Icon = Icon {
    name: "pa-star",
    view: 16.0,
    body: r#"<path d="M8 1.6L9.86 5.9L14.5 6.3L10.98 9.36L12.04 13.9L8 11.5L3.96 13.9L5.02 9.36L1.5 6.3L6.14 5.9Z" fill="currentColor"/>"#,
};

pub static DRIVE: Icon = Icon {
    name: "pa-drive",
    view: 16.0,
    body: r#"<rect x="1.75" y="4.25" width="12.5" height="7.5" rx="2" stroke="currentColor" stroke-width="1.3" fill="none"/><circle cx="11.2" cy="8" r="0.95" fill="currentColor"/><path d="M4.2 8H8" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>"#,
};

pub static LAPTOP: Icon = Icon {
    name: "pa-laptop",
    view: 16.0,
    body: r#"<rect x="3" y="3.5" width="10" height="7" rx="1.2" stroke="currentColor" stroke-width="1.3" fill="none"/><path d="M1.5 12.5H14.5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>"#,
};

pub static SAILBOAT: Icon = Icon {
    name: "pa-sailboat",
    view: 16.0,
    body: r#"<g fill="currentColor" stroke="currentColor" stroke-width="0.7" stroke-linejoin="round" stroke-linecap="round"><path d="M8.7 2.2V10.3H13.3Z"/><path d="M7.3 4.6V10.3H3.5Z"/><path d="M2.3 12H13.7L12.3 14H3.7Z"/></g>"#,
};

pub static WARNING: Icon = Icon {
    name: "pa-warning",
    view: 16.0,
    body: r#"<path d="M8 2.2L14.4 13.4H1.6Z" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" fill="none"/><path d="M8 6.4V9.4" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/><circle cx="8" cy="11.4" r="0.8" fill="currentColor"/>"#,
};

pub static SYNC: Icon = Icon {
    name: "pa-sync",
    view: 16.0,
    body: r#"<path d="M13 6.5A5.2 5.2 0 0 0 3.6 5" stroke="currentColor" stroke-width="1.4" fill="none" stroke-linecap="round"/><path d="M3 9.5A5.2 5.2 0 0 0 12.4 11" stroke="currentColor" stroke-width="1.4" fill="none" stroke-linecap="round"/><path d="M3.2 2.4V5.3H6.1" stroke="currentColor" stroke-width="1.4" fill="none" stroke-linecap="round" stroke-linejoin="round"/><path d="M12.8 13.6V10.7H9.9" stroke="currentColor" stroke-width="1.4" fill="none" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

pub static TRASH: Icon = Icon {
    name: "pa-trash",
    view: 16.0,
    body: r#"<path d="M2.5 4.2H13.5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/><path d="M6 4.2V2.6H10V4.2" stroke="currentColor" stroke-width="1.3" fill="none" stroke-linejoin="round"/><path d="M3.8 4.2L4.6 13.4H11.4L12.2 4.2" stroke="currentColor" stroke-width="1.3" fill="none" stroke-linejoin="round"/>"#,
};

pub static PALETTE: Icon = Icon {
    name: "pa-palette",
    view: 16.0,
    body: r#"<path d="M8 1.8A6.2 6.2 0 1 0 8 14.2C9 14.2 9.3 13.5 8.9 12.8C8.4 12 8.8 10.9 9.9 10.9H11.6A2.6 2.6 0 0 0 14.2 8.3C14.2 4.7 11.4 1.8 8 1.8Z" stroke="currentColor" stroke-width="1.3" fill="none"/><circle cx="5" cy="7.4" r="1" fill="currentColor"/><circle cx="7.4" cy="4.8" r="1" fill="currentColor"/><circle cx="10.6" cy="5.4" r="1" fill="currentColor"/>"#,
};

pub static LOG: Icon = Icon {
    name: "pa-log",
    view: 16.0,
    body: r#"<rect x="2.5" y="2" width="11" height="12" rx="1.8" stroke="currentColor" stroke-width="1.3" fill="none"/><path d="M5.2 5.6H10.8M5.2 8H10.8M5.2 10.4H8.6" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>"#,
};
