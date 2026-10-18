//! The Tasks tab at the level of the app: its sections in order with each state in its color, the list of squads `s`
//! opens (the suggested one picked, a broken one refused), `enter` there making the worktree and starting the session
//! with its squad's model and plan mode (nothing is really started in a unit test: what would have been is noted),
//! `c` asking before it cancels, and a task's session marked with its number on the Sessions tab.

use crate::app::{App, Confirm, Mode};
use crate::testkit::World;
use crate::tickets::{Need, NewTicket, State, Tickets};
use crate::ui;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    style::Color,
};
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

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

/// The foreground color of the first cell of `needle` on the screen last drawn.
fn fg_of(term: &Terminal<TestBackend>, needle: &str) -> Color {
    fg_of_nth(term, needle, 0)
}

/// The same, at the `nth` line (from 0) that has `needle`.
fn fg_of_nth(term: &Terminal<TestBackend>, needle: &str, mut nth: usize) -> Color {
    let buf = term.backend().buffer();
    for y in 0..buf.area.height {
        let row: Vec<String> = (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
        let line = row.concat();
        if let Some(at) = line.find(needle) {
            if nth > 0 {
                nth -= 1;
                continue;
            }
            // Cells, not bytes: count the symbols before the match.
            let mut bytes = 0;
            let x = row.iter().position(|s| {
                let here = bytes == at;
                bytes += s.len();
                here
            });
            return buf[(x.unwrap() as u16, y)].fg;
        }
    }
    panic!("{needle:?} isn't on the screen");
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// A project "Web shop" whose folder is the temp repository `shop`, and its tasks: `(title, squad, state)` each.
fn lab(name: &str, tasks: &[(&str, &str, State)]) -> (World, App, String, String) {
    let w = World::new(name);
    w.transcript("user-1", "/tmp", "make a basket page", 10);
    let shop = w.base.join("repos/shop");
    fs::create_dir_all(&shop).unwrap();
    fs::write(shop.join("README"), "hi\n").unwrap();
    git(&shop, &["init", "-q"]);
    git(&shop, &["add", "."]);
    git(&shop, &["commit", "-q", "-m", "first"]);
    let mut app = w.app();
    let project = app.projects.create("Web shop").unwrap();
    app.projects.assign("user-1", Some(&project)).unwrap();
    let folder = shop.to_string_lossy().into_owned();
    let mut store = Tickets::open(&w.config, &project);
    for (title, squad, state) in tasks {
        let n = store
            .add(NewTicket { title: title.to_string(), folder: Some(folder.clone()), squad: squad.to_string(), ..Default::default() })
            .unwrap();
        let state = state.clone();
        store
            .update(n, |t| {
                t.state = state;
                true
            })
            .unwrap();
    }
    app.reload_for_test();
    key(&mut app, KeyCode::Char('2'));
    assert!(app.board_showing());
    (w, app, project, folder)
}

#[test]
fn the_board_shows_its_sections_in_order_and_each_state_in_its_color() {
    let (_w, app, _, _) = lab(
        "tasks-sections",
        &[
            ("Write the guide", "docs", State::NotStarted),
            ("Fix the basket", "bug-fix", State::Working),
            ("Pick a page", "feature", State::NeedsYou(Need::Question("Which page first?".into()))),
            ("Old work", "feature", State::Done),
        ],
    );
    let mut term = Terminal::new(TestBackend::new(140, 30)).unwrap();
    let shown = draw(&mut term, &app);
    let at = |s: &str| shown.find(s).unwrap_or_else(|| panic!("{s:?} missing:\n{shown}"));
    assert!(
        at("Needs you · 1") < at("Working · 1") && at("Working · 1") < at("Not started · 1") && at("Not started · 1") < at("Done · 1"),
        "{shown}"
    );
    // Each row's second line: what it needs, its squad, its suggestion.
    for line in
        ["#3 Pick a page", "Feature · asks: Which page first?", "#2 Fix the basket", "#1 Write the guide", "Suggested: Docs · s starts it"]
    {
        assert!(shown.contains(line), "{line:?}:\n{shown}");
    }
    let t = app.effective_theme().clone();
    assert_eq!(fg_of(&term, "Needs you · 1"), t.warn);
    assert_eq!(fg_of(&term, "Working · 1"), t.busy);
    assert_eq!(fg_of(&term, "Not started · 1"), t.text);
    assert_eq!(fg_of(&term, "Done · 1"), t.dim);
    // The picked task (the first, which needs the user) in the details beside the list, its state in the warning color.
    assert!(shown.contains("Squad   Feature"), "{shown}");
    assert!(!shown.contains("Tests "), "platoon runs no tests, so the details have no Tests line: {shown}");
    assert_eq!(fg_of(&term, "Needs you · a question"), t.warn);
}

#[test]
fn s_lists_every_squad_with_the_suggested_one_picked_and_a_broken_one_cannot_start() {
    let (w, mut app, _, _) = lab("tasks-pick", &[("Write the guide", "docs", State::NotStarted), ("Something", "", State::NotStarted)]);
    // A squad of the user's own with no description: listed, but it can't be used.
    fs::create_dir_all(w.config.join("squads")).unwrap();
    fs::write(w.config.join("squads/mine.md"), "---\nname: Mine\n---\n\n1. Do it.\n").unwrap();
    let mut term = Terminal::new(TestBackend::new(140, 30)).unwrap();
    // Each task found by its number, wherever the board puts it.
    let pick_for = |app: &mut App, n: u32| {
        while app.board_task().map(|(_, k)| k) != Some(n) {
            key(app, KeyCode::Down);
        }
        key(app, KeyCode::Char('s'));
        assert!(matches!(app.mode, Mode::SquadPick), "{n}");
    };
    pick_for(&mut app, 1);
    let shown = draw(&mut term, &app);
    assert!(shown.contains("Start #1 with a squad") && shown.contains("enter start"), "{shown}");
    for name in ["Feature", "Bug fix", "Refactor", "Explain", "Review", "Docs", "Mine"] {
        assert!(shown.contains(name), "{name}:\n{shown}");
    }
    assert_eq!(app.sq.pick[app.sq.pick_selected].id, "docs", "its suggestion is picked");
    assert!(shown.contains("needs fixing"), "{shown}");
    // The broken one: enter says why and starts nothing.
    let mine = app.sq.pick.iter().position(|s| s.id == "mine").unwrap();
    app.sq.pick_selected = mine;
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::SquadPick));
    assert!(app.active_notice().is_some_and(|n| n.starts_with("The Mine squad can't be used: ")), "{:?}", app.active_notice());
    assert!(app.sq.launched.is_empty());
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List));
    // No suggestion: Feature.
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Up);
    pick_for(&mut app, 2);
    assert_eq!(app.sq.pick[app.sq.pick_selected].id, "feature");
}

