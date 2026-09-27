//! The keys the user can change, in `<config dir>/keys.json` beside `settings.json`: an object of action name -> key,
//! like `{ "back": "ctrl+space", "quit": "q" }`. Only `back` (one level back, everywhere: a session, `platoon open`,
//! the notes editor, the code editor, every screen and popup) and `quit` (from the top-level lists) for now; the table
//! (`ACTIONS`) is where others join later, one screen at a time, without the file changing shape.
//!
//! platoon never writes this file. A missing file is the defaults; one that can't be read or isn't a JSON object is the
//! defaults with one notice; an entry that isn't a key platoon can use is ignored with one notice saying which entry and
//! what is used instead; names it doesn't know are left alone. It is re-read when it changes (`KeysStore::refresh`), so an
//! edit applies without restarting.
//!
//! A key is written in lower case (any case is read): modifiers `ctrl+`, `alt+`, `shift+`, then a key name (`space`,
//! `esc`, `enter`, `tab`, `backspace`, `delete`, `insert`, `home`, `end`, `pageup`, `pagedown`, `up`, `down`, `left`,
//! `right`, `f1`..`f12`, or one character). The back key must be told apart from typing in three places: platoon's own
//! screen (crossterm's key events), `platoon open` (the raw bytes the terminal sends) and the code editor (Neovim's
//! notation), so it is a ctrl chord of `space` or a letter, or an alt chord of `space`, a letter or a digit. Ctrl+H, I,
//! J, M (and Ctrl+[) are refused: a terminal sends them as Backspace, Tab, Enter and Esc.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;
use std::{
    fs, io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// A key on its own, without modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Name {
    /// One character, as typed (an upper-case letter is `shift+` the letter).
    Char(char),
    Space,
    Esc,
    Enter,
    Tab,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    F(u8),
}

const NAMES: &[(&str, Name)] = &[
    ("space", Name::Space),
    ("esc", Name::Esc),
    ("escape", Name::Esc),
    ("enter", Name::Enter),
    ("return", Name::Enter),
    ("tab", Name::Tab),
    ("backspace", Name::Backspace),
    ("delete", Name::Delete),
    ("del", Name::Delete),
    ("insert", Name::Insert),
    ("home", Name::Home),
    ("end", Name::End),
    ("pageup", Name::PageUp),
    ("pgup", Name::PageUp),
    ("pagedown", Name::PageDown),
    ("pgdn", Name::PageDown),
    ("up", Name::Up),
    ("down", Name::Down),
    ("left", Name::Left),
    ("right", Name::Right),
    ("plus", Name::Char('+')),
];

/// A key with its modifiers. An upper-case letter is kept as the character (`shift` is then false).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: Name,
}

