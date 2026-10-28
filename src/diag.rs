//! The diagnostics log: what platoon decided and why, and what went wrong, in a small file on this computer
//! (`<cache>/diagnostics.log`) that `platoon report` includes. Nothing is sent anywhere.
//!
//! Two levels. Errors (a panic, a hook whose input couldn't be understood, a host that couldn't start) are always
//! written: the next unknown bug has happened before anyone thought of switching a log on. Decisions (a session kept
//! running and why, what the usage meter chose, each call on the user's plan, a task's state changing) are written only
//! while the setting "Keep a diagnostics log" is on (`Settings::diagnostics_log`, read again every few seconds, so a
//! host started long ago follows a change).
//!
//! What a line can hold is decided here, by type, not by each caller's care: a field's value is a `Safe`, which can be
//! made only from numbers, durations, booleans, `&'static str` (words written in platoon's code), an agent, and a few
//! checked kinds: a session id (`SessionId`: only a UUID's characters), a program's own name (`Program`: never its
//! arguments, never its folder), and an identifier a program chose (`Ident`: one short word, for model ids and
//! versions). A `String` from outside (a prompt, a reply, a command line, a path, an error's text) has no way in: it
//! would have to pass one of those checks, and none of them lets a sentence, a path or a command line through. Paths and
//! project names are never logged at all, so `platoon report --hide-names` has nothing to hide in the log.
//!
//! Every process uses the same small API (`error`, `decision`, `changed`): the app, a session's host, the hooks
//! (inside Claude Code's or Codex's process), the status line, the notes helper. `init` names the process and its
//! folders once; until then (and in unit tests, which never call it) every call does nothing. A line is written with
//! one `write` on a file opened with O_APPEND, so lines of processes writing at once never mix. At `CAP` the file is
//! moved to `diagnostics.log.1` (the one before is dropped), so the two together stay under twice the cap. A log that
//! can't be written is skipped without a word: the caller never fails or waits because of it.

use crate::agents::Agent;
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

/// The log's file in the cache folder.
pub const FILE: &str = "diagnostics.log";
/// The file it is moved to once it reaches `CAP`.
pub const PREVIOUS: &str = "diagnostics.log.1";
/// The size at which the log starts over (the one before kept as `PREVIOUS`).
pub const CAP: u64 = 1024 * 1024;
/// The longest line written; a longer one is cut (no field is that long, but nothing is left to chance).
const LINE_MAX: usize = 400;
/// How long the setting is believed before the settings file is read again.
const SETTING_TTL: Duration = Duration::from_secs(5);

// ---- what a field can hold ----------------------------------------------------------------------------------------

/// A value a line may carry: see the module's notes for why only these.
#[derive(Clone, Debug, PartialEq)]
pub enum Safe {
    Num(i64),
    Bool(bool),
    Dur(Duration),
    /// A word written in platoon's code.
    Word(&'static str),
    Checked(Checked),
}

/// Text that has passed one of this module's checks. Its field is private: nothing outside makes one.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Checked(String);

impl std::fmt::Display for Safe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Safe::Num(n) => write!(f, "{n}"),
            Safe::Bool(b) => write!(f, "{}", if *b { "yes" } else { "no" }),
            Safe::Dur(d) => f.write_str(&duration(*d)),
            Safe::Word(w) => f.write_str(w),
            Safe::Checked(c) => f.write_str(&c.0),
        }
    }
}

/// A duration as a person reads it: `850ms`, `12.3s`, `4m10s`, `2h05m`, `3d04h`.
pub fn duration(d: Duration) -> String {
    let ms = d.as_millis();
    let s = d.as_secs();
    match s {
        _ if ms < 1000 => format!("{ms}ms"),
        0..60 => format!("{:.1}s", d.as_secs_f64()),
        60..3600 => format!("{}m{:02}s", s / 60, s % 60),
        3600..86_400 => format!("{}h{:02}m", s / 3600, s % 3600 / 60),
        _ => format!("{}d{:02}h", s / 86_400, s % 86_400 / 3600),
    }
}

macro_rules! from_number {
    ($($t:ty),*) => {$(
        impl From<$t> for Safe {
            fn from(n: $t) -> Self {
                Safe::Num(i64::try_from(n).unwrap_or(i64::MAX))
            }
        }
    )*};
}
from_number!(u8, u16, u32, u64, usize, i32, i64);

