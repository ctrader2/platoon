//! Codex's task hooks as the real binary runs them: installed for every Codex session (`platoon setup`), they end at
//! once and say nothing in a session that is no task's (no `PLATOON_TASK`), without even reading what Codex sends; in a
//! task's session the first one writes down the session's id, and a push is held back for the user. Every folder is a
//! temp one; no real Codex or Claude Code is run.

mod common;
use common::*;
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const HOOKS: [&str; 6] = ["task-start", "task-before", "task-notify", "task-tool", "task-prompt", "task-stop"];

struct Lab {
    config: PathBuf,
    cache: PathBuf,
    claude: PathBuf,
}

impl Lab {
    fn new(name: &str) -> Lab {
        let base = tmp(name);
        let lab = Lab { config: base.join("config"), cache: base.join("cache"), claude: base.join("claude") };
        for d in [&lab.config, &lab.cache, &lab.claude] {
            fs::create_dir_all(d).unwrap();
        }
        // One project with one task, started on Codex: its session not known yet.
        let v = json!({"version": 1, "projects": {"pweb": {"name": "Web", "color": "blue", "badge": "WB", "archived": false, "created": 1, "archived_sessions": []}}, "sessions": {}});
        fs::write(lab.config.join("projects.json"), v.to_string()).unwrap();
        fs::create_dir_all(lab.config.join("projects/pweb/tasks")).unwrap();
        fs::write(lab.config.join("projects/pweb/tasks/01-basket.md"), "Add a basket.\n").unwrap();
        let tasks = json!({"next": 2, "tasks": [{"number": 1, "title": "Basket", "file": "01-basket.md", "state": "working",
            "squad": "feature", "session": "pending:1:pweb", "created": 1, "updated": 1}]});
        fs::write(lab.config.join("projects/pweb/tasks.json"), tasks.to_string()).unwrap();
        lab
    }

    fn cmd(&self, name: &str, task: Option<&str>) -> Command {
        let mut c = Command::new(BIN);
        c.args(["hook", name, "--agent", "codex"])
            .envs(NO_REAL_CODEX)
            .env("CLAUDE_CONFIG_DIR", &self.claude)
            .env("PLATOON_CACHE_DIR", &self.cache)
            .env("PLATOON_CONFIG_DIR", &self.config)
            .env("PLATOON_SKIP_SETUP_CHECK", "1")
            .env("PLATOON_NO_UPDATE_CHECK", "1")
            .env_remove("PLATOON_TASK")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(t) = task {
            c.env("PLATOON_TASK", t);
        }
        c
    }

