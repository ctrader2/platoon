//! The test check inside the app: the platoon window that holds the squad line's lock (`hold_line_lock`, the same one
//! that starts tasks from the line, so with two windows open only one runs checks) runs each task's due check, one at a
//! time, off the UI thread, and writes down what came of it; a failure is typed into the squad's session. The rules are
//! in `checks.rs`; the record in `tickets.rs`.

use super::squads_glue::{paste_into, split_size};
use super::*;
use crate::checks::{self, Place, Ran};
use crate::tickets::{CheckRun, CheckThen, State, Ticket, Tickets};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::TryRecvError;

/// How often the tasks are looked over for a check that is due.
const EVERY: Duration = Duration::from_secs(1);

/// What a check's thread sends back.
enum Outcome {
    /// The task has no folder a check can run in (any more): nothing to record.
    Nowhere,
    /// Not run, and why.
    Skipped {
        folder: PathBuf,
        why: String,
    },
    Ran {
        folder: PathBuf,
        ran: Ran,
    },
}

/// The check running now.
struct Running {
    project: String,
    number: u32,
    /// The mark it is for (`CheckDue::at`).
    due_at: u64,
    attempt: u32,
    command: String,
    rx: Receiver<Outcome>,
    /// Set when platoon closes: the command's process group is ended, and the check runs again next time.
    stop: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct CheckState {
    running: Option<Running>,
    last: Option<Instant>,
}

impl Drop for CheckState {
    /// A window that goes away (a test's) leaves no command of its own running.
    fn drop(&mut self) {
        if let Some(r) = &self.running {
            r.stop.store(true, Ordering::Relaxed);
        }
    }
}

impl App {
    /// Task `number` of `project` is having its check run by this window right now.
    pub fn check_running(&self, project: &str, number: u32) -> bool {
        self.ck.running.as_ref().is_some_and(|r| r.project == project && r.number == number)
    }

    /// Called on every refresh: take in a finished check, and look for the next one that is due (in this window only
    /// while it holds the squad line's lock).
    pub(super) fn poll_checks(&mut self) {
        self.take_check();
        if self.ck.running.is_some() || self.ck.last.is_some_and(|t| t.elapsed() < EVERY) {
            return;
        }
        self.ck.last = Some(Instant::now());
        let mut due: Vec<(u64, String, u32)> =
            self.every_ticket().iter().filter_map(|(p, t)| t.check_due.as_ref().map(|d| (d.at, p.clone(), t.number))).collect();
        if due.is_empty() || !self.hold_line_lock() {
            return;
        }
        // The one that has waited longest first.
        due.sort();
        for (_, project, number) in due {
            if self.start_check(&project, number) {
                break;
            }
        }
    }

    /// Start task `number`'s due check, read from its file as it is now. False when there was nothing to start: the
    /// task moved on (the user wrote to it, it was cancelled) or its project has no command any more, and the mark goes.
    fn start_check(&mut self, project: &str, number: u32) -> bool {
        let mut store = Tickets::open(&self.config, project);
        let Some(t) = store.get(number).cloned() else { return false };
        let Some(due) = t.check_due.clone() else { return false };
        let command = self.projects.get(project).map(|p| p.test_command.clone()).unwrap_or_default();
        if !matches!(t.state, State::Waiting | State::Done) || command.is_empty() {
            let _ = store.drop_check_due(number, due.at);
            self.reread_tickets();
            return false;
        }
        let timeout =
            Duration::from_secs(self.projects.get(project).map_or(crate::projects::TEST_TIMEOUT_MINUTES * 60, |p| p.test_timeout_secs));
        let attempt = checks::next_try(&t.checks);
        let log = crate::tickets::work_dir(&self.config, project, number).join(format!("tests-{attempt}.log"));
        let others = self.other_sessions_dirs(&t);
        let git = self.git_program.clone();
        let env = checks::env();
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let (cmd, flag) = (command.clone(), stop.clone());
        thread::spawn(move || {
            let deadline = Instant::now() + crate::gitfacts::BUDGET;
            let out = match checks::place(&t, &|d| crate::gitfacts::current_branch(&git, d, deadline)) {
                None => Outcome::Nowhere,
                Some(Place::Skip { folder, why }) => Outcome::Skipped { folder, why },
                Some(Place::Run { folder, root }) if checks::open_there(&root, &others) => {
                    Outcome::Skipped { folder, why: checks::SESSION_OPEN.to_string() }
                }
                Some(Place::Run { folder, .. }) => {
                    let ran = checks::run(&cmd, &folder, &env, timeout, &log, &flag);
                    Outcome::Ran { folder, ran }
                }
            };
            let _ = tx.send(out);
        });
        self.ck.running = Some(Running { project: project.to_string(), number, due_at: due.at, attempt, command, rx, stop });
        true
    }

