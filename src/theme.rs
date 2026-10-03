//! Themes. Every color the UI draws comes from a `Theme`, so a theme is one entry in `SPECS`, one of the theme files
//! compiled in (`FILES`, in `src/themes/`), or a user's theme file. The built-ins are compiled in and always in the
//! order the picker will show them. A theme may be light (`Theme::light`; eight of the files are): the contrast rules
//! are symmetric (WCAG's ratio doesn't care which side is lighter), and everything that is lifted to read goes toward
//! black on a light theme instead of white.
//!
//! A fixed theme is a `bg`/`text` pair plus a 16-color palette; the eleven UI roles are picked from the
//! palette (or given as a hex), and whatever is derived (surface, border, dim, subtext) or too faint to read
//! is lifted toward the text color until it passes the contrast floors below. The palette also recolors the
//! Claude session shown inside platoon (`ansi`). "Terminal colors" has no palette of its own: it uses the
//! terminal's own colors, so it follows whatever theme the terminal has.

use crate::usertheme::Registry;
use ratatui::style::Color;

/// WCAG contrast floors against `bg`, for dark and light themes alike (see `lift_contrast`).
pub const MIN_TEXT: f64 = 7.0;
pub const MIN_SUBTEXT: f64 = 4.5;
pub const MIN_DIM: f64 = 3.0;
pub const MIN_ROLE: f64 = 4.5;
/// The least Lab distance (`delta_e`) between two of accent / busy / ok / warn / err. Set from the closest pair
/// among the first built-ins (Catppuccin Mocha's lavender accent and mauve busy, 19, whose values are fixed by the
/// original design; Melange Dark's amber accent and yellow warn, 18.8, are now the closest): every status is told
/// from every other, and from the accent, by more than a hair. A
/// pair whose difference is mostly lightness would still pass, which is why each role is also checked
/// against the background. User themes are held to the same floor.
pub const MIN_DELTA_E: f64 = 18.0;

/// A palette. Every color the UI uses comes from here.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    /// Stable slug stored in settings.json: `catppuccin-mocha`, `tokyo-night`, `terminal`.
    pub id: String,
    pub name: String,
    pub bg: Color,
    pub surface: Color,
    pub border: Color,
    pub dim: Color,
    pub subtext: Color,
    pub text: Color,
    pub accent: Color,
    pub busy: Color,
    pub ok: Color,
    pub warn: Color,
    pub err: Color,
    /// Text drawn on an `accent` fill (the count badge of the current tab). `bg` for the fixed themes; the
    /// terminal's own `bg` is "whatever the terminal has", which may be light, so Terminal colors uses black.
    pub on_accent: Color,
    /// The theme's 16 terminal colors, used to recolor sessions shown inside platoon. `None`: leave the
    /// emulator's colors as they are (Terminal colors, where the terminal does the coloring).
    pub ansi: Option<[Color; 16]>,
    /// Drawn on a light background (dark text): the light built-in theme files and the user's own light themes. False
    /// for every `SPECS` theme and for Terminal colors, whose background platoon can't know. Anything that needs
    /// "lighter" or "darker than the text" asks this (the code editor's `background`, the text on a project's colored badge).
    pub light: bool,
}

/// A background at least this bright (WCAG luminance) makes a light theme; below it, a dark one. The `SPECS` themes
/// are all well below it.
pub const LIGHT_LUMINANCE: f64 = 0.2;

/// Whether `bg` is a light theme's background.
pub fn is_light(bg: Color) -> bool {
    luminance(bg) >= LIGHT_LUMINANCE
}

impl Default for Theme {
    fn default() -> Self {
        Self::all().swap_remove(0)
    }
}

pub const DEFAULT_ID: &str = "catppuccin-mocha";

/// How far the selection color is from the background toward the accent.
const SELECTION_MIX: f64 = 0.30;
/// What the theme's text must reach on the selection color (WCAG's floor for normal text).
pub const SELECTION_TEXT_CONTRAST: f64 = 4.5;

/// Where a role's color comes from.
#[derive(Clone, Copy)]
enum Pick {
    /// An entry of the palette.
    P(usize),
    /// A color that isn't in the palette, where the palette's own is a poor fit (see the notes on each theme).
    X(u32),
}

/// Surface, border, dim and subtext when they are hand-tuned rather than mixed from bg and text.
type Tuned = [u32; 4];

struct Spec {
    id: &'static str,
    name: &'static str,
    bg: u32,
    text: u32,
    /// ANSI colors 0..=15 as the terminal theme defines them.
    palette: [u32; 16],
    tuned: Option<Tuned>,
    accent: Pick,
    busy: Pick,
    ok: Pick,
    warn: Pick,
    err: Pick,
}

use Pick::{P, X};

