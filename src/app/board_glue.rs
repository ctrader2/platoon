//! The Tasks tab inside the app (`2`): every task in play across all projects and the ones in no project, in the
//! board's sections (`board.rs`), with the picked task's details beside the list on a wide terminal; and the same rows
//! in a project's Tasks pane, filtered to that project. Its keys: `enter` opens the task (beside its squad's session
//! when that runs), `y` answers the squad (a one-line box typed into its session) or gives the go-ahead, `h` puts a
//! task aside until the squad says something new, `n` makes a task in a project or in none (then its folder first),
//! `m` moves a task into another project (or out of any), and the keys a task has everywhere (`S`, `c`, `p`, ...).
//!
//! The keys that act on one task go through the task store of its project (`tk.store`): on the board that store
//! follows the picked row, so everything a task's screen and the pane do works on the board too.

use super::squads_glue::{split_size, type_into};
use super::*;
use crate::board::{self, Facts, Group, In, Row, Section};
use crate::tickets::{self, State, Ticket, Tickets};

/// A row of the board that can be picked.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum BRow {
    /// A task, by its project's id (`tickets::NO_PROJECT_ID` for none) and number.
    Task(String, u32),
    /// The fold under Finished today: enter shows the rest, and folds them again.
    Fold,
}

/// One line or row of a list of tasks, as it is drawn (the board, or a project's pane).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListItem {
    /// A section's heading, with how many tasks it has.
    Heading(Section, usize),
    Task(Row),
    /// Under Finished today: how many more are folded away (`open`: they show, and this folds them); in a project's
    /// pane, "Done and cancelled · N".
    Fold {
        section: Section,
        count: usize,
        open: bool,
    },
    /// A blank line between the board's sections.
    Gap,
}

impl ListItem {
    /// Lines it takes.
    pub fn height(&self) -> u16 {
        match self {
            ListItem::Task(_) => crate::ui::ROW_H,
            _ => 1,
        }
    }

    pub fn selectable(&self) -> bool {
        matches!(self, ListItem::Task(_) | ListItem::Fold { .. })
    }
}

/// What the project picker is open for.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PickFor {
    /// `n` on the board: which project the new task goes in.
    NewTask,
    /// `m`: where task `number` of project `.0` goes.
    Move(String, u32),
}

/// What the one-line box is for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AnswerFor {
    /// An answer to the squad's question (`y`), typed into its session as it is.
    #[default]
    Question,
    /// A change to the squad's plan (`n` on a plan to approve), typed as "Change the plan: <the note>".
    PlanChange,
}

/// What `y` types into the squad's session on a plan to approve.
pub const GO_AHEAD_WITH_PLAN: &str = "Go ahead with the plan.";
/// What `n` types before the user's note.
pub const CHANGE_THE_PLAN: &str = "Change the plan: ";

/// The one-line answer to a squad's question (`y`), or a change to its plan (`n`).
#[derive(Clone, Debug, Default)]
pub struct AnswerBox {
    pub project: String,
    pub number: u32,
    pub input: String,
    pub purpose: AnswerFor,
}

/// Everything about the board the app keeps.
pub struct BoardState {
    /// The picked row, by what it is (so it stays on the same task when the list changes).
    pub selected: Option<BRow>,
    /// Finished today shows all of its tasks.
    pub finished_open: bool,
    /// The project picker (`n`, `m`): what for, the project ids it offers (`NO_PROJECT_ID` last), the picked one.
    pub pick_for: PickFor,
    pub pick: Vec<String>,
    pub pick_selected: usize,
    pub answer: AnswerBox,
    /// The first line of the list in view (the board's and the pane's, kept where it was so the list doesn't jump).
    pub top: Cell<usize>,
}

impl BoardState {
    pub fn new() -> Self {
        Self {
            selected: None,
            finished_open: false,
            pick_for: PickFor::NewTask,
            pick: Vec::new(),
            pick_selected: 0,
            answer: AnswerBox::default(),
            top: Cell::new(0),
        }
    }
}

