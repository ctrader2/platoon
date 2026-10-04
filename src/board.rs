//! The Tasks board (the Tasks tab, and the same rows in a project's Tasks pane): every task in play across all
//! projects and the ones in no project, grouped by what it needs from the user: **Needs you**, **Working**, **Can
//! start**, **Waits for**, **Later** (put aside with `h` until the squad says something new) and **Finished today**
//! (in a project's pane: **Done and cancelled**, all of them). Every row answers the same three questions in the same
//! places: line one says what it is (the project's badge, `#N`, the title, drawn by the UI) and at the right the
//! section's word for it (the kind of need, how long since its squad last wrote, when it finished); line two says who
//! has it and what it needs from the user, in the squad's own words where it has some. Pure: the app hands it the
//! tasks, what the squads' transcripts said and which sessions run.

use crate::claude::LeaderLog;
use crate::squadwork;
use crate::tickets::{self, State, Ticket};

/// A group of the board, in the order they are shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    NeedsYou,
    Working,
    CanStart,
    WaitsFor,
    /// Put aside by the user (`h`) while it needed them, until the squad says something new.
    Later,
    /// Done or Cancelled in the last day (the board).
    Finished,
    /// Done or Cancelled, whenever (a project's pane, folded).
    Closed,
}

impl Section {
    pub fn heading(self) -> &'static str {
        match self {
            Section::NeedsYou => "Needs you",
            Section::Working => "Working",
            Section::CanStart => "Can start",
            Section::WaitsFor => "Waits for",
            Section::Later => "Later",
            Section::Finished => "Finished today",
            Section::Closed => "Done and cancelled",
        }
    }
}

/// The board's sections, in order.
pub const BOARD: [Section; 6] =
    [Section::NeedsYou, Section::Working, Section::CanStart, Section::WaitsFor, Section::Later, Section::Finished];
/// A project's pane: the same, with all of its Done and Cancelled tasks at the end.
pub const PANE: [Section; 6] = [Section::NeedsYou, Section::Working, Section::CanStart, Section::WaitsFor, Section::Later, Section::Closed];

/// Finished today shows this many before its fold.
pub const FINISHED_SHOWN: usize = 5;

/// How long Done and Cancelled tasks stay on the board, in seconds.
pub const FINISHED_FOR: u64 = 24 * 3600;

/// What a task that Needs you needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// The squad asked something (it ended its turn with a question).
    Question,
    /// It wants to publish (push, open a merge request, a page): the user's go-ahead.
    GoAhead,
    /// It can't go on, or it stopped without saying why.
    Stuck,
    /// It is waiting at Claude's permission prompt for a command.
    Permission,
}

impl Need {
    /// The row's word at the right.
    pub fn words(self) -> &'static str {
        match self {
            Need::Question => "a question",
            Need::GoAhead => "a go-ahead",
            Need::Stuck => "stuck",
            Need::Permission => "your OK on a command",
        }
    }

    /// The heading of what it says, in the details beside the board.
    pub fn heading(self) -> &'static str {
        match self {
            Need::Question => "The squad asks",
            Need::GoAhead => "The squad wants to publish",
            Need::Stuck => "The squad is stuck",
            Need::Permission => "The squad waits for your OK",
        }
    }
}

/// What a task that Needs you needs, from what the squad left on it.
pub fn need_of(t: &Ticket) -> Option<Need> {
    if t.state != State::Waiting {
        return None;
    }
    let r = t.reason.as_str();
    Some(if t.permission {
        Need::Permission
    } else if t.publish_wanted {
        Need::GoAhead
    } else if r.starts_with("Stuck") || r.starts_with("The squad is stuck") || r == crate::squads::NO_LINE {
        Need::Stuck
    } else {
        Need::Question
    })
}

