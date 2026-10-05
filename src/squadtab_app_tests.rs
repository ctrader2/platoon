//! The Squads tab at the level of the app: the Squads list and each squad's brief as it reads (what it does, its steps
//! as a rail, its checks, when it asks you, what is wrong with it), `e` opening the brief in a stand-in editor and the
//! tab reading it again, the Agents list and its details (a change to an agent shown in every squad that uses it, the
//! agents the user's coding tool has, read only, under their own heading), drafting a brief or an agent with a
//! stand-in `claude`, copying, deleting (an agent in use refused) and restoring, a file that can't be read, and `S` on
//! a task listing the user's own squads. Every directory is a temp dir (`World`); nothing here starts a session.

use crate::agentfile;
use crate::app::{ARow, App, Confirm, Mode, SqFocus, SqView};
use crate::editors::Editor;
use crate::squadfile::{self, Back, Crew, Kind};
use crate::squads::{Launch, LeaderSetup};
use crate::testkit::{AiFake, World};
use crate::tickets::{AgentNames, Leader, NewTicket, Tickets};
use crate::ui;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
};
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    thread,
    time::{Duration, Instant},
};

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn ctrl(app: &mut App, c: char) {
    app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
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

/// What is inside the hand-over popup, read top to bottom (the screen faded behind it can't run into its rows).
fn in_popup(shown: &str) -> String {
    let lines: Vec<&str> = shown.lines().collect();
    let Some(top) = lines.iter().position(|l| l.contains("╭ Hand to a squad")) else { return shown.to_string() };
    let left = lines[top].chars().position(|c| c == '╭').unwrap_or(0);
    let right = left + lines[top].chars().skip(left + 1).position(|c| c == '╮').unwrap_or(0) + 1;
    lines[top..]
        .iter()
        .take_while(|l| !l.chars().nth(left).is_some_and(|c| c == '╰'))
        .map(|l| l.chars().skip(left).take(right - left + 1).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn flat(text: &str) -> String {
    text.replace(['│', '╭', '╮', '╰', '╯', '─'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Where the list's pane ends and the details' begins: after the list's top right corner.
fn split(text: &str) -> usize {
    text.lines().find_map(|l| l.chars().collect::<String>().find("╮╭").map(|at| l[..at].chars().count() + 1)).expect("two panes")
}

/// The list's pane, flat (the rows below the tab bar, up to where the details begin).
fn left(text: &str) -> String {
    let at = split(text);
    flat(&text.lines().skip(2).map(|l| l.chars().take(at).collect::<String>()).collect::<Vec<_>>().join("\n"))
}

/// The details' pane, flat.
fn right(text: &str) -> String {
    let at = split(text);
    flat(&text.lines().skip(2).map(|l| l.chars().skip(at).collect::<String>()).collect::<Vec<_>>().join("\n"))
}

/// The app on the Squads tab.
fn on_the_tab(name: &str) -> (World, App) {
    let w = World::new(name);
    let mut app = w.app();
    key(&mut app, KeyCode::Char('4'));
    assert!(app.squads_tab_showing());
    (w, app)
}

/// The brief's text, as its file holds it.
fn brief_text(w: &World, id: &str) -> String {
    fs::read_to_string(squadfile::path(&w.config, id)).unwrap()
}

/// An agent file of the user's coding tool (Claude Code's `agents/<name>.md`), in the world's own Claude folder.
fn tool_agent(w: &World, name: &str, front: &str, prompt: &str) {
    fs::create_dir_all(w.root.join("agents")).unwrap();
    fs::write(w.root.join("agents").join(format!("{name}.md")), format!("---\nname: {name}\n{front}---\n{prompt}\n")).unwrap();
}

/// Pick the entry of the small list that reads `label`, and take it.
fn choose(app: &mut App, label: &str) {
    assert!(matches!(app.mode, Mode::SquadChoice), "{:?}", app.active_notice());
    let at = app
        .sqt
        .choice
        .options
        .iter()
        .position(|(l, _)| l.starts_with(label))
        .unwrap_or_else(|| panic!("no {label} in {:?}", app.sqt.choice.options));
    while app.sqt.choice.selected > at {
        key(app, KeyCode::Up);
    }
    while app.sqt.choice.selected < at {
        key(app, KeyCode::Down);
    }
    key(app, KeyCode::Enter);
}

fn fake_editor(w: &World, name: &str, body: &str) -> Editor {
    let path = w.base.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    Editor { id: name.into(), name: name.into(), program: path.to_string_lossy().into_owned(), args: Vec::new(), terminal: true }
}

fn wait_for_editor(app: &mut App) {
    let start = Instant::now();
    while app.notes_busy() {
        assert!(start.elapsed() < Duration::from_secs(10), "the stand-in editor never finished");
        app.poll_editors();
        thread::sleep(Duration::from_millis(10));
    }
}

/// The Agents list, with the details of agent `id` open.
fn go_agent(app: &mut App, id: &str) {
    app.sqt.focus = SqFocus::List;
    key(app, KeyCode::Right);
    assert_eq!(app.sqt.view, SqView::Agents);
    key(app, KeyCode::Home);
    let at = app.sqt.agents.iter().position(|l| l.id == id).unwrap_or_else(|| panic!("no agent {id}"));
    for _ in 0..at {
        key(app, KeyCode::Down);
    }
    key(app, KeyCode::Enter);
    assert_eq!((app.sqt.focus, app.sqt.agent().map(|a| a.id.as_str())), (SqFocus::Detail, Some(id)));
}

/// On row `row` of the open agent's details.
fn go_arow(app: &mut App, row: ARow) {
    let want = app.agent_detail_rows().iter().position(|r| *r == row).unwrap_or_else(|| panic!("no {row:?}"));
    key(app, KeyCode::Home);
    for _ in 0..want {
        key(app, KeyCode::Down);
    }
    assert_eq!(app.arow(), Some(row));
}

/// The agent's file as JSON.
fn agent_file(w: &World, id: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(agentfile::path(&w.config, id)).unwrap()).unwrap()
}

/// Where `text` starts in the last frame drawn.
fn find(term: &Terminal<TestBackend>, text: &str) -> (u16, u16) {
    let buf = term.backend().buffer();
    let want: Vec<String> = text.chars().map(|c| c.to_string()).collect();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width.saturating_sub(want.len() as u16) {
            if want.iter().enumerate().all(|(k, c)| buf[(x + k as u16, y)].symbol() == c) {
                return (x, y);
            }
        }
    }
    panic!("{text:?} isn't on the screen");
}

fn fg(term: &Terminal<TestBackend>, at: (u16, u16)) -> ratatui::style::Color {
    term.backend().buffer()[at].fg
}

fn bg(term: &Terminal<TestBackend>, at: (u16, u16)) -> ratatui::style::Color {
    term.backend().buffer()[at].bg
}

#[test]
fn the_tab_shows_each_squads_brief_as_it_reads() {
    let (_w, mut app) = on_the_tab("sqt-list");
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let shown = draw(&mut term, &app);
    let f = flat(&shown);
    assert!(f.contains("4 Squads") && !f.contains("4 Squads 4"), "the tab, without a count: {f}");
    assert!(f.contains("[Squads] Agents"), "the switch, in the list's title: {f}");
    let l = left(&shown);
    // A built-in squad as platoon has it says nothing under its name: a marker on every row would mark nothing.
    assert!(l.contains("▎ Coding 4 agents Research 4 agents Docs 3 agents Summarize 1 agent"), "{l}");
    let r = right(&shown);
    for s in [
        "Built in",
        "Built into platoon. e writes it to",
        "briefs/coding.md for you to change.",
        "What it does Makes a change to code in a git repository",
        "Steps ○ 1 planner Opus commands",
        "Checks ✓ the tests pass platoon runs the project's tests",
        "✓ the reviewer read the change platoon checks it ran after changes",
        "Every squad has the same leader: its system prompt and model are in Settings (s) → Projects.",
    ] {
        assert!(r.contains(s), "{s}: {r}");
    }
    assert!(!r.contains("can't be handed a task"), "nothing wrong with it: {r}");
    assert!(
        f.contains("e edit the brief") && f.contains("n describe a new squad or agent") && f.contains("c copy") && !f.contains("d delete"),
        "{f}"
    );
    for gone in ["About", "+ Add a step", "b send back", "g go to the agent", "[ ] move"] {
        assert!(!f.contains(gone), "the in-app step editor is gone ({gone}): {f}");
    }
    // The colours: the switch's selected label in the accent, the other dim; headings in the accent; the marks.
    let t = app.effective_theme().clone();
    assert_eq!(fg(&term, find(&term, "[Squads]")), t.accent);
    let (x, y) = find(&term, "[Squads]  Agents");
    assert_eq!(fg(&term, (x + 10, y)), t.dim, "the other label is dim");
    let (x, y) = find(&term, "▎ Coding");
    assert_eq!((bg(&term, (x + 3, y)), fg(&term, (x, y))), (t.surface, t.accent));
    assert_eq!(fg(&term, find(&term, "What it does")), t.accent);
    assert_eq!(fg(&term, find(&term, "✓ the tests pass")), t.ok);
    assert_eq!(fg(&term, find(&term, "platoon runs the project's tests")), t.subtext);
    // Further down, scrolled with the arrows once the details have the keys: when it asks you, and the leader's note.
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.sqt.focus, SqFocus::Detail);
    for _ in 0..10 {
        key(&mut app, KeyCode::Down);
    }
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("Asks you Before anything others can see, and when it is stuck."), "{f}");
    assert!(
        f.contains("✓ every acceptance box is ticked, with how platoon checks the squad's result")
            && f.contains("‖ nothing others see without your yes held by platoon"),
        "{f}"
    );
    assert!(f.contains("e edit the brief") && f.contains("↑↓ scroll") && f.contains("esc back to the list"), "{f}");
    let t2 = app.effective_theme().clone();
    assert_eq!(fg(&term, find(&term, "‖ nothing others")), t2.subtext);
    // Tab and Shift+arrows move between the panes; on the list, plain ← → switch the list.
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.sqt.focus, SqFocus::List);
    app.on_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
    assert_eq!(app.sqt.focus, SqFocus::Detail);
    key(&mut app, KeyCode::Right);
    assert_eq!((app.sqt.focus, app.sqt.view), (SqFocus::Detail, SqView::Squads), "a plain arrow in the details does nothing");
    app.on_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));
    assert_eq!(app.sqt.focus, SqFocus::List);
    key(&mut app, KeyCode::Right);
    assert_eq!((app.sqt.focus, app.sqt.view), (SqFocus::List, SqView::Agents));
    key(&mut app, KeyCode::Left);
    assert_eq!(app.sqt.view, SqView::Squads);
    key(&mut app, KeyCode::Char('/'));
    assert!(matches!(app.mode, Mode::List), "`/` is the sessions' search: not here");
}

