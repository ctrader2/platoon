//! Adapter over Claude Code's on-disk state. Everything here reads undocumented internal
//! formats (`~/.claude/sessions/<pid>.json` and `~/.claude/projects/*/<id>.jsonl`), so all
//! knowledge of them lives in this file.

use serde_json::Value;
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

const TAIL_BYTES: u64 = 512 * 1024;
/// How far into a transcript to look for its first real prompt and working directory.
const HEAD_LINES: usize = 2000;
const HEAD_BYTES: u64 = 2 * 1024 * 1024;
const DEFAULT_WINDOW: u64 = 200_000;
const LARGE_WINDOW: u64 = 1_000_000;
/// Longest title or prompt kept per conversation, so the index stays small however long they are.
const TEXT_MAX: usize = 300;

/// Where Claude Code keeps its state (`CLAUDE_CONFIG_DIR`, else `~/.claude`).
/// The user's real one: code that a unit test runs reads a temporary root instead (`Reader::with_root`).
pub fn claude_root() -> PathBuf {
    crate::usage::forbid_real_dir("claude_root");
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(&std::env::var("HOME").unwrap_or_default()).join(".claude"))
}

/// Claude Code's user config file, which holds (among other things) the MCP servers added with
/// `claude mcp add`: `.claude.json` inside `CLAUDE_CONFIG_DIR` when that is set, else `~/.claude.json`
/// (next to `~/.claude`, not inside it).
pub fn claude_json_path() -> PathBuf {
    crate::usage::forbid_real_dir("claude_json_path");
    claude_json_in(std::env::var_os("CLAUDE_CONFIG_DIR"), &std::env::var("HOME").unwrap_or_default())
}

fn claude_json_in(config_dir: Option<std::ffi::OsString>, home: &str) -> PathBuf {
    match config_dir {
        Some(dir) => PathBuf::from(dir).join(".claude.json"),
        None => Path::new(home).join(".claude.json"),
    }
}

/// A live Claude Code process, from its registry file.
#[derive(Debug)]
pub struct Live {
    pub pid: u32,
    pub id: String,
    pub cwd: String,
    /// Only a name the user chose; Claude's derived placeholder is dropped.
    pub name: Option<String>,
    pub started_at: u64,
    pub busy: bool,
    /// Set for a background session: one run by Claude's own service (`claude --bg`, `claude agents`) rather than in
    /// a terminal. It is `claude attach <id>`, never `claude --resume`, that opens it. Holds that id.
    pub background: Option<String>,
    /// Set for an interactive session that handed its conversation to a background session (Claude Code's
    /// `parkedJobId`): holds the id of that background session's job (the id `claude attach` takes). Its process
    /// stays alive but no longer works on the conversation, and Claude never sets its registry status back to idle,
    /// so a parked entry is never `busy` and never counts as a copy that writes to the conversation.
    pub parked: Option<String>,
}

impl Live {
    pub fn is_background(&self) -> bool {
        self.background.is_some()
    }

    pub fn is_parked(&self) -> bool {
        self.parked.is_some()
    }
}

/// The background job an interactive registry entry handed its conversation to (`parkedJobId`, seen in Claude Code
/// 2.1.283), if it did. The one place that knows the key: `live_from`, `registry_status` and the guard ask here.
pub fn parked_job(v: &Value) -> Option<String> {
    if v.get("kind").and_then(Value::as_str) != Some("interactive") {
        return None;
    }
    v.get("parkedJobId").and_then(Value::as_str).map(str::trim).filter(|j| !j.is_empty()).map(String::from)
}

// ---- what a process started inside a Claude Code session inherits ------------------------------------------------

/// The one place that decides which inherited environment variables a `claude` platoon starts (a hosted session, a
/// task's session, a `claude -p` call) must NOT get.
///
/// A process started from inside a Claude Code session (platoon run from Claude's own terminal, or by an agent)
/// inherits variables that describe **that calling session**, and a new `claude` that sees them takes itself for a
/// part of it: with `CLAUDE_CODE_CHILD_SESSION` it saves no transcript at all (seen with Claude Code 2.1.283), so
/// no resume, no tokens and no row. Dropped, by name (all seen in a session of 2.1.283):
/// - `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`: "running inside Claude Code, started as the CLI".
/// - `CLAUDE_CODE_CHILD_SESSION`, `CLAUDE_CODE_SESSION_ATTENDED`: the calling session's kind.
/// - `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_BRIDGE_SESSION_ID`, `CLAUDE_PID`: the calling session's ids and process.
/// - `CLAUDE_CODE_MESSAGING_SOCKET`, `CLAUDE_CODE_MESSAGING_TOKEN`: the way back into the calling session.
/// - `CLAUDE_EFFORT`: the calling session's effort level (a new session picks its own).
/// - `AI_AGENT`: "run by an agent", said by the calling session about its own tools.
///
/// And by shape, so a newer Claude Code's marker is dropped too: any other `CLAUDE_CODE_*` whose name says it is
/// about a session, its parent, child, bridge, messaging, entry point, attendance, IDE port or process id.
///
/// Kept, because they configure Claude for the user rather than describe the caller: `CLAUDE_CONFIG_DIR`,
/// `ANTHROPIC_*` (API key, base URL, models), `CLAUDE_CODE_USE_*` (Bedrock, Vertex), `CLAUDE_CODE_OAUTH_TOKEN` (the
/// user's login), `CLAUDE_CODE_EXECPATH` (only a path to Claude's own files), and every other variable.
pub const SESSION_MARKERS: [&str; 11] = [
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_BRIDGE_SESSION_ID",
    "CLAUDE_PID",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_EFFORT",
    "AI_AGENT",
];

/// The words that make any other `CLAUDE_CODE_*` variable a marker of the calling session.
const MARKER_WORDS: [&str; 9] = ["SESSION", "PARENT", "CHILD", "BRIDGE", "MESSAGING", "ENTRYPOINT", "ATTENDED", "SSE_PORT", "_PID"];

/// Is this inherited variable a marker of the calling Claude Code session (see `SESSION_MARKERS`)?
pub fn is_session_marker(name: &str) -> bool {
    if SESSION_MARKERS.contains(&name) {
        return true;
    }
    let Some(rest) = name.strip_prefix("CLAUDE_CODE_") else { return false };
    if rest.starts_with("USE_") || name == "CLAUDE_CODE_OAUTH_TOKEN" || name == "CLAUDE_CODE_EXECPATH" {
        return false;
    }
    MARKER_WORDS.iter().any(|w| name.contains(w))
}

/// The markers among these variable names.
pub fn session_markers_in(names: impl IntoIterator<Item = String>) -> Vec<String> {
    names.into_iter().filter(|n| is_session_marker(n)).collect()
}

/// The markers in this process's own environment: what a `claude` platoon starts must have taken away.
pub fn inherited_session_markers() -> Vec<String> {
    session_markers_in(std::env::vars_os().filter_map(|(k, _)| k.into_string().ok()))
}

/// The arguments that open a background session's screen: `claude attach <id>`.
pub fn attach_args(attach_id: &str) -> [String; 2] {
    ["attach".to_string(), attach_id.to_string()]
}

/// The id `claude attach` takes when the registry doesn't say: the first 8 characters of the session id,
/// which is how Claude itself prints it.
pub fn short_id(session_id: &str) -> String {
    session_id.chars().take(8).collect()
}

/// A working folder whose sessions are not conversations to show: the folder platoon's own `claude -p` calls run in.
/// A process registered there, or a transcript recorded there, is platoon's own helper, never something the user
/// opened. Compared after resolving `.`, `..` and symlinks, so a different spelling of the same folder still counts,
/// while a folder that only has a similar name (`ai-work-2`, or `ai-work` somewhere else) does not.
#[derive(Clone, Debug)]
pub struct Hidden {
    resolved: PathBuf,
}

impl Hidden {
    pub fn new(dir: &Path) -> Hidden {
        Hidden { resolved: resolve_path(dir) }
    }

    /// Is `cwd` this folder, or somewhere inside it?
    pub fn covers(&self, cwd: &str) -> bool {
        !cwd.is_empty() && resolve_path(Path::new(cwd)).starts_with(&self.resolved)
    }
}

