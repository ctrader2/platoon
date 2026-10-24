//! Codex's conversations in the list, at the level of the app: the sample sessions (`src/testdata/codex`) in a
//! world's Codex folder, real keys in, what the rows, the search and the notices say out. Every folder is a temp dir
//! (`World`).

use crate::agents::Agent;
use crate::app::{App, FindState, HandoffState, JumpPlan, Mode};
use crate::model::{NameSource, Status};
use crate::settings;
use crate::testkit::{AiFake, World};
use crate::ui;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
};
use serde_json::json;
use std::fs;

const FINISHED: &str = "01a1517c-414d-7902-93d1-efa74830493d";
const INTERRUPTED: &str = "01a15181-5d10-7fc3-8da2-6d27c8100fb6";
const PLANNING: &str = "01a15181-dc6f-7b73-81cf-c041b14f6432";
const CLAUDE: &str = "cccccccc-1111-2222-3333-444444444444";

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn screen(term: &mut Terminal<TestBackend>, app: &App) -> String {
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

/// The three sample Codex conversations, and one of Claude Code's last active between the first and the second of
/// them (2026-10-19 00:10 UTC).
fn world(name: &str) -> World {
    let w = World::new(name);
    w.codex_samples();
    w.transcript_lines(
        CLAUDE,
        &[
            json!({"type":"user","cwd":"/work/other","timestamp":"2026-10-19T00:09:00.000Z","message":{"role":"user","content":"tidy the parser"}}).to_string(),
            json!({"type":"assistant","cwd":"/work/other","timestamp":"2026-10-19T00:10:00.000Z","message":{"model":"claude-sonnet-5","usage":{"input_tokens":20_000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}).to_string(),
        ],
    );
    w
}

fn select(app: &mut App, id: &str) {
    app.selected = app.visible().iter().position(|&i| app.sessions[i].id == id).unwrap_or_else(|| panic!("{id} isn't listed"));
}

#[test]
fn codex_conversations_are_rows_beside_claudes_in_the_order_of_their_last_activity() {
    let w = world("codex-rows");
    let app = w.codex_app();
    let ids: Vec<&str> = app.visible().iter().map(|&i| app.sessions[i].id.as_str()).collect();
    assert_eq!(ids, [PLANNING, INTERRUPTED, CLAUDE, FINISHED]);
    let s = app.sessions.iter().find(|s| s.id == FINISHED).unwrap();
    assert_eq!(s.agent, Agent::Codex);
    assert_eq!((s.name.as_str(), s.dir.as_str(), s.cwd.as_str()), ("repo", "/work/repo", "/work/repo"));
    assert_eq!(s.model.as_deref(), Some("GPT-6-Luna"), "the model's name, from the rollout");
    assert_eq!(s.description, "Identify colour from notes.txt", "the name Codex gave it");
    assert_eq!(s.context_pct, 5, "14,144 of the 258,400 tokens the rollout says the window has");
    assert_eq!((s.status, s.procs.len(), s.branch.as_deref()), (Status::Idle, 0, Some("master")));
    let c = app.sessions.iter().find(|s| s.id == CLAUDE).unwrap();
    assert_eq!((c.agent, c.model.as_deref(), c.context_pct), (Agent::Claude, Some("Sonnet 5"), 10), "Claude Code's row as ever");

    let mut term = Terminal::new(TestBackend::new(140, 20)).unwrap();
    let shown = screen(&mut term, &app);
    // The model is on the row's first line, the name Codex gave it on its second.
    let lines: Vec<&str> = shown.lines().collect();
    let second = lines.iter().position(|l| l.contains("Identify colour")).unwrap_or_else(|| panic!("{shown}"));
    assert!(lines[second - 1].contains("GPT-6-Luna") && lines[second - 1].contains("/work/repo"), "{shown}");
}

#[test]
fn before_its_first_reply_a_codex_rows_model_cell_says_codex_where_claudes_says_a_dash() {
    let w = World::new("codex-no-model");
    let day = w.codex().join("sessions/2026/10/19");
    fs::create_dir_all(&day).unwrap();
    let id = "01a20000-0000-7000-8000-000000000001";
    let lines = [
        json!({"timestamp":"2026-10-19T09:00:00.000Z","type":"session_meta","payload":{"id":id,"cwd":"/work/fresh","originator":"codex-tui","cli_version":"0.163.0","source":"cli"}}),
        json!({"timestamp":"2026-10-19T09:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"first words only"}]}}),
    ];
    fs::write(day.join(format!("rollout-2026-10-19T09-00-00-{id}.jsonl")), lines.map(|l| l.to_string()).join("\n") + "\n").unwrap();
    w.transcript_lines(
        CLAUDE,
        &[json!({"type":"user","cwd":"/work/other","timestamp":"2026-10-19T08:00:00.000Z","message":{"role":"user","content":"no reply yet"}}).to_string()],
    );
    let app = w.codex_app();
    assert_eq!(app.sessions.iter().find(|s| s.id == id).unwrap().model, None);
    let mut term = Terminal::new(TestBackend::new(140, 20)).unwrap();
    let shown = screen(&mut term, &app);
    // Columns count cells: the header's MODEL and each row's cell under it, on the row's first line (with its folder).
    let chars = |l: &str| l.chars().collect::<Vec<char>>();
    let header = shown.lines().find(|l| l.contains("MODEL")).map(chars).unwrap();
    let col = (0..header.len()).find(|&i| header[i..].starts_with(&['M', 'O', 'D', 'E', 'L'])).unwrap();
    let cell = |dir: &str| {
        let line = shown.lines().find(|l| l.contains(dir)).map(chars).unwrap_or_else(|| panic!("{dir}:\n{shown}"));
        line[col..].iter().collect::<String>().split_whitespace().next().unwrap_or_default().to_string()
    };
    assert_eq!(cell("/work/fresh"), "Codex", "{shown}");
    assert_eq!(cell("/work/other"), "-", "{shown}");
}

#[test]
fn a_name_codex_gives_later_reaches_the_row_though_the_conversation_itself_did_not_change() {
    let w = world("codex-rename");
    let mut app = w.codex_app();
    let desc = |app: &App| app.sessions.iter().find(|s| s.id == INTERRUPTED).unwrap().description.clone();
    assert_eq!(desc(&app), "Run network curl command");
    let mut index = fs::read_to_string(w.codex().join("session_index.jsonl")).unwrap();
    index
        .push_str(&json!({"id": INTERRUPTED, "thread_name": "Curl through the sandbox", "updated_at": "2026-10-19T01:00:00Z"}).to_string());
    index.push('\n');
    fs::write(w.codex().join("session_index.jsonl"), index).unwrap();
    app.refresh_for_test();
    assert_eq!(desc(&app), "Curl through the sandbox");
}

#[test]
fn the_full_text_search_finds_a_codex_conversation_by_what_the_user_typed_in_it() {
    let w = world("codex-search");
    let mut app = w.codex_app();
    key(&mut app, KeyCode::Char('/'));
    app.set_search_kind(1);
    for c in "colour notes".chars() {
        key(&mut app, KeyCode::Char(c));
    }
    let found: Vec<String> = app.search.results.iter().map(|r| r.id.clone()).collect();
    assert_eq!(found, [FINISHED], "the prompt, not Codex's own blocks around it");
    key(&mut app, KeyCode::Esc);
    // Codex's own environment block and the hook's prompt are not the user's words.
    key(&mut app, KeyCode::Char('/'));
    app.set_search_kind(1);
    for c in "CHECKED".chars() {
        key(&mut app, KeyCode::Char(c));
    }
    assert!(app.search.results.is_empty(), "a reply (and a hook's prompt) is not something the user typed");
}

/// The token features on (the master switch, names and Describe as asked), a stand-in for `claude -p` answering every
/// call (the smart features' model is Claude Code's Haiku here, whatever the conversation's agent).
fn smart_on(w: &World, names: bool, find: bool) -> AiFake {
    let mut s = settings::load(&w.config).0;
    s.tokens_enabled = true;
    s.auto_archive_days = 0;
    s.set_feature("names", names);
    s.set_feature("find", find);
    settings::save(&w.config, &s).unwrap();
    AiFake::new(&w.base.file_name().unwrap().to_string_lossy())
}

/// The longer sample conversation (`testdata/codex/conversation`: a basket page, then the checkout) run in `folder`,
/// a real one, in this world's Codex folder.
const SHOP: &str = "01a20000-1111-7000-8000-00000000c0de";

fn shop_conversation(w: &World, folder: &std::path::Path) {
    let day = w.codex().join("sessions/2026/10/20");
    fs::create_dir_all(&day).unwrap();
    fs::create_dir_all(folder).unwrap();
    let name = format!("rollout-2026-10-20T09-00-00-{SHOP}.jsonl");
    let text =
        fs::read_to_string(std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src/testdata/codex/conversation")).join(&name))
            .unwrap()
            .replace("/work/shop", &folder.to_string_lossy());
    fs::write(day.join(name), text).unwrap();
}

#[test]
fn codex_conversations_are_named_and_summarized_as_claudes_are_and_codexs_own_name_stays_their_description() {
    let w = world("codex-names");
    let fake = smart_on(&w, true, false);
    let mut app = w.codex_app();
    app.set_ai_program_for_test(&fake.script);
    app.settle_ai_for_test(20);
    let sent: String = (1..=fake.calls()).map(|k| fake.stdin(k)).collect();
    // What is sent of a Codex conversation: what the user typed and a line of Codex's last reply, as for Claude's.
    assert!(
        sent.contains("First message: Read notes.txt and tell me the colour. Keep it to one line.\nCodex's last reply: CHECKED\n"),
        "{sent}"
    );
    assert!(sent.contains("First message: tidy the parser\n"), "Claude Code's conversation as ever: {sent}");
    assert!(!sent.contains("Claude's last reply: CHECKED") && !sent.contains("environment_context"), "{sent}");
    for id in [FINISHED, INTERRUPTED, PLANNING, CLAUDE] {
        let s = app.sessions.iter().find(|s| s.id == id).unwrap();
        assert_eq!(s.name_source, NameSource::Generated, "{id}");
        assert!(s.name.starts_with("Name ") && s.summary.starts_with("Summary "), "{id}: {} / {}", s.name, s.summary);
    }
    // The name Codex gave the conversation is still its description line, as Claude Code's own title is.
    assert_eq!(app.sessions.iter().find(|s| s.id == FINISHED).unwrap().description, "Identify colour from notes.txt");
    let calls = fake.calls();
    app.settle_ai_for_test(5);
    assert_eq!(fake.calls(), calls, "named once: nothing new, nothing due");
}

#[test]
fn describe_it_finds_a_codex_conversation() {
    let w = world("codex-find");
    let fake = smart_on(&w, false, true);
    let mut app = w.codex_app();
    app.set_ai_program_for_test(&fake.script);
    key(&mut app, KeyCode::Char('/'));
    app.set_search_kind(crate::app::FIND_KIND);
    for c in "the colour in the notes".chars() {
        key(&mut app, KeyCode::Char(c));
    }
    key(&mut app, KeyCode::Enter);
    app.refresh_until_for_test(10, "the answer", |a| *a.find_state() == FindState::Done);
    let sent = fake.stdin(1);
    assert!(
        sent.contains("[1]\nName: repo\nFolder: repo\nFirst message: Read notes.txt and tell me the colour."),
        "the best match first: {sent}"
    );
    assert_eq!(app.search.results.first().map(|r| r.id.as_str()), Some(FINISHED));
}

#[test]
fn t_drafts_tasks_from_a_codex_conversation_as_from_a_claude_one() {
    let w = World::new("codex-draft");
    let folder = w.base.join("shop");
    shop_conversation(&w, &folder);
    let fake = smart_on(&w, false, false);
    fake.reply(
        1,
        r#"{"tasks":[{"title":"Take card payments","does":"Checkout takes card payments through a payment provider.","done_when":["A test order is paid by card"],"squad":"feature"}]}"#,
    );
    let mut app = w.codex_app();
    app.set_ai_program_for_test(&fake.script);
    select(&mut app, SHOP);
    key(&mut app, KeyCode::Char('t'));
    app.refresh_until_for_test(20, "the draft", |a| matches!(a.mode, Mode::TaskDraft));
    let sent = fake.stdin(1);
    for want in [
        "- Add a basket page to the shop, with sizes and colours for each item.",
        "- Now make the checkout remember the basket between visits; card payments can wait for later.",
        "Codex's last reply: The checkout now keeps the basket between visits. I ran cargo test: 12 passed.",
    ] {
        assert!(sent.contains(want), "{want}: {sent}");
    }
    assert!(!sent.contains("src/basket.rs src/sizes.rs") && !sent.contains("- yes"), "no tool output, no \"yes\": {sent}");
    key(&mut app, KeyCode::Enter);
    let store = crate::tickets::Tickets::open(&w.config, crate::tickets::NO_PROJECT_ID);
    let t = &store.all()[0];
    assert_eq!((t.title.as_str(), t.folder.as_deref()), ("Take card payments", Some(folder.to_string_lossy().as_ref())));
    assert_eq!(t.from.as_ref().map(|f| f.session.as_str()), Some(SHOP), "it remembers the conversation it came from");
}

/// The longer conversation in a project, the token features on and a stand-in answering the hand-off.
fn handoff_world(name: &str) -> (World, App, AiFake, String, std::path::PathBuf) {
    let w = World::new(name);
    let folder = w.base.join("shop");
    shop_conversation(&w, &folder);
    let fake = smart_on(&w, false, false);
    fake.reply_default(r#"{"summary":"The basket page is in and the checkout keeps it.","next":["Take card payments."]}"#);
    let mut app = w.codex_app();
    app.set_ai_program_for_test(&fake.script);
    let project = app.projects.create("Web shop").unwrap();
    app.projects.assign(SHOP, Some(&project)).unwrap();
    app.reload_for_test();
    select(&mut app, SHOP);
    (w, app, fake, project, folder)
}

#[test]
fn h_hands_a_codex_conversation_on_to_a_new_codex_session_with_the_document_as_its_first_message() {
    let (_w, mut app, fake, project, folder) = handoff_world("codex-handoff");
    key(&mut app, KeyCode::Char('h'));
    app.refresh_until_for_test(10, "the brief", |a| a.handoff.state != HandoffState::Asking);
    assert_eq!(app.handoff.state, HandoffState::Ready, "{}", app.handoff.shown());
    let sent = fake.stdin(1);
    assert!(sent.contains("- Now make the checkout remember the basket between visits"), "{sent}");
    assert!(sent.contains("Codex's last reply: The checkout now keeps the basket"), "{sent}");
    let doc = app.handoff.text.clone();
    for want in [
        "# Session hand-off: shop\n".to_string(),
        format!("- Previous session id: {SHOP} (`codex resume {SHOP}` opens it)\n"),
        "- Model: GPT-6-Luna, context about 6% full\n".into(),
        "- Messages from the user: 2 (short replies like \"yes\" not counted)\n".into(),
        "- Tokens the previous session used: 20,000 sent to Codex and 900 written by Codex, plus 40,000 read again from Codex's cache\n"
            .into(),
        "## Executive summary\n\nThe basket page is in and the checkout keeps it.\n".into(),
    ] {
        assert!(doc.contains(&want), "{want}: {doc}");
    }
    assert!(!doc.contains("claude --resume"), "{doc}");

    // ctrl+s: a new Codex session, in the same folder and project, the document its first message.
    app.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    assert_eq!(app.launched.len(), 1, "{:?}", app.active_notice());
    let l = &app.launched[0];
    assert_eq!((l.agent, l.dir.as_str()), (Agent::Codex, folder.to_string_lossy().as_ref()));
    assert_eq!(l.args, ["--no-daemon".to_string(), doc.trim().to_string()]);
    assert_eq!(l.env, [("PLATOON_PROJECT".to_string(), project), ("PLATOON_OFFER_TASKS".to_string(), "1".to_string())]);
    assert!(app.active_notice().is_some_and(|n| n.contains("Started a new session in the project \"Web shop\"")));
}

#[test]
fn a_codex_conversation_is_not_handed_on_when_codex_is_not_installed_and_nothing_is_asked() {
    let (_w, mut app, fake, _, _) = handoff_world("codex-handoff-gone");
    app.set_providers_for_test(crate::agents::Providers { claude: Some(fake.script.clone()), codex: None });
    key(&mut app, KeyCode::Char('h'));
    assert!(matches!(app.mode, Mode::List));
    let notice = app.active_notice().unwrap_or_default().to_string();
    assert_eq!(notice, "\"shop\" is a Codex conversation, and Codex isn't installed on this computer, so nothing was started.");
    assert_eq!(fake.calls(), 0);
    assert!(app.launched.is_empty());
}

#[test]
fn an_idle_codex_conversation_is_resumed_and_one_its_host_just_started_is_joined() {
    let w = world("codex-resume");
    let mut app = w.codex_app();
    select(&mut app, FINISHED);
    assert_eq!(app.jump_plan(), Some(JumpPlan::Resume));
    // A host platoon started for it a moment ago, before Codex said it holds the conversation: joined, not a second one.
    app.hosts.push(crate::host::HostInfo {
        agent: Agent::Codex,
        claude_pid: 4_000_123,
        host_pid: 1,
        dir: "/work/repo".into(),
        args: Agent::Codex.args(crate::agents::Start::Resume(FINISHED)),
        sock: w.base.join("none.sock"),
    });
    assert_eq!(app.jump_plan(), Some(JumpPlan::Attach(4_000_123)));
}

/// A process of the test's own holds a conversation's writer lock, as another Codex window (or Codex's background
/// server) does: the row is running, opening it says where it is and does nothing else, archiving it is refused, and
/// nothing is ever signalled. Linux: the holder is read from `/proc/locks`.
#[cfg(target_os = "linux")]
#[test]
fn a_conversation_open_in_another_codex_window_is_shown_running_and_never_opened_twice_or_stopped() {
    use std::process::{Command, Stdio};
    let w = world("codex-held");
    let locks = w.codex().join("thread-writer-locks");
    fs::create_dir_all(&locks).unwrap();
    let lock = locks.join(format!("{PLANNING}.lock"));
    fs::write(&lock, "").unwrap();
    let mut holder = Command::new("flock").arg(&lock).args(["sleep", "60"]).stdin(Stdio::null()).spawn().unwrap();
    let mut app = w.codex_app();
    let start = std::time::Instant::now();
    while !app.sessions.iter().any(|s| s.id == PLANNING && !s.procs.is_empty()) {
        assert!(start.elapsed() < std::time::Duration::from_secs(10), "the lock's holder was never found");
        std::thread::sleep(std::time::Duration::from_millis(20));
        app.refresh_for_test();
    }
    let s = app.sessions.iter().find(|s| s.id == PLANNING).unwrap();
    assert_eq!((s.procs[0].pid, s.procs[0].hosted, s.status), (holder.id(), false, Status::Idle), "running, not platoon's");
    assert!(app.session_running(s) && s.held_elsewhere());

    select(&mut app, PLANNING);
    assert_eq!(app.jump_plan(), Some(JumpPlan::Elsewhere));
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List), "nothing opened, nothing asked");
    let notice = app.active_notice().unwrap_or_default().to_string();
    assert!(notice.contains("is open in another Codex window") && notice.contains("Close it there"), "{notice}");

    key(&mut app, KeyCode::Char('a'));
    assert!(matches!(app.mode, Mode::List), "no question about stopping it");
    assert!(app.active_notice().is_some_and(|n| n.contains("Close it there to archive it")), "{:?}", app.active_notice());
    assert_eq!(app.stops_pending(), 0);
    assert_eq!(app.archive_writes_for_test(), 0);
    assert!(crate::testkit::alive(holder.id()), "never signalled");
    let _ = holder.kill();
    let _ = holder.wait();
    app.refresh_for_test();
    let s = app.sessions.iter().find(|s| s.id == PLANNING).unwrap();
    assert!(s.procs.is_empty() && !s.held_elsewhere(), "closed there: an ordinary conversation again");
}

// ---- new sessions: `n` asks which agent, and a new Codex session's row before Codex saves it -----------------------------

/// `n`, then the folder `dir` typed into the picker and taken.
fn new_session_in(app: &mut App, dir: &std::path::Path) {
    key(app, KeyCode::Char('n'));
    assert!(matches!(app.mode, Mode::Picker));
    for c in dir.to_string_lossy().chars() {
        key(app, KeyCode::Char(c));
    }
    key(app, KeyCode::Enter);
}

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

/// Where the question drew agent `i`'s row (its clickable area).
fn agent_row(app: &App, i: usize) -> ratatui::layout::Rect {
    app.hits.borrow().iter().find(|(_, h)| matches!(h, crate::app::Hit::AgentRow(k) if *k == i)).map(|(r, _)| *r).unwrap()
}

/// The cell of agent `i`'s name in the question: (the row's start, the mark, the star, then the name).
fn agent_name_cell(term: &Terminal<TestBackend>, app: &App, i: usize) -> ratatui::buffer::Cell {
    let r = agent_row(app, i);
    term.backend().buffer()[(r.x + 4, r.y)].clone()
}

/// Nothing about a new session's agent is in the settings file.
fn nothing_remembered(w: &World) -> bool {
    fs::read_to_string(w.config.join("settings.json")).map_or(true, |t| !t.contains("new_session_agent"))
}

#[test]
fn with_both_installed_n_asks_which_agent_with_the_last_choice_picked_and_remembers_it() {
    let w = world("codex-new-both");
    let work = w.base.join("work");
    fs::create_dir_all(&work).unwrap();
    let mut app = w.codex_app();
    assert_eq!(app.providers().installed(), [Agent::Claude, Agent::Codex]);
    new_session_in(&mut app, &work);
    assert!(matches!(app.mode, Mode::PickAgent), "asked, nothing started yet");
    assert!(app.launched.is_empty());
    let p = app.agent_pick().unwrap();
    assert_eq!((p.options.as_slice(), p.selected, p.last), (&[Agent::Claude, Agent::Codex][..], 0, None), "Claude Code first");
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let shown = screen(&mut term, &app);
    for want in [
        "New session",
        "Start in Claude Code or Codex?",
        "Claude Code",
        "Codex",
        "The one you choose is picked here next time.",
        "enter start",
    ] {
        assert!(shown.contains(want), "{want}:\n{shown}");
    }
    let t = app.effective_theme().clone();
    assert_eq!(agent_name_cell(&term, &app, 0).symbol(), "C");
    assert_eq!(agent_name_cell(&term, &app, 0).bg, t.surface, "the picked one on the selection's fill");
    assert_eq!(agent_name_cell(&term, &app, 1).bg, t.bg);
    assert_eq!(cell_of(&term, "The one you choose").fg, crate::ui::hint_fg(&t));

    // Codex, by the keys: started, with the offer, and remembered.
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.launched.len(), 1);
    let l = &app.launched[0];
    assert_eq!((l.agent, l.dir.as_str(), l.args.as_slice()), (Agent::Codex, work.to_str().unwrap(), &["--no-daemon".to_string()][..]));
    assert_eq!(l.env, [("PLATOON_OFFER_TASKS".to_string(), "1".to_string())]);
    assert_eq!(settings::load(&w.config).0.new_session_agent.as_deref(), Some("codex"));

    // Next time Codex is picked in advance and starred: enter is the one extra key.
    new_session_in(&mut app, &work);
    let p = app.agent_pick().unwrap();
    assert_eq!((p.selected, p.last), (1, Some(Agent::Codex)));
    key(&mut app, KeyCode::Up);
    let shown = screen(&mut term, &app);
    assert!(shown.contains("★ your last choice: enter starts it again."), "{shown}");
    assert_eq!(cell_of(&term, "★ Codex").fg, t.accent, "the star on the one chosen last time");
    assert_eq!(agent_name_cell(&term, &app, 0).bg, t.surface);
    // A click picks, a click on the picked one starts.
    let row = agent_row(&app, 1);
    let click = ratatui::crossterm::event::MouseEvent {
        kind: ratatui::crossterm::event::MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left),
        column: row.x + 4,
        row: row.y,
        modifiers: KeyModifiers::NONE,
    };
    app.on_mouse(click);
    assert_eq!(app.agent_pick().map(|p| p.selected), Some(1));
    app.on_mouse(click);
    assert_eq!(app.launched.iter().map(|l| l.agent).collect::<Vec<_>>(), [Agent::Codex, Agent::Codex]);

    // Claude Code chosen: remembered in its turn, and started as before (no first message, the offer).
    new_session_in(&mut app, &work);
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Enter);
    let l = app.launched.last().unwrap();
    assert_eq!((l.agent, l.args.is_empty()), (Agent::Claude, true));
    assert_eq!(settings::load(&w.config).0.new_session_agent.as_deref(), Some("claude"));
}

#[test]
fn esc_in_the_question_starts_nothing_and_remembers_nothing() {
    let w = world("codex-new-esc");
    let work = w.base.join("work");
    fs::create_dir_all(&work).unwrap();
    let mut app = w.codex_app();
    new_session_in(&mut app, &work);
    assert!(matches!(app.mode, Mode::PickAgent));
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List) && app.agent_pick().is_none());
    assert!(app.launched.is_empty());
    assert!(nothing_remembered(&w));
}

