//! Tickets' worktrees inside the app, after the work. A ticket that is Done or Cancelled, whose squad's session is no
//! longer running, has its worktrees removed (its branch kept); one with changes not yet committed is kept, and its
//! screen says so, with `w` to remove it anyway after a yes. One opened in an editor with a window of its own (`v`) is
//! kept too, since platoon can't tell when that window closes: "Kept: opened in VS Code", until `w`. Continuing such a ticket's squad (enter on its screen)
//! makes the worktree again from its branch first. The git side, and every check on what may be touched, is in
//! `worktrees.rs`.
//!
//! Why here and not in a hook: a ticket goes Done in the leader's Stop hook, while the leader's session is still
//! running in the worktree (the user may carry on talking to it), and a cancelled ticket's leader is stopped by the
//! app. Only the app sees both the tickets and which sessions run, so it waits until the session has stopped, then
//! removes the worktree off the UI thread.

use super::*;
use crate::tickets::{Ticket, Tickets, Worktree, WtMark};
use crate::worktrees::{self, Removal};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// How often the closed tickets are looked over for worktrees to remove.
const EVERY: Duration = Duration::from_secs(2);

/// What a ticket says while its worktree is made again, before its squad is continued.
pub const BRINGING_BACK: &str = "Bringing its worktree back…";

/// How long `v` waits for a removal of the ticket's worktree that had just started.
const REMOVAL_WAIT: Duration = Duration::from_secs(10);

/// How long `c` may wait for git to say whether a worktree has changes, before asking.
const ASK_BUDGET: Duration = Duration::from_millis(1500);

/// What happens once a ticket's removed worktrees are back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum After {
    /// Its squad is continued.
    Squad,
    /// The code editor opens on it (`v`).
    Editor,
}

/// One ticket's worktrees, being removed or made again off the UI thread.
struct Out<T> {
    project: String,
    number: u32,
    rx: Receiver<T>,
}

/// What the removal thread sends back: each worktree's path and what came of it.
type Removals = Out<Vec<(String, Removal)>>;
/// What bringing removed worktrees back sends: the worktrees made again, or why not.
type BroughtBack = Out<Result<Vec<Worktree>, String>>;

#[derive(Default)]
pub struct WtState {
    /// Worktrees being removed: (path, what came of it). `anyway`: the user said yes to losing their changes.
    removing: Option<(Removals, bool)>,
    /// Removed worktrees being made again from their branch, to continue the squad.
    back: Option<(BroughtBack, After)>,
    /// Worktrees already looked at for the ticket as it stood (project, number, path, the ticket's `updated`): each is
    /// tried once, and again only when the ticket changes (or platoon starts again), never over and over.
    tried: HashSet<(String, u32, String, u64)>,
    last: Option<Instant>,
    /// The folders sessions are working in now (every running claude's, every platoon host's).
    busy_dirs: Vec<PathBuf>,
}

/// The folder platoon made for task `number`'s worktree `w`: the `task-<N>` (or `ticket-<N>`) folder its path is in
/// (the path itself when there is none).
fn worktree_root(w: &Worktree, number: u32) -> PathBuf {
    let names = [format!("task-{number}"), format!("ticket-{number}")];
    let path = Path::new(&w.path);
    path.ancestors().find(|a| a.file_name().is_some_and(|n| names.iter().any(|x| *x == n.to_string_lossy()))).unwrap_or(path).to_path_buf()
}

/// The one line said when platoon has, by itself, removed task `number`'s worktrees after the work (`removed` of
/// them), or kept some for the first time because they have changes not yet committed (`kept`). A worktree kept
/// before is not said again (it is looked at again each time platoon starts, in case the changes were committed).
/// `branch`: the task's branch, which stays.
pub fn removal_notice(number: u32, branch: &str, removed: usize, kept: usize) -> Option<String> {
    let removed_part = match removed {
        0 => None,
        1 => Some(format!("Removed #{number}'s worktree (its branch {branch} is kept)")),
        n => Some(format!("Removed #{number}'s worktrees ({n}); their branch {branch} is kept")),
    };
    let kept_part = match (kept, removed_part.is_some()) {
        (0, _) => None,
        (1, false) => Some(format!("Kept #{number}'s worktree: it has changes not yet committed (w on the task removes it)")),
        (n, false) => Some(format!("Kept #{number}'s worktrees ({n}): they have changes not yet committed (w on the task removes them)")),
        (1, true) => Some("kept one with changes not yet committed (w on the task removes it)".to_string()),
        (n, true) => Some(format!("kept {n} with changes not yet committed (w on the task removes them)")),
    };
    match (removed_part, kept_part) {
        (None, None) => None,
        (Some(r), None) => Some(format!("{r}.")),
        (None, Some(k)) => Some(format!("{k}.")),
        (Some(r), Some(k)) => Some(format!("{r}; {k}.")),
    }
}

