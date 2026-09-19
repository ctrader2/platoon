//! Adapter over Claude Code's on-disk state. Everything here reads undocumented internal
//! formats (`~/.claude/sessions/<pid>.json` and `~/.claude/projects/*/<id>.jsonl`), so all
//! knowledge of them lives in this file.

use serde_json::Value;
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

const TAIL_BYTES: u64 = 512 * 1024;
/// How far into a transcript to look for its first real prompt and working directory.
const HEAD_LINES: usize = 2000;
const HEAD_BYTES: u64 = 2 * 1024 * 1024;
const DEFAULT_WINDOW: u64 = 200_000;
const LARGE_WINDOW: u64 = 1_000_000;
/// Longest title or prompt kept per conversation, so the index stays small however long they are.
const TEXT_MAX: usize = 300;

/// Where Claude Code keeps its state (`CLAUDE_CONFIG_DIR`, else `~/.claude`).
pub fn claude_root() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(&std::env::var("HOME").unwrap_or_default()).join(".claude"))
}

/// A live Claude Code process, from its registry file.
pub struct Live {
    pub pid: u32,
    pub id: String,
    pub cwd: String,
    /// Only a name the user chose; Claude's derived placeholder is dropped.
    pub name: Option<String>,
    pub started_at: u64,
    pub busy: bool,
}

pub struct Reader {
    root: PathBuf,
}

impl Reader {
    pub fn new() -> Self {
        Self::with_root(claude_root())
    }

    pub fn with_root(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// All live interactive processes, oldest first. Several can share one session id.
    pub fn live(&self) -> Vec<Live> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(self.root.join("sessions")) else { return out };
        for path in entries.flatten().map(|e| e.path()) {
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(v) = fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) else {
                continue;
            };
            if let Some(l) = live_from(&v) {
                out.push(l);
            }
        }
        out.sort_by_key(|l| (l.started_at, l.pid));
        out
    }
}

fn live_from(v: &Value) -> Option<Live> {
    if v["kind"].as_str() != Some("interactive") {
        return None;
    }
    let pid = v["pid"].as_u64()? as u32;
    if !pid_alive(pid) {
        return None; // stale registry file
    }
    let name = match (v["nameSource"].as_str(), v["name"].as_str()) {
        (Some("derived") | None, _) | (_, None) => None,
        (_, Some(n)) => Some(n.to_string()),
    };
    Some(Live {
        pid,
        id: v["sessionId"].as_str()?.to_string(),
        cwd: v["cwd"].as_str().unwrap_or("").to_string(),
        name,
        started_at: v["startedAt"].as_u64().unwrap_or(0),
        busy: v["status"].as_str() == Some("busy"),
    })
}

/// `claude-sonnet-5` -> `Sonnet 5`, `claude-fable-5-1` -> `Fable 5.1`, `claude-haiku-4-5-20251001` -> `Haiku 4.5`.
/// Handles the older `claude-3-5-sonnet-...` ordering too. Anything unrecognised is shown as-is.
pub fn pretty_model(id: &str) -> String {
    let (base, ext) = match id.split_once('[') {
        Some((b, rest)) => (b, Some(rest.trim_end_matches(']').to_uppercase())),
        None => (id, None),
    };
    let Some(rest) = base.strip_prefix("claude-") else { return id.to_string() };
    let mut family = None;
    let mut nums: Vec<&str> = Vec::new();
    for t in rest.split('-') {
        if t.len() == 8 && t.chars().all(|c| c.is_ascii_digit()) {
            continue; // release date
        }
        if t.chars().all(|c| c.is_ascii_digit()) {
            nums.push(t);
        } else if family.is_none() {
            family = Some(t);
        }
    }
    let Some(family) = family else { return id.to_string() };
    let mut name: String = family.chars().next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
    name.push_str(&family[family.chars().next().map_or(0, char::len_utf8)..]);
    if !nums.is_empty() {
        name = format!("{name} {}", nums.join("."));
    }
    match ext {
        Some(e) if !e.is_empty() => format!("{name} {e}"),
        _ => name,
    }
}

