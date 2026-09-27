//! Editing a project's notes end to end, in a real terminal with the real binary: the template, the editor list
//! `e` opens (with the one used last starred and picked), an editor that runs inside platoon's window, one with a
//! window of its own, the before/after view, undo, what the pane says about the notes reaching the project's
//! sessions, and what it says when the updates are paused.
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
            ("PLATOON_NOTES_HELPER", "off".into()),
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

    fn write_settings(&self, v: Value) {
        fs::write(self.config.join("settings.json"), v.to_string()).unwrap();
    }

    /// A conversation of the project: a transcript Claude Code would have written, and its membership.
    fn session(&self, id: &str, prompt: &str) {
        let line = json!({"type":"user","cwd":self.work,"gitBranch":"main","timestamp":"2026-09-24T10:00:00.000Z","message":{"role":"user","content":prompt}}).to_string();
        fs::write(self.claude.join("projects/-work").join(format!("{id}.jsonl")), line + "\n").unwrap();
        let mut v: Value = serde_json::from_str(&fs::read_to_string(self.config.join("projects.json")).unwrap()).unwrap();
        v.get_mut("sessions").unwrap().as_object_mut().unwrap().insert(id.to_string(), json!("pweb"));
        fs::write(self.config.join("projects.json"), v.to_string()).unwrap();
    }

    /// Today, in the local calendar, as the ledger keys its days.
    fn today(&self) -> String {
        let out = std::process::Command::new("date").arg("+%F").output().unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    /// Put this many tokens on today's count, the way a day of real calls would.
    fn spend_the_day(&self, tokens: u64) {
        let day = json!({"usage": {"input": tokens, "cache_creation": 0, "cache_read": 0, "output": 0}, "cost_usd": 0.0, "calls": 1, "failed": 0, "features": {}});
        let v = json!({"version": 1, "days": {self.today(): day}});
        fs::write(self.cache.join("ai-usage.json"), v.to_string()).unwrap();
    }

    fn clear_the_day(&self) {
        let _ = fs::remove_file(self.cache.join("ai-usage.json"));
    }

    /// A status-line report saying the plan is this much used, with the window still running.
    fn plan_used(&self, pct: u64) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        fs::create_dir_all(self.cache.join("status")).unwrap();
        let v = json!({"ts": now, "context_pct": null, "five_hour": {"pct": pct, "resets_at": now + 3600}, "seven_day": null});
        fs::write(self.cache.join("status/s1.json"), v.to_string()).unwrap();
    }

    fn clear_the_plan(&self) {
        let _ = fs::remove_file(self.cache.join("status/s1.json"));
    }

    /// Open the project's screen with the notes pane in focus.
    fn open_notes(&self, t: &mut Term) {
        t.send(b"2");
        t.wait_for_flat("WB Web");
        t.send(b"\r");
        t.wait_for("╭ Notes");
        t.send(b"\x1b[1;2C"); // shift+right: the notes pane
        t.wait_for_flat("e edit notes");
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

/// The words of the notes pane as one line, whatever they wrapped onto: the right-hand pane of the project screen
/// with its borders taken off, so a sentence that runs over two lines still reads as one.
fn pane(t: &mut Term) -> String {
    let screen = t.text();
    let mut words: Vec<String> = Vec::new();
    for row in screen.lines() {
        let parts: Vec<&str> = row.split('\u{2502}').collect();
        if parts.len() < 5 {
            continue;
        }
        words.extend(parts[parts.len() - 3].split_whitespace().map(str::to_string));
    }
    words.join(" ")
}

fn wait_pane(t: &mut Term, text: &str) {
    until(&format!("the notes pane to say {text:?}"), || pane(t).contains(text));
}

fn wait_pane_gone(t: &mut Term, text: &str) {
    // Twice over, a moment apart: a half-read frame can be missing words that are still there really.
    until(&format!("the notes pane to stop saying {text:?}"), || {
        !pane(t).contains(text) && {
            std::thread::sleep(std::time::Duration::from_millis(80));
            !pane(t).contains(text)
        }
    });
}

#[test]
fn e_starts_the_notes_from_the_template_runs_the_editor_inside_platoon_and_shows_what_it_wrote() {
    let lab = Lab::new("notes-edit");
    lab.project(None);
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 110);
    t.wait_for("enter open");
    lab.open_notes(&mut t);
    // A project with no sessions and no notes: one line, and e still starts them.
    t.wait_for("platoon will keep project notes up to date.");

    // No editor used yet, so e offers the ones that were found, with where each of them runs. (All of it lands
    // in one frame, and half a frame can be read, so it is waited for together rather than snapshotted.)
    t.send(b"e");
    t.wait_for_all(&["╭ edit the notes with", "nano", "VS Code", "here in platoon", "in its own window"]);
    t.send(b"\r"); // nano, the first

    // The file was started from the template, and the editor is running inside platoon's own window.
    t.wait_for("FAKE NANO EDITING");
    t.wait_for("ctrl+space stop the editor");
    let notes = lab.notes();
    for heading in ["# Summary", "# Decisions"] {
        assert!(notes.contains(heading), "{heading} missing from the template:\n{notes}");
    }
    assert!(notes.starts_with("# Summary"), "the line about who writes what is the pane's now, not the file's:\n{notes}");

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
    // The whole popup arrives as one frame: the legend, the line that went and the line that came.
    t.wait_for_all(&["╭ what changed last", "was there before", "is there now", "- Sell socks.", "+ Sell socks and hats."]);
    let screen = t.stable();
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

    // e lists them; pick VS Code, the second.
    t.send(b"e");
    t.wait_for("╭ edit the notes with");
    t.send(b"\x1b[B\r");
    t.wait_gone("edit the notes with");
    t.wait_for("Open in VS Code, in a window of its own.");
    // platoon is not blocked: the panes still answer, and the project's screen is still drawn.
    t.send(b"\x1b[1;5D"); // ctrl+left: the sessions
    t.wait_for_flat("a archive");
    t.send(b"\x1b[1;5C"); // ctrl+right: the notes
    t.wait_for_flat("e edit notes");
    // When the editor is done, the notes are read again by themselves.
    until("the other window to write", || lab.notes().contains("written in the other window"));
    t.wait_for("written in the other window");
    t.wait_gone("Open in VS Code");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn long_notes_are_not_warned_about_however_long_they_get_and_nothing_is_cut() {
    let lab = Lab::new("notes-big");
    let body = "Sell socks to the whole world.\n".repeat(1400); // about 42 KB, twice the old warning
    lab.project(Some(&format!("# Overview\n{body}")));
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 110);
    t.wait_for("enter open");
    lab.open_notes(&mut t);
    t.wait_for("Sell socks to the whole world.");
    // Nothing is being claimed to be *missing* from half a frame: the screen has to have settled first.
    let shown = t.stable().split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(!shown.contains("costs tokens") && !shown.contains("KB"), "no warning about the length:\n{shown}");
    assert_eq!(lab.notes().len(), "# Overview\n".len() + body.len(), "the file is left whole");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn e_lists_the_editors_and_the_one_used_last_is_starred_first_and_opened_by_enter_alone() {
    let lab = Lab::new("notes-star");
    lab.project(Some("# Overview\nA project.\n"));
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 110);
    t.wait_for("enter open");
    lab.open_notes(&mut t);

    // Nothing used yet: the editors in the order they were found, nothing starred, the first one picked.
    t.send(b"e");
    t.wait_for_all(&["╭ edit the notes with", "nano", "VS Code", "starred here next time"]);
    // "nothing is starred" is a claim about the whole frame, so it waits for one that has settled.
    let shown = t.stable();
    assert!(!shown.contains("★"), "nothing to star yet:\n{shown}");
    // The second one, VS Code, in a window of its own.
    t.send(b"\x1b[B\r");
    t.wait_gone("edit the notes with");
    t.wait_for("Open in VS Code, in a window of its own.");
    until("the other window to write", || lab.notes().contains("written in the other window"));
    t.wait_gone("Open in VS Code");
    until("the choice to reach the settings file", || lab.settings()["notes_editor"] == "code");

    // Now it is starred, first and picked, so enter alone opens it again. Which row the star is on, and which
    // row comes first, are claims about the whole popup: wait for all of it, then for it to settle.
    t.send(b"e");
    t.wait_for_all(&["╭ edit the notes with", "★ VS Code", "nano", "the one you used last: enter opens it again"]);
    let shown = t.stable();
    let starred = shown.lines().find(|l| l.contains("VS Code")).unwrap_or("");
    assert!(starred.contains("★"), "the one used last carries the star: {starred}");
    assert!(!shown.lines().find(|l| l.contains("nano")).unwrap_or("").contains("★"), "only that one:\n{shown}");
    let rows: Vec<&str> = shown.lines().filter(|l| l.contains("here in platoon") || l.contains("in its own window")).collect();
    assert!(rows[0].contains("VS Code"), "and it is first: {rows:?}");
    t.send(b"\r");
    t.wait_gone("edit the notes with");
    t.wait_for("Open in VS Code, in a window of its own.");
    until("the second write", || lab.notes().matches("written in the other window").count() == 2);
    t.send(b"q");
    t.wait_exit();

    // A fresh platoon reads the choice back: still starred, still first.
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 110);
    t.wait_for("enter open");
    lab.open_notes(&mut t);
    t.send(b"e");
    t.wait_for_all(&["╭ edit the notes with", "★ VS Code"]);
    let shown = t.stable();
    assert!(shown.lines().find(|l| l.contains("VS Code")).unwrap_or("").contains("★"), "{shown}");
    t.send(b"\x1b");
    t.wait_gone("edit the notes with");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn the_pane_and_the_header_say_when_the_notes_are_not_updating_and_why_and_stop_when_the_reason_goes() {
    let lab = Lab::new("notes-paused");
    lab.project(Some("# Overview\nSell socks.\n"));
    lab.session("sess-1", "A basket page");

    // The switch off, with the features on: the pane says which switch, and where.
    lab.write_settings(json!({"auto_archive_days": 0, "tokens_enabled": true, "token_features": {"project_notes": false}}));
    // Wide enough for the three tabs, the usage and the whole note beside them.
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 165);
    t.wait_for("enter open");
    lab.open_notes(&mut t);
    wait_pane(&mut t, "These notes are not updating right now: \"Project notes\" is switched off. Press s for the settings to switch it back on.");
    assert!(!t.flat().contains("smart features paused"), "one switch is not the whole header's business:\n{}", t.flat());

    // The daily limit spent: said in the pane in full, and in the header for every tab to see.
    lab.write_settings(json!({"auto_archive_days": 0, "tokens_enabled": true, "ai_daily_tokens": 20000}));
    lab.spend_the_day(25_000);
    wait_pane(&mut t, "These notes are not updating right now: Today's token limit is used up (25,000 of 20,000 tokens). It starts over tomorrow, or you can raise it in settings.");
    t.wait_for_flat("smart features paused · today's 20,000 limit is used up · press s for the settings");
    // The header says it on the Sessions tab as well: that is the point of it being there.
    t.send(b"1");
    t.wait_for_flat("smart features paused · today's 20,000 limit is used up");

    // The plan pause, the one that caught the user out.
    lab.clear_the_day();
    lab.plan_used(82);
    t.wait_for_flat("smart features paused · your Claude plan is 82% used (5-hour limit) · press s for the settings");
    t.send(b"2");
    t.wait_for_flat("WB Web");
    t.send(b"\r");
    t.wait_for("╭ Notes");
    wait_pane(&mut t, "These notes are not updating right now: Your Claude plan is 82% used (5-hour limit), above the pause point of 80%. Smart features wait until it drops. Press s for the settings to change the pause point.");

    // The reason goes: so does the line, in the pane and in the header.
    lab.clear_the_plan();
    wait_pane_gone(&mut t, "not updating right now");
    t.wait_gone("smart features paused");
    wait_pane(&mut t, "notes are added as context to each session in a project");
    t.send(b"q");
    t.wait_exit();
}

