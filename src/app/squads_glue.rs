//! Squads inside the app: handing a ticket to a squad (`a`, a small picker), starting its leader in a platoon host
//! (with a worktree for each git folder), the split view of a ticket beside its live leader session, continuing a
//! leader that isn't running, letting a squad publish (`p`), stopping a cancelled ticket's leader, and keeping
//! leader sessions out of every list of the user's own sessions (`leader_ids`, the one place that decides it).
//! The squads themselves and everything that can be worked out without the app are in `squads.rs`.

use super::*;
use crate::ai::Request;
use crate::instructions;
use crate::squads::{self, AfterCheck, Check, FolderFact, Launch, Place, Squad};
use crate::tickets::{AgentNames, HandoverKind, Leader, State, Ticket, Tickets, Worktree};
use std::collections::HashSet;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::SystemTime;

/// How long the commander's check may take before it is given up on (and nothing is handed over).
const COMMANDER_TIMEOUT: Duration = Duration::from_secs(60);

/// What is out while a ticket is being handed to a squad, off the UI thread, in this order: the commander's check,
/// then the worktrees. Then the leader starts (on the UI thread: it only waits for its host to be ready).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Checking,
    Worktrees,
}

impl Stage {
    /// What the ticket says meanwhile.
    pub fn words(self) -> &'static str {
        match self {
            Stage::Checking => squads::CHECKING,
            Stage::Worktrees => squads::MAKING_WORKTREES,
        }
    }
}

/// What a stage sends back.
enum Step {
    Checked(Check),
    Worktrees(Result<Vec<Worktree>, String>),
}

/// A ticket being handed to a squad.
struct Starting {
    project: String,
    number: u32,
    /// The squad as it was picked: the commander checks it, and its leader starts with it.
    squad: Squad,
    stage: Stage,
    rx: Receiver<Step>,
}

/// Below this many columns there is no room for a ticket beside its session: the session gets the width and the
/// ticket one line above it.
pub const SPLIT_MIN_WIDTH: u16 = 100;

/// What the split view is drawn in, for a terminal of `w` by `h` (the frame around the whole app included).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SplitAreas {
    /// The ticket beside the session, or (narrow) the one line above it.
    pub ticket: Rect,
    pub wide: bool,
    /// The column of the line between the two (wide only).
    pub rule_x: u16,
    /// The session's own screen.
    pub term: Rect,
    /// The status line under the session.
    pub bar: Rect,
}

pub fn split_areas(w: u16, h: u16) -> SplitAreas {
    let inner = Rect::new(1, 1, w.saturating_sub(2).max(1), h.saturating_sub(2).max(2));
    if w >= SPLIT_MIN_WIDTH {
        let left = inner.width * 2 / 5;
        let right = Rect::new(inner.x + left + 1, inner.y, inner.width.saturating_sub(left + 1).max(1), inner.height);
        SplitAreas {
            ticket: Rect::new(inner.x + 1, inner.y, left.saturating_sub(2).max(1), inner.height),
            wide: true,
            rule_x: inner.x + left,
            term: Rect::new(right.x, right.y, right.width, right.height.saturating_sub(1).max(1)),
            bar: Rect::new(right.x, right.bottom().saturating_sub(1), right.width, 1),
        }
    } else {
        SplitAreas {
            ticket: Rect::new(inner.x, inner.y, inner.width, 1),
            wide: false,
            rule_x: 0,
            term: Rect::new(inner.x, inner.y + 1, inner.width, inner.height.saturating_sub(2).max(1)),
            bar: Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1),
        }
    }
}

/// Everything about squads the app keeps.
pub struct SquadState {
    /// The ticket the squad picker is open for.
    pub pick_ticket: u32,
    /// The squads the picker lists (every one that can be handed a ticket, as they were when it opened), and the
    /// picked one.
    pub pick: Vec<Squad>,
    pub pick_selected: usize,
    /// The ticket whose leader the attached view is showing beside it (the split view).
    pub split: Option<u32>,
    /// Every leader session of every ticket: never shown with the user's own sessions.
    leaders: HashSet<String>,
    /// The tickets files `leaders` was read from, as they were then.
    stamps: Vec<(PathBuf, Option<(SystemTime, u64)>)>,
    /// Tickets being handed to a squad right now.
    starting: Vec<Starting>,
    /// Every ticket of every project, as the files said when `stamps` were taken: for the places taken and the line.
    all_tickets: Vec<(String, Ticket)>,
    /// The leader sessions running now (hosted by any platoon, or registered by Claude Code).
    running: HashSet<String>,
    /// This window starts the tickets in the line for a free squad while it holds this lock (`<config>/squad-line.lock`):
    /// with two platoon windows open only one does, so they never both fill the same free place.
    line_lock: Option<std::fs::File>,
    /// Tickets taken from the line and being started: they start without taking the screen (nobody asked just now).
    from_line: HashSet<(String, u32)>,
    /// In unit tests no session is ever started (the program would be the test binary): the leaders that would
    /// have been, by ticket number.
    #[cfg(test)]
    pub launched: Vec<u32>,
    /// ... and for each, the folder it would have started in and its arguments.
    #[cfg(test)]
    pub launched_with: Vec<(String, Vec<String>)>,
}

