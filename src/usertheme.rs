//! The user's own themes: Ghostty-format files in `<config dir>/themes/`, one theme per file, named after the
//! file (`~/.config/platoon/themes/Nord` is the theme "Nord"). Ghostty's bundled themes use the same format, so
//! copying one over just works.
//!
//! A file is turned into a `Theme` the way the built-ins are (exactly as the built-in theme files in `src/themes/`):
//! the same role derivation, then the same contrast floors. A dark file is lifted until it reads, so a sloppy or
//! low-contrast one still gives something readable; a light one has to read as it is, like the built-in light ones.
//! What can't be made to work is skipped with a reason that the picker and `platoon themes` show (a light theme that
//! doesn't read, a file that isn't a theme, ...). Nothing in here may panic or hang on a strange file: sizes and counts
//! are capped, only regular files are opened (a named pipe would block), and text that isn't UTF-8 is skipped rather
//! than guessed at.

use crate::theme::{
    MIN_BORDER_ON_SURFACE, MIN_DELTA_E, MIN_DIM, MIN_ROLE, MIN_SUBTEXT, MIN_SURFACE, MIN_TEXT, Theme, contrast, delta_e, is_light,
    lift_contrast, mix,
};
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
/// What a theme file says: the two required colors, whichever palette slots it defines, and the colors it picks for
/// platoon's five roles itself, if any (`platoon-accent = #hex`, see `ROLE_KEYS`).
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    pub background: Color,
    pub foreground: Color,
    pub palette: [Option<Color>; 16],
    /// accent, busy, ok, warn, err as the file picks them, where a palette's own color doesn't suit (a theme made
    /// for platoon, like the built-ins' hand-picked colors). Ghostty has no such keys; they are platoon's own.
    pub roles: [Option<Color>; 5],
}

/// The keys a theme file can pick platoon's five roles with, in `Parsed::roles` order.
pub const ROLE_KEYS: [&str; 5] = ["platoon-accent", "platoon-busy", "platoon-ok", "platoon-warn", "platoon-err"];
/// The palette slots each role comes from, (bright, normal), in `Parsed::roles` order.
pub const ROLE_SLOTS: [(usize, usize); 5] = [(12, 4), (13, 5), (10, 2), (11, 3), (9, 1)];

impl Parsed {
    /// Whether the file is a light theme.
    pub fn light(&self) -> bool {
        is_light(self.background)
    }

    /// The five role colors the file gives, accent / busy / ok / warn / err (None where it gives none): a color the
    /// file picks itself (`platoon-accent`, ...), else a palette slot. On a dark background the bright slot (12, 13,
    /// 10, 11, 9) and failing that the normal one (4, 5, 2, 3, 1), as always. On a light one the normal slot when
    /// it reads on the background, else the bright one when that does, else whichever reads better: a light
    /// theme's normal colors are usually its darker ones, but not always (Gruvbox Light's are the other way round).
    pub fn role_picks(&self) -> [Option<Color>; 5] {
        let light = self.light();
        let reads = |c: &Color| contrast(*c, self.background) >= MIN_ROLE;
        std::array::from_fn(|i| {
            let (hi, lo) = ROLE_SLOTS[i];
            let (bright, normal) = (self.palette[hi], self.palette[lo]);
            self.roles[i].or_else(|| {
                if !light {
                    return bright.or(normal);
                }
                normal.filter(reads).or(bright.filter(reads)).or(match (normal, bright) {
                    (Some(n), Some(b)) => Some(if contrast(b, self.background) > contrast(n, self.background) { b } else { n }),
                    (n, b) => n.or(b),
                })
            })
        })
    }
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

/// Read a theme file's text. Lines are `key = value`; only `background`, `foreground`, `palette = N=#hex` (N
/// 0..=15) and platoon's own role keys (`ROLE_KEYS`) count, everything else (cursor and selection colors, comments,
/// blank lines, lines with no `=`) is ignored. A later line for the same key wins, as in Ghostty. A bad color in a palette slot leaves that slot
/// undefined (it gets filled in later); a missing or bad background or foreground is an error, since a theme
/// without them can't be made.
pub fn parse(text: &str) -> Result<Parsed, String> {
    let mut background: Option<Result<Color, String>> = None;
    let mut foreground: Option<Result<Color, String>> = None;
    let mut palette = [None; 16];
    let mut roles = [None; 5];
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
                if let Some((n, c)) = value.split_once('=')
                    && let (Ok(n), Some(c)) = (n.trim().parse::<usize>(), hex(c))
                    && n < 16
                {
                    palette[n] = Some(c);
                }
            }
            other => {
                if let Some(i) = ROLE_KEYS.iter().position(|k| *k == other) {
                    // A bad color leaves the role to the palette, like a bad palette entry.
                    roles[i] = hex(value);
                }
            }
        }
    }
    let background = background.ok_or("no background color")??;
    let foreground = foreground.ok_or("no foreground color")??;
    Ok(Parsed { background, foreground, palette, roles })
}

