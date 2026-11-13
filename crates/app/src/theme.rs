//! The themes, as data, and the semantic colours the views paint with.

use rune::Color;
use rune::color::hex;

/// One theme's palette.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub bg_base: &'static str,
    pub bg_surface: &'static str,
    pub bg_element: &'static str,
    pub bg_header: &'static str,
    pub fg_base: &'static str,
    pub fg_sub: &'static str,
    pub fg_dim: &'static str,
    pub border_base: &'static str,
    pub border_focus: &'static str,
    pub item_focus: &'static str,
    pub item_select: &'static str,
    pub item_marker: &'static str,
    pub progress: &'static str,
    pub key_bind: &'static str,
    pub stat_error: &'static str,
    pub stat_warn: &'static str,
    pub stat_success: &'static str,
    pub stat_info: &'static str,
}

/// The "System" palette: macOS's system colours on a dark window.
const SYSTEM: Palette = Palette {
    bg_base: "#1e1e1e",
    bg_surface: "#161616",
    bg_element: "#2c2c2e",
    bg_header: "#3a3a3c",
    fg_base: "#f5f5f7",
    fg_sub: "#d1d1d6",
    fg_dim: "#98989d",
    border_base: "#3a3a3c",
    border_focus: "#64d2ff",
    item_focus: "#64d2ff",
    item_select: "#f5f5f7",
    item_marker: "#ff453a",
    progress: "#30d158",
    key_bind: "#ffd60a",
    stat_error: "#ff453a",
    stat_warn: "#ffd60a",
    stat_success: "#30d158",
    stat_info: "#64d2ff",
};

const CATPPUCCIN: Palette = Palette {
    bg_base: "#1e1e2e",
    bg_surface: "#181825",
    bg_element: "#313244",
    bg_header: "#6b5885",
    fg_base: "#cdd6f4",
    fg_sub: "#bac2de",
    fg_dim: "#a6adc8",
    border_base: "#45475a",
    border_focus: "#f5c2e7",
    item_focus: "#cba6f7",
    item_select: "#f2cdcd",
    item_marker: "#fab387",
    progress: "#cba6f7",
    key_bind: "#f9e2af",
    stat_error: "#f38ba8",
    stat_warn: "#f9e2af",
    stat_success: "#a6e3a1",
    stat_info: "#89b4fa",
};

const CYBERDREAM: Palette = Palette {
    bg_base: "#16181a",
    bg_surface: "#1e2124",
    bg_element: "#3c4048",
    bg_header: "#16181a",
    fg_base: "#ffffff",
    fg_sub: "#7b8496",
    fg_dim: "#3c4048",
    border_base: "#1e2124",
    border_focus: "#5ea1ff",
    item_focus: "#5ea1ff",
    item_select: "#3c4048",
    item_marker: "#ffbd5e",
    progress: "#5ea1ff",
    key_bind: "#ffbd5e",
    stat_error: "#ff6e5e",
    stat_warn: "#f1ff5e",
    stat_success: "#5eff6c",
    stat_info: "#5ef1ff",
};

const DRACULA: Palette = Palette {
    bg_base: "#282a36",
    bg_surface: "#21222c",
    bg_element: "#44475a",
    bg_header: "#44475a",
    fg_base: "#f8f8f2",
    fg_sub: "#6272a4",
    fg_dim: "#6272a4",
    border_base: "#4d4f68",
    border_focus: "#bd93f9",
    item_focus: "#bd93f9",
    item_select: "#ff79c6",
    item_marker: "#ffb86c",
    progress: "#50fa7b",
    key_bind: "#f1fa8c",
    stat_error: "#ff5555",
    stat_warn: "#ffb86c",
    stat_success: "#50fa7b",
    stat_info: "#8be9fd",
};

const GRUVBOX: Palette = Palette {
    bg_base: "#282828",
    bg_surface: "#3c3836",
    bg_element: "#504945",
    bg_header: "#3c3836",
    fg_base: "#ebdbb2",
    fg_sub: "#bdae93",
    fg_dim: "#a89984",
    border_base: "#504945",
    border_focus: "#fe8019",
    item_focus: "#fe8019",
    item_select: "#fabd2f",
    item_marker: "#d65d0e",
    progress: "#b8bb26",
    key_bind: "#fabd2f",
    stat_error: "#fb4934",
    stat_warn: "#fabd2f",
    stat_success: "#b8bb26",
    stat_info: "#83a598",
};

const KANAGAWA: Palette = Palette {
    bg_base: "#1f1f28",
    bg_surface: "#1f1f28",
    bg_element: "#2a2a37",
    bg_header: "#1f1f28",
    fg_base: "#dcd7ba",
    fg_sub: "#c8c093",
    fg_dim: "#727169",
    border_base: "#223249",
    border_focus: "#7e9cd8",
    item_focus: "#7e9cd8",
    item_select: "#957fb8",
    item_marker: "#ffa066",
    progress: "#7e9cd8",
    key_bind: "#e6c384",
    stat_error: "#c34043",
    stat_warn: "#ffa066",
    stat_success: "#76946a",
    stat_info: "#7fb4ca",
};

