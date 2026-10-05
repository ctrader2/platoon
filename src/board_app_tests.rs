//! The Tasks tab at the level of the app: the board across projects (and tasks in no project) at 120x40, 100x30,
//! 80x24 and 60x24 (the details beside the list from 100 columns, the second line never cut in the middle of a word),
//! the details of a task whose squad asks something and of one that wants to publish, the timeline and "what ran"
//! beside the board and on the task's screen, the header's count on every tab, `y` (the answer box typed into the
//! squad's session, the go-ahead, a squad continued when its session isn't running), `h`, `n` with a project and with
//! none (the folder picker first), `m` (and its refusal while the squad runs), `/` and `enter`. Every directory is a
//! temp dir (`World`); no session is ever started (a unit test notes what would have been); the one socket is a
//! listener this test makes in its own temp dir.

use crate::app::{App, BRow, Mode, PickFor, TOP_TASKS};
use crate::host::HostInfo;
use crate::testkit::{World, iso, now_ms};
use crate::tickets::{self, State, Tickets};
use crate::ui;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    style::Color,
};
use serde_json::{Value, json};
use std::{fs, path::Path};

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

/// The board's list (left of the line between the two) and its details (right of it), each read top to bottom.
fn list(shown: &str) -> String {
    flat(&shown.lines().map(|l| l.split('│').next().unwrap_or("")).collect::<Vec<_>>().join("\n"))
}
fn details(shown: &str) -> String {
    flat(&shown.lines().map(|l| l.split_once('│').map_or("", |(_, d)| d)).collect::<Vec<_>>().join("\n"))
}

/// The foreground color of the first cell of `needle` on screen.
fn fg_of(term: &Terminal<TestBackend>, needle: &str) -> Color {
    fg_at(term, needle, false)
}

/// ... of its last place on screen (`last`), or its first.
fn fg_at(term: &Terminal<TestBackend>, needle: &str, last: bool) -> Color {
    let buf = term.backend().buffer();
    let mut rows: Vec<u16> = (0..buf.area.height).collect();
    if last {
        rows.reverse();
    }
    for y in rows {
        let line: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
        if let Some(at) = line.find(needle) {
            return buf[(line[..at].chars().count() as u16, y)].fg;
        }
    }
    panic!("{needle:?} isn't on the screen")
}

fn now() -> u64 {
    crate::usage::now()
}

/// A task as the file holds it, changed `mins_ago` minutes ago, with `extra` keys.
fn task(n: u32, title: &str, state: &str, mins_ago: u64, extra: Value) -> Value {
    let mut t = json!({"number": n, "title": title, "file": format!("{n:02}-t.md"), "state": state, "folders": ["/tmp"],
        "blocked_by": [], "created": 1, "updated": now() - mins_ago * 60});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        t.as_object_mut().unwrap().insert(k, v);
    }
    t
}

fn leader(squad: &str, session: &str, mins_ago: u64) -> Value {
    json!({"squad": squad, "session": session, "started": now() - mins_ago * 60, "names": "squad-role"})
}

/// A project's tasks file (`""`: the tasks in no project), with a body for each.
fn write(config: &Path, project: &str, tasks: &[Value]) {
    let path = tickets::path_in(config, project);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let next = tasks.iter().filter_map(|t| t["number"].as_u64()).max().unwrap_or(0) + 1;
    fs::write(&path, json!({"next": next, "tickets": tasks}).to_string()).unwrap();
    let bodies = tickets::bodies_dir(config, project);
    fs::create_dir_all(&bodies).unwrap();
    for t in tasks {
        fs::write(bodies.join(t["file"].as_str().unwrap()), "## What it should do\n\nIt works.\n").unwrap();
    }
}

