//! Squads end to end, in a real terminal with the real binary: `a` on a ticket hands it to the Coding squad (a git
//! worktree in a temp repository, the leader started in a platoon host with the squad's flags), the split view (the
//! ticket beside the live session, typing reaches it, ctrl+space goes back), the leader kept out of the Sessions tab
//! and the project's list, the two hooks with real-shaped input, `p` typing the go-ahead, Cancelled stopping only
//! that leader, the narrow layout, and "isn't running. Enter to continue it" resuming with the same flags.
//!
//! The "claude" here is a stand-in script: it writes down every argument (one file each), its environment and its
//! folder, registers itself the way Claude does, prints a line, and then echoes what is typed into it. A real
//! `claude` is never run, every directory is a temp dir (the git repository too), and the only processes signalled
//! are the ones these tests started.

mod common;
use common::*;
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const FAKE: &str = r#"#!/bin/sh
if [ "$1" = "-p" ]; then
  # The squad commander's `claude -p` call: written down, and answered from $FAKE_DIR/verdict (else "it fits").
  mkdir -p "$FAKE_DIR/calls"
  k=$(( $(ls "$FAKE_DIR/calls" | grep -c '\.stdin$') + 1 ))
  for a in "$@"; do printf '%s\n' "$a"; done > "$FAKE_DIR/calls/$k.argv"
  env > "$FAKE_DIR/calls/$k.env"
  cat > "$FAKE_DIR/calls/$k.stdin"
  text='{"fits":true,"reason":"Coding work in a git repository."}'
  [ -e "$FAKE_DIR/verdict" ] && text=$(cat "$FAKE_DIR/verdict")
  esc=$(printf '%s' "$text" | sed 's/\\/\\\\/g; s/"/\\"/g')
  printf '{"type":"result","subtype":"success","is_error":false,"result":"%s","total_cost_usd":0.001,"usage":{"input_tokens":300,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":20}}' "$esc"
  exit 0
fi
mkdir -p "$FAKE_DIR/started"
n=$(( $(ls "$FAKE_DIR/started" | wc -l) + 1 ))
d="$FAKE_DIR/started/$n"
mkdir -p "$d"
i=0
for a in "$@"; do i=$((i+1)); printf '%s' "$a" > "$d/arg$i"; done
echo "$i" > "$d/argc"
env > "$d/env"
pwd > "$d/cwd"
echo $$ > "$d/pid"
sid=""
prev=""
for a in "$@"; do
  [ "$prev" = "--session-id" ] && sid="$a"
  [ "$prev" = "--resume" ] && sid="$a"
  prev="$a"
done
printf '{"pid":%s,"sessionId":"%s","cwd":"%s","kind":"interactive","startedAt":%s,"status":"idle"}' $$ "$sid" "$(pwd)" "$(date +%s%3N)" > "$CLAUDE_CONFIG_DIR/sessions/$$.json"
echo "FAKE LEADER sid:$sid"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$d/typed"
  echo "GOT:$line"
done
"#;

struct Lab {
    base: PathBuf,
    claude: PathBuf,
    cache: PathBuf,
    config: PathBuf,
    work: PathBuf,
    shop: PathBuf,
    docs: PathBuf,
    fake: PathBuf,
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

impl Lab {
    fn new(name: &str) -> Self {
        let base = tmp(name);
        let lab = Self {
            claude: base.join("claude"),
            cache: base.join("cache"),
            config: base.join("config"),
            work: base.join("work"),
            shop: base.join("shop"),
            docs: base.join("docs"),
            fake: base.join("fake"),
            base,
        };
        for d in [&lab.claude.join("sessions"), &lab.claude.join("projects/-work"), &lab.cache, &lab.config, &lab.work, &lab.shop, &lab.docs, &lab.fake] {
            fs::create_dir_all(d).unwrap();
        }
        let script = lab.base.join("fake-claude.sh");
        fs::write(&script, FAKE).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        // The smart features on: the squad commander is one of them (and has no switch of its own).
        fs::write(lab.config.join("settings.json"), r#"{"auto_archive_days":0,"tokens_enabled":true,"tokens_asked":true}"#).unwrap();
        // A temp git repository with one commit: the only repository these tests ever touch.
        fs::write(lab.shop.join("README"), "a shop\n").unwrap();
        git(&lab.shop, &["init", "-q"]);
        git(&lab.shop, &["add", "."]);
        git(&lab.shop, &["commit", "-q", "-m", "first"]);
        // One project, "Web", with the user's own session in it and notes of its own.
        let v = json!({"version": 1, "projects": {"pweb": {"name": "Web", "color": "blue", "badge": "WB", "archived": false, "created": 1, "archived_sessions": []}}, "sessions": {"s-1": "pweb"}});
        fs::write(lab.config.join("projects.json"), v.to_string()).unwrap();
        fs::create_dir_all(lab.config.join("projects/pweb/tickets")).unwrap();
        fs::write(lab.config.join("projects/pweb/project.md"), "# Overview\n\nSell socks.\n").unwrap();
        let line = json!({"type":"user","cwd":lab.work,"gitBranch":"main","timestamp":"2026-09-24T10:00:00.000Z","message":{"role":"user","content":"make the shop"}}).to_string();
        fs::write(lab.claude.join("projects/-work/s-1.jsonl"), line + "\n").unwrap();
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
            // As if platoon had been started from inside a Claude Code session, in a project.
            ("CLAUDE_CODE_CHILD_SESSION", "1".into()),
            ("CLAUDECODE", "1".into()),
            ("CLAUDE_CODE_SESSION_ID", "caller-session".into()),
            ("CLAUDE_CODE_MESSAGING_SOCKET", "/tmp/caller.sock".into()),
            ("CLAUDE_CODE_MESSAGING_TOKEN", "caller-token".into()),
            ("CLAUDE_PID", "4242".into()),
            ("AI_AGENT", "claude-code_agent".into()),
            // ... and a setting of the user's own, which must be kept.
            ("CLAUDE_CODE_USE_BEDROCK", "0".into()),
            ("PLATOON_PROJECT", "pweb".into()),
        ]
    }

