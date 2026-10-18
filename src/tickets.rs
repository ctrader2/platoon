//! Tasks (the user's word; the code says "ticket" in places, from the name they had first): one piece of work, in a
//! project or in none. A task has a number (per project, from 1, never used twice), a title, a text in Markdown (a
//! file of its own the user edits: what it should do and its "Done when" boxes), one folder it works in, the squad
//! suggested for it (or the one it runs with), and a state.
//!
//! A task runs as one ordinary Claude Code session (`taskrun.rs`): in a git worktree of its folder on the branch
//! `task/<N>`, or in the folder itself when that is not in git. The session's own hooks move the task along: the plan
//! is ready, a question, a permission prompt, the user typed, the turn ended (with the project's tests run).
//!
//! A project's tasks live beside its notes: `<config>/projects/<id>/tasks.json` =
//! `{"next": 4, "tasks": [{"number", "title", "file", "state", "folder", "squad", "session", ...}]}`, the texts in
//! `<config>/projects/<id>/tasks/<NN>-<slug>.md` and each task's own folder in `tasks/<NN>-work/` (the squad it runs
//! with, `squad.md`, the other squads beside it, its plan, the last test output). Tasks in no project live in
//! `<config>/tasks.json` and `<config>/tasks/`, with a number space of their own (the project id `""`, `NO_PROJECT_ID`).
//!
//! Writes are atomic (temp file + rename) and happen under a lock (`tasks.lock` beside the file: the app, the hooks and
//! `platoon task add` all write), keys platoon doesn't know are kept (on the file and on each task), and a file that
//! can't be understood is never written over.

use crate::projects::{lock, write_atomic};
use serde_json::{Map, Value, json};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// The longest title kept, in characters.
pub const TITLE_MAX: usize = 100;

/// The longest reason kept (a question, a permission prompt's words, a test summary), in characters.
pub const WHY_MAX: usize = 600;

/// What a new task's text starts from: what should work when it is finished, and how to tell.
pub const TEMPLATE: &str = "\
## What it should do

What should work when this is finished, as behaviour, not a list of changes.

## Done when

- [ ] \n- [ ] \n";

// ---- states -------------------------------------------------------------------------------------------------------

/// Why a task needs the user, with what it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Need {
    /// Its plan is ready (the session called `ExitPlanMode`): the plan is `plan.md` in the task's own folder.
    Plan,
    /// It asked a question (`AskUserQuestion`), or stopped while planning: its words.
    Question(String),
    /// It waits at Claude Code's permission prompt: the prompt's words.
    Permission(String),
    /// Its tests still fail after it was sent back twice: the summary of the last run.
    Tests(String),
    /// The check of its work (`verdict.rs`) still found problems after it was sent back once: what it found, short.
    Check(String),
    /// The check of its work could not run (timed out, no answer it could read, no `claude`): why.
    CheckFailed(String),
}

impl Need {
    fn id(&self) -> &'static str {
        match self {
            Need::Plan => "plan",
            Need::Question(_) => "question",
            Need::Permission(_) => "permission",
            Need::Tests(_) => "tests",
            Need::Check(_) => "check",
            Need::CheckFailed(_) => "check_failed",
        }
    }

    /// What it says ("" for a plan: the plan is its own file).
    pub fn text(&self) -> &str {
        match self {
            Need::Plan => "",
            Need::Question(s) | Need::Permission(s) | Need::Tests(s) | Need::Check(s) | Need::CheckFailed(s) => s,
        }
    }

    /// The row's word for it.
    pub fn words(&self) -> &'static str {
        match self {
            Need::Plan => "a plan to approve",
            Need::Question(_) => "a question",
            Need::Permission(_) => "asks permission",
            Need::Tests(_) => "tests failing",
            Need::Check(_) => "the check found problems",
            Need::CheckFailed(_) => "the check could not run",
        }
    }

    fn from(id: &str, text: &str) -> Need {
        let text = clean(text, WHY_MAX);
        match id {
            "plan" => Need::Plan,
            "permission" => Need::Permission(text),
            "tests" => Need::Tests(text),
            "check" => Need::Check(text),
            "check_failed" => Need::CheckFailed(text),
            _ => Need::Question(text),
        }
    }
}

/// Where a task is.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    NotStarted,
    Working,
    NeedsYou(Need),
    Done,
    Cancelled,
}

impl State {
    fn id(&self) -> &'static str {
        match self {
            State::NotStarted => "not_started",
            State::Working => "working",
            State::NeedsYou(_) => "needs_you",
            State::Done => "done",
            State::Cancelled => "cancelled",
        }
    }

    /// What the screen and `platoon task list` call it.
    pub fn label(&self) -> &'static str {
        match self {
            State::NotStarted => "Not started",
            State::Working => "Working",
            State::NeedsYou(_) => "Needs you",
            State::Done => "Done",
            State::Cancelled => "Cancelled",
        }
    }

    /// Done and Cancelled: nothing more happens to it.
    pub fn closed(&self) -> bool {
        matches!(self, State::Done | State::Cancelled)
    }

    pub fn needs_you(&self) -> bool {
        matches!(self, State::NeedsYou(_))
    }
}

// ---- the record ---------------------------------------------------------------------------------------------------

/// The git worktree platoon made for a task.
///
/// In the file: `{"path", "branch", "base"}`, then only when there is something to say `"removed"`,
/// `"kept_uncommitted"` and `"opened_in"`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Worktree {
    /// Where the session works: the task's folder inside the worktree.
    pub path: String,
    pub branch: String,
    /// The commit it was made from (what "what did this task change" compares the branch with).
    pub base: Option<String>,
    /// When platoon removed its folder (seconds since the epoch): the task was Done or Cancelled and its session had
    /// stopped. Its branch is kept, and opening the task's session again makes it again from there.
    pub removed: Option<u64>,
    /// Left in place after the work: it had changes not yet committed.
    pub kept_uncommitted: bool,
    /// Opened in an editor with a window of its own (its name), which platoon cannot tell has closed: kept after the
    /// work until the user removes it.
    pub opened_in: Option<String>,
}

impl Worktree {
    /// Its folder is gone (platoon removed it, or it isn't there any more).
    pub fn is_gone(&self) -> bool {
        self.removed.is_some() || !Path::new(&self.path).exists()
    }
}

/// What platoon writes down about a task's worktree after the work (`Tickets::mark_worktree`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WtMark {
    /// Its folder was removed (the branch kept).
    Removed,
    /// Left in place: it has changes not yet committed.
    KeptUncommitted,
    /// It is there again (made again from its branch): neither of the above.
    Present,
}

/// Where a task came from: the Claude Code session it was drafted from, and that session's project then.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub session: String,
    pub project: String,
}

/// One run of the project's tests at the end of a turn, newest last; a task keeps `TESTS_KEPT`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct TestRun {
    pub ok: bool,
    /// One line: "214 passed", "211 passed, 3 failed", "timed out after 10 minutes".
    pub summary: String,
    pub command: String,
    /// When it ended, seconds since the epoch.
    pub at: u64,
}

