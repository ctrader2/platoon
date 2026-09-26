//! Squads inside the app: handing a ticket to a squad (`a`, a small picker), starting its leader in a platoon host
//! (with a worktree for each git folder), the split view of a ticket beside its live leader session, continuing a
//! leader that isn't running, letting a squad publish (`p`), stopping a cancelled ticket's leader, and keeping
//! leader sessions out of every list of the user's own sessions (`leader_ids`, the one place that decides it).
//! The squads themselves and everything that can be worked out without the app are in `squads.rs`.

use super::*;
use crate::ai::Request;
use crate::instructions;
use crate::squads::{self, AfterCheck, Check, FolderFact, Squad};
use crate::tickets::{HandoverKind, Leader, State, Ticket, Tickets, Worktree};
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
    squad: &'static Squad,
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
    /// The picked entry of `squads::SQUADS`.
    pub pick_selected: usize,
    /// The ticket whose leader the attached view is showing beside it (the split view).
    pub split: Option<u32>,
    /// Every leader session of every ticket: never shown with the user's own sessions.
    leaders: HashSet<String>,
    /// The tickets files `leaders` was read from, as they were then.
    stamps: Vec<(PathBuf, Option<(SystemTime, u64)>)>,
    /// Tickets being handed to a squad right now.
    starting: Vec<Starting>,
    /// In unit tests no session is ever started (the program would be the test binary): the leaders that would
    /// have been, by ticket number.
    #[cfg(test)]
    pub launched: Vec<u32>,
}

impl SquadState {
    pub fn new() -> Self {
        Self {
            pick_ticket: 0,
            pick_selected: 0,
            split: None,
            leaders: HashSet::new(),
            stamps: Vec::new(),
            starting: Vec::new(),
            #[cfg(test)]
            launched: Vec::new(),
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
        let ids = crate::tickets::leader_ids(&self.config);
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
        } else if t.state != State::New {
            Some(format!("#{number} is {}. Only a New ticket can be handed to a squad.", t.state.label()))
        } else if let Some(first) = crate::tickets::waiting_for(t, self.tickets()).first() {
            Some(format!("#{number} waits for #{first}, which isn't Done yet."))
        } else if t.folders.is_empty() {
            Some(format!("#{number} has no folder yet. Press f to give it one."))
        } else {
            t.folders.iter().find(|f| !Path::new(f).is_dir()).map(|f| format!("{} doesn't exist any more. Press f to change #{number}'s folders.", dirs::tilde(f, &self.home)))
        };
        if let Some(why) = why {
            self.notify(why);
            return;
        }
        self.sq.pick_ticket = number;
        self.sq.pick_selected = 0;
        self.mode = Mode::SquadPick;
    }

