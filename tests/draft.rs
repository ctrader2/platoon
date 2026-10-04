//! Tasks drafted from a conversation, end to end in a real terminal with the real binary: `t` on a session's row asks
//! a stand-in `claude -p` (answering from a file), the row says "Drafting tasks…" meanwhile, the popup shows the draft,
//! and `enter` makes the tasks (in the session's project, remembering where they came from) and opens the hand-over
//! popup for the one that can start; the chord inside an open session (`alt+n`) goes back to the list and drafts from
//! that session, and `k` makes the tasks without handing them over.
//!
//! The "claude" here is one small script wearing two hats: asked with `-p` it answers like `claude -p` (from
//! `$FAKE_DIR/reply`), and started without it, it registers itself the way Claude does, prints a line and gives a
//! shell. A real `claude` is never run, every directory is a temp dir, and the only processes signalled are the ones
//! these tests started.

mod common;
use common::*;
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

const FAKE: &str = r#"#!/bin/sh
if [ "$1" = "-p" ]; then
  mkdir -p "$FAKE_DIR/calls"
  k=$(( $(ls "$FAKE_DIR/calls" | grep -c '\.stdin$') + 1 ))
  c="$FAKE_DIR/calls/$k"
  for a in "$@"; do printf '%s\n' "$a"; done > "$c.argv"
  cat > "$c.stdin"
  sleep 0.3
  text=$(cat "$FAKE_DIR/reply")
  esc=$(printf '%s' "$text" | tr '\n' ' ' | sed 's/\\/\\\\/g; s/"/\\"/g')
  printf '{"type":"result","subtype":"success","is_error":false,"result":"%s","total_cost_usd":0.001,"usage":{"input_tokens":400,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":50}}' "$esc"
  exit 0
fi
sid=""
[ "$1" = "--resume" ] && sid="$2"
printf '{"pid":%s,"sessionId":"%s","cwd":"%s","kind":"interactive","startedAt":%s,"status":"idle"}' $$ "$sid" "$(pwd)" "$(date +%s)000" > "$CLAUDE_CONFIG_DIR/sessions/$$.json"
echo "FAKE SESSION sid:$sid"
exec sh
"#;

const DRAFT: &str = r#"{"tasks":[
  {"title":"Order emails in plain text too","does":"Every order email also has a plain-text part with the same items.","acceptance":["A text-only mail client gets the whole order"],"how_far":"local changes only","waits_for":[],"squad":"Coding"},
  {"title":"Document the plain-text emails","does":"The guide says order emails come in two parts.","acceptance":["The guide's email page says so"],"waits_for":[1],"squad":"Docs"}
]}"#;

