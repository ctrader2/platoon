//! The hand-off end to end, in a real terminal with the real binary: `h` on a session that is in a project asks a
//! stand-in `claude -p` for a brief, shows it in a box, lets the user type into it, and on ctrl+s starts a **new**
//! session in the same folder and the same project with that text as its first message. Esc changes nothing.
//!
//! The "claude" here is one small script wearing two hats: asked with `-p` it answers like `claude -p` (from a file
//! the test writes), and started without it, it registers itself the way Claude does, prints how it was started and
//! then gives a shell. A real `claude` is never run, every directory is a temp dir, and the only processes
//! signalled are ones these tests started.

mod common;
use common::*;
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::Child,
};

const FAKE: &str = r#"#!/bin/sh
if [ "$1" = "-p" ]; then
  mkdir -p "$FAKE_DIR/calls"
  k=$(( $(ls "$FAKE_DIR/calls" | grep -c '\.stdin$') + 1 ))
  c="$FAKE_DIR/calls/$k"
  for a in "$@"; do printf '%s\n' "$a"; done > "$c.argv"
  cat > "$c.stdin"
  text=$(cat "$FAKE_DIR/reply")
  esc=$(printf '%s' "$text" | tr '\n' ' ' | sed 's/\\/\\\\/g; s/"/\\"/g')
  printf '{"type":"result","subtype":"success","is_error":false,"result":"%s","total_cost_usd":0.001,"usage":{"input_tokens":400,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":50}}' "$esc"
  exit 0
fi
mkdir -p "$FAKE_DIR/started"
n=$(( $(ls "$FAKE_DIR/started" | wc -l) + 1 ))
printf '%s\n' "$@" > "$FAKE_DIR/started/$n"
sid="new-000$n"
[ "$1" = "--resume" ] && sid="$2"
printf '{"pid":%s,"sessionId":"%s","cwd":"%s","kind":"interactive","startedAt":%s,"status":"idle"}' $$ "$sid" "$(pwd)" "$(date +%s%3N)" > "$CLAUDE_CONFIG_DIR/sessions/$$.json"
echo "STARTED project:${PLATOON_PROJECT:-none} sid:$sid"
echo "FIRST MESSAGE: $1"
exec sh
"#;

struct Lab {
    base: PathBuf,
    claude: PathBuf,
    cache: PathBuf,
    config: PathBuf,
    work: PathBuf,
    fake: PathBuf,
    procs: Vec<Child>,
}

impl Lab {
    fn new(name: &str) -> Self {
        let base = tmp(name);
        let lab = Self {
            claude: base.join("claude"),
            cache: base.join("cache"),
            config: base.join("config"),
            work: base.join("shop"),
            fake: base.join("fake"),
            base,
            procs: Vec::new(),
        };
        for d in [&lab.claude.join("sessions"), &lab.claude.join("projects/-shop"), &lab.cache, &lab.config, &lab.work, &lab.fake] {
            fs::create_dir_all(d).unwrap();
        }
        let script = lab.base.join("fake-claude.sh");
        fs::write(&script, FAKE).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        // The token features on. The notes ones are off: this is about the hand-off, and nothing should start a
        // notes helper behind the test's back.
        fs::write(
            lab.config.join("settings.json"),
            json!({"auto_archive_days": 0, "tokens_enabled": true, "token_features": {"project_notes": false, "project_description": false}}).to_string(),
        )
        .unwrap();
        lab
    }

    fn env(&self) -> Vec<(&'static str, String)> {
        vec![
            ("CLAUDE_CONFIG_DIR", self.claude.to_string_lossy().into()),
            ("PLATOON_CACHE_DIR", self.cache.to_string_lossy().into()),
            ("PLATOON_CONFIG_DIR", self.config.to_string_lossy().into()),
            ("PLATOON_CLAUDE_BIN", self.base.join("fake-claude.sh").to_string_lossy().into()),
            ("PLATOON_SKIP_SETUP_CHECK", "1".into()),
            ("PLATOON_NOTES_HELPER", "off".into()),
            ("FAKE_DIR", self.fake.to_string_lossy().into()),
        ]
    }

    /// One project, "Web", with `session` in it and notes of its own.
    fn project(&self, session: &str) {
        let v = json!({
            "version": 1,
            "projects": {"pweb": {"name": "Web", "color": "blue", "badge": "WB", "archived": false, "created": 1, "archived_sessions": []}},
            "sessions": {session: "pweb"},
        });
        fs::write(self.config.join("projects.json"), v.to_string()).unwrap();
        fs::create_dir_all(self.config.join("projects/pweb")).unwrap();
        fs::write(self.config.join("projects/pweb/project.md"), "# Overview\n\nSell socks.\n").unwrap();
    }

    fn save(&self, id: &str, prompt: &str) {
        let line = json!({"type":"user","cwd":self.work,"gitBranch":"main","timestamp":"2026-09-24T10:00:00.000Z","message":{"role":"user","content":prompt}}).to_string();
        fs::write(self.claude.join("projects/-shop").join(format!("{id}.jsonl")), line + "\n").unwrap();
    }

