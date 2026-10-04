//! The code editor (`v`) end to end, in a real terminal with the real binary and stand-ins for the editors this
//! computer has, in a folder `PLATOON_EDITOR_PATH` makes the only place looked in: a Helix that writes down what it
//! was started with (arguments, folder, environment) and waits for a line typed on its terminal, and a VS Code that
//! writes down its arguments and ends at once. Checked: platoon's screen goes while a terminal editor runs and nothing
//! of platoon draws over it, the keys reach it, and platoon comes back drawn in full where the user was; the folders of
//! a session and of a ticket; the question while the squad is working; the list of editors; a window editor beside
//! platoon; the settings; no editor found; `v` typed into a box stays a letter.
//!
//! Every directory is a temp dir; no real editor, no real `claude`.

mod common;
use common::*;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

/// Stands in for an editor with a window of its own: writes down how it was started into `$STANDIN_OUT/<its name>-*`
/// and ends at once, as a launcher handing over to a running copy does.
const WINDOW_STAND_IN: &str = r#"#!/bin/sh
n=$(basename "$0")
printf '%s\n' "$@" > "$STANDIN_OUT/$n.argv.tmp"
pwd > "$STANDIN_OUT/$n.cwd"
if [ -t 0 ]; then echo yes > "$STANDIN_OUT/$n.tty"; fi
mv "$STANDIN_OUT/$n.argv.tmp" "$STANDIN_OUT/$n.argv"
"#;

/// Stands in for Helix: writes down how it was started into a folder of its own under `$STANDIN_OUT`, shows a line,
/// waits for a line typed on its terminal, and writes that down too.
const HX_STAND_IN: &str = r#"#!/bin/sh
d="$STANDIN_OUT/hx-$$"
mkdir -p "$d"
printf '%s\n' "$@" > "$d/argv"
pwd > "$d/cwd"
env > "$d/env"
if [ -t 0 ]; then echo yes > "$d/stdin-is-a-terminal"; fi
printf 'STAND-IN HELIX RUNNING\n'
IFS= read -r line
printf '%s\n' "$line" > "$d/typed"
"#;

