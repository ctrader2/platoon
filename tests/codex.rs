//! Codex's conversations end to end, in a real terminal with the real binary: listed beside Claude Code's with their
//! model's name, found by the search, opened (resumed) in a platoon host, and never opened twice when another Codex
//! window holds one. A small script stands in for `codex` (the real one would spend the user's allowance and touch
//! their `~/.codex`), and every folder platoon touches is a temp dir.

mod common;
use common::*;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

const FINISHED: &str = "01a1517c-414d-7902-93d1-efa74830493d";
const PLANNING: &str = "01a15181-dc6f-7b73-81cf-c041b14f6432";
const SAMPLES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/testdata/codex");

struct World {
    base: PathBuf,
    codex: PathBuf,
    cache: PathBuf,
    work: PathBuf,
}

/// A stand-in `codex`: writes the arguments it was started with to `FAKE_CODEX_ARGS`, says so, and waits.
fn fake_codex(dir: &Path) -> PathBuf {
    let p = dir.join("fake-codex.sh");
    fs::write(&p, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$FAKE_CODEX_ARGS\"\necho \"fake-codex args:$* cwd:$(pwd)\"\nexec sleep 60\n")
        .unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    p
}

impl World {
    /// The sample Codex sessions (run in this world's `work` folder) and their names.
    fn new(name: &str) -> Self {
        let base = tmp(name);
        let w = Self { codex: base.join("codex"), cache: base.join("cache"), work: base.join("work"), base };
        for d in ["claude/sessions", "cache", "config", "work"] {
            fs::create_dir_all(w.base.join(d)).unwrap();
        }
        // The samples carry fixed dates that will be old one day: this file is not about auto-archive.
        fs::write(w.base.join("config/settings.json"), r#"{"auto_archive_days":0}"#).unwrap();
        let day = "sessions/2026/10/18";
        fs::create_dir_all(w.codex.join(day)).unwrap();
        for f in fs::read_dir(Path::new(SAMPLES).join(day)).unwrap().flatten() {
            let text = fs::read_to_string(f.path()).unwrap().replace("\"/work/repo\"", &format!("\"{}\"", w.work.display()));
            fs::write(w.codex.join(day).join(f.file_name()), text).unwrap();
        }
        fs::copy(Path::new(SAMPLES).join("session_index.jsonl"), w.codex.join("session_index.jsonl")).unwrap();
        w
    }

    fn env(&self) -> Vec<(&'static str, String)> {
        let dir = |d: &str| self.base.join(d).to_string_lossy().into_owned();
        vec![
            ("CLAUDE_CONFIG_DIR", dir("claude")),
            ("PLATOON_CACHE_DIR", dir("cache")),
            ("PLATOON_CONFIG_DIR", dir("config")),
            ("PLATOON_SKIP_SETUP_CHECK", "1".into()),
            ("CODEX_HOME", self.codex.to_string_lossy().into()),
            ("PLATOON_CODEX_BIN", fake_codex(&self.base).to_string_lossy().into()),
            ("FAKE_CODEX_ARGS", dir("codex-args")),
        ]
    }
}

#[test]
fn a_codex_conversation_is_listed_with_its_model_and_found_by_the_words_typed_in_it() {
    let w = World::new("codex-list");
    let mut t = Term::spawn(&[], &w.env(), &w.work, 30, 140);
    t.wait_for_all(&["Identify colour from notes.txt", "Ask about notes.txt change", "GPT-6-Luna", "Active  3"]);
    // Full-text: what the user typed in a Codex conversation is found like a Claude one.
    t.send(b"/\tcolour notes");
    t.wait_for("Read notes.txt and tell me the colour");
    t.send(b"\x1b");
    t.wait_gone("Read notes.txt and tell me the colour");
}

#[test]
fn enter_on_a_codex_conversation_resumes_it_in_a_platoon_host_with_exactly_these_arguments() {
    let w = World::new("codex-resume");
    let mut t = Term::spawn(&[], &w.env(), &w.work, 30, 140);
    t.wait_for_all(&["Identify colour from notes.txt", "Active  3"]);
    // The oldest of the three is the last row.
    t.send(b"\x1b[B\x1b[B\r");
    t.wait_for(&format!("fake-codex args:resume {FINISHED} --no-daemon cwd:{}", w.work.display()));
    assert_eq!(fs::read_to_string(w.base.join("codex-args")).unwrap(), format!("resume\n{FINISHED}\n--no-daemon\n"));
    let (pid, sock) = find_host(&w.cache).expect("a host runs it");
    let info: serde_json::Value = serde_json::from_str(&fs::read_to_string(sock.with_extension("json")).unwrap()).unwrap();
    assert_eq!(info["agent"], "codex", "a host found again after platoon restarts knows its agent");
    assert_eq!(info["args"], serde_json::json!(["resume", FINISHED, "--no-daemon"]));
    t.send(&[0]); // ctrl+space: back to the list
    t.wait_for("enter open");
    t.send(b"q");
    t.wait_exit();
    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    until("the stand-in to exit", || !alive(pid));
}

/// Another process holds a conversation's writer lock, as another Codex window does: platoon shows it running and
/// only says where it is. Linux (`flock`, `/proc/locks`).
#[cfg(target_os = "linux")]
#[test]
fn a_codex_conversation_open_elsewhere_is_never_opened_a_second_time() {
    use std::process::{Command, Stdio};
    let w = World::new("codex-held");
    let locks = w.codex.join("thread-writer-locks");
    fs::create_dir_all(&locks).unwrap();
    let lock = locks.join(format!("{PLANNING}.lock"));
    fs::write(&lock, "").unwrap();
    let mut holder = Command::new("flock").arg(&lock).args(["sleep", "60"]).stdin(Stdio::null()).spawn().unwrap();
    let mut t = Term::spawn(&[], &w.env(), &w.work, 30, 140);
    // The newest row, held: the filled mark of a running conversation.
    t.wait_for_all(&["Ask about notes.txt change", "●", "idle"]);
    t.send(b"\r");
    t.wait_for_flat("is open in another Codex window. Close it there to open it here.");
    assert!(!w.base.join("codex-args").exists(), "nothing was started");
    assert!(find_host(&w.cache).is_none());
    assert!(running(holder.id()), "never signalled");
    let _ = holder.kill();
    let _ = holder.wait();
}

/// `platoon <args>` with `input` on stdin, in this world, with no terminal: (exit code, stdout, stderr).
fn run(w: &World, args: &[&str], input: &str) -> (i32, String, String) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(BIN)
        .args(args)
        .envs(w.env())
        .env("PLATOON_NO_UPDATE_CHECK", "1")
        .env("PLATOON_NOTES_HELPER", "off")
        .env("NO_COLOR", "1")
        .env_remove("PLATOON_SKIP_SETUP_CHECK")
        .env_remove("PLATOON_PROJECT")
        .env_remove("PLATOON_OFFER_TASKS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn setup_adds_platoons_three_codex_hooks_says_codex_will_ask_to_trust_them_and_never_blocks_on_them() {
    let w = World::new("codex-setup");
    let (code, out, err) = run(&w, &["setup"], "");
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("\nCodex\n") && out.contains("Codex asks you once to trust these hooks"), "{out}");
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(w.codex.join("hooks.json")).unwrap()).unwrap();
    for (event, args) in [
        ("SessionStart", "hook session-start --agent codex"),
        ("UserPromptSubmit", "hook guard --agent codex"),
        ("Stop", "hook stop --agent codex"),
    ] {
        assert_eq!(v["hooks"][event][0]["hooks"][0]["command"], format!("{BIN} {args}"), "{event}");
    }
    let (_, again, _) = run(&w, &["setup"], "");
    assert!(again.matches("already done").count() >= 7, "{again}");
    assert!(!again.contains("trust these hooks"), "nothing new to trust: {again}");

    // An unreadable hooks file is said and left alone; Claude Code's steps are done and platoon starts.
    fs::write(w.codex.join("hooks.json"), "{ broken").unwrap();
    let (code, out, _) = run(&w, &["setup"], "");
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Codex hooks") && out.contains("left the file alone"), "{out}");
    assert_eq!(fs::read_to_string(w.codex.join("hooks.json")).unwrap(), "{ broken");
}

