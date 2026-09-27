//! The Squads tab (`3`). Its left pane switches (`←` `→`, or a click on its title) between two lists:
//! - **Squads**: built in, changed by the user, or the user's own (and an unsaved draft), the selected one on the
//!   right in two sections: About (name, what it can do) and the Workflow, drawn as a rail (`ui/squads_view.rs`).
//!   Each step is two rows: its agent, with the agent's model and switches (`enter` picks another or makes a new one,
//!   `g` goes to it), and what it does. The leader is not here: it is the same for every squad, in the settings.
//! - **Agents**: every agent, shared by the squads (built in, changed by the user, the user's own, an unsaved draft,
//!   and "+ New agent"), the selected one on the right: name, model, the three switches, its skills and MCP servers
//!   (picked from what is installed, or typed), system prompt, and the squads it is used in (`enter` goes to one). A
//!   change to an agent is a change in every squad that uses it.
//!
//! Everything is written at once as it is changed (a draft is changed in memory). `n` asks what to describe (a new
//! squad or a new agent) for Claude to draft (one `claude -p` on Sonnet, off the UI thread), `c` copies, `d` deletes
//! the user's own (an agent a squad uses is refused, naming the squads) or puts a built-in back as platoon has it. The
//! definitions and the rules are in `squadfile.rs` and `agentfile.rs`.

use super::*;
use crate::agentfile::{self, Agent};
use crate::ai::{AiError, Reply, Request};
use crate::instructions;
use crate::squadfile::{self, Back, Kind, Listed, Squad, Step as WStep};
use std::sync::mpsc::TryRecvError;

/// How long drafting a squad or an agent may take before it is given up on.
const DRAFT_TIMEOUT: Duration = Duration::from_secs(180);

/// The value of the "+ New agent" entry of the agent picker (no agent id can look like it).
pub const NEW_AGENT: &str = "+new";
/// The value of the "Type the name of …" entry of the skills and MCP server pickers (no name can look like it).
pub const TYPE_NAME: &str = "+type";

/// Which of the tab's two panes has the keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SqFocus {
    List,
    Detail,
}

/// Which list the left pane shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SqView {
    Squads,
    Agents,
}

/// A row of the selected squad's details: the things that can be changed, in the order they are shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DRow {
    Name,
    CanDo,
    /// Step `i`'s agent (with its model and switches, which are changed on the Agents list): the step's first line
    /// on the rail.
    StepAgent(usize),
    /// What step `i` does: the line under it.
    Step(usize),
    AddStep,
}

impl DRow {
    /// The step it belongs to, if any.
    pub fn step(self) -> Option<usize> {
        match self {
            DRow::Step(i) | DRow::StepAgent(i) => Some(i),
            _ => None,
        }
    }
}

/// The rows of a squad's details (a plugin's squad, read only, has no "+ Add a step").
pub fn detail_rows(s: &Squad) -> Vec<DRow> {
    let mut out = vec![DRow::Name, DRow::CanDo];
    for i in 0..s.workflow.len() {
        out.extend([DRow::StepAgent(i), DRow::Step(i)]);
    }
    if !crate::squadplugin::is_plugin(&s.id) {
        out.push(DRow::AddStep);
    }
    out
}

/// A row of the selected agent's details.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ARow {
    Name,
    Model,
    Files,
    Commands,
    Web,
    Skills,
    Mcp,
    Prompt,
    /// The `k`th squad it is used in (`App::agent_users`).
    UsedIn(usize),
}

/// The rows of an agent's details, when `users` squads use it.
pub fn agent_rows(users: usize) -> Vec<ARow> {
    let mut out = vec![ARow::Name, ARow::Model, ARow::Files, ARow::Commands, ARow::Web, ARow::Skills, ARow::Mcp, ARow::Prompt];
    out.extend((0..users).map(ARow::UsedIn));
    out
}

/// A squad that uses an agent: which (None: the unsaved draft), its name, and the steps (1-based).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    pub squad: Option<String>,
    pub name: String,
    pub steps: Vec<usize>,
}

/// What a short line typed in the name box is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SquadName {
    /// The squad's name.
    Name,
    /// The name of a copy of the selected squad.
    Copy,
    /// The selected agent's name.
    AgentName,
    /// The name of a copy of the selected agent.
    AgentCopy,
    /// A new, blank agent: from "+ New agent" (None), or put on step `i` of the selected squad.
    NewAgent(Option<usize>),
    /// A new, blank agent that does a new step, which does this.
    NewAgentStep(String),
    /// A skill the selected agent has, typed (one not installed here: a skill of a ticket's folder, say).
    AgentSkill,
    /// An MCP server the selected agent may use, typed.
    AgentMcp,
}

/// What the text box is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextFor {
    /// The description of a new squad, for Claude to draft.
    Describe,
    /// The description of a new agent, for Claude to draft.
    DescribeAgent,
    CanDo,
    /// What step `i` does.
    StepDoes(usize),
    /// What a new step, done by this agent (`NEW_AGENT`: one made next), does.
    NewStep(String),
}

/// What the small list open over the tab is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChoiceFor {
    /// The selected agent's model.
    AgentModel,
    /// The selected agent's skills: each one ticked or not, and typing one (the list stays open).
    AgentSkills,
    /// ... its MCP servers.
    AgentMcp,
    /// Which agent does step `i`.
    StepAgent(usize),
    /// Which agent does a new step.
    NewStepAgent,
    /// Where step `i` may send the work back to (or not at all).
    BackTo(usize),
    /// How many times step `i` may send the work back to step `to`.
    BackTimes(usize, usize),
    /// `n`: a new squad or a new agent, to describe.
    Describe,
}

/// The small list: its entries (what it says, and the value behind it) and the picked one.
pub struct Choice {
    pub purpose: ChoiceFor,
    pub title: String,
    pub options: Vec<(String, String)>,
    pub selected: usize,
}

/// A paragraph being typed: it wraps at the box's width. Enter keeps it.
pub struct TextArea {
    pub text: String,
    /// Where the next character goes, as a character index.
    pub cursor: usize,
    /// The columns the text may take and the rows that show, as the last frame drew them.
    pub view: Cell<(usize, usize)>,
    pub top: Cell<usize>,
    pub max: usize,
}

impl TextArea {
    fn new(text: &str, max: usize) -> Self {
        TextArea { text: text.to_string(), cursor: text.chars().count(), view: Cell::new((60, 4)), top: Cell::new(0), max }
    }

    fn byte_at(&self, chars: usize) -> usize {
        self.text.char_indices().nth(chars).map_or(self.text.len(), |(i, _)| i)
    }

    /// Put text in at the cursor (line breaks become spaces: it is one paragraph). False when it would be too long.
    fn insert(&mut self, text: &str) -> bool {
        let text: String = crate::textbox::clean_paste(text).chars().map(|c| if c == '\n' || c == '\t' { ' ' } else { c }).collect();
        if self.text.chars().count() + text.chars().count() > self.max {
            return false;
        }
        let at = self.byte_at(self.cursor);
        self.text.insert_str(at, &text);
        self.cursor += text.chars().count();
        true
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let (at, to) = (self.byte_at(self.cursor - 1), self.byte_at(self.cursor));
        self.text.replace_range(at..to, "");
        self.cursor -= 1;
    }

    fn delete(&mut self) {
        if self.cursor >= self.text.chars().count() {
            return;
        }
        let (at, to) = (self.byte_at(self.cursor), self.byte_at(self.cursor + 1));
        self.text.replace_range(at..to, "");
    }

    fn step(&mut self, delta: i64) {
        let len = self.text.chars().count() as i64;
        self.cursor = (self.cursor as i64 + delta).clamp(0, len) as usize;
    }

    /// Up or down a row on screen, keeping the column.
    fn row(&mut self, delta: i64) {
        let chars: Vec<char> = self.text.chars().collect();
        let rows = crate::textbox::layout(&chars, self.view.get().0.max(1));
        let (row, col) = crate::textbox::locate(&rows, &chars, self.cursor);
        let to = (row as i64 + delta).clamp(0, rows.len().saturating_sub(1) as i64) as usize;
        self.cursor = crate::textbox::index_at(&rows, &chars, to, col);
    }
}

/// Which drafting call is out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drafting {
    Squad,
    Agent,
}

/// Everything the Squads tab keeps.
pub struct SquadTab {
    /// Which list the left pane shows.
    pub view: SqView,
    /// The squads, as their files and platoon say (`squadfile::load_all`), re-read when a file changes.
    pub list: Vec<Listed>,
    stamp: Vec<(String, Option<(std::time::SystemTime, u64)>)>,
    /// A drafted squad not saved yet: listed last.
    pub draft: Option<Squad>,
    /// The agents the drafted squad brings: saved with it.
    pub draft_agents: Vec<Agent>,
    /// The selected squad: an index into `list`, or `list.len()` for the draft.
    pub selected: usize,
    pub focus: SqFocus,
    /// The selected row of the squad's details (an index into `detail_rows`).
    pub row: usize,
    /// The first line of the details in view, kept between frames so they only scroll when they have to.
    pub detail_top: Cell<usize>,
    /// The agents (`agentfile::load_all`), re-read when a file changes.
    pub agents: Vec<agentfile::Listed>,
    agent_stamp: Vec<(String, Option<(std::time::SystemTime, u64)>)>,
    /// A drafted agent not saved yet: listed after the others.
    pub agent_draft: Option<Agent>,
    /// The selected agent: an index into `agents`, then the draft, then "+ New agent".
    pub agent_selected: usize,
    /// The selected row of the agent's details (an index into `agent_rows`).
    pub agent_row: usize,
    pub agent_top: Cell<usize>,
    pub agent_list_top: Cell<usize>,
    pub choice: Choice,
    pub text: TextArea,
    pub text_for: TextFor,
    /// The drafting call, while it is out.
    drafting: Option<(Drafting, Receiver<Result<Reply, AiError>>)>,
    /// The last descriptions the user gave, offered again by the next `n`.
    pub last_description: String,
    pub last_agent_description: String,
    /// Where Claude keeps its skills (its config folder) and its user config file (the MCP servers).
    pub claude_root: PathBuf,
    pub claude_json: PathBuf,
    /// What is installed, as last read (and when): read again when it is older than a few seconds.
    installed: RefCell<Option<(Instant, agentfile::Installed)>>,
}

