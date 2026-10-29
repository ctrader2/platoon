//! Tasks end to end, in a real terminal with the real binary: `t` on a session's row drafts tasks with a stand-in
//! `claude -p` (each task with the squad suggested for it), `enter` makes them Not started; on the Tasks tab, `s` opens
//! the list of squads with the suggested one picked, and `enter` starts the task: a worktree on `task/1`, `squad.md`
//! and every squad beside it in the task's folder, and its session in a platoon host with the squad's model, plan mode,
//! the hooks and the `ask` rules, `squad.md` added to its system prompt and the task's text as its first message. The
//! session is reached through its task, not on the Sessions tab; its hooks (`platoon hook task-...`, run here as Claude
//! Code would) move the task between the sections; `c` cancels after asking and stops the session. A task finished long
//! ago is on the Tasks tab's Archived list, and `a` brings it back to Active.
//!
//! The "claude" here is one small script wearing two hats: asked with `-p` it answers like `claude -p` (from
//! `$FAKE_DIR/reply`); started without it, it writes its arguments down, registers itself the way Claude does, prints
//! a line and gives a shell. A real `claude` is never run, every directory is a temp dir, and the only processes
//! signaled are the ones these tests started.

mod common;
use common::*;
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const FAKE: &str = r#"#!/bin/sh
if [ "$1" = "-p" ]; then
  mkdir -p "$FAKE_DIR/calls"
  k=$(( $(ls "$FAKE_DIR/calls" | grep -c '\.stdin$') + 1 ))
  c="$FAKE_DIR/calls/$k"
  for a in "$@"; do printf '%s\n' "$a"; done > "$c.argv"
  pwd > "$c.cwd"
  cat > "$c.stdin"
  text=$(cat "$FAKE_DIR/reply")
  esc=$(printf '%s' "$text" | tr '\n' ' ' | sed 's/\\/\\\\/g; s/"/\\"/g')
  printf '{"type":"result","subtype":"success","is_error":false,"result":"%s","total_cost_usd":0.001,"usage":{"input_tokens":400,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":50}}' "$esc"
  exit 0
fi
for a in "$@"; do printf '%s\0' "$a"; done > "$FAKE_DIR/session.argv"
sid=""
prev=""
for a in "$@"; do
  if [ "$prev" = "--session-id" ] || [ "$prev" = "--resume" ]; then sid="$a"; fi
  prev="$a"
done
printf '{"pid":%s,"sessionId":"%s","cwd":"%s","kind":"interactive","startedAt":%s,"status":"idle"}' $$ "$sid" "$(pwd)" "$(date +%s)000" > "$CLAUDE_CONFIG_DIR/sessions/$$.json"
echo "FAKE SESSION sid:$sid offer:${PLATOON_OFFER_TASKS:-none} in:$(pwd)"
exec sh
"#;

const DRAFT: &str = r#"{"tasks":[
  {"title":"Order emails in plain text too","does":"Every order email also has a plain-text part with the same items.","done_when":["A text-only mail client gets the whole order"],"squad":"feature"},
  {"title":"Document the plain-text emails","does":"The guide says order emails come in two parts.","done_when":["The guide's email page says so"],"squad":"docs"}
]}"#;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(ok.success(), "git {args:?}");
}

struct Lab {
    base: PathBuf,
    claude: PathBuf,
    cache: PathBuf,
    config: PathBuf,
    shop: PathBuf,
    fake: PathBuf,
}

