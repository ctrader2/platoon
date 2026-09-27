//! What a plugin with instructions adds, to be read before it is installed. Skills and agents are instructions Claude
//! follows with the user's permissions (and a squad is a workflow its leader follows), so platoon shows each one first:
//! a line of what it is and, in plain words, what it may do (`Item::permissions`), and its whole text to read
//! (`Item::text`, Markdown: the plugin screen draws it with `mdview.rs`, `platoon plugin show` prints it).
//!
//! An update compares the catalog's version with the one installed, item by item (`review`): what is new, what
//! changed, what goes. One that changes any instructions is shown and asked again.

use crate::agentfile::Agent;
use crate::plugins::{self, Manifest};
use crate::skillplugin::Skill;
use crate::squadfile::{self, Squad};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Skill,
    Squad,
    Agent,
}

impl ItemKind {
    pub fn word(self) -> &'static str {
        match self {
            ItemKind::Skill => "Skill",
            ItemKind::Squad => "Squad",
            ItemKind::Agent => "Agent",
        }
    }
}

/// How an item of an update compares with the version installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// Being installed for the first time, or new in this version.
    New,
    /// Its instructions or permissions are different now.
    Changed,
    /// As it is installed.
    Same,
}

/// One thing a plugin adds that Claude follows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub kind: ItemKind,
    /// Its id in the plugin (a skill's name, a squad's or an agent's file name).
    pub id: String,
    pub name: String,
    /// One line: what it is for.
    pub about: String,
    /// What it may do, in plain words.
    pub permissions: Vec<String>,
    /// All of it, to read: Markdown.
    pub text: String,
    /// What it is exactly, to tell whether an update changes it.
    fingerprint: String,
    pub change: Change,
}

/// What a plugin adds that Claude follows, and (for an update) what the new version no longer has.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Review {
    pub items: Vec<Item>,
    /// "the agent change-reader": in the version installed, not in this one.
    pub gone: Vec<String>,
}

impl Review {
    /// Anything to read again: something new, changed or gone.
    pub fn changed(&self) -> bool {
        !self.gone.is_empty() || self.items.iter().any(|i| i.change != Change::Same)
    }
}

fn list_words(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// A skill's permissions in plain words: when Claude uses it, what it may do without asking, its other files.
pub fn skill_permissions(s: &Skill) -> Vec<String> {
    let f = &s.front;
    let mut out = Vec::new();
    out.push(match (f.model_may_use, f.user_may_type) {
        (true, true) => format!("Claude may use it by itself when it fits; you can also type /{}", s.name),
        (true, false) => "Claude uses it by itself when it fits (it can't be typed)".to_string(),
        (false, _) => format!("Only when you type /{}", s.name),
    });
    if f.allowed_tools.is_empty() {
        out.push("Asks your permission as usual for anything it runs or changes".into());
    } else {
        out.push(format!("Can use these without asking you while it runs: {}", f.allowed_tools.join(", ")));
    }
    for (k, v) in &f.others {
        out.push(format!("Also asks Claude Code for {k}: {v}"));
    }
    if !s.others.is_empty() {
        let names: Vec<String> = s.others.iter().map(|(r, _)| r.clone()).collect();
        out.push(format!("With {} more file{}: {}", names.len(), if names.len() == 1 { "" } else { "s" }, list_words(&names)));
    }
    out
}

fn skill_item(s: &Skill) -> Item {
    let (_, body) = crate::skillplugin::front_matter(&s.text).unwrap_or_default();
    let permissions = skill_permissions(s);
    let mut text = format!("# Skill: {}\n\n**When Claude uses it** (its description): {}\n\n", s.name, s.front.description);
    for p in &permissions {
        text.push_str(&format!("- {p}\n"));
    }
    text.push_str("\n---\n\n");
    text.push_str(body.trim());
    text.push('\n');
    for (rel, t) in &s.others {
        text.push_str(&format!("\n---\n\n## File: {rel}\n\n"));
        if rel.ends_with(".md") {
            text.push_str(t.trim());
            text.push('\n');
        } else {
            text.push_str(&format!("```\n{}\n```\n", t.trim_end()));
        }
    }
    Item {
        kind: ItemKind::Skill,
        id: s.name.clone(),
        name: s.name.clone(),
        about: s.front.description.clone(),
        permissions,
        text,
        fingerprint: crate::skillplugin::fingerprint(s),
        change: Change::New,
    }
}

/// Whose an agent a plugin's squad names is: "this plugin", or "built in".
fn whose(id: &str, own: &[Agent]) -> &'static str {
    if own.iter().any(|a| a.id == id) { "this plugin" } else { "built in" }
}