// Transcript lines shaped like a squad leader's (see `claude::read_leader`), each written at `at` (ms).
fn at(mut v: Value, ms: u64) -> String {
    v.as_object_mut().unwrap().insert("timestamp".into(), json!(iso(ms)));
    v.to_string()
}
fn call(id: &str, agent: &str, what: &str) -> Value {
    json!({"type":"assistant","message":{"id":format!("m-{id}"),"content":[{"type":"tool_use","id":id,"name":"Agent","input":{"subagent_type":agent,"description":what,"prompt":"go"}}],"usage":{"input_tokens":1000,"output_tokens":100}}})
}
fn launched(id: &str, agent_id: &str) -> Value {
    json!({"type":"user","message":{"content":[{"tool_use_id":id,"type":"tool_result","content":"Async agent launched successfully."}]},"toolUseResult":{"isAsync":true,"status":"async_launched","agentId":agent_id}})
}
fn back(id: &str) -> Value {
    json!({"type":"user","message":{"content":format!("<task-notification>\n<tool-use-id>{id}</tool-use-id>\n<status>completed</status>\n</task-notification>")}})
}
fn bash(id: &str, command: &str) -> Value {
    json!({"type":"assistant","message":{"id":format!("b-{id}"),"content":[{"type":"tool_use","id":id,"name":"Bash","input":{"command":command}}],"usage":{"input_tokens":10,"output_tokens":10}}})
}

/// Two projects, Web shop (WS) and Docs site (DS), and a task in no project, in every section of the board: #6 asks a
/// question, #7 wants to publish, #2 is at work (its leader's transcript has three member runs and a dozen commands),
/// Docs' #4 is at work, #5 can start in both projects (the same number twice), #8 waits for #2, and the task in no
/// project finished two hours ago.
fn board_world(name: &str) -> (World, App, String, String) {
    let mut w = World::new(name);
    w.transcript("user-1", "/tmp", "make a shop", 10);
    let mut app = w.app();
    // A stand-in for the editors (a task's text opens in one), so no real editor can ever start here.
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
    let docs = app.projects.create("Docs site").unwrap();
    app.projects.assign("user-1", Some(&web)).unwrap();
    write(
        &w.config,
        &web,
        &[
            task(
                6,
                "Explain the discount rules in the help pages",
                "waiting",
                11,
                json!({"reason": "Asks: Which page should the rules live on, Help or FAQ? The basket page links to Help today.", "leaders": [leader("docs", "L6", 15)]}),
            ),
            task(
                7,
                "Rate limit the login form",
                "waiting",
                30,
                json!({"reason": "Wants to publish: push task/7 and open a merge request", "publish_wanted": true, "leaders": [leader("coding", "L7", 48)]}),
            ),
            task(
                2,
                "Discount codes at checkout",
                "working",
                2,
                json!({"leaders": [leader("coding", "L2", 40)],
                "worktrees": [{"folder": "/tmp", "path": "/tmp/tmp-worktrees/task-2", "branch": "task/2"}],
                "works_on": [{"folder": "/tmp", "how": "worktree", "lands_on": "main"}]}),
            ),
            task(5, "Order emails in plain text too", "new", 5, json!({})),
            task(8, "Basket page", "new", 4, json!({"blocked_by": [2]})),
        ],
    );
    write(
        &w.config,
        &docs,
        &[
            task(5, "Release notes for 0.4", "new", 3, json!({"folders": []})),
            task(4, "Write the docs page for the config file", "working", 1, json!({"leaders": [leader("docs", "L4", 3)]})),
        ],
    );
    write(&w.config, "", &[task(3, "Compare three config formats", "done", 120, json!({"leaders": [leader("research", "L3", 180)]}))]);
    fs::write(tickets::bodies_dir(&w.config, "").join(tickets::result_file(3)), "TOML reads best.\n").unwrap();
    // #2's leader: the planner and the builder done, the reviewer at work; commands in its own file and its members'.
    let t0 = now_ms() - 40 * 60_000;
    let min = 60_000;
    let mut lines = vec![
        at(json!({"type":"agent-setting","agentSetting":"coding-leader"}), t0),
        at(bash("l1", "git status --short"), t0 + 30_000),
        at(call("c1", "coding-planner", "Plan it"), t0 + min),
        at(launched("c1", "p1"), t0 + min + 1000),
        at(back("c1"), t0 + 5 * min),
        at(call("c2", "coding-builder", "Build it"), t0 + 6 * min),
        at(launched("c2", "b1"), t0 + 6 * min + 1000),
        at(back("c2"), t0 + 20 * min),
        at(call("c3", "coding-reviewer", "Review it"), t0 + 21 * min),
        at(launched("c3", "r1"), t0 + 21 * min + 1000),
    ];
    lines.push(String::new());
    let path = w.transcript_lines("L2", &lines);
    let sub = path.with_extension("").join("subagents");
    fs::create_dir_all(&sub).unwrap();
    let member = |id: &str, call: &str, agent: &str, commands: &[(&str, u64)]| {
        let text: Vec<String> = commands.iter().enumerate().map(|(k, (c, m))| at(bash(&format!("{id}-{k}"), c), t0 + m * 60_000)).collect();
        fs::write(sub.join(format!("agent-{id}.jsonl")), text.join("\n") + "\n").unwrap();
        fs::write(sub.join(format!("agent-{id}.meta.json")), json!({"agentType": agent, "toolUseId": call}).to_string()).unwrap();
    };
    member("p1", "c1", "coding-planner", &[("rg discount src", 2)]);
    let builds: Vec<(String, u64)> = (0..10).map(|k| (format!("cargo test step{k:02}"), 7 + k)).collect();
    let builds: Vec<(&str, u64)> = builds.iter().map(|(c, m)| (c.as_str(), *m)).collect();
    member("b1", "c2", "coding-builder", &builds);
    member("r1", "c3", "coding-reviewer", &[("git diff main", 22)]);
    // It is running (registered by Claude Code, as if resumed elsewhere).
    w.live("L2", "/tmp", 1, "busy");
    app.reload_for_test();
    key(&mut app, KeyCode::Char('2'));
    assert!(app.board_showing());
    app.refresh_until_for_test(20, "the leaders' transcripts to be read", |a| a.leader_log("L2").is_some_and(|l| l.runs.len() == 3));
    app.notice = None;
    (w, app, web, docs)
}

