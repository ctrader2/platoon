//! The Squads tab (`4`). Its left pane switches (`←` `→`, or a click on its title) between two lists:
//! - **Squads**: built in, changed by the user, or the user's own (and an unsaved draft), the selected one's brief on
//!   the right, as it reads and with what is wrong with it (`ui/squads_view.rs`). The brief is a file the user owns
//!   (`squadfile.rs`, `brief.rs`): `e` opens it in the user's editor (a built-in's text is written there first) and
//!   platoon reads it again when the editor is done. The leader is not here: it is the same for every squad, in the
//!   settings.
//! - **Agents**: every agent, shared by the squads (built in, changed by the user, the user's own, an unsaved draft,
//!   and "+ New agent"), the selected one on the right: name, model, the three switches, its skills and MCP servers
//!   (picked from what is installed, or typed), system prompt, and the squads it is used in (`enter` goes to one). A
//!   change to an agent is a change in every squad that uses it.
//!
//! An agent's changes are written at once (a draft is changed in memory). `n` asks what to describe (a new squad or a
//! new agent) for Claude to draft (one `claude -p` on Sonnet, off the UI thread), `c` copies, `d` deletes the user's
//! own (an agent a squad uses is refused, naming the squads) or puts a built-in back as platoon has it, as `r` does.

use super::*;
use crate::agentfile::{self, Agent};
use crate::ai::{AiError, Reply, Request};
use crate::instructions;
use crate::squadfile::{self, Kind, Listed, Squad};
use std::sync::mpsc::TryRecvError;

