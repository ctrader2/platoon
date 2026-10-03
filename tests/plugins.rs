//! Plugins end to end with the real binary: the settings' Plugins tab listing a catalog (a temp folder standing in for
//! the one built into platoon: `PLATOON_PLUGIN_CATALOG`) with the selected one's description, the plugin screen it
//! opens installing one with the footer line, back to the settings on it, removing it there, and `platoon plugin
//! list/add/remove/check`; a plugin with a skill, a squad and agents read before it is installed, its squad on the
//! Squads tab, and `platoon plugin show/add --yes/remove`; the catalog built into platoon installing with no network
//! and no git; a newer catalog updating installed plugins when platoon starts, two windows at once. Every folder is a
//! temp one (the data folder and Claude's folder too: `PLATOON_DATA_DIR`, `CLAUDE_CONFIG_DIR`).

mod common;
use common::*;
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use vt100::Color::Rgb;

const THEME: &str = "background = #14181f\nforeground = #dfe3ea\npalette = 1=#ff7a85\npalette = 2=#8bd49c\npalette = 3=#e6c170\npalette = 4=#7ab4ff\npalette = 5=#c89cf5\npalette = 9=#ff7a85\npalette = 10=#8bd49c\npalette = 11=#e6c170\npalette = 12=#7ab4ff\npalette = 13=#c89cf5\n";

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

const READER: &str =
    "You are the change-reader. Read the commits since the last release and list what a user would notice. Change nothing.";

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
    /// The catalog: a folder laid out like the repository's `plugins/`.
    cat: PathBuf,
}