struct Lab {
    base: PathBuf,
    claude: PathBuf,
    cache: PathBuf,
    config: PathBuf,
    work: PathBuf,
    out: PathBuf,
    /// The only place editors are looked for (`PLATOON_EDITOR_PATH`): Helix alone, unless a test puts more there.
    editors: PathBuf,
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
            out: base.join("out"),
            editors: base.join("editors"),
            base,
        };
        for d in [&lab.claude.join("sessions"), &lab.claude.join("projects/-work"), &lab.cache, &lab.config, &lab.work, &lab.out] {
            fs::create_dir_all(d).unwrap();
        }
        lab.put_editor("hx", HX_STAND_IN);
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
            ("PLATOON_SKIP_SETUP_CHECK", "1".into()),
            ("PLATOON_NOTES_HELPER", "off".into()),
            ("PLATOON_EDITOR_PATH", s(&self.editors)),
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

    /// A stand-in editor called `name` in the only place editors are looked for.
    fn put_editor(&self, name: &str, text: &str) {
        fs::create_dir_all(&self.editors).unwrap();
        fs::write(self.editors.join(name), text).unwrap();
        fs::set_permissions(self.editors.join(name), fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// The Helix stand-in's runs so far.
    fn runs(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = fs::read_dir(&self.out)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("hx-"))
            .collect();
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
    fs::read_to_string(run.join("env"))
        .unwrap()
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Waits for a file the stand-in writes.
fn wait_file(p: &Path) -> String {
    until(&format!("{} to be written", p.display()), || p.is_file());
    fs::read_to_string(p).unwrap()
}

/// The editor is running: platoon's own screen is gone and stays gone (nothing of it is drawn meanwhile).
fn editor_has_the_screen(t: &mut Term, platoon_text: &[&str]) {
    t.wait_for("STAND-IN HELIX RUNNING");
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
    // Wide: the notice at the end names two folders by their full paths, and a long temp folder (macOS's) would
    // otherwise push "Not opened:" past where a notice is cut to fit the footer.
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 220);
    t.wait_for_all(&["make the shop", "? help"]);
    // None chosen yet: the only editor found (Helix), on the conversation's folder (it opens one at a time).
    t.send(b"v");
    editor_has_the_screen(&mut t, &["Sessions", "make the shop", "? help"]);
    let run = lab.only_run();
    assert_eq!(fs::read_to_string(run.join("argv")).unwrap().lines().collect::<Vec<_>>(), vec![s(&lab.work)]);
    assert_eq!(fs::read_to_string(run.join("cwd")).unwrap().trim(), s(&lab.work));
    assert!(run.join("stdin-is-a-terminal").is_file(), "it reads the real terminal");
    // The user's own environment, as it is (only Claude Code's marks taken out).
    let env = env_of(&run);
    assert_eq!(env["XDG_CONFIG_HOME"], s(&lab.base.join("user-config")));
    assert!(!env.keys().any(|k| k.starts_with("PLATOON_KEEP_") || k.starts_with("PLATOON_EDITOR_SESSION")), "{env:?}");
    assert!(!env.contains_key("CLAUDECODE"));
    assert!(lab.cache.join("editor-open").read_dir().unwrap().flatten().count() == 1, "its folder is marked open meanwhile");
    // Keys reach the editor, not platoon (a `q` or `2` would quit platoon or switch its tab).
    t.send(b"q2 typed\r");
    assert_eq!(wait_file(&run.join("typed")).trim(), "q2 typed");
    // Back: platoon's screen drawn in full, on the same tab, the row still selected, and what Helix didn't open said
    // (in the footer, for a while).
    t.wait_for_all(&["make the shop", "Sessions"]);
    t.wait_for_flat("only: it opens one folder at a time. Not opened:");
    let screen = t.stable();
    assert!(!screen.contains("STAND-IN HELIX RUNNING"), "the editor's screen is gone:\n{screen}");
    assert!(lab.cache.join("editor-open").read_dir().unwrap().next().is_none(), "and the note that it had the folder open");
    // And platoon answers keys again.
    t.send(b"3");
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
    fs::create_dir_all(lab.config.join("projects/pweb/tasks")).unwrap();
    fs::write(lab.config.join("projects/pweb/tasks/01-basket.md"), "# Basket\n").unwrap();
    fs::write(lab.config.join("projects/pweb/tasks.json"), json!({"next": 2, "tickets": [t]}).to_string()).unwrap();
    (lab, wt, base)
}

/// The project's screen, the Tickets pane in focus on #1.
fn to_tickets(t: &mut Term) {
    t.wait_for("enter open");
    t.send(b"3");
    t.wait_for_flat("WB Web");
    t.send(b"\r");
    t.wait_for_all(&["╭ Sessions", "╭ Tasks", "╭ Notes"]);
    t.wait_for("#1");
    t.send(b"\x1b[1;2B");
    t.wait_for_flat("n new task");
}

#[test]
fn v_on_a_tickets_screen_opens_its_worktree_and_comes_back_to_the_ticket() {
    let (lab, wt, _) = ticket_lab("ed-term-ticket", "waiting");
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 120);
    to_tickets(&mut t);
    t.send(b"\r");
    t.wait_for_all(&["Basket", "v editor"]);
    t.send(b"v");
    editor_has_the_screen(&mut t, &["Basket", "v editor"]);
    let run = lab.only_run();
    assert_eq!(fs::read_to_string(run.join("argv")).unwrap().lines().collect::<Vec<_>>(), vec![s(&wt)]);
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
    t.wait_for_all(&["Squad at work", "Open #1 Basket in the editor anyway?", "open anyway"]);
    t.wait_for_all(&["The squad is still working in these files; what you change may clash", "with it."]);
    t.send(b"n");
    t.wait_gone("anyway?");
    std::thread::sleep(Duration::from_millis(300));
    assert!(lab.runs().is_empty(), "no: nothing opened");
    t.send(b"v");
    t.wait_for("anyway?");
    t.send(b"y");
    t.wait_for("STAND-IN HELIX RUNNING");
    t.send(b"\r");
    t.wait_for_flat("n new task");
    assert_eq!(lab.runs().len(), 1);
    let _ = leader.kill();
    let _ = leader.wait();
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
    t.wait_for("pick a folder");
    t.send(b"v");
    t.wait_for("❯ v");
    std::thread::sleep(Duration::from_millis(300));
    assert!(lab.runs().is_empty(), "the editor never started");
}

