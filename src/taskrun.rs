//! A task's session: what it is started with, and the hooks that move the task along while it works.
//!
//! A task runs as one ordinary interactive session of its agent in a platoon host, in the task's worktree (or its
//! folder, when that is not in git): Claude Code or Codex, as the squad's model decides. It is started
//! (`agents::TaskLaunch`, `Agent::task_args`; each agent's flags are in its module) with the squad's model, platoon's
//! fixed rules (`rules`, worded for the agent) and the squad's steps (`<task folder>/squad.md`, frozen when the task
//! starts, with every squad copied beside it, `<task folder>/squads/<id>.md`, so a step can name one), the task's
//! folder readable, and the task's text as its first message. Claude Code also gets the id platoon chose for it, its
//! name "#N title", plan mode for a squad that plans first and the hooks below for this session only (`--settings`).
//! Codex takes none of those: its hooks are installed once for every Codex session (`platoon setup`) and act only in a
//! session whose environment names a task (`agents::TASK_ENV`); its id comes from its first hook; and a squad that
//! plans first is told to write its plan and stop.
//!
//! The hooks (`platoon hook task-...`; Codex's say `--agent codex`), each reading the hook's JSON on stdin and finding
//! the task by its session (Claude Code) or by the task its environment names (Codex):
//! - `task-start`, `SessionStart` (Codex): the session's id, written on the task.
//! - `task-plan`, `PreToolUse` on `ExitPlanMode` (Claude Code): the plan is saved as the task's `plan.md`; Needs you, a
//!   plan to approve.
//! - `task-question`, `PreToolUse` on `AskUserQuestion` (Claude Code), and `request_user_input` through `task-before`
//!   (Codex): Needs you, with the question.
//! - `task-before`, `PreToolUse` on shell commands, file edits and the question tool (Codex): a command that pushes or
//!   publishes (`askmatch.rs`, Claude Code's `ask` rules) is refused until the user says yes in the session, and file
//!   edits are refused while a plan waits for the user. In a session that is no task's but platoon started, it is the
//!   offer to make tasks instead (`offerhook.rs`).
//! - `task-notify`, `Notification` (Claude Code) or `PermissionRequest` (Codex): a permission prompt makes a Working
//!   task Needs you.
//! - `task-tool`, `PostToolUse` on every tool: the answer came (the plan approved, the question answered, the
//!   permission given): Working again.
//! - `task-prompt`, `UserPromptSubmit`: the user typed: Working, and the count of refused stops starts again. On Codex
//!   it is also the user's answer to a plan (approved: edits may start) and to a command held back (it may run once).
//! - `task-stop`, `Stop`: the work is checked (`verdict.rs`; the task is Working, "checking the work", meanwhile):
//!   done, the task is Done; not done, the stop is refused once with what the check found, and the next "not done"
//!   makes the task need the user; a check that could not run makes it need the user with why. platoon runs no tests
//!   itself: the rules (`rules`) tell the session to find and run the repo's tests and report them, and the check
//!   holds it to that. On Codex a stop with the plan still to approve is the plan (Needs you, no check), and a reply
//!   that ends with a question for the user (`QUESTION_MARK`) waits for the answer. Before the check, a stop with
//!   steps of the squad never marked done or skipped sends the session back once to mark them (`unmarked_steps`).
//!
//! The squad's steps (`steps.rs`) are kept on the task from its start; the rules have the session mark each one with
//! `platoon task step` (`tickets::cli`), and on Codex, whose sandbox can't write platoon's folder, `task-before` records
//! that command itself and refuses it (`record_steps`). A Claude Code task's status line is platoon's (`status_line`).

use crate::agents::{Agent, HookEvent, HookInput, HookKind, HookReply, Tool, TurnEnd};
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

/// The hooks, by the name `platoon hook <name>` takes (the last two are Codex's only).
pub const HOOKS: [&str; 8] =
    ["task-plan", "task-question", "task-notify", "task-tool", "task-prompt", "task-stop", "task-start", "task-before"];

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
    /// The agent the task's session runs on: the rules say how to do things in its own ways.
    pub agent: Agent,
    /// Where the session works.
    pub folder: String,
    /// In git, on this branch (None: the folder is not in git).
    pub branch: Option<String>,
    /// Where the squads are copied (`<task folder>/squads`).
    pub squads_dir: String,
    pub plan_first: bool,
    /// The models a step's helpers may be started on, when a step asks for several (Codex's own list; Claude Code's
    /// are its aliases, named in its rules).
    pub helper_models: Vec<String>,
    /// Each squad copied beside the task: its name and its file's name (`write_task_files` fills it in). Codex's rules
    /// name them, so a step's "follow the Review squad" finds `review.md`.
    pub squad_files: Vec<(String, String)>,
    /// platoon by its full path, as the session runs it (`platoon task step`).
    pub exe: String,
    /// How many numbered steps the squad has (`steps::parse`): the session marks each one (none: nothing to mark).
    pub steps: usize,
}

/// The rule on tests: platoon runs none itself, so the session finds the repo's own and reports them (the check of the
/// work holds a change to code to it: `verdict::SYSTEM`).
pub const TESTS_RULE: &str = "- When you change code, find how this repo runs its tests (its CLAUDE.md, README, or build files), run them before you finish, and name the command and its result in your final message. If the repo has no tests, say so and how you checked the change instead.\n";

/// How a Codex task's session ends a reply that waits for the user's answer (Codex has no question tool platoon can
/// rely on: `request_user_input` is in its plan mode only): the last line starts with this, and the stop waits for the
/// answer instead of checking the work.
pub const QUESTION_MARK: &str = "Question for you:";

/// platoon's fixed rules, above the squad's steps in `squad.md`. Not the user's to change. The squads' steps say what
/// to do in plain words ("ask the user", "present the plan and wait", "if you can start helpers"); these say how, in
/// the agent's own ways.
pub fn rules(r: &Rules) -> String {
    let mut out = String::from("# How this task works\n\nYou are working on a task the user handed to platoon.\n\n");
    match &r.branch {
        // Codex can't commit in its sandbox (it keeps git's data read-only): platoon commits for it at each stop.
        Some(b) if r.agent == Agent::Codex => out.push_str(&format!(
            "- You work in {} on the branch `{b}`, a git worktree made for this task. Don't commit, and don't run git commands that write (add, commit, stash, checkout, switch, branch, merge, rebase, reset, tag): git's data is read-only for you here, and platoon commits what you changed on `{b}` each time you stop.\n\
             - The work stays on the branch `{b}`: it is not merged, and the user merges it. If you changed files, your final message says that the work is on the branch `{b}`, is not merged, and that platoon commits it when you stop: don't say it isn't committed. If you changed nothing, leave the branch out of it.\n",
            r.folder
        )),
        Some(b) => out.push_str(&format!(
            "- You work in {} on the branch `{b}`, a git worktree made for this task. Commit what you change there.\n\
             - What you commit stays on the branch `{b}`: it is not merged, and the user merges it. If you committed, your final message says so: that the work is on the branch `{b}` and is not merged. If you changed nothing, leave the branch out of it.\n",
            r.folder
        )),
        None => out.push_str(&format!("- You work in {}. It is not a git repository.\n", r.folder)),
    }
    match r.agent {
        Agent::Claude => {
            out.push_str(
                "- Never push, open or merge pull requests, or publish anything unless the user asks for it in this session.\n\
                 - Ask the user questions with the AskUserQuestion tool, not in prose.\n",
            );
            if r.plan_first {
                out.push_str(
                    "- You start in plan mode. Present the plan with ExitPlanMode; the user approves it before you change anything.\n",
                );
            }
            out.push_str(
                "- To start helpers (subagents), use the Agent tool with `subagent_type: \"general-purpose\"` and `model: \"opus\"` (`\"sonnet\"` for mechanical edits). When a step asks for helpers on several models, give them `\"opus\"`, `\"fable\"`, `\"sonnet\"` and `\"haiku\"`, one each; if a model is refused, use the nearest one and say so.\n",
            );
        }
        Agent::Codex => {
            out.push_str(
                "- Never push, open or merge pull requests, or publish anything unless the user asks for it in this session. Even then platoon holds the command back the first time and tells you so: stop, ask the user, and when they say yes, run exactly the same command again.\n",
            );
            out.push_str(&format!(
                "- Ask the user with your request_user_input tool when you have it. Otherwise end your reply with the question on its own last line, starting with \"{QUESTION_MARK}\", and stop there until the user answers.\n"
            ));
            if r.plan_first {
                out.push_str(
                    "- Plan first. Look around (read files, run commands that change nothing), then write the plan as your reply and stop: change no files until the user answers it. platoon holds back file edits until then. Their answer is their say on the plan: go ahead as they say.\n",
                );
            }
            let models = if r.helper_models.is_empty() { String::new() } else { format!(" (from {})", r.helper_models.join(", ")) };
            out.push_str(&format!(
                "- To start helpers (subagents), use your spawn_agent tool where you have it, give each one an exact scope, and wait for them all before you finish. When a step asks for helpers on several models, give each a different model{models} where you can. Without that tool, do each helper's part yourself, one pass each.\n"
            ));
        }
    }
    if r.steps > 0 {
        out.push_str(&steps_rule(r));
    }
    out.push_str("- When you finish, end with what you did and how you checked each \"Done when\" box of the task.\n");
    out.push_str(TESTS_RULE);
    out.push_str(match r.agent {
        Agent::Claude => "- Each time you finish, another Claude that can only read checks your work against the task and its \"Done when\" boxes. If it finds problems, you get them back (file:line first) and carry on.\n",
        Agent::Codex => "- Each time you finish, another AI that can only read checks your work against the task and its \"Done when\" boxes. If it finds problems, you get them back (file:line first) and carry on.\n",
    });
    let files = r.squad_files.iter().map(|(name, file)| format!("{name}: `{file}`")).collect::<Vec<_>>().join(", ");
    match r.agent {
        Agent::Codex if !files.is_empty() => out.push_str(&format!(
            "- The squads are in {}, one file each ({files}). When a step says to follow another squad, read its file and follow its steps.\n",
            r.squads_dir
        )),
        _ => out.push_str(&format!(
            "- The squads are in {}, one file each. When a step says to follow another squad, read its file and follow its steps.\n",
            r.squads_dir
        )),
    }
    out
}

