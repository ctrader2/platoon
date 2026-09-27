//! Plugins end to end with the real binary: the Plugins tab (`4`) drawn from a catalog fetched with git from a temp
//! repository (`PLATOON_PLUGIN_SOURCE`, never the network), installing one with the footer line, the settings'
//! Plugins tab removing it, and `platoon plugin list/add/remove/check`; a plugin with a skill, a squad and agents read
//! before it is installed, its squad on the Squads tab, and `platoon plugin show/add --yes/remove`. Every folder is a
//! temp one (the data folder and Claude's folder too: `PLATOON_DATA_DIR`, `CLAUDE_CONFIG_DIR`).

mod common;
use common::*;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use vt100::Color::Rgb;

const THEME: &str = "background = #14181f\nforeground = #dfe3ea\npalette = 1=#ff7a85\npalette = 2=#8bd49c\npalette = 3=#e6c170\npalette = 4=#7ab4ff\npalette = 5=#c89cf5\npalette = 9=#ff7a85\npalette = 10=#8bd49c\npalette = 11=#e6c170\npalette = 12=#7ab4ff\npalette = 13=#c89cf5\n";

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
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

/// A plugin folder `<at>/<id>` with one theme, `Harbor Test`.
fn plugin(at: &Path, id: &str, name: &str) -> PathBuf {
    let dir = at.join(id);
    fs::create_dir_all(dir.join("themes")).unwrap();
    fs::write(dir.join("themes/Harbor Test"), THEME).unwrap();
    let m = json!({"id": id, "name": name, "description": "A quiet dark theme for testing.", "version": "1.0.0", "author": "Someone", "license": "MIT", "platoon": "0.1.0", "themes": ["themes/Harbor Test"]});
    fs::write(dir.join("plugin.json"), serde_json::to_string_pretty(&m).unwrap()).unwrap();
    dir
}

const READER: &str = "You are the change-reader. Read the commits since the last release and list what a user would notice. Change nothing.";

/// A plugin folder `<at>/changelog-kit`: the skill changelog-kit-entry, the squad Release notes (its two agents, then
/// the built-in editor) and the agents change-reader and changelog-writer.
fn instructions_plugin(at: &Path) -> PathBuf {
    let dir = at.join("changelog-kit");
    for d in ["skills/changelog-kit-entry", "squads", "agents"] {
        fs::create_dir_all(dir.join(d)).unwrap();
    }
    fs::write(
        dir.join("skills/changelog-kit-entry/SKILL.md"),
        "---\nname: changelog-kit-entry\ndescription: Write a changelog entry for the changes on this branch.\n---\n\n# Changelog entry\n\nRead the commits, then write one plain line per change a user would notice.\n",
    )
    .unwrap();
    let reader = json!({"name": "change-reader", "model": "sonnet", "can_change_files": false, "can_run_commands": true, "can_use_web": false, "guidance": READER});
    let writer = json!({"name": "changelog-writer", "model": "sonnet", "can_change_files": true, "can_run_commands": false, "can_use_web": false, "skills": ["changelog-kit-entry"], "guidance": "You are the changelog-writer. Write the entry at the top of CHANGELOG.md."});
    fs::write(dir.join("agents/change-reader.json"), reader.to_string()).unwrap();
    fs::write(dir.join("agents/changelog-writer.json"), writer.to_string()).unwrap();
    let squad = json!({"name": "Release notes", "can_do": "Writes the changelog entry for a release.", "workflow": [
        {"agent": "change-reader", "does": "Lists what changed."},
        {"agent": "changelog-writer", "does": "Writes the entry."},
        {"agent": "editor", "does": "Checks the entry.", "back_to": 2, "times": 1}
    ]});
    fs::write(dir.join("squads/release-notes.json"), squad.to_string()).unwrap();
    let m = json!({"id": "changelog-kit", "name": "Changelog kit", "description": "A squad and a skill for changelogs.", "version": "1.0.0", "author": "Someone", "license": "MIT", "platoon": "0.1.0",
        "skills": ["skills/changelog-kit-entry"], "squads": ["squads/release-notes.json"], "agents": ["agents/change-reader.json", "agents/changelog-writer.json"]});
    fs::write(dir.join("plugin.json"), serde_json::to_string_pretty(&m).unwrap()).unwrap();
    dir
}

