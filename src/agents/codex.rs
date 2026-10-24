//! Adapter over OpenAI's Codex CLI and its on-disk state. Everything here reads formats Codex doesn't promise to keep
//! (its saved sessions, `session_index.jsonl`, the writer locks, `config.toml`), so all knowledge of them lives in this
//! file, and every reading is defensive: a record that can't be read, or one of a kind platoon doesn't know, is
//! skipped, never an error. (Codex also keeps SQLite stores and has a command that moves old sessions into them: the
//! files read here may move one day.)
//!
//! Where things are, under `$CODEX_HOME` (else `~/.codex`):
//! - `sessions/YYYY/MM/DD/rollout-<time>-<id>.jsonl`: one saved conversation ("rollout") per file, one JSON record
//!   per line, `{"timestamp", "type", "payload"}`: `session_meta` (id, folder, git), `turn_context` (the model),
//!   `response_item` (messages with roles, tool calls), and `event_msg`s (`task_started`, `task_complete`,
//!   `turn_aborted`, `token_count` with the context window, ...).
//! - `session_index.jsonl`: the names Codex gives conversations, one line per change (the latest line wins).
//! - `thread-writer-locks/<id>.lock`: held while a conversation is open. Codex keeps one writer per conversation
//!   itself, so the holder of the lock is the one process running it (sometimes Codex's shared background server,
//!   which runs many and is never signalled).
//! - `config.toml`: the MCP servers (`[mcp_servers.<name>]`), for idle stop. Read only: Codex rewrites it itself.
//! - `hooks.json`: hooks for every session; `platoon setup` adds its own there (see `HOOKS`).

use super::{
    HookAnswer, HookEvent, HookInput, HookKind, HookReply, Live, Start, TaskLaunch, Tool, Transcript, TurnEnd, one_line, parse_iso_ms,
    tidy_reply, truncate_to,
};
use crate::ai::{Outcome, Request, classify, extract_json};
use crate::ledger::{FailKind, Usage};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    time::UNIX_EPOCH,
};

const TAIL_BYTES: u64 = 512 * 1024;
/// How far into a rollout to look for its first real prompt and its folder. Codex writes a few long records of its
/// own instructions first.
const HEAD_LINES: usize = 400;
const HEAD_BYTES: u64 = 2 * 1024 * 1024;
/// Longest name or prompt kept per conversation, as for Claude Code's.
const TEXT_MAX: usize = 300;
/// The window before a rollout says (no reply yet: no tokens either). The models Codex lists have 272k windows; a
/// rollout's own figure always wins.
const DEFAULT_WINDOW: u64 = 272_000;

/// Where Codex keeps its state (`CODEX_HOME`, else `~/.codex`).
/// The user's real one: code that a unit test runs reads a temporary root instead (`Reader::with_root`).
pub fn codex_root() -> PathBuf {
    crate::usage::forbid_real_dir("codex_root");
    std::env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(&std::env::var("HOME").unwrap_or_default()).join(".codex"))
}

/// The program that is Codex here: `PLATOON_CODEX_BIN` (a wrapper, or a test's stand-in), else `codex`.
pub fn program() -> String {
    std::env::var("PLATOON_CODEX_BIN").unwrap_or_else(|_| "codex".into())
}

/// Is Codex on this computer: its folder `root` is there, or its program can be found on `path` (a `PATH` value)?
pub fn installed(root: &Path, program: &str, path: &str) -> bool {
    root.is_dir() || super::on_path(program, path)
}

// ---- starting a session ---------------------------------------------------------------------------------------------

/// The arguments `codex` is started with for `start`. Always `--no-daemon`: the session runs in the process platoon
/// started (the host's child), with its hooks, rather than in Codex's shared background server. A conversation is
/// continued with `resume <id>`. Codex has no background sessions to attach to: `Start::Attach` (never asked for one
/// of Codex's) resumes the conversation.
pub fn start_args(start: Start) -> Vec<String> {
    match start {
        Start::New(first) => std::iter::once("--no-daemon".to_string()).chain(first.map(String::from)).collect(),
        Start::Resume(id) | Start::Attach(id) => vec!["resume".into(), id.into(), "--no-daemon".into()],
    }
}

/// The session a host's arguments name: the one after `resume`.
pub fn session_of_args(args: &[String]) -> Option<String> {
    match args {
        [first, id, ..] if first == "resume" && !id.starts_with('-') && !id.is_empty() => Some(id.clone()),
        _ => None,
    }
}

/// The arguments of a task's session. No task runs on Codex yet (the squads' models are Claude Code's): this is only
/// the session itself, its model and its first message, without the squad's steps, plan mode or the task's hooks.
pub fn task_args(l: &TaskLaunch) -> Vec<String> {
    if l.resume {
        return start_args(Start::Resume(&l.session));
    }
    let mut a: Vec<String> = vec!["--no-daemon".into(), "-m".into(), l.model.clone()];
    a.extend(l.first.clone());
    a
}

// ---- hooks -----------------------------------------------------------------------------------------------------------
//
// Codex calls a hook with one JSON object on stdin, with Claude Code's field names (`session_id`, `transcript_path`,
// `cwd`, `model`, `permission_mode`, `prompt`, `source`, `stop_hook_active`, `last_assistant_message`, `tool_name`,
// `tool_input`), and reads context from stdout (`hookSpecificOutput.additionalContext`) or a decision. Its
// SessionStart fires when the first message is sent, not when the window opens.

/// The hooks `platoon setup` puts in Codex's `hooks.json`: (event, what the command says after the program). Fixed
/// text: Codex asks the user to trust each hook once, by its exact command, so one that changed would be asked about
/// again.
pub const HOOKS: [(&str, &str); 3] = [
    ("SessionStart", "hook session-start --agent codex"),
    ("UserPromptSubmit", "hook guard --agent codex"),
    ("Stop", "hook stop --agent codex"),
];

/// Codex's hooks file under its folder.
pub fn hooks_path(root: &Path) -> PathBuf {
    root.join("hooks.json")
}

/// Read one call of a hook (see `Agent::hook_input`).
pub fn hook_input(kind: HookKind, input: &str) -> Option<HookInput> {
    let v: Value = serde_json::from_str(input).ok()?;
    let session = v.get("session_id")?.as_str()?.to_string();
    let cwd = v.get("cwd").and_then(Value::as_str).unwrap_or("").to_string();
    let text = |k: &str| v.get(k).and_then(Value::as_str);
    let tool_input = v.get("tool_input").unwrap_or(&Value::Null);
    let event = match kind {
        HookKind::SessionStart => HookEvent::SessionStart { source: text("source").unwrap_or("startup").to_string() },
        HookKind::Prompt => HookEvent::PromptSubmitted,
        HookKind::BeforeTool(tool) => {
            // Codex has no plan tool: a plan is the last message of a turn in plan mode.
            let said = if tool == Tool::Question { questions(tool_input) } else { String::new() };
            HookEvent::BeforeTool { tool, said }
        }
        HookKind::AfterTool => HookEvent::AfterTool(match text("tool_name").unwrap_or_default() {
            "request_user_input" => Tool::Question,
            _ => Tool::Other,
        }),
        // Codex's `PermissionRequest`: every call is one, with the agent's reason as the tool's description.
        HookKind::PermissionWait => HookEvent::PermissionWait(
            tool_input.get("description").and_then(Value::as_str).map(str::trim).filter(|d| !d.is_empty()).map(String::from),
        ),
        HookKind::Stop => HookEvent::Stop(TurnEnd {
            // Codex's hooks say `default` in plan mode too: whether a turn planned is not known from here.
            planning: false,
            last_message: text("last_assistant_message").map(String::from),
            subagents_working: false,
        }),
    };
    Some(HookInput { session, cwd, event })
}

