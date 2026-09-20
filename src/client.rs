//! Connection from platoon to a session host, plus the emulator screen it draws from.

use crate::host::{read_frame, size_payload, write_frame, F_INPUT, F_OUTPUT, F_RESIZE, SCROLLBACK};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::{
    io,
    net::Shutdown,
    os::unix::net::UnixStream,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
    thread,
};

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
        let (p, c, mut reader) = (parser.clone(), closed.clone(), stream.try_clone()?);
        thread::spawn(move || {
            while let Ok((kind, payload)) = read_frame(&mut reader) {
                if kind == F_OUTPUT {
                    lock(&p).process(&payload);
                }
            }
            c.store(true, Ordering::Relaxed);
        });
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

    pub fn app_cursor(&self) -> bool {
        lock(&self.parser).screen().application_cursor()
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
        if m > 1 {
            format!("\x1b[{n};{m}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
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
