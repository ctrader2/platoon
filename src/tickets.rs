//! Tickets: one piece of work in a project. A ticket has a number (per project, from 1, never used twice), a title,
//! a body in Markdown (a file of its own the user edits; it also says how far to take the work), the folders it works
//! in, the tickets it waits for (`blocked_by`) and a state. Only platoon ever writes a ticket's state (for the user, or
//! for the squad working on it).
//!
//! A project's tickets live beside its notes:
//! `<config>/projects/<id>/tickets.json` = `{"next": 4, "tickets": [{"number", "title", "file", "state", "folders",
//! "blocked_by", "created", "updated", ...}]}` and the bodies in `<config>/projects/<id>/tickets/<NN>-<slug>.md`
//! (`file` holds that name; a rename of the title keeps the file).
//!
//! Like platoon's other files: writes are atomic (temp file + rename) and happen under a lock (`tickets.lock` beside
//! the file: the app and `platoon ticket add` can both write), keys platoon doesn't know are kept (on the file and on
//! each ticket; a newer platoon will add some), a state it doesn't know is kept and shown as written, and a file that
//! can't be understood is never written over: every change is refused until the user has looked at it.

use crate::projects::{lock, write_atomic};
use serde_json::{json, Map, Value};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// The longest title kept, in characters.
pub const TITLE_MAX: usize = 100;

/// What a new ticket's body starts from: what should work when it is finished, and how to tell.
pub const TEMPLATE: &str = "\
## What it should do

What should work when this is finished, as behaviour, not a list of changes. It should fit in one fresh session.

How far: local changes only (say if you want a commit, or a push and merge request).

## Acceptance

- [ ] \n- [ ] \n";

// ---- states -------------------------------------------------------------------------------------------------------

/// Where a ticket is. The first six are platoon's (in this order); anything else is a state a newer platoon wrote,
/// kept as it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    New,
    Working,
    Waiting,
    Ready,
    Done,
    Cancelled,
    Other(String),
}

/// (what the file holds, what the screen says), in the order the states come in.
const STATE_NAMES: [(&str, &str); 6] = [
    ("new", "New"),
    ("working", "Working"),
    ("waiting", "Waiting on you"),
    ("ready", "Ready to push/publish"),
    ("done", "Done"),
    ("cancelled", "Cancelled"),
];

/// The states the user can pick by hand on a ticket no squad has had (the others belong to squads).
pub const USER_STATES: [State; 3] = [State::New, State::Done, State::Cancelled];

/// The states the user can pick on a ticket that has a squad's leader: Working continues with that squad.
pub const LEADER_STATES: [State; 4] = [State::New, State::Working, State::Done, State::Cancelled];

impl State {
    pub fn from_id(id: &str) -> State {
        match id {
            "new" => State::New,
            "working" => State::Working,
            "waiting" => State::Waiting,
            "ready" => State::Ready,
            "done" => State::Done,
            "cancelled" => State::Cancelled,
            other => State::Other(other.to_string()),
        }
    }

    fn index(&self) -> Option<usize> {
        match self {
            State::New => Some(0),
            State::Working => Some(1),
            State::Waiting => Some(2),
            State::Ready => Some(3),
            State::Done => Some(4),
            State::Cancelled => Some(5),
            State::Other(_) => None,
        }
    }

    /// What the file holds.
    pub fn id(&self) -> &str {
        match self {
            State::Other(s) => s,
            s => STATE_NAMES[s.index().unwrap_or(0)].0,
        }
    }

    /// What the screen says: platoon's words, or a state it doesn't know as it was written.
    pub fn label(&self) -> &str {
        match self {
            State::Other(s) => s,
            s => STATE_NAMES[s.index().unwrap_or(0)].1,
        }
    }

    /// Done and Cancelled are listed apart from the rest.
    pub fn closed(&self) -> bool {
        matches!(self, State::Done | State::Cancelled)
    }

    /// The user's go-ahead to publish lasts only while the squad is at it: Working or Ready.
    pub fn keeps_go_ahead(&self) -> bool {
        matches!(self, State::Working | State::Ready)
    }
}

// ---- a ticket -----------------------------------------------------------------------------------------------------

/// A squad's leader session, started for a ticket. A ticket keeps every one it had, in order; the last is the one
/// working on it now (a ticket handed to another squad after Done gets that squad's own leader).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leader {
    /// The squad's id (`squads::Squad::id`).
    pub squad: String,
    /// The leader's Claude Code session id, chosen by platoon when it started it.
    pub session: String,
    /// Seconds since the epoch.
    pub started: u64,
    /// The names its session's agents were given when it started. A session is resumed with the names it began
    /// with, so a leader started before the names carried the squad's keeps its plain ones.
    pub names: AgentNames,
    /// The file beside the ticket's body that holds what it was started with (`<NN>-squad-<k>.json`): continued
    /// later, it runs with the squad as it was then. None for a leader started before there were snapshots (it
    /// continues with the squad as it is now).
    pub snapshot: Option<String>,
}

/// How a leader session's agents (`--agents`, `--agent`) are named.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AgentNames {
    /// `leader`, `planner`, `builder`, ...: the leaders started before 2026-09-26. The file has no `names` for them.
    Plain,
    /// `coding-leader`, `coding-planner`, ...: the squad's id, then the role, so Claude Code shows whose squad it is.
    /// The file says `"names": "squad-role"`.
    #[default]
    SquadRole,
}

impl AgentNames {
    fn id(&self) -> Option<&'static str> {
        match self {
            AgentNames::Plain => None,
            AgentNames::SquadRole => Some("squad-role"),
        }
    }

    /// What the file says; a leader with no `names` (or one this platoon doesn't know) was started with plain ones.
    fn from_id(id: Option<&str>) -> AgentNames {
        match id {
            Some("squad-role") => AgentNames::SquadRole,
            _ => AgentNames::Plain,
        }
    }
}

/// Why a hand-over to a squad didn't happen, written on the ticket (it stays New) so the user sees it there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandoverKind {
    /// The commander checked and said the squad doesn't fit. The user may overrule it: the same squad again asks
    /// "Hand it anyway?".
    Refused,
    /// The commander couldn't run (the daily limit, a back-off, the AI features switched off) or its check failed.
    NoCheck,
    /// Something after the check went wrong (the worktree, starting the session).
    NotStarted,
}

impl HandoverKind {
    fn id(&self) -> &'static str {
        match self {
            HandoverKind::Refused => "refused",
            HandoverKind::NoCheck => "no-check",
            HandoverKind::NotStarted => "not-started",
        }
    }

    fn from_id(id: &str) -> HandoverKind {
        match id {
            "refused" => HandoverKind::Refused,
            "no-check" => HandoverKind::NoCheck,
            _ => HandoverKind::NotStarted,
        }
    }
}

/// The last hand-over to a squad that didn't happen: which squad, why not, and what the ticket says about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Handover {
    pub squad: String,
    pub kind: HandoverKind,
    pub says: String,
}

/// What the ticket says while its leader is stuck on Claude's permission prompt.
pub const PERMISSION_REASON: &str = "Waiting for your OK on a command";

/// A git worktree platoon made for a ticket (Coding tickets always get their own).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    /// The ticket's folder it stands in for.
    pub folder: String,
    /// Where the squad works instead: the same place inside the worktree.
    pub path: String,
    pub branch: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub number: u32,
    pub title: String,
    /// The body's file name inside the project's `tickets/` folder.
    pub file: String,
    pub state: State,
    /// Absolute paths.
    pub folders: Vec<String>,
    /// Numbers of tickets in the same project that must be Done first.
    pub blocked_by: Vec<u32>,
    /// Seconds since the epoch.
    pub created: u64,
    pub updated: u64,
    /// The squads' leader sessions it had, oldest first.
    pub leaders: Vec<Leader>,
    pub worktrees: Vec<Worktree>,
    /// What the squad said when it stopped (its question, why it is stuck, what it wants to publish), in the words
    /// the screen shows. Empty when there is nothing to say.
    pub reason: String,
    /// The user let the squad publish (push, merge request, a page): lasts while the ticket is Working or Ready.
    pub go_ahead: bool,
    /// The last hand-over to a squad that didn't happen, and why (cleared when a leader starts).
    pub handover: Option<Handover>,
    /// Waiting on you because the leader is stuck on Claude's permission prompt (not a question: the next tool it
    /// runs, after the user's answer, makes it Working again).
    pub permission: bool,
    /// What the app is doing for it before a leader is written on it (the commander's check, the worktrees): the
    /// ticket shows Working with these words. Never in the file.
    pub busy: Option<String>,
}

impl Ticket {
    /// The leader working on it now: the last one it was handed to.
    pub fn leader(&self) -> Option<&Leader> {
        self.leaders.last()
    }

    /// Where a squad works: each folder, or its worktree when it has one.
    pub fn work_folders(&self) -> Vec<String> {
        self.folders.iter().map(|f| self.worktrees.iter().find(|w| w.folder == *f).map_or_else(|| f.clone(), |w| w.path.clone())).collect()
    }
}

/// One line, trimmed, at most `max` characters.
pub fn clean(text: &str, max: usize) -> String {
    crate::projects::clean(text, max)
}

/// The part of a body file's name after the number: the title in lower case, words joined with `-`, plain letters
/// and digits only.
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 40 {
            break;
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "ticket".into() } else { out }
}

/// The tickets `t` still waits for: those it is blocked by that exist and are not Done.
pub fn waiting_for(t: &Ticket, all: &[Ticket]) -> Vec<u32> {
    t.blocked_by.iter().copied().filter(|n| all.iter().any(|o| o.number == *n && o.state != State::Done)).collect()
}

