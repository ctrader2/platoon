//! Plan usage (5-hour / weekly limits) and exact context-window figures.
//!
//! Claude Code runs the configured `statusLine` command with a JSON blob on stdin, and that blob
//! carries `rate_limits` and `context_window`. `platoon statusline` is that command: it saves the
//! numbers under the cache dir, and the TUI reads them back. `platoon setup` installs it.

use crate::model::{PlanUsage, Window};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Status files for sessions that haven't reported for this long are deleted.
const STALE_SECS: u64 = 3 * 24 * 3600;

pub enum Usage {
    /// The statusLine command isn't installed in Claude Code's settings.
    NotSetUp,
    /// Installed, but no session has reported limits yet (Pro/Max only, after the first response).
    Waiting,
    Ready(PlanUsage),
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn cache_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("PLATOON_CACHE_DIR") {
        return PathBuf::from(d);
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"));
    base.join("platoon")
}

fn window(v: &Value) -> Option<Window> {
    let pct = v["used_percentage"].as_f64()?;
    Some(Window { pct: pct.round().clamp(0.0, 100.0) as u8, resets_at: v["resets_at"].as_u64().unwrap_or(0) })
}

fn window_json(w: &Option<Window>) -> Value {
    match w {
        Some(w) => json!({ "pct": w.pct, "resets_at": w.resets_at }),
        None => Value::Null,
    }
}

/// Handle one status line payload: save it, and return the line to show in Claude Code's status bar.
pub fn record(input: &str, dir: &Path, ts: u64) -> String {
    let Ok(v) = serde_json::from_str::<Value>(input) else { return String::new() };
    let five = window(&v["rate_limits"]["five_hour"]);
    let seven = window(&v["rate_limits"]["seven_day"]);

    // The id becomes a file name, so accept only what a UUID can contain.
    if let Some(id) = v["session_id"].as_str().filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')) {
        let entry = json!({
            "ts": ts,
            "context_pct": v["context_window"]["used_percentage"].as_f64().map(|p| p.round().clamp(0.0, 100.0)),
            "five_hour": window_json(&five),
            "seven_day": window_json(&seven),
        });
        let status = dir.join("status");
        if fs::create_dir_all(&status).is_ok() {
            // Write then rename so the TUI never reads a half-written file.
            let tmp = status.join(format!(".{id}.tmp"));
            if fs::write(&tmp, entry.to_string()).is_ok() {
                let _ = fs::rename(&tmp, status.join(format!("{id}.json")));
            }
        }
    }

    let mut parts = Vec::new();
    if let Some(w) = five {
        parts.push(format!("5h {}%", w.pct));
    }
    if let Some(w) = seven {
        parts.push(format!("7d {}%", w.pct));
    }
    parts.join(" · ")
}

pub struct Snapshot {
    pub plan: Option<PlanUsage>,
    /// Exact context-window percentage per session id, as Claude Code itself reports it.
    pub context: HashMap<String, u8>,
}

/// Read everything the status line command has saved. Across sessions, the newest report wins
/// for each window, and windows already past their reset time are dropped.
pub fn load(dir: &Path, now: u64) -> Snapshot {
    let mut context = HashMap::new();
    let (mut five, mut seven): (Option<(u64, Window)>, Option<(u64, Window)>) = (None, None);
    let status = dir.join("status");
    for e in fs::read_dir(&status).into_iter().flatten().flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Some(v) = fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) else { continue };
        let ts = v["ts"].as_u64().unwrap_or(0);
        if now.saturating_sub(ts) > STALE_SECS {
            let _ = fs::remove_file(&path);
            continue;
        }
        if let (Some(id), Some(p)) = (path.file_stem().and_then(|s| s.to_str()), v["context_pct"].as_f64()) {
            context.insert(id.to_string(), p.round().clamp(0.0, 100.0) as u8);
        }
        for (slot, key) in [(&mut five, "five_hour"), (&mut seven, "seven_day")] {
            let w = &v[key];
            if let (Some(pct), Some(resets_at)) = (w["pct"].as_u64(), w["resets_at"].as_u64()) {
                let newer = slot.as_ref().map_or(true, |(t, _)| ts > *t);
                if resets_at > now && newer {
                    *slot = Some((ts, Window { pct: pct.min(100) as u8, resets_at }));
                }
            }
        }
    }
    let plan = match (five, seven) {
        (None, None) => None,
        (f, s) => Some(PlanUsage { five_hour: f.map(|x| x.1), seven_day: s.map(|x| x.1) }),
    };
    Snapshot { plan, context }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("platoon-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn payload(id: &str, five: f64, seven: f64, resets: u64, ctx: f64) -> String {
        format!(
            r#"{{"session_id":"{id}","context_window":{{"used_percentage":{ctx}}},
                "rate_limits":{{"five_hour":{{"used_percentage":{five},"resets_at":{resets}}},
                                "seven_day":{{"used_percentage":{seven},"resets_at":{resets}}}}}}}"#
        )
    }

    #[test]
    fn records_and_reads_back_newest_report() {
        let dir = tmp("record");
        let line = record(&payload("aaaa-1", 23.5, 41.2, 2_000, 8.0), &dir, 1_000);
        assert_eq!(line, "5h 24% · 7d 41%");
        record(&payload("bbbb-2", 30.0, 45.0, 2_000, 61.4), &dir, 1_500); // newer report wins
        let snap = load(&dir, 1_600);
        let plan = snap.plan.unwrap();
        assert_eq!(plan.five_hour.unwrap().pct, 30);
        assert_eq!(plan.seven_day.unwrap().pct, 45);
        assert_eq!(snap.context["aaaa-1"], 8);
        assert_eq!(snap.context["bbbb-2"], 61);
    }

    #[test]
    fn expired_windows_are_dropped_and_missing_limits_are_fine() {
        let dir = tmp("expired");
        record(&payload("aaaa-1", 90.0, 50.0, 2_000, 10.0), &dir, 1_000);
        assert!(load(&dir, 2_500).plan.is_none(), "windows past resets_at must disappear");
        // API-key accounts send no rate_limits at all: still records context, prints nothing.
        let line = record(r#"{"session_id":"cccc-3","context_window":{"used_percentage":12}}"#, &dir, 3_000);
        assert_eq!(line, "");
        assert_eq!(load(&dir, 3_001).context["cccc-3"], 12);
        // Garbage in must not panic or print.
        assert_eq!(record("not json", &dir, 3_000), "");
    }

    #[test]
    fn hostile_session_id_cannot_escape_the_cache_dir() {
        let dir = tmp("hostile");
        record(&payload("../../evil", 1.0, 1.0, 2_000, 1.0), &dir, 1_000);
        assert!(!dir.join("../evil.json").exists() && !dir.join("status").join("../../evil.json").exists());
        assert!(load(&dir, 1_001).plan.is_none());
    }
}