impl SquadTab {
    pub fn new(config: &Path) -> Self {
        SquadTab {
            view: SqView::Squads,
            list: squadfile::load_all(config),
            stamp: squadfile::stamp(config),
            draft: None,
            draft_agents: Vec::new(),
            selected: 0,
            focus: SqFocus::List,
            row: 0,
            detail_top: Cell::new(0),
            agents: agentfile::load_all(config),
            agent_stamp: agentfile::stamp(config),
            agent_draft: None,
            agent_selected: 0,
            agent_row: 0,
            agent_top: Cell::new(0),
            agent_list_top: Cell::new(0),
            choice: Choice { purpose: ChoiceFor::Describe, title: String::new(), options: Vec::new(), selected: 0 },
            text: TextArea::new("", 0),
            text_for: TextFor::Describe,
            drafting: None,
            last_description: String::new(),
            last_agent_description: String::new(),
            claude_root: crate::claude::claude_root(),
            claude_json: crate::claude::claude_json_path(),
            installed: RefCell::new(None),
        }
    }

    /// The skills and MCP servers installed here, read again when the last look is more than a few seconds old (the
    /// user may install one while platoon is open). Only names are read.
    pub fn installed(&self) -> agentfile::Installed {
        let mut cached = self.installed.borrow_mut();
        if let Some((at, i)) = cached.as_ref().filter(|(at, _)| at.elapsed() < Duration::from_secs(5)) {
            let _ = at;
            return i.clone();
        }
        let now = agentfile::Installed::read(&self.claude_root, &self.claude_json);
        *cached = Some((Instant::now(), now.clone()));
        now
    }

    /// Forget what was read, so the next look reads it again.
    pub fn reread_installed(&self) {
        *self.installed.borrow_mut() = None;
    }

    /// How many squads the list has (the draft counts).
    pub fn len(&self) -> usize {
        self.list.len() + usize::from(self.draft.is_some())
    }

    pub fn draft_selected(&self) -> bool {
        self.draft.is_some() && self.selected == self.list.len()
    }

    /// The selected squad's definition, if it can be read.
    pub fn squad(&self) -> Option<&Squad> {
        if self.draft_selected() {
            return self.draft.as_ref();
        }
        self.list.get(self.selected).and_then(|l| l.squad.as_ref())
    }

    /// The selected entry of the list (None for the draft).
    pub fn listed(&self) -> Option<&Listed> {
        if self.draft_selected() { None } else { self.list.get(self.selected) }
    }

    pub fn drafting(&self) -> Option<Drafting> {
        self.drafting.as_ref().map(|(k, _)| *k)
    }

    /// The selected row of the squad's details.
    pub fn drow(&self) -> Option<DRow> {
        self.squad().and_then(|s| detail_rows(s).get(self.row).copied())
    }

    /// How many entries the agents list has: the agents, the draft, and "+ New agent".
    pub fn agent_len(&self) -> usize {
        self.agents.len() + usize::from(self.agent_draft.is_some()) + 1
    }

    pub fn agent_draft_selected(&self) -> bool {
        self.agent_draft.is_some() && self.agent_selected == self.agents.len()
    }

    /// "+ New agent" is selected.
    pub fn new_agent_selected(&self) -> bool {
        self.agent_selected + 1 == self.agent_len()
    }

    /// The selected agent, if it can be read.
    pub fn agent(&self) -> Option<&Agent> {
        if self.agent_draft_selected() {
            return self.agent_draft.as_ref();
        }
        self.agents.get(self.agent_selected).and_then(|l| l.agent.as_ref())
    }

    /// The selected entry of the agents list (None for the draft and "+ New agent").
    pub fn agent_listed(&self) -> Option<&agentfile::Listed> {
        if self.agent_draft_selected() { None } else { self.agents.get(self.agent_selected) }
    }

    /// Every agent that can be read, as the files say now: what the squads' steps name.
    pub fn library(&self) -> Vec<Agent> {
        self.agents.iter().filter_map(|l| l.agent.clone()).collect()
    }

    /// The agents the selected squad's steps may name: every agent, and the draft's new ones when it is the draft.
    pub fn known_agents(&self) -> Vec<Agent> {
        let mut out = self.library();
        if self.draft_selected() {
            out.extend(self.draft_agents.iter().cloned());
        }
        out
    }

    /// Is this agent one the selected squad's draft brings (not saved yet)?
    pub fn is_draft_agent(&self, id: &str) -> bool {
        self.draft_selected() && self.draft_agents.iter().any(|a| a.id == id)
    }
}

/// The value of a model the user picked.
fn model_options() -> Vec<(String, String)> {
    squadfile::MODELS.iter().map(|(id, label)| (label.to_string(), id.to_string())).collect()
}

fn times_label(n: u32) -> String {
    match n {
        1 => "once".into(),
        2 => "twice".into(),
        n => format!("{n} times"),
    }
}

/// How a step's send-back reads on the tab: "may send it back to step 2, at most twice".
pub fn back_words(b: &Back) -> String {
    format!("may send it back to step {}, at most {}", b.to + 1, times_label(b.times))
}

/// How an agent reads in a list: "planner · Opus · runs commands".
pub fn agent_line(a: &Agent) -> String {
    format!("{} · {} · {}", a.name, squadfile::model_label(&a.model), a.switch_words())
}

