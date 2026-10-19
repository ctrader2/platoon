//! Keeping a project's notes up to date by itself: the `Stop` hook that notices there is something new, the
//! detached helper that runs one `claude -p` update once the project has been quiet for a few minutes, and the
//! small files the two use to tell each other (and platoon's window) what is going on.
//!
//! How it hangs together:
//! - Claude Code runs `platoon hook stop` whenever a session finishes answering. The hook is instant, silent,
//!   spends no tokens and exits 0 whatever happens. If the session belongs to an active project, it writes the time
//!   into `<cache>/notes-due/<project id>` and starts the helper; a project with no notes file yet gets one from the
//!   template first, so its first update has something to fill in. Any later turn rewrites that time, which is what
//!   makes the timer start over.
//! - The helper (`platoon notes-update <project id>`, run detached, so the platoon window need not be open) takes
//!   an exclusive lock on `<cache>/notes-helper/<project id>.lock`: a second helper for the same project exits at
//!   once, because the one holding the lock will see the newer time anyway. It waits until the mark is about three
//!   minutes old, runs one update, clears the mark and, if a new one arrived meanwhile, waits again.
//! - While a call is out, `<cache>/notes-updating/<project id>` holds the helper's pid, so the notes pane can say
//!   an update is running. While platoon has an editor of its own open on the notes, `<cache>/notes-editing/<...>`
//!   holds platoon's pid and the helper does not write.
//!
//! **It must not be able to feed itself.** The runner's own `claude -p` calls work in `<cache>/ai-work` with
//! `PLATOON_AI=1` in their environment and `--setting-sources ""`, so they load no settings and no hooks at all;
//! on top of that the hook refuses outright when `PLATOON_AI` is set or the session's folder is that work folder,
//! and a session that is in no project is turned away after one small read.

use crate::agents::claude;
use crate::ai::{self, AiError, Request, Runner};
use crate::archive::Archive;
use crate::describe;
use crate::instructions;
use crate::notes;
use crate::notesupdate::{self, Doc, Material, SessionMaterial, State};
use crate::projects::Projects;
use crate::settings::Settings;
use crate::usage;
use serde_json::Value;
use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::{io::AsRawFd, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// The feature id in `ai::FEATURES`: the switch, the gate and the ledger all go by it.
pub const FEATURE: &str = "project_notes";

/// The notes are written on Sonnet whatever the smart features' model is: they go into every session of the project,
/// and on Haiku an update dropped a reversal it had been shown (a feature taken back out stayed in the notes), and
/// condensing kept about twice as many lines that were no longer true.
pub const MODEL: &str = "sonnet";

/// How long a project must be quiet before one update runs, when the settings can't be read at all. The setting
/// itself ("Update notes after", `notes_quiet_minutes`) is what normally decides, and its own default is 3 minutes.
pub const DEFAULT_QUIET_SECS: u64 = 180;

/// A helper never lives longer than this, whatever happens.
pub const DEFAULT_MAX_LIFE_SECS: u64 = 1800;

/// A session platoon has never read before is read in full, once: its first message and up to this many of its
/// latest messages. (Beyond that the whole point of the caps is lost and one project could cost a fortune.)
pub const FULL_READ_MESSAGES: usize = 20;

/// The longest one update may take before it is given up on.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// The longest the (much smaller) description call may take.
const DESCRIBE_TIMEOUT: Duration = Duration::from_secs(60);

/// How much of a transcript is kept in memory while it is read.
const KEEP_RECENT: usize = 64;

/// A line of a transcript longer than this is not a message anyone typed.
const MAX_LINE: u64 = 4 << 20;

// ---- the small files ------------------------------------------------------------------------------------------

fn dir_file(cache: &Path, dir: &str, id: &str) -> PathBuf {
    cache.join(dir).join(id)
}

/// Where the time of the last finished turn of a project's sessions is kept.
pub fn due_path(cache: &Path, id: &str) -> PathBuf {
    dir_file(cache, "notes-due", id)
}

/// Where the helper says it has a call out.
pub fn running_path(cache: &Path, id: &str) -> PathBuf {
    dir_file(cache, "notes-updating", id)
}

/// Where platoon says it has an editor of its own open on these notes.
pub fn editing_path(cache: &Path, id: &str) -> PathBuf {
    dir_file(cache, "notes-editing", id)
}

fn lock_path(cache: &Path, id: &str) -> PathBuf {
    dir_file(cache, "notes-helper", id)
}

pub fn now_ms() -> u64 {
    usage::now() * 1000
        + u64::from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_millis()).unwrap_or(0))
}

/// Note that this project has something new, at `at_ms`. A later turn simply writes a later time: that is the
/// quiet timer starting over.
pub fn mark_due(cache: &Path, id: &str, at_ms: u64) -> io::Result<()> {
    crate::projects::write_atomic(&due_path(cache, id), &at_ms.to_string())
}

/// When this project was last marked, or None when it has nothing waiting.
pub fn due_at(cache: &Path, id: &str) -> Option<u64> {
    fs::read_to_string(due_path(cache, id)).ok()?.trim().parse().ok()
}

/// Forget the mark, but only while it is still the one that was acted on: a turn that happened while the call was
/// out must not be lost.
pub fn clear_due_if(cache: &Path, id: &str, was: u64) -> bool {
    if due_at(cache, id) != Some(was) {
        return false;
    }
    let _ = fs::remove_file(due_path(cache, id));
    true
}

/// Write this process's pid into `path`, to say it is doing something.
pub fn set_marker(path: &Path) {
    let _ = crate::projects::write_atomic(path, &std::process::id().to_string());
}

pub fn clear_marker(path: &Path) {
    let _ = fs::remove_file(path);
}

/// Is the process that wrote this marker still there? A marker left behind by a platoon that was killed says
/// nothing, so it is ignored (and tidied away).
pub fn marker_alive(path: &Path) -> bool {
    let Ok(text) = fs::read_to_string(path) else { return false };
    let Ok(pid) = text.trim().parse::<u32>() else { return false };
    if crate::procs::pid_alive(pid) {
        return true;
    }
    let _ = fs::remove_file(path);
    false
}

/// An update is running for this project right now.
pub fn updating(cache: &Path, id: &str) -> bool {
    marker_alive(&running_path(cache, id))
}

/// platoon has an editor of its own open on these notes, so nothing may write them.
pub fn being_edited(cache: &Path, id: &str) -> bool {
    marker_alive(&editing_path(cache, id))
}

fn env_secs(key: &str, fallback: u64) -> u64 {
    std::env::var(key).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(fallback)
}

/// How long a project must be quiet before an update: the user's "Update notes after" setting, in seconds.
/// `PLATOON_NOTES_QUIET_SECS` always wins, so a test never waits minutes for one.
pub fn quiet_secs(settings: &Settings) -> u64 {
    quiet_secs_with(settings, &|k| std::env::var(k).ok())
}

/// The same with the environment handed in, so a test can try the knob without changing this process's own.
pub fn quiet_secs_with(settings: &Settings, env: &dyn Fn(&str) -> Option<String>) -> u64 {
    match env("PLATOON_NOTES_QUIET_SECS").and_then(|v| v.trim().parse().ok()) {
        Some(secs) => secs,
        None => settings.notes_quiet_minutes.max(1) * 60,
    }
}

fn max_life_secs() -> u64 {
    env_secs("PLATOON_NOTES_MAX_LIFE_SECS", DEFAULT_MAX_LIFE_SECS)
}

// ---- the Stop hook --------------------------------------------------------------------------------------------

/// What the hook does with one hook input: the project it marked, or None for "nothing to do", which costs one
/// small read of `projects.json` and nothing else. It never writes anything but the mark (and, for a project with
/// no notes file yet, the notes from the template: `notes_to_keep`) and never prints.
pub fn stop_mark(input: &str, config: &Path, cache: &Path, ai_child: bool, at_ms: u64) -> Option<String> {
    // The helper's own calls must never come back here. (They load no settings, so Claude Code runs no hook for
    // them at all; these two checks are the belt to that brace.)
    if ai_child {
        return None;
    }
    let v: Value = serde_json::from_str(input).ok()?;
    let session = v.get("session_id")?.as_str()?;
    if !crate::sessionhook::safe_name(session) {
        return None;
    }
    let cwd = v.get("cwd").and_then(Value::as_str).unwrap_or("");
    if !cwd.is_empty() && Path::new(cwd) == ai::work_dir(cache) {
        return None;
    }
    let (settings, corrupt) = crate::settings::load(config);
    if corrupt || !settings.feature_on(FEATURE) {
        return None;
    }
    let projects = Projects::open(config);
    let project = projects.of(session)?;
    if project.archived {
        return None;
    }
    let id = project.id.clone();
    if Archive::open(config).contains(session) {
        return None;
    }
    if !notes_to_keep(config, cache, &id) {
        return None;
    }
    mark_due(cache, &id, at_ms).ok()?;
    Some(id)
}

