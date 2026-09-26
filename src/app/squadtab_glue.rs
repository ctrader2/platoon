//! The Squads tab (`3`): every squad on the left (built in, changed by the user, or the user's own, and an unsaved
//! draft), the selected one on the right in four sections (About, Leader, Members, Workflow), and everything the user
//! can change there: names, what a squad can do, models, the three switches, system prompts (in the same editors as
//! the notes), members, steps and their order, and when a step may send the work back. `n` describes a new squad
//! for Claude to draft (one `claude -p` on Sonnet, off the UI thread), `c` copies one, `d` deletes the user's own or
//! puts a built-in back as platoon has it. The definitions and the rules are in `squadfile.rs`.

use super::*;
use crate::ai::{AiError, Reply, Request};
use crate::instructions;
use crate::squadfile::{self, Back, Kind, Listed, Squad, Step as WStep};
use std::sync::mpsc::TryRecvError;

/// How long drafting a squad may take before it is given up on.
const DRAFT_TIMEOUT: Duration = Duration::from_secs(180);

/// Which of the tab's two panes has the keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SqFocus {
    List,
    Detail,
}

/// A row of the selected squad's details: the things that can be changed, in the order they are shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DRow {
    Name,
    CanDo,
    LeaderModel,
    LeaderPrompt,
    MemberName(usize),
    MemberModel(usize),
    MemberFiles(usize),
    MemberCommands(usize),
    MemberWeb(usize),
    MemberPrompt(usize),
    AddMember,
    Step(usize),
    AddStep,
}

impl DRow {
    /// The section it is in: 0 About, 1 Leader, 2 Members, 3 Workflow.
    pub fn section(self) -> usize {
        match self {
            DRow::Name | DRow::CanDo => 0,
            DRow::LeaderModel | DRow::LeaderPrompt => 1,
            DRow::Step(_) | DRow::AddStep => 3,
            _ => 2,
        }
    }
}

/// The rows of a squad's details.
pub fn detail_rows(s: &Squad) -> Vec<DRow> {
    let mut out = vec![DRow::Name, DRow::CanDo, DRow::LeaderModel, DRow::LeaderPrompt];
    for i in 0..s.members.len() {
        out.extend([DRow::MemberName(i), DRow::MemberModel(i), DRow::MemberFiles(i), DRow::MemberCommands(i), DRow::MemberWeb(i), DRow::MemberPrompt(i)]);
    }
    out.push(DRow::AddMember);
    out.extend((0..s.workflow.len()).map(DRow::Step));
    out.push(DRow::AddStep);
    out
}

/// What a short line typed in the name box is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SquadName {
    /// The squad's name.
    Name,
    /// A new name for member `i`.
    Member(usize),
    NewMember,
    /// The name of a copy of the selected squad.
    Copy,
}

/// What the text box is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextFor {
    /// The description of a new squad, for Claude to draft.
    Describe,
    CanDo,
    /// What step `i` does.
    StepDoes(usize),
    /// What a new step, done by this member, does.
    NewStep(String),
}

/// What the small list open over the tab is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChoiceFor {
    LeaderModel,
    MemberModel(usize),
    /// Who does step `i`.
    StepMember(usize),
    /// Who does a new step.
    NewStepMember,
    /// Where step `i` may send the work back to (or not at all).
    BackTo(usize),
    /// How many times step `i` may send the work back to step `to`.
    BackTimes(usize, usize),
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

/// Everything the Squads tab keeps.
pub struct SquadTab {
    /// The squads, as their files and platoon say (`squadfile::load_all`), re-read when a file changes.
    pub list: Vec<Listed>,
    stamp: Vec<(String, Option<(std::time::SystemTime, u64)>)>,
    /// A drafted squad not saved yet: listed last.
    pub draft: Option<Squad>,
    /// The selected entry: an index into `list`, or `list.len()` for the draft.
    pub selected: usize,
    pub focus: SqFocus,
    /// The selected row of the details (an index into `detail_rows`).
    pub row: usize,
    /// The first line of the details in view, kept between frames so they only scroll when they have to.
    pub detail_top: Cell<usize>,
    pub choice: Choice,
    pub text: TextArea,
    pub text_for: TextFor,
    /// The drafting call, while it is out.
    drafting: Option<Receiver<Result<Reply, AiError>>>,
    /// The last description the user gave, offered again by the next `n`.
    pub last_description: String,
}