/// Pick a task on the board by project and number.
fn pick(app: &mut App, project: &str, n: u32) {
    app.bd.selected = Some(BRow::Task(project.to_string(), n));
    // (A key the board ignores, so the tasks the keys act on follow the pick.)
    key(app, KeyCode::Char('x'));
    assert_eq!(app.board_task(), Some((project.to_string(), n)));
}

#[test]
fn the_board_lists_every_task_in_play_across_projects_in_its_sections_with_the_details_beside_it() {
    let (_w, app, _web, _docs) = board_world("board-120");
    // (Wide enough for every row's whole second line.)
    let mut term = Terminal::new(TestBackend::new(150, 40)).unwrap();
    let shown = draw(&mut term, &app);
    let f = list(&shown);
    let t = app.effective_theme().clone();
    // The sections in order, each with its count; the one waiting longest first under Needs you.
    let order = ["Needs you · 2", "Working · 2", "Can start · 2", "Waits for · 1", "Finished today · 1"];
    let at: Vec<usize> = order.iter().map(|h| f.find(h).unwrap_or_else(|| panic!("{h}: {shown}"))).collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "{shown}");
    assert!(f.find("#7 Rate limit").unwrap() < f.find("#6 Explain").unwrap(), "the one waiting longest first: {shown}");
    // Each row: the project's badge, the number, the title, and at the right the section's word.
    assert!(f.contains("WS #7 Rate limit the login form a go-ahead"), "{shown}");
    assert!(f.contains("WS #2 Discount codes at checkout"), "{shown}");
    assert!(f.contains("DS #5 Release notes for 0.4 Needs a folder") && f.contains("WS #5 Order emails in plain text too"), "{shown}");
    assert!(f.contains("#3 Compare three config formats 2 h ago"), "no project, no badge: {shown}");
    // Line two: the squad's own words, its steps as marks with the tokens, what to press, what it waits for.
    assert!(f.contains("Coding squad wants to publish: push task/7 and open a merge request"), "{shown}");
    assert!(f.contains("Coding squad · planner ✓ builder ✓ reviewer ● verifier · · 3.6K tokens"), "{shown}");
    assert!(f.contains("New · S hands it to a squad"), "{shown}");
    assert!(f.contains("Waits for #2 Discount codes at checkout"), "{shown}");
    assert!(f.contains("result on the task"), "{shown}");
    // Colors: the kind of need in the warn color, its section too; the marks in theirs.
    assert_eq!(fg_of(&term, "a go-ahead"), t.warn);
    assert_eq!(fg_of(&term, "Needs you · 2"), t.warn);
    assert_eq!(fg_of(&term, "Working · 2"), t.busy);
    assert_eq!(fg_of(&term, "Can start · 2"), t.ok);
    assert_eq!(fg_of(&term, "✓"), t.ok);
    assert_eq!(fg_of(&term, "●  verifier"), t.busy);
    // The details of the picked row (the first: #7) beside the list, after a line.
    assert!(shown.lines().nth(5).unwrap().contains('│'), "{shown}");
    let d = details(&shown);
    assert!(
        d.starts_with(
            "#7 Rate limit the login form in WS Web shop · Coding squad · 48 min Tests none set for this project The squad wants to publish"
        ),
        "{d}"
    );
    assert!(d.contains("y go ahead"), "{d}");
    // At 120 columns the same, the second lines cut after a word where they must be.
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let shown = draw(&mut term, &app);
    assert!(list(&shown).contains("Coding squad wants to publish: push task/7 and open a…"), "{shown}");
    assert!(details(&shown).contains("The squad wants to publish push task/7 and open a merge request"), "{shown}");
    // The header: how many need you, in the warn color; the footer: the keys for the picked row.
    assert!(shown.lines().next().unwrap().contains("● 2 need you"), "{shown}");
    assert_eq!(fg_of(&term, "● 2 need you"), t.warn);
    let footer = shown.lines().last().unwrap();
    assert!(footer.contains("y go ahead") && footer.contains("h later") && footer.contains("? help"), "{footer}");
}

