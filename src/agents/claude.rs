//! Adapter over Claude Code's on-disk state. Everything here reads undocumented internal
//! formats (`~/.claude/sessions/<pid>.json` and `~/.claude/projects/*/<id>.jsonl`), so all
//! knowledge of them lives in this file.

use super::{
    Agent, Helper, Hidden, HookAnswer, HookEvent, HookInput, HookKind, HookReply, Live, SessionLog, Spent, Start, TaskLaunch, Tool, Tools,
    Transcript, TurnEnd, one_line, parse_iso_ms, tidy_reply, truncate_to,
};
use crate::ai::{Outcome, Request, classify, extract_json};
use crate::ledger::{FailKind, Usage};
use crate::procs::pid_alive;
use serde_json::{Value, json};
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::Command,
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

// ---- starting a session ---------------------------------------------------------------------------------------------

/// The program that is Claude Code here: `PLATOON_CLAUDE_BIN` (a wrapper, or a test's stand-in), else `claude`.
pub fn program() -> String {
    std::env::var("PLATOON_CLAUDE_BIN").unwrap_or_else(|_| "claude".into())
}

/// Is Claude Code on this computer: `PLATOON_CLAUDE_BIN` names it, or `claude` is on `path` (a `PATH` value)?
pub fn installed(path: &str) -> bool {
    std::env::var_os("PLATOON_CLAUDE_BIN").is_some_and(|p| !p.is_empty()) || super::on_path("claude", path)
}

/// The arguments `claude` is started with for `start`: nothing for a new session (or its first message alone),
/// `--resume <id>`, or `attach <id>`.
pub fn start_args(start: Start) -> Vec<String> {
    match start {
        Start::New(first) => first.into_iter().map(String::from).collect(),
        Start::Resume(id) => vec!["--resume".into(), id.into()],
        Start::Attach(id) => attach_args(id).into(),
    }
}

/// Do these arguments show a background session (`claude attach <id>`)?
pub fn attaches(args: &[String]) -> bool {
    args.first().is_some_and(|a| a == "attach")
}

/// The session id a host's arguments name: the value after `--session-id` (a new session platoon chose the id of)
/// or `--resume`.
pub fn session_of_args(args: &[String]) -> Option<String> {
    args.windows(2).find(|w| w[0] == "--session-id" || w[0] == "--resume").map(|w| w[1].clone()).filter(|s| !s.is_empty())
}

// ---- hooks -----------------------------------------------------------------------------------------------------------
//
// Claude Code calls each hook with one JSON object on stdin (`session_id`, `cwd`, and the event's own fields), and
// reads its answer from stdout (JSON) or, for a refused prompt, from stderr with exit code 2.

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
        HookKind::BeforeTool(tool) => HookEvent::BeforeTool { tool, said: tool_said(tool, tool_input) },
        HookKind::BeforeAnyTool => {
            let tool = match text("tool_name").unwrap_or_default() {
                "ExitPlanMode" => Tool::Plan,
                "AskUserQuestion" => Tool::Question,
                "Bash" => Tool::Shell,
                "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => Tool::Edit,
                _ => Tool::Other,
            };
            HookEvent::BeforeTool { tool, said: tool_said(tool, tool_input) }
        }
        HookKind::AfterTool => HookEvent::AfterTool(match text("tool_name").unwrap_or_default() {
            "ExitPlanMode" => Tool::Plan,
            "AskUserQuestion" => Tool::Question,
            _ => Tool::Other,
        }),
        HookKind::PermissionWait if is_permission_notice(&v) => HookEvent::PermissionWait(text("message").map(String::from)),
        HookKind::PermissionWait => return None,
        HookKind::Stop => HookEvent::Stop(TurnEnd {
            planning: text("permission_mode") == Some("plan"),
            last_message: text("last_assistant_message").map(String::from),
            subagents_working: still_working(v.get("background_tasks")),
        }),
    };
    Some(HookInput { session, cwd, event })
}

/// What a tool's input says for platoon: the plan it presents, the question it asks, the command it runs.
fn tool_said(tool: Tool, input: &Value) -> String {
    let field = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    match tool {
        Tool::Plan => field("plan"),
        Tool::Question => questions(input),
        Tool::Shell => field("command"),
        Tool::Edit | Tool::Other => String::new(),
    }
}

/// What a hook says back (see `Agent::hook_answer`): context as `hookSpecificOutput.additionalContext`, a stop sent
/// back as `{"decision": "block", "reason": ...}`, a refused prompt as its reason on stderr with exit code 2, and a
/// refused tool as `permissionDecision: deny` with its reason.
pub fn hook_answer(reply: &HookReply) -> HookAnswer {
    let context = |event: &str, text: &str| json!({"hookSpecificOutput": {"hookEventName": event, "additionalContext": text}}).to_string();
    let out = |stdout: String| HookAnswer { stdout, stderr: String::new(), code: 0 };
    match reply {
        HookReply::StartContext(text) => out(context("SessionStart", text)),
        HookReply::PromptContext(text) => out(context("UserPromptSubmit", text)),
        HookReply::SendBack(reason) => out(json!({"decision": "block", "reason": reason}).to_string()),
        HookReply::Refuse(why) => HookAnswer { stdout: String::new(), stderr: why.clone(), code: 2 },
        HookReply::Deny(why) => out(super::deny_json(why)),
    }
}

/// The questions of an `AskUserQuestion` call: each question's text, joined ("" when it has none).
fn questions(input: &Value) -> String {
    let qs: Vec<String> = input
        .get("questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|q| q.get("question").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(String::from))
        .collect();
    qs.join(" / ")
}

/// Does this `Notification` input say Claude waits for the user's permission? `notification_type`
/// "permission_prompt" says so, else a `message` that mentions permission ("Claude needs your permission to use Bash").
fn is_permission_notice(v: &Value) -> bool {
    if let Some(kind) = v.get("notification_type").and_then(Value::as_str) {
        return kind == "permission_prompt";
    }
    v.get("message").and_then(Value::as_str).is_some_and(|m| m.to_lowercase().contains("permission"))
}

/// Claude Code's `background_tasks`: anything in it means subagents are still at work, so the turn's end isn't the
/// end of the work.
fn still_working(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        Some(Value::Number(n)) => n.as_u64().is_some_and(|n| n > 0),
        Some(Value::Bool(b)) => *b,
        _ => false,
    }
}

// ---- a one-shot call (`claude -p`) ---------------------------------------------------------------------------------

/// The arguments and environment of a one-shot call of `model` for `req`: `claude -p` with the user's normal login,
/// no settings, no saved session, no slash commands, no MCP servers, the caller's system prompt and effort, the
/// answer as JSON (`--output-format json`), following `req.json_schema` when it gives one. The prompt goes in on
/// stdin. Never `--bare`: it needs an API key, which subscribers don't have.
pub fn one_shot(cmd: &mut Command, model: &str, req: &Request) {
    let tools = match req.tools {
        Tools::None => "",
        Tools::ReadOnly => "Read,Grep,Glob",
    };
    cmd.args(["-p", "--model", model, "--tools", tools, "--setting-sources", ""])
        .args(["--no-session-persistence", "--disable-slash-commands", "--strict-mcp-config"])
        .args(["--system-prompt", req.system.as_str(), "--effort", req.effort, "--output-format", "json"]);
    if let Some(schema) = &req.json_schema {
        cmd.args(["--json-schema", schema.as_str()]);
    }
    if req.effort == "low" {
        // No hidden "thinking": with it on, a naming call spent 3 to 4 times more output tokens than its JSON needs.
        cmd.env("MAX_THINKING_TOKENS", "0");
    }
}