const KANSO: Palette = Palette {
    bg_base: "#14171d",
    bg_surface: "#1f1f26",
    bg_element: "#393b44",
    bg_header: "#14171d",
    fg_base: "#c5c9c7",
    fg_sub: "#9ca6a6",
    fg_dim: "#717c7c",
    border_base: "#22262d",
    border_focus: "#7fb4ca",
    item_focus: "#7fb4ca",
    item_select: "#223249",
    item_marker: "#dca561",
    progress: "#98bb6c",
    key_bind: "#dca561",
    stat_error: "#c34043",
    stat_warn: "#dca561",
    stat_success: "#98bb6c",
    stat_info: "#7fb4ca",
};

const MONOKAI: Palette = Palette {
    bg_base: "#272822",
    bg_surface: "#221f22",
    bg_element: "#3e3d32",
    bg_header: "#403e41",
    fg_base: "#fcfcfa",
    fg_sub: "#75715e",
    fg_dim: "#75715e",
    border_base: "#49483e",
    border_focus: "#a6e22e",
    item_focus: "#ffd866",
    item_select: "#ab9df2",
    item_marker: "#fd971f",
    progress: "#a6e22e",
    key_bind: "#e6db74",
    stat_error: "#f92672",
    stat_warn: "#fd971f",
    stat_success: "#a6e22e",
    stat_info: "#66d9ef",
};

const NORD: Palette = Palette {
    bg_base: "#2e3440",
    bg_surface: "#3b4252",
    bg_element: "#434c5e",
    bg_header: "#3b4252",
    fg_base: "#d8dee9",
    fg_sub: "#e5e9f0",
    fg_dim: "#eceff4",
    border_base: "#434c5e",
    border_focus: "#88c0d0",
    item_focus: "#88c0d0",
    item_select: "#81a1c1",
    item_marker: "#d08770",
    progress: "#a3be8c",
    key_bind: "#ebcb8b",
    stat_error: "#bf616a",
    stat_warn: "#ebcb8b",
    stat_success: "#a3be8c",
    stat_info: "#81a1c1",
};

const ONE_DARK: Palette = Palette {
    bg_base: "#282c34",
    bg_surface: "#21252b",
    bg_element: "#2c313a",
    bg_header: "#21252b",
    fg_base: "#abb2bf",
    fg_sub: "#828997",
    fg_dim: "#5c6370",
    border_base: "#3e4451",
    border_focus: "#61afef",
    item_focus: "#61afef",
    item_select: "#c678dd",
    item_marker: "#d19a66",
    progress: "#98c379",
    key_bind: "#e5c07b",
    stat_error: "#e06c75",
    stat_warn: "#e5c07b",
    stat_success: "#98c379",
    stat_info: "#61afef",
};

const ROSE_PINE: Palette = Palette {
    bg_base: "#191724",
    bg_surface: "#1f1d2e",
    bg_element: "#26233a",
    bg_header: "#1f1d2e",
    fg_base: "#e0def4",
    fg_sub: "#908caa",
    fg_dim: "#6e6a86",
    border_base: "#403d52",
    border_focus: "#ebbcba",
    item_focus: "#ebbcba",
    item_select: "#ebbcba",
    item_marker: "#f6c177",
    progress: "#31748f",
    key_bind: "#f6c177",
    stat_error: "#eb6f92",
    stat_warn: "#f6c177",
    stat_success: "#31748f",
    stat_info: "#9ccfd8",
};

const SOLARIZED_DARK: Palette = Palette {
    bg_base: "#002b36",
    bg_surface: "#073642",
    bg_element: "#073642",
    bg_header: "#002b36",
    fg_base: "#839496",
    fg_sub: "#93a1a1",
    fg_dim: "#586e75",
    border_base: "#073642",
    border_focus: "#93a1a1",
    item_focus: "#2aa198",
    item_select: "#586e75",
    item_marker: "#cb4b16",
    progress: "#2aa198",
    key_bind: "#b58900",
    stat_error: "#dc322f",
    stat_warn: "#b58900",
    stat_success: "#859900",
    stat_info: "#268bd2",
};

const TOKYO_NIGHT: Palette = Palette {
    bg_base: "#1a1b26",
    bg_surface: "#1f2335",
    bg_element: "#24283b",
    bg_header: "#1a1b26",
    fg_base: "#c0caf5",
    fg_sub: "#a9b1d6",
    fg_dim: "#565f89",
    border_base: "#414868",
    border_focus: "#7aa2f7",
    item_focus: "#7aa2f7",
    item_select: "#bb9af7",
    item_marker: "#7dcfff",
    progress: "#7aa2f7",
    key_bind: "#ff9e64",
    stat_error: "#db4b4b",
    stat_warn: "#e0af68",
    stat_success: "#9ece6a",
    stat_info: "#7aa2f7",
};

