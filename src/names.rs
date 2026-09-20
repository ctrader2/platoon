//! The names the user gave conversations. Claude Code has no such thing for finished conversations, and its own
//! files are never written to, so platoon keeps them in `<config dir>/names.json` next to `archive.json`:
//! `{"version": 1, "names": {"<session id>": "my name"}}`.
//!
//! Like the archive and the settings, the file is small and precious: writes are atomic (temp file + rename), a
//! file that can't be understood reads as "no names" and is copied to `.bak` before it is ever replaced, and
//! everything in it that platoon doesn't know (other keys, another version's entries) is kept when it writes.

use crate::archive::keep_backup;
use serde_json::{json, Map, Value};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// The longest name kept, in characters. A name is a label for a row, not a sentence.
pub const NAME_MAX: usize = 100;

/// One line, trimmed, at most `NAME_MAX` characters. Empty means "no name".
pub fn clean(name: &str) -> String {
    let one = name.split_whitespace().collect::<Vec<_>>().join(" ");
    one.chars().take(NAME_MAX).collect::<String>().trim_end().to_string()
}

pub struct Names {
    path: PathBuf,
    /// Session id -> the name the user gave it.
    names: std::collections::HashMap<String, String>,
    stamp: Option<SystemTime>,
    /// The file exists but couldn't be understood; it is backed up before the next write.
    corrupt: bool,
}

impl Names {
    pub fn open(dir: &Path) -> Self {
        let mut n = Self { path: dir.join("names.json"), names: Default::default(), stamp: None, corrupt: false };
        n.load();
        n
    }

    /// The name the user gave this conversation, if any.
    pub fn get(&self, id: &str) -> Option<&str> {
        self.names.get(id).map(String::as_str)
    }

    #[cfg(test)]
    pub fn is_corrupt(&self) -> bool {
        self.corrupt
    }

    fn modified(&self) -> Option<SystemTime> {
        fs::metadata(&self.path).and_then(|m| m.modified()).ok()
    }

