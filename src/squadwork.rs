//! What a ticket's squads have done, as their leaders' transcripts say (read by `claude::read_leader`, off the UI
//! thread): the tokens per ticket (in all, and per agent: the leader and each member by its agent name, summed over
//! every leader the ticket has had), the members' runs in the order the leader started them, drawn as a timeline (each
//! run's bar within the squad's time so far, how long it took, done / now / stopped), and what ran: the commands the
//! squad ran in the shell beside platoon's own steps (the copy made, the squad started), grouped by run. It is what the
//! leader actually did, not the workflow on paper. Pure: the app hands it what was read.

use crate::claude::{LeaderLog, MemberRun, Spent};
use crate::tickets::{Leader, Ticket};

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
        "{} sent to Claude and {} written by Claude: the number the Tasks list shows. Read again from the cache is the \
         conversation so far, sent again each turn, counted apart. Each answer counted once; the members' own work included.",
        short(t.total.input),
        short(t.total.output),
    )
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
}

// ---- the timeline ---------------------------------------------------------------------------------------------

/// An agent's name as a squad's lines say it: without its squad's id in front (`coding-planner` is `planner`).
pub fn agent_label(agent: &str, squad: &str) -> String {
    agent.strip_prefix(squad).and_then(|r| r.strip_prefix('-')).filter(|r| !r.is_empty()).unwrap_or(agent).to_string()
}

impl RunState {
    /// The word at the end of its timeline line: done, now, stopped.
    pub fn word(self) -> &'static str {
        match self {
            RunState::Finished => "done",
            RunState::Running => "now",
            RunState::Failed | RunState::Unfinished => "stopped",
        }
    }
}

/// What a step that hasn't run yet says on the timeline.
pub const NOT_YET: &str = "not yet";

/// One line of a squad's timeline: a member's run, or a step of its workflow that hasn't run yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lane {
    /// The agent, as the squad's lines say it (`planner`).
    pub agent: String,
    /// None: it hasn't run yet.
    pub state: Option<RunState>,
    /// How long it took (or has taken so far), when its times are known.
    pub took: Option<u64>,
    /// Where its bar starts and how long it is, in cells of the width asked for; None when its times aren't known.
    pub bar: Option<(usize, usize)>,
}

/// A squad's timeline: one lane per member run, in the order the leader started them, each with its bar placed within
/// the squad's time so far (`from`..`to`, ms), `width` cells wide; then, when `steps` are given (the workflow's agents,
/// by the names its runs go by), one lane for each of those that hasn't run yet.
pub fn timeline(runs: &[MemberRun], running: bool, squad: &str, steps: &[String], from: u64, to: u64, width: usize) -> Vec<Lane> {
    let span = to.saturating_sub(from).max(1);
    let cell = |t: u64| (t.saturating_sub(from) as u128 * width as u128 / span as u128) as usize;
    let mut out: Vec<Lane> = runs
        .iter()
        .map(|r| {
            let state = RunState::of(r, running);
            let end = match state {
                RunState::Finished | RunState::Failed => r.ended_at,
                RunState::Running => Some(to.max(r.started_at.unwrap_or(to))),
                RunState::Unfinished => None,
            };
            let took = r.started_at.zip(end).map(|(s, e)| e.saturating_sub(s));
            let bar = r.started_at.filter(|_| width > 0).map(|s| {
                let at = cell(s).min(width - 1);
                let len = end.map_or(1, |e| cell(e).saturating_sub(cell(s))).max(1).min(width - at);
                (at, len)
            });
            Lane { agent: agent_label(&r.agent, squad), state: Some(state), took, bar }
        })
        .collect();
    for step in steps {
        if !runs.iter().any(|r| &r.agent == step) {
            out.push(Lane { agent: agent_label(step, squad), state: None, took: None, bar: None });
        }
    }
    out
}

/// How long a run took: "40s", "4m 20s", "1h 05m".
pub fn took(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m {:02}s", s / 60, s % 60),
        _ => format!("{}h {:02}m", s / 3600, s % 3600 / 60),
    }
}

// ---- what ran -----------------------------------------------------------------------------------------------------

/// The most of a task's commands that are kept (its latest), over all its squads.
pub const COMMANDS_PER_TASK: usize = 50;

/// The Tasks board shows this many of the latest things that ran.
pub const RAN_SHOWN: usize = 8;