const ZENBURN: Palette = Palette {
    bg_base: "#3f3f3f",
    bg_surface: "#4f4f4f",
    bg_element: "#5f5f5f",
    bg_header: "#4f4f4f",
    fg_base: "#dcdccc",
    fg_sub: "#9faf9f",
    fg_dim: "#7f7f7f",
    border_base: "#5f5f5f",
    border_focus: "#f0dfaf",
    item_focus: "#f0dfaf",
    item_select: "#dc8cc3",
    item_marker: "#dfaf8f",
    progress: "#7f9f7f",
    key_bind: "#f0dfaf",
    stat_error: "#dca3a3",
    stat_warn: "#f0dfaf",
    stat_success: "#7f9f7f",
    stat_info: "#8cd0d3",
};

/// Every theme, in the order the picker lists them.
pub const THEMES: &[(&str, Palette)] = &[
    ("System", SYSTEM),
    ("Catppuccin", CATPPUCCIN),
    ("Cyberdream", CYBERDREAM),
    ("Dracula", DRACULA),
    ("Gruvbox", GRUVBOX),
    ("Kanagawa", KANAGAWA),
    ("Kanso", KANSO),
    ("Monokai", MONOKAI),
    ("Nord", NORD),
    ("One Dark", ONE_DARK),
    ("Rosé Pine", ROSE_PINE),
    ("Solarized Dark", SOLARIZED_DARK),
    ("Tokyo Night", TOKYO_NIGHT),
    ("Zenburn", ZENBURN),
];

pub const DEFAULT_THEME: &str = "Catppuccin";

pub fn theme_index(name: &str) -> Option<usize> {
    THEMES.iter().position(|(n, _)| *n == name)
}

pub fn is_theme(name: &str) -> bool {
    theme_index(name).is_some()
}

/// The semantic colours, resolved to linear colours, plus a few the window
/// needs that a terminal doesn't (scrim, shadow, hover tints).
#[derive(Clone, Copy, Debug)]
pub struct Colors {
    pub background: Color,
    pub surface: Color,
    pub element: Color,
    pub text: Color,
    pub text_secondary: Color,
    pub text_dim: Color,
    pub error: Color,
    pub warning: Color,
    pub success: Color,
    pub accent: Color,
    pub border_focused: Color,
    pub border: Color,
    pub list_focused: Color,
    pub list_selected: Color,
    pub list_focused_selected: Color,
    pub marker_selected: Color,
    pub marker_cursor: Color,
    pub header_bg: Color,
    pub header_text: Color,
    pub popup_border: Color,
    pub popup_bg: Color,
    pub progress: Color,
    pub progress_empty: Color,
    pub footer_key: Color,
    pub footer_text: Color,
    pub footer_separator: Color,
    pub status_debug: Color,
    pub status_drive: Color,
    pub status_app: Color,
    pub hover: Color,
    pub press: Color,
    pub scrim: Color,
    pub shadow: Color,
}

fn c(s: &str) -> Color {
    hex(s).unwrap_or(Color::BLACK)
}

impl Colors {
    pub fn of(name: &str) -> Colors {
        let p = THEMES.iter().find(|(n, _)| *n == name).map_or(CATPPUCCIN, |(_, p)| *p);
        Colors::from_palette(&p)
    }

    pub fn from_palette(p: &Palette) -> Colors {
        let fg = c(p.fg_base);
        Colors {
            background: c(p.bg_base),
            surface: c(p.bg_surface),
            element: c(p.bg_element),
            text: fg,
            text_secondary: c(p.fg_sub),
            text_dim: c(p.fg_dim),
            error: c(p.stat_error),
            warning: c(p.stat_warn),
            success: c(p.stat_success),
            accent: c(p.item_focus),
            border_focused: c(p.border_focus),
            border: c(p.border_base),
            list_focused: c(p.item_focus),
            list_selected: c(p.item_select),
            list_focused_selected: c(p.border_focus),
            marker_selected: c(p.item_marker),
            marker_cursor: c(p.item_focus),
            header_bg: c(p.bg_header),
            header_text: fg,
            popup_border: c(p.border_focus),
            popup_bg: c(p.bg_surface),
            progress: c(p.progress),
            progress_empty: c(p.bg_element),
            footer_key: c(p.key_bind),
            footer_text: c(p.fg_dim),
            footer_separator: c(p.item_select),
            status_debug: c(p.stat_error),
            status_drive: c(p.item_select),
            status_app: c(p.stat_info),
            hover: fg.alpha(0.045),
            press: fg.alpha(0.08),
            scrim: Color::BLACK.alpha(0.5),
            shadow: Color::BLACK.alpha(0.45),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_theme_parses_and_names_are_unique() {
        for (name, p) in THEMES {
            for s in [
                p.bg_base, p.bg_surface, p.bg_element, p.bg_header, p.fg_base, p.fg_sub, p.fg_dim,
                p.border_base, p.border_focus, p.item_focus, p.item_select, p.item_marker, p.progress,
                p.key_bind, p.stat_error, p.stat_warn, p.stat_success, p.stat_info,
            ] {
                assert!(hex(s).is_some(), "{name}: {s}");
            }
            assert_eq!(THEMES.iter().filter(|(n, _)| n == name).count(), 1);
        }
        assert_eq!(THEMES.len(), 14);
        assert!(is_theme(DEFAULT_THEME));
    }
}
