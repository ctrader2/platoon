//! The plumbing that lets platoon spend the user's own tokens (Claude's or Codex's), safely. Each feature that uses it
//! is a row in `FEATURES` (or `SWITCHLESS`, for what the user asks for then and there) plus a caller of `Runner::run`.
//!
//! The rules, all enforced here and not by whoever calls:
//! - A call happens only if the master switch AND the feature's own switch are on (`gate`), today's tokens are
//!   under the daily limit, the plan isn't used above the pause point, and no back-off is running. The gate is
//!   checked immediately before the child process starts.
//! - The model decides the program (`agents::Catalog`): a Claude model's call is `claude -p` with the user's normal
//!   login: no tools, no settings, no saved session, no slash commands, no MCP servers, a SHORT system prompt supplied
//!   by the caller and low effort (Claude's own system prompt alone costs ~6,600 tokens a call). Never `--bare`: it
//!   needs an API key, which subscribers don't have. An OpenAI model's call is `codex exec` (`codex::one_shot`):
//!   ephemeral, without the user's config, in Codex's read-only sandbox (Codex can't switch its tools off), and about
//!   13,000 to 20,000 tokens a call of Codex's own instructions, mostly cached (`settings::CODEX_COST`). A model no installed program runs is
//!   refused before anything starts (`model_of`).
//! - It always runs in one fixed neutral folder (`work_dir`: `<cache dir>/ai-work`) with `PLATOON_AI=1` in its
//!   environment, because Claude makes an empty folder under `~/.claude/projects` for every working directory it is
//!   run in. The child registers itself with Claude like any session, so platoon hides everything whose working
//!   folder is that one (`agents::Hidden`): it is never a row, a count or a candidate.
//! - `MAX_THINKING_TOKENS=0` is set in the child's environment: `--effort low` alone still let the model spend hidden
//!   thinking tokens (a batch of 4 conversations cost 2,137 output tokens where the JSON needs about 250).
//! - The prompt goes in on stdin, so a big batch can't hit the limit on argument length.
//! - The runner only ever kills the process it started (and what that process started, which shares its process
//!   group). Turning a switch off cancels the calls of that feature that are running (`Switchboard`).
//! - Nothing a person typed, and nothing Claude answered, is written to disk here: the ledger holds numbers only.
//!
//! The caller asks Claude for JSON in plain text and can use `extract_json`, which copes with code fences and chatter
//! around the JSON; or it gives a JSON Schema (`Request::json_schema`), and the answer is Claude's `structured_output`.
//!
//! A task's check (`verdict.rs`) is part of the task the user started, not a smart feature: it is outside the gate
//! (`PART_OF_A_TASK`: no switch, no daily limit, no pause, and it neither waits for a back-off nor starts one), runs in
//! the task's folder with read-only tools, and its tokens go in the ledger like every call's.

use crate::agents::{Agent, Catalog, Providers, Tools};
use crate::ledger::{Backoff, FailKind, Ledger, Usage};
use crate::settings::{ModelFor, Settings};
use crate::usage;
use serde_json::Value;
use std::{
    collections::HashMap,
    io::{self, Read, Write},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

/// The label of the master switch, as the user sees it in settings and in setup.
pub const MASTER_LABEL: &str = "Enable features which require tokens";

/// One thing that spends tokens. The settings screen, the setup question and the gate all read this table, so a
/// new feature is a new row here.
pub struct Feature {
    pub id: &'static str,
    pub label: &'static str,
    /// One plain sentence: what it does for the person.
    pub description: &'static str,
    /// On as soon as the master switch is on, without being listed in the settings file. A feature added after
    /// someone has already answered the setup question would otherwise be off for them for ever.
    pub on_unless_switched_off: bool,
}

pub const FEATURES: &[Feature] = &[
    Feature {
        id: "names",
        label: "Smart names and summaries",
        description: "Gives each session a short name and a one-line summary, and keeps them up to date.",
        on_unless_switched_off: false,
    },
    Feature {
        id: "find",
        label: "Find a session by description",
        description: "Finds the session you mean from a few words about it, when searching for exact words isn't enough.",
        on_unless_switched_off: false,
    },
    Feature {
        id: crate::notesrun::FEATURE,
        label: "Project notes",
        description: "Keeps a project's notes up to date from what its sessions did, a few minutes after they stop. It never changes what you wrote yourself.",
        on_unless_switched_off: true,
    },
    Feature {
        id: crate::describe::FEATURE,
        label: "Project descriptions",
        description: "Gives each project the one line that says what it is, from its sessions' summaries and its notes, at most once an hour.",
        on_unless_switched_off: true,
    },
];

/// Work that has no switch of its own, with what `platoon tokens` calls it. It only ever runs because the person
/// asked for it then and there, so there is nothing to switch off; it obeys every other gate (the master switch,
/// the daily limit, a back-off) and its spend is in the ledger like everything else.
pub const SWITCHLESS: &[(&str, &str)] =
    &[(TEST_FEATURE, "Token test"), (crate::handoff::FEATURE, "Hand-off"), (crate::taskdraft::FEATURE, "Tasks from a conversation")];

/// Calls that are part of a task the user started, with what `platoon tokens` calls them: the task's session spends
/// without asking platoon, and so does its check. Outside the gate entirely; their tokens are still in the ledger.
pub const PART_OF_A_TASK: &[(&str, &str)] = &[(crate::verdict::FEATURE, "Task checks")];

/// Is this a call that is part of a task (`PART_OF_A_TASK`)?
pub fn part_of_a_task(id: &str) -> bool {
    PART_OF_A_TASK.iter().any(|(f, _)| *f == id)
}

/// Is this a feature id with no switch of its own?
pub fn switchless(id: &str) -> bool {
    SWITCHLESS.iter().any(|(f, _)| *f == id)
}

/// What a feature id is called where the user sees it (`platoon tokens`), switch or no switch.
pub fn feature_label(id: &str) -> &str {
    FEATURES
        .iter()
        .find(|f| f.id == id)
        .map(|f| f.label)
        .or_else(|| SWITCHLESS.iter().chain(PART_OF_A_TASK).find(|(f, _)| *f == id).map(|(_, label)| *label))
        .unwrap_or(id)
}

/// Is this feature on when the settings file says nothing about it?
pub fn on_by_default(id: &str) -> bool {
    FEATURES.iter().any(|f| f.id == id && f.on_unless_switched_off)
}

/// The id `platoon tokens test` uses: it obeys every gate except a feature's own switch, because it isn't a feature.
pub const TEST_FEATURE: &str = "test";

/// The longest a call may take before it is given up on and killed.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(90);

/// The most of Claude's output that is kept (more than any sane answer).
const MAX_OUTPUT: usize = 8 * 1024 * 1024;

/// How to turn the features on after saying no (or never being asked): in platoon, or in the settings file.
pub fn enable_hint(settings_path: &Path) -> String {
    format!(
        "To turn them on: press s in platoon and switch on \"{MASTER_LABEL}\" (and the features you want). \
         The same switch is \"tokens_enabled\" in {}.",
        settings_path.display()
    )
}

/// The one line non-interactive setup prints. `agent`: the one the smart features' model runs on here.
pub fn setup_hint(agent: Agent) -> String {
    format!(
        "Smart features that use your {} tokens are off. To turn them on, run `platoon setup` in a terminal, or press s in platoon and switch on \"{MASTER_LABEL}\".",
        agent.short()
    )
}

/// The question `platoon setup` asks (default: no), about the plan of `agent`, the one the smart features' model runs
/// on here (Codex on a computer with only Codex).
pub fn consent_text(agent: Agent) -> String {
    let plan = match agent {
        Agent::Claude => "Claude plan",
        Agent::Codex => "ChatGPT plan, through Codex,",
    };
    let mut t = format!("Smart features (optional)\n\nplatoon can use your own {plan} to add:\n");
    for f in FEATURES {
        t.push_str(&format!("  - {}: {}\n", f.label, f.description));
    }
    t.push_str(&match agent {
        Agent::Claude => "\nIf you say yes, platoon sends Claude the prompts you typed plus one line of Claude's last reply for each session (for \
             a project's notes, more of that reply), in small batches, and it counts against your Claude plan. Nothing is sent if you say no.\n"
            .to_string(),
        Agent::Codex => "\nIf you say yes, platoon sends Codex the prompts you typed plus one line of the last reply for each session (for a \
             project's notes, more of that reply), in small batches, and it counts against your plan's Codex limits; each call also \
             costs about 13,000 to 20,000 tokens of Codex's own instructions. Nothing is sent if you say no.\n"
            .to_string(),
    });
    t.push_str(&format!(
        "You can switch this off at any time: press s in platoon, then turn off \"{MASTER_LABEL}\".\n\nTurn these features on?"
    ));
    t
}

/// Why a call was not even started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The master switch is off.
    MasterOff,
    /// The feature's own switch is off (its label).
    FeatureOff(&'static str),
    /// A feature id that isn't in the table (a bug in the caller).
    UnknownFeature(String),
    /// A recent failure of a call on `agent`: wait until `until` (seconds since the epoch).
    BackedOff { agent: Agent, kind: FailKind, until: u64, now: u64 },
    /// Today's tokens on the agent the call would use have reached the daily limit.
    OverBudget { used: u64, cap: u64 },
    /// The plan of the agent the call would use is used above the pause point, in this window.
    PlanBusy { agent: Agent, pct: u8, limit: u64, window: String },
    /// No installed agent runs the model it would use: what is missing, in a sentence or two, and the program that would
    /// run it when that program is installed but switched off in platoon's settings.
    NoModel(String, Option<Agent>),
}

pub fn wait_text(secs: u64) -> String {
    match secs {
        0..=59 => "less than a minute".into(),
        60..=5399 => {
            let m = secs.div_ceil(60);
            format!("{m} minute{}", if m == 1 { "" } else { "s" })
        }
        _ => {
            let h = secs.div_ceil(3600);
            format!("{h} hours")
        }
    }
}

impl Refusal {
    /// The reason in plain words, as a sentence.
    pub fn plain(&self) -> String {
        use crate::settings::thousands;
        match self {
            Refusal::MasterOff => format!("\"{MASTER_LABEL}\" is switched off."),
            Refusal::FeatureOff(label) => format!("\"{label}\" is switched off."),
            Refusal::UnknownFeature(id) => format!("There is no smart feature called \"{id}\"."),
            Refusal::BackedOff { agent, kind, until, now } => {
                format!(
                    "Smart features are paused after a failure ({}); trying again in {}.",
                    kind.plain(*agent),
                    wait_text(until.saturating_sub(*now))
                )
            }
            Refusal::OverBudget { used, cap } => {
                format!(
                    "Today's token limit is used up ({} of {} tokens). It starts over tomorrow, or you can raise it in settings.",
                    thousands(*used),
                    thousands(*cap)
                )
            }
            Refusal::PlanBusy { agent, pct, limit, window } => {
                format!(
                    "Your {} plan is {pct}% used ({window}), above the pause point of {limit}%. Smart features wait until it drops.",
                    agent.short()
                )
            }
            Refusal::NoModel(why, _) => why.clone(),
        }
    }

    /// True when the refusal is because the user hasn't switched something on.
    pub fn is_switch(&self) -> bool {
        matches!(self, Refusal::MasterOff | Refusal::FeatureOff(_))
    }

    /// Where to put it right, for the reasons `plain()` doesn't already say that itself. Empty when there is
    /// nothing for the user to do (a back-off tries again on its own; the daily limit's own sentence says where
    /// to raise it).
    pub fn where_to_change(&self) -> &'static str {
        match self {
            Refusal::MasterOff | Refusal::FeatureOff(_) => "Press s for the settings to switch it back on.",
            Refusal::PlanBusy { .. } => "Press s for the settings to change the pause point.",
            Refusal::BackedOff { .. } | Refusal::OverBudget { .. } | Refusal::UnknownFeature(_) | Refusal::NoModel(..) => "",
        }
    }

    /// The whole thing a screen says about it: the reason, and where to change it when that helps.
    pub fn on_screen(&self) -> String {
        match self.where_to_change() {
            "" => self.plain(),
            where_to => format!("{} {where_to}", self.plain()),
        }
    }

    /// The few words the header has room for: no full stop, no quotes, and short enough to sit beside the tabs.
    pub fn short(&self) -> String {
        use crate::settings::thousands;
        match self {
            Refusal::MasterOff => "smart features off".into(),
            Refusal::FeatureOff(label) => format!("{} off", label.to_lowercase()),
            Refusal::UnknownFeature(_) => "smart features off".into(),
            Refusal::BackedOff { until, now, .. } => {
                format!("smart features paused after a failure · trying again in {}", wait_text(until.saturating_sub(*now)))
            }
            Refusal::OverBudget { cap, .. } => format!("smart features paused · today's {} limit is used up", thousands(*cap)),
            Refusal::PlanBusy { agent, pct, window, .. } => {
                format!("smart features paused · your {} plan is {pct}% used ({window})", agent.short())
            }
            Refusal::NoModel(_, Some(off)) => format!("smart features stopped · {} is switched off", off.name()),
            Refusal::NoModel(_, None) => "smart features stopped · their model isn't installed".into(),
        }
    }
}

/// The windows of `agent`'s plan that are still running at `now`, as its program last reported them: Claude Code's
/// from its status line (`usage::load`), Codex's from its saved sessions (`codex::rate_limits`). None when no figure
/// has been reported (Claude Code only reports it for Pro and Max, after its first answer; Codex after a turn), or the
/// agent isn't installed.
pub fn plan_limits(agent: Agent, cache: &Path, providers: &Providers, now: u64) -> Option<Vec<usage::Limit>> {
    match agent {
        Agent::Claude => Some(usage::load(cache, now).plan?.limits()),
        Agent::Codex => {
            let running: Vec<usage::Limit> =
                crate::agents::codex::rate_limits(providers.codex_root()?, now)?.into_iter().filter(|l| l.resets_at > now).collect();
            (!running.is_empty()).then_some(running)
        }
    }
}