struct Lab {
    base: PathBuf,
    claude: PathBuf,
    cache: PathBuf,
    config: PathBuf,
    data: PathBuf,
    work: PathBuf,
    repo: PathBuf,
    commit: String,
}

impl Lab {
    fn new(name: &str) -> Self {
        let base = tmp(name);
        let repo = base.join("source");
        fs::create_dir_all(repo.join("plugins")).unwrap();
        fs::write(repo.join("README.md"), "code lives here\n").unwrap();
        plugin(&repo.join("plugins"), "harbor", "Harbor themes");
        instructions_plugin(&repo.join("plugins"));
        fs::write(repo.join("plugins/index.json"), json!({"plugins": [{"id": "harbor", "version": "1.0.0"}, {"id": "changelog-kit", "version": "1.0.0"}]}).to_string()).unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "catalog"]);
        let commit = git(&repo, &["rev-parse", "HEAD"]);
        let lab = Lab {
            claude: base.join("claude"),
            cache: base.join("cache"),
            config: base.join("config"),
            data: base.join("data"),
            work: base.join("work"),
            repo,
            commit,
            base,
        };
        for d in [&lab.claude.join("sessions"), &lab.claude.join("projects/-work"), &lab.cache, &lab.config, &lab.work] {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(lab.config.join("settings.json"), r#"{"auto_archive_days":0}"#).unwrap();
        let line = json!({"type":"user","cwd":lab.work,"gitBranch":"main","timestamp":"2026-09-24T10:00:00.000Z","message":{"role":"user","content":"make the shop"}});
        fs::write(lab.claude.join("projects/-work/s-1.jsonl"), format!("{line}\n")).unwrap();
        lab
    }

    fn env(&self) -> Vec<(&'static str, String)> {
        vec![
            ("CLAUDE_CONFIG_DIR", s(&self.claude)),
            ("PLATOON_CACHE_DIR", s(&self.cache)),
            ("PLATOON_CONFIG_DIR", s(&self.config)),
            ("PLATOON_DATA_DIR", s(&self.data)),
            ("PLATOON_PLUGIN_SOURCE", s(&self.repo)),
            // The code editor's font and downloads: the lab's too (never the real font folder, never the network).
            ("PLATOON_FONT_DIR", s(&self.base.join("fonts"))),
            ("PLATOON_EDITOR_MIRROR", s(&self.base.join("mirror"))),
            ("PLATOON_SKIP_FC_CACHE", "1".into()),
            ("PLATOON_SKIP_SETUP_CHECK", "1".into()),
            ("PLATOON_NOTES_HELPER", "off".into()),
        ]
    }

    fn installed(&self) -> Value {
        fs::read_to_string(self.data.join("plugins/installed.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
    }

    fn cli(&self, args: &[&str]) -> (i32, String, String) {
        let mut c = Command::new(BIN);
        c.args(args).current_dir(&self.base).stdin(Stdio::null());
        for (k, v) in self.env() {
            c.env(k, v);
        }
        let out = c.output().unwrap();
        (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

// Catppuccin Mocha, the default theme.
const SURFACE: vt100::Color = Rgb(0x31, 0x32, 0x44);
const ACCENT: vt100::Color = Rgb(0xb4, 0xbe, 0xfe);

#[test]
fn the_plugins_tab_shows_the_catalog_installs_with_a_line_in_the_footer_and_the_settings_tab_removes() {
    let lab = Lab::new("plug-term");
    let mut env = lab.env();
    env.push(("NO_COLOR", String::new())); // the helper sets it, which makes crossterm draw without any color
    let mut t = Term::spawn(&[], &env, &lab.work, 30, 120);
    t.wait_for_all(&["make the shop", "4 Plugins"]);
    t.send(b"4");
    t.wait_for_all(&["Harbor themes", "Not installed", "A quiet dark theme for testing.", "Adds", "1 theme: Harbor Test", &format!("Catalog: commit {}", &lab.commit[..7])]);
    t.wait_for_all(&["enter install", "f check the catalog", "1-4 tabs"]);
    // The selected plugin is on the raised color with the accent's marker, like the Squads tab's rows.
    let (row, col) = t.find("▎ Harbor themes").unwrap();
    t.wait_bg(row, col + 2, SURFACE);
    assert_eq!(t.fg_at(row, col), ACCENT, "the marker of the selected row");
    let (hrow, hcol) = t.find("enter install").unwrap();
    assert_eq!(t.fg_at(hrow, hcol), ACCENT, "the key of a hint is in the accent color");

    // Install: asked first, then done on a thread, said in the footer.
    t.send(b"\r");
    t.wait_for_all(&["Install Harbor themes?", "It adds 1 theme", "y install", "n not now"]);
    t.send(b"y");
    t.wait_for("Harbor themes is installed: 1 theme added");
    t.wait_for("Installed");
    until("installed.json to list it", || lab.installed()["plugins"]["harbor"]["commit"] == lab.commit.as_str());
    assert!(lab.data.join("plugins/harbor/themes/Harbor Test").is_file());

    // The settings' Plugins tab (the last): the installed plugin, and d removes it after asking.
    t.send(b"s");
    t.wait_for("Settings file:");
    t.send(b"\x1b[Z");
    t.wait_for_all(&["Harbor themes", "1.0.0 · 1 theme", &format!("From the catalog (commit {})", &lab.commit[..7]), "press 4 on the main screen", "d remove"]);
    t.send(b"d");
    t.wait_for_all(&["Remove Harbor themes?", "y remove", "n keep it"]);
    t.send(b"y");
    t.wait_for("No plugins installed.");
    until("installed.json to drop it", || lab.installed()["plugins"].get("harbor").is_none());
    assert!(!lab.data.join("plugins/harbor").exists());
    t.send(b"\x1b");
    t.wait_gone("Settings file:");
    t.wait_for("Not installed");
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);
}

#[test]
fn the_command_line_lists_adds_removes_and_checks_plugins() {
    let lab = Lab::new("plug-cli");
    let (code, out, err) = lab.cli(&["plugin", "list"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains(&format!("In the catalog (commit {}):", &lab.commit[..7])) && out.contains("harbor         1.0.0    Harbor themes (1 theme)"), "{out}");
    assert!(out.contains("changelog-kit  1.0.0    Changelog kit (1 skill, 1 squad, 2 agents)"), "{out}");
    assert!(out.contains("Installed:\n  none"), "{out}");
    let (code, out, err) = lab.cli(&["plugin", "add", "harbor"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("Installed Harbor themes 1.0.0 (1 theme)."), "{out}");
    let (_, out, _) = lab.cli(&["plugin", "list"]);
    assert!(out.contains("[installed]"), "{out}");
    // Its theme is listed with the others, marked as a plugin's.
    let (code, out, _) = lab.cli(&["themes"]);
    assert_eq!(code, 0);
    assert!(out.contains("plugin:harbor/harbor-test Harbor Test  (plugin: Harbor themes)"), "{out}");
    let (code, out, _) = lab.cli(&["plugin", "remove", "harbor"]);
    assert!(code == 0 && out.starts_with("Removed Harbor themes"), "{out}");
    // A folder being made: checked, then a broken one.
    let mine = plugin(&lab.base.join("making"), "my-theme", "My theme");
    let (code, out, _) = lab.cli(&["plugin", "check", &s(&mine)]);
    assert!(code == 0 && out.contains("Looks good"), "{out}");
    fs::write(mine.join("plugin.json"), "{\"id\": \"my-theme\"}").unwrap();
    let (code, out, _) = lab.cli(&["plugin", "check", &s(&mine)]);
    assert!(code == 1 && out.contains("plugin.json has no \"name\"") && out.contains("Not ready"), "{out}");
    let (code, _, err) = lab.cli(&["plugin", "add", "./making/my-theme"]);
    assert!(code == 1 && err.contains("can't be installed"), "{err}");
    let (code, _, err) = lab.cli(&["plugin"]);
    assert!(code == 2 && err.contains("usage: platoon plugin"), "{err}");
    // Nothing was ever written outside the lab.
    assert!(lab.data.join("plugins").starts_with(&lab.base));
}

impl Lab {
    /// Put platoon's own code editor plugin (`plugins/editor`, as it is in this repository) in the lab's catalog.
    fn with_editor(&self) {
        let ours = Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/editor");
        let theirs = self.repo.join("plugins/editor");
        for rel in ["plugin.json", "downloads.json", "config/init.lua", "config/lua/platoon/tree.lua", "config/shell.sh"] {
            fs::create_dir_all(theirs.join(rel).parent().unwrap()).unwrap();
            fs::copy(ours.join(rel), theirs.join(rel)).unwrap();
        }
        let index = json!({"plugins": [{"id": "editor", "version": "1.0.0"}, {"id": "harbor", "version": "1.0.0"}]});
        fs::write(self.repo.join("plugins/index.json"), index.to_string()).unwrap();
        git(&self.repo, &["add", "-A"]);
        git(&self.repo, &["commit", "-q", "-m", "editor"]);
    }
}

/// The code editor plugin on the Plugins tab and from the command line, like any other, with what it downloads. It is
/// never installed here (its real table would need the network); installing it is tested with fake downloads.
#[test]
fn the_editor_plugin_shows_on_the_tab_and_in_the_list_with_what_it_downloads() {
    let lab = Lab::new("plug-editor");
    lab.with_editor();
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 32, 130);
    t.wait_for_all(&["make the shop", "4 Plugins"]);
    t.send(b"4");
    t.wait_for_all(&["Code editor (Neovim)", "Harbor themes", "Adds", "the code editor (Neovim, its tools and plugins,", "it downloads about"]);
    t.wait_for_all(&["Not installed. Enter installs it and its downloads; then v on a session", "enter install"]);
    t.send(b"\r");
    t.wait_for_all(&["Install Code editor (Neovim)?", "It downloads about", "y install", "n not now"]);
    t.send(b"n");
    t.wait_gone("Install Code editor (Neovim)?");
    assert!(!lab.data.join("plugins/editor").exists() && !lab.data.join("editor").exists());
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);

    let (code, out, err) = lab.cli(&["plugin", "list"]);
    assert_eq!(code, 0, "{err}");
    let line = out.lines().find(|l| l.contains("Code editor (Neovim)")).unwrap_or_else(|| panic!("{out}"));
    assert!(line.contains("editor") && line.contains("(the code editor, downloads about ") && line.ends_with(" MB)"), "{line}");
    let (code, out, _) = lab.cli(&["plugin", "check", &s(&Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/editor"))]);
    assert!(code == 0 && out.contains("adds: the code editor") && out.contains("Looks good"), "{out}");
    assert!(!lab.base.join("fonts").exists());
}

#[test]
fn a_plugin_with_a_skill_a_squad_and_agents_is_read_before_it_is_installed_and_its_squad_is_on_the_squads_tab() {
    let lab = Lab::new("plug-review-term");
    let mut env = lab.env();
    env.push(("NO_COLOR", String::new()));
    let mut t = Term::spawn(&[], &env, &lab.work, 34, 120);
    t.wait_for_all(&["make the shop", "4 Plugins"]);
    t.send(b"4");
    t.wait_for_all(&["Harbor themes", "Changelog kit"]);
    t.send(b"\x1b[B");
    t.wait_for_all(&["enter read and install", "Enter shows each skill, squad and agent it adds"]);
    // Enter: each thing it adds, with what it may do.
    t.send(b"\r");
    t.wait_for_all(&["Install Changelog kit?", "Read each one before you install it", "changelog-kit-entry", "Release notes", "change-reader", "Can run commands on this computer", "Can change files: edit and write them"]);
    t.wait_for_all(&["enter read it", "y install", "n not now"]);
    // The keys of its hints in the accent colour, like every other hint.
    let (row, col) = t.find("y install").unwrap();
    assert_eq!(t.fg_at(row, col), ACCENT, "the key of a hint is in the accent color");
    let (row, col) = t.find("enter read it").unwrap();
    assert_eq!(t.fg_at(row, col), ACCENT);
    // The selected one is on the raised color with the accent's marker.
    let (row, col) = t.find("▎ Skill").unwrap();
    assert_eq!(t.fg_at(row, col), ACCENT, "the marker of the selected item");
    t.wait_bg(row, col + 2, SURFACE);
    // Reading the reader in full, on the whole screen.
    t.send(b"\x1b[B");
    t.send(b"\x1b[B");
    t.send(b"\r");
    t.wait_for_all(&["Agent: change-reader · from Changelog kit", "Its system prompt", "Read the commits since the last release", "esc back to the list"]);
    t.send(b"\x1b");
    t.wait_gone("Its system prompt");
    t.wait_for("y install");
    t.send(b"y");
    t.wait_for("Changelog kit is installed: 1 skill, 1 squad, 2 agents added.");
    until("its skill in Claude's folder", || lab.claude.join("skills/changelog-kit-entry/SKILL.md").is_file());
    assert!(lab.installed()["plugins"]["changelog-kit"]["skills"]["changelog-kit-entry"]["files"]["SKILL.md"].is_string(), "{}", lab.installed());
    // Its squad on the Squads tab, labelled with the plugin's name.
    t.send(b"3");
    t.wait_for_all(&["Release notes", "From Changelog kit"]);
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);
    assert!(!lab.config.join("squads").exists() && !lab.config.join("agents").exists(), "nothing of it written to the user's squads or agents");
}

#[test]
fn the_command_line_shows_a_plugin_with_instructions_refuses_it_without_a_terminal_and_adds_it_with_yes() {
    let lab = Lab::new("plug-cli-instructions");
    let (code, out, err) = lab.cli(&["plugin", "show", "changelog-kit"]);
    assert_eq!(code, 0, "{err}");
    for want in ["Changelog kit 1.0.0 by Someone, license MIT", "==== Skill: changelog-kit-entry ====", "Read the commits, then write one plain line", "==== Squad: Release notes ====", "==== Agent: change-reader ====", READER, "- Can run commands on this computer"] {
        assert!(out.contains(want), "{want}: {out}");
    }
    // No terminal (stdin is not one): refused, nothing installed.
    let (code, out, err) = lab.cli(&["plugin", "add", "changelog-kit"]);
    assert_eq!(code, 1);
    assert!(out.contains("Changelog kit 1.0.0 adds instructions Claude follows with your permissions:") && out.contains("  Agent change-reader: Read the commits since the last release"), "{out}");
    assert!(err.contains("there is no terminal here to ask in") && err.contains("platoon plugin add changelog-kit --yes"), "{err}");
    assert!(lab.installed()["plugins"].get("changelog-kit").is_none());
    assert!(!lab.claude.join("skills/changelog-kit-entry").exists());
    // --yes: installed, the skill written.
    let (code, out, err) = lab.cli(&["plugin", "add", "changelog-kit", "--yes"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("Installed Changelog kit 1.0.0 (1 skill, 1 squad, 2 agents).") && out.contains("type /changelog-kit-entry in a new Claude session"), "{out}");
    assert!(lab.claude.join("skills/changelog-kit-entry/SKILL.md").is_file());
    // Its squad is listed for the /to-tickets skill with the others.
    let (code, out, _) = lab.cli(&["squads", "--kinds"]);
    assert!(code == 0 && out.contains("Release notes"), "{out}");
    // Removing it takes its untouched skill away.
    let (code, out, _) = lab.cli(&["plugin", "remove", "changelog-kit"]);
    assert!(code == 0 && out.starts_with("Removed Changelog kit (1 skill, 1 squad, 2 agents no longer offered)."), "{out}");
    assert!(!lab.claude.join("skills/changelog-kit-entry").exists());
    assert!(lab.claude.join("skills").read_dir().unwrap().next().is_none(), "nothing left in Claude's skills folder");
}
