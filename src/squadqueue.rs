//! The limit on squads at work at once, and the line of tickets waiting for a free squad (decided 2026-09-26).
//!
//! A squad holds one of the places while its ticket is Working and the ticket's current leader session is running
//! (a leader waiting on the user, ready to publish, done or cancelled holds none). A leader written on its ticket a
//! moment ago counts as running for `STARTING_SECS`, until its process shows up (another platoon window may have
//! just started it). A ticket handed over while every place is taken joins the line: it is Working in the file with
//! a `queued` mark (see `tickets::Queued`), and the platoon app starts it when a place frees, oldest first. The
//! limit is a setting (`squads_at_once`); lowering it never stops a squad, new ones just wait.
//!
//! Everything here is pure: the app gives it the tickets and says which sessions are running.

use crate::tickets::{Queued, State, Ticket};

/// How long a leader written on its ticket holds a place before its process has shown up.
pub const STARTING_SECS: u64 = 60;

/// Does ticket `t`'s squad hold one of the places? `running`: is this session running now.
pub fn holds_place(t: &Ticket, running: &dyn Fn(&str) -> bool, now: u64) -> bool {
    t.state == State::Working
        && t.queued.is_none()
        && t.leader().is_some_and(|l| running(&l.session) || now.saturating_sub(l.started) < STARTING_SECS)
}

/// How many places are taken, across every project's tickets.
pub fn places_taken(all: &[(String, Ticket)], running: &dyn Fn(&str) -> bool, now: u64) -> usize {
    all.iter().filter(|(_, t)| holds_place(t, running, now)).count()
}

/// How many more squads may start now, with `taken` places taken (never below zero: a lowered limit just waits).
pub fn free_places(limit: u64, taken: usize) -> usize {
    usize::try_from(limit).unwrap_or(usize::MAX).saturating_sub(taken)
}

/// The line for a free squad, first in line first: oldest first, then by project and number (tickets that joined
/// in the same second keep a fixed order).
pub fn line(all: &[(String, Ticket)]) -> Vec<(String, u32, Queued)> {
    let mut out: Vec<(String, u32, Queued)> = all.iter().filter_map(|(p, t)| Some((p.clone(), t.number, t.queued.clone()?))).collect();
    out.sort_by(|a, b| (a.2.since, &a.0, a.1).cmp(&(b.2.since, &b.0, b.1)));
    out
}

/// What the user is told when a ticket joins the line.
pub fn queued_notice(number: u32, squad: &str, limit: u64) -> String {
    let places =
        if limit == 1 { "The one squad allowed at once is".to_string() } else { format!("All {limit} squads allowed at once are") };
    format!("{places} at work: #{number} waits for a free squad, and goes to the {squad} squad by itself when one is free.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tickets::{Leader, Ticket};

    fn tk(number: u32, state: State, leader: Option<(&str, u64)>, queued: Option<u64>) -> Ticket {
        Ticket {
            number,
            title: format!("t{number}"),
            file: String::new(),
            state,
            folders: vec![],
            blocked_by: vec![],
            created: 0,
            updated: 0,
            leaders: leader
                .map(|(s, started)| Leader {
                    squad: "coding".into(),
                    session: s.into(),
                    started,
                    names: Default::default(),
                    snapshot: None,
                })
                .into_iter()
                .collect(),
            worktrees: vec![],
            reason: String::new(),
            go_ahead: false,
            handover: None,
            permission: false,
            busy: None,
            queued: queued.map(|since| Queued { squad: "coding".into(), since, anyway: false }),
            from: None,
        }
    }

    #[test]
    fn only_a_working_ticket_whose_leader_runs_holds_a_place() {
        let now = 10_000;
        let running = |s: &str| s == "up";
        // Working with its leader running: yes. Every other state, with the same running leader: no.
        assert!(holds_place(&tk(1, State::Working, Some(("up", 1)), None), &running, now));
        for s in [State::New, State::Waiting, State::Ready, State::Done, State::Cancelled, State::Other("later".into())] {
            assert!(!holds_place(&tk(1, s.clone(), Some(("up", 1)), None), &running, now), "{s:?}");
        }
        // Working, but its leader isn't running (closed, or never continued): no.
        assert!(!holds_place(&tk(1, State::Working, Some(("down", 1)), None), &running, now));
        // ... unless it was written on the ticket a moment ago: its process may not have shown up yet.
        assert!(holds_place(&tk(1, State::Working, Some(("down", now - 5)), None), &running, now));
        assert!(!holds_place(&tk(1, State::Working, Some(("down", now - STARTING_SECS)), None), &running, now));
        // Working with no leader, or in the line (even with an earlier leader running): no.
        assert!(!holds_place(&tk(1, State::Working, None, None), &running, now));
        assert!(!holds_place(&tk(1, State::Working, Some(("up", 1)), Some(5)), &running, now));
    }

    #[test]
    fn places_are_counted_across_projects_and_a_lowered_limit_only_waits() {
        let running = |s: &str| s.starts_with("up");
        let all = vec![
            ("a".to_string(), tk(1, State::Working, Some(("up-1", 1)), None)),
            ("b".to_string(), tk(1, State::Working, Some(("up-2", 1)), None)),
            ("b".to_string(), tk(2, State::Waiting, Some(("up-3", 1)), None)),
            ("b".to_string(), tk(3, State::Working, None, Some(4))),
        ];
        assert_eq!(places_taken(&all, &running, 100), 2);
        assert_eq!(free_places(3, 2), 1);
        assert_eq!(free_places(2, 2), 0);
        // The limit was lowered below what runs: nothing is stopped, nothing new starts.
        assert_eq!(free_places(1, 2), 0);
        assert_eq!(free_places(10, 2), 8);
    }

    #[test]
    fn the_line_is_oldest_first_then_by_project_and_number() {
        let all = vec![
            ("b".to_string(), tk(2, State::Working, None, Some(50))),
            ("a".to_string(), tk(9, State::Working, None, Some(50))),
            ("a".to_string(), tk(3, State::Working, None, Some(50))),
            ("c".to_string(), tk(1, State::Working, None, Some(20))),
            ("c".to_string(), tk(4, State::Working, Some(("x", 1)), None)),
            ("c".to_string(), tk(5, State::New, None, None)),
        ];
        let order: Vec<(String, u32)> = line(&all).into_iter().map(|(p, n, _)| (p, n)).collect();
        assert_eq!(order, [("c".into(), 1), ("a".into(), 3), ("a".into(), 9), ("b".into(), 2)]);
    }

    #[test]
    fn the_notice_says_how_many_are_allowed() {
        assert_eq!(
            queued_notice(4, "Coding", 3),
            "All 3 squads allowed at once are at work: #4 waits for a free squad, and goes to the Coding squad by itself when one is free."
        );
        assert_eq!(
            queued_notice(2, "Docs", 1),
            "The one squad allowed at once is at work: #2 waits for a free squad, and goes to the Docs squad by itself when one is free."
        );
    }
}
