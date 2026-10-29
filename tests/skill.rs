//! The `/to-tasks` skill `platoon setup` writes into Claude Code's folder (`src/skill.rs`), through the real binary
//! in a made-up Claude Code folder: added, updated when it is platoon's copy, a skill of the user's left alone and said,
//! never part of the start-up gate; platoon's copy under the skill's old name, `/to-tickets`, renamed by setup (an old
//! copy the user changed kept and said); `platoon hook offer-tasks`, the hook platoon 1.0 gave its sessions, now a
//! silent no-op; and the hint `platoon statusline` ends its line with (`src/ticketshint.rs`), naming the skill as it
//! is installed.

mod common;

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

const BIN: &str = env!("CARGO_BIN_EXE_platoon");

struct World {
    base: PathBuf,
}

impl World {
    fn new(name: &str) -> Self {
        let base = common::tmp(&format!("skill-{name}"));
        for d in ["claude", "cache", "config"] {
            fs::create_dir_all(base.join(d)).unwrap();
        }
        World { base }
    }

    fn claude(&self) -> PathBuf {
        self.base.join("claude")
    }

    fn skill(&self) -> PathBuf {
        self.claude().join("skills/to-tasks/SKILL.md")
    }

    /// Where platoon 1.3 and before wrote the skill, as `/to-tickets`.
    fn old_skill(&self) -> PathBuf {
        self.claude().join("skills/to-tickets/SKILL.md")
    }

    /// platoon 1.3's own copy at the old place, as its setup wrote it (its text, its trailer): returns the text.
    fn old_platoons_copy(&self) -> String {
        let body = "---\nname: to-tickets\ndescription: \"The user runs it by typing /to-tickets.\"\n---\n\nTurn the work into tasks.\n";
        let text = format!("{body}{TRAILER} cksum {} -->\n", cksum(body));
        fs::create_dir_all(self.old_skill().parent().unwrap()).unwrap();
        fs::write(self.old_skill(), &text).unwrap();
        text
    }

    /// `platoon <args>` on this world's folders, Claude Code installed (a stand-in that can't run), no Codex.
    fn run(&self, args: &[&str], stdin: &str) -> Output {
        let mut c = Command::new(BIN);
        c.args(args)
            .envs(common::NO_REAL_CODEX)
            .env(common::NO_REAL_CLAUDE.0, common::NO_REAL_CLAUDE.1)
            .env("CLAUDE_CONFIG_DIR", self.claude())
            .env("PLATOON_CACHE_DIR", self.base.join("cache"))
            .env("PLATOON_CONFIG_DIR", self.base.join("config"))
            .env("PLATOON_NO_UPDATE_CHECK", "1")
            .env("NO_COLOR", "1")
            .env_remove("PLATOON_SKIP_SETUP_CHECK")
            .env_remove("PLATOON_TASK")
            .env_remove("PLATOON_PROJECT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        child.wait_with_output().unwrap()
    }

    fn setup(&self) -> String {
        let out = self.run(&["setup"], "");
        let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success(), "{said}");
        said
    }
}

/// How the last line of a copy platoon's setup wrote starts.
const TRAILER: &str = "<!-- platoon setup wrote this file and keeps it up to date; once you change it, platoon leaves it alone.";

/// The CRC `cksum` prints for `data`.
fn cksum(data: &str) -> String {
    let mut c = Command::new("cksum").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    c.stdin.take().unwrap().write_all(data.as_bytes()).unwrap();
    let out = c.wait_with_output().unwrap();
    String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap().to_string()
}

/// `text` without its last line, and that line.
fn split_last(text: &str) -> (&str, &str) {
    let at = text[..text.len() - 1].rfind('\n').unwrap() + 1;
    text.split_at(at)
}

