//! The mouse wheel in the attached view, at the level of the app: real wheel events in, the bytes that reach
//! the (fake) session and the scrollback of platoon's own screen out.

use crate::app::{App, Mode};
use crate::client::Client;
use crate::testkit::World;
use ratatui::crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
use std::{io::Read, os::unix::net::UnixStream, time::Duration};

/// The session's screen is 23 rows by the whole 80 columns at (0, 0) of an 80x24 terminal: no frame around it, the
/// status bar under it.
const ROWS: u16 = 23;
const COLS: u16 = 80;

struct Rig {
    _w: World,
    app: App,
    /// The host end of the connection: what the client sends to the session arrives here.
    host: UnixStream,
}

/// An attached session that has printed 60 lines and then `setup` (the escape sequences a program sends to ask
/// for mouse reports, or nothing). `paused` makes it a copy that must not be written to.
fn rig(setup: &str, paused: bool) -> Rig {
    let mut w = World::new("wheel");
    let pid = if paused {
        w.transcript("aaa", "/tmp", "a prompt", 5);
        let pid = w.live("aaa", "/tmp", 1, "idle");
        crate::guard::mark_stale(&w.cache, pid, 1);
        pid
    } else {
        4242
    };
    let mut app = w.app();
    let (client, host) = Client::for_test(ROWS, COLS);
    host.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    let text = (0..60).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\r\n");
    client.parser.lock().unwrap().process(format!("{text}{setup}").as_bytes());
    app.client = Some(client);
    app.mode = Mode::Attached(pid);
    assert_eq!(app.attached_paused(), paused);
    Rig { _w: w, app, host }
}

impl Rig {
    fn wheel(&mut self, up: bool, x: u16, y: u16) {
        let kind = if up { MouseEventKind::ScrollUp } else { MouseEventKind::ScrollDown };
        self.app.on_mouse(MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE });
    }

    /// What the client has sent to the session so far, as the bytes inside its frames.
    fn sent(&mut self) -> Vec<u8> {
        let mut raw = Vec::new();
        let mut buf = [0u8; 256];
        while let Ok(n) = self.host.read(&mut buf) {
            if n == 0 {
                break;
            }
            raw.extend_from_slice(&buf[..n]);
        }
        let mut out = Vec::new();
        let mut r = raw.as_slice();
        while let Ok((_, payload)) = crate::host::read_frame(&mut r) {
            out.extend(payload);
        }
        out
    }

    fn scrollback(&self) -> usize {
        self.app.client.as_ref().unwrap().parser.lock().unwrap().screen().scrollback()
    }

    /// The rows of the session's screen as the view shows them now.
    fn rows(&self) -> Vec<String> {
        self.app.client.as_ref().unwrap().parser.lock().unwrap().screen().rows(0, COLS).collect()
    }
}

fn lines(range: std::ops::Range<usize>) -> Vec<String> {
    range.map(|i| format!("line {i}")).collect()
}

const TRACKING_SGR: &str = "\x1b[?1000h\x1b[?1006h";

#[test]
fn a_session_that_asked_for_mouse_reports_gets_the_wheel_and_our_scrollback_stays_put() {
    let mut r = rig(TRACKING_SGR, false);
    // Screen column 9, row 4 is cell (9, 4) of the session: reported 1-based.
    r.wheel(true, 9, 4);
    r.wheel(false, 0, 0);
    r.wheel(false, 79, 22);
    assert_eq!(String::from_utf8(r.sent()).unwrap(), "\x1b[<64;10;5M\x1b[<65;1;1M\x1b[<65;80;23M");
    assert_eq!(r.scrollback(), 0);
}

#[test]
fn the_report_follows_the_encoding_the_session_asked_for() {
    let mut plain = rig("\x1b[?1000h", false);
    plain.wheel(true, 9, 4);
    assert_eq!(plain.sent(), [0x1b, b'[', b'M', 96, 42, 37]);
    let mut utf8 = rig("\x1b[?1002h\x1b[?1005h", false);
    utf8.wheel(false, 9, 4);
    assert_eq!(utf8.sent(), [0x1b, b'[', b'M', 97, 42, 37]);
}

