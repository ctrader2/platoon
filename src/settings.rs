//! platoon's own settings, in `<config dir>/settings.json` next to `archive.json`: the two automatic
//! clean-ups and how idle stop treats MCP servers. The settings screen edits them through `save`, and
//! `ITEMS` describes what it shows.
//!
//! Like the archive file, this one must never take platoon down or be lost: a missing or unreadable file
//! just means the defaults, values that make no sense are clamped, writes are atomic, and a file that
//! can't be understood is copied to `.bak` before it is ever replaced. Everything that uses a setting
//! re-reads the file when it changes (`SettingsStore::refresh`), so an edit applies without restarting
//! anything, including the session hosts that outlive platoon.

use crate::archive::keep_backup;
use serde_json::{json, Map, Value};
use std::{fs, io, os::unix::fs::MetadataExt, path::{Path, PathBuf}, time::{Duration, SystemTime}};

/// A session nobody is looking at is stopped after this long, at most. A week is far past any sane use.
pub const MAX_IDLE_STOP_MINUTES: u64 = 7 * 24 * 60;
/// Longest auto-archive age. Also how long the archive file remembers restores (see archive.rs).
pub const MAX_AUTO_ARCHIVE_DAYS: u64 = 365;

/// For both numbers, 0 means the feature is off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Stop a platoon-hosted session with no viewer, not working and silent for this many minutes.
    pub idle_stop_minutes: u64,
    /// Archive a conversation with no running process and no activity for this many days.
    pub auto_archive_days: u64,
    /// Idle stop doesn't count MCP servers as something running under the session. Off: any process does.
    pub idle_stop_ignores_mcp: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { idle_stop_minutes: 120, auto_archive_days: 2, idle_stop_ignores_mcp: true }
    }
}

impl Settings {
    pub fn clamped(self) -> Self {
        Self {
            idle_stop_minutes: self.idle_stop_minutes.min(MAX_IDLE_STOP_MINUTES),
            auto_archive_days: self.auto_archive_days.min(MAX_AUTO_ARCHIVE_DAYS),
            ..self
        }
    }

    /// The idle-stop threshold, or None when it is off. `secs_per_minute` is 60 outside of tests.
    pub fn idle_stop_after(&self, secs_per_minute: u64) -> Option<Duration> {
        (self.idle_stop_minutes > 0).then(|| Duration::from_secs(self.idle_stop_minutes * secs_per_minute))
    }

    /// The auto-archive age in seconds, or None when it is off.
    pub fn auto_archive_secs(&self) -> Option<u64> {
        (self.auto_archive_days > 0).then(|| self.auto_archive_days * 86_400)
    }
}

/// None if the text isn't a JSON object. A key that is missing or isn't a non-negative number (or, for the
/// yes/no one, a boolean) keeps its default (a negative number is nonsense, not "off"), and numbers past the limits are clamped.
fn parse(text: &str) -> Option<Settings> {
    let v: Value = serde_json::from_str(text).ok()?;
    let o = v.as_object()?;
    let d = Settings::default();
    let number = |key: &str, default: u64| match o.get(key) {
        Some(v) => v.as_u64().or_else(|| v.as_f64().filter(|f| f.is_finite() && *f >= 0.0).map(|f| f as u64)).unwrap_or(default),
        None => default,
    };
    // Only a real true/false counts: anything else (a number, text) is not a choice, so the default stays.
    let ignores_mcp = o.get("idle_stop_ignores_mcp").and_then(Value::as_bool).unwrap_or(d.idle_stop_ignores_mcp);
    Some(
        Settings {
            idle_stop_minutes: number("idle_stop_minutes", d.idle_stop_minutes),
            auto_archive_days: number("auto_archive_days", d.auto_archive_days),
            idle_stop_ignores_mcp: ignores_mcp,
        }
        .clamped(),
    )
}

pub fn path_in(dir: &Path) -> PathBuf {
    dir.join("settings.json")
}

