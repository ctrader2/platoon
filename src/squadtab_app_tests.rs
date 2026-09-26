//! The Squads tab at the level of the app: the list and the details, every change made with the keys (a model, a
//! switch, members, steps, their order and their send-backs, a system prompt in a stand-in editor), drafting a squad
//! with a stand-in `claude`, copying, deleting and restoring, a file that can't be read, and `a` on a ticket listing
//! the user's own squads. Every directory is a temp dir (`World`); nothing here starts a session.

use crate::app::{App, Confirm, DRow, Mode, SqFocus};
use crate::editors::Editor;
use crate::squadfile::{self, Back, Kind};
use crate::squads::Launch;
use crate::testkit::{AiFake, World};
use crate::tickets::{AgentNames, Leader, NewTicket, Tickets};
use crate::ui;
use ratatui::{
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    Terminal,
};
use serde_json::Value;
use std::{fs, os::unix::fs::PermissionsExt, thread, time::{Duration, Instant}};

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
    (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

fn flat(text: &str) -> String {
    text.replace(['│', '╭', '╮', '╰', '╯', '─'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
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
    let at = app.sqt.choice.options.iter().position(|(l, _)| l.starts_with(label)).unwrap_or_else(|| panic!("no {label} in {:?}", app.sqt.choice.options));
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

#[test]
fn the_tab_lists_the_built_in_squad_with_its_details_in_four_sections() {
    let (_w, mut app) = on_the_tab("sqt-list");
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let shown = draw(&mut term, &app);
    let f = flat(&shown);
    assert!(f.contains("3 Squads 4"), "the tab and how many squads: {f}");
    assert!(f.contains("Coding 4 members Built in Research 4 members Built in Docs 3 members Built in Summarize 1 member Built in") || f.contains("Research 4 members"), "{f}");
    assert!(f.contains("▎ Coding 4 members") && f.contains("Built in"), "{f}");
    for s in ["About", "Name Coding", "What it can do Makes a change to code", "Needs a folder Yes", "Worktree Always", "Leader", "Model Sonnet", "System prompt You are given one ticket", "Members", "Can change files No", "Can run commands Yes", "Can use the web No"] {
        assert!(f.contains(s), "{s}: {shown}");
    }
    assert!(f.contains("n describe a new squad") && f.contains("c copy") && !f.contains("d delete"), "a built-in as platoon has it: nothing to delete or restore: {f}");
    // The workflow, further down: each step, who does it, and where it may send the work back.
    go(&mut app, DRow::Step(3));
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("Workflow") && f.contains("3. reviewer Reads the change against the ticket"), "{f}");
    assert!(f.contains("↩ may send it back to step 2, at most twice") && f.contains("↩ may send it back to step 2, at most once"), "{f}");
    assert!(f.contains("4. verifier Runs the tests"), "{f}");
    assert!(f.contains("m member") && f.contains("b send back") && f.contains("esc back to the list"), "{f}");
    // Tab and Shift+arrows move between the panes; plain arrows stay in one.
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.sqt.focus, SqFocus::List);
    app.on_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
    assert_eq!(app.sqt.focus, SqFocus::Detail);
    app.on_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));
    assert_eq!(app.sqt.focus, SqFocus::List);
    key(&mut app, KeyCode::Right);
    assert_eq!(app.sqt.focus, SqFocus::List, "a plain arrow doesn't leave the list");
    // `/` is the sessions' search: not here.
    key(&mut app, KeyCode::Char('/'));
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn a_model_and_a_switch_changed_make_the_coding_squad_user_modified() {
    let (w, mut app) = on_the_tab("sqt-model");
    go(&mut app, DRow::MemberModel(3));
    key(&mut app, KeyCode::Enter);
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("Model for verifier:") && f.contains("Opus") && f.contains("Sonnet") && f.contains("Haiku"), "{f}");
    choose(&mut app, "Sonnet");
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.active_notice(), Some("verifier runs on Sonnet now."));
    let v = file(&w, "coding");
    assert_eq!((v["builtin"].as_str(), v["members"][3]["model"].as_str()), (Some("coding"), Some("sonnet")));
    assert_eq!(app.squad_list()[0].kind, Kind::Modified);
    assert!(flat(&draw(&mut term, &app)).contains("User Modified"));
    // Space turns a switch on; the file says so, and the tools follow.
    go(&mut app, DRow::MemberWeb(0));
    key(&mut app, KeyCode::Char(' '));
    assert_eq!(app.active_notice(), Some("planner can use the web now."));
    assert_eq!(file(&w, "coding")["members"][0]["can_use_web"], true);
    let tools = crate::squads::tools_for(&squadfile::get(&w.config, "coding").unwrap().members[0]);
    assert_eq!(tools, ["Read", "Grep", "Glob", "Bash", "WebSearch", "WebFetch"]);
    // The leader's model.
    go(&mut app, DRow::LeaderModel);
    key(&mut app, KeyCode::Enter);
    choose(&mut app, "Opus");
    assert_eq!(file(&w, "coding")["leader"]["model"], "opus");
    // Back as it was, by hand: no file, built in again.
    for (row, label) in [(DRow::LeaderModel, "Sonnet"), (DRow::MemberModel(3), "Haiku")] {
        go(&mut app, row);
        key(&mut app, KeyCode::Enter);
        choose(&mut app, label);
    }
    go(&mut app, DRow::MemberWeb(0));
    key(&mut app, KeyCode::Char(' '));
    assert!(!squadfile::path(&w.config, "coding").exists());
    assert_eq!(app.squad_list()[0].kind, Kind::BuiltIn);
}