fn agent_item(a: &Agent) -> Item {
    let permissions = a.permission_lines();
    let mut text = format!("# Agent: {}\n\n", a.name);
    for p in &permissions {
        text.push_str(&format!("- {p}\n"));
    }
    text.push_str("\n## Its system prompt\n\n");
    text.push_str(a.guidance.trim());
    text.push_str("\n\nplatoon adds its own rules for every squad member after this (never publish, report plainly).\n");
    Item {
        kind: ItemKind::Agent,
        id: a.id.clone(),
        name: a.name.clone(),
        about: a.about(),
        permissions,
        text,
        fingerprint: crate::agentfile::to_value(a, None).to_string(),
        change: Change::New,
    }
}

fn squad_item(s: &Squad, own: &[Agent]) -> Item {
    let name_of = |id: &str| own.iter().find(|a| a.id == id).map_or_else(|| crate::agentfile::builtin(id).map_or(id.to_string(), |a| a.name), |a| a.name.clone());
    let agents: Vec<String> = s.agent_ids().iter().map(|id| format!("{} ({})", name_of(id), whose(id, own))).collect();
    let permissions = vec![
        format!("{} step{}, done by {}", s.workflow.len(), if s.workflow.len() == 1 { "" } else { "s" }, list_words(&agents)),
        "Its leader is the one every squad has (in the settings); it hands each step to an agent and never edits files itself".into(),
        "Works on a ticket in a git worktree of each of its folders, never pushing without your go-ahead".into(),
    ];
    let mut text = format!("# Squad: {}\n\n{}\n\n", s.name, s.can_do);
    for p in &permissions {
        text.push_str(&format!("- {p}\n"));
    }
    text.push_str("\n## Its workflow\n\n");
    for (i, st) in s.workflow.iter().enumerate() {
        text.push_str(&format!("{}. **{}** ({}): {}", i + 1, name_of(&st.agent), whose(&st.agent, own), st.does));
        if let Some(b) = st.back {
            text.push_str(&format!(" It may send the work back to step {}, at most {}.", b.to + 1, if b.times == 1 { "once".to_string() } else { format!("{} times", b.times) }));
        }
        text.push('\n');
    }
    Item {
        kind: ItemKind::Squad,
        id: s.id.clone(),
        name: s.name.clone(),
        about: s.can_do.clone(),
        permissions,
        text,
        fingerprint: squadfile::to_value(s, None).to_string(),
        change: Change::New,
    }
}

/// What the plugin in folder `dir` adds that Claude follows: its skills, squads and agents, as its check reads them.
/// Err: it doesn't pass its check (why).
pub fn items(dir: &Path) -> Result<(Manifest, Vec<Item>), String> {
    let r = plugins::check(dir);
    let m = match (&r.manifest, r.problems.is_empty()) {
        (Some(m), true) => m.clone(),
        (Some(m), false) => return Err(format!("{} can't be installed: {}", m.name, r.problems.join("; "))),
        (None, _) => return Err(format!("{} can't be installed: {}", dir.display(), r.problems.join("; "))),
    };
    let mut out: Vec<Item> = r.skills.iter().map(skill_item).collect();
    out.extend(r.squads.iter().map(|s| squad_item(s, &r.agents)));
    out.extend(r.agents.iter().map(agent_item));
    Ok((m, out))
}

