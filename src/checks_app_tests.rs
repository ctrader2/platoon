//! The test check at the level of the app: the project's Tests line (with and without what its `CLAUDE.md` suggests)
//! and `T`'s box. Every directory is a temp dir (`World`).

use crate::app::{App, Mode, NamePurpose};
use crate::testkit::World;
use crate::ui;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    style::Color,
};
use serde_json::Value;
use std::{fs, path::PathBuf};

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn ctrl(app: &mut App, c: char) {
    app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
}

fn typed(app: &mut App, text: &str) {
    for c in text.chars() {
        key(app, KeyCode::Char(c));
    }
}

fn draw(term: &mut Terminal<TestBackend>, app: &App) -> String {
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn flat(text: &str) -> String {
    text.replace(['│', '╭', '╮', '╰', '╯', '─'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The cell where `needle` first starts on the line holding `row`.
fn fg_on(term: &Terminal<TestBackend>, row: &str, needle: &str) -> Color {
    let buf = term.backend().buffer();
    for y in 0..buf.area.height {
        let line: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
        if line.contains(row)
            && let Some(at) = line.find(needle)
        {
            return buf[(line[..at].chars().count() as u16, y)].fg;
        }
    }
    panic!("{needle:?} isn't on the line with {row:?}")
}

/// The line holding `text`, read flat.
fn line_with(shown: &str, text: &str) -> String {
    flat(shown.lines().find(|l| l.contains(text)).unwrap_or_else(|| panic!("no {text:?} in {shown}")))
}

/// "Web shop", whose one session works in `shop` (a folder in git, as far as a `.git` in it says); the project's
/// screen open.
fn shop_world(name: &str, claude_md: Option<&str>) -> (World, App, String, PathBuf) {
    let w = World::new(name);
    let shop = w.base.join("shop");
    fs::create_dir_all(shop.join(".git")).unwrap();
    if let Some(md) = claude_md {
        fs::write(shop.join("CLAUDE.md"), md).unwrap();
    }
    w.transcript("user-1", &shop.to_string_lossy(), "make a shop", 10);
    let mut app = w.app();
    let web = app.projects.create("Web shop").unwrap();
    app.projects.assign("user-1", Some(&web)).unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Char('3'));
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.open_project.as_deref(), Some(web.as_str()));
    app.notice = None;
    (w, app, web, shop)
}

const MD: &str = "# Shop\n\n## Build and test\n\n```\nexport PATH=\"$HOME/bin:$PATH\"   # each shell starts fresh\ncargo test                       # about a minute\n```\n";

#[test]
fn the_projects_tests_line_suggests_what_its_claude_md_says_and_t_sets_the_command() {
    let (w, mut app, web, _shop) = shop_world("ck-line", Some(MD));
    let t = app.effective_theme().clone();
    for (wide, high) in [(120, 40), (80, 24)] {
        let mut term = Terminal::new(TestBackend::new(wide, high)).unwrap();
        let shown = draw(&mut term, &app);
        assert_eq!(line_with(&shown, "Tests"), "Tests not set · T sets it · CLAUDE.md says: cargo test", "{wide}: {shown}");
        // Under the line saying how squads start, its label in the same quiet color, the key in the accent.
        let rows: Vec<&str> = shown.lines().collect();
        let at = rows.iter().position(|l| l.contains("Squads start")).unwrap();
        assert!(rows[at + 1].trim_start().starts_with("Tests"), "{wide}: {shown}");
        assert_eq!(rows[at + 1].find("not set"), rows[at].find("asks before"), "the values line up: {shown}");
        assert_eq!(fg_on(&term, "Tests", "Tests"), t.dim, "{wide}");
        assert_eq!(fg_on(&term, "Tests", "not set"), t.subtext, "{wide}");
        assert_eq!(fg_on(&term, "Tests", "T sets"), t.accent, "{wide}");
        assert_eq!(fg_on(&term, "Tests", "cargo test"), t.subtext, "{wide}");
    }
    // The help lists the key (the line itself says it, so the footer lets it go early).
    key(&mut app, KeyCode::Char('?'));
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    assert!(flat(&draw(&mut term, &app)).contains("T test command"));
    key(&mut app, KeyCode::Esc);
    // Narrow, the suggestion goes before the key does.
    let mut term = Terminal::new(TestBackend::new(60, 24)).unwrap();
    assert_eq!(line_with(&draw(&mut term, &app), "Tests"), "Tests not set · T sets it");
    // Nothing was run or written from the suggestion.
    let file: Value = serde_json::from_str(&fs::read_to_string(w.config.join("projects.json")).unwrap()).unwrap();
    assert!(file["projects"][&web].get("test_command").is_none(), "{file}");

    // T: the box, holding the suggestion for the user to check; enter saves it.
    key(&mut app, KeyCode::Char('T'));
    assert!(matches!(app.mode, Mode::ProjectName) && app.name_box.purpose == NamePurpose::TestCommand(web.clone()));
    assert_eq!(app.name_box.input, "cargo test");
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("Test command") && shown.contains("❯ cargo test"), "{shown}");
    assert!(shown.contains("Run with sh where the squad works, when it finishes."), "{shown}");
    typed(&mut app, " --quiet");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.active_notice(), Some("Tests for Web shop: platoon runs `cargo test --quiet` when a squad finishes."));
    let file: Value = serde_json::from_str(&fs::read_to_string(w.config.join("projects.json")).unwrap()).unwrap();
    assert_eq!(file["projects"][&web]["test_command"], "cargo test --quiet");
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let shown = draw(&mut term, &app);
    assert_eq!(line_with(&shown, "Tests"), "Tests cargo test --quiet T changes", "no suggestion once it is set: {shown}");
    assert_eq!(fg_on(&term, "Tests", "cargo test --quiet"), t.text);
    assert_eq!(fg_on(&term, "Tests", "T changes"), t.accent);

    // From the notes pane too; an empty box takes the command away.
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Char('T'));
    assert_eq!(app.name_box.input, "cargo test --quiet", "it holds the command it has");
    ctrl(&mut app, 'u');
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.active_notice(), Some("Web shop has no test command now: platoon runs no tests for its tasks."));
    let file: Value = serde_json::from_str(&fs::read_to_string(w.config.join("projects.json")).unwrap()).unwrap();
    assert!(file["projects"][&web].get("test_command").is_none(), "{file}");
    // Esc in the box changes nothing.
    key(&mut app, KeyCode::Char('T'));
    typed(&mut app, "x");
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List));
    assert!(app.projects.get(&web).unwrap().test_command.is_empty());
}

#[test]
fn without_a_claude_md_block_the_tests_line_suggests_nothing_and_the_box_starts_empty() {
    let (_w, mut app, _web, shop) = shop_world("ck-nomd", Some("# Shop\n\nRun the tests with cargo.\n"));
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let shown = draw(&mut term, &app);
    assert_eq!(line_with(&shown, "Tests"), "Tests not set · T sets it", "{shown}");
    assert!(!shown.contains("CLAUDE.md"), "{shown}");
    // A CLAUDE.md written now is read at the next refresh.
    fs::write(shop.join("CLAUDE.md"), MD).unwrap();
    app.reload_for_test();
    assert!(flat(&draw(&mut term, &app)).contains("CLAUDE.md says: cargo test"));
    fs::remove_file(shop.join("CLAUDE.md")).unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Char('T'));
    assert_eq!(app.name_box.input, "", "nothing to suggest");
}
