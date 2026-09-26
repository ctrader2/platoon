//! Projects: a platoon idea, not Claude's. A project is a name, a color, a badge and active/archived; sessions
//! (conversations, by session id) belong to at most one project, and each keeps its own folder.
//!
//! Everything lives in `<config dir>/projects.json`:
//! `{"version": 1, "projects": {"<id>": {"name", "color", "badge", "archived", "created", "archived_sessions"}},
//!   "sessions": {"<session id>": "<project id>"}}`.
//! `archived_sessions` are the sessions that archiving the project archived (and that restoring it brings back).
//! The notes of a project are `<config dir>/projects/<id>/project.md`; folders are named by id so a rename is safe.
//!
//! Like the other files of ours: writes are atomic (temp file + rename) and happen under a lock file (the
//! SessionStart hook writes here too, from many sessions at once), a file that can't be understood reads as "no
//! projects" and is copied to `.bak` before it is ever replaced, and keys platoon doesn't know are kept.

use crate::archive::keep_backup;
use crate::theme::Theme;
use ratatui::style::Color;
use serde_json::{json, Map, Value};
use std::{
    collections::HashMap,
    fs, io,
    os::unix::io::AsRawFd,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// The longest project name kept, in characters.
pub const NAME_MAX: usize = 60;
/// The longest badge, in characters.
pub const BADGE_MAX: usize = 3;

/// The colors a project can have, by name. Stored as the name, never as RGB, so a project follows the theme (and the
/// 256-color fallback): the two named roles are the theme's own, the rest are its bright palette colors (the
/// terminal's own ones under Terminal colors).
pub const SLOTS: &[(&str, &str)] = &[
    ("accent", "Accent"),
    ("blue", "Blue"),
    ("magenta", "Magenta"),
    ("cyan", "Cyan"),
    ("green", "Green"),
    ("yellow", "Yellow"),
    ("red", "Red"),
    ("busy", "Purple"),
];

/// A slot's color in `t`. An unknown name (a newer version's) is the accent.
pub fn slot_color(t: &Theme, slot: &str) -> Color {
    let palette = |i: usize| t.ansi.map_or(Color::Indexed(i as u8), |p| p[i]);
    match slot {
        "busy" => t.busy,
        "blue" => palette(12),
        "magenta" => palette(13),
        "cyan" => palette(14),
        "green" => palette(10),
        "yellow" => palette(11),
        "red" => palette(9),
        _ => t.accent,
    }
}

/// The text color drawn on a badge of any slot color.
pub fn badge_text(t: &Theme) -> Color {
    t.on_accent
}

/// One line, trimmed, at most `max` characters.
pub fn clean(text: &str, max: usize) -> String {
    let one = text.split_whitespace().collect::<Vec<_>>().join(" ");
    one.chars().take(max).collect::<String>().trim_end().to_string()
}

/// A badge: no spaces, at most `BADGE_MAX` characters.
pub fn clean_badge(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace() && !c.is_control()).take(BADGE_MAX).collect()
}

/// The name's first letters: the first letter of each of the first two words, or the first two letters of a
/// one-word name, in capitals.
pub fn default_badge(name: &str) -> String {
    let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let letters: String = match words.as_slice() {
        [] => "P".into(),
        [one] => one.chars().take(2).collect(),
        [a, b, ..] => a.chars().take(1).chain(b.chars().take(1)).collect(),
    };
    letters.to_uppercase().chars().take(BADGE_MAX).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub name: String,
    /// One of `SLOTS` by name (or a name a newer version knows).
    pub color: String,
    pub badge: String,
    pub archived: bool,
    /// Seconds since the epoch.
    pub created: u64,
    /// The sessions archiving this project archived, which restoring it brings back.
    pub archived_sessions: Vec<String>,
    /// The one line platoon generated saying what the project is (empty until it has written one).
    pub description: String,
    /// When that line was written, seconds since the epoch (0: never).
    pub description_at: u64,
}

pub struct Projects {
    config: PathBuf,
    list: Vec<Project>,
    /// Session id -> project id, only for projects that exist.
    members: HashMap<String, String>,
    stamp: Option<(SystemTime, u64)>,
    corrupt: bool,
    /// Bumped whenever what is in memory changed.
    pub generation: u64,
}

fn path_in(config: &Path) -> PathBuf {
    config.join("projects.json")
}

/// Where a project's notes live.
pub fn notes_path(config: &Path, id: &str) -> PathBuf {
    config.join("projects").join(id).join("project.md")
}