/// The rule on the squad's steps: the session marks each one with `platoon task step` (by platoon's full path), so the
/// user sees where it is. Codex's sandbox can't write platoon's folder: its hook takes the command instead of running it.
fn steps_rule(r: &Rules) -> String {
    let exe = crate::agents::claude::shell_word(&r.exe);
    let mut out = format!(
        "- The user follows your progress through the squad's numbered steps below, 1 to {n}, counted in order. When you start a step, run `{exe} task step <n> started`. When you finish it, run `{exe} task step <n> done \"<one line: what you did>\"`; when you leave it out, run `{exe} task step <n> skipped \"<one line: why>\"`. Mark every step done or skipped before you finish. Only this squad's steps are marked: the steps of another squad a step tells you to follow are part of that step.",
        n = r.steps
    );
    if r.agent == Agent::Codex {
        out.push_str(" platoon takes this command itself instead of running it, and answers \"Recorded: ...\": that answer means it worked, so don't run it again.");
    }
    out.push('\n');
    out
}

/// Write `squad.md` (the rules, then the squad's steps) and a copy of every squad in `squads` into the task's folder.
pub fn write_task_files(task_dir: &Path, squad: &Squad, squads: &[Squad], r: &Rules) -> io::Result<()> {
    let dir = task_dir.join("squads");
    fs::create_dir_all(&dir)?;
    for s in squads.iter().filter(|s| s.usable()) {
        crate::projects::write_atomic(&dir.join(format!("{}.md", s.id)), &s.text())?;
    }
    let mut r = r.clone();
    r.squad_files = squads.iter().filter(|s| s.usable()).map(|s| (s.name_or_id(), format!("{}.md", s.id))).collect();
    let r = &r;
    let text = format!("{}\n# The squad: {}\n\n{}", rules(r), squad.name, squad.body);
    crate::projects::write_atomic(&task_dir.join(SQUAD_FILE), &text)?;
    // A Codex session can't be started in a plan mode: until the user answers its plan, platoon holds back its edits.
    if r.agent == Agent::Codex && r.plan_first {
        crate::projects::write_atomic(&task_dir.join(tickets::PLAN_PENDING_FILE), "")?;
    }
    Ok(())
}

// ---- the hooks -----------------------------------------------------------------------------------------------------

/// What a hook says back: the text for its stdout (None: nothing).
pub type Said = Option<String>;

/// Where a hook finds what it needs: platoon's folders, the programs the check of the work may run on, how long it may,
/// and the task the session's environment names (`agents::TASK_ENV`; Codex's task hooks act only with it).
pub struct Places {
    pub config: PathBuf,
    pub cache: PathBuf,
    /// The AI programs the check of the work may run on.
    pub providers: crate::agents::Providers,
    pub check_timeout: Duration,
    pub task: Option<String>,
    /// platoon by its full path, as the rules name it to the session (`platoon task step`).
    pub exe: String,
}

/// The kind of hook each of the task's hook commands is installed as (see `HOOKS`).
fn kind(name: &str) -> Option<HookKind> {
    Some(match name {
        "task-plan" => HookKind::BeforeTool(Tool::Plan),
        "task-question" => HookKind::BeforeTool(Tool::Question),
        "task-before" => HookKind::BeforeAnyTool,
        "task-notify" => HookKind::PermissionWait,
        "task-tool" => HookKind::AfterTool,
        "task-prompt" => HookKind::Prompt,
        "task-stop" => HookKind::Stop,
        "task-start" => HookKind::SessionStart,
        _ => return None,
    })
}

/// What the task needs while a command it wants to run is held back for the user.
fn held_words(command: &str) -> String {
    tickets::clean(&format!("It wants to run `{command}`: say yes in its session to let it, or tell it not to."), tickets::WHY_MAX)
}

/// Why a command that pushes or publishes didn't run (Codex's hook; Claude Code asks with its own prompt).
fn held_reason(command: &str) -> String {
    format!(
        "platoon held this back: `{command}` pushes or publishes, and that waits for the user. Stop here and ask the user \
         whether to run it (end your reply with a line starting \"{QUESTION_MARK}\"). If they say yes, run exactly the same \
         command again; it goes through once. If they say no, don't run it."
    )
}

/// Why an edit didn't run while the plan waits.
const PLAN_WAITS: &str = "platoon held this back: this task plans first, and the user hasn't answered the plan yet. \
Change no files: write the plan as your reply and stop. Once the user answers, go ahead as they say.";

/// Run hook `name` of `agent` on its JSON `input`: what it says back. Claude Code's task hooks are given to a task's
/// session only, and find the task by the session; Codex's run in every Codex session, and act only where the
/// session's environment names a task (`at.task`). Nothing happens for a session that is no task's.
pub fn hook(name: &str, agent: Agent, input: &str, at: &Places) -> Said {
    let config = at.config.as_path();
    let input = agent.hook_input(kind(name)?, input)?;
    let (project, number) = match agent {
        Agent::Claude => tickets::find_session(config, &input.session)?,
        Agent::Codex => tickets::parse_tag(at.task.as_deref()?)?,
    };
    let mut store = Tickets::open(config, &project);
    let t = store.get(number)?.clone();
    if matches!(t.state, State::NotStarted | State::Cancelled) {
        return None;
    }
    if agent == Agent::Codex {
        match codex_first(&input, at, &project, &mut store, &t) {
            Gate::Answer(said) => return said,
            Gate::Own => {}
        }
    }
    let task_dir = tickets::task_dir(config, &project, number);
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
            if agent == Agent::Codex {
                // The user's answer: to the plan (edits may start now), and to a command held back (it may run once).
                if t.state == State::NeedsYou(Need::Plan) {
                    let _ = fs::remove_file(task_dir.join(tickets::PLAN_PENDING_FILE));
                }
                held::answered(&task_dir);
            }
            let _ = store.update(number, |t| {
                let reopen = matches!(t.state, State::NeedsYou(_) | State::Done);
                if !reopen && t.checks_refused == 0 && t.checking.is_none() && !t.steps.sent_back {
                    return false;
                }
                if reopen {
                    t.state = State::Working;
                }
                t.checks_refused = 0;
                t.steps.sent_back = false;
                // A check still running is of the work before this message: its answer no longer moves the task.
                t.checking = None;
                true
            });
        }
        HookEvent::Stop(end) => {
            if agent == Agent::Codex {
                held::turn_ended(&task_dir);
                if let Some(failed) = commit_for_codex(&t, &mut store) {
                    return failed;
                }
                if let Some(waits) = codex_stop(&end, &task_dir, config, &project, &mut store, &t) {
                    return waits;
                }
            }
            let reason = stop(&end, at, &project, &mut store, &t)?;
            return Some(agent.hook_answer(&HookReply::SendBack(reason)).stdout);
        }
        HookEvent::BeforeTool { tool: Tool::Shell | Tool::Edit | Tool::Other, .. }
        | HookEvent::SessionStart { .. }
        | HookEvent::StartsWork { .. } => {}
    }
    None
}

