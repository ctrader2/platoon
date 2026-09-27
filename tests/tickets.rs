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
        // The project's conversation comes from the background index: wait for its row, or a new ticket made
        // now has no session to take its folder from.
        t.wait_for_flat("○ work");
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
fn the_project_screen_has_three_panes_and_tab_and_shift_or_ctrl_arrows_move_between_them_while_the_old_keys_still_work() {
    let lab = Lab::new("tk-panes");
    let mut t = Term::spawn(&[], &lab.colored_env(), &lab.work, 30, 120);
    lab.open_project(&mut t);
    t.wait_for("No tickets yet · n to make one");
    let (s, k, n) = (find(&mut t, "╭ Sessions"), find(&mut t, "╭ Tickets"), find(&mut t, "╭ Notes"));
    assert!(s.1 == k.1 && k.0 > s.0 && n.0 == s.0 && n.1 > s.1, "Sessions over Tickets on the left, Notes on the right:\n{}", t.text());
    wait_title(&mut t, "Sessions", ACCENT);
    t.wait_for_flat("tab shift+arrows panes");
    t.send(b"\t");
    wait_title(&mut t, "Tickets", ACCENT);
    assert_ne!(t.fg_at(s.0, s.1 + 2), ACCENT, "only the focused pane's title is in the accent");
    t.wait_for_flat("n new ticket");
    t.send(b"\t");
    wait_title(&mut t, "Notes", ACCENT);
    t.send(b"\x1b[Z"); // shift+tab
    wait_title(&mut t, "Tickets", ACCENT);
    t.send(b"\x1b[Z");
    wait_title(&mut t, "Sessions", ACCENT);
    // The xterm forms of Shift+arrow (`ESC [1;2x`) and Ctrl+arrow (`ESC [1;5x`) do the same.
    for (m, name) in [(b'2', "shift"), (b'5', "ctrl")] {
        let arrow = |c: u8| vec![0x1b, b'[', b'1', b';', m, c];
        t.send(&arrow(b'B')); // down: Tickets
        wait_title(&mut t, "Tickets", ACCENT);
        t.send(&arrow(b'C')); // right: Notes
        wait_title(&mut t, "Notes", ACCENT);
        t.send(&arrow(b'D')); // left: back to Tickets, the one of the left column used last
        wait_title(&mut t, "Tickets", ACCENT);
        t.send(&arrow(b'A')); // up: Sessions
        wait_title(&mut t, "Sessions", ACCENT);
        t.send(&arrow(b'C'));
        wait_title(&mut t, "Notes", ACCENT);
        t.send(&arrow(b'D'));
        wait_title(&mut t, "Sessions", ACCENT);
        assert_ne!(t.fg_at(n.0, n.1 + 2), ACCENT, "{name}");
    }
    // Plain left and right stay in the pane (each key after them shows where the focus was). Sessions: → then tab
    // lands on Tickets (had → gone to Notes, tab would be back on Sessions).
    t.send(b"\x1b[C\t");
    wait_title(&mut t, "Tickets", ACCENT);
    // Tickets: → then tab lands on Notes.
    t.send(b"\x1b[C\t");
    wait_title(&mut t, "Notes", ACCENT);
    // Notes: ← then `e` is still the editor list for the notes (in the Tickets pane `e` would do nothing here).
    t.send(b"\x1b[De");
    t.wait_for("╭ edit the notes with");
    t.send(b"\x1b");
    t.wait_gone("edit the notes with");
    wait_title(&mut t, "Notes", ACCENT);
    // The Sessions pane's keys: n is still a new session (the folder picker).
    t.send(b"\x1b[1;5D"); // ctrl+left: Tickets, the one of the left column used last
    wait_title(&mut t, "Tickets", ACCENT);
    t.send(b"\x1b[1;5A"); // ctrl+up: Sessions
    wait_title(&mut t, "Sessions", ACCENT);
    t.send(b"n");
    t.wait_for("pick a directory");
    t.send(b"\x1b");
    t.wait_gone("pick a directory");
    t.send(&[3]); // ctrl+c quits from any screen (q goes back one level on a project's)
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
    t.wait_for_flat("#1 Log in with a link New ready");
    let v = lab.tickets();
    let one = &v["tickets"][0];
    assert_eq!((one["number"].as_u64(), one["state"].as_str(), one["file"].as_str()), (Some(1), Some("new"), Some("01-log-in-with-a-link.md")));
    assert!(one.get("how_far").is_none(), "the ticket's text says how far: {one}");
    assert_eq!(one["folders"], json!([lab.work.to_string_lossy()]), "the folder of the project's session");
    let body = fs::read_to_string(lab.config.join("projects/pweb/tickets/01-log-in-with-a-link.md")).unwrap();
    assert!(body.starts_with("## What it should do") && body.contains("## Acceptance") && body.ends_with("A link by email logs you in.\n"), "{body}");
    assert!(body.contains("How far: local changes only (say if you want a commit, or a push and merge request)."), "{body}");
    t.send(&[3]); // ctrl+c quits from any screen (q goes back one level on a project's)
    t.wait_exit();
}

