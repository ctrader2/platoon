//! The Tasks tab: every task across all projects and the ones in no project, on two lists switched like the Sessions
//! and Projects lists' Active and Archived. **Active** is grouped by what each task needs from the user: **Needs you**,
//! **Working**, **Not started** and **Done** (finished in the last day, a Done task whose work isn't merged, and one the
//! user brought back with `a`). **Archived** is every other finished task, the latest first (`place`). Every row answers the same questions in
//! the same places: line one says what it is (the project's badge, `#N`, the title, drawn by the UI) and at the right
//! the section's word for it (what it needs, how long it has worked, when it finished); line two says which squad has
//! it and what it needs from the user, in the session's own words where it has some. Pure: the app hands it the tasks
//! and what it knows about their sessions.

use crate::tickets::{Archiving, Need, State, Ticket};

/// The Tasks tab's two lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Shelf {
    #[default]
    Active,
    Archived,
}

/// What Active says when it has nothing on it but Archived has.
pub const ALL_ARCHIVED: &str = "Every task is finished and archived (→ shows them). Press n to write one.";

impl Shelf {
    /// Both, in the order they are shown (the switch's chips, ←→).
    pub const ALL: [Shelf; 2] = [Shelf::Active, Shelf::Archived];

    pub fn label(self) -> &'static str {
        match self {
            Shelf::Active => "Active",
            Shelf::Archived => "Archived",
        }
    }

    /// What the list says when nothing is on it.
    pub fn empty(self) -> &'static str {
        match self {
            Shelf::Active => "No tasks yet. Press n to write one.",
            Shelf::Archived => "Nothing archived. A finished task moves here a day after it finishes; a on it moves it sooner.",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
}

/// A group of the board, in the order they are shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    NeedsYou,
    Working,
    NotStarted,
    /// Done or Cancelled, and still on Active (on Archived every task is in this one section, shown without its
    /// heading).
    Done,
}

impl Section {
    pub fn heading(self) -> &'static str {
        match self {
            Section::NeedsYou => "Needs you",
            Section::Working => "Working",
            Section::NotStarted => "Not started",
            Section::Done => "Done",
        }
    }
}

/// The board's sections, in order.
pub const SECTIONS: [Section; 4] = [Section::NeedsYou, Section::Working, Section::NotStarted, Section::Done];

/// How long Done and Cancelled tasks stay on Active before they move to Archived by themselves, in seconds.
pub const FINISHED_FOR: u64 = 24 * 3600;

/// What git last said about a Done task's branch (`gitfacts::not_merged`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Merge {
    /// Nothing on it to merge: its commits are in the default branch, or it has no branch.
    Merged,
    /// Its branch has commits the default branch hasn't: landing it is still the user's.
    NotMerged,
    /// Not read yet (platoon has just started, or the task has just finished).
    Unknown,
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
    /// Where the line hasn't room for all of it, it may be cut after a word (a step's title); else it goes whole or
    /// not at all (a count, a mark). The line's first piece is always cut that way.
    pub cut: bool,
}

pub fn piece(text: impl Into<String>, tone: Tone) -> Piece {
    Piece { text: text.into(), tone, cut: false }
}

/// A piece that may be cut after a word.
pub fn cuttable(text: impl Into<String>, tone: Tone) -> Piece {
    Piece { text: text.into(), tone, cut: true }
}

/// One task on the board.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// Its project's id (`tickets::NO_PROJECT_ID` for none).
    pub project: String,
    pub number: u32,
    pub title: String,
    pub section: Section,
    /// The section's word for it, at the right of line one.
    pub right: String,
    pub right_tone: Tone,
    /// Line two.
    pub second: Vec<Piece>,
    pub updated: u64,
    /// Its squad's name, for the filter.
    pub squad: String,
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
    /// What a session spent, short ("84K tokens"), once its transcript has been read.
    fn tokens(&self, session: &str) -> Option<String>;
    /// When a session last wrote (seconds since the epoch), once its transcript has been read.
    fn last_wrote(&self, session: &str) -> Option<u64>;
    /// A project's name by its id, for the filter ("No project" for none).
    fn project_name(&self, id: &str) -> String;
    /// What git last said about a Done task's branch.
    fn merge(&self, _project: &str, _t: &Ticket) -> Merge {
        Merge::Merged
    }
    /// The program a started task's session runs in, when that program is switched off in platoon's settings.
    fn switched_off(&self, _project: &str, _t: &Ticket) -> Option<crate::agents::Agent> {
        None
    }
}

