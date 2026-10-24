//! A task's session: what it is started with, and the hooks that move the task along while it works.
//!
//! A task runs as one ordinary interactive session of its agent in a platoon host, in the task's worktree (or its
//! folder, when that is not in git). It is started (`agents::TaskLaunch`, `Agent::task_args`; Claude Code's flags are
//! in `agents/claude.rs`) with the id platoon chose for it, its name "#N title", the squad's model, plan mode for a
//! squad that plans first, the hooks below for this session only, `<task folder>/squad.md` added to its system prompt:
//! platoon's fixed rules (`rules`) and the squad's steps, frozen when the task starts, with every squad copied beside
//! it (`<task folder>/squads/<id>.md`) so a step can name one; the task's folder readable; and the task's text as its
//! first message.
//!
//! The hooks (`platoon hook task-...`), each reading the hook's JSON on stdin and finding the task by its session:
//! - `task-plan`, `PreToolUse` on `ExitPlanMode`: the plan is saved as the task's `plan.md`; Needs you, a plan to approve.
//! - `task-question`, `PreToolUse` on `AskUserQuestion`: Needs you, with the question.
//! - `task-notify`, `Notification`: a permission prompt makes a Working task Needs you.
//! - `task-tool`, `PostToolUse` on every tool: the answer came (the plan approved, the question answered, the
//!   permission given): Working again.
//! - `task-prompt`, `UserPromptSubmit`: the user typed: Working, and the count of refused stops starts again.
//! - `task-stop`, `Stop`: the work is checked (`verdict.rs`; the task is Working, "checking the work", meanwhile):
//!   done, the task is Done; not done, the stop is refused once with what the check found, and the next "not done"
//!   makes the task need the user; a check that could not run makes it need the user with why. platoon runs no tests
//!   itself: the rules (`rules`) tell the session to find and run the repo's tests and report them, and the check
//!   holds it to that.

use crate::agents::{Agent, HookEvent, HookKind, HookReply, Tool, TurnEnd};
use crate::squads::Squad;
use crate::tickets::{self, Need, State, Ticket, Tickets};
use crate::verdict;
use serde_json::json;
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    time::Duration,
};

// ---- what the session is started with -----------------------------------------------------------------------------

/// The hooks, by the name `platoon hook <name>` takes.
pub const HOOKS: [&str; 6] = ["task-plan", "task-question", "task-notify", "task-tool", "task-prompt", "task-stop"];

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
    /// Where the squads are copied (`<task folder>/squads`).
    pub squads_dir: String,
    pub plan_first: bool,
}

/// The rule on tests: platoon runs none itself, so the session finds the repo's own and reports them (the check of the
/// work holds a change to code to it: `verdict::SYSTEM`).
pub const TESTS_RULE: &str = "- When you change code, find how this repo runs its tests (its CLAUDE.md, README, or build files), run them before you finish, and name the command and its result in your final message. If the repo has no tests, say so and how you checked the change instead.\n";

