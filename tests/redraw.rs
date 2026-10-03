//! Drawing only when something changed, end to end in a real terminal with the real binary: an idle list writes
//! nothing at all, a working conversation's spinner still turns, and a session's live view still shows what its
//! program writes when nobody types. A `sleep` stands in for a running claude, a small script for `claude` itself,
//! and every directory is a temp dir; the test kills only the processes it started.

mod common;
use common::*;
use std::{
    collections::HashSet,
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

struct World {
    claude: PathBuf,
    cache: PathBuf,
    config: PathBuf,
    work: PathBuf,
    /// Stands in for a running claude, while one is wanted.
    running: Option<Child>,
}

impl World {
    fn new(name: &str) -> Self {
        let base = tmp(name);
        let (claude, cache, config, work) = (base.join("claude"), base.join("cache"), base.join("config"), base.join("work"));
        for d in [&claude, &cache, &config, &work, &claude.join("sessions"), &claude.join("projects").join("-work")] {
            fs::create_dir_all(d).unwrap();
        }
        // Conversations of a fixed day, which will be old one day: this file is about drawing, not auto-archive.
        fs::write(config.join("settings.json"), r#"{"auto_archive_days":0}"#).unwrap();
        Self { claude, cache, config, work, running: None }
    }

    /// A saved conversation in `self.work`, last active `hours_ago` hours before 23:00 on a fixed day.
    fn save(&self, id: &str, prompt: &str, hours_ago: u64) {
        let ts = format!("2026-09-19T{:02}:00:00.000Z", 23 - hours_ago);
        let line = format!(
            r#"{{"type":"user","cwd":"{}","gitBranch":"main","timestamp":"{ts}","message":{{"role":"user","content":"{prompt}"}}}}"#,
            self.work.display()
        );
        fs::write(self.claude.join("projects").join("-work").join(format!("{id}.jsonl")), line + "\n").unwrap();
    }

    /// Conversation `id` is being worked on right now, by a process outside platoon (a `sleep` stands in for it).
    fn working(&mut self, id: &str) {
        let child = Command::new("sleep").arg("60").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let registry = format!(
            r#"{{"pid":{},"sessionId":"{id}","cwd":"{}","kind":"interactive","startedAt":1,"status":"busy"}}"#,
            child.id(),
            self.work.display()
        );
        fs::write(self.claude.join("sessions").join(format!("{}.json", child.id())), registry).unwrap();
        self.running = Some(child);
    }

    /// A stand-in claude that says nothing for a moment, then writes a line by itself, then gives a shell.
    fn env(&self) -> Vec<(&'static str, String)> {
        let program = self.claude.join("fake-claude.sh");
        fs::write(&program, "#!/bin/sh\necho \"fake-claude started\"\nsleep 1\necho \"written-later-$((40+2))\"\nexec sh\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        vec![
            ("CLAUDE_CONFIG_DIR", self.claude.to_string_lossy().into()),
            ("PLATOON_CACHE_DIR", self.cache.to_string_lossy().into()),
            ("PLATOON_CONFIG_DIR", self.config.to_string_lossy().into()),
            ("PLATOON_CLAUDE_BIN", program.to_string_lossy().into()),
            ("PLATOON_SKIP_SETUP_CHECK", "1".into()),
        ]
    }

    /// The session platoon started (its host's claude, here a shell), stopped.
    fn stop_host(&self) {
        if let Some((pid, _)) = find_host(&self.cache) {
            unsafe { libc::kill(pid as i32, libc::SIGKILL) }; // an interactive sh ignores SIGTERM
            until("the stand-in to exit", || !alive(pid));
        }
    }
}

impl Drop for World {
    fn drop(&mut self) {
        if let Some(c) = self.running.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// How many bytes the program writes to the terminal over `wait`.
fn written_over(t: &mut Term, wait: Duration) -> usize {
    let before = t.raw().len();
    thread::sleep(wait);
    t.raw().len() - before
}

#[test]
fn an_idle_list_writes_nothing_and_a_working_rows_spinner_still_turns() {
    let mut w = World::new("redraw-idle");
    w.save("aaaa-0001", "an idle conversation", 5);
    w.save("bbbb-0002", "another idle one", 4);
    let mut t = Term::spawn(&[], &w.env(), &w.work, 30, 110);
    t.wait_for_all(&["an idle conversation", "another idle one", "enter open"]);
    t.stable();
    // Nothing on the screen moves: not a byte is written, though platoon still looks for changes every second.
    let n = written_over(&mut t, Duration::from_millis(2500));
    assert_eq!(n, 0, "an idle list wrote {n} bytes");
    // A new size is drawn at once, too small or not.
    t.resize(10, 50);
    t.wait_for_flat("platoon needs at least");
    t.resize(30, 110);
    t.wait_for_all(&["an idle conversation", "another idle one", "enter open"]);
    t.stable();
    // Keys are still answered at once.
    t.send(b"/");
    t.wait_for("╭ Search");
    t.send(b"\x1b");
    t.wait_gone("╭ Search");

    // One of them is being worked on: its row says so and its spinner turns.
    w.working("bbbb-0002");
    t.wait_for("working");
    let mut seen = HashSet::new();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) && seen.len() < 3 {
        if let Some(line) = t.text().lines().find(|l| l.contains("working")) {
            seen.extend(line.chars().filter(|c| SPINNER.contains(c)));
        }
        thread::sleep(Duration::from_millis(40));
    }
    assert!(seen.len() >= 3, "the spinner turns: saw {seen:?}\n{}", t.text());
    assert!(written_over(&mut t, Duration::from_millis(500)) > 0, "a spinner is drawn as it turns");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn a_sessions_live_view_shows_what_its_program_writes_by_itself() {
    let w = World::new("redraw-attached");
    w.save("cccc-0003", "resume me", 3);
    let mut t = Term::spawn(&[], &w.env(), &w.work, 30, 110);
    t.wait_for_all(&["resume me", "enter open"]);
    t.send(b"\r");
    t.wait_for("fake-claude started");
    // Nothing is typed from here on: the line the program writes a second later still shows.
    t.wait_for("written-later-42");
    t.send(&[0]); // ctrl+space: back to the list
    t.wait_for("enter open");
    t.send(b"q");
    t.wait_for("running: platoon picks");
    t.wait_exit();
    w.stop_host();
}