/// The squad's own words, after its name on a row: "asks: which page?", "wants to publish: push task/7", "is stuck:
/// the tests need a database", "waits for your OK on a command".
pub fn squad_words(t: &Ticket) -> String {
    let r = t.reason.trim();
    if t.permission || r == tickets::PERMISSION_REASON {
        return "waits for your OK on a command".into();
    }
    for (lead, says) in [("Asks: ", "asks: "), ("Stuck: ", "is stuck: "), ("Wants to publish: ", "wants to publish: ")] {
        if let Some(rest) = r.strip_prefix(lead) {
            return format!("{says}{rest}");
        }
    }
    if let Some(rest) = r.strip_prefix("The squad ") {
        return rest.trim_end_matches('.').to_string();
    }
    if r.is_empty() {
        return match need_of(t) {
            Some(Need::GoAhead) => "wants to publish something (see its session)".into(),
            Some(Need::Stuck) => "is stuck (see its session)".into(),
            _ => "has a question (see its session)".into(),
        };
    }
    let mut c = r.chars();
    match c.next() {
        // "Wants to publish something (see its session)." reads on after the name as it is, lower-cased.
        Some(first) if r.starts_with("Wants") => format!("{}{}", first.to_lowercase(), c.as_str().trim_end_matches('.')),
        _ => format!("says: {r}"),
    }
}

/// What the squad said, as its details show it under the need's heading: the question itself, what it wants to
/// publish, why it is stuck.
pub fn squad_said(t: &Ticket) -> String {
    let r = t.reason.trim();
    for lead in ["Asks: ", "Stuck: ", "Wants to publish: "] {
        if let Some(rest) = r.strip_prefix(lead) {
            return rest.to_string();
        }
    }
    if t.permission || r == tickets::PERMISSION_REASON {
        return "It is waiting at a command's permission prompt in its session. Open the session to answer it.".into();
    }
    r.to_string()
}

/// How a piece of a row is colored (the UI picks the theme's color).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Subtext,
    Dim,
    Warn,
    Busy,
    Ok,
    Accent,
}

/// A piece of a row's second line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Piece {
    pub text: String,
    pub tone: Tone,
}

fn piece(text: impl Into<String>, tone: Tone) -> Piece {
    Piece { text: text.into(), tone }
}

/// One task on the board.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// Its project's id (`tickets::NO_PROJECT_ID` for none).
    pub project: String,
    pub number: u32,
    pub title: String,
    pub section: Section,
    pub need: Option<Need>,
    /// The section's word for it, at the right of line one.
    pub right: String,
    pub right_tone: Tone,
    /// Line two.
    pub second: Vec<Piece>,
    pub updated: u64,
    /// The name of the squad that has it (or had it last), for the filter.
    pub squad: Option<String>,
}

impl Row {
    /// Line two as plain text.
    #[cfg(test)]
    pub fn second_text(&self) -> String {
        self.second.iter().map(|p| p.text.as_str()).collect()
    }
}

/// A section with its rows, sorted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub section: Section,
    pub rows: Vec<Row>,
}

/// What the board knows besides the tasks themselves.
pub trait Facts {
    /// Now, in seconds since the epoch.
    fn now(&self) -> u64;
    /// A squad's name by its id.
    fn squad_name(&self, id: &str) -> String;
    /// The agents a squad's workflow names (their ids), in order, each once; empty when the squad isn't there.
    fn steps(&self, squad: &str) -> Vec<String>;
    /// What was read of a leader session's transcript.
    fn log(&self, session: &str) -> Option<&LeaderLog>;
    /// Is that leader session running now?
    fn running(&self, session: &str) -> bool;
    /// Has the squad written a result for this task?
    fn has_result(&self, project: &str, number: u32) -> bool;
    /// A project's name by its id, for the filter ("No project" for none).
    fn project_name(&self, id: &str) -> String;
}

/// A task as the board is given it: its project's id, whether that project is archived, and the other tasks of its
/// project (for what it waits for).
pub struct In<'a> {
    pub project: &'a str,
    pub archived: bool,
    pub task: &'a Ticket,
    pub siblings: &'a [Ticket],
}

/// How long ago, in plain short words: "just now", "14 min", "3 h", "2 days". Empty when it isn't known.
pub fn since(now: u64, then: u64) -> String {
    if then == 0 {
        return String::new();
    }
    let s = now.saturating_sub(then);
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min", s / 60),
        3600..=86_399 => format!("{} h", s / 3600),
        86_400..=172_799 => "1 day".into(),
        _ => format!("{} days", s / 86_400),
    }
}

/// `since` as a time in the past: "14 min ago" ("just now" as it is).
pub fn ago(now: u64, then: u64) -> String {
    match since(now, then) {
        s if s.is_empty() || s == "just now" => s,
        s => format!("{s} ago"),
    }
}

