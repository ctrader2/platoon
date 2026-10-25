//! A task's steps, as platoon follows them: the numbered top-level steps of the squad the task started with (`parse`,
//! each one's title the first sentence of its text), kept on the task (`tickets::Ticket::steps`), each with the mark the
//! session gave it with `platoon task step <n> started|done|skipped` (`Report`, read by `parse_command`; `Steps::mark`).
//! From them: the step the session is on (`Steps::current`), the steps never marked done or skipped (`unmarked`: at the
//! end of the work the session is sent back once to mark them, `send_back`), and the task's status line inside its
//! Claude Code session (`status_line`, with its helpers: `helpers_words`). Pure.
//!
//! In the tasks file: `"steps": {"list": [{"title", "mark", "at", "note"}], "sent_back": true}`, `mark` one of `started`,
//! `done`, `skipped` (no `mark`: not reached), `sent_back` only when true.

use crate::agents::Helper;
use serde_json::{Map, Value, json};
use unicode_width::UnicodeWidthStr;

/// The longest step title kept, in characters (the screens cut it further where they must).
pub const TITLE_MAX: usize = 120;

/// The longest note kept on a step (what was done, or why it was skipped), in characters.
pub const NOTE_MAX: usize = 300;

/// Where a step is.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Mark {
    /// Not reached (or not said).
    #[default]
    Open,
    /// The session said it started it, at this time (seconds since the epoch).
    Started { at: u64 },
    /// Done, with the session's one line on it.
    Done { at: u64, note: String },
    /// Left out, with why.
    Skipped { at: u64, why: String },
}

impl Mark {
    /// Done or skipped: nothing more to say about it.
    pub fn finished(&self) -> bool {
        matches!(self, Mark::Done { .. } | Mark::Skipped { .. })
    }

    /// When it was given (0 for none).
    pub fn at(&self) -> u64 {
        match self {
            Mark::Open => 0,
            Mark::Started { at } | Mark::Done { at, .. } | Mark::Skipped { at, .. } => *at,
        }
    }
}

/// One step of the squad, numbered by its place (the first is 1).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Step {
    pub title: String,
    pub mark: Mark,
}

/// A task's steps, and whether the end of its work has sent the session back to mark them in this round of work.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Steps {
    pub list: Vec<Step>,
    /// A stop that would have had the work checked sent the session back once to mark its unmarked steps. Reset when the
    /// user types (as `Ticket::checks_refused` is), so it happens once per round of work.
    pub sent_back: bool,
}

/// What `platoon task step <n> ...` says about a step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Report {
    Started,
    Done(String),
    Skipped(String),
}

