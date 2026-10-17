//! A persistent, incremental index of every saved conversation.
//!
//! With thousands of transcripts (some 10MB+) nothing may read them all on each refresh. The index
//! remembers what `claude::read_transcript` found for each file, keyed by path with the file's
//! (mtime, size). A scan lists the transcripts, compares those two numbers, and only re-reads files that
//! changed. It is saved to `<cache>/index.json` so the next start begins warm.
//!
//! Scanning runs on a background thread (`Indexer::spawn`) and streams results back in batches, newest
//! files first, so the list fills in while the UI is already usable.

use crate::claude::{Hidden, Transcript, read_transcript};
use serde_json::{Map, Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender, channel},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Bump when the parsing changes what an entry holds, so stale indexes are rebuilt instead of trusted.
/// 2: the added folders (`dirs`), which only a first reading of a file looks for in its head.
const VERSION: u64 = 2;
/// Results are sent to the UI in batches of this many files, or this often.
const BATCH: usize = 64;
const BATCH_EVERY: Duration = Duration::from_millis(60);
/// Rewriting the whole index on every change would cost more than it saves; a crash only loses this much.
const SAVE_EVERY: Duration = Duration::from_secs(20);
/// A scan that re-read at least this many files is saved at once (it's a cold or largely changed index).
const SAVE_NOW_PARSED: usize = 50;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Session id: the transcript's file name.
    pub id: String,
    pub path: PathBuf,
    /// Modified time in ms and size in bytes when it was read: the "has it changed" key.
    pub mtime: u64,
    pub size: u64,
    pub t: Transcript,
}

impl Entry {
    fn to_json(&self) -> Value {
        let t = &self.t;
        json!({
            "m": self.mtime, "s": self.size,
            "cwd": t.cwd, "dirs": t.extra_dirs, "branch": t.branch, "model": t.model, "tokens": t.context_tokens, "large": t.large_window,
            "title": t.ai_title, "prompt": t.first_prompt, "active": t.last_active_ms,
        })
    }

    fn from_json(path: &Path, v: &Value) -> Option<Self> {
        let text = |k: &str| v[k].as_str().map(String::from);
        Some(Entry {
            id: path.file_stem()?.to_str()?.to_string(),
            path: path.to_path_buf(),
            mtime: v["m"].as_u64()?,
            size: v["s"].as_u64()?,
            t: Transcript {
                cwd: text("cwd"),
                extra_dirs: v["dirs"].as_array().into_iter().flatten().filter_map(|d| d.as_str().map(String::from)).collect(),
                branch: text("branch"),
                model: text("model"),
                context_tokens: v["tokens"].as_u64().unwrap_or(0),
                large_window: v["large"].as_bool().unwrap_or(false),
                ai_title: text("title"),
                first_prompt: text("prompt"),
                last_active_ms: v["active"].as_u64(),
            },
        })
    }

    /// When the conversation last did anything: its newest entry, else the file's modified time.
    pub fn last_active_ms(&self) -> u64 {
        self.t.last_active_ms.unwrap_or(self.mtime)
    }
}

/// A batch of changes from a scan.
pub struct Update {
    pub upserts: Vec<Arc<Entry>>,
    pub removed: Vec<PathBuf>,
    /// Files this scan still has to read after this batch.
    pub remaining: usize,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Transcripts found on disk.
    pub listed: usize,
    /// Transcripts actually opened and read this scan.
    pub parsed: usize,
    /// Index entries dropped because the file is gone.
    pub removed: usize,
}

/// The scanning half: owns the persistent map and does the file work. Runs on whichever thread calls it.
pub struct Scanner {
    root: PathBuf,
    file: PathBuf,
    map: HashMap<PathBuf, Arc<Entry>>,
    unsaved: bool,
    /// None until this run has saved once; the first change is saved at once, later ones are throttled.
    last_save: Option<Instant>,
}