impl From<bool> for Safe {
    fn from(b: bool) -> Self {
        Safe::Bool(b)
    }
}

impl From<Duration> for Safe {
    fn from(d: Duration) -> Self {
        Safe::Dur(d)
    }
}

/// Only a `&'static str`: a word in platoon's own code. (A `&str` borrowed from a `String` doesn't live that long.)
impl From<&'static str> for Safe {
    fn from(w: &'static str) -> Self {
        Safe::Word(w)
    }
}

impl From<Agent> for Safe {
    fn from(a: Agent) -> Self {
        Safe::Word(a.id())
    }
}

/// What kind of failure an I/O error was (`NotFound`, `PermissionDenied`, ...), never its text (which can name a path).
impl From<&io::Error> for Safe {
    fn from(e: &io::Error) -> Self {
        Safe::Checked(Checked(format!("{:?}", e.kind())))
    }
}

impl<T: Into<Safe>> From<Option<T>> for Safe {
    fn from(v: Option<T>) -> Self {
        v.map_or(Safe::Word("-"), Into::into)
    }
}

/// A session's id: only the characters a UUID has (hex digits and `-`), at most 64 of them. A Codex task's
/// `pending:<tag>` or anything else isn't one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionId(Checked);

impl SessionId {
    pub fn new(id: &str) -> Option<SessionId> {
        let ok = !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
        ok.then(|| SessionId(Checked(id.to_string())))
    }
}

impl From<SessionId> for Safe {
    fn from(s: SessionId) -> Self {
        Safe::Checked(s.0)
    }
}

/// The session id `id` when it is one, else `-`.
pub fn session(id: &str) -> Safe {
    SessionId::new(id).into()
}

/// The characters a program's name or an identifier may have.
fn plain_word(w: &str, max: usize) -> bool {
    !w.is_empty() && w.len() <= max && w.chars().all(|c| c.is_ascii_alphanumeric() || "._+-[]".contains(c))
}

/// A program's own name, from its command line: the last part of its first word (`/usr/bin/node ...` is `node`), the
/// `-` of a login shell taken off. Never an argument, never its folder; a name with anything but letters, digits and
/// `._+-` in it, or longer than 32, is no name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program(Checked);

impl Program {
    /// From a command line as `procs::cmdline` gives it (one string with spaces when only `ps` could say).
    pub fn of(argv: &[String]) -> Option<Program> {
        Program::named(argv.first()?.split_whitespace().next()?)
    }

    /// From a word: a program's path or name, or the short form `idle::summary` makes (`npm run dev`: its first word).
    pub fn named(word: &str) -> Option<Program> {
        let first = word.split_whitespace().next()?;
        let name = first.rsplit('/').next().unwrap_or(first).trim_start_matches('-');
        plain_word(name, 32).then(|| Program(Checked(name.to_string())))
    }
}

impl From<Program> for Safe {
    fn from(p: Program) -> Self {
        Safe::Checked(p.0)
    }
}

/// An identifier a program chose: a model's id (`claude-opus-5-5`, `opus[1m]`), a version (`2.1.310`), a terminal's
/// name (`xterm-256color`). One word of at most 48 letters, digits and `._+-[]`; never for something a person typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ident(Checked);

impl Ident {
    pub fn new(word: &str) -> Option<Ident> {
        plain_word(word, 48).then(|| Ident(Checked(word.to_string())))
    }
}

impl From<Ident> for Safe {
    fn from(i: Ident) -> Self {
        Safe::Checked(i.0)
    }
}

/// `word` as an identifier when it is one, else `?`.
pub fn ident(word: &str) -> Safe {
    Ident::new(word).map_or(Safe::Word("?"), Into::into)
}

/// A moment (seconds since the epoch), in local time.
pub fn at(epoch: u64) -> Safe {
    Safe::Checked(Checked(local_time(epoch)))
}

/// The fields of a line: each a name in platoon's code and a safe value.
pub type Fields<'a> = &'a [(&'static str, Safe)];

// ---- the log ------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Always written.
    Error,
    /// Written while the setting is on.
    Decision,
}

impl Level {
    fn word(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Decision => "decision",
        }
    }
}

