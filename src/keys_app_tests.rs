//! The back and quit keys at the level of the app: the back key (Ctrl+Space, or what keys.json says) and Esc both go
//! back one level on every screen and cancel every popup; `q` goes back on a project's or a task's screen, and quits
//! only from the top-level lists; a keys.json with other keys is followed (and the hints
//! say them), a bad entry is said once and its default used. Every directory is a temp dir (`World`); the only
//! processes started are the tests' own stand-in editors.

use crate::app::{App, Mode, Pane};
use crate::client::Client;
use crate::editors::Editor;
use crate::host::{F_INPUT, read_frame};
use crate::testkit::World;
use crate::tickets::{NewTicket, Tickets};
use crate::ui;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    thread,
    time::{Duration, Instant},
};

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn ch(app: &mut App, c: char) {
    key(app, KeyCode::Char(c));
}

/// Ctrl+Space, as crossterm reads the NUL byte a terminal sends for it.
fn ctrl_space(app: &mut App) {
    app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
}

fn alt(app: &mut App, c: char) {
    app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT));
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

/// Write keys.json and let the app see it (it is read again when it changes, as settings.json is).
fn keys_file(w: &World, app: &mut App, text: &str) {
    fs::create_dir_all(&w.config).unwrap();
    crate::projects::write_atomic(&w.config.join("keys.json"), text).unwrap();
    app.refresh_for_test();
}

/// A project "Web shop" with one session and ticket #1, the project's screen open.
fn on_the_project(name: &str) -> (World, App) {
    let w = World::new(name);
    w.transcript("aaa-1", "/tmp", "a conversation", 10);
    let mut app = w.app();
    let id = app.projects.create("Web shop").unwrap();
    app.projects.assign("aaa-1", Some(&id)).unwrap();
    Tickets::open(&w.config, &id).add(NewTicket { title: "Basket page".into(), ..Default::default() }).unwrap();
    app.reload_for_test();
    ch(&mut app, '2');
    key(&mut app, KeyCode::Enter);
    assert!(app.open_project().is_some(), "the project's screen is open");
    (w, app)
}

/// Task #1's own screen, from the Tasks tab (it never started: enter shows its text).
fn open_ticket(app: &mut App) {
    ch(app, '3');
    key(app, KeyCode::Enter);
    assert_eq!(app.tk.open, Some(1), "the task's screen is open");
}

/// Where the app is: (a project open, a task open, quit asked).
fn at(app: &App) -> (bool, Option<u32>, bool) {
    (app.open_project().is_some(), app.tk.open, app.quit)
}

#[test]
fn the_back_key_and_esc_each_go_back_one_level_and_never_quit() {
    for back in [ctrl_space as fn(&mut App), |a: &mut App| key(a, KeyCode::Esc)] {
        let (_w, mut app) = on_the_project("keys-levels");
        back(&mut app);
        assert_eq!(at(&app), (false, None, false), "the project -> the list");
        open_ticket(&mut app);
        back(&mut app);
        assert_eq!(at(&app), (false, None, false), "the task -> the Tasks tab");
        assert!(matches!(app.mode, Mode::List) && app.board_showing());
        // At the top it does what esc does there (clears the filter), and never quits.
        app.filter = "zzz".into();
        back(&mut app);
        assert_eq!(at(&app), (false, None, false));
        assert!(app.filter.is_empty(), "the filter is cleared");
    }
}

#[test]
fn q_goes_back_on_a_project_or_task_screen_and_quits_only_from_the_top_level_list() {
    let (_w, mut app) = on_the_project("keys-q");
    ch(&mut app, 'q');
    assert_eq!(at(&app), (false, None, false), "q on a project: back to the list");
    open_ticket(&mut app);
    ch(&mut app, 'q');
    assert_eq!(at(&app), (false, None, false), "q on a task: back to the Tasks tab");
    ch(&mut app, 'q');
    assert!(app.quit, "q on the Tasks tab quits");

    // The hints say so: no "q quit" on a project's or a task's screen, `esc back` instead; "q quit" at the top.
    let (_w, mut app) = on_the_project("keys-q-hints");
    let mut term = Terminal::new(TestBackend::new(160, 30)).unwrap();
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("esc back") && !shown.contains("q quit"), "{shown}");
    ch(&mut app, 'q');
    open_ticket(&mut app);
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("esc back") && !shown.contains("q quit"), "{shown}");
    ch(&mut app, 'q');
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("q quit"), "{shown}");
}

