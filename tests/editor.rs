//! The code editor (`v`) end to end, in a real terminal with the real binary and a stand-in for nvim
//! (`PLATOON_NVIM_BIN`): a small script that writes down what it was started with (arguments, folder, environment,
//! the session file) and waits for a line typed on its terminal. Checked: platoon's screen goes while it runs and
//! nothing of platoon draws over it, the keys reach it, and platoon comes back drawn in full where the user was; the
//! folders of a session and of a ticket; the question while the squad is working; the offer to install (from an
//! empty mirror: never the network); `v` typed into a box stays a letter.
//!
//! Every directory is a temp dir (the editor folder too: `PLATOON_DATA_DIR`); no real nvim, no real `claude`.

mod common;
use common::*;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

/// Stands in for nvim: writes down how it was started into a folder of its own under `$STANDIN_OUT`, shows a line,
/// waits for a line typed on its terminal, and writes that down too.
const STAND_IN: &str = r#"#!/bin/sh
d="$STANDIN_OUT/run-$$"
mkdir -p "$d"
printf '%s\n' "$@" > "$d/argv"
pwd > "$d/cwd"
env > "$d/env"
cp "$PLATOON_EDITOR_SESSION" "$d/session.json"
if [ -t 0 ]; then echo yes > "$d/stdin-is-a-terminal"; fi
printf 'STAND-IN EDITOR RUNNING\n'
IFS= read -r line
printf '%s\n' "$line" > "$d/typed"
"#;