/// What came out of a run of `claude -p`, understood. Pure: the runner feeds it the bytes. With `--json-schema` the
/// answer is `structured_output`, else `result`; the usage is `usage`, the cost `total_cost_usd`.
pub fn interpret(stdout: &[u8], stderr: &[u8], exited_ok: bool) -> Outcome {
    let out = String::from_utf8_lossy(stdout);
    let err = String::from_utf8_lossy(stderr);
    let mut value = extract_json(&out);
    // Older Claude Code versions print a list of events; the answer is the one of type "result".
    if let Some(Value::Array(events)) = &value {
        value = events.iter().rev().find(|e| e.get("type").and_then(Value::as_str) == Some("result")).or(events.last()).cloned();
    }
    let Some(v) = value.filter(Value::is_object) else {
        let both = format!("{out} {err}");
        let detail = cut(both.trim(), 200);
        let kind = classify(&both);
        let detail = if detail.is_empty() { "no output".to_string() } else { detail };
        return Outcome { result: Err((kind, detail)), usage: None, cost_usd: 0.0 };
    };
    let number = |o: &Value, k: &str| o.get(k).and_then(Value::as_u64).unwrap_or(0);
    let usage = v.get("usage").filter(|u| u.is_object()).map(|u| Usage {
        input: number(u, "input_tokens"),
        cache_creation: number(u, "cache_creation_input_tokens"),
        cache_read: number(u, "cache_read_input_tokens"),
        output: number(u, "output_tokens"),
    });
    let cost_usd = v.get("total_cost_usd").and_then(Value::as_f64).filter(|c| c.is_finite() && *c >= 0.0).unwrap_or(0.0);
    // With `--json-schema` the answer is `structured_output`; `result` may then be empty or a sentence about it.
    let structured = v.get("structured_output").filter(|s| !s.is_null()).map(Value::to_string);
    let text = structured.as_deref().or_else(|| v.get("result").and_then(Value::as_str));
    let is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
    let result = match text {
        Some(t) if !is_error && exited_ok && !t.trim().is_empty() => Ok(t.to_string()),
        _ => {
            let both = format!("{} {}", text.unwrap_or(""), err);
            let mut kind = classify(&both);
            if kind == FailKind::Other
                && v.get("subtype").and_then(Value::as_str).is_some_and(|s| s.contains("rate") || s.contains("limit"))
            {
                kind = FailKind::RateLimited;
            }
            let detail = match (text.filter(|t| !t.trim().is_empty()), exited_ok) {
                (Some(t), _) => cut(t, 200),
                (None, _) if !err.trim().is_empty() => cut(&err, 200),
                (None, true) if !is_error => "Claude sent back an empty answer".to_string(),
                _ => "Claude reported an error".to_string(),
            };
            Err((kind, detail))
        }
    };
    Outcome { result, usage, cost_usd }
}

fn cut(s: &str, n: usize) -> String {
    let one_line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= n { one_line } else { format!("{}…", one_line.chars().take(n).collect::<String>()) }
}

// ---- a task's session ------------------------------------------------------------------------------------------------
//
// A task runs as one ordinary interactive `claude` session (`task_args`), started with:
// - `--session-id` (platoon picks it, so the task names it before it starts) and `-n "#N title"`;
// - `--model`, the squad's model, and `--permission-mode plan` for a squad that plans first, else `acceptEdits`;
// - `--settings`, the task's hooks for this session only (the user's own settings stay as they are), with
//   `permissions.ask` rules so pushing, opening or merging pull requests and publishing wait at Claude Code's own
//   permission prompt (`ASK`);
// - `--append-system-prompt-file <task folder>/squad.md`: platoon's fixed rules and the squad's steps;
// - `--add-dir <task folder>`, so the session can read those files;
// - the task's text as its first message.

/// Commands that publish: they wait at Claude Code's permission prompt (`permissions.ask`), whatever mode the session
/// is in. Claude Code matches a rule against each part of a compound command (`cd x && git push` asks too) and past a
/// leading `VAR=value`, and a `*` may stand anywhere, so the forms of `git push` with options before `push` (`git -C
/// <dir> push`, `git -c k=v push`, `--git-dir`, `--work-tree`) have rules of their own. Not caught (Claude Code's own
/// list of what a rule doesn't match): another spelling of the program (`/usr/bin/git push`, `git 'push'`), a shell
/// that runs it (`sh -c '...'`), and a script or tool that pushes for you. A commit whose message has " push " in it
/// after `git -C <dir>` asks too.
pub const ASK: &[&str] = &[
    "Bash(git push:*)",
    "Bash(git -C * push)",
    "Bash(git -C * push *)",
    "Bash(git -c * push)",
    "Bash(git -c * push *)",
    "Bash(git --git-dir* push)",
    "Bash(git --git-dir* push *)",
    "Bash(git --work-tree* push)",
    "Bash(git --work-tree* push *)",
    "Bash(gh pr create:*)",
    "Bash(gh pr merge:*)",
    "Bash(gh release:*)",
    "Bash(gh api -X *)",
    "Bash(gh api --method *)",
    "Bash(gh api -f *)",
    "Bash(gh api -F *)",
    "Bash(gh api --field *)",
    "Bash(gh api --raw-field *)",
    "Bash(gh api --input *)",
    "Bash(gh api * -X *)",
    "Bash(gh api * --method *)",
    "Bash(gh api * -f *)",
    "Bash(gh api * -F *)",
    "Bash(gh api * --field *)",
    "Bash(gh api * --raw-field *)",
    "Bash(gh api * --input *)",
    "Bash(glab:*)",
    "Bash(cargo publish:*)",
    "Bash(npm publish:*)",
];

/// How much longer than the check of the work may take the `Stop` hook is given before Claude Code gives up on it, in
/// seconds.
const STOP_MARGIN: u64 = 60;

