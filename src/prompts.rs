//! A local index of what the user typed, for searching the words of past conversations.
//!
//! Only the user's real prompts are kept (what `claude::prompt_in_line` accepts), which is a tiny part of a
//! transcript: on the user's own files 0.1 to 1%, the rest is tool output. Transcripts only ever grow at the end,
//! so each file is remembered with how many bytes of it have been read, and the next pass reads only the bytes
//! after that. A file that got shorter, or whose first bytes are no longer the same, was rewritten: it is read
//! again from the start. A last line that has no line break yet is left for the next pass (Claude ends every
//! line with one), so a line still being written is never read half.
//!
//! The work happens on a background thread (`PromptIndex::spawn`), like the conversation index, and never
//! blocks the UI. The result is saved to `<cache>/prompts.jsonl` (one line per file, so a damaged line costs one
//! file, not everything), which is separate from `index.json` so that one stays small and quick to load. A missing,
//! corrupt or old-format file is simply an empty index that fills again. Nothing is written anywhere else.
//!
//! The in-memory side keeps, per conversation, its prompts and one lower-case copy of them for searching:
//! a search is a substring scan over that text.

use crate::claude::{REPLY_CHARS, may_be_reply, prompt_in_line, reply_in_line};
use crate::index::Entry;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{BufWriter, Read, Seek, SeekFrom, Write},
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{Receiver, Sender, channel},
    },
    thread,
    time::{Duration, Instant},
};

/// Bump when what the file holds changes meaning, so an old one is rebuilt instead of trusted.
const VERSION: u64 = 2;
/// How much of a file is read at a time.
const CHUNK: usize = 1 << 20;
/// How many of a file's first bytes tell whether it is still the file that was read before.
const FINGERPRINT_BYTES: u64 = 512;
/// Results reach the UI in batches of this many files, or this often.
const BATCH: usize = 64;
const BATCH_EVERY: Duration = Duration::from_millis(60);
/// The whole file is rewritten on a save, so a change is saved at most this often (a crash only loses that much,
/// and what is lost is read again from the transcripts).
const SAVE_EVERY: Duration = Duration::from_secs(30);
/// A pass that read at least this many files is saved at once: it was a cold or largely changed index.
const SAVE_NOW_READ: usize = 50;
/// The saved file is handed to the UI in pieces of this many files.
const LOAD_BATCH: usize = 500;

/// What the user typed in one conversation, as the UI holds it.
#[derive(Debug, PartialEq)]
pub struct FilePrompts {
    /// Session id: the transcript's file name.
    pub id: String,
    pub path: PathBuf,
    /// The prompts in the order they were written, one line each, cut to the configured length.
    pub prompts: Vec<String>,
    /// The same, lower-cased, one per line (prompts never contain a line break), for searching.
    lower: String,
    /// The first `REPLY_CHARS` characters of the last thing Claude said in words in this conversation (no tool
    /// output, no subagents). One line of context for the smart names.
    pub reply: Option<String>,
    /// How many of `prompts` are substantive (see `smart::is_substantive`): not "yes" or "continue".
    pub substantive: usize,
}

impl FilePrompts {
    fn new(id: String, path: PathBuf, prompts: Vec<String>, reply: Option<String>) -> Self {
        let lower = prompts.iter().map(|p| p.to_lowercase()).collect::<Vec<_>>().join("\n");
        let substantive = prompts.iter().filter(|p| crate::smart::is_substantive(p)).count();
        Self { id, path, prompts, lower, reply, substantive }
    }

    /// This conversation with `more` prompts added at the end, and Claude's newest reply if a newer one was found.
    fn extended(&self, more: Vec<String>, reply: Option<String>) -> Self {
        let mut prompts = self.prompts.clone();
        let mut lower = self.lower.clone();
        let mut substantive = self.substantive;
        for p in more {
            if !prompts.is_empty() {
                lower.push('\n');
            }
            lower.push_str(&p.to_lowercase());
            substantive += usize::from(crate::smart::is_substantive(&p));
            prompts.push(p);
        }
        Self { id: self.id.clone(), path: self.path.clone(), prompts, lower, reply: reply.or_else(|| self.reply.clone()), substantive }
    }

    /// All the prompts lower-cased, one per line, for searching.
    pub fn lower_text(&self) -> &str {
        &self.lower
    }

    /// A conversation for tests of other modules.
    #[cfg(test)]
    pub fn for_test(id: &str, prompts: &[&str], reply: Option<&str>) -> Self {
        Self::new(
            id.into(),
            PathBuf::from(format!("/x/{id}.jsonl")),
            prompts.iter().map(|s| s.to_string()).collect(),
            reply.map(String::from),
        )
    }

    /// The lower-cased prompts, one per line, aligned with `prompts`.
    fn lines(&self) -> impl Iterator<Item = &str> {
        self.lower.split('\n').take(self.prompts.len())
    }
}

/// What a scan remembers about one transcript.
struct FileState {
    mtime: u64,
    size: u64,
    /// Bytes read so far: always the end of a complete line.
    offset: u64,
    /// How many of the file's first bytes `fingerprint` covers (at most `FINGERPRINT_BYTES`, at most `offset`).
    fp_len: u64,
    fingerprint: u64,
    data: Arc<FilePrompts>,
}

/// FNV-1a: the same on every run and every version, which `DefaultHasher` does not promise.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

fn head_hash(file: &mut File, len: u64) -> Option<u64> {
    let mut buf = vec![0u8; len as usize];
    file.seek(SeekFrom::Start(0)).ok()?;
    file.read_exact(&mut buf).ok()?;
    Some(fnv(&buf))
}

/// The result of reading one transcript.
struct Read1 {
    state: FileState,
    /// Bytes of the transcript that were read.
    bytes: u64,
    /// It was read from the start although it was known before: it had been rewritten.
    rebuilt: bool,
    /// The UI needs the new data: prompts were added, or the file was read afresh.
    changed: bool,
}