#[test]
fn with_one_agent_installed_n_starts_it_without_asking() {
    // Claude Code only (a unit test's app without a Codex folder): as ever.
    let w = World::new("codex-new-claude-only");
    let work = w.base.join("work");
    fs::create_dir_all(&work).unwrap();
    let mut app = w.app();
    new_session_in(&mut app, &work);
    assert!(matches!(app.mode, Mode::List), "no question");
    assert_eq!(app.launched.iter().map(|l| (l.agent, l.args.len())).collect::<Vec<_>>(), [(Agent::Claude, 0)]);
    // Codex only.
    let w = world("codex-new-codex-only");
    fs::create_dir_all(&work).unwrap();
    let mut app = w.codex_app();
    let codex = app.providers().codex.clone();
    app.set_providers_for_test(crate::agents::Providers { claude: None, codex });
    new_session_in(&mut app, &work);
    assert!(matches!(app.mode, Mode::List), "no question");
    assert_eq!(
        app.launched.iter().map(|l| (l.agent, l.args.clone())).collect::<Vec<_>>(),
        [(Agent::Codex, vec!["--no-daemon".to_string()])]
    );
    assert!(nothing_remembered(&w), "nothing was chosen, nothing written");
}

/// A host of platoon's running a new Codex session as process `pid`, in `dir`.
fn new_codex_host(w: &World, pid: u32, dir: &std::path::Path) -> crate::host::HostInfo {
    crate::host::HostInfo {
        agent: Agent::Codex,
        claude_pid: pid,
        host_pid: 1,
        dir: dir.to_string_lossy().into_owned(),
        args: Agent::Codex.args(crate::agents::Start::New(None)),
        sock: w.base.join(format!("{pid}.sock")),
    }
}