/// A value quoted in a message: short, and free of anything that could act on a terminal.
fn shorten(s: &str) -> String {
    let s: String = s.chars().filter(|c| !c.is_control()).take(16).collect();
    s
}

const WHITE: Color = Color::Rgb(255, 255, 255);
const BLACK: Color = Color::Rgb(0, 0, 0);

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

/// The theme one of the user's own files describes, or why there isn't one. A dark file is built (`build`) and lifted
/// until it reads, whatever it says. A light one is held to the floors the built-in theme files meet (`problems`: it
/// has to read as it is, since a background just on the light side leaves nothing to lift the text toward) and then
/// built the same way; one that doesn't read gets the first reason, in plain words, and how many more there are.
pub fn theme_from(id: String, name: String, p: &Parsed) -> Result<Theme, String> {
    if p.light() {
        let problems = problems(p);
        if let Some(first) = problems.first() {
            let more = match problems.len() - 1 {
                0 => String::new(),
                1 => ", and 1 more problem".into(),
                n => format!(", and {n} more problems"),
            };
            return Err(format!("a light theme has to read as it is: {first}{more}"));
        }
    }
    Ok(build(id, name, p))
}

/// Why a theme file doesn't read well enough to be used as it is, in plain words (empty: it does). The same floors the
/// built-ins are held to, on the file's own colors, before any lifting: the text at 7:1 on the background, the five
/// colors that carry meaning (`Parsed::role_picks`: the file's own `platoon-accent` ... or its palette) at 4.5:1
/// and told apart. Dark and light themes alike: the ratio is the same whichever side is the lighter. Every built-in
/// theme file meets it (a test checks), and a light theme in the user's own folder is held to it (`theme_from`).
pub fn problems(p: &Parsed) -> Vec<String> {
    let mut out = Vec::new();
    let (bg, fg) = (p.background, p.foreground);
    let ratio = |c| contrast(c, bg);
    if ratio(fg) < MIN_TEXT {
        out.push(format!("the text is hard to read on the background ({:.1}:1; it needs {MIN_TEXT}:1)", ratio(fg)));
    }
    let names = ["accent", "busy", "ok", "warning", "error"];
    let mut picked = Vec::new();
    for (i, pick) in p.role_picks().into_iter().enumerate() {
        let (role, (hi, lo)) = (names[i], ROLE_SLOTS[i]);
        match pick {
            None => out.push(format!("palette {hi} (or {lo}) is missing: it is the {role} color (or give it as {})", ROLE_KEYS[i])),
            Some(c) => {
                if ratio(c) < MIN_ROLE {
                    out.push(format!("the {role} color is hard to read on the background ({:.1}:1; it needs {MIN_ROLE}:1)", ratio(c)));
                }
                picked.push((role, c));
            }
        }
    }
    for (i, (a, ca)) in picked.iter().enumerate() {
        for (b, cb) in &picked[i + 1..] {
            if delta_e(*ca, *cb) < MIN_DELTA_E {
                out.push(format!("the {a} and {b} colors look too alike"));
            }
        }
    }
    out
}

