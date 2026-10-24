//! Starting a task, inside the app: `s` opens one list of squads (the task's suggested squad picked, else Feature) and
//! enter starts it, in Claude Code or Codex as the squad's model decides (`task_agent`). A task with no folder asks for
//! one first (the folder picker). Then, off the UI thread, its worktree is made (`worktrees::prepare`; none for a
//! folder not in git), and for a Codex task Codex itself is asked to trust that folder (`codex_server::trust_folder`),
//! so it doesn't ask in the window; then `squad.md` and the squads are written into the task's own folder
//! (`taskrun::write_task_files`), the session is written on the task (Working; a Codex task's as `tickets::PENDING`
//! until it says its id) and started in a platoon host (`Agent::task_args`, with the task in its environment:
//! `agents::TASK_ENV`), and the user is taken into it. `enter` on a started task opens its session: the running one
//! (found by the task its host was started for), or the same session continued (its worktree made again from its
//! branch first when it was removed after the work). Also here: which sessions are tasks' (for the Sessions tab's
//! mark), a Codex task's session id learned from the process holding it when no hook said it, and stopping a cancelled
//! task's session.

use super::*;
use crate::agents::TaskLaunch;
use crate::agents::codex_server;
use crate::squads::Squad;
use crate::taskrun::{self, Rules};
use crate::tickets::{State, Ticket, Tickets, Worktree};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::SystemTime;

/// What a task says while its worktree is made, before its session starts.
pub const MAKING_WORKTREE: &str = "Making its worktree…";

/// What making a task's worktree came to: the worktree (None: the folder isn't in git), and for a Codex task, what
/// came of asking Codex to trust it (`trust_worktree`).
pub(super) type Made = (Result<Option<Worktree>, String>, Option<Trusted>);

/// What came of asking Codex to trust a task's worktree: Some(key) when the trust is platoon's (written now, or
/// before), None when the user's own say about that folder was there and was left as it is; Err when Codex couldn't
/// be asked (it asks in the window then).
pub(super) type Trusted = Result<Option<String>, String>;

/// Make task `number`'s worktree in `folder` (`worktrees::prepare`), and, when `codex` is where the task runs, have
/// Codex trust it. Off the UI thread.
pub(super) fn make_worktree(git: &str, folder: &str, number: u32, recorded: Option<&Worktree>, codex: Option<&CodexAt>) -> Made {
    let made = crate::worktrees::prepare(git, folder, number, recorded);
    let trust = match (&made, codex) {
        (Ok(Some(w)), Some(c)) => Some(trust_worktree(c, w, number)),
        _ => None,
    };
    (made, trust)
}

/// Have Codex trust task `number`'s worktree folder (`worktrees::root_of`), through its own app-server: platoon's
/// entry when it is written now or was before (`w.codex_trust`), None for the user's own say left as it was.
pub(super) fn trust_worktree(c: &CodexAt, w: &Worktree, number: u32) -> Trusted {
    let root = crate::worktrees::root_of(w, number);
    codex_server::trust_folder(&c.program, &c.root, &root, w.codex_trust.as_deref(), codex_server::TIME_BOX)
}

/// A task being started: its worktree is being made off the UI thread.
struct Starting {
    project: String,
    number: u32,
    squad: Squad,
    rx: Receiver<Made>,
}

/// Everything about starting tasks the app keeps.
pub struct SquadState {
    /// The `s` list: every squad, as they were when it opened, and the picked one.
    pub pick: Vec<Squad>,
    pub pick_selected: usize,
    /// The task the list is open for: its project and number.
    pub pick_task: (String, u32),
    starting: Vec<Starting>,
    /// Every task of every project, as the files said when `stamps` were taken.
    all_tickets: Vec<(String, Ticket)>,
    stamps: Vec<(PathBuf, Option<(SystemTime, u64)>)>,
    /// The tasks' sessions: session id -> (project, number).
    sessions: HashMap<String, (String, u32)>,
    /// The task sessions Claude Code says are running (hosted by any platoon, or by hand).
    running: HashSet<String>,
    /// In unit tests no session is ever started (the program would be the test binary): the tasks that would have
    /// been, and for each the folder it would have started in and its arguments.
    #[cfg(test)]
    pub launched: Vec<u32>,
    #[cfg(test)]
    pub launched_with: Vec<(String, Vec<String>)>,
    /// What each would have had added to its environment.
    #[cfg(test)]
    pub launched_env: Vec<Vec<(String, String)>>,
}