/// `p` with symlinks and `..` resolved as far as it exists; the part that doesn't exist (yet, or any more) is
/// tidied up in writing only.
fn resolve_path(p: &Path) -> PathBuf {
    use std::path::Component;
    if let Ok(c) = fs::canonicalize(p) {
        return c;
    }
    let comps: Vec<Component> = p.components().collect();
    let (mut out, from) = (0..comps.len())
        .rev()
        .find_map(|k| fs::canonicalize(comps[..k].iter().collect::<PathBuf>()).ok().filter(|_| k > 0).map(|c| (c, k)))
        .unwrap_or_default();
    for c in &comps[from..] {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

pub struct Reader {
    root: PathBuf,
    hidden: Option<Hidden>,
}

impl Reader {
    pub fn new() -> Self {
        Self::with_root(claude_root())
    }

    pub fn with_root(root: PathBuf) -> Self {
        Self { root, hidden: None }
    }

    /// Never report a process whose working folder is (inside) `dir`.
    pub fn hide(&mut self, dir: Hidden) {
        self.hidden = Some(dir);
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// All live processes that run a conversation, oldest first: interactive ones and background sessions (see
    /// `Live::background`). Several can share one session id. Not the ones in a hidden folder (`hide`).
    pub fn live(&self) -> Vec<Live> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(self.root.join("sessions")) else { return out };
        for path in entries.flatten().map(|e| e.path()) {
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(v) = fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) else {
                continue;
            };
            if let Some(l) = live_from(&v).filter(|l| !self.hidden.as_ref().is_some_and(|h| h.covers(&l.cwd))) {
                out.push(l);
            }
        }
        out.sort_by_key(|l| (l.started_at, l.pid));
        out
    }
}

/// What Claude Code's registry says a process is doing (`busy`, `idle`, ...). None if there is no entry or
/// it can't be read: callers must treat that as "not known to be idle". `parked` for a process that handed its
/// conversation to a background session (see `parked_job`): whatever its stale `status` says, it is neither working
/// nor known to be idle.
pub fn registry_status(root: &Path, pid: u32) -> Option<String> {
    let v: Value = serde_json::from_str(&fs::read_to_string(root.join("sessions").join(format!("{pid}.json"))).ok()?).ok()?;
    if v["pid"].as_u64().is_some_and(|p| p != pid as u64) {
        return None; // not this process's entry
    }
    if parked_job(&v).is_some() {
        return Some("parked".into());
    }
    v["status"].as_str().map(String::from)
}

fn live_from(v: &Value) -> Option<Live> {
    let background = match v["kind"].as_str() {
        Some("interactive") => false,
        // Claude's own service runs these. Its idle, pre-started spares (`"spare": true`) hold no conversation yet.
        Some("bg") if v["spare"].as_bool() != Some(true) => true,
        _ => return None,
    };
    let pid = v["pid"].as_u64()? as u32;
    if !pid_alive(pid) {
        return None; // stale registry file
    }
    let name = match (v["nameSource"].as_str(), v["name"].as_str()) {
        // "derived" is the placeholder; a background session's "auto" name is Claude's own title for it.
        (Some("derived" | "auto") | None, _) | (_, None) => None,
        (_, Some(n)) => Some(n.to_string()),
    };
    let id = v["sessionId"].as_str()?.to_string();
    // The short form Claude prints and `claude attach` takes is the registry's `jobId` when it has one.
    let background = background.then(|| v["jobId"].as_str().filter(|j| !j.is_empty()).map_or_else(|| short_id(&id), String::from));
    let parked = parked_job(v);
    Some(Live {
        pid,
        id,
        cwd: v["cwd"].as_str().unwrap_or("").to_string(),
        name,
        started_at: v["startedAt"].as_u64().unwrap_or(0),
        // A parked entry keeps the `busy` it had when it handed the conversation over, for good.
        busy: v["status"].as_str() == Some("busy") && parked.is_none(),
        background,
        parked,
    })
}

/// `claude-sonnet-5` -> `Sonnet 5`, `claude-fable-5-1` -> `Fable 5.1`, `claude-haiku-4-5-20251001` -> `Haiku 4.5`.
/// Handles the older `claude-3-5-sonnet-...` ordering too. Anything unrecognised is shown as-is.
pub fn pretty_model(id: &str) -> String {
    let (base, ext) = match id.split_once('[') {
        Some((b, rest)) => (b, Some(rest.trim_end_matches(']').to_uppercase())),
        None => (id, None),
    };
    let Some(rest) = base.strip_prefix("claude-") else { return id.to_string() };
    let mut family = None;
    let mut nums: Vec<&str> = Vec::new();
    for t in rest.split('-') {
        if t.len() == 8 && t.chars().all(|c| c.is_ascii_digit()) {
            continue; // release date
        }
        if t.chars().all(|c| c.is_ascii_digit()) {
            nums.push(t);
        } else if family.is_none() {
            family = Some(t);
        }
    }
    let Some(family) = family else { return id.to_string() };
    let mut name: String = family.chars().next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
    name.push_str(&family[family.chars().next().map_or(0, char::len_utf8)..]);
    if !nums.is_empty() {
        name = format!("{name} {}", nums.join("."));
    }
    match ext {
        Some(e) if !e.is_empty() => format!("{name} {e}"),
        _ => name,
    }
}

/// Does a model id name the 1M-token window itself: `claude-opus-5-5[1m]`, or a `context-1m` variant?
fn says_large_window(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.ends_with("[1m]") || m.contains("context-1m")
}

/// The transcript doesn't record the model's window size, so infer it: 200k until the model id names the 1M window
/// or the session is seen past 200k tokens, then 1M for good (compaction shrinks usage but not the window).
pub fn window_size(tokens: u64, large: &mut bool) -> u64 {
    if tokens > DEFAULT_WINDOW {
        *large = true;
    }
    if *large { LARGE_WINDOW } else { DEFAULT_WINDOW }
}

/// Working directory a conversation was run in: the first `cwd` recorded in its transcript.
pub fn transcript_cwd(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    for line in BufReader::new(file).lines().take(400).map_while(Result::ok) {
        if let Some(cwd) = serde_json::from_str::<Value>(&line).ok().and_then(|v| v["cwd"].as_str().map(String::from)) {
            return Some(cwd);
        }
    }
    None
}

/// Every conversation id with a saved transcript, for looking a session up by (a prefix of) its id.
pub fn saved_session_ids(root: &Path) -> Vec<String> {
    let mut ids = Vec::new();
    for dir in fs::read_dir(root.join("projects")).into_iter().flatten().flatten() {
        for f in fs::read_dir(dir.path()).into_iter().flatten().flatten() {
            let p = f.path();
            if p.extension().and_then(|e| e.to_str()) == Some("jsonl")
                && let Some(stem) = p.file_stem().and_then(|s| s.to_str())
            {
                ids.push(stem.to_string());
            }
        }
    }
    ids
}

pub fn pid_alive(pid: u32) -> bool {
    // Signal 0 checks existence only. EPERM means it exists but belongs to someone else.
    let exists = unsafe { libc::kill(pid as i32, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) };
    alive_verdict(exists, read_proc_stat(pid))
}

/// What reading `/proc/<pid>/stat` came to.
#[derive(Debug, PartialEq)]
#[allow(dead_code)] // each platform makes only some of these (macOS only `Zombie`, `Vanished`, `Unreadable`)
enum StatRead {
    /// No `/proc` on this platform (macOS), or it is not mounted: the file says nothing either way.
    NoProc,
    /// The process vanished under us: the file is gone (NotFound), or the kernel refused the read because
    /// the process was ending (ESRCH, "No such process": it shows up as an unclassified error, not NotFound).
    Vanished,
    /// Some other trouble reading (permission, ...).
    Unreadable,
    Text(String),
    /// No `/proc`, but the kernel said whether it is a zombie (macOS: `procs::zombie`).
    Zombie(bool),
}

#[cfg(target_os = "macos")]
fn read_proc_stat(pid: u32) -> StatRead {
    match crate::procs::zombie(pid) {
        Ok(z) => StatRead::Zombie(z),
        Err(libc::ESRCH) => StatRead::Vanished,
        Err(_) => StatRead::Unreadable,
    }
}

#[cfg(not(target_os = "macos"))]
fn read_proc_stat(pid: u32) -> StatRead {
    if !cfg!(target_os = "linux") {
        return StatRead::NoProc;
    }
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => StatRead::Text(s),
        Err(e) => classify_read_error(&e, std::path::Path::new("/proc/self/stat").exists()),
    }
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
fn classify_read_error(e: &std::io::Error, proc_is_there: bool) -> StatRead {
    if e.kind() == std::io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ESRCH) {
        // Every pid looks "not found" when /proc is not mounted at all: then we cannot tell.
        if proc_is_there { StatRead::Vanished } else { StatRead::NoProc }
    } else {
        StatRead::Unreadable
    }
}

/// The decision behind `pid_alive`, kept pure so the race can be tested without timing.
///
/// A process that has exited stays a zombie (state `Z`) until its parent reaps it, and a zombie still
/// answers signal 0, though it is gone as far as anyone using it is concerned. And if the parent reaps it
/// between our signal 0 and our read of `/proc/<pid>/stat`, the read fails (NotFound or ESRCH): that also
/// means gone (reading it as "not a zombie, so alive" made a dead process look alive for one call). macOS has
/// no `/proc`: there the kernel is asked directly (`StatRead::Zombie`). Any other read error (say, permission),
/// an unparseable line, or no way to ask at all leaves what the signal said: it exists.
fn alive_verdict(exists_per_kill: bool, stat: StatRead) -> bool {
    if !exists_per_kill {
        return false;
    }
    match stat {
        StatRead::NoProc | StatRead::Unreadable => true,
        StatRead::Vanished => false,
        StatRead::Zombie(z) => !z,
        StatRead::Text(s) => {
            let state = s.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().next().map(str::to_owned));
            !matches!(state.as_deref(), Some("Z" | "X" | "x"))
        }
    }
}

pub fn find_transcript(root: &Path, id: &str) -> Option<PathBuf> {
    fs::read_dir(root.join("projects")).ok()?.flatten().map(|d| d.path().join(format!("{id}.jsonl"))).find(|p| p.exists())
}

