//! The smart names and summaries at the level of the app: real transcripts on disk, the app's own refresh loop, a fake
//! `claude` that records every call and answers from files, and what the list, the files and the screen show out. Every
//! directory is a temp dir (`World`, `AiFake`); the real `claude` is never reached (the app refuses it in tests).

use crate::agents::Agent;
use crate::app::{App, Mode};
use crate::ledger::Ledger;
use crate::model::NameSource;
use crate::settings::{self, Settings};
use crate::testkit::{AiFake, World, iso, now_ms};
use crate::ui;
use crate::usage;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
};
use serde_json::{Value, json};
use std::fs;

// ---- fixtures ------------------------------------------------------------------------------------------------

/// A conversation: the user typed `prompts` (each answered by a short reply), then Claude said `last_reply`, and after
/// that come things that must never be sent: tool output, a tool call, thinking and a subagent's words. The last
/// entry was written `secs_ago` seconds ago.
fn conv(w: &World, id: &str, cwd: &str, prompts: &[&str], last_reply: &str, secs_ago: u64) {
    let end = now_ms() - secs_ago * 1000;
    let mut lines = vec![json!({"type":"mode","mode":"normal"}).to_string()];
    let mut t = end - (prompts.len() as u64 * 3 + 6) * 1000;
    for p in prompts {
        lines
            .push(json!({"type":"user","cwd":cwd,"gitBranch":"main","timestamp":iso(t),"message":{"role":"user","content":p}}).to_string());
        t += 1000;
        lines.push(json!({"type":"assistant","cwd":cwd,"timestamp":iso(t),"message":{"model":"claude-sonnet-5","content":[{"type":"text","text":format!("reply about {p}")}],"usage":{"input_tokens":1000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}).to_string());
        t += 1000;
    }
    let tail = [
        json!({"type":"assistant","cwd":cwd,"timestamp":iso(end - 4000),"message":{"model":"claude-sonnet-5","content":[{"type":"text","text":last_reply}],"usage":{"input_tokens":1000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}),
        json!({"type":"assistant","cwd":cwd,"timestamp":iso(end - 3000),"message":{"model":"claude-sonnet-5","content":[{"type":"tool_use","name":"Bash","input":{"command":"echo TOOLCALL-SECRET"}}]}}),
        json!({"type":"user","cwd":cwd,"timestamp":iso(end - 2000),"message":{"role":"user","content":[{"tool_use_id":"x","type":"tool_result","content":"TOOL-OUTPUT-SECRET"}]}}),
        json!({"type":"assistant","cwd":cwd,"timestamp":iso(end - 1500),"message":{"model":"claude-sonnet-5","content":[{"type":"thinking","thinking":"THINKING-SECRET"}]}}),
        json!({"type":"assistant","isSidechain":true,"cwd":cwd,"timestamp":iso(end - 1000),"message":{"model":"claude-sonnet-5","content":[{"type":"text","text":"SIDECHAIN-SECRET"}]}}),
        json!({"type":"assistant","cwd":cwd,"timestamp":iso(end),"message":{"model":"<synthetic>","content":[{"type":"text","text":"SYNTHETIC-SECRET"}]}}),
    ];
    lines.extend(tail.iter().map(Value::to_string));
    w.transcript_lines(id, &lines);
}

/// A simple one-prompt conversation.
fn simple(w: &World, id: &str, prompt: &str, secs_ago: u64) {
    conv(w, id, &format!("/work/{id}"), &[prompt], "Done.", secs_ago);
}

/// The settings with the smart features on (master, names and find), then `change` applied.
fn smart(w: &World, change: impl FnOnce(&mut Settings)) {
    let mut s = settings::load(&w.config).0;
    s.tokens_enabled = true;
    s.set_feature("names", true);
    s.set_feature("find", true);
    change(&mut s);
    settings::save(&w.config, &s).unwrap();
}

fn app_with(w: &World, fake: &AiFake) -> App {
    let mut app = w.app();
    app.set_ai_program_for_test(&fake.script);
    app
}

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn ctrl(app: &mut App, c: char) {
    app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
}

fn type_str(app: &mut App, text: &str) {
    for c in text.chars() {
        key(app, KeyCode::Char(c));
    }
}

fn screen(term: &mut Terminal<TestBackend>, app: &App) -> String {
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

fn row<'a>(app: &'a App, id: &str) -> &'a crate::model::Session {
    app.sessions.iter().find(|s| s.id == id).unwrap_or_else(|| panic!("no row {id}"))
}

fn summaries_file(w: &World) -> Value {
    serde_json::from_str(&fs::read_to_string(w.config.join("summaries.json")).unwrap()).unwrap()
}

// ---- who is named, in what order, how ---------------------------------------------------------------------

#[test]
fn nothing_is_sent_while_the_master_switch_or_the_names_switch_is_off() {
    let w = World::new("sm-off");
    simple(&w, "aaa", "please fix the login redirect", 300);
    let fake = AiFake::new("sm-off");
    let mut app = app_with(&w, &fake);
    app.settle_ai_for_test(5);
    assert_eq!(fake.calls(), 0, "off by default");
    // The master on but the feature off; then the feature on but the master off.
    smart(&w, |s| s.set_feature("names", false));
    app.settle_ai_for_test(5);
    smart(&w, |s| {
        s.tokens_enabled = false;
    });
    app.settle_ai_for_test(5);
    assert_eq!((fake.calls(), app.smart_progress()), (0, None));
    assert!(!w.config.join("summaries.json").exists());
    assert!(!w.cache.join("ai-work").exists(), "not even the work folder");
}

#[test]
fn first_time_names_go_to_active_conversations_newest_first_in_batches_of_the_setting() {
    let w = World::new("sm-first");
    for (i, secs) in [(1, 100_000u64), (2, 50_000), (3, 4000), (4, 3000), (5, 2000), (6, 1000), (7, 500)] {
        simple(&w, &format!("c{i}"), &format!("please look into topic number {i} for me"), secs);
    }
    smart(&w, |s| s.ai_batch_size = 3);
    let fake = AiFake::new("sm-first");
    let mut app = app_with(&w, &fake);
    app.settle_ai_for_test(20);
    assert_eq!(fake.calls(), 3, "seven conversations, three to a call: 3 + 3 + 1");
    let heads = |k: usize| fake.stdin(k).lines().filter(|l| l.starts_with('[')).count();
    assert_eq!((heads(1), heads(2), heads(3)), (3, 3, 1));
    // Newest first: c7 is the newest, c1 the oldest.
    let first = fake.stdin(1);
    let at = |n: usize| first.find(&format!("topic number {n} ")).unwrap();
    assert!(at(7) < at(6) && at(6) < at(5), "newest first:\n{first}");
    assert!(fake.stdin(3).contains("topic number 1 "), "the oldest went last");
    // Every conversation has its name and summary, on its row and in the file.
    for i in 1..=7 {
        let r = row(&app, &format!("c{i}"));
        assert!(r.name.starts_with("Name ") && r.summary.starts_with("Summary "), "c{i}: {} / {}", r.name, r.summary);
        assert_eq!(r.name_source, NameSource::Generated);
    }
    let file = summaries_file(&w);
    assert_eq!(file["entries"].as_object().unwrap().len(), 7);
    assert_eq!(file["entries"]["c7"]["covers"], 1);
    assert_eq!(file["entries"]["c7"]["model"], "haiku");
    assert!(app.smart_progress().is_none(), "the indicator is gone when the work is done");
    // A second start finds nothing to do.
    let mut again = app_with(&w, &fake);
    again.settle_ai_for_test(5);
    assert_eq!(fake.calls(), 3, "nothing was named twice");
    assert_eq!(row(&again, "c3").name, row(&app, "c3").name, "and the names are kept across a restart");
}

#[test]
fn a_call_is_small_and_made_the_agreed_way() {
    let w = World::new("sm-argv");
    simple(&w, "aaa", "please fix the login redirect", 300);
    smart(&w, |s| s.ai_model = Some("sonnet".into()));
    let fake = AiFake::new("sm-argv");
    let mut app = app_with(&w, &fake);
    app.settle_ai_for_test(10);
    let argv = fake.argv(1).join(" ");
    for want in [
        "-p",
        "--model sonnet",
        "--tools ",
        "--no-session-persistence",
        "--strict-mcp-config",
        "--output-format json",
        "--effort low",
        "--system-prompt ",
    ] {
        assert!(argv.contains(want), "{want}: {argv}");
    }
    assert_eq!(fake.cwd(1), w.cache.join("ai-work").display().to_string());
    assert!(fake.env(1).contains("PLATOON_AI=1"));
    assert!(fake.argv(1).iter().any(|a| a.contains("DATA") && a.contains("JSON only")), "the short system prompt");
}

#[test]
fn with_an_openai_model_claude_code_conversations_are_named_by_codex() {
    let w = World::new("sm-codex");
    simple(&w, "aaa", "please fix the login redirect", 300);
    smart(&w, |s| s.ai_model = Some("gpt-6-luna".into()));
    let fake = AiFake::new("sm-codex");
    let codex = AiFake::codex("sm-codex");
    let mut app = app_with(&w, &fake);
    let root = crate::agents::tests::codex_with_models("sm-codex").root;
    app.set_providers_for_test(crate::agents::Providers {
        claude: Some(fake.script.clone()),
        codex: Some(codex.as_codex(&root)),
        off: Vec::new(),
    });
    app.settle_ai_for_test(10);
    assert_eq!((fake.calls(), codex.calls()), (0, 1), "the model decides the program: {:?}", app.smart_idle());
    assert_eq!(&codex.argv(1)[..8], ["exec", "--json", "--ephemeral", "--ignore-user-config", "-s", "read-only", "-m", "gpt-6-luna"]);
    assert!(codex.stdin(1).contains("please fix the login redirect"));
    assert_eq!(row(&app, "aaa").name, "Codex name 1.1");
}

#[test]
fn what_is_sent_is_the_first_message_the_last_few_cut_short_one_reply_line_and_nothing_else() {
    let w = World::new("sm-digest");
    let long = format!("{} end-marker", "word ".repeat(100));
    let prompts: Vec<String> =
        (1..=12).map(|i| if i == 12 { long.clone() } else { format!("message number {i} about the payment module") }).collect();
    let refs: Vec<&str> = prompts.iter().map(String::as_str).collect();
    let cwd = "/work/very-private-folder-name";
    conv(&w, "abcdef12-3456-7890-abcd-ef1234567890", cwd, &refs, &format!("I updated the module. {}", "x".repeat(400)), 300);
    smart(&w, |s| {
        s.ai_prompts_per_session = 3;
        s.ai_prompt_chars = 100;
    });
    let fake = AiFake::new("sm-digest");
    let mut app = app_with(&w, &fake);
    app.settle_ai_for_test(10);
    let sent = fake.stdin(1);
    assert!(sent.starts_with("Name and summarize these conversations.\n\n[1]\n"), "{sent}");
    assert!(sent.contains("First message: message number 1 about the payment module\n"), "{sent}");
    let recent: Vec<&str> = sent.lines().filter(|l| l.starts_with("- ")).collect();
    assert_eq!(recent.len(), 3, "the last three: {sent}");
    assert!(recent[0].contains("message number 10 ") && recent[1].contains("message number 11 "), "{recent:?}");
    assert!(
        recent[2].chars().count() <= 2 + 100 && recent[2].contains("word word") && !recent[2].contains("end-marker"),
        "each message is cut to the limit: {}",
        recent[2]
    );
    assert!(!sent.contains("message number 2 ") && !sent.contains("message number 9 "), "only the first and the last few: {sent}");
    // One line of Claude's last words: the text before the tool output, cut to 200 characters, from the right entry.
    let reply_line = sent.lines().find(|l| l.starts_with("Claude's last reply: ")).unwrap();
    assert!(reply_line.starts_with("Claude's last reply: I updated the module. xxx"), "{reply_line}");
    assert_eq!(reply_line.chars().count(), "Claude's last reply: ".len() + 200);
    // Nothing else: no id, no folder, no tool output, no tool call, no thinking, no subagent, no synthetic notice.
    for never in [
        "abcdef12",
        cwd,
        "very-private",
        "TOOL-OUTPUT-SECRET",
        "TOOLCALL-SECRET",
        "THINKING-SECRET",
        "SIDECHAIN-SECRET",
        "SYNTHETIC-SECRET",
        "gitBranch",
        "main",
    ] {
        assert!(!sent.contains(never), "{never} must not be sent:\n{sent}");
    }
    assert!(!sent.contains("Current name:") && !sent.contains("Previous summary:"), "nothing to remember yet");
}

#[test]
fn a_refresh_carries_the_current_name_and_the_previous_summary_and_same_keeps_the_name() {
    let w = World::new("sm-same");
    let prompts: Vec<String> = (1..=2).map(|i| format!("early message number {i} about billing")).collect();
    let refs: Vec<&str> = prompts.iter().map(String::as_str).collect();
    conv(&w, "aaa", "/work/a", &refs, "Done.", 600);
    smart(&w, |s| s.ai_quiet_seconds = 15);
    let fake = AiFake::new("sm-same");
    fake.reply(1, r#"[{"n":1,"name":"Billing work","summary":"Working on the billing export."}]"#);
    fake.reply(2, r#"[{"n":1,"same":true,"summary":"Now testing the billing export."}]"#);
    let mut app = app_with(&w, &fake);
    app.settle_ai_for_test(10);
    assert_eq!((row(&app, "aaa").name.as_str(), row(&app, "aaa").summary.as_str()), ("Billing work", "Working on the billing export."));
    // Five new substantive messages later.
    let more: Vec<String> = (3..=7).map(|i| format!("later message number {i} about billing tests")).collect();
    let all: Vec<&str> = refs.iter().copied().chain(more.iter().map(String::as_str)).collect();
    conv(&w, "aaa", "/work/a", &all, "Tests pass.", 60);
    app.advance_clock_for_test(30);
    app.settle_ai_for_test(10);
    assert_eq!(fake.calls(), 2);
    let second = fake.stdin(2);
    assert!(
        second.contains("Current name: Billing work\n") && second.contains("Previous summary: Working on the billing export.\n"),
        "{second}"
    );
    assert!(second.contains("- later message number 7 about billing tests"), "{second}");
    let r = row(&app, "aaa");
    assert_eq!(
        (r.name.as_str(), r.summary.as_str()),
        ("Billing work", "Now testing the billing export."),
        "\"same\" keeps the old name and takes the new summary"
    );
    assert_eq!(summaries_file(&w)["entries"]["aaa"]["covers"], 7);
}

#[test]
fn a_conversation_with_nothing_but_filler_or_no_transcript_is_never_sent() {
    let w = World::new("sm-filler");
    conv(&w, "filler", "/work/f", &["yes", "continue", "ok", "thanks"], "Done.", 300);
    simple(&w, "real", "please add a csv export to the report", 400);
    smart(&w, |_| {});
    // A running conversation that has not saved anything yet has no transcript: nothing to read.
    w.register(std::process::id(), "running-only", "/work/r", 1, "idle");
    let fake = AiFake::new("sm-filler");
    let mut app = app_with(&w, &fake);
    app.settle_ai_for_test(10);
    assert_eq!(fake.calls(), 1);
    let sent = fake.stdin(1);
    assert_eq!(sent.lines().filter(|l| l.starts_with('[')).count(), 1, "{sent}");
    assert!(sent.contains("csv export") && !sent.contains("continue"), "{sent}");
    assert_eq!(row(&app, "filler").name_source, NameSource::Folder);
    assert_eq!(row(&app, "running-only").name_source, NameSource::Folder);
}

// ---- what comes back --------------------------------------------------------------------------------------

#[test]
fn a_bad_answer_keeps_the_good_entries_strips_control_text_and_caps_lengths() {
    let w = World::new("sm-reply");
    for i in 1..=4 {
        simple(&w, &format!("c{i}"), &format!("please look into topic number {i} for me"), 100 * i as u64);
    }
    smart(&w, |_| {});
    let fake = AiFake::new("sm-reply");
    let long = "N".repeat(200);
    let reply = json!([
        {"n": 1, "name": "Clean \u{1b}[31mred\u{1b}[0m name\u{7}", "summary": "Line\nbreak \u{1b}]0;title\u{7}and \u{1b}[2Jmore"},
        {"n": 2, "name": long, "summary": "S".repeat(600)},
        {"n": 9, "name": "Unknown number", "summary": "x"},
        {"n": 3, "name": "", "summary": "empty name"},
        {"n": "4", "name": "Text number", "summary": "x"},
        "junk"
    ])
    .to_string();
    fake.reply(1, &reply);
    fake.reply_default("I am sorry, I cannot do that.");
    let mut app = app_with(&w, &fake);
    app.settle_ai_for_test(10);
    // c1 (the newest, number 1 in the call) and c2 are good; c3 and c4 got nothing usable.
    let r1 = row(&app, "c1");
    assert_eq!(
        (r1.name.as_str(), r1.summary.as_str()),
        ("Clean red name", "Line break and more"),
        "control characters and escape sequences are gone"
    );
    let r2 = row(&app, "c2");
    assert_eq!((r2.name.chars().count(), r2.summary.chars().count()), (60, 300), "name 60, summary 300");
    assert_eq!(row(&app, "c3").name_source, NameSource::Folder);
    assert_eq!(row(&app, "c4").name_source, NameSource::Folder);
    let text = fs::read_to_string(w.config.join("summaries.json")).unwrap();
    assert!(!text.contains('\u{1b}') && !text.contains("\\u001b") && !text.contains("\\u0007"), "nothing raw in the file: {text}");
    // The two it skipped are tried again, at most twice more (together), then left: three calls in all.
    assert_eq!(fake.calls(), 3, "the first call, and two more for the two it skipped: {}", fake.calls());
    assert_eq!((app.misses_for_test("c3"), app.misses_for_test("c4")), (3, 3));
}

#[test]
fn a_skipped_conversation_is_tried_again_only_twice_until_it_gets_a_new_message() {
    let w = World::new("sm-retry");
    simple(&w, "aaa", "please fix the login redirect for me", 300);
    smart(&w, |_| {});
    let fake = AiFake::new("sm-retry");
    fake.reply_default("[]");
    let mut app = app_with(&w, &fake);
    app.settle_ai_for_test(10);
    assert_eq!(fake.calls(), 3, "the first try and two more");
    app.advance_clock_for_test(3600);
    app.settle_ai_for_test(5);
    assert_eq!(fake.calls(), 3, "then left alone");
    // A new message starts it over.
    let two = ["please fix the login redirect for me", "and also fix the logout redirect too"];
    conv(&w, "aaa", "/work/aaa", &two, "Done.", 300);
    fake.reply_default(r#"[{"n":1,"name":"Redirects","summary":"Fixing redirects."}]"#);
    app.settle_ai_for_test(10);
    assert_eq!(fake.calls(), 4);
    assert_eq!(row(&app, "aaa").name, "Redirects");
}

#[test]
fn the_reply_is_used_even_inside_a_code_fence_or_with_chatter_around_it() {
    let w = World::new("sm-fence");
    simple(&w, "aaa", "please fix the login redirect for me", 300);
    smart(&w, |_| {});
    let fake = AiFake::new("sm-fence");
    fake.reply(1, "Sure, here you go:\n```json\n[{\"n\":1,\"name\":\"Login fix\",\"summary\":\"Fixing login.\"}]\n```\nHope that helps!");
    let mut app = app_with(&w, &fake);
    app.settle_ai_for_test(10);
    assert_eq!(row(&app, "aaa").name, "Login fix");
}

// ---- the live trigger, with a clock the test moves ------------------------------------------------------------

#[test]
fn a_summary_is_refreshed_after_five_new_substantive_messages_and_a_quiet_minute_not_before() {
    let w = World::new("sm-live");
    let base: Vec<String> = (1..=2).map(|i| format!("original message number {i} about the parser")).collect();
    let mut prompts: Vec<&str> = base.iter().map(String::as_str).collect();
    conv(&w, "aaa", "/work/a", &prompts, "Done.", 600);
    smart(&w, |_| {});
    let fake = AiFake::new("sm-live");
    let mut app = app_with(&w, &fake);
    app.settle_ai_for_test(10);
    assert_eq!(fake.calls(), 1);
    // Four new substantive messages and lots of filler, all written ten minutes ago: quiet, but not enough of them.
    let extra: Vec<String> = (1..=4).map(|i| format!("new substantive message {i} about lexer rules")).collect();
    prompts.extend(extra.iter().map(String::as_str));
    prompts.extend(["yes", "continue", "ok", "go ahead", "thanks", "yes please continue"]);
    conv(&w, "aaa", "/work/a", &prompts, "Done.", 600);
    app.refresh_for_test();
    app.settle_ai_for_test(5);
    assert_eq!(fake.calls(), 1, "four substantive messages are not five, however many trivial ones come with them");
    // The fifth, written just now: five new ones, but the conversation is not quiet yet.
    let fifth = "new substantive message 5 about lexer rules".to_string();
    prompts.push(&fifth);
    conv(&w, "aaa", "/work/a", &prompts, "Done.", 0);
    app.refresh_for_test();
    app.settle_ai_for_test(5);
    assert_eq!(fake.calls(), 1, "not quiet yet");
    app.advance_clock_for_test(50);
    app.settle_ai_for_test(5);
    assert_eq!(fake.calls(), 1, "50 seconds is less than a minute");
    app.advance_clock_for_test(20);
    app.settle_ai_for_test(5);
    assert_eq!(fake.calls(), 2, "70 seconds of quiet: due");
    assert_eq!(summaries_file(&w)["entries"]["aaa"]["covers"], 7, "it covers the five new ones now");
}

#[test]
fn the_quiet_time_is_measured_from_the_last_message_and_is_a_setting() {
    let w = World::new("sm-quiet");
    let base = ["original message number 1 about the parser", "original message number 2 about the parser"];
    conv(&w, "aaa", "/work/a", &base, "Done.", 600);
    smart(&w, |s| {
        s.ai_refresh_messages = 1;
        s.ai_quiet_seconds = 300;
    });
    let fake = AiFake::new("sm-quiet");
    let mut app = app_with(&w, &fake);
    app.settle_ai_for_test(10);
    assert_eq!(fake.calls(), 1);
    let more = [base[0], base[1], "a new substantive message about the lexer"];
    conv(&w, "aaa", "/work/a", &more, "Done.", 30); // the last entry is 30 seconds old
    app.refresh_for_test();
    app.settle_ai_for_test(3);
    assert_eq!(fake.calls(), 1, "30 s of quiet is less than 300");
    app.advance_clock_for_test(200);
    app.settle_ai_for_test(3);
    assert_eq!(fake.calls(), 1, "230 s is still less");
    app.advance_clock_for_test(100);
    app.settle_ai_for_test(5);
    assert_eq!(fake.calls(), 2, "330 s of quiet: due");
}

// ---- switches, limits, pauses ---------------------------------------------------------------------------------

#[test]
fn turning_the_feature_off_cancels_the_call_discards_its_answer_and_keeps_what_was_saved() {
    let w = World::new("sm-cancel");
    simple(&w, "old", "please review the old parser module", 5000);
    smart(&w, |s| s.ai_batch_size = 1);
    let fake = AiFake::new("sm-cancel");
    let mut app = app_with(&w, &fake);
    app.settle_ai_for_test(10);
    assert_eq!(fake.calls(), 1);
    let saved = fs::read_to_string(w.config.join("summaries.json")).unwrap();
    // A new conversation; its call hangs.
    simple(&w, "new", "please add a feature for exporting reports", 100);
    fake.hang(true);
    app.refresh_for_test();
    app.refresh_until_for_test(10, "the call to start", |_| fake.pids().len() == 2);
    let pids = fake.pids();
    assert!(pids.iter().all(|&p| crate::testkit::alive(p)) && app.smart_busy());
    // Off, by the settings file: the next refresh cancels it.
    smart(&w, |s| s.set_feature("names", false));
    app.refresh_for_test();
    app.refresh_until_for_test(10, "the call to end", |a| !a.smart_busy());
    for _ in 0..200 {
        if pids.iter().all(|&p| !crate::testkit::alive(p)) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(pids.iter().all(|&p| !crate::testkit::alive(p)), "the fake and what it started are dead");
    assert_eq!(fs::read_to_string(w.config.join("summaries.json")).unwrap(), saved, "everything saved is kept and nothing was added");
    assert_eq!(row(&app, "new").name_source, NameSource::Folder);
    assert_eq!(row(&app, "old").name_source, NameSource::Generated, "what was named stays on its row while the feature is off");
    assert!(app.smart_progress().is_none());
    // On again: it is done, and this time the answer is kept.
    fake.hang(false);
    smart(&w, |_| {});
    app.settle_ai_for_test(10);
    assert_eq!(row(&app, "new").name_source, NameSource::Generated);
}

#[test]
fn the_master_switch_off_also_cancels_and_a_late_answer_is_thrown_away() {
    let w = World::new("sm-cancel-master");
    simple(&w, "aaa", "please review the old parser module", 500);
    smart(&w, |_| {});
    let fake = AiFake::new("sm-cancel-master");
    fake.hang(true);
    let mut app = app_with(&w, &fake);
    app.refresh_until_for_test(10, "the call to start", |_| fake.pids().len() == 2);
    smart(&w, |s| s.tokens_enabled = false);
    app.refresh_until_for_test(10, "the call to end", |a| !a.smart_busy());
    assert!(!w.config.join("summaries.json").exists());
}

#[test]
fn at_the_daily_limit_it_stops_and_carries_on_the_next_day() {
    let w = World::new("sm-limit");
    for i in 1..=3 {
        simple(&w, &format!("c{i}"), &format!("please look into topic number {i} for me"), 100 * i as u64);
    }
    smart(&w, |s| {
        s.ai_batch_size = 1;
        s.ai_daily_tokens = 20_000;
    });
    let fake = AiFake::new("sm-limit");
    fake.usage(25_000, 100);
    let mut app = app_with(&w, &fake);
    app.settle_ai_for_test(10);
    assert_eq!(fake.calls(), 1, "the first call used more than the whole day's limit: nothing more today");
    assert!(app.smart_idle().is_some_and(|m| m.contains("token limit") && m.contains("tomorrow")), "{:?}", app.smart_idle());
    assert_eq!(app.smart_progress(), Some((1, 3, true)), "one of three done, and the indicator says it is waiting");
    // Hours later, still the same day: still nothing.
    app.advance_clock_for_test(3600);
    app.settle_ai_for_test(3);
    let day = crate::ledger::local_day(usage::now());
    if crate::ledger::local_day(usage::now() + 3600) == day {
        assert_eq!(fake.calls(), 1);
    }
    // Past midnight: the count starts over and the rest is done.
    app.advance_clock_for_test(86_400);
    fake.usage(400, 50);
    app.settle_ai_for_test(10);
    assert_eq!(fake.calls(), 3, "the remaining two went out on the next day");
    assert!(app.sessions.iter().all(|s| s.name_source == NameSource::Generated));
}

#[test]
fn a_failed_call_backs_off_says_so_once_and_the_work_resumes_when_the_wait_is_over() {
    let w = World::new("sm-backoff");
    simple(&w, "aaa", "please look into the flaky test for me", 300);
    smart(&w, |_| {});
    let fake = AiFake::new("sm-backoff");
    fake.fail(Some("API Error: 429 rate limit reached"));
    let mut app = app_with(&w, &fake);
    app.settle_ai_for_test(10);
    assert_eq!(fake.calls(), 1);
    assert_eq!(app.active_notice(), Some("Smart features paused: your Claude plan limit was reached"));
    // Many refreshes: no new call, no new notice.
    app.notice = None;
    for _ in 0..5 {
        app.refresh_for_test();
    }
    assert_eq!((fake.calls(), app.active_notice()), (1, None), "one line per run of failures, no busy loop");
    assert!(app.smart_idle().is_some(), "waiting");
    // The plan limit's wait is 15 minutes: after it the call is made again.
    fake.fail(None);
    app.advance_clock_for_test(16 * 60);
    app.settle_ai_for_test(10);
    assert_eq!(fake.calls(), 2);
    assert_eq!(row(&app, "aaa").name_source, NameSource::Generated);
}

#[test]
fn the_plan_pause_holds_background_naming_back() {
    let w = World::new("sm-plan");
    simple(&w, "aaa", "please look into the flaky test for me", 300);
    smart(&w, |_| {});
    let ts = usage::now();
    fs::create_dir_all(w.cache.join("status")).unwrap();
    fs::write(
        w.cache.join("status/aaaa.json"),
        json!({"ts": ts, "context_pct": null, "five_hour": {"pct": 95, "resets_at": ts + 3600}, "seven_day": null}).to_string(),
    )
    .unwrap();
    let fake = AiFake::new("sm-plan");
    let mut app = app_with(&w, &fake);
    app.settle_ai_for_test(5);
    assert_eq!(fake.calls(), 0);
    assert!(app.smart_idle().is_some_and(|m| m.contains("95%") && m.contains("Smart features wait")), "{:?}", app.smart_idle());
    // The pause is a setting: never.
    smart(&w, |s| s.ai_pause_percent = 0);
    app.advance_clock_for_test(31);
    app.settle_ai_for_test(10);
    assert_eq!(fake.calls(), 1);
}

// ---- archived conversations -------------------------------------------------------------------------------------

fn archive(w: &World, ids: &[&str]) {
    let mut a = crate::archive::Archive::open(&w.config);
    for id in ids {
        a.archive(id, usage::now()).unwrap();
    }
}

#[test]
fn archived_conversations_are_left_alone_by_default() {
    let w = World::new("sm-arch-off");
    simple(&w, "act", "please look into the active topic", 300);
    simple(&w, "arc", "please look into the archived topic", 900);
    archive(&w, &["arc"]);
    smart(&w, |_| {});
    let fake = AiFake::new("sm-arch-off");
    let mut app = app_with(&w, &fake);
    app.settle_ai_for_test(10);
    assert_eq!(fake.calls(), 1);
    let sent = fake.stdin(1);
    assert!(sent.contains("active topic") && !sent.contains("archived topic"), "{sent}");
    assert_eq!(row(&app, "arc").name_source, NameSource::Folder);
}

#[test]
fn archived_conversations_use_at_most_their_share_of_what_is_left_of_the_day_and_never_share_a_call_with_active_ones() {
    let w = World::new("sm-arch-share");
    simple(&w, "act", "please look into the active topic", 300);
    for i in 1..=4 {
        simple(&w, &format!("arc{i}"), &format!("please look into archived topic {i} today"), 1000 + i as u64 * 100);
    }
    archive(&w, &["arc1", "arc2", "arc3", "arc4"]);
    smart(&w, |s| {
        s.ai_archived = true;
        s.ai_batch_size = 1;
        s.ai_daily_tokens = 20_000;
        s.ai_archived_share = 10;
    });
    let fake = AiFake::new("sm-arch-share");
    fake.usage(550, 50); // every call uses 600 tokens, close to what one conversation is estimated to cost
    let mut app = app_with(&w, &fake);
    app.settle_ai_for_test(15);
    // The active one first, in a call of its own; then archived ones, one to a call.
    assert!(fake.stdin(1).contains("active topic"));
    assert!(!fake.stdin(2).contains("active topic") && fake.stdin(2).contains("archived topic"), "{}", fake.stdin(2));
    // 20,000 - 600 = 19,400 left after the active one; a tenth of it is 1,940. Each archived call is estimated at a
    // little over 600, so three fit (three times 600 is 1,800 used) and a fourth would go past the share.
    let ledger = Ledger::open(&w.cache);
    let (used, archived) = (ledger.today(usage::now(), Agent::Claude).tokens(), ledger.archived_today(usage::now(), Agent::Claude));
    assert_eq!(fake.calls(), 1 + 3, "the fourth archived conversation waits");
    assert_eq!((archived, used), (3 * 600, 4 * 600));
    assert!(archived <= 20_000 / 10, "within a tenth of the day's limit");
    assert_eq!(app.sessions.iter().filter(|s| s.archived && s.name_source == NameSource::Generated).count(), 3);
    // The progress counts only what is allowed to run.
    assert!(app.smart_progress().is_none(), "the archived one that has to wait is not counted");
}

#[test]
fn a_restored_conversation_is_named_at_once_ahead_of_the_rest() {
    let w = World::new("sm-restore");
    for i in 1..=3 {
        simple(&w, &format!("new{i}"), &format!("please look into fresh topic {i} today"), 100 * i as u64);
    }
    simple(&w, "old", "please look into the restored topic today", 90_000);
    archive(&w, &["old"]);
    let fake = AiFake::new("sm-restore");
    let mut app = app_with(&w, &fake);
    app.refresh_for_test(); // sees the archive
    // Restore it on the Archived tab.
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Char('a'));
    assert!(!row(&app, "old").archived);
    smart(&w, |s| s.ai_batch_size = 1);
    app.refresh_for_test();
    app.settle_ai_for_test(15);
    assert_eq!(fake.calls(), 4);
    assert!(fake.stdin(1).contains("restored topic"), "the restored one is first although it is the oldest:\n{}", fake.stdin(1));
}

#[test]
fn opening_a_conversation_counts_as_activating_it() {
    let w = World::new("sm-open");
    for i in 1..=2 {
        simple(&w, &format!("new{i}"), &format!("please look into fresh topic {i} today"), 100 * i as u64);
    }
    simple(&w, "old", "please look into the resumed topic today", 90_000);
    let fake = AiFake::new("sm-open");
    let mut app = app_with(&w, &fake);
    app.opened_for_test("old");
    smart(&w, |s| s.ai_batch_size = 1);
    app.settle_ai_for_test(15);
    assert_eq!(fake.calls(), 3);
    assert!(fake.stdin(1).contains("resumed topic"), "the opened one is first although it is the oldest:\n{}", fake.stdin(1));
}

#[test]
fn a_change_made_by_a_second_window_shows_on_the_rows() {
    let w = World::new("sm-two");
    simple(&w, "aaa", "please look into the flaky test for me", 300);
    let fake = AiFake::new("sm-two");
    let mut app = app_with(&w, &fake);
    assert_eq!(row(&app, "aaa").name_source, NameSource::Folder);
    let mut other = crate::summaries::Summaries::open(&w.config);
    other
        .set_many(&[(
            "aaa".into(),
            crate::summaries::Entry {
                name: "From the other window".into(),
                summary: "Made elsewhere.".into(),
                covers: 1,
                model: "haiku".into(),
                at: 1,
            },
        )])
        .unwrap();
    app.refresh_for_test();
    assert_eq!((row(&app, "aaa").name.as_str(), row(&app, "aaa").summary.as_str()), ("From the other window", "Made elsewhere."));
}

// ---- which name a row shows ----------------------------------------------------------------------------------------

#[test]
fn a_row_shows_your_name_then_one_typed_inside_claude_then_the_generated_one_then_the_folder() {
    let w = World::new("sm-precedence");
    simple(&w, "aaa", "please look into the flaky test for me", 300);
    let mut sums = crate::summaries::Summaries::open(&w.config);
    sums.set_many(&[(
        "aaa".into(),
        crate::summaries::Entry { name: "Generated name".into(), summary: "A summary.".into(), covers: 1, model: "haiku".into(), at: 1 },
    )])
    .unwrap();
    let mut app = w.app();
    assert_eq!(
        (row(&app, "aaa").name.as_str(), row(&app, "aaa").name_source),
        ("Generated name", NameSource::Generated),
        "over the folder"
    );
    // A name typed with /rename inside Claude (a running session whose registry says the user chose it).
    let pid = std::process::id();
    let reg = json!({"pid": pid, "sessionId": "aaa", "cwd": "/work/aaa", "kind": "interactive", "startedAt": 1, "status": "idle", "name": "Typed in Claude", "nameSource": "user"});
    fs::write(w.root.join("sessions").join(format!("{pid}.json")), reg.to_string()).unwrap();
    app.refresh_for_test();
    assert_eq!(
        (row(&app, "aaa").name.as_str(), row(&app, "aaa").name_source),
        ("Typed in Claude", NameSource::Claude),
        "over the generated one"
    );
    // A generated name arriving later does not take it over.
    let mut other = crate::summaries::Summaries::open(&w.config);
    other
        .set_many(&[(
            "aaa".into(),
            crate::summaries::Entry {
                name: "Newer generated".into(),
                summary: "New summary.".into(),
                covers: 2,
                model: "haiku".into(),
                at: 2,
            },
        )])
        .unwrap();
    app.refresh_for_test();
    assert_eq!((row(&app, "aaa").name.as_str(), row(&app, "aaa").summary.as_str()), ("Typed in Claude", "New summary."));
    // The name you gave in platoon beats all of them.
    key(&mut app, KeyCode::Char('r'));
    type_str(&mut app, "");
    for _ in 0..40 {
        key(&mut app, KeyCode::Backspace);
    }
    type_str(&mut app, "My own name");
    key(&mut app, KeyCode::Enter);
    assert_eq!((row(&app, "aaa").name.as_str(), row(&app, "aaa").name_source), ("My own name", NameSource::Platoon));
    // Clearing it gives the next in line back.
    key(&mut app, KeyCode::Char('r'));
    for _ in 0..40 {
        key(&mut app, KeyCode::Backspace);
    }
    key(&mut app, KeyCode::Enter);
    assert_eq!(row(&app, "aaa").name_source, NameSource::Claude);
    let _ = fs::remove_file(w.root.join("sessions").join(format!("{pid}.json")));
}

#[test]
fn generated_names_and_summaries_are_found_by_the_quick_filter_and_by_full_text() {
    let w = World::new("sm-search");
    simple(&w, "aaa", "please look into the flaky test for me", 300);
    simple(&w, "bbb", "please look into the login redirect for me", 400);
    let mut sums = crate::summaries::Summaries::open(&w.config);
    sums.set_many(&[
        (
            "aaa".into(),
            crate::summaries::Entry {
                name: "Gremlin hunt".into(),
                summary: "Chasing intermittent failures in CI.".into(),
                covers: 1,
                model: "haiku".into(),
                at: 1,
            },
        ),
        (
            "bbb".into(),
            crate::summaries::Entry {
                name: "Auth flow".into(),
                summary: "Fixing redirects after sign in.".into(),
                covers: 1,
                model: "haiku".into(),
                at: 1,
            },
        ),
    ])
    .unwrap();
    let mut app = w.app();
    // Quick filter: by the name and by the summary.
    key(&mut app, KeyCode::Char('/'));
    type_str(&mut app, "gremlin");
    assert_eq!(app.visible().iter().map(|&i| app.sessions[i].id.as_str()).collect::<Vec<_>>(), ["aaa"]);
    for _ in 0..7 {
        key(&mut app, KeyCode::Backspace);
    }
    type_str(&mut app, "intermittent");
    assert_eq!(app.visible().len(), 1, "the summary is searched too");
    // Full-text: by the name (no prompt has the word) and by the summary.
    key(&mut app, KeyCode::Tab);
    ctrl(&mut app, 'u');
    type_str(&mut app, "gremlin");
    assert_eq!(app.search.results.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["aaa"]);
    ctrl(&mut app, 'u');
    type_str(&mut app, "redirects sign");
    assert_eq!(app.search.results.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["bbb"], "words from the summary, in any order");
    assert!(
        app.search.results[0].detail.contains("Fixing redirects"),
        "when only the summary matched, it is what is shown: {}",
        app.search.results[0].detail
    );
}

// ---- the settings screens ---------------------------------------------------------------------------------------

/// The settings, on the tab of the entry called `name`, with that entry selected.
fn go(app: &mut App, name: &str) {
    if !matches!(app.mode, Mode::Settings) {
        key(app, KeyCode::Char('s'));
    }
    let (tab, at) = settings::find_item(name).unwrap_or_else(|| panic!("no entry called {name}"));
    while app.settings_tab != tab {
        key(app, KeyCode::Tab);
    }
    key(app, KeyCode::Home);
    for _ in 0..at {
        key(app, KeyCode::Down);
    }
}

fn click_at(app: &mut App, x: u16, y: u16) {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: x, row: y, modifiers: KeyModifiers::NONE });
}

#[test]
fn the_settings_have_six_tabs_that_tab_shift_tab_and_a_click_switch_with_the_showing_one_lit() {
    let w = World::new("sm-tabs");
    let mut app = w.app();
    // Wide enough for every title in full (at 70 columns "Smart features" is shortened to "Smart").
    let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
    key(&mut app, KeyCode::Char('s'));
    let general = screen(&mut term, &app);
    let tabs_line = general.lines().find(|l| l.contains("General")).unwrap().to_string();
    for title in ["General", "Search", "Smart features", "Sessions", "Projects", "Editors"] {
        assert!(tabs_line.contains(title), "{title} on the tab line: {tabs_line}");
    }
    // At 80 columns too, now that there are six.
    let mut at_80 = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let line = screen(&mut at_80, &app);
    let line = line.lines().find(|l| l.contains("General")).unwrap().to_string();
    for title in ["General", "Search", "Smart features", "Sessions", "Projects", "Editors"] {
        assert!(line.contains(title), "{title} on the tab line at 80 columns: {line}");
    }
    let mut narrow = Terminal::new(TestBackend::new(70, 24)).unwrap();
    let short = screen(&mut narrow, &app);
    let short = short.lines().find(|l| l.contains("General")).unwrap().to_string();
    assert!(short.contains(" Smart ") && !short.contains("Smart features") && short.contains("Editors"), "{short}");
    assert!(
        general.contains("Stop idle sessions after") && !general.contains("Daily token limit") && !general.contains("Used today"),
        "{general}"
    );
    // The showing tab is lit like the header's (raised background, bold), the others quiet.
    let lit = |term: &Terminal<TestBackend>, title: &str| {
        let buf = term.backend().buffer();
        let (y, line) = (0..buf.area.height)
            .map(|y| (y, (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>()))
            .find(|(_, l)| l.contains("General") && l.contains("Projects"))
            .unwrap();
        let x = line[..line.find(title).unwrap()].chars().count() as u16;
        let cell = &buf[(x, y)];
        (cell.bg, cell.fg, cell.modifier.contains(ratatui::style::Modifier::BOLD))
    };
    let t = app.theme.clone();
    assert_eq!(lit(&term, "General"), (t.surface, t.text, true));
    assert_eq!(lit(&term, "Search"), (t.bg, t.dim, false));
    // Tab goes on, Shift+Tab back, both round the ends.
    key(&mut app, KeyCode::Tab);
    assert_eq!((app.settings_tab, app.setting), (1, 0));
    screen(&mut term, &app);
    assert_eq!(lit(&term, "Search"), (t.surface, t.text, true));
    assert_eq!(lit(&term, "General"), (t.bg, t.dim, false));
    key(&mut app, KeyCode::BackTab);
    key(&mut app, KeyCode::BackTab);
    assert_eq!(app.settings_tab, 5, "shift+tab from the first goes to the last");
    let editors = screen(&mut term, &app);
    assert!(
        editors.contains("Editing platoon's text") && editors.contains("Editing files") && editors.contains("Always ask which editor"),
        "{editors}"
    );
    key(&mut app, KeyCode::BackTab);
    assert_eq!(app.settings_tab, 4);
    let projects = screen(&mut term, &app);
    assert!(projects.contains("Update notes after") && !projects.contains("Stop idle sessions after"), "{projects}");
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.settings_tab, 0, "tab from the last goes to the first");
    // A click on a title shows that tab, starting on its first entry.
    key(&mut app, KeyCode::Down);
    screen(&mut term, &app);
    let smart_title = app.hits.borrow().iter().find(|(_, h)| matches!(h, crate::app::Hit::SettingsTab(2))).map(|(r, _)| *r).unwrap();
    click_at(&mut app, smart_title.x + 2, smart_title.y);
    assert_eq!((app.settings_tab, app.setting, matches!(app.mode, Mode::Settings)), (2, 0, true));
    let smart_tab = screen(&mut term, &app);
    assert!(smart_tab.contains(settings::TABS[2].items[0].name) && smart_tab.contains("Used today"), "{smart_tab}");
    assert_eq!(lit(&term, "Smart features"), (t.surface, t.text, true));
    // The master switch's value reads Off, then On once it is on.
    assert!(smart_tab.lines().find(|l| l.contains(settings::TABS[2].items[0].name)).unwrap().contains("Off"), "{smart_tab}");
    smart(&w, |s| s.tokens_enabled = true);
    app.refresh_for_test();
    let smart_tab = screen(&mut term, &app);
    assert!(smart_tab.lines().find(|l| l.contains(settings::TABS[2].items[0].name)).unwrap().contains("On"), "{smart_tab}");
    // Esc closes the settings from any tab, and they open again on the first.
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List));
    key(&mut app, KeyCode::Char('s'));
    assert_eq!((app.settings_tab, app.setting), (0, 0));
}

#[test]
fn every_entry_of_every_tab_is_whole_with_its_explanation_at_80_by_24() {
    let w = World::new("sm-screen");
    simple(&w, "arc1", "please look into the archived topic one", 900);
    simple(&w, "arc2", "please look into the archived topic two", 1000);
    archive(&w, &["arc1", "arc2"]);
    let mut app = w.app();
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    for (ti, tab) in settings::TABS.iter().enumerate() {
        for item in &tab.items {
            go(&mut app, item.name);
            assert_eq!(app.settings_tab, ti);
            let shown = screen(&mut term, &app);
            let flat = shown.replace(['│', '╭', '╮', '╰', '╯', '─'], " ").split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(shown.contains(item.name), "{}: selected entry visible at 80x24:\n{shown}", item.name);
            assert!(
                flat.contains(&item.explain.split_whitespace().collect::<Vec<_>>().join(" ")),
                "{}: its explanation is whole at 80x24:\n{shown}",
                item.name
            );
            assert!(shown.contains("esc back") && shown.contains("Projects"), "{}:\n{shown}", item.name);
            // The day's use is on the smart features tab; the archived estimate with the archived entries on Sessions.
            assert_eq!(shown.contains("Used today: 0 of 500,000 tokens"), ti == settings::TAB_SMART, "{}:\n{shown}", item.name);
            let estimate = flat.contains("Archived: 2 sessions, about ") && flat.contains(" tokens to index (an estimate)");
            assert_eq!(estimate, ti == settings::TAB_SESSIONS, "{}:\n{shown}", item.name);
            for line in shown.lines().filter(|l| l.starts_with('│')) {
                assert!(line.trim_end_matches('│').ends_with(' '), "text touches the border: {line}");
            }
        }
    }
    // The estimate is the local formula: characters over four plus 450 for the one call.
    let e = app.archived_estimate();
    assert_eq!((e.conversations, e.calls), (2, 1));
    assert!(e.tokens > 450 && e.tokens < 2000, "{e:?}");
    // With nothing archived, it says so.
    let w2 = World::new("sm-screen-none");
    let mut app2 = w2.app();
    go(&mut app2, "Archived sessions");
    assert!(screen(&mut term, &app2).contains("Archived: none waiting to be named"));
}

#[test]
fn the_smart_features_screen_changes_the_new_settings_and_saves_them() {
    let w = World::new("sm-screen-keys");
    let mut app = w.app();
    go(&mut app, "Archived sessions");
    key(&mut app, KeyCode::Right);
    assert!(settings::load(&w.config).0.ai_archived);
    assert_eq!(
        serde_json::from_str::<Value>(&fs::read_to_string(w.config.join("settings.json")).unwrap()).unwrap()["ai_archived"],
        "background"
    );
    go(&mut app, "Share for archived");
    key(&mut app, KeyCode::Left);
    assert_eq!(settings::load(&w.config).0.ai_archived_share, 25);
    go(&mut app, "Wait for quiet before refreshing");
    key(&mut app, KeyCode::Right);
    assert_eq!(settings::load(&w.config).0.ai_quiet_seconds, 120);
    go(&mut app, "Description search: results");
    key(&mut app, KeyCode::Right);
    assert_eq!(settings::load(&w.config).0.ai_find_results, 15);
}

// ---- "Describe it": finding a conversation from a description ------------------------------------------------------

use crate::app::{FIND_KIND, FindState};

fn many(w: &World, n: usize, word: &str) {
    for i in 1..=n {
        simple(w, &format!("c{i:03}"), &format!("please look into the {word} topic number {i} for me"), 100 + i as u64 * 10);
    }
}

/// The dialog on "Describe it" (the switches must be on).
fn open_find(app: &mut App) {
    key(app, KeyCode::Char('/'));
    key(app, KeyCode::Tab);
    key(app, KeyCode::Tab);
    assert_eq!(app.search.kind, FIND_KIND, "the third kind is Describe it");
}

fn state(app: &App) -> FindState {
    app.find_state().clone()
}

#[test]
fn describe_it_is_offered_only_while_the_master_and_find_switches_are_on() {
    let w = World::new("fd-chip");
    simple(&w, "aaa", "please look into the flaky test for me", 300);
    let mut app = w.app();
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    key(&mut app, KeyCode::Char('/'));
    let shown = screen(&mut term, &app);
    assert!(shown.contains("Quick filter") && shown.contains("Full-text") && !shown.contains("Describe it"), "{shown}");
    assert_eq!(app.search_kinds(), [0, 1]);
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.search.kind, 0, "tab goes round the two kinds on offer");
    key(&mut app, KeyCode::Esc);
    // Master on, find off: still hidden. Find on, master off: hidden.
    smart(&w, |s| s.set_feature("find", false));
    app.refresh_for_test();
    assert_eq!(app.search_kinds(), [0, 1]);
    smart(&w, |s| s.tokens_enabled = false);
    app.refresh_for_test();
    assert_eq!(app.search_kinds(), [0, 1]);
    // Both on: three chips, in order, and tab goes round all three.
    smart(&w, |_| {});
    app.refresh_for_test();
    key(&mut app, KeyCode::Char('/'));
    let shown = screen(&mut term, &app);
    let line = shown.lines().find(|l| l.contains("Quick filter")).unwrap();
    assert!(line.find("Quick filter") < line.find("Full-text") && line.find("Full-text") < line.find("Describe it"), "{line}");
    for want in [1, 2, 0] {
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.search.kind, want);
    }
    key(&mut app, KeyCode::BackTab);
    assert_eq!(app.search.kind, FIND_KIND);
    // The chip is lit while it is showing, like the others (an accent background), and clicking one goes to it.
    let shown = screen(&mut term, &app);
    let (x, y) =
        shown.lines().enumerate().find_map(|(row, l)| l.find("Describe it").map(|b| (l[..b].chars().count() as u16, row as u16))).unwrap();
    assert_eq!(term.backend().buffer()[(x + 2, y)].bg, app.theme.accent);
    key(&mut app, KeyCode::BackTab);
    screen(&mut term, &app);
    app.on_mouse(ratatui::crossterm::event::MouseEvent {
        kind: ratatui::crossterm::event::MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left),
        column: x + 2,
        row: y,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.search.kind, FIND_KIND);
}

