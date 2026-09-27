//! Putting selected text where the user can paste it. Two independent routes, both tried:
//! the system clipboard (through the `arboard` crate: no `xclip` or `wl-copy` needed), and OSC 52, an escape
//! sequence that asks the terminal to set its own clipboard (works over SSH, and in Ghostty, kitty, WezTerm
//! and iTerm2; terminals that ignore it just do nothing). Which routes run is `PLATOON_CLIPBOARD`.
//!
//! What actually writes is behind `ClipboardSink`, so tests use a fake and never touch the real clipboard
//! or the real terminal.

use std::io::{self, Write};

/// The most text that is copied. A selection can only be as big as the screen plus scrollback, so this is
/// far above anything real; it is a guard, and the notice says when it cut something.
pub const MAX_COPY_BYTES: usize = 512 * 1024;
/// The largest OSC 52 payload (base64 characters) sent. Terminals cap these differently (kitty and xterm
/// accept a lot, others a fraction of it) and a sequence that is too long can be printed as garbage, so
/// bigger texts go to the system clipboard only.
pub const MAX_OSC52_PAYLOAD: usize = 100_000;

/// Which routes to use, from `PLATOON_CLIPBOARD`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    /// The system clipboard (and the primary selection where there is one), plus OSC 52. The default.
    System,
    /// OSC 52 only.
    Osc52,
    /// Nothing is copied; a selection is still highlighted and reported.
    Off,
}

impl Target {
    pub fn from_env() -> Self {
        Self::parse(std::env::var("PLATOON_CLIPBOARD").ok().as_deref())
    }

    /// An unknown value is the default rather than an error: nothing here is worth refusing to start over.
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("osc52") => Self::Osc52,
            Some("none" | "off") => Self::Off,
            _ => Self::System,
        }
    }
}

/// What actually writes the text.
pub trait ClipboardSink {
    /// The system clipboard, and the primary selection too where there is one (middle-click paste).
    fn set_system(&mut self, text: &str) -> Result<(), String>;
    /// Bytes for the terminal, flushed at once.
    fn write_terminal(&mut self, bytes: &[u8]) -> io::Result<()>;
}

/// The real thing. One `arboard::Clipboard` lives as long as this does, because on X11 (and Wayland) the
/// program that set the clipboard is the one that hands the text to whoever pastes; drop it and the text is
/// gone, unless a clipboard manager took a copy. It is created on the first copy, so a machine with no
/// display costs nothing until then (and is retried on the next copy).
#[derive(Default)]
pub struct SystemSink {
    board: Option<arboard::Clipboard>,
}

impl ClipboardSink for SystemSink {
    fn set_system(&mut self, text: &str) -> Result<(), String> {
        if self.board.is_none() {
            self.board = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
        }
        let board = self.board.as_mut().expect("just created");
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            use arboard::{LinuxClipboardKind, SetExtLinux};
            let result = board.set().clipboard(LinuxClipboardKind::Clipboard).text(text.to_owned());
            // Middle-click paste. Best effort: not every Wayland compositor has a primary selection.
            let _ = board.set().clipboard(LinuxClipboardKind::Primary).text(text.to_owned());
            if let Err(e) = result {
                self.board = None; // maybe the connection went away: start afresh next time
                return Err(e.to_string());
            }
            Ok(())
        }
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        {
            let result = board.set_text(text.to_owned());
            if let Err(e) = result {
                self.board = None;
                return Err(e.to_string());
            }
            Ok(())
        }
    }

    fn write_terminal(&mut self, bytes: &[u8]) -> io::Result<()> {
        let mut out = io::stdout().lock();
        out.write_all(bytes)?;
        out.flush()
    }
}

/// How one route went.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Step {
    /// Not part of what `PLATOON_CLIPBOARD` asked for.
    Skipped,
    Done,
    Failed(String),
    /// Longer than `MAX_OSC52_PAYLOAD`.
    TooBig,
}