fn ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Scanner {
    /// Start from the saved index, if there is a usable one. A missing, corrupt or old-format file is
    /// just an empty index: everything gets read again.
    pub fn load(claude_root: &Path, cache: &Path) -> Self {
        let file = cache.join("index.json");
        let mut map = HashMap::new();
        let saved = fs::read_to_string(&file).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok());
        if let Some(v) = saved.filter(|v| v["version"].as_u64() == Some(VERSION)) {
            for (p, e) in v["files"].as_object().into_iter().flatten() {
                let path = PathBuf::from(p);
                if let Some(entry) = Entry::from_json(&path, e) {
                    map.insert(path, Arc::new(entry));
                }
            }
        }
        Self { root: claude_root.to_path_buf(), file, map, unsaved: false, last_save: None }
    }

    /// What was loaded from disk, for showing rows before the first scan finishes.
    pub fn entries(&self) -> Vec<Arc<Entry>> {
        self.map.values().cloned().collect()
    }

    /// Top-level `projects/*/<id>.jsonl` only. Subdirectories hold tool results, memory and subagent
    /// transcripts, none of which are conversations.
    fn list(&self) -> Vec<(PathBuf, u64, u64)> {
        let mut out = Vec::new();
        for dir in fs::read_dir(self.root.join("projects")).into_iter().flatten().flatten() {
            if !dir.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            for f in fs::read_dir(dir.path()).into_iter().flatten().flatten() {
                let path = f.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                let Ok(meta) = f.metadata() else { continue };
                if meta.is_file() {
                    out.push((path, meta.modified().map(ms).unwrap_or(0), meta.len()));
                }
            }
        }
        out
    }

    /// One pass: stat everything, read only what changed, send results through `emit` as they come
    /// (return false from it to abandon the scan), and save the index when it's worth it.
    pub fn scan(&mut self, emit: &mut dyn FnMut(Update) -> bool) -> Stats {
        let listing = self.list();
        let mut stats = Stats { listed: listing.len(), ..Stats::default() };

        let present: HashSet<&PathBuf> = listing.iter().map(|(p, _, _)| p).collect();
        let removed: Vec<PathBuf> = self.map.keys().filter(|p| !present.contains(p)).cloned().collect();
        for p in &removed {
            self.map.remove(p);
        }
        stats.removed = removed.len();

        // Newest first: what the user is most likely to want appears first.
        let mut changed: Vec<(PathBuf, u64, u64)> =
            listing.into_iter().filter(|(p, m, s)| !self.map.get(p).is_some_and(|e| e.mtime == *m && e.size == *s)).collect();
        changed.sort_by_key(|c| std::cmp::Reverse(c.1));

        if !removed.is_empty() {
            self.unsaved = true;
            if !emit(Update { upserts: Vec::new(), removed, remaining: changed.len() }) {
                return stats;
            }
        }
        let total = changed.len();
        let mut batch = Vec::new();
        let mut sent = Instant::now();
        for (i, (path, mtime, size)) in changed.into_iter().enumerate() {
            // (mtime, size) were taken before reading: a write during the read just triggers another pass.
            let prev = self.map.get(&path).map(|e| &e.t);
            stats.parsed += 1;
            if let (Some(t), Some(id)) = (read_transcript(&path, prev), path.file_stem().and_then(|s| s.to_str())) {
                let entry = Arc::new(Entry { id: id.to_string(), path: path.clone(), mtime, size, t });
                self.map.insert(path, entry.clone());
                batch.push(entry);
                self.unsaved = true;
            }
            if batch.len() >= BATCH || (!batch.is_empty() && sent.elapsed() >= BATCH_EVERY) {
                if !emit(Update { upserts: std::mem::take(&mut batch), removed: Vec::new(), remaining: total - i - 1 }) {
                    return stats;
                }
                sent = Instant::now();
            }
        }
        if !batch.is_empty() {
            emit(Update { upserts: batch, removed: Vec::new(), remaining: 0 });
        }
        if self.unsaved && (stats.parsed >= SAVE_NOW_PARSED || self.last_save.is_none_or(|t| t.elapsed() >= SAVE_EVERY)) {
            self.save();
        }
        stats
    }

    /// Write the index (temp file + rename, so a crash never leaves half a file).
    pub fn save(&mut self) {
        let mut files = Map::new();
        for (p, e) in &self.map {
            if let Some(p) = p.to_str() {
                files.insert(p.to_string(), e.to_json());
            }
        }
        let text = json!({ "version": VERSION, "files": files }).to_string();
        let tmp = self.file.with_extension("json.tmp");
        let ok = self.file.parent().is_none_or(|d| fs::create_dir_all(d).is_ok())
            && fs::write(&tmp, text).is_ok()
            && fs::rename(&tmp, &self.file).is_ok();
        if ok {
            self.unsaved = false;
            self.last_save = Some(Instant::now());
        }
    }
}