/// The questions of a `request_user_input` call: each question's text, joined ("" when it has none).
fn questions(input: &Value) -> String {
    let qs: Vec<&str> = input
        .get("questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|q| q.get("question").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()))
        .collect();
    qs.join(" / ")
}

/// What a hook says back (see `Agent::hook_answer`): context as `hookSpecificOutput.additionalContext`, and a stop
/// sent back or a prompt refused as `{"decision": "block", "reason": ...}`.
pub fn hook_answer(reply: &HookReply) -> HookAnswer {
    let context = |event: &str, text: &str| json!({"hookSpecificOutput": {"hookEventName": event, "additionalContext": text}}).to_string();
    let stdout = match reply {
        HookReply::StartContext(text) => context("SessionStart", text),
        HookReply::PromptContext(text) => context("UserPromptSubmit", text),
        HookReply::SendBack(reason) | HookReply::Refuse(reason) => json!({"decision": "block", "reason": reason}).to_string(),
    };
    HookAnswer { stdout, stderr: String::new(), code: 0 }
}

// ---- a one-shot call (platoon's own) ---------------------------------------------------------------------------------

/// Make `cmd` (Codex's program) a one-shot call of `model` for `req`: `codex exec` printing its events as JSON lines,
/// keeping no session (`--ephemeral`), without the user's `config.toml` (`--ignore-user-config`: no MCP servers, no
/// hooks of theirs; the sign-in is still Codex's own), the system prompt as `developer_instructions`, and the prompt on
/// stdin (`-`: a prompt can be far longer than one argument may be, and nothing the user typed shows in the list of
/// processes). The answer follows `schema_file` when the request gives a JSON Schema (Codex reads it from a file,
/// which the caller writes and removes). Outside a git repository Codex refuses to run unless told it may
/// (`--skip-git-repo-check`): platoon's own work folder is one, and so may be a task's folder.
///
/// Codex can't switch its tools off: `Tools::None` and `Tools::ReadOnly` both run in its read-only sandbox (`-s
/// read-only`), where it can still read files and run commands that change nothing, but can't write or reach the
/// network.
pub fn one_shot(cmd: &mut Command, model: &str, req: &Request, cwd: &Path, schema_file: Option<&Path>) {
    cmd.args(["exec", "--json", "--ephemeral", "--ignore-user-config", "-s", "read-only", "-m", model]);
    cmd.arg("-c").arg(format!("model_reasoning_effort={}", toml_string(req.effort)));
    cmd.arg("-c").arg(format!("developer_instructions={}", toml_string(&req.system)));
    if let Some(file) = schema_file {
        cmd.arg("--output-schema").arg(file);
    }
    if !in_git(cwd) {
        cmd.arg("--skip-git-repo-check");
    }
    cmd.arg("-");
}

/// `text` as a TOML string, as `-c key=value` reads a value: quoted, so it is always text, whatever it holds.
fn toml_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Is `dir` inside a git repository (a `.git` folder, or a worktree's `.git` file, in it or above it)?
fn in_git(dir: &Path) -> bool {
    dir.ancestors().any(|d| d.join(".git").exists())
}

/// What came out of a run of `codex exec --json`, understood. Pure: the runner feeds it the bytes. One JSON event per
/// line: the answer is the text of the last `agent_message` item (with a schema, that text is the JSON), the usage is
/// `turn.completed`'s; `turn.failed` and `error` events say what went wrong. `wants_json`: the request gave a schema,
/// so an answer that isn't JSON is a failure.
pub fn interpret(stdout: &[u8], stderr: &[u8], exited_ok: bool, wants_json: bool) -> Outcome {
    let out = String::from_utf8_lossy(stdout);
    let err = String::from_utf8_lossy(stderr);
    let (mut answer, mut usage, mut finished, mut problems) = (None, None, false, Vec::new());
    for line in out.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match v.get("type").and_then(Value::as_str) {
            Some("item.completed") if v["item"]["type"] == "agent_message" => {
                answer = v["item"]["text"].as_str().map(String::from);
            }
            Some("turn.completed") => {
                finished = true;
                usage = v.get("usage").filter(|u| u.is_object()).map(usage_of);
            }
            Some("turn.failed") => problems.extend(v["error"]["message"].as_str().map(String::from)),
            Some("error") => problems.extend(v["message"].as_str().map(String::from)),
            _ => {}
        }
    }
    let fail = |kind: Option<FailKind>, detail: String| {
        let said = format!("{} {err}", problems.join(" "));
        let detail = if !problems.is_empty() { cut(&problems.join(" "), 200) } else { detail };
        Err((kind.unwrap_or_else(|| classify(&said)), detail))
    };
    let result = match answer.filter(|a| !a.trim().is_empty()) {
        // An `error` along the way that Codex got over (a dropped connection it retried) doesn't count once the turn
        // has finished.
        _ if !finished || !exited_ok => {
            let detail = match (out.trim().is_empty(), err.trim().is_empty()) {
                (true, true) => "no output".to_string(),
                (_, false) => cut(&err, 200),
                (false, true) => "Codex stopped before it finished its answer".to_string(),
            };
            fail(None, detail)
        }
        None => fail(Some(FailKind::Other), "Codex finished without an answer".into()),
        Some(a) if wants_json && extract_json(&a).is_none() => {
            fail(Some(FailKind::Other), format!("Codex's answer wasn't the JSON asked for: {}", cut(&a, 120)))
        }
        Some(a) => Ok(a),
    };
    Outcome { result, usage, cost_usd: 0.0 }
}

/// `turn.completed`'s usage as platoon counts it. Codex's `input_tokens` is everything sent, its cached part
/// (`cached_input_tokens`, and `cache_write_input_tokens`) included, so those are taken out of `input` and counted
/// apart: the total is every token sent plus every one written (`output_tokens`, its reasoning included). Codex's own
/// instructions are most of a call: about 17,000 to 26,000 tokens even for a tiny question, mostly cached.
fn usage_of(u: &Value) -> Usage {
    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    let (sent, cached, written) = (n("input_tokens"), n("cached_input_tokens"), n("cache_write_input_tokens"));
    Usage {
        input: sent.saturating_sub(cached).saturating_sub(written),
        cache_creation: written,
        cache_read: cached,
        output: n("output_tokens"),
    }
}

fn cut(s: &str, n: usize) -> String {
    let one_line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= n { one_line } else { format!("{}…", one_line.chars().take(n).collect::<String>()) }
}

// ---- models ---------------------------------------------------------------------------------------------------------

/// `gpt-6-luna` -> `GPT-6-Luna`, `gpt-5.6-terra` -> `GPT-5.6-Terra`, as Codex names them: `gpt` in capitals, every
/// other word with a capital first letter, numbers as they are.
pub fn pretty_model(id: &str) -> String {
    id.split('-')
        .map(|w| match w {
            "gpt" => "GPT".to_string(),
            w => {
                let mut c = w.chars();
                c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// One model of Codex's list (`models_cache.json`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedModel {
    pub slug: String,
    /// Its `display_name` (else the slug as a row shows it).
    pub name: String,
    /// In Codex's own picker (`visibility: list`); `hide` ones are not offered.
    pub listed: bool,
    pub window: Option<u64>,
    /// Codex's order: the lower first.
    pub priority: i64,
}

/// Why Codex's list of models couldn't be had.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListProblem {
    /// There is no `models_cache.json` (yet).
    Missing,
    /// It is there but isn't what platoon understands.
    Unreadable,
}

impl ListProblem {
    pub fn plain(&self) -> String {
        match self {
            ListProblem::Missing => "Codex's models aren't listed: Codex keeps its list of them in models_cache.json in its folder, \
                                     and there is none yet. Start Codex once and they show here."
                .into(),
            ListProblem::Unreadable => "Codex's models aren't listed: its list of them (models_cache.json in its folder) couldn't \
                                        be read."
                .into(),
        }
    }
}

/// The file Codex keeps its list of models in, under its folder.
pub fn models_path(root: &Path) -> PathBuf {
    root.join("models_cache.json")
}

/// Codex's list of models, from `models_cache.json` (what `codex debug models` prints; platoon never runs Codex for
/// it): `models[]` with `slug`, `display_name`, `visibility`, `context_window`, `priority`, in Codex's order (by
/// `priority`, the lower first). A model without a slug is skipped. Read again only when the file changes (its time
/// and size), so a settings screen can ask on every frame.
pub fn models(root: &Path) -> Result<Vec<ListedModel>, ListProblem> {
    type Seen = (Option<(std::time::SystemTime, u64)>, Result<Vec<ListedModel>, ListProblem>);
    static CACHE: std::sync::LazyLock<Mutex<HashMap<PathBuf, Seen>>> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
    let path = models_path(root);
    let stamp = fs::metadata(&path).ok().map(|m| (m.modified().unwrap_or(UNIX_EPOCH), m.len()));
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((seen, list)) = cache.get(&path)
        && *seen == stamp
        && stamp.is_some()
    {
        return list.clone();
    }
    let list = match fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ListProblem::Missing),
        Err(_) => Err(ListProblem::Unreadable),
        Ok(text) => parse_models(&text).ok_or(ListProblem::Unreadable),
    };
    cache.insert(path, (stamp, list.clone()));
    list
}

/// `models_cache.json`'s text, read; None when it isn't an object with a `models` list.
fn parse_models(text: &str) -> Option<Vec<ListedModel>> {
    let v: Value = serde_json::from_str(text).ok()?;
    let mut out: Vec<ListedModel> = v
        .get("models")?
        .as_array()?
        .iter()
        .filter_map(|m| {
            let slug = m.get("slug")?.as_str()?.trim();
            (!slug.is_empty() && !slug.chars().any(char::is_whitespace)).then_some(())?;
            let name = m.get("display_name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty());
            Some(ListedModel {
                slug: slug.to_string(),
                name: name.map_or_else(|| pretty_model(slug), |n| crate::smart::clean_text(n, 60)),
                listed: m.get("visibility").and_then(Value::as_str) == Some("list"),
                window: m.get("context_window").and_then(Value::as_u64).filter(|w| *w > 0),
                priority: m.get("priority").and_then(Value::as_i64).unwrap_or(i64::MAX),
            })
        })
        .collect();
    out.sort_by_key(|m| m.priority);
    Some(out)
}

/// The window a conversation's model has: what its rollout says, else the usual one.
pub fn window_size(t: &Transcript) -> u64 {
    t.window.filter(|&w| w > 0).unwrap_or(DEFAULT_WINDOW)
}

// ---- saved conversations ---------------------------------------------------------------------------------------------

/// The conversation id in a rollout's file name: `rollout-2026-10-18T20-07-40-<id>.jsonl`, the id a UUID.
pub fn id_of_path(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    if path.extension()? != "jsonl" || !stem.starts_with("rollout-") || stem.len() < "rollout-".len() + 36 {
        return None;
    }
    let id = &stem[stem.len() - 36..];
    let shape = id.split('-').map(str::len).collect::<Vec<_>>() == [8, 4, 4, 4, 12];
    (shape && id.chars().all(|c| c == '-' || c.is_ascii_hexdigit())).then(|| id.to_string())
}

/// The folders under `dir` (for the year, month and day levels of `sessions/`).
fn subdirs(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir).into_iter().flatten().flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect()
}

/// Every saved conversation: `sessions/YYYY/MM/DD/rollout-*.jsonl`. Not the ones Codex archived (`archived_sessions`).
pub fn rollout_paths(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for year in subdirs(&root.join("sessions")) {
        for month in subdirs(&year) {
            for day in subdirs(&month) {
                for f in fs::read_dir(&day).into_iter().flatten().flatten() {
                    let p = f.path();
                    if f.file_type().is_ok_and(|t| t.is_file()) && id_of_path(&p).is_some() {
                        out.push(p);
                    }
                }
            }
        }
    }
    out
}

/// The rollout of conversation `id`.
pub fn find_rollout(root: &Path, id: &str) -> Option<PathBuf> {
    rollout_paths(root).into_iter().find(|p| id_of_path(p).as_deref() == Some(id))
}

/// One record of a rollout, parsed; None for a line that isn't one.
fn record(line: &str) -> Option<Value> {
    serde_json::from_str::<Value>(line).ok().filter(|v| v.get("type").is_some_and(Value::is_string))
}

fn payload_type(v: &Value) -> Option<&str> {
    v.get("payload")?.get("type")?.as_str()
}

/// The folder a conversation was run in: its `session_meta` record's `cwd`.
pub fn rollout_cwd(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    for line in BufReader::new(file.take(HEAD_BYTES)).lines().take(HEAD_LINES).map_while(Result::ok) {
        if !line.contains("\"type\":\"session_meta\"") {
            continue;
        }
        if let Some(cwd) = record(&line).and_then(|v| v["payload"]["cwd"].as_str().filter(|c| !c.is_empty()).map(String::from)) {
            return Some(cwd);
        }
    }
    None
}

/// Read a rollout cheaply, whatever its size: the folder, git branch and first real prompt from the head, and the
/// latest model, context, window and time from a bounded tail (as `claude::read_transcript` does). `prev` is the
/// earlier reading of the same file: what only the head can tell is kept when it isn't found again. The name is not
/// in the rollout (see `names`): `prev`'s is kept.
pub fn read_rollout(path: &Path, prev: Option<&Transcript>) -> Option<Transcript> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut t = prev.cloned().unwrap_or_default();
    if prev.is_none() || t.first_prompt.is_none() || t.cwd.is_none() {
        scan_head(&file, &mut t);
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

fn scan_head(file: &File, t: &mut Transcript) {
    let mut reader = BufReader::new(file.take(HEAD_BYTES));
    let mut line = Vec::new();
    for _ in 0..HEAD_LINES {
        if t.cwd.is_some() && t.first_prompt.is_some() {
            break;
        }
        line.clear();
        if !matches!(reader.read_until(b'\n', &mut line), Ok(n) if n > 0) {
            break;
        }
        let line = String::from_utf8_lossy(&line);
        if t.cwd.is_none() && line.contains("\"type\":\"session_meta\"") {
            if let Some(v) = record(&line) {
                let p = &v["payload"];
                t.cwd = p["cwd"].as_str().filter(|c| !c.is_empty()).map(String::from);
                t.branch = p["git"]["branch"].as_str().filter(|b| !b.is_empty()).map(String::from).or(t.branch.take());
            }
        } else if t.first_prompt.is_none() {
            t.first_prompt = prompt_in_line(&line, TEXT_MAX);
        }
    }
}

/// `lines` run newest first, so the first hit for each fact is the latest one.
fn scan_tail<'a>(lines: impl Iterator<Item = &'a str>, t: &mut Transcript) {
    let (mut ts, mut model, mut usage, mut window) = (false, false, false, false);
    let mut need_prompt = t.first_prompt.is_none();
    for line in lines {
        if ts && model && usage && window && !need_prompt {
            break;
        }
        let Some(v) = record(line) else { continue };
        let kind = v["type"].as_str().unwrap_or_default();
        if !ts
            && matches!(kind, "response_item" | "event_msg")
            && let Some(ms) = v["timestamp"].as_str().and_then(parse_iso_ms)
        {
            t.last_active_ms = Some(ms);
            ts = true;
        }
        match (kind, payload_type(&v)) {
            ("turn_context", _) if !model => {
                if let Some(m) = v["payload"]["model"].as_str().filter(|m| !m.is_empty()) {
                    t.model = Some(m.to_string());
                    model = true;
                }
            }
            ("event_msg", Some("token_count")) if !usage => {
                let info = &v["payload"]["info"];
                if let Some(n) = info["last_token_usage"]["input_tokens"].as_u64() {
                    t.context_tokens = n;
                    usage = true;
                    if let Some(w) = info["model_context_window"].as_u64().filter(|_| !window) {
                        t.window = Some(w);
                        window = true;
                    }
                }
            }
            ("event_msg", Some("task_started")) if !window => {
                if let Some(w) = v["payload"]["model_context_window"].as_u64() {
                    t.window = Some(w);
                    window = true;
                }
            }
            ("response_item", _) if need_prompt => {
                if let Some(p) = user_text(&v, TEXT_MAX) {
                    t.first_prompt = Some(p);
                    need_prompt = false;
                }
            }
            _ => {}
        }
    }
}

/// How many of the rollouts written to last are looked through for the plan's limits.
const LIMITS_FROM: usize = 8;

/// The plan's limits as Codex last reported them: the `rate_limits` of the newest `token_count` record, in the rollout
/// written to last that has one (any rollout: a conversation resumed months later writes to its old file). Each window
/// (`primary`, `secondary`) with its length (`window_minutes`), the percent used and when it starts over; a window
/// that is null is left out. None when no rollout says. Expired windows are the caller's to drop: it knows the time.
/// A rollout is read again only when it changes (its time and size), so this can be asked every second.
pub fn rate_limits(root: &Path) -> Option<Vec<crate::usage::Limit>> {
    type Seen = ((std::time::SystemTime, u64), Option<Vec<crate::usage::Limit>>);
    static CACHE: std::sync::LazyLock<Mutex<HashMap<PathBuf, Seen>>> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
    let mut newest: Vec<(std::time::SystemTime, u64, PathBuf)> = rollout_paths(root)
        .into_iter()
        .filter_map(|p| {
            let m = fs::metadata(&p).ok()?;
            Some((m.modified().ok()?, m.len(), p))
        })
        .collect();
    newest.sort_by_key(|n| std::cmp::Reverse(n.0));
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    for (modified, len, path) in newest.into_iter().take(LIMITS_FROM) {
        let found = match cache.get(&path) {
            Some((stamp, found)) if *stamp == (modified, len) => found.clone(),
            _ => {
                let found = limits_in(&path);
                cache.insert(path, ((modified, len), found.clone()));
                found
            }
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// The newest `rate_limits` in a rollout's tail (see `rate_limits`).
fn limits_in(path: &Path) -> Option<Vec<crate::usage::Limit>> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = text.lines().skip(usize::from(start > 0)).collect();
    lines.into_iter().rev().filter(|l| l.contains("\"rate_limits\"")).find_map(|line| {
        let v = record(line)?;
        let p = &v["payload"];
        (payload_type(&v) == Some("token_count")).then_some(())?;
        let at = v["timestamp"].as_str().and_then(parse_iso_ms).map_or(0, |ms| ms / 1000);
        let windows: Vec<crate::usage::Limit> =
            ["primary", "secondary"].iter().filter_map(|k| window_of(&p["rate_limits"][*k], at)).collect();
        (!windows.is_empty()).then_some(windows)
    })
}

/// One window of `rate_limits`: `used_percent`, `window_minutes`, and `resets_at` (or, from older Codex versions,
/// `resets_in_seconds` after the record's time `at`).
fn window_of(w: &Value, at: u64) -> Option<crate::usage::Limit> {
    let pct = w.get("used_percent")?.as_f64().filter(|p| p.is_finite())?;
    let minutes = w.get("window_minutes")?.as_u64().filter(|m| *m > 0)?;
    let resets_at = w
        .get("resets_at")
        .and_then(Value::as_u64)
        .or_else(|| w.get("resets_in_seconds").and_then(Value::as_u64).map(|s| at + s))
        .unwrap_or(0);
    Some(crate::usage::Limit { minutes, pct: pct.round().clamp(0.0, 100.0) as u8, resets_at })
}

/// Is the conversation in this rollout in the middle of a turn? Some(true) when the newest of `task_started`,
/// `task_complete` and `turn_aborted` near its end is a start, Some(false) when it is an end; None when none of them
/// is there (nothing can be said, so callers must not take it for idle).
pub fn busy(path: &Path) -> Option<bool> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    busy_in(text.lines().skip(usize::from(start > 0)).collect::<Vec<_>>().into_iter().rev())
}

/// `busy` over lines given newest first.
fn busy_in<'a>(lines: impl Iterator<Item = &'a str>) -> Option<bool> {
    for line in lines {
        if !line.contains("\"type\":\"event_msg\"") {
            continue;
        }
        match record(line).as_ref().and_then(payload_type) {
            Some("task_started") => return Some(true),
            Some("task_complete" | "turn_aborted") => return Some(false),
            _ => {}
        }
    }
    None
}

/// The names Codex gave conversations, by id, from `session_index.jsonl`: one line per change, the latest line for
/// an id wins. A line that can't be read is skipped.
pub fn names(root: &Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(text) = fs::read_to_string(root.join("session_index.jsonl")) else { return out };
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let (Some(id), Some(name)) = (v.get("id").and_then(Value::as_str), v.get("thread_name").and_then(Value::as_str)) else {
            continue;
        };
        let name = one_line(name);
        if !id.is_empty() && !name.is_empty() {
            out.insert(id.to_string(), truncate_to(name, TEXT_MAX));
        }
    }
    out
}