#[test]
fn members_are_added_named_and_removed_but_not_while_a_step_uses_them() {
    let (w, mut app) = on_the_tab("sqt-members");
    go(&mut app, DRow::MemberName(0));
    key(&mut app, KeyCode::Char('a'));
    assert!(matches!(app.mode, Mode::ProjectName));
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    assert!(flat(&draw(&mut term, &app)).contains("new member"));
    typed(&mut app, "Docs Writer");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::ProjectName), "the box stays open");
    assert!(app.active_notice().unwrap().starts_with("\"Docs Writer\" can't be a member's name"), "{:?}", app.active_notice());
    ctrl(&mut app, 'u');
    typed(&mut app, "planner");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.active_notice(), Some("The squad already has a member called planner."));
    ctrl(&mut app, 'u');
    typed(&mut app, "docs");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.sqt.drow(), Some(DRow::MemberName(4)), "on the new member");
    let v = file(&w, "coding");
    assert_eq!(v["members"][4]["name"], "docs");
    assert_eq!((v["members"][4]["can_change_files"].as_bool(), v["members"][4]["can_run_commands"].as_bool()), (Some(false), Some(false)), "it can only read");
    // Renamed: the steps it does follow (it does none yet); the builder's rename carries into its step.
    go(&mut app, DRow::MemberName(1));
    key(&mut app, KeyCode::Enter);
    ctrl(&mut app, 'u');
    typed(&mut app, "maker");
    key(&mut app, KeyCode::Enter);
    assert_eq!(file(&w, "coding")["workflow"][1]["member"], "maker");
    // A member a step uses stays, with which step said.
    go(&mut app, DRow::MemberModel(1));
    key(&mut app, KeyCode::Char('x'));
    assert_eq!(app.active_notice(), Some("maker does step 2 of the workflow. Take it out of the workflow first."));
    assert_eq!(file(&w, "coding")["members"].as_array().unwrap().len(), 5);
    go(&mut app, DRow::MemberPrompt(4));
    key(&mut app, KeyCode::Char('x'));
    assert_eq!(app.active_notice(), Some("Took docs out of the squad."));
    assert_eq!(file(&w, "coding")["members"].as_array().unwrap().len(), 4);
}

