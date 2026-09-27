//! Squads and agents as kinds of plugin: definitions in the same JSON formats as the user's own
//! `<config>/squads/<id>.json` and `<config>/agents/<id>.json` (`squadfile.rs`, `agentfile.rs`), each file's name its id.
//!
//! ```json
//! "squads": ["squads/changelog.json"],
//! "agents": ["agents/change-reader.json", "agents/changelog-writer.json"]
//! ```
//!
//! A plugin's squad may use platoon's built-in agents and the plugin's own agents, nothing else. A plugin's agents may
//! also be used by the user's own squads. Nothing of a plugin is ever written to the user's squads or agents folders:
//! they are read from the installed plugin's folder, shown on the Squads tab beside the built-in ones and the user's
//! own, labelled with the plugin's name and read only there (copying one makes one of the user's own).
//!
//! **Ids are namespaced** so they never clash with a built-in, the user's own or another plugin's:
//! `plugin:<plugin id>/<id>` (`item_id`). A plugin squad's steps name its own agents by those ids and built-in agents
//! by theirs. In a squad's Claude Code session an agent's name is made of the squad's id and the agent's
//! (`squads::agent_name`): a plugin's squad and its own agents go by their short ids there (`changelog-leader`,
//! `changelog-change-reader`); a plugin's agent in a squad of another's is `<plugin>--<id>` (`runtime`), which no
//! built-in or user's id can be (they never have two dashes together).

use crate::agentfile::{self, Agent};
use crate::plugins::{valid_id, Store};
use crate::squadfile::{self, slug_problem, Squad};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// The kinds' keys in `plugin.json`.
pub const SQUADS: &str = "squads";
pub const AGENTS: &str = "agents";
/// One squad's or agent's file at most.
pub const MAX_DEF_BYTES: u64 = 64 * 1024;
/// What every plugin squad's and agent's id starts with.
pub const PREFIX: &str = "plugin:";

/// A plugin's squad's or agent's id: `plugin:<plugin>/<id>`.
pub fn item_id(plugin: &str, local: &str) -> String {
    format!("{PREFIX}{plugin}/{local}")
}

/// The plugin and the short id of a plugin squad's or agent's id, when it is one.
pub fn split(id: &str) -> Option<(&str, &str)> {
    let (plugin, local) = id.strip_prefix(PREFIX)?.split_once('/')?;
    (valid_id(plugin) && slug_problem(local).is_none()).then_some((plugin, local))
}

/// Is this a plugin's squad or agent (whether or not that plugin is installed)?
pub fn is_plugin(id: &str) -> bool {
    id.starts_with(PREFIX)
}

/// Why this can't be a plugin item's id, when it starts like one but isn't one.
pub fn id_problem(id: &str) -> Option<&'static str> {
    (is_plugin(id) && split(id).is_none()).then_some("it isn't plugin:<plugin>/<id>")
}

/// How an id is written in a Claude Code session's agent names: a plugin's `<plugin>--<id>`, anything else as it is.
pub fn runtime(id: &str) -> String {
    match split(id) {
        Some((p, l)) => format!("{p}--{l}"),
        None => id.to_string(),
    }
}

/// The part of an agent's name in squad `squad`'s session that is the agent's: a plugin squad's own agent by its short
/// id, another plugin's as `runtime`, anything else as it is.
pub fn member_part(squad: &str, agent: &str) -> String {
    match (split(squad), split(agent)) {
        (Some((sp, _)), Some((ap, local))) if sp == ap => local.to_string(),
        _ => runtime(agent),
    }
}

/// The part of the names in squad `squad`'s session that is the squad's: a plugin squad's short id.
pub fn squad_part(squad: &str) -> String {
    match split(squad) {
        Some((_, local)) => local.to_string(),
        None => squad.to_string(),
    }
}

