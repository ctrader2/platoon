//! The AI coding agents platoon runs, behind one seam. Each agent has a module of its own here (`claude.rs`: Claude
//! Code; `codex.rs`: OpenAI's Codex CLI) that is the only place that knows its files, flags, hooks and models; the rest
//! of platoon goes through this one: the `Agent` it is dealing with, and the agent-neutral shapes below, which an
//! agent's module produces.

use crate::ai::{Outcome, Request};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub mod claude;
pub mod codex;
pub mod codex_server;

/// An AI coding agent platoon runs. Closed on purpose: a new agent makes the compiler name every place that must
/// answer for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Agent {
    Claude,
    Codex,
}

impl Agent {
    /// Every agent platoon knows.
    pub const ALL: [Agent; 2] = [Agent::Claude, Agent::Codex];

    /// The word for it in platoon's own files and on its command line (`--agent codex`).
    pub fn id(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }

    /// The agent `id` names; None for a word platoon doesn't know.
    pub fn from_id(id: &str) -> Option<Agent> {
        Agent::ALL.into_iter().find(|a| a.id() == id)
    }

    /// Its name, as the user knows it.
    pub fn name(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
        }
    }

    /// A model id as a row shows it (`claude-sonnet-5` -> `Sonnet 5`, `gpt-6-luna` -> `GPT-6-Luna`).
    pub fn model_name(self, id: &str) -> String {
        match self {
            Agent::Claude => claude::pretty_model(id),
            Agent::Codex => codex::pretty_model(id),
        }
    }

    /// What a row's MODEL cell says before the conversation's first reply names a model.
    pub fn no_model_yet(self) -> &'static str {
        match self {
            Agent::Claude => "-",
            Agent::Codex => "Codex",
        }
    }

    /// The agent's own folder on this computer (Claude Code's `~/.claude`, Codex's `~/.codex`, or what the environment
    /// names). The user's real one: never in a unit test.
    pub fn home(self) -> PathBuf {
        match self {
            Agent::Claude => claude::claude_root(),
            Agent::Codex => codex::codex_root(),
        }
    }

    /// Does the agent say the session that process `pid` runs (conversation `session`, when known) is idle: not
    /// working, not waiting on anything? `root` is the agent's folder. Anything unclear is not idle.
    pub fn known_idle(self, root: &Path, pid: u32, session: Option<&str>) -> bool {
        match self {
            Agent::Claude => claude::registry_status(root, pid).as_deref() == Some("idle"),
            Agent::Codex => session.and_then(|s| codex::find_rollout(root, s)).and_then(|p| codex::busy(&p)) == Some(false),
        }
    }

    /// The program that is this agent here.
    pub fn program(self) -> String {
        match self {
            Agent::Claude => claude::program(),
            Agent::Codex => codex::program(),
        }
    }

    /// The arguments the agent's program is started with for `start`.
    pub fn args(self, start: Start) -> Vec<String> {
        match self {
            Agent::Claude => claude::start_args(start),
            Agent::Codex => codex::start_args(start),
        }
    }

    /// Do a host's arguments show a background session (`Start::Attach`)?
    pub fn attaches(self, args: &[String]) -> bool {
        match self {
            Agent::Claude => claude::attaches(args),
            Agent::Codex => false,
        }
    }

    /// The session a host's arguments name, when they do (a task's session, a resumed one).
    pub fn session_of_args(self, args: &[String]) -> Option<String> {
        match self {
            Agent::Claude => claude::session_of_args(args),
            Agent::Codex => codex::session_of_args(args),
        }
    }

    /// The arguments a task's session is started (or continued) with.
    pub fn task_args(self, l: &TaskLaunch) -> Vec<String> {
        match self {
            Agent::Claude => claude::task_args(l),
            Agent::Codex => codex::task_args(l),
        }
    }

    /// Read one call of a hook platoon installed for `kind`: None when the input isn't one (not JSON, no session id),
    /// or is an event of that kind platoon has nothing to do with.
    pub fn hook_input(self, kind: HookKind, input: &str) -> Option<HookInput> {
        match self {
            Agent::Claude => claude::hook_input(kind, input),
            Agent::Codex => codex::hook_input(kind, input),
        }
    }

    /// What a hook's process says back for `reply`.
    pub fn hook_answer(self, reply: &HookReply) -> HookAnswer {
        match self {
            Agent::Claude => claude::hook_answer(reply),
            Agent::Codex => codex::hook_answer(reply),
        }
    }

    /// Does a one-shot call's JSON Schema go in a file (Codex's `--output-schema`) rather than on the command line?
    pub fn schema_in_file(self) -> bool {
        match self {
            Agent::Claude => false,
            Agent::Codex => true,
        }
    }

    /// Make `cmd` (the agent's program) a one-shot call of `model` for `req`, run in `cwd`: no session kept, the
    /// prompt on stdin, the answer as JSON. `schema_file`: where the request's JSON Schema was written, for an agent
    /// that reads it from a file (`schema_in_file`).
    pub fn one_shot(self, cmd: &mut Command, model: &str, req: &Request, cwd: &Path, schema_file: Option<&Path>) {
        match self {
            Agent::Claude => claude::one_shot(cmd, model, req),
            Agent::Codex => codex::one_shot(cmd, model, req, cwd, schema_file),
        }
    }

    /// What a one-shot call's output says: its answer, or why there is none, and what it used.
    pub fn one_shot_outcome(self, req: &Request, stdout: &[u8], stderr: &[u8], exited_ok: bool) -> Outcome {
        match self {
            Agent::Claude => claude::interpret(stdout, stderr, exited_ok),
            Agent::Codex => codex::interpret(stdout, stderr, exited_ok, req.json_schema.is_some()),
        }
    }

    /// What the agent is called in a sentence about its plan or its answers: "Claude isn't signed in", "your Codex
    /// plan".
    pub fn short(self) -> &'static str {
        match self {
            Agent::Claude => "Claude",
            Agent::Codex => "Codex",
        }
    }

    /// How many tokens of context the conversation's model has room for.
    pub fn context_window(self, t: &Transcript) -> u64 {
        match self {
            Agent::Claude => claude::window_size(t.context_tokens, &mut t.large_window.clone()),
            Agent::Codex => codex::window_size(t),
        }
    }

    /// The conversation a saved transcript of this agent's holds, from its path; None when the path isn't one.
    pub fn session_id_of_path(self, path: &Path) -> Option<String> {
        match self {
            Agent::Claude => Some(path.file_stem()?.to_str()?.to_string()),
            Agent::Codex => codex::id_of_path(path),
        }
    }

    /// Every saved transcript of this agent's under its folder `root`.
    pub fn transcript_paths(self, root: &Path) -> Vec<PathBuf> {
        match self {
            Agent::Claude => claude::transcript_paths(root),
            Agent::Codex => codex::rollout_paths(root),
        }
    }

    /// The saved transcript of conversation `id`, under the agent's folder `root`.
    pub fn find_transcript(self, root: &Path, id: &str) -> Option<PathBuf> {
        match self {
            Agent::Claude => claude::find_transcript(root, id),
            Agent::Codex => codex::find_rollout(root, id),
        }
    }

    /// Read a transcript cheaply, whatever its size (see `claude::read_transcript`). `prev`: the earlier reading of the
    /// same file, whose facts are kept when they aren't found again.
    pub fn read_transcript(self, path: &Path, prev: Option<&Transcript>) -> Option<Transcript> {
        match self {
            Agent::Claude => claude::read_transcript(path, prev),
            Agent::Codex => codex::read_rollout(path, prev),
        }
    }

    /// The folder a conversation was run in, from its transcript.
    pub fn transcript_cwd(self, path: &Path) -> Option<String> {
        match self {
            Agent::Claude => claude::transcript_cwd(path),
            Agent::Codex => codex::rollout_cwd(path),
        }
    }

    /// The user's real prompt in one line of a transcript, if it holds one, cut to `cap` characters. Cheap on the lines
    /// that can't be one (most of a transcript): turned down without being parsed.
    pub fn prompt_in_line(self, line: &str, cap: usize) -> Option<String> {
        match self {
            Agent::Claude => claude::prompt_in_line(line, cap),
            Agent::Codex => codex::prompt_in_line(line, cap),
        }
    }

    /// Could this line of a transcript be something the agent said in words? A substring test only.
    pub fn may_be_reply(self, line: &str) -> bool {
        match self {
            Agent::Claude => claude::may_be_reply(line),
            Agent::Codex => codex::may_be_reply(line),
        }
    }

    /// The words the agent said in this line, as one line cut to `cap` characters.
    pub fn reply_in_line(self, line: &str, cap: usize) -> Option<String> {
        match self {
            Agent::Claude => claude::reply_in_line(line, cap),
            Agent::Codex => codex::reply_in_line(line, cap),
        }
    }

    /// The words the agent said in this line as it wrote them, lines kept, cleaned and cut (`tidy_reply`).
    pub fn reply_text_in_line(self, line: &str, cap: usize) -> Option<String> {
        match self {
            Agent::Claude => claude::reply_text_in_line(line, cap),
            Agent::Codex => codex::reply_text_in_line(line, cap),
        }
    }

    /// Do platoon's own AI features (smart names and summaries, "Describe it", the hand-off, tasks drafted from a
    /// conversation) read this agent's conversations? Not Codex's yet: those features are built on Claude Code's.
    pub fn smart_reads(self) -> bool {
        match self {
            Agent::Claude => true,
            Agent::Codex => false,
        }
    }

    /// Does the agent itself keep one window per conversation? Codex does (its writer lock): a copy platoon didn't
    /// start is never stopped, taken over or opened a second time; the user closes it where it is.
    pub fn keeps_one_writer(self) -> bool {
        match self {
            Agent::Claude => false,
            Agent::Codex => true,
        }
    }
}

