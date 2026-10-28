//! Switching Claude Code or Codex off (Settings, General, Programs), at the level of the app: the two switches on the
//! settings screen and what they refuse, a program switched off left out as if it weren't installed (its rows, its plan
//! usage in the header, `n`'s question, its models), what is set to one of its models said plainly and refused, and a
//! running task of it kept on the Tasks tab, marked, and still opened and cancelled. Every folder is a temp dir
//! (`World`); nothing is started.

use crate::agents::{Agent, CodexAt, Providers};
use crate::app::{App, Confirm, Mode};
use crate::settings;
use crate::testkit::World;
use crate::tickets::{self, NewTicket, State, Tickets};
use crate::ui;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
};
use serde_json::json;
use std::fs;

const FINISHED: &str = "01a1517c-414d-7902-93d1-efa74830493d";
const CLAUDE: &str = "cccccccc-1111-2222-3333-444444444444";

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn screen(term: &mut Terminal<TestBackend>, app: &App) -> String {
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

/// The screen as one line, spaces squeezed: for words a narrow popup may wrap.
fn flat(text: &str) -> String {
    text.split(|c: char| c.is_whitespace() || c == '│').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

/// The sample Codex conversations (with Codex's models) and one of Claude Code's: an app with both programs.
fn both(name: &str) -> (World, App) {
    let w = World::new(name);
    w.codex_samples();
    fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/src/testdata/codex/models_cache.json"), w.codex().join("models_cache.json")).unwrap();
    w.transcript(CLAUDE, "/work/other", "tidy the parser", 5);
    let app = w.codex_app();
    assert_eq!(app.providers().installed(), [Agent::Claude, Agent::Codex]);
    (w, app)
}

fn listed(app: &App) -> Vec<Agent> {
    let mut agents: Vec<Agent> = app.visible().iter().map(|&i| app.sessions[i].agent).collect();
    agents.dedup();
    agents.sort_by_key(|a| a.id());
    agents.dedup();
    agents
}

/// Open the settings on the General tab and select entry `i` (0: Use Claude Code, 1: Use Codex).
fn settings_on(app: &mut App, i: usize) {
    if !matches!(app.mode, Mode::Settings) {
        key(app, KeyCode::Char('s'));
    }
    while app.setting > i {
        key(app, KeyCode::Up);
    }
    while app.setting < i {
        key(app, KeyCode::Down);
    }
    assert_eq!(app.items()[app.setting].name, [settings::USE_CLAUDE_CODE, settings::USE_CODEX][i]);
}

fn on_disk(w: &World) -> serde_json::Value {
    fs::read_to_string(w.config.join("settings.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({}))
}

#[test]
fn the_settings_open_on_the_programs_each_on_off_or_not_installed_and_the_last_one_on_stays_on() {
    // Claude Code only: it can't be switched off, Codex can't be switched at all.
    let w = World::new("programs-claude-only");
    let mut app = w.app();
    let file = || fs::read_to_string(w.config.join("settings.json")).ok();
    let before = file();
    key(&mut app, KeyCode::Char('s'));
    assert_eq!(app.items()[app.setting].name, "Theme", "the settings open on Theme, below the Programs");
    // So a habitual `s` then `enter` opens the theme list, and never switches a program off.
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Themes));
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::Settings));
    let mut term = Terminal::new(TestBackend::new(100, 40)).unwrap();
    let shown = screen(&mut term, &app);
    let lines: Vec<&str> = shown.lines().collect();
    let at = |needle: &str| lines.iter().position(|l| l.contains(needle)).unwrap_or_else(|| panic!("{needle:?} missing:\n{shown}"));
    assert!(at("Programs") < at("Use Claude Code") && at("Use Claude Code") < at("Use Codex"), "{shown}");
    assert!(at("Use Codex") < at("Everything else") && at("Everything else") < at("Theme"), "{shown}");
    assert!(lines[at("Use Claude Code")].contains("‹") && lines[at("Use Claude Code")].contains(" On "), "{shown}");
    assert!(lines[at("Use Codex")].contains("Not installed"), "{shown}");
    settings_on(&mut app, 0);
    let shown = screen(&mut term, &app);
    assert!(flat(&shown).contains("Off: platoon leaves Claude Code out, as if it weren't installed."), "{shown}");

    key(&mut app, KeyCode::Left);
    assert_eq!(app.active_notice(), Some("Claude Code can't be switched off: Codex isn't installed, and platoon needs one of them."));
    assert_eq!(file(), before, "nothing was written");
    let shown = flat(&screen(&mut term, &app));
    assert!(shown.contains("Claude Code can't be switched off: Codex isn't installed, and platoon needs one of them."), "{shown}");
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.active_notice(), Some("Codex isn't installed on this computer, so there is nothing to switch."));
    assert_eq!(file(), before);

    // Both: Codex goes off and on, and while it is off Claude Code is the last one on.
    let (w, mut app) = both("programs-both");
    settings_on(&mut app, 1);
    key(&mut app, KeyCode::Left);
    assert_eq!(on_disk(&w)["use_codex"], json!(false));
    assert_eq!(
        app.active_notice(),
        Some("Codex is switched off: its sessions and models are left out. Anything of it already running keeps running.")
    );
    let shown = screen(&mut term, &app);
    let row = shown.lines().find(|l| l.contains("Use Codex")).unwrap();
    assert!(row.contains(" Off "), "{shown}");
    // Off reads muted, like any switch that is off.
    let t = app.effective_theme().clone();
    let buf = term.backend().buffer();
    let y = shown.lines().position(|l| l.contains("Use Codex")).unwrap() as u16;
    let x = row.find(" Off ").map(|b| row[..b].chars().count() as u16 + 1).unwrap();
    assert_ne!(buf[(x, y)].fg, t.accent, "Off isn't drawn as a live value");
    settings_on(&mut app, 0);
    key(&mut app, KeyCode::Enter);
    assert_eq!(
        app.active_notice(),
        Some("Claude Code can't be switched off while Codex is off too: platoon needs one of them. Switch Codex on first.")
    );
    assert_eq!(on_disk(&w)["use_claude_code"], json!(true));
    settings_on(&mut app, 1);
    key(&mut app, KeyCode::Right);
    assert_eq!(on_disk(&w)["use_codex"], json!(true));
    assert!(app.providers().off.is_empty());
    // Now Claude Code can go off.
    settings_on(&mut app, 0);
    key(&mut app, KeyCode::Enter);
    assert_eq!((on_disk(&w)["use_claude_code"].clone(), app.providers().off.clone()), (json!(false), vec![Agent::Claude]));
}

