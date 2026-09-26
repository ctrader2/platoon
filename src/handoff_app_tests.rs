//! The hand-off (`h`) at the level of the app: a real transcript on disk, a real project, a fake `claude` that
//! records every call, and what the box shows, what may be edited and what is refused. Nothing here ever starts a
//! session (that is `tests/handoff.rs`, with the real binary); every directory is a temp dir and the real
//! `claude` is never reached.

use crate::app::{App, HandoffState, Mode};
use crate::handoff;
use crate::ledger::Ledger;
use crate::projects::Projects;
use crate::settings::{self, Settings};
use crate::testkit::{AiFake, World};
use crate::usage;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::fs;

const BRIEF: &str = r#"{"summary":"The basket page is in.","done":["Baskets are session based"],"state":["Totals ignore VAT"],"next":["Take card payments."],"files":["claude-says.rs: made up"]}"#;

/// A world with one conversation in one project, the token features on, and a stand-in `claude`.
fn world(name: &str) -> (World, AiFake) {
    let w = World::new(name);
    // A folder that really exists: a hand-off starts a new session in it, so platoon checks it is there.
    fs::create_dir_all(w.base.join("shop")).unwrap();
    fs::create_dir_all(w.base.join("other")).unwrap();
    w.transcript("sess-1", &shop(&w), "Add a basket page to the shop", 5);
    let mut s = settings::load(&w.config).0;
    s.tokens_enabled = true;
    settings::save(&w.config, &s).unwrap();
    let mut p = Projects::open(&w.config);
    let id = p.create("Web shop").unwrap();
    fs::create_dir_all(w.config.join("projects").join(&id)).unwrap();
    fs::write(p.notes_path(&id), "# Overview\n\nSell socks.\n").unwrap();
    p.assign("sess-1", Some(&id)).unwrap();
    (w, AiFake::new(name))
}

fn shop(w: &World) -> String {
    w.base.join("shop").to_string_lossy().into_owned()
}

fn other(w: &World) -> String {
    w.base.join("other").to_string_lossy().into_owned()
}

fn app_of(w: &World, fake: &AiFake) -> App {
    let mut app = w.app();
    app.set_ai_program_for_test(&fake.script);
    app.refresh_with(Vec::new(), false);
    app
}

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn ctrl(app: &mut App, c: char) {
    app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
}

fn select(app: &mut App, id: &str) {
    app.selected = app.visible().iter().position(|&i| app.sessions[i].id == id).expect("the row is listed");
}

fn wait_ready(app: &mut App) {
    app.refresh_until_for_test(10, "the brief to arrive", |a| a.handoff.state != HandoffState::Asking);
}

fn project_id(w: &World) -> String {
    Projects::open(&w.config).all()[0].id.clone()
}