/// Something that ran for a task: a command its squad ran in the shell, or one of platoon's own steps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ran {
    /// When (ms since the epoch), if known.
    pub at: Option<u64>,
    /// What it is grouped by: the member run, the leader, or platoon.
    pub group: String,
    /// Who, as the group's heading says it: `planner`, `leader`, `platoon`.
    pub who: String,
    pub text: String,
}

/// Things that ran, under who ran them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RanGroup {
    pub who: String,
    pub lines: Vec<(Option<u64>, String)>,
}

/// Everything that ran for task `t`, oldest first: platoon's own steps from the task's record (each copy of a folder made
/// for it and what its work lands on, a new branch made in the user's own checkout, each squad started), and the
/// commands its squads ran in the shell (`logs`: each of its leaders with what was read of its transcript), the latest
/// `COMMANDS_PER_TASK` of those. `squad_name` names a squad by its id.
pub fn task_ran(t: &Ticket, logs: &[(&Leader, Option<&LeaderLog>)], squad_name: &dyn Fn(&str) -> String) -> Vec<Ran> {
    let platoon = |at: u64, text: String| Ran { at: Some(at * 1000), group: "platoon".into(), who: "platoon".into(), text };
    let mut steps: Vec<Ran> = Vec::new();
    if let Some(first) = t.leaders.first() {
        let lands = |folder: &str| {
            t.works_on_in(folder).map(|w| w.lands_on.trim().to_string()).filter(|l| !l.is_empty()).map(|l| format!(", from {l}"))
        };
        for w in &t.worktrees {
            let name = crate::squads::folder_name(&w.folder);
            let from = lands(&w.folder).unwrap_or_default();
            steps.push(platoon(first.started, format!("made a copy of {name} on {}{from}", w.branch)));
        }
        for f in t.folders.iter().filter(|f| t.branch_in_checkout(f)) {
            let from = lands(f).unwrap_or_default();
            steps.push(platoon(first.started, format!("made the branch {} in {}{from}", t.branch(), crate::squads::folder_name(f))));
        }
    }
    for l in &t.leaders {
        steps.push(platoon(l.started, format!("started the {} squad", squad_name(&l.squad))));
    }
    let mut commands: Vec<Ran> = Vec::new();
    for (l, log) in logs {
        let Some(log) = log else { continue };
        for c in &log.commands {
            let (group, who) = match &c.run {
                Some(call) => {
                    let agent = log.runs.iter().find(|r| &r.call == call).map_or("member", |r| r.agent.as_str());
                    (format!("run:{call}"), agent_label(agent, &l.squad))
                }
                None => (format!("leader:{}", l.session), "leader".to_string()),
            };
            commands.push(Ran { at: c.at, group, who, text: c.text.clone() });
        }
    }
    commands.sort_by_key(|r| r.at.unwrap_or(0));
    let skip = commands.len().saturating_sub(COMMANDS_PER_TASK);
    steps.extend(commands.into_iter().skip(skip));
    steps.sort_by_key(|r| r.at.unwrap_or(0));
    steps
}

