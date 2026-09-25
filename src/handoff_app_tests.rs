//! The hand-off (`h`) at the level of the app: a real transcript on disk, a real project, a fake `claude` that
//! records every call, and what the box shows, what may be edited and what is refused. Nothing here ever starts a
//! session (that is `tests/handoff.rs`, with the real binary); every directory is a temp dir and the real
//! `claude` is never reached.

use crate::app::{App, HandoffState, Mode};
use crate::handoff;
use crate::ledger::Ledger;
use crate::projects::Projects;
use crate::settings::{self, Settings};
use crate::testkit::{AiFake, World};
use crate::usage;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::fs;

const BRIEF: &str = r#"{"done":"The basket page is in.","decisions":["Baskets are session based"],"next":"Take card payments."}"#;

/// A world with one conversation in one project, the token features on, and a stand-in `claude`.
fn world(name: &str) -> (World, AiFake) {
    let w = World::new(name);
    // A folder that really exists: a hand-off starts a new session in it, so platoon checks it is there.
    fs::create_dir_all(w.base.join("shop")).unwrap();
    fs::create_dir_all(w.base.join("other")).unwrap();
    w.transcript("sess-1", &shop(&w), "Add a basket page to the shop", 5);
    let mut s = settings::load(&w.config).0;
    s.tokens_enabled = true;
    settings::save(&w.config, &s).unwrap();
    let mut p = Projects::open(&w.config);
    let id = p.create("Web shop").unwrap();
    fs::create_dir_all(w.config.join("projects").join(&id)).unwrap();
    fs::write(p.notes_path(&id), "# Overview\n\nSell socks.\n").unwrap();
    p.assign("sess-1", Some(&id)).unwrap();
    (w, AiFake::new(name))
}

fn shop(w: &World) -> String {
    w.base.join("shop").to_string_lossy().into_owned()
}

fn other(w: &World) -> String {
    w.base.join("other").to_string_lossy().into_owned()
}

fn app_of(w: &World, fake: &AiFake) -> App {
    let mut app = w.app();
    app.set_ai_program_for_test(&fake.script);
    app.refresh_with(Vec::new(), false);
    app
}

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn ctrl(app: &mut App, c: char) {
    app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
}

fn select(app: &mut App, id: &str) {
    app.selected = app.visible().iter().position(|&i| app.sessions[i].id == id).expect("the row is listed");
}

fn wait_ready(app: &mut App) {
    app.refresh_until_for_test(10, "the brief to arrive", |a| a.handoff.state != HandoffState::Asking);
}

fn project_id(w: &World) -> String {
    Projects::open(&w.config).all()[0].id.clone()
}

#[test]
fn h_asks_for_a_brief_and_shows_it_for_the_user_to_edit_before_anything_starts() {
    let (w, fake) = world("handoff-brief");
    fake.reply_default(BRIEF);
    let mut app = app_of(&w, &fake);
    select(&mut app, "sess-1");
    // The key it used to be does nothing at all now, on either screen it works on.
    key(&mut app, KeyCode::Char('H'));
    assert!(matches!(app.mode, Mode::List) && app.handoff.text.is_empty(), "H is no longer the hand-off");
    assert_eq!(fake.calls(), 0, "and it asked Claude nothing");
    key(&mut app, KeyCode::Char('h'));
    assert!(matches!(app.mode, Mode::Handoff) && app.handoff.state == HandoffState::Asking, "the box opens while the call is out");
    assert_eq!((app.handoff.session.as_str(), app.handoff.project_name.as_str(), app.handoff.cwd.clone()), ("sess-1", "Web shop", shop(&w)));
    wait_ready(&mut app);
    assert_eq!(app.handoff.state, HandoffState::Ready);

    // The brief is platoon's own shape, filled with what came back.
    let text = app.handoff.text.clone();
    assert!(text.starts_with("Carrying on from an earlier session."), "{text}");
    assert!(text.contains("What was done:\nThe basket page is in."), "{text}");
    assert!(text.contains("- Baskets are session based"), "{text}");
    assert!(text.contains("What is next:\nTake card payments."), "{text}");
    assert!(!text.contains("Sell socks."), "the project's notes are not repeated: {text}");

    // What was sent: this conversation, its folder and its project, and never its session id.
    assert_eq!(fake.calls(), 1);
    let sent = fake.stdin(1);
    assert!(sent.contains("Project: Web shop") && sent.contains("Folder: shop"), "{sent}");
    assert!(sent.contains("- Add a basket page to the shop"), "{sent}");
    assert!(!sent.contains("sess-1"), "{sent}");
    // It is the user's own key press, so it goes as user-initiated work, and its spend is in the ledger.
    assert!(fake.argv(1).contains(&"-p".to_string()));
    let day = Ledger::open(&w.cache).today(usage::now());
    assert_eq!(day.features.get(handoff::FEATURE).map(|f| f.calls), Some(1));
    assert!(day.tokens() > 0);

    // Nothing has happened to the old session or to the project.
    assert!(!app.sessions.iter().any(|s| s.archived));
    assert_eq!(Projects::open(&w.config).project_id_of("sess-1"), Some(project_id(&w).as_str()));
}

