//! `platoon tokens` and the setup question about token features, end to end with the real binary. Every directory
//! is a temp dir and `claude` is always a fake script (the environment also forbids the real one), so nothing here
//! can spend a token or touch the user's files.

mod common;
use common::*;
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

const GOOD: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","total_cost_usd":0.000612,"usage":{"input_tokens":400,"cache_creation_input_tokens":0,"cache_read_input_tokens":17,"output_tokens":5},"session_id":"x"}"#;

struct Lab {
    base: PathBuf,
}

impl Lab {
    fn new(name: &str) -> Lab {
        let base = tmp(name);
        for d in ["claude", "cache", "config"] {
            fs::create_dir_all(base.join(d)).unwrap();
        }
        Lab { base }
    }

    fn cache(&self) -> PathBuf {
        self.base.join("cache")
    }

    fn config(&self) -> PathBuf {
        self.base.join("config")
    }

    fn settings(&self, v: Value) {
        fs::write(self.config().join("settings.json"), v.to_string()).unwrap();
    }

    fn on(&self) {
        self.settings(json!({"tokens_enabled": true}));
    }

    /// A stand-in for `claude` that records what it was given and then runs `body`.
    fn fake(&self, body: &str) -> String {
        let script = self.base.join("fake-claude");
        let d = self.base.display();
        fs::write(
            &script,
            format!("#!/bin/sh\ndir='{d}'\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$dir/rec.argv\"\npwd > \"$dir/rec.cwd\"\nenv > \"$dir/rec.env\"\ncat > \"$dir/rec.stdin\"\n{body}\n"),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        script.display().to_string()
    }

    fn good(&self) -> String {
        self.fake(&format!("printf '%s' '{GOOD}'"))
    }

    fn rec(&self, what: &str) -> String {
        fs::read_to_string(self.base.join(format!("rec.{what}"))).unwrap_or_default()
    }

    fn ran(&self) -> bool {
        self.base.join("rec.argv").exists()
    }

    fn env(&self, bin: &str) -> Vec<(&'static str, String)> {
        vec![
            ("CLAUDE_CONFIG_DIR", self.base.join("claude").display().to_string()),
            ("PLATOON_CACHE_DIR", self.cache().display().to_string()),
            ("PLATOON_CONFIG_DIR", self.config().display().to_string()),
            ("PLATOON_CLAUDE_BIN", bin.to_string()),
            ("PLATOON_SKIP_SETUP_CHECK", "1".into()),
        ]
    }

    fn run(&self, args: &[&str], bin: &str) -> Output {
        let mut c = Command::new(BIN);
        c.env("PLATOON_NO_UPDATE_CHECK", "1"); // no version checks: never the network, never `claude --version`
        for (k, v) in common::NO_REAL_CODEX {
            c.env(k, v); // never the real Codex: its folder or its program
        }
        c.args(args).env("NO_COLOR", "1").env("PLATOON_FORBID_REAL_CLAUDE", "1").env("CLAUDE_CODE_CHILD_SESSION", "1").stdin(Stdio::null());
        for (k, v) in self.env(bin) {
            c.env(k, v);
        }
        c.output().unwrap()
    }

    fn ledger(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.cache().join("ai-usage.json")).unwrap()).unwrap()
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn listing(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> =
        fs::read_dir(dir).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    v.sort();
    v
}

#[test]
fn the_test_call_is_refused_while_the_features_are_off_and_says_how_to_turn_them_on() {
    let lab = Lab::new("tok-off");
    let bin = lab.good();
    let out = lab.run(&["tokens", "test"], &bin);
    assert_eq!(out.status.code(), Some(3), "{}", text(&out.stderr));
    let err = text(&out.stderr);
    assert!(err.contains("Not run:") && err.contains("Enable features which require tokens") && err.contains("switched off"), "{err}");
    assert!(err.contains("press s in platoon") && err.contains("tokens_enabled") && err.contains("settings.json"), "{err}");
    assert!(!lab.ran(), "the fake claude was never started");
    assert!(
        listing(&lab.cache()).is_empty() && listing(&lab.config()).is_empty(),
        "nothing was written: {:?} {:?}",
        listing(&lab.cache()),
        listing(&lab.config())
    );
}

#[test]
fn the_test_call_runs_the_fake_the_agreed_way_and_prints_the_reply_usage_cost_and_todays_total() {
    let lab = Lab::new("tok-ok");
    lab.on();
    let bin = lab.good();
    let settings_before = fs::read(lab.config().join("settings.json")).unwrap();
    let out = lab.run(&["tokens", "test"], &bin);
    let (o, e) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "{o}\n{e}");
    assert!(o.contains("Reply: ok"), "{o}");
    assert!(o.contains("Tokens used: 422") && o.contains("input 400") && o.contains("cache read 17") && o.contains("output 5"), "{o}");
    assert!(o.contains("$0.0006"), "{o}");
    assert!(o.contains("Used today: 422 of 500,000 tokens"), "{o}");