impl SquadState {
    pub fn new() -> Self {
        Self {
            pick: Vec::new(),
            pick_selected: 0,
            pick_task: (String::new(), 0),
            starting: Vec::new(),
            all_tickets: Vec::new(),
            stamps: Vec::new(),
            sessions: HashMap::new(),
            running: HashSet::new(),
            #[cfg(test)]
            launched: Vec::new(),
            #[cfg(test)]
            launched_with: Vec::new(),
            #[cfg(test)]
            launched_env: Vec::new(),
        }
    }

    /// The task whose session this is: its number.
    pub fn task_of(&self, session: &str) -> Option<u32> {
        self.sessions.get(session).map(|(_, n)| *n)
    }
}

fn fs_stamp(p: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.modified().ok()?, m.len()))
}

impl App {
    /// Read every project's tasks again if a file changed (one `stat` per file when nothing did). True when the tasks'
    /// sessions changed.
    pub(super) fn reread_tickets(&mut self) -> bool {
        let stamps: Vec<(PathBuf, Option<(SystemTime, u64)>)> =
            crate::tickets::all_files(&self.config).into_iter().map(|(_, p)| (p.clone(), fs_stamp(&p))).collect();
        if stamps == self.sq.stamps {
            return false;
        }
        self.sq.stamps = stamps;
        self.sq.all_tickets = crate::tickets::every_ticket(&self.config);
        let sessions: HashMap<String, (String, u32)> =
            self.sq.all_tickets.iter().filter_map(|(p, t)| t.session.clone().map(|s| (s, (p.clone(), t.number)))).collect();
        if sessions == self.sq.sessions {
            return false;
        }
        self.sq.sessions = sessions;
        true
    }

    /// Every task of every project, as the files said when they were last read.
    pub(super) fn every_ticket(&self) -> &[(String, Ticket)] {
        &self.sq.all_tickets
    }

    /// Every task of every project (a unit test's look at them).
    #[cfg(test)]
    pub fn every_ticket_for_test(&self) -> Vec<(String, Ticket)> {
        self.sq.all_tickets.clone()
    }

    /// Note which tasks' sessions are running (as Claude Code's registry and Codex's writer locks say). A Codex task
    /// whose session hasn't said its id yet (its first hook does) gets it from the Codex process its host runs, which
    /// holds the conversation: so the task finds its session even where its hooks don't run.
    pub(super) fn note_running_tasks(&mut self, live: &[Live]) {
        let mut learned = false;
        for (project, t) in self.sq.all_tickets.clone() {
            let Some(pending) = t.session.as_deref().filter(|s| s.starts_with(crate::tickets::PENDING)) else { continue };
            let tag = crate::tickets::task_tag(&project, t.number);
            let Some(h) = self.hosts.iter().find(|h| h.task.as_deref() == Some(tag.as_str())) else { continue };
            let Some(l) = live.iter().find(|l| l.agent == Agent::Codex && l.pid == h.claude_pid) else { continue };
            let _ = pending;
            if Tickets::open(&self.config, &project).record_session(t.number, &l.id).unwrap_or(false) {
                learned = true;
                if project != crate::tickets::NO_PROJECT_ID {
                    let _ = self.projects.place(&l.id, &project);
                }
            }
        }
        if learned {
            self.reread_tickets();
            if let Some(s) = self.tk.store.as_mut() {
                s.reload_if_changed();
            }
        }
        self.sq.running = live.iter().filter(|l| self.sq.sessions.contains_key(&l.id)).map(|l| l.id.clone()).collect();
    }