/// What the board knows from the app: the squads, the transcripts read, the sessions running.
struct AppFacts<'a> {
    app: &'a App,
    now: u64,
}

impl Facts for AppFacts<'_> {
    fn now(&self) -> u64 {
        self.now
    }
    fn squad_name(&self, id: &str) -> String {
        self.app.squad_name(id)
    }
    fn steps(&self, squad: &str) -> Vec<String> {
        self.app.sqt.list.iter().find(|l| l.id == squad).and_then(|l| l.squad.as_ref()).map(|s| s.agent_ids()).unwrap_or_default()
    }
    fn log(&self, session: &str) -> Option<&crate::claude::LeaderLog> {
        self.app.leader_log(session)
    }
    fn running(&self, session: &str) -> bool {
        self.app.leader_session_running(session)
    }
    fn has_result(&self, project: &str, number: u32) -> bool {
        tickets::bodies_dir(&self.app.config, project).join(tickets::result_file(number)).is_file()
    }
    fn project_name(&self, id: &str) -> String {
        self.app.task_project_name(id)
    }
}

impl App {
    /// The Tasks tab is showing (its list, or a task opened from it).
    pub fn board_showing(&self) -> bool {
        self.top == TOP_TASKS && self.open_project.is_none()
    }

    /// A task's own screen is showing: opened from a project's pane, or from the board.
    pub fn task_screen_showing(&self) -> bool {
        self.tk.open.is_some() && (self.open_project.is_some() || self.board_showing())
    }

    /// The project whose tasks the keys act on: the open project's screen, or the project of the task picked on the
    /// board (`NO_PROJECT_ID` for a task in none). None when there is none.
    pub fn task_scope(&self) -> Option<String> {
        self.tk.store.as_ref().map(|s| s.project().to_string())
    }

    /// The tasks' project as a person reads it: its name, or "No project".
    pub fn task_project_name(&self, id: &str) -> String {
        if id == tickets::NO_PROJECT_ID {
            return tickets::NO_PROJECT_NAME.to_string();
        }
        self.projects.get(id).map(|p| p.name.clone()).unwrap_or_else(|| id.to_string())
    }

    /// Make `project`'s tasks the ones the keys act on (the store is opened again only when it is another project's).
    pub(super) fn set_task_store(&mut self, project: &str) {
        // What was just written through the store shows on the board from the other projects' list too (one `stat` per
        // tasks file when nothing changed).
        self.reread_tickets();
        if self.task_scope().as_deref() == Some(project) {
            if let Some(s) = self.tk.store.as_mut() {
                s.reload_if_changed();
            }
            return;
        }
        self.tk.store = Some(Tickets::open(&self.config, project));
        self.show_starting();
    }

    /// Every task of every project and of none, by project, as the files said when last read, with the tasks the keys
    /// act on as they are now (a change just made shows at once) and the ones being handed to a squad showing what
    /// is happening.
    pub fn all_tasks(&self) -> Vec<(String, Vec<Ticket>)> {
        let mut out: Vec<(String, Vec<Ticket>)> = Vec::new();
        for (p, t) in self.every_ticket() {
            match out.iter_mut().find(|(q, _)| q == p) {
                Some((_, v)) => v.push(t.clone()),
                None => out.push((p.clone(), vec![t.clone()])),
            }
        }
        if let Some(store) = self.tk.store.as_ref().filter(|s| !s.is_corrupt()) {
            let fresh = store.all().to_vec();
            match out.iter_mut().find(|(q, _)| q == store.project()) {
                Some((_, v)) => *v = fresh,
                None => out.push((store.project().to_string(), fresh)),
            }
        }
        for (p, list) in &mut out {
            for t in list.iter_mut() {
                if let Some(words) = self.starting_words(p, t.number) {
                    t.busy = Some(words.to_string());
                    t.state = State::Working;
                }
            }
        }
        out
    }

    /// How many tasks need the user now, across everything (the header says it on every tab).
    pub fn need_you_count(&self) -> usize {
        self.all_tasks().iter().map(|(_, list)| board::need_count(list)).sum()
    }