/// A file a plugin names for a squad or an agent: its id (the file's name) and what it holds.
fn read_def(rel: &str, files: &[(String, PathBuf, u64)], safe: fn(&str) -> bool) -> Result<(String, Value), String> {
    if !safe(rel) {
        return Err(format!("\"{rel}\" isn't a file name inside the plugin (no leading /, no .., no hidden parts)"));
    }
    let Some(stem) = rel.rsplit('/').next().and_then(|n| n.strip_suffix(".json")) else {
        return Err(format!("{rel}: a squad's or an agent's file is <id>.json"));
    };
    if let Some(why) = slug_problem(stem) {
        return Err(format!("{rel}: \"{stem}\" can't be an id: {why}"));
    }
    let Some((_, path, len)) = files.iter().find(|(r, ..)| r == rel) else {
        return Err(format!("{rel} is listed in plugin.json but isn't in the folder"));
    };
    if *len > MAX_DEF_BYTES {
        return Err(format!("{rel} is bigger than {} KiB", MAX_DEF_BYTES / 1024));
    }
    let text = fs::read_to_string(path).map_err(|_| format!("{rel} isn't text"))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("{rel} isn't valid JSON ({e})"))?;
    if !v.is_object() {
        return Err(format!("{rel} isn't a JSON object"));
    }
    Ok((stem.to_string(), v))
}

fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// What `plugins::check` adds for a plugin's squads and agents: the problems, each in plain words, and the squads and
/// agents (by their short ids, as in the files) that passed. Every rule a user's own squad and agent follow
/// (`squadfile::problems`, `agentfile::problems`), and: no id or name of a built-in, a squad's steps name only built-in
/// agents and the plugin's own.
pub fn check(squads: &[String], agents: &[String], files: &[(String, PathBuf, u64)], safe: fn(&str) -> bool) -> (Vec<String>, Vec<Squad>, Vec<Agent>) {
    let mut problems = Vec::new();
    let mut own: Vec<Agent> = Vec::new();
    for rel in agents {
        let (id, v) = match read_def(rel, files, safe) {
            Ok(x) => x,
            Err(e) => {
                problems.push(e);
                continue;
            }
        };
        let a = match agentfile::from_value(&v, &id) {
            Ok(a) => a,
            Err(e) => {
                problems.push(format!("{rel}: {e}"));
                continue;
            }
        };
        let before = problems.len();
        problems.extend(agentfile::problems(&a).into_iter().map(|p| format!("{rel}: {p}")));
        if agentfile::is_builtin(&id) {
            problems.push(format!("{rel}: \"{id}\" is one of platoon's built-in agents; give it an id of its own"));
        } else if let Some(b) = agentfile::builtins().into_iter().find(|b| same_name(&b.name, &a.name)) {
            problems.push(format!("{rel}: \"{}\" is the name of platoon's built-in agent {}; give it a name of its own", a.name, b.id));
        }
        if own.iter().any(|o| o.id == id) {
            problems.push(format!("{rel}: the plugin has two agents with the id {id}"));
        }
        if problems.len() == before {
            own.push(a);
        }
    }
    let mut out: Vec<Squad> = Vec::new();
    let library: Vec<Agent> = agentfile::builtins().into_iter().chain(own.iter().cloned()).collect();
    for rel in squads {
        let (id, v) = match read_def(rel, files, safe) {
            Ok(x) => x,
            Err(e) => {
                problems.push(e);
                continue;
            }
        };
        let s = match squadfile::from_value(&v, &id) {
            Ok(s) => s,
            Err(e) => {
                problems.push(format!("{rel}: {e}"));
                continue;
            }
        };
        let before = problems.len();
        problems.extend(squadfile::problems(&s, &library).into_iter().map(|p| format!("{rel}: {p}")));
        if squadfile::is_builtin(&id) {
            problems.push(format!("{rel}: \"{id}\" is one of platoon's built-in squads; give it an id of its own"));
        } else if let Some(b) = squadfile::builtins().into_iter().find(|b| same_name(&b.name, &s.name)) {
            problems.push(format!("{rel}: \"{}\" is the name of platoon's built-in squad; give it a name of its own", b.name));
        }
        if out.iter().any(|o| o.id == id) {
            problems.push(format!("{rel}: the plugin has two squads with the id {id}"));
        }
        if problems.len() == before {
            out.push(s);
        }
    }
    // Agents shared by name only would be confusing on the Squads tab: two of one plugin can't have the same name.
    for (i, a) in own.iter().enumerate() {
        if own[..i].iter().any(|o| same_name(&o.name, &a.name)) {
            problems.push(format!("two of its agents are called {}", a.name));
        }
    }
    (problems, out, own)
}