#[test]
fn codexs_hooks_hand_a_projects_notes_to_its_sessions_and_mark_the_notes_due_after_a_turn() {
    let w = World::new("codex-hooks");
    let members = serde_json::json!({FINISHED: "pweb"});
    let projects = serde_json::json!({"version": 1, "projects": {"pweb": {"name": "Web", "color": "blue", "badge": "WB", "archived": false, "created": 1, "archived_sessions": []}}, "sessions": members});
    fs::write(w.base.join("config/projects.json"), projects.to_string()).unwrap();
    fs::create_dir_all(w.base.join("config/projects/pweb")).unwrap();
    fs::write(w.base.join("config/projects/pweb/project.md"), "# Web\n\nThe shop's notes.\n").unwrap();
    // Notes that keep themselves up to date are on (no call is made: the helper is off).
    let settings = serde_json::json!({"auto_archive_days": 0, "tokens_enabled": true, "token_features": {"project_notes": true}});
    fs::write(w.base.join("config/settings.json"), settings.to_string()).unwrap();
    let sample =
        |name: &str| fs::read_to_string(Path::new(SAMPLES).join("hooks").join(format!("{name}.json"))).unwrap().replace(PLANNING, FINISHED);

    let (code, out, err) = run(&w, &["hook", "session-start", "--agent", "codex"], &sample("SessionStart"));
    assert_eq!((code, err.as_str()), (0, ""));
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap_or_else(|e| panic!("{e}: {out}"));
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
    let context = v["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
    assert!(context.contains("\"Web\"") && context.contains("The shop's notes."), "{context}");
    assert!(!context.contains("Ready to create tasks"), "the offer is Claude Code's");

    // A prompt: nothing until the notes change, then the new version; never refused (Codex keeps one window itself).
    let (code, out, _) = run(&w, &["hook", "guard", "--agent", "codex"], &sample("UserPromptSubmit"));
    assert_eq!((code, out.as_str()), (0, ""));
    fs::write(w.base.join("config/projects/pweb/project.md"), "# Web\n\nNew notes.\n").unwrap();
    let (code, out, _) = run(&w, &["hook", "guard", "--agent", "codex"], &sample("UserPromptSubmit"));
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap_or_else(|e| panic!("{e}: {out}"));
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
    assert!(v["hookSpecificOutput"]["additionalContext"].as_str().unwrap().contains("New notes."));

    // The end of a turn marks the project's notes due (the helper is off here), silently.
    let (code, out, err) = run(&w, &["hook", "stop", "--agent", "codex"], &sample("Stop-first"));
    assert_eq!((code, out.as_str(), err.as_str()), (0, "", ""));
    assert!(w.cache.join("notes-due/pweb").exists());
}
