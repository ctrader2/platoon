//! Idle stop at the app's level: sessions whose host an older platoon started are stopped by the app when the current
//! rules say so, and a quiet session that is kept says why on its row. Every process here is one the test started (a
//! `sleep`, or a shell standing in for claude), every host file is in the world's own cache folder, and the hosts'
//! "host" is the test itself (the stand-ins' parent), so the app's check that a session is its host's child holds only
//! for the test's own children.

use crate::app::App;
use crate::testkit::{World, now_ms};
use ratatui::{Terminal, backend::TestBackend};
use serde_json::json;
use std::{
    fs,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// A stand-in claude of the test's own, killed and reaped when it goes out of scope.
struct Stand(Child);

impl Stand {
    fn sleep() -> Self {
        Stand(Command::new("sleep").arg("600").spawn().unwrap())
    }

    /// A claude that started a command in a shell of its own (as Claude Code's Bash tool does) and left it running.
    fn with_command() -> Self {
        let s = Stand(Command::new("sh").args(["-c", "sh -c 'sleep 600; true' & read x"]).stdin(Stdio::piped()).spawn().unwrap());
        let start = Instant::now();
        while crate::procs::live_descendants(s.id()).is_none_or(|k| k.len() < 2) {
            assert!(start.elapsed() < Duration::from_secs(30), "the stand-in never started its command");
            thread::sleep(Duration::from_millis(20));
        }
        s
    }

    fn id(&self) -> u32 {
        self.0.id()
    }

    /// Ended by a signal (SIGTERM), waited for generously.
    fn stopped(&mut self) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(30) {
            if self.0.try_wait().unwrap().is_some() {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// Still running after a pause.
    fn running(&mut self) -> bool {
        thread::sleep(Duration::from_millis(300));
        self.0.try_wait().unwrap().is_none()
    }
}

impl Drop for Stand {
    fn drop(&mut self) {
        for pid in crate::procs::live_descendants(self.0.id()).unwrap_or_default() {
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A host of the test's: its file in the world's own cache folder (`version` None: from before hosts named it), the
/// session's conversation, and Claude Code's registry entry saying it has been idle for `idle_mins`.
fn host(w: &World, pid: u32, id: &str, version: Option<&str>, idle_mins: u64, kept: Option<serde_json::Value>) {
    let hosts = w.cache.join("hosts");
    fs::create_dir_all(&hosts).unwrap();
    let mut v = json!({"agent": "claude", "claude_pid": pid, "host_pid": std::process::id(), "dir": "/tmp", "args": ["--resume", id]});
    if let Some(version) = version {
        v["version"] = json!(version);
    }
    if let Some(k) = kept {
        v["kept"] = k;
    }
    fs::write(hosts.join(format!("{pid}.json")), v.to_string()).unwrap();
    w.transcript(id, "/tmp", &format!("the work of {id}"), 600);
    let entry = json!({"pid": pid, "sessionId": id, "cwd": "/tmp", "kind": "interactive", "startedAt": 1, "status": "idle",
        "statusUpdatedAt": now_ms() - idle_mins * 60_000});
    fs::write(w.root.join("sessions").join(format!("{pid}.json")), entry.to_string()).unwrap();
}

fn screen(app: &App) -> String {
    let mut term = Terminal::new(TestBackend::new(160, 30)).unwrap();
    term.draw(|f| crate::ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn select(app: &mut App, id: &str) {
    app.selected = app.visible().iter().position(|&i| app.sessions[i].id == id).expect("row is listed");
}

#[test]
fn an_older_hosts_session_that_meets_the_rules_is_stopped_by_the_app_and_the_others_are_left_alone() {
    let w = World::new("idle-older");
    let (mut quiet, mut recent, mut current, mut busy) = (Stand::sleep(), Stand::sleep(), Stand::sleep(), Stand::with_command());
    host(&w, quiet.id(), "aaaa0001-old-quiet", None, 180, None);
    host(&w, recent.id(), "aaaa0002-old-recent", None, 30, None);
    host(&w, current.id(), "aaaa0003-current", Some(crate::versions::running()), 180, None);
    host(&w, busy.id(), "aaaa0004-old-busy", Some("0.9.0"), 180, None);
    let mut bystander = Stand::sleep(); // registered as idle for long, but no host of the app's runs it
    let entry = json!({"pid": bystander.id(), "sessionId": "aaaa0005-x", "kind": "interactive", "status": "idle", "statusUpdatedAt": 1});
    fs::write(w.root.join("sessions").join(format!("{}.json", bystander.id())), entry.to_string()).unwrap();

    let mut app = w.app();
    app.count_young_commands_for_test();
    app.refresh();
    app.older_hosts_looked_at_for_test();

    assert!(quiet.stopped(), "an older host's session idle for 3 hours with nothing running under it is stopped");
    assert!(recent.running(), "idle for half an hour: not yet");
    assert!(current.running(), "a host of this version stops its own sessions");
    assert!(busy.running(), "a command claude started is still going");
    assert!(bystander.running(), "nothing outside the app's hosts is ever touched");

    // The kept one says why on its row, in plain words; the one not idle for long enough says nothing.
    app.refresh();
    select(&mut app, "aaaa0004-old-busy");
    let s = screen(&app);
    assert!(s.contains("Kept running: a command Claude started is still going (sleep 600)"), "{s}");
    select(&mut app, "aaaa0002-old-recent");
    assert!(!screen(&app).contains("Kept running"));
}

#[test]
fn a_hosts_reason_for_keeping_a_quiet_session_shows_on_its_picked_row_while_idle_stop_is_on() {
    let w = World::new("idle-kept");
    let current = Stand::sleep();
    let kept = crate::idle::Kept::Command(Some("npm run dev".into())).to_json();
    host(&w, current.id(), "bbbb0001-kept", Some(crate::versions::running()), 0, Some(kept));
    w.transcript("bbbb0002-other", "/tmp", "another conversation", 5);
    let mut app = w.app();
    app.refresh();
    select(&mut app, "bbbb0001-kept");
    let s = screen(&app);
    assert!(s.contains("Kept running: a command Claude started is still going (npm run dev)"), "{s}");
    // Only the picked row says it.
    select(&mut app, "bbbb0002-other");
    assert!(!screen(&app).contains("Kept running"));
    // Idle stop switched off: there is nothing to explain.
    select(&mut app, "bbbb0001-kept");
    w.settings(0, 0);
    app.refresh();
    assert!(!screen(&app).contains("Kept running"));
}