/// A brief of the user's own with things wrong in it.
const BROKEN: &str = "---\nname: Broken\n---\n# What it does\nTries things.\n\n# Steps\n1. planner: Plans.\n2. verifer: Checks.\n\n# Checks\n- reviewer read the change\n- every claim names its source\n\n# Notes\nKept as I wrote it.\n";

#[test]
fn a_brief_with_problems_says_each_at_the_top_in_the_warning_colour_at_both_sizes() {
    let w = World::new("sqt-problems");
    squadfile::save_text(&w.config, "broken", BROKEN).unwrap();
    let mut app = w.app();
    key(&mut app, KeyCode::Char('4'));
    key(&mut app, KeyCode::End);
    assert_eq!(app.sqt.listed().map(|l| l.id.as_str()), Some("broken"));
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let shown = draw(&mut term, &app);
    assert!(left(&shown).contains("Broken 2 agents Your own · needs fixing"), "{shown}");
    let r = right(&shown);
    assert!(r.contains("Your own A file you own:") && r.contains("briefs/broken.md"), "{r}");
    for s in [
        "It can't be handed a task until these are fixed:",
        "• Steps, line 9: no agent called verifer; there are planner and verifier.",
        "• Checks, line 12: no step is done by reviewer; the steps' agents are planner and verifer.",
        "Notes Kept as I wrote it.",
        "· every claim names its source the squad checks this itself",
    ] {
        assert!(r.contains(s), "{s}: {r}");
    }
    let t = app.effective_theme().clone();
    assert_eq!(fg(&term, find(&term, "It can't be handed a task")), t.warn);
    assert_eq!(fg(&term, find(&term, "• Steps, line 9")), t.warn);
    assert_eq!(fg(&term, find(&term, "verifer · not found")), t.warn, "the step on the rail says so too");
    assert!((2..8).contains(&find(&term, "It can't be handed a task").1), "near the top, before what it does: {shown}");
    // 80x24: one pane each, every problem still a line of its own that says where, wrapped, never cut.
    let mut small = Terminal::new(TestBackend::new(80, 24)).unwrap();
    key(&mut app, KeyCode::Enter);
    let shown = draw(&mut small, &app);
    let f = flat(&shown);
    assert!(
        f.contains("It can't be handed a task until these are fixed:") && f.contains("• Steps, line 9: no agent called verifer;"),
        "{shown}"
    );
    for l in shown.lines() {
        assert!(l.chars().count() <= 80, "{l}");
    }
    assert_eq!(fg(&small, find(&small, "• Steps, line 9")), t.warn);
    // And without problems at 80x24: the brief from the top, scrolled down to its checks.
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Home);
    key(&mut app, KeyCode::Enter);
    let shown = draw(&mut small, &app);
    let r = right(&shown);
    assert!(r.contains("Built in") && r.contains("What it does Makes a change") && !r.contains("fixed:"), "{shown}");
    assert!(flat(&shown).contains("more lines below"), "{shown}");
    for _ in 0..30 {
        key(&mut app, KeyCode::Down);
    }
    let f = flat(&draw(&mut small, &app));
    assert!(f.contains("✓ the tests pass") && f.contains("Asks you"), "{f}");
}