/// Ids are made by platoon: letters and digits only, so they are safe as folder names.
fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn now_secs() -> u64 {
    crate::usage::now()
}

/// Hold `<file>.lock` for as long as the returned file is alive.
pub fn lock(path: &Path) -> io::Result<fs::File> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let lock = fs::OpenOptions::new().create(true).truncate(false).write(true).open(path.with_extension("lock"))?;
    // SAFETY: a plain advisory lock on a file we hold open; it is released when the file is dropped.
    unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) };
    Ok(lock)
}

/// Write `text` to `path` through a temp file of its own (unique per process and thread) and a rename.
pub fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tid = format!("{:?}", std::thread::current().id()).chars().filter(char::is_ascii_digit).collect::<String>();
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.{}-{tid}.tmp", std::process::id()));
    let written = fs::write(&tmp, text).and_then(|()| fs::rename(&tmp, path));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

impl Projects {
    pub fn open(config: &Path) -> Self {
        let mut p = Self { config: config.to_path_buf(), list: Vec::new(), members: HashMap::new(), stamp: None, corrupt: false, generation: 0 };
        p.load();
        p
    }

    pub fn path(&self) -> PathBuf {
        path_in(&self.config)
    }

    pub fn is_corrupt(&self) -> bool {
        self.corrupt
    }

    /// Every project, oldest first.
    pub fn all(&self) -> &[Project] {
        &self.list
    }

    pub fn get(&self, id: &str) -> Option<&Project> {
        self.list.iter().find(|p| p.id == id)
    }

    /// The project a session belongs to.
    pub fn of(&self, session: &str) -> Option<&Project> {
        self.members.get(session).and_then(|id| self.get(id))
    }

    pub fn project_id_of(&self, session: &str) -> Option<&str> {
        self.members.get(session).map(String::as_str)
    }

    /// The sessions in a project, in no particular order.
    pub fn members_of(&self, id: &str) -> Vec<String> {
        self.members.iter().filter(|(_, p)| p.as_str() == id).map(|(s, _)| s.clone()).collect()
    }

    /// Find a project by name: exact (ignoring case) first, else the one whose name starts with it.
    pub fn find_by_name(&self, name: &str) -> Result<&Project, String> {
        let want = clean(name, NAME_MAX).to_lowercase();
        if want.is_empty() {
            return Err("give the project's name".into());
        }
        if let Some(p) = self.list.iter().find(|p| p.name.to_lowercase() == want) {
            return Ok(p);
        }
        let starts: Vec<&Project> = self.list.iter().filter(|p| p.name.to_lowercase().starts_with(&want)).collect();
        match starts.as_slice() {
            [one] => Ok(one),
            [] => Err(format!("there is no project called \"{name}\"")),
            many => Err(format!("\"{name}\" could be {}", many.iter().map(|p| format!("\"{}\"", p.name)).collect::<Vec<_>>().join(", "))),
        }
    }

    pub fn notes_path(&self, id: &str) -> PathBuf {
        notes_path(&self.config, id)
    }

    fn stamp_now(&self) -> Option<(SystemTime, u64)> {
        fs::metadata(self.path()).ok().and_then(|m| Some((m.modified().ok()?, m.len())))
    }

