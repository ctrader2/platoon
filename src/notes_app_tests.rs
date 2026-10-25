//! Editing a project's notes, at the level of the app: the template, the editor list `e` opens (with the one used
//! last starred and picked), running an editor (a stand-in script, never a real editor), the before/after view and
//! undo. Every directory is a temp dir (`World`); the only processes started are the tests' own stand-in editors.

use crate::app::{App, Mode, Pane};
use crate::editors::Editor;
use crate::notes;
use crate::testkit::World;
use crate::ui;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

/// Shift+arrow: moves between the panes of a project's screen.
fn shift(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::SHIFT));
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

/// A stand-in editor: a script in the world's own folder. `body` is the shell it runs (`$1` is the file).
fn fake_editor(w: &World, name: &str, terminal: bool, body: &str) -> Editor {
    let path = w.base.join(name);
    fs::create_dir_all(&w.base).unwrap();
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    Editor { id: name.into(), name: name.into(), program: path.to_string_lossy().into_owned(), args: Vec::new(), terminal }
}

/// A world with one project, its screen open and the notes pane in focus. Returns the app and the project's id.
fn on_the_notes(name: &str) -> (World, App, String) {
    let w = World::new(name);
    w.transcript("aaa-1", "/tmp", "a conversation", 10);
    let mut app = w.app();
    let id = app.projects.create("Web shop").unwrap();
    app.projects.assign("aaa-1", Some(&id)).unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Enter);
    assert!(app.open_project().is_some(), "the project's screen is open");
    shift(&mut app, KeyCode::Right);
    assert_eq!(app.pane, Pane::Notes);
    (w, app, id)
}

fn notes_of(w: &World, id: &str) -> String {
    fs::read_to_string(crate::projects::notes_path(&w.config, id)).unwrap_or_default()
}

