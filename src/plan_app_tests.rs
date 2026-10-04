//! The plan step at the level of the app (a task handed over as "plans first, then waits for your yes"): its row on the
//! board ("a plan to approve"), the plan beside the board at 120x40 and on the task's own screen at 120x40 and 80x24
//! (drawn from `plan.md` in the task's own folder, with the keys that answer it, and still shown after the go-ahead,
//! under the result once there is one), `y` typing "Go ahead with the plan." into the squad's session and `n` opening
//! the box whose words go as "Change the plan: ...", `n` being a new task on any other row, and the hand-over popup's
//! `space` going round the three levels. Every directory is a temp dir (`World`); no session is ever started; the
//! sockets are listeners this test makes in its own temp dir.

use crate::app::{App, BRow, GO_AHEAD_WITH_PLAN, Mode, PickFor};
use crate::host::HostInfo;
use crate::testkit::World;
use crate::tickets::{self, Autonomy, State, Tickets};
use crate::ui;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    style::{Color, Modifier},
};
use serde_json::{Value, json};
use std::{fs, path::Path, path::PathBuf, thread::JoinHandle};

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

/// What is left of the first line down the screen (a task beside its squad's place), read top to bottom.
fn left(shown: &str) -> String {
    flat(&shown.lines().map(|l| l.split('│').next().unwrap_or("")).collect::<Vec<_>>().join("\n"))
}

/// The board's details: what is right of the line between the list and them, read top to bottom.
fn details(shown: &str) -> String {
    flat(&shown.lines().map(|l| l.split_once('│').map_or("", |(_, d)| d)).collect::<Vec<_>>().join("\n"))
}

/// The cell where `needle` first starts on screen.
fn cell_of(term: &Terminal<TestBackend>, needle: &str) -> ratatui::buffer::Cell {
    let buf = term.backend().buffer();
    for y in 0..buf.area.height {
        let line: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
        if let Some(at) = line.find(needle) {
            return buf[(line[..at].chars().count() as u16, y)].clone();
        }
    }
    panic!("{needle:?} isn't on the screen")
}

fn fg_of(term: &Terminal<TestBackend>, needle: &str) -> Color {
    cell_of(term, needle).fg
}

const PLAN: &str = "1. Add a text part beside the HTML part, from the same order: items, total, delivery date.\n\
2. Send both parts from the order mailer; the HTML part stays as it is.\n\n\
Risk: the delivery date is formatted in the template today; it moves to one helper.\n\n\
Checks: a mail client that shows only text gets the whole order.\n";

fn now() -> u64 {
    crate::usage::now()
}

fn task(n: u32, title: &str, state: &str, extra: Value) -> Value {
    let mut t = json!({"number": n, "title": title, "file": format!("{n:02}-t.md"), "state": state, "folders": ["/tmp"],
        "blocked_by": [], "created": 1, "updated": now() - 300, "autonomy": "plan"});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        t.as_object_mut().unwrap().insert(k, v);
    }
    t
}

fn write(config: &Path, project: &str, tasks: &[Value]) {
    let path = tickets::path_in(config, project);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, json!({"next": 9, "tickets": tasks}).to_string()).unwrap();
    let bodies = tickets::bodies_dir(config, project);
    fs::create_dir_all(&bodies).unwrap();
    for t in tasks {
        fs::write(bodies.join(t["file"].as_str().unwrap()), "## What it should do\n\nOrder emails have a text part.\n").unwrap();
    }
}

/// "Web shop" with #5, whose Coding squad's plan is ready (its `plan.md` written), and #6, a task that can start; the
/// Tasks tab showing, #5 picked. A stand-in editor so nothing real ever opens.
fn plan_world(name: &str) -> (World, App, String) {
    let w = World::new(name);
    w.transcript("user-1", "/tmp", "make a shop", 10);
    let mut app = w.app();
    let ed = w.base.join("stand-in-editor");
    fs::write(&ed, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&ed, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    app.set_editors_for_test(vec![crate::editors::Editor {
        id: "stand-in".into(),
        name: "Stand-in".into(),
        program: ed.to_string_lossy().into_owned(),
        args: Vec::new(),
        terminal: true,
    }]);
    let web = app.projects.create("Web shop").unwrap();
    app.projects.assign("user-1", Some(&web)).unwrap();
    let leader = json!([{"squad": "coding", "session": "L5", "started": now() - 600, "names": "squad-role"}]);
    write(
        &w.config,
        &web,
        &[
            task(
                5,
                "Order emails in plain text too",
                "waiting",
                json!({"reason": "The plan is ready", "plan_ready": true, "leaders": leader}),
            ),
            task(6, "Basket page", "new", json!({"folders": []})),
        ],
    );
    let plan = tickets::plan_file(&w.config, &web, 5);
    fs::create_dir_all(plan.parent().unwrap()).unwrap();
    fs::write(&plan, PLAN).unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Char('2'));
    app.bd.selected = Some(BRow::Task(web.clone(), 5));
    key(&mut app, KeyCode::Char('x'));
    assert_eq!(app.board_task(), Some((web.clone(), 5)));
    app.notice = None;
    (w, app, web)
}