#[test]
fn enter_on_a_squad_makes_the_worktree_and_starts_the_session_with_the_squads_model_and_mode() {
    let (w, mut app, project, folder) =
        lab("tasks-start", &[("Write the guide", "docs", State::NotStarted), ("Add a basket", "feature", State::NotStarted)]);
    for n in [1, 2] {
        while app.board_task().map(|(_, k)| k) != Some(n) {
            key(&mut app, KeyCode::Down);
        }
        key(&mut app, KeyCode::Char('s'));
        key(&mut app, KeyCode::Enter);
        app.refresh_until_for_test(10, "the session started", |a| a.sq.launched.contains(&n));
    }
    let store = Tickets::open(&w.config, &project);
    for (n, model, mode) in [(1, "sonnet", "acceptEdits"), (2, "opus", "plan")] {
        let t = store.get(n).unwrap();
        assert_eq!(t.state, State::Working);
        let wt = t.worktree.clone().unwrap();
        assert_eq!(wt.branch, format!("task/{n}"));
        assert!(Path::new(&wt.path).join("README").is_file());
        let (dir, args) = &app.sq.launched_with[app.sq.launched.iter().position(|k| *k == n).unwrap()];
        assert_eq!(dir, &wt.path, "it runs in the worktree, not in {folder}");
        let at = |flag: &str| args.iter().position(|a| a == flag).map(|i| args[i + 1].as_str());
        assert_eq!((at("--model"), at("--permission-mode")), (Some(model), Some(mode)), "{args:?}");
        assert_eq!(at("--session-id"), t.session.as_deref());
        let task_dir = crate::tickets::task_dir(&w.config, &project, n);
        assert!(fs::read_to_string(task_dir.join("squad.md")).unwrap().contains(&format!("on the branch `task/{n}`")));
        // The session is in the task's project.
        assert_eq!(app.projects.project_id_of(t.session.as_deref().unwrap()), Some(project.as_str()));
    }
}