/// How long drafting a squad or an agent may take before it is given up on.
const DRAFT_TIMEOUT: Duration = Duration::from_secs(180);

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
    /// The name of a copy of the selected squad.
    Copy,
    /// The selected agent's name.
    AgentName,
    /// The name of a copy of the selected agent (or of one from the user's coding tool, into platoon's own).
    AgentCopy,
    /// A new, blank agent, from "+ New agent".
    NewAgent,
    /// A skill the selected agent has, typed (one not installed here: a skill of a task's folder, say).
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
    /// The squads, as their briefs and platoon say (`squadfile::load_all`), read again when a file changes.
    pub list: Vec<Listed>,
    stamp: Vec<(String, Option<(std::time::SystemTime, u64)>)>,
    /// A drafted squad not saved yet: listed last.
    pub draft: Option<Squad>,
    /// The agents the drafted squad brings: saved with it.
    pub draft_agents: Vec<Agent>,
    /// The selected squad: an index into `list`, or `list.len()` for the draft.
    pub selected: usize,
    pub focus: SqFocus,
    /// The first line of the brief in view: the arrows scroll it while the details have the keys.
    pub detail_top: Cell<usize>,
    /// How many lines the brief had, and how many showed, as the last frame drew it (for the arrows' end).
    pub detail_size: Cell<(usize, usize)>,
    /// The tab was last drawn as one pane (a narrow terminal): the list, or the details full width in its place.
    pub one_pane: Cell<bool>,
    /// The agents: platoon's own (`agentfile::load_all`), then the ones the user's coding tool has
    /// (`agentfile::claude_listed`), read again when a file changes.
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
    /// Where Claude keeps its skills and agents (its config folder) and its user config file (the MCP servers).
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
            detail_top: Cell::new(0),
            detail_size: Cell::new((0, 0)),
            one_pane: Cell::new(false),
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
            // Set by `App::build`, where this window reads Claude's state.
            claude_root: PathBuf::new(),
            claude_json: PathBuf::new(),
            installed: RefCell::new(None),
        }
    }

    /// The skills and MCP servers installed here, read again when the last look is more than a few seconds old (the
    /// user may install one while platoon is open). Only names are read.
    pub fn installed(&self) -> agentfile::Installed {
        let mut cached = self.installed.borrow_mut();
        if let Some((_, i)) = cached.as_ref().filter(|(at, _)| at.elapsed() < Duration::from_secs(5)) {
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

    /// The selected squad's brief, if it can be read.
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

    /// The selected agent is one of the user's coding tool's: read only.
    pub fn agent_read_only(&self) -> bool {
        self.agent_listed().is_some_and(|l| l.kind == Kind::Claude)
    }

    /// Every agent that can be read, platoon's own first: what the squads' steps name (an id once, the first winning).
    pub fn library(&self) -> Vec<Agent> {
        let mut out: Vec<Agent> = Vec::new();
        for a in self.agents.iter().filter_map(|l| l.agent.as_ref()) {
            if !out.iter().any(|x| x.id == a.id) {
                out.push(a.clone());
            }
        }
        out
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

    /// Every agent that can be read, as the files say now (platoon's own first, then the user's coding tool's).
    pub fn agent_library(&self) -> Vec<Agent> {
        self.sqt.library()
    }

    /// The agents a task's squad may name, the first of an id winning: platoon's own, then the ones the user's coding
    /// tool has in the task's first folder (`.claude/agents`), then the user's own there (`~/.claude/agents`).
    pub fn agents_for(&self, folder: Option<&str>) -> Vec<Agent> {
        agentfile::resolve(&self.config, &self.sqt.claude_root, folder.map(Path::new))
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
            .filter_map(|s| {
                Some(s.steps_of(id)).filter(|n| !n.is_empty()).map(|steps| User { squad: Some(s.id.clone()), name: s.name.clone(), steps })
            })
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

    /// The agent files' stamp: platoon's own folder and the user's coding tool's.
    fn agents_stamp(&self) -> Vec<(String, Option<(std::time::SystemTime, u64)>)> {
        let mut s = agentfile::stamp(&self.config);
        s.extend(agentfile::claude_stamp(&self.sqt.claude_root));
        s
    }

    /// Read the squads and the agents again when their files changed (one listing of each folder when nothing did).
    pub(super) fn poll_squad_files(&mut self) {
        if squadfile::stamp(&self.config) != self.sqt.stamp {
            self.reload_squads(None);
        }
        if self.agents_stamp() != self.sqt.agent_stamp {
            self.reload_agents(None);
        }
    }

    /// Read the squads again, keeping the selection on the same squad (or on `select`, an id).
    pub(super) fn reload_squads(&mut self, select: Option<&str>) {
        let keep = select.map(str::to_string).or_else(|| self.sqt.listed().map(|l| l.id.clone()));
        let draft = self.sqt.draft_selected();
        let before = self.sqt.listed().map(|l| l.id.clone());
        self.sqt.list = squadfile::load_all(&self.config);
        self.sqt.stamp = squadfile::stamp(&self.config);
        // Reading the squads may have turned an old squad file's members into agents.
        if self.agents_stamp() != self.sqt.agent_stamp {
            self.reload_agents(None);
        }
        self.sqt.selected = if draft && select.is_none() {
            self.sqt.list.len()
        } else {
            keep.and_then(|id| self.sqt.list.iter().position(|l| l.id == id)).unwrap_or(0)
        };
        if self.sqt.listed().map(|l| l.id.clone()) != before {
            self.sqt.detail_top.set(0);
        }
        self.clamp_squad_row();
    }

    /// Read the agents again, keeping the selection on the same agent (or on `select`, an id).
    pub(super) fn reload_agents(&mut self, select: Option<&str>) {
        let keep = select.map(str::to_string).or_else(|| self.sqt.agent_listed().map(|l| l.id.clone()));
        let keep_kind = self.sqt.agent_listed().map(|l| l.kind == Kind::Claude).unwrap_or(false) && select.is_none();
        let draft = self.sqt.agent_draft_selected();
        let new_row = self.sqt.new_agent_selected();
        let mut agents = agentfile::load_all(&self.config);
        agents.extend(agentfile::claude_listed(&self.sqt.claude_root));
        self.sqt.agents = agents;
        self.sqt.agent_stamp = self.agents_stamp();
        self.sqt.agent_selected = if draft && select.is_none() {
            self.sqt.agents.len()
        } else if new_row && select.is_none() {
            self.sqt.agent_len() - 1
        } else {
            keep.and_then(|id| self.sqt.agents.iter().position(|l| l.id == id && (l.kind == Kind::Claude) == keep_kind)).unwrap_or(0)
        };
        self.clamp_agent_row();
    }

    fn clamp_squad_row(&mut self) {
        self.sqt.selected = self.sqt.selected.min(self.sqt.len().saturating_sub(1));
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
            self.sqt.detail_top.set(0);
        }
    }

    /// The arrows in the details: an agent's rows, or the brief's lines.
    fn move_squad_row(&mut self, delta: i32) {
        if self.sqt.view == SqView::Agents {
            let n = self.agent_detail_rows().len();
            self.sqt.agent_row = (self.sqt.agent_row as i32 + delta).clamp(0, n.saturating_sub(1) as i32) as usize;
            return;
        }
        let (lines, shown) = self.sqt.detail_size.get();
        let most = lines.saturating_sub(shown) as i64;
        let top = (self.sqt.detail_top.get() as i64 + i64::from(delta)).clamp(0, most.max(0));
        self.sqt.detail_top.set(top as usize);
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
                self.sqt.detail_top.set(0);
            }
            self.sqt.selected = i;
        }
    }

    /// A click on row `i` of the details: selects it (the details have the keys); on the selected one, Enter. The
    /// brief has no rows to pick: a click there gives it the keys.
    pub(super) fn click_squad_row(&mut self, i: usize) {
        if self.sqt.view == SqView::Squads {
            self.sqt.focus = SqFocus::Detail;
            return;
        }
        if self.sqt.focus == SqFocus::Detail && self.sqt.agent_row == i {
            return self.agent_enter();
        }
        self.sqt.focus = SqFocus::Detail;
        self.sqt.agent_row = i;
        self.clamp_agent_row();
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
        let squads = self.sqt.view == SqView::Squads;
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
            KeyCode::Char('d') if !ctrl => return self.ask_remove(false),
            KeyCode::Char('r') if !ctrl => return self.ask_remove(true),
            KeyCode::Char('e') if !ctrl && squads => return self.edit_brief(),
            _ => {}
        }
        match self.sqt.focus {
            SqFocus::List => match key.code {
                KeyCode::Left => self.show_squad_view(SqView::Squads),
                KeyCode::Right => self.show_squad_view(SqView::Agents),
                KeyCode::Up | KeyCode::Char('k') => self.move_squad(-1),
                KeyCode::Down | KeyCode::Char('j') => self.move_squad(1),
                KeyCode::PageUp => self.move_squad(-super::page_of(&self.pages.squads)),
                KeyCode::PageDown => self.move_squad(super::page_of(&self.pages.squads)),
                KeyCode::Char('g') | KeyCode::Home => self.move_squad(-(i32::MAX / 2)),
                KeyCode::Char('G') | KeyCode::End => self.move_squad(i32::MAX / 2),
                KeyCode::Enter if !squads && self.sqt.new_agent_selected() => self.open_squad_name(SquadName::NewAgent, ""),
                KeyCode::Enter => self.focus_squad_detail(),
                _ => {}
            },
            SqFocus::Detail if !squads => match key.code {
                KeyCode::Esc => self.sqt.focus = SqFocus::List,
                KeyCode::Up | KeyCode::Char('k') => self.move_squad_row(-1),
                KeyCode::Down | KeyCode::Char('j') => self.move_squad_row(1),
                KeyCode::Char('g') | KeyCode::Home => self.sqt.agent_row = 0,
                KeyCode::Char('G') | KeyCode::End => self.move_squad_row(i32::MAX / 2),
                KeyCode::Enter => self.agent_enter(),
                KeyCode::Char(' ') => self.agent_toggle(),
                _ => {}
            },
            SqFocus::Detail => match key.code {
                KeyCode::Esc => self.sqt.focus = SqFocus::List,
                KeyCode::Up | KeyCode::Char('k') => self.move_squad_row(-1),
                KeyCode::Down | KeyCode::Char('j') => self.move_squad_row(1),
                KeyCode::PageUp => self.move_squad_row(-(self.sqt.detail_size.get().1.max(2) as i32 - 1)),
                KeyCode::PageDown => self.move_squad_row(self.sqt.detail_size.get().1.max(2) as i32 - 1),
                KeyCode::Char('g') | KeyCode::Home => self.sqt.detail_top.set(0),
                KeyCode::Char('G') | KeyCode::End => self.move_squad_row(i32::MAX / 2),
                KeyCode::Enter => self.edit_brief(),
                _ => {}
            },
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
            self.notify(format!(
                "{}'s file can't be read ({why}), so there is nothing to show. Fix or move the file.",
                self.sqt.listed().map_or("", |l| l.id.as_str())
            ));
        }
    }

    // ---- the brief, in the user's editor ----------------------------------------------------------------------

    /// `e` (or enter on the brief): open the selected squad's brief in the user's editor. A built-in as platoon has it
    /// has no file yet: its text is written there first (and taken away again if the editor leaves it as it was).
    fn edit_brief(&mut self) {
        if self.sqt.draft_selected() {
            return self.notify("The draft isn't a file yet: ctrl+s saves it, then e opens it in your editor.");
        }
        let Some(l) = self.sqt.listed().cloned() else { return };
        if let Some(why) = &l.corrupt {
            return self.notify(format!(
                "{} can't be read ({why}), so platoon leaves it alone. Fix or move it to go on: {}",
                l.file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                crate::dirs::tilde(&l.file.to_string_lossy(), &self.home)
            ));
        }
        let path = squadfile::path(&self.config, &l.id);
        let mut made = false;
        if !path.exists() {
            let Some(text) = squadfile::builtin_text(&l.id) else { return };
            if let Err(e) = squadfile::save_text(&self.config, &l.id, text) {
                return self.notify(format!("Couldn't write the brief for your editor: {e}"));
            }
            made = true;
        }
        let before = std::fs::read_to_string(&path).unwrap_or_default();
        self.pick_squad_editor(SquadPromptEdit { target: PromptOf::Brief { id: l.id.clone(), made }, path, before });
    }

    /// After an editor has been and gone from a brief: read it again, and say what came of it. A built-in's brief that
    /// is word for word platoon's needs no file: it goes.
    fn after_brief_edit(&mut self, id: &str, made: bool, edit: &SquadPromptEdit) {
        let now = std::fs::read_to_string(&edit.path).unwrap_or_default();
        let name = self.squad_name(id);
        if squadfile::builtin_text(id).is_some_and(|t| t == now) {
            let _ = std::fs::remove_file(&edit.path);
            self.reload_squads(Some(id));
            return self.notify(if made {
                format!("{name} is unchanged: it is as platoon has it.")
            } else {
                format!("{name} is as platoon has it again, so it needs no file of its own.")
            });
        }
        self.reload_squads(Some(id));
        let Some(s) = self.sqt.list.iter().find(|l| l.id == id).and_then(|l| l.squad.clone()) else {
            return self.notify(format!("The {name} brief can't be read now: fix or move the file."));
        };
        let changed = now != edit.before;
        let problems = squadfile::problems(&s, &self.agent_library());
        let said = if changed { format!("Read the {} brief again", s.name) } else { format!("The {} brief is unchanged", s.name) };
        self.notify(match problems.as_slice() {
            [] => format!("{said}: it can be handed tasks."),
            [one] => format!("{said}. Fix this before it can be handed a task: {one}"),
            [first, rest @ ..] => {
                format!("{said}. It has {} things to fix before it can be handed a task; the first: {first}", rest.len() + 1)
            }
        });
    }

    // ---- changing an agent ----------------------------------------------------------------------------------

    /// The selected agent, to change: a copy of it, or None (said why) when its file can't be read, or it is one of the
    /// user's coding tool's (read only).
    fn agent_to_change(&mut self) -> Option<Agent> {
        if self.sqt.agent_read_only() {
            self.notify_read_only();
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

    fn notify_read_only(&mut self) {
        let name = self.sqt.agent().map(|a| a.name.clone()).unwrap_or_default();
        self.notify(format!(
            "{name} is an agent your coding tool has: platoon only reads it. c copies it into platoon's own to change it."
        ));
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
        if self.sqt.agent_read_only() && !matches!(row, ARow::UsedIn(_)) {
            return self.notify_read_only();
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
    /// has that aren't installed here too ("not found here": they may be a task folder's, or another computer's),
    /// and typing a name. Enter ticks or unticks one and the list stays open at `at`; esc closes it.
    fn pick_agent_tools(&mut self, mcp: bool, at: usize) {
        let Some(a) = self.sqt.agent().cloned() else { return };
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
            let away = if (if mcp { self.sqt.installed().mcp } else { self.sqt.installed().skills }).iter().any(|n| n == name) {
                ""
            } else {
                " (not found here: it has to be there where the squad works)"
            };
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
            ChoiceFor::Describe => self.describe(value == "agent"),
            ChoiceFor::AgentSkills | ChoiceFor::AgentMcp => {
                let mcp = purpose == ChoiceFor::AgentMcp;
                if value == TYPE_NAME {
                    return self.open_squad_name(if mcp { SquadName::AgentMcp } else { SquadName::AgentSkill }, "");
                }
                let at = self.sqt.choice.selected;
                self.toggle_agent_tool(mcp, &value);
                self.pick_agent_tools(mcp, at);
            }
            ChoiceFor::AgentModel => {
                let Some(mut a) = self.agent_to_change() else { return };
                a.model = value;
                let said = format!("{} runs on {label} now{}.", a.name, self.everywhere(&a.id));
                if self.commit_agent(a) {
                    self.notify(said);
                }
            }
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
                SquadName::Copy => "A squad needs a name.",
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
            SquadName::Copy => {
                let Some(s) = self.sqt.squad().cloned() else { return };
                let copy = squadfile::copy_of(&self.config, &s, &typed, &[]);
                match squadfile::save(&self.config, &copy) {
                    Ok(()) => {
                        self.reload_squads(Some(&copy.id));
                        self.notify(format!(
                            "Made \"{typed}\", a copy of {}: a brief of your own (e opens it). It uses the same agents.",
                            s.name
                        ));
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
                let from_tool = self.sqt.agent_read_only();
                let copy = if from_tool {
                    agentfile::own_copy(&self.config, &a, &typed)
                } else {
                    agentfile::copy_of(&self.config, &a, &typed, &[])
                };
                match agentfile::save(&self.config, &copy) {
                    Ok(()) => {
                        self.reload_agents(Some(&copy.id));
                        self.sqt.agent_row = 0;
                        self.notify(if from_tool && copy.id == a.id {
                            format!("Made \"{typed}\", platoon's own copy of {}: briefs that name {} use it now.", a.name, a.id)
                        } else if from_tool {
                            format!("Made \"{typed}\", platoon's own copy of {}. A brief's step uses it by its id, {}.", a.name, copy.id)
                        } else {
                            format!("Made \"{typed}\", a copy of {}. It is yours to change; no squad uses it yet.", a.name)
                        });
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
            SquadName::NewAgent => self.make_new_agent(&typed),
        }
    }

    /// A new, blank agent of the user's own, saved at once and shown on the Agents list to be filled in.
    fn make_new_agent(&mut self, name: &str) {
        let taken: Vec<String> = self.sqt.draft_agents.iter().map(|a| a.id.clone()).collect();
        let a = agentfile::blank(&self.config, name, &taken);
        if let Err(e) = agentfile::save(&self.config, &a) {
            return self.notify(format!("Couldn't save the agent: {e}"));
        }
        self.reload_agents(Some(&a.id));
        if let Some(at) = self.sqt.agents.iter().position(|l| l.id == a.id && l.kind != Kind::Claude) {
            self.open_agent(at);
            if let Some(pos) = self.agent_detail_rows().iter().position(|r| *r == ARow::Prompt) {
                self.sqt.agent_row = pos;
            }
        }
        self.notify(format!(
            "Made the agent \"{}\". It needs a system prompt before a squad can use it: enter on System prompt writes one. A brief's step names it as {}.",
            a.name, a.id
        ));
    }

    /// Show agent `at` of the list, its details with the keys.
    fn open_agent(&mut self, at: usize) {
        self.sqt.view = SqView::Agents;
        self.sqt.agent_selected = at;
        self.sqt.agent_row = 0;
        self.sqt.agent_top.set(0);
        self.sqt.focus = if self.sqt.agent().is_some() { SqFocus::Detail } else { SqFocus::List };
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

    /// Enter in the text box: the description goes to Claude to draft.
    fn take_squad_text(&mut self) {
        let typed = self.sqt.text.text.split_whitespace().collect::<Vec<_>>().join(" ");
        let purpose = self.sqt.text_for.clone();
        if typed.is_empty() {
            return self.notify(match purpose {
                TextFor::Describe => "Describe the squad first: what it is for, and the work it does.",
                TextFor::DescribeAgent => "Describe the agent first: the one job it does.",
            });
        }
        self.mode = Mode::List;
        match purpose {
            TextFor::Describe => {
                self.sqt.last_description = typed.clone();
                self.draft_squad(&typed)
            }
            TextFor::DescribeAgent => {
                self.sqt.last_agent_description = typed.clone();
                self.draft_agent(&typed)
            }
        }
    }

    // ---- system prompts, in the editors ----------------------------------------------------------------------

    /// Enter on an agent's system prompt.
    fn edit_agent_prompt(&mut self) {
        let Some(a) = self.sqt.agent().cloned() else { return };
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

    /// After an editor has been and gone from a system prompt or a brief: keep what it says now.
    pub(super) fn after_squad_prompt_edit(&mut self, edit: &SquadPromptEdit) {
        match &edit.target {
            PromptOf::DraftTask(i) => return self.after_draft_edit(*i, edit),
            PromptOf::Brief { id, made } => return self.after_brief_edit(&id.clone(), *made, edit),
            PromptOf::Agent { .. } => {}
        }
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
        let PromptOf::Agent { id, draft } = &edit.target else { return };
        let a = if *draft {
            self.sqt.agent_draft.clone().filter(|d| d.id == *id)
        } else {
            self.sqt.agents.iter().find(|l| l.id == *id && l.kind != Kind::Claude).and_then(|l| l.agent.clone())
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
            _ => format!("Saved {}'s system prompt{}, from the next task each is handed.", a.name, self.everywhere(id)),
        };
        match agentfile::save(&self.config, &a) {
            Ok(()) => {
                self.reload_agents(None);
                self.notify(said);
            }
            Err(e) => self.notify(format!("Couldn't save the agent: {e}")),
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

    /// One `claude -p` on Sonnet drafts the whole brief (told the agents there are, to reuse), off the UI thread. If
    /// the gate says no there is no call.
    fn draft_squad(&mut self, description: &str) {
        let settings = self.settings.get();
        if let Err(r) = ai::gate_for(&settings, squadfile::DRAFT_FEATURE, &self.cache, self.clock(), true) {
            return self.notify(format!("Couldn't draft a squad: {}. {}", crate::squads::refusal_words(&r), r.on_screen()));
        }
        self.sqt.reread_installed();
        let prompt = squadfile::draft_prompt(description, &self.agent_library(), &self.sqt.installed());
        let mut req = Request::new(
            squadfile::DRAFT_FEATURE,
            squadfile::draft_system(&instructions::guidance(&self.config, instructions::SQUAD_DRAFT)),
            prompt,
        );
        req.user_initiated = true;
        req.model = Some(squadfile::DRAFT_MODEL);
        req.timeout = DRAFT_TIMEOUT;
        req.cancel = self.switchboard.token(squadfile::DRAFT_FEATURE);
        self.sqt.drafting = Some((Drafting::Squad, self.spawn_call(settings, req)));
        self.notify("Claude is drafting the brief. It shows here as a draft when it is done.");
    }

    /// One `claude -p` on Sonnet drafts an agent, off the UI thread.
    fn draft_agent(&mut self, description: &str) {
        let settings = self.settings.get();
        if let Err(r) = ai::gate_for(&settings, agentfile::DRAFT_FEATURE, &self.cache, self.clock(), true) {
            return self.notify(format!("Couldn't draft an agent: {}. {}", crate::squads::refusal_words(&r), r.on_screen()));
        }
        let mut req = Request::new(
            agentfile::DRAFT_FEATURE,
            agentfile::draft_system(&instructions::guidance(&self.config, instructions::AGENT_DRAFT)),
            agentfile::draft_prompt(description, &{
                self.sqt.reread_installed();
                self.sqt.installed()
            }),
        );
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
                self.sqt.detail_top.set(0);
                let agents = match new {
                    0 => "It uses agents you have already.".to_string(),
                    1 => "It brings 1 new agent, saved with it.".to_string(),
                    n => format!("It brings {n} new agents, saved with it."),
                };
                self.notify(format!(
                    "Here is a draft of the \"{name}\" brief. {agents} ctrl+s saves it (then e opens it in your editor), d discards it."
                ));
            }
            Err(why) => self.notify(format!("Claude's draft couldn't be used: {why} Press n to try again.")),
        }
    }

    /// ctrl+s: save the draft the list shows (the brief with the agents it brings, or the agent), once nothing is
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
                self.notify(format!("Saved the \"{}\" brief{agents}. Tasks can be handed to it now; e opens it in your editor.", s.name));
            }
            Err(e) => self.notify(format!("Couldn't save the brief: {e}")),
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
                self.notify(format!("Saved the agent \"{}\". A brief's step uses it as {}.", a.name, a.id));
            }
            Err(e) => self.notify(format!("Couldn't save the agent: {e}")),
        }
    }

    /// `c`: copy the selected squad or agent under a new name (an agent of the user's coding tool: into platoon's own,
    /// under its own name, so it takes the other's place).
    fn ask_copy(&mut self) {
        if self.sqt.view == SqView::Agents {
            if self.sqt.agent_draft_selected() {
                return self.notify("Save the draft first (ctrl+s), then copy it.");
            }
            if self.sqt.new_agent_selected() {
                return;
            }
            let Some(a) = self.sqt.agent().cloned() else {
                let _ = self.agent_to_change();
                return;
            };
            let name = if self.sqt.agent_read_only() { a.name.clone() } else { format!("{} copy", a.name) };
            return self.open_squad_name(SquadName::AgentCopy, &name);
        }
        if self.sqt.draft_selected() {
            return self.notify("Save the draft first (ctrl+s), then copy it.");
        }
        let Some(s) = self.sqt.squad().cloned() else {
            if let Some(l) = self.sqt.listed() {
                let msg = format!("{}'s file can't be read, so platoon leaves it alone. Fix or move the file.", l.id);
                self.notify(msg);
            }
            return;
        };
        self.open_squad_name(SquadName::Copy, &format!("{} copy", s.name));
    }

    /// `d` (`restore` false) and `r` (true): discard a draft, delete one of the user's own, or put a changed built-in
    /// back as platoon has it, each after asking (`d` does both of the last; `r` only restores). A built-in as platoon
    /// has it can't be deleted, nor can an agent a squad uses or one of the user's coding tool's.
    fn ask_remove(&mut self, restore: bool) {
        if self.sqt.view == SqView::Agents {
            return self.ask_remove_agent(restore);
        }
        if self.sqt.draft_selected() {
            if restore {
                return self.notify("A draft has nothing to restore: d discards it.");
            }
            let name = self.sqt.draft.as_ref().map(|d| d.name.clone()).unwrap_or_default();
            self.mode = Mode::Confirm(Confirm::DiscardDraft { name });
            return;
        }
        let Some(l) = self.sqt.listed().cloned() else { return };
        let name = l.name().to_string();
        self.mode = match l.kind {
            Kind::Own if restore => return self.notify(format!("{name} is your own, not one platoon has: d deletes it.")),
            Kind::Own => Mode::Confirm(Confirm::DeleteSquad { id: l.id, name }),
            Kind::Modified => Mode::Confirm(Confirm::RestoreSquad { id: l.id, name }),
            Kind::BuiltIn | Kind::Claude => {
                return self.notify(format!(
                    "{name} is built into platoon, so it can't be deleted. It is as platoon has it already; c makes a copy of your own."
                ));
            }
        };
    }

    fn ask_remove_agent(&mut self, restore: bool) {
        if self.sqt.agent_draft_selected() {
            if restore {
                return self.notify("A draft has nothing to restore: d discards it.");
            }
            let name = self.sqt.agent_draft.as_ref().map(|d| d.name.clone()).unwrap_or_default();
            self.mode = Mode::Confirm(Confirm::DiscardAgentDraft { name });
            return;
        }
        let Some(l) = self.sqt.agent_listed().cloned() else { return };
        let name = l.name().to_string();
        self.mode = match l.kind {
            Kind::Own if restore => return self.notify(format!("{name} is your own, not one platoon has: d deletes it.")),
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
            Kind::Claude => {
                return self.notify(format!("{name} is an agent your coding tool has: platoon never changes or deletes it."));
            }
            Kind::BuiltIn => {
                return self.notify(format!(
                    "{name} is built into platoon, so it can't be deleted. It is as platoon has it already; c makes a copy of your own."
                ));
            }
        };
    }

    pub(super) fn remove_squad(&mut self, id: &str, restore: bool) {
        self.mode = Mode::List;
        let name = self.squad_name(id);
        let old = self.sqt.list.iter().find(|l| l.id == id).is_some_and(|l| l.file == squadfile::old_path(&self.config, id));
        if old {
            return match squadfile::remove(&self.config, id) {
                Ok(()) => {
                    self.reload_squads(None);
                    self.sqt.focus = SqFocus::List;
                    self.notify(format!("Put the old file of \"{name}\" aside (kept as {id}.json.bak in the squads folder)."));
                }
                Err(e) => self.notify(format!("Couldn't change the old file: {e}")),
            };
        }
        match squadfile::remove(&self.config, id) {
            Ok(()) => {
                self.reload_squads(if restore { Some(id) } else { None });
                // Restored: its name is platoon's again.
                let name = if restore { self.squad_name(id) } else { name };
                if !restore {
                    self.sqt.selected = self.sqt.selected.min(self.sqt.len().saturating_sub(1));
                }
                self.sqt.focus = SqFocus::List;
                self.notify(if restore {
                    format!("{name} is as platoon has it again. Your brief is kept as {id}.md.bak in the briefs folder.")
                } else {
                    format!("Deleted the \"{name}\" brief (kept as {id}.md.bak in the briefs folder). Tasks it already has keep it.")
                });
            }
            Err(e) => self.notify(format!("Couldn't change the brief's file: {e}")),
        }
    }

    /// Yes to deleting one of the user's own agents, or to putting a changed built-in agent back.
    pub(super) fn remove_agent(&mut self, id: &str, restore: bool) {
        self.mode = Mode::List;
        let name = self.sqt.agents.iter().find(|l| l.id == id).map_or_else(|| id.to_string(), |l| l.name().to_string());
        let done = if restore {
            agentfile::remove(&self.config, id).map_err(|e| format!("Couldn't change the agent's file: {e}"))
        } else {
            agentfile::delete(&self.config, id)
        };
        match done {
            Ok(()) => {
                self.reload_agents(if restore { Some(id) } else { None });
                self.sqt.focus = SqFocus::List;
                self.notify(if restore {
                    format!(
                        "{name} is as platoon has it again{}. Your version is kept as {id}.json.bak in the agents folder.",
                        self.everywhere(id)
                    )
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

/// What is open in an editor through the Squads tab's plumbing.
#[derive(Clone, Debug)]
pub enum PromptOf {
    /// An agent's system prompt (`draft`: the unsaved draft agent), in a file of its own.
    Agent { id: String, draft: bool },
    /// A squad's brief, its own file (`made`: platoon wrote a built-in's text there for the editor).
    Brief { id: String, made: bool },
    /// Not a system prompt: a task of the draft made from a conversation (its place in the draft), in the same
    /// editors (`app/taskdraft_glue.rs`).
    DraftTask(usize),
}

impl SquadPromptEdit {
    /// It is a drafted task, not a system prompt.
    pub fn is_draft(&self) -> bool {
        matches!(self.target, PromptOf::DraftTask(_))
    }

    /// Its file is platoon's to throw away when the editor can't be started: a file written for the editor (a
    /// system prompt's, a draft's task, a built-in's brief platoon just wrote), never a brief of the user's.
    pub fn disposable(&self) -> bool {
        !matches!(self.target, PromptOf::Brief { made: false, .. })
    }

    /// What is in the editor, for the words around it: "the system prompt", "the task", "the brief".
    pub fn what(&self) -> &'static str {
        match self.target {
            PromptOf::DraftTask(_) => "the task",
            PromptOf::Brief { .. } => "the brief",
            PromptOf::Agent { .. } => "the system prompt",
        }
    }
}

/// A system prompt (or a brief) open in an editor: a system prompt is written to a file of its own and read back into
/// the agent; a brief is its own file, read again.
#[derive(Clone, Debug)]
pub struct SquadPromptEdit {
    pub target: PromptOf,
    pub path: PathBuf,
    pub before: String,
}