/// What a task whose program is switched off says on its row: "Codex is switched off".
pub fn switched_off_words(agent: crate::agents::Agent) -> String {
    format!("{} is switched off", agent.name())
}

/// What a Done task whose work is not merged says on its row.
pub const NOT_MERGED: &str = "Done · not merged";

/// A task as the board is given it: its project's id, whether that project is archived, and what the app is doing
/// for it just now (starting it: making its worktree).
pub struct In<'a> {
    pub project: &'a str,
    pub archived: bool,
    pub task: &'a Ticket,
    pub busy: Option<&'a str>,
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

/// Where a task is on the Tasks tab: its list and its section there (on Archived, `Section::Done`), or None when it is
/// on neither (a task of an archived project that never started). `archived`: its project is; `busy`: the app is
/// starting it; `merge`: what git said about a Done task's branch.
pub fn place(t: &Ticket, archived: bool, busy: bool, merge: Merge, now: u64) -> Option<(Shelf, Section)> {
    let section = match &t.state {
        _ if busy => Section::Working,
        State::NeedsYou(_) => Section::NeedsYou,
        State::Working => Section::Working,
        State::NotStarted if archived => return None,
        State::NotStarted => Section::NotStarted,
        State::Done | State::Cancelled => return Some((finished_shelf(t, merge, now), Section::Done)),
    };
    Some((Shelf::Active, section))
}

/// The list a finished task is on: as the user said with `a`, else Active for `FINISHED_FOR` after it finished and while
/// its work isn't merged, then Archived. A branch not read yet counts as merged, so a task that went long ago never shows
/// on Active for a moment while platoon starts; one that turns out not merged comes back to Active.
fn finished_shelf(t: &Ticket, merge: Merge, now: u64) -> Shelf {
    match t.archiving {
        Archiving::Archived => Shelf::Archived,
        Archiving::KeptActive => Shelf::Active,
        Archiving::Auto if now.saturating_sub(t.updated) < FINISHED_FOR => Shelf::Active,
        Archiving::Auto if t.state == State::Done && merge == Merge::NotMerged => Shelf::Active,
        Archiving::Auto => Shelf::Archived,
    }
}

/// What a task that needs the user says on its row, after its squad's name.
pub fn need_words(n: &Need) -> String {
    match n {
        Need::Plan => "its plan is ready: read it, then answer in its session".into(),
        Need::Question(q) => format!("asks: {q}"),
        Need::Permission(m) => format!("waits at a permission prompt: {m}"),
        Need::Check(s) => format!("the check found {s}"),
        Need::CheckFailed(s) => format!("the check could not run: {s}"),
        Need::CommitFailed(s) => format!("platoon couldn't commit its work (it is still in the worktree): {s}"),
    }
}

/// Is the check of a task's work running now? Its mark, unless it is older than the check can take (a hook that was
/// ended before it could clear it).
pub fn checking(t: &Ticket, now: u64) -> bool {
    t.state == State::Working && t.checking.as_ref().is_some_and(|c| now.saturating_sub(c.since) <= crate::verdict::TIMEOUT.as_secs() + 120)
}

/// "Fable is checking the work… 0:23", while the check runs.
pub fn checking_words(t: &Ticket, now: u64) -> Option<String> {
    let c = t.checking.as_ref().filter(|_| checking(t, now))?;
    Some(crate::steps::checking_words(&c.model, now.saturating_sub(c.since)))
}