/// A program path as one word for a shell (the hook commands run through one).
pub fn shell_word(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+=:@%,".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// How often a task's status line is run again besides Claude Code's own updates, in seconds (Claude Code's
/// `statusLine.refreshInterval`, at least 1): the step, the helpers and the check's timer stay current.
pub const STATUS_EVERY: u64 = 2;

/// The `--settings` JSON of a task's session: its hooks, calling the platoon `exe`, and the `ask` rules; platoon's own
/// status line (the task's line: `platoon statusline`), run again every `STATUS_EVERY` seconds, in place of the user's
/// in this session only; and `exe task step` allowed without a permission prompt (the rules have the session run it
/// at each step, written as here). The `Stop` hook waits as long as the check of the work may take, and a little more,
/// and Claude Code shows while it runs that `checker` (the check's model, by name) is checking the work.
pub fn settings_json(exe: &str, checker: &str) -> String {
    let exe = shell_word(exe);
    let cmd = |name: &str| json!({"type": "command", "command": format!("{exe} hook {name}")});
    let mut stop = cmd("task-stop");
    if let Some(o) = stop.as_object_mut() {
        o.insert("timeout".into(), json!(crate::verdict::TIMEOUT.as_secs() + STOP_MARGIN));
        o.insert("statusMessage".into(), json!(format!("{checker} is checking the work against \"Done when\"…")));
    }
    json!({
        "permissions": {"ask": ASK, "allow": [format!("Bash({exe} task step:*)")]},
        "statusLine": {"type": "command", "command": format!("{exe} statusline"), "refreshInterval": STATUS_EVERY},
        "hooks": {
            "PreToolUse": [
                {"matcher": "ExitPlanMode", "hooks": [cmd("task-plan")]},
                {"matcher": "AskUserQuestion", "hooks": [cmd("task-question")]},
            ],
            "Notification": [{"hooks": [cmd("task-notify")]}],
            "PostToolUse": [{"matcher": "*", "hooks": [cmd("task-tool")]}],
            "UserPromptSubmit": [{"hooks": [cmd("task-prompt")]}],
            "Stop": [{"hooks": [stop]}],
        }
    })
    .to_string()
}

/// The arguments `claude` gets for a task's session. `--add-dir` comes first, so it can't take the first message for
/// a folder.
pub fn task_args(l: &TaskLaunch) -> Vec<String> {
    let mut a: Vec<String> = vec!["--add-dir".into(), l.task_dir.to_string_lossy().into_owned()];
    if l.resume {
        a.extend(["--resume".into(), l.session.clone()]);
    } else {
        a.extend(["--session-id".into(), l.session.clone(), "-n".into(), l.name.clone()]);
    }
    a.extend([
        "--model".into(),
        l.model.clone(),
        "--permission-mode".into(),
        if l.plan { "plan" } else { "acceptEdits" }.into(),
        "--settings".into(),
        settings_json(&l.exe, &l.checker),
        "--append-system-prompt-file".into(),
        l.task_dir.join(crate::taskrun::SQUAD_FILE).to_string_lossy().into_owned(),
    ]);
    if let (false, Some(m)) = (l.resume, &l.first) {
        a.push(m.clone());
    }
    a
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

/// Since when Claude Code's registry has said process `pid` is idle (`statusUpdatedAt`, ms since the epoch): None unless
/// its status is `idle` (see `registry_status`) and it says when that was.
pub fn idle_since(root: &Path, pid: u32) -> Option<u64> {
    if registry_status(root, pid)?.as_str() != "idle" {
        return None;
    }
    let v: Value = serde_json::from_str(&fs::read_to_string(root.join("sessions").join(format!("{pid}.json"))).ok()?).ok()?;
    v.get("statusUpdatedAt")?.as_u64()
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
        agent: Agent::Claude,
        pid,
        id,
        cwd: v["cwd"].as_str().unwrap_or("").to_string(),
        name,
        started_at: v["startedAt"].as_u64().unwrap_or(0),
        // A parked entry keeps the `busy` it had when it handed the conversation over, for good.
        busy: v["status"].as_str() == Some("busy") && parked.is_none(),
        background,
        parked,
        unsaved: false,
    })
}

// ---- models ---------------------------------------------------------------------------------------------------------

/// Claude Code's model aliases: each always means the latest model of its family, so they need no list to be read.
pub const OPUS: &str = "opus";
pub const SONNET: &str = "sonnet";
pub const HAIKU: &str = "haiku";
pub const FABLE: &str = "fable";

/// Claude Code's models in the catalog, in the order the squads' settings go round them.
pub const MODELS: [&str; 4] = [OPUS, SONNET, HAIKU, FABLE];

/// An alias as the user sees it (`opus` -> `Opus`); anything else as it is shown in a row (`pretty_model`).
pub fn alias_name(id: &str) -> String {
    match id {
        OPUS => "Opus".into(),
        SONNET => "Sonnet".into(),
        HAIKU => "Haiku".into(),
        FABLE => "Fable".into(),
        other => pretty_model(other),
    }
}

/// `claude-sonnet-5` -> `Sonnet 5`, `claude-fable-5-1` -> `Fable 5.1`, `claude-haiku-4-5-20251001` -> `Haiku 4.5`.
/// Handles the older `claude-3-5-sonnet-...` ordering too. Anything unrecognized is shown as-is.
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

/// Does Claude Code give this model the 1M-token window without `[1m]` in its id: Opus 4.7 and newer, Sonnet and
/// Haiku 5.5 and newer, every Fable and Mythos? Ids as any provider writes them: `claude-opus-5-5`,
/// `us.anthropic.claude-opus-4-7-v1:0`, `claude-opus-4-7@20260416`.
fn large_by_default(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    let Some((_, rest)) = m.split_once("claude-") else { return false };
    let rest = rest.split(['@', '[', ':']).next().unwrap_or_default();
    let mut family = None;
    let mut version = Vec::new();
    for t in rest.split('-') {
        if t.len() <= 2 && !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()) {
            version.push(t.parse::<u32>().unwrap_or(0));
        } else if family.is_none() && t.chars().all(|c| c.is_ascii_alphabetic()) {
            family = Some(t);
        }
    }
    let v = (version.first().copied().unwrap_or(0), version.get(1).copied().unwrap_or(0));
    match family {
        Some("fable" | "mythos") => true,
        Some("opus") => v >= (4, 7),
        Some("sonnet" | "haiku") => v >= (5, 5),
        _ => false,
    }
}

/// The transcript doesn't record the model's window size, so infer it: 1M when Claude Code gives the model 1M
/// (`large_by_default`, unless `large_off`: `large_window_off`), when the model id names the 1M window, or once the
/// session is seen past 200k tokens (for good: compaction shrinks usage but not the window); else 200k.
pub fn window_size(t: &Transcript, large_off: bool) -> u64 {
    let large = t.large_window || t.context_tokens > DEFAULT_WINDOW || !large_off && t.model.as_deref().is_some_and(large_by_default);
    if large { LARGE_WINDOW } else { DEFAULT_WINDOW }
}

/// Is Claude Code's 1M window switched off (`CLAUDE_CODE_DISABLE_1M_CONTEXT`), in `env` (its value in platoon's own
/// environment, which the sessions platoon starts inherit) or in the `env` of Claude Code's settings in `root`?
pub fn large_window_off(root: &Path, env: Option<&str>) -> bool {
    let truthy = |v: &str| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on");
    let settings = fs::read_to_string(root.join("settings.json")).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok());
    let in_settings = settings
        .as_ref()
        .and_then(|v| v.get("env")?.get("CLAUDE_CODE_DISABLE_1M_CONTEXT").cloned())
        .is_some_and(|v| v.as_str().map_or(v.as_u64() == Some(1) || v.as_bool() == Some(true), truthy));
    env.is_some_and(truthy) || in_settings
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

/// Every saved conversation's transcript: `projects/*/<id>.jsonl`, the top level of each project folder only. Its
/// subfolders hold tool results, memory and subagent transcripts, none of which are conversations.
pub fn transcript_paths(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in fs::read_dir(root.join("projects")).into_iter().flatten().flatten() {
        if !dir.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        for f in fs::read_dir(dir.path()).into_iter().flatten().flatten() {
            let path = f.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") && f.file_type().is_ok_and(|t| t.is_file()) {
                out.push(path);
            }
        }
    }
    out
}

pub fn find_transcript(root: &Path, id: &str) -> Option<PathBuf> {
    fs::read_dir(root.join("projects")).ok()?.flatten().map(|d| d.path().join(format!("{id}.jsonl"))).find(|p| p.exists())
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
    Some(SessionLog { spent: spent(path).unwrap_or_default(), last_at: Some(last_at), helpers: helpers(path) })
}

