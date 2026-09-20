//! What platoon has spent of the user's Claude tokens, and whether it is currently backing off after a failure.
//!
//! One small file in the cache dir (`ai-usage.json`): totals per local calendar day (the four usage numbers
//! Claude reports, the cost it reports, the calls, and the same per feature) and the back-off state. It never
//! holds prompt text, replies or anything else a person typed. Writes are atomic (a temp file, then a rename)
//! and serialized between platoon windows and `platoon tokens test` with a lock file, so two of them never
//! lose each other's numbers. A file that can't be understood is treated as empty: the counting starts over
//! rather than the app failing. Days older than `KEEP_DAYS` are dropped on every write.

use serde_json::{json, Map, Value};
use std::{
    collections::BTreeMap,
    fs, io,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
};

/// How many calendar days of totals are kept.
pub const KEEP_DAYS: usize = 30;

/// The name under which a day's per-feature numbers hold what archived conversations used (see `Ledger::note_archived`).
pub const ARCHIVED_KEY: &str = "names_archived";

/// The four numbers Claude reports for a call. Every one of them counts against the daily limit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
    pub output: u64,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.input.saturating_add(self.cache_creation).saturating_add(self.cache_read).saturating_add(self.output)
    }

    fn add(&mut self, o: &Usage) {
        self.input = self.input.saturating_add(o.input);
        self.cache_creation = self.cache_creation.saturating_add(o.cache_creation);
        self.cache_read = self.cache_read.saturating_add(o.cache_read);
        self.output = self.output.saturating_add(o.output);
    }

    fn to_json(self) -> Value {
        json!({"input": self.input, "cache_creation": self.cache_creation, "cache_read": self.cache_read, "output": self.output})
    }

    fn from_json(v: &Value) -> Usage {
        let n = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
        Usage { input: n("input"), cache_creation: n("cache_creation"), cache_read: n("cache_read"), output: n("output") }
    }
}

/// Why a call failed, as far as anyone can tell. Decides how long to back off and what the notice says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailKind {
    NotSignedIn,
    RateLimited,
    Network,
    TimedOut,
    Other,
}

impl FailKind {
    pub fn name(self) -> &'static str {
        match self {
            FailKind::NotSignedIn => "not_signed_in",
            FailKind::RateLimited => "rate_limited",
            FailKind::Network => "network",
            FailKind::TimedOut => "timed_out",
            FailKind::Other => "other",
        }
    }

    fn from_name(s: &str) -> FailKind {
        [FailKind::NotSignedIn, FailKind::RateLimited, FailKind::Network, FailKind::TimedOut]
            .into_iter()
            .find(|k| k.name() == s)
            .unwrap_or(FailKind::Other)
    }

    /// The reason in plain words, for after "Smart features paused: ".
    pub fn plain(self) -> &'static str {
        match self {
            FailKind::NotSignedIn => "Claude isn't signed in",
            FailKind::RateLimited => "your Claude plan limit was reached",
            FailKind::Network => "Claude can't be reached (is the internet down?)",
            FailKind::TimedOut => "Claude took too long to answer",
            FailKind::Other => "Claude returned an error",
        }
    }

    /// (first delay, longest delay) in seconds. Being signed out or over the plan limit doesn't fix itself in
    /// a minute, so those wait much longer between tries.
    pub fn delays(self) -> (u64, u64) {
        match self {
            FailKind::NotSignedIn => (30 * 60, 2 * 3600),
            FailKind::RateLimited => (15 * 60, 2 * 3600),
            FailKind::Network | FailKind::TimedOut | FailKind::Other => (30, 30 * 60),
        }
    }
}

/// Calls are refused until `until` (seconds since the epoch). `since` is when the current run of failures began
/// (a success ends it), so a notice can be shown once per run rather than once per failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backoff {
    pub failures: u32,
    pub since: u64,
    pub until: u64,
    pub kind: FailKind,
}

/// One feature's share of a day.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeatureDay {
    pub tokens: u64,
    pub calls: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Day {
    pub usage: Usage,
    pub cost_usd: f64,
    pub calls: u64,
    /// Of `calls`, how many ended in a failure.
    pub failed: u64,
    pub features: BTreeMap<String, FeatureDay>,
}