/// What platoon knows about one saved conversation, read from its transcript.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transcript {
    /// Working directory the conversation started in (the first `cwd` recorded).
    pub cwd: Option<String>,
    /// Folders added beside it (`/add-dir`, `--add-dir`), as of the latest environment snapshot read. Empty when
    /// none were added, or the transcript is older than those snapshots.
    pub extra_dirs: Vec<String>,
    pub branch: Option<String>,
    /// Raw model id from the latest assistant message.
    pub model: Option<String>,
    pub context_tokens: u64,
    /// Once a session has gone past 200k tokens it must have a large window. Never flips back.
    pub large_window: bool,
    pub ai_title: Option<String>,
    /// First real user prompt. None means nothing but slash-command echoes and injected blocks: junk.
    pub first_prompt: Option<String>,
    /// Timestamp of the latest user or assistant entry, ms since the epoch.
    pub last_active_ms: Option<u64>,
}

/// Read a transcript cheaply, whatever its size: the first real prompt and cwd from the head, and the
/// latest title, usage, model, branch and time from a bounded tail. `prev` is the earlier reading of
/// the same file: what only the head or a distant title can tell us is kept when it isn't found again.
///
/// The added folders come from the newest environment snapshot in the tail, else the one written at the start
/// (read from the head on a first reading only), else `prev`'s. The limit: in a file longer than the tail, a
/// change written between the head and the tail, before this file was first read, is missed until the next
/// change (or until the tail reaches one).
pub fn read_transcript(path: &Path, prev: Option<&Transcript>) -> Option<Transcript> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut t = prev.cloned().unwrap_or_default();
    if prev.is_none() || t.first_prompt.is_none() || t.cwd.is_none() {
        scan_head(&file, &mut t, prev.is_none());
    }

    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let mut lines = text.lines();
    if start > 0 {
        lines.next(); // probably cut mid-line
    }
    scan_tail(lines.collect::<Vec<_>>().into_iter().rev(), &mut t);
    Some(t)
}

/// When a conversation started: the time of the first user or assistant entry in its transcript (ms since the
/// epoch). Only the head of the file is read. None when there is no such entry there, or no time on it.
pub fn started_ms(path: &Path) -> Option<u64> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file.take(HEAD_BYTES));
    let mut line = Vec::new();
    for _ in 0..HEAD_LINES {
        line.clear();
        if !matches!(reader.read_until(b'\n', &mut line), Ok(n) if n > 0) {
            return None;
        }
        let line = String::from_utf8_lossy(&line);
        if !(line.contains("\"type\":\"user\"") || line.contains("\"type\":\"assistant\"")) || !line.contains("\"timestamp\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        if let Some(ms) = v["timestamp"].as_str().and_then(parse_iso_ms) {
            return Some(ms);
        }
    }
    None
}

/// What one conversation spent, summed from the usage records in its transcript (and its subagents' transcripts).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Spent {
    /// Sent to the model and not read from its cache: `input_tokens` plus `cache_creation_input_tokens`.
    pub input: u64,
    /// Written by the model (`output_tokens`, thinking included).
    pub output: u64,
    /// Read back from the model's cache (`cache_read_input_tokens`): the conversation so far, sent again each turn.
    pub cache_read: u64,
    /// Some of it was spent by the conversation's subagents.
    pub subagents: bool,
}

type Usages = std::collections::HashMap<String, (u64, u64, u64)>;

/// What a conversation spent, read from its whole transcript and from its subagents' transcripts beside it
/// (`<id>/subagents/*.jsonl`). Claude Code writes one line per piece of an answer, each carrying the same message
/// id and usage, so an answer is counted once (its last line wins). Lines that aren't JSON, or aren't an
/// assistant's answer with a usage record, are skipped. None when there is no usage record at all.
///
/// It reads every line of possibly large files: never call it on the UI thread.
pub fn spent(path: &Path) -> Option<Spent> {
    let mut seen = Usages::new();
    let mut unkeyed = 0usize;
    let main = add_usage(path, &mut seen, &mut unkeyed);
    let mut from_subagents = false;
    if let Ok(entries) = fs::read_dir(path.with_extension("").join("subagents")) {
        let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "jsonl")).collect();
        files.sort();
        for f in files {
            from_subagents |= add_usage(&f, &mut seen, &mut unkeyed);
        }
    }
    if !main && !from_subagents {
        return None;
    }
    Some(Spent { subagents: from_subagents, ..sum(&seen) })
}

/// Add one transcript's usage records to `seen` (keyed by message id, or request id, else one key per line).
/// True if it had any.
fn add_usage(path: &Path, seen: &mut Usages, unkeyed: &mut usize) -> bool {
    let Ok(file) = File::open(path) else { return false };
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut any = false;
    loop {
        line.clear();
        if !matches!(reader.read_until(b'\n', &mut line), Ok(n) if n > 0) {
            break;
        }
        let text = String::from_utf8_lossy(&line);
        if !text.contains("\"usage\"") || !text.contains("\"assistant\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
        any |= usage_of(&v, seen, unkeyed);
    }
    any
}

/// Add one transcript entry's usage record to `seen`, if it is an assistant's answer with one. True if it was.
fn usage_of(v: &Value, seen: &mut Usages, unkeyed: &mut usize) -> bool {
    if v.get("type").and_then(Value::as_str) != Some("assistant") {
        return false;
    }
    let Some(msg) = v.get("message") else { return false };
    let Some(u) = msg.get("usage").filter(|u| u.is_object()) else { return false };
    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    let counts = (n("input_tokens") + n("cache_creation_input_tokens"), n("output_tokens"), n("cache_read_input_tokens"));
    let key = match (msg.get("id").and_then(Value::as_str), v.get("requestId").and_then(Value::as_str)) {
        (Some(id), _) if !id.is_empty() => format!("m:{id}"),
        (_, Some(req)) if !req.is_empty() => format!("r:{req}"),
        _ => {
            *unkeyed += 1;
            format!("l:{unkeyed}")
        }
    };
    seen.insert(key, counts);
    true
}

/// What a task's session has used, and when it last wrote: what its transcript and its subagents' transcripts spent
/// (`spent`), and the latest time any of those files was written (ms since the epoch).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionLog {
    pub spent: Spent,
    pub last_at: Option<u64>,
}

/// Read a task's session's transcript (see `SessionLog`). None when there is no transcript yet. It reads every line:
/// never call it on the UI thread.
pub fn read_session_log(path: &Path) -> Option<SessionLog> {
    let written = |p: &Path| {
        let t = fs::metadata(p).and_then(|m| m.modified()).ok()?;
        Some(t.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64)
    };
    let mut last_at = written(path)?;
    for e in fs::read_dir(path.with_extension("").join("subagents")).into_iter().flatten().flatten() {
        if e.path().extension().is_some_and(|x| x == "jsonl") {
            last_at = last_at.max(written(&e.path()).unwrap_or(0));
        }
    }
    Some(SessionLog { spent: spent(path).unwrap_or_default(), last_at: Some(last_at) })
}

fn sum(u: &Usages) -> Spent {
    let mut s = Spent::default();
    for (input, output, cache_read) in u.values() {
        s.input += input;
        s.output += output;
        s.cache_read += cache_read;
    }
    s
}