    fn reply(&self, text: &str) {
        fs::write(self.fake.join("reply"), text).unwrap();
    }

    fn calls(&self) -> usize {
        fs::read_dir(self.fake.join("calls")).map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".stdin")).count()).unwrap_or(0)
    }

    fn sent(&self, k: usize) -> String {
        fs::read_to_string(self.fake.join("calls").join(format!("{k}.stdin"))).unwrap_or_default()
    }

    /// The whole argument list the `k`th interactive claude was started with.
    fn started(&self, k: usize) -> String {
        fs::read_to_string(self.fake.join("started").join(k.to_string())).unwrap_or_default()
    }

    fn projects(&self) -> Value {
        fs::read_to_string(self.config.join("projects.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
    }

    fn ledger(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.cache.join("ai-usage.json")).unwrap_or_default()).unwrap_or(Value::Null)
    }

    /// End the fake claudes the hosts run (an interactive `sh` ignores SIGTERM), by the exact pids they published.
    fn end_hosts(&self) {
        for e in fs::read_dir(self.cache.join("hosts")).into_iter().flatten().flatten() {
            if e.path().extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let Some(v) = fs::read_to_string(e.path()).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { continue };
            if let Some(pid) = v["claude_pid"].as_u64().map(|p| p as u32).filter(|p| *p > 1) {
                unsafe { libc::kill(pid as i32, libc::SIGKILL) };
                until("the fake claude to exit", || !alive(pid));
            }
        }
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        self.end_hosts();
        for c in &mut self.procs {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = fs::remove_dir_all(&self.base);
    }
}

#[test]
fn h_writes_a_brief_the_user_edits_and_accepts_and_a_new_session_starts_in_the_same_project() {
    let lab = Lab::new("handoff-e2e");
    lab.save("sess-1", "Add a basket page to the shop");
    lab.project("sess-1");
    lab.reply(r#"{"summary":"The basket page is in.","done":["Baskets are session based"],"next":["Take card payments."]}"#);

    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 40, 110);
    t.wait_for("Add a basket page to the shop");
    t.wait_for_flat("h hand-off");

    // The retired key does nothing at all now. (A real key press, then a settled screen to prove it.)
    t.send(b"H");
    std::thread::sleep(std::time::Duration::from_millis(400));
    let screen = t.stable();
    assert!(!screen.contains("hand this work on"), "H is no longer the hand-off:\n{screen}");

    // h: the box opens and Claude is asked. The brief fills one frame from the title down to the keys, and half
    // a frame can be read, so every line of it is waited for together.
    t.send(b"h");
    t.wait_for_all(&[
        "hand this work on",
        "project Web",
        "# Session hand-off: shop",
        "- Project: Web",
        "## Executive summary",
        "The basket page is in.",
        "- Baskets are session based",
        "1. Take card payments.",
        "ctrl+s start the new session",
    ]);
    let screen = t.stable();
    assert!(!screen.contains("Sell socks."), "the notes are not in the brief: {screen}");

    // What was asked, and what it cost: one call, under its own name in the ledger.
    assert_eq!(lab.calls(), 1);
    let sent = lab.sent(1);
    assert!(sent.contains("Project: Web") && sent.contains("Add a basket page to the shop"), "{sent}");
    assert!(!sent.contains("sess-1"), "{sent}");
    let day = lab.ledger()["days"].as_object().unwrap().values().next().unwrap().clone();
    assert_eq!(day["features"]["handoff"]["calls"], 1, "{day}");
    assert!(day["features"]["handoff"]["tokens"].as_u64().unwrap() > 0, "{day}");

    // The user goes to the end (the cursor starts at the top), types into it, then accepts with ctrl+s.
    for _ in 0..40 {
        t.send(b"\x1b[B");
    }
    t.send(b"Sizes matter too.");
    t.wait_for("Sizes matter too.");
    t.send(b"\x13");
    t.wait_gone("hand this work on");

    // A new session started, in the same folder, in the same project, with the brief as its first message.
    t.wait_for_all(&[
        "STARTED project:pweb",
        "FIRST MESSAGE:",
        "# Session hand-off: shop",
        "Sizes matter too.", // the user's own words went with it
    ]);
    let args = lab.started(1);
    assert!(args.starts_with("# Session hand-off: shop\n\n- Written: "), "one argument, the document: {args}");
    assert!(args.contains("## Executive summary\n\nThe basket page is in.\n"), "{args}");
    assert!(args.trim_end().ends_with("1. Take card payments.\nSizes matter too."), "{args}");

    // The new session is put in the project as soon as platoon sees its id; the old one is untouched.
    until("the new session to be placed in the project", || lab.projects()["sessions"]["new-0001"] == json!("pweb"));
    assert_eq!(lab.projects()["sessions"]["sess-1"], json!("pweb"));
    assert!(!lab.config.join("archive.json").exists() || !fs::read_to_string(lab.config.join("archive.json")).unwrap().contains("sess-1"));
    assert_eq!(lab.calls(), 1, "nothing else was asked of Claude");
}