/// Does this project have notes to keep up to date? Notes with something in them do. A project with no notes file
/// at all gets one from the template now (`notes::start`), and then does - unless an editor platoon started has the
/// notes (it is about to write them). A file that is there but blank was emptied by the user, and is theirs to
/// start again with `e`: platoon leaves it alone. One small read when the notes are there.
fn notes_to_keep(config: &Path, cache: &Path, id: &str) -> bool {
    let path = crate::projects::notes_path(config, id);
    match fs::read_to_string(&path) {
        Ok(text) => return !text.trim().is_empty(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return false,
    }
    if being_edited(cache, id) || notes::start(config, id).is_err() {
        return false;
    }
    // Whoever's file is there now - the template just written, or one that appeared a moment before it - decides.
    fs::read_to_string(&path).is_ok_and(|text| !text.trim().is_empty())
}

/// Start the helper for `id`, detached: its own session, no terminal, nothing waiting for it. It exits at once if
/// another helper already has the project.
pub fn spawn_helper(id: &str) -> io::Result<()> {
    if std::env::var("PLATOON_NOTES_HELPER").is_ok_and(|v| v == "off") {
        return Ok(());
    }
    // A unit test's own program is the test binary: running it as a helper would run the test suite again.
    if cfg!(test) {
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.args(["notes-update", id]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: setsid in the child before exec, so closing the terminal can't hang it up.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn().map(|_| ())
}

/// `platoon hook stop`. Always 0, always silent: a hook that gets in a session's way is worse than no hook.
pub fn run_hook() -> i32 {
    let _ = std::panic::catch_unwind(|| {
        let mut input = String::new();
        if std::io::stdin().take(1 << 20).read_to_string(&mut input).is_err() {
            return;
        }
        let ai_child = std::env::var_os("PLATOON_AI").is_some();
        let (config, cache) = (crate::archive::config_dir(), usage::cache_dir());
        if let Some(id) = stop_mark(&input, &config, &cache, ai_child, now_ms()) {
            let _ = spawn_helper(&id);
        }
    });
    0
}

/// A session has just joined a project - the user pressed `m`, a session platoon started under a project showed
/// up, or the SessionStart hook placed one. Nothing ends a turn in that case, so without this the project would
/// wait until somebody resumed one of its sessions, and a session nobody resumes would never be read at all.
///
/// Everything the `Stop` hook turns away is turned away here too (the switch off, an archived project, an
/// archived session, notes the user emptied), and marking is all it does (with the notes started from the template
/// when there is no file yet): the helper it starts takes the same one-per-project lock and waits out the same
/// quiet period. True when the project was marked.
pub fn mark_joined(config: &Path, cache: &Path, project: &str, session: &str, at_ms: u64) -> bool {
    if Archive::open(config).contains(session) {
        return false;
    }
    mark_project(config, cache, project, at_ms)
}

/// Something new for a project's notes that no session's turn brought (a session joined it, a ticket's squad
/// finished with a result): mark the project due and start the helper, as the `Stop` hook does. Turned away, after
/// small reads only, when the switch is off, the project is archived or gone, or its notes are there but blank. A
/// project with no notes file yet gets one from the template first (`notes_to_keep`). True when the project was marked.
pub fn mark_project(config: &Path, cache: &Path, project: &str, at_ms: u64) -> bool {
    let (settings, corrupt) = crate::settings::load(config);
    if corrupt || !settings.feature_on(FEATURE) {
        return false;
    }
    let projects = Projects::open(config);
    if !projects.get(project).is_some_and(|p| !p.archived) {
        return false;
    }
    if !notes_to_keep(config, cache, project) {
        return false;
    }
    if mark_due(cache, project, at_ms).is_err() {
        return false;
    }
    let _ = spawn_helper(project);
    true
}

// ---- reading what is new in a session ---------------------------------------------------------------------------

/// What one transcript holds, read once: its first message, its latest messages, how many substantive ones there
/// are in all, and Claude's last reply (one line of it, or as much as the notes ask for). Nothing else is kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Read1 {
    pub first: Option<String>,
    pub recent: Vec<String>,
    pub total: usize,
    pub reply: Option<String>,
}

/// Read a transcript for what an update may send: the user's substantive messages (each cut to `cap` characters)
/// and Claude's last reply. Only the last `KEEP_RECENT` messages are held, so a huge conversation costs no more
/// memory than a small one.
pub fn read_session(path: &Path, cap: usize) -> Read1 {
    read_session_with(path, cap, None)
}

/// `read_session`, with Claude's last reply read the way the notes need it: up to `reply_chars` characters, its
/// lines kept (`claude::reply_text_in_line`). None: one line of it, as everything else reads it.
pub fn read_session_with(path: &Path, cap: usize, reply_chars: Option<usize>) -> Read1 {
    let Ok(file) = fs::File::open(path) else { return Read1::default() };
    let mut reader = BufReader::new(file);
    let mut out = Read1::default();
    let mut recent: VecDeque<String> = VecDeque::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        // A line longer than any message anyone types is skipped rather than held (tool output can be enormous).
        match (&mut reader).take(MAX_LINE).read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        if line.last() != Some(&b'\n') && line.len() as u64 == MAX_LINE {
            // The rest of this over-long line is read and thrown away.
            let mut rest = Vec::new();
            if reader.read_until(b'\n', &mut rest).unwrap_or(0) == 0 {
                break;
            }
            continue;
        }
        let Ok(text) = std::str::from_utf8(&line) else { continue };
        if let Some(p) = claude::prompt_in_line(text, cap) {
            if crate::smart::is_substantive(&p) {
                out.total += 1;
                if out.first.is_none() {
                    out.first = Some(p.clone());
                }
                recent.push_back(p);
                if recent.len() > KEEP_RECENT {
                    recent.pop_front();
                }
            }
        } else if claude::may_be_reply(text) {
            let reply = match reply_chars {
                Some(n) => claude::reply_text_in_line(text, n),
                None => claude::reply_in_line(text, claude::REPLY_CHARS),
            };
            if let Some(r) = reply {
                out.reply = Some(r);
            }
        }
    }
    out.recent = recent.into();
    out
}

/// The last part of a session's folder, which is all the prompt says about where it is.
fn folder_of(path: &Path, cwd: Option<String>) -> String {
    let from_cwd = cwd.and_then(|c| Path::new(&c).file_name().map(|n| n.to_string_lossy().into_owned()));
    from_cwd
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
}

/// What is new in this project since the last update, and how much of each session has now been read.
pub fn gather(config: &Path, root: &Path, id: &str, name: &str, state: &State, settings: &Settings) -> (Material, BTreeMap<String, usize>) {
    let projects = Projects::open(config);
    let archive = Archive::open(config);
    let per = settings.ai_prompts_per_session as usize;
    let cap = settings.ai_prompt_chars as usize;
    let mut sessions: Vec<(u64, SessionMaterial)> = Vec::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for sid in projects.members_of(id) {
        if archive.contains(&sid) {
            continue; // archived sessions are skipped entirely
        }
        let Some(path) = claude::find_transcript(root, &sid) else { continue };
        let read = read_session_with(&path, cap, Some(settings.notes_reply_chars as usize));
        if read.total == 0 {
            continue;
        }
        let before = state.seen.get(&sid).copied();
        seen.insert(sid.clone(), read.total);
        let first_read = state.first_read(&sid);
        let messages = match before {
            // Never read before: the whole conversation, once, within a cap of its own.
            None => {
                let mut m: Vec<String> = read.recent.iter().rev().take(FULL_READ_MESSAGES).rev().cloned().collect();
                if let Some(f) = &read.first
                    && !m.first().is_some_and(|x| x == f)
                {
                    m.insert(0, f.clone());
                }
                m
            }
            Some(n) if read.total > n => {
                let fresh = read.total - n;
                read.recent.iter().rev().take(fresh.min(per)).rev().cloned().collect()
            }
            Some(_) => Vec::new(),
        };
        if messages.is_empty() {
            continue; // nothing new in this one
        }
        let modified = fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        sessions.push((
            modified,
            SessionMaterial { folder: folder_of(&path, claude::transcript_cwd(&path)), messages, reply: read.reply, first_read },
        ));
    }
    // The most recently used session first: if anything has to be cut, it is the stalest.
    sessions.sort_by_key(|s| std::cmp::Reverse(s.0));
    (Material { project: name.to_string(), sessions: sessions.into_iter().map(|(_, s)| s).collect() }, seen)
}

// ---- one update -------------------------------------------------------------------------------------------------

/// Everything the helper needs to know about where things are. A test points it at its own folders and its own
/// stand-in for `claude`.
pub struct Env {
    pub config: PathBuf,
    pub cache: PathBuf,
    /// Claude's own folder, where the transcripts are.
    pub root: PathBuf,
    /// The program that runs a call; None means `PLATOON_CLAUDE_BIN`, else `claude`.
    pub program: Option<String>,
}

impl Env {
    pub fn from_env() -> Env {
        Env { config: crate::archive::config_dir(), cache: usage::cache_dir(), root: claude::claude_root(), program: None }
    }

    fn runner(&self) -> Runner {
        match &self.program {
            Some(p) => Runner::with_program(&self.cache, p),
            None => Runner::from_env(&self.cache),
        }
    }
}

/// What one round came to. Nothing here is shown to the user as it is; it is what the helper acts on and what the
/// tests read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Round {
    /// The gate said no (switch off, daily limit, plan busy, back-off): in plain words.
    Refused(String),
    /// platoon has an editor open on these notes: not now.
    Busy,
    /// No project, no notes, or nothing readable.
    NoNotes,
    /// Nothing has been said in this project since the last update: no call, no tokens.
    NothingNew,
    /// The call itself failed.
    Failed(String),
    /// Claude's answer could not be understood, so nothing was changed.
    BadReply,
    /// Asked twice, the answer still left the notes over the cap (`notes::CAP`) and no shorter: nothing changed.
    TooLong,
    /// It ran and the answer came to nothing (the same words back again, or every section refused).
    Unchanged,
    /// The notes were written. (sections rewritten, decisions added)
    Written { sections: usize, decisions: usize },
}

impl Round {
    /// A call really went out and came back understood. Only then is the project's description looked at: that is
    /// what "refreshed right after a notes update" means.
    pub fn ran_a_call(&self) -> bool {
        matches!(self, Round::Unchanged | Round::Written { .. })
    }
}

/// What came of looking at the project's description.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Described {
    /// The gate said no (the switch, the daily limit, the plan, a back-off): in plain words.
    Refused(String),
    /// There is a description and it is less than an hour old.
    NotDue,
    /// No project, or nothing to go on.
    Nothing,
    /// The call itself failed.
    Failed(String),
    /// The answer could not be understood, so nothing was written.
    BadReply,
    /// The description now on file.
    Written(String),
}

