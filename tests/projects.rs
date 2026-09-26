//! Projects end to end, in a real terminal with the real binary: the top-level tabs, the Projects tab and its keys,
//! the project picker and the project column on the Sessions tab, a project's screen, a new session started under a project,
//! archiving and restoring a project with a live stand-in session, `platoon open --project`, and the SessionStart hook.
//!
//! The "claude" is a small script: it registers itself the way Claude does, prints how it was started and what
//! `PLATOON_PROJECT` it got, can run the hook the way Claude would, and then gives a shell. A real `claude` is never
//! started, and every directory is a temp dir; the only processes signalled are ones these tests started.

mod common;
use common::*;
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
};

struct Lab {
    base: PathBuf,
    claude: PathBuf,
    cache: PathBuf,
    config: PathBuf,
    work: PathBuf,
    procs: Vec<Child>,
}

const FAKE: &str = r#"#!/bin/sh
mkdir -p "$FAKE_DIR/started"
n=$(( $(ls "$FAKE_DIR/started" | wc -l) + 1 ))
: > "$FAKE_DIR/started/$n"
sid="new-000$n"
[ "$1" = "--resume" ] && sid="$2"
printf '{"pid":%s,"sessionId":"%s","cwd":"%s","kind":"interactive","startedAt":%s,"status":"idle"}' $$ "$sid" "$(pwd)" "$(date +%s%3N)" > "$CLAUDE_CONFIG_DIR/sessions/$$.json"
if [ -n "$FAKE_HOOK" ]; then
  printf '{"session_id":"%s","source":"startup","cwd":"%s","hook_event_name":"SessionStart"}' "$sid" "$(pwd)" | "$FAKE_HOOK" hook session-start > "$FAKE_DIR/hook-out-$n"
fi
echo "fake-claude args:$* project:${PLATOON_PROJECT:-none} sid:$sid"
exec sh
"#;