/// The settings on disk. Never fails: whatever can't be read is the defaults. The second value says the
/// file exists but couldn't be understood.
pub fn load(dir: &Path) -> (Settings, bool) {
    match fs::read_to_string(path_in(dir)) {
        Ok(text) => match parse(&text) {
            Some(s) => (s, false),
            None => (Settings::default(), true),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => (Settings::default(), false),
        Err(_) => (Settings::default(), path_in(dir).exists()),
    }
}

/// The top-level object of the file as it is now, if it holds one. `save` starts from it so that keys
/// platoon doesn't know about (a newer version's, or the user's own notes) survive an edit.
fn existing_object(path: &Path) -> Option<Map<String, Value>> {
    match serde_json::from_str(&fs::read_to_string(path).ok()?).ok()? {
        Value::Object(o) => Some(o),
        _ => None,
    }
}

/// Write the settings (clamped) atomically: a temp file, then a rename, so a crash never leaves half a
/// file. Keys other than the settings themselves are kept as they were. A file that was there but couldn't
/// be understood is kept as `settings.json.bak` first, and replaced by a fresh one.
pub fn save(dir: &Path, settings: &Settings) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = path_in(dir);
    let corrupt = load(dir).1;
    if corrupt {
        keep_backup(&path)?;
    }
    let mut object = if corrupt { Map::new() } else { existing_object(&path).unwrap_or_default() };
    let s = settings.clamped();
    object.entry("version").or_insert(json!(1));
    object.insert("idle_stop_minutes".into(), json!(s.idle_stop_minutes));
    object.insert("auto_archive_days".into(), json!(s.auto_archive_days));
    object.insert("idle_stop_ignores_mcp".into(), json!(s.idle_stop_ignores_mcp));
    let text = serde_json::to_string_pretty(&Value::Object(object)).unwrap_or_default();
    let tmp = path.with_extension("json.tmp");
    let written = fs::write(&tmp, text).and_then(|()| fs::rename(&tmp, &path));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// One line of the settings screen. The screen is built from `ITEMS` alone, so a new setting is a new entry
/// here (plus its field in `Settings`, `parse` and `save`).
pub struct Item {
    pub name: &'static str,
    /// One plain-language sentence or two: what it does and why nothing is lost.
    pub explain: &'static str,
    /// The steps offered, ascending, in the setting's own unit. For a number 0 is "Off"; a choice between
    /// two things is 0 and 1.
    pub choices: &'static [u64],
    pub get: fn(&Settings) -> u64,
    pub set: fn(&mut Settings, u64),
    /// How a value reads: "Off", "2 hours". Also used for a value that is not one of the steps.
    pub label: fn(u64) -> String,
    /// The value that means "switched off" and is drawn muted, if there is one.
    pub off: Option<u64>,
}

fn plural(n: u64, unit: &str) -> String {
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

pub fn minutes_label(m: u64) -> String {
    match m {
        0 => "Off".into(),
        m if m % 1440 == 0 => plural(m / 1440, "day"),
        m if m % 60 == 0 => plural(m / 60, "hour"),
        m => plural(m, "minute"),
    }
}

pub fn days_label(d: u64) -> String {
    if d == 0 { "Off".into() } else { plural(d, "day") }
}

pub const ITEMS: &[Item] = &[
    Item {
        name: "Stop idle sessions after",
        explain: "A session platoon is running that nobody has looked at, and that isn't working, is stopped to free memory. \
                  Nothing is lost: opening it again resumes it from its saved history.",
        choices: &[0, 30, 60, 120, 240, 480, 1440],
        get: |s| s.idle_stop_minutes,
        set: |s, v| s.idle_stop_minutes = v,
        label: minutes_label,
        off: Some(0),
    },
    Item {
        name: "Archive conversations after",
        explain: "A conversation nobody has used for this long, with nothing running, moves to the Archived tab. \
                  You can restore it from there.",
        choices: &[0, 1, 2, 7, 14, 30],
        get: |s| s.auto_archive_days,
        set: |s, v| s.auto_archive_days = v,
        label: days_label,
        off: Some(0),
    },
    Item {
        name: "Idle stop and MCP servers",
        explain: "MCP servers are helper programs Claude starts with a session. By default they are ignored, so a session with only \
                  those running is still stopped: they close with it and start again when you open it. Choose \"Count MCP servers\" \
                  and they keep it alive like any other program.",
        choices: &[0, 1],
        get: |s| u64::from(s.idle_stop_ignores_mcp),
        set: |s, v| s.idle_stop_ignores_mcp = v != 0,
        label: |v| if v == 0 { "Count MCP servers".into() } else { "Ignore MCP servers".into() },
        off: None,
    },
];

/// Which way to move through an item's choices.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    Prev,
    Next,
    /// Next, and back to the first after the last.
    Cycle,
}

impl Item {
    /// The choice `step` leads to from `value`, which needn't be one of the choices (a hand-edited file):
    /// it goes to the nearest step in that direction. Prev and Next stop at the ends; Cycle wraps.
    pub fn step(&self, value: u64, step: Step) -> u64 {
        let next = self.choices.iter().copied().find(|&c| c > value);
        let prev = self.choices.iter().rev().copied().find(|&c| c < value);
        match step {
            Step::Prev => prev.unwrap_or(value),
            Step::Next => next.unwrap_or(value),
            Step::Cycle => next.unwrap_or(self.choices[0]),
        }
    }
}

/// (modified time, size, inode): the inode changes on every atomic save, so an edit made within the file
/// system's timestamp granularity of the last one is still noticed.
type Stamp = (SystemTime, u64, u64);

/// The settings plus what is needed to notice that the file changed.
pub struct SettingsStore {
    dir: PathBuf,
    stamp: Option<Stamp>,
    settings: Settings,
    corrupt: bool,
}

impl SettingsStore {
    pub fn open(dir: &Path) -> Self {
        let mut s = Self { dir: dir.to_path_buf(), stamp: None, settings: Settings::default(), corrupt: false };
        s.reload();
        s
    }

    pub fn get(&self) -> Settings {
        self.settings
    }

    /// The file exists but couldn't be understood, so `get` is the defaults.
    pub fn is_corrupt(&self) -> bool {
        self.corrupt
    }

    pub fn path(&self) -> PathBuf {
        path_in(&self.dir)
    }

    /// Save, then read the file back either way, so what `get` says is what is on disk (after a failed
    /// save that is the old value) and the change counts as seen: nothing waits for a polling interval.
    pub fn update(&mut self, settings: &Settings) -> io::Result<()> {
        let saved = save(&self.dir, settings);
        self.reload();
        saved
    }

    fn stamp(&self) -> Option<Stamp> {
        fs::metadata(path_in(&self.dir)).ok().and_then(|m| Some((m.modified().ok()?, m.len(), m.ino())))
    }

    fn reload(&mut self) {
        self.stamp = self.stamp();
        (self.settings, self.corrupt) = load(&self.dir);
    }

    /// Pick up an edit. One `stat` when nothing changed. True when the file was re-read.
    pub fn refresh(&mut self) -> bool {
        if self.stamp() == self.stamp {
            return false;
        }
        self.reload();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("platoon-settings-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn write(d: &Path, text: &str) {
        fs::create_dir_all(d).unwrap();
        fs::write(path_in(d), text).unwrap();
    }

    #[test]
    fn a_missing_file_gives_the_defaults_and_creates_nothing() {
        let d = dir("missing");
        assert_eq!(load(&d), (Settings { idle_stop_minutes: 120, auto_archive_days: 2, ..Settings::default() }, false));
        assert!(!d.exists(), "reading must not create the directory or the file");
    }

    #[test]
    fn values_are_read_and_a_missing_key_keeps_its_default() {
        let d = dir("read");
        write(&d, r#"{"idle_stop_minutes": 30, "auto_archive_days": 7}"#);
        assert_eq!(load(&d).0, Settings { idle_stop_minutes: 30, auto_archive_days: 7, ..Settings::default() });
        write(&d, r#"{"auto_archive_days": 5}"#);
        assert_eq!(load(&d).0, Settings { idle_stop_minutes: 120, auto_archive_days: 5, ..Settings::default() });
        write(&d, "{}");
        assert_eq!(load(&d), (Settings::default(), false), "an empty object is a valid file, not a corrupt one");
    }

    #[test]
    fn zero_means_off() {
        let d = dir("zero");
        write(&d, r#"{"idle_stop_minutes": 0, "auto_archive_days": 0}"#);
        let s = load(&d).0;
        assert_eq!((s.idle_stop_minutes, s.auto_archive_days), (0, 0));
        assert_eq!((s.idle_stop_after(60), s.auto_archive_secs()), (None, None));
        let on = Settings::default();
        assert_eq!((on.idle_stop_after(60), on.auto_archive_secs()), (Some(Duration::from_secs(7200)), Some(172_800)));
    }

    #[test]
    fn silly_values_are_clamped_or_ignored() {
        let d = dir("clamp");
        write(&d, r#"{"idle_stop_minutes": 99999999999, "auto_archive_days": 1e30}"#);
        assert_eq!(load(&d).0, Settings { idle_stop_minutes: MAX_IDLE_STOP_MINUTES, auto_archive_days: MAX_AUTO_ARCHIVE_DAYS, ..Settings::default() });
        write(&d, r#"{"idle_stop_minutes": -5, "auto_archive_days": "soon"}"#);
        assert_eq!(load(&d).0, Settings::default(), "negative numbers and text are nonsense, not 'off'");
        write(&d, r#"{"idle_stop_minutes": 45.9, "auto_archive_days": null}"#);
        assert_eq!(load(&d).0, Settings { idle_stop_minutes: 45, auto_archive_days: 2, ..Settings::default() });
    }

    #[test]
    fn a_corrupt_file_reads_as_defaults_is_left_alone_and_backed_up_before_a_save() {
        let d = dir("corrupt");
        for bad in ["{ not json", "[1,2]", "42", ""] {
            write(&d, bad);
            assert_eq!(load(&d), (Settings::default(), true), "{bad:?}");
            assert_eq!(fs::read_to_string(path_in(&d)).unwrap(), bad, "reading must never modify it");
        }
        write(&d, "{ not json");
        save(&d, &Settings { idle_stop_minutes: 10, auto_archive_days: 3, ..Settings::default() }).unwrap();
        assert_eq!(fs::read_to_string(d.join("settings.json.bak")).unwrap(), "{ not json");
        assert_eq!(load(&d), (Settings { idle_stop_minutes: 10, auto_archive_days: 3, ..Settings::default() }, false));

        // A healthy file is replaced without leaving a backup behind, and a second corruption keeps the first backup.
        save(&d, &Settings::default()).unwrap();
        assert!(!d.join("settings.json.bak.1").exists());
        write(&d, "[]");
        save(&d, &Settings::default()).unwrap();
        assert_eq!(fs::read_to_string(d.join("settings.json.bak")).unwrap(), "{ not json");
        assert_eq!(fs::read_to_string(d.join("settings.json.bak.1")).unwrap(), "[]");
    }

    #[test]
    fn save_is_atomic_creates_the_directory_and_clamps() {
        let d = dir("save");
        save(&d, &Settings { idle_stop_minutes: u64::MAX, auto_archive_days: 0, ..Settings::default() }).unwrap();
        assert!(!d.join("settings.json.tmp").exists(), "the temp file is renamed away");
        assert!(!d.join("settings.json.bak").exists(), "nothing to back up on a first save");
        assert_eq!(load(&d), (Settings { idle_stop_minutes: MAX_IDLE_STOP_MINUTES, auto_archive_days: 0, ..Settings::default() }, false));
        let v: Value = serde_json::from_str(&fs::read_to_string(path_in(&d)).unwrap()).unwrap();
        assert_eq!(v["version"], 1);
    }

    #[test]
    fn the_store_notices_an_edit_without_a_restart() {
        let d = dir("store");
        let mut s = SettingsStore::open(&d);
        assert_eq!(s.get(), Settings::default());
        assert!(!s.refresh(), "nothing changed");
        save(&d, &Settings { idle_stop_minutes: 0, auto_archive_days: 9, ..Settings::default() }).unwrap();
        assert!(s.refresh());
        assert_eq!(s.get(), Settings { idle_stop_minutes: 0, auto_archive_days: 9, ..Settings::default() });
        assert!(!s.refresh());
        write(&d, "garbage");
        assert!(s.refresh() && s.is_corrupt());
        assert_eq!(s.get(), Settings::default());
    }

    #[test]
    fn save_keeps_every_key_it_does_not_know_including_nested_values_and_their_order() {
        let d = dir("keep-keys");
        write(&d, r#"{"note":"mine","idle_stop_minutes":5,"nested":{"a":[1,{"b":null}],"c":"d"},"version":7}"#);
        save(&d, &Settings { idle_stop_minutes: 60, auto_archive_days: 9, ..Settings::default() }).unwrap();
        let text = fs::read_to_string(path_in(&d)).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["note"], "mine");
        assert_eq!(v["nested"], json!({"a":[1,{"b":null}],"c":"d"}));
        assert_eq!(v["version"], 7, "a version written by something else is not rewritten");
        assert_eq!((v["idle_stop_minutes"].as_u64(), v["auto_archive_days"].as_u64()), (Some(60), Some(9)));
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["note", "idle_stop_minutes", "nested", "version", "auto_archive_days", "idle_stop_ignores_mcp"], "the file keeps its shape");
        assert!(!d.join("settings.json.bak").exists(), "a healthy file needs no backup");
    }

    #[test]
    fn a_failed_save_leaves_the_file_and_no_temp_file_behind() {
        let d = dir("fail");
        write(&d, r#"{"idle_stop_minutes":45}"#);
        // The rename target is a directory, so the write succeeds and the rename fails.
        fs::remove_file(path_in(&d)).unwrap();
        fs::create_dir(path_in(&d)).unwrap();
        assert!(save(&d, &Settings::default()).is_err());
        assert!(!d.join("settings.json.tmp").exists());
    }

    #[test]
    fn labels_read_in_the_largest_whole_unit() {
        let l: Vec<String> = [0, 1, 30, 60, 90, 120, 1440, 2880, 1500].map(minutes_label).into();
        assert_eq!(l, ["Off", "1 minute", "30 minutes", "1 hour", "90 minutes", "2 hours", "1 day", "2 days", "25 hours"]);
        assert_eq!([0, 1, 2, 90].map(days_label), ["Off", "1 day", "2 days", "90 days"]);
    }

    #[test]
    fn every_setting_offers_ascending_steps_starting_at_off_within_the_limits() {
        for item in ITEMS {
            assert_eq!(item.choices[0], 0, "{}", item.name);
            assert!(item.choices.windows(2).all(|w| w[0] < w[1]), "{}", item.name);
            assert!(!item.explain.is_empty() && item.explain.len() < 300, "{}", item.name);
            let defaults = Settings::default();
            assert!(item.choices.contains(&(item.get)(&defaults)), "the default is one of the steps for {}", item.name);
            let mut s = defaults;
            (item.set)(&mut s, *item.choices.last().unwrap());
            assert_eq!(s.clamped(), s, "the last step is within the limits for {}", item.name);
        }
    }

    #[test]
    fn stepping_goes_to_the_neighbouring_choice_from_a_step_or_from_between_steps() {
        let idle = &ITEMS[0];
        assert_eq!(idle.step(120, Step::Next), 240);
        assert_eq!(idle.step(120, Step::Prev), 60);
        assert_eq!(idle.step(90, Step::Next), 120, "from a hand-edited value, the nearest step above");
        assert_eq!(idle.step(90, Step::Prev), 60, "and below");
        assert_eq!(idle.step(90, Step::Cycle), 120);
        assert_eq!(idle.step(0, Step::Prev), 0, "the ends hold");
        assert_eq!(idle.step(1440, Step::Next), 1440);
        assert_eq!(idle.step(1440, Step::Cycle), 0, "cycling wraps to Off");
        assert_eq!(idle.step(5000, Step::Cycle), 0);
        assert_eq!(idle.step(5000, Step::Prev), 1440);
        assert_eq!(idle.step(10, Step::Prev), 0);
    }

    #[test]
    fn ignoring_mcp_servers_is_on_by_default_and_only_a_real_boolean_changes_it() {
        let d = dir("mcp-flag");
        assert!(Settings::default().idle_stop_ignores_mcp);
        assert!(load(&d).0.idle_stop_ignores_mcp, "no file");
        write(&d, "{}");
        assert!(load(&d).0.idle_stop_ignores_mcp, "key missing");
        write(&d, r#"{"idle_stop_ignores_mcp": false}"#);
        assert_eq!(load(&d), (Settings { idle_stop_ignores_mcp: false, ..Settings::default() }, false));
        write(&d, r#"{"idle_stop_ignores_mcp": true, "idle_stop_minutes": 30}"#);
        assert_eq!(load(&d).0, Settings { idle_stop_minutes: 30, ..Settings::default() });
        // Wrong type: not a choice, so the default stays (and the rest of the file is still read).
        for odd in ["0", "1", "\"false\"", "\"no\"", "null", "[]", "{}", "-1", "0.0"] {
            write(&d, &format!(r#"{{"idle_stop_ignores_mcp": {odd}, "auto_archive_days": 9}}"#));
            assert_eq!(load(&d), (Settings { auto_archive_days: 9, ..Settings::default() }, false), "{odd}");
        }
        for bad in ["{ nope", "[true]", ""] {
            write(&d, bad);
            assert_eq!(load(&d), (Settings::default(), true), "{bad:?}");
        }
    }

    #[test]
    fn the_mcp_choice_is_saved_as_a_boolean_and_other_keys_survive() {
        let d = dir("mcp-save");
        write(&d, r#"{"mine":[1,{"x":null}],"idle_stop_ignores_mcp":true}"#);
        save(&d, &Settings { idle_stop_ignores_mcp: false, ..Settings::default() }).unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(path_in(&d)).unwrap()).unwrap();
        assert_eq!(v["idle_stop_ignores_mcp"], json!(false), "a JSON boolean, not a number");
        assert_eq!(v["mine"], json!([1, {"x": null}]));
        assert!(!load(&d).0.idle_stop_ignores_mcp);
        let mut s = SettingsStore::open(&d);
        assert!(!s.get().idle_stop_ignores_mcp);
        save(&d, &Settings::default()).unwrap();
        assert!(s.refresh() && s.get().idle_stop_ignores_mcp, "a running host sees the change");
    }

    #[test]
    fn the_two_way_choice_steps_between_its_two_values_and_only_the_numbers_can_be_off() {
        let mcp = &ITEMS[2];
        assert_eq!(mcp.choices, [0, 1]);
        let mut s = Settings::default();
        assert_eq!((mcp.get)(&s), 1);
        assert_eq!((mcp.label)(1), "Ignore MCP servers");
        assert_eq!((mcp.label)(0), "Count MCP servers");
        assert_eq!(mcp.step(1, Step::Prev), 0);
        assert_eq!(mcp.step(0, Step::Prev), 0);
        assert_eq!(mcp.step(0, Step::Next), 1);
        assert_eq!(mcp.step(1, Step::Next), 1, "the ends hold");
        assert_eq!(mcp.step(1, Step::Cycle), 0, "cycling wraps");
        assert_eq!(mcp.step(0, Step::Cycle), 1);
        (mcp.set)(&mut s, 0);
        assert!(!s.idle_stop_ignores_mcp);
        (mcp.set)(&mut s, 1);
        assert!(s.idle_stop_ignores_mcp);
        assert_eq!(mcp.off, None, "neither choice is 'off'");
        assert!(ITEMS[..2].iter().all(|i| i.off == Some(0)));
    }

    #[test]
    fn update_saves_and_shows_what_is_really_on_disk_even_when_the_save_fails() {
        let d = dir("update");
        let mut store = SettingsStore::open(&d);
        store.update(&Settings { idle_stop_minutes: 30, auto_archive_days: 7, ..Settings::default() }).unwrap();
        assert_eq!(store.get(), Settings { idle_stop_minutes: 30, auto_archive_days: 7, ..Settings::default() });
        assert!(!store.refresh(), "our own save is already seen");
        fs::remove_file(path_in(&d)).unwrap();
        fs::create_dir(path_in(&d)).unwrap(); // saving now fails
        assert!(store.update(&Settings { idle_stop_minutes: 240, auto_archive_days: 7, ..Settings::default() }).is_err());
        assert_eq!(store.get().idle_stop_minutes, 120, "the store says what is in effect, not what was asked for");
    }
}
