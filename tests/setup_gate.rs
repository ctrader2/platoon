//! `platoon` must not start until `platoon setup` has been run, and setup must be safely repeatable.

mod common;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
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

fn platoon(claude: &PathBuf, cache: &PathBuf, args: &[&str]) -> std::process::Output {
    Command::new(BIN)
        .args(args)
        .env("CLAUDE_CONFIG_DIR", claude)
        .env("PLATOON_CACHE_DIR", cache)
        // Never the real one: `platoon themes` reads the user's theme files from here.
        .env("PLATOON_CONFIG_DIR", cache.parent().unwrap().join("config"))
        .env("NO_COLOR", "1")
        .env_remove("PLATOON_SKIP_SETUP_CHECK")
        .output()
        .unwrap()
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
    assert_eq!(shown.matches("[✓]").count(), 5, "{shown}");
    assert!(shown.contains("added") && shown.contains("Saved") && shown.contains("Project notes") && shown.contains("Notes updates"), "{shown}");
    let settings = fs::read_to_string(claude.join("settings.json")).unwrap();

    let second = platoon(&claude, &cache, &["setup"]);
    assert!(second.status.success());
    let shown = text(&second.stdout);
    assert_eq!(shown.matches("[✓]").count(), 5, "{shown}");
    assert_eq!(shown.matches("already done").count(), 5, "{shown}");
    assert!(shown.contains("Nothing to change"), "{shown}");
    assert_eq!(fs::read_to_string(claude.join("settings.json")).unwrap(), settings, "a repeat run must not touch the file");
}

#[test]
fn setup_writes_the_to_tickets_skill_into_claudes_config_folder_and_can_take_it_out_again() {
    let (claude, cache) = dirs("skill");
    let out = platoon(&claude, &cache, &["setup"]);
    assert!(out.status.success());
    let shown = text(&out.stdout);
    let skill = claude.join("skills/to-tickets/SKILL.md");
    assert!(shown.contains("Tickets skill") && shown.contains(&format!("Wrote {}", skill.display())), "{shown}");
    let text_on_disk = fs::read_to_string(&skill).unwrap();
    assert!(text_on_disk.starts_with("---\nname: to-tickets\n") && text_on_disk.contains("disable-model-invocation: true"), "{text_on_disk}");
    // Out of date (an older platoon wrote it): setup says so and updates it.
    fs::write(&skill, "---\nname: to-tickets\n---\nold\n").unwrap();
    let shown = text(&platoon(&claude, &cache, &["setup"]).stdout);
    assert!(shown.contains("updated: platoon's copy had changed"), "{shown}");
    assert_eq!(fs::read_to_string(&skill).unwrap(), text_on_disk);
    // Taken out on request: only it.
    let settings = fs::read_to_string(claude.join("settings.json")).unwrap();
    let out = platoon(&claude, &cache, &["setup", "--remove", "to-tickets"]);
    assert!(out.status.success(), "{out:?}");
    assert!(text(&out.stdout).starts_with("Removed the /to-tickets skill"), "{out:?}");
    assert!(!claude.join("skills/to-tickets").exists());
    assert_eq!(fs::read_to_string(claude.join("settings.json")).unwrap(), settings);
    // Without it platoon still starts; the next setup adds it back.
    let screen = run_in_terminal(&claude, &cache, "No conversations yet");
    assert!(screen.contains("No conversations yet"), "{screen}");
    let out = platoon(&claude, &cache, &["setup", "--remove", "everything"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("can remove: to-tickets"));
    assert_eq!(platoon(&claude, &cache, &["setup", "--nope"]).status.code(), Some(2));
    let shown = text(&platoon(&claude, &cache, &["setup"]).stdout);
    assert!(shown.contains("Tickets skill") && shown.contains("added") && skill.is_file(), "{shown}");
}

/// Runs `platoon` in a real terminal; returns the screen text once it shows `wait_for`, then quits it.
fn run_in_terminal(claude: &PathBuf, cache: &PathBuf, wait_for: &str) -> String {
    let pair = native_pty_system().openpty(PtySize { rows: 24, cols: 90, pixel_width: 0, pixel_height: 0 }).unwrap();
    let mut cmd = CommandBuilder::new(BIN);
    cmd.env("CLAUDE_CONFIG_DIR", claude);
    cmd.env("PLATOON_CACHE_DIR", cache);
    cmd.env("PLATOON_CONFIG_DIR", cache.join("config"));
    cmd.env("TERM", "xterm-256color");
    cmd.env_remove("PLATOON_SKIP_SETUP_CHECK");
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

    let screen = run_in_terminal(&claude, &cache, "No conversations yet");
    assert!(screen.contains("No conversations yet"), "the UI should have started:\n{screen}");

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
    let screen = run_in_terminal(&claude, &cache, "No conversations yet");
    assert!(screen.contains("No conversations yet"), "the missing notes hook must not block starting:\n{screen}");
    let out = platoon(&claude, &cache, &["setup"]);
    let shown = text(&out.stdout);
    assert_eq!(shown.matches("already done").count(), 4, "{shown}");
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
    fs::write(
        claude.join("settings.json"),
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"say-something"}]}]}}"#,
    )
    .unwrap();
    assert!(platoon(&claude, &cache, &["setup"]).status.success());
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(claude.join("settings.json")).unwrap()).unwrap();
    let groups = v["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(groups.len(), 2, "{v}");
    assert_eq!(groups[0]["hooks"][0]["command"], "say-something");
    assert!(groups[1]["hooks"][0]["command"].as_str().unwrap().ends_with(" hook stop"), "{v}");
    // Running the hook by hand with nothing to do is instant, silent and fine.
    let out = std::process::Command::new(BIN)
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
            "catppuccin-mocha", "catppuccin-macchiato", "tokyo-night", "dracula", "one-dark", "nord", "gruvbox-dark", "rose-pine",
            "kanagawa-wave", "night-owl", "ayu-mirage", "vesper", "claude", "terminal"
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
    let ids: Vec<&str> = shown.lines().filter(|l| !l.starts_with(' ') && l.contains("  ") && !l.contains("skipped")).filter_map(|l| l.split_whitespace().next()).collect();
    assert_eq!(&ids[ids.len() - 2..], ["user:deep-sea", "user:nord"], "the built-ins first, then the user's own in file-name order:\n{shown}");
    assert_eq!(ids[0], "catppuccin-mocha");
    assert!(shown.contains("user:deep-sea") && shown.contains("Deep Sea  (yours)"), "{shown}");
    assert!(shown.contains("Nord (yours)") && !shown.contains("Nord (yours)  (yours)"), "a name a built-in has is marked once: {shown}");
    assert!(shown.contains("bg #0a1622"), "the roles are listed like a built-in's:\n{shown}");
    assert!(shown.contains("3 theme files skipped in "), "{shown}");
    for want in ["Nord Light: light themes aren't supported", "Half: no background color", "Noise: the file isn't text"] {
        assert!(shown.lines().any(|l| l.trim() == want), "{want:?} in:\n{shown}");
    }
    assert!(!shown.contains("A Folder") && !shown.contains('\x1b'));
    // Nothing is written anywhere, and the folder the user made is untouched.
    assert!(fs::read_dir(&claude).unwrap().next().is_none() && fs::read_dir(&cache).unwrap().next().is_none());
    assert_eq!(fs::read_dir(&themes).unwrap().count(), 6);
}
