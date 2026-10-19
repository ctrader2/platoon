//! The AI coding agents platoon runs, behind one seam. Each agent has a module of its own here (`claude.rs`: Claude
//! Code) that is the only place that knows its files, flags, hooks and models; the rest of platoon goes through this
//! one: the `Agent` it is dealing with, and the agent-neutral shapes below, which an agent's module produces.

use std::{
    fs,
    path::{Path, PathBuf},
};

pub mod claude;

/// An AI coding agent platoon runs. Closed on purpose: a new agent makes the compiler name every place that must
/// answer for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Agent {
    Claude,
}

impl Agent {
    /// Every agent platoon knows.
    pub const ALL: [Agent; 1] = [Agent::Claude];

    /// A model id as a row shows it (`claude-sonnet-5` -> `Sonnet 5`).
    pub fn model_name(self, id: &str) -> String {
        match self {
            Agent::Claude => claude::pretty_model(id),
        }
    }

    /// The program that is this agent here.
    pub fn program(self) -> String {
        match self {
            Agent::Claude => claude::program(),
        }
    }

    /// The arguments the agent's program is started with for `start`.
    pub fn args(self, start: Start) -> Vec<String> {
        match self {
            Agent::Claude => claude::start_args(start),
        }
    }

    /// Do a host's arguments show a background session (`Start::Attach`)?
    pub fn attaches(self, args: &[String]) -> bool {
        match self {
            Agent::Claude => claude::attaches(args),
        }
    }

    /// The session a host's arguments name, when they do (a task's session, a resumed one).
    pub fn session_of_args(self, args: &[String]) -> Option<String> {
        match self {
            Agent::Claude => claude::session_of_args(args),
        }
    }

    /// The arguments a task's session is started (or continued) with.
    pub fn task_args(self, l: &TaskLaunch) -> Vec<String> {
        match self {
            Agent::Claude => claude::task_args(l),
        }
    }

    /// Read one call of a hook platoon installed for `kind`: None when the input isn't one (not JSON, no session id),
    /// or is an event of that kind platoon has nothing to do with.
    pub fn hook_input(self, kind: HookKind, input: &str) -> Option<HookInput> {
        match self {
            Agent::Claude => claude::hook_input(kind, input),
        }
    }

    /// What a hook's process says back for `reply`.
    pub fn hook_answer(self, reply: &HookReply) -> HookAnswer {
        match self {
            Agent::Claude => claude::hook_answer(reply),
        }
    }

    /// How many tokens of context the conversation's model has room for.
    pub fn context_window(self, t: &Transcript) -> u64 {
        match self {
            Agent::Claude => claude::window_size(t.context_tokens, &mut t.large_window.clone()),
        }
    }
}

/// The variables in this process's own environment that describe an agent's session it was started from (platoon run
/// from inside Claude Code, say): a session platoon starts, or a one-shot call, must not get them, or it takes itself
/// for a part of that session.
pub fn inherited_session_markers() -> Vec<String> {
    Agent::ALL
        .iter()
        .flat_map(|a| match a {
            Agent::Claude => claude::inherited_session_markers(),
        })
        .collect()
}

// ---- starting a session ---------------------------------------------------------------------------------------------

/// How a session is started in a platoon host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start<'a> {
    /// A new conversation, with this first message when there is one (a hand-off).
    New(Option<&'a str>),
    /// The saved conversation with this id, continued.
    Resume(&'a str),
    /// A background session the agent's own service runs, shown by attaching to it (see `Live::background`).
    Attach(&'a str),
}

/// What a task's session is started (or continued) with. Its id is chosen by platoon before it starts, so the task
/// names its session from the first moment.
#[derive(Clone, Debug)]
pub struct TaskLaunch {
    pub session: String,
    /// Continue the session rather than start it.
    pub resume: bool,
    /// Its name: "#N title".
    pub name: String,
    pub model: String,
    /// Start in plan mode.
    pub plan: bool,
    /// The task's own folder, with `squad.md` in it.
    pub task_dir: PathBuf,
    /// The platoon program the session's hooks call.
    pub exe: String,
    /// The first message (a new session only).
    pub first: Option<String>,
}

// ---- hooks -----------------------------------------------------------------------------------------------------------

/// Which of platoon's hooks was called: each `platoon hook ...` command is installed for one event of the agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookKind {
    SessionStart,
    /// The user sent a prompt.
    Prompt,
    /// The agent is about to use this tool.
    BeforeTool(Tool),
    /// The agent has used a tool.
    AfterTool,
    /// The agent says it is waiting, perhaps for the user's permission.
    PermissionWait,
    /// The agent has finished its turn.
    Stop,
}

/// A tool whose use moves a task along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// Presents a plan for the user to approve.
    Plan,
    /// Asks the user a question.
    Question,
    Other,
}

