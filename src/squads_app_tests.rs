//! Squads at the level of the app: squads' leader sessions left out of every list of the user's own sessions, the
//! `S` refusals and the squad picker, the state list of a ticket with a squad, `p`, the ticket's screen while its
//! squad's session isn't running, and the split view's layout. Every directory is a temp dir (`World`); nothing here
//! starts a session (the terminal tests in `tests/squads.rs` do, with a stand-in `claude`).

use crate::app::{App, Confirm, Mode, Pane, split_areas};
use crate::testkit::World;
use crate::tickets::{Leader, NewTicket, State, Tickets};
use crate::ui;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::Rect,
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
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn flat(text: &str) -> String {
    text.replace(['│', '╭', '╮', '╰', '╯', '─'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn lead(session: &str) -> Leader {
    Leader { squad: "coding".into(), session: session.into(), started: 1, names: Default::default(), snapshot: None }
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
fn a_ticket_with_a_squad_is_only_cancelled_by_hand_and_s_does_nothing() {
    let (w, mut app, id) = on_the_project("sq-states");
    let mut t = Tickets::open(&w.config, &id);
    let with = t.add(NewTicket { title: "With a squad".into(), folders: vec!["/tmp".into()], ..Default::default() }).unwrap();
    t.start_leader(with, &lead("lead-9"), &[]).unwrap();
    t.squad_says(with, &State::Waiting, "Asks: Shall I close this ticket?", None).unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.pane, Pane::Tickets);
    let mut term = Terminal::new(TestBackend::new(140, 30)).unwrap();
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("c cancel") && !shown.contains("s state"), "{shown}");
    // `s`: nothing (no list, no settings, nothing written).
    let path = Tickets::open(&w.config, &id).path();
    let before = std::fs::read_to_string(&path).unwrap();
    key(&mut app, KeyCode::Char('s'));
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    // On its own screen too: no `s` in the hints, `c` there.
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.tk.open, Some(with));
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("c cancel") && !shown.contains("s state") && shown.contains("enter continue the squad"), "{shown}");
    key(&mut app, KeyCode::Char('s'));
    assert!(matches!(app.mode, Mode::List) && app.tk.open == Some(with));
    // `c`: its session isn't running, so the question says only that talking to the squad opens it again.
    key(&mut app, KeyCode::Char('c'));
    assert!(matches!(app.mode, Mode::Confirm(Confirm::CancelTicket { squad: true, pid: None, .. })));
    let shown = flat(&draw(&mut term, &app));
    // (The question's lines share their rows with the screen behind it, so its words are checked in pieces.)
    assert!(
        shown.contains("Cancel ticket #1?") && shown.contains("Talking to its squad") && shown.contains("again opens it again."),
        "{shown}"
    );
    key(&mut app, KeyCode::Char('y'));
    let t = Tickets::open(&w.config, &id).get(with).cloned().unwrap();
    assert_eq!((&t.state, t.leaders.len(), t.reason.as_str()), (&State::Cancelled, 1, ""), "the same leader, what it said gone");
    assert!(app.sq.launched.is_empty(), "nothing started");
}