/// A plugin's squads and agents as the rest of platoon sees them: ids namespaced (`item_id`), a squad's steps naming
/// the plugin's own agents by those ids and built-in agents by theirs. A step that names anything else names an agent
/// of this plugin that isn't there (never one of the user's that happens to share its id).
pub fn namespace(plugin: &str, squads: &[Squad], agents: &[Agent]) -> (Vec<Squad>, Vec<Agent>) {
    let own: Vec<&str> = agents.iter().map(|a| a.id.as_str()).collect();
    let agents: Vec<Agent> = agents.iter().map(|a| Agent { id: item_id(plugin, &a.id), ..a.clone() }).collect();
    let squads = squads
        .iter()
        .map(|s| {
            let mut s = s.clone();
            s.id = item_id(plugin, &s.id);
            for st in &mut s.workflow {
                if own.contains(&st.agent.as_str()) || !agentfile::is_builtin(&st.agent) {
                    st.agent = item_id(plugin, &st.agent);
                }
            }
            s
        })
        .collect();
    (squads, agents)
}

/// One installed plugin's squads and agents, namespaced, with the plugin's id and name.
#[derive(Clone, Debug)]
pub struct FromPlugin {
    pub plugin: String,
    pub name: String,
    pub squads: Vec<Squad>,
    pub agents: Vec<Agent>,
}

/// Read a plugin folder's squads and agents (checked when it was installed; a file that doesn't read now is left out).
pub fn read(dir: &Path, m: &crate::plugins::Manifest) -> (Vec<Squad>, Vec<Agent>) {
    let read_one = |rel: &String| -> Option<(String, Value)> {
        let path = dir.join(rel);
        let meta = fs::symlink_metadata(&path).ok().filter(|m| m.is_file() && m.len() <= MAX_DEF_BYTES)?;
        let _ = meta;
        let stem = rel.rsplit('/').next()?.strip_suffix(".json")?.to_string();
        slug_problem(&stem).is_none().then_some(())?;
        let v: Value = serde_json::from_str(&fs::read_to_string(&path).ok()?).ok()?;
        Some((stem, v))
    };
    let agents: Vec<Agent> = m.files(AGENTS).iter().filter_map(read_one).filter_map(|(id, v)| agentfile::from_value(&v, &id).ok()).collect();
    let squads: Vec<Squad> = m.files(SQUADS).iter().filter_map(read_one).filter_map(|(id, v)| squadfile::from_value(&v, &id).ok()).collect();
    (squads, agents)
}

/// Every installed plugin's squads and agents, by plugin name.
pub fn installed(store: &Store) -> Vec<FromPlugin> {
    store
        .installed()
        .into_iter()
        .filter(|p| !p.manifest.files(SQUADS).is_empty() || !p.manifest.files(AGENTS).is_empty())
        .map(|p| {
            let (s, a) = read(&store.plugin_dir(p.id()), &p.manifest);
            let (squads, agents) = namespace(p.id(), &s, &a);
            FromPlugin { plugin: p.id().to_string(), name: p.manifest.name.clone(), squads, agents }
        })
        .collect()
}