enum Backend {
    /// A worker thread scans; `wake` asks for a scan, `busy` says one is in progress.
    Thread { rx: Receiver<Update>, wake: Sender<()>, busy: Arc<AtomicBool> },
    /// Scans on the caller's thread, to completion, on each poll. For tests, which need determinism.
    #[cfg(test)]
    Inline(Scanner),
}

/// What the UI holds: the current entries plus a way to keep them fresh.
pub struct Indexer {
    backend: Backend,
    pub entries: HashMap<PathBuf, Arc<Entry>>,
    /// Bumped whenever `entries` changes.
    pub generation: u64,
    /// Transcripts the running scan still has to read.
    pub remaining: usize,
    /// The first scan of this run has finished and been taken in: `entries` now matches the disk, rather
    /// than what an older run saved. Anything that acts on how old a conversation is waits for this.
    pub first_scan_done: bool,
    /// Transcripts recorded in this folder are not conversations (platoon's own helper), so they never enter
    /// `entries`. The scanner still keeps them, so an unchanged file isn't read again on every pass.
    hidden: Option<Hidden>,
    /// Working folder -> is it hidden, so each distinct folder is looked at once.
    hidden_memo: HashMap<String, bool>,
}

impl Indexer {
    /// Load the saved index right away (so rows can show on the first frame) and scan in the background.
    pub fn spawn(claude_root: &Path, cache: &Path) -> Self {
        let mut scanner = Scanner::load(claude_root, cache);
        let entries = scanner.entries().into_iter().map(|e| (e.path.clone(), e)).collect();
        let (tx, rx) = channel::<Update>();
        let (wake, woken) = channel::<()>();
        let busy = Arc::new(AtomicBool::new(true)); // the first scan starts immediately
        let worker_busy = busy.clone();
        thread::spawn(move || {
            loop {
                let mut cancelled = false;
                scanner.scan(&mut |u| {
                    cancelled = tx.send(u).is_err();
                    !cancelled
                });
                worker_busy.store(false, Ordering::SeqCst);
                if cancelled || woken.recv().is_err() {
                    break;
                }
                while woken.try_recv().is_ok() {}
            }
        });
        Self {
            backend: Backend::Thread { rx, wake, busy },
            entries,
            generation: 1,
            remaining: 0,
            first_scan_done: false,
            hidden: None,
            hidden_memo: HashMap::new(),
        }
    }

    /// Scan on the calling thread on every poll.
    #[cfg(test)]
    pub fn inline(claude_root: &Path, cache: &Path) -> Self {
        let scanner = Scanner::load(claude_root, cache);
        let entries = scanner.entries().into_iter().map(|e| (e.path.clone(), e)).collect();
        Self {
            backend: Backend::Inline(scanner),
            entries,
            generation: 1,
            remaining: 0,
            first_scan_done: false,
            hidden: None,
            hidden_memo: HashMap::new(),
        }
    }

