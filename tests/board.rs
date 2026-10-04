//! The Tasks tab end to end, in a real terminal with the real binary: the four tabs on `1` to `4`, the board across two
//! projects with the header's count, `h` and back, `m` into another project, `y` continuing a squad whose session had
//! stopped and then typing an answer into it, and `n` with "No project" through the folder picker.
//!
//! The "claude" here is a stand-in script (the same as `tests/squads.rs`'): it writes down how it was started,
//! registers itself the way Claude does, prints a line, and then writes down what is typed into it. A real `claude` is
//! never run, every directory is a temp dir, and the only processes signalled are the ones these tests started.

mod common;
use common::*;
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

const FAKE: &str = r#"#!/bin/sh
mkdir -p "$FAKE_DIR/started"
n=$(( $(ls "$FAKE_DIR/started" | wc -l) + 1 ))
d="$FAKE_DIR/started/$n"
mkdir -p "$d"
i=0
for a in "$@"; do i=$((i+1)); printf '%s' "$a" > "$d/arg$i"; done
echo "$i" > "$d/argc"
pwd > "$d/cwd"
echo $$ > "$d/pid"
sid=""
prev=""
for a in "$@"; do
  [ "$prev" = "--session-id" ] && sid="$a"
  [ "$prev" = "--resume" ] && sid="$a"
  prev="$a"
done
printf '{"pid":%s,"sessionId":"%s","cwd":"%s","kind":"interactive","startedAt":%s,"status":"idle"}' $$ "$sid" "$(pwd)" "$(date +%s)000" > "$CLAUDE_CONFIG_DIR/sessions/$$.json"
echo "FAKE LEADER sid:$sid"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$d/typed"
  echo "GOT:$line"
done
"#;

const SID: &str = "1a2b3c4d-0000-4000-8000-0000000000b1";

struct Lab {
    base: PathBuf,
    claude: PathBuf,
    cache: PathBuf,
    config: PathBuf,
    work: PathBuf,
    fake: PathBuf,
}

