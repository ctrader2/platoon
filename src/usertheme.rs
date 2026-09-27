//! The user's own themes: Ghostty-format files in `<config dir>/themes/`, one theme per file, named after the
//! file (`~/.config/platoon/themes/Nord` is the theme "Nord"). Ghostty's bundled themes use the same format, so
//! copying one over just works.
//!
//! A file is turned into a `Theme` the way the built-ins are: the same role derivation, then the same contrast
//! floors, so a sloppy or low-contrast file still gives something readable. What can't be made to work is
//! skipped with a reason that the picker and `platoon themes` show (a light theme, a file that isn't a theme,
//! ...). Nothing in here may panic or hang on a strange file: sizes and counts are capped, only regular files
//! are opened (a named pipe would block), and text that isn't UTF-8 is skipped rather than guessed at.

use crate::theme::{contrast, delta_e, lift_contrast, luminance, mix, Theme, MIN_DELTA_E, MIN_DIM, MIN_ROLE, MIN_SUBTEXT, MIN_TEXT};
use ratatui::style::Color;
use std::{
    collections::HashSet,
    ffi::{OsStr, OsString},
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// A theme file bigger than this is not a theme (real ones are under 1 KiB).
pub const MAX_BYTES: u64 = 64 * 1024;
/// Files read from the folder, in name order. The rest are counted but left alone.
pub const MAX_FILES: usize = 300;
/// Directory entries looked at at all, so a folder with a million files costs what it costs to skip them.
const MAX_ENTRIES: usize = 1000;
/// Longest display name kept (a file name can be 255 bytes of anything).
const MAX_NAME: usize = 40;
/// The id prefix of every user theme, so it can never collide with a built-in.
pub const ID_PREFIX: &str = "user:";
/// Same rule the built-ins are tested against.
const MAX_BG_LUMINANCE: f64 = 0.2;

/// What a theme file says: the two required colors and whichever palette slots it defines.
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    pub background: Color,
    pub foreground: Color,
    pub palette: [Option<Color>; 16],
}

/// `#RRGGBB`, `RRGGBB` or `#RGB`, either case, optionally quoted.
fn hex(s: &str) -> Option<Color> {
    let s = s.trim().trim_matches('"').trim();
    let s = s.strip_prefix('#').unwrap_or(s);
    if !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(s, 16).ok()?;
    match s.len() {
        6 => Some(Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8)),
        3 => {
            let x = |shift: u32| (((n >> shift) & 0xf) * 17) as u8;
            Some(Color::Rgb(x(8), x(4), x(0)))
        }
        _ => None,
    }
}