#[test]
fn s_hands_only_a_new_ready_ticket_with_folders_and_says_why_not_otherwise() {
    let (w, mut app, id) = on_the_project("sq-refuse");
    let mut t = Tickets::open(&w.config, &id);
    let first = t.add(NewTicket { title: "First".into(), folders: vec!["/tmp".into()], ..Default::default() }).unwrap();
    let waits =
        t.add(NewTicket { title: "Second".into(), folders: vec!["/tmp".into()], blocked_by: vec![first], ..Default::default() }).unwrap();
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
    key(&mut app, KeyCode::Char('S'));
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.active_notice(), Some("#2 waits for #1, which isn't Done yet."));
    // No folder: the picker opens, and every squad may take it (it works in the ticket's work folder).
    pick(&mut app, nofolder);
    key(&mut app, KeyCode::Char('S'));
    assert!(matches!(app.mode, Mode::SquadPick) && app.sq.pick_ticket == nofolder);
    let mut term = Terminal::new(TestBackend::new(120, 50)).unwrap();
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("▎ Coding") && shown.contains("Makes a change to code") && shown.contains("Finds things out"), "{shown}");
    assert!(!shown.contains("Needs a folder"), "no squad needs a folder any more: {shown}");
    key(&mut app, KeyCode::Esc);
    pick(&mut app, working);
    key(&mut app, KeyCode::Char('S'));
    assert_eq!(app.active_notice(), Some("#4 is Working. Only a New ticket can be handed to a squad."));
    // A New one, ready, with a folder: the picker, with what the squad can do.
    pick(&mut app, first);
    key(&mut app, KeyCode::Char('S'));
    assert!(matches!(app.mode, Mode::SquadPick) && app.sq.pick_ticket == first);
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    let shown = flat(&draw(&mut term, &app));
    assert!(
        shown.contains("hand to a squad")
            && shown.contains("#1 First")
            && shown.contains("Coding")
            && shown.contains("Makes a change to code"),
        "{shown}"
    );
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
    // Its screen says the squad let it publish, beside the squad's name.
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.tk.open, Some(n));
    let shown = draw(&mut term, &app);
    assert!(shown.lines().any(|l| l.contains("Coding squad · you let it publish")), "{shown}");
}

#[test]
fn a_ticket_whose_squad_isnt_running_shows_how_to_continue_it_and_what_the_squad_said() {
    let (w, mut app, id) = on_the_project("sq-screen");
    let mut t = Tickets::open(&w.config, &id);
    let n = t.add(NewTicket { title: "Basket".into(), folders: vec!["/tmp".into()], ..Default::default() }).unwrap();
    let wt = crate::tickets::Worktree {
        folder: "/tmp".into(),
        path: "/w/shop-worktrees/ticket-1".into(),
        branch: "ticket/1".into(),
        ..Default::default()
    };
    t.start_leader(n, &lead("lead-6"), &[wt]).unwrap();
    t.squad_says(n, &State::Waiting, "Asks: Red or blue?", None).unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.tk.open, Some(n), "the ticket's screen, not a session started by itself");
    assert!(matches!(app.mode, Mode::List));
    let mut term = Terminal::new(TestBackend::new(126, 34)).unwrap();
    let shown = draw(&mut term, &app);
    let f = flat(&shown);
    assert!(f.contains("The squad's session isn't running. Enter to continue it."), "{shown}");
    assert!(f.contains("Squad says Asks: Red or blue?"), "{shown}");
    // The squad by its name, and nothing more: whether its session runs is on the right.
    let squad_line = shown.lines().find(|l| l.contains("Coding squad")).expect(&shown);
    assert!(squad_line.split('│').next().unwrap().trim() == "Coding squad", "just \"Coding squad\": {squad_line}");
    assert!(!f.contains("Squad Coding") && !f.contains("its session isn't running") && !f.contains("let it publish"), "{shown}");
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
fn cancelling_a_ticket_whose_squad_isnt_running_asks_and_clears_the_go_ahead() {
    let (w, mut app, id) = on_the_project("sq-cancel");
    let mut t = Tickets::open(&w.config, &id);
    let n = t.add(NewTicket { title: "Basket".into(), folders: vec!["/tmp".into()], ..Default::default() }).unwrap();
    t.start_leader(n, &lead("lead-7"), &[]).unwrap();
    t.give_go_ahead(n).unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Char('c'));
    assert!(matches!(app.mode, Mode::Confirm(Confirm::CancelTicket { number: 1, pid: None, .. })));
    // Esc is no, too.
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.ticket(n).unwrap().state, State::Working);
    key(&mut app, KeyCode::Char('c'));
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));
    let t = app.ticket(n).unwrap();
    assert_eq!(t.state, State::Cancelled);
    assert!(!t.go_ahead, "leaving Working clears the go-ahead");
}