    /// The commander's calls so far.
    fn checks(&self) -> usize {
        fs::read_dir(self.fake.join("calls")).map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".stdin")).count()).unwrap_or(0)
    }

    fn check_file(&self, k: usize, what: &str) -> String {
        fs::read_to_string(self.fake.join("calls").join(format!("{k}.{what}"))).unwrap_or_default()
    }

    /// Today, in the local calendar, as the ledger keys its days.
    fn today(&self) -> String {
        let out = Command::new("date").arg("+%F").output().unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    /// Put this many tokens on today's count, the way a day of real calls would.
    fn spend_the_day(&self, tokens: u64) {
        let day = json!({"usage": {"input": tokens, "cache_creation": 0, "cache_read": 0, "output": 0}, "cost_usd": 0.0, "calls": 1, "failed": 0, "features": {}});
        fs::write(self.cache.join("ai-usage.json"), json!({"version": 1, "days": {self.today(): day}}).to_string()).unwrap();
    }

    /// Ticket #1 "Basket" in the shop (a git repository) and the docs folder (not one), and whatever else is given.
    fn ticket(&self, extra: Value) {
        let mut t = json!({"number": 1, "title": "Basket", "file": "01-basket.md", "state": "new", "how_far": "Local commit",
            "folders": [self.shop.to_string_lossy(), self.docs.to_string_lossy()], "blocked_by": [], "created": 1, "updated": 1});
        for (k, v) in extra.as_object().cloned().unwrap_or_default() {
            t.as_object_mut().unwrap().insert(k, v);
        }
        fs::write(self.config.join("projects/pweb/tickets/01-basket.md"), "## What it should do\n\nA basket page.\n").unwrap();
        fs::write(self.tickets_path(), json!({"next": 2, "tickets": [t]}).to_string()).unwrap();
    }

    fn tickets_path(&self) -> PathBuf {
        self.config.join("projects/pweb/tickets.json")
    }

    fn ticket_now(&self) -> Value {
        let v: Value = serde_json::from_str(&fs::read_to_string(self.tickets_path()).unwrap()).unwrap();
        v["tickets"][0].clone()
    }

    fn started(&self) -> usize {
        fs::read_dir(self.fake.join("started")).map(|d| d.count()).unwrap_or(0)
    }

    /// The arguments the `k`th claude was started with.
    fn args(&self, k: usize) -> Vec<String> {
        let d = self.fake.join("started").join(k.to_string());
        let n: usize = fs::read_to_string(d.join("argc")).unwrap().trim().parse().unwrap();
        (1..=n).map(|i| fs::read_to_string(d.join(format!("arg{i}"))).unwrap()).collect()
    }

    fn rec(&self, k: usize, what: &str) -> String {
        fs::read_to_string(self.fake.join("started").join(k.to_string()).join(what)).unwrap_or_default()
    }

    fn pid(&self, k: usize) -> u32 {
        self.rec(k, "pid").trim().parse().unwrap()
    }

    /// Open the project's screen and move to its Tickets pane.
    fn open_tickets(&self, t: &mut Term) {
        t.wait_for("enter open");
        t.send(b"2");
        t.wait_for_flat("WB Web");
        t.send(b"\r");
        t.wait_for_all(&["╭ Sessions", "╭ Tickets", "╭ Notes"]);
        t.send(b"\t");
        t.wait_for_flat("n new ticket");
    }

    /// `platoon hook <which>` with this lab's directories and `input` on stdin: (exit code, stderr).
    fn hook(&self, which: &str, input: &Value) -> (i32, String) {
        let mut c = Command::new(BIN);
        c.args(["hook", which]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        for (k, v) in self.env() {
            c.env(k, v);
        }
        let mut child = c.spawn().unwrap();
        child.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stderr).into_owned())
    }

    /// Run a hook command exactly as the leader's `--settings` gives it (through a shell), with `input` on stdin.
    fn run_hook_command(&self, command: &str, input: &Value) -> i32 {
        let mut c = Command::new("sh");
        c.args(["-c", command]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
        for (k, v) in self.env() {
            c.env(k, v);
        }
        let mut child = c.spawn().unwrap();
        child.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
        child.wait().unwrap().code().unwrap_or(-1)
    }

    /// End the fake claudes the hosts run, by the exact pids they published.
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

fn value_after<'a>(args: &'a [String], flag: &str) -> &'a str {
    let at = args.iter().position(|a| a == flag).unwrap_or_else(|| panic!("no {flag} in {args:?}"));
    &args[at + 1]
}

/// Hand ticket #1 to the Coding squad with `a` and wait for the split view.
fn hand_over(lab: &Lab, t: &mut Term) {
    lab.open_tickets(t);
    t.send(b"a");
    t.wait_for_all(&["hand to a squad", "Coding", "enter hand it over"]);
    t.send(b"\r");
    t.wait_for_all(&["FAKE LEADER sid:", "ctrl+space back to the project"]);
}