#[test]
fn codex_switched_off_leaves_its_rows_its_usage_its_models_and_ns_question_out_and_on_brings_them_back() {
    let (w, mut app) = both("programs-codex-off");
    let work = w.base.join("work");
    fs::create_dir_all(&work).unwrap();
    assert_eq!(listed(&app), [Agent::Claude, Agent::Codex]);
    assert!(app.codex_limits.is_some(), "Codex's plan is in the header while it is on");
    let mut term = Terminal::new(TestBackend::new(140, 24)).unwrap();
    assert!(screen(&mut term, &app).lines().next().unwrap().contains("Codex"), "the header names Codex's plan");

    settings_on(&mut app, 1);
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Esc);
    app.refresh_for_test();
    assert_eq!(listed(&app), [Agent::Claude], "only Claude Code's conversations");
    assert!(!app.sessions.iter().any(|s| s.id == FINISHED));
    assert_eq!(app.codex_limits, None);
    let shown = screen(&mut term, &app);
    assert!(!shown.lines().next().unwrap().contains("Codex"), "{shown}");
    assert!(!shown.contains("GPT-6-Luna"), "{shown}");
    let catalog = app.catalog();
    assert!(catalog.models().iter().all(|m| m.agent == Agent::Claude));
    // The full-text search finds nothing of Codex's either.
    key(&mut app, KeyCode::Char('/'));
    for c in "colour".chars() {
        key(&mut app, KeyCode::Char(c));
    }
    assert!(app.visible().iter().all(|&i| app.sessions[i].agent == Agent::Claude));
    key(&mut app, KeyCode::Esc);
    app.mode = Mode::List;
    // `n` doesn't ask: Claude Code starts at once.
    key(&mut app, KeyCode::Char('n'));
    for c in work.to_string_lossy().chars() {
        key(&mut app, KeyCode::Char(c));
    }
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List), "no question");
    assert_eq!(app.launched.iter().map(|l| l.agent).collect::<Vec<_>>(), [Agent::Claude]);

    // Back on (an edit of the file by hand is seen too): everything comes back.
    let mut s = settings::load(&w.config).0;
    s.use_codex = true;
    settings::save(&w.config, &s).unwrap();
    app.refresh_for_test();
    app.refresh_for_test();
    assert_eq!(listed(&app), [Agent::Claude, Agent::Codex]);
    assert!(app.codex_limits.is_some());
    assert!(app.catalog().models().iter().any(|m| m.id == "gpt-6-luna"));
}

