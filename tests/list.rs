//! The conversation list end to end, in a real terminal with the real binary. A tiny fake claude stands in
//! for the real one (a real claude stops at a trust prompt in new directories), and every directory
//! platoon touches is a temp dir.

mod common;
use common::*;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

struct World {
    claude: PathBuf,
    cache: PathBuf,
    config: PathBuf,
    work: PathBuf,
}

/// A stand-in claude: says how it was started, then gives a shell.
fn fake_claude(dir: &Path) -> PathBuf {
    let p = dir.join("fake-claude.sh");
    fs::write(&p, "#!/bin/sh\necho \"fake-claude args:$* cwd:$(pwd)\"\nexec sh\n").unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    p
}

impl World {
    fn new(name: &str) -> Self {
        let base = tmp(name);
        let w = Self { claude: base.join("claude"), cache: base.join("cache"), config: base.join("config"), work: base.join("work") };
        for d in [&w.claude, &w.cache, &w.work, &w.claude.join("sessions")] {
            fs::create_dir_all(d).unwrap();
        }
        // These conversations carry fixed dates that will be old one day: this file is about the list, not auto-archive.
        fs::create_dir_all(&w.config).unwrap();
        fs::write(w.config.join("settings.json"), r#"{"auto_archive_days":0}"#).unwrap();
        w
    }

    fn env(&self) -> Vec<(&'static str, String)> {
        let program = fake_claude(&self.claude);
        vec![
            ("CLAUDE_CONFIG_DIR", self.claude.to_string_lossy().into()),
            ("PLATOON_CACHE_DIR", self.cache.to_string_lossy().into()),
            ("PLATOON_CONFIG_DIR", self.config.to_string_lossy().into()),
            ("PLATOON_CLAUDE_BIN", program.to_string_lossy().into()),
            ("PLATOON_SKIP_SETUP_CHECK", "1".into()),
        ]
    }

    /// A saved conversation in `self.work`, last active `hours_ago` hours before 23:00 on a fixed day.
    fn save(&self, id: &str, prompt: &str, hours_ago: u64) {
        let dir = self.claude.join("projects").join("-work");
        fs::create_dir_all(&dir).unwrap();
        let ts = format!("2026-09-19T{:02}:00:00.000Z", 23 - hours_ago);
        let line = format!(
            r#"{{"type":"user","cwd":"{}","gitBranch":"main","timestamp":"{ts}","message":{{"role":"user","content":"{prompt}"}}}}"#,
            self.work.display()
        );
        let p = dir.join(format!("{id}.jsonl"));
        fs::write(&p, line + "\n").unwrap();
    }

    fn archived(&self) -> Vec<String> {
        let v: serde_json::Value = match fs::read_to_string(self.config.join("archive.json")) {
            Ok(s) => serde_json::from_str(&s).unwrap(),
            Err(_) => return vec![],
        };
        v["archived"].as_object().unwrap().keys().cloned().collect()
    }

    fn cleanup(&self) {
        if let Some((pid, _)) = find_host(&self.cache) {
            unsafe { libc::kill(pid as i32, libc::SIGKILL) }; // interactive sh ignores SIGTERM
            until("shell to exit", || !alive(pid));
        }
    }
}

#[test]
fn enter_on_a_saved_conversation_resumes_it_in_its_own_directory_with_no_dialog() {
    let w = World::new("list-resume");
    w.save("beef-0001-aaaa", "refactor the widget", 5);
    let mut t = Term::spawn(&[], &w.env(), &w.work, 30, 110);
    t.wait_for("refactor the widget"); // listed although nothing is running it
    assert!(t.text().contains("idle") && t.flat().contains("Active 1"), "{}", t.text());
    t.send(b"\r");
    t.wait_for("fake-claude args:--resume beef-0001-aaaa");
    let text = t.text();
    assert!(text.contains(&format!("cwd:{}", w.work.display())), "{text}");
    assert!(!text.contains("Take over") && !text.contains("Reload"), "no dialog for a resting conversation:\n{text}");

    t.send(&[0]); // ctrl+space: back to the list
    t.wait_for("enter open");
    t.send(b"q");
    t.wait_for("still running in the background");
    t.wait_exit();
    assert!(find_host(&w.cache).is_some(), "the resumed session must keep running after platoon quits");
    w.cleanup();
}

#[test]
fn archive_from_the_list_then_open_from_the_archive_restores_and_resumes() {
    let w = World::new("list-archive");
    w.save("aaaa-0001", "older conversation", 10);
    w.save("bbbb-0002", "newer conversation", 1);
    let mut t = Term::spawn(&[], &w.env(), &w.work, 30, 110);
    t.wait_for_flat("Active 2");

    // Archive whichever row is selected, then see the counts and the file change.
    t.send(b"a");
    t.wait_for_flat("Active 1");
    assert!(t.flat().contains("Archived 1"), "{}", t.text());
    let gone = w.archived();
    assert_eq!(gone.len(), 1);

    // Switch tabs with the number key; the archived one is there. Enter restores it and opens it.
    t.send(b"\x1b[C"); // right arrow: Archived
    t.wait_for("a restore");
    t.send(b"\r");
    t.wait_for(&format!("fake-claude args:--resume {}", gone[0]));
    until("the archive file to drop it", || w.archived().is_empty());

    t.send(&[0]);
    t.wait_for_flat("Active 2");
    t.send(b"q");
    t.wait_exit();
    w.cleanup();
}