fn set_notes(w: &World, id: &str, text: &str) {
    let p = crate::projects::notes_path(&w.config, id);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

/// Offer these editors and make `id` the one used last, so `e` offers it first and enter opens it.
fn choose(w: &World, app: &mut App, list: Vec<Editor>, id: &str) {
    app.set_editors_for_test(list);
    let mut s = app.settings().get();
    s.notes_editor = id.into();
    crate::settings::save(&w.config, &s).unwrap();
    app.refresh_for_test();
    assert_eq!(app.settings().get().notes_editor, id);
}

/// `e` and then enter: the list opens on the editor used last, so this is the one-keystroke way in.
fn edit(app: &mut App) {
    key(app, KeyCode::Char('e'));
    assert!(matches!(app.mode, Mode::PickEditor), "e opens the list of editors");
    key(app, KeyCode::Enter);
}

/// Run the app's polling until the editor it started has finished (or the test gives up).
fn wait_for_editor(app: &mut App) {
    let start = Instant::now();
    while app.notes_busy() {
        assert!(start.elapsed() < Duration::from_secs(10), "the stand-in editor never finished");
        app.poll_editors();
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn e_on_a_project_with_no_notes_starts_them_from_the_template_and_runs_the_editor_inside_platoon() {
    let (w, mut app, id) = on_the_notes("notes-template");
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    assert!(draw(&mut term, &app).contains("No notes yet"));
    let ed = fake_editor(&w, "fakeed", true, "echo EDITING; printf 'A line the editor added.\\n' >> \"$1\"");
    choose(&w, &mut app, vec![ed], "fakeed");
    edit(&mut app);
    assert!(matches!(app.mode, Mode::EditNotes), "the editor has platoon's window");
    // The file was started from the template before the editor could see it.
    let started = notes_of(&w, &id);
    assert!(started.starts_with(notes::TEMPLATE.lines().next().unwrap()), "{started}");
    for heading in ["# Summary", "# Decisions"] {
        assert!(started.contains(heading), "{heading} missing:\n{started}");
    }
    // The editor's own screen is what platoon draws while it runs.
    wait_until("the editor to draw", || {
        app.editing.as_ref().is_some_and(|e| e.run.parser().lock().unwrap().screen().contents().contains("EDITING"))
    });
    assert!(draw(&mut term, &app).contains("EDITING"), "the editor's screen shows inside platoon");
    assert!(draw(&mut term, &app).contains("ctrl+space"), "and the way out");
    wait_for_editor(&mut app);
    assert!(matches!(app.mode, Mode::List), "back on the project's screen when the editor exits");
    assert!(notes_of(&w, &id).contains("A line the editor added."));
    let shown = draw(&mut term, &app);
    assert!(shown.contains("A line the editor added."), "the pane shows the file as it is now:\n{shown}");
    assert!(app.active_notice().is_some_and(|n| n.contains("changed")), "{:?}", app.active_notice());
}

#[test]
fn e_lists_the_editors_with_the_one_used_last_starred_first_and_picked_and_remembers_the_choice() {
    let (w, mut app, id) = on_the_notes("notes-pick");
    set_notes(&w, &id, "# Overview\nAs it was.\n");
    let first = fake_editor(&w, "firsted", true, "printf 'first\\n' >> \"$1\"");
    let second = fake_editor(&w, "seconded", true, "printf 'second\\n' >> \"$1\"");
    app.set_editors_for_test(vec![first, second]);
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();

    // Nothing used yet: the list is in the order the editors were found, nothing is starred, the first is picked.
    assert_eq!(app.settings().get().notes_editor, "ask", "no editor remembered yet");
    key(&mut app, KeyCode::Char('e'));
    assert!(matches!(app.mode, Mode::PickEditor), "e opens the list");
    assert_eq!(app.epick.options.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["firsted", "seconded"]);
    assert_eq!((app.epick.selected, app.epick.starred.as_str()), (0, ""));
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("firsted") && shown.contains("seconded"), "both are listed: {shown}");
    assert!(shown.contains("here in platoon"), "and where each one runs: {shown}");
    assert!(!shown.contains("★"), "nothing is starred yet: {shown}");
    assert!(shown.contains("starred here next time"), "{shown}");

    // The second one: it runs, and it is remembered as the one used last.
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    wait_for_editor(&mut app);
    assert_eq!(notes_of(&w, &id), "# Overview\nAs it was.\nsecond\n", "the one picked ran");
    assert_eq!(app.settings().get().notes_editor, "seconded", "remembered as the last one used");
    assert_eq!(crate::settings::load(&w.config).0.notes_editor, "seconded", "and it is in the settings file");

    // Next time it is starred, first and picked, so enter alone opens it again.
    key(&mut app, KeyCode::Char('e'));
    assert_eq!(app.epick.options.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["seconded", "firsted"]);
    assert_eq!((app.epick.selected, app.epick.starred.as_str()), (0, "seconded"));
    let shown = draw(&mut term, &app);
    let starred = shown.lines().find(|l| l.contains("seconded")).unwrap_or("");
    assert!(starred.contains("★"), "the one used last carries the star: {starred}");
    assert_eq!(shown.matches('★').count(), 2, "one on the row, one in the line under the list:\n{shown}");
    assert!(flat(&shown).contains("the one you used last: enter opens it again"), "{shown}");
    key(&mut app, KeyCode::Enter);
    wait_for_editor(&mut app);
    assert!(notes_of(&w, &id).ends_with("second\nsecond\n"), "enter used the starred one: {}", notes_of(&w, &id));
    assert_eq!(app.settings().get().notes_editor, "seconded");
}

#[test]
fn esc_on_the_editor_list_leaves_the_notes_alone() {
    let (w, mut app, id) = on_the_notes("notes-ask");
    app.set_editors_for_test(vec![fake_editor(&w, "fakeed", true, "printf 'x\\n' >> \"$1\"")]);
    key(&mut app, KeyCode::Char('e'));
    assert!(matches!(app.mode, Mode::PickEditor));
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List));
    assert!(!crate::projects::notes_path(&w.config, &id).exists(), "nothing was written");
    assert!(!app.notes_busy());
    assert_eq!(app.settings().get().notes_editor, "ask", "and nothing was remembered");
}

#[test]
fn an_editor_this_computer_no_longer_has_is_not_starred_and_the_list_still_works() {
    let (w, mut app, id) = on_the_notes("notes-gone-editor");
    set_notes(&w, &id, "# Overview\nA project.\n");
    let ed = fake_editor(&w, "fakeed", true, "printf 'x\\n' >> \"$1\"");
    app.set_editors_for_test(vec![ed]);
    let mut s = app.settings().get();
    s.notes_editor = "one-i-uninstalled".into();
    crate::settings::save(&w.config, &s).unwrap();
    app.refresh_for_test();
    key(&mut app, KeyCode::Char('e'));
    assert_eq!((app.epick.selected, app.epick.starred.as_str()), (0, ""));
    assert_eq!(app.epick.options.len(), 1);
    key(&mut app, KeyCode::Enter);
    wait_for_editor(&mut app);
    assert_eq!(app.settings().get().notes_editor, "fakeed", "the one really used is remembered instead");
}