/// The helpers (subagents) of the session whose transcript is `transcript`: Claude Code writes one
/// `<session folder>/subagents/agent-<id>.meta.json` for each (Claude Code 2.1.310: `{"agentType", "description",
/// "model", "finished", ...}`; older ones have no `model` or `finished`), its transcript `agent-<id>.jsonl` beside it.
/// Undocumented, so read defensively: a file that isn't JSON, or has no description, is left out, a key that is
/// missing or of another kind reads as not said. `at` is the latest change of either file. Small files only: cheap
/// enough for a status line.
pub fn helpers(transcript: &Path) -> Vec<Helper> {
    let secs = |p: &Path| {
        fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs())
    };
    let dir = transcript.with_extension("").join("subagents");
    let mut out = Vec::new();
    for e in fs::read_dir(&dir).into_iter().flatten().flatten() {
        let path = e.path();
        let Some(id) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".meta.json")) else { continue };
        let Ok(meta) = fs::metadata(&path) else { continue };
        if !meta.is_file() || meta.len() > 64 * 1024 {
            continue;
        }
        let Some(v) = fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { continue };
        let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(String::from);
        let Some(description) = text("description") else { continue };
        let at = secs(&path).unwrap_or(0).max(secs(&dir.join(format!("{id}.jsonl"))).unwrap_or(0));
        out.push(Helper {
            description: crate::projects::clean(&description, 200),
            model: text("model").map(|m| crate::projects::clean(&m, 80)),
            kind: text("agentType").map(|k| crate::projects::clean(&k, 60)).unwrap_or_default(),
            finished: v.get("finished").and_then(Value::as_bool).unwrap_or(false),
            at,
        });
    }
    out
}

/// How much of the end of a transcript `to_tickets_tail` reads: only recent turns matter to the status line's hint,
/// and the status line runs often.
pub const TO_TICKETS_TAIL_BYTES: u64 = 256 * 1024;

/// What the end of a conversation says about the `/hand-to-squad` skill, for the status line's hint (`ticketshint.rs`).
/// Times are seconds since the epoch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ToTicketsTail {
    /// The latest plan the user approved: an `ExitPlanMode` call whose result is not an error (a rejected plan's is).
    pub plan_approved: Option<u64>,
    /// The latest use of the skill, `/hand-to-squad` or by an old name (`/to-tasks`, `/to-tickets`): typed by the user,
    /// or Claude running it with the Skill tool.
    pub to_tickets_used: Option<u64>,
}

/// Read the last `TO_TICKETS_TAIL_BYTES` of a transcript for `ToTicketsTail`. Claude Code 2.1.312 writes a plan as an
/// assistant entry with a `tool_use` block named `ExitPlanMode` and its answer as a user entry with a `tool_result`
/// for that block's id (`"is_error": true` when the user said no); a typed skill as a user entry of tags (see
/// `skill_typed`); the Skill tool as a `tool_use` named `Skill` with `{"skill": "hand-to-squad"}` (or an old name,
/// `"to-tasks"` or `"to-tickets"`, in conversations from before the rename or where setup hasn't run since).
/// Subagents' entries don't count. A line is parsed only when a substring test says it may matter. An entry without a
/// time takes the file's. Anything unreadable: nothing found.
pub fn to_tickets_tail(path: &Path) -> ToTicketsTail {
    // The skill's names, without the slash: today's and its old ones (`skill::OLD_COMMANDS`).
    let names: Vec<&str> =
        std::iter::once(crate::skill::COMMAND).chain(crate::skill::OLD_COMMANDS).map(|c| c.trim_start_matches('/')).collect();
    let mut out = ToTicketsTail::default();
    let Ok(mut file) = File::open(path) else { return out };
    let Ok(meta) = file.metadata() else { return out };
    let start = meta.len().saturating_sub(TO_TICKETS_TAIL_BYTES);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return out;
    }
    let mut bytes = Vec::new();
    if file.take(TO_TICKETS_TAIL_BYTES).read_to_end(&mut bytes).is_err() {
        return out;
    }
    let written = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.split('\n');
    if start > 0 {
        lines.next(); // the first line was cut by the seek
    }
    let mut plans: Vec<String> = Vec::new();
    for line in lines {
        let plan_call = line.contains("\"ExitPlanMode\"");
        let answer = !plans.is_empty() && line.contains("\"tool_result\"") && plans.iter().any(|id| line.contains(id.as_str()));
        let skill = names.iter().any(|n| line.contains(n));
        if !(plan_call || answer || skill) || line.contains("\"isSidechain\":true") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        let at = v["timestamp"].as_str().and_then(parse_iso_ms).map_or(written, |ms| ms / 1000);
        let blocks = v["message"]["content"].as_array().map(Vec::as_slice).unwrap_or_default();
        match v["type"].as_str() {
            Some("assistant") => {
                for b in blocks.iter().filter(|b| b["type"].as_str() == Some("tool_use")) {
                    match b["name"].as_str() {
                        Some("ExitPlanMode") => plans.extend(b["id"].as_str().filter(|id| !id.is_empty()).map(String::from)),
                        Some("Skill") if b["input"]["skill"].as_str().is_some_and(|s| names.contains(&s.trim())) => {
                            out.to_tickets_used = Some(at);
                        }
                        _ => {}
                    }
                }
            }
            Some("user") => {
                for b in blocks.iter().filter(|b| b["type"].as_str() == Some("tool_result")) {
                    if b["tool_use_id"].as_str().is_some_and(|id| plans.iter().any(|p| p == id)) && b["is_error"].as_bool() != Some(true) {
                        out.plan_approved = Some(at);
                    }
                }
                if skill
                    && let Some(p) = real_prompt(&v)
                    && names
                        .iter()
                        .any(|n| p.strip_prefix('/').and_then(|p| p.strip_prefix(n)).is_some_and(|r| r.is_empty() || r.starts_with(' ')))
                {
                    out.to_tickets_used = Some(at);
                }
            }
            _ => {}
        }
    }
    out
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

/// The text of a real user prompt: not a built-in command's echo, caveat, compaction summary, interrupt marker
/// or other injected `<...>` block. A skill the user typed (`/land desc-cut`) is a prompt: see `skill_typed`.
fn real_prompt(v: &Value) -> Option<String> {
    real_prompt_capped(v, TEXT_MAX)
}

/// `real_prompt`, keeping at most `cap` characters.
fn real_prompt_capped(v: &Value, cap: usize) -> Option<String> {
    let flag = |k: &str| v[k].as_bool() == Some(true);
    if v["type"].as_str() != Some("user") || flag("isMeta") || flag("isSidechain") || flag("isCompactSummary") {
        return None;
    }
    let raw = match &v["message"]["content"] {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join(" "),
        _ => return None,
    };
    let text = skill_typed(&raw).unwrap_or_else(|| one_line(&raw));
    (!text.is_empty() && !text.starts_with('<') && !text.starts_with("[Request interrupted")).then(|| truncate_to(text, cap))
}

