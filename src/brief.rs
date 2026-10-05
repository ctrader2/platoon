//! A squad's **brief**: the Markdown file the user owns, `<config>/briefs/<id>.md`, read and written here (pure: text
//! in, a `Squad` out, and back). The store, the built-ins and the old JSON files are in `squadfile.rs`.
//!
//! A brief is a short front matter and headed sections:
//!
//! ```text
//! ---
//! id: coding
//! name: Coding
//! ---
//! # What it does
//! Makes a change to code ...
//!
//! # Steps
//! 1. planner: Reads the task ...
//! 2. builder: Makes the change ...
//! 3. reviewer: Reads the change ... May send the work back to step 2, at most twice.
//!
//! # Checks
//! - the tests pass
//!
//! # Asks you
//! Before anything others can see, and when it is stuck.
//! ```
//!
//! Reading is forgiving in whitespace and case (headings at level 1 or 2 for the known sections, `1.` or `1)` or a
//! bullet for a step, a step's text going on over the lines under it) and strict in meaning: what can't be read as a
//! brief is said in one line, with where (`Squad::shape`), and `squadfile::problems` adds what is wrong given the
//! agents there are. Sections platoon doesn't know are kept as they were written and shown; so are front matter lines.
//!
//! A step may send the work back to an earlier step: a sentence of its text says so, "May send the work back to step
//! 2, at most twice." (`once`, `twice`, `3 times`, `three times`; "send it back" too), and is taken out of what the
//! step does. A check is a line of the Checks section; `check_kind` says which of the few checks platoon runs it is,
//! if any (the rest the squad checks itself).

use crate::squadfile::{Back, Squad, Step, one_line};

/// The sections platoon knows, as the file heads them.
pub const WHAT_IT_DOES: &str = "What it does";
pub const STEPS: &str = "Steps";
pub const CHECKS: &str = "Checks";
pub const ASKS_YOU: &str = "Asks you";

/// Where things are in the file a squad was read from (1-based line numbers; 0: not from a file, or not there).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct At {
    /// The front matter's first line.
    pub front: usize,
    pub id: usize,
    pub name: usize,
    pub can_do: usize,
    pub steps: usize,
    pub checks: usize,
    /// Each check's own line, in the order of `Squad::checks`.
    pub check_lines: Vec<usize>,
}

// ---- the checks ---------------------------------------------------------------------------------------------------

/// What a line of a brief's Checks section is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckKind {
    /// "the tests pass": platoon runs the project's test command (`checks.rs`).
    Tests,
    /// "<agent> read the change": a run of that agent after the last run of any agent that can change files.
    Read(String),
    /// "every acceptance box is ticked, with how": the result ticks every acceptance line of the task.
    Acceptance,
    /// "nothing others see without your yes": the publish gate platoon holds anyway.
    Gate,
    /// Anything else: the squad checks it itself (its leader is reminded).
    Prose,
}