/// The file `names` reads: its (modified time, size) tell whether it must be read again.
pub fn names_path(root: &Path) -> PathBuf {
    root.join("session_index.jsonl")
}

// ---- prompts and replies, one line at a time ------------------------------------------------------------------------

/// The text of a user's message record, if it is a real prompt: not one of the blocks Codex adds itself
/// (`<environment_context>`, a hook's `<hook_prompt>`, ...), cut to `cap` characters.
fn user_text(v: &Value, cap: usize) -> Option<String> {
    let p = &v["payload"];
    if v["type"].as_str() != Some("response_item") || p["type"].as_str() != Some("message") || p["role"].as_str() != Some("user") {
        return None;
    }
    let text = one_line(&texts(&p["content"], "input_text").join(" "));
    (!text.is_empty() && !text.starts_with('<')).then(|| truncate_to(text, cap))
}

/// The texts of a message's content blocks of `kind`.
fn texts<'a>(content: &'a Value, kind: &str) -> Vec<&'a str> {
    match content {
        Value::String(s) => vec![s.as_str()],
        Value::Array(a) => a.iter().filter(|b| b["type"].as_str() == Some(kind)).filter_map(|b| b["text"].as_str()).collect(),
        _ => Vec::new(),
    }
}

/// The user's real prompt in one line of a rollout, if it holds one, cut to `cap` characters. A line that isn't a
/// user's message is turned down by a substring test alone.
pub fn prompt_in_line(line: &str, cap: usize) -> Option<String> {
    if !line.contains("\"type\":\"response_item\"") || !line.contains("\"role\":\"user\"") {
        return None;
    }
    user_text(&record(line)?, cap)
}