/// What the user typed to run a skill, as they typed it: `/name args` on one line (`/sharpen` without words).
///
/// Claude Code records every slash command as a user entry made of tags, written by one of two code paths. A command
/// whose text goes to the model (a skill, a custom command, a built-in prompt such as `/init`) starts with its
/// `<command-message>`, and the skill's own text follows as an `isMeta` entry:
/// `<command-message>land</command-message>\n<command-name>/land</command-name>\n<command-args>x</command-args>`
/// (older versions: `land is running…` in the first tag; no `<command-args>` without words). A command Claude Code
/// runs itself (`/model`, `/plugin`, `/add-dir`, `/clear`) starts with its `<command-name>`, and its
/// `<local-command-stdout>` follows; it is no message to anyone, so it gives None here and stays out, as every
/// `<...>` text does. The first tag tells them apart on the entry itself, so a reader going line by line (the
/// index, the prompt store, `prompt_in_line`) needs no look at the entry after it.
fn skill_typed(raw: &str) -> Option<String> {
    let raw = raw.trim_start();
    if !raw.starts_with("<command-message>") {
        return None;
    }
    let after = |open: &str| raw.find(open).map(|at| &raw[at + open.len()..]);
    let name = after("<command-name>").and_then(|r| r.find("</command-name>").map(|end| one_line(&r[..end])))?;
    if name.is_empty() || name.contains('<') {
        return None;
    }
    let args = after("<command-args>").and_then(|r| r.rfind("</command-args>").map(|end| one_line(&r[..end]))).unwrap_or_default();
    let slash = if name.starts_with('/') { "" } else { "/" };
    Some(one_line(&format!("{slash}{name} {args}")))
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
    tidy_reply(&raw, cap)
}