impl Chord {
    /// Read `ctrl+space`, `alt+b`, `q`, `shift+tab`, `f10`... in any case. Err says what is wrong, in plain words.
    pub fn parse(text: &str) -> Result<Chord, String> {
        let s = text.trim().to_lowercase();
        if s.is_empty() {
            return Err("is empty".into());
        }
        // The key itself may be `+` ("+", "ctrl++").
        let (mods, key) = if s == "+" {
            ("", "+")
        } else if let Some(m) = s.strip_suffix("++") {
            (m, "+")
        } else {
            match s.rsplit_once('+') {
                Some(("", _)) => return Err("has a + with nothing on one side".into()),
                Some((m, k)) => (m, k),
                None => ("", s.as_str()),
            }
        };
        let mut c = Chord { ctrl: false, alt: false, shift: false, key: Name::Space };
        if !mods.is_empty() {
            for m in mods.split('+') {
                match m.trim() {
                    "ctrl" | "control" => c.ctrl = true,
                    "alt" | "meta" | "option" => c.alt = true,
                    "shift" => c.shift = true,
                    "" => return Err("has a + with nothing on one side".into()),
                    other => return Err(format!("has \"{other}\", which isn't ctrl, alt or shift")),
                }
            }
        }
        let key = key.trim();
        c.key = if let Some((_, n)) = NAMES.iter().find(|(n, _)| *n == key) {
            *n
        } else if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()).filter(|n| (1..=12).contains(n)) {
            Name::F(n)
        } else {
            let mut chars = key.chars();
            match (chars.next(), chars.next()) {
                (Some(' '), None) => Name::Space,
                (Some(ch), None) if !ch.is_control() => Name::Char(ch),
                _ => return Err(format!("names \"{key}\", which isn't a key platoon knows")),
            }
        };
        // Shift with a letter is the upper-case letter; with another character, the character itself is written.
        if c.shift {
            if let Name::Char(ch) = c.key {
                if ch.is_alphabetic() && !c.ctrl && !c.alt {
                    c.key = Name::Char(ch.to_uppercase().next().unwrap_or(ch));
                    c.shift = false;
                } else if !ch.is_alphabetic() {
                    return Err(format!("has shift with \"{ch}\": write the character shift gives instead"));
                }
            }
        }
        Ok(c)
    }

    /// The key as the hints show it and as it is written in keys.json: `ctrl+space`, `alt+b`, `q`, `shift+q`.
    pub fn label(&self) -> String {
        let mut out = String::new();
        if self.ctrl {
            out.push_str("ctrl+");
        }
        if self.alt {
            out.push_str("alt+");
        }
        let upper = matches!(self.key, Name::Char(ch) if ch.is_uppercase());
        if self.shift || upper {
            out.push_str("shift+");
        }
        match self.key {
            Name::Char(ch) if upper => out.extend(ch.to_lowercase()),
            Name::Char(ch) => out.push(ch),
            Name::F(n) => out.push_str(&format!("f{n}")),
            other => out.push_str(NAMES.iter().find(|(_, n)| *n == other).map_or("?", |(s, _)| s)),
        }
        out
    }

    /// Whether a key event from the terminal is this key. Ctrl+letter matches whichever case the terminal reports;
    /// a plain character must be that very character (shift is in it already).
    pub fn matches(&self, ev: &KeyEvent) -> bool {
        let m = ev.modifiers;
        if m.contains(KeyModifiers::CONTROL) != self.ctrl || m.contains(KeyModifiers::ALT) != self.alt {
            return false;
        }
        match (self.key, ev.code) {
            (Name::Space, KeyCode::Char(' ')) => true,
            (Name::Char(want), KeyCode::Char(got)) if self.ctrl => got.to_lowercase().eq(want.to_lowercase()),
            (Name::Char(want), KeyCode::Char(got)) => got == want,
            (Name::Tab, KeyCode::BackTab) => self.shift,
            (Name::Tab, KeyCode::Tab) => !self.shift,
            (key, code) => {
                let same = match (key, code) {
                    (Name::Esc, KeyCode::Esc)
                    | (Name::Enter, KeyCode::Enter)
                    | (Name::Backspace, KeyCode::Backspace)
                    | (Name::Delete, KeyCode::Delete)
                    | (Name::Insert, KeyCode::Insert)
                    | (Name::Home, KeyCode::Home)
                    | (Name::End, KeyCode::End)
                    | (Name::PageUp, KeyCode::PageUp)
                    | (Name::PageDown, KeyCode::PageDown)
                    | (Name::Up, KeyCode::Up)
                    | (Name::Down, KeyCode::Down)
                    | (Name::Left, KeyCode::Left)
                    | (Name::Right, KeyCode::Right) => true,
                    (Name::F(a), KeyCode::F(b)) => a == b,
                    _ => false,
                };
                same && m.contains(KeyModifiers::SHIFT) == self.shift
            }
        }
    }

    /// The ASCII byte of the key a ctrl or alt chord is made with (`space` is a space); None for anything else.
    fn base_byte(&self) -> Option<u8> {
        match self.key {
            Name::Space => Some(b' '),
            Name::Char(ch) if ch.is_ascii_lowercase() || ch.is_ascii_digit() => Some(ch as u8),
            _ => None,
        }
    }

    /// The byte sequences a terminal may send for this chord, for `platoon open`, which reads raw bytes: the classic
    /// form (Ctrl+Space = NUL, Ctrl+letter = 0x01..0x1a, Alt+x = ESC x) and the two newer ones a program can switch
    /// the terminal to (kitty's `CSI code;mod u`, xterm's `CSI 27;mod;code ~`). Empty for a key that isn't a
    /// usable back key (`check_back` refuses those).
    pub fn raw_forms(&self) -> Vec<Vec<u8>> {
        let Some(b) = self.base_byte() else { return Vec::new() };
        let (classic, modifier) = match (self.ctrl, self.alt, self.shift) {
            (true, false, false) if b == b' ' => (vec![0], 5),
            (true, false, false) if b.is_ascii_lowercase() => (vec![b - b'a' + 1], 5),
            (false, true, false) => (vec![0x1b, b], 3),
            _ => return Vec::new(),
        };
        vec![
            classic,
            format!("\x1b[{b};{modifier}u").into_bytes(),
            format!("\x1b[{b};{modifier}:1u").into_bytes(),
            format!("\x1b[27;{modifier};{b}~").into_bytes(),
        ]
    }

    /// The key in Neovim's notation, for the code editor: `<C-Space>`, `<C-b>`, `<M-b>`, `<M-1>`.
    pub fn nvim(&self) -> String {
        let mut mods = String::new();
        if self.ctrl {
            mods.push_str("C-");
        }
        if self.alt {
            mods.push_str("M-");
        }
        if self.shift {
            mods.push_str("S-");
        }
        let key = match self.key {
            Name::Space => "Space".to_string(),
            Name::Char('<') => "lt".to_string(),
            Name::Char('\\') => "Bslash".to_string(),
            Name::Char('|') => "Bar".to_string(),
            Name::Char(ch) => ch.to_string(),
            Name::Esc => "Esc".into(),
            Name::Enter => "CR".into(),
            Name::Tab => "Tab".into(),
            Name::Backspace => "BS".into(),
            Name::Delete => "Del".into(),
            Name::Insert => "Insert".into(),
            Name::Home => "Home".into(),
            Name::End => "End".into(),
            Name::PageUp => "PageUp".into(),
            Name::PageDown => "PageDown".into(),
            Name::Up => "Up".into(),
            Name::Down => "Down".into(),
            Name::Left => "Left".into(),
            Name::Right => "Right".into(),
            Name::F(n) => format!("F{n}"),
        };
        if mods.is_empty() && matches!(self.key, Name::Char(_)) { key } else { format!("<{mods}{key}>") }
    }
}