/// The built-in themes in display order. Palettes are the ones Ghostty bundles (see THIRD_PARTY_NOTICES.md).
///
/// Picks that leave the palette, and why: `ok` must read as green and `err` as red at a glance, `warn` as
/// yellow, and `busy` must not collide with the other statuses.
/// - Catppuccin Macchiato: accent/busy are the official lavender and mauve (the palette has no such entries),
///   matching what Mocha does.
/// - Nord: `err` is a lighter red than the palette's, which is too dark to read on the background.
/// - Gruvbox: accent is the bright orange; busy is purple, because yellow is `warn` here.
/// - Rose Pine: the palette has no green (its "green" is a teal), so `ok` is a soft green in the theme's tone.
/// - Kanagawa: busy is the theme's orange and `err` its lighter wave red (the palette's red is too dark).
/// - Ayu Mirage: accent is the theme's orange.
/// - Vesper: accent is its amber (palette 11), so `warn` is a lemon yellow to stay apart from it.
/// - Catppuccin Frappé: accent/busy are the official lavender and mauve, like Mocha and Macchiato; surface,
///   border, dim and subtext are Frappé's own surface0, surface1, overlay1 and subtext0 (the mixed dim was too
///   faint on a selected row).
/// - Gruvbox Material Dark: accent is the theme's orange (not in the palette), like Gruvbox Dark; surface, border,
///   dim and subtext are its own bg1, bg3, grey1 and grey2 (the mixed dim was too faint on a selected row).
/// - Rosé Pine Moon: like Rosé Pine, the palette's "green" is pine (a blue), so `ok` is the same soft green.
/// - Kanagawa Dragon: busy is the theme's dragon orange (not in the palette); its violet sat too close to the
///   blue accent.
/// - Solarized Dark: Solarized's accents are all a middle lightness that reads under 4.5:1 on its background, and
///   lifting them toward its grey text washes them out, so accent (violet) and err (red) are Solarized's own
///   violet and red made lighter by hand. Violet rather than blue: a blue selection turns too light in 256 colors.
///   The body text is lifted, as for every theme whose foreground reads under 7:1.
/// - Flexoki Dark: busy is Flexoki's purple (the palette has magenta but no purple); accent is its cyan, because
///   the blue is too dark for a visible selection color.
/// - Oxocarbon: the palette has no yellow and its "red" is a cyan, so `warn` and `err` are IBM Carbon's yellow
///   and red (Oxocarbon is built on IBM's Carbon design colors).
/// - Poimandres: `ok` is its mint (palette 2), the nearest it has to green, and `err` its hot red (a pink-red).
/// - Horizon: the palette's yellow is a peach, so `warn` is an amber of my own in the same warm tone.
/// - Synthwave: the palette's red is a hot pink (used as busy), so `err` is its red-orange (palette 5).
const SPECS: &[Spec] = &[
    Spec {
        id: "catppuccin-mocha",
        name: "Catppuccin Mocha",
        bg: 0x1e1e2e,
        text: 0xcdd6f4,
        palette: [
            0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8, 0x585b70, 0xf37799, 0x89d88b, 0xebd391,
            0x74a8fc, 0xf2aede, 0x6bd7ca, 0xbac2de,
        ],
        // The original hand-tuned colors, unchanged.
        tuned: Some([0x313244, 0x45475a, 0x6c7086, 0xa6adc8]),
        accent: X(0xb4befe),
        busy: X(0xcba6f7),
        ok: X(0xa6e3a1),
        warn: X(0xf9e2af),
        err: X(0xf38ba8),
    },
    Spec {
        id: "catppuccin-macchiato",
        name: "Catppuccin Macchiato",
        bg: 0x24273a,
        text: 0xcad3f5,
        palette: [
            0x494d64, 0xed8796, 0xa6da95, 0xeed49f, 0x8aadf4, 0xf5bde6, 0x8bd5ca, 0xa5adcb, 0x5b6078, 0xec7486, 0x8ccf7f, 0xe1c682,
            0x78a1f6, 0xf2a9dd, 0x63cbc0, 0xb8c0e0,
        ],
        tuned: None,
        accent: X(0xb7bdf8),
        busy: X(0xc6a0f6),
        ok: P(2),
        warn: P(3),
        err: P(1),
    },
    Spec {
        id: "catppuccin-frappe",
        name: "Catppuccin Frappé",
        bg: 0x303446,
        text: 0xc6d0f5,
        palette: [
            0x51576d, 0xe78284, 0xa6d189, 0xe5c890, 0x8caaee, 0xf4b8e4, 0x81c8be, 0xa5adce, 0x626880, 0xe67172, 0x8ec772, 0xd9ba73,
            0x7b9ef0, 0xf2a4db, 0x5abfb5, 0xb5bfe2,
        ],
        tuned: Some([0x414559, 0x51576d, 0x838ba7, 0xa5adce]),
        accent: X(0xbabbf1),
        busy: X(0xca9ee6),
        ok: P(2),
        warn: P(3),
        err: P(1),
    },
    Spec {
        id: "tokyo-night",
        name: "Tokyo Night",
        bg: 0x1a1b26,
        text: 0xc0caf5,
        palette: [
            0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6, 0x414868, 0xf7768e, 0x9ece6a, 0xe0af68,
            0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5,
        ],
        tuned: None,
        accent: P(12),
        busy: P(13),
        ok: P(10),
        warn: P(11),
        err: P(9),
    },
    Spec {
        id: "tokyo-night-storm",
        name: "Tokyo Night Storm",
        bg: 0x24283b,
        text: 0xc0caf5,
        palette: [
            0x1d202f, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6, 0x4e5575, 0xf7768e, 0x9ece6a, 0xe0af68,
            0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5,
        ],
        tuned: None,
        accent: P(12),
        busy: P(13),
        ok: P(10),
        warn: P(11),
        err: P(9),
    },
    Spec {
        id: "dracula",
        name: "Dracula",
        bg: 0x282a36,
        text: 0xf8f8f2,
        palette: [
            0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2, 0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5,
            0xd6acff, 0xff92df, 0xa4ffff, 0xffffff,
        ],
        tuned: None,
        accent: P(4), // purple
        busy: P(5),   // pink
        ok: P(2),
        warn: P(3),
        err: P(1),
    },
    Spec {
        id: "one-dark",
        name: "One Dark",
        bg: 0x21252b,
        text: 0xabb2bf,
        palette: [
            0x21252b, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf, 0x767676, 0xe06c75, 0x98c379, 0xe5c07b,
            0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
        ],
        tuned: None,
        accent: P(12),
        busy: P(13),
        ok: P(10),
        warn: P(11),
        err: P(9),
    },
    Spec {
        id: "nord",
        name: "Nord",
        bg: 0x2e3440,
        text: 0xd8dee9,
        palette: [
            0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0, 0x596377, 0xbf616a, 0xa3be8c, 0xebcb8b,
            0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4,
        ],
        tuned: None,
        accent: P(6), // frost
        busy: P(13),
        ok: P(10),
        warn: P(11),
        err: X(0xe0838c),
    },
    Spec {
        id: "gruvbox-dark",
        name: "Gruvbox Dark",
        bg: 0x282828,
        text: 0xebdbb2,
        palette: [
            0x282828, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984, 0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f,
            0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2,
        ],
        tuned: None,
        accent: X(0xfe8019), // orange
        busy: P(13),
        ok: P(10),
        warn: P(11),
        err: P(9),
    },
    Spec {
        id: "gruvbox-material-dark",
        name: "Gruvbox Material Dark",
        bg: 0x282828,
        text: 0xd4be98,
        palette: [
            0x282828, 0xea6962, 0xa9b665, 0xd8a657, 0x7daea3, 0xd3869b, 0x89b482, 0xd4be98, 0x7c6f64, 0xea6962, 0xa9b665, 0xd8a657,
            0x7daea3, 0xd3869b, 0x89b482, 0xddc7a1,
        ],
        tuned: Some([0x32302f, 0x45403d, 0x928374, 0xa89984]),
        accent: X(0xe78a4e), // orange
        busy: P(5),
        ok: P(2),
        warn: P(3),
        err: P(1),
    },
    Spec {
        id: "rose-pine",
        name: "Rosé Pine",
        bg: 0x191724,
        text: 0xe0def4,
        palette: [
            0x26233a, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4, 0x6e6a86, 0xeb6f92, 0x31748f, 0xf6c177,
            0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4,
        ],
        tuned: None,
        accent: P(5), // iris
        busy: P(6),   // rose
        ok: X(0x9ccfa0),
        warn: P(3),
        err: P(1),
    },
    Spec {
        id: "rose-pine-moon",
        name: "Rosé Pine Moon",
        bg: 0x232136,
        text: 0xe0def4,
        palette: [
            0x393552, 0xeb6f92, 0x3e8fb0, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xea9a97, 0xe0def4, 0x6e6a86, 0xeb6f92, 0x3e8fb0, 0xf6c177,
            0x9ccfd8, 0xc4a7e7, 0xea9a97, 0xe0def4,
        ],
        tuned: None,
        accent: P(5), // iris
        busy: P(6),   // rose
        ok: X(0x9ccfa0),
        warn: P(3),
        err: P(1),
    },
    Spec {
        id: "kanagawa-wave",
        name: "Kanagawa Wave",
        bg: 0x1f1f28,
        text: 0xdcd7ba,
        palette: [
            0x090618, 0xc34043, 0x76946a, 0xc0a36e, 0x7e9cd8, 0x957fb8, 0x6a9589, 0xc8c093, 0x727169, 0xe82424, 0x98bb6c, 0xe6c384,
            0x7fb4ca, 0x938aa9, 0x7aa89f, 0xdcd7ba,
        ],
        tuned: None,
        accent: P(4), // crystal blue
        busy: X(0xffa066),
        ok: P(10),
        warn: P(11),
        err: X(0xe46876),
    },
    Spec {
        id: "kanagawa-dragon",
        name: "Kanagawa Dragon",
        bg: 0x181616,
        text: 0xc5c9c5,
        palette: [
            0x0d0c0c, 0xc4746e, 0x8a9a7b, 0xc4b28a, 0x8ba4b0, 0xa292a3, 0x8ea4a2, 0xc8c093, 0xa6a69c, 0xe46876, 0x87a987, 0xe6c384,
            0x7fb4ca, 0x938aa9, 0x7aa89f, 0xc5c9c5,
        ],
        tuned: None,
        accent: P(4),      // dragon blue
        busy: X(0xb6927b), // dragon orange
        ok: P(10),
        warn: P(11),
        err: P(9),
    },
    Spec {
        id: "night-owl",
        name: "Night Owl",
        bg: 0x011627,
        text: 0xd6deeb,
        palette: [
            0x011627, 0xef5350, 0x22da6e, 0xaddb67, 0x82aaff, 0xc792ea, 0x21c7a8, 0xffffff, 0x575656, 0xef5350, 0x22da6e, 0xffeb95,
            0x82aaff, 0xc792ea, 0x7fdbca, 0xffffff,
        ],
        tuned: None,
        accent: P(12), // blue
        busy: P(14),   // cyan
        ok: P(10),
        warn: P(11),
        err: P(9),
    },
    Spec {
        id: "ayu-mirage",
        name: "Ayu Mirage",
        bg: 0x1f2430,
        text: 0xcccac2,
        palette: [
            0x171b24, 0xed8274, 0x87d96c, 0xfacc6e, 0x6dcbfa, 0xdabafa, 0x90e1c6, 0xc7c7c7, 0x686868, 0xf28779, 0xd5ff80, 0xffd173,
            0x73d0ff, 0xdfbfff, 0x95e6cb, 0xffffff,
        ],
        tuned: None,
        accent: X(0xffa759), // orange
        busy: P(12),
        ok: P(2),
        warn: P(11),
        err: P(9),
    },
    Spec {
        id: "vesper",
        name: "Vesper",
        bg: 0x101010,
        text: 0xffffff,
        palette: [
            0x101010, 0xf5a191, 0x90b99f, 0xe6b99d, 0xaca1cf, 0xe29eca, 0xea83a5, 0xa0a0a0, 0x7e7e7e, 0xff8080, 0x99ffe4, 0xffc799,
            0xb9aeda, 0xecaad6, 0xf591b2, 0xffffff,
        ],
        tuned: None,
        accent: P(11), // amber
        busy: P(12),
        ok: P(2),
        warn: X(0xf2e07f),
        err: P(9),
    },
    Spec {
        id: "solarized-dark",
        name: "Solarized Dark",
        bg: 0x002b36,
        text: 0x839496,
        palette: [
            0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5, 0x335e69, 0xcb4b16, 0x586e75, 0x657b83,
            0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3,
        ],
        tuned: None,
        accent: X(0x8488cd), // violet, lightened
        busy: P(6),          // cyan
        ok: P(2),
        warn: P(3),
        err: X(0xe56563), // red, lightened
    },
    Spec {
        id: "github-dark",
        name: "GitHub Dark",
        bg: 0x0d1117,
        text: 0xe6edf3,
        palette: [
            0x484f58, 0xff7b72, 0x3fb950, 0xd29922, 0x58a6ff, 0xbc8cff, 0x39c5cf, 0xb1bac4, 0x6e7681, 0xffa198, 0x56d364, 0xe3b341,
            0x79c0ff, 0xd2a8ff, 0x56d4dd, 0xffffff,
        ],
        tuned: None,
        accent: P(4),
        busy: P(5),
        ok: P(2),
        warn: P(3),
        err: P(1),
    },
    Spec {
        id: "material-ocean",
        name: "Material Ocean",
        bg: 0x0f111a,
        text: 0x8f93a2,
        palette: [
            0x546e7a, 0xff5370, 0xc3e88d, 0xffcb6b, 0x82aaff, 0xc792ea, 0x89ddff, 0xffffff, 0x546e7a, 0xff5370, 0xc3e88d, 0xffcb6b,
            0x82aaff, 0xc792ea, 0x89ddff, 0xffffff,
        ],
        tuned: None,
        accent: P(4),
        busy: P(5),
        ok: P(2),
        warn: P(3),
        err: P(1),
    },
    Spec {
        id: "everforest-dark-hard",
        name: "Everforest Dark Hard",
        bg: 0x1e2326,
        text: 0xd3c6aa,
        palette: [
            0x7a8478, 0xe67e80, 0xa7c080, 0xdbbc7f, 0x7fbbb3, 0xd699b6, 0x83c092, 0xf2efdf, 0xa6b0a0, 0xf85552, 0x8da101, 0xdfa000,
            0x3a94c5, 0xdf69ba, 0x35a77c, 0xfffbef,
        ],
        tuned: None,
        accent: P(4), // blue
        busy: P(5),   // purple
        ok: P(2),
        warn: P(3),
        err: P(1),
    },
    Spec {
        id: "nightfox",
        name: "Nightfox",
        bg: 0x192330,
        text: 0xcdcecf,
        palette: [
            0x393b44, 0xc94f6d, 0x81b29a, 0xdbc074, 0x719cd6, 0x9d79d6, 0x63cdcf, 0xdfdfe0, 0x575860, 0xd16983, 0x8ebaa4, 0xe0c989,
            0x86abdc, 0xbaa1e2, 0x7ad5d6, 0xe4e4e5,
        ],
        tuned: None,
        accent: P(12),
        busy: P(13),
        ok: P(10),
        warn: P(11),
        err: P(9),
    },
    Spec {
        id: "sonokai",
        name: "Sonokai",
        bg: 0x2c2e34,
        text: 0xe2e2e3,
        palette: [
            0x181819, 0xfc5d7c, 0x9ed072, 0xe7c664, 0x76cce0, 0xb39df3, 0xf39660, 0xe2e2e3, 0x7f8490, 0xfc5d7c, 0x9ed072, 0xe7c664,
            0x76cce0, 0xb39df3, 0xf39660, 0xe2e2e3,
        ],
        tuned: None,
        accent: P(4), // blue
        busy: P(5),   // purple
        ok: P(2),
        warn: P(3),
        err: P(1),
    },
    Spec {
        id: "moonfly",
        name: "Moonfly",
        bg: 0x080808,
        text: 0xbdbdbd,
        palette: [
            0x323437, 0xff5454, 0x8cc85f, 0xe3c78a, 0x80a0ff, 0xcf87e8, 0x79dac8, 0xc6c6c6, 0x949494, 0xff5189, 0x36c692, 0xc6c684,
            0x74b2ff, 0xae81ff, 0x85dc85, 0xe4e4e4,
        ],
        tuned: None,
        accent: P(4), // blue
        busy: P(5),   // violet
        ok: P(2),
        warn: P(3),
        err: P(1),
    },
    Spec {
        id: "melange-dark",
        name: "Melange Dark",
        bg: 0x292522,
        text: 0xece1d7,
        palette: [
            0x34302c, 0xbd8183, 0x78997a, 0xe49b5d, 0x7f91b2, 0xb380b0, 0x7b9695, 0xc1a78e, 0x867462, 0xd47766, 0x85b695, 0xebc06d,
            0xa3a9ce, 0xcf9bc2, 0x89b3b6, 0xece1d7,
        ],
        tuned: None,
        accent: P(3), // amber
        busy: P(13),
        ok: P(10),
        warn: P(11),
        err: P(9),
    },
    Spec {
        id: "flexoki-dark",
        name: "Flexoki Dark",
        bg: 0x100f0f,
        text: 0xcecdc3,
        palette: [
            0x100f0f, 0xd14d41, 0x879a39, 0xd0a215, 0x4385be, 0xce5d97, 0x3aa99f, 0x878580, 0x575653, 0xaf3029, 0x66800b, 0xad8301,
            0x205ea6, 0xa02f6f, 0x24837b, 0xcecdc3,
        ],
        tuned: None,
        accent: P(6),      // cyan
        busy: X(0x8b7ec8), // purple
        ok: P(2),
        warn: P(3),
        err: P(1),
    },
    Spec {
        id: "oxocarbon",
        name: "Oxocarbon",
        bg: 0x161616,
        text: 0xf2f4f8,
        palette: [
            0x161616, 0x00dfdb, 0x00b4ff, 0xff4297, 0x00c15a, 0xc693ff, 0xff74b8, 0xf2f4f8, 0x585858, 0x00dfdb, 0x00b4ff, 0xff4297,
            0x00c15a, 0xc693ff, 0xff74b8, 0xf2f4f8,
        ],
        tuned: None,
        accent: P(2),      // blue
        busy: P(5),        // purple
        ok: P(4),          // green
        warn: X(0xf1c21b), // IBM Carbon yellow
        err: X(0xfa4d56),  // IBM Carbon red
    },
    Spec {
        id: "poimandres",
        name: "Poimandres",
        bg: 0x1a1e28,
        text: 0xa6accd,
        palette: [
            0x1a1e28, 0xd0679d, 0x5de4c7, 0xfffac2, 0x89ddff, 0xfcc5e9, 0xadd7ff, 0xffffff, 0xa6accd, 0xd0679d, 0x5de4c7, 0xfffac2,
            0xadd7ff, 0xfae4fc, 0x89ddff, 0xffffff,
        ],
        tuned: None,
        accent: P(6), // light blue
        busy: P(5),   // pink
        ok: P(2),     // mint
        warn: P(3),
        err: P(1),
    },
    Spec {
        id: "horizon",
        name: "Horizon",
        bg: 0x1c1e26,
        text: 0xd5d8da,
        palette: [
            0x000000, 0xe95678, 0x29d398, 0xfab795, 0x26bbd9, 0xee64ac, 0x59e1e3, 0xe5e5e5, 0x666666, 0xec6a88, 0x3fdaa4, 0xfbc3a7,
            0x3fc4de, 0xf075b5, 0x6be4e6, 0xe5e5e5,
        ],
        tuned: None,
        accent: P(4), // cyan
        busy: P(5),   // pink
        ok: P(2),
        warn: X(0xf9c46b),
        err: P(1),
    },
    Spec {
        id: "synthwave",
        name: "Synthwave",
        bg: 0x000000,
        text: 0xdad9c7,
        palette: [
            0x000000, 0xf6188f, 0x1ebb2b, 0xfdf834, 0x2186ec, 0xf85a21, 0x12c3e2, 0xffffff, 0x7f7094, 0xf841a0, 0x25c141, 0xfdf454,
            0x2f9ded, 0xf97137, 0x19cde6, 0xffffff,
        ],
        tuned: None,
        accent: P(14), // cyan
        busy: P(9),    // hot pink
        ok: P(10),
        warn: P(11),
        err: P(5), // red-orange
    },
    // Claude Code's own dark palette (taken from its binary). The 16 colors are built around it: red is its
    // error rose, green its success green, yellow its warning amber, blue its periwinkle, magenta its purple
    // (the auto-accept color) and cyan its teal (plan mode); black/white/bright-black are the brand greys.
    Spec {
        id: "claude",
        name: "Claude",
        bg: 0x141413,
        text: 0xfaf9f5,
        palette: [
            0x2a2926, 0xff6b80, 0x4eba65, 0xffc107, 0x8a9bf0, 0xaf87ff, 0x48968c, 0xb0aea5, 0x73726c, 0xff8fa0, 0x7dd68e, 0xffd54f,
            0xb1b9f9, 0xc7a8ff, 0x6fc3b7, 0xfaf9f5,
        ],
        tuned: Some([0x2a2926, 0x3d3c38, 0x73726c, 0xb0aea5]),
        accent: X(0xd77757), // the terracotta
        busy: X(0xaf87ff),
        ok: X(0x4eba65),
        warn: X(0xffc107),
        err: X(0xff6b80),
    },
];

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl Spec {
    fn build(&self) -> Theme {
        let (bg, text) = (rgb(self.bg), rgb(self.text));
        let pick = |p: Pick| match p {
            P(i) => rgb(self.palette[i]),
            X(hex) => rgb(hex),
        };
        let role = |p: Pick| lift_contrast(pick(p), bg, text, MIN_ROLE);
        let [surface, border, dim, subtext] = match self.tuned {
            Some(t) => t.map(rgb),
            None => [0.09, 0.20, 0.46, 0.72].map(|t| mix(bg, text, t)),
        };
        Theme {
            id: self.id.into(),
            name: self.name.into(),
            bg,
            surface,
            border,
            dim: lift_contrast(dim, bg, text, MIN_DIM),
            subtext: lift_contrast(subtext, bg, text, MIN_SUBTEXT),
            text: lift_contrast(text, bg, Color::Rgb(255, 255, 255), MIN_TEXT),
            accent: role(self.accent),
            busy: role(self.busy),
            ok: role(self.ok),
            warn: role(self.warn),
            err: role(self.err),
            on_accent: bg,
            ansi: Some(self.palette.map(rgb)),
            light: false,
        }
    }
}

