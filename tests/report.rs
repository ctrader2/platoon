//! `platoon report` and the diagnostics log, end to end: the real binary run against a temporary world (its own home,
//! Claude Code and Codex folders, platoon's folders, a running "session" platoon's host file points at), with secrets
//! planted everywhere a careless report or log could pick them up: a token in a process's arguments, in an environment
//! variable, in a `.env` file, in Claude Code's `~/.claude.json` and `settings.json`, in a conversation's prompt and
//! reply, in the name the user gave a conversation and the one platoon gave it, in a task's title and text, in a hook's
//! input. None of them may reach the report or the log. Nothing here needs a terminal.

mod common;
use common::*;
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

/// Every planted secret starts with this.
const PLANTED: &str = "PLANTED";
const SESSION: &str = "0b5c2a3e-1f00-4d2b-9a77-1234567890ab";
const PROJECT_NAME: &str = "Acme Zebra Project";
const FOLDER: &str = "acme-zebra-repo";
const NOTES: &str = "# Notes\nPLANTED-notes\n";

struct World {
    base: PathBuf,
    /// The stand-in session the host's file points at: `sh` with a token in its arguments, and a `sleep` under it.
    agent: Option<Child>,
}

impl World {
    fn new(name: &str) -> World {
        let base = tmp(name);
        let (claude, config, cache) = (base.join("claude"), base.join("config"), base.join("cache"));
        let work = base.join("repos").join(FOLDER);
        for d in [&claude, &config, &cache, &work] {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(work.join(".env"), "GITLAB_TOKEN=PLANTED-dotenv-token\n").unwrap();
        // Claude Code's own files, with secrets of their own.
        fs::write(base.join(".claude.json"), r#"{"oauthAccount":{"emailAddress":"PLANTED-email@example.com"},"mcpServers":{"x":{"command":"mcp","args":["--token","PLANTED-mcp-token"]}}}"#).unwrap();
        fs::write(claude.join(".claude.json"), r#"{"primaryApiKey":"PLANTED-api-key"}"#).unwrap();
        fs::write(
            claude.join("settings.json"),
            r#"{"env":{"ANTHROPIC_API_KEY":"PLANTED-settings-key"},"statusLine":{"type":"command","command":"PLANTED-own-status-line"}}"#,
        )
        .unwrap();
        // A conversation, its prompt and reply planted, and platoon's index of it (which keeps the first prompt).
        let project = claude.join("projects").join("-repos-acme-zebra-repo");
        fs::create_dir_all(&project).unwrap();
        let transcript = project.join(format!("{SESSION}.jsonl"));
        let cwd = work.to_string_lossy();
        fs::write(
            &transcript,
            format!(
                "{}\n{}\n",
                json!({"type":"user","cwd":cwd,"sessionId":SESSION,"timestamp":"2026-10-27T10:00:00.000Z","message":{"role":"user","content":"PLANTED-prompt fix the zebra"}}),
                json!({"type":"assistant","cwd":cwd,"sessionId":SESSION,"timestamp":"2026-10-27T10:00:05.000Z","message":{"role":"assistant","content":[{"type":"text","text":"PLANTED-reply done"}]}})
            ),
        )
        .unwrap();
        let index = json!({"version": 3, "files": {transcript.to_string_lossy(): {"m": 1, "s": 1, "cwd": cwd, "dirs": [], "branch": "main",
            "model": "claude-opus-5-5", "tokens": 1000, "large": false, "title": "PLANTED-ai-title", "prompt": "PLANTED-prompt fix the zebra", "active": 1}}});
        fs::write(cache.join("index.json"), index.to_string()).unwrap();
        // platoon's own records of it: a name the user gave, a name and summary platoon made, an archive entry.
        fs::write(config.join("names.json"), json!({"version": 1, "names": {SESSION: "PLANTED-session-name"}}).to_string()).unwrap();
        fs::write(
            config.join("summaries.json"),
            json!({"version": 1, "entries": {SESSION: {"name": "PLANTED-smart-name", "summary": "PLANTED-summary", "covers": 1, "model": "haiku", "at": 1}}})
                .to_string(),
        )
        .unwrap();
        fs::write(config.join("settings.json"), json!({"diagnostics_log": true, "ai_model": "PLANTED model with words"}).to_string())
            .unwrap();
        // A project with notes, and a task in it whose title and text are planted.
        let projects = json!({"version": 1, "projects": {"pzebra": {"name": PROJECT_NAME, "color": "blue", "badge": "AZ", "archived": false,
            "created": 1, "archived_sessions": [], "description": "PLANTED-description"}}, "sessions": {SESSION: "pzebra"}});
        fs::write(config.join("projects.json"), projects.to_string()).unwrap();
        fs::create_dir_all(config.join("projects/pzebra/tasks")).unwrap();
        fs::write(config.join("projects/pzebra/project.md"), NOTES).unwrap();
        fs::write(config.join("projects/pzebra/tasks/01-zebra.md"), "PLANTED-task-text\n").unwrap();
        let worktree = base.join("repos").join(format!("{FOLDER}-worktrees")).join("task-1");
        let tasks = json!({"next": 2, "tasks": [{"number": 1, "title": "PLANTED-task-title", "file": "01-zebra.md", "state": "needs_you",
            "need": "question", "why": "PLANTED-question-words", "folder": cwd, "squad": "feature", "session": SESSION, "created": 1, "updated": 1,
            "started": 1, "worktree": {"path": worktree.to_string_lossy(), "branch": "task/1", "base": "abc"}}]});
        fs::write(config.join("projects/pzebra/tasks.json"), tasks.to_string()).unwrap();
        // The usage meter's reports.
        let now = now_secs();
        fs::create_dir_all(cache.join("status")).unwrap();
        fs::write(
            cache.join(format!("status/{SESSION}.json")),
            json!({"ts": now - 60, "context_pct": 40, "five_hour": {"pct": 7, "resets_at": now + 3600}, "seven_day": {"pct": 16, "resets_at": now + 86_400}})
                .to_string(),
        )
        .unwrap();
        // What the smart features spent, with a feature this platoon doesn't know.
        let day = local_day();
        let ledger = json!({"version": 1, "days": {day: {"usage": {"input": 1200, "output": 300, "cache_creation": 0, "cache_read": 0}, "calls": 3,
            "failed": 1, "features": {"names": {"tokens": 1000, "calls": 2}, "PLANTED-feature": {"tokens": 500, "calls": 1}}}}});
        fs::write(cache.join("ai-usage.json"), ledger.to_string()).unwrap();
        World { base, agent: None }
    }

    /// The running "session" a host's file names: `sh` with a token in its arguments and a `sleep` under it. Its host
    /// is this test's own process (alive, and never signalled by a report).
    fn start_session(&mut self) -> u32 {
        let agent = Command::new("sh")
            .args(["-c", "sleep 120; : done", "sh", "--token", "PLANTED-arg-token"])
            .env("GITLAB_TOKEN", "PLANTED-child-env")
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        let pid = agent.id();
        self.agent = Some(agent);
        let hosts = self.base.join("cache/hosts");
        fs::create_dir_all(&hosts).unwrap();
        let info = json!({"agent": "claude", "version": "1.0.1", "claude_pid": pid, "host_pid": std::process::id(),
            "dir": self.base.join("repos").join(FOLDER).to_string_lossy(), "args": ["--resume", SESSION, "--append-system-prompt", "PLANTED-host-arg"],
            "kept": {"why": "command", "what": "npm run PLANTED-run-word"}, "windows": 0});
        fs::write(hosts.join(format!("{pid}.json")), info.to_string()).unwrap();
        // A file left over by a host whose session has gone.
        fs::write(
            hosts.join("999999.json"),
            json!({"agent": "codex", "claude_pid": 999_999, "host_pid": 999_998, "dir": "/", "args": []}).to_string(),
        )
        .unwrap();
        // Wait for the shell to have started its sleep.
        until("the session's child", || procs_under(pid));
        pid
    }

    fn env(&self) -> Vec<(&'static str, String)> {
        let b = |p: &str| self.base.join(p).to_string_lossy().into_owned();
        let mut env = vec![
            ("HOME", self.base.to_string_lossy().into_owned()),
            ("CLAUDE_CONFIG_DIR", b("claude")),
            ("PLATOON_CACHE_DIR", b("cache")),
            ("PLATOON_CONFIG_DIR", b("config")),
            ("PLATOON_NO_UPDATE_CHECK", "1".into()),
            ("PLATOON_SKIP_SETUP_CHECK", "1".into()),
            ("PLATOON_SECRET_FOR_THE_TEST", "PLANTED-env-value".into()),
            ("GITLAB_TOKEN", "PLANTED-env-token".into()),
            ("TERM", "xterm-256color".into()),
        ];
        env.extend(NO_REAL_CODEX.iter().map(|(k, v)| (*k, v.to_string())));
        env.push((NO_REAL_CLAUDE.0, NO_REAL_CLAUDE.1.to_string()));
        env
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(BIN);
        c.args(args).current_dir(&self.base).env_clear().env("PATH", std::env::var("PATH").unwrap_or_default());
        for (k, v) in self.env() {
            c.env(k, v);
        }
        c.stdin(Stdio::null());
        c
    }

    fn report(&self, args: &[&str]) -> String {
        let out = self.cmd(&["report"]).args(args).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    }

    fn log(&self) -> String {
        fs::read_to_string(self.base.join("cache/diagnostics.log")).unwrap_or_default()
    }

    /// A hook run as Claude Code would, given `input`.
    fn hook(&self, name: &str, input: &str) -> std::process::Output {
        let mut c = self.cmd(&["hook", name]);
        c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        std::io::Write::write_all(child.stdin.as_mut().unwrap(), input.as_bytes()).unwrap();
        drop(child.stdin.take());
        child.wait_with_output().unwrap()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        if let Some(mut c) = self.agent.take() {
            // Our own child, and the sleep under it: its process group is ours too, so end the sleep by its parent's
            // pid only.
            let pid = c.id();
            for p in children_of(pid) {
                unsafe { libc::kill(p as i32, libc::SIGKILL) };
            }
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

/// Today in local time, as the ledger keys it (`date`, which every system has, without GNU-only flags).
fn local_day() -> String {
    let out = Command::new("date").arg("+%Y-%m-%d").output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// The pids whose parent is `pid`, from `ps` (here and on macOS).
fn children_of(pid: u32) -> Vec<u32> {
    let out = Command::new("ps").args(["-A", "-o", "pid=,ppid="]).output().unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            let (p, pp) = (w.next()?.parse::<u32>().ok()?, w.next()?.parse::<u32>().ok()?);
            (pp == pid).then_some(p)
        })
        .collect()
}

fn procs_under(pid: u32) -> bool {
    !children_of(pid).is_empty()
}

fn assert_nothing_planted(what: &str, text: &str) {
    for (i, line) in text.lines().enumerate() {
        assert!(!line.contains(PLANTED), "{what}, line {}: a planted secret got through: {line}\n\n{text}", i + 1);
    }
}

#[test]
fn the_report_has_every_section_from_its_world_and_nothing_planted() {
    let mut w = World::new("report-sections");
    let pid = w.start_session();
    // Something in the log first: a hook given what isn't a hook's input (an error, always written), with the setting
    // on a line for the hook's run too.
    let out = w.hook("session-start", "PLANTED-hook-input that is not json");
    assert!(out.status.success());
    let text = w.report(&["--stdout"]);
    assert_nothing_planted("the report", &text);
    for want in [
        "== Versions ==",
        "  platoon: ",
        "  Claude Code: installed, but its version couldn't be read",
        "  Codex: not installed",
        "  terminal: TERM=xterm-256color",
        "  terminal size: (no terminal)",
        "set in the environment: PLATOON_CACHE_DIR PLATOON_CONFIG_DIR CLAUDE_CONFIG_DIR CODEX_HOME PLATOON_CLAUDE_BIN",
        "== Folders ==",
        "  platoon's cache: ~/cache",
        "== Setup ==",
        "  Claude Code, Status line: left-alone",
        "  Claude Code, Session guard: missing",
        "  Claude Code, /to-tickets skill: missing",
        "== Settings ==",
        "  keep a diagnostics log: yes",
        "  model for smart features: ?",
        "== Conversations (counts only) ==",
        "  Claude Code: 1 in platoon's index, 0 archived, 1 named by you, 0 named by platoon, 0 not named, 1 tasks' sessions",
        "== Projects ==",
        &format!("  {PROJECT_NAME}: 1 sessions, 1 tasks, notes {} bytes, description yes", NOTES.len()),
        "== Sessions platoon runs ==",
        &format!("  claude session, pid {pid} (host {}), started by platoon 1.0.1", std::process::id()),
        &format!("in ~/repos/{FOLDER}"),
        &format!("    session {SESSION}, task none, windows open 0"),
        "    kept running because: command npm",
        "    under it: sleep, pid ",
        "  codex session, pid 999999 (host 999998), started by platoon (a platoon older than 1.1.0)",
        "    GONE: the agent's process isn't running; its file is left over",
        "== Plan usage ==",
        &format!("    {SESSION} reported 1m00s ago: 5h 7% resets "),
        "  the meter shows: 5h 7% (resets in ",
        "== Smart features ==",
        "names 1000; other 500",
        "== Tasks (no titles, no text) ==",
        "  1 tasks, 1 needs_you",
        &format!("  #1 in {PROJECT_NAME}: needs_you (question), squad feature, claude, session running yes, worktree missing"),
        &format!("    worktree ~/repos/{FOLDER}-worktrees/task-1"),
        "== Notes updates ==",
        "== Diagnostics log ==",
        "  on: decisions and errors",
        " error hook input-not-understood agent=claude kind=session-start why=not-json bytes=",
        " decision hook ran hook=session-start agent=claude took=",
    ] {
        assert!(text.contains(want), "missing {want:?} in:\n{text}");
    }
    // Under the session: its `sleep`, by name, never its arguments; and the session itself isn't listed as a program
    // with its token either.
    assert!(!text.contains("--token") && !text.contains("sleep 120") && !text.contains("run dev"), "{text}");
    // The log, read on its own: nothing planted either.
    assert_nothing_planted("the log", &w.log());
    // The report only read: the host's files, the status file and the ledger are as they were.
    assert!(w.base.join("cache/hosts/999999.json").exists(), "a report never tidies up");
    assert!(common::alive(pid), "nor stops anything");
}

#[test]
fn hide_names_replaces_every_project_and_folder_name_consistently_and_leaves_none_behind() {
    let mut w = World::new("report-hide");
    w.start_session();
    let text = w.report(&["--stdout", "--hide-names"]);
    assert_nothing_planted("the report", &text);
    let base_name = w.base.file_name().unwrap().to_string_lossy().into_owned();
    for real in [PROJECT_NAME, FOLDER, "repos", "Acme", "Zebra", &base_name] {
        assert!(!text.contains(real), "{real:?} is still in the report:\n{text}");
    }
    assert!(text.contains("Project and folder names are replaced by placeholders."), "{text}");
    // The same folder is the same placeholder everywhere: the session's folder and the task's worktree beside it.
    let folder_of = |prefix: &str| {
        let line = text.lines().find(|l| l.contains(prefix) && l.contains("~/")).unwrap_or_else(|| panic!("no {prefix:?} in:\n{text}"));
        line[line.find("~/").unwrap()..].split(|c: char| c == ',' || c.is_whitespace()).next().unwrap().to_string()
    };
    let session_dir = folder_of("    running for ");
    let worktree = folder_of("    worktree ");
    assert!(session_dir.starts_with("~/folder-") && worktree.starts_with("~/folder-"), "{session_dir} {worktree}");
    let parent = |p: &str| p.rsplit_once('/').unwrap().0.to_string();
    assert_eq!(parent(&session_dir), parent(&parent(&worktree)), "the same repos folder: {session_dir} {worktree}");
    assert!(worktree.ends_with("/task-1"), "{worktree}");
    assert!(text.contains("  project-1: 1 sessions") && text.contains("#1 in project-1: needs_you"), "{text}");
    assert!(text.contains("  platoon's cache: ~/folder-"), "{text}");
}

#[test]
fn the_report_is_written_to_a_file_in_the_current_folder_and_says_where_and_what() {
    let w = World::new("report-file");
    let out = w.cmd(&[]).arg("report").output().unwrap();
    let said = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{said}{}", String::from_utf8_lossy(&out.stderr));
    let file = fs::read_dir(&w.base)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.file_name().unwrap().to_string_lossy().starts_with("platoon-report-"))
        .expect("the report's file");
    let name = file.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.ends_with(".txt") && said.starts_with(&format!("Wrote ~/{name} (")), "{said}");
    assert!(said.contains("no conversation content") && said.contains("--hide-names") && said.contains("diagnostics log is on"), "{said}");
    let text = fs::read_to_string(&file).unwrap();
    assert!(text.starts_with("platoon report, made ") && text.contains("== Diagnostics log =="), "{text}");
    assert_nothing_planted("the report's file", &text);

    let elsewhere = w.base.join("out/r.txt");
    fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
    let out = w.cmd(&["report", "--out", &elsewhere.to_string_lossy()]).output().unwrap();
    assert!(out.status.success() && elsewhere.exists(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("Wrote ~/out/r.txt ("));
    let out = w.cmd(&["report", "--nonsense"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn a_version_probe_that_hangs_does_not_hold_the_report_up() {
    let w = World::new("report-hang");
    let slow = w.base.join("slow-claude.sh");
    fs::write(&slow, "#!/bin/sh\nexec sleep 60\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&slow, fs::Permissions::from_mode(0o755)).unwrap();
    let start = Instant::now();
    let out = w.cmd(&["report", "--stdout"]).env("PLATOON_CLAUDE_BIN", &slow).output().unwrap();
    let took = start.elapsed();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(took < Duration::from_secs(20), "took {took:?}");
    assert!(text.contains("  Claude Code: installed, but its version couldn't be read"), "{text}");
    // The probe it started was ended, not left sleeping.
    let mine = children_of(std::process::id());
    assert!(mine.iter().all(|p| {
        !Command::new("ps")
            .args(["-o", "args=", "-p", &p.to_string()])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("sleep 60"))
            .unwrap_or(false)
    }));
}

#[test]
fn a_hooks_error_lands_in_the_log_with_the_setting_off_and_its_run_only_with_it_on() {
    let w = World::new("report-hook-log");
    fs::write(w.base.join("config/settings.json"), r#"{"diagnostics_log": false}"#).unwrap();
    assert!(w.hook("session-start", "PLANTED not json").status.success());
    assert!(w.hook("stop", r#"{"no_session":"PLANTED-field"}"#).status.success());
    let log = w.log();
    assert_eq!(log.lines().count(), 2, "{log}");
    assert!(
        log.contains(" hook[") && log.contains(" error hook input-not-understood agent=claude kind=session-start why=not-json"),
        "{log}"
    );
    assert!(log.contains(" error hook input-not-understood agent=claude kind=stop why=no-session-id"), "{log}");
    assert!(!log.contains(" decision "), "with the setting off, errors only: {log}");
    assert_nothing_planted("the log", &log);

    fs::write(w.base.join("config/settings.json"), r#"{"diagnostics_log": true}"#).unwrap();
    let ok = json!({"session_id": SESSION, "cwd": "/", "hook_event_name": "SessionStart", "source": "startup"}).to_string();
    assert!(w.hook("session-start", &ok).status.success());
    let log = w.log();
    assert_eq!(log.lines().count(), 3, "{log}");
    assert!(log.lines().last().unwrap().contains(" decision hook ran hook=session-start agent=claude took="), "{log}");
}

#[test]
fn hooks_writing_at_once_leave_whole_lines() {
    let w = World::new("report-together");
    let children: Vec<_> = (0..12)
        .map(|_| {
            let mut c = w.cmd(&["hook", "session-start"]);
            c.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
            let mut child = c.spawn().unwrap();
            std::io::Write::write_all(child.stdin.as_mut().unwrap(), b"not json").unwrap();
            drop(child.stdin.take());
            child
        })
        .collect();
    for mut c in children {
        assert!(c.wait().unwrap().success());
    }
    let log = w.log();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.iter().filter(|l| l.contains(" error hook input-not-understood ")).count(), 12, "{log}");
    assert_eq!(lines.iter().filter(|l| l.contains(" decision hook ran ")).count(), 12, "{log}");
    for l in lines {
        assert!(l.starts_with("20") && (l.ends_with("bytes=8") || l.contains(" exit=0")), "a broken line: {l:?}");
    }
}

#[test]
fn a_hook_whose_log_cannot_be_written_still_runs_at_once() {
    let w = World::new("report-readonly");
    let cache = w.base.join("cache");
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&cache, fs::Permissions::from_mode(0o500)).unwrap();
    let start = Instant::now();
    let out = w.hook("session-start", "not json");
    fs::set_permissions(&cache, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(out.status.success() && start.elapsed() < Duration::from_secs(5), "{out:?}");
    assert!(out.stdout.is_empty(), "nothing said to Claude Code");
}

#[test]
fn the_report_needs_no_folders_at_all() {
    let w = World::new("report-empty");
    for d in ["cache", "config", "claude"] {
        fs::remove_dir_all(w.base.join(d)).unwrap();
    }
    let text = w.report(&["--stdout"]);
    assert!(text.contains("  platoon's cache: ~/cache (not there)") && text.contains("  (nothing written yet)"), "{text}");
    for d in ["cache", "config", "claude"] {
        assert!(!w.base.join(d).exists(), "the report made {d}");
    }
}