    /// The text the board is narrowed by: what is being typed in its filter box, else the filter kept.
    pub fn board_filter(&self) -> &str {
        match (&self.mode, &self.name_box.purpose) {
            (Mode::ProjectName, NamePurpose::TaskFilter) => &self.name_box.input,
            _ => &self.filter,
        }
    }

    fn groups_of(&self, tasks: &[(String, Vec<Ticket>)], pane: bool, filter: &str) -> Vec<Group> {
        let facts = AppFacts { app: self, now: usage::now() };
        let ins: Vec<In> = tasks
            .iter()
            .flat_map(|(p, list)| {
                let archived = self.projects.get(p).is_some_and(|x| x.archived);
                list.iter().map(move |t| In { project: p, archived, task: t, siblings: list })
            })
            .collect();
        board::board(&ins, &facts, pane, filter)
    }

    /// The board's sections and their rows.
    pub fn board_groups(&self) -> Vec<Group> {
        self.groups_of(&self.all_tasks(), false, self.board_filter())
    }

    /// The open project's pane: the same rows, its tasks only, with all of its Done and Cancelled ones last.
    pub fn pane_groups(&self) -> Vec<Group> {
        let Some(store) = self.tk.store.as_ref().filter(|_| self.open_project.is_some()) else { return Vec::new() };
        self.groups_of(&[(store.project().to_string(), store.all().to_vec())], true, "")
    }

    /// The board as it is drawn: each section's heading, its rows (Finished today folded after five), a blank line
    /// between sections.
    pub fn board_items(&self) -> Vec<ListItem> {
        let mut out = Vec::new();
        for (k, g) in self.board_groups().into_iter().enumerate() {
            if k > 0 {
                out.push(ListItem::Gap);
            }
            out.push(ListItem::Heading(g.section, g.rows.len()));
            let fold = g.section == Section::Finished && g.rows.len() > board::FINISHED_SHOWN;
            let total = g.rows.len();
            let shown = if fold && !self.bd.finished_open { board::FINISHED_SHOWN } else { total };
            out.extend(g.rows.into_iter().take(shown).map(ListItem::Task));
            if fold {
                out.push(ListItem::Fold { section: g.section, count: total - board::FINISHED_SHOWN, open: self.bd.finished_open });
            }
        }
        out
    }

    /// A project's pane as it is drawn: the open tasks under their headings, then "Done and cancelled · N", folded
    /// until enter opens it.
    pub fn pane_items(&self) -> Vec<ListItem> {
        let mut out = Vec::new();
        for g in self.pane_groups() {
            if g.section == Section::Closed {
                out.push(ListItem::Fold { section: g.section, count: g.rows.len(), open: self.tk.unfolded });
                if self.tk.unfolded {
                    out.extend(g.rows.into_iter().map(ListItem::Task));
                }
                continue;
            }
            out.push(ListItem::Heading(g.section, g.rows.len()));
            out.extend(g.rows.into_iter().map(ListItem::Task));
        }
        out
    }

    /// The board's rows that can be picked, in order.
    pub fn board_rows(&self) -> Vec<BRow> {
        self.board_items()
            .into_iter()
            .filter_map(|i| match i {
                ListItem::Task(r) => Some(BRow::Task(r.project, r.number)),
                ListItem::Fold { .. } => Some(BRow::Fold),
                _ => None,
            })
            .collect()
    }

    /// Index into `board_rows()` of the picked row (the first when the picked one isn't there any more).
    pub fn board_selected(&self) -> usize {
        let rows = self.board_rows();
        self.bd.selected.as_ref().and_then(|s| rows.iter().position(|r| r == s)).unwrap_or(0)
    }

    /// The picked row of the board, as it is now (the first row when nothing was picked).
    pub fn board_pick(&self) -> Option<BRow> {
        let rows = self.board_rows();
        let at = self.board_selected();
        rows.get(at).cloned()
    }

    /// The task the board has picked: (its project, its number).
    pub fn board_task(&self) -> Option<(String, u32)> {
        match self.board_pick() {
            Some(BRow::Task(p, n)) => Some((p, n)),
            _ => None,
        }
    }

