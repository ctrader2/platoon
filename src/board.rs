//! The Tasks tab: every task across all projects and the ones in no project, grouped by what it needs from the user:
//! **Needs you**, **Working**, **Not started** and **Done** (finished today). Every row answers the same questions in
//! the same places: line one says what it is (the project's badge, `#N`, the title, drawn by the UI) and at the right
//! the section's word for it (what it needs, how long it has worked, when it finished); line two says which squad has
//! it and what it needs from the user, in the session's own words where it has some. Pure: the app hands it the tasks
//! and what it knows about their sessions.

use crate::tickets::{Need, State, Ticket};

/// A group of the board, in the order they are shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    NeedsYou,
    Working,
    NotStarted,
    /// Done or Cancelled in the last day.
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

/// How long Done and Cancelled tasks stay on the board, in seconds.
pub const FINISHED_FOR: u64 = 24 * 3600;

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

pub fn piece(text: impl Into<String>, tone: Tone) -> Piece {
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
    /// A Done task's branch has commits its repository's default branch hasn't (as git last said).
    fn not_merged(&self, _project: &str, _number: u32) -> bool {
        false
    }
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

/// The board's section for a task, or None when it isn't on the board (finished before today, or a task of an
/// archived project that never started).
pub fn section_of(t: &Ticket, archived: bool, busy: bool, now: u64) -> Option<Section> {
    if busy {
        return Some(Section::Working);
    }
    match &t.state {
        State::NeedsYou(_) => Some(Section::NeedsYou),
        State::Working => Some(Section::Working),
        State::NotStarted if archived => None,
        State::NotStarted => Some(Section::NotStarted),
        State::Done | State::Cancelled => (now.saturating_sub(t.updated) < FINISHED_FOR).then_some(Section::Done),
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
    t.state == State::Working && t.checking.is_some_and(|at| now.saturating_sub(at) <= crate::verdict::TIMEOUT.as_secs() + 120)
}

/// A task's Verdict line in its details (after the label "Verdict"): checking now, or what the last check said, who
/// checked and when. Nothing before its first check.
pub fn verdict_line(t: &Ticket, now: u64, clock: &dyn Fn(u64) -> String) -> Vec<Piece> {
    if checking(t, now) {
        return vec![piece("checking the work…", Tone::Busy)];
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
            if f.not_merged(i.project, t.number) {
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

/// The board: the tasks in their sections, in order, each sorted by its latest change first, except Needs you, where
/// the one waiting longest comes first. Only the rows that match `filter`; empty sections are left out.
pub fn board(tasks: &[In], f: &dyn Facts, filter: &str) -> Vec<Group> {
    let now = f.now();
    let mut groups: Vec<Group> = SECTIONS.iter().map(|s| Group { section: *s, rows: Vec::new() }).collect();
    for i in tasks {
        let Some(section) = section_of(i.task, i.archived, i.busy.is_some(), now) else { continue };
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

/// What the board says when nothing is on it.
pub const EMPTY: &str = "No tasks yet. Press n to write one.";

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
        let g = board(&ins, &Known, "");
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
            "Needs you: the longest waiting first; elsewhere the latest first; finished before today left out"
        );
        assert_eq!(need_count(tasks.iter()), 4);
        assert_eq!(header_words(1).as_deref(), Some("● 1 needs you"));
        assert_eq!(header_words(0), None);
    }

    #[test]
    fn a_starting_task_works_and_an_archived_projects_unstarted_tasks_are_left_out() {
        let t = task(1, "Go", State::NotStarted, NOW);
        let g = board(&[In { project: "p", archived: false, task: &t, busy: Some("Making its worktree…") }], &Known, "");
        assert_eq!(shape(&g), vec![("Working", vec![(1, "starting".to_string(), "Making its worktree…".to_string())])]);
        assert!(board(&[In { project: "p", archived: true, task: &t, busy: None }], &Known, "").is_empty());
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
        fn not_merged(&self, project: &str, number: u32) -> bool {
            (project, number) == ("p", 6)
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
        t.checking = Some(NOW - 30);
        assert_eq!(text(verdict_line(&t, NOW, &clock)), "checking the work…");
        assert!(
            row(&In { project: "p", archived: false, task: &t, busy: None }, Section::Working, &Known)
                .second_text()
                .ends_with(" · checking the work")
        );
        t.checking = Some(NOW - crate::verdict::TIMEOUT.as_secs() - 600);
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
        let numbers = |q: &str| board(&ins, &Known, q).iter().flat_map(|g| g.rows.iter().map(|r| r.number)).collect::<Vec<_>>();
        assert_eq!(numbers("LOGIN"), [1]);
        assert_eq!(numbers("no project"), [2]);
        assert_eq!(numbers("feature"), [1]);
        assert_eq!(numbers("zzz"), Vec::<u32>::new());
    }
}
