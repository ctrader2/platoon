//! Black-box tests of `platoon hook guard` with real processes. Each "claude" here is a shell whose
//! child runs the hook, exactly like Claude Code runs it, so the parent-process lookup is exercised.
//!
//! The same hook also hands a project's notes to the conversation when they have changed, which is checked here
//! too: what it prints on standard output is the context Claude Code adds to the prompt.

mod common;

use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const BIN: &str = env!("CARGO_BIN_EXE_platoon");

struct Env {
    root: PathBuf,
    cache: PathBuf,
    config: PathBuf,
}

impl Env {
    fn new(name: &str) -> Self {
        let base = common::removed_at_exit(std::env::temp_dir().join(format!("platoon-guard-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&base);
        let (root, cache, config) = (base.join("claude"), base.join("cache"), base.join("config"));
        fs::create_dir_all(root.join("sessions")).unwrap();
        fs::create_dir_all(&cache).unwrap();
        fs::create_dir_all(&config).unwrap();
        fs::write(base.join("prompt.json"), r#"{"session_id":"sess-1","cwd":"/tmp","hook_event_name":"UserPromptSubmit"}"#).unwrap();
        Self { root, cache, config }
    }

    /// One project, "Web shop", with `session` in it and these notes.
    fn project(&self, session: &str, notes: &str) {
        let v = serde_json::json!({
            "version": 1,
            "projects": {"pweb": {"name": "Web shop", "color": "blue", "badge": "WB", "archived": false, "created": 1, "archived_sessions": []}},
            "sessions": {session: "pweb"},
        });
        fs::write(self.config.join("projects.json"), v.to_string()).unwrap();
        fs::create_dir_all(self.config.join("projects/pweb")).unwrap();
        self.notes(notes);
    }

    fn notes(&self, text: &str) {
        fs::write(self.config.join("projects/pweb/project.md"), text).unwrap();
    }
    fn input(&self) -> PathBuf {
        self.root.parent().unwrap().join("prompt.json")
    }
}

/// A stand-in for a Claude Code process: runs the hook (as its child) each time it is poked.
struct Fake {
    child: Child,
    results: PathBuf,
    errors: PathBuf,
    output: PathBuf,
}

impl Fake {
    fn start(env: &Env, tag: &str, started_at: u64, register: bool) -> Self {
        let results = env.cache.join(format!("{tag}.status"));
        let errors = env.cache.join(format!("{tag}.err"));
        let output = env.cache.join(format!("{tag}.out"));
        // Not `exec`: the hook must be a child of this shell, like a hook is of claude.
        let script = format!(
            r#"while read x; do "{BIN}" hook guard <"{}" >>"{}" 2>>"{}"; echo $? >>"{}"; done"#,
            env.input().display(),
            output.display(),
            errors.display(),
            results.display()
        );
        let child = Command::new("sh")
            .args(["-c", &script])
            .env("CLAUDE_CONFIG_DIR", &env.root)
            .env("PLATOON_CACHE_DIR", &env.cache)
            .env("PLATOON_CONFIG_DIR", &env.config)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        if register {
            fs::write(
                env.root.join("sessions").join(format!("{}.json", child.id())),
                format!(r#"{{"pid":{},"sessionId":"sess-1","cwd":"/tmp","kind":"interactive","startedAt":{started_at},"status":"idle"}}"#, child.id()),
            )
            .unwrap();
        }
        Self { child, results, errors, output }
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Submit a "prompt": returns the hook's exit code and what it printed to stderr.
    fn prompt(&mut self) -> (i32, String) {
        let count = |p: &Path| fs::read_to_string(p).map(|s| s.lines().count()).unwrap_or(0);
        let before = count(&self.results);
        writeln!(self.child.stdin.as_mut().unwrap(), "go").unwrap();
        let start = Instant::now();
        while count(&self.results) == before {
            assert!(start.elapsed() < Duration::from_secs(10), "hook never finished");
            thread::sleep(Duration::from_millis(20));
        }
        let code = fs::read_to_string(&self.results).unwrap().lines().last().unwrap().trim().parse().unwrap();
        (code, fs::read_to_string(&self.errors).unwrap_or_default())
    }

    /// Everything the hook has printed to standard output so far, and nothing else.
    fn printed(&self) -> String {
        fs::read_to_string(&self.output).unwrap_or_default()
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn only_the_earliest_copy_may_submit_prompts() {
    let env = Env::new("owner");
    let mut first = Fake::start(&env, "a", 1, false);
    let mut second = Fake::start(&env, "b", 2, false);
    for (f, t) in [(&first, 1u64), (&second, 2)] {
        fs::write(
            env.root.join("sessions").join(format!("{}.json", f.pid())),
            format!(r#"{{"pid":{},"sessionId":"sess-1","cwd":"/tmp","kind":"interactive","startedAt":{t}}}"#, f.pid()),
        )
        .unwrap();
    }

    assert_eq!(first.prompt().0, 0, "the owner must be allowed");

    let (code, err) = second.prompt();
    assert_eq!(code, 2, "the second copy must be refused");
    assert!(err.contains("already open in another terminal") && err.contains(&first.pid().to_string()), "{err}");
    assert!(err.contains("--fork-session") && err.contains("sess-1"), "the message should offer a safe way forward: {err}");
    assert!(env.cache.join("stale").join(second.pid().to_string()).exists(), "refused copies are marked stale");
}

#[test]
fn a_refused_copy_stays_refused_after_the_owner_exits() {
    let env = Env::new("stale");
    let mut owner = Fake::start(&env, "a", 1, false);
    let mut intruder = Fake::start(&env, "b", 2, false);
    for (f, t) in [(&owner, 1u64), (&intruder, 2)] {
        fs::write(
            env.root.join("sessions").join(format!("{}.json", f.pid())),
            format!(r#"{{"pid":{},"sessionId":"sess-1","cwd":"/tmp","kind":"interactive","startedAt":{t}}}"#, f.pid()),
        )
        .unwrap();
    }
    assert_eq!(intruder.prompt().0, 2);

    // The owner goes away. The intruder is now alone, but its memory is out of sync with the saved history.
    let _ = owner.child.kill();
    let _ = owner.child.wait();
    let (code, err) = intruder.prompt();
    assert_eq!(code, 2, "a stale copy must never be allowed to write again");
    assert!(err.contains("out of sync"), "{err}");
}

#[test]
fn a_lone_copy_is_never_blocked() {
    let env = Env::new("alone");
    let mut only = Fake::start(&env, "a", 1, true);
    assert_eq!(only.prompt().0, 0);
    assert_eq!(only.prompt().0, 0);
}

#[test]
fn uncertainty_fails_open() {
    // A session the registry doesn't know about (or a hook that can't tell who called it) is allowed.
    let env = Env::new("open");
    let mut unknown = Fake::start(&env, "a", 1, false);
    assert_eq!(unknown.prompt().0, 0);

    // Garbage on stdin doesn't block a prompt either.
    fs::write(env.input(), "not json").unwrap();
    let mut garbage = Fake::start(&env, "b", 1, true);
    assert_eq!(garbage.prompt().0, 0);
}

/// Write a registry entry of the given kind for a process.
fn register_as(env: &Env, pid: u32, kind: &str, started_at: u64) {
    fs::write(
        env.root.join("sessions").join(format!("{pid}.json")),
        format!(r#"{{"pid":{pid},"sessionId":"sess-1","cwd":"/tmp","kind":"{kind}","startedAt":{started_at},"status":"idle","jobId":"sess-1"}}"#),
    )
    .unwrap();
}

#[test]
fn claudes_own_background_session_is_never_blocked_and_never_blocks_anything() {
    // The guard only knows interactive processes. A background session (run by Claude's own service) writing to
    // its own conversation is allowed even when another copy of that conversation is registered, and it does not
    // count as an owner that would make a terminal copy of the same id a refused "second copy".
    let env = Env::new("bg");
    let mut bg = Fake::start(&env, "bg", 1, false);
    register_as(&env, bg.pid(), "bg", 1);
    assert_eq!(bg.prompt().0, 0, "a lone background session must be allowed");

    let mut terminal = Fake::start(&env, "t", 2, false);
    register_as(&env, terminal.pid(), "interactive", 2);
    assert_eq!(terminal.prompt().0, 0, "a later interactive copy is not refused because of a background entry");
    assert_eq!(bg.prompt().0, 0, "and the background session is not refused because of an interactive copy");
    assert!(!env.cache.join("stale").join(bg.pid().to_string()).exists(), "a background session is never marked stale");
    assert!(!env.cache.join("stale").join(terminal.pid().to_string()).exists());
}

#[test]
fn the_guard_hands_the_projects_notes_to_the_conversation_when_they_change_and_only_then() {
    let env = Env::new("notes");
    env.project("sess-1", "# Overview\nSell socks.\n");
    let mut only = Fake::start(&env, "a", 1, true);

    // The first prompt carries the notes, as context for the conversation, and is allowed as always.
    assert_eq!(only.prompt().0, 0);
    let printed = only.printed();
    let v: Value = serde_json::from_str(printed.trim()).unwrap_or_else(|e| panic!("not the hook's JSON: {printed:?} ({e})"));
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
    let context = v["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
    assert!(context.starts_with("These are the notes of the project \"Web shop\""), "{context}");
    assert!(context.ends_with("# Overview\nSell socks."), "{context}");

    // The next prompts say nothing at all: the session already has this version.
    assert_eq!(only.prompt().0, 0);
    assert_eq!(only.prompt().0, 0);
    assert_eq!(only.printed().lines().count(), 1, "still just the one hand-over: {:?}", only.printed());

    // The notes change while the session runs: the very next prompt carries them.
    env.notes("# Overview\nSell socks and hats.\n");
    assert_eq!(only.prompt().0, 0);
    let printed = only.printed();
    let lines: Vec<&str> = printed.lines().collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    let v: Value = serde_json::from_str(lines[1]).unwrap();
    assert!(v["hookSpecificOutput"]["additionalContext"].as_str().unwrap().contains("hats"));
    assert_eq!(only.prompt().0, 0);
    assert_eq!(only.printed().lines().count(), 2, "and not again");
}

#[test]
fn a_prompt_the_guard_refuses_carries_no_notes_and_a_session_in_no_project_never_does() {
    // A session in no project: allowed, and nothing printed.
    let env = Env::new("notes-none");
    let mut loose = Fake::start(&env, "a", 1, true);
    assert_eq!(loose.prompt().0, 0);
    assert_eq!(loose.printed(), "", "nothing to say for a session in no project");

    // A second copy of a session that is in a project: refused, and nothing printed either. The guard's own
    // answer is all that matters when it refuses.
    let env = Env::new("notes-blocked");
    env.project("sess-1", "# Overview\nSell socks.\n");
    let owner = Fake::start(&env, "a", 1, false);
    let mut second = Fake::start(&env, "b", 2, false);
    for (f, t) in [(&owner, 1u64), (&second, 2)] {
        fs::write(
            env.root.join("sessions").join(format!("{}.json", f.pid())),
            format!(r#"{{"pid":{},"sessionId":"sess-1","cwd":"/tmp","kind":"interactive","startedAt":{t}}}"#, f.pid()),
        )
        .unwrap();
    }
    let (code, err) = second.prompt();
    assert_eq!(code, 2, "{err}");
    assert_eq!(second.printed(), "", "a refused prompt is given nothing");
    assert!(!env.cache.join("notes-sent").join("sess-1").exists(), "and nothing is marked as sent");
}