#[test]
fn steps_are_added_moved_given_a_member_and_a_send_back() {
    let (w, mut app) = on_the_tab("sqt-steps");
    go(&mut app, DRow::AddStep);
    key(&mut app, KeyCode::Enter);
    choose(&mut app, "reviewer");
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
    assert_eq!(steps(&w)[4].member, "reviewer");
    // Up one: it is step 4 now, before the verify.
    key(&mut app, KeyCode::Char('['));
    assert_eq!(app.sqt.drow(), Some(DRow::Step(3)));
    assert_eq!(steps(&w).iter().map(|s| s.member.as_str()).collect::<Vec<_>>(), ["planner", "builder", "reviewer", "reviewer", "verifier"]);
    assert_eq!(steps(&w)[4].back, Some(Back { to: 1, times: 1 }), "the verify still goes back to the build");
    // It may send the work back to step 2, three times.
    key(&mut app, KeyCode::Char('b'));
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("Never sends the work back") && f.contains("Back to step 1: planner") && f.contains("Back to step 3: reviewer") && !f.contains("Back to step 4"), "only earlier steps: {f}");
    choose(&mut app, "Back to step 2");
    choose(&mut app, "At most 3 times");
    assert_eq!(steps(&w)[3].back, Some(Back { to: 1, times: 3 }));
    assert_eq!(app.active_notice(), Some("Step 4 may send it back to step 2, at most 3 times now."));
    // Another member does it.
    key(&mut app, KeyCode::Char('m'));
    choose(&mut app, "planner");
    assert_eq!(steps(&w)[3].member, "planner");
    // What it does, changed.
    key(&mut app, KeyCode::Enter);
    ctrl(&mut app, 'u');
    typed(&mut app, "Checks the plan was followed.");
    key(&mut app, KeyCode::Enter);
    assert_eq!(steps(&w)[3].does, "Checks the plan was followed.");
    // The leader gets it all, in order, with the send-back.
    let prompt = crate::squads::leader_prompt(&squadfile::get(&w.config, "coding").unwrap(), AgentNames::SquadRole, crate::squads::Place::WORKTREE);
    assert!(prompt.contains("4. coding-planner: Checks the plan was followed. It may send the work back: then give its reasons to coding-builder and go on again from step 2. At most 3 times;"), "{prompt}");
    assert!(prompt.contains("5. coding-verifier: "), "{prompt}");
    // Moving a step above the one it sends back to drops that send-back, and says so.
    key(&mut app, KeyCode::Char('['));
    key(&mut app, KeyCode::Char('['));
    assert_eq!(app.sqt.drow(), Some(DRow::Step(1)));
    assert!(app.active_notice().unwrap().contains("doesn't any more"), "{:?}", app.active_notice());
    assert!(crate::squadfile::problems(&squadfile::get(&w.config, "coding").unwrap()).is_empty());
    // The first step has nowhere to send it back to; a step taken out is gone.
    go(&mut app, DRow::Step(0));
    key(&mut app, KeyCode::Char('b'));
    assert_eq!(app.active_notice(), Some("The first step has no step before it to send the work back to."));
    key(&mut app, KeyCode::Char('x'));
    assert_eq!(steps(&w).len(), 4);
}

#[test]
fn a_system_prompt_is_edited_in_the_editor_and_saved_into_the_squad() {
    let (w, mut app) = on_the_tab("sqt-prompt");
    app.set_editors_for_test(vec![fake_editor(&w, "fakeed", "printf 'Build it in small commits.\\n' > \"$1\"")]);
    go(&mut app, DRow::MemberPrompt(1));
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::PickEditor), "the same list of editors as the notes");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::EditNotes));
    wait_for_editor(&mut app);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.active_notice(), Some("Saved builder's system prompt. The squad uses it from the next ticket it is handed."));
    assert_eq!(file(&w, "coding")["members"][1]["guidance"], "Build it in small commits.");
    assert!(!w.cache.join("squad-prompts/coding-builder.md").exists(), "the file it was edited in is gone");
    // An editor that leaves it as it was changes nothing and says nothing.
    app.notice = None;
    app.set_editors_for_test(vec![fake_editor(&w, "idle", "true")]);
    go(&mut app, DRow::LeaderPrompt);
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Enter);
    wait_for_editor(&mut app);
    assert_eq!(app.active_notice(), None);
    assert_eq!(file(&w, "coding")["leader"]["guidance"], crate::squads::LEADER_GUIDANCE);
    // Emptied: kept as it was.
    app.set_editors_for_test(vec![fake_editor(&w, "empty", ": > \"$1\"")]);
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Enter);
    wait_for_editor(&mut app);
    assert_eq!(app.active_notice(), Some("A system prompt can't be empty, so the leader's is as it was."));
}

