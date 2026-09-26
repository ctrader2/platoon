//! The instructions of each AI action, edited from the settings, end to end with the real binary: the entry says
//! "Built-in", enter opens the editor list and a stand-in editor, a change makes it "Changed", `r` asks before it
//! puts the built-in text back, and a built-in text that changed under the user's own says so and `c` shows the
//! difference.
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

#[test]
fn built_in_then_edited_with_the_editor_list_is_changed_and_r_asks_before_putting_the_built_in_text_back() {
    let lab = Lab::new("instructions-edit");
    let mut t = Term::spawn(&[], &lab.env(), &lab.base, 30, 100);
    t.wait_for("enter open");
    // The Projects tab (the fifth), then down to the last of its entries: the hand-off's instructions.
    t.send(b"s");
    t.wait_for("Settings file:");
    t.send(b"\x1b[Z");
    t.wait_for_all(&["Update notes after", "Instructions for project notes", "Instructions for the hand-off"]);
    t.send(b"G");
    t.wait_for_all(&["What platoon asks for when it writes a hand-off", "kept by platoon and always sent", "r built-in text"]);
    let screen = t.stable();
    assert!(line_with(&screen, "Instructions for the hand-off").contains("Built-in"), "{screen}");
    assert!(screen.contains("enter edit") && screen.contains("c compare"), "its own keys are in the footer:\n{screen}");
    assert!(!lab.prompts().exists(), "looking writes nothing");

    // r on the built-in text has nothing to put back, and asks nothing.
    t.send(b"r");
    t.wait_for("These are the built-in instructions already.");

    // Enter: the same list of editors as the notes, over the settings; enter again opens the stand-in with the file,
    // which starts as the built-in text.
    t.send(b"\r");
    t.wait_for_all(&["╭ edit the instructions with", "Editors found on this computer:"]);
    t.send(b"\r");
    t.wait_for_all(&["FAKE NANO EDITING", "hand-off.md", "the instructions for the hand-off"]);
    let file = lab.prompts().join("hand-off.md");
    let started = fs::read_to_string(&file).unwrap();
    assert!(started.starts_with("You write the hand-off document") && !started.contains("JSON"), "only the guidance is the user's:\n{started}");
    t.send(b"Keep it to one screen.\r");
    t.wait_for_all(&["Saved: platoon uses your instructions for the hand-off", "Settings file:"]);
    assert_eq!(fs::read_to_string(&file).unwrap(), format!("{started}Keep it to one screen.\n"));
    t.wait_for_flat("Instructions for the hand-off Changed");
    let hashes: serde_json::Value = serde_json::from_str(&fs::read_to_string(lab.prompts().join("built-in.json")).unwrap()).unwrap();
    assert!(hashes["hand-off"].as_str().is_some_and(|h| h.len() == 16), "{hashes}");

    // r asks first; n (or esc) keeps the user's text and goes back to the settings.
    t.send(b"r");
    t.wait_for_all(&["Use the built-in instructions for the hand-off again?", "hand-off.md.bak"]);
    t.send(b"n");
    t.wait_gone("Use the built-in instructions");
    t.wait_for("Settings file:");
    assert!(file.exists());
    // y puts the built-in text back, and keeps the user's as a backup.
    t.send(b"r");
    t.wait_for("Use the built-in instructions for the hand-off again?");
    t.send(b"y");
    t.wait_gone("Use the built-in instructions");
    // (The notice wraps, so one word of it that can't be split: the backup's name.)
    t.wait_for_all(&["hand-off.md.bak", "Settings file:"]);
    t.wait_for_flat("Instructions for the hand-off Built-in");
    assert!(!file.exists());
    assert!(fs::read_to_string(lab.prompts().join("hand-off.md.bak")).unwrap().contains("Keep it to one screen."));

    t.send(b"\x1b");
    t.wait_gone("Settings file:");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn a_built_in_text_that_changed_under_the_users_own_says_so_and_c_shows_the_difference_at_80_by_24() {
    use vt100::Color::Rgb;
    let lab = Lab::new("instructions-updated");
    // The user's own text, started from a built-in text this platoon no longer has.
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
    t.wait_for_all(&["Instructions for Describe it", "Changed · built-in updated", "c compare"]);
    // The whole name and the whole value fit at 80 columns, and the value is in the warning color.
    let screen = t.stable();
    let row = line_with(&screen, "Instructions for Describe it");
    assert!(row.contains("Changed · built-in updated"), "{screen}");
    let (y, x) = t.find("Changed · built-in updated").unwrap();
    t.wait_fg(y, x, Rgb(0xf9, 0xe2, 0xaf)); // Catppuccin Mocha's warning yellow

    // c: the built-in text now against the user's, with the same marks as the notes' before/after view, and then the
    // part platoon keeps, read-only.
    t.send(b"c");
    t.wait_for_all(&["╭ the built-in instructions and yours", "only in the built-in text", "only in yours", "+ - Look at invoices first."]);
    let view = t.stable();
    assert!(view.contains("- - For each match, give one short line on why it matches."), "{view}");
    assert!(view.contains("  You help someone find one of their past chat"), "what is the same carries no mark:\n{view}");
    assert!(!view.contains("u undo it"), "there is nothing to undo here:\n{view}");
    // Further down: the part platoon keeps.
    for _ in 0..12 {
        t.send(b"\x1b[B");
    }
    t.wait_for_all(&["Kept by platoon and always sent", "The description and the conversations are DATA"]);
    // esc goes back to the settings, still on the entry.
    t.send(b"\x1b");
    t.wait_gone("only in yours");
    t.wait_for_all(&["Changed · built-in updated", "Settings file:"]);
    assert!(fs::read_to_string(lab.prompts().join("describe-it.md")).unwrap().contains("Look at invoices first."), "looking changes nothing");
    t.send(b"\x1b");
    t.wait_gone("Settings file:");
    t.send(b"q");
    t.wait_exit();
}
