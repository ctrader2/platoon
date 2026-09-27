//! Tickets inside the app: the Tickets pane of a project's screen (open tickets, then "Done and cancelled", folded),
//! a ticket's own screen, cancelling one (`c`: the only state the user sets by hand; the squad sets the others), the
//! lists of its folders and what it waits for, and making a new one. The data itself is in `tickets.rs`; editing a body goes through the notes' editor flow
//! (`notes_glue.rs`).

use super::*;
use crate::tickets::{self, State, Ticket, Tickets};

/// A row of the Tickets pane.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TRow {
    Ticket(u32),
    /// "Done and cancelled · N": enter folds and unfolds what is under it.
    Divider(usize),
}

/// Which list is open over a ticket.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickKind {
    BlockedBy,
    Folders,
}

/// One entry of such a list.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PickOpt {
    Blocker(u32),
    Folder(String),
    AddFolder,
}

/// A list open over a ticket: `kind` for ticket `number`.
pub struct TicketPick {
    pub kind: PickKind,
    pub number: u32,
    pub selected: usize,
}

/// Everything about tickets the app keeps.
pub struct TicketsState {
    /// The open project's tickets (None while no project's screen is showing).
    pub store: Option<Tickets>,
    /// The row picked in the Tickets pane, by what it is (so it stays on the same ticket when the list changes).
    pub selected: Option<TRow>,
    /// "Done and cancelled" is showing its tickets.
    pub unfolded: bool,
    /// The ticket whose own screen is showing.
    pub open: Option<u32>,
    /// The first drawn line of the open ticket's body in view.
    pub body_top: Cell<usize>,
    pub pick: TicketPick,
}

impl TicketsState {
    pub fn new() -> Self {
        Self { store: None, selected: None, unfolded: false, open: None, body_top: Cell::new(0), pick: TicketPick { kind: PickKind::Folders, number: 0, selected: 0 } }
    }
}

fn plain(e: &io::Error) -> String {
    let why = e.to_string();
    why.split(" (os error").next().unwrap_or(&why).to_string()
}

impl App {
    /// The open project's tickets, in the order of the file (empty while none is open).
    pub fn tickets(&self) -> &[Ticket] {
        self.tk.store.as_ref().map_or(&[], |s| s.all())
    }

    pub fn tickets_store(&self) -> Option<&Tickets> {
        self.tk.store.as_ref()
    }

    pub fn ticket(&self, number: u32) -> Option<&Ticket> {
        self.tk.store.as_ref().and_then(|s| s.get(number))
    }

    /// The rows of the Tickets pane: the open tickets, then the divider, then (when unfolded) the Done and
    /// Cancelled ones.
    pub fn ticket_rows(&self) -> Vec<TRow> {
        let (open, closed) = tickets::grouped(self.tickets());
        let mut rows: Vec<TRow> = open.iter().map(|t| TRow::Ticket(t.number)).collect();
        if !closed.is_empty() {
            rows.push(TRow::Divider(closed.len()));
            if self.tk.unfolded {
                rows.extend(closed.iter().map(|t| TRow::Ticket(t.number)));
            }
        }
        rows
    }

    /// Index into `ticket_rows()` of the picked row.
    pub fn ticket_selected(&self) -> usize {
        let rows = self.ticket_rows();
        let divider = || rows.iter().position(|r| matches!(r, TRow::Divider(_)));
        match self.tk.selected {
            Some(TRow::Divider(_)) => divider().unwrap_or(0),
            // A ticket that went under the folded divider: the divider.
            Some(sel) => rows.iter().position(|r| *r == sel).or_else(divider).unwrap_or(0),
            None => 0,
        }
    }

    /// The ticket the Tickets pane has picked, if the picked row is a ticket.
    pub fn selected_ticket(&self) -> Option<&Ticket> {
        match self.ticket_rows().get(self.ticket_selected()) {
            Some(TRow::Ticket(n)) => self.ticket(*n),
            _ => None,
        }
    }

    fn select_ticket_row(&mut self, i: usize) {
        self.tk.selected = self.ticket_rows().get(i).copied();
    }

    fn move_ticket(&mut self, delta: i32) {
        let n = self.ticket_rows().len();
        if n == 0 {
            return;
        }
        let at = (self.ticket_selected() as i32 + delta).clamp(0, n as i32 - 1) as usize;
        self.select_ticket_row(at);
    }