#[test]
fn cancelling_a_ticket_whose_squad_runs_says_so_and_yes_stops_only_that_session() {
    let (w, mut app, id) = on_the_project("sq-cancel-run");
    let mut t = Tickets::open(&w.config, &id);
    let n = t.add(NewTicket { title: "Basket".into(), folders: vec!["/tmp".into()], ..Default::default() }).unwrap();
    t.start_leader(n, &lead("lead-8"), &[]).unwrap();
    app.reload_for_test();
    // Its leader "runs" in a host: a process of this test's own stands in for its claude; another one must be left alone.
    let mut leader = std::process::Command::new("sleep").arg("30").spawn().unwrap();
    let mut bystander = std::process::Command::new("sleep").arg("30").spawn().unwrap();
    app.hosts.push(crate::host::HostInfo {
        claude_pid: leader.id(),
        host_pid: 0,
        dir: "/tmp".into(),
        args: vec!["--resume".into(), "lead-8".into()],
        sock: Default::default(),
    });
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Char('c'));
    let pid = leader.id();
    assert!(matches!(app.mode, Mode::Confirm(Confirm::CancelTicket { number: 1, pid: Some(p), .. }) if p == pid));
    let mut term = Terminal::new(TestBackend::new(140, 30)).unwrap();
    let shown = flat(&draw(&mut term, &app));
    // (The question's lines share their rows with the screen behind it, so its words are checked in pieces.)
    assert!(
        shown.contains("Cancel ticket #1?")
            && shown.contains("its squad's session,")
            && shown.contains("which is running, will be stopped"),
        "{shown}"
    );
    key(&mut app, KeyCode::Char('y'));
    assert_eq!(app.ticket(n).unwrap().state, State::Cancelled);
    assert_eq!(app.active_notice(), Some("#1 is Cancelled. Stopping its squad's session; its conversation is kept."));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while leader.try_wait().unwrap().is_none() {
        assert!(std::time::Instant::now() < deadline, "the leader's stand-in was not stopped");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(bystander.try_wait().unwrap().is_none(), "nothing else is signalled");
    let _ = bystander.kill();
    let _ = bystander.wait();
}

#[test]
fn the_split_puts_the_ticket_on_the_left_and_the_session_on_the_right_or_one_line_above_it_when_narrow() {
    let a = split_areas(120, 40);
    assert!(a.wide);
    assert_eq!(a.ticket.y, 0);
    assert!(a.ticket.width >= 40 && a.ticket.width <= 50, "{a:?}");
    assert!(a.rule_x >= a.ticket.right() && a.term.x == a.rule_x + 1, "{a:?}");
    // No frame around the app: the session reaches the right edge and every row but its status line.
    assert_eq!(a.term.right(), 120, "to the right edge");
    assert_eq!((a.term.y, a.term.bottom(), a.bar.y), (0, 39, 39), "{a:?}");
    let n = split_areas(99, 30);
    assert!(!n.wide);
    assert_eq!(n.ticket, Rect::new(1, 0, 97, 1));
    assert_eq!(n.term, Rect::new(0, 1, 99, 28));
    assert_eq!(n.bar, Rect::new(0, 29, 99, 1));
    assert_eq!(crate::app::SPLIT_MIN_WIDTH, 100);
}

// ---- the commander, the worktrees and the leader: off the UI thread ------------------------------------------------

/// A project "Web shop" with ticket #1 "Basket" in `folder`, its Tickets pane focused, the smart features on (with
/// `spent` tokens already used today, of the default 500,000) and calls going to a fake `claude`.
fn with_commander(name: &str, spent: u64, folder: &std::path::Path) -> (World, App, String, crate::testkit::AiFake) {
    let w = World::new(name);
    let mut s = crate::settings::Settings { tokens_enabled: true, tokens_asked: true, ..Default::default() };
    s.auto_archive_days = 0;
    crate::settings::save(&w.config, &s).unwrap();
    if spent > 0 {
        let used = crate::ledger::Usage { input: spent, ..Default::default() };
        crate::ledger::Ledger::open(&w.cache).record_call(crate::usage::now(), "names", &used, 0.0, true).unwrap();
    }
    w.transcript("user-1", "/tmp", "make a basket page", 10);
    let mut app = w.app();
    let fake = crate::testkit::AiFake::new(name);
    app.set_ai_program_for_test(&fake.script);
    let id = app.projects.create("Web shop").unwrap();
    app.projects.assign("user-1", Some(&id)).unwrap();
    std::fs::create_dir_all(folder).unwrap();
    Tickets::open(&w.config, &id)
        .add(NewTicket { title: "Basket".into(), folders: vec![folder.to_string_lossy().into_owned()], ..Default::default() })
        .unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.pane, Pane::Tickets);
    (w, app, id, fake)
}

