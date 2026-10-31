//! The setup notice in the app (`setupnotice.rs`, `App::news_showing`): `platoon setup` has something to add outside
//! the start-up gate (the `/hand-to-squad` skill missing, platoon's copy out of date or only under an old name), said once on its own line above
//! the footer of the Sessions and Projects lists, where the version notice shows, and put away by the next key or click
//! like it; remembered once it has shown, gone by itself once the skill is in, and never for a skill of the user's or
//! where Claude Code isn't in use. Drawn the way a terminal shows it (TestBackend). Every folder is a temp dir
//! (`World`); nothing here starts a process.

use crate::agents::{Agent, CodexAt, Providers};
use crate::app::{App, TOP_SESSIONS, TOP_TASKS};
use crate::testkit::World;
use crate::{settings, setup, skill, ui};
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
};
use std::fs;

const NEW: &str = "New: /hand-to-squad in Claude Code sessions — run `platoon setup` to add it";
const UPDATE: &str = "platoon's /hand-to-squad skill has an update — run `platoon setup`";
const VERSION: &str = "Claude Code 2.2.0 is newer than this platoon was checked with (2.1.288). If something looks wrong, update platoon.";
const NONE: [&str; 0] = [];

fn shown(app: &App, w: u16, h: u16) -> Vec<String> {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect()).collect()
}