/// Bring one file up to date, reading only what is new when that is safe.
fn read_file(path: &Path, id: &str, mtime: u64, size: u64, cap: usize, prev: Option<&FileState>) -> Option<Read1> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut rebuilt = false;
    let keep = match prev {
        Some(p) if p.offset <= len && (p.fp_len == 0 || head_hash(&mut file, p.fp_len) == Some(p.fingerprint)) => Some(p),
        Some(_) => {
            rebuilt = true;
            None
        }
        None => None,
    };
    let start = keep.map_or(0, |p| p.offset);
    file.seek(SeekFrom::Start(start)).ok()?;

    let mut prompts: Vec<String> = Vec::new();
    // The newest line that could be Claude's words, kept as text and parsed once, after the last chunk: the rest of
    // the file's assistant lines (most of it) are never parsed.
    let mut reply_line: Option<String> = None;
    let mut consumed = 0u64;
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        let old = pending.len();
        pending.extend_from_slice(&buf[..n]);
        // Only whole lines are taken: the last one may still be being written.
        let Some(last) = buf[..n].iter().rposition(|&b| b == b'\n') else { continue };
        let end = old + last + 1;
        let chunk = String::from_utf8_lossy(&pending[..end]);
        let mut newest: Option<&str> = None;
        for line in chunk.split('\n') {
            // A user entry is never an assistant one, so the second test only runs on the rest.
            if line.contains("\"type\":\"user\"") {
                if let Some(p) = prompt_in_line(line, cap) {
                    prompts.push(p);
                }
            } else if may_be_reply(line) {
                newest = Some(line);
            }
        }
        if let Some(l) = newest {
            reply_line = Some(l.to_string());
        }
        consumed += end as u64;
        pending.drain(..end);
    }

    let offset = start + consumed;
    let (fp_len, fingerprint) = match keep {
        Some(p) if p.fp_len >= FINGERPRINT_BYTES.min(offset) => (p.fp_len, p.fingerprint),
        _ => {
            let n = offset.min(FINGERPRINT_BYTES);
            (n, if n == 0 { 0 } else { head_hash(&mut file, n)? })
        }
    };
    let reply = reply_line.and_then(|l| reply_in_line(&l, REPLY_CHARS));
    let (data, changed) = match keep {
        Some(p) if prompts.is_empty() && reply.as_ref().is_none_or(|r| p.data.reply.as_ref() == Some(r)) => (p.data.clone(), false),
        Some(p) => (Arc::new(p.data.extended(prompts, reply)), true),
        None => {
            let data = Arc::new(FilePrompts::new(id.to_string(), path.to_path_buf(), prompts, reply));
            // A file seen for the first time with nothing in it is not news; one read again from the start is (it may
            // have had prompts before).
            let changed = prev.is_some() || !data.prompts.is_empty();
            (data, changed)
        }
    };
    Some(Read1 { state: FileState { mtime, size, offset, fp_len, fingerprint, data }, bytes: consumed, rebuilt, changed })
}

/// One batch of changes from a scan.
pub struct Update {
    pub upserts: Vec<Arc<FilePrompts>>,
    pub removed: Vec<PathBuf>,
    /// Files this scan still has to read after this batch.
    pub remaining: usize,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Transcripts the scan was asked about.
    pub listed: usize,
    /// Transcripts opened and read (from their saved position, or from the start).
    pub read: usize,
    /// Of those, the ones read from the start although they had been read before: they were rewritten.
    pub rebuilt: usize,
    /// Bytes of transcript actually read.
    pub bytes: u64,
    /// Files dropped because the transcript is gone.
    pub removed: usize,
}

/// The working half: owns what is known about every transcript and does the file work, on whichever thread
/// calls it.
pub struct Store {
    file: PathBuf,
    /// The longest prompt kept, in characters. What was saved with another value is thrown away.
    cap: usize,
    files: HashMap<PathBuf, FileState>,
    unsaved: bool,
    /// When the file was last written or loaded. None (nothing was loaded, so nothing is saved) means the first pass
    /// is saved at once; after that a save waits `SAVE_EVERY`.
    last_save: Option<Instant>,
}

impl Store {
    /// Start from the saved file when it is usable. A missing, corrupt or old-format file, or one made with another
    /// prompt length, is an empty store. A damaged line loses only its own file.
    pub fn load(cache: &Path, cap: usize) -> Self {
        let file = cache.join("prompts.jsonl");
        let mut files = HashMap::new();
        if let Ok(text) = fs::read_to_string(&file) {
            let mut lines = text.lines();
            let header = lines.next().and_then(|l| serde_json::from_str::<Value>(l).ok());
            if header.is_some_and(|h| h["version"].as_u64() == Some(VERSION) && h["cap"].as_u64() == Some(cap as u64)) {
                for line in lines {
                    if let Some((path, state)) = serde_json::from_str::<Value>(line).ok().and_then(|v| state_from_json(&v)) {
                        files.insert(path, state);
                    }
                }
            }
        }
        // Starting from a saved file, the first small change waits its turn like the rest: rewriting all of it for one
        // new prompt would be the price of every start. Starting from nothing, the first pass is saved at once.
        let last_save = (!files.is_empty()).then(Instant::now);
        Self { file, cap, files, unsaved: false, last_save }
    }

    /// What was loaded, for the UI to search before the first scan is done.
    pub fn snapshot(&self) -> Vec<Arc<FilePrompts>> {
        self.files.values().filter(|s| !s.data.prompts.is_empty()).map(|s| s.data.clone()).collect()
    }

