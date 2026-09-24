//! Editing a project's notes end to end, in a real terminal with the real binary: the template, an editor that
//! runs inside platoon's window, one with a window of its own, the before/after view, undo, the size warning and
//! the "Edit notes with" setting.
//!
//! The "editors" are two small scripts in a folder of the test's own, which `PLATOON_EDITOR_PATH` makes the only
//! place platoon looks: no real editor is ever started, and every directory is a temp dir.

mod common;
use common::*;
use serde_json::{json, Value};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

/// Stands in for nano: draws on its terminal, waits for one line, and puts that line into the file (a line
/// starting with `s/` is a replacement, as in sed, so a test can take a line away as well as add one).
const FAKE_TERMINAL: &str = r#"#!/bin/sh
f=""
for a in "$@"; do case "$a" in -*) ;; *) f="$a" ;; esac; done
echo "FAKE NANO EDITING $f"
read line
case "$line" in
  s/*) sed -i "$line" "$f" ;;
  *) printf '%s\n' "$line" >> "$f" ;;
esac
"#;

/// Stands in for VS Code: takes a moment in a window of its own and then writes into the file.
const FAKE_WINDOW: &str = r#"#!/bin/sh
f=""
for a in "$@"; do case "$a" in -*) ;; *) f="$a" ;; esac; done
sleep 0.4
printf 'written in the other window\n' >> "$f"
"#;

struct Lab {
    base: PathBuf,
    claude: PathBuf,
    cache: PathBuf,
    config: PathBuf,
    work: PathBuf,
    bin: PathBuf,
}

impl Lab {
    fn new(name: &str) -> Self {
        let base = tmp(name);
        let lab = Self {
            claude: base.join("claude"),
            cache: base.join("cache"),
            config: base.join("config"),
            work: base.join("work"),
            bin: base.join("bin"),
            base,
        };
        for d in [&lab.claude.join("sessions"), &lab.claude.join("projects/-work"), &lab.cache, &lab.config, &lab.work, &lab.bin] {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(lab.config.join("settings.json"), r#"{"auto_archive_days":0}"#).unwrap();
        for (name, body) in [("nano", FAKE_TERMINAL), ("code", FAKE_WINDOW)] {
            let p = lab.bin.join(name);
            fs::write(&p, body).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        }
        lab
    }

    fn env(&self) -> Vec<(&'static str, String)> {
        vec![
            ("CLAUDE_CONFIG_DIR", self.claude.to_string_lossy().into()),
            ("PLATOON_CACHE_DIR", self.cache.to_string_lossy().into()),
            ("PLATOON_CONFIG_DIR", self.config.to_string_lossy().into()),
            ("PLATOON_SKIP_SETUP_CHECK", "1".into()),
            // The only place platoon looks for editors: the two scripts above.
            ("PLATOON_EDITOR_PATH", self.bin.to_string_lossy().into()),
            ("EDITOR", String::new()),
            ("VISUAL", String::new()),
        ]
    }

    /// One project, "Web" (badge WB), with these notes (or none).
    fn project(&self, notes: Option<&str>) {
        let v = json!({"version": 1, "projects": {"pweb": {"name": "Web", "color": "blue", "badge": "WB", "archived": false, "created": 1, "archived_sessions": []}}, "sessions": {}});
        fs::write(self.config.join("projects.json"), v.to_string()).unwrap();
        fs::create_dir_all(self.config.join("projects/pweb")).unwrap();
        if let Some(n) = notes {
            fs::write(self.notes_path(), n).unwrap();
        }
    }

    fn notes_path(&self) -> PathBuf {
        self.config.join("projects/pweb/project.md")
    }

    fn notes(&self) -> String {
        fs::read_to_string(self.notes_path()).unwrap_or_default()
    }

    fn versions(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.config.join("projects/pweb/history"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn settings(&self) -> Value {
        fs::read_to_string(self.config.join("settings.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
    }

    /// Open the project's screen with the notes pane in focus.
    fn open_notes(&self, t: &mut Term) {
        t.send(b"2");
        t.wait_for_flat("WB Web");
        t.send(b"\r");
        t.wait_for("╭ Notes");
        t.send(b"\x1b[C"); // right: the notes pane
        t.wait_for_flat("e edit notes");
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

#[test]
fn e_starts_the_notes_from_the_template_runs_the_editor_inside_platoon_and_shows_what_it_wrote() {
    let lab = Lab::new("notes-edit");
    lab.project(None);
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 110);
    t.wait_for("enter open");
    lab.open_notes(&mut t);
    t.wait_for("No notes yet");

    // The setting is "Ask each time" to begin with, so e offers the editors that were found.
    t.send(b"e");
    t.wait_for("╭ edit the notes with");
    let screen = t.text();
    assert!(screen.contains("nano") && screen.contains("VS Code"), "both were found:\n{screen}");
    assert!(screen.contains("here in platoon") && screen.contains("in its own window"), "{screen}");
    t.send(b"\r"); // nano, the first

    // The file was started from the template, and the editor is running inside platoon's own window.
    t.wait_for("FAKE NANO EDITING");
    t.wait_for("ctrl+space stop the editor");
    let notes = lab.notes();
    for heading in ["# Overview", "# Requirements", "# Status", "# Decisions", "# Notes"] {
        assert!(notes.contains(heading), "{heading} missing from the template:\n{notes}");
    }
    assert!(notes.lines().next().unwrap().contains("Overview, Requirements and Status"), "{notes}");

    // What is typed reaches the editor; when it exits, platoon is back and shows the file as it is now.
    t.send(b"Sell socks online.\r");
    t.wait_for("Sell socks online.");
    until("the editor to write the notes", || lab.notes().contains("Sell socks online."));
    t.wait_for("The notes changed");
    t.wait_for("╭ Notes");
    assert!(t.text().contains("Sell socks online."), "the pane shows it:\n{}", t.text());
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn c_shows_what_changed_with_plain_marks_and_u_puts_the_notes_back() {
    let lab = Lab::new("notes-changes");
    lab.project(Some("# Overview\nSell socks.\n# Status\nStarted.\n"));
    let mut env = lab.env();
    env.push(("NO_COLOR", String::new())); // read it the way a terminal with no colors would
    let mut t = Term::spawn(&[], &env, &lab.work, 30, 110);
    t.wait_for("enter open");
    lab.open_notes(&mut t);
    t.wait_for("Sell socks.");

    t.send(b"e");
    t.wait_for("╭ edit the notes with");
    t.send(b"\r");
    t.wait_for("FAKE NANO EDITING");
    t.send(b"s/Sell socks./Sell socks and hats./\r");
    until("the editor to write the notes", || lab.notes().contains("And hats too.") || lab.notes().contains("socks and hats"));
    t.wait_for("The notes changed");
    assert_eq!(lab.versions().len(), 1, "the version before the edit was kept");

    // c: the before/after view. The marks are what carry the meaning.
    t.send(b"c");
    t.wait_for("╭ what changed last");
    t.wait_for("was there before");
    t.wait_for("is there now");
    let screen = t.text();
    assert!(screen.contains("- Sell socks."), "the line that went, marked -:\n{screen}");
    assert!(screen.contains("+ Sell socks and hats."), "the line that came, marked +:\n{screen}");
    assert!(screen.contains("  # Status"), "unchanged lines carry no mark:\n{screen}");
    t.send(b"\x1b");
    t.wait_gone("what changed last");

    // u: back as it was, and u again brings the edit back.
    t.send(b"u");
    t.wait_for("back as they were");
    until("the notes to go back", || !lab.notes().contains("socks and hats"));
    assert_eq!(lab.notes(), "# Overview\nSell socks.\n# Status\nStarted.\n");
    t.wait_gone("socks and hats");
    t.send(b"u");
    until("the undo to be undone", || lab.notes().contains("socks and hats"));
    t.wait_for("Sell socks and hats.");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn an_editor_with_its_own_window_says_so_platoon_carries_on_and_the_notes_come_back_changed() {
    let lab = Lab::new("notes-window");
    lab.project(Some("# Overview\nBefore the window.\n"));
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 110);
    t.wait_for("enter open");
    lab.open_notes(&mut t);

    // E always asks; pick VS Code, the second.
    t.send(b"E");
    t.wait_for("╭ edit the notes with");
    t.send(b"\x1b[B\r");
    t.wait_gone("edit the notes with");
    t.wait_for("Open in VS Code, in a window of its own.");
    // platoon is not blocked: the panes still answer, and the project's screen is still drawn.
    t.send(b"\x1b[D");
    t.wait_for_flat("a archive");
    t.send(b"\x1b[C");
    t.wait_for_flat("e edit notes");
    // When the editor is done, the notes are read again by themselves.
    until("the other window to write", || lab.notes().contains("written in the other window"));
    t.wait_for("written in the other window");
    t.wait_gone("Open in VS Code");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn notes_past_about_twenty_kilobytes_say_plainly_what_that_costs_and_nothing_is_cut() {
    let lab = Lab::new("notes-big");
    let body = "Sell socks to the whole world.\n".repeat(1000); // about 30 KB
    lab.project(Some(&format!("# Overview\n{body}")));
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 110);
    t.wait_for("enter open");
    lab.open_notes(&mut t);
    t.wait_for_flat("These notes are 30 KB");
    t.wait_for("The whole file is sent to every");
    t.wait_for("so it costs tokens each time.");
    assert_eq!(lab.notes().len(), "# Overview\n".len() + body.len(), "the file is left whole");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn the_edit_notes_with_setting_lists_what_was_found_saves_at_once_and_is_used_next_time() {
    let lab = Lab::new("notes-setting");
    lab.project(Some("# Overview\nA project.\n"));
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 110);
    t.wait_for("enter open");
    t.send(b"s");
    t.wait_for("Settings file:");
    t.send(b"G"); // the last entry, Smart features...
    t.send(b"\x1b[A"); // ...and up one: Edit notes with
    t.wait_for("Edit notes with");
    t.wait_for("Ask each time");
    t.wait_for("Only editors this computer already has");
    t.send(b"\x1b[C");
    t.wait_for_flat("‹ nano ›");
    until("the choice to reach the file", || lab.settings()["notes_editor"] == "nano");
    t.send(b"\x1b[C");
    t.wait_for_flat("‹ VS Code ›");
    t.send(b"\x1b[D");
    t.wait_for_flat("‹ nano ›");
    t.send(b"\x1b");
    t.wait_gone("Settings file:");
    t.send(b"q");
    t.wait_exit();
    assert_eq!(lab.settings()["notes_editor"], "nano");

    // A new run reads it back, and e now opens that editor without asking.
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 110);
    t.wait_for("enter open");
    lab.open_notes(&mut t);
    t.send(b"e");
    t.wait_for("FAKE NANO EDITING");
    assert!(!t.text().contains("edit the notes with"), "no question: the setting answered it");
    t.send(b"One more line.\r");
    until("the editor to write the notes", || lab.notes().contains("One more line."));
    t.wait_for("The notes changed");
    t.send(b"q");
    t.wait_exit();
}
