//! The Tasks tab inside the app (`3`): every task across all projects and the ones in no project, in the board's
//! sections (`board.rs`), with the picked task's details beside the list on a wide terminal. Its keys: `n` makes a
//! task (the project first, then its title; a task in no project picks its folder), `s` starts the picked one, `enter`
//! opens it (its session, or its text when it never started), `e` edits its text, `c` cancels it, `/` narrows the list.
//!
//! The keys that act on one task go through the task store of its project (`tk.store`), which follows the picked row.

use super::*;
use crate::board::{self, Facts, Group, In, Row, Section};
use crate::tickets::{self, Ticket, Tickets};

/// A task on the board: its project's id (`tickets::NO_PROJECT_ID` for none) and its number.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BRow(pub String, pub u32);

/// One line or row of the board, as it is drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListItem {
    /// A section's heading, with how many tasks it has.
    Heading(Section, usize),
    Task(Row),
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
}

/// Everything about the board the app keeps.
pub struct BoardState {
    /// The picked task (so it stays on the same task when the list changes).
    pub selected: Option<BRow>,
    /// The project picker of `n`: the project ids it offers (`NO_PROJECT_ID` last) and the picked one.
    pub pick: Vec<String>,
    pub pick_selected: usize,
    /// The first line of the list in view (kept where it was so the list doesn't jump).
    pub top: Cell<usize>,
}

impl BoardState {
    pub fn new() -> Self {
        Self { selected: None, pick: Vec::new(), pick_selected: 0, top: Cell::new(0) }
    }
}

/// What the board knows from the app: the squads, the transcripts read.
struct AppFacts<'a> {
    app: &'a App,
    squads: Vec<crate::squads::Squad>,
    now: u64,
}

impl Facts for AppFacts<'_> {
    fn now(&self) -> u64 {
        self.now
    }
    fn squad_name(&self, id: &str) -> String {
        crate::squads::name_of(&self.squads, id)
    }
    fn tokens(&self, session: &str) -> Option<String> {
        self.app.session_spent(session).map(|s| crate::spent::row_total(&s))
    }
    fn last_wrote(&self, session: &str) -> Option<u64> {
        self.app.session_log(session).and_then(|l| l.last_at).map(|ms| ms / 1000)
    }
    fn project_name(&self, id: &str) -> String {
        self.app.task_project_name(id)
    }
    fn not_merged(&self, project: &str, number: u32) -> bool {
        self.app.not_merged(project, number).is_some()
    }
}

impl App {
    /// The Tasks tab is showing (its list, or a task opened from it).
    pub fn board_showing(&self) -> bool {
        self.top == TOP_TASKS && self.open_project.is_none()
    }

    /// A task's own screen is showing.
    pub fn task_screen_showing(&self) -> bool {
        self.tk.open.is_some() && self.board_showing()
    }

    /// The project whose tasks the keys act on: the picked task's (`NO_PROJECT_ID` for a task in none).
    pub fn task_scope(&self) -> Option<String> {
        self.tk.store.as_ref().map(|s| s.project().to_string())
    }

    /// A project's name as a person reads it: its name, or "No project".
    pub fn task_project_name(&self, id: &str) -> String {
        if id == tickets::NO_PROJECT_ID {
            return tickets::NO_PROJECT_NAME.to_string();
        }
        self.projects.get(id).map(|p| p.name.clone()).unwrap_or_else(|| id.to_string())
    }

    /// The name of a squad, by its id (as the squads are now).
    pub fn squad_name(&self, id: &str) -> String {
        crate::squads::name_of(&crate::squads::all(&self.config), id)
    }

    /// Make `project`'s tasks the ones the keys act on (the store is opened again only when it is another project's).
    pub(super) fn set_task_store(&mut self, project: &str) {
        self.reread_tickets();
        if self.task_scope().as_deref() == Some(project) {
            if let Some(s) = self.tk.store.as_mut() {
                s.reload_if_changed();
            }
            return;
        }
        self.tk.store = Some(Tickets::open(&self.config, project));
    }

    /// Every task of every project and of none, by project, as the files said when last read, with the store the keys
    /// act on as it is now (a change just made shows at once).
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

    /// The board's sections and their rows.
    pub fn board_groups(&self) -> Vec<Group> {
        let tasks = self.all_tasks();
        let facts = AppFacts { app: self, squads: crate::squads::all(&self.config), now: usage::now() };
        let ins: Vec<In> = tasks
            .iter()
            .flat_map(|(p, list)| {
                let archived = self.projects.get(p).is_some_and(|x| x.archived);
                list.iter().map(move |t| In { project: p, archived, task: t, busy: self.starting_words(p, t.number) })
            })
            .collect();
        board::board(&ins, &facts, self.board_filter())
    }