#[test]
fn a_new_codex_session_has_a_row_before_its_first_message_that_opens_it_and_keeps_nothing_under_its_id() {
    let w = world("codex-unsaved");
    let work = w.base.join("fresh");
    fs::create_dir_all(&work).unwrap();
    let mut app = w.codex_app();
    let pid = 4_000_321;
    app.refresh_with(vec![new_codex_host(&w, pid, &work)], false);
    let id = crate::model::unsaved_id(pid);
    let s = app.sessions.iter().find(|s| s.id == id).unwrap_or_else(|| panic!("no row for the new session"));
    assert_eq!((s.agent, s.name.as_str(), s.model.as_deref(), s.status), (Agent::Codex, "fresh", None, Status::Idle));
    assert!(s.unsaved && s.procs.len() == 1 && s.procs[0].hosted);
    let mut term = Terminal::new(TestBackend::new(140, 20)).unwrap();
    let shown = screen(&mut term, &app);
    let line = shown.lines().find(|l| l.contains("fresh")).unwrap_or_else(|| panic!("{shown}"));
    assert!(line.contains("Codex") && line.contains('●'), "a running Codex row: {line}");
    select(&mut app, &id);
    assert_eq!(app.jump_plan(), Some(JumpPlan::Attach(pid)), "enter shows it");
    for (k, what) in [('a', "archived"), ('r', "renamed"), ('m', "put in a project"), ('h', "handed on"), ('t', "drafted into tasks")] {
        key(&mut app, KeyCode::Char(k));
        assert!(matches!(app.mode, Mode::List), "{k}");
        assert_eq!(
            app.active_notice(),
            Some(
                format!("\"fresh\" has no conversation yet: Codex saves it when you send your first message. Then it can be {what}.")
                    .as_str()
            ),
        );
    }
    assert!(!w.config.join("archive.json").exists() && !w.config.join("names.json").exists() && !w.config.join("projects.json").exists());
    // A resumed conversation's host is not a new one; a host gone takes its row with it.
    let mut resumed = new_codex_host(&w, pid, &work);
    resumed.args = Agent::Codex.args(crate::agents::Start::Resume(FINISHED));
    app.refresh_with(vec![resumed], false);
    assert!(!app.sessions.iter().any(|s| s.unsaved));
    app.refresh_with(Vec::new(), false);
    assert!(!app.sessions.iter().any(|s| s.unsaved));
}

