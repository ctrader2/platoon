//! Which conversations are archived. Claude Code has no archive feature, so platoon keeps its own list in
//! `<config dir>/archive.json` (never inside `~/.claude`). Archiving hides a conversation from the Active
//! tab; the transcript is untouched.
//!
//! The file is small and precious, so writes are atomic (temp file + rename), a file that can't be read
//! counts as empty rather than crashing, and it is never overwritten without first keeping a `.bak` copy.

use serde_json::{json, Map, Value};
use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// `$PLATOON_CONFIG_DIR`, else `$XDG_CONFIG_HOME/platoon`, else `~/.config/platoon`.
pub fn config_dir() -> PathBuf {
    config_dir_from(&|k| std::env::var(k).ok())
}

fn config_dir_from(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    let set = |k: &str| env(k).filter(|v| !v.is_empty());
    if let Some(d) = set("PLATOON_CONFIG_DIR") {
        return PathBuf::from(d);
    }
    let base = set("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(set("HOME").unwrap_or_default()).join(".config"));
    base.join("platoon")
}

pub struct Archive {
    path: PathBuf,
    /// Session id -> when it was archived (seconds since the epoch).
    ids: HashMap<String, u64>,
    stamp: Option<SystemTime>,
    /// The file exists but couldn't be understood; it gets backed up before the next write.
    corrupt: bool,
}

impl Archive {
    pub fn open(dir: &Path) -> Self {
        let mut a = Self { path: dir.join("archive.json"), ids: HashMap::new(), stamp: None, corrupt: false };
        a.load();
        a
    }

    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains_key(id)
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    fn modified(&self) -> Option<SystemTime> {
        fs::metadata(&self.path).and_then(|m| m.modified()).ok()
    }

    fn load(&mut self) {
        self.stamp = self.modified();
        self.corrupt = false;
        self.ids.clear();
        let text = match fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return,
            Err(_) => {
                self.corrupt = self.path.exists();
                return;
            }
        };
        match serde_json::from_str::<Value>(&text).ok().as_ref().and_then(|v| v["archived"].as_object()) {
            Some(map) => {
                self.ids = map.iter().map(|(id, ts)| (id.clone(), ts.as_u64().unwrap_or(0))).collect();
            }
            None => self.corrupt = true,
        }
    }

    /// Pick up changes made by another platoon window. One `stat` when nothing changed.
    pub fn reload_if_changed(&mut self) -> bool {
        if self.modified() == self.stamp {
            return false;
        }
        self.load();
        true
    }

    pub fn archive(&mut self, id: &str, at: u64) -> io::Result<()> {
        self.load(); // start from what's on disk, so a second window's changes aren't lost
        self.ids.insert(id.to_string(), at);
        self.save()
    }

    pub fn restore(&mut self, id: &str) -> io::Result<()> {
        self.load();
        self.ids.remove(id);
        self.save()
    }

    fn save(&mut self) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        if self.corrupt {
            // Whatever is in there might be recoverable by hand: keep it rather than overwrite it.
            let mut bak = self.path.with_extension("json.bak");
            let mut n = 1;
            while bak.exists() {
                bak = self.path.with_extension(format!("json.bak.{n}"));
                n += 1;
            }
            fs::copy(&self.path, &bak)?;
        }
        let mut map = Map::new();
        let mut ids: Vec<_> = self.ids.iter().collect();
        ids.sort();
        for (id, at) in ids {
            map.insert(id.clone(), json!(at));
        }
        let text = serde_json::to_string_pretty(&json!({ "version": 1, "archived": map })).unwrap_or_default();
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, text)?;
        fs::rename(&tmp, &self.path)?;
        self.corrupt = false;
        self.stamp = self.modified();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("platoon-archive-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn archive_and_restore_persist_across_runs() {
        let d = dir("persist");
        let mut a = Archive::open(&d); // the directory doesn't exist yet
        assert_eq!(a.len(), 0);
        a.archive("aaa", 100).unwrap();
        a.archive("bbb", 200).unwrap();
        a.restore("aaa").unwrap();
        let b = Archive::open(&d);
        assert!(!b.contains("aaa") && b.contains("bbb"));
        assert!(!d.join("archive.json.tmp").exists(), "the temp file must be renamed away");
    }

    #[test]
    fn a_second_window_sees_changes_and_none_are_lost() {
        let d = dir("two");
        let (mut a, mut b) = (Archive::open(&d), Archive::open(&d));
        a.archive("one", 1).unwrap();
        b.archive("two", 2).unwrap(); // b reloads before writing, so "one" survives
        assert!(b.contains("one") && b.contains("two"));
        assert!(a.reload_if_changed());
        assert!(a.contains("two"));
        assert!(!a.reload_if_changed(), "nothing changed since");
    }

    #[test]
    fn a_corrupt_file_reads_as_empty_and_is_backed_up_before_being_replaced() {
        let d = dir("corrupt");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("archive.json"), "{ this is not json").unwrap();
        let mut a = Archive::open(&d);
        assert_eq!(a.len(), 0, "must not crash, and must not pretend to know anything");
        assert_eq!(fs::read_to_string(d.join("archive.json")).unwrap(), "{ this is not json", "reading must not touch it");
        a.archive("x", 5).unwrap();
        assert_eq!(fs::read_to_string(d.join("archive.json.bak")).unwrap(), "{ this is not json");
        assert!(Archive::open(&d).contains("x"));

        // A second corruption keeps the first backup too.
        fs::write(d.join("archive.json"), "[]").unwrap();
        let mut a = Archive::open(&d);
        a.archive("y", 6).unwrap();
        assert_eq!(fs::read_to_string(d.join("archive.json.bak")).unwrap(), "{ this is not json");
        assert_eq!(fs::read_to_string(d.join("archive.json.bak.1")).unwrap(), "[]");
    }

    #[test]
    fn a_missing_file_is_just_empty_and_creates_no_backup() {
        let d = dir("missing");
        let mut a = Archive::open(&d);
        a.archive("x", 1).unwrap();
        assert!(!d.join("archive.json.bak").exists());
    }

    #[test]
    fn config_dir_follows_the_documented_order() {
        let env = |pairs: &'static [(&str, &str)]| move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string());
        let all = env(&[("PLATOON_CONFIG_DIR", "/p"), ("XDG_CONFIG_HOME", "/x"), ("HOME", "/h")]);
        assert_eq!(config_dir_from(&all), PathBuf::from("/p"));
        let xdg = env(&[("XDG_CONFIG_HOME", "/x"), ("HOME", "/h")]);
        assert_eq!(config_dir_from(&xdg), PathBuf::from("/x/platoon"));
        let home = env(&[("HOME", "/h")]);
        assert_eq!(config_dir_from(&home), PathBuf::from("/h/.config/platoon"));
    }
}