/// One call of a hook, read: the session it is about, the folder it works in ("" when not said), and what happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookInput {
    pub session: String,
    pub cwd: String,
    pub event: HookEvent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookEvent {
    /// A session started: `source` is "startup", "resume", "clear" or "compact".
    SessionStart {
        source: String,
    },
    PromptSubmitted,
    /// The agent is about to use `tool`; `said` is the plan it presents or the question it asks ("" when none).
    BeforeTool {
        tool: Tool,
        said: String,
    },
    AfterTool(Tool),
    /// The agent waits for the user's permission, and says what for when it does.
    PermissionWait(Option<String>),
    Stop(TurnEnd),
}

/// How a turn ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnEnd {
    /// The session is planning (in plan mode).
    pub planning: bool,
    /// The agent's last message, when it says.
    pub last_message: Option<String>,
    /// Subagents are still at work: the turn's end isn't the end of the work.
    pub subagents_working: bool,
}

/// What platoon answers a hook with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookReply {
    /// Context added to the conversation as the session starts.
    StartContext(String),
    /// Context added to the conversation with the prompt.
    PromptContext(String),
    /// The prompt is refused, and the user is told why.
    Refuse(String),
    /// The stop is refused: the agent carries on, with this as its reason.
    SendBack(String),
}

/// What a hook's process says back: its standard output and error ("" for nothing) and its exit code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookAnswer {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

impl HookAnswer {
    /// Say it, and give the exit code.
    pub fn print(&self) -> i32 {
        if !self.stdout.is_empty() {
            println!("{}", self.stdout);
        }
        if !self.stderr.is_empty() {
            eprintln!("{}", self.stderr);
        }
        self.code
    }
}

// ---- what platoon reads about conversations and the processes running them ----------------------------------------

/// A live agent process running a conversation, as its agent registers it.
#[derive(Debug)]
pub struct Live {
    pub pid: u32,
    pub id: String,
    pub cwd: String,
    /// Only a name the user chose; the agent's own placeholder is dropped.
    pub name: Option<String>,
    pub started_at: u64,
    pub busy: bool,
    /// Set for a background session: one run by the agent's own service (Claude Code's `claude --bg`, `claude
    /// agents`) rather than in a terminal. It is opened by attaching to it (`Start::Attach`), never by resuming it.
    /// Holds the id that attaching takes.
    pub background: Option<String>,
    /// Set for an interactive session that handed its conversation to a background session (Claude Code's
    /// `parkedJobId`): holds the id of that background session's job (the id attaching takes). Its process stays
    /// alive but no longer works on the conversation, and its registry status is never set back to idle, so a parked
    /// entry is never `busy` and never counts as a copy that writes to the conversation.
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
    /// The model's large context window is in use (the agent's model id says so, or the conversation has been seen
    /// past the default window). Never flips back. See `Agent::context_window`.
    pub large_window: bool,
    pub ai_title: Option<String>,
    /// First real user prompt. None means nothing but slash-command echoes and injected blocks: junk.
    pub first_prompt: Option<String>,
    /// Timestamp of the latest user or assistant entry, ms since the epoch.
    pub last_active_ms: Option<u64>,
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

/// What a task's session has used, and when it last wrote: what its transcript and its subagents' transcripts spent
/// (`spent`), and the latest time any of those files was written (ms since the epoch).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionLog {
    pub spent: Spent,
    pub last_at: Option<u64>,
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

#[cfg(test)]
mod tests {
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
    fn a_claude_conversation_shows_its_model_by_name_and_its_window_from_what_its_transcript_says() {
        let a = Agent::Claude;
        assert_eq!(a.model_name("claude-opus-5-5"), "Opus 5.5");
        let t = Transcript { context_tokens: 120_000, ..Default::default() };
        assert_eq!(a.context_window(&t), 200_000);
        assert_eq!(a.context_window(&Transcript { large_window: true, ..t.clone() }), 1_000_000);
        assert_eq!(a.context_window(&Transcript { context_tokens: 250_000, ..t }), 1_000_000);
    }
}