#[test]
fn done_and_cancelled_are_folded_apart_and_a_ticket_moves_there_with_c_after_asking() {
    let lab = Lab::new("tk-fold");
    lab.write_tickets(&[(1, "Base", "done", &[]), (2, "Middle", "new", &[]), (3, "Top", "new", &[1, 2])]);
    let mut t = Term::spawn(&[], &lab.colored_env(), &lab.work, 30, 120);
    lab.open_project(&mut t);
    t.send(b"\t");
    t.wait_for_all(&["#2 Middle", "#3 Top", "▸ Done and cancelled · 1"]);
    let screen = t.stable();
    assert!(!screen.contains("#1 Base"), "folded:\n{screen}");
    let flat = screen.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(flat.contains("#2 Middle New ready") && flat.contains("#3 Top New waits for #2"), "{screen}");
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
    // #3 cancelled with c (after asking): it goes under the divider, whose count goes up.
    t.send(b"g");
    t.send(b"\x1b[B");
    t.wait_bg(r3, c3, SURFACE);
    t.wait_for_flat("c cancel");
    assert!(!t.stable().split_whitespace().collect::<Vec<_>>().join(" ").contains("s state"), "no state list any more");
    t.send(b"c");
    t.wait_for_all(&["Cancel ticket #3?", "No squad has had it", "keep it"]);
    t.send(b"y");
    t.wait_for("▸ Done and cancelled · 2");
    t.wait_gone("#3 Top");
    assert_eq!(lab.tickets()["tickets"][2]["state"], "cancelled");
    // Unfold and pick #3: c only says it is cancelled already.
    t.send(b"\r");
    t.wait_for("#3 Top");
    t.send(b"G");
    t.send(b"c");
    t.wait_for_flat("#3 is cancelled already.");
    let screen = t.stable();
    let (top, divider) = (screen.find("#3 Top").unwrap(), screen.find("Done and cancelled").unwrap());
    assert!(top > divider, "#3 stays under the divider:\n{screen}");
    assert_eq!(lab.tickets()["tickets"][2]["state"], "cancelled");
    t.send(&[3]); // ctrl+c quits from any screen (q goes back one level on a project's)
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
    t.wait_for_all(&["#2 Basket page", "State", "New · waits for #1", "Folders", "None: a squad works in its work folder", "Waits for", "#1 Base · New", "What it should do", "The basket keeps its items.", "step 1"]);
    t.wait_for_flat("c cancel e edit r rename f folders b waits for");
    let screen = t.stable();
    assert!(!screen.contains("How far") && !screen.contains("Local changes only") && !screen.contains("how far"), "no how far, though the file has one:\n{screen}");
    assert!(!t.text().contains("step 40"));
    for _ in 0..80 {
        t.send(b"\x1b[B");
    }
    t.wait_for("step 60");
    t.wait_gone("The basket keeps its items.");
    assert!(t.text().contains("New · waits for #1"), "the fields stay where they are");
    t.send(b"g");
    t.wait_for("The basket keeps its items.");
    // `h` was how far: it opens nothing now (the rename box after it proves the key was read).
    t.send(b"hr");
    t.wait_for("╭ rename ticket");
    assert!(!t.text().contains("how far"), "{}", t.text());
    t.send(b"\x1b");
    t.wait_gone("╭ rename ticket");
    assert_eq!(lab.tickets()["tickets"][1]["how_far"], "Local changes only", "an old value stays in the file");
    // Esc: the project's screen, the same ticket picked.
    t.send(b"\x1b");
    t.wait_for("╭ Tickets");
    t.wait_for_flat("enter open n new ticket c cancel");
    t.send(b"\r");
    t.wait_for_all(&["Waits for", "#1 Base · New", "The basket keeps its items."]);
    t.send(&[3]); // ctrl+c quits from any screen (q goes back one level on a project's)
    t.wait_exit();
}