const DRAFT: &str = r#"{"name":"Sentry triage","can_do":"Reads a Sentry error, reproduces it and fixes it.","needs_folder":true,"worktree":"always",
 "leader":{"model":"sonnet","guidance":"Lead briefly."},
 "members":[
  {"name":"reader","model":"haiku","can_change_files":false,"can_run_commands":false,"can_use_web":true,"guidance":"Read the error."},
  {"name":"fixer","model":"opus","can_change_files":true,"can_run_commands":true,"can_use_web":false,"guidance":"Fix it."}],
 "workflow":[
  {"member":"reader","does":"Reads the error and its stack.","back_to":null,"times":0},
  {"member":"fixer","does":"Fixes it and runs the tests.","back_to":null,"times":0},
  {"member":"reader","does":"Checks the fix against the error.","back_to":2,"times":1}]}"#;

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
fn describe_drafts_a_squad_with_claude_which_is_edited_then_saved() {
    let (w, mut app, fake) = with_claude("sqt-draft");
    fake.reply_default(DRAFT);
    key(&mut app, KeyCode::Char('n'));
    assert!(matches!(app.mode, Mode::SquadText));
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("describe a new squad") && f.contains("enter ask Claude to draft it"), "{f}");
    typed(&mut app, "a squad that triages Sentry errors: a reader, a fixer");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));
    assert!(app.squad_drafting());
    assert!(flat(&draw(&mut term, &app)).contains("Claude is drafting a squad…"));
    app.refresh_until_for_test(20, "the draft", |a| !a.squad_drafting());
    // One call, on Sonnet, as the user asked for it, with the description as data.
    assert_eq!(fake.calls(), 1);
    let argv = fake.argv(1);
    assert_eq!(argv[argv.iter().position(|a| a == "--model").unwrap() + 1], "sonnet");
    assert!(fake.has_arg(1, &squadfile::draft_system(squadfile::DRAFT_GUIDANCE)));
    assert!(fake.stdin(1).contains("a squad that triages Sentry errors: a reader, a fixer"), "{}", fake.stdin(1));
    let day = crate::ledger::Ledger::open(app.cache_for_test()).load();
    assert_eq!(day.days.values().next().unwrap().features.get(squadfile::DRAFT_FEATURE).map(|f| f.calls), Some(1), "its tokens are counted");
    // The draft is listed, selected, not saved.
    assert!(app.sqt.draft_selected() && app.sqt.focus == SqFocus::Detail);
    assert!(app.active_notice().unwrap().starts_with("Here is a draft of \"Sentry triage\"."), "{:?}", app.active_notice());
    app.notice = None;
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("Sentry triage 2 members") && f.contains("Draft, not saved") && f.contains("ctrl+s save draft"), "{f}");
    assert!(!squadfile::path(&w.config, "sentry-triage").exists(), "not saved yet");
    // Changed before it is saved: in memory only.
    go(&mut app, DRow::MemberModel(1));
    key(&mut app, KeyCode::Enter);
    choose(&mut app, "Sonnet");
    assert!(!squadfile::dir(&w.config).join("sentry-triage.json").exists());
    // The draft's two settings are the ones Claude gave it.
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("Needs a folder Yes") && f.contains("Worktree Always"), "{f}");
    ctrl(&mut app, 's');
    assert_eq!(app.active_notice(), Some("Saved the squad \"Sentry triage\". Tickets can be handed to it now."));
    let v = file(&w, "sentry-triage");
    assert_eq!((v["needs_folder"].as_bool(), v["worktree"].as_str()), (Some(true), Some("always")));
    assert_eq!((v["members"][1]["model"].as_str(), v["workflow"][2]["back_to"].as_u64()), (Some("sonnet"), Some(2)));
    assert!(v.get("builtin").is_none());
    assert!(app.sqt.draft.is_none());
    assert_eq!(app.sqt.listed().map(|l| (l.id.as_str(), l.kind)), Some(("sentry-triage", Kind::Own)));
    assert!(flat(&draw(&mut term, &app)).contains("3 Squads 5"));
}

