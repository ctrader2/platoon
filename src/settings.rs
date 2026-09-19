//! platoon's own settings, in `<config dir>/settings.json` next to `archive.json`. Today that is the two
//! automatic clean-ups; the settings screen (later) edits them through `save`.
//!
//! Like the archive file, this one must never take platoon down or be lost: a missing or unreadable file
//! just means the defaults, values that make no sense are clamped, writes are atomic, and a file that
//! can't be understood is copied to `.bak` before it is ever replaced. Everything that uses a setting
//! re-reads the file when it changes (`SettingsStore::refresh`), so an edit applies without restarting
//! anything, including the session hosts that outlive platoon.

use crate::archive::keep_backup;
use serde_json::{json, Value};
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
}

impl Default for Settings {
    fn default() -> Self {
        Self { idle_stop_minutes: 120, auto_archive_days: 2 }
    }
}

impl Settings {
    pub fn clamped(self) -> Self {
        Self {
            idle_stop_minutes: self.idle_stop_minutes.min(MAX_IDLE_STOP_MINUTES),
            auto_archive_days: self.auto_archive_days.min(MAX_AUTO_ARCHIVE_DAYS),
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

/// None if the text isn't a JSON object. A key that is missing or isn't a non-negative number keeps its
/// default (a negative number is nonsense, not "off"), and numbers past the limits are clamped.
fn parse(text: &str) -> Option<Settings> {
    let v: Value = serde_json::from_str(text).ok()?;
    let o = v.as_object()?;
    let d = Settings::default();
    let number = |key: &str, default: u64| match o.get(key) {
        Some(v) => v.as_u64().or_else(|| v.as_f64().filter(|f| f.is_finite() && *f >= 0.0).map(|f| f as u64)).unwrap_or(default),
        None => default,
    };
    Some(Settings { idle_stop_minutes: number("idle_stop_minutes", d.idle_stop_minutes), auto_archive_days: number("auto_archive_days", d.auto_archive_days) }.clamped())
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

/// Write the settings (clamped) atomically: a temp file, then a rename, so a crash never leaves half a
/// file. A file that was there but couldn't be understood is kept as `settings.json.bak` first.
#[allow(dead_code)] // the settings screen (a later step) is its caller
pub fn save(dir: &Path, settings: &Settings) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = path_in(dir);
    if load(dir).1 {
        keep_backup(&path)?;
    }
    let s = settings.clamped();
    let text = serde_json::to_string_pretty(&json!({ "version": 1, "idle_stop_minutes": s.idle_stop_minutes, "auto_archive_days": s.auto_archive_days })).unwrap_or_default();
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, &path)
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

    #[cfg(test)]
    pub fn is_corrupt(&self) -> bool {
        self.corrupt
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
        assert_eq!(load(&d), (Settings { idle_stop_minutes: 120, auto_archive_days: 2 }, false));
        assert!(!d.exists(), "reading must not create the directory or the file");
    }

    #[test]
    fn values_are_read_and_a_missing_key_keeps_its_default() {
        let d = dir("read");
        write(&d, r#"{"idle_stop_minutes": 30, "auto_archive_days": 7}"#);
        assert_eq!(load(&d).0, Settings { idle_stop_minutes: 30, auto_archive_days: 7 });
        write(&d, r#"{"auto_archive_days": 5}"#);
        assert_eq!(load(&d).0, Settings { idle_stop_minutes: 120, auto_archive_days: 5 });
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
        assert_eq!(load(&d).0, Settings { idle_stop_minutes: MAX_IDLE_STOP_MINUTES, auto_archive_days: MAX_AUTO_ARCHIVE_DAYS });
        write(&d, r#"{"idle_stop_minutes": -5, "auto_archive_days": "soon"}"#);
        assert_eq!(load(&d).0, Settings::default(), "negative numbers and text are nonsense, not 'off'");
        write(&d, r#"{"idle_stop_minutes": 45.9, "auto_archive_days": null}"#);
        assert_eq!(load(&d).0, Settings { idle_stop_minutes: 45, auto_archive_days: 2 });
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
        save(&d, &Settings { idle_stop_minutes: 10, auto_archive_days: 3 }).unwrap();
        assert_eq!(fs::read_to_string(d.join("settings.json.bak")).unwrap(), "{ not json");
        assert_eq!(load(&d), (Settings { idle_stop_minutes: 10, auto_archive_days: 3 }, false));

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
        save(&d, &Settings { idle_stop_minutes: u64::MAX, auto_archive_days: 0 }).unwrap();
        assert!(!d.join("settings.json.tmp").exists(), "the temp file is renamed away");
        assert!(!d.join("settings.json.bak").exists(), "nothing to back up on a first save");
        assert_eq!(load(&d), (Settings { idle_stop_minutes: MAX_IDLE_STOP_MINUTES, auto_archive_days: 0 }, false));
        let v: Value = serde_json::from_str(&fs::read_to_string(path_in(&d)).unwrap()).unwrap();
        assert_eq!(v["version"], 1);
    }

    #[test]
    fn the_store_notices_an_edit_without_a_restart() {
        let d = dir("store");
        let mut s = SettingsStore::open(&d);
        assert_eq!(s.get(), Settings::default());
        assert!(!s.refresh(), "nothing changed");
        save(&d, &Settings { idle_stop_minutes: 0, auto_archive_days: 9 }).unwrap();
        assert!(s.refresh());
        assert_eq!(s.get(), Settings { idle_stop_minutes: 0, auto_archive_days: 9 });
        assert!(!s.refresh());
        write(&d, "garbage");
        assert!(s.refresh() && s.is_corrupt());
        assert_eq!(s.get(), Settings::default());
    }
}