/// The transcript doesn't record the model's window size, so infer it: 200k until the session is seen
/// past 200k tokens, then 1M for good (compaction shrinks usage but not the window).
pub fn window_size(tokens: u64, large: &mut bool) -> u64 {
    if tokens > DEFAULT_WINDOW {
        *large = true;
    }
    if *large { LARGE_WINDOW } else { DEFAULT_WINDOW }
}

/// Working directory a conversation was run in: the first `cwd` recorded in its transcript.
pub fn transcript_cwd(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    for line in BufReader::new(file).lines().take(400).map_while(Result::ok) {
        if let Some(cwd) = serde_json::from_str::<Value>(&line).ok().and_then(|v| v["cwd"].as_str().map(String::from)) {
            return Some(cwd);
        }
    }
    None
}

/// Every conversation id with a saved transcript, for looking a session up by (a prefix of) its id.
pub fn saved_session_ids(root: &Path) -> Vec<String> {
    let mut ids = Vec::new();
    for dir in fs::read_dir(root.join("projects")).into_iter().flatten().flatten() {
        for f in fs::read_dir(dir.path()).into_iter().flatten().flatten() {
            let p = f.path();
            if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    ids.push(stem.to_string());
                }
            }
        }
    }
    ids
}

pub fn pid_alive(pid: u32) -> bool {
    // Signal 0 checks existence only. EPERM means it exists but belongs to someone else.
    let exists = unsafe { libc::kill(pid as i32, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) };
    exists && !is_zombie(pid)
}

/// A process that has exited but not yet been reaped by its parent still answers signal 0, though it
/// is gone as far as anyone using it is concerned. (Linux only; elsewhere it counts as alive.)
fn is_zombie(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|s| s.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().next().map(|st| st == "Z")))
        .unwrap_or(false)
}

pub fn find_transcript(root: &Path, id: &str) -> Option<PathBuf> {
    fs::read_dir(root.join("projects"))
        .ok()?
        .flatten()
        .map(|d| d.path().join(format!("{id}.jsonl")))
        .find(|p| p.exists())
}

/// What platoon knows about one saved conversation, read from its transcript.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transcript {
    /// Working directory the conversation started in (the first `cwd` recorded).
    pub cwd: Option<String>,
    pub branch: Option<String>,
    /// Raw model id from the latest assistant message.
    pub model: Option<String>,
    pub context_tokens: u64,
    /// Once a session has gone past 200k tokens it must have a large window. Never flips back.
    pub large_window: bool,
    pub ai_title: Option<String>,
    /// First real user prompt. None means nothing but slash-command echoes and injected blocks: junk.
    pub first_prompt: Option<String>,
    /// Timestamp of the latest user or assistant entry, ms since the epoch.
    pub last_active_ms: Option<u64>,
}

/// Read a transcript cheaply, whatever its size: the first real prompt and cwd from the head, and the
/// latest title, usage, model, branch and time from a bounded tail. `prev` is the earlier reading of
/// the same file: what only the head or a distant title can tell us is kept when it isn't found again.
pub fn read_transcript(path: &Path, prev: Option<&Transcript>) -> Option<Transcript> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut t = prev.cloned().unwrap_or_default();
    if t.first_prompt.is_none() || t.cwd.is_none() {
        scan_head(&file, &mut t);
    }

    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let mut lines = text.lines();
    if start > 0 {
        lines.next(); // probably cut mid-line
    }
    scan_tail(lines.collect::<Vec<_>>().into_iter().rev(), &mut t);
    Some(t)
}

