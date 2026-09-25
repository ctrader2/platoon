//! The mouse wheel in the attached view, end to end: the real binary in a real terminal, a `sh` standing in
//! for claude. One test has the shell ask for mouse reports (as Claude's full screen mode does) and then show
//! what reaches it; the other leaves the mouse alone and checks that the wheel still scrolls platoon's own
//! screen. Every directory platoon uses is a temp dir; the real clipboard is never touched.

mod common;
use common::*;
use std::fs;

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
        ("PLATOON_CLAUDE_BIN", "sh".into()),
        ("PLATOON_SKIP_SETUP_CHECK", "1".into()),
        ("NO_COLOR", String::new()), // the helper sets it, which makes crossterm draw without any color
    ]
}

/// A wheel event as a terminal sends it (SGR, 1-based): 64 is up, 65 is down.
fn wheel(up: bool, col: u16, row: u16) -> Vec<u8> {
    format!("\x1b[<{};{col};{row}M", if up { 64 } else { 65 }).into_bytes()
}

/// Ends the session host the test started, even when the test fails half way.
struct EndHost(std::path::PathBuf);

impl Drop for EndHost {
    fn drop(&mut self) {
        if let Some((pid, _)) = find_host(&self.0) {
            unsafe { libc::kill(pid as i32, libc::SIGKILL) }; // the sh this test started (an interactive sh ignores SIGTERM)
            until("the shell to exit", || !alive(pid));
        }
    }
}

/// Starts a new session in a temp directory and attaches to it.
fn attach(name: &str) -> (Term, EndHost) {
    let base = tmp(name);
    let env = env(&base);
    let work = base.join("work");
    fs::create_dir_all(&work).unwrap();
    let end = EndHost(base.join("cache"));
    let mut t = Term::spawn(&[], &env, &work, 30, 100);
    t.wait_for("enter open");
    t.send(b"n");
    t.wait_for("pick a directory");
    t.send(work.to_string_lossy().as_bytes());
    t.send(b"\r");
    t.wait_for("back to list");
    (t, end)
}

#[test]
fn a_session_that_asked_for_mouse_reports_hears_the_wheel() {
    let (mut t, _end) = attach("wheel-e2e-tracking");
    // Ask for mouse reports (button events, SGR encoding), then print every byte that arrives, escapes made
    // visible. Raw mode so that nothing waits for a newline and the terminal echoes nothing itself.
    // Only once "go-42" shows is the terminal raw, so nothing sent before that could be echoed by it.
    t.send(b"printf '\\033[?1000h\\033[?1006h'; stty raw -echo; printf 'go-%s\\r\\n' $((6*7)); cat -v\r");
    t.wait_for("go-42");

    // The pointer at screen column 20, row 8 (1-based); the session's screen starts inside a one-cell frame,
    // so that is its column 19, row 7.
    t.send(&wheel(true, 20, 8));
    t.wait_for("^[[<64;19;7M");
    t.send(&wheel(false, 30, 4));
    t.wait_for("^[[<65;29;3M");

    // Over the frame it is not sent: the first row of the screen is the frame.
    t.send(&wheel(true, 20, 1));
    // Then a marker turn: once it shows, everything sent before it has been through the shell.
    t.send(&wheel(true, 2, 2));
    t.wait_for("^[[<64;1;1M");
    assert_eq!(t.text().matches("^[[<").count(), 3, "nothing for the frame:\n{}", t.text());

    // The way back to the list is untouched.
    t.send(&[0]); // ctrl+space
    t.wait_for("enter open");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn a_session_that_did_not_ask_keeps_the_wheel_for_scrolling_platoons_own_screen() {
    let (mut t, _end) = attach("wheel-e2e-plain");
    t.send(b"seq 1000 1099; echo done-$((6*7))\r");
    t.wait_for("done-42");
    let numbers = |screen: &str| -> Vec<u32> { screen.lines().filter_map(|l| l.trim().trim_matches('\u{2502}').trim().parse().ok()).collect() };
    let before = numbers(&t.text());
    assert!(!before.is_empty(), "the numbers are on the screen:\n{}", t.text());
    let top = *before.iter().min().unwrap();
    assert!(top > 1000, "the first numbers have already scrolled off:\n{}", t.text());

    t.send(&wheel(true, 20, 8));
    until("the screen to show older lines", || numbers(&t.text()).iter().min().is_some_and(|m| *m < top));
    assert!(!t.stable().contains("^["), "and nothing was typed into the shell");

    t.send(&[0]); // ctrl+space
    t.wait_for("enter open");
    t.send(b"q");
    t.wait_exit();
}
