//! Tickets end to end, in a real terminal with the real binary: the three panes of a project's screen and moving
//! between them, making a ticket with `n` and a stand-in editor, the Done and cancelled group, waits for and ready,
//! a ticket's own screen, `platoon ticket add` and `list` while the app is open, and an 80-column terminal.
//!
//! The editor is a small script in a folder of the test's own (`PLATOON_EDITOR_PATH` is the only place platoon looks),
//! and every directory is a temp dir: no real editor, no real `claude`, and never the user's own config.

mod common;
use common::*;
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Stdio},
};

/// Stands in for nano: draws on its terminal, waits for one line, and adds it to the file.
const FAKE_TERMINAL: &str = r#"#!/bin/sh
f=""
for a in "$@"; do case "$a" in -*) ;; *) f="$a" ;; esac; done
echo "FAKE NANO EDITING $f"
read line
printf '%s\n' "$line" >> "$f"
"#;

/// Catppuccin Mocha, the default theme: what the cells are drawn in.
const ACCENT: vt100::Color = vt100::Color::Rgb(0xb4, 0xbe, 0xfe);
const OK: vt100::Color = vt100::Color::Rgb(0xa6, 0xe3, 0xa1);
const WARN: vt100::Color = vt100::Color::Rgb(0xf9, 0xe2, 0xaf);
const SURFACE: vt100::Color = vt100::Color::Rgb(0x31, 0x32, 0x44);
const BG: vt100::Color = vt100::Color::Rgb(0x1e, 0x1e, 0x2e);

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
        let p = lab.bin.join("nano");
        fs::write(&p, FAKE_TERMINAL).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        // One project, "Web" (badge WB), with one conversation in the work folder.
        let v = json!({"version": 1, "projects": {"pweb": {"name": "Web", "color": "blue", "badge": "WB", "archived": false, "created": 1, "archived_sessions": []}}, "sessions": {"s-1": "pweb"}});
        fs::write(lab.config.join("projects.json"), v.to_string()).unwrap();
        let line = json!({"type":"user","cwd":lab.work,"gitBranch":"main","timestamp":"2026-09-24T10:00:00.000Z","message":{"role":"user","content":"make the shop"}}).to_string();
        fs::write(lab.claude.join("projects/-work/s-1.jsonl"), line + "\n").unwrap();
        lab
    }

    fn env(&self) -> Vec<(&'static str, String)> {
        vec![
            ("CLAUDE_CONFIG_DIR", self.claude.to_string_lossy().into()),
            ("PLATOON_CACHE_DIR", self.cache.to_string_lossy().into()),
            ("PLATOON_CONFIG_DIR", self.config.to_string_lossy().into()),
            ("PLATOON_SKIP_SETUP_CHECK", "1".into()),
            ("PLATOON_NOTES_HELPER", "off".into()),
            ("PLATOON_EDITOR_PATH", self.bin.to_string_lossy().into()),
            ("EDITOR", String::new()),
            ("VISUAL", String::new()),
        ]
    }

    /// The same, with colors drawn (the harness turns them off by default), for the tests that read cell colors.
    fn colored_env(&self) -> Vec<(&'static str, String)> {
        let mut env = self.env();
        env.push(("NO_COLOR", String::new()));
        env
    }

    fn tickets_path(&self) -> PathBuf {
        self.config.join("projects/pweb/tickets.json")
    }

    fn tickets(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.tickets_path()).unwrap()).unwrap()
    }

    /// Tickets written straight into the file: (number, title, state, blocked by).
    fn write_tickets(&self, list: &[(u32, &str, &str, &[u32])]) {
        let tickets: Vec<Value> = list
            .iter()
            .map(|(n, title, state, blocked)| {
                json!({"number": n, "title": title, "file": format!("{n:02}-t.md"), "state": state, "how_far": "Local changes only", "folders": [], "blocked_by": blocked, "created": 1, "updated": 1})
            })
            .collect();
        let next = list.iter().map(|t| t.0).max().unwrap_or(0) + 1;
        fs::create_dir_all(self.config.join("projects/pweb/tickets")).unwrap();
        fs::write(self.tickets_path(), json!({"next": next, "tickets": tickets}).to_string()).unwrap();
    }

    /// `platoon ticket ...` with this lab's directories, and what it printed.
    fn cli(&self, args: &[&str], stdin: &str) -> (i32, String, String) {
        let mut c = Command::new(BIN);
        c.arg("ticket").args(args).current_dir(&self.work).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        for (k, v) in self.env() {
            c.env(k, v);
        }
        let mut child = c.spawn().unwrap();
        {
            use std::io::Write;
            let mut input = child.stdin.take().unwrap();
            let _ = input.write_all(stdin.as_bytes());
        }
        let out = child.wait_with_output().unwrap();
        (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into(), String::from_utf8_lossy(&out.stderr).into())
    }

    /// Open the project's screen.
    fn open_project(&self, t: &mut Term) {
        t.wait_for("enter open");
        t.send(b"2");
        t.wait_for_flat("WB Web");
        t.send(b"\r");
        t.wait_for_all(&["╭ Sessions", "╭ Tickets", "╭ Notes"]);
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn find(t: &mut Term, needle: &str) -> (u16, u16) {
    t.find(needle).unwrap_or_else(|| panic!("{needle:?} is not on screen:\n{}", t.text()))
}

/// Waits until the title of a pane (`╭ Name`) is drawn in `color`.
fn wait_title(t: &mut Term, name: &str, color: vt100::Color) {
    let (row, col) = find(t, &format!("╭ {name}"));
    t.wait_fg(row, col + 2, color);
}

#[test]
fn the_project_screen_has_three_panes_and_tab_left_and_right_move_between_them_while_the_old_keys_still_work() {
    let lab = Lab::new("tk-panes");
    let mut t = Term::spawn(&[], &lab.colored_env(), &lab.work, 30, 120);
    lab.open_project(&mut t);
    t.wait_for("No tickets yet · n to make one");
    let (s, k, n) = (find(&mut t, "╭ Sessions"), find(&mut t, "╭ Tickets"), find(&mut t, "╭ Notes"));
    assert!(s.1 == k.1 && k.0 > s.0 && n.0 == s.0 && n.1 > s.1, "Sessions over Tickets on the left, Notes on the right:\n{}", t.text());
    wait_title(&mut t, "Sessions", ACCENT);
    t.send(b"\t");
    wait_title(&mut t, "Tickets", ACCENT);
    assert_ne!(t.fg_at(s.0, s.1 + 2), ACCENT, "only the focused pane's title is in the accent");
    t.wait_for_flat("n new ticket");
    t.send(b"\t");
    wait_title(&mut t, "Notes", ACCENT);
    t.send(b"\x1b[D"); // left: back to Tickets, the one of the left column used last
    wait_title(&mut t, "Tickets", ACCENT);
    t.send(b"\x1b[Z"); // shift+tab
    wait_title(&mut t, "Sessions", ACCENT);
    // The Sessions pane's keys: n is still a new session (the folder picker).
    t.send(b"n");
    t.wait_for("pick a directory");
    t.send(b"\x1b");
    t.wait_gone("pick a directory");
    // The Notes pane's keys: e is still the editor list for the notes.
    t.send(b"\x1b[C");
    wait_title(&mut t, "Notes", ACCENT);
    t.send(b"e");
    t.wait_for("╭ edit the notes with");
    t.send(b"\x1b");
    t.wait_gone("edit the notes with");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn n_in_the_tickets_pane_makes_a_ticket_and_opens_its_body_in_the_editor() {
    let lab = Lab::new("tk-new");
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 120);
    lab.open_project(&mut t);
    t.send(b"\t");
    t.wait_for_flat("n new ticket");
    // Esc in the name box makes nothing.
    t.send(b"n");
    t.wait_for("╭ new ticket");
    t.send(b"Never mind");
    t.wait_for("Never mind");
    t.send(b"\x1b");
    t.wait_gone("╭ new ticket");
    assert!(!lab.tickets_path().exists());
    t.send(b"n");
    t.wait_for("╭ new ticket");
    t.send(b"Log in with a link\r");
    t.wait_for_all(&["╭ edit ticket #1 with", "nano"]);
    t.send(b"\r");
    t.wait_for("FAKE NANO EDITING");
    t.wait_for("nano · ticket #1");
    t.send(b"A link by email logs you in.\r");
    t.wait_for("Saved the body of #1.");
    t.wait_for_flat("#1 Log in with a link New local ready");
    let v = lab.tickets();
    let one = &v["tickets"][0];
    assert_eq!((one["number"].as_u64(), one["state"].as_str(), one["how_far"].as_str(), one["file"].as_str()), (Some(1), Some("new"), Some("Local changes only"), Some("01-log-in-with-a-link.md")));
    assert_eq!(one["folders"], json!([lab.work.to_string_lossy()]), "the folder of the project's session");
    let body = fs::read_to_string(lab.config.join("projects/pweb/tickets/01-log-in-with-a-link.md")).unwrap();
    assert!(body.starts_with("## What it should do") && body.contains("## Acceptance") && body.ends_with("A link by email logs you in.\n"), "{body}");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn done_and_cancelled_are_folded_apart_and_a_ticket_moves_there_and_back_with_s() {
    let lab = Lab::new("tk-fold");
    lab.write_tickets(&[(1, "Base", "done", &[]), (2, "Middle", "new", &[]), (3, "Top", "new", &[1, 2])]);
    let mut t = Term::spawn(&[], &lab.colored_env(), &lab.work, 30, 120);
    lab.open_project(&mut t);
    t.send(b"\t");
    t.wait_for_all(&["#2 Middle", "#3 Top", "▸ Done and cancelled · 1"]);
    let screen = t.stable();
    assert!(!screen.contains("#1 Base"), "folded:\n{screen}");
    let flat = screen.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(flat.contains("#2 Middle New local ready") && flat.contains("#3 Top New local waits for #2"), "{screen}");
    // Colors, by cell: the picked row has the highlight, ready is green, waits for is yellow.
    let (r2, c2) = find(&mut t, "#2 Middle");
    let (r3, c3) = find(&mut t, "#3 Top");
    t.wait_bg(r2, c2, SURFACE);
    assert_eq!(t.bg_at(r3, c3), BG, "another row");
    let (ry, rx) = find(&mut t, "ready");
    assert_eq!(t.fg_at(ry, rx), OK);
    let (by, bx) = find(&mut t, "waits for #2");
    assert_eq!(t.fg_at(by, bx), WARN);
    // Enter on the divider unfolds it.
    t.send(b"G\r");
    t.wait_for_all(&["▾ Done and cancelled · 1", "#1 Base"]);
    let (dy, dx) = find(&mut t, "Done  ");
    assert_eq!(t.fg_at(dy, dx), OK, "Done in green");
    t.send(b"\r");
    t.wait_gone("#1 Base");
    // #3 to Cancelled with s: it goes under the divider, whose count goes up.
    t.send(b"g");
    t.send(b"\x1b[B");
    t.wait_bg(r3, c3, SURFACE);
    t.send(b"s");
    t.wait_for_all(&["╭ state of #3", "Cancelled"]);
    t.send(b"\x1b[B\x1b[B\r");
    t.wait_for("▸ Done and cancelled · 2");
    t.wait_gone("#3 Top");
    assert_eq!(lab.tickets()["tickets"][2]["state"], "cancelled");
    // Unfold, pick #3 and make it New again: back up with the open ones.
    t.send(b"\r");
    t.wait_for("#3 Top");
    t.send(b"G");
    t.send(b"s");
    t.wait_for("╭ state of #3");
    t.send(b"g\r");
    t.wait_for("is New again");
    t.wait_for("▾ Done and cancelled · 1");
    let screen = t.stable();
    let (top, divider) = (screen.find("#3 Top").unwrap(), screen.find("Done and cancelled").unwrap());
    assert!(top < divider, "#3 is above the divider again:\n{screen}");
    assert_eq!(lab.tickets()["tickets"][2]["state"], "new");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn a_tickets_screen_shows_its_fields_and_scrolls_its_body_and_esc_goes_back_to_the_same_row() {
    let lab = Lab::new("tk-screen");
    lab.write_tickets(&[(1, "Base", "new", &[]), (2, "Basket page", "new", &[1])]);
    let steps: String = (1..=60).map(|i| format!("- step {i}\n")).collect();
    fs::write(lab.config.join("projects/pweb/tickets/02-t.md"), format!("## What it should do\n\nThe basket keeps its items.\n\n{steps}")).unwrap();
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 110);
    lab.open_project(&mut t);
    t.send(b"\t");
    t.wait_for("#2 Basket page");
    t.send(b"\x1b[B\r");
    t.wait_for_all(&["#2 Basket page", "State", "New · waits for #1", "How far", "Local changes only", "Folders", "No folder yet", "Waits for", "#1 Base · New", "What it should do", "The basket keeps its items.", "step 1"]);
    t.wait_for_flat("e edit r rename s state h how far f folders b waits for");
    assert!(!t.text().contains("step 40"));
    for _ in 0..80 {
        t.send(b"\x1b[B");
    }
    t.wait_for("step 60");
    t.wait_gone("The basket keeps its items.");
    assert!(t.text().contains("New · waits for #1"), "the fields stay where they are");
    t.send(b"g");
    t.wait_for("The basket keeps its items.");
    // How far, in the user's own words.
    t.send(b"h");
    t.wait_for_all(&["╭ how far #2 goes", "In my own words…"]);
    t.send(b"G\r");
    t.wait_for("╭ how far");
    t.send(b"Until the tests pass\r");
    t.wait_for_flat("How far Until the tests pass");
    assert_eq!(lab.tickets()["tickets"][1]["how_far"], "Until the tests pass");
    // Esc: the project's screen, the same ticket picked.
    t.send(b"\x1b");
    t.wait_for("╭ Tickets");
    t.wait_for_flat("enter open n new ticket s state");
    t.send(b"\r");
    t.wait_for_all(&["Waits for", "#1 Base · New", "Until the tests pass"]);
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn platoon_ticket_add_while_the_app_is_open_shows_up_without_a_restart() {
    let lab = Lab::new("tk-cli");
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 120);
    lab.open_project(&mut t);
    t.wait_for("No tickets yet");
    let (code, out, err) = lab.cli(&["add", "--project", "web", "--title", "From the command line", "--how-far", "local commit", "--folder", "."], "## Body\npiped in\n");
    assert_eq!((code, out.as_str(), err.as_str()), (0, "#1\n", ""));
    t.wait_for_flat("#1 From the command line New commit ready");
    let (code, out, _) = lab.cli(&["add", "--project", "pweb", "--title", "Second", "--waits-for", "1"], "");
    assert_eq!((code, out.as_str()), (0, "#2\n"));
    t.wait_for_flat("#2 Second New local waits for #1");
    // Refusals say one plain line and make nothing.
    let (code, out, err) = lab.cli(&["add", "--project", "Nope", "--title", "x"], "");
    assert!(code != 0 && out.is_empty() && err.trim() == "platoon ticket add: there is no project called \"Nope\"", "{err}");
    let (code, _, err) = lab.cli(&["add", "--project", "Web", "--title", "x", "--blocked-by", "7"], "");
    assert!(code != 0 && err.contains("no ticket #7"), "{err}");
    assert_eq!(lab.tickets()["tickets"].as_array().unwrap().len(), 2);
    let body = fs::read_to_string(lab.config.join("projects/pweb/tickets/01-from-the-command-line.md")).unwrap();
    assert_eq!(body, "## Body\npiped in\n");
    let (code, out, _) = lab.cli(&["list", "--json"], "");
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!((v[0]["title"].as_str(), v[0]["folders"][0].as_str()), (Some("From the command line"), Some(lab.work.canonicalize().unwrap().to_str().unwrap())));
    assert_eq!((v[1]["ready"].as_bool(), v[1]["blocked_by"].clone()), (Some(false), json!([1])));
    assert_eq!(v[1]["folders"], json!([lab.work.canonicalize().unwrap().to_str().unwrap()]), "no --folder: the folder it was run in");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn eighty_columns_keep_each_tickets_number_title_and_what_it_waits_for() {
    let lab = Lab::new("tk-80");
    lab.write_tickets(&[(1, "A rather long title for a ticket", "new", &[]), (2, "Second", "new", &[1]), (3, "Third", "cancelled", &[])]);
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 24, 80);
    lab.open_project(&mut t);
    t.send(b"\t");
    t.wait_for_all(&["#1 A rather", "ready", "#2 Second", "waits for #1", "Done and cancelled · 1"]);
    t.send(b"G\r");
    t.wait_for("#3 Third");
    let screen = t.stable();
    let flat = screen.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(flat.contains("#3 Third Cancelled"), "{screen}");
    assert!(!flat.contains("local"), "how far is the first to go:\n{screen}");
    for line in screen.lines() {
        assert!(line.chars().count() <= 80, "{line}");
    }
    let (row, _) = find(&mut t, "#2 Second");
    let line = screen.lines().nth(row as usize).unwrap();
    let pane = line.split('│').nth(2).unwrap_or("");
    assert!(pane.contains("waits for #1"), "inside its own pane: {line}");
    t.send(b"q");
    t.wait_exit();
}