/// Read a theme file's text. Lines are `key = value`; only `background`, `foreground` and `palette = N=#hex` (N
/// 0..=15) count, everything else (cursor and selection colors, comments, blank lines, lines with no `=`) is
/// ignored. A later line for the same key wins, as in Ghostty. A bad color in a palette slot leaves that slot
/// undefined (it gets filled in later); a missing or bad background or foreground is an error, since a theme
/// without them can't be made.
pub fn parse(text: &str) -> Result<Parsed, String> {
    let mut background: Option<Result<Color, String>> = None;
    let mut foreground: Option<Result<Color, String>> = None;
    let mut palette = [None; 16];
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
        let color = |what: &str| hex(value).ok_or_else(|| format!("the {what} isn't a color ({})", shorten(value)));
        match key.as_str() {
            "background" => background = Some(color("background")),
            "foreground" => foreground = Some(color("foreground")),
            "palette" => {
                if let Some((n, c)) = value.split_once('=') {
                    if let (Ok(n), Some(c)) = (n.trim().parse::<usize>(), hex(c)) {
                        if n < 16 {
                            palette[n] = Some(c);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let background = background.ok_or("no background color")??;
    let foreground = foreground.ok_or("no foreground color")??;
    Ok(Parsed { background, foreground, palette })
}

/// A value quoted in a message: short, and free of anything that could act on a terminal.
fn shorten(s: &str) -> String {
    let s: String = s.chars().filter(|c| !c.is_control()).take(16).collect();
    s
}

const WHITE: Color = Color::Rgb(255, 255, 255);

/// The colors used where a file leaves a role's slots out: the hues of One Dark, which read on any dark background.
const HUES: [u32; 6] = [0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2];
/// What the five roles fall back to when nothing else is available, and the colors tried when two roles collide.
const ROLE_FALLBACK: [u32; 5] = [0x61afef, 0xc678dd, 0x98c379, 0xe5c07b, 0xe06c75];
const SPARE: [u32; 3] = [0x56b6c2, 0xd19a66, 0xf78fb3];

fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// All 16 colors: the file's, and for a slot it leaves out one derived from its partner (the bright color from
/// the normal one and back) or, with neither, from a neutral default. Only used to recolor sessions; the
/// interface picks its roles from the file's own slots first.
fn resolve(p: &Parsed) -> [Color; 16] {
    let (bg, fg) = (p.background, p.foreground);
    let file = &p.palette;
    std::array::from_fn(|i| {
        file[i].unwrap_or_else(|| match i {
            0 => file[8].map_or(mix(bg, fg, 0.15), |c| mix(c, bg, 0.5)),
            8 => file[0].map_or(mix(bg, fg, 0.4), |c| mix(c, fg, 0.35)),
            7 => file[15].map_or(mix(fg, bg, 0.15), |c| mix(c, bg, 0.15)),
            15 => file[7].map_or(fg, |c| mix(c, WHITE, 0.35)),
            1..=6 => file[i + 8].map_or(rgb(HUES[i - 1]), |c| mix(c, bg, 0.2)),
            _ => file[i - 8].map_or(mix(rgb(HUES[i - 9]), WHITE, 0.2), |c| mix(c, WHITE, 0.2)),
        })
    })
}

/// `t` from `from` upward until `ok` says the color of `bg` mixed that far toward `text` is enough.
fn mixed_until(bg: Color, text: Color, from: f64, ok: impl Fn(Color) -> bool) -> Color {
    let mut t = from;
    loop {
        let c = mix(bg, text, t);
        if ok(c) || t >= 1.0 {
            return c;
        }
        t += 0.01;
    }
}

/// The theme a file describes, or why there isn't one. Same roles as the built-ins: bg and text from the file,
/// surface, border, dim and subtext mixed between them, accent / busy / ok / warn / err from palette slots
/// 12 / 13 / 10 / 11 / 9 (falling back to 4 / 5 / 2 / 3 / 1). Then the contrast floors: the roles and the
/// muted text are lifted toward the text color until they read, surface and border are kept apart from the
/// background and from each other, and the five colors that carry meaning are kept clearly distinct.
pub fn theme_from(id: String, name: String, p: &Parsed) -> Result<Theme, String> {
    let (bg, fg) = (p.background, p.foreground);
    if luminance(bg) >= MAX_BG_LUMINANCE {
        return Err("light themes aren't supported".into());
    }
    let pal = resolve(p);
    let slot = |hi: usize, lo: usize| p.palette[hi].or(p.palette[lo]).unwrap_or(pal[hi]);
    let picks = [slot(12, 4), slot(13, 5), slot(10, 2), slot(11, 3), slot(9, 1)];

    // Everything is mixed toward the text color; the text itself is lifted last, on the surface it will sit on.
    let toward = lift_contrast(fg, bg, WHITE, MIN_TEXT);
    let surface = mixed_until(bg, toward, 0.09, |c| contrast(c, bg) >= 1.08);
    let text = lift_contrast(toward, surface, WHITE, 6.0);
    let border = mixed_until(bg, toward, 0.20, |c| contrast(c, bg) >= 1.25 && contrast(c, surface) >= 1.1);
    let dim = lift_contrast(lift_contrast(mix(bg, toward, 0.46), bg, toward, MIN_DIM), surface, toward, 2.5);
    let subtext = lift_contrast(mix(bg, toward, 0.72), bg, toward, MIN_SUBTEXT);
    let roles = distinct(picks, bg, text);
    let [accent, busy, ok, warn, err] = roles;
    Ok(Theme {
        id,
        name,
        bg,
        surface,
        border,
        dim,
        subtext,
        text,
        accent,
        busy,
        ok,
        warn,
        err,
        on_accent: bg,
        ansi: Some(pal),
    })
}

/// The five role colors, each lifted to read on `bg`, and no two of them alike: one that is too close to an
/// earlier one is replaced by the neutral default for its role, or failing that a spare hue. The ones that mean
/// something (err, ok, warn) choose first, then busy, and the accent gives way last.
fn distinct(picks: [Color; 5], bg: Color, text: Color) -> [Color; 5] {
    let lifted = |c: Color| lift_contrast(c, bg, text, MIN_ROLE);
    let mut out = picks.map(lifted);
    let mut taken: Vec<Color> = Vec::new();
    for idx in [4, 2, 3, 1, 0] {
        let own = out[idx];
        let fallbacks = std::iter::once(ROLE_FALLBACK[idx]).chain(ROLE_FALLBACK).chain(SPARE).map(|h| lifted(rgb(h)));
        let chosen = std::iter::once(own).chain(fallbacks).find(|c| taken.iter().all(|t| delta_e(*t, *c) >= MIN_DELTA_E)).unwrap_or(own);
        out[idx] = chosen;
        taken.push(chosen);
    }
    out
}

/// A file name as a theme's display name: control characters dropped (they could act on the terminal that shows
/// it) and so are the invisible ones that reorder text, runs of space folded, cut at `MAX_NAME` characters.
pub fn display_name(file: &OsStr) -> String {
    let odd = |c: char| c.is_control() || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}');
    let name: String = file.to_string_lossy().chars().map(|c| if odd(c) { ' ' } else { c }).collect();
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    name.chars().take(MAX_NAME).collect::<String>().trim_end().to_string()
}

/// Lowercase ASCII letters and digits, anything else a single dash: `Nord Light` is `nord-light`. A name with no
/// ASCII at all gets a short hash, so the id is still stable from run to run.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        // FNV-1a: stable across runs and platforms, unlike the standard library's hasher.
        let hash = name.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100000001b3));
        return format!("theme-{:08x}", hash as u32);
    }
    out
}

/// A file that didn't become a theme, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    pub name: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scan {
    /// In file-name order.
    pub themes: Vec<Theme>,
    pub skipped: Vec<Skipped>,
    /// Files beyond `MAX_FILES`, not read.
    pub over_limit: usize,
}