#[test]
fn under_a_hundred_columns_the_list_has_the_width_and_no_line_is_cut_in_the_middle_of_a_word() {
    let (_w, app, _web, _docs) = board_world("board-sizes");
    let seconds: Vec<String> = app.board_groups().iter().flat_map(|g| g.rows.iter().map(|r| r.second_text())).collect();
    for (w, h) in [(100u16, 30u16), (80, 24), (60, 24)] {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let shown = draw(&mut term, &app);
        let wide = w >= 100;
        assert_eq!(
            flat(&shown).contains("What it has done") || flat(&shown).contains("The squad wants to publish"),
            wide,
            "{w}x{h}: {shown}"
        );
        for line in shown.lines() {
            assert!(line.chars().count() <= w as usize, "{line}");
            // A drawn second line is its row's whole text, or a part of it that ends at a word, then "…".
            let text = line.split('│').next().unwrap_or("").trim();
            let start = text.trim_end_matches('…').trim_end();
            let full = seconds
                .iter()
                .find(|s| s.as_str() == text)
                .or_else(|| seconds.iter().find(|s| text.ends_with('…') && s.starts_with(start)));
            let Some(full) = full.filter(|_| !text.is_empty()) else { continue };
            if let Some(cut) = text.strip_suffix('…') {
                let cut = cut.trim_end();
                assert!(full[cut.len()..].starts_with(' '), "{w}x{h}: cut mid-word: {text:?} of {full:?}\n{shown}");
            } else {
                assert_eq!(text, full.trim_end(), "{w}x{h}\n{shown}");
            }
        }
    }
    // At 60 columns the rows still say what they are and what they need.
    let mut term = Terminal::new(TestBackend::new(60, 24)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("Needs you · 2") && f.contains("a go-ahead") && f.contains("#6 "), "{f}");
}

#[test]
fn the_details_say_what_the_squad_asks_or_wants_to_publish_and_the_keys_that_answer() {
    let (_w, mut app, web, _docs) = board_world("board-details");
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let t = app.effective_theme().clone();
    pick(&mut app, &web, 6);
    let shown = draw(&mut term, &app);
    let f = details(&shown);
    assert!(f.contains("#6 Explain the discount rules in the help pages"), "{shown}");
    assert!(f.contains("in WS Web shop · Docs squad · 15 min"), "where it is, who has it, for how long: {shown}");
    assert!(f.contains("The squad asks Which page should the rules live on, Help or FAQ?"), "{shown}");
    assert_eq!(fg_of(&term, "Which page should"), t.warn);
    assert!(f.contains("y answer enter open the session h later"), "{shown}");
    pick(&mut app, &web, 7);
    let f = details(&draw(&mut term, &app));
    assert!(f.contains("The squad wants to publish push task/7 and open a merge request") && f.contains("y go ahead"), "{f}");
    // A task no squad has: what it is, where it works, and S.
    pick(&mut app, &web, 5);
    let f = details(&draw(&mut term, &app));
    assert!(f.contains("Can start Works in tmp") && f.contains("S hand to a squad"), "{f}");
    // The finished one: its result first.
    pick(&mut app, "", 3);
    let f = details(&draw(&mut term, &app));
    assert!(f.contains("No project · Research squad") && f.contains("Result TOML reads best."), "{f}");
}