/// What a Codex task hook does before the steps every agent shares.
enum Gate {
    /// Done: say this (or nothing).
    Answer(Said),
    /// The task's own session, with an event the shared steps take.
    Own,
}

/// A Codex task's hook, first: its session's id written on the task (`SessionStart`); a tool about to run held back
/// when it pushes or publishes, or edits while the plan waits (whichever session of the task's process asks: a
/// helper's too); and anything else only for the task's own session.
fn codex_first(input: &HookInput, at: &Places, project: &str, store: &mut Tickets, t: &Ticket) -> Gate {
    let task_dir = tickets::task_dir(&at.config, project, t.number);
    match &input.event {
        HookEvent::SessionStart { .. } => {
            if store.record_session(t.number, &input.session).unwrap_or(false) && project != tickets::NO_PROJECT_ID {
                let _ = crate::projects::Projects::open(&at.config).place(&input.session, project);
            }
            Gate::Answer(None)
        }
        HookEvent::BeforeTool { tool, said } => {
            let plan_waits = task_dir.join(tickets::PLAN_PENDING_FILE).exists();
            let refuse = |why: String| Gate::Answer(Some(Agent::Codex.hook_answer(&HookReply::Deny(why)).stdout));
            match tool {
                Tool::Edit if plan_waits => return refuse(PLAN_WAITS.into()),
                Tool::Shell if plan_waits && crate::askmatch::runs_any(said, &["apply_patch", "applypatch"]) => {
                    return refuse(PLAN_WAITS.into());
                }
                Tool::Shell if !step_commands(said, &at.exe).0.is_empty() => {
                    return refuse(record_steps(said, &at.exe, store, t.number));
                }
                Tool::Shell => {
                    if let Some(command) = crate::askmatch::waits(said) {
                        if held::take_if_allowed(&task_dir, said) {
                            return Gate::Answer(None);
                        }
                        held::hold(&task_dir, said);
                        let need = State::NeedsYou(Need::Question(held_words(&command)));
                        let _ = store.update(t.number, |x| {
                            let change = x.state != need;
                            x.state = need.clone();
                            change
                        });
                        return refuse(held_reason(&command));
                    }
                }
                _ => {}
            }
            if t.session_id() == Some(input.session.as_str()) && *tool == Tool::Question { Gate::Own } else { Gate::Answer(None) }
        }
        _ if t.session_id() == Some(input.session.as_str()) => Gate::Own,
        _ => Gate::Answer(None),
    }
}

/// The runs of `platoon task step ...` (platoon by its path, `exe`) in the shell line `said`, each as the words after
/// `step`, and whether the line runs anything else too.
fn step_commands(said: &str, exe: &str) -> (Vec<Vec<String>>, bool) {
    let (runs, mut others) = crate::askmatch::runs_of(said, exe);
    let mut steps = Vec::new();
    for words in runs {
        match words.as_slice() {
            [task, step, rest @ ..] if task == "task" && step == "step" => steps.push(rest.to_vec()),
            _ => others = true,
        }
    }
    (steps, others)
}

/// A Codex task's `platoon task step ...` (Codex's sandbox can't write platoon's folder): each one in the shell line
/// `said` is recorded here instead, and the command is refused with what was recorded, so it never runs and never asks
/// for an approval. Anything else on the same line didn't run either, and the answer says so.
fn record_steps(said: &str, exe: &str, store: &mut Tickets, number: u32) -> String {
    let (runs, others) = step_commands(said, exe);
    let mut out: Vec<String> = runs
        .iter()
        .map(|args| {
            let marked = crate::steps::parse_command(args)
                .and_then(|(n, report)| store.mark_step(number, n, &report).map(|()| crate::steps::said(n, &report)));
            match marked {
                Ok(said) => format!("Recorded: {}", said.replacen("Step", "step", 1)),
                Err(why) => format!("Not recorded: {why}. The command is {}.", crate::steps::USAGE),
            }
        })
        .collect();
    if others {
        out.push("Nothing else on that command line ran: run the rest as a command of its own.".into());
    }
    out.join(" ")
}

/// The commit message of a Codex task's work: its number and title.
pub fn commit_message(t: &Ticket) -> String {
    format!("#{} {}", t.number, t.title)
}

/// A Codex task's stop, first: what it changed in its worktree is committed on its branch (`taskcommit.rs`: Codex's
/// sandbox can't), whatever the stop turns out to be (plan, question, the end of the work), so the check sees it in the
/// branch's diff and nothing is left only in the worktree; nothing changed, no commit. A commit that couldn't be made
/// makes the task need the user with why (the work stays in the worktree) and the stop goes through unchecked: Some
/// with what the hook says (nothing). None: committed, or nothing to commit, or no worktree (a folder not in git).
fn commit_for_codex(t: &Ticket, store: &mut Tickets) -> Option<Said> {
    let w = t.worktree.as_ref().filter(|w| !w.is_gone())?;
    match crate::taskcommit::commit_all("git", Path::new(&w.path), &t.branch(), &commit_message(t), crate::taskcommit::BUDGET) {
        crate::taskcommit::Committed::Failed(why) => {
            let why = tickets::clean(&why, tickets::WHY_MAX);
            let _ = store.update(t.number, |x| {
                x.state = State::NeedsYou(Need::CommitFailed(why.clone()));
                x.checking = None;
                true
            });
            Some(None)
        }
        _ => None,
    }
}

/// A Codex task's stop, before the check: while its plan waits for the user, the reply is the plan (written as
/// `plan.md`; Needs you, a plan to approve), and a reply that ends with a question for the user (`QUESTION_MARK`) waits
/// for the answer. Some when one of those, with what the hook says (nothing); None: the check goes on. A command the user
/// let through once no longer is.
fn codex_stop(end: &TurnEnd, task_dir: &Path, config: &Path, project: &str, store: &mut Tickets, t: &Ticket) -> Option<Said> {
    if t.state != State::Working {
        return None;
    }
    let said = end.last_message.as_deref().unwrap_or_default();
    if task_dir.join(tickets::PLAN_PENDING_FILE).exists() {
        if !said.trim().is_empty() {
            let file = tickets::plan_file(config, project, t.number);
            let _ = crate::projects::write_atomic(&file, said);
        }
        let _ = store.update(t.number, |x| {
            x.state = State::NeedsYou(Need::Plan);
            true
        });
        return Some(None);
    }
    let last = said.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or_default();
    let q = last.trim_start_matches(['*', '_', '#', ' ', '>']).strip_prefix(QUESTION_MARK)?.trim_start_matches(['*', '_', ' ']).trim();
    let q = if q.is_empty() { "It asked you something: see its session.".to_string() } else { tickets::clean(q, tickets::WHY_MAX) };
    let _ = store.update(t.number, |x| {
        x.state = State::NeedsYou(Need::Question(q.clone()));
        true
    });
    Some(None)
}

/// A command held back for the user (`tickets::HELD_FILE` in the task's own folder): the command as Codex gave it,
/// and whether the user has typed since. The user's next message lets that exact command through once; the end of
/// the turn after it takes that away again.
mod held {
    use serde_json::{Value, json};
    use std::{fs, path::Path};

    fn read(task_dir: &Path) -> Option<(String, bool)> {
        let v: Value = serde_json::from_str(&fs::read_to_string(task_dir.join(crate::tickets::HELD_FILE)).ok()?).ok()?;
        Some((v.get("command")?.as_str()?.to_string(), v.get("answered").and_then(Value::as_bool).unwrap_or(false)))
    }

    fn write(task_dir: &Path, command: &str, answered: bool) {
        let _ = fs::create_dir_all(task_dir);
        let text = json!({"command": command, "answered": answered}).to_string();
        let _ = crate::projects::write_atomic(&task_dir.join(crate::tickets::HELD_FILE), &text);
    }

    pub fn hold(task_dir: &Path, command: &str) {
        write(task_dir, command, false);
    }

    /// The user typed: a command held back may now run once.
    pub fn answered(task_dir: &Path) {
        if let Some((command, false)) = read(task_dir) {
            write(task_dir, &command, true);
        }
    }

    /// Is `command` the one held back, answered since? Then it runs, and only this once.
    pub fn take_if_allowed(task_dir: &Path, command: &str) -> bool {
        let ok = read(task_dir).is_some_and(|(held, answered)| answered && held == command);
        if ok {
            let _ = fs::remove_file(task_dir.join(crate::tickets::HELD_FILE));
        }
        ok
    }