impl Steps {
    /// Fresh steps with these titles, none marked.
    pub fn new(titles: &[String]) -> Steps {
        Steps { list: titles.iter().map(|t| Step { title: t.clone(), mark: Mark::Open }).collect(), sent_back: false }
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Mark step `n` (from 1) as `report` says, at `now`. A step may be marked again (a done one started over, a
    /// note put right). Err, in words for the session, when there is no such step.
    pub fn mark(&mut self, n: usize, report: &Report, now: u64) -> Result<(), String> {
        let count = self.list.len();
        if count == 0 {
            return Err("this task's squad has no numbered steps, so there is nothing to mark".into());
        }
        let step = n
            .checked_sub(1)
            .and_then(|i| self.list.get_mut(i))
            .ok_or_else(|| format!("there is no step {n}: the steps are 1 to {count}"))?;
        step.mark = match report {
            Report::Started => Mark::Started { at: now },
            Report::Done(note) => Mark::Done { at: now, note: crate::projects::clean(note, NOTE_MAX) },
            Report::Skipped(why) => Mark::Skipped { at: now, why: crate::projects::clean(why, NOTE_MAX) },
        };
        Ok(())
    }

    /// The step the session is on (its index): the one started last that isn't finished, else the first not reached.
    /// None when every step is done or skipped (or there are none).
    pub fn current(&self) -> Option<usize> {
        let started =
            self.list.iter().enumerate().filter(|(_, s)| matches!(s.mark, Mark::Started { .. })).max_by_key(|(i, s)| (s.mark.at(), *i));
        started.map(|(i, _)| i).or_else(|| self.list.iter().position(|s| s.mark == Mark::Open))
    }

    /// The steps not marked done or skipped, by number (from 1).
    pub fn unmarked(&self) -> Vec<usize> {
        self.list.iter().enumerate().filter(|(_, s)| !s.mark.finished()).map(|(i, _)| i + 1).collect()
    }

    /// When a step was last marked (0: never): the helpers shown are the ones of this stretch of the work.
    pub fn last_marked(&self) -> u64 {
        self.list.iter().map(|s| s.mark.at()).max().unwrap_or(0)
    }

    /// Where the session is, in a few words: "step 5/8 Commit the failing test first", or "all 8 steps marked". None
    /// for a squad without steps.
    pub fn words(&self) -> Option<String> {
        if self.list.is_empty() {
            return None;
        }
        Some(match self.current() {
            Some(i) => format!("{} {}", self.position(i), self.list[i].title),
            None => format!("all {} steps marked", self.list.len()),
        })
    }

    /// "step 5/8": where step `i` (an index) is among them.
    pub fn position(&self, i: usize) -> String {
        format!("step {}/{}", i + 1, self.list.len())
    }

    pub fn to_json(&self) -> Option<Value> {
        if self.list.is_empty() {
            return None;
        }
        let list: Vec<Value> = self
            .list
            .iter()
            .map(|s| {
                let mut o = Map::new();
                o.insert("title".into(), json!(s.title));
                let (mark, note) = match &s.mark {
                    Mark::Open => (None, None),
                    Mark::Started { .. } => (Some("started"), None),
                    Mark::Done { note, .. } => (Some("done"), Some(note)),
                    Mark::Skipped { why, .. } => (Some("skipped"), Some(why)),
                };
                if let Some(m) = mark {
                    o.insert("mark".into(), json!(m));
                    o.insert("at".into(), json!(s.mark.at()));
                }
                if let Some(n) = note.filter(|n| !n.is_empty()) {
                    o.insert("note".into(), json!(n));
                }
                Value::Object(o)
            })
            .collect();
        let mut o = Map::new();
        o.insert("list".into(), Value::Array(list));
        if self.sent_back {
            o.insert("sent_back".into(), json!(true));
        }
        Some(Value::Object(o))
    }

    /// Read from the tasks file, defensively: a step that isn't shaped like one is left out, a mark platoon doesn't
    /// know reads as not reached.
    pub fn from_json(v: Option<&Value>) -> Steps {
        let Some(o) = v.and_then(Value::as_object) else { return Steps::default() };
        let list = o
            .get("list")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|s| {
                let title = crate::projects::clean(s.get("title")?.as_str()?, TITLE_MAX);
                let at = s.get("at").and_then(Value::as_u64).unwrap_or(0);
                let note = crate::projects::clean(s.get("note").and_then(Value::as_str).unwrap_or_default(), NOTE_MAX);
                let mark = match s.get("mark").and_then(Value::as_str) {
                    Some("started") => Mark::Started { at },
                    Some("done") => Mark::Done { at, note },
                    Some("skipped") => Mark::Skipped { at, why: note },
                    _ => Mark::Open,
                };
                Some(Step { title, mark })
            })
            .collect();
        Steps { list, sent_back: o.get("sent_back").and_then(Value::as_bool).unwrap_or(false) }
    }
}

// ---- the steps of a squad ------------------------------------------------------------------------------------------

/// A squad's numbered top-level steps (`1. ...` or `1) ...` at the start of a line), in their order, each as its title:
/// the first sentence of its text (its lines joined), without the full stop, at most `TITLE_MAX` characters. A
/// numbered line inside a step (indented) is the step's own; the squad's steps are counted from 1 whatever numbers
/// they are written with.
pub fn parse(body: &str) -> Vec<String> {
    let mut texts: Vec<String> = Vec::new();
    let mut open = false;
    for line in body.lines() {
        if let Some(rest) = numbered(line) {
            texts.push(rest.trim().to_string());
            open = true;
        } else if line.trim().is_empty() {
            open = false;
        } else if open && line.starts_with([' ', '\t']) {
            if let Some(t) = texts.last_mut() {
                t.push(' ');
                t.push_str(line.trim());
            }
        } else {
            open = false;
        }
    }
    texts.iter().map(|t| first_sentence(t)).filter(|t| !t.is_empty()).collect()
}

/// The text after a top-level list number: `12. Text` or `12) Text`.
fn numbered(line: &str) -> Option<&str> {
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let rest = line[digits..].strip_prefix(['.', ')'])?;
    rest.starts_with([' ', '\t']).then_some(rest)
}

/// `text` without its asides in brackets ("a helper (a subagent, where you can start one), give it" -> "a helper,
/// give it"): a title says what the step is, not the fine print.
fn without_asides(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.replace(" ,", ",").replace("  ", " ")
}

/// The first sentence of `text` (its asides in brackets left out): up to the first `.`, `?` or `!` that ends a word (followed by a space or the end), the
/// full stop left off (not the one of "e.g." or "i.e."), and up to a colon or semicolon when a few words come before
/// it; at most `TITLE_MAX` characters, cut after a word.
pub fn first_sentence(text: &str) -> String {
    let text = crate::projects::clean(&without_asides(text), usize::MAX);
    let chars: Vec<char> = text.chars().collect();
    let mut end = chars.len();
    for (i, c) in chars.iter().enumerate() {
        let abbreviation = || {
            let word: String = chars[..i].iter().rev().take_while(|c| !c.is_whitespace()).collect::<Vec<_>>().into_iter().rev().collect();
            matches!(word.to_lowercase().as_str(), "e.g" | "i.e")
        };
        if matches!(c, '.' | '?' | '!') && chars.get(i + 1).is_none_or(|n| n.is_whitespace()) && !abbreviation() {
            end = if *c == '.' { i } else { i + 1 };
            break;
        }
    }
    let s: String = chars[..end].iter().collect();
    // "Name the data shape first: the types, ...": what comes before a colon says it, when that is a few words.
    let s = match s.split_once([':', ';']) {
        Some((before, _)) if before.split_whitespace().count() >= 3 => before.to_string(),
        _ => s,
    };
    if s.chars().count() <= TITLE_MAX {
        return s.trim().to_string();
    }
    let mut out = String::new();
    for word in s.split_inclusive(' ') {
        if out.chars().count() + word.trim_end().chars().count() >= TITLE_MAX {
            break;
        }
        out.push_str(word);
    }
    format!("{}…", out.trim_end())
}

// ---- the command ----------------------------------------------------------------------------------------------------

/// How the command is written, for the session and for its errors.
pub const USAGE: &str = "platoon task step <n> started | done \"<one line>\" | skipped \"<one line why>\"";

/// Read `task step`'s arguments (after `step`): the step's number and what is said about it. The words after `done` or
/// `skipped` are its one line, quoted or not; `skipped` needs one.
pub fn parse_command(args: &[String]) -> Result<(usize, Report), String> {
    let (Some(n), Some(mark)) = (args.first(), args.get(1)) else {
        return Err(format!("say which step and how: {USAGE}"));
    };
    let n: usize = n
        .trim()
        .trim_start_matches('#')
        .parse()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| format!("\"{n}\" isn't a step number (1, 2, ...)"))?;
    let line = crate::projects::clean(&args[2..].join(" "), NOTE_MAX);
    let report = match mark.trim().to_lowercase().as_str() {
        "started" | "start" => Report::Started,
        "done" => Report::Done(line),
        "skipped" | "skip" if line.is_empty() => {
            return Err(format!("say why step {n} was skipped: platoon task step {n} skipped \"<one line why>\""));
        }
        "skipped" | "skip" => Report::Skipped(line),
        other => return Err(format!("\"{other}\" isn't started, done or skipped")),
    };
    Ok((n, report))
}

