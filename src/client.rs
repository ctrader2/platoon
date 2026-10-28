//! Connection from platoon to a session host, plus the emulator screen it draws from.

use crate::host::{F_INPUT, F_OUTPUT, F_RESIZE, SCROLLBACK, read_frame, size_payload, write_frame};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::{
    io,
    net::Shutdown,
    os::unix::net::UnixStream,
    path::Path,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

/// How soon after connecting again a window gives up when its connection closes once more: a host that keeps closing
/// it is not chased in a loop, and the window ends as if the session had.
pub const RECONNECT_GAP: Duration = Duration::from_secs(1);

pub struct Client {
    pub parser: Arc<Mutex<vt100::Parser>>,
    stream: UnixStream,
    closed: Arc<AtomicBool>,
    size: (u16, u16),
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Client {
    pub fn connect(sock: &Path, rows: u16, cols: u16) -> io::Result<Self> {
        let stream = UnixStream::connect(sock)?;
        let mut writer = stream.try_clone()?;
        write_frame(&mut writer, F_RESIZE, &size_payload(rows, cols))?;
        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, SCROLLBACK)));
        let closed = Arc::new(AtomicBool::new(false));
        let p = parser.clone();
        read_output(stream.try_clone()?, closed.clone(), move |payload| lock(&p).process(payload));
        Ok(Self { parser, stream, closed, size: (rows, cols) })
    }

    /// A client with nothing behind it, for tests: the emulator is fed by hand, and what the client writes
    /// can be read from the other end of the pair (the "host").
    #[cfg(test)]
    pub fn for_test(rows: u16, cols: u16) -> (Self, UnixStream) {
        let (stream, host) = UnixStream::pair().unwrap();
        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, SCROLLBACK)));
        (Self { parser, stream, closed: Arc::new(AtomicBool::new(false)), size: (rows, cols) }, host)
    }

    pub fn closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    pub fn write(&mut self, bytes: &[u8]) {
        if write_frame(&mut self.stream, F_INPUT, bytes).is_err() {
            self.closed.store(true, Ordering::Relaxed);
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        if self.size == (rows, cols) {
            return;
        }
        self.size = (rows, cols);
        let _ = write_frame(&mut self.stream, F_RESIZE, &size_payload(rows, cols));
        lock(&self.parser).screen_mut().set_size(rows, cols);
    }

    pub fn scroll(&self, delta: i32) {
        let mut p = lock(&self.parser);
        let cur = p.screen().scrollback() as i32;
        p.screen_mut().set_scrollback((cur + delta).max(0) as usize);
    }

    pub fn reset_scroll(&self) {
        lock(&self.parser).screen_mut().set_scrollback(0);
    }

    /// Read the session's whole text (`select::History`): what is kept above its screen as well as the screen.
    pub fn history<R>(&self, read: impl FnOnce(&crate::select::History) -> R) -> R {
        let mut p = lock(&self.parser);
        read(&crate::select::History::new(p.screen_mut()))
    }

    /// The session has asked to hear the mouse (Claude's full screen mode does): the wheel goes to it, and platoon
    /// has no scrollback of its own to move.
    pub fn wants_mouse(&self) -> bool {
        lock(&self.parser).screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None
    }

    pub fn app_cursor(&self) -> bool {
        lock(&self.parser).screen().application_cursor()
    }

    /// What to send the session for a wheel turn at this cell (1-based, within the session's screen), or None
    /// when the session has not asked to hear the mouse: then the wheel is ours, to scroll what we have. Empty
    /// when it has asked but the cell cannot be told to it (a very wide screen): the turn is dropped.
    pub fn wheel_report(&self, up: bool, col: u16, row: u16) -> Option<Vec<u8>> {
        let p = lock(&self.parser);
        let screen = p.screen();
        if screen.mouse_protocol_mode() == vt100::MouseProtocolMode::None {
            return None;
        }
        Some(wheel_report(up, col, row, screen.mouse_protocol_encoding()).unwrap_or_default())
    }
}