#[test]
fn h_asks_for_a_brief_and_shows_it_for_the_user_to_edit_before_anything_starts() {
    let (w, fake) = world("handoff-brief");
    fake.reply_default(BRIEF);
    let mut app = app_of(&w, &fake);
    select(&mut app, "sess-1");
    // The key it used to be does nothing at all now, on either screen it works on.
    key(&mut app, KeyCode::Char('H'));
    assert!(matches!(app.mode, Mode::List) && app.handoff.text.is_empty(), "H is no longer the hand-off");
    assert_eq!(fake.calls(), 0, "and it asked Claude nothing");
    key(&mut app, KeyCode::Char('h'));
    assert!(matches!(app.mode, Mode::Handoff) && app.handoff.state == HandoffState::Asking, "the box opens while the call is out");
    assert_eq!((app.handoff.session.as_str(), app.handoff.project_name.as_str(), app.handoff.cwd.clone()), ("sess-1", "Web shop", shop(&w)));
    wait_ready(&mut app);
    assert_eq!(app.handoff.state, HandoffState::Ready);

    // The document is platoon's fixed shape: its own facts, then what came back.
    let text = app.handoff.text.clone();
    assert!(text.starts_with("# Session hand-off: shop\n\nThis is context for this session. The user will prompt you for the next course of action.\n\n- Written: "), "{text}");
    assert!(text.contains("- Project: Web shop\n"), "{text}");
    assert!(text.contains(&format!("- Previous session: shop, in {} (not running)\n", shop(&w))), "{text}");
    // The id, so the new session can look back at the old one; where the project's notes are; what it spent.
    assert!(text.contains("- Previous session id: sess-1 (`claude --resume sess-1` opens it)\n"), "{text}");
    let notes = Projects::open(&w.config).notes_path(&project_id(&w));
    assert!(text.contains(&format!("- Project notes: {}\n", notes.display())), "{text}");
    assert!(text.contains("- Tokens the previous session used: 50,000 sent to Claude and 0 written by Claude\n"), "{text}");
    assert!(text.contains("- Started: ") && text.contains(" between)\n"), "{text}");
    assert!(text.contains("- Model: Sonnet 5, context "), "{text}");
    assert!(text.contains("- Messages from the user: 1 "), "{text}");
    assert!(text.contains("## Executive summary\n\nThe basket page is in.\n"), "{text}");
    assert!(text.contains("## Work completed\n\n- Baskets are session based\n"), "{text}");
    assert!(text.contains("## Current state and known gotchas\n\n- Totals ignore VAT\n"), "{text}");
    assert!(text.contains("## Next steps\n\n1. Take card payments.\n"), "{text}");
    // The folder is no git repository: no git facts, and the files are Claude's own account, said to be so.
    assert!(!text.contains("Git branch"), "{text}");
    assert!(text.contains("As Claude described them (this folder is not a git repository):\n- claude-says.rs: made up"), "{text}");
    assert!(!text.contains("Sell socks."), "the project's notes are not repeated: {text}");
    assert!(handoff::why_not(&text).is_none());

    // What was sent: this conversation, its folder and its project, and never its session id.
    assert_eq!(fake.calls(), 1);
    let sent = fake.stdin(1);
    assert!(sent.contains("Project: Web shop") && sent.contains("Folder: shop"), "{sent}");
    assert!(sent.contains("- Add a basket page to the shop"), "{sent}");
    assert!(sent.contains("not a git repository, so fill \"files\""), "{sent}");
    assert!(!sent.contains("sess-1") && !sent.contains("--resume"), "the id is in the document, never in the request: {sent}");
    assert!(!fake.argv(1).iter().any(|a| a.contains("sess-1")), "nor in the call's arguments: {:?}", fake.argv(1));
    // It is the user's own key press, so it goes as user-initiated work, and its spend is in the ledger.
    assert!(fake.argv(1).contains(&"-p".to_string()));
    let day = Ledger::open(&w.cache).today(usage::now());
    assert_eq!(day.features.get(handoff::FEATURE).map(|f| f.calls), Some(1));
    assert!(day.tokens() > 0);

    // Nothing has happened to the old session or to the project.
    assert!(!app.sessions.iter().any(|s| s.archived));
    assert_eq!(Projects::open(&w.config).project_id_of("sess-1"), Some(project_id(&w).as_str()));
}

#[test]
fn h_works_on_the_project_screen_too_and_the_old_key_does_not() {
    let (w, fake) = world("handoff-project-screen");
    fake.reply_default(BRIEF);
    let mut app = app_of(&w, &fake);
    // On to the Projects tab and into the project: its sessions pane has the focus.
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Enter);
    assert!(app.open_project().is_some(), "the project's screen is open");
    select(&mut app, "sess-1");
    key(&mut app, KeyCode::Char('H'));
    assert!(matches!(app.mode, Mode::List), "H does nothing here either");
    assert_eq!(fake.calls(), 0);
    key(&mut app, KeyCode::Char('h'));
    assert!(matches!(app.mode, Mode::Handoff), "h asks for the brief straight away, with nothing to confirm");
    wait_ready(&mut app);
    assert_eq!(app.handoff.state, HandoffState::Ready);
    assert_eq!(fake.calls(), 1);
}

