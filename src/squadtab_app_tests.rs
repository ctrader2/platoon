//! The Squads tab at the level of the app: the Squads and Agents lists and their details, every change made with the
//! keys (a model, a switch, steps, their agents, order and send-backs, a system prompt in a stand-in editor), a change
//! to an agent shown in every squad that uses it, "Used in" and `g` between a step and its agent, drafting a squad
//! or an agent with a stand-in `claude`, copying, deleting (an agent in use refused) and restoring, a file that can't
//! be read, and `a` on a ticket listing the user's own squads. Every directory is a temp dir (`World`); nothing here starts a session.

use crate::agentfile;
use crate::app::{ARow, App, Confirm, DRow, Mode, SqFocus, SqView};
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

fn flat(text: &str) -> String {
    text.replace(['│', '╭', '╮', '╰', '╯', '─'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Where the list's pane ends and the details' begins: after the list's top right corner.
fn split(text: &str) -> usize {
    text.lines().find_map(|l| l.chars().collect::<String>().find("╮╭").map(|at| l[..at].chars().count() + 1)).expect("two panes")
}

/// The list's pane, flat (the frame's rows below the tab bar, up to where the details begin).
fn left(text: &str) -> String {
    let at = split(text);
    flat(&text.lines().skip(3).map(|l| l.chars().take(at).collect::<String>()).collect::<Vec<_>>().join("\n"))
}

/// The details' pane, flat.
fn right(text: &str) -> String {
    let at = split(text);
    flat(&text.lines().skip(3).map(|l| l.chars().skip(at).collect::<String>()).collect::<Vec<_>>().join("\n"))
}

/// The app on the Squads tab.
fn on_the_tab(name: &str) -> (World, App) {
    let w = World::new(name);
    let mut app = w.app();
    key(&mut app, KeyCode::Char('3'));
    assert!(app.squads_tab_showing());
    (w, app)
}

/// The details have the keys and the cursor is on `row` (moved there with the arrows, as a person would).
fn go(app: &mut App, row: DRow) {
    if app.sqt.focus != SqFocus::Detail {
        key(app, KeyCode::Enter);
    }
    assert_eq!(app.sqt.focus, SqFocus::Detail);
    let rows = crate::app::detail_rows(app.sqt.squad().unwrap());
    let want = rows.iter().position(|r| *r == row).unwrap_or_else(|| panic!("no {row:?}"));
    key(app, KeyCode::Home);
    for _ in 0..want {
        key(app, KeyCode::Down);
    }
    assert_eq!(app.sqt.drow(), Some(row));
}

/// The squad's file as JSON.
fn file(w: &World, id: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(squadfile::path(&w.config, id)).unwrap()).unwrap()
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
fn the_tab_lists_the_built_in_squads_with_about_and_the_workflow_as_a_rail() {
    let (_w, mut app) = on_the_tab("sqt-list");
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let shown = draw(&mut term, &app);
    let f = flat(&shown);
    assert!(f.contains("3 Squads 4"), "the tab and how many squads: {f}");
    assert!(f.contains("[Squads] Agents"), "the switch, in the list's title: {f}");
    let l = left(&shown);
    assert!(l.contains("▎ Coding 4 agents Built in Research 4 agents Built in Docs 3 agents Built in Summarize 1 agent Built in"), "{l}");
    let r = right(&shown);
    for s in [
        "About",
        "Name Coding",
        "What it can do Makes a change to code",
        "Every squad has the same leader: its system prompt and model are in Settings (s), on the Projects tab.",
        "Workflow",
    ] {
        assert!(r.contains(s), "{s}: {shown}");
    }
    for gone in ["Needs a folder", "Use a worktree", "Leader Model", "System prompt You are given one ticket"] {
        assert!(!f.contains(gone), "no {gone} on a squad any more: {f}");
    }
    assert!(!f.contains("Members"), "a squad has no members of its own any more: {f}");
    assert!(
        f.contains("n describe a new squad or agent") && f.contains("c copy") && !f.contains("d delete") && f.contains("← → agents"),
        "{f}"
    );
    // The selected label of the switch is in the accent colour, the other dim; the selected squad has the surface.
    let t = app.effective_theme().clone();
    assert_eq!(fg(&term, find(&term, "[Squads]")), t.accent);
    let (x, y) = find(&term, "[Squads]  Agents");
    assert_eq!(fg(&term, (x + 10, y)), t.dim, "the other label is dim");
    let (x, y) = find(&term, "▎ Coding");
    assert_eq!((bg(&term, (x + 3, y)), fg(&term, (x, y))), (t.surface, t.accent));
    let (x, y) = find(&term, "Research");
    assert_eq!(bg(&term, (x, y)), t.bg, "not selected");
    // The workflow: each step, its agent with the agent's model and switches, and where it may send the work back.
    go(&mut app, DRow::StepAgent(3));
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("● 3 reviewer Opus commands Reads the change against the ticket"), "{f}");
    assert!(f.contains("↩ back to step 2 (builder), at most twice") && f.contains("↩ back to step 2 (builder), at most once"), "{f}");
    assert!(
        f.contains("● 4 verifier Haiku commands Runs the tests") && f.contains("◉ Done: the result goes on the ticket + Add a step"),
        "{f}"
    );
    assert!(
        f.contains("enter pick the agent")
            && f.contains("g go to the agent")
            && f.contains("b send back")
            && f.contains("ctrl+space back to the list"),
        "{f}"
    );
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
    // `/` is the sessions' search: not here.
    key(&mut app, KeyCode::Char('/'));
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn the_agents_list_shows_every_agent_grouped_with_its_model_and_how_many_squads_use_it() {
    let (_w, mut app) = on_the_tab("sqt-agents");
    key(&mut app, KeyCode::Right);
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let shown = draw(&mut term, &app);
    let l = left(&shown);
    assert!(l.starts_with("Squads [Agents] Built in"), "{l}");
    assert!(l.contains("Built in ▎ planner Opus · 1 squad builder Sonnet · 1 squad reviewer Opus · 1 squad"), "{l}");
    // A long name keeps its model beside it when the count doesn't fit.
    for s in ["report-writer Sonnet checker", "tech-writer Sonnet editor", "summarizer Sonnet · 1 squad"] {
        assert!(l.contains(s), "{s}: {l}");
    }
    key(&mut app, KeyCode::End);
    let l = left(&draw(&mut term, &app));
    assert!(l.contains("summarizer Sonnet · 1 squad ▎ + New agent"), "{l}");
    key(&mut app, KeyCode::Home);
    let f = flat(&draw(&mut term, &app));
    // The first agent's details: what it is, and the squads it is used in.
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
    assert!(f.contains("← → squads") && f.contains("n describe a new squad or agent"), "{f}");
    let t = app.effective_theme().clone();
    assert_eq!(fg(&term, find(&term, "[Agents]")), t.accent);
    let (x, y) = find(&term, "Squads  [Agents]");
    assert_eq!(fg(&term, (x, y)), t.dim, "the other label is dim");
    let (x, y) = find(&term, "▎ planner");
    assert_eq!(bg(&term, (x + 3, y)), t.surface);
    assert_eq!(bg(&term, find(&term, "builder")), t.bg);
    // Down the list: the selection follows, and "+ New agent" says what enter does there.
    key(&mut app, KeyCode::End);
    assert!(app.sqt.new_agent_selected());
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("▎ + New agent") && f.contains("enter new agent") && f.contains("Enter makes a new agent of your own"), "{f}");
    let (x, y) = find(&term, "+ New agent");
    assert_eq!((fg(&term, (x, y)), bg(&term, (x, y))), (t.accent, t.surface));
    // ← switches back.
    key(&mut app, KeyCode::Left);
    assert_eq!(app.sqt.view, SqView::Squads);
}

#[test]
fn an_edit_to_an_agent_shows_in_every_squad_that_uses_it() {
    let (w, mut app) = on_the_tab("sqt-shared");
    // A copy of Coding: the same agents.
    key(&mut app, KeyCode::Char('c'));
    ctrl(&mut app, 'u');
    typed(&mut app, "Careful coding");
    key(&mut app, KeyCode::Enter);
    assert!(app.active_notice().unwrap().contains("it uses the same agents"), "{:?}", app.active_notice());
    go_agent(&mut app, "builder");
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let shown = draw(&mut term, &app);
    assert!(
        right(&shown).contains("Used in Coding step 2 Careful coding step 2") && left(&shown).contains("builder Sonnet · 2 squads"),
        "{shown}"
    );
    go_arow(&mut app, ARow::Model);
    key(&mut app, KeyCode::Enter);
    assert!(flat(&draw(&mut term, &app)).contains("Model for builder:"));
    choose(&mut app, "Opus");
    assert_eq!(app.active_notice(), Some("builder runs on Opus now, in every squad that uses it (Coding and Careful coding)."));
    let v = agent_file(&w, "builder");
    assert_eq!(
        (v["builtin"].as_str(), v["model"].as_str(), v["guidance"].as_str()),
        (Some("builder"), Some("opus"), Some(crate::squads::BUILDER_GUIDANCE))
    );
    assert!(!squadfile::path(&w.config, "coding").exists(), "the squads themselves didn't change");
    let l = left(&draw(&mut term, &app));
    assert!(
        l.contains("summarizer Sonnet · 1 squad User Modified ▎ builder Opus · 2 squads"),
        "a changed built-in, in a group of its own: {l}"
    );
    // Both squads show it, and both start with it.
    for (id, downs) in [("coding", 0), ("careful-coding", 4)] {
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Left);
        key(&mut app, KeyCode::Home);
        for _ in 0..downs {
            key(&mut app, KeyCode::Down);
        }
        assert_eq!(app.sqt.listed().map(|l| l.id.as_str()), Some(id));
        go(&mut app, DRow::StepAgent(1));
        let f = flat(&draw(&mut term, &app));
        assert!(f.contains("● 2 builder Opus files · commands"), "{id}: {f}");
        let crew = Crew::load(&w.config, &squadfile::get(&w.config, id).unwrap());
        let agents: Value =
            serde_json::from_str(&Launch::of(&crew, &LeaderSetup::builtin(), AgentNames::SquadRole, crate::squads::Place::WORKTREE).agents)
                .unwrap();
        assert_eq!(agents[format!("{id}-builder")]["model"], "opus", "{id}");
    }
    // A switch: the tools follow, everywhere.
    go_agent(&mut app, "builder");
    go_arow(&mut app, ARow::Web);
    key(&mut app, KeyCode::Char(' '));
    assert_eq!(app.active_notice(), Some("builder can use the web now, in every squad that uses it (Coding and Careful coding)."));
    assert_eq!(
        crate::squads::tools_for(&agentfile::get(&w.config, "builder").unwrap()),
        ["Read", "Grep", "Glob", "Edit", "Write", "Bash", "WebSearch", "WebFetch"]
    );
    // Put back as platoon has it, after asking.
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Char('d'));
    assert!(matches!(app.mode, Mode::Confirm(Confirm::RestoreAgent { .. })));
    assert!(flat(&draw(&mut term, &app)).contains("Put builder back as platoon has it?"));
    key(&mut app, KeyCode::Char('y'));
    assert!(!agentfile::path(&w.config, "builder").exists() && agentfile::path(&w.config, "builder").with_extension("json.bak").exists());
    assert!(
        app.active_notice().unwrap().starts_with("builder is as platoon has it again, in every squad that uses it"),
        "{:?}",
        app.active_notice()
    );
    assert_eq!(agentfile::get(&w.config, "builder").unwrap().model, "sonnet");
}

