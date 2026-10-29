//! The `/to-tasks` hint at the end of Claude Code's status line (`platoon statusline`): a few words, shown only while
//! a conversation has agreed on work that could become platoon tasks, never a question and never anything sent into
//! the conversation.
//!
//! Two signals say a conversation qualifies:
//! - **The notes' verdict**: the project-notes update (`notesrun::update_once`) asks, in the same call, which of the
//!   conversations it was shown have agreed on concrete work of several steps that is not yet tasks. Each conversation
//!   it asked about gets a mark here, `<cache>/tickets-hint/<session id>`: `yes <time>` or `no <time>`, the time the
//!   material was read. Only sessions of a project, after the notes' quiet wait.
//! - **An approved plan**: an `ExitPlanMode` the user said yes to, near the end of the transcript
//!   (`claude::to_tickets_tail`).
//!
//! It goes away once the skill is used in the conversation after the signal (`/to-tasks`, or its old name
//! `/to-tickets`), when a later notes verdict about it is no, and `LIFE_SECS` after the signal. Never in a task's
//! session (`PLATOON_TASK` set: the caller doesn't ask, and a session a task records is refused here too), never
//! without the skill installed for Claude Code, never in Codex. It names the skill as installed (`skill::command`).

use crate::agents::claude::ToTicketsTail;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// How long a signal keeps the hint up.
pub const LIFE_SECS: u64 = 30 * 60;

/// The hint, as the status line ends with it, after the skill's command (`skill::command`: the one installed).
pub const HINT: &str = "turns this into tasks";

const DIR: &str = "tickets-hint";

/// What the notes update said about a conversation, and when (seconds: when the material it judged was read).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    pub qualifies: bool,
    pub at: u64,
}

impl Mark {
    fn text(self) -> String {
        format!("{} {}", if self.qualifies { "yes" } else { "no" }, self.at)
    }

    fn parse(text: &str) -> Option<Mark> {
        let (word, at) = text.trim().split_once(' ')?;
        let qualifies = match word {
            "yes" => true,
            "no" => false,
            _ => return None,
        };
        Some(Mark { qualifies, at: at.trim().parse().ok()? })
    }
}

/// The mark's file for `session`; None for an id that can't be a file name.
fn path(cache: &Path, session: &str) -> Option<PathBuf> {
    crate::sessionhook::safe_name(session).then(|| cache.join(DIR).join(session))
}

pub fn read(cache: &Path, session: &str) -> Option<Mark> {
    Mark::parse(&fs::read_to_string(path(cache, session)?).ok()?)
}

/// Record what one notes update said: `verdicts` are the sessions it was asked about, each with its answer. Marks that
/// no longer matter (older than `LIFE_SECS`, or unreadable) are tidied away at the same time.
pub fn record(cache: &Path, verdicts: &[(String, bool)], at: u64, now: u64) {
    for (session, qualifies) in verdicts {
        if let Some(p) = path(cache, session) {
            let _ = crate::projects::write_atomic(&p, &Mark { qualifies: *qualifies, at }.text());
        }
    }
    for e in fs::read_dir(cache.join(DIR)).into_iter().flatten().flatten() {
        let stale = fs::read_to_string(e.path()).ok().and_then(|t| Mark::parse(&t)).is_none_or(|m| now.saturating_sub(m.at) >= LIFE_SECS);
        if stale && e.file_type().is_ok_and(|t| t.is_file()) {
            let _ = fs::remove_file(e.path());
        }
    }
}

/// Is the hint due, from the notes' mark and the transcript's end, at `now`? The newest signal still alive counts: a
/// yes from the notes, or an approved plan no later notes verdict said no to. The skill used at or after it puts
/// it away.
pub fn due(mark: Option<Mark>, tail: &ToTicketsTail, now: u64) -> bool {
    let alive = |at: u64| now.saturating_sub(at) < LIFE_SECS;
    let from_notes = mark.filter(|m| m.qualifies && alive(m.at)).map(|m| m.at);
    let from_plan = tail.plan_approved.filter(|&at| alive(at) && !mark.is_some_and(|m| !m.qualifies && m.at >= at));
    let Some(signal) = from_notes.max(from_plan) else { return false };
    tail.to_tickets_used.is_none_or(|used| used < signal)
}

/// `line` with the hint for the skill typed as `command` at its end, within `width` characters: the hint gives way
/// first, to the command alone and then altogether.
pub fn with_hint(line: &str, command: &str, width: usize) -> String {
    for hint in [format!("{command} {HINT}"), command.to_string()] {
        let joined = if line.is_empty() { hint.to_string() } else { format!("{line} · {hint}") };
        if joined.chars().count() <= width {
            return joined;
        }
    }
    line.to_string()
}

/// Where the hint looks: platoon's config and cache, and Claude Code's folder (for the skill).
pub struct Places<'a> {
    pub config: &'a Path,
    pub cache: &'a Path,
    pub claude_root: &'a Path,
}

