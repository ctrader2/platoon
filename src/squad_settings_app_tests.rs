//! A squad's settings changed on the Squads tab without an editor: with a squad open (`enter`), `↑` `↓` pick its Model,
//! Verdict model or Plans first line (lit, "enter changes" at its end), `enter` writes the next choice to that one line
//! of its file (a built-in copied to the user's folder first, gone again when it is the built-in's text once more),
//! past the settings the arrows scroll, a click picks and changes, and an editor holding the file holds the change off.
//! Every directory is a temp dir (`World`); the only editor is a stand-in script.

use crate::app::{App, TOP_SQUADS};
use crate::squads::{self, Origin};
use crate::testkit::World;
use crate::ui;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
    style::Color,
};
use std::fs;

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn draw(term: &mut Terminal<TestBackend>, app: &App) -> String {
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Where `needle` is on the screen: (column, row).
fn at(shown: &str, needle: &str) -> (u16, u16) {
    for (y, line) in shown.lines().enumerate() {
        if let Some(i) = line.find(needle) {
            return (line[..i].chars().count() as u16, y as u16);
        }
    }
    panic!("{needle:?} isn't on the screen:\n{shown}")
}

/// The foreground and background of the cell at the start of `needle`.
fn colors(term: &Terminal<TestBackend>, shown: &str, needle: &str) -> (Color, Color) {
    let (x, y) = at(shown, needle);
    let c = &term.backend().buffer()[(x, y)];
    (c.fg, c.bg)
}

/// The Squads tab with squad `id` open full width.
fn open(name: &str, id: &str) -> (World, App) {
    let w = World::new(name);
    let mut app = w.app();
    key(&mut app, KeyCode::Char('4'));
    assert_eq!(app.top, TOP_SQUADS);
    app.sqt.selected = app.sqt.list.iter().position(|s| s.id == id).unwrap();
    key(&mut app, KeyCode::Enter);
    assert!(app.sqt.open);
    (w, app)
}

#[test]
fn the_arrows_pick_a_setting_lit_and_enter_writes_the_next_choice_to_that_line() {
    let (w, mut app) = open("squad-settings", "docs");
    let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
    let t = app.effective_theme().clone();
    // The Model line is picked as the squad opens: lit, with what enter does at its end.
    let shown = draw(&mut term, &app);
    assert!(shown.contains("Model          sonnet") && shown.contains("enter changes"), "{shown}");
    assert_eq!(at(&shown, "enter changes").1, at(&shown, "Model          sonnet").1, "on the Model line: {shown}");
    assert_eq!(colors(&term, &shown, "Model          sonnet").1, t.surface, "the picked line is lit");
    assert_eq!(colors(&term, &shown, "enter changes").0, ui::hint_fg(&t));
    assert_ne!(colors(&term, &shown, "Verdict model").1, t.surface, "the others are not");
    assert!(shown.lines().last().unwrap().contains("enter change"), "the footer says so: {shown}");

    // enter: the next model. The built-in's text goes to the user's folder with that one line changed.
    key(&mut app, KeyCode::Enter);
    let file = squads::path(&w.config, "docs");
    let built_in = squads::built_in_text("docs").unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), built_in.replace("\nmodel: sonnet\n", "\nmodel: haiku\n"));
    assert_eq!(app.active_notice(), Some("Docs: Model haiku. Yours now, in place of the built-in."));
    let shown = draw(&mut term, &app);
    assert!(shown.contains("Model          haiku") && !shown.contains("File "), "{shown}");

    // ↓ the verdict model, ↓ plans first: each its own line, the rest of the file as it was.
    key(&mut app, KeyCode::Down);
    let shown = draw(&mut term, &app);
    assert_eq!(colors(&term, &shown, "Verdict model").1, t.surface);
    assert_eq!(at(&shown, "enter changes").1, at(&shown, "Verdict model").1);
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.active_notice(), Some("Docs: Plans first yes."));
    let want = built_in
        .replace("\nmodel: sonnet\n", "\nmodel: haiku\n")
        .replace("\nverdict_model: opus\n", "\nverdict_model: sonnet\n")
        .replace("\nplan_first: false\n", "\nplan_first: true\n");
    assert_eq!(fs::read_to_string(&file).unwrap(), want, "only those three lines changed");
    let docs = app.sqt.picked().unwrap();
    assert_eq!(
        (docs.model.as_str(), docs.verdict_model.as_str(), docs.plan_first, docs.origin),
        ("haiku", "sonnet", true, Origin::Replaces)
    );
    let shown = draw(&mut term, &app);
    assert!(shown.contains("Plans first    yes, then waits for your yes"), "{shown}");
}