#[test]
fn a_cell_the_plain_encoding_cannot_express_is_dropped_not_scrolled() {
    let mut r = rig("\x1b[?1000h", false);
    // A 260 column terminal: the session's screen is 260 wide, so column 230 of it is past the 223 limit.
    r.app.sync_size(260, 24);
    let _ = r.sent(); // the resize message
    r.wheel(true, 230, 5);
    assert!(r.sent().is_empty(), "nothing is sent for a cell it cannot tell");
    assert_eq!(r.scrollback(), 0, "and it does not scroll our own screen either");
    r.wheel(true, 199, 4);
    assert_eq!(r.sent(), [0x1b, b'[', b'M', 96, 32 + 200, 37]);
}

#[test]
fn without_mouse_reports_the_wheel_scrolls_our_own_scrollback_and_writes_nothing() {
    let mut r = rig("", false);
    r.wheel(true, 10, 5);
    assert_eq!(r.scrollback(), 3);
    r.wheel(true, 10, 5);
    assert_eq!(r.scrollback(), 6);
    r.wheel(false, 10, 5);
    assert_eq!(r.scrollback(), 3);
    assert!(r.sent().is_empty());
}

#[test]
fn turning_mouse_reports_off_again_gives_the_wheel_back() {
    let mut r = rig(TRACKING_SGR, false);
    r.app.client.as_ref().unwrap().parser.lock().unwrap().process(b"\x1b[?1000l");
    r.wheel(true, 10, 5);
    assert_eq!((r.scrollback(), r.sent().is_empty()), (3, true));
}

#[test]
fn a_paused_session_is_never_written_to() {
    let mut r = rig(TRACKING_SGR, true);
    r.wheel(true, 10, 5);
    r.wheel(false, 10, 5);
    assert!(r.sent().is_empty(), "nothing may reach a paused session");
    r.wheel(true, 10, 5);
    assert_eq!(r.scrollback(), 3, "the wheel falls back to our own scrollback");
}

#[test]
fn a_wheel_over_the_status_bar_is_not_sent() {
    let mut r = rig(TRACKING_SGR, false);
    r.wheel(true, 0, 23); // the status bar, left end
    r.wheel(true, 10, 23); // the status bar
    r.wheel(true, 79, 23); // the status bar, right end
    assert!(r.sent().is_empty());
    assert_eq!(r.scrollback(), 9, "each one still scrolls our own screen, as before");
}

#[test]
fn the_wheel_still_drops_the_selection_when_it_is_sent_on() {
    let mut r = rig(TRACKING_SGR, false);
    let ev = |kind, x, y| MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE };
    r.app.on_mouse(ev(MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left), 2, 1));
    r.app.on_mouse(ev(MouseEventKind::Drag(ratatui::crossterm::event::MouseButton::Left), 9, 1));
    assert!(r.app.selection.is_some());
    r.wheel(true, 9, 4);
    assert!(r.app.selection.is_none());
    assert_eq!(r.sent(), b"\x1b[<64;10;5M");
}

#[test]
fn scrolling_back_first_and_then_meeting_a_session_that_wants_the_mouse_shows_the_live_screen() {
    let mut r = rig("", false);
    r.wheel(true, 10, 5);
    assert_eq!(r.scrollback(), 3);
    r.app.client.as_ref().unwrap().parser.lock().unwrap().process(TRACKING_SGR.as_bytes());
    r.wheel(true, 9, 4);
    assert_eq!(r.scrollback(), 0, "the view goes back to the live screen, where the session answers");
    assert_eq!(r.sent(), b"\x1b[<64;10;5M");
}

#[test]
fn a_shorter_window_keeps_the_lines_that_no_longer_fit_above_it_and_a_taller_one_brings_them_back() {
    let mut r = rig("", false);
    r.app.sync_size(80, 24);
    assert_eq!(r.rows(), lines(37..60));
    r.wheel(true, 10, 5);
    assert_eq!(r.scrollback(), 3);

    // 15 rows: the session's screen is 14. The newest lines stay, where the session's cursor is, and the view is on the
    // live screen again, as a terminal's is after a resize.
    r.app.sync_size(80, 15);
    assert_eq!((r.scrollback(), r.rows()), (0, lines(46..60)));
    // What no longer fits went above the screen, so the wheel finds it.
    r.wheel(true, 10, 5);
    assert_eq!(r.rows(), lines(43..57));

    // Taller again: the same lines in the same places, none lost and none twice.
    r.app.sync_size(80, 24);
    assert_eq!((r.scrollback(), r.rows()), (0, lines(37..60)));
    r.wheel(true, 10, 5);
    assert_eq!(r.rows(), lines(34..57), "and the history above them as it was");
}