#[test]
fn an_editor_with_its_own_window_says_so_and_platoon_carries_on() {
    let (w, mut app, id) = on_the_notes("notes-window");
    set_notes(&w, &id, "# Overview\nBefore.\n");
    // It takes a moment, as a window app does, and then writes.
    let ed = fake_editor(&w, "winded", false, "sleep 0.3; printf 'from the window\\n' >> \"$1\"");
    choose(&w, &mut app, vec![ed], "winded");
    edit(&mut app);
    assert!(matches!(app.mode, Mode::List), "platoon stays where it was");
    assert!(app.notes_busy() && app.notes_editor_open() == Some("winded"));
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("Open in winded, in a window of its own."), "{shown}");
    // The list still works while it is open, and platoon refuses to write the notes itself.
    shift(&mut app, KeyCode::Left);
    assert_eq!(app.pane, Pane::Sessions);
    shift(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Char('u'));
    assert!(app.active_notice().is_some_and(|n| n.contains("open in winded")), "{:?}", app.active_notice());
    assert_eq!(notes_of(&w, &id), "# Overview\nBefore.\n");
    wait_for_editor(&mut app);
    assert!(notes_of(&w, &id).ends_with("from the window\n"));
    assert!(flat(&draw(&mut term, &app)).contains("from the window"), "the notes are read again");
    assert!(app.notes_editor_open().is_none());
}

#[test]
fn c_shows_what_changed_with_marks_that_read_without_color_and_u_puts_it_back() {
    let (w, mut app, id) = on_the_notes("notes-changes");
    set_notes(&w, &id, "# Overview\nSell socks.\n# Status\nStarted.\n");
    // Not `sed -i`: macOS's sed reads the next word as a backup suffix. Rewritten in place, as an editor saves.
    let ed = fake_editor(&w, "fakeed", true, "t=$(sed 's/Sell socks./Sell socks and hats./' \"$1\") && printf '%s\\n' \"$t\" > \"$1\"");
    choose(&w, &mut app, vec![ed], "fakeed");
    edit(&mut app);
    wait_for_editor(&mut app);
    assert!(notes_of(&w, &id).contains("socks and hats"));

    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    key(&mut app, KeyCode::Char('c'));
    assert!(matches!(app.mode, Mode::Changes));
    let shown = draw(&mut term, &app);
    assert!(shown.contains("- Sell socks."), "what went, marked with a minus:\n{shown}");
    assert!(shown.contains("+ Sell socks and hats."), "what came, marked with a plus:\n{shown}");
    assert!(shown.contains("was there before") && shown.contains("is there now"), "the marks are explained:\n{shown}");
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List));

    // u puts the earlier version back, and can itself be undone.
    key(&mut app, KeyCode::Char('u'));
    assert_eq!(notes_of(&w, &id), "# Overview\nSell socks.\n# Status\nStarted.\n");
    assert!(app.active_notice().is_some_and(|n| n.contains("back as they were")), "{:?}", app.active_notice());
    assert!(draw(&mut term, &app).contains("Sell socks."));
    key(&mut app, KeyCode::Char('u'));
    assert!(notes_of(&w, &id).contains("socks and hats"), "undoing the undo brings it back");
    // With no earlier version at all, both keys say so plainly and nothing is written.
    let (w2, mut app2, id2) = on_the_notes("notes-nochanges");
    set_notes(&w2, &id2, "# Overview\nOnly ever this.\n");
    key(&mut app2, KeyCode::Char('c'));
    assert!(matches!(app2.mode, Mode::List) && app2.active_notice().is_some_and(|n| n.contains("nothing to compare")));
    key(&mut app2, KeyCode::Char('u'));
    assert!(app2.active_notice().is_some_and(|n| n.contains("nothing to undo")), "{:?}", app2.active_notice());
    assert_eq!(notes_of(&w2, &id2), "# Overview\nOnly ever this.\n");
}

