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
        ("PLATOON_CLAUDE_BIN", shell_claude(base).to_string_lossy().into()),
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
    t.wait_for("pick a folder");
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

/// The text an OSC 52 payload carries (standard base64 with padding).
fn unbase64(payload: &str) -> String {
    const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bytes = Vec::new();
    let (mut bits, mut n) = (0u32, 0);
    for c in payload.bytes().take_while(|&c| c != b'=') {
        bits = bits << 6 | ABC.iter().position(|&a| a == c).expect("a base64 character") as u32;
        n += 6;
        if n >= 8 {
            n -= 8;
            bytes.push((bits >> n) as u8);
        }
    }
    String::from_utf8(bytes).unwrap()
}

#[test]
fn a_drag_held_still_above_the_session_keeps_scrolling_back_and_letting_go_copies_all_of_it() {
    let base = tmp("select-scroll-e2e");
    let env = env(&base);
    let work = base.join("work");
    fs::create_dir_all(&work).unwrap();
    let _end = EndHost(base.join("cache"));
    let mut t = Term::spawn(&[], &env, &work, 30, 100);
    t.wait_for("enter open");
    t.send(b"n");
    t.wait_for("pick a folder");
    t.send(work.to_string_lossy().as_bytes());
    t.send(b"\r");
    t.wait_for("back to list");

    // Eighty lines, n1 to n80: far more than the session's 29 rows, so most go above the screen.
    t.send(b"seq 80 | sed 's/^/n/'\r");
    let line = |screen: &str, want: &str| screen.lines().position(|l| l.trim_end() == want);
    let mut row = None;
    until("the last lines on screen", || {
        row = line(&t.text(), "n70");
        row.is_some() && line(&t.text(), "n80").is_some()
    });
    let row = row.unwrap() as u16;
    assert!(line(&t.text(), "n1").is_none(), "the first lines are above the screen");

    // Press at the end of "n70", drag up to the top row, and hold still there: no more mouse events.
    t.send(&sgr(0, 2, row, false));
    t.send(&sgr(32, 2, row - 5, false));
    t.send(&sgr(32, 0, 0, false));
    until("the view to scroll back to the first line by itself", || line(&t.text(), "n1").is_some());
    assert!(osc52_payloads(&t.raw()).is_empty(), "nothing is copied before the button is released");
    t.send(&sgr(0, 0, 0, true));
    until("the copy", || !osc52_payloads(&t.raw()).is_empty());
    let copied = unbase64(&osc52_payloads(&t.raw())[0]);
    // Let go as soon as n1 is in view, the view may have gone on past it to what the shell printed before (its prompt,
    // the command), which depends on the shell; from n1 on it is exact.
    let all = (1..=70).map(|i| format!("n{i}")).collect::<Vec<_>>().join("\n");
    assert!(copied == all || copied.ends_with(&format!("\n{all}")), "every line from the first to n70, in order:\n{copied}");
    assert_eq!(osc52_payloads(&t.raw()).len(), 1, "copied once");

    t.send(&[0]); // ctrl+space
    t.wait_for("enter open");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn a_session_that_asked_for_the_mouse_selects_by_itself_and_what_it_copies_reaches_the_clipboard_once() {
    let base = tmp("select-fullscreen-e2e");
    let env = env(&base);
    let work = base.join("work");
    fs::create_dir_all(&work).unwrap();
    let _end = EndHost(base.join("cache"));
    let mut t = Term::spawn(&[], &env, &work, 30, 100);
    t.wait_for("enter open");
    t.send(b"n");
    t.wait_for("pick a folder");
    t.send(work.to_string_lossy().as_bytes());
    t.send(b"\r");
    t.wait_for("back to list");

    // The session copies as Claude Code's full screen mode does: an OSC 52 to its terminal, which is platoon. Platoon
    // copies it its own way, here the OSC 52 it sends its own terminal. (The typed command has the escape as `\033`
    // text, so only the shell's output carries one.)
    t.send(b"printf '\\033]52;c;ZnVsbHNjcmVlbi00Mg==\\007'; echo copied-$((6*7))\r");
    t.wait_for_all(&["copied-42", "Copied 13 characters"]);
    assert_eq!(osc52_payloads(&t.raw()), ["ZnVsbHNjcmVlbi00Mg=="], "fullscreen-42, once");

    // Ask for button events in SGR, then print every byte that arrives, escapes made visible. Raw mode so that
    // nothing waits for a newline and the terminal echoes nothing itself; only once "go-42" shows is it raw.
    t.send(b"printf '\\033[?1002h\\033[?1006h'; stty raw -echo; printf 'go-%s\\r\\n' $((6*7)); cat -v\r");
    t.wait_for("go-42");
    let plain_bg = t.bg_at(2, 30);
    // Press, a drag along the row, a drag down onto the status bar (the last of the 30 rows), let go there.
    t.send(&sgr(0, 19, 7, false));
    t.send(&sgr(32, 24, 7, false));
    t.send(&sgr(32, 5, 29, false));
    t.send(&sgr(0, 5, 29, true));
    // The status bar is below the session's 29 rows: reported on its last one.
    t.wait_for_flat("^[[<0;20;8M^[[<32;25;8M^[[<32;6;29M^[[<0;6;29m");
    assert_eq!(t.bg_at(7, 22), plain_bg, "platoon highlights nothing of its own");
    assert_eq!(osc52_payloads(&t.raw()).len(), 1, "and copies nothing of its own");

    // A window connecting now (a reconnect is one) starts from the host's snapshot of the screen, which has no copy in
    // it to make again. Asked here as a window asks, at the same size as platoon's (29 rows under 30, no frame).
    let (_, sock) = find_host(&base.join("cache")).expect("the session's host");
    let mut window = std::os::unix::net::UnixStream::connect(&sock).unwrap();
    write_frame(&mut window, b'R', &[0, 29, 0, 100]).unwrap();
    window.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
    let (kind, snapshot) = read_frame(&mut window).expect("the snapshot");
    assert_eq!(kind, b'O');
    let snapshot = String::from_utf8_lossy(&snapshot);
    assert!(snapshot.contains("copied-42") && !snapshot.contains("\x1b]52"), "{snapshot:?}");
    let _ = window.shutdown(std::net::Shutdown::Both);
    t.send(b"end-of-test");
    t.wait_for("end-of-test");
    assert_eq!(osc52_payloads(&t.raw()).len(), 1, "copied once");

    t.send(&[0]); // ctrl+space
    t.wait_for("enter open");
    t.send(b"q");
    t.wait_exit();
}
