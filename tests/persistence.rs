//! Black-box tests of the "sessions outlive platoon" guarantee, using the real binary.
//! `sh` stands in for claude (via `--program` / PLATOON_CLAUDE_BIN).

mod common;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::{
    io::{self, Read, Write},
    os::unix::{net::UnixStream, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{channel, Receiver},
    thread,
    time::{Duration, Instant},
};

const BIN: &str = env!("CARGO_BIN_EXE_platoon");
const WAIT: Duration = Duration::from_secs(10);

fn cache(name: &str) -> PathBuf {
    let d = common::removed_at_exit(std::env::temp_dir().join(format!("platoon-it-{name}-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

fn until(what: &str, mut cond: impl FnMut() -> bool) {
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
fn find_host(cache: &Path) -> Option<(u32, PathBuf)> {
    let dir = cache.join("hosts");
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        if e.path().extension().and_then(|x| x.to_str()) == Some("json") {
            let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
            let pid = v["claude_pid"].as_u64()? as u32;
            return Some((pid, e.path().with_extension("sock")));
        }
    }
    None
}

// Same framing as src/host.rs: kind byte, u32 BE length, payload.
fn write_frame(w: &mut impl Write, kind: u8, payload: &[u8]) -> io::Result<()> {
    let mut head = [kind, 0, 0, 0, 0];
    head[1..].copy_from_slice(&(payload.len() as u32).to_be_bytes());
    w.write_all(&head)?;
    w.write_all(payload)?;
    w.flush()
}

fn read_frame(r: &mut impl Read) -> io::Result<(u8, Vec<u8>)> {
    let mut head = [0u8; 5];
    r.read_exact(&mut head)?;
    let mut buf = vec![0; u32::from_be_bytes([head[1], head[2], head[3], head[4]]) as usize];
    r.read_exact(&mut buf)?;
    Ok((head[0], buf))
}

struct Attached {
    stream: UnixStream,
    frames: Receiver<Vec<u8>>,
    screen: vt100::Parser,
}

impl Attached {
    fn connect(sock: &Path) -> Self {
        let stream = UnixStream::connect(sock).unwrap();
        let mut w = stream.try_clone().unwrap();
        write_frame(&mut w, b'R', &[0, 24, 0, 80]).unwrap();
        let (tx, frames) = channel();
        let mut r = stream.try_clone().unwrap();
        thread::spawn(move || {
            while let Ok((kind, payload)) = read_frame(&mut r) {
                if kind == b'O' && tx.send(payload).is_err() {
                    break;
                }
            }
        });
        Self { stream, frames, screen: vt100::Parser::new(24, 80, 100) }
    }

    fn type_line(&mut self, text: &str) {
        write_frame(&mut self.stream, b'I', format!("{text}\r").as_bytes()).unwrap();
    }

    fn wait_for(&mut self, text: &str) {
        let start = Instant::now();
        while start.elapsed() < WAIT {
            if self.screen.screen().contents().contains(text) {
                return;
            }
            if let Ok(b) = self.frames.recv_timeout(Duration::from_millis(100)) {
                self.screen.process(&b);
            }
        }
        panic!("never saw {text:?}; screen was:\n{}", self.screen.screen().contents());
    }
}

#[test]
fn host_outlives_clients_and_reattach_restores_the_screen() {
    let cache = cache("proto");
    let mut cmd = Command::new(BIN);
    cmd.args(["host", "--program", "sh", "--dir", "/tmp", "--rows", "24", "--cols", "80", "--"])
        .env("PLATOON_CACHE_DIR", &cache)
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
    let mut host = cmd.spawn().unwrap();
    until("host to publish itself", || find_host(&cache).is_some());
    let (pid, sock) = find_host(&cache).unwrap();

    let mut first = Attached::connect(&sock);
    first.type_line("echo persisted-$((40+2))");
    first.wait_for("persisted-42");
    drop(first); // "quit platoon"

    thread::sleep(Duration::from_millis(300));
    assert!(alive(pid), "the session died when its client disconnected");

    let mut second = Attached::connect(&sock);
    second.wait_for("persisted-42"); // restored from the snapshot, not re-typed
    second.type_line("echo still-typing-$((1+1))");
    second.wait_for("still-typing-2");
    second.type_line("exit");

    until("host to exit and clean up", || !alive(pid) && find_host(&cache).is_none());
    assert!(host.wait().unwrap().success());
}

#[test]
fn quitting_the_ui_leaves_the_session_running() {
    let cache = cache("ui");
    let pair = native_pty_system().openpty(PtySize { rows: 30, cols: 100, pixel_width: 0, pixel_height: 0 }).unwrap();
    let mut cmd = CommandBuilder::new(BIN);
    cmd.env("PLATOON_CACHE_DIR", &cache);
    // Nothing of the real ~/.claude or ~/.config/platoon is read or written.
    cmd.env("CLAUDE_CONFIG_DIR", cache.join("claude"));
    cmd.env("PLATOON_CONFIG_DIR", cache.join("config"));
    cmd.env("PLATOON_CLAUDE_BIN", "sh");
    cmd.env("PLATOON_SKIP_SETUP_CHECK", "1"); // this test is about sessions, not setup
    cmd.env("TERM", "xterm-256color");
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();
    let (tx, rx) = channel::<Vec<u8>>();
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut screen = vt100::Parser::new(30, 100, 0);
    let mut wait_for = |text: &str| {
        let start = Instant::now();
        while start.elapsed() < WAIT {
            if screen.screen().contents().contains(text) {
                return;
            }
            if let Ok(b) = rx.recv_timeout(Duration::from_millis(100)) {
                screen.process(&b);
            }
        }
        panic!("never saw {text:?}; screen was:\n{}", screen.screen().contents());
    };

    wait_for("platoon");
    writer.write_all(b"n").unwrap(); // new session
    wait_for("pick a directory");
    writer.write_all(b"\r").unwrap(); // first suggestion
    wait_for("back to list"); // attached to the hosted shell
    writer.write_all(b"echo ui-$((6*7))\r").unwrap();
    wait_for("ui-42");
    writer.write_all(&[0]).unwrap(); // ctrl+space: back to the list
    wait_for("enter open");
    writer.write_all(b"q").unwrap();
    wait_for("still running in the background");
    child.wait().unwrap();

    let (pid, _) = find_host(&cache).expect("host record vanished when platoon quit");
    assert!(alive(pid), "the session ended when platoon quit");
    unsafe { libc::kill(pid as i32, libc::SIGKILL) }; // tidy up (interactive sh ignores SIGTERM)
    until("shell to exit", || !alive(pid));
}

/// Coming back to platoon while one of its sessions still runs: Enter pressed the moment the row shows must attach
/// to platoon's own host. Before platoon knew its hosts from the first frame, it took its own session for a copy
/// running in some other terminal for up to a second, and offered to take it over (which would have ended it).
#[test]
fn enter_right_after_starting_attaches_to_a_session_platoon_already_hosts() {
    let base = cache("reopen");
    let (claude, work) = (base.join("claude"), base.join("work"));
    for d in [claude.join("sessions"), claude.join("projects/-work"), work.clone()] {
        std::fs::create_dir_all(d).unwrap();
    }
    let line = format!(
        r#"{{"type":"user","cwd":"{}","gitBranch":"main","timestamp":"2026-09-19T05:00:00.000Z","message":{{"role":"user","content":"refactor the widget"}}}}"#,
        work.display()
    );
    std::fs::write(claude.join("projects/-work/aaaa-0001.jsonl"), line + "\n").unwrap();
    let env = [
        ("CLAUDE_CONFIG_DIR", claude.to_string_lossy().into_owned()),
        ("PLATOON_CACHE_DIR", base.join("cache").to_string_lossy().into_owned()),
        ("PLATOON_CONFIG_DIR", base.join("config").to_string_lossy().into_owned()),
        ("PLATOON_CLAUDE_BIN", "/nonexistent/claude-is-never-started-here".into()),
        ("PLATOON_SKIP_SETUP_CHECK", "1".into()),
    ];
    std::fs::create_dir_all(base.join("config")).unwrap();
    std::fs::write(base.join("config/settings.json"), r#"{"auto_archive_days":0}"#).unwrap();

    // The session a previous platoon started and left running: a host of ours, with a stand-in for claude inside,
    // registered the way Claude registers a session.
    let mut cmd = Command::new(BIN);
    cmd.args(["host", "--program", "sh", "--dir"])
        .arg(&work)
        .args(["--rows", "24", "--cols", "80", "--", "-c", "echo reopened-$((40+2)); exec sleep 60"])
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut host = cmd.spawn().unwrap();
    until("host to publish itself", || find_host(&base.join("cache")).is_some());
    let (pid, _) = find_host(&base.join("cache")).unwrap();
    // Ends the stand-in our own host started (and so the host), even when the test fails.
    struct Reap(u32);
    impl Drop for Reap {
        fn drop(&mut self) {
            unsafe { libc::kill(self.0 as i32, libc::SIGKILL) };
        }
    }
    let reap = Reap(pid);
    let reg = serde_json::json!({"pid": pid, "sessionId": "aaaa-0001", "cwd": work, "kind": "interactive", "startedAt": 1, "status": "idle"});
    std::fs::write(claude.join("sessions").join(format!("{pid}.json")), reg.to_string()).unwrap();

    let mut t = common::Term::spawn(&[], &env, &work, 30, 110);
    t.wait_for("refactor the widget");
    t.send(b"\r");
    t.wait_for("reopened-42");
    assert!(!t.text().to_lowercase().contains("take over"), "{}", t.text());
    drop(t);

    drop(reap);
    until("stand-in to exit", || !alive(pid));
    let _ = host.kill();
    let _ = host.wait();
}
