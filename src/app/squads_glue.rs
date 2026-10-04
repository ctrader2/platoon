//! Squads inside the app: handing a task to a squad (`S`: the hand-over popup, with the squads by name, how much the
//! squad does on its own and, per git folder, how it works there and where the work lands), starting its leader in a
//! platoon host (with a copy of each git folder: a worktree, or a new branch in the user's own checkout when the task
//! says so), the split view of a task beside its live leader session, continuing a leader that isn't running, letting
//! a squad publish (`p`), stopping a cancelled task's leader, and keeping leader sessions out of every list of the
//! user's own sessions (`leader_ids`, the one place that decides it). The squads themselves and everything that can be
//! worked out without the app are in `squads.rs`.

use super::*;
use crate::ai::Request;
use crate::instructions;
use crate::squads::{self, AfterCheck, Check, FolderFact, Launch, LeaderSetup, Place, Squad, Terms, WorkPlan};
use crate::tickets::{AgentNames, Autonomy, HandoverKind, How, Leader, State, Ticket, Tickets, WorksOn, Worktree};
use std::collections::HashSet;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::SystemTime;

/// How long the commander's check may take before it is given up on (and nothing is handed over).
const COMMANDER_TIMEOUT: Duration = Duration::from_secs(60);

/// How long the hand-over popup gives git, all its folders together, to say each repository's default branch and
/// whether its checkout is clean (on the UI thread, once, when the popup opens).
const HAND_BUDGET: Duration = Duration::from_millis(2500);

/// One of the task's folders in the hand-over popup: how the squad works there, and the branch the work lands on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderLine {
    pub folder: String,
    /// Its name, as the line calls it ("shop").
    pub name: String,
    /// In a git repository (else the line only says the folder is used as it is).
    pub git: bool,
    pub how: How,
    pub lands_on: String,
    /// Why "a new branch in your checkout" isn't offered for it just now (None: it is).
    pub no_branch: Option<String>,
}

impl FolderLine {
    /// The line's words after its name: "a copy on task/5, lands on main".
    pub fn words(&self, branch: &str) -> String {
        if !self.git {
            return "used as it is (not in git)".into();
        }
        let how = match self.how {
            How::Worktree => format!("a copy on {branch}"),
            How::Branch => format!("a new branch {branch} in your checkout"),
        };
        if self.lands_on.is_empty() { how } else { format!("{how}, lands on {}", self.lands_on) }
    }
}

/// The hand-over popup's lines as the user has them (the squads are `SquadState::pick`): how much the squad does on
/// its own, one line per folder, and the row the cursor is on (the squads first, then "On its own", then the folders).
#[derive(Clone, Debug, Default)]
pub struct HandForm {
    pub autonomy: Autonomy,
    pub folders: Vec<FolderLine>,
    pub cursor: usize,
    /// The task's branch, as the lines name it.
    pub branch: String,
}

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

/// What the split view is drawn in, for a terminal of `w` by `h` (there is no frame around the app: the session reaches
/// the right edge and the bottom, with only its status line under it).
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
    let all = Rect::new(0, 0, w.max(1), h.max(2));
    if w >= SPLIT_MIN_WIDTH {
        let left = all.width * 2 / 5;
        let right = Rect::new(left + 1, 0, all.width.saturating_sub(left + 1).max(1), all.height);
        SplitAreas {
            // A column of air at the left edge and before the line between the two.
            ticket: Rect::new(1, 0, left.saturating_sub(2).max(1), all.height),
            wide: true,
            rule_x: left,
            term: Rect::new(right.x, right.y, right.width, right.height.saturating_sub(1).max(1)),
            bar: Rect::new(right.x, right.bottom().saturating_sub(1), right.width, 1),
        }
    } else {
        SplitAreas {
            ticket: Rect::new(1, 0, all.width.saturating_sub(2).max(1), 1),
            wide: false,
            rule_x: 0,
            term: Rect::new(0, 1, all.width, all.height.saturating_sub(2).max(1)),
            bar: Rect::new(0, all.bottom().saturating_sub(1), all.width, 1),
        }
    }
}