struct Lab {
    base: PathBuf,
    claude: PathBuf,
    cache: PathBuf,
    config: PathBuf,
    work: PathBuf,
    data: PathBuf,
    out: PathBuf,
    standin: PathBuf,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

impl Lab {
    fn new(name: &str) -> Self {
        let base = tmp(name);
        let lab = Self {
            claude: base.join("claude"),
            cache: base.join("cache"),
            config: base.join("config"),
            work: base.join("work"),
            data: base.join("data"),
            out: base.join("out"),
            standin: base.join("bin/nvim-stand-in"),
            base,
        };
        for d in [&lab.claude.join("sessions"), &lab.claude.join("projects/-work"), &lab.cache, &lab.config, &lab.work, &lab.out] {
            fs::create_dir_all(d).unwrap();
        }
        fs::create_dir_all(lab.standin.parent().unwrap()).unwrap();
        fs::write(&lab.standin, STAND_IN).unwrap();
        fs::set_permissions(&lab.standin, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(lab.config.join("settings.json"), r#"{"auto_archive_days":0}"#).unwrap();
        // The work folder is a git repository.
        fs::write(lab.work.join("README"), "hi\n").unwrap();
        git(&lab.work, &["init", "-q"]);
        git(&lab.work, &["add", "."]);
        git(&lab.work, &["commit", "-q", "-m", "first"]);
        lab
    }

    fn env(&self) -> Vec<(&'static str, String)> {
        vec![
            ("CLAUDE_CONFIG_DIR", s(&self.claude)),
            ("PLATOON_CACHE_DIR", s(&self.cache)),
            ("PLATOON_CONFIG_DIR", s(&self.config)),
            ("PLATOON_DATA_DIR", s(&self.data)),
            ("PLATOON_SKIP_SETUP_CHECK", "1".into()),
            ("PLATOON_NOTES_HELPER", "off".into()),
            ("PLATOON_NVIM_BIN", s(&self.standin)),
            ("STANDIN_OUT", s(&self.out)),
            ("XDG_CONFIG_HOME", s(&self.base.join("user-config"))),
        ]
    }

    /// A conversation in the work folder with `added` folders beside it (Claude Code's environment snapshot).
    fn session(&self, added: &[String]) {
        let lines = [
            json!({"type":"user","cwd":self.work,"gitBranch":"main","timestamp":"2026-09-24T10:00:00.000Z","message":{"role":"user","content":"make the shop"}}),
            json!({"type":"attachment","isSidechain":false,"cwd":self.work,"timestamp":"2026-09-24T10:00:00.000Z",
                "attachment":{"type":"environment","snapshot":{"workingDirectory":self.work,"additionalWorkingDirectories":added},"changes":null}}),
        ];
        let text: Vec<String> = lines.iter().map(|v| v.to_string()).collect();
        fs::write(self.claude.join("projects/-work/s-1.jsonl"), text.join("\n") + "\n").unwrap();
    }

    /// The stand-in's runs so far.
    fn runs(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = fs::read_dir(&self.out).unwrap().flatten().map(|e| e.path()).collect();
        v.sort();
        v
    }

    fn only_run(&self) -> PathBuf {
        let runs = self.runs();
        assert_eq!(runs.len(), 1, "{runs:?}");
        runs[0].clone()
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn env_of(run: &Path) -> HashMap<String, String> {
    fs::read_to_string(run.join("env")).unwrap().lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn session_of(run: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(run.join("session.json")).unwrap()).unwrap()
}

/// Waits for a file the stand-in writes.
fn wait_file(p: &Path) -> String {
    until(&format!("{} to be written", p.display()), || p.is_file());
    fs::read_to_string(p).unwrap()
}

/// The editor is running: platoon's own screen is gone and stays gone (nothing of it is drawn meanwhile).
fn editor_has_the_screen(t: &mut Term, platoon_text: &[&str]) {
    t.wait_for("STAND-IN EDITOR RUNNING");
    let screen = t.stable();
    for p in platoon_text {
        assert!(!screen.contains(p), "platoon's {p:?} is still on the screen:\n{screen}");
    }
    std::thread::sleep(Duration::from_millis(1500));
    let later = t.text();
    assert_eq!(later.trim_end(), screen.trim_end(), "nothing is drawn while the editor runs");
}

#[test]
fn v_on_a_session_hands_the_terminal_to_the_editor_and_platoon_comes_back_where_it_was() {
    let lab = Lab::new("ed-term-session");
    let docs = lab.base.join("docs");
    fs::create_dir_all(&docs).unwrap();
    lab.session(&[s(&docs)]);
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 120);
    t.wait_for_all(&["make the shop", "v editor"]);
    t.send(b"v");
    editor_has_the_screen(&mut t, &["Sessions", "make the shop", "v editor"]);
    let run = lab.only_run();
    // Started as platoon's nvim is: its own settings file, in the conversation's folder.
    let argv: Vec<String> = fs::read_to_string(run.join("argv")).unwrap().lines().map(String::from).collect();
    let editor = lab.data.join("editor");
    assert_eq!(argv, vec!["-u".to_string(), s(&editor.join("config/init.lua"))]);
    assert_eq!(fs::read_to_string(run.join("cwd")).unwrap().trim(), s(&lab.work));
    assert!(run.join("stdin-is-a-terminal").is_file(), "it reads the real terminal");
    assert!(editor.join("config/init.lua").is_file() && editor.join("config/lua/platoon/tree.lua").is_file() && editor.join("config/shell").is_file());
    // Its XDG folders are inside platoon's editor folder; the user's own are kept for its terminal.
    let env = env_of(&run);
    for (k, sub) in [("XDG_CONFIG_HOME", "nvim/config"), ("XDG_DATA_HOME", "nvim/data"), ("XDG_STATE_HOME", "nvim/state"), ("XDG_CACHE_HOME", "nvim/cache")] {
        assert_eq!(env[k], s(&editor.join(sub)), "{k}");
    }
    assert_eq!(env["PLATOON_KEEP_XDG_CONFIG_HOME"], s(&lab.base.join("user-config")));
    assert!(env["PATH"].starts_with(&format!("{}:", editor.join("bin").display())), "platoon's programs first");
    assert_eq!(format!("{}:{}", editor.join("bin").display(), env["PLATOON_KEEP_PATH"]), env["PATH"]);
    assert!(!env.contains_key("PLATOON_KEEP_XDG_DATA_HOME"), "not set by the user: nothing kept");
    // The folders: the conversation's, and the one added to it.
    let session = session_of(&run);
    let roots: Vec<&str> = session["roots"].as_array().unwrap().iter().map(|r| r["path"].as_str().unwrap()).collect();
    assert_eq!(roots, vec![s(&lab.work), s(&docs)]);
    assert_eq!(session["roots"][0]["repo"], s(&lab.work).as_str());
    assert_eq!(session["colors"]["mode"], "terminal", "NO_COLOR (the harness): nothing forced");
    // Keys reach the editor, not platoon (a `q` or `2` would quit platoon or switch its tab).
    t.send(b"q2 typed\r");
    assert_eq!(wait_file(&run.join("typed")).trim(), "q2 typed");
    // Back: platoon's screen drawn in full, on the same tab, the row still selected.
    t.wait_for_all(&["make the shop", "v editor", "Sessions"]);
    let screen = t.stable();
    assert!(!screen.contains("STAND-IN EDITOR RUNNING"), "the editor's screen is gone:\n{screen}");
    assert!(!lab.data.join("editor/config").read_dir().unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with("session-")), "its session file is removed");
    assert!(lab.cache.join("editor-open").read_dir().map_or(true, |mut d| d.next().is_none()), "and the note that it had the folders open");
    // And platoon answers keys again.
    t.send(b"2");
    t.wait_for_flat("No projects yet");
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);
}

/// A ticket written straight into the project's file, with the worktree its squad made (a commit on its branch and a
/// change not yet committed), `state`, and its leader `lead-1`.
fn ticket_lab(name: &str, state: &str) -> (Lab, PathBuf, String) {
    let lab = Lab::new(name);
    lab.session(&[]);
    let base = git(&lab.work, &["rev-parse", "HEAD"]);
    let wt = lab.base.join("work-worktrees/ticket-1");
    git(&lab.work, &["worktree", "add", "-q", "-b", "ticket/1", &s(&wt), &base]);
    fs::write(wt.join("basket.txt"), "b\n").unwrap();
    git(&wt, &["add", "."]);
    git(&wt, &["commit", "-q", "-m", "basket"]);
    fs::write(wt.join("README"), "changed\n").unwrap();
    let projects = json!({"version": 1, "projects": {"pweb": {"name": "Web", "color": "blue", "badge": "WB", "archived": false, "created": 1, "archived_sessions": []}}, "sessions": {"s-1": "pweb"}});
    fs::write(lab.config.join("projects.json"), projects.to_string()).unwrap();
    let t = json!({"number": 1, "title": "Basket", "file": "01-basket.md", "state": state, "folders": [lab.work], "blocked_by": [], "created": 1, "updated": 1,
        "leaders": [{"squad": "coding", "session": "lead-1", "started": 1}],
        "worktrees": [{"folder": lab.work, "path": wt, "branch": "ticket/1", "base": base}]});
    fs::create_dir_all(lab.config.join("projects/pweb/tickets")).unwrap();
    fs::write(lab.config.join("projects/pweb/tickets/01-basket.md"), "# Basket\n").unwrap();
    fs::write(lab.config.join("projects/pweb/tickets.json"), json!({"next": 2, "tickets": [t]}).to_string()).unwrap();
    (lab, wt, base)
}

/// The project's screen, the Tickets pane in focus on #1.
fn to_tickets(t: &mut Term) {
    t.wait_for("enter open");
    t.send(b"2");
    t.wait_for_flat("WB Web");
    t.send(b"\r");
    t.wait_for_all(&["╭ Sessions", "╭ Tickets", "╭ Notes"]);
    t.wait_for("#1");
    t.send(b"\x1b[1;2B");
    t.wait_for_flat("n new ticket");
}

#[test]
fn v_on_a_tickets_screen_opens_its_worktree_counted_from_where_it_was_made_and_comes_back_to_the_ticket() {
    let (lab, wt, base) = ticket_lab("ed-term-ticket", "waiting");
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 120);
    to_tickets(&mut t);
    t.send(b"\r");
    t.wait_for_all(&["Basket", "v editor"]);
    t.send(b"v");
    editor_has_the_screen(&mut t, &["Basket", "v editor"]);
    let run = lab.only_run();
    let session = session_of(&run);
    assert_eq!(session["title"], "#1 Basket");
    let root = &session["roots"][0];
    assert_eq!((root["path"].as_str(), root["base"].as_str(), root["kind"].as_str()), (Some(s(&wt).as_str()), Some(base.as_str()), Some("worktree")));
    assert_eq!(root["repo_name"], "work");
    assert_eq!(fs::read_to_string(run.join("cwd")).unwrap().trim(), s(&wt));
    t.send(b"\r");
    // Back on the ticket's own screen.
    t.wait_for_all(&["Basket", "v editor", "esc back"]);
    assert!(!t.stable().contains("STAND-IN"));
}

#[test]
fn v_on_a_ticket_whose_squad_is_working_asks_first() {
    let (lab, wt, _) = ticket_lab("ed-term-working", "working");
    // The leader's session, "hosted": a host file for a process of the test's own (nothing to connect to, and
    // nothing here opens it).
    let mut leader = Command::new("sleep").arg("60").spawn().unwrap();
    fs::create_dir_all(lab.cache.join("hosts")).unwrap();
    let host = json!({"claude_pid": leader.id(), "host_pid": leader.id(), "dir": wt, "args": ["--resume", "lead-1"]});
    fs::write(lab.cache.join(format!("hosts/{}.json", leader.id())), host.to_string()).unwrap();
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 120);
    to_tickets(&mut t);
    t.send(b"v");
    t.wait_for_all(&["squad at work", "Open #1 Basket in the editor anyway?", "open anyway"]);
    t.wait_for_all(&["The squad is still working in these files; what you change may clash", "with it."]);
    t.send(b"n");
    t.wait_gone("anyway?");
    std::thread::sleep(Duration::from_millis(300));
    assert!(lab.runs().is_empty(), "no: nothing opened");
    t.send(b"v");
    t.wait_for("anyway?");
    t.send(b"y");
    t.wait_for("STAND-IN EDITOR RUNNING");
    t.send(b"\r");
    t.wait_for_flat("n new ticket");
    assert_eq!(lab.runs().len(), 1);
    let _ = leader.kill();
    let _ = leader.wait();
}