#[test]
fn c_asks_before_it_cancels() {
    let (w, mut app, project, _) = lab("tasks-cancel", &[("Write the guide", "docs", State::NotStarted)]);
    key(&mut app, KeyCode::Char('c'));
    assert!(matches!(app.mode, Mode::Confirm(Confirm::CancelTicket { number: 1, .. })));
    key(&mut app, KeyCode::Char('n'));
    assert_eq!(Tickets::open(&w.config, &project).get(1).unwrap().state, State::NotStarted);
    key(&mut app, KeyCode::Char('c'));
    key(&mut app, KeyCode::Char('y'));
    assert_eq!(Tickets::open(&w.config, &project).get(1).unwrap().state, State::Cancelled);
}

#[test]
fn a_tasks_session_is_marked_with_its_number_on_the_sessions_tab() {
    let (w, mut app, project, folder) = lab("tasks-mark", &[("Write the guide", "docs", State::NotStarted)]);
    Tickets::open(&w.config, &project).start(1, "docs", "task-sess", None).unwrap();
    w.transcript("task-sess", &folder, "Task #1: Write the guide", 1);
    app.reload_for_test();
    key(&mut app, KeyCode::Char('1'));
    let mut term = Terminal::new(TestBackend::new(140, 20)).unwrap();
    let shown = draw(&mut term, &app);
    assert!(shown.contains("#1 shop"), "{shown}");
    assert_eq!(shown.matches("#1 ").count(), 1, "only the task's session: {shown}");
}

#[test]
fn sessions_platoon_starts_offer_to_make_tasks_but_a_tasks_own_session_does_not() {
    let (w, mut app, project, folder) = lab("tasks-offer", &[("Write the guide", "docs", State::NotStarted)]);
    Tickets::open(&w.config, &project).start(1, "docs", "task-sess", None).unwrap();
    w.transcript("task-sess", &folder, "Task #1: Write the guide", 1);
    app.reload_for_test();
    assert!(app.offers_tasks(&[]), "a new session");
    assert!(app.offers_tasks(&["a prompt to start with"]), "a new session from a hand-off");
    assert!(app.offers_tasks(&["--resume", "user-1"]), "a resumed one");
    assert!(!app.offers_tasks(&["--resume", "task-sess"]), "a task's own session");
    assert!(!app.offers_tasks(&["attach", "abcd1234"]), "claude attach");
}

/// Write the check's answer (and whether it runs now) on task `n`.
fn checked(w: &World, project: &str, n: u32, v: Option<crate::tickets::Verdict>, checking: bool) {
    Tickets::open(&w.config, project)
        .update(n, |t| {
            t.verdict = v;
            t.checking = checking.then(crate::usage::now);
            true
        })
        .unwrap();
}

