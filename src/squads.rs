//! Squads: what a task's session follows. A squad is one Markdown file of steps (a playbook): front matter that says
//! what it is for and how its session starts, then the steps, written to the session. The built-in squads are compiled
//! in (`src/squads/*.md`); the user's own are `<config>/squads/<id>.md`, and a file with a built-in's id replaces that
//! built-in. A step may tell the session to follow another squad ("follow the Review squad on your plan"): every squad
//! is copied beside the task (`taskrun.rs`), so the step can name it by its file.
//!
//! ```text
//! ---
//! name: Feature
//! description: New or changed behaviour in code.
//! model: opus
//! verdict_model: fable
//! plan_first: true
//! ---
//! 1. ...
//! ```
//!
//! `model` is the session's model (a Claude Code alias such as `opus`, `sonnet`, `fable`, or a full model name),
//! `plan_first` starts it in plan mode, `description` (one line: what kind of work it is for) is what a squad is
//! suggested by, and `verdict_model` is the model of the check of a task's work before it is Done (`verdict.rs`). Each thing wrong with a file is
//! one line that says where (`problems`); a squad with problems can't be started.

use std::{
    fs,
    path::{Path, PathBuf},
};

/// The built-in squads, in the order they are listed: (id, the file's text).
pub const BUILT_IN: [(&str, &str); 6] = [
    ("feature", include_str!("squads/feature.md")),
    ("bug-fix", include_str!("squads/bug-fix.md")),
    ("refactor", include_str!("squads/refactor.md")),
    ("explain", include_str!("squads/explain.md")),
    ("review", include_str!("squads/review.md")),
    ("docs", include_str!("squads/docs.md")),
];

/// The squad a task starts with when nothing suggested another.
pub const DEFAULT: &str = "feature";

/// The keys a squad's front matter has.
const KEYS: [&str; 5] = ["name", "description", "model", "verdict_model", "plan_first"];

/// Where a squad comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    BuiltIn,
    /// The user's own file, in place of the built-in of the same id.
    Replaces,
    Own,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Squad {
    /// Its file's name without `.md`.
    pub id: String,
    pub name: String,
    pub description: String,
    pub model: String,
    pub verdict_model: String,
    pub plan_first: bool,
    /// The steps: everything under the front matter.
    pub body: String,
    pub origin: Origin,
    /// Everything wrong with its file, one line each. A squad with problems can't be started.
    pub problems: Vec<String>,
}

impl Squad {
    pub fn usable(&self) -> bool {
        self.problems.is_empty()
    }

    /// Its whole file, as `parse` reads it (the copy beside a task).
    pub fn text(&self) -> String {
        format!(
            "---\nname: {}\ndescription: {}\nmodel: {}\nverdict_model: {}\nplan_first: {}\n---\n{}",
            self.name, self.description, self.model, self.verdict_model, self.plan_first, self.body
        )
    }
}

