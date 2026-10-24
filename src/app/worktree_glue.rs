//! Tasks' worktrees inside the app, after the work. A task that is Done or Cancelled, whose session is no longer
//! running, has its worktree removed (its branch kept); one with changes not yet committed is kept, and so is one
//! opened in an editor with a window of its own (`v`), since platoon can't tell when that window closes. Opening such a
//! task's session again (enter) makes the worktree again from its branch first. The git side, and every check on what
//! may be touched, is in `worktrees.rs`.
//!
//! Why here and not in a hook: a task goes Done in its session's Stop hook, while the session is still running in the
//! worktree (the user may carry on talking to it). Only the app sees both the tasks and which sessions run, so it waits
//! until the session has stopped, then removes the worktree off the UI thread.

use super::*;
use crate::tickets::{Ticket, Tickets, Worktree, WtMark};
use crate::worktrees::{self, Removal};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// How often the closed tasks are looked over for worktrees to remove.
const EVERY: Duration = Duration::from_secs(2);

/// How long `v` waits for a removal of the task's worktree that had just started.
const REMOVAL_WAIT: Duration = Duration::from_secs(10);

/// How long `c` may wait for git to say whether a worktree has changes, before asking.
const ASK_BUDGET: Duration = Duration::from_millis(1500);

/// What happens once a task's removed worktree is back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum After {
    /// Its session is continued.
    Session,
    /// The code editor opens on it (`v`).
    Editor,
}

/// One task's worktree, being removed or made again off the UI thread.
struct Out<T> {
    project: String,
    number: u32,
    rx: Receiver<T>,
}

/// What making a worktree again comes back with (see `squads_glue::make_worktree`).
type Made = super::squads_glue::Made;

/// What removing a worktree comes back with: the removal, and for a folder platoon had Codex trust, what came of
/// taking that trust away (how many entries went, or why Codex couldn't be asked).
type Removed = (Removal, Option<Result<usize, String>>);

#[derive(Default)]
pub struct WtState {
    /// A worktree being removed.
    removing: Option<Out<Removed>>,
    /// A removed worktree being made again from its branch.
    back: Option<(Out<Made>, After)>,
    /// Worktrees already looked at for the task as it stood (project, number, the task's `updated`): each is tried once,
    /// and again only when the task changes (or platoon starts again), never over and over.
    tried: HashSet<(String, u32, u64)>,
    last: Option<Instant>,
    /// The folders sessions are working in now (every running claude's, every platoon host's, every open editor's).
    busy_dirs: Vec<PathBuf>,
}

impl App {
    /// Note where sessions are working now (from Claude Code's registry and platoon's hosts), and where a code editor
    /// has a folder open, in this platoon window or another (`editor::open_now`).
    pub(super) fn note_busy_dirs(&mut self, live: &[Live]) {
        self.wt.busy_dirs = live
            .iter()
            .map(|l| PathBuf::from(&l.cwd))
            .chain(self.hosts.iter().map(|h| PathBuf::from(&h.dir)))
            .chain(crate::editor::open_now(&self.cache))
            .filter(|d| !d.as_os_str().is_empty())
            .collect();
    }

    /// A session is working in task `number`'s worktree `w` (or a folder inside it).
    fn someone_in(&self, w: &Worktree, number: u32) -> bool {
        let root = worktrees::root_of(w, number);
        self.wt.busy_dirs.iter().any(|d| d.starts_with(&root))
    }

    fn working_on(&self, project: &str, number: u32) -> bool {
        let is = |p: &str, n: u32| p == project && n == number;
        self.wt.removing.as_ref().is_some_and(|o| is(&o.project, o.number))
            || self.wt.back.as_ref().is_some_and(|(o, _)| is(&o.project, o.number))
    }

