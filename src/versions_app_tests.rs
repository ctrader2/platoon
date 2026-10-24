//! The version notice in the app (`versions.rs`, `App::news_showing`): its own lines above the footer of the Sessions and
//! Projects lists, drawn the way a terminal shows them (TestBackend) at 110 and 80 columns; only there, remembered as
//! announced only once it has shown, and put away by the next key or click. Every directory is a temp dir (`World`);
//! nothing here starts a process or reaches the network.

use crate::app::{App, TOP_SESSIONS, TOP_TASKS, TOPS};

/// The Projects tab, as `app.top` numbers it (`TOPS`).
const TOP_PROJECTS: usize = 1;

#[test]
fn the_tab_numbers_here_are_the_apps() {
    assert_eq!((TOPS[TOP_TASKS], TOPS[TOP_PROJECTS], TOPS.len()), ("Tasks", "Projects", 4));
}
use crate::testkit::World;
use crate::ui;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
};

const CLAUDE: &str = "Claude Code 2.2.0 is newer than this platoon was checked with (2.1.288). If something looks wrong, update platoon.";
const BOTH: &str =
    "Claude Code 2.2.0 is newer than this platoon was checked with (2.1.288). platoon 1.1.0 is out. To update: brew upgrade platoon";

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

fn with_news(w: &World, text: &str) -> App {
    let mut app = w.app();
    app.notice = None;
    app.set_news_for_test(text);
    app.refresh();
    app
}

/// The rows of the notice: the lines between the list and the footer that hold its words.
fn notice_rows(lines: &[String], first_words: &str) -> std::ops::Range<usize> {
    let start = lines.iter().position(|l| l.trim_start().starts_with(first_words)).unwrap_or_else(|| {
        panic!("{first_words:?} does not start a line:\n{}", lines.join("\n"));
    });
    let footer = lines.len() - 1;
    start..footer
}

#[test]
fn the_notice_has_its_own_lines_above_the_footer_whole_at_110_and_80_columns() {
    let w = world("vn-render");
    for text in [CLAUDE, BOTH] {
        let app = with_news(&w, text);
        for width in [110u16, 80] {
            let lines = shown(&app, width, 24);
            println!("{width}x24:\n{}\n", lines.join("\n"));
            let rows = notice_rows(&lines, "Claude Code 2.2.0 is newer");
            // Every word of it, in order, on the lines just above the footer; nothing cut.
            assert_eq!(words(&lines[rows.clone()]), text, "{width} columns:\n{}", lines.join("\n"));
            assert!(!lines[rows.clone()].iter().any(|l| l.contains('…')), "{width}: not cut");
            assert!(rows.len() <= 2, "{width}: two lines at most for these: {}", rows.len());
            // The footer is still there, with the help.
            assert!(lines[lines.len() - 1].contains("? help"), "{width}: {:?}", lines[lines.len() - 1]);
            // And the list above it.
            assert!(lines[..rows.start].iter().any(|l| l.contains("project-0")), "{width}:\n{}", lines.join("\n"));
        }
    }
    // The Claude Code notice is 112 characters: two lines at 110 columns, one where it fits.
    let app = with_news(&w, CLAUDE);
    let lines = shown(&app, 120, 24);
    assert_eq!(notice_rows(&lines, "Claude Code").len(), 1, "one line where it fits:\n{}", lines.join("\n"));
}

#[test]
fn a_long_notice_on_a_short_narrow_terminal_is_cut_at_two_lines() {
    let w = world("vn-small");
    let long = format!(
        "platoon 1.1.0 is out. To update, run the install script again: curl -LsSf {} | sh {CLAUDE}",
        crate::versions::INSTALLER_URL
    );
    let app = with_news(&w, &long);
    let lines = shown(&app, 60, 14);
    println!("{}", lines.join("\n"));
    let rows = notice_rows(&lines, "platoon 1.1.0 is out.");
    assert_eq!(rows.len(), 2);
    assert!(lines[rows.end - 1].trim_end().ends_with('…'), "{:?}", lines[rows.end - 1]);
    let app = with_news(&w, &long);
    let lines = shown(&app, 80, 30);
    assert!(notice_rows(&lines, "platoon 1.1.0 is out.").len() <= 4);
}