/// The user's own squads (and changed built-ins) whose steps use an agent of plugin `plugin`: each one's name and the
/// agents' names. What removing the plugin would leave with a missing agent.
pub fn users_of_plugin(config: &Path, plugin: &str) -> Vec<(String, Vec<String>)> {
    let listed = squadfile::load_all(config);
    let agents = agentfile::all(config);
    listed
        .iter()
        .filter(|l| l.kind != squadfile::Kind::Plugin)
        .filter_map(|l| l.squad.as_ref())
        .filter_map(|s| {
            let names: Vec<String> = s
                .agent_ids()
                .into_iter()
                .filter(|id| split(id).is_some_and(|(p, _)| p == plugin))
                .map(|id| agents.iter().find(|a| a.id == id).map_or_else(|| id.clone(), |a| a.name.clone()))
                .collect();
            (!names.is_empty()).then(|| (s.name.clone(), names))
        })
        .collect()
}

/// "Your squads A and B use its agents: their steps will show the agent as missing until you pick another." (None
/// when no squad of the user's uses them.)
pub fn removal_warning(users: &[(String, Vec<String>)]) -> Option<String> {
    let names: Vec<String> = users.iter().map(|(n, _)| n.clone()).collect();
    let list = match names.as_slice() {
        [] => return None,
        [one] => format!("Your squad {one} uses"),
        [rest @ .., last] => format!("Your squads {} and {last} use", rest.join(", ")),
    };
    Some(format!("{list} its agents: those steps will show the agent as missing until you pick another one on the Squads tab."))
}

#[cfg(test)]
pub mod testing {
    //! A plugin with a skill, a squad and two agents of its own, for tests here and in the app's.
    use super::*;
    use serde_json::json;

    pub const READER: &str = "You are the change-reader. Read the commits and the diff since the last release and list what a user of the project would notice. Report the list; change nothing.";
    pub const WRITER: &str = "You are the changelog-writer. Write the new entry at the top of CHANGELOG.md in the file's own style, from the reader's list. Report what you wrote.";