struct Lab {
    base: PathBuf,
    claude: PathBuf,
    cache: PathBuf,
    config: PathBuf,
    work: PathBuf,
    fake: PathBuf,
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
        };
        for d in [&lab.claude.join("sessions"), &lab.claude.join("projects/-shop"), &lab.cache, &lab.config, &lab.work, &lab.fake] {
            fs::create_dir_all(d).unwrap();
        }
        let script = lab.base.join("fake-claude.sh");
        fs::write(&script, FAKE).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        // The token features on; the notes ones off, so nothing starts a notes helper behind the test's back.
        fs::write(
            lab.config.join("settings.json"),
            json!({"auto_archive_days": 0, "tokens_enabled": true, "tokens_asked": true, "token_features": {"project_notes": false, "project_description": false}}).to_string(),
        )
        .unwrap();
        // One project, "Web", with the conversation in it.
        let v = json!({
            "version": 1,
            "projects": {"pweb": {"name": "Web", "color": "blue", "badge": "WB", "archived": false, "created": 1, "archived_sessions": []}},
            "sessions": {"sess-1": "pweb"},
        });
        fs::write(lab.config.join("projects.json"), v.to_string()).unwrap();
        fs::create_dir_all(lab.config.join("projects/pweb")).unwrap();
        let line = json!({"type":"user","cwd":lab.work,"gitBranch":"main","timestamp":"2026-09-24T10:00:00.000Z","message":{"role":"user","content":"The order email is HTML only. A plain-text part can wait for later."}}).to_string();
        fs::write(lab.claude.join("projects/-shop/sess-1.jsonl"), line + "\n").unwrap();
        fs::write(lab.fake.join("reply"), DRAFT).unwrap();
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

    fn tasks(&self) -> Value {
        fs::read_to_string(self.config.join("projects/pweb/tasks.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null)
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
        let _ = fs::remove_dir_all(&self.base);
    }
}

#[test]
fn t_on_a_row_drafts_the_tasks_and_enter_makes_them_and_offers_each_that_can_start_to_a_squad() {
    let lab = Lab::new("draft-t");
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 40, 140);
    t.wait_for_all(&["The order email is HTML only", "t draft tasks"]);
    t.send(b"t");
    // Meanwhile the row says so, and platoon keeps going.
    t.wait_for("Drafting tasks…");
    t.wait_for_all(&[
        "Tasks from this conversation",
        "Order emails in plain text too",
        "Squad      Coding (suggested)",
        "Document the plain-text emails",
        "Waits for  Order emails in plain text too",
        "enter make them and hand over",
    ]);
    let sent = fs::read_to_string(lab.fake.join("calls/1.stdin")).unwrap();
    assert!(sent.contains("Project: Web") && sent.contains("- The order email is HTML only."), "{sent}");
    // enter: both are made; the first can start, so the hand-over popup opens for it with Coding picked.
    t.send(b"\r");
    t.wait_for_all(&["Hand to a squad", "● Coding", "enter hand it over"]);
    let v = lab.tasks();
    let all = v["tickets"].as_array().unwrap();
    assert_eq!(all.len(), 2, "{v}");
    assert_eq!((all[0]["title"].as_str(), all[1]["blocked_by"].clone()), (Some("Order emails in plain text too"), json!([1])));
    assert_eq!(all[0]["from"], json!({"session": "sess-1", "project": "pweb"}));
    assert_eq!(all[0]["folders"], json!([lab.work.to_string_lossy()]));
    let body = fs::read_to_string(lab.config.join("projects/pweb/tasks").join(all[0]["file"].as_str().unwrap())).unwrap();
    assert!(body.starts_with("## What it should do\n\nEvery order email also has a plain-text part with the same items.\n"), "{body}");
    // esc there leaves it as Can start; the second waits for the first, so that is all, and the line says so.
    t.send(b"\x1b");
    t.wait_gone("Hand to a squad");
    t.wait_for_flat("Made #1 and #2 in Web. #2 waits for another");
    t.send(&[3]);
    t.wait_exit();
}

#[test]
fn the_chord_inside_an_open_session_goes_back_and_drafts_from_it_and_k_makes_them_for_later() {
    let lab = Lab::new("draft-chord");
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 40, 140);
    t.wait_for("The order email is HTML only");
    t.send(b"\r");
    t.wait_for_all(&["FAKE SESSION sid:sess-1", "ctrl+space back to list", "alt+n tasks from this conversation"]);
    // alt+n: ESC n, as a terminal sends it.
    t.send(b"\x1bn");
    t.wait_for_all(&["Tasks from this conversation", "Order emails in plain text too", "k make them for later"]);
    t.send(b"k");
    t.wait_gone("Tasks from this conversation");
    t.wait_for_flat("Made 2 tasks in Web: #1 and #2. S on each hands it to a squad.");
    let v = lab.tasks();
    assert_eq!(v["tickets"].as_array().map(Vec::len), Some(2), "{v}");
    assert!(v["tickets"].as_array().unwrap().iter().all(|t| t["state"] == "new"), "{v}");
    t.send(&[3]);
    t.wait_exit();
}
