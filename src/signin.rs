//! When the user's Claude Code sign-in ends, and the notice that says so before platoon's smart features and task
//! checks (and the user's own sessions) start failing.
//!
//! Since Claude Code 2.1.318 every `claude -p` answer names the end of a stored claude.ai sign-in
//! (`credential_expires_at`, read by `claude::interpret`). Claude Code's own description of it: the end of the login's
//! refresh chain as stored at sign-in, which renewing the access token doesn't move; the server's date when it sent
//! one, else a 30-day estimate; the real end can be up to 3 days earlier; absent for every other credential (an API
//! key, a token from the environment, another provider). So one date stands for one sign-in.
//!
//! Every answer platoon gets is noted here by the runner (`ai::Runner`), whichever process made the call: the app,
//! the notes helper, a task's check in its hook. The newest is kept in `<cache>/sign-in.json` (`{"ends": secs}`), and
//! an answer with no date takes it away. The app reads it at each refresh and, once the earliest end is a week away
//! (`WARN_BEFORE`: when Claude Code's own `claude -p` starts warning), says so once per sign-in where the version
//! notice shows (`App::news_showing`), remembered in `<cache>/sign-in-notice.json` (`{"said": secs}`) by the date it
//! was said for. Two files, so the calls and the app never write the same one.

use serde_json::{Value, json};
use std::path::Path;

const FILE: &str = "sign-in.json";
const SAID_FILE: &str = "sign-in-notice.json";

const DAY: u64 = 86_400;
/// How much earlier than its date a sign-in may end (Claude Code's own margin).
pub const EARLY_BY: u64 = 3 * DAY;
/// How long before its date the notice is given: a week before the earliest end. Claude Code's `claude -p` starts its
/// own warning then, so the two agree.
pub const WARN_BEFORE: u64 = 7 * DAY + EARLY_BY;

/// What one answer said about the sign-in it was made with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignIn {
    /// Nothing can be told: the answer couldn't be read, or its agent never says (Codex). What was known stays.
    Unknown,
    /// No end is known: an API key, a token from the environment, another provider, or a Claude Code before 2.1.318.
    NoEnd,
    /// It ends at this moment (seconds since the epoch), or up to `EARLY_BY` before it.
    EndsAt(u64),
}

/// The end recorded last, if any.
pub fn recorded(cache: &Path) -> Option<u64> {
    read(&cache.join(FILE), "ends")
}

/// Keep what an answer said: a date replaces the one recorded, no date takes it away. Written only when it changes.
pub fn note(cache: &Path, said: SignIn) {
    let path = cache.join(FILE);
    match said {
        SignIn::Unknown => {}
        SignIn::NoEnd => {
            if path.exists() {
                let _ = std::fs::remove_file(&path);
            }
        }
        SignIn::EndsAt(t) if recorded(cache) != Some(t) => {
            let _ = crate::projects::write_atomic(&path, &json!({ "ends": t }).to_string());
        }
        SignIn::EndsAt(_) => {}
    }
}

/// A notice to give about the sign-in that ends at `ends`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    ends: u64,
    text: String,
}

impl Notice {
    /// What the notice says.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The sign-in it is about, by its date.
    pub fn ends(&self) -> u64 {
        self.ends
    }
}

/// The notice for a sign-in that ends at `ends`, at `now`, with days in the user's own calendar (`day`): none until
/// `WARN_BEFORE`; then the earliest and the latest day it can end; any day now once the earliest has passed; and once
/// its date has passed, that it may have ended.
pub fn due(ends: Option<u64>, now: u64, day: impl Fn(u64) -> String) -> Option<Notice> {
    let ends = ends?;
    if ends > now + WARN_BEFORE {
        return None;
    }
    let earliest = ends.saturating_sub(EARLY_BY);
    let text = if now >= ends {
        "Your Claude Code sign-in may have ended. If sessions, task checks or smart features stop working, run `claude auth login`."
            .to_string()
    } else if now >= earliest {
        format!(
            "Your Claude Code sign-in can end any day now (by {}). Run `claude auth login` to keep sessions, task checks and smart features working.",
            day(ends)
        )
    } else {
        format!(
            "Your Claude Code sign-in ends between {} and {}. Run `claude auth login` before then to keep sessions, task checks and smart features working.",
            day(earliest),
            day(ends)
        )
    };
    Some(Notice { ends, text })
}

/// `due` for what is recorded in `cache`, at `now`, in the user's time zone.
pub fn look(cache: &Path, now: u64) -> Option<Notice> {
    due(recorded(cache), now, local_day)
}

/// A day as the notice says it, in the user's time zone: "Nov 18".
pub fn local_day(epoch: u64) -> String {
    short_day(&crate::ledger::local_day(epoch))
}

