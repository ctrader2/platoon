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
pub const BUILT_IN: [(&str, &str); 7] = [
    ("feature", include_str!("squads/feature.md")),
    ("bug-fix", include_str!("squads/bug-fix.md")),
    ("incident", include_str!("squads/incident.md")),
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

    /// Its name, or its file's name when it has none.
    pub fn name_or_id(&self) -> String {
        if self.name.is_empty() { self.id.clone() } else { self.name.clone() }
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

/// The file of the user's squad `id` (there or not).
pub fn path(config: &Path, id: &str) -> PathBuf {
    dir(config).join(format!("{id}.md"))
}

/// The built-in text of squad `id`, if it is a built-in.
pub fn built_in_text(id: &str) -> Option<&'static str> {
    BUILT_IN.iter().find(|(b, _)| *b == id).map(|(_, text)| *text)
}

/// A squad's file, ready for an editor: its path, its text before the edit, and whether platoon has just written it
/// (a built-in's text, so that leaving it unchanged takes the copy away again: `after_edit`). A file of the user's that
/// can't be read is not opened, and nothing is written over it.
pub fn prepare_edit(config: &Path, id: &str) -> std::io::Result<(PathBuf, String, bool)> {
    let p = path(config, id);
    match fs::read_to_string(&p) {
        Ok(text) => Ok((p, text, false)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let text = built_in_text(id).ok_or(e)?;
            crate::projects::write_atomic(&p, text)?;
            Ok((p, text.to_string(), true))
        }
        Err(e) => Err(e),
    }
}

/// After an editor has been on squad `id`'s file: whether it changed. A built-in's copy that platoon wrote and the
/// user left as it was is removed, so the built-in stays a built-in.
pub fn after_edit(config: &Path, id: &str, before: &str, copied: bool) -> bool {
    let p = path(config, id);
    let changed = fs::read_to_string(&p).map_or(true, |now| now != before);
    if !changed && copied {
        let _ = fs::remove_file(&p);
    }
    changed
}

/// The file name a new squad called `name` gets (without `.md`): its name in lower-case letters, digits and `-`, or
/// None when the name has no letter or digit to make one from.
pub fn id_for(name: &str) -> Option<String> {
    if !name.chars().any(char::is_alphanumeric) {
        return None;
    }
    let mut id = crate::usertheme::slug(name);
    if !id.starts_with(|c: char| c.is_ascii_lowercase()) {
        id = format!("squad-{id}");
    }
    id.truncate(40);
    Some(id.trim_end_matches('-').to_string())
}

/// The file a new squad starts as: the Feature squad's settings and one step to write over.
pub fn template(name: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: One line on what kind of work this squad is for.\nmodel: opus\nverdict_model: fable\nplan_first: true\n---\n\
         1. Write the first step here: what the session does, and how it checks that it worked.\n"
    )
}

/// Why a new squad couldn't be made.
#[derive(Debug)]
pub enum NewError {
    /// The name has no letter or digit.
    NoName,
    /// There is a squad with that file name already: its name.
    Taken(String),
    Io(std::io::Error),
}

/// Make a new squad of the user's called `name` from the template: its id. A name that would replace a built-in, or a
/// file that is there already, is refused (nothing is ever written over).
pub fn create(config: &Path, name: &str) -> Result<String, NewError> {
    let id = id_for(name).ok_or(NewError::NoName)?;
    if let Some(s) = all(config).into_iter().find(|s| s.id == id) {
        return Err(NewError::Taken(if s.name.is_empty() { id } else { s.name }));
    }
    let p = path(config, &id);
    if p.exists() {
        return Err(NewError::Taken(id));
    }
    crate::projects::write_atomic(&p, &template(name)).map_err(NewError::Io)?;
    Ok(id)
}

/// Delete the user's squad `id`: its file is kept as `<id>.md.bak` (one, the latest). A built-in it replaced comes back.
pub fn delete(config: &Path, id: &str) -> std::io::Result<()> {
    fs::rename(path(config, id), dir(config).join(format!("{id}.md.bak")))
}

/// A squad's name for a task's row: its name, else its id as written.
pub fn name_of(squads: &[Squad], id: &str) -> String {
    squads.iter().find(|s| s.id == id).map(|s| s.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| id.to_string())
}

// ---- the settings changed from the Squads tab ------------------------------------------------------------------------

/// The models a squad's settings are picked from on the Squads tab (its file may name any other).
pub const MODELS: [&str; 4] = ["opus", "sonnet", "haiku", "fable"];