/// The last check of a task's work (`verdict.rs`): a fresh read-only `claude -p` judged whether the work does what
/// the task asked. Kept on the task (the last one only); its whole answer is `verdict.json` in the task's own folder.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Verdict {
    /// When it ended, seconds since the epoch.
    pub at: u64,
    /// The model that checked (the squad's `verdict_model`).
    pub model: String,
    /// It could not run, and why. Then nothing below counts.
    pub failed: Option<String>,
    /// Done, as platoon counts it: the check said so, or it found nothing that holds up.
    pub done: bool,
    /// The task's "Done when" boxes as the check found them.
    pub boxes: Vec<BoxCheck>,
    /// What it found wrong, each quote found where it said.
    pub findings: Vec<Finding>,
    /// Findings left out because their quote isn't in the file near that line.
    pub dropped: u32,
    /// Why it counts as done though the check said no ("the check's findings could not be found in the code").
    pub note: String,
    /// What it used.
    pub tokens: u64,
}

/// One "Done when" box as the check found it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BoxCheck {
    pub text: String,
    pub met: bool,
}

/// One problem the check found: where (relative to the task's folder) and what.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Finding {
    pub file: String,
    pub line: u64,
    pub problem: String,
}

impl Finding {
    /// `file:line: problem`.
    pub fn line_text(&self) -> String {
        format!("{}:{}: {}", self.file, self.line, self.problem)
    }
}

impl Verdict {
    /// The unmet boxes.
    pub fn unmet(&self) -> impl Iterator<Item = &BoxCheck> {
        self.boxes.iter().filter(|b| !b.met)
    }

    /// How many problems it found: the findings and the unmet boxes.
    pub fn problems(&self) -> usize {
        self.findings.len() + self.unmet().count()
    }

    fn to_json(&self) -> Value {
        let mut o = json!({
            "at": self.at,
            "model": self.model,
            "done": self.done,
            "boxes": self.boxes.iter().map(|b| json!({"text": b.text, "met": b.met})).collect::<Vec<_>>(),
            "findings": self.findings.iter().map(|f| json!({"file": f.file, "line": f.line, "problem": f.problem})).collect::<Vec<_>>(),
            "tokens": self.tokens,
        });
        if let Some(o) = o.as_object_mut() {
            if let Some(why) = &self.failed {
                o.insert("failed".into(), json!(why));
            }
            if self.dropped > 0 {
                o.insert("dropped".into(), json!(self.dropped));
            }
            if !self.note.is_empty() {
                o.insert("note".into(), json!(self.note));
            }
        }
        o
    }

    fn from_json(v: &Value) -> Option<Verdict> {
        let o = v.as_object()?;
        let s = |v: &Value, k: &str, max: usize| clean(v.get(k).and_then(Value::as_str).unwrap_or_default(), max);
        let list = |k: &str| o.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
        Some(Verdict {
            at: o.get("at").and_then(Value::as_u64).unwrap_or(0),
            model: s(v, "model", 60),
            failed: o.get("failed").and_then(Value::as_str).map(|w| clean(w, WHY_MAX)),
            done: o.get("done").and_then(Value::as_bool).unwrap_or(false),
            boxes: list("boxes")
                .iter()
                .filter_map(|b| Some(BoxCheck { text: s(b, "text", WHY_MAX), met: b.get("met")?.as_bool()? }))
                .collect(),
            findings: list("findings")
                .iter()
                .filter_map(|f| {
                    Some(Finding { file: s(f, "file", 300), line: f.get("line")?.as_u64()?, problem: s(f, "problem", WHY_MAX) })
                })
                .collect(),
            dropped: o.get("dropped").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).unwrap_or(0),
            note: s(v, "note", WHY_MAX),
            tokens: o.get("tokens").and_then(Value::as_u64).unwrap_or(0),
        })
    }
}

/// How many times in a row a turn's end is refused because the check found problems; the next time the task needs
/// the user.
pub const CHECKS_REFUSED_MAX: u32 = 1;

/// How many test runs a task keeps.
pub const TESTS_KEPT: usize = 5;

/// How many times in a row a turn's end is refused because the tests fail; the next time the task needs the user.
pub const STOPS_REFUSED_MAX: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Ticket {
    pub number: u32,
    pub title: String,
    /// The text's file name inside the project's `tasks/` folder.
    pub file: String,
    pub state: State,
    /// The one folder it works in (absolute). None: asked for when it is started.
    pub folder: Option<String>,
    /// The squad suggested for it (before it starts), or the one it runs with: its id. Empty: none.
    pub squad: String,
    /// Its Claude Code session, once started.
    pub session: Option<String>,
    /// When it was started (seconds since the epoch).
    pub started: u64,
    pub worktree: Option<Worktree>,
    /// Seconds since the epoch.
    pub created: u64,
    pub updated: u64,
    pub from: Option<Source>,
    pub tests: Vec<TestRun>,
    /// How many ends of a turn in a row were refused because the tests failed (reset when the user types).
    pub stops_refused: u32,
    /// The last check of its work.
    pub verdict: Option<Verdict>,
    /// How many ends of a turn in a row were refused because the check found problems (reset when the user types).
    pub checks_refused: u32,
    /// The check of its work is running, since then (seconds since the epoch). Cleared when it ends or the user types.
    pub checking: Option<u64>,
}

impl Ticket {
    /// Where its session works: its worktree, else its folder.
    pub fn work_folder(&self) -> Option<String> {
        match &self.worktree {
            Some(w) => Some(w.path.clone()),
            None => self.folder.clone(),
        }
    }

    /// The branch its work goes on.
    pub fn branch(&self) -> String {
        self.worktree.as_ref().map(|w| w.branch.clone()).filter(|b| !b.is_empty()).unwrap_or_else(|| task_branch(self.number))
    }

    /// The latest test run.
    pub fn last_tests(&self) -> Option<&TestRun> {
        self.tests.last()
    }

    /// It has a session (it was started).
    pub fn started(&self) -> bool {
        self.session.is_some()
    }
}

/// The branch a task's work goes on.
pub fn task_branch(number: u32) -> String {
    format!("task/{number}")
}

/// One line, trimmed, at most `max` characters.
pub fn clean(text: &str, max: usize) -> String {
    crate::projects::clean(text, max)
}

/// The part of a text file's name after the number: the title in lower case, words joined with `-`, plain letters and
/// digits only.
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
    if out.is_empty() { "task".into() } else { out }
}

fn now_secs() -> u64 {
    crate::usage::now()
}