/// Could this line be something Codex said in words (an assistant message with text)? A substring test only.
pub fn may_be_reply(line: &str) -> bool {
    line.contains("\"type\":\"response_item\"") && line.contains("\"role\":\"assistant\"") && line.contains("\"output_text\"")
}

/// The raw words of an assistant message record.
fn reply_texts(line: &str) -> Option<Vec<String>> {
    let v = record(line)?;
    let p = &v["payload"];
    if v["type"].as_str() != Some("response_item") || p["type"].as_str() != Some("message") || p["role"].as_str() != Some("assistant") {
        return None;
    }
    Some(texts(&p["content"], "output_text").into_iter().map(String::from).collect())
}

/// The words Codex said in this line, as one line cut to `cap` characters. Never tool calls or their output.
pub fn reply_in_line(line: &str, cap: usize) -> Option<String> {
    let text = one_line(&reply_texts(line)?.join(" "));
    (!text.is_empty()).then(|| truncate_to(text, cap))
}

/// The words Codex said in this line as it wrote them, lines kept, for the notes updater (see `tidy_reply`).
pub fn reply_text_in_line(line: &str, cap: usize) -> Option<String> {
    tidy_reply(&reply_texts(line)?.join("\n\n"), cap)
}

// ---- running conversations: the writer locks -------------------------------------------------------------------------

/// Reads which conversations are open right now, and in which process, from Codex's writer locks.
pub struct Reader {
    root: PathBuf,
    /// Rollouts found so far, by conversation id (finding one walks the folders of every day Codex was used).
    found: Mutex<HashMap<String, PathBuf>>,
    #[cfg(target_os = "macos")]
    lsof: Mutex<lsof::Cache>,
}

impl Reader {
    pub fn new() -> Self {
        Self::with_root(codex_root())
    }

    pub fn with_root(root: PathBuf) -> Self {
        Self {
            root,
            found: Mutex::new(HashMap::new()),
            #[cfg(target_os = "macos")]
            lsof: Mutex::new(lsof::Cache::default()),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The rollout of conversation `id`, remembered once found.
    fn rollout(&self, id: &str) -> Option<PathBuf> {
        let mut found = self.found.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = found.get(id).filter(|p| p.exists()) {
            return Some(p.clone());
        }
        let p = find_rollout(&self.root, id)?;
        found.insert(id.to_string(), p.clone());
        Some(p)
    }

    /// The lock files there are: (conversation id, path).
    fn locks(&self) -> Vec<(String, PathBuf)> {
        fs::read_dir(self.root.join("thread-writer-locks"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "lock"))
            .filter_map(|p| Some((p.file_stem()?.to_str()?.to_string(), p.clone())))
            .collect()
    }

    /// The process holding each lock that is held: (pid, conversation id).
    #[cfg(target_os = "linux")]
    fn holders(&self, locks: &[(String, PathBuf)]) -> Vec<(u32, String)> {
        linux::holders(locks)
    }

    #[cfg(target_os = "macos")]
    fn holders(&self, locks: &[(String, PathBuf)]) -> Vec<(u32, String)> {
        let open = self.lsof.lock().unwrap_or_else(|e| e.into_inner()).get(&self.root.join("thread-writer-locks"));
        holders_by_path(locks, &open)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn holders(&self, _locks: &[(String, PathBuf)]) -> Vec<(u32, String)> {
        Vec::new()
    }

    /// Every conversation open right now, with the process holding it, oldest first. The holder can be Codex's
    /// shared background server: it is listed like any other process, and only a platoon host's own child is ever
    /// treated as platoon's (see `App`).
    pub fn live(&self) -> Vec<Live> {
        let locks = self.locks();
        if locks.is_empty() {
            return Vec::new();
        }
        let since = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok());
        let mut out: Vec<Live> = self
            .holders(&locks)
            .into_iter()
            .map(|(pid, id)| {
                let lock = locks.iter().find(|(l, _)| *l == id).map(|(_, p)| p.clone()).unwrap_or_default();
                let rollout = self.rollout(&id);
                Live {
                    agent: super::Agent::Codex,
                    pid,
                    cwd: rollout.as_deref().and_then(rollout_cwd).unwrap_or_default(),
                    busy: rollout.as_deref().and_then(busy) == Some(true),
                    started_at: since(&lock).map_or(0, |d| d.as_millis() as u64),
                    id,
                    name: None,
                    background: None,
                    parked: None,
                }
            })
            .collect();
        out.sort_by_key(|l| (l.started_at, l.pid));
        out
    }
}

/// The holders among the processes with a file open (`open`: (pid, path)), by the lock files' paths.
#[cfg(any(target_os = "macos", test))]
fn holders_by_path(locks: &[(String, PathBuf)], open: &[(u32, PathBuf)]) -> Vec<(u32, String)> {
    let canon = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let mut out: Vec<(u32, String)> = Vec::new();
    for (pid, path) in open {
        let path = canon(path);
        if let Some((id, _)) = locks.iter().find(|(_, l)| canon(l) == path)
            && !out.iter().any(|(p, i)| p == pid && i == id)
        {
            out.push((*pid, id.clone()));
        }
    }
    out
}

/// Linux: the kernel's table of held locks (`/proc/locks`) names the process holding each one, without touching the
/// lock. A line is matched to a lock file by its inode, and the process is checked to have that very file open (an
/// inode number alone could be another file system's; some file systems report another device there).
#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    pub fn holders(locks: &[(String, PathBuf)]) -> Vec<(u32, String)> {
        let Ok(table) = fs::read_to_string("/proc/locks") else { return Vec::new() };
        let by_inode: HashMap<u64, &(String, PathBuf)> = locks.iter().filter_map(|l| Some((fs::metadata(&l.1).ok()?.ino(), l))).collect();
        let mut out: Vec<(u32, String)> = Vec::new();
        for (pid, inode) in super::proc_locks(&table) {
            let Some((id, path)) = by_inode.get(&inode) else { continue };
            let holder = match u32::try_from(pid) {
                Ok(p) if p > 0 && has_open(p, path) => Some(p),
                // A lock tied to an open file rather than a process (`OFDLCK`, pid -1): whoever has the file open.
                _ if pid < 0 => opener(path),
                _ => None,
            };
            if let Some(p) = holder.filter(|p| !out.iter().any(|(q, i)| q == p && i == id)) {
                out.push((p, id.clone()));
            }
        }
        out
    }

