//! Scale checks with thousands of synthetic transcripts, some huge. The bounds are deliberately generous
//! (they run in an unoptimised test build, possibly alongside other tests): they catch a scan that reads
//! everything on every refresh, or state that grows without limit, not small regressions.

use crate::index::{Indexer, Scanner};
use crate::testkit::{World, iso, now_ms};
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
};
use serde_json::json;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const CONVERSATIONS: usize = 5000;
const JUNK: usize = 50;

fn line_user(cwd: &str, text: &str, at: u64) -> String {
    json!({"type":"user","cwd":cwd,"gitBranch":"main","timestamp":iso(at),"message":{"role":"user","content":text}}).to_string()
}

fn line_assistant(cwd: &str, tokens: u64, at: u64) -> String {
    // Real assistant entries carry a lot of text; make these fat so big files are big.
    json!({"type":"assistant","cwd":cwd,"gitBranch":"main","timestamp":iso(at),"message":{"model":"claude-sonnet-5",
        "content":[{"type":"text","text":"lorem ipsum ".repeat(300)}],
        "usage":{"input_tokens":tokens,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}})
    .to_string()
}

/// About 150KB of realistic conversation, repeated to make big files cheaply.
fn block() -> &'static str {
    static BLOCK: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    BLOCK.get_or_init(|| {
        let at = now_ms() - 3_000_000;
        (0..40)
            .map(|k| format!("{}\n{}\n", line_assistant("/work/big", 40_000 + k, at), line_user("/work/big", "continue", at + 1000)))
            .collect()
    })
}

/// `n` transcripts under `root/projects/<100 folders>/`. Most are small; some are a megabyte or ten.
fn generate(root: &Path, n: usize) -> Vec<PathBuf> {
    let now = now_ms();
    let mut paths = Vec::new();
    for i in 0..n {
        let dir = root.join("projects").join(format!("-proj-{:03}", i % 100));
        if i < 100 {
            fs::create_dir_all(&dir).unwrap();
        }
        let path = dir.join(format!("{i:08}-aaaa-bbbb-cccc-dddddddddddd.jsonl"));
        let cwd = format!("/work/proj-{}", i % 100);
        let at = now - (i as u64) * 60_000;
        let mut f = std::io::BufWriter::new(fs::File::create(&path).unwrap());
        if i < JUNK {
            writeln!(f, "{}", line_user(&cwd, "<command-name>/clear</command-name>", at)).unwrap();
        } else {
            writeln!(f, "{}", line_user(&cwd, &format!("please work on task number {i}"), at - 5000)).unwrap();
            // 3 files of 10MB+, 20 of about 1MB, the rest a few KB.
            let repeats = match i {
                100..=102 => 72,
                _ if i % 250 == 7 => 7,
                _ => 0,
            };
            for _ in 0..repeats {
                f.write_all(block().as_bytes()).unwrap();
            }
            writeln!(f, "{}", line_assistant(&cwd, 90_000, at - 1000)).unwrap();
            writeln!(f, "{}", json!({"type":"ai-title","aiTitle":format!("Task {i}")})).unwrap();
            writeln!(f, "{}", line_user(&cwd, "thanks", at)).unwrap();
        }
        paths.push(path);
    }
    paths
}

#[cfg(target_os = "linux")]
fn rss_mb() -> u64 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok()))
        .unwrap_or(0)
        / 1024
}

#[cfg(not(target_os = "linux"))]
fn rss_mb() -> u64 {
    0
}

