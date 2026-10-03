//! The limit on squads at work at once, at the level of the app: a hand-over while every place is taken joins the
//! line ("waiting for a free squad"), starts by itself when a place frees (and not beside the ticket), survives a
//! new platoon, leaves the line when the user moves it, waits when the limit is lowered, and only one platoon window
//! starts the line. Also what the ticket screen and the Tickets pane say about a squad's work (tokens, members' runs)
//! from synthetic transcripts. Every directory is a temp dir; nothing here starts a session.

use crate::app::{App, Mode, Pane};
use crate::claude::MemberRun;
use crate::testkit::World;
use crate::tickets::{AgentNames, Leader, NewTicket, QUEUED_WORDS, State, Tickets};
use crate::ui;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
};
use serde_json::json;

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

fn flat(text: &str) -> String {
    text.replace(['│', '╭', '╮', '╰', '╯', '─'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A project "Web shop" with `tickets` tickets (no folder), its screen open on the Tickets pane, the smart features
/// on, `at_once` squads allowed at once, and the commander answered by a stand-in `claude -p` that says it fits.
fn world(name: &str, tickets: u32, at_once: u64) -> (World, App, String, crate::testkit::AiFake) {
    let w = World::new(name);
    w.transcript("user-1", "/tmp", "make a basket page", 10);
    let s = crate::settings::Settings {
        tokens_enabled: true,
        tokens_asked: true,
        auto_archive_days: 0,
        squads_at_once: at_once,
        ..Default::default()
    };
    crate::settings::save(&w.config, &s).unwrap();
    let mut app = w.app();
    let fake = crate::testkit::AiFake::new(name);
    fake.reply_default(r#"{"fits":true,"reason":"It fits."}"#);
    app.set_ai_program_for_test(&fake.script);
    let id = app.projects.create("Web shop").unwrap();
    app.projects.assign("user-1", Some(&id)).unwrap();
    let mut t = Tickets::open(&w.config, &id);
    for i in 1..=tickets {
        t.add(NewTicket { title: format!("Ticket {i}"), ..Default::default() }).unwrap();
    }
    open_project(&mut app);
    (w, app, id, fake)
}

fn open_project(app: &mut App) {
    app.reload_for_test();
    key(app, KeyCode::Char('2'));
    key(app, KeyCode::Enter);
    key(app, KeyCode::Tab);
    assert_eq!(app.pane, Pane::Tickets);
}

fn pick(app: &mut App, n: u32) {
    let at = app.ticket_rows().iter().position(|r| *r == crate::app::TRow::Ticket(n)).unwrap();
    app.tk.selected = Some(app.ticket_rows()[at]);
}

/// `a` on ticket `n`, the Summarize squad picked (it needs no folder), and the commander's answer taken in.
fn hand(app: &mut App, n: u32) {
    pick(app, n);
    key(app, KeyCode::Char('a'));
    assert!(matches!(app.mode, Mode::SquadPick), "{:?}", app.active_notice());
    let to = app.sq.pick.iter().position(|s| s.name == "Summarize").unwrap();
    for _ in 0..to {
        key(app, KeyCode::Down);
    }
    key(app, KeyCode::Enter);
    app.refresh_until_for_test(20, "the commander's answer", |a| !a.squads_busy());
}

fn on_disk(w: &World, id: &str, n: u32) -> crate::tickets::Ticket {
    Tickets::open(&w.config, id).get(n).cloned().unwrap()
}

#[test]
fn with_every_place_taken_a_hand_over_waits_and_starts_by_itself_when_a_place_frees() {
    let (w, mut app, id, fake) = world("sqq-line", 3, 1);
    hand(&mut app, 1);
    assert_eq!(app.sq.launched, [1]);
    // The first holds the one place (its process hasn't shown up in a unit test: it was written on the ticket just now).
    hand(&mut app, 2);
    assert_eq!(app.sq.launched, [1], "no second squad while the place is taken");
    assert_eq!(fake.calls(), 2, "the commander checked it first");
    let t = on_disk(&w, &id, 2);
    assert_eq!(
        (t.state, t.leaders.len(), t.queued.as_ref().map(|q| (q.squad.as_str(), q.anyway))),
        (State::Working, 0, Some(("summarize", false)))
    );
    assert_eq!(app.active_notice(), Some(crate::squadqueue::queued_notice(2, "Summarize", 1).as_str()));
    let mut term = Terminal::new(TestBackend::new(130, 34)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("#2 Ticket 2 Working waiting for a free squad"), "{f}");
    // A third joins the line behind it.
    hand(&mut app, 3);
    assert!(on_disk(&w, &id, 3).queued.is_some() && app.sq.launched == [1]);
    // On its own screen: Working · waiting for a free squad; `a` and enter say it is in the line.
    pick(&mut app, 2);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.tk.open, Some(2));
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains(&format!("State Working · {QUEUED_WORDS}")), "{f}");
    assert!(!f.contains("enter continue the squad") && !f.contains("a hand to a squad"), "{f}");
    key(&mut app, KeyCode::Char('a'));
    assert_eq!(app.active_notice(), Some("#2 is waiting for a free squad already. To take it out of the line, cancel it with c."));
    key(&mut app, KeyCode::Esc);

    // The first squad stops to ask the user something: its place frees, and the oldest in the line starts, not
    // beside its ticket (nobody asked just now).
    Tickets::open(&w.config, &id).squad_says(1, &State::Waiting, "Asks: red or blue?", None).unwrap();
    app.refresh_until_for_test(20, "#2 to start", |a| a.sq.launched.len() == 2);
    assert_eq!(app.sq.launched, [1, 2]);
    let t = on_disk(&w, &id, 2);
    assert_eq!((t.state, t.leaders.len(), t.queued), (State::Working, 1, None));
    assert_eq!(t.leaders[0].squad, "summarize");
    assert!(matches!(app.mode, Mode::List) && app.sq.split.is_none());
    assert_eq!(app.active_notice(), Some("A squad is free: #2 is with the Summarize squad now. Open it from Web shop's tickets."));
    assert_eq!(fake.calls(), 3, "no second check when it leaves the line");
    // #2 holds the place now: #3 still waits.
    app.refresh_for_test();
    assert!(on_disk(&w, &id, 3).queued.is_some() && app.sq.launched.len() == 2);
    // #2 is done: #3 goes.
    Tickets::open(&w.config, &id).squad_says(2, &State::Done, "", Some("Summed up.")).unwrap();
    app.refresh_until_for_test(20, "#3 to start", |a| a.sq.launched.len() == 3);
    assert_eq!(on_disk(&w, &id, 3).leaders.len(), 1);
}

#[test]
fn the_line_survives_platoon_closing_and_the_next_platoon_starts_it_oldest_first() {
    let (w, app, id, _fake) = world("sqq-restart", 3, 1);
    drop(app);
    // Written as a platoon that has since closed left them: #3 joined the line before #2.
    let mut t = Tickets::open(&w.config, &id);
    t.queue(3, "summarize", false).unwrap();
    let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(t.path()).unwrap()).unwrap();
    v["tickets"][2]["queued"]["since"] = json!(100);
    std::fs::write(t.path(), v.to_string()).unwrap();
    let mut t = Tickets::open(&w.config, &id);
    t.queue(2, "summarize", true).unwrap();
    let mut app = w.app();
    open_project(&mut app);
    app.refresh_until_for_test(20, "the oldest in the line to start", |a| !a.sq.launched.is_empty());
    assert_eq!(app.sq.launched, [3], "the oldest first, and only one: the limit is one");
    app.refresh_for_test();
    assert_eq!(app.sq.launched, [3]);
    let two = on_disk(&w, &id, 2);
    assert!(two.queued.is_some_and(|q| q.anyway), "still waiting, still overruled");
}

#[test]
fn cancelling_takes_a_ticket_out_of_the_line_and_it_never_starts() {
    let (w, mut app, id, _fake) = world("sqq-out", 2, 1);
    hand(&mut app, 1);
    hand(&mut app, 2);
    assert!(on_disk(&w, &id, 2).queued.is_some());
    // `c` on a ticket in the line: asked, then Cancelled and out of the line.
    pick(&mut app, 2);
    key(&mut app, KeyCode::Char('c'));
    assert!(matches!(app.mode, Mode::Confirm(crate::app::Confirm::CancelTicket { number: 2, pid: None, .. })));
    key(&mut app, KeyCode::Char('y'));
    let t = on_disk(&w, &id, 2);
    assert_eq!((t.state, t.queued), (State::Cancelled, None));
    Tickets::open(&w.config, &id).squad_says(1, &State::Done, "", Some("Done.")).unwrap();
    for _ in 0..5 {
        app.refresh_for_test();
    }
    assert_eq!(app.sq.launched, [1], "out of the line: nothing starts by itself");
}

#[test]
fn lowering_the_limit_stops_nothing_and_new_squads_wait_until_there_is_room() {
    let (w, mut app, id, _fake) = world("sqq-lower", 3, 2);
    hand(&mut app, 1);
    hand(&mut app, 2);
    assert_eq!(app.sq.launched, [1, 2]);
    let mut s = crate::settings::load(&w.config).0;
    s.squads_at_once = 1;
    crate::settings::save(&w.config, &s).unwrap();
    app.refresh_for_test();
    // Both still at work; nothing was stopped (a unit test starts no process, and none was ended).
    assert!([1, 2].iter().all(|n| on_disk(&w, &id, *n).state == State::Working));
    hand(&mut app, 3);
    assert!(on_disk(&w, &id, 3).queued.is_some());
    assert!(app.active_notice().unwrap().starts_with("The one squad allowed at once is at work"), "{:?}", app.active_notice());
    // One finishes: one is still at work, which is the limit now: #3 waits.
    Tickets::open(&w.config, &id).squad_says(1, &State::Done, "", Some("x")).unwrap();
    for _ in 0..5 {
        app.refresh_for_test();
    }
    assert_eq!(app.sq.launched, [1, 2]);
    // Raised again: it goes.
    s.squads_at_once = 3;
    crate::settings::save(&w.config, &s).unwrap();
    app.refresh_until_for_test(20, "#3 to start", |a| a.sq.launched.len() == 3);
}

#[test]
fn only_one_platoon_window_starts_the_line() {
    let (w, mut first, id, _fake) = world("sqq-two", 2, 1);
    hand(&mut first, 1);
    hand(&mut first, 2);
    // `first` took the line's lock on its first refresh. A second window on the same files never starts anything
    // from the line while it is open; once it closes, the second takes over.
    let mut second = w.app();
    open_project(&mut second);
    Tickets::open(&w.config, &id).squad_says(1, &State::Waiting, "Asks: ?", None).unwrap();
    for _ in 0..5 {
        second.refresh_for_test();
    }
    assert!(second.sq.launched.is_empty() && on_disk(&w, &id, 2).queued.is_some());
    drop(first);
    second.refresh_until_for_test(20, "the second window to start #2", |a| a.sq.launched == [2]);
    assert!(on_disk(&w, &id, 2).queued.is_none());
}

#[test]
fn a_running_leader_holds_its_place_long_after_it_started_and_one_that_ended_doesnt() {
    let (w, mut app, id, _fake) = world("sqq-running", 2, 1);
    // #1's leader started long ago, and is still Working.
    let mut t = Tickets::open(&w.config, &id);
    let lead = Leader { squad: "summarize".into(), session: "lead-old".into(), started: 1, names: AgentNames::SquadRole, snapshot: None };
    t.start_leader(1, &lead, &[]).unwrap();
    app.refresh_for_test();
    assert_eq!(app.squads_at_work(), 0, "Working, but its session isn't running: no place held");
    hand(&mut app, 2);
    assert_eq!(app.sq.launched, [2], "so #2 started at once");
    Tickets::open(&w.config, &id).squad_says(2, &State::Done, "", Some("x")).unwrap();
    // Now #1's session runs (registered by Claude Code, as if resumed in a terminal): it holds the place.
    let mut w = w;
    w.live("lead-old", "/tmp", 1, "busy");
    app.refresh_for_test();
    assert_eq!(app.squads_at_work(), 1);
}

// ---- tokens and the members' runs --------------------------------------------------------------------------------

/// A leader transcript for session `id` as a real one is shaped: its agent, one answer of its own, runs of the
/// planner (finished) and the builder (sent back once: two runs, the second still out), and the members' own files.
fn leader_transcript(w: &World, id: &str, leader_tokens: u64) {
    let answer = |m: &str, n: u64| {
        json!({"type":"assistant","message":{"id":m,"usage":{"input_tokens":n,"output_tokens":0,"cache_read_input_tokens":0}}}).to_string()
    };
    let call = |m: &str, c: &str, agent: &str, what: &str| {
        json!({"type":"assistant","message":{"id":m,"content":[{"type":"tool_use","id":c,"name":"Agent","input":{"subagent_type":agent,"description":what,"prompt":"go"}}],"usage":{"input_tokens":0,"output_tokens":0}}}).to_string()
    };
    let launched = |c: &str, a: &str| {
        json!({"type":"user","message":{"content":[{"tool_use_id":c,"type":"tool_result","content":"Async agent launched successfully."}]},"toolUseResult":{"status":"async_launched","agentId":a}}).to_string()
    };
    let notice = |c: &str| {
        json!({"type":"user","message":{"content":format!("<task-notification>\n<tool-use-id>{c}</tool-use-id>\n<status>completed</status>\n</task-notification>")}}).to_string()
    };
    // An answer that mostly read the conversation again from the cache: counted apart from the rest.
    let cached = json!({"type":"assistant","message":{"id":format!("{id}-k"),"usage":{"input_tokens":0,"output_tokens":5_000,"cache_read_input_tokens":900_000}}}).to_string();
    let lines = vec![
        json!({"type":"agent-setting","agentSetting":"summarize-leader"}).to_string(),
        answer(&format!("{id}-a"), leader_tokens),
        cached,
        call(&format!("{id}-b"), &format!("{id}-c1"), "summarize-summarizer", "Sum up the thread"),
        launched(&format!("{id}-c1"), &format!("{id}p")),
        notice(&format!("{id}-c1")),
        call(&format!("{id}-d"), &format!("{id}-c2"), "summarize-checker", "Check the summary"),
        launched(&format!("{id}-c2"), &format!("{id}q")),
    ];
    let path = w.transcript_lines(id, &lines);
    let sub = path.with_extension("").join("subagents");
    std::fs::create_dir_all(&sub).unwrap();
    for (agent_id, agent, n) in [(format!("{id}p"), "summarize-summarizer", 1_000_000u64), (format!("{id}q"), "summarize-checker", 200_000)]
    {
        std::fs::write(sub.join(format!("agent-{agent_id}.jsonl")), answer(&format!("{agent_id}-x"), n) + "\n").unwrap();
        std::fs::write(sub.join(format!("agent-{agent_id}.meta.json")), json!({"agentType":agent}).to_string()).unwrap();
    }
}

#[test]
fn the_ticket_screen_shows_tokens_per_agent_over_every_leader_and_what_the_squad_did() {
    let (w, mut app, id, _fake) = world("sqq-tokens", 2, 3);
    let mut t = Tickets::open(&w.config, &id);
    for (k, session) in ["lead-a", "lead-b"].iter().enumerate() {
        if k > 0 {
            t.set_state(1, &State::New).unwrap();
        }
        t.start_leader(
            1,
            &Leader { squad: "summarize".into(), session: session.to_string(), started: 1, names: AgentNames::SquadRole, snapshot: None },
            &[],
        )
        .unwrap();
    }
    leader_transcript(&w, "lead-a", 30_000);
    leader_transcript(&w, "lead-b", 20_000);
    app.refresh_until_for_test(20, "the transcripts to be read", |a| {
        a.ticket(1).and_then(|t| a.ticket_tokens(t)).is_some_and(|t| t.agents.len() == 3)
    });
    let tk = app.ticket(1).map(|t| app.ticket_tokens(t).unwrap()).unwrap();
    let agents: Vec<(&str, u64)> = tk.agents.iter().map(|(n, s)| (n.as_str(), s.total())).collect();
    assert_eq!(
        agents,
        [("summarize-leader", 50_000 + 10_000 + 1_800_000), ("summarize-summarizer", 2_000_000), ("summarize-checker", 400_000)]
    );
    // The pane: the short total on the row that has one (sent and written: 2.46M, no cache reads, which would make
    // it 4.3M), and nothing on the one that hasn't.
    let mut term = Terminal::new(TestBackend::new(130, 40)).unwrap();
    let shown = draw(&mut term, &app);
    let row = |n: &str| shown.lines().find(|l| l.contains(n)).unwrap().to_string();
    assert!(row("#1 Ticket 1").contains("2.5M tokens") && !row("#1 Ticket 1").contains("4.3M"), "{shown}");
    assert!(!row("#2 Ticket 2").contains("tokens"), "{shown}");
    // The screen (its session isn't running: the checker's run never came back, and says so).
    pick(&mut app, 1);
    key(&mut app, KeyCode::Enter);
    let shown = draw(&mut term, &app);
    let f = flat(&shown);
    // The screen: the pane's number, said as what it is, and the cache reads beside it.
    assert!(f.contains("Tokens used 2.5M sent and written, plus 1.8M read again from Claude's cache"), "{f}");
    // (The note about the session sits beside the explanation, so it is checked in pieces.)
    assert!(
        f.contains("2.5M sent to Claude and 10K written by Claude:")
            && f.contains("the number the Tickets list shows. Read again")
            && f.contains("from the cache is the conversation so far"),
        "{f}"
    );
    assert!(!f.contains("in all"), "{f}");
    assert!(f.contains("summarize-leader 60K + 1.8M from cache summarize-summarizer 2M summarize-checker 400K"), "{f}");
    assert!(
        f.contains("What the squad has done Summarize squad [x] summarize-summarizer · Sum up the thread [-] summarize-checker · Check the summary (didn't finish) Summarize squad [x]"),
        "{f}"
    );
    assert!(!f.contains("Now:"), "nothing runs now: {f}");
}

#[test]
fn the_ticket_screen_shows_the_last_eight_runs_and_how_many_came_before() {
    let (w, mut app, id, _fake) = world("sqq-runs", 1, 3);
    let call = |c: &str, what: &str| {
        json!({"type":"assistant","message":{"id":format!("m-{c}"),"content":[{"type":"tool_use","id":c,"name":"Agent","input":{"subagent_type":"summarize-summarizer","description":what,"prompt":"go"}}],"usage":{"input_tokens":1,"output_tokens":1}}}).to_string()
    };
    let done = |c: &str| json!({"type":"user","message":{"content":[{"tool_use_id":c,"type":"tool_result","content":"done"}]}}).to_string();
    let mut lines = vec![json!({"type":"agent-setting","agentSetting":"summarize-leader"}).to_string()];
    for k in 1..=12 {
        lines.push(call(&format!("c{k}"), &format!("Pass {k:02}")));
        lines.push(done(&format!("c{k}")));
    }
    w.transcript_lines("lead-r", &lines);
    let mut t = Tickets::open(&w.config, &id);
    t.start_leader(
        1,
        &Leader { squad: "summarize".into(), session: "lead-r".into(), started: 1, names: AgentNames::SquadRole, snapshot: None },
        &[],
    )
    .unwrap();
    app.refresh_until_for_test(20, "the transcript to be read", |a| a.leader_log("lead-r").is_some_and(|l| l.runs.len() == 12));
    pick(&mut app, 1);
    key(&mut app, KeyCode::Enter);
    let mut term = Terminal::new(TestBackend::new(130, 60)).unwrap();
    let f = flat(&draw(&mut term, &app));
    // (The note beside the ticket about its session sits between the heading and the runs in this flattened text.)
    assert!(f.contains("What the squad has done") && f.contains("and 4 earlier [x] summarize-summarizer · Pass 05"), "{f}");
    for k in 1..=4 {
        assert!(!f.contains(&format!("Pass {k:02}")), "run {k} is folded: {f}");
    }
    for k in 5..=12 {
        assert!(f.contains(&format!("[x] summarize-summarizer · Pass {k:02}")), "run {k}: {f}");
    }
    assert_eq!(f.matches("[x] summarize-summarizer").count(), 8, "{f}");
}

#[test]
fn eight_runs_or_fewer_are_all_shown_with_no_earlier_line() {
    let (w, mut app, id, _fake) = world("sqq-runs8", 1, 3);
    leader_transcript(&w, "lead-e", 5);
    let mut t = Tickets::open(&w.config, &id);
    t.start_leader(
        1,
        &Leader { squad: "summarize".into(), session: "lead-e".into(), started: 1, names: AgentNames::SquadRole, snapshot: None },
        &[],
    )
    .unwrap();
    app.refresh_until_for_test(20, "the transcript to be read", |a| a.leader_log("lead-e").is_some_and(|l| l.runs.len() == 2));
    pick(&mut app, 1);
    key(&mut app, KeyCode::Enter);
    let mut term = Terminal::new(TestBackend::new(130, 60)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("What the squad has done [x] summarize-summarizer · Sum up the thread") && !f.contains("earlier"), "{f}");
}

#[test]
fn while_the_leader_runs_its_open_run_is_running_and_now_names_the_member() {
    let (w, mut app, id, _fake) = world("sqq-now", 1, 3);
    leader_transcript(&w, "lead-n", 5);
    let mut t = Tickets::open(&w.config, &id);
    t.start_leader(
        1,
        &Leader { squad: "summarize".into(), session: "lead-n".into(), started: 1, names: AgentNames::SquadRole, snapshot: None },
        &[],
    )
    .unwrap();
    let mut w = w;
    w.live("lead-n", "/tmp", 1, "busy");
    app.refresh_until_for_test(20, "the transcript to be read", |a| a.leader_log("lead-n").is_some_and(|l| l.runs.len() == 2));
    let runs: Vec<MemberRun> = app.leader_log("lead-n").unwrap().runs.clone();
    assert_eq!(runs[1].ended, None);
    // The ticket with its session running elsewhere (not hosted here): its screen, beside the way to open it.
    app.tk.open = Some(1);
    let mut term = Terminal::new(TestBackend::new(130, 40)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(
        f.contains("[x] summarize-summarizer · Sum up the thread [>] summarize-checker · Check the summary Now: summarize-checker"),
        "{f}"
    );
}