    /// The platoon host running task `t` of `project`, if one is: the one started for the task (its environment names
    /// it: `HostInfo::task`), else the one started with the task's session id (as a new session, or resumed).
    pub fn task_host(&self, project: &str, t: &Ticket) -> Option<&HostInfo> {
        let tag = crate::tickets::task_tag(project, t.number);
        self.hosts.iter().find(|h| h.task.as_deref() == Some(tag.as_str())).or_else(|| {
            let session = t.session_id()?;
            self.hosts.iter().find(|h| h.session().as_deref() == Some(session))
        })
    }

    /// Is task `t`'s session running now (hosted by a platoon, or as its agent says)?
    pub fn task_running(&self, project: &str, t: &Ticket) -> bool {
        self.task_host(project, t).is_some() || t.session_id().is_some_and(|s| self.sq.running.contains(s))
    }

    /// What the app is doing for task `number` of `project` while it is being started, if it is.
    pub fn starting_words(&self, project: &str, number: u32) -> Option<&'static str> {
        self.sq.starting.iter().any(|st| st.project == project && st.number == number).then_some(MAKING_WORKTREE)
    }

    /// True while a task is being started (so the screen is refreshed quickly).
    pub fn squads_busy(&self) -> bool {
        !self.sq.starting.is_empty()
    }

    // ---- s: the list of squads ----------------------------------------------------------------------------------

    /// `s` on a task: the list of squads, unless the task can't start now (said in one line). A task with no folder,
    /// or whose folder has gone, gets the folder picker first.
    pub(super) fn ask_start(&mut self, number: u32) {
        let Some(project) = self.task_scope() else { return };
        let Some(t) = self.ticket(number).cloned() else { return };
        let why = if self.starting_words(&project, number).is_some() {
            Some(format!("#{number} is starting. Wait a moment."))
        } else if let Some(p) = self.projects.get(&project).filter(|p| p.archived) {
            Some(crate::tickets::archived_refusal(&p.name))
        } else if t.started() {
            Some(format!("#{number} has its session already: enter opens it."))
        } else if t.state == State::Cancelled {
            Some(format!("#{number} is cancelled."))
        } else {
            None
        };
        if let Some(why) = why {
            self.notify(why);
            return;
        }
        if !t.folder.as_deref().is_some_and(|f| Path::new(f).is_dir()) {
            let p = self.projects.get(&project).map(|p| p.id.clone());
            self.open_picker_in(p);
            self.picker.task_folder = Some(number);
            self.notify(format!("Pick the folder #{number} works in."));
            return;
        }
        self.open_squad_list(&project, number);
    }

    /// Enter in the folder picker opened for task `number`: its folder is written, then the list of squads opens.
    pub(super) fn take_task_folder(&mut self, number: u32, dir: String) {
        self.mode = Mode::List;
        let Some(store) = self.tk.store.as_mut() else { return };
        if let Err(e) = store.set_folder(number, &dir) {
            return self.report_task_error(e);
        }
        let project = store.project().to_string();
        self.open_squad_list(&project, number);
    }

    fn open_squad_list(&mut self, project: &str, number: u32) {
        let suggested = self.ticket(number).map(|t| t.squad.clone()).unwrap_or_default();
        self.sq.pick = crate::squads::all(&self.config);
        self.sq.pick_selected = self
            .sq
            .pick
            .iter()
            .position(|s| s.id == suggested)
            .or_else(|| self.sq.pick.iter().position(|s| s.id == crate::squads::DEFAULT))
            .unwrap_or(0);
        self.sq.pick_task = (project.to_string(), number);
        self.mode = Mode::SquadPick;
    }

    pub(super) fn move_squad_pick(&mut self, delta: i32) {
        let last = self.sq.pick.len().saturating_sub(1) as i32;
        self.sq.pick_selected = (self.sq.pick_selected as i32 + delta).clamp(0, last) as usize;
    }

    /// A click on a squad in the list: picks it; a click on the picked one starts.
    pub(super) fn click_squad_row(&mut self, i: usize) {
        if i == self.sq.pick_selected {
            self.act(Action::Confirm);
        } else if i < self.sq.pick.len() {
            self.sq.pick_selected = i;
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
            KeyCode::Home | KeyCode::Char('g') => self.sq.pick_selected = 0,
            KeyCode::End | KeyCode::Char('G') => self.sq.pick_selected = self.sq.pick.len().saturating_sub(1),
            _ => {}
        }
    }

    /// Enter in the list: start the task with the picked squad. A squad whose file has something wrong says what, and
    /// the list stays open.
    pub(super) fn take_squad_pick(&mut self) {
        let Some(squad) = self.sq.pick.get(self.sq.pick_selected).cloned() else { return };
        if let Some(first) = squad.problems.first() {
            self.notify(format!("The {} squad can't be used: {first}", if squad.name.is_empty() { &squad.id } else { &squad.name }));
            return;
        }
        let agent = match self.task_agent(&squad) {
            Ok(a) => a,
            Err(why) => return self.notify(why),
        };
        self.mode = Mode::List;
        let (project, number) = self.sq.pick_task.clone();
        let Some(t) = Tickets::open(&self.config, &project).get(number).cloned() else { return };
        let Some(folder) = t.folder.clone() else { return };
        let git = self.git_program.clone();
        let recorded = t.worktree.clone();
        let codex = self.providers.codex.clone().filter(|_| agent == Agent::Codex);
        let (tx, rx) = channel();
        thread::spawn(move || {
            let _ = tx.send(make_worktree(&git, &folder, number, recorded.as_ref(), codex.as_ref()));
        });
        self.sq.starting.push(Starting { project, number, squad, rx });
    }

    /// Take in the worktrees made. Called on every refresh; cheap when nothing is out.
    pub(super) fn poll_squad_starts(&mut self) {
        let mut i = 0;
        while i < self.sq.starting.len() {
            let made = match self.sq.starting[i].rx.try_recv() {
                Ok(made) => made,
                Err(TryRecvError::Empty) => {
                    i += 1;
                    continue;
                }
                Err(TryRecvError::Disconnected) => (Err("it stopped without saying why".into()), None),
            };
            let st = self.sq.starting.remove(i);
            self.after_worktree(&st.project, st.number, &st.squad, made);
        }
    }

    /// The worktree is there (or the folder is used as it is): the squad's files go in the task's folder, the session is
    /// written on the task (Working) and started, and the user is taken into it when nothing else took the screen.
    fn after_worktree(&mut self, project: &str, number: u32, squad: &Squad, (made, trust): Made) {
        let mut worktree = match made {
            Ok(w) => w,
            Err(e) => return self.notify(format!("Couldn't start #{number}: {e}")),
        };
        let agent = match self.task_agent(squad) {
            Ok(a) => a,
            Err(why) => return self.notify(why),
        };
        let mut untrusted = None;
        if let (Some(w), Some(trust)) = (worktree.as_mut(), trust) {
            match trust {
                Ok(key) => w.codex_trust = key,
                Err(why) => untrusted = Some(trust_failed(number, &crate::worktrees::root_of(w, number), &why, &self.home)),
            }
        }
        let squad = &squad.as_run(&self.catalog());
        let mut store = Tickets::open(&self.config, project);
        let Some(t) = store.get(number).cloned().filter(|t| !t.started() && t.state != State::Cancelled) else {
            return self.notify(format!("#{number} was changed meanwhile, so it wasn't started."));
        };
        let task_dir = crate::tickets::task_dir(&self.config, project, number);
        let catalog = self.catalog();
        let rules = Rules {
            agent,
            folder: worktree.as_ref().map(|w| w.path.clone()).or(t.folder.clone()).unwrap_or_default(),
            branch: worktree.as_ref().map(|w| w.branch.clone()),
            squads_dir: task_dir.join("squads").to_string_lossy().into_owned(),
            plan_first: squad.plan_first,
            helper_models: catalog.of_agent(agent).map(|m| m.id.clone()).filter(|_| agent == Agent::Codex).collect(),
            squad_files: Vec::new(),
        };
        // Every squad as it would run here: a step that follows another squad follows what this computer runs.
        let all: Vec<Squad> = crate::squads::all(&self.config).iter().map(|s| s.as_run(&catalog)).collect();
        if let Err(e) = taskrun::write_task_files(&task_dir, squad, &all, &rules) {
            return self.notify(format!("Couldn't start #{number}: couldn't write its squad ({e})"));
        }
        // Claude Code takes the id platoon chooses; Codex says its own once it starts (`tickets::PENDING`).
        let session = match agent {
            Agent::Claude => taskrun::new_session_id(),
            Agent::Codex => format!("{}{}", crate::tickets::PENDING, crate::tickets::task_tag(project, number)),
        };
        if let Err(e) = store.start(number, &squad.id, &session, worktree) {
            return self.report_task_error(e);
        }
        // Written down before it starts, so its hooks find it from its first moment.
        self.reread_tickets();
        if let Some(s) = self.tk.store.as_mut() {
            s.reload_if_changed();
        }
        if agent == Agent::Claude
            && project != crate::tickets::NO_PROJECT_ID
            && let Err(e) = self.projects.assign(&session, Some(project))
        {
            self.notify(format!("Couldn't put #{number}'s session in its project: {e}"));
        }
        let show = matches!(self.mode, Mode::List) && !self.help_open;
        match self.launch_task(project, number, false, show) {
            Ok(()) => match untrusted {
                Some(said) => self.notify(said),
                None if show => {}
                None => self.notify(format!("#{number} started with the {} squad.", squad.name)),
            },
            Err(e) => {
                let _ = Tickets::open(&self.config, project).forget_start(number);
                self.reread_tickets();
                self.notify(format!("Couldn't start #{number}: {e}"));
            }
        }
    }

    /// The squad task `t` of `project` runs with: its copy beside the task, as it was when the task started (else the
    /// squad as it is now, as it would run here: `Squad::as_run`).
    pub(super) fn task_squad(&self, project: &str, t: &Ticket) -> Result<Squad, String> {
        let task_dir = crate::tickets::task_dir(&self.config, project, t.number);
        std::fs::read_to_string(task_dir.join("squads").join(format!("{}.md", t.squad)))
            .ok()
            .map(|text| crate::squads::parse(&t.squad, &text, crate::squads::Origin::Own))
            .or_else(|| crate::squads::get(&self.config, &t.squad).map(|s| s.as_run(&self.catalog())))
            .ok_or_else(|| format!("its squad \"{}\" isn't there any more", t.squad))
    }

    /// Start task `number`'s session in a platoon host: a new one with its text as the first message, or (`resume`)
    /// the same session continued. `show`: the user is taken into it. A Codex task whose session never said its id
    /// (it never got as far as its first message) starts afresh.
    pub(super) fn launch_task(&mut self, project: &str, number: u32, resume: bool, show: bool) -> Result<(), String> {
        let store = Tickets::open(&self.config, project);
        let t = store.get(number).cloned().ok_or_else(|| format!("#{number} isn't there any more"))?;
        t.session.as_ref().ok_or_else(|| format!("#{number} has no session"))?;
        let task_dir = crate::tickets::task_dir(&self.config, project, number);
        let squad = self.task_squad(project, &t)?;
        let dir = t.work_folder().ok_or_else(|| format!("#{number} has no folder"))?;
        if !Path::new(&dir).is_dir() {
            return Err(format!("{} doesn't exist any more", dirs::tilde(&dir, &self.home)));
        }
        let agent = self.task_agent(&squad)?;
        let resume = resume && t.session_id().is_some();
        let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "platoon".into());
        let plan = squad.plan_first && (!resume || t.state == State::NeedsYou(crate::tickets::Need::Plan));
        let launch = TaskLaunch {
            session: t.session_id().unwrap_or_default().to_string(),
            resume,
            name: taskrun::session_name(&t),
            model: squad.model.clone(),
            plan,
            instructions: std::fs::read_to_string(task_dir.join(taskrun::SQUAD_FILE)).unwrap_or_default(),
            task_dir,
            exe,
            first: (!resume).then(|| taskrun::first_message(&t, &store.body(&t))),
            tag: crate::tickets::task_tag(project, number),
        };
        let args = agent.task_args(&launch);
        let mut env: Vec<(&str, String)> = launch.env();
        if project != crate::tickets::NO_PROJECT_ID {
            env.push(("PLATOON_PROJECT", project.to_string()));
        }
        // A unit test's program is the test binary itself: nothing is started, what would have been is noted.
        #[cfg(test)]
        {
            let _ = show;
            self.sq.launched.push(number);
            self.sq.launched_with.push((dir, args));
            self.sq.launched_env.push(env.into_iter().map(|(k, v)| (k.to_string(), v)).collect());
            Ok(())
        }
        #[cfg(not(test))]
        {
            let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
            let (rows, cols) = self.pty_size();
            let (info, child) = host::launch(&self.host_dirs(), agent, &self.program_of(agent), &dir, &args, &env, rows, cols)?;
            let pid = info.claude_pid;
            self.spawned.push(child);
            self.hosts.push(info);
            if show {
                self.attach(pid);
            }
            Ok(())
        }
    }

    /// The program task `t` of `project` runs in (as its squad's model says), when that can be told.
    pub fn task_program(&self, project: &str, t: &Ticket) -> Option<Agent> {
        self.task_squad(project, t).ok().and_then(|s| self.task_agent(&s).ok())
    }

    /// The agent a task's session runs on: its squad's model decides (an OpenAI model runs it in Codex). Err, plainly,
    /// when nothing here can run it. Which programs are here is `Providers`' to say.
    pub(super) fn task_agent(&self, squad: &Squad) -> Result<Agent, String> {
        let catalog = self.catalog();
        let squad = squad.as_run(&catalog);
        let name = squad.name_or_id();
        match catalog.agent_of(&squad.model) {
            Some(agent) => Ok(agent),
            None => Err(format!("The {name} squad's model can't be used: {}", catalog.missing(&squad.model).unwrap_or_default())),
        }
    }

    /// Enter on a started task: its session, running or continued.
    pub(super) fn open_task_session(&mut self, number: u32) {
        let Some(project) = self.task_scope() else { return };
        let Some(t) = self.ticket(number).cloned().filter(Ticket::started) else { return };
        if let Some(pid) = self.task_host(&project, &t).map(|h| h.claude_pid) {
            self.attach(pid);
            return;
        }
        // Running somewhere platoon doesn't host (someone resumed it by hand): a second copy would fight it.
        if let Some(session) = t.session_id()
            && let Some(l) = self.live_now().into_iter().find(|l| l.id == session)
        {
            self.notify(format!("#{number}'s session is running in another terminal (pid {}). Close it there first.", l.pid));
            return;
        }
        // Its worktree was removed after the work: made again from its branch first, then continued.
        if self.bring_worktree_back(number) {
            return;
        }
        if let Err(e) = self.launch_task(&project, number, true, true) {
            self.notify(format!("Couldn't open #{number}'s session: {e}"));
        }
    }

    /// Stop only task `number`'s session (SIGTERM to the `claude` its host runs, on a thread of its own), when `pid` is
    /// still that session's `claude`. Its conversation is kept. True when it is being stopped.
    pub(super) fn stop_task_session(&mut self, number: u32, pid: u32) -> bool {
        let (Some(project), Some(t)) = (self.task_scope(), self.ticket(number).cloned()) else { return false };
        if self.task_host(&project, &t).map(|h| h.claude_pid) != Some(pid) {
            return false;
        }
        thread::spawn(move || {
            let _ = host::end_processes(&[pid]);
        });
        true
    }
}

/// What `after_worktree` says when Codex couldn't be asked to trust task `number`'s worktree `root`: the task starts
/// all the same, and Codex asks its own question in the window.
pub(super) fn trust_failed(number: u32, root: &Path, why: &str, home: &str) -> String {
    format!(
        "#{number} started, but platoon couldn't ask Codex to trust {} ({why}): Codex asks you in the window whether you trust it.",
        dirs::tilde(&root.to_string_lossy(), home)
    )
}