    /// Has process `pid` got the file at `path` open?
    fn has_open(pid: u32, path: &Path) -> bool {
        let want = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        fs::read_dir(format!("/proc/{pid}/fd")).into_iter().flatten().flatten().any(|fd| fs::read_link(fd.path()).is_ok_and(|t| t == want))
    }

    /// The first process found with the file at `path` open.
    fn opener(path: &Path) -> Option<u32> {
        fs::read_dir("/proc").ok()?.flatten().filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok()).find(|&pid| has_open(pid, path))
    }
}

/// The held locks in `/proc/locks`: (pid, inode) of each line that holds one (not the `->` lines of a process waiting
/// for it). A line that can't be read is skipped. The pid is -1 for a lock tied to an open file.
#[cfg(any(target_os = "linux", test))]
fn proc_locks(table: &str) -> Vec<(i64, u64)> {
    table
        .lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            // `1: FLOCK  ADVISORY  WRITE 1234 08:02:5678 0 EOF`
            if f.get(1) == Some(&"->") || f.len() < 6 {
                return None;
            }
            let pid = f[4].parse::<i64>().ok()?;
            let inode = f[5].rsplit(':').next()?.parse::<u64>().ok()?;
            Some((pid, inode))
        })
        .collect()
}

/// macOS (and the tests): which processes have files in a folder open, as `lsof` says. The answer is remembered and
/// asked again in the background, so a list refreshed every second never waits for `lsof`.
#[cfg(any(target_os = "macos", test))]
mod lsof {
    use std::{
        path::{Path, PathBuf},
        process::{Command, Stdio},
        sync::mpsc::{Receiver, channel},
        time::{Duration, Instant},
    };

    /// How old an answer may be before it is asked again.
    const FRESH: Duration = Duration::from_secs(2);

    /// (pid, path) of every file open in `dir`, or None when `lsof` couldn't be run.
    pub fn open_in(dir: &Path) -> Option<Vec<(u32, PathBuf)>> {
        let out = Command::new("lsof").args(["-n", "-F", "pn", "+d"]).arg(dir).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        Some(parse(&String::from_utf8_lossy(&out.stdout)))
    }

    /// `lsof -F pn` output: a `p<pid>` line, then an `n<path>` line for each of its open files (other lines skipped).
    pub fn parse(text: &str) -> Vec<(u32, PathBuf)> {
        let mut out = Vec::new();
        let mut pid = None;
        for line in text.lines() {
            match line.split_at_checked(1) {
                Some(("p", p)) => pid = p.parse::<u32>().ok(),
                Some(("n", n)) => {
                    if let Some(p) = pid {
                        out.push((p, PathBuf::from(n)));
                    }
                }
                _ => {}
            }
        }
        out
    }

    #[derive(Default)]
    pub struct Cache {
        at: Option<Instant>,
        open: Vec<(u32, PathBuf)>,
        asking: Option<Receiver<Vec<(u32, PathBuf)>>>,
    }

    impl Cache {
        /// What is open in `dir`: the first time asked and waited for, then the latest answer, asked again in the
        /// background once it is `FRESH` old.
        pub fn get(&mut self, dir: &Path) -> Vec<(u32, PathBuf)> {
            if let Some(rx) = &self.asking
                && let Ok(open) = rx.try_recv()
            {
                (self.open, self.at, self.asking) = (open, Some(Instant::now()), None);
            }
            match self.at {
                None => {
                    self.open = open_in(dir).unwrap_or_default();
                    self.at = Some(Instant::now());
                }
                Some(t) if t.elapsed() >= FRESH && self.asking.is_none() => {
                    let (tx, rx) = channel();
                    let dir = dir.to_path_buf();
                    std::thread::spawn(move || {
                        let _ = tx.send(open_in(&dir).unwrap_or_default());
                    });
                    self.asking = Some(rx);
                }
                _ => {}
            }
            self.open.clone()
        }
    }
}

// ---- MCP servers (for idle stop) ------------------------------------------------------------------------------------

/// One MCP server Codex starts as a process of its own: its name, command and arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

/// Codex's configuration file.
pub fn config_path(root: &Path) -> PathBuf {
    root.join("config.toml")
}

