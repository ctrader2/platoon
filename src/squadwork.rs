//! What a ticket's squads have done, as their leaders' transcripts say (read by `claude::read_leader`, off the UI
//! thread): the tokens per ticket (in all, and per agent: the leader and each member by its agent name, summed over
//! every leader the ticket has had), and the members' runs in the order the leader started them. It is what the
//! leader actually did, not the workflow on paper. Pure: the app hands it what was read.

use crate::claude::{LeaderLog, MemberRun, Spent};

/// What a ticket's squads spent: in all, and per agent in the order they first worked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TicketTokens {
    pub total: Spent,
    pub agents: Vec<(String, Spent)>,
}

/// The tokens of a ticket from its leaders, oldest first: for each, the name its own agent goes by when the
/// transcript doesn't say (`coding-leader`, or `leader` for one started with the plain names), and what was read of
/// its transcript (None: not read, or not there). None when none of them has been read.
pub fn ticket_tokens(leaders: &[(String, Option<&LeaderLog>)]) -> Option<TicketTokens> {
    let mut out = TicketTokens::default();
    let mut any = false;
    let add = |out: &mut TicketTokens, name: &str, s: &Spent| match out.agents.iter_mut().find(|(n, _)| n == name) {
        Some((_, have)) => have.add(s),
        None => out.agents.push((name.to_string(), *s)),
    };
    for (default_name, log) in leaders {
        let Some(log) = log else { continue };
        any = true;
        let name = log.leader_agent.clone().unwrap_or_else(|| default_name.clone());
        add(&mut out, &name, &log.leader);
        for (member, s) in &log.members {
            add(&mut out, member, s);
        }
    }
    if !any {
        return None;
    }
    for (_, s) in &out.agents {
        out.total.add(s);
    }
    Some(out)
}

/// A count of tokens, short: 950, 12K, 1.2M (one decimal below ten).
pub fn short(n: u64) -> String {
    let one = |v: u64, unit: u64, suffix: &str| {
        let tenths = (v * 10 + unit / 2) / unit;
        if tenths < 100 && !tenths.is_multiple_of(10) {
            format!("{}.{}{suffix}", tenths / 10, tenths % 10)
        } else {
            format!("{}{suffix}", (v + unit / 2) / unit)
        }
    };
    match n {
        n if n < 1000 => n.to_string(),
        n if n < 999_500 => one(n, 1000, "K"),
        n => one(n, 1_000_000, "M"),
    }
}

/// What the short totals count: tokens sent to Claude and written by it. Cache reads (the conversation so far,
/// read back each turn) are left out: they are cheap and would make every total look huge. The ticket screen gives
/// them on their own.
pub fn sent_and_written(s: &Spent) -> u64 {
    s.input + s.output
}

/// The short total the Tickets pane shows on a ticket's row: "84K tokens" (sent and written, no cache reads).
pub fn pane_total(t: &TicketTokens) -> String {
    format!("{} tokens", short(sent_and_written(&t.total)))
}

/// The ticket screen's total: the pane's number, said as what it is, then the cache reads beside it:
/// "84K sent and written, plus 1.9M read again from Claude's cache".
pub fn screen_total(t: &TicketTokens) -> String {
    let main = format!("{} sent and written", short(sent_and_written(&t.total)));
    if t.total.cache_read == 0 {
        return main;
    }
    format!("{main}, plus {} read again from Claude's cache", short(t.total.cache_read))
}

/// What the numbers count, in plain words, with their parts.
pub fn explained(t: &TicketTokens) -> String {
    format!(
        "{} sent to Claude and {} written by Claude: the number the Tickets list shows. Read again from the cache is the \
         conversation so far, sent again each turn, counted apart. Each answer counted once; the members' own work included.",
        short(t.total.input),
        short(t.total.output),
    )
}

/// The ticket screen shows this many of the members' latest runs; the ones before are summed up in a line.
pub const RUNS_SHOWN: usize = 8;