/// An id a squad's file may have: lower-case letters, digits and `-`, starting with a letter.
pub fn valid_id(id: &str) -> bool {
    id.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && id.len() <= 40
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Read a squad's file. Whatever can be read is kept; each thing wrong is one line in `problems`, saying where.
pub fn parse(id: &str, text: &str, origin: Origin) -> Squad {
    let mut s = Squad {
        id: id.to_string(),
        name: String::new(),
        description: String::new(),
        model: String::new(),
        verdict_model: String::new(),
        plan_first: false,
        body: String::new(),
        origin,
        problems: Vec::new(),
    };
    if !valid_id(id) {
        s.problems.push(format!("file name: \"{id}.md\" should be lower-case letters, digits and -, like feature.md"));
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines: Vec<&str> = text.lines().collect();
    if lines.first().map(|l| l.trim_end()) != Some("---") {
        s.problems.push("line 1: a squad starts with its front matter: a line \"---\", then name:, description: and model:".into());
        s.body = text.to_string();
        return s;
    }
    let Some(end) = lines.iter().skip(1).position(|l| l.trim_end() == "---").map(|i| i + 1) else {
        s.problems.push("the front matter has no closing \"---\" line".into());
        return s;
    };
    for (i, line) in lines.iter().enumerate().take(end).skip(1) {
        let at = i + 1;
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            s.problems.push(format!("line {at}: \"{}\" isn't \"key: value\"", line.trim()));
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"').trim_matches('\'').trim().to_string();
        if matches!(key, "model" | "verdict_model") && value.contains(char::is_whitespace) {
            s.problems.push(format!("line {at}: {key} is one word, like opus or sonnet"));
        }
        match key {
            "name" => s.name = value,
            "description" => s.description = value,
            "model" => s.model = value,
            "verdict_model" => s.verdict_model = value,
            "plan_first" => match value.as_str() {
                "true" | "yes" => s.plan_first = true,
                "false" | "no" | "" => s.plan_first = false,
                other => s.problems.push(format!("line {at}: plan_first is true or false, not \"{other}\"")),
            },
            other => s.problems.push(format!("line {at}: unknown setting \"{other}\" (a squad has {})", KEYS.join(", "))),
        }
    }
    for (key, value) in [("name", &s.name), ("description", &s.description), ("model", &s.model)] {
        if value.is_empty() {
            s.problems.push(format!("front matter: no {key}"));
        }
    }
    s.body = lines[end + 1..].join("\n").trim().to_string() + "\n";
    if s.body.trim().is_empty() {
        s.problems.push(format!("line {}: no steps under the front matter", end + 2));
    }
    s
}

/// Where the user's own squads are.
pub fn dir(config: &Path) -> PathBuf {
    config.join("squads")
}

/// Every squad: the built-ins in their order (a user's file with a built-in's id in its place), then the user's own by
/// name. A file that can't be read is listed with that as its problem.
pub fn all(config: &Path) -> Vec<Squad> {
    let mut own: Vec<(String, PathBuf)> = fs::read_dir(dir(config))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let name = p.file_name()?.to_str()?;
            let id = name.strip_suffix(".md")?.to_string();
            (!id.starts_with('.') && p.is_file()).then_some((id, p))
        })
        .collect();
    own.sort();
    let read = |id: &str, p: &Path, origin: Origin| match fs::read_to_string(p) {
        Ok(text) => parse(id, &text, origin),
        Err(e) => {
            let mut s = parse(id, "", origin);
            s.problems = vec![format!("{} can't be read: {e}", p.display())];
            s
        }
    };
    let mut out: Vec<Squad> = BUILT_IN
        .iter()
        .map(|(id, text)| match own.iter().position(|(o, _)| o == id) {
            Some(i) => {
                let (id, p) = own.remove(i);
                read(&id, &p, Origin::Replaces)
            }
            None => parse(id, text, Origin::BuiltIn),
        })
        .collect();
    let mut mine: Vec<Squad> = own.iter().map(|(id, p)| read(id, p, Origin::Own)).collect();
    mine.sort_by_key(|s| (s.name.to_lowercase(), s.id.clone()));
    out.extend(mine);
    out
}

/// The squad with this id, as `all` has it.
pub fn get(config: &Path, id: &str) -> Option<Squad> {
    all(config).into_iter().find(|s| s.id == id)
}