/// The agent a hook's command line names after its kind (`platoon hook stop --agent codex`): none, or one platoon
/// doesn't know, is Claude Code, whose hooks name none.
pub fn agent_arg(args: &[String]) -> Agent {
    args.windows(2).find(|w| w[0] == "--agent").and_then(|w| Agent::from_id(&w[1])).unwrap_or(Agent::Claude)
}

/// The variables in this process's own environment that describe an agent's session it was started from (platoon run
/// from inside Claude Code, say): a session platoon starts, or a one-shot call, must not get them, or it takes itself
/// for a part of that session.
pub fn inherited_session_markers() -> Vec<String> {
    Agent::ALL
        .iter()
        .flat_map(|a| match a {
            Agent::Claude => claude::inherited_session_markers(),
            // None known: nothing Codex puts in its shells' environment is known to make a session take itself for
            // a part of another.
            Agent::Codex => Vec::new(),
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

// ---- a one-shot call (platoon's own: names, notes, the check of a task's work) ---------------------------------------

/// The tools a one-shot call may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tools {
    None,
    /// Reading files and searching them, nothing that changes anything.
    ReadOnly,
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

// ---- models ---------------------------------------------------------------------------------------------------------

/// A model platoon can run, and the agent that runs it: the model decides the agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    /// As a squad's file, the settings and the agent's command line name it.
    pub id: String,
    pub agent: Agent,
    /// Its name where the user sees it: `Sonnet`, `GPT-6-Luna`.
    pub name: String,
    /// The tokens of context it has room for, when its agent says.
    pub window: Option<u64>,
}

/// The AI programs on this computer that platoon can use. What models there are follows from it (`Catalog`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Providers {
    /// Claude Code's program, when Claude Code is installed.
    pub claude: Option<String>,
    /// Codex's program and folder, when Codex is installed.
    pub codex: Option<CodexAt>,
}

/// Where Codex is: its program, and its folder (`codex::codex_root`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexAt {
    pub program: String,
    pub root: PathBuf,
}