/// A new Codex session in the order the real Codex 0.163 goes through it, from a project's screen: first no lock (its own
/// update and trust questions: the stand-in row), then the conversation's lock held by the host's own child with no
/// rollout yet (the session's prompt, nothing sent: the real id, the host's folder, still unsaved), then the rollout
/// (the first message). One row at every stage, in the project from the moment its id is known. Linux (`flock`,
/// `/proc/locks`).
#[cfg(target_os = "linux")]
#[test]
fn a_new_codex_session_is_one_row_before_the_lock_while_held_unsaved_and_once_saved() {
    use std::process::{Command, Stdio};
    let w = world("codex-unsaved-saved");
    let work = w.base.join("fresh");
    fs::create_dir_all(&work).unwrap();
    let id = "01a30000-0000-7000-8000-00000000beef";
    let locks = w.codex().join("thread-writer-locks");
    fs::create_dir_all(&locks).unwrap();
    // The stand-in "codex": waits for the session to open (the file `open`), then holds the conversation's lock itself,
    // in the same process (`exec`), as the real one does from then on.
    let open = w.base.join("open");
    let script = format!(
        "while [ ! -e '{}' ]; do sleep 0.02; done; exec flock '{}' sleep 60",
        open.display(),
        locks.join(format!("{id}.lock")).display()
    );
    let mut codex = Command::new("sh").args(["-c", &script]).current_dir(&w.base).stdin(Stdio::null()).spawn().unwrap();
    let mut app = w.codex_app();
    let project = app.projects.create("Web shop").unwrap();
    // Started from the project's screen: placed by its pid once its id shows up.
    crate::projects::note_launch(app.cache_for_test(), codex.id(), &project, crate::testkit::now_ms() - 5_000).unwrap();
    let host = new_codex_host(&w, codex.id(), &work);
    let rows = |app: &App| app.sessions.iter().filter(|s| s.procs.iter().any(|p| p.pid == codex.id())).count();

    // 1. Nothing held yet: the stand-in.
    app.refresh_with(vec![host.clone()], false);
    let stand_in = crate::model::unsaved_id(codex.id());
    assert!(app.sessions.iter().any(|s| s.id == stand_in && s.unsaved));
    assert_eq!(rows(&app), 1);
    assert_eq!(app.projects.project_id_of(&stand_in), None, "nothing is kept under a stand-in's id");

    // 2. The session is open: Codex holds the conversation, no rollout yet.
    fs::write(&open, "").unwrap();
    let start = std::time::Instant::now();
    while !app.sessions.iter().any(|s| s.id == id) {
        assert!(start.elapsed() < std::time::Duration::from_secs(10), "the held conversation never showed");
        std::thread::sleep(std::time::Duration::from_millis(20));
        app.refresh_with(vec![host.clone()], false);
    }
    assert_eq!(rows(&app), 1, "the stand-in gave way");
    let s = app.sessions.iter().find(|s| s.id == id).unwrap();
    assert_eq!(
        (s.name.as_str(), s.cwd.as_str(), s.unsaved),
        ("fresh", work.to_str().unwrap(), true),
        "the host's folder, not the process's"
    );
    assert_eq!((s.procs[0].pid, s.procs[0].hosted), (codex.id(), true));
    assert_eq!(app.projects.project_id_of(id), Some(project.as_str()), "placed by its pid, with its real id");
    let mut term = Terminal::new(TestBackend::new(140, 20)).unwrap();
    let shown = screen(&mut term, &app);
    let line = shown.lines().find(|l| l.contains("fresh")).unwrap_or_else(|| panic!("{shown}"));
    assert!(line.contains("Codex") && line.contains("Web shop"), "named by its folder, in its project: {line}");
    select(&mut app, id);
    for (k, what) in [('a', "archived"), ('r', "renamed"), ('m', "put in a project"), ('h', "handed on"), ('t', "drafted into tasks")] {
        key(&mut app, KeyCode::Char(k));
        assert!(matches!(app.mode, Mode::List), "{k}");
        assert_eq!(
            app.active_notice(),
            Some(
                format!("\"fresh\" has no conversation yet: Codex saves it when you send your first message. Then it can be {what}.")
                    .as_str()
            ),
        );
    }

    // 3. The first message: Codex writes the rollout.
    let day = w.codex().join("sessions/2026/10/24");
    fs::create_dir_all(&day).unwrap();
    let lines = [
        json!({"timestamp":"2026-10-24T09:00:00.000Z","type":"session_meta","payload":{"id":id,"cwd":work,"originator":"codex-tui","cli_version":"0.163.0","source":"cli"}}),
        json!({"timestamp":"2026-10-24T09:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello there codex"}]}}),
    ];
    fs::write(day.join(format!("rollout-2026-10-24T09-00-00-{id}.jsonl")), lines.map(|l| l.to_string()).join("\n") + "\n").unwrap();
    app.refresh_with(vec![host.clone()], false);
    let s = app.sessions.iter().find(|s| s.id == id).unwrap();
    assert_eq!((s.unsaved, s.description.as_str(), s.name.as_str()), (false, "hello there codex", "fresh"));
    assert_eq!(rows(&app), 1);
    assert_eq!(app.projects.project_id_of(id), Some(project.as_str()));
    let _ = codex.kill();
    let _ = codex.wait();
}