    /// The top-level object of the file as it is now: None when there is no file, Err when there is one that isn't
    /// an object with (if it has one) an object under `names`.
    fn read(&self) -> Result<Option<Map<String, Value>>, ()> {
        let text = match fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(()),
        };
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(o)) if o.get("names").is_none_or(Value::is_object) => Ok(Some(o)),
            _ => Err(()),
        }
    }

    fn load(&mut self) {
        self.stamp = self.modified();
        self.names.clear();
        self.corrupt = false;
        match self.read() {
            Ok(Some(o)) => {
                for (id, v) in o.get("names").and_then(Value::as_object).into_iter().flatten() {
                    // Anything that isn't text is somebody else's (a newer version's): kept on disk, not shown.
                    if let Some(name) = v.as_str().map(clean).filter(|n| !n.is_empty()) {
                        self.names.insert(id.clone(), name);
                    }
                }
            }
            Ok(None) => {}
            Err(()) => self.corrupt = true,
        }
    }

    /// Pick up a change made by another platoon window. One `stat` when nothing changed.
    pub fn reload_if_changed(&mut self) -> bool {
        if self.modified() == self.stamp {
            return false;
        }
        self.load();
        true
    }

    /// Give a conversation a name, or take its name away (an empty one). Starts from what is on disk, so a second
    /// window's changes aren't lost, and keeps everything else in the file as it was.
    pub fn set(&mut self, id: &str, name: &str) -> io::Result<()> {
        let name = clean(name);
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut root = match self.read() {
            Ok(o) => o.unwrap_or_default(),
            Err(()) => {
                // Whatever is in there might be recoverable by hand: keep it rather than overwrite it.
                keep_backup(&self.path)?;
                Map::new()
            }
        };
        root.entry("version").or_insert(json!(1));
        let names = root.entry("names").or_insert_with(|| json!({}));
        if let Some(map) = names.as_object_mut() {
            if name.is_empty() {
                map.remove(id);
            } else {
                map.insert(id.to_string(), json!(name));
            }
        }
        let text = serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_default();
        let tmp = self.path.with_extension("json.tmp");
        let written = fs::write(&tmp, text).and_then(|()| fs::rename(&tmp, &self.path));
        if written.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        self.load();
        written
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("platoon-names-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn saved(d: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(d.join("names.json")).unwrap()).unwrap()
    }

    #[test]
    fn a_name_is_saved_read_back_and_survives_a_new_run() {
        let d = dir("basic");
        let mut n = Names::open(&d);
        assert_eq!(n.get("aaa"), None);
        assert!(!d.exists(), "reading creates nothing");
        n.set("aaa", "  My   payments\nwork ").unwrap();
        assert_eq!(n.get("aaa"), Some("My payments work"), "one line, trimmed");
        assert!(!d.join("names.json.tmp").exists(), "the temp file is renamed away");
        assert_eq!(saved(&d), json!({"version":1,"names":{"aaa":"My payments work"}}));
        assert_eq!(Names::open(&d).get("aaa"), Some("My payments work"));
    }

    #[test]
    fn an_empty_name_clears_it() {
        let d = dir("clear");
        let mut n = Names::open(&d);
        n.set("aaa", "one").unwrap();
        n.set("bbb", "two").unwrap();
        n.set("aaa", "   ").unwrap();
        assert_eq!((n.get("aaa"), n.get("bbb")), (None, Some("two")));
        assert_eq!(saved(&d)["names"], json!({"bbb":"two"}));
    }

    #[test]
    fn a_name_is_cut_to_a_sensible_length() {
        let d = dir("long");
        let mut n = Names::open(&d);
        n.set("aaa", &"x".repeat(500)).unwrap();
        assert_eq!(n.get("aaa").unwrap().chars().count(), NAME_MAX);
        assert_eq!(clean("é".repeat(300).as_str()).chars().count(), NAME_MAX);
    }

    #[test]
    fn everything_in_the_file_that_is_not_ours_survives_a_change() {
        let d = dir("keep");
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("names.json"),
            r#"{"note":"mine","version":7,"nested":{"a":[1,{"b":null}]},"names":{"old":"Old name","future":{"name":"Fancy","color":"red"},"num":5},"tail":true}"#,
        )
        .unwrap();
        let mut n = Names::open(&d);
        assert_eq!((n.get("old"), n.get("future"), n.get("num")), (Some("Old name"), None, None), "only text is a name");
        n.set("new", "Brand new").unwrap();
        n.set("old", "").unwrap();
        let v = saved(&d);
        assert_eq!(v["note"], "mine");
        assert_eq!(v["version"], 7, "a version written by something else is not rewritten");
        assert_eq!(v["nested"], json!({"a":[1,{"b":null}]}));
        assert_eq!(v["tail"], true);
        assert_eq!(v["names"], json!({"future":{"name":"Fancy","color":"red"},"num":5,"new":"Brand new"}));
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["note", "version", "nested", "names", "tail"], "the file keeps its shape");
        assert!(!d.join("names.json.bak").exists(), "a healthy file needs no backup");
    }

    #[test]
    fn a_corrupt_file_reads_as_no_names_and_is_backed_up_before_being_replaced() {
        let d = dir("corrupt");
        fs::create_dir_all(&d).unwrap();
        for bad in ["{ not json", "[1,2]", "42", "", r#"{"names":[1]}"#, r#"{"names":"x"}"#] {
            fs::write(d.join("names.json"), bad).unwrap();
            let n = Names::open(&d);
            assert!(n.is_corrupt() && n.get("x").is_none(), "{bad:?}");
            assert_eq!(fs::read_to_string(d.join("names.json")).unwrap(), bad, "reading must never modify it");
        }
        fs::write(d.join("names.json"), "{ not json").unwrap();
        let mut n = Names::open(&d);
        n.set("aaa", "fresh").unwrap();
        assert_eq!(fs::read_to_string(d.join("names.json.bak")).unwrap(), "{ not json");
        assert_eq!(Names::open(&d).get("aaa"), Some("fresh"));
        assert!(!n.is_corrupt());
        // A second corruption keeps the first backup too.
        fs::write(d.join("names.json"), "[]").unwrap();
        Names::open(&d).set("bbb", "x").unwrap();
        assert_eq!(fs::read_to_string(d.join("names.json.bak")).unwrap(), "{ not json");
        assert_eq!(fs::read_to_string(d.join("names.json.bak.1")).unwrap(), "[]");
    }

    #[test]
    fn a_second_window_sees_changes_and_none_are_lost() {
        let d = dir("two");
        let (mut a, mut b) = (Names::open(&d), Names::open(&d));
        a.set("one", "first").unwrap();
        b.set("two", "second").unwrap(); // b starts from the file, so "one" survives
        assert!(b.get("one").is_some() && b.get("two").is_some());
        assert!(a.reload_if_changed());
        assert_eq!(a.get("two"), Some("second"));
        assert!(!a.reload_if_changed(), "nothing changed since");
    }

    #[test]
    fn a_failed_save_leaves_the_file_and_no_temp_file_behind() {
        let d = dir("fail");
        fs::create_dir_all(&d).unwrap();
        let mut n = Names::open(&d);
        n.set("aaa", "kept").unwrap();
        // The rename target becomes a directory, so the write works and the rename fails.
        fs::remove_file(d.join("names.json")).unwrap();
        fs::create_dir(d.join("names.json")).unwrap();
        assert!(n.set("bbb", "lost").is_err());
        assert!(!d.join("names.json.tmp").exists());
    }
}