/// The MCP servers in Codex's `config.toml` (`[mcp_servers.<name>]`) that run as a local process: with a `command`,
/// and not switched off (`enabled = false`). A server reached by `url` has no process. A malformed entry is skipped.
/// Err when the text isn't TOML.
pub fn mcp_servers(text: &str) -> Result<Vec<McpServer>, String> {
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.message().to_string())?;
    let mut out = Vec::new();
    for (name, entry) in table.get("mcp_servers").and_then(toml::Value::as_table).into_iter().flatten() {
        let Some(e) = entry.as_table() else { continue };
        if e.get("enabled").and_then(toml::Value::as_bool) == Some(false) {
            continue;
        }
        let Some(command) = e.get("command").and_then(toml::Value::as_str).filter(|c| !c.trim().is_empty()) else { continue };
        let args: Option<Vec<String>> = match e.get("args") {
            None => Some(Vec::new()),
            Some(toml::Value::Array(a)) => a.iter().map(|x| x.as_str().map(String::from)).collect(),
            Some(_) => None,
        };
        let Some(args) = args else { continue };
        out.push(McpServer { name: name.clone(), command: command.to_string(), args });
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/testdata/codex");
    /// A finished turn with a tool call, and a Stop that sent the agent back once.
    const FINISHED: &str = "01a1517c-414d-7902-93d1-efa74830493d";
    /// Interrupted at a permission prompt.
    const INTERRUPTED: &str = "01a15181-5d10-7fc3-8da2-6d27c8100fb6";
    /// In plan mode, interrupted at a question.
    const PLANNING: &str = "01a15181-dc6f-7b73-81cf-c041b14f6432";

    fn data() -> PathBuf {
        PathBuf::from(DATA)
    }

    fn rollout(id: &str) -> PathBuf {
        find_rollout(&data(), id).unwrap()
    }

    fn hook(name: &str) -> String {
        fs::read_to_string(data().join("hooks").join(format!("{name}.json"))).unwrap()
    }

    fn tmp(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-codex-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn each_sample_rollout_reads_as_its_conversation() {
        let t = read_rollout(&rollout(FINISHED), None).unwrap();
        assert_eq!(t.cwd.as_deref(), Some("/work/repo"));
        assert_eq!(t.branch.as_deref(), Some("master"));
        assert_eq!(t.first_prompt.as_deref(), Some("Read notes.txt and tell me the colour. Keep it to one line."));
        assert_eq!(t.model.as_deref(), Some("gpt-6-luna"));
        assert_eq!((t.context_tokens, t.window), (14_144, Some(258_400)), "the last reply's input, and the exact window");
        assert_eq!(t.last_active_ms, parse_iso_ms("2026-10-19T00:07:44.377Z"));
        assert_eq!(t.ai_title, None, "names are not in the rollout");
        assert_eq!(busy(&rollout(FINISHED)), Some(false));

        let t = read_rollout(&rollout(INTERRUPTED), None).unwrap();
        assert_eq!(t.first_prompt.as_deref().map(|p| &p[..32]), Some("Run exactly this command: curl -"));
        assert_eq!((t.context_tokens, t.window), (14_193, Some(258_400)));
        assert_eq!(t.last_active_ms, parse_iso_ms("2026-10-19T00:13:31.599Z"));
        assert_eq!(busy(&rollout(INTERRUPTED)), Some(false), "an interrupted turn has ended");

        let t = read_rollout(&rollout(PLANNING), None).unwrap();
        assert!(t.first_prompt.as_deref().is_some_and(|p| p.starts_with("Plan how to add a second line")));
        assert_eq!((t.model.as_deref(), t.context_tokens), (Some("gpt-6-luna"), 15_635));
        assert_eq!(busy(&rollout(PLANNING)), Some(false));
        assert_eq!(pretty_model("gpt-6-luna"), "GPT-6-Luna");
        assert_eq!(pretty_model("gpt-5.6-terra"), "GPT-5.6-Terra");
        assert_eq!(super::super::Agent::Codex.context_window(&t), 258_400);
        assert_eq!(window_size(&Transcript::default()), DEFAULT_WINDOW);
    }

    #[test]
    fn a_turn_in_progress_is_busy_and_without_any_turn_record_nothing_is_said() {
        let d = tmp("busy");
        let p = d.join("rollout-2026-10-18T20-07-40-01a1517c-414d-7902-93d1-efa74830493d.jsonl");
        let started = r#"{"timestamp":"2026-10-19T00:07:40.187Z","type":"event_msg","payload":{"type":"task_started","model_context_window":258400}}"#;
        let done =
            r#"{"timestamp":"2026-10-19T00:07:44.377Z","type":"event_msg","payload":{"type":"task_complete","last_agent_message":"ok"}}"#;
        let msg = r#"{"timestamp":"2026-10-19T00:07:41.014Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"go"}]}}"#;
        fs::write(&p, [started, msg, done, started, msg].join("\n") + "\n").unwrap();
        assert_eq!(busy(&p), Some(true));
        let t = read_rollout(&p, None).unwrap();
        assert_eq!((t.window, t.context_tokens, t.first_prompt.as_deref()), (Some(258_400), 0, Some("go")), "the window before any reply");
        fs::write(&p, format!("{msg}\n")).unwrap();
        assert_eq!(busy(&p), None);
        assert_eq!(busy(&d.join("missing.jsonl")), None);
    }

    #[test]
    fn an_unknown_or_broken_record_is_skipped_and_the_rest_still_reads() {
        let d = tmp("broken");
        let src = fs::read_to_string(rollout(FINISHED)).unwrap();
        let mut lines: Vec<String> = src.lines().map(String::from).collect();
        lines.insert(1, "{ not json".into());
        lines.insert(
            3,
            r#"{"timestamp":"2026-10-19T00:07:40.500Z","type":"something_new","payload":{"type":"never_seen","x":[1,2]}}"#.into(),
        );
        lines.insert(5, r#"{"type":"response_item","payload":{"type":"message","role":"user","content":42}}"#.into());
        lines.push(r#"{"timestamp":"2026-10-19T00:08:00.000Z","type":"event_msg","payload":{"type":"token_count","info":null}}"#.into());
        lines.push("[1,2,3]".into());
        let p = d.join("rollout-2026-10-18T20-07-40-01a1517c-414d-7902-93d1-efa74830493d.jsonl");
        fs::write(&p, lines.join("\n") + "\n").unwrap();
        let t = read_rollout(&p, None).unwrap();
        assert_eq!(t.first_prompt.as_deref(), Some("Read notes.txt and tell me the colour. Keep it to one line."));
        assert_eq!((t.context_tokens, t.window), (14_144, Some(258_400)), "a token count without figures is passed over");
        assert_eq!(t.last_active_ms, parse_iso_ms("2026-10-19T00:08:00.000Z"));
        assert_eq!(busy(&p), Some(false));
    }

    #[test]
    fn rollouts_are_found_by_their_id_and_nothing_else_counts_as_one() {
        let ids: Vec<String> = rollout_paths(&data()).iter().filter_map(|p| id_of_path(p)).collect();
        assert_eq!(ids.len(), 3);
        for id in [FINISHED, INTERRUPTED, PLANNING] {
            assert!(ids.contains(&id.to_string()), "{id}");
        }
        for not in [
            "rollout-2026.jsonl",
            "notes.jsonl",
            "rollout-2026-10-18T20-07-40-01a1517c-414d-7902-93d1-efa74830493d.json",
            "x-01a1517c-414d-7902-93d1-efa74830493d.jsonl",
        ] {
            assert_eq!(id_of_path(Path::new(not)), None, "{not}");
        }
        assert_eq!(rollout_cwd(&rollout(PLANNING)).as_deref(), Some("/work/repo"));
        assert_eq!(find_rollout(&data(), "01a1517c-0000-0000-0000-000000000000"), None);
    }

    #[test]
    fn names_come_from_the_session_index_the_latest_line_winning() {
        let n = names(&data());
        assert_eq!(n.get(FINISHED).map(String::as_str), Some("Identify colour from notes.txt"));
        assert_eq!(n.len(), 3);
        let d = tmp("names");
        let line = |id: &str, name: &str| json!({"id": id, "thread_name": name, "updated_at": "2026-10-19T00:07:44Z"}).to_string();
        fs::write(
            d.join("session_index.jsonl"),
            [line("a", "First name"), "{ broken".into(), line("b", "Other"), line("a", "Renamed  \n twice"), line("c", "  ")].join("\n"),
        )
        .unwrap();
        let n = names(&d);
        assert_eq!(
            (n.get("a").map(String::as_str), n.get("b").map(String::as_str), n.get("c")),
            (Some("Renamed twice"), Some("Other"), None)
        );
        assert!(names(&d.join("nothing")).is_empty());
    }

    #[test]
    fn prompts_and_replies_are_read_line_by_line_and_codexs_own_blocks_are_not_prompts() {
        let src = fs::read_to_string(rollout(FINISHED)).unwrap();
        let prompts: Vec<String> = src.lines().filter_map(|l| prompt_in_line(l, 300)).collect();
        assert_eq!(prompts, ["Read notes.txt and tell me the colour. Keep it to one line."], "not the environment or the hook's prompt");
        let replies: Vec<String> = src.lines().filter(|l| may_be_reply(l)).filter_map(|l| reply_in_line(l, 200)).collect();
        assert_eq!(replies, ["Teal.", "CHECKED"]);
        assert_eq!(prompt_in_line(src.lines().find(|l| l.contains("Read notes.txt")).unwrap(), 9).as_deref(), Some("Read note"));
        let two = r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"First line.\n\n\n\nSecond."}]}}"#;
        assert_eq!(reply_text_in_line(two, 100).as_deref(), Some("First line.\n\nSecond."));
        assert_eq!(reply_in_line(two, 100).as_deref(), Some("First line. Second."));
        let tool = src.lines().find(|l| l.contains("custom_tool_call")).unwrap();
        assert!(!may_be_reply(tool) && prompt_in_line(tool, 300).is_none());
    }

    #[test]
    fn a_conversation_is_resumed_with_exactly_these_arguments_and_a_hosts_arguments_name_it() {
        let a = start_args(Start::Resume(FINISHED));
        assert_eq!(a, ["resume", FINISHED, "--no-daemon"]);
        assert_eq!(session_of_args(&a).as_deref(), Some(FINISHED));
        assert_eq!(start_args(Start::New(None)), ["--no-daemon"]);
        assert_eq!(session_of_args(&start_args(Start::New(Some("hi")))), None);
        assert_eq!(session_of_args(&["resume".into(), "--last".into()]), None);
    }

    #[test]
    fn each_sample_hook_input_is_read_for_the_hook_it_was_given_to() {
        let start = hook_input(HookKind::SessionStart, &hook("SessionStart")).unwrap();
        assert_eq!((start.session.as_str(), start.cwd.as_str()), (PLANNING, "/work/repo"));
        assert_eq!(start.event, HookEvent::SessionStart { source: "startup".into() });
        let prompt = hook_input(HookKind::Prompt, &hook("UserPromptSubmit")).unwrap();
        assert_eq!((prompt.session.as_str(), prompt.event), (FINISHED, HookEvent::PromptSubmitted));
        for (name, last) in [("Stop-first", "Teal."), ("Stop-after-block", "CHECKED")] {
            let stop = hook_input(HookKind::Stop, &hook(name)).unwrap();
            let end = TurnEnd { planning: false, last_message: Some(last.into()), subagents_working: false };
            assert_eq!(stop.event, HookEvent::Stop(end), "{name}");
        }
        let q = hook_input(HookKind::BeforeTool(Tool::Question), &hook("PreToolUse")).unwrap();
        assert_eq!(q.event, HookEvent::BeforeTool { tool: Tool::Question, said: "What should the second line of notes.txt say?".into() });
        let perm = hook_input(HookKind::PermissionWait, &hook("PermissionRequest")).unwrap();
        assert_eq!(
            perm.event,
            HookEvent::PermissionWait(Some(
                "May I rerun the exact curl command with network access after sandbox DNS resolution failed?".into()
            ))
        );
        assert_eq!(hook_input(HookKind::AfterTool, &hook("PreToolUse")).unwrap().event, HookEvent::AfterTool(Tool::Question));
        for bad in ["", "not json", "{}", r#"{"session_id": 5}"#] {
            assert_eq!(hook_input(HookKind::Prompt, bad), None, "{bad}");
        }
    }

    #[test]
    fn a_hook_is_answered_in_exactly_these_words() {
        let a = hook_answer(&HookReply::StartContext("the notes".into()));
        assert_eq!(a.stdout, r#"{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"the notes"}}"#);
        assert_eq!((a.stderr.as_str(), a.code), ("", 0));
        let a = hook_answer(&HookReply::PromptContext("newer notes".into()));
        assert_eq!(a.stdout, r#"{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"newer notes"}}"#);
        let a = hook_answer(&HookReply::SendBack("Run the tests.".into()));
        assert_eq!((a.stdout.as_str(), a.code), (r#"{"decision":"block","reason":"Run the tests."}"#, 0));
        for (event, args) in HOOKS {
            assert!(args.ends_with("--agent codex") && !args.contains('\''), "{event}: fixed text, the same for every session");
        }
    }

    #[test]
    fn the_mcp_servers_that_run_a_process_are_read_from_config_toml() {
        let text = r#"
model = "gpt-6-luna"
[mcp_servers.docs]
command = "npx"
args = ["-y", "docs-server"]
[mcp_servers.docs.env]
TOKEN = "x"
[mcp_servers.remote]
url = "https://example.invalid/mcp"
[mcp_servers.off]
command = "off-server"
enabled = false
[mcp_servers.odd]
command = "odd"
args = "not a list"
[mcp_servers.plain]
command = "plain-server"
[projects."/work/repo"]
trust_level = "trusted"
"#;
        let got = mcp_servers(text).unwrap();
        assert_eq!(
            got,
            [
                McpServer { name: "docs".into(), command: "npx".into(), args: vec!["-y".into(), "docs-server".into()] },
                McpServer { name: "plain".into(), command: "plain-server".into(), args: vec![] },
            ]
        );
        assert!(mcp_servers("model = ").is_err());
        assert_eq!(mcp_servers("").unwrap(), []);
    }

    #[test]
    fn the_kernels_lock_table_is_read_for_the_holders_and_not_the_waiters() {
        let table = "1: FLOCK  ADVISORY  WRITE 4242 08:02:131074 0 EOF\n\
                     1: -> FLOCK  ADVISORY  WRITE 999 08:02:131074 0 EOF\n\
                     2: OFDLCK ADVISORY  READ  -1 00:1b:77 0 EOF\n\
                     3: POSIX  ADVISORY  WRITE 17 fd:01:5 0 EOF\n\
                     garbage line\n";
        assert_eq!(proc_locks(table), [(4242, 131_074), (-1, 77), (17, 5)]);
    }

    #[test]
    fn lsofs_answer_is_read_by_process_and_matched_to_the_lock_files() {
        let out = "p100\nfcwd\nn/tmp/x/a.lock\nf5\nn/tmp/x/b.lock\np200\nf3\nn/tmp/x/b.lock\nnnot-a-field\n";
        let open = lsof::parse(out);
        assert_eq!(open[0], (100, PathBuf::from("/tmp/x/a.lock")));
        assert_eq!(open.len(), 4);
        let locks = vec![("a".to_string(), PathBuf::from("/tmp/x/a.lock")), ("b".to_string(), PathBuf::from("/tmp/x/b.lock"))];
        assert_eq!(holders_by_path(&locks, &open[..3]), [(100, "a".to_string()), (100, "b".to_string()), (200, "b".to_string())]);
    }

    /// A process of the test's own (`flock` holding a temp lock file, as Codex holds its writer lock) is found as the
    /// holder of that conversation; with the lock let go, nothing is.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_process_holding_a_writer_lock_is_found_and_none_once_it_is_free() {
        use std::process::{Command, Stdio};
        let root = tmp("locks");
        fs::create_dir_all(root.join("thread-writer-locks")).unwrap();
        let lock = root.join("thread-writer-locks").join(format!("{FINISHED}.lock"));
        fs::write(&lock, "").unwrap();
        fs::write(root.join("thread-writer-locks").join(format!("{PLANNING}.lock")), "").unwrap();
        let day = root.join("sessions/2026/10/18");
        fs::create_dir_all(&day).unwrap();
        fs::copy(rollout(FINISHED), day.join(rollout(FINISHED).file_name().unwrap())).unwrap();
        let reader = Reader::with_root(root.clone());
        assert!(reader.live().is_empty(), "lock files that nobody holds");
        let mut child = Command::new("flock").arg(&lock).args(["sleep", "30"]).stdin(Stdio::null()).spawn().unwrap();
        let start = std::time::Instant::now();
        let live = loop {
            let live = reader.live();
            if !live.is_empty() || start.elapsed() > std::time::Duration::from_secs(10) {
                break live;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        // `flock` takes the lock and then runs `sleep` with it held: the lock's holder is `flock` itself.
        assert_eq!(live.len(), 1, "{live:?}");
        let l = &live[0];
        assert_eq!((l.pid, l.id.as_str(), l.cwd.as_str(), l.busy), (child.id(), FINISHED, "/work/repo", false));
        assert_eq!(l.agent, super::super::Agent::Codex);
        let _ = child.kill();
        let _ = child.wait();
        assert!(reader.live().is_empty(), "free again");
    }

    /// The same through `lsof`, as macOS reads it (run here too when `lsof` is installed).
    #[test]
    fn lsof_finds_the_process_with_a_lock_file_open() {
        use std::process::{Command, Stdio};
        let dir = tmp("lsof");
        let lock = dir.join("x.lock");
        fs::write(&lock, "").unwrap();
        let Some(_) = lsof::open_in(&dir) else { return }; // no lsof here
        let mut child = Command::new("sh").arg("-c").arg("exec 3<\"$0\"; exec sleep 30").arg(&lock).stdin(Stdio::null()).spawn().unwrap();
        let start = std::time::Instant::now();
        let mut cache = lsof::Cache::default();
        let found = loop {
            let open = lsof::open_in(&dir).unwrap_or_default();
            let h = holders_by_path(&[("x".into(), lock.clone())], &open);
            if !h.is_empty() || start.elapsed() > std::time::Duration::from_secs(10) {
                break h;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        assert_eq!(found, [(child.id(), "x".to_string())]);
        assert!(cache.get(&dir).iter().any(|(p, _)| *p == child.id()), "the first answer is waited for");
        let _ = child.kill();
        let _ = child.wait();
    }

    // ---- a one-shot call ----

    fn args(cmd: &Command) -> Vec<String> {
        cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn a_one_shot_call_is_codex_exec_read_only_ephemeral_without_the_users_config_with_the_prompt_on_stdin() {
        let mut req = Request::new("names", "Be brief.\nSay \"ok\" \\ nothing else.", "the prompt");
        req.effort = "low";
        let outside = tmp("exec-outside");
        let mut cmd = Command::new("codex");
        one_shot(&mut cmd, "gpt-6-luna", &req, &outside, None);
        assert_eq!(
            args(&cmd),
            [
                "exec",
                "--json",
                "--ephemeral",
                "--ignore-user-config",
                "-s",
                "read-only",
                "-m",
                "gpt-6-luna",
                "-c",
                "model_reasoning_effort=\"low\"",
                "-c",
                "developer_instructions=\"Be brief.\\nSay \\\"ok\\\" \\\\ nothing else.\"",
                "--skip-git-repo-check",
                "-"
            ]
        );
        assert!(!args(&cmd).iter().any(|a| a.contains("the prompt")), "the prompt goes on stdin");
        // In a git repository (a worktree's `.git` is a file), with a schema: no skipping the check, the schema's file.
        let repo = tmp("exec-repo");
        fs::write(repo.join(".git"), "gitdir: /elsewhere\n").unwrap();
        fs::create_dir_all(repo.join("src")).unwrap();
        req.json_schema = Some("{}".into());
        req.effort = "medium";
        let mut cmd = Command::new("codex");
        one_shot(&mut cmd, "gpt-5.6-terra", &req, &repo.join("src"), Some(Path::new("/work/schema-1.json")));
        let a = args(&cmd);
        assert!(a.windows(2).any(|w| w == ["--output-schema", "/work/schema-1.json"]), "{a:?}");
        assert!(a.contains(&"model_reasoning_effort=\"medium\"".to_string()) && !a.contains(&"--skip-git-repo-check".to_string()), "{a:?}");
        assert_eq!(a.last().map(String::as_str), Some("-"));
    }

    #[test]
    fn a_text_value_is_always_a_toml_string_whatever_it_holds() {
        assert_eq!(toml_string("plain"), "\"plain\"");
        assert_eq!(toml_string("42"), "\"42\"", "never a number");
        assert_eq!(toml_string("a\tb\rc\u{1b}d"), "\"a\\tb\\rc\\u001Bd\"");
        let parsed: toml::Value = toml::from_str(&format!("v = {}", toml_string("q\"uote\\back\nline é"))).unwrap();
        assert_eq!(parsed["v"].as_str(), Some("q\"uote\\back\nline é"));
    }

    fn sample_exec() -> Vec<u8> {
        fs::read(data().join("exec/exec-sample.jsonl")).unwrap()
    }

    #[test]
    fn the_answer_is_the_last_agent_message_and_the_usage_turn_completed_s() {
        // A real `codex exec --json --output-schema` run (Codex 0.163): a message, a command, the JSON answer.
        let o = interpret(&sample_exec(), b"Reading additional input from stdin...\n", true, true);
        assert_eq!(o.result, Ok("{\"colour\":\"teal\",\"files_read\":1}".to_string()));
        // 26,038 sent, 12,800 of it cached: counted apart, nothing lost.
        assert_eq!(o.usage, Some(Usage { input: 13_238, cache_creation: 0, cache_read: 12_800, output: 76 }));
        assert_eq!(o.usage.unwrap().total(), 26_038 + 76);
        assert_eq!(o.cost_usd, 0.0, "Codex says no cost");
        // Without a schema any words are an answer.
        let plain = b"{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"ok\"}}\n{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":5}}\n";
        assert_eq!(interpret(plain, b"", true, false).result, Ok("ok".to_string()));
    }

    #[test]
    fn every_way_a_codex_call_goes_wrong_is_a_failure_said_plainly_and_what_it_used_still_counts() {
        let done = r#"{"type":"turn.completed","usage":{"input_tokens":20000,"cached_input_tokens":15000,"output_tokens":9}}"#;
        let said = |t: &str| format!(r#"{{"type":"item.completed","item":{{"id":"i","type":"agent_message","text":{}}}}}"#, json!(t));
        let lines = |l: &[&str]| l.join("\n").into_bytes();
        // Nothing at all.
        let o = interpret(b"", b"", true, true);
        assert_eq!(o.result, Err((FailKind::Other, "no output".into())));
        assert_eq!(o.usage, None);
        // It stopped before the turn finished.
        let o = interpret(&lines(&[r#"{"type":"thread.started"}"#, r#"{"type":"turn.started"}"#, &said("I'll look")]), b"", false, true);
        assert_eq!(o.result, Err((FailKind::Other, "Codex stopped before it finished its answer".into())));
        // A failed turn says why, and the kind is read from it; a non-zero exit with what it printed on stderr too.
        let o = interpret(
            &lines(&[r#"{"type":"turn.failed","error":{"message":"You've hit your usage limit. Try again later."}}"#]),
            b"",
            false,
            true,
        );
        assert_eq!(o.result, Err((FailKind::RateLimited, "You've hit your usage limit. Try again later.".into())));
        let o = interpret(b"", b"Error: Not logged in. Run codex login.\n", false, true);
        assert_eq!(o.result.unwrap_err().0, FailKind::NotSignedIn);
        // A finished turn with no words, or words that aren't the JSON asked for.
        let o = interpret(&lines(&[done]), b"", true, true);
        assert_eq!(o.result, Err((FailKind::Other, "Codex finished without an answer".into())));
        assert_eq!(o.usage.map(|u| u.total()), Some(20_009), "what it used counts even so");
        let o = interpret(&lines(&[&said("I couldn't decide."), done]), b"", true, true);
        assert_eq!(o.result, Err((FailKind::Other, "Codex's answer wasn't the JSON asked for: I couldn't decide.".into())));
        // A finished turn that exited non-zero is not an answer either.
        let o = interpret(&lines(&[&said("{}"), done]), b"something broke", false, true);
        assert_eq!(o.result, Err((FailKind::Other, "something broke".into())));
        // A dropped connection Codex got over doesn't spoil a turn that finished.
        let o = interpret(&lines(&[r#"{"type":"error","message":"Reconnecting... 1/5"}"#, &said("{\"a\":1}"), done]), b"", true, true);
        assert_eq!(o.result, Ok("{\"a\":1}".into()));
        // Garbage lines are skipped.
        let o = interpret(&lines(&["not json", &said("[1]"), "{oops", done]), b"", true, true);
        assert_eq!(o.result, Ok("[1]".into()));
    }

    // ---- the plan's limits ----

    /// A rollout holding one `token_count` record with these `rate_limits` windows, last written `age` seconds ago
    /// (so the newest file is the one written to last).
    pub(crate) fn limits_rollout(root: &Path, id: &str, primary: Value, secondary: Value, age: u64) -> PathBuf {
        let dir = root.join("sessions/2026/10/18");
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join(format!("rollout-2026-10-18T20-07-40-{id}.jsonl"));
        let lines = [
            json!({"timestamp":"2026-10-18T20:07:40.000Z","type":"session_meta","payload":{"id":id,"cwd":"/work"}}),
            json!({"timestamp":"2026-10-18T20:08:00.000Z","type":"event_msg","payload":{"type":"token_count","info":null,
                "rate_limits":{"limit_id":"codex","primary":primary,"secondary":secondary,"plan_type":"plus"}}}),
        ];
        fs::write(&p, lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n").unwrap();
        let at = std::time::SystemTime::now() - std::time::Duration::from_secs(age);
        File::options().write(true).open(&p).unwrap().set_modified(at).unwrap();
        p
    }

    #[test]
    fn the_plans_limits_come_from_the_newest_token_count_of_the_rollout_written_to_last() {
        use crate::usage::Limit;
        // The samples: a free plan, one 30-day window, nothing used, no second window.
        assert_eq!(rate_limits(&data()), Some(vec![Limit { minutes: 43_200, pct: 0, resets_at: 1_794_959_888 }]));
        let root = tmp("limits");
        assert_eq!(rate_limits(&root), None, "no rollout: nothing said");
        let a = "01a1517c-0000-7000-8000-00000000000a";
        let b = "01a1517c-0000-7000-8000-00000000000b";
        let w = |pct: f64, minutes: u64, resets: u64| json!({"used_percent": pct, "window_minutes": minutes, "resets_at": resets});
        limits_rollout(&root, a, w(12.4, 300, 5_000), w(40.6, 10_080, 9_000), 60);
        assert_eq!(
            rate_limits(&root),
            Some(vec![Limit { minutes: 300, pct: 12, resets_at: 5_000 }, Limit { minutes: 10_080, pct: 41, resets_at: 9_000 }])
        );
        // Another conversation written to since: its figures are the newest.
        limits_rollout(&root, b, w(80.0, 43_200, 7_000), Value::Null, 5);
        assert_eq!(rate_limits(&root), Some(vec![Limit { minutes: 43_200, pct: 80, resets_at: 7_000 }]));
        // A rollout written to last with no figure of its own leaves the one before it; an older Codex's
        // `resets_in_seconds` counts from the record's time.
        let p = limits_rollout(&root, b, json!({"used_percent": 3, "window_minutes": 60, "resets_in_seconds": 100}), Value::Null, 1);
        let at = parse_iso_ms("2026-10-18T20:08:00.000Z").unwrap() / 1000;
        assert_eq!(rate_limits(&root), Some(vec![Limit { minutes: 60, pct: 3, resets_at: at + 100 }]));
        fs::write(&p, "{\"type\":\"session_meta\",\"payload\":{}}\n").unwrap();
        assert_eq!(rate_limits(&root).map(|l| l[0].minutes), Some(300), "the older rollout's figures");
    }

    #[test]
    fn a_window_reads_as_its_length_in_plain_words() {
        use crate::usage::Limit;
        let l = |minutes| Limit { minutes, pct: 0, resets_at: 0 };
        assert_eq!([l(300).short(), l(10_080).short(), l(43_200).short(), l(90).short()], ["5h", "7d", "30d", "90m"]);
        assert_eq!(
            [l(300).name(), l(10_080).name(), l(43_200).name(), l(1440).name(), l(90).name()],
            ["5-hour limit", "weekly limit", "30-day limit", "1-day limit", "90-minute limit"]
        );
    }
}