/// The latest `last` of `ran` (all of them: None), grouped by who ran them: each group where its first line comes, its
/// lines in the order they ran.
pub fn grouped(ran: &[Ran], last: Option<usize>) -> Vec<RanGroup> {
    let skip = last.map_or(0, |n| ran.len().saturating_sub(n));
    let mut keys: Vec<&str> = Vec::new();
    let mut out: Vec<RanGroup> = Vec::new();
    for r in &ran[skip..] {
        match keys.iter().position(|k| *k == r.group) {
            Some(i) => out[i].lines.push((r.at, r.text.clone())),
            None => {
                keys.push(&r.group);
                out.push(RanGroup { who: r.who.clone(), lines: vec![(r.at, r.text.clone())] });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spent(input: u64, output: u64, cache_read: u64) -> Spent {
        Spent { input, output, cache_read, subagents: false }
    }

    fn run(agent: &str, what: &str, ended: Option<&str>) -> MemberRun {
        MemberRun {
            call: format!("c-{agent}-{what}"),
            agent: agent.into(),
            description: what.into(),
            ended: ended.map(String::from),
            ..Default::default()
        }
    }

    #[test]
    fn tokens_are_summed_per_agent_across_every_leader_the_ticket_had() {
        let first = LeaderLog {
            leader_agent: Some("coding-leader".into()),
            leader: spent(10, 20, 300),
            members: vec![("coding-planner".into(), spent(100, 5, 1000)), ("coding-builder".into(), spent(50, 50, 0))],
            runs: vec![],
            ..Default::default()
        };
        // A second leader of the same squad (handed over again after Done), and one that doesn't say its name.
        let second = LeaderLog {
            leader_agent: Some("coding-leader".into()),
            leader: spent(1, 2, 3),
            members: vec![("coding-builder".into(), spent(5, 5, 5))],
            runs: vec![],
            ..Default::default()
        };
        let third = LeaderLog {
            leader_agent: None,
            leader: spent(7, 0, 0),
            members: vec![("docs-tech-writer".into(), spent(1, 1, 1))],
            runs: vec![],
            ..Default::default()
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
            "40K sent to Claude and 44K written by Claude: the number the Tasks list shows. Read again from the cache is the conversation so far, sent again each turn, counted apart. Each answer counted once; the members' own work included."
        );
        // No cache reads: nothing to add.
        assert_eq!(screen_total(&TicketTokens { total: spent(900, 100, 0), agents: vec![] }), "1K sent and written");
        assert_eq!(sent_and_written(&spent(1, 2, 1000)), 3);
    }

    #[test]
    fn a_run_is_finished_failed_running_or_left_unfinished_by_a_stopped_leader() {
        let runs = [
            run("coding-planner", "Plan it", Some("completed")),
            run("coding-builder", "Fix what the review found", Some("failed")),
            run("coding-reviewer", "Review again", None),
        ];
        let states: Vec<RunState> = runs.iter().map(|r| RunState::of(r, true)).collect();
        assert_eq!(states, [RunState::Finished, RunState::Failed, RunState::Running]);
        // The leader stopped: a run without a result never reports.
        assert_eq!(RunState::of(&runs[2], false), RunState::Unfinished);
    }

    #[test]
    fn the_timeline_places_each_run_within_the_squads_time_and_lists_the_steps_not_run_yet() {
        assert_eq!(agent_label("coding-planner", "coding"), "planner");
        assert_eq!(agent_label("planner", "coding"), "planner", "a leader with the plain names");
        assert_eq!(agent_label("docs-editor", "coding"), "docs-editor", "another squad's agent keeps its name");
        let timed = |agent: &str, start: u64, end: Option<u64>, ended: Option<&str>| MemberRun {
            agent: agent.into(),
            call: format!("c-{agent}-{start}"),
            started_at: Some(start),
            ended_at: end,
            ended: ended.map(String::from),
            ..Default::default()
        };
        let runs = [
            timed("coding-planner", 0, Some(20_000), Some("completed")),
            timed("coding-builder", 20_000, Some(60_000), Some("completed")),
            timed("coding-reviewer", 60_000, None, None),
        ];
        let steps: Vec<String> = ["planner", "builder", "reviewer", "verifier"].iter().map(|a| format!("coding-{a}")).collect();
        let lanes = timeline(&runs, true, "coding", &steps, 0, 100_000, 10);
        type Got<'a> = (&'a str, Option<RunState>, Option<u64>, Option<(usize, usize)>);
        let got: Vec<Got> = lanes.iter().map(|l| (l.agent.as_str(), l.state, l.took, l.bar)).collect();
        assert_eq!(
            got,
            [
                ("planner", Some(RunState::Finished), Some(20_000), Some((0, 2))),
                ("builder", Some(RunState::Finished), Some(40_000), Some((2, 4))),
                ("reviewer", Some(RunState::Running), Some(40_000), Some((6, 4))),
                ("verifier", None, None, None),
            ]
        );
        // Its leader stopped: the open run never reports, and how long it took isn't known.
        let stopped = timeline(&runs, false, "coding", &[], 0, 100_000, 10);
        assert_eq!((stopped[2].state, stopped[2].took, stopped[2].bar), (Some(RunState::Unfinished), None, Some((6, 1))));
        assert_eq!(stopped.len(), 3, "no steps asked for: only the runs");
        // A run without times has no bar; a very short one still gets a cell.
        let odd = [
            MemberRun { agent: "coding-planner".into(), ended: Some("completed".into()), ..Default::default() },
            timed("x", 50, Some(51), Some("completed")),
        ];
        let lanes = timeline(&odd, true, "coding", &[], 0, 100_000, 10);
        assert_eq!((lanes[0].bar, lanes[0].took), (None, None));
        assert_eq!(lanes[1].bar, Some((0, 1)));
        assert_eq!(
            [RunState::Finished.word(), RunState::Running.word(), RunState::Failed.word(), RunState::Unfinished.word()],
            ["done", "now", "stopped", "stopped"]
        );
        assert_eq!((took(40_000), took(260_000), took(3_900_000)), ("40s".to_string(), "4m 20s".to_string(), "1h 05m".to_string()));
    }

    #[test]
    fn what_ran_puts_platoons_steps_beside_the_squads_commands_grouped_by_run() {
        use crate::claude::ShellCommand;
        use crate::tickets::{How, WorksOn, Worktree};
        let lead = |session: &str, started: u64| Leader {
            squad: "coding".into(),
            session: session.into(),
            started,
            names: Default::default(),
            snapshot: None,
        };
        let t = Ticket {
            number: 2,
            folders: vec!["/home/u/shop".into(), "/home/u/api".into()],
            worktrees: vec![Worktree {
                folder: "/home/u/shop".into(),
                path: "/home/u/shop-worktrees/task-2".into(),
                branch: "task/2".into(),
                ..Default::default()
            }],
            works_on: vec![
                WorksOn { folder: "/home/u/shop".into(), how: How::Worktree, lands_on: "main".into() },
                WorksOn { folder: "/home/u/api".into(), how: How::Branch, lands_on: "develop".into() },
            ],
            leaders: vec![lead("L1", 100)],
            ..Default::default()
        };
        let cmd = |at: u64, run: Option<&str>, text: &str| ShellCommand { at: Some(at), run: run.map(String::from), text: text.into() };
        let run = |call: &str, agent: &str| MemberRun { call: call.into(), agent: agent.into(), ..Default::default() };
        let log = LeaderLog {
            runs: vec![run("c1", "coding-planner"), run("c2", "coding-builder")],
            commands: vec![
                cmd(101_000, None, "git status"),
                cmd(102_000, Some("c1"), "rg basket"),
                cmd(103_000, Some("c2"), "cargo build"),
                cmd(104_000, Some("c1"), "git log -3"),
                cmd(105_000, Some("c2"), "cargo test"),
            ],
            ..Default::default()
        };
        let name = |id: &str| if id == "coding" { "Coding".to_string() } else { id.to_string() };
        let ran = task_ran(&t, &[(&t.leaders[0], Some(&log))], &name);
        let all: Vec<(Option<u64>, &str, &str)> = ran.iter().map(|r| (r.at, r.who.as_str(), r.text.as_str())).collect();
        assert_eq!(
            all,
            [
                (Some(100_000), "platoon", "made a copy of shop on task/2, from main"),
                (Some(100_000), "platoon", "made the branch task/2 in api, from develop"),
                (Some(100_000), "platoon", "started the Coding squad"),
                (Some(101_000), "leader", "git status"),
                (Some(102_000), "planner", "rg basket"),
                (Some(103_000), "builder", "cargo build"),
                (Some(104_000), "planner", "git log -3"),
                (Some(105_000), "builder", "cargo test"),
            ]
        );
        // Grouped by run, each group where it first comes, its lines in order.
        let groups = grouped(&ran, None);
        let shape: Vec<(&str, Vec<&str>)> =
            groups.iter().map(|g| (g.who.as_str(), g.lines.iter().map(|(_, l)| l.as_str()).collect())).collect();
        assert_eq!(
            shape,
            [
                (
                    "platoon",
                    vec![
                        "made a copy of shop on task/2, from main",
                        "made the branch task/2 in api, from develop",
                        "started the Coding squad"
                    ]
                ),
                ("leader", vec!["git status"]),
                ("planner", vec!["rg basket", "git log -3"]),
                ("builder", vec!["cargo build", "cargo test"]),
            ]
        );
        // The latest few only: the board's.
        let last: Vec<String> = grouped(&ran, Some(3)).into_iter().map(|g| g.who).collect();
        assert_eq!(last, ["builder", "planner"]);
        // A task keeps its latest fifty commands, over all its squads; platoon's own steps always.
        let many = LeaderLog { commands: (0..70).map(|k| cmd(200_000 + k, None, &format!("step {k}"))).collect(), ..Default::default() };
        let ran = task_ran(&t, &[(&t.leaders[0], Some(&many))], &name);
        assert_eq!(ran.iter().filter(|r| r.who == "leader").count(), COMMANDS_PER_TASK);
        assert_eq!(ran.iter().filter(|r| r.who == "platoon").count(), 3);
        assert_eq!(ran.iter().find(|r| r.who == "leader").map(|r| r.text.as_str()), Some("step 20"));
        // No squad yet: nothing ran.
        assert!(task_ran(&Ticket::default(), &[], &name).is_empty());
    }
}
