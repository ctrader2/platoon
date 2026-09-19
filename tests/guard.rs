//! Black-box tests of `platoon hook guard` with real processes. Each "claude" here is a shell whose
//! child runs the hook, exactly like Claude Code runs it, so the parent-process lookup is exercised.

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
}

impl Env {
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!("platoon-guard-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (root, cache) = (base.join("claude"), base.join("cache"));
        fs::create_dir_all(root.join("sessions")).unwrap();
        fs::create_dir_all(&cache).unwrap();
        fs::write(base.join("prompt.json"), r#"{"session_id":"sess-1","hook_event_name":"UserPromptSubmit"}"#).unwrap();
        Self { root, cache }
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
}

impl Fake {
    fn start(env: &Env, tag: &str, started_at: u64, register: bool) -> Self {
        let results = env.cache.join(format!("{tag}.status"));
        let errors = env.cache.join(format!("{tag}.err"));
        // Not `exec`: the hook must be a child of this shell, like a hook is of claude.
        let script = format!(r#"while read x; do "{BIN}" hook guard <"{}" 2>>"{}"; echo $? >>"{}"; done"#, env.input().display(), errors.display(), results.display());
        let child = Command::new("sh")
            .args(["-c", &script])
            .env("CLAUDE_CONFIG_DIR", &env.root)
            .env("PLATOON_CACHE_DIR", &env.cache)
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
        Self { child, results, errors }
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
