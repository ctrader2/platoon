//! A task's session: what it is started with, and the hooks that move the task along while it works.
//!
//! A task runs as one ordinary interactive `claude` session in a platoon host, in the task's worktree (or its folder,
//! when that is not in git). It is started with (`args`):
//! - `--session-id` (platoon picks it, so the task names it before it starts) and `-n "#N title"`;
//! - `--model`, the squad's model, and `--permission-mode plan` for a squad that plans first, else `acceptEdits`;
//! - `--settings`, the hooks below for this session only (the user's own settings stay as they are), with
//!   `permissions.ask` rules so pushing, opening or merging pull requests and publishing wait at Claude Code's own
//!   permission prompt (`ASK`);
//! - `--append-system-prompt-file <task folder>/squad.md`: platoon's fixed rules (`rules`) and the squad's steps, frozen
//!   when the task starts, with every squad copied beside it (`<task folder>/squads/<id>.md`) so a step can name one;
//! - `--add-dir <task folder>`, so the session can read those files;
//! - the task's text as its first message.
//!
//! The hooks (`platoon hook task-...`), each reading the hook's JSON on stdin and finding the task by its session:
//! - `task-plan`, `PreToolUse` on `ExitPlanMode`: the plan is saved as the task's `plan.md`; Needs you, a plan to approve.
//! - `task-question`, `PreToolUse` on `AskUserQuestion`: Needs you, with the question.
//! - `task-notify`, `Notification`: a permission prompt makes a Working task Needs you.
//! - `task-tool`, `PostToolUse` on every tool: the answer came (the plan approved, the question answered, the
//!   permission given): Working again.
//! - `task-prompt`, `UserPromptSubmit`: the user typed: Working, and the counts of refused stops start again.
//! - `task-stop`, `Stop`: the project's tests run in the task's folder; failing, the stop is refused with their last
//!   lines (twice in a row at most), then the task needs the user. Passing, or with no test command, the work is
//!   checked (`verdict.rs`; the task is Working, "checking the work", meanwhile): done, the task is Done; not done, the
//!   stop is refused once with what the check found, and the next "not done" makes the task need the user; a check
//!   that could not run makes it need the user with why.

use crate::checks;
use crate::squads::Squad;
use crate::tickets::{self, Need, State, TestRun, Ticket, Tickets};
use crate::verdict;
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Duration,
};

// ---- what the session is started with -----------------------------------------------------------------------------

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

/// The hooks, by the name `platoon hook <name>` takes.
pub const HOOKS: [&str; 6] = ["task-plan", "task-question", "task-notify", "task-tool", "task-prompt", "task-stop"];

/// How much longer than the tests may run the `Stop` hook is given before Claude Code gives up on it, in seconds.
const STOP_MARGIN: u64 = 60;