#[test]
fn e_opens_the_brief_in_the_editor_and_the_tab_reads_it_again() {
    let (w, mut app) = on_the_tab("sqt-edit");
    // An editor that changes the second step's agent and the name, and one that leaves it as it is.
    let change = "sed 's/^2. builder:/2. reviewer:/; s/^name: Coding$/name: Careful coding/' \"$1\" > \"$1.new\" && mv \"$1.new\" \"$1\"";
    app.set_editors_for_test(vec![fake_editor(&w, "fakeed", "true")]);
    key(&mut app, KeyCode::Char('e'));
    assert!(matches!(app.mode, Mode::PickEditor), "the same list of editors as the notes");
    assert!(squadfile::path(&w.config, "coding").exists(), "a built-in's text is written there first");
    key(&mut app, KeyCode::Esc);
    assert!(!squadfile::path(&w.config, "coding").exists(), "and taken away again when no editor opens it");
    key(&mut app, KeyCode::Char('e'));
    key(&mut app, KeyCode::Enter);
    wait_for_editor(&mut app);
    assert_eq!(app.active_notice(), Some("Coding is unchanged: it is as platoon has it."));
    assert!(!squadfile::path(&w.config, "coding").exists(), "no file is needed for the built-in");
    app.set_editors_for_test(vec![fake_editor(&w, "changer", change)]);
    key(&mut app, KeyCode::Char('e'));
    key(&mut app, KeyCode::Enter);
    wait_for_editor(&mut app);
    assert_eq!(app.active_notice(), Some("Read the Careful coding brief again: it can be handed tasks."));
    let text = brief_text(&w, "coding");
    assert!(text.contains("name: Careful coding\n") && text.contains("2. reviewer: Makes the change"), "{text}");
    assert_eq!((app.sqt.list[0].kind, app.sqt.list[0].name()), (Kind::Modified, "Careful coding"));
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    app.notice = None;
    let shown = draw(&mut term, &app);
    let r = right(&shown);
    assert!(r.contains("● 2 reviewer Opus commands") && r.contains("User Modified A file you own:"), "{r}");
    let shown = draw(&mut term, &app);
    assert!(shown.lines().last().unwrap().contains("r restore"), "{shown}");
    // A change that breaks it: read again, and what is wrong said at once.
    app.set_editors_for_test(vec![fake_editor(
        &w,
        "breaker",
        "sed 's/^1. planner:/1. planer:/' \"$1\" > \"$1.new\" && mv \"$1.new\" \"$1\"",
    )]);
    key(&mut app, KeyCode::Char('e'));
    key(&mut app, KeyCode::Enter);
    wait_for_editor(&mut app);
    assert_eq!(
        app.active_notice(),
        Some(
            "Read the Careful coding brief again. Fix this before it can be handed a task: Steps, line 10: no agent called planer; there are reviewer, verifier and planner."
        )
    );
    // Restored: `r` asks, and the brief is kept as a .bak.
    key(&mut app, KeyCode::Char('r'));
    assert!(matches!(app.mode, Mode::Confirm(Confirm::RestoreSquad { .. })));
    assert!(flat(&draw(&mut term, &app)).contains("Put Careful coding back as platoon has it?"));
    key(&mut app, KeyCode::Char('y'));
    assert!(!squadfile::path(&w.config, "coding").exists() && squadfile::path(&w.config, "coding").with_extension("md.bak").exists());
    assert_eq!(app.squad_list()[0].kind, Kind::BuiltIn);
    assert!(
        app.active_notice().unwrap().starts_with("Coding is as platoon has it again. Your brief is kept as coding.md.bak"),
        "{:?}",
        app.active_notice()
    );
    // The draft isn't a file yet.
    assert!(!w.cache.join("squad-prompts").exists(), "a brief is edited in its own file, never a copy");
}

#[test]
fn the_agents_list_shows_every_agent_grouped_with_its_model_and_how_many_squads_use_it() {
    let (_w, mut app) = on_the_tab("sqt-agents");
    key(&mut app, KeyCode::Right);
    let mut term = Terminal::new(TestBackend::new(116, 40)).unwrap();
    let shown = draw(&mut term, &app);
    let l = left(&shown);
    // Every agent is built in, as platoon has it: no heading over them (it would tell nothing apart).
    assert!(l.starts_with("Squads [Agents] ▎ planner"), "{l}");
    assert!(l.contains("▎ planner Opus · 1 squad builder Sonnet · 1 squad reviewer Opus · 1 squad"), "{l}");
    for s in ["report-writer Sonnet · 1 squad checker", "summarizer Sonnet · 1 squad"] {
        assert!(l.contains(s), "{s}: {l}");
    }
    let mut narrow = Terminal::new(TestBackend::new(90, 40)).unwrap();
    let l = left(&draw(&mut narrow, &app));
    for s in ["report-writer Sonnet checker Opus", "summarizer Sonnet"] {
        assert!(l.contains(s), "{s}: {l}");
    }
    let f = flat(&draw(&mut term, &app));
    for s in [
        "Built in",
        "Agent",
        "A change here is a change in every squad that uses it.",
        "Name planner",
        "Model Opus",
        "Can change files No",
        "Can run commands Yes",
        "Can use the web No",
        "System prompt You are the planner.",
        "Used in",
        "Coding step 1",
    ] {
        assert!(f.contains(s), "{s}: {f}");
    }
    let t = app.effective_theme().clone();
    assert_eq!(fg(&term, find(&term, "[Agents]")), t.accent);
    key(&mut app, KeyCode::End);
    assert!(app.sqt.new_agent_selected());
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("▎ + New agent") && f.contains("enter new agent") && f.contains("Enter makes a new agent of your own"), "{f}");
    key(&mut app, KeyCode::Left);
    assert_eq!(app.sqt.view, SqView::Squads);
}