/// A ticket is ready to be worked on when it is New and everything it is blocked by is Done. Only Done lets a ticket
/// go: one waiting for a Cancelled ticket stays held up until the user takes that wait away (decided 2026-09-26).
pub fn is_ready(t: &Ticket, all: &[Ticket]) -> bool {
    t.state == State::New && waiting_for(t, all).is_empty()
}

/// Would "`ticket` is blocked by `blocker`" make a loop (including a ticket blocking itself)? True when `ticket` can
/// already be reached from `blocker` by following what each ticket is blocked by.
pub fn would_loop(all: &[Ticket], ticket: u32, blocker: u32) -> bool {
    let mut seen: Vec<u32> = Vec::new();
    let mut todo = vec![blocker];
    while let Some(n) = todo.pop() {
        if n == ticket {
            return true;
        }
        if seen.contains(&n) {
            continue;
        }
        seen.push(n);
        if let Some(t) = all.iter().find(|t| t.number == n) {
            todo.extend(t.blocked_by.iter().copied());
        }
    }
    false
}

/// Open tickets first (by number), then the Done and Cancelled ones (by number).
pub fn grouped(all: &[Ticket]) -> (Vec<&Ticket>, Vec<&Ticket>) {
    let mut open: Vec<&Ticket> = all.iter().filter(|t| !t.state.closed()).collect();
    let mut closed: Vec<&Ticket> = all.iter().filter(|t| t.state.closed()).collect();
    open.sort_by_key(|t| t.number);
    closed.sort_by_key(|t| t.number);
    (open, closed)
}

fn now_secs() -> u64 {
    crate::usage::now()
}