#[test]
fn reaching_describe_it_with_the_switches_off_says_how_to_turn_it_on_and_asks_nothing() {
    let w = World::new("fd-hint");
    simple(&w, "aaa", "please look into the flaky test for me", 300);
    let fake = AiFake::new("fd-hint");
    let mut app = app_with(&w, &fake);
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    key(&mut app, KeyCode::Char('/'));
    app.set_search_kind(FIND_KIND);
    let shown = screen(&mut term, &app);
    let flat = shown.split_whitespace().collect::<Vec<_>>().join(" ").replace("│ ", "").replace(" │", "");
    assert!(
        flat.contains("Describe it is switched off")
            && flat.contains("Smart features")
            && flat.contains("Enable features which require tokens")
            && flat.contains("Find a session by description"),
        "{shown}"
    );
    type_str(&mut app, "the flaky thing");
    key(&mut app, KeyCode::Enter);
    match state(&app) {
        FindState::Failed(m) => assert!(m.contains("is switched off") && m.contains("press s"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(fake.calls(), 0);
}

#[test]
fn nothing_is_asked_while_typing_only_when_enter_is_pressed_and_enter_runs_exactly_one_call() {
    let w = World::new("fd-enter");
    many(&w, 12, "invoices");
    smart(&w, |s| s.set_feature("names", false)); // only the search may use tokens here
    let fake = AiFake::new("fd-enter");
    let mut app = app_with(&w, &fake);
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    open_find(&mut app);
    type_str(&mut app, "that thing about invoices");
    assert_eq!((fake.calls(), state(&app)), (0, FindState::Idle), "typing asks nothing");
    let shown = screen(&mut term, &app);
    assert!(
        shown.contains("Press enter to ask Claude. It uses some of your plan, and nothing is") && shown.contains("sent until you do."),
        "{shown}"
    );
    key(&mut app, KeyCode::Enter);
    assert_eq!(state(&app), FindState::Asking);
    assert!(screen(&mut term, &app).contains("Asking Claude..."));
    app.settle_ai_for_test(10);
    assert_eq!((fake.calls(), state(&app)), (1, FindState::Done));
    // The call: the description, then the candidates by number, nothing else.
    let sent = fake.stdin(1);
    assert!(sent.starts_with("Description: that thing about invoices\n"), "{sent}");
    assert_eq!(sent.lines().filter(|l| l.starts_with('[')).count(), 12, "all twelve are candidates (fewer than the limit)");
    assert!(!sent.contains("/work/"), "no path is sent, only the folder's own name: {sent}");
    let sys = fake.argv(1).join("\n");
    assert!(sys.contains("DATA") && sys.contains("at most 10"), "{sys}");
    // Three results, each with its reason; the highlighted first one.
    assert_eq!(app.search.results.len(), 3);
    assert_eq!(app.search.results.iter().map(|r| r.detail.as_str()).collect::<Vec<_>>(), ["Reason 1", "Reason 2", "Reason 3"]);
    let shown = screen(&mut term, &app);
    assert!(shown.contains("Reason 1") && shown.contains("Reason 3") && shown.contains("3 found"), "{shown}");
    assert!(shown.contains("enter open") && shown.contains("ctrl+w search wider"), "{shown}");
    // More typing drops the answer (it was for other words); enter asks again.
    type_str(&mut app, " please");
    assert_eq!((state(&app), app.search.results.len()), (FindState::Idle, 0));
    key(&mut app, KeyCode::Enter);
    app.settle_ai_for_test(10);
    assert_eq!(fake.calls(), 2);
    // Enter on a result jumps to it exactly like Full-text does.
    let first = app.search.results[0].id.clone();
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.selected_session().map(|s| s.id.clone()), Some(first));
    assert_eq!(app.filter, "", "nothing narrows the list");
    assert_eq!(fake.calls(), 2, "opening a result asks nothing more");
}

#[test]
fn a_result_on_the_archived_tab_opens_that_tab_and_the_results_are_capped_and_use_only_known_numbers() {
    let w = World::new("fd-archived");
    many(&w, 6, "billing");
    archive(&w, &["c006"]);
    smart(&w, |s| {
        s.set_feature("names", false);
        s.ai_find_results = 3;
    });
    let fake = AiFake::new("fd-archived");
    fake.reply_default(r#"[{"n":99,"reason":"unknown"},{"n":1,"reason":"one"},{"n":2,"reason":"two"},{"n":1,"reason":"again"},{"n":3,"reason":"three"},{"n":4,"reason":"four"},{"n":5,"reason":"five"}]"#);
    let mut app = app_with(&w, &fake);
    open_find(&mut app);
    type_str(&mut app, "billing");
    key(&mut app, KeyCode::Enter);
    app.settle_ai_for_test(10);
    assert_eq!(
        app.search.results.iter().map(|r| r.detail.as_str()).collect::<Vec<_>>(),
        ["one", "two", "three"],
        "known numbers, once each, at most the setting's 3"
    );
    // Make the archived one the result and open it: the Archived tab comes up with it selected.
    fake.reply(2, r#"[{"n":6,"reason":"the archived one"}]"#);
    ctrl(&mut app, 'u');
    type_str(&mut app, "billing again");
    key(&mut app, KeyCode::Enter);
    app.settle_ai_for_test(10);
    assert_eq!(app.search.results.len(), 1);
    let id = app.search.results[0].id.clone();
    assert!(app.sessions.iter().find(|s| s.id == id).unwrap().archived, "candidate 6 in this call is an archived conversation");
    key(&mut app, KeyCode::Enter);
    assert_eq!((app.tab, app.selected_session().map(|s| s.id.clone())), (1, Some(id)));
}

#[test]
fn esc_calls_off_the_request_kills_it_and_stays_in_the_dialog_and_a_second_esc_closes() {
    let w = World::new("fd-esc");
    many(&w, 3, "billing");
    smart(&w, |s| s.set_feature("names", false));
    let fake = AiFake::new("fd-esc");
    fake.hang(true);
    let mut app = app_with(&w, &fake);
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    open_find(&mut app);
    type_str(&mut app, "the billing one");
    key(&mut app, KeyCode::Enter);
    app.refresh_until_for_test(10, "the call to start", |_| fake.pids().len() == 2);
    let pids = fake.pids();
    assert!(pids.iter().all(|&p| crate::testkit::alive(p)));
    let shown = screen(&mut term, &app);
    assert!(shown.contains("Asking Claude...") && shown.contains("esc cancel"), "{shown}");
    // Typing is held while it is out.
    type_str(&mut app, "zzz");
    assert_eq!(app.search.text, "the billing one");
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::Search), "still in the dialog, back to typing");
    assert_eq!(state(&app), FindState::Idle);
    app.refresh_until_for_test(10, "the call to end", |a| !a.smart_busy());
    for _ in 0..200 {
        if pids.iter().all(|&p| !crate::testkit::alive(p)) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(pids.iter().all(|&p| !crate::testkit::alive(p)), "the process and what it started are gone");
    assert_eq!(app.search.text, "the billing one", "the words are still there to change");
    // A cancelled call is no failure: nothing is paused.
    assert!(Ledger::open(&w.cache).load().claude.backoff.is_none());
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn the_refusals_are_shown_plainly_in_the_dialog_and_nothing_is_started() {
    let w = World::new("fd-refuse");
    many(&w, 3, "billing");
    smart(&w, |s| {
        s.set_feature("names", false);
        s.ai_daily_tokens = 20_000;
    });
    let fake = AiFake::new("fd-refuse");
    let mut app = app_with(&w, &fake);
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    open_find(&mut app);
    type_str(&mut app, "the billing one");
    // The daily limit.
    Ledger::open(&w.cache)
        .record_call(usage::now(), Agent::Claude, "names", &crate::ledger::Usage { input: 25_000, ..Default::default() }, 0.0, true)
        .unwrap();
    key(&mut app, KeyCode::Enter);
    let shown = screen(&mut term, &app);
    let flat = shown.split_whitespace().collect::<Vec<_>>().join(" ").replace("│ ", "").replace(" │", "");
    assert!(flat.contains("Today's token limit is used up") && flat.contains("or you can raise it in settings"), "{shown}");
    assert_eq!(fake.calls(), 0);
    // A back-off after a failure.
    let l = Ledger::open(&w.cache);
    let _ = fs::remove_file(l.path());
    l.note_failure(usage::now(), Agent::Claude, crate::ledger::FailKind::Network).unwrap();
    key(&mut app, KeyCode::Enter);
    assert!(
        matches!(state(&app), FindState::Failed(ref m) if m.contains("paused after a failure") && m.contains("trying again in")),
        "{:?}",
        state(&app)
    );
    assert_eq!(fake.calls(), 0);
    // A call that fails: the plain message, and the back-off starts.
    let _ = fs::remove_file(l.path());
    fake.fail(Some("Not logged in · Please run /login"));
    key(&mut app, KeyCode::Enter);
    app.settle_ai_for_test(10);
    assert_eq!(fake.calls(), 1);
    assert!(matches!(state(&app), FindState::Failed(ref m) if m == "Claude isn't signed in."), "{:?}", state(&app));
    assert!(screen(&mut term, &app).contains("Claude isn't signed in."));
    // Master switch turned off elsewhere while the dialog is open: enter says so.
    smart(&w, |s| s.tokens_enabled = false);
    app.refresh_for_test();
    let _ = fs::remove_file(l.path());
    key(&mut app, KeyCode::Enter);
    assert!(
        matches!(state(&app), FindState::Failed(ref m) if m.contains("Enable features which require tokens") && m.contains("switched off")),
        "{:?}",
        state(&app)
    );
}

#[test]
fn a_description_search_ignores_the_plan_pause_that_holds_the_background_work_back() {
    let w = World::new("fd-plan");
    many(&w, 3, "billing");
    smart(&w, |_| {});
    let ts = usage::now();
    fs::create_dir_all(w.cache.join("status")).unwrap();
    fs::write(
        w.cache.join("status/aaaa.json"),
        json!({"ts": ts, "context_pct": null, "five_hour": {"pct": 97, "resets_at": ts + 3600}, "seven_day": null}).to_string(),
    )
    .unwrap();
    let fake = AiFake::new("fd-plan");
    let mut app = app_with(&w, &fake);
    app.settle_ai_for_test(5);
    assert_eq!(fake.calls(), 0, "naming waits for the plan");
    open_find(&mut app);
    type_str(&mut app, "the billing one");
    key(&mut app, KeyCode::Enter);
    app.settle_ai_for_test(10);
    assert_eq!((fake.calls(), state(&app)), (1, FindState::Done), "but the person asked for this one");
    assert!(app.smart_idle().is_some_and(|m| m.contains("97%")), "and the naming is still waiting");
}

#[test]
fn a_wider_search_says_what_it_costs_waits_for_enter_and_sends_more_candidates() {
    let w = World::new("fd-wide");
    many(&w, 70, "billing");
    smart(&w, |s| {
        s.set_feature("names", false);
        s.ai_find_candidates = 20;
        s.ai_find_wide_candidates = 60;
    });
    let fake = AiFake::new("fd-wide");
    let mut app = app_with(&w, &fake);
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    open_find(&mut app);
    ctrl(&mut app, 'w');
    assert_eq!(state(&app), FindState::Idle, "nothing to widen before there are words");
    type_str(&mut app, "billing");
    key(&mut app, KeyCode::Enter);
    app.settle_ai_for_test(10);
    let heads = |k: usize| fake.stdin(k).lines().filter(|l| l.starts_with('[')).count();
    assert_eq!(heads(1), 20, "the first search sends the setting's 20");
    // Ctrl+W: a question, no call yet.
    ctrl(&mut app, 'w');
    let count = match state(&app) {
        FindState::Confirm { estimate, count } => {
            assert!(estimate.tokens > 450);
            count
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(count, 60);
    let shown = screen(&mut term, &app);
    let flat = shown.split_whitespace().collect::<Vec<_>>().join(" ").replace("│ ", "").replace(" │", "");
    assert!(
        flat.contains("Search wider? This sends 60 sessions to Claude, about ")
            && flat.contains("tokens (an estimate)")
            && flat.contains("enter to go ahead")
            || flat.contains("Press enter to go ahead"),
        "{shown}"
    );
    assert!(shown.contains("enter go ahead") && shown.contains("esc back"), "{shown}");
    assert_eq!(fake.calls(), 1);
    // Esc goes back to the earlier answer without asking anything.
    key(&mut app, KeyCode::Esc);
    assert_eq!((state(&app), app.search.results.len(), matches!(app.mode, Mode::Search)), (FindState::Done, 3, true));
    assert_eq!(fake.calls(), 1);
    // Ctrl+W then enter: one more call, with the wider list.
    ctrl(&mut app, 'w');
    type_str(&mut app, "typing is held while the question is open");
    assert_eq!(app.search.text, "billing");
    key(&mut app, KeyCode::Enter);
    app.settle_ai_for_test(10);
    assert_eq!((fake.calls(), heads(2)), (2, 60));
    assert_eq!(state(&app), FindState::Done);
}

#[test]
fn candidates_come_from_the_local_words_and_a_weak_match_is_topped_up_with_the_most_recent() {
    let w = World::new("fd-narrow");
    many(&w, 30, "billing");
    for i in 1..=3 {
        simple(&w, &format!("z{i}"), &format!("something else entirely number {i} about gardening"), 50_000 + i as u64);
    }
    smart(&w, |s| s.set_feature("names", false));
    let fake = AiFake::new("fd-narrow");
    let mut app = app_with(&w, &fake);
    open_find(&mut app);
    type_str(&mut app, "gardening plants");
    key(&mut app, KeyCode::Enter);
    app.settle_ai_for_test(10);
    let sent = fake.stdin(1);
    let heads = sent.lines().filter(|l| l.starts_with('[')).count();
    assert_eq!(heads, 20, "three matched, so it is topped up with the most recent to 20");
    assert!(sent.find("gardening").unwrap() < sent.find("billing").unwrap(), "the local matches come first:\n{sent}");
    // Conversations that have a summary are represented by it; the others by a piece of their first and last message.
    let mut sums = crate::summaries::Summaries::open(&w.config);
    sums.set_many(&[(
        "c001".into(),
        crate::summaries::Entry {
            name: "Billing export".into(),
            summary: "Exporting invoices to CSV.".into(),
            covers: 1,
            model: "haiku".into(),
            at: 1,
        },
    )])
    .unwrap();
    app.refresh_for_test();
    ctrl(&mut app, 'u');
    type_str(&mut app, "invoices csv");
    key(&mut app, KeyCode::Enter);
    app.settle_ai_for_test(10);
    let sent = fake.stdin(2);
    assert!(sent.contains("Name: Billing export\nFolder: c001\nSummary: Exporting invoices to CSV.\n"), "{sent}");
    assert!(sent.contains("First message: "), "the others are shown by their first message:\n{sent}");
}

#[test]
fn a_description_search_waits_for_the_naming_call_in_flight_instead_of_running_two_at_once() {
    let w = World::new("fd-serial");
    many(&w, 3, "billing");
    smart(&w, |_| {});
    let fake = AiFake::new("fd-serial");
    fake.slow(1.0);
    let mut app = app_with(&w, &fake);
    app.refresh_until_for_test(10, "the naming call to start", |_| fake.calls() == 1);
    open_find(&mut app);
    type_str(&mut app, "the billing one");
    key(&mut app, KeyCode::Enter);
    assert_eq!(state(&app), FindState::Waiting, "one call at a time");
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    assert!(screen(&mut term, &app).contains("Asking Claude..."));
    app.refresh_until_for_test(10, "the search to be answered", |a| matches!(a.find_state(), FindState::Done));
    assert_eq!(fake.calls(), 2, "the naming call, then the search");
    assert!(fake.stdin(1).starts_with("Name and summarize") && fake.stdin(2).starts_with("Description:"));
}

#[test]
fn esc_while_the_search_waits_for_the_naming_call_drops_it_and_nothing_is_asked() {
    let w = World::new("fd-serial-esc");
    many(&w, 3, "billing");
    smart(&w, |_| {});
    let fake = AiFake::new("fd-serial-esc");
    fake.slow(0.6);
    let mut app = app_with(&w, &fake);
    app.refresh_until_for_test(10, "the naming call to start", |_| fake.calls() == 1);
    open_find(&mut app);
    type_str(&mut app, "the billing one");
    key(&mut app, KeyCode::Enter);
    assert_eq!(state(&app), FindState::Waiting);
    key(&mut app, KeyCode::Esc);
    assert_eq!(state(&app), FindState::Idle);
    assert!(matches!(app.mode, Mode::Search));
    app.settle_ai_for_test(10);
    assert_eq!(fake.calls(), 1, "only the naming call was ever made");
}

#[test]
fn describe_it_at_80_by_24_and_wider_shows_the_chips_the_words_and_the_results_with_their_reasons() {
    let w = World::new("fd-draw");
    many(&w, 4, "billing");
    smart(&w, |s| s.set_feature("names", false));
    let fake = AiFake::new("fd-draw");
    let mut app = app_with(&w, &fake);
    for (cols, rows) in [(80, 24), (100, 30), (60, 20)] {
        let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
        app.mode = Mode::List;
        open_find(&mut app);
        if app.search.text.is_empty() {
            type_str(&mut app, "the billing one");
            key(&mut app, KeyCode::Enter);
            app.settle_ai_for_test(10);
        }
        let shown = screen(&mut term, &app);
        let flat = shown.split_whitespace().collect::<Vec<_>>().join(" ").replace("│ ", "").replace(" │", "");
        for want in ["Describe it", "the billing one", "Reason 1", "Reason 2", "enter open", "esc close"] {
            assert!(flat.contains(want), "{want:?} missing at {cols}x{rows}:\n{shown}");
        }
        // The first result is highlighted (the accent bar) and its reason is under its name.
        let lines: Vec<&str> = shown.lines().collect();
        let bar_y = (0..lines.len() - 1)
            .find(|&r| lines[r].contains('▎') && lines[r].contains("c00") && lines[r + 1].contains("Reason 1"))
            .unwrap_or_else(|| panic!("no highlighted result at {cols}x{rows}:\n{shown}")) as u16;
        let bar_x = lines[bar_y as usize].rfind('▎').map(|b| lines[bar_y as usize][..b].chars().count() as u16).unwrap();
        assert_eq!(term.backend().buffer()[(bar_x, bar_y)].fg, app.theme.accent);
        let below: String = (0..cols).map(|x| term.backend().buffer()[(x, bar_y + 1)].symbol().to_string()).collect();
        assert!(below.contains("Reason 1"), "the reason sits under the name at {cols}x{rows}: {below}");
        for line in shown.lines().filter(|l| l.starts_with('│')) {
            assert!(
                line.trim_end_matches('│').ends_with(' ') || line.trim_end_matches('│').chars().last().is_some_and(|c| c == ' '),
                "text touches the border at {cols}x{rows}: {line}"
            );
        }
    }
}

// ---- how the rows, the strip and the header look --------------------------------------------------------------------

fn name_generated(w: &World, id: &str, name: &str, summary: &str) {
    let mut sums = crate::summaries::Summaries::open(&w.config);
    sums.set_many(&[(
        id.into(),
        crate::summaries::Entry { name: name.into(), summary: summary.into(), covers: 1, model: "haiku".into(), at: 1 },
    )])
    .unwrap();
}

/// (column, row) of the first `needle` on the screen, counted in characters.
fn at(shown: &str, needle: &str) -> Option<(u16, u16)> {
    shown.lines().enumerate().find_map(|(row, l)| l.find(needle).map(|b| (l[..b].chars().count() as u16, row as u16)))
}

#[test]
fn a_generated_name_has_a_dim_mark_at_the_end_of_its_column_and_a_name_you_chose_has_none() {
    let w = World::new("rd-marker");
    simple(&w, "gen", "please fix the login redirect after sign in", 300);
    simple(&w, "own", "add a csv export to the invoices report", 400);
    simple(&w, "fol", "why does the nightly build fail on ci", 500);
    name_generated(&w, "gen", "Login fix", "Fixing the redirect.");
    name_generated(&w, "own", "Generated but overridden", "Some summary.");
    crate::names::Names::open(&w.config).set("own", "My own name").unwrap();
    let mut app = w.app();
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let shown = screen(&mut term, &app);
    let line = |name: &str| shown.lines().find(|l| l.contains(name)).unwrap_or_else(|| panic!("no row {name}:\n{shown}")).to_string();
    let (made, own, fol) = (line("Login fix"), line("My own name"), line("/work/fol"));
    assert!(made.contains('✦') && !own.contains('✦') && !fol.contains('✦'), "only the generated name is marked:\n{shown}");
    // It sits after the name, before the folder, with air on both sides, and is drawn dim on the row's background.
    let (mx, my) = at(&shown, "✦").unwrap();
    let chars: Vec<char> = made.chars().collect();
    assert_eq!((chars[mx as usize - 1], chars[mx as usize + 1]), (' ', ' '));
    assert!(mx as usize > made.find("Login").map(|b| made[..b].chars().count()).unwrap() + "Login fix".len(), "after the name");
    let dir_at = made.find("/work/gen").map(|b| made[..b].chars().count()).unwrap();
    assert!(mx as usize + 1 < dir_at, "before the folder column");
    let cell = &term.backend().buffer()[(mx, my)];
    let t = &app.theme;
    let dim = if cell.bg == t.surface { t.readable_on(t.dim, t.surface, crate::theme::MIN_DIM) } else { t.dim };
    assert_eq!(cell.fg, dim);
    // The name itself is drawn as any name is: not dim.
    let (nx, ny) = at(&shown, "Login fix").unwrap();
    assert_eq!(term.backend().buffer()[(nx, ny)].fg, app.theme.text);
    // A terminal whose font may lack the glyph gets a plain star in the same place.
    app.plain_marker = true;
    let plain = screen(&mut term, &app);
    assert!(!plain.contains('✦') && at(&plain, "*").map(|p| p.0) == Some(mx), "{plain}");
    assert!(crate::app::detect_plain_marker_with(&|k| (k == "PLATOON_GLYPHS").then(|| "plain".to_string())));
    assert!(crate::app::detect_plain_marker_with(&|k| (k == "TERM").then(|| "linux".to_string())));
    assert!(!crate::app::detect_plain_marker_with(&|k| match k {
        "TERM" => Some("linux".into()),
        "PLATOON_GLYPHS" => Some("fancy".into()),
        _ => None,
    }));
    assert!(!crate::app::detect_plain_marker_with(&|k| (k == "TERM").then(|| "xterm-256color".to_string())));
    // Narrow columns still keep the mark, and the list is never cut in half by it.
    let mut narrow = Terminal::new(TestBackend::new(60, 20)).unwrap();
    app.plain_marker = false;
    let shown = screen(&mut narrow, &app);
    assert!(shown.contains('✦'), "{shown}");
}

#[test]
fn when_the_path_has_to_be_cut_a_row_with_another_name_keeps_the_end_of_it_so_the_folder_shows() {
    let w = World::new("rd-folder");
    let deep = "/work/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/project-folder";
    conv(&w, "gen", deep, &["please fix the login redirect after sign in"], "Done.", 300);
    conv(&w, "fol", deep, &["add a csv export to the invoices report"], "Done.", 400);
    name_generated(&w, "gen", "Login fix", "Fixing the redirect.");
    let app = w.app();
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let shown = screen(&mut term, &app);
    let made = shown.lines().find(|l| l.contains("Login fix")).unwrap();
    let fol = shown.lines().find(|l| l.contains("fol") || l.contains("project-folder") && !l.contains("Login")).unwrap();
    assert!(made.contains("…project-folder") || made.contains("project-folder"), "the folder stays on a row with a made-up name:\n{shown}");
    assert!(fol.contains("/work/aaaa") || fol.contains("project"), "{shown}");
    assert!(!made.contains("/work/aaaa"), "it is the start that goes when there is not room:\n{shown}");
}

#[test]
fn the_summary_of_the_selected_row_shows_in_two_lines_just_above_the_footer_and_follows_the_selection() {
    let w = World::new("rd-strip");
    simple(&w, "aaa", "please fix the login redirect after sign in", 300);
    simple(&w, "bbb", "add a csv export to the invoices report", 400);
    simple(&w, "ccc", "why does the nightly build fail on ci", 500);
    let long = "Fixing the redirect that sends people back to the sign in page after they have logged in, and adding a test for it, and checking the other pages that redirect the same way, and then a third sentence that does not fit.";
    name_generated(&w, "aaa", "Login redirect fix", long);
    name_generated(&w, "bbb", "Invoices CSV", "Adding a CSV export to the invoices report.");
    let mut app = w.app();
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let shown = screen(&mut term, &app);
    let lines: Vec<&str> = shown.lines().collect();
    let footer = lines.iter().position(|l| l.contains("enter open")).unwrap();
    assert_eq!(footer, 23, "the footer is the last line: there is no frame");
    assert!(
        lines[footer - 2].contains("✦ Fixing the redirect")
            && lines[footer - 1].starts_with("   ")
            && lines[footer - 1].contains("adding a test"),
        "two lines right above it:\n{shown}"
    );
    assert!(
        lines[footer - 1].contains('…') && lines[footer - 1].ends_with(' '),
        "a longer summary is cut with an ellipsis, not run to the edge:\n{shown}"
    );
    assert!(!shown.contains("third sentence"));
    // The marker is dim, the words are the subtext color.
    let (x, y) = at(&shown, "✦ Fixing").unwrap();
    assert_eq!((term.backend().buffer()[(x, y)].fg, term.backend().buffer()[(x + 3, y)].fg), (app.theme.dim, app.theme.subtext));
    // It follows the selection.
    key(&mut app, KeyCode::Down);
    let shown = screen(&mut term, &app);
    assert!(shown.contains("✦ Adding a CSV export to the invoices report.") && !shown.contains("Fixing the redirect"), "{shown}");
    // A row with no summary has no strip: the two lines are empty again.
    key(&mut app, KeyCode::Down);
    let shown = screen(&mut term, &app);
    let lines: Vec<&str> = shown.lines().collect();
    let footer = lines.iter().position(|l| l.contains("enter open")).unwrap();
    assert!(
        lines[footer - 1].trim_matches(|c| c == '│' || c == ' ').is_empty()
            && lines[footer - 2].trim_matches(|c| c == '│' || c == ' ').is_empty(),
        "{shown}"
    );
    // Too short a terminal: it is left out, the list keeps its room.
    key(&mut app, KeyCode::Home);
    for (rows, shown_strip) in [(14u16, false), (15, true), (20, true), (40, true)] {
        let mut t = Terminal::new(TestBackend::new(80, rows)).unwrap();
        let s = screen(&mut t, &app);
        assert_eq!(s.contains("✦ Fixing the redirect"), shown_strip, "80x{rows}:\n{s}");
        assert!(s.contains("enter open"), "80x{rows}");
    }
    // Archived rows too, and every summary is searched by the filter.
    assert!(!screen(&mut Terminal::new(TestBackend::new(80, 24)).unwrap(), &app).contains("third sentence"));
}

#[test]
fn the_header_never_counts_the_names_being_made() {
    // The count ("Indexing 0 of 4") was noise: the names arrive on their own, and a stop still shows through the
    // header's note on why nothing that spends tokens can run.
    let w = World::new("rd-progress");
    for i in 1..=4 {
        simple(&w, &format!("c{i}"), &format!("please look into topic number {i} for me"), 100 * i as u64);
    }
    smart(&w, |s| s.ai_batch_size = 2);
    let fake = AiFake::new("rd-progress");
    fake.slow(0.5);
    let mut app = app_with(&w, &fake);
    let mut term = Terminal::new(TestBackend::new(112, 24)).unwrap();
    app.refresh_until_for_test(10, "the first call to start", |_| fake.calls() == 1);
    assert!(app.smart_progress().is_some(), "there is work");
    let shown = screen(&mut term, &app);
    assert!(!shown.lines().nth(1).unwrap().contains("Indexing"), "{shown}");
    app.settle_ai_for_test(10);
}

#[test]
fn the_old_row_layout_is_unchanged_without_smart_features() {
    let w = World::new("rd-plain");
    simple(&w, "aaa", "please fix the login redirect after sign in", 300);
    let app = w.app();
    let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
    let shown = screen(&mut term, &app);
    assert!(!shown.contains('✦') && !shown.contains("Indexing"), "{shown}");
    let lines: Vec<&str> = shown.lines().collect();
    let footer = lines.iter().position(|l| l.contains("enter open")).unwrap();
    assert!(lines[footer - 1].trim_matches(|c| c == '│' || c == ' ').is_empty(), "no strip");
}

// ---- platoon's own helper is never a conversation --------------------------------------------------------------

/// The ways the helper's folder can be written down (it exists, so `..` and the link resolve).
fn helper_spellings(w: &World) -> Vec<String> {
    let work = w.cache.join("ai-work");
    fs::create_dir_all(&work).unwrap();
    fs::create_dir_all(w.cache.join("x")).unwrap();
    std::os::unix::fs::symlink(&work, w.base.join("link-to-work")).unwrap();
    vec![
        work.display().to_string(),
        format!("{}/", work.display()),
        w.cache.join("x/../ai-work").display().to_string(),
        w.base.join("link-to-work").display().to_string(),
    ]
}

#[test]
fn the_helpers_own_process_is_no_row_no_count_and_no_badge_and_the_screen_does_not_change() {
    let mut w = World::new("hid-live");
    let now = usage::now();
    w.live("real1", "/work/real1", now - 100, "busy");
    w.live("real2", "/work/real2", now - 90, "idle");
    simple(&w, "real3", "please look into the saved one", 300);
    // Similar names are real folders and stay in the list.
    fs::create_dir_all(w.cache.join("ai-work-2")).unwrap();
    w.live("near1", &w.cache.join("ai-work-2").display().to_string(), now - 80, "idle");
    w.live("near2", "/somewhere/else/ai-work", now - 70, "idle");
    let mut app = w.app();
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    app.refresh_for_test();
    let ids = |app: &App| {
        let mut v: Vec<String> = app.sessions.iter().map(|s| s.id.clone()).collect();
        v.sort();
        v
    };
    let before = (ids(&app), app.counts.clone(), screen(&mut term, &app));
    assert_eq!(before.0, ["near1", "near2", "real1", "real2", "real3"]);
    // The helper, once per spelling of its folder, busy (would be a working badge) and idle.
    for (i, cwd) in helper_spellings(&w).iter().enumerate() {
        w.live(&format!("helper-{i}"), cwd, now - 50 + i as u64, if i % 2 == 0 { "busy" } else { "idle" });
    }
    w.register_bg(std::process::id(), "helper-bg-0001", &w.cache.join("ai-work").display().to_string(), now - 40, "helper-b", false);
    app.refresh_for_test();
    app.refresh_for_test();
    assert_eq!((ids(&app), app.counts.clone()), (before.0.clone(), before.1.clone()));
    assert_eq!(screen(&mut term, &app), before.2, "the list looks exactly the same");
    assert!(!app.sessions.iter().any(|s| s.id.starts_with("helper") || s.cwd.ends_with("/cache/ai-work")));
}

#[test]
fn a_transcript_recorded_in_the_helpers_folder_is_no_row_no_search_hit_and_no_naming_candidate() {
    let w = World::new("hid-transcript");
    let mut spellings = helper_spellings(&w).into_iter();
    // A real conversation and a decoy in a similarly named folder, both about "zebra".
    simple(&w, "real", "please look into the zebra crossing bug", 300);
    conv(&w, "decoy", &w.cache.join("ai-work-2").display().to_string(), &["please look into the zebra decoy"], "Done.", 400);
    // The helper's own transcripts, one per spelling, all about "zebra" too, one of them archived by an old run.
    for (i, cwd) in spellings.by_ref().enumerate() {
        conv(&w, &format!("helper{i}"), &cwd, &["please look into the zebra helper prompt"], "Done.", 100 + i as u64);
    }
    archive(&w, &["helper3"]);
    smart(&w, |s| s.ai_archived = true);
    let fake = AiFake::new("hid-transcript");
    let mut app = app_with(&w, &fake);
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let ids = |app: &App| {
        let mut v: Vec<String> = app.sessions.iter().map(|s| s.id.clone()).collect();
        v.sort();
        v
    };
    assert_eq!(ids(&app), ["decoy", "real"]);
    assert_eq!(app.counts, [2, 0], "the helper's archived one is not counted either");
    assert!(!screen(&mut term, &app).contains("helper"));
    // Estimate: only what could really be named.
    let solo = {
        let w2 = World::new("hid-transcript-solo");
        simple(&w2, "real", "please look into the zebra crossing bug", 300);
        conv(&w2, "decoy", &w2.cache.join("ai-work-2").display().to_string(), &["please look into the zebra decoy"], "Done.", 400);
        w2.app().archived_estimate()
    };
    assert_eq!(app.archived_estimate(), solo);
    // Full-text: the words are found in the two real ones only.
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Tab);
    type_str(&mut app, "zebra");
    let mut hits: Vec<String> = app.search.results.iter().map(|r| r.id.clone()).collect();
    hits.sort();
    assert_eq!(hits, ["decoy", "real"]);
    // Describe it: nothing of the helper's is sent. (Call 1 named the conversations, call 2 is the search.)
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Enter);
    app.settle_ai_for_test(10);
    assert_eq!(fake.calls(), 2);
    let all: String = (1..=fake.calls()).map(|k| fake.stdin(k)).collect::<Vec<_>>().join("\n");
    assert!(!all.contains("helper prompt"), "{all}");
    // Naming: two conversations, and after it the helper still is not a row.
    let heads = fake.stdin(1).lines().filter(|l| l.starts_with('[')).count();
    assert_eq!(heads, 2, "{}", fake.stdin(1));
    assert_eq!(ids(&app), ["decoy", "real"]);
}

#[test]
fn a_describe_search_does_not_offer_the_helpers_transcripts() {
    let w = World::new("hid-find");
    conv(&w, "real", "/work/real", &["please look into the zebra crossing bug"], "Done.", 300);
    conv(&w, "helper", &w.cache.join("ai-work").display().to_string(), &["please look into the zebra helper prompt"], "Done.", 100);
    smart(&w, |s| s.set_feature("names", false));
    let fake = AiFake::new("hid-find");
    let mut app = app_with(&w, &fake);
    open_find(&mut app);
    type_str(&mut app, "zebra");
    key(&mut app, KeyCode::Enter);
    app.settle_ai_for_test(10);
    assert_eq!(fake.calls(), 1);
    let sent = fake.stdin(1);
    assert!(sent.contains("crossing bug") && !sent.contains("helper prompt"), "{sent}");
    assert!(app.search.results.iter().all(|r| r.id == "real"));
}

#[test]
fn auto_archive_never_reaches_the_helpers_old_transcript() {
    let w = World::new("hid-auto");
    w.settings(120, 1);
    conv(&w, "old-real", "/work/real", &["please look into the old thing"], "Done.", 3 * 24 * 60 * 60 / 60 * 60);
    conv(
        &w,
        "old-helper",
        &w.cache.join("ai-work").display().to_string(),
        &["please look into the helper thing"],
        "Done.",
        3 * 24 * 60 * 60 / 60 * 60,
    );
    let mut app = w.app();
    app.refresh_for_test();
    let archived: Vec<String> = w.archive_file().map(|v| v.to_string()).into_iter().collect();
    let text = archived.join("");
    assert!(text.contains("old-real") && !text.contains("old-helper"), "{text}");
    assert_eq!(app.counts, [0, 1]);
}

#[test]
fn every_call_carries_the_no_thinking_and_helper_marks_in_its_environment() {
    let w = World::new("hid-env");
    simple(&w, "aaa", "please fix the login redirect", 300);
    smart(&w, |_| {});
    let fake = AiFake::new("hid-env");
    let mut app = app_with(&w, &fake);
    app.settle_ai_for_test(10);
    let env = fake.env(1);
    assert!(env.lines().any(|l| l == "MAX_THINKING_TOKENS=0") && env.lines().any(|l| l == "PLATOON_AI=1"), "{env}");
    // Nothing else about the call changed: the same flags as before.
    let argv = fake.argv(1);
    assert_eq!(argv[..2], ["-p", "--model"]);
    assert!(!argv.iter().any(|a| a.contains("thinking")), "{argv:?}");
}

// ---- the user's own instructions ------------------------------------------------------------------------------------

/// The user's own guidance for an action, as if they had edited it in the settings.
fn own_guidance(w: &World, id: &str, text: &str) {
    fs::create_dir_all(w.config.join("prompts")).unwrap();
    fs::write(w.config.join("prompts").join(format!("{id}.md")), text).unwrap();
}

#[test]
fn naming_sends_the_users_own_guidance_then_the_fixed_part_and_the_built_in_text_without_it() {
    use crate::instructions::{NAMES, compose};
    let w = World::new("sm-own-names");
    simple(&w, "aaa", "please fix the login redirect", 300);
    smart(&w, |_| {});
    let fake = AiFake::new("sm-own-names");
    let mut app = app_with(&w, &fake);
    app.settle_ai_for_test(10);
    assert!(fake.has_arg(1, &crate::smart::names_system()), "the built-in guidance, then the fixed part: {:?}", fake.argv(1));
    // With the user's own words, those replace the guidance and the fixed part still follows.
    let w = World::new("sm-own-names-2");
    simple(&w, "aaa", "please fix the login redirect", 300);
    smart(&w, |_| {});
    own_guidance(&w, NAMES, "Name each one like a ship.\nNo more than three words.\n");
    let fake = AiFake::new("sm-own-names-2");
    let mut app = app_with(&w, &fake);
    app.settle_ai_for_test(10);
    let want = compose("Name each one like a ship.\nNo more than three words.", crate::smart::NAMES_FIXED);
    assert!(fake.has_arg(1, &want), "{:?}", fake.argv(1));
    assert!(!fake.call_text_has(1, "two to six words"), "the built-in guidance is not sent as well");
}

#[test]
fn describe_it_sends_the_users_own_guidance_then_the_fixed_part() {
    use crate::instructions::{FIND, compose};
    let w = World::new("fd-own");
    many(&w, 4, "billing");
    smart(&w, |s| s.set_feature("names", false));
    own_guidance(&w, FIND, "Prefer the newest conversation when two match.");
    let fake = AiFake::new("fd-own");
    let mut app = app_with(&w, &fake);
    open_find(&mut app);
    type_str(&mut app, "the billing one");
    key(&mut app, KeyCode::Enter);
    app.settle_ai_for_test(10);
    assert_eq!(fake.calls(), 1);
    let want =
        compose("Prefer the newest conversation when two match.", &crate::smart::find_fixed(settings::DEFAULT_FIND_RESULTS as usize));
    assert!(fake.has_arg(1, &want), "{:?}", fake.argv(1));
    assert_eq!(state(&app), FindState::Done, "the answer is read as always");
}

#[test]
fn a_system_prompt_entry_opens_the_editor_list_on_enter_or_a_click_and_r_restores_only_a_changed_one() {
    let w = World::new("sm-own-keys");
    let mut app = w.app();
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let footer =
        |shown: &str| shown.lines().rev().find(|l| l.contains("esc back")).unwrap_or_else(|| panic!("no footer:\n{shown}")).to_string();
    // On an ordinary entry c and r do nothing.
    go(&mut app, "Update notes after");
    key(&mut app, KeyCode::Char('c'));
    key(&mut app, KeyCode::Char('r'));
    assert!(matches!(app.mode, Mode::Settings) && app.active_notice().is_none());
    // An unchanged prompt invites a change, and offers no restore: r and c do nothing on it.
    go(&mut app, "System prompt for project notes");
    let shown = screen(&mut term, &app);
    assert!(shown.lines().find(|l| l.contains("System prompt for project notes")).unwrap().contains("Enter to change"), "{shown}");
    assert!(footer(&shown).contains("enter edit") && !footer(&shown).contains("restore") && !shown.contains("compare"), "{shown}");
    for c in ['r', 'c'] {
        key(&mut app, KeyCode::Char(c));
        assert!(matches!(app.mode, Mode::Settings) && app.active_notice().is_none(), "{c} does nothing here");
    }
    // The arrows have nothing to step; enter opens the editor list, and esc comes back.
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Left);
    assert!(matches!(app.mode, Mode::Settings));
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::PickEditor) && app.epick.instructions == Some(crate::instructions::NOTES));
    let shown = screen(&mut term, &app);
    assert!(
        shown.contains("Edit the system prompt with") && shown.contains("System prompt for project notes"),
        "the settings stay behind it:\n{shown}"
    );
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::Settings), "esc goes back to the settings, not the list");
    // A click on the value does what enter does.
    screen(&mut term, &app);
    let value = app
        .hits
        .borrow()
        .iter()
        .find(|(_, h)| matches!(h, crate::app::Hit::SettingStep(i, settings::Step::Cycle) if *i == app.setting))
        .map(|(r, _)| *r)
        .unwrap();
    click_at(&mut app, value.x + 1, value.y);
    assert!(matches!(app.mode, Mode::PickEditor));
    key(&mut app, KeyCode::Esc);
    assert!(!w.config.join("prompts").exists(), "none of this wrote anything");

    // User Modified: the value says so, the footer offers restore, and c still does nothing.
    own_guidance(&w, crate::instructions::NOTES, "Keep the notes short.");
    let shown = screen(&mut term, &app);
    assert!(shown.lines().find(|l| l.contains("System prompt for project notes")).unwrap().contains("User Modified"), "{shown}");
    assert!(footer(&shown).contains("r restore"), "{shown}");
    key(&mut app, KeyCode::Char('c'));
    assert!(matches!(app.mode, Mode::Settings) && app.active_notice().is_none(), "no compare view");
    // r asks first; esc keeps the user's version.
    key(&mut app, KeyCode::Char('r'));
    assert!(matches!(app.mode, Mode::Confirm(crate::app::Confirm::ResetInstructions { id: "project-notes" })));
    let shown = screen(&mut term, &app);
    assert!(
        shown.contains("Restore platoon's default system prompt for project notes?") && shown.contains("project-notes.md.bak"),
        "{shown}"
    );
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::Settings));
    assert!(w.config.join("prompts/project-notes.md").exists());
    // y restores: the entry invites a change again, and the user's version is kept beside it.
    key(&mut app, KeyCode::Char('r'));
    key(&mut app, KeyCode::Char('y'));
    assert!(matches!(app.mode, Mode::Settings));
    assert_eq!(app.active_notice(), Some("Restored platoon's default system prompt for project notes."));
    assert!(!w.config.join("prompts/project-notes.md").exists());
    assert_eq!(fs::read_to_string(w.config.join("prompts/project-notes.md.bak")).unwrap(), "Keep the notes short.");
    let shown = screen(&mut term, &app);
    assert!(shown.lines().find(|l| l.contains("System prompt for project notes")).unwrap().contains("Enter to change"), "{shown}");
    assert!(!footer(&shown).contains("restore"), "{shown}");
}