fn hand(app: &mut App) {
    key(app, KeyCode::Char('S'));
    assert!(matches!(app.mode, Mode::SquadPick), "{:?}", app.active_notice());
    key(app, KeyCode::Enter);
}

#[test]
fn the_commander_checks_off_the_ui_thread_and_a_yes_starts_the_leader() {
    let base = std::env::temp_dir().join(format!("platoon-sq-yes-{}", std::process::id()));
    let (w, mut app, id, fake) = with_commander("sq-yes", 0, &base.join("docs"));
    fake.slow(0.6);
    fake.reply_default(r#"{"fits":true,"reason":"Coding work."}"#);
    hand(&mut app);
    // Straight back to the screen: the ticket shows Working, "Commander is checking…", and nothing is on disk yet.
    assert!(matches!(app.mode, Mode::List));
    let t = app.ticket(1).unwrap();
    assert_eq!((t.state.clone(), t.busy.as_deref()), (State::Working, Some("Commander is checking…")));
    assert_eq!(Tickets::open(&w.config, &id).get(1).unwrap().state, State::New, "the file says New until a leader starts");
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("#1 Basket") && f.contains("Commander is checking…"), "{f}");
    // On its own screen: Working, and what is happening.
    key(&mut app, KeyCode::Enter);
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("State Working · Commander is checking…"), "{f}");
    assert!(app.squads_busy());
    // What would change it waits meanwhile.
    key(&mut app, KeyCode::Char('c'));
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.active_notice(), Some("#1: Commander is checking… Wait a moment."));
    app.refresh_until_for_test(20, "the leader to start", |a| !a.sq.launched.is_empty());
    assert_eq!(app.sq.launched, [1]);
    let t = Tickets::open(&w.config, &id).get(1).cloned().unwrap();
    assert_eq!((t.state, t.leaders.len(), t.handover), (State::Working, 1, None));
    assert!(!app.squads_busy() && app.ticket(1).unwrap().busy.is_none());
    // One call, on Haiku, as the user asked for it, with the ticket and the squad in it.
    assert_eq!(fake.calls(), 1);
    let argv = fake.argv(1);
    assert_eq!(argv[argv.iter().position(|a| a == "--model").unwrap() + 1], "haiku");
    assert!(fake.stdin(1).starts_with("The squad: Coding\n") && fake.stdin(1).contains("The ticket: #1 Basket"), "{}", fake.stdin(1));
    assert!(fake.has_arg(1, &crate::squads::commander_system(crate::squads::COMMANDER_GUIDANCE)));
    let day = crate::ledger::Ledger::open(app.cache_for_test()).load();
    let today = day.days.values().next().unwrap();
    assert_eq!(today.features.get(crate::squads::COMMANDER_FEATURE).map(|f| f.calls), Some(1), "its tokens are counted");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_no_puts_it_back_to_new_with_the_reason_and_the_same_squad_again_asks_to_hand_it_anyway() {
    let base = std::env::temp_dir().join(format!("platoon-sq-no-{}", std::process::id()));
    let (w, mut app, id, fake) = with_commander("sq-no", 0, &base.join("docs"));
    fake.reply_default(r#"{"fits":false,"reason":"This is docs work; the Coding squad writes code."}"#);
    hand(&mut app);
    app.refresh_until_for_test(20, "the commander's answer", |a| !a.squads_busy());
    let t = app.ticket(1).cloned().unwrap();
    assert_eq!(t.state, State::New);
    assert_eq!(t.handover.as_ref().map(|h| h.says.as_str()), Some("This is docs work; the Coding squad writes code."));
    assert!(app.sq.launched.is_empty() && t.leaders.is_empty());
    assert!(
        app.active_notice().unwrap().starts_with("The commander says the Coding squad doesn't fit #1: This is docs work"),
        "{:?}",
        app.active_notice()
    );
    key(&mut app, KeyCode::Enter);
    let mut term = Terminal::new(TestBackend::new(120, 34)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("State New") && f.contains("Commander This is docs work; the Coding squad writes code."), "{f}");
    // The same squad again: asked, not checked again.
    hand(&mut app);
    assert!(matches!(app.mode, Mode::Confirm(crate::app::Confirm::HandAnyway { number: 1, .. })));
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("The commander said this squad doesn't fit. Hand it anyway?") && f.contains("y hand it over"), "{f}");
    key(&mut app, KeyCode::Char('n'));
    assert!(matches!(app.mode, Mode::List) && app.sq.launched.is_empty());
    hand(&mut app);
    key(&mut app, KeyCode::Char('y'));
    assert_eq!(app.ticket(1).unwrap().busy.as_deref(), Some("Making the worktree…"));
    app.refresh_until_for_test(20, "the leader to start", |a| !a.sq.launched.is_empty());
    assert_eq!(fake.calls(), 1, "overruled: no second check");
    let t = Tickets::open(&w.config, &id).get(1).cloned().unwrap();
    assert_eq!((t.state, t.leaders.len(), t.handover), (State::Working, 1, None));
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn when_the_commander_cant_run_nothing_is_handed_over_and_the_ticket_says_why() {
    let base = std::env::temp_dir().join(format!("platoon-sq-limit-{}", std::process::id()));
    // A git repository of the test's own: no worktree may be made of it.
    let repo = base.join("shop");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("README"), "x\n").unwrap();
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(ok);
    };
    git(&["init", "-q"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "first"]);
    // Today's tokens are used up: the gate says no before any call.
    let (w, mut app, id, fake) = with_commander("sq-limit", 600_000, &repo);
    hand(&mut app);
    assert!(!app.squads_busy(), "no call was even started");
    let t = Tickets::open(&w.config, &id).get(1).cloned().unwrap();
    assert_eq!(t.state, State::New);
    assert_eq!(t.handover.as_ref().map(|h| h.says.as_str()), Some("Session limit has been reached"));
    assert!(
        app.active_notice().unwrap().starts_with("Session limit has been reached. #1 wasn't handed over."),
        "{:?}",
        app.active_notice()
    );
    assert_eq!(fake.calls(), 0);
    assert!(app.sq.launched.is_empty() && t.leaders.is_empty());
    assert!(!base.join("shop-worktrees").exists(), "no worktree");
    // A refusal isn't overruled with "Hand it anyway?": picking the squad again checks again.
    hand(&mut app);
    assert!(matches!(app.mode, Mode::List));
    // The AI features switched off: said as that.
    let mut s = crate::settings::load(&w.config).0;
    s.tokens_enabled = false;
    crate::settings::save(&w.config, &s).unwrap();
    app.refresh_for_test();
    hand(&mut app);
    assert_eq!(app.ticket(1).unwrap().handover.as_ref().map(|h| h.says.as_str()), Some("AI features are switched off"));
    // The call fails: said plainly, nothing handed over.
    s.tokens_enabled = true;
    s.ai_daily_tokens = 5_000_000;
    crate::settings::save(&w.config, &s).unwrap();
    app.refresh_for_test();
    fake.fail(Some("boom"));
    hand(&mut app);
    app.refresh_until_for_test(20, "the failed check", |a| !a.squads_busy());
    let t = app.ticket(1).cloned().unwrap();
    assert_eq!(t.state, State::New);
    assert!(t.handover.as_ref().unwrap().says.starts_with("The commander's check failed ("), "{t:?}");
    assert!(app.sq.launched.is_empty() && !base.join("shop-worktrees").exists());
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_worktree_that_cant_be_made_puts_the_ticket_back_to_new_with_the_reason() {
    let base = std::env::temp_dir().join(format!("platoon-sq-wt-{}", std::process::id()));
    let (w, mut app, id, fake) = with_commander("sq-wt", 0, &base.join("docs"));
    fake.reply_default(r#"{"fits":true,"reason":"Coding work."}"#);
    // A git that says no to everything but finding the repository.
    let git = base.join("fake-git");
    std::fs::write(&git, "#!/bin/sh\ncase \"$*\" in *rev-parse\\ --show-toplevel*) echo /nowhere/shop; exit 0;; *worktree\\ list*) exit 0;; esac\necho 'fatal: not today' >&2\nexit 128\n").unwrap();
    std::fs::set_permissions(&git, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    app.set_git_program_for_test(&git.to_string_lossy());
    hand(&mut app);
    app.refresh_until_for_test(20, "the worktree step", |a| !a.squads_busy());
    let t = Tickets::open(&w.config, &id).get(1).cloned().unwrap();
    assert_eq!((t.state.clone(), t.leaders.len()), (State::New, 0), "{t:?}");
    let h = t.handover.unwrap();
    assert_eq!(h.kind, crate::tickets::HandoverKind::NotStarted);
    assert!(h.says.starts_with("Couldn't make the worktree: "), "{}", h.says);
    assert!(app.active_notice().unwrap().starts_with("Couldn't hand #1 to the Coding squad: "), "{:?}", app.active_notice());
    assert!(app.sq.launched.is_empty());
    let _ = std::fs::remove_dir_all(&base);
}

// ---- squads that need no folder, the work folder, and worktrees by the squad's rule ----

/// A project "Web shop" with its screen open on the Tickets pane, the smart features on and the commander answered
/// by a stand-in `claude -p` that says every squad fits. `folders`: each ticket's folders, one ticket each.
fn handing(name: &str, folders: &[Vec<String>]) -> (World, App, String, crate::testkit::AiFake) {
    let w = World::new(name);
    w.transcript("user-1", "/tmp", "make a basket page", 10);
    let mut s = crate::settings::Settings { tokens_enabled: true, tokens_asked: true, ..Default::default() };
    s.auto_archive_days = 0;
    crate::settings::save(&w.config, &s).unwrap();
    let mut app = w.app();
    let fake = crate::testkit::AiFake::new(name);
    fake.reply_default(r#"{"fits":true,"reason":"It fits."}"#);
    app.set_ai_program_for_test(&fake.script);
    let id = app.projects.create("Web shop").unwrap();
    app.projects.assign("user-1", Some(&id)).unwrap();
    let mut t = Tickets::open(&w.config, &id);
    for (i, f) in folders.iter().enumerate() {
        t.add(NewTicket { title: format!("Ticket {}", i + 1), folders: f.clone(), ..Default::default() }).unwrap();
    }
    app.reload_for_test();
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.pane, Pane::Tickets);
    (w, app, id, fake)
}

/// `S` on ticket `n`, the squad called `squad` picked, and the leader started (as far as a unit test starts one).
fn hand_to(app: &mut App, n: u32, squad: &str) {
    let at = app.ticket_rows().iter().position(|r| *r == crate::app::TRow::Ticket(n)).unwrap();
    app.tk.selected = Some(app.ticket_rows()[at]);
    key(app, KeyCode::Char('S'));
    assert!(matches!(app.mode, Mode::SquadPick), "{:?}", app.active_notice());
    let to = app.sq.pick.iter().position(|s| s.name == squad).unwrap();
    for _ in 0..to {
        key(app, KeyCode::Down);
    }
    let before = app.sq.launched.len();
    key(app, KeyCode::Enter);
    app.refresh_until_for_test(20, "the leader to start", |a| a.sq.launched.len() > before);
}

fn value_after<'a>(args: &'a [String], flag: &str) -> &'a str {
    &args[args.iter().position(|a| a == flag).unwrap_or_else(|| panic!("no {flag} in {args:?}")) + 1]
}

