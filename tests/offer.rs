//! The offer to make tasks, end to end with the real binary: the hook that holds back the first start of work once in a
//! session platoon started (`platoon hook offer-tasks` for Claude Code, given with `--settings`; Codex's `hook
//! task-before --agent codex`), fed stand-in inputs, and a session `platoon open` starts with that hook, which a
//! stand-in `claude` runs as Claude Code would. Every folder is a temp dir; no real `claude` or `codex` runs.

mod common;
use common::*;
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const SAMPLES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/testdata/codex/hooks");

struct World {
    base: PathBuf,
    cache: PathBuf,
    config: PathBuf,
    codex: PathBuf,
    work: PathBuf,
}

impl World {
    fn new(name: &str) -> Self {
        let base = tmp(name);
        let w = Self { cache: base.join("cache"), config: base.join("config"), codex: base.join("codex"), work: base.join("work"), base };
        for d in [&w.cache, &w.config, &w.codex, &w.work, &w.base.join("claude/sessions")] {
            fs::create_dir_all(d).unwrap();
        }
        w
    }

    fn env(&self) -> Vec<(&'static str, String)> {
        let at = |p: &Path| p.to_string_lossy().into_owned();
        vec![
            ("CLAUDE_CONFIG_DIR", at(&self.base.join("claude"))),
            ("PLATOON_CACHE_DIR", at(&self.cache)),
            ("PLATOON_CONFIG_DIR", at(&self.config)),
            (NO_REAL_CLAUDE.0, NO_REAL_CLAUDE.1.into()),
            // Codex installed (its folder is there), its program one that can't exist.
            ("CODEX_HOME", at(&self.codex)),
            ("PLATOON_CODEX_BIN", NO_REAL_CODEX[1].1.into()),
            ("PLATOON_NO_UPDATE_CHECK", "1".into()),
        ]
    }