#[test]
fn claude_code_switched_off_leaves_its_rows_and_usage_out_and_a_model_set_to_it_is_said_plainly() {
    let (w, mut app) = both("programs-claude-off");
    let mut s = settings::load(&w.config).0;
    s.ai_model = Some("haiku".into());
    s.use_claude_code = false;
    settings::save(&w.config, &s).unwrap();
    app.refresh_for_test();
    app.refresh_for_test();
    assert_eq!(app.providers().off, [Agent::Claude]);
    assert_eq!(listed(&app), [Agent::Codex], "only Codex's conversations");
    assert!(!app.sessions.iter().any(|s| s.id == CLAUDE));
    let catalog = app.catalog();
    assert!(catalog.models().iter().all(|m| m.agent == Agent::Codex));
    // The smart features' model, Haiku, isn't swapped: said, with why.
    let (label, ..) = app.model_setting(settings::ModelFor::Smart);
    assert_eq!(label, "Haiku (Claude Code is switched off in Settings)");
    // The notes' model, never picked, follows what is on: Codex's first.
    assert_eq!(settings::ModelFor::Notes.in_effect(&settings::load(&w.config).0, &catalog).as_deref(), Some("gpt-6-luna"));
    let mut term = Terminal::new(TestBackend::new(140, 24)).unwrap();
    let header = screen(&mut term, &app).lines().next().unwrap().to_string();
    assert!(!header.contains("Claude") && header.contains("Codex"), "Claude's plan isn't in the header, Codex's is: {header}");
    // On the settings screen, under Smart features.
    key(&mut app, KeyCode::Char('s'));
    app.switch_settings_tab(settings::TAB_SMART);
    let mut term = Terminal::new(TestBackend::new(120, 50)).unwrap();
    let shown = flat(&screen(&mut term, &app));
    assert!(shown.contains("‹ Haiku (Claude Code is switched off in Settin… ›"), "said as far as the row has room: {shown}");
}

/// A project "Web shop" whose task #1 runs on the user's squad "Mine" (GPT-6-Luna, so in Codex), started, with its
/// session `sess`; and an app with Claude Code and Codex, Codex switched off in the settings.
fn task_lab(name: &str) -> (World, App, String) {
    let w = World::new(name);
    let shop = w.base.join("repos/shop");
    fs::create_dir_all(&shop).unwrap();
    fs::create_dir_all(w.config.join("squads")).unwrap();
    fs::write(w.config.join("squads/mine.md"), "---\nname: Mine\ndescription: Ours\nmodel: gpt-6-luna\n---\n\n1. Do it.\n").unwrap();
    settings::save(&w.config, &settings::Settings { use_codex: false, ..settings::Settings::default() }).unwrap();
    let mut app = w.app();
    let project = app.projects.create("Web shop").unwrap();
    let mut store = Tickets::open(&w.config, &project);
    for title in ["Add a basket", "Add a till"] {
        store
            .add(NewTicket {
                title: title.into(),
                folder: Some(shop.to_string_lossy().into_owned()),
                squad: "mine".into(),
                ..Default::default()
            })
            .unwrap();
    }
    store.start(1, "mine", "sess", None, &[]).unwrap();
    let codex = crate::agents::tests::codex_with_models(name);
    app.set_providers_for_test(Providers {
        claude: Some("/nonexistent/platoon-unit-test/claude".into()),
        codex: Some(CodexAt { program: "/nonexistent/platoon-unit-test/codex".into(), root: codex.root }),
        off: Vec::new(),
    });
    assert_eq!(app.providers().off, [Agent::Codex]);
    app.reload_for_test();
    key(&mut app, KeyCode::Char('3'));
    (w, app, project)
}