#[test]
fn the_back_key_cancels_the_settings_the_theme_list_a_box_the_search_and_a_question() {
    let w = World::new("keys-popups");
    w.transcript("aaa-1", "/tmp", "a conversation", 10);
    let mut app = w.app();
    let before = app.settings().get();
    // Settings: ctrl+space closes them (and is not the space that changes the entry it is on).
    ch(&mut app, 's');
    assert!(matches!(app.mode, Mode::Settings));
    ctrl_space(&mut app);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.settings().get(), before, "nothing changed");
    // The theme list (the Theme row is below the two Programs): back puts the old theme back, then back again closes the
    // settings.
    ch(&mut app, 's');
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Themes));
    key(&mut app, KeyCode::Down);
    assert!(app.preview.is_some());
    ctrl_space(&mut app);
    assert!(matches!(app.mode, Mode::Settings) && app.preview.is_none());
    ctrl_space(&mut app);
    assert!(matches!(app.mode, Mode::List));
    // The rename box: the name stays what it was.
    ch(&mut app, 'r');
    assert!(matches!(app.mode, Mode::Rename));
    ch(&mut app, 'x');
    ctrl_space(&mut app);
    assert!(matches!(app.mode, Mode::List));
    assert!(!w.config.join("names.json").exists(), "nothing saved");
    // The search.
    ch(&mut app, '/');
    assert!(matches!(app.mode, Mode::Search));
    ctrl_space(&mut app);
    assert!(matches!(app.mode, Mode::List));
    // A question (archive?) is answered no.
    ch(&mut app, 'a');
    if matches!(app.mode, Mode::Confirm(_)) {
        ctrl_space(&mut app);
        assert!(matches!(app.mode, Mode::List));
    }
    assert!(!app.quit);
}

#[test]
fn keys_json_changes_the_back_and_quit_keys_and_the_hints_say_them() {
    let (w, mut app) = on_the_project("keys-custom");
    keys_file(&w, &mut app, r#"{ "back": "alt+x", "quit": "w" }"#);
    assert_eq!(app.active_notice(), None, "nothing to say about a good file");
    let mut term = Terminal::new(TestBackend::new(160, 30)).unwrap();
    let shown = flat(&draw(&mut term, &app));
    assert!(
        shown.contains("esc back") && !shown.contains("ctrl+space") && !shown.contains("alt+x"),
        "platoon's own screens say esc: {shown}"
    );
    assert_eq!(app.back_label(), "alt+x", "where Esc is the session's or the editor's, the hints name the back key from keys.json");
    // The quit key goes back too, below the top; q is nothing now.
    ch(&mut app, 'q');
    assert_eq!(at(&app), (true, None, false));
    ch(&mut app, 'w');
    assert_eq!(at(&app), (false, None, false));
    open_ticket(&mut app);
    // Ctrl+Space is nothing now; alt+x goes back; so does esc, always.
    ctrl_space(&mut app);
    assert_eq!(at(&app), (false, Some(1), false));
    alt(&mut app, 'x');
    assert_eq!(at(&app), (false, None, false));
    ch(&mut app, '1');
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("w quit") && !shown.contains("q quit"), "{shown}");
    ch(&mut app, 'q');
    assert!(!app.quit, "q no longer quits");
    ch(&mut app, 's');
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("esc back") && !shown.contains("alt+x"), "the settings' hint too: {shown}");
    ctrl_space(&mut app);
    assert!(matches!(app.mode, Mode::Settings), "ctrl+space no longer closes them");
    alt(&mut app, 'x');
    assert!(matches!(app.mode, Mode::List));
    ch(&mut app, 'w');
    assert!(app.quit);
}

