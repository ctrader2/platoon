//! Live sharing of one session between windows, and `platoon open`. `sh` (or a tiny fake claude script)
//! stands in for claude via PLATOON_CLAUDE_BIN.

mod common;
use common::*;
use std::{
    fs,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

fn start_host(cache: &Path, program: &str) -> (u32, PathBuf) {
    let mut cmd = Command::new(BIN);
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
    let mut t = Term::spawn(&["open"], &w.env("sh"), &w.work, 24, 80);
    t.send(b"echo first-$((6*7))\r");
    t.wait_for("first-42");
    t.send(&[0]); // Ctrl+Space
    assert_eq!(t.wait_exit(), 0);
    assert!(t.text().contains("detached") && t.text().contains("still running"), "{}", t.text());

    let (pid, _) = find_host(&w.cache).expect("the session must keep running after detaching");
    assert!(alive(pid));

    // Rejoin by a prefix of its id.
    w.register(pid, "abc-123-def", 1);
    let mut t2 = Term::spawn(&["open", "abc-1"], &w.env("sh"), &w.work, 24, 80);
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
fn platoon_open_resumes_a_saved_session_nothing_is_running_in_its_own_directory() {
    let w = World::new("resume");
    w.save_transcript("beef-0001-aaaa");
    let claude = fake_claude(&w.claude);
    let mut t = Term::spawn(&["open", "beef"], &w.env(&claude.to_string_lossy()), &w.work, 24, 100);
    t.wait_for("fake-claude args:--resume beef-0001-aaaa");
    assert!(t.text().contains(&format!("cwd:{}", w.work.display())), "{}", t.text());
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
    let mut t = Term::spawn(&["open", "zzzz"], &w.env("sh"), &w.work, 24, 80);
    assert_eq!(t.wait_exit(), 1);
    assert!(t.text().contains("no session matches"), "{}", t.text());
}

#[test]
fn platoon_open_detaches_on_the_back_key_from_keys_json_and_passes_ctrl_space_through() {
    let w = World::new("open-keys");
    let config = w.cache.join("config");
    fs::create_dir_all(&config).unwrap();
    fs::write(config.join("keys.json"), r#"{ "back": "ctrl+q" }"#).unwrap();
    let mut t = Term::spawn(&["open"], &w.env("sh"), &w.work, 24, 80);
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