    /// The file's top-level object: None when there is none, Err when there is one that isn't understood.
    fn read(&self) -> Result<Option<Map<String, Value>>, ()> {
        let text = match fs::read_to_string(self.path()) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(()),
        };
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(o))
                if o.get("projects").is_none_or(Value::is_object) && o.get("sessions").is_none_or(Value::is_object) =>
            {
                Ok(Some(o))
            }
            _ => Err(()),
        }
    }

    fn load(&mut self) {
        self.stamp = self.stamp_now();
        self.list.clear();
        self.members.clear();
        self.corrupt = false;
        self.generation += 1;
        let root = match self.read() {
            Ok(Some(o)) => o,
            Ok(None) => return,
            Err(()) => {
                self.corrupt = true;
                return;
            }
        };
        for (id, v) in root.get("projects").and_then(Value::as_object).into_iter().flatten() {
            // Anything that isn't shaped like a project is somebody else's: kept on disk, not shown.
            let Some(o) = v.as_object() else { continue };
            let Some(name) = o.get("name").and_then(Value::as_str).map(|n| clean(n, NAME_MAX)).filter(|n| !n.is_empty()) else { continue };
            if !safe_id(id) {
                continue;
            }
            let badge = o.get("badge").and_then(Value::as_str).map(clean_badge).filter(|b| !b.is_empty()).unwrap_or_else(|| default_badge(&name));
            self.list.push(Project {
                id: id.clone(),
                color: o.get("color").and_then(Value::as_str).unwrap_or("accent").to_string(),
                badge,
                archived: o.get("archived").and_then(Value::as_bool).unwrap_or(false),
                created: o.get("created").and_then(Value::as_u64).unwrap_or(0),
                archived_sessions: o
                    .get("archived_sessions")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                    .unwrap_or_default(),
                description: o.get("description").and_then(Value::as_str).map(|d| clean(d, crate::describe::MAX_CHARS)).unwrap_or_default(),
                description_at: o.get("description_at").and_then(Value::as_u64).unwrap_or(0),
                name,
            });
        }
        for (s, p) in root.get("sessions").and_then(Value::as_object).into_iter().flatten() {
            if let Some(p) = p.as_str().filter(|p| self.list.iter().any(|x| x.id == *p)) {
                self.members.insert(s.clone(), p.to_string());
            }
        }
    }

    /// Pick up a change made by another platoon window or by the hook. One `stat` when nothing changed.
    pub fn reload_if_changed(&mut self) -> bool {
        if self.stamp_now() == self.stamp {
            return false;
        }
        self.load();
        true
    }

    /// Change the file: under the lock, starting from what is on disk now (so another window's or the hook's
    /// change isn't lost), keeping everything else in it. `f` returns Err to change nothing.
    fn change<T>(&mut self, f: impl FnOnce(&mut Map<String, Value>) -> Result<T, String>) -> io::Result<T> {
        let _lock = lock(&self.path())?;
        let mut root = match self.read() {
            Ok(o) => o.unwrap_or_default(),
            Err(()) => {
                // Whatever is in there might be recoverable by hand: keep it rather than overwrite it.
                keep_backup(&self.path())?;
                Map::new()
            }
        };
        root.entry("version").or_insert(json!(1));
        for key in ["projects", "sessions"] {
            if !root.get(key).is_some_and(Value::is_object) {
                root.insert(key.into(), json!({}));
            }
        }
        let out = f(&mut root).map_err(io::Error::other);
        let out = match out {
            Ok(v) => v,
            Err(e) => {
                self.load();
                return Err(e);
            }
        };
        let text = serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_default();
        let written = write_atomic(&self.path(), &text);
        self.load();
        written.map(|()| out)
    }

    /// A new, active project called `name`. Its color is the first of `SLOTS` no active project has (in turn once
    /// all are taken) and its badge the name's first letters. Returns its id.
    pub fn create(&mut self, name: &str) -> io::Result<String> {
        let name = clean(name, NAME_MAX);
        if name.is_empty() {
            return Err(io::Error::other("a project needs a name"));
        }
        let used: Vec<&str> = self.list.iter().filter(|p| !p.archived).map(|p| p.color.as_str()).collect();
        let color = SLOTS
            .iter()
            .map(|(s, _)| *s)
            .find(|s| !used.contains(s))
            .unwrap_or(SLOTS[self.list.len() % SLOTS.len()].0)
            .to_string();
        let badge = default_badge(&name);
        let now = now_secs();
        self.change(|root| {
            let projects = root.get_mut("projects").and_then(Value::as_object_mut).ok_or("projects")?;
            let mut n: u64 = 0;
            let id = loop {
                let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
                let id = format!("p{:x}{:04x}", now, (u64::from(nanos) ^ u64::from(std::process::id()) ^ n) & 0xffff);
                if !projects.contains_key(&id) {
                    break id;
                }
                n += 1;
            };
            projects.insert(
                id.clone(),
                json!({"name": name, "color": color, "badge": badge, "archived": false, "created": now, "archived_sessions": []}),
            );
            Ok(id)
        })
    }

    /// Change one project's own object in the file.
    fn edit(&mut self, id: &str, f: impl FnOnce(&mut Map<String, Value>)) -> io::Result<()> {
        let id = id.to_string();
        self.change(move |root| {
            let p = root
                .get_mut("projects")
                .and_then(|p| p.get_mut(&id))
                .and_then(Value::as_object_mut)
                .ok_or_else(|| "that project no longer exists".to_string())?;
            f(p);
            Ok(())
        })
    }

    pub fn rename(&mut self, id: &str, name: &str) -> io::Result<()> {
        let name = clean(name, NAME_MAX);
        if name.is_empty() {
            return Err(io::Error::other("a project needs a name"));
        }
        self.edit(id, |p| {
            p.insert("name".into(), json!(name));
        })
    }

    /// A new color (one of `SLOTS`) and badge. An empty badge goes back to the name's first letters.
    pub fn set_look(&mut self, id: &str, color: &str, badge: &str) -> io::Result<()> {
        let badge = clean_badge(badge);
        let badge = if badge.is_empty() { self.get(id).map(|p| default_badge(&p.name)).unwrap_or_default() } else { badge };
        let color = color.to_string();
        self.edit(id, |p| {
            p.insert("color".into(), json!(color));
            p.insert("badge".into(), json!(badge));
        })
    }

    /// The one line that says what the project is, and when it was written. Written by the notes helper, read by
    /// every screen that lists projects; the text is cleaned before it ever gets here.
    pub fn set_description(&mut self, id: &str, text: &str, at: u64) -> io::Result<()> {
        let text = clean(text, crate::describe::MAX_CHARS);
        self.edit(id, |p| {
            p.insert("description".into(), json!(text));
            p.insert("description_at".into(), json!(at));
        })
    }

    /// Put a session in a project, or (None) in none. A session is in at most one project: this moves it. Leaving
    /// a project also drops it from what restoring that project would bring back.
    pub fn assign(&mut self, session: &str, project: Option<&str>) -> io::Result<()> {
        let (session, project) = (session.to_string(), project.map(String::from));
        self.change(move |root| {
            if let Some(p) = &project {
                if !root.get("projects").and_then(|x| x.get(p)).is_some_and(Value::is_object) {
                    return Err("that project no longer exists".into());
                }
            }
            let old = root.get("sessions").and_then(|s| s.get(&session)).and_then(Value::as_str).map(String::from);
            if old.is_some() && old != project {
                drop_archived_session(root, old.as_deref().unwrap_or(""), &session);
            }
            let sessions = root.get_mut("sessions").and_then(Value::as_object_mut).ok_or("sessions")?;
            match &project {
                Some(p) => sessions.insert(session.clone(), json!(p)),
                None => sessions.remove(&session),
            };
            Ok(())
        })
    }

    /// Put a new session in `project` unless it is already in one (for sessions started under a project: an
    /// explicit choice is never overridden). True when it was placed.
    pub fn place(&mut self, session: &str, project: &str) -> io::Result<bool> {
        // Nothing automatic writes over a file that couldn't be read: only the user's own action replaces it.
        self.reload_if_changed();
        if self.corrupt {
            return Ok(false);
        }
        let (session, project) = (session.to_string(), project.to_string());
        self.change(move |root| {
            if !root.get("projects").and_then(|x| x.get(&project)).is_some_and(Value::is_object) {
                return Ok(false);
            }
            let sessions = root.get_mut("sessions").and_then(Value::as_object_mut).ok_or("sessions")?;
            if sessions.get(&session).and_then(Value::as_str).is_some() {
                return Ok(false);
            }
            sessions.insert(session, json!(project));
            Ok(true)
        })
    }

    /// The project is archived, and with it `sessions` (its active ones, which the caller archives): they are
    /// remembered so restoring the project brings back exactly those.
    pub fn archive(&mut self, id: &str, sessions: &[String]) -> io::Result<()> {
        let list: Vec<Value> = sessions.iter().map(|s| json!(s)).collect();
        self.edit(id, |p| {
            p.insert("archived".into(), json!(true));
            p.insert("archived_sessions".into(), Value::Array(list));
        })
    }

    /// The project is active again. Returns the sessions its archiving archived (still in it), for the caller to
    /// restore; sessions that were archived before stay archived.
    pub fn restore(&mut self, id: &str) -> io::Result<Vec<String>> {
        let back: Vec<String> = self.get(id).map(|p| p.archived_sessions.clone()).unwrap_or_default();
        self.edit(id, |p| {
            p.insert("archived".into(), json!(false));
            p.insert("archived_sessions".into(), json!([]));
        })?;
        Ok(back.into_iter().filter(|s| self.members.get(s).map(String::as_str) == Some(id)).collect())
    }

    /// One session was restored on its own: it stays in its project (which stays as it is), and a later restore
    /// of the project has nothing more to do for it.
    pub fn session_restored(&mut self, session: &str) -> io::Result<()> {
        let Some(p) = self.of(session).filter(|p| p.archived_sessions.iter().any(|s| s == session)).map(|p| p.id.clone()) else { return Ok(()) };
        let session = session.to_string();
        self.change(move |root| {
            drop_archived_session(root, &p, &session);
            Ok(())
        })
    }

    /// Delete a project: its sessions are in no project afterwards, its notes folder goes, and one backup copy
    /// (the notes, the tickets and the project's entry) is kept in `<cache>/deleted-projects/<id>/`, replacing an older one.
    /// Conversations are never touched.
    pub fn delete(&mut self, id: &str, cache: &Path) -> io::Result<()> {
        if !safe_id(id) {
            return Err(io::Error::other("not a project id"));
        }
        let entry = {
            let _lock = lock(&self.path())?;
            self.read().ok().flatten().and_then(|r| r.get("projects").and_then(|p| p.get(id)).cloned())
        };
        let backup = cache.join("deleted-projects").join(id);
        let _ = fs::remove_dir_all(&backup);
        fs::create_dir_all(&backup)?;
        let notes = self.notes_path(id);
        if notes.exists() {
            fs::copy(&notes, backup.join("project.md"))?;
        }
        // The tickets too: the list and every body, as they are.
        let tickets = crate::tickets::path_in(&self.config, id);
        if tickets.exists() {
            fs::copy(&tickets, backup.join("tickets.json"))?;
        }
        let bodies = crate::tickets::bodies_dir(&self.config, id);
        if bodies.is_dir() {
            fs::create_dir_all(backup.join("tickets"))?;
            for e in fs::read_dir(&bodies)?.flatten() {
                if e.path().is_file() {
                    fs::copy(e.path(), backup.join("tickets").join(e.file_name()))?;
                }
            }
        }
        if let Some(e) = entry {
            let members: Vec<String> = self.members_of(id);
            let text = serde_json::to_string_pretty(&json!({"project": e, "sessions": members})).unwrap_or_default();
            fs::write(backup.join("project.json"), text)?;
        }
        let owned = id.to_string();
        self.change(move |root| {
            if let Some(p) = root.get_mut("projects").and_then(Value::as_object_mut) {
                p.remove(&owned);
            }
            if let Some(s) = root.get_mut("sessions").and_then(Value::as_object_mut) {
                s.retain(|_, v| v.as_str() != Some(owned.as_str()));
            }
            Ok(())
        })?;
        let folder = self.config.join("projects").join(id);
        if folder.exists() {
            fs::remove_dir_all(&folder)?;
        }
        Ok(())
    }

    /// Whether the user was already told that project notes need one more hook (a one-time notice).
    pub fn hook_told(&self) -> bool {
        self.read().ok().flatten().and_then(|r| r.get("notes_hook_told").and_then(Value::as_bool)).unwrap_or(false)
    }

    pub fn set_hook_told(&mut self) -> io::Result<()> {
        self.change(|root| {
            root.insert("notes_hook_told".into(), json!(true));
            Ok(())
        })
    }
}