/// Ctrl chords Claude Code (or the line editing in its prompt) uses itself: allowed as the back key, with a notice.
const CLAUDE_CTRL: &str = "abcdefgklnoprstuvwyz";
/// Alt chords the same: word moves and deletes in the prompt, switching the model and thinking.
const CLAUDE_ALT: &str = "bdfmpt";

/// Whether `c` can be the back key. Ok(Some(warning)) when it can but something else uses it too; Err(why) when it
/// can't, in words that finish "\"back\" is \"...\", which ...".
pub fn check_back(c: &Chord) -> Result<Option<String>, String> {
    if !c.ctrl && !c.alt {
        return Err("would be typed rather than go back (it needs ctrl or alt)".into());
    }
    if c.ctrl && c.alt {
        return Err("holds both ctrl and alt, which terminals don't send the same way (use one of them)".into());
    }
    if c.shift || matches!(c.key, Name::Char(ch) if ch.is_uppercase()) {
        return Err("has shift, which terminals don't send with ctrl or alt in a way platoon can always tell apart".into());
    }
    if c.ctrl {
        return match c.key {
            Name::Char(ch @ ('h' | 'i' | 'j' | 'm')) => {
                let same = match ch {
                    'h' => "backspace",
                    'i' => "tab",
                    _ => "enter",
                };
                Err(format!("is the same as {same} to a terminal"))
            }
            Name::Char('[') => Err("is the same as esc to a terminal".into()),
            Name::Char(ch) if ch.is_ascii_lowercase() && CLAUDE_CTRL.contains(ch) => Ok(Some(format!("Claude Code uses ctrl+{ch} itself: in a session it now goes back instead"))),
            Name::Space | Name::Char('a'..='z') => Ok(None),
            _ => Err("can't be told apart from other keys everywhere it is needed (use ctrl with space or a letter)".into()),
        };
    }
    match c.key {
        Name::Char(ch) if CLAUDE_ALT.contains(ch) => Ok(Some(format!("Claude Code uses alt+{ch} itself: in a session it now goes back instead"))),
        Name::Space | Name::Char('a'..='z' | '0'..='9') => Ok(None),
        _ => Err("can't be told apart from other keys everywhere it is needed (use alt with space, a letter or a digit)".into()),
    }
}