    /// `<base>/<id>`: a plugin with the skill `<id>-entry`, the squad `<id>` (reader, writer, then the built-in editor
    /// checking, which may send it back once) and its two agents.
    pub fn plugin(base: &Path, id: &str, version: &str) -> PathBuf {
        let dir = base.join(id);
        crate::skillplugin::testing::write_skill(&dir, &format!("{id}-entry"), &crate::skillplugin::testing::skill_md(&format!("{id}-entry"), "Write a changelog entry for the work on this branch."), &[]);
        fs::create_dir_all(dir.join("agents")).unwrap();
        fs::create_dir_all(dir.join("squads")).unwrap();
        let reader = json!({"name": "change-reader", "model": "sonnet", "can_change_files": false, "can_run_commands": true, "can_use_web": false, "guidance": READER});
        let writer = json!({"name": "changelog-writer", "model": "sonnet", "can_change_files": true, "can_run_commands": false, "can_use_web": false, "skills": [format!("{id}-entry")], "guidance": WRITER});
        fs::write(dir.join("agents/change-reader.json"), serde_json::to_string_pretty(&reader).unwrap()).unwrap();
        fs::write(dir.join("agents/changelog-writer.json"), serde_json::to_string_pretty(&writer).unwrap()).unwrap();
        let squad = json!({"name": format!("{id} squad"), "can_do": "Writes the changelog entry for a release.", "workflow": [
            {"agent": "change-reader", "does": "Lists what changed since the last release."},
            {"agent": "changelog-writer", "does": "Writes the entry at the top of CHANGELOG.md."},
            {"agent": "editor", "does": "Checks the entry against the changes.", "back_to": 2, "times": 1}
        ]});
        fs::write(dir.join(format!("squads/{id}.json")), serde_json::to_string_pretty(&squad).unwrap()).unwrap();
        let m = json!({"id": id, "name": format!("{id} name"), "description": format!("What {id} is."), "version": version, "author": "Someone", "license": "MIT", "platoon": "0.1.0",
            "skills": [format!("skills/{id}-entry")], "squads": [format!("squads/{id}.json")], "agents": ["agents/change-reader.json", "agents/changelog-writer.json"]});
        fs::write(dir.join(crate::plugins::MANIFEST), serde_json::to_string_pretty(&m).unwrap()).unwrap();
        dir
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use crate::plugins::{check as check_plugin, testing::tmp};

    #[test]
    fn ids_are_namespaced_and_session_names_never_clash() {
        assert_eq!(item_id("changelog", "change-reader"), "plugin:changelog/change-reader");
        assert_eq!(split("plugin:changelog/change-reader"), Some(("changelog", "change-reader")));
        for bad in ["plugin:", "plugin:x", "plugin:X/y", "plugin:x/Y", "plugin:x/y/z", "coding"] {
            assert_eq!(split(bad), None, "{bad}");
        }
        assert!(id_problem("plugin:x/Y").is_some() && id_problem("coding").is_none() && id_problem("plugin:x/y").is_none());
        assert_eq!(runtime("plugin:changelog/change-reader"), "changelog--change-reader");
        assert_eq!(runtime("planner"), "planner");
        // In the plugin's own squad its agents go by their short ids; in another squad by plugin and id.
        assert_eq!(member_part("plugin:changelog/changelog", "plugin:changelog/change-reader"), "change-reader");
        assert_eq!(member_part("plugin:changelog/changelog", "editor"), "editor");
        assert_eq!(member_part("careful", "plugin:changelog/change-reader"), "changelog--change-reader");
        assert_eq!(member_part("plugin:other/x", "plugin:changelog/change-reader"), "changelog--change-reader");
        assert_eq!(squad_part("plugin:changelog/changelog"), "changelog");
        assert!(slug_problem("changelog--change-reader").is_some(), "no user's or built-in id can look like a plugin agent's session name");
    }

    #[test]
    fn a_good_plugin_with_a_squad_and_agents_passes_and_its_squad_uses_built_in_and_own_agents() {
        let base = tmp("sq-good");
        let dir = plugin(&base, "changelog", "1.0.0");
        let r = check_plugin(&dir);
        assert!(r.ok(), "{:?}", r.problems);
        assert_eq!(r.squads.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["changelog"]);
        assert_eq!(r.agents.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["change-reader", "changelog-writer"]);
        let (squads, agents) = namespace("changelog", &r.squads, &r.agents);
        assert_eq!(squads[0].id, "plugin:changelog/changelog");
        assert_eq!(squads[0].agent_ids(), ["plugin:changelog/change-reader", "plugin:changelog/changelog-writer", "editor"]);
        let library: Vec<Agent> = agentfile::builtins().into_iter().chain(agents).collect();
        assert!(squadfile::problems(&squads[0], &library).is_empty(), "{:?}", squadfile::problems(&squads[0], &library));
        let crew = squadfile::Crew::of(&squads[0], &library);
        assert_eq!(crew.agents.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["change-reader", "changelog-writer", "editor"]);
    }

    #[test]
    fn installed_plugin_squads_and_agents_are_listed_labelled_read_only_and_start_with_their_own_names() {
        use crate::squadfile::Kind;
        let base = tmp("sq-installed");
        let config = base.join("config");
        let src = plugin(&base.join("src"), "changelog", "1.0.0");
        let store = crate::plugins::store_for(&config);
        assert!(store.dir.starts_with(&base) && store.claude.as_ref().is_some_and(|c| c.starts_with(&base)), "a test's store is beside its config");
        let stamp = squadfile::stamp(&config);
        store.install(&src, crate::plugins::Origin::Folder { path: src.to_string_lossy().into_owned() }).unwrap();
        assert_ne!(squadfile::stamp(&config), stamp, "installing a plugin changes what the lists show");
        // Listed after the built-ins and the user's own, labelled with the plugin's name.
        let listed = squadfile::load_all(&config);
        let last = listed.last().unwrap();
        assert_eq!((last.id.as_str(), last.kind, last.label()), ("plugin:changelog/changelog", Kind::Plugin, "From changelog name".to_string()));
        let agents = agentfile::load_all(&config);
        let theirs: Vec<(&str, String)> = agents.iter().filter(|l| l.kind == Kind::Plugin).map(|l| (l.id.as_str(), l.label())).collect();
        assert_eq!(theirs, [("plugin:changelog/change-reader", "From changelog name".to_string()), ("plugin:changelog/changelog-writer", "From changelog name".to_string())]);
        // Found by id; read only.
        let squad = squadfile::get(&config, "plugin:changelog/changelog").unwrap();
        let reader = agentfile::get(&config, "plugin:changelog/change-reader").unwrap();
        assert!(squadfile::save(&config, &squad).unwrap_err().to_string().contains("comes with a plugin"));
        assert!(agentfile::save(&config, &reader).unwrap_err().to_string().contains("comes with a plugin"));
        assert!(squadfile::remove(&config, &squad.id).is_err() && agentfile::remove(&config, &reader.id).is_err());
        assert!(agentfile::delete(&config, &reader.id).unwrap_err().contains("comes with a plugin"));
        assert!(!squadfile::dir(&config).exists() && !agentfile::dir(&config).exists(), "nothing of a plugin is written to the user's folders");
        // It starts: the built-in editor and its own agents, named by their short ids in its session.
        let crew = squadfile::Crew::load(&config, &squad);
        assert!(squadfile::problems(&squad, &agentfile::all(&config)).is_empty());
        let launch = crate::squads::Launch::of(&crew, &crate::squads::LeaderSetup::builtin(), Default::default(), crate::squads::Place::WorkFolder);
        let v: Value = serde_json::from_str(&launch.agents).unwrap();
        let names: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(names, ["changelog-leader", "changelog-change-reader", "changelog-changelog-writer", "changelog-editor"]);
        assert_eq!(launch.agent, "changelog-leader");
        assert_eq!(v["changelog-changelog-writer"]["skills"], serde_json::json!(["changelog-entry"]));
        assert!(v["changelog-changelog-writer"]["tools"].as_array().unwrap().iter().any(|t| t == "Skill"));
        // A copy of it is the user's own, to change: it uses the plugin's agents, and removing the plugin says so.
        let mine = squadfile::copy_of(&config, &squad, "Careful changelog", &[]);
        squadfile::save(&config, &mine).unwrap();
        let users = users_of_plugin(&config, "changelog");
        assert_eq!(users, [("Careful changelog".to_string(), vec!["change-reader".to_string(), "changelog-writer".to_string()])]);
        assert_eq!(removal_warning(&users).unwrap(), "Your squad Careful changelog uses its agents: those steps will show the agent as missing until you pick another one on the Squads tab.");
        let two = [("A".to_string(), vec![]), ("B".to_string(), vec![]), ("C".to_string(), vec![])];
        assert!(removal_warning(&two).unwrap().starts_with("Your squads A, B and C use its agents"));
        assert_eq!(removal_warning(&[]), None);
        // The snapshot of a leader that started is all it needs: it reads back whole once the plugin has gone.
        let snapshot = launch.to_json();
        store.remove("changelog").unwrap();
        assert_eq!(crate::squads::Launch::from_json(&snapshot), Some(launch));
        // Gone: not listed, not found, and the user's copy says which agent is missing.
        assert!(squadfile::load_all(&config).iter().all(|l| l.kind != Kind::Plugin));
        assert!(squadfile::get(&config, "plugin:changelog/changelog").is_none() && agentfile::get(&config, "plugin:changelog/change-reader").is_none());
        let mine = squadfile::get(&config, &mine.id).unwrap();
        assert_eq!(squadfile::problems(&mine, &agentfile::all(&config))[0], "Step 1 names an agent that isn't there (plugin:changelog/change-reader).");
    }

    #[test]
    fn each_bad_squad_or_agent_fails_the_check_plainly() {
        let base = tmp("sq-bad");
        let reasons = |dir: &Path| check_plugin(dir).problems.join(" | ");
        let edit = |dir: &Path, rel: &str, f: &dyn Fn(&mut Value)| {
            let p = dir.join(rel);
            let mut v: Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
            f(&mut v);
            fs::write(p, v.to_string()).unwrap();
        };
        // A step naming an agent that is neither built in nor the plugin's (the user's own can't be counted on).
        let d = plugin(&base.join("a"), "changelog", "1.0.0");
        edit(&d, "squads/changelog.json", &|v| {
            if let Some(s) = v.get_mut("workflow").and_then(|w| w.get_mut(0)).and_then(Value::as_object_mut) {
                s.insert("agent".into(), "my-own-agent".into());
            }
        });
        assert!(reasons(&d).contains("squads/changelog.json: Step 1 names an agent that isn't there (my-own-agent)."), "{}", reasons(&d));
        // An agent with a built-in's id, one with a built-in's name, one without a system prompt, a bad model.
        let d = plugin(&base.join("b"), "changelog", "1.0.0");
        fs::rename(d.join("agents/change-reader.json"), d.join("agents/planner.json")).unwrap();
        edit(&d, "plugin.json", &|v| {
            if let Some(o) = v.as_object_mut() {
                o.insert("agents".into(), serde_json::json!(["agents/planner.json", "agents/changelog-writer.json"]));
            }
        });
        assert!(reasons(&d).contains("\"planner\" is one of platoon's built-in agents"), "{}", reasons(&d));
        let d = plugin(&base.join("c"), "changelog", "1.0.0");
        edit(&d, "agents/change-reader.json", &|v| {
            if let Some(o) = v.as_object_mut() {
                o.insert("name".into(), "Builder".into());
                o.insert("guidance".into(), "".into());
                o.insert("model".into(), "gpt".into());
            }
        });
        let r = reasons(&d);
        assert!(r.contains("is the name of platoon's built-in agent builder") && r.contains("has no system prompt yet") && r.contains("isn't one platoon offers"), "{r}");
        // A squad with a built-in's id or name, a file that isn't JSON, too big, missing, a bad id.
        let d = plugin(&base.join("d"), "changelog", "1.0.0");
        edit(&d, "squads/changelog.json", &|v| {
            if let Some(o) = v.as_object_mut() {
                o.insert("name".into(), "coding".into());
            }
        });
        assert!(reasons(&d).contains("is the name of platoon's built-in squad"), "{}", reasons(&d));
        fs::write(d.join("squads/changelog.json"), "{ nope").unwrap();
        assert!(reasons(&d).contains("squads/changelog.json isn't valid JSON"));
        fs::write(d.join("squads/changelog.json"), format!("{{\"name\": \"{}\"}}", "x".repeat(70_000))).unwrap();
        assert!(reasons(&d).contains("bigger than 64 KiB"));
        fs::remove_file(d.join("squads/changelog.json")).unwrap();
        assert!(reasons(&d).contains("squads/changelog.json is listed in plugin.json but isn't in the folder"));
        let d = plugin(&base.join("e"), "changelog", "1.0.0");
        fs::rename(d.join("squads/changelog.json"), d.join("squads/Change_Log.json")).unwrap();
        edit(&d, "plugin.json", &|v| {
            if let Some(o) = v.as_object_mut() {
                o.insert("squads".into(), serde_json::json!(["squads/Change_Log.json"]));
            }
        });
        assert!(reasons(&d).contains("\"Change_Log\" can't be an id"), "{}", reasons(&d));
    }
}