/// Everything about squads the app keeps.
pub struct SquadState {
    /// The task the hand-over popup is open for.
    pub pick_ticket: u32,
    /// The squads the popup lists (every one that can be handed a task, as they were when it opened), and the
    /// picked one.
    pub pick: Vec<Squad>,
    pub pick_selected: usize,
    /// The popup's other lines.
    pub form: HandForm,
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
    /// Tasks handed over one after the other (just made from a conversation): they start without taking the screen
    /// either, since the next hand-over may be up by then.
    pub(super) quiet: HashSet<(String, u32)>,
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
            form: HandForm::default(),
            split: None,
            leaders: HashSet::new(),
            stamps: Vec::new(),
            starting: Vec::new(),
            all_tickets: Vec::new(),
            running: HashSet::new(),
            line_lock: None,
            from_line: HashSet::new(),
            quiet: HashSet::new(),
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

    /// Every ticket of every project, as the files said when they were last read.
    pub(super) fn every_ticket(&self) -> &[(String, Ticket)] {
        &self.sq.all_tickets
    }

    /// Read every project's tickets again if a file changed.
    pub(super) fn reread_all_tickets(&mut self) {
        self.reread_tickets();
        self.refresh_tickets();
    }

    /// Ticket `number`'s current leader is running under platoon.
    pub fn leader_running(&self, number: u32) -> bool {
        self.ticket(number).and_then(|t| t.leader()).is_some_and(|l| self.leader_host(&l.session).is_some())
    }

    // ---- handing a task to a squad ----------------------------------------------------------------------------