/// What kind of key an action may have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    /// It works inside sessions and editors too: a ctrl or alt chord all three places can tell apart (`check_back`).
    Chord,
    /// Only on platoon's own screens: any key.
    Any,
}

/// One action the user can give a key to.
pub struct Def {
    pub name: &'static str,
    pub default: &'static str,
    pub rule: Rule,
}

/// Every action with a key of its own, in the order notices are given. Another action joins here (and in `Keys`).
pub const ACTIONS: &[Def] = &[
    Def { name: "back", default: "ctrl+space", rule: Rule::Chord },
    Def { name: "quit", default: "q", rule: Rule::Any },
];

fn default_of(name: &str) -> Chord {
    let d = ACTIONS.iter().find(|d| d.name == name).map_or("", |d| d.default);
    Chord::parse(d).expect("the defaults are keys")
}

/// The keys in effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keys {
    pub back: Chord,
    pub quit: Chord,
    back_label: String,
    quit_label: String,
}

impl Default for Keys {
    fn default() -> Self {
        Keys::new(default_of("back"), default_of("quit"))
    }
}

impl Keys {
    fn new(back: Chord, quit: Chord) -> Keys {
        Keys { back_label: back.label(), quit_label: quit.label(), back, quit }
    }

    pub fn back_label(&self) -> &str {
        &self.back_label
    }

    pub fn quit_label(&self) -> &str {
        &self.quit_label
    }

    /// The keys a file's text gives, and a notice for each entry that was ignored or needs saying. Never fails.
    pub fn parse(text: &str) -> (Keys, Vec<String>) {
        let object = match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(o)) => o,
            Ok(_) => return (Keys::default(), vec!["keys.json isn't a list of keys like { \"back\": \"ctrl+space\" }: using the default keys.".into()]),
            Err(_) => return (Keys::default(), vec!["keys.json can't be read as JSON: using the default keys.".into()]),
        };
        let mut notes = Vec::new();
        let mut take = |name: &str, check: &dyn Fn(&Chord) -> Result<Option<String>, String>| -> Chord {
            let d = default_of(name);
            let Some(v) = object.get(name) else { return d };
            let Some(text) = v.as_str() else {
                notes.push(format!("keys.json: \"{name}\" should be a key written like \"{}\". Using {}.", d.label(), d.label()));
                return d;
            };
            let rule = ACTIONS.iter().find(|a| a.name == name).map_or(Rule::Any, |a| a.rule);
            let checked = |c: &Chord| match rule {
                Rule::Chord => check_back(c),
                Rule::Any => Ok(None),
            };
            let why = match Chord::parse(text) {
                Ok(c) => match checked(&c).and_then(|w| check(&c).map(|w2| w.or(w2))) {
                    Ok(None) => return c,
                    Ok(Some(warning)) => {
                        notes.push(format!("keys.json: \"{name}\" is \"{}\". {warning}.", c.label()));
                        return c;
                    }
                    Err(why) => why,
                },
                Err(why) => why,
            };
            notes.push(format!("keys.json: \"{name}\" is \"{}\", which {why}. Using {}.", text.trim(), d.label()));
            d
        };
        let back = take("back", &|_| Ok(None));
        let quit = take("quit", &|c| if *c == back { Err("is the back key too".to_string()) } else { Ok(None) });
        (Keys::new(back, quit), notes)
    }

    /// The keys in `<dir>/keys.json`: a missing file is the defaults, with nothing to say.
    pub fn load(dir: &Path) -> (Keys, Vec<String>) {
        match fs::read_to_string(path_in(dir)) {
            Ok(text) => Keys::parse(&text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (Keys::default(), Vec::new()),
            Err(_) => (Keys::default(), vec!["keys.json can't be read: using the default keys.".into()]),
        }
    }
}