#[test]
fn used_in_and_g_go_between_a_step_and_its_agent() {
    let (_w, mut app) = on_the_tab("sqt-used-in");
    go(&mut app, DRow::Step(2));
    key(&mut app, KeyCode::Char('g'));
    assert_eq!((app.sqt.view, app.sqt.focus, app.sqt.agent().map(|a| a.id.as_str())), (SqView::Agents, SqFocus::Detail, Some("reviewer")));
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("[Agents]") && f.contains("Used in Coding step 3"), "{f}");
    // Enter on the squad it is used in: that squad, on the step's agent.
    go_arow(&mut app, ARow::UsedIn(0));
    assert!(flat(&draw(&mut term, &app)).contains("enter open the squad"));
    key(&mut app, KeyCode::Enter);
    assert_eq!((app.sqt.view, app.sqt.focus, app.sqt.listed().map(|l| l.id.as_str())), (SqView::Squads, SqFocus::Detail, Some("coding")));
    assert_eq!(app.sqt.drow(), Some(DRow::StepAgent(2)));
    // The same from Docs' second step: the tech-writer.
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    go(&mut app, DRow::StepAgent(1));
    key(&mut app, KeyCode::Char('g'));
    assert_eq!(app.sqt.agent().map(|a| a.id.as_str()), Some("tech-writer"));
    // Away from a step, g is the top of the details, as before.
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Left);
    go(&mut app, DRow::CanDo);
    key(&mut app, KeyCode::Char('g'));
    assert_eq!((app.sqt.view, app.sqt.drow()), (SqView::Squads, Some(DRow::Name)));
}

