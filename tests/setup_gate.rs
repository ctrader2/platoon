//! `platoon` must not start until `platoon setup` has been run, and setup must be safely repeatable.

mod common;

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
    process::Command,
    sync::mpsc::channel,
    thread,
    time::{Duration, Instant},
};

const BIN: &str = env!("CARGO_BIN_EXE_platoon");

fn dirs(name: &str) -> (PathBuf, PathBuf) {
    let base = common::removed_at_exit(std::env::temp_dir().join(format!("platoon-gate-{name}-{}", std::process::id())));
    let _ = fs::remove_dir_all(&base);
    let (claude, cache) = (base.join("claude"), base.join("cache"));
    fs::create_dir_all(&claude).unwrap();
    fs::create_dir_all(&cache).unwrap();
    (claude, cache)
}

/// The computer a test's platoon runs on: Claude Code installed (`common::NO_REAL_CLAUDE`, a program that can't run),
/// no Codex.
fn with_claude() -> Vec<(&'static str, Option<String>)> {
    vec![(common::NO_REAL_CLAUDE.0, Some(common::NO_REAL_CLAUDE.1.to_string()))]
}

/// A computer with neither Claude Code nor this machine's own `claude` or `codex` on the path (whatever the machine
/// running the tests has), plus `extra`.
fn without_claude(extra: &[(&'static str, String)]) -> Vec<(&'static str, Option<String>)> {
    let path: Vec<PathBuf> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .filter(|d| !d.join("claude").exists() && !d.join("codex").exists())
        .collect();
    let mut env = vec![("PLATOON_CLAUDE_BIN", None), ("PATH", Some(std::env::join_paths(path).unwrap().to_string_lossy().into_owned()))];
    env.extend(extra.iter().map(|(k, v)| (*k, Some(v.clone()))));
    env
}

fn platoon(claude: &PathBuf, cache: &PathBuf, args: &[&str]) -> std::process::Output {
    platoon_on(claude, cache, &with_claude(), args)
}

/// `platoon <args>` with `env` set (None: taken out) after the usual environment.
fn platoon_on(claude: &PathBuf, cache: &PathBuf, env: &[(&str, Option<String>)], args: &[&str]) -> std::process::Output {
    let mut c = Command::new(BIN);
    c.env("PLATOON_NO_UPDATE_CHECK", "1")
        .envs(common::NO_REAL_CODEX)
        .args(args)
        .env("CLAUDE_CONFIG_DIR", claude)
        .env("PLATOON_CACHE_DIR", cache)
        // Never the real one: `platoon themes` reads the user's theme files from here.
        .env("PLATOON_CONFIG_DIR", cache.parent().unwrap().join("config"))
        .env("NO_COLOR", "1")
        .env_remove("PLATOON_SKIP_SETUP_CHECK");
    for (k, v) in env {
        match v {
            Some(v) => c.env(k, v),
            None => c.env_remove(k),
        };
    }
    c.output().unwrap()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn platoon_refuses_to_start_until_setup_is_done_and_says_what_is_missing() {
    let (claude, cache) = dirs("blocked");
    let out = platoon(&claude, &cache, &[]);
    assert_eq!(out.status.code(), Some(1));
    let err = text(&out.stderr);
    assert!(err.contains("one-time setup"), "{err}");
    assert_eq!(err.matches("[✗]").count(), 2, "both steps should show as not done:\n{err}");
    assert!(err.contains("Status line") && err.contains("Session guard") && err.contains("platoon setup"), "{err}");
}

#[test]
fn setup_lists_each_step_and_running_it_again_marks_everything_already_done() {
    let (claude, cache) = dirs("setup");
    let first = platoon(&claude, &cache, &["setup"]);
    assert!(first.status.success());
    let shown = text(&first.stdout);
    assert_eq!(shown.matches("[✓]").count(), 4, "{shown}");
    assert!(
        shown.contains("added") && shown.contains("Saved") && shown.contains("Project notes") && shown.contains("Notes updates"),
        "{shown}"
    );
    let settings = fs::read_to_string(claude.join("settings.json")).unwrap();

    let second = platoon(&claude, &cache, &["setup"]);
    assert!(second.status.success());
    let shown = text(&second.stdout);
    assert_eq!(shown.matches("[✓]").count(), 4, "{shown}");
    assert_eq!(shown.matches("already done").count(), 4, "{shown}");
    assert!(shown.contains("Nothing to change"), "{shown}");
    assert_eq!(fs::read_to_string(claude.join("settings.json")).unwrap(), settings, "a repeat run must not touch the file");
}

/// setup installs no skill and takes no arguments: anything after it is refused with the usage, and nothing is
/// written.
#[test]
fn setup_installs_no_skill_and_refuses_arguments() {
    let (claude, cache) = dirs("args");
    let out = platoon(&claude, &cache, &["setup"]);
    assert!(out.status.success());
    let shown = text(&out.stdout);
    assert!(!shown.contains("skill") && !shown.contains("to-tickets"), "{shown}");
    assert!(!claude.join("skills").exists(), "nothing written there");
    let settings = fs::read_to_string(claude.join("settings.json")).unwrap();
    for gone in [&["setup", "--remove", "to-tickets"][..], &["setup", "--add", "to-tickets"], &["setup", "--nope"]] {
        let out = platoon(&claude, &cache, gone);
        assert_eq!(out.status.code(), Some(2), "{gone:?}");
        assert!(text(&out.stderr).starts_with("usage: platoon setup"), "{gone:?}: {out:?}");
    }
    assert_eq!(fs::read_to_string(claude.join("settings.json")).unwrap(), settings);
}

/// Runs `platoon` in a real terminal; returns the screen text once it shows `wait_for`, then quits it.
fn run_in_terminal(claude: &PathBuf, cache: &PathBuf, wait_for: &str) -> String {
    run_in_terminal_on(claude, cache, &with_claude(), wait_for)
}

/// `run_in_terminal` with `env` set (None: taken out) after the usual environment.
fn run_in_terminal_on(claude: &PathBuf, cache: &PathBuf, env: &[(&str, Option<String>)], wait_for: &str) -> String {
    let pair = native_pty_system().openpty(PtySize { rows: 24, cols: 90, pixel_width: 0, pixel_height: 0 }).unwrap();
    let mut cmd = CommandBuilder::new(BIN);
    cmd.env("PLATOON_NO_UPDATE_CHECK", "1"); // no version checks: never the network, never `claude --version`
    for (k, v) in common::NO_REAL_CODEX {
        cmd.env(k, v); // never the real Codex: its folder or its program
    }
    cmd.env("CLAUDE_CONFIG_DIR", claude);
    cmd.env("PLATOON_CACHE_DIR", cache);
    cmd.env("PLATOON_CONFIG_DIR", cache.join("config"));
    cmd.env("TERM", "xterm-256color");
    cmd.env_remove("PLATOON_SKIP_SETUP_CHECK");
    for (k, v) in env {
        match v {
            Some(v) => cmd.env(k, v),
            None => cmd.env_remove(k),
        }
    }
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();
    let (tx, rx) = channel::<Vec<u8>>();
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut screen = vt100::Parser::new(24, 90, 0);
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if screen.screen().contents().contains(wait_for) {
            break;
        }
        if let Ok(b) = rx.recv_timeout(Duration::from_millis(100)) {
            screen.process(&b);
        }
    }
    let shown = screen.screen().contents();
    let _ = writer.write_all(b"q");
    let _ = child.kill();
    let _ = child.wait();
    shown
}

#[test]
fn once_setup_is_done_platoon_starts_and_undoing_a_step_blocks_it_again() {
    let (claude, cache) = dirs("start");
    assert!(platoon(&claude, &cache, &["setup"]).status.success());

    let screen = run_in_terminal(&claude, &cache, "No sessions yet");
    assert!(screen.contains("No sessions yet"), "the UI should have started:\n{screen}");

    // Someone removes the guard hook: platoon notices and refuses again.
    let path = claude.join("settings.json");
    let mut v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    v["hooks"] = serde_json::json!({});
    fs::write(&path, v.to_string()).unwrap();
    let out = platoon(&claude, &cache, &[]);
    assert_eq!(out.status.code(), Some(1));
    let err = text(&out.stderr);
    assert!(err.contains("[✓]") && err.contains("[✗]") && err.contains("Session guard"), "{err}");
}

#[test]
fn without_the_project_notes_hook_platoon_still_starts_and_setup_adds_only_that() {
    let (claude, cache) = dirs("notes-hook");
    assert!(platoon(&claude, &cache, &["setup"]).status.success());
    // An existing user's settings: the two hooks from before, not the new one.
    let path = claude.join("settings.json");
    let mut v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    v["hooks"].as_object_mut().unwrap().remove("SessionStart");
    fs::write(&path, v.to_string()).unwrap();
    let screen = run_in_terminal(&claude, &cache, "No sessions yet");
    assert!(screen.contains("No sessions yet"), "the missing notes hook must not block starting:\n{screen}");
    let out = platoon(&claude, &cache, &["setup"]);
    let shown = text(&out.stdout);
    assert_eq!(shown.matches("already done").count(), 3, "{shown}");
    assert!(shown.contains("Project notes") && shown.contains("added"), "{shown}");
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let cmd = v["hooks"]["SessionStart"][0]["hooks"][0]["command"].as_str().unwrap();
    assert!(cmd.ends_with(" hook session-start"), "{cmd}");
    assert_eq!(v["hooks"]["UserPromptSubmit"].as_array().unwrap().len(), 1, "the guard is not added twice");
    assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 1, "and neither is the Stop hook");
}

#[test]
fn the_stop_hook_that_keeps_notes_up_to_date_is_added_beside_any_other_stop_hook() {
    let (claude, cache) = dirs("stop-hook");
    // Somebody else's Stop hook is already there: it must survive, and platoon's goes beside it.
    fs::write(claude.join("settings.json"), r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"say-something"}]}]}}"#).unwrap();
    assert!(platoon(&claude, &cache, &["setup"]).status.success());
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(claude.join("settings.json")).unwrap()).unwrap();
    let groups = v["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(groups.len(), 2, "{v}");
    assert_eq!(groups[0]["hooks"][0]["command"], "say-something");
    assert!(groups[1]["hooks"][0]["command"].as_str().unwrap().ends_with(" hook stop"), "{v}");
    // Running the hook by hand with nothing to do is instant, silent and fine.
    let out = std::process::Command::new(BIN)
        .env("PLATOON_NO_UPDATE_CHECK", "1")
        .envs(common::NO_REAL_CODEX)
        .args(["hook", "stop"])
        .env("CLAUDE_CONFIG_DIR", &claude)
        .env("PLATOON_CACHE_DIR", &cache)
        .env("PLATOON_CONFIG_DIR", cache.join("config"))
        .env("PLATOON_NOTES_HELPER", "off")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            use std::io::Write;
            c.stdin.take().unwrap().write_all(br#"{"session_id":"nobody","cwd":"/tmp","hook_event_name":"Stop"}"#)?;
            c.wait_with_output()
        })
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
}