impl Scan {
    /// How many files didn't become a theme, counting the ones never read.
    pub fn skipped_count(&self) -> usize {
        self.skipped.len() + self.over_limit
    }

    /// "Nord Light: light themes aren't supported", one per file, plus a line for files never read.
    pub fn reasons(&self) -> Vec<String> {
        let mut out: Vec<String> = self.skipped.iter().map(|s| format!("{}: {}", s.name, s.reason)).collect();
        if self.over_limit > 0 {
            out.push(format!("{} more files not read (the limit is {MAX_FILES})", self.over_limit));
        }
        out
    }
}

/// Regular files of `dir` (not hidden, not folders, not pipes or devices), by file name, and how many entries
/// were looked at. A symlink to a file counts as a file.
fn candidates(dir: &Path) -> Vec<(OsString, PathBuf, u64, Option<SystemTime>)> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<_> = entries
        .take(MAX_ENTRIES)
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| {
            let meta = fs::metadata(e.path()).ok().filter(|m| m.is_file())?;
            Some((e.file_name(), e.path(), meta.len(), meta.modified().ok()))
        })
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

pub fn read_theme_text(path: &Path, len: u64) -> Result<String, String> {
    if len > MAX_BYTES {
        return Err(format!("the file is bigger than {} KiB", MAX_BYTES / 1024));
    }
    let mut bytes = Vec::new();
    // Capped again here: the file may have grown since it was measured.
    fs::File::open(path).and_then(|f| f.take(MAX_BYTES + 1).read_to_end(&mut bytes)).map_err(|_| "the file couldn't be read".to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!("the file is bigger than {} KiB", MAX_BYTES / 1024));
    }
    let text = String::from_utf8(bytes).map_err(|_| "the file isn't text".to_string())?;
    Ok(text.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(text))
}

/// Every theme file in `dir`. A folder that doesn't exist is no themes and no complaint.
pub fn scan(dir: &Path) -> Scan {
    let files = candidates(dir);
    let mut out = Scan { over_limit: files.len().saturating_sub(MAX_FILES), ..Scan::default() };
    let builtin: Vec<String> = Theme::all().iter().map(|t| t.name.to_lowercase()).collect();
    let mut names: Vec<(String, String)> = Vec::new(); // (lowercase, as shown), for the duplicates
    let mut ids: HashSet<String> = HashSet::new();
    for (file, path, len, _) in files.into_iter().take(MAX_FILES) {
        let name = display_name(&file);
        let skip = |out: &mut Scan, name: &str, reason: String| out.skipped.push(Skipped { name: if name.is_empty() { "(unnamed)".into() } else { name.to_string() }, reason });
        if name.is_empty() {
            skip(&mut out, "", "the file name has nothing to show".into());
            continue;
        }
        if let Some((_, first)) = names.iter().find(|(lower, _)| *lower == name.to_lowercase()) {
            skip(&mut out, &name, format!("same name as {first}"));
            continue;
        }
        let parsed = read_theme_text(&path, len).and_then(|text| parse(&text));
        let id = {
            let base = format!("{ID_PREFIX}{}", slug(&name));
            (1..).map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") }).find(|c| !ids.contains(c)).unwrap_or(base)
        };
        // A name that is a built-in's too is marked, so the two can be told apart wherever a name is all there is.
        let shown = if builtin.contains(&name.to_lowercase()) { format!("{name} (yours)") } else { name.clone() };
        match parsed.and_then(|p| theme_from(id.clone(), shown, &p)) {
            Ok(theme) => {
                ids.insert(id);
                names.push((name.to_lowercase(), name));
                out.themes.push(theme);
            }
            Err(reason) => skip(&mut out, &name, reason),
        }
    }
    out
}

type Print = Vec<(OsString, u64, Option<SystemTime>)>;

type PluginPrint = Option<(u64, Option<SystemTime>)>;

/// The user's themes as of the last look at the folder, and what is needed to see that it changed. Also the themes
/// of installed plugins, when it was opened with a plugins folder.
#[derive(Default)]
pub struct Registry {
    dir: PathBuf,
    print: Option<Print>,
    scan: Scan,
    plugins: Option<crate::plugins::Store>,
    plugin_print: PluginPrint,
    plugin_themes: Vec<crate::plugins::PluginTheme>,
}

impl Registry {
    /// The themes in `<config>/themes`, read now. (`Registry::default()` has none and never looks anywhere.)
    #[cfg(test)]
    pub fn open(config: &Path) -> Self {
        let mut r = Self { dir: config.join("themes"), ..Self::default() };
        r.reload();
        r
    }