    /// Called on every refresh: take in what came back, and every couple of seconds look for a closed task whose session
    /// has stopped and whose worktree is still there. One task at a time, off the UI thread.
    pub(super) fn poll_worktrees(&mut self) {
        self.take_removal();
        self.take_back();
        if self.wt.removing.is_some() || self.wt.last.is_some_and(|t| t.elapsed() < EVERY) {
            return;
        }
        self.wt.last = Some(Instant::now());
        let pick = self.every_ticket().iter().find_map(|(project, t)| {
            let w = t.worktree.as_ref()?;
            let skip = !t.state.closed()
                || self.task_running(project, t)
                || self.working_on(project, t.number)
                || self.editor_holds(project, t.number)
                // (One opened in a windowed editor is the user's to remove: platoon can't tell when that closes.)
                || w.removed.is_some()
                || w.opened_in.is_some()
                || self.someone_in(w, t.number)
                || self.wt.tried.contains(&(project.clone(), t.number, t.updated));
            (!skip).then(|| (project.clone(), t.clone()))
        });
        if let Some((project, t)) = pick {
            self.wt.tried.insert((project.clone(), t.number, t.updated));
            self.start_removing(project, &t);
        }
    }

    /// Remove task `t`'s worktree off the UI thread, and once it is gone take away the trust platoon had Codex give
    /// its folder (`codex_server::forget_folders`), when it did.
    fn start_removing(&mut self, project: String, t: &Ticket) {
        let (Some(w), Some(folder)) = (t.worktree.clone(), t.folder.clone()) else { return };
        let git = self.git_program.clone();
        let number = t.number;
        let forget = w.codex_trust.clone().zip(self.providers.codex.clone());
        let (tx, rx) = channel();
        thread::spawn(move || {
            let removal = worktrees::remove(&git, &folder, &w, number, false);
            let forgot = match (&removal, forget) {
                (Removal::Removed, Some((key, c))) => {
                    Some(crate::agents::codex_server::forget_folders(&c.program, &c.root, &[key], crate::agents::codex_server::TIME_BOX))
                }
                _ => None,
            };
            let _ = tx.send((removal, forgot));
        });
        self.wt.removing = Some(Out { project, number, rx });
    }

    /// What a removal came to, written on the task: removed, or kept with changes not yet committed. One platoon refused
    /// to touch is only noted in memory (tried once).
    fn take_removal(&mut self) {
        let Some(out) = &self.wt.removing else { return };
        let (got, forgot) = match out.rx.try_recv() {
            Ok(got) => got,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => (Removal::Refused("it stopped half way".into()), None),
        };
        let (project, number) = (out.project.clone(), out.number);
        self.wt.removing = None;
        let mut store = Tickets::open(&self.config, &project);
        // Codex no longer trusts the folder: nothing of platoon's is left in its settings for it. (When Codex couldn't
        // be asked, the record stays: making the worktree again finds its trust there, and `platoon uninstall` takes it
        // away.)
        if let Some(Ok(_)) = forgot {
            let _ = store.set_codex_trust(number, None);
        }
        let branch = store.get(number).map(Ticket::branch).unwrap_or_else(|| crate::tickets::task_branch(number));
        let said = match got {
            Removal::Removed => store
                .mark_worktree(number, WtMark::Removed)
                .ok()
                .map(|_| format!("Removed #{number}'s worktree (its branch {branch} is kept).")),
            Removal::Uncommitted => match store.mark_worktree(number, WtMark::KeptUncommitted) {
                Ok(true) => Some(format!("Kept #{number}'s worktree: it has changes not yet committed.")),
                _ => None,
            },
            Removal::Refused(_) => None,
        };
        self.reread_tickets();
        if let Some(s) = self.tk.store.as_mut() {
            s.reload_if_changed();
        }
        // (`v` on the task waited for this removal and is about to bring the worktree back: nothing to say.)
        if let Some(line) = said.filter(|_| !self.editor_holds(&project, number)) {
            self.notify(line);
        }
    }

    // ---- bringing a removed worktree back ------------------------------------------------------------------------