    /// The folders the user's other sessions work in now: every running claude's and every platoon host's, except the
    /// sessions of task `t`'s own squads.
    fn other_sessions_dirs(&self, t: &Ticket) -> Vec<PathBuf> {
        let own: HashSet<&str> = t.leaders.iter().map(|l| l.session.as_str()).collect();
        let live = self.reader.live();
        live.iter()
            .filter(|l| !own.contains(l.id.as_str()))
            .map(|l| PathBuf::from(&l.cwd))
            .chain(self.hosts.iter().filter(|h| !h.args.iter().any(|a| own.contains(a.as_str()))).map(|h| PathBuf::from(&h.dir)))
            .filter(|d| !d.as_os_str().is_empty())
            .collect()
    }

    /// A check is over: what came of it is written on the task, and a failure the squad is to fix is typed into its
    /// session. Said in one line.
    fn take_check(&mut self) {
        let Some(r) = &self.ck.running else { return };
        let out = match r.rx.try_recv() {
            Ok(out) => out,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.ck.running = None;
                return;
            }
        };
        let Some(r) = self.ck.running.take() else { return };
        let mut store = Tickets::open(&self.config, &r.project);
        let n = r.number;
        let (folder, ran, why) = match out {
            Outcome::Nowhere => {
                let _ = store.drop_check_due(n, r.due_at);
                self.reread_tickets();
                return;
            }
            Outcome::Ran { ran, .. } if ran.cancelled => return,
            Outcome::Skipped { folder, why } => (folder, None, Some(why)),
            Outcome::Ran { folder, ran } => (folder, Some(ran), None),
        };
        let folder_text = folder.to_string_lossy().into_owned();
        let run = CheckRun {
            kind: checks::TESTS.to_string(),
            ok: ran.as_ref().is_some_and(|x| x.ok),
            summary: why.clone().or_else(|| ran.as_ref().map(|x| x.summary.clone())).unwrap_or_default(),
            command: r.command.clone(),
            at: usage::now(),
            folder: folder_text.clone(),
            attempt: r.attempt,
            skipped: why.is_some(),
        };
        let sock = store.get(n).and_then(|t| t.leader()).and_then(|l| self.leader_host(&l.session)).map(|h| h.sock.clone());
        match store.record_check(n, r.due_at, &run, sock.is_some()) {
            Ok(CheckThen::Fix) => {
                if let (Some(sock), Some(ran)) = (sock, ran.as_ref()) {
                    let text = checks::typed_message(&r.command, &folder_text, ran);
                    let (rows, cols) = split_size(self.size);
                    thread::spawn(move || paste_into(&sock, rows, cols, &text));
                }
                self.notify(format!(
                    "#{n}'s tests failed ({}): platoon told its squad, which is fixing them (try {} of {}).",
                    run.summary,
                    r.attempt,
                    checks::FIXES
                ));
            }
            Ok(CheckThen::NeedsYou(why)) => self.notify(format!("#{n} needs you: {why} ({}).", run.summary)),
            Ok(CheckThen::Keep) if run.skipped => self.notify(format!("#{n}'s tests weren't run: {}.", run.summary)),
            Ok(CheckThen::Keep) if run.ok => self.notify(format!("#{n}'s tests passed: {}.", run.summary)),
            Ok(CheckThen::Keep) => self.notify(format!("#{n}'s tests failed: {}.", run.summary)),
            Err(e) => {
                let why = e.to_string();
                self.notify(format!("Couldn't write #{n}'s test result: {}", why.split(" (os error").next().unwrap_or(&why)));
            }
        }
        self.reread_tickets();
    }

    /// platoon is closing: a check still running is stopped (its command's process group ended), and runs again the
    /// next time platoon is open, since its mark stays on the task.
    pub fn end_checks(&mut self) {
        if let Some(r) = self.ck.running.take() {
            r.stop.store(true, Ordering::Relaxed);
            let _ = r.rx.recv_timeout(Duration::from_secs(4));
        }
    }
}