/// The built-in themes kept as theme files (the format of the user's own and of Ghostty's), in display order: twelve
/// dark, then eight light. Each file says where its colors come from and what platoon picked by hand
/// (THIRD_PARTY_NOTICES.md credits them). They are built the way a user's theme file is (`usertheme::build`), and each
/// reads as it is (a test checks), so nothing is lifted; the id is the name's slug (`Rosé Pine Dawn` is
/// `rose-pine-dawn`).
const FILES: [(&str, &str); 20] = [
    ("Harbor Night", include_str!("themes/Harbor Night")),
    ("Moss Lantern", include_str!("themes/Moss Lantern")),
    ("Ayu Dark", include_str!("themes/Ayu Dark")),
    ("Doom One", include_str!("themes/Doom One")),
    ("Embark", include_str!("themes/Embark")),
    ("Everblush", include_str!("themes/Everblush")),
    ("GitHub Dark Dimmed", include_str!("themes/GitHub Dark Dimmed")),
    ("Iceberg Dark", include_str!("themes/Iceberg Dark")),
    ("Jellybeans", include_str!("themes/Jellybeans")),
    ("Srcery", include_str!("themes/Srcery")),
    ("Tomorrow Night", include_str!("themes/Tomorrow Night")),
    ("Zenbones Dark", include_str!("themes/Zenbones Dark")),
    ("Catppuccin Latte", include_str!("themes/Catppuccin Latte")),
    ("Dayfox", include_str!("themes/Dayfox")),
    ("Flexoki Light", include_str!("themes/Flexoki Light")),
    ("GitHub Light", include_str!("themes/GitHub Light")),
    ("Gruvbox Light", include_str!("themes/Gruvbox Light")),
    ("One Half Light", include_str!("themes/One Half Light")),
    ("Rosé Pine Dawn", include_str!("themes/Rosé Pine Dawn")),
    ("Solarized Light", include_str!("themes/Solarized Light")),
];