#[test]
fn the_brief_can_be_typed_into_before_it_is_accepted() {
    let (w, fake) = world("handoff-edit");
    fake.reply_default(BRIEF);
    let mut app = app_of(&w, &fake);
    select(&mut app, "sess-1");
    key(&mut app, KeyCode::Char('h'));
    wait_ready(&mut app);
    let before = app.handoff.text.clone();

    // The cursor starts at the top, where the document starts; Down and End take it to the end.
    assert_eq!(app.handoff.cursor, 0);
    for _ in 0..before.lines().count() + 5 {
        key(&mut app, KeyCode::Down);
    }
    key(&mut app, KeyCode::End);
    assert_eq!(app.handoff.cursor, before.chars().count());
    for c in " Also: sizes.".chars() {
        key(&mut app, KeyCode::Char(c));
    }
    assert!(app.handoff.text.ends_with(" Also: sizes."), "{}", app.handoff.text);
    key(&mut app, KeyCode::Backspace);
    assert!(app.handoff.text.ends_with(" Also: sizes"), "{}", app.handoff.text);
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('x'));
    assert!(app.handoff.text.ends_with("sizes\nx"), "{}", app.handoff.text);

    // And it can be moved about: home, one to the left, a character in the middle.
    key(&mut app, KeyCode::Home);
    assert_eq!(app.handoff.cursor_at().1, 0);
    key(&mut app, KeyCode::Char('y'));
    assert!(app.handoff.text.ends_with("sizes\nyx"), "{}", app.handoff.text);
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::End);
    key(&mut app, KeyCode::Char('!'));
    assert!(app.handoff.text.contains("sizes!\nyx"), "{}", app.handoff.text);
    key(&mut app, KeyCode::Delete);
    assert!(app.handoff.text.contains("sizes!yx"), "the line break was deleted: {}", app.handoff.text);
    // Pasting goes in at the cursor too.
    app.on_paste(" pasted");
    assert!(app.handoff.text.contains("sizes! pastedyx"), "{}", app.handoff.text);
    // Backspace only ever takes away what is in front of the cursor; what is after it stays.
    let after_cursor: String = app.handoff.text.chars().skip(app.handoff.cursor).collect();
    for _ in 0..(app.handoff.text.chars().count() + 5) {
        key(&mut app, KeyCode::Backspace);
    }
    assert_eq!((app.handoff.text.clone(), app.handoff.cursor), (after_cursor, 0));
    // And moving about in an almost empty box does nothing silly.
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Home);
    assert_eq!(app.handoff.cursor, 0);
    key(&mut app, KeyCode::End);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Home);
    key(&mut app, KeyCode::Delete);
    key(&mut app, KeyCode::Delete);
    key(&mut app, KeyCode::Delete);
    assert_eq!((app.handoff.text.as_str(), app.handoff.cursor), ("", 0));
    assert_eq!(fake.calls(), 1, "editing asks Claude nothing");
}

#[test]
fn esc_calls_the_whole_thing_off_and_changes_nothing() {
    let (w, fake) = world("handoff-esc");
    fake.reply_default(BRIEF);
    let mut app = app_of(&w, &fake);
    select(&mut app, "sess-1");
    key(&mut app, KeyCode::Char('h'));
    wait_ready(&mut app);
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List));
    assert!(app.active_notice().is_none(), "nothing to say: nothing happened");
    // The conversation is where it was, in the project it was in, and no session was started.
    assert_eq!(Projects::open(&w.config).members_of(&project_id(&w)), ["sess-1"]);
    assert!(!app.sessions.iter().any(|s| s.archived));
    assert_eq!(app.sessions.len(), 1);
    assert_eq!(fake.calls(), 1, "and nothing else was asked");

    // Esc while the call is still out calls it off too.
    fake.hang(true);
    key(&mut app, KeyCode::Char('h'));
    assert_eq!(app.handoff.state, HandoffState::Asking);
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List));
    app.refresh_until_for_test(10, "the cancelled call to end", |a| !a.handoff_busy());
    assert!(fake.pids().iter().all(|&p| !crate::testkit::alive(p)), "the call's process is gone");
}

