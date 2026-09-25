//! The smart names, summaries and "Describe it" search, and the first pass in `platoon setup`, end to end: the real
//! binary in a real terminal, a fake `claude` that records what it is sent and answers from files. Every directory is a
//! temp dir, `claude` is never the real one (`PLATOON_CLAUDE_BIN`, and the environment forbids the real one), and no
//! session is started.

mod common;
use common::*;
use std::{fs, path::PathBuf, process::{Command, Stdio}, thread, time::Duration};

struct Lab {
    base: PathBuf,
}

impl Lab {
    fn new(name: &str) -> Lab {
        let base = tmp(name);
        for d in ["claude/sessions", "claude/projects/-work", "cache", "config"] {
            fs::create_dir_all(base.join(d)).unwrap();
        }
        Lab { base }
    }

    fn dir(&self, d: &str) -> PathBuf {
        self.base.join(d)
    }

    /// A saved conversation run in a folder called `id`, whose user typed `prompt`, last active a few minutes ago.
    fn save(&self, id: &str, prompt: &str, mins_ago: u64) {
        let cwd = self.base.join("work").join(id);
        fs::create_dir_all(&cwd).unwrap();
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() - mins_ago * 60;
        let ts = |s: u64| {
            // 2026-09-20 is far enough from any month boundary that a plain conversion by hand is not needed: use the
            // seconds of the day on today's date via `date`.
            let out = Command::new("date").args(["-u", "-d", &format!("@{s}"), "+%Y-%m-%dT%H:%M:%S.000Z"]).output().unwrap();
            String::from_utf8(out.stdout).unwrap().trim().to_string()
        };
        let text = format!(
            "{}\n{}\n",
            format!(r#"{{"type":"user","cwd":"{}","gitBranch":"main","timestamp":"{}","message":{{"role":"user","content":"{}"}}}}"#, cwd.display(), ts(secs - 5), prompt),
            format!(r#"{{"type":"assistant","cwd":"{}","timestamp":"{}","message":{{"model":"claude-sonnet-5","content":[{{"type":"text","text":"On it."}}],"usage":{{"input_tokens":1000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}}}}"#, cwd.display(), ts(secs)),
        );
        fs::write(self.dir("claude/projects/-work").join(format!("{id}.jsonl")), text).unwrap();
    }

    fn settings(&self, v: serde_json::Value) {
        fs::write(self.dir("config/settings.json"), v.to_string()).unwrap();
    }

    fn on(&self) {
        self.settings(serde_json::json!({"tokens_enabled": true, "tokens_asked": true, "token_features": {"names": true, "find": true}, "auto_archive_days": 0}));
    }

    fn env(&self, fake: &AiFake) -> Vec<(&'static str, String)> {
        vec![
            ("CLAUDE_CONFIG_DIR", self.dir("claude").display().to_string()),
            ("PLATOON_CACHE_DIR", self.dir("cache").display().to_string()),
            ("PLATOON_CONFIG_DIR", self.dir("config").display().to_string()),
            ("PLATOON_CLAUDE_BIN", fake.script.clone()),
            ("PLATOON_SKIP_SETUP_CHECK", "1".into()),
        ]
    }

    fn summaries(&self) -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(self.dir("config/summaries.json")).unwrap_or_else(|_| "{}".into())).unwrap_or_default()
    }
}