    /// The user's themes and those of the plugins installed in `plugins` (`<data>/plugins`).
    pub fn open_with_plugins(config: &Path, plugins: &Path) -> Self {
        let mut r = Self { dir: config.join("themes"), plugins: Some(crate::plugins::Store::new(plugins.to_path_buf())), ..Self::default() };
        r.reload();
        r
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn fingerprint(&self) -> Option<Print> {
        if self.dir.as_os_str().is_empty() {
            return None;
        }
        if !self.dir.is_dir() {
            return None;
        }
        Some(candidates(&self.dir).into_iter().take(MAX_FILES + 1).map(|(name, _, len, at)| (name, len, at)).collect())
    }

    fn reload(&mut self) {
        self.print = self.fingerprint();
        self.scan = scan(&self.dir);
        self.reload_plugins();
    }

    fn reload_plugins(&mut self) {
        let Some(store) = &self.plugins else { return };
        self.plugin_print = crate::plugins::fingerprint(store);
        let taken: Vec<String> = Theme::all().into_iter().map(|t| t.name).chain(self.scan.themes.iter().map(|t| t.name.clone())).collect();
        self.plugin_themes = crate::plugins::themes(store, &taken);
    }

    /// Look at the folder again: a listing of names, sizes and times, and the files are only read again if that
    /// changed. True when they were. Bounded by `MAX_ENTRIES`, so cheap enough for every settings refresh. The
    /// plugins' themes are read again when what is installed changed.
    pub fn refresh(&mut self) -> bool {
        let now = self.fingerprint();
        if now != self.print {
            self.reload();
            return true;
        }
        match &self.plugins {
            Some(store) if crate::plugins::fingerprint(store) != self.plugin_print => {
                self.reload_plugins();
                true
            }
            _ => false,
        }
    }

    /// The user's themes, in file-name order.
    pub fn user(&self) -> &[Theme] {
        &self.scan.themes
    }

    pub fn skipped(&self) -> &Scan {
        &self.scan
    }

    /// Built-ins in their order, then the user's, then the plugins'.
    pub fn all(&self) -> Vec<Theme> {
        Theme::all().into_iter().chain(self.user().iter().cloned()).chain(self.plugin_themes.iter().map(|p| p.theme.clone())).collect()
    }

    pub fn by_id(&self, id: &str) -> Option<Theme> {
        Theme::by_id(id)
            .or_else(|| self.scan.themes.iter().find(|t| t.id == id).cloned())
            .or_else(|| self.plugin_themes.iter().find(|p| p.theme.id == id).map(|p| p.theme.clone()))
    }

    /// The name of the plugin a theme came from (None for a built-in or one of the user's).
    pub fn plugin_of(&self, id: &str) -> Option<&str> {
        self.plugin_themes.iter().find(|p| p.theme.id == id).map(|p| p.plugin.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(hex: u32) -> Color {
        rgb(hex)
    }

    const NORD: &str = "palette = 0=#3b4252\npalette = 1=#bf616a\npalette = 2=#a3be8c\npalette = 3=#ebcb8b\npalette = 4=#81a1c1\npalette = 5=#b48ead\npalette = 6=#88c0d0\npalette = 7=#e5e9f0\npalette = 8=#596377\npalette = 9=#bf616a\npalette = 10=#a3be8c\npalette = 11=#ebcb8b\npalette = 12=#81a1c1\npalette = 13=#b48ead\npalette = 14=#8fbcbb\npalette = 15=#eceff4\nbackground = #2e3440\nforeground = #d8dee9\ncursor-color = #eceff4\ncursor-text = #282828\nselection-background = #eceff4\nselection-foreground = #4c566a\n";

    fn tmp(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-usertheme-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("themes")).unwrap();
        d
    }

    fn put(config: &Path, name: &str, text: impl AsRef<[u8]>) {
        fs::write(config.join("themes").join(name), text).unwrap();
    }

    #[test]
    fn a_real_ghostty_theme_parses_to_its_palette() {
        let p = parse(NORD).unwrap();
        assert_eq!((p.background, p.foreground), (c(0x2e3440), c(0xd8dee9)));
        assert_eq!(p.palette[0], Some(c(0x3b4252)));
        assert_eq!(p.palette[12], Some(c(0x81a1c1)));
        assert_eq!(p.palette[15], Some(c(0xeceff4)));
        assert!(p.palette.iter().all(Option::is_some));
    }

    #[test]
    fn the_format_is_read_generously() {
        let text = "# a comment\r\n\r\n  background   =   2E3440  \r\nForeground=\"#D8DEE9\"\r\npalette=1=#BF616A\r\n palette = 04 = #81a1c1\r\ncursor-color = #fff\r\nnot a setting\r\n# palette = 2=#00ff00\r\nfont-size = 12\r\n";
        let p = parse(text).unwrap();
        assert_eq!((p.background, p.foreground), (c(0x2e3440), c(0xd8dee9)), "no #, upper case, quotes, spaces, CRLF");
        assert_eq!(p.palette[1], Some(c(0xbf616a)));
        assert_eq!(p.palette[4], Some(c(0x81a1c1)), "N may have a leading zero");
        assert_eq!(p.palette[2], None, "a commented-out line is not there");
        assert_eq!(p.palette.iter().flatten().count(), 2);
        // Short hex, and the last line for a key wins.
        let p = parse("background = #123\nbackground = #000000\nforeground = fff").unwrap();
        assert_eq!((p.background, p.foreground), (c(0), c(0xffffff)));
        assert_eq!(hex("#123"), Some(c(0x112233)));
    }

    #[test]
    fn missing_or_bad_required_colors_are_errors_with_a_reason() {
        assert_eq!(parse("").unwrap_err(), "no background color");
        assert_eq!(parse("background = #000000").unwrap_err(), "no foreground color");
        assert_eq!(parse("foreground = #ffffff").unwrap_err(), "no background color");
        assert!(parse("background = #zzzzzz\nforeground = #fff").unwrap_err().contains("the background isn't a color"));
        assert!(parse("background = #12345\nforeground = #fff").is_err(), "five digits");
        assert!(parse("background = #1234567\nforeground = #fff").is_err(), "seven digits");
        assert!(parse("background = red\nforeground = #fff").is_err(), "names are not supported");
        // A later bad line replaces an earlier good one, like any later line.
        assert!(parse("background = #000\nbackground = nope\nforeground = #fff").is_err());
        // A control character in a bad value never reaches the message.
        assert!(!parse("background = \x1b[31mred\nforeground=#fff").unwrap_err().contains('\x1b'));
    }

    #[test]
    fn palette_lines_that_make_no_sense_are_ignored() {
        let p = parse("background=#000\nforeground=#fff\npalette = 16=#ffffff\npalette = -1=#ffffff\npalette = x=#ffffff\npalette = 3=nothex\npalette = 5\npalette = 7=#aabbcc").unwrap();
        assert_eq!(p.palette.iter().flatten().count(), 1);
        assert_eq!(p.palette[7], Some(c(0xaabbcc)));
    }

    #[test]
    fn garbage_never_panics() {
        for text in ["\0\0\0", "===", "=", "palette", "palette=", "palette==", "background=", &"x".repeat(100_000), "\u{feff}", "é=é\nñ", "palette = 99999999999999999999=#fff"] {
            let _ = parse(text);
        }
        for name in ["", " ", "\u{202e}", "é", "a\nb", &"n".repeat(300)] {
            let _ = slug(name);
            let _ = display_name(OsStr::new(name));
        }
    }

    #[test]
    fn a_theme_gets_the_built_ins_roles_from_slots_12_13_10_11_9() {
        let t = theme_from("user:nord".into(), "Nord".into(), &parse(NORD).unwrap()).unwrap();
        assert_eq!((t.id.as_str(), t.name.as_str()), ("user:nord", "Nord"));
        assert_eq!((t.bg, t.on_accent), (c(0x2e3440), c(0x2e3440)));
        // Nord's own picks are the palette's 12/13/10/11 (the built-in tunes accent and err by hand, a file can't).
        assert_eq!((t.accent, t.ok, t.warn), (c(0x81a1c1), c(0xa3be8c), c(0xebcb8b)));
        assert_ne!(t.busy, t.accent);
        assert_eq!(t.ansi.unwrap()[4], c(0x81a1c1), "sessions are recolored with the file's 16 colors");
        assert_eq!(t.ansi.unwrap()[15], c(0xeceff4));
    }

    #[test]
    fn slots_fall_back_to_the_normal_colors_and_a_partial_file_still_loads() {
        // Only the normal colors: the roles use 4/5/2/3/1, and the bright slots are derived.
        let text = "background=#1a1b26\nforeground=#c0caf5\npalette=1=#f7768e\npalette=2=#9ece6a\npalette=3=#e0af68\npalette=4=#7aa2f7\npalette=5=#bb9af7";
        let t = theme_from("user:x".into(), "X".into(), &parse(text).unwrap()).unwrap();
        assert_eq!((t.accent, t.busy, t.ok, t.warn, t.err), (c(0x7aa2f7), c(0xbb9af7), c(0x9ece6a), c(0xe0af68), c(0xf7768e)));
        let a = t.ansi.unwrap();
        assert_eq!((a[1], a[4]), (c(0xf7768e), c(0x7aa2f7)), "the file's own slots are kept");
        assert!(luminance(a[12]) > luminance(a[4]), "a missing bright color is the normal one made lighter");
        // Only the two required colors: everything else comes from neutral defaults, and it is a working theme.
        let bare = theme_from("user:bare".into(), "Bare".into(), &parse("background=#101010\nforeground=#eeeeee").unwrap()).unwrap();
        assert!(bare.ansi.unwrap().iter().all(|c| matches!(c, Color::Rgb(..))));
        for (name, col) in [("accent", bare.accent), ("busy", bare.busy), ("ok", bare.ok), ("warn", bare.warn), ("err", bare.err)] {
            assert!(contrast(col, bare.bg) >= MIN_ROLE, "{name}");
        }
        // A palette that only has the bright slots.
        let bright = "background=#101010\nforeground=#eeeeee\npalette=9=#ff5555\npalette=10=#55ff55\npalette=12=#5555ff";
        let t = theme_from("user:b".into(), "B".into(), &parse(bright).unwrap()).unwrap();
        assert_eq!(t.ok, c(0x55ff55));
        assert!(luminance(t.ansi.unwrap()[2]) < luminance(c(0x55ff55)), "the normal green is derived, darker");
    }

    /// Everything the built-ins are held to, for one derived theme. A floor is only demanded where white on that
    /// background could meet it: on a dark but not very dark one (a saturated blue) nothing can read at 7:1.
    fn assert_readable(t: &Theme) {
        let reach = contrast(WHITE, t.bg) - 0.01;
        let on = |x: Color| contrast(x, t.bg);
        assert!(on(t.text) >= MIN_TEXT.min(reach), "{}: text {:.2}", t.name, on(t.text));
        assert!(on(t.subtext) >= MIN_SUBTEXT.min(reach), "{}: subtext {:.2}", t.name, on(t.subtext));
        assert!(on(t.dim) >= MIN_DIM.min(reach), "{}: dim {:.2}", t.name, on(t.dim));
        for (n, col) in [("accent", t.accent), ("busy", t.busy), ("ok", t.ok), ("warn", t.warn), ("err", t.err)] {
            assert!(on(col) >= MIN_ROLE.min(reach), "{}: {n} {:.2}", t.name, on(col));
        }
        assert!(on(t.text) >= on(t.subtext) && on(t.subtext) >= on(t.dim), "{}: text stays ordered", t.name);
        assert!(contrast(t.surface, t.bg) >= 1.08, "{}: surface vs bg", t.name);
        assert!(contrast(t.border, t.bg) >= 1.25, "{}: border vs bg", t.name);
        assert!(contrast(t.border, t.surface) >= 1.1, "{}: border vs surface", t.name);
        if luminance(t.bg) < 0.05 {
            assert!(contrast(t.dim, t.surface) >= 2.5, "{}: dim on surface", t.name);
            assert!(contrast(t.text, t.surface) >= 6.0, "{}: text on surface", t.name);
        }
        assert!(contrast(t.on_accent, t.accent) >= 4.5f64.min(reach), "{}: badge text", t.name);
        // (Just under the light limit everything must be near white to read, which leaves no colors to tell apart.)
        let set = [t.accent, t.busy, t.ok, t.warn, t.err];
        for (i, a) in set.iter().enumerate().filter(|_| reach >= 5.0) {
            for b in &set[i + 1..] {
                assert!(delta_e(*a, *b) >= MIN_DELTA_E, "{}: {a:?} and {b:?} are alike", t.name);
            }
        }
    }

    #[test]
    fn an_ugly_file_is_made_readable() {
        // Dark grey text on a near-black background, every palette color the same murky grey, black-on-black
        // extremes: the worst a sloppy file can do.
        let mut ugly = String::from("background = #0a0a0a\nforeground = #1c1c1c\n");
        for n in 0..16 {
            ugly.push_str(&format!("palette = {n}=#161616\n"));
        }
        let t = theme_from("user:ugly".into(), "Ugly".into(), &parse(&ugly).unwrap()).unwrap();
        assert_readable(&t);
        assert_ne!(t.err, t.ok, "the statuses were all one color and are told apart now");
        // A near-white-on-black theme whose colors are all one bright hue, and an all-black one.
        for (bg, fg, col) in [("#000000", "#ffffff", "#ff0000"), ("#000000", "#000000", "#000000"), ("#05050a", "#101018", "#20202a"), ("#111111", "#aaaaaa", "#222222")] {
            let mut text = format!("background = {bg}\nforeground = {fg}\n");
            for n in 0..16 {
                text.push_str(&format!("palette = {n}={col}\n"));
            }
            let t = theme_from("user:t".into(), format!("T {bg} {fg} {col}"), &parse(&text).unwrap()).unwrap();
            assert_readable(&t);
        }
        // A palette that is fine gets only what it needs: nothing changes for a good color.
        let good = theme_from("user:g".into(), "G".into(), &parse(NORD).unwrap()).unwrap();
        assert_readable(&good);
        assert_eq!(good.ok, c(0xa3be8c), "a color that reads and stands apart is left alone");
    }

    #[test]
    fn every_bundled_ghostty_dark_theme_becomes_a_readable_theme_when_they_are_installed() {
        // Not part of the shipped behavior, only a wide check when the machine has Ghostty (skipped otherwise).
        let dir = Path::new("/usr/share/ghostty/themes");
        let Ok(entries) = fs::read_dir(dir) else { return };
        let (mut loaded, mut light) = (0, 0);
        for e in entries.flatten().take(500) {
            let Ok(text) = fs::read_to_string(e.path()) else { continue };
            let Ok(p) = parse(&text) else { continue };
            match theme_from("user:t".into(), e.file_name().to_string_lossy().into_owned(), &p) {
                Ok(t) => {
                    assert_readable(&t);
                    loaded += 1;
                }
                Err(_) => light += 1,
            }
        }
        assert!(loaded > 0 || light == 0);
    }

    #[test]
    fn light_themes_are_refused_with_a_reason() {
        let light = "background = #ffffff\nforeground = #000000";
        assert_eq!(theme_from("user:l".into(), "L".into(), &parse(light).unwrap()).unwrap_err(), "light themes aren't supported");
        // Right at the threshold the built-ins use.
        assert!(theme_from("u".into(), "n".into(), &parse("background=#707070\nforeground=#fff").unwrap()).is_ok(), "luminance {:.3}", luminance(c(0x707070)));
        assert!(theme_from("u".into(), "n".into(), &parse("background=#808080\nforeground=#fff").unwrap()).is_err(), "luminance {:.3}", luminance(c(0x808080)));
    }

    #[test]
    fn ids_are_slugs_stable_and_never_empty() {
        assert_eq!(slug("Nord"), "nord");
        assert_eq!(slug("Nord Light"), "nord-light");
        assert_eq!(slug("  --Catppuccin_Mocha!! "), "catppuccin-mocha");
        assert_eq!(slug("Rosé Pine"), "ros-pine");
        assert_eq!(slug("a.b.c"), "a-b-c");
        let cjk = slug("夜の海");
        assert!(cjk.starts_with("theme-") && cjk.len() == 14, "{cjk}");
        assert_eq!(cjk, slug("夜の海"), "the same name gives the same id every run");
        assert_ne!(cjk, slug("夜の空"));
        for name in ["Nord", "", "x y", "-", "..", "é"] {
            let s = slug(name);
            assert!(!s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'), "{s}");
            assert!(!s.starts_with('-') && !s.ends_with('-'));
        }
    }

    #[test]
    fn display_names_are_safe_to_print() {
        assert_eq!(display_name(OsStr::new("Nord")), "Nord");
        assert_eq!(display_name(OsStr::new("Night\x1b[31m Owl\nX")), "Night [31m Owl X");
        assert_eq!(display_name(OsStr::new("  spaced   out ")), "spaced out");
        assert_eq!(display_name(OsStr::new(&"n".repeat(300))).chars().count(), MAX_NAME);
        assert_eq!(display_name(OsStr::new("\x07\x08")), "");
        assert_eq!(display_name(OsStr::new("a\u{202e}b\u{200b}c")), "a b c", "text-reordering characters are dropped too");
    }

    #[test]
    fn a_missing_folder_is_no_themes_and_creates_nothing() {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-usertheme-none-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        let mut r = Registry::open(&d);
        assert!(r.user().is_empty() && r.skipped().skipped_count() == 0);
        assert!(!r.refresh());
        assert!(!d.exists(), "looking never creates the folder");
        assert!(Registry::default().user().is_empty());
        let mut none = Registry::default();
        assert!(!none.refresh());
        assert_eq!(none.all().len(), Theme::all().len());
    }

    #[test]
    fn the_folder_is_read_in_name_order_with_everything_odd_skipped_and_explained() {
        let d = tmp("scan");
        put(&d, "Nord", NORD);
        put(&d, "Zeta", "background = #000000\nforeground = #eeeeee\n");
        put(&d, "Alpha", "background = #10141c\nforeground = #eeeeee\n");
        put(&d, "Nord Light", "background = #eceff4\nforeground = #2e3440\n");
        put(&d, "Empty", "");
        put(&d, "Binary", [0u8, 159, 146, 150, 0xff, 0xfe]);
        put(&d, "Huge", vec![b'#'; (MAX_BYTES + 1) as usize]);
        put(&d, "No Foreground", "background = #000000\n");
        put(&d, "Bad Color", "background = #00000g\nforeground = #ffffff\n");
        put(&d, ".hidden", NORD);
        fs::create_dir(d.join("themes/Folder")).unwrap();
        put(&d, "Folder/inside", NORD);
        let scan = scan(&d.join("themes"));
        let names: Vec<&str> = scan.themes.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "Nord (yours)", "Zeta"], "name order; a name a built-in has is marked");
        let ids: Vec<&str> = scan.themes.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["user:alpha", "user:nord", "user:zeta"]);
        let reasons = scan.reasons();
        let has = |name: &str, why: &str| reasons.iter().any(|r| r.starts_with(&format!("{name}: ")) && r.contains(why));
        assert!(has("Nord Light", "light themes aren't supported"), "{reasons:?}");
        assert!(has("Empty", "no background"), "{reasons:?}");
        assert!(has("Binary", "isn't text"), "{reasons:?}");
        assert!(has("Huge", "bigger than 64 KiB"), "{reasons:?}");
        assert!(has("No Foreground", "no foreground"), "{reasons:?}");
        assert!(has("Bad Color", "isn't a color"), "{reasons:?}");
        assert_eq!(scan.skipped_count(), 6, "the hidden file and the folder are not files that failed: {reasons:?}");
    }