fn add_dirs(args: &[String]) -> Vec<String> {
    args.windows(2).filter(|w| w[0] == "--add-dir").map(|w| w[1].clone()).collect()
}

fn repo(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("README.md"), "# Shop\n").unwrap();
    for args in [&["init", "-q"][..], &["add", "."], &["commit", "-q", "-m", "first"]] {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }
}

#[test]
fn a_ticket_without_a_folder_goes_to_any_squad_and_starts_in_its_work_folder() {
    let (w, mut app, id, fake) = handing("sq-nofolder", &[vec![], vec![]]);
    // Coding takes it: the commander is asked (told it has no folder of its own), then the leader starts in the work folder.
    hand_to(&mut app, 1, "Coding");
    assert!(
        fake.stdin(1).starts_with("The squad: Coding\n")
            && fake.stdin(1).contains(
                "Where it works: the ticket has no folder of its own; the squad works in a private work folder platoon keeps for it.\n"
            ),
        "{}",
        fake.stdin(1)
    );
    let work = crate::tickets::work_dir(&w.config, &id, 1);
    assert!(work.ends_with("tickets/01-work") && work.is_dir(), "{}", work.display());
    let (dir, args) = app.sq.launched_with[0].clone();
    assert_eq!(dir, work.to_string_lossy(), "the leader starts in the ticket's work folder");
    assert!(add_dirs(&args).is_empty(), "no folders to add: {args:?}");
    assert_eq!(value_after(&args, "--agent"), "coding-leader");
    assert_eq!(value_after(&args, "--model"), "sonnet");
    assert_eq!(value_after(&args, "-n"), "Coding squad · #1 Ticket 1");
    let agents: serde_json::Value = serde_json::from_str(value_after(&args, "--agents")).unwrap();
    let prompt = agents["coding-leader"]["prompt"].as_str().unwrap();
    assert!(
        prompt.contains("The ticket has no folder of its own: you and your members work in its private work folder")
            && !prompt.contains("stays in the worktree"),
        "{prompt}"
    );
    let first = args.last().unwrap();
    assert!(
        first.contains(&format!("- {} (the ticket's private work folder; the session starts here.", work.display()))
            && first.contains("The ticket has no folder of its own."),
        "{first}"
    );
    let t = app.ticket(1).unwrap();
    assert!(t.state == State::Working && t.worktrees.is_empty(), "{t:?}");
    let snap = Tickets::open(&w.config, &id).snapshot(t.leader().unwrap()).unwrap();
    assert!(snap.contains("\"starts_in\": \"work-folder\""), "continued, it starts there again: {snap}");
    // Summarize takes the other one too.
    hand_to(&mut app, 2, "Summarize");
    let (dir, args) = app.sq.launched_with[1].clone();
    assert_eq!(dir, crate::tickets::work_dir(&w.config, &id, 2).to_string_lossy());
    assert_eq!(value_after(&args, "--agent"), "summarize-leader");
}