impl SquadState {
    pub fn new() -> Self {
        Self {
            pick_ticket: 0,
            pick: Vec::new(),
            pick_selected: 0,
            split: None,
            leaders: HashSet::new(),
            stamps: Vec::new(),
            starting: Vec::new(),
            all_tickets: Vec::new(),
            running: HashSet::new(),
            line_lock: None,
            from_line: HashSet::new(),
            #[cfg(test)]
            launched: Vec::new(),
            #[cfg(test)]
            launched_with: Vec::new(),
        }
    }
}

impl App {
    /// The sessions that are squads' leaders.
    #[cfg(test)]
    pub fn leader_ids(&self) -> &HashSet<String> {
        &self.sq.leaders
    }

    /// Read the leaders again if any project's tickets file changed (one `stat` per file when nothing did). True when
    /// the set changed.
    pub(super) fn refresh_leaders(&mut self) -> bool {
        let stamps: Vec<(PathBuf, Option<(SystemTime, u64)>)> = crate::tickets::all_files(&self.config)
            .into_iter()
            .map(|(_, p)| {
                let st = fs_stamp(&p);
                (p, st)
            })
            .collect();
        if stamps == self.sq.stamps {
            return false;
        }
        self.sq.stamps = stamps;
        self.sq.all_tickets = crate::tickets::every_ticket(&self.config);
        let ids: HashSet<String> = self.sq.all_tickets.iter().flat_map(|(_, t)| t.leaders.iter().map(|l| l.session.clone())).collect();
        if ids == self.sq.leaders {
            return false;
        }
        self.sq.leaders = ids;
        true
    }

    /// The platoon host running this leader session, if one is: the one started with its id (as a new session, or
    /// resumed).
    pub fn leader_host(&self, session: &str) -> Option<&HostInfo> {
        self.hosts.iter().find(|h| h.args.iter().any(|a| a == session))
    }

    /// Note which leader sessions Claude Code says are running (from the whole list, before leaders are left out).
    pub(super) fn note_running_leaders(&mut self, live: &[Live]) {
        self.sq.running = live.iter().filter(|l| self.sq.leaders_contains(&l.id)).map(|l| l.id.clone()).collect();
    }

    /// How many squads are at work now (see `squadqueue`): from the tickets as last read, and the tickets this window
    /// is starting (their leaders aren't written yet).
    pub fn squads_at_work(&self) -> usize {
        let now = usage::now();
        let taken = crate::squadqueue::places_taken(&self.sq.all_tickets, &|s| self.leader_session_running(s), now);
        // Past the commander's check, making its worktrees: its leader starts next.
        let starting = self.sq.starting.iter().filter(|st| st.stage == Stage::Worktrees).count();
        taken + starting
    }

    /// Ticket `number`'s current leader is running under platoon.
    pub fn leader_running(&self, number: u32) -> bool {
        self.ticket(number).and_then(|t| t.leader()).is_some_and(|l| self.leader_host(&l.session).is_some())
    }

    // ---- handing a ticket to a squad --------------------------------------------------------------------------

    /// `a` on a ticket: the squad picker, unless the ticket can't go to a squad now (said in one plain line).
    pub(super) fn ask_hand_to_squad(&mut self, number: u32) {
        let Some(t) = self.ticket(number) else { return };
        let why = if let Some(busy) = &t.busy {
            Some(format!("#{number}: {busy}"))
        } else if self.open_project().is_some_and(|p| p.archived) {
            Some(crate::tickets::archived_refusal(&self.open_project().map(|p| p.name.clone()).unwrap_or_default()))
        } else if t.queued.is_some() {
            Some(format!("#{number} is waiting for a free squad already. Change its state to take it out of the line."))
        } else if t.state != State::New {
            Some(format!("#{number} is {}. Only a New ticket can be handed to a squad.", t.state.label()))
        } else if let Some(first) = crate::tickets::waiting_for(t, self.tickets()).first() {
            Some(format!("#{number} waits for #{first}, which isn't Done yet."))
        } else {
            // No folder at all is fine for a squad that needs none: that is said when a squad is picked.
            t.folders.iter().find(|f| !Path::new(f).is_dir()).map(|f| format!("{} doesn't exist any more. Press f to change #{number}'s folders.", dirs::tilde(f, &self.home)))
        };
        if let Some(why) = why {
            self.notify(why);
            return;
        }
        // Every squad that can be read, by name; one that has something to fix says what when it is picked.
        let pick: Vec<Squad> = self.squad_list().iter().filter_map(|l| l.squad.clone()).collect();
        if pick.is_empty() {
            self.notify("There is no squad to hand it to: the squads' files can't be read (see the Squads tab).");
            return;
        }
        self.sq.pick = pick;
        self.sq.pick_ticket = number;
        self.sq.pick_selected = 0;
        self.mode = Mode::SquadPick;
    }