#[test]
fn an_editor_that_changes_nothing_leaves_no_copy_behind_and_does_not_bury_the_last_change() {
    let (w, mut app, id) = on_the_notes("notes-nochange");
    set_notes(&w, &id, "# Overview\nFirst.\n");
    let ed = fake_editor(&w, "fakeed", true, "printf 'Second.\\n' >> \"$1\"");
    choose(&w, &mut app, vec![ed], "fakeed");
    edit(&mut app);
    wait_for_editor(&mut app);
    assert_eq!(notes::versions(&w.config, &id).len(), 1);

    // Now an editor that is opened and closed again with nothing written.
    let idle = fake_editor(&w, "idleed", true, "true");
    choose(&w, &mut app, vec![idle], "idleed");
    edit(&mut app);
    wait_for_editor(&mut app);
    assert!(app.active_notice().is_some_and(|n| n.contains("unchanged")), "{:?}", app.active_notice());
    assert_eq!(notes::versions(&w.config, &id).len(), 1, "no copy of a copy");
    // The change before it is still the one that undo and the before/after view act on.
    key(&mut app, KeyCode::Char('u'));
    assert_eq!(notes_of(&w, &id), "# Overview\nFirst.\n");
}

#[test]
fn long_notes_are_not_warned_about_at_all_and_nothing_is_cut() {
    let (w, mut app, id) = on_the_notes("notes-big");
    let body = "Sell socks to the world.\n".repeat(400); // about 10 KB
    set_notes(&w, &id, &format!("# Overview\n{body}"));
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    app.reload_for_test();
    assert!(!flat(&draw(&mut term, &app)).contains("costs tokens"));
    set_notes(&w, &id, &format!("# Overview\n{}", body.repeat(4))); // about 40 KB, twice the old warning
    let shown = flat(&draw(&mut term, &app));
    assert!(!shown.contains("costs tokens") && !shown.contains("KB"), "length is the project's business now:\n{shown}");
    assert_eq!(notes_of(&w, &id).len(), "# Overview\n".len() + body.len() * 4, "the file itself is left whole");
}

#[test]
fn the_pane_says_in_one_line_when_platoon_last_updated_the_notes_and_where_they_go() {
    let (w, mut app, id) = on_the_notes("notes-reach");
    let reach = "Every session in this project starts with these notes.";
    // platoon's own old line at the top of the file is left out; the one line above the notes says it now.
    set_notes(&w, &id, &format!("{}\n\n# Overview\nSell socks.\n", notes::OLD_PREAMBLES[0]));
    app.reload_for_test();
    // Wide enough for the line to fit the notes pane in one.
    let mut term = Terminal::new(TestBackend::new(200, 30)).unwrap();
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains(reach) && shown.contains("Sell socks."), "{shown}");
    assert!(!shown.contains("platoon last updated") && !shown.contains("keeps these up to date"), "{shown}");
    assert!(!shown.contains("latest version"), "no count of sessions any more: {shown}");

    // After an update: when, on the same line.
    let mut state = crate::notesupdate::State::load(&w.config, &id);
    state.last_update_at = crate::usage::now() - 300;
    state.save(&w.config, &id).unwrap();
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains(&format!("platoon last updated these 5m ago. {reach}")), "{shown}");

    // While an update runs (a live mark from this process): that part says so, in the theme's warning color.
    let mark = crate::notesrun::running_path(&w.cache, &id);
    std::fs::create_dir_all(mark.parent().unwrap()).unwrap();
    std::fs::write(&mark, std::process::id().to_string()).unwrap();
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains(&format!("platoon is currently updating the project notes. {reach}")), "{shown}");
    let t = app.effective_theme();
    let buf = term.backend().buffer().clone();
    // The color of the cell where `word` starts.
    let at = |word: &str| {
        let row =
            (0..buf.area.height).find(|&y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>().contains(word)).unwrap();
        let line: String = (0..buf.area.width).map(|x| buf[(x, row)].symbol()).collect();
        let col = line[..line.find(word).unwrap()].chars().count() as u16;
        buf[(col, row)].fg
    };
    assert_eq!(at("currently updating"), t.warn);
    assert_eq!(at("Every session in this"), t.dim);
}

#[test]
fn the_notes_pane_offers_its_keys_and_the_sessions_pane_keeps_its_own() {
    let (w, mut app, id) = on_the_notes("notes-hints");
    set_notes(&w, &id, "# Overview\nSomething.\n");
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    let shown = draw(&mut term, &app);
    for hint in ["e edit notes", "c changes", "u undo"] {
        assert!(shown.contains(hint), "{hint:?} missing from the footer:\n{shown}");
    }
    assert!(!shown.contains("E editor"), "the separate editor key is retired:\n{shown}");
    shift(&mut app, KeyCode::Left);
    let shown = draw(&mut term, &app);
    assert!(shown.contains("a archive") && !shown.contains("c changes"), "the sessions pane keeps its own keys:\n{shown}");
    // The notes keys do nothing there: `u` on a session is not an undo.
    key(&mut app, KeyCode::Char('u'));
    assert!(app.active_notice().is_none());
}