#[test]
fn themes_lists_the_built_ins_without_setup_and_touches_nothing() {
    let (claude, cache) = dirs("themes");
    let out = platoon(&claude, &cache, &["themes"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let shown = text(&out.stdout);
    assert!(!shown.contains('\x1b'), "piped output is plain text");
    let ids: Vec<&str> = shown.lines().filter(|l| !l.starts_with(' ')).filter_map(|l| l.split_whitespace().next()).collect();
    assert_eq!(
        ids,
        [
            "catppuccin-mocha",
            "catppuccin-macchiato",
            "catppuccin-frappe",
            "tokyo-night",
            "tokyo-night-storm",
            "dracula",
            "one-dark",
            "nord",
            "gruvbox-dark",
            "gruvbox-material-dark",
            "rose-pine",
            "rose-pine-moon",
            "kanagawa-wave",
            "kanagawa-dragon",
            "night-owl",
            "ayu-mirage",
            "vesper",
            "solarized-dark",
            "github-dark",
            "material-ocean",
            "everforest-dark-hard",
            "nightfox",
            "sonokai",
            "moonfly",
            "melange-dark",
            "flexoki-dark",
            "oxocarbon",
            "poimandres",
            "horizon",
            "synthwave",
            "claude",
            "harbor-night",
            "moss-lantern",
            "ayu-dark",
            "doom-one",
            "embark",
            "everblush",
            "github-dark-dimmed",
            "iceberg-dark",
            "jellybeans",
            "srcery",
            "tomorrow-night",
            "zenbones-dark",
            "catppuccin-latte",
            "dayfox",
            "flexoki-light",
            "github-light",
            "gruvbox-light",
            "one-half-light",
            "rose-pine-dawn",
            "solarized-light",
            "terminal"
        ]
    );
    assert!(shown.contains("Catppuccin Mocha  (default)") && shown.contains("Rosé Pine") && shown.contains("Terminal colors"), "{shown}");
    assert!(shown.contains("accent #b4befe"), "{shown}");
    assert!(fs::read_dir(&claude).unwrap().next().is_none() && fs::read_dir(&cache).unwrap().next().is_none(), "nothing is written");
}

#[test]
fn themes_lists_the_users_own_themes_marked_and_the_files_it_skipped_with_the_reason() {
    let (claude, cache) = dirs("themes-user");
    let themes = cache.parent().unwrap().join("config/themes");
    fs::create_dir_all(&themes).unwrap();
    fs::write(themes.join("Deep Sea"), "background = #0a1622\nforeground = #dde6ee\npalette = 12=#4fb0ff\n").unwrap();
    fs::write(themes.join("Nord"), "background = #001020\nforeground = #e6e6e6\n").unwrap(); // a built-in's name
    fs::write(themes.join("Nord Light"), "background = #eceff4\nforeground = #2e3440\n").unwrap();
    fs::write(themes.join("Half"), "foreground = #ffffff\n").unwrap();
    fs::write(themes.join("Noise"), [0u8, 159, 146, 150]).unwrap();
    fs::create_dir(themes.join("A Folder")).unwrap();
    let out = platoon(&claude, &cache, &["themes"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let shown = text(&out.stdout);
    let ids: Vec<&str> = shown
        .lines()
        .filter(|l| !l.starts_with(' ') && l.contains("  ") && !l.contains("skipped"))
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    assert_eq!(
        &ids[ids.len() - 2..],
        ["user:deep-sea", "user:nord"],
        "the built-ins first, then the user's own in file-name order:\n{shown}"
    );
    assert_eq!(ids[0], "catppuccin-mocha");
    assert!(shown.contains("user:deep-sea") && shown.contains("Deep Sea  (yours)"), "{shown}");
    assert!(shown.contains("Nord (yours)") && !shown.contains("Nord (yours)  (yours)"), "a name a built-in has is marked once: {shown}");
    assert!(shown.contains("bg #0a1622"), "the roles are listed like a built-in's:\n{shown}");
    assert!(shown.contains("3 theme files skipped in "), "{shown}");
    fs::write(themes.join("Paper"), "background = #fafafa\nforeground = #202020\npalette = 4=#1a55c0\npalette = 5=#9a2fa0\npalette = 2=#2e7d32\npalette = 3=#8a5d00\npalette = 1=#c62828\n").unwrap();
    let out = platoon(&claude, &cache, &["themes"]);
    let shown = text(&out.stdout);
    assert!(
        shown.contains("user:paper") && shown.contains("Paper  (yours)"),
        "a light theme that reads as it is is one of the user's:\n{shown}"
    );
    let nord_light = "Nord Light: a light theme has to read as it is: palette 12 (or 4) is missing: it is the accent color (or give it as platoon-accent), and 4 more problems";
    for want in [nord_light, "Half: no background color", "Noise: the file isn't text"] {
        assert!(shown.lines().any(|l| l.trim() == want), "{want:?} in:\n{shown}");
    }
    assert!(!shown.contains("A Folder") && !shown.contains('\x1b'));
    // Nothing is written anywhere, and the folder the user made is untouched.
    assert!(fs::read_dir(&claude).unwrap().next().is_none() && fs::read_dir(&cache).unwrap().next().is_none());
    assert_eq!(fs::read_dir(&themes).unwrap().count(), 7);
}

/// setup asks nothing about a code editor and shows no line for one (an old `setup.json` that remembers the question an
/// older platoon asked still reads), `setup` takes no arguments, and `platoon plugin`
/// is a command platoon doesn't know: each fails like any other unknown one, and nothing is installed or fetched.
#[test]
fn there_is_no_editor_step_and_no_plugin_command() {
    let (claude, cache) = dirs("editor");
    let base = cache.parent().unwrap().to_path_buf();
    fs::create_dir_all(base.join("config")).unwrap();
    fs::write(base.join("config/setup.json"), r#"{"editor": {"asked": true, "wanted": true}}"#).unwrap();
    let out = platoon(&claude, &cache, &["setup"]);
    assert!(out.status.success(), "{out:?}");
    let shown = text(&out.stdout);
    assert!(!shown.contains("Code editor") && !shown.contains("Install the code editor"), "{shown}");
    fs::write(claude.join("settings.json"), "{}").unwrap();
    let err = text(&platoon(&claude, &cache, &[]).stderr);
    assert!(!err.contains("Code editor") && err.matches("[✗]").count() == 2, "{err}");
    assert!(platoon(&claude, &cache, &["setup"]).status.success());
    let choices: serde_json::Value = serde_json::from_str(&fs::read_to_string(base.join("config/setup.json")).unwrap()).unwrap();
    assert_eq!(choices["editor"], serde_json::json!({"asked": true, "wanted": true}), "the old record is left alone");

    // setup takes no arguments: how to use it.
    for args in [["setup", "--remove", "editor"], ["setup", "--add", "editor"], ["setup", "--replace", "editor"]] {
        let out = platoon(&claude, &cache, &args);
        assert_eq!(out.status.code(), Some(2), "{out:?}");
        assert!(text(&out.stderr).starts_with("usage: platoon setup"), "{out:?}");
    }
    // `platoon plugin ...` is unknown, exactly as a made-up command is.
    let unknown = platoon(&claude, &cache, &["no-such-command"]);
    for args in [&["plugin", "list"][..], &["plugin", "add", "editor"], &["plugin"]] {
        let out = platoon(&claude, &cache, args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {out:?}");
        assert_eq!((&out.stdout, &out.stderr), (&unknown.stdout, &unknown.stderr), "{args:?}");
    }
    assert!(text(&unknown.stderr).starts_with("usage: platoon") && !text(&unknown.stderr).contains("plugin"));
    assert!(!base.join("data").exists() && !base.join("fonts").exists(), "nothing installed anywhere");
}

#[test]
fn version_prints_the_name_and_cargo_version_and_needs_no_setup() {
    let (claude, cache) = dirs("version");
    for flag in ["--version", "-V"] {
        let out = platoon(&claude, &cache, &[flag]);
        assert!(out.status.success(), "{flag}: {}", text(&out.stderr));
        assert_eq!(text(&out.stdout), format!("platoon {}\n", env!("CARGO_PKG_VERSION")), "{flag}");
    }
    assert_eq!(fs::read_dir(&claude).unwrap().count(), 0, "nothing written");
}

#[test]
fn with_only_codex_platoon_starts_without_claude_codes_setup_and_setup_leaves_claudes_settings_alone() {
    let (claude, cache) = dirs("codex-only");
    let codex = cache.parent().unwrap().join("codex");
    fs::create_dir_all(&codex).unwrap();
    // Codex is installed (its folder is there); its program can't run, so setup can't have it trust the hooks.
    let env = without_claude(&[("CODEX_HOME", codex.display().to_string())]);
    let out = platoon_on(&claude, &cache, &env, &["setup"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let shown = text(&out.stdout);
    assert!(shown.contains("Claude Code isn't installed on this computer, so there is nothing to set up for it."), "{shown}");
    assert!(!shown.contains("Status line") && !shown.contains("Session guard"), "no Claude Code steps: {shown}");
    assert!(shown.contains("Codex") && shown.matches("added").count() == 3 && shown.contains("All set"), "{shown}");
    assert!(!claude.join("settings.json").exists(), "Claude Code's settings are left alone");
    assert!(codex.join("hooks.json").exists(), "Codex's notes hooks are in");
    // And platoon starts: no setup gate for a program that isn't there, and Codex's plan in the header.
    let screen = run_in_terminal_on(&claude, &cache, &env, "No sessions yet");
    assert!(screen.contains("No sessions yet") && !screen.contains("one-time setup"), "{screen}");
    assert!(screen.contains("Codex plan: no figure yet") && !screen.contains("plan usage: off"), "{screen}");
}

#[test]
fn with_neither_platoon_and_its_setup_say_plainly_what_to_install() {
    let (claude, cache) = dirs("neither");
    let env = without_claude(&[]);
    for args in [&[][..], &["setup"]] {
        let out = platoon_on(&claude, &cache, &env, args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        let err = text(&out.stderr);
        assert!(err.contains("neither is installed on this computer") && err.contains("Install one of them"), "{args:?}: {err}");
        assert!(err.contains("Claude Code: https://") && err.contains("Codex: https://"), "{err}");
        assert!(text(&out.stdout).is_empty(), "{args:?}");
    }
    assert!(!claude.join("settings.json").exists(), "nothing written");
}