#[test]
fn a_bad_entry_is_said_once_and_its_default_used() {
    let (w, mut app) = on_the_project("keys-bad");
    keys_file(&w, &mut app, r#"{ "back": "x", "quit": "w" }"#);
    let said = app.active_notice().expect("a notice").to_string();
    assert!(said.contains("keys.json") && said.contains("\"back\"") && said.contains("Using ctrl+space"), "{said}");
    assert!(!said.contains("quit"), "only the bad entry: {said}");
    // The default works, the good entry is used.
    ctrl_space(&mut app);
    assert!(app.open_project().is_none());
    ch(&mut app, 'x');
    assert!(!app.quit, "x is not the back key, nor quit");
    // Said once: read again with the same problem, nothing more is said.
    app.notice = None;
    app.refresh_for_test();
    keys_file(&w, &mut app, r#"{ "back": "x", "quit": "w" }"#);
    assert_eq!(app.active_notice(), None);
    // The file is never written by platoon.
    assert_eq!(fs::read_to_string(w.config.join("keys.json")).unwrap(), r#"{ "back": "x", "quit": "w" }"#);
    // A file that isn't JSON: the defaults, said.
    keys_file(&w, &mut app, "{ back: ");
    assert!(app.active_notice().is_some_and(|n| n.contains("keys.json")), "{:?}", app.active_notice());
    ch(&mut app, 'q');
    assert!(app.quit, "the default quit key");
}

/// Everything the client sent to the session within a moment.
/// The session's end of a test client, with a read timeout set while the client is still connected: macOS refuses
/// the option (EINVAL) on a socket whose other end has closed, as it has once the back key dropped the client.
fn for_test(rows: u16, cols: u16) -> (Client, std::os::unix::net::UnixStream) {
    let (client, host) = Client::for_test(rows, cols);
    host.set_read_timeout(Some(Duration::from_millis(150))).unwrap();
    (client, host)
}

fn sent(host: &mut std::os::unix::net::UnixStream) -> Vec<u8> {
    let mut out = Vec::new();
    while let Ok((kind, payload)) = read_frame(host) {
        if kind == F_INPUT {
            out.extend(payload);
        }
    }
    out
}

#[test]
fn in_a_session_the_back_key_leaves_and_everything_else_goes_to_claude() {
    let w = World::new("keys-attached");
    let mut app = w.app();
    let (client, mut host) = for_test(20, 70);
    app.client = Some(client);
    app.mode = Mode::Attached(4242);
    ch(&mut app, 'q');
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::Attached(_)), "q and esc are Claude's");
    ctrl_space(&mut app);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(sent(&mut host), b"q\x1b".to_vec(), "the back key itself isn't sent");

    // With alt+b as the back key: Ctrl+Space reaches Claude (as NUL), alt+b leaves. Claude Code uses alt+b itself: said.
    keys_file(&w, &mut app, r#"{ "back": "alt+b" }"#);
    assert!(app.active_notice().is_some_and(|n| n.contains("Claude Code uses alt+b")), "{:?}", app.active_notice());
    let (client, mut host) = for_test(20, 70);
    app.client = Some(client);
    app.mode = Mode::Attached(4242);
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    assert!(flat(&draw(&mut term, &app)).contains("alt+b back to list"));
    ctrl_space(&mut app);
    assert!(matches!(app.mode, Mode::Attached(_)));
    alt(&mut app, 'b');
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(sent(&mut host), vec![0], "only ctrl+space reached the session");
}

#[test]
fn the_back_key_stops_the_notes_editor_and_otherwise_every_key_is_the_editors() {
    let (w, mut app) = on_the_project("keys-notes");
    keys_file(&w, &mut app, r#"{ "back": "ctrl+g" }"#);
    let path = w.base.join("slow-editor");
    fs::write(&path, "#!/bin/sh\necho EDITING\nexec sleep 30\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    app.set_editors_for_test(vec![Editor {
        id: "slow".into(),
        name: "slow".into(),
        program: path.to_string_lossy().into_owned(),
        args: Vec::new(),
        terminal: true,
    }]);
    let mut s = app.settings().get();
    s.notes_editor = "slow".into();
    crate::settings::save(&w.config, &s).unwrap();
    app.refresh_for_test();
    app.notice = None;
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.pane, Pane::Notes);
    ch(&mut app, 'e');
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::EditNotes));
    let start = Instant::now();
    while !app.editing.as_ref().is_some_and(|e| e.run.parser().lock().unwrap().screen().contents().contains("EDITING")) {
        assert!(start.elapsed() < Duration::from_secs(10), "the stand-in editor never drew");
        thread::sleep(Duration::from_millis(10));
    }
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    assert!(flat(&draw(&mut term, &app)).contains("ctrl+g leave the editor"));
    ctrl_space(&mut app);
    key(&mut app, KeyCode::Esc);
    ch(&mut app, 'q');
    app.poll_editors();
    assert!(matches!(app.mode, Mode::EditNotes), "those are the editor's keys");
    app.on_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
    assert!(app.editing.as_ref().is_some_and(|e| e.leaving), "the back key asks first");
    ch(&mut app, 'y');
    assert!(matches!(app.mode, Mode::List), "and y stops it");
    assert!(app.open_project().is_some(), "back on the project's screen");
}
