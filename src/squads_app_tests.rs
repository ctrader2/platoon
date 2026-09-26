//! Squads at the level of the app: squads' leader sessions left out of every list of the user's own sessions, the
//! `a` refusals and the squad picker, the state list of a ticket with a squad, `p`, the ticket's screen while its
//! squad's session isn't running, and the split view's layout. Every directory is a temp dir (`World`); nothing here
//! starts a session (the terminal tests in `tests/squads.rs` do, with a stand-in `claude`).

use crate::app::{split_areas, App, Mode, Pane, PickOpt};
use crate::testkit::World;
use crate::tickets::{Leader, NewTicket, State, Tickets};
use crate::ui;
use ratatui::{
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::Rect,
    Terminal,
};

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn typed(app: &mut App, text: &str) {
    for c in text.chars() {
        key(app, KeyCode::Char(c));
    }
}

fn draw(term: &mut Terminal<TestBackend>, app: &App) -> String {
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

fn flat(text: &str) -> String {
    text.replace(['│', '╭', '╮', '╰', '╯', '─'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn lead(session: &str) -> Leader {
    Leader { squad: "coding".into(), session: session.into(), started: 1 }
}

/// A project "Web shop" with the user's session `user-1` in it, its screen open.
fn on_the_project(name: &str) -> (World, App, String) {
    let w = World::new(name);
    w.transcript("user-1", "/tmp", "make a basket page", 10);
    let mut app = w.app();
    let id = app.projects.create("Web shop").unwrap();
    app.projects.assign("user-1", Some(&id)).unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Enter);
    assert!(app.open_project().is_some());
    (w, app, id)
}

fn ids(app: &App) -> Vec<String> {
    app.visible().iter().map(|&i| app.sessions[i].id.clone()).collect()
}

#[test]
fn leader_sessions_are_left_out_of_every_list_counts_and_search() {
    let w = World::new("sq-hidden");
    w.transcript("user-1", "/tmp", "make a basket page", 10);
    w.transcript("lead-1", "/tmp", "Ticket #1: basket page for the squad", 5);
    // It is even running right now (this process stands in for it).
    w.register(std::process::id(), "lead-1", "/tmp", 1, "busy");
    let mut app = w.app();
    let id = app.projects.create("Web shop").unwrap();
    app.projects.assign("user-1", Some(&id)).unwrap();
    // As if something had put the leader in the project anyway: it must still not show there.
    app.projects.assign("lead-1", Some(&id)).unwrap();
    app.reload_for_test();
    assert_eq!(ids(&app).len(), 2, "before it is anyone's leader, it is an ordinary session");

    let mut t = Tickets::open(&w.config, &id);
    let n = t.add(NewTicket { title: "Basket".into(), folders: vec!["/tmp".into()], ..Default::default() }).unwrap();
    t.start_leader(n, &lead("lead-1"), &[]).unwrap();
    app.refresh_for_test();
    assert!(app.leader_ids().contains("lead-1"));
    // The Sessions tab, its counts, the unread count, the whole list.
    assert_eq!(ids(&app), ["user-1"]);
    assert_eq!(app.counts, [1, 0]);
    assert!(app.sessions.iter().all(|s| s.id != "lead-1"));
    // Full-text search.
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Tab);
    typed(&mut app, "basket");
    let found: Vec<String> = app.search.results.iter().map(|r| r.id.clone()).collect();
    assert_eq!(found, ["user-1"]);
    key(&mut app, KeyCode::Esc);
    // The project's own list and its counts.
    key(&mut app, KeyCode::Char('2'));
    let st = app.pstats.get(&id).cloned().unwrap();
    assert_eq!((st.active, st.archived), (1, 0), "the project counts only the user's session");
    key(&mut app, KeyCode::Enter);
    assert_eq!(ids(&app), ["user-1"]);
    // Nothing in the list can be Enter'd into a "Take over?" of it.
    assert!(app.selected_session().is_some_and(|s| s.id == "user-1"));
}

#[test]
fn the_state_list_offers_working_only_on_a_ticket_with_a_squad() {
    let (w, mut app, id) = on_the_project("sq-states");
    let mut t = Tickets::open(&w.config, &id);
    let plain = t.add(NewTicket { title: "Plain".into(), ..Default::default() }).unwrap();
    let with = t.add(NewTicket { title: "With a squad".into(), folders: vec!["/tmp".into()], ..Default::default() }).unwrap();
    t.start_leader(with, &lead("lead-9"), &[]).unwrap();
    t.set_state(with, &State::Done).unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.pane, Pane::Tickets);
    let states = |app: &App| app.ticket_pick_options().into_iter().map(|(o, _)| o).collect::<Vec<_>>();
    key(&mut app, KeyCode::Char('s'));
    assert_eq!(app.tk.pick.number, plain);
    assert_eq!(states(&app), [PickOpt::State(State::New), PickOpt::State(State::Done), PickOpt::State(State::Cancelled)]);
    key(&mut app, KeyCode::Esc);
    // The Done one is under the folded divider: unfold, then pick it.
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Char('s'));
    assert_eq!(app.tk.pick.number, with);
    assert_eq!(states(&app), [PickOpt::State(State::New), PickOpt::State(State::Working), PickOpt::State(State::Done), PickOpt::State(State::Cancelled)]);
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    assert!(flat(&draw(&mut term, &app)).contains("Working · continue with the squad"));
    // Back to Working: the same leader, nothing started by the list itself.
    assert_eq!(app.tk.pick.selected, 2, "the list starts on what it is now");
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Enter);
    let t = app.ticket(with).unwrap();
    assert_eq!((&t.state, t.leaders.len()), (&State::Working, 1));
    assert!(matches!(app.mode, Mode::List), "its session isn't running, so there is nothing to ask");
}

