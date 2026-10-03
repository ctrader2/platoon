//! `platoon uninstall`, end to end, each in a made-up world: HOME, every folder variable and PATH point into a temp
//! folder. `PLATOON_UNINSTALL_TEST_HOME` makes the command refuse to run anywhere else.

mod common;

use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
};

const BIN: &str = env!("CARGO_BIN_EXE_platoon");

struct World {
    base: PathBuf,
    home: PathBuf,
    /// Stand-in programs, first on PATH.
    stubs: PathBuf,
}

impl World {
    fn new(name: &str) -> Self {
        let base = common::tmp(&format!("uninstall-{name}"));
        let (home, stubs) = (base.join("home"), base.join("stubs"));
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::create_dir_all(&stubs).unwrap();
        Self { base, home, stubs }
    }

    fn claude(&self) -> PathBuf {
        self.home.join(".claude")
    }
    fn config(&self) -> PathBuf {
        self.home.join(".config/platoon")
    }
    fn cache(&self) -> PathBuf {
        self.home.join(".cache/platoon")
    }

    /// A command with nothing of the real environment: only this world's folders, and `answers` (None: no terminal).
    fn command(&self, program: &str, extra_path: &[&Path], answers: Option<&str>) -> Command {
        let mut path: Vec<String> = extra_path.iter().map(|p| p.display().to_string()).collect();
        path.push(self.stubs.display().to_string());
        path.push("/usr/bin:/bin".into());
        let tty = self.base.join("answers");
        match answers {
            Some(a) => fs::write(&tty, a).unwrap(),
            None => {
                let _ = fs::remove_file(&tty);
            }
        }
        let mut c = Command::new(program);
        c.env_clear()
            .env("HOME", &self.home)
            .env("PATH", path.join(":"))
            .env("LANG", "C")
            .env("PLATOON_UNINSTALL_TEST_HOME", &self.base)
            .env("PLATOON_UNINSTALL_TTY", &tty)
            .env("PLATOON_NO_UPDATE_CHECK", "1")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null());
        c
    }

    /// `platoon setup` with the real binary, as a user would, on top of `user`'s settings.
    fn set_up(&self, user: &Value) {
        fs::write(self.claude().join("settings.json"), serde_json::to_string_pretty(user).unwrap()).unwrap();
        let out = self.command(BIN, &[], None).arg("setup").output().unwrap();
        assert!(out.status.success(), "{}", text(&out));
        fs::create_dir_all(self.config().join("projects")).unwrap();
        fs::create_dir_all(self.cache().join("status")).unwrap();
    }

    fn settings(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.claude().join("settings.json")).unwrap()).unwrap()
    }
}

