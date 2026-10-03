//! Plan usage (5-hour / weekly limits) and exact context-window figures.
//!
//! Claude Code runs the configured `statusLine` command with a JSON blob on stdin, and that blob
//! carries `rate_limits` and `context_window`. `platoon statusline` is that command: it saves the
//! numbers under the cache dir, and the TUI reads them back. `platoon setup` installs it.

use crate::model::{PlanUsage, Window};
use serde_json::{Value, json};
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
///
/// A window that hasn't started reads 0%. Right after the 5-hour window resets, Claude Code leaves
/// `five_hour` out of its reports altogether (it does not send 0) while still sending `seven_day`,
/// until the first answer of the new window. So a report with no 5-hour figure but a weekly one
/// that is still running says "on a plan, nothing used in this 5-hour window yet", and counts as a
/// 5-hour figure of 0%. It competes by `ts` with every report that carried a 5-hour figure, even
/// one that has since expired: only when it is the newest does the 5-hour window read 0%, so an
/// older empty report never hides a newer real figure. Its `resets_at` is 0: unknown, since the
/// window starts with the next answer. When no report carries either figure (API-key logins), the
/// plan stays None: a genuinely unknown figure still reads `n/a`.
///
/// The weekly window gets no such rule: every 5-hour window lies inside a weekly one, so a report
/// with a 5-hour figure but no weekly one is missing data, not "nothing used yet".
pub fn load(dir: &Path, now: u64) -> Snapshot {
    let mut context = HashMap::new();
    let (mut five, mut seven): (Option<(u64, Window)>, Option<(u64, Window)>) = (None, None);
    // The newest report that carried a 5-hour figure, expired or not, and the newest that left it out
    // while its weekly figure was still running (see above).
    let (mut five_newest, mut five_unstarted): (Option<u64>, Option<u64>) = (None, None);
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
        let figure = |key: &str| {
            let w = &v[key];
            w["pct"].as_u64().zip(w["resets_at"].as_u64())
        };
        for (slot, key) in [(&mut five, "five_hour"), (&mut seven, "seven_day")] {
            if let Some((pct, resets_at)) = figure(key) {
                let newer = slot.as_ref().map_or(true, |(t, _)| ts > *t);
                if resets_at > now && newer {
                    *slot = Some((ts, Window { pct: pct.min(100) as u8, resets_at }));
                }
            }
        }
        if figure("five_hour").is_some() {
            five_newest = five_newest.max(Some(ts));
        } else if v["five_hour"].is_null() && figure("seven_day").is_some_and(|(_, resets_at)| resets_at > now) {
            five_unstarted = five_unstarted.max(Some(ts));
        }
    }
    // A tie goes to the real figure.
    if five_unstarted.is_some_and(|t| five_newest.map_or(true, |n| t > n)) {
        five = Some((five_unstarted.unwrap_or(0), Window { pct: 0, resets_at: 0 }));
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
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-test-{name}-{}", std::process::id())));
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

    /// A status file as `platoon statusline` writes it; `None` is a window Claude Code left out.
    fn status(dir: &Path, id: &str, ts: u64, five: Option<(u64, u64)>, seven: Option<(u64, u64)>) {
        let w = |x: Option<(u64, u64)>| x.map_or(Value::Null, |(pct, resets_at)| json!({"pct": pct, "resets_at": resets_at}));
        fs::create_dir_all(dir.join("status")).unwrap();
        let v = json!({"ts": ts, "context_pct": 5.0, "five_hour": w(five), "seven_day": w(seven)});
        fs::write(dir.join("status").join(format!("{id}.json")), v.to_string()).unwrap();
    }

    fn five(dir: &Path, now: u64) -> Option<(u8, u64)> {
        load(dir, now).plan.and_then(|p| p.five_hour).map(|w| (w.pct, w.resets_at))
    }

    #[test]
    fn a_five_hour_window_that_has_not_started_reads_zero_until_a_real_figure_arrives() {
        // What the user's machine saw: the window reset at 12:20:00 (reset = 20_000 here), a report one second later
        // had no 5-hour figure but a running weekly one, and a later report carried the new window at 2%.
        let dir = tmp("unstarted");
        let (reset, week) = (20_000, 500_000);
        status(&dir, "old-1", 10_000, Some((14, reset)), Some((1, week)));
        status(&dir, "old-2", 12_000, Some((6, reset)), Some((5, week)));
        assert!(five(&dir, reset - 1).is_some_and(|(p, _)| p == 6), "before the reset the newest real figure shows");
        assert_eq!(five(&dir, reset + 1), None, "just after the reset, before any report: unknown");
        status(&dir, "new-1", reset + 1, None, Some((3, week)));
        let plan = load(&dir, reset + 2).plan.unwrap();
        assert_eq!(plan.five_hour.map(|w| (w.pct, w.resets_at)), Some((0, 0)), "not started: 0%, reset time unknown");
        assert_eq!(plan.seven_day.unwrap().pct, 3);
        // Eleven minutes later a report carries the new window: that figure wins over the older empty report.
        status(&dir, "new-2", reset + 660, Some((2, reset + 5 * 3600)), Some((3, week)));
        assert_eq!(five(&dir, reset + 661), Some((2, reset + 5 * 3600)));
    }

    #[test]
    fn an_older_empty_report_never_hides_a_newer_figure() {
        let dir = tmp("older-empty");
        status(&dir, "empty", 1_000, None, Some((3, 9_000)));
        status(&dir, "real", 1_100, Some((40, 5_000)), Some((3, 9_000)));
        assert_eq!(five(&dir, 1_200), Some((40, 5_000)));
        // The same second: the real figure wins the tie.
        status(&dir, "empty", 1_100, None, Some((3, 9_000)));
        assert_eq!(five(&dir, 1_200), Some((40, 5_000)));
        // A newer figure that has since expired still outranks the older empty report: the window started after it,
        // so 0% would be a guess. Unknown stays n/a.
        assert_eq!(five(&dir, 5_000), None);
        // A newer empty report (after that window reset) does read 0%.
        status(&dir, "empty", 5_001, None, Some((3, 9_000)));
        assert_eq!(five(&dir, 5_002), Some((0, 0)));
    }

    #[test]
    fn no_figures_anywhere_is_still_waiting_and_an_empty_report_needs_a_running_weekly_window() {
        let dir = tmp("nothing");
        // API-key logins: neither window, ever.
        status(&dir, "api", 1_000, None, None);
        assert!(load(&dir, 1_001).plan.is_none(), "nothing reports plan usage: Waiting, not 0%");
        // A weekly figure that has expired does not say "on a plan now".
        status(&dir, "api", 1_000, None, Some((3, 1_500)));
        assert!(load(&dir, 1_600).plan.is_none());
        assert_eq!(five(&dir, 1_400), Some((0, 0)), "while that week ran, it did");
        // Old status files without the key at all behave the same as before.
        fs::write(dir.join("status/api.json"), r#"{"ts":1000,"context_pct":5.0}"#).unwrap();
        assert!(load(&dir, 1_001).plan.is_none());
        // The weekly window has no such rule: a 5-hour figure without a weekly one leaves the weekly one unknown.
        status(&dir, "api", 1_000, Some((7, 2_000)), None);
        let plan = load(&dir, 1_001).plan.unwrap();
        assert!(plan.seven_day.is_none() && plan.five_hour.unwrap().pct == 7);
    }

    #[test]
    fn a_window_that_has_not_started_never_pauses_the_smart_features() {
        let dir = tmp("no-pause");
        status(&dir, "empty", 1_000, None, Some((3, 9_000)));
        assert_eq!(crate::ai::plan_percent(&dir, 1_001), Some((3, "weekly limit")), "the weekly figure is the highest");
        status(&dir, "empty", 1_000, None, Some((0, 9_000)));
        let (pct, _) = crate::ai::plan_percent(&dir, 1_001).unwrap();
        assert_eq!(pct, 0);
        let mut settings = crate::settings::Settings::default();
        settings.tokens_enabled = true;
        settings.ai_pause_percent = 1;
        let refused = crate::ai::gate(&settings, crate::ai::TEST_FEATURE, &dir, 1_001).err();
        assert!(!matches!(refused, Some(crate::ai::Refusal::PlanBusy { .. })));
    }
}