#[test]
fn the_timeline_and_what_ran_are_beside_the_board_and_on_the_tasks_screen_all_of_it() {
    let (_w, mut app, web, _docs) = board_world("board-timeline");
    let mut term = Terminal::new(TestBackend::new(120, 50)).unwrap();
    let t = app.effective_theme().clone();
    pick(&mut app, &web, 2);
    let shown = draw(&mut term, &app);
    let f = details(&shown);
    // One line per run: its bar, how long it took, done / now; the step not run yet.
    let lane = |name: &str| {
        shown
            .lines()
            .find(|l| l.split('│').nth(1).is_some_and(|d| d.trim_start().starts_with(name)))
            .unwrap_or_else(|| panic!("{name}: {shown}"))
            .to_string()
    };
    assert!(lane("planner").contains("4m 00s  done") && lane("planner").contains('━'), "{shown}");
    assert!(lane("builder").contains("14m 00s  done"), "{shown}");
    assert!(lane("reviewer").trim_end().ends_with("now"), "{shown}");
    assert!(lane("verifier").trim_end().ends_with("not yet"), "{shown}");
    // The bars follow the order they ran in.
    let bar = |name: &str| lane(name).find('━').unwrap();
    assert!(bar("planner") < bar("builder") && bar("builder") < bar("reviewer"), "{shown}");
    assert_eq!(fg_of(&term, "now"), t.busy);
    // What ran: the latest eight, grouped by run, each with its time.
    assert!(f.contains("What ran builder"), "{shown}");
    assert!(f.contains("cargo test step09") && !f.contains("cargo test step01"), "the latest eight only: {shown}");
    assert!(f.contains("reviewer") && f.contains("git diff main"), "{shown}");
    // Its own screen: the same timeline, and everything that ran, platoon's own steps too.
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.tk.open, Some(2));
    assert!(app.task_screen_showing());
    let mut tall = Terminal::new(TestBackend::new(120, 90)).unwrap();
    let f = flat(&draw(&mut tall, &app));
    assert!(f.contains("What the squad has done planner") && f.contains("verifier not yet"), "{f}");
    for want in [
        "made a copy of tmp on task/2",
        "started the Coding squad",
        "git status --short",
        "rg discount src",
        "cargo test step01",
        "cargo test step09",
    ] {
        assert!(f.contains(want), "{want}: {f}");
    }
    // Back to the board, on the same task.
    key(&mut app, KeyCode::Esc);
    assert!(app.tk.open.is_none() && app.board_task() == Some((web.clone(), 2)));
}

#[test]
fn the_header_says_how_many_tasks_need_you_on_every_tab() {
    let (_w, mut app, web, _docs) = board_world("board-header");
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    for tab in ['1', '2', '3', '4'] {
        key(&mut app, KeyCode::Char(tab));
        let header = draw(&mut term, &app).lines().next().unwrap().to_string();
        assert!(header.contains("● 2 need you"), "{tab}: {header}");
    }
    key(&mut app, KeyCode::Char('2'));
    pick(&mut app, &web, 6);
    key(&mut app, KeyCode::Char('h'));
    assert!(draw(&mut term, &app).lines().next().unwrap().contains("● 1 needs you"));
    pick(&mut app, &web, 7);
    key(&mut app, KeyCode::Char('h'));
    assert!(!draw(&mut term, &app).lines().next().unwrap().contains("need"), "nothing when none");
    // Too narrow for it beside the tabs: it goes whole, never cut.
    key(&mut app, KeyCode::Char('h'));
    let mut narrow = Terminal::new(TestBackend::new(60, 24)).unwrap();
    let header = draw(&mut narrow, &app).lines().next().unwrap().to_string();
    assert!(!header.contains("need") && header.contains("4 Briefs"), "{header}");
}