#[test]
fn it_shows_on_the_two_lists_only_and_is_remembered_once_it_has_shown() {
    let w = world("vn-where");
    let state = w.cache.join("version-check.json");
    let mut app = w.app();
    app.notice = None;
    // The checks answer while the Tasks tab shows (its details have the room beside the list): nothing is drawn,
    // nothing is remembered yet.
    app.top = TOP_TASKS;
    app.set_news_for_test(CLAUDE);
    app.refresh();
    assert_eq!(app.news_showing(), None);
    assert!(!words(&shown(&app, 110, 24)).contains("Claude Code 2.2.0"));
    assert!(!state.exists(), "not announced before it has shown");
    // Back on a list: it shows, and its versions are remembered as announced.
    app.top = TOP_PROJECTS;
    app.refresh();
    assert_eq!(app.news_showing(), Some(CLAUDE));
    assert!(words(&shown(&app, 110, 24)).contains(CLAUDE));
    assert!(state.exists());
    app.top = TOP_SESSIONS;
    assert_eq!(app.news_showing(), Some(CLAUDE));
    // Not over the help.
    app.help_open = true;
    assert_eq!(app.news_showing(), None);
    app.help_open = false;
    assert_eq!(app.news_showing(), Some(CLAUDE));
}

#[test]
fn the_next_key_or_click_puts_it_away_and_still_does_what_it_does() {
    let w = world("vn-key");
    let mut app = with_news(&w, CLAUDE);
    assert_eq!(app.news_showing(), Some(CLAUDE));
    app.on_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
    assert_eq!(app.news_showing(), None);
    assert_eq!(app.top, TOP_PROJECTS, "the key went to the Projects tab as well");
    app.on_key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE));
    app.refresh();
    assert_eq!(app.news_showing(), None, "gone for good");
    let lines = shown(&app, 110, 24);
    assert!(!words(&lines).contains("Claude Code 2.2.0"));

    let mut app = with_news(&w, CLAUDE);
    app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 50, row: 10, modifiers: KeyModifiers::NONE });
    assert_eq!(app.news_showing(), None);
}

#[test]
fn a_test_app_starts_no_version_checks() {
    // `App::with_dirs` (every unit test's app) never starts them; the real `App::new` does, unless PLATOON_NO_UPDATE_CHECK.
    let w = world("vn-none");
    let mut app = w.app();
    app.refresh();
    assert_eq!(app.news_showing(), None);
    assert!(!w.cache.join("version-check.json").exists());
}

#[test]
fn a_newer_codex_has_its_own_notice_whole_at_110_and_80_columns_and_joins_the_others_in_one() {
    const CODEX: &str = "Codex 0.164.0 is newer than this platoon was checked with (0.163.0). If something looks wrong, update platoon.";
    const ALL: &str = "Claude Code 2.2.0 and Codex 0.164.0 are newer than this platoon was checked with (2.1.288 and 0.163.0). platoon \
                       1.1.0 is out. To update: brew upgrade platoon";
    let w = world("vn-codex");
    for (text, first) in [(CODEX, "Codex 0.164.0 is newer"), (ALL, "Claude Code 2.2.0 and Codex")] {
        let app = with_news(&w, text);
        for width in [110u16, 80] {
            let lines = shown(&app, width, 24);
            let rows = notice_rows(&lines, first);
            assert_eq!(words(&lines[rows.clone()]), text, "{width} columns:\n{}", lines.join("\n"));
            assert!(!lines[rows.clone()].iter().any(|l| l.contains('…')), "{width}: not cut");
            // The Codex notice fits two lines; all three parts take a third at 80 columns.
            let most = if text == CODEX || width == 110 { 2 } else { 3 };
            assert!(rows.len() <= most, "{width}: {most} lines at most: {}", rows.len());
            assert!(lines[lines.len() - 1].contains("? help"), "{width}: {:?}", lines[lines.len() - 1]);
        }
    }
}