/// What the command says when it has marked step `n`: "Step 3 done."
pub fn said(n: usize, report: &Report) -> String {
    match report {
        Report::Started => format!("Step {n} started."),
        Report::Done(_) => format!("Step {n} done."),
        Report::Skipped(_) => format!("Step {n} skipped."),
    }
}

/// Why the end of the work is sent back: these steps (numbers from 1) were never marked done or skipped. `exe` is
/// platoon by its full path, as the rules name it.
pub fn send_back(steps: &Steps, unmarked: &[usize], exe: &str) -> String {
    let exe = crate::agents::claude::shell_word(exe);
    let named: Vec<String> = unmarked.iter().filter_map(|n| steps.list.get(n - 1).map(|s| format!("{n} ({})", s.title))).collect();
    let which = if unmarked.len() == 1 {
        format!("Step {} isn't marked", named.join(""))
    } else {
        format!("Steps {} aren't marked", named.join(", "))
    };
    format!(
        "Before you finish: {which} done or skipped. For each one, run `{exe} task step <n> done \"<one line: what you did>\"`, or \
         `{exe} task step <n> skipped \"<one line: why>\"` for one you left out. Then finish as you were going to."
    )
}

// ---- what the session's helpers are doing ---------------------------------------------------------------------------

/// A helper that isn't finished and whose files haven't changed for this long is taken as gone (seconds).
pub const HELPER_GONE_AFTER: u64 = 15 * 60;