#[test]
fn a_steps_agent_is_picked_from_every_agent_or_a_new_one_made_there() {
    let (w, mut app) = on_the_tab("sqt-pick-agent");
    app.set_editors_for_test(vec![fake_editor(&w, "fakeed", "printf 'You are the test runner. Run the tests.\\n' > \"$1\"")]);
    go(&mut app, DRow::StepAgent(3));
    key(&mut app, KeyCode::Enter);
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(
        f.contains("Which agent does step 4:")
            && f.contains("planner · Opus · runs commands")
            && f.contains("report-writer · Sonnet · changes files"),
        "{f}"
    );
    assert_eq!(app.sqt.choice.options.last().map(|(l, _)| l.as_str()), Some("+ New agent"), "at the bottom");
    assert_eq!(app.sqt.choice.options[app.sqt.choice.selected].1, "verifier", "on the one it has");
    choose(&mut app, "reviewer");
    assert_eq!(app.active_notice(), Some("reviewer does step 4 now."));
    assert_eq!(squadfile::get(&w.config, "coding").unwrap().workflow[3].agent, "reviewer");
    let l = left(&draw(&mut term, &app));
    assert!(l.contains("Coding 3 agents User Modified"), "the different agents its steps use: {l}");
    // A new one: named, made blank, put on the step, and opened on the Agents list to fill in.
    key(&mut app, KeyCode::Enter);
    choose(&mut app, "+ New agent");
    assert!(matches!(app.mode, Mode::ProjectName));
    assert!(flat(&draw(&mut term, &app)).contains("new agent"));
    typed(&mut app, "Test runner");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(squadfile::get(&w.config, "coding").unwrap().workflow[3].agent, "test-runner");
    let v = agent_file(&w, "test-runner");
    assert_eq!(
        (v["name"].as_str(), v["guidance"].as_str(), v["model"].as_str(), v.get("builtin")),
        (Some("Test runner"), Some(""), Some("sonnet"), None)
    );
    assert_eq!((app.sqt.view, app.sqt.focus, app.arow()), (SqView::Agents, SqFocus::Detail, Some(ARow::Prompt)));
    assert!(
        app.active_notice()
            .unwrap()
            .starts_with("Made the agent \"Test runner\" and put it on step 4. It needs a system prompt before the squad can start"),
        "{:?}",
        app.active_notice()
    );
    let shown = draw(&mut term, &app);
    let f = right(&shown);
    assert!(
        f.contains("Squads can't use it until this is fixed: Test runner has no system prompt yet.")
            && f.contains("System prompt None yet: enter writes one")
            && f.contains("Your own"),
        "{f}"
    );
    assert!(left(&shown).contains("Your own ▎ Test runner needs fixing"), "{shown}");
    // Until it has one, the squad says so and can't start.
    let s = squadfile::get(&w.config, "coding").unwrap();
    assert_eq!(squadfile::problems(&s, &agentfile::all(&w.config)), ["Step 4's agent isn't ready: Test runner has no system prompt yet."]);
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Home);
    go(&mut app, DRow::Name);
    let f = flat(&draw(&mut term, &app));
    assert!(
        f.contains("User Modified · needs fixing")
            && f.contains("Can't be handed a ticket until this is fixed: Step 4's agent isn't ready"),
        "{f}"
    );
    // Written in the editor: ready.
    go_agent(&mut app, "test-runner");
    go_arow(&mut app, ARow::Prompt);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::PickEditor));
    key(&mut app, KeyCode::Enter);
    wait_for_editor(&mut app);
    assert_eq!(
        app.active_notice(),
        Some("Saved Test runner's system prompt, in every squad that uses it (Coding), from the next ticket each is handed.")
    );
    assert_eq!(agent_file(&w, "test-runner")["guidance"], "You are the test runner. Run the tests.");
    let s = squadfile::get(&w.config, "coding").unwrap();
    assert!(squadfile::problems(&s, &agentfile::all(&w.config)).is_empty());
    // "+ New agent" on the Agents list makes one on no step.
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::End);
    key(&mut app, KeyCode::Enter);
    typed(&mut app, "Linter");
    key(&mut app, KeyCode::Enter);
    assert!(agentfile::path(&w.config, "linter").exists());
    assert!(app.active_notice().unwrap().starts_with("Made the agent \"Linter\". It needs a system prompt"), "{:?}", app.active_notice());
    assert!(flat(&draw(&mut term, &app)).contains("No squad uses it yet."));
}

#[test]
fn a_new_step_can_be_given_a_new_agent_made_there() {
    let (w, mut app) = on_the_tab("sqt-new-step-agent");
    go(&mut app, DRow::AddStep);
    key(&mut app, KeyCode::Char('a'));
    choose(&mut app, "+ New agent");
    assert!(matches!(app.mode, Mode::SquadText));
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("a new step for a new agent") && f.contains("You name the new agent next."), "{f}");
    typed(&mut app, "Writes the release notes.");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::ProjectName), "its name next");
    assert!(flat(&draw(&mut term, &app)).contains("new agent"));
    assert_eq!(squadfile::get(&w.config, "coding").unwrap().workflow.len(), 4, "nothing made before it has a name");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.active_notice(), Some("An agent needs a name."));
    typed(&mut app, "Release writer");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));
    let s = squadfile::get(&w.config, "coding").unwrap();
    assert_eq!(
        (s.workflow.len(), s.workflow[4].agent.as_str(), s.workflow[4].does.as_str()),
        (5, "release-writer", "Writes the release notes.")
    );
    let v = agent_file(&w, "release-writer");
    assert_eq!((v["name"].as_str(), v["guidance"].as_str()), (Some("Release writer"), Some("")));
    assert_eq!(
        (app.sqt.view, app.sqt.focus, app.sqt.agent().map(|a| a.id.as_str()), app.arow()),
        (SqView::Agents, SqFocus::Detail, Some("release-writer"), Some(ARow::Prompt))
    );
    assert_eq!(
        app.active_notice(),
        Some(
            "Made the agent \"Release writer\" and added step 5, done by it. It needs a system prompt before the squad can start: enter on System prompt writes one."
        )
    );
    let f = right(&draw(&mut term, &app));
    assert!(f.contains("Used in Coding step 5") && f.contains("System prompt None yet: enter writes one"), "{f}");
    // Back on the squad, the cursor is on the new step's agent.
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Left);
    assert_eq!(app.sqt.drow(), Some(DRow::StepAgent(4)));
    // Cancelling the name makes nothing.
    go(&mut app, DRow::AddStep);
    key(&mut app, KeyCode::Char('a'));
    choose(&mut app, "+ New agent");
    typed(&mut app, "Something.");
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Esc);
    assert_eq!(squadfile::get(&w.config, "coding").unwrap().workflow.len(), 5);
    assert_eq!(agentfile::all(&w.config).len(), 13);
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
    key(&mut app, KeyCode::Char('3'));
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::End);
    key(&mut app, KeyCode::Up);
    assert_eq!(app.sqt.agent().map(|a| a.id.as_str()), Some("test-runner"));
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    assert!(flat(&draw(&mut term, &app)).contains("d delete"));
    key(&mut app, KeyCode::Char('d'));
    assert!(matches!(app.mode, Mode::List), "nothing to ask: it can't go");
    assert_eq!(
        app.active_notice(),
        Some(
            "Test runner is used in the Coding (step 2) and Careful coding (step 2) squads, so it can't be deleted. Give those steps another agent first."
        )
    );
    assert!(agentfile::path(&w.config, "test-runner").exists());
    // A built-in is never deleted.
    key(&mut app, KeyCode::Home);
    key(&mut app, KeyCode::Char('d'));
    assert!(
        app.active_notice().unwrap().starts_with("planner is built into platoon, so it can't be deleted."),
        "{:?}",
        app.active_notice()
    );
    // No longer used: deleted after asking, kept as a .bak.
    squadfile::remove(&w.config, "careful-coding").unwrap();
    squadfile::remove(&w.config, "coding").unwrap();
    app.refresh_for_test();
    key(&mut app, KeyCode::End);
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Char('d'));
    assert!(matches!(app.mode, Mode::Confirm(Confirm::DeleteAgent { .. })));
    assert!(flat(&draw(&mut term, &app)).contains("Delete the agent \"Test runner\"?"));
    key(&mut app, KeyCode::Char('n'));
    assert!(agentfile::path(&w.config, "test-runner").exists());
    key(&mut app, KeyCode::Char('d'));
    key(&mut app, KeyCode::Char('y'));
    assert!(
        !agentfile::path(&w.config, "test-runner").exists()
            && agentfile::path(&w.config, "test-runner").with_extension("json.bak").exists()
    );
    assert_eq!(app.active_notice(), Some("Deleted the agent \"Test runner\" (kept as test-runner.json.bak in the agents folder)."));
    assert!(!app.sqt.agents.iter().any(|l| l.id == "test-runner"));
}