/// The themes of `FILES`, read once.
fn from_files() -> &'static [Theme] {
    static BUILT: std::sync::OnceLock<Vec<Theme>> = std::sync::OnceLock::new();
    BUILT.get_or_init(|| {
        FILES
            .iter()
            .filter_map(|(name, text)| {
                let parsed = crate::usertheme::parse(text).ok()?;
                Some(crate::usertheme::build(crate::usertheme::slug(name), (*name).to_string(), &parsed))
            })
            .collect()
    })
}

impl Theme {
    /// Every built-in, in display order: the default first, then the rest of `SPECS`, then the theme files compiled
    /// in (`FILES`: twelve dark, then eight light), Terminal colors last.
    pub fn all() -> Vec<Theme> {
        SPECS.iter().map(Spec::build).chain(from_files().iter().cloned()).chain([Self::terminal()]).collect()
    }

    /// A built-in by id. The user's own themes are found through `Registry::by_id`.
    pub fn by_id(id: &str) -> Option<Theme> {
        Self::all().into_iter().find(|t| t.id == id)
    }

    /// The color text moves toward to stand out more: white on a dark theme, black on a light one.
    pub fn strongest(&self) -> Color {
        if self.light { Color::Rgb(0, 0, 0) } else { Color::Rgb(255, 255, 255) }
    }