/// With no step marked yet, the finished helpers of this last stretch are shown (seconds).
pub const HELPERS_LOOK_BACK: u64 = 30 * 60;

/// The helpers worth showing now: the ones at work (not finished, changed in the last `HELPER_GONE_AFTER`), and the ones
/// finished since `since` (the last time a step was marked; 0: in the last `HELPERS_LOOK_BACK`), by their description.
pub fn helpers_now(all: &[Helper], since: u64, now: u64) -> Vec<Helper> {
    let since = if since == 0 { now.saturating_sub(HELPERS_LOOK_BACK) } else { since };
    let mut out: Vec<Helper> =
        all.iter().filter(|h| if h.finished { h.at >= since } else { now.saturating_sub(h.at) <= HELPER_GONE_AFTER }).cloned().collect();
    out.sort_by(|a, b| a.description.cmp(&b.description).then(a.at.cmp(&b.at)));
    out
}

/// What the helpers are called together: when every description starts with the same word for someone who does
/// something ("Reviewer A on the diff", "Reviewer B ..."), that word made plural ("Reviewers"); else "Helpers".
pub fn helpers_label(helpers: &[Helper]) -> String {
    let first = |h: &Helper| h.description.split(|c: char| !c.is_alphabetic()).find(|w| !w.is_empty()).map(str::to_lowercase);
    let words: Vec<Option<String>> = helpers.iter().map(first).collect();
    match words.first() {
        Some(Some(w))
            if w.chars().count() > 3 && (w.ends_with("er") || w.ends_with("or")) && words.iter().all(|x| x.as_ref() == Some(w)) =>
        {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect::<String>() + "s").unwrap_or_default()
        }
        _ => "Helpers".into(),
    }
}

/// A helper as it is named: its model's name, else its kind when that says more than "general-purpose", else "Helper".
fn helper_name(h: &Helper) -> String {
    match (&h.model, h.kind.as_str()) {
        (Some(m), _) => crate::agents::model_display_name(m),
        (None, k) if !k.is_empty() && k != "general-purpose" => k.to_string(),
        _ => "Helper".into(),
    }
}

/// The helpers in a few words: "Reviewers: Opus ✓ · Fable … · Sonnet ✓" (`✓` finished, `…` at work), or, `short`,
/// "Reviewers 2/3 done". None when there are none.
pub fn helpers_words(helpers: &[Helper], short: bool) -> Option<String> {
    if helpers.is_empty() {
        return None;
    }
    let label = helpers_label(helpers);
    if short {
        let done = helpers.iter().filter(|h| h.finished).count();
        return Some(format!("{label} {done}/{} done", helpers.len()));
    }
    let each: Vec<String> = helpers.iter().map(|h| format!("{} {}", helper_name(h), if h.finished { "✓" } else { "…" })).collect();
    Some(format!("{label}: {}", each.join(" · ")))
}

