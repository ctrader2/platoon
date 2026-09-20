//! Shared helpers for the black-box tests: a scripted terminal, and a raw client of the host protocol.
#![allow(dead_code)]

use portable_pty::{native_pty_system, Child, CommandBuilder, PtySize};
use std::{
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::mpsc::{channel, Receiver},
    thread,
    time::{Duration, Instant},
};

pub const BIN: &str = env!("CARGO_BIN_EXE_platoon");
pub const WAIT: Duration = Duration::from_secs(10);

pub fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("platoon-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub fn alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

pub fn until(what: &str, mut cond: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < WAIT {
        if cond() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for: {what}");
}

/// (claude_pid, socket) of the first host in the cache dir.
pub fn find_host(cache: &Path) -> Option<(u32, PathBuf)> {
    for e in std::fs::read_dir(cache.join("hosts")).ok()?.flatten() {
        if e.path().extension().and_then(|x| x.to_str()) == Some("json") {
            let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
            return Some((v["claude_pid"].as_u64()? as u32, e.path().with_extension("sock")));
        }
    }
    None
}

/// A command running in a real terminal we can type into and read.
pub struct Term {
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    rx: Receiver<Vec<u8>>,
    screen: vt100::Parser,
    /// Every byte the program wrote, for what the screen model cannot show (an OSC 52 clipboard write).
    raw: Vec<u8>,
}

impl Term {
    pub fn spawn(args: &[&str], env: &[(&str, String)], cwd: &Path, rows: u16, cols: u16) -> Self {
        let pair = native_pty_system().openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).unwrap();
        let mut cmd = CommandBuilder::new(BIN);
        cmd.args(args);
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("NO_COLOR", "1");
        cmd.env("PLATOON_BADGES", "square"); // the same text whatever terminal the tests run in
        cmd.env("PLATOON_COLORS", "truecolor"); // and the same colors
        cmd.env("PLATOON_CLIPBOARD", "osc52"); // a test must never touch the real clipboard
        for (k, v) in env {
            cmd.env(k, v);
        }
        let child = pair.slave.spawn_command(cmd).unwrap();
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();
        let writer = pair.master.take_writer().unwrap();
        let (tx, rx) = channel::<Vec<u8>>();
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        // Keep the master alive for the life of the Term by leaking it into a thread-less holder.
        std::mem::forget(pair.master);
        Self { child, writer, rx, screen: vt100::Parser::new(rows, cols, 200), raw: Vec::new() }
    }

    pub fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).unwrap();
        self.writer.flush().unwrap();
    }

    fn drain(&mut self, wait: Duration) {
        if let Ok(b) = self.rx.recv_timeout(wait) {
            self.screen.process(&b);
            self.raw.extend_from_slice(&b);
        }
    }

    pub fn text(&mut self) -> String {
        while let Ok(b) = self.rx.try_recv() {
            self.screen.process(&b);
            self.raw.extend_from_slice(&b);
        }
        self.screen.screen().contents()
    }

    /// Everything the program has written so far, byte for byte.
    pub fn raw(&mut self) -> Vec<u8> {
        self.text();
        self.raw.clone()
    }

    /// The background color of a screen cell, as the terminal was told to draw it.
    pub fn bg_at(&mut self, row: u16, col: u16) -> vt100::Color {
        self.text();
        self.screen.screen().cell(row, col).map(|c| c.bgcolor()).unwrap()
    }

    /// The foreground color of a screen cell.
    pub fn fg_at(&mut self, row: u16, col: u16) -> vt100::Color {
        self.text();
        self.screen.screen().cell(row, col).map(|c| c.fgcolor()).unwrap()
    }

    /// Waits until a cell has the given foreground.
    pub fn wait_fg(&mut self, row: u16, col: u16, want: vt100::Color) {
        let start = Instant::now();
        while start.elapsed() < WAIT {
            if self.fg_at(row, col) == want {
                return;
            }
            self.drain(Duration::from_millis(50));
        }
        panic!("cell ({row},{col}) never got foreground {want:?}; it is {:?}", self.fg_at(row, col));
    }

    /// Waits until a cell has the given background.
    pub fn wait_bg(&mut self, row: u16, col: u16, want: vt100::Color) {
        let start = Instant::now();
        while start.elapsed() < WAIT {
            if self.bg_at(row, col) == want {
                return;
            }
            self.drain(Duration::from_millis(50));
        }
        panic!("cell ({row},{col}) never got background {want:?}; it is {:?}", self.bg_at(row, col));
    }

    /// Waits for text to disappear, e.g. a popup closing.
    pub fn wait_gone(&mut self, text: &str) {
        let start = Instant::now();
        while start.elapsed() < WAIT {
            if !self.text().contains(text) {
                return;
            }
            self.drain(Duration::from_millis(50));
        }
        panic!("{text:?} never went away; screen was:\n{}", self.text());
    }

    /// The screen with runs of spaces collapsed, for text whose spacing is styling ("1 Active  3").
    pub fn flat(&mut self) -> String {
        self.text().split_whitespace().collect::<Vec<_>>().join(" ")
    }

    pub fn wait_for_flat(&mut self, text: &str) {
        let start = Instant::now();
        while start.elapsed() < WAIT {
            if self.flat().contains(text) {
                return;
            }
            self.drain(Duration::from_millis(50));
        }
        panic!("never saw {text:?}; screen was:\n{}", self.text());
    }

    pub fn wait_for(&mut self, text: &str) {
        let start = Instant::now();
        while start.elapsed() < WAIT {
            if self.text().contains(text) {
                return;
            }
            self.drain(Duration::from_millis(50));
        }
        panic!("never saw {text:?}; screen was:\n{}", self.text());
    }

    /// Exit code once the program has exited.
    pub fn wait_exit(&mut self) -> u32 {
        let start = Instant::now();
        while start.elapsed() < WAIT {
            if let Ok(Some(status)) = self.child.try_wait() {
                // Its last output may still be in flight through the pty: take it before reading the screen.
                while let Ok(b) = self.rx.recv_timeout(Duration::from_millis(100)) {
                    self.screen.process(&b);
                    self.raw.extend_from_slice(&b);
                }
                return status.exit_code();
            }
            self.drain(Duration::from_millis(50));
        }
        panic!("program never exited; screen was:\n{}", self.text());
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// Same framing as src/host.rs: kind byte, u32 BE length, payload.
pub fn write_frame(w: &mut impl Write, kind: u8, payload: &[u8]) -> io::Result<()> {
    let mut head = [kind, 0, 0, 0, 0];
    head[1..].copy_from_slice(&(payload.len() as u32).to_be_bytes());
    w.write_all(&head)?;
    w.write_all(payload)?;
    w.flush()
}

pub fn read_frame(r: &mut impl Read) -> io::Result<(u8, Vec<u8>)> {
    let mut head = [0u8; 5];
    r.read_exact(&mut head)?;
    let mut buf = vec![0; u32::from_be_bytes([head[1], head[2], head[3], head[4]]) as usize];
    r.read_exact(&mut buf)?;
    Ok((head[0], buf))
}

/// A window onto a host, speaking the socket protocol directly.
pub struct Viewer {
    stream: UnixStream,
    frames: Receiver<Vec<u8>>,
    screen: vt100::Parser,
}

impl Viewer {
    /// Connects and waits for the first snapshot, so the host has already fitted itself to this window.
    pub fn connect(sock: &Path, rows: u16, cols: u16) -> Self {
        let stream = UnixStream::connect(sock).unwrap();
        let mut w = stream.try_clone().unwrap();
        write_frame(&mut w, b'R', &[(rows >> 8) as u8, rows as u8, (cols >> 8) as u8, cols as u8]).unwrap();
        let (tx, frames) = channel();
        let mut r = stream.try_clone().unwrap();
        thread::spawn(move || {
            while let Ok((kind, payload)) = read_frame(&mut r) {
                if kind == b'O' && tx.send(payload).is_err() {
                    break;
                }
            }
        });
        let mut v = Self { stream, frames, screen: vt100::Parser::new(rows, cols, 100) };
        let first = v.frames.recv_timeout(WAIT).expect("no snapshot from the host");
        v.screen.process(&first);
        v
    }

    /// Leave now. (Dropping a Viewer doesn't close the socket while its reader thread still holds it.)
    pub fn close(self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }

    pub fn type_line(&mut self, text: &str) {
        write_frame(&mut self.stream, b'I', format!("{text}\r").as_bytes()).unwrap();
    }

    pub fn text(&mut self) -> String {
        while let Ok(b) = self.frames.try_recv() {
            self.screen.process(&b);
        }
        self.screen.screen().contents()
    }

    pub fn wait_for(&mut self, text: &str) {
        let start = Instant::now();
        while start.elapsed() < WAIT {
            if self.text().contains(text) {
                return;
            }
            if let Ok(b) = self.frames.recv_timeout(Duration::from_millis(50)) {
                self.screen.process(&b);
            }
        }
        panic!("never saw {text:?}; screen was:\n{}", self.text());
    }

    /// Keep asking until the answer shows up: for things that change a moment after an event.
    pub fn retry(&mut self, command: &str, expect: &str) {
        let start = Instant::now();
        while start.elapsed() < WAIT {
            self.type_line(command);
            thread::sleep(Duration::from_millis(250));
            if self.text().contains(expect) {
                return;
            }
        }
        panic!("`{command}` never printed {expect:?}; screen was:\n{}", self.text());
    }
}