impl App {
    /// Note where sessions are working now (from Claude Code's registry and platoon's hosts).
    /// And where a code editor has a folder open, in this platoon window or another (`editor::open_now`).
    pub(super) fn note_busy_dirs(&mut self, live: &[Live]) {
        self.wt.busy_dirs = live
            .iter()
            .map(|l| PathBuf::from(&l.cwd))
            .chain(self.hosts.iter().map(|h| PathBuf::from(&h.dir)))
            .chain(crate::editor::open_now(&self.cache))
            .filter(|d| !d.as_os_str().is_empty())
            .collect();
    }

    /// A session is working in ticket `number`'s worktree `w` (or a folder inside it).
    fn someone_in(&self, w: &Worktree, number: u32) -> bool {
        let root = worktree_root(w, number);
        self.wt.busy_dirs.iter().any(|d| d.starts_with(&root))
    }

    /// Any of ticket `t`'s squads' sessions is running.
    fn squad_running(&self, t: &Ticket) -> bool {
        t.leaders.iter().any(|l| self.leader_session_running(&l.session))
    }

    fn working_on(&self, project: &str, number: u32) -> bool {
        let is = |p: &str, n: u32| p == project && n == number;
        self.wt.removing.as_ref().is_some_and(|(o, _)| is(&o.project, o.number))
            || self.wt.back.as_ref().is_some_and(|(o, _)| is(&o.project, o.number))
    }

    /// Called on every refresh: take in what came back, and every couple of seconds look for a closed ticket whose
    /// squad has stopped and whose worktrees are still there. One ticket at a time, off the UI thread.
    pub(super) fn poll_worktrees(&mut self) {
        self.take_removals();
        self.take_back();
        if self.wt.removing.is_some() || self.wt.last.is_some_and(|t| t.elapsed() < EVERY) {
            return;
        }
        self.wt.last = Some(Instant::now());
        let mut pick: Option<(String, u32, u64, Vec<Worktree>)> = None;
        for (project, t) in self.every_ticket() {
            // (`v` on the ticket is on its way to opening the editor there: left alone until it has.)
            // (Nor while its tests are due or running there.)
            if !t.state.closed()
                || t.worktrees.is_empty()
                || t.check_due.is_some()
                || self.check_running(project, t.number)
                || self.squad_running(t)
                || self.working_on(project, t.number)
                || self.editor_holds(project, t.number)
            {
                continue;
            }
            let todo: Vec<Worktree> = t
                .worktrees
                .iter()
                // (One opened in a windowed editor is the user's to remove: platoon can't tell when that closes.)
                .filter(|w| w.removed.is_none() && w.opened_in.is_none() && !self.someone_in(w, t.number))
                .filter(|w| !self.wt.tried.contains(&(project.clone(), t.number, w.path.clone(), t.updated)))
                .cloned()
                .collect();
            if !todo.is_empty() {
                pick = Some((project.clone(), t.number, t.updated, todo));
                break;
            }
        }
        if let Some((project, number, updated, todo)) = pick {
            for w in &todo {
                self.wt.tried.insert((project.clone(), number, w.path.clone(), updated));
            }
            self.start_removing(project, number, todo, false);
        }
    }

    fn start_removing(&mut self, project: String, number: u32, todo: Vec<Worktree>, anyway: bool) {
        let git = self.git_program.clone();
        // A task moved to another project keeps the number its branch was made with.
        let branch_no = Tickets::open(&self.config, &project).get(number).map_or(number, Ticket::branch_number);
        let (tx, rx) = channel();
        thread::spawn(move || {
            let out: Vec<(String, Removal)> =
                todo.iter().map(|w| (w.path.clone(), worktrees::remove(&git, w, branch_no, anyway))).collect();
            let _ = tx.send(out);
        });
        self.wt.removing = Some((Out { project, number, rx }, anyway));
    }