    #[test]
    fn duplicate_names_keep_the_first_and_slug_collisions_get_a_number() {
        let d = tmp("dupes");
        put(&d, "Night", "background = #000000\nforeground = #eeeeee\n");
        put(&d, "night", "background = #101010\nforeground = #eeeeee\n");
        put(&d, "NIGHT", "background = #111111\nforeground = #eeeeee\n");
        put(&d, "Night!", "background = #121212\nforeground = #eeeeee\n");
        put(&d, "Night?", "background = #131313\nforeground = #eeeeee\n");
        let scan = scan(&d.join("themes"));
        let shown: Vec<(&str, &str)> = scan.themes.iter().map(|t| (t.id.as_str(), t.name.as_str())).collect();
        // Bytewise: NIGHT, Night, Night!, Night?, night. The first of a case-insensitive name wins.
        assert_eq!(shown, [("user:night", "NIGHT"), ("user:night-2", "Night!"), ("user:night-3", "Night?")]);
        assert_eq!(scan.skipped.len(), 2);
        assert!(scan.skipped.iter().all(|s| s.reason == "same name as NIGHT"), "{:?}", scan.skipped);
    }

    #[test]
    fn weird_file_names_load_or_are_skipped_without_trouble() {
        let d = tmp("weird");
        let ok = "background = #000000\nforeground = #eeeeee\n";
        for name in ["with space", "ünïcode", "夜の海", "dots.and.more", "-dash", "A\u{1b}[2Jb", "x".repeat(200).as_str(), "trailing.", "a\tb"] {
            put(&d, name, ok);
        }
        let scan = scan(&d.join("themes"));
        assert_eq!(scan.themes.len(), 9, "{:?}", scan.reasons());
        for t in &scan.themes {
            assert!(t.id.starts_with("user:") && !t.name.chars().any(char::is_control), "{:?} {:?}", t.id, t.name);
            assert!(t.name.chars().count() <= MAX_NAME);
        }
        let mut ids: Vec<&String> = scan.themes.iter().map(|t| &t.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 9, "every id is different");
    }

