//! Selecting text in the attached view, end to end: the real binary in a real terminal, a `sh` standing in for
//! claude, SGR mouse events typed the way a terminal sends them. `Term::spawn` sets PLATOON_CLIPBOARD=osc52, so
//! the only copy made is the OSC 52 sequence in the output, which is read back here; the real clipboard is
//! never touched. Every directory platoon uses is a temp dir.

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
        ("NO_COLOR", String::new()), // the helper sets it, which makes platoon draw without any color
    ]
}

/// (column, row), 0-based, of the first `needle` on the screen.
fn find(screen: &str, needle: &str) -> (u16, u16) {
    screen
        .lines()
        .enumerate()
        .find_map(|(row, line)| line.find(needle).map(|byte| (line[..byte].chars().count() as u16, row as u16)))
        .unwrap_or_else(|| panic!("no {needle:?} on the screen:\n{screen}"))
}

/// A left-button event as a terminal sends it (SGR, 1-based): `b` is 0 for a press, 32 for a drag; `up` is the release.
fn sgr(b: u8, col: u16, row: u16, up: bool) -> Vec<u8> {
    format!("\x1b[<{b};{};{}{}", col + 1, row + 1, if up { 'm' } else { 'M' }).into_bytes()
}

/// The payloads of the OSC 52 sequences in what the terminal was sent, still base64.
fn osc52_payloads(raw: &[u8]) -> Vec<String> {
    let s = String::from_utf8_lossy(raw);
    s.split("\x1b]52;c;").skip(1).map(|rest| rest.split('\x07').next().unwrap().to_string()).collect()
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

#[test]
fn dragging_over_a_line_of_the_session_highlights_it_and_copies_it_once_and_a_double_click_copies_a_word() {
    let base = tmp("select-e2e");
    let env = env(&base);
    let work = base.join("work");
    fs::create_dir_all(&work).unwrap();
    let _end = EndHost(base.join("cache"));
    let mut t = Term::spawn(&[], &env, &work, 30, 100);
    t.wait_for("enter open");
    t.send(b"n");
    t.wait_for("pick a directory");
    t.send(work.to_string_lossy().as_bytes());
    t.send(b"\r");
    t.wait_for("back to list");

    t.send(b"echo hello-select-$((6*7))\r");
    t.wait_for("hello-select-42");
    let (col, row) = {
        let screen = t.text();
        // The command was typed with `$((6*7))`: only its output has the 42.
        find(&screen, "hello-select-42")
    };
    let len = "hello-select-42".chars().count() as u16;
    let plain_bg = t.bg_at(row, col + len + 3);

    // Press, a move on the way, release at the last character.
    t.send(&sgr(0, col, row, false));
    t.send(&sgr(32, col + 5, row, false));
    t.send(&sgr(32, col + len - 1, row, false));
    // While the button is down the text is highlighted (the background of those cells is not the plain one), nothing is copied yet.
    until("the selection to be highlighted", || t.bg_at(row, col + 3) != plain_bg);
    assert!(matches!(t.bg_at(row, col + 3), vt100::Color::Rgb(..)), "a color of the theme, not the terminal's own");
    assert_eq!(t.bg_at(row, col + len + 3), plain_bg, "past the drag it is the plain background");
    assert!(osc52_payloads(&t.raw()).is_empty(), "nothing is copied before the button is released");
    t.send(&sgr(0, col + len - 1, row, true));
    t.wait_for_flat("Copied 15 characters");
    assert_eq!(osc52_payloads(&t.raw()), ["aGVsbG8tc2VsZWN0LTQy"], "hello-select-42, once");
    assert_ne!(t.bg_at(row, col + 3), plain_bg, "and the highlight stays");

    // A click without moving copies nothing more and takes the highlight away.
    t.send(&sgr(0, col + 2, row, false));
    t.send(&sgr(0, col + 2, row, true));
    until("the highlight to go", || t.bg_at(row, col + 3) == plain_bg);
    assert_eq!(osc52_payloads(&t.raw()).len(), 1);

    // A double click on a word.
    t.send(b"echo alpha bravo-charlie\r");
    // The output line, not the echoed command (which has `echo` in front of it).
    let mut at = None;
    until("the output line", || {
        let screen = t.text();
        at = screen
            .lines()
            .position(|l| l.contains("alpha bravo-charlie") && !l.contains("echo"))
            .map(|row| (find(screen.lines().nth(row).unwrap(), "bravo-charlie").0, row as u16));
        at.is_some()
    });
    let (col, row) = at.unwrap();
    // One write, so that the two clicks reach platoon well inside its multi-click time however busy the machine is.
    let click = [sgr(0, col + 4, row, false), sgr(0, col + 4, row, true)].concat();
    t.send(&[click.clone(), click].concat());
    t.wait_for_flat("Copied 13 characters");
    assert_eq!(osc52_payloads(&t.raw()), ["aGVsbG8tc2VsZWN0LTQy", "YnJhdm8tY2hhcmxpZQ=="], "bravo-charlie, hyphen and all");

    // The way back to the list is untouched by any of this.
    t.send(&[0]); // ctrl+space
    t.wait_for("enter open");
    t.send(b"q");
    t.wait_exit();
}
