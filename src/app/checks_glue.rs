//! The checks inside the app: the platoon window that holds the squad line's lock (`hold_line_lock`, the same one
//! that starts tasks from the line, so with two windows open only one makes checks) makes each task's due checks, one
//! task at a time, off the UI thread, and writes down what came of them; failures the squad is to fix are typed into
//! its session, in one message. Which checks: the ones the brief had when the leader started (its snapshot): the
//! project's tests, an agent reading the change (from the leader's transcript), the acceptance boxes (in the result);
//! a leader from before briefs has the tests alone. The rules are in `checks.rs`; the record in `tickets.rs`.

use super::squads_glue::{paste_into, split_size};
use super::*;
use crate::brief::CheckKind;
use crate::checks::{self, Place};
use crate::tickets::{CheckRun, CheckThen, State, Ticket, Tickets};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::TryRecvError;

/// How often the tasks are looked over for a check that is due.
const EVERY: Duration = Duration::from_secs(1);

/// What a task's checks' thread sends back: each check made, with what is typed into the squad's session when it
/// failed (empty when it passed, or wasn't run), and whether platoon closed meanwhile (then nothing is recorded).
struct Made {
    runs: Vec<(CheckRun, String)>,
    cancelled: bool,
}

/// The checks running now.
struct Running {
    project: String,
    number: u32,
    /// The mark it is for (`CheckDue::at`).
    due_at: u64,
    rx: Receiver<Made>,
    /// Set when platoon closes: the test command's process group is ended, and the checks run again next time.
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

/// What a task's checks need, worked out on the UI thread from its file and its leader's snapshot.
struct Due {
    /// The test command to run, when the brief has the tests and the project a command.
    tests: Option<String>,
    /// The agent that is to read the change, by its name in the session, and the members that can change files.
    review: Option<(String, Vec<String>, String)>,
    acceptance: bool,
}

impl App {
    /// Task `number` of `project` is having its checks made by this window right now.
    pub fn check_running(&self, project: &str, number: u32) -> bool {
        self.ck.running.as_ref().is_some_and(|r| r.project == project && r.number == number)
    }

    /// Called on every refresh: take in finished checks, and look for the next task whose checks are due (in this
    /// window only while it holds the squad line's lock).
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

    /// The checks task `x` of `project`'s current leader was started with (its snapshot), each line with its kind; none
    /// for a task with no leader, or a leader from before briefs.
    pub fn leader_checks(&self, project: &str, x: &Ticket) -> Vec<(String, CheckKind)> {
        x.leader()
            .and_then(|l| crate::tickets::snapshot_text(&self.config, project, l))
            .and_then(|text| crate::squads::Launch::from_json(&text))
            .filter(|l| l.checks.is_some())
            .map(|l| l.check_kinds())
            .unwrap_or_default()
    }

    /// The checks the brief of task `t`'s current leader asks for (its snapshot), and what each needs. A leader from
    /// before briefs: the tests alone.
    fn due_checks(&self, store: &Tickets, project: &str, t: &Ticket) -> Due {
        let launch = t.leader().and_then(|l| store.snapshot(l)).and_then(|text| crate::squads::Launch::from_json(&text));
        let kinds: Vec<CheckKind> =
            launch.as_ref().map_or_else(|| vec![CheckKind::Tests], |l| l.check_kinds().into_iter().map(|(_, k)| k).collect());
        let command = self.projects.get(project).map(|p| p.test_command.clone()).unwrap_or_default();
        let review = kinds.iter().find_map(|k| match k {
            CheckKind::Read(a) => launch.as_ref().map(|l| {
                let name = l.member_name(a).unwrap_or_else(|| format!("{}-{a}", l.squad));
                (name, l.changers(), l.squad.clone())
            }),
            _ => None,
        });
        Due {
            tests: (kinds.contains(&CheckKind::Tests) && !command.is_empty()).then_some(command),
            review,
            acceptance: kinds.contains(&CheckKind::Acceptance),
        }
    }