#[test]
fn setup_writes_the_skill_naming_itself_and_updates_only_its_own_copy() {
    let w = World::new("setup");
    let said = w.setup();
    assert!(said.contains("[✓] /to-tasks") && said.contains("added") && !said.contains("to-tickets"), "{said}");
    let text = fs::read_to_string(w.skill()).unwrap();
    assert!(text.starts_with("---\nname: to-tasks\n"), "{text}");
    assert!(text.contains(&format!("{BIN} task squads")) && text.contains(&format!("\"Bash({BIN} task add:*)\"")), "{text}");
    // Its last line says platoon wrote it, with the POSIX cksum of the rest (what `scripts/uninstall.sh` checks).
    let (body, last) = split_last(&text);
    assert!(last.starts_with("<!-- platoon setup wrote this file") && last.ends_with(&format!(" cksum {} -->\n", cksum(body))), "{last}");

    let again = w.setup();
    assert!(again.contains("Nothing to change.") && !again.contains("added"), "{again}");
    assert_eq!(fs::read_to_string(w.skill()).unwrap(), text);

    // An older platoon's copy (another text, its own trailer): updated to this one.
    let older = body.replace("Fewer is better", "Few is better");
    fs::write(w.skill(), format!("{older}{TRAILER} cksum {} -->\n", cksum(&older))).unwrap();
    let said = w.setup();
    assert!(said.contains("updated to this platoon's copy") && !said.contains("Nothing to change."), "{said}");
    assert_eq!(fs::read_to_string(w.skill()).unwrap(), text);

    // The user's own (this one: platoon's, edited): left alone, said in one line, and platoon still starts.
    let mine = text.replace("Fewer is better", "Fewer is nicer");
    fs::write(w.skill(), &mine).unwrap();
    let said = w.setup();
    let line = format!(
        "left alone: {} wasn't written by platoon (delete its folder and run `platoon setup` again for platoon's)",
        w.skill().display()
    );
    assert!(said.contains("[–] /to-tasks") && said.contains(&line), "{said}");
    assert_eq!(fs::read_to_string(w.skill()).unwrap(), mine);
    let tasks = w.run(&["task", "squads"], "");
    assert!(tasks.status.success() && String::from_utf8_lossy(&tasks.stdout).contains("bug-fix: "), "{tasks:?}");
}

#[test]
fn setup_renames_platoons_old_to_tickets_copy_and_keeps_one_the_user_changed() {
    // platoon 1.3's copy, unchanged: /to-tasks written, the old copy and its folder gone, said as a rename.
    let w = World::new("rename");
    w.old_platoons_copy();
    let said = w.setup();
    assert!(said.contains("[✓] /to-tasks") && said.contains("renamed /to-tickets to /to-tasks"), "{said}");
    assert!(!said.contains("Nothing to change."), "{said}");
    assert!(fs::read_to_string(w.skill()).unwrap().starts_with("---\nname: to-tasks\n"));
    assert!(!w.old_skill().parent().unwrap().exists(), "the old copy's folder went too");
    let again = w.setup();
    assert!(again.contains("Nothing to change.") && !again.contains("to-tickets"), "{again}");

    // The old copy changed by the user: /to-tasks written beside it, the old one kept and said in one line.
    let w = World::new("rename-edited");
    let mine = w.old_platoons_copy().replace("Turn the work", "Turn my work");
    fs::write(w.old_skill(), &mine).unwrap();
    let said = w.setup();
    assert!(said.contains("[✓] /to-tasks") && said.contains("added") && !said.contains("renamed"), "{said}");
    let line = format!(
        "left alone: {} was changed since platoon wrote it (or isn't platoon's), so it is yours; delete its folder when you no longer want it",
        w.old_skill().display()
    );
    assert!(said.contains("[–] /to-tickets") && said.contains(&line), "{said}");
    assert_eq!(fs::read_to_string(w.old_skill()).unwrap(), mine);
    assert!(w.skill().is_file());

    // The user's own /to-tasks: not written, and platoon's old copy stays (the only platoon skill there is).
    let w = World::new("rename-theirs");
    let old = w.old_platoons_copy();
    w.own_skill();
    let said = w.setup();
    assert!(said.contains("[–] /to-tasks") && said.contains("wasn't written by platoon"), "{said}");
    assert!(
        said.contains(&format!("kept: {} is platoon's, and stays while /to-tasks isn't platoon's copy", w.old_skill().display())),
        "{said}"
    );
    assert_eq!(fs::read_to_string(w.old_skill()).unwrap(), old);
    assert!(fs::read_to_string(w.skill()).unwrap().contains("description: mine"));
}

