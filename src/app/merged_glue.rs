//! Whether a Done task's work is merged, for the Tasks tab: for each Done task that worked on a branch, how many
//! commits `task/<N>` has that its repository's default branch hasn't (`gitfacts::not_merged`), read with git on a
//! thread of its own. A Done task with such commits reads "Done · not merged" (`board::row`), its details say how many
//! and where, and it stays on Active however long ago it finished (`board::place`).
//!
//! Every such task is read once soon after platoon starts, whatever tab shows, so the lists are right by the time the
//! Tasks tab is looked at; after that, every few seconds while the tab shows, the ones whose answer can still change
//! without a session working on them: those finished in the last day and those not merged (the user merges them).

use super::*;
use crate::board::Merge;
use crate::gitfacts::{self, Unmerged};
use crate::tickets::{State, Ticket};
use std::sync::mpsc::TryRecvError;

/// How often the branches are looked at again.
const EVERY: Duration = Duration::from_secs(5);

/// How long git may take over one task's branch.
const BUDGET: Duration = Duration::from_secs(5);

/// A task: its project's id and its number.
type Key = (String, u32);

/// What one reading found: each task asked about, with its branch's commits the default branch hasn't.
type Found = Vec<(Key, Option<Unmerged>)>;

#[derive(Default)]
pub struct MergeState {
    /// What git last said about each Done task's branch it was asked about: the commits the default branch hasn't
    /// (None: none, it is merged; also when git couldn't say, or there is no repository to ask).
    read: HashMap<Key, Option<Unmerged>>,
    rx: Option<Receiver<Found>>,
    last: Option<Instant>,
}

/// Where git is asked about a task's branch: the folder it was started from (its repository), else its worktree.
fn repo_of(t: &Ticket) -> Option<PathBuf> {
    [t.folder.clone(), t.worktree.as_ref().map(|w| w.path.clone())].into_iter().flatten().map(PathBuf::from).find(|p| p.is_dir())
}

impl App {
    /// Take in the last reading, and start another: at once for a Done task not read yet, and every few seconds while
    /// the Tasks tab shows for the ones whose answer may change. Called on every refresh; git never runs on the UI
    /// thread.
    pub(super) fn poll_merged(&mut self) {
        if let Some(rx) = &self.merged.rx {
            match rx.try_recv() {
                Ok(found) => {
                    self.merged.read.extend(found);
                    self.merged.rx = None;
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => self.merged.rx = None,
            }
        }
        let now = usage::now();
        let done: Vec<(Key, &Ticket)> = self
            .every_ticket()
            .iter()
            .filter(|(_, t)| t.state == State::Done && t.worktree.is_some())
            .map(|(p, t)| ((p.clone(), t.number), t))
            .collect();
        let read = &self.merged.read;
        let due = self.board_showing() && self.merged.last.is_none_or(|t| t.elapsed() >= EVERY);
        let may_change =
            |k: &Key, t: &Ticket| now.saturating_sub(t.updated) < crate::board::FINISHED_FOR || read.get(k).is_some_and(Option::is_some);
        let asks: Vec<(Key, Option<PathBuf>, String)> = done
            .iter()
            .filter(|(k, t)| !read.contains_key(k) || (due && may_change(k, t)))
            .map(|(k, t)| (k.clone(), repo_of(t), t.branch()))
            .collect();
        // A task that is no longer Done (worked on again, or gone) is read afresh when it is Done again.
        let keep: HashSet<Key> = done.into_iter().map(|(k, _)| k).collect();
        self.merged.read.retain(|k, _| keep.contains(k));
        if asks.is_empty() {
            return;
        }
        self.merged.last = Some(Instant::now());
        let git = self.git_program.clone();
        let (tx, rx) = channel();
        thread::spawn(move || {
            let found = asks
                .into_iter()
                .map(|(key, dir, branch)| {
                    let unmerged = dir.and_then(|dir| gitfacts::not_merged(&git, &dir, &branch, Instant::now() + BUDGET));
                    (key, unmerged)
                })
                .collect();
            let _ = tx.send(found);
        });
        self.merged.rx = Some(rx);
    }

    /// The commits task `number` of `project` has that its repository's default branch hasn't, as last read: only for a
    /// Done task that has some.
    pub fn not_merged(&self, project: &str, number: u32) -> Option<&Unmerged> {
        self.merged.read.get(&(project.to_string(), number)).and_then(Option::as_ref)
    }

    /// What git last said about task `t` of `project`'s branch: Merged for a task with no branch, Unknown until its
    /// branch has been read once.
    pub fn merge_of(&self, project: &str, t: &Ticket) -> Merge {
        match self.merged.read.get(&(project.to_string(), t.number)) {
            _ if t.worktree.is_none() => Merge::Merged,
            None => Merge::Unknown,
            Some(None) => Merge::Merged,
            Some(Some(_)) => Merge::NotMerged,
        }
    }
}

#[cfg(test)]
impl App {
    /// Read the branches again at the next refresh, without waiting for `EVERY`.
    pub fn read_merged_soon_for_test(&mut self) {
        self.merged.last = None;
    }
}