#[test]
fn platoon_ticket_add_while_the_app_is_open_shows_up_without_a_restart() {
    let lab = Lab::new("tk-cli");
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 120);
    lab.open_project(&mut t);
    t.wait_for("No tickets yet");
    // --how-far is gone: refused, with where to say it instead.
    let (code, out, err) = lab.cli(&["add", "--project", "web", "--title", "x", "--how-far", "local commit"], "");
    assert!(code == 1 && out.is_empty() && err.trim() == format!("platoon ticket add: don't know \"--how-far\": say how far to take the work in the ticket's text instead (for example \"How far: a local commit\")"), "{err}");
    assert!(!lab.tickets_path().exists(), "nothing made");
    let (code, out, err) = lab.cli(&["add", "--project", "web", "--title", "From the command line", "--folder", "."], "## Body\npiped in\n\nHow far: a local commit.\n");
    assert_eq!((code, out.as_str(), err.as_str()), (0, "#1\n", ""));
    t.wait_for_flat("#1 From the command line New ready");
    let (code, out, _) = lab.cli(&["add", "--project", "pweb", "--title", "Second", "--waits-for", "1"], "");
    assert_eq!((code, out.as_str()), (0, "#2\n"));
    t.wait_for_flat("#2 Second New waits for #1");
    // Refusals say one plain line and make nothing.
    let (code, out, err) = lab.cli(&["add", "--project", "Nope", "--title", "x"], "");
    assert!(code != 0 && out.is_empty() && err.trim() == "platoon ticket add: there is no project called \"Nope\"", "{err}");
    let (code, _, err) = lab.cli(&["add", "--project", "Web", "--title", "x", "--blocked-by", "7"], "");
    assert!(code != 0 && err.contains("no ticket #7"), "{err}");
    assert_eq!(lab.tickets()["tickets"].as_array().unwrap().len(), 2);
    let body = fs::read_to_string(lab.config.join("projects/pweb/tickets/01-from-the-command-line.md")).unwrap();
    assert_eq!(body, "## Body\npiped in\n\nHow far: a local commit.\n");
    let (code, out, _) = lab.cli(&["list", "--json"], "");
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!((v[0]["title"].as_str(), v[0]["folders"][0].as_str()), (Some("From the command line"), Some(lab.work.canonicalize().unwrap().to_str().unwrap())));
    assert_eq!((v[1]["ready"].as_bool(), v[1]["blocked_by"].clone()), (Some(false), json!([1])));
    assert!(v[0].get("how_far").is_none() && v[1].get("how_far").is_none(), "{out}");
    let (_, out, _) = lab.cli(&["list"], "");
    assert!(out.contains("#1   From the command line  ·  New  ready") && !out.contains("Local"), "{out}");
    assert_eq!(v[1]["folders"], json!([lab.work.canonicalize().unwrap().to_str().unwrap()]), "no --folder: the folder it was run in");
    t.send(&[3]); // ctrl+c quits from any screen (q goes back one level on a project's)
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
    assert!(!flat.contains("local"), "no how far:\n{screen}");
    for line in screen.lines() {
        assert!(line.chars().count() <= 80, "{line}");
    }
    let (row, _) = find(&mut t, "#2 Second");
    let line = screen.lines().nth(row as usize).unwrap();
    let pane = line.split('│').nth(2).unwrap_or("");
    assert!(pane.contains("waits for #1"), "inside its own pane: {line}");
    t.send(&[3]); // ctrl+c quits from any screen (q goes back one level on a project's)
    t.wait_exit();
}