impl Day {
    pub fn tokens(&self) -> u64 {
        self.usage.total()
    }

    fn to_json(&self) -> Value {
        let mut o = Map::new();
        o.insert("usage".into(), self.usage.to_json());
        o.insert("cost_usd".into(), json!(self.cost_usd));
        o.insert("calls".into(), json!(self.calls));
        o.insert("failed".into(), json!(self.failed));
        let f: Map<String, Value> = self.features.iter().map(|(k, v)| (k.clone(), json!({"tokens": v.tokens, "calls": v.calls}))).collect();
        o.insert("features".into(), Value::Object(f));
        Value::Object(o)
    }

    fn from_json(v: &Value) -> Day {
        let features = v
            .get("features")
            .and_then(Value::as_object)
            .map(|o| {
                o.iter()
                    .map(|(k, f)| {
                        let n = |key: &str| f.get(key).and_then(Value::as_u64).unwrap_or(0);
                        (k.clone(), FeatureDay { tokens: n("tokens"), calls: n("calls") })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Day {
            usage: v.get("usage").map(Usage::from_json).unwrap_or_default(),
            cost_usd: v.get("cost_usd").and_then(Value::as_f64).filter(|c| c.is_finite() && *c >= 0.0).unwrap_or(0.0),
            calls: v.get("calls").and_then(Value::as_u64).unwrap_or(0),
            failed: v.get("failed").and_then(Value::as_u64).unwrap_or(0),
            features,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Data {
    /// Keyed by local calendar day, `YYYY-MM-DD` (so key order is date order).
    pub days: BTreeMap<String, Day>,
    pub backoff: Option<Backoff>,
}

impl Data {
    fn to_json(&self) -> Value {
        let days: Map<String, Value> = self.days.iter().map(|(k, d)| (k.clone(), d.to_json())).collect();
        let mut o = Map::new();
        o.insert("version".into(), json!(1));
        o.insert("days".into(), Value::Object(days));
        if let Some(b) = &self.backoff {
            o.insert("backoff".into(), json!({"failures": b.failures, "since": b.since, "until": b.until, "kind": b.kind.name()}));
        }
        Value::Object(o)
    }

    /// None when the text isn't a JSON object.
    fn parse(text: &str) -> Option<Data> {
        let v: Value = serde_json::from_str(text).ok()?;
        let o = v.as_object()?;
        let days = o
            .get("days")
            .and_then(Value::as_object)
            .map(|d| d.iter().filter(|(k, _)| is_day(k)).map(|(k, v)| (k.clone(), Day::from_json(v))).collect())
            .unwrap_or_default();
        let backoff = o.get("backoff").and_then(Value::as_object).and_then(|b| {
            Some(Backoff {
                failures: b.get("failures").and_then(Value::as_u64).unwrap_or(1).clamp(1, 1000) as u32,
                since: b.get("since").and_then(Value::as_u64)?,
                until: b.get("until").and_then(Value::as_u64)?,
                kind: FailKind::from_name(b.get("kind").and_then(Value::as_str).unwrap_or("")),
            })
        });
        Some(Data { days, backoff })
    }
}

fn is_day(k: &str) -> bool {
    let b = k.as_bytes();
    b.len() == 10 && b.iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
}

/// The local calendar day of a moment (seconds since the epoch), `YYYY-MM-DD`. "Today" for the daily limit is
/// this, so the count starts over at the user's midnight. (Falls back to UTC if the system can't say.)
pub fn local_day(epoch: u64) -> String {
    let t = epoch as libc::time_t;
    // SAFETY: `localtime_r` writes only into the `tm` we hand it.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return utc_day(epoch);
        }
        tm
    };
    format!("{:04}-{:02}-{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday)
}

fn utc_day(epoch: u64) -> String {
    // civil-from-days (Howard Hinnant)
    let z = (epoch / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

pub struct Ledger {
    path: PathBuf,
    lock: PathBuf,
}

impl Ledger {
    pub fn open(cache: &Path) -> Ledger {
        Ledger { path: cache.join("ai-usage.json"), lock: cache.join("ai-usage.lock") }
    }

    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The saved numbers. A missing or unreadable file is an empty ledger, and reading creates nothing.
    pub fn load(&self) -> Data {
        fs::read_to_string(&self.path).ok().and_then(|t| Data::parse(&t)).unwrap_or_default()
    }

    /// Change the saved data and write it back, holding the lock so that another platoon can't slip in between.
    pub fn update(&self, change: impl FnOnce(&mut Data)) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let lock = fs::OpenOptions::new().create(true).truncate(false).write(true).open(&self.lock)?;
        // SAFETY: a plain advisory lock on a file we hold open; it is released when `lock` is dropped.
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) };
        let mut data = self.load();
        change(&mut data);
        while data.days.len() > KEEP_DAYS {
            let oldest = data.days.keys().next().cloned().unwrap_or_default();
            data.days.remove(&oldest);
        }
        let tmp = self.path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(&data.to_json()).unwrap_or_default();
        let written = fs::write(&tmp, text).and_then(|()| fs::rename(&tmp, &self.path));
        if written.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        written
    }

    /// The totals for the local day `now` falls on.
    pub fn today(&self, now: u64) -> Day {
        self.load().days.remove(&local_day(now)).unwrap_or_default()
    }

    /// Add a finished call to today's totals. `ok` is false for a call that failed but still reported usage.
    pub fn record_call(&self, now: u64, feature: &str, usage: &Usage, cost_usd: f64, ok: bool) -> io::Result<()> {
        self.update(|d| {
            let day = d.days.entry(local_day(now)).or_default();
            day.usage.add(usage);
            day.cost_usd += if cost_usd.is_finite() && cost_usd > 0.0 { cost_usd } else { 0.0 };
            day.calls += 1;
            day.failed += u64::from(!ok);
            let f = day.features.entry(feature.to_string()).or_default();
            f.tokens = f.tokens.saturating_add(usage.total());
            f.calls += 1;
        })
    }

    /// Tokens of today that went to archived conversations. Kept beside the per-feature numbers under a name of their
    /// own and NOT added to the day's total (the call that used them already was), so the share rule can tell what
    /// archived work has used.
    pub fn note_archived(&self, now: u64, tokens: u64) -> io::Result<()> {
        self.update(|d| {
            let f = d.days.entry(local_day(now)).or_default().features.entry(ARCHIVED_KEY.to_string()).or_default();
            f.tokens = f.tokens.saturating_add(tokens);
            f.calls += 1;
        })
    }

    /// The tokens archived conversations have used on the local day `now` falls on.
    pub fn archived_today(&self, now: u64) -> u64 {
        self.load().days.get(&local_day(now)).and_then(|d| d.features.get(ARCHIVED_KEY)).map_or(0, |f| f.tokens)
    }

    /// A call failed: refuse calls for a while, twice as long as after the previous failure in this run of them,
    /// up to a limit that depends on why. The run's start is kept, so the notice appears once.
    pub fn note_failure(&self, now: u64, kind: FailKind) -> io::Result<()> {
        self.update(|d| {
            let (failures, since) = d.backoff.map_or((1, now), |b| (b.failures.saturating_add(1), b.since));
            let (first, longest) = kind.delays();
            let delay = first.saturating_mul(1u64 << (failures - 1).min(20)).min(longest);
            d.backoff = Some(Backoff { failures, since, until: now + delay, kind });
        })
    }

    /// A call worked: the run of failures is over.
    pub fn note_success(&self) -> io::Result<()> {
        if self.load().backoff.is_none() {
            return Ok(());
        }
        self.update(|d| d.backoff = None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("platoon-ledger-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn u(input: u64, cc: u64, cr: u64, output: u64) -> Usage {
        Usage { input, cache_creation: cc, cache_read: cr, output }
    }

    // Moments days apart, so they are different calendar days in every time zone.
    const DAY1: u64 = 1_800_000_000;
    const DAY2: u64 = DAY1 + 3 * 86_400;

    #[test]
    fn all_four_numbers_count_and_calls_add_up_per_feature() {
        let d = dir("add");
        let l = Ledger::open(&d);
        assert_eq!(l.load(), Data::default());
        assert!(!d.exists(), "reading creates nothing");
        l.record_call(DAY1, "names", &u(400, 100, 3000, 20), 0.001, true).unwrap();
        l.record_call(DAY1 + 60, "names", &u(1, 2, 3, 4), 0.0005, true).unwrap();
        l.record_call(DAY1 + 90, "find", &u(10, 0, 0, 5), 0.0, false).unwrap();
        let t = l.today(DAY1 + 100);
        assert_eq!(t.usage, u(411, 102, 3003, 29));
        assert_eq!(t.tokens(), 411 + 102 + 3003 + 29);
        assert_eq!((t.calls, t.failed), (3, 1));
        assert!((t.cost_usd - 0.0015).abs() < 1e-9);
        assert_eq!(t.features["names"], FeatureDay { tokens: 3520 + 10, calls: 2 });
        assert_eq!(t.features["find"], FeatureDay { tokens: 15, calls: 1 });
    }

    #[test]
    fn a_new_day_starts_at_zero_and_yesterday_is_kept() {
        let d = dir("rollover");
        let l = Ledger::open(&d);
        l.record_call(DAY1, "names", &u(1000, 0, 0, 0), 0.0, true).unwrap();
        assert_eq!(l.today(DAY2).tokens(), 0, "tomorrow starts from nothing");
        l.record_call(DAY2, "names", &u(5, 0, 0, 0), 0.0, true).unwrap();
        assert_eq!(l.today(DAY2).tokens(), 5);
        assert_eq!(l.today(DAY1).tokens(), 1000, "yesterday is still there");
        assert_eq!(l.load().days.len(), 2);
    }

    #[test]
    fn only_the_newest_thirty_days_are_kept() {
        let d = dir("prune");
        let l = Ledger::open(&d);
        for i in 0..45u64 {
            l.record_call(DAY1 + i * 86_400 * 2, "names", &u(1, 0, 0, 0), 0.0, true).unwrap();
        }
        let data = l.load();
        assert_eq!(data.days.len(), KEEP_DAYS);
        assert!(data.days.contains_key(&local_day(DAY1 + 44 * 2 * 86_400)), "the newest is kept");
        assert!(!data.days.contains_key(&local_day(DAY1)), "the oldest is gone");
    }

    #[test]
    fn a_corrupt_file_starts_fresh_and_is_replaced_by_the_next_write() {
        let d = dir("corrupt");
        fs::create_dir_all(&d).unwrap();
        let l = Ledger::open(&d);
        for bad in ["{ nope", "[1]", "", "42", "\u{0}\u{0}"] {
            fs::write(l.path(), bad).unwrap();
            assert_eq!(l.load(), Data::default(), "{bad:?}");
            assert_eq!(l.today(DAY1).tokens(), 0);
        }
        fs::write(l.path(), "{ nope").unwrap();
        l.record_call(DAY1, "names", &u(7, 0, 0, 0), 0.0, true).unwrap();
        assert_eq!(l.today(DAY1).tokens(), 7);
        // Odd pieces inside a good file are ignored one by one, not fatal.
        fs::write(l.path(), r#"{"days":{"nonsense":{"usage":{"input":5}},"2026-01-02":{"usage":{"input":"many","output":3},"calls":"x"}},"backoff":{"until":"soon"}}"#).unwrap();
        let data = l.load();
        assert_eq!(data.days.len(), 1);
        assert_eq!(data.days["2026-01-02"].usage, u(0, 0, 0, 3));
        assert_eq!(data.backoff, None);
    }

    #[test]
    fn writes_are_atomic_and_leave_no_temp_file() {
        let d = dir("atomic");
        let l = Ledger::open(&d);
        l.record_call(DAY1, "names", &u(1, 0, 0, 0), 0.0, true).unwrap();
        assert!(!d.join("ai-usage.json.tmp").exists());
        let v: Value = serde_json::from_str(&fs::read_to_string(l.path()).unwrap()).unwrap();
        assert_eq!(v["version"], 1);
        // A failed write (the target is a directory) leaves nothing behind and reports the error.
        let d2 = dir("atomic-fail");
        fs::create_dir_all(d2.join("ai-usage.json")).unwrap();
        let l2 = Ledger::open(&d2);
        assert!(l2.record_call(DAY1, "names", &u(1, 0, 0, 0), 0.0, true).is_err());
        assert!(!d2.join("ai-usage.json.tmp").exists());
    }

    #[test]
    fn the_file_never_holds_anything_but_numbers_and_names() {
        let d = dir("nothing-else");
        let l = Ledger::open(&d);
        l.record_call(DAY1, "names", &u(1, 2, 3, 4), 0.5, true).unwrap();
        l.note_failure(DAY1, FailKind::RateLimited).unwrap();
        let text = fs::read_to_string(l.path()).unwrap();
        for word in ["prompt", "reply", "result", "text"] {
            assert!(!text.contains(word), "{word} in {text}");
        }
    }

    #[test]
    fn backoff_doubles_up_to_a_limit_keeps_the_start_of_the_run_and_a_success_ends_it() {
        let d = dir("backoff");
        let l = Ledger::open(&d);
        let mut now = 1_000_000;
        let mut delays = Vec::new();
        for _ in 0..8 {
            l.note_failure(now, FailKind::Network).unwrap();
            let b = l.load().backoff.unwrap();
            delays.push(b.until - now);
            assert_eq!(b.since, 1_000_000, "the run began at the first failure");
            now = b.until;
        }
        assert_eq!(delays, [30, 60, 120, 240, 480, 960, 1800, 1800], "doubling, then capped at half an hour");
        assert_eq!(l.load().backoff.unwrap().failures, 8);
        l.note_success().unwrap();
        assert_eq!(l.load().backoff, None);
        l.note_failure(now + 5, FailKind::Other).unwrap();
        let b = l.load().backoff.unwrap();
        assert_eq!((b.failures, b.until - (now + 5), b.since), (1, 30, now + 5), "a success resets the growth");
    }

    #[test]
    fn being_signed_out_or_over_the_plan_limit_waits_longer() {
        let d = dir("backoff-long");
        let l = Ledger::open(&d);
        l.note_failure(0, FailKind::RateLimited).unwrap();
        assert_eq!(l.load().backoff.unwrap().until, 15 * 60);
        l.note_success().unwrap();
        l.note_failure(0, FailKind::NotSignedIn).unwrap();
        assert_eq!(l.load().backoff.unwrap().until, 30 * 60);
        let mut now = 0;
        let mut last = 0;
        for _ in 0..10 {
            l.note_failure(now, FailKind::NotSignedIn).unwrap();
            let until = l.load().backoff.unwrap().until;
            last = until - now;
            now = until;
        }
        assert_eq!(last, 2 * 3600, "capped at two hours");
        for k in [FailKind::Network, FailKind::TimedOut, FailKind::Other] {
            let (first, longest) = k.delays();
            assert!(first < FailKind::RateLimited.delays().0 && longest < FailKind::RateLimited.delays().1, "{k:?}");
        }
    }

    #[test]
    fn success_with_nothing_to_clear_writes_nothing() {
        let d = dir("success-quiet");
        Ledger::open(&d).note_success().unwrap();
        assert!(!d.exists());
    }

    #[test]
    fn the_local_day_is_a_date_and_the_utc_fallback_is_right() {
        assert_eq!(utc_day(0), "1970-01-01");
        assert_eq!(utc_day(1_800_000_000), "2027-01-15");
        assert_eq!(utc_day(951_782_400), "2000-02-29");
        let d = local_day(DAY1);
        assert!(is_day(&d), "{d}");
        assert_ne!(local_day(DAY1), local_day(DAY2));
        assert!(!is_day("2026-9-20") && !is_day("abcd-ef-gh") && is_day("2026-09-20"));
    }

    #[test]
    fn several_writers_at_once_lose_nothing() {
        let d = dir("threads");
        let l = std::sync::Arc::new(Ledger::open(&d));
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let l = l.clone();
                std::thread::spawn(move || {
                    for _ in 0..25 {
                        l.record_call(DAY1, "names", &u(1, 0, 0, 0), 0.0, true).unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(l.today(DAY1).tokens(), 100);
        assert_eq!(l.today(DAY1).calls, 100);
    }
}
