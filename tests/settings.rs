//! The settings screen end to end, in a real terminal with the real binary. Every directory platoon
//! touches is a temp dir, and no session is started, so there is nothing to clean up afterwards.

mod common;
use common::*;
use std::fs;

fn env(base: &std::path::Path) -> Vec<(&'static str, String)> {
    let dir = |name: &str| {
        let d = base.join(name);
        fs::create_dir_all(&d).unwrap();
        d.to_string_lossy().into_owned()
    };
    fs::create_dir_all(base.join("claude/sessions")).unwrap();
    vec![
        ("CLAUDE_CONFIG_DIR", base.join("claude").to_string_lossy().into()),
        ("PLATOON_CACHE_DIR", dir("cache")),
        ("PLATOON_CONFIG_DIR", dir("config")),
        ("PLATOON_SKIP_SETUP_CHECK", "1".into()),
    ]
}

fn saved(base: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(base.join("config/settings.json")).unwrap()).unwrap()
}

/// (column, row), 0-based, of the first `needle` on the screen.
fn find(screen: &str, needle: &str) -> (usize, usize) {
    screen
        .lines()
        .enumerate()
        .find_map(|(row, line)| line.find(needle).map(|byte| (line[..byte].chars().count(), row)))
        .unwrap_or_else(|| panic!("no {needle:?} on the screen:\n{screen}"))
}

#[test]
fn s_opens_the_settings_a_change_is_saved_at_once_and_the_rest_of_the_file_is_kept() {
    let base = tmp("settings-e2e");
    let env = env(&base);
    let file = base.join("config/settings.json");
    fs::write(&file, r#"{"idle_stop_minutes":120,"auto_archive_days":0,"keep":{"me":[1,2]}}"#).unwrap();
    let mut t = Term::spawn(&[], &env, &base, 30, 100);
    t.wait_for("enter open");
    assert!(t.text().contains("s settings"), "{}", t.text());

    t.send(b"s");
    t.wait_for("Stop idle sessions after");
    t.wait_for("Changes are saved and apply immediately."); // the whole screen, not a half-drawn one
    let text = t.text();
    assert!(text.contains("2 hours") && text.contains("Off") && text.contains("Archive conversations after"), "{text}");
    assert!(text.contains("Changes are saved and apply immediately."), "{text}");
    assert!(text.contains("settings.json"), "the file's location is shown:\n{text}");
    assert_eq!(saved(&base)["idle_stop_minutes"], 120, "opening the screen writes nothing");

    // A key: right arrow steps 2 hours -> 4 hours, saved straight away.
    t.send(b"\x1b[C");
    t.wait_for("4 hours");
    until("the change to reach the file", || saved(&base)["idle_stop_minutes"] == 240);
    assert_eq!(saved(&base)["keep"], serde_json::json!({"me":[1,2]}), "keys platoon doesn't know are kept");
    assert_eq!(saved(&base)["auto_archive_days"], 0);

    // The mouse: click the `›` on the second setting (Off -> 1 day). Terminal coordinates are 1-based.
    let screen = t.text();
    let (col, row) = {
        let (_, second) = find(&screen, "Archive conversations after");
        let line: Vec<&str> = screen.lines().collect();
        (line[second].chars().position(|c| c == '›').expect("a next button"), second)
    };
    t.send(format!("\x1b[<0;{};{}M\x1b[<0;{};{}m", col + 1, row + 1, col + 1, row + 1).as_bytes());
    until("the click to reach the file", || saved(&base)["auto_archive_days"] == 1);
    t.wait_for("1 day");

    // Back to the list with esc; q on the settings screen also goes back rather than quitting.
    t.send(b"\x1b");
    t.wait_for("enter open");
    t.send(b"sq");
    t.wait_for("enter open");
    t.send(b"q");
    t.wait_exit();
    assert_eq!(saved(&base)["idle_stop_minutes"], 240);
}

#[test]
fn a_damaged_file_opens_on_the_standard_values_and_the_first_change_keeps_a_copy() {
    let base = tmp("settings-e2e-corrupt");
    let env = env(&base);
    fs::write(base.join("config/settings.json"), "{ oops").unwrap();
    let mut t = Term::spawn(&[], &env, &base, 30, 100);
    t.wait_for("enter open");
    t.send(b"s");
    t.wait_for("couldn't be read");
    assert_eq!(fs::read_to_string(base.join("config/settings.json")).unwrap(), "{ oops");
    t.send(b"l");
    until("a fresh file", || base.join("config/settings.json").exists() && saved_ok(&base));
    assert_eq!(fs::read_to_string(base.join("config/settings.json.bak")).unwrap(), "{ oops");
    t.send(b"\x1b");
    t.wait_for("enter open"); // (esc then q sent together would read as alt+q)
    t.send(b"q");
    t.wait_exit();
}

fn saved_ok(base: &std::path::Path) -> bool {
    fs::read_to_string(base.join("config/settings.json")).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()).is_some()
}

#[test]
fn the_mcp_choice_is_listed_explained_and_saved_from_the_real_screen_at_80_columns() {
    let base = tmp("settings-e2e-mcp");
    let env = env(&base);
    let mut t = Term::spawn(&[], &env, &base, 30, 80);
    t.wait_for("enter open");
    t.send(b"s");
    t.wait_for("Idle stop and MCP servers");
    t.wait_for("Changes are saved and apply immediately.");
    let text = t.text();
    assert!(text.contains("Ignore MCP servers") && text.contains("helper programs") && text.contains("MCP servers are"), "{text}");
    assert!(!base.join("config/settings.json").exists(), "looking at the screen writes nothing");

    t.send(b"jj"); // the third entry
    t.send(b"\x1b[D"); // left: Count MCP servers
    t.wait_for("Count MCP servers");
    until("the choice to reach the file", || saved_ok(&base) && saved(&base)["idle_stop_ignores_mcp"] == serde_json::json!(false));
    assert_eq!(saved(&base)["idle_stop_minutes"], 120, "the other settings are written as they were");

    // The value itself is a button that goes to the other choice (and back).
    let screen = t.text();
    let (col, row) = find(&screen, "Count MCP servers");
    t.send(format!("\x1b[<0;{};{}M\x1b[<0;{};{}m", col + 2, row + 1, col + 2, row + 1).as_bytes());
    until("the click to reach the file", || saved(&base)["idle_stop_ignores_mcp"] == serde_json::json!(true));
    t.wait_for("Ignore MCP servers");

    t.send(b"\x1b");
    t.wait_for("enter open");
    t.send(b"q");
    t.wait_exit();
}