#[test]
fn the_skill_is_not_part_of_the_start_up_gate() {
    // A platoon upgraded without running setup again: Claude Code's settings done by the older setup, no skill.
    let w = World::new("gate");
    w.setup();
    fs::remove_dir_all(w.claude().join("skills")).unwrap();
    let out = w.run(&["open", "--project", "nope"], "");
    let err = String::from_utf8_lossy(&out.stderr);
    // Past the gate, `open` stops at the first thing it can't do: with no terminal to size (as in CI) that is the
    // terminal, else the project. Either way it got past the gate.
    let past = err.contains("no project called \"nope\"") || err.contains("platoon open needs an interactive terminal.");
    assert!(!err.contains("one-time setup") && past, "past the gate: {err}");
}

/// The platoon window on this world's folders, past the start-up gate (no `PLATOON_SKIP_SETUP_CHECK`).
fn window(w: &World) -> common::Term {
    let env = [
        ("CLAUDE_CONFIG_DIR", w.claude().to_string_lossy().into_owned()),
        ("PLATOON_CACHE_DIR", w.base.join("cache").to_string_lossy().into_owned()),
        ("PLATOON_CONFIG_DIR", w.base.join("config").to_string_lossy().into_owned()),
    ];
    common::Term::spawn(&[], &env, &w.base, 24, 100)
}

const NOTICE: &str = "New: /to-tasks in Claude Code sessions — run `platoon setup` to add it";

#[test]
fn platoon_says_once_that_setup_has_the_skill_to_add_and_nothing_once_it_is_in() {
    // Updated without running setup again: the skill is missing. Said above the footer, once.
    let w = World::new("notice");
    w.setup();
    fs::remove_dir_all(w.claude().join("skills")).unwrap();
    let mut t = window(&w);
    t.wait_for_all(&[NOTICE, "? help"]);
    t.send(b"j");
    t.wait_gone("New: /to-tasks");
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);
    let mut t = window(&w);
    t.wait_for("? help");
    assert!(!t.stable().contains("/to-tasks"), "said once only");
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);

    // With the skill in: nothing.
    let w = World::new("notice-done");
    w.setup();
    let mut t = window(&w);
    t.wait_for("? help");
    assert!(!t.stable().contains("/to-tasks"));
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);

    // platoon 1.3's /to-tickets there, setup not run since: said as a rename.
    let w = World::new("notice-renamed");
    w.setup();
    fs::remove_dir_all(w.claude().join("skills")).unwrap();
    w.old_platoons_copy();
    let mut t = window(&w);
    t.wait_for_all(&["/to-tickets is now called /to-tasks — run `platoon setup` to rename it", "? help"]);
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);
}