/// A front matter setting the Squads tab changes without an editor. The description and the steps are the editor's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    Model,
    VerdictModel,
    PlanFirst,
}

impl Setting {
    /// In the order the squad's pane shows them.
    pub const ALL: [Setting; 3] = [Setting::Model, Setting::VerdictModel, Setting::PlanFirst];

    pub fn key(self) -> &'static str {
        match self {
            Setting::Model => "model",
            Setting::VerdictModel => "verdict_model",
            Setting::PlanFirst => "plan_first",
        }
    }

    /// Its label in the pane.
    pub fn label(self) -> &'static str {
        match self {
            Setting::Model => "Model",
            Setting::VerdictModel => "Verdict model",
            Setting::PlanFirst => "Plans first",
        }
    }

    /// What `s` has for it, in the pane's words.
    pub fn words(self, s: &Squad) -> String {
        match self {
            Setting::Model => s.model.clone(),
            Setting::VerdictModel if s.verdict_model.is_empty() => "the same as the model".into(),
            Setting::VerdictModel => s.verdict_model.clone(),
            Setting::PlanFirst => if s.plan_first { "yes, then waits for your yes" } else { "no" }.into(),
        }
    }

    /// The value after `s`'s, as its file writes it: the models in `MODELS`' order, round (a model not in it goes to the
    /// first); the verdict model the same, then "the same as the model" (None: no `verdict_model` line); plans first
    /// yes or no.
    pub fn next(self, s: &Squad) -> Option<String> {
        let after = |m: &str| MODELS.iter().position(|x| *x == m).map(|i| i + 1);
        match self {
            Setting::Model => Some(MODELS[after(&s.model).unwrap_or(0) % MODELS.len()].to_string()),
            Setting::VerdictModel if s.verdict_model.is_empty() => Some(MODELS[0].to_string()),
            Setting::VerdictModel => MODELS.get(after(&s.verdict_model).unwrap_or(0)).map(|m| m.to_string()),
            Setting::PlanFirst => Some((!s.plan_first).to_string()),
        }
    }
}

/// `text` (a squad's file) with its front matter's `key` line saying `value`: the line rewritten in place (the last
/// one, which is the one read, when there are several), or added before the closing `---`; with None, every `key` line
/// taken out. Every other byte stays as it was. None when the file has no front matter to change.
pub fn set_line(text: &str, key: &str, value: Option<&str>) -> Option<String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let bare = |l: &str| l.trim_start_matches('\u{feff}').trim_end().to_string();
    if lines.first().map(|l| bare(l)) != Some("---".to_string()) {
        return None;
    }
    let end = lines.iter().skip(1).position(|l| bare(l) == "---")? + 1;
    let is_key = |l: &str| {
        let t = l.trim();
        !t.starts_with('#') && t.split_once(':').is_some_and(|(k, _)| k.trim() == key)
    };
    let found: Vec<usize> = (1..end).filter(|&i| is_key(lines[i])).collect();
    let ending = |l: &str| {
        if l.ends_with("\r\n") {
            "\r\n"
        } else if l.ends_with('\n') {
            "\n"
        } else {
            ""
        }
    };
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    match (value, found.last()) {
        (Some(v), Some(&i)) => {
            let l = lines[i];
            let colon = l.find(':').unwrap_or(l.len());
            out[i] = format!("{} {v}{}", &l[..=colon], ending(l));
        }
        (Some(v), None) => {
            let nl = match ending(lines[end]) {
                "" => ending(lines[end - 1]),
                e => e,
            };
            out.insert(end, format!("{key}: {v}{}", if nl.is_empty() { "\n" } else { nl }));
        }
        (None, _) => {
            for &i in found.iter().rev() {
                out.remove(i);
            }
        }
    }
    Some(out.concat())
}

/// What changing a setting made of the squad's file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Became {
    /// The user's file, changed.
    Changed,
    /// A built-in's text was written to the user's folder with the change: the user's copy now replaces it.
    Yours,
    /// The change made the user's copy the built-in's text again: the copy is gone, and it is built in again.
    BuiltIn,
}

/// Why a setting couldn't be changed.
#[derive(Debug)]
pub enum ChangeError {
    /// The file has no front matter to change (it needs the editor).
    NoFrontMatter,
    Io(std::io::Error),
}