#[test]
fn the_agents_the_coding_tool_has_are_listed_read_only_under_their_own_heading_and_c_copies_one() {
    let w = World::new("sqt-tool-agents");
    tool_agent(&w, "code-reviewer", "description: Reviews code.\ntools: Read, Grep, Glob\nmodel: opus\n", "You review code. Plainly.");
    tool_agent(&w, "planner", "model: haiku\n", "A planner of the user's own coding tool.");
    tool_agent(&w, "free-hand", "", "Does anything.");
    // A brief of the user's own that names one of them.
    let mut s = squadfile::copy_of(&w.config, &squadfile::builtin("coding").unwrap(), "Reviewed", &[]);
    s.workflow[2].agent = "code-reviewer".into();
    s.checks = vec!["code-reviewer read the change".into()];
    squadfile::save(&w.config, &s).unwrap();
    let mut app = w.app();
    key(&mut app, KeyCode::Char('4'));
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    key(&mut app, KeyCode::End);
    let r = right(&draw(&mut term, &app));
    assert!(r.contains("● 3 code-reviewer Opus reads only · from your coding tool"), "a step may name it: {r}");
    assert!(!r.contains("fixed"), "the brief is fine with it: {r}");
    key(&mut app, KeyCode::Right);
    let shown = draw(&mut term, &app);
    let l = left(&shown);
    assert!(l.contains("Built in ▎ planner Opus · 2 squads"), "headings now that the agents differ: {l}");
    assert!(
        l.contains("From Claude Code code-reviewer Opus · 1 squad free-hand Leader's model · not used planner Haiku · platoon's is used"),
        "{l}"
    );
    let t = app.effective_theme().clone();
    assert_eq!(fg(&term, find(&term, "From Claude Code")), t.dim);
    let at = app.sqt.agents.iter().position(|l| l.id == "code-reviewer").unwrap();
    for _ in 0..at {
        key(&mut app, KeyCode::Down);
    }
    let shown = draw(&mut term, &app);
    let f = flat(&shown);
    let r = right(&shown);
    assert!(
        r.contains("From Claude Code")
            && r.contains("agents/code-reviewer.md Platoon only reads it: c copies it into platoon's own agents"),
        "{r}"
    );
    assert!(r.contains("Can change files No") && r.contains("Can run commands No") && r.contains("Used in Reviewed step 3"), "{r}");
    assert!(f.contains("c copy into platoon's own") && !f.contains("d delete"), "{f}");
    // Nothing on it can be changed, and it is never deleted.
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(
        app.active_notice(),
        Some("code-reviewer is an agent your coding tool has: platoon only reads it. c copies it into platoon's own to change it.")
    );
    key(&mut app, KeyCode::Char('d'));
    assert_eq!(app.active_notice(), Some("code-reviewer is an agent your coding tool has: platoon never changes or deletes it."));
    // `c` copies it into platoon's own, under its own name: briefs that name it use the copy from then on.
    key(&mut app, KeyCode::Char('c'));
    assert_eq!(app.name_box.input, "code-reviewer");
    key(&mut app, KeyCode::Enter);
    assert_eq!(
        app.active_notice(),
        Some("Made \"code-reviewer\", platoon's own copy of code-reviewer: briefs that name code-reviewer use it now.")
    );
    let v = agent_file(&w, "code-reviewer");
    assert_eq!(
        (v["model"].as_str(), v["can_change_files"].as_bool(), v["guidance"].as_str()),
        (Some("opus"), Some(false), Some("You review code. Plainly."))
    );
    assert_eq!(app.sqt.agent_listed().map(|l| l.kind), Some(Kind::Own));
    let lib = app.agent_library();
    assert!(lib.iter().find(|a| a.id == "code-reviewer").is_some_and(|a| a.from_file.is_none()), "platoon's own comes first");
    assert!(
        fs::read_to_string(w.root.join("agents/code-reviewer.md")).unwrap().contains("You review code."),
        "the tool's file is left as it is"
    );
}

#[test]
fn an_edit_to_an_agent_shows_in_every_squad_that_uses_it() {
    let (w, mut app) = on_the_tab("sqt-shared");
    key(&mut app, KeyCode::Char('c'));
    ctrl(&mut app, 'u');
    typed(&mut app, "Careful coding");
    key(&mut app, KeyCode::Enter);
    assert!(app.active_notice().unwrap().contains("It uses the same agents"), "{:?}", app.active_notice());
    assert!(brief_text(&w, "careful-coding").starts_with("---\nid: careful-coding\nname: Careful coding\n---\n# What it does\n"));
    go_agent(&mut app, "builder");
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let shown = draw(&mut term, &app);
    assert!(
        right(&shown).contains("Used in Coding step 2 Careful coding step 2") && left(&shown).contains("builder Sonnet · 2 squads"),
        "{shown}"
    );
    go_arow(&mut app, ARow::Model);
    key(&mut app, KeyCode::Enter);
    choose(&mut app, "Opus");
    assert_eq!(app.active_notice(), Some("builder runs on Opus now, in every squad that uses it (Coding and Careful coding)."));
    assert!(!squadfile::path(&w.config, "coding").exists(), "the briefs themselves didn't change");
    for (id, downs) in [("coding", 0), ("careful-coding", 4)] {
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Left);
        key(&mut app, KeyCode::Home);
        for _ in 0..downs {
            key(&mut app, KeyCode::Down);
        }
        assert_eq!(app.sqt.listed().map(|l| l.id.as_str()), Some(id));
        let f = flat(&draw(&mut term, &app));
        assert!(f.contains("● 2 builder Opus files · commands"), "{id}: {f}");
        let crew = Crew::load(&w.config, &squadfile::get(&w.config, id).unwrap());
        let launch = Launch::of(
            &crew,
            &LeaderSetup::builtin(),
            AgentNames::SquadRole,
            crate::squads::Place::WORKTREE,
            &crate::squads::Terms::default(),
        );
        let agents: Value = serde_json::from_str(&launch.agents).unwrap();
        assert_eq!(agents[format!("{id}-builder")]["model"], "opus", "{id}");
    }
    // Enter on a squad it is used in opens that squad's brief.
    go_agent(&mut app, "builder");
    go_arow(&mut app, ARow::UsedIn(1));
    key(&mut app, KeyCode::Enter);
    assert_eq!(
        (app.sqt.view, app.sqt.focus, app.sqt.listed().map(|l| l.id.as_str())),
        (SqView::Squads, SqFocus::Detail, Some("careful-coding"))
    );
    go_agent(&mut app, "builder");
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Char('d'));
    assert!(matches!(app.mode, Mode::Confirm(Confirm::RestoreAgent { .. })));
    key(&mut app, KeyCode::Char('y'));
    assert_eq!(agentfile::get(&w.config, "builder").unwrap().model, "sonnet");
}

#[test]
fn deleting_an_agent_a_squad_uses_is_refused_naming_the_squads() {
    let w = World::new("sqt-delete-agent");
    let mut mine = agentfile::blank(&w.config, "Test runner", &[]);
    mine.guidance = "You are the test runner.".into();
    agentfile::save(&w.config, &mine).unwrap();
    let mut coding = squadfile::builtin("coding").unwrap();
    coding.workflow[1].agent = "test-runner".into();
    squadfile::save(&w.config, &coding).unwrap();
    let careful = squadfile::copy_of(&w.config, &coding, "Careful coding", &[]);
    squadfile::save(&w.config, &careful).unwrap();
    let mut app = w.app();
    key(&mut app, KeyCode::Char('4'));
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::End);
    key(&mut app, KeyCode::Up);
    assert_eq!(app.sqt.agent().map(|a| a.id.as_str()), Some("test-runner"));
    key(&mut app, KeyCode::Char('d'));
    assert!(matches!(app.mode, Mode::List), "nothing to ask: it can't go");
    assert_eq!(
        app.active_notice(),
        Some(
            "Test runner is used in the Coding (step 2) and Careful coding (step 2) squads, so it can't be deleted. Give those steps another agent first."
        )
    );
    key(&mut app, KeyCode::Char('r'));
    assert_eq!(app.active_notice(), Some("Test runner is your own, not one platoon has: d deletes it."));
    squadfile::remove(&w.config, "careful-coding").unwrap();
    squadfile::remove(&w.config, "coding").unwrap();
    app.refresh_for_test();
    key(&mut app, KeyCode::End);
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Char('d'));
    key(&mut app, KeyCode::Char('y'));
    assert!(
        !agentfile::path(&w.config, "test-runner").exists()
            && agentfile::path(&w.config, "test-runner").with_extension("json.bak").exists()
    );
}

