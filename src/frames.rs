//! Writing frames to the terminal without ever waiting for it. A terminal can stop reading for a long time (Ghostty
//! reads nothing while its window is behind another one), and a write to it then blocks until it reads again. So the
//! event loop never writes a frame itself: it leaves its newest one with the writer, a thread of its own that owns the
//! terminal's output, and carries on ticking, refreshing and handling keys. The writer takes the newest frame whenever
//! it is free: frames the terminal never took are skipped, not queued. It sends only the cells that differ from what it
//! actually wrote last, and every cell again after a write that had to wait a long time (or when asked: the window came
//! back to the front), so whatever the terminal skipped or lost is drawn again.

use ratatui::{
    backend::{Backend, CrosstermBackend},
    buffer::Buffer,
    crossterm::{
        queue,
        style::{Attribute, ResetColor, SetAttribute},
        terminal::{Clear, ClearType},
    },
    layout::Position,
};
use std::{
    io::{self, Write},
    sync::{Arc, Condvar, Mutex, MutexGuard},
    thread,
    time::{Duration, Instant},
};

/// A write that took longer than this means the terminal stopped reading for a while: the frame after it repaints every
/// cell, for terminals that don't say when their window comes back.
pub const LONG_WAIT: Duration = Duration::from_secs(1);

/// One frame: its cells, and where the cursor shows (None: hidden).
#[derive(Clone, PartialEq, Debug)]
pub struct Frame {
    pub cells: Buffer,
    pub cursor: Option<Position>,
}

/// What the event loop leaves for the writer, and what the writer says back.
#[derive(Default)]
struct State {
    /// The newest frame not yet written. A newer one replaces it.
    frame: Option<Frame>,
    /// Bytes for the terminal itself (a copy to its clipboard), written in order before the next frame, never skipped.
    bytes: Vec<u8>,
    /// The next frame repaints every cell.
    full: bool,
    /// Write nothing until resumed (another program has the terminal).
    paused: bool,
    /// A write is under way.
    writing: bool,
    /// Write what is waiting, then end.
    stop: bool,
    /// The write that failed; the writer has ended.
    error: Option<io::Error>,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn wait<'a>(&self, guard: MutexGuard<'a, State>) -> MutexGuard<'a, State> {
        self.changed.wait(guard).unwrap_or_else(|e| e.into_inner())
    }
}