/// A check's words as they are compared: lower case, curly quotes straight, only letters, digits, `'` and `-`
/// between single spaces, no full stop at the end.
fn plain_words(line: &str) -> String {
    let mut out = String::new();
    for c in line.chars() {
        let c = match c {
            '\u{2019}' | '\u{2018}' => '\'',
            c => c,
        };
        if c.is_alphanumeric() || c == '\'' || c == '-' {
            out.extend(c.to_lowercase());
        } else {
            out.push(' ');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What a line of the Checks section is: one of the few checks platoon runs or holds, matched loosely (case,
/// punctuation, a leading "the" or "all") but only as the whole line, so "the tests pass on macOS" is the squad's own.
pub fn check_kind(line: &str) -> CheckKind {
    let w = plain_words(line);
    let w = w.as_str();
    let tests = ["tests pass", "tests passed", "the tests pass", "the tests passed", "all tests pass", "all the tests pass"];
    if tests.contains(&w) || w == "the project's tests pass" {
        return CheckKind::Tests;
    }
    let boxes = ["every acceptance box", "each acceptance box", "all acceptance boxes", "all the acceptance boxes"];
    let checks = ["every acceptance check", "each acceptance check", "all acceptance checks", "all the acceptance checks"];
    for lead in boxes.iter().chain(checks.iter()) {
        if let Some(rest) = w.strip_prefix(lead) {
            let rest = rest.trim();
            let rest = rest.strip_prefix("is").or_else(|| rest.strip_prefix("are")).map(str::trim).unwrap_or(rest);
            if ["ticked", "ticked with how", "checked", "checked with how"].contains(&rest) {
                return CheckKind::Acceptance;
            }
        }
    }
    if w.starts_with("nothing ")
        && ["without your yes", "without your ok", "without your go-ahead"].iter().any(|end| w.ends_with(end))
        && ["nothing others see", "nothing others can see", "nothing goes out", "nothing is published", "nothing anyone else sees"]
            .iter()
            .any(|lead| w.starts_with(lead))
    {
        return CheckKind::Gate;
    }
    // "<agent> read the change": "the reviewer read the change", "reviewer reads the changes", "the reviewer has read it".
    let words: Vec<&str> = w.split(' ').collect();
    let words: &[&str] = match words.first() {
        Some(&"the" | &"a" | &"an") => &words[1..],
        _ => &words[..],
    };
    let tail: &[&[&str]] =
        &[&["read", "the", "change"], &["reads", "the", "change"], &["read", "the", "changes"], &["reads", "the", "changes"]];
    if let [agent, rest @ ..] = words {
        let rest = match rest.first() {
            Some(&"has") => &rest[1..],
            _ => rest,
        };
        let agent_ok = agent.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && !agent.is_empty();
        if agent_ok && tail.contains(&rest) {
            return CheckKind::Read(agent.to_string());
        }
    }
    CheckKind::Prose
}

// ---- send-backs ---------------------------------------------------------------------------------------------------

/// "once", "twice", "3 times".
pub fn times_words(n: u32) -> String {
    match n {
        1 => "once".into(),
        2 => "twice".into(),
        n => format!("{n} times"),
    }
}

/// The sentence a step's text holds when it may send the work back: "May send the work back to step 2, at most
/// twice."
pub fn send_back_sentence(b: &Back) -> String {
    format!("May send the work back to step {}, at most {}.", b.to + 1, times_words(b.times))
}

/// A number said in words or digits ("3", "three"), up to ten.
fn number_of(word: &str) -> Option<u32> {
    let words = ["one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten"];
    word.parse().ok().or_else(|| words.iter().position(|w| *w == word).map(|i| i as u32 + 1))
}

/// Find the send-back sentence in a step's text: the text without it, and what it says (or what is wrong with it, in
/// a few words). None: the step never sends the work back.
fn take_send_back(does: &str) -> (String, Option<Result<Back, String>>) {
    // ASCII lower case keeps every byte where it was, so places found in it are places in `does`.
    let lower = does.to_ascii_lowercase();
    let leads = ["may send the work back", "may send it back", "can send the work back", "can send it back"];
    let Some((start, lead)) = leads.iter().filter_map(|l| lower.find(l).map(|at| (at, *l))).min_by_key(|(at, _)| *at) else {
        return (does.to_string(), None);
    };
    // The sentence ends at the first full stop after it (or the text's end).
    let after = start + lead.len();
    let end = lower[after..].find('.').map_or(lower.len(), |i| after + i + 1);
    let sentence = lower[after..end].trim_end_matches('.').trim();
    let rest = format!("{} {}", does[..start].trim_end(), does[end..].trim_start()).trim().to_string();
    let words: Vec<&str> = sentence.split(|c: char| c.is_whitespace() || c == ',').filter(|w| !w.is_empty()).collect();
    let back = (|| {
        let to_at = words.iter().position(|w| *w == "step").ok_or("say which step it may send the work back to")?;
        let to = words
            .get(to_at + 1)
            .and_then(|w| w.parse::<usize>().ok())
            .filter(|n| *n >= 1)
            .ok_or("say which step it may send the work back to")?;
        let most = words.iter().position(|w| *w == "most").ok_or("say how often: \"at most once\"")?;
        let times = match words.get(most + 1) {
            Some(&"once") => 1,
            Some(&"twice") => 2,
            Some(w) => {
                let n = number_of(w).ok_or("say how often: \"at most once\", \"twice\" or \"3 times\"")?;
                if words.get(most + 2) != Some(&"times") && n != 1 {
                    return Err("say how often: \"at most once\", \"twice\" or \"3 times\"");
                }
                n
            }
            None => return Err("say how often: \"at most once\""),
        };
        Ok(Back { to: to - 1, times })
    })();
    (rest, Some(back.map_err(String::from)))
}

// ---- reading ------------------------------------------------------------------------------------------------------

/// A heading line: its level and its text.
fn heading(line: &str) -> Option<(usize, String)> {
    let t = line.trim_start();
    let level = t.chars().take_while(|c| *c == '#').count();
    let rest = &t[level..];
    if !(1..=6).contains(&level) || !rest.starts_with([' ', '\t']) {
        return None;
    }
    Some((level, rest.trim().trim_end_matches('#').trim().to_string()))
}

/// The known section a heading's text names, if it does (case, spaces and a colon at the end don't matter).
fn known(title: &str) -> Option<&'static str> {
    let t = title.trim_end_matches(':').split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    [WHAT_IT_DOES, STEPS, CHECKS, ASKS_YOU].into_iter().find(|k| k.to_lowercase() == t)
}

/// A section's text as paragraphs: lines joined into one paragraph each, a blank line between paragraphs.
fn paragraphs(lines: &[(usize, &str)]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    for (_, l) in lines {
        if l.trim().is_empty() {
            if !cur.is_empty() {
                out.push(one_line(&cur.join(" ")));
                cur.clear();
            }
        } else {
            cur.push(l);
        }
    }
    if !cur.is_empty() {
        out.push(one_line(&cur.join(" ")));
    }
    out.join("\n\n")
}

/// A section's text as written, with the blank lines around it taken off.
fn verbatim(lines: &[(usize, &str)]) -> String {
    let text: Vec<&str> = lines.iter().map(|(_, l)| *l).collect();
    text.join("\n").trim_matches('\n').trim_end().to_string()
}

/// The start of a step: its number (None for a bullet) and the rest of the line.
fn step_start(line: &str) -> Option<(Option<usize>, &str)> {
    let t = line.trim_start();
    if let Some(rest) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")).or_else(|| t.strip_prefix("+ ")) {
        return Some((None, rest));
    }
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let rest = &t[digits..];
    let rest = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")).or_else(|| rest.strip_prefix(".\t"))?;
    Some((t[..digits].parse().ok(), rest))
}

/// A section as it is read: its heading's text, the heading's line, and the lines under it (each with its number).
type Section<'a> = (String, usize, Vec<(usize, &'a str)>);