#[test]
fn five_thousand_conversations_index_once_then_refresh_by_statting_only() {
    let w = World::new("perf");
    let paths = generate(&w.root, CONVERSATIONS);
    let biggest = paths.iter().map(|p| fs::metadata(p).unwrap().len()).max().unwrap();
    assert!(biggest > 10 * 1024 * 1024, "the fixture must include a 10MB+ transcript, got {biggest}");

    // Cold: everything is read once (head and a bounded tail), never whole files.
    let t = Instant::now();
    let mut s = Scanner::load(&w.root, &w.cache);
    let cold = s.scan(&mut |_| true);
    let cold_time = t.elapsed();
    s.save();
    println!("cold index of {CONVERSATIONS}: {cold_time:?}");
    assert_eq!((cold.listed, cold.parsed), (CONVERSATIONS, CONVERSATIONS));
    assert!(cold_time < Duration::from_secs(60), "cold index took {cold_time:?}");
    // The index stays small however big the transcripts are (about a KB per conversation at most).
    let index_bytes = fs::metadata(w.cache.join("index.json")).unwrap().len();
    assert!(index_bytes < (CONVERSATIONS as u64) * 1500, "index.json is {index_bytes} bytes");

    // Warm, in a new process's shoes: load the saved index and refresh. Nothing is parsed.
    let t = Instant::now();
    let mut s = Scanner::load(&w.root, &w.cache);
    let load_time = t.elapsed();
    let warm = s.scan(&mut |_| true);
    let warm_time = t.elapsed();
    println!("warm start: load {load_time:?}, refresh {warm_time:?}");
    assert_eq!((warm.listed, warm.parsed, warm.removed), (CONVERSATIONS, 0, 0));
    assert!(warm_time < Duration::from_secs(5), "warm refresh took {warm_time:?}");

    // Repeated refreshes don't grow memory or the entry count.
    let before = rss_mb();
    for _ in 0..20 {
        assert_eq!(s.scan(&mut |_| true).parsed, 0);
    }
    assert_eq!(s.entries().len(), CONVERSATIONS);
    let grown = rss_mb().saturating_sub(before);
    assert!(grown < 100, "20 idle refreshes grew memory by {grown} MB");

    // A few files change (one of them the 10MB kind): exactly those are re-read.
    let mut touched = 0;
    for p in paths.iter().skip(200).step_by(1700).take(3) {
        let mut f = fs::OpenOptions::new().append(true).open(p).unwrap();
        writeln!(f, "{}", line_user("/work/x", "one more", now_ms())).unwrap();
        touched += 1;
    }
    let t = Instant::now();
    assert_eq!(s.scan(&mut |_| true).parsed, touched);
    assert!(t.elapsed() < Duration::from_secs(5));
}