/// A listener on `sock` standing in for the squad's host: what one window types into it, once it closes.
fn host_listening(sock: &PathBuf) -> JoinHandle<String> {
    let listener = std::os::unix::net::UnixListener::bind(sock).unwrap();
    std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut typed = Vec::new();
        while let Ok((kind, payload)) = crate::host::read_frame(&mut s) {
            if kind == crate::host::F_INPUT {
                typed.extend(payload);
            }
        }
        String::from_utf8_lossy(&typed).into_owned()
    })
}

#[test]
fn the_plan_shows_beside_the_board_with_the_keys_that_answer_it() {
    let (_w, app, _web) = plan_world("plan-board");
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let shown = draw(&mut term, &app);
    let f = flat(&shown);
    assert!(f.contains("Needs you · 1") && f.contains("#5 Order emails in plain text too a plan to approve"), "{shown}");
    assert!(f.contains("Coding squad wrote its plan: y goes ahead, n changes it"), "{shown}");
    let d = details(&shown);
    assert!(d.contains("The plan 1. Add a text part beside the HTML part"), "{d}");
    assert!(d.contains("Risk: the delivery date is formatted in the template today"), "{d}");
    // The keys that answer it, each whole (the ones that don't fit beside them are in the footer).
    assert!(d.contains("y go ahead n change the plan What ran"), "{d}");
    // The row's word warns; the heading reads as one; the keys are in the accent color.
    let t = app.effective_theme();
    assert_eq!(fg_of(&term, "a plan to approve"), t.warn);
    let heading = cell_of(&term, "The plan");
    assert!(heading.modifier.contains(Modifier::BOLD) && heading.fg == t.text, "{heading:?}");
    assert_eq!(fg_of(&term, "n change the plan"), t.accent);
    // The footer: the go-ahead and the change, and not "new task" (n is the change here).
    let footer = shown.lines().last().unwrap();
    assert!(footer.contains("y go ahead") && footer.contains("n change the plan") && !footer.contains("new task"), "{footer}");
    // At 80 columns the list has the width: the row and the footer say it.
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let shown = draw(&mut term, &app);
    assert!(flat(&shown).contains("a plan to approve"), "{shown}");
    assert!(shown.lines().last().unwrap().contains("n change the plan"), "{shown}");
}

#[test]
fn the_plan_is_on_the_tasks_own_screen_and_stays_there_after_the_go_ahead_under_the_result() {
    let (w, mut app, web) = plan_world("plan-screen");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.tk.open, Some(5));
    for (wide, high) in [(120, 40), (80, 24)] {
        let mut term = Terminal::new(TestBackend::new(wide, high)).unwrap();
        let shown = draw(&mut term, &app);
        // (Wide, the squad's place is beside the task: only the task's side is read.)
        let f = if wide >= 100 { left(&shown) } else { flat(&shown) };
        assert!(f.contains("Squad says The plan is ready"), "{wide}: {shown}");
        assert!(f.contains("On its own plans first, then waits for your yes"), "{wide}: {shown}");
        assert!(f.contains("The plan 1. Add a text part beside the HTML part"), "{wide}: {shown}");
        assert!(f.contains("y go ahead n change the plan"), "{wide}: {shown}");
        assert_eq!(fg_of(&term, "y go ahead"), app.effective_theme().accent, "{wide}");
        assert!(shown.lines().last().unwrap().contains("n change the plan"), "{wide}: the footer: {shown}");
    }
    // After the go-ahead it stays, without the keys; once there is a result, under it.
    Tickets::open(&w.config, &web).plan_answered(5).unwrap();
    Tickets::open(&w.config, &web).squad_says(5, &State::Done, "", Some("Both parts are sent now.")).unwrap();
    app.reload_for_test();
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let shown = draw(&mut term, &app);
    let f = left(&shown);
    let (result, plan) = (f.find("Result Both parts are sent now.").expect("the result"), f.find("The plan 1. Add").expect("the plan"));
    assert!(result < plan, "the plan under the result: {f}");
    assert!(!f.contains("n change the plan"), "{f}");
}