fn text(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

fn user_settings() -> Value {
    json!({
        "model": "opus",
        "statusLine": null,
        "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "notify-send done"}]}]},
        "permissions": {"allow": ["Bash(ls)"]}
    })
}

/// A stand-in "claude" this test started (never anyone else's), reaped as soon as it ends so it doesn't linger as a
/// zombie that still looks alive.
fn sleeper() -> (u32, std::thread::JoinHandle<std::process::ExitStatus>) {
    let mut c: Child = Command::new("sleep").arg("60").stdin(Stdio::null()).spawn().unwrap();
    let pid = c.id();
    (pid, std::thread::spawn(move || c.wait().unwrap()))
}

fn end(pid: u32, waiter: std::thread::JoinHandle<std::process::ExitStatus>) -> Option<i32> {
    if common::alive(pid) {
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    }
    use std::os::unix::process::ExitStatusExt;
    waiter.join().unwrap().signal()
}

fn host_entry(w: &World, pid: u32) {
    let hosts = w.cache().join("hosts");
    fs::create_dir_all(&hosts).unwrap();
    fs::write(
        hosts.join(format!("{pid}.json")),
        json!({"claude_pid": pid, "host_pid": pid, "dir": "/work/app", "args": ["--resume", "sess-42"]}).to_string(),
    )
    .unwrap();
}

// ---- platoon uninstall, the real binary --------------------------------------------------------------------------

#[test]
fn uninstall_takes_out_exactly_what_the_real_setup_added_and_a_second_run_finds_nothing() {
    let w = World::new("roundtrip");
    let mut user = user_settings();
    user.as_object_mut().unwrap().remove("statusLine");
    w.set_up(&user);
    let set_up = fs::read_to_string(w.claude().join("settings.json")).unwrap();
    assert!(set_up.contains("hook guard") && set_up.contains("hook session-start") && set_up.contains(" statusline"), "{set_up}");
    assert!(w.claude().join("skills/to-tickets/SKILL.md").exists());

    let out = w.command(BIN, &[], Some("n\n")).arg("uninstall").output().unwrap();
    let said = text(&out);
    assert_eq!(out.status.code(), Some(0), "{said}");
    assert_eq!(w.settings(), user, "{said}");
    assert_eq!(fs::read_to_string(w.claude().join("settings.json.platoon-bak")).unwrap(), set_up);
    assert!(!w.claude().join("skills/to-tickets").exists() && !w.cache().exists() && w.config().exists(), "{said}");
    for want in [
        "Also delete your projects, notes, tickets and settings? [y/N]",
        "Removed:",
        "the status line, the session guard hook, the project notes hook, the notes updates hook",
        "the /to-tickets skill",
        "platoon's cache folder",
        "Kept:",
        "To delete them later: rm -rf",
        "your conversations: they are Claude Code's",
        "Last step, remove the program: rm ",
    ] {
        assert!(said.contains(want), "{want}: {said}");
    }

    let again = w.command(BIN, &[], None).arg("uninstall").output().unwrap();
    let said = text(&again);
    assert_eq!(again.status.code(), Some(0), "{said}");
    assert!(said.contains("Nothing of platoon's was left to remove.") && !said.contains("Removed:"), "{said}");
    assert!(said.contains("there was no terminal to ask in; --all deletes it"), "{said}");
    assert_eq!(w.settings(), user);
}

#[test]
fn uninstall_refuses_to_run_outside_the_test_home_and_rejects_unknown_flags() {
    let w = World::new("guard");
    w.set_up(&json!({}));
    let before = fs::read_to_string(w.claude().join("settings.json")).unwrap();
    let out = w.command(BIN, &[], None).arg("uninstall").env("PLATOON_CACHE_DIR", "/var/tmp/not-here").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out).contains("refusing to run"), "{}", text(&out));
    let out = w.command(BIN, &[], None).args(["uninstall", "--yes"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out).contains("don't know \"--yes\"") && text(&out).contains("--stop-sessions"), "{}", text(&out));
    assert_eq!(fs::read_to_string(w.claude().join("settings.json")).unwrap(), before, "nothing changed");
    let help = w.command(BIN, &[], None).args(["uninstall", "--help"]).output().unwrap();
    assert!(help.status.success() && text(&help).contains("--all"));
}

#[test]
fn uninstall_stops_running_sessions_only_after_a_yes() {
    let w = World::new("sessions");
    w.set_up(&json!({}));
    let (pid, waiter) = sleeper();
    host_entry(&w, pid);
    // No, then no terminal at all: nothing changes, the session runs on.
    for answers in [Some("n\n"), None] {
        let out = w.command(BIN, &[], answers).args(["uninstall", "--all"]).output().unwrap();
        let said = text(&out);
        assert_eq!(out.status.code(), Some(1), "{said}");
        assert!(said.contains(&format!("in /work/app (pid {pid}): `claude --resume sess-42` opens it again")), "{said}");
        assert!(said.contains("Nothing was changed") || said.contains("changed nothing"), "{said}");
        assert!(common::alive(pid) && w.config().exists() && w.settings().to_string().contains("platoon"));
    }
    let out = w.command(BIN, &[], Some("y\n")).args(["uninstall", "--all"]).output().unwrap();
    let said = text(&out);
    assert_eq!(out.status.code(), Some(0), "{said}");
    assert!(said.contains("Stop it and go on? [y/N]") && said.contains("stopped (their conversations are saved)"), "{said}");
    assert!(!said.contains("Also delete"), "--all doesn't ask: {said}");
    assert_eq!(end(pid, waiter), Some(libc::SIGTERM), "stopped the gentle way");
    assert!(!w.config().exists() && !w.cache().exists());
}