    /// The board as it is drawn: each section's heading, its rows, a blank line between sections.
    pub fn board_items(&self) -> Vec<ListItem> {
        let mut out = Vec::new();
        for (k, g) in self.board_groups().into_iter().enumerate() {
            if k > 0 {
                out.push(ListItem::Gap);
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
                ListItem::Task(r) => Some(BRow(r.project, r.number)),
                _ => None,
            })
            .collect()
    }

    /// Index into `board_rows()` of the picked row (the first when the picked one isn't there any more).
    pub fn board_selected(&self) -> usize {
        let rows = self.board_rows();
        self.bd.selected.as_ref().and_then(|s| rows.iter().position(|r| r == s)).unwrap_or(0)
    }

    /// The task the board has picked: (its project, its number).
    pub fn board_task(&self) -> Option<(String, u32)> {
        let rows = self.board_rows();
        rows.get(self.board_selected()).map(|r| (r.0.clone(), r.1))
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

    /// The tasks the keys act on follow the board's picked row (its project's store). A task's screen, or a popup about
    /// a task, keeps the store it has.
    pub(super) fn sync_board_store(&mut self) {
        self.reread_tickets();
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

    // ---- keys -----------------------------------------------------------------------------------------------------

    /// Keys of the board's list (the ones it shares with every top-level list are handled before this).
    pub(super) fn board_key(&mut self, key: KeyEvent) {
        self.sync_board_store();
        match key.code {
            KeyCode::Char('n') => self.ask_task_project(),
            KeyCode::Char('/') => self.open_name_box(NamePurpose::TaskFilter),
            KeyCode::Esc => self.filter.clear(),
            KeyCode::Down | KeyCode::Char('j') => self.move_board(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_board(-1),
            KeyCode::PageDown => self.move_board(super::page_of(&self.pages.tickets)),
            KeyCode::PageUp => self.move_board(-super::page_of(&self.pages.tickets)),
            KeyCode::Char('g') | KeyCode::Home => self.select_board_row(0),
            KeyCode::Char('G') | KeyCode::End => self.select_board_row(self.board_rows().len().saturating_sub(1)),
            _ => match self.board_task() {
                Some((p, n)) => {
                    self.set_task_store(&p);
                    self.ticket_key(n, key);
                }
                None if matches!(key.code, KeyCode::Char('s' | 'e' | 'c')) => self.notify("No task picked. Press n to write one."),
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

    // ---- n: a new task ---------------------------------------------------------------------------------------------

    /// The project picker for a new task: the active projects and No project, the picked row's project first picked.
    pub(super) fn ask_task_project(&mut self) {
        let mut active: Vec<&crate::projects::Project> = self.projects.all().iter().filter(|p| !p.archived).collect();
        active.sort_by_key(|p| p.name.to_lowercase());
        let mut pick: Vec<String> = active.iter().map(|p| p.id.clone()).collect();
        pick.push(tickets::NO_PROJECT_ID.to_string());
        let here = self.board_task().map(|(p, _)| p);
        self.bd.pick_selected = here.and_then(|h| pick.iter().position(|p| *p == h)).unwrap_or(0);
        self.bd.pick = pick;
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

    /// Enter in the project picker: the title box.
    pub(super) fn take_task_project(&mut self) {
        self.mode = Mode::List;
        let Some(to) = self.bd.pick.get(self.bd.pick_selected).cloned() else { return };
        self.open_name_box(NamePurpose::NewTask(to));
    }

    /// Enter in the title box of a new task: in a project, it works in the folder of the project's most recently used
    /// session (none when it has none: it asks for one when it starts) and is written now; in no project, the folder
    /// picker comes first, and the task is written once one is picked.
    pub(super) fn make_board_task(&mut self, project: &str, title: &str) {
        if project == tickets::NO_PROJECT_ID {
            self.open_picker_in(None);
            self.picker.new_task = Some(title.to_string());
            return;
        }
        let folder = self.project_folders(project).into_iter().next();
        self.write_new_task(project, title, folder);
    }

    /// Enter in the folder picker opened for a new task in no project.
    pub(super) fn new_task_folder(&mut self, title: &str, dir: String) {
        self.mode = Mode::List;
        self.write_new_task(tickets::NO_PROJECT_ID, title, Some(dir));
    }

    fn write_new_task(&mut self, project: &str, title: &str, folder: Option<String>) {
        if let Some(p) = self.projects.get(project).filter(|p| p.archived) {
            let why = tickets::archived_refusal(&p.name);
            self.notify(why);
            return;
        }
        self.set_task_store(project);
        let Some(store) = self.tk.store.as_mut() else { return };
        match store.add(tickets::NewTicket { title: title.to_string(), folder, ..Default::default() }) {
            Ok(n) => {
                self.reread_tickets();
                self.bd.selected = Some(BRow(project.to_string(), n));
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
}