    /// A turn ended: a command the user let through and that wasn't run again is held back again next time.
    pub fn turn_ended(task_dir: &Path) {
        if let Some((_, true)) = read(task_dir) {
            let _ = fs::remove_file(task_dir.join(crate::tickets::HELD_FILE));
        }
    }
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
    if let Some(reason) = unmarked_steps(at, store, t) {
        return Some(reason);
    }
    check(end, at, project, store, t.number)
}

/// The end of the work with steps never marked done or skipped: the session is sent back once in this round of work to
/// mark them, before any tokens are spent on the check (Some, its reason). The next stop goes through to the check.
fn unmarked_steps(at: &Places, store: &mut Tickets, t: &Ticket) -> Option<String> {
    let unmarked = t.steps.unmarked();
    if unmarked.is_empty() || t.steps.sent_back {
        return None;
    }
    let _ = store.update(t.number, |x| {
        x.steps.sent_back = true;
        true
    });
    Some(crate::steps::send_back(&t.steps, &unmarked, &at.exe))
}

/// The model a task's work is checked with: the `verdict_model` of the squad it started with (its copy in the task's
/// own folder, as it ran: `squads::Squad::as_run`, else the squad as it is now, the same way), else that squad's own
/// model.
pub fn verdict_model(config: &Path, project: &str, t: &Ticket, catalog: &crate::agents::Catalog) -> String {
    let copy = tickets::task_dir(config, project, t.number).join("squads").join(format!("{}.md", t.squad));
    let squad = fs::read_to_string(&copy)
        .ok()
        .map(|text| crate::squads::parse(&t.squad, &text, crate::squads::Origin::Own))
        .or_else(|| crate::squads::get(config, &t.squad).map(|s| s.as_run(catalog)));
    let model = squad
        .map(|s| if s.verdict_model.is_empty() { s.model } else { s.verdict_model })
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "opus".into());
    catalog.stand_in_for(&model).filter(|_| model == "opus").unwrap_or(model)
}

/// The end of a turn: check the work (`verdict.rs`). Done, refused once with what it found, or
/// Needs you. While it runs the task is Working with `checking` set; a message the user types meanwhile clears that,
/// and then the answer is kept but moves nothing.
fn check(end: &TurnEnd, at: &Places, project: &str, store: &mut Tickets, number: u32) -> Option<String> {
    let started = crate::usage::now();
    let model = verdict_model(&at.config, project, store.get(number)?, &at.providers.catalog());
    let _ = store.update(number, |x| {
        x.checking = Some(tickets::Checking { since: started, model: model.clone() });
        true
    });
    let t = store.get(number)?.clone();
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
        let ours = x.checking.as_ref().is_some_and(|c| c.since == started);
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

/// `platoon statusline` in a task's Claude Code session (its `--settings` names it, run every couple of seconds): the
/// task's line (`steps::status_line`) for the session the payload `input` names, with the plan usage (`usage`, the line
/// every session gets) at its end, in `width` cells. None when the session is no task's. Cheap: the tasks files, the
/// squad's copy beside the task and the session's helpers' small files (`claude::helpers`), nothing else.
pub fn status_line(input: &str, config: &Path, usage: &str, width: usize, now: u64) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(input).ok()?;
    let session = v.get("session_id")?.as_str()?;
    let (project, number) = tickets::find_session(config, session)?;
    let store = Tickets::open(config, &project);
    let t = store.get(number)?;
    let copy = tickets::task_dir(config, &project, number).join("squads").join(format!("{}.md", t.squad));
    let squad = fs::read_to_string(&copy)
        .ok()
        .map(|text| crate::squads::parse(&t.squad, &text, crate::squads::Origin::Own))
        .or_else(|| crate::squads::get(config, &t.squad))
        .map(|s| s.name_or_id())
        .unwrap_or_else(|| t.squad.clone());
    let text = |p: &str| v.pointer(p).and_then(serde_json::Value::as_str).filter(|s| !s.is_empty());
    let model = text("/model/display_name")
        .map(String::from)
        .or_else(|| text("/model/id").map(crate::agents::model_display_name))
        .unwrap_or_default();
    let helpers = text("/transcript_path").map(|p| crate::agents::claude::helpers(Path::new(p))).unwrap_or_default();
    let helpers = crate::steps::helpers_now(&helpers, t.steps.last_marked(), now);
    let checking = t.checking.as_ref().filter(|_| crate::board::checking(t, now)).map(|c| (c.model.as_str(), now.saturating_sub(c.since)));
    let line = crate::steps::StatusLine { number, squad: &squad, steps: &t.steps, model: &model, helpers: &helpers, checking, usage };
    Some(crate::steps::status_line(&line, width))
}