/// Of `total` runs, how many earlier ones are left out, and the line that says so ("and 4 earlier").
pub fn earlier(total: usize) -> (usize, Option<String>) {
    let skip = total.saturating_sub(RUNS_SHOWN);
    (skip, (skip > 0).then(|| format!("and {skip} earlier")))
}

/// How a member's run stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    /// Its result came back.
    Finished,
    /// It came back saying it failed (or was stopped).
    Failed,
    /// No result yet, and its leader is running.
    Running,
    /// No result, and its leader isn't running: it will never report.
    Unfinished,
}

impl RunState {
    pub fn of(r: &MemberRun, leader_running: bool) -> RunState {
        match r.ended.as_deref() {
            Some("completed") => RunState::Finished,
            Some(_) => RunState::Failed,
            None if leader_running => RunState::Running,
            None => RunState::Unfinished,
        }
    }

    /// The plain mark before the run.
    pub fn mark(self) -> &'static str {
        match self {
            RunState::Finished | RunState::Failed => "[x]",
            RunState::Running => "[>]",
            RunState::Unfinished => "[-]",
        }
    }

    /// Said after the run, when it isn't simply finished or running.
    pub fn note(self) -> &'static str {
        match self {
            RunState::Failed => " (failed)",
            RunState::Unfinished => " (didn't finish)",
            _ => "",
        }
    }
}

/// One run as a line: "[x] coding-planner · Plan the change".
pub fn run_line(r: &MemberRun, leader_running: bool) -> (RunState, String) {
    let st = RunState::of(r, leader_running);
    let what = if r.description.is_empty() { String::new() } else { format!(" · {}", r.description) };
    (st, format!("{} {}{what}{}", st.mark(), r.agent, st.note()))
}