/// `fresh`: nothing was read from this file before, so the added folders are looked for too. The snapshot
/// written at the start comes just after the first prompt and before the first reply, so the first reply ends
/// that search (an older transcript has none).
fn scan_head(file: &File, t: &mut Transcript, fresh: bool) {
    let mut reader = BufReader::new(file.take(HEAD_BYTES));
    let mut line = Vec::new();
    let mut env_done = !fresh;
    for _ in 0..HEAD_LINES {
        if t.cwd.is_some() && t.first_prompt.is_some() && env_done {
            break;
        }
        line.clear();
        if !matches!(reader.read_until(b'\n', &mut line), Ok(n) if n > 0) {
            break;
        }
        let line = String::from_utf8_lossy(&line);
        if !env_done {
            if let Some(dirs) = added_dirs(&line) {
                t.extra_dirs = dirs;
                env_done = true;
            } else if line.contains("\"type\":\"assistant\"") {
                env_done = true;
            }
        }
        let want_cwd = t.cwd.is_none() && line.contains("\"cwd\"");
        let want_prompt = t.first_prompt.is_none() && line.contains("\"type\":\"user\"");
        if !want_cwd && !want_prompt {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v["isSidechain"].as_bool() == Some(true) {
            continue; // subagent content is not the conversation
        }
        if want_cwd {
            t.cwd = v["cwd"].as_str().filter(|c| !c.is_empty()).map(String::from);
        }
        if want_prompt {
            t.first_prompt = real_prompt(&v);
        }
    }
}

/// The folders added beside a conversation's own (`/add-dir`, `--add-dir`), when this transcript line is one of
/// Claude Code's environment snapshots: `{"type":"attachment","attachment":{"type":"environment","snapshot":
/// {"additionalWorkingDirectories":[...]}}}`. One is written at the start and another whenever the environment
/// changes, each with the whole list (which can be empty again). A line that isn't one, or is malformed: None.
fn added_dirs(line: &str) -> Option<Vec<String>> {
    if !line.contains("\"additionalWorkingDirectories\"") {
        return None;
    }
    let v = serde_json::from_str::<Value>(line).ok()?;
    let a = &v["attachment"];
    if v["type"].as_str() != Some("attachment") || a["type"].as_str() != Some("environment") || v["isSidechain"].as_bool() == Some(true) {
        return None;
    }
    let list = a["snapshot"]["additionalWorkingDirectories"].as_array()?;
    let dirs = list.iter().map(|d| d.as_str().map(String::from)).collect::<Option<Vec<_>>>()?;
    Some(dirs.into_iter().filter(|d| !d.is_empty()).collect())
}

/// `lines` run newest first, so the first hit for each fact is the latest one.
fn scan_tail<'a>(lines: impl Iterator<Item = &'a str>, t: &mut Transcript) {
    let (mut usage, mut branch, mut ts, mut title, mut env) = (false, false, false, false, false);
    let mut need_prompt = t.first_prompt.is_none();
    for line in lines {
        if usage && branch && ts && title && env && !need_prompt {
            break;
        }
        if !env && let Some(dirs) = added_dirs(line) {
            t.extra_dirs = dirs;
            env = true;
            continue;
        }
        let is_title = line.contains("\"type\":\"ai-title\"");
        let is_turn = line.contains("\"type\":\"assistant\"") || line.contains("\"type\":\"user\"");
        // Once the cheap facts are in, only a title (or a fallback prompt) is worth another parse.
        let done_turns = usage && branch && ts && !need_prompt;
        if !(is_title && !title || is_turn && !done_turns) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match v["type"].as_str() {
            Some("ai-title") if !title => {
                if let Some(x) = v["aiTitle"].as_str().map(one_line).filter(|x| !x.is_empty()) {
                    t.ai_title = Some(truncate(x));
                    title = true;
                }
            }
            Some(kind @ ("assistant" | "user")) => {
                if !ts && let Some(ms) = v["timestamp"].as_str().and_then(parse_iso_ms) {
                    t.last_active_ms = Some(ms);
                    ts = true;
                }
                let side = v["isSidechain"].as_bool() == Some(true);
                if !branch
                    && !side
                    && let Some(b) = v["gitBranch"].as_str()
                {
                    t.branch = Some(b.to_string());
                    branch = true;
                }
                // Subagent (sidechain) messages have their own context; ignore them.
                if kind == "assistant" && !usage && !side {
                    let u = &v["message"]["usage"];
                    if u.is_object() && v["message"]["model"].as_str() != Some("<synthetic>") {
                        if let Some(m) = v["message"]["model"].as_str() {
                            t.model = Some(m.to_string());
                        }
                        let n = |k: &str| u[k].as_u64().unwrap_or(0);
                        t.context_tokens = n("input_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens");
                        if t.context_tokens > DEFAULT_WINDOW || t.model.as_deref().is_some_and(says_large_window) {
                            t.large_window = true;
                        }
                        usage = true;
                    }
                }
                if kind == "user"
                    && need_prompt
                    && !side
                    && let Some(p) = real_prompt(&v)
                {
                    t.first_prompt = Some(p);
                    need_prompt = false;
                }
            }
            _ => {}
        }
    }
}

/// The text of a real user prompt: not a slash-command echo, caveat, compaction summary, interrupt marker
/// or other injected `<...>` block.
fn real_prompt(v: &Value) -> Option<String> {
    real_prompt_capped(v, TEXT_MAX)
}

/// `real_prompt`, keeping at most `cap` characters.
fn real_prompt_capped(v: &Value, cap: usize) -> Option<String> {
    let flag = |k: &str| v[k].as_bool() == Some(true);
    if v["type"].as_str() != Some("user") || flag("isMeta") || flag("isSidechain") || flag("isCompactSummary") {
        return None;
    }
    let text = match &v["message"]["content"] {
        Value::String(s) => one_line(s),
        Value::Array(a) => one_line(&a.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join(" ")),
        _ => return None,
    };
    (!text.is_empty() && !text.starts_with('<') && !text.starts_with("[Request interrupted")).then(|| truncate_to(text, cap))
}

/// The real user prompt in one line of a transcript, if it holds one (same rules as the description of a row),
/// cut to `cap` characters. Built for reading a whole transcript, most of which is tool output: a line that
/// isn't a user entry, that carries a tool result, or that belongs to a subagent is turned down by a substring
/// test alone, without being parsed.
pub fn prompt_in_line(line: &str, cap: usize) -> Option<String> {
    if !line.contains("\"type\":\"user\"") || line.contains("\"type\":\"tool_result\"") || line.contains("\"isSidechain\":true") {
        return None;
    }
    real_prompt_capped(&serde_json::from_str::<Value>(line).ok()?, cap)
}

/// How much of Claude's last reply is kept per conversation (characters): one line of context for the smart names.
pub const REPLY_CHARS: usize = 200;

/// Could this line of a transcript be something Claude said in words? A substring test only, so a whole transcript
/// can be run through it without parsing: an assistant entry that is not a subagent's, not one of Claude's own
/// synthetic notices (an error message, "no response requested") and that has a text block. A tool call, a tool
/// result and a thinking block are other kinds of entry or block and don't match.
pub fn may_be_reply(line: &str) -> bool {
    line.contains("\"type\":\"assistant\"")
        && line.contains("\"type\":\"text\"")
        && !line.contains("\"isSidechain\":true")
        && !line.contains("<synthetic>")
}

/// The words Claude said in this line of a transcript: the text blocks of an assistant entry that is not a
/// subagent's, as one line, cut to `cap` characters. Never tool calls, tool output or thinking. None when the line
/// has no such text.
pub fn reply_in_line(line: &str, cap: usize) -> Option<String> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v["type"].as_str() != Some("assistant") || v["isSidechain"].as_bool() == Some(true) {
        return None;
    }
    let content = &v["message"]["content"];
    let text = match content {
        Value::String(s) => one_line(s),
        Value::Array(a) => one_line(
            &a.iter().filter(|b| b["type"].as_str() == Some("text")).filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join(" "),
        ),
        _ => return None,
    };
    (!text.is_empty()).then(|| truncate_to(text, cap))
}

/// The words Claude said in this line of a transcript as it wrote them, line breaks kept, for the notes updater:
/// what is still open in a conversation lives in Claude's replies, and one line of one is not enough to see it. The
/// same entries as `reply_in_line` (text blocks of an assistant entry that is not a subagent's; never tool calls,
/// tool output or thinking); each line cleaned of escape sequences and control characters, runs of blank lines
/// folded into one, and the whole cut to `cap` characters with `…` where it was cut. None when there is no text.
pub fn reply_text_in_line(line: &str, cap: usize) -> Option<String> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v["type"].as_str() != Some("assistant") || v["isSidechain"].as_bool() == Some(true) {
        return None;
    }
    let raw = match &v["message"]["content"] {
        Value::String(s) => s.clone(),
        Value::Array(a) => {
            a.iter().filter(|b| b["type"].as_str() == Some("text")).filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n\n")
        }
        _ => return None,
    };
    // A reply can be enormous: far more than could ever be kept is not even looked at.
    let raw: String = raw.chars().take(cap.saturating_mul(4).max(64)).collect();
    let mut lines: Vec<String> = Vec::new();
    let mut blank = false;
    for l in raw.split('\n') {
        let l = crate::smart::clean_text(l, cap.max(1));
        if l.trim().is_empty() {
            blank = !lines.is_empty();
            continue;
        }
        if blank {
            lines.push(String::new());
            blank = false;
        }
        lines.push(l);
    }
    let text = lines.join("\n");
    if text.is_empty() {
        return None;
    }
    Some(match text.char_indices().nth(cap) {
        Some((at, _)) => format!("{}…", text[..at].trim_end()),
        None => text,
    })
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(s: String) -> String {
    truncate_to(s, TEXT_MAX)
}

fn truncate_to(s: String, cap: usize) -> String {
    match s.char_indices().nth(cap) {
        Some((at, _)) => s[..at].to_string(),
        None => s,
    }
}