impl Providers {
    /// What this computer has: Claude Code when `PLATOON_CLAUDE_BIN` names it or `claude` is on the PATH, Codex as
    /// `codex::installed` decides. Reads the user's real Codex folder: never in a unit test.
    pub fn here() -> Providers {
        let path = std::env::var("PATH").unwrap_or_default();
        let claude = claude::installed(&path).then(claude::program);
        let (root, program) = (codex::codex_root(), codex::program());
        let codex = codex::installed(&root, &program, &path).then_some(CodexAt { program, root });
        Providers { claude, codex }
    }

    /// The program that is `agent` here, when it is installed.
    pub fn program(&self, agent: Agent) -> Option<&str> {
        match agent {
            Agent::Claude => self.claude.as_deref(),
            Agent::Codex => self.codex.as_ref().map(|c| c.program.as_str()),
        }
    }

    /// Codex's folder, when Codex is installed.
    pub fn codex_root(&self) -> Option<&Path> {
        self.codex.as_ref().map(|c| c.root.as_path())
    }

    /// The models of the agents installed here.
    pub fn catalog(&self) -> Catalog {
        Catalog::of(self)
    }
}

/// Every model platoon can offer on this computer: Claude Code's (its aliases, built in: Claude Code has no command
/// that lists models), then Codex's (the ones its `models_cache.json` lists, in its order), each only while its agent
/// is installed. A model that is in no installed agent's list is not installed: nothing runs it, and it is never taken
/// for another agent's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Catalog {
    models: Vec<Model>,
    claude: bool,
    /// Codex's list as it was read: None while Codex isn't installed.
    codex: Option<CodexList>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CodexList {
    /// Read: the models it keeps out of its own picker are here, so a file naming one still finds it.
    Read { hidden: Vec<String> },
    /// Missing or not understood (`codex::ListProblem`).
    Unread(codex::ListProblem),
}