    /// What a removal came to, written on the ticket: removed, or kept with changes not yet committed. One platoon
    /// refused to touch is only noted in memory (tried once), and said when the user asked.
    fn take_removals(&mut self) {
        let Some((out, anyway)) = &self.wt.removing else { return };
        let got = match out.rx.try_recv() {
            Ok(got) => got,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Vec::new(),
        };
        let (project, number, anyway) = (out.project.clone(), out.number, *anyway);
        self.wt.removing = None;
        let mut store = Tickets::open(&self.config, &project);
        let branch = store.get(number).map(Ticket::branch).unwrap_or_else(|| crate::tickets::task_branch(number));
        let mut refused = None;
        let (mut removed, mut newly_kept) = (0, 0);
        for (path, r) in &got {
            let mark = match r {
                Removal::Removed => WtMark::Removed,
                Removal::Uncommitted => WtMark::KeptUncommitted,
                Removal::Refused(why) => {
                    refused.get_or_insert_with(|| why.clone());
                    continue;
                }
            };
            match store.mark_worktree(number, path, mark) {
                Ok(changed) => match mark {
                    WtMark::Removed => removed += 1,
                    _ if changed => newly_kept += 1,
                    _ => {}
                },
                Err(e) => {
                    refused.get_or_insert_with(|| e.to_string());
                }
            }
        }
        self.reread_all_tickets();
        if anyway {
            match refused {
                Some(why) => self.notify(format!("#{number}'s worktree wasn't removed: {why}")),
                None => self.notify(format!("#{number}'s worktree is removed. Its branch {branch} is kept.")),
            }
        } else if !self.editor_holds(&project, number) {
            // (`v` on the ticket waited for this removal and is about to bring the worktree back: nothing to say.)
            if let Some(line) = removal_notice(number, &branch, removed, newly_kept) {
                self.notify(line);
            }
        }
    }

    // ---- continuing a squad whose worktree was removed ----------------------------------------------------------

    /// Before ticket `number`'s squad is continued: when a worktree of it is gone (removed after the work, or by
    /// hand), make it again from its branch, off the UI thread, and continue the squad once it is there. True when
    /// that is under way (or the ticket must wait a moment), false when there is nothing to bring back.
    pub(super) fn bring_worktrees_back(&mut self, number: u32) -> bool {
        self.bring_worktrees_back_then(number, After::Squad)
    }

    /// `bring_worktrees_back`, then `then`.
    pub(super) fn bring_worktrees_back_then(&mut self, number: u32, then: After) -> bool {
        let Some(project) = self.task_scope() else { return false };
        let Some(t) = self.ticket(number).cloned() else { return false };
        let gone: Vec<String> = t.worktrees.iter().filter(|w| w.is_gone()).map(|w| w.folder.clone()).collect();
        if gone.is_empty() {
            return false;
        }
        if self.working_on(&project, number) {
            self.notify(format!("#{number}: platoon is busy with its worktree just now. Try again in a moment."));
            return true;
        }
        if self.wt.back.is_some() {
            self.notify("platoon is bringing another task's worktree back just now. Try again in a moment.");
            return true;
        }
        let git = self.git_program.clone();
        let recorded = t.worktrees.clone();
        let (tx, rx) = channel();
        thread::spawn(move || {
            let _ = tx.send(crate::squads::prepare_worktrees(&git, &gone, number, &recorded));
        });
        if let Some(store) = self.tk.store.as_mut() {
            store.set_busy(number, Some(BRINGING_BACK));
        }
        self.wt.back = Some((Out { project, number, rx }, then));
        true
    }

