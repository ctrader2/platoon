//! Shared helpers for the black-box tests: a scripted terminal, and a raw client of the host protocol.
#![allow(dead_code)]

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::{
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, channel},
    thread,
    time::{Duration, Instant},
};

pub const BIN: &str = env!("CARGO_BIN_EXE_platoon");
pub const WAIT: Duration = Duration::from_secs(10);

/// Directories to remove when the test binary exits.
static AT_EXIT: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());
static REGISTER: std::sync::Once = std::sync::Once::new();

/// A test's temp directory, removed when the test binary exits - whether its tests passed or failed. Returns
/// `path` as it is. (The same as `testkit::removed_at_exit` in the unit tests.)
pub fn removed_at_exit(path: PathBuf) -> PathBuf {
    REGISTER.call_once(|| {
        // SAFETY: registers a plain `extern "C" fn` with the C library, which calls it once as the process exits.
        unsafe { libc::atexit(remove_at_exit) };
    });
    AT_EXIT.lock().unwrap_or_else(|e| e.into_inner()).push(path.clone());
    path
}

extern "C" fn remove_at_exit() {
    // Never wait at exit: a lock still held means a test thread is mid-registration; its dirs are left alone.
    let dirs = match AT_EXIT.try_lock() {
        Ok(mut dirs) => std::mem::take(&mut *dirs),
        Err(std::sync::TryLockError::Poisoned(e)) => std::mem::take(&mut *e.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => return,
    };
    for d in dirs {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// Milliseconds since the epoch, as Claude Code writes them: `2026-09-19T03:53:40.091Z`. (The same as
/// `testkit::iso` in the unit tests; not `date -d`, which only GNU's `date` has.)
pub fn iso(ms: u64) -> String {
    let (secs, frac) = ((ms / 1000) as i64, ms % 1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{frac:03}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

pub fn tmp(name: &str) -> PathBuf {
    let d = removed_at_exit(std::env::temp_dir().join(format!("platoon-it-{name}-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The pid exists (a zombie, which has exited but not been reaped yet, counts: see `running`).
pub fn alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// The process exists and has not exited: not gone, and not a zombie waiting for its parent to reap it.
pub fn running(pid: u32) -> bool {
    alive(pid) && !zombie_or_gone(pid)
}

/// Linux reads `/proc/<pid>/stat`; where there is no `/proc` (macOS) `ps` says, its STAT starting with Z for a
/// zombie, and printing nothing for a pid that is gone.
fn zombie_or_gone(pid: u32) -> bool {
    let state = if Path::new("/proc/self/stat").exists() {
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(s) => s.rsplit_once(')').and_then(|(_, r)| r.split_whitespace().next().map(str::to_string)),
            Err(_) => None,
        }
    } else {
        std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    state.is_none_or(|st| st.starts_with('Z'))
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
    /// The terminal's own end, kept for the life of the Term (and to resize it).
    master: Box<dyn MasterPty + Send>,
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
        cmd.env("PLATOON_FORBID_REAL_CLAUDE", "1"); // ...nor the real `claude`, if a test forgets to name a stand-in
        cmd.env("PLATOON_FORBID_REAL_EDITOR", "1"); // ...nor a real editor: a test names its own stand-ins
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
        Self { child, master: pair.master, writer, rx, screen: vt100::Parser::new(rows, cols, 200), raw: Vec::new() }
    }

    /// Make the terminal `rows` by `cols`, as a window being resized does (the program gets SIGWINCH).
    pub fn resize(&mut self, rows: u16, cols: u16) {
        // What was drawn at the old size is taken in first, so it isn't read as drawn at the new one.
        while let Ok(b) = self.rx.recv_timeout(Duration::from_millis(50)) {
            self.screen.process(&b);
            self.raw.extend_from_slice(&b);
        }
        self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).unwrap();
        self.screen.screen_mut().set_size(rows, cols);
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

    /// Whether a screen cell is drawn (bold, italic, underlined).
    pub fn attrs_at(&mut self, row: u16, col: u16) -> (bool, bool, bool) {
        self.text();
        self.screen.screen().cell(row, col).map(|c| (c.bold(), c.italic(), c.underline())).unwrap()
    }

    /// Where `needle` starts on the screen (row, column), if it is on it. Columns count cells, so a line holding
    /// wide characters before the needle is measured the way the terminal draws it.
    pub fn find(&mut self, needle: &str) -> Option<(u16, u16)> {
        let text = self.text();
        for (row, line) in text.lines().enumerate() {
            if let Some(at) = line.find(needle) {
                let col = line[..at].chars().count();
                return Some((row as u16, col as u16));
            }
        }
        None
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

    /// Waits for `platoon setup` to end, and checks it asked nothing about a code editor (an older platoon's setup
    /// asked last). Its exit code.
    #[allow(dead_code)]
    pub fn exit_without_editor_question(&mut self) -> u32 {
        let code = self.wait_exit();
        let shown = self.flat();
        assert!(
            !shown.contains("Install the code editor") && !shown.contains("Code editor"),
            "setup asked about the code editor:\n{shown}"
        );
        code
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

    /// The screen with its rows joined end to end (trailing blanks dropped), so a line the terminal wrapped reads
    /// whole again: for text whose length depends on the machine, such as a temp folder's path.
    pub fn unwrapped(&mut self) -> String {
        self.text().lines().map(str::trim_end).collect()
    }

    pub fn wait_for_unwrapped(&mut self, text: &str) {
        let start = Instant::now();
        while start.elapsed() < WAIT {
            if self.unwrapped().contains(text) {
                return;
            }
            self.drain(Duration::from_millis(50));
        }
        panic!("never saw {text:?}, even across wrapped rows; screen was:\n{}", self.text());
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

    /// Waits until **every** one of `needles` is on the screen at the same time.
    ///
    /// A frame reaches this harness as a stream of bytes, cell by cell from the top down, and `text()` reads
    /// whatever has arrived: `wait_for` returns the moment its own needle lands, which can be halfway through a
    /// frame, with everything below it still showing the frame before. So when one change puts several things on
    /// the screen at once - a popup's legend and the lines under it, a message that wrapped onto two rows - wait
    /// for all of them here instead of waiting for one and reading the rest out of a snapshot.
    pub fn wait_for_all(&mut self, needles: &[&str]) {
        let start = Instant::now();
        while start.elapsed() < WAIT {
            let text = self.text();
            if needles.iter().all(|n| text.contains(n)) {
                return;
            }
            self.drain(Duration::from_millis(50));
        }
        let text = self.text();
        let missing: Vec<&str> = needles.iter().copied().filter(|n| !text.contains(n)).collect();
        panic!("never saw {missing:?} (waiting for all of {needles:?}); screen was:\n{text}");
    }

    /// The screen once it has stopped changing. For assertions about a whole frame - what is **not** on it, what
    /// order the rows are in, how many times something appears - which a half-drawn frame would answer wrongly.
    /// (It watches the screen itself, not the stream: the program writes nothing while nothing changes, but a frame
    /// that changed only colors still arrives as bytes.)
    pub fn stable(&mut self) -> String {
        let start = Instant::now();
        let mut last = self.text();
        let mut unchanged = 0;
        while start.elapsed() < WAIT {
            self.drain(Duration::from_millis(60));
            let now = self.text();
            if now == last {
                unchanged += 1;
                if unchanged >= 2 {
                    return now;
                }
            } else {
                unchanged = 0;
                last = now;
            }
        }
        panic!("the screen never stopped changing; it was:\n{last}");
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

/// A stand-in for `claude -p`: see `tests/fake-claude.sh`. Records every call under `calls/N.*` and answers from files
/// the test writes (`reply.N`, `reply.default`, `usage`, `hang`, `fail`, `delay`).
pub struct AiFake {
    pub dir: PathBuf,
    pub script: String,
}

impl AiFake {
    pub fn new(name: &str) -> AiFake {
        let dir = tmp(&format!("aifake-{name}"));
        std::fs::create_dir_all(dir.join("calls")).unwrap();
        let script = dir.join("fake-claude");
        std::fs::write(&script, include_str!("../fake-claude.sh").replace("@DIR@", &dir.display().to_string())).unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        AiFake { script: script.display().to_string(), dir }
    }

    pub fn calls(&self) -> usize {
        std::fs::read_dir(self.dir.join("calls"))
            .map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".stdin")).count())
            .unwrap_or(0)
    }

    pub fn stdin(&self, k: usize) -> String {
        std::fs::read_to_string(self.dir.join("calls").join(format!("{k}.stdin"))).unwrap_or_default()
    }

    pub fn reply_default(&self, text: &str) {
        std::fs::write(self.dir.join("reply.default"), text).unwrap();
    }

    pub fn hang(&self, on: bool) {
        let p = self.dir.join("hang");
        if on {
            std::fs::write(p, "").unwrap();
        } else {
            let _ = std::fs::remove_file(p);
        }
    }

    pub fn usage(&self, input: u64, output: u64) {
        std::fs::write(
            self.dir.join("usage"),
            format!(r#"{{"input_tokens":{input},"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":{output}}}"#),
        )
        .unwrap();
    }

    /// The pids of the hanging fake and of the `sleep` it started.
    pub fn pids(&self) -> Vec<u32> {
        ["pid", "sleep-pid"].iter().filter_map(|f| std::fs::read_to_string(self.dir.join(f)).ok()?.trim().parse().ok()).collect()
    }

    /// True while any of them still runs. (A zombie is not running.)
    pub fn any_alive(&self) -> bool {
        self.pids().into_iter().any(running)
    }
}

impl Drop for AiFake {
    fn drop(&mut self) {
        for p in self.pids() {
            if p > 1 && alive(p) {
                unsafe { libc::kill(p as i32, libc::SIGKILL) };
            }
        }
    }
}