#[test]
fn a_draft_that_cant_be_used_says_why_and_n_offers_the_description_again() {
    let (w, mut app, fake) = with_claude("sqt-baddraft");
    fake.reply_default(&DRAFT.replace(r#""member":"fixer""#, r#""member":"mender""#));
    key(&mut app, KeyCode::Char('n'));
    typed(&mut app, "a triage squad");
    key(&mut app, KeyCode::Enter);
    app.refresh_until_for_test(20, "the draft", |a| !a.squad_drafting());
    assert_eq!(app.active_notice(), Some("Claude's draft couldn't be used: Step 2 names a member the squad doesn't have (mender). Press n to try again."));
    assert!(app.sqt.draft.is_none() && !squadfile::dir(&w.config).exists());
    key(&mut app, KeyCode::Char('n'));
    assert_eq!(app.sqt.text.text, "a triage squad", "what was described is offered again");
    key(&mut app, KeyCode::Esc);
    // A draft is thrown away only after asking.
    fake.reply_default(DRAFT);
    key(&mut app, KeyCode::Char('n'));
    key(&mut app, KeyCode::Enter);
    app.refresh_until_for_test(20, "the draft", |a| !a.squad_drafting());
    assert!(app.sqt.draft_selected());
    key(&mut app, KeyCode::Char('n'));
    assert_eq!(app.active_notice(), Some("There is a draft already: ctrl+s saves it, d discards it."));
    key(&mut app, KeyCode::Char('d'));
    assert!(matches!(app.mode, Mode::Confirm(Confirm::DiscardDraft { .. })));
    key(&mut app, KeyCode::Char('n'));
    assert!(app.sqt.draft.is_some());
    key(&mut app, KeyCode::Char('d'));
    key(&mut app, KeyCode::Char('y'));
    assert!(app.sqt.draft.is_none() && !squadfile::dir(&w.config).join("sentry-triage.json").exists());
    // With the AI features off, nothing is asked.
    let mut s = crate::settings::load(&w.config).0;
    s.tokens_enabled = false;
    crate::settings::save(&w.config, &s).unwrap();
    app.refresh_for_test();
    key(&mut app, KeyCode::Char('n'));
    key(&mut app, KeyCode::Enter);
    assert!(!app.squad_drafting() && app.active_notice().unwrap().starts_with("Couldn't draft a squad: AI features are switched off."), "{:?}", app.active_notice());
    assert_eq!(fake.calls(), 2);
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
    assert_eq!(file(&w, "careful-coding")["members"].as_array().unwrap().len(), 4);
    // Deleting it asks; no leaves it.
    key(&mut app, KeyCode::Char('d'));
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("Delete the squad \"Careful coding\"?") && f.contains("careful-coding.json.bak"), "{f}");
    key(&mut app, KeyCode::Char('n'));
    assert!(squadfile::path(&w.config, "careful-coding").exists());
    key(&mut app, KeyCode::Char('d'));
    key(&mut app, KeyCode::Char('y'));
    assert!(!squadfile::path(&w.config, "careful-coding").exists() && squadfile::path(&w.config, "careful-coding").with_extension("json.bak").exists());
    assert_eq!(app.squad_list().len(), 4);
    // A changed built-in is restored, after asking.
    key(&mut app, KeyCode::Char('g'));
    go(&mut app, DRow::MemberWeb(2));
    key(&mut app, KeyCode::Char(' '));
    assert!(squadfile::path(&w.config, "coding").exists());
    key(&mut app, KeyCode::Char('d'));
    assert!(matches!(app.mode, Mode::Confirm(Confirm::RestoreSquad { .. })));
    assert!(flat(&draw(&mut term, &app)).contains("Put Coding back as platoon has it?"));
    key(&mut app, KeyCode::Char('y'));
    assert_eq!(app.squad_list()[0].kind, Kind::BuiltIn);
    assert!(app.active_notice().unwrap().starts_with("Coding is as platoon has it again."));
}

#[test]
fn a_squad_file_that_cant_be_read_is_shown_as_such_and_left_alone() {
    let w = World::new("sqt-corrupt");
    fs::create_dir_all(squadfile::dir(&w.config)).unwrap();
    fs::write(squadfile::path(&w.config, "mine"), "{ nope").unwrap();
    let mut app = w.app();
    key(&mut app, KeyCode::Char('3'));
    key(&mut app, KeyCode::Char('G'));
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("mine Can't be read") && f.contains("mine.json can't be read (it isn't valid JSON), so platoon leaves it alone."), "{f}");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.sqt.focus, SqFocus::List, "nothing to change");
    key(&mut app, KeyCode::Char('c'));
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(fs::read_to_string(squadfile::path(&w.config, "mine")).unwrap(), "{ nope");
}