const DRAFT: &str = "---
name: Sentry triage
---
# What it does
Reads a Sentry error, reproduces it and fixes it.

# Steps
1. reader: Reads the error and its stack.
2. builder: Fixes it and runs the tests.
3. reader: Checks the fix against the error. May send the work back to step 2, at most once.

# Checks
- the tests pass
- reader read the change

# Asks you
When it can't reproduce the error.

```json
[{\"name\":\"reader\",\"model\":\"haiku\",\"can_change_files\":false,\"can_run_commands\":false,\"can_use_web\":true,\"guidance\":\"Read the error.\"}]
```";

const AGENT_DRAFT: &str = r#"{"name":"test-writer","model":"sonnet","can_change_files":true,"can_run_commands":true,"can_use_web":false,"guidance":"You are the test writer. Write failing tests first."}"#;

/// The tab, with the smart features on and calls going to a fake `claude`.
fn with_claude(name: &str) -> (World, App, AiFake) {
    let (w, mut app) = on_the_tab(name);
    let mut s = crate::settings::Settings { tokens_enabled: true, tokens_asked: true, ..Default::default() };
    s.auto_archive_days = 0;
    crate::settings::save(&w.config, &s).unwrap();
    app.refresh_for_test();
    let fake = AiFake::new(name);
    app.set_ai_program_for_test(&fake.script);
    (w, app, fake)
}

#[test]
fn n_asks_whether_to_describe_a_squad_or_an_agent_and_an_agent_is_drafted_on_its_own() {
    let (w, mut app, fake) = with_claude("sqt-draft-agent");
    fake.reply_default(AGENT_DRAFT);
    key(&mut app, KeyCode::Char('n'));
    assert!(matches!(app.mode, Mode::SquadChoice));
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("What to describe for Claude to draft:") && f.contains("▎ A new squad") && f.contains("A new agent"), "{f}");
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Char('n'));
    assert_eq!(app.sqt.choice.options[app.sqt.choice.selected].1, "agent", "on the Agents list, an agent first");
    key(&mut app, KeyCode::Enter);
    typed(&mut app, "writes failing tests first");
    key(&mut app, KeyCode::Enter);
    assert!(app.squad_drafting());
    app.refresh_until_for_test(20, "the draft", |a| !a.squad_drafting());
    assert_eq!(fake.calls(), 1);
    assert!(fake.has_arg(1, &agentfile::draft_system(agentfile::DRAFT_GUIDANCE)));
    assert!(app.sqt.agent_draft_selected() && app.sqt.focus == SqFocus::Detail && app.sqt.view == SqView::Agents);
    assert!(!agentfile::path(&w.config, "test-writer").exists(), "not saved yet");
    ctrl(&mut app, 's');
    assert_eq!(app.active_notice(), Some("Saved the agent \"test-writer\". A brief's step uses it as test-writer."));
    assert_eq!(app.sqt.agent_listed().map(|l| (l.id.as_str(), l.kind)), Some(("test-writer", Kind::Own)));
}

#[test]
fn describe_drafts_a_brief_that_reuses_agents_and_saves_its_new_ones_with_it() {
    let (w, mut app, fake) = with_claude("sqt-draft");
    fake.reply_default(DRAFT);
    key(&mut app, KeyCode::Char('n'));
    choose(&mut app, "A new squad");
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(
        f.contains("Describe a new squad") && f.contains("drafts its brief on Sonnet") && f.contains("enter ask Claude to draft it"),
        "{f}"
    );
    typed(&mut app, "a squad that triages Sentry errors: a reader and a fixer");
    key(&mut app, KeyCode::Enter);
    assert!(flat(&draw(&mut term, &app)).contains("Claude is drafting a squad…"));
    app.refresh_until_for_test(20, "the draft", |a| !a.squad_drafting());
    assert_eq!(fake.calls(), 1);
    let argv = fake.argv(1);
    assert_eq!(argv[argv.iter().position(|a| a == "--model").unwrap() + 1], "sonnet");
    assert!(fake.has_arg(1, &squadfile::draft_system(squadfile::DRAFT_GUIDANCE)));
    let asked = fake.stdin(1);
    assert!(asked.contains("- builder (Sonnet; changes files, runs commands): Make the change the plan describes"), "{asked}");
    assert!(app.sqt.draft_selected() && app.sqt.focus == SqFocus::Detail);
    assert_eq!(
        app.active_notice(),
        Some(
            "Here is a draft of the \"Sentry triage\" brief. It brings 1 new agent, saved with it. ctrl+s saves it (then e opens it in your editor), d discards it."
        )
    );
    app.notice = None;
    let shown = draw(&mut term, &app);
    let r = right(&shown);
    assert!(left(&shown).contains("Sentry triage 2 agents Draft, not saved") && flat(&shown).contains("ctrl+s save draft"), "{shown}");
    assert!(r.contains("○ 1 reader Haiku web · new with the draft") && r.contains("● 2 builder Sonnet files · commands Fixes it"), "{r}");
    assert!(r.contains("✓ reader read the change") && r.contains("Asks you When it can't reproduce the error."), "{r}");
    assert!(!squadfile::path(&w.config, "sentry-triage").exists() && !agentfile::path(&w.config, "reader").exists(), "not saved yet");
    key(&mut app, KeyCode::Char('e'));
    assert_eq!(app.active_notice(), Some("The draft isn't a file yet: ctrl+s saves it, then e opens it in your editor."));
    ctrl(&mut app, 's');
    assert_eq!(
        app.active_notice(),
        Some("Saved the \"Sentry triage\" brief and its new agent. Tasks can be handed to it now; e opens it in your editor.")
    );
    let text = brief_text(&w, "sentry-triage");
    assert!(
        text.contains("3. reader: Checks the fix against the error. May send the work back to step 2, at most once.")
            && text.contains("- reader read the change"),
        "{text}"
    );
    assert_eq!(agent_file(&w, "reader")["guidance"], "Read the error.");
    assert_eq!(app.sqt.listed().map(|l| (l.id.as_str(), l.kind)), Some(("sentry-triage", Kind::Own)));
    assert_eq!(squadfile::get(&w.config, "sentry-triage").unwrap().workflow[2].back, Some(Back { to: 1, times: 1 }));
}