#[test]
fn the_offer_hook_older_sessions_still_run_is_a_silent_no_op() {
    // A session platoon 1.0 started has `--settings` with this hook on its first edit: after an upgrade it must not
    // fail, refuse or say anything, wherever it runs and whatever it is given.
    let w = World::new("offer");
    let input = format!(
        r#"{{"session_id":"s-1","cwd":"{}","hook_event_name":"PreToolUse","tool_name":"Edit","tool_input":{{"file_path":"a.txt"}}}}"#,
        w.base.display()
    );
    for (args, stdin) in [
        (&["hook", "offer-tasks"][..], input.as_str()),
        (&["hook", "offer-tasks"], ""),
        (&["hook", "offer-tasks", "--agent", "codex"], "not json"),
    ] {
        let mut c = Command::new(BIN);
        c.args(args)
            .env("CLAUDE_CONFIG_DIR", w.claude())
            .env("PLATOON_CACHE_DIR", w.base.join("cache"))
            .env("PLATOON_CONFIG_DIR", w.base.join("config"))
            .env("PLATOON_OFFER_TASKS", "1")
            .envs(common::NO_REAL_CODEX)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!((out.status.code(), out.stdout.as_slice(), out.stderr.as_slice()), (Some(0), &b""[..], &b""[..]), "{args:?}");
    }
    assert_eq!(fs::read_dir(w.base.join("cache")).unwrap().count(), 0, "nothing marked");
    assert!(!Path::new(&w.base.join("cache/offer-asked")).exists());
}

// ---- the status line's hint ---------------------------------------------------------------------------------------

/// Seconds since the epoch, now.
fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

impl World {
    /// `platoon statusline` as Claude Code runs it for `session`, with its transcript at `transcript`, the task
    /// `task` in its environment (or none) and `columns` columns (or none said).
    fn status_line(&self, session: &str, transcript: &Path, task: Option<&str>, columns: Option<&str>) -> String {
        let input = serde_json::json!({"session_id": session, "transcript_path": transcript,
            "model": {"id": "claude-opus-5-5", "display_name": "Opus"},
            "rate_limits": {"five_hour": {"used_percentage": 12}, "seven_day": {"used_percentage": 40}}})
        .to_string();
        let mut c = Command::new(BIN);
        c.arg("statusline")
            .envs(common::NO_REAL_CODEX)
            .env(common::NO_REAL_CLAUDE.0, common::NO_REAL_CLAUDE.1)
            .env("CLAUDE_CONFIG_DIR", self.claude())
            .env("PLATOON_CACHE_DIR", self.base.join("cache"))
            .env("PLATOON_CONFIG_DIR", self.base.join("config"))
            .env("PLATOON_NO_UPDATE_CHECK", "1")
            .env_remove("PLATOON_TASK")
            .env_remove("PLATOON_PROJECT")
            .env_remove("COLUMNS")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(t) = task {
            c.env("PLATOON_TASK", t);
        }
        if let Some(n) = columns {
            c.env("COLUMNS", n);
        }
        let mut child = c.spawn().unwrap();
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success() && out.stderr.is_empty(), "{out:?}");
        String::from_utf8(out.stdout).unwrap()
    }

    /// What a notes update said about `session`, `ago` seconds ago.
    fn mark(&self, session: &str, qualifies: bool, ago: u64) {
        let dir = self.base.join("cache/tickets-hint");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(session), format!("{} {}", if qualifies { "yes" } else { "no" }, now() - ago)).unwrap();
    }

    /// A transcript of made-up content in Claude Code's record shapes: `lines`, each written `ago` seconds ago.
    fn transcript(&self, session: &str, lines: &[(u64, serde_json::Value)]) -> PathBuf {
        let p = self.claude().join(format!("projects/-shop/{session}.jsonl"));
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        let text: String = lines
            .iter()
            .map(|(ago, v)| {
                let mut v = v.clone();
                v.as_object_mut().unwrap().insert("timestamp".into(), common::iso((now() - ago) * 1000).into());
                v.as_object_mut().unwrap().insert("sessionId".into(), session.into());
                format!("{v}\n")
            })
            .collect();
        fs::write(&p, text).unwrap();
        p
    }

    /// The user's own `/to-tasks` skill (as present as platoon's copy).
    fn own_skill(&self) {
        fs::create_dir_all(self.skill().parent().unwrap()).unwrap();
        fs::write(self.skill(), "---\nname: to-tasks\ndescription: mine\n---\n\nMake tasks.\n").unwrap();
    }
}