    let argv: Vec<String> = lab.rec("argv").lines().map(String::from).collect();
    assert_eq!(
        argv[..12],
        [
            "-p",
            "--model",
            "haiku",
            "--tools",
            "",
            "--setting-sources",
            "",
            "--no-session-persistence",
            "--disable-slash-commands",
            "--strict-mcp-config",
            "--system-prompt",
            argv[11].as_str()
        ]
    );
    assert_eq!(argv[12..], ["--effort", "low", "--output-format", "json"]);
    assert!(!argv.iter().any(|a| a == "--bare"));
    assert_eq!(lab.rec("stdin"), "Reply with the single word ok");
    assert_eq!(fs::canonicalize(lab.rec("cwd").trim()).unwrap(), fs::canonicalize(lab.cache().join("ai-work")).unwrap());
    assert!(lab.rec("env").lines().any(|l| l == "PLATOON_AI=1"));
    assert!(lab.rec("env").lines().any(|l| l == "MAX_THINKING_TOKENS=0"), "no hidden thinking tokens");
    assert!(!lab.rec("env").contains("CLAUDE_CODE_CHILD_SESSION"), "platoon ran inside Claude Code must not pass that on");

    // Only the cache dir got anything, and the settings file is exactly as it was.
    assert_eq!(fs::read(lab.config().join("settings.json")).unwrap(), settings_before);
    assert_eq!(listing(&lab.config()), ["settings.json"]);
    let names = listing(&lab.cache());
    assert!(names.iter().all(|n| ["ai-usage.json", "ai-usage.lock", "ai-work"].contains(&n.as_str())), "{names:?}");
    let ledger = lab.ledger();
    let day = ledger["days"].as_object().unwrap().values().next().unwrap();
    assert_eq!(
        (day["calls"].as_u64(), day["usage"]["input"].as_u64(), day["features"]["test"]["tokens"].as_u64()),
        (Some(1), Some(400), Some(422))
    );
    assert!(!fs::read_to_string(lab.cache().join("ai-usage.json")).unwrap().contains("single word"), "no prompt text on disk");

    // A second call adds to today's total.
    let o = text(&lab.run(&["tokens", "test"], &bin).stdout);
    assert!(o.contains("Used today: 844 of 500,000 tokens"), "{o}");
    // And `platoon tokens` reports it.
    let s = text(&lab.run(&["tokens"], &bin).stdout);
    assert!(
        s.contains("Features that use tokens: on")
            && s.contains("Used today: 844 of 500,000 tokens")
            && s.contains("Model for smart features: Haiku (Claude Code)"),
        "{s}"
    );
    assert!(s.contains("Model for project notes: Sonnet (Claude Code)"), "the notes' own model: {s}");
    // Where the tokens went, feature by feature, so the spend of each one is visible.
    assert!(s.contains("  Token test: 844 tokens in 2 calls"), "work with no switch of its own still says what it spent: {s}");
    assert!(s.contains("Project notes: on") && s.contains("Project descriptions: on"), "every feature has its own line: {s}");
}

