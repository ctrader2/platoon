//! Codex's conversations in the list, at the level of the app: the sample sessions (`src/testdata/codex`) in a
//! world's Codex folder, real keys in, what the rows, the search and the notices say out. Every folder is a temp dir
//! (`World`).

use crate::agents::Agent;
use crate::app::{App, JumpPlan, Mode};
use crate::model::Status;
use crate::testkit::World;
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

#[test]
fn the_smart_features_hand_off_and_task_drafts_leave_codex_rows_alone_and_say_so() {
    let w = world("codex-smart");
    let mut app = w.codex_app();
    select(&mut app, FINISHED);
    key(&mut app, KeyCode::Char('h'));
    assert!(app.active_notice().is_some_and(|n| n.contains("Claude Code conversations only")), "{:?}", app.active_notice());
    assert!(matches!(app.mode, Mode::List));
    key(&mut app, KeyCode::Char('t'));
    assert!(app.active_notice().is_some_and(|n| n.contains("Claude Code conversations only")), "{:?}", app.active_notice());
    assert!(matches!(app.mode, Mode::List));
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