    /// Text for a fill of `fill` (a pill, a badge): the more readable of the theme's background and text, or, when
    /// neither reads at 4.5:1 (a middling color like a light theme's yellow), black or white, whichever reads better.
    pub fn text_on(&self, fill: Color) -> Color {
        let best = |a: Color, b: Color| if contrast(a, fill) >= contrast(b, fill) { a } else { b };
        let own = best(self.bg, self.text);
        if contrast(own, fill) >= MIN_ROLE {
            return own;
        }
        best(Color::Rgb(0, 0, 0), Color::Rgb(255, 255, 255))
    }

    /// One of the user's own theme files rather than a built-in.
    pub fn is_user(&self) -> bool {
        self.id.starts_with(crate::usertheme::ID_PREFIX)
    }

    /// The background of selected text in an attached session: the theme's background moved part of the way
    /// to its accent, so it is recognisably this theme's and clearly not the background, then darkened as far
    /// as it takes for the theme's text to keep `SELECTION_TEXT_CONTRAST` on it. None when the theme has no
    /// colors of its own to mix (Terminal colors), where selected cells are drawn reversed instead.
    pub fn selection_bg(&self) -> Option<Color> {
        if ![self.bg, self.text, self.accent].iter().all(|c| matches!(c, Color::Rgb(..))) {
            return None;
        }
        Some(lift_contrast(mix(self.bg, self.accent, SELECTION_MIX), self.text, self.bg, SELECTION_TEXT_CONTRAST))
    }

    /// Whatever the terminal has: its default colors for the background and text, and its own ANSI colors for
    /// everything else, so this follows the terminal's theme. Limits: the terminal decides how far apart these
    /// are, and a terminal theme whose black is the same as its background shows no selection band (the
    /// accent marker beside the selected row still does). Bright black is dim text and borders: on the
    /// usual dark themes it is a mid grey, distinct from both the background and black. Nothing here is a
    /// fixed color, so it is also what a terminal with 16 colors gets, and `ansi` is `None`: the session's
    /// colors are the terminal's already.
    pub fn terminal() -> Theme {
        Theme {
            id: "terminal".into(),
            name: "Terminal colors".into(),
            bg: Color::Reset,
            surface: Color::Black,
            border: Color::DarkGray,
            dim: Color::DarkGray,
            subtext: Color::Gray,
            text: Color::Reset,
            accent: Color::LightBlue,
            busy: Color::LightMagenta,
            ok: Color::LightGreen,
            warn: Color::LightYellow,
            err: Color::LightRed,
            on_accent: Color::Black,
            ansi: None,
            light: false,
        }
    }

    /// The theme's roles by name, in the order the swatches and `platoon themes` show them.
    pub fn roles(&self) -> [(&'static str, Color); 11] {
        [
            ("bg", self.bg),
            ("surface", self.surface),
            ("border", self.border),
            ("dim", self.dim),
            ("subtext", self.subtext),
            ("text", self.text),
            ("accent", self.accent),
            ("busy", self.busy),
            ("ok", self.ok),
            ("warn", self.warn),
            ("err", self.err),
        ]
    }
}

// --- `platoon themes` ---------------------------------------------------------------------------------

/// The SGR parameters that select `c` as a foreground (or background) color.
fn sgr(c: Color, fg: bool) -> String {
    let base = if fg { 30 } else { 40 };
    let ansi16 = |i: u8| if i < 8 { format!("{}", base + i) } else { format!("{}", base + 60 + i - 8) };
    match c {
        Color::Reset => format!("{}", base + 9),
        Color::Black => ansi16(0),
        Color::Red => ansi16(1),
        Color::Green => ansi16(2),
        Color::Yellow => ansi16(3),
        Color::Blue => ansi16(4),
        Color::Magenta => ansi16(5),
        Color::Cyan => ansi16(6),
        Color::Gray => ansi16(7),
        Color::DarkGray => ansi16(8),
        Color::LightRed => ansi16(9),
        Color::LightGreen => ansi16(10),
        Color::LightYellow => ansi16(11),
        Color::LightBlue => ansi16(12),
        Color::LightMagenta => ansi16(13),
        Color::LightCyan => ansi16(14),
        Color::White => ansi16(15),
        Color::Indexed(i) => format!("{};5;{i}", base + 8),
        Color::Rgb(r, g, b) => format!("{};2;{r};{g};{b}", base + 8),
    }
}

/// How a role's color reads in plain text: `#1e1e2e`, `default` (the terminal's own) or `ansi 12`.
fn describe(c: Color) -> String {
    let index = |i: u8| format!("ansi {i}");
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Reset => "default".into(),
        Color::Indexed(i) => index(i),
        Color::Black => index(0),
        Color::Red => index(1),
        Color::Green => index(2),
        Color::Yellow => index(3),
        Color::Blue => index(4),
        Color::Magenta => index(5),
        Color::Cyan => index(6),
        Color::Gray => index(7),
        Color::DarkGray => index(8),
        Color::LightRed => index(9),
        Color::LightGreen => index(10),
        Color::LightYellow => index(11),
        Color::LightBlue => index(12),
        Color::LightMagenta => index(13),
        Color::LightCyan => index(14),
        Color::White => index(15),
    }
}

/// The text of `platoon themes`: each theme (the built-ins, then the user's own, marked) with its id and name, then
/// its roles. With `depth` (a terminal that shows color) the roles are a line of colored blocks on the theme's own
/// background, the way they read in platoon; without it, plain text with the values. Theme files that were
/// skipped are listed at the end with the reason.
pub fn render_list(depth: Option<crate::colordepth::Depth>, user: &Registry) -> String {
    let mut out = String::new();
    for t in user.all() {
        let tag = if t.id == DEFAULT_ID {
            "  (default)".to_string()
        } else if t.is_user() && !t.name.ends_with("(yours)") {
            "  (yours)".to_string()
        } else if let Some(plugin) = user.plugin_of(&t.id).filter(|p| !t.name.ends_with(&format!("({p})"))) {
            format!("  (plugin: {plugin})")
        } else {
            String::new()
        };
        out.push_str(&format!("{:<22} {}{tag}\n", t.id, t.name));
        match depth {
            Some(d) => {
                let on = |c: Color| d.color(c);
                out.push_str(&format!("  \x1b[{}m  ", sgr(on(t.bg), false)));
                for (_, c) in t.roles().iter().skip(1) {
                    out.push_str(&format!("\x1b[{}m\u{2588}\u{2588} ", sgr(on(*c), true)));
                }
                // The blocks stand on the theme's background; end the line on it, then reset.
                out.push_str("\x1b[0m\n");
            }
            None => {
                let roles: Vec<String> = t.roles().iter().map(|(name, c)| format!("{name} {}", describe(*c))).collect();
                out.push_str(&format!("  {}\n", roles.join("  ")));
            }
        }
    }
    let skipped = user.skipped();
    if skipped.skipped_count() > 0 {
        let n = skipped.skipped_count();
        out.push_str(&format!("\n{n} theme file{} skipped in {}:\n", if n == 1 { "" } else { "s" }, user.dir().display()));
        for reason in skipped.reasons() {
            out.push_str(&format!("  {reason}\n"));
        }
    }
    out
}