#[test]
fn the_list_stays_responsive_with_five_thousand_rows() {
    let w = World::new("perf-ui");
    generate(&w.root, CONVERSATIONS);
    // A previous run's index, so this is the warm path the UI normally takes.
    let mut s = Scanner::load(&w.root, &w.cache);
    s.scan(&mut |_| true);
    s.save();

    let t = Instant::now();
    let mut app = w.app();
    let start = t.elapsed();
    assert_eq!(app.sessions.len(), CONVERSATIONS - JUNK, "junk hidden");
    assert_eq!(app.visible().len(), CONVERSATIONS - JUNK);
    assert_eq!(app.sessions[0].description, "Task 50", "newest first");

    // The once-a-second refresh: stat and compare, rebuild nothing when nothing changed.
    let t = Instant::now();
    for _ in 0..5 {
        app.refresh_for_test();
    }
    let refresh = t.elapsed() / 5;

    // Typing a filter is instant even so.
    let t = Instant::now();
    for c in ['/', 't', 'a', 's', 'k', ' ', '4', '9'] {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let typing = t.elapsed();
    assert!(!app.visible().is_empty() && app.visible().len() < 200, "{} rows match", app.visible().len());

    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let t = Instant::now();
    term.draw(|f| crate::ui::draw(f, &app)).unwrap();
    let frame = t.elapsed();
    println!("5000 rows: start {start:?}, refresh {refresh:?}, typing 8 keys {typing:?}, frame {frame:?}");
    assert!(start < Duration::from_secs(10), "start {start:?}");
    assert!(refresh < Duration::from_secs(2), "refresh {refresh:?}");
    assert!(typing < Duration::from_secs(2), "typing {typing:?}");
    assert!(frame < Duration::from_secs(1), "frame {frame:?}");
}

#[test]
fn a_background_index_of_thousands_fills_progressively_without_blocking_the_caller() {
    let w = World::new("perf-bg");
    generate(&w.root, 1500);
    let t = Instant::now();
    let mut ix = Indexer::spawn(&w.root, &w.cache);
    assert!(t.elapsed() < Duration::from_millis(500), "spawn must return at once, took {:?}", t.elapsed());
    while ix.entries.len() < 1500 {
        ix.poll();
        assert!(t.elapsed() < Duration::from_secs(60), "indexed {} of 1500", ix.entries.len());
        std::thread::sleep(Duration::from_millis(5));
    }
    // That rows arrive in batches is checked on the scanner itself. Watching for a half-filled index from here
    // depends on this thread being scheduled between two batches, which a busy machine does not promise.
    let mut batches = Vec::new();
    Scanner::load(&w.root, &w.base.join("cache-batches")).scan(&mut |u| {
        batches.push(u.upserts.len());
        true
    });
    assert!(batches.len() > 1 && batches[0] < 1500, "rows must arrive in batches, not all at the end: {batches:?}");
    assert_eq!(batches.iter().sum::<usize>(), 1500);
}

/// A user entry that carries a tool result of about `kb` KB: what most of a real transcript is made of.
fn tool_result_line(cwd: &str, kb: usize, at: u64) -> String {
    json!({"type":"user","cwd":cwd,"timestamp":iso(at),"toolUseResult":{},"message":{"role":"user","content":[{"tool_use_id":"toolu_1","type":"tool_result","content":"output line of a command\n".repeat(kb * 40)}]}}).to_string()
}

#[test]
fn the_prompt_index_reads_transcripts_once_then_only_new_bytes_and_a_search_stays_instant() {
    use crate::prompts::{self, PromptIndex, Store};
    let w = World::new("perf-prompts");
    let paths = generate(&w.root, CONVERSATIONS);
    // Real transcripts are mostly tool output: make the three huge files (and 200 more) that way, on top of the
    // fat assistant text of `generate`, and give 500 conversations a hundred typed prompts each so there is
    // something to search (about 15MB of prompt text).
    let now = now_ms();
    for p in paths.iter().skip(100).take(3) {
        let mut f = fs::OpenOptions::new().append(true).open(p).unwrap();
        for _ in 0..500 {
            writeln!(f, "{}", tool_result_line("/work/big", 20, now)).unwrap();
        }
    }
    let words = ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet", "kilo", "lima"];
    for (n, p) in paths.iter().skip(JUNK).take(500).enumerate() {
        let mut f = fs::OpenOptions::new().append(true).open(p).unwrap();
        for k in 0..100 {
            let text: Vec<&str> = (0..45).map(|j| words[(n * 7 + k * 3 + j * 5) % words.len()]).collect();
            writeln!(f, "{}", line_user("/work/x", &format!("{} marker{n}x{k}", text.join(" ")), now)).unwrap();
        }
    }
    let total: u64 = paths.iter().map(|p| fs::metadata(p).unwrap().len()).sum();

    // The conversation index, as the app would have it.
    let mut entries = Vec::new();
    Scanner::load(&w.root, &w.base.join("cache-index")).scan(&mut |u| {
        entries.extend(u.upserts);
        true
    });
    assert_eq!(entries.len(), CONVERSATIONS);

    // Cold: every transcript read once, each byte once.
    let t = Instant::now();
    let mut store = Store::load(&w.cache, 2000);
    let cold = store.scan(&entries, 2000, true, &mut |_| true);
    let cold_time = t.elapsed();
    store.save();
    let file = fs::metadata(w.cache.join("prompts.jsonl")).unwrap().len();
    println!(
        "prompt index, cold: {} transcripts, {} MB read in {cold_time:?}; prompts.jsonl is {} KB",
        cold.read,
        cold.bytes / 1_000_000,
        file / 1000
    );
    assert_eq!((cold.listed, cold.read, cold.rebuilt), (CONVERSATIONS, CONVERSATIONS - JUNK + JUNK, 0));
    assert!(cold.bytes <= total && cold.bytes > total - 1000 * CONVERSATIONS as u64, "each byte once: {} of {total}", cold.bytes);
    assert!(cold_time < Duration::from_secs(120), "cold prompt index took {cold_time:?}");
    assert!(file < 30 * 1024 * 1024, "prompts.jsonl is {file} bytes");
    // Claude's last words are found in the same pass, from the newest assistant text only: 200 characters of the
    // fat lorem-ipsum entry that ends each real conversation.
    let snapshot = store.snapshot();
    let with_reply = snapshot
        .iter()
        .filter(|f| {
            f.reply.as_ref().is_some_and(|r| r.starts_with("lorem ipsum") && r.chars().count() == crate::agents::claude::REPLY_CHARS)
        })
        .count();
    println!("prompt index: {with_reply} of {} conversations have a last reply", snapshot.len());
    assert_eq!(with_reply, snapshot.len(), "every conversation with prompts ends with a reply");

    // Warm, in a new process's shoes: load the saved file and check every file's size: nothing is read.
    let t = Instant::now();
    let mut store = Store::load(&w.cache, 2000);
    let load_time = t.elapsed();
    let warm = store.scan(&entries, 2000, true, &mut |_| true);
    println!("prompt index, warm: load {load_time:?}, pass {:?}, bytes read {}", t.elapsed() - load_time, warm.bytes);
    assert_eq!((warm.read, warm.bytes), (0, 0));
    assert!(t.elapsed() < Duration::from_secs(5), "warm prompt pass took {:?}", t.elapsed());

    // An append to one of the 10MB+ files reads only the appended bytes.
    let big = &paths[100];
    let before = fs::metadata(big).unwrap().len();
    assert!(before > 10 * 1024 * 1024);
    let mut f = fs::OpenOptions::new().append(true).open(big).unwrap();
    writeln!(f, "{}", line_user("/work/x", "one more thing about quebec", now_ms())).unwrap();
    writeln!(f, "{}", json!({"type":"assistant","timestamp":iso(now_ms()),"message":{"model":"claude-sonnet-5","content":[{"type":"text","text":"Newest words from Claude"}]}})).unwrap();
    drop(f);
    let after = fs::metadata(big).unwrap().len();
    let mut fresh = entries.clone();
    let stamp = |p: &Path| {
        std::sync::Arc::new(crate::index::Entry {
            mtime: fs::metadata(p).unwrap().modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64,
            size: fs::metadata(p).unwrap().len(),
            ..(**entries.iter().find(|e| e.path == *p).unwrap()).clone()
        })
    };
    let at = fresh.iter().position(|e| e.path == *big).unwrap();
    fresh[at] = stamp(big);
    let t = Instant::now();
    let mut got = Vec::new();
    let append = store.scan(&fresh, 2000, true, &mut |u| {
        got.extend(u.upserts);
        true
    });
    println!(
        "prompt index, one appended prompt and reply of a {} MB file: {:?}, {} bytes read",
        before / 1_000_000,
        t.elapsed(),
        append.bytes
    );
    assert_eq!((append.read, append.rebuilt, append.bytes), (1, 0, after - before));
    assert!(got[0].prompts.last().unwrap().contains("quebec"));
    assert_eq!(got[0].reply.as_deref(), Some("Newest words from Claude"), "the reply moved on by reading only the appended bytes");

    // Search over all of it, through the app, as typed: every key is a search.
    let mut ix = PromptIndex::inline(&w.cache, 2000);
    let map: std::collections::HashMap<_, _> = entries.iter().map(|e| (e.path.clone(), e.clone())).collect();
    ix.refresh(&map, 1, 2000, true);
    let (mut worst, mut hits) = (Duration::ZERO, 0);
    let t = Instant::now();
    for q in ["alpha bravo charlie", "marker7x9", "zzzz nothing", "kilo lima alpha delta echo", "task number 4999"] {
        let words = prompts::words(q);
        let t = Instant::now();
        let mut n = 0;
        for e in entries.iter() {
            n += usize::from(prompts::find(ix.get(&e.id).map(|d| &**d), "", &words).is_some());
        }
        worst = worst.max(t.elapsed());
        hits += n;
        println!("search {q:?}: {n} conversations in {:?}", t.elapsed());
    }
    println!("5 searches over {CONVERSATIONS} conversations: {:?} in all, slowest {worst:?}; {hits} hits", t.elapsed());
    assert!(hits > 0);
    assert!(worst < Duration::from_secs(2), "a search took {worst:?}");

    // And through the real dialog: typing eight keys into the word search over the same data.
    let mut app = w.app();
    let mut typing = Duration::ZERO;
    let mut press = |app: &mut crate::app::App, c: KeyCode| {
        let t = Instant::now();
        app.on_key(KeyEvent::new(c, KeyModifiers::NONE));
        typing = typing.max(t.elapsed());
    };
    press(&mut app, KeyCode::Char('/'));
    press(&mut app, KeyCode::Tab);
    for c in "alpha br".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    println!("word search dialog: {} results of {}, slowest key {typing:?}", app.search.results.len(), app.search.total);
    assert!(app.search.total > 0 && app.search.results.len() <= 50);
    assert!(typing < Duration::from_secs(2), "a key took {typing:?}");
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let t = Instant::now();
    term.draw(|f| crate::ui::draw(f, &app)).unwrap();
    assert!(t.elapsed() < Duration::from_secs(1), "frame {:?}", t.elapsed());
}

#[test]
fn a_background_prompt_index_never_blocks_the_caller_and_fills_in() {
    use crate::prompts::PromptIndex;
    let w = World::new("perf-prompts-bg");
    generate(&w.root, 1500);
    let mut entries = Vec::new();
    Scanner::load(&w.root, &w.base.join("cache-index")).scan(&mut |u| {
        entries.extend(u.upserts);
        true
    });
    let map: std::collections::HashMap<_, _> = entries.into_iter().map(|e| (e.path.clone(), e)).collect();
    let t = Instant::now();
    let mut ix = PromptIndex::spawn(&w.cache, 2000);
    assert!(t.elapsed() < Duration::from_millis(200), "spawn must return at once, took {:?}", t.elapsed());
    let t = Instant::now();
    ix.refresh(&map, 1, 2000, true);
    assert!(t.elapsed() < Duration::from_millis(200), "asking must return at once, took {:?}", t.elapsed());
    while ix.len() < 1500 - JUNK || ix.catching_up() {
        ix.refresh(&map, 1, 2000, true);
        assert!(t.elapsed() < Duration::from_secs(60), "indexed {} of {}", ix.len(), 1500 - JUNK);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_smart_scheduler_costs_next_to_nothing_when_a_big_history_is_all_named() {
    use crate::summaries::{Entry as Named, Summaries};
    let w = World::new("perf-smart");
    generate(&w.root, CONVERSATIONS);
    let entry = |i: usize| {
        (
            format!("{i:08}-aaaa-bbbb-cccc-dddddddddddd"),
            Named { name: format!("Task {i}"), summary: format!("Working on task number {i}."), covers: 1, model: "haiku".into(), at: 1 },
        )
    };
    let mut sums = Summaries::open(&w.config);
    let t = Instant::now();
    sums.set_many(&(JUNK..CONVERSATIONS).map(entry).collect::<Vec<_>>()).unwrap();
    println!(
        "summaries.json: {} entries written in {:?}, {} KB",
        CONVERSATIONS - JUNK,
        t.elapsed(),
        fs::metadata(w.config.join("summaries.json")).unwrap().len() / 1000
    );
    // One batch of thirty arriving in a file that size: what the app does after every call.
    let t = Instant::now();
    sums.set_many(&(JUNK..JUNK + 30).map(entry).collect::<Vec<_>>()).unwrap();
    let batch = t.elapsed();
    println!("summaries.json: one batch of 30 saved in {batch:?}");
    assert!(batch < Duration::from_secs(2), "saving a batch took {batch:?}");

    let mut settings = crate::settings::load(&w.config).0;
    settings.tokens_enabled = true;
    settings.set_feature("names", true);
    crate::settings::save(&w.config, &settings).unwrap();
    let fake = crate::testkit::AiFake::new("perf-smart");
    let mut app = w.app();
    app.set_ai_program_for_test(&fake.script);
    for _ in 0..3 {
        app.refresh_for_test();
    }
    let t = Instant::now();
    for _ in 0..10 {
        app.refresh_for_test();
    }
    let on = t.elapsed() / 10;
    settings.tokens_enabled = false;
    crate::settings::save(&w.config, &settings).unwrap();
    app.refresh_for_test();
    let t = Instant::now();
    for _ in 0..10 {
        app.refresh_for_test();
    }
    let off = t.elapsed() / 10;
    println!(
        "smart features, {CONVERSATIONS} conversations all named: a refresh takes {on:?} with them on, {off:?} off; calls made: {}",
        fake.calls()
    );
    assert_eq!(fake.calls(), 0, "everything is named: nothing to send");
    assert!(on < off + Duration::from_millis(500), "the scheduler added {:?} to a refresh", on.saturating_sub(off));
}

/// The archive key with something running the conversation: the key press returns without waiting for the process to
/// shut down (it takes 1.5 s here), with a handful of conversations and with thousands, and with nothing running it.
#[test]
fn archiving_never_waits_for_a_process_and_stays_quick_with_five_thousand_rows() {
    let key = |app: &mut crate::app::App, c: char| app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    let hosted =
        |pid| crate::host::HostInfo { claude_pid: pid, host_pid: 0, dir: "/tmp".into(), args: Vec::new(), sock: Default::default() };
    let limit = Duration::from_millis(500);

    // A few conversations, the process takes 1.5 s to exit.
    let mut w = World::new("perf-arch-small");
    w.transcript("aaa", "/srv/a", "one", 5);
    w.transcript("bbb", "/srv/b", "two", 50);
    let pid = w.live_slow_exit("aaa", "/srv/a", 1, "idle", 1.5);
    let mut app = w.app();
    app.refresh_with(vec![hosted(pid)], true);
    app.selected = app.visible().iter().position(|&i| app.sessions[i].id == "aaa").unwrap();
    let t = Instant::now();
    key(&mut app, 'a');
    let small_running = t.elapsed();
    assert_eq!(app.visible().len(), 1);
    assert!(app.settle_stops_for_test(Duration::from_secs(10)));

    // A few, nothing running.
    let w = World::new("perf-arch-none");
    w.transcript("aaa", "/srv/a", "one", 5);
    w.transcript("bbb", "/srv/b", "two", 50);
    let mut app = w.app();
    let t = Instant::now();
    key(&mut app, 'a');
    let small_idle = t.elapsed();

    // Thousands of rows: the same two cases, and the parts of the second one told apart.
    let mut w = World::new("perf-arch-big");
    generate(&w.root, CONVERSATIONS);
    let mut s = Scanner::load(&w.root, &w.cache);
    s.scan(&mut |_| true);
    s.save();
    let busy_id = "00000100-aaaa-bbbb-cccc-dddddddddddd";
    let pid = w.live_slow_exit(busy_id, "/work/proj-0", 1, "idle", 1.5);
    let mut app = w.app();
    app.refresh_with(vec![hosted(pid)], true);
    app.selected = app.visible().iter().position(|&i| app.sessions[i].id == busy_id).unwrap();
    let t = Instant::now();
    key(&mut app, 'a');
    let big_running = t.elapsed();
    assert_eq!(app.visible().len(), CONVERSATIONS - JUNK - 1);
    assert!(crate::testkit::alive(pid), "still shutting down when the key press has returned");
    assert!(app.settle_stops_for_test(Duration::from_secs(10)));
    let t = Instant::now();
    app.reload_for_test();
    let reload = t.elapsed();
    app.selected = 200;
    let t = Instant::now();
    key(&mut app, 'a');
    let big_idle = t.elapsed();
    assert_eq!(app.visible().len(), CONVERSATIONS - JUNK - 2);
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let t = Instant::now();
    term.draw(|f| crate::ui::draw(f, &app)).unwrap();
    let frame = t.elapsed();
    println!("archive key: {small_running:?} (a few rows, process 1.5 s to exit), {small_idle:?} (a few rows, nothing running)");
    println!(
        "archive key: {big_running:?} ({CONVERSATIONS} rows, process 1.5 s to exit), {big_idle:?} ({CONVERSATIONS} rows, nothing running; a reload alone {reload:?}, a frame {frame:?})"
    );
    for (what, took) in [("small running", small_running), ("small idle", small_idle), ("big running", big_running), ("big idle", big_idle)]
    {
        assert!(took < limit, "{what}: the archive key took {took:?}");
    }
}