/// One update, start to finish, in the caller's own thread. The helper loop calls it; so does every test.
pub fn update_once(env: &Env, id: &str) -> Round {
    let (settings, corrupt) = crate::settings::load(&env.config);
    if corrupt {
        return Round::NoNotes;
    }
    let now = usage::now();
    if let Err(r) = ai::gate(&settings, FEATURE, &env.cache, now) {
        return Round::Refused(r.plain());
    }
    if being_edited(&env.cache, id) {
        return Round::Busy;
    }
    let projects = Projects::open(&env.config);
    let Some(project) = projects.get(id).filter(|p| !p.archived).cloned() else { return Round::NoNotes };
    let notes_path = projects.notes_path(id);
    let Ok(before) = fs::read_to_string(&notes_path) else { return Round::NoNotes };
    if before.trim().is_empty() {
        return Round::NoNotes;
    }
    let mut state = State::load(&env.config, id);
    let (material, seen) = gather(&env.config, &env.root, id, &project.name, &state, &settings);
    if material.is_empty() {
        return Round::NothingNew;
    }
    let first = notesupdate::prompt(&Doc::parse(&before), &state, &material);
    let system = notesupdate::system_with(&instructions::guidance(&env.config, instructions::NOTES));
    let mut text = first.clone();
    // An answer that leaves the notes over the cap is asked for again once, condensed; then the notes stay as they are.
    let mut tries = 0;
    let (merged, outcome) = loop {
        tries += 1;
        let mut req = Request::new(FEATURE, system.clone(), text.clone());
        req.timeout = CALL_TIMEOUT;
        req.model = Some(MODEL.into());
        let running = running_path(&env.cache, id);
        set_marker(&running);
        let answer = env.runner().run(&settings, &req);
        clear_marker(&running);

        let reply = match answer {
            Ok(r) => r.text,
            Err(AiError::Refused(r)) => return Round::Refused(r.plain()),
            Err(e) => return Round::Failed(e.plain()),
        };
        let Some(update) = notesupdate::parse_update(&reply) else { return Round::BadReply };

        // What is on disk now is what counts: the user may have written the notes while the call was out, and then
        // the merge is worked out again against what they wrote (and refused if it no longer fits).
        let _lock = crate::projects::lock(&notes_path);
        let current = fs::read_to_string(&notes_path).unwrap_or_default();
        if current.trim().is_empty() {
            return Round::NoNotes;
        }
        if being_edited(&env.cache, id) {
            return Round::Busy;
        }
        let merged = notesupdate::merge(&current, &state, &update, now);
        if notesupdate::too_long(&current, &merged.text) {
            if tries < 2 {
                text = notesupdate::condense_again(&first, &merged.text);
                continue;
            }
            return Round::TooLong;
        }
        let outcome = if merged.changed {
            // The first time notes over the cap are condensed, the file as it was is kept beside it, once.
            if notes::chars(&current) > notes::CAP {
                let _ = notes::keep_backup_once(&env.config, id, &current);
            }
            if notes::write(&env.config, id, &merged.text).is_err() {
                return Round::Failed("the notes couldn't be written".into());
            }
            Round::Written { sections: merged.wrote.len(), decisions: merged.added.len() }
        } else {
            Round::Unchanged
        };
        break (merged, outcome);
    };

    // The material was read and paid for, so it is not read again; what platoon wrote is remembered, so the next
    // update can tell its own sentences from the user's.
    state.seen.extend(seen);
    // What platoon now calls its own, heading by heading: a heading that was renamed has moved, one that was
    // folded into another is gone, and a line of the user's that the answer had to repeat is not platoon's.
    state.written = merged.mine.clone();
    if merged.changed {
        state.last_update_at = now;
    }
    let _ = state.save(&env.config, id);
    outcome
}

// ---- the project's description ------------------------------------------------------------------------------------

/// What the description is worked out from: the project's notes as they are now, and the name and summary
/// platoon already made for each of its active conversations (newest summary first). No transcript is read and no
/// session id is ever in it.
pub fn describe_material(config: &Path, id: &str, name: &str) -> describe::Material {
    let projects = Projects::open(config);
    let archive = Archive::open(config);
    let summaries = crate::summaries::Summaries::open(config);
    let mut sessions: Vec<(u64, describe::SessionLine)> = Vec::new();
    for sid in projects.members_of(id) {
        if archive.contains(&sid) {
            continue; // archived sessions are skipped entirely
        }
        let Some(e) = summaries.get(&sid) else { continue };
        sessions.push((e.at, describe::SessionLine { name: e.name.clone(), summary: e.summary.clone() }));
    }
    sessions.sort_by_key(|s| std::cmp::Reverse(s.0));
    let notes = fs::read_to_string(projects.notes_path(id)).unwrap_or_default();
    describe::Material { project: name.to_string(), notes, sessions: sessions.into_iter().map(|(_, s)| s).collect() }
}

/// One look at the project's description: at most one an hour, and nothing at all for an archived project or
/// while the switch is off. Run straight after a notes update, in the helper's own thread.
pub fn describe_once(env: &Env, id: &str) -> Described {
    let (settings, corrupt) = crate::settings::load(&env.config);
    if corrupt {
        return Described::Nothing;
    }
    let now = usage::now();
    if let Err(r) = ai::gate(&settings, describe::FEATURE, &env.cache, now) {
        return Described::Refused(r.plain());
    }
    let mut projects = Projects::open(&env.config);
    let Some(project) = projects.get(id).filter(|p| !p.archived).cloned() else { return Described::Nothing };
    if !describe::due(project.description_at, now) {
        return Described::NotDue;
    }
    let material = describe_material(&env.config, id, &project.name);
    if material.is_empty() {
        return Described::Nothing;
    }
    let mut req = Request::new(
        describe::FEATURE,
        describe::system_with(&instructions::guidance(&env.config, instructions::DESCRIPTION)),
        describe::prompt(&material),
    );
    req.timeout = DESCRIBE_TIMEOUT;
    let reply = match env.runner().run(&settings, &req) {
        Ok(r) => r.text,
        Err(AiError::Refused(r)) => return Described::Refused(r.plain()),
        Err(e) => return Described::Failed(e.plain()),
    };
    // A reply that isn't understood writes nothing at all, not even the time: the line that is there stays.
    let Some(text) = describe::parse(&reply) else { return Described::BadReply };
    match projects.set_description(id, &text, now) {
        Ok(()) => Described::Written(text),
        Err(e) => Described::Failed(e.to_string()),
    }
}

// ---- the helper -------------------------------------------------------------------------------------------------

/// How many times the lock file is opened again when the open itself failed, and how long between tries.
const LOCK_OPEN_TRIES: u32 = 5;
const LOCK_OPEN_WAIT: Duration = Duration::from_millis(100);

/// What came of trying to take a project's lock. "Another helper has it" and "the lock file couldn't even be
/// opened" are very different answers: the first means there is nothing to do here, the second means platoon
/// couldn't tell. Treating the second as the first would skip an update without a word.
#[derive(Debug)]
enum Taken {
    /// It is ours until this file is dropped, or this process exits.
    Mine(fs::File),
    /// Another helper has this project.
    Held,
    /// The lock file couldn't be opened at all: no room, no permission, no descriptors left.
    Cant(io::Error),
}

fn try_lock(path: &Path) -> Taken {
    if let Some(dir) = path.parent()
        && let Err(e) = fs::create_dir_all(dir)
    {
        return Taken::Cant(e);
    }
    let mut file = match fs::OpenOptions::new().create(true).truncate(false).write(true).open(path) {
        Ok(f) => f,
        Err(e) => return Taken::Cant(e),
    };
    // SAFETY: an advisory lock on a file this process holds open; it goes when the file is dropped or we exit.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Taken::Held;
    }
    let _ = file.set_len(0);
    let _ = write!(file, "{}", std::process::id());
    let _ = file.flush();
    Taken::Mine(file)
}

/// Take the lock, giving a failed open a few more goes: running out of descriptors is a passing thing, and it
/// must never be mistaken for another helper having the project. "Held" is never tried again - that is what the
/// lock is for.
fn take_lock(path: &Path) -> Taken {
    for _ in 1..LOCK_OPEN_TRIES {
        match try_lock(path) {
            Taken::Cant(_) => thread::sleep(LOCK_OPEN_WAIT),
            answer => return answer,
        }
    }
    try_lock(path)
}

/// `platoon notes-update <project id>`: wait for the project to go quiet, then run one update; again if something
/// new arrived while it ran. One per project, and never alive longer than `max_life_secs`.
pub fn run_helper(args: &[String]) -> i32 {
    let Some(id) = args.first().filter(|a| crate::sessionhook::safe_name(a)) else { return 2 };
    let env = Env::from_env();
    let _lock = match take_lock(&lock_path(&env.cache, id)) {
        Taken::Mine(file) => file,
        // Another helper has this project. It will see the newer mark, so there is nothing to do here.
        Taken::Held => return 0,
        // platoon couldn't tell. Nothing is lost: the mark stays, so the next turn of a session in this project
        // starts a helper again. Said out loud for anyone running this by hand; started by the hook it goes nowhere.
        Taken::Cant(why) => {
            eprintln!("platoon: couldn't open the lock for project {id}: {why}");
            return 1;
        }
    };
    let (settings, corrupt) = crate::settings::load(&env.config);
    let quiet = if corrupt { DEFAULT_QUIET_SECS } else { quiet_secs(&settings) };
    helper_loop(&env, id, quiet * 1000);
    0
}