/// The report a terminal sends for one wheel notch (button 64 up, 65 down) at a 1-based cell, in the way the
/// session asked for. None when the cell does not fit that encoding (the plain one stops at 223).
pub fn wheel_report(up: bool, col: u16, row: u16, encoding: vt100::MouseProtocolEncoding) -> Option<Vec<u8>> {
    let button = if up { 64u16 } else { 65 };
    match encoding {
        vt100::MouseProtocolEncoding::Sgr => Some(format!("\x1b[<{button};{col};{row}M").into_bytes()),
        vt100::MouseProtocolEncoding::Default => {
            let byte = |v: u16| u8::try_from(v + 32).ok();
            Some(vec![0x1b, b'[', b'M', byte(button)?, byte(col)?, byte(row)?])
        }
        vt100::MouseProtocolEncoding::Utf8 => {
            // Each value is a character, so it can go as far as two bytes (2015).
            let mut out = b"\x1b[M".to_vec();
            for v in [button, col, row] {
                let c = char::from_u32(u32::from(v) + 32).filter(|c| *c <= '\u{7ff}')?;
                out.extend_from_slice(c.to_string().as_bytes());
            }
            Some(out)
        }
    }
}

/// Read the host's output on a thread of its own, handing each piece to `feed` (the emulator), until the connection
/// ends. However the thread ends, `closed` says so, a panic inside `feed` included: a window is never left showing a
/// screen nothing updates any more while it looks connected.
fn read_output(mut reader: UnixStream, closed: Arc<AtomicBool>, mut feed: impl FnMut(&[u8]) + Send + 'static) {
    thread::spawn(move || {
        let _ended = SaysClosed(closed);
        while let Ok((kind, payload)) = read_frame(&mut reader) {
            if kind == F_OUTPUT {
                feed(&payload);
            }
        }
    });
}

/// Marks the connection closed when dropped, which also happens when its thread panics.
struct SaysClosed(Arc<AtomicBool>);

