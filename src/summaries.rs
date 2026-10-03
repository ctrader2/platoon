//! The names and summaries platoon generated for conversations (step C's "smart names"). They cost tokens to
//! make, so unlike the cache they live in the config folder, `<config dir>/summaries.json`, next to `names.json`:
//! `{"version": 1, "entries": {"<session id>": {"name": "...", "summary": "...", "covers": 7, "model": "haiku", "at": 1789000000}}}`.
//!
//! What an entry holds: the name, the summary, how many substantive messages of the conversation it covers, which
//! model wrote it and when. Never a message you typed and never a reply from Claude.
//!
//! Like the archive, the names and the settings, the file is small and precious: writes are atomic (temp file +
//! rename), a file that can't be understood reads as empty and is copied to `.bak` before it is ever replaced, and
//! everything in it that platoon doesn't know (other keys, other fields of an entry, another version's entries) is
//! kept when it writes. A change made by a second platoon window is picked up.

use crate::archive::keep_backup;
use serde_json::{Map, Value, json};
use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// One conversation's generated name and summary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub summary: String,
    /// How many substantive messages of the conversation this covers: what a refresh counts new ones from.
    pub covers: usize,
    /// The model that wrote it (`haiku`, `sonnet`).
    pub model: String,
    /// When it was made, seconds since the epoch.
    pub at: u64,
}

impl Entry {
    fn from_json(v: &Value) -> Option<Entry> {
        let o = v.as_object()?;
        let text = |k: &str| o.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
        Some(Entry {
            name: text("name")?.to_string(),
            summary: text("summary").unwrap_or("").to_string(),
            covers: o.get("covers").and_then(Value::as_u64).unwrap_or(0) as usize,
            model: text("model").unwrap_or("").to_string(),
            at: o.get("at").and_then(Value::as_u64).unwrap_or(0),
        })
    }

    /// Our fields written into `target`, other fields of it left as they are.
    fn write_into(&self, target: &mut Map<String, Value>) {
        target.insert("name".into(), json!(self.name));
        target.insert("summary".into(), json!(self.summary));
        target.insert("covers".into(), json!(self.covers));
        target.insert("model".into(), json!(self.model));
        target.insert("at".into(), json!(self.at));
    }
}

pub struct Summaries {
    path: PathBuf,
    entries: HashMap<String, Entry>,
    stamp: Option<SystemTime>,
    /// The file exists but couldn't be understood; it is backed up before the next write.
    corrupt: bool,
}

impl Summaries {
    pub fn open(dir: &Path) -> Self {
        let mut s = Self { path: dir.join("summaries.json"), entries: HashMap::new(), stamp: None, corrupt: false };
        s.load();
        s
    }

    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.get(id)
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[cfg(test)]
    pub fn is_corrupt(&self) -> bool {
        self.corrupt
    }

    fn modified(&self) -> Option<SystemTime> {
        fs::metadata(&self.path).and_then(|m| m.modified()).ok()
    }