#[test]
fn h_puts_a_task_aside_under_later_until_its_squad_says_something_new() {
    let (w, mut app, web, _docs) = board_world("board-aside");
    pick(&mut app, &web, 6);
    key(&mut app, KeyCode::Char('h'));
    assert!(app.active_notice().unwrap().starts_with("#6 is under Later until its squad says something new."), "{:?}", app.active_notice());
    let sections: Vec<&str> = app.board_groups().iter().map(|g| g.section.heading()).collect();
    assert!(sections.contains(&"Later"), "{sections:?}");
    let later = app.board_groups().into_iter().find(|g| g.section == crate::board::Section::Later).unwrap();
    assert_eq!(later.rows.iter().map(|r| r.number).collect::<Vec<_>>(), [6]);
    // The cursor stayed where it was: on the next task that needs the user.
    assert_eq!(app.board_task(), Some((web.clone(), 7)));
    // The squad says something new: back under Needs you.
    Tickets::open(&w.config, &web).squad_says(6, &State::Waiting, "Asks: And the FAQ?", None).unwrap();
    app.refresh_for_test();
    let needs: Vec<u32> = app.board_groups()[0].rows.iter().map(|r| r.number).collect();
    assert_eq!(needs, [7, 6]);
    // h again on one put aside brings it back; h on a task that doesn't need you says so.
    pick(&mut app, &web, 6);
    key(&mut app, KeyCode::Char('h'));
    pick(&mut app, &web, 6);
    key(&mut app, KeyCode::Char('h'));
    assert_eq!(app.active_notice(), Some("#6 needs you again."));
    pick(&mut app, &web, 2);
    key(&mut app, KeyCode::Char('h'));
    assert!(app.active_notice().unwrap().contains("h puts aside a task under Needs you"), "{:?}", app.active_notice());
}

#[test]
fn y_types_the_answer_into_the_squads_session_gives_the_go_ahead_or_continues_a_stopped_squad() {
    use std::io::Read;
    let (w, mut app, web, _docs) = board_world("board-answer");
    // y on the one that wants to publish: the go-ahead, as p gives it.
    pick(&mut app, &web, 7);
    key(&mut app, KeyCode::Char('y'));
    let t = Tickets::open(&w.config, &web).get(7).cloned().unwrap();
    assert!(t.go_ahead && t.state == State::Working, "{t:?}");
    // y on a question whose squad isn't running: it is continued (beside its task), and the line says where to type.
    pick(&mut app, &web, 6);
    key(&mut app, KeyCode::Char('y'));
    assert_eq!(app.sq.launched, [6]);
    assert_eq!(app.active_notice(), Some("#6's squad is running again: type your answer in its session."));
    assert!(!matches!(app.mode, Mode::Answer));
    app.tk.open = None;
    // Now it runs, in a host whose socket this test listens on: y opens the box.
    let sock = w.base.join("lead.sock");
    let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    let got = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut typed = Vec::new();
        while let Ok((kind, payload)) = crate::host::read_frame(&mut s) {
            if kind == crate::host::F_INPUT {
                typed.extend(payload);
            }
        }
        let mut rest = Vec::new();
        let _ = s.read_to_end(&mut rest);
        String::from_utf8_lossy(&typed).into_owned()
    });
    let host = HostInfo { claude_pid: 1, host_pid: 1, dir: "/tmp".into(), args: vec!["--resume".into(), "L6".into()], sock };
    app.refresh_with(vec![host], false);
    pick(&mut app, &web, 6);
    key(&mut app, KeyCode::Char('y'));
    assert!(matches!(app.mode, Mode::Answer), "{:?}", app.active_notice());
    // The box at 80 columns: its title, the question, what typing does, its keys.
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    typed(&mut app, "Help. Link it from the FAQ.");
    let shown = draw(&mut term, &app);
    let f = flat(&shown);
    assert!(f.contains("Answer the Docs squad on #6"), "{shown}");
    assert!(f.contains("Which page should the rules live on, Help or FAQ?"), "{shown}");
    assert!(f.contains("❯ Help. Link it from the FAQ."), "{shown}");
    assert!(f.contains("Sent to the squad's session as your reply; the task goes back to Working."), "{shown}");
    assert!(f.contains("enter send ctrl+o open the session instead esc cancel"), "{shown}");
    // (The details behind the box say it too, faded: the box's own is the lower one.)
    assert_eq!(fg_at(&term, "Which page should", true), app.effective_theme().warn);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.active_notice(), Some("Answer sent to #6."));
    assert_eq!(got.join().unwrap(), "Help. Link it from the FAQ.\r", "the words, then Enter");
    // The task's state is the squad's prompt hook's to change, not this key's.
    assert_eq!(Tickets::open(&w.config, &web).get(6).unwrap().state, State::Waiting);
    // y where nothing needs you says so.
    pick(&mut app, &web, 5);
    key(&mut app, KeyCode::Char('y'));
    assert_eq!(app.active_notice(), Some("#5 doesn't need you just now: y answers a task under Needs you."));
}