fn words(lines: &[String]) -> String {
    lines.join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn world(name: &str) -> World {
    let w = World::new(name);
    for i in 0..6u64 {
        w.transcript(&format!("conv-{i}"), &format!("/srv/project-{i}"), &format!("prompt number {i}"), 10 + i);
    }
    w
}

/// This platoon's path, as setup writes it into the skill (the test binary's).
fn exe() -> String {
    setup::this_program().unwrap().0
}

/// An app that has looked for the setup notice as `App::new` does, and refreshed once.
fn looked(w: &World) -> App {
    let mut app = w.app();
    app.notice = None;
    app.look_for_setup_news();
    app.refresh();
    app
}

fn key(app: &mut App, c: char) {
    app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
}

/// The index of the line `first_words` starts.
fn line_of(lines: &[String], first_words: &str) -> usize {
    lines.iter().position(|l| l.trim_start().starts_with(first_words)).unwrap_or_else(|| {
        panic!("{first_words:?} does not start a line:\n{}", lines.join("\n"));
    })
}

#[test]
fn a_missing_skill_is_said_on_its_own_line_just_above_the_footer_whole_at_110_and_80_columns() {
    let w = world("sn-render");
    let app = looked(&w);
    assert_eq!(app.news_showing(), [NEW]);
    for width in [110u16, 80] {
        let lines = shown(&app, width, 24);
        println!("{width}x24:\n{}\n", lines.join("\n"));
        let at = line_of(&lines, "New: /hand-to-squad");
        assert_eq!(at, lines.len() - 2, "{width}: the line just above the footer");
        assert_eq!(lines[at].trim(), NEW, "{width}: whole, on one line");
        assert!(lines[lines.len() - 1].contains("? help"), "{width}: the footer is still there");
        assert!(lines[..at].iter().any(|l| l.contains("project-0")), "{width}: the list above it");
    }
}

#[test]
fn platoons_older_copy_is_said_as_an_update() {
    let w = world("sn-update");
    fs::create_dir_all(skill::path(&w.root).parent().unwrap()).unwrap();
    fs::write(skill::path(&w.root), skill::text("/somewhere/else/platoon")).unwrap();
    let app = looked(&w);
    assert_eq!(app.news_showing(), [UPDATE]);
    let lines = shown(&app, 80, 24);
    assert_eq!(lines[line_of(&lines, "platoon's /hand-to-squad")].trim(), UPDATE);
}

/// platoon's own unchanged copy under the old name `old` (its text matters not: its last line does).
fn old_copy(w: &World, old: &str) -> std::path::PathBuf {
    let at = skill::old_path(&w.root, old);
    fs::create_dir_all(at.parent().unwrap()).unwrap();
    fs::write(&at, skill::text("/an/older/platoon")).unwrap();
    at
}

#[test]
fn platoons_copy_under_an_old_name_is_said_as_a_rename_until_setup_renames_it() {
    for (old, other) in [("/to-tasks", "/to-tickets"), ("/to-tickets", "/to-tasks")] {
        let w = world(&format!("sn-renamed{}", old.replace('/', "-")));
        let at = old_copy(&w, old);
        let renamed = format!("{old} is now called /hand-to-squad — run `platoon setup` to rename it");
        let mut app = looked(&w);
        assert_eq!(app.news_showing(), [renamed.as_str()]);
        let lines = shown(&app, 80, 24);
        assert_eq!(lines[line_of(&lines, &format!("{old} is now"))].trim(), renamed, "whole, on one line at 80 columns");
        // Setup run: /hand-to-squad in, the old copy gone; the notice goes with its reason.
        skill::install(&w.root, &exe());
        let retired = skill::retire_old(&w.root);
        assert!(
            retired.contains(&(old, skill::Retired::Removed(at))) && retired.contains(&(other, skill::Retired::NotThere)),
            "{retired:?}"
        );
        app.refresh();
        assert_eq!(app.news_showing(), NONE);
    }
}

#[test]
fn a_user_told_of_the_rename_to_to_tasks_is_told_of_this_one_once() {
    // What platoon 1.4 remembered after saying "/to-tickets is now called /to-tasks" (or "New: /to-tasks"): the same
    // step, a reason naming its own skill's text (the cksum of 1.4's src/skills/to-tasks/SKILL.md).
    for (said_by_14, old) in
        [("renamed 1741090795", "/to-tickets"), ("missing 1741090795", "/to-tasks"), ("renamed 1741090795", "/to-tasks")]
    {
        let w = world("sn-told-by-14");
        fs::write(w.cache.join("setup-notice.json"), format!(r#"{{"said": {{"to-tickets": "{said_by_14}"}}}}"#)).unwrap();
        old_copy(&w, old);
        let renamed = format!("{old} is now called /hand-to-squad — run `platoon setup` to rename it");
        let mut app = looked(&w);
        assert_eq!(app.news_showing(), [renamed.as_str()], "{said_by_14} {old}");
        key(&mut app, 'j');
        assert_eq!(looked(&w).news_showing(), NONE, "once");
    }
}

#[test]
fn shown_once_put_away_by_a_key_or_a_click_and_never_again_for_the_same_reason() {
    let w = world("sn-once");
    let state = w.cache.join("setup-notice.json");
    let mut app = w.app();
    app.notice = None;
    // Looked for while the Tasks tab shows: nothing drawn, nothing remembered yet.
    app.top = TOP_TASKS;
    app.look_for_setup_news();
    app.refresh();
    assert_eq!(app.news_showing(), NONE);
    assert!(!words(&shown(&app, 110, 24)).contains("New: /hand-to-squad"), "the notice; the empty Tasks tab names /hand-to-squad itself");
    assert!(!state.exists(), "not said before it has shown");
    // On a list: it shows, and is remembered as said.
    app.top = TOP_SESSIONS;
    app.refresh();
    assert_eq!(app.news_showing(), [NEW]);
    assert!(state.exists());
    // Not over the help.
    app.help_open = true;
    assert_eq!(app.news_showing(), NONE);
    app.help_open = false;
    // The next key puts it away, and still does what it does.
    key(&mut app, '2');
    assert_eq!(app.news_showing(), NONE);
    assert_eq!(app.top, 1, "the key went to the Projects tab as well");
    key(&mut app, '1');
    app.refresh();
    assert_eq!(app.news_showing(), NONE, "gone for good");
    // platoon started again, the skill still missing: not said again.
    assert_eq!(looked(&w).news_showing(), NONE);

    // A click puts it away too.
    let w = world("sn-click");
    let mut app = looked(&w);
    assert_eq!(app.news_showing(), [NEW]);
    app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 50, row: 10, modifiers: KeyModifiers::NONE });
    assert_eq!(app.news_showing(), NONE);
    assert_eq!(looked(&w).news_showing(), NONE);
}

#[test]
fn a_new_reason_is_said_once_again() {
    let w = world("sn-reason");
    let mut app = looked(&w);
    assert_eq!(app.news_showing(), [NEW]);
    key(&mut app, 'j');
    assert_eq!(looked(&w).news_showing(), NONE);
    // Later platoon's copy is there but out of date (another platoon wrote it): a new reason, said once.
    fs::create_dir_all(skill::path(&w.root).parent().unwrap()).unwrap();
    fs::write(skill::path(&w.root), skill::text("/an/older/platoon")).unwrap();
    let mut app = looked(&w);
    assert_eq!(app.news_showing(), [UPDATE]);
    key(&mut app, 'j');
    assert_eq!(looked(&w).news_showing(), NONE);
}

#[test]
fn gone_by_itself_once_the_skill_is_in() {
    let w = world("sn-installed");
    let mut app = w.app();
    app.notice = None;
    app.top = TOP_TASKS;
    app.look_for_setup_news();
    app.refresh();
    // `platoon setup` run in another terminal while the notice waits: it never shows, and isn't remembered.
    skill::install(&w.root, &exe());
    app.top = TOP_SESSIONS;
    app.refresh();
    assert_eq!(app.news_showing(), NONE);
    assert!(!w.cache.join("setup-notice.json").exists());
    // Run while it shows: gone at the next refresh.
    let w = world("sn-installed-showing");
    let mut app = looked(&w);
    assert_eq!(app.news_showing(), [NEW]);
    skill::install(&w.root, &exe());
    app.refresh();
    assert_eq!(app.news_showing(), NONE);
    // And none at the next start.
    assert_eq!(looked(&w).news_showing(), NONE);
}

#[test]
fn nothing_for_a_skill_of_the_users() {
    let w = world("sn-theirs");
    fs::create_dir_all(skill::path(&w.root).parent().unwrap()).unwrap();
    fs::write(skill::path(&w.root), "---\nname: hand-to-squad\n---\nMy own.\n").unwrap();
    assert_eq!(looked(&w).news_showing(), NONE);
    // platoon's copy, changed by the user: theirs too.
    let edited = skill::text(&exe()).replace("Fewer is better", "Fewer is nicer");
    fs::write(skill::path(&w.root), edited).unwrap();
    assert_eq!(looked(&w).news_showing(), NONE);
    assert!(!w.cache.join("setup-notice.json").exists());
}

#[test]
fn nothing_without_claude_code_or_with_it_switched_off() {
    let codex = |w: &World| Some(CodexAt { program: "/nonexistent/platoon-unit-test/codex".into(), root: w.codex() });
    // Codex alone on this computer.
    let w = world("sn-no-claude");
    let mut app = w.app();
    app.notice = None;
    app.set_providers_for_test(Providers { claude: None, codex: codex(&w), off: Vec::new() });
    app.look_for_setup_news();
    app.refresh();
    assert_eq!(app.news_showing(), NONE);

    // Both installed, Claude Code switched off in platoon's settings.
    let w = world("sn-claude-off");
    let mut app = w.app();
    app.notice = None;
    app.set_providers_for_test(Providers { claude: Some("claude".into()), codex: codex(&w), off: Vec::new() });
    let mut s = settings::load(&w.config).0;
    s.use_claude_code = false;
    settings::save(&w.config, &s).unwrap();
    app.refresh_for_test();
    app.refresh_for_test();
    assert_eq!(app.providers().off, [Agent::Claude]);
    app.look_for_setup_news();
    app.refresh();
    assert_eq!(app.news_showing(), NONE);
    assert!(!w.cache.join("setup-notice.json").exists());

    // Switched off while the notice shows: gone at the next refresh.
    let w = world("sn-off-later");
    let mut app = w.app();
    app.notice = None;
    app.set_providers_for_test(Providers { claude: Some("claude".into()), codex: codex(&w), off: Vec::new() });
    app.look_for_setup_news();
    app.refresh();
    assert_eq!(app.news_showing(), [NEW]);
    let mut s = settings::load(&w.config).0;
    s.use_claude_code = false;
    settings::save(&w.config, &s).unwrap();
    app.refresh_for_test();
    app.refresh();
    assert_eq!(app.news_showing(), NONE);
}

#[test]
fn with_a_version_notice_both_show_each_on_its_own_lines_and_one_key_puts_both_away() {
    let w = world("sn-both");
    let mut app = w.app();
    app.notice = None;
    app.look_for_setup_news();
    app.set_news_for_test(VERSION);
    app.refresh();
    assert_eq!(app.news_showing(), [NEW, VERSION]);
    for (width, height) in [(110u16, 24u16), (80, 24)] {
        let lines = shown(&app, width, height);
        println!("{width}x{height}:\n{}\n", lines.join("\n"));
        let at = line_of(&lines, "New: /hand-to-squad");
        assert_eq!(lines[at].trim(), NEW, "{width}");
        assert!(lines[at + 1].trim_start().starts_with("Claude Code 2.2.0 is newer"), "{width}: the version notice starts the next line");
        assert_eq!(words(&lines[at + 1..lines.len() - 1]), VERSION, "{width}: whole, just above the footer");
        assert!(lines[lines.len() - 1].contains("? help"));
    }
    // A short terminal: one line each, the longer one cut.
    let lines = shown(&app, 80, 14);
    println!("80x14:\n{}\n", lines.join("\n"));
    let at = line_of(&lines, "New: /hand-to-squad");
    assert_eq!(at, lines.len() - 3);
    assert!(lines[at + 1].trim_end().ends_with('…'), "{:?}", lines[at + 1]);
    key(&mut app, 'j');
    assert_eq!(app.news_showing(), NONE);
    assert_eq!(looked(&w).news_showing(), NONE);
}