impl Lab {
    fn new(name: &str) -> Self {
        let base = tmp(name);
        let lab = Self {
            claude: base.join("claude"),
            cache: base.join("cache"),
            config: base.join("config"),
            shop: base.join("shop"),
            fake: base.join("fake"),
            base,
        };
        for d in [&lab.claude.join("sessions"), &lab.claude.join("projects/-shop"), &lab.cache, &lab.config, &lab.shop, &lab.fake] {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(lab.shop.join("README"), "a shop\n").unwrap();
        git(&lab.shop, &["init", "-q"]);
        git(&lab.shop, &["add", "."]);
        git(&lab.shop, &["commit", "-q", "-m", "first"]);
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
        let line = json!({"type":"user","cwd":lab.shop,"gitBranch":"main","timestamp":"2026-09-24T10:00:00.000Z","message":{"role":"user","content":"The order email is HTML only. A plain-text part can wait for later."}}).to_string();
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

    fn task(&self, n: usize) -> Value {
        self.tasks()["tasks"][n - 1].clone()
    }

    /// What the stand-in `claude` was started with as a session.
    fn session_argv(&self) -> Vec<String> {
        let raw = fs::read(self.fake.join("session.argv")).unwrap_or_default();
        raw.split(|b| *b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect()
    }

    /// `platoon hook <name>` with `input` on stdin, as Claude Code runs a task session's hook: what it prints.
    fn hook(&self, name: &str, input: Value) -> String {
        let mut child = Command::new(BIN)
            .args(["hook", name])
            .envs(self.env())
            .envs(common::NO_REAL_CODEX)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{name}: {out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// `platoon task step <args>` as task #1's session runs it (the task in its environment): what it printed.
    fn step(&self, args: &[&str]) -> String {
        let out = Command::new(BIN)
            .args(["task", "step"])
            .args(args)
            .envs(self.env())
            .envs(common::NO_REAL_CODEX)
            .env("PLATOON_TASK", "1:pweb")
            .env_remove("PLATOON_PROJECT")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success(), "task step {args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
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
fn drafted_tasks_start_as_one_session_with_their_squad_and_their_hooks_move_them_along() {
    let lab = Lab::new("tasks-flow");
    let mut t = Term::spawn(&[], &lab.env(), &lab.shop, 40, 150);
    t.wait_for_all(&["The order email is HTML only", "t draft tasks"]);

    // t: the draft, each task with the squad that suits it.
    t.send(b"t");
    t.wait_for_all(&[
        "Tasks from this conversation",
        "Order emails in plain text too",
        "Document the plain-text emails",
        "enter make them",
    ]);
    t.wait_for_flat("Squad Feature (suggested)");
    t.wait_for_flat("Squad Docs (suggested)");
    let sent = fs::read_to_string(lab.fake.join("calls/1.stdin")).unwrap();
    assert!(sent.contains("- feature: ") && sent.contains("- docs: ") && !sent.contains("waits_for"), "{sent}");
    // enter: both made, Not started, on the Tasks tab.
    t.send(b"\r");
    t.wait_for_all(&["Not started · 2", "Suggested: Feature · s starts it", "Suggested: Docs · s starts it"]);
    t.wait_for_flat("Made 2 tasks in Web: #1 and #2. s on each starts it.");
    let one = lab.task(1);
    assert_eq!((one["state"].as_str(), one["squad"].as_str()), (Some("not_started"), Some("feature")), "{one}");
    assert_eq!(one["folder"], json!(lab.shop.to_string_lossy()));
    assert!(one.get("session").is_none());

    // s on #1: the list of squads, its suggestion picked; enter starts it.
    t.wait_for_flat("▎ WB #1 Order emails in plain text too");
    t.send(b"s");
    t.wait_for_all(&[
        "Start #1 with a squad",
        "Feature",
        "Bug fix",
        "Refactor",
        "Explain",
        "Review",
        "Docs",
        "plans first, then waits for your yes",
        "enter start",
    ]);
    let shown = t.stable();
    assert!(shown.lines().any(|l| l.contains("▎ Feature")), "the suggested squad is picked: {shown}");
    t.send(b"\r");
    let wt = lab.base.join("shop-worktrees/task-1");
    t.wait_for_unwrapped(&format!("in:{}", wt.to_string_lossy()));
    let one = lab.task(1);
    let session = one["session"].as_str().unwrap().to_string();
    t.wait_for(&format!("FAKE SESSION sid:{session} offer:none"));
    assert_eq!((one["state"].as_str(), one["squad"].as_str()), (Some("working"), Some("feature")), "{one}");
    assert_eq!(one["worktree"]["branch"], "task/1");
    assert_eq!(one["worktree"]["path"], json!(wt.to_string_lossy()));
    assert!(wt.join("README").is_file(), "the worktree has the shop's files");
    // What the session was started with.
    let argv = lab.session_argv();
    let at = |flag: &str| argv.iter().position(|a| a == flag).map(|i| argv[i + 1].clone());
    let task_dir = lab.config.join("projects/pweb/tasks/01-work");
    assert_eq!(at("--add-dir"), Some(task_dir.to_string_lossy().into_owned()));
    assert_eq!(at("--session-id").as_deref(), Some(session.as_str()));
    assert_eq!(at("--model").as_deref(), Some("opus"));
    assert_eq!(at("--permission-mode").as_deref(), Some("plan"), "Feature plans first");
    assert_eq!(at("--append-system-prompt-file"), Some(task_dir.join("squad.md").to_string_lossy().into_owned()));
    assert!(argv.last().unwrap().starts_with("Task #1: Order emails in plain text too\n\n## What it should do"), "{argv:?}");
    let settings: Value = serde_json::from_str(&at("--settings").unwrap()).unwrap();
    assert!(settings["permissions"]["ask"].as_array().unwrap().contains(&json!("Bash(git push:*)")), "{settings}");
    assert!(settings["hooks"]["Stop"][0]["hooks"][0]["command"].as_str().unwrap().ends_with("hook task-stop"), "{settings}");
    assert_eq!(settings["hooks"]["Stop"][0]["hooks"][0]["statusMessage"], "Fable is checking the work against \"Done when\"…");
    assert_eq!(settings["statusLine"]["command"], json!(format!("{BIN} statusline")));
    assert_eq!(settings["statusLine"]["refreshInterval"], 2);
    assert_eq!(settings["permissions"]["allow"], json!([format!("Bash({BIN} task step:*)")]));
    assert!(!argv.iter().any(|a| a == "--agents" || a == "--agent"));
    // The task's own folder: squad.md (platoon's rules, then the squad's steps) and every squad beside it.
    let squad_md = fs::read_to_string(task_dir.join("squad.md")).unwrap();
    assert!(squad_md.contains(&format!("You work in {} on the branch `task/1`", wt.to_string_lossy())), "{squad_md}");
    assert!(squad_md.contains("# The squad: Feature"), "{squad_md}");
    for id in ["feature", "bug-fix", "incident", "refactor", "explain", "review", "docs"] {
        assert!(task_dir.join("squads").join(format!("{id}.md")).is_file(), "{id}");
    }

    // Back to the list: the session is reached through its task, not listed on the Sessions tab.
    t.send(&[0]);
    t.wait_for("Working · 1");
    t.send(b"1");
    t.wait_for_all(&["The order email is HTML only", "Active  1"]);
    let shown = t.stable();
    assert!(!shown.contains("task-1") && !shown.contains("#1 "), "{shown}");
    t.send(b"3");
    t.wait_for("Working · 1");

    // Its hooks, as Claude Code runs them: a plan to approve, then approved; a question, then answered; the turn ends.
    let ev = |name: &str, extra: Value| {
        let mut v = json!({"session_id": session, "hook_event_name": name, "cwd": wt, "permission_mode": "plan"});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    };
    lab.hook(
        "task-plan",
        ev("PreToolUse", json!({"tool_name": "ExitPlanMode", "tool_input": {"plan": "## Plan\n\n1. Add the text part.\n"}})),
    );
    t.wait_for_all(&["Needs you · 1", "a plan to approve", "The plan", "1. Add the text part."]);
    assert_eq!(fs::read_to_string(task_dir.join("plan.md")).unwrap(), "## Plan\n\n1. Add the text part.\n");
    lab.hook("task-tool", ev("PostToolUse", json!({"tool_name": "ExitPlanMode"})));
    t.wait_for("Working · 1");
    lab.hook(
        "task-question",
        ev("PreToolUse", json!({"tool_name": "AskUserQuestion", "tool_input": {"questions": [{"question": "Which mail client first?"}]}})),
    );
    t.wait_for_all(&["Needs you · 1", "a question", "Feature · asks: Which mail client first?"]);
    lab.hook("task-tool", ev("PostToolUse", json!({"tool_name": "AskUserQuestion"})));
    t.wait_for("Working · 1");
    // The session marks its steps as it goes: the board follows.
    assert_eq!(lab.step(&["1", "done", "read the mail code"]), "Step 1 done.\n");
    lab.step(&["2", "started"]);
    t.wait_for_all(&["Feature · step 2/9 · Name the data shape first", "Steps · 1 of 9 marked", "▸ 2 Name the data shape first"]);
    // The turn ends with steps not marked: sent back once to mark them, before any check.
    let calls = || fs::read_dir(lab.fake.join("calls")).map(|d| d.count()).unwrap_or(0);
    let before = calls();
    let said: Value = serde_json::from_str(&lab.hook("task-stop", ev("Stop", json!({"permission_mode": "acceptEdits"})))).unwrap();
    assert_eq!(said["decision"], "block");
    assert!(said["reason"].as_str().unwrap().contains("Steps 2 (Name the data shape first), 3 (Write the plan)"), "{said}");
    assert_eq!(calls(), before, "no check yet");
    t.wait_for_all(&["Sent back to mark steps 2, 3, 4, 5, 6, 7, 8 and 9 done or", "Steps · 8 steps not marked"]);
    for n in 2..=9 {
        lab.step(&[&n.to_string(), "done"]);
    }
    t.wait_for_all(&["Feature · all 9 steps marked", "Steps · 9 of 9 marked"]);
    // The turn ends again, and the check of the work says done: Done.
    fs::write(lab.fake.join("reply"), r#"{"done":true,"boxes":[],"findings":[]}"#).unwrap();
    let said = lab.hook("task-stop", ev("Stop", json!({"permission_mode": "acceptEdits"})));
    assert!(said.trim().is_empty(), "nothing refused: {said}");
    t.wait_for_all(&["Done · 1", "Not started · 1"]);
    assert_eq!(lab.task(1)["state"], "done");
    assert_eq!(lab.task(1)["verdict"]["done"], true);
    // Typed into: Working again.
    lab.hook("task-prompt", ev("UserPromptSubmit", json!({"prompt": "one more thing"})));
    t.wait_for("Working · 1");

    // c: asked first; yes cancels it and stops its session (the conversation is kept).
    t.send(b"c");
    t.wait_for_all(&["Cancel task #1?", "which is running, will be stopped"]);
    t.send(b"y");
    t.wait_for_flat("#1 is cancelled. Stopping its session");
    assert_eq!(lab.task(1)["state"], "cancelled");
    t.send(&[3]);
    t.wait_exit();
}

#[test]
fn a_task_with_no_folder_asks_for_one_before_its_squad() {
    let lab = Lab::new("tasks-folder");
    // A task in the project, with no folder (no session of the project's worked anywhere yet).
    let v = json!({"next": 2, "tasks": [{"number": 1, "title": "Write the docs", "file": "01-write-the-docs.md", "state": "not_started", "created": 1, "updated": 1}]});
    fs::write(lab.config.join("projects/pweb/tasks.json"), v.to_string()).unwrap();
    let mut t = Term::spawn(&[], &lab.env(), &lab.shop, 40, 150);
    t.wait_for("The order email is HTML only");
    t.send(b"3");
    t.wait_for_all(&["Not started · 1", "No folder yet · s starts it"]);
    t.send(b"s");
    t.wait_for_all(&["The folder task #1 works in", "enter pick a squad"]);
    t.send(lab.shop.to_string_lossy().as_bytes());
    t.wait_for_unwrapped(&format!("❯ {}", lab.shop.to_string_lossy()));
    t.send(b"\r");
    t.wait_for_all(&["Start #1 with a squad", "▎ Feature"]);
    assert_eq!(lab.task(1)["folder"], json!(lab.shop.to_string_lossy()));
    // esc: nothing starts.
    t.send(b"\x1b");
    t.wait_gone("Start #1 with a squad");
    assert_eq!(lab.task(1)["state"], "not_started");
    t.send(&[3]);
    t.wait_exit();
}

#[test]
fn the_check_of_the_work_sends_it_back_once_then_needs_you_with_what_it_found() {
    let lab = Lab::new("tasks-verdict");
    // A task at work in the shop, as its session's hooks would have left it.
    let v = json!({"next": 2, "tasks": [{"number": 1, "title": "Text mails", "file": "01-text-mails.md", "state": "working",
        "folder": lab.shop, "squad": "feature", "session": "s-9", "started": 1, "created": 1, "updated": 1}]});
    fs::write(lab.config.join("projects/pweb/tasks.json"), v.to_string()).unwrap();
    fs::create_dir_all(lab.config.join("projects/pweb/tasks")).unwrap();
    fs::write(
        lab.config.join("projects/pweb/tasks/01-text-mails.md"),
        "## What it should do\n\nOrder mails have a text part.\n\n## Done when\n\n- [ ] A text-only client gets the order\n",
    )
    .unwrap();
    fs::write(lab.shop.join("mail.rs"), "fn send() {\n    html_only();\n}\n").unwrap();
    fs::write(
        lab.fake.join("reply"),
        r#"{"done":false,"boxes":[{"text":"A text-only client gets the order","met":false,"evidence":"no text part"}],
            "findings":[{"file":"mail.rs","line":2,"quote":"html_only();","problem":"Only HTML is sent."},
                        {"file":"mail.rs","line":30,"quote":"text_part();","problem":"Not in the code."}]}"#,
    )
    .unwrap();
    let mut t = Term::spawn(&[], &lab.env(), &lab.shop, 40, 150);
    t.wait_for("The order email is HTML only");
    t.send(b"3");
    t.wait_for_all(&["Working · 1", "#1 Text mails"]);
    let stop = json!({"session_id": "s-9", "hook_event_name": "Stop", "cwd": lab.shop, "permission_mode": "acceptEdits",
        "last_assistant_message": "I added the text part."});
    // Not done: the stop is refused once, with what it found, file:line first; the made-up finding is left out.
    let said: Value = serde_json::from_str(&lab.hook("task-stop", stop.clone())).unwrap();
    let reason = said["reason"].as_str().unwrap();
    assert_eq!(said["decision"], "block");
    assert!(reason.contains("- mail.rs:2: Only HTML is sent.") && !reason.contains("Not in the code"), "{reason}");
    t.wait_for_all(&["Working · 1", "Verdict 2 problems · sent back to the session", "mail.rs:2: Only HTML is sent."]);
    // What the check was given and how it ran: read only, in the task's folder, on Feature's verdict model.
    let argv = fs::read_to_string(lab.fake.join("calls/1.argv")).unwrap();
    assert!(argv.contains("--tools\nRead,Grep,Glob\n") && argv.contains("--model\nfable\n") && argv.contains("--json-schema\n"), "{argv}");
    assert_eq!(fs::read_to_string(lab.fake.join("calls/1.cwd")).unwrap().trim(), lab.shop.canonicalize().unwrap().to_string_lossy());
    let sent = fs::read_to_string(lab.fake.join("calls/1.stdin")).unwrap();
    assert!(sent.contains("- [ ] A text-only client gets the order") && sent.contains("I added the text part."), "{sent}");
    // Not done again: the stop goes through, and the task needs you with what the check found.
    assert!(lab.hook("task-stop", stop).trim().is_empty());
    t.wait_for_all(&["Needs you · 1", "the check found problems", "Feature · the check found 2 problems: mail.rs:2"]);
    let shown = t.stable();
    assert!(shown.contains("✗   A text-only client gets the order"), "{shown}");
    assert_eq!(lab.task(1)["state"], "needs_you");
    assert_eq!(lab.task(1)["need"], "check");
    assert!(lab.config.join("projects/pweb/tasks/01-work/verdict.json").is_file());
    t.send(&[3]);
    t.wait_exit();
}

#[test]
fn a_projects_screen_lists_its_sessions_and_tasks_apart_and_enter_shows_a_task_that_never_started() {
    let lab = Lab::new("tasks-project-screen");
    let v = json!({"next": 3, "tasks": [
        {"number": 1, "title": "Write the docs", "file": "01-write-the-docs.md", "state": "not_started", "created": 1, "updated": 1},
        {"number": 2, "title": "Old idea", "file": "02-old-idea.md", "state": "cancelled", "created": 1, "updated": 1},
    ]});
    fs::write(lab.config.join("projects/pweb/tasks.json"), v.to_string()).unwrap();
    let mut t = Term::spawn(&[], &lab.env(), &lab.shop, 40, 150);
    t.wait_for("The order email is HTML only");
    t.send(b"2");
    t.wait_for_flat("WB Web");
    t.send(b"\r");
    t.wait_for_all(&["Sessions and tasks", "▎○ shop", "#1 Write the docs", "Not started"]);
    let shown = t.stable();
    let lines: Vec<&str> = shown.lines().collect();
    let at = |s: &str| lines.iter().position(|l| l.contains(s)).unwrap_or_else(|| panic!("{s}:\n{shown}"));
    assert!(at("│ Sessions ") < at("▎○ shop") && at("▎○ shop") < at("│ Tasks ") && at("│ Tasks ") < at("#1 Write the docs"), "{shown}");
    assert!(!shown.contains("Old idea"), "a cancelled task is not listed: {shown}");
    // Down to the task, enter: its text on its own screen; esc: back on the project's screen.
    t.send(b"j");
    t.wait_for("▎  #1 Write the docs");
    t.send(b"\r");
    t.wait_gone("Sessions and tasks");
    t.wait_for_all(&["#1 Write the docs", "Not started"]);
    t.send(b"\x1b");
    t.wait_for_all(&["Sessions and tasks", "▎  #1 Write the docs"]);
    t.send(&[3]);
    t.wait_exit();
}

#[test]
fn with_claude_code_switched_off_a_started_claude_tasks_hooks_still_move_it_and_its_work_is_checked() {
    let lab = Lab::new("tasks-claude-off");
    // Claude Code switched off in platoon's settings, with Codex installed (its folder is there), so it really is off.
    let mut settings: Value = serde_json::from_str(&fs::read_to_string(lab.config.join("settings.json")).unwrap()).unwrap();
    settings.as_object_mut().unwrap().insert("use_claude_code".into(), json!(false));
    fs::write(lab.config.join("settings.json"), settings.to_string()).unwrap();
    let codex = lab.base.join("codex");
    fs::create_dir_all(&codex).unwrap();
    // A task at work in the shop on the Feature squad (Claude's models), as its session's hooks would have left it.
    let v = json!({"next": 2, "tasks": [{"number": 1, "title": "Text mails", "file": "01-text-mails.md", "state": "working",
        "folder": lab.shop, "squad": "feature", "session": "s-7", "started": 1, "created": 1, "updated": 1}]});
    fs::write(lab.config.join("projects/pweb/tasks.json"), v.to_string()).unwrap();
    fs::create_dir_all(lab.config.join("projects/pweb/tasks")).unwrap();
    fs::write(lab.config.join("projects/pweb/tasks/01-text-mails.md"), "## What it should do\n\nOrder mails have a text part.\n").unwrap();
    let hook = |name: &str, input: Value| {
        let mut child = Command::new(BIN)
            .args(["hook", name])
            .envs(lab.env())
            .envs(common::NO_REAL_CODEX)
            .env("CODEX_HOME", &codex)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{name}: {out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let ev = |name: &str, extra: Value| {
        let mut v = json!({"session_id": "s-7", "hook_event_name": name, "cwd": lab.shop, "permission_mode": "plan"});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    };
    // Its plan: Needs you, a plan to approve, kept beside the task.
    hook("task-plan", ev("PreToolUse", json!({"tool_name": "ExitPlanMode", "tool_input": {"plan": "## Plan\n\n1. Add the text part.\n"}})));
    assert_eq!((lab.task(1)["state"].as_str(), lab.task(1)["need"].as_str()), (Some("needs_you"), Some("plan")));
    let plan = lab.config.join("projects/pweb/tasks/01-work/plan.md");
    assert_eq!(fs::read_to_string(plan).unwrap(), "## Plan\n\n1. Add the text part.\n");
    // Approved: Working again. Then the turn ends and the check of the work, on Claude Code, says done.
    hook("task-tool", ev("PostToolUse", json!({"tool_name": "ExitPlanMode"})));
    assert_eq!(lab.task(1)["state"], "working");
    fs::write(lab.fake.join("reply"), r#"{"done":true,"boxes":[],"findings":[]}"#).unwrap();
    let said = hook("task-stop", ev("Stop", json!({"permission_mode": "acceptEdits", "last_assistant_message": "Added it."})));
    assert!(said.trim().is_empty(), "{said}");
    let argv = fs::read_to_string(lab.fake.join("calls/1.argv")).unwrap();
    assert!(argv.contains("--model\nfable\n"), "checked on Feature's verdict model: {argv}");
    assert_eq!(lab.task(1)["state"], "done");
}

#[test]
fn a_task_finished_long_ago_is_on_archived_and_a_brings_it_back_to_active() {
    let lab = Lab::new("tasks-archived");
    let v = json!({"next": 3, "tasks": [
        {"number": 1, "title": "Old work", "file": "01-old-work.md", "state": "done", "folder": lab.shop, "squad": "feature",
         "session": "s-old", "started": 1, "created": 1, "updated": 1},
        {"number": 2, "title": "Write the docs", "file": "02-write-the-docs.md", "state": "not_started", "created": 1, "updated": 1},
    ]});
    fs::write(lab.config.join("projects/pweb/tasks.json"), v.to_string()).unwrap();
    // The old task's conversation: never on the Sessions tab.
    let line = json!({"type":"user","cwd":lab.shop,"gitBranch":"task/1","timestamp":"2026-09-20T10:00:00.000Z","message":{"role":"user","content":"Task #1: Old work"}}).to_string();
    fs::write(lab.claude.join("projects/-shop/s-old.jsonl"), line + "\n").unwrap();
    let mut t = Term::spawn(&[], &lab.env(), &lab.shop, 40, 150);
    t.wait_for_all(&["The order email is HTML only", "Active  1", "Archived  0"]);
    assert!(!t.stable().contains("Task #1: Old work"));
    t.send(b"3");
    t.wait_for_all(&["Not started · 1", "#2 Write the docs", "Active  1", "Archived  1"]);
    assert!(!t.stable().contains("Old work"), "finished long ago: not on Active");
    // → shows Archived: the old task, with no section heading.
    t.send(b"\x1b[C");
    t.wait_for_all(&["#1 Old work", "a restore", "←→ active/archived"]);
    let shown = t.stable();
    assert!(!shown.contains("Write the docs") && !shown.contains("Done · 1"), "{shown}");
    // a: back to Active, and it stays there (the file says so).
    t.send(b"a");
    t.wait_for_flat("Restored #1 \"Old work\"");
    t.wait_for("Nothing archived.");
    assert_eq!(lab.task(1)["archived"], json!(false));
    t.send(b"\x1b[D");
    t.wait_for_all(&["Done · 1", "#1 Old work", "Not started · 1", "Active  2", "Archived  0"]);
    t.send(&[3]);
    t.wait_exit();
}

#[test]
fn a_tasks_session_marks_its_steps_with_platoon_task_step_and_its_status_line_shows_where_it_is() {
    let lab = Lab::new("tasks-steps");
    // A task at work in the shop, three steps from its squad, none marked.
    let v = json!({"next": 2, "tasks": [{"number": 1, "title": "Text mails", "file": "01-text-mails.md", "state": "working",
        "folder": lab.shop, "squad": "feature", "session": "s-7", "started": 1, "created": 1, "updated": 1,
        "steps": {"list": [{"title": "Look"}, {"title": "Build"}, {"title": "Check"}]}}]});
    fs::write(lab.config.join("projects/pweb/tasks.json"), v.to_string()).unwrap();
    // Run as the session would: the task in its environment (PLATOON_TASK), or none; nothing inherited from here.
    let run = |args: &[&str], task: Option<&str>, stdin: &str| {
        let mut c = Command::new(BIN);
        c.args(args).envs(lab.env()).envs(common::NO_REAL_CODEX).env("PLATOON_NO_UPDATE_CHECK", "1");
        for k in ["PLATOON_TASK", "PLATOON_PROJECT", "COLUMNS"] {
            c.env_remove(k);
        }
        if let Some(t) = task {
            c.env("PLATOON_TASK", t);
        }
        let mut child = c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    assert_eq!(run(&["task", "step", "1", "done", "read the mail code"], Some("1:pweb"), ""), (0, "Step 1 done.\n".into(), String::new()));
    assert_eq!(run(&["task", "step", "2", "started"], Some("1:pweb"), "").0, 0);
    let (code, _, err) = run(&["task", "step", "5", "done"], Some("1:pweb"), "");
    assert_eq!((code, err.as_str()), (1, "platoon task step: there is no step 5: the steps are 1 to 3\n"));
    let (code, _, err) = run(&["task", "step", "3", "skipped"], Some("1:pweb"), "");
    assert!(code == 1 && err.contains("say why step 3 was skipped"), "{err}");
    let (code, _, err) = run(&["task", "step", "3", "done"], None, "");
    assert!(code == 1 && err.contains("works inside a task's session only"), "{err}");
    let steps = &lab.task(1)["steps"]["list"];
    assert_eq!((steps[0]["mark"].as_str(), steps[0]["note"].as_str()), (Some("done"), Some("read the mail code")));
    assert_eq!(steps[1]["mark"], "started");
    assert!(steps[2].get("mark").is_none(), "nothing written by a wrong one: {steps}");
    // The status line: the task's, in its session; the plan usage alone in any other. Both are recorded.
    let payload = |session: &str| {
        json!({"session_id": session, "transcript_path": lab.claude.join(format!("projects/-shop/{session}.jsonl")),
            "model": {"id": "claude-opus-5-5", "display_name": "Opus"},
            "rate_limits": {"five_hour": {"used_percentage": 12}, "seven_day": {"used_percentage": 40}}})
        .to_string()
    };
    assert_eq!(run(&["statusline"], Some("1:pweb"), &payload("s-7")).1, "#1 Feature · step 2/3 Build · Opus · 5h 12% · 7d 40%\n");
    assert_eq!(run(&["statusline"], None, &payload("s-other")).1, "5h 12% · 7d 40%\n");
    assert_eq!(run(&["statusline"], Some("1:pweb"), &payload("s-other")).1, "5h 12% · 7d 40%\n", "another session than the task's");
    for id in ["s-7", "s-other"] {
        let saved: Value = serde_json::from_str(&fs::read_to_string(lab.cache.join(format!("status/{id}.json"))).unwrap()).unwrap();
        assert_eq!(saved["five_hour"]["pct"], 12, "{id}");
    }
}