/// Read a brief's text. `id`: its file's name (without `.md`), which is the squad's id. Never fails: whatever can't be
/// read as a brief is said in `Squad::shape`, one line each, with where.
pub fn parse(text: &str, id: &str) -> Squad {
    let mut s = Squad { id: id.to_string(), ..Squad::default() };
    let lines: Vec<(usize, &str)> = text.lines().enumerate().map(|(i, l)| (i + 1, l.trim_end_matches('\r'))).collect();
    let mut i = 0;
    while i < lines.len() && lines[i].1.trim().is_empty() {
        i += 1;
    }
    // The front matter.
    if i < lines.len() && lines[i].1.trim() == "---" {
        s.at.front = lines[i].0;
        i += 1;
        let mut closed = false;
        while i < lines.len() {
            let (n, l) = lines[i];
            i += 1;
            if l.trim() == "---" {
                closed = true;
                break;
            }
            if l.trim().is_empty() {
                continue;
            }
            let Some((k, v)) = l.split_once(':') else {
                s.shape.push(format!("Front matter, line {n}: \"{}\" isn't a \"key: value\" line.", l.trim()));
                continue;
            };
            let v = v.trim().trim_matches(|c| c == '"' || c == '\'').trim().to_string();
            match k.trim().to_lowercase().as_str() {
                "id" => {
                    s.at.id = n;
                    if v != id {
                        s.shape.push(format!("Front matter, line {n}: its id is \"{v}\", but the file is {id}.md: make them the same."));
                    }
                }
                "name" => {
                    s.at.name = n;
                    s.name = one_line(&v);
                }
                _ => s.meta.push((k.trim().to_string(), v)),
            }
        }
        if !closed {
            s.shape.push(format!("Front matter, line {}: it has no closing \"---\" line.", s.at.front));
        }
    } else {
        s.shape.push("The brief has no front matter: start it with a \"---\" line, \"name: ...\" and another \"---\".".into());
    }
    // The sections: (its name, the heading's line, the lines under it).
    let mut sections: Vec<Section> = Vec::new();
    let mut before: Vec<(usize, &str)> = Vec::new();
    for &(n, l) in &lines[i..] {
        let starts = heading(l).filter(|(level, title)| *level == 1 || (*level == 2 && known(title).is_some()));
        match starts {
            Some((_, title)) => sections.push((title, n, Vec::new())),
            None => match sections.last_mut() {
                Some(sec) => sec.2.push((n, l)),
                None => before.push((n, l)),
            },
        }
    }
    if before.iter().any(|(_, l)| !l.trim().is_empty()) {
        s.other.push((String::new(), verbatim(&before)));
    }
    let mut seen: Vec<&str> = Vec::new();
    for (title, n, body) in &sections {
        let Some(k) = known(title) else {
            s.other.push((title.clone(), verbatim(body)));
            continue;
        };
        if seen.contains(&k) {
            s.shape.push(format!("Line {n}: a second \"# {k}\" section; a brief has one."));
            continue;
        }
        seen.push(k);
        match k {
            WHAT_IT_DOES => {
                s.at.can_do = *n;
                s.can_do = paragraphs(body);
            }
            ASKS_YOU => s.asks_you = paragraphs(body),
            CHECKS => {
                s.at.checks = *n;
                for &(ln, l) in body {
                    let t = l.trim();
                    if t.is_empty() {
                        continue;
                    }
                    let item = step_start(t).map_or(t, |(_, rest)| rest).trim();
                    let item = item.strip_prefix("[ ] ").or_else(|| item.strip_prefix("[x] ")).unwrap_or(item).trim();
                    if !item.is_empty() {
                        s.checks.push(item.to_string());
                        s.at.check_lines.push(ln);
                    }
                }
            }
            _ => {
                s.at.steps = *n;
                read_steps(&mut s, body);
            }
        }
    }
    for k in [WHAT_IT_DOES, STEPS] {
        if !seen.contains(&k) {
            s.shape.push(format!("The brief has no \"# {k}\" section."));
        }
    }
    s
}

