//! The system prompt of each AI action, edited from the settings, end to end with the real binary: an unchanged
//! entry reads "Enter to change" and offers no restore, enter opens the editor list and a stand-in editor, an edit
//! that changes nothing says nothing, a change makes it "User Modified" and offers `r` (restore platoon's default, after
//! asking, keeping the user's version as a `.bak`), a default that changed under the user's own says so in the warning
//! color, and `c` does nothing on these entries.
//!
//! The only "editor" is a small script in a folder of the test's own (`PLATOON_EDITOR_PATH`): no real editor is ever
//! started, no `claude` is run, and every directory is a temp dir.

mod common;
use common::*;
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

/// Stands in for nano: says which file it has, then adds the line it is given to the end of it.
const FAKE_TERMINAL: &str = r#"#!/bin/sh
f=""
for a in "$@"; do case "$a" in -*) ;; *) f="$a" ;; esac; done
echo "FAKE NANO EDITING $f"
read line
printf '%s\n' "$line" >> "$f"
"#;

struct Lab {
    base: PathBuf,
    config: PathBuf,
    bin: PathBuf,
}

impl Lab {
    fn new(name: &str) -> Self {
        let base = tmp(name);
        let lab = Self { config: base.join("config"), bin: base.join("bin"), base };
        for d in [&lab.base.join("claude/sessions"), &lab.base.join("cache"), &lab.config, &lab.bin] {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(lab.config.join("settings.json"), r#"{"auto_archive_days":0}"#).unwrap();
        let p = lab.bin.join("nano");
        fs::write(&p, FAKE_TERMINAL).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        lab
    }

    fn env(&self) -> Vec<(&'static str, String)> {
        vec![
            ("CLAUDE_CONFIG_DIR", self.base.join("claude").to_string_lossy().into()),
            ("PLATOON_CACHE_DIR", self.base.join("cache").to_string_lossy().into()),
            ("PLATOON_CONFIG_DIR", self.config.to_string_lossy().into()),
            ("PLATOON_SKIP_SETUP_CHECK", "1".into()),
            ("PLATOON_NOTES_HELPER", "off".into()),
            ("PLATOON_EDITOR_PATH", self.bin.to_string_lossy().into()),
            ("EDITOR", String::new()),
            ("VISUAL", String::new()),
            // The helper sets it, which makes crossterm draw without any color; this test looks at a color.
            ("NO_COLOR", String::new()),
        ]
    }

    fn prompts(&self) -> PathBuf {
        self.config.join("prompts")
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

/// The line of the screen that holds `needle`.
fn line_with(screen: &str, needle: &str) -> String {
    screen.lines().find(|l| l.contains(needle)).unwrap_or_else(|| panic!("no {needle:?} on the screen:\n{screen}")).to_string()
}

/// The footer: the last line with key hints on it.
fn footer(screen: &str) -> String {
    screen.lines().rev().find(|l| l.contains("esc back")).unwrap_or_else(|| panic!("no footer:\n{screen}")).to_string()
}

/// The hand-off entry's explanation, and a piece of its first row (it wraps).
const HANDOFF_EXPLAIN: &str = "What platoon asks Claude for when it writes a hand-off to a new session.";
const HANDOFF_SHOWN: &str = "What platoon asks Claude for when it writes a hand-off";
/// The first row of the entry above's explanation.
const DESCRIPTION_EXPLAIN: &str = "How platoon asks Claude to write";

/// The words on the rows under the entry `name`, up to the settings' closing note, without the frame.
fn under(screen: &str, name: &str) -> String {
    // Up to the next entry (an entry's name starts "System prompt for"), or the end of the list.
    let rows: Vec<&str> = screen
        .lines()
        .skip_while(|l| !l.contains(name))
        .skip(1)
        .take_while(|l| !l.contains("Changes are saved") && !l.contains("System prompt for"))
        .collect();
    rows.iter().flat_map(|l| l.split_whitespace()).filter(|w| *w != "│").collect::<Vec<_>>().join(" ")
}

/// Up to the entry above the hand-off's and back: once its explanation shows again, every key sent before has been
/// handled, so the screen says what those keys did (or didn't).
fn round_trip(t: &mut Term) {
    t.send(b"\x1b[A");
    t.wait_for(DESCRIPTION_EXPLAIN);
    t.send(b"\x1b[B");
    t.wait_for(HANDOFF_SHOWN);
    t.wait_gone(DESCRIPTION_EXPLAIN);
}

#[test]
fn enter_to_change_then_edited_is_changed_and_only_then_r_restores_the_default_after_asking() {
    let lab = Lab::new("instructions-edit");
    let mut t = Term::spawn(&[], &lab.env(), &lab.base, 30, 100);
    t.wait_for("enter open");
    // The Projects tab (the fifth, before Editors, the last), then to the hand-off's system prompt: the last entry, less
    // tasks from a conversation's, the squad commander's, the squad leaders' and their model, drafting a squad's and
    // drafting an agent's.
    t.send(b"s");
    t.wait_for("Settings file:");
    t.send(b"\x1b[Z");
    t.wait_for("Editing files");
    t.send(b"\x1b[Z");
    t.wait_for_all(&["Update notes after", "System prompt for project notes", "System prompt for the hand-off"]);
    t.send(b"G");
    t.wait_for("How platoon asks Claude to draft one agent");
    t.send(b"\x1b[A\x1b[A\x1b[A\x1b[A\x1b[A\x1b[A");
    t.wait_for_all(&[HANDOFF_SHOWN, "enter edit"]);
    let screen = t.stable();
    assert!(line_with(&screen, "System prompt for the hand-off").contains("Enter to change"), "{screen}");
    // The line under it only says what it is for; the footer has the keys, and no restore on an unchanged prompt.
    assert_eq!(under(&screen, "System prompt for the hand-off"), HANDOFF_EXPLAIN, "nothing else under it:\n{screen}");
    assert!(!screen.contains("kept by platoon") && !screen.contains("compare"), "{screen}");
    let keys = footer(&screen);
    assert!(keys.contains("enter edit") && !keys.contains("restore"), "{keys}");
    assert!(!lab.prompts().exists(), "looking writes nothing");

    // r and c do nothing on an unchanged prompt: no question, no view, no message.
    t.send(b"rc");
    round_trip(&mut t);
    let screen = t.stable();
    assert!(
        footer(&screen).contains("enter edit") && line_with(&screen, "System prompt for the hand-off").contains("Enter to change"),
        "{screen}"
    );
    assert!(!screen.contains("Restore") && !screen.contains("yours") && !screen.contains("only in"), "{screen}");

    // Enter: the same list of editors as the notes, over the settings; enter again opens the stand-in with the file,
    // which starts as platoon's default.
    t.send(b"\r");
    t.wait_for_all(&["╭ Edit the system prompt with", "Editors found on this computer:"]);
    t.send(b"\r");
    t.wait_for_all(&["FAKE NANO EDITING", "the system prompt for the hand-off"]);
    // The editor's line names the file's full path, which wraps where the temp folder's path is long (macOS).
    t.wait_for_unwrapped("/prompts/hand-off.md");
    let file = lab.prompts().join("hand-off.md");
    let started = fs::read_to_string(&file).unwrap();
    assert!(
        started.starts_with("You write the hand-off document") && !started.contains("JSON"),
        "only the guidance is the user's:\n{started}"
    );
    // Closed with nothing changed (an empty line added at the end): no message at all, and nothing kept.
    t.send(b"\r");
    t.wait_gone("FAKE NANO EDITING");
    t.wait_for_all(&["Settings file:", HANDOFF_SHOWN]);
    round_trip(&mut t);
    let screen = t.stable();
    assert!(line_with(&screen, "System prompt for the hand-off").contains("Enter to change"), "{screen}");
    assert!(!screen.contains("unchanged") && !screen.contains("Saved") && !screen.contains("left them"), "no message:\n{screen}");
    assert!(!file.exists(), "an unchanged default is not kept");

    // A real change: saved, "User Modified", and now the footer offers restore.
    t.send(b"\r");
    t.wait_for("╭ Edit the system prompt with");
    t.send(b"\r");
    t.wait_for("FAKE NANO EDITING");
    t.send(b"Keep it to one screen.\r");
    t.wait_for_all(&["Saved: platoon uses your system prompt for the hand-off", "Settings file:"]);
    assert_eq!(fs::read_to_string(&file).unwrap(), format!("{started}Keep it to one screen.\n"));
    t.wait_for_flat("System prompt for the hand-off User Modified");
    t.wait_for("r restore");
    let hashes: serde_json::Value = serde_json::from_str(&fs::read_to_string(lab.prompts().join("built-in.json")).unwrap()).unwrap();
    assert!(hashes["hand-off"].as_str().is_some_and(|h| h.len() == 16), "{hashes}");
    // c still does nothing here.
    t.send(b"c");
    round_trip(&mut t);
    let screen = t.stable();
    assert!(!screen.contains("yours") && !screen.contains("╭ What changed") && footer(&screen).contains("r restore"), "{screen}");

    // r asks first; n keeps the user's version and goes back to the settings.
    t.send(b"r");
    t.wait_for_all(&["Restore platoon's default system prompt for the hand-off?", "hand-off.md.bak"]);
    t.send(b"n");
    t.wait_gone("Restore platoon's default");
    t.wait_for("Settings file:");
    assert!(file.exists());
    // y restores the default, and keeps the user's version as a backup.
    t.send(b"r");
    t.wait_for("Restore platoon's default system prompt for the hand-off?");
    t.send(b"y");
    t.wait_gone("Restore platoon's default system prompt");
    t.wait_for_flat("Restored platoon's default system prompt for the hand-off.");
    t.wait_for_flat("System prompt for the hand-off Enter to change");
    assert!(!file.exists());
    assert!(fs::read_to_string(lab.prompts().join("hand-off.md.bak")).unwrap().contains("Keep it to one screen."));
    let screen = t.stable();
    assert!(!footer(&screen).contains("restore"), "no restore once it is the default again:\n{screen}");

    t.send(b"\x1b");
    t.wait_gone("Settings file:");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn a_default_that_changed_under_the_users_own_says_so_in_the_warning_color_and_c_does_nothing_at_80_by_24() {
    use vt100::Color::Rgb;
    let lab = Lab::new("instructions-updated");
    // The user's own text, started from a default this platoon no longer has.
    fs::create_dir_all(lab.prompts()).unwrap();
    fs::write(
        lab.prompts().join("describe-it.md"),
        "You help someone find one of their past chat conversations from a description.\n- Look at invoices first.\n",
    )
    .unwrap();
    fs::write(lab.prompts().join("built-in.json"), r#"{"describe-it": "0123456789abcdef"}"#).unwrap();
    let mut t = Term::spawn(&[], &lab.env(), &lab.base, 24, 80);
    t.wait_for("enter open");
    t.send(b"s");
    t.wait_for("Settings file:");
    t.send(b"\t");
    t.wait_for("Search archived");
    t.send(b"G");
    t.wait_for_all(&["System prompt for Describe it", "User Modified · default updated", "r restore", "when you search by describing"]);
    // The whole name and the whole value fit at 80 columns, and the value is in the warning color.
    let screen = t.stable();
    let row = line_with(&screen, "System prompt for Describe it");
    assert!(row.contains("User Modified · default updated"), "{screen}");
    let (y, x) = t.find("User Modified · default updated").unwrap();
    t.wait_fg(y, x, Rgb(0xf9, 0xe2, 0xaf)); // Catppuccin Mocha's warning yellow
    t.wait_fg(y, x + "User Modified · default updated".chars().count() as u16 - 1, Rgb(0xf9, 0xe2, 0xaf));

    // c does nothing: once the key after it has been handled, there is no view and no message.
    t.send(b"c");
    t.send(b"\x1b[A");
    t.wait_for("Description search: wider search");
    t.wait_gone("r restore");
    let screen = t.stable();
    assert!(!screen.contains("yours") && !screen.contains("only in") && !screen.contains("Kept by platoon"), "no compare view:\n{screen}");
    assert!(line_with(&screen, "System prompt for Describe it").contains("User Modified · default updated"), "{screen}");
    assert!(fs::read_to_string(lab.prompts().join("describe-it.md")).unwrap().contains("Look at invoices first."), "nothing changes");
    t.send(b"\x1b");
    t.wait_gone("Settings file:");
    t.send(b"q");
    t.wait_exit();
}