/// One process's log: where it goes, what this process is, and whether decisions are written.
pub struct Log {
    dir: PathBuf,
    config: PathBuf,
    /// What this process is: `app`, `host`, `hook`, ...
    kind: &'static str,
    pid: u32,
    /// The setting as last read, and when.
    setting: Mutex<Option<(Instant, bool)>>,
    /// What `changed` last wrote for each (area, key): the line's words, without its moving parts.
    last: Mutex<HashMap<(&'static str, u64), String>>,
}

impl Log {
    /// The log of a process of `kind`, kept in `cache`, with platoon's settings in `config`.
    pub fn new(kind: &'static str, cache: &Path, config: &Path) -> Log {
        Log {
            dir: cache.to_path_buf(),
            config: config.to_path_buf(),
            kind,
            pid: std::process::id(),
            setting: Mutex::new(None),
            last: Mutex::new(HashMap::new()),
        }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(FILE)
    }

    /// Whether decisions are written: the setting, read again once it is `SETTING_TTL` old.
    pub fn decisions_on(&self) -> bool {
        let mut s = self.setting.lock().unwrap_or_else(|e| e.into_inner());
        match *s {
            Some((at, on)) if at.elapsed() < SETTING_TTL => on,
            _ => {
                let on = crate::settings::load(&self.config).0.diagnostics_log;
                *s = Some((Instant::now(), on));
                on
            }
        }
    }

    /// Write one line (an error always; a decision when the setting is on).
    pub fn write(&self, level: Level, area: &'static str, event: &'static str, fields: Fields) {
        if level == Level::Decision && !self.decisions_on() {
            return;
        }
        self.append(&self.line(level, area, event, fields, &[]));
    }

    /// A decision, written only when it differs from the last one of the same `area` and `key` (a session's pid, ...)
    /// in this process: `fields` are compared, `moving` (a time that grows, ...) is written but not compared. For what
    /// is decided over and over (each second, each half minute) and is only news when it changes.
    pub fn changed(&self, area: &'static str, key: u64, event: &'static str, fields: Fields, moving: Fields) {
        if !self.decisions_on() {
            return;
        }
        let words = render(event, fields);
        {
            let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
            if last.get(&(area, key)) == Some(&words) {
                return;
            }
            last.insert((area, key), words);
        }
        self.append(&self.line(Level::Decision, area, event, fields, moving));
    }

    fn line(&self, level: Level, area: &'static str, event: &'static str, fields: Fields, moving: Fields) -> String {
        let mut line =
            format!("{} {}[{}] {} {area} {}", local_time(crate::usage::now()), self.kind, self.pid, level.word(), render(event, fields));
        for (k, v) in moving {
            line.push_str(&format!(" {k}={v}"));
        }
        // Nothing here makes a line break or a control character, but a line is a line.
        line.retain(|c| !c.is_control());
        if line.len() > LINE_MAX {
            let mut cut = LINE_MAX;
            while !line.is_char_boundary(cut) {
                cut -= 1;
            }
            line.truncate(cut);
        }
        line.push('\n');
        line
    }

    /// Append one line with one `write`, moving the file aside first once it has reached `CAP`. Every failure is
    /// skipped.
    fn append(&self, line: &str) {
        let path = self.path();
        let open = || OpenOptions::new().create(true).append(true).mode(0o600).open(&path);
        let Ok(mut file) = open().or_else(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                fs::create_dir_all(&self.dir)?;
            }
            open()
        }) else {
            return;
        };
        let full = file.metadata().is_ok_and(|m| m.len() >= CAP);
        if full {
            // Only the file this process has open is moved: another process may have moved it already, and the one
            // at the path now is a new one.
            let same = fs::metadata(&path).ok().zip(file.metadata().ok()).is_some_and(|(a, b)| a.ino() == b.ino() && a.dev() == b.dev());
            if same {
                let _ = fs::rename(&path, self.dir.join(PREVIOUS));
            }
            match open() {
                Ok(f) => file = f,
                Err(_) => return,
            }
        }
        let _ = file.write(line.as_bytes());
    }
}

/// `event k=v k=v`.
fn render(event: &'static str, fields: Fields) -> String {
    let mut s = event.to_string();
    for (k, v) in fields {
        s.push_str(&format!(" {k}={v}"));
    }
    s
}