    /// `S` on a task: the hand-over popup, unless the task can't go to a squad now (said in one plain line).
    pub(super) fn ask_hand_to_squad(&mut self, number: u32) {
        let Some(t) = self.ticket(number).cloned() else { return };
        let why = if let Some(busy) = &t.busy {
            Some(format!("#{number}: {busy}"))
        } else if let Some(p) = self.task_scope().and_then(|id| self.projects.get(&id)).filter(|p| p.archived) {
            Some(crate::tickets::archived_refusal(&p.name))
        } else if t.queued.is_some() {
            Some(format!("#{number} is waiting for a free squad already. To take it out of the line, cancel it with c."))
        } else if t.state != State::New {
            Some(format!("#{number} is {}. Only a task that hasn't started can be handed to a squad.", t.state.label()))
        } else if let Some(first) = crate::tickets::waiting_for(&t, self.tickets()).first() {
            Some(format!("#{number} waits for #{first}, which isn't Done yet."))
        } else if let Some(f) = t.folders.iter().find(|f| !Path::new(f).is_dir()) {
            Some(format!("{} doesn't exist any more. Press f to change #{number}'s folders.", dirs::tilde(f, &self.home)))
        } else {
            // No folder at all is fine: the squad works in the task's own folder then. A folder not in git takes one
            // task at a time: two squads in one folder would write over each other.
            self.folder_taken(number, &t)
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
        self.sq.form = self.hand_form(number, &t);
        self.sq.pick = pick;
        self.sq.pick_ticket = number;
        self.sq.pick_selected = 0;
        self.mode = Mode::SquadPick;
    }

    /// A folder of `t` that isn't in git and that another open task (Working or Needs you, in any project) already
    /// works in: the refusal, naming that task.
    fn folder_taken(&self, number: u32, t: &Ticket) -> Option<String> {
        let mine = self.task_scope().unwrap_or_default();
        for f in t.folders.iter().filter(|f| !squads::in_git(Path::new(f))) {
            let other = self
                .every_ticket()
                .iter()
                .find(|(p, o)| !(*p == mine && o.number == number) && crate::tickets::is_open(o) && o.folders.contains(f));
            if let Some((p, o)) = other {
                let whose = if *p == mine { String::new() } else { format!(" (in {})", self.project_name_of(p)) };
                return Some(format!(
                    "#{}{whose} already works in {}, which isn't in git: one task at a time there. Wait until it is done.",
                    o.number,
                    squads::folder_name(f)
                ));
            }
        }
        None
    }

    /// A project's name for a notice, by id (the tasks in no project: "no project").
    fn project_name_of(&self, id: &str) -> String {
        if id == crate::tickets::NO_PROJECT_ID {
            return "no project".into();
        }
        self.projects.get(id).map(|p| p.name.clone()).unwrap_or_else(|| id.to_string())
    }

    /// The hand-over popup's lines for task `t`, pre-filled: how much the squad does on its own as the task says (else
    /// the project's default), and for each git folder how it works there and what the work lands on as the task says
    /// (else a copy, and the repository's default branch, found read-only), with "a new branch in your checkout" offered
    /// only when nothing else works in that checkout.
    fn hand_form(&self, number: u32, t: &Ticket) -> HandForm {
        let deadline = Instant::now() + HAND_BUDGET;
        let git = self.git_program.clone();
        let scope = self.task_scope().unwrap_or_default();
        let autonomy =
            t.autonomy.clone().unwrap_or_else(|| self.projects.get(&scope).map(|p| p.autonomy_default.clone()).unwrap_or_default());
        let folders = t
            .folders
            .iter()
            .map(|f| {
                let name = squads::folder_name(f);
                let in_git = squads::in_git(Path::new(f));
                let saved = t.works_on_in(f);
                let mut lands_on = saved.map(|w| w.lands_on.trim().to_string()).unwrap_or_default();
                if lands_on.is_empty() && in_git {
                    lands_on = crate::gitfacts::default_branch(&git, Path::new(f), deadline).unwrap_or_default();
                }
                let no_branch = if in_git { self.branch_option_blocked(number, f, &name, deadline) } else { None };
                let how = match saved.map(|w| w.how).unwrap_or_default() {
                    How::Branch if no_branch.is_none() => How::Branch,
                    _ => How::Worktree,
                };
                FolderLine { folder: f.clone(), name, git: in_git, how, lands_on, no_branch }
            })
            .collect();
        HandForm { autonomy, folders, cursor: 0, branch: t.branch() }
    }

    /// Why task `number` can't work in `folder` (a git checkout) as a new branch there: it has changes not yet
    /// committed, another open task works there as a branch already, or a session of the user's is running in it.
    /// None: it can.
    fn branch_option_blocked(&self, number: u32, folder: &str, name: &str, deadline: Instant) -> Option<String> {
        match crate::gitfacts::is_clean(&self.git_program, Path::new(folder), deadline) {
            Some(true) => {}
            Some(false) => return Some(format!("{name} has changes not yet committed")),
            None => return Some(format!("git couldn't say in time whether {name} has changes not yet committed")),
        }
        let mine = self.task_scope().unwrap_or_default();
        if let Some((p, o)) = self
            .every_ticket()
            .iter()
            .find(|(p, o)| !(*p == mine && o.number == number) && crate::tickets::is_open(o) && o.branch_in_checkout(folder))
        {
            let whose = if *p == mine { String::new() } else { format!(" (in {})", self.project_name_of(p)) };
            return Some(format!("#{}{whose} already works in {name} on a branch", o.number));
        }
        let in_it = |d: &str| !d.is_empty() && (d == folder || Path::new(d).starts_with(folder));
        if self.sessions.iter().any(|s| !s.procs.is_empty() && in_it(&s.cwd)) || self.hosts.iter().any(|h| in_it(&h.dir)) {
            return Some(format!("a session of yours is running in {name}"));
        }
        None
    }

    /// How many rows the hand-over popup has: the squads, "On its own", and one per folder.
    pub fn hand_rows(&self) -> usize {
        self.sq.pick.len() + 1 + self.sq.form.folders.len()
    }

    /// The folder line the cursor is on, if it is on one.
    pub fn hand_folder_at_cursor(&self) -> Option<usize> {
        self.sq.form.cursor.checked_sub(self.sq.pick.len() + 1).filter(|i| *i < self.sq.form.folders.len())
    }

    /// The cursor is on the "On its own" line.
    pub fn hand_on_autonomy(&self) -> bool {
        self.sq.form.cursor == self.sq.pick.len()
    }

    /// Moving the cursor never changes the picked squad: the user may walk down past the squads to the lines and keep
    /// the one they picked (space on a squad picks it; enter on a squad picks it and hands over).
    pub(super) fn move_squad_pick(&mut self, delta: i32) {
        let last = self.hand_rows().saturating_sub(1) as i32;
        self.sq.form.cursor = (self.sq.form.cursor as i32 + delta).clamp(0, last) as usize;
    }

    /// A click on row `i` of the popup: the cursor goes there, and a squad is picked by it; a second click on the same
    /// row does what enter does on a squad (hands it over) and what space does on a line (changes it).
    pub(super) fn click_hand_row(&mut self, i: usize) {
        if i == self.sq.form.cursor {
            if i < self.sq.pick.len() { self.act(Action::Confirm) } else { self.hand_change() }
        } else {
            self.sq.form.cursor = i.min(self.hand_rows().saturating_sub(1));
            if i < self.sq.pick.len() {
                self.sq.pick_selected = i;
            }
        }
    }

    pub(super) fn squad_pick_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.act(Action::Cancel),
            KeyCode::Char('c') if ctrl => self.act(Action::Cancel),
            KeyCode::Enter => self.act(Action::Confirm),
            KeyCode::Up | KeyCode::Char('k') => self.move_squad_pick(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_squad_pick(1),
            KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right => self.hand_change(),
            KeyCode::Char('l') => self.edit_lands_on(),
            _ => {}
        }
    }