impl Lab {
    fn new(name: &str) -> Self {
        let base = tmp(name);
        let cat = base.join("catalog");
        fs::create_dir_all(&cat).unwrap();
        plugin(&cat, "harbor", "Harbor themes");
        instructions_plugin(&cat);
        fs::write(
            cat.join("index.json"),
            json!({"plugins": [{"id": "harbor", "version": "1.0.0"}, {"id": "changelog-kit", "version": "1.0.0"}]}).to_string(),
        )
        .unwrap();
        let lab = Lab {
            claude: base.join("claude"),
            cache: base.join("cache"),
            config: base.join("config"),
            data: base.join("data"),
            work: base.join("work"),
            cat,
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
            ("PLATOON_PLUGIN_CATALOG", s(&self.cat)),
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
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }
}

// Catppuccin Mocha, the default theme.
const SURFACE: vt100::Color = Rgb(0x31, 0x32, 0x44);
const ACCENT: vt100::Color = Rgb(0xb4, 0xbe, 0xfe);

/// `s`, then the settings' last tab (Plugins), with the catalog's plugins listed.
fn settings_plugins(t: &mut Term, first: &str) {
    t.send(b"s");
    t.wait_for("Settings file:");
    t.send(b"\x1b[Z");
    t.wait_for_all(&[first, "Enter opens a plugin", "enter open"]);
}

/// Back from the plugin screen to the settings, then out of them to the list (each gone before the next key: an esc
/// followed quickly by another key reads as alt+key).
fn back_to_the_list(t: &mut Term) {
    t.send(b"\x1b");
    t.wait_gone("Settings · Plugins");
    t.wait_for("Settings file:");
    t.send(b"\x1b");
    t.wait_gone("Settings file:");
}

#[test]
fn the_settings_list_every_plugin_and_open_the_plugin_screen_which_installs_and_goes_back_to_them() {
    let lab = Lab::new("plug-term");
    let mut env = lab.env();
    env.push(("NO_COLOR", String::new())); // the helper sets it, which makes crossterm draw without any color
    let mut t = Term::spawn(&[], &env, &lab.work, 30, 120);
    t.wait_for_all(&["make the shop", "1 Sessions", "3 Squads"]);
    let shown = t.stable();
    assert!(!shown.contains("Plugins") && !shown.contains("1-4"), "three top-level tabs:\n{shown}");
    // 4 is no tab now.
    t.send(b"4");
    let shown = t.stable();
    assert!(shown.contains("make the shop") && !shown.contains("Plugins"), "{shown}");

    // The settings' Plugins tab: every plugin of the catalog, one line each, and where each stands.
    settings_plugins(&mut t, "Harbor themes");
    // The selected plugin's description under the list.
    t.wait_for_all(&["Changelog kit", "A quiet dark theme for testing."]);
    let shown = t.stable();
    let line = |name: &str| shown.lines().find(|l| l.contains(name)).unwrap_or_else(|| panic!("{name}:\n{shown}")).to_string();
    assert!(line("▎ Harbor themes").contains("1.0.0") && line("Harbor themes").contains(" Available "), "{shown}");
    assert!(line("Changelog kit").contains(" Available "), "{shown}");
    assert!(!shown.contains("d remove"), "nothing installed to remove:\n{shown}");

    // Enter: the plugin screen, full screen, on that plugin.
    t.send(b"\r");
    t.wait_for_all(&["Settings · Plugins", "Not installed", "A quiet dark theme for testing.", "Adds", "1 theme: Harbor Test", "Needs"]);
    t.wait_for_all(&["enter install", "esc back to settings"]);
    let shown = t.stable();
    assert!(!shown.contains("Settings file:") && !shown.contains("make the shop"), "the plugin screen takes the whole body:\n{shown}");
    // No "From" line, nothing about fetching, no hint for the tab numbers.
    for gone in ["From", "Catalog", "check the catalog", "1-3"] {
        assert!(!shown.contains(gone), "{gone}:\n{shown}");
    }
    // f does nothing.
    t.send(b"f");
    assert!(!t.stable().contains("Checking"), "f fetches nothing");
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
    t.wait_gone("Install Harbor themes?");
    until("installed.json to list it", || lab.installed()["plugins"]["harbor"]["origin"] == "catalog");
    assert!(lab.data.join("plugins/harbor/themes/Harbor Test").is_file());
    t.wait_for_all(&["Settings · Plugins", "d remove"]);

    // Moving to the second, then the back key (ctrl+space): the settings' Plugins tab, the same plugin selected.
    t.send(b"\x1b[B");
    t.wait_for("▎ Changelog kit");
    t.send(b"\x00");
    t.wait_gone("Settings · Plugins");
    t.wait_for_all(&["Settings file:", "▎ Changelog kit", "Harbor themes"]);
    let shown = t.stable();
    let line = |name: &str| shown.lines().find(|l| l.contains(name)).unwrap_or_else(|| panic!("{name}:\n{shown}")).to_string();
    assert!(line("Harbor themes").contains(" Installed "), "{shown}");
    assert!(line("▎ Changelog kit").contains(" Available "), "{shown}");
    // Up to Harbor: d removes it from here, after asking.
    t.send(b"\x1b[A");
    t.wait_for_all(&["▎ Harbor themes", "d remove"]);
    t.send(b"d");
    t.wait_for_all(&["Remove Harbor themes?", "y remove", "n keep it"]);
    t.send(b"y");
    t.wait_gone("Remove Harbor themes?");
    until("installed.json to drop it", || lab.installed()["plugins"].get("harbor").is_none());
    assert!(!lab.data.join("plugins/harbor").exists());
    t.wait_for("Settings file:");
    let shown = t.stable();
    let harbor = shown.lines().find(|l| l.contains("▎ Harbor themes")).unwrap_or_else(|| panic!("{shown}")).to_string();
    assert!(harbor.contains(" Available "), "still listed, from the catalog:\n{shown}");

    // q on the plugin screen goes back to the settings; it does not quit.
    t.send(b"\r");
    t.wait_for("Settings · Plugins");
    t.send(b"q");
    t.wait_gone("Settings · Plugins");
    t.wait_for("Settings file:");
    t.send(b"\x1b");
    t.wait_gone("Settings file:");
    t.wait_for("make the shop");
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);
}

#[test]
fn the_command_line_lists_adds_removes_and_checks_plugins() {
    let lab = Lab::new("plug-cli");
    let (code, out, err) = lab.cli(&["plugin", "list"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.starts_with("In the catalog:\n") && out.contains("harbor         1.0.0    Harbor themes (1 theme)"), "{out}");
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
        let theirs = self.cat.join("editor");
        for rel in ["plugin.json", "downloads.json", "config/init.lua", "config/lua/platoon/tree.lua", "config/shell.sh"] {
            fs::create_dir_all(theirs.join(rel).parent().unwrap()).unwrap();
            fs::copy(ours.join(rel), theirs.join(rel)).unwrap();
        }
        // The index names the version the plugin's own file has (a catalog whose two disagree is refused).
        let version =
            serde_json::from_str::<serde_json::Value>(&fs::read_to_string(ours.join("plugin.json")).unwrap()).unwrap()["version"].clone();
        let index = json!({"plugins": [{"id": "editor", "version": version}, {"id": "harbor", "version": "1.0.0"}]});
        fs::write(self.cat.join("index.json"), index.to_string()).unwrap();
    }
}