impl Catalog {
    pub fn of(p: &Providers) -> Catalog {
        let mut models: Vec<Model> = Vec::new();
        if p.claude.is_some() {
            models.extend(claude::MODELS.iter().map(|&id| Model {
                id: id.into(),
                agent: Agent::Claude,
                name: claude::alias_name(id),
                window: None,
            }));
        }
        let codex = p.codex_root().map(|root| match codex::models(root) {
            Ok(list) => {
                let (listed, hidden): (Vec<_>, Vec<_>) = list.into_iter().partition(|m| m.listed);
                models.extend(listed.into_iter().map(|m| Model { id: m.slug, agent: Agent::Codex, name: m.name, window: m.window }));
                CodexList::Read { hidden: hidden.into_iter().map(|m| m.slug).collect() }
            }
            Err(problem) => CodexList::Unread(problem),
        });
        Catalog { models, claude: p.claude.is_some(), codex }
    }

    /// The models to offer, in order.
    pub fn models(&self) -> &[Model] {
        &self.models
    }

    /// The models of one agent, in order.
    pub fn of_agent(&self, agent: Agent) -> impl Iterator<Item = &Model> {
        self.models.iter().filter(move |m| m.agent == agent)
    }

    /// Is `agent` installed here?
    pub fn installed(&self, agent: Agent) -> bool {
        match agent {
            Agent::Claude => self.claude,
            Agent::Codex => self.codex.is_some(),
        }
    }

    pub fn find(&self, id: &str) -> Option<&Model> {
        self.models.iter().find(|m| m.id == id)
    }

    /// The agent that runs `model` here; None when no installed agent does. Besides the models offered: Claude Code
    /// takes a full model name (`claude-opus-4-1`) as well as its aliases, Codex a model it keeps out of its picker,
    /// and, while its list can't be read, any name of its own kind (`needs`), which Codex then judges itself.
    pub fn agent_of(&self, model: &str) -> Option<Agent> {
        if let Some(m) = self.find(model) {
            return Some(m.agent);
        }
        match needs(model)? {
            Agent::Claude if self.claude && model.starts_with("claude-") => Some(Agent::Claude),
            Agent::Codex => match &self.codex {
                Some(CodexList::Read { hidden }) if hidden.iter().any(|h| h == model) => Some(Agent::Codex),
                Some(CodexList::Unread(_)) => Some(Agent::Codex),
                _ => None,
            },
            Agent::Claude => None,
        }
    }

    /// `model`'s name where the user sees it: the catalog's, else as its agent would show it.
    pub fn name(&self, model: &str) -> String {
        match self.find(model) {
            Some(m) => m.name.clone(),
            None => match needs(model) {
                Some(Agent::Claude) => claude::alias_name(model),
                Some(a) => a.model_name(model),
                None => model.to_string(),
            },
        }
    }