    /// Start task `number`'s due checks, read from its file as it is now. False when there was nothing to start: the
    /// task moved on (the user wrote to it, it was cancelled) or there is nothing to check any more, and the mark goes.
    fn start_check(&mut self, project: &str, number: u32) -> bool {
        let mut store = Tickets::open(&self.config, project);
        let Some(t) = store.get(number).cloned() else { return false };
        let Some(due) = t.check_due.clone() else { return false };
        let what = self.due_checks(&store, project, &t);
        if !matches!(t.state, State::Waiting | State::Done) || (what.tests.is_none() && what.review.is_none() && !what.acceptance) {
            let _ = store.drop_check_due(number, due.at);
            self.reread_tickets();
            return false;
        }
        let timeout =
            Duration::from_secs(self.projects.get(project).map_or(crate::projects::TEST_TIMEOUT_MINUTES * 60, |p| p.test_timeout_secs));
        let test_try = checks::next_try(&t.checks);
        let review_try = checks::next_try_of(&t.checks, checks::REVIEW);
        let acceptance_try = checks::next_try_of(&t.checks, checks::ACCEPTANCE);
        let log = crate::tickets::work_dir(&self.config, project, number).join(format!("tests-{test_try}.log"));
        let others = self.other_sessions_dirs(&t);
        let git = self.git_program.clone();
        let env = checks::env();
        let body = store.body(&t);
        let result = store.result(number);
        let root = self.reader.root().to_path_buf();
        let session = t.leader().map(|l| l.session.clone());
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let flag = stop.clone();
        thread::spawn(move || {
            let mut runs: Vec<(CheckRun, String)> = Vec::new();
            let made = |kind: &str, attempt: u32, j: checks::Judged| {
                let run = CheckRun { kind: kind.into(), ok: j.ok, summary: j.summary, at: usage::now(), attempt, ..CheckRun::default() };
                (run, j.message)
            };
            if let Some((reviewer, changers, squad)) = &what.review {
                let found =
                    session.as_deref().and_then(|s| crate::claude::find_transcript(&root, s)).and_then(|p| crate::claude::read_leader(&p));
                let member_runs = found.map(|l| l.runs).unwrap_or_default();
                runs.push(made(checks::REVIEW, review_try, checks::review(&member_runs, reviewer, changers, squad)));
            }
            if what.acceptance {
                runs.push(made(checks::ACCEPTANCE, acceptance_try, checks::acceptance(&body, result.as_deref())));
            }
            let mut cancelled = false;
            if let Some(cmd) = &what.tests {
                let deadline = Instant::now() + crate::gitfacts::BUDGET;
                let tested = match checks::place(&t, &|d| crate::gitfacts::current_branch(&git, d, deadline)) {
                    // No folder to run them in (any more): nothing to record.
                    None => None,
                    Some(Place::Skip { folder, why }) => Some((folder, None, Some(why))),
                    Some(Place::Run { folder, root }) if checks::open_there(&root, &others) => {
                        Some((folder, None, Some(checks::SESSION_OPEN.to_string())))
                    }
                    Some(Place::Run { folder, .. }) => {
                        let ran = checks::run(cmd, &folder, &env, timeout, &log, &flag);
                        cancelled = ran.cancelled;
                        Some((folder, Some(ran), None))
                    }
                };
                if let Some((folder, ran, why)) = tested.filter(|_| !cancelled) {
                    let folder_text = folder.to_string_lossy().into_owned();
                    let message = ran.as_ref().filter(|r| !r.ok).map(|r| checks::typed_message(cmd, &folder_text, r)).unwrap_or_default();
                    let run = CheckRun {
                        kind: checks::TESTS.to_string(),
                        ok: ran.as_ref().is_some_and(|x| x.ok),
                        summary: why.clone().or_else(|| ran.as_ref().map(|x| x.summary.clone())).unwrap_or_default(),
                        command: cmd.clone(),
                        at: usage::now(),
                        folder: folder_text,
                        attempt: test_try,
                        skipped: why.is_some(),
                    };
                    runs.push((run, message));
                }
            }
            let _ = tx.send(Made { runs, cancelled });
        });
        self.ck.running = Some(Running { project: project.to_string(), number, due_at: due.at, rx, stop });
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

    /// A task's checks are over: what came of them is written on the task, and the failures the squad is to fix are
    /// typed into its session, in one message. Said in one line.
    fn take_check(&mut self) {
        let Some(r) = &self.ck.running else { return };
        let made = match r.rx.try_recv() {
            Ok(made) => made,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.ck.running = None;
                return;
            }
        };
        let Some(r) = self.ck.running.take() else { return };
        if made.cancelled {
            return;
        }
        let mut store = Tickets::open(&self.config, &r.project);
        let n = r.number;
        if made.runs.is_empty() {
            let _ = store.drop_check_due(n, r.due_at);
            self.reread_tickets();
            return;
        }
        let runs: Vec<CheckRun> = made.runs.iter().map(|(run, _)| run.clone()).collect();
        let sock = store.get(n).and_then(|t| t.leader()).and_then(|l| self.leader_host(&l.session)).map(|h| h.sock.clone());
        match store.record_checks(n, r.due_at, &runs, sock.is_some()) {
            Ok(rec) => {
                if rec.then == CheckThen::Fix
                    && let Some(sock) = sock
                {
                    let text: Vec<String> =
                        made.runs.iter().filter(|(run, m)| rec.fix.contains(&run.kind) && !m.is_empty()).map(|(_, m)| m.clone()).collect();
                    let text = text.join("\n\n");
                    let (rows, cols) = split_size(self.size);
                    thread::spawn(move || paste_into(&sock, rows, cols, &text));
                }
                self.notify(said(n, &rec.then, &rec.fix, &runs));
            }
            Err(e) => {
                let why = e.to_string();
                self.notify(format!("Couldn't write #{n}'s check results: {}", why.split(" (os error").next().unwrap_or(&why)));
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

/// What the status line says once a task's checks are made: the tests' result as before, the others' when they fail.
fn said(n: u32, then: &CheckThen, fix: &[String], runs: &[CheckRun]) -> String {
    let tests = runs.iter().find(|r| r.kind == checks::TESTS);
    let failed: Vec<&CheckRun> = runs.iter().filter(|r| !r.ok && !r.skipped).collect();
    match then {
        CheckThen::Fix if fix.len() == 1 && fix[0] == checks::TESTS => {
            let t = failed.iter().find(|r| r.kind == checks::TESTS).copied().cloned().unwrap_or_default();
            format!(
                "#{n}'s tests failed ({}): platoon told its squad, which is fixing them (try {} of {}).",
                t.summary,
                t.attempt,
                checks::FIXES
            )
        }
        CheckThen::Fix => {
            let parts: Vec<String> = failed
                .iter()
                .filter(|r| fix.contains(&r.kind))
                .map(|r| match r.kind.as_str() {
                    checks::TESTS => format!("the tests failed ({}; try {} of {})", r.summary, r.attempt, checks::FIXES),
                    checks::REVIEW => format!("the change wasn't read ({})", r.summary),
                    _ => format!("not every acceptance box is ticked ({})", r.summary),
                })
                .collect();
            format!("#{n}: {}. platoon told its squad, which is on it.", parts.join("; "))
        }
        CheckThen::NeedsYou(why) => {
            let what = failed.first().map_or(String::new(), |r| format!(" ({})", r.summary));
            format!("#{n} needs you: {why}{what}.")
        }
        CheckThen::Keep => match (tests, failed.first()) {
            (_, Some(f)) if f.kind != checks::TESTS => format!("#{n}'s check failed: {}.", f.summary),
            (Some(t), _) if t.skipped => format!("#{n}'s tests weren't run: {}.", t.summary),
            (Some(t), _) if t.ok => format!("#{n}'s tests passed: {}.", t.summary),
            (Some(t), _) => format!("#{n}'s tests failed: {}.", t.summary),
            (None, Some(f)) => format!("#{n}'s tests failed: {}.", f.summary),
            (None, None) => format!("#{n}'s checks passed."),
        },
    }
}