#[test]
fn a_built_in_changed_back_is_built_in_again_and_the_same_as_the_model_is_a_choice() {
    let (w, mut app) = open("squad-settings-back", "feature");
    let file = squads::path(&w.config, "feature");
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert!(file.exists() && !app.sqt.picked().unwrap().plan_first);
    key(&mut app, KeyCode::Enter);
    assert!(!file.exists(), "the built-in's text again: no copy");
    assert_eq!(app.sqt.picked().unwrap().origin, Origin::BuiltIn);
    assert_eq!(app.active_notice(), Some("Feature: Plans first yes. That is the built-in again, so it is built in."));
    // Feature's verdict model is fable, the last model: next is "the same as the model", no line in the file.
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Enter);
    assert!(!fs::read_to_string(&file).unwrap().contains("verdict_model"));
    assert_eq!(app.active_notice(), Some("Feature: Verdict model the same as the model. Yours now, in place of the built-in."));
    let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
    assert!(draw(&mut term, &app).contains("Verdict model  the same as the model"));
}

#[test]
fn past_the_settings_the_arrows_scroll_and_up_at_the_top_picks_them_again() {
    let (_w, mut app) = open("squad-settings-scroll", "review");
    let mut term = Terminal::new(TestBackend::new(110, 20)).unwrap();
    draw(&mut term, &app);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    assert_eq!(app.sqt.field, Some(2));
    key(&mut app, KeyCode::Down);
    let shown = draw(&mut term, &app);
    assert_eq!((app.sqt.field, app.sqt.top.get()), (None, 1), "scrolled one line");
    assert!(!shown.contains("enter changes") && !shown.lines().last().unwrap().contains("enter change"), "{shown}");
    // enter does nothing with no setting picked.
    key(&mut app, KeyCode::Enter);
    assert!(!squads::path(&_w.config, "review").exists());
    key(&mut app, KeyCode::Up);
    draw(&mut term, &app);
    assert_eq!((app.sqt.field, app.sqt.top.get()), (None, 0));
    key(&mut app, KeyCode::Up);
    assert_eq!(app.sqt.field, Some(2), "back on the last setting");
    // Home goes to the top and the first setting.
    key(&mut app, KeyCode::PageDown);
    key(&mut app, KeyCode::Home);
    assert_eq!((app.sqt.field, app.sqt.top.get()), (Some(0), 0));
}

#[test]
fn a_click_picks_a_setting_and_a_click_on_the_picked_one_changes_it() {
    let (w, mut app) = open("squad-settings-click", "explain");
    let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
    let shown = draw(&mut term, &app);
    let (x, y) = at(&shown, "Plans first");
    let click = |app: &mut App| {
        app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: x + 2, row: y, modifiers: KeyModifiers::NONE })
    };
    click(&mut app);
    assert_eq!(app.sqt.field, Some(2));
    assert!(!squads::path(&w.config, "explain").exists(), "picked, not changed");
    draw(&mut term, &app);
    click(&mut app);
    assert!(app.sqt.picked().unwrap().plan_first);
    assert!(fs::read_to_string(squads::path(&w.config, "explain")).unwrap().contains("\nplan_first: true\n"));
}

#[test]
fn an_editor_on_the_squads_file_holds_the_change_off() {
    let (w, mut app) = open("squad-settings-held", "docs");
    // A stand-in editor with a window of its own that stays until the test lets it go (20 seconds at most).
    let go = w.base.join("go");
    let ed = w.base.join("stand-in-editor");
    let script = format!("#!/bin/sh\nn=0\nwhile [ ! -e '{}' ] && [ $n -lt 400 ]; do sleep 0.05; n=$((n+1)); done\n", go.display());
    fs::write(&ed, script).unwrap();
    fs::set_permissions(&ed, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    app.set_editors_for_test(vec![crate::editors::Editor {
        id: "stand-in".into(),
        name: "Stand-in".into(),
        program: ed.to_string_lossy().into_owned(),
        args: Vec::new(),
        terminal: false,
    }]);
    key(&mut app, KeyCode::Char('e'));
    key(&mut app, KeyCode::Enter);
    assert!(app.sqt.open && app.sqt.field == Some(0));
    let file = squads::path(&w.config, "docs");
    let before = fs::read_to_string(&file).unwrap();
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.active_notice(), Some("Docs is open in Stand-in. Close it first."));
    assert_eq!(fs::read_to_string(&file).unwrap(), before, "the file under the editor is left alone");
    // The editor goes: the copy left unchanged goes with it, and enter changes the setting again.
    fs::write(&go, "").unwrap();
    let start = std::time::Instant::now();
    while !app.active_notice().is_some_and(|n| n.contains("unchanged")) {
        assert!(start.elapsed() < std::time::Duration::from_secs(20), "the editor never finished");
        std::thread::sleep(std::time::Duration::from_millis(20));
        app.poll_editors();
    }
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.sqt.picked().unwrap().model, "haiku");
}

#[test]
fn a_file_without_settings_at_its_top_is_left_alone_and_e_is_the_way() {
    let w = World::new("squad-settings-loose");
    let mine = squads::path(&w.config, "loose");
    fs::create_dir_all(mine.parent().unwrap()).unwrap();
    fs::write(&mine, "just steps\n").unwrap();
    let mut app = w.app();
    key(&mut app, KeyCode::Char('4'));
    app.sqt.selected = app.sqt.list.iter().position(|s| s.id == "loose").unwrap();
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.active_notice(), Some("loose's file has no settings at its top to change: e opens it to fix that."));
    assert_eq!(fs::read_to_string(&mine).unwrap(), "just steps\n");
}