/// What a copy did, for the notice.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Outcome {
    pub target: Target,
    pub system: Step,
    pub osc52: Step,
    /// The text was longer than `MAX_COPY_BYTES` and was cut.
    pub truncated: bool,
    pub lines: usize,
    pub chars: usize,
}

impl Outcome {
    /// Something took the text. (OSC 52 counts once it was written: a terminal doesn't say whether it obeyed.)
    pub fn copied(&self) -> bool {
        self.system == Step::Done || self.osc52 == Step::Done
    }

    /// The one line for the status bar.
    pub fn notice(&self) -> String {
        let what = if self.lines > 1 {
            format!("{} lines", self.lines)
        } else if self.chars == 1 {
            "1 character".to_string()
        } else {
            format!("{} characters", self.chars)
        };
        if self.target == Target::Off {
            return format!("Selected {what} (clipboard off)");
        }
        if !self.copied() {
            // The reason (no display, a closed terminal, a text too big for OSC 52) is in `system` and
            // `osc52` for whoever wants it; the status bar has room for what to do about it, not why.
            return "Copy failed: hold Shift and drag to select natively".to_string();
        }
        let mut notes: Vec<&str> = Vec::new();
        if self.truncated {
            notes.push("cut to 512 KiB");
        }
        if self.osc52 == Step::TooBig {
            notes.push("too big for the terminal's clipboard");
        }
        if matches!(self.system, Step::Failed(_)) {
            notes.push("terminal only");
        }
        if notes.is_empty() { format!("Copied {what}") } else { format!("Copied {what} ({})", notes.join(", ")) }
    }
}

pub struct Clipboard {
    target: Target,
    sink: Box<dyn ClipboardSink>,
}

impl Clipboard {
    pub fn new(target: Target, sink: Box<dyn ClipboardSink>) -> Self {
        Self { target, sink }
    }

    /// The real clipboard, as `PLATOON_CLIPBOARD` says.
    pub fn from_env() -> Self {
        Self::new(Target::from_env(), Box::new(SystemSink::default()))
    }

    /// Which routes this clipboard uses (the code editor copies the same way).
    pub fn target(&self) -> Target {
        self.target
    }

    /// A clipboard that copies nothing anywhere: what an `App` has until it is given a real one.
    pub fn off() -> Self {
        struct Nowhere;
        impl ClipboardSink for Nowhere {
            fn set_system(&mut self, _: &str) -> Result<(), String> {
                Err("no clipboard".into())
            }
            fn write_terminal(&mut self, _: &[u8]) -> io::Result<()> {
                Ok(())
            }
        }
        Self::new(Target::Off, Box::new(Nowhere))
    }

    pub fn copy(&mut self, text: &str) -> Outcome {
        let (text, truncated) = truncate(text, MAX_COPY_BYTES);
        let mut out = Outcome {
            target: self.target,
            system: Step::Skipped,
            osc52: Step::Skipped,
            truncated,
            lines: text.split('\n').count(),
            chars: text.chars().count(),
        };
        if self.target == Target::Off {
            return out;
        }
        if self.target == Target::System {
            out.system = match self.sink.set_system(text) {
                Ok(()) => Step::Done,
                Err(e) => Step::Failed(e),
            };
        }
        let payload = base64(text.as_bytes());
        out.osc52 = if payload.len() > MAX_OSC52_PAYLOAD {
            Step::TooBig
        } else {
            match self.sink.write_terminal(osc52(&payload).as_bytes()) {
                Ok(()) => Step::Done,
                Err(e) => Step::Failed(e.to_string()),
            }
        };
        out
    }
}

// ---- `platoon clipboard-hold`: the code editor's way to the system clipboard ---------------------------------------

/// The hidden command the code editor runs for each copy, with the text on its input.
pub const HOLD_COMMAND: &str = "clipboard-hold";

