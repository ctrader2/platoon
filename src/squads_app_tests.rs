//! The Squads tab (`4`) at the level of the app: the list with where each squad comes from and "needs fixing" in the
//! warning colour, the pane beside it from `TWO_PANES_FROM` (one pane below it, enter opening the squad), `e` on a
//! built-in (the copy written, kept when changed, taken away when not), `n` (the name box, the template, the editor),
//! `d` (asked first, a `.bak` kept, a replaced built-in back), and the help. Every directory is a temp dir (`World`);
//! the only editor is a stand-in script with a window of its own.

use crate::app::{App, Confirm, Mode, NamePurpose, TOP_SQUADS};
use crate::squads::{self, Origin};
use crate::testkit::World;
use crate::ui;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    style::Color,
};
use std::{fs, path::Path};

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn ch(app: &mut App, c: char) {
    key(app, KeyCode::Char(c));
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
    text.replace(['│', '╭', '╮', '╰', '╯', '─', '▎'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn fg_of(term: &Terminal<TestBackend>, needle: &str) -> Color {
    let buf = term.backend().buffer();
    for y in 0..buf.area.height {
        let line: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
        if let Some(at) = line.find(needle) {
            return buf[(line[..at].chars().count() as u16, y)].fg;
        }
    }
    panic!("{needle:?} isn't on the screen")
}

/// A stand-in editor with a window of its own, running `script` on the file it is given (`$1`).
fn editor(app: &mut App, w: &World, script: &str) {
    let ed = w.base.join("stand-in-editor");
    fs::write(&ed, format!("#!/bin/sh\n{script}\n")).unwrap();
    fs::set_permissions(&ed, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    app.set_editors_for_test(vec![crate::editors::Editor {
        id: "stand-in".into(),
        name: "Stand-in".into(),
        program: ed.to_string_lossy().into_owned(),
        args: Vec::new(),
        terminal: false,
    }]);
}

/// Poll the editors until `cond`.
fn until(app: &mut App, what: &str, cond: impl Fn(&App) -> bool) {
    let start = std::time::Instant::now();
    while !cond(app) {
        assert!(start.elapsed() < std::time::Duration::from_secs(20), "timed out waiting for: {what}");
        std::thread::sleep(std::time::Duration::from_millis(20));
        app.poll_editors();
    }
}

/// A world with one squad replacing Review and one of the user's own with a broken file, on the Squads tab.
fn squads_world(name: &str) -> (World, App) {
    let w = World::new(name);
    let dir = squads::dir(&w.config);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("review.md"), "---\nname: Our review\ndescription: Reviews the way we do them\nmodel: sonnet\n---\n1. Read it.\n")
        .unwrap();
    fs::write(dir.join("sloppy.md"), "---\nname: Sloppy\ndescription: Half written\nmodle: opus\n---\n1. Go.\n").unwrap();
    let mut app = w.app();
    ch(&mut app, '4');
    assert_eq!(app.top, TOP_SQUADS);
    (w, app)
}

fn pick(app: &mut App, id: &str) {
    let i = app.sqt.list.iter().position(|s| s.id == id).unwrap();
    app.sqt.selected = i;
}

fn exists(p: &Path) -> bool {
    p.exists()
}

#[test]
fn the_tab_lists_every_squad_with_where_it_comes_from_and_the_picked_one_beside_it() {
    let (_w, mut app) = squads_world("squads-list");
    let ids: Vec<&str> = app.sqt.list.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["feature", "bug-fix", "incident", "refactor", "explain", "review", "docs", "sloppy"]);
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let shown = draw(&mut term, &app);
    let text = flat(&shown);
    assert!(shown.lines().next().unwrap().contains("4 Squads"), "{shown}");
    for want in ["Feature built in", "Our review yours, replaces the built-in", "Sloppy yours · needs fixing", "Reviews the way we do them"]
    {
        assert!(text.contains(want), "{want}\n{shown}");
    }
    let t = app.effective_theme().clone();
    assert_eq!(fg_of(&term, "needs fixing"), t.warn, "needs fixing is in the warning colour");
    assert_ne!(fg_of(&term, "built in"), t.warn);
    // Beside the list: Feature, picked first.
    for want in ["Model opus", "Verdict model fable", "Plans first yes, then waits for your yes", "File built in", "Steps", "1. Learn how"]
    {
        assert!(text.contains(want), "{want}\n{shown}");
    }
    assert!(!text.contains("Needs fixing"), "nothing wrong with Feature");
    // The broken one says what is wrong, one line each, in the warning colour.
    pick(&mut app, "sloppy");
    let shown = draw(&mut term, &app);
    let text = flat(&shown);
    let at = |needle: &str| shown.lines().position(|l| l.contains(needle)).unwrap_or_else(|| panic!("{needle}\n{shown}"));
    assert_eq!(at("│ Needs fixing") + 1, at("│ · line 4: unknown setting \"modle\""), "{shown}");
    assert!(text.contains("· front matter: no model"), "{shown}");
    assert_eq!(fg_of(&term, "line 4: unknown setting"), t.warn);
    // The path wraps wherever the temp folder's length puts it: read the pane's lines joined up.
    let pane: String = shown.lines().filter_map(|l| l.split_once('│')).map(|(_, r)| r.trim()).collect();
    assert!(text.contains("File ") && pane.contains("config/squads/sloppy.md"), "its file: {shown}");
    // The one replacing a built-in: no verdict model of its own, plans first not.
    pick(&mut app, "review");
    let text = flat(&draw(&mut term, &app));
    assert!(
        text.contains("Verdict model the same as the model")
            && text.contains("Plans first no")
            && text.contains("in place of the built-in"),
        "{text}"
    );
    // The footer.
    let shown = draw(&mut term, &app);
    let footer = shown.lines().last().unwrap();
    for want in ["enter open", "e edit", "n new squad", "d delete"] {
        assert!(footer.contains(want), "{want}: {footer}");
    }
    pick(&mut app, "feature");
    let shown = draw(&mut term, &app);
    assert!(!shown.lines().last().unwrap().contains("d delete"), "a built-in isn't deleted");
}

#[test]
fn below_two_panes_the_list_is_alone_and_enter_opens_the_squad_full_width() {
    let (_w, mut app) = squads_world("squads-narrow");
    let mut term = Terminal::new(TestBackend::new(70, 24)).unwrap();
    let text = flat(&draw(&mut term, &app));
    assert!(text.contains("Feature") && text.contains("Bug fix") && !text.contains("Verdict model"), "{text}");
    assert!(app.at_top_level());
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert!(app.sqt.open && !app.at_top_level());
    let shown = draw(&mut term, &app);
    let text = flat(&shown);
    assert!(text.contains("Bug fix") && text.contains("Verdict model fable") && text.contains("Steps"), "{shown}");
    assert!(text.contains("more lines"), "a long squad says what is below: {shown}");
    assert!(shown.lines().last().unwrap().contains("esc back"), "{shown}");
    // The arrows go through its three settings, then scroll it; esc goes back to the list, on the same squad.
    for _ in 0..3 {
        key(&mut app, KeyCode::Down);
    }
    draw(&mut term, &app);
    assert_eq!(app.sqt.top.get(), 1);
    key(&mut app, KeyCode::Esc);
    assert!(!app.sqt.open && app.sqt.selected == 1);
    // q goes back from the open squad, as esc does, and quits from the list.
    key(&mut app, KeyCode::Enter);
    ch(&mut app, 'q');
    assert!(!app.sqt.open && !app.quit);
    // 4 again from the open squad goes back to the list too.
    key(&mut app, KeyCode::Enter);
    ch(&mut app, '4');
    assert!(!app.sqt.open);
    // `/` searches sessions elsewhere; here there are none to find.
    ch(&mut app, '/');
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn e_on_a_built_in_keeps_its_copy_only_when_it_changed() {
    let (w, mut app) = squads_world("squads-edit");
    let file = squads::path(&w.config, "docs");
    pick(&mut app, "docs");
    // Left as it was: the copy goes and Docs stays built in.
    editor(&mut app, &w, "exit 0");
    ch(&mut app, 'e');
    assert!(matches!(app.mode, Mode::PickEditor), "{:?}", app.active_notice());
    assert!(exists(&file), "the built-in's text is written for the editor");
    key(&mut app, KeyCode::Enter);
    until(&mut app, "the editor to finish", |a| a.active_notice().is_some_and(|n| n.contains("unchanged")));
    assert_eq!(app.active_notice(), Some("Docs is unchanged, so it stays built in."));
    assert!(!exists(&file));
    assert_eq!(app.sqt.picked().map(|s| (s.id.as_str(), s.origin)), Some(("docs", Origin::BuiltIn)));
    // esc on the list of editors: the copy goes too.
    ch(&mut app, 'e');
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List) && !exists(&file));
    // Changed: it is the user's, in place of the built-in.
    editor(&mut app, &w, "sed 's/^model: sonnet$/model: haiku/' \"$1\" > \"$1.new\" && mv \"$1.new\" \"$1\"");
    ch(&mut app, 'e');
    key(&mut app, KeyCode::Enter);
    until(&mut app, "the edit", |a| a.active_notice().is_some_and(|n| n.starts_with("Saved")));
    assert_eq!(app.active_notice(), Some("Saved Docs. Tasks started from now on follow it."));
    let docs = app.sqt.picked().unwrap();
    assert_eq!((docs.id.as_str(), docs.origin, docs.model.as_str()), ("docs", Origin::Replaces, "haiku"));
    // Editing the user's own file that breaks it: said, and the list shows it.
    editor(&mut app, &w, "printf -- '---\\nname: Docs\\n---\\n' > \"$1\"");
    ch(&mut app, 'e');
    key(&mut app, KeyCode::Enter);
    until(&mut app, "the edit", |a| a.active_notice().is_some_and(|n| n.contains("needs fixing")));
    assert!(exists(&file) && !app.sqt.picked().unwrap().usable());
}

#[test]
fn n_makes_a_squad_from_the_template_and_opens_it() {
    let (w, mut app) = squads_world("squads-new");
    editor(&mut app, &w, "exit 0");
    ch(&mut app, 'n');
    assert!(matches!(&app.mode, Mode::ProjectName) && app.name_box.purpose == NamePurpose::NewSquad);
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    assert!(flat(&draw(&mut term, &app)).contains("New squad"));
    for c in "Quick fix".chars() {
        ch(&mut app, c);
    }
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::PickEditor), "{:?}", app.active_notice());
    let file = squads::path(&w.config, "quick-fix");
    assert!(exists(&file));
    key(&mut app, KeyCode::Enter);
    until(&mut app, "the editor to finish", |a| a.active_notice().is_some());
    // Left as the template is, it stays: the user asked for it.
    assert!(exists(&file));
    let s = app.sqt.picked().unwrap();
    assert_eq!((s.id.as_str(), s.name.as_str(), s.origin, s.usable()), ("quick-fix", "Quick fix", Origin::Own, true));
    // The same name again: said, and the box stays for another.
    ch(&mut app, 'n');
    for c in "quick FIX".chars() {
        ch(&mut app, c);
    }
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::ProjectName));
    assert_eq!(app.active_notice(), Some("There is a squad called Quick fix already. Give this one another name."));
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn d_deletes_one_of_the_users_after_a_yes_and_a_replaced_built_in_comes_back() {
    let (w, mut app) = squads_world("squads-delete");
    let dir = squads::dir(&w.config);
    pick(&mut app, "feature");
    ch(&mut app, 'd');
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.active_notice(), Some("Feature is built in, so it can't be deleted. e changes it."));

    pick(&mut app, "review");
    ch(&mut app, 'd');
    assert!(matches!(&app.mode, Mode::Confirm(Confirm::DeleteSquad { replaces: true, .. })));
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let text = flat(&draw(&mut term, &app));
    assert!(text.contains("Delete the squad \"Our review\"?") && text.contains("The built-in Our review comes back"), "{text}");
    ch(&mut app, 'n');
    assert!(dir.join("review.md").exists(), "no: nothing deleted");
    ch(&mut app, 'd');
    ch(&mut app, 'y');
    assert!(!dir.join("review.md").exists() && dir.join("review.md.bak").exists());
    assert_eq!(app.sqt.picked().map(|s| (s.name.as_str(), s.origin)), Some(("Review", Origin::BuiltIn)));
    assert!(app.active_notice().is_some_and(|n| n.contains("the built-in one is back")));

    pick(&mut app, "sloppy");
    ch(&mut app, 'd');
    let text = flat(&draw(&mut term, &app));
    assert!(text.contains("kept as sloppy.md.bak"), "{text}");
    ch(&mut app, 'y');
    assert!(dir.join("sloppy.md.bak").exists() && app.sqt.list.iter().all(|s| s.id != "sloppy"));
    assert_eq!(app.sqt.selected, app.sqt.list.len() - 1, "the cursor stays in the list");
}

#[test]
fn a_change_made_by_hand_shows_at_the_next_refresh_and_the_help_has_the_tabs_keys() {
    let (w, mut app) = squads_world("squads-refresh");
    fs::write(squads::path(&w.config, "alpha"), "---\nname: Alpha\ndescription: First of mine\nmodel: haiku\n---\n1. Go.\n").unwrap();
    app.refresh_for_test();
    assert!(app.sqt.list.iter().any(|s| s.id == "alpha" && s.origin == Origin::Own));
    // An unreadable squads folder is no squads of the user's, not an error.
    fs::remove_dir_all(squads::dir(&w.config)).unwrap();
    fs::write(squads::dir(&w.config), "not a folder").unwrap();
    app.refresh_for_test();
    assert_eq!(app.sqt.list.len(), squads::BUILT_IN.len());
    ch(&mut app, '?');
    let mut term = Terminal::new(TestBackend::new(120, 50)).unwrap();
    let text = flat(&draw(&mut term, &app));
    for want in ["enter open", "e edit", "n new squad", "1-4 the tabs: Sessions, Tasks, Projects, Squads", "A squad is a Markdown file"] {
        assert!(text.contains(want), "{want}\n{text}");
    }
}