#[test]
fn p_gives_the_go_ahead_and_the_cursor_moves_to_the_next_task_that_needs_you_as_y_does() {
    let (w, mut app, web, _docs) = board_world("board-p");
    // #7 wants to publish (it has waited longest, so it comes first); #6 asks a question.
    pick(&mut app, &web, 7);
    key(&mut app, KeyCode::Char('p'));
    let t = Tickets::open(&w.config, &web).get(7).cloned().unwrap();
    assert!(t.go_ahead && t.state == State::Working, "{t:?}");
    assert_eq!(app.board_task(), Some((web.clone(), 6)), "the cursor stays in place: on the next task that needs you");
    // p on one that wants nothing published says so, and the cursor stays on it.
    key(&mut app, KeyCode::Char('p'));
    assert_eq!(app.active_notice(), Some("#6 isn't waiting to publish anything."));
    assert_eq!(app.board_task(), Some((web.clone(), 6)));
}

#[test]
fn n_makes_a_task_in_a_project_or_in_none_after_its_folder_is_picked() {
    let (w, mut app, web, docs) = board_world("board-new");
    // In no project: the folder picker comes before anything is written.
    key(&mut app, KeyCode::Char('n'));
    assert!(matches!(app.mode, Mode::TaskProject) && app.bd.pick_for == PickFor::NewTask);
    assert_eq!(app.bd.pick, [docs.clone(), web.clone(), String::new()], "the projects by name, then No project");
    key(&mut app, KeyCode::End);
    key(&mut app, KeyCode::Enter);
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    assert!(flat(&draw(&mut term, &app)).contains("New task in no project"));
    typed(&mut app, "Try the new linter");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Picker) && app.picker.new_task.as_deref() == Some("Try the new linter"));
    assert!(flat(&draw(&mut term, &app)).contains("The folder the new task works in"));
    let before = fs::read_to_string(tickets::path_in(&w.config, "")).unwrap();
    // Esc there: nothing is made.
    key(&mut app, KeyCode::Esc);
    assert_eq!(fs::read_to_string(tickets::path_in(&w.config, "")).unwrap(), before);
    assert!(app.active_notice().unwrap().starts_with("No task was made"));
    key(&mut app, KeyCode::Char('n'));
    key(&mut app, KeyCode::End);
    key(&mut app, KeyCode::Enter);
    typed(&mut app, "Try the new linter");
    key(&mut app, KeyCode::Enter);
    let folder = app.picker.results[app.picker.selected].clone();
    key(&mut app, KeyCode::Enter);
    let made = Tickets::open(&w.config, "").get(4).cloned().expect("#4 in no project");
    assert_eq!((made.title.as_str(), made.folders.clone()), ("Try the new linter", vec![folder]));
    assert!(matches!(app.mode, Mode::PickEditor) && app.epick.ticket == Some(4), "then its text, in the editor");
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.board_task(), Some((String::new(), 4)));
    // In a project: its next number, in the folder of its most recent session.
    key(&mut app, KeyCode::Char('n'));
    app.bd.pick_selected = app.bd.pick.iter().position(|p| *p == web).unwrap();
    key(&mut app, KeyCode::Enter);
    assert!(flat(&draw(&mut term, &app)).contains("New task in Web shop"));
    typed(&mut app, "Gift cards");
    key(&mut app, KeyCode::Enter);
    let made = Tickets::open(&w.config, &web).get(9).cloned().expect("#9 in Web shop");
    assert_eq!((made.title.as_str(), made.folders.clone()), ("Gift cards", vec!["/tmp".to_string()]));
    assert_eq!(app.active_notice(), Some("Made task #9 in Web shop"));
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.board_task(), Some((web.clone(), 9)));
}

