//! Archiving a conversation something is running, end to end in a real terminal with the real binary: the row must
//! leave at once while the process takes its time to shut down, the archive must be written only after it has
//! exited, and quitting must neither hang nor archive a conversation whose process still runs.
//!
//! The "claude" here is a shell script the test starts itself (it is registered the way Claude registers a session
//! but is not a real one), so the only process ever signaled is one of ours. Every directory is a temp dir.

mod common;
use common::*;
use std::{
    fs,
    path::PathBuf,
    process::{Child, Command},
    time::{Duration, Instant},
};

struct Lab {
    base: PathBuf,
    claude: PathBuf,
    cache: PathBuf,
    config: PathBuf,
    work: PathBuf,
    procs: Vec<Child>,
}

impl Lab {
    fn new(name: &str) -> Self {
        let base = tmp(name);
        let lab = Self {
            base: base.clone(),
            claude: base.join("claude"),
            cache: base.join("cache"),
            config: base.join("config"),
            work: base.join("work"),
            procs: Vec::new(),
        };
        for d in [&lab.claude.join("sessions"), &lab.claude.join("projects/-work"), &lab.cache, &lab.config, &lab.work] {
            fs::create_dir_all(d).unwrap();
        }
        // The transcripts carry fixed dates that will be old one day: this is not about auto-archive.
        fs::write(lab.config.join("settings.json"), r#"{"auto_archive_days":0}"#).unwrap();
        lab
    }

    fn env(&self) -> Vec<(&'static str, String)> {
        vec![
            ("CLAUDE_CONFIG_DIR", self.claude.to_string_lossy().into()),
            ("PLATOON_CACHE_DIR", self.cache.to_string_lossy().into()),
            ("PLATOON_CONFIG_DIR", self.config.to_string_lossy().into()),
            ("PLATOON_CLAUDE_BIN", "/nonexistent/claude-is-never-started-here".into()),
            ("PLATOON_SKIP_SETUP_CHECK", "1".into()),
        ]
    }

    fn save(&self, id: &str, prompt: &str, hour: u64) {
        let line = format!(
            r#"{{"type":"user","cwd":"{}","gitBranch":"main","timestamp":"2026-09-19T{hour:02}:00:00.000Z","message":{{"role":"user","content":"{prompt}"}}}}"#,
            self.work.display()
        );
        fs::write(self.claude.join("projects/-work").join(format!("{id}.jsonl")), line + "\n").unwrap();
    }

    /// A process of ours that, told to stop, takes `secs` seconds to end (`None`: ignores it), registered as running `id`.
    fn run(&mut self, id: &str, secs: Option<f32>) -> u32 {
        let ready = self.cache.join(format!("ready-{id}"));
        let trap = secs.map_or(String::new(), |s| format!("sleep {s}; exit 0"));
        let script = format!("trap '{trap}' TERM; : > '{}'; while :; do sleep 0.05; done", ready.display());
        let child = Command::new("sh").arg("-c").arg(script).spawn().unwrap();
        let pid = child.id();
        self.procs.push(child);
        until("the stand-in to be ready", || ready.exists());
        let reg =
            serde_json::json!({"pid": pid, "sessionId": id, "cwd": self.work, "kind": "interactive", "startedAt": 1, "status": "idle"});
        fs::write(self.claude.join("sessions").join(format!("{pid}.json")), reg.to_string()).unwrap();
        pid
    }

    /// Has our own process (by pid) exited? (`try_wait` also reaps it: a zombie still answers signal 0.)
    fn exited(&mut self, pid: u32) -> bool {
        self.procs.iter_mut().find(|c| c.id() == pid).is_none_or(|c| c.try_wait().unwrap().is_some())
    }

    fn archived(&self) -> Vec<String> {
        let Ok(text) = fs::read_to_string(self.config.join("archive.json")) else { return vec![] };
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v["archived"].as_object().unwrap().keys().cloned().collect()
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        for c in &mut self.procs {
            let _ = c.kill(); // our own children, by handle
            let _ = c.wait();
        }
        let _ = fs::remove_dir_all(&self.base);
    }
}

#[test]
fn the_row_leaves_at_once_while_claude_shuts_down_and_the_archive_follows_its_exit() {
    let mut lab = Lab::new("stop-live");
    lab.save("aaaa-0001", "refactor the widget", 5); // the newest, so the selected row
    lab.save("bbbb-0002", "write the release notes", 2);
    let pid = lab.run("aaaa-0001", Some(1.5));
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 110);
    t.wait_for("refactor the widget");
    t.wait_for_flat("Active 2");

    // `a` on the row whose process is not one platoon started asks first; the answer is what starts the stop.
    t.send(b"a");
    t.wait_for("y archive");
    assert!(!lab.exited(pid) && lab.archived().is_empty(), "asking changes nothing");
    let asked = Instant::now();
    t.send(b"y");
    t.wait_gone("refactor the widget");
    let gone_after = asked.elapsed();
    assert!(gone_after < Duration::from_millis(1000), "the row took {gone_after:?} to leave (the process needs 1.5 s)");
    assert!(!lab.exited(pid), "the process is still shutting down when the row is already gone");
    assert!(lab.archived().is_empty(), "not archived while it still runs");
    t.wait_for_flat("Active 1");

    until("the archive record after the process exited", || !lab.archived().is_empty());
    assert!(lab.exited(pid), "the record was written only once the process had exited");
    assert_eq!(lab.archived(), ["aaaa-0001"]);
    t.wait_for_flat("Archived 1");
    t.wait_for("Archived \"");

    t.send(b"\x1b[C"); // right arrow: Archived
    t.wait_for("refactor the widget");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn quitting_right_after_archiving_waits_for_the_stop_so_the_record_is_written() {
    let mut lab = Lab::new("stop-quit");
    lab.save("aaaa-0001", "refactor the widget", 5);
    let pid = lab.run("aaaa-0001", Some(1.0));
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 110);
    t.wait_for("refactor the widget");
    t.send(b"a");
    t.wait_for("y archive");
    t.send(b"y");
    t.wait_gone("refactor the widget");
    t.send(b"q");
    let quit_at = Instant::now();
    t.wait_exit();
    assert!(quit_at.elapsed() < Duration::from_secs(4), "quitting took {:?}", quit_at.elapsed());
    assert!(lab.exited(pid) && lab.archived() == ["aaaa-0001"], "the process is gone and the record written before platoon left");
}

#[test]
fn quitting_with_a_stop_that_is_not_finishing_leaves_within_a_few_seconds_and_archives_nothing() {
    let mut lab = Lab::new("stop-quit-deaf");
    lab.save("aaaa-0001", "refactor the widget", 5);
    let pid = lab.run("aaaa-0001", None);
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 110);
    t.wait_for("refactor the widget");
    t.send(b"a");
    t.wait_for("y archive");
    t.send(b"y");
    t.wait_gone("refactor the widget");
    t.send(b"q");
    let quit_at = Instant::now();
    t.wait_exit();
    let took = quit_at.elapsed();
    assert!(took < Duration::from_secs(5), "quitting took {took:?}: it must not wait for the full stop time");
    assert!(!lab.exited(pid), "our stand-in ignores SIGTERM");
    assert!(lab.archived().is_empty(), "never archived while its process still runs");
}