/// The theme a file describes. Same roles as the built-ins: bg and text from the file, surface, border, dim and
/// subtext mixed between them, accent / busy / ok / warn / err as `Parsed::role_picks` finds them. Then the
/// contrast floors: the roles and the muted text are lifted toward the text color until they read, surface and
/// border are kept apart from the background and from each other, and the five colors that carry meaning are kept
/// clearly distinct. On a light background everything works the same way the other way round: the text and
/// whatever is lifted go darker (toward black rather than white), and the surface (the selection fill) is darker
/// than the background by `MIN_SURFACE`.
pub fn build(id: String, name: String, p: &Parsed) -> Theme {
    let (bg, fg) = (p.background, p.foreground);
    let light = p.light();
    let strongest = if light { BLACK } else { WHITE };
    let pal = resolve(p);
    let given = p.role_picks();
    let picks: [Color; 5] = std::array::from_fn(|i| given[i].unwrap_or(pal[ROLE_SLOTS[i].0]));

    // Everything is mixed toward the text color; the text itself is lifted last, on the surface it will sit on.
    let toward = lift_contrast(fg, bg, strongest, MIN_TEXT);
    let surface = mixed_until(bg, toward, 0.09, |c| contrast(c, bg) >= MIN_SURFACE);
    let text = lift_contrast(toward, surface, strongest, MIN_SUBTEXT);
    let border = mixed_until(bg, toward, 0.20, |c| contrast(c, bg) >= 1.25 && contrast(c, surface) >= MIN_BORDER_ON_SURFACE);
    // Dim and subtext are held to their floors on the page; on the selection fill they are kept readable where they
    // are drawn (`Theme::readable_on`), so a stronger fill doesn't make them heavier everywhere.
    let dim = lift_contrast(mix(bg, toward, 0.46), bg, toward, MIN_DIM);
    let subtext = lift_contrast(mix(bg, toward, 0.72), bg, toward, MIN_SUBTEXT);
    let roles = distinct(picks, bg, text);
    let [accent, busy, ok, warn, err] = roles;
    Theme { id, name, bg, surface, border, dim, subtext, text, accent, busy, ok, warn, err, on_accent: bg, ansi: Some(pal), light }
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
    let odd =
        |c: char| c.is_control() || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}');
    let name: String = file.to_string_lossy().chars().map(|c| if odd(c) { ' ' } else { c }).collect();
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    name.chars().take(MAX_NAME).collect::<String>().trim_end().to_string()
}

