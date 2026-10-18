//! Whether a Done task's work is merged, for the Tasks tab: for each Done task on the board that worked on a branch,
//! how many commits `task/<N>` has that its repository's default branch hasn't (`gitfacts::not_merged`), read with git
//! on a thread of its own every few seconds while the tab shows. A Done task with such commits reads "Done · not
//! merged" (`board::row`), and its details say how many and where.

use super::*;
use crate::gitfacts::{self, Unmerged};
use crate::tickets::{State, Ticket};
use std::sync::mpsc::TryRecvError;

/// How often the branches are looked at again.
const EVERY: Duration = Duration::from_secs(5);

/// How long git may take over one task's branch.
const BUDGET: Duration = Duration::from_secs(5);

/// A task: its project's id and its number.
type Key = (String, u32);

#[derive(Default)]
pub struct MergeState {
    /// The Done tasks whose branch has commits the default branch hasn't.
    known: HashMap<Key, Unmerged>,
    rx: Option<Receiver<HashMap<Key, Unmerged>>>,
    last: Option<Instant>,
}

/// Where git is asked about a task's branch: the folder it was started from (its repository), else its worktree.
fn repo_of(t: &Ticket) -> Option<PathBuf> {
    [t.folder.clone(), t.worktree.as_ref().map(|w| w.path.clone())].into_iter().flatten().map(PathBuf::from).find(|p| p.is_dir())
}

impl App {
    /// Take in the last reading, and start another every few seconds while the board shows Done tasks that worked on a
    /// branch. Called on every refresh; git never runs on the UI thread.
    pub(super) fn poll_merged(&mut self) {
        if let Some(rx) = &self.merged.rx {
            match rx.try_recv() {
                Ok(known) => {
                    self.merged.known = known;
                    self.merged.rx = None;
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => self.merged.rx = None,
            }
        }
        if self.merged.last.is_some_and(|t| t.elapsed() < EVERY) || !self.board_showing() {
            return;
        }
        let now = usage::now();
        let asks: Vec<(Key, PathBuf, String)> = self
            .all_tasks()
            .iter()
            .flat_map(|(p, list)| list.iter().map(move |t| (p, t)))
            .filter(|(_, t)| t.state == State::Done && t.worktree.is_some() && now.saturating_sub(t.updated) < crate::board::FINISHED_FOR)
            .filter_map(|(p, t)| Some(((p.clone(), t.number), repo_of(t)?, t.branch())))
            .collect();
        if asks.is_empty() {
            self.merged.known.clear();
            return;
        }
        self.merged.last = Some(Instant::now());
        let git = self.git_program.clone();
        let (tx, rx) = channel();
        thread::spawn(move || {
            let known = asks
                .into_iter()
                .filter_map(|(key, dir, branch)| Some((key, gitfacts::not_merged(&git, &dir, &branch, Instant::now() + BUDGET)?)))
                .collect();
            let _ = tx.send(known);
        });
        self.merged.rx = Some(rx);
    }

    /// The commits task `number` of `project` has that its repository's default branch hasn't, as last read: only for a
    /// Done task that has some.
    pub fn not_merged(&self, project: &str, number: u32) -> Option<&Unmerged> {
        self.merged.known.get(&(project.to_string(), number))
    }
}

#[cfg(test)]
impl App {
    /// Read the branches again at the next refresh, without waiting for `EVERY`.
    pub fn read_merged_soon_for_test(&mut self) {
        self.merged.last = None;
    }
}
