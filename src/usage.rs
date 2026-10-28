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

/// The user's real folders (`cache_dir`, `archive::config_dir`, `claude::claude_root`, `claude::claude_json_path`,
/// `codex::codex_root`) are for the running program only. A unit test that reaches one is a bug that would read, write or clean up the
/// real thing (the live sessions' sockets are in `<cache>/hosts`), so in a unit test it panics, whatever the
/// environment says. Code a test runs takes its folders as arguments instead.
pub fn forbid_real_dir(what: &str) {
    #[cfg(test)]
    panic!("a unit test reached the user's real folder ({what}): pass the test's own folder instead");
    #[cfg(not(test))]
    let _ = what;
}

/// `$PLATOON_CACHE_DIR`, else `$XDG_CACHE_HOME/platoon`, else `~/.cache/platoon`: the user's real one. Code that a unit
/// test runs takes the cache folder as an argument instead (an `App` has its own, `testkit::World` a temporary one).
pub fn cache_dir() -> PathBuf {
    forbid_real_dir("cache_dir");
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

/// One usage-limit window of a plan, as an agent reports it: how long the window is, how much of it is used, and when it
/// starts over (seconds since the epoch; 0 when not known). Claude Code's are 5 hours and a week; Codex's are what the
/// plan has (a free plan: one of 30 days).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limit {
    pub minutes: u64,
    pub pct: u8,
    pub resets_at: u64,
}

impl Limit {
    pub const FIVE_HOURS: u64 = 300;
    pub const WEEK: u64 = 7 * 1440;

    /// The window in a few characters, for the header: `5h`, `7d`, `30d`.
    pub fn short(&self) -> String {
        match self.minutes {
            m if m > 0 && m % 1440 == 0 => format!("{}d", m / 1440),
            m if m > 0 && m % 60 == 0 => format!("{}h", m / 60),
            m => format!("{m}m"),
        }
    }

    /// The window in words: `5-hour limit`, `weekly limit`, `30-day limit`.
    pub fn name(&self) -> String {
        match self.minutes {
            Limit::WEEK => "weekly limit".into(),
            m if m > 0 && m % 1440 == 0 => format!("{}-day limit", m / 1440),
            m if m > 0 && m % 60 == 0 => format!("{}-hour limit", m / 60),
            m => format!("{m}-minute limit"),
        }
    }
}

impl PlanUsage {
    /// Claude Code's windows as limits, the 5-hour one first; the ones it hasn't reported are left out.
    pub fn limits(&self) -> Vec<Limit> {
        [(Limit::FIVE_HOURS, self.five_hour), (Limit::WEEK, self.seven_day)]
            .into_iter()
            .filter_map(|(minutes, w)| w.map(|w| Limit { minutes, pct: w.pct, resets_at: w.resets_at }))
            .collect()
    }
}

pub struct Snapshot {
    pub plan: Option<PlanUsage>,
    /// Exact context-window percentage per session id, as Claude Code itself reports it.
    pub context: HashMap<String, u8>,
}

/// Reports of one window whose `resets_at` are at most this far apart are the same window (they can differ a little,
/// by rounding); a report this much earlier than the latest is of a window that is over.
pub const SAME_WINDOW_SECS: u64 = 10 * 60;

/// The figure of the current window among `figures` (percent used, `resets_at`), all of one window length and still
/// running: the latest `resets_at` is the current window, and of the figures within `SAME_WINDOW_SECS` of it, the
/// highest percent (usage only grows within a window), with its own `resets_at`. See `load`.
pub fn current(figures: impl IntoIterator<Item = (u8, u64)>) -> Option<(u8, u64)> {
    let figures: Vec<(u8, u64)> = figures.into_iter().collect();
    let latest = figures.iter().map(|f| f.1).max()?;
    figures.into_iter().filter(|f| latest - f.1 <= SAME_WINDOW_SECS).max()
}