/// `agent`'s plan's highest used percentage among the windows that are still running, with the window's name. None
/// when no figure has been reported: no figure never pauses anything. A window's figure only goes up until it resets,
/// and a reset window is dropped, so a figure that is still listed is a true lower bound however old the report.
/// Claude Code's 5-hour window that hasn't started is listed as 0% (see `usage::load`): also a lower bound, and never
/// above any pause setting.
pub fn plan_percent(agent: Agent, cache: &Path, providers: &Providers, now: u64) -> Option<(u8, String)> {
    plan_limits(agent, cache, providers, now)?.into_iter().max_by_key(|l| l.pct).map(|l| (l.pct, l.name()))
}

/// The agent the smart features' calls run on: their model's; Claude Code when it can't be told (none installed).
pub fn smart_agent(settings: &Settings, catalog: &Catalog) -> Agent {
    settings.smart_model_in(catalog).and_then(|m| catalog.agent_of(&m)).unwrap_or(Agent::Claude)
}

/// May a background call for `feature` start now? Cheap checks first; reads only small files.
pub fn gate(settings: &Settings, feature: &str, cache: &Path, providers: &Providers, now: u64) -> Result<(), Refusal> {
    gate_for(settings, feature, cache, providers, now, false)
}

/// The gate, saying who started the call. A call the user started by pressing a key (`user_initiated`: finding a
/// conversation by description) ignores the plan-usage pause, which exists so background work doesn't take the plan
/// from the person's own; it still needs the switches and the daily limit and still waits out a back-off. The limits,
/// the back-off and the pause are those of the agent the feature's model runs on.
pub fn gate_for(
    settings: &Settings,
    feature: &str,
    cache: &Path,
    providers: &Providers,
    now: u64,
    user_initiated: bool,
) -> Result<(), Refusal> {
    switches(settings, feature)?;
    let (_, agent) = model_for(settings, feature, None, &providers.catalog())?;
    limits(settings, agent, cache, providers, now, user_initiated)
}

/// The master switch and the feature's own.
fn switches(settings: &Settings, feature: &str) -> Result<(), Refusal> {
    if !settings.tokens_enabled {
        return Err(Refusal::MasterOff);
    }
    if !switchless(feature) {
        match FEATURES.iter().find(|f| f.id == feature) {
            None => return Err(Refusal::UnknownFeature(feature.into())),
            Some(f) if !settings.feature_toggle(f.id) => return Err(Refusal::FeatureOff(f.label)),
            Some(_) => {}
        }
    }
    Ok(())
}

/// What may stop a call on `agent` that the switches allow: its back-off, its share of today's limit, its plan's pause.
fn limits(settings: &Settings, agent: Agent, cache: &Path, providers: &Providers, now: u64, user_initiated: bool) -> Result<(), Refusal> {
    let data = Ledger::open(cache).load();
    let book = data.of(agent);
    if let Some(b) = book.backoff_at(now) {
        return Err(Refusal::BackedOff { agent, kind: b.kind, until: b.until, now });
    }
    let used = book.day(now).tokens();
    if used >= settings.ai_daily_tokens {
        return Err(Refusal::OverBudget { used, cap: settings.ai_daily_tokens });
    }
    if settings.ai_pause_percent > 0
        && !user_initiated
        && let Some((pct, window)) = plan_percent(agent, cache, providers, now).filter(|(p, _)| u64::from(*p) > settings.ai_pause_percent)
    {
        return Err(Refusal::PlanBusy { agent, pct, limit: settings.ai_pause_percent, window });
    }
    Ok(())
}

/// Everything a screen might want to say about the token features right now.
pub struct Status {
    pub master: bool,
    /// (feature, its own switch)
    pub features: Vec<(&'static Feature, bool)>,
    /// Why a feature that is switched on would be refused right now (budget, back-off, plan, a model not installed),
    /// if it would: for the agent the smart features' model runs on.
    pub paused: Option<Refusal>,
}

pub fn status(settings: &Settings, cache: &Path, providers: &Providers, now: u64) -> Status {
    let mut all_on = settings.clone();
    all_on.tokens_enabled = true;
    Status {
        master: settings.tokens_enabled,
        features: FEATURES.iter().map(|f| (f, settings.feature_toggle(f.id))).collect(),
        paused: gate(&all_on, TEST_FEATURE, cache, providers, now).err(),
    }
}

/// What calls on one agent have spent today, against what may stop them.
pub struct Spend {
    pub used_today: u64,
    pub cap: u64,
    /// Set while calls on it are being refused after a failure.
    pub backoff: Option<Backoff>,
    /// Its plan's highest figure, if its program has reported one.
    pub plan: Option<(u8, String)>,
}

pub fn spend(settings: &Settings, cache: &Path, providers: &Providers, agent: Agent, now: u64) -> Spend {
    let book = Ledger::open(cache).load().of(agent).clone();
    Spend {
        used_today: book.day(now).tokens(),
        cap: settings.ai_daily_tokens,
        backoff: book.backoff_at(now),
        plan: plan_percent(agent, cache, providers, now),
    }
}

/// Why `feature` cannot run right now, in the words a screen shows: the gate's own reason plus where to change it.
/// None while it can run. Nothing here is a new wording: `Refusal` says it (see `on_screen`).
pub fn stopped_reason(settings: &Settings, feature: &str, cache: &Path, providers: &Providers, now: u64) -> Option<String> {
    Some(gate(settings, feature, cache, providers, now).err()?.on_screen())
}

/// What the header says while the features are on but something has stopped them: the short form, and the same
/// with where to change it for when there is room. None while they are running as they should.
///
/// Only the reasons that come out of nowhere and stop every feature at once are here - the daily limit, the plan
/// pause, a back-off after failures, a model that is no longer installed - because those are the ones that go
/// unnoticed. A switch the user turned off themselves is said where that feature lives (the notes pane says it for
/// the notes) rather than following them around every tab for as long as they leave it off.
pub fn header_note(settings: &Settings, cache: &Path, providers: &Providers, now: u64) -> Option<(String, String)> {
    if !settings.tokens_enabled {
        return None;
    }
    let r = status(settings, cache, providers, now).paused?;
    let short = r.short();
    // A back-off starts again by itself, so there is nothing to press; the others are the user's to change.
    let full = if matches!(r, Refusal::BackedOff { .. }) { short.clone() } else { format!("{short} · press s for the settings") };
    Some((short, full))
}

/// The one line for the footer when calls are being refused after a failure: `("<run id>", text)`. The id is the
/// same for the whole run of failures, so a caller that remembers it shows the line once, not once per failure.
/// None when the features are off (nobody is waiting for them) or no back-off is running, on any agent.
pub fn backoff_notice(settings: &Settings, cache: &Path, now: u64) -> Option<(String, String)> {
    if !settings.tokens_enabled {
        return None;
    }
    let data = Ledger::open(cache).load();
    let (agent, b) = Agent::ALL.into_iter().find_map(|a| Some((a, data.of(a).backoff_at(now)?)))?;
    Some((format!("{}:{}:{}", agent.id(), b.since, b.kind.name()), format!("Smart features paused: {}", b.kind.plain(agent))))
}

/// What one call needs. The caller writes the system prompt (short: Claude's own costs thousands of tokens) and the
/// prompt, and says which feature it is for.
pub struct Request {
    pub feature: &'static str,
    pub system: String,
    pub prompt: String,
    pub timeout: Duration,
    pub cancel: Cancel,
    /// The person asked for this by pressing a key (see `gate_for`): the plan-usage pause doesn't apply.
    pub user_initiated: bool,
    /// A model of its own for this call; None: the smart features' model.
    pub model: Option<String>,
    /// The tools it may use.
    pub tools: Tools,
    /// The folder it runs in; None: the one work folder (`work_dir`), where nothing it does is shown as a session.
    pub cwd: Option<PathBuf>,
    /// A JSON Schema the answer follows: the answer is then JSON that follows it.
    pub json_schema: Option<String>,
    /// How hard the model thinks: "low" or "medium". At "low" hidden thinking is turned off too.
    pub effort: &'static str,
}

impl Request {
    pub fn new(feature: &'static str, system: impl Into<String>, prompt: impl Into<String>) -> Self {
        Request {
            feature,
            system: system.into(),
            prompt: prompt.into(),
            timeout: DEFAULT_TIMEOUT,
            cancel: Cancel::new(),
            user_initiated: false,
            model: None,
            tools: Tools::None,
            cwd: None,
            json_schema: None,
            effort: "low",
        }
    }
}

/// A flag to stop one call. Clone it into the request and keep a copy: `cancel()` kills the call's process.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// The cancel flags of the calls in flight, per feature. Turning a switch off cancels what was started under it; a
/// call started after that gets a fresh flag. The app owns one and calls `apply` whenever the settings change.
#[derive(Default)]
pub struct Switchboard {
    flags: Mutex<HashMap<String, Cancel>>,
}

impl Switchboard {
    pub fn new() -> Self {
        Self::default()
    }

    /// The flag for a new call of `feature`.
    pub fn token(&self, feature: &str) -> Cancel {
        self.flags.lock().unwrap().entry(feature.to_string()).or_default().clone()
    }

    pub fn cancel(&self, feature: &str) {
        if let Some(c) = self.flags.lock().unwrap().remove(feature) {
            c.cancel();
        }
    }