/// Change squad `id`'s `setting` to the next value (`Setting::next`) in its file: the user's file, or, for a built-in
/// not yet theirs, a copy of its text written to the user's folder first (as `e` does). Only that one line changes. A
/// file that can't be read is never written over; a copy left the same as the built-in is removed, so it stays built in.
pub fn change(config: &Path, id: &str, setting: Setting) -> Result<(Squad, Became), ChangeError> {
    let p = path(config, id);
    let (text, copied) = match fs::read_to_string(&p) {
        Ok(text) => (text, false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (built_in_text(id).ok_or(ChangeError::Io(e))?.to_string(), true),
        Err(e) => return Err(ChangeError::Io(e)),
    };
    let origin = if copied { Origin::BuiltIn } else { Origin::Own };
    let value = setting.next(&parse(id, &text, origin));
    let new = set_line(&text, setting.key(), value.as_deref()).ok_or(ChangeError::NoFrontMatter)?;
    let became = if built_in_text(id) == Some(new.as_str()) {
        if !copied {
            fs::remove_file(&p).map_err(ChangeError::Io)?;
        }
        Became::BuiltIn
    } else {
        crate::projects::write_atomic(&p, &new).map_err(ChangeError::Io)?;
        if copied { Became::Yours } else { Became::Changed }
    };
    let squad = get(config, id).ok_or_else(|| ChangeError::Io(std::io::Error::other("the squad is gone")))?;
    Ok((squad, became))
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
        assert_eq!(
            all.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["feature", "bug-fix", "incident", "refactor", "explain", "review", "docs"]
        );
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
        let incident = by("incident");
        assert_eq!((incident.model.as_str(), incident.verdict_model.as_str(), incident.plan_first), ("opus", "fable", false));
        // Stop the damage first, asking before anything others see; prove the cause; a postmortem at the end.
        for want in [
            "Stabilise first",
            "ask the user with `AskUserQuestion` before you do it",
            "MCP tools",
            "`Agent` tool",
            "timeline",
            "Prove the cause before you believe it",
            "Write a test that fails",
            "postmortem",
            "what would have caught it sooner",
        ] {
            assert!(incident.body.contains(want), "{want}");
        }
        // Every squad that changes code runs the repo's tests where it verifies, and names them at the end.
        for id in ["feature", "bug-fix", "incident", "refactor"] {
            let body = by(id).body.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(body.contains("find how this repo runs its tests (its CLAUDE.md, README, or build files) and run them all"), "{id}");
            assert!(body.contains("test command you ran and its result"), "{id}");
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
        assert_eq!(
            ids,
            ["feature", "bug-fix", "incident", "refactor", "explain", "review", "docs", "broken", "zebra"],
            "not a .md file: not a squad"
        );
        let review = all.iter().find(|s| s.id == "review").unwrap();
        assert_eq!((review.name.as_str(), review.origin, review.verdict_model.as_str()), ("Our review", Origin::Replaces, ""));
        assert!(review.usable());
        let broken = all.iter().find(|s| s.id == "broken").unwrap();
        assert!(!broken.usable() && broken.origin == Origin::Own);
        assert_eq!(name_of(&all, "zebra"), "Alpha");
        assert_eq!(name_of(&all, "gone"), "gone");
    }

    #[test]
    fn editing_a_built_in_writes_its_copy_and_leaving_it_unchanged_takes_the_copy_away() {
        let d = dir_for("edit-built-in");
        let (p, before, copied) = prepare_edit(&d, "docs").unwrap();
        assert!(copied && p == path(&d, "docs"));
        assert_eq!(fs::read_to_string(&p).unwrap(), before);
        assert_eq!(get(&d, "docs").unwrap().origin, Origin::Replaces, "while the editor has it");
        assert!(!after_edit(&d, "docs", &before, copied));
        assert!(!p.exists(), "unchanged: the copy goes");
        assert_eq!(get(&d, "docs").unwrap().origin, Origin::BuiltIn);

        let (p, before, copied) = prepare_edit(&d, "docs").unwrap();
        fs::write(&p, before.replace("model: sonnet", "model: haiku")).unwrap();
        assert!(after_edit(&d, "docs", &before, copied));
        let docs = get(&d, "docs").unwrap();
        assert_eq!((docs.origin, docs.model.as_str()), (Origin::Replaces, "haiku"), "changed: it is the user's now");

        // The user's own file again: nothing is written, and unchanged keeps it.
        let (_, again, copied) = prepare_edit(&d, "docs").unwrap();
        assert!(!copied && again.contains("model: haiku"));
        assert!(!after_edit(&d, "docs", &again, copied));
        assert!(path(&d, "docs").exists());
    }

    #[test]
    fn a_file_that_cannot_be_read_is_not_opened_and_not_written_over() {
        let d = dir_for("unreadable");
        fs::create_dir_all(path(&d, "odd")).unwrap(); // a folder where the file would be: read_to_string fails
        assert!(prepare_edit(&d, "odd").is_err());
        assert!(path(&d, "odd").is_dir());
        assert!(prepare_edit(&d, "nobody").is_err(), "neither a file nor a built-in");
        assert!(!path(&d, "nobody").exists());
    }

    #[test]
    fn a_new_squad_gets_a_file_name_from_its_name_and_never_writes_over_one() {
        assert_eq!(id_for("My Squad!").as_deref(), Some("my-squad"));
        assert_eq!(id_for("Écrire la doc").as_deref(), Some("ecrire-la-doc"));
        assert_eq!(id_for("2nd pass").as_deref(), Some("squad-2nd-pass"));
        assert_eq!(id_for("  ?! "), None);
        assert!(id_for(&"long name ".repeat(10)).is_some_and(|id| valid_id(&id) && !id.ends_with('-')));

        let d = dir_for("new");
        let id = create(&d, "Quick fix").unwrap();
        assert_eq!(id, "quick-fix");
        let s = get(&d, &id).unwrap();
        assert!(s.usable(), "the template has no problems: {:?}", s.problems);
        let feature = get(&d, DEFAULT).unwrap();
        assert_eq!(
            (s.name.as_str(), s.origin, s.model.as_str(), s.verdict_model.as_str(), s.plan_first),
            ("Quick fix", Origin::Own, feature.model.as_str(), feature.verdict_model.as_str(), feature.plan_first)
        );
        assert!(matches!(create(&d, "quick fix"), Err(NewError::Taken(n)) if n == "Quick fix"));
        assert!(matches!(create(&d, "Feature"), Err(NewError::Taken(n)) if n == "Feature"), "e changes a built-in, n doesn't");
        assert!(matches!(create(&d, "!!"), Err(NewError::NoName)));
    }

    #[test]
    fn a_setting_is_one_line_rewritten_and_every_other_byte_stays() {
        let text = "\u{feff}---\r\nname: Mine\r\n# a note of the user's: model: none\r\n  model:   haiku  \r\nfuture: kept\r\n---\r\n1. Go.\r\nmodel: in the steps\r\n";
        let changed = set_line(text, "model", Some("opus")).unwrap();
        assert_eq!(changed, text.replace("  model:   haiku  \r\n", "  model: opus\r\n"), "the line in place, its ending kept");
        // Missing: added before the closing line, in the file's line endings. None: every line of it taken out.
        let added = set_line(text, "verdict_model", Some("fable")).unwrap();
        assert_eq!(added, text.replace("future: kept\r\n---", "future: kept\r\nverdict_model: fable\r\n---"));
        let twice = "---\nname: X\nverdict_model: opus\nverdict_model: sonnet\n---\nGo.\n";
        assert_eq!(set_line(twice, "verdict_model", None).unwrap(), "---\nname: X\n---\nGo.\n");
        assert_eq!(
            set_line(twice, "verdict_model", Some("haiku")).unwrap(),
            "---\nname: X\nverdict_model: opus\nverdict_model: haiku\n---\nGo.\n",
            "the one read: the last"
        );
        assert_eq!(
            set_line("---\nname: X\nplan_first: true\n---", "plan_first", Some("false")).unwrap(),
            "---\nname: X\nplan_first: false\n---"
        );
        assert_eq!(set_line("---\nname: X\n---", "model", Some("opus")).unwrap(), "---\nname: X\nmodel: opus\n---");
        assert_eq!(set_line("no front matter", "model", Some("opus")), None);
        assert_eq!(set_line("---\nname: X\n", "model", Some("opus")), None, "no closing line");
        // What it reads back as.
        let s = parse("mine", &changed, Origin::Own);
        assert_eq!(s.model, "opus");
        assert_eq!(s.body, parse("mine", text, Origin::Own).body);
    }

    #[test]
    fn each_setting_goes_round_its_choices() {
        let mut s = parse("x", "---\nname: X\ndescription: D\nmodel: opus\n---\nGo.\n", Origin::Own);
        let mut seen = Vec::new();
        for _ in 0..5 {
            let next = Setting::Model.next(&s).unwrap();
            seen.push(next.clone());
            s.model = next;
        }
        assert_eq!(seen, ["sonnet", "haiku", "fable", "opus", "sonnet"]);
        s.model = "claude-some-full-name".into();
        assert_eq!(Setting::Model.next(&s).as_deref(), Some("opus"), "a model of the file's own goes to the first");
        let mut seen = Vec::new();
        for _ in 0..6 {
            let next = Setting::VerdictModel.next(&s);
            seen.push(next.clone().unwrap_or_else(|| "same".into()));
            s.verdict_model = next.unwrap_or_default();
        }
        assert_eq!(seen, ["opus", "sonnet", "haiku", "fable", "same", "opus"]);
        assert_eq!(Setting::VerdictModel.words(&Squad { verdict_model: String::new(), ..s.clone() }), "the same as the model");
        assert_eq!(Setting::PlanFirst.next(&s).as_deref(), Some("true"));
        s.plan_first = true;
        assert_eq!(Setting::PlanFirst.next(&s).as_deref(), Some("false"));
    }

    #[test]
    fn changing_a_built_ins_setting_makes_it_yours_and_changing_it_back_makes_it_built_in_again() {
        let d = dir_for("change");
        let (docs, became) = change(&d, "docs", Setting::Model).unwrap();
        assert_eq!((docs.model.as_str(), docs.origin, became), ("haiku", Origin::Replaces, Became::Yours));
        let text = fs::read_to_string(path(&d, "docs")).unwrap();
        assert_eq!(text, built_in_text("docs").unwrap().replace("\nmodel: sonnet\n", "\nmodel: haiku\n"), "only that line");
        // Round again: the copy is the built-in's text once more, so it goes.
        change(&d, "docs", Setting::Model).unwrap();
        change(&d, "docs", Setting::Model).unwrap();
        let (docs, became) = change(&d, "docs", Setting::Model).unwrap();
        assert_eq!((docs.model.as_str(), docs.origin, became), ("sonnet", Origin::BuiltIn, Became::BuiltIn));
        assert!(!path(&d, "docs").exists());
        // The verdict model "the same as the model" is no line at all.
        let (feature, _) = change(&d, "feature", Setting::VerdictModel).unwrap();
        assert_eq!(feature.verdict_model, "", "fable was the last choice");
        assert!(!fs::read_to_string(path(&d, "feature")).unwrap().contains("verdict_model"));
        let (feature, became) = change(&d, "feature", Setting::PlanFirst).unwrap();
        assert_eq!((feature.plan_first, became), (false, Became::Changed));
    }

    #[test]
    fn a_setting_never_writes_over_a_file_it_cant_read_or_change() {
        let d = dir_for("change-refused");
        fs::create_dir_all(path(&d, "odd")).unwrap();
        assert!(matches!(change(&d, "odd", Setting::Model), Err(ChangeError::Io(_))));
        assert!(path(&d, "odd").is_dir());
        assert!(matches!(change(&d, "nobody", Setting::Model), Err(ChangeError::Io(_))), "neither a file nor a built-in");
        assert!(!path(&d, "nobody").exists());
        fs::write(path(&d, "loose"), "just steps\n").unwrap();
        assert!(matches!(change(&d, "loose", Setting::Model), Err(ChangeError::NoFrontMatter)));
        assert_eq!(fs::read_to_string(path(&d, "loose")).unwrap(), "just steps\n");
        // An own squad changes in place, its unknown text kept.
        let own = "---\nname: Mine\ndescription: D\nmodel: opus\nmood: calm\n---\n1. Go.\n";
        fs::write(path(&d, "mine"), own).unwrap();
        let (mine, became) = change(&d, "mine", Setting::Model).unwrap();
        assert_eq!((mine.model.as_str(), mine.origin, became), ("sonnet", Origin::Own, Became::Changed));
        assert_eq!(fs::read_to_string(path(&d, "mine")).unwrap(), own.replace("model: opus", "model: sonnet"));
    }

    #[test]
    fn deleting_keeps_a_bak_and_a_replaced_built_in_comes_back() {
        let d = dir_for("delete");
        create(&d, "Mine").unwrap();
        fs::write(path(&d, "review"), "---\nname: Our review\ndescription: Ours\nmodel: sonnet\n---\nRead.\n").unwrap();
        delete(&d, "mine").unwrap();
        delete(&d, "review").unwrap();
        assert!(dir(&d).join("mine.md.bak").exists() && dir(&d).join("review.md.bak").exists());
        let all = all(&d);
        assert!(all.iter().all(|s| s.id != "mine"), "a .bak is not a squad");
        assert_eq!(all.iter().find(|s| s.id == "review").map(|s| (s.name.as_str(), s.origin)), Some(("Review", Origin::BuiltIn)));
        assert!(delete(&d, "mine").is_err(), "nothing left to delete");
    }
}