#[test]
fn the_check_of_the_work_shows_on_the_row_and_in_the_details_with_what_it_found_in_their_colors() {
    use crate::tickets::{BoxCheck, Finding, Verdict};
    let (w, mut app, project, _) = lab(
        "tasks-verdict",
        &[
            ("Text mails", "feature", State::NeedsYou(Need::Check("2 problems: src/mail.rs:2: Only HTML is sent.".into()))),
            ("Being checked", "feature", State::Working),
        ],
    );
    let v = Verdict {
        at: crate::usage::now(),
        model: "fable".into(),
        boxes: vec![
            BoxCheck { text: "The guide says so".into(), met: true },
            BoxCheck { text: "A text-only client gets the order".into(), met: false },
        ],
        findings: vec![Finding { file: "src/mail.rs".into(), line: 2, problem: "Only HTML is sent.".into() }],
        tokens: 12_300,
        ..Default::default()
    };
    checked(&w, &project, 1, Some(v.clone()), false);
    checked(&w, &project, 2, None, true);
    app.reload_for_test();
    let mut term = Terminal::new(TestBackend::new(150, 40)).unwrap();
    let shown = draw(&mut term, &app);
    let t = app.effective_theme().clone();
    // The row: what it needs, in the session's words; the one being checked says so.
    for want in
        ["the check found problems", "Feature · the check found 2 problems: src/mail.rs:2: Only HTML", "Feature · checking the work"]
    {
        assert!(shown.contains(want), "{want:?}:\n{shown}");
    }
    assert_eq!(fg_of(&term, "checking the work"), t.busy);
    // The details: the Verdict line, what it found (file:line first), the boxes ticked or crossed.
    let at = |s: &str| shown.find(s).unwrap_or_else(|| panic!("{s:?} missing:\n{shown}"));
    assert!(at("Folder ") < at("Verdict 2 problems · checked by fable at "), "{shown}");
    assert!(shown.contains("· 12K tokens") && shown.contains("        src/mail.rs:2: Only HTML is sent."), "{shown}");
    assert_eq!(fg_of(&term, "2 problems · checked"), t.warn);
    assert_eq!(fg_of_nth(&term, "src/mail.rs:2: Only", 2), t.text, "the details' finding, after the row and the reason");
    assert_eq!(fg_of(&term, "✓   The guide says so"), t.ok);
    assert_eq!(fg_of(&term, "✗   A text-only client"), t.warn);
    assert_eq!(fg_of(&term, "Needs you · the check found problems"), t.warn);
    // Done: "done" in the color of done. Could not run: why, in the warning color.
    Tickets::open(&w.config, &project)
        .update(1, |x| {
            x.state = State::Done;
            true
        })
        .unwrap();
    checked(&w, &project, 1, Some(Verdict { done: true, findings: vec![], boxes: vec![v.boxes[0].clone()], ..v.clone() }), false);
    checked(&w, &project, 2, Some(Verdict { failed: Some("no answer within 600 seconds".into()), ..v }), false);
    Tickets::open(&w.config, &project)
        .update(2, |x| {
            x.state = State::NeedsYou(Need::CheckFailed("no answer within 600 seconds".into()));
            true
        })
        .unwrap();
    app.reload_for_test();
    let shown = draw(&mut term, &app);
    assert!(shown.contains("Verdict could not run · no answer within 600 seconds"), "{shown}");
    assert!(shown.contains("Feature · the check could not run: no answer within 600 seconds"), "{shown}");
    assert_eq!(fg_of(&term, "could not run · no answer"), t.warn);
    while app.board_task().map(|(_, k)| k) != Some(1) {
        key(&mut app, KeyCode::Down);
    }
    let shown = draw(&mut term, &app);
    assert!(shown.contains("Verdict done · checked by fable"), "{shown}");
    assert_eq!(fg_of(&term, "done · checked by fable"), t.ok);
    assert!(!shown.contains("src/mail.rs:2"), "{shown}");
}