#[test]
fn h_works_on_the_project_screen_too_and_the_old_key_does_not() {
    let (w, fake) = world("handoff-project-screen");
    fake.reply_default(BRIEF);
    let mut app = app_of(&w, &fake);
    // On to the Projects tab and into the project: its sessions pane has the focus.
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Enter);
    assert!(app.open_project().is_some(), "the project's screen is open");
    select(&mut app, "sess-1");
    key(&mut app, KeyCode::Char('H'));
    assert!(matches!(app.mode, Mode::List), "H does nothing here either");
    assert_eq!(fake.calls(), 0);
    key(&mut app, KeyCode::Char('h'));
    assert!(matches!(app.mode, Mode::Handoff), "h asks for the brief straight away, with nothing to confirm");
    wait_ready(&mut app);
    assert_eq!(app.handoff.state, HandoffState::Ready);
    assert_eq!(fake.calls(), 1);
}

#[test]
fn the_brief_can_be_typed_into_before_it_is_accepted() {
    let (w, fake) = world("handoff-edit");
    fake.reply_default(BRIEF);
    let mut app = app_of(&w, &fake);
    select(&mut app, "sess-1");
    key(&mut app, KeyCode::Char('h'));
    wait_ready(&mut app);
    let before = app.handoff.text.clone();

    // The cursor starts at the end; typing goes in there.
    assert_eq!(app.handoff.cursor, before.chars().count());
    for c in " Also: sizes.".chars() {
        key(&mut app, KeyCode::Char(c));
    }
    assert!(app.handoff.text.ends_with(" Also: sizes."), "{}", app.handoff.text);
    key(&mut app, KeyCode::Backspace);
    assert!(app.handoff.text.ends_with(" Also: sizes"), "{}", app.handoff.text);
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('x'));
    assert!(app.handoff.text.ends_with("sizes\nx"), "{}", app.handoff.text);

    // And it can be moved about: home, one to the left, a character in the middle.
    key(&mut app, KeyCode::Home);
    assert_eq!(app.handoff.cursor_at().1, 0);
    key(&mut app, KeyCode::Char('y'));
    assert!(app.handoff.text.ends_with("sizes\nyx"), "{}", app.handoff.text);
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::End);
    key(&mut app, KeyCode::Char('!'));
    assert!(app.handoff.text.contains("sizes!\nyx"), "{}", app.handoff.text);
    key(&mut app, KeyCode::Delete);
    assert!(app.handoff.text.contains("sizes!yx"), "the line break was deleted: {}", app.handoff.text);
    // Pasting goes in at the cursor too.
    app.on_paste(" pasted");
    assert!(app.handoff.text.contains("sizes! pastedyx"), "{}", app.handoff.text);
    // Backspace only ever takes away what is in front of the cursor; what is after it stays.
    let after_cursor: String = app.handoff.text.chars().skip(app.handoff.cursor).collect();
    for _ in 0..(app.handoff.text.chars().count() + 5) {
        key(&mut app, KeyCode::Backspace);
    }
    assert_eq!((app.handoff.text.clone(), app.handoff.cursor), (after_cursor, 0));
    // And moving about in an almost empty box does nothing silly.
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Home);
    assert_eq!(app.handoff.cursor, 0);
    key(&mut app, KeyCode::End);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Home);
    key(&mut app, KeyCode::Delete);
    key(&mut app, KeyCode::Delete);
    key(&mut app, KeyCode::Delete);
    assert_eq!((app.handoff.text.as_str(), app.handoff.cursor), ("", 0));
    assert_eq!(fake.calls(), 1, "editing asks Claude nothing");
}

#[test]
fn esc_calls_the_whole_thing_off_and_changes_nothing() {
    let (w, fake) = world("handoff-esc");
    fake.reply_default(BRIEF);
    let mut app = app_of(&w, &fake);
    select(&mut app, "sess-1");
    key(&mut app, KeyCode::Char('h'));
    wait_ready(&mut app);
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List));
    assert!(app.active_notice().is_none(), "nothing to say: nothing happened");
    // The conversation is where it was, in the project it was in, and no session was started.
    assert_eq!(Projects::open(&w.config).members_of(&project_id(&w)), ["sess-1"]);
    assert!(!app.sessions.iter().any(|s| s.archived));
    assert_eq!(app.sessions.len(), 1);
    assert_eq!(fake.calls(), 1, "and nothing else was asked");

    // Esc while the call is still out calls it off too.
    fake.hang(true);
    key(&mut app, KeyCode::Char('h'));
    assert_eq!(app.handoff.state, HandoffState::Asking);
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List));
    app.refresh_until_for_test(10, "the cancelled call to end", |a| !a.handoff_busy());
    assert!(fake.pids().iter().all(|&p| !crate::testkit::alive(p)), "the call's process is gone");
}