const DRAFT: &str = r#"{"name":"Sentry triage","can_do":"Reads a Sentry error, reproduces it and fixes it.",
 "new_agents":[
  {"name":"reader","model":"haiku","can_change_files":false,"can_run_commands":false,"can_use_web":true,"guidance":"Read the error."}],
 "workflow":[
  {"agent":"reader","does":"Reads the error and its stack.","back_to":null,"times":0},
  {"agent":"builder","does":"Fixes it and runs the tests.","back_to":null,"times":0},
  {"agent":"reader","does":"Checks the fix against the error.","back_to":2,"times":1}]}"#;

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
    assert!(
        f.contains("What to describe for Claude to draft:") && f.contains("▎ A new squad") && f.contains("A new agent"),
        "on the Squads list, a squad first: {f}"
    );
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Char('n'));
    assert_eq!(app.sqt.choice.options[app.sqt.choice.selected].1, "agent", "on the Agents list, an agent first");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::SquadText));
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("describe a new agent") && f.contains("enter ask Claude to draft it"), "{f}");
    typed(&mut app, "writes failing tests first");
    key(&mut app, KeyCode::Enter);
    assert!(app.squad_drafting());
    assert!(flat(&draw(&mut term, &app)).contains("Claude is drafting an agent…"));
    app.refresh_until_for_test(20, "the draft", |a| !a.squad_drafting());
    assert_eq!(fake.calls(), 1);
    let argv = fake.argv(1);
    assert_eq!(argv[argv.iter().position(|a| a == "--model").unwrap() + 1], "sonnet");
    assert!(fake.has_arg(1, &agentfile::draft_system(agentfile::DRAFT_GUIDANCE)));
    assert!(fake.stdin(1).contains("The agent the user wants, in their words:\nwrites failing tests first"), "{}", fake.stdin(1));
    let day = crate::ledger::Ledger::open(app.cache_for_test()).load();
    assert_eq!(
        day.days.values().next().unwrap().features.get(agentfile::DRAFT_FEATURE).map(|f| f.calls),
        Some(1),
        "its tokens are counted"
    );
    assert!(app.sqt.agent_draft_selected() && app.sqt.focus == SqFocus::Detail && app.sqt.view == SqView::Agents);
    assert!(app.active_notice().unwrap().starts_with("Here is a draft of the agent \"test-writer\"."), "{:?}", app.active_notice());
    app.notice = None;
    let shown = draw(&mut term, &app);
    assert!(
        left(&shown).contains("Draft, not saved ▎ test-writer Sonnet")
            && right(&shown).contains("Can change files Yes")
            && flat(&shown).contains("ctrl+s save draft"),
        "{shown}"
    );
    assert!(!agentfile::path(&w.config, "test-writer").exists(), "not saved yet");
    // Changed before it is saved: in memory only.
    go_arow(&mut app, ARow::Model);
    key(&mut app, KeyCode::Enter);
    choose(&mut app, "Opus");
    assert!(!agentfile::path(&w.config, "test-writer").exists());
    // A second n finds the draft.
    key(&mut app, KeyCode::Char('n'));
    choose(&mut app, "A new agent");
    assert_eq!(app.active_notice(), Some("There is a draft agent already: ctrl+s saves it, d discards it."));
    ctrl(&mut app, 's');
    assert_eq!(app.active_notice(), Some("Saved the agent \"test-writer\". A squad's step can use it now (enter on a step's agent)."));
    let v = agent_file(&w, "test-writer");
    assert_eq!((v["model"].as_str(), v["can_change_files"].as_bool()), (Some("opus"), Some(true)));
    assert!(app.sqt.agent_draft.is_none());
    assert_eq!(app.sqt.agent_listed().map(|l| (l.id.as_str(), l.kind)), Some(("test-writer", Kind::Own)));
    // One that can't be used says why.
    fake.reply_default(&AGENT_DRAFT.replace("\"sonnet\"", "\"gpt\""));
    key(&mut app, KeyCode::Char('n'));
    choose(&mut app, "A new agent");
    assert_eq!(app.sqt.text.text, "writes failing tests first", "what was described is offered again");
    key(&mut app, KeyCode::Enter);
    app.refresh_until_for_test(20, "the draft", |a| !a.squad_drafting());
    assert_eq!(
        app.active_notice(),
        Some(
            "Claude's draft couldn't be used: test-writer's model \"gpt\" isn't one platoon offers (Opus, Sonnet or Haiku). Press n to try again."
        )
    );
    // Discarding a draft agent asks.
    fake.reply_default(AGENT_DRAFT);
    key(&mut app, KeyCode::Char('n'));
    choose(&mut app, "A new agent");
    key(&mut app, KeyCode::Enter);
    app.refresh_until_for_test(20, "the draft", |a| !a.squad_drafting());
    assert_eq!(app.sqt.agent_draft.as_ref().map(|a| a.id.as_str()), Some("test-writer-2"), "its own id");
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Char('d'));
    assert!(matches!(app.mode, Mode::Confirm(Confirm::DiscardAgentDraft { .. })));
    key(&mut app, KeyCode::Char('y'));
    assert!(app.sqt.agent_draft.is_none());
}

#[test]
fn describe_drafts_a_squad_that_reuses_agents_and_saves_its_new_ones_with_it() {
    let (w, mut app, fake) = with_claude("sqt-draft");
    fake.reply_default(DRAFT);
    key(&mut app, KeyCode::Char('n'));
    choose(&mut app, "A new squad");
    assert!(matches!(app.mode, Mode::SquadText));
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(
        f.contains("describe a new squad") && f.contains("enter ask Claude to draft it") && f.contains("using your agents where they fit"),
        "{f}"
    );
    typed(&mut app, "a squad that triages Sentry errors: a reader and a fixer");
    key(&mut app, KeyCode::Enter);
    assert!(app.squad_drafting());
    assert!(flat(&draw(&mut term, &app)).contains("Claude is drafting a squad…"));
    app.refresh_until_for_test(20, "the draft", |a| !a.squad_drafting());
    assert_eq!(fake.calls(), 1);
    let argv = fake.argv(1);
    assert_eq!(argv[argv.iter().position(|a| a == "--model").unwrap() + 1], "sonnet");
    assert!(fake.has_arg(1, &squadfile::draft_system(squadfile::DRAFT_GUIDANCE)));
    let asked = fake.stdin(1);
    assert!(asked.contains("a squad that triages Sentry errors: a reader and a fixer"), "{asked}");
    assert!(
        asked.contains("- builder (Sonnet; changes files, runs commands): Make the change the plan describes"),
        "the agents there are, to reuse: {asked}"
    );
    let day = crate::ledger::Ledger::open(app.cache_for_test()).load();
    assert_eq!(
        day.days.values().next().unwrap().features.get(squadfile::DRAFT_FEATURE).map(|f| f.calls),
        Some(1),
        "its tokens are counted"
    );
    assert!(app.sqt.draft_selected() && app.sqt.focus == SqFocus::Detail);
    assert_eq!(
        app.active_notice(),
        Some(
            "Here is a draft of \"Sentry triage\". It brings 1 new agent, saved with it. Change what you like: ctrl+s saves it, d discards it."
        )
    );
    app.notice = None;
    go(&mut app, DRow::StepAgent(1));
    let shown = draw(&mut term, &app);
    let f = right(&shown);
    assert!(left(&shown).contains("Sentry triage 2 agents Draft, not saved") && flat(&shown).contains("ctrl+s save draft"), "{shown}");
    assert!(
        f.contains("○ 1 reader Haiku web · new with the draft") && f.contains("● 2 builder Sonnet files · commands Fixes it"),
        "which are new and which are there already: {f}"
    );
    assert!(!squadfile::path(&w.config, "sentry-triage").exists() && !agentfile::path(&w.config, "reader").exists(), "not saved yet");
    // g on a new one says where it will be.
    go(&mut app, DRow::StepAgent(0));
    key(&mut app, KeyCode::Char('g'));
    assert_eq!(app.active_notice(), Some("reader comes with the draft: it is saved with it (ctrl+s), and then it is on the Agents list."));
    // Changed before it is saved: in memory only.
    go(&mut app, DRow::StepAgent(1));
    key(&mut app, KeyCode::Enter);
    choose(&mut app, "reviewer ·");
    assert!(!squadfile::dir(&w.config).join("sentry-triage.json").exists());
    ctrl(&mut app, 's');
    assert_eq!(app.active_notice(), Some("Saved the squad \"Sentry triage\" and its new agent. Tickets can be handed to it now."));
    let v = file(&w, "sentry-triage");
    assert!(
        v.get("members").is_none() && v.get("leader").is_none() && v.get("needs_folder").is_none() && v.get("use_worktree").is_none(),
        "{v}"
    );
    assert_eq!(
        (v["workflow"][1]["agent"].as_str(), v["workflow"][0]["agent"].as_str(), v["workflow"][2]["back_to"].as_u64()),
        (Some("reviewer"), Some("reader"), Some(2))
    );
    assert_eq!(agent_file(&w, "reader")["guidance"], "Read the error.");
    assert!(app.sqt.draft.is_none() && app.sqt.draft_agents.is_empty());
    assert_eq!(app.sqt.listed().map(|l| (l.id.as_str(), l.kind)), Some(("sentry-triage", Kind::Own)));
    assert!(flat(&draw(&mut term, &app)).contains("3 Squads 5"));
    assert!(app.sqt.agents.iter().any(|l| l.id == "reader" && l.kind == Kind::Own));
}