/// Ends the session host a test started (the stand-in claude), even when the test fails half way.
struct EndHost(PathBuf);

impl Drop for EndHost {
    fn drop(&mut self) {
        if let Some((pid, _)) = find_host(&self.0) {
            unsafe { libc::kill(pid as i32, libc::SIGKILL) }; // the stand-in this test started
            until("the stand-in claude to exit", || !alive(pid));
        }
    }
}

#[test]
fn a_ticket_made_from_a_session_shows_where_it_came_from_and_enter_opens_that_session() {
    let lab = Lab::new("tk-from");
    // The stand-in for claude: says how it was started, then waits.
    let fake = lab.bin.join("fake-claude");
    fs::write(&fake, "#!/bin/sh\necho \"FAKE CLAUDE ARGS: $*\"\nexec sh\n").unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let _end = EndHost(lab.cache.clone());
    let mut env = lab.env();
    env.push(("PLATOON_CLAUDE_BIN", fake.to_string_lossy().into()));
    // What the skill runs, from inside session s-1 (in the project "Web").
    assert_eq!(lab.cli(&["where", "--session", "s-1"], ""), (0, "Web (id pweb)\n".into(), String::new()));
    let (code, _, err) = lab.cli(&["where", "--session", "s-nowhere"], "");
    assert!(code == 1 && err.contains("isn't in a project") && err.contains("press m"), "{err}");
    let body = lab.base.join("body.md");
    fs::write(&body, "## What it should do\n\nA basket.\n").unwrap();
    let (code, out, err) = lab.cli(&["add", "--from-session", "s-1", "--title", "Basket page", "--body-file", body.to_str().unwrap(), "--folder", lab.work.to_str().unwrap()], "");
    assert_eq!((code, out.as_str(), err.as_str()), (0, "#1\n", ""));
    assert_eq!(lab.tickets()["tickets"][0]["from"], json!({"session": "s-1", "project": "pweb"}));
    let kinds = Command::new(BIN).args(["squads", "--kinds"]).envs(lab.env()).output().unwrap();
    assert!(String::from_utf8_lossy(&kinds.stdout).starts_with("Coding\n  Makes a change to code"), "{kinds:?}");

    let mut t = Term::spawn(&[], &env, &lab.work, 30, 120);
    lab.open_project(&mut t);
    t.send(b"\t");
    t.wait_for_all(&["#1 Basket page", "n new ticket"]);
    t.send(b"\r");
    // Its name on the Sessions tab is its folder, so the ticket names it by its first prompt.
    t.wait_for_all(&["From", "make the shop", "enter opens it", "A basket."]);
    t.wait_for_flat("enter where it came from");
    t.send(b"\r");
    t.wait_for("FAKE CLAUDE ARGS: --resume s-1");
    // Ctrl+Space: back to the ticket.
    t.send(&[0]);
    t.wait_for_all(&["From", "make the shop"]);
    t.send(&[3]); // ctrl+c quits from any screen (q goes back one level on a project's)
    t.wait_exit();
}