#[test]
fn a_on_a_ticket_lists_every_squad_by_name_and_hands_it_to_the_users_own() {
    let w = World::new("sqt-hand");
    let mut own = squadfile::copy_of(&w.config, &squadfile::builtin("coding").unwrap(), "Careful coding", &[]);
    own.members[3].model = "sonnet".into();
    squadfile::save(&w.config, &own).unwrap();
    let mut broken = squadfile::copy_of(&w.config, &own, "Broken", &[]);
    broken.workflow[0].member = "nobody".into();
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
    Tickets::open(&w.config, &id).add(NewTicket { title: "Basket".into(), folders: vec![folder.to_string_lossy().into_owned()], ..Default::default() }).unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Char('a'));
    assert!(matches!(app.mode, Mode::SquadPick));
    let names: Vec<&str> = app.sq.pick.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Coding", "Research", "Docs", "Summarize", "Broken", "Careful coding"], "every squad, by name: built-ins first, then the user's own");
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("▎ Coding") && !f.contains("Careful coding"), "six squads don't fit in 30 rows: {f}");
    // One that needs fixing says what, and the picker stays. The list scrolls to the picked one.
    for _ in 0..5 {
        key(&mut app, KeyCode::Down);
    }
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("▎ Careful coding") && f.contains("Needs fixing on the Squads tab first. Makes a change") && !f.contains("▎ Coding"), "{f}");
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::SquadPick));
    assert_eq!(app.active_notice(), Some("The Broken squad needs fixing on the Squads tab first: Step 1 names a member the squad doesn't have (nobody)."));
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    app.refresh_until_for_test(20, "the leader to start", |a| !a.sq.launched.is_empty());
    assert!(fake.stdin(1).starts_with("The squad: Careful coding\nWhat it can do: "), "the commander reads its name and what it can do: {}", fake.stdin(1));
    let t = Tickets::open(&w.config, &id).get(1).cloned().unwrap();
    let leader = t.leader().unwrap().clone();
    assert_eq!((leader.squad.as_str(), leader.snapshot.as_deref()), ("careful-coding", Some("01-squad-1.json")));
    let launch = app.leader_launch(&id, &leader, &t).unwrap();
    assert_eq!((launch.agent.as_str(), launch.model.as_str(), launch.name.as_str()), ("careful-coding-leader", "sonnet", "Careful coding"));
    let agents: Value = serde_json::from_str(&launch.agents).unwrap();
    assert_eq!(agents["careful-coding-verifier"]["model"], "sonnet");
    assert_eq!(app.squad_name("careful-coding"), "Careful coding");
}