/// "Fable is checking the work… 0:23": the check of the work running for `secs`, by the model it runs on.
pub fn checking_words(model: &str, secs: u64) -> String {
    let who = if model.is_empty() { "The check".to_string() } else { crate::agents::model_display_name(model) };
    format!("{who} is checking the work… {}:{:02}", secs / 60, secs % 60)
}

// ---- the status line ------------------------------------------------------------------------------------------------

/// What a task's status line says.
pub struct StatusLine<'a> {
    pub number: u32,
    /// Its squad's name ("" for none).
    pub squad: &'a str,
    pub steps: &'a Steps,
    /// The session's model, as Claude Code names it ("Opus").
    pub model: &'a str,
    /// The helpers worth showing (`helpers_now`).
    pub helpers: &'a [Helper],
    /// The check of the work, running: its model and for how long (seconds).
    pub checking: Option<(&'a str, u64)>,
    /// The plan usage the status line says for every session ("5h 12% · 7d 40%"; "" for none).
    pub usage: &'a str,
}

/// The task's status line in at most `width` cells: `#5 Bug fix · step 5/8 Commit the failing test first · Opus`, then
/// the helpers, the check running and the plan usage. When it doesn't fit, it gives way in this order: the usage, the
/// model, the helpers named one by one (counted instead), the step's title (cut after a word), the helpers, the squad's
/// name; last of all the line is cut.
pub fn status_line(s: &StatusLine, width: usize) -> String {
    let head_full = if s.squad.is_empty() { format!("#{}", s.number) } else { format!("#{} {}", s.number, s.squad) };
    let current = s.steps.current();
    let checking = s.checking.map(|(m, secs)| checking_words(m, secs));
    // Level 0 has everything; each level after it gives one more thing way. `title_room`: the step's title cut to fit.
    let build = |level: u8, title_room: Option<usize>| -> String {
        let mut parts: Vec<String> = vec![if level >= 5 { format!("#{}", s.number) } else { head_full.clone() }];
        match current {
            Some(i) => {
                let pos = s.steps.position(i);
                let title = title_room.map_or_else(|| s.steps.list[i].title.clone(), |room| fit(&s.steps.list[i].title, room));
                parts.push(if title.is_empty() { pos } else { format!("{pos} {title}") });
            }
            None => parts.extend(s.steps.words()),
        }
        if level < 2 && !s.model.is_empty() {
            parts.push(s.model.to_string());
        }
        if level < 4 {
            parts.extend(helpers_words(s.helpers, level >= 3));
        }
        parts.extend(checking.clone());
        if level < 1 && !s.usage.is_empty() {
            parts.push(s.usage.to_string());
        }
        parts.join(" · ")
    };
    for level in 0..=5u8 {
        let line = build(level, None);
        if line.width() <= width {
            return line;
        }
        // From the helpers counted on, the step's title is cut to the room left before anything more goes, while a
        // few words of it are left (any at the last).
        let least = match level {
            3 => 16,
            4 => 12,
            _ => 1,
        };
        if level >= 3 && current.is_some() {
            let room = width.saturating_sub(build(level, Some(0)).width() + 1);
            let line = build(level, Some(room));
            if room >= least && line.width() <= width {
                return line;
            }
        }
    }
    fit(&build(5, Some(0)), width)
}