    /// Space on a row of the popup: on a squad, it is picked; the next level of "On its own"; a copy or a new branch
    /// in the checkout for a folder (when that is offered: otherwise the reason, in one line).
    fn hand_change(&mut self) {
        if self.sq.form.cursor < self.sq.pick.len() {
            self.sq.pick_selected = self.sq.form.cursor;
            return;
        }
        if self.hand_on_autonomy() {
            self.sq.form.autonomy = self.sq.form.autonomy.next();
            return;
        }
        let Some(i) = self.hand_folder_at_cursor() else { return };
        let line = &self.sq.form.folders[i];
        if !line.git {
            return self.notify(format!("{} isn't in git: it is used as it is.", line.name));
        }
        if let Some(why) = &line.no_branch {
            return self.notify(format!("A new branch in your checkout isn't offered for {} just now: {why}.", line.name));
        }
        let line = &mut self.sq.form.folders[i];
        line.how = match line.how {
            How::Worktree => How::Branch,
            How::Branch => How::Worktree,
        };
    }

    /// `l` on a folder's line: the box for the branch the work lands on.
    fn edit_lands_on(&mut self) {
        let Some(i) = self.hand_folder_at_cursor() else {
            return self.notify("Move to a folder's line first: l changes the branch its work lands on.");
        };
        if !self.sq.form.folders[i].git {
            return self.notify(format!("{} isn't in git, so its work lands on no branch.", self.sq.form.folders[i].name));
        }
        self.open_name_box(NamePurpose::LandsOn(i));
    }

    /// Enter in the lands-on box: the branch must be one git finds in that repository; the box stays open otherwise.
    pub(super) fn save_lands_on(&mut self, i: usize) {
        let typed = self.name_box.input.trim().to_string();
        let Some(line) = self.sq.form.folders.get(i).cloned() else {
            self.mode = Mode::SquadPick;
            return;
        };
        if typed.is_empty() {
            return self.notify("Say which branch the work lands on.");
        }
        if !crate::gitfacts::branch_exists(&self.git_program, Path::new(&line.folder), &typed, Instant::now() + HAND_BUDGET) {
            return self.notify(format!("There is no branch {typed} in {}.", line.name));
        }
        self.sq.form.folders[i].lands_on = typed;
        self.mode = Mode::SquadPick;
    }