// ---- more editors ------------------------------------------------------------------------------------------------

impl Lab {
    /// VS Code too (a window stand-in) beside Helix, and the settings `extra` merged into the settings file.
    fn own_editors(&self, extra: Value) -> Vec<(&'static str, String)> {
        self.put_editor("code", WINDOW_STAND_IN);
        let mut settings: Value = serde_json::from_str(&fs::read_to_string(self.config.join("settings.json")).unwrap()).unwrap();
        for (k, v) in extra.as_object().unwrap() {
            settings.as_object_mut().unwrap().insert(k.clone(), v.clone());
        }
        fs::write(self.config.join("settings.json"), settings.to_string()).unwrap();
        self.env()
    }

    fn settings(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.config.join("settings.json")).unwrap()).unwrap()
    }
}

#[test]
fn always_ask_lists_the_editors_with_the_favourite_starred_and_a_window_editor_starts_beside_platoon() {
    let lab = Lab::new("ed-term-list");
    let docs = lab.base.join("docs");
    fs::create_dir_all(&docs).unwrap();
    lab.session(&[s(&docs)]);
    let mut env = lab.own_editors(json!({"files_editor_ask": true, "files_editor": "code"}));
    // Colours as a terminal without NO_COLOR has them.
    env.push(("NO_COLOR", String::new()));
    let mut t = Term::spawn(&[], &env, &lab.work, 30, 120);
    t.wait_for_all(&["make the shop", "? help"]);
    t.send(b"v");
    t.wait_for_all(&["╭ Open in editor", "★ VS Code", "Helix", "Choosing another makes it the favourite"]);
    let shown = t.stable();
    let rows: Vec<&str> = shown.lines().filter(|l| l.contains("VS Code") || l.contains("Helix")).collect();
    assert_eq!(rows.len(), 2, "{shown}");
    assert!(rows[0].contains("VS Code") && rows[1].contains("Helix"), "in the order found:\n{shown}");
    assert!(rows[0].contains("★") && !rows[1].contains("★"), "only the favourite is starred:\n{shown}");
    assert!(
        rows[0].contains("▎") && rows[0].contains("in its own window") && rows[1].contains("here in platoon's terminal"),
        "and picked:\n{shown}"
    );
    // The star is drawn in the theme's accent (Catppuccin Mocha lavender).
    let (row, col) = t.find("★ VS Code").unwrap();
    assert_eq!(t.fg_at(row, col), vt100::Color::Rgb(0xb4, 0xbe, 0xfe));
    // Enter: VS Code (a stand-in) starts beside platoon with both folders in one window; platoon stays on screen.
    t.send(b"\r");
    t.wait_gone("╭ Open in editor");
    let argv = wait_file(&lab.out.join("code.argv"));
    assert_eq!(argv.lines().collect::<Vec<_>>(), vec!["--new-window".to_string(), s(&lab.work), s(&docs)]);
    assert_eq!(fs::read_to_string(lab.out.join("code.cwd")).unwrap().trim(), s(&lab.work));
    assert!(!lab.out.join("code.tty").exists(), "it doesn't get platoon's terminal");
    t.wait_for_all(&["make the shop", "Opened in VS Code, in a window of its own."]);
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);
}