#[test]
fn esc_leaves_the_hand_off_and_everything_else_exactly_as_it_was() {
    let lab = Lab::new("handoff-esc");
    lab.save("sess-1", "Add a basket page to the shop");
    lab.project("sess-1");
    lab.reply(r#"{"summary":"The basket page is in.","next":["Take card payments."]}"#);

    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 40, 110);
    t.wait_for("Add a basket page to the shop");
    t.send(b"h");
    t.wait_for_all(&["hand this work on", "## Executive summary", "esc cancel"]);
    t.send(b"\x1b");
    t.wait_gone("hand this work on");
    t.wait_for("Add a basket page to the shop");

    // No session was started, nothing joined or left the project, and nothing was archived.
    assert!(!lab.fake.join("started").exists(), "no claude was ever started");
    assert_eq!(lab.projects()["sessions"], json!({"sess-1": "pweb"}));
    assert!(!lab.cache.join("hosts").join("x").exists());
    assert_eq!(lab.calls(), 1, "the one call it takes to write a brief, and no more");
}

#[test]
fn a_session_in_no_project_is_told_what_to_do_instead_and_nothing_is_asked() {
    let lab = Lab::new("handoff-loose");
    lab.save("loose-1", "Something else entirely");
    lab.project("sess-1"); // the project exists, but this session is not in it

    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 40, 110);
    t.wait_for("Something else entirely");
    t.send(b"h");
    t.wait_for_flat("is in no project. Press m to put it in one first.");
    assert_eq!(lab.calls(), 0, "nothing was asked of Claude");
    assert!(!lab.fake.join("started").exists());
}

#[test]
fn a_long_document_wraps_and_scrolls_in_the_box_and_the_new_session_gets_all_of_it() {
    let lab = Lab::new("handoff-long");
    lab.save("sess-1", "Add a basket page to the shop");
    lab.project("sess-1");
    let summary = "The basket page shows every item with its size, colour and price, and the totals at the foot of it \
                   now add up across currencies once the exchange rates are fetched when the page first loads.";
    let steps: Vec<String> = (1..=8)
        .map(|i| format!("Step {i}: check the totals, the sizes, the colours and the prices on the basket page before moving on to step {}", i + 1))
        .collect();
    let next: Vec<String> = steps.iter().map(|s| format!("\"{s}\"")).collect();
    lab.reply(&format!(r#"{{"summary":"{summary}","done":["Built the basket page"],"next":[{}]}}"#, next.join(",")));

    // A small terminal: the box is 64 columns wide and shows 13 rows of text.
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 22, 70);
    t.wait_for("Add a basket page to the shop");
    t.send(b"h");
    t.wait_for_all(&["hand this work on", "# Session hand-off: shop", "more lines below", "esc cancel"]);
    let screen = t.stable();
    assert!(!screen.contains("more lines above") && !screen.contains("more line above"), "{screen}");
    // Nothing is cut at the box's edge: every row of text ends inside the box, with its border after it.
    for line in screen.lines().filter(|l| l.contains("│ - ") || l.contains("│ # ")) {
        assert!(line.trim_end().ends_with("│   │") || line.trim_end().ends_with('│'), "{line:?}\n{screen}");
        assert!(!line.contains('…'), "a row was cut short: {line:?}\n{screen}");
    }

    // One PageDown: the summary, wrapped over several rows, every word of it whole.
    t.send(b"\x1b[6~");
    t.wait_for_all(&["more lines above", "The basket page shows", "first loads."]);
    let screen = t.stable();
    let words: Vec<&str> = screen.split(|c: char| c.is_whitespace() || c == '│').filter(|w| !w.is_empty()).collect();
    for w in summary.split_whitespace() {
        assert!(words.contains(&w), "{w:?} is not whole on screen:\n{screen}");
    }

    // On to the end: the last step in view, nothing more below.
    for _ in 0..6 {
        t.send(b"\x1b[6~");
    }
    t.wait_for_all(&["8. Step 8: check", "moving on to step 9", "more lines above"]);
    t.wait_gone("more lines below");

    // ctrl+s: the new session gets the whole document, not just what was on screen.
    t.send(b"\x13");
    t.wait_gone("hand this work on");
    // (The stand-in prints the whole document, so its first line scrolls off this small screen: read its record.)
    until("the new session's arguments to be written", || lab.started(1).contains("## Next steps"));
    let args = lab.started(1);
    assert!(args.starts_with("# Session hand-off: shop\n\n- Written: "), "{args}");
    assert!(args.contains(&format!("## Executive summary\n\n{summary}\n")), "{args}");
    assert!(args.contains("## Work completed\n\n- Built the basket page\n"), "{args}");
    for (n, s) in steps.iter().enumerate() {
        assert!(args.contains(&format!("{}. {s}\n", n + 1)), "step {}: {args}", n + 1);
    }
    assert_eq!(args.lines().filter(|l| l.starts_with("## ")).count(), 3, "summary, work completed, next steps: {args}");
    until("the new session to be placed in the project", || lab.projects()["sessions"]["new-0001"] == json!("pweb"));
}