#[test]
fn a_running_task_of_a_program_switched_off_stays_on_the_tasks_tab_marked_and_is_opened_and_cancelled() {
    let (w, mut app, project) = task_lab("programs-task-running");
    // Its session runs in a host of platoon's: a process of this test's own, and a socket that takes connections.
    let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
    let sock = w.base.join("task.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    let host = crate::host::HostInfo {
        agent: Agent::Codex,
        claude_pid: child.id(),
        host_pid: child.id(),
        dir: w.base.join("repos/shop").to_string_lossy().into_owned(),
        args: Agent::Codex.args(crate::agents::Start::Resume("sess")),
        sock,
        task: Some(tickets::task_tag(&project, 1)),
        version: None,
        kept: None,
    };
    app.refresh_with(vec![host], false);
    let mut term = Terminal::new(TestBackend::new(160, 30)).unwrap();
    let shown = screen(&mut term, &app);
    let row = shown.lines().position(|l| l.contains("#1 Add a basket") && l.contains("just now")).unwrap_or_else(|| panic!("{shown}"));
    assert!(shown.lines().nth(row + 1).unwrap().contains("Codex is switched off · Mine"), "marked on its row:\n{shown}");
    assert!(shown.contains("Mine · in Codex, which is switched off"), "and in its details:\n{shown}");
    let other = shown.lines().position(|l| l.contains("Add a till")).unwrap();
    assert!(!shown.lines().nth(other + 1).unwrap().contains("switched off"), "a task not started isn't marked:\n{shown}");

    // `s` has nothing to start; enter opens the running session.
    key(&mut app, KeyCode::Char('s'));
    assert_eq!(app.active_notice(), Some("#1 has its session already: enter opens it."));
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Attached(pid) if pid == child.id()), "attached to its host");
    app.mode = Mode::List;

    // `c` cancels it, and stops its session.
    key(&mut app, KeyCode::Char('c'));
    assert!(matches!(app.mode, Mode::Confirm(Confirm::CancelTicket { number: 1, pid: Some(p), .. }) if p == child.id()));
    key(&mut app, KeyCode::Char('y'));
    assert_eq!(Tickets::open(&w.config, &project).get(1).unwrap().state, State::Cancelled);
    let status = child.wait().unwrap();
    assert!(!status.success(), "its session was stopped");
}

#[test]
fn a_task_of_a_program_switched_off_is_not_started_or_continued_and_says_why() {
    let (w, mut app, project) = task_lab("programs-task-refused");
    // Not running: enter doesn't continue it.
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(
        app.active_notice(),
        Some(
            "#1 runs in Codex, which is switched off in platoon's settings, so its session wasn't started again. Switch Codex back on to go on with it."
        )
    );
    assert!(app.sq.launched.is_empty());
    // A task not started, on a squad whose model is Codex's: `s` and enter refuse it.
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Char('s'));
    app.sq.pick_selected = app.sq.pick.iter().position(|s| s.id == "mine").unwrap();
    key(&mut app, KeyCode::Enter);
    assert_eq!(
        app.active_notice(),
        Some("The Mine squad's model can't be used: GPT-6-Luna runs on Codex, which is switched off in platoon's settings.")
    );
    assert_eq!(Tickets::open(&w.config, &project).get(2).unwrap().state, State::NotStarted);
    assert!(app.sq.launched.is_empty());
    // The Squads tab says it of the squad's model too.
    app.mode = Mode::List;
    key(&mut app, KeyCode::Char('4'));
    let mut term = Terminal::new(TestBackend::new(160, 30)).unwrap();
    let shown = screen(&mut term, &app);
    let _ = shown;
    // (the pane beside the list shows the picked squad: pick Mine)
    while app.sqt.list.get(app.sqt.selected).is_some_and(|s| s.id != "mine") {
        key(&mut app, KeyCode::Down);
    }
    let shown = screen(&mut term, &app);
    assert!(flat(&shown).contains("GPT-6-Luna (Codex is switched off in Settings)"), "{shown}");
}