#[test]
fn each_stage_of_a_new_codex_session_is_one_row_with_the_hosts_folder() {
    use crate::agents::Live;
    let host = |pid: u32| crate::host::HostInfo {
        agent: Agent::Codex,
        claude_pid: pid,
        host_pid: 1,
        dir: "/home/u/shop".into(),
        args: Agent::Codex.args(crate::agents::Start::New(None)),
        sock: "/nonexistent/s.sock".into(),
    };
    let held = |pid: u32, cwd: &str, unsaved: bool| Live {
        agent: Agent::Codex,
        pid,
        id: "01a30000-0000-7000-8000-00000000beef".into(),
        cwd: cwd.into(),
        name: None,
        started_at: 5,
        busy: false,
        background: None,
        parked: None,
        unsaved,
    };
    let rows = |live: Vec<Live>| crate::app::new_codex_sessions(live, &[host(7)], |_| 9);
    let shape = |l: &Live| (l.id.clone(), l.cwd.clone(), l.unsaved, l.started_at);
    // Before the lock: one stand-in, the host's folder.
    assert_eq!(rows(Vec::new()).iter().map(shape).collect::<Vec<_>>(), [(crate::model::unsaved_id(7), "/home/u/shop".into(), true, 9)]);
    // Held, no rollout (the process's own folder, or none, from `Reader::live`): the host's folder wins, no stand-in.
    for cwd in ["", "/somewhere/else"] {
        let got = rows(vec![held(7, cwd, true)]);
        assert_eq!(
            got.iter().map(shape).collect::<Vec<_>>(),
            [("01a30000-0000-7000-8000-00000000beef".into(), "/home/u/shop".into(), true, 5)]
        );
    }
    // Saved: as its rollout says.
    let got = rows(vec![held(7, "/home/u/shop/sub", false)]);
    assert_eq!(
        got.iter().map(shape).collect::<Vec<_>>(),
        [("01a30000-0000-7000-8000-00000000beef".into(), "/home/u/shop/sub".into(), false, 5)]
    );
    // Held by a process that isn't a host's: left as it is (its own folder); the host still has its stand-in.
    let got = rows(vec![held(8, "/elsewhere", true)]);
    assert_eq!(got.len(), 2);
    assert_eq!(shape(&got[0]), ("01a30000-0000-7000-8000-00000000beef".into(), "/elsewhere".into(), true, 5));
    // A resumed conversation's host is never a new session.
    let mut resumed = host(7);
    resumed.args = Agent::Codex.args(crate::agents::Start::Resume("01a1517c-414d-7902-93d1-efa74830493d"));
    assert!(crate::app::new_codex_sessions(Vec::new(), &[resumed], |_| 9).is_empty());
}