/// `YYYY-MM-DD` as "Nov 18" (as it was, when it isn't a day).
fn short_day(ymd: &str) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let mut parts = ymd.split('-').skip(1).map(|p| p.parse::<usize>().ok());
    match (parts.next().flatten(), parts.next().flatten()) {
        (Some(m @ 1..=12), Some(d)) => format!("{} {d}", MONTHS[m - 1]),
        _ => ymd.to_string(),
    }
}

/// The notice for this sign-in was given before.
pub fn said(cache: &Path, n: &Notice) -> bool {
    read(&cache.join(SAID_FILE), "said") == Some(n.ends)
}

/// Remember that the notice for this sign-in was given.
pub fn remember(cache: &Path, n: &Notice) -> std::io::Result<()> {
    crate::projects::write_atomic(&cache.join(SAID_FILE), &json!({ "said": n.ends }).to_string())
}

fn read(path: &Path, key: &str) -> Option<u64> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    v.get(key)?.as_u64()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    /// 2026-11-21 12:00 UTC: noon, so its day is the same in nearly every time zone.
    const ENDS: u64 = 1_795_262_400;

    fn tmp(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-ut-signin-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn utc_day(epoch: u64) -> String {
        short_day(&crate::ledger::utc_day(epoch))
    }

    #[test]
    fn a_day_is_said_by_its_month_and_number() {
        assert_eq!(short_day("2026-11-08"), "Nov 8");
        assert_eq!(short_day("2027-01-31"), "Jan 31");
        assert_eq!(short_day("odd"), "odd");
        assert_eq!(utc_day(ENDS), "Nov 21");
    }

    #[test]
    fn nothing_is_said_until_the_earliest_end_is_a_week_away_then_the_days_any_day_now_and_may_have_ended() {
        let at = |now: u64| due(Some(ENDS), now, utc_day).map(|n| n.text().to_string());
        assert_eq!(due(None, ENDS, utc_day), None, "no date, nothing said");
        assert_eq!(at(ENDS - WARN_BEFORE - 1), None);
        assert_eq!(at(ENDS - 30 * DAY), None, "a new sign-in's estimate is a month away");
        let between = "Your Claude Code sign-in ends between Nov 18 and Nov 21. Run `claude auth login` before then to keep sessions, task checks and smart features working.";
        assert_eq!(at(ENDS - WARN_BEFORE).as_deref(), Some(between), "ten days before its date");
        assert_eq!(at(ENDS - EARLY_BY - 1).as_deref(), Some(between));
        let any_day = "Your Claude Code sign-in can end any day now (by Nov 21). Run `claude auth login` to keep sessions, task checks and smart features working.";
        assert_eq!(at(ENDS - EARLY_BY).as_deref(), Some(any_day));
        assert_eq!(at(ENDS - 1).as_deref(), Some(any_day));
        let ended =
            "Your Claude Code sign-in may have ended. If sessions, task checks or smart features stop working, run `claude auth login`.";
        assert_eq!(at(ENDS).as_deref(), Some(ended));
        assert_eq!(at(ENDS + 40 * DAY).as_deref(), Some(ended));
        assert_eq!(due(Some(ENDS), ENDS, utc_day).unwrap().ends(), ENDS);
    }

    #[test]
    fn the_newest_answer_decides_a_date_replaces_none_takes_it_away_and_unknown_leaves_it() {
        let cache = tmp("note");
        assert_eq!(recorded(&cache), None);
        note(&cache, SignIn::EndsAt(ENDS));
        assert_eq!(recorded(&cache), Some(ENDS));
        note(&cache, SignIn::Unknown);
        assert_eq!(recorded(&cache), Some(ENDS), "an answer that says nothing leaves it");
        note(&cache, SignIn::EndsAt(ENDS + 30 * DAY));
        assert_eq!(recorded(&cache), Some(ENDS + 30 * DAY), "signed in again");
        note(&cache, SignIn::NoEnd);
        assert_eq!(recorded(&cache), None, "an API key now: no date");
        assert!(!cache.join(FILE).exists());
        note(&cache, SignIn::NoEnd);
        fs::write(cache.join(FILE), "not json").unwrap();
        assert_eq!(recorded(&cache), None, "a file that can't be read is no date");
    }

    #[test]
    fn a_notice_is_remembered_by_its_sign_in_so_a_new_sign_in_is_said_again() {
        let cache = tmp("said");
        let first = due(Some(ENDS), ENDS - 5 * DAY, utc_day).unwrap();
        assert!(!said(&cache, &first));
        remember(&cache, &first).unwrap();
        assert!(said(&cache, &first));
        let later_words = due(Some(ENDS), ENDS - DAY, utc_day).unwrap();
        assert_ne!(later_words.text(), first.text());
        assert!(said(&cache, &later_words), "the same sign-in, said once whatever the words");
        let next = due(Some(ENDS + 30 * DAY), ENDS + 25 * DAY, utc_day).unwrap();
        assert!(!said(&cache, &next), "a new sign-in");
    }
}