fn said(text: &str) -> serde_json::Value {
    serde_json::json!({"type": "user", "isSidechain": false, "cwd": "/home/u/shop", "message": {"role": "user", "content": text}})
}

fn plan(id: &str) -> serde_json::Value {
    serde_json::json!({"type": "assistant", "isSidechain": false, "cwd": "/home/u/shop", "message": {"role": "assistant", "model": "claude-opus-5-5",
        "content": [{"type": "tool_use", "id": id, "name": "ExitPlanMode", "input": {"plan": "## Plan\n\n1. Add the text part.\n2. Send both.\n"}}]}})
}

fn plan_answer(id: &str, approved: bool) -> serde_json::Value {
    let mut block = serde_json::json!({"tool_use_id": id, "type": "tool_result", "content": if approved {
        "User has approved your plan. You can now start coding."
    } else {
        "The user doesn't want to proceed with this tool use. The tool use was rejected."
    }});
    if !approved {
        block.as_object_mut().unwrap().insert("is_error".into(), true.into());
    }
    serde_json::json!({"type": "user", "isSidechain": false, "cwd": "/home/u/shop", "message": {"role": "user", "content": [block]}})
}

fn typed(name: &str) -> serde_json::Value {
    said(&format!("<command-message>{name}</command-message>\n<command-name>/{name}</command-name>"))
}

const FIGURES: &str = "5h 12% · 7d 40%";

#[test]
fn the_status_line_hints_at_to_tasks_while_a_conversation_has_agreed_on_work() {
    let w = World::new("hint");
    let plain = format!("{FIGURES}\n");
    let hinted = format!("{FIGURES} · /to-tasks turns this into tasks\n");
    let talk = w.transcript("s-1", &[(600, said("Plan the plain-text mails with me"))]);

    // Nothing says so: the figures alone.
    w.own_skill();
    assert_eq!(w.status_line("s-1", &talk, None, None), plain);
    // The notes said yes a minute ago: the hint, giving way when the line is narrow.
    w.mark("s-1", true, 60);
    assert_eq!(w.status_line("s-1", &talk, None, None), hinted);
    assert_eq!(w.status_line("s-1", &talk, None, Some("40")), format!("{FIGURES} · /to-tasks\n"));
    assert_eq!(w.status_line("s-1", &talk, None, Some("22")), plain);
    // Only platoon 1.3's /to-tickets there (setup not run since): the hint names the command that works.
    fs::remove_dir_all(w.claude().join("skills")).unwrap();
    w.old_platoons_copy();
    assert_eq!(w.status_line("s-1", &talk, None, None), format!("{FIGURES} · /to-tickets turns this into tasks\n"));
    w.own_skill();
    assert_eq!(w.status_line("s-1", &talk, None, None), hinted, "both there: the new name");
    // Without the skill there is nothing to run: no hint.
    fs::remove_dir_all(w.claude().join("skills")).unwrap();
    assert_eq!(w.status_line("s-1", &talk, None, None), plain);
    w.own_skill();
    // A later notes pass said no; then a yes 31 minutes old.
    w.mark("s-1", false, 10);
    assert_eq!(w.status_line("s-1", &talk, None, None), plain);
    w.mark("s-1", true, 31 * 60);
    assert_eq!(w.status_line("s-1", &talk, None, None), plain, "too old");
    // The skill used since the yes: gone. Typed, or run by Claude with the Skill tool; by its name or its old one.
    w.mark("s-1", true, 120);
    for name in ["to-tasks", "to-tickets"] {
        let used = w.transcript("s-1", &[(600, said("Plan the plain-text mails with me")), (60, typed(name))]);
        assert_eq!(w.status_line("s-1", &used, None, None), plain, "{name}");
        let skill_tool = serde_json::json!({"type": "assistant", "isSidechain": false, "message": {"role": "assistant",
            "content": [{"type": "tool_use", "id": "toolu_9", "name": "Skill", "input": {"skill": name}}]}});
        let used = w.transcript("s-1", &[(600, said("Plan the plain-text mails with me")), (60, skill_tool)]);
        assert_eq!(w.status_line("s-1", &used, None, None), plain, "{name}");
    }
}