#[test]
fn y_types_go_ahead_with_the_plan_and_n_sends_the_change_into_the_squads_session() {
    let (w, mut app, web) = plan_world("plan-keys");
    // Not running: y continues the squad beside the task and says what to tell it.
    key(&mut app, KeyCode::Char('y'));
    assert_eq!(app.sq.launched, [5]);
    assert_eq!(app.active_notice(), Some("#5's squad is running again: tell it to go ahead with the plan there."));
    assert!(Tickets::open(&w.config, &web).get(5).unwrap().plan_ready, "nothing was sent, so the plan still waits");
    app.tk.open = None;
    // Running, in a host whose socket this test listens on: n opens the box for the change.
    let sock = w.base.join("lead.sock");
    let got = host_listening(&sock);
    let host = HostInfo { claude_pid: 1, host_pid: 1, dir: "/tmp".into(), args: vec!["--resume".into(), "L5".into()], sock: sock.clone() };
    app.refresh_with(vec![host.clone()], false);
    app.bd.selected = Some(BRow::Task(web.clone(), 5));
    key(&mut app, KeyCode::Char('n'));
    assert!(matches!(app.mode, Mode::Answer), "{:?}", app.active_notice());
    typed(&mut app, "Keep the date where it is.");
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let shown = draw(&mut term, &app);
    let f = flat(&shown);
    assert!(f.contains("Change the Coding squad's plan for #5"), "{shown}");
    assert!(f.contains("What should change in the plan?") && f.contains("❯ Keep the date where it is."), "{shown}");
    assert!(f.contains("it plans again and stops with the new plan"), "{shown}");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.active_notice(), Some("Sent the change to #5's squad: it plans again."));
    assert_eq!(got.join().unwrap(), "Change the plan: Keep the date where it is.\r");
    let t = Tickets::open(&w.config, &web).get(5).cloned().unwrap();
    assert_eq!((&t.state, t.plan_ready, t.reason.as_str()), (&State::Working, false, ""), "Working again: {t:?}");
    // The squad plans again and stops with it: y goes ahead.
    Tickets::open(&w.config, &web).squad_plan_ready(5, crate::squads::PLAN_READY, Some("1. The same, with the date kept.")).unwrap();
    let got = host_listening(&{
        let s = w.base.join("lead2.sock");
        let mut h = host.clone();
        h.sock = s.clone();
        app.refresh_with(vec![h], false);
        s
    });
    app.reload_for_test();
    app.bd.selected = Some(BRow::Task(web.clone(), 5));
    key(&mut app, KeyCode::Char('x'));
    key(&mut app, KeyCode::Char('y'));
    assert_eq!(app.active_notice(), Some("#5's squad goes ahead with the plan."));
    assert_eq!(got.join().unwrap(), format!("{GO_AHEAD_WITH_PLAN}\r"));
    assert_eq!(GO_AHEAD_WITH_PLAN, "Go ahead with the plan.");
    let t = Tickets::open(&w.config, &web).get(5).cloned().unwrap();
    assert_eq!((&t.state, t.plan_ready), (&State::Working, false));
    assert!(Tickets::open(&w.config, &web).plan(5).unwrap().contains("the date kept"), "the plan stays with the task");
    // With nothing waiting, y says so, and n on another row is a new task as ever.
    app.bd.selected = Some(BRow::Task(web.clone(), 5));
    key(&mut app, KeyCode::Char('x'));
    key(&mut app, KeyCode::Char('y'));
    assert!(app.active_notice().unwrap().contains("doesn't need you"), "{:?}", app.active_notice());
    app.bd.selected = Some(BRow::Task(web.clone(), 6));
    key(&mut app, KeyCode::Char('x'));
    key(&mut app, KeyCode::Char('n'));
    assert!(matches!(app.mode, Mode::TaskProject) && app.bd.pick_for == PickFor::NewTask);
}

#[test]
fn the_hand_over_popup_goes_round_the_three_levels_and_the_project_default_starts_it() {
    let (w, mut app, web) = plan_world("plan-popup");
    let mut p = crate::projects::Projects::open(&w.config);
    let mut pj: Value = serde_json::from_str(&fs::read_to_string(p.path()).unwrap()).unwrap();
    pj["projects"][&web]["autonomy_default"] = json!("plan");
    fs::write(p.path(), pj.to_string()).unwrap();
    p.reload_if_changed();
    app.projects.reload_if_changed();
    app.bd.selected = Some(BRow::Task(web.clone(), 6));
    key(&mut app, KeyCode::Char('x'));
    key(&mut app, KeyCode::Char('S'));
    assert!(matches!(app.mode, Mode::SquadPick), "{:?}", app.active_notice());
    assert_eq!(app.sq.form.autonomy, Autonomy::PlanFirst, "the project's default");
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    assert!(flat(&draw(&mut term, &app)).contains("On its own: plans first, then waits for your yes"));
    while !app.hand_on_autonomy() {
        key(&mut app, KeyCode::Down);
    }
    let mut seen = Vec::new();
    for _ in 0..3 {
        key(&mut app, KeyCode::Char(' '));
        seen.push(app.sq.form.autonomy.clone());
    }
    assert_eq!(seen, [Autonomy::Ask, Autonomy::Publish, Autonomy::PlanFirst]);
    key(&mut app, KeyCode::Esc);
}
