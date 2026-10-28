//! The `/to-tickets` skill `platoon setup` writes into Claude Code's folder (`src/skill.rs`), through the real binary
//! in a made-up Claude Code folder: added, updated when it is platoon's copy, a skill of the user's left alone and said,
//! never part of the start-up gate; and `platoon hook offer-tasks`, the hook platoon 1.0 gave its sessions, now a
//! silent no-op.

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
        self.claude().join("skills/to-tickets/SKILL.md")
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
    assert!(said.contains("[✓] /to-tickets") && said.contains("added"), "{said}");
    let text = fs::read_to_string(w.skill()).unwrap();
    assert!(text.starts_with("---\nname: to-tickets\n"), "{text}");
    assert!(text.contains(&format!("{BIN} task squads")) && text.contains(&format!("\"Bash({BIN} task add:*)\"")), "{text}");
    // Its last line says platoon wrote it, with the POSIX cksum of the rest (what `scripts/uninstall.sh` checks).
    let (body, last) = split_last(&text);
    assert!(last.starts_with("<!-- platoon setup wrote this file") && last.ends_with(&format!(" cksum {} -->\n", cksum(body))), "{last}");

    let again = w.setup();
    assert!(again.contains("Nothing to change.") && !again.contains("added"), "{again}");
    assert_eq!(fs::read_to_string(w.skill()).unwrap(), text);

    // An older platoon's copy (another text, its own trailer): updated to this one.
    let older = body.replace("Fewer is better", "Few is better");
    fs::write(w.skill(), format!("{older}<!-- platoon setup wrote this file and keeps it up to date; once you change it, platoon leaves it alone. cksum {} -->\n", cksum(&older))).unwrap();
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
    assert!(said.contains("[–] /to-tickets") && said.contains(&line), "{said}");
    assert_eq!(fs::read_to_string(w.skill()).unwrap(), mine);
    let tasks = w.run(&["task", "squads"], "");
    assert!(tasks.status.success() && String::from_utf8_lossy(&tasks.stdout).contains("bug-fix: "), "{tasks:?}");
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

const NOTICE: &str = "New: /to-tickets in Claude Code sessions — run `platoon setup` to add it";

#[test]
fn platoon_says_once_that_setup_has_the_skill_to_add_and_nothing_once_it_is_in() {
    // Updated without running setup again: the skill is missing. Said above the footer, once.
    let w = World::new("notice");
    w.setup();
    fs::remove_dir_all(w.claude().join("skills")).unwrap();
    let mut t = window(&w);
    t.wait_for_all(&[NOTICE, "? help"]);
    t.send(b"j");
    t.wait_gone("New: /to-tickets");
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);
    let mut t = window(&w);
    t.wait_for("? help");
    assert!(!t.stable().contains("/to-tickets"), "said once only");
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);

    // With the skill in: nothing.
    let w = World::new("notice-done");
    w.setup();
    let mut t = window(&w);
    t.wait_for("? help");
    assert!(!t.stable().contains("/to-tickets"));
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