fn ticket_from(v: &Value) -> Option<Ticket> {
    let o = v.as_object()?;
    let number = u32::try_from(o.get("number")?.as_u64()?).ok().filter(|n| *n > 0)?;
    let strings = |k: &str| -> Vec<String> {
        o.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
    };
    Some(Ticket {
        number,
        title: o.get("title").and_then(Value::as_str).map(|t| clean(t, TITLE_MAX)).unwrap_or_default(),
        file: o.get("file").and_then(Value::as_str).unwrap_or_default().to_string(),
        state: State::from_id(o.get("state").and_then(Value::as_str).unwrap_or("new")),
        folders: strings("folders"),
        blocked_by: o
            .get("blocked_by")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|x| x.as_u64().and_then(|n| u32::try_from(n).ok())).collect())
            .unwrap_or_default(),
        created: o.get("created").and_then(Value::as_u64).unwrap_or(0),
        updated: o.get("updated").and_then(Value::as_u64).unwrap_or(0),
        leaders: o
            .get("leaders")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|l| {
                        let session = l.get("session")?.as_str()?.to_string();
                        (!session.is_empty()).then(|| Leader {
                            squad: l.get("squad").and_then(Value::as_str).unwrap_or_default().to_string(),
                            session,
                            started: l.get("started").and_then(Value::as_u64).unwrap_or(0),
                            names: AgentNames::from_id(l.get("names").and_then(Value::as_str)),
                            snapshot: l.get("snapshot").and_then(Value::as_str).filter(|f| is_snapshot_file(f)).map(String::from),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        worktrees: o
            .get("worktrees")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|w| {
                        let s = |k: &str| w.get(k).and_then(Value::as_str).map(String::from);
                        Some(Worktree { folder: s("folder")?, path: s("path")?, branch: s("branch").unwrap_or_default() })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        reason: o.get("reason").and_then(Value::as_str).unwrap_or_default().to_string(),
        go_ahead: o.get("go_ahead").and_then(Value::as_bool).unwrap_or(false),
        handover: o.get("handover").and_then(|h| {
            Some(Handover {
                squad: h.get("squad")?.as_str()?.to_string(),
                kind: HandoverKind::from_id(h.get("kind").and_then(Value::as_str).unwrap_or("")),
                says: h.get("says").and_then(Value::as_str).unwrap_or_default().to_string(),
            })
        }),
        permission: o.get("permission").and_then(Value::as_bool).unwrap_or(false),
        busy: None,
    })
}

/// What a new ticket is made of.
#[derive(Clone, Debug, Default)]
pub struct NewTicket {
    pub title: String,
    /// None: the template.
    pub body: Option<String>,
    pub folders: Vec<String>,
    pub blocked_by: Vec<u32>,
}

/// Where a project's tickets file is.
pub fn path_in(config: &Path, project: &str) -> PathBuf {
    config.join("projects").join(project).join("tickets.json")
}

/// Where a project's ticket bodies are.
pub fn bodies_dir(config: &Path, project: &str) -> PathBuf {
    config.join("projects").join(project).join("tickets")
}

/// What `tickets.json` says the user must look at, in the words the pane shows.
pub const CORRUPT: &str = "tickets.json can't be read, so platoon leaves it alone. Fix or move the file to go on.";

/// The tickets of one project, as the file says.
pub struct Tickets {
    config: PathBuf,
    project: String,
    list: Vec<Ticket>,
    next: u32,
    stamp: Option<(SystemTime, u64)>,
    corrupt: bool,
    /// Bumped whenever what is in memory changed.
    pub generation: u64,
    /// Tickets the app is busy handing to a squad (see `Ticket::busy`): shown Working, never written.
    busy: std::collections::BTreeMap<u32, String>,
}

impl Tickets {
    pub fn open(config: &Path, project: &str) -> Self {
        let mut t = Self {
            config: config.to_path_buf(),
            project: project.to_string(),
            list: Vec::new(),
            next: 1,
            stamp: None,
            corrupt: false,
            generation: 0,
            busy: std::collections::BTreeMap::new(),
        };
        t.load();
        t
    }

    pub fn project(&self) -> &str {
        &self.project
    }

    pub fn path(&self) -> PathBuf {
        path_in(&self.config, &self.project)
    }

    /// The file is there but can't be understood: nothing is shown and nothing is written.
    pub fn is_corrupt(&self) -> bool {
        self.corrupt
    }

    /// Every ticket, in the order of the file.
    pub fn all(&self) -> &[Ticket] {
        &self.list
    }

    pub fn get(&self, number: u32) -> Option<&Ticket> {
        self.list.iter().find(|t| t.number == number)
    }

    /// The full path of a ticket's body.
    pub fn body_path(&self, t: &Ticket) -> PathBuf {
        bodies_dir(&self.config, &self.project).join(&t.file)
    }

    /// The body as it is on disk now ("" when there is none).
    pub fn body(&self, t: &Ticket) -> String {
        if t.file.is_empty() {
            return String::new();
        }
        fs::read_to_string(self.body_path(t)).unwrap_or_default()
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
            Ok(Value::Object(o)) if o.get("tickets").is_none_or(Value::is_array) && o.get("next").is_none_or(Value::is_u64) => Ok(Some(o)),
            _ => Err(()),
        }
    }

    fn load(&mut self) {
        self.stamp = self.stamp_now();
        self.list.clear();
        self.next = 1;
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
        // Anything in the list that isn't shaped like a ticket is somebody else's: kept on disk, not shown.
        self.list = root.get("tickets").and_then(Value::as_array).into_iter().flatten().filter_map(ticket_from).collect();
        self.next = next_number(&root);
        self.apply_busy();
    }

    fn apply_busy(&mut self) {
        for t in &mut self.list {
            t.busy = self.busy.get(&t.number).cloned();
            if t.busy.is_some() {
                t.state = State::Working;
            }
        }
    }

    /// The app is busy handing ticket `number` to a squad (`Some(what it is doing)`), or no longer is (None). Only
    /// what this store shows changes: the file doesn't.
    pub fn set_busy(&mut self, number: u32, what: Option<&str>) {
        let changed = match what {
            Some(w) => self.busy.insert(number, w.to_string()).as_deref() != Some(w),
            None => self.busy.remove(&number).is_some(),
        };
        if changed {
            // From the file again, so a ticket that is no longer busy shows its real state.
            self.load();
        }
    }

    /// Pick up a change made by `platoon ticket add` or another window. One `stat` when nothing changed.
    pub fn reload_if_changed(&mut self) -> bool {
        if self.stamp_now() == self.stamp {
            return false;
        }
        self.load();
        true
    }

    /// Change the file: under the lock, from what is on disk now, keeping everything else in it. `f` returns Err to
    /// change nothing. A file that can't be understood is never replaced.
    fn change<T>(&mut self, f: impl FnOnce(&mut Map<String, Value>, &[Ticket]) -> Result<T, String>) -> io::Result<T> {
        let _lock = lock(&self.path())?;
        let mut root = match self.read() {
            Ok(o) => o.unwrap_or_default(),
            Err(()) => {
                self.load();
                return Err(io::Error::other(CORRUPT));
            }
        };
        if !root.get("tickets").is_some_and(Value::is_array) {
            root.insert("tickets".into(), json!([]));
        }
        let now: Vec<Ticket> = root.get("tickets").and_then(Value::as_array).into_iter().flatten().filter_map(ticket_from).collect();
        let out = match f(&mut root, &now) {
            Ok(v) => v,
            Err(e) => {
                self.load();
                return Err(io::Error::other(e));
            }
        };
        let text = serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_default();
        let written = write_atomic(&self.path(), &text);
        self.load();
        written.map(|()| out)
    }

    /// A new ticket: the next number, state New, its body file written. Returns the number.
    pub fn add(&mut self, new: NewTicket) -> io::Result<u32> {
        let title = clean(&new.title, TITLE_MAX);
        if title.is_empty() {
            return Err(io::Error::other("a ticket needs a title"));
        }
        let dir = bodies_dir(&self.config, &self.project);
        let body = new.body.unwrap_or_else(|| TEMPLATE.to_string());
        let now = now_secs();
        let written_file: std::cell::RefCell<Option<PathBuf>> = std::cell::RefCell::new(None);
        let result = self.change(|root, all| {
            for b in &new.blocked_by {
                if !all.iter().any(|t| t.number == *b) {
                    return Err(format!("there is no ticket #{b} in this project"));
                }
            }
            let number = next_number(root).max(all.iter().map(|t| t.number + 1).max().unwrap_or(1));
            let stem = format!("{number:02}-{}", slug(&title));
            let mut file = format!("{stem}.md");
            let mut k = 2;
            while dir.join(&file).exists() {
                file = format!("{stem}-{k}.md");
                k += 1;
            }
            fs::create_dir_all(&dir).map_err(|e| format!("couldn't make the tickets folder: {e}"))?;
            write_atomic(&dir.join(&file), &body).map_err(|e| format!("couldn't write the ticket: {e}"))?;
            *written_file.borrow_mut() = Some(dir.join(&file));
            let mut blocked = new.blocked_by.clone();
            blocked.sort_unstable();
            blocked.dedup();
            let t = json!({
                "number": number,
                "title": title,
                "file": file,
                "state": State::New.id(),
                "folders": new.folders,
                "blocked_by": blocked,
                "created": now,
                "updated": now,
            });
            root.get_mut("tickets").and_then(Value::as_array_mut).ok_or("tickets")?.push(t);
            root.insert("next".into(), json!(number + 1));
            Ok(number)
        });
        // The list didn't take it: the body file it wrote goes too, so nothing is left without a ticket.
        if result.is_err() {
            if let Some(p) = written_file.take() {
                let _ = fs::remove_file(p);
            }
        }
        result
    }

    /// Change one ticket's own object in the file (its `updated` time too).
    fn edit(&mut self, number: u32, f: impl FnOnce(&mut Map<String, Value>, &[Ticket]) -> Result<(), String>) -> io::Result<()> {
        let now = now_secs();
        self.change(move |root, all| {
            let t = root
                .get_mut("tickets")
                .and_then(Value::as_array_mut)
                .and_then(|a| a.iter_mut().find(|t| t.get("number").and_then(Value::as_u64) == Some(u64::from(number))))
                .and_then(Value::as_object_mut)
                .ok_or_else(|| format!("there is no ticket #{number} any more"))?;
            f(t, all)?;
            t.insert("updated".into(), json!(now));
            Ok(())
        })
    }

    pub fn rename(&mut self, number: u32, title: &str) -> io::Result<()> {
        let title = clean(title, TITLE_MAX);
        if title.is_empty() {
            return Err(io::Error::other("a ticket needs a title"));
        }
        self.edit(number, |t, _| {
            t.insert("title".into(), json!(title));
            Ok(())
        })
    }

    /// The user moves a ticket along. What the squad last said goes (it was about the state being left), and so
    /// does the go-ahead to publish unless the ticket stays with the squad (Working or Ready).
    pub fn set_state(&mut self, number: u32, state: &State) -> io::Result<()> {
        let id = state.id().to_string();
        let keep = state.keeps_go_ahead();
        self.edit(number, |t, _| {
            t.insert("state".into(), json!(id));
            t.remove("reason");
            t.remove("handover");
            t.remove("permission");
            if !keep {
                t.remove("go_ahead");
            }
            Ok(())
        })
    }

    /// A hand-over to `squad` didn't happen: the ticket is New, with why not written on it.
    pub fn handover_failed(&mut self, number: u32, squad: &str, kind: HandoverKind, says: &str) -> io::Result<()> {
        let h = json!({"squad": squad, "kind": kind.id(), "says": says});
        self.edit(number, move |t, _| {
            t.insert("state".into(), json!(State::New.id()));
            t.insert("handover".into(), h);
            t.remove("reason");
            t.remove("permission");
            t.remove("go_ahead");
            Ok(())
        })
    }

    /// `platoon hook squad-notify`: the leader is stuck on Claude's permission prompt. A Working ticket becomes
    /// Waiting on you ("Waiting for your OK on a command"); any other state is left alone. True when it changed.
    pub fn permission_wait(&mut self, number: u32) -> io::Result<bool> {
        self.edit_when(number, |t| {
            if t.get("state").and_then(Value::as_str) != Some(State::Working.id()) {
                return false;
            }
            t.insert("state".into(), json!(State::Waiting.id()));
            t.insert("reason".into(), json!(PERMISSION_REASON));
            t.insert("permission".into(), json!(true));
            true
        })
    }

    /// `platoon hook squad-tool`: a tool ran after the user answered the permission prompt, so the work goes on.
    /// Only a ticket Waiting *for a permission* becomes Working again; one waiting on a question or stuck is left
    /// for the leader's next turn to decide. True when it changed.
    pub fn permission_answered(&mut self, number: u32) -> io::Result<bool> {
        self.edit_when(number, |t| {
            let waiting = t.get("state").and_then(Value::as_str) == Some(State::Waiting.id());
            if !(waiting && t.get("permission").and_then(Value::as_bool) == Some(true)) {
                return false;
            }
            t.insert("state".into(), json!(State::Working.id()));
            t.remove("reason");
            t.remove("permission");
            true
        })
    }

    /// Change one ticket only when `f` says so (it returns false having changed nothing): nothing is written
    /// otherwise, not even the time. The hooks that run after every tool call use this, so a ticket with nothing to
    /// change costs one read.
    fn edit_when(&mut self, number: u32, f: impl FnOnce(&mut Map<String, Value>) -> bool) -> io::Result<bool> {
        const NOTHING: &str = "\u{0}nothing to change";
        match self.edit(number, |t, _| if f(t) { Ok(()) } else { Err(NOTHING.to_string()) }) {
            Ok(()) => Ok(true),
            Err(e) if e.to_string() == NOTHING => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// A squad's leader was started for the ticket: it is listed on the ticket (last, so it is the one working on it
    /// now), with the worktrees made for it, and the ticket is Working.
    pub fn start_leader(&mut self, number: u32, leader: &Leader, worktrees: &[Worktree]) -> io::Result<()> {
        let mut l = json!({"squad": leader.squad, "session": leader.session, "started": leader.started});
        if let (Some(id), Some(o)) = (leader.names.id(), l.as_object_mut()) {
            o.insert("names".into(), json!(id));
        }
        if let (Some(f), Some(o)) = (&leader.snapshot, l.as_object_mut()) {
            o.insert("snapshot".into(), json!(f));
        }
        let w: Vec<Value> = worktrees.iter().map(|w| json!({"folder": w.folder, "path": w.path, "branch": w.branch})).collect();
        self.edit(number, move |t, _| {
            match t.get_mut("leaders").and_then(Value::as_array_mut) {
                Some(a) => a.push(l),
                None => {
                    t.insert("leaders".into(), json!([l]));
                }
            }
            t.insert("worktrees".into(), Value::Array(w));
            t.insert("state".into(), json!(State::Working.id()));
            t.remove("reason");
            t.remove("go_ahead");
            t.remove("handover");
            t.remove("permission");
            Ok(())
        })
    }

    /// The leader couldn't be started after all: take it off the ticket again, and put the ticket back as it was.
    pub fn forget_leader(&mut self, number: u32, session: &str, state: &State) -> io::Result<()> {
        let (session, id) = (session.to_string(), state.id().to_string());
        self.edit(number, move |t, _| {
            if let Some(a) = t.get_mut("leaders").and_then(Value::as_array_mut) {
                a.retain(|l| l.get("session").and_then(Value::as_str) != Some(session.as_str()));
            }
            t.insert("state".into(), json!(id));
            Ok(())
        })
    }

    /// What a squad's turn ended with (`platoon hook squad-stop`): the state, what it said, and on Done its result,
    /// written beside the body as `tickets/<NN>-result.md`. The go-ahead goes once the ticket leaves Working/Ready.
    pub fn squad_says(&mut self, number: u32, state: &State, reason: &str, result: Option<&str>) -> io::Result<()> {
        if let Some(text) = result {
            let dir = bodies_dir(&self.config, &self.project);
            fs::create_dir_all(&dir)?;
            write_atomic(&dir.join(result_file(number)), &format!("{}\n", text.trim_end()))?;
        }
        let (id, reason, keep) = (state.id().to_string(), reason.to_string(), state.keeps_go_ahead());
        self.edit(number, move |t, _| {
            t.insert("state".into(), json!(id));
            if reason.is_empty() {
                t.remove("reason");
            } else {
                t.insert("reason".into(), json!(reason));
            }
            t.remove("permission");
            if !keep {
                t.remove("go_ahead");
            }
            Ok(())
        })
    }

    /// `p` on a Ready ticket: the user lets the squad publish. The ticket is Working again.
    pub fn give_go_ahead(&mut self, number: u32) -> io::Result<()> {
        self.edit(number, |t, _| {
            t.insert("go_ahead".into(), json!(true));
            t.insert("state".into(), json!(State::Working.id()));
            t.remove("reason");
            t.remove("permission");
            Ok(())
        })
    }

    /// Keep what a new leader of ticket `number` is started with, beside the ticket's body: the `k`th leader it has.
    /// Returns the file's name, for the leader's entry.
    pub fn save_snapshot(&self, number: u32, k: usize, text: &str) -> io::Result<String> {
        let name = snapshot_file(number, k);
        write_atomic(&bodies_dir(&self.config, &self.project).join(&name), text)?;
        Ok(name)
    }

    /// The text of a leader's snapshot, if it has one and it can be read.
    pub fn snapshot(&self, leader: &Leader) -> Option<String> {
        let name = leader.snapshot.as_deref()?;
        fs::read_to_string(bodies_dir(&self.config, &self.project).join(name)).ok()
    }

    /// The squad's result for the ticket, if it has written one.
    pub fn result(&self, number: u32) -> Option<String> {
        let text = fs::read_to_string(bodies_dir(&self.config, &self.project).join(result_file(number))).ok()?;
        (!text.trim().is_empty()).then_some(text)
    }

    pub fn set_folders(&mut self, number: u32, folders: &[String]) -> io::Result<()> {
        let list = folders.to_vec();
        self.edit(number, |t, _| {
            t.insert("folders".into(), json!(list));
            Ok(())
        })
    }

    /// Add `blocker` to what `number` is blocked by, or take it away if it is there. A blocker that would make a
    /// loop (or a ticket that would block itself) is refused, with the reason in plain words. Returns whether it is
    /// blocked by it now.
    pub fn toggle_blocker(&mut self, number: u32, blocker: u32) -> io::Result<bool> {
        let mut now_blocked = false;
        self.edit(number, |t, all| {
            let mut list: Vec<u64> = t.get("blocked_by").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
            if let Some(pos) = list.iter().position(|n| *n == u64::from(blocker)) {
                list.remove(pos);
            } else {
                refuse_blocker(all, number, blocker)?;
                list.push(u64::from(blocker));
                list.sort_unstable();
                now_blocked = true;
            }
            t.insert("blocked_by".into(), json!(list));
            Ok(())
        })?;
        Ok(now_blocked)
    }

    /// The body changed in the user's editor: note the time.
    pub fn touch(&mut self, number: u32) -> io::Result<()> {
        self.edit(number, |_, _| Ok(()))
    }
}

/// The name of the file the `k`th leader of ticket `number` keeps what it was started with in, beside the bodies.
pub fn snapshot_file(number: u32, k: usize) -> String {
    format!("{number:02}-squad-{k}.json")
}

/// A snapshot's file name as the ticket names it: a plain name in the bodies' folder, never a path elsewhere.
fn is_snapshot_file(name: &str) -> bool {
    name.ends_with(".json") && !name.contains('/') && !name.contains('\\') && !name.starts_with('.')
}

/// The name of the file a squad's result for ticket `number` is kept in, beside the bodies.
pub fn result_file(number: u32) -> String {
    format!("{number:02}-result.md")
}

// ---- leaders across every project ---------------------------------------------------------------------------------

/// Every project's tickets file there is (`<config>/projects/<id>/tickets.json`), with its project id.
pub fn all_files(config: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = fs::read_dir(config.join("projects"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let id = e.file_name().to_str()?.to_string();
            let p = e.path().join("tickets.json");
            p.is_file().then_some((id, p))
        })
        .collect();
    out.sort();
    out
}

/// The session ids of every squad leader any ticket in any project ever had. These sessions belong to their tickets
/// only: they are never listed with the user's own sessions (the one place that decides this is here).
pub fn leader_ids(config: &Path) -> std::collections::HashSet<String> {
    let mut ids = std::collections::HashSet::new();
    for (project, _) in all_files(config) {
        for t in Tickets::open(config, &project).all() {
            ids.extend(t.leaders.iter().map(|l| l.session.clone()));
        }
    }
    ids
}

/// Where a leader session belongs: its project's id, the ticket's number, and whether it is the ticket's current
/// leader (a ticket handed to another squad after Done keeps its earlier leaders, which no longer move it along).
pub fn find_leader(config: &Path, session: &str) -> Option<(String, u32, bool)> {
    if session.is_empty() {
        return None;
    }
    for (project, _) in all_files(config) {
        for t in Tickets::open(config, &project).all() {
            if t.leaders.iter().any(|l| l.session == session) {
                let current = t.leader().is_some_and(|l| l.session == session);
                return Some((project, t.number, current));
            }
        }
    }
    None
}

/// Is `session` the current leader of a ticket that is Working? Such a session is never idle-stopped: its members
/// may be at work while nobody watches it. Waiting on you, Ready, Done and Cancelled don't hold it.
pub fn leader_at_work(config: &Path, session: &str) -> bool {
    let Some((project, number, true)) = find_leader(config, session) else { return false };
    Tickets::open(config, &project).get(number).is_some_and(|t| t.state == State::Working)
}

/// Why `ticket` can't be blocked by `blocker`, or Ok.
pub fn refuse_blocker(all: &[Ticket], ticket: u32, blocker: u32) -> Result<(), String> {
    if blocker == ticket {
        return Err(format!("#{ticket} can't wait for itself"));
    }
    if !all.iter().any(|t| t.number == blocker) {
        return Err(format!("there is no ticket #{blocker} in this project"));
    }
    if would_loop(all, ticket, blocker) {
        return Err(format!("#{blocker} already waits for #{ticket}, so #{ticket} can't wait for #{blocker}"));
    }
    Ok(())
}

/// The number the next ticket gets: the file's `next`, and never one a ticket in it already has.
fn next_number(root: &Map<String, Value>) -> u32 {
    let next = root.get("next").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).unwrap_or(1).max(1);
    let past = root
        .get("tickets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| t.get("number").and_then(Value::as_u64))
        .max()
        .and_then(|n| u32::try_from(n + 1).ok())
        .unwrap_or(1);
    next.max(past)
}

// ---- the command line ---------------------------------------------------------------------------------------------

/// Why an archived project takes no new ticket (the command line and the app say the same).
pub fn archived_refusal(name: &str) -> String {
    format!("\"{name}\" is archived; restore it first")
}

/// The project `want` names: its id, else its name ignoring case. A name two projects share is refused rather
/// than guessed.
pub fn find_project<'a>(projects: &'a crate::projects::Projects, want: &str) -> Result<&'a crate::projects::Project, String> {
    let want = want.trim();
    if want.is_empty() {
        return Err("say which project with --project <name>".into());
    }
    if let Some(p) = projects.get(want) {
        return Ok(p);
    }
    let low = want.to_lowercase();
    let named: Vec<&crate::projects::Project> = projects.all().iter().filter(|p| p.name.to_lowercase() == low).collect();
    match named.as_slice() {
        [one] => Ok(one),
        [] => Err(format!("there is no project called \"{want}\"")),
        _ => Err(format!("more than one project is called \"{want}\"; use its id instead ({})", named.iter().map(|p| p.id.as_str()).collect::<Vec<_>>().join(", "))),
    }
}

const USAGE: &str = "usage: platoon ticket add --project <name or id> --title <text> [--body-file <path>]
                           [--folder <path>]... [--waits-for <number>]...
       platoon ticket list [--project <name or id>] [--json]
  add    make a ticket and print its number (#N). The body comes from --body-file, else from what is piped
         in, else the template. Without --folder the ticket works in the current folder. Say in the body how
         far to take the work (\"How far: a local commit\", say); a squad stops at local changes when it doesn't say
  list   the tickets, open ones first, then the Done and Cancelled ones";

/// What `platoon ticket add --how-far` says: the flag is gone, and where "how far" goes now.
pub const HOW_FAR_GONE: &str = "don't know \"--how-far\": say how far to take the work in the ticket's text instead (for example \"How far: a local commit\")";

/// `platoon ticket ...`. Returns the exit code; what it prints goes to `out`, problems to `err`.
pub fn cli(args: &[String], config: &Path, stdin: &mut dyn io::Read, stdin_is_terminal: bool, cwd: &Path, out: &mut dyn io::Write, err: &mut dyn io::Write) -> i32 {
    match args.first().map(String::as_str) {
        Some("add") => match cli_add(&args[1..], config, stdin, stdin_is_terminal, cwd) {
            Ok(n) => {
                let _ = writeln!(out, "#{n}");
                0
            }
            Err(why) => {
                let _ = writeln!(err, "platoon ticket add: {why}");
                1
            }
        },
        Some("list") => match cli_list(&args[1..], config) {
            Ok(text) => {
                let _ = write!(out, "{text}");
                0
            }
            Err(why) => {
                let _ = writeln!(err, "platoon ticket list: {why}");
                1
            }
        },
        _ => {
            let _ = writeln!(err, "{USAGE}");
            2
        }
    }
}

/// The value after a flag.
fn value<'a>(args: &'a [String], i: &mut usize, flag: &str) -> Result<&'a str, String> {
    *i += 1;
    args.get(*i).map(String::as_str).ok_or_else(|| format!("{flag} needs a value"))
}

fn cli_add(args: &[String], config: &Path, stdin: &mut dyn io::Read, stdin_is_terminal: bool, cwd: &Path) -> Result<u32, String> {
    let (mut project, mut title, mut body_file) = (None, None, None);
    let (mut folders, mut blocked): (Vec<String>, Vec<u32>) = (Vec::new(), Vec::new());
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--project" => project = Some(value(args, &mut i, "--project")?.to_string()),
            "--title" => title = Some(value(args, &mut i, "--title")?.to_string()),
            "--body-file" => body_file = Some(value(args, &mut i, "--body-file")?.to_string()),
            // How far to take the work is no field any more: the ticket's own text says it.
            f if f == "--how-far" || f.starts_with("--how-far=") => return Err(HOW_FAR_GONE.into()),
            "--folder" => {
                let f = value(args, &mut i, "--folder")?;
                let p = if Path::new(f).is_absolute() { PathBuf::from(f) } else { cwd.join(f) };
                if !p.is_dir() {
                    return Err(format!("the folder {f} does not exist"));
                }
                let p = p.canonicalize().unwrap_or(p).to_string_lossy().into_owned();
                if !folders.contains(&p) {
                    folders.push(p);
                }
            }
            // `--blocked-by` is the name it had first, still taken quietly.
            flag @ ("--waits-for" | "--blocked-by") => {
                let flag = flag.to_string();
                let b = value(args, &mut i, &flag)?;
                let n = b.trim().trim_start_matches('#').parse::<u32>().map_err(|_| format!("{flag} takes a ticket number, not \"{b}\""))?;
                blocked.push(n);
            }
            other => return Err(format!("don't know \"{other}\" (see platoon ticket)")),
        }
        i += 1;
    }
    let project = project.ok_or("say which project with --project <name>")?;
    let title = title.filter(|t| !clean(t, TITLE_MAX).is_empty()).ok_or("give the ticket a title with --title <text>")?;
    let body = match body_file {
        Some(f) => {
            let p = if Path::new(&f).is_absolute() { PathBuf::from(&f) } else { cwd.join(&f) };
            Some(fs::read_to_string(&p).map_err(|e| format!("couldn't read {f}: {}", plain(&e)))?)
        }
        None if !stdin_is_terminal => {
            let mut text = String::new();
            stdin.read_to_string(&mut text).map_err(|e| format!("couldn't read the body from stdin: {}", plain(&e)))?;
            Some(text).filter(|t| !t.trim().is_empty())
        }
        None => None,
    };
    let projects = crate::projects::Projects::open(config);
    let p = find_project(&projects, &project)?;
    if p.archived {
        return Err(archived_refusal(&p.name));
    }
    // No --folder: the folder it was run in (where a session asking for the ticket is working).
    if folders.is_empty() {
        folders.push(cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf()).to_string_lossy().into_owned());
    }
    let mut tickets = Tickets::open(config, &p.id);
    if tickets.is_corrupt() {
        return Err(CORRUPT.into());
    }
    let new = NewTicket { title, body, folders, blocked_by: blocked };
    tickets.add(new).map_err(|e| plain(&e))
}