#[test]
fn a_hands_the_ticket_to_the_coding_squad_in_a_worktree_and_shows_it_beside_the_ticket() {
    let lab = Lab::new("sq-hand");
    lab.ticket(json!({}));
    let projects_before = fs::read(lab.config.join("projects.json")).unwrap();
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 40, 130);
    hand_over(&lab, &mut t);

    // The ticket: Working, with its leader and its worktree written on it.
    let tk = lab.ticket_now();
    assert_eq!(tk["state"], "working", "{tk}");
    let sid = tk["leaders"][0]["session"].as_str().unwrap().to_string();
    assert_eq!((sid.len(), tk["leaders"][0]["squad"].as_str()), (36, Some("coding")), "{tk}");
    assert_eq!(tk["leaders"][0]["names"], "squad-role", "the names it was started with, for resuming it: {tk}");
    let wt = lab.base.join("shop-worktrees/ticket-1");
    assert_eq!(tk["worktrees"][0]["path"], wt.to_string_lossy().as_ref());
    assert_eq!(tk["worktrees"][0]["branch"], "ticket/1");
    assert!(wt.join("README").exists(), "a real worktree of the temp repository");

    // How the stand-in claude was started.
    assert_eq!(lab.started(), 1);
    let args = lab.args(1);
    assert_eq!(value_after(&args, "--session-id"), sid);
    assert_eq!(value_after(&args, "-n"), "Coding squad · #1 Basket");
    assert_eq!(value_after(&args, "--agent"), "coding-leader");
    assert_eq!(value_after(&args, "--model"), "sonnet");
    assert_eq!(value_after(&args, "--permission-mode"), "acceptEdits");
    assert_eq!(value_after(&args, "--add-dir"), lab.docs.to_string_lossy(), "the other folder, as it is");
    let agents: Value = serde_json::from_str(value_after(&args, "--agents")).unwrap();
    let models: Vec<(String, String)> = agents.as_object().unwrap().iter().map(|(k, v)| (k.clone(), v["model"].as_str().unwrap().to_string())).collect();
    assert_eq!(
        models,
        [("coding-leader", "sonnet"), ("coding-planner", "opus"), ("coding-builder", "sonnet"), ("coding-reviewer", "opus"), ("coding-verifier", "haiku")].map(|(a, b)| (a.to_string(), b.to_string()))
    );
    let leader_prompt = agents["coding-leader"]["prompt"].as_str().unwrap();
    assert!(leader_prompt.contains("SQUAD: ready to publish") && leader_prompt.contains("\n1. coding-planner: Reads the ticket"), "{leader_prompt}");
    // What it started with is kept beside the ticket, for continuing it later with the squad as it was.
    assert_eq!(tk["leaders"][0]["snapshot"], "01-squad-1.json", "{tk}");
    let snap: Value = serde_json::from_str(&fs::read_to_string(lab.config.join("projects/pweb/tickets/01-squad-1.json")).unwrap()).unwrap();
    assert_eq!(snap["agents"], agents, "the very same agents");
    assert_eq!((snap["agent"].as_str(), snap["model"].as_str(), snap["name"].as_str()), (Some("coding-leader"), Some("sonnet"), Some("Coding")));
    assert!(leader_prompt.contains("When it doesn't say how far, stop at local changes"), "{leader_prompt}");
    let settings: Value = serde_json::from_str(value_after(&args, "--settings")).unwrap();
    let stop = settings["hooks"]["Stop"][0]["hooks"][0]["command"].as_str().unwrap();
    let gate = settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"].as_str().unwrap();
    assert_eq!(settings["hooks"]["PreToolUse"][0]["matcher"], "Bash");
    assert!(stop.ends_with(" hook squad-stop") && gate.ends_with(" hook squad-gate"), "{settings}");
    assert!(settings["hooks"]["Notification"][0]["hooks"][0]["command"].as_str().unwrap().ends_with(" hook squad-notify"), "{settings}");
    assert!(settings["hooks"]["PostToolUse"][0]["hooks"][0]["command"].as_str().unwrap().ends_with(" hook squad-tool"), "{settings}");
    assert!(stop.starts_with(BIN) || stop.starts_with(&format!("'{BIN}")), "the platoon that is running: {stop}");
    let first = args.last().unwrap();
    assert!(first.starts_with("Ticket #1: Basket\nProject: Web\n\n## The ticket\n\n## What it should do\n\nA basket page."), "{first}");
    assert!(first.contains("## How far to take it\n\nAs far as the ticket's text says, and no further. When it doesn't say, stop at local changes"), "{first}");
    assert!(!first.contains("Local commit"), "an old how_far in the file is not read: {first}");
    assert!(!first.contains("Sell socks"), "the notes arrive separately");
    assert_eq!(fs::canonicalize(lab.rec(1, "cwd").trim()).unwrap(), fs::canonicalize(&wt).unwrap(), "it starts in the worktree");
    let env = lab.rec(1, "env");
    for marker in ["CLAUDE_CODE_CHILD_SESSION", "CLAUDECODE=", "CLAUDE_CODE_SESSION_ID", "CLAUDE_CODE_MESSAGING_SOCKET", "CLAUDE_CODE_MESSAGING_TOKEN", "CLAUDE_PID", "AI_AGENT"] {
        assert!(!env.contains(marker), "the calling session's {marker} is never passed on: {env}");
    }
    assert!(env.lines().any(|l| l == "CLAUDE_CODE_USE_BEDROCK=0") && env.lines().any(|l| l.starts_with("CLAUDE_CONFIG_DIR=")), "the user's own settings are: {env}");
    // The leader may hand work on, read and run things; it never edits a file itself.
    assert_eq!(agents["coding-leader"]["tools"], json!(["Agent", "Read", "Grep", "Glob", "Bash"]));
    // The commander was asked first: one `claude -p` call on Haiku, with the ticket and the squad, and the same
    // markers taken away.
    assert_eq!(lab.checks(), 1);
    let argv = lab.check_file(1, "argv");
    assert!(argv.contains("\n--model\nhaiku\n"), "{argv}");
    let asked = lab.check_file(1, "stdin");
    assert!(asked.starts_with("The squad: Coding\n") && asked.contains("The ticket: #1 Basket") && asked.contains("A basket page."), "{asked}");
    assert!(!asked.contains("How far"), "{asked}");
    assert!(asked.contains(&format!("- {} (in a git repository)", lab.shop.display())) && asked.contains(&format!("- {} (not in a git repository)", lab.docs.display())), "{asked}");
    let check_env = lab.check_file(1, "env");
    assert!(!check_env.contains("CLAUDE_CODE_SESSION_ID") && !check_env.contains("CLAUDE_CODE_CHILD_SESSION") && check_env.contains("CLAUDE_CODE_USE_BEDROCK=0"), "{check_env}");
    assert!(!env.lines().any(|l| l.starts_with("PLATOON_PROJECT=")), "a leader never joins the project: {env}");
    assert_eq!(fs::read(lab.config.join("projects.json")).unwrap(), projects_before, "projects.json is untouched");

    // The split: the ticket on the left, the session on the right; typing goes to the session.
    let screen = t.stable();
    let (ticket_at, session_at) = (t.find("#1 Basket").unwrap(), t.find("FAKE LEADER").unwrap());
    assert!(session_at.1 > ticket_at.1 + 40, "side by side:\n{screen}");
    assert!(screen.contains("Working") && screen.contains("Coding squad") && !screen.contains("its session is running"), "{screen}");
    t.send(b"hello squad\r");
    t.wait_for("GOT:hello squad");
    assert_eq!(lab.rec(1, "typed"), "hello squad\n");

    // ctrl+space: back to the project's screen, on the ticket.
    t.send(&[0]);
    t.wait_for_all(&["╭ Sessions", "╭ Tickets", "#1"]);
    t.wait_gone("ctrl+space back to the project");
    // The leader is in neither the project's list nor the Sessions tab (it registered itself like any session).
    let screen = t.stable();
    assert!(!screen.contains("ticket-1 ") && !screen.contains("shop-worktrees"), "not in the project's sessions:\n{screen}");
    t.send(b"1");
    t.wait_gone("╭ Tickets");
    t.wait_for("work");
    let screen = t.stable();
    assert!(!screen.contains("ticket-1"), "not on the Sessions tab:\n{screen}");
    // Enter on the ticket again: straight back into its running session (no second one started).
    t.send(b"2");
    t.wait_for_flat("WB Web");
    t.send(b"\r");
    t.wait_for("╭ Tickets");
    t.send(b"\t");
    t.wait_for_flat("n new ticket");
    t.send(b"\r");
    t.wait_for_all(&["FAKE LEADER sid:", "GOT:hello squad", "ctrl+space back to the project"]);
    assert_eq!(lab.started(), 1);
    t.send(&[0]);
    t.wait_gone("ctrl+space back to the project");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn the_hooks_move_the_ticket_and_p_types_the_go_ahead_into_the_squads_session() {
    let lab = Lab::new("sq-ready");
    lab.ticket(json!({}));
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 40, 130);
    hand_over(&lab, &mut t);
    let sid = lab.ticket_now()["leaders"][0]["session"].as_str().unwrap().to_string();
    let settings: Value = serde_json::from_str(value_after(&lab.args(1), "--settings")).unwrap();
    let stop = settings["hooks"]["Stop"][0]["hooks"][0]["command"].as_str().unwrap().to_string();
    let gate = settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"].as_str().unwrap().to_string();
    let stop_input = |msg: &str, bg: Value| json!({"session_id": sid, "last_assistant_message": msg, "background_tasks": bg, "stop_hook_active": false, "transcript_path": "/x.jsonl", "cwd": "/tmp", "hook_event_name": "Stop"});
    let bash = |cmd: &str| json!({"session_id": sid, "tool_name": "Bash", "tool_input": {"command": cmd}, "hook_event_name": "PreToolUse", "cwd": "/tmp"});

    // The gate, run as the settings give it: a push is refused until the user says so; the rest runs.
    assert_eq!(lab.run_hook_command(&gate, &bash("git status")), 0);
    assert_eq!(lab.run_hook_command(&gate, &bash("cd /w && git -C x push origin HEAD")), 2);
    // Stuck on Claude's permission prompt: Waiting on you, said on the ticket beside the session; the next tool that
    // runs (the user answered) makes it Working again. Run as the settings give them.
    let notify = settings["hooks"]["Notification"][0]["hooks"][0]["command"].as_str().unwrap().to_string();
    let tool = settings["hooks"]["PostToolUse"][0]["hooks"][0]["command"].as_str().unwrap().to_string();
    let notice = json!({"session_id": sid, "hook_event_name": "Notification", "notification_type": "permission_prompt", "message": "Claude needs your permission to use Bash", "cwd": "/tmp", "transcript_path": "/x.jsonl"});
    assert_eq!(lab.run_hook_command(&notify, &notice), 0);
    assert_eq!((lab.ticket_now()["state"].as_str(), lab.ticket_now()["reason"].as_str()), (Some("waiting"), Some("Waiting for your OK on a command")));
    t.wait_for_all(&["Waiting on you", "Waiting for your OK on a command"]);
    let idle = json!({"session_id": sid, "hook_event_name": "Notification", "notification_type": "idle_prompt", "message": "Claude is waiting for your input"});
    assert_eq!(lab.run_hook_command(&notify, &idle), 0);
    let ran = json!({"session_id": sid, "hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_input": {"command": "cargo test"}, "tool_response": {}});
    assert_eq!(lab.run_hook_command(&tool, &ran), 0);
    assert_eq!(lab.ticket_now()["state"], "working");
    assert!(lab.ticket_now().get("reason").is_none() && lab.ticket_now().get("permission").is_none());
    t.wait_gone("Waiting for your OK on a command");
    // The leader's turn ends with members still running: Working, and the ticket side shows it live.
    assert_eq!(lab.run_hook_command(&stop, &stop_input("Handed it to the builder.\nSQUAD: done", json!([{"id": "b1"}]))), 0);
    assert_eq!(lab.ticket_now()["state"], "working");
    // Then it wants to push.
    assert_eq!(lab.run_hook_command(&stop, &stop_input("Committed on ticket/1.\nSQUAD: ready to publish - push ticket/1", json!([]))), 0);
    assert_eq!((lab.ticket_now()["state"].as_str(), lab.ticket_now()["reason"].as_str()), (Some("ready"), Some("Wants to publish: push ticket/1")));
    t.wait_for_all(&["Ready to push/publish", "Wants to publish: push ticket/1"]);

    // Back on the project's screen, p on the ticket: the go-ahead, Working, and the words typed into the session.
    t.send(&[0]);
    t.wait_for_flat("p let it publish");
    t.send(b"p");
    until("the go-ahead to reach the squad's session", || lab.rec(1, "typed").contains("Go ahead and publish."));
    assert_eq!(lab.rec(1, "typed"), "Go ahead and publish.\n", "the words, then Enter");
    let tk = lab.ticket_now();
    assert_eq!((tk["state"].as_str(), tk["go_ahead"].as_bool()), (Some("working"), Some(true)), "{tk}");
    assert_eq!(lab.run_hook_command(&gate, &bash("git push -u origin ticket/1")), 0, "now it may");
    // Done: the result is kept beside the body, the go-ahead ends, and the gate asks again.
    assert_eq!(lab.run_hook_command(&stop, &stop_input("Pushed ticket/1.\nSQUAD: done", json!([]))), 0);
    let tk = lab.ticket_now();
    assert_eq!((tk["state"].as_str(), tk.get("go_ahead")), (Some("done"), None), "{tk}");
    assert_eq!(fs::read_to_string(lab.config.join("projects/pweb/tickets/01-result.md")).unwrap(), "Pushed ticket/1.\n");
    assert_eq!(lab.run_hook_command(&gate, &bash("git push")), 2);
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn the_hooks_read_real_shaped_input_and_leave_everything_else_alone() {
    let lab = Lab::new("sq-hooks");
    lab.ticket(json!({"state": "working", "leaders": [{"squad": "coding", "session": "lead-1", "started": 1}]}));
    let stop = |sid: &str, msg: &str, bg: Value| json!({"session_id": sid, "last_assistant_message": msg, "background_tasks": bg, "stop_hook_active": false, "transcript_path": "/x.jsonl", "cwd": "/tmp", "hook_event_name": "Stop"});
    // Not a leader: exit 0, nothing written.
    let before = fs::read(lab.tickets_path()).unwrap();
    assert_eq!(lab.hook("squad-stop", &stop("someone", "SQUAD: done", json!([]))), (0, String::new()));
    assert_eq!(fs::read(lab.tickets_path()).unwrap(), before);
    // No line at all: waiting on the user, with the reason.
    assert_eq!(lab.hook("squad-stop", &stop("lead-1", "I stopped here.", json!([]))).0, 0);
    let tk = lab.ticket_now();
    assert_eq!((tk["state"].as_str(), tk["reason"].as_str()), (Some("waiting"), Some("The squad stopped without saying why.")));
    assert_eq!(lab.hook("squad-stop", &stop("lead-1", "SQUAD: blocked - the tests need a database", json!([]))).0, 0);
    assert_eq!(lab.ticket_now()["reason"], "Stuck: the tests need a database");
    assert_eq!(lab.hook("squad-stop", &stop("lead-1", "SQUAD: working", json!([]))).0, 0);
    assert_eq!(lab.ticket_now()["state"], "working");
    assert!(lab.ticket_now().get("reason").is_none(), "Working says nothing more");
    // Garbage in: still 0.
    let mut c = Command::new(BIN);
    c.args(["hook", "squad-stop"]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    for (k, v) in lab.env() {
        c.env(k, v);
    }
    let mut child = c.spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"{ not json").unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(0));
    // The gate: the refusal is said on stderr with exit 2.
    let bash = |cmd: &str| json!({"session_id": "lead-1", "tool_name": "Bash", "tool_input": {"command": cmd}, "hook_event_name": "PreToolUse"});
    let (code, err) = lab.hook("squad-gate", &bash("sh -c 'gh pr create --fill'"));
    assert_eq!(code, 2);
    assert_eq!(err.trim(), "platoon: other people would see this. Stop and end your turn with `SQUAD: ready to publish - <what>`; the user will tell you when to go on.");
    assert_eq!(lab.hook("squad-gate", &bash("echo \"git push\" && git log")), (0, String::new()));
    let read = json!({"session_id": "lead-1", "tool_name": "Write", "tool_input": {"file_path": "/tmp/x", "content": "git push"}});
    assert_eq!(lab.hook("squad-gate", &read).0, 0, "only shell commands are looked at");
}