/// "Now: coding-reviewer": the members running now (several at once are named together), or None.
pub fn now_line(runs: &[MemberRun], leader_running: bool) -> Option<String> {
    let mut names: Vec<&str> = Vec::new();
    for r in runs.iter().filter(|r| RunState::of(r, leader_running) == RunState::Running) {
        if !names.contains(&r.agent.as_str()) {
            names.push(&r.agent);
        }
    }
    (!names.is_empty()).then(|| format!("Now: {}", names.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spent(input: u64, output: u64, cache_read: u64) -> Spent {
        Spent { input, output, cache_read, subagents: false }
    }

    fn run(agent: &str, what: &str, ended: Option<&str>) -> MemberRun {
        MemberRun { call: format!("c-{agent}-{what}"), agent: agent.into(), description: what.into(), ended: ended.map(String::from) }
    }

    #[test]
    fn tokens_are_summed_per_agent_across_every_leader_the_ticket_had() {
        let first = LeaderLog {
            leader_agent: Some("coding-leader".into()),
            leader: spent(10, 20, 300),
            members: vec![("coding-planner".into(), spent(100, 5, 1000)), ("coding-builder".into(), spent(50, 50, 0))],
            runs: vec![],
        };
        // A second leader of the same squad (handed over again after Done), and one that doesn't say its name.
        let second = LeaderLog {
            leader_agent: Some("coding-leader".into()),
            leader: spent(1, 2, 3),
            members: vec![("coding-builder".into(), spent(5, 5, 5))],
            runs: vec![],
        };
        let third = LeaderLog {
            leader_agent: None,
            leader: spent(7, 0, 0),
            members: vec![("docs-tech-writer".into(), spent(1, 1, 1))],
            runs: vec![],
        };
        let t = ticket_tokens(&[
            ("coding-leader".into(), Some(&first)),
            ("coding-leader".into(), Some(&second)),
            ("docs-leader".into(), Some(&third)),
        ])
        .unwrap();
        let agents: Vec<(&str, u64)> = t.agents.iter().map(|(n, s)| (n.as_str(), s.total())).collect();
        assert_eq!(
            agents,
            [
                ("coding-leader", 330 + 6),
                ("coding-planner", 1105),
                ("coding-builder", 100 + 15),
                ("docs-leader", 7),
                ("docs-tech-writer", 3)
            ]
        );
        assert_eq!(t.total.total(), 336 + 1105 + 115 + 7 + 3);
        assert_eq!(
            (t.total.input, t.total.output, t.total.cache_read),
            (10 + 100 + 50 + 1 + 5 + 7 + 1, 20 + 5 + 50 + 2 + 5 + 1, 300 + 1000 + 3 + 5 + 1)
        );
        // A leader whose transcript isn't there (yet) adds nothing; none read at all: nothing to say.
        let one = ticket_tokens(&[("coding-leader".into(), None), ("coding-leader".into(), Some(&second))]).unwrap();
        assert_eq!(one.total.total(), 6 + 15);
        assert_eq!(ticket_tokens(&[("coding-leader".into(), None)]), None);
        assert_eq!(ticket_tokens(&[]), None);
        // An empty transcript is zero, not nothing.
        assert_eq!(
            ticket_tokens(&[("leader".into(), Some(&LeaderLog::default()))]).map(|t| (t.total.total(), t.agents)),
            Some((0, vec![("leader".to_string(), Spent::default())]))
        );
    }

    #[test]
    fn counts_are_short_and_say_what_they_include() {
        let cases = [
            (0, "0"),
            (950, "950"),
            (1000, "1K"),
            (1234, "1.2K"),
            (12_345, "12K"),
            (999_499, "999K"),
            (999_500, "1M"),
            (1_200_000, "1.2M"),
            (1_250_000, "1.3M"),
            (12_600_000, "13M"),
        ];
        for (n, want) in cases {
            assert_eq!(short(n), want, "{n}");
        }
        let t = TicketTokens { total: spent(40_000, 44_000, 1_900_000), agents: vec![] };
        // The pane's number leaves the cache reads out; the screen says the same number, then the cache reads apart.
        assert_eq!(pane_total(&t), "84K tokens");
        assert_eq!(screen_total(&t), "84K sent and written, plus 1.9M read again from Claude's cache");
        assert_eq!(
            explained(&t),
            "40K sent to Claude and 44K written by Claude: the number the Tickets list shows. Read again from the cache is the conversation so far, sent again each turn, counted apart. Each answer counted once; the members' own work included."
        );
        // No cache reads: nothing to add.
        assert_eq!(screen_total(&TicketTokens { total: spent(900, 100, 0), agents: vec![] }), "1K sent and written");
        assert_eq!(sent_and_written(&spent(1, 2, 1000)), 3);
    }

    #[test]
    fn only_the_last_eight_runs_are_shown() {
        assert_eq!(earlier(0), (0, None));
        assert_eq!(earlier(8), (0, None));
        assert_eq!(earlier(9), (1, Some("and 1 earlier".into())));
        assert_eq!(earlier(12), (4, Some("and 4 earlier".into())));
    }

    #[test]
    fn runs_read_finished_running_and_what_a_stopped_leader_left() {
        let runs = [
            run("coding-planner", "Plan it", Some("completed")),
            run("coding-builder", "Build it", Some("completed")),
            run("coding-reviewer", "Review it", Some("completed")),
            run("coding-builder", "Fix what the review found", Some("failed")),
            run("coding-reviewer", "Review again", None),
        ];
        let lines: Vec<String> = runs.iter().map(|r| run_line(r, true).1).collect();
        assert_eq!(
            lines,
            [
                "[x] coding-planner · Plan it",
                "[x] coding-builder · Build it",
                "[x] coding-reviewer · Review it",
                "[x] coding-builder · Fix what the review found (failed)",
                "[>] coding-reviewer · Review again"
            ]
        );
        assert_eq!(now_line(&runs, true).as_deref(), Some("Now: coding-reviewer"));
        // The leader stopped: a run without a result never reports, and nothing runs now.
        assert_eq!(run_line(&runs[4], false).1, "[-] coding-reviewer · Review again (didn't finish)");
        assert_eq!(now_line(&runs, false), None);
        // Two at once are named together, each once.
        let two = [run("a", "1", None), run("b", "2", None), run("a", "3", None)];
        assert_eq!(now_line(&two, true).as_deref(), Some("Now: a, b"));
        assert_eq!(run_line(&run("x", "", Some("completed")), true).1, "[x] x");
    }
}