#[test]
fn a_draft_that_cant_be_used_says_why_and_n_offers_the_description_again() {
    let (w, mut app, fake) = with_claude("sqt-baddraft");
    fake.reply_default(&DRAFT.replace(r#""agent":"builder""#, r#""agent":"mender""#));
    key(&mut app, KeyCode::Char('n'));
    key(&mut app, KeyCode::Enter);
    typed(&mut app, "a triage squad");
    key(&mut app, KeyCode::Enter);
    app.refresh_until_for_test(20, "the draft", |a| !a.squad_drafting());
    assert_eq!(
        app.active_notice(),
        Some("Claude's draft couldn't be used: Step 2 names an agent that isn't there (mender). Press n to try again.")
    );
    assert!(app.sqt.draft.is_none() && !squadfile::dir(&w.config).exists() && !agentfile::path(&w.config, "reader").exists());
    key(&mut app, KeyCode::Char('n'));
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.sqt.text.text, "a triage squad", "what was described is offered again");
    key(&mut app, KeyCode::Esc);
    // A draft is thrown away only after asking, with the agents it brought.
    fake.reply_default(DRAFT);
    key(&mut app, KeyCode::Char('n'));
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Enter);
    app.refresh_until_for_test(20, "the draft", |a| !a.squad_drafting());
    assert!(app.sqt.draft_selected());
    key(&mut app, KeyCode::Char('n'));
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.active_notice(), Some("There is a draft already: ctrl+s saves it, d discards it."));
    key(&mut app, KeyCode::Char('d'));
    assert!(matches!(app.mode, Mode::Confirm(Confirm::DiscardDraft { .. })));
    key(&mut app, KeyCode::Char('n'));
    assert!(app.sqt.draft.is_some());
    key(&mut app, KeyCode::Char('d'));
    key(&mut app, KeyCode::Char('y'));
    assert!(app.sqt.draft.is_none() && app.sqt.draft_agents.is_empty() && !squadfile::dir(&w.config).join("sentry-triage.json").exists());
    // With the AI features off, nothing is asked.
    let mut s = crate::settings::load(&w.config).0;
    s.tokens_enabled = false;
    crate::settings::save(&w.config, &s).unwrap();
    app.refresh_for_test();
    key(&mut app, KeyCode::Char('n'));
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Enter);
    assert!(
        !app.squad_drafting() && app.active_notice().unwrap().starts_with("Couldn't draft a squad: AI features are switched off."),
        "{:?}",
        app.active_notice()
    );
    assert_eq!(fake.calls(), 2);
}

#[test]
fn steps_are_added_moved_given_an_agent_and_a_send_back() {
    let (w, mut app) = on_the_tab("sqt-steps");
    go(&mut app, DRow::AddStep);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.sqt.choice.options.last().map(|(l, _)| l.as_str()), Some("+ New agent"), "a new step can have a new agent too");
    choose(&mut app, "reviewer ·");
    assert!(matches!(app.mode, Mode::SquadText));
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    assert!(flat(&draw(&mut term, &app)).contains("a new step for reviewer"));
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::SquadText), "an empty step isn't added");
    typed(&mut app, "Reads the whole change once more.");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.sqt.drow(), Some(DRow::Step(4)), "on the new step");
    let steps = |w: &World| squadfile::get(&w.config, "coding").unwrap().workflow;
    assert_eq!(steps(&w)[4].agent, "reviewer");
    // Up one: it is step 4 now, before the verify.
    key(&mut app, KeyCode::Char('['));
    assert_eq!(app.sqt.drow(), Some(DRow::Step(3)));
    assert_eq!(steps(&w).iter().map(|s| s.agent.as_str()).collect::<Vec<_>>(), ["planner", "builder", "reviewer", "reviewer", "verifier"]);
    assert_eq!(steps(&w)[4].back, Some(Back { to: 1, times: 1 }), "the verify still goes back to the build");
    // It may send the work back to step 2, three times.
    key(&mut app, KeyCode::Char('b'));
    let f = flat(&draw(&mut term, &app));
    assert!(
        f.contains("Never sends the work back")
            && f.contains("Back to step 1: planner")
            && f.contains("Back to step 3: reviewer")
            && !f.contains("Back to step 4"),
        "only earlier steps: {f}"
    );
    choose(&mut app, "Back to step 2");
    choose(&mut app, "At most 3 times");
    assert_eq!(steps(&w)[3].back, Some(Back { to: 1, times: 3 }));
    assert_eq!(app.active_notice(), Some("Step 4 may send it back to step 2, at most 3 times now."));
    // Another agent does it.
    key(&mut app, KeyCode::Char('m'));
    choose(&mut app, "planner ·");
    assert_eq!(steps(&w)[3].agent, "planner");
    // What it does, changed.
    key(&mut app, KeyCode::Enter);
    ctrl(&mut app, 'u');
    typed(&mut app, "Checks the plan was followed.");
    key(&mut app, KeyCode::Enter);
    assert_eq!(steps(&w)[3].does, "Checks the plan was followed.");
    // The leader gets it all, in order, with the send-back.
    let crew = Crew::load(&w.config, &squadfile::get(&w.config, "coding").unwrap());
    let prompt = crate::squads::leader_prompt(&crew, &LeaderSetup::builtin(), AgentNames::SquadRole, crate::squads::Place::WORKTREE);
    assert!(prompt.contains("4. coding-planner: Checks the plan was followed. It may send the work back: then give its reasons to coding-builder and go on again from step 2. At most 3 times;"), "{prompt}");
    assert!(prompt.contains("5. coding-verifier: "), "{prompt}");
    // Moving a step above the one it sends back to drops that send-back, and says so.
    key(&mut app, KeyCode::Char('['));
    key(&mut app, KeyCode::Char('['));
    assert_eq!(app.sqt.drow(), Some(DRow::Step(1)));
    assert!(app.active_notice().unwrap().contains("doesn't any more"), "{:?}", app.active_notice());
    assert!(squadfile::problems(&squadfile::get(&w.config, "coding").unwrap(), &agentfile::builtins()).is_empty());
    // The first step has nowhere to send it back to; a step taken out is gone.
    go(&mut app, DRow::Step(0));
    key(&mut app, KeyCode::Char('b'));
    assert_eq!(app.active_notice(), Some("The first step has no step before it to send the work back to."));
    key(&mut app, KeyCode::Char('x'));
    assert_eq!(steps(&w).len(), 4);
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
    assert!(matches!(app.mode, Mode::EditNotes));
    wait_for_editor(&mut app);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(
        app.active_notice(),
        Some("Saved builder's system prompt, in every squad that uses it (Coding), from the next ticket each is handed.")
    );
    assert_eq!(agent_file(&w, "builder")["guidance"], "Build it in small commits.");
    assert!(!w.cache.join("squad-prompts/agent-builder.md").exists(), "the file it was edited in is gone");
    assert!(!squadfile::path(&w.config, "coding").exists());
    // An editor that leaves it as it was changes nothing and says nothing.
    app.notice = None;
    app.set_editors_for_test(vec![fake_editor(&w, "idle", "true")]);
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Enter);
    wait_for_editor(&mut app);
    assert_eq!(app.active_notice(), None);
    assert_eq!(agent_file(&w, "builder")["guidance"], "Build it in small commits.");
    // The squad has no leader's system prompt of its own to edit: its rows are About and the workflow.
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Enter);
    let rows = crate::app::detail_rows(app.sqt.squad().unwrap());
    assert_eq!(&rows[..4], [DRow::Name, DRow::CanDo, DRow::StepAgent(0), DRow::Step(0)]);
    assert_eq!(rows.last(), Some(&DRow::AddStep));
}