#[test]
fn a_tickets_worktree_opened_in_a_window_editor_is_kept_on_its_screen_until_w() {
    let (lab, wt, _) = ticket_lab("ed-term-kept", "done");
    let env = lab.own_editors(json!({"files_editor": "code"}));
    let mut t = Term::spawn(&[], &env, &lab.work, 30, 130);
    t.wait_for("enter open");
    t.send(b"3");
    t.wait_for_flat("WB Web");
    t.send(b"\r");
    t.wait_for_all(&["╭ Sessions", "╭ Tasks", "╭ Notes"]);
    // A Done ticket is under the folded "Done and cancelled" group: into the Tickets pane, open the group, on #1.
    t.send(b"\x1b[1;2B");
    t.wait_for_flat("n new task");
    t.send(b"\r");
    t.wait_for("#1");
    t.send(b"\x1b[B\r");
    t.wait_for_all(&["Basket", "v editor", "esc back"]);
    t.send(b"v");
    assert_eq!(wait_file(&lab.out.join("code.argv")).lines().collect::<Vec<_>>(), vec!["--new-window".to_string(), s(&wt)]);
    t.wait_for_flat("Kept: opened in VS Code.");
    t.wait_for_flat("w removes it");
    // Longer than the clean-up's interval: still there.
    std::thread::sleep(Duration::from_millis(3000));
    assert!(wt.is_dir());
    t.send(b"w");
    t.wait_for_flat("Remove #1's copy of the folder anyway?");
    t.wait_for_flat("It was opened in VS Code: close it there first.");
    t.send(b"y");
    t.wait_for_flat("#1's worktree is removed. Its branch ticket/1 is kept.");
    assert!(!wt.exists());
    let tickets: Value = serde_json::from_str(&fs::read_to_string(lab.config.join("projects/pweb/tasks.json")).unwrap()).unwrap();
    assert!(tickets["tickets"][0]["worktrees"][0].get("opened_in").is_none());
}

#[test]
fn the_settings_show_the_editors_in_two_groups_and_step_through_the_ones_found() {
    let lab = Lab::new("ed-term-settings");
    lab.session(&[]);
    let env = lab.own_editors(json!({}));
    let mut t = Term::spawn(&[], &env, &lab.work, 40, 110);
    t.wait_for("make the shop");
    t.send(b"s");
    t.wait_for("Stop idle sessions after");
    t.send(b"\x1b[Z"); // shift+tab: from the first tab to the last, Editors
    t.wait_for_all(&[
        "Editing platoon's text",
        "Editor for notes and tasks",
        "Editing files",
        "Editor for folders (v)",
        "Always ask which editor",
    ]);
    let shown = t.stable();
    let at = |needle: &str| shown.lines().position(|l| l.contains(needle)).unwrap_or_else(|| panic!("{needle}:\n{shown}"));
    assert!(at("Editing platoon's text") < at("Editor for notes and tasks"));
    assert!(at("Editor for notes and tasks") < at("Editing files") && at("Editing files") < at("Editor for folders (v)"));
    assert!(at("Editor for folders (v)") < at("Always ask which editor"));
    let files_row = shown.lines().nth(at("Editor for folders (v)")).unwrap();
    assert!(files_row.contains("VS Code"), "none chosen yet: the first one found, which v opens: {files_row}");
    // Down to the files editor, right: the next editor found here, saved at once.
    t.send(b"\x1b[B\x1b[C");
    t.wait_for_all(&["Editor for folders (v)", "Helix"]);
    until("the choice to reach the settings file", || lab.settings()["files_editor"] == "helix");
    t.send(b"\x1b[B\r");
    until("always ask on", || lab.settings()["files_editor_ask"] == true);
    t.send(b"\x1b");
    t.wait_gone("Editing files");
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);
}

/// No editor on this computer: `v` says so in one plain line, and nothing starts.
#[test]
fn v_with_no_editor_found_says_so() {
    let lab = Lab::new("ed-term-none");
    lab.session(&[]);
    fs::remove_file(lab.editors.join("hx")).unwrap();
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 120);
    t.wait_for_all(&["make the shop", "? help"]);
    t.send(b"v");
    t.wait_for_flat("No editor found: install VS Code, Zed, Helix or a JetBrains IDE.");
    let screen = t.stable();
    assert!(screen.contains("make the shop") && !screen.contains("open with"), "platoon stays as it was:\n{screen}");
    assert!(lab.runs().is_empty() && fs::read_dir(&lab.out).unwrap().next().is_none(), "nothing started");
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);
}
