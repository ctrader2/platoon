//! `platoon` must not start until `platoon setup` has been run, and setup must be safely repeatable.

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
    process::Command,
    sync::mpsc::channel,
    thread,
    time::{Duration, Instant},
};

const BIN: &str = env!("CARGO_BIN_EXE_platoon");

fn dirs(name: &str) -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("platoon-gate-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let (claude, cache) = (base.join("claude"), base.join("cache"));
    fs::create_dir_all(&claude).unwrap();
    fs::create_dir_all(&cache).unwrap();
    (claude, cache)
}

fn platoon(claude: &PathBuf, cache: &PathBuf, args: &[&str]) -> std::process::Output {
    Command::new(BIN)
        .args(args)
        .env("CLAUDE_CONFIG_DIR", claude)
        .env("PLATOON_CACHE_DIR", cache)
        .env("NO_COLOR", "1")
        .env_remove("PLATOON_SKIP_SETUP_CHECK")
        .output()
        .unwrap()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn platoon_refuses_to_start_until_setup_is_done_and_says_what_is_missing() {
    let (claude, cache) = dirs("blocked");
    let out = platoon(&claude, &cache, &[]);
    assert_eq!(out.status.code(), Some(1));
    let err = text(&out.stderr);
    assert!(err.contains("one-time setup"), "{err}");
    assert_eq!(err.matches("[✗]").count(), 2, "both steps should show as not done:\n{err}");
    assert!(err.contains("Status line") && err.contains("Session guard") && err.contains("platoon setup"), "{err}");
}

#[test]
fn setup_lists_each_step_and_running_it_again_marks_everything_already_done() {
    let (claude, cache) = dirs("setup");
    let first = platoon(&claude, &cache, &["setup"]);
    assert!(first.status.success());
    let shown = text(&first.stdout);
    assert_eq!(shown.matches("[✓]").count(), 2, "{shown}");
    assert!(shown.contains("added") && shown.contains("Saved"), "{shown}");
    let settings = fs::read_to_string(claude.join("settings.json")).unwrap();

    let second = platoon(&claude, &cache, &["setup"]);
    assert!(second.status.success());
    let shown = text(&second.stdout);
    assert_eq!(shown.matches("[✓]").count(), 2, "{shown}");
    assert_eq!(shown.matches("already done").count(), 2, "{shown}");
    assert!(shown.contains("Nothing to change"), "{shown}");
    assert_eq!(fs::read_to_string(claude.join("settings.json")).unwrap(), settings, "a repeat run must not touch the file");
}

/// Runs `platoon` in a real terminal; returns the screen text once it shows `wait_for`, then quits it.
fn run_in_terminal(claude: &PathBuf, cache: &PathBuf, wait_for: &str) -> String {
    let pair = native_pty_system().openpty(PtySize { rows: 24, cols: 90, pixel_width: 0, pixel_height: 0 }).unwrap();
    let mut cmd = CommandBuilder::new(BIN);
    cmd.env("CLAUDE_CONFIG_DIR", claude);
    cmd.env("PLATOON_CACHE_DIR", cache);
    cmd.env("TERM", "xterm-256color");
    cmd.env_remove("PLATOON_SKIP_SETUP_CHECK");
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();
    let (tx, rx) = channel::<Vec<u8>>();
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut screen = vt100::Parser::new(24, 90, 0);
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if screen.screen().contents().contains(wait_for) {
            break;
        }
        if let Ok(b) = rx.recv_timeout(Duration::from_millis(100)) {
            screen.process(&b);
        }
    }
    let shown = screen.screen().contents();
    let _ = writer.write_all(b"q");
    let _ = child.kill();
    let _ = child.wait();
    shown
}

#[test]
fn once_setup_is_done_platoon_starts_and_undoing_a_step_blocks_it_again() {
    let (claude, cache) = dirs("start");
    assert!(platoon(&claude, &cache, &["setup"]).status.success());

    let screen = run_in_terminal(&claude, &cache, "No active Claude Code sessions");
    assert!(screen.contains("No active Claude Code sessions"), "the UI should have started:\n{screen}");

    // Someone removes the guard hook: platoon notices and refuses again.
    let path = claude.join("settings.json");
    let mut v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    v["hooks"] = serde_json::json!({});
    fs::write(&path, v.to_string()).unwrap();
    let out = platoon(&claude, &cache, &[]);
    assert_eq!(out.status.code(), Some(1));
    let err = text(&out.stderr);
    assert!(err.contains("[✓]") && err.contains("[✗]") && err.contains("Session guard"), "{err}");
}