#[test]
fn a_leader_continued_later_runs_with_the_squad_as_it_was_when_it_started() {
    let w = World::new("sqt-snapshot");
    let app = w.app();
    let mut store = Tickets::open(&w.config, "p1");
    let n = store.add(NewTicket { title: "Basket".into(), folders: vec!["/tmp".into()], ..Default::default() }).unwrap();
    let coding = squadfile::builtin("coding").unwrap();
    let at_start = Launch::of(&coding, AgentNames::SquadRole, crate::squads::Place::WORKTREE);
    let snap = store.save_snapshot(n, 1, &at_start.to_json()).unwrap();
    let with = Leader { squad: "coding".into(), session: "s-1".into(), started: 1, names: AgentNames::SquadRole, snapshot: Some(snap) };
    // The squad changes afterwards.
    let mut changed = coding.clone();
    changed.members[3].model = "sonnet".into();
    changed.members[0].guidance = "Plan in one line.".into();
    squadfile::save(&w.config, &changed).unwrap();
    let t = store.get(n).cloned().unwrap();
    let resumed = app.leader_launch("p1", &with, &t).unwrap();
    assert_eq!(resumed, at_start, "the snapshot, not the squad as it is now");
    let agents: Value = serde_json::from_str(&resumed.agents).unwrap();
    assert_eq!(agents["coding-verifier"]["model"], "haiku");
    // A leader from before snapshots (the user's ticket #1) continues as before: the squad as it is now, under the
    // plain names it began with.
    let old = Leader { squad: "coding".into(), session: "s-0".into(), started: 1, names: AgentNames::Plain, snapshot: None };
    let now = app.leader_launch("p1", &old, &t).unwrap();
    let agents: Value = serde_json::from_str(&now.agents).unwrap();
    assert_eq!((now.agent.as_str(), agents["verifier"]["model"].as_str()), ("leader", Some("sonnet")));
    // A snapshot that has gone: said, not guessed.
    let gone = Leader { snapshot: Some("01-squad-9.json".into()), ..with.clone() };
    assert_eq!(app.leader_launch("p1", &gone, &t).unwrap_err(), "what it was started with can't be read any more");
}


#[test]
fn needs_a_folder_and_worktree_are_shown_in_about_and_changed_there() {
    let (w, mut app) = on_the_tab("sqt-settings");
    // Research: no folder needed, never a worktree.
    key(&mut app, KeyCode::Down);
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("Name Research") && f.contains("Needs a folder No") && f.contains("Worktree Never"), "{f}");
    // Space switches "Needs a folder"; the file says so, and it is User Modified.
    go(&mut app, DRow::NeedsFolder);
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("space switch"), "{f}");
    key(&mut app, KeyCode::Char(' '));
    assert_eq!(app.active_notice(), Some("The Research squad needs a folder now: a ticket without one can't be handed to it."));
    assert_eq!((file(&w, "research")["needs_folder"].as_bool(), file(&w, "research")["builtin"].as_str()), (Some(true), Some("research")));
    assert!(squadfile::get(&w.config, "research").unwrap().needs_folder);
    assert!(flat(&draw(&mut term, &app)).contains("Needs a folder Yes"));
    // Enter on "Worktree": the three choices.
    go(&mut app, DRow::Worktree);
    key(&mut app, KeyCode::Enter);
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("Always") && f.contains("When the folder is a git repo") && f.contains("Never"), "{f}");
    choose(&mut app, "When the folder");
    assert_eq!(file(&w, "research")["worktree"], "when-git");
    assert!(app.active_notice().unwrap().starts_with("The squad works in a git worktree when the ticket's folder is a git repo now"), "{:?}", app.active_notice());
    assert!(flat(&draw(&mut term, &app)).contains("Worktree When the folder is a git repo"));
    assert_eq!(squadfile::get(&w.config, "research").unwrap().worktree, squadfile::WorktreeRule::WhenGit);
    // Back as platoon has them: no file any more.
    go(&mut app, DRow::NeedsFolder);
    key(&mut app, KeyCode::Char(' '));
    go(&mut app, DRow::Worktree);
    key(&mut app, KeyCode::Enter);
    choose(&mut app, "Never");
    assert!(!squadfile::path(&w.config, "research").exists(), "the built-in as it was needs no file");
    assert_eq!(app.squad_list()[1].kind, Kind::BuiltIn);
}