/// Lowercase ASCII letters and digits, anything else a single dash: `Nord Light` is `nord-light`. An accented Latin
/// letter is its plain letter (`Rosé Pine Dawn` is `rose-pine-dawn`, see `plain_letters`), and a combining accent (a
/// name written decomposed, as macOS may store a file name) adds nothing. A name with nothing left gets a short hash,
/// so the id is still stable from run to run.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if let Some(plain) = plain_letters(c) {
            out.push_str(plain);
        } else if matches!(c, '\u{300}'..='\u{36f}') {
            // A combining accent belongs to the letter before it.
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

/// A lowercase Latin-1 or Latin Extended-A letter as plain ASCII letters (é is e, ß is ss, æ is ae, þ is th), or None
/// for anything else.
fn plain_letters(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'ď' | 'đ' | 'ð' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'ĳ' => "ij",
        'ĵ' => "j",
        'ķ' | 'ĸ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' | 'ŉ' | 'ŋ' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'œ' => "oe",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' | 'ſ' => "s",
        'ß' => "ss",
        'ţ' | 'ť' | 'ŧ' => "t",
        'þ' => "th",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        _ => return None,
    })
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

    /// "Half: no background color", one per file, plus a line for files never read.
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
    fs::File::open(path)
        .and_then(|f| f.take(MAX_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|_| "the file couldn't be read".to_string())?;
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
        let skip = |out: &mut Scan, name: &str, reason: String| {
            out.skipped.push(Skipped { name: if name.is_empty() { "(unnamed)".into() } else { name.to_string() }, reason })
        };
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

/// The user's themes as of the last look at the folder, and what is needed to see that it changed.
#[derive(Default)]
pub struct Registry {
    dir: PathBuf,
    print: Option<Print>,
    scan: Scan,
}

impl Registry {
    /// The themes in `<config>/themes`, read now. (`Registry::default()` has none and never looks anywhere.)
    pub fn open(config: &Path) -> Self {
        let mut r = Self { dir: config.join("themes"), ..Self::default() };
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
    }

    /// Look at the folder again: a listing of names, sizes and times, and the files are only read again if that
    /// changed. True when they were. Bounded by `MAX_ENTRIES`, so cheap enough for every settings refresh.
    pub fn refresh(&mut self) -> bool {
        let now = self.fingerprint();
        if now != self.print {
            self.reload();
            return true;
        }
        false
    }

    /// The user's themes, in file-name order.
    pub fn user(&self) -> &[Theme] {
        &self.scan.themes
    }

    pub fn skipped(&self) -> &Scan {
        &self.scan
    }

    /// Built-ins in their order, then the user's.
    pub fn all(&self) -> Vec<Theme> {
        Theme::all().into_iter().chain(self.user().iter().cloned()).collect()
    }

    pub fn by_id(&self, id: &str) -> Option<Theme> {
        Theme::by_id(id).or_else(|| self.scan.themes.iter().find(|t| t.id == id).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::luminance;

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
        for text in [
            "\0\0\0",
            "===",
            "=",
            "palette",
            "palette=",
            "palette==",
            "background=",
            &"x".repeat(100_000),
            "\u{feff}",
            "é=é\nñ",
            "palette = 99999999999999999999=#fff",
        ] {
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

    /// Everything the built-ins are held to, for one derived theme, dark or light. A floor is only demanded where
    /// white (black, on a light one) on that background could meet it: on a dark but not very dark one (a saturated
    /// blue) nothing can read at 7:1.
    fn assert_readable(t: &Theme) {
        let reach = contrast(if t.light { c(0) } else { c(0xffffff) }, t.bg) - 0.01;
        let on = |x: Color| contrast(x, t.bg);
        assert!(on(t.text) >= MIN_TEXT.min(reach), "{}: text {:.2}", t.name, on(t.text));
        assert!(on(t.subtext) >= MIN_SUBTEXT.min(reach), "{}: subtext {:.2}", t.name, on(t.subtext));
        assert!(on(t.dim) >= MIN_DIM.min(reach), "{}: dim {:.2}", t.name, on(t.dim));
        for (n, col) in [("accent", t.accent), ("busy", t.busy), ("ok", t.ok), ("warn", t.warn), ("err", t.err)] {
            assert!(on(col) >= MIN_ROLE.min(reach), "{}: {n} {:.2}", t.name, on(col));
        }
        assert!(on(t.text) >= on(t.subtext) && on(t.subtext) >= on(t.dim), "{}: text stays ordered", t.name);
        assert!(contrast(t.surface, t.bg) >= MIN_SURFACE.min(on(t.text)), "{}: surface vs bg", t.name);
        assert!(contrast(t.border, t.bg) >= 1.25, "{}: border vs bg", t.name);
        assert!(contrast(t.border, t.surface) >= MIN_BORDER_ON_SURFACE, "{}: border vs surface", t.name);
        if luminance(t.bg) < 0.05 {
            assert!(contrast(t.surface, t.bg) >= MIN_SURFACE, "{}: the selection fill", t.name);
            // Dim text is kept readable on the selected row where it is drawn; the text reads there as it is.
            assert!(contrast(t.readable_on(t.dim, t.surface, MIN_DIM), t.surface) >= MIN_DIM, "{}: dim on surface", t.name);
            assert!(contrast(t.readable_on(t.subtext, t.surface, MIN_SUBTEXT), t.surface) >= MIN_SUBTEXT, "{}: subtext on surface", t.name);
            assert!(contrast(t.text, t.surface) >= MIN_SUBTEXT, "{}: text on surface", t.name);
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
        for (bg, fg, col) in [
            ("#000000", "#ffffff", "#ff0000"),
            ("#000000", "#000000", "#000000"),
            ("#05050a", "#101018", "#20202a"),
            ("#111111", "#aaaaaa", "#222222"),
        ] {
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
    fn every_bundled_ghostty_theme_becomes_a_readable_theme_or_a_light_one_says_why_when_they_are_installed() {
        // Not part of the shipped behavior, only a wide check when the machine has Ghostty (skipped otherwise).
        let dir = Path::new("/usr/share/ghostty/themes");
        let Ok(entries) = fs::read_dir(dir) else { return };
        for e in entries.flatten().take(500) {
            let Ok(text) = fs::read_to_string(e.path()) else { continue };
            let Ok(p) = parse(&text) else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            match theme_from("user:t".into(), name.clone(), &p) {
                Ok(t) => {
                    assert_eq!(t.light, p.light(), "{name}");
                    assert_readable(&t);
                }
                Err(why) => assert!(
                    p.light() && why.starts_with("a light theme has to read as it is: ") && !problems(&p).is_empty(),
                    "{name}: {why}"
                ),
            }
        }
    }

    #[test]
    fn a_light_theme_that_reads_as_it_is_loads_and_one_that_doesnt_says_why() {
        // The built-in Solarized Light, as a file in the user's folder: it reads on its background as it is.
        let good = include_str!("themes/Solarized Light");
        let p = parse(good).unwrap();
        assert!(p.light() && problems(&p).is_empty(), "{:?}", problems(&p));
        let t = theme_from("user:sol".into(), "Sol".into(), &p).unwrap();
        assert_eq!(t, build("user:sol".into(), "Sol".into(), &p), "built the way a built-in theme file is");
        assert!(t.light);
        assert_eq!((t.text, t.accent, t.ok, t.warn), (c(0x47565c), c(0x6268b8), c(0x687700), c(0x8b6900)), "used as it is");
        assert_readable(&t);
        // Black on white with no colors of its own: it has no accent to offer, and says so first.
        let bare = theme_from("user:l".into(), "L".into(), &parse("background = #ffffff\nforeground = #000000").unwrap()).unwrap_err();
        assert_eq!(
            bare,
            "a light theme has to read as it is: palette 12 (or 4) is missing: it is the accent color (or give it as platoon-accent), and 4 more problems"
        );
        // Faint text: the text's own ratio, in plain words, and a light theme is never lightened or darkened to fit.
        let faint = format!("{good}\nforeground = #b0b0a8");
        let why = theme_from("u".into(), "n".into(), &parse(&faint).unwrap()).unwrap_err();
        assert!(
            why.starts_with("a light theme has to read as it is: the text is hard to read on the background (2.")
                && why.ends_with("it needs 7:1)")
                && !why.contains("more"),
            "{why}"
        );
        // Right at the threshold the built-ins use: just dark is lifted and loads; just light has nothing to reach 7:1 with.
        assert!(
            theme_from("u".into(), "n".into(), &parse("background=#707070\nforeground=#fff").unwrap()).is_ok(),
            "luminance {:.3}",
            luminance(c(0x707070))
        );
        let why = theme_from("u".into(), "n".into(), &parse("background=#808080\nforeground=#fff").unwrap()).unwrap_err();
        assert!(
            why.contains("the text is hard to read on the background (3.9:1; it needs 7:1)"),
            "luminance {:.3}: {why}",
            luminance(c(0x808080))
        );
    }

    #[test]
    fn a_file_can_pick_platoons_five_colors_itself_and_a_bad_pick_leaves_it_to_the_palette() {
        let text = format!("{NORD}platoon-accent = #88c0d0\nPLATOON-ERR = e0838c\nplatoon-busy = nope\nplatoon-other = #ffffff\n");
        let p = parse(&text).unwrap();
        assert_eq!(p.roles, [Some(c(0x88c0d0)), None, None, None, Some(c(0xe0838c))]);
        assert_eq!(p.role_picks(), [Some(c(0x88c0d0)), Some(c(0xb48ead)), Some(c(0xa3be8c)), Some(c(0xebcb8b)), Some(c(0xe0838c))]);
        let t = build("user:n".into(), "N".into(), &p);
        assert_eq!((t.accent, t.err), (c(0x88c0d0), c(0xe0838c)), "the file's own picks, which read, are used as they are");
        assert_eq!(parse(NORD).unwrap().roles, [None; 5], "a Ghostty file has none");
    }

    #[test]
    fn a_light_file_picks_the_colors_that_read_on_it_and_builds_a_light_theme() {
        // Normal colors that read (blue, magenta), one that doesn't where the bright one does (green), and a pale
        // yellow pair where neither does: the darker of the two is picked (and lifted when built).
        let text = "background = #fafafa\nforeground = #202020\npalette = 4=#1a55c0\npalette = 12=#6fa0ff\npalette = 5=#9a2fa0\npalette = 13=#e08ae6\npalette = 2=#7fcf7f\npalette = 10=#2e7d32\npalette = 3=#f0d060\npalette = 11=#d8a820\npalette = 1=#c62828\npalette = 9=#ff8080";
        let p = parse(text).unwrap();
        assert!(p.light());
        assert_eq!(p.role_picks(), [Some(c(0x1a55c0)), Some(c(0x9a2fa0)), Some(c(0x2e7d32)), Some(c(0xd8a820)), Some(c(0xc62828))]);
        let t = build("user:l".into(), "L".into(), &p);
        assert!(t.light);
        assert_eq!(t.text, c(0x202020));
        assert!(luminance(t.surface) < luminance(t.bg) && luminance(t.border) < luminance(t.surface), "darker toward the text");
        let on = |x: Color| contrast(x, t.bg);
        assert!(on(t.text) >= MIN_TEXT && on(t.subtext) >= MIN_SUBTEXT && on(t.dim) >= MIN_DIM);
        for x in [t.accent, t.busy, t.ok, t.warn, t.err] {
            assert!(on(x) >= MIN_ROLE, "{x:?}");
        }
        assert!(luminance(t.warn) < luminance(c(0xd8a820)), "the yellow is darkened until it reads, not lightened");
        assert!(contrast(t.surface, t.bg) >= MIN_SURFACE && contrast(t.text, t.surface) >= MIN_SUBTEXT);
        assert!(contrast(t.readable_on(t.dim, t.surface, MIN_DIM), t.surface) >= MIN_DIM);
        // A light theme with faint text gets darker text, never lighter.
        let faint = build("x".into(), "F".into(), &parse("background = #fafafa\nforeground = #9a9a9a").unwrap());
        assert!(contrast(faint.text, faint.bg) >= MIN_TEXT && luminance(faint.text) < luminance(c(0x9a9a9a)));
        // In the user's own folder a light theme has to read as it is, and this one's yellow doesn't.
        let why = theme_from("user:l".into(), "L".into(), &p).unwrap_err();
        assert!(why.starts_with("a light theme has to read as it is: the warning color is hard to read on the background (2."), "{why}");
        // Text on a fill: the theme's own text or background when one reads, else black or white.
        assert_eq!(t.text_on(t.accent), t.bg);
        assert_eq!(t.text_on(c(0xf0d060)), t.text);
        assert_eq!(t.text_on(c(0xb07a10)), c(0), "a middling color: neither of the theme's reads at 4.5:1");
        let dark = build("x".into(), "D".into(), &parse(NORD).unwrap());
        assert!(!dark.light);
    }

    #[test]
    fn ids_are_slugs_stable_and_never_empty() {
        assert_eq!(slug("Nord"), "nord");
        assert_eq!(slug("Nord Light"), "nord-light");
        assert_eq!(slug("  --Catppuccin_Mocha!! "), "catppuccin-mocha");
        assert_eq!(slug("Rosé Pine"), "rose-pine", "an accented letter is its plain letter");
        assert_eq!(slug("Rosé Pine Dawn"), "rose-pine-dawn");
        assert_eq!(slug("Rose\u{301} Pine Dawn"), "rose-pine-dawn", "written decomposed, as macOS may store a file name");
        assert_eq!(slug("ÀÁÂÄÃÅ àáâäãå"), "aaaaaa-aaaaaa");
        assert_eq!(slug("Ça Ñu Öø Üü"), "ca-nu-oo-uu");
        assert_eq!(slug("Straße Æther Œuvre"), "strasse-aether-oeuvre");
        assert_eq!(slug("Łódź Ďěčí Þórr"), "lodz-deci-thorr");
        assert_eq!(slug("Frappé"), "frappe");
        assert_eq!(slug("Crème ☕ Brûlée"), "creme-brulee", "anything else is dropped, as before");
        assert_eq!(slug("Nuit ƒ"), "nuit");
        let greek = slug("Ωμέγα");
        assert!(greek.starts_with("theme-"), "{greek}: not Latin, so the hash");
        assert_eq!(slug("a.b.c"), "a-b-c");
        let cjk = slug("夜の海");
        assert!(cjk.starts_with("theme-") && cjk.len() == 14, "{cjk}");
        assert_eq!(cjk, slug("夜の海"), "the same name gives the same id every run");
        assert_ne!(cjk, slug("夜の空"));
        for name in ["Nord", "", "x y", "-", "..", "é", "\u{301}", "ßẞİ", "Ǆ"] {
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
        put(
            &d,
            "Paper",
            "background = #fafafa\nforeground = #202020\npalette = 4=#1a55c0\npalette = 5=#9a2fa0\npalette = 2=#2e7d32\npalette = 3=#8a5d00\npalette = 1=#c62828\n",
        );
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
        assert_eq!(
            names,
            ["Alpha", "Nord (yours)", "Paper", "Zeta"],
            "name order; a name a built-in has is marked; a light theme that reads"
        );
        let ids: Vec<&str> = scan.themes.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["user:alpha", "user:nord", "user:paper", "user:zeta"]);
        assert!(scan.themes[2].light && !scan.themes[0].light);
        let reasons = scan.reasons();
        let has = |name: &str, why: &str| reasons.iter().any(|r| r.starts_with(&format!("{name}: ")) && r.contains(why));
        assert!(has("Nord Light", "a light theme has to read as it is: palette 12 (or 4) is missing"), "{reasons:?}");
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
        for name in
            ["with space", "ünïcode", "夜の海", "dots.and.more", "-dash", "A\u{1b}[2Jb", "x".repeat(200).as_str(), "trailing.", "a\tb"]
        {
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