    /// Enter in the hand-over popup (on a squad: that one is picked first): what the lines say is written on the task
    /// (so a hand-over that doesn't happen keeps it for the next try), then the task goes to the picked squad.
    pub(super) fn take_squad_pick(&mut self) {
        if self.sq.form.cursor < self.sq.pick.len() {
            self.sq.pick_selected = self.sq.form.cursor;
        }
        let Some(squad) = self.sq.pick.get(self.sq.pick_selected).cloned() else { return };
        if let Some(first) = crate::squadfile::problems(&squad, &self.agent_library()).into_iter().next() {
            // The popup stays open: another squad can be picked.
            self.notify(format!("The {} squad needs fixing on the Squads tab first: {first}", squad.name));
            return;
        }
        let number = self.sq.pick_ticket;
        let autonomy = self.sq.form.autonomy.clone();
        let works_on: Vec<WorksOn> = self
            .sq
            .form
            .folders
            .iter()
            .filter(|l| l.git)
            .map(|l| WorksOn { folder: l.folder.clone(), how: l.how, lands_on: l.lands_on.clone() })
            .collect();
        if let Some(store) = self.tk.store.as_mut()
            && let Err(e) = store.set_terms(number, &autonomy, &works_on)
        {
            self.report_squad_error(e);
            return;
        }
        self.mode = Mode::List;
        self.hand_to_squad(number, squad);
    }

    /// Hand the ticket to `squad`: the commander checks first. When the commander has already said this very squad
    /// doesn't fit, the user is asked whether to hand it over anyway (yes skips the check); another squad gets a
    /// check of its own.
    fn hand_to_squad(&mut self, number: u32, squad: Squad) {
        let Some(t) = self.ticket(number) else { return };
        if let Some(h) = t.handover.as_ref().filter(|h| h.squad == squad.id && h.kind == HandoverKind::Refused) {
            self.mode =
                Mode::Confirm(Confirm::HandAnyway { number, squad: squad.id.clone(), title: t.title.clone(), says: h.says.clone() });
            return;
        }
        self.ask_commander(number, squad);
    }

    /// Yes to "Hand it anyway?": the worktrees and the leader, without a new check.
    pub(super) fn hand_anyway(&mut self, number: u32, squad: &str) {
        self.mode = Mode::List;
        let (Some(squad), Some(project)) = (crate::squadfile::get(&self.config, squad), self.task_scope()) else { return };
        if !self.ticket(number).is_some_and(|t| t.state == State::New && t.busy.is_none()) {
            return;
        }
        self.notify(format!("Handing #{number} to the {} squad, as you said.", squad.name));
        self.start_or_queue(&project, number, squad, true);
    }