/// Read everything the status line command has saved. For each window, across sessions, the
/// highest figure of the current window wins (`current`); windows already past their reset time
/// are dropped.
///
/// Each session reports what its own last answer said, and a session that sits idle keeps
/// reporting it (a task's session re-runs its status line every few seconds): its report is
/// often the newest and its figure old. Within one window usage only grows, so an old snapshot of
/// the current window is lower than the newest truth, and the time of a report never picks the
/// figure: the highest one of the current window does, with its own `resets_at`.
///
/// The current window is the one with the latest `resets_at` still ahead. A report whose
/// `resets_at` is clearly earlier (more than `SAME_WINDOW_SECS` before it) belongs to a window that
/// is over although its `resets_at` has not passed: Anthropic can start a new window before the
/// old one's reset time, and an idle session's old 40% must not beat the new window's 3% until
/// then. Reports of one window may give its `resets_at` a little differently (rounding), so the
/// ones within `SAME_WINDOW_SECS` of the latest count as the same window.
///
/// A window that hasn't started reads 0%. Right after the 5-hour window resets, Claude Code leaves
/// `five_hour` out of its reports altogether (it does not send 0) while still sending `seven_day`,
/// until the first answer of the new window. So a report with no 5-hour figure but a weekly one
/// that is still running says "on a plan, nothing used in this 5-hour window yet". It reads 0%
/// only when no report holds a 5-hour figure that is still running: a session idle since before
/// the reset keeps saying "not started" while another one is using the new window. Its
/// `resets_at` is 0: unknown, since the window starts with the next answer. When no report carries
/// either figure (API-key logins), the plan stays None: a genuinely unknown figure still reads
/// `n/a`.
///
/// The weekly window gets the same choice of figure but no "not started" rule: every 5-hour
/// window lies inside a weekly one, so a report with a 5-hour figure but no weekly one is missing
/// data, not "nothing used yet".
pub fn load(dir: &Path, now: u64) -> Snapshot {
    let mut context = HashMap::new();
    // Every figure still running, as (pct, resets_at).
    let mut fives: Vec<(u8, u64)> = Vec::new();
    let mut sevens: Vec<(u8, u64)> = Vec::new();
    // Some report left the 5-hour figure out while its weekly figure was still running (see above).
    let mut five_unstarted = false;
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
        for (list, key) in [(&mut fives, "five_hour"), (&mut sevens, "seven_day")] {
            if let Some((pct, resets_at)) = figure(key).filter(|(_, resets_at)| *resets_at > now) {
                list.push((pct.min(100) as u8, resets_at));
            }
        }
        if v["five_hour"].is_null() && figure("seven_day").is_some_and(|(_, resets_at)| resets_at > now) {
            five_unstarted = true;
        }
    }
    let window = |figures: Vec<(u8, u64)>| current(figures).map(|(pct, resets_at)| Window { pct, resets_at });
    let (mut five, seven) = (window(fives), window(sevens));
    if five.is_none() && five_unstarted {
        five = Some(Window { pct: 0, resets_at: 0 });
    }
    let plan = match (five, seven) {
        (None, None) => None,
        (five_hour, seven_day) => Some(PlanUsage { five_hour, seven_day }),
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
        // Usage only grows within a window: the older 14% is the higher figure of the same window, and the newer 6% an
        // old snapshot of it. (This used to show the newest report's 6%.)
        assert_eq!(five(&dir, reset - 1), Some((14, reset)), "before the reset the highest running figure shows");
        assert_eq!(load(&dir, reset - 1).plan.unwrap().seven_day.unwrap().pct, 5);
        assert_eq!(five(&dir, reset + 1), None, "just after the reset, before any report: unknown");
        status(&dir, "new-1", reset + 1, None, Some((3, week)));
        let plan = load(&dir, reset + 2).plan.unwrap();
        assert_eq!(plan.five_hour.map(|w| (w.pct, w.resets_at)), Some((0, 0)), "not started: 0%, reset time unknown");
        assert_eq!(plan.seven_day.unwrap().pct, 5, "the highest weekly figure of the week still running");
        // Eleven minutes later a report carries the new window: that figure wins over the older empty report.
        status(&dir, "new-2", reset + 660, Some((2, reset + 5 * 3600)), Some((3, week)));
        assert_eq!(five(&dir, reset + 661), Some((2, reset + 5 * 3600)));
    }

    #[test]
    fn an_empty_report_never_hides_a_running_figure_and_reads_zero_once_none_is_running() {
        let dir = tmp("older-empty");
        status(&dir, "empty", 1_000, None, Some((3, 9_000)));
        status(&dir, "real", 1_100, Some((40, 5_000)), Some((3, 9_000)));
        assert_eq!(five(&dir, 1_200), Some((40, 5_000)));
        // The same second, or newer: the running figure wins.
        status(&dir, "empty", 1_100, None, Some((3, 9_000)));
        assert_eq!(five(&dir, 1_200), Some((40, 5_000)));
        status(&dir, "empty", 1_300, None, Some((3, 9_000)));
        assert_eq!(five(&dir, 1_400), Some((40, 5_000)));
        // Once that figure has expired its window is over, and nothing has started a new one: 0%, whatever the times
        // of the reports. (This used to read unknown while the expired figure was the newer report.)
        assert_eq!(five(&dir, 5_000), Some((0, 0)));
    }

    #[test]
    fn an_idle_sessions_newer_lower_figures_never_beat_the_live_sessions_higher_ones() {
        // What the user's Mac had: ordinary sessions a minute earlier said 15%, 15% and 16% for the week (Anthropic's
        // usage page said 16%); the newest report, from a task's session idle for a day and a half whose status line
        // re-runs every 2 s, said 7% for the same week and had no 5-hour figure. The meter showed 5h 0% and 7d 7%.
        let dir = tmp("mac");
        let (now, week) = (1_000_000, 1_400_000);
        status(&dir, "a", now - 70, Some((20, now - 10)), Some((15, week)));
        status(&dir, "b", now - 65, None, Some((15, week)));
        status(&dir, "c", now - 60, None, Some((16, week)));
        status(&dir, "idle-task", now - 1, None, Some((7, week)));
        let plan = load(&dir, now).plan.unwrap();
        assert_eq!(plan.seven_day.map(|w| (w.pct, w.resets_at)), Some((16, week)), "the highest weekly figure");
        // No 5-hour figure is running (a's has expired): not started.
        assert_eq!(plan.five_hour.map(|w| (w.pct, w.resets_at)), Some((0, 0)));
        // Once a session holds a running 5-hour figure it shows, and the idle session's reports change nothing.
        status(&dir, "c", now + 5, Some((4, now + 18_000)), Some((16, week)));
        for ts in [now + 6, now + 8, now + 10] {
            status(&dir, "idle-task", ts, None, Some((7, week)));
            let plan = load(&dir, ts + 1).plan.unwrap();
            assert_eq!(plan.five_hour.map(|w| (w.pct, w.resets_at)), Some((4, now + 18_000)));
            assert_eq!(plan.seven_day.unwrap().pct, 16);
        }
        // An idle session's older snapshot of the running 5-hour window is lower, so it never shows either.
        status(&dir, "idle-task", now + 12, Some((1, now + 18_000)), Some((7, week)));
        assert_eq!(five(&dir, now + 13), Some((4, now + 18_000)));
    }

    #[test]
    fn a_window_started_early_shows_the_new_figure_not_an_idle_sessions_old_one() {
        // Anthropic started a new 5-hour window before the old one's reset time `t`: an idle session still reports the
        // old window at 40% (resetting at `t`, still ahead), the live session the new one at 3%.
        let dir = tmp("early");
        let (now, t) = (100_000, 104_000);
        let new_reset = t + 4 * 3600 + 55 * 60;
        status(&dir, "idle", now - 2, Some((40, t)), Some((30, 700_000)));
        status(&dir, "live", now - 30, Some((3, new_reset)), Some((31, 700_000)));
        assert_eq!(five(&dir, now), Some((3, new_reset)), "the window with the latest reset is the current one");
        // The weekly window follows the same rule: a week started early beats the old week's higher figure.
        status(&dir, "idle", now - 2, Some((40, t)), Some((80, 300_000)));
        status(&dir, "live", now - 30, Some((3, new_reset)), Some((2, 300_000 + 6 * 86_400)));
        let plan = load(&dir, now).plan.unwrap();
        assert_eq!(plan.seven_day.map(|w| (w.pct, w.resets_at)), Some((2, 300_000 + 6 * 86_400)));
        assert_eq!(plan.five_hour.map(|w| w.pct), Some(3));
    }

    #[test]
    fn reports_of_one_window_a_little_apart_in_their_reset_time_are_one_window() {
        let dir = tmp("rounding");
        let (now, reset, week) = (100_000, 110_000, 600_000);
        // One second apart: the higher figure wins, with its own reset time, whichever is later.
        status(&dir, "a", now - 10, Some((25, reset)), Some((44, week + 1)));
        status(&dir, "b", now - 5, Some((12, reset + 1)), Some((41, week)));
        let plan = load(&dir, now).plan.unwrap();
        assert_eq!(plan.five_hour.map(|w| (w.pct, w.resets_at)), Some((25, reset)));
        assert_eq!(plan.seven_day.map(|w| (w.pct, w.resets_at)), Some((44, week + 1)));
        // Ten minutes apart is still the same window; more than that is another one.
        status(&dir, "b", now - 5, Some((12, reset + SAME_WINDOW_SECS)), Some((41, week)));
        assert_eq!(five(&dir, now), Some((25, reset)));
        status(&dir, "b", now - 5, Some((12, reset + SAME_WINDOW_SECS + 1)), Some((41, week)));
        assert_eq!(five(&dir, now), Some((12, reset + SAME_WINDOW_SECS + 1)));
    }

    #[test]
    fn when_a_five_hour_window_ends_it_reads_not_started_until_a_new_figure_arrives() {
        // At the end of a 5-hour window Claude Code re-runs every session's status line one second after `resets_at`,
        // all of them without a 5-hour figure.
        let dir = tmp("window-end");
        let (reset, week) = (50_000, 400_000);
        status(&dir, "live", reset - 300, Some((62, reset)), Some((30, week)));
        status(&dir, "other", reset - 200, Some((58, reset)), Some((29, week)));
        status(&dir, "idle-task", reset - 1, None, Some((12, week)));
        assert_eq!(five(&dir, reset - 1), Some((62, reset)));
        for id in ["live", "other", "idle-task"] {
            status(&dir, id, reset + 1, None, Some((30, week)));
        }
        assert_eq!(five(&dir, reset + 2), Some((0, 0)), "not started");
        // The first answer of the new window: its figure wins over every "not started" report after it.
        status(&dir, "live", reset + 120, Some((1, reset + 120 + 18_000)), Some((30, week)));
        for ts in [reset + 122, reset + 124, reset + 600] {
            status(&dir, "idle-task", ts, None, Some((30, week)));
            assert_eq!(five(&dir, ts + 1), Some((1, reset + 120 + 18_000)));
        }
        // The old window's figures never come back: they expired at `resets_at`, whatever was reported.
        status(&dir, "other", reset + 700, Some((58, reset)), Some((29, week)));
        assert_eq!(five(&dir, reset + 701), Some((1, reset + 120 + 18_000)));
    }

    #[test]
    fn an_idle_session_saying_not_started_never_hides_a_running_window_from_another() {
        // What the user saw: the meter flapping between 31% and 0%. The user's own session reported a running window
        // (31%, resetting at `reset`). A task's session, idle since before the last reset, re-ran its status line every
        // 2 s and each time left the 5-hour figure out: "not started", and always the newest report.
        let dir = tmp("flap");
        let (reset, week) = (20_000, 500_000);
        status(&dir, "live", 15_000, Some((31, reset)), Some((40, week)));
        for ts in [15_002, 15_004, 15_006] {
            status(&dir, "task", ts, None, Some((40, week)));
            assert_eq!(five(&dir, ts + 1), Some((31, reset)), "a running window has started, whatever is newer");
            // The live session reports again in between: the same figure, never a flap to 0%.
            status(&dir, "live", ts + 1, Some((31, reset)), Some((40, week)));
            assert_eq!(five(&dir, ts + 1), Some((31, reset)));
        }
        // Once that window is over and nothing holds a running one, "not started" is 0% again.
        status(&dir, "task", reset + 5, None, Some((40, week)));
        assert_eq!(five(&dir, reset + 6), Some((0, 0)));
        // And a session that answers in the new window shows its figure, whatever reports come after it.
        status(&dir, "live", reset + 10, Some((2, reset + 18_000)), Some((41, week)));
        status(&dir, "task", reset + 12, None, Some((41, week)));
        assert_eq!(five(&dir, reset + 13), Some((2, reset + 18_000)));
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
        let claude = crate::ai::tests::claude_here();
        let percent = |now| crate::ai::plan_percent(crate::agents::Agent::Claude, &dir, &claude, now);
        assert_eq!(percent(1_001), Some((3, "weekly limit".to_string())), "the weekly figure is the highest");
        status(&dir, "empty", 1_000, None, Some((0, 9_000)));
        let (pct, _) = percent(1_001).unwrap();
        assert_eq!(pct, 0);
        let settings = crate::settings::Settings { tokens_enabled: true, ai_pause_percent: 1, ..Default::default() };
        let refused = crate::ai::gate(&settings, crate::ai::TEST_FEATURE, &dir, &claude, 1_001).err();
        assert!(!matches!(refused, Some(crate::ai::Refusal::PlanBusy { .. })));
    }
}