    /// Open the project's tickets (called with the project's screen).
    pub(super) fn open_tickets(&mut self, project: &str) {
        self.tk = TicketsState::new();
        self.tk.store = Some(Tickets::open(&self.config, project));
        self.left_pane = Pane::Sessions;
    }

    pub(super) fn close_tickets(&mut self) {
        self.tk = TicketsState::new();
    }

    /// Pick up a ticket added by `platoon ticket add` (or changed in another window). One `stat` when nothing changed.
    pub(super) fn refresh_tickets(&mut self) {
        let open = self.open_project.clone();
        match (&mut self.tk.store, open) {
            (Some(s), Some(p)) if s.project() == p => {
                s.reload_if_changed();
            }
            (_, Some(p)) => self.open_tickets(&p),
            (Some(_), None) => self.close_tickets(),
            (None, None) => {}
        }
        if self.tk.open.is_some_and(|n| self.ticket(n).is_none()) {
            self.tk.open = None;
        }
        // Tickets being handed to a squad show Working, with what is happening (a store opened again forgets).
        self.show_starting();
    }

    /// Give a pane of the project's screen the focus. The left column remembers which of its two was used last.
    pub(super) fn focus_pane(&mut self, p: Pane) {
        self.pane = p;
        if p != Pane::Notes {
            self.left_pane = p;
        }
    }

    /// Tab and Shift+Tab on a project's screen: Sessions, Tickets, Notes, round again.
    pub(super) fn cycle_pane(&mut self, delta: i32) {
        const ORDER: [Pane; 3] = [Pane::Sessions, Pane::Tickets, Pane::Notes];
        let at = ORDER.iter().position(|p| *p == self.pane).unwrap_or(0) as i32;
        self.focus_pane(ORDER[(at + delta).rem_euclid(3) as usize]);
    }

    fn report_tickets_error(&mut self, e: io::Error) {
        self.notify(format!("Couldn't save the tickets: {}", plain(&e)));
    }

    // ---- keys -------------------------------------------------------------------------------------------------

