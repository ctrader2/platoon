//! Tasks inside the app (the code says "ticket" in places, the name they had first): the task store the keys act on
//! (the project of the task picked on the Tasks tab), a task's own screen (its details and its text: enter on a task
//! that never started), cancelling one (`c`, after asking: the only state the user sets by hand), and editing its text
//! (`e`, through the notes' editor flow in `notes_glue.rs`). The data itself is in `tickets.rs`.

use super::*;
use crate::tickets::{State, Ticket, Tickets};

/// Everything about tasks the app keeps.
pub struct TicketsState {
    /// The tasks of the picked task's project (None until one is picked).
    pub store: Option<Tickets>,
    /// The task whose own screen is showing.
    pub open: Option<u32>,
    /// The first drawn line of the open task's screen in view.
    pub body_top: Cell<usize>,
    /// The project's screen (and its picked row) the open task's screen was opened from, where esc goes back to.
    pub back_to: Option<(String, usize)>,
}

impl TicketsState {
    pub fn new() -> Self {
        Self { store: None, open: None, body_top: Cell::new(0), back_to: None }
    }
}

impl App {
    pub fn tickets_store(&self) -> Option<&Tickets> {
        self.tk.store.as_ref()
    }

    pub fn ticket(&self, number: u32) -> Option<&Ticket> {
        self.tk.store.as_ref().and_then(|s| s.get(number))
    }

    /// Pick up a task added by `platoon task add`, or changed by a hook or in another window. One `stat` when nothing
    /// changed.
    pub(super) fn refresh_tickets(&mut self) {
        if self.board_showing() {
            self.sync_board_store();
        } else if let Some(s) = self.tk.store.as_mut() {
            s.reload_if_changed();
        }
        if self.tk.open.is_some_and(|n| self.ticket(n).is_none()) {
            self.tk.open = None;
            self.tk.back_to = None;
        }
    }

    pub(super) fn report_task_error(&mut self, e: io::Error) {
        let why = e.to_string();
        self.notify(format!("Couldn't save the task: {}", why.split(" (os error").next().unwrap_or(&why)));
    }

    // ---- keys -------------------------------------------------------------------------------------------------

    /// The keys that act on one task, the same on its row and on its screen.
    pub(super) fn ticket_key(&mut self, number: u32, key: KeyEvent) {
        match key.code {
            KeyCode::Char('s') => self.ask_start(number),
            KeyCode::Char('e') => self.pick_ticket_editor(number),
            KeyCode::Char('c') => self.ask_cancel_ticket(number),
            KeyCode::Enter => self.open_task(number),
            _ => {}
        }
    }

    /// Enter on a task: its session (running or continued) once it has one; else its own screen, with its text.
    pub(super) fn open_task(&mut self, number: u32) {
        let Some(t) = self.ticket(number) else { return };
        if t.started() {
            self.open_task_session(number);
        } else if self.tk.open != Some(number) {
            self.tk.open = Some(number);
            self.tk.back_to = None;
            self.tk.body_top.set(0);
        }
    }

    /// Keys on a task's own screen.
    pub(super) fn ticket_screen_key(&mut self, key: KeyEvent) {
        let Some(number) = self.tk.open else { return };
        let scroll = |app: &mut App, d: i64| app.tk.body_top.set((app.tk.body_top.get() as i64 + d).max(0) as usize);
        match key.code {
            KeyCode::Esc => self.close_ticket_screen(),
            KeyCode::Down | KeyCode::Char('j') => scroll(self, 1),
            KeyCode::Up | KeyCode::Char('k') => scroll(self, -1),
            KeyCode::PageDown => scroll(self, 10),
            KeyCode::PageUp => scroll(self, -10),
            KeyCode::Char('g') | KeyCode::Home => self.tk.body_top.set(0),
            _ => self.ticket_key(number, key),
        }
    }

    /// Esc on a task's screen: back to the Tasks tab, with the same task picked; or, when it was opened from a
    /// project's screen, back there on its row.
    pub(super) fn close_ticket_screen(&mut self) {
        let Some(n) = self.tk.open.take() else { return };
        if let Some(p) = self.task_scope() {
            self.bd.selected = Some(BRow(p, n));
        }
        if let Some((project, row)) = self.tk.back_to.take().filter(|(p, _)| self.projects.get(p).is_some()) {
            self.top = super::projects_glue::TOP_PROJECTS;
            self.open_project = Some(project);
            self.pane = Pane::Sessions;
            self.filter.clear();
            self.update_view();
            self.selected = row.min(self.row_count().saturating_sub(1));
        }
    }

    /// The wheel over a task's screen scrolls it.
    pub(super) fn wheel_ticket(&mut self, up: bool) {
        let d: i64 = if up { -3 } else { 3 };
        self.tk.body_top.set((self.tk.body_top.get() as i64 + d).max(0) as usize);
    }

    // ---- cancelling ---------------------------------------------------------------------------------------------

    /// `c`: cancel the task, after asking. When its session is running the question says it will be stopped too.
    fn ask_cancel_ticket(&mut self, number: u32) {
        let (Some(project), Some(t)) = (self.task_scope(), self.ticket(number).cloned()) else { return };
        if t.state == State::Cancelled {
            self.notify(format!("#{number} is cancelled already."));
            return;
        }
        let title = t.title.clone();
        let pid = t.started().then(|| self.task_host(&project, &t).map(|h| h.claude_pid)).flatten();
        let uncommitted = self.has_uncommitted_worktree(number);
        self.mode = Mode::Confirm(Confirm::CancelTicket { number, title, pid, uncommitted });
    }

    /// Yes to "Cancel #N?": Cancelled, and its session stopped when it was running when asked (the conversation is
    /// kept).
    pub(super) fn cancel_ticket(&mut self, number: u32, pid: Option<u32>) {
        self.mode = Mode::List;
        let Some(store) = self.tk.store.as_mut() else { return };
        if let Err(e) = store.cancel(number) {
            return self.report_task_error(e);
        }
        match pid {
            Some(pid) if self.stop_task_session(number, pid) => {
                self.notify(format!("#{number} is cancelled. Stopping its session (the conversation is kept)."))
            }
            _ => self.notify(format!("#{number} is cancelled.")),
        }
    }
}