/// The Steps section's lines into `s.workflow`: each step `1. agent: what it does`, its text going on over the lines
/// under it, the send-back sentence taken out.
fn read_steps(s: &mut Squad, body: &[(usize, &str)]) {
    // (its line, its written number, the text after the number)
    let mut raw: Vec<(usize, Option<usize>, String)> = Vec::new();
    for &(n, l) in body {
        if l.trim().is_empty() {
            continue;
        }
        if let Some((num, rest)) = step_start(l) {
            raw.push((n, num, rest.trim().to_string()));
        } else if let Some(last) = raw.last_mut() {
            last.2 = format!("{} {}", last.2, l.trim());
        } else {
            s.shape.push(format!("Steps, line {n}: \"{}\" isn't a step; write a step as \"1. builder: what it does\".", cut(l.trim())));
        }
    }
    for (k, (n, num, text)) in raw.into_iter().enumerate() {
        if let Some(num) = num
            && num != k + 1
            && num != 1
        {
            s.shape.push(format!("Steps, line {n}: this is step {} but it is numbered {num}; number them 1, 2, 3 in order.", k + 1));
        }
        let Some((agent, does)) = text.split_once(':') else {
            s.shape.push(format!("Steps, line {n}: start the step with its agent and a colon, as \"{}. builder: what it does\".", k + 1));
            s.workflow.push(Step { agent: String::new(), does: one_line(&text), back: None, line: n });
            continue;
        };
        let (does, back) = take_send_back(&one_line(does));
        let back = match back {
            None => None,
            Some(Ok(b)) => Some(b),
            Some(Err(why)) => {
                s.shape.push(format!("Steps, line {n}: {why}, as in \"May send the work back to step 2, at most once.\""));
                None
            }
        };
        let agent = agent.trim().trim_matches('`').trim_matches('*').trim().to_lowercase();
        s.workflow.push(Step { agent, does, back, line: n });
    }
}