#[test]
fn a_hands_only_a_new_ready_ticket_with_folders_and_says_why_not_otherwise() {
    let (w, mut app, id) = on_the_project("sq-refuse");
    let mut t = Tickets::open(&w.config, &id);
    let first = t.add(NewTicket { title: "First".into(), folders: vec!["/tmp".into()], ..Default::default() }).unwrap();
    let waits = t.add(NewTicket { title: "Second".into(), folders: vec!["/tmp".into()], blocked_by: vec![first], ..Default::default() }).unwrap();
    let nofolder = t.add(NewTicket { title: "Third".into(), ..Default::default() }).unwrap();
    let working = t.add(NewTicket { title: "Fourth".into(), folders: vec!["/tmp".into()], ..Default::default() }).unwrap();
    t.start_leader(working, &lead("lead-4"), &[]).unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Tab);
    let pick = |app: &mut App, n: u32| {
        let at = app.ticket_rows().iter().position(|r| *r == crate::app::TRow::Ticket(n)).unwrap();
        app.tk.selected = Some(app.ticket_rows()[at]);
    };
    pick(&mut app, waits);
    key(&mut app, KeyCode::Char('a'));
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.active_notice(), Some("#2 waits for #1, which isn't Done yet."));
    pick(&mut app, nofolder);
    key(&mut app, KeyCode::Char('a'));
    assert_eq!(app.active_notice(), Some("#3 has no folder yet. Press f to give it one."));
    pick(&mut app, working);
    key(&mut app, KeyCode::Char('a'));
    assert_eq!(app.active_notice(), Some("#4 is Working. Only a New ticket can be handed to a squad."));
    // A New one, ready, with a folder: the picker, with what the squad can do.
    pick(&mut app, first);
    key(&mut app, KeyCode::Char('a'));
    assert!(matches!(app.mode, Mode::SquadPick) && app.sq.pick_ticket == first);
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("hand to a squad") && shown.contains("#1 First") && shown.contains("Coding") && shown.contains("Makes a change to code"), "{shown}");
    assert!(shown.contains("enter hand it over"), "{shown}");
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.ticket(first).unwrap().state, State::New, "nothing happened");
    assert!(!w.config.join("projects").join(&id).join("tickets.json").exists() || app.ticket(first).unwrap().leaders.is_empty());
}

#[test]
fn p_lets_a_ready_ticket_publish_and_nothing_else() {
    let (w, mut app, id) = on_the_project("sq-publish");
    let mut t = Tickets::open(&w.config, &id);
    let n = t.add(NewTicket { title: "Basket".into(), folders: vec!["/tmp".into()], ..Default::default() }).unwrap();
    t.start_leader(n, &lead("lead-5"), &[]).unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Char('p'));
    assert_eq!(app.active_notice(), Some("#1 isn't waiting to publish anything."));
    assert!(!app.ticket(n).unwrap().go_ahead);
    Tickets::open(&w.config, &id).squad_says(n, &State::Ready, "Wants to publish: push ticket/1", None).unwrap();
    app.reload_for_test();
    app.notice = None;
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    assert!(flat(&draw(&mut term, &app)).contains("p let it publish"));
    key(&mut app, KeyCode::Char('p'));
    let t = app.ticket(n).unwrap();
    assert!(t.go_ahead && t.state == State::Working && t.reason.is_empty(), "{t:?}");
    assert!(app.active_notice().unwrap().starts_with("#1 may publish now. Its squad's session isn't running"), "{:?}", app.active_notice());
}