    pub(super) fn move_squad_pick(&mut self, delta: i32) {
        let last = self.sq.pick.len().saturating_sub(1) as i32;
        self.sq.pick_selected = (self.sq.pick_selected as i32 + delta).clamp(0, last) as usize;
    }

    pub(super) fn squad_pick_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.act(Action::Cancel),
            KeyCode::Char('c') if ctrl => self.act(Action::Cancel),
            KeyCode::Enter => self.act(Action::Confirm),
            KeyCode::Up => self.move_squad_pick(-1),
            KeyCode::Down => self.move_squad_pick(1),
            _ => {}
        }
    }

    /// Why ticket `number` can't go to `squad` because it has no folder, if that is so.
    fn needs_a_folder(&self, number: u32, squad: &Squad) -> Option<String> {
        let t = self.ticket(number)?;
        (squad.needs_folder && t.folders.is_empty()).then(|| format!("The {} squad needs a folder to work in, and #{number} has none. Press f to give it one.", squad.name))
    }

    /// Enter in the squad picker: the ticket goes to that squad.
    pub(super) fn take_squad_pick(&mut self) {
        let Some(squad) = self.sq.pick.get(self.sq.pick_selected).cloned() else { return };
        if let Some(first) = crate::squadfile::problems(&squad).into_iter().next() {
            // The picker stays open: another squad can be picked.
            self.notify(format!("The {} squad needs fixing on the Squads tab first: {first}", squad.name));
            return;
        }
        if let Some(why) = self.needs_a_folder(self.sq.pick_ticket, &squad) {
            // Here too: a squad that needs no folder can still be picked.
            self.notify(why);
            return;
        }
        self.mode = Mode::List;
        self.hand_to_squad(self.sq.pick_ticket, squad);
    }

    /// Hand the ticket to `squad`: the commander checks first. When the commander has already said this very squad
    /// doesn't fit, the user is asked whether to hand it over anyway (yes skips the check); another squad gets a
    /// check of its own.
    fn hand_to_squad(&mut self, number: u32, squad: Squad) {
        let Some(t) = self.ticket(number) else { return };
        if let Some(h) = t.handover.as_ref().filter(|h| h.squad == squad.id && h.kind == HandoverKind::Refused) {
            self.mode = Mode::Confirm(Confirm::HandAnyway { number, squad: squad.id.clone(), title: t.title.clone(), says: h.says.clone() });
            return;
        }
        self.ask_commander(number, squad);
    }

    /// Yes to "Hand it anyway?": the worktrees and the leader, without a new check.
    pub(super) fn hand_anyway(&mut self, number: u32, squad: &str) {
        self.mode = Mode::List;
        let (Some(squad), Some(project)) = (crate::squadfile::get(&self.config, squad), self.open_project.clone()) else { return };
        if !self.ticket(number).is_some_and(|t| t.state == State::New && t.busy.is_none()) {
            return;
        }
        if let Some(why) = self.needs_a_folder(number, &squad) {
            return self.notify(why);
        }
        self.notify(format!("Handing #{number} to the {} squad, as you said.", squad.name));
        self.start_or_queue(&project, number, squad, true);
    }

    /// The commander's check, off the UI thread (the ticket shows Working, "Commander is checking…"). If the gate
    /// says no there is no call at all: the ticket stays New, with why.
    fn ask_commander(&mut self, number: u32, squad: Squad) {
        let Some(project) = self.open_project.clone() else { return };
        self.sq.from_line.remove(&(project.clone(), number));
        let Some(t) = self.ticket(number).cloned() else { return };
        let settings = self.settings.get();
        // The user asked for this by pressing a key: the plan-usage pause doesn't apply, every other gate does.
        if let Err(r) = ai::gate_for(&settings, squads::COMMANDER_FEATURE, &self.cache, self.clock(), true) {
            self.after_commander(&project, number, squad, Check::Refused(r));
            return;
        }
        let body = self.tk.store.as_ref().map(|s| s.body(&t)).unwrap_or_default();
        let mut req = Request::new(squads::COMMANDER_FEATURE, squads::commander_system(&instructions::guidance(&self.config, instructions::COMMANDER)), String::new());
        req.user_initiated = true;
        req.model = Some(squads::COMMANDER_MODEL);
        req.timeout = COMMANDER_TIMEOUT;
        req.cancel = self.switchboard.token(squads::COMMANDER_FEATURE);
        let program = self.ai_program();
        let runner = self.ai_runner(&program);
        // A unit test that forgot to name a stand-in must never reach the real `claude`: the call is simply not made.
        let never = cfg!(test) && program == "claude";
        let (tx, rx) = channel();
        let checked = squad.clone();
        thread::spawn(move || {
            // Whether each folder is in git is looked up here too: a stat or two each, but on a network folder even
            // that can wait.
            let facts: Vec<FolderFact> = t.folders.iter().map(|f| FolderFact { path: f.clone(), git: squads::in_git(Path::new(f)) }).collect();
            req.prompt = squads::commander_prompt(&checked, &t, &body, &facts);
            let check = if never { Check::Failed("no stand-in for claude in this test".into()) } else { Check::from_answer(runner.run(&settings, &req)) };
            let _ = tx.send(Step::Checked(check));
        });
        self.begin_stage(&project, number, squad, Stage::Checking, rx);
    }

    fn begin_stage(&mut self, project: &str, number: u32, squad: Squad, stage: Stage, rx: Receiver<Step>) {
        self.sq.starting.retain(|s| !(s.project == project && s.number == number));
        self.sq.starting.push(Starting { project: project.to_string(), number, squad, stage, rx });
        self.show_starting();
    }

    /// The open project's tickets that are being handed over show Working, with what is happening.
    pub(super) fn show_starting(&mut self) {
        let Some(store) = self.tk.store.as_mut() else { return };
        let project = store.project().to_string();
        for st in self.sq.starting.iter().filter(|st| st.project == project) {
            store.set_busy(st.number, Some(st.stage.words()));
        }
    }

    fn done_starting(&mut self, project: &str, number: u32) {
        if let Some(store) = self.tk.store.as_mut().filter(|s| s.project() == project) {
            store.set_busy(number, None);
        }
    }

    /// True while a ticket is being handed to a squad (so the screen is refreshed quickly).
    pub fn squads_busy(&self) -> bool {
        !self.sq.starting.is_empty() || self.squad_drafting()
    }

    /// Take in what the stages sent back. Called on every refresh; cheap when nothing is out.
    pub(super) fn poll_squad_starts(&mut self) {
        let mut i = 0;
        while i < self.sq.starting.len() {
            let step = match self.sq.starting[i].rx.try_recv() {
                Ok(step) => step,
                Err(TryRecvError::Empty) => {
                    i += 1;
                    continue;
                }
                Err(TryRecvError::Disconnected) => match self.sq.starting[i].stage {
                    Stage::Checking => Step::Checked(Check::Failed("it stopped without an answer".into())),
                    Stage::Worktrees => Step::Worktrees(Err("it stopped without saying why".into())),
                },
            };
            let st = self.sq.starting.remove(i);
            self.done_starting(&st.project, st.number);
            match step {
                Step::Checked(check) => self.after_commander(&st.project, st.number, st.squad, check),
                Step::Worktrees(made) => self.after_worktrees(&st.project, st.number, st.squad, made),
            }
        }
    }

    /// A ticket as its file says now (not as the open project's screen shows it).
    fn ticket_on_disk(&self, project: &str, number: u32) -> Option<Ticket> {
        Tickets::open(&self.config, project).get(number).cloned()
    }

    /// Change the tickets of `project`: through the open screen's store when it is that project's, so the screen
    /// shows the change at once.
    fn with_tickets<T>(&mut self, project: &str, f: impl FnOnce(&mut Tickets) -> T) -> T {
        match self.tk.store.as_mut().filter(|s| s.project() == project) {
            Some(store) => f(store),
            None => f(&mut Tickets::open(&self.config, project)),
        }
    }

    /// Still New, with nobody else having moved it on while a stage was out? Otherwise say so: nothing goes on.
    fn still_new(&mut self, project: &str, number: u32) -> bool {
        let ok = self.ticket_on_disk(project, number).is_some_and(|t| t.state == State::New);
        if !ok {
            self.notify(format!("#{number} was changed meanwhile, so it wasn't handed over."));
        }
        ok
    }

    /// Nothing is handed over: the ticket stays New with why not on it, and a notice says the same.
    fn not_handed(&mut self, project: &str, number: u32, squad: &Squad, kind: HandoverKind, says: &str, notice: String) {
        self.sq.from_line.remove(&(project.to_string(), number));
        if let Err(e) = self.with_tickets(project, |s| s.handover_failed(number, &squad.id, kind, says)) {
            self.report_squad_error(e);
            return;
        }
        self.notify(notice);
    }

    fn after_commander(&mut self, project: &str, number: u32, squad: Squad, check: Check) {
        match squads::after_check(number, &squad.name, &check) {
            AfterCheck::Start => {
                if self.still_new(project, number) {
                    self.start_or_queue(project, number, squad, false);
                }
            }
            AfterCheck::BackToNew { says, kind, notice } => self.not_handed(project, number, &squad, kind, &says, notice),
        }
    }

    /// Read the tickets files again if they changed (the places taken and the line come from them).
    fn reread_tickets(&mut self) {
        if self.refresh_leaders() {
            self.built.0 = u64::MAX;
        }
    }

    /// The ticket may go to its squad (the commander said it fits, or the user said "hand it anyway"): it starts now
    /// if a place is free and nobody is in the line before it; otherwise it joins the line (Working, "waiting for a
    /// free squad") and starts by itself when a place frees, oldest first.
    fn start_or_queue(&mut self, project: &str, number: u32, squad: Squad, anyway: bool) {
        self.reread_tickets();
        let limit = self.settings.get().squads_at_once;
        let nobody_before = crate::squadqueue::line(&self.sq.all_tickets).is_empty();
        if nobody_before && crate::squadqueue::free_places(limit, self.squads_at_work()) > 0 {
            return self.make_worktrees(project, number, squad);
        }
        match self.with_tickets(project, |s| s.queue(number, &squad.id, anyway)) {
            Ok(()) => {
                self.reread_tickets();
                self.notify(crate::squadqueue::queued_notice(number, &squad.name, limit));
            }
            Err(e) => self.report_squad_error(e),
        }
    }

    /// Take this window's turn at starting the tickets in the line: only the platoon that holds the line's lock does
    /// (the first one opened; another takes over when it closes). True while this one holds it.
    fn hold_line_lock(&mut self) -> bool {
        if self.sq.line_lock.is_some() {
            return true;
        }
        use std::os::fd::AsRawFd;
        if std::fs::create_dir_all(&self.config).is_err() {
            return false;
        }
        let Ok(f) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(self.config.join("squad-line.lock")) else { return false };
        // SAFETY: a plain advisory lock, not waited for, on a file we hold open; released when it is dropped.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return false;
        }
        self.sq.line_lock = Some(f);
        true
    }

    /// Start the tickets waiting for a free squad, oldest first, while places are free. Called on every refresh;
    /// nothing is read when nobody is in the line. Each is taken out of the line under the tickets lock first, so a
    /// ticket is never started twice (by two windows, or twice by one).
    pub(super) fn start_from_line(&mut self) {
        let line = crate::squadqueue::line(&self.sq.all_tickets);
        if line.is_empty() || !self.hold_line_lock() {
            return;
        }
        let mut free = crate::squadqueue::free_places(self.settings.get().squads_at_once, self.squads_at_work());
        for (project, number, _) in line {
            if free == 0 {
                break;
            }
            let q = match self.with_tickets(&project, |s| s.claim_queued(number)) {
                Ok(Some(q)) => q,
                Ok(None) => continue,
                Err(e) => {
                    self.report_squad_error(e);
                    continue;
                }
            };
            free -= 1;
            let squad = crate::squadfile::get(&self.config, &q.squad);
            let broken = squad.as_ref().and_then(|s| crate::squadfile::problems(s).into_iter().next());
            match (squad, broken) {
                (Some(squad), None) => {
                    self.sq.from_line.insert((project.clone(), number));
                    self.make_worktrees(&project, number, squad);
                }
                (Some(squad), Some(first)) => {
                    let says = format!("The {} squad needs fixing on the Squads tab first: {first}", squad.name);
                    self.not_handed(&project, number, &squad, HandoverKind::NotStarted, &says, format!("#{number} didn't start: {says}"));
                }
                (None, _) => {
                    let says = format!("The squad \"{}\" isn't there any more, or its file can't be read.", q.squad);
                    let _ = self.with_tickets(&project, |s| s.handover_failed(number, &q.squad, HandoverKind::NotStarted, &says));
                    self.notify(format!("#{number} didn't start: {says}"));
                }
            }
        }
        self.reread_tickets();
    }

    /// A worktree for each git folder, off the UI thread (git can be slow on a big repository or a network folder);
    /// the ticket shows "Making the worktree…" meanwhile. A squad that never works in a worktree, or a ticket with no
    /// folder, goes straight on to its leader.
    fn make_worktrees(&mut self, project: &str, number: u32, squad: Squad) {
        let Some(t) = self.ticket_on_disk(project, number) else { return };
        if squad.needs_folder && t.folders.is_empty() {
            // Its folders were taken away while the commander checked.
            let says = format!("The {} squad needs a folder to work in, and #{number} has none.", squad.name);
            return self.not_handed(project, number, &squad, HandoverKind::NotStarted, &says, says.clone());
        }
        if !squad.use_worktree || t.folders.is_empty() {
            return self.after_worktrees(project, number, squad, Ok(Vec::new()));
        }
        let git = self.git_program.clone();
        let (tx, rx) = channel();
        thread::spawn(move || {
            let _ = tx.send(Step::Worktrees(squads::prepare_worktrees(&git, &t.folders, number)));
        });
        self.begin_stage(project, number, squad, Stage::Worktrees, rx);
    }

    /// The worktrees are there: write the new leader on the ticket (Working) and start it in a platoon host with the
    /// ticket as its first message, beside the ticket when its project's screen is still showing. A failure leaves
    /// the ticket New with the reason; worktrees already made stay (the next try uses them again), and a leader is
    /// only ever left written on a ticket when its session started.
    fn after_worktrees(&mut self, project: &str, number: u32, squad: Squad, made: Result<Vec<Worktree>, String>) {
        let worktrees = match made {
            Ok(w) => w,
            Err(e) => {
                let notice = format!("Couldn't hand #{number} to the {} squad: {e}", squad.name);
                self.not_handed(project, number, &squad, HandoverKind::NotStarted, &format!("Couldn't make the worktree: {e}"), notice);
                return;
            }
        };
        if !self.still_new(project, number) {
            return;
        }
        let session = squads::new_session_id();
        // Where it works: the worktrees just made, the folders as they are, or the ticket's own work folder.
        let place = match self.ticket_on_disk(project, number) {
            Some(mut t) => {
                t.worktrees = worktrees.clone();
                Place::of(squad.use_worktree, &t)
            }
            None => return,
        };
        let work = crate::tickets::work_dir(&self.config, project, number);
        if let Err(e) = std::fs::create_dir_all(&work) {
            let notice = format!("Couldn't start the squad: couldn't make the ticket's work folder ({e})");
            return self.not_handed(project, number, &squad, HandoverKind::NotStarted, &notice, notice.clone());
        }
        // What it starts with is kept beside the ticket: continued later, it runs with the squad as it is at this
        // moment, whatever is changed on the Squads tab meanwhile.
        let launch = Launch::of(&squad, AgentNames::SquadRole, place);
        let k = self.ticket_on_disk(project, number).map_or(0, |t| t.leaders.len()) + 1;
        let snapshot = match Tickets::open(&self.config, project).save_snapshot(number, k, &launch.to_json()) {
            Ok(name) => name,
            Err(e) => {
                let notice = format!("Couldn't start the squad: couldn't keep what it starts with ({e})");
                self.not_handed(project, number, &squad, HandoverKind::NotStarted, &notice, notice.clone());
                return;
            }
        };
        let leader = Leader { squad: squad.id.to_string(), session: session.clone(), started: usage::now(), names: AgentNames::SquadRole, snapshot: Some(snapshot) };
        if let Err(e) = self.with_tickets(project, |s| s.start_leader(number, &leader, &worktrees)) {
            self.report_squad_error(e);
            return;
        }
        // Written down before it starts, so the lists leave it out from its very first moment (and its hooks find it).
        if self.refresh_leaders() {
            self.built.0 = u64::MAX;
        }
        let Some(t) = self.ticket_on_disk(project, number) else { return };
        let body = Tickets::open(&self.config, project).body(&t);
        let project_name = self.projects.get(project).map(|p| p.name.clone()).unwrap_or_default();
        let first = squads::first_message(&t, &project_name, &body, place, &work.to_string_lossy());
        // Beside the ticket only if the user is still on that project's screen and not in another session, and not
        // for a ticket that waited in the line (it starts by itself, while the user may be doing anything).
        let from_line = self.sq.from_line.remove(&(project.to_string(), number));
        let show = !from_line && self.open_project.as_deref() == Some(project) && matches!(self.mode, Mode::List);
        match self.launch_leader_in(project, number, Some(&first), show) {
            Ok(()) if show => self.notify(format!("#{number} is with the {} squad now. ctrl+space goes back to the project.", squad.name)),
            Ok(()) if from_line => self.notify(format!("A squad is free: #{number} is with the {} squad now. Open it from {project_name}'s tickets.", squad.name)),
            Ok(()) => self.notify(format!("#{number} is with the {} squad now. Open it from {project_name}'s tickets.", squad.name)),
            Err(e) => {
                // Only a New ticket is ever handed to a squad: that is what it goes back to.
                let _ = self.with_tickets(project, |s| s.forget_leader(number, &session, &State::New));
                let notice = format!("Couldn't start the squad: {e}");
                self.not_handed(project, number, &squad, HandoverKind::NotStarted, &notice, notice.clone());
            }
        }
    }

    fn report_squad_error(&mut self, e: io::Error) {
        let why = e.to_string();
        self.notify(format!("Couldn't save the ticket: {}", why.split(" (os error").next().unwrap_or(&why)));
    }

    /// Continue ticket `number`'s current leader (the open project's) in a platoon host, beside the ticket. False,
    /// said in a notice, when it couldn't start.
    fn launch_leader(&mut self, number: u32, first: Option<&str>) -> bool {
        let Some(project) = self.open_project.clone() else { return false };
        match self.launch_leader_in(&project, number, first, true) {
            Ok(()) => true,
            Err(e) => {
                self.notify(format!("Couldn't start the squad: {e}"));
                false
            }
        }
    }

    /// Start ticket `number`'s current leader in a platoon host: a new session with `first` as its first message, or
    /// (None) the same session resumed. The same squad flags either way, and never `PLATOON_PROJECT`: a leader
    /// belongs to its ticket, not to the project. `show`: in the split view, beside the ticket.
    fn launch_leader_in(&mut self, project: &str, number: u32, first: Option<&str>, show: bool) -> Result<(), String> {
        let t = self.ticket_on_disk(project, number).ok_or_else(|| format!("#{number} isn't there any more"))?;
        let leader = t.leader().cloned().ok_or_else(|| format!("#{number} has no squad"))?;
        let launch = self.leader_launch(project, &leader, &t)?;
        // The work folder is made again if it has gone (a leader continued after it was deleted by hand).
        let work = crate::tickets::work_dir(&self.config, project, number);
        std::fs::create_dir_all(&work).map_err(|e| format!("couldn't make the ticket's work folder ({e})"))?;
        let (dir, add_dirs) = squads::start_dirs(&t, launch.work_folder, &work.to_string_lossy()).ok_or_else(|| format!("#{number} has no folder to work in"))?;
        if !Path::new(&dir).is_dir() {
            return Err(format!("{} doesn't exist any more", dirs::tilde(&dir, &self.home)));
        }
        let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "platoon".into());
        let args = squads::leader_args(&launch, &exe, &leader.session, &squads::session_name(&launch.name, &t), &add_dirs, first);
        // A unit test's program is the test binary itself: nothing is started, what would have been is noted.
        #[cfg(test)]
        {
            let _ = show;
            self.sq.launched.push(number);
            self.sq.launched_with.push((dir, args));
            Ok(())
        }
        #[cfg(not(test))]
        {
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            if show {
                self.tk.open = Some(number);
                self.sq.split = Some(number);
            }
            let (rows, cols) = self.pty_size();
            match host::launch(&self.program, &dir, &refs, &[], rows, cols) {
                Ok((info, child)) => {
                    let pid = info.claude_pid;
                    self.spawned.push(child);
                    self.hosts.push(info);
                    if show {
                        self.attach(pid);
                        if !matches!(self.mode, Mode::Attached(_)) {
                            self.sq.split = None;
                        }
                    }
                    Ok(())
                }
                Err(e) => {
                    if show {
                        self.sq.split = None;
                        self.mode = Mode::List;
                    }
                    Err(e)
                }
            }
        }
    }

    /// What a leader of `project` (of ticket `t`) is started (or continued) with: its snapshot, the squad as it was
    /// when it first started. A leader from before there were snapshots continues with its squad as it is now, under
    /// the names it began with.
    pub fn leader_launch(&self, project: &str, leader: &Leader, t: &Ticket) -> Result<Launch, String> {
        let store = Tickets::open(&self.config, project);
        if leader.snapshot.is_some() {
            return store
                .snapshot(leader)
                .and_then(|text| Launch::from_json(&text))
                .ok_or_else(|| "what it was started with can't be read any more".to_string());
        }
        let squad = crate::squadfile::get(&self.config, &leader.squad).ok_or_else(|| format!("the squad \"{}\" isn't there any more, or its file can't be read", leader.squad))?;
        Ok(Launch::of(&squad, leader.names, Place::of(squad.use_worktree, t)))
    }

    /// Enter on a ticket that has a leader: its session beside it, running or continued.
    pub(super) fn open_leader(&mut self, number: u32) {
        if self.ticket(number).is_some_and(|t| t.queued.is_some()) {
            self.notify(format!("#{number} is waiting for a free squad. It starts by itself when one is free."));
            return;
        }
        let Some(session) = self.ticket(number).and_then(|t| t.leader()).map(|l| l.session.clone()) else { return };
        self.tk.open = Some(number);
        match self.leader_host(&session).map(|h| h.claude_pid) {
            Some(pid) => {
                self.sq.split = Some(number);
                self.attach(pid);
                if !matches!(self.mode, Mode::Attached(_)) {
                    self.sq.split = None;
                }
            }
            None => {
                // Running somewhere platoon doesn't host (someone resumed it by hand): a second copy would fight it.
                if let Some(l) = self.reader.live().into_iter().find(|l| l.id == session) {
                    self.notify(format!("#{number}'s squad is running in another terminal (pid {}). Close it there first.", l.pid));
                    return;
                }
                self.launch_leader(number, None);
            }
        }
    }

    /// The split view is showing ticket `number` beside its session.
    pub fn split_showing(&self) -> Option<u32> {
        if matches!(self.mode, Mode::Attached(_)) { self.sq.split } else { None }
    }

    // ---- letting it publish -----------------------------------------------------------------------------------

    /// `p` on a ticket that is Ready to push/publish: the user's go-ahead is written on the ticket (Working again) and
    /// the leader is told to go on.
    pub(super) fn let_it_publish(&mut self, number: u32) {
        let Some(t) = self.ticket(number) else { return };
        if t.state != State::Ready {
            self.notify(format!("#{number} isn't waiting to publish anything."));
            return;
        }
        let session = t.leader().map(|l| l.session.clone()).unwrap_or_default();
        let Some(store) = self.tk.store.as_mut() else { return };
        if let Err(e) = store.give_go_ahead(number) {
            self.report_squad_error(e);
            return;
        }
        let Some(sock) = self.leader_host(&session).map(|h| h.sock.clone()) else {
            self.notify(format!("#{number} may publish now. Its squad's session isn't running: open it with enter and tell it to go ahead."));
            return;
        };
        let (rows, cols) = split_size(self.size);
        thread::spawn(move || type_into(&sock, rows, cols, "Go ahead and publish."));
        self.notify(format!("#{number} may publish now: platoon told its squad to go ahead."));
    }

    // ---- stopping a cancelled ticket's leader -----------------------------------------------------------------

    /// After the user made a ticket Cancelled: if its leader is running, ask whether to stop it too.
    pub(super) fn after_cancel(&mut self, number: u32) {
        let Some(session) = self.ticket(number).and_then(|t| t.leader()).map(|l| l.session.clone()) else { return };
        let Some(pid) = self.leader_host(&session).map(|h| h.claude_pid) else { return };
        let title = self.ticket(number).map(|t| t.title.clone()).unwrap_or_default();
        self.mode = Mode::Confirm(Confirm::StopSquad { number, title, pid });
    }

    /// Yes: stop only that leader's session (SIGTERM to the `claude` its host runs, on a thread of its own). Its
    /// conversation is kept, so choosing Working later continues it.
    pub(super) fn stop_squad(&mut self, number: u32, pid: u32) {
        self.mode = Mode::List;
        // The host of this ticket's leader, and nothing else.
        let Some(session) = self.ticket(number).and_then(|t| t.leader()).map(|l| l.session.clone()) else { return };
        if self.leader_host(&session).map(|h| h.claude_pid) != Some(pid) {
            return;
        }
        thread::spawn(move || {
            let _ = host::end_processes(&[pid]);
        });
        self.notify(format!("Stopping #{number}'s squad. Its conversation is kept."));
    }
}