impl Drop for SaysClosed {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

impl Drop for Client {
    /// Detaching only closes our connection; the host and claude keep running.
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

/// Bytes a terminal would send for this key press.
pub fn key_bytes(k: KeyEvent, app_cursor: bool) -> Vec<u8> {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    let shift = k.modifiers.contains(KeyModifiers::SHIFT);
    // xterm modifier parameter: 1 + shift(1) + alt(2) + ctrl(4)
    let m = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8;
    let esc = |s: &str| s.as_bytes().to_vec();
    let with_esc = |mut v: Vec<u8>| {
        if alt {
            v.insert(0, 0x1b);
        }
        v
    };
    let cursor = |c: char| {
        if m > 1 {
            format!("\x1b[1;{m}{c}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{c}").into_bytes()
        } else {
            format!("\x1b[{c}").into_bytes()
        }
    };
    let tilde = |n: u8| {
        if m > 1 { format!("\x1b[{n};{m}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() }
    };
    match k.code {
        KeyCode::Char(c) => {
            if ctrl {
                let b = match c {
                    'a'..='z' | 'A'..='Z' => Some(c.to_ascii_lowercase() as u8 - b'a' + 1),
                    ' ' | '@' | '2' => Some(0),
                    '[' => Some(27),
                    '\\' => Some(28),
                    ']' => Some(29),
                    '^' | '6' => Some(30),
                    '_' | '/' => Some(31),
                    _ => None,
                };
                if let Some(b) = b {
                    return with_esc(vec![b]);
                }
            }
            with_esc(c.to_string().into_bytes())
        }
        KeyCode::Enter => with_esc(vec![b'\r']),
        KeyCode::Backspace => with_esc(vec![0x7f]),
        KeyCode::Tab => with_esc(vec![b'\t']),
        KeyCode::BackTab => esc("\x1b[Z"),
        KeyCode::Esc => with_esc(vec![0x1b]),
        KeyCode::Up => cursor('A'),
        KeyCode::Down => cursor('B'),
        KeyCode::Right => cursor('C'),
        KeyCode::Left => cursor('D'),
        KeyCode::Home => cursor('H'),
        KeyCode::End => cursor('F'),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::F(n @ 1..=4) => format!("\x1bO{}", ['P', 'Q', 'R', 'S'][n as usize - 1]).into_bytes(),
        KeyCode::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][n as usize - 5]),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{F_OUTPUT, read_output, wheel_report, write_frame};
    use std::{
        os::unix::net::UnixStream,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::{Duration, Instant},
    };
    use vt100::MouseProtocolEncoding::{Default, Sgr, Utf8};

    fn closed_within(closed: &AtomicBool, wait: Duration) -> bool {
        let start = Instant::now();
        while !closed.load(Ordering::Relaxed) && start.elapsed() < wait {
            std::thread::sleep(Duration::from_millis(10));
        }
        closed.load(Ordering::Relaxed)
    }

    #[test]
    fn a_reader_that_dies_on_what_it_was_sent_says_the_connection_is_closed() {
        let (mut host, window) = UnixStream::pair().unwrap();
        let closed = Arc::new(AtomicBool::new(false));
        // Stands in for the emulator failing on some output (the host's connection stays open throughout).
        read_output(window, closed.clone(), |payload| {
            if payload == b"poison" {
                panic!("a stand-in for the emulator failing on its input (expected by this test)");
            }
        });
        write_frame(&mut host, F_OUTPUT, b"fine").unwrap();
        assert!(!closed_within(&closed, Duration::from_millis(200)), "output it can take keeps it open");
        write_frame(&mut host, F_OUTPUT, b"poison").unwrap();
        assert!(closed_within(&closed, Duration::from_secs(5)), "the window must learn that nothing will update its screen");
    }

    #[test]
    fn a_reader_whose_host_closes_the_connection_says_so() {
        let (host, window) = UnixStream::pair().unwrap();
        let closed = Arc::new(AtomicBool::new(false));
        read_output(window, closed.clone(), |_| {});
        host.shutdown(std::net::Shutdown::Both).unwrap();
        assert!(closed_within(&closed, Duration::from_secs(5)));
    }

    #[test]
    fn sgr_reports_carry_the_button_and_the_cell_as_numbers() {
        assert_eq!(wheel_report(true, 10, 5, Sgr).unwrap(), b"\x1b[<64;10;5M");
        assert_eq!(wheel_report(false, 78, 21, Sgr).unwrap(), b"\x1b[<65;78;21M");
        assert_eq!(wheel_report(true, 300, 400, Sgr).unwrap(), b"\x1b[<64;300;400M", "no limit on the size of the screen");
    }

    #[test]
    fn plain_reports_add_32_to_each_value_and_stop_at_223() {
        assert_eq!(wheel_report(true, 10, 5, Default).unwrap(), [0x1b, b'[', b'M', 96, 42, 37]);
        assert_eq!(wheel_report(false, 1, 1, Default).unwrap(), [0x1b, b'[', b'M', 97, 33, 33]);
        assert_eq!(wheel_report(true, 223, 223, Default).unwrap(), [0x1b, b'[', b'M', 96, 255, 255]);
        assert_eq!(wheel_report(true, 224, 5, Default), None);
        assert_eq!(wheel_report(true, 5, 224, Default), None);
    }

    #[test]
    fn utf8_reports_are_the_plain_ones_with_each_value_a_character() {
        // Small cells are one byte each, the same as the plain report.
        assert_eq!(wheel_report(true, 10, 5, Utf8), wheel_report(true, 10, 5, Default));
        // Past 95 the characters need two bytes: 32 + 200 = 232 = U+00E8.
        assert_eq!(wheel_report(false, 200, 5, Utf8).unwrap(), "\x1b[M\u{61}\u{e8}\u{25}".as_bytes());
        assert_eq!(wheel_report(true, 2015, 2015, Utf8).unwrap().len(), 3 + 1 + 2 + 2);
        assert_eq!(wheel_report(true, 2016, 5, Utf8), None, "past what two bytes hold");
    }
}