#[test]
fn a_draft_that_cant_be_used_says_why_and_n_offers_the_description_again() {
    let (w, mut app, fake) = with_claude("sqt-baddraft");
    fake.reply_default(&DRAFT.replace("2. builder:", "2. mender:"));
    key(&mut app, KeyCode::Char('n'));
    key(&mut app, KeyCode::Enter);
    typed(&mut app, "a triage squad");
    key(&mut app, KeyCode::Enter);
    app.refresh_until_for_test(20, "the draft", |a| !a.squad_drafting());
    let said = app.active_notice().unwrap().to_string();
    assert!(
        said.starts_with("Claude's draft couldn't be used: Step 2: no agent called mender; there are reader")
            && said.ends_with("Press n to try again."),
        "{said}"
    );
    assert!(app.sqt.draft.is_none() && !squadfile::dir(&w.config).exists() && !agentfile::path(&w.config, "reader").exists());
    key(&mut app, KeyCode::Char('n'));
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.sqt.text.text, "a triage squad", "what was described is offered again");
    key(&mut app, KeyCode::Esc);
    fake.reply_default(DRAFT);
    key(&mut app, KeyCode::Char('n'));
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Enter);
    app.refresh_until_for_test(20, "the draft", |a| !a.squad_drafting());
    assert!(app.sqt.draft_selected());
    key(&mut app, KeyCode::Char('d'));
    assert!(matches!(app.mode, Mode::Confirm(Confirm::DiscardDraft { .. })));
    key(&mut app, KeyCode::Char('y'));
    assert!(app.sqt.draft.is_none() && app.sqt.draft_agents.is_empty() && !squadfile::path(&w.config, "sentry-triage").exists());
}

#[test]
fn an_agents_system_prompt_is_edited_in_the_editor_and_saved_into_the_agent() {
    let (w, mut app) = on_the_tab("sqt-prompt");
    app.set_editors_for_test(vec![fake_editor(&w, "fakeed", "printf 'Build it in small commits.\\n' > \"$1\"")]);
    go_agent(&mut app, "builder");
    go_arow(&mut app, ARow::Prompt);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::PickEditor), "the same list of editors as the notes");
    key(&mut app, KeyCode::Enter);
    wait_for_editor(&mut app);
    assert_eq!(
        app.active_notice(),
        Some("Saved builder's system prompt, in every squad that uses it (Coding), from the next task each is handed.")
    );
    assert_eq!(agent_file(&w, "builder")["guidance"], "Build it in small commits.");
    assert!(!w.cache.join("squad-prompts/agent-builder.md").exists(), "the file it was edited in is gone");
    assert!(!squadfile::path(&w.config, "coding").exists());
}

#[test]
fn copy_delete_and_restore_each_ask_first_and_a_built_in_is_never_deleted() {
    let (w, mut app) = on_the_tab("sqt-copy");
    key(&mut app, KeyCode::Char('d'));
    assert!(app.active_notice().unwrap().starts_with("Coding is built into platoon, so it can't be deleted."), "{:?}", app.active_notice());
    key(&mut app, KeyCode::Char('r'));
    assert!(app.active_notice().unwrap().starts_with("Coding is built into platoon"), "nothing to restore: {:?}", app.active_notice());
    key(&mut app, KeyCode::Char('c'));
    assert_eq!(app.name_box.input, "Coding copy");
    ctrl(&mut app, 'u');
    typed(&mut app, "Careful coding");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.sqt.listed().map(|l| (l.id.as_str(), l.kind)), Some(("careful-coding", Kind::Own)));
    assert!(app.active_notice().unwrap().starts_with("Made \"Careful coding\", a copy of Coding: a brief of your own (e opens it)."));
    key(&mut app, KeyCode::Char('d'));
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("Delete the squad \"Careful coding\"?") && f.contains("careful-coding.md.bak in the briefs folder"), "{f}");
    key(&mut app, KeyCode::Char('n'));
    assert!(squadfile::path(&w.config, "careful-coding").exists());
    key(&mut app, KeyCode::Char('d'));
    key(&mut app, KeyCode::Char('y'));
    assert!(
        !squadfile::path(&w.config, "careful-coding").exists()
            && squadfile::path(&w.config, "careful-coding").with_extension("md.bak").exists()
    );
    assert_eq!(app.squad_list().len(), 4);
    go_agent(&mut app, "reviewer");
    key(&mut app, KeyCode::Char('c'));
    assert_eq!(app.name_box.input, "reviewer copy");
    ctrl(&mut app, 'u');
    typed(&mut app, "Strict reviewer");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.sqt.agent_listed().map(|l| (l.id.as_str(), l.kind)), Some(("strict-reviewer", Kind::Own)));
    assert_eq!(app.active_notice(), Some("Made \"Strict reviewer\", a copy of reviewer. It is yours to change; no squad uses it yet."));
}

#[test]
fn a_file_that_cant_be_read_is_shown_as_such_and_left_alone() {
    let w = World::new("sqt-corrupt");
    fs::create_dir_all(squadfile::dir(&w.config)).unwrap();
    fs::write(squadfile::path(&w.config, "mine"), [0xff, 0xfe]).unwrap();
    fs::create_dir_all(agentfile::dir(&w.config)).unwrap();
    fs::write(agentfile::path(&w.config, "helper"), "[1]").unwrap();
    let mut app = w.app();
    key(&mut app, KeyCode::Char('4'));
    key(&mut app, KeyCode::Char('G'));
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(
        f.contains("mine Can't be read") && f.contains("mine.md can't be read (it isn't text (UTF-8)), so platoon leaves it alone."),
        "{f}"
    );
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.sqt.focus, SqFocus::List, "nothing to show");
    key(&mut app, KeyCode::Char('e'));
    assert!(
        matches!(app.mode, Mode::List) && app.active_notice().unwrap().starts_with("mine.md can't be read"),
        "{:?}",
        app.active_notice()
    );
    assert_eq!(fs::read(squadfile::path(&w.config, "mine")).unwrap(), [0xff, 0xfe]);
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::End);
    key(&mut app, KeyCode::Up);
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("helper can't be read") && f.contains("helper.json can't be read (it isn't a JSON object)"), "{f}");
}

#[test]
fn an_old_squad_file_is_shown_as_a_brief() {
    let w = World::new("sqt-converted");
    fs::create_dir_all(squadfile::old_dir(&w.config)).unwrap();
    let mut old = squadfile::to_value(&squadfile::copy_of(&w.config, &squadfile::builtin("docs").unwrap(), "Handbook", &[]), None);
    old.as_object_mut().unwrap().insert("leader".into(), serde_json::json!({"model": "opus"}));
    fs::write(squadfile::old_path(&w.config, "handbook"), old.to_string()).unwrap();
    let mut app = w.app();
    key(&mut app, KeyCode::Char('4'));
    key(&mut app, KeyCode::End);
    assert_eq!(app.sqt.listed().map(|l| (l.id.as_str(), l.kind)), Some(("handbook", Kind::Own)));
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let r = right(&draw(&mut term, &app));
    assert!(
        r.contains("Your own A file you own:") && r.contains("briefs/handbook.md") && r.contains("○ 1 outliner Sonnet reads only"),
        "{r}"
    );
    assert!(r.contains("✓ the tests pass") && r.contains("‖ nothing others see without your yes"), "what every squad had before: {r}");
    assert!(squadfile::old_dir(&w.config).join("handbook.json.bak").exists() && !squadfile::old_path(&w.config, "handbook").exists());
}