/// Names in words: "Coding", "Coding and Docs", "Coding, Docs and Research".
fn and_words(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

impl App {
    // ---- the lists --------------------------------------------------------------------------------------------

    /// Every squad (built-ins first, then the user's own), as the files say now.
    pub fn squad_list(&self) -> &[Listed] {
        &self.sqt.list
    }

    /// Every agent that can be read, as the files say now.
    pub fn agent_library(&self) -> Vec<Agent> {
        self.sqt.library()
    }

    /// A squad's name, by its id (the id itself when there is no such squad now).
    pub fn squad_name(&self, id: &str) -> String {
        self.sqt.list.iter().find(|l| l.id == id).map_or_else(|| id.to_string(), |l| l.name().to_string())
    }

    /// The squads that use agent `id` (the saved ones, then the draft), with the steps that name it.
    pub fn agent_users(&self, id: &str) -> Vec<User> {
        let mut out: Vec<User> = self
            .sqt
            .list
            .iter()
            .filter_map(|l| l.squad.as_ref())
            .filter_map(|s| Some(s.steps_of(id)).filter(|n| !n.is_empty()).map(|steps| User { squad: Some(s.id.clone()), name: s.name.clone(), steps }))
            .collect();
        if let Some(d) = self.sqt.draft.as_ref() {
            let steps = d.steps_of(id);
            if !steps.is_empty() {
                out.push(User { squad: None, name: format!("{} (draft)", d.name), steps });
            }
        }
        out
    }

    /// " in every squad that uses it (Coding and Docs)", or "" when none does: said after a change to an agent.
    fn everywhere(&self, id: &str) -> String {
        let names: Vec<String> = self.agent_users(id).into_iter().map(|u| u.name).collect();
        if names.is_empty() { String::new() } else { format!(", in every squad that uses it ({})", and_words(&names)) }
    }

    /// The rows of the selected agent's details.
    pub fn agent_detail_rows(&self) -> Vec<ARow> {
        match self.sqt.agent() {
            Some(a) => agent_rows(self.agent_users(&a.id).len()),
            None => Vec::new(),
        }
    }

    /// The selected row of the agent's details.
    pub fn arow(&self) -> Option<ARow> {
        self.agent_detail_rows().get(self.sqt.agent_row).copied()
    }

    /// Read the squads and the agents again when their files changed (one listing of each folder when nothing did).
    pub(super) fn poll_squad_files(&mut self) {
        if squadfile::stamp(&self.config) != self.sqt.stamp {
            self.reload_squads(None);
        }
        if agentfile::stamp(&self.config) != self.sqt.agent_stamp {
            self.reload_agents(None);
        }
    }

    /// Read the squads again, keeping the selection on the same squad (or on `select`, an id).
    fn reload_squads(&mut self, select: Option<&str>) {
        let keep = select.map(str::to_string).or_else(|| self.sqt.listed().map(|l| l.id.clone()));
        let draft = self.sqt.draft_selected();
        self.sqt.list = squadfile::load_all(&self.config);
        self.sqt.stamp = squadfile::stamp(&self.config);
        // Reading the squads may have converted an old squad file into agents.
        if agentfile::stamp(&self.config) != self.sqt.agent_stamp {
            self.reload_agents(None);
        }
        self.sqt.selected = if draft && select.is_none() {
            self.sqt.list.len()
        } else {
            keep.and_then(|id| self.sqt.list.iter().position(|l| l.id == id)).unwrap_or(0)
        };
        self.clamp_squad_row();
    }

    /// Read the agents again, keeping the selection on the same agent (or on `select`, an id).
    fn reload_agents(&mut self, select: Option<&str>) {
        let keep = select.map(str::to_string).or_else(|| self.sqt.agent_listed().map(|l| l.id.clone()));
        let draft = self.sqt.agent_draft_selected();
        let new_row = self.sqt.new_agent_selected();
        self.sqt.agents = agentfile::load_all(&self.config);
        self.sqt.agent_stamp = agentfile::stamp(&self.config);
        self.sqt.agent_selected = if draft && select.is_none() {
            self.sqt.agents.len()
        } else if new_row && select.is_none() {
            self.sqt.agent_len() - 1
        } else {
            keep.and_then(|id| self.sqt.agents.iter().position(|l| l.id == id)).unwrap_or(0)
        };
        self.clamp_agent_row();
    }

    fn clamp_squad_row(&mut self) {
        self.sqt.selected = self.sqt.selected.min(self.sqt.len().saturating_sub(1));
        let n = self.sqt.squad().map_or(0, |s| detail_rows(s).len());
        self.sqt.row = self.sqt.row.min(n.saturating_sub(1));
    }

    fn clamp_agent_row(&mut self) {
        self.sqt.agent_selected = self.sqt.agent_selected.min(self.sqt.agent_len().saturating_sub(1));
        let n = self.agent_detail_rows().len();
        self.sqt.agent_row = self.sqt.agent_row.min(n.saturating_sub(1));
    }

    fn move_squad(&mut self, delta: i32) {
        if self.sqt.view == SqView::Agents {
            let last = self.sqt.agent_len().saturating_sub(1) as i32;
            let to = (self.sqt.agent_selected as i32 + delta).clamp(0, last) as usize;
            if to != self.sqt.agent_selected {
                self.sqt.agent_selected = to;
                self.sqt.agent_row = 0;
                self.sqt.agent_top.set(0);
            }
            return;
        }
        let last = self.sqt.len().saturating_sub(1) as i32;
        let to = (self.sqt.selected as i32 + delta).clamp(0, last) as usize;
        if to != self.sqt.selected {
            self.sqt.selected = to;
            self.sqt.row = 0;
            self.sqt.detail_top.set(0);
        }
    }

    fn move_squad_row(&mut self, delta: i32) {
        if self.sqt.view == SqView::Agents {
            let n = self.agent_detail_rows().len();
            self.sqt.agent_row = (self.sqt.agent_row as i32 + delta).clamp(0, n.saturating_sub(1) as i32) as usize;
            return;
        }
        let n = self.sqt.squad().map_or(0, |s| detail_rows(s).len());
        self.sqt.row = (self.sqt.row as i32 + delta).clamp(0, n.saturating_sub(1) as i32) as usize;
    }

    /// Show the Squads list or the Agents list in the left pane.
    pub(super) fn show_squad_view(&mut self, view: SqView) {
        if self.sqt.view != view {
            self.sqt.view = view;
            self.sqt.focus = SqFocus::List;
        }
    }

    /// A click on the list's entry `i`: selects it (the list has the keys).
    pub(super) fn click_squad(&mut self, i: usize) {
        self.sqt.focus = SqFocus::List;
        if self.sqt.view == SqView::Agents {
            if i < self.sqt.agent_len() {
                if i != self.sqt.agent_selected {
                    self.sqt.agent_row = 0;
                    self.sqt.agent_top.set(0);
                }
                self.sqt.agent_selected = i;
            }
            return;
        }
        if i < self.sqt.len() {
            if i != self.sqt.selected {
                self.sqt.row = 0;
                self.sqt.detail_top.set(0);
            }
            self.sqt.selected = i;
        }
    }

    /// A click on row `i` of the details: selects it (the details have the keys); on the selected one, Enter.
    pub(super) fn click_squad_row(&mut self, i: usize) {
        let on = if self.sqt.view == SqView::Agents { self.sqt.agent_row } else { self.sqt.row };
        if self.sqt.focus == SqFocus::Detail && on == i {
            return self.squad_enter();
        }
        self.sqt.focus = SqFocus::Detail;
        if self.sqt.view == SqView::Agents {
            self.sqt.agent_row = i;
            self.clamp_agent_row();
        } else {
            self.sqt.row = i;
            self.clamp_squad_row();
        }
    }

    pub(super) fn wheel_squads(&mut self, up: bool) {
        let d = if up { -1 } else { 1 };
        match self.sqt.focus {
            SqFocus::List => self.move_squad(d),
            SqFocus::Detail => self.move_squad_row(d),
        }
    }

    // ---- keys -------------------------------------------------------------------------------------------------

    /// Keys on the Squads tab. Plain ↑↓ move inside a pane; on the list, plain ← → switch between the Squads and the
    /// Agents lists. Shift or Ctrl with ← and → (and Tab) move between the list and the details, as on a project's
    /// screen.
    pub(super) fn squads_tab_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let across = key.modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Left if across => return self.sqt.focus = SqFocus::List,
            KeyCode::Right if across => return self.focus_squad_detail(),
            KeyCode::Tab | KeyCode::BackTab => {
                return match self.sqt.focus {
                    SqFocus::List => self.focus_squad_detail(),
                    SqFocus::Detail => self.sqt.focus = SqFocus::List,
                };
            }
            KeyCode::Char('s') if ctrl => return self.save_draft(),
            KeyCode::Char('n') if !ctrl => return self.ask_what_to_describe(),
            KeyCode::Char('c') if !ctrl => return self.ask_copy(),
            KeyCode::Char('d') if !ctrl => return self.ask_remove(),
            _ => {}
        }
        match self.sqt.focus {
            SqFocus::List => match key.code {
                KeyCode::Left => self.show_squad_view(SqView::Squads),
                KeyCode::Right => self.show_squad_view(SqView::Agents),
                KeyCode::Up => self.move_squad(-1),
                KeyCode::Down => self.move_squad(1),
                KeyCode::Char('g') | KeyCode::Home => self.move_squad(-(i32::MAX / 2)),
                KeyCode::Char('G') | KeyCode::End => self.move_squad(i32::MAX / 2),
                KeyCode::Enter if self.sqt.view == SqView::Agents && self.sqt.new_agent_selected() => self.open_squad_name(SquadName::NewAgent(None), ""),
                KeyCode::Enter => self.focus_squad_detail(),
                _ => {}
            },
            SqFocus::Detail if self.sqt.view == SqView::Agents => match key.code {
                KeyCode::Esc => self.sqt.focus = SqFocus::List,
                KeyCode::Up => self.move_squad_row(-1),
                KeyCode::Down => self.move_squad_row(1),
                KeyCode::Char('g') | KeyCode::Home => self.sqt.agent_row = 0,
                KeyCode::Char('G') | KeyCode::End => self.move_squad_row(i32::MAX / 2),
                KeyCode::Enter => self.squad_enter(),
                KeyCode::Char(' ') => self.agent_toggle(),
                _ => {}
            },
            SqFocus::Detail => {
                let on_step = self.sqt.drow().and_then(DRow::step).is_some();
                match key.code {
                    KeyCode::Esc => self.sqt.focus = SqFocus::List,
                    KeyCode::Up => self.move_squad_row(-1),
                    KeyCode::Down => self.move_squad_row(1),
                    KeyCode::Char('g') if on_step => self.go_to_step_agent(),
                    KeyCode::Char('g') | KeyCode::Home => self.sqt.row = 0,
                    KeyCode::Char('G') | KeyCode::End => self.move_squad_row(i32::MAX / 2),
                    KeyCode::Enter => self.squad_enter(),
                    KeyCode::Char('a') => self.squad_add(),
                    KeyCode::Char('x') => self.squad_remove_row(),
                    KeyCode::Char('m') => self.pick_step_agent(),
                    KeyCode::Char('b') => self.pick_send_back(),
                    KeyCode::Char('[') => self.move_step(true),
                    KeyCode::Char(']') => self.move_step(false),
                    _ => {}
                }
            }
        }
    }

    fn focus_squad_detail(&mut self) {
        if self.sqt.view == SqView::Agents {
            if self.sqt.agent().is_some() {
                self.sqt.focus = SqFocus::Detail;
                self.clamp_agent_row();
            } else if let Some(l) = self.sqt.agent_listed() {
                let why = l.corrupt.clone().unwrap_or_default();
                self.notify(format!("{}'s file can't be read ({why}), so there is nothing to change. Fix or move the file.", l.id));
            }
            return;
        }
        if self.sqt.squad().is_some() {
            self.sqt.focus = SqFocus::Detail;
            self.clamp_squad_row();
        } else if let Some(why) = self.sqt.listed().and_then(|l| l.corrupt.clone()) {
            self.notify(format!("{}'s file can't be read ({why}), so there is nothing to change. Fix or move the file.", self.sqt.listed().map_or("", |l| l.id.as_str())));
        }
    }

    // ---- changing a squad -----------------------------------------------------------------------------------

    /// The selected squad or agent (of the list showing) comes with a plugin, so it can't be changed here: said, and
    /// true. Copying it (`c`) makes one of the user's own.
    fn plugin_locked(&mut self) -> bool {
        let (what, label) = if self.sqt.view == SqView::Agents {
            ("agent", self.sqt.agent_listed().filter(|l| l.kind == Kind::Plugin).map(|l| (l.name().to_string(), l.plugin.clone().unwrap_or_default())))
        } else {
            ("squad", self.sqt.listed().filter(|l| l.kind == Kind::Plugin).map(|l| (l.name().to_string(), l.plugin.clone().unwrap_or_default())))
        };
        match label {
            Some((name, plugin)) => {
                self.notify(squadfile::read_only_words(what, &name, &plugin));
                true
            }
            None => false,
        }
    }

    /// The selected squad, to change: a copy of it, or None (said why) when its file can't be read or it comes with a
    /// plugin.
    fn squad_to_change(&mut self) -> Option<Squad> {
        if self.sqt.view == SqView::Squads && self.plugin_locked() {
            return None;
        }
        match self.sqt.squad() {
            Some(s) => Some(s.clone()),
            None => {
                if let Some(l) = self.sqt.listed() {
                    let msg = format!("{}'s file can't be read, so platoon leaves it alone. Fix or move the file.", l.id);
                    self.notify(msg);
                }
                None
            }
        }
    }

    /// Keep a change to the selected squad: the draft is changed in memory; a saved squad is written at once. True
    /// when it was kept.
    fn commit_squad(&mut self, s: Squad) -> bool {
        if self.sqt.draft_selected() {
            self.sqt.draft = Some(s);
            self.clamp_squad_row();
            return true;
        }
        match squadfile::save(&self.config, &s) {
            Ok(()) => {
                self.reload_squads(Some(&s.id));
                true
            }
            Err(e) => {
                let why = e.to_string();
                self.notify(format!("Couldn't save the squad: {}", why.split(" (os error").next().unwrap_or(&why)));
                self.reload_squads(None);
                false
            }
        }
    }

    /// Enter on a row of the details: change it.
    fn squad_enter(&mut self) {
        if self.sqt.view == SqView::Agents {
            return self.agent_enter();
        }
        if self.plugin_locked() {
            return;
        }
        let Some(s) = self.sqt.squad().cloned() else { return };
        let Some(row) = self.sqt.drow() else { return };
        match row {
            DRow::Name => self.open_squad_name(SquadName::Name, &s.name),
            DRow::CanDo => self.open_squad_text(TextFor::CanDo, &s.can_do, squadfile::CAN_DO_MAX),
            DRow::Step(i) => self.open_squad_text(TextFor::StepDoes(i), &s.workflow[i].does, squadfile::DOES_MAX),
            DRow::StepAgent(_) => self.pick_step_agent(),
            DRow::AddStep => self.squad_add(),
        }
    }

    /// The agent picker's entries: every agent (and the draft's new ones), each with its model and switches.
    fn agent_options(&self, with_new: bool) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .sqt
            .known_agents()
            .iter()
            .map(|a| {
                let new = if self.sqt.is_draft_agent(&a.id) {
                    " · new, saved with the draft".to_string()
                } else {
                    self.sqt.agents.iter().find(|l| l.id == a.id).and_then(|l| l.plugin.as_ref()).map_or(String::new(), |p| format!(" · from {p}"))
                };
                (format!("{}{new}", agent_line(a)), a.id.clone())
            })
            .collect();
        if with_new {
            out.push(("+ New agent".into(), NEW_AGENT.into()));
        }
        out
    }

    /// `a`: a new step, done by an agent picked from all of them.
    fn squad_add(&mut self) {
        let Some(s) = self.squad_to_change() else { return };
        if s.workflow.len() >= squadfile::MAX_STEPS {
            return self.notify(format!("A workflow has at most {} steps.", squadfile::MAX_STEPS));
        }
        let options = self.agent_options(true);
        self.open_choice(ChoiceFor::NewStepAgent, "Which agent does the new step", options, "");
    }

    /// `x`: take out the step the cursor is on.
    fn squad_remove_row(&mut self) {
        let Some(i) = self.sqt.drow().and_then(DRow::step) else { return };
        let Some(mut s) = self.squad_to_change() else { return };
        s.remove_step(i);
        if self.commit_squad(s) {
            self.notify(format!("Took step {} out of the workflow.", i + 1));
        }
    }

    /// Put the cursor on step `i` of the selected squad (the row `agent`: its agent's, else what it does).
    fn select_step(&mut self, i: usize, agent: bool) {
        let want = if agent { DRow::StepAgent(i) } else { DRow::Step(i) };
        if let Some(pos) = self.sqt.squad().and_then(|s| detail_rows(s).iter().position(|r| *r == want)) {
            self.sqt.row = pos;
        }
    }

    /// `[` and `]` on a step: move it up or down the workflow.
    fn move_step(&mut self, up: bool) {
        let Some(row) = self.sqt.drow() else { return };
        let Some(i) = row.step() else { return };
        let Some(mut s) = self.squad_to_change() else { return };
        let (to, cleared) = s.move_step(i, up);
        if to == i {
            return;
        }
        if self.commit_squad(s) {
            self.select_step(to, matches!(row, DRow::StepAgent(_)));
            let more = if cleared { " A step that sent the work back to a step no longer before it doesn't any more." } else { "" };
            self.notify(format!("Step {} is step {} now.{more}", i + 1, to + 1));
        }
    }

    /// `m` on a step, or enter on its agent: which agent does it (or a new one).
    fn pick_step_agent(&mut self) {
        let Some(i) = self.sqt.drow().and_then(DRow::step) else { return };
        if self.plugin_locked() {
            return;
        }
        let Some(s) = self.sqt.squad().cloned() else { return };
        let options = self.agent_options(true);
        self.open_choice(ChoiceFor::StepAgent(i), &format!("Which agent does step {}", i + 1), options, &s.workflow[i].agent);
    }

    /// `g` on a step: its agent, on the Agents list.
    fn go_to_step_agent(&mut self) {
        let Some(i) = self.sqt.drow().and_then(DRow::step) else { return };
        let Some(id) = self.sqt.squad().and_then(|s| s.workflow.get(i)).map(|st| st.agent.clone()) else { return };
        if self.sqt.is_draft_agent(&id) {
            return self.notify(format!("{id} comes with the draft: it is saved with it (ctrl+s), and then it is on the Agents list."));
        }
        match self.sqt.agents.iter().position(|l| l.id == id) {
            Some(at) => self.open_agent(at),
            None => self.notify(format!("There is no agent \"{id}\": enter picks one for step {}.", i + 1)),
        }
    }

    /// Show agent `at` of the list, its details with the keys.
    fn open_agent(&mut self, at: usize) {
        self.sqt.view = SqView::Agents;
        self.sqt.agent_selected = at;
        self.sqt.agent_row = 0;
        self.sqt.agent_top.set(0);
        self.sqt.focus = if self.sqt.agent().is_some() { SqFocus::Detail } else { SqFocus::List };
    }

    /// `b` on a step: may it send the work back, and to which earlier step.
    fn pick_send_back(&mut self) {
        let Some(i) = self.sqt.drow().and_then(DRow::step) else { return };
        if self.plugin_locked() {
            return;
        }
        let Some(s) = self.sqt.squad().cloned() else { return };
        if i == 0 {
            return self.notify("The first step has no step before it to send the work back to.");
        }
        let mut options = vec![("Never sends the work back".to_string(), String::new())];
        for (k, st) in s.workflow.iter().enumerate().take(i) {
            options.push((format!("Back to step {}: {} ({})", k + 1, st.agent, st.does), k.to_string()));
        }
        let now = s.workflow[i].back.map(|b| b.to.to_string()).unwrap_or_default();
        self.open_choice(ChoiceFor::BackTo(i), &format!("Where step {} may send the work back to", i + 1), options, &now);
    }

    // ---- changing an agent ----------------------------------------------------------------------------------

    /// The selected agent, to change: a copy of it, or None (said why) when its file can't be read or it comes with a
    /// plugin.
    fn agent_to_change(&mut self) -> Option<Agent> {
        if self.sqt.view == SqView::Agents && self.plugin_locked() {
            return None;
        }
        match self.sqt.agent() {
            Some(a) => Some(a.clone()),
            None => {
                if let Some(l) = self.sqt.agent_listed() {
                    let msg = format!("{}'s file can't be read, so platoon leaves it alone. Fix or move the file.", l.id);
                    self.notify(msg);
                }
                None
            }
        }
    }

    /// Keep a change to the selected agent: the draft is changed in memory; a saved agent is written at once (so every
    /// squad that uses it has it). True when it was kept.
    fn commit_agent(&mut self, a: Agent) -> bool {
        if self.sqt.agent_draft_selected() {
            self.sqt.agent_draft = Some(a);
            self.clamp_agent_row();
            return true;
        }
        match agentfile::save(&self.config, &a) {
            Ok(()) => {
                self.reload_agents(Some(&a.id));
                true
            }
            Err(e) => {
                let why = e.to_string();
                self.notify(format!("Couldn't save the agent: {}", why.split(" (os error").next().unwrap_or(&why)));
                self.reload_agents(None);
                false
            }
        }
    }

    /// Enter on a row of an agent's details.
    fn agent_enter(&mut self) {
        let Some(a) = self.sqt.agent().cloned() else { return };
        let Some(row) = self.arow() else { return };
        if !matches!(row, ARow::UsedIn(_)) && self.plugin_locked() {
            return;
        }
        match row {
            ARow::Name => self.open_squad_name(SquadName::AgentName, &a.name),
            ARow::Model => self.open_choice(ChoiceFor::AgentModel, &format!("Model for {}", a.name), model_options(), &a.model),
            ARow::Files | ARow::Commands | ARow::Web => self.agent_toggle(),
            ARow::Skills => self.pick_agent_tools(false, 0),
            ARow::Mcp => self.pick_agent_tools(true, 0),
            ARow::Prompt => self.edit_agent_prompt(),
            ARow::UsedIn(k) => {
                let Some(u) = self.agent_users(&a.id).get(k).cloned() else { return };
                let at = match &u.squad {
                    Some(id) => self.sqt.list.iter().position(|l| l.id == *id),
                    None => self.sqt.draft.as_ref().map(|_| self.sqt.list.len()),
                };
                let Some(at) = at else { return };
                self.sqt.view = SqView::Squads;
                self.sqt.selected = at;
                self.sqt.detail_top.set(0);
                self.sqt.focus = SqFocus::Detail;
                self.select_step(u.steps.first().map_or(0, |n| n - 1), true);
            }
        }
    }

    /// Space (or Enter) on one of an agent's switches.
    fn agent_toggle(&mut self) {
        let Some(row) = self.arow() else { return };
        if !matches!(row, ARow::Files | ARow::Commands | ARow::Web) {
            return;
        }
        let Some(mut a) = self.agent_to_change() else { return };
        let (what, now) = match row {
            ARow::Files => {
                a.files = !a.files;
                ("change files", a.files)
            }
            ARow::Commands => {
                a.commands = !a.commands;
                ("run commands", a.commands)
            }
            _ => {
                a.web = !a.web;
                ("use the web", a.web)
            }
        };
        let said = format!("{} {} {what} now{}.", a.name, if now { "can" } else { "can't" }, self.everywhere(&a.id));
        if self.commit_agent(a) {
            self.notify(said);
        }
    }

    /// Enter on an agent's Skills or MCP servers: every one installed here, ticked when the agent has it, the ones it
    /// has that aren't installed here too ("not found here": they may be a ticket folder's, or another computer's),
    /// and typing a name. Enter ticks or unticks one and the list stays open at `at`; esc closes it.
    fn pick_agent_tools(&mut self, mcp: bool, at: usize) {
        let Some(a) = self.sqt.agent().cloned() else { return };
        if self.plugin_locked() {
            return;
        }
        let installed = self.sqt.installed();
        let (has, there) = if mcp { (&a.mcp, &installed.mcp) } else { (&a.skills, &installed.skills) };
        let mut names: Vec<String> = there.clone();
        for n in has {
            if !names.contains(n) {
                names.push(n.clone());
            }
        }
        let mut options: Vec<(String, String)> = names
            .iter()
            .map(|n| {
                let tick = if has.contains(n) { "[x]" } else { "[ ]" };
                let away = if there.contains(n) { "" } else { " · not found here" };
                (format!("{tick} {n}{away}"), n.clone())
            })
            .collect();
        let what = if mcp { "an MCP server" } else { "a skill" };
        options.push((format!("Type the name of {what}…"), TYPE_NAME.into()));
        let (purpose, title) = if mcp {
            (ChoiceFor::AgentMcp, format!("MCP servers {} may use (enter ticks one; esc closes)", a.name))
        } else {
            (ChoiceFor::AgentSkills, format!("Skills {} starts with (enter ticks one; esc closes)", a.name))
        };
        self.open_choice(purpose, &title, options, "");
        self.sqt.choice.selected = at.min(self.sqt.choice.options.len().saturating_sub(1));
    }

    /// Give the selected agent a skill or an MCP server, or take it away. Said in a notice.
    fn toggle_agent_tool(&mut self, mcp: bool, name: &str) -> bool {
        let Some(mut a) = self.agent_to_change() else { return false };
        let list = if mcp { &mut a.mcp } else { &mut a.skills };
        let now = if let Some(at) = list.iter().position(|n| n == name) {
            list.remove(at);
            false
        } else {
            list.push(name.to_string());
            true
        };
        let what = if mcp { "MCP server" } else { "skill" };
        let said = if now {
            let away = if (if mcp { self.sqt.installed().mcp } else { self.sqt.installed().skills }).iter().any(|n| n == name) { "" } else { " (not found here: it has to be there where the squad works)" };
            format!("{} has the {what} {name} now{away}{}.", a.name, self.everywhere(&a.id))
        } else {
            format!("{} doesn't have the {what} {name} any more{}.", a.name, self.everywhere(&a.id))
        };
        if self.commit_agent(a) {
            self.notify(said);
            return true;
        }
        false
    }

    // ---- the small list, the name box and the text box ------------------------------------------------------

    fn open_choice(&mut self, purpose: ChoiceFor, title: &str, options: Vec<(String, String)>, now: &str) {
        let selected = options.iter().position(|(_, v)| v == now).unwrap_or(0);
        self.sqt.choice = Choice { purpose, title: title.to_string(), options, selected };
        self.mode = Mode::SquadChoice;
    }

    pub(super) fn move_squad_choice(&mut self, delta: i32) {
        let last = self.sqt.choice.options.len().saturating_sub(1) as i32;
        self.sqt.choice.selected = (self.sqt.choice.selected as i32 + delta).clamp(0, last) as usize;
    }

    pub(super) fn squad_choice_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.mode = Mode::List,
            KeyCode::Char('c') if ctrl => self.mode = Mode::List,
            KeyCode::Enter => self.take_squad_choice(),
            KeyCode::Up => self.move_squad_choice(-1),
            KeyCode::Down => self.move_squad_choice(1),
            _ => {}
        }
    }

    /// Enter in the small list.
    pub(super) fn take_squad_choice(&mut self) {
        self.mode = Mode::List;
        let Some((label, value)) = self.sqt.choice.options.get(self.sqt.choice.selected).cloned() else { return };
        let purpose = self.sqt.choice.purpose.clone();
        match purpose {
            ChoiceFor::Describe => return self.describe(value == "agent"),
            ChoiceFor::NewStepAgent => return self.open_squad_text(TextFor::NewStep(value), "", squadfile::DOES_MAX),
            ChoiceFor::StepAgent(i) if value == NEW_AGENT => return self.open_squad_name(SquadName::NewAgent(Some(i)), ""),
            ChoiceFor::AgentSkills | ChoiceFor::AgentMcp => {
                let mcp = purpose == ChoiceFor::AgentMcp;
                if value == TYPE_NAME {
                    return self.open_squad_name(if mcp { SquadName::AgentMcp } else { SquadName::AgentSkill }, "");
                }
                let at = self.sqt.choice.selected;
                self.toggle_agent_tool(mcp, &value);
                return self.pick_agent_tools(mcp, at);
            }
            ChoiceFor::AgentModel => {
                let Some(mut a) = self.agent_to_change() else { return };
                a.model = value;
                let said = format!("{} runs on {label} now{}.", a.name, self.everywhere(&a.id));
                if self.commit_agent(a) {
                    self.notify(said);
                }
                return;
            }
            ChoiceFor::BackTo(i) => {
                if value.is_empty() {
                    let Some(mut s) = self.squad_to_change() else { return };
                    if s.workflow.get(i).is_some_and(|st| st.back.is_some()) {
                        s.workflow[i].back = None;
                        if self.commit_squad(s) {
                            self.notify(format!("Step {} never sends the work back now.", i + 1));
                        }
                    }
                    return;
                }
                let to: usize = value.parse().unwrap_or(0);
                let now = self.sqt.squad().and_then(|s| s.workflow.get(i)).and_then(|st| st.back).filter(|b| b.to == to).map_or(1, |b| b.times);
                let options = (1..=squadfile::MAX_TIMES).map(|n| (format!("At most {}", times_label(n)), n.to_string())).collect();
                return self.open_choice(ChoiceFor::BackTimes(i, to), &format!("How often step {} may send it back to step {}", i + 1, to + 1), options, &now.to_string());
            }
            _ => {}
        }
        let Some(mut s) = self.squad_to_change() else { return };
        let _ = &label;
        let said = match purpose {
            ChoiceFor::StepAgent(i) => {
                if s.workflow[i].agent == value {
                    return;
                }
                s.workflow[i].agent = value.clone();
                let name = self.sqt.known_agents().into_iter().find(|a| a.id == value).map_or(value, |a| a.name);
                format!("{name} does step {} now.", i + 1)
            }
            ChoiceFor::BackTimes(i, to) => {
                let times: u32 = value.parse().unwrap_or(1);
                s.workflow[i].back = Some(Back { to, times });
                format!("Step {} {} now.", i + 1, back_words(&Back { to, times }))
            }
            _ => return,
        };
        if self.commit_squad(s) {
            self.notify(said);
        }
    }

    fn open_squad_name(&mut self, purpose: SquadName, now: &str) {
        self.name_box = NameBox { purpose: NamePurpose::Squad(purpose), input: now.to_string() };
        self.mode = Mode::ProjectName;
    }

    /// The longest the name box takes for this.
    pub(super) fn squad_name_max(purpose: &SquadName) -> usize {
        match purpose {
            SquadName::AgentSkill | SquadName::AgentMcp => agentfile::TOOL_NAME_MAX,
            _ => squadfile::NAME_MAX,
        }
    }

    /// Enter in the name box, for a squad or an agent. The box stays open while what is typed can't be used.
    pub(super) fn save_squad_name(&mut self, purpose: SquadName) {
        let typed = self.name_box.input.split_whitespace().collect::<Vec<_>>().join(" ");
        if typed.is_empty() {
            return self.notify(match purpose {
                SquadName::Name | SquadName::Copy => "A squad needs a name.",
                SquadName::AgentSkill => "Type the skill's name, as its SKILL.md calls it.",
                SquadName::AgentMcp => "Type the MCP server's name, as it is configured.",
                _ => "An agent needs a name.",
            });
        }
        if matches!(purpose, SquadName::AgentSkill | SquadName::AgentMcp) && !agentfile::name_ok(&typed) {
            return self.notify("That can't be a name: use letters, digits, dashes, dots, colons and underscores, no spaces.");
        }
        self.mode = Mode::List;
        match purpose {
            SquadName::Name => {
                let Some(mut s) = self.squad_to_change() else { return };
                if s.name == typed {
                    return;
                }
                s.name = typed.clone();
                if self.commit_squad(s) {
                    self.notify(format!("Renamed the squad to \"{typed}\"."));
                }
            }
            SquadName::Copy => {
                let Some(s) = self.sqt.squad().cloned() else { return };
                let copy = squadfile::copy_of(&self.config, &s, &typed, &[]);
                match squadfile::save(&self.config, &copy) {
                    Ok(()) => {
                        self.reload_squads(Some(&copy.id));
                        self.sqt.row = 0;
                        self.notify(format!("Made \"{typed}\", a copy of {}. It is yours to change; it uses the same agents.", s.name));
                    }
                    Err(e) => self.notify(format!("Couldn't save the copy: {e}")),
                }
            }
            SquadName::AgentName => {
                let Some(mut a) = self.agent_to_change() else { return };
                if a.name == typed {
                    return;
                }
                let old = std::mem::replace(&mut a.name, typed.clone());
                if self.commit_agent(a) {
                    self.notify(format!("Renamed {old} to \"{typed}\"."));
                }
            }
            SquadName::AgentCopy => {
                let Some(a) = self.sqt.agent().cloned() else { return };
                let copy = agentfile::copy_of(&self.config, &a, &typed, &[]);
                match agentfile::save(&self.config, &copy) {
                    Ok(()) => {
                        self.reload_agents(Some(&copy.id));
                        self.sqt.agent_row = 0;
                        self.notify(format!("Made \"{typed}\", a copy of {}. It is yours to change; no squad uses it yet.", a.name));
                    }
                    Err(e) => self.notify(format!("Couldn't save the copy: {e}")),
                }
            }
            SquadName::AgentSkill | SquadName::AgentMcp => {
                let mcp = purpose == SquadName::AgentMcp;
                let has = self.sqt.agent().is_some_and(|a| if mcp { a.mcp.contains(&typed) } else { a.skills.contains(&typed) });
                if has {
                    return self.notify(format!("It has {typed} already."));
                }
                self.toggle_agent_tool(mcp, &typed);
            }
            SquadName::NewAgent(step) => self.make_new_agent(&typed, step, None),
            SquadName::NewAgentStep(does) => self.make_new_agent(&typed, None, Some(does)),
        }
    }

    /// A new, blank agent of the user's own, saved at once and shown on the Agents list to be filled in; with `step`,
    /// put on that step of the selected squad first; with `new_step`, doing a new step (which does that) added to it.
    fn make_new_agent(&mut self, name: &str, step: Option<usize>, new_step: Option<String>) {
        let taken: Vec<String> = self.sqt.draft_agents.iter().map(|a| a.id.clone()).collect();
        let a = agentfile::blank(&self.config, name, &taken);
        if let Err(e) = agentfile::save(&self.config, &a) {
            return self.notify(format!("Couldn't save the agent: {e}"));
        }
        self.reload_agents(Some(&a.id));
        let mut said = format!("Made the agent \"{}\". It needs a system prompt before a squad can use it: enter on System prompt writes one.", a.name);
        if let Some(i) = step {
            if let Some(mut s) = self.squad_to_change().filter(|s| i < s.workflow.len()) {
                s.workflow[i].agent = a.id.clone();
                if self.commit_squad(s) {
                    said = format!("Made the agent \"{}\" and put it on step {}. It needs a system prompt before the squad can start: enter on System prompt writes one.", a.name, i + 1);
                }
            }
        }
        if let Some(does) = new_step {
            if let Some(mut s) = self.squad_to_change() {
                s.workflow.push(WStep { agent: a.id.clone(), does, back: None, extra: Default::default() });
                let n = s.workflow.len();
                if self.commit_squad(s) {
                    self.select_step(n - 1, true);
                    said = format!("Made the agent \"{}\" and added step {n}, done by it. It needs a system prompt before the squad can start: enter on System prompt writes one.", a.name);
                }
            }
        }
        if let Some(at) = self.sqt.agents.iter().position(|l| l.id == a.id) {
            self.open_agent(at);
            if let Some(pos) = self.agent_detail_rows().iter().position(|r| *r == ARow::Prompt) {
                self.sqt.agent_row = pos;
            }
        }
        self.notify(said);
    }

    fn open_squad_text(&mut self, purpose: TextFor, now: &str, max: usize) {
        self.sqt.text = TextArea::new(now, max);
        self.sqt.text_for = purpose;
        self.mode = Mode::SquadText;
    }

    pub(super) fn squad_text_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => return self.mode = Mode::List,
            KeyCode::Char('c') if ctrl => return self.mode = Mode::List,
            KeyCode::Enter => return self.take_squad_text(),
            KeyCode::Char('s') if ctrl => return self.take_squad_text(),
            KeyCode::Char(c) if !ctrl && !alt => return self.squad_type(&c.to_string()),
            _ => {}
        }
        let t = &mut self.sqt.text;
        match key.code {
            KeyCode::Char('u') if ctrl => {
                t.text.clear();
                t.cursor = 0;
            }
            KeyCode::Backspace => t.backspace(),
            KeyCode::Delete => t.delete(),
            KeyCode::Left => t.step(-1),
            KeyCode::Right => t.step(1),
            KeyCode::Up => t.row(-1),
            KeyCode::Down => t.row(1),
            KeyCode::Home => t.cursor = 0,
            KeyCode::End => t.cursor = t.text.chars().count(),
            _ => {}
        }
    }

    pub(super) fn squad_type(&mut self, text: &str) {
        if !self.sqt.text.insert(text) {
            let max = self.sqt.text.max;
            self.notify(format!("That doesn't fit: at most {max} characters."));
        }
    }

    /// Enter in the text box.
    fn take_squad_text(&mut self) {
        let typed = self.sqt.text.text.split_whitespace().collect::<Vec<_>>().join(" ");
        let purpose = self.sqt.text_for.clone();
        if typed.is_empty() {
            let why = match purpose {
                TextFor::Describe => "Describe the squad first: what it is for, and the work it does.",
                TextFor::DescribeAgent => "Describe the agent first: the one job it does.",
                TextFor::CanDo => "Say what the squad can do: the commander reads it before a ticket is handed over.",
                _ => "Say what the step does.",
            };
            return self.notify(why);
        }
        self.mode = Mode::List;
        match purpose {
            TextFor::Describe => {
                self.sqt.last_description = typed.clone();
                return self.draft_squad(&typed);
            }
            TextFor::DescribeAgent => {
                self.sqt.last_agent_description = typed.clone();
                return self.draft_agent(&typed);
            }
            _ => {}
        }
        let Some(mut s) = self.squad_to_change() else { return };
        let said = match purpose {
            TextFor::CanDo => {
                s.can_do = typed;
                "Saved what the squad can do.".to_string()
            }
            TextFor::StepDoes(i) => {
                if s.workflow.get(i).is_some_and(|st| st.does == typed) {
                    return;
                }
                s.workflow[i].does = typed;
                format!("Saved what step {} does.", i + 1)
            }
            TextFor::NewStep(agent) if agent == NEW_AGENT => {
                // Its name next; the agent and the step are made together.
                return self.open_squad_name(SquadName::NewAgentStep(typed), "");
            }
            TextFor::NewStep(agent) => {
                s.workflow.push(WStep { agent: agent.clone(), does: typed, back: None, extra: Default::default() });
                let name = self.sqt.known_agents().into_iter().find(|a| a.id == agent).map_or(agent, |a| a.name);
                format!("Added step {}, done by {name}. b says whether it may send the work back.", s.workflow.len())
            }
            TextFor::Describe | TextFor::DescribeAgent => return,
        };
        let new_step = matches!(self.sqt.text_for, TextFor::NewStep(_));
        let steps = s.workflow.len();
        if self.commit_squad(s) {
            if new_step {
                self.select_step(steps - 1, false);
            }
            self.notify(said);
        }
    }

    // ---- system prompts, in the editors ----------------------------------------------------------------------

    /// Enter on an agent's system prompt.
    fn edit_agent_prompt(&mut self) {
        let Some(a) = self.sqt.agent().cloned() else { return };
        if self.plugin_locked() {
            return;
        }
        let draft = self.sqt.agent_draft_selected();
        let target = PromptOf::Agent { id: a.id.clone(), draft };
        self.edit_prompt(target, &format!("agent-{}{}", if draft { "draft-" } else { "" }, a.id), &a.guidance);
    }

    fn edit_prompt(&mut self, target: PromptOf, file: &str, guidance: &str) {
        let path = self.cache.join("squad-prompts").join(format!("{file}.md"));
        if let Err(e) = crate::projects::write_atomic(&path, &format!("{}\n", guidance.trim())) {
            return self.notify(format!("Couldn't start the system prompt file: {e}"));
        }
        let edit = SquadPromptEdit { target, path, before: guidance.trim().to_string() };
        self.pick_squad_editor(edit);
    }

    /// After an editor has been and gone from a system prompt: keep what it says now.
    pub(super) fn after_squad_prompt_edit(&mut self, edit: &SquadPromptEdit) {
        let text = std::fs::read_to_string(&edit.path).unwrap_or_default();
        let _ = std::fs::remove_file(&edit.path);
        let text = text.trim();
        let text: String = match text.char_indices().nth(instructions::MAX_CHARS) {
            Some((at, _)) => text[..at].trim_end().to_string(),
            None => text.to_string(),
        };
        if text == edit.before {
            return;
        }
        match &edit.target {
            PromptOf::Agent { id, draft } => {
                let a = if *draft {
                    self.sqt.agent_draft.clone().filter(|d| d.id == *id)
                } else {
                    self.sqt.agents.iter().find(|l| l.id == *id).and_then(|l| l.agent.clone())
                };
                let Some(mut a) = a else {
                    return self.notify("The agent isn't there any more, so the system prompt wasn't kept.");
                };
                if text.is_empty() && !a.guidance.trim().is_empty() {
                    return self.notify(format!("A system prompt can't be empty, so {}'s is as it was.", a.name));
                }
                a.guidance = text;
                if *draft {
                    let name = a.name.clone();
                    self.sqt.agent_draft = Some(a);
                    return self.notify(format!("Changed {name}'s system prompt in the draft."));
                }
                let said = match self.agent_users(id).len() {
                    0 => format!("Saved {}'s system prompt.", a.name),
                    _ => format!("Saved {}'s system prompt{}, from the next ticket each is handed.", a.name, self.everywhere(id)),
                };
                match agentfile::save(&self.config, &a) {
                    Ok(()) => {
                        self.reload_agents(None);
                        self.notify(said);
                    }
                    Err(e) => self.notify(format!("Couldn't save the agent: {e}")),
                }
            }
        }
    }

    // ---- making, copying, deleting ---------------------------------------------------------------------------

    /// `n`: what to describe for Claude to draft, a new squad or a new agent (the one the list shows comes first).
    fn ask_what_to_describe(&mut self) {
        if let Some(k) = self.sqt.drafting() {
            let what = if k == Drafting::Agent { "an agent" } else { "a squad" };
            return self.notify(format!("Claude is drafting {what} already. It shows here when it is done."));
        }
        let options = vec![("A new squad".to_string(), "squad".to_string()), ("A new agent".to_string(), "agent".to_string())];
        let now = if self.sqt.view == SqView::Agents { "agent" } else { "squad" };
        self.open_choice(ChoiceFor::Describe, "What to describe for Claude to draft", options, now);
    }

    /// The description box for a new squad or agent (one unsaved draft of each at a time).
    fn describe(&mut self, agent: bool) {
        if agent {
            if self.sqt.agent_draft.is_some() {
                self.sqt.view = SqView::Agents;
                self.sqt.agent_selected = self.sqt.agents.len();
                return self.notify("There is a draft agent already: ctrl+s saves it, d discards it.");
            }
            let last = self.sqt.last_agent_description.clone();
            return self.open_squad_text(TextFor::DescribeAgent, &last, squadfile::DESCRIPTION_MAX);
        }
        if self.sqt.draft.is_some() {
            self.sqt.view = SqView::Squads;
            self.sqt.selected = self.sqt.list.len();
            return self.notify("There is a draft already: ctrl+s saves it, d discards it.");
        }
        let last = self.sqt.last_description.clone();
        self.open_squad_text(TextFor::Describe, &last, squadfile::DESCRIPTION_MAX);
    }

    /// One `claude -p` on Sonnet drafts the whole squad (told the agents there are, to reuse), off the UI thread. If
    /// the gate says no there is no call.
    fn draft_squad(&mut self, description: &str) {
        let settings = self.settings.get();
        if let Err(r) = ai::gate_for(&settings, squadfile::DRAFT_FEATURE, &self.cache, self.clock(), true) {
            return self.notify(format!("Couldn't draft a squad: {}. {}", crate::squads::refusal_words(&r), r.on_screen()));
        }
        self.sqt.reread_installed();
        let prompt = squadfile::draft_prompt(description, &self.agent_library(), &self.sqt.installed());
        let mut req = Request::new(squadfile::DRAFT_FEATURE, squadfile::draft_system(&instructions::guidance(&self.config, instructions::SQUAD_DRAFT)), prompt);
        req.user_initiated = true;
        req.model = Some(squadfile::DRAFT_MODEL);
        req.timeout = DRAFT_TIMEOUT;
        req.cancel = self.switchboard.token(squadfile::DRAFT_FEATURE);
        self.sqt.drafting = Some((Drafting::Squad, self.spawn_call(settings, req)));
        self.notify("Claude is drafting the squad. It shows here as a draft when it is done.");
    }

    /// One `claude -p` on Sonnet drafts an agent, off the UI thread.
    fn draft_agent(&mut self, description: &str) {
        let settings = self.settings.get();
        if let Err(r) = ai::gate_for(&settings, agentfile::DRAFT_FEATURE, &self.cache, self.clock(), true) {
            return self.notify(format!("Couldn't draft an agent: {}. {}", crate::squads::refusal_words(&r), r.on_screen()));
        }
        let mut req = Request::new(agentfile::DRAFT_FEATURE, agentfile::draft_system(&instructions::guidance(&self.config, instructions::AGENT_DRAFT)), agentfile::draft_prompt(description, &{
            self.sqt.reread_installed();
            self.sqt.installed()
        }));
        req.user_initiated = true;
        req.model = Some(squadfile::DRAFT_MODEL);
        req.timeout = DRAFT_TIMEOUT;
        req.cancel = self.switchboard.token(agentfile::DRAFT_FEATURE);
        self.sqt.drafting = Some((Drafting::Agent, self.spawn_call(settings, req)));
        self.notify("Claude is drafting the agent. It shows on the Agents list as a draft when it is done.");
    }

    /// Take in the drafted squad or agent, if it has come. Called on every refresh; cheap when nothing is out.
    pub(super) fn poll_squad_draft(&mut self) {
        let answer = match self.sqt.drafting.as_ref().map(|(_, rx)| rx.try_recv()) {
            None | Some(Err(TryRecvError::Empty)) => return,
            Some(Ok(a)) => a,
            Some(Err(TryRecvError::Disconnected)) => Err(AiError::Cancelled),
        };
        let Some((kind, _)) = self.sqt.drafting.take() else { return };
        let what = if kind == Drafting::Agent { "an agent" } else { "a squad" };
        let reply = match answer {
            Ok(r) => r,
            Err(AiError::Cancelled) => return self.notify(format!("Drafting {what} was called off.")),
            Err(e) => return self.notify(format!("Couldn't draft {what}: {}", e.plain())),
        };
        if kind == Drafting::Agent {
            let taken: Vec<String> = self.sqt.draft_agents.iter().map(|a| a.id.clone()).collect();
            return match agentfile::parse_draft(&reply.text, &self.config, &taken, &self.sqt.installed()) {
                Ok(a) => {
                    let name = a.name.clone();
                    self.sqt.agent_draft = Some(a);
                    self.sqt.view = SqView::Agents;
                    self.sqt.agent_selected = self.sqt.agents.len();
                    self.sqt.focus = SqFocus::Detail;
                    self.sqt.agent_row = 0;
                    self.sqt.agent_top.set(0);
                    self.notify(format!("Here is a draft of the agent \"{name}\". Change what you like: ctrl+s saves it, d discards it."));
                }
                Err(why) => self.notify(format!("Claude's draft couldn't be used: {why} Press n to try again.")),
            };
        }
        let taken: Vec<String> = self.sqt.list.iter().map(|l| l.id.clone()).collect();
        let mut library = self.agent_library();
        if let Some(d) = self.sqt.agent_draft.as_ref() {
            // The draft agent's id is kept for it.
            library.retain(|a| a.id != d.id);
        }
        match squadfile::parse_draft(&reply.text, &self.config, &taken, &library, &self.sqt.installed()) {
            Ok(d) => {
                let name = d.squad.name.clone();
                let new = d.new_agents.len();
                self.sqt.draft = Some(d.squad);
                self.sqt.draft_agents = d.new_agents;
                self.sqt.view = SqView::Squads;
                self.sqt.selected = self.sqt.list.len();
                self.sqt.focus = SqFocus::Detail;
                self.sqt.row = 0;
                self.sqt.detail_top.set(0);
                let agents = match new {
                    0 => "It uses agents you have already.".to_string(),
                    1 => "It brings 1 new agent, saved with it.".to_string(),
                    n => format!("It brings {n} new agents, saved with it."),
                };
                self.notify(format!("Here is a draft of \"{name}\". {agents} Change what you like: ctrl+s saves it, d discards it."));
            }
            Err(why) => self.notify(format!("Claude's draft couldn't be used: {why} Press n to try again.")),
        }
    }

    /// ctrl+s: save the draft the list shows (the squad's with the agents it brings, or the agent), once nothing is
    /// wrong with it.
    fn save_draft(&mut self) {
        if self.sqt.view == SqView::Agents {
            return self.save_agent_draft();
        }
        let Some(mut s) = self.sqt.draft.clone() else { return };
        let mut known = self.agent_library();
        known.extend(self.sqt.draft_agents.iter().cloned());
        if let Some(first) = squadfile::problems(&s, &known).into_iter().next() {
            return self.notify(format!("Not saved yet: {first}"));
        }
        // Another squad may have taken its id meanwhile, or another agent one of the new agents' ids.
        if squadfile::path(&self.config, &s.id).exists() || squadfile::is_builtin(&s.id) {
            s.id = squadfile::new_id(&self.config, &s.name, &[]);
        }
        let mut saved: Vec<String> = Vec::new();
        for a in self.sqt.draft_agents.clone() {
            let mut a = a;
            if agentfile::path(&self.config, &a.id).exists() || agentfile::is_builtin(&a.id) {
                let id = agentfile::new_id(&self.config, &a.name, &saved);
                for st in s.workflow.iter_mut().filter(|st| st.agent == a.id) {
                    st.agent = id.clone();
                }
                a.id = id;
            }
            if let Err(e) = agentfile::save(&self.config, &a) {
                return self.notify(format!("Couldn't save the agent {}: {e}", a.name));
            }
            saved.push(a.id);
        }
        match squadfile::save(&self.config, &s) {
            Ok(()) => {
                self.sqt.draft = None;
                self.sqt.draft_agents.clear();
                self.reload_agents(None);
                self.reload_squads(Some(&s.id));
                let agents = match saved.len() {
                    0 => String::new(),
                    1 => " and its new agent".to_string(),
                    n => format!(" and its {n} new agents"),
                };
                self.notify(format!("Saved the squad \"{}\"{agents}. Tickets can be handed to it now.", s.name));
            }
            Err(e) => self.notify(format!("Couldn't save the squad: {e}")),
        }
    }

    fn save_agent_draft(&mut self) {
        let Some(mut a) = self.sqt.agent_draft.clone() else { return };
        if let Some(first) = agentfile::problems(&a).into_iter().next() {
            return self.notify(format!("Not saved yet: {first}"));
        }
        if agentfile::path(&self.config, &a.id).exists() || agentfile::is_builtin(&a.id) {
            a.id = agentfile::new_id(&self.config, &a.name, &[]);
        }
        match agentfile::save(&self.config, &a) {
            Ok(()) => {
                self.sqt.agent_draft = None;
                self.reload_agents(Some(&a.id));
                self.notify(format!("Saved the agent \"{}\". A squad's step can use it now (enter on a step's agent).", a.name));
            }
            Err(e) => self.notify(format!("Couldn't save the agent: {e}")),
        }
    }

    /// `c`: copy the selected squad or agent under a new name.
    fn ask_copy(&mut self) {
        if self.sqt.view == SqView::Agents {
            if self.sqt.agent_draft_selected() {
                return self.notify("Save the draft first (ctrl+s), then copy it.");
            }
            if self.sqt.new_agent_selected() {
                return;
            }
            // A plugin's agent can be copied: the copy is the user's own, to change.
            let Some(a) = self.sqt.agent().cloned() else {
                let _ = self.agent_to_change();
                return;
            };
            return self.open_squad_name(SquadName::AgentCopy, &format!("{} copy", a.name));
        }
        if self.sqt.draft_selected() {
            return self.notify("Save the draft first (ctrl+s), then copy it.");
        }
        let Some(s) = self.sqt.squad().cloned() else {
            let _ = self.squad_to_change();
            return;
        };
        self.open_squad_name(SquadName::Copy, &format!("{} copy", s.name));
    }

    /// `d`: discard a draft, delete one of the user's own, or put a changed built-in back as platoon has it, each after
    /// asking. A built-in as platoon has it can't be deleted, and nor can an agent a squad uses.
    fn ask_remove(&mut self) {
        if self.sqt.view == SqView::Agents {
            return self.ask_remove_agent();
        }
        if self.sqt.draft_selected() {
            let name = self.sqt.draft.as_ref().map(|d| d.name.clone()).unwrap_or_default();
            self.mode = Mode::Confirm(Confirm::DiscardDraft { name });
            return;
        }
        let Some(l) = self.sqt.listed().cloned() else { return };
        let name = l.name().to_string();
        self.mode = match l.kind {
            Kind::Own => Mode::Confirm(Confirm::DeleteSquad { id: l.id, name }),
            Kind::Modified => Mode::Confirm(Confirm::RestoreSquad { id: l.id, name }),
            Kind::BuiltIn => {
                return self.notify(format!("{name} is built into platoon, so it can't be deleted. It is as platoon has it already; c makes a copy of your own."));
            }
            Kind::Plugin => {
                let plugin = l.plugin.unwrap_or_default();
                return self.notify(format!("{name} comes with the plugin {plugin}: removing the plugin (the Plugins tab, 4) takes it off. c makes a copy of your own."));
            }
        };
    }

    fn ask_remove_agent(&mut self) {
        if self.sqt.agent_draft_selected() {
            let name = self.sqt.agent_draft.as_ref().map(|d| d.name.clone()).unwrap_or_default();
            self.mode = Mode::Confirm(Confirm::DiscardAgentDraft { name });
            return;
        }
        let Some(l) = self.sqt.agent_listed().cloned() else { return };
        let name = l.name().to_string();
        self.mode = match l.kind {
            Kind::Own => {
                let mut squads: Vec<Squad> = self.sqt.list.iter().filter_map(|l| l.squad.clone()).collect();
                if let Some(d) = self.sqt.draft.clone() {
                    squads.push(Squad { name: format!("{} (draft)", d.name), ..d });
                }
                if let Some(why) = agentfile::in_use(&name, &l.id, &squads) {
                    return self.notify(why);
                }
                Mode::Confirm(Confirm::DeleteAgent { id: l.id, name })
            }
            Kind::Modified => Mode::Confirm(Confirm::RestoreAgent { id: l.id, name }),
            Kind::BuiltIn => {
                return self.notify(format!("{name} is built into platoon, so it can't be deleted. It is as platoon has it already; c makes a copy of your own."));
            }
            Kind::Plugin => {
                let plugin = l.plugin.unwrap_or_default();
                return self.notify(format!("{name} comes with the plugin {plugin}: removing the plugin (the Plugins tab, 4) takes it off. c makes a copy of your own."));
            }
        };
    }

    pub(super) fn remove_squad(&mut self, id: &str, restore: bool) {
        self.mode = Mode::List;
        let name = self.squad_name(id);
        match squadfile::remove(&self.config, id) {
            Ok(()) => {
                self.reload_squads(if restore { Some(id) } else { None });
                if !restore {
                    self.sqt.selected = self.sqt.selected.min(self.sqt.len().saturating_sub(1));
                }
                self.sqt.focus = SqFocus::List;
                self.notify(if restore {
                    format!("{name} is as platoon has it again. Your version is kept as {id}.json.bak in the squads folder.")
                } else {
                    format!("Deleted the squad \"{name}\" (kept as {id}.json.bak in the squads folder). Tickets it already has keep it.")
                });
            }
            Err(e) => self.notify(format!("Couldn't change the squad's file: {e}")),
        }
    }

    /// Yes to deleting one of the user's own agents, or to putting a changed built-in agent back.
    pub(super) fn remove_agent(&mut self, id: &str, restore: bool) {
        self.mode = Mode::List;
        let name = self.sqt.agents.iter().find(|l| l.id == id).map_or_else(|| id.to_string(), |l| l.name().to_string());
        let done = if restore { agentfile::remove(&self.config, id).map_err(|e| format!("Couldn't change the agent's file: {e}")) } else { agentfile::delete(&self.config, id) };
        match done {
            Ok(()) => {
                self.reload_agents(if restore { Some(id) } else { None });
                self.sqt.focus = SqFocus::List;
                self.notify(if restore {
                    format!("{name} is as platoon has it again{}. Your version is kept as {id}.json.bak in the agents folder.", self.everywhere(id))
                } else {
                    format!("Deleted the agent \"{name}\" (kept as {id}.json.bak in the agents folder).")
                });
            }
            Err(why) => self.notify(why),
        }
    }

    pub(super) fn discard_draft(&mut self) {
        self.mode = Mode::List;
        self.sqt.draft = None;
        self.sqt.draft_agents.clear();
        self.sqt.focus = SqFocus::List;
        self.clamp_squad_row();
        self.notify("Discarded the draft.");
    }

    pub(super) fn discard_agent_draft(&mut self) {
        self.mode = Mode::List;
        self.sqt.agent_draft = None;
        self.sqt.focus = SqFocus::List;
        self.clamp_agent_row();
        self.notify("Discarded the draft agent.");
    }

    /// True while a squad or an agent is being drafted (so the screen is refreshed quickly).
    pub fn squad_drafting(&self) -> bool {
        self.sqt.drafting().is_some()
    }
}

/// Whose system prompt is open in an editor.
#[derive(Clone, Debug)]
pub enum PromptOf {
    /// An agent (`draft`: the unsaved draft agent).
    Agent { id: String, draft: bool },
}

/// A system prompt open in an editor: it is written to a file of its own, and read back into the squad or agent.
#[derive(Clone, Debug)]
pub struct SquadPromptEdit {
    pub target: PromptOf,
    pub path: PathBuf,
    pub before: String,
}