/// What has just sent a task's work back to its session, while the session works on it (until the next check, or the
/// user types): the stop sent it back to mark its steps, or the check found problems.
pub fn sent_back(t: &Ticket, now: u64) -> Option<String> {
    if t.state != State::Working || checking(t, now) {
        return None;
    }
    let unmarked = t.steps.unmarked();
    if t.steps.sent_back && !unmarked.is_empty() && t.checks_refused == 0 {
        return Some(format!("Sent back to mark {} done or skipped.", steps_not_marked(&unmarked)));
    }
    let v = t.verdict.as_ref().filter(|v| t.checks_refused > 0 && v.failed.is_none() && !v.done)?;
    let by = if v.model.is_empty() { "The check".to_string() } else { format!("{}'s check", crate::agents::model_display_name(&v.model)) };
    Some(format!("{by} sent it back: {}", crate::verdict::short(v)))
}

/// "step 4", "steps 4 and 8", "steps 2, 4 and 8".
fn steps_not_marked(n: &[usize]) -> String {
    let each: Vec<String> = n.iter().map(usize::to_string).collect();
    match each.as_slice() {
        [one] => format!("step {one}"),
        [rest @ .., last] => format!("steps {} and {last}", rest.join(", ")),
        [] => String::new(),
    }
}

/// What the Steps checklist of a task's details is headed with, after "Steps": how far it is ("4 of 8 marked"), or,
/// once the end of the work has sent it back to mark them, how many are still not marked ("2 not marked", a warning).
pub fn steps_heading(t: &Ticket) -> Option<Piece> {
    if t.steps.is_empty() {
        return None;
    }
    let unmarked = t.steps.unmarked().len();
    let total = t.steps.list.len();
    Some(if t.steps.sent_back && unmarked > 0 {
        piece(if unmarked == 1 { "1 step not marked".to_string() } else { format!("{unmarked} steps not marked") }, Tone::Warn)
    } else {
        piece(format!("{} of {total} marked", total - unmarked), Tone::Dim)
    })
}

/// A task's Verdict line in its details (after the label "Verdict"): checking now, or what the last check said, who
/// checked and when. Nothing before its first check.
pub fn verdict_line(t: &Ticket, now: u64, clock: &dyn Fn(u64) -> String) -> Vec<Piece> {
    if let Some(words) = checking_words(t, now) {
        return vec![piece(words, Tone::Busy)];
    }
    let Some(v) = &t.verdict else { return Vec::new() };
    let at = clock(v.at);
    let who = format!(" · checked by {}{}", v.model, if at.is_empty() { String::new() } else { format!(" at {at}") });
    let tokens = if v.tokens > 0 { format!(" · {} tokens", crate::spent::short(v.tokens)) } else { String::new() };
    let mut out = match (&v.failed, v.done) {
        (Some(why), _) => vec![piece("could not run", Tone::Warn), piece(format!(" · {why}"), Tone::Dim)],
        (None, true) => vec![piece("done", Tone::Ok)],
        (None, false) => {
            let n = v.problems();
            let mut p = vec![piece(if n == 1 { "1 problem".to_string() } else { format!("{n} problems") }, Tone::Warn)];
            if t.state == State::Working {
                p.push(piece(" · sent back to the session", Tone::Dim));
            }
            p
        }
    };
    out.push(piece(who, Tone::Dim));
    if !v.note.is_empty() && v.failed.is_none() {
        out.push(piece(format!(" · {}", v.note), Tone::Dim));
    }
    out.push(piece(tokens, Tone::Dim));
    out.retain(|p| !p.text.is_empty());
    out
}