impl SquadTab {
    pub fn new(config: &Path) -> Self {
        SquadTab {
            list: squadfile::load_all(config),
            stamp: squadfile::stamp(config),
            draft: None,
            selected: 0,
            focus: SqFocus::List,
            row: 0,
            detail_top: Cell::new(0),
            choice: Choice { purpose: ChoiceFor::LeaderModel, title: String::new(), options: Vec::new(), selected: 0 },
            text: TextArea::new("", 0),
            text_for: TextFor::Describe,
            drafting: None,
            last_description: String::new(),
        }
    }

    /// How many entries the list has (the draft counts).
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

    pub fn drafting(&self) -> bool {
        self.drafting.is_some()
    }

    /// The selected row of the details.
    pub fn drow(&self) -> Option<DRow> {
        self.squad().and_then(|s| detail_rows(s).get(self.row).copied())
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

impl App {
    // ---- the list ---------------------------------------------------------------------------------------------

    /// Every squad (built-ins first, then the user's own), as the files say now.
    pub fn squad_list(&self) -> &[Listed] {
        &self.sqt.list
    }

    /// A squad's name, by its id (the id itself when there is no such squad now).
    pub fn squad_name(&self, id: &str) -> String {
        self.sqt.list.iter().find(|l| l.id == id).map_or_else(|| id.to_string(), |l| l.name().to_string())
    }

    /// Read the squads again when their files changed (one listing of the folder when nothing did).
    pub(super) fn poll_squad_files(&mut self) {
        let stamp = squadfile::stamp(&self.config);
        if stamp != self.sqt.stamp {
            self.reload_squads(None);
        }
    }

    /// Read the squads again, keeping the selection on the same squad (or on `select`, an id).
    fn reload_squads(&mut self, select: Option<&str>) {
        let keep = select.map(str::to_string).or_else(|| self.sqt.listed().map(|l| l.id.clone()));
        let draft = self.sqt.draft_selected();
        self.sqt.list = squadfile::load_all(&self.config);
        self.sqt.stamp = squadfile::stamp(&self.config);
        self.sqt.selected = if draft && select.is_none() {
            self.sqt.list.len()
        } else {
            keep.and_then(|id| self.sqt.list.iter().position(|l| l.id == id)).unwrap_or(0)
        };
        self.clamp_squad_row();
    }

    fn clamp_squad_row(&mut self) {
        self.sqt.selected = self.sqt.selected.min(self.sqt.len().saturating_sub(1));
        let n = self.sqt.squad().map_or(0, |s| detail_rows(s).len());
        self.sqt.row = self.sqt.row.min(n.saturating_sub(1));
    }

    fn move_squad(&mut self, delta: i32) {
        let last = self.sqt.len().saturating_sub(1) as i32;
        let to = (self.sqt.selected as i32 + delta).clamp(0, last) as usize;
        if to != self.sqt.selected {
            self.sqt.selected = to;
            self.sqt.row = 0;
            self.sqt.detail_top.set(0);
        }
    }

    fn move_squad_row(&mut self, delta: i32) {
        let n = self.sqt.squad().map_or(0, |s| detail_rows(s).len());
        self.sqt.row = (self.sqt.row as i32 + delta).clamp(0, n.saturating_sub(1) as i32) as usize;
    }

    /// A click on the list's entry `i`: selects it (the list has the keys).
    pub(super) fn click_squad(&mut self, i: usize) {
        if i < self.sqt.len() {
            if i != self.sqt.selected {
                self.sqt.row = 0;
                self.sqt.detail_top.set(0);
            }
            self.sqt.selected = i;
            self.sqt.focus = SqFocus::List;
        }
    }

    /// A click on row `i` of the details: selects it (the details have the keys); on the selected one, Enter.
    pub(super) fn click_squad_row(&mut self, i: usize) {
        if self.sqt.focus == SqFocus::Detail && self.sqt.row == i {
            return self.squad_enter();
        }
        self.sqt.focus = SqFocus::Detail;
        self.sqt.row = i;
        self.clamp_squad_row();
    }

    pub(super) fn wheel_squads(&mut self, up: bool) {
        let d = if up { -1 } else { 1 };
        match self.sqt.focus {
            SqFocus::List => self.move_squad(d),
            SqFocus::Detail => self.move_squad_row(d),
        }
    }

    // ---- keys -------------------------------------------------------------------------------------------------

    /// Keys on the Squads tab. Plain arrows move inside a pane; Shift or Ctrl with ← and → (and Tab) move between
    /// the list and the details, as on a project's screen.
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
            KeyCode::Char('n') if !ctrl => return self.describe_squad(),
            KeyCode::Char('c') if !ctrl => return self.ask_copy_squad(),
            KeyCode::Char('d') if !ctrl => return self.ask_remove_squad(),
            _ => {}
        }
        match self.sqt.focus {
            SqFocus::List => match key.code {
                KeyCode::Up => self.move_squad(-1),
                KeyCode::Down => self.move_squad(1),
                KeyCode::Char('g') | KeyCode::Home => self.move_squad(-(self.sqt.len() as i32)),
                KeyCode::Char('G') | KeyCode::End => self.move_squad(self.sqt.len() as i32),
                KeyCode::Enter => self.focus_squad_detail(),
                _ => {}
            },
            SqFocus::Detail => match key.code {
                KeyCode::Esc => self.sqt.focus = SqFocus::List,
                KeyCode::Up => self.move_squad_row(-1),
                KeyCode::Down => self.move_squad_row(1),
                KeyCode::Char('g') | KeyCode::Home => self.sqt.row = 0,
                KeyCode::Char('G') | KeyCode::End => self.move_squad_row(i32::MAX / 2),
                KeyCode::Enter => self.squad_enter(),
                KeyCode::Char(' ') => self.squad_toggle(),
                KeyCode::Char('a') => self.squad_add(),
                KeyCode::Char('x') => self.squad_remove_row(),
                KeyCode::Char('m') => self.pick_step_member(),
                KeyCode::Char('b') => self.pick_send_back(),
                KeyCode::Char('[') => self.move_step(true),
                KeyCode::Char(']') => self.move_step(false),
                _ => {}
            },
        }
    }

