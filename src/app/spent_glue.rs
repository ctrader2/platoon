//! What tasks' sessions have spent, for the Tasks tab: each session's transcript (and its subagents' beside it) read by
//! `claude::read_session_log` on a thread of its own, every couple of seconds, and only again when a file changed (by size
//! and time). Only the sessions of the tasks on the board are read. What is shown from it is worked out in `spent.rs`.

use super::*;
use crate::agents::{SessionLog, Spent};
use crate::tickets::Ticket;
use std::sync::mpsc::TryRecvError;
use std::time::SystemTime;

/// How often the transcripts are looked at again.
const EVERY: Duration = Duration::from_secs(2);

/// Each file a session's reading came from, with its size and time: the same again means nothing to read.
type Stamp = Vec<(PathBuf, u64, Option<SystemTime>)>;

#[derive(Clone, Debug, Default)]
struct Reading {
    /// Where the transcript was found (looked for again only when it is gone).
    path: PathBuf,
    stamp: Stamp,
    log: Option<SessionLog>,
}

#[derive(Default)]
pub struct WorkState {
    /// By session id.
    logs: HashMap<String, Reading>,
    rx: Option<Receiver<HashMap<String, Reading>>>,
    last: Option<Instant>,
}

/// The files a session's reading depends on: its transcript and everything in its `subagents` folder.
fn stamp_of(path: &Path) -> Stamp {
    let st = |p: &Path| std::fs::metadata(p).ok().map(|m| (p.to_path_buf(), m.len(), m.modified().ok()));
    let mut out: Stamp = st(path).into_iter().collect();
    let mut subs: Stamp = std::fs::read_dir(path.with_extension("").join("subagents"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| st(&e.path()))
        .collect();
    subs.sort_by(|a, b| a.0.cmp(&b.0));
    out.extend(subs);
    out
}

/// Read what changed (off the UI thread): each session's transcript found under `root`, read again only when its
/// files are not as they were.
fn read_all(root: &Path, sessions: &[String], prev: &HashMap<String, Reading>) -> HashMap<String, Reading> {
    let mut out = prev.clone();
    for id in sessions {
        let known = prev.get(id).map(|r| r.path.clone()).filter(|p| p.is_file());
        let Some(path) = known.or_else(|| claude::find_transcript(root, id)) else {
            out.remove(id);
            continue;
        };
        let stamp = stamp_of(&path);
        if prev.get(id).is_some_and(|r| r.stamp == stamp) {
            continue;
        }
        out.insert(id.clone(), Reading { log: claude::read_session_log(&path), path, stamp });
    }
    out
}

impl App {
    /// Take in the last reading, and start another every couple of seconds while the board has tasks with sessions.
    /// Called on every refresh; nothing is read on the UI thread.
    pub(super) fn poll_squad_work(&mut self) {
        if let Some(rx) = &self.sw.rx {
            match rx.try_recv() {
                Ok(logs) => {
                    self.sw.logs = logs;
                    self.sw.rx = None;
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => self.sw.rx = None,
            }
        }
        if self.sw.last.is_some_and(|t| t.elapsed() < EVERY) || !self.board_showing() {
            return;
        }
        let sessions = self.sessions_to_read();
        if sessions.is_empty() {
            return;
        }
        self.sw.last = Some(Instant::now());
        let root = self.reader.root().to_path_buf();
        let prev = self.sw.logs.clone();
        let (tx, rx) = channel();
        thread::spawn(move || {
            let _ = tx.send(read_all(&root, &sessions, &prev));
        });
        self.sw.rx = Some(rx);
    }

    /// The sessions whose transcripts are read: those of every task on the board (in play or finished today).
    fn sessions_to_read(&self) -> Vec<String> {
        let now = usage::now();
        let on_board = |t: &Ticket| !t.state.closed() || now.saturating_sub(t.updated) < crate::board::FINISHED_FOR;
        self.all_tasks().iter().flat_map(|(_, list)| list.iter()).filter(|t| on_board(t)).filter_map(|t| t.session.clone()).collect()
    }

    /// What was last read of this session's transcript.
    pub fn session_log(&self, session: &str) -> Option<&SessionLog> {
        self.sw.logs.get(session).and_then(|r| r.log.as_ref())
    }

    /// What this session spent, its subagents too. None until it has been read.
    pub fn session_spent(&self, session: &str) -> Option<Spent> {
        self.session_log(session).map(|l| l.spent)
    }
}