#[test]
fn the_daily_limit_and_the_plan_pause_and_a_failure_all_refuse_the_next_call_and_say_why() {
    // The daily limit.
    let lab = Lab::new("tok-cap");
    lab.settings(json!({"tokens_enabled": true, "ai_daily_tokens": 20000}));
    let bin = lab.good();
    assert_eq!(lab.run(&["tokens", "test"], &bin).status.code(), Some(0));
    // 422 tokens per call: push today's count over the limit through the ledger the way a real day would.
    let mut ledger = lab.ledger();
    let day = ledger["days"].as_object_mut().unwrap().values_mut().next().unwrap();
    *day.get_mut("usage").unwrap().get_mut("input").unwrap() = json!(25_000);
    fs::write(lab.cache().join("ai-usage.json"), ledger.to_string()).unwrap();
    let _ = fs::remove_file(lab.base.join("rec.argv"));
    let out = lab.run(&["tokens", "test"], &bin);
    let e = text(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{e}");
    assert!(e.contains("Today's token limit is used up") && e.contains("25,022 of 20,000"), "{e}");
    assert!(!lab.ran());

    // The plan pause: a status line report of 95% used, window not yet reset.
    let lab = Lab::new("tok-plan");
    lab.on();
    let bin = lab.good();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    fs::create_dir_all(lab.cache().join("status")).unwrap();
    fs::write(
        lab.cache().join("status/s1.json"),
        json!({"ts": now, "context_pct": null, "five_hour": {"pct": 95, "resets_at": now + 3600}, "seven_day": null}).to_string(),
    )
    .unwrap();
    let out = lab.run(&["tokens", "test"], &bin);
    assert_eq!(out.status.code(), Some(3));
    assert!(text(&out.stderr).contains("95% used") && text(&out.stderr).contains("pause point of 80%"), "{}", text(&out.stderr));
    assert!(!lab.ran());
    // ...and with the pause point set to Never it goes through.
    lab.settings(json!({"tokens_enabled": true, "ai_pause_percent": 0}));
    assert_eq!(lab.run(&["tokens", "test"], &bin).status.code(), Some(0));

    // A failure: signed out. The call fails plainly, and the next one is refused without starting anything.
    let lab = Lab::new("tok-fail");
    lab.on();
    let bin = lab.fake("printf '%s' '{\"is_error\":true,\"result\":\"Not logged in · Please run /login\"}'\nexit 1");
    let out = lab.run(&["tokens", "test"], &bin);
    assert_eq!(out.status.code(), Some(1));
    let err = text(&out.stderr);
    assert!(err.contains("The call failed: Claude isn't signed in."), "{err}");
    assert!(err.contains("wait 30 minutes"), "{err}");
    let _ = fs::remove_file(lab.base.join("rec.argv"));
    let again = lab.run(&["tokens", "test"], &bin);
    assert_eq!(again.status.code(), Some(3));
    assert!(
        text(&again.stderr).contains("paused after a failure") && text(&again.stderr).contains("isn't signed in"),
        "{}",
        text(&again.stderr)
    );
    assert!(!lab.ran());
    let s = text(&lab.run(&["tokens"], &bin).stdout);
    assert!(s.contains("Paused after a failure"), "{s}");
}

#[test]
fn a_test_that_forgets_to_name_a_fake_never_reaches_the_real_claude() {
    let lab = Lab::new("tok-forbid");
    lab.on();
    let mut c = Command::new(BIN);
    c.env("PLATOON_NO_UPDATE_CHECK", "1"); // no version checks: never the network, never `claude --version`
    for (k, v) in common::NO_REAL_CODEX {
        c.env(k, v); // never the real Codex: its folder or its program
    }
    // A `claude` on the path makes Claude Code installed; the one here only leaves a mark if it is ever run.
    let path = lab.base.join("path");
    fs::create_dir_all(&path).unwrap();
    let ran = lab.base.join("real-claude-ran");
    fs::write(path.join("claude"), format!("#!/bin/sh\n: > '{}'\n", ran.display())).unwrap();
    fs::set_permissions(path.join("claude"), fs::Permissions::from_mode(0o755)).unwrap();
    c.args(["tokens", "test"])
        .env("PLATOON_CACHE_DIR", lab.cache())
        .env("PLATOON_CONFIG_DIR", lab.config())
        .env("PLATOON_FORBID_REAL_CLAUDE", "1")
        .env_remove("PLATOON_CLAUDE_BIN")
        .env("PATH", &path)
        .stdin(Stdio::null());
    let out = c.output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("refusing to run the real claude"), "{}", text(&out.stderr));
    assert!(!ran.exists(), "the `claude` on the path was never run");
}