impl Lab {
    fn new(name: &str) -> Self {
        let base = tmp(name);
        let lab = Self { claude: base.join("claude"), cache: base.join("cache"), config: base.join("config"), work: base.join("work"), base, procs: Vec::new() };
        for d in [&lab.claude.join("sessions"), &lab.claude.join("projects/-work"), &lab.cache, &lab.config, &lab.work] {
            fs::create_dir_all(d).unwrap();
        }
        // The transcripts carry fixed dates that will be old one day: this is not about auto-archive.
        fs::write(lab.config.join("settings.json"), r#"{"auto_archive_days":0}"#).unwrap();
        let fake = lab.base.join("fake-claude.sh");
        fs::write(&fake, FAKE).unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        lab
    }

    fn env(&self) -> Vec<(&'static str, String)> {
        vec![
            ("CLAUDE_CONFIG_DIR", self.claude.to_string_lossy().into()),
            ("PLATOON_CACHE_DIR", self.cache.to_string_lossy().into()),
            ("PLATOON_CONFIG_DIR", self.config.to_string_lossy().into()),
            ("PLATOON_CLAUDE_BIN", self.base.join("fake-claude.sh").to_string_lossy().into()),
            ("PLATOON_SKIP_SETUP_CHECK", "1".into()),
            ("FAKE_DIR", self.base.to_string_lossy().into()),
        ]
    }

    fn save(&self, id: &str, prompt: &str, hour: u64) {
        let line = format!(
            r#"{{"type":"user","cwd":"{}","gitBranch":"main","timestamp":"2026-09-19T{hour:02}:00:00.000Z","message":{{"role":"user","content":"{prompt}"}}}}"#,
            self.work.display()
        );
        fs::write(self.claude.join("projects/-work").join(format!("{id}.jsonl")), line + "\n").unwrap();
    }

    /// A projects file with one project `pweb` ("Web", badge WB, blue) holding `sessions`.
    fn project(&self, sessions: &[&str], notes: Option<&str>) {
        let members: serde_json::Map<String, Value> = sessions.iter().map(|s| (s.to_string(), json!("pweb"))).collect();
        let v = json!({"version": 1, "projects": {"pweb": {"name": "Web", "color": "blue", "badge": "WB", "archived": false, "created": 1, "archived_sessions": []}}, "sessions": members});
        fs::write(self.config.join("projects.json"), v.to_string()).unwrap();
        if let Some(n) = notes {
            fs::create_dir_all(self.config.join("projects/pweb")).unwrap();
            fs::write(self.config.join("projects/pweb/project.md"), n).unwrap();
        }
    }

    fn projects(&self) -> Value {
        fs::read_to_string(self.config.join("projects.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
    }

    fn archived(&self) -> Vec<String> {
        let Ok(text) = fs::read_to_string(self.config.join("archive.json")) else { return vec![] };
        let v: Value = serde_json::from_str(&text).unwrap();
        v["archived"].as_object().unwrap().keys().cloned().collect()
    }

    /// A process of ours registered as running `id`; told to stop, it exits after a moment.
    fn run(&mut self, id: &str) -> u32 {
        let ready = self.cache.join(format!("ready-{id}"));
        let script = format!("trap 'sleep 0.3; exit 0' TERM; : > '{}'; while :; do sleep 0.05; done", ready.display());
        let child = Command::new("sh").arg("-c").arg(script).spawn().unwrap();
        let pid = child.id();
        self.procs.push(child);
        until("the stand-in to be ready", || ready.exists());
        let reg = json!({"pid": pid, "sessionId": id, "cwd": self.work, "kind": "interactive", "startedAt": 1, "status": "idle"});
        fs::write(self.claude.join("sessions").join(format!("{pid}.json")), reg.to_string()).unwrap();
        pid
    }

    fn exited(&mut self, pid: u32) -> bool {
        self.procs.iter_mut().find(|c| c.id() == pid).is_none_or(|c| c.try_wait().unwrap().is_some())
    }

    /// End the fake claudes the hosts run (interactive `sh` ignores SIGTERM), by the exact pids they published.
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

/// Where `needle` starts on the screen: (row, column).
fn locate(text: &str, needle: &str) -> (u16, u16) {
    for (r, line) in text.lines().enumerate() {
        if let Some(b) = line.find(needle) {
            return (r as u16, line[..b].chars().count() as u16);
        }
    }
    panic!("{needle:?} not on screen:\n{text}");
}

fn click(t: &mut Term, row: u16, col: u16) {
    t.send(format!("\x1b[<0;{};{}M\x1b[<0;{};{}m", col + 1, row + 1, col + 1, row + 1).as_bytes());
}

#[test]
fn projects_are_made_renamed_given_sessions_shown_as_badges_and_deleted() {
    let lab = Lab::new("proj-crud");
    lab.save("aaaa-0001", "refactor the widget", 5);
    lab.save("bbbb-0002", "write the release notes", 2);
    let mut env = lab.env();
    env.push(("NO_COLOR", String::new())); // the helper sets it, which makes platoon draw without any color
    let mut t = Term::spawn(&[], &env, &lab.work, 30, 110);
    t.wait_for("refactor the widget");
    t.wait_for_flat("1 Sessions 2 2 Projects 0");
    t.wait_for_flat("Active 2 Archived 0");

    // Left and right: Active and Archived.
    t.send(b"\x1b[C");
    t.wait_for("Nothing archived.");
    t.send(b"\x1b[D");
    t.wait_for("refactor the widget");

    // 2: the Projects tab. n makes a project.
    t.send(b"2");
    t.wait_for("No projects yet. Press n to make one.");
    t.send(b"n");
    t.wait_for("╭ new project");
    t.send(b"Web shop");
    t.wait_for("❯ Web shop");
    t.send(b"\r");
    t.wait_gone("╭ new project");
    t.wait_for_flat("WS Web shop");
    t.wait_for_flat("2 Projects 1");
    let id = lab.projects()["projects"].as_object().unwrap().keys().next().unwrap().clone();

    // r renames it.
    t.send(b"r");
    t.wait_for("╭ rename project");
    t.send(b"\x15Shop");
    t.wait_for("❯ Shop");
    t.send(b"\r");
    t.wait_gone("╭ rename project");
    t.wait_for_flat("WS Shop");
    assert_eq!(lab.projects()["projects"][&id]["name"], "Shop");

    // 1, then m on a session: the picker; enter takes the first (the only) project.
    t.send(b"1");
    t.wait_for("refactor the widget");
    t.wait_for_flat("m project");
    t.send(b"m");
    t.wait_for("New project…");
    t.wait_for("No project");
    t.send(b"\r");
    t.wait_gone("New project…");
    t.wait_for_all(&["PROJECT", "NAME", " Shop "]);
    let session = lab.projects()["sessions"].as_object().unwrap().keys().next().cloned().expect("a member");
    assert!(session == "aaaa-0001" || session == "bbbb-0002");
    // The project's own column, right after NAME: its name in a pill of its color (the first project gets the theme's
    // accent), the letters on it dark. The badge letters are no longer in front of the name.
    // Where the columns sit, and what is no longer in front of the name: claims about the whole row, so the
    // screen has to have settled.
    let text = t.stable();
    let (head, head_col) = locate(&text, "PROJECT");
    let (row, col) = locate(&text, " Shop ");
    assert!(row > head && col == head_col && locate(&text, "NAME").1 < head_col, "{text}");
    assert!(!text.contains(" WS "), "{text}");
    t.wait_bg(row, col + 1, vt100::Color::Rgb(0xb4, 0xbe, 0xfe));
    t.wait_fg(row, col + 1, vt100::Color::Rgb(0x1e, 0x1e, 0x2e));

    // A click on the Projects chip goes there.
    let (row, col) = locate(&t.text(), "2 Projects");
    click(&mut t, row, col + 2);
    t.wait_for("PROJECT");
    t.wait_for_flat("1 active");

    // d deletes it, after asking; the session stays, in no project.
    t.send(b"d");
    t.wait_for("╭ delete project");
    t.wait_for_flat("y delete");
    t.send(b"y");
    t.wait_for("No projects yet");
    let v = lab.projects();
    assert!(v["projects"].as_object().unwrap().is_empty() && v["sessions"].as_object().unwrap().is_empty(), "{v}");
    assert!(lab.cache.join("deleted-projects").join(&id).join("project.json").exists());
    t.send(b"1");
    t.wait_for_flat("Active 2");
    t.wait_gone(" Shop ");
    t.wait_gone("PROJECT");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn a_projects_screen_shows_its_sessions_and_notes_searches_only_them_and_a_new_session_lands_in_it() {
    let lab = Lab::new("proj-screen");
    lab.save("aaaa-0001", "fix the docker build", 5);
    lab.save("bbbb-0002", "write the docs", 2);
    lab.project(&["aaaa-0001"], Some("# Overview\nSell socks online.\n"));
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 110);
    t.wait_for("write the docs");
    t.wait_for_flat("2 Projects 1");
    t.send(b"2");
    t.wait_for_flat("WB Web");
    t.send(b"\r");
    t.wait_for("Sell socks online.");
    t.wait_for_flat("WB Web 1 active");
    t.wait_for("╭ Sessions");
    t.wait_for("╭ Notes");
    let shown = t.stable();
    assert!(!shown.contains("write the docs") && !shown.contains("bbbb"), "only its own sessions:\n{shown}");

    // The quick filter only looks at the project's sessions.
    t.send(b"/");
    t.wait_for("╭ Search");
    t.send(b"docs");
    t.wait_for("No sessions match the filter");
    t.send(b"\x15work");
    t.wait_for_flat("1 shown");
    // Full-text: "docs" is only in the other session, so nothing is found here.
    t.send(b"\t");
    t.wait_for("Finds conversations by the words you typed");
    t.send(b"\x15docs");
    t.wait_for("Nothing found");
    t.send(b"\x15docker");
    t.wait_for_flat("1 found");
    t.send(b"\x1b");
    t.wait_gone("╭ Search");

    // n: the folder picker, the project's folder first; the new session carries the project and is put in it.
    t.send(b"n");
    t.wait_for("new session in Web");
    t.send(b"\r");
    t.wait_for("project:pweb sid:new-0001");
    until("the new session to be put in the project", || lab.projects()["sessions"]["new-0001"] == "pweb");
    t.send(&[0]); // back to the project's screen
    t.wait_for_flat("WB Web 2 active");
    // Enter on the project's older session resumes it, carrying the project too.
    t.send(b"\x1b[B\r");
    t.wait_for("args:--resume aaaa-0001 project:pweb");
    t.send(&[0]);
    t.wait_for_flat("WB Web 2 active");
    t.send(b"\x1b");
    t.wait_for("PROJECT");
    t.send(b"q");
    t.wait_for("still running in the background");
    t.wait_exit();
}

#[test]
fn archiving_a_project_stops_its_live_session_and_restoring_brings_back_exactly_those() {
    let mut lab = Lab::new("proj-archive");
    lab.save("aaaa-0001", "refactor the widget", 5);
    lab.save("bbbb-0002", "write the release notes", 4);
    lab.save("cccc-0003", "an old side quest", 3);
    lab.save("dddd-0004", "not in the project", 2);
    lab.project(&["aaaa-0001", "bbbb-0002", "cccc-0003"], None);
    fs::write(lab.config.join("archive.json"), r#"{"version":1,"archived":{"cccc-0003":5}}"#).unwrap();
    let pid = lab.run("aaaa-0001");
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 110);
    t.wait_for("refactor the widget");
    t.send(b"2");
    t.wait_for_flat("2 active · 1 archived");
    // The live one runs in a terminal outside platoon: asked first.
    t.send(b"a");
    t.wait_for("╭ archive project");
    t.wait_for_flat("1 of its sessions is running");
    assert!(!lab.exited(pid) && lab.archived() == ["cccc-0003"], "asking changes nothing");
    t.send(b"y");
    until("both active sessions archived", || lab.archived().len() == 3);
    assert!(lab.exited(pid), "archived only once the stand-in had exited");
    t.wait_for_flat("Active 0 Archived 1");
    t.send(b"\x1b[C");
    t.wait_for_flat("a restore");
    t.send(b"a");
    until("the project's two sessions restored", || lab.archived() == ["cccc-0003"]);
    t.wait_for_flat("Active 1 Archived 0");
    t.send(b"1");
    t.wait_for("refactor the widget");
    t.wait_for_flat("Active 3 Archived 1");
    t.send(b"q");
    t.wait_exit();
    assert_eq!(lab.projects()["projects"]["pweb"]["archived"], false);
}

#[test]
fn open_with_a_project_starts_a_session_in_it_and_the_hook_places_it_and_hands_it_the_notes() {
    let lab = Lab::new("proj-open");
    lab.project(&[], Some("# Overview\nSell socks online.\n"));
    let mut env = lab.env();
    env.push(("FAKE_HOOK", BIN.to_string()));
    // An unknown project is refused with its reason.
    let mut t = Term::spawn(&["open", "--project", "nope"], &env, &lab.work, 24, 90);
    t.wait_for("there is no project called \"nope\"");
    assert_eq!(t.wait_exit(), 1);
    // By name, ignoring case: a new session here, carrying the project.
    let mut t = Term::spawn(&["open", "--project", "web"], &env, &lab.work, 24, 90);
    t.wait_for("project:pweb sid:new-0001");
    assert!(t.text().contains("args: "), "a new session, no resume: {}", t.text());
    // The hook (run by the stand-in with its own environment, as Claude runs hooks) put it in the project and gave
    // it the notes.
    until("the session to be placed", || lab.projects()["sessions"]["new-0001"] == "pweb");
    let out = fs::read_to_string(lab.base.join("hook-out-1")).unwrap();
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    let ctx = v["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
    assert!(ctx.contains("the project \"Web\"") && ctx.contains("Sell socks online."), "{ctx}");
    t.send(&[0]);
    t.wait_exit();
}

fn hook(lab: &Lab, input: &str, project: Option<&str>) -> (i32, String) {
    let mut cmd = Command::new(BIN);
    cmd.args(["hook", "session-start"])
        .env("CLAUDE_CONFIG_DIR", &lab.claude)
        .env("PLATOON_CACHE_DIR", &lab.cache)
        .env("PLATOON_CONFIG_DIR", &lab.config)
        .env_remove("PLATOON_PROJECT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(p) = project {
        cmd.env("PLATOON_PROJECT", p);
    }
    let mut child = cmd.spawn().unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned())
}

fn start(id: &str, source: &str) -> String {
    json!({"session_id": id, "transcript_path": "/nowhere", "cwd": "/tmp", "hook_event_name": "SessionStart", "source": source}).to_string()
}

#[test]
fn the_session_start_hook_sends_notes_only_when_new_or_changed_and_otherwise_prints_nothing() {
    let lab = Lab::new("proj-hook");
    lab.project(&["member-1"], Some("# Overview\nSell socks.\n"));
    let (code, out) = hook(&lab, &start("member-1", "startup"), None);
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(out.trim()).expect("one JSON object");
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
    assert!(v["hookSpecificOutput"]["additionalContext"].as_str().unwrap().ends_with("# Overview\nSell socks."));
    assert_eq!(hook(&lab, &start("member-1", "resume"), None), (0, String::new()), "unchanged: nothing");
    assert_eq!(hook(&lab, &start("member-1", "clear"), None), (0, String::new()));
    assert!(!hook(&lab, &start("member-1", "compact"), None).1.is_empty(), "after a compaction it goes again");
    fs::write(lab.config.join("projects/pweb/project.md"), "# Overview\nSell socks and hats.\n").unwrap();
    assert!(hook(&lab, &start("member-1", "resume"), None).1.contains("hats"), "changed: sent again");
    // Nothing to say: in no project, bad input, no input.
    for input in [start("stranger", "startup"), "not json".into(), String::new(), "{}".into()] {
        assert_eq!(hook(&lab, &input, None), (0, String::new()), "{input}");
    }
    // PLATOON_PROJECT places a brand-new session.
    let (code, out) = hook(&lab, &start("fresh-1", "startup"), Some("pweb"));
    assert!(code == 0 && out.contains("Sell socks and hats"), "{out}");
    assert_eq!(lab.projects()["sessions"]["fresh-1"], "pweb");
    // No notes file: nothing, but the placing still happens.
    fs::remove_file(lab.config.join("projects/pweb/project.md")).unwrap();
    assert_eq!(hook(&lab, &start("fresh-2", "startup"), Some("pweb")), (0, String::new()));
    assert_eq!(lab.projects()["sessions"]["fresh-2"], "pweb");
    // A projects file that can't be read is left alone and nothing is printed.
    fs::write(lab.config.join("projects.json"), "{ broken").unwrap();
    assert_eq!(hook(&lab, &start("fresh-3", "startup"), Some("pweb")), (0, String::new()));
    assert_eq!(fs::read_to_string(lab.config.join("projects.json")).unwrap(), "{ broken");
}
