//! Live sharing of one session between windows, and `platoon open`. `sh` (or a tiny fake claude script)
//! stands in for claude via PLATOON_CLAUDE_BIN.

mod common;
use common::*;
use std::{
    fs,
    io::{ErrorKind, Read},
    os::unix::{fs::PermissionsExt, net::UnixStream, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn start_host(cache: &Path, program: &str) -> (u32, PathBuf) {
    let mut cmd = Command::new(BIN);
    cmd.env("PLATOON_NO_UPDATE_CHECK", "1"); // no version checks: never the network, never `claude --version`
    for (k, v) in common::NO_REAL_CODEX {
        cmd.env(k, v); // never the real Codex: its folder or its program
    }
    cmd.args(["host", "--program", program, "--dir", "/tmp", "--rows", "24", "--cols", "80", "--"])
        .env("PLATOON_CACHE_DIR", cache)
        // The host reads Claude's registry and platoon's settings: point them at nothing real.
        .env("CLAUDE_CONFIG_DIR", cache.join("claude"))
        .env("PLATOON_CONFIG_DIR", cache.join("config"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    // The host is our child: reap it whenever it exits, or it stays a zombie until the test binary ends.
    let mut host = cmd.spawn().unwrap();
    thread::spawn(move || host.wait());
    until("host to publish itself", || find_host(cache).is_some());
    find_host(cache).unwrap()
}

#[test]
fn two_windows_see_the_same_session_live_and_either_can_type() {
    let cache = tmp("share");
    let (pid, sock) = start_host(&cache, "sh");
    let mut a = Viewer::connect(&sock, 24, 80);
    let mut b = Viewer::connect(&sock, 24, 80);

    a.type_line("echo from-a-$((40+2))");
    b.wait_for("from-a-42"); // typed in one window, appears in the other

    b.type_line("echo from-b-$((50+5))");
    a.wait_for("from-b-55");

    // A window joining later gets the whole screen so far, not a blank one.
    let mut late = Viewer::connect(&sock, 24, 80);
    assert!(late.text().contains("from-a-42") && late.text().contains("from-b-55"), "{}", late.text());

    a.type_line("exit");
    until("host to exit", || !alive(pid));
}

#[test]
fn the_terminal_fits_the_smallest_window_and_grows_back_when_it_leaves() {
    let cache = tmp("size");
    let (pid, sock) = start_host(&cache, "sh");
    let mut big = Viewer::connect(&sock, 24, 80);
    big.retry("stty size", "24 80");

    let small = Viewer::connect(&sock, 20, 60);
    big.retry("stty size", "20 60"); // one terminal can only be one size: the smaller wins

    drop(small);
    big.retry("stty size", "24 80"); // and it grows back once that window is gone

    big.type_line("exit");
    until("host to exit", || !alive(pid));
}

#[test]
fn a_window_made_shorter_and_taller_again_shows_every_line_once_and_so_does_a_window_joining_then() {
    let cache = tmp("shrink");
    let (pid, sock) = start_host(&cache, "sh");
    let reap = Reap(pid);
    let mut w = Viewer::connect(&sock, 24, 80);
    w.type_line("printf 'row-%02d\\n' $(seq 1 15)");
    w.wait_for("row-15");

    // Shorter: the lines that no longer fit go above the screen, as in a terminal, and the newest stay (the prompt the
    // session waits at among them), so a window joining now is shown where the session is.
    w.resize(8, 80);
    w.type_line("stty size"); // read after the resize on the same connection: the host is 8 rows by then
    w.wait_for("8 80");
    let mut joined = Viewer::connect(&sock, 24, 80); // the smaller window keeps the session at 8 rows
    let shown = joined.text();
    assert!(shown.contains("row-15") && shown.contains("8 80") && !shown.contains("row-01"), "{shown}");
    joined.close();

    // Taller again: the lines that went above come back where they were, none lost and none twice.
    w.resize(24, 80);
    w.type_line("stty size");
    w.wait_for("24 80");
    let mut late = Viewer::connect(&sock, 24, 80);
    let shown = late.text();
    let at: Vec<usize> = (1..=15)
        .map(|i| {
            let row = format!("row-{i:02}");
            assert_eq!(shown.matches(&row).count(), 1, "{row} once:\n{shown}");
            shown.find(&row).unwrap()
        })
        .collect();
    assert!(at.is_sorted(), "in order:\n{shown}");
    assert!(shown.contains("8 80") && shown.contains("24 80"), "{shown}");

    drop(reap);
    until("host to exit", || !alive(pid));
}

#[test]
fn a_window_that_says_it_has_no_size_is_shown_the_session_and_breaks_nothing() {
    let cache = tmp("nosize");
    let (pid, sock) = start_host(&cache, "sh");
    let reap = Reap(pid);
    let mut w = Viewer::connect(&sock, 24, 80);
    w.type_line("echo before-$((40+2))");
    w.wait_for("before-42");

    // A terminal that reports no size: the session is made one row and one column for it rather than nothing, and the
    // window is sent the screen like any other (`connect` waits for it).
    let mut none = Viewer::connect(&sock, 0, 0);
    w.retry("stty size", "1 1");
    none.resize(0, 80);
    w.retry("stty size", "1 80");
    none.resize(24, 0);
    w.retry("stty size", "24 1");
    none.close();
    w.retry("stty size", "24 80"); // and back to the other window's size once it has gone

    let log = fs::read_to_string(cache.join("diagnostics.log")).unwrap_or_default();
    assert!(!log.contains("panic"), "{log}");
    drop(reap);
    until("host to exit", || !alive(pid));
}

/// Ends the stand-in a host of the test's runs (and so the host), even when the test fails.
struct Reap(u32);

impl Drop for Reap {
    fn drop(&mut self) {
        unsafe { libc::kill(self.0 as i32, libc::SIGKILL) };
    }
}

/// More output than any socket's buffer holds, and the line `flood-done-42` after it: typed into a shell session, it
/// is more than a window that doesn't read can be handed.
const FLOOD: &str = "yes platoon-flood | head -n 200000; echo flood-done-$((6*7))";

/// Read what `stream` was sent until the host closes it (true), or until nothing more comes for a while (false: the
/// connection is still open, with nothing on its way).
fn drained_to_the_end(stream: &mut UnixStream) -> bool {
    // macOS refuses a new timeout (EINVAL) on a socket the other end has already shut down; the reads below then see the
    // end at once, so the timeout set earlier is enough.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
    let start = Instant::now();
    let mut buf = [0u8; 1 << 16];
    while start.elapsed() < WAIT {
        match stream.read(&mut buf) {
            Ok(0) => return true,
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::ConnectionReset => return true,
            Err(_) => {} // nothing for a second: ask again until the time is up
        }
    }
    false
}

#[test]
fn a_window_that_stops_reading_is_cut_off_and_the_others_keep_getting_output() {
    let cache = tmp("stalled");
    let (pid, sock) = start_host(&cache, "sh");
    let reap = Reap(pid);
    // A window that takes its snapshot and then reads nothing more (stuck, or its computer stalled).
    let mut stalled = UnixStream::connect(&sock).unwrap();
    write_frame(&mut stalled, b'R', &[0, 24, 0, 80]).unwrap();
    stalled.set_read_timeout(Some(WAIT)).unwrap();
    read_frame(&mut stalled).expect("the snapshot");
    let mut live = Viewer::connect(&sock, 24, 80);

    live.type_line(FLOOD);
    live.wait_for("flood-done-42");
    // The host can't hand the stuck window what the session wrote: it closes its connection rather than keep it
    // connected and sent nothing ever again (a screen frozen with no sign of it).
    assert!(drained_to_the_end(&mut stalled), "the host kept a window it no longer feeds connected");

    // The window that reads was never held up by the other one.
    live.type_line("echo after-$((2+3))");
    live.wait_for("after-5");
    drop(reap);
    until("host to exit", || !alive(pid));
}

/// A stand-in claude: says how it was started, then gives a shell.
fn fake_claude(dir: &Path) -> PathBuf {
    let p = dir.join("fake-claude.sh");
    fs::write(&p, "#!/bin/sh\necho \"fake-claude args:$* cwd:$(pwd)\"\nexec sh\n").unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    p
}

struct World {
    claude: PathBuf,
    cache: PathBuf,
    work: PathBuf,
}

impl World {
    fn new(name: &str) -> Self {
        let base = tmp(name);
        let (claude, cache, work) = (base.join("claude"), base.join("cache"), base.join("work"));
        for d in [&claude, &cache, &work] {
            fs::create_dir_all(d).unwrap();
        }
        fs::create_dir_all(claude.join("sessions")).unwrap();
        Self { claude, cache, work }
    }

    fn env(&self, program: &str) -> Vec<(&'static str, String)> {
        vec![
            ("CLAUDE_CONFIG_DIR", self.claude.to_string_lossy().into()),
            ("PLATOON_CACHE_DIR", self.cache.to_string_lossy().into()),
            ("PLATOON_CONFIG_DIR", self.cache.join("config").to_string_lossy().into()),
            ("PLATOON_CLAUDE_BIN", program.into()),
            ("PLATOON_SKIP_SETUP_CHECK", "1".into()),
        ]
    }

    /// A plain shell standing in for claude (`shell_claude`).
    fn shell(&self) -> String {
        shell_claude(&self.claude).to_string_lossy().into_owned()
    }

    fn register(&self, pid: u32, id: &str, started: u64) {
        fs::write(
            self.claude.join("sessions").join(format!("{pid}.json")),
            format!(r#"{{"pid":{pid},"sessionId":"{id}","cwd":"/tmp","kind":"interactive","startedAt":{started},"status":"idle"}}"#),
        )
        .unwrap();
    }

    fn save_transcript(&self, id: &str) {
        let dir = self.claude.join("projects").join("-tmp-x");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("{id}.jsonl")),
            format!("{{\"type\":\"user\",\"cwd\":\"{}\",\"sessionId\":\"{id}\"}}\n", self.work.display()),
        )
        .unwrap();
    }
}

#[test]
fn platoon_open_starts_a_session_detaches_and_rejoins_it_by_id_with_the_screen_restored() {
    let w = World::new("open");
    let mut t = Term::spawn(&["open"], &w.env(&w.shell()), &w.work, 24, 80);
    t.send(b"echo first-$((6*7))\r");
    t.wait_for("first-42");
    t.send(&[0]); // Ctrl+Space
    assert_eq!(t.wait_exit(), 0);
    assert!(t.text().contains("detached") && t.text().contains("still running"), "{}", t.text());

    let (pid, _) = find_host(&w.cache).expect("the session must keep running after detaching");
    assert!(alive(pid));

    // Rejoin by a prefix of its id.
    w.register(pid, "abc-123-def", 1);
    let mut t2 = Term::spawn(&["open", "abc-1"], &w.env(&w.shell()), &w.work, 24, 80);
    t2.wait_for("first-42"); // restored from the host's snapshot, not typed again
    t2.send(b"echo second-$((2+2))\r");
    t2.wait_for("second-4");
    t2.send(&[0]);
    assert_eq!(t2.wait_exit(), 0);
    assert!(alive(pid), "detaching the second time must not end it either");

    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    until("shell to exit", || !alive(pid));
}

#[test]
fn platoon_open_connects_again_by_itself_when_the_host_cuts_it_off() {
    let w = World::new("open-cut");
    let mut t = Term::spawn(&["open"], &w.env(&w.shell()), &w.work, 24, 80);
    t.send(b"echo first-$((6*7))\r");
    t.wait_for("first-42");
    let (pid, sock) = find_host(&w.cache).expect("the session's host");
    let reap = Reap(pid);

    // The window stops reading for a while (as when its computer stalls) while the session writes more than it can be
    // handed: the host cuts it off. Another window sees the whole of it.
    let open = t.pid();
    unsafe { libc::kill(open as i32, libc::SIGSTOP) };
    let mut other = Viewer::connect(&sock, 24, 80);
    other.type_line(FLOOD);
    other.wait_for("flood-done-42");
    unsafe { libc::kill(open as i32, libc::SIGCONT) };

    // Back, it shows the session as it is now and what it writes from then on, without leaving and coming back.
    t.wait_for("flood-done-42");
    t.send(b"echo back-$((3+4))\r");
    t.wait_for("back-7");
    assert!(!t.text().contains("session ended"), "{}", t.text());
    t.send(&[0]); // Ctrl+Space
    assert_eq!(t.wait_exit(), 0);
    assert!(t.text().contains("detached"), "{}", t.text());
    assert!(alive(pid), "the session runs on");
    drop(reap);
    until("host to exit", || !alive(pid));
}

#[test]
fn platoon_connects_an_open_session_again_by_itself_when_the_host_cuts_it_off() {
    let w = World::new("app-cut");
    let (pid, sock) = start_host(&w.cache, "sh");
    let reap = Reap(pid);
    // The session's conversation, and the stand-in registered as Claude registers one.
    let dir = w.claude.join("projects").join("-work");
    fs::create_dir_all(&dir).unwrap();
    let line = format!(
        r#"{{"type":"user","cwd":"{}","timestamp":"2026-09-19T05:00:00.000Z","message":{{"role":"user","content":"tidy the widget"}}}}"#,
        w.work.display()
    );
    fs::write(dir.join("aaaa-0001.jsonl"), line + "\n").unwrap();
    w.register(pid, "aaaa-0001", 1);

    let mut t = Term::spawn(&[], &w.env(NO_REAL_CLAUDE.1), &w.work, 30, 110);
    t.wait_for("tidy the widget");
    t.send(b"\r");
    t.send(b"echo first-$((6*7))\r");
    t.wait_for("first-42");

    // platoon stops reading for a while (as when its computer stalls) while the session writes more than it can be
    // handed: the host cuts it off. Another window sees the whole of it.
    let app = t.pid();
    unsafe { libc::kill(app as i32, libc::SIGSTOP) };
    let mut other = Viewer::connect(&sock, 24, 80);
    other.type_line(FLOOD);
    other.wait_for("flood-done-42");
    unsafe { libc::kill(app as i32, libc::SIGCONT) };

    // Back, it shows the session as it is now and what it writes from then on, still in the session.
    t.wait_for("flood-done-42");
    t.send(b"echo back-$((3+4))\r");
    t.wait_for("back-7");
    assert!(!t.text().contains("Session ended"), "{}", t.text());
    drop(t);
    drop(reap);
    until("host to exit", || !alive(pid));
}

/// The line `flood-2-on`, a second later five seconds of random text, then the line `flood-over-42`: typed into a shell
/// session, it has the window draw a new screen, every cell of it different, all the while.
const LONG_FLOOD: &str = "echo flood-$((1+1))-on; sleep 1; end=$(($(date +%s)+5)); while [ $(date +%s) -lt $end ]; do head -c 4000 /dev/urandom | base64; done; echo flood-over-$((6*7))";

/// A world with one conversation, "tidy the widget", whose session is a shell in a host of the test's: (its pid, the
/// world).
fn world_with_a_session(name: &str) -> (u32, World) {
    let w = World::new(name);
    let (pid, _) = start_host(&w.cache, "sh");
    let dir = w.claude.join("projects").join("-work");
    fs::create_dir_all(&dir).unwrap();
    let line = format!(
        r#"{{"type":"user","cwd":"{}","timestamp":"2026-09-19T05:00:00.000Z","message":{{"role":"user","content":"tidy the widget"}}}}"#,
        w.work.display()
    );
    fs::write(dir.join("aaaa-0001.jsonl"), line + "\n").unwrap();
    w.register(pid, "aaaa-0001", 1);
    (pid, w)
}

#[test]
fn platoon_keeps_working_while_its_terminal_reads_nothing_and_shows_the_latest_screen_when_it_reads_again() {
    let (pid, w) = world_with_a_session("app-stall");
    let reap = Reap(pid);
    let mut t = Term::spawn(&[], &w.env(NO_REAL_CLAUDE.1), &w.work, 30, 110);
    t.wait_for("tidy the widget");
    assert!(t.raw().windows(8).any(|b| b == b"\x1b[?1004h"), "platoon asks to hear when its window gains focus");
    t.send(b"\r");
    t.send(b"echo first-$((6*7))\r");
    t.wait_for("first-42");

    // The session writes on and on, and the terminal stops reading (its window went behind another one): platoon's
    // screens soon fill what the terminal holds, and writing one more would wait.
    t.send(format!("{LONG_FLOOD}\r").as_bytes());
    t.wait_for("flood-2-on");
    t.stop_reading();
    thread::sleep(Duration::from_secs(2));
    // What is typed meanwhile still reaches the session (it runs once the flood is over).
    let typed = w.work.join("typed-while-not-read");
    t.send(format!("touch {}; echo typed-$((2+3))\r", typed.display()).as_bytes());
    until("what was typed while the terminal read nothing to reach the session", || typed.exists());

    // Reading again, the terminal shows the session as it is now.
    t.read_again();
    t.wait_for_all(&["flood-over-42", "typed-5"]);
    // The window coming back to the front is not typed into the session.
    t.send(b"\x1b[I");
    t.send(b"echo back-$((3+4))\r");
    t.wait_for("back-7");
    let shown = t.stable();
    assert!(!shown.contains("[I") && !shown.contains("Session ended"), "{shown}");
    // Quitting gives the terminal back as it was: no word of focus any more.
    t.send(&[0]); // Ctrl+Space
    t.wait_for("tidy the widget");
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);
    assert!(t.raw().windows(8).any(|b| b == b"\x1b[?1004l"), "platoon stops asking about focus as it quits");
    drop(reap);
    until("host to exit", || !alive(pid));
}

#[test]
fn platoon_open_shows_the_latest_screen_once_its_terminal_reads_again() {
    let w = World::new("open-stall");
    let mut t = Term::spawn(&["open"], &w.env(&w.shell()), &w.work, 24, 80);
    t.send(b"echo first-$((6*7))\r");
    t.wait_for("first-42");
    let (pid, _) = find_host(&w.cache).expect("the session's host");
    let reap = Reap(pid);

    // The terminal reads nothing for a while the session writes on: the window can't take it, and its host cuts it off.
    t.send(format!("{LONG_FLOOD}\r").as_bytes());
    t.wait_for("flood-2-on");
    t.stop_reading();
    thread::sleep(Duration::from_secs(2));
    t.read_again();

    // Reading again, it connects again by itself and shows the session as it is now.
    t.wait_for("flood-over-42");
    t.send(b"echo back-$((3+4))\r");
    t.wait_for("back-7");
    assert!(!t.text().contains("session ended"), "{}", t.text());
    t.send(&[0]); // Ctrl+Space
    assert_eq!(t.wait_exit(), 0);
    drop(reap);
    until("host to exit", || !alive(pid));
}

#[test]
fn platoon_open_resumes_a_saved_session_nothing_is_running_in_its_own_directory() {
    let w = World::new("resume");
    w.save_transcript("beef-0001-aaaa");
    let claude = fake_claude(&w.claude);
    let mut t = Term::spawn(&["open", "beef"], &w.env(&claude.to_string_lossy()), &w.work, 24, 100);
    t.wait_for_all(&["fake-claude args:--resume beef-0001-aaaa", &format!("cwd:{}", w.work.display())]);
    t.send(&[0]);
    t.wait_exit();
    if let Some((pid, _)) = find_host(&w.cache) {
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        until("shell to exit", || !alive(pid));
    }
}

#[test]
fn platoon_open_asks_before_ending_a_copy_running_in_a_plain_terminal() {
    let w = World::new("foreign");
    w.save_transcript("cafe-0002-bbbb");
    let claude = fake_claude(&w.claude);
    let mut plain = Command::new("sleep").arg("60").spawn().unwrap(); // a claude platoon doesn't own
    let plain_pid = plain.id();
    w.register(plain_pid, "cafe-0002-bbbb", 1);
    // Reap it when it dies, as the shell that started a real claude would.
    let reaper = thread::spawn(move || {
        let _ = plain.wait();
    });

    // Say no: nothing is touched.
    let mut t = Term::spawn(&["open", "cafe"], &w.env(&claude.to_string_lossy()), &w.work, 24, 100);
    t.wait_for("outside platoon");
    t.send(b"n\r");
    assert_eq!(t.wait_exit(), 1);
    assert!(alive(plain_pid), "answering no must leave the other copy alone");
    assert!(find_host(&w.cache).is_none());

    // Say yes: the other copy is ended and the session is reopened from its saved history.
    let mut t = Term::spawn(&["open", "cafe"], &w.env(&claude.to_string_lossy()), &w.work, 24, 100);
    t.wait_for("outside platoon");
    t.send(b"y\r");
    t.wait_for("fake-claude args:--resume cafe-0002-bbbb");
    until("the plain copy to be ended", || !alive(plain_pid));
    t.send(&[0]);
    t.wait_exit();
    if let Some((pid, _)) = find_host(&w.cache) {
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    }
    unsafe { libc::kill(plain_pid as i32, libc::SIGKILL) };
    let _ = reaper.join();
    thread::sleep(Duration::from_millis(100));
}

#[test]
fn platoon_open_explains_itself_when_it_cannot_find_the_session() {
    let w = World::new("missing");
    let mut t = Term::spawn(&["open", "zzzz"], &w.env(&w.shell()), &w.work, 24, 80);
    assert_eq!(t.wait_exit(), 1);
    assert!(t.text().contains("no session matches"), "{}", t.text());
}

#[test]
fn platoon_open_detaches_on_the_back_key_from_keys_json_and_passes_ctrl_space_through() {
    let w = World::new("open-keys");
    let config = w.cache.join("config");
    fs::create_dir_all(&config).unwrap();
    fs::write(config.join("keys.json"), r#"{ "back": "ctrl+q" }"#).unwrap();
    let mut t = Term::spawn(&["open"], &w.env(&w.shell()), &w.work, 24, 80);
    t.send(b"echo first-$((6*7))\r");
    t.wait_for("first-42");
    // Ctrl+Space (NUL) goes to the session now; the shell is still there to answer.
    t.send(&[0]);
    t.send(b"echo second-$((2+3))\r");
    t.wait_for("second-5");
    t.send(&[0x11]); // ctrl+q
    assert_eq!(t.wait_exit(), 0);
    assert!(t.text().contains("detached"), "{}", t.text());
    let (pid, _) = find_host(&w.cache).expect("the session keeps running after detaching");
    assert!(alive(pid));
    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    until("shell to exit", || !alive(pid));
}