// --- color math ------------------------------------------------------------------------------------
// Only `Color::Rgb` has numbers to work with; anything else is the terminal's own and is left as it is.

fn channels(c: Color) -> Option<[f64; 3]> {
    match c {
        Color::Rgb(r, g, b) => Some([f64::from(r), f64::from(g), f64::from(b)]),
        _ => None,
    }
}

fn from_channels(c: [f64; 3]) -> Color {
    Color::Rgb(c[0].round().clamp(0.0, 255.0) as u8, c[1].round().clamp(0.0, 255.0) as u8, c[2].round().clamp(0.0, 255.0) as u8)
}

/// `a` moved `t` (0..1) of the way to `b`, in sRGB.
pub fn mix(a: Color, b: Color, t: f64) -> Color {
    match (channels(a), channels(b)) {
        (Some(a), Some(b)) => from_channels([0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)),
        _ => a,
    }
}

/// WCAG relative luminance, 0 (black) to 1 (white).
pub fn luminance(c: Color) -> f64 {
    let lin = |v: f64| {
        let v = v / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    channels(c).map_or(0.0, |[r, g, b]| 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b))
}

/// WCAG contrast ratio, 1 (identical) to 21 (black on white).
pub fn contrast(a: Color, b: Color) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `color` itself if it already has `min` contrast against `bg`; otherwise the first color on the way to
/// `toward` (normally the theme's text color: lighter on a dark theme, darker on a light one) that does, in 1%
/// steps, so a derived or borrowed color is lifted by as little as it takes and keeps its hue as far as possible.
/// `toward` itself when even that is not enough. Reusable for user themes.
pub fn lift_contrast(color: Color, bg: Color, toward: Color, min: f64) -> Color {
    if contrast(color, bg) >= min || channels(color).is_none() {
        return color;
    }
    (1..=100).map(|i| mix(color, toward, f64::from(i) / 100.0)).find(|c| contrast(*c, bg) >= min).unwrap_or(toward)
}

