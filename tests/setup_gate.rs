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

fn platoon(claude: &PathBuf, cache: &PathBuf, args: &[&str]) -> std::process::Output {
    Command::new(BIN)
        .args(args)
        .env("CLAUDE_CONFIG_DIR", claude)
        .env("PLATOON_CACHE_DIR", cache)
        // Never the real one: `platoon themes` reads the user's theme files from here.
        .env("PLATOON_CONFIG_DIR", cache.parent().unwrap().join("config"))
        // Nor the real editor folder or fonts, and never the network: downloads come from an empty mirror.
        .env("PLATOON_DATA_DIR", cache.parent().unwrap().join("data"))
        .env("PLATOON_FONT_DIR", cache.parent().unwrap().join("fonts"))
        .env("PLATOON_EDITOR_MIRROR", cache.parent().unwrap().join("mirror"))
        .env("PLATOON_SKIP_FC_CACHE", "1")
        // ...nor the plugin catalog built into platoon: a catalog folder that isn't there.
        .env("PLATOON_PLUGIN_CATALOG", cache.parent().unwrap().join("no-plugin-catalog"))
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
    assert!(
        shown.contains("added") && shown.contains("Saved") && shown.contains("Project notes") && shown.contains("Notes updates"),
        "{shown}"
    );
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
    assert!(text_on_disk.starts_with("---\nname: to-tickets\n") && !text_on_disk.contains("disable-model-invocation"), "{text_on_disk}");
    // Claude may offer it, but only after asking, only in a platoon project, once, and after a no it only says how to
    // type it.
    let desc = text_on_disk.lines().find(|l| l.starts_with("description: ")).unwrap();
    for want in [
        "Never start it unasked",
        "only after a yes",
        "belongs to a platoon project",
        "ask once",
        "After a no",
        "type /to-tickets any time",
        "do not offer again",
    ] {
        assert!(desc.contains(want), "{want:?} in {desc}");
    }
    // Written by the platoon before this one, which kept no record (so there is none): platoon's own, so it is updated
    // quietly.
    fs::remove_file(cache.parent().unwrap().join("config/setup.json")).unwrap();
    fs::write(&skill, include_str!("fixtures/to-tickets-skill-1623a0e.md")).unwrap();
    let shown = text(&platoon(&claude, &cache, &["setup"]).stdout);
    assert!(shown.contains("updated: platoon's copy had changed"), "{shown}");
    assert_eq!(fs::read_to_string(&skill).unwrap(), text_on_disk);
    assert!(!claude.join("skills/to-tickets/SKILL.md.platoon-bak").exists());
    // Written by the platoon before this one (`d8e7d22`, typed only), which did keep a record: the record says so, so
    // it is updated quietly too.
    let before = include_str!("fixtures/to-tickets-skill-d8e7d22.md");
    assert_ne!(before, text_on_disk, "platoon's copy has moved on since");
    fs::write(&skill, before).unwrap();
    fs::write(cache.parent().unwrap().join("config/setup.json"), serde_json::json!({"to-tickets": {"written": fnv(before)}}).to_string())
        .unwrap();
    let shown = text(&platoon(&claude, &cache, &["setup"]).stdout);
    assert!(shown.contains("updated: platoon's copy had changed"), "{shown}");
    assert_eq!(fs::read_to_string(&skill).unwrap(), text_on_disk);
    assert!(!claude.join("skills/to-tickets/SKILL.md.platoon-bak").exists());
    // The same old copy with a line of the user's: theirs, left alone.
    fs::write(&skill, format!("{before}\nMy line.\n")).unwrap();
    fs::write(cache.parent().unwrap().join("config/setup.json"), serde_json::json!({"to-tickets": {"written": fnv(before)}}).to_string())
        .unwrap();
    assert!(text(&platoon(&claude, &cache, &["setup"]).stdout).contains("changed by you"));
    assert!(fs::read_to_string(&skill).unwrap().ends_with("My line.\n"));
    fs::write(&skill, &text_on_disk).unwrap();
    // Taken out on request: only it.
    let settings = fs::read_to_string(claude.join("settings.json")).unwrap();
    let out = platoon(&claude, &cache, &["setup", "--remove", "to-tickets"]);
    assert!(out.status.success(), "{out:?}");
    let said = text(&out.stdout);
    assert!(said.starts_with("Removed the /to-tickets skill") && said.contains("platoon setup --add to-tickets"), "{said}");
    assert!(!claude.join("skills/to-tickets").exists());
    assert_eq!(fs::read_to_string(claude.join("settings.json")).unwrap(), settings);
    // Without it platoon still starts.
    let screen = run_in_terminal(&claude, &cache, "No conversations yet");
    assert!(screen.contains("No conversations yet"), "{screen}");
    let out = platoon(&claude, &cache, &["setup", "--remove", "everything"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("can remove: to-tickets"));
    assert_eq!(platoon(&claude, &cache, &["setup", "--nope"]).status.code(), Some(2));
    assert_eq!(platoon(&claude, &cache, &["setup", "--add"]).status.code(), Some(2));
}

/// Every place the skill can be in, through the real binary: what `platoon setup` says and does, and the three options.
#[test]
fn setup_remembers_a_removal_and_leaves_a_skill_the_user_changed_alone() {
    let (claude, cache) = dirs("skill-choices");
    let config = cache.parent().unwrap().join("config");
    let skill = claude.join("skills/to-tickets/SKILL.md");
    let backup = claude.join("skills/to-tickets/SKILL.md.platoon-bak");
    let setup = || text(&platoon(&claude, &cache, &["setup"]).stdout);
    let tickets_line = |shown: &str| -> String {
        let at = shown.find("Tickets skill").unwrap_or_else(|| panic!("{shown}"));
        shown[at..].lines().take(2).collect::<Vec<_>>().join("\n")
    };
    // Missing: written.
    assert!(tickets_line(&setup()).contains("added"));
    let ours = fs::read_to_string(&skill).unwrap();
    // Unchanged: already done.
    assert!(tickets_line(&setup()).contains("already done"));

    // Removed by the user: gone, remembered in platoon's own config (other keys kept), and setup leaves it out.
    fs::create_dir_all(&config).unwrap();
    let choices = config.join("setup.json");
    fs::write(&choices, "{\"someone-elses\": [1, 2]}").unwrap();
    assert!(platoon(&claude, &cache, &["setup", "--remove", "to-tickets"]).status.success());
    let saved: serde_json::Value = serde_json::from_str(&fs::read_to_string(&choices).unwrap()).unwrap();
    assert_eq!(saved, serde_json::json!({"someone-elses": [1, 2], "to-tickets": {"removed": true}}));
    for _ in 0..2 {
        let shown = setup();
        let line = tickets_line(&shown);
        assert!(
            line.starts_with("Tickets skill")
                && line.contains("removed by you")
                && line.contains("`platoon setup --add to-tickets` puts it back"),
            "{shown}"
        );
        assert!(!skill.exists() && shown.contains("Nothing to change"), "{shown}");
    }
    // Put back: written, and the mark is gone.
    let out = platoon(&claude, &cache, &["setup", "--add", "to-tickets"]);
    assert!(out.status.success() && text(&out.stdout).starts_with("Added the /to-tickets skill"), "{out:?}");
    assert_eq!(fs::read_to_string(&skill).unwrap(), ours);
    let saved: serde_json::Value = serde_json::from_str(&fs::read_to_string(&choices).unwrap()).unwrap();
    assert_eq!(saved["someone-elses"], serde_json::json!([1, 2]));
    assert!(saved["to-tickets"].get("removed").is_none() && saved["to-tickets"]["written"].is_string(), "{saved}");
    assert!(tickets_line(&setup()).contains("already done"));

    // What platoon last wrote, untouched, while platoon's copy has moved on: updated quietly.
    let older = "---\nname: to-tickets\n---\nan older copy platoon wrote\n";
    fs::write(&skill, older).unwrap();
    let mut rec = saved.clone();
    rec["to-tickets"]["written"] = serde_json::json!(fnv(older));
    fs::write(&choices, rec.to_string()).unwrap();
    assert!(tickets_line(&setup()).contains("updated: platoon's copy had changed"));
    assert_eq!(fs::read_to_string(&skill).unwrap(), ours);
    assert!(!backup.exists());

    // Changed by the user: left alone every time, with how to replace it; never blocking.
    fs::write(&skill, format!("{ours}\nMy own line.\n")).unwrap();
    for _ in 0..2 {
        let line = tickets_line(&setup());
        assert!(line.contains("changed by you") && line.contains("`platoon setup --replace to-tickets`"), "{line}");
        assert!(fs::read_to_string(&skill).unwrap().ends_with("My own line.\n"));
    }
    let screen = run_in_terminal(&claude, &cache, "No conversations yet");
    assert!(screen.contains("No conversations yet"), "{screen}");
    // Replaced on request: theirs kept beside it.
    let out = platoon(&claude, &cache, &["setup", "--replace", "to-tickets"]);
    assert!(out.status.success() && text(&out.stdout).contains("yours is kept as"), "{out:?}");
    assert_eq!(fs::read_to_string(&skill).unwrap(), ours);
    assert!(fs::read_to_string(&backup).unwrap().ends_with("My own line.\n"));
    assert!(tickets_line(&setup()).contains("already done"));

    // No record at all (a copy from somewhere else) that differs from platoon's: the user's.
    fs::remove_file(&choices).unwrap();
    fs::write(&skill, "---\nname: to-tickets\n---\nsomething else\n").unwrap();
    assert!(tickets_line(&setup()).contains("changed by you"));
    assert_eq!(fs::read_to_string(&skill).unwrap(), "---\nname: to-tickets\n---\nsomething else\n");
}

/// The hash setup keeps (FNV-1a over the exact bytes), to write a record by hand.
fn fnv(text: &str) -> String {
    format!("{:016x}", text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)))
}