    /// `v` on ticket `number`: when platoon has just started removing its worktree (it went Done and its squad
    /// stopped a moment ago), wait for that to finish and write it down, so the editor opens on a worktree that
    /// stays, brought back from its branch if it went.
    pub(super) fn settle_removal(&mut self, project: &str, number: u32) {
        let start = Instant::now();
        while self.wt.removing.as_ref().is_some_and(|(o, _)| o.project == project && o.number == number) {
            self.take_removals();
            if start.elapsed() > REMOVAL_WAIT {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// The worktrees are back: written on the ticket, then its squad continued (beside the ticket when its screen is
    /// still showing).
    fn take_back(&mut self) {
        let Some((out, then)) = &self.wt.back else { return };
        let then = *then;
        let got = match out.rx.try_recv() {
            Ok(got) => got,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("it stopped half way".into()),
        };
        let (project, number) = (out.project.clone(), out.number);
        self.wt.back = None;
        if let Some(store) = self.tk.store.as_mut().filter(|s| s.project() == project) {
            store.set_busy(number, None);
        }
        let made = match got {
            Ok(made) => made,
            Err(e) => {
                self.reread_all_tickets();
                self.notify(format!("Couldn't bring #{number}'s worktree back: {e}"));
                if then == After::Editor {
                    self.editor_cancel();
                }
                return;
            }
        };
        let mut store = Tickets::open(&self.config, &project);
        for w in &made {
            let _ = store.mark_worktree(number, &w.path, WtMark::Present);
        }
        self.reread_all_tickets();
        if then == After::Editor {
            self.editor_continue();
            return;
        }
        let show =
            self.task_scope().as_deref() == Some(project.as_str()) && self.tk.open == Some(number) && matches!(self.mode, Mode::List);
        if let Err(e) = self.launch_leader_in(&project, number, None, show) {
            self.notify(format!("Couldn't start the squad: {e}"));
        }
    }

    // ---- the user's say ---------------------------------------------------------------------------------------

    /// The worktrees of ticket `number` that were kept after the work: they have changes not yet committed, or they
    /// were opened in an editor with a window of its own.
    pub fn kept_worktrees(&self, number: u32) -> Vec<Worktree> {
        self.ticket(number)
            .map(|t| t.worktrees.iter().filter(|w| (w.kept_uncommitted || w.opened_in.is_some()) && w.removed.is_none()).cloned().collect())
            .unwrap_or_default()
    }

    /// `w` would remove ticket `number`'s kept worktree now: it is Done or Cancelled, one was kept, and no squad of
    /// it is running.
    pub fn can_remove_worktree(&self, number: u32) -> bool {
        self.ticket(number).is_some_and(|t| t.state.closed() && !self.squad_running(t)) && !self.kept_worktrees(number).is_empty()
    }

    /// Does ticket `number` have a worktree with changes not yet committed? Asked before cancelling it, with git
    /// given a moment (false when it can't say in time).
    pub(super) fn has_uncommitted_worktree(&self, number: u32) -> bool {
        let Some(t) = self.ticket(number) else { return false };
        t.worktrees
            .iter()
            .filter(|w| w.removed.is_none() && Path::new(&w.path).is_dir())
            .any(|w| w.kept_uncommitted || worktrees::uncommitted_quick(&self.git_program, Path::new(&w.path), ASK_BUDGET))
    }

    /// `w` on a ticket: remove its worktree that was kept for changes not yet committed, after asking.
    pub(super) fn ask_remove_worktree(&mut self, number: u32) {
        let Some(t) = self.ticket(number) else { return };
        if self.kept_worktrees(number).is_empty() {
            let why = if t.worktrees.iter().any(|w| w.removed.is_none()) && !t.state.closed() {
                format!("#{number}'s worktree goes by itself once the task is Done or cancelled.")
            } else {
                format!("#{number} has no worktree kept after the work.")
            };
            self.notify(why);
            return;
        }
        if self.squad_running(t) {
            self.notify(format!("#{number}'s squad's session is running in its worktree. Stop it first."));
            return;
        }
        if !t.state.closed() {
            self.notify(format!("#{number} is {}: its worktree is in use.", t.state.label()));
            return;
        }
        let kept = self.kept_worktrees(number);
        let opened_in = kept.iter().find_map(|w| w.opened_in.clone());
        let uncommitted =
            kept.iter().any(|w| w.kept_uncommitted || worktrees::uncommitted_quick(&self.git_program, Path::new(&w.path), ASK_BUDGET));
        let branch = t.branch();
        self.mode = Mode::Confirm(Confirm::RemoveWorktree { number, branch, uncommitted, opened_in });
    }

    /// Yes to "Remove #N's worktree anyway?": removed with its changes (the branch kept), off the UI thread.
    pub(super) fn remove_worktree_anyway(&mut self, number: u32) {
        self.mode = Mode::List;
        let Some(project) = self.task_scope() else { return };
        let todo: Vec<Worktree> = self.kept_worktrees(number);
        if todo.is_empty() || !self.can_remove_worktree(number) {
            return;
        }
        if todo.iter().any(|w| self.someone_in(w, number)) {
            self.notify(format!("A session is working in #{number}'s worktree. Close it first."));
            return;
        }
        if self.wt.removing.is_some() {
            self.notify("platoon is removing another worktree just now. Try again in a moment.");
            return;
        }
        self.start_removing(project, number, todo, true);
        self.notify(format!("Removing #{number}'s worktree…"));
    }

    /// How many worktrees have been looked at (in a unit test).
    #[cfg(test)]
    pub fn worktrees_tried_for_test(&self) -> usize {
        self.wt.tried.len()
    }

    /// Wait (in a unit test) until no worktree is being removed or made again.
    #[cfg(test)]
    pub fn worktrees_idle(&self) -> bool {
        self.wt.removing.is_none() && self.wt.back.is_none()
    }
}