#[test]
fn s_on_a_task_lists_every_squad_by_name_and_hands_it_to_the_users_own() {
    let w = World::new("sqt-hand");
    let mut slow = agentfile::copy_of(&w.config, &agentfile::builtin("verifier").unwrap(), "Slow verifier", &[]);
    slow.model = "sonnet".into();
    agentfile::save(&w.config, &slow).unwrap();
    let mut own = squadfile::copy_of(&w.config, &squadfile::builtin("coding").unwrap(), "Careful coding", &[]);
    own.workflow[3].agent = "slow-verifier".into();
    squadfile::save(&w.config, &own).unwrap();
    let mut broken = squadfile::copy_of(&w.config, &own, "Broken", &[]);
    broken.workflow[0].agent = "nobody".into();
    squadfile::save(&w.config, &broken).unwrap();
    let mut s = crate::settings::Settings { tokens_enabled: true, tokens_asked: true, ..Default::default() };
    s.auto_archive_days = 0;
    crate::settings::save(&w.config, &s).unwrap();
    w.transcript("user-1", "/tmp", "make a basket page", 10);
    let mut app = w.app();
    let fake = AiFake::new("sqt-hand");
    fake.reply_default(r#"{"fits":true,"reason":"Coding work."}"#);
    app.set_ai_program_for_test(&fake.script);
    let id = app.projects.create("Web shop").unwrap();
    app.projects.assign("user-1", Some(&id)).unwrap();
    let folder = w.base.join("docs");
    fs::create_dir_all(&folder).unwrap();
    Tickets::open(&w.config, &id)
        .add(NewTicket { title: "Basket".into(), folders: vec![folder.to_string_lossy().into_owned()], ..Default::default() })
        .unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Char('3'));
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Char('S'));
    assert!(matches!(app.mode, Mode::SquadPick));
    let names: Vec<&str> = app.sq.pick.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Coding", "Research", "Docs", "Summarize", "Broken", "Careful coding"]);
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    for _ in 0..4 {
        key(&mut app, KeyCode::Down);
    }
    key(&mut app, KeyCode::Char(' '));
    let f = flat(&in_popup(&draw(&mut term, &app)));
    assert!(f.contains("▎ ● Broken") && f.contains("Needs fixing on the Squads tab first. Makes a change"), "{f}");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::SquadPick));
    assert_eq!(
        app.active_notice(),
        Some(
            "The Broken squad needs fixing on the Squads tab first: Steps, line 9: no agent called nobody; there are builder, reviewer and slow-verifier."
        )
    );
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    app.refresh_until_for_test(20, "the leader to start", |a| !a.sq.launched.is_empty());
    assert!(fake.stdin(1).starts_with("The squad: Careful coding\nWhat it can do: "), "{}", fake.stdin(1));
    let t = Tickets::open(&w.config, &id).get(1).cloned().unwrap();
    let leader = t.leader().unwrap().clone();
    assert_eq!((leader.squad.as_str(), leader.snapshot.as_deref()), ("careful-coding", Some("01-squad-1.json")));
    let launch = app.leader_launch(&id, &leader, &t).unwrap();
    assert_eq!((launch.agent.as_str(), launch.model.as_str(), launch.name.as_str()), ("careful-coding-leader", "sonnet", "Careful coding"));
    let agents: Value = serde_json::from_str(&launch.agents).unwrap();
    assert_eq!(agents["careful-coding-slow-verifier"]["model"], "sonnet");
    let leader_prompt = agents["careful-coding-leader"]["prompt"].as_str().unwrap();
    assert!(
        leader_prompt.contains("When to ask the user, as the brief says: Before anything others can see, and when it is stuck."),
        "{leader_prompt}"
    );
    // The task's screen says which brief.
}

#[test]
fn a_task_folders_own_agents_are_found_at_the_hand_over() {
    let w = World::new("sqt-repo-agents");
    let folder = w.base.join("shop");
    fs::create_dir_all(folder.join(".claude/agents")).unwrap();
    fs::write(folder.join(".claude/agents/shop-tester.md"), "---\nname: shop-tester\ntools: Read, Bash\n---\nYou run the shop's tests.\n")
        .unwrap();
    let mut own = squadfile::copy_of(&w.config, &squadfile::builtin("coding").unwrap(), "Shop coding", &[]);
    own.workflow[3].agent = "shop-tester".into();
    squadfile::save(&w.config, &own).unwrap();
    let app = w.app();
    // On the tab, without a task, it isn't found; with the task's folder, it is.
    assert!(!squadfile::problems(&own, &app.agent_library()).is_empty());
    let agents = app.agents_for(Some(&folder.to_string_lossy()));
    assert!(squadfile::problems(&own, &agents).is_empty(), "{:?}", squadfile::problems(&own, &agents));
    let crew = Crew::of(&own, &agents);
    let launch =
        Launch::of(&crew, &LeaderSetup::builtin(), AgentNames::SquadRole, crate::squads::Place::WORKTREE, &crate::squads::Terms::default());
    let v: Value = serde_json::from_str(&launch.agents).unwrap();
    let tester = &v["shop-coding-shop-tester"];
    assert_eq!(tester["tools"], serde_json::json!(["Read", "Bash"]), "its own tools, as its file has them");
    assert!(tester.get("model").is_none(), "no model of its own: the session's");
    assert!(tester["prompt"].as_str().unwrap().starts_with("You run the shop's tests."));
}

#[test]
fn a_leader_continued_later_runs_with_the_squad_as_it_was_when_it_started() {
    let w = World::new("sqt-snapshot");
    let app = w.app();
    let mut store = Tickets::open(&w.config, "p1");
    let n = store.add(NewTicket { title: "Basket".into(), folders: vec!["/tmp".into()], ..Default::default() }).unwrap();
    let coding = squadfile::builtin("coding").unwrap();
    let at_start = Launch::of(
        &Crew::of(&coding, &agentfile::builtins()),
        &LeaderSetup::builtin(),
        AgentNames::SquadRole,
        crate::squads::Place::WORKTREE,
        &crate::squads::Terms::default(),
    );
    let snap = store.save_snapshot(n, 1, &at_start.to_json()).unwrap();
    let with = Leader { squad: "coding".into(), session: "s-1".into(), started: 1, names: AgentNames::SquadRole, snapshot: Some(snap) };
    let mut verifier = agentfile::builtin("verifier").unwrap();
    verifier.model = "sonnet".into();
    agentfile::save(&w.config, &verifier).unwrap();
    let t = store.get(n).cloned().unwrap();
    let resumed = app.leader_launch("p1", &with, &t).unwrap();
    assert_eq!(resumed, at_start, "the snapshot, not the agents as they are now");
    let old = Leader { squad: "coding".into(), session: "s-0".into(), started: 1, names: AgentNames::Plain, snapshot: None };
    let now = app.leader_launch("p1", &old, &t).unwrap();
    let agents: Value = serde_json::from_str(&now.agents).unwrap();
    assert_eq!((now.agent.as_str(), agents["verifier"]["model"].as_str()), ("leader", Some("sonnet")));
    let gone = Leader { snapshot: Some("01-squad-9.json".into()), ..with.clone() };
    assert_eq!(app.leader_launch("p1", &gone, &t).unwrap_err(), "what it was started with can't be read any more");
}