#[test]
fn in_a_terminal_the_test_call_reads_well_and_when_refused_says_how_to_enable() {
    let lab = Lab::new("tok-pty");
    let bin = lab.good();
    // Refused.
    let mut t = Term::spawn(&["tokens", "test"], &lab.env(&bin), &lab.base, 24, 100);
    assert_eq!(t.wait_exit(), 3);
    let screen = t.flat();
    assert!(screen.contains("Not run:") && screen.contains("press s in platoon"), "{screen}");
    // Allowed.
    lab.on();
    let mut t = Term::spawn(&["tokens", "test"], &lab.env(&bin), &lab.base, 24, 100);
    assert_eq!(t.wait_exit(), 0);
    let screen = t.text();
    assert!(
        screen.contains("Asking Claude Code (Haiku)")
            && screen.contains("Reply: ok")
            && screen.contains("Used today: 422 of 500,000 tokens"),
        "{screen}"
    );
}

#[test]
fn ctrl_c_while_a_call_is_running_stops_it_and_leaves_no_process_behind() {
    let lab = Lab::new("tok-int");
    lab.on();
    let bin = lab.fake("echo $$ > \"$dir/pid\"\nsleep 300 &\necho $! > \"$dir/sleep\"\nwait");
    let mut t = Term::spawn(&["tokens", "test"], &lab.env(&bin), &lab.base, 24, 100);
    t.wait_for("Asking Claude");
    let pid_file = lab.base.join("sleep");
    until("the fake to start", || fs::read_to_string(&pid_file).is_ok_and(|s| !s.trim().is_empty()));
    let pids: Vec<u32> = ["pid", "sleep"].iter().map(|f| fs::read_to_string(lab.base.join(f)).unwrap().trim().parse().unwrap()).collect();
    assert!(pids.iter().all(|p| alive(*p)));
    t.send(b"\x03");
    assert_eq!(t.wait_exit(), 1);
    assert!(t.text().contains("Cancelled."), "{}", t.text());
    until("the fake and its child to be gone", || pids.iter().all(|p| !alive(*p)));
    assert!(!lab.cache().join("ai-usage.json").exists() || lab.ledger().get("backoff").is_none(), "cancelling is not a failure");
}

// ---- the setup question ----

fn setup_env(lab: &Lab) -> Vec<(&'static str, String)> {
    let mut e = lab.env("/nonexistent/never-run");
    e.retain(|(k, _)| *k != "PLATOON_SKIP_SETUP_CHECK");
    e
}

fn saved(lab: &Lab) -> Value {
    serde_json::from_str(&fs::read_to_string(lab.config().join("settings.json")).unwrap()).unwrap()
}