/// What `/to-tickets` runs to write a ticket, word for word from the skill: one `platoon ticket add` with the body in a
/// quoted heredoc. Run through `sh` with the real binary as `platoon` on the PATH, as Claude Code's Bash tool would.
#[test]
fn the_skills_heredoc_command_makes_a_ticket_with_that_body_and_an_empty_one_gets_the_template() {
    let lab = Lab::new("tk-heredoc");
    let path_dir = lab.base.join("path");
    fs::create_dir_all(&path_dir).unwrap();
    std::os::unix::fs::symlink(BIN, path_dir.join("platoon")).unwrap();
    let sh = |script: &str| -> (i32, String, String) {
        let out = Command::new("sh")
            .args(["-c", script])
            .envs(lab.env())
            .env("PATH", format!("{}:/usr/bin:/bin", path_dir.display()))
            .env("CLAUDE_CODE_SESSION_ID", "s-1")
            .env_remove("PLATOON_PROJECT")
            .current_dir(&lab.work)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into(), String::from_utf8_lossy(&out.stderr).into())
    };
    let body_of = |n: u64| -> String {
        let t = lab.tickets();
        let file = t["tickets"].as_array().unwrap().iter().find(|t| t["number"] == n).unwrap()["file"].as_str().unwrap().to_string();
        fs::read_to_string(lab.config.join("projects/pweb/tickets").join(file)).unwrap()
    };
    // The skill's own example, with a real title, folder and body.
    let skill = include_str!("../src/skills/to-tickets/SKILL.md");
    let step5 = &skill[skill.find("## 5.").unwrap()..];
    let example = &step5[step5.find("```sh\n").unwrap() + 6..];
    let example = &example[..example.find("```").unwrap()];
    let body = "## What it should do\n\nA basket that keeps `$HOME` and $(date) as they are.\n\n## Acceptance\n\n- [ ] it's there\n";
    let command = |title: &str, waits: &str| {
        example
            .replace("Title of the ticket", title)
            .replace("/path/to/main/folder", lab.work.to_str().unwrap())
            .replace(" --waits-for \"3\"", waits)
            .replace("## What it should do\n...\n", body)
    };
    let first = command("Basket page", "");
    assert!(first.starts_with("platoon ticket add --from-session \"$CLAUDE_CODE_SESSION_ID\" --title \"Basket page\"") && first.contains("A basket") && !first.contains("..."), "{first}");
    assert_eq!(sh(&first), (0, "#1\n".into(), String::new()));
    assert_eq!(body_of(1), body, "the heredoc's text, nothing expanded");
    let t = lab.tickets();
    assert_eq!(t["tickets"][0]["from"], json!({"session": "s-1", "project": "pweb"}));
    assert_eq!(t["tickets"][0]["folders"], json!([lab.work.to_str().unwrap()]));
    assert_eq!(sh(&command("Checkout", " --waits-for \"1\"")), (0, "#2\n".into(), String::new()));
    assert_eq!(lab.tickets()["tickets"][1]["blocked_by"], json!([1]));
    // A heredoc with nothing in it, and a command with nothing on stdin at all: the template.
    let empty = "platoon ticket add --from-session \"$CLAUDE_CODE_SESSION_ID\" --title \"Empty\" --no-folder <<'PLATOON_TICKET_BODY'\nPLATOON_TICKET_BODY\n";
    assert_eq!(sh(empty), (0, "#3\n".into(), String::new()));
    let nothing = "platoon ticket add --from-session \"$CLAUDE_CODE_SESSION_ID\" --title \"Nothing piped\" --no-folder";
    assert_eq!(sh(nothing), (0, "#4\n".into(), String::new()));
    let template = body_of(3);
    assert!(template.starts_with("## What it should do\n\nWhat should work when this is finished"), "{template}");
    assert_eq!(body_of(4), template);
}