#[test]
fn a_change_made_on_disk_while_platoon_watches_is_what_is_shown_and_what_is_undone() {
    let (w, mut app, id) = on_the_notes("notes-disk");
    set_notes(&w, &id, "# Overview\nplatoon read this.\n");
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    assert!(draw(&mut term, &app).contains("platoon read this."));
    // Somebody else's editor writes the file behind platoon's back.
    let ed = fake_editor(&w, "fakeed", true, "printf 'and the editor added this.\\n' >> \"$1\"");
    choose(&w, &mut app, vec![ed], "fakeed");
    set_notes(&w, &id, "# Overview\nsomebody else wrote this.\n");
    assert!(draw(&mut term, &app).contains("somebody else wrote this."), "the file on disk is what is shown");
    edit(&mut app);
    wait_for_editor(&mut app);
    // The version kept before the editor ran is what was really on disk, so undo puts that back.
    assert_eq!(notes_of(&w, &id), "# Overview\nsomebody else wrote this.\nand the editor added this.\n");
    key(&mut app, KeyCode::Char('u'));
    assert_eq!(notes_of(&w, &id), "# Overview\nsomebody else wrote this.\n");
}

#[test]
fn twenty_versions_are_kept_per_project_and_the_oldest_go() {
    let (w, mut app, id) = on_the_notes("notes-rotate");
    let ed = fake_editor(&w, "fakeed", true, "printf 'one more line\\n' >> \"$1\"");
    choose(&w, &mut app, vec![ed], "fakeed");
    for _ in 0..(notes::MAX_VERSIONS + 5) {
        edit(&mut app);
        wait_for_editor(&mut app);
    }
    let kept = notes::versions(&w.config, &id);
    assert_eq!(kept.len(), notes::MAX_VERSIONS);
    let newest = fs::read_to_string(&kept[0]).unwrap();
    let now = notes_of(&w, &id);
    assert_eq!(newest.lines().count() + 1, now.lines().count(), "the newest version is the one before this");
    let history: PathBuf = notes::history_dir(&w.config, &id);
    assert!(history.starts_with(&w.config), "the copies live beside the notes, in the config folder");
}

#[test]
fn nothing_here_ever_touches_the_real_config_or_starts_a_real_editor() {
    // The detection only reads; it never runs anything. On this machine it finds whatever is installed, and on a
    // machine with nothing it still offers something to edit with.
    let list = crate::editors::detect();
    assert!(!list.is_empty());
    assert!(list.iter().all(|e| !e.name.is_empty() && !e.program.is_empty()));
    let (w, app, _) = on_the_notes("notes-safe");
    assert!(w.config.starts_with(Path::new(&std::env::temp_dir())), "the world is a temp dir: {}", w.config.display());
    assert!(app.config.starts_with(&w.base));
    assert!(!app.notes_busy());
}