/// The code editor plugin on the plugin screen and from the command line, like any other, with what it downloads. It is
/// never installed here (its real table would need the network); installing it is tested with fake downloads.
#[test]
fn the_editor_plugin_shows_on_the_tab_and_in_the_list_with_what_it_downloads() {
    let lab = Lab::new("plug-editor");
    lab.with_editor();
    let mut t = Term::spawn(&[], &lab.env(), &lab.work, 32, 130);
    t.wait_for_all(&["make the shop", "3 Squads"]);
    settings_plugins(&mut t, "Code editor (Neovim)");
    t.send(b"\r");
    t.wait_for_all(&[
        "Settings · Plugins",
        "Code editor (Neovim)",
        "Harbor themes",
        "Adds",
        "the code editor (Neovim, its tools and plugins,",
        "it downloads about",
    ]);
    t.wait_for_all(&["Not installed. Enter installs it and its downloads; then v on a session", "enter install"]);
    t.send(b"\r");
    t.wait_for_all(&["Install Code editor (Neovim)?", "It downloads about", "y install", "n not now"]);
    t.send(b"n");
    t.wait_gone("Install Code editor (Neovim)?");
    t.wait_for("Settings · Plugins");
    assert!(!lab.data.join("plugins/editor").exists() && !lab.data.join("editor").exists());
    back_to_the_list(&mut t);
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
    t.wait_for_all(&["make the shop", "3 Squads"]);
    settings_plugins(&mut t, "Changelog kit");
    t.send(b"\x1b[B");
    t.wait_for("▎ Changelog kit");
    t.send(b"\r");
    t.wait_for_all(&["Settings · Plugins", "Harbor themes", "Changelog kit"]);
    t.wait_for_all(&["enter read and install", "Enter shows each skill, squad and agent it adds"]);
    // Enter: each thing it adds, with what it may do.
    t.send(b"\r");
    t.wait_for_all(&[
        "Install Changelog kit?",
        "Read each one before you install it",
        "changelog-kit-entry",
        "Release notes",
        "change-reader",
        "Can run commands on this computer",
        "Can change files: edit and write them",
    ]);
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
    t.wait_for_all(&[
        "Agent: change-reader · from Changelog kit",
        "Its system prompt",
        "Read the commits since the last release",
        "esc back to the list",
    ]);
    t.send(b"\x1b");
    t.wait_gone("Its system prompt");
    t.wait_for("y install");
    t.send(b"y");
    t.wait_for("Changelog kit is installed: 1 skill, 1 squad, 2 agents added.");
    until("its skill in Claude's folder", || lab.claude.join("skills/changelog-kit-entry/SKILL.md").is_file());
    assert!(
        lab.installed()["plugins"]["changelog-kit"]["skills"]["changelog-kit-entry"]["files"]["SKILL.md"].is_string(),
        "{}",
        lab.installed()
    );
    // Its squad on the Squads tab, labelled with the plugin's name.
    t.wait_gone("Install Changelog kit?");
    back_to_the_list(&mut t);
    t.send(b"3");
    t.wait_for_all(&["Release notes", "From Changelog kit"]);
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);
    assert!(
        !lab.config.join("squads").exists() && !lab.config.join("agents").exists(),
        "nothing of it written to the user's squads or agents"
    );
}

