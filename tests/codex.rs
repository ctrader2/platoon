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
            // Claude Code installed beside Codex, whatever this machine's path has (setup does both parts).
            (NO_REAL_CLAUDE.0, NO_REAL_CLAUDE.1.into()),
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
    run_with(w, &[], args, input)
}

/// The same, with `extra` added to (or replacing) the world's environment.
fn run_with(w: &World, extra: &[(&str, String)], args: &[&str], input: &str) -> (i32, String, String) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(BIN)
        .args(args)
        .envs(w.env())
        .envs(extra.iter().cloned())
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

/// A stand-in for Codex's app-server (`fake-codex-server.sh`) in `<base>/<name>`, answering `initialize` for this
/// world's Codex folder; it records what it is sent in `requests`.
fn fake_server(w: &World, name: &str) -> PathBuf {
    let dir = w.base.join(name);
    fs::create_dir_all(&dir).unwrap();
    let p = dir.join("codex");
    fs::write(&p, include_str!("fake-codex-server.sh").replace("@DIR@", &dir.to_string_lossy())).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    answer(&dir, "initialize", "result", serde_json::json!({"codexHome": w.codex}));
    dir
}

/// Have the stand-in in `dir` answer the requests the file `file` stands for (`hooks-list.1`) with `{kind: value}`.
fn answer(dir: &Path, file: &str, kind: &str, value: serde_json::Value) {
    let line = serde_json::json!({"id": 0, (kind): value}).to_string().replacen("{\"id\":0,", "{\"id\":@ID@,", 1);
    fs::write(dir.join(file), line + "\n").unwrap();
}

fn requests(dir: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(dir.join("requests")).unwrap_or_default().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

const OURS: [(&str, &str, &str, &str); 3] = [
    ("SessionStart", "sessionStart", "session_start", "hook session-start --agent codex"),
    ("UserPromptSubmit", "userPromptSubmit", "user_prompt_submit", "hook guard --agent codex"),
    ("Stop", "stop", "stop", "hook stop --agent codex"),
];

/// What Codex's `hooks/list` says of platoon's three hooks in this world, each `trust`.
fn listed(w: &World, trust: &str) -> serde_json::Value {
    let file = w.codex.join("hooks.json");
    let hooks: Vec<serde_json::Value> = OURS
        .iter()
        .map(|(_, event, snake, args)| {
            serde_json::json!({"key": format!("{}:{snake}:0:0", file.display()), "eventName": event, "handlerType": "command",
                "command": format!("{BIN} {args}"), "sourcePath": file, "source": "user", "isManaged": false,
                "currentHash": format!("sha256:{snake}"), "trustStatus": trust})
        })
        .collect();
    serde_json::json!({"data": [{"cwd": "/", "hooks": hooks, "warnings": [], "errors": []}]})
}

#[test]
fn setup_adds_platoons_three_codex_hooks_has_codex_trust_them_and_never_blocks_on_them() {
    let w = World::new("codex-setup");
    let server = fake_server(&w, "server");
    answer(&server, "hooks-list.1", "result", listed(&w, "untrusted"));
    answer(&server, "hooks-list", "result", listed(&w, "trusted"));
    answer(&server, "config-batchWrite", "result", serde_json::json!({"status": "ok", "version": "v2"}));
    let codex_bin = [("PLATOON_CODEX_BIN", server.join("codex").to_string_lossy().into_owned())];
    let (code, out, err) = run_with(&w, &codex_bin, &["setup"], "");
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("\nCodex\n") && out.matches("added; now trusted in Codex").count() == 3, "{out}");
    assert!(out.contains("Setup had Codex mark them trusted, so they run in your Codex sessions without asking."), "{out}");
    assert!(!out.contains("Codex asks you once"), "{out}");
    assert!(out.contains(&format!("Saved {}.\n", w.codex.join("hooks.json").display())), "no file before, no backup named: {out}");
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(w.codex.join("hooks.json")).unwrap()).unwrap();
    for (event, _, _, args) in OURS {
        assert_eq!(v["hooks"][event][0]["hooks"][0]["command"], format!("{BIN} {args}"), "{event}");
    }
    let sent = requests(&server);
    let methods: Vec<&str> = sent.iter().map(|r| r["method"].as_str().unwrap()).collect();
    assert_eq!(methods, ["initialize", "initialized", "hooks/list", "config/batchWrite", "hooks/list"]);
    assert_eq!(sent[2]["params"], serde_json::json!({"cwds": [std::env::var("HOME").unwrap()]}));
    let records = &sent[3]["params"]["edits"][0]["value"];
    let file = w.codex.join("hooks.json");
    let want: serde_json::Map<String, serde_json::Value> = OURS
        .iter()
        .map(|(_, _, snake, _)| (format!("{}:{snake}:0:0", file.display()), serde_json::json!({"trusted_hash": format!("sha256:{snake}")})))
        .collect();
    assert_eq!(records, &serde_json::Value::Object(want), "exactly platoon's three");
    assert!(!w.codex.join("config.toml").exists(), "platoon itself never writes Codex's config.toml");

    // Again: Codex already trusts them, so nothing is written anywhere.
    let (_, again, _) = run_with(&w, &codex_bin, &["setup"], "");
    assert!(again.matches("already done").count() >= 7, "{again}");
    assert_eq!(again.matches("already done; Codex trusts it").count(), 3, "{again}");
    assert!(again.contains("Nothing to change.") && !again.contains("trust these hooks"), "nothing new to trust: {again}");
    assert_eq!(requests(&server).len(), 8, "only asked: initialize, initialized, hooks/list");

    // An unreadable hooks file is said and left alone, and Codex isn't asked; Claude Code's steps are done and platoon starts.
    fs::write(w.codex.join("hooks.json"), "{ broken").unwrap();
    let (code, out, _) = run_with(&w, &codex_bin, &["setup"], "");
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Codex hooks") && out.contains("left the file alone"), "{out}");
    assert_eq!(fs::read_to_string(w.codex.join("hooks.json")).unwrap(), "{ broken");
    assert_eq!(requests(&server).len(), 8);
}

#[test]
fn when_codex_cant_trust_platoons_hooks_setup_says_codex_will_ask_once_and_still_succeeds() {
    let w = World::new("codex-setup-refused");
    let server = fake_server(&w, "server");
    answer(&server, "hooks-list", "result", listed(&w, "untrusted"));
    answer(&server, "config-batchWrite", "error", serde_json::json!({"code": -32600, "message": "config.toml is read-only"}));
    let codex_bin = [("PLATOON_CODEX_BIN", server.join("codex").to_string_lossy().into_owned())];
    let (code, out, err) = run_with(&w, &codex_bin, &["setup"], "");
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("[✓] Status line") && out.contains("[✓] Session guard"), "Claude Code's steps as ever: {out}");
    assert!(out.contains("platoon couldn't ask Codex to trust them: Codex refused: config.toml is read-only."), "{out}");
    assert!(out.contains("Codex asks you once to trust these hooks") && out.contains("Approve them there"), "{out}");
    assert!(!out.contains("now trusted"), "{out}");
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