#[test]
fn a_ticket_whose_squad_isnt_running_shows_how_to_continue_it_and_what_the_squad_said() {
    let (w, mut app, id) = on_the_project("sq-screen");
    let mut t = Tickets::open(&w.config, &id);
    let n = t.add(NewTicket { title: "Basket".into(), folders: vec!["/tmp".into()], ..Default::default() }).unwrap();
    let wt = crate::tickets::Worktree { folder: "/tmp".into(), path: "/w/shop-worktrees/ticket-1".into(), branch: "ticket/1".into() };
    t.start_leader(n, &lead("lead-6"), &[wt]).unwrap();
    t.squad_says(n, &State::Waiting, "Asks: Red or blue?", None).unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.tk.open, Some(n), "the ticket's screen, not a session started by itself");
    assert!(matches!(app.mode, Mode::List));
    let mut term = Terminal::new(TestBackend::new(130, 34)).unwrap();
    let shown = draw(&mut term, &app);
    let f = flat(&shown);
    assert!(f.contains("The squad's session isn't running. Enter to continue it."), "{shown}");
    assert!(f.contains("Squad says Asks: Red or blue?"), "{shown}");
    assert!(f.contains("Squad Coding · its session isn't running"), "{shown}");
    assert!(f.contains("Worktree …/shop-worktrees/ticket-1 (ticket/1)"), "cut from the start when narrow: {shown}");
    assert!(f.contains("enter continue the squad"), "{shown}");
    // The message is on the right, the ticket on the left.
    let row_of = |needle: &str| shown.lines().find_map(|l| l.find(needle).map(|b| l[..b].chars().count())).unwrap();
    assert!(row_of("The squad's session") > row_of("#1 Basket") + 30, "{shown}");
    // Done, with a result: the result comes first in the ticket's text.
    Tickets::open(&w.config, &id).squad_says(n, &State::Done, "", Some("Added the basket page on ticket/1.")).unwrap();
    app.reload_for_test();
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("Result") && f.contains("Added the basket page on ticket/1."), "{f}");
    // Narrow: the message above the ticket.
    let mut narrow = Terminal::new(TestBackend::new(80, 30)).unwrap();
    let shown = draw(&mut narrow, &app);
    let lines: Vec<&str> = shown.lines().collect();
    let msg = lines.iter().position(|l| l.contains("isn't running. Enter to continue it.")).expect(&shown);
    let title = lines.iter().position(|l| l.contains("#1 Basket")).expect(&shown);
    assert!(msg < title, "{shown}");
}

#[test]
fn cancelling_a_ticket_whose_squad_isnt_running_asks_nothing() {
    let (w, mut app, id) = on_the_project("sq-cancel");
    let mut t = Tickets::open(&w.config, &id);
    let n = t.add(NewTicket { title: "Basket".into(), folders: vec!["/tmp".into()], ..Default::default() }).unwrap();
    t.start_leader(n, &lead("lead-7"), &[]).unwrap();
    t.give_go_ahead(n).unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Char('s'));
    let at = app.ticket_pick_options().iter().position(|(o, _)| *o == PickOpt::State(State::Cancelled)).unwrap();
    app.tk.pick.selected = at;
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));
    let t = app.ticket(n).unwrap();
    assert_eq!(t.state, State::Cancelled);
    assert!(!t.go_ahead, "leaving Working clears the go-ahead");
}

#[test]
fn the_split_puts_the_ticket_on_the_left_and_the_session_on_the_right_or_one_line_above_it_when_narrow() {
    let a = split_areas(120, 40);
    assert!(a.wide);
    assert_eq!(a.ticket.y, 1);
    assert!(a.ticket.width >= 40 && a.ticket.width <= 50, "{a:?}");
    assert!(a.rule_x >= a.ticket.right() && a.term.x == a.rule_x + 1, "{a:?}");
    assert_eq!(a.term.right(), 119, "to the frame");
    assert_eq!((a.term.y, a.term.bottom(), a.bar.y), (1, 38, 38), "{a:?}");
    let n = split_areas(99, 30);
    assert!(!n.wide);
    assert_eq!(n.ticket, Rect::new(1, 1, 97, 1));
    assert_eq!(n.term, Rect::new(1, 2, 97, 26));
    assert_eq!(n.bar, Rect::new(1, 28, 97, 1));
    assert_eq!(crate::app::SPLIT_MIN_WIDTH, 100);
}