#[test]
fn the_steps_rail_draws_codings_send_backs_as_nested_loops_in_muted_colours() {
    let (_w, app) = on_the_tab("sqt-rail");
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let shown = draw(&mut term, &app);
    for piece in [
        "○  1 planner",
        "┌─▶ ●  2 builder",
        "│┌▶ │",
        "││  ●  3 reviewer",
        "│└──┤",
        "│   ●  4 verifier",
        "└───┤",
        "◉  Done: the result goes on the task",
    ] {
        assert!(shown.contains(piece), "{piece}: {shown}");
    }
    let row_of = |text: &str| shown.lines().position(|l| l.contains(text)).unwrap_or_else(|| panic!("{text}"));
    assert!(shown.lines().nth(row_of("│└──┤")).unwrap().contains("↩ back to step 2 (builder), at most twice"));
    assert!(shown.lines().nth(row_of("└───┤")).unwrap().contains("↩ back to step 2 (builder), at most once"));
    let t = app.effective_theme().clone();
    let (x, y) = find(&term, "┌─▶ ●");
    for dx in 0..3 {
        assert_eq!(fg(&term, (x + dx, y)), t.dim, "the loop at {dx}");
    }
    assert_eq!(fg(&term, (x + 4, y)), t.subtext, "a step's mark");
    assert_eq!(bg(&term, (x + 4, y)), t.bg, "nothing on the brief is picked: it is read, and edited in its file");
    assert_eq!(fg(&term, find(&term, "↩ back to step 2 (builder), at most twice")), t.subtext);
}

#[test]
fn twelve_steps_with_five_send_backs_scroll_at_80_columns() {
    let w = World::new("sqt-rail-12");
    let mut s = squadfile::copy_of(&w.config, &squadfile::builtin("coding").unwrap(), "Long haul", &[]);
    let agents = ["planner", "builder", "reviewer", "verifier"];
    s.workflow = (0..12)
        .map(|i| squadfile::Step {
            agent: agents[i % 4].into(),
            does: format!("Does part {} of the long haul, carefully and completely, then reports back.", i + 1),
            back: None,
            line: 0,
        })
        .collect();
    for (from, to, times) in [(11, 0, 1), (10, 1, 2), (7, 2, 3), (5, 3, 1), (8, 6, 2)] {
        s.workflow[from].back = Some(Back { to, times });
    }
    squadfile::save(&w.config, &s).unwrap();
    let mut app = w.app();
    key(&mut app, KeyCode::Char('4'));
    key(&mut app, KeyCode::End);
    assert_eq!(app.sqt.listed().map(|l| l.id.as_str()), Some("long-haul"));
    let mut term = Terminal::new(TestBackend::new(80, 30)).unwrap();
    key(&mut app, KeyCode::Enter);
    let mut seen = String::new();
    for _ in 0..80 {
        key(&mut app, KeyCode::Down);
        seen.push_str(&draw(&mut term, &app));
    }
    for n in 1..=12 {
        assert!(seen.contains(&format!(" {n:>2} ")), "step {n}: {seen}");
    }
    let last = draw(&mut term, &app);
    assert!(seen.contains("◉  Done") && seen.contains("▶") && seen.contains("┼"), "the loops drawn, one crossing another");
    assert!(last.contains("◉  Done") && !last.contains("more lines below"), "scrolled to the end: {last}");
    assert!(seen.contains("↩ back to step 1, at most once") && !seen.contains("↩ back to step 1 (planner), a…"), "{seen}");
    for l in last.lines().skip(2).take(25) {
        assert!(l.chars().count() <= 80 && l.ends_with(' '), "{l}");
        let l = l.trim_end();
        assert!(l.ends_with('│') || l.ends_with('╮') || l.ends_with('╯') || l.is_empty(), "{l}");
    }
}

#[test]
fn an_agent_has_skills_and_mcp_servers_picked_from_what_is_installed_or_typed() {
    let w = World::new("sqt-tools");
    fs::create_dir_all(w.root.join("skills/pdf")).unwrap();
    fs::write(w.root.join("skills/pdf/SKILL.md"), "---\nname: pdf\ndescription: PDFs\n---\n").unwrap();
    fs::write(w.root.join(".claude.json"), r#"{"mcpServers":{"sentry":{"command":"npx","env":{"TOKEN":"secret-token"}},"github":{}}}"#)
        .unwrap();
    let mut app = w.app();
    key(&mut app, KeyCode::Char('4'));
    go_agent(&mut app, "builder");
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    go_arow(&mut app, ARow::Skills);
    key(&mut app, KeyCode::Enter);
    let labels: Vec<&str> = app.sqt.choice.options.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(labels, ["[ ] pdf", "Type the name of a skill…"]);
    choose(&mut app, "[ ] pdf");
    assert_eq!(app.active_notice(), Some("builder has the skill pdf now, in every squad that uses it (Coding)."));
    key(&mut app, KeyCode::Esc);
    go_arow(&mut app, ARow::Mcp);
    key(&mut app, KeyCode::Enter);
    assert!(!flat(&draw(&mut term, &app)).contains("secret-token"), "what a server is, is never shown");
    choose(&mut app, "[ ] sentry");
    key(&mut app, KeyCode::Esc);
    let crew = Crew::load(&w.config, &squadfile::builtin("coding").unwrap());
    let launch =
        Launch::of(&crew, &LeaderSetup::builtin(), AgentNames::SquadRole, crate::squads::Place::WORKTREE, &crate::squads::Terms::default());
    let agents: Value = serde_json::from_str(&launch.agents).unwrap();
    let b = &agents["coding-builder"];
    assert_eq!((b["skills"].clone(), b["mcpServers"].clone()), (serde_json::json!(["pdf"]), serde_json::json!(["sentry"])));
    assert_eq!(b["tools"], serde_json::json!(["Read", "Grep", "Glob", "Edit", "Write", "Bash", "Skill", "mcp__sentry"]));
    // The rail shows it has them.
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Left);
    assert!(flat(&draw(&mut term, &app)).contains("● 2 builder Sonnet files · commands · skills · MCP"));
}