#[test]
fn research_works_in_a_worktree_of_a_git_folder_and_writes_its_report_in_the_work_folder() {
    let base = std::env::temp_dir().join(format!("platoon-sq-read-{}", std::process::id()));
    let base = crate::testkit::removed_at_exit(base);
    let shop = base.join("shop");
    repo(&shop);
    let notes = base.join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    let folders = vec![shop.to_string_lossy().into_owned(), notes.to_string_lossy().into_owned()];
    let (w, mut app, id, _fake) = handing("sq-read", std::slice::from_ref(&folders));
    hand_to(&mut app, 1, "Research");
    let work = crate::tickets::work_dir(&w.config, &id, 1);
    let wt = base.join("shop-worktrees/ticket-1");
    assert!(wt.join("README.md").exists(), "Research works in a worktree of the git folder now, like every squad");
    let (dir, args) = app.sq.launched_with[0].clone();
    assert_eq!(dir, wt.to_string_lossy(), "it starts in the worktree");
    assert_eq!(
        add_dirs(&args),
        [notes.to_string_lossy().into_owned(), work.to_string_lossy().into_owned()],
        "the plain folder as it is, then the work folder for its report"
    );
    assert_eq!(app.ticket(1).unwrap().worktrees.len(), 1);
    let agents: serde_json::Value = serde_json::from_str(value_after(&args, "--agents")).unwrap();
    let prompt = agents["research-leader"]["prompt"].as_str().unwrap();
    assert!(
        prompt.contains("goes in the ticket's private work folder, never in the project's folders") && !prompt.contains("for reading only"),
        "{prompt}"
    );
    assert!(
        agents["research-report-writer"]["prompt"]
            .as_str()
            .unwrap()
            .contains("report.md in the ticket's private work folder (the leader gives you its path), never in the project's folders")
    );
    let first = args.last().unwrap();
    assert!(
        first.contains(&format!(
            "- {} (the ticket's private work folder, kept by platoon: for what the squad writes for the ticket rather than the project",
            work.display()
        )),
        "{first}"
    );
    let snap = Tickets::open(&w.config, &id).snapshot(app.ticket(1).unwrap().leader().unwrap()).unwrap();
    assert!(snap.contains("\"starts_in\": \"folder\""), "{snap}");
}

