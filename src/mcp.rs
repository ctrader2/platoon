//! What runs under a session after an idle stop: its MCP servers and other helpers that may not notice the agent has
//! gone.
//!
//! Before an idle stop the host records everything under the agent that is not a command the agent started (`idle::under`:
//! the MCP servers, whatever launched them, and their own children), with what proves it is the same process later
//! (`Tracked`). Once the agent has exited, those still running get a moment to leave by themselves and are then asked to
//! (SIGTERM), each only if it is still the process that was recorded. Nothing else is ever signalled.

use crate::procs;
use std::time::{Duration, Instant};

/// A process to remember: enough to be sure later that a pid is still this same process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tracked {
    pub pid: u32,
    pub start: u64,
    pub argv: Option<Vec<String>>,
}

/// SIGTERM `t` if that pid is still the process that was recorded: alive, and started at the same time (and
/// with the same command line, where that was recorded). Anything else, such as a pid reused since, is left
/// alone. True if a signal was sent.
pub fn terminate_if_same(t: &Tracked) -> bool {
    if t.pid <= 1 || t.start == 0 {
        return false; // a start time is the only proof of identity
    }
    let Some(now) = procs::table().and_then(|rows| rows.into_iter().find(|r| r.pid == t.pid)) else { return false };
    if now.zombie || now.start != t.start {
        return false;
    }
    if let (Some(before), Some(after)) = (&t.argv, procs::cmdline(t.pid))
        && *before != after
    {
        return false;
    }
    unsafe { libc::kill(t.pid as i32, libc::SIGTERM) == 0 }
}