/// A squad's name for a task's row: its name, else its id as written.
pub fn name_of(squads: &[Squad], id: &str) -> String {
    squads.iter().find(|s| s.id == id).map(|s| s.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir_for(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-squads-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn every_built_in_squad_parses_with_no_problems_and_says_what_it_is_for() {
        let d = dir_for("built-in");
        let all = all(&d);
        assert_eq!(all.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["feature", "bug-fix", "refactor", "explain", "review", "docs"]);
        for s in &all {
            assert!(s.problems.is_empty(), "{}: {:?}", s.id, s.problems);
            assert_eq!(s.origin, Origin::BuiltIn);
            assert!(!s.description.contains('\n') && s.description.len() > 20, "{}: one line that says what it is for", s.id);
            let size = BUILT_IN.iter().find(|(id, _)| *id == s.id).unwrap().1.len();
            assert!((800..=4200).contains(&size), "{}: {size} bytes", s.id);
            for word in ["pstack", "poteto", "Cursor", "Lauren", "Denyer", "poteto-agent", "bun ", "/setup-"] {
                assert!(!s.text().contains(word), "{} names {word}", s.id);
            }
        }
        let by = |id: &str| all.iter().find(|s| s.id == id).unwrap().clone();
        for id in ["feature", "bug-fix", "refactor"] {
            let s = by(id);
            assert_eq!((s.model.as_str(), s.verdict_model.as_str(), s.plan_first), ("opus", "fable", true), "{id}");
        }
        for id in ["explain", "review"] {
            let s = by(id);
            assert_eq!((s.model.as_str(), s.verdict_model.as_str(), s.plan_first), ("opus", "sonnet", false), "{id}");
        }
        let docs = by("docs");
        assert_eq!((docs.model.as_str(), docs.verdict_model.as_str(), docs.plan_first), ("sonnet", "opus", false));
        assert_eq!(get(&d, DEFAULT).map(|s| s.name), Some("Feature".to_string()));
    }

    #[test]
    fn the_front_matter_and_the_steps_are_read_and_written_back_the_same() {
        let s = parse(
            "mine",
            "---\nname: My squad\ndescription: \"Small fixes in the docs\"\nmodel: sonnet\nverdict_model: opus\nplan_first: yes\n---\n\n1. Read.\n2. Fix.\n",
            Origin::Own,
        );
        assert!(s.problems.is_empty(), "{:?}", s.problems);
        assert_eq!((s.name.as_str(), s.description.as_str(), s.model.as_str()), ("My squad", "Small fixes in the docs", "sonnet"));
        assert_eq!((s.verdict_model.as_str(), s.plan_first, s.body.as_str()), ("opus", true, "1. Read.\n2. Fix.\n"));
        let again = parse("mine", &s.text(), Origin::Own);
        assert_eq!(again, s, "its copy beside a task reads the same");
    }

    #[test]
    fn a_broken_file_says_what_is_wrong_one_line_each_with_where() {
        let s = parse("Bad Name", "---\nname: X\nmodle: opus\nplan_first: maybe\nthis line\n---\n", Origin::Own);
        let p = s.problems.join("\n");
        for want in [
            "file name: \"Bad Name.md\"",
            "line 3: unknown setting \"modle\"",
            "line 4: plan_first is true or false, not \"maybe\"",
            "line 5: \"this line\" isn't \"key: value\"",
            "front matter: no description",
            "front matter: no model",
            "line 7: no steps under the front matter",
        ] {
            assert!(p.contains(want), "{want}\n{p}");
        }
        assert!(s.problems.iter().all(|l| !l.contains('\n')), "one line each");
        assert!(!s.usable());
        assert!(parse("x", "no front matter here", Origin::Own).problems[0].starts_with("line 1: a squad starts with its front matter"));
        assert_eq!(parse("x", "---\nname: X\n", Origin::Own).problems, ["the front matter has no closing \"---\" line"]);
    }

    #[test]
    fn a_users_file_replaces_the_built_in_of_its_id_and_their_own_come_after_by_name() {
        let d = dir_for("own");
        fs::create_dir_all(dir(&d)).unwrap();
        fs::write(
            dir(&d).join("review.md"),
            "---\nname: Our review\ndescription: Reviews the way we do it\nmodel: sonnet\n---\nRead it.\n",
        )
        .unwrap();
        fs::write(dir(&d).join("zebra.md"), "---\nname: Alpha\ndescription: First by name\nmodel: haiku\n---\nGo.\n").unwrap();
        fs::write(dir(&d).join("broken.md"), "nothing").unwrap();
        fs::write(dir(&d).join("coding.json"), "{}").unwrap();
        let all = all(&d);
        let ids: Vec<&str> = all.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["feature", "bug-fix", "refactor", "explain", "review", "docs", "broken", "zebra"], "not a .md file: not a squad");
        let review = all.iter().find(|s| s.id == "review").unwrap();
        assert_eq!((review.name.as_str(), review.origin, review.verdict_model.as_str()), ("Our review", Origin::Replaces, ""));
        assert!(review.usable());
        let broken = all.iter().find(|s| s.id == "broken").unwrap();
        assert!(!broken.usable() && broken.origin == Origin::Own);
        assert_eq!(name_of(&all, "zebra"), "Alpha");
        assert_eq!(name_of(&all, "gone"), "gone");
    }
}
