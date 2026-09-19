//! Adapter over Claude Code's on-disk state. Everything here reads undocumented internal
//! formats (`~/.claude/sessions/<pid>.json` and `~/.claude/projects/*/<id>.jsonl`), so all
//! knowledge of them lives in this file.

use crate::model::{Session, Status};
use serde_json::Value;
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

const TAIL_BYTES: u64 = 512 * 1024;
const DEFAULT_WINDOW: u64 = 200_000;
const LARGE_WINDOW: u64 = 1_000_000;

#[derive(Default)]
struct Cache {
    transcript: Option<PathBuf>,
    /// Transcript length at the last scan; skip rescanning if unchanged.
    scanned_len: Option<u64>,
    first_prompt: Option<String>,
    ai_title: Option<String>,
    branch: Option<String>,
    /// Raw model id from the latest assistant message.
    model: Option<String>,
    context_tokens: u64,
    /// Once a session has gone past 200k tokens it must have a large window. Never flips back.
    large_window: bool,
}

pub struct Reader {
    root: PathBuf,
    home: String,
    cache: HashMap<String, Cache>,
}

impl Reader {
    pub fn new() -> Self {
        let home = std::env::var("HOME").unwrap_or_default();
        let root = std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(&home).join(".claude"));
        Self { root, home, cache: HashMap::new() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// All live interactive sessions, oldest first. `unread` is left false; the app tracks it.
    pub fn load(&mut self) -> Vec<Session> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(self.root.join("sessions")) else { return out };
        for path in entries.flatten().map(|e| e.path()) {
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(v) = fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) else {
                continue;
            };
            if let Some(s) = self.session(&v) {
                out.push(s);
            }
        }
        out.sort_by_key(|s| s.started_at);
        let live: Vec<&String> = out.iter().map(|s| &s.id).collect();
        self.cache.retain(|id, _| live.contains(&id));
        out
    }

    fn session(&mut self, v: &Value) -> Option<Session> {
        if v["kind"].as_str() != Some("interactive") {
            return None;
        }
        let pid = v["pid"].as_u64()? as u32;
        if !pid_alive(pid) {
            return None; // stale registry file
        }
        let id = v["sessionId"].as_str()?.to_string();
        let cwd = v["cwd"].as_str().unwrap_or("").to_string();

        let root = self.root.clone();
        let dir = self.tilde(&cwd);
        let c = self.cache.entry(id.clone()).or_default();
        refresh_cache(c, &root, &id);

        // Claude Code derives a placeholder name; only trust names the user chose.
        let name = match (v["nameSource"].as_str(), v["name"].as_str()) {
            (Some("derived") | None, _) | (_, None) => {
                Path::new(&cwd).file_name().and_then(|n| n.to_str()).unwrap_or(&cwd).to_string()
            }
            (_, Some(n)) => n.to_string(),
        };
        let description = c
            .ai_title
            .clone()
            .or_else(|| c.first_prompt.clone())
            .unwrap_or_default();
        let window = window_size(c.context_tokens, &mut c.large_window);

        Some(Session {
            id,
            pid,
            name,
            dir,
            cwd,
            branch: c.branch.clone().filter(|b| b != "HEAD"),
            model: c.model.as_deref().map(pretty_model),
            status: if v["status"].as_str() == Some("busy") { Status::Busy } else { Status::Idle },
            unread: false,
            external: true,
            context_pct: (c.context_tokens * 100 / window).min(100) as u8,
            context_exact: false,
            description,
            started_at: v["startedAt"].as_u64().unwrap_or(0),
        })
    }

    fn tilde(&self, p: &str) -> String {
        match p.strip_prefix(&self.home) {
            Some(rest) if !self.home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => format!("~{rest}"),
            _ => p.to_string(),
        }
    }
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
fn window_size(tokens: u64, large: &mut bool) -> u64 {
    if tokens > DEFAULT_WINDOW {
        *large = true;
    }
    if *large { LARGE_WINDOW } else { DEFAULT_WINDOW }
}

pub fn pid_alive(pid: u32) -> bool {
    // Signal 0 checks existence only. EPERM means it exists but belongs to someone else.
    unsafe { libc::kill(pid as i32, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
}

fn find_transcript(root: &Path, id: &str) -> Option<PathBuf> {
    fs::read_dir(root.join("projects"))
        .ok()?
        .flatten()
        .map(|d| d.path().join(format!("{id}.jsonl")))
        .find(|p| p.exists())
}

fn refresh_cache(c: &mut Cache, root: &Path, id: &str) {
    if c.transcript.is_none() {
        c.transcript = find_transcript(root, id);
    }
    let Some(path) = c.transcript.clone() else { return };
    let Ok(mut file) = File::open(&path) else { return };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if c.scanned_len == Some(len) {
        return;
    }
    c.scanned_len = Some(len);

    if c.first_prompt.is_none() {
        c.first_prompt = first_prompt(&file);
    }

    // Only the tail: it holds the latest usage, title and branch, and stays cheap on huge files.
    let start = len.saturating_sub(TAIL_BYTES);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return;
    }
    let mut buf = Vec::new();
    if file.take(TAIL_BYTES).read_to_end(&mut buf).is_err() {
        return;
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines = text.lines();
    if start > 0 {
        lines.next(); // probably cut mid-line
    }
    for line in lines {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if let Some(b) = v["gitBranch"].as_str() {
            c.branch = Some(b.to_string());
        }
        match v["type"].as_str() {
            Some("ai-title") => {
                if let Some(t) = v["aiTitle"].as_str() {
                    c.ai_title = Some(one_line(t));
                }
            }
            // Subagent (sidechain) messages have their own context; ignore them.
            Some("assistant") if v["isSidechain"].as_bool() != Some(true) => {
                let u = &v["message"]["usage"];
                if u.is_object() && v["message"]["model"].as_str() != Some("<synthetic>") {
                    if let Some(m) = v["message"]["model"].as_str() {
                        c.model = Some(m.to_string());
                    }
                    let n = |k: &str| u[k].as_u64().unwrap_or(0);
                    c.context_tokens = n("input_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens");
                }
            }
            _ => {}
        }
    }
}

/// First real user prompt: skips slash-command echoes, caveats and other injected `<...>` blocks.
fn first_prompt(file: &File) -> Option<String> {
    for line in BufReader::new(file).lines().take(2000).map_while(Result::ok) {
        if !line.contains("\"type\":\"user\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v["type"].as_str() != Some("user") || v["isMeta"].as_bool() == Some(true) || v["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        let content = &v["message"]["content"];
        let text = match content {
            Value::String(s) => s.clone(),
            Value::Array(a) => a.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join(" "),
            _ => continue,
        };
        let text = one_line(&text);
        if !text.is_empty() && !text.starts_with('<') {
            return Some(text);
        }
    }
    None
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
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
