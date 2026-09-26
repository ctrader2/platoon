//! What the open project's squads have done, for the ticket screen and the Tickets pane: each leader's transcript
//! (and its members' beside it) read by `claude::read_leader` on a thread of its own, every couple of seconds, and
//! only again when a file changed (by size and time). Only tickets that have leaders are read. What is shown from it
//! (tokens per ticket and per agent, the members' runs) is worked out in `squadwork.rs`.

use super::*;
use crate::claude::LeaderLog;
use crate::squadwork::{self, TicketTokens};
use crate::tickets::Ticket;
use std::sync::mpsc::TryRecvError;
use std::time::SystemTime;

/// How often the transcripts are looked at again.
const EVERY: Duration = Duration::from_secs(2);

/// Each file a leader's reading came from, with its size and time: the same again means nothing to read.
type Stamp = Vec<(PathBuf, u64, Option<SystemTime>)>;

#[derive(Clone, Debug, Default)]
struct Reading {
    /// Where the transcript was found (looked for again only when it is gone).
    path: PathBuf,
    stamp: Stamp,
    log: Option<LeaderLog>,
}

#[derive(Default)]
pub struct WorkState {
    /// By leader session id.
    logs: HashMap<String, Reading>,
    rx: Option<Receiver<HashMap<String, Reading>>>,
    last: Option<Instant>,
}

/// The files a leader's reading depends on: its transcript and everything in its `subagents` folder.
fn stamp_of(path: &Path) -> Stamp {
    let st = |p: &Path| std::fs::metadata(p).ok().map(|m| (p.to_path_buf(), m.len(), m.modified().ok()));
    let mut out: Stamp = st(path).into_iter().collect();
    let mut subs: Stamp = std::fs::read_dir(path.with_extension("").join("subagents")).into_iter().flatten().flatten().filter_map(|e| st(&e.path())).collect();
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
        out.insert(id.clone(), Reading { log: claude::read_leader(&path), path, stamp });
    }
    out
}

impl App {
    /// Take in the last reading, and start another every couple of seconds while the open project has tickets with
    /// leaders. Called on every refresh; nothing is read on the UI thread.
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
        if self.sw.last.is_some_and(|t| t.elapsed() < EVERY) {
            return;
        }
        let sessions: Vec<String> = self.tickets().iter().flat_map(|t| t.leaders.iter().map(|l| l.session.clone())).collect();
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

    /// What was last read of this leader session's transcript.
    pub fn leader_log(&self, session: &str) -> Option<&LeaderLog> {
        self.sw.logs.get(session).and_then(|r| r.log.as_ref())
    }

    /// The tokens ticket `t`'s squads spent, over every leader it has had. None until one has been read.
    pub fn ticket_tokens(&self, t: &Ticket) -> Option<TicketTokens> {
        let leaders: Vec<(String, Option<&LeaderLog>)> =
            t.leaders.iter().map(|l| (crate::squads::agent_name(&l.squad, crate::squads::LEADER_ROLE, l.names), self.leader_log(&l.session))).collect();
        squadwork::ticket_tokens(&leaders)
    }

    /// Is this leader session running now (hosted by a platoon, or registered by Claude Code)?
    pub fn leader_session_running(&self, session: &str) -> bool {
        self.leader_host(session).is_some() || self.sq.running_contains(session)
    }
}