/// `2026-09-19T03:53:40.091Z` -> ms since the epoch. Only the UTC form Claude Code writes is understood.
pub fn parse_iso_ms(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, sec) = (num(0..4)?, num(5..7)?, num(8..10)?, num(11..13)?, num(14..16)?, num(17..19)?);
    let frac: u64 = match b.get(19) {
        Some(b'.') => {
            let digits: String = s[20..].chars().take_while(|c| c.is_ascii_digit()).take(3).collect();
            format!("{digits:0<3}").parse().ok()?
        }
        _ => 0,
    };
    // Days since 1970-01-01 (Howard Hinnant's civil-from-days, inverted).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3600 + mi * 60 + sec;
    u64::try_from(secs).ok().map(|s| s * 1000 + frac)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn liveness_is_decided_by_the_signal_and_then_the_state_line() {
        use StatRead::{NoProc, Unreadable, Vanished};
        // Not there per the signal: never alive, whatever the state file says.
        assert!(!alive_verdict(false, stat("S")));
        for r in [NoProc, Vanished, Unreadable, StatRead::Text(String::new())] {
            assert!(!alive_verdict(false, r));
        }
        // There, and running, sleeping, waiting: alive (comm with spaces and a ')' is parsed past).
        for st in ["R", "S", "D", "T", "t", "I"] {
            assert!(alive_verdict(true, stat(st)), "{st}");
        }
        // There but a zombie or dead: gone.
        for st in ["Z", "X", "x"] {
            assert!(!alive_verdict(true, stat(st)), "{st}");
        }
        // The race: the signal worked, then the parent reaped it before we read the state (file gone, or
        // the kernel said ESRCH): gone.
        assert!(!alive_verdict(true, Vanished));
        // Other read trouble, or a line we cannot read, or no /proc (macOS): the signal stands.
        assert!(alive_verdict(true, Unreadable));
        // macOS: the kernel's answer about a zombie decides.
        assert!(alive_verdict(true, StatRead::Zombie(false)) && !alive_verdict(true, StatRead::Zombie(true)));
        assert!(!alive_verdict(false, StatRead::Zombie(false)));
        assert!(alive_verdict(true, StatRead::Text("garbage".into())));
        assert!(alive_verdict(true, StatRead::Text(String::new())));
        assert!(alive_verdict(true, NoProc));
    }

    #[test]
    fn a_failed_read_of_the_state_file_is_sorted_into_gone_or_unknown() {
        use std::io::{Error, ErrorKind};
        let esrch = Error::from_raw_os_error(libc::ESRCH);
        assert_eq!(classify_read_error(&Error::from(ErrorKind::NotFound), true), StatRead::Vanished);
        assert_eq!(classify_read_error(&esrch, true), StatRead::Vanished, "the kernel says ESRCH for a process that is ending");
        assert_eq!(classify_read_error(&Error::from(ErrorKind::NotFound), false), StatRead::NoProc, "no /proc: cannot tell");
        assert_eq!(classify_read_error(&Error::from(ErrorKind::PermissionDenied), true), StatRead::Unreadable);
        assert_eq!(classify_read_error(&Error::from_raw_os_error(libc::EIO), true), StatRead::Unreadable);
    }

    #[test]
    fn a_child_that_has_exited_but_was_not_reaped_yet_is_not_alive() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        // Not reaped: a zombie. The kill takes a moment to land, so wait for it (generously: a loaded machine).
        let start = std::time::Instant::now();
        while pid_alive(pid) {
            assert!(start.elapsed() < std::time::Duration::from_secs(30), "a killed child nobody reaped still reads as alive");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, 0, "it is a zombie, which still answers a signal: only its state tells");
        child.wait().unwrap();
    }

    #[test]
    fn pid_alive_tells_a_running_process_from_a_reaped_one() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        assert!(pid_alive(pid));
        child.kill().unwrap();
        child.wait().unwrap(); // reaped: gone for good
        assert!(!pid_alive(pid));
        assert!(pid_alive(std::process::id()));
    }

    #[test]
    fn the_user_config_lives_in_the_config_dir_when_that_is_set_and_next_to_the_dot_claude_dir_otherwise() {
        assert_eq!(claude_json_in(Some("/x/cfg".into()), "/home/u"), Path::new("/x/cfg/.claude.json"));
        assert_eq!(claude_json_in(None, "/home/u"), Path::new("/home/u/.claude.json"));
    }

    fn stat(state: &str) -> StatRead {
        StatRead::Text(format!("123 (a b) c) {state} 1 123 123 0 -1"))
    }

    #[test]
    fn the_calling_sessions_markers_are_dropped_and_what_configures_claude_is_kept() {
        // Everything a Claude Code 2.1.283 session gave its shell, plus the user's own settings.
        let seen = [
            "AI_AGENT",
            "CLAUDECODE",
            "CLAUDE_CODE_BRIDGE_SESSION_ID",
            "CLAUDE_CODE_CHILD_SESSION",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_CODE_EXECPATH",
            "CLAUDE_CODE_MESSAGING_SOCKET",
            "CLAUDE_CODE_MESSAGING_TOKEN",
            "CLAUDE_CODE_SESSION_ATTENDED",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_EFFORT",
            "CLAUDE_PID",
            "CLAUDE_CONFIG_DIR",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_BASE_URL",
            "MAX_THINKING_TOKENS",
            "PATH",
            "HOME",
            "PLATOON_CONFIG_DIR",
            // Shapes a newer Claude Code might add.
            "CLAUDE_CODE_PARENT_SESSION_ID",
            "CLAUDE_CODE_SSE_PORT",
            "CLAUDE_CODE_SESSION_ACCESS_TOKEN",
            "CLAUDE_CODE_CALLER_PID",
        ];
        let dropped = session_markers_in(seen.iter().map(|s| s.to_string()));
        assert_eq!(
            dropped,
            [
                "AI_AGENT",
                "CLAUDECODE",
                "CLAUDE_CODE_BRIDGE_SESSION_ID",
                "CLAUDE_CODE_CHILD_SESSION",
                "CLAUDE_CODE_ENTRYPOINT",
                "CLAUDE_CODE_MESSAGING_SOCKET",
                "CLAUDE_CODE_MESSAGING_TOKEN",
                "CLAUDE_CODE_SESSION_ATTENDED",
                "CLAUDE_CODE_SESSION_ID",
                "CLAUDE_EFFORT",
                "CLAUDE_PID",
                "CLAUDE_CODE_PARENT_SESSION_ID",
                "CLAUDE_CODE_SSE_PORT",
                "CLAUDE_CODE_SESSION_ACCESS_TOKEN",
                "CLAUDE_CODE_CALLER_PID",
            ]
        );
        for kept in [
            "CLAUDE_CONFIG_DIR",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CLAUDE_CODE_EXECPATH",
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
        ] {
            assert!(!is_session_marker(kept), "{kept}");
        }
        for m in SESSION_MARKERS {
            assert!(is_session_marker(m), "{m}");
        }
    }

    /// A pid that certainly belongs to no live process: a child that has been waited for.
    fn dead_pid() -> u32 {
        let mut c = std::process::Command::new("true").spawn().unwrap();
        let pid = c.id();
        c.wait().unwrap();
        pid
    }

    fn entry(kind: &str, pid: u32, extra: Value) -> Value {
        let mut v = serde_json::json!({"pid":pid,"sessionId":"dffac639-c805-47ba-b57c-3767f4f768d8","cwd":"/work","kind":kind,"startedAt":7,"status":"busy"});
        for (k, x) in extra.as_object().unwrap() {
            v.as_object_mut().unwrap().insert(k.clone(), x.clone());
        }
        v
    }

    #[test]
    fn a_background_entry_is_a_live_process_told_apart_from_an_interactive_one() {
        let me = std::process::id();
        let bg = live_from(&entry("bg", me, serde_json::json!({"jobId":"dffac639"}))).unwrap();
        assert_eq!(
            (bg.pid, bg.id.as_str(), bg.cwd.as_str(), bg.started_at, bg.busy),
            (me, "dffac639-c805-47ba-b57c-3767f4f768d8", "/work", 7, true)
        );
        assert_eq!(bg.background.as_deref(), Some("dffac639"));
        assert!(bg.is_background());

        let inter = live_from(&entry("interactive", me, serde_json::json!({}))).unwrap();
        assert!(!inter.is_background() && inter.background.is_none());
    }

    #[test]
    fn the_attach_id_is_the_registrys_job_id_or_else_the_first_eight_characters() {
        let me = std::process::id();
        let job = live_from(&entry("bg", me, serde_json::json!({"jobId":"abc12345"}))).unwrap();
        assert_eq!(job.background.as_deref(), Some("abc12345"), "what Claude itself says wins");
        let plain = live_from(&entry("bg", me, serde_json::json!({}))).unwrap();
        assert_eq!(plain.background.as_deref(), Some("dffac639"));
        let empty = live_from(&entry("bg", me, serde_json::json!({"jobId":""}))).unwrap();
        assert_eq!(empty.background.as_deref(), Some("dffac639"));
        assert_eq!(short_id("xyz"), "xyz");
        assert_eq!(attach_args("dffac639"), ["attach", "dffac639"]);
    }

    #[test]
    fn a_stale_background_entry_or_one_of_the_services_idle_spares_or_another_kind_is_not_live() {
        let me = std::process::id();
        assert!(live_from(&entry("bg", dead_pid(), serde_json::json!({}))).is_none(), "its process has gone");
        assert!(live_from(&entry("interactive", dead_pid(), serde_json::json!({}))).is_none());
        // Claude keeps pre-started processes waiting for work; they hold no conversation.
        assert!(live_from(&entry("bg", me, serde_json::json!({"spare":true,"jobId":"e418370e"}))).is_none());
        assert!(live_from(&entry("bg", me, serde_json::json!({"spare":false}))).is_some());
        assert!(live_from(&entry("daemon", me, serde_json::json!({}))).is_none());
        assert!(live_from(&serde_json::json!({"pid":me,"kind":"bg"})).is_none(), "no session id");
    }

    #[test]
    fn an_interactive_entry_that_handed_its_conversation_to_a_background_job_is_parked_and_never_busy() {
        let me = std::process::id();
        // The shape seen in Claude Code 2.1.283: the status stays "busy" from before the hand-over, for good.
        let parked = live_from(&entry("interactive", me, serde_json::json!({"parkedJobId":"a70fa3de","statusUpdatedAt":1}))).unwrap();
        assert_eq!(parked.parked.as_deref(), Some("a70fa3de"));
        assert!(parked.is_parked() && !parked.busy && !parked.is_background());
        assert_eq!((parked.pid, parked.id.as_str(), parked.cwd.as_str()), (me, "dffac639-c805-47ba-b57c-3767f4f768d8", "/work"));

        // Without the key, or with an empty or odd one, nothing changes.
        for extra in [
            serde_json::json!({}),
            serde_json::json!({"parkedJobId":""}),
            serde_json::json!({"parkedJobId":"  "}),
            serde_json::json!({"parkedJobId":null}),
            serde_json::json!({"parkedJobId":7}),
        ] {
            let l = live_from(&entry("interactive", me, extra.clone())).unwrap();
            assert!(l.parked.is_none() && !l.is_parked() && l.busy, "{extra}");
        }
        // Only an interactive entry can be parked; a background session is the one it was handed to.
        let bg = live_from(&entry("bg", me, serde_json::json!({"jobId":"a70fa3de","parkedJobId":"zzz"}))).unwrap();
        assert!(bg.parked.is_none() && bg.busy && bg.is_background());
        // A parked entry whose process has gone is gone like any other.
        assert!(live_from(&entry("interactive", dead_pid(), serde_json::json!({"parkedJobId":"a70fa3de"}))).is_none());
    }

    #[test]
    fn the_registry_status_of_a_parked_process_is_parked_never_busy() {
        let dir = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-claude-parked-{}", std::process::id())));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sessions")).unwrap();
        let put = |pid: u32, v: Value| fs::write(dir.join("sessions").join(format!("{pid}.json")), v.to_string()).unwrap();
        put(11, entry("interactive", 11, serde_json::json!({"parkedJobId":"a70fa3de"})));
        put(12, entry("interactive", 12, serde_json::json!({"parkedJobId":""})));
        put(13, entry("interactive", 13, serde_json::json!({"status":"idle"})));
        assert_eq!(registry_status(&dir, 11).as_deref(), Some("parked"));
        assert_eq!(registry_status(&dir, 12).as_deref(), Some("busy"));
        assert_eq!(registry_status(&dir, 13).as_deref(), Some("idle"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_background_sessions_own_title_is_not_taken_for_a_name_the_user_chose() {
        let me = std::process::id();
        let auto = live_from(&entry("bg", me, serde_json::json!({"name":"Continue work and release","nameSource":"auto"}))).unwrap();
        assert_eq!(auto.name, None);
        let derived = live_from(&entry("interactive", me, serde_json::json!({"name":"platoon-40","nameSource":"derived"}))).unwrap();
        assert_eq!(derived.name, None);
        let chosen = live_from(&entry("interactive", me, serde_json::json!({"name":"my work","nameSource":"user"}))).unwrap();
        assert_eq!(chosen.name.as_deref(), Some("my work"));
    }

    #[test]
    fn the_reader_lists_interactive_and_background_processes_oldest_first_from_a_registry_folder() {
        let dir = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-claude-live-{}", std::process::id())));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sessions")).unwrap();
        let me = std::process::id();
        let put = |name: &str, v: Value| fs::write(dir.join("sessions").join(name), v.to_string()).unwrap();
        put("1.json", entry("bg", me, serde_json::json!({"startedAt":5})));
        put("2.json", entry("interactive", me, serde_json::json!({"startedAt":3,"sessionId":"other"})));
        put("3.json", entry("bg", me, serde_json::json!({"startedAt":1,"spare":true,"sessionId":"spare-one"})));
        fs::write(dir.join("sessions/4.json"), "not json").unwrap();
        fs::write(dir.join("sessions/5.key"), "secret").unwrap();
        let live = Reader::with_root(dir.clone()).live();
        assert_eq!(
            live.iter().map(|l| (l.id.as_str(), l.is_background())).collect::<Vec<_>>(),
            [("other", false), ("dffac639-c805-47ba-b57c-3767f4f768d8", true)]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn model_names_are_shortened() {
        assert_eq!(pretty_model("claude-sonnet-5"), "Sonnet 5");
        assert_eq!(pretty_model("claude-fable-5-1"), "Fable 5.1");
        assert_eq!(pretty_model("claude-opus-5"), "Opus 5");
        assert_eq!(pretty_model("claude-haiku-4-5-20251001"), "Haiku 4.5");
        assert_eq!(pretty_model("claude-3-5-sonnet-20241022"), "Sonnet 3.5");
        assert_eq!(pretty_model("claude-sonnet-5[1m]"), "Sonnet 5 1M");
        assert_eq!(pretty_model("some-other-model"), "some-other-model");
        assert_eq!(pretty_model("claude-"), "claude-");
    }

    #[test]
    fn window_estimate_never_flips_back() {
        let mut large = false;
        assert_eq!(window_size(190_000, &mut large), 200_000);
        assert_eq!(window_size(210_000, &mut large), 1_000_000);
        // After compaction the usage drops, but the window is still the large one.
        assert_eq!(window_size(60_000, &mut large), 1_000_000);
    }

    #[test]
    fn the_context_is_the_last_main_replys_input_and_a_model_id_can_name_the_large_window() {
        let dir = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-claude-ctx-{}", std::process::id())));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let reply = |model: &str, input: u64, write: u64, read: u64, side: bool| {
            serde_json::json!({"type":"assistant","isSidechain":side,"message":{"model":model,
                "usage":{"input_tokens":input,"cache_creation_input_tokens":write,"cache_read_input_tokens":read,"output_tokens":9}}})
            .to_string()
        };
        let read_with = |name: &str, lines: &[String]| {
            let p = dir.join(name);
            fs::write(&p, lines.join("\n") + "\n").unwrap();
            read_transcript(&p, None).unwrap()
        };
        // The three input counts of the last reply of the conversation itself; a subagent's later reply is not it.
        let t = read_with(
            "a.jsonl",
            &[
                reply("claude-opus-5-5", 1, 1, 1, false),
                reply("claude-opus-5-5", 2_000, 8_000, 110_000, false),
                reply("claude-haiku-5-5", 7, 0, 0, true),
            ],
        );
        assert_eq!((t.context_tokens, t.large_window), (120_000, false));
        let mut large = t.large_window;
        assert_eq!(t.context_tokens * 100 / window_size(t.context_tokens, &mut large), 60);
        // A model id that names the 1M window: the same use is a small share of it.
        for model in ["claude-opus-5-5[1m]", "claude-sonnet-5-context-1m"] {
            let t = read_with("b.jsonl", &[reply(model, 2_000, 8_000, 110_000, false)]);
            assert!(t.large_window, "{model}");
            let mut large = t.large_window;
            assert_eq!(window_size(t.context_tokens, &mut large), 1_000_000, "{model}");
        }
        // Past 200k the window must be the large one, whatever the id says.
        assert!(read_with("c.jsonl", &[reply("claude-opus-5-5", 0, 0, 250_000, false)]).large_window);
        let _ = fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod transcript_tests {
    use super::*;

    #[test]
    fn iso_timestamps_convert_to_epoch_ms() {
        assert_eq!(parse_iso_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(parse_iso_ms("2026-09-19T03:53:40.091Z"), Some(1_789_790_020_091));
        assert_eq!(parse_iso_ms("2024-02-29T12:00:00Z"), Some(1_709_208_000_000));
        assert_eq!(parse_iso_ms("2026-09-19T03:53:40.5Z"), Some(1_789_790_020_500));
        assert_eq!(parse_iso_ms("not a time"), None);
    }

    fn prompt(v: Value) -> Option<String> {
        real_prompt(&v)
    }

    #[test]
    fn only_real_prompts_count() {
        use serde_json::json;
        let user = |c: Value| json!({"type":"user","message":{"content":c}});
        assert_eq!(prompt(user(json!("fix the bug"))).as_deref(), Some("fix the bug"));
        assert_eq!(prompt(user(json!([{"type":"text","text":"a\n  b"}]))).as_deref(), Some("a b"));
        assert_eq!(prompt(user(json!("<command-name>/clear</command-name>"))), None);
        assert_eq!(prompt(user(json!("[Request interrupted by user]"))), None);
        assert_eq!(prompt(user(json!([{"type":"tool_result","content":"x"}]))), None);
        assert_eq!(prompt(json!({"type":"user","isMeta":true,"message":{"content":"Caveat: x"}})), None);
        assert_eq!(prompt(json!({"type":"user","isSidechain":true,"message":{"content":"sub"}})), None);
        assert_eq!(prompt(json!({"type":"user","isCompactSummary":true,"message":{"content":"This session is being continued"}})), None);
        assert_eq!(prompt(json!({"type":"assistant","message":{"content":"hi"}})), None);
    }

    #[test]
    fn a_prompt_in_a_line_follows_the_same_rules_and_takes_its_own_length() {
        let line = |c: &str| serde_json::json!({"type":"user","message":{"content":c}}).to_string();
        assert_eq!(prompt_in_line(&line("fix   the\nbug"), 100).as_deref(), Some("fix the bug"));
        assert_eq!(prompt_in_line(&line("abcdefghij"), 4).as_deref(), Some("abcd"));
        assert_eq!(prompt_in_line(&line("é".repeat(10).as_str()), 3).as_deref(), Some("ééé"), "characters, not bytes");
        assert_eq!(prompt_in_line(&line("<command-name>/x</command-name>"), 100), None);
        assert_eq!(prompt_in_line(r#"{"type":"assistant","message":{"content":"hi"}}"#, 100), None);
        assert_eq!(prompt_in_line(r#"{"type":"user","isSidechain":true,"message":{"content":"sub"}}"#, 100), None);
        assert_eq!(prompt_in_line(r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"x"}]}}"#, 100), None);
        assert_eq!(prompt_in_line("not json but \"type\":\"user\"", 100), None);
        assert_eq!(prompt_in_line("", 100), None);
    }

    #[test]
    fn a_reply_is_the_text_claude_said_and_nothing_else() {
        use serde_json::json;
        let asst = |c: Value| json!({"type":"assistant","message":{"model":"claude-sonnet-5","content":c}}).to_string();
        let line = asst(json!([{"type":"text","text":"I fixed\nthe   redirect."}]));
        assert!(may_be_reply(&line));
        assert_eq!(reply_in_line(&line, 200).as_deref(), Some("I fixed the redirect."));
        assert_eq!(reply_in_line(&line, 7).as_deref(), Some("I fixed"), "cut to the given number of characters");
        let two = asst(
            json!([{"type":"text","text":"one"},{"type":"tool_use","name":"Bash","input":{"command":"rm -rf /"}},{"type":"text","text":"two"}]),
        );
        assert_eq!(reply_in_line(&two, 200).as_deref(), Some("one two"), "tool calls are not in it");
        // Not replies: a tool call alone, thinking, tool output, the user's own words, a subagent, Claude's synthetic notices.
        let tool = asst(json!([{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]));
        assert!(!may_be_reply(&tool) && reply_in_line(&tool, 200).is_none());
        let thinking = asst(json!([{"type":"thinking","thinking":"hmm"}]));
        assert!(!may_be_reply(&thinking) && reply_in_line(&thinking, 200).is_none());
        let result =
            json!({"type":"user","message":{"content":[{"type":"tool_result","content":"{\"type\":\"assistant\"} {\"type\":\"text\"}"}]}})
                .to_string();
        assert!(!may_be_reply(&result), "text inside a tool result is escaped and never matches");
        assert!(reply_in_line(&result, 200).is_none());
        let side = json!({"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"sub"}]}}).to_string();
        assert!(!may_be_reply(&side) && reply_in_line(&side, 200).is_none());
        let synthetic =
            json!({"type":"assistant","message":{"model":"<synthetic>","content":[{"type":"text","text":"No response requested."}]}})
                .to_string();
        assert!(!may_be_reply(&synthetic));
        assert_eq!(reply_in_line(&asst(json!([{"type":"text","text":"   "}])), 200), None);
        assert_eq!(reply_in_line("not json", 200), None);
        assert_eq!(reply_in_line(&asst(json!("plain string")), 200).as_deref(), Some("plain string"));
    }

    #[test]
    fn the_notes_read_a_reply_with_its_lines_kept_cleaned_and_cut_to_the_length_asked_for() {
        use serde_json::json;
        let asst = |c: Value| json!({"type":"assistant","message":{"model":"claude-sonnet-5","content":c}}).to_string();
        // A long reply of many lines, with an escape sequence, a bell and a run of blank lines in it.
        let mut text = String::from(
            "Done. Two things are still open:\n\n\n\n- \u{1b}[31mWhich card provider?\u{1b}[0m\u{7}\n- Does VAT go in the total?\n\n",
        );
        for n in 0..200 {
            text.push_str(&format!("Line {n} of the rest of a long answer.\n"));
        }
        let line = asst(
            json!([{"type":"text","text":text},{"type":"tool_use","name":"Bash","input":{"command":"ls"}},{"type":"text","text":"Last words."}]),
        );
        let got = reply_text_in_line(&line, 1500).unwrap();
        assert!(
            got.starts_with("Done. Two things are still open:\n\n- Which card provider?\n- Does VAT go in the total?\n\nLine 0"),
            "{got}"
        );
        assert!(!got.chars().any(|c| c.is_control() && c != '\n'), "{got:?}");
        assert!(!got.contains("\n\n\n"), "blank runs fold into one");
        assert_eq!(got.chars().count(), 1501, "cut to the length asked for, with a mark where it was cut");
        assert!(got.ends_with('…'));
        // Each length the setting offers.
        for cap in [500, 1500, 3000] {
            assert!(reply_text_in_line(&line, cap).unwrap().chars().count() <= cap + 1);
        }
        // A short one comes back whole, with the tool call left out between its two parts.
        let short =
            asst(json!([{"type":"text","text":"one\ntwo"},{"type":"tool_use","name":"Bash","input":{}},{"type":"text","text":"three"}]));
        assert_eq!(reply_text_in_line(&short, 1500).as_deref(), Some("one\ntwo\n\nthree"));
        // Not replies, exactly as for the one-line reading.
        let side = json!({"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"sub"}]}}).to_string();
        assert_eq!(reply_text_in_line(&side, 1500), None);
        assert_eq!(reply_text_in_line(&asst(json!([{"type":"thinking","thinking":"hmm"}])), 1500), None);
        assert_eq!(reply_text_in_line(&asst(json!([{"type":"text","text":" \n\n "}])), 1500), None);
        assert_eq!(reply_text_in_line("not json", 1500), None);
        // And the one-line reading the prompt index and the smart names use is unchanged.
        assert_eq!(reply_in_line(&line, REPLY_CHARS).unwrap().lines().count(), 1);
        assert_eq!(reply_in_line(&line, REPLY_CHARS).unwrap().chars().count(), REPLY_CHARS);
    }
}

#[cfg(test)]
mod hidden_tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn tmp(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-hidden-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_folder_is_hidden_however_it_is_spelled_and_a_similar_name_is_not() {
        let base = tmp("spell");
        let work = base.join("cache/ai-work");
        fs::create_dir_all(&work).unwrap();
        fs::create_dir_all(base.join("cache/ai-work-2")).unwrap();
        fs::create_dir_all(base.join("other/ai-work")).unwrap();
        fs::create_dir_all(base.join("cache/x")).unwrap();
        symlink(&work, base.join("link")).unwrap();
        let h = Hidden::new(&work);
        let s = |p: PathBuf| p.display().to_string();
        for yes in [
            s(work.clone()),
            format!("{}/", work.display()),
            s(base.join("cache/x/../ai-work")),
            s(base.join("./cache/./ai-work")),
            s(base.join("link")),
            s(work.join("inside")),
            s(base.join("link/deeper/still")),
        ] {
            assert!(h.covers(&yes), "{yes}");
        }
        for no in [
            "",
            "/",
            "relative/ai-work",
            &s(base.join("cache")),
            &s(base.join("cache/ai-work-2")),
            &s(base.join("other/ai-work")),
            &s(base.join("cache/ai-wor")),
        ] {
            assert!(!h.covers(no), "{no}");
        }
        // Hidden before the folder exists (nothing has been called yet).
        let ahead = Hidden::new(&base.join("cache/not-yet/ai-work"));
        assert!(ahead.covers(&s(base.join("cache/not-yet/ai-work"))) && ahead.covers(&s(base.join("cache/x/../not-yet/ai-work"))));
        assert!(!ahead.covers(&s(base.join("cache/not-yet/ai-work-2"))));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn the_reader_leaves_out_live_processes_in_a_hidden_folder_and_keeps_the_rest() {
        let base = tmp("reader");
        let work = base.join("cache/ai-work");
        fs::create_dir_all(work.join("sub")).unwrap();
        fs::create_dir_all(base.join("cache/ai-work-2")).unwrap();
        fs::create_dir_all(base.join("claude/sessions")).unwrap();
        let me = std::process::id();
        let write = |n: u32, id: &str, cwd: &Path, kind: &str| {
            let v = serde_json::json!({"pid":me,"sessionId":id,"cwd":cwd,"kind":kind,"startedAt":n,"status":"busy","jobId":"abcd1234"});
            fs::write(base.join("claude/sessions").join(format!("{n}.json")), v.to_string()).unwrap();
        };
        write(1, "helper", &work, "interactive");
        write(2, "helper-bg", &work.join("sub"), "bg");
        write(3, "near", &base.join("cache/ai-work-2"), "interactive");
        write(4, "real", Path::new("/work/real"), "interactive");
        let mut r = Reader::with_root(base.join("claude"));
        let ids = |r: &Reader| r.live().into_iter().map(|l| l.id).collect::<Vec<_>>();
        assert_eq!(ids(&r), ["helper", "helper-bg", "near", "real"], "shown while nothing is hidden");
        r.hide(Hidden::new(&work));
        assert_eq!(ids(&r), ["near", "real"]);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_conversation_started_at_its_first_real_entry_and_a_file_without_one_has_no_start() {
        let base = tmp("started");
        let p = base.join("s.jsonl");
        let lines = [
            r#"{"type":"mode","mode":"normal","timestamp":"2026-09-20T09:00:00.000Z"}"#,
            r#"{"type":"user","isSidechain":true,"timestamp":"2026-09-20T09:30:00.000Z"}"#,
            r#"{"type":"user","timestamp":"2026-09-24T10:00:00.500Z","message":{"content":"hi"}}"#,
            r#"{"type":"assistant","timestamp":"2026-09-24T10:01:00.000Z"}"#,
        ];
        fs::write(&p, lines.join("\n") + "\n").unwrap();
        assert_eq!(started_ms(&p), parse_iso_ms("2026-09-24T10:00:00.500Z"));
        fs::write(&p, r#"{"type":"user","message":{"content":"no time"}}"#.to_string() + "\n").unwrap();
        assert_eq!(started_ms(&p), None);
        assert_eq!(started_ms(&base.join("missing.jsonl")), None);
        let _ = fs::remove_dir_all(&base);
    }

    /// One of Claude Code's environment snapshots, as written in a transcript (trimmed of the keys platoon ignores).
    fn env(dirs: &str) -> String {
        format!(
            r#"{{"parentUuid":"p","isSidechain":false,"attachment":{{"type":"environment","snapshot":{{"workingDirectory":"/work/x","isWorktree":false,"additionalWorkingDirectories":{dirs},"platform":"linux"}},"changes":null}},"type":"attachment","uuid":"u","timestamp":"2026-09-26T12:00:00.000Z","cwd":"/work/x","sessionId":"s"}}"#
        )
    }

    fn user_line(text: &str) -> String {
        format!(
            r#"{{"type":"user","cwd":"/work/x","timestamp":"2026-09-26T12:00:00.000Z","message":{{"role":"user","content":"{text}"}}}}"#
        )
    }

    /// An assistant reply big enough that a few hundred of them push the start of a file out of the tail.
    fn reply_line() -> String {
        format!(
            r#"{{"type":"assistant","cwd":"/work/x","timestamp":"2026-09-26T12:01:00.000Z","message":{{"model":"claude-sonnet-5","content":[{{"type":"text","text":"{}"}}],"usage":{{"input_tokens":10}}}}}}"#,
            "x".repeat(2000)
        )
    }

    fn dirs_of(p: &Path, lines: &[String], prev: Option<&Transcript>) -> Vec<String> {
        fs::write(p, lines.join("\n") + "\n").unwrap();
        read_transcript(p, prev).unwrap().extra_dirs
    }

    #[test]
    fn the_added_folders_are_the_latest_environment_snapshots_list() {
        let base = tmp("extra-dirs");
        let p = base.join("s.jsonl");
        let start = |dirs: &str| vec![r#"{"type":"mode","mode":"normal"}"#.to_string(), user_line("hello"), env(dirs), reply_line()];
        // Only the snapshot written at the start: its list (`--add-dir`), or nothing.
        assert_eq!(dirs_of(&p, &start(r#"["/"]"#), None), ["/"]);
        assert!(dirs_of(&p, &start("[]"), None).is_empty());
        // A later change (`/add-dir`) replaces it with the whole new list, and a later empty list clears it.
        let mut lines = start("[]");
        lines.extend([user_line("add one"), env(r#"["/home/u/docs","/srv"]"#), reply_line()]);
        assert_eq!(dirs_of(&p, &lines, None), ["/home/u/docs", "/srv"]);
        lines.extend([env("[]"), reply_line()]);
        assert!(dirs_of(&p, &lines, None).is_empty());
        // A transcript from before these snapshots has none.
        assert!(dirs_of(&p, &[user_line("old"), reply_line()], None).is_empty());
        // Malformed snapshots are passed over: the last good one stands.
        let mut lines = start(r#"["/good"]"#);
        for bad in [r#"[1,2]"#, r#""/not-a-list""#, "null"] {
            lines.push(env(bad));
        }
        lines.push(r#"{"type":"attachment","attachment":{"type":"environment","snapshot":{"additionalWorkingDirectories":["/cut"#.into());
        lines.push(env(r#"["/sub"]"#).replace(r#""isSidechain":false"#, r#""isSidechain":true"#));
        lines.push(env(r#"["/other"]"#).replace(r#"{"type":"environment""#, r#"{"type":"something_else""#));
        assert_eq!(dirs_of(&p, &lines, None), ["/good"]);
        // A user message that merely mentions the key is not a snapshot.
        let mut lines = start(r#"["/good"]"#);
        lines.push(user_line(r#"see \"additionalWorkingDirectories\":[\"/nope\"]"#));
        assert_eq!(dirs_of(&p, &lines, None), ["/good"]);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn in_a_long_transcript_the_head_gives_the_start_list_the_tail_the_latest_and_else_the_earlier_reading_stands() {
        let base = tmp("extra-dirs-long");
        let p = base.join("s.jsonl");
        let filler = || std::iter::repeat_with(reply_line).take((TAIL_BYTES / 2000 + 50) as usize);
        // The start snapshot is far outside the tail: a first reading finds it in the head.
        let mut lines = vec![user_line("hello"), env(r#"["/from-start"]"#), reply_line()];
        lines.extend(filler());
        assert_eq!(dirs_of(&p, &lines, None), ["/from-start"]);
        // A later reading (the head is not read again) keeps what the earlier one found when the tail has nothing.
        let prev = Transcript {
            cwd: Some("/work/x".into()),
            first_prompt: Some("hello".into()),
            extra_dirs: vec!["/earlier".into()],
            ..Transcript::default()
        };
        assert_eq!(dirs_of(&p, &lines, Some(&prev)), ["/earlier"]);
        // A change in the tail wins over both.
        lines.extend([env(r#"["/from-tail"]"#), reply_line()]);
        assert_eq!(dirs_of(&p, &lines, None), ["/from-tail"]);
        assert_eq!(dirs_of(&p, &lines, Some(&prev)), ["/from-tail"]);
        // Only in the tail (a conversation that had none at the start).
        let mut lines = vec![user_line("hello"), env("[]"), reply_line()];
        lines.extend(filler());
        lines.push(env(r#"["/added-late"]"#));
        assert_eq!(dirs_of(&p, &lines, None), ["/added-late"]);
        // The head's search for the start snapshot ends at the first reply: one after it is not the start list.
        let mut lines = vec![user_line("hello"), reply_line(), env(r#"["/mid"]"#)];
        lines.extend(filler());
        assert!(dirs_of(&p, &lines, None).is_empty());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn what_a_conversation_spent_is_summed_once_per_answer_from_its_transcript_and_its_subagents() {
        let base = tmp("spent");
        let p = base.join("abc.jsonl");
        let answer = |id: &str, input: u64, output: u64, write: u64, read: u64| {
            serde_json::json!({"type":"assistant","requestId":format!("req-{id}"),"message":{"id":id,"model":"claude-sonnet-5","usage":{
                "input_tokens":input,"output_tokens":output,"cache_creation_input_tokens":write,"cache_read_input_tokens":read}}})
            .to_string()
        };
        let lines = [
            r#"{"type":"user","message":{"content":"the word \"usage\" in a prompt, and \"assistant\" too"}}"#.to_string(),
            // One answer written as two lines (a thinking block and a text block): counted once, the last line wins.
            answer("m1", 2, 100, 1000, 0),
            answer("m1", 2, 120, 1000, 0),
            answer("m2", 3, 50, 10, 2000),
            "{\"type\":\"assistant\",\"message\":{\"usage\":".to_string(), // cut off mid-line
            "not json at all \"usage\" \"assistant\"".to_string(),
            r#"{"type":"assistant","message":{"usage":"not an object"}}"#.to_string(),
            // A request id and no message id: keyed by the request.
            r#"{"type":"assistant","requestId":"rq","message":{"usage":{"input_tokens":1,"output_tokens":1}}}"#.to_string(),
            r#"{"type":"assistant","requestId":"rq","message":{"usage":{"input_tokens":1,"output_tokens":1}}}"#.to_string(),
            // No ids at all: every such line counts on its own.
            r#"{"type":"assistant","message":{"usage":{"input_tokens":5,"output_tokens":6}}}"#.to_string(),
            r#"{"type":"assistant","message":{"usage":{"input_tokens":5,"output_tokens":6}}}"#.to_string(),
        ];
        fs::write(&p, lines.join("\n") + "\n").unwrap();
        let s = spent(&p).unwrap();
        assert_eq!(s, Spent { input: 2 + 1000 + 3 + 10 + 1 + 5 + 5, output: 120 + 50 + 1 + 6 + 6, cache_read: 2000, subagents: false });

        // Its subagents' transcripts count too, and it says so; files that aren't transcripts don't.
        let sub = base.join("abc/subagents");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("agent-1.jsonl"), answer("s1", 7, 8, 0, 9) + "\n").unwrap();
        fs::write(sub.join("agent-1.meta.json"), answer("s2", 999, 999, 999, 999)).unwrap();
        let s2 = spent(&p).unwrap();
        assert_eq!((s2.input, s2.output, s2.cache_read, s2.subagents), (s.input + 7, s.output + 8, s.cache_read + 9, true));

        // No usage record anywhere, a file of rubbish, or no file: nothing is said.
        let q = base.join("none.jsonl");
        fs::write(&q, r#"{"type":"user","message":{"content":"hi"}}"#.to_string() + "\n{broken\n").unwrap();
        assert_eq!(spent(&q), None);
        assert_eq!(spent(&base.join("missing.jsonl")), None);
        // An empty usage record is still a record: zero, not unknown.
        fs::write(&q, r#"{"type":"assistant","message":{"id":"z","usage":{}}}"#.to_string() + "\n").unwrap();
        assert_eq!(spent(&q), Some(Spent::default()));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_task_sessions_log_counts_its_subagents_and_says_when_it_last_wrote() {
        let base = tmp("session-log");
        let p = base.join("abc.jsonl");
        assert_eq!(read_session_log(&p), None, "no transcript yet");
        let answer = |id: &str, input: u64, output: u64| {
            serde_json::json!({"type":"assistant","message":{"id":id,"usage":{"input_tokens":input,"output_tokens":output}}}).to_string()
        };
        fs::write(&p, answer("a1", 10, 20) + "\n").unwrap();
        let only = read_session_log(&p).unwrap();
        assert_eq!((only.spent.input, only.spent.output, only.spent.subagents), (10, 20, false));
        let sub = base.join("abc/subagents");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("agent-1.jsonl"), answer("s1", 1, 2) + "\n").unwrap();
        let both = read_session_log(&p).unwrap();
        assert_eq!((both.spent.input, both.spent.output, both.spent.subagents), (11, 22, true));
        assert!(both.last_at.unwrap() >= only.last_at.unwrap());
        let _ = fs::remove_dir_all(&base);
    }
}