    /// The picked task's row as the board draws it.
    pub fn board_row(&self) -> Option<Row> {
        let (p, n) = self.board_task()?;
        self.board_groups().into_iter().flat_map(|g| g.rows).find(|r| r.project == p && r.number == n)
    }

    fn select_board_row(&mut self, i: usize) {
        self.bd.selected = self.board_rows().get(i).cloned();
        self.sync_board_store();
    }

    fn move_board(&mut self, delta: i32) {
        let n = self.board_rows().len();
        if n == 0 {
            return;
        }
        let at = (self.board_selected() as i32 + delta).clamp(0, n as i32 - 1) as usize;
        self.select_board_row(at);
    }

    /// The tasks the keys act on follow the board's picked row (its project's store). A task's screen opened from the
    /// board keeps its own.
    pub(super) fn sync_board_store(&mut self) {
        self.reread_tickets();
        // A task's screen, or a popup about a task (its number is the store's project's), keeps the store it has.
        if self.tk.open.is_some() || !matches!(self.mode, Mode::List) {
            if let Some(s) = self.tk.store.as_mut() {
                s.reload_if_changed();
            }
            return;
        }
        if let Some((p, _)) = self.board_task() {
            self.set_task_store(&p);
        }
    }

    /// After an answer, a go-ahead or a task put aside on the board, the cursor stays where it was (on the next task
    /// that needs the user), rather than following that task to its new section.
    fn keep_board_place(&mut self, at: usize) {
        self.reread_tickets();
        let items = self.board_items();
        let needs: Vec<BRow> = items
            .iter()
            .filter_map(|i| match i {
                ListItem::Task(r) if r.section == Section::NeedsYou => Some(BRow::Task(r.project.clone(), r.number)),
                _ => None,
            })
            .collect();
        let rows = self.board_rows();
        self.bd.selected = match needs.len() {
            // The next one that needs the user (the board lists those first).
            n if n > 0 => needs.get(at.min(n - 1)).cloned(),
            _ => rows.get(at.min(rows.len().saturating_sub(1))).cloned(),
        };
        self.sync_board_store();
    }

    // ---- keys -----------------------------------------------------------------------------------------------------