fn scan_head(file: &File, t: &mut Transcript) {
    let mut reader = BufReader::new(file.take(HEAD_BYTES));
    let mut line = Vec::new();
    for _ in 0..HEAD_LINES {
        line.clear();
        if !matches!(reader.read_until(b'\n', &mut line), Ok(n) if n > 0) {
            break;
        }
        let line = String::from_utf8_lossy(&line);
        let want_cwd = t.cwd.is_none() && line.contains("\"cwd\"");
        let want_prompt = t.first_prompt.is_none() && line.contains("\"type\":\"user\"");
        if !want_cwd && !want_prompt {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v["isSidechain"].as_bool() == Some(true) {
            continue; // subagent content is not the conversation
        }
        if want_cwd {
            t.cwd = v["cwd"].as_str().filter(|c| !c.is_empty()).map(String::from);
        }
        if want_prompt {
            t.first_prompt = real_prompt(&v);
        }
        if t.cwd.is_some() && t.first_prompt.is_some() {
            break;
        }
    }
}

/// `lines` run newest first, so the first hit for each fact is the latest one.
fn scan_tail<'a>(lines: impl Iterator<Item = &'a str>, t: &mut Transcript) {
    let (mut usage, mut branch, mut ts, mut title) = (false, false, false, false);
    let mut need_prompt = t.first_prompt.is_none();
    for line in lines {
        if usage && branch && ts && title && !need_prompt {
            break;
        }
        let is_title = line.contains("\"type\":\"ai-title\"");
        let is_turn = line.contains("\"type\":\"assistant\"") || line.contains("\"type\":\"user\"");
        // Once the cheap facts are in, only a title (or a fallback prompt) is worth another parse.
        let done_turns = usage && branch && ts && !need_prompt;
        if !(is_title && !title || is_turn && !done_turns) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match v["type"].as_str() {
            Some("ai-title") if !title => {
                if let Some(x) = v["aiTitle"].as_str().map(one_line).filter(|x| !x.is_empty()) {
                    t.ai_title = Some(truncate(x));
                    title = true;
                }
            }
            Some(kind @ ("assistant" | "user")) => {
                if !ts {
                    if let Some(ms) = v["timestamp"].as_str().and_then(parse_iso_ms) {
                        t.last_active_ms = Some(ms);
                        ts = true;
                    }
                }
                let side = v["isSidechain"].as_bool() == Some(true);
                if !branch && !side {
                    if let Some(b) = v["gitBranch"].as_str() {
                        t.branch = Some(b.to_string());
                        branch = true;
                    }
                }
                // Subagent (sidechain) messages have their own context; ignore them.
                if kind == "assistant" && !usage && !side {
                    let u = &v["message"]["usage"];
                    if u.is_object() && v["message"]["model"].as_str() != Some("<synthetic>") {
                        if let Some(m) = v["message"]["model"].as_str() {
                            t.model = Some(m.to_string());
                        }
                        let n = |k: &str| u[k].as_u64().unwrap_or(0);
                        t.context_tokens = n("input_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens");
                        if t.context_tokens > DEFAULT_WINDOW {
                            t.large_window = true;
                        }
                        usage = true;
                    }
                }
                if kind == "user" && need_prompt && !side {
                    if let Some(p) = real_prompt(&v) {
                        t.first_prompt = Some(p);
                        need_prompt = false;
                    }
                }
            }
            _ => {}
        }
    }
}