#[test]
fn an_answer_that_is_not_understood_or_a_call_that_fails_says_so_and_starts_nothing() {
    let (w, fake) = world("handoff-bad");
    fake.reply_default("I'm afraid I can't help with that.");
    let mut app = app_of(&w, &fake);
    select(&mut app, "sess-1");
    key(&mut app, KeyCode::Char('h'));
    wait_ready(&mut app);
    match &app.handoff.state {
        HandoffState::Failed(why) => assert!(why.contains("couldn't be understood") && why.contains("Nothing has changed"), "{why}"),
        other => panic!("{other:?}"),
    }
    // Nothing may be accepted from here.
    ctrl(&mut app, 's');
    assert!(matches!(app.mode, Mode::Handoff), "ctrl+s does nothing without a brief");
    key(&mut app, KeyCode::Esc);

    // A call that fails outright says why in the same place.
    fake.fail(Some("Claude is over capacity"));
    key(&mut app, KeyCode::Char('h'));
    wait_ready(&mut app);
    match &app.handoff.state {
        HandoffState::Failed(why) => assert!(why.contains("Nothing has changed"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(app.sessions.len(), 1, "no session was started either way");
}

#[test]
fn hand_off_needs_a_project_a_live_project_and_the_gate_and_says_plainly_which_is_missing() {
    let (w, fake) = world("handoff-refused");
    fake.reply_default(BRIEF);
    // A second conversation, in no project.
    w.transcript("loose-2", &other(&w), "Something else entirely", 4);
    let mut app = app_of(&w, &fake);

    select(&mut app, "loose-2");
    key(&mut app, KeyCode::Char('h'));
    assert!(matches!(app.mode, Mode::List));
    let said = app.active_notice().unwrap_or_default().to_string();
    assert!(said.contains("is in no project") && said.contains("Press m"), "{said}");

    // The master switch off: nothing is asked, and it says how to turn it on.
    let mut s = settings::load(&w.config).0;
    s.tokens_enabled = false;
    settings::save(&w.config, &s).unwrap();
    app.refresh_with(Vec::new(), false);
    select(&mut app, "sess-1");
    key(&mut app, KeyCode::Char('h'));
    assert!(matches!(app.mode, Mode::List), "the box never opens");
    let said = app.active_notice().unwrap_or_default().to_string();
    assert!(said.starts_with("No hand-off:") && said.contains("Smart features"), "{said}");
    assert_eq!(fake.calls(), 0, "nothing was asked at all");

    // Today's tokens spent: the same, in its own words.
    s.tokens_enabled = true;
    settings::save(&w.config, &s).unwrap();
    let spent = crate::ledger::Usage { input: Settings::default().clamped().ai_daily_tokens + 1, ..Default::default() };
    Ledger::open(&w.cache).record_call(usage::now(), "names", &spent, 0.0, true).unwrap();
    app.refresh_with(Vec::new(), false);
    select(&mut app, "sess-1");
    key(&mut app, KeyCode::Char('h'));
    let said = app.active_notice().unwrap_or_default().to_string();
    assert!(said.contains("limit is used up"), "{said}");
    assert_eq!(fake.calls(), 0);

    // An archived project is refused before anything is read.
    let _ = fs::remove_file(Ledger::open(&w.cache).path());
    let id = project_id(&w);
    Projects::open(&w.config).archive(&id, &[]).unwrap();
    app.refresh_with(Vec::new(), false);
    select(&mut app, "sess-1");
    key(&mut app, KeyCode::Char('h'));
    let said = app.active_notice().unwrap_or_default().to_string();
    assert!(said.contains("is archived"), "{said}");
    assert_eq!(fake.calls(), 0);
}

#[test]
fn a_brief_that_could_not_be_handed_on_is_refused_with_the_box_left_as_it_was() {
    let (w, fake) = world("handoff-whynot");
    fake.reply_default(BRIEF);
    let mut app = app_of(&w, &fake);
    select(&mut app, "sess-1");
    key(&mut app, KeyCode::Char('h'));
    wait_ready(&mut app);
    // Emptied: there is nothing to start a session with.
    let was = app.handoff.text.chars().count();
    for _ in 0..was {
        key(&mut app, KeyCode::Backspace);
    }
    ctrl(&mut app, 's');
    assert!(matches!(app.mode, Mode::Handoff), "the box stays open");
    assert!(app.active_notice().unwrap_or_default().contains("The brief is empty"), "{:?}", app.active_notice());
    // Starting with a dash would be read as an option by claude, so it is refused with the text untouched.
    for c in "--resume trouble".chars() {
        key(&mut app, KeyCode::Char(c));
    }
    ctrl(&mut app, 's');
    assert!(matches!(app.mode, Mode::Handoff));
    assert_eq!(app.handoff.text, "--resume trouble", "the text is left exactly as it was");
    assert!(app.active_notice().unwrap_or_default().contains("start with a dash"), "{:?}", app.active_notice());
    assert_eq!(app.sessions.len(), 1, "and no session was started");
}

// ---- a session joining a project tells the notes -------------------------------------------------------------

/// The mark the notes helper waits on for this project, if there is one.
fn due(w: &World, id: &str) -> Option<u64> {
    crate::notesrun::due_at(&w.cache, id)
}

#[test]
fn putting_a_session_in_a_project_with_m_marks_it_so_its_notes_are_updated() {
    let (w, fake) = world("joined-m");
    w.transcript("loose-2", &other(&w), "Something else entirely", 4);
    let id = project_id(&w);
    let mut app = app_of(&w, &fake);
    let _ = fs::remove_file(crate::notesrun::due_path(&w.cache, &id));
    assert_eq!(due(&w, &id), None);

    select(&mut app, "loose-2");
    key(&mut app, KeyCode::Char('m'));
    assert!(matches!(app.mode, Mode::Assign));
    key(&mut app, KeyCode::Enter); // the only project there is
    assert_eq!(Projects::open(&w.config).project_id_of("loose-2"), Some(id.as_str()));
    assert!(due(&w, &id).is_some(), "the project is marked, so an update runs without anyone resuming anything");

    // Taking it out again marks nothing new.
    let was = due(&w, &id);
    select(&mut app, "loose-2");
    key(&mut app, KeyCode::Char('m'));
    key(&mut app, KeyCode::Down); // past the one project, to "New project…"
    key(&mut app, KeyCode::Down); // and on to "No project"
    key(&mut app, KeyCode::Enter);
    assert_eq!(Projects::open(&w.config).project_id_of("loose-2"), None);
    assert_eq!(due(&w, &id), was, "leaving a project is not something to update from");
}

#[test]
fn a_session_started_under_a_project_marks_it_as_soon_as_it_is_placed() {
    let (mut w, fake) = world("joined-launch");
    let id = project_id(&w);
    let mut app = app_of(&w, &fake);
    let _ = fs::remove_file(crate::notesrun::due_path(&w.cache, &id));

    // A session platoon started under the project: its pid is noted, and its id shows up a moment later.
    let pid = w.live("new-3", &shop(&w), crate::testkit::now_ms(), "idle");
    crate::projects::note_launch(&w.cache, pid, &id, crate::testkit::now_ms() - 5_000).unwrap();
    app.refresh_with(Vec::new(), false);
    assert_eq!(Projects::open(&w.config).project_id_of("new-3"), Some(id.as_str()), "placed by the launch");
    assert!(due(&w, &id).is_some(), "and the project was told there is something new");

    // A refresh that places nothing marks nothing new.
    let was = due(&w, &id);
    app.refresh_with(Vec::new(), false);
    assert_eq!(due(&w, &id), was);
}

#[test]
fn the_session_start_hook_marks_the_project_it_places_a_session_into() {
    let (w, _fake) = world("joined-hook");
    let id = project_id(&w);
    let _ = fs::remove_file(crate::notesrun::due_path(&w.cache, &id));
    let input = serde_json::json!({"session_id": "brand-new", "source": "startup", "cwd": "x", "hook_event_name": "SessionStart"}).to_string();
    let out = crate::sessionhook::session_start(&input, Some(&id), &w.config, &w.cache);
    assert!(out.is_some_and(|o| o.contains("Sell socks.")), "the notes still go to the session");
    assert_eq!(Projects::open(&w.config).project_id_of("brand-new"), Some(id.as_str()));
    assert!(due(&w, &id).is_some(), "and the project is marked");

    // A session that was already in the project (a plain resume) places nothing and marks nothing new.
    let was = due(&w, &id);
    let input = serde_json::json!({"session_id": "sess-1", "source": "resume", "cwd": "x", "hook_event_name": "SessionStart"}).to_string();
    crate::sessionhook::session_start(&input, Some(&id), &w.config, &w.cache);
    assert_eq!(due(&w, &id), was);
}