    /// Before task `number`'s session is continued: when its worktree is gone (removed after the work, or by hand),
    /// make it again from its branch, off the UI thread, and continue the session once it is there. True when that is
    /// under way (or the task must wait a moment), false when there is nothing to bring back.
    pub(super) fn bring_worktree_back(&mut self, number: u32) -> bool {
        self.bring_worktree_back_then(number, After::Session)
    }

    /// `bring_worktree_back`, then `then`.
    pub(super) fn bring_worktree_back_then(&mut self, number: u32, then: After) -> bool {
        let Some(project) = self.task_scope() else { return false };
        let Some(t) = self.ticket(number).cloned() else { return false };
        let (Some(w), Some(folder)) = (t.worktree.clone(), t.folder.clone()) else { return false };
        if !w.is_gone() {
            return false;
        }
        if self.working_on(&project, number) || self.wt.back.is_some() {
            self.notify(format!("#{number}: platoon is busy with a worktree just now. Try again in a moment."));
            return true;
        }
        let git = self.git_program.clone();
        // A Codex task's folder is trusted in Codex again, as when it was first made.
        let on_codex = self.task_squad(&project, &t).and_then(|s| self.task_agent(&s)) == Ok(Agent::Codex);
        let codex = self.providers.codex.clone().filter(|_| on_codex);
        let (tx, rx) = channel();
        thread::spawn(move || {
            let _ = tx.send(super::squads_glue::make_worktree(&git, &folder, number, Some(&w), codex.as_ref()));
        });
        self.notify(format!("Bringing #{number}'s worktree back from its branch first…"));
        self.wt.back = Some((Out { project, number, rx }, then));
        true
    }

    /// `v` on task `number`: when platoon has just started removing its worktree (it went Done and its session stopped a
    /// moment ago), wait for that to finish and write it down, so the editor opens on a worktree that stays.
    pub(super) fn settle_removal(&mut self, project: &str, number: u32) {
        let start = Instant::now();
        while self.wt.removing.as_ref().is_some_and(|o| o.project == project && o.number == number) {
            self.take_removal();
            if start.elapsed() > REMOVAL_WAIT {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// The worktree is back: written on the task, then its session continued (or the editor opened).
    fn take_back(&mut self) {
        let Some((out, then)) = &self.wt.back else { return };
        let then = *then;
        let (got, trust) = match out.rx.try_recv() {
            Ok(got) => got,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => (Err("it stopped half way".into()), None),
        };
        let (project, number) = (out.project.clone(), out.number);
        self.wt.back = None;
        let made = match got {
            Ok(made) => made,
            Err(e) => {
                self.notify(format!("Couldn't bring #{number}'s worktree back: {e}"));
                if then == After::Editor {
                    self.editor_cancel();
                }
                return;
            }
        };
        let mut store = Tickets::open(&self.config, &project);
        let _ = store.mark_worktree(number, WtMark::Present);
        match (trust, made) {
            (Some(Ok(key)), _) => {
                let _ = store.set_codex_trust(number, key.as_deref());
            }
            (Some(Err(why)), Some(w)) => {
                self.notify(super::squads_glue::trust_failed(number, &worktrees::root_of(&w, number), &why, &self.home));
            }
            _ => {}
        }
        self.reread_tickets();
        if let Some(s) = self.tk.store.as_mut() {
            s.reload_if_changed();
        }
        if then == After::Editor {
            self.editor_continue();
            return;
        }
        let show = matches!(self.mode, Mode::List);
        if let Err(e) = self.launch_task(&project, number, true, show) {
            self.notify(format!("Couldn't open #{number}'s session: {e}"));
        }
    }

    /// Does task `number` have a worktree with changes not yet committed? Asked before cancelling it, with git given a
    /// moment (false when it can't say in time).
    pub(super) fn has_uncommitted_worktree(&self, number: u32) -> bool {
        let Some(w) = self.ticket(number).and_then(|t| t.worktree.as_ref()) else { return false };
        w.removed.is_none()
            && Path::new(&w.path).is_dir()
            && (w.kept_uncommitted || worktrees::uncommitted_quick(&self.git_program, Path::new(&w.path), ASK_BUDGET))
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