    /// The top-level object as it is now: None when there is no file, Err when there is one that isn't an object
    /// with (if it has one) an object under `entries`.
    fn read(&self) -> Result<Option<Map<String, Value>>, ()> {
        let text = match fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(()),
        };
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(o)) if o.get("entries").is_none_or(Value::is_object) => Ok(Some(o)),
            _ => Err(()),
        }
    }

    fn load(&mut self) {
        self.stamp = self.modified();
        self.entries.clear();
        self.corrupt = false;
        match self.read() {
            Ok(Some(o)) => {
                for (id, v) in o.get("entries").and_then(Value::as_object).into_iter().flatten() {
                    // An entry that isn't ours (another version's shape) is kept on disk and not shown.
                    if let Some(e) = Entry::from_json(v) {
                        self.entries.insert(id.clone(), e);
                    }
                }
            }
            Ok(None) => {}
            Err(()) => self.corrupt = true,
        }
    }

    /// Pick up a change made by another platoon window. One `stat` when nothing changed. True when re-read.
    pub fn reload_if_changed(&mut self) -> bool {
        if self.modified() == self.stamp {
            return false;
        }
        self.load();
        true
    }

    /// Save these entries (all in one write). Starts from what is on disk, so another window's changes aren't lost,
    /// and keeps everything else in the file as it was.
    pub fn set_many(&mut self, changes: &[(String, Entry)]) -> io::Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        // Someone else (a second window) wrote since we last looked: what we hold is out of date, so read it all after.
        let stale = self.modified() != self.stamp;
        let mut root = match self.read() {
            Ok(o) => o.unwrap_or_default(),
            Err(()) => {
                // Whatever is in there might be recoverable by hand: keep it rather than overwrite it.
                keep_backup(&self.path)?;
                Map::new()
            }
        };
        root.entry("version").or_insert(json!(1));
        let entries = root.entry("entries").or_insert_with(|| json!({}));
        if let Some(map) = entries.as_object_mut() {
            for (id, e) in changes {
                let slot = map.entry(id.clone()).or_insert_with(|| json!({}));
                if !slot.is_object() {
                    *slot = json!({});
                }
                if let Some(o) = slot.as_object_mut() {
                    e.write_into(o);
                }
            }
        }
        let text = serde_json::to_string(&Value::Object(root)).unwrap_or_default();
        let tmp = self.path.with_extension("json.tmp");
        let written = fs::write(&tmp, text).and_then(|()| fs::rename(&tmp, &self.path));
        if written.is_err() || stale {
            if written.is_err() {
                let _ = fs::remove_file(&tmp);
            }
            self.load();
            return written;
        }
        // What was just written is what is known: no need to read the whole file again for it. (Entries other
        // windows wrote in the meantime are in the file and are picked up when its time stamp is seen to change.)
        for (id, e) in changes {
            self.entries.insert(id.clone(), e.clone());
        }
        self.corrupt = false;
        self.stamp = self.modified();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-summaries-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn e(name: &str, summary: &str, covers: usize) -> Entry {
        Entry { name: name.into(), summary: summary.into(), covers, model: "haiku".into(), at: 1_789_000_000 }
    }

    fn saved(d: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(d.join("summaries.json")).unwrap()).unwrap()
    }

    #[test]
    fn an_entry_is_saved_read_back_and_survives_a_new_run_and_holds_only_what_it_should() {
        let d = dir("basic");
        let mut s = Summaries::open(&d);
        assert!(s.get("aaa").is_none() && s.is_empty());
        assert!(!d.exists(), "reading creates nothing");
        s.set_many(&[("aaa".into(), e("Fix login", "Fixing the login redirect.", 4))]).unwrap();
        assert_eq!(s.get("aaa"), Some(&e("Fix login", "Fixing the login redirect.", 4)));
        assert!(!d.join("summaries.json.tmp").exists(), "the temp file is renamed away");
        assert_eq!(
            saved(&d),
            json!({"version":1,"entries":{"aaa":{"name":"Fix login","summary":"Fixing the login redirect.","covers":4,"model":"haiku","at":1_789_000_000u64}}})
        );
        assert_eq!(Summaries::open(&d).get("aaa").map(|x| x.covers), Some(4));
        assert_eq!(Summaries::open(&d).len(), 1);
    }

    #[test]
    fn many_entries_go_in_one_write_and_a_later_one_replaces_an_earlier_one() {
        let d = dir("many");
        let mut s = Summaries::open(&d);
        s.set_many(&[("a".into(), e("A", "a", 1)), ("b".into(), e("B", "b", 1))]).unwrap();
        s.set_many(&[("a".into(), e("A2", "a2", 6))]).unwrap();
        assert_eq!((s.get("a").map(|x| x.name.as_str()), s.get("b").map(|x| x.name.as_str())), (Some("A2"), Some("B")));
        assert!(s.set_many(&[]).is_ok());
    }

    #[test]
    fn everything_in_the_file_that_is_not_ours_survives_a_change() {
        let d = dir("keep");
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("summaries.json"),
            r#"{"note":"mine","version":7,"entries":{"old":{"name":"Old","summary":"s","covers":2,"model":"haiku","at":5,"extra":{"x":[1]}},"weird":42,"future":{"title":"other shape"}},"tail":true}"#,
        )
        .unwrap();
        let mut s = Summaries::open(&d);
        assert_eq!((s.get("old").map(|x| x.name.as_str()), s.get("weird"), s.get("future")), (Some("Old"), None, None));
        s.set_many(&[("new".into(), e("New", "n", 3)), ("old".into(), e("Old renamed", "s2", 9))]).unwrap();
        let v = saved(&d);
        assert_eq!((v["note"].clone(), v["version"].clone(), v["tail"].clone()), (json!("mine"), json!(7), json!(true)));
        assert_eq!(v["entries"]["weird"], json!(42));
        assert_eq!(v["entries"]["future"], json!({"title":"other shape"}));
        assert_eq!(v["entries"]["old"]["extra"], json!({"x":[1]}), "another field of an entry we own is kept");
        assert_eq!(v["entries"]["old"]["name"], "Old renamed");
        assert_eq!(v["entries"]["new"]["covers"], 3);
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["note", "version", "entries", "tail"], "the file keeps its shape");
        assert!(!d.join("summaries.json.bak").exists(), "a healthy file needs no backup");
    }

    #[test]
    fn a_corrupt_file_reads_as_empty_and_is_backed_up_before_being_replaced() {
        let d = dir("corrupt");
        fs::create_dir_all(&d).unwrap();
        for bad in ["{ not json", "[1,2]", "42", "", r#"{"entries":[1]}"#, r#"{"entries":"x"}"#] {
            fs::write(d.join("summaries.json"), bad).unwrap();
            let s = Summaries::open(&d);
            assert!(s.is_corrupt() && s.is_empty(), "{bad:?}");
            assert_eq!(fs::read_to_string(d.join("summaries.json")).unwrap(), bad, "reading must never modify it");
        }
        fs::write(d.join("summaries.json"), "{ not json").unwrap();
        let mut s = Summaries::open(&d);
        s.set_many(&[("aaa".into(), e("Fresh", "f", 1))]).unwrap();
        assert_eq!(fs::read_to_string(d.join("summaries.json.bak")).unwrap(), "{ not json");
        assert_eq!(Summaries::open(&d).get("aaa").map(|x| x.name.as_str()), Some("Fresh"));
        assert!(!s.is_corrupt());
        fs::write(d.join("summaries.json"), "[]").unwrap();
        Summaries::open(&d).set_many(&[("b".into(), e("B", "b", 1))]).unwrap();
        assert_eq!(fs::read_to_string(d.join("summaries.json.bak")).unwrap(), "{ not json", "the first backup is kept too");
        assert_eq!(fs::read_to_string(d.join("summaries.json.bak.1")).unwrap(), "[]");
    }

    #[test]
    fn a_second_window_sees_changes_and_none_are_lost() {
        let d = dir("two");
        let (mut a, mut b) = (Summaries::open(&d), Summaries::open(&d));
        a.set_many(&[("one".into(), e("First", "1", 1))]).unwrap();
        b.set_many(&[("two".into(), e("Second", "2", 1))]).unwrap(); // b starts from the file, so "one" survives
        assert!(b.get("one").is_some() && b.get("two").is_some());
        assert!(a.reload_if_changed());
        assert_eq!(a.get("two").map(|x| x.name.as_str()), Some("Second"));
        assert!(!a.reload_if_changed(), "nothing changed since");
    }

    #[test]
    fn a_failed_save_leaves_no_temp_file_behind() {
        let d = dir("fail");
        fs::create_dir_all(&d).unwrap();
        let mut s = Summaries::open(&d);
        s.set_many(&[("aaa".into(), e("Kept", "k", 1))]).unwrap();
        fs::remove_file(d.join("summaries.json")).unwrap();
        fs::create_dir(d.join("summaries.json")).unwrap();
        assert!(s.set_many(&[("bbb".into(), e("Lost", "l", 1))]).is_err());
        assert!(!d.join("summaries.json.tmp").exists());
    }
}