#[test]
fn copy_delete_and_restore_each_ask_first_and_a_built_in_is_never_deleted() {
    let (w, mut app) = on_the_tab("sqt-copy");
    key(&mut app, KeyCode::Char('d'));
    assert!(matches!(app.mode, Mode::List));
    assert!(app.active_notice().unwrap().starts_with("Coding is built into platoon, so it can't be deleted."), "{:?}", app.active_notice());
    key(&mut app, KeyCode::Char('c'));
    assert!(matches!(app.mode, Mode::ProjectName));
    assert_eq!(app.name_box.input, "Coding copy");
    ctrl(&mut app, 'u');
    typed(&mut app, "Careful coding");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.sqt.listed().map(|l| (l.id.as_str(), l.kind)), Some(("careful-coding", Kind::Own)));
    assert!(app.active_notice().unwrap().starts_with("Made \"Careful coding\", a copy of Coding."));
    assert_eq!(file(&w, "careful-coding")["workflow"].as_array().unwrap().len(), 4);
    // Deleting it asks; no leaves it.
    key(&mut app, KeyCode::Char('d'));
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("Delete the squad \"Careful coding\"?") && f.contains("careful-coding.json.bak"), "{f}");
    key(&mut app, KeyCode::Char('n'));
    assert!(squadfile::path(&w.config, "careful-coding").exists());
    key(&mut app, KeyCode::Char('d'));
    key(&mut app, KeyCode::Char('y'));
    assert!(
        !squadfile::path(&w.config, "careful-coding").exists()
            && squadfile::path(&w.config, "careful-coding").with_extension("json.bak").exists()
    );
    assert_eq!(app.squad_list().len(), 4);
    // A changed built-in is restored, after asking.
    key(&mut app, KeyCode::Char('g'));
    go(&mut app, DRow::StepAgent(3));
    key(&mut app, KeyCode::Char('m'));
    choose(&mut app, "reviewer ·");
    assert!(squadfile::path(&w.config, "coding").exists());
    key(&mut app, KeyCode::Char('d'));
    assert!(matches!(app.mode, Mode::Confirm(Confirm::RestoreSquad { .. })));
    assert!(flat(&draw(&mut term, &app)).contains("Put Coding back as platoon has it?"));
    key(&mut app, KeyCode::Char('y'));
    assert_eq!(app.squad_list()[0].kind, Kind::BuiltIn);
    assert!(app.active_notice().unwrap().starts_with("Coding is as platoon has it again."));
    // An agent is copied at once, as one of the user's own, used by no squad.
    go_agent(&mut app, "reviewer");
    key(&mut app, KeyCode::Char('c'));
    assert_eq!(app.name_box.input, "reviewer copy");
    ctrl(&mut app, 'u');
    typed(&mut app, "Strict reviewer");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.sqt.agent_listed().map(|l| (l.id.as_str(), l.kind)), Some(("strict-reviewer", Kind::Own)));
    assert_eq!(agent_file(&w, "strict-reviewer")["guidance"], crate::squads::REVIEWER_GUIDANCE);
    assert_eq!(app.active_notice(), Some("Made \"Strict reviewer\", a copy of reviewer. It is yours to change; no squad uses it yet."));
}

#[test]
fn a_file_that_cant_be_read_is_shown_as_such_and_left_alone() {
    let w = World::new("sqt-corrupt");
    fs::create_dir_all(squadfile::dir(&w.config)).unwrap();
    fs::write(squadfile::path(&w.config, "mine"), "{ nope").unwrap();
    fs::create_dir_all(agentfile::dir(&w.config)).unwrap();
    fs::write(agentfile::path(&w.config, "helper"), "[1]").unwrap();
    let mut app = w.app();
    key(&mut app, KeyCode::Char('3'));
    key(&mut app, KeyCode::Char('G'));
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(
        f.contains("mine Can't be read") && f.contains("mine.json can't be read (it isn't valid JSON), so platoon leaves it alone."),
        "{f}"
    );
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.sqt.focus, SqFocus::List, "nothing to change");
    key(&mut app, KeyCode::Char('c'));
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(fs::read_to_string(squadfile::path(&w.config, "mine")).unwrap(), "{ nope");
    // An agent's file too.
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::End);
    key(&mut app, KeyCode::Up);
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("helper can't be read") && f.contains("helper.json can't be read (it isn't a JSON object)"), "{f}");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.sqt.focus, SqFocus::List);
    key(&mut app, KeyCode::Char('d'));
    assert!(matches!(app.mode, Mode::Confirm(Confirm::DeleteAgent { .. })), "one of the user's own, used by no squad");
    key(&mut app, KeyCode::Char('n'));
    assert_eq!(fs::read_to_string(agentfile::path(&w.config, "helper")).unwrap(), "[1]");
}