    /// `model` as a setting shows it: its name, and, when nothing here runs it, why in a few words
    /// ("GPT-6-Luna (needs Codex, not installed)").
    pub fn label(&self, model: &str) -> String {
        match self.missing_words(model) {
            None => self.name(model),
            Some(why) => format!("{} ({why})", self.name(model)),
        }
    }

    /// Why nothing here runs `model`, in a few words; None when something does.
    fn missing_words(&self, model: &str) -> Option<String> {
        if self.agent_of(model).is_some() {
            return None;
        }
        Some(match needs(model) {
            Some(a) if !self.installed(a) => format!("needs {}, not installed", a.name()),
            Some(a) => format!("{} doesn't offer it", a.name()),
            None => "not a model platoon knows".into(),
        })
    }

    /// Why nothing here runs `model`, as a sentence that says what is missing; None when something does.
    pub fn missing(&self, model: &str) -> Option<String> {
        if self.agent_of(model).is_some() {
            return None;
        }
        let name = self.name(model);
        Some(match needs(model) {
            Some(a) if !self.installed(a) => format!("{name} needs {}, which isn't installed on this computer.", a.name()),
            Some(a) => format!("{} doesn't offer {name} on this computer.", a.name()),
            None if model.is_empty() => "No model is chosen.".to_string(),
            None => format!("\"{name}\" isn't a model of Claude Code's or Codex's."),
        })
    }

    /// The model the smart features use unless the user picked one: Claude Code's smallest (`haiku`) where Claude
    /// Code is installed, else the first model Codex lists. None with neither.
    pub fn default_smart(&self) -> Option<String> {
        self.default_with(claude::HAIKU)
    }

    /// The model project notes are written with unless the user picked one: Sonnet where Claude Code is installed
    /// (decided 2026-10-18), else the first model Codex lists.
    pub fn default_notes(&self) -> Option<String> {
        self.default_with(claude::SONNET)
    }

    fn default_with(&self, claude_model: &str) -> Option<String> {
        if self.claude {
            return Some(claude_model.to_string());
        }
        self.of_agent(Agent::Codex).next().map(|m| m.id.clone())
    }

    /// Why Codex adds no models although it is installed, in plain words; None when it isn't installed or its list
    /// was read.
    pub fn codex_problem(&self) -> Option<String> {
        match &self.codex {
            Some(CodexList::Unread(p)) => Some(p.plain()),
            _ => None,
        }
    }
}

/// The agent whose kind of name `model` is, whether or not it is installed: Claude Code's aliases and `claude-...`
/// names, OpenAI's `gpt-...`, `codex-...` and `o<digit>...`. None for anything else.
pub fn needs(model: &str) -> Option<Agent> {
    if claude::MODELS.contains(&model) || model.starts_with("claude-") {
        Some(Agent::Claude)
    } else if model.starts_with("gpt-")
        || model.starts_with("codex-")
        || model.strip_prefix('o').is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit()))
    {
        Some(Agent::Codex)
    } else {
        None
    }
}

/// Is `program` on `path` (a `PATH` value)? A name with a `/` is looked at where it says.
pub fn on_path(program: &str, path: &str) -> bool {
    if program.contains('/') {
        return Path::new(program).is_file();
    }
    !program.is_empty() && std::env::split_paths(path).any(|d| d.join(program).is_file())
}

// ---- what platoon reads about conversations and the processes running them ----------------------------------------

