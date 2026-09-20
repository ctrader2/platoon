//! Scale checks with thousands of synthetic transcripts, some huge. The bounds are deliberately generous
//! (they run in an unoptimised test build, possibly alongside other tests): they catch a scan that reads
//! everything on every refresh, or state that grows without limit, not small regressions.

use crate::index::{Indexer, Scanner};
use crate::testkit::{iso, now_ms, World};
use ratatui::{backend::TestBackend, crossterm::event::{KeyCode, KeyEvent, KeyModifiers}, Terminal};
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
        (0..40).map(|k| format!("{}\n{}\n", line_assistant("/work/big", 40_000 + k, at), line_user("/work/big", "continue", at + 1000))).collect()
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
    assert!(app.visible().len() > 0 && app.visible().len() < 200, "{} rows match", app.visible().len());

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
