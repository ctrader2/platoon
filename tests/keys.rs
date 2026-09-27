//! keys.json end to end, in a real terminal with the real binary: the hints show the configured back and quit keys, the
//! back key typed as its terminal bytes goes back (and Ctrl+Space no longer does), and a bad entry is said on screen.
//! Every directory is a temp dir and no session is started, so there is nothing to clean up afterwards.

mod common;
use common::*;
use std::{fs, thread, time::Duration};

fn env(base: &std::path::Path) -> Vec<(&'static str, String)> {
    let dir = |name: &str| {
        let d = base.join(name);
        fs::create_dir_all(&d).unwrap();
        d.to_string_lossy().into_owned()
    };
    fs::create_dir_all(base.join("claude/sessions")).unwrap();
    vec![
        ("CLAUDE_CONFIG_DIR", base.join("claude").to_string_lossy().into()),
        ("PLATOON_CACHE_DIR", dir("cache")),
        ("PLATOON_CONFIG_DIR", dir("config")),
        ("PLATOON_SKIP_SETUP_CHECK", "1".into()),
    ]
}

#[test]
fn the_configured_back_and_quit_keys_work_and_the_hints_show_them() {
    let base = tmp("keys-e2e");
    let env = env(&base);
    fs::write(base.join("config/keys.json"), r#"{ "back": "alt+x", "quit": "w" }"#).unwrap();
    let mut t = Term::spawn(&[], &env, &base, 30, 120);
    t.wait_for_all(&["enter open", "w quit"]);
    assert!(!t.text().contains("q quit"), "{}", t.text());

    t.send(b"s");
    t.wait_for_all(&["Stop idle sessions after", "alt+x back"]);
    // Ctrl+Space (a NUL byte) is nothing now: the settings stay.
    t.send(&[0]);
    thread::sleep(Duration::from_millis(400));
    assert!(t.text().contains("Stop idle sessions after"), "{}", t.text());
    // Alt+x, as a terminal sends it (esc, then x, together), closes them.
    t.send(b"\x1bx");
    t.wait_gone("Stop idle sessions after");
    t.wait_for("w quit");
    // q is nothing; the quit key quits.
    t.send(b"q");
    thread::sleep(Duration::from_millis(300));
    assert!(t.text().contains("w quit"), "still running: {}", t.text());
    t.send(b"w");
    assert_eq!(t.wait_exit(), 0);
}

#[test]
fn a_bad_entry_is_said_on_screen_and_ctrl_space_stays_the_back_key() {
    let base = tmp("keys-bad-e2e");
    let env = env(&base);
    fs::write(base.join("config/keys.json"), r#"{ "back": "x" }"#).unwrap();
    let mut t = Term::spawn(&[], &env, &base, 30, 160);
    t.wait_for_all(&["keys.json: \"back\" is \"x\"", "Using ctrl+space."]);
    t.send(b"s");
    t.wait_for_all(&["Stop idle sessions after", "ctrl+space back"]);
    t.send(&[0]);
    t.wait_gone("Stop idle sessions after");
    t.wait_for("q quit");
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);
    assert_eq!(fs::read_to_string(base.join("config/keys.json")).unwrap(), r#"{ "back": "x" }"#, "never written");
}