/// The board's section for a task, or None when it isn't on the board (finished before today, or a New task of an
/// archived project). `pane`: a project's pane, where every Done and Cancelled task is under Done and cancelled.
pub fn section_of(t: &Ticket, siblings: &[Ticket], archived: bool, now: u64, pane: bool) -> Option<Section> {
    match &t.state {
        State::Waiting if t.is_aside() => Some(Section::Later),
        State::Waiting => Some(Section::NeedsYou),
        State::Done | State::Cancelled if pane => Some(Section::Closed),
        State::Done | State::Cancelled => (now.saturating_sub(t.updated) < FINISHED_FOR).then_some(Section::Finished),
        State::New if archived && !pane => None,
        State::New if tickets::waiting_for(t, siblings).is_empty() => Some(Section::CanStart),
        State::New => Some(Section::WaitsFor),
        State::Working | State::Other(_) => Some(Section::Working),
    }
}

/// The marks of a squad's steps on a Working row: "planner ✓", "builder ●", "verifier ·", from the runs read so far
/// (the latest run of each agent) and the squad's workflow (`steps`, by agent id). With no workflow (the squad isn't
/// there any more), the agents that ran, in order.
pub fn step_marks(log: &LeaderLog, squad: &str, names: tickets::AgentNames, steps: &[String], running: bool) -> Vec<(String, Tone)> {
    let mut agents: Vec<String> = steps.iter().map(|s| crate::squads::agent_name(squad, s, names)).collect();
    if agents.is_empty() {
        for r in &log.runs {
            if !agents.contains(&r.agent) {
                agents.push(r.agent.clone());
            }
        }
    }
    agents
        .iter()
        .map(|a| {
            let label = squadwork::agent_label(a, squad);
            let (mark, tone) = match log.runs.iter().rev().find(|r| &r.agent == a).map(|r| squadwork::RunState::of(r, running)) {
                Some(squadwork::RunState::Finished) => ("✓", Tone::Ok),
                Some(squadwork::RunState::Running) => ("●", Tone::Busy),
                Some(squadwork::RunState::Failed | squadwork::RunState::Unfinished) => ("✗", Tone::Warn),
                None => ("·", Tone::Dim),
            };
            (format!("{label} {mark}"), tone)
        })
        .collect()
}

/// The tokens a task's squads spent, short ("187K tokens"), once a leader's transcript has been read.
fn tokens_of(t: &Ticket, f: &dyn Facts) -> Option<String> {
    let leaders: Vec<(String, Option<&LeaderLog>)> =
        t.leaders.iter().map(|l| (crate::squads::agent_name(&l.squad, crate::squads::LEADER_ROLE, l.names), f.log(&l.session))).collect();
    squadwork::ticket_tokens(&leaders).map(|tk| squadwork::pane_total(&tk))
}

/// When the squad last wrote: the latest entry in its leader's transcripts (the members' too), else when it started.
pub fn last_wrote(t: &Ticket, f: &dyn Facts) -> u64 {
    let read = t.leaders.iter().filter_map(|l| f.log(&l.session)).filter_map(|log| log.last_at).max().map(|ms| ms / 1000);
    read.or_else(|| t.leader().map(|l| l.started)).unwrap_or(t.updated)
}