/// CIE Lab (D65) of an sRGB color, for `delta_e`.
fn lab(c: Color) -> [f64; 3] {
    let lin = |v: f64| {
        let v = v / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    let [r, g, b] = channels(c).map_or([0.0; 3], |c| c.map(lin));
    let (x, y, z) = (
        (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047,
        0.2126 * r + 0.7152 * g + 0.0722 * b,
        (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883,
    );
    let f = |t: f64| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
    [116.0 * f(y) - 16.0, 500.0 * (f(x) - f(y)), 200.0 * (f(y) - f(z))]
}

/// CIE76 color difference: the distance in Lab. About 2 is the smallest difference an eye notices side by
/// side; 10-20 is "similar but different"; 30 and more reads as another color at a glance.
pub fn delta_e(a: Color, b: Color) -> f64 {
    let (a, b) = (lab(a), lab(b));
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `SPECS` themes (the ones kept as theme files have their own tests, in `light_theme_tests.rs`).
    fn fixed() -> Vec<Theme> {
        SPECS.iter().map(Spec::build).collect()
    }

    #[test]
    fn the_registry_is_in_display_order_with_unique_slug_ids_and_the_default_first() {
        let ids: Vec<String> = Theme::all().into_iter().map(|t| t.id).collect();
        assert_eq!(
            ids,
            [
                "catppuccin-mocha",
                "catppuccin-macchiato",
                "catppuccin-frappe",
                "tokyo-night",
                "tokyo-night-storm",
                "dracula",
                "one-dark",
                "nord",
                "gruvbox-dark",
                "gruvbox-material-dark",
                "rose-pine",
                "rose-pine-moon",
                "kanagawa-wave",
                "kanagawa-dragon",
                "night-owl",
                "ayu-mirage",
                "vesper",
                "solarized-dark",
                "github-dark",
                "material-ocean",
                "everforest-dark-hard",
                "nightfox",
                "sonokai",
                "moonfly",
                "melange-dark",
                "flexoki-dark",
                "oxocarbon",
                "poimandres",
                "horizon",
                "synthwave",
                "claude",
                "harbor-night",
                "moss-lantern",
                "ayu-dark",
                "doom-one",
                "embark",
                "everblush",
                "github-dark-dimmed",
                "iceberg-dark",
                "jellybeans",
                "srcery",
                "tomorrow-night",
                "zenbones-dark",
                "catppuccin-latte",
                "dayfox",
                "flexoki-light",
                "github-light",
                "gruvbox-light",
                "one-half-light",
                "rose-pine-dawn",
                "solarized-light",
                "terminal"
            ]
        );
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "ids are unique");
        for id in &ids {
            assert!(!id.is_empty() && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'), "{id} is a slug");
            assert!(!id.starts_with('-') && !id.ends_with('-') && !id.contains("--"), "{id}");
        }
        assert_eq!(Theme::default().id, DEFAULT_ID);
        assert_eq!(Theme::default().name, "Catppuccin Mocha");
        assert_eq!(Theme::all().last().map(|t| t.name.as_str()), Some("Terminal colors"));
        assert!(Theme::all().iter().all(|t| !t.name.is_empty()));
        // The `SPECS` ids are written out and the theme files' are their names' slugs: an accented name gives its slug either way.
        for t in Theme::all().iter().filter(|t| !t.name.is_ascii()) {
            assert_eq!(crate::usertheme::slug(&t.name), t.id, "{}", t.name);
        }
    }

    #[test]
    fn by_id_finds_every_theme_and_nothing_else() {
        for t in Theme::all() {
            assert_eq!(Theme::by_id(&t.id), Some(t.clone()));
        }
        assert_eq!(Theme::by_id("nope"), None);
        assert_eq!(Theme::by_id(""), None);
        assert_eq!(Theme::by_id("Nord"), None, "ids are exact");
    }

    #[test]
    fn the_default_keeps_todays_hand_tuned_colors() {
        let t = Theme::default();
        let expect = [
            (t.bg, 0x1e1e2e),
            (t.surface, 0x313244),
            (t.border, 0x45475a),
            (t.dim, 0x6c7086),
            (t.subtext, 0xa6adc8),
            (t.text, 0xcdd6f4),
            (t.accent, 0xb4befe),
            (t.busy, 0xcba6f7),
            (t.ok, 0xa6e3a1),
            (t.warn, 0xf9e2af),
            (t.err, 0xf38ba8),
        ];
        for (got, hex) in expect {
            assert_eq!(got, rgb(hex));
        }
        assert_eq!(t.on_accent, t.bg);
    }

    #[test]
    fn claude_uses_claude_codes_own_colors() {
        let t = Theme::by_id("claude").unwrap();
        assert_eq!((t.bg, t.text, t.surface, t.dim), (rgb(0x141413), rgb(0xfaf9f5), rgb(0x2a2926), rgb(0x73726c)));
        assert_eq!(
            (t.accent, t.busy, t.ok, t.warn, t.err),
            (
                Color::Rgb(215, 119, 87),
                Color::Rgb(175, 135, 255),
                Color::Rgb(78, 186, 101),
                Color::Rgb(255, 193, 7),
                Color::Rgb(255, 107, 128)
            )
        );
        let a = t.ansi.unwrap();
        let hue = |i: usize| channels(a[i]).unwrap();
        // Recognizable hues where programs expect them, in both the normal and the bright row.
        for i in [1, 9] {
            assert!(hue(i)[0] > hue(i)[1] + 60.0 && hue(i)[0] > hue(i)[2] + 40.0, "red at {i}");
        }
        for i in [2, 10] {
            assert!(hue(i)[1] > hue(i)[0] + 40.0 && hue(i)[1] > hue(i)[2] + 40.0, "green at {i}");
        }
        for i in [3, 11] {
            assert!(hue(i)[0] > 200.0 && hue(i)[1] > 170.0 && hue(i)[2] < 110.0, "yellow at {i}");
        }
        for i in [4, 12] {
            assert!(hue(i)[2] > hue(i)[0] && hue(i)[2] > hue(i)[1], "blue at {i}");
        }
        for i in [5, 13] {
            assert!(hue(i)[2] > hue(i)[1] + 40.0 && hue(i)[0] > hue(i)[1] + 20.0, "magenta at {i}");
        }
        for i in [6, 14] {
            assert!(hue(i)[1] > hue(i)[0] + 30.0 && hue(i)[2] > hue(i)[0] + 30.0, "cyan at {i}");
        }
    }

    #[test]
    fn every_fixed_theme_reads_well_on_its_background() {
        for t in fixed() {
            let c = |x: Color| contrast(x, t.bg);
            assert!(c(t.text) >= MIN_TEXT, "{}: text {:.2}", t.id, c(t.text));
            assert!(c(t.subtext) >= MIN_SUBTEXT, "{}: subtext {:.2}", t.id, c(t.subtext));
            assert!(c(t.dim) >= MIN_DIM, "{}: dim {:.2}", t.id, c(t.dim));
            for (name, col) in [("accent", t.accent), ("busy", t.busy), ("ok", t.ok), ("warn", t.warn), ("err", t.err)] {
                assert!(c(col) >= MIN_ROLE, "{}: {name} {:.2}", t.id, c(col));
            }
            // Text stays ordered: text over subtext over dim.
            assert!(c(t.text) > c(t.subtext) && c(t.subtext) > c(t.dim), "{}", t.id);
        }
    }

    #[test]
    fn surface_and_border_are_visible_but_quiet() {
        for t in fixed() {
            assert!(contrast(t.surface, t.bg) >= 1.08, "{}: surface vs bg {:.2}", t.id, contrast(t.surface, t.bg));
            assert!(contrast(t.border, t.bg) >= 1.25, "{}: border vs bg {:.2}", t.id, contrast(t.border, t.bg));
            // Bars and hover chips draw the border color on a selected row's surface.
            assert!(contrast(t.border, t.surface) >= 1.1, "{}: border vs surface {:.2}", t.id, contrast(t.border, t.surface));
            // Dim text must still read on the selected row.
            assert!(contrast(t.dim, t.surface) >= 2.5, "{}: dim on surface {:.2}", t.id, contrast(t.dim, t.surface));
            assert!(contrast(t.text, t.surface) >= 6.0, "{}: text on surface {:.2}", t.id, contrast(t.text, t.surface));
            assert!(contrast(t.on_accent, t.accent) >= 4.5, "{}: badge text {:.2}", t.id, contrast(t.on_accent, t.accent));
        }
    }

    #[test]
    fn status_colors_and_the_accent_are_pairwise_clearly_distinct() {
        for t in fixed() {
            let set = [("accent", t.accent), ("busy", t.busy), ("ok", t.ok), ("warn", t.warn), ("err", t.err)];
            for (i, (a, ca)) in set.iter().enumerate() {
                for (b, cb) in &set[i + 1..] {
                    let d = delta_e(*ca, *cb);
                    assert!(d >= MIN_DELTA_E, "{}: {a} and {b} are only {d:.1} apart", t.id);
                }
            }
        }
    }

    #[test]
    fn every_fixed_theme_is_dark_and_has_a_full_palette() {
        for t in fixed() {
            assert!(luminance(t.bg) < 0.2, "{}: bg luminance {:.3}", t.id, luminance(t.bg));
            let ansi = t.ansi.unwrap();
            assert!(ansi.iter().all(|c| matches!(c, Color::Rgb(..))), "{}", t.id);
            assert!(t.roles().iter().all(|(_, c)| matches!(c, Color::Rgb(..))), "{}", t.id);
        }
        assert_eq!(fixed().len(), 31, "everything but Terminal colors");
    }

    #[test]
    fn terminal_colors_use_only_the_terminals_own_colors() {
        let t = Theme::by_id("terminal").unwrap();
        assert_eq!((t.bg, t.text), (Color::Reset, Color::Reset));
        assert!(t.ansi.is_none());
        for (name, c) in t.roles() {
            assert!(!matches!(c, Color::Rgb(..)), "{name} must not be a fixed color");
        }
        assert!(!matches!(t.on_accent, Color::Rgb(..)));
        // The pairs the UI draws on top of each other must not collapse into one color.
        assert_ne!(t.surface, t.bg, "the selection band differs from the background");
        assert_ne!(t.dim, t.surface, "dim text on the selected row");
        assert_ne!(t.border, t.surface, "bars on the selected row");
        assert_ne!(t.dim, t.bg);
        assert_ne!(t.border, t.bg);
        assert_ne!(t.subtext, t.dim);
        assert_ne!(t.text, t.surface);
        assert_ne!(t.on_accent, t.accent);
        let status = [t.accent, t.busy, t.ok, t.warn, t.err];
        for (i, a) in status.iter().enumerate() {
            assert!(status[i + 1..].iter().all(|b| a != b), "{a:?} is used twice");
        }
    }

    #[test]
    fn lifting_raises_contrast_just_enough_and_leaves_good_colors_alone() {
        let bg = rgb(0x1e1e2e);
        let text = rgb(0xcdd6f4);
        let good = rgb(0xa6e3a1);
        assert_eq!(lift_contrast(good, bg, text, 4.5), good);
        let faint = rgb(0x45475a);
        let lifted = lift_contrast(faint, bg, text, 4.5);
        assert!(contrast(lifted, bg) >= 4.5);
        assert!(contrast(faint, bg) < 4.5);
        // Just enough: not overshooting by more than a percent step of the way to the text color.
        assert!(contrast(lifted, bg) < 4.5 + 0.5, "{}", contrast(lifted, bg));
        // Unreachable: ends at the target, and a color that isn't a fixed rgb is never touched.
        assert_eq!(lift_contrast(faint, bg, rgb(0x2a2a3a), 12.0), rgb(0x2a2a3a));
        assert_eq!(lift_contrast(Color::DarkGray, bg, text, 12.0), Color::DarkGray);
        assert_eq!(mix(Color::Reset, text, 0.5), Color::Reset);
    }

    #[test]
    fn contrast_and_distance_match_known_values() {
        assert!((contrast(rgb(0xffffff), rgb(0x000000)) - 21.0).abs() < 1e-9);
        assert!((contrast(rgb(0x777777), rgb(0x777777)) - 1.0).abs() < 1e-9);
        assert!((delta_e(rgb(0xff0000), rgb(0xff0000))).abs() < 1e-9);
        assert!((delta_e(rgb(0x000000), rgb(0xffffff)) - 100.0).abs() < 0.5);
    }

    #[test]
    fn the_theme_list_is_plain_text_without_color_and_swatches_with_it() {
        use crate::colordepth::Depth;
        let none = Registry::default();
        let plain = render_list(None, &none);
        assert!(!plain.contains('\x1b'));
        let lines: Vec<&str> = plain.lines().collect();
        assert_eq!(lines.len(), 2 * Theme::all().len(), "a name line and a roles line each");
        assert_eq!(lines[0], "catppuccin-mocha       Catppuccin Mocha  (default)");
        assert!(lines[1].starts_with("  bg #1e1e2e  surface #313244") && lines[1].ends_with("err #f38ba8"), "{}", lines[1]);
        assert!(lines[2].starts_with("catppuccin-macchiato   Catppuccin Macchiato") && !lines[2].contains("default"));
        let last = lines.len() - 1;
        assert!(lines[last - 1].starts_with("terminal") && lines[last - 1].contains("Terminal colors"));
        assert_eq!(
            lines[last],
            "  bg default  surface ansi 0  border ansi 8  dim ansi 8  subtext ansi 7  text default  accent ansi 12  busy ansi 13  ok ansi 10  warn ansi 11  err ansi 9"
        );

        let color = render_list(Some(Depth::TrueColor), &none);
        assert!(
            color.contains("\x1b[48;2;30;30;46m") && color.contains("\x1b[38;2;180;190;254m\u{2588}"),
            "truecolor escapes for a fixed theme"
        );
        assert!(color.contains("\x1b[49m") && color.contains("\x1b[94m"), "the terminal's own colors by name");
        assert!(
            color.lines().all(|l| l.is_empty() || l.starts_with(char::is_alphabetic) || l.ends_with("\x1b[0m")),
            "every swatch line resets"
        );
        let limited = render_list(Some(Depth::Indexed256), &none);
        assert!(!limited.contains(";2;") && limited.contains("48;5;"), "no truecolor escapes on a 256-color terminal");
    }

    #[test]
    fn every_built_in_theme_is_credited_and_nothing_commercial_is() {
        let notices = include_str!("../THIRD_PARTY_NOTICES.md");
        for t in Theme::all() {
            assert!(notices.contains(&t.name), "{} is missing from THIRD_PARTY_NOTICES.md", t.name);
        }
        assert!(notices.contains("iTerm2-Color-Schemes") && notices.contains("Ghostty"));
        assert!(!notices.to_lowercase().contains("monokai"));
    }

    #[test]
    fn the_theme_list_includes_the_users_themes_marked_and_the_skipped_files_with_reasons() {
        use crate::colordepth::Depth;
        let dir = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-theme-list-{}", std::process::id())));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("themes")).unwrap();
        std::fs::write(dir.join("themes/Deep Sea"), "background = #0a1622\nforeground = #dde6ee\n").unwrap();
        std::fs::write(dir.join("themes/Dracula"), "background = #101010\nforeground = #eeeeee\n").unwrap();
        std::fs::write(dir.join("themes/Sunny"), "background = #ffffff\nforeground = #bbbbbb\n").unwrap();
        let reg = Registry::open(&dir);
        let plain = render_list(None, &reg);
        let lines: Vec<&str> = plain.lines().collect();
        let n = Theme::all().len();
        assert_eq!(
            &lines[..2 * n],
            render_list(None, &Registry::default()).lines().collect::<Vec<_>>(),
            "the built-ins come first, exactly as before"
        );
        assert_eq!(lines[2 * n], "user:deep-sea          Deep Sea  (yours)");
        assert!(lines[2 * n + 1].starts_with("  bg #0a1622  surface #"), "{}", lines[2 * n + 1]);
        assert_eq!(lines[2 * n + 2], "user:dracula           Dracula (yours)", "a built-in's name is marked in the name, and not twice");
        assert_eq!(lines[2 * n + 4], "");
        assert!(lines[2 * n + 5].starts_with("1 theme file skipped in ") && lines[2 * n + 5].ends_with("themes:"), "{}", lines[2 * n + 5]);
        assert!(
            lines[2 * n + 6].starts_with(
                "  Sunny: a light theme has to read as it is: the text is hard to read on the background (1.9:1; it needs 7:1)"
            ),
            "{}",
            lines[2 * n + 6]
        );
        assert_eq!(lines.len(), 2 * n + 7);
        let color = render_list(Some(Depth::TrueColor), &reg);
        assert!(color.contains("48;2;10;22;34m"), "the user theme's swatch is on its own background");
        assert!(color.contains("Sunny: a light theme has to read as it is") && !color.contains("skipped in \x1b"));
    }
    #[test]
    fn the_selection_color_keeps_the_themes_text_readable_and_is_clearly_not_the_background() {
        for t in fixed() {
            let sel = t.selection_bg().unwrap_or_else(|| panic!("{} has colors of its own", t.id));
            assert!(contrast(t.text, sel) >= SELECTION_TEXT_CONTRAST, "{}: text on the selection is {:.2}", t.id, contrast(t.text, sel));
            // (Session text that reads worse than 3:1 on the selection is swapped for `text` when drawn.)
            assert!(contrast(t.subtext, sel) >= 2.5, "{}: the muted text is still there on it ({:.2})", t.id, contrast(t.subtext, sel));
            assert!(contrast(sel, t.bg) >= 1.5, "{}: the selection stands out from the background ({:.2})", t.id, contrast(sel, t.bg));
            assert!(
                delta_e(sel, t.bg) >= 15.0,
                "{}: and is a different color, not just a different lightness ({:.1})",
                t.id,
                delta_e(sel, t.bg)
            );
            assert_ne!(sel, t.bg);
        }
    }

    #[test]
    fn the_selection_color_is_still_distinct_and_readable_after_the_256_color_pass() {
        use crate::colordepth::rgb_to_256;
        let xterm = |i: u8| -> Color {
            let cube = [0u8, 95, 135, 175, 215, 255];
            match i {
                16..=231 => {
                    Color::Rgb(cube[usize::from((i - 16) / 36)], cube[usize::from((i - 16) / 6 % 6)], cube[usize::from((i - 16) % 6)])
                }
                _ => Color::Rgb(8 + 10 * (i - 232), 8 + 10 * (i - 232), 8 + 10 * (i - 232)),
            }
        };
        let quantized = |c: Color| match c {
            Color::Rgb(r, g, b) => xterm(rgb_to_256(r, g, b)),
            other => other,
        };
        for t in fixed() {
            let (sel, bg, text) = (quantized(t.selection_bg().unwrap()), quantized(t.bg), quantized(t.text));
            assert_ne!(sel, bg, "{}: the highlight must not vanish into the background", t.id);
            assert!(contrast(text, sel) >= 4.0, "{}: text on the selection in 256 colors is {:.2}", t.id, contrast(text, sel));
        }
    }

    #[test]
    fn terminal_colors_has_no_selection_color_because_it_has_no_colors_to_mix() {
        assert_eq!(Theme::by_id("terminal").unwrap().selection_bg(), None);
    }
}