#[test]
fn docs_makes_a_worktree_only_of_a_git_folder_and_uses_a_plain_one_as_it_is() {
    let base = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-sq-docs-{}", std::process::id())));
    let shop = base.join("shop");
    repo(&shop);
    let plain = base.join("handbook");
    std::fs::create_dir_all(&plain).unwrap();
    let (s, p) = (shop.to_string_lossy().into_owned(), plain.to_string_lossy().into_owned());
    let (w, mut app, id, _fake) = handing("sq-docs", &[vec![s.clone(), p.clone()], vec![p.clone()]]);
    // A git folder and a plain one: a worktree of the git one only, where it starts; the plain one as it is.
    hand_to(&mut app, 1, "Docs");
    let wt = base.join("shop-worktrees/ticket-1");
    assert!(wt.join("README.md").exists(), "a worktree of the temp repository");
    let t = app.ticket(1).unwrap();
    assert_eq!(t.worktrees.len(), 1, "{t:?}");
    assert_eq!((t.worktrees[0].folder.as_str(), t.worktrees[0].branch.as_str()), (s.as_str(), "ticket/1"));
    let (dir, args) = app.sq.launched_with[0].clone();
    assert_eq!(dir, wt.to_string_lossy());
    assert_eq!(
        add_dirs(&args),
        [p.clone(), crate::tickets::work_dir(&w.config, &id, 1).to_string_lossy().into_owned()],
        "the plain folder, then the work folder"
    );
    let agents: serde_json::Value = serde_json::from_str(value_after(&args, "--agents")).unwrap();
    assert_eq!(agents["docs-tech-writer"]["tools"], serde_json::json!(["Read", "Grep", "Glob", "Edit", "Write"]));
    assert!(
        agents["docs-leader"]["prompt"]
            .as_str()
            .unwrap()
            .contains("the change stays where it was made (the worktree, or the folder used as it is)")
    );
    // Only a plain folder: no worktree at all; it starts in the folder itself.
    hand_to(&mut app, 2, "Docs");
    let t = app.ticket(2).unwrap();
    assert!(t.worktrees.is_empty(), "{t:?}");
    let (dir, args) = app.sq.launched_with[1].clone();
    assert_eq!(dir, p);
    assert_eq!(add_dirs(&args), [crate::tickets::work_dir(&w.config, &id, 2).to_string_lossy().into_owned()]);
    let agents: serde_json::Value = serde_json::from_str(value_after(&args, "--agents")).unwrap();
    let prompt = agents["docs-leader"]["prompt"].as_str().unwrap();
    assert!(prompt.contains("the change stays in the ticket's folder, uncommitted") && !prompt.contains("worktree"), "{prompt}");
    assert!(!base.join("handbook-worktrees").exists());
}