fn fs_stamp(p: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// The size of the session's screen in the split view, for a terminal of this size.
pub fn split_size(size: (u16, u16)) -> (u16, u16) {
    let a = split_areas(size.0, size.1);
    (a.term.height, a.term.width)
}

/// Type `text` into a hosted session and press Enter, as a window of its own would: the words first, Enter a moment
/// later (sent together, Claude would take the Enter as part of a paste).
fn type_into(sock: &Path, rows: u16, cols: u16, text: &str) {
    use crate::host::{size_payload, write_frame, F_INPUT, F_RESIZE};
    let Ok(mut s) = std::os::unix::net::UnixStream::connect(sock) else { return };
    if write_frame(&mut s, F_RESIZE, &size_payload(rows, cols)).is_err() {
        return;
    }
    thread::sleep(Duration::from_millis(200));
    let _ = write_frame(&mut s, F_INPUT, text.as_bytes());
    thread::sleep(Duration::from_millis(400));
    let _ = write_frame(&mut s, F_INPUT, b"\r");
    thread::sleep(Duration::from_millis(200));
    let _ = s.shutdown(std::net::Shutdown::Both);
}

impl SquadState {
    /// Is this leader session registered as running by Claude Code?
    pub fn running_contains(&self, id: &str) -> bool {
        self.running.contains(id)
    }

    /// Is this session a squad's leader?
    pub fn leaders_contains(&self, id: &str) -> bool {
        !self.leaders.is_empty() && self.leaders.contains(id)
    }
}