fn drop_archived_session(root: &mut Map<String, Value>, project: &str, session: &str) {
    let list = root
        .get_mut("projects")
        .and_then(|p| p.get_mut(project))
        .and_then(|p| p.get_mut("archived_sessions"))
        .and_then(Value::as_array_mut);
    if let Some(list) = list {
        list.retain(|v| v.as_str() != Some(session));
    }
}

// ---- sessions started under a project, before platoon knows their id ------------------------------------------

/// A session platoon (or `platoon open --project`) started under a project: its claude's pid, when, and the session
/// ids already placed from it (so a session later moved out by the user is not put back). Kept in
/// `<cache>/project-launches/<pid>.json` until the process has gone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub pid: u32,
    pub project: String,
    /// Milliseconds since the epoch, a little before the process started.
    pub at_ms: u64,
    pub placed: Vec<String>,
}

fn launches_dir(cache: &Path) -> PathBuf {
    cache.join("project-launches")
}

fn write_launch(cache: &Path, l: &Launch) -> io::Result<()> {
    let v = json!({"project": l.project, "at_ms": l.at_ms, "placed": l.placed});
    write_atomic(&launches_dir(cache).join(format!("{}.json", l.pid)), &v.to_string())
}

/// Remember that `pid` was started under `project`.
pub fn note_launch(cache: &Path, pid: u32, project: &str, at_ms: u64) -> io::Result<()> {
    write_launch(cache, &Launch { pid, project: project.to_string(), at_ms, placed: Vec::new() })
}