fn names_reply(fake: &AiFake) {
    fake.reply_default(r#"[{"n":1,"name":"Newest topic","summary":"About the newest thing."},{"n":2,"name":"Middle topic","summary":"About the middle thing."},{"n":3,"name":"Oldest topic","summary":"About the oldest thing."}]"#);
}

#[test]
fn the_list_fills_in_generated_names_with_a_mark_and_the_summary_under_it() {
    let lab = Lab::new("smart-names");
    lab.save("alpha", "please fix the login redirect after sign in", 5);
    lab.save("beta", "add a csv export to the invoices report", 50);
    lab.save("gamma", "why does the nightly build fail on ci", 500);
    lab.on();
    let fake = AiFake::new("smart-names");
    names_reply(&fake);
    let mut t = Term::spawn(&[], &lab.env(&fake), &lab.base, 24, 100);
    t.wait_for_all(&["Newest topic", "Oldest topic", "✦", "About the newest thing."]);
    assert_eq!(fake.calls(), 1, "three conversations, one call");
    assert!(!fake.stdin(1).contains(lab.base.to_str().unwrap()), "no path is sent");
    // The files: names and summaries in the config folder, nothing in Claude's.
    assert_eq!(lab.summaries()["entries"].as_object().map(|m| m.len()), Some(3));
    assert!(!fs::read_to_string(lab.dir("config/summaries.json")).unwrap().contains("login redirect"), "no message text");
    assert_eq!(fs::read_dir(lab.dir("claude/projects")).unwrap().count(), 1, "no new folder appeared under projects");
    // A second start shows them at once without asking again.
    t.send(b"q");
    t.wait_exit();
    let calls = fake.calls();
    let mut t2 = Term::spawn(&[], &lab.env(&fake), &lab.base, 24, 100);
    t2.wait_for("Newest topic");
    thread::sleep(Duration::from_millis(600));
    assert_eq!(fake.calls(), calls, "nothing to do the second time");
}

#[test]
fn describe_it_runs_one_call_on_enter_shows_results_and_enter_jumps_and_esc_cancels_with_no_process_left() {
    let lab = Lab::new("smart-find");
    lab.save("alpha", "please fix the login redirect after sign in", 5);
    lab.save("beta", "add a csv export to the invoices report", 50);
    lab.save("gamma", "why does the nightly build fail on ci", 500);
    // Only the search is on, so the names worker doesn't add calls of its own.
    lab.settings(serde_json::json!({"tokens_enabled": true, "tokens_asked": true, "token_features": {"names": false, "find": true}, "auto_archive_days": 0}));
    let fake = AiFake::new("smart-find");
    fake.reply_default(r#"[{"n":2,"reason":"Talks about the invoices export"},{"n":1,"reason":"Also mentions reports"}]"#);
    let mut t = Term::spawn(&[], &lab.env(&fake), &lab.base, 24, 100);
    t.wait_for("alpha");
    t.send(b"/");
    t.wait_for("Quick filter");
    t.send(b"\t\t");
    t.wait_for("Describe what you remember");
    t.send(b"that invoices thing");
    t.wait_for("Press enter to ask Claude");
    thread::sleep(Duration::from_millis(400));
    assert_eq!(fake.calls(), 0, "typing asks nothing");
    t.send(b"\r");
    t.wait_for("Talks about the invoices export");
    assert_eq!(fake.calls(), 1);
    let sent = fake.stdin(1);
    assert!(sent.starts_with("Description: that invoices thing"), "{sent}");
    assert!(t.text().contains("Also mentions reports") && t.text().contains("2 found") && t.text().contains("ctrl+w search wider"));
    // Enter opens the first result (the one Claude ranked first) in the list behind, and the dialog is gone.
    t.send(b"\r");
    t.wait_gone("Also mentions reports");
    assert_eq!(fake.calls(), 1);
    // Now a request that never answers, cancelled with esc.
    fake.hang(true);
    t.send(b"/");
    t.wait_for("Quick filter");
    t.send(b"\t\t");
    t.wait_for("Describe what you remember");
    t.send(b"anything at all");
    t.wait_for("Press enter to ask Claude");
    t.send(b"\r");
    t.wait_for("Asking Claude...");
    until("the fake to start", || fake.pids().len() == 2);
    assert!(fake.any_alive());
    t.send(b"\x1b");
    t.wait_gone("Asking Claude...");
    until("the process to be gone", || !fake.any_alive());
    assert!(t.text().contains("anything at all"), "still in the dialog, back to typing");
    // A second esc closes the dialog (wait for the first to have been read as a key on its own).
    thread::sleep(Duration::from_millis(300));
    t.send(b"\x1b");
    t.wait_gone("Describe it");
    t.send(b"q");
    t.wait_exit();
    assert!(!fake.any_alive(), "no stray process");
}

#[test]
fn describe_it_says_plainly_when_the_daily_limit_is_used_up_and_how_to_raise_it() {
    let lab = Lab::new("smart-limit");
    lab.save("alpha", "please fix the login redirect after sign in", 5);
    lab.settings(serde_json::json!({"tokens_enabled": true, "tokens_asked": true, "token_features": {"names": false, "find": true}, "ai_daily_tokens": 20000, "auto_archive_days": 0}));
    let fake = AiFake::new("smart-limit");
    // Today's use, in the ledger the way a real day would have left it.
    let day = String::from_utf8(Command::new("date").args(["+%Y-%m-%d"]).output().unwrap().stdout).unwrap().trim().to_string();
    fs::write(
        lab.dir("cache/ai-usage.json"),
        serde_json::json!({"version": 1, "days": {day: {"usage": {"input": 30000, "cache_creation": 0, "cache_read": 0, "output": 0}, "cost_usd": 0.0, "calls": 1, "failed": 0, "features": {}}}}).to_string(),
    )
    .unwrap();
    let mut t = Term::spawn(&[], &lab.env(&fake), &lab.base, 24, 100);
    t.wait_for("alpha");
    t.send(b"/");
    t.wait_for("Quick filter");
    t.send(b"\t\t");
    t.wait_for("Describe what you remember");
    t.send(b"the login thing\r");
    // Both halves of the message, which may have wrapped onto two rows that arrive one after the other.
    t.wait_for_all(&["Today's token limit is used up", "raise it in settings"]);
    assert_eq!(fake.calls(), 0);
}

#[test]
fn the_wider_search_asks_first_and_sends_more_candidates_only_after_enter() {
    let lab = Lab::new("smart-wide");
    for i in 1..=30 {
        lab.save(&format!("c{i:02}"), &format!("please look into the billing topic number {i} for me"), i * 3);
    }
    lab.settings(serde_json::json!({"tokens_enabled": true, "tokens_asked": true, "token_features": {"names": false, "find": true}, "ai_find_candidates": 10, "ai_find_wide_candidates": 50, "auto_archive_days": 0}));
    let fake = AiFake::new("smart-wide");
    let mut t = Term::spawn(&[], &lab.env(&fake), &lab.base, 24, 100);
    t.wait_for("c01");
    t.send(b"/");
    t.wait_for("Quick filter");
    t.send(b"\t\t");
    t.wait_for("Describe what you remember");
    t.send(b"billing\r");
    t.wait_for("found");
    assert_eq!(fake.stdin(1).lines().filter(|l| l.starts_with('[')).count(), 10);
    t.send(b"\x17"); // ctrl+w
    t.wait_for_flat("Search wider? This sends 30 conversations to Claude");
    t.wait_for_flat("(an estimate)"); // all thirty there are: the limit of 50 is more than they are
    assert_eq!(fake.calls(), 1, "a question, not a call");
    t.send(b"\r");
    until("the second call", || fake.calls() == 2);
    assert_eq!(fake.stdin(2).lines().filter(|l| l.starts_with('[')).count(), 30);
}

// ---- platoon setup -----------------------------------------------------------------------------------------

fn setup_lab(name: &str) -> (Lab, AiFake) {
    let lab = Lab::new(name);
    lab.save("alpha", "please fix the login redirect after sign in", 5);
    lab.save("beta", "add a csv export to the invoices report", 50);
    let fake = AiFake::new(name);
    fake.reply_default(r#"[{"n":1,"name":"Login fix","summary":"About login."},{"n":2,"name":"CSV export","summary":"About CSV."}]"#);
    (lab, fake)
}

fn setup_env(lab: &Lab, fake: &AiFake) -> Vec<(&'static str, String)> {
    let mut env = lab.env(fake);
    env.retain(|(k, _)| *k != "PLATOON_SKIP_SETUP_CHECK");
    env
}

#[test]
fn setup_names_the_active_conversations_after_yes_and_offers_archived_ones_with_skip_as_the_default() {
    let (lab, fake) = setup_lab("setup-yes");
    let mut t = Term::spawn(&["setup"], &setup_env(&lab, &fake), &lab.base, 30, 110);
    t.wait_for("Turn these features on?");
    t.send(b"y\r");
    t.wait_for_flat("Smart names: 2 active conversations to name, about");
    t.wait_for_flat("Naming conversations: 2 of 2");
    assert!(t.flat().contains("(an estimate, made on this computer)"));
    t.wait_for_flat("Used about 450 tokens.");
    assert_eq!(t.wait_exit(), 0, "{}", t.text());
    assert_eq!(fake.calls(), 1);
    assert_eq!(lab.summaries()["entries"].as_object().map(|m| m.len()), Some(2));
    let s: serde_json::Value = serde_json::from_str(&fs::read_to_string(lab.dir("config/settings.json")).unwrap()).unwrap();
    assert_eq!((s["tokens_enabled"].clone(), s["tokens_asked"].clone()), (serde_json::json!(true), serde_json::json!(true)));
    assert!(fs::read_to_string(lab.dir("claude/settings.json")).unwrap().contains("statusLine"), "setup itself did what it always does");
}

#[test]
fn setup_asks_about_archived_conversations_and_background_is_remembered() {
    let (lab, fake) = setup_lab("setup-arch");
    lab.save("old1", "please review the old archived thing", 900);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    fs::write(lab.dir("config/archive.json"), serde_json::json!({"version": 1, "archived": {"old1": now}}).to_string()).unwrap();
    let mut t = Term::spawn(&["setup"], &setup_env(&lab, &fake), &lab.base, 30, 110);
    t.wait_for("Turn these features on?");
    t.send(b"y\r");
    t.wait_for_flat("Archived: 1 archived conversation, about");
    t.wait_for_flat("[S]kip / [b]ackground:");
    t.send(b"b\r");
    t.wait_for_flat("Archived conversations will be named slowly in the background.");
    assert_eq!(t.wait_exit(), 0);
    let s: serde_json::Value = serde_json::from_str(&fs::read_to_string(lab.dir("config/settings.json")).unwrap()).unwrap();
    assert_eq!(s["ai_archived"], "background");
    assert!(!fake.stdin(1).contains("archived thing"), "not sent during setup");
}

#[test]
fn setup_after_no_asks_nothing_more_and_never_calls_claude() {
    let (lab, fake) = setup_lab("setup-no");
    let mut t = Term::spawn(&["setup"], &setup_env(&lab, &fake), &lab.base, 30, 110);
    t.wait_for("Turn these features on?");
    t.send(b"\r");
    assert_eq!(t.wait_exit(), 0);
    assert!(!t.text().contains("Smart names:"));
    assert_eq!(fake.calls(), 0);
}

#[test]
fn ctrl_c_during_the_setup_scan_skips_it_kills_the_call_and_setup_still_succeeds() {
    let (lab, fake) = setup_lab("setup-int");
    fake.hang(true);
    let mut t = Term::spawn(&["setup"], &setup_env(&lab, &fake), &lab.base, 30, 110);
    t.wait_for("Turn these features on?");
    t.send(b"y\r");
    t.wait_for_flat("Naming conversations: 0 of 2");
    until("the fake to start", || fake.pids().len() == 2);
    t.send(b"\x03");
    t.wait_for_flat("Skipped. platoon will name the rest in the background while it is open.");
    assert_eq!(t.wait_exit(), 0, "{}", t.text());
    until("the fake to be gone", || !fake.any_alive());
    let s: serde_json::Value = serde_json::from_str(&fs::read_to_string(lab.dir("config/settings.json")).unwrap()).unwrap();
    assert_eq!(s["tokens_enabled"], true, "the yes was already saved");
}

#[test]
fn setup_without_a_terminal_scans_nothing_and_says_one_line_and_its_exit_code_is_unchanged() {
    let (lab, fake) = setup_lab("setup-nontty");
    lab.on();
    let mut c = Command::new(BIN);
    c.arg("setup").env("NO_COLOR", "1").env("PLATOON_FORBID_REAL_CLAUDE", "1").stdin(Stdio::null());
    for (k, v) in setup_env(&lab, &fake) {
        c.env(k, v);
    }
    let out = c.output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("Smart names haven't been made yet") && text.contains("in the background while it is open"), "{text}");
    assert_eq!(fake.calls(), 0);
    assert!(!lab.dir("config/summaries.json").exists());
}

#[test]
fn setup_again_after_a_yes_with_nothing_named_yet_runs_the_scan_and_then_stays_quiet() {
    let (lab, fake) = setup_lab("setup-again");
    lab.on(); // said yes earlier, nothing named
    let mut t = Term::spawn(&["setup"], &setup_env(&lab, &fake), &lab.base, 30, 110);
    t.wait_for_flat("Smart names: 2 active conversations to name");
    assert_eq!(t.wait_exit(), 0);
    assert_eq!(fake.calls(), 1);
    // ...and a third run has nothing to say about it.
    let mut t = Term::spawn(&["setup"], &setup_env(&lab, &fake), &lab.base, 30, 110);
    assert_eq!(t.wait_exit(), 0);
    assert!(!t.text().contains("Smart names"), "{}", t.text());
    assert_eq!(fake.calls(), 1);
}