#[test]
fn an_approved_plan_brings_the_hint_and_a_rejected_one_does_not() {
    let w = World::new("hint-plan");
    w.own_skill();
    let hinted = format!("{FIGURES} · /to-tasks turns this into tasks\n");
    let approved = w.transcript("s-2", &[(300, said("Plan it")), (200, plan("toolu_1")), (100, plan_answer("toolu_1", true))]);
    assert_eq!(w.status_line("s-2", &approved, None, None), hinted);
    let rejected = w.transcript("s-2", &[(300, said("Plan it")), (200, plan("toolu_1")), (100, plan_answer("toolu_1", false))]);
    assert_eq!(w.status_line("s-2", &rejected, None, None), format!("{FIGURES}\n"));
    let waiting = w.transcript("s-2", &[(300, said("Plan it")), (200, plan("toolu_1"))]);
    assert_eq!(w.status_line("s-2", &waiting, None, None), format!("{FIGURES}\n"), "not answered yet");
    // A notes pass after the plan said no: gone; one before it doesn't count. (One file per session: written again.)
    let approved = w.transcript("s-2", &[(300, said("Plan it")), (200, plan("toolu_1")), (100, plan_answer("toolu_1", true))]);
    w.mark("s-2", false, 50);
    assert_eq!(w.status_line("s-2", &approved, None, None), format!("{FIGURES}\n"));
    w.mark("s-2", false, 150);
    assert_eq!(w.status_line("s-2", &approved, None, None), hinted);
    // /to-tasks typed after the plan: gone.
    let used = w.transcript("s-2", &[(200, plan("toolu_1")), (100, plan_answer("toolu_1", true)), (30, typed("to-tasks"))]);
    assert_eq!(w.status_line("s-2", &used, None, None), format!("{FIGURES}\n"));
}

#[test]
fn a_tasks_session_never_shows_the_hint() {
    let w = World::new("hint-task");
    w.own_skill();
    // A task in no project whose session is s-task, at work: its transcript full of approved plans, and marked yes.
    let task = serde_json::json!({"next": 2, "tasks": [{"number": 1, "title": "Text mails", "file": "01-text-mails.md", "state": "working",
        "folder": w.base.join("shop"), "squad": "feature", "session": "s-task", "started": 1, "created": 1, "updated": 1}]});
    fs::write(w.base.join("config/tasks.json"), task.to_string()).unwrap();
    let mut lines = Vec::new();
    for n in 0..5u64 {
        let id = format!("toolu_{n}");
        lines.push((500 - n * 100, plan(&id)));
        lines.push((450 - n * 100, plan_answer(&id, true)));
    }
    let path = w.transcript("s-task", &lines);
    w.mark("s-task", true, 30);
    // In its environment (PLATOON_TASK), whichever line it shows: the task's own, or the figures for another session.
    for (session, task) in [("s-task", Some("1:")), ("s-task", Some("9:nowhere")), ("s-other", Some("1:"))] {
        if session == "s-other" {
            w.mark("s-other", true, 30);
        }
        let line = w.status_line(session, &path, task, None);
        assert!(!line.contains("to-tasks"), "{session} {task:?}: {line}");
    }
    // And without it, the session a task records is still a task's.
    let line = w.status_line("s-task", &path, None, None);
    assert_eq!(line, format!("{FIGURES}\n"));
    // The same transcript in a session that is no task's does get it (the test above is not vacuous).
    let line = w.status_line("s-free", &w.transcript("s-free", &lines), None, None);
    assert!(line.contains("/to-tasks turns this into tasks"), "{line}");
}