/// `platoon hook <name>` (of `agent`): always 0, with what the hook says on stdout. It never fails the session. A
/// Codex task hook in a session that is no task's (most of them: these hooks are every Codex session's) ends at once,
/// without reading anything, except `task-before` in a session platoon started: its edit may be the start of the work,
/// which platoon offers to make tasks of (`offerhook.rs`).
pub fn run_hook(name: &str, agent: Agent) -> i32 {
    let task = std::env::var(crate::agents::TASK_ENV).ok().filter(|t| !t.is_empty());
    if agent == Agent::Codex && task.is_none() {
        return if name == "task-before" { crate::offerhook::run(agent) } else { 0 };
    }
    let said = std::panic::catch_unwind(|| {
        let mut input = String::new();
        std::io::stdin().take(1 << 20).read_to_string(&mut input).ok()?;
        let at = Places {
            config: crate::archive::config_dir(),
            cache: crate::usage::cache_dir(),
            // A task already started keeps working when its program is switched off in platoon's settings: its hooks
            // and the check of its work run on what this computer has, whatever the switches say.
            providers: crate::agents::Providers::installed_here(),
            check_timeout: verdict::TIMEOUT,
            task: task.clone(),
            exe: crate::offerhook::this_platoon(),
        };
        hook(name, agent, &input, &at)
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
cat > "$D/check-stdin"
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
            providers: crate::agents::Providers { claude: Some(script.to_string_lossy().into_owned()), codex: None, off: Vec::new() },
            check_timeout: Duration::from_secs(20),
            task: None,
            exe: "/opt/platoon/bin/platoon".into(),
        }
    }

    /// The hook, with the stand-in checker.
    fn hook(name: &str, input: &str, d: &Path) -> Said {
        super::hook(name, Agent::Claude, input, &places(d))
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
        store.start(n, "feature", "s-1", None, &[]).unwrap();
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
            instructions: "the rules".into(),
            tag: "5:p1".into(),
            checker: "Fable".into(),
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
        for h in &HOOKS[..6] {
            assert!(s.to_string().contains(&format!("hook {h} ")) || s.to_string().contains(&format!("hook {h}\"")), "{h}");
        }
        assert!(!s.to_string().contains("task-start") && !s.to_string().contains("task-before"), "Codex's only");
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
            ..Default::default()
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
        assert_eq!(super::hook("task-stop", Agent::Claude, &input("Stop", json!({})), &at), None);
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

    // ---- a task's session on Codex ----

    /// A project, and a task of it started on Codex in `<d>/work`: its session not yet known (`tickets::PENDING`).
    fn codex_world(name: &str) -> (PathBuf, String, u32) {
        let d = dir(name);
        let p = crate::projects::Projects::open(&d).create("Shop").unwrap();
        let folder = d.join("work");
        fs::create_dir_all(&folder).unwrap();
        let mut store = Tickets::open(&d, &p);
        let n = store
            .add(NewTicket { title: "Login".into(), folder: Some(folder.to_string_lossy().into_owned()), ..Default::default() })
            .unwrap();
        let pending = format!("{}{}", tickets::PENDING, tickets::task_tag(&p, n));
        store.start(n, "feature", &pending, None, &[]).unwrap();
        (d, p, n)
    }

    /// Codex's hook `name` for the task (`PLATOON_TASK` names it), with the stand-in checker.
    fn chook(name: &str, input: &str, d: &Path, p: &str, n: u32) -> Said {
        let at = Places { task: Some(tickets::task_tag(p, n)), ..places(d) };
        super::hook(name, Agent::Codex, input, &at)
    }

    /// One of Codex's hook inputs for session `session`, with Codex's field names.
    fn cinput(session: &str, event: &str, extra: Value) -> String {
        let mut v = json!({"session_id": session, "turn_id": "t1", "transcript_path": null, "cwd": "/w", "hook_event_name": event,
            "model": "gpt-6-luna", "permission_mode": "default"});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v.to_string()
    }

    const CODEX_ID: &str = "01a1aaaa-0000-7000-8000-000000000001";

    fn shell(command: &str) -> Value {
        json!({"tool_name": "Bash", "tool_input": {"command": command}, "tool_use_id": "c1"})
    }

    fn denied(said: &Said) -> Option<String> {
        let v: Value = serde_json::from_str(said.as_ref()?).ok()?;
        (v["hookSpecificOutput"]["permissionDecision"] == "deny")
            .then(|| v["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().to_string())
    }

    #[test]
    fn a_codex_task_learns_its_session_from_its_first_hook_and_a_session_that_is_no_tasks_is_left_alone() {
        let (d, p, n) = codex_world("codex-start");
        let start = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/testdata/codex/hooks/SessionStart.json")).unwrap();
        // Without the task in its environment (any other Codex session): nothing, said or written.
        assert_eq!(super::hook("task-start", Agent::Codex, &start, &places(&d)), None);
        assert!(task(&d, &p, n).session_id().is_none());
        assert_eq!(chook("task-start", &start, &d, &p, n), None);
        let t = task(&d, &p, n);
        assert_eq!(t.session_id(), Some("01a15181-dc6f-7b73-81cf-c041b14f6432"), "the real Codex 0.163 SessionStart's id");
        assert_eq!(crate::projects::Projects::open(&d).of("01a15181-dc6f-7b73-81cf-c041b14f6432").map(|x| x.id.clone()), Some(p.clone()));
        // A later start (the user's /new, or resumed) doesn't take the task over.
        chook("task-start", &cinput("someone-else", "SessionStart", json!({"source": "startup"})), &d, &p, n);
        assert_eq!(task(&d, &p, n).session_id(), Some("01a15181-dc6f-7b73-81cf-c041b14f6432"));
        // Another task's tag, a tag of no task, or one that isn't one: nothing.
        for tag in ["99:".to_string(), format!("{n}:no-such-project"), "nonsense".into(), String::new()] {
            let at = Places { task: Some(tag.clone()), ..places(&d) };
            assert_eq!(super::hook("task-stop", Agent::Codex, &cinput(CODEX_ID, "Stop", json!({})), &at), None, "{tag}");
        }
        assert_eq!(checks_made(&d), 0);
    }

    #[test]
    fn codexs_events_move_its_task_as_claude_codes_do() {
        let (d, p, n) = codex_world("codex-events");
        chook("task-start", &cinput(CODEX_ID, "SessionStart", json!({"source": "startup"})), &d, &p, n);
        // Its question tool: Needs you with its words, until the tool is done.
        let q = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/testdata/codex/hooks/PreToolUse.json"))
            .unwrap()
            .replace("01a15181-dc6f-7b73-81cf-c041b14f6432", CODEX_ID);
        assert_eq!(chook("task-before", &q, &d, &p, n), None);
        assert_eq!(state(&d, &p, n), State::NeedsYou(Need::Question("What should the second line of notes.txt say?".into())));
        chook("task-tool", &cinput(CODEX_ID, "PostToolUse", json!({"tool_name": "request_user_input", "tool_response": "{}"})), &d, &p, n);
        assert_eq!(state(&d, &p, n), State::Working);
        // A permission prompt: Needs you with Codex's reason, until a tool runs.
        let perm = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/testdata/codex/hooks/PermissionRequest.json"))
            .unwrap()
            .replace("01a15181-5d10-7fc3-8da2-6d27c8100fb6", CODEX_ID);
        chook("task-notify", &perm, &d, &p, n);
        assert!(matches!(state(&d, &p, n), State::NeedsYou(Need::Permission(why)) if why.starts_with("May I rerun the exact curl")));
        chook("task-tool", &cinput(CODEX_ID, "PostToolUse", shell("curl -sS https://example.com")), &d, &p, n);
        assert_eq!(state(&d, &p, n), State::Working);
        // Another session's events (a helper's, another conversation in the same window) move nothing.
        chook("task-notify", &perm.replace(CODEX_ID, "helper-thread"), &d, &p, n);
        assert_eq!(state(&d, &p, n), State::Working);
        // The stop: checked; not done sends it back once, in Codex's words, then Needs you; typing starts again.
        fs::write(d.join("check-reply"), r#"{"done":false,"boxes":[{"text":"It logs in","met":false,"evidence":"none"}],"findings":[]}"#)
            .unwrap();
        let first = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/testdata/codex/hooks/Stop-first.json")).unwrap();
        let said = chook("task-stop", &first, &d, &p, n);
        assert_eq!(said, None, "the sample is another session's: no check");
        let said = chook("task-stop", &first.replace("01a1517c-414d-7902-93d1-efa74830493d", CODEX_ID), &d, &p, n).expect("sent back");
        let v: Value = serde_json::from_str(&said).unwrap();
        assert_eq!(v["decision"], "block");
        assert!(v["reason"].as_str().unwrap().contains("\"It logs in\": not met"), "{said}");
        let again = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/testdata/codex/hooks/Stop-after-block.json"))
            .unwrap()
            .replace("01a1517c-414d-7902-93d1-efa74830493d", CODEX_ID);
        assert_eq!(chook("task-stop", &again, &d, &p, n), None);
        assert!(matches!(state(&d, &p, n), State::NeedsYou(Need::Check(_))));
        assert_eq!(checks_made(&d), 2);
        chook("task-prompt", &cinput(CODEX_ID, "UserPromptSubmit", json!({"prompt": "look again"})), &d, &p, n);
        assert_eq!(state(&d, &p, n), State::Working);
        let _ = fs::remove_file(d.join("check-reply"));
        assert_eq!(chook("task-stop", &cinput(CODEX_ID, "Stop", json!({"last_assistant_message": "Done."})), &d, &p, n), None);
        assert_eq!(state(&d, &p, n), State::Done);
    }

    #[test]
    fn a_plan_first_codex_task_writes_its_plan_stops_and_edits_wait_for_the_users_answer() {
        let (d, p, n) = codex_world("codex-plan");
        let task_dir = tickets::task_dir(&d, &p, n);
        let all = crate::squads::all(&d);
        let r = Rules { agent: Agent::Codex, folder: "/w".into(), plan_first: true, squads_dir: "/s".into(), ..Default::default() };
        write_task_files(&task_dir, &all[0], &all, &r).unwrap();
        assert!(task_dir.join(tickets::PLAN_PENDING_FILE).exists());
        chook("task-start", &cinput(CODEX_ID, "SessionStart", json!({"source": "startup"})), &d, &p, n);
        let patch =
            fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/testdata/codex/hooks/PreToolUse-apply_patch.from-source.json"))
                .unwrap();
        // Looking around goes through; an edit doesn't, whichever way it comes (a helper's too).
        assert_eq!(chook("task-before", &cinput(CODEX_ID, "PreToolUse", shell("rg -n login src")), &d, &p, n), None);
        let why = denied(&chook("task-before", &patch, &d, &p, n)).expect("an edit waits for the plan's answer");
        assert!(why.contains("hasn't answered the plan") && why.contains("write the plan as your reply and stop"), "{why}");
        assert!(
            denied(&chook(
                "task-before",
                &cinput("helper", "PreToolUse", shell("cd /w && apply_patch <<'EOF'\n*** Begin Patch\nEOF")),
                &d,
                &p,
                n
            ))
            .is_some()
        );
        // The stop with the plan: Needs you, the plan kept, no check.
        let plan = "1. Add the form.\n2. Test it.";
        assert_eq!(chook("task-stop", &cinput(CODEX_ID, "Stop", json!({"last_assistant_message": plan})), &d, &p, n), None);
        assert_eq!(state(&d, &p, n), State::NeedsYou(Need::Plan));
        assert_eq!(Tickets::open(&d, &p).plan(n).as_deref(), Some(plan));
        assert_eq!(checks_made(&d), 0, "a plan isn't checked as work");
        // The user answers: Working, and edits go through from now on; the next stop is checked.
        chook("task-prompt", &cinput(CODEX_ID, "UserPromptSubmit", json!({"prompt": "go ahead"})), &d, &p, n);
        assert_eq!(state(&d, &p, n), State::Working);
        assert!(!task_dir.join(tickets::PLAN_PENDING_FILE).exists());
        assert_eq!(chook("task-before", &patch, &d, &p, n), None);
        assert_eq!(chook("task-stop", &cinput(CODEX_ID, "Stop", json!({"last_assistant_message": "Built it."})), &d, &p, n), None);
        assert_eq!((state(&d, &p, n), checks_made(&d)), (State::Done, 1));
        // A Claude Code task's files have no such mark: its plan mode is Claude Code's own.
        let claude_dir = d.join("claude-task");
        write_task_files(&claude_dir, &all[0], &all, &Rules { plan_first: true, ..r }).unwrap();
        let _ = &claude_dir;
    }

    #[test]
    fn typing_before_the_plan_comes_is_no_answer_to_it() {
        let (d, p, n) = codex_world("codex-plan-early");
        let task_dir = tickets::task_dir(&d, &p, n);
        let all = crate::squads::all(&d);
        write_task_files(&task_dir, &all[0], &all, &Rules { agent: Agent::Codex, plan_first: true, ..Default::default() }).unwrap();
        chook("task-start", &cinput(CODEX_ID, "SessionStart", json!({"source": "startup"})), &d, &p, n);
        chook("task-prompt", &cinput(CODEX_ID, "UserPromptSubmit", json!({"prompt": "also look at the signup page"})), &d, &p, n);
        assert!(task_dir.join(tickets::PLAN_PENDING_FILE).exists(), "still waiting for a plan to answer");
        assert!(
            denied(&chook(
                "task-before",
                &cinput(CODEX_ID, "PreToolUse", json!({"tool_name": "apply_patch", "tool_input": {"command": "x"}})),
                &d,
                &p,
                n
            ))
            .is_some()
        );
    }

    #[test]
    fn pushing_waits_for_the_users_yes_in_the_session_and_then_runs_once() {
        let (d, p, n) = codex_world("codex-push");
        chook("task-start", &cinput(CODEX_ID, "SessionStart", json!({"source": "startup"})), &d, &p, n);
        let push = "git -C /w push -u origin task/1";
        let why = denied(&chook("task-before", &cinput(CODEX_ID, "PreToolUse", shell(push)), &d, &p, n)).expect("held back");
        assert!(
            why.contains("`git push -u origin task/1` pushes or publishes") && why.contains("run exactly the same command again"),
            "{why}"
        );
        assert_eq!(
            state(&d, &p, n),
            State::NeedsYou(Need::Question(
                "It wants to run `git push -u origin task/1`: say yes in its session to let it, or tell it not to.".into()
            ))
        );
        // Its turn ends asking: no check; trying again before the user answers is held back again.
        assert_eq!(
            chook("task-stop", &cinput(CODEX_ID, "Stop", json!({"last_assistant_message": "Question for you: push it?"})), &d, &p, n),
            None
        );
        assert_eq!(checks_made(&d), 0);
        assert!(denied(&chook("task-before", &cinput(CODEX_ID, "PreToolUse", shell(push)), &d, &p, n)).is_some());
        // The user says yes: that very command goes through, once; another one is held back.
        chook("task-prompt", &cinput(CODEX_ID, "UserPromptSubmit", json!({"prompt": "yes, push it"})), &d, &p, n);
        assert_eq!(state(&d, &p, n), State::Working);
        assert!(denied(&chook("task-before", &cinput(CODEX_ID, "PreToolUse", shell("git push --force")), &d, &p, n)).is_some());
        chook("task-prompt", &cinput(CODEX_ID, "UserPromptSubmit", json!({"prompt": "yes"})), &d, &p, n);
        assert_eq!(chook("task-before", &cinput(CODEX_ID, "PreToolUse", shell("git push --force")), &d, &p, n), None, "the one answered");
        assert!(denied(&chook("task-before", &cinput(CODEX_ID, "PreToolUse", shell("git push --force")), &d, &p, n)).is_some(), "once");
        // A yes not used in the turn after it is gone when that turn ends.
        chook("task-prompt", &cinput(CODEX_ID, "UserPromptSubmit", json!({"prompt": "ok"})), &d, &p, n);
        chook("task-stop", &cinput(CODEX_ID, "Stop", json!({"last_assistant_message": "Question for you: really?"})), &d, &p, n);
        chook("task-prompt", &cinput(CODEX_ID, "UserPromptSubmit", json!({"prompt": "later"})), &d, &p, n);
        chook("task-stop", &cinput(CODEX_ID, "Stop", json!({"last_assistant_message": "Question for you: now?"})), &d, &p, n);
        assert!(denied(&chook("task-before", &cinput(CODEX_ID, "PreToolUse", shell("git push --force")), &d, &p, n)).is_some());
        // Whichever session of the task's process runs it (a helper's), and whatever the shell around it.
        for cmd in ["bash -lc 'cd /w && git push'", "gh pr create --fill", "npm publish"] {
            assert!(denied(&chook("task-before", &cinput("a-helper", "PreToolUse", shell(cmd)), &d, &p, n)).is_some(), "{cmd}");
        }
        // Everything else goes through, saying nothing.
        for cmd in ["git commit -m 'push the login'", "cargo test", "echo git push"] {
            assert_eq!(chook("task-before", &cinput(CODEX_ID, "PreToolUse", shell(cmd)), &d, &p, n), None, "{cmd}");
        }
    }

    #[test]
    fn a_codex_reply_ending_with_a_question_waits_for_the_answer_without_a_check() {
        let (d, p, n) = codex_world("codex-question");
        chook("task-start", &cinput(CODEX_ID, "SessionStart", json!({"source": "startup"})), &d, &p, n);
        let said = "I looked at both pages.\n\n**Question for you:** Should the button be red or blue?\n";
        assert_eq!(chook("task-stop", &cinput(CODEX_ID, "Stop", json!({"last_assistant_message": said})), &d, &p, n), None);
        assert_eq!(state(&d, &p, n), State::NeedsYou(Need::Question("Should the button be red or blue?".into())));
        assert_eq!(checks_made(&d), 0);
        // A question in the middle of a finished reply is no question for the user: the work is checked.
        chook("task-prompt", &cinput(CODEX_ID, "UserPromptSubmit", json!({"prompt": "blue"})), &d, &p, n);
        let done = "Why red? Blue reads better.\nThe button is blue now. cargo test: 3 passed.";
        chook("task-stop", &cinput(CODEX_ID, "Stop", json!({"last_assistant_message": done})), &d, &p, n);
        assert_eq!((state(&d, &p, n), checks_made(&d)), (State::Done, 1));
    }

    #[test]
    fn the_rules_say_how_in_each_programs_own_ways() {
        let base = Rules {
            folder: "/w/shop-worktrees/task-5".into(),
            branch: Some("task/5".into()),
            squads_dir: "/c/squads".into(),
            plan_first: true,
            ..Default::default()
        };
        // Claude Code's: as they were, with one line on starting helpers (the squads no longer name its tool and
        // models).
        let claude = rules(&base);
        assert_eq!(
            claude,
            "# How this task works\n\nYou are working on a task the user handed to platoon.\n\n\
             - You work in /w/shop-worktrees/task-5 on the branch `task/5`, a git worktree made for this task. Commit what you change there.\n\
             - What you commit stays on the branch `task/5`: it is not merged, and the user merges it. If you committed, your final message says so: that the work is on the branch `task/5` and is not merged. If you changed nothing, leave the branch out of it.\n\
             - Never push, open or merge pull requests, or publish anything unless the user asks for it in this session.\n\
             - Ask the user questions with the AskUserQuestion tool, not in prose.\n\
             - You start in plan mode. Present the plan with ExitPlanMode; the user approves it before you change anything.\n\
             - To start helpers (subagents), use the Agent tool with `subagent_type: \"general-purpose\"` and `model: \"opus\"` (`\"sonnet\"` for mechanical edits). When a step asks for helpers on several models, give them `\"opus\"`, `\"fable\"`, `\"sonnet\"` and `\"haiku\"`, one each; if a model is refused, use the nearest one and say so.\n\
             - When you finish, end with what you did and how you checked each \"Done when\" box of the task.\n"
                .to_string()
                + TESTS_RULE
                + "- Each time you finish, another Claude that can only read checks your work against the task and its \"Done when\" boxes. If it finds problems, you get them back (file:line first) and carry on.\n\
                   - The squads are in /c/squads, one file each. When a step says to follow another squad, read its file and follow its steps.\n"
        );
        // Codex's: its own question tool or a last line, the plan as a reply, its own helpers, pushing held back.
        let codex = rules(&Rules {
            agent: Agent::Codex,
            helper_models: vec!["gpt-6-luna".into(), "gpt-5.6-terra".into()],
            squad_files: vec![("Feature".into(), "feature.md".into()), ("Plan first".into(), "planfirst.md".into())],
            ..base.clone()
        });
        for want in [
            "Don't commit, and don't run git commands that write (add, commit, stash",
            "platoon commits what you changed on `task/5` each time you stop",
            "If you changed files, your final message says that the work is on the branch `task/5`, is not merged, and that platoon commits it when you stop: don't say it isn't committed.",
            "one file each (Feature: `feature.md`, Plan first: `planfirst.md`)",
            "request_user_input tool when you have it",
            "starting with \"Question for you:\"",
            "write the plan as your reply and stop: change no files until the user answers it",
            "spawn_agent tool where you have it",
            "a different model (from gpt-6-luna, gpt-5.6-terra) where you can",
            "do each helper's part yourself, one pass each",
            "platoon holds the command back the first time",
            "another AI that can only read",
            "branch `task/5`",
            TESTS_RULE,
        ] {
            assert!(codex.contains(want), "{want}\n{codex}");
        }
        for not in ["AskUserQuestion", "ExitPlanMode", "Agent tool", "plan mode", "another Claude", "Commit what you change"] {
            assert!(!codex.contains(not), "{not}\n{codex}");
        }
        let no_plan = rules(&Rules { agent: Agent::Codex, plan_first: false, ..base });
        assert!(!no_plan.contains("Plan first"), "{no_plan}");
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A Codex task started in a worktree of the repository `<d>/shop` (its own git identity), its session known.
    fn codex_git_world(name: &str) -> (PathBuf, String, u32, PathBuf, PathBuf) {
        let d = dir(name);
        let p = crate::projects::Projects::open(&d).create("Shop").unwrap();
        let shop = d.join("shop");
        fs::create_dir_all(&shop).unwrap();
        git(&shop, &["init", "-q"]);
        for (k, v) in [("user.name", "Shop Owner"), ("user.email", "owner@example.com"), ("commit.gpgsign", "false")] {
            git(&shop, &["config", k, v]);
        }
        fs::write(shop.join("README"), "hi\n").unwrap();
        git(&shop, &["add", "."]);
        git(&shop, &["commit", "-q", "-m", "first"]);
        let mut store = Tickets::open(&d, &p);
        let n = store
            .add(NewTicket { title: "Login".into(), folder: Some(shop.to_string_lossy().into_owned()), ..Default::default() })
            .unwrap();
        let w = crate::worktrees::prepare("git", &shop.to_string_lossy(), n, None).unwrap().unwrap();
        store.start(n, "feature", CODEX_ID, Some(w.clone()), &[]).unwrap();
        (d, p, n, shop, PathBuf::from(w.path))
    }

    #[test]
    fn at_each_stop_platoon_commits_what_a_codex_task_changed_and_the_check_sees_it() {
        let (d, p, n, shop, wt) = codex_git_world("codex-commit");
        fs::write(wt.join("login.rs"), "fn login() {}\n").unwrap();
        assert_eq!(chook("task-stop", &cinput(CODEX_ID, "Stop", json!({"last_assistant_message": "Added login."})), &d, &p, n), None);
        assert_eq!(git(&shop, &["log", "-1", "--format=%s|%an", "task/1"]), "#1 Login|Shop Owner", "one commit, the repo's own identity");
        assert_eq!(git(&shop, &["rev-list", "--count", "main..task/1"]), "1");
        assert_eq!(git(&wt, &["status", "--porcelain"]), "");
        let asked = fs::read_to_string(d.join("check-stdin")).unwrap();
        assert!(asked.contains("+fn login() {}"), "the check saw the commit in the branch's diff: {asked}");
        assert_eq!((state(&d, &p, n), checks_made(&d)), (State::Done, 1));
        // Typed into again, a stop with nothing changed makes no commit; the check runs all the same.
        chook("task-prompt", &cinput(CODEX_ID, "UserPromptSubmit", json!({"prompt": "anything else?"})), &d, &p, n);
        chook("task-stop", &cinput(CODEX_ID, "Stop", json!({"last_assistant_message": "Nothing more."})), &d, &p, n);
        assert_eq!(git(&shop, &["rev-list", "--count", "main..task/1"]), "1", "no empty commit");
        assert_eq!(checks_made(&d), 2);
    }

    #[test]
    fn a_commit_that_fails_needs_you_with_why_keeps_the_work_and_isnt_checked() {
        let (d, p, n, shop, wt) = codex_git_world("codex-commit-fails");
        let hook = shop.join(".git/hooks/pre-commit");
        fs::write(&hook, "#!/bin/sh\necho 'lint: login.rs has no tests' >&2\nexit 1\n").unwrap();
        fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        fs::write(wt.join("login.rs"), "fn login() {}\n").unwrap();
        assert_eq!(chook("task-stop", &cinput(CODEX_ID, "Stop", json!({"last_assistant_message": "Added login."})), &d, &p, n), None);
        let State::NeedsYou(Need::CommitFailed(why)) = state(&d, &p, n) else { panic!("{:?}", state(&d, &p, n)) };
        assert!(why.contains("git commit failed") && why.contains("lint: login.rs has no tests"), "{why}");
        assert_eq!(Need::CommitFailed(why).words(), "its work couldn't be committed");
        assert_eq!(fs::read_to_string(wt.join("login.rs")).unwrap(), "fn login() {}\n", "the work is kept");
        assert_eq!(git(&shop, &["rev-list", "--count", "main..task/1"]), "0");
        assert_eq!(checks_made(&d), 0, "nothing checked on work that isn't on the branch");
        // Fixed (the hook lets it through), the user types, the next stop commits and is checked.
        fs::remove_file(&hook).unwrap();
        chook("task-prompt", &cinput(CODEX_ID, "UserPromptSubmit", json!({"prompt": "try again"})), &d, &p, n);
        chook("task-stop", &cinput(CODEX_ID, "Stop", json!({"last_assistant_message": "Done."})), &d, &p, n);
        assert_eq!((git(&shop, &["rev-list", "--count", "main..task/1"]).as_str(), state(&d, &p, n)), ("1", State::Done));
    }

    #[test]
    fn a_plan_stop_commits_nothing_when_nothing_changed() {
        let (d, p, n, shop, _) = codex_git_world("codex-commit-plan");
        let task_dir = tickets::task_dir(&d, &p, n);
        let all = crate::squads::all(&d);
        write_task_files(&task_dir, &all[0], &all, &Rules { agent: Agent::Codex, plan_first: true, ..Default::default() }).unwrap();
        chook("task-stop", &cinput(CODEX_ID, "Stop", json!({"last_assistant_message": "1. Add the form."})), &d, &p, n);
        assert_eq!(state(&d, &p, n), State::NeedsYou(Need::Plan));
        assert_eq!(git(&shop, &["rev-list", "--count", "main..task/1"]), "0");
    }

    // ---- the squad's steps ----

    /// Task `n` of `p` has these steps (as it would from its squad when it started).
    fn with_steps(d: &Path, p: &str, n: u32, titles: &[&str]) {
        let titles: Vec<String> = titles.iter().map(|t| t.to_string()).collect();
        Tickets::open(d, p)
            .update(n, |t| {
                t.steps = crate::steps::Steps::new(&titles);
                true
            })
            .unwrap();
    }

    fn mark(d: &Path, p: &str, n: u32, step: usize, report: crate::steps::Report) {
        Tickets::open(d, p).mark_step(n, step, &report).unwrap();
    }

    #[test]
    fn a_stop_with_steps_not_marked_sends_the_session_back_once_before_any_check_and_typing_starts_that_again() {
        use crate::steps::Report;
        let (d, p, n) = world("unmarked");
        with_steps(&d, &p, n, &["Look", "Build", "Check"]);
        mark(&d, &p, n, 1, Report::Done("read it".into()));
        let said = hook("task-stop", &input("Stop", json!({"last_assistant_message": "Built it."})), &d).expect("sent back");
        let v: Value = serde_json::from_str(&said).unwrap();
        assert_eq!(v["decision"], "block");
        let reason = v["reason"].as_str().unwrap();
        assert!(reason.starts_with("Before you finish: Steps 2 (Build), 3 (Check) aren't marked done or skipped."), "{reason}");
        assert!(reason.contains("`/opt/platoon/bin/platoon task step <n> done \"<one line: what you did>\"`"), "{reason}");
        assert_eq!(checks_made(&d), 0, "no tokens spent on the check yet");
        let t = task(&d, &p, n);
        assert!(t.steps.sent_back && t.state == State::Working && t.checking.is_none());
        // Still not marked at the next stop: it goes through to the check as ever.
        assert_eq!(hook("task-stop", &input("Stop", json!({})), &d), None);
        assert_eq!((state(&d, &p, n), checks_made(&d)), (State::Done, 1));
        // The user types: a new round of work, so the steps are asked for once more.
        hook("task-prompt", &input("UserPromptSubmit", json!({"prompt": "one more thing"})), &d);
        assert!(!task(&d, &p, n).steps.sent_back);
        assert!(hook("task-stop", &input("Stop", json!({})), &d).is_some(), "sent back again");
        assert_eq!(checks_made(&d), 1);
        // Every step marked: straight to the check.
        hook("task-prompt", &input("UserPromptSubmit", json!({"prompt": "and now?"})), &d);
        mark(&d, &p, n, 2, Report::Done(String::new()));
        mark(&d, &p, n, 3, Report::Skipped("nothing to check".into()));
        assert_eq!(hook("task-stop", &input("Stop", json!({})), &d), None);
        assert_eq!((state(&d, &p, n), checks_made(&d)), (State::Done, 2));
    }

    #[test]
    fn a_stop_while_subagents_work_or_planning_asks_for_no_steps() {
        let (d, p, n) = world("unmarked-busy");
        with_steps(&d, &p, n, &["Look", "Build"]);
        assert_eq!(hook("task-stop", &input("Stop", json!({"background_tasks": [{"id": "a1"}]})), &d), None);
        assert_eq!(hook("task-stop", &input("Stop", json!({"permission_mode": "plan", "last_assistant_message": "Red?"})), &d), None);
        assert!(!task(&d, &p, n).steps.sent_back);
    }

    #[test]
    fn a_codex_tasks_step_command_is_recorded_by_its_hook_and_refused_so_it_never_runs() {
        use crate::steps::Mark;
        let (d, p, n) = codex_world("codex-steps");
        chook("task-start", &cinput(CODEX_ID, "SessionStart", json!({"source": "startup"})), &d, &p, n);
        with_steps(&d, &p, n, &["Look", "Build", "Check"]);
        let run = |cmd: &str, session: &str| denied(&chook("task-before", &cinput(session, "PreToolUse", shell(cmd)), &d, &p, n));
        let exe = "/opt/platoon/bin/platoon";
        assert_eq!(run(&format!("{exe} task step 1 done 'read the code'"), CODEX_ID).as_deref(), Some("Recorded: step 1 done."));
        let t = task(&d, &p, n);
        assert!(matches!(&t.steps.list[0].mark, Mark::Done { note, .. } if note == "read the code"), "{:?}", t.steps);
        // A helper's, inside a shell; on a line with something else (which didn't run either, and it says so).
        assert_eq!(run(&format!("bash -lc '{exe} task step 2 started'"), "a-helper").as_deref(), Some("Recorded: step 2 started."));
        let both = run(&format!("cd /w && {exe} task step 2 done"), CODEX_ID).unwrap();
        assert_eq!(both, "Recorded: step 2 done. Nothing else on that command line ran: run the rest as a command of its own.");
        // Wrong: said, nothing recorded.
        let wrong = run(&format!("{exe} task step 9 done"), CODEX_ID).unwrap();
        assert!(wrong.starts_with("Not recorded: there is no step 9: the steps are 1 to 3."), "{wrong}");
        let why = run(&format!("{exe} task step 3 skipped"), CODEX_ID).unwrap();
        assert!(why.starts_with("Not recorded: say why step 3 was skipped"), "{why}");
        assert!(matches!(task(&d, &p, n).steps.list[2].mark, Mark::Open));
        // By name too: the session may leave the path out, or another platoon may have started it.
        assert_eq!(run("platoon task step 3 done", CODEX_ID).as_deref(), Some("Recorded: step 3 done."));
        mark(&d, &p, n, 3, crate::steps::Report::Started);
        // platoon's other commands, and a mention: not taken.
        for cmd in [format!("{exe} task list"), format!("echo '{exe} task step 3 done'")] {
            assert_eq!(chook("task-before", &cinput(CODEX_ID, "PreToolUse", shell(&cmd)), &d, &p, n), None, "{cmd}");
        }
        // The end of the work with step 3 unmarked: sent back once in Codex's words, no check.
        let back = chook("task-stop", &cinput(CODEX_ID, "Stop", json!({"last_assistant_message": "Done."})), &d, &p, n).expect("sent back");
        let v: Value = serde_json::from_str(&back).unwrap();
        assert_eq!(v["decision"], "block");
        assert!(v["reason"].as_str().unwrap().contains("Step 3 (Check) isn't marked"), "{back}");
        assert_eq!(checks_made(&d), 0);
        assert_eq!(chook("task-stop", &cinput(CODEX_ID, "Stop", json!({"last_assistant_message": "Done."})), &d, &p, n), None);
        assert_eq!((state(&d, &p, n), checks_made(&d)), (State::Done, 1));
    }

    #[test]
    fn the_rules_have_the_session_mark_each_step_by_platoons_path_and_codex_hears_how_its_hook_takes_it() {
        let base =
            Rules { folder: "/w".into(), squads_dir: "/s".into(), exe: "/opt/my tools/platoon".into(), steps: 9, ..Default::default() };
        let claude = rules(&base);
        for want in [
            "numbered steps below, 1 to 9, counted in order",
            "run `'/opt/my tools/platoon' task step <n> started`",
            "`'/opt/my tools/platoon' task step <n> done \"<one line: what you did>\"`",
            "`'/opt/my tools/platoon' task step <n> skipped \"<one line: why>\"`",
            "Mark every step done or skipped before you finish.",
            "the steps of another squad a step tells you to follow are part of that step",
        ] {
            assert!(claude.contains(want), "{want}\n{claude}");
        }
        assert!(!claude.contains("Recorded"), "{claude}");
        let codex = rules(&Rules { agent: Agent::Codex, ..base.clone() });
        assert!(codex.contains("answers \"Recorded: ...\": that answer means it worked, so don't run it again"), "{codex}");
        assert!(!rules(&Rules { steps: 0, ..base }).contains("task step"), "a squad without steps has nothing to mark");
    }

    #[test]
    fn a_task_sessions_status_line_names_its_task_step_model_helpers_and_check_and_another_sessions_has_none() {
        use crate::steps::Report;
        let (d, p, n) = world("status-line");
        let task_dir = tickets::task_dir(&d, &p, n);
        let all = crate::squads::all(&d);
        write_task_files(&task_dir, &all[0], &all, &Rules { exe: "/p".into(), steps: 9, ..Default::default() }).unwrap();
        let titles = crate::steps::parse(&all[0].body);
        Tickets::open(&d, &p)
            .update(n, |t| {
                t.steps = crate::steps::Steps::new(&titles);
                true
            })
            .unwrap();
        mark(&d, &p, n, 1, Report::Done("read it".into()));
        mark(&d, &p, n, 2, Report::Started);
        // Two reviewers, as Claude Code 2.1.310 writes them: one finished, one at work.
        let transcript = d.join("claude/projects/-w/s-1.jsonl");
        let subagents = d.join("claude/projects/-w/s-1/subagents");
        fs::create_dir_all(&subagents).unwrap();
        fs::write(&transcript, "").unwrap();
        let meta = |id: &str, description: &str, model: &str, finished: bool| {
            let v = json!({"agentType": "general-purpose", "description": description, "toolUseId": "toolu_1", "spawnDepth": 1,
                "requestShape": "background", "requestNonInteractive": true, "model": model, "finished": finished});
            fs::write(subagents.join(format!("agent-{id}.meta.json")), v.to_string()).unwrap();
        };
        meta("b2", "Reviewer B on the diff", "fable", false);
        meta("a1", "Reviewer A on the diff", "opus", true);
        let payload = |session: &str| {
            json!({"session_id": session, "transcript_path": transcript, "model": {"id": "claude-opus-5-5", "display_name": "Opus"},
                "rate_limits": {"five_hour": {"used_percentage": 12}}})
            .to_string()
        };
        let now = crate::usage::now();
        assert_eq!(
            status_line(&payload("s-1"), &d, "5h 12%", 200, now).as_deref(),
            Some("#1 Feature · step 2/9 Name the data shape first · Opus · Reviewers: Opus ✓ · Fable … · 5h 12%")
        );
        // The check running: who checks, and for how long.
        Tickets::open(&d, &p)
            .update(n, |t| {
                t.checking = Some(tickets::Checking { since: now - 23, model: "fable".into() });
                true
            })
            .unwrap();
        let line = status_line(&payload("s-1"), &d, "", 200, now).unwrap();
        assert!(line.ends_with(" · Fable is checking the work… 0:23"), "{line}");
        assert!(status_line(&payload("s-1"), &d, "5h 12%", 60, now).unwrap().chars().count() <= 60);
        // Not a task's session, or a payload that isn't one: none (the usage figures are shown as ever).
        assert_eq!(status_line(&payload("someone-else"), &d, "5h 12%", 200, now), None);
        assert_eq!(status_line("not json", &d, "", 200, now), None);
    }
}