/// What installing the plugin in `new` means, item by item: each one New, or (with the installed copy `old`) New,
/// Changed or Same, and what goes.
pub fn review(new: &Path, old: Option<&Path>) -> Result<(Manifest, Review), String> {
    let (m, mut items) = items(new)?;
    let before = old.and_then(|o| items_lenient(o)).unwrap_or_default();
    if old.is_some() {
        for i in &mut items {
            i.change = match before.iter().find(|b| b.kind == i.kind && b.id == i.id) {
                None => Change::New,
                Some(b) if b.fingerprint == i.fingerprint => Change::Same,
                Some(_) => Change::Changed,
            };
        }
    }
    let gone = before
        .iter()
        .filter(|b| !items.iter().any(|i| i.kind == b.kind && i.id == b.id))
        .map(|b| format!("the {} {}", b.kind.word().to_lowercase(), b.name))
        .collect();
    Ok((m, Review { items, gone }))
}

/// The items of an installed copy (which passed its check when it was installed).
fn items_lenient(dir: &Path) -> Option<Vec<Item>> {
    items(dir).ok().map(|(_, i)| i)
}

/// The summary `platoon plugin add` prints before asking: each item with its permissions, and where to read them in
/// full.
pub fn summary(m: &Manifest, r: &Review, update: Option<(&str, &str)>) -> String {
    let mut out = match update {
        Some((from, to)) if r.changed() => format!("{} {from} -> {to} changes instructions Claude follows with your permissions. Read them again:\n", m.name),
        Some((from, to)) => format!("{} {from} -> {to} adds instructions Claude follows with your permissions:\n", m.name),
        None => format!("{} {} adds instructions Claude follows with your permissions:\n", m.name, m.version),
    };
    for i in &r.items {
        let mark = match (update, i.change) {
            (Some(_), Change::New) => "  [new]",
            (Some(_), Change::Changed) => "  [changed]",
            _ => "",
        };
        out.push_str(&format!("  {} {}{mark}: {}\n", i.kind.word(), i.name, one_line(&i.about, 110)));
        for p in &i.permissions {
            out.push_str(&format!("      - {p}\n"));
        }
    }
    for g in &r.gone {
        out.push_str(&format!("  No longer in it: {g}\n"));
    }
    out.push_str(&format!("Read them in full: platoon plugin show {}\n", m.id));
    out
}