/// A Done ticket whose worktree (a real one, in a temp repository) has a file not yet committed: platoon keeps it,
/// the ticket's screen says so with `w` to remove it anyway, and after a yes it is removed and its branch kept.
#[test]
fn a_done_tickets_worktree_with_uncommitted_changes_is_kept_said_on_its_screen_and_removed_after_a_yes() {
    let lab = Lab::new("tk-wt-kept");
    let shop = lab.base.join("shop");
    let wt = lab.base.join("shop-worktrees/ticket-1");
    fs::create_dir_all(&shop).unwrap();
    fs::write(shop.join("README"), "hi\n").unwrap();
    let git = |dir: &std::path::Path, args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    git(&shop, &["init", "-q"]);
    git(&shop, &["add", "."]);
    git(&shop, &["commit", "-q", "-m", "first"]);
    let base = git(&shop, &["rev-parse", "HEAD"]);
    git(&shop, &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "ticket/1"]);
    fs::write(wt.join("draft.txt"), "not committed yet\n").unwrap();
    let (shop_s, wt_s) = (shop.to_string_lossy().into_owned(), wt.to_string_lossy().into_owned());
    let v = json!({"next": 2, "tickets": [{"number": 1, "title": "Basket", "file": "01-t.md", "state": "done", "folders": [shop_s], "blocked_by": [], "created": 1, "updated": 1,
        "leaders": [{"squad": "coding", "session": "lead-1", "started": 1}],
        "worktrees": [{"folder": shop_s, "path": wt_s, "branch": "ticket/1", "base": base}]}]});
    fs::create_dir_all(lab.tickets_path().parent().unwrap()).unwrap();
    fs::write(lab.tickets_path(), v.to_string()).unwrap();

    let mut t = Term::spawn(&[], &lab.colored_env(), &lab.work, 32, 130);
    lab.open_project(&mut t);
    t.send(b"\t");
    t.wait_for("▸ Done and cancelled · 1");
    t.send(b"\r");
    t.wait_for("#1 Basket");
    t.send(b"G\r");
    // Wrapped in the ticket's column.
    t.wait_for_all(&["Kept because it has changes not yet", "committed.", "w removes it anyway", "Worktree"]);
    t.wait_for_flat("w remove worktree");
    assert!(wt.join("draft.txt").exists());
    assert_eq!(lab.tickets()["tickets"][0]["worktrees"][0]["kept_uncommitted"], true);
    // The words in the warning colour; the key in the accent and its words dimmed, like the footer's.
    let (ky, kx) = find(&mut t, "Kept because");
    t.wait_fg(ky, kx, WARN);
    let (wy, wx) = find(&mut t, "w removes it anyway");
    t.wait_fg(wy, wx, ACCENT);
    assert_eq!(t.fg_at(wy, wx + 2), vt100::Color::Rgb(0x6c, 0x70, 0x86), "the words in the dim colour");
    let (fy, fx) = find(&mut t, "w remove worktree");
    assert_eq!(t.fg_at(fy, fx), ACCENT);
    // `w` asks first; `n` keeps it.
    t.send(b"w");
    t.wait_for_all(&["Remove #1's worktree anyway?", "they will be lost", "y remove it", "n keep it"]);
    t.send(b"n");
    t.wait_gone("Remove #1's worktree anyway?");
    assert!(wt.join("draft.txt").exists());
    t.send(b"w");
    t.wait_for("Remove #1's worktree anyway?");
    t.send(b"y");
    t.wait_gone("Remove #1's worktree anyway?");
    t.wait_for_all(&["Removed; its branch ticket/1 is", "#1's worktree is removed."]);
    let screen = t.stable();
    assert!(!screen.contains("Kept because") && !screen.contains("w remove"), "{screen}");
    assert!(!wt.exists());
    assert_eq!(git(&shop, &["branch", "--list", "ticket/1", "--format=%(refname:short)"]), "ticket/1", "the branch is kept");
    assert!(lab.tickets()["tickets"][0]["worktrees"][0]["removed"].is_u64());
    assert_eq!(lab.tickets()["tickets"][0]["worktrees"][0]["base"], base.as_str());
    t.send(&[3]); // ctrl+c quits from any screen (q goes back one level on a project's)
    t.wait_exit();
}