    fn focus_squad_detail(&mut self) {
        if self.sqt.squad().is_some() {
            self.sqt.focus = SqFocus::Detail;
            self.clamp_squad_row();
        } else if let Some(why) = self.sqt.listed().and_then(|l| l.corrupt.clone()) {
            self.notify(format!("{}'s file can't be read ({why}), so there is nothing to change. Fix or move the file.", self.sqt.listed().map_or("", |l| l.id.as_str())));
        }
    }

    // ---- changing a squad -----------------------------------------------------------------------------------

    /// The selected squad, to change: a copy of it, or None (said why) when its file can't be read.
    fn squad_to_change(&mut self) -> Option<Squad> {
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
        let Some(s) = self.sqt.squad().cloned() else { return };
        let Some(row) = self.sqt.drow() else { return };
        match row {
            DRow::Name => self.open_squad_name(SquadName::Name, &s.name),
            DRow::CanDo => self.open_squad_text(TextFor::CanDo, &s.can_do, squadfile::CAN_DO_MAX),
            DRow::LeaderModel => self.open_choice(ChoiceFor::LeaderModel, "Model for the leader", model_options(), &s.leader.model),
            DRow::MemberModel(i) => {
                let (name, model) = (s.members[i].name.clone(), s.members[i].model.clone());
                self.open_choice(ChoiceFor::MemberModel(i), &format!("Model for {name}"), model_options(), &model);
            }
            DRow::MemberName(i) => self.open_squad_name(SquadName::Member(i), &s.members[i].name),
            DRow::MemberFiles(_) | DRow::MemberCommands(_) | DRow::MemberWeb(_) => self.squad_toggle(),
            DRow::LeaderPrompt => self.edit_squad_prompt(None),
            DRow::MemberPrompt(i) => self.edit_squad_prompt(Some(s.members[i].name.clone())),
            DRow::AddMember => self.squad_add(),
            DRow::Step(i) => self.open_squad_text(TextFor::StepDoes(i), &s.workflow[i].does, squadfile::DOES_MAX),
            DRow::AddStep => self.squad_add(),
        }
    }