/// Should the status line of the session Claude Code's status-line JSON `input` describes carry the hint, and naming
/// which command (the skill as installed: `skill::command`)? Only for a session that is no task's, with the skill
/// installed and Claude Code not switched off in platoon's settings. Reads one small mark, the skill's files and the
/// transcript's last `TO_TICKETS_TAIL_BYTES`; anything unreadable is no.
pub fn wanted(input: &str, at: &Places, now: u64) -> Option<&'static str> {
    let v = serde_json::from_str::<serde_json::Value>(input).ok()?;
    let session = v.get("session_id").and_then(serde_json::Value::as_str).filter(|s| crate::sessionhook::safe_name(s))?;
    if !crate::skill::present(at.claude_root) {
        return None;
    }
    let tail = v
        .get("transcript_path")
        .and_then(serde_json::Value::as_str)
        .filter(|p| !p.is_empty())
        .map(|p| crate::agents::claude::to_tickets_tail(Path::new(p)))
        .unwrap_or_default();
    let wanted = due(read(at.cache, session), &tail, now)
        && crate::tickets::find_session(at.config, session).is_none()
        && !crate::agents::switched_off(at.config, crate::agents::Agent::Claude);
    wanted.then(|| crate::skill::command(at.claude_root))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;

    fn tail(plan: Option<u64>, used: Option<u64>) -> ToTicketsTail {
        ToTicketsTail { plan_approved: plan, to_tickets_used: used }
    }

    fn yes(at: u64) -> Option<Mark> {
        Some(Mark { qualifies: true, at })
    }

    fn no(at: u64) -> Option<Mark> {
        Some(Mark { qualifies: false, at })
    }

    #[test]
    fn a_fresh_signal_shows_the_hint_until_it_is_used_said_no_to_or_too_old() {
        let none = ToTicketsTail::default();
        assert!(!due(None, &none, NOW), "nothing at all");
        assert!(due(yes(NOW - 60), &none, NOW), "the notes said yes a minute ago");
        assert!(!due(no(NOW - 60), &none, NOW));
        assert!(due(yes(NOW - LIFE_SECS + 1), &none, NOW));
        assert!(!due(yes(NOW - LIFE_SECS), &none, NOW), "30 minutes on");
        assert!(due(None, &tail(Some(NOW - 60), None), NOW), "a plan approved a minute ago");
        assert!(!due(None, &tail(Some(NOW - LIFE_SECS), None), NOW));
        // The skill used after the signal puts it away; a newer signal brings it back.
        assert!(!due(yes(NOW - 60), &tail(None, Some(NOW - 30)), NOW));
        assert!(!due(None, &tail(Some(NOW - 60), Some(NOW - 60)), NOW));
        assert!(due(yes(NOW - 10), &tail(None, Some(NOW - 30)), NOW), "a later yes about new material");
        // A later no from the notes puts a plan's hint away; an earlier one doesn't.
        assert!(!due(no(NOW - 30), &tail(Some(NOW - 60), None), NOW));
        assert!(due(no(NOW - 90), &tail(Some(NOW - 60), None), NOW));
        // The newest signal counts: a plan after an old yes, the skill used between them.
        assert!(due(yes(NOW - 600), &tail(Some(NOW - 60), Some(NOW - 300)), NOW));
    }

    #[test]
    fn the_hint_gives_way_first_when_the_line_is_narrow() {
        let line = "5h 12% · 7d 40%";
        let hinted = format!("{line} · /to-tasks turns this into tasks");
        assert_eq!(with_hint(line, "/to-tasks", 120), hinted);
        let full = hinted.chars().count();
        assert_eq!(with_hint(line, "/to-tasks", full), hinted);
        assert_eq!(with_hint(line, "/to-tasks", full - 1), format!("{line} · /to-tasks"));
        assert_eq!(with_hint(line, "/to-tasks", line.chars().count() + 3), line, "no room even for the short one");
        assert_eq!(with_hint("", "/to-tasks", 120), "/to-tasks turns this into tasks", "no usage figures: the hint alone");
        // The skill by its old name while only that is installed.
        assert_eq!(with_hint(line, "/to-tickets", 120), format!("{line} · /to-tickets turns this into tasks"));
    }

    #[test]
    fn marks_are_written_read_and_tidied_and_a_bad_id_writes_nothing() {
        let cache = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-tickets-hint-{}", std::process::id())));
        let _ = fs::remove_dir_all(&cache);
        record(&cache, &[("s-1".into(), true), ("s-2".into(), false), ("../x".into(), true)], NOW - 100, NOW);
        assert_eq!(read(&cache, "s-1"), yes(NOW - 100));
        assert_eq!(read(&cache, "s-2"), no(NOW - 100));
        assert_eq!(read(&cache, "../x"), None);
        assert!(!cache.join("x").exists());
        fs::write(cache.join(DIR).join("junk"), "maybe").unwrap();
        record(&cache, &[("s-3".into(), true)], NOW, NOW + LIFE_SECS - 50);
        let mut left: Vec<String> =
            fs::read_dir(cache.join(DIR)).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["s-3"], "the old marks and the unreadable one are gone");
        let _ = fs::remove_dir_all(&cache);
    }
}