// ---- the screens name the program that answers ----------------------------------------------------------------------

/// The smart features' model one of Codex's (GPT-6-Luna), answered by a stand-in `codex exec`; Claude Code installed
/// beside it (its stand-in is never asked).
fn on_codex(w: &World, app: &mut App, claude: &AiFake) -> AiFake {
    let codex = AiFake::codex(&format!("{}-exec", w.base.file_name().unwrap().to_string_lossy()));
    let mut s = settings::load(&w.config).0;
    s.ai_model = Some("gpt-6-luna".into());
    settings::save(&w.config, &s).unwrap();
    app.refresh_for_test(); // the settings file is read again
    app.set_ai_program_for_test(&claude.script);
    app.set_providers_for_test(crate::agents::Providers { claude: Some(claude.script.clone()), codex: Some(codex.as_codex(&w.codex())) });
    assert_eq!(app.smart_agent(), Agent::Codex);
    codex
}

#[test]
fn with_a_codex_model_the_hand_off_says_it_asks_codex_and_whose_answer_it_couldnt_read() {
    let (w, mut app, claude, _, _) = handoff_world("codex-asks-handoff");
    let codex = on_codex(&w, &mut app, &claude);
    codex.reply(1, "not json at all");
    select(&mut app, SHOP);
    key(&mut app, KeyCode::Char('h'));
    assert_eq!(app.handoff.shown(), "Asking Codex what to hand on from \"shop\"…");
    app.refresh_until_for_test(10, "the answer", |a| a.handoff.state != HandoffState::Asking);
    assert_eq!(app.handoff.shown(), "Codex's answer couldn't be understood, so there is no brief. Nothing has changed.");
    assert_eq!((claude.calls(), codex.calls()), (0, 1));
    // When git can't say which files changed, the document says whose account of them it is.
    key(&mut app, KeyCode::Esc);
    codex.reply(2, r#"{"summary":"The basket is in.","files":["src/basket.rs: new"]}"#);
    key(&mut app, KeyCode::Char('h'));
    app.refresh_until_for_test(10, "the brief", |a| a.handoff.state != HandoffState::Asking);
    assert!(
        app.handoff
            .text
            .contains("## Files affected\n\nAs Codex described them (this folder is not a git repository):\n- src/basket.rs: new\n"),
        "{}",
        app.handoff.text
    );
}

#[test]
fn with_a_codex_model_a_draft_that_comes_back_empty_or_unreadable_says_codex() {
    let w = World::new("codex-asks-draft");
    shop_conversation(&w, &w.base.join("shop"));
    let claude = smart_on(&w, false, false);
    let mut app = w.codex_app();
    let codex = on_codex(&w, &mut app, &claude);
    codex.reply(1, "not json");
    codex.reply(2, r#"{"tasks":[]}"#);
    select(&mut app, SHOP);
    for want in [
        "Codex's answer couldn't be understood, so no tasks were drafted. Nothing was written.",
        "Codex found no work left for later in this conversation, so there are no tasks to make.",
    ] {
        key(&mut app, KeyCode::Char('t'));
        app.refresh_until_for_test(20, "the answer", |a| a.drafting_from().is_none());
        assert_eq!(app.active_notice(), Some(want));
    }
    assert_eq!(claude.calls(), 0);
}

#[test]
fn with_a_codex_model_describe_it_says_codex_and_with_both_installed_the_full_text_search_names_both() {
    let w = world("codex-asks-find");
    let claude = smart_on(&w, false, true);
    let mut app = w.codex_app();
    let codex = on_codex(&w, &mut app, &claude);
    codex.reply(1, "[]");
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    key(&mut app, KeyCode::Char('/'));
    app.set_search_kind(1);
    assert!(screen(&mut term, &app).contains("Type words you asked Claude or Codex about, in any order."));
    app.set_search_kind(crate::app::FIND_KIND);
    let shown = screen(&mut term, &app);
    assert!(shown.contains("Describe it in your own words; Codex picks the likely ones."), "{shown}");
    for c in "the colour in the notes".chars() {
        key(&mut app, KeyCode::Char(c));
    }
    let shown = screen(&mut term, &app);
    assert!(shown.contains("Press enter to ask Codex.") && shown.contains("ask Codex"), "{shown}");
    key(&mut app, KeyCode::Enter);
    app.refresh_until_for_test(10, "the answer", |a| *a.find_state() == FindState::Done);
    assert!(screen(&mut term, &app).contains("Codex found no likely match."));
    assert_eq!((claude.calls(), codex.calls()), (0, 1));
}

#[test]
fn the_settings_name_the_program_each_setting_is_about() {
    let w = world("codex-asks-settings");
    let claude = smart_on(&w, false, false);
    let mut app = w.codex_app();
    let item = |name: &str| crate::settings::TABS.iter().flat_map(|t| t.items.iter()).find(|i| i.name == name).unwrap();
    let prompt_item = |action: &'static str| {
        crate::settings::TABS.iter().flat_map(|t| t.items.iter()).find(|i| i.kind == crate::settings::Kind::Instructions(action)).unwrap()
    };
    // Claude's models, both programs installed: as written, but where the sessions can be either program's.
    assert_eq!(app.setting_explain(prompt_item(crate::instructions::NAMES)), prompt_item(crate::instructions::NAMES).explain);
    assert_eq!(app.setting_name(item("Read Claude's last reply up to")), "Read Claude's or Codex's last reply up to");
    assert!(app.setting_explain(item("Read Claude's last reply up to")).starts_with("How much of Claude's or Codex's last answer"));
    // The smart features on Codex: their prompts and numbers say Codex; the notes, still on Sonnet, say Claude.
    on_codex(&w, &mut app, &claude);
    assert_eq!(
        app.setting_explain(prompt_item(crate::instructions::NAMES)),
        "What platoon asks Codex for when it names and summarizes your sessions."
    );
    assert_eq!(
        app.setting_explain(prompt_item(crate::instructions::HANDOFF)),
        "What platoon asks Codex for when it writes a hand-off to a new session."
    );
    assert_eq!(app.setting_explain(prompt_item(crate::instructions::NOTES)), prompt_item(crate::instructions::NOTES).explain);
    assert_eq!(
        app.setting_explain(item("Sessions per call")),
        "How many sessions one request to Codex covers. Larger batches use less of your plan overall."
    );
    let mut term = Terminal::new(TestBackend::new(110, 44)).unwrap();
    key(&mut app, KeyCode::Char('s'));
    let (tab, at) = settings::find_item("System prompt for naming").unwrap();
    while app.settings_tab != tab {
        key(&mut app, KeyCode::Tab);
    }
    key(&mut app, KeyCode::Home);
    for _ in 0..at {
        key(&mut app, KeyCode::Down);
    }
    let shown = screen(&mut term, &app);
    assert!(shown.contains("What platoon asks Codex for when it names"), "drawn as said:\n{shown}");
}