    /// Keys of the board's list (the ones it shares with every top-level list are handled before this).
    pub(super) fn board_key(&mut self, key: KeyEvent) {
        self.sync_board_store();
        match key.code {
            // On a plan to approve, `n` asks for a change to it (beside `y`, the go-ahead); elsewhere it is a new task.
            KeyCode::Char('n') if self.board_task().is_some_and(|(p, n)| self.plan_waits(&p, n)) => {
                if let Some((p, n)) = self.board_task() {
                    self.set_task_store(&p);
                    self.ask_plan_change(n);
                }
            }
            KeyCode::Char('n') => self.ask_task_project(PickFor::NewTask),
            KeyCode::Char('/') => self.open_name_box(NamePurpose::TaskFilter),
            KeyCode::Esc => self.filter.clear(),
            KeyCode::Enter => match self.board_pick() {
                Some(BRow::Fold) => self.bd.finished_open = !self.bd.finished_open,
                Some(BRow::Task(p, n)) => {
                    self.set_task_store(&p);
                    self.open_ticket_screen(n);
                }
                None => {}
            },
            KeyCode::Down | KeyCode::Char('j') => self.move_board(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_board(-1),
            KeyCode::PageDown => self.move_board(super::page_of(&self.pages.tickets)),
            KeyCode::PageUp => self.move_board(-super::page_of(&self.pages.tickets)),
            KeyCode::Char('g') | KeyCode::Home => self.select_board_row(0),
            KeyCode::Char('G') | KeyCode::End => self.select_board_row(self.board_rows().len().saturating_sub(1)),
            // The go-ahead to publish: as with `y`, the cursor stays where it was, on the next task that needs the user.
            KeyCode::Char('p') if self.board_task().is_some() => {
                let Some((p, n)) = self.board_task() else { return };
                self.set_task_store(&p);
                let place = self.board_selected();
                let wanted = self.ticket(n).is_some_and(|t| t.state == State::Waiting && t.publish_wanted && t.busy.is_none());
                self.ticket_key(n, key);
                if wanted {
                    self.keep_board_place(place);
                }
            }
            _ => match self.board_task() {
                Some((p, n)) => {
                    self.set_task_store(&p);
                    self.ticket_key(n, key);
                }
                None if matches!(key.code, KeyCode::Char('y' | 'h' | 'm' | 'S' | 'c' | 'p' | 'e' | 'r' | 'f' | 'b' | 'w')) => {
                    self.notify("No task picked. Press n to write one.")
                }
                None => {}
            },
        }
    }

    /// A click on a row of the board: picks it; a click on the picked one is enter.
    pub(super) fn click_board_row(&mut self, i: usize) {
        if i == self.board_selected() && self.board_rows().get(i).is_some() {
            self.board_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        } else {
            self.select_board_row(i);
        }
    }

    // ---- y: answering the squad ------------------------------------------------------------------------------------

    /// `y` on a task that Needs you (or one put aside): the answer box for a question (or a squad that is stuck), the
    /// go-ahead for one that wants to publish, its session for a command waiting at the permission prompt. A squad
    /// whose session isn't running is continued, and the answer is typed there.
    pub(super) fn answer_squad(&mut self, number: u32) {
        let Some(t) = self.ticket(number).cloned() else { return };
        let Some(need) = board::need_of(&t) else {
            self.notify(format!("#{number} doesn't need you just now: y answers a task under Needs you."));
            return;
        };
        let place = self.board_selected();
        match need {
            board::Need::Plan => self.go_ahead_with_plan(number),
            board::Need::GoAhead => {
                self.let_it_publish(number);
                if self.board_showing() {
                    self.keep_board_place(place);
                }
            }
            board::Need::Permission => {
                self.open_ticket_screen_for_session(number);
                self.notify(format!("#{number}'s squad waits for your OK on a command: answer it in its session."));
            }
            board::Need::Question | board::Need::Stuck | board::Need::Tests => {
                let running = t.leader().is_some_and(|l| self.leader_host(&l.session).is_some());
                if !running {
                    self.open_ticket_screen_for_session(number);
                    self.notify(format!("#{number}'s squad is running again: type your answer in its session."));
                    return;
                }
                let project = self.task_scope().unwrap_or_default();
                self.bd.answer = AnswerBox { project, number, input: String::new(), purpose: AnswerFor::Question };
                self.mode = Mode::Answer;
            }
        }
    }

    /// Task `number` of `project` waits for the user's yes to its squad's plan (as the board read it last).
    pub(super) fn plan_waits(&self, project: &str, number: u32) -> bool {
        self.all_tasks().iter().filter(|(p, _)| p == project).flat_map(|(_, l)| l.iter()).any(|t| t.number == number && t.plan_ready)
    }

    /// `y` on a plan to approve: "Go ahead with the plan." is typed into the squad's session, and the task is Working
    /// again. A squad whose session isn't running is continued beside the task, to be told there.
    pub(super) fn go_ahead_with_plan(&mut self, number: u32) {
        let Some(t) = self.ticket(number).cloned() else { return };
        if !t.plan_ready {
            self.notify(format!("#{number} has no plan waiting for you."));
            return;
        }
        let session = t.leader().map(|l| l.session.clone()).unwrap_or_default();
        let Some(sock) = self.leader_host(&session).map(|h| h.sock.clone()) else {
            self.open_ticket_screen_for_session(number);
            self.notify(format!("#{number}'s squad is running again: tell it to go ahead with the plan there."));
            return;
        };
        let place = self.board_selected();
        let Some(store) = self.tk.store.as_mut() else { return };
        if let Err(e) = store.plan_answered(number) {
            return self.report_task_error(e);
        }
        let (rows, cols) = split_size(self.size);
        thread::spawn(move || type_into(&sock, rows, cols, GO_AHEAD_WITH_PLAN));
        self.notify(format!("#{number}'s squad goes ahead with the plan."));
        if self.board_showing() {
            self.keep_board_place(place);
        }
    }

    /// `n` on a plan to approve: the one-line box for what should change. Its squad's session must be running to take
    /// it (one that isn't is continued beside the task, to be told there).
    pub(super) fn ask_plan_change(&mut self, number: u32) {
        let Some(t) = self.ticket(number).cloned() else { return };
        if !t.plan_ready {
            self.notify(format!("#{number} has no plan waiting for you: n changes a squad's plan once it is ready."));
            return;
        }
        let running = t.leader().is_some_and(|l| self.leader_host(&l.session).is_some());
        if !running {
            self.open_ticket_screen_for_session(number);
            self.notify(format!("#{number}'s squad is running again: say what to change in the plan there."));
            return;
        }
        let project = self.task_scope().unwrap_or_default();
        self.bd.answer = AnswerBox { project, number, input: String::new(), purpose: AnswerFor::PlanChange };
        self.mode = Mode::Answer;
    }

    /// The task's screen with its squad's session beside it (continued when it isn't running).
    fn open_ticket_screen_for_session(&mut self, number: u32) {
        self.tk.open = Some(number);
        self.tk.body_top.set(0);
        self.open_leader(number);
    }

    /// The squad the answer box is for, by name.
    pub fn answer_squad_name(&self) -> String {
        let a = &self.bd.answer;
        self.ticket(a.number).and_then(|t| t.leader()).map(|l| self.squad_name(&l.squad)).unwrap_or_else(|| "squad's".into())
    }

    pub(super) fn answer_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => self.act(Action::Cancel),
            KeyCode::Char('c') if ctrl => self.act(Action::Cancel),
            KeyCode::Enter => self.act(Action::Confirm),
            KeyCode::Char('o') if ctrl => {
                self.mode = Mode::List;
                let n = self.bd.answer.number;
                self.open_ticket_screen_for_session(n);
            }
            KeyCode::Char('u') if ctrl => self.bd.answer.input.clear(),
            KeyCode::Backspace => {
                self.bd.answer.input.pop();
            }
            KeyCode::Char(c) if !ctrl && !alt => self.type_answer(&c.to_string()),
            _ => {}
        }
    }

    /// Typed or pasted into the answer box: one line (a pasted line break is a space), at most 2,000 characters.
    pub(super) fn type_answer(&mut self, text: &str) {
        let room = 2000usize.saturating_sub(self.bd.answer.input.chars().count());
        let text = text.replace(['\r', '\n'], " ");
        self.bd.answer.input.extend(text.chars().take(room));
    }

    /// Enter in the answer box: the words are typed into the squad's session, as a message of the user's own (its
    /// prompt hook makes the task Working again).
    pub(super) fn send_answer(&mut self) {
        let plan = self.bd.answer.purpose == AnswerFor::PlanChange;
        let text = self.bd.answer.input.trim().to_string();
        if text.is_empty() {
            self.notify(if plan {
                "Say what should change first, or esc to leave it."
            } else {
                "Type your answer first, or esc to leave it."
            });
            return;
        }
        let (project, number) = (self.bd.answer.project.clone(), self.bd.answer.number);
        self.mode = Mode::List;
        self.set_task_store(&project);
        let session = self.ticket(number).and_then(|t| t.leader()).map(|l| l.session.clone()).unwrap_or_default();
        let Some(sock) = self.leader_host(&session).map(|h| h.sock.clone()) else {
            let again = if plan { "Press n to continue it." } else { "Press y to continue it." };
            self.notify(format!("#{number}'s squad stopped meanwhile, so nothing was sent. {again}"));
            return;
        };
        let place = self.board_selected();
        let text = if plan {
            // Working again at once: the squad plans again, and says so with its next plan.
            if let Some(Err(e)) = self.tk.store.as_mut().map(|s| s.plan_answered(number)) {
                return self.report_task_error(e);
            }
            format!("{CHANGE_THE_PLAN}{text}")
        } else {
            text
        };
        let (rows, cols) = split_size(self.size);
        thread::spawn(move || type_into(&sock, rows, cols, &text));
        self.notify(if plan {
            format!("Sent the change to #{number}'s squad: it plans again.")
        } else {
            format!("Answer sent to #{number}.")
        });
        if self.board_showing() {
            self.keep_board_place(place);
        }
    }

    // ---- h: later ----------------------------------------------------------------------------------------------------

    /// `h` on a task that Needs you: aside, under Later, until its squad says something new; `h` again brings it back.
    pub(super) fn toggle_aside(&mut self, number: u32) {
        let Some(t) = self.ticket(number).cloned() else { return };
        if t.state != State::Waiting {
            self.notify(format!("#{number} doesn't need you: h puts aside a task under Needs you, until its squad says something new."));
            return;
        }
        let on = !t.is_aside();
        let place = self.board_selected();
        let Some(store) = self.tk.store.as_mut() else { return };
        match store.set_aside(number, on) {
            Ok(()) if on => self.notify(format!("#{number} is under Later until its squad says something new. h brings it back.")),
            Ok(()) => self.notify(format!("#{number} needs you again.")),
            Err(e) => return self.report_task_error(e),
        }
        if self.board_showing() && on {
            self.keep_board_place(place);
        }
    }

    fn report_task_error(&mut self, e: io::Error) {
        let why = e.to_string();
        self.notify(format!("Couldn't save the task: {}", why.split(" (os error").next().unwrap_or(&why)));
    }

    // ---- the project picker: n and m -------------------------------------------------------------------------------

    /// The project picker: for a new task (the active projects and No project, the picked row's project first picked),
    /// or to move task `number` (the others).
    pub(super) fn ask_task_project(&mut self, purpose: PickFor) {
        let mut active: Vec<&crate::projects::Project> = self.projects.all().iter().filter(|p| !p.archived).collect();
        active.sort_by_key(|p| p.name.to_lowercase());
        let mut pick: Vec<String> = active.iter().map(|p| p.id.clone()).collect();
        pick.push(tickets::NO_PROJECT_ID.to_string());
        let here = match &purpose {
            PickFor::NewTask => self.board_task().map(|(p, _)| p),
            PickFor::Move(p, _) => Some(p.clone()),
        };
        if let PickFor::Move(from, _) = &purpose {
            pick.retain(|p| p != from);
        }
        self.bd.pick_selected = match &purpose {
            PickFor::NewTask => here.and_then(|h| pick.iter().position(|p| *p == h)).unwrap_or(0),
            PickFor::Move(..) => 0,
        };
        self.bd.pick = pick;
        self.bd.pick_for = purpose;
        self.mode = Mode::TaskProject;
    }

    pub(super) fn move_task_project_pick(&mut self, delta: i32) {
        let last = self.bd.pick.len().saturating_sub(1) as i32;
        self.bd.pick_selected = (self.bd.pick_selected as i32 + delta).clamp(0, last) as usize;
    }

    pub(super) fn task_project_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.act(Action::Cancel),
            KeyCode::Char('c') if ctrl => self.act(Action::Cancel),
            KeyCode::Enter => self.act(Action::Confirm),
            KeyCode::Up | KeyCode::Char('k') => self.move_task_project_pick(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_task_project_pick(1),
            KeyCode::Home | KeyCode::Char('g') => self.bd.pick_selected = 0,
            KeyCode::End | KeyCode::Char('G') => self.bd.pick_selected = self.bd.pick.len().saturating_sub(1),
            _ => {}
        }
    }

    /// Enter in the project picker.
    pub(super) fn take_task_project(&mut self) {
        self.mode = Mode::List;
        let Some(to) = self.bd.pick.get(self.bd.pick_selected).cloned() else { return };
        match self.bd.pick_for.clone() {
            PickFor::NewTask => self.open_name_box(NamePurpose::NewTask(to)),
            PickFor::Move(from, number) => self.move_task_to(&from, number, &to),
        }
    }

    // ---- n: a new task on the board ----------------------------------------------------------------------------------

    /// Enter in the title box of a new task on the board: in a project, it works in the folder of the project's most
    /// recently used session (as the pane's `n` does) and is written now; in no project, the folder picker comes first
    /// (a task needs a folder to work in), and the task is written once one is picked.
    pub(super) fn make_board_task(&mut self, project: &str, title: &str) {
        if project == tickets::NO_PROJECT_ID {
            self.open_picker_in(None);
            self.picker.new_task = Some(title.to_string());
            return;
        }
        let folders: Vec<String> = self.project_folders(project).into_iter().take(1).collect();
        self.write_new_task(project, title, folders);
    }

    /// Enter in the folder picker opened for a new task in no project.
    pub(super) fn new_task_folder(&mut self, title: &str, dir: String) {
        self.mode = Mode::List;
        self.write_new_task(tickets::NO_PROJECT_ID, title, vec![dir]);
    }

    fn write_new_task(&mut self, project: &str, title: &str, folders: Vec<String>) {
        if let Some(p) = self.projects.get(project).filter(|p| p.archived) {
            let why = tickets::archived_refusal(&p.name);
            self.notify(why);
            return;
        }
        self.set_task_store(project);
        let Some(store) = self.tk.store.as_mut() else { return };
        match store.add(tickets::NewTicket { title: title.to_string(), folders, ..Default::default() }) {
            Ok(n) => {
                self.bd.selected = Some(BRow::Task(project.to_string(), n));
                let whose = if project == tickets::NO_PROJECT_ID {
                    " (in no project)".to_string()
                } else {
                    format!(" in {}", self.task_project_name(project))
                };
                self.notify(format!("Made task #{n}{whose}"));
                self.pick_ticket_editor(n);
            }
            Err(e) => self.report_task_error(e),
        }
    }

    // ---- m: moving a task ----------------------------------------------------------------------------------------------

    /// `m` on a task: the project picker, to move it. Refused while its squad's session runs, while it is being handed
    /// over, or while it waits in the line for a free squad.
    pub(super) fn ask_move_task(&mut self, number: u32) {
        let Some(t) = self.ticket(number).cloned() else { return };
        let Some(from) = self.task_scope() else { return };
        let why = if t.busy.is_some() {
            Some(format!("#{number} is being handed to a squad. Wait a moment."))
        } else if t.queued.is_some() {
            Some(format!("#{number} is waiting for a free squad, so it stays where it is. Cancel it first to move it."))
        } else if t.leaders.iter().any(|l| self.leader_session_running(&l.session)) {
            Some(format!("#{number}'s squad is at work on it, so it stays where it is until its session stops."))
        } else {
            None
        };
        if let Some(why) = why {
            self.notify(why);
            return;
        }
        self.ask_task_project(PickFor::Move(from, number));
        if self.bd.pick.is_empty() {
            self.mode = Mode::List;
            self.notify("There is no other project to move it to.");
        }
    }

    fn move_task_to(&mut self, from: &str, number: u32, to: &str) {
        // Checked again: the squad may have started meanwhile.
        if self.ticket(number).is_some_and(|t| t.leaders.iter().any(|l| self.leader_session_running(&l.session))) {
            self.notify(format!("#{number}'s squad is at work on it, so it stays where it is until its session stops."));
            return;
        }
        match tickets::move_task(&self.config, from, number, to) {
            Ok(new) => {
                if let Some(s) = self.tk.store.as_mut() {
                    s.reload_if_changed();
                }
                self.reread_tickets();
                let msg = if to == tickets::NO_PROJECT_ID {
                    format!("Moved #{number} out of {}: it is #{new} under No project now.", self.task_project_name(from))
                } else {
                    format!("Moved #{number} to {}: it is #{new} there.", self.task_project_name(to))
                };
                self.notify(msg);
                if self.board_showing() {
                    self.bd.selected = Some(BRow::Task(to.to_string(), new));
                    self.set_task_store(to);
                } else if self.tk.open == Some(number) {
                    self.tk.open = None;
                }
            }
            Err(e) => {
                let why = e.to_string();
                self.notify(format!("Couldn't move #{number}: {}", why.split(" (os error").next().unwrap_or(&why)));
            }
        }
    }
}