    /// Space (or Enter) on a switch: turn it on or off.
    fn squad_toggle(&mut self) {
        let Some(row) = self.sqt.drow() else { return };
        let (DRow::MemberFiles(i) | DRow::MemberCommands(i) | DRow::MemberWeb(i)) = row else { return };
        let Some(mut s) = self.squad_to_change() else { return };
        let m = &mut s.members[i];
        let (what, now) = match row {
            DRow::MemberFiles(_) => {
                m.files = !m.files;
                ("change files", m.files)
            }
            DRow::MemberCommands(_) => {
                m.commands = !m.commands;
                ("run commands", m.commands)
            }
            _ => {
                m.web = !m.web;
                ("use the web", m.web)
            }
        };
        let name = m.name.clone();
        if self.commit_squad(s) {
            self.notify(format!("{name} {} {what} now.", if now { "can" } else { "can't" }));
        }
    }

    /// `a`: a new member (in About, Leader or Members) or a new step (in Workflow).
    fn squad_add(&mut self) {
        let Some(s) = self.squad_to_change() else { return };
        let in_workflow = self.sqt.drow().is_some_and(|r| r.section() == 3);
        if in_workflow {
            if s.workflow.len() >= squadfile::MAX_STEPS {
                return self.notify(format!("A workflow has at most {} steps.", squadfile::MAX_STEPS));
            }
            if s.members.is_empty() {
                return self.notify("Add a member first: a step is done by one of them.");
            }
            let options = s.members.iter().map(|m| (m.name.clone(), m.name.clone())).collect();
            self.open_choice(ChoiceFor::NewStepMember, "Who does the new step", options, "");
        } else {
            if s.members.len() >= squadfile::MAX_MEMBERS {
                return self.notify(format!("A squad has at most {} members.", squadfile::MAX_MEMBERS));
            }
            self.open_squad_name(SquadName::NewMember, "");
        }
    }

    /// `x`: take out the member or the step the cursor is on. A member a step uses stays, and platoon says which step.
    fn squad_remove_row(&mut self) {
        let Some(row) = self.sqt.drow() else { return };
        let Some(mut s) = self.squad_to_change() else { return };
        match row {
            DRow::MemberName(i) | DRow::MemberModel(i) | DRow::MemberFiles(i) | DRow::MemberCommands(i) | DRow::MemberWeb(i) | DRow::MemberPrompt(i) => {
                let name = s.members[i].name.clone();
                match s.remove_member(i) {
                    Err(why) => self.notify(why),
                    Ok(()) => {
                        if self.commit_squad(s) {
                            self.notify(format!("Took {name} out of the squad."));
                        }
                    }
                }
            }
            DRow::Step(i) => {
                s.remove_step(i);
                if self.commit_squad(s) {
                    self.notify(format!("Took step {} out of the workflow.", i + 1));
                }
            }
            _ => {}
        }
    }

    /// `[` and `]` on a step: move it up or down the workflow.
    fn move_step(&mut self, up: bool) {
        let Some(DRow::Step(i)) = self.sqt.drow() else { return };
        let Some(mut s) = self.squad_to_change() else { return };
        let (to, cleared) = s.move_step(i, up);
        if to == i {
            return;
        }
        if self.commit_squad(s) {
            if let Some(pos) = self.sqt.squad().and_then(|s| detail_rows(s).iter().position(|r| *r == DRow::Step(to))) {
                self.sqt.row = pos;
            }
            let more = if cleared { " A step that sent the work back to a step no longer before it doesn't any more." } else { "" };
            self.notify(format!("Step {} is step {} now.{more}", i + 1, to + 1));
        }
    }

    /// `m` on a step: who does it.
    fn pick_step_member(&mut self) {
        let Some(DRow::Step(i)) = self.sqt.drow() else { return };
        let Some(s) = self.sqt.squad().cloned() else { return };
        let options = s.members.iter().map(|m| (m.name.clone(), m.name.clone())).collect();
        self.open_choice(ChoiceFor::StepMember(i), &format!("Who does step {}", i + 1), options, &s.workflow[i].member);
    }