/// A moment (seconds since the epoch) in local time with its offset from UTC: `2026-10-28T14:03:05-04:00`.
pub fn local_time(epoch: u64) -> String {
    #[allow(deprecated)]
    let t = epoch as libc::time_t;
    // SAFETY: `localtime_r` writes only into the `tm` we hand it.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return format!("@{epoch}");
        }
        tm
    };
    let off = tm.tm_gmtoff;
    let (sign, off) = if off < 0 { ('-', -off) } else { ('+', off) };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{sign}{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
        off / 3600,
        off % 3600 / 60
    )
}

// ---- this process's log -------------------------------------------------------------------------------------------

static LOG: OnceLock<Log> = OnceLock::new();

/// Name this process and its folders, once, early (`main`). Before it, and in unit tests, nothing is written.
pub fn init(kind: &'static str, cache: &Path, config: &Path) {
    let _ = LOG.set(Log::new(kind, cache, config));
}

/// An error: always written.
pub fn error(area: &'static str, event: &'static str, fields: Fields) {
    if let Some(log) = LOG.get() {
        log.write(Level::Error, area, event, fields);
    }
}

/// A decision: written while the setting is on.
pub fn decision(area: &'static str, event: &'static str, fields: Fields) {
    if let Some(log) = LOG.get() {
        log.write(Level::Decision, area, event, fields);
    }
}

/// A decision written only when it changed (see `Log::changed`).
pub fn changed(area: &'static str, key: u64, event: &'static str, fields: Fields, moving: Fields) {
    if let Some(log) = LOG.get() {
        log.changed(area, key, event, fields, moving);
    }
}

// ---- panics -------------------------------------------------------------------------------------------------------

/// The beginnings of the panic messages Rust itself writes, which say what went wrong without anything of the data. A
/// panic message can carry anything (a slice's text, an error's text, an `expect`'s words built from data), so only
/// these beginnings are kept; where it was (`location`) says the rest.
const PANIC_KINDS: &[&str] = &[
    "index out of bounds",
    "byte index",
    "begin <= end",
    "range start index",
    "range end index",
    "slice index starts",
    "attempt to add with overflow",
    "attempt to subtract with overflow",
    "attempt to multiply with overflow",
    "attempt to divide by zero",
    "attempt to calculate the remainder with a divisor of zero",
    "attempt to negate with overflow",
    "attempt to shift left with overflow",
    "attempt to shift right with overflow",
    "called `Option::unwrap()` on a `None` value",
    "called `Result::unwrap()` on an `Err` value",
    "called `Result::unwrap_err()` on an `Ok` value",
    "already borrowed",
    "already mutably borrowed",
    "capacity overflow",
    "assertion failed",
    "assertion `left == right` failed",
    "assertion `left != right` failed",
    "internal error: entered unreachable code",
    "not implemented",
    "not yet implemented",
    "explicit panic",
    "removal index",
    "insertion index",
    "swap_remove index",
    "failed to spawn thread",
    "a unit test reached the user's real folder",
];

/// What a panic message is, as far as `PANIC_KINDS` can say without its data.
pub fn panic_kind(message: &str) -> &'static str {
    PANIC_KINDS.iter().find(|k| message.starts_with(*k)).copied().unwrap_or("(message not recorded)")
}

/// Where a panic was: its source file from its `src` folder on, after the crate's own folder when that is a published
/// crate's (`ratatui-0.30.2/src/buffer/buffer.rs`) or Rust's (`core/src/str/mod.rs`), so never the folder it was built
/// in, which can hold a person's name; and its line. `?` when that can't be said plainly.
pub fn panic_place(file: &str, line: u32) -> Safe {
    let parts: Vec<&str> = file.split('/').filter(|p| !p.is_empty()).collect();
    let crate_dir = |p: &str| {
        matches!(p, "core" | "alloc" | "std") || p.rsplit_once('-').is_some_and(|(_, v)| v.starts_with(|c: char| c.is_ascii_digit()))
    };
    let from = match parts.iter().rposition(|p| *p == "src") {
        Some(i) if i > 0 && crate_dir(parts[i - 1]) => i - 1,
        Some(i) => i,
        None => parts.len().saturating_sub(1),
    };
    let tail = &parts[from..];
    if tail.is_empty() || !tail.iter().all(|p| plain_word(p, 64)) {
        return Safe::Word("?");
    }
    Safe::Checked(Checked(format!("{}:{line}", tail.join("/"))))
}