    /// Keys of the Tickets pane (the ones it shares with the other panes are handled before this).
    pub(super) fn tickets_pane_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('n') => self.start_new_ticket(),
            KeyCode::Enter => match self.ticket_rows().get(self.ticket_selected()) {
                Some(TRow::Divider(_)) => self.tk.unfolded = !self.tk.unfolded,
                Some(TRow::Ticket(n)) => {
                    let n = *n;
                    self.open_ticket_screen(n);
                }
                None => {}
            },
            KeyCode::Down => self.move_ticket(1),
            KeyCode::Up => self.move_ticket(-1),
            KeyCode::PageDown => self.move_ticket(5),
            KeyCode::PageUp => self.move_ticket(-5),
            KeyCode::Char('g') | KeyCode::Home => self.select_ticket_row(0),
            KeyCode::Char('G') | KeyCode::End => self.select_ticket_row(self.ticket_rows().len().saturating_sub(1)),
            _ => {
                if let Some(n) = self.selected_ticket().map(|t| t.number) {
                    self.ticket_key(n, key);
                }
            }
        }
    }

    /// The keys that act on one ticket, the same on its screen and on its row.
    fn ticket_key(&mut self, number: u32, key: KeyEvent) {
        // While it is being handed to a squad, what would change that waits.
        if let Some(busy) = self.ticket(number).and_then(|t| t.busy.clone()) {
            if matches!(key.code, KeyCode::Char('c' | 'f' | 'a' | 'p' | 'w')) {
                self.notify(format!("#{number}: {busy} Wait a moment."));
                return;
            }
        }
        match key.code {
            KeyCode::Char('e') => self.edit_ticket(number),
            KeyCode::Char('r') => self.open_name_box(NamePurpose::TicketTitle(number)),
            KeyCode::Char('c') => self.ask_cancel_ticket(number),
            KeyCode::Char('f') => self.open_ticket_pick(PickKind::Folders, number),
            KeyCode::Char('b') => self.open_ticket_pick(PickKind::BlockedBy, number),
            KeyCode::Char('a') => self.ask_hand_to_squad(number),
            KeyCode::Char('p') => self.let_it_publish(number),
            KeyCode::Char('w') => self.ask_remove_worktree(number),
            _ => {}
        }
    }

    /// Keys on a ticket's own screen.
    pub(super) fn ticket_screen_key(&mut self, key: KeyEvent) {
        let Some(number) = self.tk.open else { return };
        let scroll = |app: &mut App, d: i64| app.tk.body_top.set((app.tk.body_top.get() as i64 + d).max(0) as usize);
        match key.code {
            KeyCode::Esc => self.close_ticket_screen(),
            KeyCode::Down => scroll(self, 1),
            KeyCode::Up => scroll(self, -1),
            KeyCode::PageDown => scroll(self, 10),
            KeyCode::PageUp => scroll(self, -10),
            KeyCode::Char('g') | KeyCode::Home => self.tk.body_top.set(0),
            // A ticket with a squad: its session, beside it (continued, when it isn't running).
            KeyCode::Enter if self.ticket(number).is_some_and(|t| t.leader().is_some()) => self.open_leader(number),
            // One made from a session (the "From" line): that session, the way enter opens it on the Sessions tab.
            // `o` does the same when enter is the squad's.
            KeyCode::Enter | KeyCode::Char('o') if self.ticket(number).is_some_and(|t| t.from.is_some()) => self.open_ticket_source(number),
            _ => self.ticket_key(number, key),
        }
    }

    /// The key that opens the session a ticket came from, on its screen: enter, unless enter is its squad's session.
    pub fn ticket_source_key(&self, t: &Ticket) -> &'static str {
        if t.leader().is_some() { "o" } else { "enter" }
    }

    /// How a ticket's screen names the session it came from: its name as the Sessions tab shows it, else its first
    /// prompt (or Claude's title for it), else its short id.
    pub fn ticket_source_name(&self, from: &tickets::Source) -> String {
        match self.sessions.iter().find(|s| s.id == from.session) {
            Some(s) if s.name_source != crate::model::NameSource::Folder => s.name.clone(),
            Some(s) if !s.description.trim().is_empty() => s.description.split_whitespace().collect::<Vec<_>>().join(" "),
            _ => crate::claude::short_id(&from.session),
        }
    }

    /// Open the session ticket `number` came from, the way enter on the Sessions tab does.
    fn open_ticket_source(&mut self, number: u32) {
        let Some(from) = self.ticket(number).and_then(|t| t.from.clone()) else { return };
        if self.sessions.iter().any(|s| s.id == from.session) {
            self.jump_to(&from.session);
        } else {
            self.notify(format!("The conversation #{number} came from ({}) isn't in platoon's list any more.", crate::claude::short_id(&from.session)));
        }
    }

    pub(super) fn open_ticket_screen(&mut self, number: u32) {
        if self.ticket(number).is_none() {
            return;
        }
        self.tk.open = Some(number);
        self.tk.body_top.set(0);
        // A ticket whose squad is at work opens beside its live session. One whose session isn't running shows the
        // ticket with the way to continue it (enter), rather than starting it by itself.
        if self.leader_running(number) && self.ticket(number).is_some_and(|t| t.queued.is_none()) {
            self.open_leader(number);
        }
    }

    /// Esc on a ticket's screen: back to the project's screen, with the same ticket picked.
    pub(super) fn close_ticket_screen(&mut self) {
        let Some(n) = self.tk.open.take() else { return };
        self.focus_pane(Pane::Tickets);
        self.show_ticket_row(n);
    }

    /// Pick `n`'s row, unfolding "Done and cancelled" if that is where it is.
    fn show_ticket_row(&mut self, n: u32) {
        if self.ticket(n).is_some_and(|t| t.state.closed()) {
            self.tk.unfolded = true;
        }
        self.tk.selected = Some(TRow::Ticket(n));
    }

    pub(super) fn wheel_ticket(&mut self, up: bool) {
        if self.tk.open.is_some() {
            let d: i64 = if up { -3 } else { 3 };
            self.tk.body_top.set((self.tk.body_top.get() as i64 + d).max(0) as usize);
        } else {
            self.move_ticket(if up { -1 } else { 1 });
        }
    }

    /// A click on a row of the Tickets pane: picks it; a click on the picked one is enter.
    pub(super) fn click_ticket_row(&mut self, i: usize) {
        if self.pane == Pane::Tickets && i == self.ticket_selected() {
            self.tickets_pane_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        } else {
            self.focus_pane(Pane::Tickets);
            self.select_ticket_row(i);
        }
    }

    // ---- a new ticket -----------------------------------------------------------------------------------------

    /// `n`: the name box for a new ticket, unless the project is archived (it takes no new tickets; the ones it has
    /// can still be opened and changed).
    fn start_new_ticket(&mut self) {
        if !self.refuse_ticket_if_archived() {
            self.open_name_box(NamePurpose::NewTicket);
        }
    }

    /// The open project is archived: say so (the same line as `platoon ticket add`) and return true.
    fn refuse_ticket_if_archived(&mut self) -> bool {
        let Some(name) = self.open_project().filter(|p| p.archived).map(|p| p.name.clone()) else { return false };
        self.notify(tickets::archived_refusal(&name));
        true
    }

    /// Enter in the name box for a new ticket: the next number, the template body (which says how far), the folder
    /// of the project's most recently used session (none when it has no sessions), New. Then the body opens in the
    /// editor, the same way the notes do.
    pub(super) fn make_ticket(&mut self, title: &str) {
        let Some(project) = self.open_project.clone() else { return };
        if self.refuse_ticket_if_archived() {
            return;
        }
        let folders: Vec<String> = self.project_folders(&project).into_iter().take(1).collect();
        let Some(store) = self.tk.store.as_mut() else { return };
        match store.add(tickets::NewTicket { title: title.to_string(), folders, ..Default::default() }) {
            Ok(n) => {
                self.focus_pane(Pane::Tickets);
                self.show_ticket_row(n);
                self.notify(format!("Made ticket #{n}"));
                self.pick_ticket_editor(n);
            }
            Err(e) => self.report_tickets_error(e),
        }
    }

    pub(super) fn rename_ticket(&mut self, number: u32, title: &str) {
        let Some(store) = self.tk.store.as_mut() else { return };
        match store.rename(number, title) {
            Ok(()) => self.notify(format!("Renamed #{number}")),
            Err(e) => self.report_tickets_error(e),
        }
    }

    // ---- the lists over a ticket ------------------------------------------------------------------------------

    pub(super) fn open_ticket_pick(&mut self, kind: PickKind, number: u32) {
        if self.ticket(number).is_none() {
            return;
        }
        if kind == PickKind::BlockedBy && self.tickets().len() < 2 {
            self.notify("There are no other tickets in this project to wait for.");
            return;
        }
        self.tk.pick = TicketPick { kind, number, selected: 0 };
        self.mode = Mode::TicketPick;
    }

    /// The entries of the list that is open, and whether each is what the ticket has now.
    pub fn ticket_pick_options(&self) -> Vec<(PickOpt, bool)> {
        let p = &self.tk.pick;
        let Some(t) = self.ticket(p.number) else { return Vec::new() };
        match p.kind {
            PickKind::BlockedBy => {
                let mut others: Vec<&Ticket> = self.tickets().iter().filter(|o| o.number != t.number).collect();
                others.sort_by_key(|o| o.number);
                others.iter().map(|o| (PickOpt::Blocker(o.number), t.blocked_by.contains(&o.number))).collect()
            }
            PickKind::Folders => {
                let mut v: Vec<(PickOpt, bool)> = t.folders.iter().map(|f| (PickOpt::Folder(f.clone()), false)).collect();
                v.push((PickOpt::AddFolder, false));
                v
            }
        }
    }

    pub(super) fn move_ticket_pick(&mut self, delta: i32) {
        let last = self.ticket_pick_options().len().saturating_sub(1) as i32;
        self.tk.pick.selected = (self.tk.pick.selected as i32 + delta).clamp(0, last) as usize;
    }

    pub(super) fn ticket_pick_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.act(Action::Cancel),
            KeyCode::Char('c') if ctrl => self.act(Action::Cancel),
            KeyCode::Enter => self.act(Action::Confirm),
            KeyCode::Char(' ') if self.tk.pick.kind == PickKind::BlockedBy => self.act(Action::Confirm),
            KeyCode::Up => self.move_ticket_pick(-1),
            KeyCode::Down => self.move_ticket_pick(1),
            KeyCode::Char('g') | KeyCode::Home => self.tk.pick.selected = 0,
            KeyCode::Char('G') | KeyCode::End => self.tk.pick.selected = self.ticket_pick_options().len().saturating_sub(1),
            KeyCode::Char('d' | 'x') | KeyCode::Delete | KeyCode::Backspace if self.tk.pick.kind == PickKind::Folders => self.remove_ticket_folder(),
            _ => {}
        }
    }

    /// Enter in the list: take that choice. "What it waits for" stays open so several can be ticked; the folders list
    /// opens the folder picker for "Add a folder…".
    pub(super) fn take_ticket_pick(&mut self) {
        let number = self.tk.pick.number;
        let Some((opt, _)) = self.ticket_pick_options().get(self.tk.pick.selected).cloned() else {
            self.mode = Mode::List;
            return;
        };
        match opt {
            PickOpt::Blocker(b) => {
                let Some(store) = self.tk.store.as_mut() else { return };
                match store.toggle_blocker(number, b) {
                    Ok(true) => self.notify(format!("#{number} waits for #{b} now")),
                    Ok(false) => self.notify(format!("#{number} no longer waits for #{b}")),
                    Err(e) => self.notify(format!("Can't do that: {}", plain(&e))),
                }
            }
            PickOpt::Folder(_) => {}
            PickOpt::AddFolder => self.open_ticket_folder_picker(number),
        }
    }

    // ---- cancelling ---------------------------------------------------------------------------------------------

    /// `c`: cancel the ticket, after asking. The only state the user sets by hand: the squad sets the others, and a
    /// message to its leader makes the ticket Working again (`platoon hook squad-prompt`). When its leader is running
    /// the question says that session will be stopped too.
    fn ask_cancel_ticket(&mut self, number: u32) {
        let Some(t) = self.ticket(number) else { return };
        if t.state == State::Cancelled {
            self.notify(format!("#{number} is cancelled already."));
            return;
        }
        let (title, squad) = (t.title.clone(), t.leader().is_some());
        let pid = t.leader().map(|l| l.session.clone()).and_then(|s| self.leader_host(&s)).map(|h| h.claude_pid);
        let uncommitted = self.has_uncommitted_worktree(number);
        self.mode = Mode::Confirm(Confirm::CancelTicket { number, title, squad, pid, uncommitted });
    }

    /// Yes to "Cancel ticket #N?": Cancelled (out of the line for a free squad too, and the go-ahead goes), listed
    /// under Done and cancelled; and its leader's session stopped when it was running when asked.
    pub(super) fn cancel_ticket(&mut self, number: u32, pid: Option<u32>) {
        self.mode = Mode::List;
        let Some(store) = self.tk.store.as_mut() else { return };
        if let Err(e) = store.set_state(number, &State::Cancelled) {
            return self.report_tickets_error(e);
        }
        // Follow it on the pane: under the divider (the divider is picked while that is folded).
        self.tk.selected = Some(TRow::Ticket(number));
        match pid {
            Some(pid) if self.stop_squad(number, pid) => self.notify(format!("#{number} is Cancelled. Stopping its squad's session; its conversation is kept.")),
            _ => self.notify(format!("#{number} is Cancelled now. It is listed under Done and cancelled.")),
        }
    }

    fn remove_ticket_folder(&mut self) {
        let number = self.tk.pick.number;
        let Some((PickOpt::Folder(f), _)) = self.ticket_pick_options().get(self.tk.pick.selected).cloned() else { return };
        let Some(t) = self.ticket(number) else { return };
        let left: Vec<String> = t.folders.iter().filter(|x| **x != f).cloned().collect();
        let Some(store) = self.tk.store.as_mut() else { return };
        match store.set_folders(number, &left) {
            Ok(()) => {
                let shown = dirs::tilde(&f, &self.home);
                self.notify(format!("#{number} no longer works in {shown}"));
            }
            Err(e) => self.report_tickets_error(e),
        }
        self.move_ticket_pick(0);
    }

    /// The folder picker, to add a folder to ticket `number`: the project's folders first, as for a new session.
    fn open_ticket_folder_picker(&mut self, number: u32) {
        let project = self.open_project.clone();
        self.open_picker_in(project);
        self.picker.ticket = Some(number);
    }

    /// Enter in the folder picker opened from a ticket's folders.
    pub(super) fn add_ticket_folder(&mut self, number: u32, dir: String) {
        self.mode = Mode::TicketPick;
        let Some(t) = self.ticket(number) else { return };
        if t.folders.contains(&dir) {
            self.notify("That folder is already on the ticket.");
            return;
        }
        let mut list = t.folders.clone();
        list.push(dir.clone());
        let Some(store) = self.tk.store.as_mut() else { return };
        match store.set_folders(number, &list) {
            Ok(()) => {
                let shown = dirs::tilde(&dir, &self.home);
                self.notify(format!("#{number} works in {shown} too"));
            }
            Err(e) => self.report_tickets_error(e),
        }
    }

    // ---- the body ---------------------------------------------------------------------------------------------

    /// `e`: the editor list, then the body in the editor picked.
    pub(super) fn edit_ticket(&mut self, number: u32) {
        self.pick_ticket_editor(number);
    }
}