/// Notes written in Markdown, the way the updater now writes them.
const FORMATTED: &str = "platoon keeps these up to date.

# Summary

A shop that sells **socks** online.

# Open questions

## Pricing

- Does VAT go in the basket total?
  - only for the EU
- A long question that keeps on going about shipping costs and whether they belong in the total the customer sees first

Run it with `cargo test`:

```
cargo test --release
```

# Decisions

";

#[test]
fn the_pane_draws_the_markdown_styled_with_hanging_bullets_and_scrolls_in_drawn_lines() {
    let lab = Lab::new("notes-md");
    let mut notes = FORMATTED.to_string();
    for n in 1..=40 {
        notes.push_str(&format!("- 2026-09-25: decision number {n}\n"));
    }
    lab.project(Some(&notes));
    // With NO_COLOR set (as the harness sets it) platoon draws without color; empty, it is off, and the cells show
    // the colors too.
    let mut env = lab.env();
    env.push(("NO_COLOR", String::new()));
    let mut t = Term::spawn(&[], &env, &lab.work, 30, 110);
    t.wait_for("enter open");
    lab.open_notes(&mut t);
    t.wait_for_all(&["Summary", "A shop that sells socks online.", "Pricing", "• Does VAT go in the basket total?", "◦ only for the EU"]);
    let screen = t.stable();
    // The marks are gone from the screen (the file keeps them).
    for gone in ["# Summary", "## Pricing", "**", "`cargo", "```"] {
        assert!(!screen.contains(gone), "{gone:?} is drawn:\n{screen}");
    }
    assert!(lab.notes().contains("# Summary") && lab.notes().contains("**socks**"), "the file is untouched");
    // Headings and bold are drawn as such, not only colored.
    let (r, c) = t.find("Summary").unwrap();
    let (bold, _, underline) = t.attrs_at(r, c);
    assert!(bold && underline, "a section heading is bold and underlined: {:?} at {r},{c}\n{screen}", t.attrs_at(r, c));
    let (r, c) = t.find("Pricing").unwrap();
    assert_eq!(t.attrs_at(r, c), (true, false, false), "a subheading is bold, without the underline");
    let (r, c) = t.find("socks online").unwrap();
    assert!(t.attrs_at(r, c).0, "**socks** is bold");
    assert!(!t.attrs_at(r, c - 2).0, "the words around it are not");
    // The long bullet wraps under its own text, not under the bullet.
    let (r, c) = t.find("• A long question").unwrap();
    let next: String = screen.lines().nth(r as usize + 1).unwrap().chars().skip(c as usize).take(4).collect();
    assert!(next.starts_with("  ") && next.chars().nth(2).is_some_and(|ch| ch.is_alphabetic()), "hanging indent: {next:?}\n{screen}");
    // Scrolling moves through the drawn lines. One line down: the top line goes, one more comes in at the bottom.
    assert!(!screen.contains("decision number 40") && !screen.contains("cargo test --release"));
    t.send(b"\x1b[B");
    t.wait_gone("platoon keeps these up to date.");
    // A page down: the code, with its words and without its marks.
    t.send(b"\x1b[6~");
    t.wait_for_all(&["Run it with cargo test:", "cargo test --release"]);
    // To the end: the last decision, and the top has gone.
    for _ in 0..8 {
        t.send(b"\x1b[6~");
    }
    t.wait_for("decision number 40");
    let screen = t.stable();
    assert!(!screen.contains("A shop that sells socks online."), "{screen}");
    t.send(b"\x1b[H"); // home: back to the top
    t.wait_for("A shop that sells socks online.");
    t.send(b"q");
    t.wait_exit();

    // With plain glyphs (the Linux console), bullets are plain too.
    let mut env = lab.env();
    env.push(("PLATOON_GLYPHS", "plain".into()));
    let mut t = Term::spawn(&[], &env, &lab.work, 30, 110);
    t.wait_for("enter open");
    lab.open_notes(&mut t);
    t.wait_for_all(&["- Does VAT go in the basket total?", "* only for the EU"]);
    assert!(!t.stable().contains('•'));
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn with_no_color_set_the_colors_go_but_bold_and_underline_stay() {
    let lab = Lab::new("notes-nocolor");
    lab.project(Some(FORMATTED));
    // The harness sets NO_COLOR=1: nothing is added to the environment here.
    let env = lab.env();
    assert!(!env.iter().any(|(k, _)| *k == "NO_COLOR"));
    let mut t = Term::spawn(&[], &env, &lab.work, 30, 110);
    t.wait_for("enter open");
    lab.open_notes(&mut t);
    t.wait_for_all(&["Summary", "A shop that sells socks online.", "Pricing"]);
    let screen = t.stable();
    let (r, c) = t.find("Summary").unwrap();
    assert_eq!(t.attrs_at(r, c), (true, false, true), "a section heading is bold and underlined\n{screen}");
    let (r, c) = t.find("Pricing").unwrap();
    assert_eq!(t.attrs_at(r, c), (true, false, false), "a subheading is bold\n{screen}");
    let (r, c) = t.find("socks online").unwrap();
    assert!(t.attrs_at(r, c).0 && !t.attrs_at(r, c - 2).0, "**socks** is bold, the words around it are not\n{screen}");
    // And no color reached the terminal: every cell is in its default colors.
    for row in 0..30 {
        for col in 0..110 {
            assert_eq!((t.fg_at(row, col), t.bg_at(row, col)), (vt100::Color::Default, vt100::Color::Default), "{row},{col}\n{screen}");
        }
    }
    let raw = String::from_utf8_lossy(&t.raw()).into_owned();
    assert!(!raw.contains("[38;") && !raw.contains("[48;"), "no color sequence was written");
    t.send(b"q");
    t.wait_exit();
}
