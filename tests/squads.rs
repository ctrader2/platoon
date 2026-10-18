//! The Squads tab end to end, in a real terminal with the real binary: `4` lists every squad with where it comes from
//! ("needs fixing" in the warning colour) and the picked one beside the list; `e` on a built-in writes its text to the
//! user's squads folder and opens it in the notes' editors, and a change makes it the user's; `d` deletes it after a yes
//! (kept as `.md.bak`) and the built-in is back; `n` makes a new squad from the template and opens it.
//!
//! The only "editor" is a small script in a folder of the test's own, which `PLATOON_EDITOR_PATH` makes the only place
//! platoon looks: no real editor is ever started, and every directory is a temp dir.

mod common;
use common::*;
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

/// Stands in for nano: draws on its terminal, waits for one line, and puts that line into the file (a line starting
/// with `s/` is a replacement, as in sed).
const FAKE_NANO: &str = r#"#!/bin/sh
f=""
for a in "$@"; do case "$a" in -*) ;; *) f="$a" ;; esac; done
echo "FAKE NANO EDITING $f"
read line
case "$line" in
  s/*) t=$(sed "$line" "$f") && printf '%s\n' "$t" > "$f" ;;
  *) printf '%s\n' "$line" >> "$f" ;;
esac
"#;

/// Catppuccin Mocha's warning yellow (the default theme).
const WARN: vt100::Color = vt100::Color::Rgb(0xf9, 0xe2, 0xaf);

struct Lab {
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
        };
        for d in [&lab.claude.join("sessions"), &lab.cache, &lab.config.join("squads"), &lab.work, &lab.bin] {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(lab.config.join("settings.json"), r#"{"auto_archive_days":0}"#).unwrap();
        let p = lab.bin.join("nano");
        fs::write(&p, FAKE_NANO).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
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
            // The helper may set it, which makes crossterm draw without any color; this test looks at a color.
            ("NO_COLOR", String::new()),
        ]
    }

    fn squad(&self, id: &str) -> PathBuf {
        self.config.join("squads").join(format!("{id}.md"))
    }
}

#[test]
fn the_tab_shows_every_squad_and_e_and_d_change_a_built_in_and_bring_it_back() {
    let lab = Lab::new("squads-tab");
    fs::write(lab.squad("sloppy"), "---\nname: Sloppy\ndescription: Half written\nmodle: opus\n---\n1. Go.\n").unwrap();
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 110);
    t.wait_for("enter open");
    t.send(b"4");
    t.wait_for_all(&[
        "4 Squads",
        "Feature",
        "Bug fix",
        "Docs",
        "built in",
        "Sloppy",
        "yours · needs fixing",
        "Verdict model",
        "n new squad",
    ]);
    t.stable();
    let (row, col) = t.find("needs fixing").unwrap();
    t.wait_fg(row, col, WARN);
    let (row, col) = t.find("built in").unwrap();
    assert_ne!(t.fg_at(row, col), WARN);

    // Down to Docs (the seventh), and e: its text is written to the squads folder and opens in the editor.
    t.send(b"jjjjjj");
    t.wait_for("Model          sonnet");
    t.send(b"e");
    t.wait_for("╭ Edit the squad with");
    t.send(b"\r");
    t.wait_for("FAKE NANO EDITING");
    assert!(fs::read_to_string(lab.squad("docs")).unwrap().contains("name: Docs"));
    t.send(b"s/^model: sonnet$/model: haiku/\r");
    t.wait_for("Saved Docs.");
    t.wait_for_all(&["yours, replaces the built-in", "Model          haiku"]);
    assert!(fs::read_to_string(lab.squad("docs")).unwrap().contains("model: haiku"));

    // d asks, says the built-in comes back, and keeps the file.
    t.send(b"d");
    t.wait_for_all(&["Delete the squad \"Docs\"?", "comes back", "y delete"]);
    t.send(b"y");
    t.wait_gone("Delete the squad");
    t.wait_for("Model          sonnet");
    assert!(!lab.squad("docs").exists() && lab.config.join("squads/docs.md.bak").exists());
    t.send(&[3]);
    t.wait_exit();
}

#[test]
fn n_makes_a_squad_from_the_template_and_opens_it_in_the_editor() {
    let lab = Lab::new("squads-new");
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 110);
    t.wait_for("enter open");
    t.send(b"4");
    t.wait_for("n new squad");
    t.send(b"n");
    t.wait_for("╭ New squad");
    t.send(b"Quick fix\r");
    t.wait_for("╭ Edit the squad with");
    let file = lab.squad("quick-fix");
    let text = fs::read_to_string(&file).unwrap();
    assert!(text.starts_with("---\nname: Quick fix\n") && text.contains("model: opus\nverdict_model: fable\nplan_first: true\n"), "{text}");
    t.send(b"\r");
    t.wait_for("FAKE NANO EDITING");
    t.send(b"2. Check it.\r");
    t.wait_for("Saved Quick fix.");
    t.wait_for_all(&["Quick fix", "yours", "2. Check it."]);
    t.send(&[3]);
    t.wait_exit();
}

#[test]
fn enter_on_an_open_squads_setting_changes_that_line_of_its_file() {
    let lab = Lab::new("squads-settings");
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 110);
    t.wait_for("enter open");
    t.send(b"4");
    t.wait_for("n new squad");
    // Docs is the seventh: open it, and the Model line is picked.
    t.send(b"jjjjjj");
    t.wait_for("Model          sonnet");
    t.send(b"\r");
    t.wait_for_all(&["enter changes", "enter change", "esc back"]);
    t.send(b"\r");
    t.wait_for_all(&["Model          haiku", "Docs: Model haiku. Yours now, in place of the built-in."]);
    let text = fs::read_to_string(lab.squad("docs")).unwrap();
    assert!(text.contains("\nmodel: haiku\n") && text.contains("name: Docs") && !text.contains("model: sonnet"), "{text}");
    // Down to Plans first: it says yes now, and the file's line with it.
    t.send(b"jj\r");
    t.wait_for("Plans first    yes");
    assert!(fs::read_to_string(lab.squad("docs")).unwrap().contains("\nplan_first: true\n"));
    t.send(&[3]);
    t.wait_exit();
}