pub fn path_in(dir: &Path) -> PathBuf {
    dir.join("keys.json")
}

/// (modified time, size, inode), as for settings.json: an atomic save changes the inode.
type Stamp = (SystemTime, u64, u64);

/// The keys plus what is needed to notice the file changed, and which notices were already given.
pub struct KeysStore {
    dir: PathBuf,
    stamp: Option<Stamp>,
    keys: Keys,
    /// The problems of the file as it was last read, said once each.
    told: Vec<String>,
    /// Notices not yet shown.
    pending: Vec<String>,
}

impl KeysStore {
    pub fn open(dir: &Path) -> Self {
        let mut s = KeysStore { dir: dir.to_path_buf(), stamp: None, keys: Keys::default(), told: Vec::new(), pending: Vec::new() };
        s.reload();
        s
    }

    pub fn get(&self) -> &Keys {
        &self.keys
    }

    fn stamp(&self) -> Option<Stamp> {
        fs::metadata(path_in(&self.dir)).ok().and_then(|m| Some((m.modified().ok()?, m.len(), m.ino())))
    }

    fn reload(&mut self) {
        self.stamp = self.stamp();
        let (keys, notes) = Keys::load(&self.dir);
        self.keys = keys;
        // A problem already said stays unsaid while it lasts; one that went away and came back is said again.
        self.pending.extend(notes.iter().filter(|n| !self.told.contains(n)).cloned());
        self.told = notes;
    }

    /// Pick up an edit. One `stat` when nothing changed. True when the file was re-read.
    pub fn refresh(&mut self) -> bool {
        if self.stamp() == self.stamp {
            return false;
        }
        self.reload();
        true
    }

    /// The notices not shown yet, as one line (None when there are none).
    pub fn take_notice(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        Some(std::mem::take(&mut self.pending).join(" "))
    }
}