/// The longest a `clipboard-hold` stays in the background holding a copy nothing else has replaced.
const HOLD_FOR: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// `platoon clipboard-hold`: put the text on its input on the system clipboard (and the primary selection), then, on
/// X11 and Wayland, stay in the background holding it until something else is copied (at most `HOLD_FOR`), because
/// there the program that copied is the one that hands the text over when it is pasted (what `xclip` does). Started
/// by the code editor (`src/editor/init.lua`) only when `PLATOON_CLIPBOARD` is `system`; it prints nothing, and its
/// exit code is all it says (0 copied, 1 not).
pub fn run_hold() -> i32 {
    use std::io::Read;
    let mut text = String::new();
    if io::stdin().read_to_string(&mut text).is_err() || text.is_empty() {
        return 1;
    }
    let (text, _) = truncate(&text, MAX_COPY_BYTES);
    let text = text.to_owned();
    background();
    match hold(&text) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// Carry on without the editor: a session of its own (a closing terminal doesn't end it) and nothing of the
/// terminal's kept open.
fn background() {
    // SAFETY: plain system calls on this process's own descriptors; setsid fails harmlessly when already a leader.
    unsafe {
        libc::setsid();
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
        if null >= 0 {
            for fd in 0..3 {
                libc::dup2(null, fd);
            }
            if null > 2 {
                libc::close(null);
            }
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn hold(text: &str) -> Result<(), String> {
    use arboard::{LinuxClipboardKind, SetExtLinux};
    let mut board = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    let _ = board.set().clipboard(LinuxClipboardKind::Primary).text(text.to_owned());
    board.set().clipboard(LinuxClipboardKind::Clipboard).wait_until(std::time::Instant::now() + HOLD_FOR).text(text.to_owned()).map_err(|e| e.to_string())
}

/// (macOS keeps what was copied after the program has gone: nothing to wait for.)
#[cfg(not(all(unix, not(target_os = "macos"))))]
fn hold(text: &str) -> Result<(), String> {
    let _ = HOLD_FOR;
    arboard::Clipboard::new().and_then(|mut b| b.set_text(text.to_owned())).map_err(|e| e.to_string())
}

/// The sequence that sets the terminal's clipboard (`c`) to base64 `payload`.
pub fn osc52(payload: &str) -> String {
    format!("\x1b]52;c;{payload}\x07")
}

/// At most `max` bytes of `text`, cut on a character boundary.
fn truncate(text: &str, max: usize) -> (&str, bool) {
    if text.len() <= max {
        return (text, false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

/// Standard base64 with padding (RFC 4648), which is what OSC 52 carries. Too small to be worth a crate.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
pub mod fake {
    //! A sink that records what it was asked to do.
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    pub struct Record {
        pub system: Vec<String>,
        /// Everything written to the "terminal", in order.
        pub terminal: Vec<u8>,
    }

    pub struct Fake {
        pub record: Arc<Mutex<Record>>,
        pub system_error: Option<String>,
        pub terminal_error: bool,
    }

    impl Fake {
        pub fn new() -> (Self, Arc<Mutex<Record>>) {
            let record = Arc::new(Mutex::new(Record::default()));
            (Self { record: record.clone(), system_error: None, terminal_error: false }, record)
        }
    }

    impl ClipboardSink for Fake {
        fn set_system(&mut self, text: &str) -> Result<(), String> {
            if let Some(e) = &self.system_error {
                return Err(e.clone());
            }
            self.record.lock().unwrap().system.push(text.to_string());
            Ok(())
        }
        fn write_terminal(&mut self, bytes: &[u8]) -> io::Result<()> {
            if self.terminal_error {
                return Err(io::Error::other("closed"));
            }
            self.record.lock().unwrap().terminal.extend_from_slice(bytes);
            Ok(())
        }
    }

    /// The texts of every OSC 52 sequence in `bytes`, decoded.
    pub fn osc52_texts(bytes: &[u8]) -> Vec<String> {
        let s = String::from_utf8_lossy(bytes).to_string();
        s.split("\x1b]52;c;")
            .skip(1)
            .map(|rest| {
                let payload = rest.split('\x07').next().unwrap();
                String::from_utf8(unbase64(payload)).unwrap()
            })
            .collect()
    }

    pub fn unbase64(s: &str) -> Vec<u8> {
        let val = |c: u8| match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => panic!("not base64: {c}"),
        };
        let mut out = Vec::new();
        for chunk in s.trim_end_matches('=').as_bytes().chunks(4) {
            let n = chunk.iter().enumerate().fold(0u32, |n, (i, &c)| n | u32::from(val(c)) << (18 - 6 * i));
            for i in 0..chunk.len() - 1 {
                out.push((n >> (16 - 8 * i)) as u8);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;

    fn board(target: Target) -> (Clipboard, std::sync::Arc<std::sync::Mutex<Record>>) {
        let (fake, record) = Fake::new();
        (Clipboard::new(target, Box::new(fake)), record)
    }

    #[test]
    fn base64_matches_the_rfc_4648_vectors() {
        for (plain, encoded) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64(plain.as_bytes()), encoded);
        }
        assert_eq!(base64(&[0xff, 0xfe, 0xfd, 0xfc]), "//79/A==");
        assert_eq!(base64("héllo wörld ✓ 世界".as_bytes()), "aMOpbGxvIHfDtnJsZCDinJMg5LiW55WM");
    }

    #[test]
    fn base64_round_trips_every_length_and_all_byte_values() {
        let all: Vec<u8> = (0..=255).collect();
        for n in 0..=all.len() {
            assert_eq!(unbase64(&base64(&all[..n])), &all[..n], "{n} bytes");
        }
    }

    #[test]
    fn the_environment_value_picks_the_routes() {
        assert_eq!(Target::parse(None), Target::System);
        assert_eq!(Target::parse(Some("system")), Target::System);
        assert_eq!(Target::parse(Some("osc52")), Target::Osc52);
        assert_eq!(Target::parse(Some(" OSC52 ")), Target::Osc52);
        assert_eq!(Target::parse(Some("none")), Target::Off);
        assert_eq!(Target::parse(Some("off")), Target::Off);
        assert_eq!(Target::parse(Some("carrier pigeon")), Target::System, "an unknown value is the default");
    }

    #[test]
    fn system_mode_sets_the_system_clipboard_and_sends_osc52() {
        let (mut b, rec) = board(Target::System);
        let out = b.copy("héllo\nworld ✓");
        let rec = rec.lock().unwrap();
        assert_eq!(rec.system, ["héllo\nworld ✓"]);
        assert_eq!(rec.terminal, format!("\x1b]52;c;{}\x07", base64("héllo\nworld ✓".as_bytes())).as_bytes());
        assert_eq!(osc52_texts(&rec.terminal), ["héllo\nworld ✓"]);
        assert!(out.copied());
        assert_eq!(out.notice(), "Copied 2 lines");
    }

    #[test]
    fn osc52_mode_never_touches_the_system_clipboard() {
        let (mut b, rec) = board(Target::Osc52);
        let out = b.copy("just this");
        let rec = rec.lock().unwrap();
        assert!(rec.system.is_empty());
        assert_eq!(osc52_texts(&rec.terminal), ["just this"]);
        assert_eq!(out.notice(), "Copied 9 characters");
    }

    #[test]
    fn off_mode_copies_nothing_but_still_says_what_was_selected() {
        let (mut b, rec) = board(Target::Off);
        let out = b.copy("a\nb\nc");
        let rec = rec.lock().unwrap();
        assert!(rec.system.is_empty() && rec.terminal.is_empty());
        assert!(!out.copied());
        assert_eq!(out.notice(), "Selected 3 lines (clipboard off)");
    }

    #[test]
    fn notices_count_lines_and_characters_properly() {
        let (mut b, _) = board(Target::Osc52);
        assert_eq!(b.copy("x").notice(), "Copied 1 character");
        assert_eq!(b.copy("12 chars....").notice(), "Copied 12 characters");
        assert_eq!(b.copy("世界").notice(), "Copied 2 characters", "characters, not bytes");
        assert_eq!(b.copy("one\ntwo\nthree").notice(), "Copied 3 lines");
    }

    #[test]
    fn if_the_system_clipboard_fails_the_terminal_route_still_runs_and_the_notice_says_so() {
        let (mut fake, rec) = Fake::new();
        fake.system_error = Some("no display".into());
        let mut b = Clipboard::new(Target::System, Box::new(fake));
        let out = b.copy("hello");
        assert_eq!(osc52_texts(&rec.lock().unwrap().terminal), ["hello"]);
        assert!(out.copied());
        assert_eq!(out.notice(), "Copied 5 characters (terminal only)");
    }

    #[test]
    fn if_every_route_fails_the_notice_points_to_shift_drag() {
        let (mut fake, rec) = Fake::new();
        fake.system_error = Some("no display".into());
        fake.terminal_error = true;
        let mut b = Clipboard::new(Target::System, Box::new(fake));
        let out = b.copy("hello");
        assert!(!out.copied());
        assert_eq!(out.notice(), "Copy failed: hold Shift and drag to select natively");
        assert!(rec.lock().unwrap().terminal.is_empty());
        // The same when the only route asked for fails.
        let (mut fake, _) = Fake::new();
        fake.terminal_error = true;
        let out = Clipboard::new(Target::Osc52, Box::new(fake)).copy("hello");
        assert_eq!(out.notice(), "Copy failed: hold Shift and drag to select natively");
    }

    #[test]
    fn a_text_too_big_for_osc52_goes_to_the_system_clipboard_only_and_says_so() {
        let big = "x".repeat(MAX_OSC52_PAYLOAD / 4 * 3 + 100);
        let (mut b, rec) = board(Target::System);
        let out = b.copy(&big);
        let rec = rec.lock().unwrap();
        assert_eq!(rec.system, [big.clone()]);
        assert!(rec.terminal.is_empty(), "no oversized sequence is written");
        assert_eq!(out.osc52, Step::TooBig);
        assert!(out.notice().contains("too big for the terminal's clipboard"), "{}", out.notice());
        // Only OSC 52 was asked for: nothing could take it, and the notice says to use the terminal's selection.
        let (mut b, rec) = board(Target::Osc52);
        let out = b.copy(&big);
        assert!(!out.copied() && rec.lock().unwrap().terminal.is_empty());
        assert_eq!(out.notice(), "Copy failed: hold Shift and drag to select natively");
    }

    #[test]
    fn a_text_just_under_the_osc52_limit_is_still_sent() {
        let ok = "y".repeat(MAX_OSC52_PAYLOAD / 4 * 3);
        assert_eq!(base64(ok.as_bytes()).len(), MAX_OSC52_PAYLOAD);
        let (mut b, rec) = board(Target::Osc52);
        assert!(b.copy(&ok).copied());
        assert_eq!(osc52_texts(&rec.lock().unwrap().terminal), [ok]);
    }

    #[test]
    fn a_huge_text_is_cut_at_a_character_boundary_and_the_notice_says_so() {
        let text = "é".repeat(MAX_COPY_BYTES); // two bytes each
        let (mut b, rec) = board(Target::System);
        let out = b.copy(&text);
        let got = &rec.lock().unwrap().system[0];
        assert!(got.len() <= MAX_COPY_BYTES && got.len() >= MAX_COPY_BYTES - 1);
        assert!(got.chars().all(|c| c == 'é'), "no character was split");
        assert!(out.truncated);
        assert!(out.notice().contains("cut to 512 KiB"), "{}", out.notice());
        let (short, cut) = truncate("abc", 3);
        assert_eq!((short, cut), ("abc", false));
    }
}