#[test]
fn m_moves_a_task_to_another_project_with_its_next_number_and_not_while_its_squad_runs() {
    let (w, mut app, web, docs) = board_world("board-move");
    pick(&mut app, &web, 5);
    key(&mut app, KeyCode::Char('m'));
    assert!(matches!(app.mode, Mode::TaskProject) && app.bd.pick_for == PickFor::Move(web.clone(), 5));
    assert_eq!(app.bd.pick, [docs.clone(), String::new()], "the other projects, then No project");
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("Move #5") && f.contains("Where to? It is in Web shop now."), "{f}");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.active_notice(), Some("Moved #5 to Docs site: it is #6 there."));
    let moved = Tickets::open(&w.config, &docs).get(6).cloned().unwrap();
    assert_eq!((moved.title.as_str(), moved.old_number), ("Order emails in plain text too", Some(5)));
    assert!(Tickets::open(&w.config, &web).get(5).is_none());
    assert_eq!(app.board_task(), Some((docs.clone(), 6)), "the board follows it");
    // #2's squad is running: it stays where it is, and the line says why.
    pick(&mut app, &web, 2);
    key(&mut app, KeyCode::Char('m'));
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.active_notice(), Some("#2's squad is at work on it, so it stays where it is until its session stops."));
    assert!(Tickets::open(&w.config, &web).get(2).is_some());
}

#[test]
fn the_filter_narrows_the_board_by_title_project_or_squad_and_esc_clears_it() {
    let (_w, mut app, _web, _docs) = board_world("board-filter");
    key(&mut app, KeyCode::Char('/'));
    assert!(matches!(app.mode, Mode::ProjectName));
    typed(&mut app, "docs");
    // The board narrows while typing: Docs site's tasks, and the ones the Docs squad has.
    let numbers: Vec<(String, u32)> =
        app.board_groups().iter().flat_map(|g| g.rows.iter().map(|r| (r.project.clone(), r.number))).collect();
    assert_eq!(numbers.len(), 3, "{numbers:?}");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.filter, "docs");
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    assert!(draw(&mut term, &app).lines().next().unwrap().contains("filter: docs"));
    key(&mut app, KeyCode::Esc);
    assert!(app.filter.is_empty());
    key(&mut app, KeyCode::Char('/'));
    typed(&mut app, "nothing like it");
    key(&mut app, KeyCode::Enter);
    assert!(flat(&draw(&mut term, &app)).contains("No tasks match the filter. Esc clears it."));
}

#[test]
fn an_empty_board_says_what_to_press_and_enter_on_a_task_opens_it_beside_its_session() {
    let w = World::new("board-empty");
    let mut app = w.app();
    key(&mut app, KeyCode::Char('2'));
    assert_eq!(app.top, TOP_TASKS);
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("Nothing in play. Press n to write a task, or S on a task that can start."), "{f}");
    assert!(!f.contains(" t "), "{f}");
    // q quits from the board, a top-level list.
    key(&mut app, KeyCode::Char('q'));
    assert!(app.quit);
}

#[test]
fn the_help_on_the_board_lists_every_key_it_has_once_whatever_row_is_picked() {
    let (_w, mut app, web, _docs) = board_world("board-help");
    for n in [7, 2, 5] {
        pick(&mut app, &web, n);
        let here: Vec<String> =
            ui::help_for_test(&app).into_iter().filter(|(s, k, _)| s == "On this screen" && !k.is_empty()).map(|(_, k, _)| k).collect();
        for k in ["y", "enter", "h", "n", "m", "S", "c", "/", "s"] {
            assert_eq!(here.iter().filter(|x| *x == k).count(), 1, "#{n}: {k} once: {here:?}");
        }
    }
}