/// `s` in at most `n` cells: whole, or cut after a word with "…" (a first word longer than that is cut where it must
/// be); "" for no room.
fn fit(s: &str, n: usize) -> String {
    if s.width() <= n {
        return s.to_string();
    }
    if n == 0 {
        return String::new();
    }
    let room = n - 1;
    let mut out = String::new();
    for word in s.split_inclusive(' ') {
        if out.width() + word.trim_end().width() > room {
            break;
        }
        out.push_str(word);
    }
    let out = out.trim_end();
    if !out.is_empty() {
        return format!("{out}…");
    }
    let mut cut = String::new();
    for c in s.chars() {
        if cut.width() + unicode_width::UnicodeWidthChar::width(c).unwrap_or(0) > room {
            break;
        }
        cut.push(c);
    }
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(titles: &[&str]) -> Steps {
        Steps::new(&titles.iter().map(|t| t.to_string()).collect::<Vec<_>>())
    }

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_squads_steps_are_its_numbered_top_level_items_each_titled_by_its_first_sentence() {
        let body = "You own this. Copy nothing.\n\n\
                    1. Reproduce it yourself, on the surface where it was reported: run the command. Ask the user only\n   when you can't.\n\
                    2. Name the cause\n   in one line. Then go on.\n   1. A numbered line inside a step is the step's own.\n\
                    3) Is it fixed? Check it.\n\n\
                    Some closing words.\n\
                    9. Numbers as written don't matter, e.g. this is the fourth. More.\n";
        assert_eq!(
            parse(body),
            [
                "Reproduce it yourself, on the surface where it was reported",
                "Name the cause in one line",
                "Is it fixed?",
                "Numbers as written don't matter, e.g. this is the fourth",
            ]
        );
        let long = format!("1. {}.", "word ".repeat(60));
        let t = &parse(&long)[0];
        assert!(t.ends_with("word…") && t.chars().count() <= TITLE_MAX, "{t}");
        assert!(parse("No steps here, just prose.\n- a bullet\n").is_empty());
        // Every built-in squad has its steps.
        for (id, text) in crate::squads::BUILT_IN {
            let s = crate::squads::parse(id, text, crate::squads::Origin::BuiltIn);
            assert!(parse(&s.body).len() >= 5, "{id}");
        }
        let feature = crate::squads::parse("feature", crate::squads::BUILT_IN[0].1, crate::squads::Origin::BuiltIn);
        let titles = parse(&feature.body);
        assert_eq!(titles.len(), 9);
        assert_eq!(titles[0], "Learn how the affected part works before you plan");
        assert_eq!(titles[7], "Before you finish, follow the Review squad on your diff");
        let review = crate::squads::parse("review", crate::squads::BUILT_IN[5].1, crate::squads::Origin::BuiltIn);
        assert_eq!(parse(&review.body)[2], "Start four reviewers, A to D, all at once, each on a different model where you can");
    }

    #[test]
    fn the_current_step_is_the_last_one_started_and_not_finished_else_the_next_not_reached() {
        let mut s = steps(&["Look", "Plan", "Build", "Check"]);
        assert_eq!(s.current(), Some(0));
        assert_eq!(s.words().as_deref(), Some("step 1/4 Look"));
        s.mark(1, &Report::Done("read it".into()), 10).unwrap();
        assert_eq!(s.current(), Some(1), "the next one not reached");
        s.mark(3, &Report::Started, 20).unwrap();
        s.mark(2, &Report::Started, 30).unwrap();
        assert_eq!(s.current(), Some(1), "started last");
        s.mark(2, &Report::Skipped("nothing contested".into()), 40).unwrap();
        assert_eq!(s.current(), Some(2), "the one still started");
        assert_eq!(s.unmarked(), [3, 4]);
        s.mark(3, &Report::Done(String::new()), 50).unwrap();
        s.mark(4, &Report::Done("cargo test: 12 passed".into()), 60).unwrap();
        assert_eq!((s.current(), s.words().as_deref(), s.unmarked().len()), (None, Some("all 4 steps marked"), 0));
        assert_eq!(s.last_marked(), 60);
        // Out of range, or no steps at all: said plainly.
        assert_eq!(s.mark(5, &Report::Started, 1), Err("there is no step 5: the steps are 1 to 4".into()));
        assert_eq!(s.mark(0, &Report::Started, 1), Err("there is no step 0: the steps are 1 to 4".into()));
        assert!(Steps::default().mark(1, &Report::Started, 1).unwrap_err().contains("no numbered steps"));
        assert_eq!(Steps::default().words(), None);
    }

    #[test]
    fn steps_go_into_the_tasks_file_and_come_back_and_odd_entries_are_read_defensively() {
        let mut s = steps(&["Look", "Plan", "Build"]);
        s.mark(1, &Report::Done("read it".into()), 10).unwrap();
        s.mark(2, &Report::Skipped("no design to contest".into()), 20).unwrap();
        s.mark(3, &Report::Started, 30).unwrap();
        s.sent_back = true;
        let v = s.to_json().unwrap();
        assert_eq!(v["list"][0], json!({"title": "Look", "mark": "done", "at": 10, "note": "read it"}));
        assert_eq!(v["list"][2], json!({"title": "Build", "mark": "started", "at": 30}));
        assert_eq!(Steps::from_json(Some(&v)), s);
        assert_eq!(Steps::default().to_json(), None, "no key for a task without steps");
        let odd = json!({"list": [{"title": "Look", "mark": "wandering"}, {"mark": "done"}, 7, {"title": "Plan", "mark": "done"}]});
        let read = Steps::from_json(Some(&odd));
        assert_eq!(read.list.len(), 2);
        assert_eq!(read.list[0].mark, Mark::Open);
        assert_eq!(read.list[1].mark, Mark::Done { at: 0, note: String::new() });
        assert_eq!(Steps::from_json(Some(&json!("nonsense"))), Steps::default());
    }

    #[test]
    fn the_command_takes_a_number_and_a_mark_and_skipping_needs_a_reason() {
        assert_eq!(parse_command(&args(&["3", "started"])), Ok((3, Report::Started)));
        assert_eq!(parse_command(&args(&["3", "done", "wrote", "the", "test"])), Ok((3, Report::Done("wrote the test".into()))));
        assert_eq!(parse_command(&args(&["#4", "DONE"])), Ok((4, Report::Done(String::new()))));
        assert_eq!(parse_command(&args(&["2", "skipped", "nothing to review"])), Ok((2, Report::Skipped("nothing to review".into()))));
        assert_eq!(
            parse_command(&args(&["2", "skipped"])).unwrap_err(),
            "say why step 2 was skipped: platoon task step 2 skipped \"<one line why>\""
        );
        assert_eq!(parse_command(&args(&["two", "done"])).unwrap_err(), "\"two\" isn't a step number (1, 2, ...)");
        assert_eq!(parse_command(&args(&["0", "done"])).unwrap_err(), "\"0\" isn't a step number (1, 2, ...)");
        assert_eq!(parse_command(&args(&["2", "finished"])).unwrap_err(), "\"finished\" isn't started, done or skipped");
        assert!(parse_command(&args(&["2"])).unwrap_err().starts_with("say which step and how"));
        assert_eq!(said(3, &Report::Done(String::new())), "Step 3 done.");
    }

    #[test]
    fn the_send_back_names_each_unmarked_step_and_the_command_by_platoons_path() {
        let s = steps(&["Look", "Plan", "Build"]);
        let one = send_back(&s, &[2], "/opt/my tools/platoon");
        assert!(one.starts_with("Before you finish: Step 2 (Plan) isn't marked done or skipped."), "{one}");
        assert!(one.contains("`'/opt/my tools/platoon' task step <n> done \"<one line: what you did>\"`"), "{one}");
        let two = send_back(&s, &[1, 3], "/usr/bin/platoon");
        assert!(two.starts_with("Before you finish: Steps 1 (Look), 3 (Build) aren't marked"), "{two}");
        assert!(two.contains("`/usr/bin/platoon task step <n> skipped \"<one line: why>\"`"), "{two}");
    }

    fn helper(description: &str, model: Option<&str>, finished: bool, at: u64) -> Helper {
        Helper { description: description.into(), model: model.map(String::from), kind: "general-purpose".into(), finished, at }
    }

    #[test]
    fn the_helpers_shown_are_the_ones_at_work_and_the_ones_finished_in_this_stretch_named_by_model() {
        let now = 10_000;
        let all = [
            helper("Reviewer D on the diff", Some("haiku"), true, 9_900),
            helper("Reviewer A on the diff", Some("opus"), true, 9_800),
            helper("Reviewer B on the diff", Some("fable"), false, 9_990),
            helper("Reviewer C on the diff", Some("sonnet"), true, 9_950),
            helper("Explorer of an earlier step", Some("opus"), true, 5_000),
            helper("Gone quiet long ago", Some("opus"), false, now - HELPER_GONE_AFTER - 1),
        ];
        let shown = helpers_now(&all, 9_000, now);
        assert_eq!(helpers_words(&shown, false).as_deref(), Some("Reviewers: Opus ✓ · Fable … · Sonnet ✓ · Haiku ✓"));
        assert_eq!(helpers_words(&shown, true).as_deref(), Some("Reviewers 3/4 done"));
        // Mixed descriptions are helpers; no model says its kind, or just "Helper".
        let mixed = [
            helper("Explorer: the index", Some("claude-sonnet-5-5"), false, now),
            Helper { kind: "Explore".into(), ..helper("Look at the UI", None, true, now) },
            helper("Write the test", None, true, now),
        ];
        assert_eq!(helpers_words(&helpers_now(&mixed, 0, now), false).as_deref(), Some("Helpers: Sonnet 5.5 … · Explore ✓ · Helper ✓"));
        assert_eq!(helpers_label(&[helper("Explore the UI", None, true, 1), helper("Explore the data", None, true, 1)]), "Helpers");
        assert_eq!(helpers_words(&[], false), None);
        // No step marked yet: the finished ones of the last half hour.
        assert_eq!(helpers_now(&all, 0, now).len(), 4);
    }

    #[test]
    fn the_status_line_names_the_task_its_step_and_model_then_helpers_the_check_and_usage_and_gives_way_in_order() {
        let mut s = steps(&["Look", "Plan", "Build", "Write the test", "Commit the failing test first", "Fix it", "Review", "Reply"]);
        for n in 1..=4 {
            s.mark(n, &Report::Done(String::new()), n as u64).unwrap();
        }
        s.mark(5, &Report::Started, 9).unwrap();
        let helpers = [helper("Reviewer A", Some("opus"), true, 1), helper("Reviewer B", Some("fable"), false, 1)];
        let all = StatusLine { number: 5, squad: "Bug fix", steps: &s, model: "Opus", helpers: &helpers, checking: None, usage: "5h 12%" };
        let at = |w: usize| status_line(&all, w);
        assert_eq!(at(200), "#5 Bug fix · step 5/8 Commit the failing test first · Opus · Reviewers: Opus ✓ · Fable … · 5h 12%");
        // Narrower: the usage goes first, then the model, then the helpers are counted, then the title is cut, then the
        // helpers go, then the squad's name.
        assert_eq!(at(97), at(200), "all of it, to the cell");
        assert_eq!(at(96), "#5 Bug fix · step 5/8 Commit the failing test first · Opus · Reviewers: Opus ✓ · Fable …");
        assert_eq!(at(87), "#5 Bug fix · step 5/8 Commit the failing test first · Reviewers: Opus ✓ · Fable …");
        assert_eq!(at(80), "#5 Bug fix · step 5/8 Commit the failing test first · Reviewers 1/2 done");
        assert_eq!(at(64), "#5 Bug fix · step 5/8 Commit the failing… · Reviewers 1/2 done");
        assert_eq!(at(51), "#5 Bug fix · step 5/8 Commit the failing test first");
        assert_eq!(at(46), "#5 Bug fix · step 5/8 Commit the failing test…");
        assert_eq!(at(30), "#5 · step 5/8 Commit the…");
        assert_eq!(at(25), "#5 · step 5/8 Commit the…");
        for w in 0..=120 {
            assert!(at(w).width() <= w, "{w}: {}", at(w));
        }
        // The check running stays while there is room for it.
        let checking = StatusLine { checking: Some(("fable", 83)), usage: "", helpers: &[], ..all };
        assert_eq!(
            status_line(&checking, 200),
            "#5 Bug fix · step 5/8 Commit the failing test first · Opus · Fable is checking the work… 1:23"
        );
        assert_eq!(checking_words("gpt-6-luna", 5), "GPT-6-Luna is checking the work… 0:05");
        // Every step marked; no steps at all.
        let mut done = s.clone();
        for n in 5..=8 {
            done.mark(n, &Report::Done(String::new()), 20).unwrap();
        }
        let plain = |steps: &Steps| {
            status_line(&StatusLine { number: 2, squad: "", steps, model: "Sonnet", helpers: &[], checking: None, usage: "" }, 100)
        };
        assert_eq!(plain(&done), "#2 · all 8 steps marked · Sonnet");
        assert_eq!(plain(&Steps::default()), "#2 · Sonnet");
    }
}
