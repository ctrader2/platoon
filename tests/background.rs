//! Claude's own background sessions (`claude --bg`, `claude agents`), end to end with the real binary in a real
//! terminal. A `sleep` process stands in for the process Claude's service runs, a small fake script stands in for
//! `claude` and records how it was started, and every directory is a temp dir. What a real `claude attach` does
//! cannot be tried here (a real claude stops at its trust prompt).

mod common;
use common::*;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
};

const BG_ID: &str = "dffac639-c805-47ba-b57c-3767f4f768d8";

struct World {
    claude: PathBuf,
    cache: PathBuf,
    config: PathBuf,
    work: PathBuf,
    argv_log: PathBuf,
    /// Stands in for Claude's service's process running the background session.
    service: Child,
}

impl World {
    fn new(name: &str) -> Self {
        let base = tmp(name);
        let (claude, cache, config, work) = (base.join("claude"), base.join("cache"), base.join("config"), base.join("work"));
        for d in [&claude, &cache, &config, &work, &claude.join("sessions"), &claude.join("projects").join("-work")] {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(config.join("settings.json"), r#"{"auto_archive_days":0}"#).unwrap();
        // Not the test runner's stdout: a lingering child holding it would keep the runner's pipe open.
        let service = Command::new("sleep").arg("120").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let w = Self { claude, cache, config, work, argv_log: base.join("argv.log"), service };
        let registry = format!(
            r#"{{"pid":{},"sessionId":"{BG_ID}","cwd":"{}","kind":"bg","startedAt":1,"status":"busy","jobId":"dffac639","name":"A title","nameSource":"auto"}}"#,
            w.service.id(),
            w.work.display()
        );
        fs::write(w.claude.join("sessions").join(format!("{}.json", w.service.id())), registry).unwrap();
        // The conversation itself: last active recently in this fixed day's terms, newer than the plain one below.
        w.save(BG_ID, "continue the release work", 1);
        w.save("beef-0001-aaaa", "an ordinary idle conversation", 5);
        w
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

    /// A stand-in claude: writes its arguments to a log, says how it was started, then gives a shell.
    fn fake_claude(&self) -> PathBuf {
        let p = self.claude.join("fake-claude.sh");
        fs::write(&p, format!("#!/bin/sh\necho \"$*\" >> \"{}\"\necho \"fake-claude args:$* cwd:$(pwd)\"\nexec sh\n", self.argv_log.display())).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn env(&self) -> Vec<(&'static str, String)> {
        vec![
            ("CLAUDE_CONFIG_DIR", self.claude.to_string_lossy().into()),
            ("PLATOON_CACHE_DIR", self.cache.to_string_lossy().into()),
            ("PLATOON_CONFIG_DIR", self.config.to_string_lossy().into()),
            ("PLATOON_CLAUDE_BIN", self.fake_claude().to_string_lossy().into()),
            ("PLATOON_SKIP_SETUP_CHECK", "1".into()),
            ("NO_COLOR", String::new()), // the helper sets it, which makes crossterm draw without any color
        ]
    }

    fn launches(&self) -> Vec<String> {
        fs::read_to_string(&self.argv_log).unwrap_or_default().lines().map(String::from).collect()
    }

    fn hosts(&self) -> usize {
        fs::read_dir(self.cache.join("hosts")).map(|d| d.flatten().filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json")).count()).unwrap_or(0)
    }

    fn service_alive(&mut self) -> bool {
        matches!(self.service.try_wait(), Ok(None))
    }

    fn cleanup(&mut self) {
        if let Some((pid, _)) = find_host(&self.cache) {
            unsafe { libc::kill(pid as i32, libc::SIGKILL) }; // the interactive sh ignores SIGTERM
            until("shell to exit", || !alive(pid));
        }
        let _ = self.service.kill();
        let _ = self.service.wait();
    }
}

impl Drop for World {
    fn drop(&mut self) {
        // Also when a test fails half way: nothing of ours may be left running.
        if let Some((pid, _)) = find_host(&self.cache) {
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        }
        let _ = self.service.kill();
        let _ = self.service.wait();
    }
}

/// Row and column of `needle` on the terminal's screen.
fn find(t: &mut Term, needle: &str) -> (u16, u16) {
    let text = t.text();
    for (row, line) in text.lines().enumerate() {
        if let Some(at) = line.find(needle) {
            return (row as u16, line[..at].chars().count() as u16);
        }
    }
    panic!("{needle:?} is not on screen:\n{text}");
}

#[test]
fn the_list_shows_background_and_enter_attaches_it_instead_of_resuming_and_leaving_keeps_it_running() {
    let mut w = World::new("bg-list");
    let mut t = Term::spawn(&[], &w.env(), &w.work, 30, 110);
    t.wait_for("an ordinary idle conversation"); // the saved conversations are read in the background
    t.wait_for("Background");
    let text = t.text();
    assert!(text.contains("idle"), "the ordinary conversation is still idle:\n{text}");
    assert!(t.flat().contains("Active 2"), "a background session is on the Active tab:\n{text}");

    // The status is drawn in the theme's accent (Catppuccin Mocha lavender), not the idle grey or working purple.
    let (row, col) = find(&mut t, "Background");
    let accent = vt100::Color::Rgb(0xb4, 0xbe, 0xfe);
    for i in 0..10 {
        assert_eq!(t.fg_at(row, col + i), accent, "cell {i} of the word Background");
    }
    let (idle_row, idle_col) = find(&mut t, "idle");
    assert_ne!(t.fg_at(idle_row, idle_col), accent);

    // Enter on the background row (the newest, so selected first): `claude attach dffac639` in its directory.
    t.send(b"\r");
    t.wait_for("fake-claude args:attach dffac639");
    let text = t.text();
    assert!(text.contains(&format!("cwd:{}", w.work.display())), "{text}");
    assert!(!text.contains("--resume") && !text.contains("Take over") && !text.contains("Reload"), "{text}");
    assert_eq!(w.launches(), ["attach dffac639"]);

    // Back to the list: the row is still Background; opening it again joins the same host, no second launch.
    t.send(&[0]);
    t.wait_for("enter open");
    // The footer can arrive before the rows of the same frame: wait for the row, don't read the screen once.
    t.wait_for("Background");
    t.send(b"\r");
    t.wait_for("fake-claude args:attach dffac639");
    assert_eq!((w.launches().len(), w.hosts()), (1, 1), "the second Enter joins the host that shows it");

    t.send(&[0]);
    t.wait_for("enter open");
    t.send(b"q");
    t.wait_for("still running in the background");
    t.wait_exit();
    assert!(w.service_alive(), "leaving platoon must not stop the background session");
    assert!(w.claude.join("sessions").read_dir().unwrap().count() == 1, "platoon touched the registry");
    w.cleanup();
}

#[test]
fn the_archive_key_on_a_background_row_says_no_and_changes_nothing() {
    let mut w = World::new("bg-archive");
    let mut t = Term::spawn(&[], &w.env(), &w.work, 30, 110);
    t.wait_for("an ordinary idle conversation");
    t.wait_for("Background");
    t.send(b"a");
    t.wait_for("can't archive it");
    assert!(t.flat().contains("Active 2"), "{}", t.text());
    assert!(w.service_alive());
    assert!(!w.config.join("archive.json").exists());
    assert!(w.launches().is_empty());
    t.send(b"q");
    t.wait_exit();
    w.cleanup();
}

#[test]
fn a_background_entry_whose_process_has_gone_is_an_ordinary_idle_conversation_again() {
    let mut w = World::new("bg-stale");
    let _ = w.service.kill();
    let _ = w.service.wait();
    let mut t = Term::spawn(&[], &w.env(), &w.work, 30, 110);
    t.wait_for("continue the release work");
    assert!(!t.text().contains("Background"), "{}", t.text());
    t.send(b"\r");
    t.wait_for(&format!("fake-claude args:--resume {BG_ID}"));
    assert_eq!(w.launches(), [format!("--resume {BG_ID}")]);
    t.send(&[0]);
    t.wait_for("enter open");
    t.send(b"q");
    t.wait_exit();
    w.cleanup();
}

#[test]
fn platoon_open_on_a_background_session_runs_attach_by_prefix_and_joins_it_the_second_time() {
    let mut w = World::new("bg-open");
    let mut t = Term::spawn(&["open", "dffa"], &w.env(), &w.work, 24, 100);
    t.wait_for("fake-claude args:attach dffac639");
    assert!(t.text().contains(&format!("cwd:{}", w.work.display())), "{}", t.text());
    assert!(!t.text().contains("--resume"));
    t.send(&[0]);
    assert_eq!(t.wait_exit(), 0);
    assert!(w.service_alive(), "detaching must not stop the background session");
    assert_eq!(w.launches(), ["attach dffac639"]);

    // The same command again joins the running attach host: the screen comes back and nothing is launched again.
    let mut t2 = Term::spawn(&["open", BG_ID], &w.env(), &w.work, 24, 100);
    t2.wait_for("fake-claude args:attach dffac639");
    t2.send(&[0]);
    assert_eq!(t2.wait_exit(), 0);
    assert_eq!((w.launches().len(), w.hosts()), (1, 1));
    assert!(w.service_alive());
    w.cleanup();
}