impl Lab {
    /// Two projects, Web (WB) and Docs (DC), the user's session in Web. Web's #1 asks a question (its squad's session
    /// isn't running), its #2 can start; Docs' #1 can start, and Docs has given out numbers up to 4.
    fn new(name: &str) -> Self {
        let base = tmp(name);
        let lab = Self {
            claude: base.join("claude"),
            cache: base.join("cache"),
            config: base.join("config"),
            work: base.join("work"),
            fake: base.join("fake"),
            base,
        };
        for d in [&lab.claude.join("sessions"), &lab.claude.join("projects/-work"), &lab.cache, &lab.config, &lab.work, &lab.fake] {
            fs::create_dir_all(d).unwrap();
        }
        let script = lab.base.join("fake-claude.sh");
        fs::write(&script, FAKE).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(lab.config.join("settings.json"), r#"{"auto_archive_days":0}"#).unwrap();
        let v = json!({"version": 1, "projects": {
            "pweb": {"name": "Web", "color": "blue", "badge": "WB", "archived": false, "created": 1, "archived_sessions": []},
            "pdoc": {"name": "Docs", "color": "green", "badge": "DC", "archived": false, "created": 2, "archived_sessions": []}},
            "sessions": {"s-1": "pweb"}});
        fs::write(lab.config.join("projects.json"), v.to_string()).unwrap();
        let line = json!({"type":"user","cwd":lab.work,"gitBranch":"main","timestamp":"2026-09-24T10:00:00.000Z","message":{"role":"user","content":"make the shop"}}).to_string();
        fs::write(lab.claude.join("projects/-work/s-1.jsonl"), line + "\n").unwrap();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let work = lab.work.to_string_lossy().to_string();
        lab.tasks(
            "pweb",
            5,
            &[
                json!({"number": 1, "title": "Basket", "file": "01-basket.md", "state": "waiting", "reason": "Asks: Red or blue?",
                    "folders": [work], "blocked_by": [], "created": 1, "updated": now - 600,
                    "leaders": [{"squad": "coding", "session": SID, "started": now - 900, "names": "squad-role"}]}),
                json!({"number": 2, "title": "Checkout", "file": "02-checkout.md", "state": "new", "folders": [work],
                    "blocked_by": [], "created": 1, "updated": now - 60}),
            ],
        );
        lab.tasks(
            "pdoc",
            5,
            &[json!({"number": 1, "title": "Guide", "file": "01-guide.md", "state": "new", "folders": [work], "blocked_by": [], "created": 1, "updated": now - 120})],
        );
        lab
    }

    fn tasks(&self, project: &str, next: u32, list: &[Value]) {
        let dir = self.config.join("projects").join(project);
        fs::create_dir_all(dir.join("tasks")).unwrap();
        for t in list {
            fs::write(dir.join("tasks").join(t["file"].as_str().unwrap()), "## What it should do\n\nIt works.\n").unwrap();
        }
        fs::write(dir.join("tasks.json"), json!({"next": next, "tickets": list}).to_string()).unwrap();
    }

    fn tasks_of(&self, project: Option<&str>) -> Value {
        let path = match project {
            Some(p) => self.config.join("projects").join(p).join("tasks.json"),
            None => self.config.join("tasks.json"),
        };
        serde_json::from_str(&fs::read_to_string(path).unwrap_or_else(|_| "{}".into())).unwrap()
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

    fn started(&self) -> usize {
        fs::read_dir(self.fake.join("started")).map(|d| d.flatten().filter(|e| e.path().join("pid").exists()).count()).unwrap_or(0)
    }

    fn rec(&self, k: usize, what: &str) -> String {
        fs::read_to_string(self.fake.join("started").join(k.to_string()).join(what)).unwrap_or_default()
    }

    /// End the stand-in claudes the hosts run, by the exact pids they published.
    fn end_hosts(&self) {
        for e in fs::read_dir(self.cache.join("hosts")).into_iter().flatten().flatten() {
            if e.path().extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let Some(v) = fs::read_to_string(e.path()).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { continue };
            if let Some(pid) = v["claude_pid"].as_u64().map(|p| p as u32).filter(|p| *p > 1) {
                unsafe { libc::kill(pid as i32, libc::SIGKILL) };
                until("the stand-in claude to exit", || !alive(pid));
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
fn the_tabs_switch_on_one_to_four_and_the_board_puts_a_task_aside_and_moves_one() {
    let lab = Lab::new("bd-tabs");
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 40, 130);
    t.wait_for("enter open");
    t.wait_for_flat("1 Sessions 2 Tasks 3 Projects 4 Squads");
    // 2: the board, across both projects, with how many need you in the header.
    t.send(b"2");
    t.wait_for_all(&[
        "Needs you · 1",
        "#1 Basket",
        "a question",
        "Coding squad asks: Red or blue?",
        "Can start · 2",
        "#2 Checkout",
        "#1 Guide",
    ]);
    t.wait_for("● 1 needs you");
    let screen = t.stable();
    assert!(screen.contains("WB  #1 Basket") && screen.contains("DC  #1 Guide"), "each with its project's badge:\n{screen}");
    // 3, 4, 1: the other tabs, the count still in the header.
    t.send(b"3");
    t.wait_for_all(&["Active", "Web", "Docs", "● 1 needs you"]);
    t.send(b"4");
    t.wait_for("[Squads]");
    t.send(b"1");
    t.wait_for("NAME");
    t.send(b"2");
    t.wait_for("Needs you · 1");
    // h on #1 (picked: the first row): under Later, and nothing needs you now.
    t.send(b"h");
    t.wait_for_all(&["Later · 1", "#1 is under Later until its squad says something new."]);
    t.wait_gone("needs you");
    assert!(lab.tasks_of(Some("pweb"))["tickets"][0].get("aside").is_some());
    // ... and back: it is the last row.
    t.send(b"G");
    t.send(b"h");
    t.wait_for_all(&["Needs you · 1", "#1 needs you again."]);
    // m on #2 Checkout (the first under Can start, the latest change): into Docs, with Docs' next number.
    t.send(b"g");
    t.send(b"j");
    t.send(b"m");
    t.wait_for_all(&["Move #2", "Where to? It is in Web now.", "No project"]);
    t.send(b"\r");
    t.wait_for("Moved #2 to Docs: it is #5 there.");
    t.wait_for("DC  #5 Checkout");
    let docs = lab.tasks_of(Some("pdoc"));
    let moved = docs["tickets"].as_array().unwrap().iter().find(|x| x["number"] == 5).cloned().unwrap();
    assert_eq!((moved["title"].as_str(), moved["old_number"].as_u64()), (Some("Checkout"), Some(2)));
    assert!(lab.config.join("projects/pdoc/tasks/05-checkout.md").exists(), "its text went with it");
    assert!(lab.tasks_of(Some("pweb"))["tickets"].as_array().unwrap().iter().all(|x| x["number"] != 2));
    assert_eq!(lab.started(), 0, "nothing was started");
    t.send(&[3]);
    t.wait_exit();
}

#[test]
fn y_continues_a_squad_whose_session_stopped_then_types_the_answer_into_it() {
    let lab = Lab::new("bd-answer");
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 40, 130);
    t.wait_for("enter open");
    t.send(b"2");
    t.wait_for_all(&["Needs you · 1", "#1 Basket", "The squad asks", "y answer"]);
    // Its session isn't running: y continues it, beside the task, and says to type the answer there.
    t.send(b"y");
    t.wait_for_all(&[&format!("FAKE LEADER sid:{SID}"), "ctrl+space back to the tasks"]);
    until("the squad to be continued", || lab.started() == 1);
    t.send(&[0]); // ctrl+space: back to the board
    t.wait_gone("ctrl+space back to the tasks");
    t.wait_for("Needs you · 1");
    // Now it runs: y opens the answer box, and enter types the answer into its session.
    t.send(b"y");
    t.wait_for_all(&["Answer the Coding squad on #1", "Red or blue?", "enter send"]);
    t.send(b"Blue, the darker one");
    t.wait_for("❯ Blue, the darker one");
    t.send(b"\r");
    t.wait_for("Answer sent to #1.");
    until("the answer to reach the squad's session", || lab.rec(1, "typed").contains("Blue, the darker one"));
    assert_eq!(lab.rec(1, "typed"), "Blue, the darker one\n", "the words, then Enter");
    t.send(&[3]);
    t.wait_exit();
}

#[test]
fn n_with_no_project_picks_the_tasks_folder_before_anything_is_written() {
    let lab = Lab::new("bd-new");
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 40, 130);
    t.wait_for("enter open");
    t.send(b"2");
    t.wait_for("Needs you · 1");
    t.send(b"n");
    t.wait_for_all(&["Which project is it in?", "Web", "Docs", "No project"]);
    t.send(b"G\r");
    t.wait_for("New task in no project");
    t.send(b"Try the new linter\r");
    t.wait_for("The folder the new task works in");
    assert!(!lab.config.join("tasks.json").exists(), "nothing is written before a folder is picked");
    let work = lab.work.to_string_lossy().to_string();
    t.send(work.as_bytes());
    t.send(b"\r");
    until("the task to be written", || lab.config.join("tasks.json").exists());
    let v = lab.tasks_of(None);
    assert_eq!(v["tickets"][0]["title"], "Try the new linter");
    assert_eq!(v["tickets"][0]["folders"], json!([work]));
    t.wait_for("Made task #1 (in no project)");
    t.send(b"\x1b");
    t.wait_for_all(&["Can start · 3", "#1 Try the new linter"]);
    t.send(&[3]);
    t.wait_exit();
}
