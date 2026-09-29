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
    assert!(editor.join("config/init.lua").is_file() && editor.join("config/lua/platoon/tree.lua").is_file() && editor.join("config/shell.sh").is_file());
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
    // Copies go where platoon's go: PLATOON_CLIPBOARD=osc52 (the harness) means OSC 52 only, no system helper.
    assert_eq!(session["clipboard"], json!({"target": "osc52", "helper": null, "osc52_max": 100000}));
    // The back key, in Neovim's notation (no keys.json: the default).
    assert_eq!(session["keys"], json!({"back": "<C-Space>"}));
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
    t.wait_for_all(&["Basket", "v editor", "ctrl+space back"]);
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

fn sha256(p: &Path) -> String {
    let out = Command::new("sha256sum").arg(p).output().unwrap();
    String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap().to_string()
}

/// `<mirror>/<name>`: a tar.gz of the folder `top` holding `files` (path, text; runnable when it starts with `#!`).
fn pack(mirror: &Path, name: &str, top: &str, files: &[(&str, &str)]) -> Value {
    let stage = mirror.join(format!(".stage-{name}"));
    for (p, text) in files {
        let path = stage.join(top).join(p);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        if text.starts_with("#!") {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    let out = mirror.join(name);
    // An empty `top`: the files themselves at the top of the archive (as a font release has them).
    let (from, what): (PathBuf, Vec<String>) = if top.is_empty() { (stage.clone(), files.iter().map(|(p, _)| p.to_string()).collect()) } else { (stage.clone(), vec![top.to_string()]) };
    assert!(Command::new("tar").arg("-czf").arg(&out).arg("-C").arg(&from).args(&what).status().unwrap().success());
    fs::remove_dir_all(&stage).unwrap();
    json!({"url": format!("https://example.invalid/{name}"), "sha256": sha256(&out), "size": fs::metadata(&out).unwrap().len()})
}

/// A plugin catalog (a folder standing in for the one built into platoon: `PLATOON_PLUGIN_CATALOG`) whose code
/// editor plugin is platoon's own (`plugins/editor`: its settings files, its version) with a download table of tiny
/// fake downloads in a mirror: its "Neovim" is the stand-in (so after the install platoon runs the stand-in as the
/// editor's own nvim), one Neovim plugin, a font. Never the network.
fn editor_catalog(lab: &Lab) -> (PathBuf, PathBuf) {
    editor_catalog_with(lab, None, &[])
}

/// The same at `version` (None: the plugin's own), with more fake Neovim plugins (`extra`, by name). A download
/// already in the mirror is kept as it is (the same file, the same checksum), so a later table changes only what it adds.
fn editor_catalog_with(lab: &Lab, version: Option<&str>, extra: &[&str]) -> (PathBuf, PathBuf) {
    let mirror = lab.base.join("mirror");
    fs::create_dir_all(&mirror).unwrap();
    let packed = |name: &str, top: &str, files: &[(&str, &str)]| {
        let out = mirror.join(name);
        if out.is_file() {
            json!({"url": format!("https://example.invalid/{name}"), "sha256": sha256(&out), "size": fs::metadata(&out).unwrap().len()})
        } else {
            pack(&mirror, name, top, files)
        }
    };
    let nvim = packed("nvim-fake.tar.gz", "nvim-dist", &[("bin/nvim", STAND_IN)]);
    let font = packed("Font.tar.xz", "", &[("FakeNerdFont-Regular.ttf", "font"), ("OFL.txt", "licence")]);
    let downloads: serde_json::Map<String, Value> = ["linux-x64", "linux-arm64", "macos-arm64", "macos-x64"].iter().map(|k| (k.to_string(), nvim.clone())).collect();
    let mut pins = Vec::new();
    for name in std::iter::once("alpha.nvim").chain(extra.iter().copied()) {
        let short = name.trim_end_matches(".nvim");
        let got = packed(&format!("rev-{short}"), &format!("{short}-rev"), &[(&format!("lua/{short}.lua"), "return 1")]);
        let mut pin = json!({"name": name, "repo": format!("o/{short}"), "rev": format!("rev-{short}")});
        pin.as_object_mut().unwrap().extend(got.as_object().unwrap().clone());
        pins.push(pin);
    }
    let mut font_pin = json!({"family": "Fake Nerd Font", "version": "1", "files": ["FakeNerdFont-Regular.ttf", "OFL.txt"]});
    font_pin.as_object_mut().unwrap().extend(font.as_object().unwrap().clone());
    // Never the machine's own nvim, whatever it has: only a version no nvim has would be taken.
    let table = json!({"format": 1, "tools": [{"id": "nvim", "version": "1.0.0", "min_yours": "999.0.0", "names": ["nvim"], "exe": "bin/nvim", "downloads": downloads}],
        "plugins": pins, "parsers": [], "font": font_pin});
    let cat = lab.base.join("catalog");
    let ours = Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/editor");
    let theirs = cat.join("editor");
    for rel in ["plugin.json", "config/init.lua", "config/lua/platoon/tree.lua", "config/shell.sh"] {
        fs::create_dir_all(theirs.join(rel).parent().unwrap()).unwrap();
        fs::copy(ours.join(rel), theirs.join(rel)).unwrap();
    }
    fs::write(theirs.join("downloads.json"), serde_json::to_string_pretty(&table).unwrap()).unwrap();
    // The index names the version the plugin's own file has (a catalog whose two disagree is refused).
    let mut manifest: Value = serde_json::from_str(&fs::read_to_string(ours.join("plugin.json")).unwrap()).unwrap();
    if let Some(v) = version {
        manifest.as_object_mut().unwrap().insert("version".into(), json!(v));
        fs::write(theirs.join("plugin.json"), serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
    }
    fs::write(cat.join("index.json"), json!({"plugins": [{"id": "editor", "version": manifest["version"].clone()}]}).to_string()).unwrap();
    (cat, mirror)
}

#[test]
fn not_installed_v_offers_the_plugin_and_yes_installs_it_and_its_downloads_then_opens_it() {
    let lab = Lab::new("ed-term-install");
    lab.session(&[]);
    let (cat, mirror) = editor_catalog(&lab);
    // No stand-in named: the editor's own nvim is asked for, and it isn't installed. The catalog and the downloads
    // are the lab's; the "Neovim" it installs is the stand-in, allowed to run (an empty PLATOON_FORBID_REAL_EDITOR).
    let env: Vec<(&str, String)> = lab
        .env()
        .into_iter()
        .filter(|(k, _)| *k != "PLATOON_NVIM_BIN")
        .chain([
            ("PLATOON_EDITOR_MIRROR", s(&mirror)),
            ("PLATOON_PLUGIN_CATALOG", s(&cat)),
            ("PLATOON_FONT_DIR", s(&lab.base.join("fonts"))),
            ("PLATOON_FORBID_REAL_EDITOR", String::new()),
            ("PATH", "/usr/bin:/bin".to_string()),
        ])
        .collect();
    let mut t = Term::spawn(&[], &env, &lab.work, 30, 120);
    t.wait_for("make the shop");
    t.send(b"v");
    t.wait_for_all(&["platoon's code editor isn't installed. Install its plugin now?", "not now"]);
    // (What it downloads: the catalog's figure.)
    t.wait_for("It downloads about");
    t.send(b"n");
    t.wait_for_flat("No code editor, then. The settings (s, then Plugins) or `platoon plugin add editor` installs it later.");
    t.wait_gone("Install its plugin now?");
    assert!(!lab.data.join("editor/installed.json").exists() && !lab.data.join("plugins/editor").exists());
    t.send(b"v");
    t.wait_for_flat("Install its plugin now?");
    t.send(b"y");
    // Installed from the catalog, then its downloads, then it opens.
    until("the editor to start", || !lab.runs().is_empty());
    let run = lab.only_run();
    wait_file(&run.join("argv"));
    let argv = fs::read_to_string(run.join("argv")).unwrap();
    assert!(argv.contains(&s(&lab.data.join("editor/config/init.lua"))), "{argv}");
    assert!(lab.data.join("plugins/editor/plugin.json").is_file() && lab.data.join("editor/installed.json").is_file());
    assert!(lab.data.join("editor/site/pack/platoon/start/alpha.nvim/lua/alpha.lua").is_file());
    assert!(lab.base.join("fonts/FakeNerdFont-Regular.ttf").is_file());
    let init = fs::read_to_string(lab.data.join("editor/config/init.lua")).unwrap();
    assert!(init.contains("from its editor plugin"), "the plugin's settings file");
    t.send(b"done\r");
    wait_file(&run.join("typed"));
    t.wait_for("make the shop");
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

/// The helper the editor runs for each copy says "not copied" (exit 1) at once when there is nothing to copy or no
/// display to copy to, and never waits. (Run with an empty environment: no display, so the real clipboard can't be
/// reached.)
#[test]
fn the_clipboard_helper_gives_up_at_once_without_text_or_a_display() {
    use std::io::Write;
    for text in ["", "some text"] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_platoon"))
            .arg("clipboard-hold")
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(st) = child.try_wait().unwrap() {
                break st;
            }
            if std::time::Instant::now() > until {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the helper kept waiting with {text:?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(status.code(), Some(1), "{text:?}");
    }
}

// ---- the machine's own editors ------------------------------------------------------------------------------------

/// Stands in for an editor with a window of its own: writes down how it was started into `$STANDIN_OUT/<its name>-*`
/// and ends at once, as a launcher handing over to a running copy does.
const WINDOW_STAND_IN: &str = r#"#!/bin/sh
n=$(basename "$0")
printf '%s\n' "$@" > "$STANDIN_OUT/$n.argv.tmp"
pwd > "$STANDIN_OUT/$n.cwd"
if [ -t 0 ]; then echo yes > "$STANDIN_OUT/$n.tty"; fi
mv "$STANDIN_OUT/$n.argv.tmp" "$STANDIN_OUT/$n.argv"
"#;

/// Stands in for Helix: writes down how it was started, shows a line, waits for a line typed on its terminal.
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

impl Lab {
    /// A folder of stand-in editors (`code`, `hx`) that `PLATOON_EDITOR_PATH` makes the only place looked in, and
    /// the settings `extra` merged into the settings file.
    fn own_editors(&self, extra: Value) -> Vec<(&'static str, String)> {
        let bin = self.base.join("editors");
        fs::create_dir_all(&bin).unwrap();
        for (name, text) in [("code", WINDOW_STAND_IN), ("hx", HX_STAND_IN)] {
            fs::write(bin.join(name), text).unwrap();
            fs::set_permissions(bin.join(name), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut settings: Value = serde_json::from_str(&fs::read_to_string(self.config.join("settings.json")).unwrap()).unwrap();
        for (k, v) in extra.as_object().unwrap() {
            settings.as_object_mut().unwrap().insert(k.clone(), v.clone());
        }
        fs::write(self.config.join("settings.json"), settings.to_string()).unwrap();
        let mut env = self.env();
        env.push(("PLATOON_EDITOR_PATH", s(&bin)));
        env
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
    t.wait_for_all(&["make the shop", "v editor"]);
    t.send(b"v");
    t.wait_for_all(&["╭ open with", "platoon's editor", "★ VS Code", "Helix", "Choosing another makes it the favourite"]);
    let shown = t.stable();
    let rows: Vec<&str> = shown.lines().filter(|l| l.contains("platoon's editor") || l.contains("VS Code") || l.contains("Helix")).collect();
    assert_eq!(rows.len(), 3, "{shown}");
    assert!(rows[0].contains("platoon's editor") && rows[1].contains("VS Code") && rows[2].contains("Helix"), "platoon's own first:\n{shown}");
    assert!(rows[1].contains("★") && !rows[0].contains("★") && !rows[2].contains("★"), "only the favourite is starred:\n{shown}");
    assert!(rows[1].contains("▎") && rows[1].contains("in its own window") && rows[2].contains("here in platoon's terminal"), "and picked:\n{shown}");
    // The star is drawn in the theme's accent (Catppuccin Mocha lavender).
    let (row, col) = t.find("★ VS Code").unwrap();
    assert_eq!(t.fg_at(row, col), vt100::Color::Rgb(0xb4, 0xbe, 0xfe));
    // Enter: VS Code (a stand-in) starts beside platoon with both folders in one window; platoon stays on screen.
    t.send(b"\r");
    t.wait_gone("╭ open with");
    let argv = wait_file(&lab.out.join("code.argv"));
    assert_eq!(argv.lines().collect::<Vec<_>>(), vec!["--new-window".to_string(), s(&lab.work), s(&docs)]);
    assert_eq!(fs::read_to_string(lab.out.join("code.cwd")).unwrap().trim(), s(&lab.work));
    assert!(!lab.out.join("code.tty").exists(), "it doesn't get platoon's terminal");
    t.wait_for_all(&["make the shop", "Opened in VS Code, in a window of its own."]);
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);
}

#[test]
fn a_terminal_editor_of_yours_is_handed_the_terminal_on_the_first_folder_and_platoon_comes_back() {
    let lab = Lab::new("ed-term-hx");
    let docs = lab.base.join("docs");
    fs::create_dir_all(&docs).unwrap();
    lab.session(&[s(&docs)]);
    let env = lab.own_editors(json!({"files_editor": "helix"}));
    let mut t = Term::spawn(&[], &env, &lab.work, 30, 120);
    t.wait_for_all(&["make the shop", "v editor"]);
    t.send(b"v");
    t.wait_for("STAND-IN HELIX RUNNING");
    let screen = t.stable();
    assert!(!screen.contains("make the shop") && !screen.contains("v editor"), "platoon's screen is gone:\n{screen}");
    let run = fs::read_dir(&lab.out).unwrap().flatten().map(|e| e.path()).find(|p| p.file_name().unwrap().to_string_lossy().starts_with("hx-")).unwrap();
    assert_eq!(fs::read_to_string(run.join("argv")).unwrap().lines().collect::<Vec<_>>(), vec![s(&lab.work)]);
    assert_eq!(fs::read_to_string(run.join("cwd")).unwrap().trim(), s(&lab.work));
    assert!(run.join("stdin-is-a-terminal").is_file());
    // The user's own environment, not platoon's editor's world.
    let env = env_of(&run);
    assert_eq!(env["XDG_CONFIG_HOME"], s(&lab.base.join("user-config")));
    assert!(!env.contains_key("PLATOON_EDITOR_SESSION") && !env.contains_key("PLATOON_EDITOR_ENV"));
    assert!(lab.cache.join("editor-open").read_dir().unwrap().flatten().count() == 1, "its folder is marked open meanwhile");
    // Keys reach it; then platoon is back, drawn in full, and says what Helix didn't open.
    t.send(b"q2 typed\r");
    assert_eq!(wait_file(&run.join("typed")).trim(), "q2 typed");
    t.wait_for_all(&["make the shop", "Sessions"]);
    t.wait_for_flat("only: it opens one folder at a time. Not opened:");
    assert!(!t.stable().contains("STAND-IN HELIX"));
    assert!(lab.cache.join("editor-open").read_dir().unwrap().next().is_none(), "the mark is gone");
    assert!(lab.runs().iter().all(|r| !r.file_name().unwrap().to_string_lossy().starts_with("run-")), "platoon's own editor never started");
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);
}

#[test]
fn a_tickets_worktree_opened_in_a_window_editor_is_kept_on_its_screen_until_w() {
    let (lab, wt, _) = ticket_lab("ed-term-kept", "done");
    let env = lab.own_editors(json!({"files_editor": "code"}));
    let mut t = Term::spawn(&[], &env, &lab.work, 30, 130);
    t.wait_for("enter open");
    t.send(b"2");
    t.wait_for_flat("WB Web");
    t.send(b"\r");
    t.wait_for_all(&["╭ Sessions", "╭ Tickets", "╭ Notes"]);
    // A Done ticket is under the folded "Done and cancelled" group: into the Tickets pane, open the group, on #1.
    t.send(b"\x1b[1;2B");
    t.wait_for_flat("n new ticket");
    t.send(b"\r");
    t.wait_for("#1");
    t.send(b"\x1b[B\r");
    t.wait_for_all(&["Basket", "v editor", "ctrl+space back"]);
    t.send(b"v");
    assert_eq!(wait_file(&lab.out.join("code.argv")).lines().collect::<Vec<_>>(), vec!["--new-window".to_string(), s(&wt)]);
    t.wait_for_flat("Kept: opened in VS Code.");
    t.wait_for_flat("w removes it");
    // Longer than the clean-up's interval: still there.
    std::thread::sleep(Duration::from_millis(3000));
    assert!(wt.is_dir());
    t.send(b"w");
    t.wait_for_flat("Remove #1's worktree anyway?");
    t.wait_for_flat("It was opened in VS Code: close it there first.");
    t.send(b"y");
    t.wait_for_flat("#1's worktree is removed. Its branch ticket/1 is kept.");
    assert!(!wt.exists());
    let tickets: Value = serde_json::from_str(&fs::read_to_string(lab.config.join("projects/pweb/tickets.json")).unwrap()).unwrap();
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
    t.send(b"\x1b[Z"); // shift+tab: from the first tab to the last (Plugins), then Editors before it
    t.wait_for("No plugins installed, and none from the catalog to show.");
    t.send(b"\x1b[Z");
    t.wait_for_all(&["Editing platoon's text", "Editor for notes and tickets", "Editing files", "Editor for files (v)", "Always ask which editor"]);
    let shown = t.stable();
    let at = |needle: &str| shown.lines().position(|l| l.contains(needle)).unwrap_or_else(|| panic!("{needle}:\n{shown}"));
    assert!(at("Editing platoon's text") < at("Editor for notes and tickets"));
    assert!(at("Editor for notes and tickets") < at("Editing files") && at("Editing files") < at("Editor for files (v)"));
    assert!(at("Editor for files (v)") < at("Always ask which editor"));
    let files_row = shown.lines().nth(at("Editor for files (v)")).unwrap();
    assert!(files_row.contains("platoon's editor"), "the default: {files_row}");
    // Down to the files editor, right: the next editor found here, saved at once.
    t.send(b"\x1b[B\x1b[C");
    t.wait_for_all(&["Editor for files (v)", "VS Code"]);
    until("the choice to reach the settings file", || lab.settings()["files_editor"] == "code");
    t.send(b"\x1b[B\r");
    until("always ask on", || lab.settings()["files_editor_ask"] == true);
    t.send(b"\x1b");
    t.wait_gone("Editing files");
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);
}

#[test]
fn the_editor_is_given_the_back_key_from_keys_json() {
    let lab = Lab::new("ed-term-keys");
    lab.session(&[]);
    fs::write(lab.config.join("keys.json"), r#"{ "back": "alt+x" }"#).unwrap();
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 30, 120);
    t.wait_for_all(&["make the shop", "v editor"]);
    t.send(b"v");
    t.wait_for("STAND-IN EDITOR RUNNING");
    let run = lab.only_run();
    until("the session file to be copied", || run.join("session.json").is_file());
    assert_eq!(session_of(&run)["keys"], json!({"back": "<M-x>"}));
    t.send(b"done\r");
    t.wait_for_all(&["make the shop", "v editor"]);
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);
}

#[test]
fn a_newer_editor_plugin_is_updated_when_platoon_starts_and_fetches_only_what_changed() {
    let lab = Lab::new("ed-term-auto");
    lab.session(&[]);
    let (cat, mirror) = editor_catalog_with(&lab, Some("1.0.0"), &[]);
    let env: Vec<(&str, String)> = lab
        .env()
        .into_iter()
        .chain([
            ("PLATOON_EDITOR_MIRROR", s(&mirror)),
            ("PLATOON_PLUGIN_CATALOG", s(&cat)),
            ("PLATOON_FONT_DIR", s(&lab.base.join("fonts"))),
            ("PLATOON_SKIP_FC_CACHE", "1".into()),
            ("PATH", "/usr/bin:/bin".to_string()),
        ])
        .collect();
    let cli = |args: &[&str]| {
        let out = Command::new(BIN).args(args).envs(env.iter().map(|(k, v)| (*k, v.as_str()))).current_dir(&lab.base).stdin(Stdio::null()).output().unwrap();
        (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
    };
    let (code, out, err) = cli(&["plugin", "add", "editor"]);
    assert_eq!(code, 0, "{out}{err}");
    let site = lab.data.join("editor/site/pack/platoon/start");
    assert!(site.join("alpha.nvim/lua/alpha.lua").is_file());
    // A newer platoon: the editor plugin 1.1.0 with one more Neovim plugin. What didn't change leaves the mirror, so
    // fetching it again would fail: only the new one can be fetched.
    editor_catalog_with(&lab, Some("1.1.0"), &["beta.nvim"]);
    for name in ["nvim-fake.tar.gz", "rev-alpha", "Font.tar.xz"] {
        fs::remove_file(mirror.join(name)).unwrap();
    }
    let mut t = Term::spawn(&[], &env, &lab.work, 30, 120);
    t.wait_for_flat("Updated with platoon: Code editor (Neovim) 1.1.0.");
    assert!(site.join("beta.nvim/lua/beta.lua").is_file() && site.join("alpha.nvim/lua/alpha.lua").is_file());
    let installed: Value = serde_json::from_str(&fs::read_to_string(lab.data.join("plugins/installed.json")).unwrap()).unwrap();
    assert_eq!(installed["plugins"]["editor"]["version"], "1.1.0");
    let shown = t.stable();
    assert!(!shown.contains("didn't finish"), "{shown}");
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);
    // Started again: nothing more to do, nothing said.
    let mut t = Term::spawn(&[], &env, &lab.work, 30, 120);
    t.wait_for("make the shop");
    assert!(!t.stable().contains("Updated with platoon"));
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);
}