#[test]
fn a_ticket_whose_squad_isnt_running_continues_it_with_the_same_flags_and_narrow_shows_one_line_of_the_ticket() {
    let lab = Lab::new("sq-resume");
    let sid = "0f0e0d0c-0b0a-4908-8706-050403020100";
    lab.ticket(json!({"state": "done", "leaders": [{"squad": "coding", "session": sid, "started": 1}], "worktrees": []}));
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 90);
    lab.open_tickets(&mut t);
    // Done is folded away: unfold it, then open the ticket.
    t.send(b"\r");
    t.wait_for("#1");
    t.send(b"\x1b[B");
    t.send(b"\r");
    t.wait_for_all(&["The squad's session isn't running. Enter to continue it.", "enter continue the squad"]);
    assert_eq!(lab.started(), 0, "nothing starts by itself");
    t.send(b"\r");
    t.wait_for_all(&["FAKE LEADER sid:0f0e0d0c", "ctrl+space back to the project"]);
    let args = lab.args(1);
    assert_eq!(value_after(&args, "--resume"), sid);
    assert!(!args.iter().any(|a| a == "--session-id" || a == "-n"), "{args:?}");
    // A leader started before its agents carried the squad's name (no "names" on it) keeps the names it began with.
    assert_eq!(value_after(&args, "--agent"), "leader");
    let agents: Value = serde_json::from_str(value_after(&args, "--agents")).unwrap();
    assert_eq!(agents.as_object().unwrap().keys().map(String::as_str).collect::<Vec<_>>(), ["leader", "planner", "builder", "reviewer", "verifier"]);
    assert_eq!(agents["verifier"]["model"], "haiku");
    assert!(!agents["leader"]["prompt"].as_str().unwrap().contains("coding-"), "its prompt calls them by those names too");
    let settings: Value = serde_json::from_str(value_after(&args, "--settings")).unwrap();
    assert!(settings["hooks"]["Stop"][0]["hooks"][0]["command"].as_str().unwrap().ends_with("hook squad-stop"));
    assert_eq!(args.last().unwrap(), &settings.to_string(), "no first message this time");
    assert_eq!(value_after(&args, "--add-dir"), lab.docs.to_string_lossy());
    assert_eq!(fs::canonicalize(lab.rec(1, "cwd").trim()).unwrap(), fs::canonicalize(&lab.shop).unwrap(), "no worktree recorded: the folder itself");
    // Narrow (90 columns): the session has the width, the ticket one line above it.
    let screen = t.stable();
    let lines: Vec<&str> = screen.lines().collect();
    assert!(lines[1].contains("#1 Basket · Done"), "{screen}");
    assert!(lines[2].contains("FAKE LEADER"), "{screen}");
    t.send(b"typed here\r");
    t.wait_for("GOT:typed here");
    t.send(&[0]);
    t.wait_gone("ctrl+space back to the project");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn cancelling_a_ticket_whose_squad_runs_asks_and_stops_only_that_session() {
    let lab = Lab::new("sq-cancel");
    lab.ticket(json!({}));
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 40, 130);
    hand_over(&lab, &mut t);
    let leader = lab.pid(1);
    // A process of the tests' own that must be left alone.
    let mut bystander = Command::new("sleep").arg("30").spawn().unwrap();
    t.send(&[0]);
    t.wait_for_flat("s state");
    t.send(b"s");
    t.wait_for("Working · continue with the squad");
    // New, Working, Done, Cancelled: it starts on Working.
    t.send(b"\x1b[B\x1b[B");
    t.send(b"\r");
    t.wait_for_all(&["Stop the squad's session too?", "leave it running"]);
    assert_eq!(lab.ticket_now()["state"], "cancelled");
    t.send(b"y");
    until("the squad's session to stop", || !alive(leader));
    assert!(alive(bystander.id()), "nothing else is signalled");
    let _ = bystander.kill();
    let _ = bystander.wait();
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn when_the_commander_says_no_the_ticket_stays_new_with_why_and_a_second_a_can_hand_it_anyway() {
    let lab = Lab::new("sq-no");
    lab.ticket(json!({}));
    fs::write(lab.fake.join("verdict"), r#"{"fits":false,"reason":"This is docs work; the Coding squad writes code."}"#).unwrap();
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 40, 130);
    lab.open_tickets(&mut t);
    t.send(b"a");
    t.wait_for_all(&["hand to a squad", "enter hand it over"]);
    t.send(b"\r");
    t.wait_for_flat("The commander says the Coding squad doesn't fit #1: This is docs work");
    let tk = lab.ticket_now();
    assert_eq!((tk["state"].as_str(), tk["handover"]["says"].as_str(), tk["handover"]["kind"].as_str()), (Some("new"), Some("This is docs work; the Coding squad writes code."), Some("refused")), "{tk}");
    assert!(tk.get("leaders").is_none(), "{tk}");
    assert_eq!(lab.started(), 0);
    assert!(!lab.base.join("shop-worktrees").exists(), "nothing made");
    // On the ticket's own screen.
    t.send(b"\r");
    t.wait_for_all(&["Commander", "This is docs work; the Coding squad writes code."]);
    // The same squad again: asked, and yes hands it over without a second check.
    t.send(b"a");
    t.wait_for_all(&["hand to a squad", "enter hand it over"]);
    t.send(b"\r");
    t.wait_for_all(&["The commander said this squad doesn't fit. Hand it anyway?", "y hand it over"]);
    t.send(b"y");
    t.wait_for_all(&["FAKE LEADER sid:", "ctrl+space back to the project"]);
    assert_eq!(lab.checks(), 1, "overruled: not checked again");
    let tk = lab.ticket_now();
    assert_eq!(tk["state"], "working");
    assert!(tk.get("handover").is_none(), "{tk}");
    assert_eq!(lab.started(), 1);
    t.send(&[0]);
    t.wait_gone("ctrl+space back to the project");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn when_the_session_limit_is_reached_nothing_is_handed_over() {
    let lab = Lab::new("sq-limit");
    lab.ticket(json!({}));
    lab.spend_the_day(600_000);
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 40, 130);
    lab.open_tickets(&mut t);
    t.send(b"a");
    t.wait_for_all(&["hand to a squad", "enter hand it over"]);
    t.send(b"\r");
    t.wait_for_flat("Session limit has been reached. #1 wasn't handed over.");
    let tk = lab.ticket_now();
    assert_eq!((tk["state"].as_str(), tk["handover"]["says"].as_str()), (Some("new"), Some("Session limit has been reached")), "{tk}");
    assert_eq!((lab.checks(), lab.started()), (0, 0), "no call, no session");
    assert!(!lab.base.join("shop-worktrees").exists(), "no worktree");
    t.send(b"\r");
    t.wait_for_all(&["Commander", "Session limit has been reached"]);
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn the_worktree_is_made_off_the_ui_thread_while_the_screen_keeps_going() {
    let lab = Lab::new("sq-slowgit");
    lab.ticket(json!({}));
    // A git that takes its time to make a worktree (and is the real one otherwise), first on the PATH.
    let bin = lab.base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let real = String::from_utf8(Command::new("sh").args(["-c", "command -v git"]).output().unwrap().stdout).unwrap().trim().to_string();
    fs::write(bin.join("git"), format!("#!/bin/sh\ncase \"$*\" in *'worktree add'*) touch '{}'; sleep 3;; esac\nexec '{real}' \"$@\"\n", lab.fake.join("git-slow").display())).unwrap();
    fs::set_permissions(bin.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
    let mut env = lab.env();
    env.push(("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default())));
    let mut t = Term::spawn(&[], &env, &lab.work, 40, 130);
    lab.open_tickets(&mut t);
    t.send(b"a");
    t.wait_for_all(&["hand to a squad", "enter hand it over"]);
    t.send(b"\r");
    until("the slow git to start", || lab.fake.join("git-slow").exists());
    // While git sleeps: the ticket says what is happening, and keys are answered at once.
    t.wait_for("Making the worktree…");
    t.send(b"s");
    t.wait_for_flat("#1: Making the worktree… Wait a moment.");
    assert_eq!(lab.ticket_now()["state"], "new", "nothing written until the leader starts");
    assert_eq!(lab.started(), 0);
    t.wait_for_all(&["FAKE LEADER sid:", "ctrl+space back to the project"]);
    assert!(lab.base.join("shop-worktrees/ticket-1/README").exists());
    assert_eq!(lab.ticket_now()["state"], "working");
    t.send(&[0]);
    t.wait_gone("ctrl+space back to the project");
    t.send(b"q");
    t.wait_exit();
}

// ---- squads that need no folder, the work folder, and Docs' worktree when the folder is in git ----

/// Hand ticket #1 to the squad `downs` rows down the picker (Coding, Research, Docs, Summarize, ...) and wait for
/// the split view.
fn hand_to(lab: &Lab, t: &mut Term, squad: &str, downs: usize) {
    lab.open_tickets(t);
    t.send(b"a");
    t.wait_for_all(&["hand to a squad", squad, "enter hand it over"]);
    t.send(&b"\x1b[B".repeat(downs));
    t.send(b"\r");
    t.wait_for_all(&["FAKE LEADER sid:", "ctrl+space back to the project"]);
}

fn add_dirs(args: &[String]) -> Vec<String> {
    args.windows(2).filter(|w| w[0] == "--add-dir").map(|w| w[1].clone()).collect()
}

#[test]
fn a_ticket_without_a_folder_goes_to_research_which_starts_in_the_tickets_work_folder() {
    let lab = Lab::new("sq-research");
    lab.ticket(json!({"folders": []}));
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 40, 130);
    // Coding needs a folder: it says so, and nothing is asked or started.
    lab.open_tickets(&mut t);
    t.send(b"a");
    t.wait_for_all(&["hand to a squad", "Needs a folder, and #1 has none.", "enter hand it over"]);
    t.send(b"\r");
    t.wait_for_flat("The Coding squad needs a folder to work in, and #1 has none. Press f to give it one.");
    assert_eq!((lab.checks(), lab.started()), (0, 0));
    t.send(b"\x1b");
    t.wait_gone("enter hand it over");
    // Research takes it.
    t.send(b"a");
    t.wait_for_all(&["hand to a squad", "Research", "enter hand it over"]);
    t.send(b"\x1b[B\r");
    t.wait_for_all(&["FAKE LEADER sid:", "ctrl+space back to the project"]);
    let work = lab.config.join("projects/pweb/tickets/01-work");
    assert!(work.is_dir(), "the ticket's work folder is made");
    assert_eq!(fs::canonicalize(lab.rec(1, "cwd").trim()).unwrap(), fs::canonicalize(&work).unwrap(), "the leader starts in it");
    let args = lab.args(1);
    assert!(add_dirs(&args).is_empty(), "the ticket has no folder to add: {args:?}");
    assert_eq!(value_after(&args, "--agent"), "research-leader");
    assert_eq!(value_after(&args, "-n"), "Research squad · #1 Basket");
    assert_eq!(value_after(&args, "--model"), "sonnet");
    let agents: Value = serde_json::from_str(value_after(&args, "--agents")).unwrap();
    let got: Vec<(String, String, Value)> = agents.as_object().unwrap().iter().map(|(k, v)| (k.clone(), v["model"].as_str().unwrap().to_string(), v["tools"].clone())).collect();
    let web = json!(["Read", "Grep", "Glob", "WebSearch", "WebFetch"]);
    assert_eq!(
        got,
        [
            ("research-leader".to_string(), "sonnet".to_string(), json!(["Agent", "Read", "Grep", "Glob", "Bash"])),
            ("research-questioner".into(), "sonnet".into(), json!(["Read", "Grep", "Glob"])),
            ("research-researcher".into(), "sonnet".into(), web.clone()),
            ("research-writer".into(), "sonnet".into(), json!(["Read", "Grep", "Glob", "Edit", "Write"])),
            ("research-checker".into(), "opus".into(), web),
        ]
    );
    let prompt = agents["research-leader"]["prompt"].as_str().unwrap();
    assert!(prompt.contains("You and your members work in the ticket's work folder") && !prompt.contains("worktree"), "{prompt}");
    let first = args.last().unwrap();
    assert!(first.contains("the ticket's private work folder; the session starts here") && first.contains("The ticket has no folder of its own."), "{first}");
    assert!(first.contains("stop once the result is written in your work folder"), "{first}");
    // The commander was told there is no folder; nothing else was made.
    assert!(lab.check_file(1, "stdin").starts_with("The squad: Research\n") && lab.check_file(1, "stdin").contains("Where it works: the ticket has no folder."));
    let tk = lab.ticket_now();
    assert_eq!((tk["state"].as_str(), tk["leaders"][0]["squad"].as_str()), (Some("working"), Some("research")), "{tk}");
    assert_eq!(tk["worktrees"], json!([]));
    assert!(!lab.base.join("shop-worktrees").exists());
    // The ticket beside the session says which squad has it and where its work folder is.
    t.wait_for_all(&["Research squad", "Work folder"]);
    t.send(&[0]);
    t.wait_gone("ctrl+space back to the project");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn summarize_reads_the_tickets_folders_from_its_work_folder_and_makes_no_worktree() {
    let lab = Lab::new("sq-summarize");
    lab.ticket(json!({}));
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 40, 130);
    hand_to(&lab, &mut t, "Summarize", 3);
    let work = lab.config.join("projects/pweb/tickets/01-work");
    assert_eq!(fs::canonicalize(lab.rec(1, "cwd").trim()).unwrap(), fs::canonicalize(&work).unwrap());
    let args = lab.args(1);
    assert_eq!(add_dirs(&args), [lab.shop.to_string_lossy().into_owned(), lab.docs.to_string_lossy().into_owned()], "the folders themselves, to read");
    assert!(!lab.base.join("shop-worktrees").exists(), "no worktree, though the shop is a git repository");
    let agents: Value = serde_json::from_str(value_after(&args, "--agents")).unwrap();
    assert_eq!(agents.as_object().unwrap().keys().map(String::as_str).collect::<Vec<_>>(), ["summarize-leader", "summarize-summarizer"]);
    assert_eq!(agents["summarize-summarizer"]["tools"], json!(["Read", "Grep", "Glob", "WebSearch", "WebFetch"]));
    assert!(agents["summarize-leader"]["prompt"].as_str().unwrap().contains("The ticket's own folders are for reading only"));
    let first = args.last().unwrap();
    assert!(first.contains(&format!("For reading only (change nothing in them unless the ticket asks you to):\n- {}\n- {}\n", lab.shop.display(), lab.docs.display())), "{first}");
    t.send(&[0]);
    t.wait_gone("ctrl+space back to the project");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn docs_works_in_a_worktree_of_a_git_folder() {
    let lab = Lab::new("sq-docs-git");
    lab.ticket(json!({"folders": [lab.shop.to_string_lossy()]}));
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 40, 130);
    hand_to(&lab, &mut t, "Docs", 2);
    let wt = lab.base.join("shop-worktrees/ticket-1");
    assert!(wt.join("README").exists(), "a real worktree of the temp repository");
    assert_eq!(fs::canonicalize(lab.rec(1, "cwd").trim()).unwrap(), fs::canonicalize(&wt).unwrap(), "it starts in the worktree");
    let tk = lab.ticket_now();
    assert_eq!((tk["worktrees"][0]["path"].as_str(), tk["worktrees"][0]["branch"].as_str()), (Some(wt.to_string_lossy().as_ref()), Some("ticket/1")), "{tk}");
    let args = lab.args(1);
    assert_eq!(add_dirs(&args), [lab.config.join("projects/pweb/tickets/01-work").to_string_lossy().into_owned()], "only its work folder besides");
    assert_eq!(value_after(&args, "--agent"), "docs-leader");
    let agents: Value = serde_json::from_str(value_after(&args, "--agents")).unwrap();
    assert_eq!(agents["docs-writer"]["tools"], json!(["Read", "Grep", "Glob", "Edit", "Write"]));
    assert_eq!(agents["docs-editor"]["model"], "opus");
    assert!(agents["docs-leader"]["prompt"].as_str().unwrap().contains("the change stays in the worktree, uncommitted"));
    assert!(args.last().unwrap().contains(&format!("- {} (a git worktree of {}, on the branch ticket/1; the session starts here)", wt.display(), lab.shop.display())));
    t.send(&[0]);
    t.wait_gone("ctrl+space back to the project");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn docs_works_in_a_folder_outside_git_as_it_is() {
    let lab = Lab::new("sq-docs-plain");
    lab.ticket(json!({"folders": [lab.docs.to_string_lossy()]}));
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 40, 130);
    hand_to(&lab, &mut t, "Docs", 2);
    assert_eq!(fs::canonicalize(lab.rec(1, "cwd").trim()).unwrap(), fs::canonicalize(&lab.docs).unwrap(), "it starts in the folder itself");
    assert_eq!(lab.ticket_now()["worktrees"], json!([]));
    assert!(!lab.base.join("docs-worktrees").exists() && !lab.base.join("shop-worktrees").exists(), "no worktree anywhere");
    let args = lab.args(1);
    assert_eq!(add_dirs(&args), [lab.config.join("projects/pweb/tickets/01-work").to_string_lossy().into_owned()]);
    let agents: Value = serde_json::from_str(value_after(&args, "--agents")).unwrap();
    let prompt = agents["docs-leader"]["prompt"].as_str().unwrap();
    assert!(prompt.contains("the change stays in the ticket's folder, uncommitted") && !prompt.contains("worktree"), "{prompt}");
    assert!(args.last().unwrap().contains(&format!("- {} (used as it is, not a git worktree; the session starts here)", lab.docs.display())));
    t.send(&[0]);
    t.wait_gone("ctrl+space back to the project");
    t.send(b"q");
    t.wait_exit();
}