/// Wait up to `wait` for `claude` to exit, and only then clean up after it (see `end_leftovers`). While it is
/// still running its MCP servers are in use, so if it never goes nothing is signalled.
pub fn end_leftovers_after(claude: u32, tracked: &[Tracked], wait: Duration, grace: Duration) {
    let started = Instant::now();
    let running = || procs::table().is_none_or(|rows| rows.iter().any(|r| r.pid == claude && !r.zombie));
    while running() {
        if started.elapsed() >= wait {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    end_leftovers(tracked, grace);
}

/// Give leftovers `grace` to exit on their own, then SIGTERM each one that is still the same process.
pub fn end_leftovers(tracked: &[Tracked], grace: Duration) {
    let started = Instant::now();
    let any_running = || {
        procs::table().is_some_and(|rows| tracked.iter().any(|t| rows.iter().any(|r| r.pid == t.pid && !r.zombie && r.start == t.start)))
    };
    while started.elapsed() < grace && any_running() {
        std::thread::sleep(Duration::from_millis(50));
    }
    for t in tracked {
        terminate_if_same(t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command};

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // --- signalling leftovers ------------------------------------------------------------------------------
    //
    // Every wait here is for something to happen, with a deadline far longer than it ever takes, so a loaded
    // machine is slow but never fails; only "nothing happened" (`still_running`) has to be a fixed pause.

    /// Longer than anything here takes even on a loaded machine, so only a real failure reaches it.
    const PATIENCE: Duration = Duration::from_secs(60);

    /// A `sleep` of the test's own, killed and reaped when it goes out of scope (a failing test leaves none behind).
    struct Kid(Child);

    impl Kid {
        fn id(&self) -> u32 {
            self.0.id()
        }
    }

    impl Drop for Kid {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// Long enough never to end by itself while a test runs, however slow the machine.
    fn spawn(secs: &str) -> Kid {
        Kid(Command::new("sleep").arg(secs).spawn().unwrap())
    }

    /// What `Tracked` records for `pid`. Its command line is waited for: right after a spawn returns, the new
    /// program's arguments can still be unset for a moment, which reads as no command line at all.
    fn tracked(pid: u32) -> Tracked {
        let start = procs::table().unwrap().into_iter().find(|r| r.pid == pid).unwrap().start;
        let since = Instant::now();
        let argv = loop {
            match procs::cmdline(pid) {
                Some(a) => break a,
                None => assert!(since.elapsed() < PATIENCE, "pid {pid} never showed its command line"),
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        Tracked { pid, start, argv: Some(argv) }
    }

    fn wait_gone(c: &mut Kid) -> bool {
        let since = Instant::now();
        while since.elapsed() < PATIENCE {
            if c.0.try_wait().unwrap().is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// Still there after a pause: what "not signalled" looks like (it would have ended within the pause).
    fn still_running(c: &mut Kid) -> bool {
        std::thread::sleep(Duration::from_millis(300));
        c.0.try_wait().unwrap().is_none()
    }

    #[test]
    fn only_the_recorded_process_is_signalled() {
        let (mut mine, mut bystander) = (spawn("641"), spawn("641"));
        let t = tracked(mine.id());
        assert!(terminate_if_same(&t));
        assert!(wait_gone(&mut mine));
        assert!(still_running(&mut bystander), "same command line, different process: untouched");
        // The process is gone: signalling it again does nothing (its pid could be anybody's by now).
        assert!(!terminate_if_same(&t));
    }

    #[test]
    fn a_pid_that_now_belongs_to_another_process_is_left_alone() {
        let mut other = spawn("642");
        let good = tracked(other.id());
        // Same pid, but a different start time: what a reused pid looks like.
        let reused = Tracked { start: good.start + 1, ..good.clone() };
        assert!(!terminate_if_same(&reused));
        // Same pid and start but not the command line that was recorded.
        let renamed = Tracked { argv: Some(strs(&["something", "else"])), ..good.clone() };
        assert!(!terminate_if_same(&renamed));
        // No start time recorded: no way to tell, so no signal. Nor to the pids nobody may signal.
        assert!(!terminate_if_same(&Tracked { start: 0, ..good.clone() }));
        for pid in [0, 1] {
            assert!(!terminate_if_same(&Tracked { pid, start: 5, argv: None }));
        }
        assert!(still_running(&mut other));
        assert!(terminate_if_same(&good), "the real one is still fine");
        assert!(wait_gone(&mut other));
    }

    #[test]
    fn leftovers_get_a_grace_period_then_only_the_recorded_ones_are_ended() {
        let (mut a, mut b, mut bystander, mut reused) = (spawn("643"), spawn("643"), spawn("643"), spawn("643"));
        let list = vec![tracked(a.id()), tracked(b.id()), Tracked { start: tracked(reused.id()).start + 7, ..tracked(reused.id()) }];
        let t = Instant::now();
        end_leftovers(&list, Duration::from_millis(300));
        assert!(t.elapsed() >= Duration::from_millis(300), "waited for them to leave by themselves first");
        assert!(wait_gone(&mut a) && wait_gone(&mut b));
        assert!(still_running(&mut bystander) && still_running(&mut reused));
    }

    #[test]
    fn nothing_is_signalled_while_claude_is_still_running_and_leftovers_are_ended_once_it_is_gone() {
        let (mut claude, mut server) = (spawn("644"), spawn("644"));
        let list = vec![tracked(server.id())];
        end_leftovers_after(claude.id(), &list, Duration::from_millis(400), Duration::from_millis(50));
        assert!(still_running(&mut server), "claude is still there, so its server is still in use");
        let id = claude.id();
        let killer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            unsafe { libc::kill(id as i32, libc::SIGKILL) };
        });
        // It returns as soon as claude is gone (a zombie counts): the long wait only matters on a loaded machine.
        end_leftovers_after(claude.id(), &list, PATIENCE, Duration::from_millis(50));
        killer.join().unwrap();
        assert!(wait_gone(&mut server), "claude exited (a zombie counts), so the server was asked to");
        let _ = claude.0.wait();
    }

    #[test]
    fn leftovers_that_leave_by_themselves_are_not_waited_for() {
        let mut gone = spawn("643");
        let list = vec![tracked(gone.id())];
        let _ = gone.0.kill();
        let _ = gone.0.wait();
        // The grace is far longer than the check may take even on a loaded machine: not waiting it out is the point.
        let t = Instant::now();
        end_leftovers(&list, 2 * PATIENCE);
        assert!(t.elapsed() < PATIENCE, "took {:?}", t.elapsed());
    }
}
