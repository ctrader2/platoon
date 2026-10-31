//! The Tasks tab at the level of the app: its sections in order with each state in its color, the list of squads `s`
//! opens (the suggested one picked, a broken one refused), `enter` there making the worktree and starting the session
//! with its squad's model and plan mode (nothing is really started in a unit test: what would have been is noted),
//! `c` asking before it cancels, Active and Archived (`a` both ways, a Done task not merged staying), and a task's
//! session on neither of the Sessions tab's lists.

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
        let squad_md = fs::read_to_string(task_dir.join("squad.md")).unwrap();
        assert!(squad_md.contains(&format!("on the branch `task/{n}`")));
        // The squad's steps kept with the task, none marked; the session told to mark each one; who checks, said while
        // the check runs.
        let titles = crate::steps::parse(&crate::squads::get(&w.config, &t.squad).unwrap().body);
        assert_eq!(t.steps, crate::steps::Steps::new(&titles));
        assert!(squad_md.contains(&format!("numbered steps below, 1 to {}", titles.len())) && squad_md.contains(" task step <n> started`"));
        let settings: serde_json::Value = serde_json::from_str(at("--settings").unwrap()).unwrap();
        let checker = if n == 1 { "Opus" } else { "Fable" };
        assert_eq!(
            settings["hooks"]["Stop"][0]["hooks"][0]["statusMessage"],
            format!("{checker} is checking the work against \"Done when\"…").as_str()
        );
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
fn a_tasks_session_is_on_neither_sessions_list_nor_counted_nor_found_by_the_search() {
    let (w, mut app, project, folder) = lab("tasks-off-sessions", &[("Write the guide", "docs", State::NotStarted)]);
    // Its conversation, archived on the Sessions tab before it was the task's: on Archived it isn't either.
    w.transcript("task-sess", &folder, "Task #1: Write the guide about zebras", 1);
    w.transcript("old-task-sess", &folder, "Task #1, the first go: zebras again", 2);
    app.reload_for_test();
    key(&mut app, KeyCode::Char('1'));
    assert_eq!(app.counts, [3, 0]);
    while app.selected_session().map(|s| s.id.as_str()) != Some("old-task-sess") {
        key(&mut app, KeyCode::Down);
    }
    key(&mut app, KeyCode::Char('a'));
    assert_eq!(app.counts, [2, 1]);
    Tickets::open(&w.config, &project).start(1, "docs", "task-sess", None, &[]).unwrap();
    Tickets::open(&w.config, &project)
        .update(1, |t| {
            t.state = State::Done;
            true
        })
        .unwrap();
    // A second task whose session is the archived one.
    let mut store = Tickets::open(&w.config, &project);
    let n = store.add(NewTicket { title: "Again".into(), folder: Some(folder.clone()), ..Default::default() }).unwrap();
    store.start(n, "docs", "old-task-sess", None, &[]).unwrap();
    app.reload_for_test();
    let ids = |app: &App| app.visible().iter().map(|&i| app.sessions[i].id.clone()).collect::<Vec<_>>();
    assert_eq!((ids(&app), app.counts.clone()), (vec!["user-1".to_string()], vec![1, 0]), "Active, and the counts");
    key(&mut app, KeyCode::Right);
    assert!(ids(&app).is_empty(), "nor on Archived: {:?}", ids(&app));
    key(&mut app, KeyCode::Left);
    let mut term = Terminal::new(TestBackend::new(140, 20)).unwrap();
    let shown = draw(&mut term, &app);
    assert!(!shown.contains("zebras") && !shown.contains("#1 "), "{shown}");
    // The search: the quick filter, the words typed in a conversation.
    key(&mut app, KeyCode::Char('/'));
    for c in "zebras".chars() {
        key(&mut app, KeyCode::Char(c));
    }
    assert!(app.visible().is_empty(), "the quick filter: {:?}", ids(&app));
    app.set_search_kind(1);
    assert!(app.search.results.is_empty(), "full text: {:?}", app.search.results.iter().map(|r| &r.id).collect::<Vec<_>>());
    key(&mut app, KeyCode::Esc);
    // Reached through its task, on the Tasks tab.
    key(&mut app, KeyCode::Char('3'));
    assert!(app.board_rows().iter().any(|r| r.1 == 1));
}