#[test]
fn an_answer_that_is_not_understood_or_a_call_that_fails_says_so_and_starts_nothing() {
    let (w, fake) = world("handoff-bad");
    fake.reply_default("I'm afraid I can't help with that.");
    let mut app = app_of(&w, &fake);
    select(&mut app, "sess-1");
    key(&mut app, KeyCode::Char('h'));
    wait_ready(&mut app);
    match &app.handoff.state {
        HandoffState::Failed(why) => assert!(why.contains("couldn't be understood") && why.contains("Nothing has changed"), "{why}"),
        other => panic!("{other:?}"),
    }
    // Nothing may be accepted from here.
    ctrl(&mut app, 's');
    assert!(matches!(app.mode, Mode::Handoff), "ctrl+s does nothing without a brief");
    key(&mut app, KeyCode::Esc);

    // A call that fails outright says why in the same place.
    fake.fail(Some("Claude is over capacity"));
    key(&mut app, KeyCode::Char('h'));
    wait_ready(&mut app);
    match &app.handoff.state {
        HandoffState::Failed(why) => assert!(why.contains("Nothing has changed"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(app.sessions.len(), 1, "no session was started either way");
}

#[test]
fn hand_off_needs_a_project_a_live_project_and_the_gate_and_says_plainly_which_is_missing() {
    let (w, fake) = world("handoff-refused");
    fake.reply_default(BRIEF);
    // A second conversation, in no project.
    w.transcript("loose-2", &other(&w), "Something else entirely", 4);
    let mut app = app_of(&w, &fake);

    select(&mut app, "loose-2");
    key(&mut app, KeyCode::Char('h'));
    assert!(matches!(app.mode, Mode::List));
    let said = app.active_notice().unwrap_or_default().to_string();
    assert!(said.contains("is in no project") && said.contains("Press m"), "{said}");

    // The master switch off: nothing is asked, and it says how to turn it on.
    let mut s = settings::load(&w.config).0;
    s.tokens_enabled = false;
    settings::save(&w.config, &s).unwrap();
    app.refresh_with(Vec::new(), false);
    select(&mut app, "sess-1");
    key(&mut app, KeyCode::Char('h'));
    assert!(matches!(app.mode, Mode::List), "the box never opens");
    let said = app.active_notice().unwrap_or_default().to_string();
    assert!(said.starts_with("No hand-off:") && said.contains("Smart features"), "{said}");
    assert_eq!(fake.calls(), 0, "nothing was asked at all");

    // Today's tokens spent: the same, in its own words.
    s.tokens_enabled = true;
    settings::save(&w.config, &s).unwrap();
    let spent = crate::ledger::Usage { input: Settings::default().clamped().ai_daily_tokens + 1, ..Default::default() };
    Ledger::open(&w.cache).record_call(usage::now(), "names", &spent, 0.0, true).unwrap();
    app.refresh_with(Vec::new(), false);
    select(&mut app, "sess-1");
    key(&mut app, KeyCode::Char('h'));
    let said = app.active_notice().unwrap_or_default().to_string();
    assert!(said.contains("limit is used up"), "{said}");
    assert_eq!(fake.calls(), 0);

    // An archived project is refused before anything is read.
    let _ = fs::remove_file(Ledger::open(&w.cache).path());
    let id = project_id(&w);
    Projects::open(&w.config).archive(&id, &[]).unwrap();
    app.refresh_with(Vec::new(), false);
    select(&mut app, "sess-1");
    key(&mut app, KeyCode::Char('h'));
    let said = app.active_notice().unwrap_or_default().to_string();
    assert!(said.contains("is archived"), "{said}");
    assert_eq!(fake.calls(), 0);
}

#[test]
fn a_brief_that_could_not_be_handed_on_is_refused_with_the_box_left_as_it_was() {
    let (w, fake) = world("handoff-whynot");
    fake.reply_default(BRIEF);
    let mut app = app_of(&w, &fake);
    select(&mut app, "sess-1");
    key(&mut app, KeyCode::Char('h'));
    wait_ready(&mut app);
    // Emptied: there is nothing to start a session with.
    let was = app.handoff.text.chars().count();
    for _ in 0..was {
        key(&mut app, KeyCode::Delete);
    }
    ctrl(&mut app, 's');
    assert!(matches!(app.mode, Mode::Handoff), "the box stays open");
    assert!(app.active_notice().unwrap_or_default().contains("The brief is empty"), "{:?}", app.active_notice());
    // Starting with a dash would be read as an option by claude, so it is refused with the text untouched.
    for c in "--resume trouble".chars() {
        key(&mut app, KeyCode::Char(c));
    }
    ctrl(&mut app, 's');
    assert!(matches!(app.mode, Mode::Handoff));
    assert_eq!(app.handoff.text, "--resume trouble", "the text is left exactly as it was");
    assert!(app.active_notice().unwrap_or_default().contains("start with a dash"), "{:?}", app.active_notice());
    assert_eq!(app.sessions.len(), 1, "and no session was started");
}

// ---- the document's facts from git, and the box that wraps and scrolls -----------------------------------------

/// A git command that writes, only ever in a test's own temp folder, with a fixed author and none of the user's
/// own git settings.
fn git(dir: &std::path::Path, args: &[&str], date: Option<&str>) {
    let mut c = std::process::Command::new("git");
    c.arg("-C").arg(dir).args(args);
    c.env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_NOSYSTEM", "1");
    c.env("GIT_AUTHOR_NAME", "Test").env("GIT_AUTHOR_EMAIL", "test@example.com");
    c.env("GIT_COMMITTER_NAME", "Test").env("GIT_COMMITTER_EMAIL", "test@example.com");
    if let Some(d) = date {
        c.env("GIT_AUTHOR_DATE", d).env("GIT_COMMITTER_DATE", d);
    }
    let out = c.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped()).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn in_a_git_repository_platoon_fills_the_branch_the_commits_and_the_files_itself() {
    let (w, fake) = world("handoff-git");
    let dir = w.base.join("shop");
    // Before the session: one commit. During it (the transcript starts 5 minutes ago): another, and some work
    // not committed yet.
    git(&dir, &["init", "-q", "-b", "basket"], None);
    fs::write(dir.join("old.rs"), "old").unwrap();
    git(&dir, &["add", "-A"], None);
    git(&dir, &["commit", "-q", "-m", "Before the session"], Some("2020-01-01T00:00:00Z"));
    fs::write(dir.join("basket.rs"), "basket").unwrap();
    git(&dir, &["add", "-A"], None);
    git(&dir, &["commit", "-q", "-m", "Add the basket"], None);
    fs::write(dir.join("old.rs"), "changed").unwrap();
    fs::write(dir.join("todo.txt"), "new").unwrap();
    fake.reply_default(BRIEF);
    let mut app = app_of(&w, &fake);
    select(&mut app, "sess-1");
    key(&mut app, KeyCode::Char('h'));
    wait_ready(&mut app);
    let text = app.handoff.text.clone();
    assert!(text.contains("- Git branch: basket\n"), "{text}");
    assert!(text.contains("- Uncommitted changes: 2 files\n"), "{text}");
    assert!(text.contains(" Add the basket\n") && text.contains("- Last commit: "), "{text}");
    assert!(text.contains("- Commits since the session started: 1, newest first:\n  - "), "{text}");
    assert!(text.contains(" Add the basket\n- Last commit: ") || text.contains(" Add the basket\n\n## "), "the subject is listed: {text}");
    assert!(!text.contains("Before the session\n"), "a commit from before the session is not: {text}");
    assert!(!text.contains("- Compared with"), "no upstream, so nothing is said about one: {text}");
    assert!(
        text.contains("## Files affected\n\nNot committed yet:\n- old.rs (changed)\n- todo.txt (new, not yet added to git)\nCommitted since the session started:\n- basket.rs\n"),
        "{text}"
    );
    // Git knows the files, so Claude's own list is not used, and Claude was told the files instead of asked.
    assert!(!text.contains("claude-says.rs"), "{text}");
    let sent = fake.stdin(1);
    assert!(sent.contains("- old.rs (changed, not committed)") && sent.contains("- basket.rs (committed during the session)"), "{sent}");
    assert!(sent.contains("leave \"files\" out"), "{sent}");
    // Reading changed nothing in the repository.
    assert!(!dir.join(".git/index.lock").exists());
}

#[test]
fn a_session_still_running_says_so_and_its_spend_counts_its_subagents_and_each_answer_once() {
    let (mut w, fake) = world("handoff-running");
    // The old session is still running, and its transcript has one answer written as two lines, and a subagent.
    let path = crate::claude::find_transcript(&w.root, "sess-1").unwrap();
    let answer = serde_json::json!({"type":"assistant","message":{"id":"msg-9","model":"claude-sonnet-5","usage":{"input_tokens":1,"cache_creation_input_tokens":2_000,"cache_read_input_tokens":30_000,"output_tokens":700}}}).to_string();
    let mut t = fs::read_to_string(&path).unwrap();
    t.push_str(&format!("{answer}\n{answer}\n"));
    fs::write(&path, t).unwrap();
    let sub = path.with_extension("").join("subagents");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("agent-a.jsonl"), serde_json::json!({"type":"assistant","message":{"id":"msg-s","usage":{"input_tokens":9,"output_tokens":300}}}).to_string() + "\n").unwrap();
    w.live("sess-1", &shop(&w), crate::testkit::now_ms(), "idle");
    fake.reply_default(BRIEF);
    let mut app = app_of(&w, &fake);
    select(&mut app, "sess-1");
    key(&mut app, KeyCode::Char('h'));
    wait_ready(&mut app);
    let text = app.handoff.text.clone();
    assert!(text.contains(&format!("- Previous session: shop, in {} (still running)\n", shop(&w))), "{text}");
    assert!(
        text.contains("- Tokens the previous session used: 52,010 sent to Claude and 1,000 written by Claude, plus 30,000 read again from Claude's cache (its subagents included)\n"),
        "{text}"
    );
    // A project whose notes file is gone: the path is left out rather than pointing at nothing.
    key(&mut app, KeyCode::Esc);
    fs::remove_file(Projects::open(&w.config).notes_path(&project_id(&w))).unwrap();
    key(&mut app, KeyCode::Char('h'));
    wait_ready(&mut app);
    assert!(app.handoff.state == HandoffState::Ready && !app.handoff.text.contains("- Project notes"), "{}", app.handoff.text);
}

#[test]
fn a_git_that_fails_just_leaves_the_git_facts_out() {
    let (w, fake) = world("handoff-git-fails");
    let bad = w.base.join("bad-git");
    fs::write(&bad, "#!/bin/sh\nexit 128\n").unwrap();
    std::os::unix::fs::PermissionsExt::set_mode(&mut fs::metadata(&bad).unwrap().permissions(), 0o755);
    fs::set_permissions(&bad, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    fake.reply_default(BRIEF);
    let mut app = app_of(&w, &fake);
    app.set_git_program_for_test(bad.to_str().unwrap());
    select(&mut app, "sess-1");
    key(&mut app, KeyCode::Char('h'));
    wait_ready(&mut app);
    let text = app.handoff.text.clone();
    assert_eq!(app.handoff.state, HandoffState::Ready, "the document is still made");
    assert!(!text.contains("Git branch") && !text.contains("Uncommitted") && !text.contains("Last commit"), "{text}");
    assert!(text.contains("- claude-says.rs: made up"), "Claude's account stands in: {text}");
}

/// A long answer: a summary that wraps many times, and many steps.
fn long_brief() -> String {
    let summary = "The basket page shows every item with its size, colour and price, and the totals at the foot of it now \
                   add up across currencies once the rates are fetched, which happens when the page first loads.";
    let next: Vec<String> = (1..=handoff::NEXT_MAX)
        .map(|i| format!("\"Step number {i} of the plan, written out long enough to need several rows in a narrow box: check the totals, the sizes, the colours and the prices before the next one\""))
        .collect();
    format!(r#"{{"summary":"{summary}","next":[{}]}}"#, next.join(","))
}

fn term(w: u16, h: u16) -> ratatui::Terminal<ratatui::backend::TestBackend> {
    ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap()
}

/// The screen, one string per line (a wide character's second cell is empty, so the text reads as typed).
fn draw(t: &mut ratatui::Terminal<ratatui::backend::TestBackend>, app: &App) -> Vec<String> {
    t.draw(|f| crate::ui::draw(f, app)).unwrap();
    let buf = t.backend().buffer();
    (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect()).collect()
}

/// The box's own rows of text: the lines between its two rulers, without the border.
fn body(screen: &[String]) -> Vec<String> {
    let rulers: Vec<usize> = screen.iter().enumerate().filter(|(_, l)| l.contains("│ ───") || l.contains("│──")).map(|(n, _)| n).collect();
    let (a, b) = (rulers[0], rulers[rulers.len() - 1]);
    screen[a + 1..b].iter().map(|l| l.trim_matches(|c| c == ' ' || c == '│').to_string()).collect()
}

#[test]
fn a_long_document_wraps_at_the_box_edge_and_scrolls_with_the_cursor_and_says_what_is_above_and_below() {
    let (w, fake) = world("handoff-wrap");
    fake.reply_default(&long_brief());
    let mut app = app_of(&w, &fake);
    let mut t = term(70, 40);
    select(&mut app, "sess-1");
    key(&mut app, KeyCode::Char('h'));
    wait_ready(&mut app);
    let screen = draw(&mut t, &app);
    let joined = screen.join("\n");
    // The top of the document, nothing above it, and a count of what is below.
    assert!(joined.contains("# Session hand-off:"), "{joined}");
    assert!(!joined.contains("more line") || !joined.contains("above"), "{joined}");
    assert!(screen.iter().any(|l| l.contains("↓ ") && l.contains(" more lines below")), "{joined}");
    // No row of the box runs past its edge: every word of the long summary is on screen whole, over several rows.
    let rows = body(&screen);
    let inside = rows.join(" ");
    for word in "The basket page shows every item with its size, colour and price, and the totals at the foot".split(' ') {
        assert!(inside.split_whitespace().any(|x| x == word), "{word:?} is not whole on screen:\n{joined}");
    }
    let (width, _) = app.handoff.view.get();
    for r in &rows {
        assert!(crate::textbox::width_of(&r.trim_end().chars().collect::<Vec<_>>()) <= width, "{r:?}");
    }
    // The cursor is at the top left of the text.
    let pos = t.get_cursor_position().unwrap();
    assert_eq!(screen[pos.y as usize].chars().nth(pos.x as usize), Some('#'), "{pos:?}");

    // PageDown moves down a page: the view shows what was below and says there is more above now.
    key(&mut app, KeyCode::PageDown);
    let screen = draw(&mut t, &app);
    let joined = screen.join("\n");
    assert!(screen.iter().any(|l| l.contains("↑ ") && l.contains(" more lines above")), "{joined}");
    assert!(!joined.contains("# Session hand-off:"), "{joined}");
    // Down through the whole document: the view follows, and the last step comes into view with nothing below.
    for _ in 0..200 {
        key(&mut app, KeyCode::Down);
    }
    let screen = draw(&mut t, &app);
    let joined = screen.join("\n");
    assert!(joined.contains(&format!("{n}. Step number {n}", n = handoff::NEXT_MAX)), "{joined}");
    assert!(!joined.contains("more lines below") && !joined.contains("more line below"), "{joined}");
    let pos = t.get_cursor_position().unwrap();
    // The document ends with a line break, so the very end is the empty row under the last step.
    assert!(screen[pos.y as usize - 1].contains("the next one"), "the cursor is on the last row:\n{joined}");
    assert_eq!(app.handoff.cursor, app.handoff.text.chars().count());
    // Typing at the cursor lands where the cursor is shown.
    key(&mut app, KeyCode::End);
    for c in " Done.".chars() {
        key(&mut app, KeyCode::Char(c));
    }
    let screen = draw(&mut t, &app);
    let pos = t.get_cursor_position().unwrap();
    let before: String = screen[pos.y as usize].chars().take(pos.x as usize).collect();
    assert!(before.ends_with("Done."), "{before:?}");
    // PageUp all the way back to the top.
    for _ in 0..20 {
        key(&mut app, KeyCode::PageUp);
    }
    let screen = draw(&mut t, &app);
    assert!(screen.join("\n").contains("# Session hand-off:"));
    assert!(!screen.iter().any(|l| l.contains("above")));
}

#[test]
fn up_and_down_move_by_rows_on_screen_and_keep_their_column() {
    let (w, fake) = world("handoff-rows");
    fake.reply_default(BRIEF);
    let mut app = app_of(&w, &fake);
    select(&mut app, "sess-1");
    key(&mut app, KeyCode::Char('h'));
    wait_ready(&mut app);
    // A box 20 columns wide, 5 rows high, holding one long line.
    app.handoff.view.set((20, 5));
    let line = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu";
    app.handoff.text = line.to_string();
    app.handoff.cursor = 0;
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Down);
    assert_eq!(app.handoff.cursor_row_col(), (1, 2), "one row down, same column");
    assert_eq!(app.handoff.cursor_at().0, 0, "and still the same line as typed");
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    assert_eq!(app.handoff.cursor_row_col().0, 3);
    key(&mut app, KeyCode::Up);
    assert_eq!(app.handoff.cursor_row_col(), (2, 2));
    // Home and End go to the ends of the line as typed, not of the row.
    key(&mut app, KeyCode::End);
    assert_eq!(app.handoff.cursor, line.chars().count());
    key(&mut app, KeyCode::Home);
    assert_eq!(app.handoff.cursor, 0);
    // Wide characters and a paste: the cursor's column counts two for each wide character.
    app.on_paste("日本語 ");
    assert_eq!(app.handoff.cursor, 4);
    assert_eq!(app.handoff.cursor_row_col(), (0, 7));
    // A paste can't bring a control character or an escape sequence in with it.
    app.on_paste("\u{1b}[2Jx\ty\r\nz");
    assert!(app.handoff.text.starts_with("日本語 [2Jx    y\nzalpha"), "{:?}", app.handoff.text);
}

#[test]
fn the_wheel_scrolls_the_document_and_a_long_message_while_there_is_no_document() {
    use ratatui::crossterm::event::{MouseEvent, MouseEventKind};
    let wheel = |app: &mut App, down: bool| {
        let kind = if down { MouseEventKind::ScrollDown } else { MouseEventKind::ScrollUp };
        app.on_mouse(MouseEvent { kind, column: 30, row: 10, modifiers: KeyModifiers::NONE });
    };
    let (w, fake) = world("handoff-wheel");
    fake.reply_default(&long_brief());
    let mut app = app_of(&w, &fake);
    let mut t = term(70, 24);
    select(&mut app, "sess-1");
    key(&mut app, KeyCode::Char('h'));
    wait_ready(&mut app);
    draw(&mut t, &app);
    wheel(&mut app, false);
    assert_eq!(app.handoff.top.get(), 0, "nothing above the top");
    wheel(&mut app, true);
    wheel(&mut app, true);
    let screen = draw(&mut t, &app);
    assert_eq!(app.handoff.top.get(), 6, "three rows a turn");
    assert!(screen.iter().any(|l| l.contains("↑ 6 more lines above")), "{}", screen.join("\n"));
    // The cursor was taken along so it stays in view (the draw would otherwise pull the view back to it).
    assert_eq!(app.handoff.cursor_row_col().0, 6, "on the top row that shows");
    let pos = t.get_cursor_position().unwrap();
    let first = screen.iter().position(|l| l.contains("↑ 6 more lines above")).unwrap();
    assert_eq!(pos.y as usize, first + 1, "and drawn there");
    key(&mut app, KeyCode::Esc);

    // A failure message too long for the box can be scrolled to its end with the wheel and the arrows. (No real
    // failure says this much today; the box must still cope if one ever does.)
    fake.hang(true);
    key(&mut app, KeyCode::Char('h'));
    let long = format!("{} Nothing has changed.", "The call could not be made because of something long. ".repeat(40));
    app.handoff.state = HandoffState::Failed(long);
    let screen = draw(&mut t, &app);
    assert!(screen.iter().any(|l| l.contains("more lines below")), "{}", screen.join("\n"));
    for _ in 0..100 {
        wheel(&mut app, true);
    }
    let screen = draw(&mut t, &app);
    let joined = screen.join("\n");
    assert!(joined.contains("Nothing has changed.") && !joined.contains("more lines below"), "{joined}");
    assert!(joined.contains("more lines above"), "{joined}");
    // And back up with the arrow keys, a row at a time.
    let top = app.handoff.top.get();
    key(&mut app, KeyCode::Up);
    assert_eq!(app.handoff.top.get(), top - 1);
    key(&mut app, KeyCode::Esc);
    app.refresh_until_for_test(10, "the cancelled call to end", |a| !a.handoff_busy());
}

// ---- a session joining a project tells the notes -------------------------------------------------------------

/// The mark the notes helper waits on for this project, if there is one.
fn due(w: &World, id: &str) -> Option<u64> {
    crate::notesrun::due_at(&w.cache, id)
}

#[test]
fn putting_a_session_in_a_project_with_m_marks_it_so_its_notes_are_updated() {
    let (w, fake) = world("joined-m");
    w.transcript("loose-2", &other(&w), "Something else entirely", 4);
    let id = project_id(&w);
    let mut app = app_of(&w, &fake);
    let _ = fs::remove_file(crate::notesrun::due_path(&w.cache, &id));
    assert_eq!(due(&w, &id), None);

    select(&mut app, "loose-2");
    key(&mut app, KeyCode::Char('m'));
    assert!(matches!(app.mode, Mode::Assign));
    key(&mut app, KeyCode::Enter); // the only project there is
    assert_eq!(Projects::open(&w.config).project_id_of("loose-2"), Some(id.as_str()));
    assert!(due(&w, &id).is_some(), "the project is marked, so an update runs without anyone resuming anything");

    // Taking it out again marks nothing new.
    let was = due(&w, &id);
    select(&mut app, "loose-2");
    key(&mut app, KeyCode::Char('m'));
    key(&mut app, KeyCode::Down); // past the one project, to "New project…"
    key(&mut app, KeyCode::Down); // and on to "No project"
    key(&mut app, KeyCode::Enter);
    assert_eq!(Projects::open(&w.config).project_id_of("loose-2"), None);
    assert_eq!(due(&w, &id), was, "leaving a project is not something to update from");
}

#[test]
fn a_session_started_under_a_project_marks_it_as_soon_as_it_is_placed() {
    let (mut w, fake) = world("joined-launch");
    let id = project_id(&w);
    let mut app = app_of(&w, &fake);
    let _ = fs::remove_file(crate::notesrun::due_path(&w.cache, &id));

    // A session platoon started under the project: its pid is noted, and its id shows up a moment later.
    let pid = w.live("new-3", &shop(&w), crate::testkit::now_ms(), "idle");
    crate::projects::note_launch(&w.cache, pid, &id, crate::testkit::now_ms() - 5_000).unwrap();
    app.refresh_with(Vec::new(), false);
    assert_eq!(Projects::open(&w.config).project_id_of("new-3"), Some(id.as_str()), "placed by the launch");
    assert!(due(&w, &id).is_some(), "and the project was told there is something new");

    // A refresh that places nothing marks nothing new.
    let was = due(&w, &id);
    app.refresh_with(Vec::new(), false);
    assert_eq!(due(&w, &id), was);
}

#[test]
fn the_session_start_hook_marks_the_project_it_places_a_session_into() {
    let (w, _fake) = world("joined-hook");
    let id = project_id(&w);
    let _ = fs::remove_file(crate::notesrun::due_path(&w.cache, &id));
    let input = serde_json::json!({"session_id": "brand-new", "source": "startup", "cwd": "x", "hook_event_name": "SessionStart"}).to_string();
    let out = crate::sessionhook::session_start(&input, Some(&id), &w.config, &w.cache);
    assert!(out.is_some_and(|o| o.contains("Sell socks.")), "the notes still go to the session");
    assert_eq!(Projects::open(&w.config).project_id_of("brand-new"), Some(id.as_str()));
    assert!(due(&w, &id).is_some(), "and the project is marked");

    // A session that was already in the project (a plain resume) places nothing and marks nothing new.
    let was = due(&w, &id);
    let input = serde_json::json!({"session_id": "sess-1", "source": "resume", "cwd": "x", "hook_event_name": "SessionStart"}).to_string();
    crate::sessionhook::session_start(&input, Some(&id), &w.config, &w.cache);
    assert_eq!(due(&w, &id), was);
}