#[test]
fn not_installed_v_offers_the_install_and_no_says_how_to_do_it_later() {
    let lab = Lab::new("ed-term-install");
    lab.session(&[]);
    let mirror = lab.base.join("empty-mirror");
    fs::create_dir_all(&mirror).unwrap();
    // No stand-in: the real editor is asked for, and it isn't installed. Downloads come from an empty mirror.
    let env: Vec<(&str, String)> = lab.env().into_iter().filter(|(k, _)| *k != "PLATOON_NVIM_BIN").chain([("PLATOON_EDITOR_MIRROR", s(&mirror)), ("PATH", "/usr/bin:/bin".to_string())]).collect();
    let mut t = Term::spawn(&[], &env, &lab.work, 30, 120);
    t.wait_for("make the shop");
    t.send(b"v");
    t.wait_for_all(&["The code editor isn't installed. Install it now?", "not now"]);
    t.wait_for_flat("about 150 MB, a few seconds");
    t.send(b"n");
    t.wait_for_flat("No code editor, then. `platoon setup --add editor` installs it later.");
    assert!(!lab.data.join("editor/installed.json").exists());
    t.send(b"v");
    t.wait_for_flat("Install it now?");
    t.send(b"y");
    t.wait_for_flat("didn't finish installing");
    assert!(lab.runs().is_empty());
    let setup: Value = serde_json::from_str(&fs::read_to_string(lab.config.join("setup.json")).unwrap()).unwrap();
    assert_eq!(setup["editor"]["wanted"], true, "asked for with v: wanted, like `platoon setup --add editor`");
}

#[test]
fn v_typed_into_a_box_is_a_letter() {
    let lab = Lab::new("ed-term-box");
    lab.session(&[]);
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 120);
    t.wait_for("make the shop");
    // The search box.
    t.send(b"/");
    t.wait_for("Quick filter");
    t.send(b"vv");
    t.wait_for("vv");
    t.send(b"\x1b");
    t.wait_gone("Quick filter");
    // The new session's folder picker.
    t.send(b"n");
    t.wait_for("pick a directory");
    t.send(b"v");
    t.wait_for("❯ v");
    std::thread::sleep(Duration::from_millis(300));
    assert!(lab.runs().is_empty(), "the editor never started");
}
