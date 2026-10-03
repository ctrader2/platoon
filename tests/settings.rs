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

    // A key: down to the first of the numeric settings (the Theme is above it), then right arrow steps 2 hours -> 4 hours,
    // saved straight away.
    t.send(b"\x1b[B");
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
    t.wait_gone("Settings file:"); // the list's footer shows behind the popup, so wait for the popup itself to go
    t.send(b"sq");
    t.wait_gone("Settings file:");
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
    t.wait_gone("Settings file:"); // (esc then q sent together would read as alt+q)
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
    // With this many entries only the selected one is explained at this height: move to the MCP entry, the fourth
    // (the theme is the first).
    t.send(b"\x1b[B\x1b[B\x1b[B");
    t.wait_for("helper programs");
    let text = t.text();
    assert!(text.contains("Ignore MCP servers") && text.contains("MCP servers are"), "{text}");
    assert!(!base.join("config/settings.json").exists(), "looking at the screen writes nothing");

    t.send(b"\x1b[D"); // left: Count MCP servers
    t.wait_for_flat("‹ Count MCP servers ›"); // (the explanation, on screen already, says it too)
    until("the choice to reach the file", || saved_ok(&base) && saved(&base)["idle_stop_ignores_mcp"] == serde_json::json!(false));
    assert_eq!(saved(&base)["idle_stop_minutes"], 120, "the other settings are written as they were");

    // The value itself is a button that goes to the other choice (and back).
    let screen = t.text();
    let (col, row) = find(&screen, "Count MCP servers");
    t.send(format!("\x1b[<0;{};{}M\x1b[<0;{};{}m", col + 2, row + 1, col + 2, row + 1).as_bytes());
    until("the click to reach the file", || saved(&base)["idle_stop_ignores_mcp"] == serde_json::json!(true));
    t.wait_for_flat("‹ Ignore MCP servers ›");

    t.send(b"\x1b");
    t.wait_gone("Settings file:");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn the_theme_in_settings_json_colors_the_running_ui_and_an_edit_applies_without_a_restart() {
    use vt100::Color::Rgb;
    let base = tmp("theme-e2e");
    let env = env(&base);
    let file = base.join("config/settings.json");
    fs::write(&file, r#"{"theme":"nord","keep":"me"}"#).unwrap();
    let mut env = env;
    env.push(("NO_COLOR", String::new())); // the helper sets it, which makes crossterm draw without any color
    let mut t = Term::spawn(&[], &env, &base, 24, 100);
    t.wait_for("enter open");
    t.wait_bg(1, 1, Rgb(0x2e, 0x34, 0x40)); // Nord's background, inside the frame

    // An edit made by hand (or by a picker one day) shows within the normal refresh.
    fs::write(&file, r#"{"theme":"tokyo-night","keep":"me"}"#).unwrap();
    t.wait_bg(1, 1, Rgb(0x1a, 0x1b, 0x26));

    // An id nothing knows shows the default theme, and opening the settings and changing a value keeps the id.
    fs::write(&file, r#"{"theme":"one-i-made-up","keep":"me"}"#).unwrap();
    t.wait_bg(1, 1, Rgb(0x1e, 0x1e, 0x2e));
    t.send(b"s");
    t.wait_for("Stop idle sessions after");
    t.send(b"\x1b[B\x1b[C"); // down from the Theme row, then right
    until("the change to reach the file", || saved(&base)["idle_stop_minutes"] == 240);
    assert_eq!(saved(&base)["theme"], "one-i-made-up");
    assert_eq!(saved(&base)["keep"], "me");
    t.send(b"\x1b");
    t.wait_gone("Settings file:");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn without_truecolor_the_ui_is_drawn_in_256_colors_and_terminal_colors_uses_the_terminals_own() {
    use vt100::Color::{Default, Idx};
    let base = tmp("theme-256-e2e");
    let mut env = env(&base);
    env.push(("PLATOON_COLORS", "256".into())); // later entries override the helper's truecolor
    env.push(("NO_COLOR", String::new())); // the helper sets it, which makes crossterm draw without any color
    fs::write(base.join("config/settings.json"), r#"{"theme":"nord"}"#).unwrap();
    let mut t = Term::spawn(&[], &env, &base, 24, 100);
    t.wait_for("enter open");
    t.wait_bg(1, 1, Idx(237)); // 2e3440 is nearest the gray ramp's 58,58,58
    t.send(b"q");
    t.wait_exit();

    fs::write(base.join("config/settings.json"), r#"{"theme":"terminal"}"#).unwrap();
    let mut t = Term::spawn(&[], &env, &base, 24, 100);
    t.wait_for("enter open");
    assert_eq!(t.bg_at(1, 1), Default, "the terminal's own background");
    t.send(b"q");
    t.wait_exit();
}

#[test]
fn the_theme_list_previews_on_the_real_screen_esc_puts_it_back_and_enter_saves_a_user_theme() {
    use vt100::Color::Rgb;
    let base = tmp("theme-list-e2e");
    let mut env = env(&base);
    env.push(("NO_COLOR", String::new())); // the helper sets it, which makes crossterm draw without any color
    fs::create_dir_all(base.join("config/themes")).unwrap();
    fs::write(base.join("config/themes/Deep Sea"), "background = #0a1622\nforeground = #dde6ee\npalette = 12=#4fb0ff\n").unwrap();
    fs::write(base.join("config/themes/Bright Day"), "background = #ffffff\nforeground = #000000\n").unwrap();
    let file = base.join("config/settings.json");
    fs::write(&file, r#"{"keep":{"me":[1,2]}}"#).unwrap();
    let mocha = (Rgb(0x1e, 0x1e, 0x2e), Rgb(0xb4, 0xbe, 0xfe)); // background, and the header's accent
    let macchiato = (Rgb(0x24, 0x27, 0x3a), Rgb(0xb7, 0xbd, 0xf8));
    let mut t = Term::spawn(&[], &env, &base, 30, 100);
    t.wait_for("enter open");
    t.wait_bg(1, 1, mocha.0);
    t.wait_fg(1, 3, mocha.1); // the "p" of the header's title

    // s, then Enter on the Theme row (the first): the list opens beside the real screen, which stays as it was.
    t.send(b"s");
    t.wait_for("Settings file:");
    t.send(b"\r");
    t.wait_for("sample");
    t.wait_for("1 theme file skipped");
    let screen = t.text();
    // (The user's own theme is at the end of the list, past the built-ins: it is seen below, after jumping there.)
    assert!(screen.contains("Bright Day:"), "{screen}");
    t.wait_fg(1, 3, mocha.1); // not faded: still the accent, not the muted border color

    // Down previews the next theme on the whole screen: the frame's inside and the header's title change...
    t.send(b"\x1b[B");
    t.wait_bg(1, 1, macchiato.0);
    t.wait_fg(1, 3, macchiato.1);
    assert!(!saved_theme_key(&file), "previewing saves nothing:\n{}", fs::read_to_string(&file).unwrap());

    // ...and Esc puts everything back.
    t.send(b"\x1b");
    t.wait_gone("sample");
    t.wait_bg(1, 1, mocha.0);
    t.wait_fg(1, 3, Rgb(0x45, 0x47, 0x5a)); // the settings popup is back, and fades the list behind it to Mocha's border color
    assert!(!saved_theme_key(&file));

    // Enter keeps: open again, jump to the last entry (the user's theme), see it, keep it.
    t.send(b"\r");
    t.wait_for("sample");
    t.send(b"G");
    t.wait_bg(1, 1, Rgb(0x0a, 0x16, 0x22));
    t.wait_for_all(&["Deep Sea", "yours"]);
    t.send(b"\r");
    t.wait_gone("sample");
    until("the theme to reach the file", || {
        fs::read_to_string(&file)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .is_some_and(|v| v["theme"] == "user:deep-sea")
    });
    assert_eq!(saved(&base)["keep"], serde_json::json!({"me":[1,2]}), "the rest of the file is kept");
    t.wait_bg(1, 1, Rgb(0x0a, 0x16, 0x22));
    t.wait_for("Deep Sea"); // the Theme row now shows it

    t.send(b"\x1b");
    t.wait_gone("Settings file:");
    t.send(b"q");
    t.wait_exit();
    // A second run starts in it.
    let mut t = Term::spawn(&[], &env, &base, 30, 100);
    t.wait_for("enter open");
    t.wait_bg(1, 1, Rgb(0x0a, 0x16, 0x22));
    t.send(b"q");
    t.wait_exit();
}

fn saved_theme_key(file: &std::path::Path) -> bool {
    serde_json::from_str::<serde_json::Value>(&fs::read_to_string(file).unwrap()).unwrap().get("theme").is_some()
}

/// Column of `needle` on screen row `row`, counted in cells.
fn col_on(screen: &str, row: u16, needle: &str) -> u16 {
    let line = screen.lines().nth(row as usize).unwrap_or_default();
    let at = line.find(needle).unwrap_or_else(|| panic!("no {needle:?} on row {row}:\n{screen}"));
    line[..at].chars().count() as u16
}

fn click(t: &mut Term, row: u16, col: u16) {
    t.send(format!("\x1b[<0;{};{}M\x1b[<0;{};{}m", col + 1, row + 1, col + 1, row + 1).as_bytes());
}

#[test]
fn the_tabs_switch_by_key_and_by_click_are_lit_and_a_change_on_another_tab_is_saved_at_80_by_24() {
    use vt100::Color::Rgb;
    let base = tmp("settings-e2e-tabs");
    let mut env = env(&base);
    env.push(("NO_COLOR", String::new())); // the helper sets it, which makes crossterm draw without any color
    let file = base.join("config/settings.json");
    fs::write(&file, r#"{"keep":"me","auto_archive_days":0}"#).unwrap();
    // Catppuccin Mocha: the raised surface and the text on it for the tab showing; the plain background and the
    // muted text for the others.
    let (surface, bg, text, dim) = (Rgb(0x31, 0x32, 0x44), Rgb(0x1e, 0x1e, 0x2e), Rgb(0xcd, 0xd6, 0xf4), Rgb(0x6c, 0x70, 0x86));
    let mut t = Term::spawn(&[], &env, &base, 24, 80);
    t.wait_for("enter open");
    t.send(b"s");
    // At 80 columns the seven titles only fit with "Smart features" shortened to its first word.
    t.wait_for_all(&[
        "General",
        " Smart ",
        "Sessions   Projects",
        "Editors   Plugins",
        "Stop idle sessions after",
        "Settings file:",
        "ctrl+space back",
    ]);
    let (row, _) = t.find("General").unwrap();
    let screen = t.text();
    let col = |name: &str| col_on(&screen, row, name);
    let (general, search, smart, projects) = (col("General"), col("Search"), col(" Smart ") + 1, col("Projects"));
    t.wait_bg(row, general, surface);
    t.wait_fg(row, general, text);
    assert_eq!((t.bg_at(row, search), t.fg_at(row, search)), (bg, dim), "a tab not showing is quiet");

    // Tab goes to the next tab, lit in its turn; shift+tab comes back.
    t.send(b"\t");
    t.wait_for_all(&["Search archived", "Description search: wider search"]);
    t.wait_gone("Stop idle sessions after");
    assert_eq!(t.find("General").unwrap().0, row, "the tabs stay where they are");
    t.wait_bg(row, search, surface);
    t.wait_bg(row, general, bg);
    assert_eq!(t.fg_at(row, general), dim);
    t.send(b"\x1b[Z");
    t.wait_for("Stop idle sessions after");
    t.wait_gone("Search archived");
    t.wait_bg(row, general, surface);

    // A click on a title shows that tab: Projects, where right steps the first entry 3 -> 5 minutes, saved at once.
    click(&mut t, row, projects + 2);
    t.wait_for_all(&["Update notes after", "Read Claude's last reply up to", "3 minutes"]);
    t.wait_bg(row, projects, surface);
    t.send(b"\x1b[C");
    t.wait_for("5 minutes");
    until("the change to reach the file", || saved_ok(&base) && saved(&base)["notes_quiet_minutes"] == 5);
    assert_eq!(saved(&base)["keep"], "me", "keys platoon doesn't know are kept");

    // The smart features tab carries the day's token count.
    click(&mut t, row, smart + 2);
    t.wait_for_all(&[
        "Enable features which require tokens",
        "Model for smart features",
        "Used today: 0 of 500,000 tokens",
        "ctrl+space back",
    ]);
    t.wait_bg(row, smart, surface);
    // At 80x24 the whole popup fits: its frame, the tabs, the file and the hints, and no text touches the border.
    let whole = t.stable();
    for want in ["╭", "╰", "General", "Projects", "Settings file:", "ctrl+space back", "Used today"] {
        assert!(whole.contains(want), "{want:?} missing:\n{whole}");
    }
    let inside: Vec<&str> = whole.lines().filter(|l| l.starts_with("│  │")).collect();
    assert!(inside.len() >= 15, "{whole}");
    for line in inside {
        // Drop the outer frame, then the popup's own right border: what is left ends in a blank.
        let popup = line.trim_end_matches('│').trim_end().strip_suffix('│').unwrap_or_else(|| panic!("the popup's border is cut: {line}"));
        assert!(popup.ends_with(' '), "text touches the border: {line}");
    }

    // Esc closes the settings from any tab.
    t.send(b"\x1b");
    t.wait_gone("Settings file:");
    t.send(b"q");
    t.wait_exit();
}
