//! The smallest terminal platoon draws in (60x14), end to end: the real binary in a real terminal that is resized
//! below it and back. Below it only a message shows and no key but quit acts; big enough again, the screen is back as
//! it was. Every directory platoon touches is a temp dir; no `claude` is started.

mod common;
use common::*;
use std::fs;

fn env(base: &std::path::Path) -> Vec<(&'static str, String)> {
    let claude = base.join("claude");
    let project = claude.join("projects").join("-work");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(base.join("config")).unwrap();
    // A fixed old date: this file is about the screen, not auto-archive.
    fs::write(base.join("config/settings.json"), r#"{"auto_archive_days":0}"#).unwrap();
    for (id, prompt) in [("aaaa-0001", "fix the login page"), ("bbbb-0002", "write the release notes")] {
        let line = format!(
            r#"{{"type":"user","cwd":"/srv/work","gitBranch":"main","timestamp":"2026-09-19T10:00:00.000Z","message":{{"role":"user","content":"{prompt}"}}}}"#
        );
        fs::write(project.join(format!("{id}.jsonl")), format!("{line}\n")).unwrap();
    }
    vec![
        ("CLAUDE_CONFIG_DIR", claude.to_string_lossy().into()),
        ("PLATOON_CACHE_DIR", base.join("cache").to_string_lossy().into()),
        ("PLATOON_CONFIG_DIR", base.join("config").to_string_lossy().into()),
        // Nothing here may start a session: a stand-in that isn't claude, in case a key slipped through.
        ("PLATOON_CLAUDE_BIN", "/usr/bin/false".into()), // where macOS has it too
        ("PLATOON_SKIP_SETUP_CHECK", "1".into()),
    ]
}

#[test]
fn below_sixty_by_fourteen_a_message_shows_keys_do_nothing_and_resizing_back_brings_the_screen_back() {
    let base = tmp("small-resize");
    let mut t = Term::spawn(&[], &env(&base), &base, 24, 80);
    t.wait_for_all(&["fix the login page", "write the release notes", "? help"]);

    // Smaller than the minimum: the message, centered, and nothing else.
    t.resize(12, 40);
    t.wait_for_flat("platoon needs at least 60×14 — this window is 40×12");
    t.wait_for_flat("Make the window bigger, or press q to quit.");
    let screen = t.stable();
    assert!(!screen.contains("1 Sessions") && !screen.contains("fix the login page"), "{screen}");
    // Keys do nothing while it is too small: no picker, no tab change, no help, no settings.
    t.send(b"n2?s");
    t.send(b"\x1b[B");

    // Big enough again: everything is back, exactly as it was (the first row still selected, the Sessions tab).
    t.resize(24, 80);
    t.wait_for_all(&["1 Sessions", "fix the login page", "? help"]);
    let screen = t.stable();
    assert!(!screen.contains("platoon needs"), "{screen}");
    assert!(
        !screen.contains("pick a folder")
            && !screen.contains("Settings file")
            && !screen.contains("On this screen")
            && !screen.contains("No projects yet"),
        "no key acted:\n{screen}"
    );
    let first = screen.lines().position(|l| l.contains("fix the login page") || l.contains("write the release notes")).unwrap();
    assert!(screen.lines().nth(first - 1).unwrap().contains('▎'), "the first row is still the selected one:\n{screen}");

    // At exactly the minimum it draws the screen; one row less and it is the message again.
    t.resize(14, 60);
    t.wait_for_all(&["1 Sessions", "? help"]);
    t.resize(13, 60);
    t.wait_for_flat("this window is 60×13");
    // q quits even while too small (the message says so).
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);
}

#[test]
fn the_help_opens_with_question_mark_and_closes_with_esc_in_a_real_terminal() {
    let base = tmp("small-help");
    let mut t = Term::spawn(&[], &env(&base), &base, 24, 80);
    t.wait_for_all(&["fix the login page", "? help"]);
    t.send(b"?");
    t.wait_for_all(&["╭ Keys", "On this screen", "hand-off", "esc close"]);
    // Esc, then wait for it to go before anything else (a key right after Esc would read as Alt+key).
    t.send(b"\x1b");
    t.wait_gone("On this screen");
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);
}