#[test]
fn setup_asks_in_a_terminal_and_yes_saves_the_master_switch_the_features_and_the_marker() {
    let lab = Lab::new("tok-setup-yes");
    let mut t = Term::spawn(&["setup"], &setup_env(&lab), &lab.base, 50, 110);
    t.wait_for_flat("Turn these features on? [y/N]");
    let screen = t.flat();
    for want in [
        "Smart names and summaries",
        "Find a session by description",
        "your own Claude plan",
        "the prompts you typed",
        "one line of Claude's last reply for each session",
        "press s in platoon",
    ] {
        assert!(screen.contains(want), "{want}:\n{screen}");
    }
    // The word the rest of platoon uses: a session, never a "conversation".
    assert!(!screen.to_lowercase().contains("conversation"), "{screen}");
    assert!(screen.contains("[✓]") || screen.contains("already done") || screen.contains("added"), "the checklist came first:\n{screen}");
    t.send(b"y\r");
    assert_eq!(t.exit_without_editor_question(), 0);
    assert!(t.flat().contains("Smart features are on"), "{}", t.flat());
    let s = saved(&lab);
    assert_eq!((s["tokens_enabled"].clone(), s["tokens_asked"].clone()), (json!(true), json!(true)));
    assert_eq!(s["token_features"], json!({"names": true, "find": true, "project_notes": true, "project_description": true}));
    // Asked once: the second run prints no question and changes nothing.
    let before = fs::read(lab.config().join("settings.json")).unwrap();
    let mut t = Term::spawn(&["setup"], &setup_env(&lab), &lab.base, 50, 110);
    assert_eq!(t.wait_exit(), 0);
    assert!(!t.flat().contains("Turn these features on?"), "{}", t.flat());
    assert_eq!(fs::read(lab.config().join("settings.json")).unwrap(), before);
}

#[test]
fn setup_defaults_to_no_and_remembers_that_it_asked() {
    let lab = Lab::new("tok-setup-no");
    let mut t = Term::spawn(&["setup"], &setup_env(&lab), &lab.base, 50, 110);
    t.wait_for_flat("Turn these features on? [y/N]");
    t.send(b"\r");
    assert_eq!(t.exit_without_editor_question(), 0);
    let s = saved(&lab);
    assert_eq!((s["tokens_enabled"].clone(), s["tokens_asked"].clone()), (json!(false), json!(true)));
    assert!(s.get("token_features").is_none_or(|f| f.as_object().is_some_and(|o| o.values().all(|v| v == &json!(false)))));
    let flat = t.flat();
    assert!(flat.contains("Smart features stay off") && flat.contains("press s in platoon"), "{flat}");
    let mut t = Term::spawn(&["setup"], &setup_env(&lab), &lab.base, 50, 110);
    assert_eq!(t.wait_exit(), 0);
    assert!(!t.flat().contains("Turn these features on?"));
}

#[test]
fn setup_without_a_terminal_asks_nothing_leaves_everything_off_and_prints_one_line() {
    let lab = Lab::new("tok-setup-pipe");
    let mut c = Command::new(BIN);
    c.env("PLATOON_NO_UPDATE_CHECK", "1"); // no version checks: never the network, never `claude --version`
    for (k, v) in common::NO_REAL_CODEX {
        c.env(k, v); // never the real Codex: its folder or its program
    }
    c.arg("setup").env("NO_COLOR", "1").stdin(Stdio::null());
    for (k, v) in setup_env(&lab) {
        c.env(k, v);
    }
    let out = c.output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let o = text(&out.stdout);
    assert!(o.contains("All set."), "{o}");
    let hint: Vec<&str> = o.lines().filter(|l| l.contains("Smart features")).collect();
    assert_eq!(hint.len(), 1, "{o}");
    assert!(
        hint[0].contains("off") && hint[0].contains("platoon setup") && hint[0].contains("Enable features which require tokens"),
        "{o}"
    );
    assert!(!lab.config().join("settings.json").exists(), "nothing saved: {:?}", listing(&lab.config()));
    assert!(!o.contains("[y/N]"));
    // With piped input too: a `y` on stdin is not an answer when stdout is not a terminal.
    let mut c = Command::new(BIN);
    c.env("PLATOON_NO_UPDATE_CHECK", "1"); // no version checks: never the network, never `claude --version`
    for (k, v) in common::NO_REAL_CODEX {
        c.env(k, v); // never the real Codex: its folder or its program
    }
    c.arg("setup").env("NO_COLOR", "1").stdin(Stdio::piped()).stdout(Stdio::piped());
    for (k, v) in setup_env(&lab) {
        c.env(k, v);
    }
    let mut child = c.spawn().unwrap();
    // The answer may never be read: setup, seeing no terminal, asks nothing and can be gone before this
    // write lands (a broken pipe, and that is the point of the test), so a failed write is fine here.
    let _ = std::io::Write::write_all(child.stdin.as_mut().unwrap(), b"y\n");
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(!lab.config().join("settings.json").exists());
}