    pub(super) fn move_squad_pick(&mut self, delta: i32) {
        let last = squads::SQUADS.len().saturating_sub(1) as i32;
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

    /// Enter in the squad picker: the ticket goes to that squad.
    pub(super) fn take_squad_pick(&mut self) {
        self.mode = Mode::List;
        let Some(squad) = squads::SQUADS.get(self.sq.pick_selected).copied() else { return };
        self.hand_to_squad(self.sq.pick_ticket, squad);
    }

    /// Hand the ticket to `squad`: the commander checks first. When the commander has already said this very squad
    /// doesn't fit, the user is asked whether to hand it over anyway (yes skips the check); another squad gets a
    /// check of its own.
    fn hand_to_squad(&mut self, number: u32, squad: &'static Squad) {
        let Some(t) = self.ticket(number) else { return };
        if let Some(h) = t.handover.as_ref().filter(|h| h.squad == squad.id && h.kind == HandoverKind::Refused) {
            self.mode = Mode::Confirm(Confirm::HandAnyway { number, squad: squad.id, title: t.title.clone(), says: h.says.clone() });
            return;
        }
        self.ask_commander(number, squad);
    }

    /// Yes to "Hand it anyway?": the worktrees and the leader, without a new check.
    pub(super) fn hand_anyway(&mut self, number: u32, squad: &str) {
        self.mode = Mode::List;
        let (Some(squad), Some(project)) = (squads::find(squad), self.open_project.clone()) else { return };
        if !self.ticket(number).is_some_and(|t| t.state == State::New && t.busy.is_none()) {
            return;
        }
        self.notify(format!("Handing #{number} to the {} squad, as you said.", squad.name));
        self.make_worktrees(&project, number, squad);
    }

    /// The commander's check, off the UI thread (the ticket shows Working, "Commander is checking…"). If the gate
    /// says no there is no call at all: the ticket stays New, with why.
    fn ask_commander(&mut self, number: u32, squad: &'static Squad) {
        let Some(project) = self.open_project.clone() else { return };
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
        thread::spawn(move || {
            // Whether each folder is in git is looked up here too: a stat or two each, but on a network folder even
            // that can wait.
            let facts: Vec<FolderFact> = t.folders.iter().map(|f| FolderFact { path: f.clone(), git: squads::in_git(Path::new(f)) }).collect();
            req.prompt = squads::commander_prompt(squad, &t, &body, &facts);
            let check = if never { Check::Failed("no stand-in for claude in this test".into()) } else { Check::from_answer(runner.run(&settings, &req)) };
            let _ = tx.send(Step::Checked(check));
        });
        self.begin_stage(&project, number, squad, Stage::Checking, rx);
    }

    fn begin_stage(&mut self, project: &str, number: u32, squad: &'static Squad, stage: Stage, rx: Receiver<Step>) {
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
        !self.sq.starting.is_empty()
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
        if let Err(e) = self.with_tickets(project, |s| s.handover_failed(number, squad.id, kind, says)) {
            self.report_squad_error(e);
            return;
        }
        self.notify(notice);
    }

    fn after_commander(&mut self, project: &str, number: u32, squad: &'static Squad, check: Check) {
        match squads::after_check(number, squad, &check) {
            AfterCheck::Start => {
                if self.still_new(project, number) {
                    self.make_worktrees(project, number, squad);
                }
            }
            AfterCheck::BackToNew { says, kind, notice } => self.not_handed(project, number, squad, kind, &says, notice),
        }
    }

    /// A worktree for each git folder, off the UI thread (git can be slow on a big repository or a network folder);
    /// the ticket shows "Making the worktree…" meanwhile.
    fn make_worktrees(&mut self, project: &str, number: u32, squad: &'static Squad) {
        let Some(t) = self.ticket_on_disk(project, number) else { return };
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
    fn after_worktrees(&mut self, project: &str, number: u32, squad: &'static Squad, made: Result<Vec<Worktree>, String>) {
        let worktrees = match made {
            Ok(w) => w,
            Err(e) => {
                let notice = format!("Couldn't hand #{number} to the {} squad: {e}", squad.name);
                self.not_handed(project, number, squad, HandoverKind::NotStarted, &format!("Couldn't make the worktree: {e}"), notice);
                return;
            }
        };
        if !self.still_new(project, number) {
            return;
        }
        let session = squads::new_session_id();
        let leader = Leader { squad: squad.id.to_string(), session: session.clone(), started: usage::now() };
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
        let first = squads::first_message(&t, &project_name, &body);
        // Beside the ticket only if the user is still on that project's screen and not in another session.
        let show = self.open_project.as_deref() == Some(project) && matches!(self.mode, Mode::List);
        match self.launch_leader_in(project, number, Some(&first), show) {
            Ok(()) if show => self.notify(format!("#{number} is with the {} squad now. ctrl+space goes back to the project.", squad.name)),
            Ok(()) => self.notify(format!("#{number} is with the {} squad now. Open it from {project_name}'s tickets.", squad.name)),
            Err(e) => {
                // Only a New ticket is ever handed to a squad: that is what it goes back to.
                let _ = self.with_tickets(project, |s| s.forget_leader(number, &session, &State::New));
                let notice = format!("Couldn't start the squad: {e}");
                self.not_handed(project, number, squad, HandoverKind::NotStarted, &notice, notice.clone());
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
        let squad = squads::find(&leader.squad).unwrap_or(&squads::CODING);
        let folders = t.work_folders();
        let dir = folders.first().cloned().ok_or_else(|| format!("#{number} has no folder to work in"))?;
        if !Path::new(&dir).is_dir() {
            return Err(format!("{} doesn't exist any more", dirs::tilde(&dir, &self.home)));
        }
        let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "platoon".into());
        let args = squads::leader_args(squad, &self.config, &exe, &leader.session, &squads::session_name(&t), &folders[1..], first);
        // A unit test's program is the test binary itself: nothing is started, what would have been is noted.
        #[cfg(test)]
        {
            let _ = (args, show);
            self.sq.launched.push(number);
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

    /// Enter on a ticket that has a leader: its session beside it, running or continued.
    pub(super) fn open_leader(&mut self, number: u32) {
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
    /// Is this session a squad's leader?
    pub fn leaders_contains(&self, id: &str) -> bool {
        !self.leaders.is_empty() && self.leaders.contains(id)
    }
}