fn one_line(s: &str, max: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    match s.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::testing::tmp;
    use crate::squadplugin::testing::{plugin, READER};
    use std::fs;

    #[test]
    fn a_plugin_shows_each_skill_squad_and_agent_with_its_permissions_and_its_whole_text() {
        let base = tmp("review");
        let dir = plugin(&base, "changelog", "1.0.0");
        let (m, r) = review(&dir, None).unwrap();
        assert!(m.has_instructions());
        let seen: Vec<(ItemKind, &str, Change)> = r.items.iter().map(|i| (i.kind, i.name.as_str(), i.change)).collect();
        assert_eq!(
            seen,
            [
                (ItemKind::Skill, "changelog-entry", Change::New),
                (ItemKind::Squad, "changelog squad", Change::New),
                (ItemKind::Agent, "change-reader", Change::New),
                (ItemKind::Agent, "changelog-writer", Change::New)
            ]
        );
        let skill = &r.items[0];
        assert_eq!(skill.permissions, ["Claude may use it by itself when it fits; you can also type /changelog-entry", "Asks your permission as usual for anything it runs or changes"]);
        assert!(skill.text.contains("Do the thing carefully"), "the skill's own words: {}", skill.text);
        let squad = &r.items[1];
        assert_eq!(squad.permissions[0], "3 steps, done by change-reader (this plugin), changelog-writer (this plugin) and editor (built in)");
        assert!(squad.text.contains("3. **editor** (built in): Checks the entry against the changes. It may send the work back to step 2, at most once."), "{}", squad.text);
        let reader = &r.items[2];
        assert_eq!(reader.permissions, ["Runs on Sonnet", "Reads and searches files", "Can run commands on this computer"]);
        assert!(reader.text.contains(READER));
        let writer = &r.items[3];
        assert_eq!(writer.permissions, ["Runs on Sonnet", "Reads and searches files", "Can change files: edit and write them", "Starts with the skill changelog-entry (and can use other skills)"]);
        let text = summary(&m, &r, None);
        assert!(text.starts_with("changelog name 1.0.0 adds instructions Claude follows with your permissions:\n"), "{text}");
        assert!(text.contains("  Agent change-reader: Read the commits and the diff") && text.contains("      - Can run commands on this computer\n") && text.ends_with("Read them in full: platoon plugin show changelog\n"), "{text}");
    }

    #[test]
    fn permissions_are_worded_from_an_agents_switches_skills_and_servers() {
        let mut a = crate::agentfile::builtin("questioner").unwrap();
        assert_eq!(a.permission_lines(), ["Runs on Sonnet", "Only reads and searches files: it can't change files, run commands or use the web"]);
        a.web = true;
        a.mcp = vec!["sentry".into(), "linear".into()];
        a.skills = vec!["a".into(), "b".into(), "c".into()];
        a.model = "opus".into();
        assert_eq!(
            a.permission_lines(),
            [
                "Runs on Opus",
                "Reads and searches files",
                "Can use the web: search it and read pages",
                "Starts with the skills a, b and c (and can use other skills)",
                "Can use the MCP servers sentry and linear (programs set up in your Claude Code)"
            ]
        );
        let mut s = crate::skillplugin::Skill {
            name: "x-y".into(),
            folder: "skills/x-y".into(),
            front: crate::skillplugin::front_matter("---\nname: x-y\ndescription: d\nallowed-tools: Bash(git log:*)\ndisable-model-invocation: true\nmodel: opus\n---\n").unwrap().0,
            text: String::new(),
            others: vec![("ref.md".into(), "r".into())],
        };
        assert_eq!(skill_permissions(&s), ["Only when you type /x-y", "Can use these without asking you while it runs: Bash(git log:*)", "Also asks Claude Code for model: opus", "With 1 more file: ref.md"]);
        s.front.model_may_use = true;
        s.front.user_may_type = false;
        assert!(skill_permissions(&s)[0].contains("it can't be typed"));
    }

    #[test]
    fn an_update_says_what_is_new_changed_and_gone() {
        let base = tmp("review-update");
        let old = plugin(&base.join("old"), "changelog", "1.0.0");
        let new = plugin(&base.join("new"), "changelog", "1.1.0");
        let (_, r) = review(&new, Some(&old)).unwrap();
        assert!(!r.changed(), "the same instructions: {r:?}");
        // The reader's instructions change, the writer goes, a skill file changes.
        let p = new.join("agents/change-reader.json");
        fs::write(&p, fs::read_to_string(&p).unwrap().replace("change nothing", "change nothing, ever")).unwrap();
        fs::write(new.join("skills/changelog-entry/SKILL.md"), crate::skillplugin::testing::skill_md("changelog-entry", "Write it better.")).unwrap();
        let (m, r) = review(&new, Some(&old)).unwrap();
        let changes: Vec<(&str, Change)> = r.items.iter().map(|i| (i.name.as_str(), i.change)).collect();
        assert_eq!(changes, [("changelog-entry", Change::Changed), ("changelog squad", Change::Same), ("change-reader", Change::Changed), ("changelog-writer", Change::Same)]);
        assert!(r.changed());
        let text = summary(&m, &r, Some(("1.0.0", "1.1.0")));
        assert!(text.starts_with("changelog name 1.0.0 -> 1.1.0 changes instructions Claude follows with your permissions. Read them again:"), "{text}");
        assert!(text.contains("Skill changelog-entry  [changed]") && text.contains("Agent change-reader  [changed]"), "{text}");
    }
}