fn truncate(s: String) -> String {
    truncate_to(s, TEXT_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sessions_helpers_are_read_from_its_subagents_meta_files_and_odd_ones_are_left_out_or_read_as_not_said() {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-claude-helpers-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        let transcript = d.join("projects/-w/s-1.jsonl");
        let subs = d.join("projects/-w/s-1/subagents");
        fs::create_dir_all(&subs).unwrap();
        fs::write(&transcript, "").unwrap();
        assert!(helpers(&d.join("projects/-w/none.jsonl")).is_empty(), "no folder: none");
        // Claude Code 2.1.310's shape, finished and not; an older one without model or finished; and odd files.
        let write = |name: &str, text: &str| fs::write(subs.join(name), text).unwrap();
        write(
            "agent-a1.meta.json",
            r#"{"agentType":"general-purpose","description":"Reviewer D on the diff","toolUseId":"toolu_1","spawnDepth":1,"requestShape":"background","requestNonInteractive":true,"model":"haiku","finished":true}"#,
        );
        write("agent-a1.jsonl", "{}\n");
        write(
            "agent-b2.meta.json",
            r#"{"agentType":"general-purpose","description":"Reviewer A on the diff","model":"opus","finished":false}"#,
        );
        write("agent-c3.meta.json", r#"{"agentType":"Explore","description":"Find the index","toolUseId":"toolu_3"}"#);
        write("agent-d4.meta.json", "not json");
        write("agent-e5.meta.json", r#"{"agentType":"general-purpose","model":"opus"}"#);
        write("agent-f6.meta.json", r#"{"description":"Odd kinds","model":7,"finished":"yes","agentType":["x"]}"#);
        write("notes.txt", "not a helper");
        let mut got = helpers(&transcript);
        got.sort_by(|a, b| a.description.cmp(&b.description));
        let now = crate::usage::now();
        assert!(got.iter().all(|h| now.saturating_sub(h.at) < 600), "{got:?}");
        let seen: Vec<(&str, Option<&str>, &str, bool)> =
            got.iter().map(|h| (h.description.as_str(), h.model.as_deref(), h.kind.as_str(), h.finished)).collect();
        assert_eq!(
            seen,
            [
                ("Find the index", None, "Explore", false),
                ("Odd kinds", None, "", false),
                ("Reviewer A on the diff", Some("opus"), "general-purpose", false),
                ("Reviewer D on the diff", Some("haiku"), "general-purpose", true),
            ]
        );
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
        let mut v = serde_json::json!({"pid":pid,"sessionId":"feedf00d-9999-4aaa-8bbb-cccccccccccc","cwd":"/work","kind":kind,"startedAt":7,"status":"busy"});
        for (k, x) in extra.as_object().unwrap() {
            v.as_object_mut().unwrap().insert(k.clone(), x.clone());
        }
        v
    }

    #[test]
    fn a_background_entry_is_a_live_process_told_apart_from_an_interactive_one() {
        let me = std::process::id();
        let bg = live_from(&entry("bg", me, serde_json::json!({"jobId":"feedf00d"}))).unwrap();
        assert_eq!(
            (bg.pid, bg.id.as_str(), bg.cwd.as_str(), bg.started_at, bg.busy),
            (me, "feedf00d-9999-4aaa-8bbb-cccccccccccc", "/work", 7, true)
        );
        assert_eq!(bg.background.as_deref(), Some("feedf00d"));
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
        assert_eq!(plain.background.as_deref(), Some("feedf00d"));
        let empty = live_from(&entry("bg", me, serde_json::json!({"jobId":""}))).unwrap();
        assert_eq!(empty.background.as_deref(), Some("feedf00d"));
        assert_eq!(short_id("xyz"), "xyz");
        assert_eq!(attach_args("feedf00d"), ["attach", "feedf00d"]);
    }

    #[test]
    fn a_session_is_started_resumed_and_attached_to_with_exactly_these_arguments() {
        let v = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(start_args(Start::New(None)), v(&[]));
        assert_eq!(start_args(Start::New(Some("Carry on from here"))), v(&["Carry on from here"]));
        assert_eq!(start_args(Start::Resume("feedf00d-9999")), v(&["--resume", "feedf00d-9999"]));
        assert_eq!(start_args(Start::Attach("abc12345")), v(&["attach", "abc12345"]));
        assert!(attaches(&start_args(Start::Attach("abc12345"))));
        assert!(!attaches(&start_args(Start::Resume("x"))) && !attaches(&[]));
        assert_eq!(session_of_args(&start_args(Start::Resume("feedf00d-9999"))).as_deref(), Some("feedf00d-9999"));
        assert_eq!(session_of_args(&start_args(Start::New(Some("--resume")))), None);
    }

    #[test]
    fn a_hooks_input_is_read_for_the_kind_of_hook_it_was_given_to() {
        let read = |kind: HookKind, v: Value| hook_input(kind, &v.to_string());
        let start = read(HookKind::SessionStart, json!({"session_id": "s1", "cwd": "/w", "source": "compact"})).unwrap();
        assert_eq!((start.session.as_str(), start.cwd.as_str()), ("s1", "/w"));
        assert_eq!(start.event, HookEvent::SessionStart { source: "compact".into() });
        let bare = read(HookKind::SessionStart, json!({"session_id": "s1"})).unwrap();
        assert_eq!((bare.cwd.as_str(), bare.event), ("", HookEvent::SessionStart { source: "startup".into() }));
        assert_eq!(read(HookKind::Prompt, json!({"session_id": "s1", "prompt": "hi"})).unwrap().event, HookEvent::PromptSubmitted);
        let plan = json!({"session_id": "s1", "tool_name": "ExitPlanMode", "tool_input": {"plan": "1. Do it"}});
        assert_eq!(
            read(HookKind::BeforeTool(Tool::Plan), plan).unwrap().event,
            HookEvent::BeforeTool { tool: Tool::Plan, said: "1. Do it".into() }
        );
        let q = json!({"session_id": "s1", "tool_input": {"questions": [{"question": " Which page? "}, {"question": ""}, {"question": "Red?"}]}});
        assert_eq!(
            read(HookKind::BeforeTool(Tool::Question), q).unwrap().event,
            HookEvent::BeforeTool { tool: Tool::Question, said: "Which page? / Red?".into() }
        );
        let none = read(HookKind::BeforeTool(Tool::Question), json!({"session_id": "s1"})).unwrap();
        assert_eq!(none.event, HookEvent::BeforeTool { tool: Tool::Question, said: String::new() });
        for (name, tool) in [("ExitPlanMode", Tool::Plan), ("AskUserQuestion", Tool::Question), ("Bash", Tool::Other)] {
            assert_eq!(
                read(HookKind::AfterTool, json!({"session_id": "s1", "tool_name": name})).unwrap().event,
                HookEvent::AfterTool(tool)
            );
        }
        let asks =
            json!({"session_id": "s1", "notification_type": "permission_prompt", "message": "Claude needs your permission to use Bash"});
        assert_eq!(
            read(HookKind::PermissionWait, asks).unwrap().event,
            HookEvent::PermissionWait(Some("Claude needs your permission to use Bash".into()))
        );
        let worded = json!({"session_id": "s1", "message": "Needs Permission"});
        assert_eq!(read(HookKind::PermissionWait, worded).unwrap().event, HookEvent::PermissionWait(Some("Needs Permission".into())));
        assert_eq!(read(HookKind::PermissionWait, json!({"session_id": "s1", "notification_type": "idle_prompt"})), None);
        let end = json!({"session_id": "s1", "permission_mode": "plan", "last_assistant_message": "Red or blue?", "background_tasks": [{"id": "a"}]});
        assert_eq!(
            read(HookKind::Stop, end).unwrap().event,
            HookEvent::Stop(TurnEnd { planning: true, last_message: Some("Red or blue?".into()), subagents_working: true })
        );
        let plain = read(HookKind::Stop, json!({"session_id": "s1", "background_tasks": []})).unwrap();
        assert_eq!(plain.event, HookEvent::Stop(TurnEnd { planning: false, last_message: None, subagents_working: false }));
        for bad in ["", "{", "[]", "null", r#"{"cwd":"/x"}"#, r#"{"session_id":5}"#] {
            assert_eq!(hook_input(HookKind::Prompt, bad), None, "{bad}");
        }
    }

    #[test]
    fn a_hook_is_answered_in_exactly_the_words_claude_code_reads() {
        let said = |r: HookReply| hook_answer(&r);
        assert_eq!(
            said(HookReply::StartContext("the notes".into())).stdout,
            r#"{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"the notes"}}"#
        );
        assert_eq!(
            said(HookReply::PromptContext("the notes".into())).stdout,
            r#"{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"the notes"}}"#
        );
        let back = said(HookReply::SendBack("Fix it".into()));
        assert_eq!((back.stdout.as_str(), back.stderr.as_str(), back.code), (r#"{"decision":"block","reason":"Fix it"}"#, "", 0));
        let refused = said(HookReply::Refuse("Exit this copy".into()));
        assert_eq!((refused.stdout.as_str(), refused.stderr.as_str(), refused.code), ("", "Exit this copy", 2));
    }

    /// A Claude Code task's `--settings`, byte for byte as platoon 1.0 wrote it (taken from a running task's arguments):
    /// Codex's tasks share its `ASK` table and change nothing of it.
    const SETTINGS_BEFORE: &str = r##"{"permissions":{"ask":["Bash(git push:*)","Bash(git -C * push)","Bash(git -C * push *)","Bash(git -c * push)","Bash(git -c * push *)","Bash(git --git-dir* push)","Bash(git --git-dir* push *)","Bash(git --work-tree* push)","Bash(git --work-tree* push *)","Bash(gh pr create:*)","Bash(gh pr merge:*)","Bash(gh release:*)","Bash(gh api -X *)","Bash(gh api --method *)","Bash(gh api -f *)","Bash(gh api -F *)","Bash(gh api --field *)","Bash(gh api --raw-field *)","Bash(gh api --input *)","Bash(gh api * -X *)","Bash(gh api * --method *)","Bash(gh api * -f *)","Bash(gh api * -F *)","Bash(gh api * --field *)","Bash(gh api * --raw-field *)","Bash(gh api * --input *)","Bash(glab:*)","Bash(cargo publish:*)","Bash(npm publish:*)"]},"hooks":{"PreToolUse":[{"matcher":"ExitPlanMode","hooks":[{"type":"command","command":"/opt/platoon/bin/platoon hook task-plan"}]},{"matcher":"AskUserQuestion","hooks":[{"type":"command","command":"/opt/platoon/bin/platoon hook task-question"}]}],"Notification":[{"hooks":[{"type":"command","command":"/opt/platoon/bin/platoon hook task-notify"}]}],"PostToolUse":[{"matcher":"*","hooks":[{"type":"command","command":"/opt/platoon/bin/platoon hook task-tool"}]}],"UserPromptSubmit":[{"hooks":[{"type":"command","command":"/opt/platoon/bin/platoon hook task-prompt"}]}],"Stop":[{"hooks":[{"type":"command","command":"/opt/platoon/bin/platoon hook task-stop","timeout":660}]}]}}"##;

    #[test]
    fn a_claude_tasks_settings_keep_what_they_had_and_add_platoons_status_line_the_step_command_and_the_checks_words() {
        let mut now: Value = serde_json::from_str(&settings_json("/opt/platoon/bin/platoon", "Fable")).unwrap();
        // The task's status line: platoon's, run again every 2 seconds.
        assert_eq!(now["statusLine"], json!({"type": "command", "command": "/opt/platoon/bin/platoon statusline", "refreshInterval": 2}));
        // `task step` by platoon's path, as the rules write it, needs no permission prompt.
        assert_eq!(now["permissions"]["allow"], json!(["Bash(/opt/platoon/bin/platoon task step:*)"]));
        // While the stop's check runs, Claude Code says who checks.
        let stop = &now["hooks"]["Stop"][0]["hooks"][0];
        assert_eq!(stop["statusMessage"], "Fable is checking the work against \"Done when\"…");
        // Everything else is byte for byte what it was.
        if let Some(o) = now.as_object_mut() {
            o.remove("statusLine");
        }
        if let Some(p) = now.get_mut("permissions").and_then(Value::as_object_mut) {
            p.remove("allow");
        }
        if let Some(h) = now.pointer_mut("/hooks/Stop/0/hooks/0").and_then(Value::as_object_mut) {
            h.remove("statusMessage");
        }
        assert_eq!(now.to_string(), SETTINGS_BEFORE);
        // A path with a space: the rule is written as the command is, quoted.
        let quoted: Value = serde_json::from_str(&settings_json("/opt/my tools/platoon", "GPT-6-Luna")).unwrap();
        assert_eq!(quoted["permissions"]["allow"], json!(["Bash('/opt/my tools/platoon' task step:*)"]));
        assert_eq!(quoted["statusLine"]["command"], "'/opt/my tools/platoon' statusline");
    }

    #[test]
    fn a_tasks_session_gets_exactly_these_arguments_in_this_order() {
        let l = TaskLaunch {
            session: "abc".into(),
            resume: false,
            name: "#5 Login".into(),
            model: "opus".into(),
            plan: true,
            task_dir: PathBuf::from("/c/tasks/05-work"),
            exe: "/opt/platoon/bin/platoon".into(),
            first: Some("Task #5: Login\n\nDo it.".into()),
            instructions: "the rules".into(),
            tag: "5:p1".into(),
            checker: "Fable".into(),
        };
        let settings = settings_json("/opt/platoon/bin/platoon", "Fable");
        let v = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(
            task_args(&l),
            v(&[
                "--add-dir",
                "/c/tasks/05-work",
                "--session-id",
                "abc",
                "-n",
                "#5 Login",
                "--model",
                "opus",
                "--permission-mode",
                "plan",
                "--settings",
                &settings,
                "--append-system-prompt-file",
                "/c/tasks/05-work/squad.md",
                "Task #5: Login\n\nDo it.",
            ])
        );
        let again = TaskLaunch { resume: true, plan: false, ..l };
        assert_eq!(
            task_args(&again),
            v(&[
                "--add-dir",
                "/c/tasks/05-work",
                "--resume",
                "abc",
                "--model",
                "opus",
                "--permission-mode",
                "acceptEdits",
                "--settings",
                &settings,
                "--append-system-prompt-file",
                "/c/tasks/05-work/squad.md",
            ])
        );
        assert_eq!(session_of_args(&task_args(&again)).as_deref(), Some("abc"));
    }

    #[test]
    fn a_stale_background_entry_or_one_of_the_services_idle_spares_or_another_kind_is_not_live() {
        let me = std::process::id();
        assert!(live_from(&entry("bg", dead_pid(), serde_json::json!({}))).is_none(), "its process has gone");
        assert!(live_from(&entry("interactive", dead_pid(), serde_json::json!({}))).is_none());
        // Claude keeps pre-started processes waiting for work; they hold no conversation.
        assert!(live_from(&entry("bg", me, serde_json::json!({"spare":true,"jobId":"5ca1ab1e"}))).is_none());
        assert!(live_from(&entry("bg", me, serde_json::json!({"spare":false}))).is_some());
        assert!(live_from(&entry("daemon", me, serde_json::json!({}))).is_none());
        assert!(live_from(&serde_json::json!({"pid":me,"kind":"bg"})).is_none(), "no session id");
    }

    #[test]
    fn an_interactive_entry_that_handed_its_conversation_to_a_background_job_is_parked_and_never_busy() {
        let me = std::process::id();
        // The shape seen in Claude Code 2.1.283: the status stays "busy" from before the hand-over, for good.
        let parked = live_from(&entry("interactive", me, serde_json::json!({"parkedJobId":"beefcafe","statusUpdatedAt":1}))).unwrap();
        assert_eq!(parked.parked.as_deref(), Some("beefcafe"));
        assert!(parked.is_parked() && !parked.busy && !parked.is_background());
        assert_eq!((parked.pid, parked.id.as_str(), parked.cwd.as_str()), (me, "feedf00d-9999-4aaa-8bbb-cccccccccccc", "/work"));

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
        let bg = live_from(&entry("bg", me, serde_json::json!({"jobId":"beefcafe","parkedJobId":"zzz"}))).unwrap();
        assert!(bg.parked.is_none() && bg.busy && bg.is_background());
        // A parked entry whose process has gone is gone like any other.
        assert!(live_from(&entry("interactive", dead_pid(), serde_json::json!({"parkedJobId":"beefcafe"}))).is_none());
    }

    #[test]
    fn the_registry_status_of_a_parked_process_is_parked_never_busy() {
        let dir = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-claude-parked-{}", std::process::id())));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sessions")).unwrap();
        let put = |pid: u32, v: Value| fs::write(dir.join("sessions").join(format!("{pid}.json")), v.to_string()).unwrap();
        put(11, entry("interactive", 11, serde_json::json!({"parkedJobId":"beefcafe"})));
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
            [("other", false), ("feedf00d-9999-4aaa-8bbb-cccccccccccc", true)]
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
        let at = |tokens: u64, large_window: bool| Transcript { context_tokens: tokens, large_window, ..Transcript::default() };
        assert_eq!(window_size(&at(190_000, false), false), 200_000);
        assert_eq!(window_size(&at(210_000, false), false), 1_000_000);
        // After compaction the usage drops, but the window is still the large one (the read saw it past 200k).
        assert_eq!(window_size(&at(60_000, true), false), 1_000_000);
    }

    #[test]
    fn the_models_claude_code_gives_1m_are_measured_against_1m() {
        let pct = |model: &str, off: bool| {
            let t = Transcript { model: Some(model.into()), context_tokens: 150_000, ..Transcript::default() };
            t.context_tokens * 100 / window_size(&t, off)
        };
        for model in [
            "claude-opus-5-5",
            "claude-sonnet-5-5",
            "claude-haiku-5-5",
            "claude-opus-5",
            "claude-opus-4-8",
            "claude-opus-4-7",
            "claude-fable-5-1",
            "claude-fable-5",
            "claude-mythos-5-1",
            "anthropic.claude-opus-5-5",
            "us.anthropic.claude-opus-4-7-v1:0",
            "claude-opus-4-7@20260416",
            "claude-haiku-5-5-20261001",
            "Claude-Sonnet-5-5",
        ] {
            assert_eq!(pct(model, false), 15, "{model}");
            assert_eq!(pct(model, true), 75, "{model}, with CLAUDE_CODE_DISABLE_1M_CONTEXT");
        }
        for model in [
            "claude-haiku-4-5",
            "claude-haiku-4-5-20251001",
            "claude-sonnet-5",
            "claude-sonnet-4-6",
            "claude-opus-4-6",
            "claude-opus-4-5-20251101",
            "claude-3-5-sonnet-20241022",
            "anthropic.claude-sonnet-4-6",
            "some-other-model",
            "<synthetic>",
        ] {
            assert_eq!(pct(model, false), 75, "{model}");
        }
    }

    #[test]
    fn claude_codes_1m_window_can_be_switched_off_in_its_settings_or_the_environment() {
        let root = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-claude-1m-{}", std::process::id())));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        assert!(!large_window_off(&root, None), "no settings file");
        for (env, off) in [(Some("1"), true), (Some("true"), true), (Some("0"), false), (Some(""), false), (None, false)] {
            assert_eq!(large_window_off(&root, env), off, "{env:?}");
        }
        let with = |settings: &str| {
            fs::write(root.join("settings.json"), settings).unwrap();
            large_window_off(&root, None)
        };
        assert!(with(r#"{"env":{"CLAUDE_CODE_DISABLE_1M_CONTEXT":"1"}}"#));
        assert!(with(r#"{"env":{"CLAUDE_CODE_DISABLE_1M_CONTEXT":1}}"#));
        assert!(!with(r#"{"env":{"CLAUDE_CODE_DISABLE_1M_CONTEXT":"0"}}"#));
        assert!(!with(r#"{"env":{"OTHER":"1"}}"#));
        assert!(!with(r#"{"env":"1"}"#));
        assert!(!with("not json"));
        let _ = fs::remove_dir_all(&root);
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
        assert_eq!(t.context_tokens * 100 / window_size(&t, false), 12, "Opus 5.5's window is 1M");
        assert_eq!(t.context_tokens * 100 / window_size(&t, true), 60, "unless Claude Code's 1M is switched off");
        // A model id that names the 1M window: the same use is a small share of it.
        for model in ["claude-opus-5-5[1m]", "claude-sonnet-5-context-1m"] {
            let t = read_with("b.jsonl", &[reply(model, 2_000, 8_000, 110_000, false)]);
            assert!(t.large_window, "{model}");
            assert_eq!(window_size(&t, false), 1_000_000, "{model}");
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

    /// Claude Code's two shapes of a typed slash command, as it writes them (trimmed, made-up words).
    fn skill_entry(name: &str, args: Option<&str>) -> Value {
        let args = args.map(|a| format!("\n<command-args>{a}</command-args>")).unwrap_or_default();
        serde_json::json!({"type":"user","origin":{"kind":"human"},"message":{"role":"user",
            "content":format!("<command-message>{name}</command-message>\n<command-name>/{name}</command-name>{args}")}})
    }

    fn builtin_entry(name: &str, args: &str) -> Value {
        serde_json::json!({"type":"user","message":{"role":"user","content":format!(
            "<command-name>/{name}</command-name>\n            <command-message>{name}</command-message>\n            <command-args>{args}</command-args>")}})
    }

    #[test]
    fn a_typed_skill_is_what_the_user_typed_and_a_built_in_command_is_nothing() {
        use serde_json::json;
        assert_eq!(prompt(skill_entry("land", Some("the parser fix"))).as_deref(), Some("/land the parser fix"));
        assert_eq!(prompt(skill_entry("sharpen", None)).as_deref(), Some("/sharpen"), "no words: the skill alone");
        assert_eq!(prompt(skill_entry("sharpen", Some(""))).as_deref(), Some("/sharpen"));
        assert_eq!(prompt(skill_entry("tools:review", Some("line one\n  line   two"))).as_deref(), Some("/tools:review line one line two"));
        // An older Claude Code said "is running…" in the first tag; only the name and the words count.
        let old = json!({"type":"user","message":{"content":
            "<command-message>land is running…</command-message>\n<command-name>/land</command-name>\n<command-args>x y</command-args>"}});
        assert_eq!(prompt(old).as_deref(), Some("/land x y"));
        // Built-in commands Claude Code runs itself are never messages, words or not.
        for (name, args) in [("model", "sonnet"), ("plugin", "marketplace add someone/thing"), ("add-dir", ""), ("clear", "")] {
            assert_eq!(prompt(builtin_entry(name, args)), None, "/{name}");
        }
        // The skill's own text (the next entry) is Claude Code's, not the user's.
        assert_eq!(
            prompt(json!({"type":"user","isMeta":true,"message":{"content":[{"type":"text","text":"Base directory for this skill"}]}})),
            None
        );
        assert_eq!(prompt(json!({"type":"user","message":{"content":"<local-command-stdout>Set model</local-command-stdout>"}})), None);
        // Broken tags are not guessed at.
        assert_eq!(prompt(json!({"type":"user","message":{"content":"<command-message>x</command-message>"}})), None);
        // In a line, by the same rules, cut to the line reader's length.
        let line = |v: Value| v.to_string();
        assert_eq!(prompt_in_line(&line(skill_entry("land", Some("the parser fix"))), 100).as_deref(), Some("/land the parser fix"));
        assert_eq!(prompt_in_line(&line(skill_entry("land", Some("the parser fix"))), 9).as_deref(), Some("/land the"));
        assert_eq!(prompt_in_line(&line(builtin_entry("model", "sonnet")), 100), None);
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

    fn tmp(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-hidden-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
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

    #[test]
    fn the_end_of_a_transcript_says_when_a_plan_was_approved_and_when_the_skill_was_used() {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-claude-to-tasks-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        let p = d.join("s.jsonl");
        let at = |secs: u64| crate::testkit::iso(secs * 1000);
        let plan = |id: &str, t: u64, side: bool| {
            json!({"type":"assistant","isSidechain":side,"timestamp":at(t),"message":{"role":"assistant","content":[
                {"type":"text","text":"Here is the plan."},
                {"type":"tool_use","id":id,"name":"ExitPlanMode","input":{"plan":"1. One.\n2. Two.\n"}}]}})
            .to_string()
        };
        let answer = |id: &str, t: u64, error: bool| {
            let mut b = json!({"tool_use_id":id,"type":"tool_result","content":"User has approved your plan."});
            if error {
                b["is_error"] = json!(true);
            }
            json!({"type":"user","timestamp":at(t),"message":{"role":"user","content":[b]}}).to_string()
        };
        let typed = |text: &str, t: u64| json!({"type":"user","timestamp":at(t),"message":{"role":"user","content":text}}).to_string();
        let read = |lines: &[String]| {
            fs::write(&p, lines.join("\n") + "\n").unwrap();
            to_tickets_tail(&p)
        };
        assert_eq!(to_tickets_tail(&d.join("none.jsonl")), ToTicketsTail::default());
        // Approved, rejected, not answered yet, a subagent's.
        assert_eq!(read(&[plan("t1", 100, false), answer("t1", 110, false)]).plan_approved, Some(110));
        assert_eq!(read(&[plan("t1", 100, false), answer("t1", 110, true)]).plan_approved, None);
        assert_eq!(read(&[plan("t1", 100, false)]).plan_approved, None);
        assert_eq!(read(&[plan("t1", 100, true), answer("t1", 110, false)]).plan_approved, None);
        assert_eq!(read(&[answer("t1", 110, false)]).plan_approved, None, "an answer to no plan");
        // The latest of two; a rejected one after an approved one leaves the approved one's time.
        let two = read(&[plan("t1", 100, false), answer("t1", 110, false), plan("t2", 200, false), answer("t2", 210, true)]);
        assert_eq!(two.plan_approved, Some(110));
        // The skill typed (with words or without), or run with the Skill tool; mentioned in words is not a use. By its
        // name, /hand-to-squad, or an old one, /to-tasks or /to-tickets (conversations from before the renames).
        for name in ["hand-to-squad", "to-tasks", "to-tickets"] {
            let cmd = |args: &str| format!("<command-message>{name}</command-message>\n<command-name>/{name}</command-name>{args}");
            assert_eq!(read(&[typed(&cmd(""), 300)]).to_tickets_used, Some(300), "{name}");
            assert_eq!(read(&[typed(&cmd("\n<command-args>just the mails</command-args>"), 300)]).to_tickets_used, Some(300), "{name}");
            let tool = json!({"type":"assistant","timestamp":at(400),"message":{"role":"assistant","content":[
                {"type":"tool_use","id":"t9","name":"Skill","input":{"skill":name}}]}})
            .to_string();
            assert_eq!(read(std::slice::from_ref(&tool)).to_tickets_used, Some(400), "{name}");
            assert_eq!(read(&[typed(&format!("Should I run /{name} for this?"), 300)]).to_tickets_used, None, "{name}");
            let meta = json!({"type":"user","isMeta":true,"timestamp":at(300),"message":{"role":"user","content":format!("Base directory for this skill: /x/{name}")}}).to_string();
            assert_eq!(read(&[meta]).to_tickets_used, None, "the skill's own text that follows a typed one");
        }
        for other in ["hand-to-squad-later", "to-tasks-later"] {
            let other = format!("<command-message>{other}</command-message>\n<command-name>/{other}</command-name>");
            assert_eq!(read(&[typed(&other, 300)]).to_tickets_used, None, "another skill whose name starts the same");
        }
        // Only the end is read: a plan approved before the last TO_TICKETS_TAIL_BYTES is not seen, one after is.
        let filler = typed(&"x".repeat(1000), 150);
        let mut long = vec![plan("t1", 100, false), answer("t1", 110, false)];
        long.extend(std::iter::repeat_n(filler, (TO_TICKETS_TAIL_BYTES / 1000) as usize + 10));
        assert_eq!(read(&long).plan_approved, None);
        long.extend([plan("t3", 500, false), answer("t3", 510, false)]);
        assert_eq!(read(&long).plan_approved, Some(510));
        let _ = fs::remove_dir_all(&d);
    }
}
