//! Which conversations are archived. Claude Code has no archive feature, so platoon keeps its own list in
//! `<config dir>/archive.json` (never inside `~/.claude`). Archiving hides a conversation from the Active
//! tab; the transcript is untouched.
//!
//! The file is small and precious, so writes are atomic (temp file + rename), a file that can't be read
//! counts as empty rather than crashing, and it is never overwritten without first keeping a `.bak` copy.
//!
//! Besides what is archived, the file remembers when the user last brought a conversation back or opened it
//! (`touched`). Auto-archive counts that as activity: without it, restoring an old conversation without
//! sending anything would just get it archived again at the next check. Files without that key (written
//! before it existed) read as if nothing was touched, and older platoons ignore the key.

use crate::model::Session;
use crate::settings::MAX_AUTO_ARCHIVE_DAYS;
use serde_json::{Map, Value, json};
use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// Copy a file that can't be understood to `<name>.bak` (`.bak.1`, ...) so replacing it loses nothing.
pub fn keep_backup(path: &Path) -> io::Result<()> {
    let mut bak = path.with_extension("json.bak");
    let mut n = 1;
    while bak.exists() {
        bak = path.with_extension(format!("json.bak.{n}"));
        n += 1;
    }
    fs::copy(path, &bak).map(|_| ())
}

/// `$PLATOON_CONFIG_DIR`, else `$XDG_CONFIG_HOME/platoon`, else `~/.config/platoon`.
/// The user's real one: code that a unit test runs takes the config folder as an argument instead.
pub fn config_dir() -> PathBuf {
    crate::usage::forbid_real_dir("config_dir");
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
    /// Session id -> when the user last restored or opened it (seconds since the epoch).
    touched: HashMap<String, u64>,
    stamp: Option<SystemTime>,
    /// The file exists but couldn't be understood; it gets backed up before the next write.
    corrupt: bool,
    /// How many times the file was written, so a test can prove a batch is one write.
    #[cfg(test)]
    saves: usize,
}

impl Archive {
    pub fn open(dir: &Path) -> Self {
        let mut a = Self {
            path: dir.join("archive.json"),
            ids: HashMap::new(),
            touched: HashMap::new(),
            stamp: None,
            corrupt: false,
            #[cfg(test)]
            saves: 0,
        };
        a.load();
        a
    }

    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains_key(id)
    }

    /// How many times the file has been written by this instance.
    #[cfg(test)]
    pub fn saves(&self) -> usize {
        self.saves
    }

    /// The file exists but couldn't be understood. What it held is unknown, so nothing automatic writes over it.
    pub fn is_corrupt(&self) -> bool {
        self.corrupt
    }

    pub fn touched_at(&self, id: &str) -> Option<u64> {
        self.touched.get(id).copied()
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
        self.touched.clear();
        let text = match fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return,
            Err(_) => {
                self.corrupt = self.path.exists();
                return;
            }
        };
        let v = serde_json::from_str::<Value>(&text).ok();
        match v.as_ref().and_then(|v| v["archived"].as_object()) {
            Some(map) => {
                self.ids = map.iter().map(|(id, ts)| (id.clone(), ts.as_u64().unwrap_or(0))).collect();
                if let Some(map) = v.as_ref().and_then(|v| v["touched"].as_object()) {
                    self.touched = map.iter().filter_map(|(id, ts)| Some((id.clone(), ts.as_u64()?))).collect();
                }
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
        self.archive_many(&[id.to_string()], at)
    }

    /// Archive several at once with a single write: a first run can archive hundreds.
    pub fn archive_many(&mut self, ids: &[String], at: u64) -> io::Result<()> {
        self.load(); // start from what's on disk, so a second window's changes aren't lost
        for id in ids {
            self.ids.insert(id.clone(), at);
            self.touched.remove(id);
        }
        self.save(at)
    }

    /// Take it out of the archive and count `at` as activity, so it isn't archived again straight away.
    pub fn restore(&mut self, id: &str, at: u64) -> io::Result<()> {
        self.load();
        self.ids.remove(id);
        self.touched.insert(id.to_string(), at);
        self.save(at)
    }

    /// Restore several at once with a single write (restoring a project brings back all of its sessions).
    pub fn restore_many(&mut self, ids: &[String], at: u64) -> io::Result<()> {
        self.load();
        for id in ids {
            self.ids.remove(id);
            self.touched.insert(id.clone(), at);
        }
        self.save(at)
    }

    /// The user opened it: count `at` as activity even if nothing gets written to the conversation.
    pub fn touch(&mut self, id: &str, at: u64) -> io::Result<()> {
        self.load();
        self.touched.insert(id.to_string(), at);
        self.save(at)
    }

    /// `now` is only used to forget restores too old to matter.
    fn save(&mut self, now: u64) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        if self.corrupt {
            // Whatever is in there might be recoverable by hand: keep it rather than overwrite it.
            keep_backup(&self.path)?;
        }
        let sorted = |m: &HashMap<String, u64>| {
            let mut v: Vec<_> = m.iter().collect();
            v.sort();
            v.into_iter().map(|(id, at)| (id.clone(), json!(at))).collect::<Map<String, Value>>()
        };
        // Restores older than any auto-archive age can no longer matter.
        let oldest = now.saturating_sub((MAX_AUTO_ARCHIVE_DAYS + 1) * 86_400);
        self.touched.retain(|_, at| *at >= oldest);
        let text = serde_json::to_string_pretty(&json!({ "version": 1, "archived": sorted(&self.ids), "touched": sorted(&self.touched) }))
            .unwrap_or_default();
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, text)?;
        fs::rename(&tmp, &self.path)?;
        self.corrupt = false;
        self.stamp = self.modified();
        #[cfg(test)]
        {
            self.saves += 1;
        }
        Ok(())
    }
}