/// The wait-and-update loop. `quiet_ms` is how long the project must have been quiet (a parameter, so a test runs
/// it in its own thread without touching the environment).
fn helper_loop(env: &Env, id: &str, quiet_ms: u64) {
    let poll = Duration::from_millis((quiet_ms / 4).clamp(20, 1000));
    let deadline = Instant::now() + Duration::from_secs(max_life_secs());
    while Instant::now() < deadline {
        let Some(at) = due_at(&env.cache, id) else { break };
        if now_ms().saturating_sub(at) < quiet_ms {
            thread::sleep(poll);
            continue;
        }
        let round = update_once(env, id);
        // The description follows a notes update, and never more than once an hour (`describe_once` says no).
        if round.ran_a_call() {
            describe_once(env, id);
        }
        // A turn that happened while the call was out keeps the mark, and the timer starts over.
        clear_due_if(&env.cache, id, at);
        match round {
            // Nothing will change within this helper's life: stop rather than ask again and again.
            Round::Refused(_) | Round::NoNotes => break,
            Round::Busy => thread::sleep(poll),
            _ => {}
        }
    }
    clear_marker(&running_path(&env.cache, id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{Ledger, Usage};
    use crate::testkit::{AiFake, iso};
    use serde_json::json;

    struct Rig {
        base: PathBuf,
        env: Env,
        fake: AiFake,
    }

    impl Rig {
        fn new(name: &str) -> Rig {
            let base = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-nrun-{name}-{}", std::process::id())));
            let _ = fs::remove_dir_all(&base);
            let (config, cache, root) = (base.join("config"), base.join("cache"), base.join("claude"));
            for d in [&config, &cache, &root.join("projects/-work")] {
                fs::create_dir_all(d).unwrap();
            }
            let fake = AiFake::new(name);
            let env = Env { config, cache, root, program: Some(fake.script.clone()) };
            let mut s = Settings { tokens_enabled: true, ..Settings::default() };
            s.set_feature(FEATURE, true);
            crate::settings::save(&env.config, &s).unwrap();
            Rig { base, env, fake }
        }

        fn project(&self, notes: &str) -> String {
            let mut p = Projects::open(&self.env.config);
            let id = p.create("Web shop").unwrap();
            fs::create_dir_all(self.env.config.join("projects").join(&id)).unwrap();
            fs::write(p.notes_path(&id), notes).unwrap();
            id
        }

        /// A transcript with these messages from the user and this last reply from Claude.
        fn session(&self, project: &str, sid: &str, messages: &[&str], reply: &str) {
            let mut p = Projects::open(&self.env.config);
            p.assign(sid, Some(project)).unwrap();
            let mut lines: Vec<String> = Vec::new();
            for (n, m) in messages.iter().enumerate() {
                lines.push(json!({"type":"user","cwd":"/home/x/work","timestamp":iso(1_789_000_000_000 + n as u64 * 1000),"message":{"role":"user","content":m}}).to_string());
            }
            lines.push(json!({"type":"assistant","cwd":"/home/x/work","timestamp":iso(1_789_000_100_000u64),"message":{"role":"assistant","content":[{"type":"text","text":reply}]}}).to_string());
            fs::write(self.env.root.join("projects/-work").join(format!("{sid}.jsonl")), lines.join("\n") + "\n").unwrap();
        }

        fn notes(&self, id: &str) -> String {
            fs::read_to_string(Projects::open(&self.env.config).notes_path(id)).unwrap_or_default()
        }

        /// A generated name and summary for a session, as the naming feature would have written them.
        fn summary(&self, sid: &str, name: &str, summary: &str, at: u64) {
            let mut sums = crate::summaries::Summaries::open(&self.env.config);
            sums.set_many(&[(
                sid.to_string(),
                crate::summaries::Entry { name: name.into(), summary: summary.into(), covers: 1, model: "haiku".into(), at },
            )])
            .unwrap();
        }

        fn description(&self, id: &str) -> String {
            Projects::open(&self.env.config).get(id).map(|p| p.description.clone()).unwrap_or_default()
        }

        fn hook(&self, sid: &str) -> Option<String> {
            let input = json!({"session_id": sid, "cwd": "/home/x/work", "hook_event_name": "Stop"}).to_string();
            stop_mark(&input, &self.env.config, &self.env.cache, false, now_ms())
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    /// Notes in the shape platoon wrote before the sections became fluid: an update has to cope with them.
    const NOTES: &str =
        "platoon keeps these.\n\n# Overview\n\nSell socks.\n\n# Requirements\n\n# Status\n\n# Decisions\n\n# Notes\n\nmine\n";

    #[test]
    fn the_hook_marks_a_project_and_costs_nothing_for_anything_else() {
        let r = Rig::new("hook");
        let id = r.project(NOTES);
        r.session(&id, "in-project", &["Add a basket page to the shop"], "Done.");
        assert_eq!(r.hook("in-project"), Some(id.clone()));
        assert!(due_at(&r.env.cache, &id).is_some());
        let _ = fs::remove_file(due_path(&r.env.cache, &id));

        // A session in no project at all.
        assert_eq!(r.hook("loose"), None);
        // Bad input of every shape.
        for bad in ["", "{", "[]", "null", r#"{"cwd":"/x"}"#, r#"{"session_id":5}"#, r#"{"session_id":"../../etc"}"#] {
            assert_eq!(stop_mark(bad, &r.env.config, &r.env.cache, false, 1), None, "{bad}");
        }
        // The helper's own session: by its environment, and by the folder it works in.
        assert_eq!(
            stop_mark(&json!({"session_id":"in-project"}).to_string(), &r.env.config, &r.env.cache, true, 1),
            None,
            "PLATOON_AI is set"
        );
        let work = ai::work_dir(&r.env.cache).display().to_string();
        assert_eq!(
            stop_mark(&json!({"session_id":"in-project","cwd":work}).to_string(), &r.env.config, &r.env.cache, false, 1),
            None,
            "the work folder"
        );
        // An archived session of the project, and an archived project.
        let mut archive = Archive::open(&r.env.config);
        archive.archive("in-project", usage::now()).unwrap();
        assert_eq!(r.hook("in-project"), None, "an archived session");
        archive.restore("in-project", usage::now()).unwrap();
        let mut p = Projects::open(&r.env.config);
        p.archive(&id, &[]).unwrap();
        assert_eq!(r.hook("in-project"), None, "an archived project");
        p.restore(&id).unwrap();
        // No notes file: they are started from the template, and the project is marked.
        let notes = Projects::open(&r.env.config).notes_path(&id);
        fs::remove_file(&notes).unwrap();
        assert_eq!(r.hook("in-project"), Some(id.clone()), "no notes file yet");
        assert_eq!(fs::read_to_string(&notes).unwrap(), notes::TEMPLATE);
        let _ = fs::remove_file(due_path(&r.env.cache, &id));
        // Notes the user emptied are theirs: left as they are, and nothing is marked.
        fs::write(&notes, "  \n\n").unwrap();
        assert_eq!(r.hook("in-project"), None, "empty notes");
        assert_eq!(fs::read_to_string(&notes).unwrap(), "  \n\n");
        fs::write(&notes, NOTES).unwrap();
        // The switch off costs nothing either.
        let mut s = crate::settings::load(&r.env.config).0;
        s.set_feature(FEATURE, false);
        crate::settings::save(&r.env.config, &s).unwrap();
        assert_eq!(r.hook("in-project"), None, "the feature is switched off");
        s.set_feature(FEATURE, true);
        s.tokens_enabled = false;
        crate::settings::save(&r.env.config, &s).unwrap();
        assert_eq!(r.hook("in-project"), None, "the master switch is off");
        // Nothing of all that wrote a mark.
        assert_eq!(due_at(&r.env.cache, &id), None);
        assert_eq!(r.fake.calls(), 0, "the hook never asks Claude anything");
    }

    /// Every file under `dir`, relative to it.
    fn files_under(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut todo = vec![dir.to_path_buf()];
        while let Some(d) = todo.pop() {
            for e in fs::read_dir(&d).into_iter().flatten().flatten() {
                if e.path().is_dir() {
                    todo.push(e.path());
                } else {
                    out.push(e.path().strip_prefix(dir).unwrap().display().to_string());
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn a_project_with_no_notes_file_gets_the_template_and_is_marked_and_nothing_else_is_written() {
        let r = Rig::new("hook-start");
        let id = Projects::open(&r.env.config).create("Web shop").unwrap();
        r.session(&id, "s1", &["Add a basket page to the shop"], "Done.");
        let notes = Projects::open(&r.env.config).notes_path(&id);
        assert!(!notes.exists() && !notes.parent().unwrap().exists(), "not even the project's folder is there yet");
        let before = files_under(&r.base);
        assert_eq!(r.hook("s1"), Some(id.clone()));
        assert_eq!(fs::read_to_string(&notes).unwrap(), notes::TEMPLATE, "the same template `e` starts from");
        assert!(due_at(&r.env.cache, &id).is_some(), "and the project is marked, as one with notes is");
        // The notes and the mark, and not a byte more: no temp file left beside them, no earlier version kept.
        let mut wrote: Vec<String> = files_under(&r.base).into_iter().filter(|f| !before.contains(f)).collect();
        wrote.sort();
        let mut expected = vec![format!("config/projects/{id}/project.md"), format!("cache/notes-due/{id}")];
        expected.sort();
        assert_eq!(wrote, expected);
        assert!(crate::notes::versions(&r.env.config, &id).is_empty());
        // A session joining a project with no notes file starts them in the same way.
        let other = Projects::open(&r.env.config).create("Docs").unwrap();
        Projects::open(&r.env.config).assign("s2", Some(&other)).unwrap();
        assert!(mark_joined(&r.env.config, &r.env.cache, &other, "s2", 5));
        assert_eq!(fs::read_to_string(Projects::open(&r.env.config).notes_path(&other)).unwrap(), notes::TEMPLATE);
        assert_eq!(due_at(&r.env.cache, &other), Some(5));
        assert_eq!(r.fake.calls(), 0, "the hook never asks Claude anything");
    }

    #[test]
    fn notes_are_never_started_where_the_project_would_be_turned_away_or_over_a_file_that_is_there() {
        let r = Rig::new("hook-no-start");
        let id = Projects::open(&r.env.config).create("Web shop").unwrap();
        r.session(&id, "s1", &["Add a basket page to the shop"], "Done.");
        let notes = Projects::open(&r.env.config).notes_path(&id);
        let no_notes = |why: &str| {
            assert!(!notes.exists(), "{why}: no notes were started");
            assert_eq!(due_at(&r.env.cache, &id), None, "{why}: nothing was marked");
        };
        // A session in no project, the helper's own calls, its work folder.
        assert_eq!(r.hook("loose"), None);
        no_notes("a session in no project");
        assert_eq!(stop_mark(&json!({"session_id":"s1"}).to_string(), &r.env.config, &r.env.cache, true, 1), None);
        no_notes("PLATOON_AI is set");
        let work = ai::work_dir(&r.env.cache).display().to_string();
        assert_eq!(stop_mark(&json!({"session_id":"s1","cwd":work}).to_string(), &r.env.config, &r.env.cache, false, 1), None);
        no_notes("the work folder");
        // An archived session, by the hook and by joining.
        let mut archive = Archive::open(&r.env.config);
        archive.archive("s1", usage::now()).unwrap();
        assert_eq!(r.hook("s1"), None);
        assert!(!mark_joined(&r.env.config, &r.env.cache, &id, "s1", 1));
        no_notes("an archived session");
        archive.restore("s1", usage::now()).unwrap();
        // An archived project.
        let mut p = Projects::open(&r.env.config);
        p.archive(&id, &[]).unwrap();
        assert_eq!(r.hook("s1"), None);
        assert!(!mark_project(&r.env.config, &r.env.cache, &id, 1));
        no_notes("an archived project");
        p.restore(&id).unwrap();
        // The notes switch off, and the master switch off.
        let mut s = crate::settings::load(&r.env.config).0;
        s.set_feature(FEATURE, false);
        crate::settings::save(&r.env.config, &s).unwrap();
        assert_eq!(r.hook("s1"), None);
        assert!(!mark_project(&r.env.config, &r.env.cache, &id, 1));
        no_notes("the feature is switched off");
        s.set_feature(FEATURE, true);
        s.tokens_enabled = false;
        crate::settings::save(&r.env.config, &s).unwrap();
        assert_eq!(r.hook("s1"), None);
        no_notes("the master switch is off");
        s.tokens_enabled = true;
        crate::settings::save(&r.env.config, &s).unwrap();
        // An editor platoon started has the notes (this process is alive): they are not started under it.
        set_marker(&editing_path(&r.env.cache, &id));
        assert_eq!(r.hook("s1"), None);
        no_notes("an editor of platoon's has the notes");
        clear_marker(&editing_path(&r.env.cache, &id));
        // A file that is there is never replaced, however it got there.
        fs::create_dir_all(notes.parent().unwrap()).unwrap();
        fs::write(&notes, "# Mine\n\nWritten a moment ago.\n").unwrap();
        assert!(!notes::start(&r.env.config, &id).unwrap(), "there is a file: nothing is written");
        assert_eq!(r.hook("s1"), Some(id.clone()));
        assert_eq!(fs::read_to_string(&notes).unwrap(), "# Mine\n\nWritten a moment ago.\n");
        assert_eq!(r.fake.calls(), 0);
    }

    #[test]
    fn the_first_update_of_notes_started_from_the_template_reads_every_session_in_full() {
        let r = Rig::new("first-from-template");
        let id = Projects::open(&r.env.config).create("Web shop").unwrap();
        r.session(&id, "s1", &["Add a basket page to the shop", "And make the totals add up"], "The totals add up now.");
        r.session(&id, "s2", &["Write the shipping page"], "The shipping page is written.");
        assert_eq!(r.hook("s1"), Some(id.clone()));
        assert_eq!(r.notes(&id), notes::TEMPLATE);
        r.fake.reply(
            1,
            r#"{"summary":"A shop that sells socks.","sections":[{"heading":"Open questions","body":"- Free shipping?"}],"decisions":["Baskets are session based"]}"#,
        );
        assert_eq!(update_once(&r.env, &id), Round::Written { sections: 2, decisions: 1 });
        let text = r.notes(&id);
        assert!(text.starts_with("# Summary\n\nA shop that sells socks."), "{text}");
        assert!(text.contains("# Open questions\n\n- Free shipping?"), "{text}");
        assert!(text.contains(": Baskets are session based"), "{text}");
        // Both sessions, every message of them, in the one call.
        let sent = r.fake.stdin(1);
        for m in ["Add a basket page to the shop", "And make the totals add up", "Write the shipping page"] {
            assert!(sent.contains(m), "{m} missing from: {sent}");
        }
        // The template is the version kept before it, so `u` takes the notes back to it.
        assert_eq!(crate::notes::last_change(&r.env.config, &id).map(|(b, _)| b), Some(notes::TEMPLATE.to_string()));
    }

    #[test]
    fn how_long_a_project_must_be_quiet_is_the_users_setting_and_the_test_knob_still_wins() {
        let r = Rig::new("quiet-setting");
        let mut s = crate::settings::load(&r.env.config).0;
        assert_eq!(s.notes_quiet_minutes, 3, "three minutes unless the user says otherwise");
        assert_eq!(quiet_secs(&s), 180);
        for (minutes, secs) in [(1, 60), (5, 300), (15, 900), (30, 1800)] {
            s.notes_quiet_minutes = minutes;
            crate::settings::save(&r.env.config, &s).unwrap();
            let on_disk = crate::settings::load(&r.env.config).0;
            assert_eq!((on_disk.notes_quiet_minutes, quiet_secs(&on_disk)), (minutes, secs));
        }
        // Nonsense in the file can never mean "no wait at all".
        s.notes_quiet_minutes = 0;
        assert_eq!(quiet_secs(&s), 60);
        // The knob tests use wins over any setting, so no test ever waits minutes. (Handed in rather than set on
        // this process: other threads here are busy forking stand-in claudes.)
        let knob = |v: &'static str| move |k: &str| (k == "PLATOON_NOTES_QUIET_SECS").then(|| v.to_string());
        let half_hour = Settings { notes_quiet_minutes: 30, ..Settings::default() };
        assert_eq!(quiet_secs_with(&half_hour, &knob("0")), 0, "the test knob wins");
        assert_eq!(quiet_secs_with(&half_hour, &knob("2")), 2);
        assert_eq!(quiet_secs_with(&half_hour, &|_| None), 1800, "without it, the setting decides");
    }

    #[test]
    fn a_later_turn_starts_the_quiet_timer_over() {
        let r = Rig::new("timer");
        let id = r.project(NOTES);
        mark_due(&r.env.cache, &id, 1000).unwrap();
        assert_eq!(due_at(&r.env.cache, &id), Some(1000));
        mark_due(&r.env.cache, &id, 5000).unwrap();
        assert_eq!(due_at(&r.env.cache, &id), Some(5000), "the newer turn wins");
        // Acting on the older mark must not throw the newer one away.
        assert!(!clear_due_if(&r.env.cache, &id, 1000));
        assert_eq!(due_at(&r.env.cache, &id), Some(5000));
        assert!(clear_due_if(&r.env.cache, &id, 5000));
        assert_eq!(due_at(&r.env.cache, &id), None);
    }

    /// Take a lock that nothing should be holding, waiting briefly for it to come free and insisting all the
    /// while that it reads as "held" and never as "couldn't tell". The file is handed back, so the caller decides
    /// when it is let go.
    ///
    /// Why a wait at all: an `flock` belongs to the open file *description*, not to the process or to one
    /// descriptor, and it is only released once EVERY descriptor for that description is closed - and `fork`
    /// duplicates all of them. So while any other thread of this test binary sits between `fork` and `exec` (the
    /// editor, host and terminal tests start processes all the time), a child holds a copy of the descriptor this
    /// test has just closed, and the lock stays held for that instant, until close-on-exec tidies the copy away.
    /// Measured here at 5 in 400 with six threads spawning hard, which is why it only ever showed up under a full
    /// suite. It is this test process that is unusual, not the code: a helper is a fresh, single-threaded process
    /// that forks nothing before it takes its lock, so nothing of its own can be holding a copy.
    fn take_when_free(path: &Path) -> Option<fs::File> {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            match try_lock(path) {
                Taken::Mine(file) => return Some(file),
                Taken::Held => thread::sleep(Duration::from_millis(5)),
                // Never quietly treated as "somebody has it": that is the whole point of telling them apart.
                Taken::Cant(e) => panic!("the lock file couldn't be opened at all: {e}"),
            }
        }
        None
    }

    #[test]
    fn only_one_helper_per_project_may_run() {
        let r = Rig::new("lock");
        let id = r.project(NOTES);
        let path = lock_path(&r.env.cache, &id);
        let Taken::Mine(first) = try_lock(&path) else { panic!("the first helper takes it") };
        assert_eq!(fs::read_to_string(&path).unwrap(), std::process::id().to_string(), "the holder writes its pid");
        // A second helper for the same project is turned away, and is told the project is taken - not that
        // something went wrong. (Two descriptors for one file conflict even inside one process.)
        assert!(matches!(try_lock(&path), Taken::Held));
        // Another project is free.
        assert!(matches!(try_lock(&lock_path(&r.env.cache, "other")), Taken::Mine(_)));
        drop(first);
        assert!(take_when_free(&path).is_some(), "released when the holder is dropped");
    }

    #[test]
    fn taking_and_releasing_the_lock_survives_other_threads_starting_processes() {
        // The regression test for a real flake: `fork` duplicates every open descriptor, and an `flock` is only
        // released once all of them are closed, so a lock this process has just dropped can look held for as long
        // as some other thread's child takes to reach `exec`. Taking the lock straight after letting it go, with
        // no wait, failed at the first round nearly every time here (and 320 times in 400 in the measurement this
        // test came from). With the wait it must not fail at all.
        let r = Rig::new("lock-forks");
        let path = lock_path(&r.env.cache, "p1");
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let spawners: Vec<_> = (0..2)
            .map(|_| {
                let stop = stop.clone();
                thread::spawn(move || {
                    while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                        // Ours, started here and waited for here; nothing else is ever signalled.
                        let _ = Command::new("true").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
                    }
                })
            })
            .collect();
        for n in 0..60 {
            let held = take_when_free(&path);
            assert!(held.is_some(), "round {n}: the lock never came free");
            drop(held);
        }
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        for t in spawners {
            t.join().unwrap();
        }
    }

    #[test]
    fn a_lock_file_that_cannot_be_opened_is_not_another_helper_holding_it() {
        // Saying "held" here would skip the update without a word, so the two answers are kept apart. The folder
        // the lock would live in is a file, so it can never be made.
        let r = Rig::new("lock-cant");
        let blocked = r.env.cache.join("in-the-way");
        fs::write(&blocked, "not a folder").unwrap();
        let path = blocked.join("p1");
        assert!(matches!(try_lock(&path), Taken::Cant(_)));
        // The retrying wrapper gives the same answer, and takes a free lock straight away.
        assert!(matches!(take_lock(&path), Taken::Cant(_)));
        assert!(matches!(take_lock(&lock_path(&r.env.cache, "fresh")), Taken::Mine(_)));
    }

    #[test]
    fn only_what_is_new_is_sent_and_a_session_added_late_is_read_in_full_once() {
        let r = Rig::new("new");
        let id = r.project(NOTES);
        let many: Vec<String> = (1..=30).map(|n| format!("This is message number {n} of the shop work")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        r.session(&id, "s1", &refs, "I did all of that.");
        let settings = crate::settings::load(&r.env.config).0;
        // Never read before: the first message and the latest ones, capped.
        let (m, seen) = gather(&r.env.config, &r.env.root, &id, "Web shop", &State::default(), &settings);
        assert_eq!(seen.get("s1"), Some(&30));
        let sent = &m.sessions[0];
        assert!(sent.first_read);
        assert_eq!(sent.messages.len(), FULL_READ_MESSAGES + 1, "the first message and the last twenty");
        assert_eq!(sent.messages[0], many[0]);
        assert_eq!(sent.messages.last().unwrap(), &many[29]);
        assert_eq!(sent.reply.as_deref(), Some("I did all of that."));
        assert_eq!(sent.folder, "work");
        // Read before: only what came after, under the per-session cap.
        let mut state = State::default();
        state.seen.insert("s1".into(), 28);
        let (m, _) = gather(&r.env.config, &r.env.root, &id, "Web shop", &state, &settings);
        assert!(!m.sessions[0].first_read);
        assert_eq!(m.sessions[0].messages, [many[28].clone(), many[29].clone()]);
        // Nothing new at all.
        state.seen.insert("s1".into(), 30);
        let (m, _) = gather(&r.env.config, &r.env.root, &id, "Web shop", &state, &settings);
        assert!(m.is_empty() && m.sessions.is_empty());
        // Filler ("yes", "ok") is not new material.
        r.session(&id, "s2", &["ok", "yes please", "thanks"], "Right.");
        let (m, seen) = gather(&r.env.config, &r.env.root, &id, "Web shop", &state, &settings);
        assert!(m.sessions.is_empty() && !seen.contains_key("s2"), "{m:?}");
        // An archived session is skipped entirely.
        Archive::open(&r.env.config).archive("s1", usage::now()).unwrap();
        let (m, seen) = gather(&r.env.config, &r.env.root, &id, "Web shop", &State::default(), &settings);
        assert!(m.sessions.is_empty() && seen.is_empty());
    }

    #[test]
    fn the_notes_read_claudes_last_reply_up_to_the_setting_with_its_lines_kept() {
        let r = Rig::new("reply");
        let id = r.project(NOTES);
        let mut reply = String::from("Done.\n\nStill open:\n- \u{1b}[1mWhich card provider?\u{1b}[0m\n- Does VAT go in the total?\n\n");
        for n in 0..400 {
            reply.push_str(&format!("Detail {n} of a long answer.\n"));
        }
        r.session(&id, "s1", &["Add a basket page to the shop"], &reply);
        let mut settings = crate::settings::load(&r.env.config).0;
        assert_eq!(settings.notes_reply_chars, 1500, "about 1500 characters unless the user says otherwise");
        let (m, _) = gather(&r.env.config, &r.env.root, &id, "Web shop", &State::default(), &settings);
        let got = m.sessions[0].reply.clone().unwrap();
        assert!(got.starts_with("Done.\n\nStill open:\n- Which card provider?\n- Does VAT go in the total?"), "{got}");
        assert_eq!(got.chars().count(), 1501, "cut at the setting, with a mark where");
        assert!(!got.contains('\u{1b}'));
        // The prompt carries all of it, set off from the rest so a line of it can't pass for a heading of the notes.
        let p = notesupdate::prompt(&Doc::parse(NOTES), &State::default(), &m);
        assert!(p.contains("Claude's last reply:\n  Done.\n\n  Still open:\n  - Which card provider?"), "{p}");
        for chars in [500u64, 3000] {
            settings.notes_reply_chars = chars;
            let (m, _) = gather(&r.env.config, &r.env.root, &id, "Web shop", &State::default(), &settings);
            assert_eq!(m.sessions[0].reply.as_ref().unwrap().chars().count() as u64, chars + 1);
        }
        // Everything else that reads a session still gets one line of the reply.
        let path = claude::find_transcript(&r.env.root, "s1").unwrap();
        let one = read_session(&path, 300).reply.unwrap();
        assert_eq!((one.lines().count(), one.chars().count()), (1, claude::REPLY_CHARS));
    }

    #[test]
    fn an_update_writes_the_notes_keeps_a_version_and_records_what_it_spent() {
        let r = Rig::new("write");
        let id = r.project(NOTES);
        r.session(&id, "s1", &["Add a basket page to the shop"], "I added the basket page.");
        // platoon has written nothing here yet, so every line of Overview is the user's: it may add, not reword.
        r.fake.reply_default(
            r#"{"summary":"A shop that sells socks.","sections":[{"heading":"Overview","body":"Sell socks.\nThere is a basket page."}],"decisions":["Baskets are session based"]}"#,
        );
        r.fake.usage(900, 120);
        assert_eq!(update_once(&r.env, &id), Round::Written { sections: 2, decisions: 1 });
        let text = r.notes(&id);
        assert!(text.contains("Sell socks.\nThere is a basket page."), "{text}");
        assert!(
            text.starts_with("platoon keeps these.\n\n# Summary\n\nA shop that sells socks."),
            "the summary goes in at the top: {text}"
        );
        assert!(text.contains(&format!("- {}: Baskets are session based", notesupdate::day(usage::now()))), "{text}");
        assert!(text.contains("# Notes\n\nmine"), "Notes is untouched: {text}");
        // The version before it is kept, so `c` and `u` work on it.
        assert_eq!(crate::notes::last_change(&r.env.config, &id).map(|(b, _)| b), Some(NOTES.to_string()));
        // The spend is in the ledger under this feature.
        let day = Ledger::open(&r.env.cache).today(usage::now());
        assert_eq!(day.features.get(FEATURE).map(|f| (f.tokens, f.calls)), Some((1020, 1)));
        assert_eq!(day.usage, Usage { input: 900, output: 120, ..Usage::default() });
        // What was sent: one call, in the work folder, with the project's name and no session id.
        assert_eq!(r.fake.calls(), 1);
        let sent = r.fake.stdin(1);
        assert!(sent.contains("Project: Web shop") && sent.contains("Add a basket page"), "{sent}");
        assert!(!sent.contains("s1"), "{sent}");
        assert_eq!(r.fake.cwd(1), ai::work_dir(&r.env.cache).display().to_string());
        // And it is not read again: a second round has nothing new.
        assert_eq!(update_once(&r.env, &id), Round::NothingNew);
        assert_eq!(r.fake.calls(), 1);
    }

    #[test]
    fn the_second_update_may_reword_what_platoon_wrote_but_not_what_the_user_wrote() {
        let r = Rig::new("reword");
        let id = r.project(NOTES);
        r.session(&id, "s1", &["Add a basket page to the shop"], "Done.");
        r.fake.reply(1, r#"{"sections":[{"heading":"Overview","body":"Sell socks.\nThere is a basket page."}]}"#);
        assert!(matches!(update_once(&r.env, &id), Round::Written { .. }));
        // Only platoon's own line is recorded as platoon's: "Sell socks." was the user's and stays theirs.
        assert_eq!(State::load(&r.env.config, &id).mine("Overview"), "There is a basket page.");
        // The user adds a line of their own.
        let path = Projects::open(&r.env.config).notes_path(&id);
        let text = fs::read_to_string(&path).unwrap().replace("There is a basket page.", "There is a basket page.\nWe never sell hats.");
        fs::write(&path, &text).unwrap();
        r.session(&id, "s1", &["Add a basket page to the shop", "Now add the checkout page as well"], "Done.");
        // An answer that drops the user's line changes nothing.
        r.fake.reply(2, r#"{"sections":[{"heading":"Overview","body":"Sell socks.\nThere is a basket and a checkout page."}]}"#);
        assert_eq!(update_once(&r.env, &id), Round::Unchanged);
        assert_eq!(fs::read_to_string(&path).unwrap(), text, "nothing written");
        // An answer that keeps it goes in, with platoon's own line reworded around it.
        r.session(&id, "s1", &["Add a basket page to the shop", "Now add the checkout page as well", "And a receipt page too"], "Done.");
        r.fake.reply(
            3,
            r#"{"sections":[{"heading":"Overview","body":"Sell socks.\nThere is a basket and a checkout page.\nWe never sell hats."}]}"#,
        );
        assert!(matches!(update_once(&r.env, &id), Round::Written { .. }));
        let after = fs::read_to_string(&path).unwrap();
        assert!(after.contains("There is a basket and a checkout page.") && after.contains("We never sell hats."), "{after}");
    }

    /// Notes over the cap: 25 sections platoon wrote (400 characters each) and 30 dated decisions.
    fn long_project(r: &Rig) -> (String, String) {
        let mut text = String::from("# Summary\n\nA shop.\n\n");
        let mut state = State::default();
        for n in 1..=25 {
            let body = format!("- {}", "w".repeat(400));
            text.push_str(&format!("# Part {n}\n\n{body}\n\n"));
            state.set_mine(&format!("Part {n}"), &body);
        }
        text.push_str("# Decisions\n\n");
        for n in 1..=30 {
            text.push_str(&format!("- 2026-09-{:02}: Decision number {n} about the shop.\n", n % 28 + 1));
        }
        assert!(notes::chars(&text) > notes::CAP);
        let id = r.project(&text);
        state.save(&r.env.config, &id).unwrap();
        r.session(&id, "s1", &["Add a basket page to the shop"], "Done.");
        (id, text)
    }

    /// An answer that folds every part into one and condenses the decisions.
    fn condensed() -> String {
        let was: Vec<String> = (1..=25).map(|n| format!("Part {n}")).collect();
        json!({
            "sections": [{"heading": "History", "body": "- Twenty-five parts, condensed.", "was": was}],
            "decisions_condensed": "- 2026-09: Thirty decisions about the shop.",
        })
        .to_string()
    }

    #[test]
    fn an_answer_over_the_cap_is_asked_for_again_once_condensed_and_the_old_notes_are_kept_once() {
        let r = Rig::new("cap-retry");
        let (id, before) = long_project(&r);
        // The first answer only adds: still over and longer. The second condenses.
        r.fake.reply(1, r#"{"decisions":["Baskets are session based"]}"#);
        r.fake.reply(2, &condensed());
        assert!(matches!(update_once(&r.env, &id), Round::Written { .. }));
        assert_eq!(r.fake.calls(), 2);
        assert!(r.fake.has_arg(1, "sonnet") && r.fake.has_arg(2, "sonnet"), "notes are written on Sonnet: {:?}", r.fake.argv(1));
        assert!(!r.fake.has_arg(1, "haiku"), "not the smart features' model");
        let again = r.fake.stdin(2);
        assert!(again.contains("Your last answer would have left the notes at") && again.contains("over the 8,000"), "{again}");
        assert!(!r.fake.stdin(1).contains("Your last answer"));
        let after = r.notes(&id);
        assert!(notes::chars(&after) <= notes::CAP, "{after}");
        assert!(after.contains("# History\n\n- Twenty-five parts, condensed.") && after.contains("- 2026-09: Thirty decisions"), "{after}");
        assert!(!after.contains("# Part 1\n"));
        // The notes as they were, kept beside them, once.
        assert_eq!(fs::read_to_string(notes::backup_path(&r.env.config, &id)).unwrap(), before);
        // The user writes them long again; the next condense leaves the first copy alone.
        let path = Projects::open(&r.env.config).notes_path(&id);
        fs::write(&path, format!("{after}\n# Mine\n\n{}\n", "m".repeat(notes::CAP))).unwrap();
        r.session(&id, "s1", &["Add a basket page to the shop", "And a checkout"], "Done.");
        r.fake.reply(3, r#"{"sections":[{"heading":"History","body":"- Parts."}]}"#);
        let round = update_once(&r.env, &id);
        assert_eq!(round, Round::Written { sections: 1, decisions: 0 }, "over but shorter: on the way down; calls {}", r.fake.calls());
        assert!(notes::chars(&r.notes(&id)) > notes::CAP);
        assert_eq!(fs::read_to_string(notes::backup_path(&r.env.config, &id)).unwrap(), before, "never replaced");
    }

    #[test]
    fn an_answer_still_over_the_cap_after_the_retry_changes_nothing() {
        let r = Rig::new("cap-twice");
        let (id, before) = long_project(&r);
        r.fake.reply_default(r#"{"decisions":["Baskets are session based"]}"#);
        assert_eq!(update_once(&r.env, &id), Round::TooLong);
        assert_eq!(r.fake.calls(), 2, "one retry, no more");
        assert_eq!(r.notes(&id), before);
        assert!(!notes::backup_path(&r.env.config, &id).exists());
        // Not counted as read: the next update sends it again.
        assert!(State::load(&r.env.config, &id).seen.is_empty());
        // An answer that pushes notes under the cap over it is refused the same way.
        let r = Rig::new("cap-push");
        let id = r.project(NOTES);
        r.session(&id, "s1", &["Add a basket page to the shop"], "Done.");
        let huge =
            json!({"sections": (0..3).map(|n| json!({"heading": format!("Big {n}"), "body": "z".repeat(3_900)})).collect::<Vec<_>>()});
        r.fake.reply_default(&huge.to_string());
        assert_eq!(update_once(&r.env, &id), Round::TooLong);
        assert_eq!(r.notes(&id), NOTES);
    }

    #[test]
    fn a_reply_that_makes_no_sense_changes_nothing() {
        let r = Rig::new("bad");
        let id = r.project(NOTES);
        r.session(&id, "s1", &["Add a basket page to the shop"], "Done.");
        for (n, bad) in ["I am afraid I cannot help with that.", "{}", "[]", r#"{"summary":""}"#, r#"{"sections":[{"heading":"Status"}]}"#]
            .iter()
            .enumerate()
        {
            r.fake.reply(n + 1, bad);
            assert_eq!(update_once(&r.env, &id), Round::BadReply, "{bad}");
            assert_eq!(r.notes(&id), NOTES, "{bad}");
            assert!(crate::notes::versions(&r.env.config, &id).is_empty(), "no copy kept either");
            // The material is not counted as read, so the next try sends it again.
            assert_eq!(State::load(&r.env.config, &id), State::default());
        }
        // A call that fails outright leaves the notes alone too.
        r.fake.fail(Some("something went wrong"));
        assert!(matches!(update_once(&r.env, &id), Round::Failed(_)));
        assert_eq!(r.notes(&id), NOTES);
    }

    #[test]
    fn the_notes_as_they_are_on_disk_win_over_what_the_call_started_from() {
        let r = Rig::new("disk");
        let id = r.project(NOTES);
        r.session(&id, "s1", &["Add a basket page to the shop"], "Done.");
        let path = Projects::open(&r.env.config).notes_path(&id);
        // The fake rewrites the notes while its call is out: exactly what a user typing in an editor does.
        let rewritten = NOTES.replace("Sell socks.", "Sell socks.\nWritten by hand while the call was out.");
        fs::write(r.fake.dir.join("reply.1"), r#"{"sections":[{"heading":"Overview","body":"Sell socks.\nThere is a basket page."}]}"#)
            .unwrap();
        fs::write(&path, &rewritten).unwrap();
        assert_eq!(update_once(&r.env, &id), Round::Unchanged, "the answer no longer fits what is on disk");
        assert_eq!(fs::read_to_string(&path).unwrap(), rewritten, "what the person wrote is still there");
        // An editor platoon started stops an update before it begins.
        fs::write(&path, NOTES).unwrap();
        set_marker(&editing_path(&r.env.cache, &id));
        assert_eq!(update_once(&r.env, &id), Round::Busy);
        clear_marker(&editing_path(&r.env.cache, &id));
        // A marker left behind by a platoon that is gone says nothing.
        crate::projects::write_atomic(&editing_path(&r.env.cache, &id), "999999999").unwrap();
        assert!(!being_edited(&r.env.cache, &id));
    }

    #[test]
    fn the_gate_is_obeyed_before_anything_is_read_or_sent() {
        let r = Rig::new("gate");
        let id = r.project(NOTES);
        r.session(&id, "s1", &["Add a basket page to the shop"], "Done.");
        let mut s = crate::settings::load(&r.env.config).0;
        s.set_feature(FEATURE, false);
        crate::settings::save(&r.env.config, &s).unwrap();
        let refused = update_once(&r.env, &id);
        assert!(matches!(&refused, Round::Refused(why) if why.contains("Project notes")), "{refused:?}");
        s.set_feature(FEATURE, true);
        s.tokens_enabled = false;
        crate::settings::save(&r.env.config, &s).unwrap();
        assert!(matches!(update_once(&r.env, &id), Round::Refused(_)));
        // The day's limit spent.
        s.tokens_enabled = true;
        crate::settings::save(&r.env.config, &s).unwrap();
        let spent = Usage { input: s.clone().clamped().ai_daily_tokens + 1, ..Usage::default() };
        Ledger::open(&r.env.cache).record_call(usage::now(), FEATURE, &spent, 0.0, true).unwrap();
        let refused = update_once(&r.env, &id);
        assert!(matches!(&refused, Round::Refused(why) if why.contains("limit")), "{refused:?}");
        assert_eq!(r.fake.calls(), 0, "nothing was ever asked");
        assert_eq!(r.notes(&id), NOTES);
    }

    #[test]
    fn the_update_and_the_description_send_the_users_own_guidance_then_the_fixed_part() {
        let r = Rig::new("own");
        let id = r.project(NOTES);
        r.session(&id, "s1", &["Add a basket page to the shop"], "Done.");
        r.fake.reply(1, r#"{"sections":[{"heading":"Status","body":"The basket page is in."}]}"#);
        r.fake.reply(2, r#"{"description":"A sock shop with a basket."}"#);
        fs::create_dir_all(r.env.config.join("prompts")).unwrap();
        fs::write(r.env.config.join("prompts/project-notes.md"), "Keep every section under ten bullets.").unwrap();
        fs::write(r.env.config.join("prompts/project-description.md"), "Start with a verb.").unwrap();
        mark_due(&r.env.cache, &id, now_ms()).unwrap();
        helper_loop(&r.env, &id, 0);
        assert_eq!(r.fake.calls(), 2);
        assert!(r.fake.has_arg(1, &notesupdate::system_with("Keep every section under ten bullets.")), "{:?}", r.fake.argv(1));
        assert!(!r.fake.call_text_has(1, "How to write (Markdown)"), "the built-in guidance is not sent as well");
        assert!(r.fake.has_arg(2, &describe::system_with("Start with a verb.")), "{:?}", r.fake.argv(2));
        // The replies are read exactly as with the built-in text: the rules are platoon's.
        assert!(r.notes(&id).contains("The basket page is in."));
        assert_eq!(Projects::open(&r.env.config).get(&id).unwrap().description, "A sock shop with a basket.");
    }

    #[test]
    fn the_helper_waits_for_quiet_then_runs_once_and_stops() {
        let r = Rig::new("loop");
        let id = r.project(NOTES);
        r.session(&id, "s1", &["Add a basket page to the shop"], "Done.");
        r.fake.reply_default(r#"{"sections":[{"heading":"Status","body":"The basket page is in."}]}"#);
        // A mark that is already old enough (the quiet period is a parameter here).
        mark_due(&r.env.cache, &id, now_ms()).unwrap();
        helper_loop(&r.env, &id, 0);
        // One update, then the description that follows it, and then it stopped.
        assert_eq!(r.fake.calls(), 2, "the update and the description that follows it");
        assert!(r.fake.stdin(2).contains("Write the line for this project."), "the second call is the description: {}", r.fake.stdin(2));
        assert!(r.notes(&id).contains("The basket page is in."));
        // The stand-in answered the description call with the update's own JSON, which is not a description: so
        // nothing was written for it.
        assert_eq!(Projects::open(&r.env.config).get(&id).unwrap().description, "");
        assert_eq!(due_at(&r.env.cache, &id), None, "the mark is cleared");
        assert!(!updating(&r.env.cache, &id), "and nothing says an update is running");
    }

    // ---- the project's one-line description -------------------------------------------------------------------

    #[test]
    fn the_description_is_written_from_the_summaries_and_the_notes_and_then_not_again_for_an_hour() {
        let r = Rig::new("describe");
        let id = r.project(NOTES);
        r.session(&id, "s1", &["Add a basket page to the shop"], "Done.");
        r.session(&id, "s2", &["Take card payments"], "Done.");
        r.summary("s1", "Basket page", "Building the basket and its totals.", 10);
        r.summary("s2", "Payments", "Taking card payments.", 20);
        r.fake.reply_default(r#"{"description":"A sock shop with a basket and card payments."}"#);

        assert_eq!(describe_once(&r.env, &id), Described::Written("A sock shop with a basket and card payments.".into()));
        assert_eq!(r.description(&id), "A sock shop with a basket and card payments.");
        // What was sent: the notes, both conversations (newest summary first), no session id anywhere.
        let sent = r.fake.stdin(1);
        assert!(sent.contains("Project: Web shop") && sent.contains("Sell socks."), "{sent}");
        assert!(sent.contains("[1] Payments") && sent.contains("[2] Basket page"), "the newest summary first: {sent}");
        assert!(!sent.contains("s1") || !sent.contains("s2"), "no session ids: {sent}");
        assert_eq!(r.fake.calls(), 1);
        // The spend is in the ledger under its own feature.
        let day = Ledger::open(&r.env.cache).today(usage::now());
        assert_eq!(day.features.get(crate::describe::FEATURE).map(|f| f.calls), Some(1));

        // One an hour: straight away there is nothing to do, and no call is made.
        assert_eq!(describe_once(&r.env, &id), Described::NotDue);
        assert_eq!(r.fake.calls(), 1, "nothing was asked a second time");
        // An hour later it is asked again.
        let mut p = Projects::open(&r.env.config);
        p.set_description(&id, &r.description(&id), usage::now() - crate::describe::REFRESH_SECS).unwrap();
        r.fake.reply_default(r#"{"description":"A sock shop that now takes cards."}"#);
        assert_eq!(describe_once(&r.env, &id), Described::Written("A sock shop that now takes cards.".into()));
        assert_eq!(r.description(&id), "A sock shop that now takes cards.");
        assert_eq!(r.fake.calls(), 2);
    }

    #[test]
    fn a_description_that_is_not_understood_writes_nothing_at_all() {
        let r = Rig::new("describe-bad");
        let id = r.project(NOTES);
        r.session(&id, "s1", &["Add a basket page to the shop"], "Done.");
        r.summary("s1", "Basket page", "Building the basket.", 10);
        let mut p = Projects::open(&r.env.config);
        p.set_description(&id, "What it was before", 1).unwrap();
        for nonsense in ["I can't do that.", "{}", r#"{"description":""}"#, r#"{"description":"ok"}"#] {
            r.fake.reply_default(nonsense);
            assert_eq!(describe_once(&r.env, &id), Described::BadReply, "{nonsense:?}");
            assert_eq!(r.description(&id), "What it was before", "the line that is there stays: {nonsense:?}");
            // The time is not touched either, so the next update tries again rather than waiting an hour.
            assert_eq!(Projects::open(&r.env.config).get(&id).unwrap().description_at, 1);
        }
        // A control character or an escape sequence never reaches the file.
        r.fake.reply_default("{\"description\":\"\\u001b[31mSocks\\u0007 and hats for sale\"}");
        assert_eq!(describe_once(&r.env, &id), Described::Written("Socks and hats for sale".into()));
        assert_eq!(r.description(&id), "Socks and hats for sale");
    }

    #[test]
    fn the_description_obeys_the_switch_the_daily_limit_and_an_archived_project() {
        let r = Rig::new("describe-gate");
        let id = r.project(NOTES);
        r.session(&id, "s1", &["Add a basket page to the shop"], "Done.");
        r.summary("s1", "Basket page", "Building the basket.", 10);
        r.fake.reply_default(r#"{"description":"A sock shop with a basket."}"#);
        let mut s = crate::settings::load(&r.env.config).0;

        // Its own switch off.
        s.set_feature(crate::describe::FEATURE, false);
        crate::settings::save(&r.env.config, &s).unwrap();
        let refused = describe_once(&r.env, &id);
        assert!(matches!(&refused, Described::Refused(why) if why.contains("Project descriptions")), "{refused:?}");

        // The master switch off.
        s.set_feature(crate::describe::FEATURE, true);
        s.tokens_enabled = false;
        crate::settings::save(&r.env.config, &s).unwrap();
        assert!(matches!(describe_once(&r.env, &id), Described::Refused(_)));

        // Today's tokens spent.
        s.tokens_enabled = true;
        crate::settings::save(&r.env.config, &s).unwrap();
        let spent = Usage { input: s.clone().clamped().ai_daily_tokens + 1, ..Usage::default() };
        Ledger::open(&r.env.cache).record_call(usage::now(), FEATURE, &spent, 0.0, true).unwrap();
        let refused = describe_once(&r.env, &id);
        assert!(matches!(&refused, Described::Refused(why) if why.contains("limit")), "{refused:?}");

        // An archived project is skipped entirely, even with everything switched on and nothing spent.
        let _ = fs::remove_file(Ledger::open(&r.env.cache).path());
        let mut p = Projects::open(&r.env.config);
        p.archive(&id, &[]).unwrap();
        assert_eq!(describe_once(&r.env, &id), Described::Nothing);
        assert_eq!(r.fake.calls(), 0, "nothing was ever asked");
        assert_eq!(r.description(&id), "");
    }

    #[test]
    fn nothing_to_go_on_costs_no_call_and_an_archived_session_is_left_out() {
        let r = Rig::new("describe-material");
        let id = r.project("   \n");
        r.session(&id, "s1", &["Add a basket page to the shop"], "Done.");
        // Empty notes and no summary yet: nothing to describe.
        assert_eq!(describe_once(&r.env, &id), Described::Nothing);
        assert_eq!(r.fake.calls(), 0);
        // An archived session's summary is not used.
        r.summary("s1", "Basket page", "Building the basket.", 10);
        crate::archive::Archive::open(&r.env.config).archive_many(&["s1".to_string()], usage::now()).unwrap();
        let m = describe_material(&r.env.config, &id, "Web shop");
        assert!(m.sessions.is_empty() && m.is_empty(), "{m:?}");
        assert_eq!(describe_once(&r.env, &id), Described::Nothing);
        assert_eq!(r.fake.calls(), 0);
    }

    // ---- a session joining a project -----------------------------------------------------------------------------

    #[test]
    fn a_session_joining_a_project_marks_it_so_an_update_runs_without_anyone_resuming_anything() {
        let r = Rig::new("joined");
        let id = r.project(NOTES);
        let mut p = Projects::open(&r.env.config);
        p.assign("s1", Some(&id)).unwrap();
        assert!(mark_joined(&r.env.config, &r.env.cache, &id, "s1", 12_345));
        assert_eq!(due_at(&r.env.cache, &id), Some(12_345), "the quiet timer starts from here");
        // A later one simply starts the timer over, exactly as a later turn does.
        assert!(mark_joined(&r.env.config, &r.env.cache, &id, "s1", 99_000));
        assert_eq!(due_at(&r.env.cache, &id), Some(99_000));
    }

    #[test]
    fn joining_marks_nothing_when_there_is_nothing_to_keep_up_to_date() {
        let r = Rig::new("joined-no");
        let id = r.project(NOTES);
        let empty = {
            let mut p = Projects::open(&r.env.config);
            let e = p.create("Empty").unwrap();
            fs::create_dir_all(r.env.config.join("projects").join(&e)).unwrap();
            fs::write(p.notes_path(&e), "\n").unwrap();
            e
        };
        assert!(!mark_joined(&r.env.config, &r.env.cache, &empty, "s1", 1), "a project whose notes the user emptied");
        assert!(!mark_joined(&r.env.config, &r.env.cache, "pnope", "s1", 1), "a project that isn't there");
        // An archived session is skipped.
        crate::archive::Archive::open(&r.env.config).archive_many(&["gone".to_string()], usage::now()).unwrap();
        assert!(!mark_joined(&r.env.config, &r.env.cache, &id, "gone", 1));
        // An archived project is skipped.
        let mut p = Projects::open(&r.env.config);
        p.archive(&id, &[]).unwrap();
        assert!(!mark_joined(&r.env.config, &r.env.cache, &id, "s1", 1));
        p.restore(&id).unwrap();
        // The switch off means no marks at all.
        let mut s = crate::settings::load(&r.env.config).0;
        s.set_feature(FEATURE, false);
        crate::settings::save(&r.env.config, &s).unwrap();
        assert!(!mark_joined(&r.env.config, &r.env.cache, &id, "s1", 1));
        assert!(!r.env.cache.join("notes-due").join(&id).exists(), "nothing was ever marked");
        // With it back on, it marks.
        s.set_feature(FEATURE, true);
        crate::settings::save(&r.env.config, &s).unwrap();
        assert!(mark_joined(&r.env.config, &r.env.cache, &id, "s1", 7));
    }
}