/// Runs `platoon` in a real terminal; returns the screen text once it shows `wait_for`, then quits it.
fn run_in_terminal(claude: &PathBuf, cache: &PathBuf, wait_for: &str) -> String {
    let pair = native_pty_system().openpty(PtySize { rows: 24, cols: 90, pixel_width: 0, pixel_height: 0 }).unwrap();
    let mut cmd = CommandBuilder::new(BIN);
    cmd.env("CLAUDE_CONFIG_DIR", claude);
    cmd.env("PLATOON_CACHE_DIR", cache);
    cmd.env("PLATOON_CONFIG_DIR", cache.join("config"));
    cmd.env("PLATOON_DATA_DIR", cache.parent().unwrap().join("data"));
    cmd.env("PLATOON_FONT_DIR", cache.parent().unwrap().join("fonts"));
    cmd.env("PLATOON_EDITOR_MIRROR", cache.parent().unwrap().join("mirror"));
    cmd.env("PLATOON_PLUGIN_CATALOG", cache.parent().unwrap().join("no-plugin-catalog"));
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
    fs::write(claude.join("settings.json"), r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"say-something"}]}]}}"#).unwrap();
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

/// The code editor is a plugin now: setup neither asks about it nor shows a line for it (an old `setup.json` that
/// remembers the old question still reads), and `setup --add/--remove editor` say so and do the plugin's add/remove
/// (here with an empty catalog in place of the built-in one: said plainly, nothing installed; never the network).
#[test]
fn the_code_editor_is_a_plugin_now_and_setups_add_and_remove_say_so() {
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

    let out = platoon(&claude, &cache, &["setup", "--add", "editor"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(text(&out.stdout).starts_with("The code editor is a plugin now: this is `platoon plugin add editor`."), "{out:?}");
    assert!(text(&out.stderr).contains("There is no plugin called \"editor\" in the catalog"), "{out:?}");
    assert!(
        !base.join("data/editor").exists() && !base.join("data/plugins/editor").exists() && !base.join("fonts").exists(),
        "nothing installed"
    );
    let out = platoon(&claude, &cache, &["setup", "--remove", "editor"]);
    assert!(out.status.success(), "{out:?}");
    assert!(
        text(&out.stdout).contains("this is `platoon plugin remove editor`")
            && text(&out.stdout).contains("isn't installed; there is nothing to remove"),
        "{out:?}"
    );
    // `--replace` is only for the skill.
    let out = platoon(&claude, &cache, &["setup", "--replace", "editor"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("can replace: to-tickets"), "{out:?}");
}