/// Where the back key is in bytes read from a terminal: (start, length) of the first one. For `platoon open`.
pub fn find_back(buf: &[u8], forms: &[Vec<u8>]) -> Option<(usize, usize)> {
    forms.iter().filter(|f| !f.is_empty()).filter_map(|f| buf.windows(f.len()).position(|w| w == f.as_slice()).map(|at| (at, f.len()))).min_by_key(|&(at, len)| (at, std::cmp::Reverse(len)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Chord {
        Chord::parse(s).unwrap()
    }

    fn ev(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }

    fn dir(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-keys-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn key_strings_are_read_in_any_case_and_written_back_the_same() {
        assert_eq!(p("ctrl+space"), Chord { ctrl: true, alt: false, shift: false, key: Name::Space });
        assert_eq!(p("Ctrl+Space"), p("ctrl+space"));
        assert_eq!(p(" ALT+B "), Chord { ctrl: false, alt: true, shift: false, key: Name::Char('b') });
        assert_eq!(p("q").key, Name::Char('q'));
        assert_eq!(p("Q"), p("q"), "case doesn't matter; shift+q is the capital");
        assert_eq!(p("shift+q").key, Name::Char('Q'));
        assert_eq!(p("f10").key, Name::F(10));
        assert_eq!(p("escape").key, Name::Esc);
        assert_eq!(p("+").key, Name::Char('+'));
        assert_eq!(p("ctrl++"), Chord { ctrl: true, alt: false, shift: false, key: Name::Char('+') });
        assert_eq!(p("shift+tab"), Chord { ctrl: false, alt: false, shift: true, key: Name::Tab });
        for s in ["ctrl+space", "alt+b", "q", "shift+q", "f10", "shift+tab", "ctrl+alt+x", "esc", "alt+space", "pageup"] {
            assert_eq!(p(s).label(), s);
            assert_eq!(p(&p(s).label()), p(s));
        }
        for bad in ["", "hyper+x", "ctrl+", "ctrl+foo", "f13", "+x", "shift+1", "ctrl++x"] {
            assert!(Chord::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_chord_matches_the_events_a_terminal_gives_for_it() {
        let c = KeyModifiers::CONTROL;
        let back = p("ctrl+space");
        // crossterm reads NUL as ctrl + ' '
        assert!(back.matches(&ev(KeyCode::Char(' '), c)));
        assert!(!back.matches(&ev(KeyCode::Char(' '), KeyModifiers::NONE)));
        assert!(!back.matches(&ev(KeyCode::Char(' '), c | KeyModifiers::ALT)));
        let b = p("ctrl+b");
        assert!(b.matches(&ev(KeyCode::Char('b'), c)));
        assert!(b.matches(&ev(KeyCode::Char('B'), c | KeyModifiers::SHIFT)), "whichever case the terminal reports");
        assert!(!b.matches(&ev(KeyCode::Char('b'), KeyModifiers::NONE)));
        let ab = p("alt+b");
        assert!(ab.matches(&ev(KeyCode::Char('b'), KeyModifiers::ALT)));
        assert!(!ab.matches(&ev(KeyCode::Char('b'), c)));
        let q = p("q");
        assert!(q.matches(&ev(KeyCode::Char('q'), KeyModifiers::NONE)));
        assert!(!q.matches(&ev(KeyCode::Char('Q'), KeyModifiers::SHIFT)));
        assert!(!q.matches(&ev(KeyCode::Char('q'), c)));
        assert!(p("shift+q").matches(&ev(KeyCode::Char('Q'), KeyModifiers::SHIFT)));
        assert!(p("f10").matches(&ev(KeyCode::F(10), KeyModifiers::NONE)));
        assert!(p("shift+tab").matches(&ev(KeyCode::BackTab, KeyModifiers::SHIFT)));
        assert!(!p("tab").matches(&ev(KeyCode::BackTab, KeyModifiers::SHIFT)));
        assert!(p("esc").matches(&ev(KeyCode::Esc, KeyModifiers::NONE)));
    }

    #[test]
    fn the_back_key_must_be_a_chord_all_three_places_can_tell_apart() {
        for ok in ["ctrl+space", "ctrl+q", "ctrl+x", "alt+space", "alt+x", "alt+1", "ctrl+b", "alt+b"] {
            assert!(check_back(&p(ok)).is_ok(), "{ok}");
        }
        for (bad, why) in [
            ("q", "typed"),
            ("esc", "typed"),
            ("ctrl+h", "backspace"),
            ("ctrl+i", "tab"),
            ("ctrl+j", "enter"),
            ("ctrl+m", "enter"),
            ("ctrl+[", "esc"),
            ("ctrl+alt+x", "both"),
            ("ctrl+shift+x", "shift"),
            ("alt+shift+x", "shift"),
            ("ctrl+f1", "told apart"),
            ("alt+enter", "told apart"),
            ("ctrl+1", "told apart"),
            ("alt+[", "told apart"),
        ] {
            let e = check_back(&p(bad)).unwrap_err();
            assert!(e.contains(why), "{bad}: {e}");
        }
        // Accepted, but Claude Code uses them itself: said.
        for warned in ["ctrl+c", "ctrl+d", "ctrl+r", "ctrl+o", "ctrl+t", "ctrl+l", "ctrl+z", "alt+b"] {
            assert!(check_back(&p(warned)).unwrap().is_some_and(|w| w.contains("Claude Code")), "{warned}");
        }
        assert_eq!(check_back(&p("ctrl+space")), Ok(None));
        assert_eq!(check_back(&p("alt+x")), Ok(None));
        // Everything accepted has the three forms: bytes, an event, Neovim's notation.
        for ok in ["ctrl+space", "ctrl+q", "alt+x", "alt+1", "alt+space"] {
            assert!(!p(ok).raw_forms().is_empty(), "{ok}");
        }
    }

    #[test]
    fn the_bytes_of_a_chord_are_what_terminals_send() {
        let forms = p("ctrl+space").raw_forms();
        assert_eq!(forms[0], vec![0]);
        assert!(forms.contains(&b"\x1b[32;5u".to_vec()), "kitty's form");
        assert!(forms.contains(&b"\x1b[27;5;32~".to_vec()), "xterm's form");
        assert_eq!(p("ctrl+b").raw_forms()[0], vec![2]);
        assert_eq!(p("ctrl+z").raw_forms()[0], vec![26]);
        assert_eq!(p("alt+b").raw_forms()[0], b"\x1bb".to_vec());
        assert!(p("alt+b").raw_forms().contains(&b"\x1b[98;3u".to_vec()));
        assert_eq!(p("alt+1").raw_forms()[0], b"\x1b1".to_vec());
        assert!(p("q").raw_forms().is_empty());
    }

    #[test]
    fn platoon_open_finds_the_back_key_in_what_was_typed() {
        let space = p("ctrl+space").raw_forms();
        assert_eq!(find_back(b"hello\0more", &space), Some((5, 1)));
        assert_eq!(find_back(b"hello", &space), None);
        let alt = p("alt+b").raw_forms();
        assert_eq!(find_back(b"ab\x1bbcd", &alt), Some((2, 2)));
        assert_eq!(find_back(b"ab\0cd", &alt), None, "ctrl+space no longer detaches");
        assert_eq!(find_back(b"x\x1b[98;3u", &alt), Some((1, 7)), "the kitty form too");
        // Arrow keys and other escape sequences aren't the chord.
        assert_eq!(find_back(b"\x1b[A\x1bOB", &alt), None);
        let ctrl = p("ctrl+q").raw_forms();
        assert_eq!(find_back(b"ab\x11", &ctrl), Some((2, 1)));
        assert_eq!(find_back(b"ab\0", &ctrl), None);
    }

    #[test]
    fn neovim_notation() {
        assert_eq!(p("ctrl+space").nvim(), "<C-Space>");
        assert_eq!(p("ctrl+b").nvim(), "<C-b>");
        assert_eq!(p("alt+b").nvim(), "<M-b>");
        assert_eq!(p("alt+1").nvim(), "<M-1>");
        assert_eq!(p("alt+space").nvim(), "<M-Space>");
        assert_eq!(p("q").nvim(), "q");
        assert_eq!(p("f10").nvim(), "<F10>");
    }

    #[test]
    fn the_file_gives_the_keys_and_says_once_what_it_ignored() {
        assert_eq!(Keys::parse("{}"), (Keys::default(), Vec::new()));
        let (k, n) = Keys::parse(r#"{ "back": "alt+x", "quit": "x", "later": "ctrl+y" }"#);
        assert_eq!((k.back, k.quit, n.len()), (p("alt+x"), p("x"), 0), "unknown names are left alone");
        assert_eq!((k.back_label(), k.quit_label()), ("alt+x", "x"));
        // A bad entry: its default, and one notice naming the entry and the default.
        let (k, n) = Keys::parse(r#"{ "back": "x", "quit": "w" }"#);
        assert_eq!((k.back, k.quit), (p("ctrl+space"), p("w")));
        assert_eq!(n, vec!["keys.json: \"back\" is \"x\", which would be typed rather than go back (it needs ctrl or alt). Using ctrl+space.".to_string()]);
        let (k, n) = Keys::parse(r#"{ "back": 7 }"#);
        assert_eq!((k.back, n.len()), (p("ctrl+space"), 1));
        assert!(n[0].contains("\"back\"") && n[0].contains("ctrl+space"));
        let (k, n) = Keys::parse(r#"{ "back": "ctrl+j" }"#);
        assert_eq!(k.back, p("ctrl+space"));
        assert!(n[0].contains("enter"), "{n:?}");
        // Warned but kept.
        let (k, n) = Keys::parse(r#"{ "back": "ctrl+c" }"#);
        assert_eq!(k.back, p("ctrl+c"));
        assert!(n[0].contains("Claude Code uses ctrl+c"), "{n:?}");
        // Quit can't be the back key.
        let (k, n) = Keys::parse(r#"{ "back": "alt+q", "quit": "Alt+Q" }"#);
        assert_eq!((k.back, k.quit), (p("alt+q"), p("q")));
        assert!(n[0].contains("\"quit\"") && n[0].contains("back key") && n[0].contains("Using q"), "{n:?}");
        // Not JSON, or not an object: the defaults, said.
        for bad in ["{ back: ", "[1]", "\"ctrl+b\""] {
            let (k, n) = Keys::parse(bad);
            assert_eq!((k, n.len()), (Keys::default(), 1), "{bad}");
        }
    }

    #[test]
    fn the_store_reads_edits_and_says_each_problem_once() {
        let d = dir("store");
        let mut s = KeysStore::open(&d);
        assert_eq!(*s.get(), Keys::default(), "no file: the defaults");
        assert_eq!(s.take_notice(), None);
        assert!(!s.refresh());
        fs::write(path_in(&d), r#"{ "back": "alt+x" }"#).unwrap();
        assert!(s.refresh());
        assert_eq!(s.get().back, p("alt+x"));
        assert_eq!(s.take_notice(), None);
        // A bad entry: said once, even when the file is read again with the same problem.
        crate::projects::write_atomic(&path_in(&d), r#"{ "back": "x", "quit": "z" }"#).unwrap();
        assert!(s.refresh());
        assert_eq!(s.get().back, p("ctrl+space"));
        assert_eq!(s.get().quit, p("z"));
        let said = s.take_notice().expect("a notice");
        assert!(said.contains("\"back\"") && said.contains("Using ctrl+space"), "{said}");
        assert_eq!(s.take_notice(), None);
        crate::projects::write_atomic(&path_in(&d), r#"{ "back": "x", "quit": "y" }"#).unwrap();
        assert!(s.refresh());
        assert_eq!(s.get().quit, p("y"));
        assert_eq!(s.take_notice(), None, "the same problem isn't said twice");
        // Fixed, then broken again: said again.
        crate::projects::write_atomic(&path_in(&d), r#"{ }"#).unwrap();
        assert!(s.refresh());
        crate::projects::write_atomic(&path_in(&d), r#"{ "back": "x" }"#).unwrap();
        assert!(s.refresh());
        assert!(s.take_notice().is_some());
        // platoon never writes the file.
        assert_eq!(fs::read_to_string(path_in(&d)).unwrap(), r#"{ "back": "x" }"#);
        // Gone: the defaults again.
        fs::remove_file(path_in(&d)).unwrap();
        assert!(s.refresh());
        assert_eq!(*s.get(), Keys::default());
    }
}