/// Conversations to archive automatically: nothing running them (nor Claude's own service, for a background session), not the one being viewed (which may not
/// have registered its process yet), not already
/// archived, and no activity for `age_secs`. Activity is the later of the transcript's last entry and the
/// last time the user restored or opened it. A conversation with no known time is never archived.
pub fn auto_archive_ids(sessions: &[Session], archive: &Archive, now_secs: u64, age_secs: u64, viewing: Option<&str>) -> Vec<String> {
    sessions
        .iter()
        // A background session is run by Claude's own service and shows as a process, so `procs` already keeps it
        // out; the status is checked as well so that stays true whatever happens to `procs`.
        .filter(|s| !archive.contains(&s.id) && s.procs.is_empty() && s.status != crate::model::Status::Background)
        .filter(|s| viewing != Some(s.id.as_str()))
        .filter(|s| {
            let active = (s.last_active / 1000).max(archive.touched_at(&s.id).unwrap_or(0));
            active > 0 && now_secs.saturating_sub(active) >= age_secs
        })
        .map(|s| s.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-archive-{name}-{}", std::process::id())));
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
        a.restore("aaa", 300).unwrap();
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
    fn a_batch_is_one_atomic_write_and_a_restore_is_remembered() {
        let d = dir("batch");
        let mut a = Archive::open(&d);
        let ids: Vec<String> = (0..200).map(|i| format!("id-{i}")).collect();
        a.archive_many(&ids, 1000).unwrap();
        assert_eq!(a.saves(), 1, "200 conversations must cost one write, not 200");
        assert!(!d.join("archive.json.tmp").exists());
        let b = Archive::open(&d);
        assert_eq!(b.len(), 200);

        let mut b = b;
        b.restore("id-7", 5000).unwrap();
        let c = Archive::open(&d);
        assert!(!c.contains("id-7") && c.contains("id-8"));
        assert_eq!(c.touched_at("id-7"), Some(5000), "the restore time survives a restart");
        assert_eq!(c.touched_at("id-8"), None);
    }

    #[test]
    fn a_file_from_before_touched_existed_still_loads_and_old_platoons_ignore_the_new_key() {
        let d = dir("compat");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("archive.json"), r#"{"version":1,"archived":{"old":5}}"#).unwrap();
        let mut a = Archive::open(&d);
        assert!(a.contains("old") && !a.is_corrupt() && a.touched_at("old").is_none());
        a.touch("other", 1000).unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(d.join("archive.json")).unwrap()).unwrap();
        assert_eq!(v["archived"]["old"], 5, "what an old platoon reads is unchanged");
        assert!(v["touched"]["other"].as_u64().is_some());
    }

    #[test]
    fn touches_too_old_to_matter_are_forgotten_on_the_next_write() {
        let d = dir("prune");
        let mut a = Archive::open(&d);
        let now = 1000 * DAY;
        a.touch("ancient", 100).unwrap();
        a.touch("recent", now).unwrap(); // writing at `now` drops the 900-day-old entry
        let b = Archive::open(&d);
        assert_eq!((b.touched_at("ancient"), b.touched_at("recent")), (None, Some(now)));
    }

    fn session(id: &str, last_active_secs: u64, running: bool) -> Session {
        Session {
            id: id.into(),
            agent: crate::agents::Agent::Claude,
            name: id.into(),
            dir: String::new(),
            extra_dirs: Vec::new(),
            cwd: String::new(),
            branch: None,
            model: None,
            status: crate::model::Status::Idle,
            unread: false,
            context_pct: 0,
            context_exact: false,
            context_est: 0,
            description: String::new(),
            last_active: last_active_secs * 1000,
            archived: false,
            procs: if running {
                vec![crate::model::Proc {
                    pid: 1,
                    started_at: 0,
                    hosted: true,
                    background: None,
                    stale: false,
                    parked: false,
                    moved_to: None,
                }]
            } else {
                vec![]
            },
            haystack: String::new(),
            name_source: crate::model::NameSource::Folder,
            summary: String::new(),
        }
    }

    const DAY: u64 = 86_400;
    const NOW: u64 = 100 * DAY;

    fn picked(sessions: &[Session], a: &Archive, viewing: Option<&str>) -> Vec<String> {
        auto_archive_ids(sessions, a, NOW, 2 * DAY, viewing)
    }

    #[test]
    fn only_old_idle_unarchived_conversations_are_picked() {
        let d = dir("pick");
        let mut a = Archive::open(&d);
        a.archive("already", NOW - 5 * DAY).unwrap();
        let sessions = [
            session("old", NOW - 3 * DAY, false),
            session("exactly-2-days", NOW - 2 * DAY, false),
            session("recent", NOW - 2 * DAY + 60, false),
            session("running", NOW - 30 * DAY, true),
            session("watched", NOW - 30 * DAY, false),
            session("already", NOW - 30 * DAY, false),
            session("unknown-time", 0, false),
        ];
        assert_eq!(picked(&sessions, &a, Some("watched")), ["old", "exactly-2-days"]);
        assert!(picked(&sessions, &a, None).contains(&"watched".to_string()), "only being viewed protects it");
    }

    #[test]
    fn a_background_session_is_never_picked_whatever_its_process_list_says() {
        let d = dir("bg");
        let a = Archive::open(&d);
        let mut bg = session("bg", NOW - 30 * DAY, false);
        bg.status = crate::model::Status::Background;
        let with_proc = {
            let mut s = session("bg-proc", NOW - 30 * DAY, true);
            s.status = crate::model::Status::Background;
            s
        };
        assert!(picked(&[bg, with_proc, session("old", NOW - 30 * DAY, false)], &a, None) == ["old"]);
    }

    #[test]
    fn a_restore_counts_as_activity_but_never_makes_a_conversation_look_older() {
        let d = dir("touched");
        let mut a = Archive::open(&d);
        a.touch("restored-now", NOW - 60).unwrap();
        a.touch("restored-long-ago", NOW - 9 * DAY).unwrap();
        a.touch("active-since", NOW - 10 * DAY).unwrap();
        let sessions = [
            session("restored-now", NOW - 30 * DAY, false),
            session("restored-long-ago", NOW - 30 * DAY, false),
            session("active-since", NOW - 60, false),
        ];
        assert_eq!(picked(&sessions, &a, None), ["restored-long-ago"], "max(last active, touched) is what counts");
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