#[test]
fn typing_in_the_editor_reaches_it_and_ctrl_space_gets_out_of_a_wedged_one() {
    let (w, mut app, id) = on_the_notes("notes-typing");
    set_notes(&w, &id, "# Overview\n");
    // Reads one line from its terminal and writes it into the file, as a real editor would.
    let ed = fake_editor(&w, "fakeed", true, "echo READY; read line; printf '%s\\n' \"$line\" >> \"$1\"");
    choose(&w, &mut app, vec![ed], "fakeed");
    edit(&mut app);
    wait_until("the editor to be ready", || {
        app.editing.as_ref().is_some_and(|e| e.run.parser().lock().unwrap().screen().contents().contains("READY"))
    });
    typed(&mut app, "hello from platoon");
    key(&mut app, KeyCode::Enter);
    wait_for_editor(&mut app);
    assert_eq!(notes_of(&w, &id), "# Overview\nhello from platoon\n");

    // One that never exits: ctrl+space asks "Leave the editor?", and `y` ends it (it is platoon's own child) and gives
    // the screen back. Anything else stays: enter, esc, n and the back key again.
    let stuck = fake_editor(&w, "stucked", true, "echo STUCK; while :; do sleep 0.05; done");
    choose(&w, &mut app, vec![stuck], "stucked");
    edit(&mut app);
    wait_until("the stuck editor to draw", || {
        app.editing.as_ref().is_some_and(|e| e.run.parser().lock().unwrap().screen().contents().contains("STUCK"))
    });
    let back = || KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL);
    let asking = |app: &App| matches!(app.mode, Mode::EditNotes) && app.editing.as_ref().is_some_and(|e| e.leaving);
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("ctrl+space leave the editor") && !f.contains("Leave the editor?"), "{f}");
    app.on_key(back());
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("Leave the editor?") && f.contains("Anything you haven't saved there is lost."), "{f}");
    assert!(f.contains("y leave") && f.contains("n stay"), "{f}");
    key(&mut app, KeyCode::Char('n'));
    let f = flat(&draw(&mut term, &app));
    assert!(!f.contains("Leave the editor?") && f.contains("STUCK"), "the question goes, the editor stays: {f}");
    // By mouse: the bar's hint asks, "n stay" stays (never back to the list behind it).
    app.act(crate::app::Action::Back);
    assert!(asking(&app));
    app.act(crate::app::Action::Cancel);
    assert!(matches!(app.mode, Mode::EditNotes) && !asking(&app), "a click on stay stays");
    for stay in [KeyCode::Enter, KeyCode::Esc, KeyCode::Char('n')] {
        app.on_key(back());
        assert!(asking(&app), "the back key asks first");
        key(&mut app, stay);
        assert!(matches!(app.mode, Mode::EditNotes) && !asking(&app), "{stay:?} stays in the editor");
    }
    app.on_key(back());
    app.on_key(back());
    assert!(matches!(app.mode, Mode::EditNotes) && !asking(&app), "the back key twice stays");
    app.on_key(back());
    key(&mut app, KeyCode::Char('y'));
    assert!(matches!(app.mode, Mode::List) && !app.notes_busy(), "back on the project's screen");
}

#[test]
fn notes_past_the_cap_are_warned_about_in_the_warning_colour() {
    let (w, app, id) = on_the_notes("notes-cap");
    let mut term = Terminal::new(TestBackend::new(140, 30)).unwrap();
    set_notes(&w, &id, "# Summary\n\nA shop.\n");
    assert!(!flat(&draw(&mut term, &app)).contains("go to sessions"), "short notes: no warning");
    // 12,400 characters.
    let long = format!("# Summary\n\nA shop.\n\n# Mine\n\n{}", format!("{}\n", "a".repeat(99)).repeat(120));
    let long = format!("{long}{}", "b".repeat(12_400 - notes::chars(&long)));
    assert_eq!(notes::chars(&long), 12_400);
    set_notes(&w, &id, &long);
    let warning = "Notes are 12,400 characters; only the first 8,000 go to sessions.";
    assert!(flat(&draw(&mut term, &app)).contains(warning), "{}", draw(&mut term, &app));
    let buf = term.backend().buffer().clone();
    let warn = app.effective_theme().warn;
    let found = (0..buf.area.height).find_map(|y| {
        let row: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
        row.find("Notes are 12,400").map(|at| (row[..at].chars().count() as u16, y))
    });
    let (x, y) = found.expect("the warning is on one line");
    for dx in 0.."Notes are 12,400".len() as u16 {
        assert_eq!(buf[(x + dx, y)].fg, warn, "in the warning colour");
    }
}

#[test]
fn the_help_says_where_the_notes_cap_and_the_offer_to_make_tasks_apply() {
    let (w, mut app, _id) = on_the_notes("notes-cap-help");
    let mut term = Terminal::new(TestBackend::new(150, 70)).unwrap();
    let offer = "just before it starts the work: \"Turn this into platoon tasks for a squad?\"";
    key(&mut app, KeyCode::Char('?'));
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("Notes go to the project's sessions up to 8,000 characters."), "{shown}");
    assert!(shown.contains(offer), "the project's sessions are listed here too: {shown}");
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Char('1'));
    key(&mut app, KeyCode::Char('?'));
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains(offer) && shown.contains("\"Offer tasks before starting work\"") && !shown.contains("8,000"), "{shown}");
    // With the setting off no session asks, and the help doesn't say one does.
    key(&mut app, KeyCode::Char('?'));
    let mut s = crate::settings::load(&w.config).0;
    s.offer_tasks = false;
    crate::settings::save(&w.config, &s).unwrap();
    app.refresh_with(Vec::new(), false);
    key(&mut app, KeyCode::Char('?'));
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("Good to know") && !shown.contains("platoon tasks for a squad"), "{shown}");
}