    /// Leave out every transcript whose working folder is (inside) `dir`, now and from here on.
    pub fn hide(&mut self, dir: Hidden) {
        self.hidden = Some(dir);
        self.hidden_memo.clear();
        let doomed: Vec<PathBuf> = self.entries.values().filter(|e| self.is_hidden(e)).map(|e| e.path.clone()).collect();
        if !doomed.is_empty() {
            for p in doomed {
                self.entries.remove(&p);
            }
            self.generation += 1;
        }
    }

    fn is_hidden(&self, e: &Entry) -> bool {
        let (Some(h), Some(cwd)) = (&self.hidden, &e.t.cwd) else { return false };
        self.hidden_memo.get(cwd).copied().unwrap_or_else(|| h.covers(cwd))
    }

    /// Take whatever the scan has produced, and make sure another scan follows. Cheap: it never reads
    /// a transcript itself. Returns true when the entries changed.
    pub fn poll(&mut self) -> bool {
        let before = self.generation;
        let mut updates = Vec::new();
        match &mut self.backend {
            Backend::Thread { rx, wake, busy } => {
                // Ask before draining: a scan that had already finished has sent everything we are about to take.
                let finished = !busy.load(Ordering::SeqCst);
                while let Ok(u) = rx.try_recv() {
                    updates.push(u);
                }
                self.first_scan_done |= finished;
                // Whenever the worker has finished, ask for the next scan: that is the once-a-second stat pass.
                if !busy.swap(true, Ordering::SeqCst) {
                    let _ = wake.send(());
                }
            }
            #[cfg(test)]
            Backend::Inline(scanner) => {
                scanner.scan(&mut |u| {
                    updates.push(u);
                    true
                });
                self.first_scan_done = true;
            }
        }
        for u in updates {
            self.remaining = u.remaining;
            for p in u.removed {
                if self.entries.remove(&p).is_some() {
                    self.generation += 1;
                }
            }
            for e in u.upserts {
                if let (Some(h), Some(cwd)) = (&self.hidden, &e.t.cwd) {
                    let hidden = *self.hidden_memo.entry(cwd.clone()).or_insert_with(|| h.covers(cwd));
                    if hidden {
                        if self.entries.remove(&e.path).is_some() {
                            self.generation += 1;
                        }
                        continue;
                    }
                }
                self.entries.insert(e.path.clone(), e);
                self.generation += 1;
            }
        }
        self.generation != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-index-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn user(text: &str, ts: &str) -> String {
        format!(
            r#"{{"type":"user","cwd":"/work/x","gitBranch":"main","timestamp":"{ts}","message":{{"role":"user","content":{}}}}}"#,
            json!(text)
        )
    }

    fn assistant(tokens: u64, ts: &str) -> String {
        format!(
            r#"{{"type":"assistant","cwd":"/work/x","gitBranch":"dev","timestamp":"{ts}","message":{{"model":"claude-sonnet-5","usage":{{"input_tokens":{tokens},"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}}}}"#
        )
    }

    fn write(root: &Path, dir: &str, id: &str, lines: &[String]) -> PathBuf {
        let d = root.join("projects").join(dir);
        fs::create_dir_all(&d).unwrap();
        let p = d.join(format!("{id}.jsonl"));
        fs::write(&p, lines.join("\n") + "\n").unwrap();
        p
    }

    fn scan(s: &mut Scanner) -> (Stats, Vec<Arc<Entry>>) {
        let mut got = Vec::new();
        let stats = s.scan(&mut |u| {
            got.extend(u.upserts);
            true
        });
        (stats, got)
    }

    #[test]
    fn reads_the_facts_out_of_a_transcript() {
        let (root, cache) = (tmp("facts-c"), tmp("facts-i"));
        write(
            &root,
            "-work-x",
            "abc",
            &[
                r#"{"type":"mode","mode":"normal"}"#.into(),
                r#"{"type":"user","isMeta":true,"cwd":"/work/x","message":{"content":"Caveat: injected"}}"#.into(),
                user("<command-name>/clear</command-name>", "2026-09-19T01:00:00.000Z"),
                user("fix the parser", "2026-09-19T01:00:05.000Z"),
                assistant(50_000, "2026-09-19T01:00:10.000Z"),
                r#"{"type":"ai-title","aiTitle":"Parser fix"}"#.into(),
                user("thanks", "2026-09-19T01:05:00.000Z"),
            ],
        );
        let mut s = Scanner::load(&root, &cache);
        let (stats, got) = scan(&mut s);
        assert_eq!(stats, Stats { listed: 1, parsed: 1, removed: 0 });
        let e = &got[0];
        assert_eq!(e.id, "abc");
        assert_eq!(e.t.cwd.as_deref(), Some("/work/x"));
        assert_eq!(e.t.first_prompt.as_deref(), Some("fix the parser"));
        assert_eq!(e.t.ai_title.as_deref(), Some("Parser fix"));
        assert_eq!((e.t.branch.as_deref(), e.t.model.as_deref(), e.t.context_tokens), (Some("main"), Some("claude-sonnet-5"), 50_000));
        assert_eq!(e.last_active_ms(), crate::claude::parse_iso_ms("2026-09-19T01:05:00.000Z").unwrap());
    }

    #[test]
    fn the_first_scan_is_reported_done_only_once_its_results_have_been_taken_in() {
        let (root, cache) = (tmp("first-c"), tmp("first-i"));
        for i in 0..5 {
            write(&root, "-work-x", &format!("id{i}"), &[user("hello", "2026-09-19T01:00:00.000Z")]);
        }
        // An older run's saved index describes conversations as they were then, not as they are now.
        let mut idx = Indexer::spawn(&root, &cache);
        assert!(!idx.first_scan_done, "nothing has been scanned yet");
        let start = Instant::now();
        while !idx.first_scan_done {
            assert!(start.elapsed() < Duration::from_secs(10), "the scan never finished");
            idx.poll();
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!((idx.entries.len(), idx.remaining), (5, 0), "done means every result is already in");
        let inline = {
            let mut i = Indexer::inline(&root, &cache);
            assert!(!i.first_scan_done);
            i.poll();
            i.first_scan_done
        };
        assert!(inline);
    }

    #[test]
    fn junk_conversations_are_indexed_but_have_no_prompt_and_subdirectories_are_ignored() {
        let (root, cache) = (tmp("junk-c"), tmp("junk-i"));
        write(
            &root,
            "-work-x",
            "junk",
            &[
                user("<command-name>/clear</command-name>", "2026-09-19T01:00:00.000Z"),
                r#"{"type":"user","cwd":"/w","message":{"content":""}}"#.into(),
            ],
        );
        write(&root, "-work-x", "empty", &[]);
        // Subagent and tool-result files live in subdirectories of the session's own folder.
        write(&root, "-work-x/junk/subagents", "agent-1", &[user("subagent prompt", "2026-09-19T01:00:00.000Z")]);
        let mut s = Scanner::load(&root, &cache);
        let (stats, got) = scan(&mut s);
        assert_eq!(stats.listed, 2, "only top-level transcripts");
        assert!(got.iter().all(|e| e.t.first_prompt.is_none()));
    }

    #[test]
    fn a_reopened_index_reads_nothing_again_and_only_changed_files_are_reread() {
        let (root, cache) = (tmp("reuse-c"), tmp("reuse-i"));
        let a = write(&root, "-p", "aaa", &[user("first", "2026-09-19T01:00:00.000Z")]);
        write(&root, "-p", "bbb", &[user("second", "2026-09-19T01:00:00.000Z")]);
        let mut s = Scanner::load(&root, &cache);
        assert_eq!(scan(&mut s).0.parsed, 2);
        s.save();

        // A new run (new Scanner) starts warm from index.json.
        let mut s = Scanner::load(&root, &cache);
        assert_eq!(s.entries().len(), 2);
        assert_eq!(scan(&mut s).0, Stats { listed: 2, parsed: 0, removed: 0 });
        assert_eq!(scan(&mut s).0.parsed, 0);

        // Appending to one file re-reads that file and nothing else.
        let mut f = fs::OpenOptions::new().append(true).open(&a).unwrap();
        writeln!(f, "{}", user("more", "2026-09-19T02:00:00.000Z")).unwrap();
        drop(f);
        let (stats, got) = scan(&mut s);
        assert_eq!((stats.parsed, got[0].id.as_str()), (1, "aaa"));
        assert_eq!(got[0].t.first_prompt.as_deref(), Some("first"), "the head facts survive a re-read");

        // A deleted file leaves the index.
        fs::remove_file(&a).unwrap();
        assert_eq!(scan(&mut s).0, Stats { listed: 1, parsed: 0, removed: 1 });
        assert_eq!(s.entries().len(), 1);
    }

    #[test]
    fn the_added_folders_are_kept_in_the_index_and_an_index_from_before_them_is_read_again() {
        let (root, cache) = (tmp("dirs-c"), tmp("dirs-i"));
        let env = |dirs: &str| {
            format!(
                r#"{{"type":"attachment","isSidechain":false,"attachment":{{"type":"environment","snapshot":{{"additionalWorkingDirectories":{dirs}}},"changes":null}}}}"#
            )
        };
        write(
            &root,
            "-p",
            "aaa",
            &[user("first", "2026-09-19T01:00:00.000Z"), env(r#"["/","/srv/data"]"#), assistant(10, "2026-09-19T01:00:01.000Z")],
        );
        write(&root, "-p", "bbb", &[user("second", "2026-09-19T01:00:00.000Z")]);
        let mut s = Scanner::load(&root, &cache);
        scan(&mut s);
        s.save();
        let dirs = |s: &Scanner, id: &str| s.entries().into_iter().find(|e| e.id == id).unwrap().t.extra_dirs.clone();
        let s = Scanner::load(&root, &cache);
        assert_eq!((dirs(&s, "aaa"), dirs(&s, "bbb")), (vec!["/".to_string(), "/srv/data".to_string()], vec![]), "a round trip keeps them");
        for e in s.entries() {
            let back = Entry::from_json(&e.path, &e.to_json()).unwrap();
            assert_eq!(&back, e.as_ref(), "every fact survives a round trip");
        }

        // An index written before the added folders were read (version 1) is not trusted: every file is read again,
        // so a conversation's start snapshot is found even though its head facts were known.
        let mut v: Value = serde_json::from_str(&fs::read_to_string(cache.join("index.json")).unwrap()).unwrap();
        *v.get_mut("version").unwrap() = json!(1);
        for (_, e) in v.get_mut("files").unwrap().as_object_mut().unwrap() {
            e.as_object_mut().unwrap().remove("dirs");
        }
        fs::write(cache.join("index.json"), v.to_string()).unwrap();
        let mut s = Scanner::load(&root, &cache);
        assert!(s.entries().is_empty());
        assert_eq!(scan(&mut s).0.parsed, 2);
        assert_eq!(dirs(&s, "aaa"), ["/", "/srv/data"]);
    }

    #[test]
    fn a_corrupt_or_old_index_is_rebuilt_not_trusted() {
        let (root, cache) = (tmp("bad-c"), tmp("bad-i"));
        write(&root, "-p", "aaa", &[user("first", "2026-09-19T01:00:00.000Z")]);
        for junk in ["{ not json", r#"{"version":0,"files":{}}"#, ""] {
            fs::write(cache.join("index.json"), junk).unwrap();
            let mut s = Scanner::load(&root, &cache);
            assert_eq!(scan(&mut s).0.parsed, 1, "{junk:?}");
        }
    }

    #[test]
    fn the_longest_text_is_capped_so_the_index_stays_small() {
        let (root, cache) = (tmp("cap-c"), tmp("cap-i"));
        write(&root, "-p", "aaa", &[user(&"word ".repeat(10_000), "2026-09-19T01:00:00.000Z")]);
        let mut s = Scanner::load(&root, &cache);
        let (_, got) = scan(&mut s);
        assert!(got[0].t.first_prompt.as_ref().unwrap().chars().count() <= 300);
    }

    #[test]
    fn a_background_indexer_fills_in_and_then_only_stats() {
        let (root, cache) = (tmp("bg-c"), tmp("bg-i"));
        for i in 0..200 {
            write(&root, "-p", &format!("id{i:03}"), &[user(&format!("prompt {i}"), "2026-09-19T01:00:00.000Z")]);
        }
        let mut ix = Indexer::spawn(&root, &cache);
        assert!(ix.entries.is_empty(), "nothing is read on the caller's thread");
        let start = Instant::now();
        while ix.entries.len() < 200 {
            ix.poll();
            assert!(start.elapsed() < Duration::from_secs(20), "only {} indexed", ix.entries.len());
            thread::sleep(Duration::from_millis(5));
        }
        // Once done and nothing changes, the generation stops moving.
        thread::sleep(Duration::from_millis(100));
        ix.poll();
        let g = ix.generation;
        for _ in 0..5 {
            thread::sleep(Duration::from_millis(30));
            ix.poll();
        }
        assert_eq!(ix.generation, g);
        // Reopened, the saved index gives rows on the very first frame.
        assert!(cache.join("index.json").exists());
        let again = Indexer::spawn(&root, &cache);
        assert_eq!(again.entries.len(), 200);
    }

    #[test]
    fn transcripts_recorded_in_a_hidden_folder_never_enter_the_entries_and_a_similar_folder_does() {
        let d = tmp("hidden");
        let (root, cache) = (d.join("claude"), d.join("cache"));
        let line = |cwd: &str| {
            format!(
                r#"{{"type":"user","cwd":"{cwd}","timestamp":"2026-09-19T03:53:40.091Z","message":{{"role":"user","content":"a real prompt"}}}}"#
            )
        };
        let work = cache.join("ai-work");
        fs::create_dir_all(&work).unwrap();
        write(&root, "-a", "helper", &[line(&work.display().to_string())]);
        write(&root, "-b", "near", &[line(&format!("{}-2", work.display()))]);
        write(&root, "-c", "real", &[line("/work/real")]);
        let ids = |ix: &Indexer| {
            let mut v: Vec<String> = ix.entries.values().map(|e| e.id.clone()).collect();
            v.sort();
            v
        };
        // Hidden after the saved index was loaded: purged at once.
        let mut ix = Indexer::inline(&root, &cache);
        ix.poll();
        assert_eq!(ids(&ix), ["helper", "near", "real"]);
        let before = ix.generation;
        ix.hide(Hidden::new(&work));
        assert_eq!(ids(&ix), ["near", "real"]);
        assert!(ix.generation > before, "the list is rebuilt");
        // Later scans, changed file included, keep it out.
        write(&root, "-a", "helper", &[line(&work.display().to_string()), line(&work.display().to_string())]);
        write(&root, "-d", "helper2", &[line(&work.join("x/..").display().to_string())]);
        ix.poll();
        assert_eq!(ids(&ix), ["near", "real"]);
        // A fresh indexer that reads the saved index (which knows all four files) hides them from the start.
        let mut again = Indexer::inline(&root, &cache);
        again.hide(Hidden::new(&work));
        again.poll();
        assert_eq!(ids(&again), ["near", "real"]);
        let _ = fs::remove_dir_all(&d);
    }
}