pub fn launches(cache: &Path) -> Vec<Launch> {
    let mut out = Vec::new();
    for e in fs::read_dir(launches_dir(cache)).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(pid) = name.strip_suffix(".json").and_then(|p| p.parse::<u32>().ok()) else { continue };
        let Some(v) = fs::read_to_string(e.path()).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { continue };
        let Some(project) = v.get("project").and_then(Value::as_str) else { continue };
        let placed = v.get("placed").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        out.push(Launch { pid, project: project.to_string(), at_ms: v.get("at_ms").and_then(Value::as_u64).unwrap_or(0), placed });
    }
    out
}

/// Match launches with the live processes (pid, session id, started ms): a session id not placed from that launch
/// yet goes into the project (unless it is in one already), and a launch whose process has gone is forgotten.
/// `alive` says whether a pid still runs. Gives back (session, project) for every session actually placed, so the
/// caller can tell the notes helper that the project has something new.
pub fn resolve_launches(cache: &Path, projects: &mut Projects, live: &[(u32, String, u64)], alive: &dyn Fn(u32) -> bool) -> Vec<(String, String)> {
    let mut placed: Vec<(String, String)> = Vec::new();
    for mut l in launches(cache) {
        let path = launches_dir(cache).join(format!("{}.json", l.pid));
        if !alive(l.pid) {
            let _ = fs::remove_file(&path);
            continue;
        }
        let mut dirty = false;
        for (pid, id, started) in live {
            // A pid reused by another process that started before the launch is not ours.
            if *pid != l.pid || l.placed.contains(id) || (*started > 0 && started + 5_000 < l.at_ms) {
                continue;
            }
            if projects.place(id, &l.project).unwrap_or(false) {
                placed.push((id.clone(), l.project.clone()));
            }
            l.placed.push(id.clone());
            dirty = true;
        }
        if dirty {
            let _ = write_launch(cache, &l);
        }
    }
    placed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-projects-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn saved(d: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(d.join("projects.json")).unwrap()).unwrap()
    }

    #[test]
    fn a_project_is_created_with_a_name_a_color_and_a_badge_and_survives_a_new_run() {
        let d = dir("create");
        let mut p = Projects::open(&d);
        assert!(p.all().is_empty() && !d.exists(), "reading creates nothing");
        let a = p.create("  Platoon   app ").unwrap();
        let b = p.create("website").unwrap();
        assert_ne!(a, b);
        let pa = p.get(&a).unwrap();
        assert_eq!((pa.name.as_str(), pa.badge.as_str(), pa.color.as_str(), pa.archived), ("Platoon app", "PA", "accent", false));
        assert_eq!((p.get(&b).unwrap().badge.as_str(), p.get(&b).unwrap().color.as_str()), ("WE", "blue"), "the next free color");
        assert!(p.create("   ").is_err(), "a name is required");
        let again = Projects::open(&d);
        assert_eq!(again.all().len(), 2);
        assert_eq!(again.all()[0].id, a, "oldest first");
        assert!(!fs::read_dir(&d).unwrap().flatten().any(|e| e.file_name().to_string_lossy().ends_with(".tmp")));
    }

    #[test]
    fn badges_are_the_first_letters_and_can_be_changed() {
        assert_eq!(default_badge("my web app"), "MW");
        assert_eq!(default_badge("x"), "X");
        assert_eq!(default_badge("éclair"), "ÉC");
        assert_eq!(default_badge("--"), "P");
        let d = dir("look");
        let mut p = Projects::open(&d);
        let a = p.create("alpha").unwrap();
        p.set_look(&a, "green", " a b c d ").unwrap();
        assert_eq!((p.get(&a).unwrap().color.as_str(), p.get(&a).unwrap().badge.as_str()), ("green", "abc"));
        p.set_look(&a, "red", "").unwrap();
        assert_eq!(p.get(&a).unwrap().badge, "AL", "empty goes back to the name's letters");
        p.rename(&a, "Beta").unwrap();
        assert_eq!(p.get(&a).unwrap().name, "Beta");
        assert!(p.rename(&a, " ").is_err());
    }

    #[test]
    fn a_session_is_in_at_most_one_project() {
        let d = dir("members");
        let mut p = Projects::open(&d);
        let (a, b) = (p.create("a").unwrap(), p.create("b").unwrap());
        p.assign("s1", Some(&a)).unwrap();
        p.assign("s2", Some(&a)).unwrap();
        p.assign("s1", Some(&b)).unwrap();
        assert_eq!(p.project_id_of("s1"), Some(b.as_str()));
        assert_eq!(p.members_of(&a), ["s2"]);
        assert_eq!(saved(&d)["sessions"].as_object().unwrap().len(), 2, "one entry per session");
        p.assign("s2", None).unwrap();
        assert!(p.of("s2").is_none() && p.members_of(&a).is_empty());
        assert!(p.assign("s3", Some("nope")).is_err(), "no such project");
        // Placing never overrides a choice.
        assert!(!p.place("s1", &a).unwrap());
        assert!(p.place("s4", &a).unwrap());
        assert_eq!(p.project_id_of("s4"), Some(a.as_str()));
    }

    #[test]
    fn archiving_a_project_and_restoring_it_brings_back_exactly_the_sessions_it_archived() {
        let d = dir("archive");
        let mut p = Projects::open(&d);
        let a = p.create("a").unwrap();
        for s in ["s1", "s2", "s3", "old"] {
            p.assign(s, Some(&a)).unwrap();
        }
        // "old" was archived before; the project archives s1..s3.
        p.archive(&a, &["s1".into(), "s2".into(), "s3".into()]).unwrap();
        assert!(p.get(&a).unwrap().archived);
        // One of them is restored on its own: it stays in the project, the project stays archived.
        p.session_restored("s2").unwrap();
        assert!(p.get(&a).unwrap().archived && p.project_id_of("s2") == Some(a.as_str()));
        // One is moved to another project meanwhile.
        let b = p.create("b").unwrap();
        p.assign("s3", Some(&b)).unwrap();
        let back = p.restore(&a).unwrap();
        assert_eq!(back, ["s1"]);
        assert!(!p.get(&a).unwrap().archived && p.get(&a).unwrap().archived_sessions.is_empty());
        assert_eq!(Projects::open(&d).get(&a).unwrap().archived, false);
    }

    #[test]
    fn deleting_a_project_unassigns_its_sessions_deletes_the_notes_and_keeps_one_backup() {
        let d = dir("delete");
        let cache = d.join("cache");
        let mut p = Projects::open(&d);
        let (a, b) = (p.create("a").unwrap(), p.create("b").unwrap());
        p.assign("s1", Some(&a)).unwrap();
        p.assign("s2", Some(&b)).unwrap();
        let notes = p.notes_path(&a);
        fs::create_dir_all(notes.parent().unwrap()).unwrap();
        fs::write(&notes, "# notes\nkeep me").unwrap();
        fs::write(crate::tickets::path_in(&d, &a), r#"{"next":2,"tickets":[]}"#).unwrap();
        fs::create_dir_all(crate::tickets::bodies_dir(&d, &a)).unwrap();
        fs::write(crate::tickets::bodies_dir(&d, &a).join("01-one.md"), "body").unwrap();
        p.delete(&a, &cache).unwrap();
        assert!(p.get(&a).is_none() && p.of("s1").is_none() && p.project_id_of("s2") == Some(b.as_str()));
        assert!(!d.join("projects").join(&a).exists(), "the notes folder is gone");
        let bak = cache.join("deleted-projects").join(&a);
        assert_eq!(fs::read_to_string(bak.join("project.md")).unwrap(), "# notes\nkeep me");
        assert_eq!(fs::read_to_string(bak.join("tickets.json")).unwrap(), r#"{"next":2,"tickets":[]}"#);
        assert_eq!(fs::read_to_string(bak.join("tickets").join("01-one.md")).unwrap(), "body");
        let entry: Value = serde_json::from_str(&fs::read_to_string(bak.join("project.json")).unwrap()).unwrap();
        assert_eq!((entry["project"]["name"].as_str(), entry["sessions"].clone()), (Some("a"), json!(["s1"])));
        assert!(!saved(&d)["sessions"].as_object().unwrap().contains_key("s1"));
        assert!(p.delete("../etc", &cache).is_err(), "never a path");
    }

    #[test]
    fn everything_in_the_file_that_is_not_ours_survives_a_change() {
        let d = dir("keep");
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("projects.json"),
            r#"{"note":"mine","version":3,"projects":{"pold":{"name":"Old","color":"teal","badge":"OL","archived":false,"created":5,"archived_sessions":[],"icon":"star"},"weird":7},"sessions":{"x":"pold","y":"gone"},"tail":[1]}"#,
        )
        .unwrap();
        let mut p = Projects::open(&d);
        assert_eq!(p.all().len(), 1);
        assert_eq!(p.get("pold").unwrap().color, "teal", "a color a newer version knows is kept");
        assert!(p.of("y").is_none(), "a session pointing at a project that isn't there is in none");
        p.rename("pold", "Older").unwrap();
        p.create("new").unwrap();
        let v = saved(&d);
        assert_eq!((v["note"].clone(), v["version"].clone(), v["tail"].clone()), (json!("mine"), json!(3), json!([1])));
        assert_eq!(v["projects"]["pold"]["icon"], "star");
        assert_eq!(v["projects"]["weird"], 7);
        assert_eq!(v["sessions"]["y"], "gone");
        assert!(!d.join("projects.json.bak").exists());
    }

    #[test]
    fn a_corrupt_file_reads_as_no_projects_and_is_backed_up_before_being_replaced() {
        let d = dir("corrupt");
        fs::create_dir_all(&d).unwrap();
        for bad in ["{ nope", "[]", r#"{"projects":[1]}"#, r#"{"sessions":"x"}"#] {
            fs::write(d.join("projects.json"), bad).unwrap();
            let p = Projects::open(&d);
            assert!(p.is_corrupt() && p.all().is_empty(), "{bad}");
            assert_eq!(fs::read_to_string(d.join("projects.json")).unwrap(), bad, "reading never changes it");
        }
        let mut p = Projects::open(&d);
        p.create("fresh").unwrap();
        assert_eq!(fs::read_to_string(d.join("projects.json.bak")).unwrap(), r#"{"sessions":"x"}"#);
        assert!(!Projects::open(&d).is_corrupt());
    }

    #[test]
    fn a_second_window_sees_changes_and_none_are_lost() {
        let d = dir("two");
        let (mut a, mut b) = (Projects::open(&d), Projects::open(&d));
        let pa = a.create("one").unwrap();
        b.create("two").unwrap(); // b starts from the file, so "one" survives
        assert_eq!(b.all().len(), 2);
        b.assign("s", Some(&pa)).unwrap();
        assert!(a.reload_if_changed());
        assert_eq!(a.project_id_of("s"), Some(pa.as_str()));
        assert!(!a.reload_if_changed());
    }

    #[test]
    fn many_writers_at_once_lose_nothing() {
        let d = dir("many");
        let id = Projects::open(&d).create("p").unwrap();
        let threads: Vec<_> = (0..16)
            .map(|i| {
                let (d, id) = (d.clone(), id.clone());
                std::thread::spawn(move || {
                    let mut p = Projects::open(&d);
                    for k in 0..5 {
                        p.place(&format!("s{i}-{k}"), &id).unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let p = Projects::open(&d);
        assert!(!p.is_corrupt());
        assert_eq!(p.members_of(&id).len(), 80);
    }

    #[test]
    fn a_session_started_under_a_project_is_placed_once_its_id_is_known_and_never_put_back() {
        let d = dir("launch");
        let cache = d.join("cache");
        let mut p = Projects::open(&d);
        let a = p.create("a").unwrap();
        note_launch(&cache, 4242, &a, 10_000).unwrap();
        let alive = |pid: u32| pid == 4242;
        assert!(resolve_launches(&cache, &mut p, &[], &alive).is_empty(), "nothing registered yet");
        assert_eq!(resolve_launches(&cache, &mut p, &[(4242, "new".into(), 10_500)], &alive), [("new".to_string(), a.clone())]);
        assert_eq!(p.project_id_of("new"), Some(a.as_str()));
        // The user moves it out: it is not put back while the process runs.
        p.assign("new", None).unwrap();
        assert!(resolve_launches(&cache, &mut p, &[(4242, "new".into(), 10_500)], &alive).is_empty());
        assert!(p.of("new").is_none());
        // A /clear gives the same process a new id: that one goes in too.
        assert_eq!(resolve_launches(&cache, &mut p, &[(4242, "cleared".into(), 10_500)], &alive), [("cleared".to_string(), a.clone())]);
        // An unrelated process that started long before, reusing the pid, is not ours.
        note_launch(&cache, 77, &a, 1_000_000).unwrap();
        assert!(resolve_launches(&cache, &mut p, &[(77, "other".into(), 10)], &|_| true).is_empty());
        // A launch whose process has gone is forgotten.
        resolve_launches(&cache, &mut p, &[], &|_| false);
        assert!(launches(&cache).is_empty());
    }

    #[test]
    fn slots_follow_the_theme_and_terminal_colors_use_the_terminals_own() {
        let t = Theme::by_id("tokyo-night").unwrap();
        assert_eq!(slot_color(&t, "accent"), t.accent);
        assert_eq!(slot_color(&t, "blue"), t.ansi.unwrap()[12]);
        assert_eq!(slot_color(&t, "something-new"), t.accent);
        let term = Theme::terminal();
        assert_eq!(slot_color(&term, "green"), Color::Indexed(10));
        assert!(SLOTS.iter().all(|(s, _)| slot_color(&t, s) != t.bg));
    }
}