/// platoon's fixed rules, above the squad's steps in `squad.md`. Not the user's to change.
pub fn rules(r: &Rules) -> String {
    let mut out = String::from("# How this task works\n\nYou are working on a task the user handed to platoon.\n\n");
    match &r.branch {
        Some(b) => out.push_str(&format!(
            "- You work in {} on the branch `{b}`, a git worktree made for this task. Commit what you change there.\n\
             - What you commit stays on the branch `{b}`: it is not merged, and the user merges it. If you committed, your final message says so: that the work is on the branch `{b}` and is not merged. If you changed nothing, leave the branch out of it.\n",
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
    out.push_str(TESTS_RULE);
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

/// What a hook says back: the text for its stdout (None: nothing).
pub type Said = Option<String>;

/// Where a hook finds what it needs: platoon's folders, the `claude` the check of the work runs, and how long it may.
pub struct Places {
    pub config: PathBuf,
    pub cache: PathBuf,
    /// The AI programs the check of the work may run on.
    pub providers: crate::agents::Providers,
    pub check_timeout: Duration,
}

/// The kind of hook each of the task's hook commands is installed as (see `HOOKS`).
fn kind(name: &str) -> Option<HookKind> {
    Some(match name {
        "task-plan" => HookKind::BeforeTool(Tool::Plan),
        "task-question" => HookKind::BeforeTool(Tool::Question),
        "task-notify" => HookKind::PermissionWait,
        "task-tool" => HookKind::AfterTool,
        "task-prompt" => HookKind::Prompt,
        "task-stop" => HookKind::Stop,
        _ => return None,
    })
}

/// Run hook `name` on its JSON `input` (Claude Code's: its hooks are the ones that call these): what it says back.
/// Nothing happens for a session that is no task's.
pub fn hook(name: &str, input: &str, at: &Places) -> Said {
    let agent = Agent::Claude;
    let config = at.config.as_path();
    let input = agent.hook_input(kind(name)?, input)?;
    let (project, number) = tickets::find_session(config, &input.session)?;
    let mut store = Tickets::open(config, &project);
    let t = store.get(number)?.clone();
    if matches!(t.state, State::NotStarted | State::Cancelled) {
        return None;
    }
    let set = |store: &mut Tickets, f: &dyn Fn(&Ticket) -> Option<State>| {
        let _ = store.update(number, |t| match f(t) {
            Some(s) if s != t.state => {
                t.state = s;
                true
            }
            _ => false,
        });
    };
    match input.event {
        HookEvent::BeforeTool { tool: Tool::Plan, said: plan } => {
            let file = tickets::plan_file(config, &project, number);
            if let Some(dir) = file.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = crate::projects::write_atomic(&file, &plan);
            set(&mut store, &|_| Some(State::NeedsYou(Need::Plan)));
        }
        HookEvent::BeforeTool { tool: Tool::Question, said } => {
            let said = if said.is_empty() { "It asked you something: see its session.".into() } else { said };
            let q = tickets::clean(&said, tickets::WHY_MAX);
            set(&mut store, &|_| Some(State::NeedsYou(Need::Question(q.clone()))));
        }
        HookEvent::PermissionWait(msg) => {
            let msg = msg.unwrap_or_else(|| "It waits for your permission: see its session.".into());
            let msg = tickets::clean(&msg, tickets::WHY_MAX);
            set(&mut store, &|t| (t.state == State::Working).then(|| State::NeedsYou(Need::Permission(msg.clone()))));
        }
        HookEvent::AfterTool(tool) => set(&mut store, &|t| match &t.state {
            State::NeedsYou(Need::Plan) if tool == Tool::Plan => Some(State::Working),
            State::NeedsYou(Need::Question(_)) if tool == Tool::Question => Some(State::Working),
            State::NeedsYou(Need::Permission(_)) => Some(State::Working),
            _ => None,
        }),
        HookEvent::PromptSubmitted => {
            let _ = store.update(number, |t| {
                let reopen = matches!(t.state, State::NeedsYou(_) | State::Done);
                if !reopen && t.checks_refused == 0 && t.checking.is_none() {
                    return false;
                }
                if reopen {
                    t.state = State::Working;
                }
                t.checks_refused = 0;
                // A check still running is of the work before this message: its answer no longer moves the task.
                t.checking = None;
                true
            });
        }
        HookEvent::Stop(end) => {
            let reason = stop(&end, at, &project, &mut store, &t)?;
            return Some(agent.hook_answer(&HookReply::SendBack(reason)).stdout);
        }
        HookEvent::BeforeTool { tool: Tool::Other, .. } | HookEvent::SessionStart { .. } => {}
    }
    None
}

/// The `Stop` hook: the end of a turn. Some reason when the stop is refused.
fn stop(end: &TurnEnd, at: &Places, project: &str, store: &mut Tickets, t: &Ticket) -> Option<String> {
    // Waiting for the user already (a plan to approve, a question), or subagents still at work: nothing to do.
    if t.state != State::Working || end.subagents_working {
        return None;
    }
    if end.planning {
        let said = end.last_message.as_deref().map(|m| tickets::clean(m, tickets::WHY_MAX));
        let why = said.filter(|s| !s.is_empty()).unwrap_or_else(|| "It stopped while planning: see its session.".into());
        let _ = store.update(t.number, |x| {
            x.state = State::NeedsYou(Need::Question(why));
            true
        });
        return None;
    }
    check(end, at, project, store, t.number)
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

/// The end of a turn: check the work (`verdict.rs`). Done, refused once with what it found, or
/// Needs you. While it runs the task is Working with `checking` set; a message the user types meanwhile clears that,
/// and then the answer is kept but moves nothing.
fn check(end: &TurnEnd, at: &Places, project: &str, store: &mut Tickets, number: u32) -> Option<String> {
    let started = crate::usage::now();
    let _ = store.update(number, |x| {
        x.checking = Some(started);
        true
    });
    let t = store.get(number)?.clone();
    let model = verdict_model(&at.config, project, &t);
    let claim = end.last_message.as_deref().unwrap_or_default();
    let m = verdict::gather("git", &t, &store.body(&t), claim);
    let runner = crate::ai::Runner::new(&at.cache, at.providers.clone());
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
            x.checks_refused = 0;
        } else if x.checks_refused < tickets::CHECKS_REFUSED_MAX {
            x.checks_refused += 1;
            refuse = true;
        } else {
            x.state = State::NeedsYou(Need::Check(verdict::short(&found)));
        }
        true
    });
    refuse.then(|| verdict::refusal(&found))
}

/// `platoon hook <name>`: always 0, with what the hook says on stdout. It never fails the session.
pub fn run_hook(name: &str) -> i32 {
    let said = std::panic::catch_unwind(|| {
        let mut input = String::new();
        std::io::stdin().take(1 << 20).read_to_string(&mut input).ok()?;
        let at = Places {
            config: crate::archive::config_dir(),
            cache: crate::usage::cache_dir(),
            providers: crate::agents::Providers::here(),
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
    use crate::agents::claude::shell_word;
    use crate::agents::{Agent, TaskLaunch};
    use crate::tickets::NewTicket;
    use serde_json::Value;

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
            providers: crate::agents::Providers { claude: Some(script.to_string_lossy().into_owned()), codex: None },
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

    /// A project, and a task of it started in `<d>/work` as session `s-1`.
    fn world(name: &str) -> (PathBuf, String, u32) {
        let d = dir(name);
        let p = crate::projects::Projects::open(&d).create("Shop").unwrap();
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
        let l = TaskLaunch {
            session: "abc".into(),
            resume: false,
            name: "#5 Login".into(),
            model: "opus".into(),
            plan: true,
            task_dir: PathBuf::from("/c/tasks/05-work"),
            exe: "/opt/platoon/bin/platoon".into(),
            first: Some("Task #5: Login\n\nDo it.".into()),
        };
        let a = Agent::Claude.task_args(&l);
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
        assert_eq!(hooks["Stop"][0]["hooks"][0]["timeout"], 600 + 60, "the check's time and a minute");
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
        let a = Agent::Claude.task_args(&again);
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
            squads_dir: task_dir.join("squads").to_string_lossy().into_owned(),
            plan_first: true,
        };
        write_task_files(&task_dir, &feature, &all, &r).unwrap();
        let text = fs::read_to_string(task_dir.join("squad.md")).unwrap();
        for want in [
            "You work in /w/shop-worktrees/task-5 on the branch `task/5`",
            "Commit what you change there.",
            "If you committed, your final message says so: that the work is on the branch `task/5` and is not merged.",
            "If you changed nothing, leave the branch out of it.",
            "the user merges it",
            "Never push, open or merge pull requests, or publish anything unless the user asks for it in this session.",
            "AskUserQuestion tool, not in prose",
            "how you checked each \"Done when\" box",
            "find how this repo runs its tests (its CLAUDE.md, README, or build files), run them before you finish, and name the command and its result in your final message",
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
        // Not in git, not plan first: none of those lines. The tests are the session's to run either way.
        let plain = rules(&Rules { folder: "/notes".into(), squads_dir: "/x".into(), ..Default::default() });
        assert!(plain.contains("You work in /notes. It is not a git repository."));
        assert!(!plain.contains("branch") && !plain.contains("plan mode"), "{plain}");
        assert!(plain.contains(TESTS_RULE), "{plain}");
        assert!(!plain.contains("platoon runs"), "platoon runs no tests: {plain}");
    }

    #[test]
    fn a_plan_is_saved_and_needs_you_until_the_user_approves_it() {
        let (d, p, n) = world("plan");
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
        let (d, p, n) = world("question");
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
        let (d, p, n) = world("permission");
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
    fn a_stop_the_check_passes_is_done_and_typing_makes_it_working_again() {
        let (d, p, n) = world("check-passes");
        assert_eq!(hook("task-stop", &input("Stop", json!({"stop_hook_active": false})), &d), None);
        assert_eq!(state(&d, &p, n), State::Done);
        assert_eq!(checks_made(&d), 1, "the check ran, and nothing else");
        assert!(!tickets::task_dir(&d, &p, n).join("tests.log").exists(), "platoon runs no tests");
        hook("task-prompt", &input("UserPromptSubmit", json!({"prompt": "one more thing"})), &d);
        assert_eq!(state(&d, &p, n), State::Working, "typed into a Done task");
    }

    #[test]
    fn a_stop_with_subagents_still_at_work_or_waiting_for_the_user_changes_nothing() {
        let (d, p, n) = world("busy");
        hook("task-stop", &input("Stop", json!({"background_tasks": [{"id": "a1"}]})), &d);
        assert_eq!(state(&d, &p, n), State::Working);
        hook("task-plan", &input("PreToolUse", json!({"tool_name": "ExitPlanMode", "tool_input": {"plan": "p"}})), &d);
        hook("task-stop", &input("Stop", json!({})), &d);
        assert_eq!(state(&d, &p, n), State::NeedsYou(Need::Plan), "the plan still waits for the user");
    }

    #[test]
    fn a_stop_while_planning_without_a_plan_needs_you_with_its_last_words() {
        let (d, p, n) = world("planning");
        hook("task-stop", &input("Stop", json!({"permission_mode": "plan", "last_assistant_message": "Should it be red or blue?"})), &d);
        assert_eq!(state(&d, &p, n), State::NeedsYou(Need::Question("Should it be red or blue?".into())));
        assert_eq!(checks_made(&d), 0, "no check while planning");
    }

    #[test]
    fn a_session_that_is_no_tasks_and_a_cancelled_task_are_left_alone() {
        let (d, p, n) = world("alone");
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
        let (d, p, n) = world("check-done");
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
        let day = crate::ledger::Ledger::open(&d.join("cache")).today(crate::usage::now(), Agent::Claude);
        assert_eq!(day.features.get(verdict::FEATURE).map(|f| f.tokens), Some(1200));
        // The check is outside the token gate: the smart features are off here, and it ran all the same.
        assert!(!crate::settings::load(&d).0.tokens_enabled);
    }

    #[test]
    fn not_done_refuses_the_stop_once_with_what_it_found_then_needs_you_and_typing_starts_again() {
        let (d, p, n) = world("check-not-done");
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
        assert_eq!((t.state.clone(), t.checks_refused), (State::Working, 1));
        assert_eq!(t.verdict.as_ref().map(|v| (v.done, v.dropped, v.problems())), Some((false, 1, 2)));
        // The second "not done": the stop goes through, and the task needs the user with what was found.
        assert_eq!(hook("task-stop", &input("Stop", json!({})), &d), None);
        let t = task(&d, &p, n);
        assert_eq!(t.state, State::NeedsYou(Need::Check("2 problems: mail.rs:2: Only HTML is sent.".into())));
        assert_eq!(checks_made(&d), 2, "the check, each stop");
        // The user types: Working, and one more refusal before it needs them again.
        hook("task-prompt", &input("UserPromptSubmit", json!({"prompt": "look again"})), &d);
        let t = task(&d, &p, n);
        assert_eq!((t.state.clone(), t.checks_refused), (State::Working, 0));
        assert!(hook("task-stop", &input("Stop", json!({})), &d).is_some(), "refused again");
    }

    #[test]
    fn a_not_done_whose_findings_are_not_in_the_code_counts_as_done_with_a_note() {
        let (d, p, n) = world("check-made-up");
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
        let (d, p, n) = world("check-fails");
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