    /// `b` on a step: may it send the work back, and to which earlier step.
    fn pick_send_back(&mut self) {
        let Some(DRow::Step(i)) = self.sqt.drow() else { return };
        let Some(s) = self.sqt.squad().cloned() else { return };
        if i == 0 {
            return self.notify("The first step has no step before it to send the work back to.");
        }
        let mut options = vec![("Never sends the work back".to_string(), String::new())];
        for (k, st) in s.workflow.iter().enumerate().take(i) {
            options.push((format!("Back to step {}: {} ({})", k + 1, st.member, st.does), k.to_string()));
        }
        let now = s.workflow[i].back.map(|b| b.to.to_string()).unwrap_or_default();
        self.open_choice(ChoiceFor::BackTo(i), &format!("Where step {} may send the work back to", i + 1), options, &now);
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
        if purpose == ChoiceFor::NewStepMember {
            return self.open_squad_text(TextFor::NewStep(value), "", squadfile::DOES_MAX);
        }
        if let ChoiceFor::BackTo(i) = purpose {
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
        let Some(mut s) = self.squad_to_change() else { return };
        let said = match purpose {
            ChoiceFor::LeaderModel => {
                s.leader.model = value;
                format!("The leader runs on {label} now.")
            }
            ChoiceFor::MemberModel(i) => {
                s.members[i].model = value;
                format!("{} runs on {label} now.", s.members[i].name)
            }
            ChoiceFor::StepMember(i) => {
                s.workflow[i].member = value.clone();
                format!("{value} does step {} now.", i + 1)
            }
            ChoiceFor::BackTimes(i, to) => {
                let times: u32 = value.parse().unwrap_or(1);
                s.workflow[i].back = Some(Back { to, times });
                format!("Step {} {} now.", i + 1, back_words(&Back { to, times }))
            }
            ChoiceFor::NewStepMember | ChoiceFor::BackTo(_) => return,
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
            SquadName::Name | SquadName::Copy => squadfile::NAME_MAX,
            SquadName::Member(_) | SquadName::NewMember => squadfile::SLUG_MAX,
        }
    }

    /// Enter in the name box, for a squad. The box stays open while what is typed can't be used.
    pub(super) fn save_squad_name(&mut self, purpose: SquadName) {
        let typed = self.name_box.input.split_whitespace().collect::<Vec<_>>().join(" ");
        match purpose {
            SquadName::Name | SquadName::Copy if typed.is_empty() => self.notify("A squad needs a name."),
            SquadName::Name => {
                self.mode = Mode::List;
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
                self.mode = Mode::List;
                let Some(s) = self.sqt.squad().cloned() else { return };
                let copy = squadfile::copy_of(&self.config, &s, &typed, &[]);
                match squadfile::save(&self.config, &copy) {
                    Ok(()) => {
                        self.reload_squads(Some(&copy.id));
                        self.sqt.row = 0;
                        self.notify(format!("Made \"{typed}\", a copy of {}. It is yours to change.", s.name));
                    }
                    Err(e) => self.notify(format!("Couldn't save the copy: {e}")),
                }
            }
            SquadName::Member(i) => {
                let Some(mut s) = self.squad_to_change() else { return };
                let old = s.members.get(i).map(|m| m.name.clone()).unwrap_or_default();
                if old == typed {
                    self.mode = Mode::List;
                    return;
                }
                if let Err(why) = s.rename_member(i, &typed) {
                    return self.notify(why);
                }
                self.mode = Mode::List;
                if self.commit_squad(s) {
                    self.notify(format!("Renamed {old} to {typed}."));
                }
            }
            SquadName::NewMember => {
                let Some(mut s) = self.squad_to_change() else { return };
                if let Some(why) = squadfile::member_name_problem(&typed) {
                    return self.notify(why);
                }
                if s.member(&typed).is_some() {
                    return self.notify(format!("The squad already has a member called {typed}."));
                }
                self.mode = Mode::List;
                s.members.push(squadfile::new_member(&typed));
                let i = s.members.len() - 1;
                if self.commit_squad(s) {
                    if let Some(pos) = self.sqt.squad().and_then(|s| detail_rows(s).iter().position(|r| *r == DRow::MemberName(i))) {
                        self.sqt.row = pos;
                    }
                    self.notify(format!("Added {typed}. It can only read until you switch more on; give it a step in the workflow."));
                }
            }
        }
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
                TextFor::Describe => "Describe the squad first: what it is for, and who is in it.",
                TextFor::CanDo => "Say what the squad can do: the commander reads it before a ticket is handed over.",
                _ => "Say what the step does.",
            };
            return self.notify(why);
        }
        self.mode = Mode::List;
        if purpose == TextFor::Describe {
            self.sqt.last_description = typed.clone();
            return self.draft_squad(&typed);
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
            TextFor::NewStep(member) => {
                s.workflow.push(WStep { member: member.clone(), does: typed, back: None, extra: Default::default() });
                format!("Added step {}, done by {member}. b says whether it may send the work back.", s.workflow.len())
            }
            TextFor::Describe => return,
        };
        let new_step = matches!(self.sqt.text_for, TextFor::NewStep(_));
        if self.commit_squad(s) {
            if new_step {
                if let Some(pos) = self.sqt.squad().map(|s| detail_rows(s).len().saturating_sub(2)) {
                    self.sqt.row = pos;
                }
            }
            self.notify(said);
        }
    }

    // ---- system prompts, in the editors ----------------------------------------------------------------------

    /// Enter on a system prompt: the list of editors, as for the notes. `member`: None for the leader's.
    fn edit_squad_prompt(&mut self, member: Option<String>) {
        let Some(s) = self.sqt.squad().cloned() else { return };
        let guidance = match &member {
            Some(m) => s.member(m).map(|m| m.guidance.clone()).unwrap_or_default(),
            None => s.leader.guidance.clone(),
        };
        let role = member.clone().unwrap_or_else(|| squadfile::LEADER_ROLE.to_string());
        let path = self.cache.join("squad-prompts").join(format!("{}-{role}.md", s.id));
        if let Err(e) = crate::projects::write_atomic(&path, &format!("{}\n", guidance.trim())) {
            return self.notify(format!("Couldn't start the system prompt file: {e}"));
        }
        let edit = SquadPromptEdit { squad: s.id.clone(), draft: self.sqt.draft_selected(), member, path, before: guidance.trim().to_string() };
        self.pick_squad_editor(edit);
    }

    /// After an editor has been and gone from a squad's system prompt: keep what it says now.
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
        let who = edit.member.clone().unwrap_or_else(|| "the leader".into());
        if text.is_empty() {
            return self.notify(format!("A system prompt can't be empty, so {who}'s is as it was."));
        }
        // The squad as it is now (the draft, or its file), found again by its id.
        let s = if edit.draft {
            self.sqt.draft.clone().filter(|d| d.id == edit.squad)
        } else {
            self.sqt.list.iter().find(|l| l.id == edit.squad).and_then(|l| l.squad.clone())
        };
        let Some(mut s) = s else {
            return self.notify("The squad isn't there any more, so the system prompt wasn't kept.");
        };
        match &edit.member {
            Some(m) => match s.members.iter_mut().find(|x| x.name == *m) {
                Some(x) => x.guidance = text,
                None => return self.notify(format!("{m} isn't in the squad any more, so the system prompt wasn't kept.")),
            },
            None => s.leader.guidance = text,
        }
        if edit.draft {
            self.sqt.draft = Some(s);
            self.notify(format!("Changed {who}'s system prompt in the draft."));
            return;
        }
        match squadfile::save(&self.config, &s) {
            Ok(()) => {
                self.reload_squads(None);
                self.notify(format!("Saved {who}'s system prompt. The squad uses it from the next ticket it is handed."));
            }
            Err(e) => self.notify(format!("Couldn't save the squad: {e}")),
        }
    }

    // ---- making, copying, deleting ---------------------------------------------------------------------------

    /// `n`: describe a new squad, for Claude to draft.
    fn describe_squad(&mut self) {
        if self.sqt.drafting() {
            return self.notify("Claude is drafting a squad already. It shows here when it is done.");
        }
        if self.sqt.draft.is_some() {
            self.sqt.selected = self.sqt.list.len();
            return self.notify("There is a draft already: ctrl+s saves it, d discards it.");
        }
        let last = self.sqt.last_description.clone();
        self.open_squad_text(TextFor::Describe, &last, squadfile::DESCRIPTION_MAX);
    }

    /// One `claude -p` on Sonnet drafts the whole squad, off the UI thread. If the gate says no there is no call.
    fn draft_squad(&mut self, description: &str) {
        let settings = self.settings.get();
        if let Err(r) = ai::gate_for(&settings, squadfile::DRAFT_FEATURE, &self.cache, self.clock(), true) {
            return self.notify(format!("Couldn't draft a squad: {}. {}", crate::squads::refusal_words(&r), r.on_screen()));
        }
        let mut req = Request::new(squadfile::DRAFT_FEATURE, squadfile::draft_system(&instructions::guidance(&self.config, instructions::SQUAD_DRAFT)), squadfile::draft_prompt(description));
        req.user_initiated = true;
        req.model = Some(squadfile::DRAFT_MODEL);
        req.timeout = DRAFT_TIMEOUT;
        req.cancel = self.switchboard.token(squadfile::DRAFT_FEATURE);
        self.sqt.drafting = Some(self.spawn_call(settings, req));
        self.notify("Claude is drafting the squad. It shows here as a draft when it is done.");
    }

    /// Take in the drafted squad, if it has come. Called on every refresh; cheap when nothing is out.
    pub(super) fn poll_squad_draft(&mut self) {
        let answer = match self.sqt.drafting.as_ref().map(|rx| rx.try_recv()) {
            None | Some(Err(TryRecvError::Empty)) => return,
            Some(Ok(a)) => a,
            Some(Err(TryRecvError::Disconnected)) => Err(AiError::Cancelled),
        };
        self.sqt.drafting = None;
        let reply = match answer {
            Ok(r) => r,
            Err(AiError::Cancelled) => return self.notify("Drafting the squad was called off."),
            Err(e) => return self.notify(format!("Couldn't draft a squad: {}", e.plain())),
        };
        let taken: Vec<String> = self.sqt.list.iter().map(|l| l.id.clone()).collect();
        match squadfile::parse_draft(&reply.text, &self.config, &taken) {
            Ok(s) => {
                let name = s.name.clone();
                self.sqt.draft = Some(s);
                self.sqt.selected = self.sqt.list.len();
                self.sqt.focus = SqFocus::Detail;
                self.sqt.row = 0;
                self.sqt.detail_top.set(0);
                self.notify(format!("Here is a draft of \"{name}\". Change what you like: ctrl+s saves it, d discards it."));
            }
            Err(why) => self.notify(format!("Claude's draft couldn't be used: {why} Press n to try again.")),
        }
    }

    /// ctrl+s: save the draft as one of the user's own squads, once nothing is wrong with it.
    fn save_draft(&mut self) {
        let Some(mut s) = self.sqt.draft.clone() else { return };
        if let Some(first) = squadfile::problems(&s).into_iter().next() {
            return self.notify(format!("Not saved yet: {first}"));
        }
        // Another squad may have taken its id meanwhile.
        if squadfile::path(&self.config, &s.id).exists() || squadfile::is_builtin(&s.id) {
            s.id = squadfile::new_id(&self.config, &s.name, &[]);
        }
        match squadfile::save(&self.config, &s) {
            Ok(()) => {
                self.sqt.draft = None;
                self.reload_squads(Some(&s.id));
                self.notify(format!("Saved the squad \"{}\". Tickets can be handed to it now.", s.name));
            }
            Err(e) => self.notify(format!("Couldn't save the squad: {e}")),
        }
    }

    /// `c`: copy the selected squad under a new name.
    fn ask_copy_squad(&mut self) {
        if self.sqt.draft_selected() {
            return self.notify("Save the draft first (ctrl+s), then copy it.");
        }
        let Some(s) = self.squad_to_change() else { return };
        self.open_squad_name(SquadName::Copy, &format!("{} copy", s.name));
    }

    /// `d`: discard the draft, delete one of the user's own squads, or put a changed built-in back as platoon has it,
    /// each after asking. A built-in as platoon has it can't be deleted.
    fn ask_remove_squad(&mut self) {
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

    pub(super) fn discard_draft(&mut self) {
        self.mode = Mode::List;
        self.sqt.draft = None;
        self.sqt.focus = SqFocus::List;
        self.clamp_squad_row();
        self.notify("Discarded the draft.");
    }

    /// True while a squad is being drafted (so the screen is refreshed quickly).
    pub fn squad_drafting(&self) -> bool {
        self.sqt.drafting()
    }
}

/// A squad's system prompt open in an editor: it is written to a file of its own, and read back into the squad.
#[derive(Clone, Debug)]
pub struct SquadPromptEdit {
    /// The squad's id.
    pub squad: String,
    /// It is the unsaved draft.
    pub draft: bool,
    /// The member's name; None for the leader.
    pub member: Option<String>,
    pub path: PathBuf,
    pub before: String,
}