fn plain(e: &io::Error) -> String {
    let why = e.to_string();
    why.split(" (os error").next().unwrap_or(&why).to_string()
}

fn cli_list(args: &[String], config: &Path) -> Result<String, String> {
    let (mut project, mut as_json) = (None, false);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--project" => project = Some(value(args, &mut i, "--project")?.to_string()),
            "--json" => as_json = true,
            other => return Err(format!("don't know \"{other}\" (see platoon ticket)")),
        }
        i += 1;
    }
    let projects = crate::projects::Projects::open(config);
    let chosen: Vec<&crate::projects::Project> = match &project {
        Some(want) => vec![find_project(&projects, want)?],
        None => projects.all().iter().collect(),
    };
    let mut text = String::new();
    let mut rows: Vec<Value> = Vec::new();
    for p in chosen {
        let tickets = Tickets::open(config, &p.id);
        if tickets.is_corrupt() {
            if project.is_some() {
                return Err(CORRUPT.into());
            }
            text.push_str(&format!("{}\n  {CORRUPT}\n\n", p.name));
            continue;
        }
        if tickets.all().is_empty() && project.is_none() {
            continue;
        }
        let all = tickets.all();
        let (open, closed) = grouped(all);
        text.push_str(&format!("{}\n", p.name));
        if all.is_empty() {
            text.push_str("  No tickets yet.\n");
        }
        let line = |t: &Ticket| {
            let wait = waiting_for(t, all);
            let tail = if !wait.is_empty() && !t.state.closed() {
                format!("  waits for {}", wait.iter().map(|n| format!("#{n}")).collect::<Vec<_>>().join(", "))
            } else if is_ready(t, all) {
                "  ready".to_string()
            } else {
                String::new()
            };
            format!("  #{:<3} {}  ·  {}{tail}\n", t.number, t.title, t.state.label())
        };
        for t in &open {
            text.push_str(&line(t));
        }
        if !closed.is_empty() {
            text.push_str(&format!("  Done and cancelled · {}\n", closed.len()));
            for t in &closed {
                text.push_str(&line(t));
            }
        }
        text.push('\n');
        for t in open.iter().chain(closed.iter()) {
            rows.push(json!({
                "project": p.name,
                "project_id": p.id,
                "number": t.number,
                "title": t.title,
                "state": t.state.label(),
                "folders": t.folders,
                "blocked_by": t.blocked_by,
                "ready": is_ready(t, all),
                "file": tickets.body_path(t).to_string_lossy(),
            }));
        }
    }
    if as_json {
        return Ok(serde_json::to_string_pretty(&Value::Array(rows)).unwrap_or_default() + "\n");
    }
    if text.is_empty() {
        text = "No tickets yet.\n".into();
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-tickets-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn saved(d: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(path_in(d, "p1")).unwrap()).unwrap()
    }

    fn add(t: &mut Tickets, title: &str) -> u32 {
        t.add(NewTicket { title: title.into(), ..Default::default() }).unwrap()
    }

    #[test]
    fn numbers_start_at_one_and_are_never_used_twice_even_after_cancelling() {
        let d = dir("numbers");
        let mut t = Tickets::open(&d, "p1");
        assert!(t.all().is_empty() && !d.exists(), "reading makes nothing");
        assert_eq!(add(&mut t, "First thing"), 1);
        assert_eq!(add(&mut t, "Second thing"), 2);
        t.set_state(2, &State::Cancelled).unwrap();
        assert_eq!(add(&mut t, "Third"), 3, "a cancelled ticket keeps its number");
        // Even with the last one gone from the list by hand, `next` remembers.
        let mut v = saved(&d);
        v.get_mut("tickets").unwrap().as_array_mut().unwrap().retain(|x| x["number"] != 3);
        fs::write(path_in(&d, "p1"), v.to_string()).unwrap();
        assert_eq!(add(&mut Tickets::open(&d, "p1"), "Fourth"), 4);
        // And a file whose `next` is behind its tickets never hands out a number twice.
        let mut v = saved(&d);
        v.as_object_mut().unwrap().insert("next".into(), json!(1));
        fs::write(path_in(&d, "p1"), v.to_string()).unwrap();
        assert_eq!(add(&mut Tickets::open(&d, "p1"), "Fifth"), 5);
        let t = Tickets::open(&d, "p1");
        let first = t.get(1).unwrap();
        assert_eq!((first.title.as_str(), first.file.as_str(), &first.state), ("First thing", "01-first-thing.md", &State::New));
        assert!(saved(&d)["tickets"][0].get("how_far").is_none(), "how far is no field: the ticket's text says it");
        assert_eq!(t.body(first), TEMPLATE, "the template body");
        assert!(bodies_dir(&d, "p1").join("05-fifth.md").exists());
        assert!(add_err(&mut Tickets::open(&d, "p1"), "   ").contains("title"));
    }

    fn add_err(t: &mut Tickets, title: &str) -> String {
        t.add(NewTicket { title: title.into(), ..Default::default() }).unwrap_err().to_string()
    }

    #[test]
    fn the_template_says_what_it_should_do_and_how_to_check_it_without_file_paths() {
        assert!(TEMPLATE.starts_with("## What it should do\n"));
        assert!(TEMPLATE.contains("as behaviour, not a list of changes. It should fit in one fresh session."));
        assert!(TEMPLATE.contains("## Acceptance\n\n- [ ] \n- [ ] \n"));
        assert!(!TEMPLATE.contains('/'), "no paths");
        // How far to take the work is said in the text, under what it should do.
        let hint = TEMPLATE.find("How far: local changes only (say if you want a commit, or a push and merge request).").unwrap();
        assert!(TEMPLATE.find("## What it should do").unwrap() < hint && hint < TEMPLATE.find("## Acceptance").unwrap());
    }

    #[test]
    fn a_renamed_ticket_keeps_its_file_and_slugs_are_plain() {
        assert_eq!(slug("Fix the Login page!"), "fix-the-login-page");
        assert_eq!(slug("  ☃ "), "ticket");
        assert_eq!(slug("café au lait"), "caf-au-lait");
        assert!(slug(&"word ".repeat(30)).len() <= 40);
        let d = dir("rename");
        let mut t = Tickets::open(&d, "p1");
        add(&mut t, "Old name");
        t.rename(1, "New name").unwrap();
        assert_eq!((t.get(1).unwrap().title.as_str(), t.get(1).unwrap().file.as_str()), ("New name", "01-old-name.md"));
        assert!(t.rename(1, " ").is_err());
        // Two tickets with the same title get files of their own.
        add(&mut t, "Same");
        fs::write(bodies_dir(&d, "p1").join("03-same.md"), "someone else's").unwrap();
        add(&mut t, "Same");
        assert_eq!(t.get(3).unwrap().file, "03-same-2.md");
        assert_eq!(fs::read_to_string(bodies_dir(&d, "p1").join("03-same.md")).unwrap(), "someone else's", "never written over");
    }

    #[test]
    fn keys_and_states_platoon_does_not_know_survive_a_rewrite() {
        let d = dir("keep");
        fs::create_dir_all(d.join("projects/p1")).unwrap();
        fs::write(
            path_in(&d, "p1"),
            r#"{"next":3,"squads":["coding"],"tickets":[
                {"number":1,"title":"One","file":"01-one.md","state":"reviewing","how_far":"Local commit","folders":["/tmp"],"blocked_by":[],"created":1,"updated":1,"squad":"coding","leader":"abc"},
                {"number":2,"title":"Two","file":"02-two.md","state":"new","how_far":"Ship it on Friday","folders":[],"blocked_by":[1],"created":1,"updated":1},
                "not a ticket"]}"#,
        )
        .unwrap();
        let mut t = Tickets::open(&d, "p1");
        assert_eq!(t.all().len(), 2);
        assert_eq!(t.get(1).unwrap().state, State::Other("reviewing".into()));
        assert_eq!(t.get(1).unwrap().state.label(), "reviewing", "shown as written");
        assert!(!t.get(1).unwrap().state.closed(), "listed with the open ones");
        t.rename(1, "Uno").unwrap();
        t.set_state(2, &State::Done).unwrap();
        add(&mut t, "Three");
        let v = saved(&d);
        assert_eq!(v["squads"], json!(["coding"]));
        let one = &v["tickets"][0];
        assert_eq!((one["squad"].as_str(), one["leader"].as_str(), one["state"].as_str(), one["title"].as_str()), (Some("coding"), Some("abc"), Some("reviewing"), Some("Uno")));
        assert_eq!(v["tickets"][2], "not a ticket");
        assert_eq!(v["tickets"][1]["state"], "done");
        // "how far" was a field once: a ticket that has it keeps it in the file, and nothing reads it.
        assert_eq!((v["tickets"][0]["how_far"].as_str(), v["tickets"][1]["how_far"].as_str()), (Some("Local commit"), Some("Ship it on Friday")));
        assert!(v["tickets"][3].get("how_far").is_none());
        assert_eq!(v["next"], 4);
    }

    #[test]
    fn a_corrupt_file_is_never_written_over() {
        let d = dir("corrupt");
        fs::create_dir_all(d.join("projects/p1")).unwrap();
        for bad in ["{ nope", "[]", r#"{"tickets":{}}"#, r#"{"next":"x"}"#] {
            fs::write(path_in(&d, "p1"), bad).unwrap();
            let mut t = Tickets::open(&d, "p1");
            assert!(t.is_corrupt() && t.all().is_empty(), "{bad}");
            let e = t.add(NewTicket { title: "x".into(), ..Default::default() }).unwrap_err();
            assert_eq!(e.to_string(), CORRUPT);
            assert!(t.set_state(1, &State::Done).is_err());
            assert_eq!(fs::read_to_string(path_in(&d, "p1")).unwrap(), bad, "left exactly as it was");
            assert!(!bodies_dir(&d, "p1").exists() || fs::read_dir(bodies_dir(&d, "p1")).unwrap().next().is_none(), "no body left behind");
        }
    }

    #[test]
    fn writes_are_atomic_and_leave_no_temp_files() {
        let d = dir("atomic");
        let mut t = Tickets::open(&d, "p1");
        for i in 0..5 {
            add(&mut t, &format!("T{i}"));
        }
        let names: Vec<String> = fs::read_dir(d.join("projects/p1")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(!names.iter().any(|n| n.ends_with(".tmp")), "{names:?}");
        assert!(names.contains(&"tickets.lock".to_string()), "the lock sits beside the file: {names:?}");
        // Whatever is read at any moment is a whole file: a reader in a loop never sees half of one.
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (d2, s2) = (d.clone(), stop.clone());
        let reader = std::thread::spawn(move || {
            while !s2.load(std::sync::atomic::Ordering::SeqCst) {
                let text = fs::read_to_string(path_in(&d2, "p1")).unwrap();
                assert!(serde_json::from_str::<Value>(&text).is_ok(), "half a file: {text}");
            }
        });
        for i in 0..40 {
            t.rename(1, &format!("Name {i}")).unwrap();
        }
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        reader.join().unwrap();
    }

    #[test]
    fn two_writers_adding_at_once_get_different_numbers() {
        let d = dir("lock");
        Tickets::open(&d, "p1");
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let d = d.clone();
                std::thread::spawn(move || {
                    let mut t = Tickets::open(&d, "p1");
                    (0..5).map(|k| add(&mut t, &format!("t{i}-{k}"))).collect::<Vec<u32>>()
                })
            })
            .collect();
        let mut all: Vec<u32> = threads.into_iter().flat_map(|t| t.join().unwrap()).collect();
        all.sort_unstable();
        assert_eq!(all, (1..=40).collect::<Vec<u32>>());
        let t = Tickets::open(&d, "p1");
        assert_eq!(t.all().len(), 40);
        assert_eq!(fs::read_dir(bodies_dir(&d, "p1")).unwrap().count(), 40, "a body each");
    }

    #[test]
    fn a_second_reader_sees_changes() {
        let d = dir("reload");
        let (mut a, mut b) = (Tickets::open(&d, "p1"), Tickets::open(&d, "p1"));
        add(&mut a, "one");
        assert!(b.reload_if_changed());
        assert_eq!(b.all().len(), 1);
        assert!(!b.reload_if_changed());
    }

    fn tk(number: u32, state: State, blocked_by: &[u32]) -> Ticket {
        Ticket { number, title: format!("t{number}"), file: String::new(), state, folders: vec![], blocked_by: blocked_by.to_vec(), created: 0, updated: 0, leaders: vec![], worktrees: vec![], reason: String::new(), go_ahead: false, handover: None, permission: false, busy: None }
    }

    #[test]
    fn a_ticket_is_ready_when_it_is_new_and_everything_it_waits_for_is_done() {
        let all = vec![tk(1, State::Done, &[]), tk(2, State::New, &[]), tk(3, State::New, &[1, 2]), tk(4, State::New, &[1]), tk(5, State::Done, &[2]), tk(6, State::New, &[9]), tk(7, State::New, &[8]), tk(8, State::Cancelled, &[])];
        assert_eq!(waiting_for(&all[2], &all), [2], "only the unfinished blockers");
        assert!(!is_ready(&all[2], &all));
        assert!(is_ready(&all[3], &all) && is_ready(&all[1], &all));
        assert!(!is_ready(&all[4], &all), "a Done ticket is not ready");
        assert!(is_ready(&all[5], &all), "a blocker that isn't there doesn't hold it up");
        assert!(!is_ready(&all[6], &all), "a Cancelled blocker is not Done");
        let (open, closed) = grouped(&all);
        assert_eq!(open.iter().map(|t| t.number).collect::<Vec<_>>(), [2, 3, 4, 6, 7]);
        assert_eq!(closed.iter().map(|t| t.number).collect::<Vec<_>>(), [1, 5, 8]);
    }

    #[test]
    fn loops_and_a_ticket_blocking_itself_are_refused() {
        let d = dir("loops");
        let mut t = Tickets::open(&d, "p1");
        for n in ["a", "b", "c", "d"] {
            add(&mut t, n);
        }
        assert!(t.toggle_blocker(2, 1).unwrap());
        assert!(t.toggle_blocker(3, 2).unwrap());
        // 1 <- 2 <- 3: 1 waiting for 3 would go round.
        let e = t.toggle_blocker(1, 3).unwrap_err().to_string();
        assert_eq!(e, "#3 already waits for #1, so #1 can't wait for #3");
        assert_eq!(t.toggle_blocker(4, 4).unwrap_err().to_string(), "#4 can't wait for itself");
        assert!(t.toggle_blocker(4, 9).unwrap_err().to_string().contains("no ticket #9"));
        assert!(t.get(1).unwrap().blocked_by.is_empty(), "nothing changed");
        // Toggling again takes it away.
        assert!(!t.toggle_blocker(3, 2).unwrap());
        assert!(t.get(3).unwrap().blocked_by.is_empty());
        assert!(t.toggle_blocker(1, 3).unwrap(), "no loop any more");
        assert!(would_loop(t.all(), 3, 1) && !would_loop(t.all(), 4, 1));
        // A new ticket can only wait for tickets that exist.
        assert!(t.add(NewTicket { title: "e".into(), blocked_by: vec![7], ..Default::default() }).unwrap_err().to_string().contains("no ticket #7"));
        assert_eq!(t.add(NewTicket { title: "e".into(), blocked_by: vec![2, 1, 2], ..Default::default() }).unwrap(), 5);
        assert_eq!(t.get(5).unwrap().blocked_by, [1, 2]);
    }

    #[test]
    fn states_have_the_words_of_the_screen() {
        let words: Vec<String> = ["new", "working", "waiting", "ready", "done", "cancelled"].iter().map(|s| State::from_id(s).label().to_string()).collect();
        assert_eq!(words, ["New", "Working", "Waiting on you", "Ready to push/publish", "Done", "Cancelled"]);
        assert_eq!(USER_STATES.iter().map(State::label).collect::<Vec<_>>(), ["New", "Done", "Cancelled"]);
        assert!(State::Done.closed() && State::Cancelled.closed() && !State::Working.closed());
    }

    #[test]
    fn the_command_line_finds_the_project_by_id_then_by_name_and_refuses_what_it_cannot_tell() {
        let d = dir("find");
        let mut p = crate::projects::Projects::open(&d);
        let web = p.create("Web shop").unwrap();
        let a = p.create("Twins").unwrap();
        let b = p.create("twins").unwrap();
        assert_eq!(find_project(&p, &web).unwrap().name, "Web shop");
        assert_eq!(find_project(&p, "WEB SHOP").unwrap().id, web);
        assert!(find_project(&p, "Web").unwrap_err().contains("no project called \"Web\""), "no guessing from a prefix");
        let e = find_project(&p, "twins").unwrap_err();
        assert!(e.contains("more than one") && e.contains(&a) && e.contains(&b), "{e}");
        assert_eq!(find_project(&p, &a).unwrap().id, a, "the id still works");
        assert!(find_project(&p, " ").is_err());
    }

    fn run(d: &Path, args: &[&str], stdin: &str, terminal: bool) -> (i32, String, String) {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = cli(&args, d, &mut stdin.as_bytes(), terminal, d, &mut out, &mut err);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    #[test]
    fn the_command_line_adds_and_lists() {
        let d = dir("cli");
        let id = crate::projects::Projects::open(&d).create("Web shop").unwrap();
        fs::create_dir_all(d.join("work")).unwrap();
        assert_eq!(run(&d, &["add", "--project", "web shop", "--title", "Login"], "", true), (0, "#1\n".into(), String::new()));
        let (code, out, _) = run(&d, &["add", "--project", &id, "--title", "Basket", "--folder", "work", "--waits-for", "#1"], "## Body\nfrom stdin\n", false);
        assert_eq!((code, out.as_str()), (0, "#2\n"));
        let t = Tickets::open(&d, &id);
        let two = t.get(2).unwrap();
        assert_eq!(two.blocked_by.as_slice(), &[1][..]);
        assert_eq!(two.folders, [d.join("work").canonicalize().unwrap().to_string_lossy().into_owned()]);
        assert_eq!(t.body(two), "## Body\nfrom stdin\n");
        assert_eq!(t.body(t.get(1).unwrap()), TEMPLATE, "nothing piped in: the template");
        assert_eq!(t.get(1).unwrap().folders, [d.canonicalize().unwrap().to_string_lossy().into_owned()], "no --folder: where it was run");
        fs::write(d.join("body.md"), "from a file").unwrap();
        let (code, _, _) = run(&d, &["add", "--project", "Web shop", "--title", "Own", "--body-file", "body.md"], "ignored", false);
        assert_eq!(code, 0);
        let t = Tickets::open(&d, &id);
        assert_eq!(t.body(t.get(3).unwrap()), "from a file");
        // Refusals: one plain line each, and nothing made.
        for (args, says) in [
            (vec!["add", "--project", "Nope", "--title", "x"], "no project called"),
            (vec!["add", "--project", "Web shop", "--title", "x", "--blocked-by", "9"], "no ticket #9"),
            (vec!["add", "--project", "Web shop", "--title", "x", "--folder", "/no/such/folder"], "does not exist"),
            (vec!["add", "--project", "Web shop"], "title"),
            (vec!["add", "--title", "x"], "--project"),
            (vec!["add", "--project", "Web shop", "--title", "x", "--waits-for", "two"], "--waits-for takes a ticket number"),
            // How far is said in the ticket's text now: the flag is refused, with where it goes instead.
            (vec!["add", "--project", "Web shop", "--title", "x", "--how-far", "local commit"], "in the ticket's text instead"),
            (vec!["add", "--project", "Web shop", "--title", "x", "--how-far=local commit"], "don't know \"--how-far\""),
        ] {
            let (code, out, err) = run(&d, &args, "", true);
            assert!(code != 0 && out.is_empty() && err.contains(says) && err.lines().count() == 1, "{args:?}: {err}");
        }
        assert_eq!(Tickets::open(&d, &id).all().len(), 3);
        assert_eq!(run(&d, &["frobnicate"], "", true).0, 2);

        let mut tt = Tickets::open(&d, &id);
        tt.set_state(1, &State::Done).unwrap();
        let (code, out, _) = run(&d, &["list", "--project", "Web shop"], "", true);
        assert_eq!(code, 0);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "Web shop");
        assert!(lines[1].contains("#2") && lines[1].contains("ready"), "{out}");
        assert_eq!(lines[2], "  #3   Own  ·  New  ready", "{out}");
        assert!(lines[1].starts_with("  #2   Basket  ·  New  ready"), "number, title, state, then what it waits for: {out}");
        assert_eq!(lines[3], "  Done and cancelled · 1");
        assert!(lines[4].contains("#1") && lines[4].contains("Done"), "{out}");
        let (_, out, _) = run(&d, &["list", "--json"], "", true);
        let v: Value = serde_json::from_str(&out).unwrap();
        let first = &v[0];
        assert_eq!((first["number"].as_u64(), first["state"].as_str(), first["ready"].as_bool()), (Some(2), Some("New"), Some(true)));
        assert!(v.as_array().unwrap().iter().all(|t| t.get("how_far").is_none()), "no how far in the JSON: {out}");
        assert_eq!(first["blocked_by"], json!([1]));
        assert!(first["file"].as_str().unwrap().ends_with("/tickets/02-basket.md") && Path::new(first["file"].as_str().unwrap()).is_absolute());
        assert_eq!(v[2]["number"], 1, "the Done one last");
    }

    #[test]
    fn a_cancelled_ticket_still_holds_up_the_ones_waiting_for_it_and_only_done_lets_them_go() {
        let d = dir("cancelled");
        let mut t = Tickets::open(&d, "p1");
        add(&mut t, "Base");
        t.add(NewTicket { title: "After".into(), blocked_by: vec![1], ..Default::default() }).unwrap();
        t.set_state(1, &State::Cancelled).unwrap();
        assert_eq!(waiting_for(t.get(2).unwrap(), t.all()), [1], "a Cancelled ticket is still waited for");
        assert!(!is_ready(t.get(2).unwrap(), t.all()));
        t.set_state(1, &State::Done).unwrap();
        assert!(waiting_for(t.get(2).unwrap(), t.all()).is_empty() && is_ready(t.get(2).unwrap(), t.all()), "Done lets it go");
        // The command line says the same.
        let id = crate::projects::Projects::open(&d).create("Web").unwrap();
        run(&d, &["add", "--project", "Web", "--title", "Base"], "", true);
        run(&d, &["add", "--project", "Web", "--title", "After", "--waits-for", "1"], "", true);
        Tickets::open(&d, &id).set_state(1, &State::Cancelled).unwrap();
        let (_, out, _) = run(&d, &["list", "--project", "Web"], "", true);
        assert!(out.lines().nth(1).unwrap().contains("#2") && out.lines().nth(1).unwrap().ends_with("waits for #1"), "{out}");
    }

    #[test]
    fn with_no_folder_a_ticket_works_where_the_command_was_run_and_folders_given_replace_that() {
        let d = dir("cwd");
        let id = crate::projects::Projects::open(&d).create("Web").unwrap();
        let here = d.join("here");
        let other = d.join("other");
        fs::create_dir_all(&here).unwrap();
        fs::create_dir_all(&other).unwrap();
        let call = |args: &[&str]| {
            let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            let (mut out, mut err) = (Vec::new(), Vec::new());
            // Run "in" `here`, given as a path with a `..` in it: the ticket keeps the full, plain path.
            let code = cli(&args, &d, &mut "".as_bytes(), true, &here.join("..").join("here"), &mut out, &mut err);
            assert_eq!(code, 0, "{}", String::from_utf8_lossy(&err));
        };
        call(&["add", "--project", "Web", "--title", "Here"]);
        call(&["add", "--project", "Web", "--title", "There", "--folder", other.to_str().unwrap()]);
        let t = Tickets::open(&d, &id);
        let full = |p: &Path| p.canonicalize().unwrap().to_string_lossy().into_owned();
        assert_eq!(t.get(1).unwrap().folders, [full(&here)]);
        assert_eq!(t.get(2).unwrap().folders, [full(&other)], "an explicit --folder replaces the current folder");
    }

    #[test]
    fn an_archived_project_takes_no_new_tickets_from_the_command_line() {
        let d = dir("archived");
        let mut p = crate::projects::Projects::open(&d);
        let id = p.create("Web shop").unwrap();
        assert_eq!(run(&d, &["add", "--project", "Web shop", "--title", "Before"], "", true).0, 0);
        p.archive(&id, &[]).unwrap();
        let (code, out, err) = run(&d, &["add", "--project", "web shop", "--title", "After"], "", true);
        assert_eq!((code, out.as_str(), err.as_str()), (1, "", "platoon ticket add: \"Web shop\" is archived; restore it first\n"));
        assert_eq!(Tickets::open(&d, &id).all().len(), 1, "nothing made");
        // Its tickets can still be listed and changed.
        assert!(run(&d, &["list", "--project", "Web shop"], "", true).1.contains("#1"));
        Tickets::open(&d, &id).set_state(1, &State::Done).unwrap();
        // Restored, it takes them again.
        p.restore(&id).unwrap();
        assert_eq!(run(&d, &["add", "--project", "Web shop", "--title", "After"], "", true), (0, "#2\n".into(), String::new()));
    }

    #[test]
    fn waits_for_is_the_flag_and_blocked_by_still_works_quietly() {
        let d = dir("alias");
        let id = crate::projects::Projects::open(&d).create("Web").unwrap();
        run(&d, &["add", "--project", "Web", "--title", "One"], "", true);
        run(&d, &["add", "--project", "Web", "--title", "Two"], "", true);
        assert_eq!(run(&d, &["add", "--project", "Web", "--title", "Three", "--waits-for", "1", "--blocked-by", "#2"], "", true).0, 0);
        assert_eq!(Tickets::open(&d, &id).get(3).unwrap().blocked_by, [1, 2]);
        let (code, _, err) = run(&d, &["add", "--project", "Web", "--title", "x", "--blocked-by", "nine"], "", true);
        assert!(code == 1 && err.contains("--blocked-by takes a ticket number"), "the flag it was given: {err}");
        let (code, _, err) = run(&d, &["add", "--project", "Web", "--title", "x", "--waits-for"], "", true);
        assert!(code == 1 && err.contains("--waits-for needs a value"), "{err}");
        // The usage text shows only the new name.
        let (code, _, usage) = run(&d, &[], "", true);
        assert_eq!(code, 2);
        assert!(usage.contains("--waits-for <number>") && !usage.contains("blocked"), "{usage}");
    }

    #[test]
    fn a_hand_over_that_didnt_happen_is_written_on_the_ticket_and_goes_when_a_leader_starts() {
        let d = dir("handover");
        let mut t = Tickets::open(&d, "p1");
        let n = add(&mut t, "Basket");
        t.handover_failed(n, "coding", HandoverKind::Refused, "This is docs work.").unwrap();
        let back = Tickets::open(&d, "p1");
        let x = back.get(n).unwrap();
        assert_eq!(x.state, State::New);
        assert_eq!(x.handover, Some(Handover { squad: "coding".into(), kind: HandoverKind::Refused, says: "This is docs work.".into() }));
        t.handover_failed(n, "coding", HandoverKind::NoCheck, "Session limit has been reached").unwrap();
        assert_eq!(Tickets::open(&d, "p1").get(n).unwrap().handover.as_ref().map(|h| h.kind.clone()), Some(HandoverKind::NoCheck));
        t.start_leader(n, &Leader { squad: "coding".into(), session: "s".into(), started: 1, names: Default::default(), snapshot: None }, &[]).unwrap();
        assert_eq!(Tickets::open(&d, "p1").get(n).unwrap().handover, None, "a leader started: nothing to say any more");
        t.handover_failed(n, "coding", HandoverKind::NotStarted, "x").unwrap();
        t.set_state(n, &State::Cancelled).unwrap();
        assert_eq!(Tickets::open(&d, "p1").get(n).unwrap().handover, None, "the user moved it on");
    }

    #[test]
    fn a_permission_wait_is_its_own_kind_of_waiting_and_only_it_ends_with_the_next_tool() {
        let d = dir("permission");
        let mut t = Tickets::open(&d, "p1");
        let n = add(&mut t, "Basket");
        t.start_leader(n, &Leader { squad: "coding".into(), session: "s".into(), started: 1, names: Default::default(), snapshot: None }, &[]).unwrap();
        let before = fs::read(t.path()).unwrap();
        assert!(!t.permission_answered(n).unwrap(), "Working: nothing to answer");
        assert_eq!(fs::read(t.path()).unwrap(), before, "and nothing written, not even the time");
        assert!(t.permission_wait(n).unwrap());
        let x = Tickets::open(&d, "p1").get(n).cloned().unwrap();
        assert_eq!((x.state, x.reason.as_str(), x.permission), (State::Waiting, PERMISSION_REASON, true));
        assert!(!t.permission_wait(n).unwrap(), "already waiting");
        assert!(t.permission_answered(n).unwrap());
        let x = Tickets::open(&d, "p1").get(n).cloned().unwrap();
        assert_eq!((x.state, x.reason.as_str(), x.permission), (State::Working, "", false));
        // A question is not a permission: a tool running doesn't end it.
        t.squad_says(n, &State::Waiting, "Asks: Red or blue?", None).unwrap();
        assert!(!t.permission_answered(n).unwrap());
        assert_eq!(Tickets::open(&d, "p1").get(n).unwrap().state, State::Waiting);
        assert!(!t.permission_wait(n).unwrap(), "only a Working ticket starts waiting for a permission");
        // Ready and Done are left alone too.
        t.squad_says(n, &State::Ready, "Wants to publish: x", None).unwrap();
        assert!(!t.permission_wait(n).unwrap());
        // A permission wait that the leader's turn ends: the Stop hook's word counts, and the mark goes.
        t.set_state(n, &State::Working).unwrap();
        t.permission_wait(n).unwrap();
        t.squad_says(n, &State::Waiting, "Asks: which?", None).unwrap();
        assert!(!Tickets::open(&d, "p1").get(n).unwrap().permission);
        assert!(!t.permission_answered(n).unwrap());
    }

    #[test]
    fn a_busy_ticket_shows_working_with_what_is_happening_and_the_file_never_says_so() {
        let d = dir("busy");
        let mut t = Tickets::open(&d, "p1");
        let n = add(&mut t, "Basket");
        let before = fs::read(t.path()).unwrap();
        t.set_busy(n, Some("Commander is checking…"));
        let x = t.get(n).unwrap();
        assert_eq!((x.state.clone(), x.busy.as_deref()), (State::Working, Some("Commander is checking…")));
        assert_eq!(fs::read(t.path()).unwrap(), before);
        assert_eq!(Tickets::open(&d, "p1").get(n).unwrap().state, State::New, "another reader sees the file");
        // It lasts across a change made on disk.
        t.rename(n, "Basket page").unwrap();
        assert_eq!(t.get(n).unwrap().state, State::Working);
        t.set_busy(n, None);
        assert_eq!((t.get(n).unwrap().state.clone(), t.get(n).unwrap().busy.clone()), (State::New, None));
    }

    #[test]
    fn a_leader_remembers_how_its_agents_were_named_and_one_written_before_that_had_the_plain_names() {
        let d = dir("names");
        let mut t = Tickets::open(&d, "p1");
        let n = add(&mut t, "a");
        t.start_leader(n, &Leader { squad: "coding".into(), session: "new-1".into(), started: 1, names: AgentNames::SquadRole, snapshot: None }, &[]).unwrap();
        assert_eq!(saved(&d)["tickets"][0]["leaders"][0]["names"], "squad-role");
        assert_eq!(Tickets::open(&d, "p1").get(n).unwrap().leader().unwrap().names, AgentNames::SquadRole);
        t.start_leader(n, &Leader { squad: "coding".into(), session: "old-1".into(), started: 2, names: AgentNames::Plain, snapshot: None }, &[]).unwrap();
        assert!(saved(&d)["tickets"][0]["leaders"][1].get("names").is_none(), "the plain names: nothing written, as before");
        // A leader written by the platoon before this one (no "names") was started with the plain names.
        let mut v = saved(&d);
        if let Some(l) = v.pointer_mut("/tickets/0/leaders/0").and_then(Value::as_object_mut) {
            l.remove("names");
        }
        fs::write(path_in(&d, "p1"), v.to_string()).unwrap();
        let t = Tickets::open(&d, "p1");
        assert_eq!(t.get(n).unwrap().leaders.iter().map(|l| l.names).collect::<Vec<_>>(), [AgentNames::Plain, AgentNames::Plain]);
        assert_eq!(AgentNames::default(), AgentNames::SquadRole, "new leaders get the squad's names");
    }

    #[test]
    fn a_leader_keeps_its_snapshot_beside_the_body_and_only_by_a_plain_file_name() {
        let d = dir("snapshot");
        let mut t = Tickets::open(&d, "p1");
        let n = add(&mut t, "a");
        let name = t.save_snapshot(n, 1, "{\"agents\":{}}\n").unwrap();
        assert_eq!(name, "01-squad-1.json");
        assert!(bodies_dir(&d, "p1").join("01-squad-1.json").exists());
        let l = Leader { squad: "coding".into(), session: "s-1".into(), started: 1, names: AgentNames::SquadRole, snapshot: Some(name) };
        t.start_leader(n, &l, &[]).unwrap();
        assert_eq!(saved(&d)["tickets"][0]["leaders"][0]["snapshot"], "01-squad-1.json");
        let back = Tickets::open(&d, "p1");
        let got = back.get(n).unwrap().leader().unwrap().clone();
        assert_eq!(got, l);
        assert_eq!(back.snapshot(&got).as_deref(), Some("{\"agents\":{}}\n"));
        // Only a plain name in the bodies' folder is ever read: a path somewhere else is no snapshot.
        let mut v = saved(&d);
        if let Some(o) = v.pointer_mut("/tickets/0/leaders/0").and_then(Value::as_object_mut) {
            o.insert("snapshot".into(), json!("../../../settings.json"));
        }
        fs::write(path_in(&d, "p1"), v.to_string()).unwrap();
        assert_eq!(Tickets::open(&d, "p1").get(n).unwrap().leader().unwrap().snapshot, None);
    }

    #[test]
    fn only_the_current_leader_of_a_working_ticket_is_at_work() {
        let d = dir("at-work");
        let mut t = Tickets::open(&d, "p1");
        let n = add(&mut t, "Basket");
        assert!(!leader_at_work(&d, "lead-1"));
        t.start_leader(n, &Leader { squad: "coding".into(), session: "lead-1".into(), started: 1, names: Default::default(), snapshot: None }, &[]).unwrap();
        assert!(leader_at_work(&d, "lead-1"));
        assert!(!leader_at_work(&d, "someone"));
        for s in [State::Waiting, State::Ready, State::Done, State::Cancelled, State::New] {
            t.set_state(n, &s).unwrap();
            assert!(!leader_at_work(&d, "lead-1"), "{s:?}");
        }
        t.set_state(n, &State::Working).unwrap();
        t.start_leader(n, &Leader { squad: "coding".into(), session: "lead-2".into(), started: 2, names: Default::default(), snapshot: None }, &[]).unwrap();
        assert!(!leader_at_work(&d, "lead-1"), "an earlier leader");
        assert!(leader_at_work(&d, "lead-2"));
    }
}