    #[test]
    fn the_number_of_files_read_is_capped_and_the_rest_are_counted() {
        let d = tmp("cap");
        for n in 0..(MAX_FILES + 25) {
            put(&d, &format!("t{n:04}"), format!("background = #0000{:02x}\nforeground = #eeeeee\n", n % 200));
        }
        let scan = scan(&d.join("themes"));
        assert_eq!(scan.themes.len(), MAX_FILES);
        assert_eq!(scan.over_limit, 25);
        assert!(scan.reasons().last().unwrap().contains("25 more files not read"));
        assert_eq!(scan.skipped_count(), 25);
    }

    #[test]
    fn only_regular_files_are_opened_so_a_pipe_cannot_hang_the_scan() {
        let d = tmp("pipe");
        let fifo = std::ffi::CString::new(d.join("themes/Pipe").to_string_lossy().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        put(&d, "Real", "background = #000000\nforeground = #eeeeee\n");
        std::os::unix::fs::symlink(d.join("themes/Real"), d.join("themes/Linked")).unwrap();
        std::os::unix::fs::symlink(d.join("themes/Gone"), d.join("themes/Dangling")).unwrap();
        let scan = scan(&d.join("themes"));
        let names: Vec<&str> = scan.themes.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Linked", "Real"], "a link to a file is a file; a pipe and a dead link are nothing");
    }

    #[test]
    fn the_registry_orders_built_ins_first_finds_by_id_and_notices_edits() {
        let d = tmp("registry");
        put(&d, "Mine", "background = #000000\nforeground = #eeeeee\n");
        let mut r = Registry::open(&d);
        let ids: Vec<String> = r.all().into_iter().map(|t| t.id).collect();
        assert_eq!(ids.len(), Theme::all().len() + 1);
        assert_eq!(ids.last().unwrap(), "user:mine");
        assert_eq!(ids[0], "catppuccin-mocha");
        assert_eq!(r.by_id("user:mine").unwrap().name, "Mine");
        assert_eq!(r.by_id("nord").unwrap().name, "Nord");
        assert!(r.by_id("user:other").is_none() && r.by_id("").is_none());
        assert!(!r.refresh(), "nothing changed");
        put(&d, "Other", "background = #000000\nforeground = #eeeeee\n");
        assert!(r.refresh());
        assert_eq!(r.user().len(), 2);
        // An edit in place, even one that keeps the size, is seen through the modification time.
        std::thread::sleep(std::time::Duration::from_millis(20));
        put(&d, "Other", "background = #000000\nforeground = #111111\n");
        put(&d, "Other", "background = #0a0a0a\nforeground = #eeeeee\n");
        assert!(r.refresh());
        assert_eq!(r.by_id("user:other").unwrap().bg, c(0x0a0a0a));
        fs::remove_file(d.join("themes/Mine")).unwrap();
        assert!(r.refresh());
        assert!(r.by_id("user:mine").is_none());
        assert!(!r.refresh());
    }

    #[test]
    fn ids_and_colors_are_the_same_on_every_scan_of_the_same_folder() {
        let d = tmp("stable");
        for n in ["Beta", "Alpha", "alpha", "Gamma Ray", "夜の海"] {
            put(&d, n, NORD);
        }
        let (a, b) = (scan(&d.join("themes")), scan(&d.join("themes")));
        assert_eq!(a, b);
        let ids: Vec<&str> = a.themes.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids.len(), 4);
        assert!(ids.contains(&"user:alpha") && ids.contains(&"user:beta") && ids.contains(&"user:gamma-ray"));
        // Adding an unrelated file leaves the other ids as they were.
        put(&d, "Zed", NORD);
        let c = scan(&d.join("themes"));
        for t in &a.themes {
            assert!(c.themes.iter().any(|x| x.id == t.id && x == t), "{} changed", t.id);
        }
    }

    #[test]
    fn a_file_of_exactly_the_size_limit_still_loads_and_one_byte_more_does_not() {
        let d = tmp("limit");
        let mut text = String::from("background = #000000\nforeground = #eeeeee\n# ");
        text.push_str(&"x".repeat(MAX_BYTES as usize - text.len()));
        assert_eq!(text.len() as u64, MAX_BYTES);
        put(&d, "Exact", &text);
        put(&d, "Over", format!("{text}x"));
        let scan = scan(&d.join("themes"));
        assert_eq!(scan.themes.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Exact"]);
        assert_eq!(scan.skipped.len(), 1);
        assert!(scan.skipped[0].reason.contains("bigger than 64 KiB"));
    }
}