/// One task's row in `section`.
pub fn row(i: &In, section: Section, f: &dyn Facts) -> Row {
    let t = i.task;
    let now = f.now();
    let squad = if t.squad.is_empty() { String::new() } else { f.squad_name(&t.squad) };
    let session = t.session.as_deref().unwrap_or_default();
    let (mut right, mut right_tone) = (String::new(), Tone::Dim);
    let mut second: Vec<Piece> = Vec::new();
    let named = |words: String| if squad.is_empty() { words } else { format!("{squad} · {words}") };
    match (&t.state, i.busy) {
        (_, Some(busy)) => {
            right = "starting".into();
            right_tone = Tone::Busy;
            second.push(piece(busy, Tone::Subtext));
        }
        (State::NeedsYou(n), _) => {
            right = n.words().to_string();
            right_tone = Tone::Warn;
            second.push(piece(named(need_words(n)), Tone::Warn));
        }
        (State::Working, _) => {
            right = since(now, f.last_wrote(session).unwrap_or(t.updated));
            right_tone = Tone::Busy;
            second.push(piece(if squad.is_empty() { "Working".to_string() } else { squad.clone() }, Tone::Subtext));
            match t.steps.current() {
                Some(i) => {
                    second.push(piece(format!(" · {}", t.steps.position(i)), Tone::Subtext));
                    second.push(cuttable(format!(" · {}", t.steps.list[i].title), Tone::Subtext));
                }
                None => second.extend(t.steps.words().map(|w| piece(format!(" · {w}"), Tone::Subtext))),
            }
            if checking(t, now) {
                second.push(piece(" · checking the work", Tone::Busy));
            }
            if let Some(n) = f.tokens(session) {
                second.push(piece(format!(" · {n}"), Tone::Dim));
            }
        }
        (State::NotStarted, _) => {
            if t.folder.is_none() {
                second.push(piece("No folder yet · ", Tone::Dim));
            }
            if !squad.is_empty() {
                second.push(piece(format!("Suggested: {squad} · "), Tone::Dim));
            }
            second.push(piece("s", Tone::Accent));
            second.push(piece(" starts it", Tone::Dim));
        }
        (State::Cancelled, _) => {
            right = ago(now, t.updated);
            second.push(piece(if squad.is_empty() { "Cancelled".to_string() } else { format!("Cancelled · {squad}") }, Tone::Dim));
        }
        (State::Done, _) => {
            right = ago(now, t.updated);
            right_tone = Tone::Ok;
            if f.merge(i.project, t) == Merge::NotMerged {
                // Its work is on its branch, and the default branch doesn't have it yet: the user merges it.
                if !squad.is_empty() {
                    second.push(piece(format!("{squad} · "), Tone::Subtext));
                }
                second.push(piece(NOT_MERGED, Tone::Warn));
            } else {
                second.push(piece(if squad.is_empty() { "Done".to_string() } else { squad.clone() }, Tone::Subtext));
            }
            if t.worktree.is_some() {
                second.push(piece(format!(" · on {}", t.branch()), Tone::Dim));
            }
            if let Some(n) = f.tokens(session) {
                second.push(piece(format!(" · {n}"), Tone::Dim));
            }
        }
    }
    // Its program is switched off: it stays on the board (it may still be running), and says so first.
    if i.busy.is_none()
        && t.state != State::Cancelled
        && let Some(agent) = f.switched_off(i.project, t)
    {
        second.insert(0, piece(format!("{} · ", switched_off_words(agent)), Tone::Warn));
    }
    Row {
        project: i.project.to_string(),
        number: t.number,
        title: t.title.clone(),
        section,
        right,
        right_tone,
        second,
        updated: t.updated,
        squad,
    }
}

/// Does the row match what the filter says: its title, its project's name or its squad's name holds the words
/// (ignoring case)?
pub fn matches(r: &Row, q: &str, f: &dyn Facts) -> bool {
    let q = q.trim().to_lowercase();
    q.is_empty()
        || r.title.to_lowercase().contains(&q)
        || f.project_name(&r.project).to_lowercase().contains(&q)
        || r.squad.to_lowercase().contains(&q)
}

/// Where task `i` is, as `place` says with what the board knows.
fn place_of(i: &In, f: &dyn Facts, now: u64) -> Option<(Shelf, Section)> {
    place(i.task, i.archived, i.busy.is_some(), f.merge(i.project, i.task), now)
}