/// The terminal's writer. Nothing it is given ever waits for the terminal; only `pause` and `finish` wait, for the
/// write under way.
pub struct Writer {
    shared: Arc<Shared>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Writer {
    /// A writer of frames to `out` (the terminal: stdout).
    pub fn spawn(out: impl Write + Send + 'static) -> Self {
        Self::spawn_with(out, LONG_WAIT)
    }

    /// `spawn`, with `long_wait` for `LONG_WAIT` (tests don't wait a second).
    fn spawn_with(out: impl Write + Send + 'static, long_wait: Duration) -> Self {
        let shared = Arc::new(Shared { state: Mutex::new(State::default()), changed: Condvar::new() });
        let thread = thread::spawn({
            let shared = shared.clone();
            move || write_frames(&shared, out, long_wait)
        });
        Self { shared, thread: Some(thread) }
    }

    /// Leave `frame` to be written; a frame still waiting is dropped for it.
    pub fn send(&self, frame: Frame) {
        self.shared.lock().frame = Some(frame);
        self.shared.changed.notify_all();
    }

    /// The next frame repaints every cell, whatever the terminal shows now.
    pub fn repaint(&self) {
        self.shared.lock().full = true;
    }

    /// Where bytes for the terminal itself go (a copy to its clipboard): written in order with the frames.
    pub fn bytes(&self) -> TerminalBytes {
        TerminalBytes(self.shared.clone())
    }

    /// Wait for the write under way to end, then write nothing (the frame waiting is dropped) until `resume`: the
    /// terminal is the caller's meanwhile.
    pub fn pause(&self) {
        let mut s = self.shared.lock();
        s.paused = true;
        s.frame = None;
        while s.writing {
            s = self.shared.wait(s);
        }
    }

    /// Write again after `pause`.
    pub fn resume(&self) {
        self.shared.lock().paused = false;
        self.shared.changed.notify_all();
    }

    /// Ok while the writer is writing; else why it stopped (a write to the terminal failed).
    pub fn check(&self) -> io::Result<()> {
        if let Some(e) = self.shared.lock().error.take() {
            return Err(e);
        }
        match &self.thread {
            Some(t) if t.is_finished() => Err(io::Error::other("platoon's screen writer stopped")),
            _ => Ok(()),
        }
    }

    /// Write what is waiting, then end the writer and wait for it: after this, the terminal is the caller's.
    pub fn finish(mut self) -> io::Result<()> {
        self.end();
        self.shared.lock().error.take().map_or(Ok(()), Err)
    }

    fn end(&mut self) {
        self.shared.lock().stop = true;
        self.shared.changed.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Writer {
    /// Never left writing behind its owner's back: the terminal is put back after this.
    fn drop(&mut self) {
        self.end();
    }
}

/// Bytes for the terminal, written by the writer in order with the frames: for a copy to the terminal's clipboard
/// (OSC 52), which must neither wait for the terminal nor land in the middle of a frame.
#[derive(Clone)]
pub struct TerminalBytes(Arc<Shared>);

impl TerminalBytes {
    pub fn send(&self, bytes: &[u8]) {
        self.0.lock().bytes.extend_from_slice(bytes);
        self.0.changed.notify_all();
    }
}

/// The writer's thread: take what is waiting, write it, and say when done, until told to stop.
fn write_frames(shared: &Shared, mut out: impl Write, long_wait: Duration) {
    // What the terminal was last sent: the next frame sends only the cells that differ from it.
    let mut last: Option<Frame> = None;
    // The last write waited long: repaint the last frame in full if no newer one comes.
    let mut again = false;
    loop {
        let (bytes, frame, full) = {
            let mut s = shared.lock();
            loop {
                let waiting = s.frame.is_some() || !s.bytes.is_empty() || (again && last.is_some());
                if !s.paused && waiting {
                    break;
                }
                if s.stop {
                    return;
                }
                s = shared.wait(s);
            }
            let frame = s.frame.take();
            // (A repaint asked for waits for a frame: bytes alone don't use it up.)
            let full = again || frame.is_some() && std::mem::take(&mut s.full);
            s.writing = true;
            (std::mem::take(&mut s.bytes), frame, full)
        };
        // A repaint because the last write waited long is not repeated if it waits long itself: a terminal that is
        // always slow is not sent every cell over and over.
        let repainting = std::mem::take(&mut again);
        let mut chunk = bytes;
        let frame = frame.or_else(|| if repainting { last.take() } else { None });
        if let Some(f) = &frame {
            encode(&mut chunk, last.as_ref().filter(|_| !full), f);
        }
        let start = Instant::now();
        let written = out.write_all(&chunk).and_then(|()| out.flush());
        if frame.is_some() {
            last = frame;
        }
        let mut s = shared.lock();
        s.writing = false;
        shared.changed.notify_all();
        if let Err(e) = written {
            s.error = Some(e);
            return;
        }
        again = !repainting && start.elapsed() >= long_wait;
    }
}

/// The bytes that turn the terminal's screen from `base` (None: unknown) into `frame`, appended to `out`: only the
/// cells that differ, or with no base the screen cleared and every cell drawn.
fn encode(out: &mut Vec<u8>, base: Option<&Frame>, frame: &Frame) {
    let blank;
    let from = match base {
        Some(b) if b.cells.area == frame.cells.area => &b.cells,
        _ => {
            let _ = queue!(out, ResetColor, SetAttribute(Attribute::Reset), Clear(ClearType::All));
            blank = Buffer::empty(frame.cells.area);
            &blank
        }
    };
    let mut backend = CrosstermBackend::new(out);
    // Writing to memory can't fail.
    let _ = backend.draw(from.diff(&frame.cells).into_iter());
    let _ = match frame.cursor {
        None => backend.hide_cursor(),
        Some(p) => backend.set_cursor_position(p).and_then(|()| backend.show_cursor()),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use std::sync::mpsc;

    /// A frame of `rows` lines of text, 20 cells wide.
    fn frame(rows: &[&str]) -> Frame {
        let mut cells = Buffer::empty(Rect::new(0, 0, 20, rows.len() as u16));
        for (y, line) in rows.iter().enumerate() {
            cells.set_string(0, y as u16, line, ratatui::style::Style::default());
        }
        Frame { cells, cursor: None }
    }

    /// A terminal that takes each write only when the test lets it (`let_through`), and keeps what it took.
    struct Gated {
        writes: Arc<Mutex<Vec<Vec<u8>>>>,
        gate: mpsc::Receiver<()>,
        arrived: Arc<Mutex<usize>>,
    }

    impl Write for Gated {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            *self.arrived.lock().unwrap() += 1;
            let _ = self.gate.recv();
            self.writes.lock().unwrap().push(buf.to_vec());
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct Term {
        writes: Arc<Mutex<Vec<Vec<u8>>>>,
        gate: mpsc::Sender<()>,
        arrived: Arc<Mutex<usize>>,
        let_through: usize,
    }

    impl Term {
        fn new() -> (Self, Gated) {
            let (gate, rx) = mpsc::channel();
            let (writes, arrived) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(0)));
            (Term { writes: writes.clone(), gate, arrived: arrived.clone(), let_through: 0 }, Gated { writes, gate: rx, arrived })
        }

        /// The next write is waiting at the gate.
        fn wait_held(&self) {
            wait_until(|| *self.arrived.lock().unwrap() > self.let_through);
        }

        fn let_through(&mut self) {
            self.let_through += 1;
            self.gate.send(()).unwrap();
        }

        fn writes(&self) -> Vec<Vec<u8>> {
            self.writes.lock().unwrap().clone()
        }

        /// The screen a terminal shows after everything written so far.
        fn screen(&self) -> String {
            let mut p = vt100::Parser::new(3, 20, 0);
            for w in self.writes() {
                p.process(&w);
            }
            p.screen().contents()
        }
    }

    fn wait_until(mut done: impl FnMut() -> bool) {
        let start = Instant::now();
        while !done() {
            assert!(start.elapsed() < Duration::from_secs(10), "timed out");
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// How many writes the terminal has taken once every write let through has landed and nothing more comes for a
    /// while.
    fn settled(term: &Term) -> usize {
        wait_until(|| term.writes().len() >= term.let_through);
        thread::sleep(Duration::from_millis(150));
        term.writes().len()
    }

    #[test]
    fn the_newest_frame_wins_and_frames_the_terminal_never_took_are_skipped_not_queued() {
        let (mut term, out) = Term::new();
        let writer = Writer::spawn_with(out, Duration::from_secs(60));
        writer.send(frame(&["alpha"]));
        term.wait_held(); // the terminal reads nothing: the writer is stuck in its first write
        // Meanwhile the loop goes on: nothing it does waits.
        let start = Instant::now();
        for word in ["bravo", "charlie", "delta"] {
            writer.send(frame(&[word]));
        }
        assert!(start.elapsed() < Duration::from_millis(500));
        term.let_through();
        term.wait_held();
        term.let_through();
        assert_eq!(settled(&term), 2, "the first frame, then only the newest");
        assert_eq!(term.screen().trim_end(), "delta");
        let all = String::from_utf8_lossy(&term.writes().concat()).into_owned();
        assert!(!all.contains("bravo") && !all.contains("charl"), "a skipped frame was written: {all:?}");
    }

    #[test]
    fn each_frame_is_sent_as_a_difference_from_what_was_actually_written_last() {
        let (mut term, out) = Term::new();
        let writer = Writer::spawn_with(out, Duration::from_secs(60));
        writer.send(frame(&["aaaa", "same line"]));
        term.wait_held();
        // Skipped: if the next frame were sent as a difference from this one, the terminal would end up with "aaad".
        writer.send(frame(&["cccc", "same line"]));
        writer.send(frame(&["cccd", "same line"]));
        term.let_through();
        term.wait_held();
        term.let_through();
        settled(&term);
        assert_eq!(term.screen().lines().map(str::trim_end).collect::<Vec<_>>(), ["cccd", "same line"]);
        let second = String::from_utf8_lossy(&term.writes()[1]).into_owned();
        assert!(!second.contains("same line"), "an unchanged line was sent again: {second:?}");
        assert!(!second.contains("\x1b[2J"), "the screen was cleared for a small change: {second:?}");
        // The same frame again: nothing changed on the terminal, but a write still happens only when sent.
        writer.send(frame(&["cccd", "same line"]));
        term.wait_held();
        term.let_through();
        settled(&term);
        let third = String::from_utf8_lossy(&term.writes()[2]).into_owned();
        assert!(!third.contains('c'), "cells already showing were sent again: {third:?}");
    }

    #[test]
    fn a_repaint_sends_every_cell_again_on_a_cleared_screen() {
        let (mut term, out) = Term::new();
        let writer = Writer::spawn_with(out, Duration::from_secs(60));
        writer.send(frame(&["first", "second"]));
        term.wait_held();
        term.let_through();
        settled(&term);
        writer.repaint();
        // Bytes for the terminal alone (a copy) don't use the repaint up: it is the next frame's.
        writer.bytes().send(b"\x1b]52;c;eA==\x07");
        term.wait_held();
        term.let_through();
        settled(&term);
        writer.send(frame(&["first", "second"]));
        term.wait_held();
        term.let_through();
        settled(&term);
        let again = String::from_utf8_lossy(&term.writes()[2]).into_owned();
        assert!(again.contains("\x1b[2J") && again.contains("first") && again.contains("second"), "{again:?}");
        // And only that one: the next is a difference again.
        writer.send(frame(&["first", "secone"]));
        term.wait_held();
        term.let_through();
        settled(&term);
        let next = String::from_utf8_lossy(&term.writes()[3]).into_owned();
        assert!(!next.contains("\x1b[2J") && !next.contains("first"), "{next:?}");
    }

    #[test]
    fn after_a_write_that_waited_long_the_screen_is_repainted_once_by_itself() {
        let (mut term, out) = Term::new();
        let writer = Writer::spawn_with(out, Duration::from_millis(200));
        writer.send(frame(&["before"]));
        term.wait_held();
        thread::sleep(Duration::from_millis(300)); // the terminal read nothing for a while
        term.let_through();
        // The writer sends the same frame again, every cell, with nothing new from the loop.
        term.wait_held();
        thread::sleep(Duration::from_millis(300)); // and this one waits long too
        term.let_through();
        assert_eq!(settled(&term), 2, "a repaint that waited long is not repeated");
        let again = String::from_utf8_lossy(&term.writes()[1]).into_owned();
        assert!(again.contains("\x1b[2J") && again.contains("before"), "{again:?}");
    }

    #[test]
    fn paused_it_finishes_the_write_under_way_and_writes_nothing_until_resumed() {
        let (mut term, out) = Term::new();
        let writer = Arc::new(Writer::spawn_with(out, Duration::from_secs(60)));
        writer.send(frame(&["one"]));
        term.wait_held();
        let paused = Arc::new(Mutex::new(false));
        let pausing = thread::spawn({
            let (writer, paused) = (writer.clone(), paused.clone());
            move || {
                writer.pause();
                *paused.lock().unwrap() = true;
            }
        });
        thread::sleep(Duration::from_millis(100));
        assert!(!*paused.lock().unwrap(), "pause returned while a write was under way");
        term.let_through();
        pausing.join().unwrap();
        // Paused: frames and bytes wait (the editor has the terminal).
        writer.send(frame(&["two"]));
        writer.bytes().send(b"\x1b]52;c;eA==\x07");
        assert_eq!(settled(&term), 1);
        writer.resume();
        term.wait_held();
        term.let_through();
        assert_eq!(settled(&term), 2);
        let after = String::from_utf8_lossy(&term.writes()[1]).into_owned();
        assert!(after.starts_with("\x1b]52;c;eA==\x07") && after.contains("two"), "{after:?}");
    }

    #[test]
    fn finishing_writes_the_last_frame_then_gives_the_terminal_back() {
        let (mut term, out) = Term::new();
        let writer = Writer::spawn_with(out, Duration::from_secs(60));
        writer.send(frame(&["last"]));
        let finishing = thread::spawn(move || writer.finish());
        term.wait_held();
        term.let_through();
        finishing.join().unwrap().unwrap();
        assert_eq!(term.screen().trim_end(), "last");
    }

    #[test]
    fn a_terminal_that_is_gone_is_said() {
        struct Gone;
        impl Write for Gone {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("the terminal went away"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let writer = Writer::spawn(Gone);
        assert!(writer.check().is_ok());
        writer.send(frame(&["x"]));
        wait_until(|| writer.check().is_err());
    }
}