    /// Run `platoon <args>` with `input` on stdin, the world's environment and `extra`: (exit code, stdout).
    fn hook(&self, extra: &[(&str, &str)], args: &[&str], input: &str) -> (i32, String) {
        let mut child = Command::new(BIN)
            .args(args)
            .envs(self.env())
            .env_remove("PLATOON_OFFER_TASKS")
            .env_remove("PLATOON_TASK")
            .env_remove("PLATOON_PROJECT")
            .envs(extra.iter().copied())
            .current_dir(&self.work)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        // A hook that doesn't offer ends without reading its input: the pipe it never read is no failure.
        match child.stdin.take().unwrap().write_all(input.as_bytes()) {
            Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => panic!("couldn't send the hook its input: {e}"),
            _ => {}
        }
        let out = child.wait_with_output().unwrap();
        (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn claude_input(&self, session: &str, tool: &str, tool_input: serde_json::Value) -> String {
        serde_json::json!({"session_id": session, "cwd": self.work, "hook_event_name": "PreToolUse", "tool_name": tool,
            "tool_input": tool_input})
        .to_string()
    }

    fn mark(&self, session: &str) -> PathBuf {
        self.cache.join("offer-asked").join(session)
    }
}

const OFFER: (&str, &str) = ("PLATOON_OFFER_TASKS", "1");

/// The refusal's reason, checking its shape.
fn reason(out: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(out).unwrap_or_else(|e| panic!("{e}: {out}"));
    let o = &v["hookSpecificOutput"];
    assert_eq!((o["hookEventName"].as_str(), o["permissionDecision"].as_str()), (Some("PreToolUse"), Some("deny")), "{out}");
    o["permissionDecisionReason"].as_str().unwrap().to_string()
}

#[test]
fn claude_codes_hook_holds_back_the_first_start_of_work_once_in_a_session_platoon_started() {
    let w = World::new("offer-claude");
    let edit = w.claude_input("sess-1", "Edit", serde_json::json!({"file_path": "src/main.rs", "old_string": "a", "new_string": "b"}));
    let hook = ["hook", "offer-tasks"];
    // Not a session platoon started, or a task's own: nothing, at once, and no mark.
    assert_eq!(w.hook(&[], &hook, &edit), (0, String::new()));
    assert_eq!(w.hook(&[OFFER, ("PLATOON_TASK", "pweb/3")], &hook, &edit), (0, String::new()));
    // A shell command is not the start of the work.
    let shell = w.claude_input("sess-1", "Bash", serde_json::json!({"command": "cargo test"}));
    assert_eq!(w.hook(&[OFFER], &hook, &shell), (0, String::new()));
    assert!(!w.mark("sess-1").exists());
    // The first edit: held back, with how to make the tasks by this platoon's full path.
    let (code, out) = w.hook(&[OFFER], &hook, &edit);
    assert_eq!(code, 0);
    let why = reason(&out);
    assert!(why.starts_with("platoon is asking, not an error."), "{why}");
    assert!(why.contains("AskUserQuestion") && why.contains("\"Turn this into platoon tasks for a squad?\""), "{why}");
    assert!(why.contains(&format!("{BIN} task add --folder {} --squad <squad>", w.work.display())), "{why}");
    assert!(w.mark("sess-1").exists());
    // After that everything goes through, whatever the answer was.
    assert_eq!(w.hook(&[OFFER], &hook, &edit), (0, String::new()));
    let agent = w.claude_input("sess-1", "Agent", serde_json::json!({"prompt": "go"}));
    assert_eq!(w.hook(&[OFFER], &hook, &agent), (0, String::new()));
    // Off in the settings: another session's first step goes through, and it is asked once the setting is on again.
    let other = w.claude_input("sess-2", "Agent", serde_json::json!({"prompt": "go"}));
    fs::write(w.config.join("settings.json"), r#"{"offer_tasks": false}"#).unwrap();
    assert_eq!(w.hook(&[OFFER], &hook, &other), (0, String::new()));
    fs::write(w.config.join("settings.json"), r#"{"offer_tasks": true, "use_claude_code": false}"#).unwrap();
    assert_eq!(w.hook(&[OFFER], &hook, &other), (0, String::new()), "Claude Code switched off");
    fs::write(w.config.join("settings.json"), "{}").unwrap();
    assert!(!reason(&w.hook(&[OFFER], &hook, &other).1).is_empty());
}

#[test]
fn codexs_before_tool_hook_holds_back_its_first_edit_once_in_a_session_platoon_started() {
    let w = World::new("offer-codex");
    let cwd = w.work.to_string_lossy().into_owned();
    let sample = |name: &str| fs::read_to_string(Path::new(SAMPLES).join(format!("{name}.json"))).unwrap().replace("/work/repo", &cwd);
    let (patch, shell) = (sample("PreToolUse-apply_patch.from-source"), sample("PreToolUse-Bash.from-source"));
    let hook = ["hook", "task-before", "--agent", "codex"];
    assert_eq!(w.hook(&[], &hook, &patch), (0, String::new()), "a Codex session platoon didn't start");
    assert_eq!(w.hook(&[OFFER], &hook, &shell), (0, String::new()), "a shell command");
    let why = reason(&w.hook(&[OFFER], &hook, &patch).1);
    assert!(why.contains("Ask at the end of your reply, in plain words") && !why.contains("AskUserQuestion"), "{why}");
    assert!(why.contains(&format!("{BIN} task add --folder {cwd} --squad <squad>")), "{why}");
    assert_eq!(w.hook(&[OFFER], &hook, &patch), (0, String::new()), "once");
    // Codex switched off in platoon's settings (Claude Code is installed beside it): nothing.
    let mut other: serde_json::Value = serde_json::from_str(&patch).unwrap();
    other["session_id"] = "01a15181-dc6f-7b73-81cf-00000000beef".into();
    fs::write(w.config.join("settings.json"), r#"{"use_codex": false}"#).unwrap();
    assert_eq!(w.hook(&[OFFER], &hook, &other.to_string()), (0, String::new()));
    assert!(!w.cache.join("offer-asked/01a15181-dc6f-7b73-81cf-00000000beef").exists());
}

/// A stand-in `claude` that does what Claude Code does with the offer's `--settings` hook: runs its command with a
/// first edit's input, twice (the answers in `<out>/first.json` and `second.json`), then says so and waits.
fn hooking_claude(dir: &Path, out: &Path) -> PathBuf {
    let p = dir.join("hooking-claude.sh");
    let script = format!(
        r#"#!/bin/sh
settings=
while [ $# -gt 0 ]; do
  if [ "$1" = --settings ]; then settings=$2; fi
  shift
done
O='{out}'
if [ -n "$settings" ]; then
  cmd=$(printf '%s' "$settings" | sed 's/.*"command":"\([^"]*\)".*/\1/')
  input='{{"session_id":"live-1","cwd":"'"$(pwd)"'","hook_event_name":"PreToolUse","tool_name":"Write","tool_input":{{"file_path":"src/new.rs","content":""}}}}'
  printf '%s' "$input" | sh -c "$cmd" > "$O/first.json"
  printf '%s' "$input" | sh -c "$cmd" > "$O/second.json"
fi
echo "fake-claude offer:${{PLATOON_OFFER_TASKS:-none}} settings:${{settings:+yes}}"
exec sleep 60
"#,
        out = out.display()
    );
    fs::write(&p, script).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    p
}

/// Start `platoon open` in the world's folder with the hooking stand-in, wait for `want`, then detach and end the
/// session it started (only that process).
fn open_and_end(w: &World, want: &str) {
    let claude = hooking_claude(&w.base, &w.base);
    let mut env = w.env();
    env.retain(|(k, _)| *k != NO_REAL_CLAUDE.0);
    env.push(("PLATOON_CLAUDE_BIN", claude.to_string_lossy().into()));
    env.push(("PLATOON_SKIP_SETUP_CHECK", "1".into()));
    let mut t = Term::spawn(&["open"], &env, &w.work, 24, 100);
    t.wait_for(want);
    t.send(&[0]); // the back key: detach
    t.wait_exit();
    let (pid, _) = find_host(&w.cache).expect("the session runs in a host");
    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    until("the stand-in to end", || !alive(pid));
}

#[test]
fn a_claude_session_platoon_open_starts_carries_the_hook_that_asks_once() {
    let w = World::new("offer-open");
    // Claude Code is the one new sessions start in.
    fs::write(w.config.join("settings.json"), r#"{"new_session_agent": "claude"}"#).unwrap();
    open_and_end(&w, "fake-claude offer:1 settings:yes");
    let first = fs::read_to_string(w.base.join("first.json")).unwrap();
    let why = reason(first.trim());
    assert!(why.contains(&format!("task add --folder {} --squad", w.work.display())), "{why}");
    assert_eq!(fs::read_to_string(w.base.join("second.json")).unwrap(), "", "once");

    // The setting off: the session is started without the hook or the variable.
    let w = World::new("offer-open-off");
    fs::write(w.config.join("settings.json"), r#"{"new_session_agent": "claude", "offer_tasks": false}"#).unwrap();
    open_and_end(&w, "fake-claude offer:none settings:");
    assert!(!w.base.join("first.json").exists());
}