#[test]
fn a_session_a_tasks_host_runs_is_a_tasks_before_its_id_is_known() {
    let (_w, mut app, _, _) = lab("tasks-host-session", &[("Write the guide", "docs", State::NotStarted)]);
    let i = app.sessions.iter().position(|s| s.id == "user-1").unwrap();
    assert!(!app.is_task_session(&app.sessions[i]));
    app.sessions[i].procs.push(crate::model::Proc {
        pid: 4_000_000,
        started_at: 1,
        hosted: true,
        background: None,
        stale: false,
        parked: false,
        moved_to: None,
    });
    let host = |task: Option<&str>| crate::host::HostInfo {
        agent: crate::agents::Agent::Codex,
        claude_pid: 4_000_000,
        host_pid: 4_000_001,
        dir: "/tmp".into(),
        args: Vec::new(),
        sock: "/nonexistent/platoon-unit-test.sock".into(),
        task: task.map(String::from),
        version: None,
        kept: None,
    };
    app.hosts.push(host(None));
    assert!(!app.is_task_session(&app.sessions[i]), "an ordinary host");
    app.hosts = vec![host(Some("1:p"))];
    assert!(app.is_task_session(&app.sessions[i]), "the host of task #1");
}

/// Write task `n`'s state and when it last changed straight into its file (the store sets that time itself).
fn finished(w: &World, project: &str, n: u32, state: &str, secs_ago: u64) {
    let path = crate::tickets::path_in(&w.config, project);
    let mut v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let t = v
        .get_mut("tasks")
        .and_then(|l| l.as_array_mut())
        .and_then(|l| l.iter_mut().find(|t| t.get("number").and_then(|x| x.as_u64()) == Some(u64::from(n))))
        .and_then(|t| t.as_object_mut())
        .unwrap();
    t.insert("state".into(), state.into());
    t.insert("updated".into(), (crate::usage::now() - secs_ago).into());
    fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

const DAY: u64 = 86_400;

fn numbers(app: &App) -> Vec<u32> {
    app.board_rows().iter().map(|r| r.1).collect()
}

fn pick(app: &mut App, n: u32) {
    key(app, KeyCode::Home);
    for _ in 0..10 {
        if app.board_task().map(|(_, k)| k) == Some(n) {
            return;
        }
        key(app, KeyCode::Down);
    }
    panic!("#{n} is not on the list: {:?}", numbers(app));
}

#[test]
fn a_moves_a_finished_task_to_archived_and_back_where_it_stays_and_archived_lists_them_the_latest_first() {
    let (w, mut app, project, folder) = lab(
        "tasks-archive",
        &[
            ("Fresh done", "feature", State::NotStarted),
            ("Busy one", "feature", State::Working),
            ("Old done", "feature", State::NotStarted),
            ("Old cancel", "docs", State::NotStarted),
            ("Older cancel", "docs", State::NotStarted),
        ],
    );
    Tickets::open(&w.config, &project).start(1, "feature", "fresh-sess", None, &[]).unwrap();
    w.transcript("fresh-sess", &folder, "Task #1: Fresh done", 1);
    finished(&w, &project, 1, "done", 60);
    finished(&w, &project, 3, "done", 3 * DAY);
    finished(&w, &project, 4, "cancelled", 2 * DAY);
    finished(&w, &project, 5, "cancelled", 5 * DAY);
    app.reload_for_test();
    assert_eq!((numbers(&app), app.board_counts()), (vec![2, 1], [2, 3]), "a day after it finished, a task is on Archived");
    let mut term = Terminal::new(TestBackend::new(140, 30)).unwrap();
    let shown = draw(&mut term, &app);
    assert!(shown.contains("Active  2") && shown.contains("Archived  3"), "the switch, with its counts:\n{shown}");
    // On a narrow terminal the switch has the row under the tabs to itself, and the list starts under it.
    let mut narrow = Terminal::new(TestBackend::new(60, 16)).unwrap();
    let small = draw(&mut narrow, &app);
    let lines: Vec<&str> = small.lines().collect();
    assert!(lines[2].contains("Active  2   Archived  3") && lines[3].contains("Working · 1"), "{small}");
    assert_eq!(fg_of(&narrow, "Working · 1"), app.effective_theme().busy);

    // a on a task that isn't finished: it says why, and it stays.
    pick(&mut app, 2);
    assert!(!draw(&mut term, &app).contains("a archive"), "not offered on a task at work");
    key(&mut app, KeyCode::Char('a'));
    assert_eq!(app.active_notice(), Some("#2 isn't finished: a archives a Done or Cancelled task."));
    assert_eq!(numbers(&app), [2, 1]);
    // a on a finished one: on Archived at once, though it finished a minute ago; the row after it is picked.
    pick(&mut app, 1);
    assert!(draw(&mut term, &app).contains("a archive"));
    key(&mut app, KeyCode::Char('a'));
    assert_eq!(app.active_notice(), Some("Archived #1 \"Fresh done\""));
    assert_eq!((numbers(&app), app.board_counts()), (vec![2], [1, 4]));
    assert_eq!(Tickets::open(&w.config, &project).get(1).unwrap().archiving, crate::tickets::Archiving::Archived);

    // Archived: every finished task, the latest first, with no section heading; a restores.
    key(&mut app, KeyCode::Right);
    assert_eq!(numbers(&app), [1, 4, 3, 5]);
    let shown = draw(&mut term, &app);
    assert!(shown.contains("#5 Older cancel") && !shown.contains("Done · 4") && shown.contains("a restore"), "{shown}");
    // enter on an archived task opens its session, as on Active (continued: nothing really starts in a unit test).
    pick(&mut app, 1);
    key(&mut app, KeyCode::Enter);
    app.refresh_until_for_test(10, "its session continued", |a| a.sq.launched.contains(&1));
    // / narrows Archived too.
    key(&mut app, KeyCode::Char('/'));
    for c in "cancel".chars() {
        key(&mut app, KeyCode::Char(c));
    }
    key(&mut app, KeyCode::Enter);
    assert_eq!(numbers(&app), [4, 5]);
    key(&mut app, KeyCode::Esc);
    assert_eq!(numbers(&app), [1, 4, 3, 5]);
    // Brought back three days after it finished: it stays on Active.
    pick(&mut app, 3);
    key(&mut app, KeyCode::Char('a'));
    assert_eq!(app.active_notice(), Some("Restored #3 \"Old done\""));
    assert_eq!(numbers(&app), [1, 4, 5]);
    app.reload_for_test();
    key(&mut app, KeyCode::Left);
    assert_eq!(numbers(&app), [2, 3]);
    // A click on a chip of the switch shows that list.
    let shown = draw(&mut term, &app);
    let (row, col) = shown
        .lines()
        .enumerate()
        .find_map(|(y, l)| l.find("Archived  ").map(|b| (y as u16, l[..b].chars().count() as u16)))
        .unwrap_or_else(|| panic!("{shown}"));
    app.on_mouse(ratatui::crossterm::event::MouseEvent {
        kind: ratatui::crossterm::event::MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left),
        column: col + 1,
        row,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(numbers(&app), [1, 4, 5]);
}

#[test]
fn each_list_says_what_it_is_when_it_is_empty() {
    let (w, mut app, project, _) = lab("tasks-archive-empty", &[("Write the guide", "docs", State::NotStarted)]);
    let mut term = Terminal::new(TestBackend::new(140, 30)).unwrap();
    key(&mut app, KeyCode::Right);
    let shown = draw(&mut term, &app);
    assert!(shown.contains(&crate::board::Shelf::Archived.empty(None)), "{shown}");
    key(&mut app, KeyCode::Left);
    finished(&w, &project, 1, "cancelled", 2 * DAY);
    app.reload_for_test();
    let shown = draw(&mut term, &app);
    assert!(shown.contains(crate::board::ALL_ARCHIVED), "{shown}");
}

#[test]
fn with_no_task_yet_the_tasks_tab_says_how_to_make_one_and_that_the_skill_turns_a_conversation_into_tasks() {
    let (w, mut app, _, _) = lab("tasks-none-yet", &[]);
    let mut term = Terminal::new(TestBackend::new(140, 30)).unwrap();
    app.set_providers_for_test(crate::agents::Providers {
        claude: Some("/nonexistent/platoon-unit-test/claude".into()),
        codex: None,
        off: Vec::new(),
    });
    let words = |command: &str| {
        format!("No tasks yet. Press n to write one, or type {command} in a session to turn the work you agreed on there into tasks.")
    };
    // No skill yet: the one setup adds.
    let shown = draw(&mut term, &app);
    assert!(shown.contains(&words("/hand-to-squad")), "{shown}");
    // Only copies under old names (platoon updated, setup not run since): the newest name that works.
    for old in ["/to-tickets", "/to-tasks"] {
        let at = crate::skill::old_path(&w.root, old);
        fs::create_dir_all(at.parent().unwrap()).unwrap();
        fs::write(&at, "---\nname: old\n---\nMake tasks.\n").unwrap();
        let shown = draw(&mut term, &app);
        assert!(shown.contains(&words(old)), "{old}: {shown}");
    }
    // Setup run: /hand-to-squad.
    crate::skill::install(&w.root, "/usr/bin/platoon");
    let shown = draw(&mut term, &app);
    assert!(shown.contains(&words("/hand-to-squad")), "{shown}");
    // Without Claude Code there is no skill to type.
    app.set_providers_for_test(crate::agents::Providers::default());
    let shown = draw(&mut term, &app);
    assert!(shown.contains("No tasks yet. Press n to write one.") && !shown.contains("/hand-to") && !shown.contains("/to-ta"), "{shown}");
}

#[test]
fn an_old_done_task_stays_on_active_while_its_work_is_not_merged_and_a_can_archive_it_anyway() {
    let (w, mut app, project, folder) =
        lab("tasks-old-not-merged", &[("Order mails", "feature", State::NotStarted), ("Text mails", "feature", State::NotStarted)]);
    let shop = Path::new(&folder);
    for n in [1, 2] {
        git(shop, &["switch", "-q", "-c", &format!("task/{n}"), "main"]);
        fs::write(shop.join(format!("f{n}")), "x").unwrap();
        git(shop, &["add", "."]);
        git(shop, &["commit", "-q", "-m", &format!("work {n}")]);
        let wt = w.base.join(format!("repos/shop-worktrees/task-{n}")).to_string_lossy().into_owned();
        Tickets::open(&w.config, &project)
            .update(n, |t| {
                t.worktree =
                    Some(crate::tickets::Worktree { path: wt, branch: format!("task/{n}"), removed: Some(1), ..Default::default() });
                true
            })
            .unwrap();
        // #2 finished a little after #1.
        finished(&w, &project, n, "done", 3 * DAY - u64::from(n) * 10);
    }
    git(shop, &["switch", "-q", "main"]);
    app.reload_for_test();
    // Not read yet (platoon just started): where a merged one would be, never on Active for a moment.
    assert_eq!(app.board_counts(), [0, 2]);
    refreshed_until(&mut app, "the branches to be read", |a| a.not_merged(&project, 1).is_some() && a.not_merged(&project, 2).is_some());
    assert_eq!((numbers(&app), app.board_counts()), (vec![2, 1], [2, 0]), "not merged: on Active, three days on");
    let mut term = Terminal::new(TestBackend::new(150, 40)).unwrap();
    assert!(draw(&mut term, &app).contains("Feature · Done · not merged · on task/1"));
    // a archives one all the same; on Archived it still says it isn't merged.
    pick(&mut app, 2);
    key(&mut app, KeyCode::Char('a'));
    assert_eq!(numbers(&app), [1]);
    key(&mut app, KeyCode::Right);
    assert_eq!(numbers(&app), [2]);
    assert!(draw(&mut term, &app).contains("Feature · Done · not merged · on task/2"));
    // Merged: the other moves to Archived by itself.
    key(&mut app, KeyCode::Left);
    git(shop, &["merge", "-q", "--no-ff", "-m", "merge task 1", "task/1"]);
    app.read_merged_soon_for_test();
    refreshed_until(&mut app, "the merge to be seen", |a| a.not_merged(&project, 1).is_none());
    assert_eq!(app.board_counts(), [0, 2]);
}

#[test]
fn sessions_platoon_starts_carry_no_offer_and_the_settings_screen_has_none() {
    let (w, mut app, _, folder) = lab("tasks-no-offer", &[("Write the guide", "docs", State::NotStarted)]);
    // The settings screen's Sessions tab: no offer setting (the offer is the /hand-to-squad skill now).
    key(&mut app, KeyCode::Char('1'));
    key(&mut app, KeyCode::Char('s'));
    for _ in 0..crate::settings::TABS.len() {
        if app.settings_tab == crate::settings::TAB_SESSIONS {
            break;
        }
        key(&mut app, KeyCode::Tab);
    }
    assert_eq!(app.settings_tab, crate::settings::TAB_SESSIONS);
    let mut term = Terminal::new(TestBackend::new(140, 30)).unwrap();
    let shown = draw(&mut term, &app);
    assert!(shown.contains("Sessions per call"), "{shown}");
    for gone in ["Offer tasks", "Making tasks", "Ask once"] {
        assert!(!shown.contains(gone), "{gone:?}:\n{shown}");
    }
    key(&mut app, KeyCode::Esc);
    // A session started by platoon gets the agent's own arguments and no offer: no `--settings`, no variable.
    w.transcript("user-1", &folder, "Plan the docs", 1);
    app.reload_for_test();
    key(&mut app, KeyCode::Enter);
    let l = app.launched.last().expect("enter on its row resumes it");
    assert_eq!(l.args, ["--resume".to_string(), "user-1".to_string()]);
    assert!(l.env.iter().all(|(k, _)| k != "PLATOON_OFFER_TASKS"), "{:?}", l.env);
}

/// Write the check's answer (and whether it runs now) on task `n`.
fn checked(w: &World, project: &str, n: u32, v: Option<crate::tickets::Verdict>, checking: bool) {
    Tickets::open(&w.config, project)
        .update(n, |t| {
            t.verdict = v;
            t.checking = checking.then(|| crate::tickets::Checking { since: crate::usage::now(), model: "fable".into() });
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
    Tickets::open(&w.config, &project).start(2, "feature", "task-sess", None, &[]).unwrap();
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
    // ← and → go to the notes and back, and the task picked stays picked.
    pick(&mut app, 1);
    key(&mut app, KeyCode::Right);
    assert_eq!(app.pane, crate::app::Pane::Notes);
    key(&mut app, KeyCode::Left);
    assert_eq!(app.pane, crate::app::Pane::Sessions);
    assert_eq!(app.project_task_picked(), Some(1));
    // Enter on one that never started: its text, on its own screen; esc comes back to the project's screen, on its row.
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

/// The foreground color of the first cell of `word` inside the first `row` on the screen last drawn.
fn fg_in(term: &Terminal<TestBackend>, row: &str, word: &str) -> Color {
    let buf = term.backend().buffer();
    for y in 0..buf.area.height {
        let cells: Vec<String> = (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
        let line = cells.concat();
        let Some(at) = line.find(row) else { continue };
        let x = line[..at + row.find(word).unwrap()].chars().count();
        return buf[(x as u16, y)].fg;
    }
    panic!("{row:?} isn't on the screen");
}

/// Task `n` of `project` at work on its squad's steps: 1 and 2 done (2 with what was done), 3 skipped, 4 the one it is on.
fn on_step_four(w: &World, project: &str, n: u32, session: &str) {
    use crate::steps::Report;
    let mut store = Tickets::open(&w.config, project);
    let titles = crate::steps::parse(&crate::squads::get(&w.config, "bug-fix").unwrap().body);
    store.start(n, "bug-fix", session, None, &titles).unwrap();
    for (step, report) in [
        (1, Report::Done(String::new())),
        (2, Report::Done("the basket forgets the last item".into())),
        (3, Report::Skipped("the fix is one line".into())),
        (4, Report::Started),
    ] {
        store.mark_step(n, step, &report).unwrap();
    }
}

#[test]
fn a_working_tasks_row_names_its_step_and_its_details_check_off_the_steps_with_its_helpers_and_the_checks_timer() {
    use crate::tickets::{Checking, Finding, Verdict};
    let (w, mut app, project, _) = lab("tasks-steps", &[("Fix the basket", "bug-fix", State::NotStarted)]);
    on_step_four(&w, &project, 1, "s-steps");
    // Its session's transcript, with two helpers as Claude Code writes them down.
    let transcript = w.transcript("s-steps", "/w", "Task #1: Fix the basket", 1);
    let subagents = transcript.with_extension("").join("subagents");
    fs::create_dir_all(&subagents).unwrap();
    for (id, description, model, finished) in
        [("a1", "Reviewer A on the fix", "opus", true), ("b2", "Reviewer B on the fix", "fable", false)]
    {
        let meta = serde_json::json!({"agentType": "general-purpose", "description": description, "model": model, "finished": finished});
        fs::write(subagents.join(format!("agent-{id}.meta.json")), meta.to_string()).unwrap();
    }
    app.reload_for_test();
    app.refresh_until_for_test(10, "the helpers read", |a| a.session_log("s-steps").is_some_and(|l| l.helpers.len() == 2));
    let mut term = Terminal::new(TestBackend::new(150, 44)).unwrap();
    let shown = draw(&mut term, &app);
    let t = app.effective_theme().clone();
    // The row: its squad, the step it is on and that step's title.
    assert!(shown.contains("Bug fix · step 4/8 · Fix it where it first goes wrong, not where it shows"), "{shown}");
    assert_eq!(fg_in(&term, "Bug fix · step 4/8", "step 4/8"), t.subtext);
    // The details: the helpers, then the steps checked off, each mark in its color, what was said under its title.
    for want in [
        "Helpers Reviewers: Opus ✓ · Fable …",
        "Steps · 3 of 8 marked",
        "✓ 1 Reproduce it yourself, on the surface where it was reported",
        "✓ 2 Find the cause by elimination",
        "    the basket forgets the last item",
        "– 3 Plan the fix, present it and wait for the user's yes",
        "    skipped: the fix is one line",
        "▸ 4 Fix it where it first goes wrong, not where it shows",
        "○ 5 Commit the failing test first",
    ] {
        assert!(shown.contains(want), "{want:?}:\n{shown}");
    }
    assert_eq!(fg_of(&term, "✓ 1 Reproduce"), t.ok);
    assert_eq!(fg_of(&term, "– 3 Plan the fix"), t.dim);
    assert_eq!(fg_of(&term, "▸ 4 Fix it"), t.busy);
    assert_eq!(fg_of(&term, "○ 5 Commit"), t.dim);
    assert_eq!(fg_in(&term, "○ 5 Commit", "Commit"), t.subtext, "a step not reached is quieter");
    assert_eq!(fg_in(&term, "▸ 4 Fix it", "Fix it"), t.text);
    assert_eq!(fg_in(&term, "    the basket forgets", "the basket"), t.subtext);
    // The check running: who checks, and for how long, on the row and in the details; the board keeps the clock going.
    let set = |f: &dyn Fn(&mut crate::tickets::Ticket)| {
        Tickets::open(&w.config, &project)
            .update(1, |x| {
                f(x);
                true
            })
            .unwrap();
    };
    set(&|x| x.checking = Some(Checking { since: crate::usage::now() - 23, model: "fable".into() }));
    app.reload_for_test();
    assert!(app.board_clock_running());
    let shown = draw(&mut term, &app);
    assert!(shown.contains("Verdict Fable is checking the work… 0:2"), "{shown}");
    assert_eq!(fg_of(&term, "Fable is checking the work…"), t.busy);
    // The check sent it back: said with what it found while the session works on it.
    let found = Verdict {
        at: crate::usage::now(),
        model: "fable".into(),
        findings: vec![Finding { file: "src/basket.rs".into(), line: 3, problem: "The last item is still dropped.".into() }],
        ..Default::default()
    };
    set(&|x| {
        x.checking = None;
        x.checks_refused = 1;
        x.verdict = Some(found.clone());
    });
    app.reload_for_test();
    assert!(!app.board_clock_running());
    let shown = draw(&mut term, &app);
    assert!(
        shown.contains("Fable's check sent it back: 1 problem: src/basket.rs:3: The last") && shown.contains("item is still dropped."),
        "{shown}"
    );
    assert_eq!(fg_of(&term, "Fable's check sent it back"), t.warn);
    // Sent back to mark its steps: which ones, and the heading counts them.
    set(&|x| {
        x.checks_refused = 0;
        x.verdict = None;
        x.steps.sent_back = true;
    });
    app.reload_for_test();
    let shown = draw(&mut term, &app);
    assert!(shown.contains("Sent back to mark steps 4, 5, 6, 7 and 8 done or skipped."), "{shown}");
    assert_eq!(fg_of(&term, "Sent back to mark steps"), t.warn);
    assert!(shown.contains("Steps · 5 steps not marked"), "{shown}");
    assert_eq!(fg_of(&term, "5 steps not marked"), t.warn);
    // Narrow: no details, and the row's step title is cut after a word.
    let mut narrow = Terminal::new(TestBackend::new(60, 30)).unwrap();
    let shown = draw(&mut narrow, &app);
    let row = shown.lines().find(|l| l.contains("step 4/8")).unwrap_or_else(|| panic!("{shown}"));
    assert_eq!(row.trim(), "Bug fix · step 4/8 · Fix it where it first goes…", "{row:?}");
    assert!(!shown.contains("Steps ·"), "no details at this width:\n{shown}");
}

#[test]
fn a_task_whose_squad_has_no_numbered_steps_shows_no_steps() {
    let (w, mut app, project, _) = lab("tasks-no-steps", &[("Write the guide", "docs", State::NotStarted)]);
    Tickets::open(&w.config, &project).start(1, "docs", "s-plain", None, &[]).unwrap();
    app.reload_for_test();
    let mut term = Terminal::new(TestBackend::new(150, 40)).unwrap();
    let shown = draw(&mut term, &app);
    assert!(shown.contains("#1 Write the guide") && !shown.contains("Steps ·") && !shown.contains("step 1/"), "{shown}");
}
