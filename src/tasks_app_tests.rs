//! The Tasks tab at the level of the app: its sections in order with each state in its color, the list of squads `s`
//! opens (the suggested one picked, a broken one refused), `enter` there making the worktree and starting the session
//! with its squad's model and plan mode (nothing is really started in a unit test: what would have been is noted),
//! `c` asking before it cancels, and a task's session marked with its number on the Sessions tab.

use crate::agents::Start;
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
    key(&mut app, KeyCode::Char('3'));
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
fn a_squad_whose_model_cant_run_a_tasks_session_here_starts_nothing_and_says_why() {
    let (w, mut app, project, _) = lab("tasks-model", &[("Write the guide", "mine", State::NotStarted)]);
    fs::create_dir_all(w.config.join("squads")).unwrap();
    let squad = |model: &str| format!("---\nname: Mine\ndescription: Ours\nmodel: {model}\n---\n\n1. Do it.\n");
    // A model Codex doesn't offer.
    let codex = crate::agents::tests::codex_with_models("tasks-model");
    app.set_providers_for_test(crate::agents::Providers {
        claude: Some("/nonexistent/platoon-unit-test/claude".into()),
        codex: Some(codex),
        off: Vec::new(),
    });
    fs::write(w.config.join("squads/mine.md"), squad("gpt-9")).unwrap();
    key(&mut app, KeyCode::Char('s'));
    app.sq.pick_selected = app.sq.pick.iter().position(|s| s.id == "mine").unwrap();
    key(&mut app, KeyCode::Enter);
    let said = "The Mine squad's model can't be used: Codex doesn't offer GPT-9 on this computer.";
    assert!(app.active_notice().is_some_and(|n| n.starts_with(said)), "{:?}", app.active_notice());
    assert!(app.sq.launched.is_empty() && !app.squads_busy(), "nothing made, nothing started");
    key(&mut app, KeyCode::Esc);
    // Claude Code not installed: its model is said to need it.
    app.set_providers_for_test(crate::agents::Providers::default());
    fs::write(w.config.join("squads/mine.md"), squad("opus")).unwrap();
    key(&mut app, KeyCode::Char('s'));
    app.sq.pick_selected = app.sq.pick.iter().position(|s| s.id == "mine").unwrap();
    key(&mut app, KeyCode::Enter);
    assert_eq!(
        app.active_notice(),
        Some("The Mine squad's model can't be used: Opus needs Claude Code, which isn't installed on this computer.")
    );
    assert_eq!(Tickets::open(&w.config, &project).get(1).unwrap().state, State::NotStarted);
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
    assert!(app.offers_tasks(Start::New(None)), "a new session");
    assert!(app.offers_tasks(Start::New(Some("a prompt to start with"))), "a new session from a hand-off");
    assert!(app.offers_tasks(Start::Resume("user-1")), "a resumed one");
    assert!(!app.offers_tasks(Start::Resume("task-sess")), "a task's own session");
    assert!(!app.offers_tasks(Start::Attach("abcd1234")), "claude attach");
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

/// Refresh until `cond` (git reads the branches on a thread of its own).
fn refreshed_until(app: &mut App, what: &str, cond: impl Fn(&App) -> bool) {
    let start = std::time::Instant::now();
    while !cond(app) {
        assert!(start.elapsed() < std::time::Duration::from_secs(20), "timed out waiting for: {what}");
        std::thread::sleep(std::time::Duration::from_millis(20));
        app.refresh_for_test();
    }
}

#[test]
fn a_done_task_whose_branch_is_not_merged_says_so_until_the_default_branch_has_it() {
    let (w, mut app, project, folder) = lab("tasks-not-merged", &[("Order mails", "feature", State::Done)]);
    let shop = Path::new(&folder);
    git(shop, &["switch", "-q", "-c", "task/1"]);
    for f in ["a", "b"] {
        fs::write(shop.join(f), f).unwrap();
        git(shop, &["add", "."]);
        git(shop, &["commit", "-q", "-m", f]);
    }
    git(shop, &["switch", "-q", "main"]);
    let wt = w.base.join("repos/shop-worktrees/task-1").to_string_lossy().into_owned();
    Tickets::open(&w.config, &project)
        .update(1, |t| {
            t.worktree =
                Some(crate::tickets::Worktree { path: wt.clone(), branch: "task/1".into(), removed: Some(1), ..Default::default() });
            true
        })
        .unwrap();
    app.reload_for_test();
    refreshed_until(&mut app, "the branch to be read", |a| a.not_merged(&project, 1).is_some());
    let mut term = Terminal::new(TestBackend::new(150, 40)).unwrap();
    let shown = draw(&mut term, &app);
    let t = app.effective_theme().clone();
    // The row, and the details: how many commits, on which branch, not in which.
    assert!(shown.contains("Feature · Done · not merged · on task/1"), "{shown}");
    assert_eq!(fg_of_nth(&term, "Done · not merged", 0), t.warn, "the row's");
    assert_eq!(fg_of_nth(&term, "Done · not merged", 1), t.warn, "the details' state");
    assert!(shown.contains("Branch task/1 has 2 commits not in main."), "{shown}");
    assert_eq!(fg_of(&term, "Branch task/1 has"), t.warn);
    // Merged: plain Done again.
    git(shop, &["merge", "-q", "--no-ff", "-m", "merge task 1", "task/1"]);
    app.read_merged_soon_for_test();
    refreshed_until(&mut app, "the merge to be seen", |a| a.not_merged(&project, 1).is_none());
    let shown = draw(&mut term, &app);
    assert!(!shown.contains("not merged") && !shown.contains("commits not in"), "{shown}");
    assert!(shown.contains("Feature · on task/1"), "{shown}");
}

/// The foreground color of `word` on the line that has `row` (after `row`'s own text).
fn fg_after(term: &Terminal<TestBackend>, row: &str, word: &str) -> Color {
    let buf = term.backend().buffer();
    for y in 0..buf.area.height {
        let cells: Vec<String> = (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
        let line = cells.concat();
        let Some(at) = line.find(row) else { continue };
        let x = line[..at + row.len() + line[at + row.len()..].find(word).unwrap()].chars().count();
        return buf[(x as u16, y)].fg;
    }
    panic!("{row:?} isn't on the screen");
}

#[test]
fn a_projects_screen_lists_its_sessions_and_its_tasks_apart_and_enter_on_a_task_opens_it() {
    let (w, mut app, project, folder) = lab(
        "tasks-on-project",
        &[
            ("Write the guide", "docs", State::NotStarted),
            ("Add a basket", "feature", State::NotStarted),
            ("Fix the cart", "bug-fix", State::Done),
            ("Old idea", "feature", State::Cancelled),
            ("Pick a font", "docs", State::NeedsYou(Need::Question("Which one?".into()))),
        ],
    );
    // #2 has a session, in the project: it is listed as its task, not among the sessions.
    Tickets::open(&w.config, &project).start(2, "feature", "task-sess", None).unwrap();
    w.transcript("task-sess", &folder, "Task #2: Add a basket", 1);
    app.projects.assign("task-sess", Some(&project)).unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Enter);
    assert!(app.open_project().is_some());
    assert_eq!(app.visible().iter().map(|&i| app.sessions[i].id.as_str()).collect::<Vec<_>>(), ["user-1"]);
    let mut term = Terminal::new(TestBackend::new(140, 30)).unwrap();
    let shown = draw(&mut term, &app);
    let lines: Vec<&str> = shown.lines().collect();
    let line_of = |s: &str| lines.iter().position(|l| l.contains(s)).unwrap_or_else(|| panic!("{s:?} missing:\n{shown}"));
    // Sessions, then Tasks: the open ones first (the latest change first), Done last, the cancelled one left out.
    let heading = |s: &str| lines.iter().position(|l| l.starts_with(&format!(" │ {s} "))).unwrap_or_else(|| panic!("{s}:\n{shown}"));
    let (sessions, tasks) = (heading("Sessions"), heading("Tasks"));
    assert!(sessions < line_of("tmp") && line_of("tmp") < tasks, "{shown}");
    let done = line_of("#3 Fix the cart");
    for open in ["#2 Add a basket", "#5 Pick a font", "#1 Write the guide"] {
        assert!(tasks < line_of(open) && line_of(open) < done, "{open}\n{shown}");
    }
    assert!(shown.contains("1 active"), "the task's session is counted as its task: {shown}");
    assert!(!shown.contains("Old idea") && !shown.contains("#2 shop"), "{shown}");
    // Each task's state in the words and the colors of the Tasks tab.
    let t = app.theme.clone();
    for (row, word, color) in [
        ("#2 Add a basket", "Working", t.busy),
        ("#5 Pick a font", "Needs you", t.warn),
        ("#1 Write the guide", "Not started", t.text),
        ("#3 Fix the cart", "Done", t.ok),
    ] {
        assert!(lines[line_of(row)].contains(word), "{row}: {word}\n{shown}");
        assert_eq!(fg_after(&term, row, word), color, "{row}: {word}");
    }
    // A session's row says how full its context is, as on the Sessions tab.
    assert!(lines[line_of("tmp")].contains(" 25% "), "{shown}");
    assert_eq!(fg_after(&term, "/tmp", "25%"), t.subtext);

    // The task rows come after the sessions in one list. On one, enter is its only key.
    let pick = |app: &mut App, n: u32| {
        key(app, KeyCode::Home);
        for _ in 0..6 {
            key(app, KeyCode::Down);
            if app.project_task_picked() == Some(n) {
                return;
            }
        }
        panic!("#{n} is not in the list");
    };
    key(&mut app, KeyCode::Down);
    assert!(app.project_task_picked().is_some());
    let shown = draw(&mut term, &app);
    assert!(shown.contains("enter open") && !shown.contains("a archive") && !shown.contains("h hand-off"), "{shown}");
    // Enter on one that never started: its text, on its own screen; esc comes back to the project's screen, on its row.
    pick(&mut app, 1);
    key(&mut app, KeyCode::Enter);
    assert!(app.task_screen_showing() && app.tk.open == Some(1));
    assert!(draw(&mut term, &app).contains("Write the guide"));
    key(&mut app, KeyCode::Esc);
    assert!(app.open_project().is_some() && !app.task_screen_showing());
    assert_eq!(app.project_task_picked(), Some(1));
    // Enter on a started one: its session, continued (nothing is really started in a unit test).
    pick(&mut app, 2);
    key(&mut app, KeyCode::Enter);
    assert!(app.sq.launched.contains(&2), "its session was continued");
    assert!(app.open_project().is_some(), "still on the project's screen");
}