/// One of the board's lists: the tasks on `shelf` in their sections, in order, each sorted by its latest change first,
/// except Needs you, where the one waiting longest comes first (Archived has the one section). Only the rows that match
/// `filter`; empty sections are left out.
pub fn board(tasks: &[In], f: &dyn Facts, shelf: Shelf, filter: &str) -> Vec<Group> {
    let now = f.now();
    let mut groups: Vec<Group> = SECTIONS.iter().map(|s| Group { section: *s, rows: Vec::new() }).collect();
    for i in tasks {
        let Some((on, section)) = place_of(i, f, now) else { continue };
        if on != shelf {
            continue;
        }
        let r = row(i, section, f);
        if !matches(&r, filter, f) {
            continue;
        }
        if let Some(g) = groups.iter_mut().find(|g| g.section == section) {
            g.rows.push(r);
        }
    }
    for g in &mut groups {
        let oldest_first = g.section == Section::NeedsYou;
        g.rows.sort_by(|a, b| {
            let by_time = if oldest_first { a.updated.cmp(&b.updated) } else { b.updated.cmp(&a.updated) };
            by_time.then(a.project.cmp(&b.project)).then(a.number.cmp(&b.number))
        });
    }
    groups.retain(|g| !g.rows.is_empty());
    groups
}

/// How many tasks each list has (not narrowed by the filter), in the order of `Shelf::ALL`.
pub fn counts(tasks: &[In], f: &dyn Facts) -> [usize; 2] {
    let now = f.now();
    let mut out = [0; 2];
    for (shelf, _) in tasks.iter().filter_map(|i| place_of(i, f, now)) {
        out[shelf.index()] += 1;
    }
    out
}

/// How many tasks need the user, across everything.
pub fn need_count<'a>(tasks: impl IntoIterator<Item = &'a Ticket>) -> usize {
    tasks.into_iter().filter(|t| t.state.needs_you()).count()
}

