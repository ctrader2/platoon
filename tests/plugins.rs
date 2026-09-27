//! Plugins end to end with the real binary: the Plugins tab (`4`) drawn from a catalog fetched with git from a temp
//! repository (`PLATOON_PLUGIN_SOURCE`, never the network), installing one with the footer line, the settings'
//! Plugins tab removing it, and `platoon plugin list/add/remove/check`. Every folder is a temp one (the data folder
//! too: `PLATOON_DATA_DIR`).

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
        fs::write(repo.join("plugins/index.json"), json!({"plugins": [{"id": "harbor", "version": "1.0.0"}]}).to_string()).unwrap();
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
    assert!(out.contains(&format!("In the catalog (commit {}):", &lab.commit[..7])) && out.contains("harbor  1.0.0    Harbor themes (1 theme)"), "{out}");
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