    /// The commander's check, off the UI thread (the ticket shows Working, "Commander is checking…"). If the gate
    /// says no there is no call at all: the ticket stays New, with why.
    fn ask_commander(&mut self, number: u32, squad: Squad) {
        let Some(project) = self.task_scope() else { return };
        self.sq.from_line.remove(&(project.clone(), number));
        let Some(t) = self.ticket(number).cloned() else { return };
        let settings = self.settings.get();
        // The user asked for this by pressing a key: the plan-usage pause doesn't apply, every other gate does.
        if let Err(r) = ai::gate_for(&settings, squads::COMMANDER_FEATURE, &self.cache, self.clock(), true) {
            self.after_commander(&project, number, squad, Check::Refused(r));
            return;
        }
        let body = self.tk.store.as_ref().map(|s| s.body(&t)).unwrap_or_default();
        let mut req = Request::new(
            squads::COMMANDER_FEATURE,
            squads::commander_system(&instructions::guidance(&self.config, instructions::COMMANDER)),
            String::new(),
        );
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
            let facts: Vec<FolderFact> =
                t.folders.iter().map(|f| FolderFact { path: f.clone(), git: squads::in_git(Path::new(f)) }).collect();
            req.prompt = squads::commander_prompt(&checked, &t, &body, &facts);
            let check = if never {
                Check::Failed("no stand-in for claude in this test".into())
            } else {
                Check::from_answer(runner.run(&settings, &req))
            };
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

    /// What the app is doing for task `number` of `project` while it is being handed to a squad, if it is.
    pub fn starting_words(&self, project: &str, number: u32) -> Option<&'static str> {
        self.sq.starting.iter().find(|st| st.project == project && st.number == number).map(|st| st.stage.words())
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
    pub(super) fn reread_tickets(&mut self) {
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
        let Ok(f) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(self.config.join("squad-line.lock")) else {
            return false;
        };
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
            let agents = crate::agentfile::all(&self.config);
            let broken = squad.as_ref().and_then(|s| crate::squadfile::problems(s, &agents).into_iter().next());
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

    /// A copy of each git folder (a worktree from the branch the work lands on, or the task's branch made in the user's
    /// own checkout when the task says so), off the UI thread (git can be slow on a big repository or a network
    /// folder); the task shows "Making its copy of the folder…" meanwhile. Whatever the squad: a task with no folder
    /// goes straight on to its leader, in its own folder.
    fn make_worktrees(&mut self, project: &str, number: u32, squad: Squad) {
        let Some(t) = self.ticket_on_disk(project, number) else { return };
        if t.folders.is_empty() {
            return self.after_worktrees(project, number, squad, Ok(Vec::new()));
        }
        let git = self.git_program.clone();
        let (tx, rx) = channel();
        thread::spawn(move || {
            let _ = tx.send(Step::Worktrees(squads::prepare_work(&git, &t.folders, &WorkPlan::of(&t))));
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
                self.not_handed(project, number, &squad, HandoverKind::NotStarted, &format!("Couldn't set up where it works: {e}"), notice);
                return;
            }
        };
        if !self.still_new(project, number) {
            return;
        }
        let session = squads::new_session_id();
        // Where it works: the worktrees just made, the folders as they are, or the task's own folder; and its terms.
        let (place, terms) = match self.ticket_on_disk(project, number) {
            Some(mut t) => {
                t.worktrees = worktrees.clone();
                (Place::of(&t), Terms::of(&t))
            }
            None => return,
        };
        let work = crate::tickets::work_dir(&self.config, project, number);
        if let Err(e) = std::fs::create_dir_all(&work) {
            let notice = format!("Couldn't start the squad: couldn't make the task's own folder ({e})");
            return self.not_handed(project, number, &squad, HandoverKind::NotStarted, &notice, notice.clone());
        }
        // What it starts with is kept beside the ticket: continued later, it runs with the squad as it is at this
        // moment, whatever is changed on the Squads tab meanwhile.
        let agents = crate::agentfile::all(&self.config);
        if let Some(first) = crate::squadfile::problems(&squad, &agents).into_iter().next() {
            // Changed on the Squads tab while it was being handed over.
            let says = format!("The {} squad needs fixing on the Squads tab first: {first}", squad.name);
            return self.not_handed(project, number, &squad, HandoverKind::NotStarted, &says, format!("#{number} didn't start: {says}"));
        }
        let leader_setup = LeaderSetup::load(&self.config, &self.settings.get());
        let launch = Launch::of(&crate::squadfile::Crew::of(&squad, &agents), &leader_setup, AgentNames::SquadRole, place, &terms);
        let k = self.ticket_on_disk(project, number).map_or(0, |t| t.leaders.len()) + 1;
        let snapshot = match Tickets::open(&self.config, project).save_snapshot(number, k, &launch.to_json()) {
            Ok(name) => name,
            Err(e) => {
                let notice = format!("Couldn't start the squad: couldn't keep what it starts with ({e})");
                self.not_handed(project, number, &squad, HandoverKind::NotStarted, &notice, notice.clone());
                return;
            }
        };
        let leader = Leader {
            squad: squad.id.to_string(),
            session: session.clone(),
            started: usage::now(),
            names: AgentNames::SquadRole,
            snapshot: Some(snapshot),
        };
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
        let where_said = match self.projects.get(project) {
            // On the Tasks tab it is right there.
            _ if self.board_showing() => String::new(),
            Some(p) => format!(" Open it from {}'s tasks.", p.name),
            None => " Open it from the Tasks tab.".to_string(),
        };
        // Beside the ticket only if the user is still on that project's screen and not in another session, and not
        // for a ticket that waited in the line (it starts by itself, while the user may be doing anything).
        let from_line = self.sq.from_line.remove(&(project.to_string(), number));
        let quiet = self.sq.quiet.remove(&(project.to_string(), number));
        let show = !from_line && !quiet && self.open_project.as_deref() == Some(project) && matches!(self.mode, Mode::List);
        match self.launch_leader_in(project, number, Some(&first), show) {
            Ok(()) if show => {
                self.notify(format!("#{number} is with the {} squad now. {} goes back to the project.", squad.name, self.back_label()))
            }
            Ok(()) if from_line => self.notify(format!("A squad is free: #{number} is with the {} squad now.{where_said}", squad.name)),
            Ok(()) => self.notify(format!("#{number} is with the {} squad now.{where_said}", squad.name)),
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
        self.notify(format!("Couldn't save the task: {}", why.split(" (os error").next().unwrap_or(&why)));
    }

    /// Continue ticket `number`'s current leader (the open project's) in a platoon host, beside the ticket. False,
    /// said in a notice, when it couldn't start.
    fn launch_leader(&mut self, number: u32, first: Option<&str>) -> bool {
        let Some(project) = self.task_scope() else { return false };
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
    pub(super) fn launch_leader_in(&mut self, project: &str, number: u32, first: Option<&str>, show: bool) -> Result<(), String> {
        let t = self.ticket_on_disk(project, number).ok_or_else(|| format!("#{number} isn't there any more"))?;
        let leader = t.leader().cloned().ok_or_else(|| format!("#{number} has no squad"))?;
        let launch = self.leader_launch(project, &leader, &t)?;
        // The task's own folder is made again if it has gone (a leader continued after it was deleted by hand).
        let work = crate::tickets::work_dir(&self.config, project, number);
        std::fs::create_dir_all(&work).map_err(|e| format!("couldn't make the task's own folder ({e})"))?;
        let (dir, add_dirs) = squads::start_dirs(&t, launch.work_folder, &work.to_string_lossy())
            .ok_or_else(|| format!("#{number} has no folder to work in"))?;
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
            match host::launch(&self.host_dirs(), &self.program, &dir, &refs, &[], rows, cols) {
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
        let squad = crate::squadfile::get(&self.config, &leader.squad)
            .ok_or_else(|| format!("the squad \"{}\" isn't there any more, or its file can't be read", leader.squad))?;
        let setup = LeaderSetup::load(&self.config, &self.settings.get());
        Ok(Launch::of(&crate::squadfile::Crew::load(&self.config, &squad), &setup, leader.names, Place::of(t), &Terms::of(t)))
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
                // Its worktree was removed after the work: made again from its branch first, then continued.
                if self.bring_worktrees_back(number) {
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

    /// `p` on a task that Needs you because its squad wants to publish: the user's go-ahead is written on the task
    /// (Working again) and the leader is told to go on.
    pub(super) fn let_it_publish(&mut self, number: u32) {
        let Some(t) = self.ticket(number) else { return };
        if !(t.state == State::Waiting && t.publish_wanted) {
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
            self.notify(format!(
                "#{number} may publish now. Its squad's session isn't running: open it with enter and tell it to go ahead."
            ));
            return;
        };
        let (rows, cols) = split_size(self.size);
        thread::spawn(move || type_into(&sock, rows, cols, "Go ahead and publish."));
        self.notify(format!("#{number} may publish now: platoon told its squad to go ahead."));
    }

    // ---- stopping a cancelled ticket's leader -----------------------------------------------------------------

    /// Stop only ticket `number`'s leader session (SIGTERM to the `claude` its host runs, on a thread of its own),
    /// when `pid` is still that session's `claude`. Its conversation is kept, so a message to it later (enter, then
    /// typing) continues it and opens the ticket again. True when it is being stopped.
    pub(super) fn stop_squad(&mut self, number: u32, pid: u32) -> bool {
        // The host of this ticket's leader, and nothing else.
        let Some(session) = self.ticket(number).and_then(|t| t.leader()).map(|l| l.session.clone()) else { return false };
        if self.leader_host(&session).map(|h| h.claude_pid) != Some(pid) {
            return false;
        }
        thread::spawn(move || {
            let _ = host::end_processes(&[pid]);
        });
        true
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
pub(super) fn type_into(sock: &Path, rows: u16, cols: u16, text: &str) {
    use crate::host::{F_INPUT, F_RESIZE, size_payload, write_frame};
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
