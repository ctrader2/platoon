//! Archiving a conversation something is running: the row leaves at once, the process is stopped in the background,
//! and the archive is written only once it has really exited. Every process here is one the test started itself
//! (a shell script that takes a while to end on SIGTERM, or ignores it): nothing else is ever signalled.

use crate::app::{App, Confirm, Mode};
use crate::host::HostInfo;
use crate::testkit::{World, alive};
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
};
use std::time::{Duration, Instant};

const LONG: Duration = Duration::from_secs(10);

fn key(app: &mut App, c: KeyCode) {
    app.on_key(KeyEvent::new(c, KeyModifiers::NONE));
}

fn ch(app: &mut App, c: char) {
    key(app, KeyCode::Char(c));
}

fn hosted(pid: u32) -> HostInfo {
    HostInfo { claude_pid: pid, host_pid: 0, dir: "/tmp".into(), args: Vec::new(), sock: Default::default() }
}

fn ids(app: &App) -> Vec<String> {
    app.visible().iter().map(|&i| app.sessions[i].id.clone()).collect()
}

fn select(app: &mut App, id: &str) {
    app.selected = app.visible().iter().position(|&i| app.sessions[i].id == id).expect("row is listed");
}

fn screen(term: &mut Terminal<TestBackend>, app: &App) -> String {
    term.draw(|f| crate::ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn archived_ids(w: &World) -> Vec<String> {
    let mut v: Vec<String> = w.archive_file().map(|f| f["archived"].as_object().unwrap().keys().cloned().collect()).unwrap_or_default();
    v.sort();
    v
}

/// Three conversations (alpha, beta, gamma), with `alpha` run by `pid` under a platoon host. The process is made by `make`.
fn rig(name: &str, make: impl FnOnce(&mut World) -> u32) -> (World, App, u32) {
    let mut w = World::new(name);
    w.transcript("alpha-1", "/srv/alpha", "refactor the parser", 5);
    w.transcript("beta-2", "/srv/beta", "add a login page", 30);
    w.transcript("gamma-3", "/srv/gamma", "write the docs", 60);
    let pid = make(&mut w);
    let mut app = w.app();
    app.refresh_with(vec![hosted(pid)], true);
    (w, app, pid)
}

fn slow(w: &mut World) -> u32 {
    w.live_slow_exit("alpha-1", "/srv/alpha", 1, "idle", 1.5)
}

#[test]
fn the_row_leaves_at_once_and_the_archive_is_written_only_after_the_process_has_exited() {
    let (w, mut app, pid) = rig("stop-slow", slow);
    let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
    assert!(screen(&mut term, &app).contains("refactor the parser"));
    assert_eq!(app.counts, [3, 0]);
    select(&mut app, "alpha-1");

    // The key press, the next event-loop turn and a frame: none of them waits for the process.
    let t = Instant::now();
    ch(&mut app, 'a');
    app.poll_stops();
    let shown = screen(&mut term, &app);
    let took = t.elapsed();
    assert!(took < Duration::from_millis(400), "the key press took {took:?} (the process needs 1.5 s)");
    assert!(alive(pid), "the process is still shutting down: the row left without waiting for it");
    assert!(
        !shown.contains("refactor the parser") && shown.contains("add a login page"),
        "gone from the screen, the others stay:\n{shown}"
    );
    assert_eq!(ids(&app), ["beta-2", "gamma-3"]);
    assert_eq!(app.counts, [2, 0], "the Active count dropped at once");
    assert!(shown.split_whitespace().collect::<Vec<_>>().join(" ").contains("Active 2 Archived 0"), "{shown}");
    assert!(archived_ids(&w).is_empty(), "not archived while its process runs");
    assert_eq!(app.stops_pending(), 1);

    // Until the process is gone, nothing is recorded; the moment it is, the record follows.
    let start = Instant::now();
    while app.stops_pending() > 0 {
        app.poll_stops();
        if !archived_ids(&w).is_empty() {
            assert!(!alive(pid), "the archive was written while the process still ran");
        }
        assert!(start.elapsed() < LONG, "the stop never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!alive(pid));
    assert_eq!(archived_ids(&w), ["alpha-1"]);
    assert_eq!(app.counts, [2, 1]);
    assert_eq!(app.active_notice(), Some("Archived \"alpha\""));
    key(&mut app, KeyCode::Right);
    assert_eq!(ids(&app), ["alpha-1"], "the Archived tab shows it now");
    assert!(screen(&mut term, &app).contains("refactor the parser"));
    assert!(w.root.join("projects/-some-project/alpha-1.jsonl").exists(), "the transcript is kept");
}

#[test]
fn a_process_that_does_not_stop_in_time_brings_the_row_back_with_the_notice_and_nothing_is_archived() {
    let (w, mut app, pid) = rig("stop-deaf", |w| w.live_deaf("alpha-1", "/srv/alpha", 1, "idle"));
    app.set_stop_times_for_test(Duration::from_millis(300), Duration::from_secs(3));
    select(&mut app, "alpha-1");
    ch(&mut app, 'a');
    assert_eq!(ids(&app), ["beta-2", "gamma-3"], "the row leaves at once, even though it will not stop");
    assert!(app.settle_stops_for_test(LONG));
    assert_eq!(ids(&app), ["alpha-1", "beta-2", "gamma-3"], "and comes back");
    assert_eq!(app.counts, [3, 0]);
    assert_eq!(app.active_notice(), Some("Couldn't stop \"alpha\": it didn't exit in time. Try again in a moment."));
    assert!(archived_ids(&w).is_empty() && w.archive_file().is_none(), "no archive record was ever written");
    assert!(alive(pid), "our stand-in ignored SIGTERM");
    let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
    let shown = screen(&mut term, &app);
    assert!(shown.contains("refactor the parser") && shown.contains("Couldn't stop"), "{shown}");
    // Trying again is possible.
    select(&mut app, "alpha-1");
    ch(&mut app, 'a');
    assert_eq!((ids(&app), app.stops_pending()), (vec!["beta-2".to_string(), "gamma-3".to_string()], 1));
}

#[test]
fn a_conversation_nothing_runs_is_archived_at_once_as_before() {
    let (w, mut app, _pid) = rig("stop-none", slow);
    select(&mut app, "gamma-3");
    let t = Instant::now();
    ch(&mut app, 'a');
    assert!(t.elapsed() < Duration::from_millis(400));
    assert_eq!(app.stops_pending(), 0, "no worker for nothing to stop");
    assert_eq!(archived_ids(&w), ["gamma-3"], "recorded straight away");
    assert_eq!((app.counts.clone(), app.active_notice()), (vec![2, 1], Some("Archived \"gamma\"")));
}

#[test]
fn two_conversations_can_be_stopping_at_once_and_each_is_archived_when_its_own_process_is_gone() {
    let mut w = World::new("stop-two");
    w.transcript("alpha-1", "/srv/alpha", "refactor the parser", 5);
    w.transcript("beta-2", "/srv/beta", "add a login page", 30);
    w.transcript("gamma-3", "/srv/gamma", "write the docs", 60);
    let a = w.live_slow_exit("alpha-1", "/srv/alpha", 1, "idle", 0.8);
    let b = w.live_slow_exit("beta-2", "/srv/beta", 1, "idle", 1.6);
    let mut app = w.app();
    app.refresh_with(vec![hosted(a), hosted(b)], true);
    select(&mut app, "alpha-1");
    ch(&mut app, 'a');
    select(&mut app, "beta-2");
    ch(&mut app, 'a');
    assert_eq!((ids(&app), app.counts.clone(), app.stops_pending()), (vec!["gamma-3".to_string()], vec![1, 0], 2));
    // Each record appears only after its own process is gone (the quicker one first, the other still on its way).
    let start = Instant::now();
    while app.stops_pending() > 0 {
        app.poll_stops();
        let done = archived_ids(&w);
        assert!(!done.contains(&"alpha-1".to_string()) || !alive(a), "alpha archived while it ran");
        assert!(!done.contains(&"beta-2".to_string()) || !alive(b), "beta archived while it ran");
        assert!(start.elapsed() < LONG);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(app.settle_stops_for_test(LONG));
    assert_eq!(archived_ids(&w), ["alpha-1", "beta-2"]);
    assert_eq!(app.counts, [1, 2]);
    assert!(!alive(a) && !alive(b));
}

#[test]
fn one_of_two_that_fails_comes_back_while_the_other_is_archived() {
    let mut w = World::new("stop-mixed");
    w.transcript("alpha-1", "/srv/alpha", "refactor the parser", 5);
    w.transcript("beta-2", "/srv/beta", "add a login page", 30);
    let a = w.live_slow_exit("alpha-1", "/srv/alpha", 1, "idle", 0.1);
    let b = w.live_deaf("beta-2", "/srv/beta", 1, "idle");
    let mut app = w.app();
    app.set_stop_times_for_test(Duration::from_millis(500), Duration::from_secs(3));
    app.refresh_with(vec![hosted(a), hosted(b)], true);
    select(&mut app, "alpha-1");
    ch(&mut app, 'a');
    select(&mut app, "beta-2");
    ch(&mut app, 'a');
    assert!(ids(&app).is_empty());
    assert!(app.settle_stops_for_test(LONG));
    assert_eq!(archived_ids(&w), ["alpha-1"]);
    assert_eq!((ids(&app), app.counts.clone()), (vec!["beta-2".to_string()], vec![1, 1]));
}

#[test]
fn pressing_a_again_for_a_conversation_already_stopping_does_nothing_more() {
    let (w, mut app, pid) = rig("stop-twice", slow);
    select(&mut app, "alpha-1");
    ch(&mut app, 'a');
    app.archive_for_test("alpha-1", "alpha"); // the same conversation once more (a second window, a stale dialog)
    assert_eq!(app.stops_pending(), 1);
    assert!(alive(pid));
    assert!(app.settle_stops_for_test(LONG));
    assert_eq!(archived_ids(&w), ["alpha-1"]);
    assert_eq!(app.archive_writes_for_test(), 1, "one record, not two");
}

#[test]
fn a_lone_row_that_is_stopping_cannot_be_acted_on_again() {
    let mut w = World::new("stop-lone");
    w.transcript("alpha-1", "/srv/alpha", "refactor the parser", 5);
    let pid = w.live_slow_exit("alpha-1", "/srv/alpha", 1, "idle", 1.0);
    let mut app = w.app();
    app.refresh_with(vec![hosted(pid)], true);
    ch(&mut app, 'a');
    assert!(ids(&app).is_empty());
    ch(&mut app, 'a');
    assert_eq!(app.active_notice(), Some("No conversation selected"));
    assert_eq!(app.stops_pending(), 1);
    assert!(app.settle_stops_for_test(LONG));
    assert_eq!(archived_ids(&w), ["alpha-1"]);
}

#[test]
fn restore_rename_filter_and_search_keep_working_on_other_rows_while_a_stop_is_in_flight() {
    let mut w = World::new("stop-others");
    w.transcript("alpha-1", "/srv/alpha", "refactor the parser", 5);
    w.transcript("beta-2", "/srv/beta", "add a login page", 30);
    w.transcript("gamma-3", "/srv/gamma", "write the docs", 60);
    w.transcript("old-4", "/srv/delta", "an old parser question", 90);
    std::fs::create_dir_all(&w.config).unwrap();
    std::fs::write(w.config.join("archive.json"), r#"{"version":1,"archived":{"old-4":5}}"#).unwrap();
    let pid = w.live_slow_exit("alpha-1", "/srv/alpha", 1, "idle", 1.5);
    let mut app = w.app();
    app.refresh_with(vec![hosted(pid)], true);
    select(&mut app, "alpha-1");
    ch(&mut app, 'a');
    assert!(alive(pid) && app.stops_pending() == 1);

    // Rename another row.
    select(&mut app, "beta-2");
    ch(&mut app, 'r');
    app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    for c in "Login work".chars() {
        ch(&mut app, c);
    }
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.selected_session().unwrap().name, "Login work");

    // The quick filter narrows what is listed, and the stopping row is not in it.
    ch(&mut app, '/');
    for c in "parser".chars() {
        ch(&mut app, c);
    }
    assert!(app.visible().is_empty(), "alpha mentions the parser but is on its way out; the other one is archived");
    key(&mut app, KeyCode::Esc);
    std::thread::sleep(Duration::from_millis(60)); // an Esc followed at once by a key reads as Alt+key
    assert!(app.filter.is_empty());

    // The full-text search: the stopping conversation is not offered, the archived one is.
    ch(&mut app, '/');
    key(&mut app, KeyCode::Tab);
    for c in "parser".chars() {
        ch(&mut app, c);
    }
    let found: Vec<&str> = app.search.results.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(found, ["old-4"], "the stopping row is left out of the results");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!((app.tab, app.selected_session().unwrap().id.as_str()), (1, "old-4"));

    // Restore the archived one (`a` on the Archived tab) while the stop is still going.
    ch(&mut app, 'a');
    assert_eq!(app.active_notice(), Some("Restored \"delta\""));
    assert_eq!(app.stops_pending(), 1);
    assert!(alive(pid), "all of that happened while the process was still shutting down");
    assert!(app.settle_stops_for_test(LONG));
    assert_eq!(archived_ids(&w), ["alpha-1"]);
    assert_eq!(app.counts, [3, 1]);
}

#[test]
fn quitting_waits_for_a_pending_stop_so_its_record_is_written() {
    let (w, mut app, pid) = rig("stop-quit-wait", |w| w.live_slow_exit("alpha-1", "/srv/alpha", 1, "idle", 0.6));
    select(&mut app, "alpha-1");
    ch(&mut app, 'a');
    assert!(archived_ids(&w).is_empty());
    let t = Instant::now();
    app.finish_stops();
    assert!(t.elapsed() < Duration::from_secs(3), "waited {:?}", t.elapsed());
    assert!(!alive(pid));
    assert_eq!(archived_ids(&w), ["alpha-1"], "written before leaving");
    assert_eq!(app.stops_pending(), 0);
}

#[test]
fn quitting_does_not_hang_on_a_stop_that_is_not_finishing_and_leaves_nothing_archived() {
    let (w, mut app, pid) = rig("stop-quit-hang", |w| w.live_deaf("alpha-1", "/srv/alpha", 1, "idle"));
    app.set_stop_times_for_test(Duration::from_secs(30), Duration::from_millis(400));
    select(&mut app, "alpha-1");
    ch(&mut app, 'a');
    let t = Instant::now();
    app.finish_stops();
    let took = t.elapsed();
    assert!(took >= Duration::from_millis(350) && took < Duration::from_secs(2), "waited {took:?}");
    assert!(alive(pid), "the stand-in ignores SIGTERM");
    assert!(archived_ids(&w).is_empty() && w.archive_file().is_none(), "never archived while its process still runs");
    // A later run of platoon sees an ordinary, running conversation.
    let mut later = w.app();
    later.refresh_with(vec![hosted(pid)], true);
    assert_eq!((later.counts.clone(), later.visible().len()), (vec![3, 0], 3));
}

#[test]
fn a_conversation_opened_again_from_another_terminal_while_it_stopped_stays_in_the_list() {
    let mut w = World::new("stop-reopened");
    w.transcript("alpha-1", "/srv/alpha", "refactor the parser", 5);
    let pid = w.live_slow_exit("alpha-1", "/srv/alpha", 1, "idle", 0.5);
    let mut app = w.app();
    app.refresh_with(vec![hosted(pid)], true);
    ch(&mut app, 'a');
    // `platoon open alpha-1` from elsewhere starts a new process for it (a stand-in of ours) before the old one is gone.
    let again = w.live("alpha-1", "/srv/alpha", 2, "idle");
    assert!(app.settle_stops_for_test(LONG));
    assert!(alive(again));
    assert!(archived_ids(&w).is_empty(), "it is running again: not archived");
    assert_eq!(app.counts, [1, 0]);
    assert_eq!(app.active_notice(), Some("\"alpha\" was opened again, so it stays in the list."));
}

#[test]
fn auto_archive_leaves_a_conversation_that_is_being_stopped_to_the_stop() {
    let mut w = World::new("stop-auto");
    w.settings(120, 2);
    w.transcript("old-1", "/srv/alpha", "refactor the parser", 3 * 24 * 60);
    let pid = w.live_slow_exit("old-1", "/srv/alpha", 1, "idle", 0.4);
    let mut app = w.app();
    app.refresh_with(vec![hosted(pid)], true);
    ch(&mut app, 'a');
    while alive(pid) {
        std::thread::sleep(Duration::from_millis(10));
    }
    // Its process is gone but the stop has not been taken in yet: auto-archive must not act on it.
    app.reload_for_test();
    app.auto_archive_at_for_test(crate::usage::now() + 60);
    assert!(app.active_notice().is_none_or(|n| !n.contains("Auto-archived")), "{:?}", app.active_notice());
    assert_eq!(archived_ids(&w), Vec::<String>::new());
    assert!(app.settle_stops_for_test(LONG));
    assert_eq!(archived_ids(&w), ["old-1"]);
    assert_eq!(app.active_notice(), Some("Archived \"alpha\""));
}

#[test]
fn the_dialog_paths_still_ask_first_and_then_take_the_same_no_wait_road() {
    // Working right now: asks, cancel touches nothing, yes hides the row at once.
    let mut w = World::new("stop-confirm");
    w.transcript("busy-1", "/srv/alpha", "long task", 1);
    w.transcript("other-2", "/srv/beta", "another", 20);
    let pid = w.live_slow_exit("busy-1", "/srv/alpha", 1, "busy", 1.5);
    let mut app = w.app();
    app.refresh_with(vec![hosted(pid)], true);
    select(&mut app, "busy-1");
    ch(&mut app, 'a');
    assert!(matches!(app.mode, Mode::Confirm(Confirm::Archive { busy: true, .. })));
    assert_eq!((app.counts.clone(), app.stops_pending()), (vec![2, 0], 0), "nothing hides before the answer");
    ch(&mut app, 'n');
    assert!(alive(pid) && app.stops_pending() == 0 && archived_ids(&w).is_empty());
    ch(&mut app, 'a');
    let t = Instant::now();
    ch(&mut app, 'y');
    assert!(t.elapsed() < Duration::from_millis(400) && alive(pid));
    assert!(matches!(app.mode, Mode::List));
    assert_eq!((ids(&app), app.counts.clone()), (vec!["other-2".to_string()], vec![1, 0]));
    assert!(archived_ids(&w).is_empty());
    assert!(app.settle_stops_for_test(LONG));
    assert_eq!(archived_ids(&w), ["busy-1"]);

    // A copy in a terminal platoon did not start: asks as well.
    let mut w = World::new("stop-confirm-outside");
    w.transcript("out-1", "/srv/alpha", "outside", 5);
    let pid = w.live_slow_exit("out-1", "/srv/alpha", 1, "idle", 1.5);
    let mut app = w.app();
    ch(&mut app, 'a');
    assert!(matches!(app.mode, Mode::Confirm(Confirm::Archive { outside: true, .. })));
    ch(&mut app, 'y');
    assert!(alive(pid) && ids(&app).is_empty() && app.stops_pending() == 1);
    assert!(app.settle_stops_for_test(LONG));
    assert!(!alive(pid));
    assert_eq!(archived_ids(&w), ["out-1"]);
}

#[test]
fn a_background_row_still_refuses_and_starts_no_stop() {
    let mut w = World::new("stop-bg");
    let id = "dffac639-c805-47ba-b57c-3767f4f768d8";
    w.transcript(id, "/srv/alpha", "continue the release", 30);
    let pid = w.live_bg(id, "/srv/alpha", 1);
    let mut app = w.app();
    ch(&mut app, 'a');
    assert_eq!(app.stops_pending(), 0);
    assert!(alive(pid) && w.archive_file().is_none());
    assert!(app.active_notice().is_some_and(|n| n.contains("can't archive it")));
}