    /// The hook run in the task's session with `input`: what it prints.
    fn hook(&self, name: &str, input: Value) -> String {
        let mut child = self.cmd(name, Some("1:pweb")).spawn().unwrap();
        child.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{name}: {out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn task(&self) -> Value {
        let v: Value = serde_json::from_str(&fs::read_to_string(self.config.join("projects/pweb/tasks.json")).unwrap()).unwrap();
        v["tasks"][0].clone()
    }
}

#[test]
fn in_a_session_that_is_no_tasks_each_codex_task_hook_ends_at_once_saying_nothing() {
    let lab = Lab::new("codex-hooks-idle");
    let before = fs::read_to_string(lab.config.join("projects/pweb/tasks.json")).unwrap();
    for name in HOOKS {
        let started = Instant::now();
        // Its input is never even read: stdin stays open, and it ends all the same.
        let mut child = lab.cmd(name, None).spawn().unwrap();
        let stdin = child.stdin.take();
        let status = loop {
            if let Some(s) = child.try_wait().unwrap() {
                break s;
            }
            assert!(started.elapsed() < Duration::from_secs(5), "{name} waited");
            std::thread::sleep(Duration::from_millis(5));
        };
        drop(stdin);
        let out = child.wait_with_output().unwrap();
        assert!(status.success() && out.stdout.is_empty() && out.stderr.is_empty(), "{name}: {out:?}");
    }
    assert_eq!(fs::read_to_string(lab.config.join("projects/pweb/tasks.json")).unwrap(), before, "nothing written");
}

#[test]
fn in_a_tasks_session_the_first_hook_names_the_session_and_a_push_waits_for_the_user() {
    let lab = Lab::new("codex-hooks-task");
    let id = "01a1aaaa-0000-7000-8000-000000000001";
    let base = json!({"session_id": id, "turn_id": "t", "transcript_path": null, "cwd": "/w", "model": "gpt-6-luna", "permission_mode": "default"});
    let with = |extra: Value| {
        let mut v = base.clone();
        for (k, x) in extra.as_object().unwrap() {
            v.as_object_mut().unwrap().insert(k.clone(), x.clone());
        }
        v
    };
    assert_eq!(lab.hook("task-start", with(json!({"hook_event_name": "SessionStart", "source": "startup"}))), "");
    assert_eq!(lab.task()["session"], id);
    let push = with(
        json!({"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": "git push origin task/1"}, "tool_use_id": "c"}),
    );
    let said: Value = serde_json::from_str(lab.hook("task-before", push.clone()).trim()).unwrap();
    assert_eq!(said["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    assert_eq!(said["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(said["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().contains("waits for the user"), "{said}");
    assert_eq!((lab.task()["state"].as_str(), lab.task()["need"].as_str()), (Some("needs_you"), Some("question")));
    // The user says yes in the session: the same command goes through, once.
    assert_eq!(lab.hook("task-prompt", with(json!({"hook_event_name": "UserPromptSubmit", "prompt": "yes"}))), "");
    assert_eq!(lab.task()["state"], "working");
    assert_eq!(lab.hook("task-before", push.clone()), "");
    assert!(lab.hook("task-before", push).contains("\"deny\""));
    let reading =
        with(json!({"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": "git log -3"}, "tool_use_id": "c"}));
    assert_eq!(lab.hook("task-before", reading), "");
}

#[test]
fn with_codex_switched_off_a_tasks_hooks_end_at_once_saying_nothing_and_changing_nothing() {
    let lab = Lab::new("codex-hooks-off");
    // Codex installed (its folder is there) beside Claude Code, and switched off in platoon's settings.
    let codex = lab.config.parent().unwrap().join("codex");
    fs::create_dir_all(&codex).unwrap();
    fs::write(lab.config.join("settings.json"), r#"{"use_codex": false}"#).unwrap();
    let id = "01a1aaaa-0000-7000-8000-000000000002";
    let input = json!({"session_id": id, "turn_id": "t", "transcript_path": null, "cwd": "/w", "model": "gpt-6-luna",
        "permission_mode": "default", "hook_event_name": "SessionStart", "source": "startup"});
    let run = |name: &str| {
        let started = Instant::now();
        let mut child = lab.cmd(name, Some("1:pweb")).env("CODEX_HOME", &codex).env(NO_REAL_CLAUDE.0, NO_REAL_CLAUDE.1).spawn().unwrap();
        // Ended without reading anything: it doesn't wait for its input.
        let stdin = child.stdin.take();
        let status = loop {
            if let Some(s) = child.try_wait().unwrap() {
                break s;
            }
            assert!(started.elapsed() < Duration::from_secs(5), "{name} waited");
            std::thread::sleep(Duration::from_millis(5));
        };
        if let Some(mut stdin) = stdin {
            let _ = stdin.write_all(input.to_string().as_bytes());
        }
        let out = child.wait_with_output().unwrap();
        (status, out)
    };
    let tasks = || fs::read_to_string(lab.config.join("projects/pweb/tasks.json")).unwrap();
    let (before, cache_before) = (tasks(), fs::read_dir(&lab.cache).unwrap().count());
    for name in HOOKS {
        let (status, out) = run(name);
        assert!(status.success() && out.stdout.is_empty() && out.stderr.is_empty(), "{name}: {out:?}");
    }
    assert_eq!(tasks(), before, "the task is as it was: its session still not known");
    assert_eq!(fs::read_dir(&lab.cache).unwrap().count(), cache_before, "nothing written");
    // Switched on again, the first hook names the session as ever.
    fs::write(lab.config.join("settings.json"), r#"{"use_codex": true}"#).unwrap();
    let mut child =
        lab.cmd("task-start", Some("1:pweb")).env("CODEX_HOME", &codex).env(NO_REAL_CLAUDE.0, NO_REAL_CLAUDE.1).spawn().unwrap();
    child.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
    assert!(child.wait_with_output().unwrap().status.success());
    assert_eq!(lab.task()["session"], id);
}