fn ticket_from(v: &Value) -> Option<Ticket> {
    let o = v.as_object()?;
    let number = u32::try_from(o.get("number")?.as_u64()?).ok().filter(|n| *n > 0)?;
    let s = |k: &str| o.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let state = match o.get("state").and_then(Value::as_str).unwrap_or_default() {
        "working" => State::Working,
        "needs_you" => State::NeedsYou(Need::from(&s("need"), &s("why"))),
        "done" => State::Done,
        "cancelled" => State::Cancelled,
        _ => State::NotStarted,
    };
    Some(Ticket {
        number,
        title: clean(&s("title"), TITLE_MAX),
        file: s("file"),
        state,
        folder: o.get("folder").and_then(Value::as_str).filter(|f| !f.is_empty()).map(String::from),
        squad: s("squad"),
        session: o.get("session").and_then(Value::as_str).filter(|f| !f.is_empty()).map(String::from),
        started: o.get("started").and_then(Value::as_u64).unwrap_or(0),
        worktree: o.get("worktree").and_then(|w| {
            let s = |k: &str| w.get(k).and_then(Value::as_str).map(String::from);
            Some(Worktree {
                path: s("path").filter(|p| !p.is_empty())?,
                branch: s("branch").unwrap_or_default(),
                base: s("base").filter(|b| is_commit_id(b)),
                removed: w.get("removed").and_then(Value::as_u64),
                kept_uncommitted: w.get("kept_uncommitted").and_then(Value::as_bool).unwrap_or(false),
                opened_in: s("opened_in").filter(|n| !n.is_empty()),
            })
        }),
        created: o.get("created").and_then(Value::as_u64).unwrap_or(0),
        updated: o.get("updated").and_then(Value::as_u64).unwrap_or(0),
        from: o.get("from").and_then(|f| {
            let session = f.get("session")?.as_str().filter(|s| !s.is_empty())?.to_string();
            Some(Source { session, project: f.get("project").and_then(Value::as_str).unwrap_or_default().to_string() })
        }),
        tests: o
            .get("tests")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|r| {
                        Some(TestRun {
                            ok: r.get("ok")?.as_bool()?,
                            summary: clean(r.get("summary").and_then(Value::as_str).unwrap_or_default(), crate::checks::SUMMARY_MAX),
                            command: r.get("command").and_then(Value::as_str).unwrap_or_default().to_string(),
                            at: r.get("at").and_then(Value::as_u64).unwrap_or(0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        stops_refused: o.get("stops_refused").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).unwrap_or(0),
        verdict: o.get("verdict").and_then(Verdict::from_json),
        checks_refused: o.get("checks_refused").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).unwrap_or(0),
        checking: o.get("checking").and_then(Value::as_u64),
    })
}

/// A commit id as git writes it: 4 to 64 hex digits.
fn is_commit_id(s: &str) -> bool {
    (4..=64).contains(&s.len()) && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Write `t`'s fields into its object in the file, keeping the keys platoon doesn't know (the worktree's too).
fn ticket_into(t: &Ticket, o: &mut Map<String, Value>) {
    let mut set = |k: &str, v: Option<Value>| match v {
        Some(v) => {
            o.insert(k.into(), v);
        }
        None => {
            o.remove(k);
        }
    };
    set("title", Some(json!(t.title)));
    set("file", Some(json!(t.file)));
    set("state", Some(json!(t.state.id())));
    match &t.state {
        State::NeedsYou(n) => {
            set("need", Some(json!(n.id())));
            set("why", (!n.text().is_empty()).then(|| json!(n.text())));
        }
        _ => {
            set("need", None);
            set("why", None);
        }
    }
    set("folder", t.folder.as_ref().map(|f| json!(f)));
    set("squad", (!t.squad.is_empty()).then(|| json!(t.squad)));
    set("session", t.session.as_ref().map(|s| json!(s)));
    set("started", (t.started > 0).then(|| json!(t.started)));
    set("created", Some(json!(t.created)));
    set("updated", Some(json!(t.updated)));
    set("from", t.from.as_ref().map(|f| json!({"session": f.session, "project": f.project})));
    set(
        "tests",
        (!t.tests.is_empty()).then(|| {
            Value::Array(t.tests.iter().map(|r| json!({"ok": r.ok, "summary": r.summary, "command": r.command, "at": r.at})).collect())
        }),
    );
    set("stops_refused", (t.stops_refused > 0).then(|| json!(t.stops_refused)));
    set("verdict", t.verdict.as_ref().map(Verdict::to_json));
    set("checks_refused", (t.checks_refused > 0).then(|| json!(t.checks_refused)));
    set("checking", t.checking.map(|at| json!(at)));
    let old = o.get("worktree").and_then(Value::as_object).cloned();
    let w = t.worktree.as_ref().map(|w| {
        let mut x = old.unwrap_or_default();
        x.insert("path".into(), json!(w.path));
        x.insert("branch".into(), json!(w.branch));
        match &w.base {
            Some(b) => x.insert("base".into(), json!(b)),
            None => x.remove("base"),
        };
        match w.removed {
            Some(at) => x.insert("removed".into(), json!(at)),
            None => x.remove("removed"),
        };
        if w.kept_uncommitted {
            x.insert("kept_uncommitted".into(), json!(true));
        } else {
            x.remove("kept_uncommitted");
        }
        match &w.opened_in {
            Some(n) => x.insert("opened_in".into(), json!(n)),
            None => x.remove("opened_in"),
        };
        Value::Object(x)
    });
    match w {
        Some(w) => {
            o.insert("worktree".into(), w);
        }
        None => {
            o.remove("worktree");
        }
    }
}

/// What a new task is made of.
#[derive(Clone, Debug, Default)]
pub struct NewTicket {
    pub title: String,
    /// None: the template.
    pub body: Option<String>,
    pub folder: Option<String>,
    /// The squad suggested for it.
    pub squad: String,
    /// The session it is drafted from, if any.
    pub from: Option<Source>,
}

/// The project id of the tasks in no project (`<config>/tasks.json`).
pub const NO_PROJECT_ID: &str = "";

/// What the tasks in no project are listed under.
pub const NO_PROJECT_NAME: &str = "No project";

/// The list's file name, and its key for the tasks.
pub const FILE: &str = "tasks.json";
const KEY: &str = "tasks";
const BODIES: &str = "tasks";

/// The folder a project's task files are in (the config folder itself for the tasks in no project).
fn folder_of(config: &Path, project: &str) -> PathBuf {
    if project == NO_PROJECT_ID { config.to_path_buf() } else { config.join("projects").join(project) }
}

/// Where a project's tasks file is.
pub fn path_in(config: &Path, project: &str) -> PathBuf {
    folder_of(config, project).join(FILE)
}

/// Where a project's task texts are.
pub fn bodies_dir(config: &Path, project: &str) -> PathBuf {
    folder_of(config, project).join(BODIES)
}

/// The task's own folder (`tasks/<NN>-work/`): the squad it runs with, the other squads, its plan, the last test
/// output. Made when it starts; platoon never removes it.
pub fn task_dir(config: &Path, project: &str, number: u32) -> PathBuf {
    bodies_dir(config, project).join(format!("{number:02}-work"))
}

/// The check's whole answer, in the task's own folder.
pub const VERDICT_FILE: &str = "verdict.json";

/// The plan's file name, in the task's own folder.
pub const PLAN_FILE: &str = "plan.md";

/// Where a task's plan is kept.
pub fn plan_file(config: &Path, project: &str, number: u32) -> PathBuf {
    task_dir(config, project, number).join(PLAN_FILE)
}

/// What the tasks file says the user must look at, in the words the screen shows.
pub const CORRUPT: &str = "The tasks file can't be read, so platoon leaves it alone. Fix or move the file to go on.";

/// The tasks of one project (or of none: `NO_PROJECT_ID`), as the file says.
pub struct Tickets {
    config: PathBuf,
    project: String,
    list: Vec<Ticket>,
    stamp: Option<(SystemTime, u64)>,
    corrupt: bool,
    /// Bumped whenever what is in memory changed.
    pub generation: u64,
}

impl Tickets {
    pub fn open(config: &Path, project: &str) -> Self {
        let mut t = Self {
            config: config.to_path_buf(),
            project: project.to_string(),
            list: Vec::new(),
            stamp: None,
            corrupt: false,
            generation: 0,
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

    /// Every task, in the order of the file.
    pub fn all(&self) -> &[Ticket] {
        &self.list
    }

    pub fn get(&self, number: u32) -> Option<&Ticket> {
        self.list.iter().find(|t| t.number == number)
    }

    /// The full path of a task's text.
    pub fn body_path(&self, t: &Ticket) -> PathBuf {
        bodies_dir(&self.config, &self.project).join(&t.file)
    }

    /// The text as it is on disk now ("" when there is none).
    pub fn body(&self, t: &Ticket) -> String {
        if t.file.is_empty() {
            return String::new();
        }
        fs::read_to_string(self.body_path(t)).unwrap_or_default()
    }

    /// The task's plan, if it has written one.
    pub fn plan(&self, number: u32) -> Option<String> {
        let text = fs::read_to_string(plan_file(&self.config, &self.project, number)).ok()?;
        (!text.trim().is_empty()).then_some(text)
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
            Ok(Value::Object(o)) if o.get(KEY).is_none_or(Value::is_array) && o.get("next").is_none_or(Value::is_u64) => Ok(Some(o)),
            _ => Err(()),
        }
    }

    fn load(&mut self) {
        self.stamp = self.stamp_now();
        self.list.clear();
        self.corrupt = false;
        self.generation += 1;
        match self.read() {
            Ok(Some(root)) => {
                // Anything in the list that isn't shaped like a task is somebody else's: kept on disk, not shown.
                self.list = root.get(KEY).and_then(Value::as_array).into_iter().flatten().filter_map(ticket_from).collect();
            }
            Ok(None) => {}
            Err(()) => self.corrupt = true,
        }
    }

    /// Pick up a change made by a hook, `platoon task add` or another window. One `stat` when nothing changed.
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
        let path = self.path();
        let _lock = lock(&path)?;
        let mut root = match self.read() {
            Ok(o) => o.unwrap_or_default(),
            Err(()) => {
                self.load();
                return Err(io::Error::other(CORRUPT));
            }
        };
        if !root.get(KEY).is_some_and(Value::is_array) {
            root.insert(KEY.into(), json!([]));
        }
        let now: Vec<Ticket> = root.get(KEY).and_then(Value::as_array).into_iter().flatten().filter_map(ticket_from).collect();
        let out = match f(&mut root, &now) {
            Ok(v) => v,
            Err(e) => {
                self.load();
                return Err(io::Error::other(e));
            }
        };
        let text = serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_default();
        let written = write_atomic(&path, &text);
        self.load();
        written.map(|()| out)
    }

    /// A new task: the next number, Not started, its text written. Returns the number.
    pub fn add(&mut self, new: NewTicket) -> io::Result<u32> {
        let title = clean(&new.title, TITLE_MAX);
        if title.is_empty() {
            return Err(io::Error::other("a task needs a title"));
        }
        let dir = bodies_dir(&self.config, &self.project);
        let body = new.body.unwrap_or_else(|| TEMPLATE.to_string());
        let now = now_secs();
        let written_file: std::cell::RefCell<Option<PathBuf>> = std::cell::RefCell::new(None);
        let result = self.change(|root, all| {
            let number = next_number(root).max(all.iter().map(|t| t.number + 1).max().unwrap_or(1));
            let stem = format!("{number:02}-{}", slug(&title));
            let mut file = format!("{stem}.md");
            let mut k = 2;
            while dir.join(&file).exists() {
                file = format!("{stem}-{k}.md");
                k += 1;
            }
            fs::create_dir_all(&dir).map_err(|e| format!("couldn't make the tasks folder: {e}"))?;
            write_atomic(&dir.join(&file), &body).map_err(|e| format!("couldn't write the task: {e}"))?;
            *written_file.borrow_mut() = Some(dir.join(&file));
            let t = Ticket {
                number,
                title: title.clone(),
                file,
                folder: new.folder.clone(),
                squad: new.squad.clone(),
                created: now,
                updated: now,
                from: new.from.clone(),
                ..Default::default()
            };
            let mut o = Map::new();
            o.insert("number".into(), json!(number));
            ticket_into(&t, &mut o);
            root.get_mut(KEY).and_then(Value::as_array_mut).ok_or("tasks")?.push(Value::Object(o));
            root.insert("next".into(), json!(number + 1));
            Ok(number)
        });
        // The list didn't take it: the text file it wrote goes too, so nothing is left without a task.
        if result.is_err()
            && let Some(p) = written_file.take()
        {
            let _ = fs::remove_file(p);
        }
        result
    }

    /// Change task `number` when `f` says so (it returns false having changed nothing: then nothing is written, not
    /// even the time). True when it changed. The hooks that run after every tool call use this, so a task with
    /// nothing to change costs one read.
    pub fn update(&mut self, number: u32, f: impl FnOnce(&mut Ticket) -> bool) -> io::Result<bool> {
        self.update_at(number, false, f)
    }

    /// `update`, keeping the task's `updated` time as it is (what platoon writes down after the work, not news).
    fn update_at(&mut self, number: u32, keep_time: bool, f: impl FnOnce(&mut Ticket) -> bool) -> io::Result<bool> {
        const NOTHING: &str = "\u{0}nothing to change";
        let now = now_secs();
        let r = self.change(|root, _| {
            let o = root
                .get_mut(KEY)
                .and_then(Value::as_array_mut)
                .and_then(|a| a.iter_mut().find(|t| t.get("number").and_then(Value::as_u64) == Some(u64::from(number))))
                .and_then(Value::as_object_mut)
                .ok_or_else(|| format!("there is no task #{number} any more"))?;
            let mut t = ticket_from(&Value::Object(o.clone())).ok_or_else(|| format!("#{number} can't be read"))?;
            if !f(&mut t) {
                return Err(NOTHING.to_string());
            }
            if !keep_time {
                t.updated = now;
            }
            ticket_into(&t, o);
            Ok(())
        });
        match r {
            Ok(()) => Ok(true),
            Err(e) if e.to_string() == NOTHING => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// The text changed in the user's editor: note the time.
    pub fn touch(&mut self, number: u32) -> io::Result<()> {
        self.update(number, |_| true).map(|_| ())
    }

    /// `c`: Cancelled (the only state the user sets by hand).
    pub fn cancel(&mut self, number: u32) -> io::Result<()> {
        self.update(number, |t| {
            t.state = State::Cancelled;
            true
        })
        .map(|_| ())
    }

    pub fn set_folder(&mut self, number: u32, folder: &str) -> io::Result<()> {
        let folder = folder.to_string();
        self.update(number, |t| {
            t.folder = Some(folder);
            true
        })
        .map(|_| ())
    }

    /// The task's session was started: with this squad, in this worktree (None: in its folder). Working.
    pub fn start(&mut self, number: u32, squad: &str, session: &str, worktree: Option<Worktree>) -> io::Result<()> {
        let (squad, session) = (squad.to_string(), session.to_string());
        let now = now_secs();
        self.update(number, |t| {
            t.squad = squad;
            t.session = Some(session);
            t.started = now;
            t.worktree = worktree;
            t.state = State::Working;
            t.stops_refused = 0;
            t.checks_refused = 0;
            true
        })
        .map(|_| ())
    }

    /// The session couldn't be started after all: back to Not started, without it.
    pub fn forget_start(&mut self, number: u32) -> io::Result<()> {
        self.update(number, |t| {
            t.session = None;
            t.started = 0;
            t.state = State::NotStarted;
            true
        })
        .map(|_| ())
    }

    /// Write down what became of task `number`'s worktree (the task's `updated` stays). True when it changed.
    pub fn mark_worktree(&mut self, number: u32, mark: WtMark) -> io::Result<bool> {
        let now = now_secs();
        self.update_at(number, true, |t| {
            let Some(w) = t.worktree.as_mut() else { return false };
            let (removed, kept) = (w.removed.is_some(), w.kept_uncommitted);
            match mark {
                WtMark::Removed if !removed => {
                    w.removed = Some(now);
                    w.kept_uncommitted = false;
                    w.opened_in = None;
                }
                WtMark::KeptUncommitted if !kept || removed => {
                    w.kept_uncommitted = true;
                    w.removed = None;
                }
                WtMark::Present if removed || kept => {
                    w.removed = None;
                    w.kept_uncommitted = false;
                }
                _ => return false,
            }
            true
        })
    }

    /// Task `number`'s worktree was opened in `editor`, a windowed editor platoon cannot tell has closed: kept after
    /// the work until the user removes it. True when that changed anything.
    pub fn mark_opened_in(&mut self, number: u32, editor: &str) -> io::Result<bool> {
        let editor = editor.to_string();
        self.update_at(number, true, |t| {
            let Some(w) = t.worktree.as_mut().filter(|w| w.removed.is_none() && w.opened_in.as_deref() != Some(editor.as_str())) else {
                return false;
            };
            w.opened_in = Some(editor);
            true
        })
    }
}

/// The number the next task gets: the file's `next`, and never one a task in it already has.
fn next_number(root: &Map<String, Value>) -> u32 {
    let next = root.get("next").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).unwrap_or(1).max(1);
    let past = root
        .get(KEY)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| t.get("number").and_then(Value::as_u64))
        .max()
        .and_then(|n| u32::try_from(n + 1).ok())
        .unwrap_or(1);
    next.max(past)
}

// ---- across every project -----------------------------------------------------------------------------------------

/// Every tasks file there is, with its project id; the tasks in no project first.
pub fn all_files(config: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = fs::read_dir(config.join("projects"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let id = e.file_name().to_str()?.to_string();
            let p = path_in(config, &id);
            p.is_file().then_some((id, p))
        })
        .collect();
    out.sort();
    let loose = path_in(config, NO_PROJECT_ID);
    if loose.is_file() {
        out.insert(0, (NO_PROJECT_ID.to_string(), loose));
    }
    out
}

/// Every task of every project, with its project's id (a file that can't be read gives none).
pub fn every_ticket(config: &Path) -> Vec<(String, Ticket)> {
    let mut out = Vec::new();
    for (project, _) in all_files(config) {
        out.extend(Tickets::open(config, &project).all().iter().cloned().map(|t| (project.clone(), t)));
    }
    out
}

/// The task whose session is `session`: its project's id and its number.
pub fn find_session(config: &Path, session: &str) -> Option<(String, u32)> {
    if session.is_empty() {
        return None;
    }
    every_ticket(config).into_iter().find(|(_, t)| t.session.as_deref() == Some(session)).map(|(p, t)| (p, t.number))
}

/// Is `session` the session of a task that is Working? Such a session is never idle-stopped: its subagents may be at
/// work while nobody watches it.
pub fn session_at_work(config: &Path, session: &str) -> bool {
    let Some((project, number)) = find_session(config, session) else { return false };
    Tickets::open(config, &project).get(number).is_some_and(|t| t.state == State::Working)
}

// ---- the command line ---------------------------------------------------------------------------------------------

/// Why an archived project takes no new task (the command line and the app say the same).
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
        _ => Err(format!(
            "more than one project is called \"{want}\"; use its id instead ({})",
            named.iter().map(|p| p.id.as_str()).collect::<Vec<_>>().join(", ")
        )),
    }
}

const USAGE: &str = "usage: platoon task add [--project <name or id>] --title <text> [--body-file <path>] [--folder <path>] [--squad <id>]
       platoon task list [--project <name or id>] [--json]
  add    make a task and print its number (#N). The text comes from --body-file, else from what is piped in,
         else the template. Without --folder the task works in the current folder. With no --project, the task
         goes in the project whose sessions worked in the current folder, or in no project (\"No project\").
         --squad suggests the squad that starts it (an id from the Squads tab)
  list   the tasks, open ones first, then the Done and Cancelled ones";

/// Where the command line looks when no project is named: the sessions' folders (the index platoon keeps in its
/// cache, read as it is). None (a unit test, or no cache): no lookup, and the task goes in no project.
#[derive(Clone, Copy)]
pub struct Roots<'a> {
    pub claude_root: &'a Path,
    pub cache: &'a Path,
}

/// `platoon task ...`. Returns the exit code; what it prints goes to `out`, problems to `err`.
#[allow(clippy::too_many_arguments)]
pub fn cli(
    args: &[String],
    config: &Path,
    roots: Option<Roots>,
    stdin: &mut dyn io::Read,
    stdin_is_terminal: bool,
    cwd: &Path,
    out: &mut dyn io::Write,
    err: &mut dyn io::Write,
) -> i32 {
    match args.first().map(String::as_str) {
        Some("add") => match cli_add(&args[1..], config, roots, stdin, stdin_is_terminal, cwd) {
            Ok(n) => {
                let _ = writeln!(out, "#{n}");
                0
            }
            Err(why) => {
                let _ = writeln!(err, "platoon task add: {why}");
                1
            }
        },
        Some("list") => match cli_list(&args[1..], config) {
            Ok(text) => {
                let _ = write!(out, "{text}");
                0
            }
            Err(why) => {
                let _ = writeln!(err, "platoon task list: {why}");
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

/// The project the folder `cwd` belongs to, by the sessions that worked in it (or in a folder above it), as the
/// index says: Ok(Some) for one project, Ok(None) for none, Err when sessions of more than one project did.
pub fn project_of_folder(projects: &crate::projects::Projects, roots: Option<Roots>, cwd: &Path) -> Result<Option<String>, String> {
    let Some(r) = roots else { return Ok(None) };
    let here = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let mut found: Vec<String> = Vec::new();
    for e in crate::index::Scanner::load(r.claude_root, r.cache).entries() {
        let Some(dir) = e.t.cwd.as_deref().filter(|d| !d.is_empty()) else { continue };
        if !here.starts_with(Path::new(dir)) {
            continue;
        }
        if let Some(p) = projects.project_id_of(&e.id).map(String::from)
            && !found.contains(&p)
        {
            found.push(p);
        }
    }
    match found.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(one.clone())),
        many => {
            let names: Vec<String> =
                many.iter().map(|id| format!("\"{}\"", projects.get(id).map_or(id.as_str(), |p| p.name.as_str()))).collect();
            Err(format!("sessions of more than one project worked in this folder ({}); say which with --project", names.join(", ")))
        }
    }
}

fn cli_add(
    args: &[String],
    config: &Path,
    roots: Option<Roots>,
    stdin: &mut dyn io::Read,
    stdin_is_terminal: bool,
    cwd: &Path,
) -> Result<u32, String> {
    let (mut project, mut title, mut body_file, mut folder, mut squad) = (None, None, None, None, String::new());
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--project" => project = Some(value(args, &mut i, "--project")?.to_string()),
            "--squad" => {
                let want = value(args, &mut i, "--squad")?.trim().to_string();
                let all = crate::squads::all(config);
                if !all.iter().any(|s| s.id == want) {
                    let ids: Vec<&str> = all.iter().map(|s| s.id.as_str()).collect();
                    return Err(format!("there is no squad \"{want}\" (there are: {})", ids.join(", ")));
                }
                squad = want;
            }
            "--title" => title = Some(value(args, &mut i, "--title")?.to_string()),
            "--body-file" => body_file = Some(value(args, &mut i, "--body-file")?.to_string()),
            "--folder" => {
                let f = value(args, &mut i, "--folder")?;
                let p = if Path::new(f).is_absolute() { PathBuf::from(f) } else { cwd.join(f) };
                if !p.is_dir() {
                    return Err(format!("the folder {f} does not exist"));
                }
                folder = Some(p.canonicalize().unwrap_or(p).to_string_lossy().into_owned());
            }
            other => return Err(format!("don't know \"{other}\" (see platoon task)")),
        }
        i += 1;
    }
    let title = title.filter(|t| !clean(t, TITLE_MAX).is_empty()).ok_or("give the task a title with --title <text>")?;
    let body = match body_file {
        Some(f) => {
            let p = if Path::new(&f).is_absolute() { PathBuf::from(&f) } else { cwd.join(&f) };
            Some(fs::read_to_string(&p).map_err(|e| format!("couldn't read {f}: {}", plain(&e)))?)
        }
        None if !stdin_is_terminal => {
            let mut text = String::new();
            stdin.read_to_string(&mut text).map_err(|e| format!("couldn't read the text from stdin: {}", plain(&e)))?;
            Some(text).filter(|t| !t.trim().is_empty())
        }
        None => None,
    };
    let projects = crate::projects::Projects::open(config);
    let p: Option<&crate::projects::Project> = match &project {
        Some(want) => Some(find_project(&projects, want)?),
        None => project_of_folder(&projects, roots, cwd)?.and_then(|id| projects.get(&id)),
    };
    if let Some(p) = p
        && p.archived
    {
        return Err(archived_refusal(&p.name));
    }
    let project_id = p.map_or(NO_PROJECT_ID.to_string(), |p| p.id.clone());
    // No --folder: the folder it was run in.
    let folder = folder.unwrap_or_else(|| cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf()).to_string_lossy().into_owned());
    let mut tickets = Tickets::open(config, &project_id);
    if tickets.is_corrupt() {
        return Err(CORRUPT.into());
    }
    tickets.add(NewTicket { title, body, folder: Some(folder), squad, ..Default::default() }).map_err(|e| plain(&e))
}

fn plain(e: &io::Error) -> String {
    let why = e.to_string();
    why.split(" (os error").next().unwrap_or(&why).to_string()
}

/// What a task's state says in a list: its label, and why it needs the user.
pub fn state_words(t: &Ticket) -> String {
    match &t.state {
        State::NeedsYou(n) => format!("Needs you: {}", n.words()),
        s => s.label().to_string(),
    }
}

fn cli_list(args: &[String], config: &Path) -> Result<String, String> {
    let (mut project, mut as_json) = (None, false);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--project" => project = Some(value(args, &mut i, "--project")?.to_string()),
            "--json" => as_json = true,
            other => return Err(format!("don't know \"{other}\" (see platoon task)")),
        }
        i += 1;
    }
    let projects = crate::projects::Projects::open(config);
    // (name, id): every project, then the tasks in no project; or the one asked for.
    let chosen: Vec<(String, String)> = match &project {
        Some(want) => {
            let p = find_project(&projects, want)?;
            vec![(p.name.clone(), p.id.clone())]
        }
        None => projects
            .all()
            .iter()
            .map(|p| (p.name.clone(), p.id.clone()))
            .chain(std::iter::once((NO_PROJECT_NAME.to_string(), NO_PROJECT_ID.to_string())))
            .collect(),
    };
    let mut text = String::new();
    let mut rows: Vec<Value> = Vec::new();
    for (name, id) in chosen {
        let tickets = Tickets::open(config, &id);
        if tickets.is_corrupt() {
            if project.is_some() {
                return Err(CORRUPT.into());
            }
            text.push_str(&format!("{name}\n  {CORRUPT}\n\n"));
            continue;
        }
        if tickets.all().is_empty() && project.is_none() {
            continue;
        }
        let mut open: Vec<&Ticket> = tickets.all().iter().filter(|t| !t.state.closed()).collect();
        let mut closed: Vec<&Ticket> = tickets.all().iter().filter(|t| t.state.closed()).collect();
        open.sort_by_key(|t| t.number);
        closed.sort_by_key(|t| t.number);
        text.push_str(&format!("{name}\n"));
        if tickets.all().is_empty() {
            text.push_str("  No tasks yet.\n");
        }
        let line = |t: &Ticket| {
            let why = match &t.state {
                State::NeedsYou(n) if !n.text().is_empty() => format!("  {}", n.text()),
                _ => String::new(),
            };
            format!("  #{:<3} {}  ·  {}{why}\n", t.number, t.title, state_words(t))
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
            let mut row = json!({
                "project": if id == NO_PROJECT_ID { Value::Null } else { json!(name) },
                "project_id": if id == NO_PROJECT_ID { Value::Null } else { json!(id) },
                "number": t.number,
                "title": t.title,
                "state": t.state.label(),
                "folder": t.folder,
                "squad": t.squad,
                "file": tickets.body_path(t).to_string_lossy(),
            });
            if let (State::NeedsYou(n), Some(o)) = (&t.state, row.as_object_mut()) {
                o.insert("needs".into(), json!(n.words()));
                if !n.text().is_empty() {
                    o.insert("why".into(), json!(n.text()));
                }
            }
            rows.push(row);
        }
    }
    if as_json {
        return Ok(serde_json::to_string_pretty(&Value::Array(rows)).unwrap_or_default() + "\n");
    }
    if text.is_empty() {
        text = "No tasks yet.\n".into();
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

    fn set(t: &mut Tickets, n: u32, s: State) {
        t.update(n, |x| {
            x.state = s;
            true
        })
        .unwrap();
    }

    #[test]
    fn numbers_start_at_one_and_are_never_used_twice_even_after_cancelling() {
        let d = dir("numbers");
        let mut t = Tickets::open(&d, "p1");
        assert!(t.all().is_empty() && !d.exists(), "reading makes nothing");
        assert_eq!(add(&mut t, "First thing"), 1);
        assert_eq!(add(&mut t, "Second thing"), 2);
        t.cancel(2).unwrap();
        assert_eq!(add(&mut t, "Third"), 3, "a cancelled task keeps its number");
        let mut v = saved(&d);
        v.get_mut("tasks").unwrap().as_array_mut().unwrap().retain(|x| x["number"] != 3);
        fs::write(path_in(&d, "p1"), v.to_string()).unwrap();
        assert_eq!(add(&mut Tickets::open(&d, "p1"), "Fourth"), 4, "`next` remembers");
        let mut v = saved(&d);
        v.as_object_mut().unwrap().insert("next".into(), json!(1));
        fs::write(path_in(&d, "p1"), v.to_string()).unwrap();
        assert_eq!(add(&mut Tickets::open(&d, "p1"), "Fifth"), 5, "never a number a task has");
        let t = Tickets::open(&d, "p1");
        let first = t.get(1).unwrap();
        assert_eq!((first.title.as_str(), first.file.as_str(), &first.state), ("First thing", "01-first-thing.md", &State::NotStarted));
        assert_eq!(t.body(first), TEMPLATE);
        assert!(
            Tickets::open(&d, "p1").add(NewTicket { title: "   ".into(), ..Default::default() }).unwrap_err().to_string().contains("title")
        );
    }

    #[test]
    fn the_template_says_what_it_should_do_and_when_it_is_done() {
        assert!(TEMPLATE.starts_with("## What it should do\n"));
        assert!(TEMPLATE.contains("## Done when\n\n- [ ] \n- [ ] \n"));
        assert!(!TEMPLATE.contains('/'), "no paths");
    }

    #[test]
    fn each_state_is_written_with_its_reason_as_data_and_read_back() {
        let d = dir("states");
        let mut t = Tickets::open(&d, "p1");
        let n = add(&mut t, "Login");
        for s in [
            State::Working,
            State::NeedsYou(Need::Plan),
            State::NeedsYou(Need::Question("Which page?".into())),
            State::NeedsYou(Need::Permission("Claude needs your permission to use Bash".into())),
            State::NeedsYou(Need::Tests("3 failed".into())),
            State::Done,
            State::Cancelled,
            State::NotStarted,
        ] {
            set(&mut t, n, s.clone());
            assert_eq!(Tickets::open(&d, "p1").get(n).unwrap().state, s);
            let v = &saved(&d)["tasks"][0];
            match &s {
                State::NeedsYou(need) => {
                    assert_eq!(v["state"], "needs_you");
                    assert_eq!(v["need"], need.id());
                    assert_eq!(v.get("why").and_then(Value::as_str).unwrap_or(""), need.text());
                }
                _ => assert!(v.get("need").is_none() && v.get("why").is_none(), "{v}"),
            }
        }
        let mut v = saved(&d);
        v["tasks"][0].as_object_mut().unwrap().insert("state".into(), json!("someday"));
        fs::write(path_in(&d, "p1"), v.to_string()).unwrap();
        assert_eq!(Tickets::open(&d, "p1").get(n).unwrap().state, State::NotStarted, "a state it doesn't know");
    }

    #[test]
    fn keys_platoon_does_not_know_survive_a_rewrite() {
        let d = dir("unknown");
        let mut t = Tickets::open(&d, "p1");
        let n = add(&mut t, "Login");
        t.start(n, "feature", "s-1", Some(Worktree { path: "/w".into(), branch: "task/1".into(), ..Default::default() })).unwrap();
        let mut v = saved(&d);
        v.as_object_mut().unwrap().insert("future".into(), json!({"a": 1}));
        let o = v["tasks"][0].as_object_mut().unwrap();
        o.insert("colour".into(), json!("blue"));
        o.get_mut("worktree").unwrap().as_object_mut().unwrap().insert("extra".into(), json!(true));
        fs::write(path_in(&d, "p1"), v.to_string()).unwrap();
        let mut t = Tickets::open(&d, "p1");
        set(&mut t, n, State::Done);
        let v = saved(&d);
        assert_eq!((v["future"]["a"].as_u64(), v["tasks"][0]["colour"].as_str()), (Some(1), Some("blue")));
        assert_eq!(v["tasks"][0]["worktree"]["extra"], true);
        assert_eq!((v["tasks"][0]["squad"].as_str(), v["tasks"][0]["session"].as_str()), (Some("feature"), Some("s-1")));
    }

    #[test]
    fn a_corrupt_file_is_never_written_over() {
        let d = dir("corrupt");
        fs::create_dir_all(d.join("projects/p1")).unwrap();
        fs::write(path_in(&d, "p1"), "{ not json").unwrap();
        let mut t = Tickets::open(&d, "p1");
        assert!(t.is_corrupt() && t.all().is_empty());
        assert!(t.add(NewTicket { title: "x".into(), ..Default::default() }).is_err());
        assert_eq!(fs::read_to_string(path_in(&d, "p1")).unwrap(), "{ not json");
        assert!(fs::read_dir(bodies_dir(&d, "p1")).map(|r| r.count()).unwrap_or(0) == 0, "no text file left behind");
    }

    #[test]
    fn an_update_that_changes_nothing_writes_nothing() {
        let d = dir("nothing");
        let mut t = Tickets::open(&d, "p1");
        let n = add(&mut t, "Login");
        let before = fs::read_to_string(path_in(&d, "p1")).unwrap();
        assert!(!t.update(n, |_| false).unwrap());
        assert_eq!(fs::read_to_string(path_in(&d, "p1")).unwrap(), before);
        assert!(t.update(99, |_| true).is_err(), "no such task");
    }

    #[test]
    fn two_writers_adding_at_once_get_different_numbers() {
        let d = dir("race");
        let handles: Vec<_> = (0..6)
            .map(|i| {
                let d = d.clone();
                std::thread::spawn(move || add(&mut Tickets::open(&d, "p1"), &format!("T{i}")))
            })
            .collect();
        let mut got: Vec<u32> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        got.sort_unstable();
        assert_eq!(got, vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn the_worktree_marks_keep_the_tasks_time_and_only_change_what_they_say() {
        let d = dir("marks");
        let mut t = Tickets::open(&d, "p1");
        let n = add(&mut t, "Login");
        assert!(!t.mark_worktree(n, WtMark::Removed).unwrap(), "no worktree, nothing to mark");
        t.start(n, "feature", "s-1", Some(Worktree { path: "/w".into(), branch: "task/1".into(), ..Default::default() })).unwrap();
        let updated = t.get(n).unwrap().updated;
        assert!(t.mark_opened_in(n, "VS Code").unwrap());
        assert!(!t.mark_opened_in(n, "VS Code").unwrap());
        assert!(t.mark_worktree(n, WtMark::KeptUncommitted).unwrap());
        assert!(t.mark_worktree(n, WtMark::Removed).unwrap());
        let w = t.get(n).unwrap().worktree.clone().unwrap();
        assert!(w.removed.is_some() && !w.kept_uncommitted && w.opened_in.is_none());
        assert!(t.mark_worktree(n, WtMark::Present).unwrap());
        assert_eq!(t.get(n).unwrap().updated, updated);
    }

    #[test]
    fn a_task_is_found_by_its_session_across_projects_and_in_none() {
        let d = dir("find-session");
        let mut loose = Tickets::open(&d, NO_PROJECT_ID);
        assert_eq!(loose.path(), d.join("tasks.json"));
        add(&mut loose, "Loose");
        let mut p = Tickets::open(&d, "p1");
        add(&mut p, "In a project");
        let n = add(&mut p, "Started");
        p.start(n, "feature", "sess-9", None).unwrap();
        assert_eq!(find_session(&d, "sess-9"), Some(("p1".to_string(), 2)));
        assert_eq!(find_session(&d, "nope"), None);
        assert_eq!(find_session(&d, ""), None);
        assert_eq!(every_ticket(&d).len(), 3);
        assert_eq!(task_dir(&d, NO_PROJECT_ID, 2), d.join("tasks/02-work"));
        assert_eq!(plan_file(&d, "p1", 2), d.join("projects/p1/tasks/02-work/plan.md"));
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
        assert!(find_project(&p, "Web").unwrap_err().contains("no project called \"Web\""));
        let e = find_project(&p, "twins").unwrap_err();
        assert!(e.contains("more than one") && e.contains(&a) && e.contains(&b), "{e}");
        assert!(find_project(&p, " ").is_err());
    }

    fn run(d: &Path, args: &[&str], stdin: &str, terminal: bool) -> (i32, String, String) {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = cli(&args, d, None, &mut stdin.as_bytes(), terminal, d, &mut out, &mut err);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    #[test]
    fn the_command_line_adds_and_lists() {
        let d = dir("cli");
        let id = crate::projects::Projects::open(&d).create("Web shop").unwrap();
        fs::create_dir_all(d.join("work")).unwrap();
        assert_eq!(run(&d, &["add", "--project", "web shop", "--title", "Login"], "", true), (0, "#1\n".into(), String::new()));
        let (code, out, _) = run(&d, &["add", "--project", &id, "--title", "Basket", "--folder", "work"], "## Body\nfrom stdin\n", false);
        assert_eq!((code, out.as_str()), (0, "#2\n"));
        let t = Tickets::open(&d, &id);
        let two = t.get(2).unwrap();
        assert_eq!(two.folder, Some(d.join("work").canonicalize().unwrap().to_string_lossy().into_owned()));
        assert_eq!(t.body(two), "## Body\nfrom stdin\n");
        assert_eq!(t.body(t.get(1).unwrap()), TEMPLATE, "nothing piped in: the template");
        assert_eq!(t.get(1).unwrap().folder, Some(d.canonicalize().unwrap().to_string_lossy().into_owned()), "where it was run");
        for (args, says) in [
            (vec!["add", "--project", "Nope", "--title", "x"], "no project called"),
            (vec!["add", "--project", "Web shop", "--title", "x", "--folder", "/no/such/folder"], "does not exist"),
            (vec!["add", "--project", "Web shop"], "title"),
            (vec!["add", "--project", "Web shop", "--title", "x", "--waits-for", "1"], "don't know \"--waits-for\""),
        ] {
            let (code, out, err) = run(&d, &args, "", true);
            assert!(code != 0 && out.is_empty() && err.contains(says) && err.lines().count() == 1, "{args:?}: {err}");
        }
        assert_eq!(run(&d, &["frobnicate"], "", true).0, 2);
        let mut tt = Tickets::open(&d, &id);
        set(&mut tt, 1, State::Done);
        set(&mut tt, 2, State::NeedsYou(Need::Question("Which page?".into())));
        let (code, out, _) = run(&d, &["list", "--project", "Web shop"], "", true);
        assert_eq!(code, 0);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "Web shop");
        assert_eq!(lines[1], "  #2   Basket  ·  Needs you: a question  Which page?", "{out}");
        assert_eq!(lines[2], "  Done and cancelled · 1");
        assert_eq!(lines[3], "  #1   Login  ·  Done");
        let (_, out, _) = run(&d, &["list", "--json"], "", true);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            (v[0]["number"].as_u64(), v[0]["state"].as_str(), v[0]["needs"].as_str()),
            (Some(2), Some("Needs you"), Some("a question"))
        );
        assert_eq!(v[1]["number"], 1, "the Done one last");
        assert!(v[0]["file"].as_str().unwrap().ends_with("/tasks/02-basket.md"));
        // --squad: a squad there is, else refused with the ones there are.
        let (code, out, _) = run(&d, &["add", "--project", "Web shop", "--title", "Fix it", "--squad", "bug-fix"], "", true);
        assert_eq!((code, out.as_str()), (0, "#3\n"));
        assert_eq!(Tickets::open(&d, &id).get(3).unwrap().squad, "bug-fix");
        let (code, out, err) = run(&d, &["add", "--project", "Web shop", "--title", "x", "--squad", "nope"], "", true);
        assert!(code == 1 && out.is_empty() && err.contains("no squad \"nope\"") && err.contains("feature, "), "{err}");
        assert!(Tickets::open(&d, &id).get(4).is_none(), "nothing made");
    }

    #[test]
    fn with_no_project_named_the_task_goes_where_the_folders_sessions_worked_or_in_no_project() {
        let d = dir("where-folder");
        let (claude, cache) = (d.join("claude"), d.join("cache"));
        fs::create_dir_all(claude.join("projects/-x")).unwrap();
        fs::create_dir_all(&cache).unwrap();
        let (shop, docs, free) = (d.join("shop"), d.join("docs"), d.join("free"));
        for f in [&shop, &docs, &free] {
            fs::create_dir_all(f.join("sub")).unwrap();
        }
        let mut projects = crate::projects::Projects::open(&d);
        let web = projects.create("Web shop").unwrap();
        let writing = projects.create("Writing").unwrap();
        projects.assign("s-shop", Some(&web)).unwrap();
        projects.assign("s-docs", Some(&writing)).unwrap();
        projects.assign("s-docs2", Some(&web)).unwrap();
        let entry = |cwd: &Path| json!({"m": 1, "s": 1, "cwd": cwd.to_string_lossy(), "dirs": [], "active": 1});
        let files = json!({
            claude.join("projects/-x/s-shop.jsonl").to_string_lossy(): entry(&shop),
            claude.join("projects/-x/s-docs.jsonl").to_string_lossy(): entry(&docs),
            claude.join("projects/-x/s-docs2.jsonl").to_string_lossy(): entry(&docs),
            claude.join("projects/-x/s-none.jsonl").to_string_lossy(): entry(&free),
        });
        fs::write(cache.join("index.json"), json!({"version": 2, "files": files}).to_string()).unwrap();
        let roots = Roots { claude_root: &claude, cache: &cache };
        let run_in = |cwd: &Path, args: &[&str]| {
            let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let code = cli(&args, &d, Some(roots), &mut "".as_bytes(), true, cwd, &mut out, &mut err);
            (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
        };
        assert_eq!(run_in(&shop.join("sub"), &["add", "--title", "Linter"]), (0, "#1\n".into(), String::new()));
        assert_eq!(Tickets::open(&d, &web).get(1).map(|t| t.title.as_str()), Some("Linter"));
        let (code, _, err) = run_in(&docs, &["add", "--title", "Ambiguous"]);
        assert!(code == 1 && err.contains("sessions of more than one project worked in this folder"), "{err}");
        assert_eq!(run_in(&free, &["add", "--title", "Loose"]), (0, "#1\n".into(), String::new()));
        assert_eq!(
            Tickets::open(&d, NO_PROJECT_ID).get(1).and_then(|t| t.folder.clone()),
            Some(free.canonicalize().unwrap().to_string_lossy().into_owned())
        );
    }

    #[test]
    fn an_archived_project_takes_no_new_tasks_from_the_command_line() {
        let d = dir("archived");
        let mut p = crate::projects::Projects::open(&d);
        let id = p.create("Old").unwrap();
        p.archive(&id, &[]).unwrap();
        let (code, _, err) = run(&d, &["add", "--project", "Old", "--title", "x"], "", true);
        assert!(code == 1 && err.contains("archived"), "{err}");
    }
}
