//! What tasks' sessions have spent, for the Tasks tab: each session's transcript (and its subagents' beside it) read by
//! `claude::read_session_log` on a thread of its own, every couple of seconds, and only again when a file changed (by size
//! and time); a Codex task's session from its rollout (`Agent::spent`, Codex's running total). Only the sessions of the
//! tasks on the list showing (Active or Archived) are read. What is shown from it is worked out in `spent.rs`.

use super::*;
use crate::agents::{SessionLog, Spent};
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

/// What a Codex rollout says a session spent, as a `SessionLog`: its running total, and when the file was last written.
fn codex_log(path: &Path) -> Option<SessionLog> {
    let last_at = std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
    Some(SessionLog { spent: Agent::Codex.spent(path)?, last_at: last_at.map(|d| d.as_millis() as u64), helpers: Vec::new() })
}

/// Read what changed (off the UI thread): each session's transcript found under `root` (Claude Code's folder), else its
/// rollout under `codex_root`, read again only when its files are not as they were.
fn read_all(root: &Path, codex_root: Option<&Path>, sessions: &[String], prev: &HashMap<String, Reading>) -> HashMap<String, Reading> {
    let mut out = prev.clone();
    for id in sessions {
        let known = prev.get(id).map(|r| r.path.clone()).filter(|p| p.is_file());
        let found = known
            .map(|p| {
                let codex = codex_root.is_some_and(|c| p.starts_with(c));
                (p, codex)
            })
            .or_else(|| claude::find_transcript(root, id).map(|p| (p, false)))
            .or_else(|| codex_root.and_then(|c| Agent::Codex.find_transcript(c, id)).map(|p| (p, true)));
        let Some((path, codex)) = found else {
            out.remove(id);
            continue;
        };
        let stamp = stamp_of(&path);
        if prev.get(id).is_some_and(|r| r.stamp == stamp) {
            continue;
        }
        let log = if codex { codex_log(&path) } else { claude::read_session_log(&path) };
        out.insert(id.clone(), Reading { log, path, stamp });
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
        // A task of a program switched off is still on the board: what it spent is read all the same.
        let codex_root = self.installed.codex_root().map(Path::to_path_buf);
        let prev = self.sw.logs.clone();
        let (tx, rx) = channel();
        thread::spawn(move || {
            let _ = tx.send(read_all(&root, codex_root.as_deref(), &sessions, &prev));
        });
        self.sw.rx = Some(rx);
    }

    /// The sessions whose transcripts are read: those of every task on the list showing (Active or Archived).
    fn sessions_to_read(&self) -> Vec<String> {
        self.all_tasks()
            .iter()
            .flat_map(|(p, list)| list.iter().map(move |t| (p, t)))
            .filter(|(p, t)| self.board_shelf_of(p, t) == Some(self.bd.shelf))
            .filter_map(|(_, t)| t.session_id().map(String::from))
            .collect()
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