#[test]
fn a_on_a_ticket_lists_every_squad_by_name_and_hands_it_to_the_users_own() {
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
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Char('a'));
    assert!(matches!(app.mode, Mode::SquadPick));
    let names: Vec<&str> = app.sq.pick.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        ["Coding", "Research", "Docs", "Summarize", "Broken", "Careful coding"],
        "every squad, by name: built-ins first, then the user's own"
    );
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("▎ Coding") && !f.contains("Careful coding"), "six squads don't fit in 30 rows: {f}");
    for _ in 0..5 {
        key(&mut app, KeyCode::Down);
    }
    let f = flat(&draw(&mut term, &app));
    assert!(
        f.contains("▎ Careful coding") && f.contains("Needs fixing on the Squads tab first. Makes a change") && !f.contains("▎ Coding"),
        "{f}"
    );
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::SquadPick));
    assert_eq!(
        app.active_notice(),
        Some("The Broken squad needs fixing on the Squads tab first: Step 1 names an agent that isn't there (nobody).")
    );
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    app.refresh_until_for_test(20, "the leader to start", |a| !a.sq.launched.is_empty());
    assert!(
        fake.stdin(1).starts_with("The squad: Careful coding\nWhat it can do: "),
        "the commander reads its name and what it can do: {}",
        fake.stdin(1)
    );
    let t = Tickets::open(&w.config, &id).get(1).cloned().unwrap();
    let leader = t.leader().unwrap().clone();
    assert_eq!((leader.squad.as_str(), leader.snapshot.as_deref()), ("careful-coding", Some("01-squad-1.json")));
    let launch = app.leader_launch(&id, &leader, &t).unwrap();
    assert_eq!((launch.agent.as_str(), launch.model.as_str(), launch.name.as_str()), ("careful-coding-leader", "sonnet", "Careful coding"));
    let agents: Value = serde_json::from_str(&launch.agents).unwrap();
    assert_eq!(agents["careful-coding-slow-verifier"]["model"], "sonnet");
    assert!(agents.get("careful-coding-verifier").is_none());
    assert_eq!(app.squad_name("careful-coding"), "Careful coding");
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
    );
    let snap = store.save_snapshot(n, 1, &at_start.to_json()).unwrap();
    let with = Leader { squad: "coding".into(), session: "s-1".into(), started: 1, names: AgentNames::SquadRole, snapshot: Some(snap) };
    // Its agents change afterwards.
    let mut verifier = agentfile::builtin("verifier").unwrap();
    verifier.model = "sonnet".into();
    agentfile::save(&w.config, &verifier).unwrap();
    let mut planner = agentfile::builtin("planner").unwrap();
    planner.guidance = "Plan in one line.".into();
    agentfile::save(&w.config, &planner).unwrap();
    let t = store.get(n).cloned().unwrap();
    let resumed = app.leader_launch("p1", &with, &t).unwrap();
    assert_eq!(resumed, at_start, "the snapshot, not the agents as they are now");
    let agents: Value = serde_json::from_str(&resumed.agents).unwrap();
    assert_eq!(agents["coding-verifier"]["model"], "haiku");
    // A leader from before snapshots (the user's ticket #1) continues as before: the squad and its agents as they are
    // now, under the plain names it began with.
    let old = Leader { squad: "coding".into(), session: "s-0".into(), started: 1, names: AgentNames::Plain, snapshot: None };
    let now = app.leader_launch("p1", &old, &t).unwrap();
    let agents: Value = serde_json::from_str(&now.agents).unwrap();
    assert_eq!((now.agent.as_str(), agents["verifier"]["model"].as_str()), ("leader", Some("sonnet")));
    assert!(agents["planner"]["prompt"].as_str().unwrap().starts_with("Plan in one line."));
    // A snapshot that has gone: said, not guessed.
    let gone = Leader { snapshot: Some("01-squad-9.json".into()), ..with.clone() };
    assert_eq!(app.leader_launch("p1", &gone, &t).unwrap_err(), "what it was started with can't be read any more");
}

#[test]
fn the_workflow_rail_draws_codings_send_backs_as_nested_loops_in_muted_colours() {
    let (_w, mut app) = on_the_tab("sqt-rail");
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let shown = draw(&mut term, &app);
    // Both send-backs go to the build: the verify's (the longer) outside, the review's inside it, each with its words.
    for piece in [
        "○  1 planner",
        "┌─▶ ●  2 builder",
        "│┌▶ │",
        "││  ●  3 reviewer",
        "│└──┤",
        "│   ●  4 verifier",
        "└───┤",
        "◉  Done: the result goes on the ticket",
        "+  Add a step",
    ] {
        assert!(shown.contains(piece), "{piece}: {shown}");
    }
    let row_of = |text: &str| shown.lines().position(|l| l.contains(text)).unwrap_or_else(|| panic!("{text}"));
    assert!(shown.lines().nth(row_of("│└──┤")).unwrap().contains("↩ back to step 2 (builder), at most twice"));
    assert!(shown.lines().nth(row_of("└───┤")).unwrap().contains("↩ back to step 2 (builder), at most once"));
    assert!(
        row_of("○  1 planner") < row_of("┌─▶ ●  2 builder")
            && row_of("┌─▶ ●  2 builder") < row_of("│└──┤")
            && row_of("│└──┤") < row_of("└───┤")
            && row_of("└───┤") < row_of("◉  Done")
    );
    // The rail and the loops are muted; nothing on the details has the cursor yet.
    let t = app.effective_theme().clone();
    let (x, y) = find(&term, "┌─▶ ●");
    for dx in 0..3 {
        assert_eq!(fg(&term, (x + dx, y)), t.dim, "the loop at {dx}");
    }
    assert_eq!(fg(&term, (x + 4, y)), t.subtext, "a step's mark");
    assert_eq!(bg(&term, (x + 4, y)), t.bg);
    let (x, y) = find(&term, "│└──┤");
    assert_eq!((fg(&term, (x, y)), fg(&term, (x + 4, y))), (t.dim, t.dim));
    assert_eq!(fg(&term, find(&term, "↩ back to step 2 (builder), at most twice")), t.subtext);
    // The cursor on the review's agent: its line on the surface, its mark in the accent; its "does" line not.
    go(&mut app, DRow::StepAgent(2));
    draw(&mut term, &app);
    let (x, y) = find(&term, "3 reviewer");
    let (mx, _) = find(&term, "││  ●  3 reviewer");
    assert_eq!((bg(&term, (x, y)), fg(&term, (mx + 4, y)), bg(&term, (mx, y))), (t.surface, t.accent, t.surface));
    assert_eq!(fg(&term, (mx - 2, y)), t.accent, "the cursor's bar");
    assert_eq!(fg(&term, (mx, y)), t.dim, "the loop keeps its colour on the surface");
    assert_eq!(bg(&term, (x, y + 1)), t.bg, "what it does is a row of its own");
    key(&mut app, KeyCode::Down);
    draw(&mut term, &app);
    assert_eq!(app.sqt.drow(), Some(DRow::Step(2)));
    assert_eq!((bg(&term, (x, y)), bg(&term, (x, y + 1))), (t.bg, t.surface));
    assert_eq!(fg(&term, (mx + 4, y)), t.accent, "the step's mark stays lit while either of its rows has the cursor");
    // Enter on what it does edits it; on its agent picks one.
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::SquadText));
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::SquadChoice));
    key(&mut app, KeyCode::Esc);
    // The last row: "+ Add a step", lit when the cursor is there.
    key(&mut app, KeyCode::End);
    draw(&mut term, &app);
    assert_eq!(app.sqt.drow(), Some(DRow::AddStep));
    let (x, y) = find(&term, "+  Add a step");
    assert_eq!((fg(&term, (x, y)), bg(&term, (x, y))), (t.accent, t.surface));
}

/// A squad of twelve steps with five send-backs: some nested, one crossing another.
fn twelve_steps(w: &World) -> squadfile::Squad {
    let mut s = squadfile::copy_of(&w.config, &squadfile::builtin("coding").unwrap(), "Long haul", &[]);
    let agents = ["planner", "builder", "reviewer", "verifier"];
    s.workflow = (0..12)
        .map(|i| squadfile::Step {
            agent: agents[i % 4].into(),
            does: format!("Does part {} of the long haul, carefully and completely, then reports back.", i + 1),
            back: None,
            extra: Default::default(),
        })
        .collect();
    for (from, to, times) in [(11, 0, 1), (10, 1, 2), (7, 2, 3), (5, 3, 1), (8, 6, 2)] {
        s.workflow[from].back = Some(Back { to, times });
    }
    s
}