/// A live agent process running a conversation, as its agent registers it.
#[derive(Debug)]
pub struct Live {
    /// The agent running it.
    pub agent: Agent,
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
    /// The model's context window, when the transcript records it (Codex's do). See `Agent::context_window`.
    pub window: Option<u64>,
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

// ---- reading what agents write ---------------------------------------------------------------------------------------

/// `2026-09-19T03:53:40.091Z` -> ms since the epoch. Only the UTC form both agents write is understood.
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

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_to(s: String, cap: usize) -> String {
    match s.char_indices().nth(cap) {
        Some((at, _)) => s[..at].to_string(),
        None => s,
    }
}

/// A reply as the agent wrote it, for the notes updater: each line cleaned of escape sequences and control
/// characters, runs of blank lines folded into one, and the whole cut to `cap` characters with `…` where it was cut.
/// None when there is no text.
fn tidy_reply(raw: &str, cap: usize) -> Option<String> {
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
pub(crate) mod tests {
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

    /// A computer with Claude Code (a stand-in that is never run) and no Codex.
    pub(crate) fn claude_catalog() -> Catalog {
        Providers { claude: Some("/nonexistent/platoon-unit-test/claude".into()), codex: None }.catalog()
    }

    /// A Codex folder (a temp dir) holding the sample `models_cache.json`, as a real Codex 0.163 wrote it (account
    /// fields left out): GPT-6-Luna, GPT-5.6-Terra and GPT-5.6-Luna listed, two hidden.
    pub(crate) fn codex_with_models(name: &str) -> CodexAt {
        let root = tmp(&format!("codex-models-{name}"));
        fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/src/testdata/codex/models_cache.json"), root.join("models_cache.json")).unwrap();
        CodexAt { program: "/nonexistent/platoon-unit-test/codex".into(), root }
    }

    fn ids(c: &Catalog) -> Vec<&str> {
        c.models().iter().map(|m| m.id.as_str()).collect()
    }

    #[test]
    fn with_only_claude_code_the_catalog_is_its_four_aliases_and_an_openai_model_is_not_installed() {
        let c = claude_catalog();
        assert_eq!(ids(&c), ["opus", "sonnet", "haiku", "fable"]);
        assert_eq!(c.models().iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["Opus", "Sonnet", "Haiku", "Fable"]);
        assert!(c.models().iter().all(|m| m.agent == Agent::Claude));
        // The model decides the agent; a full Claude model name is Claude Code's too.
        for m in ["opus", "fable", "claude-opus-4-1"] {
            assert_eq!(c.agent_of(m), Some(Agent::Claude), "{m}");
        }
        // Anything else is not installed: never taken for Claude Code's.
        for m in ["gpt-6-luna", "", "mystery", "claudette"] {
            assert_eq!(c.agent_of(m), None, "{m}");
        }
        assert_eq!(c.label("gpt-6-luna"), "GPT-6-Luna (needs Codex, not installed)");
        assert_eq!(c.label("sonnet"), "Sonnet");
        assert_eq!(c.label("mystery"), "mystery (not a model platoon knows)");
        assert_eq!(c.missing("gpt-6-luna").as_deref(), Some("GPT-6-Luna needs Codex, which isn't installed on this computer."));
        assert_eq!(c.missing("haiku"), None);
        assert_eq!((c.default_smart().as_deref(), c.default_notes().as_deref()), (Some("haiku"), Some("sonnet")));
        assert_eq!(c.codex_problem(), None, "Codex isn't installed: nothing to say about its list");
    }

    #[test]
    fn with_only_codex_the_catalog_is_its_listed_models_in_its_order_and_a_claude_model_is_not_installed() {
        let codex = codex_with_models("codex-only");
        let c = Providers { claude: None, codex: Some(codex) }.catalog();
        assert_eq!(ids(&c), ["gpt-6-luna", "gpt-5.6-terra", "gpt-5.6-luna"], "by priority; the hidden ones are not offered");
        assert_eq!(c.models().iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["GPT-6-Luna", "GPT-5.6-Terra", "GPT-5.6-Luna"]);
        assert!(c.models().iter().all(|m| m.agent == Agent::Codex && m.window == Some(272_000)));
        assert_eq!(c.agent_of("gpt-5.6-terra"), Some(Agent::Codex));
        assert_eq!(c.agent_of("gpt-reserve"), Some(Agent::Codex), "a hidden model a file names still runs on Codex");
        assert_eq!(c.agent_of("gpt-9"), None, "Codex's list is what it offers");
        assert_eq!(c.label("gpt-9"), "GPT-9 (Codex doesn't offer it)");
        for m in ["haiku", "opus", "claude-opus-4-1"] {
            assert_eq!(c.agent_of(m), None, "{m}");
        }
        assert_eq!(c.label("haiku"), "Haiku (needs Claude Code, not installed)");
        assert_eq!(c.missing("sonnet").as_deref(), Some("Sonnet needs Claude Code, which isn't installed on this computer."));
        // The defaults are Codex's first model, for the smart features and the notes alike.
        assert_eq!((c.default_smart().as_deref(), c.default_notes().as_deref()), (Some("gpt-6-luna"), Some("gpt-6-luna")));
    }

    #[test]
    fn with_both_claudes_models_come_first_and_the_defaults_stay_claudes() {
        let codex = codex_with_models("both");
        let c = Providers { claude: Some("claude".into()), codex: Some(codex) }.catalog();
        assert_eq!(ids(&c), ["opus", "sonnet", "haiku", "fable", "gpt-6-luna", "gpt-5.6-terra", "gpt-5.6-luna"]);
        assert_eq!(c.agent_of("haiku"), Some(Agent::Claude));
        assert_eq!(c.agent_of("gpt-6-luna"), Some(Agent::Codex));
        assert_eq!(c.of_agent(Agent::Claude).count(), 4);
        assert_eq!((c.default_smart().as_deref(), c.default_notes().as_deref()), (Some("haiku"), Some("sonnet")));
    }

    #[test]
    fn with_neither_there_is_no_model_and_no_default() {
        let c = Providers::default().catalog();
        assert!(c.models().is_empty());
        assert_eq!((c.default_smart(), c.default_notes()), (None, None));
        assert_eq!(c.label("haiku"), "Haiku (needs Claude Code, not installed)");
        assert_eq!(c.label("gpt-6-luna"), "GPT-6-Luna (needs Codex, not installed)");
        assert!(!c.installed(Agent::Claude) && !c.installed(Agent::Codex));
    }

    #[test]
    fn a_missing_or_broken_model_list_adds_no_codex_models_says_why_and_leaves_codexs_own_names_to_codex() {
        let root = tmp("codex-no-list");
        let codex = CodexAt { program: "/nonexistent/platoon-unit-test/codex".into(), root: root.clone() };
        let p = Providers { claude: None, codex: Some(codex) };
        let c = p.catalog();
        assert!(c.models().is_empty() && c.installed(Agent::Codex));
        let why = c.codex_problem().unwrap();
        assert!(why.contains("models_cache.json") && why.contains("Start Codex once"), "{why}");
        // Codex is there: a name of its kind is left to it (it says itself if it doesn't know it); Claude's is not installed.
        assert_eq!(c.agent_of("gpt-6-luna"), Some(Agent::Codex));
        assert_eq!(c.agent_of("haiku"), None);
        assert_eq!(c.default_smart(), None, "no listed model to default to");
        for broken in ["not json", "[]", r#"{"models": "nope"}"#, r#"{"other": []}"#] {
            fs::write(root.join("models_cache.json"), broken).unwrap();
            let c = p.catalog();
            assert!(c.models().is_empty(), "{broken}");
            assert!(c.codex_problem().unwrap().contains("couldn't be read"), "{broken}");
        }
        // Entries that make no sense are skipped, the rest kept, in Codex's order; a name without a display name is
        // shown as a row would show it. The file is read again once it changes.
        fs::write(
            root.join("models_cache.json"),
            r#"{"models":[{"slug":"gpt-b","visibility":"list","priority":2},{"display_name":"No slug","visibility":"list"},
               {"slug":"gpt-a","display_name":"GPT-A","visibility":"list","priority":1,"context_window":400000},
               {"slug":"has space","visibility":"list"},{"slug":"gpt-c","visibility":"hide","priority":0}]}"#,
        )
        .unwrap();
        let c = p.catalog();
        assert_eq!(ids(&c), ["gpt-a", "gpt-b"]);
        assert_eq!((c.models()[0].window, c.models()[1].name.as_str()), (Some(400_000), "GPT-B"));
        assert_eq!(c.codex_problem(), None);
    }

    #[test]
    fn the_kind_of_name_says_which_agent_a_model_needs() {
        for (m, a) in [
            ("opus", Some(Agent::Claude)),
            ("claude-sonnet-5", Some(Agent::Claude)),
            ("gpt-6-luna", Some(Agent::Codex)),
            ("codex-auto-review", Some(Agent::Codex)),
            ("o3", Some(Agent::Codex)),
            ("o", None),
            ("ollama", None),
            ("", None),
        ] {
            assert_eq!(needs(m), a, "{m}");
        }
    }
}