#[test]
fn the_question_never_holds_up_starting_platoon_or_changes_the_gate() {
    let lab = Lab::new("tok-setup-nostart");
    // Before setup, the gate still blocks with exit 1 and no question.
    let mut c = Command::new(BIN);
    c.env("PLATOON_NO_UPDATE_CHECK", "1"); // no version checks: never the network, never `claude --version`
    for (k, v) in common::NO_REAL_CODEX {
        c.env(k, v); // never the real Codex: its folder or its program
    }
    c.env("NO_COLOR", "1").stdin(Stdio::null());
    for (k, v) in setup_env(&lab) {
        c.env(k, v);
    }
    let out = c.output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("one-time setup") && !text(&out.stderr).contains("[y/N]"), "{}", text(&out.stderr));
    // After a non-interactive setup (question unanswered), platoon starts straight into the list.
    let mut c = Command::new(BIN);
    c.env("PLATOON_NO_UPDATE_CHECK", "1"); // no version checks: never the network, never `claude --version`
    for (k, v) in common::NO_REAL_CODEX {
        c.env(k, v); // never the real Codex: its folder or its program
    }
    c.arg("setup").stdin(Stdio::null());
    for (k, v) in setup_env(&lab) {
        c.env(k, v);
    }
    assert!(c.output().unwrap().status.success());
    let mut t = Term::spawn(&[], &setup_env(&lab), &lab.base, 24, 100);
    t.wait_for("enter open");
    let shown = t.stable();
    assert!(!shown.contains("[y/N]") && !shown.contains("Turn these features on"), "{shown}");
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);
}

#[test]
fn with_an_openai_model_the_test_call_asks_codex_and_says_so() {
    let lab = Lab::new("tok-codex");
    let claude = lab.good();
    // Codex: a folder with its list of models, and a stand-in for `codex exec`.
    let (codex_home, fake) = (lab.base.join("codex"), lab.base.join("codex-fake"));
    fs::create_dir_all(&codex_home).unwrap();
    fs::create_dir_all(fake.join("calls")).unwrap();
    fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/src/testdata/codex/models_cache.json"), codex_home.join("models_cache.json")).unwrap();
    let script = fake.join("codex");
    fs::write(&script, include_str!("fake-codex-exec.sh").replace("@DIR@", &fake.display().to_string())).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(fake.join("reply.default"), "ok").unwrap();
    lab.settings(json!({"tokens_enabled": true, "ai_model": "gpt-6-luna"}));
    let mut env = lab.env(&claude);
    env.push(("CODEX_HOME", codex_home.display().to_string()));
    env.push(("PLATOON_CODEX_BIN", script.display().to_string()));
    let mut t = Term::spawn(&["tokens", "test"], &env, &lab.base, 24, 100);
    assert_eq!(t.wait_exit(), 0);
    let screen = t.flat();
    assert!(screen.contains("Asking Codex (GPT-6-Luna) to reply with one word...") && screen.contains("Reply: ok"), "{screen}");
    assert!(screen.contains("Tokens used: 21,040") && !screen.contains("Cost Claude reported"), "{screen}");
    assert!(!lab.ran(), "Claude Code was not asked");
    let argv = fs::read_to_string(fake.join("calls/1.argv")).unwrap();
    assert!(argv.starts_with("exec\n--json\n--ephemeral\n--ignore-user-config\n-s\nread-only\n-m\ngpt-6-luna\n"), "{argv}");
    // And `platoon tokens` names it, and the program that runs it.
    let mut t = Term::spawn(&["tokens"], &env, &lab.base, 24, 100);
    assert_eq!(t.wait_exit(), 0);
    let screen = t.flat();
    assert!(screen.contains("Model for smart features: GPT-6-Luna (Codex)"), "{screen}");
    assert!(screen.contains("Model for project notes: Sonnet (Claude Code)"), "{screen}");
}