#[test]
fn twelve_steps_with_five_send_backs_scroll_at_80_columns() {
    let w = World::new("sqt-rail-12");
    let s = twelve_steps(&w);
    squadfile::save(&w.config, &s).unwrap();
    let mut app = w.app();
    key(&mut app, KeyCode::Char('3'));
    key(&mut app, KeyCode::End);
    assert_eq!(app.sqt.listed().map(|l| l.id.as_str()), Some("long-haul"));
    let mut term = Terminal::new(TestBackend::new(80, 30)).unwrap();
    let shown = draw(&mut term, &app);
    assert!(right(&shown).contains("Workflow"), "{shown}");
    // Down the rail to its end: it scrolls, and every step shows on the way with its loops beside it.
    key(&mut app, KeyCode::Enter);
    let mut seen = String::new();
    for _ in 0..30 {
        key(&mut app, KeyCode::Down);
        seen.push_str(&draw(&mut term, &app));
    }
    assert_eq!(app.sqt.drow(), Some(DRow::AddStep));
    for n in 1..=12 {
        assert!(seen.contains(&format!(" {n:>2} ")), "step {n}: {seen}");
    }
    let last = draw(&mut term, &app);
    assert!(
        last.contains("◉  Done") && last.contains("+  Add a step") && seen.contains("▶") && seen.contains("┼"),
        "the loops drawn, one crossing another: {last}"
    );
    // The words beside a loop: as much as fits in the width, never cut halfway.
    assert!(
        seen.contains("↩ back to step 1, at most once")
            && seen.contains("↩ back to step 7, twice")
            && !seen.contains("↩ back to step 1 (planner), a…"),
        "{seen}"
    );
    // Nothing runs past the pane: every line of the details stops at its right border.
    for l in last.lines().skip(3).take(25) {
        assert!(l.chars().count() <= 80, "{l}");
        assert!(l.ends_with('│') || l.ends_with('╮') || l.ends_with('╯') || l.trim().is_empty() || !l.contains('╭'), "{l}");
    }
}

#[test]
fn an_agent_has_skills_and_mcp_servers_picked_from_what_is_installed_or_typed() {
    let w = World::new("sqt-tools");
    // What is installed "here": a skill in Claude's config folder, two servers in its user config (a temp folder).
    fs::create_dir_all(w.root.join("skills/pdf")).unwrap();
    fs::write(w.root.join("skills/pdf/SKILL.md"), "---\nname: pdf\ndescription: PDFs\n---\n").unwrap();
    fs::write(w.root.join(".claude.json"), r#"{"mcpServers":{"sentry":{"command":"npx","env":{"TOKEN":"secret-token"}},"github":{}}}"#)
        .unwrap();
    let mut app = w.app();
    key(&mut app, KeyCode::Char('3'));
    go_agent(&mut app, "builder");
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let f = right(&draw(&mut term, &app));
    assert!(
        f.contains("Skills None: enter picks from what is installed") && f.contains("MCP servers None: enter picks from what is installed"),
        "{f}"
    );
    // Skills: the installed ones, and typing one. Enter ticks one and the list stays open.
    go_arow(&mut app, ARow::Skills);
    assert!(flat(&draw(&mut term, &app)).contains("enter pick"));
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::SquadChoice));
    let labels: Vec<&str> = app.sqt.choice.options.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(labels, ["[ ] pdf", "Type the name of a skill…"]);
    choose(&mut app, "[ ] pdf");
    assert!(matches!(app.mode, Mode::SquadChoice), "still open");
    assert_eq!(app.sqt.choice.options[0].0, "[x] pdf");
    assert_eq!(app.active_notice(), Some("builder has the skill pdf now, in every squad that uses it (Coding)."));
    key(&mut app, KeyCode::Esc);
    // MCP servers: one picked, one typed that isn't installed here.
    go_arow(&mut app, ARow::Mcp);
    key(&mut app, KeyCode::Enter);
    let labels: Vec<&str> = app.sqt.choice.options.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(labels, ["[ ] github", "[ ] sentry", "Type the name of an MCP server…"], "names only, sorted");
    assert!(!flat(&draw(&mut term, &app)).contains("secret-token"), "what a server is, is never shown");
    choose(&mut app, "[ ] sentry");
    choose(&mut app, "Type the name");
    assert!(matches!(app.mode, Mode::ProjectName));
    assert!(flat(&draw(&mut term, &app)).contains("an MCP server"));
    typed(&mut app, "my server");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::ProjectName), "a space can't be in the name");
    assert!(app.active_notice().unwrap().starts_with("That can't be a name"), "{:?}", app.active_notice());
    ctrl(&mut app, 'u');
    typed(&mut app, "linear");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));
    assert!(app.active_notice().unwrap().starts_with("builder has the MCP server linear now (not found here"), "{:?}", app.active_notice());
    let v = agent_file(&w, "builder");
    assert_eq!((v["skills"].clone(), v["mcp_servers"].clone()), (serde_json::json!(["pdf"]), serde_json::json!(["sentry", "linear"])));
    // Shown on the agent's screen; one that isn't installed here says so, in the warning colour.
    let shown = draw(&mut term, &app);
    let f = right(&shown);
    assert!(f.contains("Skills pdf") && f.contains("MCP servers sentry, linear (not found here)"), "{f}");
    let t = app.effective_theme().clone();
    assert_eq!(fg(&term, find(&term, "sentry, linear")), t.warn);
    assert_eq!(fg(&term, find(&term, "Skills")), t.dim);
    // Not found here is shown in the picker too, ticked, and can be taken away there.
    key(&mut app, KeyCode::Enter);
    let labels: Vec<&str> = app.sqt.choice.options.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(labels, ["[ ] github", "[x] sentry", "[x] linear · not found here", "Type the name of an MCP server…"]);
    choose(&mut app, "[x] linear");
    key(&mut app, KeyCode::Esc);
    assert_eq!(agentfile::get(&w.config, "builder").unwrap().mcp, ["sentry"]);
    // A squad using it starts it with them: preloaded skills, servers by name, the tools that let it use them.
    let crew = Crew::load(&w.config, &squadfile::builtin("coding").unwrap());
    let agents: Value =
        serde_json::from_str(&Launch::of(&crew, &LeaderSetup::builtin(), AgentNames::SquadRole, crate::squads::Place::WORKTREE).agents)
            .unwrap();
    let b = &agents["coding-builder"];
    assert_eq!((b["skills"].clone(), b["mcpServers"].clone()), (serde_json::json!(["pdf"]), serde_json::json!(["sentry"])));
    assert_eq!(b["tools"], serde_json::json!(["Read", "Grep", "Glob", "Edit", "Write", "Bash", "Skill", "mcp__sentry"]));
    // The rail shows it has them.
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Left);
    assert!(flat(&draw(&mut term, &app)).contains("● 2 builder Sonnet files · commands · skills · MCP"));
    // Nothing is written to Claude's own files.
    assert_eq!(
        fs::read_to_string(w.root.join(".claude.json")).unwrap(),
        r#"{"mcpServers":{"sentry":{"command":"npx","env":{"TOKEN":"secret-token"}},"github":{}}}"#
    );
}