/// Record every panic of this process (where, and what kind: `panic_kind`), then do what was done before.
pub fn catch_panics() {
    let before = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let message =
            payload.downcast_ref::<&str>().copied().or_else(|| payload.downcast_ref::<String>().map(String::as_str)).unwrap_or("");
        let at = info.location().map_or(Safe::Word("?"), |l| panic_place(l.file(), l.line()));
        error("panic", "panicked", &[("at", at), ("what", panic_kind(message).into())]);
        before(info);
    }));
}

// ---- reading it back ----------------------------------------------------------------------------------------------

/// The last `n` lines of the log in `cache` (the file before it first), each as it was written. A line that isn't
/// plain printable text, or is longer than any line written, is left out: only what the log itself wrote comes back.
pub fn tail(cache: &Path, n: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for f in [PREVIOUS, FILE] {
        if let Ok(text) = fs::read(cache.join(f)) {
            let text = String::from_utf8_lossy(&text);
            lines.extend(
                text.lines()
                    .filter(|l| !l.is_empty() && l.len() <= LINE_MAX && l.chars().all(|c| c.is_ascii_graphic() || c == ' '))
                    .map(str::to_string),
            );
        }
    }
    let skip = lines.len().saturating_sub(n);
    lines.split_off(skip)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn dir(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-diag-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn with_setting(d: &Path, on: bool) -> Log {
        let config = d.join("config");
        let s = crate::settings::Settings { diagnostics_log: on, ..Default::default() };
        crate::settings::save(&config, &s).unwrap();
        Log::new("test", &d.join("cache"), &config)
    }

    fn text(d: &Path) -> String {
        fs::read_to_string(d.join("cache").join(FILE)).unwrap_or_default()
    }

    #[test]
    fn a_line_is_the_time_the_process_the_level_the_area_and_the_fields() {
        let d = dir("line");
        let log = with_setting(&d, true);
        log.write(
            Level::Decision,
            "idle",
            "kept",
            &[
                ("session", session("0b5c2a3e-1f00-4d2b-9a77-1234567890ab")),
                ("why", "command".into()),
                ("program", Program::of(&["/usr/bin/npm run dev --token sk-secret".into()]).into()),
                ("quiet", Duration::from_secs(7500).into()),
                ("windows", 0u32.into()),
            ],
        );
        let t = text(&d);
        let line = t.lines().next().unwrap();
        let (time, rest) = line.split_once(' ').unwrap();
        assert!(time.len() == 25 && time.as_bytes()[10] == b'T', "{time}");
        assert_eq!(
            rest,
            format!(
                "test[{}] decision idle kept session=0b5c2a3e-1f00-4d2b-9a77-1234567890ab why=command program=npm quiet=2h05m windows=0",
                std::process::id()
            )
        );
        assert!(!t.contains("secret") && !t.contains("run dev"), "a program is its name only: {t}");
    }

    #[test]
    fn only_checked_text_gets_in() {
        assert_eq!(SessionId::new("not a session; rm -rf ~"), None);
        assert_eq!(SessionId::new("pending:p1-3"), None);
        assert!(SessionId::new("019a0b2c-7d8e-7f00-8123-4567890abcde").is_some());
        assert_eq!(Program::named("/home/someone/bin/deploy.sh"), Program::named("deploy.sh"));
        assert_eq!(Program::named("node"), Program::of(&["node".into(), "/opt/mcp-gitlab".into(), "--token".into(), "x".into()]));
        assert_eq!(Program::named("weird$(name)"), None);
        assert_eq!(Program::named("-zsh").map(Safe::from), Some(Safe::Checked(Checked("zsh".into()))));
        assert_eq!(Ident::new("claude-opus-5-5").map(Safe::from).map(|s| s.to_string()).as_deref(), Some("claude-opus-5-5"));
        assert!(Ident::new("opus[1m]").is_some() && Ident::new("2.1.310").is_some());
        for bad in ["two words", "a/path", "", "key=value", "x".repeat(49).as_str(), "naïve"] {
            assert_eq!(Ident::new(bad), None, "{bad:?}");
        }
        assert_eq!(ident("fix the login page").to_string(), "?");
        let e = io::Error::new(io::ErrorKind::PermissionDenied, "couldn't open /home/someone/secret-project/x");
        assert_eq!(Safe::from(&e).to_string(), "PermissionDenied", "an error's kind, never its text");
        assert_eq!(Safe::from(None::<u32>).to_string(), "-");
        assert_eq!(Safe::from(true).to_string(), "yes");
    }

    #[test]
    fn durations_read_as_a_person_says_them() {
        let d = |ms: u64| duration(Duration::from_millis(ms));
        assert_eq!(
            [d(5), d(999), d(1_000), d(12_340), d(250_000), d(3_600_000), d(7_500_000), d(90_000_000)],
            ["5ms", "999ms", "1.0s", "12.3s", "4m10s", "1h00m", "2h05m", "1d01h"]
        );
    }

    #[test]
    fn with_the_setting_off_only_errors_are_written_and_on_decisions_too() {
        let d = dir("levels");
        let off = with_setting(&d, false);
        off.write(Level::Decision, "meter", "chose", &[("pct", 7u8.into())]);
        off.changed("meter", 0, "chose", &[("pct", 7u8.into())], &[]);
        off.write(Level::Error, "hook", "input-not-understood", &[("hook", "session-start".into())]);
        let t = text(&d);
        assert_eq!(t.lines().count(), 1, "{t}");
        assert!(t.contains(" error hook input-not-understood hook=session-start"), "{t}");

        let on = with_setting(&d, true);
        on.write(Level::Decision, "meter", "chose", &[("pct", 7u8.into())]);
        assert!(text(&d).contains(" decision meter chose pct=7"), "{}", text(&d));
    }

    #[test]
    fn the_setting_is_read_again_after_a_few_seconds_not_on_every_line() {
        let d = dir("ttl");
        let log = with_setting(&d, false);
        assert!(!log.decisions_on());
        crate::settings::save(&d.join("config"), &crate::settings::Settings { diagnostics_log: true, ..Default::default() }).unwrap();
        assert!(!log.decisions_on(), "believed for a while");
        *log.setting.lock().unwrap() = Some((Instant::now() - SETTING_TTL, false));
        assert!(log.decisions_on(), "then read again");
    }

    #[test]
    fn a_change_is_written_once_and_a_moving_figure_alone_is_no_change() {
        let d = dir("changed");
        let log = with_setting(&d, true);
        for quiet in [10u64, 20, 30] {
            log.changed("idle", 42, "kept", &[("why", "window".into())], &[("quiet", Duration::from_secs(quiet).into())]);
        }
        log.changed("idle", 43, "kept", &[("why", "window".into())], &[]);
        log.changed("idle", 42, "kept", &[("why", "not-idle".into())], &[]);
        log.changed("idle", 42, "kept", &[("why", "window".into())], &[]);
        let t = text(&d);
        assert_eq!(t.lines().count(), 4, "{t}");
        assert!(t.lines().next().unwrap().ends_with("kept why=window quiet=10.0s"), "{t}");
    }

    #[test]
    fn at_its_cap_the_log_starts_over_and_keeps_one_file_before_it() {
        let d = dir("rotate");
        let log = with_setting(&d, true);
        let cache = d.join("cache");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join(FILE), "x".repeat(CAP as usize)).unwrap();
        fs::write(cache.join(PREVIOUS), "the oldest\n").unwrap();
        log.write(Level::Error, "hook", "failed", &[]);
        assert_eq!(fs::metadata(cache.join(PREVIOUS)).unwrap().len(), CAP, "the full one moved aside, the oldest dropped");
        let t = text(&d);
        assert!(t.ends_with(" error hook failed\n") && t.lines().count() == 1, "{t}");
        // However much is written, the two files stay under twice the cap.
        for i in 0..30_000u32 {
            log.write(Level::Error, "hook", "failed", &[("n", i.into())]);
        }
        let total = fs::metadata(cache.join(FILE)).unwrap().len() + fs::metadata(cache.join(PREVIOUS)).unwrap().len();
        assert!(total <= 2 * CAP + LINE_MAX as u64, "{total}");
    }

    #[test]
    fn writers_at_once_leave_whole_lines_even_across_the_cap() {
        let d = dir("together");
        let cache = d.join("cache");
        fs::create_dir_all(&cache).unwrap();
        // Each thread is its own writer with its own open file, as separate processes are.
        let handles: Vec<_> = (0..8)
            .map(|w| {
                let (cache, config) = (cache.clone(), d.join("config"));
                std::thread::spawn(move || {
                    let log = Log::new("test", &cache, &config);
                    for i in 0..2000u32 {
                        log.write(
                            Level::Error,
                            "test",
                            "line",
                            &[("writer", w.into()), ("n", i.into()), ("pad", "abcdefghijklmnopqrstuvwxyz".into())],
                        );
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let mut seen = 0;
        for f in [PREVIOUS, FILE] {
            for l in fs::read_to_string(cache.join(f)).unwrap_or_default().lines() {
                assert!(l.contains(" error test line writer=") && l.ends_with(" pad=abcdefghijklmnopqrstuvwxyz"), "a broken line: {l:?}");
                seen += 1;
            }
        }
        assert!(seen > 0);
    }

    #[test]
    fn a_log_that_cannot_be_written_is_skipped_without_failing_or_waiting() {
        let d = dir("readonly");
        let cache = d.join("cache");
        fs::create_dir_all(&cache).unwrap();
        fs::set_permissions(&cache, fs::Permissions::from_mode(0o500)).unwrap();
        let log = Log::new("test", &cache, &d.join("config"));
        let start = Instant::now();
        for _ in 0..100 {
            log.write(Level::Error, "hook", "failed", &[]);
        }
        assert!(start.elapsed() < Duration::from_secs(2));
        fs::set_permissions(&cache, fs::Permissions::from_mode(0o700)).unwrap();
        // (As root a read-only folder can be written: then the line is simply there.)
        let _ = fs::read_to_string(cache.join(FILE));
        // A cache that is a file, not a folder: nothing to write into either.
        let file = d.join("a-file");
        fs::write(&file, "").unwrap();
        Log::new("test", &file.join("cache"), &d.join("config")).write(Level::Error, "hook", "failed", &[]);
    }

    #[test]
    fn a_panic_keeps_where_it_was_and_rusts_own_words_never_the_data() {
        assert_eq!(panic_kind("byte index 5 is not a char boundary; it is inside 'é' (bytes 4..6) of `my secret prompt`"), "byte index");
        assert_eq!(
            panic_kind("called `Result::unwrap()` on an `Err` value: Os { code: 2, message: \"/home/x\" }"),
            "called `Result::unwrap()` on an `Err` value"
        );
        assert_eq!(panic_kind("couldn't parse the reply: here is the reply text"), "(message not recorded)");
        assert_eq!(panic_place("src/app/idle_glue.rs", 42).to_string(), "src/app/idle_glue.rs:42");
        assert_eq!(
            panic_place("/home/someone/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ratatui-0.30.2/src/buffer/buffer.rs", 7)
                .to_string(),
            "ratatui-0.30.2/src/buffer/buffer.rs:7"
        );
        assert_eq!(panic_place("/home/someone/work/thing/src/main.rs", 3).to_string(), "src/main.rs:3", "never the home folder");
        assert_eq!(panic_place("/Users/Jane Doe/src/x.rs", 1).to_string(), "src/x.rs:1");
        assert_eq!(panic_place("/Users/Jane Doe/x.rs", 1).to_string(), "x.rs:1");
        assert_eq!(panic_place("/rustc/abc123/library/core/src/str/mod.rs", 9).to_string(), "core/src/str/mod.rs:9");
    }

    #[test]
    fn the_tail_is_the_last_lines_of_both_files_and_nothing_that_isnt_a_line_of_the_log() {
        let d = dir("tail");
        let cache = d.join("cache");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join(PREVIOUS), "one\ntwo\n").unwrap();
        fs::write(cache.join(FILE), "three\nfour\twith a tab\nfive\x1b[31m\nsix\n").unwrap();
        assert_eq!(tail(&cache, 10), ["one", "two", "three", "six"]);
        assert_eq!(tail(&cache, 2), ["three", "six"]);
        assert!(tail(&d.join("nothing"), 5).is_empty());
    }

    #[test]
    fn local_time_has_its_offset() {
        let t = local_time(1_793_186_731);
        assert_eq!(t.len(), 25, "{t}");
        assert!(t.starts_with("2026-10-2") && matches!(&t[19..20], "+" | "-"), "{t}");
    }
}
