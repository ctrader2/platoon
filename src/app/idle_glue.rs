//! Idle stop in the app: sessions whose host an older platoon started, and why a quiet session is kept.
//!
//! A host keeps the idle-stop rules of the platoon that started it, and hosts run for days, so a better rule would
//! never reach the sessions running when platoon is updated. A host's file names its version (`HostInfo::version`); for
//! each host of an older platoon (or one from before hosts said), the app applies the current rules from outside
//! (`idle::check_older`) at most once a minute, off the UI thread, and stops the sessions they say to stop. Hosts of
//! this version stop their own sessions, as before.
//!
//! A running session's row says why it is kept once it has been quiet for the idle-stop time (`kept_of`): a host of
//! this version says so in its file (`HostInfo::kept`), and for an older one the app's own look says it.

use super::*;
use crate::idle::{self, Kept, OldHost, Rules};
use std::sync::mpsc::TryRecvError;

/// How often the older hosts are looked at.
const EVERY: Duration = Duration::from_secs(60);
/// How long a shell under the agent must have run to count as a command it started (see `idle`).
const SETTLE: Duration = Duration::from_secs(30);

/// What one look at the older hosts found: each one's agent pid, and why it is kept (None: stopped, or nothing to say).
type Found = Vec<(u32, Option<Kept>)>;

pub struct IdleState {
    /// Why each older host's quiet session is kept, by its agent's pid, as the last look found.
    kept: HashMap<u32, Kept>,
    rx: Option<Receiver<Found>>,
    last: Option<Instant>,
    /// `SETTLE`, which a unit test shortens.
    settle: Duration,
}

impl Default for IdleState {
    fn default() -> Self {
        Self { kept: HashMap::new(), rx: None, last: None, settle: SETTLE }
    }
}

impl App {
    /// Take in the last look at the older hosts, and start another once a minute while there are any and idle stop is
    /// on. Called on every refresh; nothing is read or signalled on the UI thread.
    pub(super) fn poll_older_hosts(&mut self) {
        if let Some(rx) = &self.idle.rx {
            match rx.try_recv() {
                Ok(found) => {
                    self.idle.kept = found.into_iter().filter_map(|(pid, k)| Some((pid, k?))).collect();
                    self.idle.rx = None;
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => self.idle.rx = None,
            }
        }
        let Some(threshold) = self.settings.get().idle_stop_after(60) else {
            self.idle.kept.clear();
            return;
        };
        if self.idle.last.is_some_and(|t| t.elapsed() < EVERY) {
            return;
        }
        let old: Vec<OldHost> = self
            .hosts
            .iter()
            .filter(|h| idle::older(h.version.as_deref()))
            .map(|h| OldHost { info: h.clone(), session: h.session().or_else(|| self.session_run_by(h.claude_pid)) })
            .collect();
        if old.is_empty() {
            self.idle.kept.clear();
            return;
        }
        self.idle.last = Some(Instant::now());
        let rules = Rules {
            threshold,
            settle: self.idle.settle,
            claude_root: self.reader.root().to_path_buf(),
            codex_root: self.providers.codex_root().map(Path::to_path_buf),
            config: self.config.clone(),
            attached: self.attached_pid(),
        };
        let (tx, rx) = channel();
        self.idle.rx = Some(rx);
        thread::spawn(move || {
            let _ = tx.send(idle::check_older(&old, &rules));
        });
    }

    /// The conversation process `pid` runs, as the rows know it.
    fn session_run_by(&self, pid: u32) -> Option<String> {
        self.sessions.iter().find(|s| !s.unsaved && s.procs.iter().any(|p| p.pid == pid)).map(|s| s.id.clone())
    }

    /// Why the running session `s` is still running although it has been quiet for the idle-stop time, in the words its
    /// row shows ("Kept running: ..."). None while idle stop is off, or there is nothing to say.
    pub fn kept_of(&self, s: &Session) -> Option<String> {
        self.settings.get().idle_stop_after(60)?;
        s.procs.iter().filter(|p| p.hosted && p.background.is_none() && !p.parked).find_map(|p| {
            let h = self.hosts.iter().find(|h| h.claude_pid == p.pid)?;
            let kept = if idle::older(h.version.as_deref()) { self.idle.kept.get(&p.pid) } else { h.kept.as_ref() };
            kept.map(|k| k.words(s.agent))
        })
    }

    /// A shell under an agent counts at once (a unit test's commands are only just started).
    #[cfg(test)]
    pub fn count_young_commands_for_test(&mut self) {
        self.idle.settle = Duration::ZERO;
    }

    /// Wait (in a unit test) until the look at the older hosts that is under way has come back.
    #[cfg(test)]
    pub fn older_hosts_looked_at_for_test(&mut self) {
        let start = Instant::now();
        while self.idle.rx.is_some() {
            assert!(start.elapsed() < Duration::from_secs(30), "the look at the older hosts never came back");
            thread::sleep(Duration::from_millis(10));
            self.poll_older_hosts();
        }
    }
}