    /// Cancel every call whose feature is no longer allowed to run under these settings.
    pub fn apply(&self, settings: &Settings) {
        let mut flags = self.flags.lock().unwrap();
        let off: Vec<String> =
            flags.keys().filter(|id| !(settings.feature_on(id) || (switchless(id) && settings.tokens_enabled))).cloned().collect();
        for id in off {
            if let Some(c) = flags.remove(&id) {
                c.cancel();
            }
        }
    }
}

/// A finished, understood call.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    pub text: String,
    pub usage: Usage,
    /// Claude did not report usage, so `usage` is a rough guess (characters / 4) kept only so the daily limit still counts something.
    pub usage_estimated: bool,
    /// What Claude says the call cost. 0 when it didn't say.
    pub cost_usd: f64,
    pub model: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AiError {
    /// Not started.
    Refused(Refusal),
    /// The caller cancelled (a switch was turned off): any result was thrown away.
    Cancelled,
    /// It ran on `agent` and failed. `detail` is the agent's own error text, cut short, for whoever is debugging: it
    /// is never saved.
    Failed { agent: Agent, kind: FailKind, detail: String },
}

impl AiError {
    pub fn plain(&self) -> String {
        match self {
            AiError::Refused(r) => r.plain(),
            AiError::Cancelled => "The request was cancelled.".into(),
            AiError::Failed { agent, kind, .. } => {
                let mut s = kind.plain(*agent);
                s[..1].make_ascii_uppercase();
                s.push('.');
                s
            }
        }
    }
}

/// Sort a failure by what Claude (or the system) said. Order matters: being signed out is checked before the plan limit.
pub fn classify(text: &str) -> FailKind {
    let t = text.to_lowercase();
    let any = |words: &[&str]| words.iter().any(|w| t.contains(w));
    let status = |code: &str| {
        t.match_indices(code).any(|(i, _)| {
            let before = t[..i].chars().next_back().is_none_or(|c| !c.is_ascii_digit());
            let after = t[i + code.len()..].chars().next().is_none_or(|c| !c.is_ascii_digit());
            before && after
        })
    };
    if any(&[
        "not logged in",
        "not signed in",
        "/login",
        "invalid api key",
        "authentication",
        "unauthorized",
        "oauth token",
        "token has expired",
        "sign in",
    ]) || status("401")
    {
        FailKind::NotSignedIn
    } else if any(&[
        "usage limit",
        "hit your limit",
        "limit reached",
        "rate limit",
        "rate_limit",
        "too many requests",
        "quota",
        "credit balance",
        "resets at",
    ]) || status("429")
    {
        FailKind::RateLimited
    } else if any(&[
        "enotfound",
        "econnrefused",
        "econnreset",
        "etimedout",
        "eai_again",
        "network",
        "could not resolve",
        "connection error",
        "connection refused",
        "connection reset",
        "unable to connect",
        "fetch failed",
        "socket hang up",
        "getaddrinfo",
        "offline",
        "dns",
    ]) {
        FailKind::Network
    } else {
        FailKind::Other
    }
}

/// The first JSON value in `text` that parses: the whole text, or what is inside a code fence, or the outermost
/// object or array found after some chatter. None when there is none.
pub fn extract_json(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return Some(v);
    }
    // Code fences: take what is inside the first pair, whatever the language tag says.
    if let Some(start) = trimmed.find("```") {
        let after = &trimmed[start + 3..];
        let body_start = after.find('\n').map_or(0, |i| i + 1);
        if let Some(end) = after[body_start..].find("```")
            && let Ok(v) = serde_json::from_str::<Value>(after[body_start..body_start + end].trim())
        {
            return Some(v);
        }
    }
    // The outermost balanced object or array, respecting strings. A few starting points are tried; garbage stays cheap.
    let bytes = trimmed.as_bytes();
    let mut tried = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if b != b'{' && b != b'[' {
            continue;
        }
        tried += 1;
        if tried > 20 {
            break;
        }
        let (mut depth, mut in_str, mut escaped) = (0i32, false, false);
        for (j, &c) in bytes.iter().enumerate().skip(i) {
            if in_str {
                match (escaped, c) {
                    (true, _) => escaped = false,
                    (false, b'\\') => escaped = true,
                    (false, b'"') => in_str = false,
                    _ => {}
                }
                continue;
            }
            match c {
                b'"' => in_str = true,
                b'{' | b'[' => depth += 1,
                b'}' | b']' => {
                    depth -= 1;
                    if depth == 0 {
                        if let Ok(v) = serde_json::from_str::<Value>(&trimmed[i..=j]) {
                            return Some(v);
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// What came out of a one-shot call, understood (by its agent: `Agent::one_shot_outcome`).
pub struct Outcome {
    /// The answer, or why there isn't one.
    pub result: Result<String, (FailKind, String)>,
    /// What Claude reported (present even for a failed call, when the JSON had it).
    pub usage: Option<Usage>,
    pub cost_usd: f64,
}

/// The one folder every call runs in, under platoon's cache folder. Defined here only: whatever must recognise or
/// hide the runner's own sessions asks for it.
pub fn work_dir(cache: &Path) -> PathBuf {
    cache.join("ai-work")
}

/// Runs a one-shot call for a request on the agent its model needs (`claude -p`, `codex exec`), honoring every gate.
/// Cheap to make; blocking; use it from a thread.
pub struct Runner {
    /// The AI programs it may run: what decides the models there are, and which program a model runs on.
    providers: Providers,
    cache: PathBuf,
    /// Seconds added to the clock this runner reads (tests move it to cross midnight without waiting).
    offset: u64,
}

impl Runner {
    pub fn new(cache: &Path, providers: Providers) -> Runner {
        Runner { providers, cache: cache.to_path_buf(), offset: 0 }
    }

    /// Claude Code only, run as `program` (a stand-in).
    #[cfg(test)]
    pub fn with_program(cache: &Path, program: &str) -> Runner {
        Runner::new(cache, Providers { claude: Some(program.to_string()), ..Providers::default() })
    }

    /// The same runner with its clock `secs` seconds ahead.
    pub fn with_clock_offset(mut self, secs: u64) -> Runner {
        self.offset = secs;
        self
    }

    fn now(&self) -> u64 {
        usage::now() + self.offset
    }

    /// The folder every call runs in.
    pub fn work_dir(&self) -> PathBuf {
        work_dir(&self.cache)
    }

    /// The real `claude` or `codex` must never be reached by a test: in unit tests it panics, and in a test run of the
    /// binary (PLATOON_FORBID_REAL_CLAUDE set) the real `claude` is refused (the tests' `codex` is never a bare name).
    fn check_program(program: &str) -> Result<(), AiError> {
        if program != "claude" && program != "codex" {
            return Ok(());
        }
        if cfg!(test) {
            panic!("a test tried to run the real `{program}`: give the runner a stand-in (Runner::with_program, Providers)");
        }
        if program == "codex" {
            return Ok(());
        }
        if std::env::var_os("PLATOON_FORBID_REAL_CLAUDE").is_some() {
            return Err(AiError::Failed {
                agent: Agent::Claude,
                kind: FailKind::Other,
                detail: "PLATOON_FORBID_REAL_CLAUDE is set and PLATOON_CLAUDE_BIN is not: refusing to run the real claude".into(),
            });
        }
        Ok(())
    }

    pub fn run(&self, settings: &Settings, req: &Request) -> Result<Reply, AiError> {
        let start = self.now();
        let gated = !part_of_a_task(req.feature);
        if gated {
            switches(settings, req.feature).map_err(AiError::Refused)?;
        }
        let (model, agent) = model_of(settings, req, &self.providers.catalog()).map_err(AiError::Refused)?;
        if gated {
            limits(settings, agent, &self.cache, &self.providers, start, req.user_initiated).map_err(AiError::Refused)?;
        }
        if req.cancel.is_cancelled() {
            return Err(AiError::Cancelled);
        }
        let program = self.providers.program(agent).unwrap_or_default().to_string();
        Self::check_program(&program)?;
        let ledger = Ledger::open(&self.cache);
        let other = |detail: String| AiError::Failed { agent, kind: FailKind::Other, detail };
        let work = self.work_dir();
        std::fs::create_dir_all(&work).map_err(|e| other(format!("couldn't create the work folder: {e}")))?;
        let cwd = req.cwd.clone().unwrap_or_else(|| work.clone());
        // An agent that reads the answer's schema from a file gets one in the work folder, gone with the call.
        let schema = match &req.json_schema {
            Some(text) if agent.schema_in_file() => {
                Some(SchemaFile::write(&work, text).map_err(|e| other(format!("couldn't write the answer's schema: {e}")))?)
            }
            _ => None,
        };

        let mut cmd = Command::new(&program);
        agent.one_shot(&mut cmd, &model, req, &cwd, schema.as_ref().map(|s| s.0.as_path()));
        cmd.current_dir(&cwd)
            .env("PLATOON_AI", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Its own process group: what it starts dies with it, and nothing else is ever signalled.
            .process_group(0);
        // Started from inside a Claude Code session, platoon inherits that session's markers; they aren't this call's
        // to carry (which ones, and why: `claude::SESSION_MARKERS`).
        for k in crate::agents::inherited_session_markers() {
            cmd.env_remove(k);
        }
        // Run from a task's hook (the check of its work), platoon has the task in its environment: the call is no part
        // of the task's session, and Codex's task hooks must not take it for one.
        cmd.env_remove(crate::agents::TASK_ENV);
        let mut child = match spawn_retrying(&mut cmd) {
            Ok(c) => c,
            Err(e) => {
                let why = if e.kind() == io::ErrorKind::NotFound {
                    format!("couldn't find `{program}` to run")
                } else {
                    format!("couldn't start `{program}`: {e}")
                };
                if gated {
                    let _ = ledger.note_failure(self.now(), agent, FailKind::Other);
                }
                return Err(other(why));
            }
        };
        let pid = child.id();
        let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let prompt = req.prompt.clone().into_bytes();
        thread::spawn(move || {
            if let Some(mut s) = stdin {
                let _ = s.write_all(&prompt); // a child that has already exited just closes the pipe
            }
        });
        let out_rx = read_capped(stdout);
        let err_rx = read_capped(stderr);

        let began = Instant::now();
        let (mut cancelled, mut timed_out) = (false, false);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {}
                Err(_) => break None,
            }
            if req.cancel.is_cancelled() {
                cancelled = true;
                kill_group(pid, &mut child);
                break None;
            }
            if began.elapsed() >= req.timeout {
                timed_out = true;
                kill_group(pid, &mut child);
                break None;
            }
            thread::sleep(Duration::from_millis(10));
        };
        let collect = |rx: mpsc::Receiver<Vec<u8>>| rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default();
        let (stdout, stderr) = (collect(out_rx), collect(err_rx));
        let now = self.now();

        if cancelled {
            return Err(AiError::Cancelled);
        }
        if timed_out {
            if gated {
                let _ = ledger.note_failure(now, agent, FailKind::TimedOut);
            }
            return Err(AiError::Failed {
                agent,
                kind: FailKind::TimedOut,
                detail: format!("no answer within {} seconds", req.timeout.as_secs()),
            });
        }
        drop(schema);
        let outcome = agent.one_shot_outcome(req, &stdout, &stderr, status.is_some_and(|s| s.success()));
        let ok = outcome.result.is_ok();
        // Whatever Claude said it used counts, even if the call failed. If it said nothing about a good call, a rough
        // figure still counts, so the daily limit can't be silently bypassed.
        let (used, estimated) = match (outcome.usage, &outcome.result) {
            (Some(u), _) => (Some(u), false),
            (None, Ok(text)) => (
                Some(Usage {
                    input: ((req.system.len() + req.prompt.len()) / 4) as u64,
                    output: (text.len() / 4) as u64,
                    ..Usage::default()
                }),
                true,
            ),
            (None, Err(_)) => (None, false),
        };
        if let Some(u) = &used {
            let _ = ledger.record_call(now, agent, req.feature, u, outcome.cost_usd, ok);
        }
        match outcome.result {
            Ok(text) => {
                if gated {
                    let _ = ledger.note_success(agent);
                }
                if req.cancel.is_cancelled() {
                    return Err(AiError::Cancelled); // finished, but the switch was turned off meanwhile: the result is thrown away
                }
                Ok(Reply { text, usage: used.unwrap_or_default(), usage_estimated: estimated, cost_usd: outcome.cost_usd, model })
            }
            Err((kind, detail)) => {
                if gated {
                    let _ = ledger.note_failure(now, agent, kind);
                }
                Err(AiError::Failed { agent, kind, detail })
            }
        }
    }
}

/// The model `req` runs on and the agent that runs it: its own model, else the one the settings pick for its feature
/// (on this computer, `catalog`). Refused, saying what is missing, when no installed agent runs that model.
pub fn model_of(settings: &Settings, req: &Request, catalog: &Catalog) -> Result<(String, Agent), Refusal> {
    model_for(settings, req.feature, req.model.as_deref(), catalog)
}

/// `model_of` for a call of `feature` whose own model is `own`.
fn model_for(settings: &Settings, feature: &str, own: Option<&str>, catalog: &Catalog) -> Result<(String, Agent), Refusal> {
    let (model, setting) = match own {
        Some(m) => (Some(m.to_string()), None),
        None => {
            let (m, which) = settings.model_for(feature, catalog);
            (m, Some(which))
        }
    };
    let Some(model) = model else {
        return Err(Refusal::NoModel(
            "Neither Claude Code nor Codex is installed on this computer, so there is no model to ask.".into(),
            None,
        ));
    };
    match catalog.agent_of(&model) {
        Some(agent) => Ok((model, agent)),
        None => {
            let mut why = catalog.missing(&model).unwrap_or_default();
            let off = crate::agents::needs(&model).filter(|a| catalog.is_off(*a));
            match (setting, off) {
                (Some(which), Some(a)) => why.push_str(&format!(
                    " Pick another for \"{}\" in platoon's settings, or switch {} back on there.",
                    which.setting_name(),
                    a.name()
                )),
                (Some(which), None) => why.push_str(&format!(" Pick another for \"{}\" in platoon's settings.", which.setting_name())),
                (None, _) => {}
            }
            Err(Refusal::NoModel(why, off))
        }
    }
}

/// A JSON Schema written to a file for one call (`Agent::schema_in_file`), removed when the call is over.
struct SchemaFile(PathBuf);

impl SchemaFile {
    fn write(dir: &Path, schema: &str) -> io::Result<SchemaFile> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = dir.join(format!("schema-{}-{}.json", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
        std::fs::write(&path, schema)?;
        Ok(SchemaFile(path))
    }
}

impl Drop for SchemaFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Start the process, trying again a few times if the program is a script that another thread has only just finished
/// writing ("text file busy").
pub(crate) fn spawn_retrying(cmd: &mut Command) -> io::Result<std::process::Child> {
    let mut tries = 0;
    loop {
        match cmd.spawn() {
            Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) && tries < 50 => {
                tries += 1;
                thread::sleep(Duration::from_millis(20));
            }
            other => return other,
        }
    }
}

/// Read a pipe to its end on a thread, keeping at most `MAX_OUTPUT` bytes (the rest is read and dropped, so the
/// child never blocks on a full pipe).
fn read_capped(pipe: Option<impl Read + Send + 'static>) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut kept = Vec::new();
        if let Some(mut p) = pipe {
            let mut buf = [0u8; 16 * 1024];
            while let Ok(n) = p.read(&mut buf) {
                if n == 0 {
                    break;
                }
                if kept.len() < MAX_OUTPUT {
                    kept.extend_from_slice(&buf[..n.min(MAX_OUTPUT - kept.len())]);
                }
            }
        }
        let _ = tx.send(kept);
    });
    rx
}

/// Kill the child we started and everything in its process group (it made the group, so the group is ours), then
/// collect it. Only called while the child has not been collected, so its pid is still ours.
fn kill_group(pid: u32, child: &mut std::process::Child) {
    // SAFETY: a negative pid signals the group whose id is `pid`: the group this child created for itself.
    unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
    let _ = child.kill();
    let _ = child.wait();
}

// ---- `platoon tokens` ------------------------------------------------------------------------------------

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_interrupt(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// Start listening for Ctrl+C: from now on it sets a flag (`interrupted`) instead of ending the program, so whoever is
/// waiting for a call can stop it (the call runs in its own process group and never sees the key).
pub fn watch_interrupt() {
    INTERRUPTED.store(false, Ordering::SeqCst);
    // SAFETY: the handler only stores into an atomic.
    unsafe { libc::signal(libc::SIGINT, on_interrupt as *const () as libc::sighandler_t) };
}

/// Ctrl+C ends the program again, as it does by default.
pub fn stop_watching_interrupt() {
    // SAFETY: restoring the default action of a signal.
    unsafe { libc::signal(libc::SIGINT, libc::SIG_DFL) };
}

/// Has Ctrl+C been pressed since `watch_interrupt`?
pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

/// `platoon tokens [status | test]`. Returns the exit code: 0 fine, 1 the call failed, 2 usage, 3 refused by a gate.
pub fn tokens_cli(args: &[String], cache: &Path, config: &Path, providers: &Providers, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let (store, now) = (crate::settings::SettingsStore::open(config), usage::now());
    let settings = store.get();
    match args.first().map(String::as_str) {
        None | Some("status") => {
            let s = status(&settings, cache, providers, now);
            let onoff = |b: bool| if b { "on" } else { "off" };
            let _ = writeln!(out, "Features that use tokens: {}", onoff(s.master));
            for (f, on) in &s.features {
                let _ = writeln!(out, "  {}: {}", f.label, onoff(*on));
            }
            let catalog = providers.catalog();
            for which in [ModelFor::Smart, ModelFor::Notes] {
                let model = which.in_effect(&settings, &catalog);
                let _ = writeln!(out, "{}: {}", which.setting_name(), model.map_or_else(|| "none".into(), |m| model_words(&catalog, &m)));
            }
            // Each program installed has its own count against the daily limit, its own plan and its own back-off; with
            // only one, it goes without saying which.
            let mut agents: Vec<Agent> = Agent::ALL.into_iter().filter(|a| catalog.installed(*a)).collect();
            if agents.is_empty() {
                agents.push(Agent::Claude);
            }
            let both = agents.len() > 1;
            for agent in agents {
                let (on, of) = if both { (format!(" on {}", agent.name()), format!(" of {}", agent.name())) } else { Default::default() };
                let sp = spend(&settings, cache, providers, agent, now);
                let _ = writeln!(
                    out,
                    "Used today{on}: {} of {} tokens",
                    crate::settings::thousands(sp.used_today),
                    crate::settings::thousands(sp.cap)
                );
                // What each feature spent today, so it is clear where the tokens went.
                for (id, used) in &Ledger::open(cache).today(now, agent).features {
                    let label = feature_label(id);
                    let _ = writeln!(
                        out,
                        "  {label}: {} tokens in {} call{}",
                        crate::settings::thousands(used.tokens),
                        used.calls,
                        if used.calls == 1 { "" } else { "s" }
                    );
                }
                match sp.plan {
                    Some((pct, w)) => {
                        let _ = writeln!(
                            out,
                            "Plan usage{of}: {pct}% ({w}); pause above {}",
                            crate::settings::percent_label(settings.ai_pause_percent)
                        );
                    }
                    None => {
                        let _ = writeln!(out, "Plan usage{of}: no figure reported yet (nothing pauses on it)");
                    }
                }
                if let Some(b) = sp.backoff {
                    let _ = writeln!(out, "Paused after a failure ({}): {}", b.kind.plain(agent), wait_text(b.until.saturating_sub(now)));
                }
            }
            let _ = writeln!(out, "Settings file: {}", store.path().display());
            0
        }
        Some("test") if args.len() == 1 => tokens_test(&settings, &store, cache, providers, out, err),
        Some(_) => {
            let _ = writeln!(
                err,
                "usage: platoon tokens [status | test]\n  status  show whether the features are on and how many tokens were used today\n  test    make one tiny call on the smart features' model (Claude's or Codex's) through the same gates the features use"
            );
            2
        }
    }
}

/// A model as `platoon tokens` says it: its name and the program that runs it ("Haiku (Claude Code)"), or why nothing
/// here does.
fn model_words(catalog: &Catalog, model: &str) -> String {
    match catalog.agent_of(model) {
        Some(agent) => format!("{} ({})", catalog.name(model), agent.name()),
        None => catalog.label(model),
    }
}

fn tokens_test(
    settings: &Settings,
    store: &crate::settings::SettingsStore,
    cache: &Path,
    providers: &Providers,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let mut req =
        Request::new(TEST_FEATURE, "You are a test. Reply with exactly the word asked for, nothing else.", "Reply with the single word ok");
    let refused =
        gate(settings, TEST_FEATURE, cache, providers, usage::now()).and_then(|()| model_of(settings, &req, &providers.catalog()));
    let (model, agent) = match refused {
        Ok(found) => found,
        Err(r) => {
            let _ = writeln!(err, "Not run: {}", r.plain());
            if r.is_switch() {
                let _ = writeln!(err, "{}", enable_hint(&store.path()));
            }
            return 3;
        }
    };
    let _ = writeln!(out, "Asking {} ({}) to reply with one word...", agent.name(), providers.catalog().name(&model));
    let _ = out.flush();
    req.timeout = Duration::from_secs(60);
    // Ctrl+C reaches us but not Claude (it has its own process group), so stop it ourselves.
    INTERRUPTED.store(false, Ordering::SeqCst);
    // SAFETY: the handler only stores into an atomic.
    unsafe { libc::signal(libc::SIGINT, on_interrupt as *const () as libc::sighandler_t) };
    let cancel = req.cancel.clone();
    let done = Arc::new(AtomicBool::new(false));
    let watcher = {
        let done = done.clone();
        thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                if INTERRUPTED.load(Ordering::SeqCst) {
                    cancel.cancel();
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
        })
    };
    let result = Runner::new(cache, providers.clone()).run(settings, &req);
    done.store(true, Ordering::SeqCst);
    let _ = watcher.join();
    let today = |out: &mut dyn Write| {
        let used = Ledger::open(cache).today(usage::now(), agent).tokens();
        let _ = writeln!(
            out,
            "Used today: {} of {} tokens",
            crate::settings::thousands(used),
            crate::settings::thousands(settings.ai_daily_tokens)
        );
    };
    match result {
        Ok(reply) => {
            let u = reply.usage;
            let _ = writeln!(out, "Reply: {}", reply.text.trim());
            let _ = writeln!(
                out,
                "Tokens used: {} (input {}, cache write {}, cache read {}, output {}){}",
                crate::settings::thousands(u.total()),
                u.input,
                u.cache_creation,
                u.cache_read,
                u.output,
                if reply.usage_estimated {
                    format!(" - {} did not report usage; this is a rough estimate", agent.short())
                } else {
                    String::new()
                }
            );
            // Claude Code says what a call cost; Codex doesn't.
            if agent == Agent::Claude {
                let _ = writeln!(out, "Cost Claude reported: ${:.4}", reply.cost_usd);
            }
            today(out);
            0
        }
        Err(AiError::Refused(r)) => {
            let _ = writeln!(err, "Not run: {}", r.plain());
            3
        }
        Err(AiError::Cancelled) => {
            let _ = writeln!(err, "Cancelled.");
            1
        }
        Err(e @ AiError::Failed { .. }) => {
            let _ = writeln!(err, "The call failed: {}", e.plain());
            if let AiError::Failed { agent, detail, .. } = &e {
                let _ = writeln!(err, "  {} said: {detail}", agent.short());
            }
            if let Some(b) = Ledger::open(cache).load().of(agent).backoff {
                let _ = writeln!(err, "Smart features now wait {} before trying again.", wait_text(b.until.saturating_sub(usage::now())));
            }
            today(out);
            1
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::agents::claude::interpret;
    use std::{fs, os::unix::fs::PermissionsExt, sync::atomic::AtomicUsize};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    pub(crate) struct Fake {
        pub dir: PathBuf,
        pub cache: PathBuf,
        pub script: String,
    }

    impl Fake {
        /// A stand-in for `claude`: records its arguments (one per line, NUL-free), folder, environment and stdin under
        /// `dir/rec.*`, then runs `body` (a shell snippet).
        pub fn new(name: &str, body: &str) -> Fake {
            let dir = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!(
                "platoon-ai-{name}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            )));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            let cache = dir.join("cache");
            let script = dir.join("fake-claude");
            let d = dir.display();
            let text = format!(
                "#!/bin/sh\ndir='{d}'\nrec=\"$dir/rec\"\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$rec.argv\"\npwd > \"$rec.cwd\"\nenv > \"$rec.env\"\ncat > \"$rec.stdin\"\n{body}\n"
            );
            fs::write(&script, text).unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
            Fake { script: script.display().to_string(), dir, cache }
        }

        pub fn ok(name: &str) -> Fake {
            Fake::new(name, &format!("printf '%s' '{}'", ok_json("ok", 400, 0, 17, 5)))
        }

        pub fn runner(&self) -> Runner {
            Runner::with_program(&self.cache, &self.script)
        }

        pub fn rec(&self, what: &str) -> String {
            fs::read_to_string(self.dir.join(format!("rec.{what}"))).unwrap_or_default()
        }

        pub fn ran(&self) -> bool {
            self.dir.join("rec.argv").exists()
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    pub(crate) fn ok_json(text: &str, input: u64, cc: u64, cr: u64, output: u64) -> String {
        format!(
            r#"{{"type":"result","subtype":"success","is_error":false,"result":"{text}","total_cost_usd":0.000612,"usage":{{"input_tokens":{input},"cache_creation_input_tokens":{cc},"cache_read_input_tokens":{cr},"output_tokens":{output}}},"session_id":"x"}}"#
        )
    }

    /// This computer as a test's gate sees it: Claude Code (a stand-in, never run by the gate) and no Codex.
    pub(crate) fn claude_here() -> Providers {
        Providers { claude: Some("/nonexistent/platoon-unit-test/claude".into()), codex: None, off: Vec::new() }
    }

    pub(crate) fn on() -> Settings {
        let mut s = Settings { tokens_enabled: true, ..Settings::default() };
        s.set_feature("names", true);
        s.set_feature("find", true);
        s
    }

    /// Runs for five minutes unless killed, with a child of its own; both pids are written down.
    const HANG: &str = "echo $$ > \"$dir/pid\"\nsleep 300 &\necho $! > \"$dir/sleep\"\nwait";

    fn req() -> Request {
        Request::new("names", "Be brief.", "Name these:\nfirst\nsecond")
    }

    fn alive(pid: &str) -> bool {
        pid.trim().parse::<u32>().is_ok_and(crate::procs::pid_alive)
    }

    /// Longer than a process ever takes to end or start even on a loaded machine: waits return as soon as it has.
    const PATIENCE: Duration = Duration::from_secs(60);

    fn wait_dead(pidfile: &Path) {
        let since = std::time::Instant::now();
        while since.elapsed() < PATIENCE {
            if !alive(&fs::read_to_string(pidfile).unwrap_or_default()) {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("process in {} is still alive", pidfile.display());
    }

    // ---- the feature table and the gate ----

    #[test]
    fn the_feature_table_has_plain_unique_rows() {
        let ids: Vec<&str> = FEATURES.iter().map(|f| f.id).collect();
        assert_eq!(ids, ["names", "find", "project_notes", "project_description"]);
        for f in FEATURES {
            assert!(!f.label.is_empty() && f.description.ends_with('.') && f.description.len() < 200, "{}", f.id);
            assert_ne!(f.id, TEST_FEATURE);
        }
        // The newer rows are on as soon as the master switch is: the first two were there before anyone was asked.
        assert_eq!(
            FEATURES.iter().filter(|f| f.on_unless_switched_off).map(|f| f.id).collect::<Vec<_>>(),
            ["project_notes", "project_description"]
        );
        assert!(
            on_by_default("project_notes") && on_by_default("project_description") && !on_by_default("names") && !on_by_default("nonsense")
        );
        // Work with no switch of its own is not in the table, has a name of its own, and is not a feature id twice over.
        assert!(SWITCHLESS.iter().all(|(id, _)| !ids.contains(id)));
        assert_eq!((feature_label("project_notes"), feature_label(crate::handoff::FEATURE)), ("Project notes", "Hand-off"));
        assert_eq!(feature_label("something else"), "something else");
        assert!(switchless(TEST_FEATURE) && switchless(crate::handoff::FEATURE) && !switchless("names"));
    }

    #[test]
    fn everything_is_off_by_default_and_the_master_switch_overrides_every_feature() {
        let d = Settings::default();
        for f in FEATURES {
            assert!(!d.feature_on(f.id), "{}", f.id);
        }
        let mut s = on();
        for f in FEATURES {
            assert!(s.feature_on(f.id));
        }
        s.tokens_enabled = false;
        for f in FEATURES {
            assert!(!s.feature_on(f.id), "master off makes {} off", f.id);
            assert!(s.feature_toggle(f.id), "but its own switch is remembered");
        }
        let cache = std::env::temp_dir().join("platoon-ai-never-created");
        for f in FEATURES {
            assert_eq!(gate(&s, f.id, &cache, &claude_here(), 1_800_000_000), Err(Refusal::MasterOff));
        }
        assert_eq!(gate(&s, TEST_FEATURE, &cache, &claude_here(), 1_800_000_000), Err(Refusal::MasterOff));
        assert!(!cache.exists(), "the gate creates nothing");
    }

    #[test]
    fn a_feature_needs_its_own_switch_and_the_test_call_does_not() {
        let cache = std::env::temp_dir().join("platoon-ai-gate-own");
        let mut s = Settings { tokens_enabled: true, ..Settings::default() };
        assert_eq!(gate(&s, "names", &cache, &claude_here(), 1_800_000_000), Err(Refusal::FeatureOff("Smart names and summaries")));
        s.set_feature("names", true);
        assert_eq!(gate(&s, "names", &cache, &claude_here(), 1_800_000_000), Ok(()));
        assert_eq!(gate(&s, "find", &cache, &claude_here(), 1_800_000_000), Err(Refusal::FeatureOff("Find a session by description")));
        assert_eq!(gate(&s, TEST_FEATURE, &cache, &claude_here(), 1_800_000_000), Ok(()));
        assert_eq!(gate(&s, "nonsense", &cache, &claude_here(), 1_800_000_000), Err(Refusal::UnknownFeature("nonsense".into())));
    }

    #[test]
    fn the_daily_limit_refuses_at_and_over_and_starts_over_tomorrow() {
        let f = Fake::ok("cap");
        let mut s = on();
        s.ai_daily_tokens = 20_000;
        let now = 1_800_000_000;
        let l = Ledger::open(&f.cache);
        l.record_call(now, Agent::Claude, "names", &Usage { input: 5000, cache_creation: 5000, cache_read: 5000, output: 4999 }, 0.0, true)
            .unwrap();
        assert_eq!(gate(&s, "names", &f.cache, &claude_here(), now), Ok(()), "one under the limit still runs");
        l.record_call(now, Agent::Claude, "names", &Usage { output: 1, ..Usage::default() }, 0.0, true).unwrap();
        assert_eq!(
            gate(&s, "names", &f.cache, &claude_here(), now),
            Err(Refusal::OverBudget { used: 20_000, cap: 20_000 }),
            "all four numbers count, and at the limit is refused"
        );
        assert_eq!(gate(&s, "names", &f.cache, &claude_here(), now + 3 * 86_400), Ok(()), "another day is a fresh count");
        let msg = Refusal::OverBudget { used: 20_000, cap: 20_000 }.plain();
        assert!(msg.contains("20,000 of 20,000") && msg.contains("tomorrow"), "{msg}");
    }

    #[test]
    fn every_reason_the_gate_can_give_turns_into_words_a_screen_can_show() {
        let now = 1_800_000_000;
        let cases = [
            Refusal::MasterOff,
            Refusal::FeatureOff("Project notes"),
            Refusal::OverBudget { used: 25_000, cap: 20_000 },
            Refusal::PlanBusy { agent: Agent::Claude, pct: 82, limit: 80, window: "weekly limit".into() },
            Refusal::BackedOff { agent: Agent::Claude, kind: crate::ledger::FailKind::NotSignedIn, until: now + 1800, now },
        ];
        for r in &cases {
            let full = r.on_screen();
            assert!(full.starts_with(&r.plain()), "the gate's own reason comes first: {full}");
            assert!(full.ends_with('.') && !full.contains("  "), "{full}");
            assert!(full.chars().next().unwrap().is_uppercase() || full.starts_with('"'), "{full}");
            let short = r.short();
            assert!(short.len() < 70 && !short.ends_with('.') && short == short.trim(), "the header has little room: {short:?}");
            for jargon in ["gate", "refus", "token bucket", "backoff", "API"] {
                assert!(!full.contains(jargon) && !short.contains(jargon), "{jargon:?} in {full:?} / {short:?}");
            }
        }
        // Where to put it right, for the reasons that don't already say it themselves.
        assert_eq!(
            Refusal::MasterOff.on_screen(),
            "\"Enable features which require tokens\" is switched off. Press s for the settings to switch it back on."
        );
        assert_eq!(
            Refusal::FeatureOff("Project notes").on_screen(),
            "\"Project notes\" is switched off. Press s for the settings to switch it back on."
        );
        assert_eq!(
            Refusal::PlanBusy { agent: Agent::Claude, pct: 82, limit: 80, window: "weekly limit".into() }.on_screen(),
            "Your Claude plan is 82% used (weekly limit), above the pause point of 80%. Smart features wait until it drops. \
             Press s for the settings to change the pause point."
        );
        // These two say it in their own words already, so nothing is added.
        assert_eq!(
            Refusal::OverBudget { used: 25_000, cap: 20_000 }.on_screen(),
            Refusal::OverBudget { used: 25_000, cap: 20_000 }.plain()
        );
        let backed = Refusal::BackedOff { agent: Agent::Claude, kind: crate::ledger::FailKind::Network, until: now + 300, now };
        assert_eq!(backed.on_screen(), backed.plain());
        assert!(backed.on_screen().contains("trying again in 5 minutes"), "{}", backed.on_screen());
        // The short forms, for the header.
        assert_eq!(Refusal::MasterOff.short(), "smart features off");
        assert_eq!(Refusal::FeatureOff("Project notes").short(), "project notes off");
        assert_eq!(Refusal::OverBudget { used: 25_000, cap: 20_000 }.short(), "smart features paused · today's 20,000 limit is used up");
        assert_eq!(
            Refusal::PlanBusy { agent: Agent::Claude, pct: 82, limit: 80, window: "weekly limit".into() }.short(),
            "smart features paused · your Claude plan is 82% used (weekly limit)"
        );
        assert_eq!(backed.short(), "smart features paused after a failure · trying again in 5 minutes");
    }

    #[test]
    fn the_reason_a_feature_is_stopped_is_the_one_the_gate_gives_it() {
        let f = Fake::ok("stopped");
        let now = 1_800_000_000;
        let mut s = Settings::default();
        // The master switch off, then this feature's own switch off, then both on.
        assert_eq!(
            stopped_reason(&s, crate::notesrun::FEATURE, &f.cache, &claude_here(), now).as_deref(),
            Some(Refusal::MasterOff.on_screen().as_str())
        );
        s.tokens_enabled = true;
        s.set_feature(crate::notesrun::FEATURE, false);
        let said = stopped_reason(&s, crate::notesrun::FEATURE, &f.cache, &claude_here(), now).unwrap();
        assert!(said.starts_with("\"Project notes\" is switched off."), "{said}");
        s.set_feature(crate::notesrun::FEATURE, true);
        assert_eq!(stopped_reason(&s, crate::notesrun::FEATURE, &f.cache, &claude_here(), now), None, "nothing in the way: nothing to say");
        // The daily limit, spent.
        Ledger::open(&f.cache)
            .record_call(now, Agent::Claude, crate::notesrun::FEATURE, &Usage { input: 600_000, ..Usage::default() }, 0.0, true)
            .unwrap();
        let said = stopped_reason(&s, crate::notesrun::FEATURE, &f.cache, &claude_here(), now).unwrap();
        assert!(said.contains("Today's token limit is used up (600,000 of 500,000 tokens)"), "{said}");
    }

    #[test]
    fn the_header_speaks_only_for_what_stops_everything_and_shrinks_to_fit() {
        let f = Fake::ok("header");
        let now = 1_800_000_000;
        // Off by the user's own choice, or one feature's switch: not the header's business.
        assert_eq!(
            header_note(&Settings::default(), &f.cache, &claude_here(), now),
            None,
            "a master switch the user left off doesn't follow them around"
        );
        let mut s = on();
        s.set_feature("names", false);
        assert_eq!(header_note(&s, &f.cache, &claude_here(), now), None, "nor does one feature's own switch");
        // The daily limit: said, with where to change it when there is room.
        s.ai_daily_tokens = 20_000;
        Ledger::open(&f.cache).record_call(now, Agent::Claude, "names", &Usage { input: 25_000, ..Usage::default() }, 0.0, true).unwrap();
        let (short, full) = header_note(&s, &f.cache, &claude_here(), now).expect("the header says so");
        assert_eq!(short, "smart features paused · today's 20,000 limit is used up");
        assert_eq!(full, "smart features paused · today's 20,000 limit is used up · press s for the settings");
        // A back-off tries again by itself, so there is nothing to press.
        let f2 = Fake::ok("header-backoff");
        Ledger::open(&f2.cache).note_failure(now, Agent::Claude, crate::ledger::FailKind::Network).unwrap();
        let (short, full) = header_note(&on(), &f2.cache, &claude_here(), now).expect("a back-off is said too");
        assert_eq!(short, full);
        assert!(short.contains("after a failure"), "{short}");
    }

    fn plan_report(cache: &Path, five: u8, seven: Option<u8>, resets: u64, ts: u64) {
        let pct = |p: u8| serde_json::json!({"pct": p, "resets_at": resets});
        let v = serde_json::json!({"ts": ts, "context_pct": null, "five_hour": pct(five), "seven_day": seven.map(pct)});
        fs::create_dir_all(cache.join("status")).unwrap();
        fs::write(cache.join("status/aaaa.json"), v.to_string()).unwrap();
    }

    #[test]
    fn the_plan_pause_uses_the_statusline_figure_above_the_setting_and_never_without_one() {
        let f = Fake::ok("pause");
        let s = on();
        let now = 1_800_000_000;
        assert_eq!(gate(&s, "names", &f.cache, &claude_here(), now), Ok(()), "no figure at all: nothing pauses, nothing is pretended");
        plan_report(&f.cache, 80, Some(10), now + 3600, now - 10);
        assert_eq!(gate(&s, "names", &f.cache, &claude_here(), now), Ok(()), "exactly at the pause point is not above it");
        plan_report(&f.cache, 81, Some(10), now + 3600, now - 10);
        assert_eq!(
            gate(&s, "names", &f.cache, &claude_here(), now),
            Err(Refusal::PlanBusy { agent: Agent::Claude, pct: 81, limit: 80, window: "5-hour limit".into() })
        );
        plan_report(&f.cache, 10, Some(95), now + 3600, now - 10);
        assert_eq!(
            gate(&s, "names", &f.cache, &claude_here(), now),
            Err(Refusal::PlanBusy { agent: Agent::Claude, pct: 95, limit: 80, window: "weekly limit".into() }),
            "the higher window decides"
        );
        // A window that has reset says nothing any more.
        assert_eq!(gate(&s, "names", &f.cache, &claude_here(), now + 7200), Ok(()));
        // A different pause point, and "never".
        let mut lax = s.clone();
        lax.ai_pause_percent = 99;
        assert_eq!(gate(&lax, "names", &f.cache, &claude_here(), now), Ok(()));
        let mut never = s.clone();
        never.ai_pause_percent = 0;
        plan_report(&f.cache, 100, None, now + 3600, now - 10);
        assert_eq!(gate(&never, "names", &f.cache, &claude_here(), now), Ok(()));
        assert!(matches!(gate(&s, "names", &f.cache, &claude_here(), now), Err(Refusal::PlanBusy { pct: 100, .. })));
        assert!(Refusal::PlanBusy { agent: Agent::Claude, pct: 90, limit: 80, window: "5-hour limit".into() }.plain().contains("90%"));
    }

    #[test]
    fn a_call_the_user_asked_for_ignores_the_plan_pause_and_nothing_else() {
        let f = Fake::ok("user-gate");
        let s = on();
        let now = 1_800_000_000;
        plan_report(&f.cache, 95, Some(10), now + 3600, now - 10);
        assert!(
            matches!(gate_for(&s, "find", &f.cache, &claude_here(), now, false), Err(Refusal::PlanBusy { pct: 95, .. })),
            "background: paused"
        );
        assert_eq!(
            gate_for(&s, "find", &f.cache, &claude_here(), now, true),
            Ok(()),
            "asked for by a key press: the plan's own pause is not for it"
        );
        assert_eq!(
            gate(&s, "find", &f.cache, &claude_here(), now),
            gate_for(&s, "find", &f.cache, &claude_here(), now, false),
            "`gate` is the background gate"
        );
        // The switches still count...
        let mut off = s.clone();
        off.set_feature("find", false);
        assert_eq!(gate_for(&off, "find", &f.cache, &claude_here(), now, true), Err(Refusal::FeatureOff("Find a session by description")));
        off = s.clone();
        off.tokens_enabled = false;
        assert_eq!(gate_for(&off, "find", &f.cache, &claude_here(), now, true), Err(Refusal::MasterOff));
        // ...and so do the daily limit and a running back-off.
        let mut small = s.clone();
        small.ai_daily_tokens = 20_000;
        Ledger::open(&f.cache).record_call(now, Agent::Claude, "names", &Usage { input: 20_000, ..Usage::default() }, 0.0, true).unwrap();
        assert!(matches!(gate_for(&small, "find", &f.cache, &claude_here(), now, true), Err(Refusal::OverBudget { .. })));
        let g = Fake::ok("user-gate-2");
        Ledger::open(&g.cache).note_failure(now, Agent::Claude, FailKind::Network).unwrap();
        assert!(matches!(gate_for(&s, "find", &g.cache, &claude_here(), now + 1, true), Err(Refusal::BackedOff { .. })));
    }

    #[test]
    fn the_runner_lets_a_user_initiated_request_through_the_plan_pause_and_a_background_one_not() {
        let f = Fake::ok("user-run");
        let s = on();
        let now = usage::now();
        plan_report(&f.cache, 99, None, now + 3600, now);
        let mut bg = Request::new("find", "Be brief.", "x");
        assert!(matches!(f.runner().run(&s, &bg), Err(AiError::Refused(Refusal::PlanBusy { .. }))));
        assert!(!f.ran());
        bg.user_initiated = true;
        assert!(f.runner().run(&s, &bg).is_ok());
        assert!(f.ran());
    }

    #[test]
    fn a_runner_whose_clock_is_ahead_sees_the_next_day() {
        let f = Fake::ok("clock");
        let mut s = on();
        s.ai_daily_tokens = 20_000;
        Ledger::open(&f.cache)
            .record_call(usage::now(), Agent::Claude, "names", &Usage { input: 30_000, ..Usage::default() }, 0.0, true)
            .unwrap();
        assert!(matches!(f.runner().run(&s, &req()), Err(AiError::Refused(Refusal::OverBudget { .. }))));
        assert!(f.runner().with_clock_offset(2 * 86_400).run(&s, &req()).is_ok(), "two days on, today's use is a different day's");
        let day2 = crate::ledger::local_day(usage::now() + 2 * 86_400);
        assert!(Ledger::open(&f.cache).load().claude.days.contains_key(&day2), "and the call is recorded under that day");
    }

    #[test]
    fn a_running_backoff_refuses_and_an_expired_one_does_not() {
        let f = Fake::ok("backoff-gate");
        let s = on();
        let now = 1_800_000_000;
        Ledger::open(&f.cache).note_failure(now, Agent::Claude, FailKind::RateLimited).unwrap();
        assert_eq!(
            gate(&s, "names", &f.cache, &claude_here(), now + 10),
            Err(Refusal::BackedOff { agent: Agent::Claude, kind: FailKind::RateLimited, until: now + 900, now: now + 10 })
        );
        assert_eq!(gate(&s, "names", &f.cache, &claude_here(), now + 900), Ok(()), "the wait is over");
        let plain = Refusal::BackedOff { agent: Agent::Claude, kind: FailKind::RateLimited, until: now + 900, now: now + 10 }.plain();
        assert!(plain.contains("plan limit") && plain.contains("15 minutes"), "{plain}");
    }

    #[test]
    fn the_status_says_why_features_wait_and_how_much_was_used() {
        let f = Fake::ok("status");
        let s = on();
        let now = 1_800_000_000;
        let st = status(&s, &f.cache, &claude_here(), now);
        let sp = spend(&s, &f.cache, &claude_here(), Agent::Claude, now);
        assert!(st.master && st.paused.is_none() && sp.used_today == 0 && sp.cap == 500_000 && sp.plan.is_none() && sp.backoff.is_none());
        assert_eq!(
            st.features.iter().map(|(f, on)| (f.id, *on)).collect::<Vec<_>>(),
            [("names", true), ("find", true), ("project_notes", true), ("project_description", true)]
        );
        Ledger::open(&f.cache).record_call(now, Agent::Claude, "names", &Usage { input: 1234, ..Usage::default() }, 0.0, true).unwrap();
        Ledger::open(&f.cache).note_failure(now, Agent::Claude, FailKind::Network).unwrap();
        let st = status(&s, &f.cache, &claude_here(), now + 1);
        let sp = spend(&s, &f.cache, &claude_here(), Agent::Claude, now + 1);
        assert_eq!(sp.used_today, 1234);
        assert!(
            matches!(st.paused, Some(Refusal::BackedOff { agent: Agent::Claude, kind: FailKind::Network, .. })) && sp.backoff.is_some()
        );
    }

    // ---- the notice ----

    #[test]
    fn one_notice_line_per_run_of_failures_and_none_while_the_features_are_off() {
        let f = Fake::ok("notice");
        let s = on();
        let now = 1_800_000_000;
        assert_eq!(backoff_notice(&s, &f.cache, now), None);
        let l = Ledger::open(&f.cache);
        l.note_failure(now, Agent::Claude, FailKind::NotSignedIn).unwrap();
        let (id, text) = backoff_notice(&s, &f.cache, now + 5).unwrap();
        assert_eq!(text, "Smart features paused: Claude isn't signed in");
        // The same run keeps the same id, however many failures follow.
        l.note_failure(now + 2000, Agent::Claude, FailKind::NotSignedIn).unwrap();
        l.note_failure(now + 9000, Agent::Claude, FailKind::NotSignedIn).unwrap();
        assert_eq!(backoff_notice(&s, &f.cache, now + 9001).unwrap().0, id);
        assert_eq!(backoff_notice(&s, &f.cache, now + 100_000), None, "nothing to say once the wait is over");
        let mut off = s.clone();
        off.tokens_enabled = false;
        assert_eq!(backoff_notice(&off, &f.cache, now + 9001), None, "features off: nobody is waiting");
        // A new run (after a success) is a new id.
        l.note_success(Agent::Claude).unwrap();
        l.note_failure(now + 200_000, Agent::Claude, FailKind::NotSignedIn).unwrap();
        assert_ne!(backoff_notice(&s, &f.cache, now + 200_001).unwrap().0, id);
    }

    // ---- reading Claude's answer ----

    #[test]
    fn interpret_reads_a_good_answer_with_its_usage_and_cost() {
        let o = interpret(ok_json("hello", 417, 3, 20, 5).as_bytes(), b"", true);
        assert_eq!(o.result, Ok("hello".into()));
        assert_eq!(o.usage, Some(Usage { input: 417, cache_creation: 3, cache_read: 20, output: 5 }));
        assert!((o.cost_usd - 0.000612).abs() < 1e-9);
        // The older list-of-events shape.
        let list = format!("[{{\"type\":\"system\"}},{}]", ok_json("hi", 1, 2, 3, 4));
        let o = interpret(list.as_bytes(), b"", true);
        assert_eq!((o.result, o.usage), (Ok("hi".into()), Some(Usage { input: 1, cache_creation: 2, cache_read: 3, output: 4 })));
        // Chatter around the JSON is tolerated.
        let noisy = format!("warning: something\n{}\n", ok_json("ok", 1, 0, 0, 1));
        assert_eq!(interpret(noisy.as_bytes(), b"", true).result, Ok("ok".into()));
    }

    #[test]
    fn interpret_handles_every_kind_of_bad_output_without_panicking() {
        let kind = |o: Outcome| o.result.unwrap_err().0;
        // is_error with the message in `result`, usage still counted
        let e = r#"{"type":"result","is_error":true,"result":"Not logged in · Please run /login","usage":{"input_tokens":3,"output_tokens":0}}"#;
        let o = interpret(e.as_bytes(), b"", false);
        assert_eq!(o.usage, Some(Usage { input: 3, ..Usage::default() }));
        assert_eq!(kind(o), FailKind::NotSignedIn);
        assert_eq!(kind(interpret(br#"{"is_error":true,"result":"You've hit your limit"}"#, b"", true)), FailKind::RateLimited);
        assert_eq!(kind(interpret(br#"{"is_error":true,"result":"API Error: 429 rate limited"}"#, b"", true)), FailKind::RateLimited);
        assert_eq!(
            kind(interpret(br#"{"is_error":true,"result":"API Error: Unable to connect: ENOTFOUND api.anthropic.com"}"#, b"", true)),
            FailKind::Network
        );
        assert_eq!(kind(interpret(br#"{"is_error":true,"result":"something odd"}"#, b"", true)), FailKind::Other);
        // is_error false but a non-zero exit is still a failure
        assert!(interpret(ok_json("ok", 1, 0, 0, 1).as_bytes(), b"", false).result.is_err());
        // no usage on a good answer
        let o = interpret(br#"{"is_error":false,"result":"ok"}"#, b"", true);
        assert_eq!((o.result, o.usage), (Ok("ok".into()), None));
        // missing result, empty result
        assert_eq!(kind(interpret(br#"{"is_error":false,"usage":{"input_tokens":1}}"#, b"", true)), FailKind::Other);
        assert_eq!(kind(interpret(br#"{"is_error":false,"result":"  "}"#, b"", true)), FailKind::Other);
        // empty, garbage, non-JSON error on stderr, binary
        assert_eq!(kind(interpret(b"", b"", true)), FailKind::Other);
        assert_eq!(kind(interpret(b"", b"", false)), FailKind::Other);
        assert_eq!(kind(interpret(b"<html>502</html>", b"", true)), FailKind::Other);
        assert_eq!(kind(interpret(b"", b"Error: Not logged in", false)), FailKind::NotSignedIn);
        assert_eq!(kind(interpret(b"", b"getaddrinfo EAI_AGAIN api.anthropic.com", false)), FailKind::Network);
        assert_eq!(kind(interpret(&[0xff, 0xfe, 0x00], b"", true)), FailKind::Other);
        assert_eq!(kind(interpret(b"[1,2,3]", b"", true)), FailKind::Other);
        assert_eq!(kind(interpret(b"\"just a string\"", b"", true)), FailKind::Other);
        assert_eq!(kind(interpret(b"null", b"", true)), FailKind::Other);
    }

    #[test]
    fn classification_looks_at_words_not_at_digits_inside_numbers() {
        assert_eq!(classify("HTTP 401"), FailKind::NotSignedIn);
        assert_eq!(classify("used 14290 tokens"), FailKind::Other);
        assert_eq!(classify("error 429"), FailKind::RateLimited);
        assert_eq!(classify("port 4290 closed"), FailKind::Other);
        assert_eq!(classify("Claude AI usage limit reached|1760000000"), FailKind::RateLimited);
        assert_eq!(classify("Invalid API key · Please run /login"), FailKind::NotSignedIn);
    }

    #[test]
    fn plain_messages_never_show_the_error_text_and_read_as_sentences() {
        for kind in [FailKind::NotSignedIn, FailKind::RateLimited, FailKind::Network, FailKind::TimedOut, FailKind::Other] {
            let m = AiError::Failed { agent: Agent::Claude, kind, detail: "SECRET stuff".into() }.plain();
            assert!(!m.contains("SECRET") && m.ends_with('.') && m.chars().next().unwrap().is_uppercase(), "{m}");
        }
        assert_eq!(
            AiError::Failed { agent: Agent::Claude, kind: FailKind::NotSignedIn, detail: String::new() }.plain(),
            "Claude isn't signed in."
        );
        assert_eq!(
            AiError::Failed { agent: Agent::Codex, kind: FailKind::NotSignedIn, detail: String::new() }.plain(),
            "Codex isn't signed in."
        );
    }

    #[test]
    fn extract_json_copes_with_fences_and_chatter() {
        use serde_json::json;
        assert_eq!(extract_json(r#"{"a":1}"#), Some(json!({"a":1})));
        assert_eq!(extract_json("```json\n{\"a\":[1,2]}\n```"), Some(json!({"a":[1,2]})));
        assert_eq!(extract_json("```\n[1,2]\n```\nHope that helps!"), Some(json!([1, 2])));
        assert_eq!(extract_json("Sure! Here you go:\n{\"names\":{\"x\":\"a } b\"}} and more"), Some(json!({"names":{"x":"a } b"}})));
        assert_eq!(extract_json("text [1,{\"k\":\"v\"}] tail"), Some(json!([1, {"k": "v"}])));
        assert_eq!(extract_json("{\"a\": \"esc \\\" quote {\"} x"), Some(json!({"a": "esc \" quote {"})));
        assert_eq!(extract_json("nothing here"), None);
        assert_eq!(extract_json("{ not closed"), None);
        assert_eq!(extract_json(""), None);
        assert_eq!(extract_json("{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{{"), None, "garbage is cheap to give up on");
        assert_eq!(extract_json("{oops} then {\"good\":true}"), Some(json!({"good": true})));
    }

    // ---- the runner against a fake claude ----

    #[test]
    fn a_call_runs_with_exactly_the_agreed_flags_in_the_neutral_folder_with_the_prompt_on_stdin() {
        let f = Fake::ok("argv");
        let reply = f.runner().run(&on(), &req()).unwrap();
        assert_eq!(reply.text, "ok");
        let argv: Vec<String> = f.rec("argv").lines().map(String::from).collect();
        assert_eq!(
            argv,
            [
                "-p",
                "--model",
                "haiku",
                "--tools",
                "",
                "--setting-sources",
                "",
                "--no-session-persistence",
                "--disable-slash-commands",
                "--strict-mcp-config",
                "--system-prompt",
                "Be brief.",
                "--effort",
                "low",
                "--output-format",
                "json"
            ]
        );
        assert!(!argv.iter().any(|a| a == "--bare"), "--bare needs an API key");
        assert!(!argv.iter().any(|a| a.contains("Name these")), "the prompt is not in the arguments");
        assert_eq!(f.rec("stdin"), "Name these:\nfirst\nsecond", "the prompt arrives on stdin, whole");
        let cwd = f.rec("cwd");
        assert_eq!(fs::canonicalize(cwd.trim()).unwrap(), fs::canonicalize(f.cache.join("ai-work")).unwrap(), "always the neutral folder");
        assert!(f.rec("env").lines().any(|l| l == "PLATOON_AI=1"), "{}", f.rec("env"));
        assert!(f.rec("env").lines().any(|l| l == "MAX_THINKING_TOKENS=0"), "{}", f.rec("env"));
    }

    #[test]
    fn the_model_setting_is_passed_on_and_a_big_prompt_goes_through_stdin() {
        let f = Fake::ok("model");
        let mut s = on();
        s.ai_model = Some("sonnet".into());
        let big = "x".repeat(2_000_000);
        let r = Request::new("names", "s", big.clone());
        let reply = f.runner().run(&s, &r).unwrap();
        assert_eq!(reply.model, "sonnet");
        assert!(f.rec("argv").lines().any(|l| l == "sonnet"));
        assert_eq!(f.rec("stdin").len(), big.len());
    }

    #[test]
    fn a_good_call_is_returned_with_usage_and_cost_and_recorded_in_the_ledger() {
        let f = Fake::ok("record");
        let reply = f.runner().run(&on(), &req()).unwrap();
        assert_eq!(reply.usage, Usage { input: 400, cache_creation: 0, cache_read: 17, output: 5 });
        assert!(!reply.usage_estimated && (reply.cost_usd - 0.000612).abs() < 1e-9);
        let today = Ledger::open(&f.cache).today(usage::now(), Agent::Claude);
        assert_eq!((today.tokens(), today.calls, today.failed), (422, 1, 0));
        assert_eq!(today.features["names"].tokens, 422);
        assert_eq!(Ledger::open(&f.cache).load().claude.backoff, None);
        let text = fs::read_to_string(f.cache.join("ai-usage.json")).unwrap();
        assert!(!text.contains("Name these") && !text.contains("Be brief"), "prompt text is never saved");
        // Only the cache dir is written to.
        let names: Vec<String> = fs::read_dir(&f.cache).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(names.iter().all(|n| ["ai-work", "ai-usage.json", "ai-usage.lock"].contains(&n.as_str())), "{names:?}");
    }

    #[test]
    fn usage_missing_from_a_good_answer_is_estimated_and_flagged() {
        let f = Fake::new("no-usage", r#"printf '%s' '{"is_error":false,"result":"twelve chars"}'"#);
        let reply = f.runner().run(&on(), &Request::new("names", "sys!", "a".repeat(400))).unwrap();
        assert!(reply.usage_estimated);
        assert_eq!((reply.usage.input, reply.usage.output), (101, 3));
        assert_eq!(Ledger::open(&f.cache).today(usage::now(), Agent::Claude).tokens(), 104, "the limit still sees it");
    }

    #[test]
    fn a_tasks_check_is_outside_the_gate_runs_where_it_is_told_and_its_answer_is_the_structured_output() {
        let f = Fake::new(
            "task-check",
            r#"printf '%s' '{"type":"result","subtype":"success","is_error":false,"result":"","structured_output":{"done":true},"usage":{"input_tokens":10,"output_tokens":5}}'"#,
        );
        let folder = f.dir.join("worktree");
        std::fs::create_dir_all(&folder).unwrap();
        // The token features off, and a back-off running: a smart feature would be refused; the task's check is not.
        let off = Settings::default();
        let ledger = Ledger::open(&f.cache);
        ledger.note_failure(usage::now(), Agent::Claude, FailKind::Network).unwrap();
        let before = ledger.load().claude.backoff;
        assert!(before.is_some());
        let mut req = Request::new(crate::verdict::FEATURE, "sys", "prompt");
        req.model = Some("fable".into());
        req.tools = Tools::ReadOnly;
        req.cwd = Some(folder.clone());
        req.json_schema = Some(r#"{"type":"object"}"#.into());
        req.effort = "medium";
        let reply = f.runner().run(&off, &req).unwrap();
        assert_eq!(extract_json(&reply.text), Some(serde_json::json!({"done": true})));
        let argv = f.rec("argv");
        for want in ["--model\nfable\n", "--tools\nRead,Grep,Glob\n", "--json-schema\n{\"type\":\"object\"}\n", "--effort\nmedium\n"] {
            assert!(argv.contains(want), "{want:?}: {argv}");
        }
        assert_eq!(f.rec("cwd").trim(), folder.canonicalize().unwrap().to_string_lossy());
        assert!(!f.rec("env").contains("MAX_THINKING_TOKENS="), "thinking allowed above low effort");
        assert_eq!(ledger.load().claude.backoff, before, "it neither waits for a back-off nor moves it");
        let day = ledger.today(usage::now(), Agent::Claude);
        assert_eq!(day.features.get(crate::verdict::FEATURE).map(|x| x.tokens), Some(15), "its tokens are in the ledger");
        assert_eq!(feature_label(crate::verdict::FEATURE), "Task checks");
        assert_eq!(f.runner().run(&off, &Request::new("names", "s", "p")), Err(AiError::Refused(Refusal::MasterOff)));
    }

    // ---- calls on Codex ----

    /// A runner with Claude Code's stand-in `f` and Codex's stand-in `codex` (its folder holding the sample model list).
    fn with_codex(f: &Fake, codex: &crate::testkit::AiFake, name: &str) -> Runner {
        let root = crate::agents::tests::codex_with_models(name).root;
        Runner::new(&f.cache, Providers { claude: Some(f.script.clone()), codex: Some(codex.as_codex(&root)), off: Vec::new() })
    }

    #[test]
    fn a_smart_features_call_on_an_openai_model_runs_codex_exec_the_agreed_way_in_the_work_folder() {
        let f = Fake::ok("codex-names");
        let codex = crate::testkit::AiFake::codex("codex-names");
        codex.reply(1, "the answer");
        let mut s = on();
        s.ai_model = Some("gpt-6-luna".into());
        let reply = with_codex(&f, &codex, "codex-names").run(&s, &req()).unwrap();
        assert!(!f.ran(), "Claude Code's program is not run for an OpenAI model");
        assert_eq!((reply.text.as_str(), reply.model.as_str()), ("the answer", "gpt-6-luna"));
        assert_eq!(
            codex.argv(1),
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
                "developer_instructions=\"Be brief.\"",
                "--skip-git-repo-check",
                "-"
            ]
        );
        assert_eq!(codex.stdin(1), "Name these:\nfirst\nsecond", "the prompt arrives on stdin, whole");
        assert_eq!(fs::canonicalize(codex.cwd(1)).unwrap(), fs::canonicalize(f.cache.join("ai-work")).unwrap(), "the neutral folder");
        assert!(codex.env(1).lines().any(|l| l == "PLATOON_AI=1"));
        // Codex's own instructions are most of what a call costs: every token it reports counts, cached ones too.
        assert_eq!(reply.usage, Usage { input: 8_200, cache_creation: 0, cache_read: 12_800, output: 40 });
        assert_eq!((reply.cost_usd, reply.usage_estimated), (0.0, false));
        // Counted as Codex's, apart from Claude Code's.
        let day = Ledger::open(&f.cache).today(usage::now(), Agent::Codex);
        assert_eq!((day.tokens(), day.calls), (21_040, 1));
        assert_eq!(Ledger::open(&f.cache).today(usage::now(), Agent::Claude).tokens(), 0);
        // No schema file was left behind, and none was asked for.
        assert!(fs::read_dir(f.cache.join("ai-work")).unwrap().next().is_none(), "the work folder is left empty");
    }

    #[test]
    fn a_tasks_check_on_codex_runs_read_only_in_the_tasks_folder_with_its_schema_in_a_file_that_goes_after() {
        let f = Fake::ok("codex-check");
        let codex = crate::testkit::AiFake::codex("codex-check");
        codex.reply(1, r#"{"done":true,"boxes":[],"findings":[]}"#);
        let folder = f.dir.join("worktree");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join(".git"), "gitdir: /somewhere/.git/worktrees/task-1\n").unwrap();
        let runner = with_codex(&f, &codex, "codex-check");
        let (text, tokens) = crate::verdict::run(&runner, "gpt-5.6-terra", &folder, "the task".into(), Duration::from_secs(20)).unwrap();
        assert_eq!(text, r#"{"done":true,"boxes":[],"findings":[]}"#);
        assert_eq!(tokens, 21_040);
        let argv = codex.argv(1);
        let at = |flag: &str| argv.iter().position(|a| a == flag).map(|i| argv[i + 1].clone());
        assert_eq!((at("-s").as_deref(), at("-m").as_deref()), (Some("read-only"), Some("gpt-5.6-terra")), "{argv:?}");
        assert!(argv.contains(&"model_reasoning_effort=\"medium\"".to_string()), "{argv:?}");
        assert!(!argv.contains(&"--skip-git-repo-check".to_string()), "a task's folder is in git: {argv:?}");
        let schema = at("--output-schema").unwrap();
        assert!(schema.starts_with(&f.cache.join("ai-work").display().to_string()), "in platoon's work folder: {schema}");
        assert_eq!(codex.schema(1), crate::verdict::SCHEMA, "the schema, whole, while the call ran");
        assert!(!Path::new(&schema).exists(), "and gone after");
        assert!(codex.has_arg(1, &format!("developer_instructions={}", toml_quoted(crate::verdict::SYSTEM))), "{argv:?}");
        assert_eq!(fs::canonicalize(codex.cwd(1)).unwrap(), fs::canonicalize(&folder).unwrap(), "in the task's folder");
        assert_eq!(codex.stdin(1), "the task");
    }

    /// `text` quoted as the TOML string `-c` reads (what `codex::one_shot` gives `developer_instructions`).
    fn toml_quoted(text: &str) -> String {
        format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n"))
    }

    #[test]
    fn a_codex_call_that_goes_wrong_is_said_as_codexs_backs_off_and_its_schema_file_still_goes() {
        let f = Fake::ok("codex-fail");
        let codex = crate::testkit::AiFake::codex("codex-fail");
        let mut s = on();
        s.ai_model = Some("gpt-6-luna".into());
        let runner = with_codex(&f, &codex, "codex-fail");
        codex.out(1, r#"{"type":"turn.failed","error":{"message":"You've hit your usage limit."}}"#);
        codex.exit_code(1);
        let e = runner.run(&s, &req()).unwrap_err();
        assert_eq!(e, AiError::Failed { agent: Agent::Codex, kind: FailKind::RateLimited, detail: "You've hit your usage limit.".into() });
        assert_eq!(e.plain(), "Your Codex plan limit was reached.");
        let data = Ledger::open(&f.cache).load();
        assert!(data.codex.backoff.is_some() && data.claude.backoff.is_none(), "a failure backs off Codex's calls, not Claude's");
        // With a schema (a description search's ranking, say): the file goes whatever happened.
        let _ = fs::remove_file(f.cache.join("ai-usage.json"));
        let mut find = Request::new("find", "s", "p");
        find.json_schema = Some("{}".into());
        find.user_initiated = true;
        codex.out(2, "");
        codex.exit_code(0);
        let e = runner.run(&s, &find).unwrap_err();
        assert!(matches!(&e, AiError::Failed { agent: Agent::Codex, detail, .. } if detail == "no output"), "{e:?}");
        assert!(!codex.schema(2).is_empty(), "it had its schema");
        assert!(fs::read_dir(f.cache.join("ai-work")).unwrap().next().is_none(), "and nothing is left in the work folder");
        // A program that isn't there.
        let gone = Runner::new(
            &f.cache,
            Providers {
                claude: None,
                codex: Some(crate::agents::CodexAt { program: "/nonexistent/platoon-unit-test/codex".into(), root: f.dir.clone() }),
                off: Vec::new(),
            },
        );
        let _ = fs::remove_file(f.cache.join("ai-usage.json"));
        let mut s = on();
        s.ai_model = Some("gpt-6-luna".into());
        let e = gone.run(&s, &req()).unwrap_err();
        assert!(matches!(&e, AiError::Failed { detail, .. } if detail.contains("couldn't find")), "{e:?}");
    }

    // ---- limits and the ledger, per provider ----

    /// Claude Code (a stand-in) and Codex at `root` (its list of models copied there).
    fn both_at(root: &Path) -> Providers {
        fs::create_dir_all(root).unwrap();
        fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/src/testdata/codex/models_cache.json"), root.join("models_cache.json")).unwrap();
        Providers {
            claude: Some("/nonexistent/platoon-unit-test/claude".into()),
            codex: Some(crate::agents::CodexAt { program: "/nonexistent/platoon-unit-test/codex".into(), root: root.to_path_buf() }),
            off: Vec::new(),
        }
    }

    #[test]
    fn the_pause_follows_the_plan_of_the_program_the_call_would_use() {
        let f = Fake::ok("pause-agent");
        let p = both_at(&f.dir.join("codex"));
        let root = p.codex_root().unwrap().to_path_buf();
        let now = usage::now();
        let window =
            |pct: u64, minutes: u64| serde_json::json!({"used_percent": pct, "window_minutes": minutes, "resets_at": now + 86_400});
        // Claude's plan nearly used up, Codex's 30-day window hardly.
        plan_report(&f.cache, 95, None, now + 3600, now);
        crate::agents::codex::tests::limits_rollout(&root, "01a1517c-0000-7000-8000-0000000000c1", window(10, 43_200), Value::Null, 1);
        let mut s = on();
        assert!(matches!(gate(&s, "names", &f.cache, &p, now), Err(Refusal::PlanBusy { agent: Agent::Claude, pct: 95, .. })));
        // The smart features on an OpenAI model: Codex's plan decides, and it has room.
        s.ai_model = Some("gpt-6-luna".into());
        assert_eq!(gate(&s, "names", &f.cache, &p, now), Ok(()));
        // The notes on their own model (Sonnet): Claude's plan decides for them.
        s.set_feature(crate::notesrun::FEATURE, true);
        assert!(matches!(gate(&s, crate::notesrun::FEATURE, &f.cache, &p, now), Err(Refusal::PlanBusy { agent: Agent::Claude, .. })));
        // Codex's 30-day window above the pause point, said as such.
        crate::agents::codex::tests::limits_rollout(&root, "01a1517c-0000-7000-8000-0000000000c1", window(85, 43_200), Value::Null, 0);
        let r = gate(&s, "names", &f.cache, &p, now).unwrap_err();
        assert_eq!(r, Refusal::PlanBusy { agent: Agent::Codex, pct: 85, limit: 80, window: "30-day limit".into() });
        assert_eq!(
            r.plain(),
            "Your Codex plan is 85% used (30-day limit), above the pause point of 80%. Smart features wait until it drops."
        );
        assert_eq!(r.short(), "smart features paused · your Codex plan is 85% used (30-day limit)");
        // A window that has started over no longer counts.
        assert_eq!(gate(&s, "names", &f.cache, &p, now + 2 * 86_400), Ok(()));
        assert_eq!(plan_percent(Agent::Codex, &f.cache, &p, now), Some((85, "30-day limit".into())));
        assert_eq!(plan_percent(Agent::Codex, &f.cache, &claude_here(), now), None, "no Codex installed: nothing said");
    }

    #[test]
    fn the_ledger_counts_each_program_apart_and_the_daily_limit_and_back_off_are_each_its_own() {
        let f = Fake::ok("ledger-agent");
        let p = both_at(&f.dir.join("codex"));
        let now = usage::now();
        let mut s = on();
        s.ai_daily_tokens = 20_000;
        let l = Ledger::open(&f.cache);
        l.record_call(now, Agent::Codex, "names", &Usage { input: 25_000, ..Usage::default() }, 0.0, true).unwrap();
        // Codex's tokens are over the limit: its calls wait; Claude Code's have their own count.
        assert_eq!(gate(&s, "names", &f.cache, &p, now), Ok(()));
        s.ai_model = Some("gpt-6-luna".into());
        assert_eq!(gate(&s, "names", &f.cache, &p, now), Err(Refusal::OverBudget { used: 25_000, cap: 20_000 }));
        // A back-off is the program's that failed.
        l.note_failure(now, Agent::Claude, FailKind::NotSignedIn).unwrap();
        s.ai_daily_tokens = 100_000;
        assert_eq!(gate(&s, "names", &f.cache, &p, now), Ok(()), "Claude Code signed out doesn't stop Codex");
        s.ai_model = None;
        let r = gate(&s, "names", &f.cache, &p, now).unwrap_err();
        assert!(matches!(r, Refusal::BackedOff { agent: Agent::Claude, kind: FailKind::NotSignedIn, .. }));
        assert!(r.plain().contains("Claude isn't signed in"), "{}", r.plain());
        // On disk: Claude Code's numbers where they always were, Codex's under "codex", numbers only.
        let v: Value = serde_json::from_str(&fs::read_to_string(f.cache.join("ai-usage.json")).unwrap()).unwrap();
        assert!(v["days"].as_object().unwrap().is_empty() && v["backoff"]["kind"] == "not_signed_in", "{v}");
        let day = local_day_of(now);
        assert_eq!(v["codex"]["days"][&day]["usage"]["input"], 25_000, "{v}");
        assert!(v["codex"].get("backoff").is_none());
        // A file from before there were two programs reads as Claude Code's.
        fs::write(f.cache.join("ai-usage.json"), format!(r#"{{"version":1,"days":{{"{day}":{{"usage":{{"input":7}},"calls":1}}}}}}"#))
            .unwrap();
        assert_eq!((l.today(now, Agent::Claude).tokens(), l.today(now, Agent::Codex).tokens()), (7, 0));
    }

    fn local_day_of(now: u64) -> String {
        crate::ledger::local_day(now)
    }

    #[test]
    fn tokens_says_each_programs_use_plan_and_back_off_when_both_are_installed() {
        let f = Fake::ok("cli-both");
        let p = both_at(&f.dir.join("codex"));
        let root = p.codex_root().unwrap().to_path_buf();
        let config = f.dir.join("config");
        let now = usage::now();
        crate::agents::codex::tests::limits_rollout(
            &root,
            "01a1517c-0000-7000-8000-0000000000d1",
            serde_json::json!({"used_percent": 12, "window_minutes": 43_200, "resets_at": now + 86_400}),
            Value::Null,
            1,
        );
        let l = Ledger::open(&f.cache);
        l.record_call(now, Agent::Claude, "names", &Usage { input: 844, ..Usage::default() }, 0.0, true).unwrap();
        l.record_call(now, Agent::Codex, "project_notes", &Usage { input: 21_040, ..Usage::default() }, 0.0, true).unwrap();
        l.note_failure(now, Agent::Codex, FailKind::NotSignedIn).unwrap();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(tokens_cli(&[], &f.cache, &config, &p, &mut out, &mut err), 0);
        let out = String::from_utf8(out).unwrap();
        for want in [
            "Used today on Claude Code: 844 of 500,000 tokens\n  Smart names and summaries: 844 tokens in 1 call\n",
            "Plan usage of Claude Code: no figure reported yet",
            "Used today on Codex: 21,040 of 500,000 tokens\n  Project notes: 21,040 tokens in 1 call\n",
            "Plan usage of Codex: 12% (30-day limit); pause above 80%",
            "Paused after a failure (Codex isn't signed in)",
        ] {
            assert!(out.contains(want), "{want:?}:\n{out}");
        }
        let notice = backoff_notice(&on(), &f.cache, now).unwrap();
        assert_eq!(notice.1, "Smart features paused: Codex isn't signed in");
        assert!(notice.0.starts_with("codex:"), "{}", notice.0);
    }

    #[test]
    fn a_refused_call_never_starts_anything() {
        let f = Fake::ok("refused");
        let r = f.runner().run(&Settings::default(), &req());
        assert_eq!(r, Err(AiError::Refused(Refusal::MasterOff)));
        let mut s = on();
        s.set_feature("names", false);
        assert_eq!(f.runner().run(&s, &req()), Err(AiError::Refused(Refusal::FeatureOff("Smart names and summaries"))));
        assert!(!f.ran(), "the fake was never started");
        assert!(!f.cache.exists(), "nothing was created, not even the work folder");
        // Over the limit
        let mut s = on();
        s.ai_daily_tokens = 20_000;
        Ledger::open(&f.cache)
            .record_call(usage::now(), Agent::Claude, "names", &Usage { input: 30_000, ..Usage::default() }, 0.0, true)
            .unwrap();
        assert!(matches!(f.runner().run(&s, &req()), Err(AiError::Refused(Refusal::OverBudget { .. }))));
        assert!(!f.ran());
    }

    #[test]
    fn a_model_of_a_program_switched_off_is_refused_saying_so_and_nothing_starts() {
        let f = Fake::ok("model-off");
        let codex = crate::agents::tests::codex_with_models("ai-model-off");
        let off = Providers { claude: Some(f.script.clone()), codex: Some(codex), off: Vec::new() }.switched(&[Agent::Codex]);
        let runner = Runner::new(&f.cache, off);
        let mut s = on();
        s.ai_model = Some("gpt-6-luna".into());
        let Err(AiError::Refused(r)) = runner.run(&s, &req()) else { panic!("refused") };
        assert_eq!(
            r.plain(),
            "GPT-6-Luna runs on Codex, which is switched off in platoon's settings. Pick another for \"Model for smart features\" in \
             platoon's settings, or switch Codex back on there."
        );
        assert_eq!(r.short(), "smart features stopped · Codex is switched off");
        // A call's own model (a task's check): said, without a setting to change.
        let mut check = Request::new(crate::verdict::FEATURE, "s", "p");
        check.model = Some("gpt-5.6-terra".into());
        let Err(AiError::Refused(r)) = runner.run(&Settings::default(), &check) else { panic!("refused") };
        assert_eq!(r.plain(), "GPT-5.6-Terra runs on Codex, which is switched off in platoon's settings.");
        assert!(!f.ran(), "nothing was started");
    }

    #[test]
    fn a_model_nothing_here_runs_is_refused_plainly_never_swapped_and_nothing_starts() {
        let f = Fake::ok("no-model");
        // The smart features' model, saved when Codex was there: said, with the setting to change.
        let mut s = on();
        s.ai_model = Some("gpt-6-luna".into());
        let Err(AiError::Refused(r)) = f.runner().run(&s, &req()) else { panic!("refused") };
        assert_eq!(
            r.plain(),
            "GPT-6-Luna needs Codex, which isn't installed on this computer. Pick another for \"Model for smart features\" in platoon's settings."
        );
        assert!(!r.is_switch());
        // The notes' own model setting is named for a notes call.
        let mut s = on();
        s.set_feature(crate::notesrun::FEATURE, true);
        s.notes_model = Some("gpt-6-luna".into());
        let notes = Request::new(crate::notesrun::FEATURE, "s", "p");
        let Err(AiError::Refused(r)) = f.runner().run(&s, &notes) else { panic!("refused") };
        assert!(r.plain().ends_with("Pick another for \"Model for project notes\" in platoon's settings."), "{}", r.plain());
        // A call's own model (a task's check, from its squad) is refused too, outside the gate as it is.
        let mut check = Request::new(crate::verdict::FEATURE, "s", "p");
        check.model = Some("gpt-6-luna".into());
        let Err(AiError::Refused(r)) = f.runner().run(&Settings::default(), &check) else { panic!("refused") };
        assert_eq!(r.plain(), "GPT-6-Luna needs Codex, which isn't installed on this computer.");
        // With no AI program at all there is no model to ask.
        let none = Runner::new(&f.cache, Providers::default());
        let Err(AiError::Refused(r)) = none.run(&on(), &req()) else { panic!("refused") };
        assert!(r.plain().starts_with("Neither Claude Code nor Codex is installed"), "{}", r.plain());
        assert!(!f.ran(), "nothing was started");
        assert!(Ledger::open(&f.cache).load().claude.backoff.is_none(), "not a failure: nothing backs off");
    }

    #[test]
    fn each_failure_class_gets_its_plain_message_records_usage_when_reported_and_backs_off() {
        let cases: [(&str, String, i32, FailKind, &str); 5] = [
            (
                "signed-out",
                r#"{"is_error":true,"result":"Not logged in · Please run /login","usage":{"input_tokens":9,"output_tokens":1}}"#.into(),
                1,
                FailKind::NotSignedIn,
                "Claude isn't signed in.",
            ),
            (
                "limit",
                r#"{"is_error":true,"result":"Claude AI usage limit reached|1760000000"}"#.into(),
                1,
                FailKind::RateLimited,
                "Your Claude plan limit was reached.",
            ),
            (
                "network",
                r#"{"is_error":true,"result":"Unable to connect to API (ENOTFOUND)"}"#.into(),
                1,
                FailKind::Network,
                "Claude can't be reached (is the internet down?).",
            ),
            ("other", r#"{"is_error":true,"result":"Something else went wrong"}"#.into(), 0, FailKind::Other, "Claude returned an error."),
            ("garbage", "this is not json".into(), 0, FailKind::Other, "Claude returned an error."),
        ];
        for (name, body, code, kind, message) in cases {
            let f = Fake::new(name, &format!("cat <<'EOF'\n{body}\nEOF\nexit {code}"));
            let e = f.runner().run(&on(), &req()).unwrap_err();
            assert!(matches!(&e, AiError::Failed { kind: k, .. } if *k == kind), "{name}: {e:?}");
            assert_eq!(e.plain(), message, "{name}");
            let data = Ledger::open(&f.cache).load();
            let b = data.claude.backoff.unwrap_or_else(|| panic!("{name}: no back-off"));
            assert_eq!((b.kind, b.failures), (kind, 1), "{name}");
            let day = data.claude.days.into_values().next();
            if name == "signed-out" {
                let day = day.expect("usage reported by a failed call is recorded");
                assert_eq!((day.tokens(), day.calls, day.failed), (10, 1, 1));
            } else {
                assert!(day.is_none(), "{name}: no usage reported, none recorded");
            }
        }
    }

    #[test]
    fn empty_output_a_nonzero_exit_and_a_missing_program_are_failures_too() {
        let f = Fake::new("empty", "exit 0");
        assert!(matches!(f.runner().run(&on(), &req()), Err(AiError::Failed { kind: FailKind::Other, .. })));
        let f = Fake::new("exit3", &format!("printf '%s' '{}'\nexit 3", ok_json("ok", 1, 0, 0, 1)));
        assert!(
            matches!(f.runner().run(&on(), &req()), Err(AiError::Failed { .. })),
            "a non-zero exit is a failure even with a good-looking answer"
        );
        let f = Fake::ok("missing");
        let r = Runner::with_program(&f.cache, "/nonexistent/claude-fake").run(&on(), &req());
        match r {
            Err(AiError::Failed { kind: FailKind::Other, detail, .. }) => assert!(detail.contains("couldn't find"), "{detail}"),
            other => panic!("{other:?}"),
        }
        assert!(Ledger::open(&f.cache).load().claude.backoff.is_some(), "a program that can't be found backs off too");
    }

    #[test]
    fn after_a_failure_calls_are_refused_without_starting_anything_until_the_wait_is_over() {
        let f = Fake::new("refuse-after", r#"printf '%s' '{"is_error":true,"result":"boom"}'"#);
        assert!(matches!(f.runner().run(&on(), &req()), Err(AiError::Failed { .. })));
        let _ = fs::remove_file(f.dir.join("rec.argv"));
        let again = f.runner().run(&on(), &req());
        assert!(
            matches!(again, Err(AiError::Refused(Refusal::BackedOff { agent: Agent::Claude, kind: FailKind::Other, .. }))),
            "{again:?}"
        );
        assert!(!f.ran(), "refused before spawning");
        // A wait that is over lets a call through, and a success clears the whole thing.
        let l = Ledger::open(&f.cache);
        l.update(|d| d.claude.backoff.as_mut().unwrap().until = 1).unwrap();
        fs::write(&f.script, format!("#!/bin/sh\ncat >/dev/null\nprintf '%s' '{}'\n", ok_json("ok", 1, 0, 0, 1))).unwrap();
        assert_eq!(f.runner().run(&on(), &req()).unwrap().text, "ok");
        assert_eq!(l.load().claude.backoff, None, "a success resets the growth");
    }

    #[test]
    fn a_call_that_takes_too_long_is_killed_with_everything_it_started_and_backs_off() {
        let f = Fake::new("timeout", HANG);
        let mut r = req();
        r.timeout = Duration::from_millis(300);
        let began = Instant::now();
        let e = f.runner().run(&on(), &r).unwrap_err();
        assert!(matches!(e, AiError::Failed { kind: FailKind::TimedOut, .. }), "{e:?}");
        assert!(began.elapsed() < Duration::from_secs(10), "gave up in {:?}", began.elapsed());
        assert_eq!(e.plain(), "Claude took too long to answer.");
        wait_dead(&f.dir.join("pid"));
        wait_dead(&f.dir.join("sleep"));
        assert_eq!(Ledger::open(&f.cache).load().claude.backoff.unwrap().kind, FailKind::TimedOut);
    }

    #[test]
    fn cancelling_stops_the_call_kills_only_its_own_process_and_is_not_a_failure() {
        let f = Fake::new("cancel", HANG);
        // A bystander that must survive: not ours, not in our process group.
        let mut bystander = Command::new("sleep").arg("300").spawn().unwrap();
        let r = req();
        let cancel = r.cancel.clone();
        let (runner, settings) = (f.runner(), on());
        let handle = thread::spawn(move || runner.run(&settings, &r));
        let since = std::time::Instant::now();
        while since.elapsed() < PATIENCE {
            if f.dir.join("sleep").exists() && !fs::read_to_string(f.dir.join("sleep")).unwrap().trim().is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        assert!(f.dir.join("pid").exists(), "the fake started");
        cancel.cancel();
        let result = handle.join().unwrap();
        assert_eq!(result, Err(AiError::Cancelled));
        wait_dead(&f.dir.join("pid"));
        wait_dead(&f.dir.join("sleep"));
        assert!(crate::procs::pid_alive(bystander.id()), "a process that isn't ours is left alone");
        let _ = bystander.kill();
        let _ = bystander.wait();
        assert_eq!(Ledger::open(&f.cache).load().claude.backoff, None, "cancelling is not a failure");
    }

    #[test]
    fn cancelling_before_the_call_starts_runs_nothing() {
        let f = Fake::ok("early-cancel");
        let r = req();
        r.cancel.cancel();
        assert_eq!(f.runner().run(&on(), &r), Err(AiError::Cancelled));
        assert!(!f.ran());
    }

    #[test]
    fn the_switchboard_cancels_only_the_calls_whose_feature_was_switched_off() {
        let sb = Switchboard::new();
        let (a, b, t) = (sb.token("names"), sb.token("find"), sb.token(TEST_FEATURE));
        sb.apply(&on());
        assert!(!a.is_cancelled() && !b.is_cancelled() && !t.is_cancelled(), "everything still allowed");
        let mut s = on();
        s.set_feature("names", false);
        sb.apply(&s);
        assert!(a.is_cancelled() && !b.is_cancelled() && !t.is_cancelled());
        assert!(!sb.token("names").is_cancelled(), "a call started afterwards has a fresh flag");
        s.tokens_enabled = false;
        sb.apply(&s);
        assert!(b.is_cancelled() && t.is_cancelled(), "master off cancels everything");
        let d = sb.token("names");
        sb.cancel("names");
        assert!(d.is_cancelled());
    }

    #[test]
    #[should_panic(expected = "real `claude`")]
    fn a_unit_test_that_would_reach_the_real_claude_fails_loudly() {
        let cache = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-ai-guard-{}", std::process::id())));
        let _ = Runner::with_program(&cache, "claude").run(&on(), &req());
    }

    // ---- `platoon tokens` ----

    fn cli(args: &[&str], f: &Fake, config: &Path) -> (i32, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let providers = Providers { claude: Some(f.script.clone()), codex: None, off: Vec::new() };
        let code = tokens_cli(&args, &f.cache, config, &providers, &mut out, &mut err);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    #[test]
    fn tokens_status_reads_and_writes_nothing() {
        let f = Fake::ok("cli-status");
        let config = f.dir.join("config");
        let (code, out, _) = cli(&[], &f, &config);
        assert_eq!(code, 0);
        assert!(
            out.contains("Features that use tokens: off")
                && out.contains("Used today: 0 of 500,000 tokens")
                && out.contains("no figure reported"),
            "{out}"
        );
        assert!(!config.exists() && !f.cache.exists());
        assert_eq!(cli(&["nonsense"], &f, &config).0, 2);
        assert_eq!(cli(&["test", "extra"], &f, &config).0, 2);
    }
}