    /// One pass over `entries` (the conversations the index knows, with the size and time they had when it read
    /// them; `prune`: that is every conversation there is, so a file not in it is gone): read what is new, send results through `emit` as they come (return false from it to give up), and
    /// save when it is worth it.
    pub fn scan(&mut self, entries: &[Arc<Entry>], cap: usize, prune: bool, emit: &mut dyn FnMut(Update) -> bool) -> Stats {
        let mut stats = Stats { listed: entries.len(), ..Stats::default() };
        if cap != self.cap {
            self.files.clear();
            self.cap = cap;
            self.unsaved = true;
        }
        let present: HashSet<&PathBuf> = entries.iter().map(|e| &e.path).collect();
        // A partial list (the conversation index is still reading) says nothing about what is gone.
        let removed: Vec<PathBuf> = if prune { self.files.keys().filter(|p| !present.contains(p)).cloned().collect() } else { Vec::new() };
        for p in &removed {
            self.files.remove(p);
        }
        stats.removed = removed.len();

        // Newest first: what the user is most likely to search for appears first.
        let mut changed: Vec<&Arc<Entry>> =
            entries.iter().filter(|e| !self.files.get(&e.path).is_some_and(|s| s.mtime == e.mtime && s.size == e.size)).collect();
        changed.sort_by(|a, b| b.mtime.cmp(&a.mtime));

        if !removed.is_empty() {
            self.unsaved = true;
            if !emit(Update { upserts: Vec::new(), removed, remaining: changed.len() }) {
                return stats;
            }
        }
        let total = changed.len();
        let mut batch = Vec::new();
        let mut sent = Instant::now();
        for (i, e) in changed.into_iter().enumerate() {
            if let Some(r) = read_file(&e.path, &e.id, e.mtime, e.size, self.cap, self.files.get(&e.path)) {
                stats.read += 1;
                stats.bytes += r.bytes;
                stats.rebuilt += usize::from(r.rebuilt);
                if r.changed {
                    batch.push(r.state.data.clone());
                }
                self.unsaved = true;
                self.files.insert(e.path.clone(), r.state);
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
        if self.unsaved && (stats.read >= SAVE_NOW_READ || self.last_save.is_none_or(|t| t.elapsed() >= SAVE_EVERY)) {
            self.save();
        }
        stats
    }

    /// Write the file (temp file + rename, so a crash never leaves half a file).
    pub fn save(&mut self) {
        let tmp = self.file.with_extension("jsonl.tmp");
        let write = || -> std::io::Result<()> {
            if let Some(d) = self.file.parent() {
                fs::create_dir_all(d)?;
            }
            let mut out = BufWriter::new(File::create(&tmp)?);
            writeln!(out, "{}", json!({ "version": VERSION, "cap": self.cap }))?;
            for (path, s) in &self.files {
                let Some(path) = path.to_str() else { continue };
                let v = json!({ "p": path, "m": s.mtime, "s": s.size, "o": s.offset, "n": s.fp_len, "h": s.fingerprint, "t": s.data.prompts, "r": s.data.reply });
                writeln!(out, "{v}")?;
            }
            out.flush()?;
            fs::rename(&tmp, &self.file)
        };
        if write().is_ok() {
            self.unsaved = false;
            self.last_save = Some(Instant::now());
        } else {
            let _ = fs::remove_file(&tmp);
        }
    }
}

fn state_from_json(v: &Value) -> Option<(PathBuf, FileState)> {
    let path = PathBuf::from(v["p"].as_str()?);
    let id = path.file_stem()?.to_str()?.to_string();
    let prompts: Vec<String> = v["t"].as_array()?.iter().map(|p| p.as_str().map(String::from)).collect::<Option<_>>()?;
    let (offset, fp_len) = (v["o"].as_u64()?, v["n"].as_u64()?);
    if fp_len > offset.min(FINGERPRINT_BYTES) {
        return None;
    }
    let state = FileState {
        mtime: v["m"].as_u64()?,
        size: v["s"].as_u64()?,
        offset,
        fp_len,
        fingerprint: v["h"].as_u64()?,
        data: Arc::new(FilePrompts::new(id, path.clone(), prompts, v["r"].as_str().map(String::from))),
    };
    Some((path, state))
}

/// What the UI asks the worker to bring up to date.
struct Request {
    seq: u64,
    entries: Vec<Arc<Entry>>,
    cap: usize,
    prune: bool,
}

enum Msg {
    Batch(Update),
    /// The saved file has been handed over.
    Loaded,
    /// The request with this number has been dealt with completely.
    Done(u64),
}

enum Backend {
    Thread {
        requests: Sender<Request>,
        rx: Receiver<Msg>,
    },
    /// Works on the caller's thread, to completion, when asked. For tests, which need determinism.
    #[cfg(test)]
    Inline(Store),
}

/// What the UI holds: the prompts of every conversation, kept fresh in the background.
pub struct PromptIndex {
    backend: Backend,
    by_path: HashMap<PathBuf, Arc<FilePrompts>>,
    /// Usually one file per conversation; the same id can be saved under two project folders.
    by_id: HashMap<String, Vec<Arc<FilePrompts>>>,
    /// The index generation, prompt length and whether the list was complete that the last request was made for.
    asked: Option<(u64, usize, bool)>,
    seq: u64,
    loaded: bool,
    /// Transcripts the running pass still has to read.
    pub remaining: usize,
    /// Bumped whenever the prompts change.
    pub generation: u64,
}

impl PromptIndex {
    /// Nothing is read on the calling thread: the worker loads the saved file and hands it over, then waits for
    /// `refresh` to tell it which transcripts there are.
    pub fn spawn(cache: &Path, cap: usize) -> Self {
        let (requests, wanted) = channel::<Request>();
        let (tx, rx) = channel::<Msg>();
        let cache = cache.to_path_buf();
        thread::spawn(move || {
            let mut store = Store::load(&cache, cap);
            let all = store.snapshot();
            for chunk in all.chunks(LOAD_BATCH) {
                if tx.send(Msg::Batch(Update { upserts: chunk.to_vec(), removed: Vec::new(), remaining: 0 })).is_err() {
                    return;
                }
            }
            drop(all);
            if tx.send(Msg::Loaded).is_err() {
                return;
            }
            while let Ok(mut req) = wanted.recv() {
                // Only the latest request matters: it lists everything.
                while let Ok(newer) = wanted.try_recv() {
                    req = newer;
                }
                let mut gone = false;
                store.scan(&req.entries, req.cap, req.prune, &mut |u| {
                    gone = tx.send(Msg::Batch(u)).is_err();
                    !gone
                });
                if gone || tx.send(Msg::Done(req.seq)).is_err() {
                    return;
                }
            }
        });
        Self::with(Backend::Thread { requests, rx }, false)
    }

    /// Works on the calling thread on every `refresh`.
    #[cfg(test)]
    pub fn inline(cache: &Path, cap: usize) -> Self {
        let store = Store::load(cache, cap);
        let all = store.snapshot();
        let mut me = Self::with(Backend::Inline(store), true);
        for f in all {
            me.insert(f);
        }
        me
    }

    fn with(backend: Backend, loaded: bool) -> Self {
        Self { backend, by_path: HashMap::new(), by_id: HashMap::new(), asked: None, seq: 0, loaded, remaining: 0, generation: 1 }
    }

    /// Ask for the prompts of `entries` (the conversation index at `generation`) to be brought up to date, if that
    /// is news since last time (`complete`: the conversation index has read everything, so a file missing from it is gone), and take in whatever the worker has produced. Cheap: it never reads a transcript
    /// itself. True when the prompts changed.
    pub fn refresh(&mut self, entries: &HashMap<PathBuf, Arc<Entry>>, generation: u64, cap: usize, complete: bool) -> bool {
        let before = self.generation;
        if self.asked != Some((generation, cap, complete)) {
            self.asked = Some((generation, cap, complete));
            self.seq += 1;
            let list: Vec<Arc<Entry>> = entries.values().cloned().collect();
            match &mut self.backend {
                Backend::Thread { requests, .. } => {
                    let _ = requests.send(Request { seq: self.seq, entries: list, cap, prune: complete });
                }
                #[cfg(test)]
                Backend::Inline(store) => {
                    let mut updates = Vec::new();
                    store.scan(&list, cap, complete, &mut |u| {
                        updates.push(u);
                        true
                    });
                    for u in updates {
                        self.apply(u);
                    }
                }
            }
        }
        let mut msgs = Vec::new();
        #[allow(irrefutable_let_patterns)] // (in release builds the thread is the only backend)
        if let Backend::Thread { rx, .. } = &self.backend {
            while let Ok(m) = rx.try_recv() {
                msgs.push(m);
            }
        }
        for m in msgs {
            match m {
                Msg::Batch(u) => self.apply(u),
                Msg::Loaded => self.loaded = true,
                Msg::Done(seq) if seq == self.seq => self.remaining = 0,
                Msg::Done(_) => {}
            }
        }
        self.generation != before
    }

    fn apply(&mut self, u: Update) {
        self.remaining = u.remaining;
        for p in u.removed {
            if let Some(old) = self.by_path.remove(&p) {
                self.forget(&old);
                self.generation += 1;
            }
        }
        for f in u.upserts {
            self.insert(f);
        }
    }

    fn forget(&mut self, old: &Arc<FilePrompts>) {
        if let Some(v) = self.by_id.get_mut(&old.id) {
            v.retain(|f| f.path != old.path);
            if v.is_empty() {
                self.by_id.remove(&old.id);
            }
        }
    }

    fn insert(&mut self, f: Arc<FilePrompts>) {
        if let Some(old) = self.by_path.insert(f.path.clone(), f.clone()) {
            self.forget(&old);
        }
        if f.prompts.is_empty() {
            // A file that was emptied out: nothing to search, and nothing to keep.
            self.by_path.remove(&f.path);
        } else {
            self.by_id.entry(f.id.clone()).or_default().push(f);
        }
        self.generation += 1;
    }

    /// True while the index is still catching up on a large amount of reading (a cold start, or many changed
    /// files), so a search may not see everything yet.
    pub fn catching_up(&self) -> bool {
        !self.loaded || self.remaining > 0
    }

    /// What the user typed in this conversation. The same id under two project folders gives the longer record.
    pub fn get(&self, id: &str) -> Option<&Arc<FilePrompts>> {
        self.by_id.get(id)?.iter().max_by_key(|f| f.prompts.len())
    }

    /// How many conversations have prompts kept.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.by_path.len()
    }
}

/// One conversation that matched a search.
#[derive(Debug, PartialEq, Eq)]
pub struct Found {
    /// Every word is in the conversation's name.
    pub in_name: bool,
    /// One single prompt has every word (rather than the words being spread over several).
    pub in_one_prompt: bool,
    /// How many prompts have at least one of the words.
    pub hits: usize,
    /// The prompt to show: the one with the most of the words, the latest of those. None when only the name matched.
    pub prompt: Option<usize>,
}

/// Does the conversation match all of `words` (already lower-cased, at least one)? A word matches inside a
/// longer word, and a conversation matches when each word is somewhere in what the user typed or in the name the
/// user gave it, not necessarily in the same place.
#[cfg(test)]
pub fn find(data: Option<&FilePrompts>, name_lower: &str, words: &[String]) -> Option<Found> {
    find_with(data, name_lower, "", words)
}

/// `find`, also looking in the summary platoon generated for the conversation (`summary_lower`, already lower-cased):
/// a word in it counts like a word in a prompt, and one in the name counts as the name having it.
pub fn find_with(data: Option<&FilePrompts>, name_lower: &str, summary_lower: &str, words: &[String]) -> Option<Found> {
    if words.is_empty() {
        return None;
    }
    let in_name = words.iter().all(|w| name_lower.contains(w.as_str()));
    let text = data.map_or("", |d| d.lower.as_str());
    if !words.iter().all(|w| name_lower.contains(w.as_str()) || summary_lower.contains(w.as_str()) || text.contains(w.as_str())) {
        return None;
    }
    let (mut hits, mut in_one_prompt, mut best) = (0, false, None::<(usize, usize)>);
    for (i, line) in data.into_iter().flat_map(FilePrompts::lines).enumerate() {
        let n = words.iter().filter(|w| line.contains(w.as_str())).count();
        if n == 0 {
            continue;
        }
        hits += 1;
        in_one_prompt |= n == words.len();
        if best.is_none_or(|(most, _)| n >= most) {
            best = Some((n, i));
        }
    }
    Some(Found { in_name, in_one_prompt, hits, prompt: best.map(|(_, i)| i) })
}

/// The lower-case words of a search text.
pub fn words(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// Where the words are in `text`, as ranges of characters (not bytes), overlapping ones merged.
pub fn spans(text: &str, words: &[String]) -> Vec<Range<usize>> {
    // Lower-casing can change a character's length: keep, for every byte of the lower-case text, the character
    // of the original it came from.
    let (mut low, mut from) = (String::new(), Vec::new());
    for (i, c) in text.chars().enumerate() {
        for l in c.to_lowercase() {
            let before = low.len();
            low.push(l);
            from.extend(std::iter::repeat_n(i, low.len() - before));
        }
    }
    let mut out: Vec<Range<usize>> = Vec::new();
    for w in words.iter().filter(|w| !w.is_empty()) {
        for (at, _) in low.match_indices(w.as_str()) {
            out.push(from[at]..from[at + w.len() - 1] + 1);
        }
    }
    out.sort_by_key(|r| (r.start, r.end));
    let mut merged: Vec<Range<usize>> = Vec::new();
    for r in out {
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    merged
}

/// A piece of `text` about `width` characters long that shows its first match, with `…` where it was cut, and
/// where the matches fall in it (character ranges of the returned text).
pub fn snippet(text: &str, spans: &[Range<usize>], width: usize) -> (String, Vec<Range<usize>>) {
    let chars: Vec<char> = text.chars().collect();
    let (len, width) = (chars.len(), width.max(1));
    if len <= width {
        return (text.to_string(), spans.to_vec());
    }
    // Start a little before the first match so it has something around it, but never so early that the end of the
    // text would leave room unused.
    let first = spans.first().map_or(0, |r| r.start);
    let mut at = first.saturating_sub(width / 4);
    if len - at <= width {
        at = len - width;
    }
    let left = at > 0;
    let from = (at + usize::from(left)).min(first);
    let to = (from + width - usize::from(left)).min(len);
    let right = to < len;
    let to = if right { to.saturating_sub(1).max(from) } else { to };
    let mut out = String::new();
    if left {
        out.push('…');
    }
    out.extend(&chars[from..to]);
    if right {
        out.push('…');
    }
    let shift = |i: usize| i - from + usize::from(left);
    let marks = spans.iter().filter(|r| r.end > from && r.start < to).map(|r| shift(r.start.max(from))..shift(r.end.min(to))).collect();
    (out, marks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-prompts-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn user(text: &str) -> String {
        json!({"type":"user","cwd":"/w","timestamp":"2026-09-19T01:00:00.000Z","message":{"role":"user","content":text}}).to_string()
    }

    fn entry(path: &Path) -> Arc<Entry> {
        let meta = fs::metadata(path).unwrap();
        let mtime = meta.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        Arc::new(Entry {
            id: path.file_stem().unwrap().to_str().unwrap().to_string(),
            path: path.to_path_buf(),
            mtime,
            size: meta.len(),
            t: Default::default(),
        })
    }

    fn write(dir: &Path, id: &str, lines: &[String]) -> PathBuf {
        let p = dir.join(format!("{id}.jsonl"));
        fs::write(&p, lines.join("\n") + "\n").unwrap();
        p
    }

    fn append(p: &Path, lines: &[String]) {
        let mut f = fs::OpenOptions::new().append(true).open(p).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
    }

    fn scan(s: &mut Store, paths: &[&Path]) -> (Stats, Vec<Arc<FilePrompts>>) {
        let entries: Vec<Arc<Entry>> = paths.iter().map(|p| entry(p)).collect();
        let cap = s.cap;
        let mut got = Vec::new();
        let stats = s.scan(&entries, cap, true, &mut |u| {
            got.extend(u.upserts);
            true
        });
        (stats, got)
    }

    #[test]
    fn only_real_typed_prompts_are_kept() {
        let d = tmp("rules");
        let p = write(
            &d,
            "abc",
            &[
                r#"{"type":"mode","mode":"normal"}"#.into(),
                user("fix the parser"),
                user("<command-name>/clear</command-name>"),
                user("[Request interrupted by user]"),
                json!({"type":"user","isMeta":true,"message":{"content":"Caveat: injected"}}).to_string(),
                json!({"type":"user","isSidechain":true,"message":{"content":"subagent prompt"}}).to_string(),
                json!({"type":"user","isCompactSummary":true,"message":{"content":"This session is being continued"}}).to_string(),
                json!({"type":"user","message":{"role":"user","content":[{"tool_use_id":"t1","type":"tool_result","content":"the file says hello"}]},"toolUseResult":{}}).to_string(),
                json!({"type":"assistant","message":{"content":[{"type":"text","text":"an answer"}]}}).to_string(),
                json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":"second  thing\nwith lines"}]}}).to_string(),
                "{ broken json \"type\":\"user\"".into(),
            ],
        );
        let mut s = Store::load(&d, 2000);
        let (stats, got) = scan(&mut s, &[&p]);
        assert_eq!((stats.listed, stats.read), (1, 1));
        assert_eq!(got[0].prompts, ["fix the parser", "second thing with lines"]);
        assert_eq!(got[0].id, "abc");
    }

    #[test]
    fn a_prompt_that_talks_about_tool_results_is_still_a_prompt() {
        let d = tmp("talk");
        let p = write(&d, "abc", &[user(r#"why does {"type":"tool_result"} show up, and "isSidechain":true too?"#)]);
        let (_, got) = scan(&mut Store::load(&d, 2000), &[&p]);
        assert_eq!(got[0].prompts.len(), 1, "the words are inside a string, so they are escaped and never look like the real thing");
    }

    #[test]
    fn a_prompt_is_cut_to_the_configured_length() {
        let d = tmp("cap");
        let p = write(&d, "abc", &[user(&"word ".repeat(1000)), user("short")]);
        let (_, got) = scan(&mut Store::load(&d, 50), &[&p]);
        assert_eq!(got[0].prompts[0].chars().count(), 50);
        assert_eq!(got[0].prompts[1], "short");
        let (_, got) = scan(&mut Store::load(&tmp("cap2"), 4000), &[&p]);
        assert!(got[0].prompts[0].chars().count() > 900, "a longer setting keeps more");
    }

    #[test]
    fn an_appended_file_reads_only_the_new_bytes_and_keeps_what_it_had() {
        let d = tmp("append");
        let p = write(&d, "abc", &[user("first prompt"), "x".repeat(200_000)]);
        let other = write(&d, "def", &[user("another")]);
        let mut s = Store::load(&d, 2000);
        let (stats, _) = scan(&mut s, &[&p, &other]);
        let first_len = fs::metadata(&p).unwrap().len();
        assert_eq!(stats.bytes, first_len + fs::metadata(&other).unwrap().len(), "the first pass reads everything once");
        assert_eq!(scan(&mut s, &[&p, &other]).0, Stats { listed: 2, ..Stats::default() }, "unchanged files are not even opened");

        let new = vec![user("second prompt"), "y".repeat(5000)];
        append(&p, &new);
        let added = fs::metadata(&p).unwrap().len() - first_len;
        let (stats, got) = scan(&mut s, &[&p, &other]);
        assert_eq!((stats.read, stats.rebuilt, stats.bytes), (1, 0, added), "only the bytes after the saved position");
        assert_eq!(got[0].prompts, ["first prompt", "second prompt"]);
        assert_eq!(got[0].lower, "first prompt\nsecond prompt");
    }

    #[test]
    fn a_line_still_being_written_is_read_once_it_is_complete_and_not_twice() {
        let d = tmp("partial");
        let p = d.join("abc.jsonl");
        let line = user("half written");
        let (a, b) = line.split_at(20);
        fs::write(&p, format!("{}\n{a}", user("whole"))).unwrap();
        let mut s = Store::load(&d, 2000);
        assert_eq!(scan(&mut s, &[&p]).1[0].prompts, ["whole"]);
        let mut f = fs::OpenOptions::new().append(true).open(&p).unwrap();
        writeln!(f, "{b}").unwrap();
        drop(f);
        let (_, got) = scan(&mut s, &[&p]);
        assert_eq!(got[0].prompts, ["whole", "half written"]);
        // Touching the file without adding anything reads nothing new and sends nothing.
        append(&p, &[]);
        let (stats, got) = scan(&mut s, &[&p]);
        assert!(got.is_empty() && stats.bytes == 0, "{stats:?}");
    }

    #[test]
    fn a_file_that_got_shorter_or_starts_differently_is_read_again_from_the_start() {
        let d = tmp("rewrite");
        let p = write(&d, "abc", &[user("original one"), user("original two"), user("original three")]);
        let mut s = Store::load(&d, 2000);
        scan(&mut s, &[&p]);

        // Shorter.
        fs::write(&p, user("rewritten") + "\n").unwrap();
        let (stats, got) = scan(&mut s, &[&p]);
        assert_eq!((stats.read, stats.rebuilt), (1, 1));
        assert_eq!(got[0].prompts, ["rewritten"]);

        // Longer, but the beginning is not the same file any more.
        fs::write(&p, [user("something else entirely"), user("with more"), user("and more still")].join("\n") + "\n").unwrap();
        let (stats, got) = scan(&mut s, &[&p]);
        assert_eq!((stats.read, stats.rebuilt), (1, 1), "same or bigger size is no proof of an append");
        assert_eq!(got[0].prompts, ["something else entirely", "with more", "and more still"]);

        // And a file emptied out leaves no prompts, told to the UI as a change.
        fs::write(&p, "").unwrap();
        let (_, got) = scan(&mut s, &[&p]);
        assert!(got[0].prompts.is_empty());
    }

    #[test]
    fn a_removed_transcript_leaves_the_store() {
        let d = tmp("removed");
        let (a, b) = (write(&d, "aaa", &[user("one")]), write(&d, "bbb", &[user("two")]));
        let mut s = Store::load(&d, 2000);
        scan(&mut s, &[&a, &b]);
        let entries = vec![entry(&a)];
        let mut gone = Vec::new();
        let stats = s.scan(&entries, 2000, true, &mut |u| {
            gone.extend(u.removed);
            true
        });
        assert_eq!((stats.removed, gone), (1, vec![b]));
        assert_eq!(s.files.len(), 1);
    }

    #[test]
    fn a_partial_list_never_removes_anything_but_a_complete_one_does() {
        let d = tmp("partial-list");
        let (a, b) = (write(&d, "aaa", &[user("one")]), write(&d, "bbb", &[user("two")]));
        let mut s = Store::load(&d, 2000);
        scan(&mut s, &[&a, &b]);
        // The conversation index is still reading and only knows `a` so far: `b` is not gone.
        let stats = s.scan(&[entry(&a)], 2000, false, &mut |_| true);
        assert_eq!((stats.removed, s.files.len()), (0, 2));
        let stats = s.scan(&[entry(&a)], 2000, true, &mut |_| true);
        assert_eq!((stats.removed, s.files.len()), (1, 1));
    }

    #[test]
    fn a_saved_store_is_picked_up_again_and_the_next_pass_reads_only_what_was_added() {
        let (d, cache) = (tmp("saved-d"), tmp("saved-c"));
        let p = write(&d, "abc", &[user("kept across runs")]);
        let mut s = Store::load(&cache, 2000);
        scan(&mut s, &[&p]);
        s.save();
        assert!(!cache.join("prompts.jsonl.tmp").exists() && !cache.join("index.json").exists(), "its own file, renamed into place");

        let mut s = Store::load(&cache, 2000);
        assert_eq!(s.snapshot().len(), 1);
        assert_eq!(scan(&mut s, &[&p]).0, Stats { listed: 1, ..Stats::default() }, "warm: nothing is read");
        append(&p, &[user("added later")]);
        let (stats, got) = scan(&mut s, &[&p]);
        assert_eq!((stats.rebuilt, got[0].prompts.clone()), (0, vec!["kept across runs".to_string(), "added later".to_string()]));
        assert!(stats.bytes < 400, "{stats:?}");
    }

    #[test]
    fn a_different_prompt_length_a_missing_file_or_a_corrupt_one_starts_over_without_complaint() {
        let (d, cache) = (tmp("bad-d"), tmp("bad-c"));
        let p = write(&d, "abc", &[user("hello there")]);
        let mut s = Store::load(&cache, 2000);
        scan(&mut s, &[&p]);
        s.save();

        assert_eq!(Store::load(&cache, 500).snapshot().len(), 0, "made for another length");
        for junk in ["", "not json at all", "{\"version\":0,\"cap\":2000}\n", "{\"version\":1}\n", "\u{0}\u{0}\u{0}", "[1,2,3]"] {
            fs::write(cache.join("prompts.jsonl"), junk).unwrap();
            let mut s = Store::load(&cache, 2000);
            assert_eq!(s.snapshot().len(), 0, "{junk:?}");
            assert_eq!(scan(&mut s, &[&p]).0.read, 1, "{junk:?}: read again");
        }
        fs::remove_file(cache.join("prompts.jsonl")).unwrap();
        assert_eq!(Store::load(&cache, 2000).snapshot().len(), 0);

        // A damaged line costs only that file.
        let q = write(&d, "def", &[user("second file")]);
        let mut s = Store::load(&cache, 2000);
        scan(&mut s, &[&p, &q]);
        s.save();
        let text = fs::read_to_string(cache.join("prompts.jsonl")).unwrap();
        let mut lines: Vec<String> = text.lines().map(String::from).collect();
        assert_eq!(lines.len(), 3);
        lines[2] = lines[2][..lines[2].len() / 2].to_string();
        fs::write(cache.join("prompts.jsonl"), lines.join("\n")).unwrap();
        assert_eq!(Store::load(&cache, 2000).snapshot().len(), 1);
    }

    #[test]
    fn a_saved_line_that_does_not_add_up_is_ignored_rather_than_trusted() {
        let (d, cache) = (tmp("lie-d"), tmp("lie-c"));
        let p = write(&d, "abc", &[user("hello there")]);
        let path = p.to_str().unwrap();
        let head = "{\"version\":1,\"cap\":2000}\n";
        // A fingerprint longer than what was read, wrong types, and a missing field.
        for line in [
            json!({"p":path,"m":1,"s":1,"o":10,"n":600,"h":1,"t":["x"]}),
            json!({"p":path,"m":1,"s":1,"o":10,"n":5,"h":1,"t":[1]}),
            json!({"p":path,"m":1,"s":1,"o":10,"n":5,"t":["x"]}),
            json!({"p":path,"m":"1","s":1,"o":10,"n":5,"h":1,"t":["x"]}),
        ] {
            fs::write(cache.join("prompts.jsonl"), format!("{head}{line}\n")).unwrap();
            assert_eq!(Store::load(&cache, 2000).snapshot().len(), 0, "{line}");
        }
    }

    #[test]
    fn saving_only_touches_the_cache_directory() {
        let (d, cache) = (tmp("only-d"), tmp("only-c"));
        let p = write(&d, "abc", &[user("hello")]);
        let before: Vec<_> = fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name()).collect();
        let mut s = Store::load(&cache, 2000);
        scan(&mut s, &[&p]);
        s.save();
        let after: Vec<_> = fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(before, after, "the transcripts' folder is untouched");
        let names: Vec<String> = fs::read_dir(&cache).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["prompts.jsonl"]);
    }

    fn data(prompts: &[&str]) -> FilePrompts {
        FilePrompts::new("id".into(), PathBuf::from("/x/id.jsonl"), prompts.iter().map(|s| s.to_string()).collect(), None)
    }

    #[test]
    fn a_search_is_case_insensitive_and_needs_every_word_in_any_order() {
        let d = data(&["Fix the Docker build", "add a migration for users"]);
        let w = words;
        assert!(find(Some(&d), "", &w("DOCKER")).is_some());
        assert!(find(Some(&d), "", &w("build docker")).is_some(), "any order");
        assert!(find(Some(&d), "", &w("docker migration")).is_some(), "the words may be in different prompts");
        assert!(find(Some(&d), "", &w("docker kubernetes")).is_none(), "every word is needed");
        assert!(find(Some(&d), "", &w("migrat")).is_some(), "inside a longer word");
        assert!(find(Some(&d), "", &w("")).is_none() && find(Some(&d), "", &w("   ")).is_none(), "nothing typed matches nothing");
        assert!(find(None, "", &w("docker")).is_none(), "a conversation with no prompts and no name");
    }

    #[test]
    fn a_name_the_user_gave_counts_like_prompt_text() {
        let d = data(&["fix the build"]);
        let f = find(Some(&d), "payments work", &words("payments build")).unwrap();
        assert!(!f.in_name && !f.in_one_prompt, "one word from the name, one from a prompt");
        assert_eq!(f.prompt, Some(0));
        let only_name = find(Some(&d), "payments work", &words("Payments")).unwrap();
        assert!(only_name.in_name && only_name.prompt.is_none());
        assert!(find(None, "payments work", &words("payments")).is_some(), "even with nothing typed yet");
    }

    #[test]
    fn what_is_found_says_how_well_it_matches_and_which_prompt_to_show() {
        let d = data(&["docker is slow", "unrelated", "docker and kubernetes together", "docker again"]);
        let f = find(Some(&d), "", &words("docker kubernetes")).unwrap();
        assert!(f.in_one_prompt);
        assert_eq!((f.hits, f.prompt), (3, Some(2)), "the prompt with the most of the words");
        let f = find(Some(&d), "", &words("docker")).unwrap();
        assert_eq!((f.hits, f.prompt), (3, Some(3)), "of equally good prompts, the latest");
        let spread = data(&["only docker here", "only kubernetes here"]);
        assert!(!find(Some(&spread), "", &words("docker kubernetes")).unwrap().in_one_prompt);
    }

    #[test]
    fn matches_are_marked_by_characters_even_after_lowercasing_changes_lengths() {
        assert_eq!(spans("Fix the Docker build", &words("docker")), [8..14]);
        assert_eq!(spans("aa bb aa", &words("aa")), [0..2, 6..8]);
        assert_eq!(spans("foobar", &words("foo bar")), [0..6], "touching matches merge");
        assert_eq!(spans("abcdef", &words("bcd cde")), [1..5], "overlapping matches merge");
        // Ü is one character either way; İ lower-cases to two.
        assert_eq!(spans("ÄÖ Über", &words("über")), [3..7]);
        assert_eq!(spans("İstanbul city", &words("city")), [9..13]);
        assert_eq!(spans("nothing", &words("zzz")), Vec::<Range<usize>>::new());
    }

    #[test]
    fn a_snippet_shows_the_match_and_says_where_it_was_cut() {
        let text = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa NEEDLE bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let sp = spans(text, &words("needle"));
        let (short, marks) = snippet(text, &sp, 200);
        assert_eq!(short, text, "fits: untouched");
        assert_eq!(marks, sp);
        let (cut, marks) = snippet(text, &sp, 30);
        assert!(cut.starts_with('…') && cut.ends_with('…') && cut.chars().count() <= 30, "{cut:?}");
        let shown: String = cut.chars().skip(marks[0].start).take(marks[0].len()).collect();
        assert_eq!(shown, "NEEDLE", "{cut:?} {marks:?}");
        // A match at the start is not preceded by an ellipsis; one at the end not followed by one.
        let at_start = "needle then a very long tail that goes on and on and on and on and on and on";
        let (cut, marks) = snippet(at_start, &spans(at_start, &words("needle")), 20);
        assert!(!cut.starts_with('…') && cut.ends_with('…') && marks == [0..6] && cut.chars().count() <= 20, "{cut:?} {marks:?}");
        let at_end = "a very long lead that goes on and on and on and on and on and on then needle";
        let (cut, marks) = snippet(at_end, &spans(at_end, &words("needle")), 20);
        let shown: String = cut.chars().skip(marks[0].start).take(marks[0].len()).collect();
        assert!(cut.starts_with('…') && !cut.ends_with('…') && shown == "needle" && cut.chars().count() <= 20, "{cut:?} {marks:?}");
        // Tiny widths never panic.
        for w in 0..8 {
            let _ = snippet(text, &sp, w);
        }
    }

    #[test]
    fn the_index_fills_in_the_background_and_stays_in_step_with_the_conversation_index() {
        let (d, cache) = (tmp("live-d"), tmp("live-c"));
        let mut entries = HashMap::new();
        for i in 0..30 {
            let p = write(&d, &format!("id{i:02}"), &[user(&format!("prompt number {i}"))]);
            entries.insert(p.clone(), entry(&p));
        }
        let mut ix = PromptIndex::spawn(&cache, 2000);
        assert!(ix.catching_up(), "nothing has been read yet");
        assert_eq!(ix.len(), 0, "nothing is read on the caller's thread");
        let start = Instant::now();
        while ix.len() < 30 || ix.catching_up() {
            ix.refresh(&entries, 1, 2000, true);
            assert!(start.elapsed() < Duration::from_secs(20), "only {} indexed", ix.len());
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(ix.get("id07").unwrap().prompts, ["prompt number 7"]);
        assert!(ix.get("nope").is_none());
        // A change to the conversation index (a new generation) brings the new prompt in.
        let p = d.join("id07.jsonl");
        append(&p, &[user("a later prompt")]);
        entries.insert(p.clone(), entry(&p));
        while ix.get("id07").unwrap().prompts.len() < 2 {
            ix.refresh(&entries, 2, 2000, true);
            assert!(start.elapsed() < Duration::from_secs(20));
            thread::sleep(Duration::from_millis(5));
        }
        // And a removed transcript goes.
        entries.remove(&p);
        while ix.get("id07").is_some() {
            ix.refresh(&entries, 3, 2000, true);
            assert!(start.elapsed() < Duration::from_secs(20));
            thread::sleep(Duration::from_millis(5));
        }
        // Reopened, the saved file gives the prompts without the conversation index having said anything.
        thread::sleep(Duration::from_millis(50));
        let mut again = PromptIndex::spawn(&cache, 2000);
        let start = Instant::now();
        while again.len() < 29 {
            again.refresh(&HashMap::new(), 0, 2000, false);
            assert!(start.elapsed() < Duration::from_secs(20), "warm start gave {}", again.len());
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_new_prompt_length_reads_everything_again() {
        let (d, cache) = (tmp("recap-d"), tmp("recap-c"));
        let p = write(&d, "abc", &[user(&"long ".repeat(200))]);
        let mut entries = HashMap::new();
        entries.insert(p.clone(), entry(&p));
        let mut ix = PromptIndex::inline(&cache, 2000);
        ix.refresh(&entries, 1, 2000, true);
        assert_eq!(ix.get("abc").unwrap().prompts[0].chars().count(), 999, "a trailing space is trimmed");
        ix.refresh(&entries, 1, 100, true);
        assert_eq!(ix.get("abc").unwrap().prompts[0].chars().count(), 100);
    }

    // ---- Claude's last reply ----

    fn say(text: &str) -> String {
        json!({"type":"assistant","cwd":"/w","timestamp":"2026-09-19T01:00:00.000Z","message":{"model":"claude-sonnet-5","content":[{"type":"text","text":text}]}}).to_string()
    }

    fn tool_call() -> String {
        json!({"type":"assistant","message":{"model":"claude-sonnet-5","content":[{"type":"tool_use","name":"Bash","input":{"command":"echo SECRET-CALL"}}]}}).to_string()
    }

    fn tool_output() -> String {
        json!({"type":"user","message":{"role":"user","content":[{"tool_use_id":"t","type":"tool_result","content":"SECRET-OUTPUT"}]}})
            .to_string()
    }

    #[test]
    fn a_conversations_last_reply_is_the_newest_thing_claude_said_in_words_and_nothing_else() {
        let d = tmp("reply");
        let p = write(
            &d,
            "abc",
            &[
                user("fix the parser"),
                say("First answer."),
                tool_call(),
                tool_output(),
                say("The parser is fixed.\nAll tests pass."),
                tool_call(),
                tool_output(),
                json!({"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"SECRET-SUBAGENT"}]}}).to_string(),
                json!({"type":"assistant","message":{"model":"<synthetic>","content":[{"type":"text","text":"SECRET-SYNTHETIC"}]}})
                    .to_string(),
                json!({"type":"assistant","message":{"content":[{"type":"thinking","thinking":"SECRET-THINKING"}]}}).to_string(),
            ],
        );
        let (_, got) = scan(&mut Store::load(&d, 2000), &[&p]);
        assert_eq!(got[0].reply.as_deref(), Some("The parser is fixed. All tests pass."));
        assert_eq!(got[0].prompts, ["fix the parser"]);
        // Cut to REPLY_CHARS.
        let p2 = write(&d, "long", &[user("hello there"), say(&"w".repeat(1000))]);
        let (_, got) = scan(&mut Store::load(&tmp("reply2"), 2000), &[&p2]);
        assert_eq!(got[0].reply.as_ref().map(|r| r.chars().count()), Some(REPLY_CHARS));
        // No reply at all: None, and the prompts are still kept.
        let p3 = write(&d, "mute", &[user("anyone there")]);
        let (_, got) = scan(&mut Store::load(&tmp("reply3"), 2000), &[&p3]);
        assert_eq!((got[0].reply.clone(), got[0].prompts.len()), (None, 1));
    }

    #[test]
    fn only_the_newest_candidate_line_is_parsed_so_an_older_reply_is_never_dug_out() {
        let d = tmp("reply-newest");
        // The newest line that looks like Claude's words is damaged: it is the only one looked at, and it says nothing.
        let broken = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"cut off"#.to_string();
        let p = write(&d, "abc", &[user("hello there"), say("An old answer."), broken]);
        let (_, got) = scan(&mut Store::load(&d, 2000), &[&p]);
        assert_eq!(got[0].reply, None, "older lines are not parsed at all");
    }

    #[test]
    fn the_reply_follows_appended_lines_reading_only_the_new_bytes() {
        let d = tmp("reply-append");
        let p = write(&d, "abc", &[user("first prompt"), say("Old answer."), "x".repeat(300_000)]);
        let mut s = Store::load(&d, 2000);
        let (_, got) = scan(&mut s, &[&p]);
        assert_eq!(got[0].reply.as_deref(), Some("Old answer."));
        // A newer answer arrives: one line read, the reply moves on, the prompts are the same.
        let new = vec![say("Newer answer.")];
        append(&p, &new);
        let (stats, got) = scan(&mut s, &[&p]);
        assert_eq!(stats.bytes, (new[0].len() + 1) as u64, "only the appended bytes were read");
        assert_eq!((got[0].reply.as_deref(), got[0].prompts.len()), (Some("Newer answer."), 1));
        // A tool call and its output are not words: nothing changes, and nothing is announced.
        append(&p, &[tool_call(), tool_output()]);
        let (stats, got) = scan(&mut s, &[&p]);
        assert!(stats.bytes > 0 && got.is_empty(), "read, but there is nothing new to say");
        // A new prompt keeps the reply that was there.
        append(&p, &[user("second prompt")]);
        let (_, got) = scan(&mut s, &[&p]);
        assert_eq!((got[0].reply.as_deref(), got[0].prompts.len()), (Some("Newer answer."), 2));
        // The same reply again (a repeated line) changes nothing.
        append(&p, &[say("Newer answer.")]);
        let (_, got) = scan(&mut s, &[&p]);
        assert!(got.is_empty());
    }

    #[test]
    fn the_reply_is_saved_and_an_index_of_the_old_version_is_thrown_away() {
        let d = tmp("reply-save");
        let p = write(&d, "abc", &[user("please fix the parser"), say("Done.")]);
        let mut s = Store::load(&d, 2000);
        scan(&mut s, &[&p]);
        s.save();
        let text = fs::read_to_string(d.join("prompts.jsonl")).unwrap();
        assert!(
            text.starts_with(&format!("{{\"cap\":2000,\"version\":{VERSION}"))
                || text.lines().next().unwrap().contains(&format!("\"version\":{VERSION}")),
            "{text}"
        );
        assert!(VERSION >= 2, "the reply made the saved format different");
        let again = Store::load(&d, 2000);
        assert_eq!(again.snapshot()[0].reply.as_deref(), Some("Done."));
        assert_eq!(again.snapshot()[0].substantive, 1);
        // A file from before the reply was kept (version 1) is not trusted: it is an empty index that fills again.
        let old = text.replacen(&format!("\"version\":{VERSION}"), "\"version\":1", 1);
        fs::write(d.join("prompts.jsonl"), old).unwrap();
        assert!(Store::load(&d, 2000).snapshot().is_empty());
        // And a pass over the same transcript brings the reply back at once.
        let (_, got) = scan(&mut Store::load(&d, 2000), &[&p]);
        assert_eq!(got[0].reply.as_deref(), Some("Done."));
    }

    #[test]
    fn substantive_messages_are_counted_as_they_are_read() {
        let d = tmp("substantive");
        let p = write(&d, "abc", &[user("please fix the parser"), user("yes"), user("continue"), user("and add tests for the lexer too")]);
        let mut s = Store::load(&d, 2000);
        let (_, got) = scan(&mut s, &[&p]);
        assert_eq!((got[0].prompts.len(), got[0].substantive), (4, 2));
        append(&p, &[user("ok"), user("now refactor the parser module please")]);
        let (_, got) = scan(&mut s, &[&p]);
        assert_eq!((got[0].prompts.len(), got[0].substantive), (6, 3));
        s.save();
        assert_eq!(Store::load(&d, 2000).snapshot()[0].substantive, 3, "and again after a restart");
    }
}