#[test]
fn the_command_line_shows_a_plugin_with_instructions_refuses_it_without_a_terminal_and_adds_it_with_yes() {
    let lab = Lab::new("plug-cli-instructions");
    let (code, out, err) = lab.cli(&["plugin", "show", "changelog-kit"]);
    assert_eq!(code, 0, "{err}");
    for want in [
        "Changelog kit 1.0.0 by Someone, license MIT",
        "==== Skill: changelog-kit-entry ====",
        "Read the commits, then write one plain line",
        "==== Squad: Release notes ====",
        "==== Agent: change-reader ====",
        READER,
        "- Can run commands on this computer",
    ] {
        assert!(out.contains(want), "{want}: {out}");
    }
    // No terminal (stdin is not one): refused, nothing installed.
    let (code, out, err) = lab.cli(&["plugin", "add", "changelog-kit"]);
    assert_eq!(code, 1);
    assert!(
        out.contains("Changelog kit 1.0.0 adds instructions Claude follows with your permissions:")
            && out.contains("  Agent change-reader: Read the commits since the last release"),
        "{out}"
    );
    assert!(err.contains("there is no terminal here to ask in") && err.contains("platoon plugin add changelog-kit --yes"), "{err}");
    assert!(lab.installed()["plugins"].get("changelog-kit").is_none());
    assert!(!lab.claude.join("skills/changelog-kit-entry").exists());
    // --yes: installed, the skill written.
    let (code, out, err) = lab.cli(&["plugin", "add", "changelog-kit", "--yes"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("Installed Changelog kit 1.0.0 (1 skill, 1 squad, 2 agents).")
            && out.contains("type /changelog-kit-entry in a new Claude session"),
        "{out}"
    );
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

impl Lab {
    /// `platoon <args>` with the lab's folders and these variables on top.
    fn cli_with(&self, extra: &[(&str, String)], args: &[&str]) -> (i32, String, String) {
        let mut c = Command::new(BIN);
        c.args(args).current_dir(&self.base).stdin(Stdio::null());
        for (k, v) in self.env().into_iter().chain(extra.iter().map(|(k, v)| (*k, v.clone()))) {
            c.env(k, v);
        }
        let out = c.output().unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// The version installed of plugin `id`, as `installed.json` has it.
    fn version(&self, id: &str) -> String {
        self.installed()["plugins"][id]["version"].as_str().unwrap_or_default().to_string()
    }

    /// A newer version of plugin `id` in the lab's catalog (its plugin.json and the index), as a newer platoon brings.
    fn bump(&self, id: &str, to: &str) {
        let path = self.cat.join(id).join("plugin.json");
        let mut m: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        m.as_object_mut().unwrap().insert("version".into(), json!(to));
        fs::write(&path, m.to_string()).unwrap();
        let mut index: Value = serde_json::from_str(&fs::read_to_string(self.cat.join("index.json")).unwrap()).unwrap();
        for p in index.get_mut("plugins").and_then(Value::as_array_mut).unwrap() {
            if p.get("id").and_then(Value::as_str) == Some(id) {
                p.as_object_mut().unwrap().insert("version".into(), json!(to));
            }
        }
        fs::write(self.cat.join("index.json"), index.to_string()).unwrap();
    }
}

/// A `git` that only writes down that it was run, first on PATH (then the usual system folders): platoon must never run
/// git for plugins. The PATH, and the file it writes.
fn git_that_tells(lab: &Lab) -> (String, PathBuf) {
    let bin = lab.base.join("fake-bin");
    fs::create_dir_all(&bin).unwrap();
    let said = lab.base.join("git-was-run");
    fs::write(bin.join("git"), format!("#!/bin/sh\necho \"$@\" >> '{}'\nexit 1\n", said.display())).unwrap();
    fs::set_permissions(bin.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
    (format!("{}:/usr/bin:/bin", bin.display()), said)
}

/// A plugin of the catalog built into platoon that adds themes only (whatever it is called now): its id and name.
fn a_built_in_theme_plugin() -> (String, String) {
    let cat = Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins");
    let index: Value = serde_json::from_str(&fs::read_to_string(cat.join("index.json")).unwrap()).unwrap();
    for p in index["plugins"].as_array().unwrap() {
        let id = p["id"].as_str().unwrap();
        let m: Value = serde_json::from_str(&fs::read_to_string(cat.join(id).join("plugin.json")).unwrap()).unwrap();
        let o = m.as_object().unwrap();
        if o.contains_key("themes") && !["editor", "skills", "squads", "agents"].iter().any(|k| o.contains_key(*k)) {
            return (id.to_string(), m["name"].as_str().unwrap().to_string());
        }
    }
    panic!("the built-in catalog has a theme plugin");
}

#[test]
fn the_catalog_built_into_platoon_lists_and_installs_with_no_network_and_no_git() {
    let lab = Lab::new("plug-built-in");
    let (path, git_ran) = git_that_tells(&lab);
    let (id, name) = a_built_in_theme_plugin();
    // An empty PLATOON_PLUGIN_CATALOG: the catalog built in. A proxy that answers nothing: no network either.
    let extra = [
        ("PLATOON_PLUGIN_CATALOG", String::new()),
        ("PATH", path.clone()),
        ("https_proxy", "http://127.0.0.1:9".into()),
        ("HTTPS_PROXY", "http://127.0.0.1:9".into()),
    ];
    let (code, out, err) = lab.cli_with(&extra, &["plugin", "list"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.starts_with("In the catalog:\n") && out.contains("Code editor (Neovim)") && out.contains(&name), "{out}");
    assert!(!out.contains("Harbor themes"), "not the lab's catalog: {out}");
    let (code, out, err) = lab.cli_with(&extra, &["plugin", "add", &id]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains(&format!("Installed {name} ")), "{out}");
    assert_eq!(lab.installed()["plugins"][id.as_str()]["origin"], "catalog");
    assert!(lab.data.join("plugins").join(&id).join("plugin.json").is_file());
    let (code, out, _) = lab.cli_with(&extra, &["plugin", "update"]);
    assert!(code == 0 && out.contains(&format!("{name} is up to date.")), "{out}");
    // The UI lists the same plugins, the selected one's description under them.
    let mut env = lab.env();
    env.extend(extra.iter().map(|(k, v)| (*k, v.clone())));
    let mut t = Term::spawn(&[], &env, &lab.work, 34, 120);
    t.wait_for_all(&["make the shop", "3 Squads"]);
    settings_plugins(&mut t, "Code editor (Neovim)");
    t.wait_for_all(&[&name, "Neovim"]);
    t.send(b"\x1b");
    t.wait_gone("Settings file:");
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);
    assert!(!git_ran.exists(), "git was run: {}", fs::read_to_string(&git_ran).unwrap_or_default());
    // The copy an older platoon fetched with git goes, and nothing else of the data folder.
    fs::create_dir_all(lab.data.join("plugins/.catalog/editor")).unwrap();
    fs::write(lab.data.join("plugins/.catalog.json"), "{}").unwrap();
    let (code, _, _) = lab.cli_with(&extra, &["plugin", "list"]);
    assert_eq!(code, 0);
    assert!(!lab.data.join("plugins/.catalog").exists() && !lab.data.join("plugins/.catalog.json").exists());
    assert!(lab.data.join("plugins").join(&id).is_dir() && lab.data.join("plugins/installed.json").is_file());
}

#[test]
fn a_newer_catalog_updates_installed_plugins_when_platoon_starts_and_asks_about_one_with_instructions() {
    let lab = Lab::new("plug-auto-term");
    let env = lab.env();
    let (code, _, err) = lab.cli(&["plugin", "add", "harbor"]);
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = lab.cli(&["plugin", "add", "changelog-kit", "--yes"]);
    assert_eq!(code, 0, "{err}");
    // One of the user's own, from a folder: never touched, though the catalog has a newer one of that id.
    let mine = plugin(&lab.base.join("making"), "my-theme", "My theme");
    let (code, _, err) = lab.cli(&["plugin", "add", &s(&mine)]);
    assert_eq!(code, 0, "{err}");
    plugin(&lab.cat, "my-theme", "My theme (catalog)");
    let mut index: Value = serde_json::from_str(&fs::read_to_string(lab.cat.join("index.json")).unwrap()).unwrap();
    index["plugins"].as_array_mut().unwrap().push(json!({"id": "my-theme", "version": "1.0.0"}));
    fs::write(lab.cat.join("index.json"), index.to_string()).unwrap();
    lab.bump("my-theme", "5.0.0");

    // Nothing newer: nothing said, nothing asked.
    let mut t = Term::spawn(&[], &env, &lab.work, 34, 120);
    t.wait_for_all(&["make the shop", "3 Squads"]);
    let shown = t.stable();
    assert!(!shown.contains("Updated with platoon") && !shown.contains("Update Changelog kit"), "{shown}");
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);

    // A newer platoon: Harbor 1.1.0 (a theme: quietly), Changelog kit 1.1.0 (instructions: asked).
    lab.bump("harbor", "1.1.0");
    fs::write(lab.cat.join("harbor/themes/Harbor Test"), THEME.replace("#14181f", "#121820")).unwrap();
    lab.bump("changelog-kit", "1.1.0");
    let reader = lab.cat.join("changelog-kit/agents/change-reader.json");
    fs::write(&reader, fs::read_to_string(&reader).unwrap().replace("Change nothing.", "Change nothing, not even a tag.")).unwrap();
    let mut t = Term::spawn(&[], &env, &lab.work, 34, 120);
    t.wait_for("Updated with platoon: Harbor themes 1.1.0.");
    t.wait_for_all(&["Update Changelog kit from 1.0.0 to 1.1.0?", "change-reader · changed", "y update", "n not now"]);
    assert_eq!(lab.version("harbor"), "1.1.0");
    assert_eq!(lab.version("changelog-kit"), "1.0.0", "not without asking");
    assert_eq!(lab.version("my-theme"), "1.0.0");
    assert_eq!(lab.installed()["plugins"]["my-theme"]["origin"], "folder");
    t.send(b"n");
    t.wait_gone("Update Changelog kit");
    t.wait_for_flat("Changelog kit stays at 1.0.0; platoon won't ask about 1.1.0 again.");
    assert_eq!(lab.installed()["plugins"]["changelog-kit"]["declined_update"], "1.1.0");
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);

    // Started again: not asked again, nothing said; the settings say an update is ready.
    let mut t = Term::spawn(&[], &env, &lab.work, 34, 120);
    t.wait_for_all(&["make the shop", "3 Squads"]);
    let shown = t.stable();
    assert!(!shown.contains("Updated with platoon") && !shown.contains("Update Changelog kit"), "{shown}");
    settings_plugins(&mut t, "Harbor themes");
    t.wait_for("Changelog kit");
    let shown = t.stable();
    let line = |name: &str| shown.lines().find(|l| l.contains(name)).unwrap_or_else(|| panic!("{name}:\n{shown}")).to_string();
    assert!(line("Changelog kit").contains(" Update ready "), "{shown}");
    assert!(line("Harbor themes").contains("1.1.0") && line("Harbor themes").contains(" Installed "), "{shown}");
    t.send(b"\x1b");
    t.wait_gone("Settings file:");
    t.send(b"q");
    assert_eq!(t.wait_exit(), 0);
    // `platoon plugin update` asks too (no terminal here: it says how), and with --yes updates it.
    let (code, out, err) = lab.cli(&["plugin", "update"]);
    assert!(
        code == 1
            && out.contains("Changelog kit 1.0.0 -> 1.1.0 changes instructions")
            && err.contains("platoon plugin update changelog-kit --yes"),
        "{out}{err}"
    );
    let (code, out, err) = lab.cli(&["plugin", "update", "--yes"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("Updated Changelog kit from 1.0.0 to 1.1.0.")
            && out.contains("Harbor themes is up to date.")
            && out.contains("My theme was installed from a folder"),
        "{out}"
    );
    assert!(lab.installed()["plugins"]["changelog-kit"].get("declined_update").is_none());
}

#[test]
fn two_platoons_starting_at_once_update_a_plugin_once() {
    let lab = Lab::new("plug-auto-twice");
    let env = lab.env();
    let (code, _, err) = lab.cli(&["plugin", "add", "harbor"]);
    assert_eq!(code, 0, "{err}");
    lab.bump("harbor", "1.1.0");
    let mut a = Term::spawn(&[], &env, &lab.work, 30, 120);
    let mut b = Term::spawn(&[], &env, &lab.work, 30, 120);
    a.wait_for("make the shop");
    b.wait_for("make the shop");
    until("the update", || lab.version("harbor") == "1.1.0");
    // Whichever did it says so; the other has nothing to say (and nothing went wrong in either).
    let said = |t: &mut Term| t.stable().contains("Updated with platoon: Harbor themes 1.1.0.");
    let mut both = (false, false);
    until("one of them to say it", || {
        both = (said(&mut a), said(&mut b));
        both.0 || both.1
    });
    assert!(!(both.0 && both.1), "said once");
    for t in [&mut a, &mut b] {
        let shown = t.stable();
        assert!(!shown.contains("couldn't") && !shown.contains("Another platoon"), "{shown}");
    }
    assert!(!lab.data.join("plugins/.tmp").exists());
    assert_eq!(lab.installed()["plugins"]["harbor"]["origin"], "catalog");
    for t in [&mut a, &mut b] {
        t.send(b"q");
        assert_eq!(t.wait_exit(), 0);
    }
}