/// The text of a real user prompt: not a slash-command echo, caveat, compaction summary, interrupt marker
/// or other injected `<...>` block.
fn real_prompt(v: &Value) -> Option<String> {
    let flag = |k: &str| v[k].as_bool() == Some(true);
    if v["type"].as_str() != Some("user") || flag("isMeta") || flag("isSidechain") || flag("isCompactSummary") {
        return None;
    }
    let text = match &v["message"]["content"] {
        Value::String(s) => one_line(s),
        Value::Array(a) => one_line(&a.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join(" ")),
        _ => return None,
    };
    (!text.is_empty() && !text.starts_with('<') && !text.starts_with("[Request interrupted")).then(|| truncate(text))
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(s: String) -> String {
    if s.chars().count() <= TEXT_MAX { s } else { s.chars().take(TEXT_MAX).collect() }
}

/// `2026-09-19T03:53:40.091Z` -> ms since the epoch. Only the UTC form Claude Code writes is understood.
pub fn parse_iso_ms(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, sec) = (num(0..4)?, num(5..7)?, num(8..10)?, num(11..13)?, num(14..16)?, num(17..19)?);
    let frac: u64 = match b.get(19) {
        Some(b'.') => {
            let digits: String = s[20..].chars().take_while(|c| c.is_ascii_digit()).take(3).collect();
            format!("{digits:0<3}").parse().ok()?
        }
        _ => 0,
    };
    // Days since 1970-01-01 (Howard Hinnant's civil-from-days, inverted).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3600 + mi * 60 + sec;
    u64::try_from(secs).ok().map(|s| s * 1000 + frac)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_names_are_shortened() {
        assert_eq!(pretty_model("claude-sonnet-5"), "Sonnet 5");
        assert_eq!(pretty_model("claude-fable-5-1"), "Fable 5.1");
        assert_eq!(pretty_model("claude-opus-5"), "Opus 5");
        assert_eq!(pretty_model("claude-haiku-4-5-20251001"), "Haiku 4.5");
        assert_eq!(pretty_model("claude-3-5-sonnet-20241022"), "Sonnet 3.5");
        assert_eq!(pretty_model("claude-sonnet-5[1m]"), "Sonnet 5 1M");
        assert_eq!(pretty_model("some-other-model"), "some-other-model");
        assert_eq!(pretty_model("claude-"), "claude-");
    }

    #[test]
    fn window_estimate_never_flips_back() {
        let mut large = false;
        assert_eq!(window_size(190_000, &mut large), 200_000);
        assert_eq!(window_size(210_000, &mut large), 1_000_000);
        // After compaction the usage drops, but the window is still the large one.
        assert_eq!(window_size(60_000, &mut large), 1_000_000);
    }
}

#[cfg(test)]
mod transcript_tests {
    use super::*;

    #[test]
    fn iso_timestamps_convert_to_epoch_ms() {
        assert_eq!(parse_iso_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(parse_iso_ms("2026-09-19T03:53:40.091Z"), Some(1_789_790_020_091));
        assert_eq!(parse_iso_ms("2024-02-29T12:00:00Z"), Some(1_709_208_000_000));
        assert_eq!(parse_iso_ms("2026-09-19T03:53:40.5Z"), Some(1_789_790_020_500));
        assert_eq!(parse_iso_ms("not a time"), None);
    }

    fn prompt(v: Value) -> Option<String> {
        real_prompt(&v)
    }

    #[test]
    fn only_real_prompts_count() {
        use serde_json::json;
        let user = |c: Value| json!({"type":"user","message":{"content":c}});
        assert_eq!(prompt(user(json!("fix the bug"))).as_deref(), Some("fix the bug"));
        assert_eq!(prompt(user(json!([{"type":"text","text":"a\n  b"}]))).as_deref(), Some("a b"));
        assert_eq!(prompt(user(json!("<command-name>/clear</command-name>"))), None);
        assert_eq!(prompt(user(json!("[Request interrupted by user]"))), None);
        assert_eq!(prompt(user(json!([{"type":"tool_result","content":"x"}]))), None);
        assert_eq!(prompt(json!({"type":"user","isMeta":true,"message":{"content":"Caveat: x"}})), None);
        assert_eq!(prompt(json!({"type":"user","isSidechain":true,"message":{"content":"sub"}})), None);
        assert_eq!(prompt(json!({"type":"user","isCompactSummary":true,"message":{"content":"This session is being continued"}})), None);
        assert_eq!(prompt(json!({"type":"assistant","message":{"content":"hi"}})), None);
    }
}