/// A program path as one word for a shell (the hook commands run through one).
pub fn shell_word(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+=:@%,".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// The `--settings` JSON of a task's session: its hooks, calling the platoon `exe`, and the `ask` rules. `test_secs`:
/// how long the project's tests may run (the `Stop` hook waits that long, the check of the work's time, and a little
/// more).
pub fn settings_json(exe: &str, test_secs: u64) -> String {
    let exe = shell_word(exe);
    let cmd = |name: &str| json!({"type": "command", "command": format!("{exe} hook {name}")});
    let mut stop = cmd("task-stop");
    if let Some(o) = stop.as_object_mut() {
        o.insert("timeout".into(), json!(test_secs + verdict::TIMEOUT.as_secs() + STOP_MARGIN));
    }
    json!({
        "permissions": {"ask": ASK},
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

/// What a task's session is started (or continued) with.
#[derive(Clone, Debug)]
pub struct Launch {
    pub session: String,
    /// Continue the session (`--resume`) rather than start it.
    pub resume: bool,
    /// Its name: "#N title".
    pub name: String,
    pub model: String,
    /// Start in plan mode.
    pub plan: bool,
    /// The task's own folder, with `squad.md` in it.
    pub task_dir: PathBuf,
    /// The `--settings` JSON.
    pub settings: String,
    /// The first message (a new session only).
    pub first: Option<String>,
}

/// The arguments `claude` gets. `--add-dir` comes first, so it can't take the first message for a folder.
pub fn args(l: &Launch) -> Vec<String> {
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
        l.settings.clone(),
        "--append-system-prompt-file".into(),
        l.task_dir.join(SQUAD_FILE).to_string_lossy().into_owned(),
    ]);
    if let (false, Some(m)) = (l.resume, &l.first) {
        a.push(m.clone());
    }
    a
}

/// The session's name.
pub fn session_name(t: &Ticket) -> String {
    format!("#{} {}", t.number, t.title)
}

/// The task's text as the session's first message.
pub fn first_message(t: &Ticket, body: &str) -> String {
    format!("Task #{}: {}\n\n{}", t.number, t.title, body.trim_end())
}

/// A new session id, chosen by platoon so it can be written on the task before the session starts (a version 4 UUID,
/// the shape Claude Code's own ids have).
pub fn new_session_id() -> String {
    let mut b = [0u8; 16];
    let read = fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b));
    if read.is_err() {
        // No /dev/urandom: the time and the process id, spread over the bytes. Unique enough for one computer.
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
            ^ u128::from(std::process::id()) << 64;
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for (i, x) in b.iter_mut().enumerate() {
            h = (h ^ (seed >> ((i % 16) * 8)) as u64 & 0xff).wrapping_mul(0x0100_0000_01b3);
            *x = (h >> 24) as u8;
        }
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

// ---- squad.md ------------------------------------------------------------------------------------------------------

/// The file in the task's folder the session's system prompt is added from.
pub const SQUAD_FILE: &str = "squad.md";

/// What platoon's fixed rules say about a task.
#[derive(Clone, Debug, Default)]
pub struct Rules {
    /// Where the session works.
    pub folder: String,
    /// In git, on this branch (None: the folder is not in git).
    pub branch: Option<String>,
    /// The project's test command ("" for none).
    pub test_command: String,
    /// Where the squads are copied (`<task folder>/squads`).
    pub squads_dir: String,
    pub plan_first: bool,
}

/// platoon's fixed rules, above the squad's steps in `squad.md`. Not the user's to change.
pub fn rules(r: &Rules) -> String {
    let mut out = String::from("# How this task works\n\nYou are working on a task the user handed to platoon.\n\n");
    match &r.branch {
        Some(b) => out.push_str(&format!(
            "- You work in {} on the branch `{b}`, a git worktree made for this task. Commit your work there.\n",
            r.folder
        )),
        None => out.push_str(&format!("- You work in {}. It is not a git repository.\n", r.folder)),
    }
    out.push_str(
        "- Never push, open or merge pull requests, or publish anything unless the user asks for it in this session.\n\
         - Ask the user questions with the AskUserQuestion tool, not in prose.\n",
    );
    if r.plan_first {
        out.push_str("- You start in plan mode. Present the plan with ExitPlanMode; the user approves it before you change anything.\n");
    }
    out.push_str("- When you finish, end with what you did and how you checked each \"Done when\" box of the task.\n");
    if !r.test_command.is_empty() {
        out.push_str(&format!(
            "- When you stop, platoon runs the project's tests (`{}`) in your folder. If they fail, you get their output back and carry on.\n",
            r.test_command
        ));
    }
    out.push_str(
        "- Each time you finish, another Claude that can only read checks your work against the task and its \"Done when\" boxes. If it finds problems, you get them back (file:line first) and carry on.\n",
    );
    out.push_str(&format!(
        "- The squads are in {}, one file each. When a step says to follow another squad, read its file and follow its steps.\n",
        r.squads_dir
    ));
    out
}

/// Write `squad.md` (the rules, then the squad's steps) and a copy of every squad in `squads` into the task's folder.
pub fn write_task_files(task_dir: &Path, squad: &Squad, squads: &[Squad], r: &Rules) -> io::Result<()> {
    let dir = task_dir.join("squads");
    fs::create_dir_all(&dir)?;
    for s in squads.iter().filter(|s| s.usable()) {
        crate::projects::write_atomic(&dir.join(format!("{}.md", s.id)), &s.text())?;
    }
    let text = format!("{}\n# The squad: {}\n\n{}", rules(r), squad.name, squad.body);
    crate::projects::write_atomic(&task_dir.join(SQUAD_FILE), &text)
}

// ---- the hooks -----------------------------------------------------------------------------------------------------

/// The text of the first question of an `AskUserQuestion` call (each question's text, joined).
fn questions(input: &Value) -> String {
    let qs: Vec<String> = input
        .get("questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|q| q.get("question").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(String::from))
        .collect();
    if qs.is_empty() { "It asked you something: see its session.".into() } else { qs.join(" / ") }
}

/// Does this `Notification` input say Claude waits for the user's permission? `notification_type`
/// "permission_prompt" says so, else a `message` that mentions permission ("Claude needs your permission to use Bash").
pub fn is_permission_notice(v: &Value) -> bool {
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

/// What a hook says back: the text for its stdout (None: nothing).
pub type Said = Option<String>;

/// Where a hook finds what it needs: platoon's folders, the `claude` the check of the work runs, and how long it may.
pub struct Places {
    pub config: PathBuf,
    pub cache: PathBuf,
    /// `claude`, or `PLATOON_CLAUDE_BIN`.
    pub claude: String,
    pub check_timeout: Duration,
}

/// Run hook `name` on its JSON `input`: what it says back. Nothing happens for a session that is no task's.
pub fn hook(name: &str, input: &str, at: &Places) -> Said {
    let config = at.config.as_path();
    let v: Value = serde_json::from_str(input).ok()?;
    let session = v.get("session_id").and_then(Value::as_str)?;
    let (project, number) = tickets::find_session(config, session)?;
    let mut store = Tickets::open(config, &project);
    let t = store.get(number)?.clone();
    if matches!(t.state, State::NotStarted | State::Cancelled) {
        return None;
    }
    let tool = v.get("tool_name").and_then(Value::as_str).unwrap_or_default().to_string();
    let set = |store: &mut Tickets, f: &dyn Fn(&Ticket) -> Option<State>| {
        let _ = store.update(number, |t| match f(t) {
            Some(s) if s != t.state => {
                t.state = s;
                true
            }
            _ => false,
        });
    };
    match name {
        "task-plan" => {
            let plan = v.get("tool_input").and_then(|i| i.get("plan")).and_then(Value::as_str).unwrap_or_default();
            let file = tickets::plan_file(config, &project, number);
            if let Some(dir) = file.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = crate::projects::write_atomic(&file, plan);
            set(&mut store, &|_| Some(State::NeedsYou(Need::Plan)));
        }
        "task-question" => {
            let q = tickets::clean(&questions(v.get("tool_input").unwrap_or(&Value::Null)), tickets::WHY_MAX);
            set(&mut store, &|_| Some(State::NeedsYou(Need::Question(q.clone()))));
        }
        "task-notify" if is_permission_notice(&v) => {
            let msg = v.get("message").and_then(Value::as_str).unwrap_or("It waits for your permission: see its session.");
            let msg = tickets::clean(msg, tickets::WHY_MAX);
            set(&mut store, &|t| (t.state == State::Working).then(|| State::NeedsYou(Need::Permission(msg.clone()))));
        }
        "task-tool" => set(&mut store, &|t| match &t.state {
            State::NeedsYou(Need::Plan) if tool == "ExitPlanMode" => Some(State::Working),
            State::NeedsYou(Need::Question(_)) if tool == "AskUserQuestion" => Some(State::Working),
            State::NeedsYou(Need::Permission(_)) => Some(State::Working),
            _ => None,
        }),
        "task-prompt" => {
            let _ = store.update(number, |t| {
                let reopen = matches!(t.state, State::NeedsYou(_) | State::Done);
                if !reopen && t.stops_refused == 0 && t.checks_refused == 0 && t.checking.is_none() {
                    return false;
                }
                if reopen {
                    t.state = State::Working;
                }
                t.stops_refused = 0;
                t.checks_refused = 0;
                // A check still running is of the work before this message: its answer no longer moves the task.
                t.checking = None;
                true
            });
        }
        "task-stop" => return stop(&v, at, &project, &mut store, &t),
        _ => {}
    }
    None
}

/// The `Stop` hook: the end of a turn.
fn stop(v: &Value, at: &Places, project: &str, store: &mut Tickets, t: &Ticket) -> Said {
    let config = at.config.as_path();
    // Waiting for the user already (a plan to approve, a question), or subagents still at work: nothing to do.
    if t.state != State::Working || still_working(v.get("background_tasks")) {
        return None;
    }
    if v.get("permission_mode").and_then(Value::as_str) == Some("plan") {
        let said = v.get("last_assistant_message").and_then(Value::as_str).map(|m| tickets::clean(m, tickets::WHY_MAX));
        let why = said.filter(|s| !s.is_empty()).unwrap_or_else(|| "It stopped while planning: see its session.".into());
        let _ = store.update(t.number, |x| {
            x.state = State::NeedsYou(Need::Question(why));
            true
        });
        return None;
    }
    let p = crate::projects::Projects::open(config).get(project).cloned();
    let command = p.as_ref().map(|p| p.test_command.clone()).unwrap_or_default();
    if command.is_empty() {
        return check(v, at, project, store, t.number);
    }
    let timeout = Duration::from_secs(p.map_or(crate::projects::TEST_TIMEOUT_MINUTES * 60, |p| p.test_timeout_secs));
    let folder = t.work_folder().unwrap_or_default();
    let log = tickets::task_dir(config, project, t.number).join("tests.log");
    let ran = checks::run(&command, Path::new(&folder), &checks::env(), timeout, &log, &AtomicBool::new(false));
    let run = TestRun {
        ok: ran.ok,
        summary: tickets::clean(&ran.summary, checks::SUMMARY_MAX),
        command: command.clone(),
        at: crate::usage::now(),
    };
    let mut refuse = false;
    let _ = store.update(t.number, |x| {
        x.tests.push(run.clone());
        let extra = x.tests.len().saturating_sub(tickets::TESTS_KEPT);
        x.tests.drain(..extra);
        if run.ok {
            x.stops_refused = 0;
        } else if x.stops_refused < tickets::STOPS_REFUSED_MAX {
            x.stops_refused += 1;
            refuse = true;
        } else {
            x.state = State::NeedsYou(Need::Tests(run.summary.clone()));
        }
        true
    });
    if run.ok {
        return check(v, at, project, store, t.number);
    }
    if !refuse {
        return None;
    }
    let tail = checks::tail(&ran.output, checks::TAIL_LINES);
    let mut reason = format!("The project's tests failed when platoon ran `{command}` in {folder}");
    if tail.trim().is_empty() {
        reason.push_str(&format!(" ({}).", ran.summary));
    } else {
        reason.push_str(&format!(":\n\n```\n{tail}\n```\n"));
    }
    reason.push_str("\nFix what makes them fail, then finish again: platoon runs them each time you stop.");
    Some(json!({"decision": "block", "reason": reason}).to_string())
}

/// The model a task's work is checked with: the `verdict_model` of the squad it started with (its copy in the task's
/// own folder, else the squad as it is now), else that squad's own model.
fn verdict_model(config: &Path, project: &str, t: &Ticket) -> String {
    let copy = tickets::task_dir(config, project, t.number).join("squads").join(format!("{}.md", t.squad));
    let squad = fs::read_to_string(&copy)
        .ok()
        .map(|text| crate::squads::parse(&t.squad, &text, crate::squads::Origin::Own))
        .or_else(|| crate::squads::get(config, &t.squad));
    squad
        .map(|s| if s.verdict_model.is_empty() { s.model } else { s.verdict_model })
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "opus".into())
}

/// The tests passed (or there are none): check the work (`verdict.rs`). Done, refused once with what it found, or
/// Needs you. While it runs the task is Working with `checking` set; a message the user types meanwhile clears that,
/// and then the answer is kept but moves nothing.
fn check(v: &Value, at: &Places, project: &str, store: &mut Tickets, number: u32) -> Said {
    let started = crate::usage::now();
    let _ = store.update(number, |x| {
        x.checking = Some(started);
        true
    });
    let t = store.get(number)?.clone();
    let model = verdict_model(&at.config, project, &t);
    let claim = v.get("last_assistant_message").and_then(Value::as_str).unwrap_or_default();
    let m = verdict::gather("git", &t, &store.body(&t), claim);
    let runner = crate::ai::Runner::with_program(&at.cache, &at.claude);
    let ran = verdict::run(&runner, &model, Path::new(&m.folder), verdict::prompt(&m), at.check_timeout);
    let file = tickets::task_dir(&at.config, project, number).join(tickets::VERDICT_FILE);
    let now = crate::usage::now();
    let found = match ran {
        Ok((text, tokens)) => {
            verdict::keep(&file, &text);
            match verdict::parse(&text) {
                Ok(a) => verdict::judge(&a, Path::new(&m.folder), &model, tokens, now),
                Err(why) => verdict::failed(&model, &why, tokens, now),
            }
        }
        Err((why, tokens)) => {
            verdict::keep(&file, &json!({"failed": why}).to_string());
            verdict::failed(&model, &why, tokens, now)
        }
    };
    let mut refuse = false;
    let _ = store.update(number, |x| {
        let ours = x.checking == Some(started);
        if ours {
            x.checking = None;
        }
        x.verdict = Some(found.clone());
        if !ours || x.state != State::Working {
            return true;
        }
        if let Some(why) = &found.failed {
            x.state = State::NeedsYou(Need::CheckFailed(why.clone()));
        } else if found.done {
            x.state = State::Done;
            x.stops_refused = 0;
            x.checks_refused = 0;
        } else if x.checks_refused < tickets::CHECKS_REFUSED_MAX {
            x.checks_refused += 1;
            refuse = true;
        } else {
            x.state = State::NeedsYou(Need::Check(verdict::short(&found)));
        }
        true
    });
    refuse.then(|| json!({"decision": "block", "reason": verdict::refusal(&found)}).to_string())
}

/// `platoon hook <name>`: always 0, with what the hook says on stdout. It never fails the session.
pub fn run_hook(name: &str) -> i32 {
    let said = std::panic::catch_unwind(|| {
        let mut input = String::new();
        std::io::stdin().take(1 << 20).read_to_string(&mut input).ok()?;
        let at = Places {
            config: crate::archive::config_dir(),
            cache: crate::usage::cache_dir(),
            claude: std::env::var("PLATOON_CLAUDE_BIN").unwrap_or_else(|_| "claude".into()),
            check_timeout: verdict::TIMEOUT,
        };
        hook(name, &input, &at)
    })
    .ok()
    .flatten();
    if let Some(text) = said {
        println!("{text}");
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tickets::{NewTicket, Worktree};

    fn dir(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-taskrun-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// A stand-in for the `claude -p` that checks the work, in `d`: it notes each call (`check-calls`, its arguments
    /// in `check-argv`, its folder in `check-cwd`, the tasks file as it was meanwhile in `check-during`), then answers
    /// `check-reply` (else done) as `structured_output`; `check-hang` makes it hang, `check-fail` fail.
    const CHECKER: &str = r#"#!/bin/sh
D='@D@'
cat > /dev/null
echo x >> "$D/check-calls"
printf '%s\n' "$@" > "$D/check-argv"
pwd > "$D/check-cwd"
cat "$D"/projects/*/tasks.json > "$D/check-during" 2>/dev/null
if [ -e "$D/check-hang" ]; then echo $$ > "$D/check-pid"; exec sleep 30; fi
if [ -e "$D/check-fail" ]; then printf '{"type":"result","is_error":true,"result":"API Error: 500"}'; exit 1; fi
r='{"done":true,"boxes":[],"findings":[]}'
[ -e "$D/check-reply" ] && r=$(cat "$D/check-reply")
printf '{"type":"result","subtype":"success","is_error":false,"result":"","structured_output":%s,"usage":{"input_tokens":1000,"output_tokens":200}}' "$r"
"#;

    fn places(d: &Path) -> Places {
        let script = d.join("fake-claude");
        if !script.exists() {
            fs::write(&script, CHECKER.replace("@D@", &d.to_string_lossy())).unwrap();
            fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        }
        Places {
            config: d.to_path_buf(),
            cache: d.join("cache"),
            claude: script.to_string_lossy().into_owned(),
            check_timeout: Duration::from_secs(20),
        }
    }

    /// The hook, with the stand-in checker.
    fn hook(name: &str, input: &str, d: &Path) -> Said {
        super::hook(name, input, &places(d))
    }

    fn checks_made(d: &Path) -> usize {
        fs::read_to_string(d.join("check-calls")).map(|s| s.lines().count()).unwrap_or(0)
    }

    /// A project with a test command, and a task of it started in `folder` as session `s-1`.
    fn world(name: &str, command: &str) -> (PathBuf, String, u32) {
        let d = dir(name);
        let mut projects = crate::projects::Projects::open(&d);
        let p = projects.create("Shop").unwrap();
        if !command.is_empty() {
            projects.set_test_command(&p, command).unwrap();
        }
        let folder = d.join("work");
        fs::create_dir_all(&folder).unwrap();
        let mut store = Tickets::open(&d, &p);
        let n = store
            .add(NewTicket { title: "Login".into(), folder: Some(folder.to_string_lossy().into_owned()), ..Default::default() })
            .unwrap();
        store.start(n, "feature", "s-1", None).unwrap();
        (d, p, n)
    }

    fn state(d: &Path, p: &str, n: u32) -> State {
        Tickets::open(d, p).get(n).unwrap().state.clone()
    }

    fn task(d: &Path, p: &str, n: u32) -> Ticket {
        Tickets::open(d, p).get(n).unwrap().clone()
    }

    /// Does `text` match the rule cut at its `*`s (`parts`): the first part at the start, the last at the end, the
    /// others in order between?
    fn glob(parts: &[&str], text: &str) -> bool {
        let Some((first, rest)) = parts.split_first() else { return text.is_empty() };
        let Some(mut left) = text.strip_prefix(first) else { return false };
        let Some((last, middle)) = rest.split_last() else { return left.is_empty() };
        for p in middle {
            match left.find(p) {
                Some(i) => left = &left[i + p.len()..],
                None => return false,
            }
        }
        left.ends_with(last)
    }

    fn input(event: &str, extra: Value) -> String {
        let mut v = json!({"session_id": "s-1", "hook_event_name": event, "cwd": "/w", "permission_mode": "acceptEdits"});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v.to_string()
    }

    #[test]
    fn the_launch_starts_the_squads_model_in_plan_mode_with_the_hooks_the_ask_rules_and_squad_md() {
        let l = Launch {
            session: "abc".into(),
            resume: false,
            name: "#5 Login".into(),
            model: "opus".into(),
            plan: true,
            task_dir: PathBuf::from("/c/tasks/05-work"),
            settings: settings_json("/opt/platoon/bin/platoon", 600),
            first: Some("Task #5: Login\n\nDo it.".into()),
        };
        let a = args(&l);
        assert_eq!(&a[..2], ["--add-dir", "/c/tasks/05-work"], "first, so it can't take the message");
        let at = |flag: &str| a.iter().position(|x| x == flag).map(|i| a[i + 1].clone());
        assert_eq!(at("--session-id").as_deref(), Some("abc"));
        assert_eq!(at("-n").as_deref(), Some("#5 Login"));
        assert_eq!(at("--model").as_deref(), Some("opus"));
        assert_eq!(at("--permission-mode").as_deref(), Some("plan"));
        assert_eq!(at("--append-system-prompt-file").as_deref(), Some("/c/tasks/05-work/squad.md"));
        assert_eq!(a.last().map(String::as_str), Some("Task #5: Login\n\nDo it."));
        assert!(!a.iter().any(|x| x == "--agents" || x == "--agent"), "no agents of platoon's");
        let s: Value = serde_json::from_str(&at("--settings").unwrap()).unwrap();
        let hooks = &s["hooks"];
        assert_eq!(hooks["PreToolUse"][0]["matcher"], "ExitPlanMode");
        assert_eq!(hooks["PreToolUse"][0]["hooks"][0]["command"], "/opt/platoon/bin/platoon hook task-plan");
        assert_eq!(hooks["PreToolUse"][1]["matcher"], "AskUserQuestion");
        assert_eq!(hooks["PreToolUse"][1]["hooks"][0]["command"], "/opt/platoon/bin/platoon hook task-question");
        assert_eq!(hooks["Notification"][0]["hooks"][0]["command"], "/opt/platoon/bin/platoon hook task-notify");
        assert_eq!(hooks["PostToolUse"][0]["matcher"], "*");
        assert_eq!(hooks["PostToolUse"][0]["hooks"][0]["command"], "/opt/platoon/bin/platoon hook task-tool");
        assert_eq!(hooks["UserPromptSubmit"][0]["hooks"][0]["command"], "/opt/platoon/bin/platoon hook task-prompt");
        assert_eq!(hooks["Stop"][0]["hooks"][0]["command"], "/opt/platoon/bin/platoon hook task-stop");
        assert_eq!(hooks["Stop"][0]["hooks"][0]["timeout"], 600 + 600 + 60, "the tests' time, the check's and a minute");
        let ask: Vec<&str> = s["permissions"]["ask"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        for want in [
            "Bash(git push:*)",
            "Bash(gh pr create:*)",
            "Bash(gh pr merge:*)",
            "Bash(gh release:*)",
            "Bash(glab:*)",
            "Bash(cargo publish:*)",
            "Bash(npm publish:*)",
        ] {
            assert!(ask.contains(&want), "{want}");
        }
        assert!(ask.iter().any(|r| r.starts_with("Bash(gh api") && r.contains("-X")), "gh api writes");
        // A push with git's options before `push`, as Claude Code matches a rule: everything before the first `*` as
        // written, a `*` for any text, and a trailing ` *` for the bare command too.
        let asks = |cmd: &str| {
            ask.iter().filter_map(|r| r.strip_prefix("Bash(")?.strip_suffix(')')).any(|rule| {
                let rule = rule.strip_suffix(":*").map(|r| format!("{r} *")).unwrap_or_else(|| rule.to_string());
                let parts: Vec<&str> = rule.split('*').collect();
                let bare = rule.ends_with(" *") && parts.len() == 2 && cmd == parts[0].trim_end();
                bare || glob(&parts, cmd)
            })
        };
        for cmd in [
            "git push",
            "git push origin main",
            "git -C /w/shop push",
            "git -C /w/shop push -u origin task/5",
            "git -c push.default=current push",
            "git --git-dir=/w/.git push origin",
            "git --work-tree /w push",
        ] {
            assert!(asks(cmd), "{cmd}");
        }
        for cmd in ["git status", "git -C /w/shop commit -m done", "git log --oneline"] {
            assert!(!asks(cmd), "{cmd}");
        }
        assert!(!ask.contains(&"Bash(gh api:*)"), "reading with gh api doesn't ask");
        for h in HOOKS {
            assert!(s.to_string().contains(&format!("hook {h}")), "{h}");
        }
        // Not plan first: acceptEdits. Continued: --resume, no name, no first message.
        let mut again = l.clone();
        again.plan = false;
        again.resume = true;
        let a = args(&again);
        assert!(a.windows(2).any(|w| w == ["--permission-mode", "acceptEdits"]));
        assert!(a.windows(2).any(|w| w == ["--resume", "abc"]));
        assert!(!a.iter().any(|x| x == "--session-id" || x == "-n" || x.starts_with("Task #5")));
        assert!(a.iter().any(|x| x == "--append-system-prompt-file"), "the squad again on a continued session");
        assert_eq!(shell_word("/a b/platoon"), "'/a b/platoon'");
    }

    #[test]
    fn squad_md_is_the_fixed_rules_then_the_squads_steps_with_every_squad_copied_beside_it() {
        let d = dir("files");
        let all = crate::squads::all(&d);
        let feature = all[0].clone();
        let task_dir = d.join("tasks/05-work");
        let r = Rules {
            folder: "/w/shop-worktrees/task-5".into(),
            branch: Some("task/5".into()),
            test_command: "cargo test".into(),
            squads_dir: task_dir.join("squads").to_string_lossy().into_owned(),
            plan_first: true,
        };
        write_task_files(&task_dir, &feature, &all, &r).unwrap();
        let text = fs::read_to_string(task_dir.join("squad.md")).unwrap();
        for want in [
            "You work in /w/shop-worktrees/task-5 on the branch `task/5`",
            "Commit your work there.",
            "Never push, open or merge pull requests, or publish anything unless the user asks for it in this session.",
            "AskUserQuestion tool, not in prose",
            "how you checked each \"Done when\" box",
            "platoon runs the project's tests (`cargo test`)",
            "another Claude that can only read checks your work",
            "You start in plan mode",
            "# The squad: Feature",
        ] {
            assert!(text.contains(want), "{want}\n{text}");
        }
        assert!(text.ends_with(&feature.body), "the squad's steps last");
        for s in &all {
            let copy = fs::read_to_string(task_dir.join("squads").join(format!("{}.md", s.id))).unwrap();
            assert_eq!(crate::squads::parse(&s.id, &copy, s.origin), *s, "a copy reads as the squad");
        }
        // Not in git, no test command, not plan first: none of those lines.
        let plain = rules(&Rules { folder: "/notes".into(), squads_dir: "/x".into(), ..Default::default() });
        assert!(plain.contains("You work in /notes. It is not a git repository."));
        assert!(!plain.contains("branch") && !plain.contains("tests") && !plain.contains("plan mode"), "{plain}");
    }

    #[test]
    fn a_plan_is_saved_and_needs_you_until_the_user_approves_it() {
        let (d, p, n) = world("plan", "");
        let plan = "## Plan\n\n1. Do it.\n";
        assert_eq!(hook("task-plan", &input("PreToolUse", json!({"tool_name": "ExitPlanMode", "tool_input": {"plan": plan}})), &d), None);
        assert_eq!(state(&d, &p, n), State::NeedsYou(Need::Plan));
        assert_eq!(fs::read_to_string(tickets::plan_file(&d, &p, n)).unwrap(), plan);
        assert_eq!(Tickets::open(&d, &p).plan(n).as_deref(), Some(plan));
        // Another tool running meanwhile (a subagent's) doesn't answer the plan; the plan's own tool finishing does.
        hook("task-tool", &input("PostToolUse", json!({"tool_name": "Read"})), &d);
        assert_eq!(state(&d, &p, n), State::NeedsYou(Need::Plan));
        hook("task-tool", &input("PostToolUse", json!({"tool_name": "ExitPlanMode"})), &d);
        assert_eq!(state(&d, &p, n), State::Working);
    }

    #[test]
    fn a_question_needs_you_with_its_words_until_it_is_answered() {
        let (d, p, n) = world("question", "");
        let q = json!({"tool_name": "AskUserQuestion", "tool_input": {"questions": [{"question": "Which page?", "header": "Page", "options": []}, {"question": "Which colour?"}]}});
        hook("task-question", &input("PreToolUse", q), &d);
        assert_eq!(state(&d, &p, n), State::NeedsYou(Need::Question("Which page? / Which colour?".into())));
        hook("task-tool", &input("PostToolUse", json!({"tool_name": "Bash"})), &d);
        assert!(state(&d, &p, n).needs_you(), "another tool isn't the answer");
        hook("task-tool", &input("PostToolUse", json!({"tool_name": "AskUserQuestion"})), &d);
        assert_eq!(state(&d, &p, n), State::Working);
    }

    #[test]
    fn a_permission_prompt_needs_you_and_the_next_tool_that_runs_is_the_answer() {
        let (d, p, n) = world("permission", "");
        hook(
            "task-notify",
            &input("Notification", json!({"message": "Claude is waiting for your input", "notification_type": "idle_prompt"})),
            &d,
        );
        assert_eq!(state(&d, &p, n), State::Working, "not a permission prompt");
        let notice = json!({"message": "Claude needs your permission to use Bash", "notification_type": "permission_prompt"});
        hook("task-notify", &input("Notification", notice.clone()), &d);
        assert_eq!(state(&d, &p, n), State::NeedsYou(Need::Permission("Claude needs your permission to use Bash".into())));
        hook("task-tool", &input("PostToolUse", json!({"tool_name": "Bash"})), &d);
        assert_eq!(state(&d, &p, n), State::Working);
        // Only a Working task: a question stays a question.
        hook(
            "task-question",
            &input("PreToolUse", json!({"tool_name": "AskUserQuestion", "tool_input": {"questions": [{"question": "Q?"}]}})),
            &d,
        );
        hook("task-notify", &input("Notification", notice), &d);
        assert_eq!(state(&d, &p, n), State::NeedsYou(Need::Question("Q?".into())));
    }

    #[test]
    fn a_stop_with_no_test_command_is_done_and_typing_makes_it_working_again() {
        let (d, p, n) = world("no-command", "");
        assert_eq!(hook("task-stop", &input("Stop", json!({"stop_hook_active": false})), &d), None);
        assert_eq!(state(&d, &p, n), State::Done);
        assert!(task(&d, &p, n).tests.is_empty(), "nothing ran");
        hook("task-prompt", &input("UserPromptSubmit", json!({"prompt": "one more thing"})), &d);
        assert_eq!(state(&d, &p, n), State::Working, "typed into a Done task");
    }

    #[test]
    fn a_stop_with_subagents_still_at_work_or_waiting_for_the_user_changes_nothing() {
        let (d, p, n) = world("busy", "");
        hook("task-stop", &input("Stop", json!({"background_tasks": [{"id": "a1"}]})), &d);
        assert_eq!(state(&d, &p, n), State::Working);
        hook("task-plan", &input("PreToolUse", json!({"tool_name": "ExitPlanMode", "tool_input": {"plan": "p"}})), &d);
        hook("task-stop", &input("Stop", json!({})), &d);
        assert_eq!(state(&d, &p, n), State::NeedsYou(Need::Plan), "the plan still waits for the user");
    }

    #[test]
    fn a_stop_while_planning_without_a_plan_needs_you_with_its_last_words() {
        let (d, p, n) = world("planning", "true");
        hook("task-stop", &input("Stop", json!({"permission_mode": "plan", "last_assistant_message": "Should it be red or blue?"})), &d);
        assert_eq!(state(&d, &p, n), State::NeedsYou(Need::Question("Should it be red or blue?".into())));
        assert!(task(&d, &p, n).tests.is_empty(), "no tests while planning");
    }

    #[test]
    fn passing_tests_make_the_stop_done_and_are_recorded() {
        let (d, p, n) = world("pass", "echo 'test result: ok. 7 passed; 0 failed'");
        assert_eq!(hook("task-stop", &input("Stop", json!({})), &d), None);
        let t = task(&d, &p, n);
        assert_eq!(t.state, State::Done);
        assert_eq!((t.tests.len(), t.tests[0].ok, t.tests[0].summary.as_str()), (1, true, "7 passed"));
        assert!(tickets::task_dir(&d, &p, n).join("tests.log").is_file());
    }

    #[test]
    fn failing_tests_refuse_the_stop_twice_with_their_tail_then_need_you_and_typing_starts_the_count_again() {
        let (d, p, n) = world("fail", "echo first line; echo 'assertion failed: login'; exit 1");
        for k in 1..=2 {
            let said = hook("task-stop", &input("Stop", json!({"stop_hook_active": k > 1})), &d).expect("refused");
            let v: Value = serde_json::from_str(&said).unwrap();
            assert_eq!(v["decision"], "block");
            let reason = v["reason"].as_str().unwrap();
            assert!(reason.contains("assertion failed: login") && reason.contains("first line"), "the tail: {reason}");
            assert!(reason.contains("echo first line"), "what ran: {reason}");
            let t = task(&d, &p, n);
            assert_eq!((t.state.clone(), t.stops_refused), (State::Working, k));
        }
        // The third time: the stop goes through and the task needs the user.
        assert_eq!(hook("task-stop", &input("Stop", json!({"stop_hook_active": true})), &d), None);
        let t = task(&d, &p, n);
        assert_eq!(t.state, State::NeedsYou(Need::Tests("assertion failed: login".into())));
        assert_eq!(t.tests.len(), 3);
        // The user types: Working, and two more tries.
        hook("task-prompt", &input("UserPromptSubmit", json!({"prompt": "try again"})), &d);
        let t = task(&d, &p, n);
        assert_eq!((t.state.clone(), t.stops_refused), (State::Working, 0));
        assert!(hook("task-stop", &input("Stop", json!({})), &d).is_some(), "refused again");
        // Typing while Working starts the count again too.
        hook("task-prompt", &input("UserPromptSubmit", json!({"prompt": "hint"})), &d);
        assert_eq!(task(&d, &p, n).stops_refused, 0);
        // Only the last few runs are kept.
        for _ in 0..6 {
            hook("task-stop", &input("Stop", json!({})), &d);
            hook("task-prompt", &input("UserPromptSubmit", json!({})), &d);
        }
        assert_eq!(task(&d, &p, n).tests.len(), tickets::TESTS_KEPT);
    }

    #[test]
    fn tests_run_in_the_tasks_worktree_and_are_stopped_at_the_projects_time() {
        let (d, p, n) = world("where", "pwd > where.txt; sleep 30");
        let wt = d.join("wt");
        fs::create_dir_all(&wt).unwrap();
        Tickets::open(&d, &p)
            .update(n, |t| {
                t.worktree = Some(Worktree { path: wt.to_string_lossy().into_owned(), branch: "task/1".into(), ..Default::default() });
                true
            })
            .unwrap();
        let file = crate::projects::Projects::open(&d).path();
        let mut v: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        if let Some(o) = v.get_mut("projects").and_then(|x| x.get_mut(&p)).and_then(Value::as_object_mut) {
            o.insert("test_timeout_minutes".into(), json!(0.02));
        }
        fs::write(&file, v.to_string()).unwrap();
        let started = std::time::Instant::now();
        let said = hook("task-stop", &input("Stop", json!({})), &d).expect("a timeout fails");
        assert!(started.elapsed() < Duration::from_secs(10), "ended at its time, not after the sleep");
        assert!(said.contains("timed out after 1 second"), "{said}");
        assert_eq!(fs::read_to_string(wt.join("where.txt")).unwrap().trim(), wt.canonicalize().unwrap().to_string_lossy());
    }

    #[test]
    fn a_session_that_is_no_tasks_and_a_cancelled_task_are_left_alone() {
        let (d, p, n) = world("alone", "");
        let other = json!({"session_id": "someone-else", "hook_event_name": "Stop"}).to_string();
        assert_eq!(hook("task-stop", &other, &d), None);
        assert_eq!(state(&d, &p, n), State::Working);
        Tickets::open(&d, &p).cancel(n).unwrap();
        hook("task-prompt", &input("UserPromptSubmit", json!({})), &d);
        hook("task-stop", &input("Stop", json!({})), &d);
        assert_eq!(state(&d, &p, n), State::Cancelled);
        assert_eq!(hook("task-stop", "not json", &d), None);
    }

    #[test]
    fn a_stop_is_done_only_once_the_check_says_so_and_the_check_is_read_only_on_the_squads_verdict_model() {
        let (d, p, n) = world("check-done", "");
        fs::write(
            d.join("check-reply"),
            r#"{"done":true,"boxes":[{"text":"It logs in","met":true,"evidence":"src/login.rs:3"}],"findings":[]}"#,
        )
        .unwrap();
        let said = hook("task-stop", &input("Stop", json!({"last_assistant_message": "I added the login form."})), &d);
        assert_eq!(said, None);
        let t = task(&d, &p, n);
        assert_eq!(t.state, State::Done);
        let v = t.verdict.clone().expect("the check's answer is kept");
        assert!(v.done && v.failed.is_none() && t.checking.is_none());
        assert_eq!((v.model.as_str(), v.tokens), ("fable", 1200), "Feature's verdict model");
        assert_eq!(v.boxes, vec![tickets::BoxCheck { text: "It logs in".into(), met: true }]);
        // How it was asked: read only, in the task's folder, with the schema; it ran while the task said "checking".
        let argv = fs::read_to_string(d.join("check-argv")).unwrap();
        for want in
            ["--tools\nRead,Grep,Glob\n", "--model\nfable\n", "--json-schema\n", "--setting-sources\n\n", "--no-session-persistence"]
        {
            assert!(argv.contains(want), "{want:?}: {argv}");
        }
        assert!(!argv.contains("Bash") && !argv.contains("Edit"), "{argv}");
        assert_eq!(fs::read_to_string(d.join("check-cwd")).unwrap().trim(), d.join("work").canonicalize().unwrap().to_string_lossy());
        let during: Value = serde_json::from_str(&fs::read_to_string(d.join("check-during")).unwrap()).unwrap();
        assert!(during["tasks"][0]["checking"].is_u64() && during["tasks"][0]["state"] == "working", "{during}");
        // Its whole answer in the task's folder; its tokens in the ledger as the task check's.
        let kept: Value = serde_json::from_str(&fs::read_to_string(tickets::task_dir(&d, &p, n).join("verdict.json")).unwrap()).unwrap();
        assert_eq!(kept["boxes"][0]["evidence"], "src/login.rs:3");
        let day = crate::ledger::Ledger::open(&d.join("cache")).today(crate::usage::now());
        assert_eq!(day.features.get(verdict::FEATURE).map(|f| f.tokens), Some(1200));
        // The check is outside the token gate: the smart features are off here, and it ran all the same.
        assert!(!crate::settings::load(&d).0.tokens_enabled);
    }

    #[test]
    fn not_done_refuses_the_stop_once_with_what_it_found_then_needs_you_and_typing_starts_again() {
        let (d, p, n) = world("check-not-done", "echo 'test result: ok. 3 passed; 0 failed'");
        fs::write(d.join("work/mail.rs"), "fn send() {\n    html_only();\n}\n").unwrap();
        fs::write(
            d.join("check-reply"),
            r#"{"done":false,"boxes":[{"text":"A text-only client gets the order","met":false,"evidence":"none"}],
               "findings":[{"file":"mail.rs","line":2,"quote":"html_only();","problem":"Only HTML is sent."},
                           {"file":"mail.rs","line":9,"quote":"made_up();","problem":"Not there."}]}"#,
        )
        .unwrap();
        let said = hook("task-stop", &input("Stop", json!({})), &d).expect("refused");
        let v: Value = serde_json::from_str(&said).unwrap();
        assert_eq!(v["decision"], "block");
        let reason = v["reason"].as_str().unwrap();
        assert!(reason.contains("- mail.rs:2: Only HTML is sent.\n"), "file:line first: {reason}");
        assert!(reason.contains("\"A text-only client gets the order\": not met"), "{reason}");
        assert!(!reason.contains("Not there"), "a finding whose quote isn't there is dropped: {reason}");
        let t = task(&d, &p, n);
        assert_eq!((t.state.clone(), t.checks_refused, t.tests.len()), (State::Working, 1, 1), "the tests ran first");
        assert_eq!(t.verdict.as_ref().map(|v| (v.done, v.dropped, v.problems())), Some((false, 1, 2)));
        // The second "not done": the stop goes through, and the task needs the user with what was found.
        assert_eq!(hook("task-stop", &input("Stop", json!({})), &d), None);
        let t = task(&d, &p, n);
        assert_eq!(t.state, State::NeedsYou(Need::Check("2 problems: mail.rs:2: Only HTML is sent.".into())));
        assert_eq!((checks_made(&d), t.tests.len()), (2, 2), "the tests and the check, each stop");
        // The user types: Working, and one more refusal before it needs them again.
        hook("task-prompt", &input("UserPromptSubmit", json!({"prompt": "look again"})), &d);
        let t = task(&d, &p, n);
        assert_eq!((t.state.clone(), t.checks_refused), (State::Working, 0));
        assert!(hook("task-stop", &input("Stop", json!({})), &d).is_some(), "refused again");
        // Failing tests are handed back before any check runs.
        let (d2, _, _) = world("check-after-tests", "exit 1");
        assert!(hook("task-stop", &input("Stop", json!({})), &d2).is_some());
        assert_eq!(checks_made(&d2), 0);
    }

    #[test]
    fn a_not_done_whose_findings_are_not_in_the_code_counts_as_done_with_a_note() {
        let (d, p, n) = world("check-made-up", "");
        fs::write(
            d.join("check-reply"),
            r#"{"done":false,"boxes":[],"findings":[{"file":"nowhere.rs","line":3,"quote":"x();","problem":"Made up."}]}"#,
        )
        .unwrap();
        assert_eq!(hook("task-stop", &input("Stop", json!({})), &d), None);
        let t = task(&d, &p, n);
        assert_eq!(t.state, State::Done);
        let v = t.verdict.unwrap();
        assert_eq!((v.done, v.dropped, v.note.as_str()), (true, 1, "the check's findings could not be found in the code"));
    }

    #[test]
    fn a_check_that_cannot_run_needs_you_with_why_and_never_blocks() {
        let (d, p, n) = world("check-fails", "");
        fs::write(d.join("check-fail"), "").unwrap();
        assert_eq!(hook("task-stop", &input("Stop", json!({})), &d), None, "never refused");
        let t = task(&d, &p, n);
        assert!(matches!(&t.state, State::NeedsYou(Need::CheckFailed(why)) if why.contains("500")), "{:?}", t.state);
        assert!(t.verdict.unwrap().failed.is_some());
        // An answer that isn't one.
        let _ = fs::remove_file(d.join("check-fail"));
        fs::write(d.join("check-reply"), r#"{"verdict":"looks fine"}"#).unwrap();
        hook("task-prompt", &input("UserPromptSubmit", json!({})), &d);
        assert_eq!(hook("task-stop", &input("Stop", json!({})), &d), None);
        assert_eq!(task(&d, &p, n).state, State::NeedsYou(Need::CheckFailed("its answer has no done: true or false".into())));
        // Too slow: ended at its time.
        fs::write(d.join("check-hang"), "").unwrap();
        hook("task-prompt", &input("UserPromptSubmit", json!({})), &d);
        let started = std::time::Instant::now();
        let at = Places { check_timeout: Duration::from_secs(1), ..places(&d) };
        assert_eq!(super::hook("task-stop", &input("Stop", json!({})), &at), None);
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(matches!(&task(&d, &p, n).state, State::NeedsYou(Need::CheckFailed(why)) if why.contains("no answer within 1 seconds")));
        let pid: i32 = fs::read_to_string(d.join("check-pid")).unwrap().trim().parse().unwrap();
        crate::testkit::wait_ended(pid as u32);
        // A session that is no task's is never checked.
        let other = json!({"session_id": "someone-else", "hook_event_name": "Stop"}).to_string();
        let before = checks_made(&d);
        hook("task-stop", &other, &d);
        assert_eq!(checks_made(&d), before);
    }
}