/// A line quoted in a problem: at most 60 characters.
fn cut(s: &str) -> String {
    match s.char_indices().nth(60) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s.to_string(),
    }
}

// ---- writing ------------------------------------------------------------------------------------------------------

/// A squad as its brief: the front matter (id, name, the lines platoon doesn't know), What it does, the Steps (one
/// line each, the send-back as its sentence), the Checks and Asks you when there are any, then the sections platoon
/// doesn't know, as they were.
pub fn write(s: &Squad) -> String {
    let mut out = format!("---\nid: {}\nname: {}\n", s.id, s.name);
    for (k, v) in &s.meta {
        out.push_str(&format!("{k}: {v}\n"));
    }
    out.push_str("---\n");
    if let Some((_, text)) = s.other.iter().find(|(h, _)| h.is_empty()) {
        out.push_str(&format!("{text}\n\n"));
    }
    out.push_str(&format!("# {WHAT_IT_DOES}\n{}\n\n# {STEPS}\n", s.can_do.trim()));
    for (i, st) in s.workflow.iter().enumerate() {
        let mut line = format!("{}. {}: {}", i + 1, st.agent, st.does.trim());
        if let Some(b) = &st.back {
            line.push(' ');
            line.push_str(&send_back_sentence(b));
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    if !s.checks.is_empty() {
        out.push_str(&format!("\n# {CHECKS}\n"));
        for c in &s.checks {
            out.push_str(&format!("- {}\n", c.trim()));
        }
    }
    if !s.asks_you.trim().is_empty() {
        out.push_str(&format!("\n# {ASKS_YOU}\n{}\n", s.asks_you.trim()));
    }
    for (h, text) in s.other.iter().filter(|(h, _)| !h.is_empty()) {
        out.push_str(&format!("\n# {h}\n"));
        if !text.trim().is_empty() {
            out.push_str(&format!("{}\n", text.trim_end()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = "---
id: coding
name: Coding
---
# What it does
Makes a change to code in a git repository: plans it, builds it in a copy of its own, reviews it and runs
the tests, then takes it as far as the task says.

# Steps
1. planner: Reads the task and the code it touches and writes a short plan that restates the acceptance checks.
2. builder: Makes the change the plan describes in the task's copy and runs the tests.
3. reviewer: Reads the change against the task and the plan, and checks that each acceptance check could have
   failed before the change. May send the work back to step 2, at most twice.
4. verifier: Runs the tests and the build and reports plainly what passed and what failed. May send the work
   back to step 2, at most once.

# Checks
- the tests pass
- the reviewer read the change
- every acceptance box is ticked, with how
- nothing others see without your yes

# Asks you
Before anything others can see, and when it is stuck.
";

    #[test]
    fn the_example_reads_as_a_squad_and_round_trips() {
        let s = parse(EXAMPLE, "coding");
        assert!(s.shape.is_empty(), "{:?}", s.shape);
        assert_eq!((s.id.as_str(), s.name.as_str()), ("coding", "Coding"));
        assert!(s.can_do.starts_with("Makes a change to code") && s.can_do.ends_with("as far as the task says."), "{}", s.can_do);
        assert!(!s.can_do.contains('\n'), "the wrapped line is one paragraph");
        let steps: Vec<(&str, Option<Back>)> = s.workflow.iter().map(|st| (st.agent.as_str(), st.back)).collect();
        assert_eq!(
            steps,
            [
                ("planner", None),
                ("builder", None),
                ("reviewer", Some(Back { to: 1, times: 2 })),
                ("verifier", Some(Back { to: 1, times: 1 }))
            ]
        );
        assert_eq!(
            s.workflow[2].does,
            "Reads the change against the task and the plan, and checks that each acceptance check could have failed before the change."
        );
        assert_eq!(s.workflow[3].does, "Runs the tests and the build and reports plainly what passed and what failed.");
        assert_eq!(s.workflow.iter().map(|st| st.line).collect::<Vec<_>>(), [10, 11, 12, 14]);
        assert_eq!(s.checks.len(), 4);
        assert_eq!(s.at.check_lines, [18, 19, 20, 21]);
        assert_eq!(s.asks_you, "Before anything others can see, and when it is stuck.");
        // Written and read again: the same squad.
        let again = parse(&write(&s), "coding");
        assert!(again.shape.is_empty(), "{:?}", again.shape);
        assert!(again.same_as(&s), "{again:#?}\n{s:#?}");
        assert!(write(&s).contains("3. reviewer: Reads the change against the task and the plan, and checks that each acceptance check could have failed before the change. May send the work back to step 2, at most twice.\n"));
    }

    #[test]
    fn whitespace_and_case_are_forgiven() {
        let text = "\n\n  ---\nID:  mine \n Name : \"Mine\"\n---\n\n## WHAT IT DOES:\n  Does   things.\n\n#   steps\n\n1)  Builder :  builds it.\n   And more.\n\n2.   REVIEWER: reads it.  may send IT back to step 1,  At Most Three Times.\n# checks\n*  The Tests Pass.\n";
        let s = parse(text, "mine");
        assert!(s.shape.is_empty(), "{:?}", s.shape);
        assert_eq!((s.name.as_str(), s.can_do.as_str()), ("Mine", "Does things."));
        assert_eq!(s.workflow[0].agent, "builder");
        assert_eq!(s.workflow[0].does, "builds it. And more.");
        assert_eq!(
            (s.workflow[1].agent.as_str(), s.workflow[1].does.as_str(), s.workflow[1].back),
            ("reviewer", "reads it.", Some(Back { to: 0, times: 3 }))
        );
        assert_eq!(s.checks, ["The Tests Pass."]);
        assert_eq!(check_kind(&s.checks[0]), CheckKind::Tests);
    }

    #[test]
    fn a_send_back_reads_in_each_form() {
        let back = |text: &str| take_send_back(text).1;
        assert_eq!(back("x. May send the work back to step 2, at most once."), Some(Ok(Back { to: 1, times: 1 })));
        assert_eq!(back("x. May send the work back to step 2, at most twice."), Some(Ok(Back { to: 1, times: 2 })));
        assert_eq!(back("x. May send the work back to step 3, at most 4 times."), Some(Ok(Back { to: 2, times: 4 })));
        assert_eq!(back("x. may send it back to step 1, at most five times"), Some(Ok(Back { to: 0, times: 5 })));
        assert_eq!(back("x. Can send the work back to step 1 at most once."), Some(Ok(Back { to: 0, times: 1 })));
        assert_eq!(back("Nothing about it."), None);
        assert!(matches!(back("x. May send the work back to step 2."), Some(Err(_))), "how often is missing");
        assert!(matches!(back("x. May send the work back, at most once."), Some(Err(_))), "which step is missing");
        assert!(matches!(back("x. May send the work back to step 2, at most lots."), Some(Err(_))));
        let (rest, _) = take_send_back("Checks it. May send the work back to step 2, at most twice. Then reports.");
        assert_eq!(rest, "Checks it. Then reports.");
        for b in [Back { to: 0, times: 1 }, Back { to: 3, times: 2 }, Back { to: 1, times: 5 }] {
            assert_eq!(take_send_back(&format!("Does it. {}", send_back_sentence(&b))), ("Does it.".to_string(), Some(Ok(b))));
        }
        // A bad sentence is said where it is.
        let s = parse("---\nname: X\n---\n# What it does\nX.\n# Steps\n1. a: does. May send the work back to step 1.\n", "x");
        assert_eq!(s.shape, ["Steps, line 7: say how often: \"at most once\", as in \"May send the work back to step 2, at most once.\""]);
    }

    #[test]
    fn missing_and_unknown_sections_and_the_front_matter() {
        let s = parse("# Notes\nkeep me\n\n- and me\n", "x");
        assert_eq!(
            s.shape,
            [
                "The brief has no front matter: start it with a \"---\" line, \"name: ...\" and another \"---\".",
                "The brief has no \"# What it does\" section.",
                "The brief has no \"# Steps\" section."
            ]
        );
        assert_eq!(s.other, [("Notes".to_string(), "keep me\n\n- and me".to_string())]);
        let text = "---\nid: other\nname: Mine\ncolour: red\n---\nA line before.\n# What it does\nIt does.\n# Steps\n1. builder: builds.\nstray\n# Notes\n## Sub\nkept, sub heading too\n# What it does\nagain\n";
        let s = parse(text, "mine");
        assert_eq!(
            s.shape,
            [
                "Front matter, line 2: its id is \"other\", but the file is mine.md: make them the same.",
                "Line 15: a second \"# What it does\" section; a brief has one."
            ]
        );
        assert_eq!(s.workflow[0].does, "builds. stray", "a line under a step goes on with it");
        assert_eq!(s.meta, [("colour".to_string(), "red".to_string())]);
        assert_eq!(
            s.other,
            [(String::new(), "A line before.".to_string()), ("Notes".to_string(), "## Sub\nkept, sub heading too".to_string())]
        );
        let w = write(&s);
        assert!(
            w.contains("colour: red\n---\nA line before.\n\n# What it does\n") && w.contains("# Notes\n## Sub\nkept, sub heading too\n"),
            "{w}"
        );
        // Unclosed, and not key: value.
        let s = parse("---\nname: X\nnonsense\n", "x");
        assert!(s.shape.contains(&"Front matter, line 3: \"nonsense\" isn't a \"key: value\" line.".to_string()), "{:?}", s.shape);
        assert!(s.shape.contains(&"Front matter, line 1: it has no closing \"---\" line.".to_string()), "{:?}", s.shape);
        // A step without an agent, a stray line, numbers out of order.
        let s = parse("---\nname: X\n---\n# What it does\nX.\n# Steps\nfirst a line\n1. builds it all\n3. b: two\n", "x");
        assert_eq!(
            s.shape,
            [
                "Steps, line 7: \"first a line\" isn't a step; write a step as \"1. builder: what it does\".",
                "Steps, line 8: start the step with its agent and a colon, as \"1. builder: what it does\".",
                "Steps, line 9: this is step 2 but it is numbered 3; number them 1, 2, 3 in order."
            ]
        );
        // All numbered 1 is fine (Markdown numbers them).
        assert!(parse("---\nname: X\n---\n# What it does\nX.\n# Steps\n1. a: x\n1. b: y\n", "x").shape.is_empty());
    }

    #[test]
    fn each_check_is_matched_loosely_but_only_as_a_whole_line() {
        for l in ["the tests pass", "Tests pass.", "All the tests pass", "the project's tests pass", "THE TESTS PASSED"] {
            assert_eq!(check_kind(l), CheckKind::Tests, "{l}");
        }
        for l in
            ["the reviewer read the change", "Reviewer reads the changes.", "the reviewer has read the change", "an editor read the change"]
        {
            assert!(matches!(check_kind(l), CheckKind::Read(_)), "{l}");
        }
        assert_eq!(check_kind("The Report-Writer read the change"), CheckKind::Read("report-writer".into()));
        for l in [
            "every acceptance box is ticked, with how",
            "Every acceptance box is ticked",
            "all acceptance boxes are ticked with how",
            "each acceptance check is checked",
        ] {
            assert_eq!(check_kind(l), CheckKind::Acceptance, "{l}");
        }
        for l in ["nothing others see without your yes", "Nothing others can see without your yes.", "nothing goes out without your OK"] {
            assert_eq!(check_kind(l), CheckKind::Gate, "{l}");
        }
        for l in [
            "the tests pass on macOS",
            "the reviewer read the change and the tests pass",
            "every claim names its source",
            "a second agent read the change against the task",
            "nothing is deleted without your yes",
            "",
        ] {
            assert_eq!(check_kind(l), CheckKind::Prose, "{l}");
        }
    }
}