/// One task's row in `section`.
pub fn row(i: &In, section: Section, f: &dyn Facts) -> Row {
    let t = i.task;
    let now = f.now();
    let leader = t.leader();
    let squad = leader.map(|l| format!("{} squad", f.squad_name(&l.squad)));
    let need = need_of(t);
    let (mut right, mut right_tone) = (String::new(), Tone::Dim);
    let mut second: Vec<Piece> = Vec::new();
    let named = |words: String| match &squad {
        Some(s) => format!("{s} {words}"),
        None => {
            let mut c = words.chars();
            c.next().map(|x| x.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    };
    match section {
        Section::NeedsYou | Section::Later => {
            let need = need.unwrap_or(Need::Question);
            right = need.words().to_string();
            let live = section == Section::NeedsYou;
            right_tone = if live { Tone::Warn } else { Tone::Dim };
            second.push(piece(named(squad_words(t)), if live { Tone::Warn } else { Tone::Dim }));
        }
        Section::Working => {
            if let Some(busy) = &t.busy {
                right = "starting".into();
                right_tone = Tone::Busy;
                second.push(piece(busy.clone(), Tone::Subtext));
            } else if t.queued.is_some() {
                right = "in line".into();
                right_tone = Tone::Busy;
                second.push(piece(tickets::QUEUED_WORDS, Tone::Subtext));
                let queued = t.queued.as_ref().map(|q| format!("{} squad", f.squad_name(&q.squad)));
                if let Some(s) = queued {
                    second.push(piece(format!(" · {s}"), Tone::Subtext));
                }
            } else if let State::Other(word) = &t.state {
                right = word.clone();
                right_tone = Tone::Subtext;
                second.extend(squad.clone().map(|s| piece(s, Tone::Subtext)));
            } else {
                right = since(now, last_wrote(t, f));
                right_tone = Tone::Busy;
                if let Some(s) = &squad {
                    second.push(piece(s.clone(), Tone::Subtext));
                }
                if let Some((l, log)) = leader.and_then(|l| f.log(&l.session).map(|log| (l, log))) {
                    let marks = step_marks(log, &l.squad, l.names, &f.steps(&l.squad), f.running(&l.session));
                    for (k, (label, tone)) in marks.into_iter().enumerate() {
                        second.push(piece(if k == 0 { " · " } else { "  " }, Tone::Dim));
                        let (name, mark) = label.rsplit_once(' ').unwrap_or((label.as_str(), ""));
                        second.push(piece(format!("{name} "), Tone::Subtext));
                        second.push(piece(mark, tone));
                    }
                }
                if let Some(n) = tokens_of(t, f) {
                    second.push(piece(format!(" · {n}"), Tone::Dim));
                }
            }
        }
        Section::CanStart => {
            if t.folders.is_empty() {
                right = tickets::NEEDS_FOLDER.into();
                right_tone = Tone::Subtext;
            }
            match t.handover.as_ref() {
                Some(h) => {
                    let who = if h.kind == tickets::HandoverKind::NotStarted { "Not started" } else { "Commander" };
                    second.push(piece(format!("{who}: {}", h.says), Tone::Warn));
                }
                None => {
                    let new = if t.folders.is_empty() { "New, with no folder of its own · " } else { "New · " };
                    second.push(piece(new, Tone::Dim));
                    second.push(piece("S", Tone::Accent));
                    second.push(piece(" hands it to a squad", Tone::Dim));
                }
            }
        }
        Section::WaitsFor => {
            let waits: Vec<String> = tickets::waiting_for(t, i.siblings)
                .iter()
                .map(|n| match i.siblings.iter().find(|o| o.number == *n) {
                    Some(o) => format!("#{n} {}", o.title),
                    None => format!("#{n}"),
                })
                .collect();
            second.push(piece(format!("Waits for {}", waits.join(", ")), Tone::Dim));
        }
        Section::Finished | Section::Closed => {
            if section == Section::Finished {
                right = ago(now, t.updated);
            } else {
                right = t.state.label().to_string();
                right_tone = if t.state == State::Done { Tone::Ok } else { Tone::Dim };
            }
            if t.state == State::Cancelled {
                second.push(piece("Cancelled", Tone::Dim));
                if let Some(s) = &squad {
                    second.push(piece(format!(" · {s}"), Tone::Dim));
                }
            } else {
                second.push(piece(squad.clone().unwrap_or_else(|| "Done".into()), Tone::Subtext));
                if f.has_result(i.project, t.number) {
                    second.push(piece(" · result on the task", Tone::Subtext));
                }
                if !t.worktrees.is_empty() || t.folders.iter().any(|x| t.branch_in_checkout(x)) {
                    second.push(piece(format!(" · on {}", t.branch()), Tone::Dim));
                }
                if let Some(n) = tokens_of(t, f) {
                    second.push(piece(format!(" · {n}"), Tone::Dim));
                }
            }
        }
    }
    Row {
        project: i.project.to_string(),
        number: t.number,
        title: t.title.clone(),
        section,
        need,
        right,
        right_tone,
        second,
        updated: t.updated,
        squad: leader.map(|l| f.squad_name(&l.squad)),
    }
}

/// Does the row match what the filter says: its title, its project's name or its squad's name holds the words
/// (ignoring case)?
pub fn matches(r: &Row, q: &str, f: &dyn Facts) -> bool {
    let q = q.trim().to_lowercase();
    q.is_empty()
        || r.title.to_lowercase().contains(&q)
        || f.project_name(&r.project).to_lowercase().contains(&q)
        || r.squad.as_deref().is_some_and(|s| s.to_lowercase().contains(&q))
}

/// The board: the tasks in their sections (the board's, or with `pane` a project's pane's), in order, each sorted by
/// its latest change first, except Needs you, where the one waiting longest comes first. Only the rows that match
/// `filter`; empty sections are left out.
pub fn board(tasks: &[In], f: &dyn Facts, pane: bool, filter: &str) -> Vec<Group> {
    let order: &[Section] = if pane { &PANE } else { &BOARD };
    let now = f.now();
    let mut groups: Vec<Group> = order.iter().map(|s| Group { section: *s, rows: Vec::new() }).collect();
    for i in tasks {
        let Some(section) = section_of(i.task, i.siblings, i.archived, now, pane) else { continue };
        let r = row(i, section, f);
        if !matches(&r, filter, f) {
            continue;
        }
        if let Some(g) = groups.iter_mut().find(|g| g.section == section) {
            g.rows.push(r);
        }
    }
    for g in &mut groups {
        // A project's Done and cancelled stay in the order of their numbers, as that pane always listed them.
        if g.section == Section::Closed {
            g.rows.sort_by_key(|r| r.number);
            continue;
        }
        let oldest_first = g.section == Section::NeedsYou;
        g.rows.sort_by(|a, b| {
            let by_time = if oldest_first { a.updated.cmp(&b.updated) } else { b.updated.cmp(&a.updated) };
            by_time.then(a.project.cmp(&b.project)).then(a.number.cmp(&b.number))
        });
    }
    groups.retain(|g| !g.rows.is_empty());
    groups
}

/// How many tasks need the user, across everything: Needs you and not put aside.
pub fn need_count<'a>(tasks: impl IntoIterator<Item = &'a Ticket>) -> usize {
    tasks.into_iter().filter(|t| t.state == State::Waiting && !t.is_aside()).count()
}

/// What the header says about them: "● 2 need you" ("1 needs you"), or nothing.
pub fn need_words(n: usize) -> Option<String> {
    match n {
        0 => None,
        1 => Some("● 1 needs you".into()),
        n => Some(format!("● {n} need you")),
    }
}

/// What the board says when nothing is in play.
pub const EMPTY: &str = "Nothing in play. Press n to write a task, or S on a task that can start.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::MemberRun;
    use crate::tickets::{AgentNames, Handover, HandoverKind, Leader, Queued};
    use std::collections::HashMap;

    const NOW: u64 = 1_800_000_000;

    #[derive(Default)]
    struct Known {
        logs: HashMap<String, LeaderLog>,
        running: Vec<String>,
        results: Vec<(String, u32)>,
    }

    impl Facts for Known {
        fn now(&self) -> u64 {
            NOW
        }
        fn squad_name(&self, id: &str) -> String {
            match id {
                "coding" => "Coding".into(),
                "docs" => "Docs".into(),
                other => other.into(),
            }
        }
        fn steps(&self, squad: &str) -> Vec<String> {
            match squad {
                "coding" => ["planner", "builder", "reviewer", "verifier"].iter().map(|s| s.to_string()).collect(),
                _ => Vec::new(),
            }
        }
        fn log(&self, session: &str) -> Option<&LeaderLog> {
            self.logs.get(session)
        }
        fn running(&self, session: &str) -> bool {
            self.running.iter().any(|s| s == session)
        }
        fn has_result(&self, project: &str, number: u32) -> bool {
            self.results.iter().any(|(p, n)| p == project && *n == number)
        }
        fn project_name(&self, id: &str) -> String {
            match id {
                "pweb" => "Web shop".into(),
                "pdocs" => "Docs site".into(),
                "" => "No project".into(),
                other => other.into(),
            }
        }
    }

    fn task(number: u32, title: &str, state: State, updated: u64) -> Ticket {
        Ticket { number, title: title.into(), state, updated, folders: vec!["/home/u/shop".into()], ..Default::default() }
    }

    fn led(mut t: Ticket, squad: &str, session: &str) -> Ticket {
        t.leaders.push(Leader {
            squad: squad.into(),
            session: session.into(),
            started: NOW - 3600,
            names: AgentNames::SquadRole,
            snapshot: None,
        });
        t
    }

    fn waiting(number: u32, title: &str, updated: u64, reason: &str) -> Ticket {
        let mut t = led(task(number, title, State::Waiting, updated), "docs", &format!("L{number}"));
        t.reason = reason.into();
        t
    }

    /// A row as (project, number, right word, line two).
    type Shape = (String, u32, String, String);

    /// The sections with their rows.
    fn shape(groups: &[Group]) -> Vec<(&'static str, Vec<Shape>)> {
        groups
            .iter()
            .map(|g| {
                (g.section.heading(), g.rows.iter().map(|r| (r.project.clone(), r.number, r.right.clone(), r.second_text())).collect())
            })
            .collect()
    }

    #[test]
    fn every_task_in_play_goes_in_its_section_in_order_with_its_words_and_the_sort() {
        let web = vec![
            waiting(6, "Explain the discount rules", NOW - 600, "Asks: Which page should the rules live on?"),
            {
                let mut t = waiting(7, "Rate limit the login form", NOW - 1200, "Wants to publish: push task/7 and open a merge request");
                t.publish_wanted = true;
                t.leaders[0].squad = "coding".into();
                t
            },
            led(task(2, "Discount codes at checkout", State::Working, NOW - 100), "coding", "L2"),
            task(5, "Order emails in plain text", State::New, NOW - 50),
            {
                let mut t = task(8, "Basket page", State::New, NOW - 40);
                t.blocked_by = vec![2, 5];
                t
            },
            led(task(3, "Compare config formats", State::Done, NOW - 7200), "coding", "L3"),
            // Finished before today: not on the board.
            led(task(1, "Old work", State::Done, NOW - 3 * 86_400), "coding", "L1"),
        ];
        let docs = vec![{
            let mut t = task(5, "Release notes", State::New, NOW - 30);
            t.folders.clear();
            t
        }];
        let loose = vec![{
            let mut t = waiting(1, "Look into the linter", NOW - 3000, "Stuck: the tests need a database");
            t.leaders[0].squad = "coding".into();
            t
        }];
        let mut known = Known::default();
        let ms = |s: u64| s * 1000;
        let run = |agent: &str, start: u64, ended: Option<&str>| MemberRun {
            call: format!("c-{agent}"),
            agent: format!("coding-{agent}"),
            started_at: Some(ms(start)),
            ended: ended.map(String::from),
            ..Default::default()
        };
        known.logs.insert(
            "L2".into(),
            LeaderLog {
                runs: vec![
                    run("planner", NOW - 3000, Some("completed")),
                    run("builder", NOW - 2000, Some("completed")),
                    run("reviewer", NOW - 900, None),
                ],
                leader: crate::claude::Spent { input: 150_000, output: 37_000, cache_read: 9_000_000, subagents: false },
                last_at: Some(ms(NOW - 14 * 60)),
                ..Default::default()
            },
        );
        known.running.push("L2".into());
        known.results.push(("pweb".into(), 3));
        let mut tasks: Vec<In> = Vec::new();
        tasks.extend(web.iter().map(|t| In { project: "pweb", archived: false, task: t, siblings: &web }));
        tasks.extend(docs.iter().map(|t| In { project: "pdocs", archived: false, task: t, siblings: &docs }));
        tasks.extend(loose.iter().map(|t| In { project: "", archived: false, task: t, siblings: &loose }));
        let groups = board(&tasks, &known, false, "");
        let s = |a: &str| a.to_string();
        assert_eq!(
            shape(&groups),
            [
                (
                    "Needs you",
                    vec![
                        // The one waiting longest first.
                        (s(""), 1, s("stuck"), s("Coding squad is stuck: the tests need a database")),
                        (s("pweb"), 7, s("a go-ahead"), s("Coding squad wants to publish: push task/7 and open a merge request")),
                        (s("pweb"), 6, s("a question"), s("Docs squad asks: Which page should the rules live on?")),
                    ]
                ),
                (
                    "Working",
                    vec![(s("pweb"), 2, s("14 min"), s("Coding squad · planner ✓  builder ✓  reviewer ●  verifier · · 187K tokens"))]
                ),
                (
                    "Can start",
                    vec![
                        // The latest change first; the same number in two projects is two rows.
                        (s("pdocs"), 5, s("Needs a folder"), s("New, with no folder of its own · S hands it to a squad")),
                        (s("pweb"), 5, s(""), s("New · S hands it to a squad")),
                    ]
                ),
                ("Waits for", vec![(s("pweb"), 8, s(""), s("Waits for #2 Discount codes at checkout, #5 Order emails in plain text"))]),
                ("Finished today", vec![(s("pweb"), 3, s("2 h ago"), s("Coding squad · result on the task"))]),
            ]
        );
        // The kinds of need, and each row's colors.
        let needs: Vec<Option<Need>> = groups[0].rows.iter().map(|r| r.need).collect();
        assert_eq!(needs, [Some(Need::Stuck), Some(Need::GoAhead), Some(Need::Question)]);
        assert!(groups[0].rows.iter().all(|r| r.right_tone == Tone::Warn && r.second.iter().all(|p| p.tone == Tone::Warn)));
        let marks: Vec<(&str, Tone)> =
            groups[1].rows[0].second.iter().map(|p| (p.text.as_str(), p.tone)).filter(|(t, _)| t.chars().count() == 1).collect();
        assert_eq!(marks, [("✓", Tone::Ok), ("✓", Tone::Ok), ("●", Tone::Busy), ("·", Tone::Dim)]);
        assert_eq!(groups[1].rows[0].right_tone, Tone::Busy);
    }

    #[test]
    fn a_question_a_go_ahead_being_stuck_and_a_command_waiting_each_read_as_what_they_are() {
        let mut t = waiting(1, "x", NOW, "Asks: Which colour?");
        assert_eq!(
            (need_of(&t), squad_words(&t), squad_said(&t)),
            (Some(Need::Question), "asks: Which colour?".into(), "Which colour?".into())
        );
        t.reason = "The squad has a question (see its session).".into();
        assert_eq!(squad_words(&t), "has a question (see its session)");
        t.reason = crate::squads::NO_LINE.into();
        assert_eq!((need_of(&t), squad_words(&t)), (Some(Need::Stuck), "stopped without saying why".into()));
        t.reason = "The squad is stuck (see its session).".into();
        assert_eq!((need_of(&t), squad_words(&t)), (Some(Need::Stuck), "is stuck (see its session)".into()));
        t.reason = "Wants to publish something (see its session).".into();
        t.publish_wanted = true;
        assert_eq!((need_of(&t), squad_words(&t)), (Some(Need::GoAhead), "wants to publish something (see its session)".into()));
        t.publish_wanted = false;
        t.permission = true;
        t.reason = tickets::PERMISSION_REASON.into();
        assert_eq!((need_of(&t), squad_words(&t)), (Some(Need::Permission), "waits for your OK on a command".into()));
        assert!(squad_said(&t).contains("Open the session"));
        t.permission = false;
        t.reason = "Something a newer platoon wrote".into();
        assert_eq!((need_of(&t), squad_words(&t)), (Some(Need::Question), "says: Something a newer platoon wrote".into()));
        let words: Vec<&str> = [Need::Question, Need::GoAhead, Need::Stuck, Need::Permission].iter().map(|n| n.words()).collect();
        assert_eq!(words, ["a question", "a go-ahead", "stuck", "your OK on a command"]);
        assert_eq!(need_of(&task(2, "y", State::Working, NOW)), None);
    }

    #[test]
    fn a_task_put_aside_waits_under_later_until_the_squad_says_something_new() {
        let mut t = waiting(4, "Aside", NOW - 100, "Asks: Now or later?");
        t.aside = Some(tickets::Aside { updated: t.updated, reason: t.reason.clone() });
        let all = vec![t.clone()];
        let known = Known::default();
        let groups = board(&[In { project: "pweb", archived: false, task: &all[0], siblings: &all }], &known, false, "");
        assert_eq!(
            shape(&groups),
            [("Later", vec![("pweb".to_string(), 4, "a question".to_string(), "Docs squad asks: Now or later?".to_string())])]
        );
        assert!(groups[0].rows[0].second.iter().all(|p| p.tone == Tone::Dim), "it steps back");
        assert_eq!(need_count(&all), 0, "nothing needs the user now");
        // The squad says something new: back in Needs you, and counted.
        t.reason = "Asks: Which one?".into();
        t.updated += 5;
        let all = vec![t];
        let groups = board(&[In { project: "pweb", archived: false, task: &all[0], siblings: &all }], &known, false, "");
        assert_eq!(groups[0].section, Section::NeedsYou);
        assert_eq!(need_count(&all), 1);
        assert_eq!((need_words(0), need_words(1), need_words(2)), (None, Some("● 1 needs you".into()), Some("● 2 need you".into())));
    }

    #[test]
    fn nothing_in_play_is_an_empty_board_and_finished_today_has_its_own_word() {
        let known = Known::default();
        assert!(board(&[], &known, false, "").is_empty());
        let old = vec![led(task(1, "Long done", State::Done, NOW - 2 * 86_400), "coding", "L")];
        assert!(board(&[In { project: "p", archived: false, task: &old[0], siblings: &old }], &known, false, "").is_empty());
        assert!(EMPTY.contains("Press n") && !EMPTY.contains(" t "));
        // A cancelled one says so first; on a project's pane both are under Done and cancelled with their state word.
        let mut c = led(task(2, "Called off", State::Cancelled, NOW - 60), "coding", "L");
        c.title = "Called off".into();
        let all = vec![c];
        let groups = board(&[In { project: "p", archived: false, task: &all[0], siblings: &all }], &known, false, "");
        assert_eq!(groups[0].rows[0].second_text(), "Cancelled · Coding squad");
        assert_eq!(groups[0].rows[0].right, "1 min ago");
        let pane = board(&[In { project: "p", archived: false, task: &all[0], siblings: &all }], &known, true, "");
        assert_eq!((pane[0].section, pane[0].rows[0].right.as_str()), (Section::Closed, "Cancelled"));
        // A New task of an archived project isn't in play (its pane still lists it).
        let new = vec![task(3, "Parked", State::New, NOW)];
        assert!(board(&[In { project: "p", archived: true, task: &new[0], siblings: &new }], &known, false, "").is_empty());
        assert_eq!(board(&[In { project: "p", archived: true, task: &new[0], siblings: &new }], &known, true, "").len(), 1);
        assert_eq!(
            (since(NOW, NOW - 30), since(NOW, NOW - 14 * 60), since(NOW, NOW - 3 * 3600), since(NOW, NOW - 90_000), since(NOW, 0)),
            ("just now".to_string(), "14 min".to_string(), "3 h".to_string(), "1 day".to_string(), String::new())
        );
    }

    #[test]
    fn the_filter_finds_a_task_by_its_title_its_project_or_its_squad() {
        let web = vec![led(task(1, "Basket page", State::Working, NOW), "coding", "L1"), task(2, "Login", State::New, NOW)];
        let docs = vec![task(1, "Guide", State::New, NOW)];
        let known = Known::default();
        let mut tasks: Vec<In> = web.iter().map(|t| In { project: "pweb", archived: false, task: t, siblings: &web }).collect();
        tasks.extend(docs.iter().map(|t| In { project: "pdocs", archived: false, task: t, siblings: &docs }));
        let found = |q: &str| -> Vec<(String, u32)> {
            board(&tasks, &known, false, q).iter().flat_map(|g| g.rows.iter().map(|r| (r.project.clone(), r.number))).collect()
        };
        assert_eq!(found("basket"), [("pweb".to_string(), 1)]);
        assert_eq!(found("DOCS SITE"), [("pdocs".to_string(), 1)]);
        assert_eq!(found("coding"), [("pweb".to_string(), 1)]);
        assert_eq!(found("").len(), 3);
        assert!(found("nothing like it").is_empty());
    }

    #[test]
    fn a_task_being_handed_over_in_line_or_refused_by_the_commander_says_so() {
        let mut busy = task(1, "Busy", State::Working, NOW);
        busy.busy = Some("Commander is checking…".into());
        let mut queued = task(2, "Queued", State::Working, NOW);
        queued.queued = Some(Queued { squad: "coding".into(), since: NOW, anyway: false });
        let mut refused = task(3, "Refused", State::New, NOW);
        refused.handover = Some(Handover { squad: "coding".into(), kind: HandoverKind::Refused, says: "This is docs work.".into() });
        let all = vec![busy, queued, refused];
        let known = Known::default();
        let tasks: Vec<In> = all.iter().map(|t| In { project: "p", archived: false, task: t, siblings: &all }).collect();
        let groups = board(&tasks, &known, false, "");
        let rows: Vec<(u32, String, String)> =
            groups.iter().flat_map(|g| g.rows.iter().map(|r| (r.number, r.right.clone(), r.second_text()))).collect();
        assert!(rows.contains(&(1, "starting".into(), "Commander is checking…".into())), "{rows:?}");
        assert!(rows.contains(&(2, "in line".into(), "Waiting for a free squad · Coding squad".into())), "{rows:?}");
        assert!(rows.contains(&(3, String::new(), "Commander: This is docs work.".into())), "{rows:?}");
    }
}