/// What the header says about them: "● 2 need you" ("1 needs you"), or nothing.
pub fn header_words(n: usize) -> Option<String> {
    match n {
        0 => None,
        1 => Some("● 1 needs you".into()),
        n => Some(format!("● {n} need you")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;

    struct Known;

    impl Facts for Known {
        fn now(&self) -> u64 {
            NOW
        }
        fn squad_name(&self, id: &str) -> String {
            match id {
                "feature" => "Feature".into(),
                other => other.into(),
            }
        }
        fn tokens(&self, session: &str) -> Option<String> {
            (session == "s-read").then(|| "84K tokens".into())
        }
        fn last_wrote(&self, session: &str) -> Option<u64> {
            (session == "s-read").then_some(NOW - 120)
        }
        fn project_name(&self, id: &str) -> String {
            if id.is_empty() { "No project".into() } else { format!("Project {id}") }
        }
    }

    fn task(number: u32, title: &str, state: State, updated: u64) -> Ticket {
        Ticket { number, title: title.into(), state, updated, folder: Some("/w".into()), ..Default::default() }
    }

    fn started(mut t: Ticket, session: &str) -> Ticket {
        t.squad = "feature".into();
        t.session = Some(session.into());
        t
    }

    type Shape = Vec<(&'static str, Vec<(u32, String, String)>)>;

    fn shape(groups: &[Group]) -> Shape {
        groups
            .iter()
            .map(|g| (g.section.heading(), g.rows.iter().map(|r| (r.number, r.right.clone(), r.second_text())).collect()))
            .collect()
    }

    #[test]
    fn every_task_goes_in_its_section_in_order_with_its_words() {
        let tasks = vec![
            started(task(1, "Plan me", State::NeedsYou(Need::Plan), NOW - 50), "s-1"),
            started(task(2, "Ask me", State::NeedsYou(Need::Question("Which page?".into())), NOW - 500), "s-2"),
            started(task(3, "Busy", State::Working, NOW - 10), "s-read"),
            task(4, "Fresh", State::NotStarted, NOW - 5),
            Ticket { squad: "feature".into(), folder: None, ..task(5, "Suggested", State::NotStarted, NOW - 1) },
            started(task(6, "Finished", State::Done, NOW - 7200), "s-read"),
            started(task(7, "Old", State::Done, NOW - 2 * 86_400), "s-7"),
            task(8, "Dropped", State::Cancelled, NOW - 60),
            started(
                task(9, "Permission", State::NeedsYou(Need::Permission("Claude needs your permission to use Bash".into())), NOW - 20),
                "s-9",
            ),
            started(task(10, "Checked", State::NeedsYou(Need::Check("1 problem: a.rs:3: x".into())), NOW - 30), "s-10"),
        ];
        let ins: Vec<In> = tasks.iter().map(|t| In { project: "p", archived: false, task: t, busy: None }).collect();
        let g = board(&ins, &Known, Shelf::Active, "");
        let s = |a: &str| a.to_string();
        assert_eq!(
            shape(&g),
            vec![
                (
                    "Needs you",
                    vec![
                        (2, s("a question"), s("Feature · asks: Which page?")),
                        (1, s("a plan to approve"), s("Feature · its plan is ready: read it, then answer in its session")),
                        (10, s("the check found problems"), s("Feature · the check found 1 problem: a.rs:3: x")),
                        (9, s("asks permission"), s("Feature · waits at a permission prompt: Claude needs your permission to use Bash")),
                    ]
                ),
                ("Working", vec![(3, s("2 min"), s("Feature · 84K tokens"))]),
                ("Not started", vec![(5, s(""), s("No folder yet · Suggested: Feature · s starts it")), (4, s(""), s("s starts it"))]),
                ("Done", vec![(8, s("1 min ago"), s("Cancelled")), (6, s("2 h ago"), s("Feature · 84K tokens"))]),
            ],
            "Needs you: the longest waiting first; elsewhere the latest first; finished over a day ago on Archived"
        );
        assert_eq!(
            shape(&board(&ins, &Known, Shelf::Archived, "")),
            vec![("Done", vec![(7, "2 days ago".to_string(), "Feature".to_string())])]
        );
        assert_eq!(counts(&ins, &Known), [9, 1]);
        assert_eq!(need_count(tasks.iter()), 4);
        assert_eq!(header_words(1).as_deref(), Some("● 1 needs you"));
        assert_eq!(header_words(0), None);
    }

    #[test]
    fn a_starting_task_works_and_an_archived_projects_unstarted_tasks_are_left_out() {
        let t = task(1, "Go", State::NotStarted, NOW);
        let g = board(&[In { project: "p", archived: false, task: &t, busy: Some("Making its worktree…") }], &Known, Shelf::Active, "");
        assert_eq!(shape(&g), vec![("Working", vec![(1, "starting".to_string(), "Making its worktree…".to_string())])]);
        assert!(board(&[In { project: "p", archived: true, task: &t, busy: None }], &Known, Shelf::Active, "").is_empty());
    }

    #[test]
    fn a_finished_task_moves_to_archived_a_day_after_it_finished_unless_its_work_is_not_merged_or_the_user_said() {
        use Merge::*;
        let at = |state: State, ago: u64, archiving: Archiving| Ticket { archiving, ..task(1, "T", state, NOW - ago) };
        let shelf = |t: &Ticket, merge: Merge| place(t, false, false, merge, NOW).map(|(s, _)| s);
        let (active, archived) = (Some(Shelf::Active), Some(Shelf::Archived));
        // A day after it finished, to the second, whether Done or Cancelled.
        for state in [State::Done, State::Cancelled] {
            assert_eq!(shelf(&at(state.clone(), FINISHED_FOR - 1, Archiving::Auto), Merged), active, "{state:?}");
            assert_eq!(shelf(&at(state.clone(), FINISHED_FOR, Archiving::Auto), Merged), archived, "{state:?}");
        }
        // A Done task whose work is not merged stays, however old; not read yet, it is where a merged one would be.
        let old_done = at(State::Done, 30 * 86_400, Archiving::Auto);
        assert_eq!(shelf(&old_done, NotMerged), active);
        assert_eq!(shelf(&old_done, Unknown), archived);
        assert_eq!(shelf(&at(State::Cancelled, 30 * 86_400, Archiving::Auto), NotMerged), archived, "a cancelled one is not landed");
        // By hand, both ways: it sticks, whatever its age and whether its work is merged.
        assert_eq!(shelf(&at(State::Done, 60, Archiving::Archived), NotMerged), archived);
        assert_eq!(shelf(&at(State::Cancelled, 60, Archiving::Archived), Merged), archived);
        assert_eq!(shelf(&at(State::Done, 365 * 86_400, Archiving::KeptActive), Merged), active);
        assert_eq!(shelf(&at(State::Cancelled, 365 * 86_400, Archiving::KeptActive), Merged), active);
        // On Archived every task is in the one section; a task at work is always on Active.
        assert_eq!(place(&old_done, false, false, Merged, NOW), Some((Shelf::Archived, Section::Done)));
        assert_eq!(
            place(&at(State::Working, 365 * 86_400, Archiving::Archived), false, false, Merged, NOW),
            Some((Shelf::Active, Section::Working))
        );
        assert_eq!(place(&old_done, false, true, Merged, NOW), Some((Shelf::Active, Section::Working)), "being started");
        // An archived project: its finished tasks are still listed, the ones that never started are not.
        assert_eq!(place(&old_done, true, false, Merged, NOW), Some((Shelf::Archived, Section::Done)));
        assert_eq!(place(&at(State::NotStarted, 0, Archiving::Auto), true, false, Merged, NOW), None);
    }

    #[test]
    fn archived_lists_every_finished_task_the_latest_first_and_the_filter_narrows_it() {
        let tasks = [
            started(task(1, "Login page", State::Done, NOW - 5 * 86_400), "s-1"),
            task(2, "Dropped idea", State::Cancelled, NOW - 2 * 86_400),
            Ticket { archiving: Archiving::Archived, ..started(task(3, "Just done", State::Done, NOW - 60), "s-3") },
            started(task(4, "Fresh", State::Done, NOW - 60), "s-4"),
            Ticket { archiving: Archiving::KeptActive, ..task(5, "Kept", State::Done, NOW - 9 * 86_400) },
        ];
        let ins: Vec<In> = tasks.iter().map(|t| In { project: "p", archived: false, task: t, busy: None }).collect();
        let numbers =
            |shelf: Shelf, q: &str| board(&ins, &Known, shelf, q).iter().flat_map(|g| g.rows.iter().map(|r| r.number)).collect::<Vec<_>>();
        assert_eq!(numbers(Shelf::Archived, ""), [3, 2, 1]);
        assert_eq!(numbers(Shelf::Active, ""), [4, 5]);
        assert_eq!(numbers(Shelf::Archived, "login"), [1]);
        assert_eq!(numbers(Shelf::Archived, "fresh"), Vec::<u32>::new(), "the filter narrows the list showing only");
        assert_eq!(counts(&ins, &Known), [2, 3]);
        assert_eq!(
            shape(&board(&ins, &Known, Shelf::Archived, "dropped")),
            vec![("Done", vec![(2, "2 days ago".to_string(), "Cancelled".to_string())])]
        );
    }

    /// `Known`, with task 6 of project `p` not merged.
    struct Unmerged;

    impl Facts for Unmerged {
        fn now(&self) -> u64 {
            Known.now()
        }
        fn squad_name(&self, id: &str) -> String {
            Known.squad_name(id)
        }
        fn tokens(&self, session: &str) -> Option<String> {
            Known.tokens(session)
        }
        fn last_wrote(&self, session: &str) -> Option<u64> {
            Known.last_wrote(session)
        }
        fn project_name(&self, id: &str) -> String {
            Known.project_name(id)
        }
        fn merge(&self, project: &str, t: &Ticket) -> Merge {
            if (project, t.number) == ("p", 6) { Merge::NotMerged } else { Merge::Merged }
        }
    }

    #[test]
    fn a_done_task_whose_branch_is_not_merged_says_so_and_a_merged_one_reads_plain_done() {
        let mut t = started(task(6, "Finished", State::Done, NOW - 60), "s-read");
        t.worktree = Some(crate::tickets::Worktree { path: "/w-worktrees/task-6".into(), branch: "task/6".into(), ..Default::default() });
        let i = In { project: "p", archived: false, task: &t, busy: None };
        let r = row(&i, Section::Done, &Unmerged);
        assert_eq!(r.second_text(), "Feature · Done · not merged · on task/6 · 84K tokens");
        assert_eq!(r.second.iter().find(|p| p.text == NOT_MERGED).map(|p| p.tone), Some(Tone::Warn));
        assert_eq!(row(&i, Section::Done, &Known).second_text(), "Feature · on task/6 · 84K tokens", "merged: plain Done");
        let bare = Ticket { squad: String::new(), ..t.clone() };
        let i = In { project: "p", archived: false, task: &bare, busy: None };
        assert_eq!(row(&i, Section::Done, &Unmerged).second_text(), "Done · not merged · on task/6 · 84K tokens");
    }

    #[test]
    fn the_verdict_line_says_checking_then_what_the_check_said_and_a_stale_mark_is_not_checking() {
        use crate::tickets::Verdict;
        let clock = |_: u64| "13:52".to_string();
        let text = |p: Vec<Piece>| p.iter().map(|x| x.text.as_str()).collect::<String>();
        let mut t = started(task(1, "T", State::Working, NOW), "s-1");
        assert!(verdict_line(&t, NOW, &clock).is_empty(), "nothing before its first check");
        t.checking = Some(crate::tickets::Checking { since: NOW - 30, model: "fable".into() });
        assert_eq!(text(verdict_line(&t, NOW, &clock)), "Fable is checking the work… 0:30", "who checks, and for how long");
        assert!(
            row(&In { project: "p", archived: false, task: &t, busy: None }, Section::Working, &Known)
                .second_text()
                .ends_with(" · checking the work")
        );
        t.checking = Some(crate::tickets::Checking { since: NOW - crate::verdict::TIMEOUT.as_secs() - 600, model: String::new() });
        assert!(!checking(&t, NOW), "a mark left by a hook that was ended");
        t.checking = None;
        t.verdict = Some(Verdict { at: NOW, model: "fable".into(), done: true, tokens: 8_200, ..Default::default() });
        assert_eq!(text(verdict_line(&t, NOW, &clock)), "done · checked by fable at 13:52 · 8.2K tokens");
        t.verdict = Some(Verdict {
            model: "fable".into(),
            done: true,
            note: "the check said not done but gave no reason".into(),
            ..Default::default()
        });
        assert_eq!(text(verdict_line(&t, NOW, &clock)), "done · checked by fable at 13:52 · the check said not done but gave no reason");
        t.verdict = Some(Verdict { model: "opus".into(), failed: Some("no answer within 600 seconds".into()), ..Default::default() });
        assert_eq!(text(verdict_line(&t, NOW, &clock)), "could not run · no answer within 600 seconds · checked by opus at 13:52");
        assert_eq!(need_words(&Need::Check("1 problem: a.rs:3: x".into())), "the check found 1 problem: a.rs:3: x");
    }

    #[test]
    fn the_filter_finds_a_task_by_its_title_its_project_or_its_squad() {
        let a = started(task(1, "Login page", State::Working, NOW), "s-1");
        let b = task(2, "Docs", State::NotStarted, NOW);
        let ins =
            vec![In { project: "web", archived: false, task: &a, busy: None }, In { project: "", archived: false, task: &b, busy: None }];
        let numbers =
            |q: &str| board(&ins, &Known, Shelf::Active, q).iter().flat_map(|g| g.rows.iter().map(|r| r.number)).collect::<Vec<_>>();
        assert_eq!(numbers("LOGIN"), [1]);
        assert_eq!(numbers("no project"), [2]);
        assert_eq!(numbers("feature"), [1]);
        assert_eq!(numbers("zzz"), Vec::<u32>::new());
    }
}
