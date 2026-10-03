//! `platoon setup`: the one-time changes platoon makes to Claude Code's settings, as a checklist.
//!
//! Every step's status is computed from the settings file itself, so "done" is always a fact about the
//! files as they are now: `platoon` refuses to start until every step is done, running `setup` again
//! is harmless, and it repairs a step whose command points at a platoon that no longer exists.

use crate::{ai, settings};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, Write},
    path::{Path, PathBuf},
};

pub struct Step {
    pub name: &'static str,
    pub what: &'static str,
    /// platoon refuses to start without it. A step that isn't (the two notes hooks) only makes a feature work.
    pub blocking: bool,
    /// What is said, plainly, when a step that isn't blocking hasn't been done.
    pub missing_note: &'static str,
}

/// Same order as `check` and `Report::outcomes`.
pub const STEPS: [Step; 5] = [
    Step { name: "Status line", what: "plan usage, and Claude's own context % in the list", blocking: true, missing_note: "" },
    Step { name: "Session guard", what: "stops two copies of a session overwriting each other", blocking: true, missing_note: "" },
    Step {
        name: "Project notes",
        what: "hands a project's notes to its sessions when they start",
        blocking: false,
        missing_note: "not set up yet: projects work, but their notes don't reach sessions",
    },
    Step {
        name: "Notes updates",
        what: "lets platoon keep a project's notes up to date after its sessions stop",
        blocking: false,
        missing_note: "not set up yet: projects work, but platoon can't keep their notes up to date by itself",
    },
    Step {
        name: "Tickets skill",
        what: "/to-tickets in Claude Code: turns a conversation into tickets",
        blocking: false,
        missing_note: "not set up yet: tickets are still made in platoon, but not with /to-tickets",
    },
];

#[derive(Debug, PartialEq, Eq)]
pub enum State {
    Done,
    Missing,
    /// Present, but its command points at a platoon that isn't there.
    Broken(String),
    /// Deliberately left alone (someone else's status line); doesn't block starting.
    Skipped(String),
    /// There, but not what this platoon would put there (the tickets skill: platoon's own copy has changed).
    OutOfDate,
    /// The user took it out (`platoon setup --remove`): setup leaves it out. Only the tickets skill.
    Removed,
    /// The user changed platoon's copy: setup leaves it alone. Only the tickets skill.
    Changed,
}

impl State {
    pub fn satisfied(&self) -> bool {
        matches!(self, State::Done | State::Skipped(_))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    AlreadyDone,
    Added,
    Repaired,
    /// Replaced by platoon's newer copy (the tickets skill).
    Updated,
    Skipped(String),
    /// Left out because the user removed it (the tickets skill).
    Removed,
    /// Left alone because the user changed it (the tickets skill).
    Changed,
}

pub struct Report {
    pub outcomes: Vec<Outcome>,
    /// Set when settings.json was rewritten.
    pub saved: Option<PathBuf>,
    /// Set when the tickets skill was written.
    pub skill: Option<PathBuf>,
    pub notes: Vec<String>,
}

fn read_settings(root: &Path) -> Option<Value> {
    fs::read_to_string(root.join("settings.json")).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok())
}

/// The program a hook command runs: `'/a b/platoon' guard` -> `/a b/platoon`.
fn program_of(command: &str) -> &str {
    let c = command.trim_start();
    match c.strip_prefix('\'') {
        Some(rest) => rest.split('\'').next().unwrap_or(""),
        None => c.split_whitespace().next().unwrap_or(""),
    }
}

/// A command we wrote points at an absolute path; check that it still exists.
fn command_state(command: &str) -> State {
    let prog = program_of(command);
    if Path::new(prog).is_absolute() && !Path::new(prog).exists() { State::Broken(format!("{prog} no longer exists")) } else { State::Done }
}

fn is_statusline(c: &str) -> bool {
    c.contains("platoon") && c.contains("statusline")
}

fn is_guard(c: &str) -> bool {
    c.contains("platoon") && c.contains("hook guard")
}

fn is_notes_hook(c: &str) -> bool {
    c.contains("platoon") && c.contains("hook session-start")
}

fn is_stop_hook(c: &str) -> bool {
    c.contains("platoon") && c.contains("hook stop")
}

fn hook_commands<'a>(v: &'a Value, event: &str, is_ours: fn(&str) -> bool) -> Vec<&'a str> {
    v["hooks"][event]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|g| g["hooks"].as_array().into_iter().flatten())
        .filter_map(|h| h["command"].as_str())
        .filter(|c| is_ours(c))
        .collect()
}

fn guard_commands(v: &Value) -> Vec<&str> {
    v["hooks"]["UserPromptSubmit"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|g| g["hooks"].as_array().into_iter().flatten())
        .filter_map(|h| h["command"].as_str())
        .filter(|c| is_guard(c))
        .collect()
}

fn check_statusline(v: &Value) -> State {
    match v["statusLine"]["command"].as_str() {
        Some(c) if is_statusline(c) => command_state(c),
        Some(other) => {
            State::Skipped(format!("you have your own status line ({other}); make it also run `platoon statusline` to see plan usage"))
        }
        None => State::Missing,
    }
}

fn check_notes_hook(v: &Value) -> State {
    commands_state(&hook_commands(v, "SessionStart", is_notes_hook))
}

fn check_stop_hook(v: &Value) -> State {
    commands_state(&hook_commands(v, "Stop", is_stop_hook))
}

fn commands_state(cmds: &[&str]) -> State {
    match cmds.iter().map(|c| command_state(c)).find(|s| *s != State::Done) {
        _ if cmds.is_empty() => State::Missing,
        Some(broken) => broken,
        None => State::Done,
    }
}

fn check_guard(v: &Value) -> State {
    let cmds = guard_commands(v);
    match cmds.iter().map(|c| command_state(c)).find(|s| *s != State::Done) {
        _ if cmds.is_empty() => State::Missing,
        Some(broken) => broken,
        None => State::Done,
    }
}

/// Where each step stands, in `STEPS` order. `root` is Claude Code's folder, `config` platoon's own (where setup
/// remembers what the user chose about the tickets skill).
pub fn check(root: &Path, config: &Path) -> Vec<State> {
    let v = read_settings(root).unwrap_or(Value::Null);
    vec![check_statusline(&v), check_guard(&v), check_notes_hook(&v), check_stop_hook(&v), check_skill(root, config)]
}

/// The `/to-tickets` skill in Claude Code's skills folder: a file, not a setting.
fn check_skill(root: &Path, config: &Path) -> State {
    skill_state(crate::totickets::installed(root, config))
}

fn skill_state(i: crate::totickets::Installed) -> State {
    use crate::totickets::Installed;
    match i {
        Installed::Yes => State::Done,
        Installed::No => State::Missing,
        Installed::OutOfDate => State::OutOfDate,
        Installed::Removed => State::Removed,
        Installed::Changed => State::Changed,
    }
}

/// The steps `platoon setup --remove` and `--add` take, by the word they take.
pub const REMOVABLE: &[&str] = &[crate::totickets::NAME];
/// The steps `--replace` takes.
pub const REPLACEABLE: &[&str] = &[crate::totickets::NAME];

/// What the tickets skill's line says when setup left it out or alone.
const REMOVED_NOTE: &str = "removed by you: `platoon setup --add to-tickets` puts it back";
const CHANGED_NOTE: &str =
    "changed by you, so left as it is: `platoon setup --replace to-tickets` puts platoon's copy in (yours is kept as SKILL.md.platoon-bak)";

fn unknown_step(verb: &str, other: &str) -> String {
    let can = if verb == "replace" { REPLACEABLE } else { REMOVABLE };
    format!("platoon setup can't {verb} \"{other}\"; it can {verb}: {}", can.join(", "))
}

/// `platoon setup --remove <what>`: take one step out again, and remember it. Says what happened in one line.
pub fn remove_step(root: &Path, config: &Path, what: &str) -> Result<String, String> {
    match what {
        crate::totickets::NAME => {
            let path = crate::totickets::skill_path(root);
            Ok(if crate::totickets::remove(root, config)? {
                format!(
                    "Removed the /to-tickets skill ({}). `platoon setup` leaves it out from now on; `platoon setup --add to-tickets` puts it back.",
                    path.display()
                )
            } else {
                "The /to-tickets skill wasn't there. `platoon setup` leaves it out from now on; `platoon setup --add to-tickets` puts it back.".to_string()
            })
        }
        other => Err(unknown_step("remove", other)),
    }
}

/// `platoon setup --add <what>`: undo a removal and set the step up again. One line.
pub fn add_step(root: &Path, config: &Path, what: &str) -> Result<String, String> {
    use crate::totickets::Installed;
    match what {
        crate::totickets::NAME => {
            let path = crate::totickets::skill_path(root);
            Ok(match crate::totickets::add(root, config)? {
                Installed::No | Installed::Removed => format!("Added the /to-tickets skill ({}).", path.display()),
                Installed::OutOfDate => format!("Updated the /to-tickets skill ({}) to platoon's newer copy.", path.display()),
                Installed::Yes => "The /to-tickets skill is already there.".to_string(),
                Installed::Changed => format!("The /to-tickets skill is there already, {CHANGED_NOTE}."),
            })
        }
        other => Err(unknown_step("add", other)),
    }
}

/// `platoon setup --replace <what>`: platoon's copy in place of the user's, theirs kept beside it. One line.
pub fn replace_step(root: &Path, config: &Path, what: &str) -> Result<String, String> {
    match what {
        crate::totickets::NAME => {
            let path = crate::totickets::skill_path(root);
            Ok(if crate::totickets::replace(root, config)? {
                format!(
                    "Put platoon's /to-tickets skill in {}; yours is kept as {}.",
                    path.display(),
                    crate::totickets::backup_path(root).display()
                )
            } else {
                format!("Wrote platoon's /to-tickets skill to {}.", path.display())
            })
        }
        other => Err(unknown_step("replace", other)),
    }
}

/// Every step platoon can't do without is done (the notes hook is not one of them).
pub fn ready(states: &[State]) -> bool {
    states.iter().zip(STEPS.iter()).all(|(s, step)| !step.blocking || s.satisfied())
}

/// True when the SessionStart hook that hands project notes to sessions is present and working.
pub fn notes_hook_installed(root: &Path) -> bool {
    read_settings(root).is_some_and(|v| check_notes_hook(&v) == State::Done)
}

/// True when the Stop hook that keeps a project's notes up to date is present and working.
pub fn stop_hook_installed(root: &Path) -> bool {
    read_settings(root).is_some_and(|v| check_stop_hook(&v) == State::Done)
}

pub fn statusline_installed(root: &Path) -> bool {
    read_settings(root).is_some_and(|v| check_statusline(&v) == State::Done)
}

/// True when the guard hook is present and working, i.e. later copies of a running session get refused.
pub fn guard_installed(root: &Path) -> bool {
    read_settings(root).is_some_and(|v| check_guard(&v) == State::Done)
}

fn remove_guard(v: &mut Value) {
    remove_hook(v, "UserPromptSubmit", is_guard);
}

/// Take our own hook commands out of `event`, and any group that holds nothing else. Other hooks stay.
fn remove_hook(v: &mut Value, event: &str, is_ours: fn(&str) -> bool) {
    // `get_mut`, not indexing: mutable indexing on a serde_json Value creates the missing keys as null.
    let groups = v.get_mut("hooks").and_then(|h| h.get_mut(event)).and_then(Value::as_array_mut);
    if let Some(groups) = groups {
        for g in groups.iter_mut() {
            if let Some(hs) = g.get_mut("hooks").and_then(Value::as_array_mut) {
                hs.retain(|h| !h.get("command").and_then(Value::as_str).is_some_and(is_ours));
            }
        }
        groups.retain(|g| g.get("hooks").and_then(Value::as_array).is_none_or(|hs| !hs.is_empty()));
    }
}

/// Add one hook command to `event`, in a group of its own.
fn add_hook(v: &mut Value, event: &str, command: String) -> Result<(), String> {
    let list = v
        .as_object_mut()
        .ok_or("settings.json isn't a JSON object")?
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("\"hooks\" in settings.json isn't an object")?
        .entry(event)
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| format!("\"hooks.{event}\" in settings.json isn't a list"))?;
    list.push(json!({ "hooks": [{ "type": "command", "command": command }] }));
    Ok(())
}

/// Take the notes hook out of Claude Code's settings in `root` again, leaving everything else. True when it was there.
#[cfg(test)]
pub fn remove_notes_hook(root: &Path) -> Result<bool, String> {
    remove_one_hook(root, "SessionStart", is_notes_hook, check_notes_hook)
}

/// The same for the Stop hook that keeps the notes up to date.
#[cfg(test)]
pub fn remove_stop_hook(root: &Path) -> Result<bool, String> {
    remove_one_hook(root, "Stop", is_stop_hook, check_stop_hook)
}

#[cfg(test)]
fn remove_one_hook(root: &Path, event: &str, is_ours: fn(&str) -> bool, check: fn(&Value) -> State) -> Result<bool, String> {
    let path = root.join("settings.json");
    let mut v: Value = serde_json::from_str(&fs::read_to_string(&path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    if check(&v) == State::Missing {
        return Ok(false);
    }
    remove_hook(&mut v, event, is_ours);
    fs::write(&path, serde_json::to_string_pretty(&v).map_err(|e| e.to_string())? + "\n").map_err(|e| e.to_string())?;
    Ok(true)
}

/// Do whatever is still missing or broken. Everything else in settings.json, including other hooks and
/// someone else's status line, is left exactly as it was.
pub fn run(root: &Path, config: &Path) -> Result<Report, String> {
    let path = root.join("settings.json");
    let mut v: Value = match fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).map_err(|e| format!("{} isn't valid JSON: {e}", path.display()))?,
        Err(_) => json!({}),
    };
    if !v.is_object() {
        return Err(format!("{} isn't a JSON object", path.display()));
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.to_string_lossy().into_owned();
    let quoted = if exe.contains(' ') { format!("'{exe}'") } else { exe.clone() };
    let mut outcomes = Vec::new();
    let mut changed = false;

    outcomes.push(match check_statusline(&v) {
        State::Done => Outcome::AlreadyDone,
        State::Skipped(why) => Outcome::Skipped(why),
        s @ (State::Missing | State::Broken(_) | State::OutOfDate | State::Removed | State::Changed) => {
            v["statusLine"] = json!({ "type": "command", "command": format!("{quoted} statusline") });
            changed = true;
            if s == State::Missing { Outcome::Added } else { Outcome::Repaired }
        }
    });

    let guard = check_guard(&v);
    outcomes.push(match guard {
        State::Done | State::Skipped(_) => Outcome::AlreadyDone,
        State::Missing | State::Broken(_) | State::OutOfDate | State::Removed | State::Changed => {
            remove_guard(&mut v);
            let list = v
                .as_object_mut()
                .unwrap()
                .entry("hooks")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .ok_or("\"hooks\" in settings.json isn't an object")?
                .entry("UserPromptSubmit")
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .ok_or("\"hooks.UserPromptSubmit\" in settings.json isn't a list")?;
            list.push(json!({ "hooks": [{ "type": "command", "command": format!("{quoted} hook guard") }] }));
            changed = true;
            if guard == State::Missing { Outcome::Added } else { Outcome::Repaired }
        }
    });

    for (event, command, is_ours, check) in [
        ("SessionStart", "hook session-start", is_notes_hook as fn(&str) -> bool, check_notes_hook as fn(&Value) -> State),
        ("Stop", "hook stop", is_stop_hook, check_stop_hook),
    ] {
        let state = check(&v);
        outcomes.push(match state {
            State::Done | State::Skipped(_) => Outcome::AlreadyDone,
            State::Missing | State::Broken(_) | State::OutOfDate | State::Removed | State::Changed => {
                remove_hook(&mut v, event, is_ours);
                add_hook(&mut v, event, format!("{quoted} {command}"))?;
                changed = true;
                if state == State::Missing { Outcome::Added } else { Outcome::Repaired }
            }
        });
    }

    let mut saved = None;
    let mut notes = Vec::new();
    if changed {
        if path.exists() {
            fs::copy(&path, root.join("settings.json.platoon-bak")).map_err(|e| format!("couldn't back up settings: {e}"))?;
        }
        fs::write(&path, serde_json::to_string_pretty(&v).map_err(|e| e.to_string())? + "\n")
            .map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
        saved = Some(path);
        if exe.contains("/target/") {
            notes.push(
                "That's a build directory path. Run `cargo install --path .` and then `platoon setup` again for a stable one.".to_string(),
            );
        }
    }

    // The tickets skill: a file of its own in Claude Code's skills folder. It never blocks, so a failure to write
    // it is said on its line and setup carries on. One the user removed or changed is left as it is.
    let mut skill = None;
    outcomes.push(match crate::totickets::setup(root, config) {
        Ok(crate::totickets::Installed::Yes) => Outcome::AlreadyDone,
        Ok(crate::totickets::Installed::No) => {
            skill = Some(crate::totickets::skill_path(root));
            Outcome::Added
        }
        Ok(crate::totickets::Installed::OutOfDate) => {
            skill = Some(crate::totickets::skill_path(root));
            Outcome::Updated
        }
        Ok(crate::totickets::Installed::Removed) => Outcome::Removed,
        Ok(crate::totickets::Installed::Changed) => Outcome::Changed,
        Err(why) => Outcome::Skipped(why),
    });
    Ok(Report { outcomes, saved, skill, notes })
}

/// What became of the question about the features that use tokens.
#[derive(Debug, PartialEq, Eq)]
pub enum TokenQuestion {
    /// Asked before (the answer is in the settings): nothing is printed.
    AlreadyAsked,
    /// Not in a terminal: nothing changed, one line says how to turn them on.
    NotInteractive,
    /// The settings file couldn't be read, so it was left alone.
    SettingsUnreadable,
    /// The person answered and it was saved (true = yes).
    Answered(bool),
    /// No answer came (the input ended): nothing saved, it will be asked again next time.
    NoAnswer,
    /// Answered, but the settings couldn't be written.
    NotSaved,
}

/// The one question `platoon setup` adds to the checklist: may platoon use the person's Claude tokens for smart
/// features? Asked only in a terminal and only until it has been answered; the default is no; nothing else in
/// setup, and nothing about starting platoon, depends on it. `interactive` is whether both stdin and stdout are a
/// terminal (a parameter so tests can give it a script).
pub fn ask_about_tokens(config: &Path, interactive: bool, input: &mut dyn BufRead, out: &mut dyn Write) -> TokenQuestion {
    let (mut current, corrupt) = settings::load(config);
    if corrupt {
        let _ = writeln!(
            out,
            "\nYour settings file couldn't be read, so platoon left the smart features (which use your Claude tokens) off and didn't ask about them."
        );
        return TokenQuestion::SettingsUnreadable;
    }
    if current.tokens_asked {
        return TokenQuestion::AlreadyAsked;
    }
    if !interactive {
        let _ = writeln!(out, "\n{}", ai::setup_hint());
        return TokenQuestion::NotInteractive;
    }
    let _ = write!(out, "\n{} [y/N] ", ai::consent_text());
    let _ = out.flush();
    let mut line = String::new();
    if input.read_line(&mut line).unwrap_or(0) == 0 {
        let _ = writeln!(out, "\nNo answer, so they stay off. `platoon setup` will ask again.");
        return TokenQuestion::NoAnswer;
    }
    // Anything but a clear yes is a no.
    let yes = matches!(line.trim().to_lowercase().as_str(), "y" | "yes");
    current.tokens_asked = true;
    current.tokens_enabled = yes;
    if yes {
        for f in ai::FEATURES {
            current.set_feature(f.id, true);
        }
    }
    match settings::save(config, &current) {
        Ok(()) if yes => {
            let _ =
                writeln!(out, "Smart features are on. Turn them off any time: press s in platoon and switch off \"{}\".", ai::MASTER_LABEL);
            TokenQuestion::Answered(true)
        }
        Ok(()) => {
            let _ = writeln!(out, "Smart features stay off. {}", ai::enable_hint(&settings::path_in(config)));
            TokenQuestion::Answered(false)
        }
        Err(e) => {
            let _ = writeln!(out, "Couldn't save your answer ({e}); the smart features stay off. `platoon setup` will ask again.");
            TokenQuestion::NotSaved
        }
    }
}

fn paint(text: &str, code: &str, color: bool) -> String {
    if color { format!("\x1b[{code}m{text}\x1b[0m") } else { text.to_string() }
}

fn row(ok: Option<bool>, step: &Step, detail: &str, color: bool) -> String {
    let (glyph, code) = match ok {
        Some(true) => ("✓", "32"),
        Some(false) => ("✗", "31"),
        None => ("–", "33"),
    };
    let mut line = format!("  [{}] {:<15}{}", paint(glyph, code, color), step.name, step.what);
    if !detail.is_empty() {
        line.push_str(&format!("\n      {}", paint(detail, "2", color)));
    }
    line
}

/// What `platoon setup` prints after running.
pub fn render_report(r: &Report, root: &Path, color: bool) -> String {
    let mut out = vec!["platoon setup".to_string(), String::new()];
    for (step, o) in STEPS.iter().zip(&r.outcomes) {
        out.push(match o {
            Outcome::AlreadyDone => row(Some(true), step, "already done", color),
            Outcome::Added => row(Some(true), step, "added", color),
            Outcome::Repaired => row(Some(true), step, "repaired: its command pointed at a platoon that no longer exists", color),
            Outcome::Updated => row(Some(true), step, "updated: platoon's copy had changed", color),
            Outcome::Skipped(why) => row(None, step, &format!("skipped: {why}"), color),
            Outcome::Removed => row(None, step, REMOVED_NOTE, color),
            Outcome::Changed => row(None, step, CHANGED_NOTE, color),
        });
    }
    out.push(String::new());
    if r.saved.is_some() {
        out.push(format!("Saved {} (backup: settings.json.platoon-bak).", root.join("settings.json").display()));
        out.push("Claude Code picks up hook changes live. Restart running sessions for the status line.".into());
    }
    if let Some(p) = &r.skill {
        out.push(format!(
            "Wrote {}. Type /to-tickets in a session that is in a project; `platoon setup --remove to-tickets` takes it out.",
            p.display()
        ));
    }
    if r.saved.is_none() && r.skill.is_none() {
        out.push("Nothing to change.".into());
    }
    out.extend(r.notes.iter().cloned());
    out.push("All set. Run `platoon`.".into());
    out.join("\n")
}

/// What `platoon` prints instead of starting when setup isn't complete.
pub fn render_blocked(states: &[State], color: bool) -> String {
    let mut out = vec!["platoon needs a one-time setup before it can run.".to_string(), String::new()];
    for (step, s) in STEPS.iter().zip(states) {
        out.push(match s {
            State::Done => row(Some(true), step, "", color),
            State::Skipped(why) => row(None, step, &format!("skipped: {why}"), color),
            // Not needed to start: said plainly, without the red cross.
            State::Missing if !step.blocking => row(None, step, step.missing_note, color),
            State::Broken(why) if !step.blocking => row(None, step, &format!("broken: {why}"), color),
            State::OutOfDate => row(None, step, "out of date: `platoon setup` puts platoon's newer copy in", color),
            State::Removed => row(None, step, REMOVED_NOTE, color),
            State::Changed => row(None, step, CHANGED_NOTE, color),
            State::Missing => row(Some(false), step, "not set up", color),
            State::Broken(why) => row(Some(false), step, &format!("broken: {why}"), color),
        });
    }
    out.push(String::new());
    out.push("Run `platoon setup` to finish. It backs up settings.json first and never touches your other settings.".into());
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-setup-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// platoon's own config folder for a test's Claude folder.
    fn pc(root: &Path) -> PathBuf {
        root.join("platoon-config")
    }

    fn settings(root: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(root.join("settings.json")).unwrap()).unwrap()
    }

    #[test]
    fn nothing_is_done_at_first_and_everything_is_after_setup() {
        let root = tmp("fresh");
        let before = check(&root, &pc(&root));
        assert_eq!(before, (0..STEPS.len()).map(|_| State::Missing).collect::<Vec<_>>());
        assert!(!ready(&before));
        let r = run(&root, &pc(&root)).unwrap();
        assert_eq!(r.outcomes, (0..STEPS.len()).map(|_| Outcome::Added).collect::<Vec<_>>());
        assert_eq!(check(&root, &pc(&root)), (0..STEPS.len()).map(|_| State::Done).collect::<Vec<_>>());
        assert!(notes_hook_installed(&root) && stop_hook_installed(&root));
        assert!(ready(&check(&root, &pc(&root))) && statusline_installed(&root) && guard_installed(&root));
    }

    #[test]
    fn running_setup_again_marks_everything_done_and_changes_nothing() {
        let root = tmp("again");
        run(&root, &pc(&root)).unwrap();
        let text = fs::read_to_string(root.join("settings.json")).unwrap();
        let r = run(&root, &pc(&root)).unwrap();
        assert_eq!(r.outcomes, (0..STEPS.len()).map(|_| Outcome::AlreadyDone).collect::<Vec<_>>());
        assert!(r.saved.is_none());
        assert_eq!(fs::read_to_string(root.join("settings.json")).unwrap(), text);
        let shown = render_report(&r, &root, false);
        assert_eq!(shown.matches("[✓]").count(), STEPS.len(), "{shown}");
        assert!(shown.contains("already done") && shown.contains("Nothing to change"), "{shown}");
    }

    #[test]
    fn other_settings_and_hooks_are_preserved() {
        let root = tmp("keep");
        let original = r#"{"theme":"dark","hooks":{"Stop":[{"hooks":[{"type":"command","command":"x"}]}],"UserPromptSubmit":[{"hooks":[{"type":"command","command":"mine"}]}]}}"#;
        fs::write(root.join("settings.json"), original).unwrap();
        run(&root, &pc(&root)).unwrap();
        let v = settings(&root);
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "x");
        assert_eq!(v["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"], "mine");
        assert_eq!(v["hooks"]["UserPromptSubmit"].as_array().unwrap().len(), 2);
        assert_eq!(fs::read_to_string(root.join("settings.json.platoon-bak")).unwrap(), original);
    }

    #[test]
    fn someone_elses_status_line_is_skipped_not_replaced_and_does_not_block_starting() {
        let root = tmp("foreign");
        fs::write(root.join("settings.json"), r#"{"statusLine":{"type":"command","command":"my-script"}}"#).unwrap();
        assert!(matches!(check(&root, &pc(&root))[0], State::Skipped(_)));
        let r = run(&root, &pc(&root)).unwrap();
        assert!(matches!(r.outcomes[0], Outcome::Skipped(_)) && r.outcomes[1] == Outcome::Added);
        assert!(fs::read_to_string(root.join("settings.json")).unwrap().contains("my-script"));
        assert!(ready(&check(&root, &pc(&root))), "a skipped step must not lock the user out");
        assert!(render_report(&r, &root, false).contains("[–]"));
    }

    #[test]
    fn a_command_pointing_at_a_missing_platoon_is_broken_and_setup_repairs_it() {
        let root = tmp("broken");
        fs::write(
            root.join("settings.json"),
            r#"{"statusLine":{"type":"command","command":"/gone/platoon statusline"},
                "hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"/gone/platoon hook guard"}]}]}}"#,
        )
        .unwrap();
        let before = check(&root, &pc(&root));
        assert!(matches!(before[0], State::Broken(_)) && matches!(before[1], State::Broken(_)));
        assert!(!ready(&before));
        assert!(render_blocked(&before, false).contains("no longer exists"));
        let r = run(&root, &pc(&root)).unwrap();
        assert_eq!(r.outcomes, vec![Outcome::Repaired, Outcome::Repaired, Outcome::Added, Outcome::Added, Outcome::Added]);
        assert!(ready(&check(&root, &pc(&root))));
        // Repaired in place, not duplicated.
        assert_eq!(settings(&root)["hooks"]["UserPromptSubmit"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn the_blocked_message_lists_every_step_with_its_state() {
        let root = tmp("blocked");
        let shown = render_blocked(&check(&root, &pc(&root)), false);
        assert_eq!(shown.matches("[✗]").count(), 2, "{shown}");
        assert!(shown.contains("Status line") && shown.contains("Session guard") && shown.contains("platoon setup"), "{shown}");
        assert!(shown.contains("[–] Project notes") && shown.contains("notes don't reach sessions"), "optional, not a cross: {shown}");
        assert!(
            shown.contains("[–] Notes updates") && shown.contains("can't keep their notes up to date"),
            "the same for the Stop hook: {shown}"
        );
    }

    #[test]
    fn the_notes_hook_is_installed_and_removed_without_disturbing_other_hooks_and_never_blocks() {
        let root = tmp("notes-hook");
        let original = r#"{"hooks":{"SessionStart":[{"matcher":"startup","hooks":[{"type":"command","command":"my-start"}]}],"Stop":[{"hooks":[{"type":"command","command":"x"}]}]}}"#;
        fs::write(root.join("settings.json"), original).unwrap();
        run(&root, &pc(&root)).unwrap();
        let v = settings(&root);
        let starts = v["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(starts.len(), 2);
        assert_eq!(starts[0]["hooks"][0]["command"], "my-start");
        assert_eq!(starts[0]["matcher"], "startup");
        assert!(starts[1]["hooks"][0]["command"].as_str().unwrap().ends_with(" hook session-start"));
        assert!(starts[1].get("matcher").is_none(), "every source: startup, resume, clear and compact");
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "x", "somebody else's Stop hook is left alone");
        assert!(v["hooks"]["Stop"][1]["hooks"][0]["command"].as_str().unwrap().ends_with(" hook stop"), "ours goes beside it: {v}");
        assert!(notes_hook_installed(&root) && stop_hook_installed(&root));
        // Taken out again: only ours goes, and platoon still starts without it.
        assert!(remove_notes_hook(&root).unwrap());
        let v = settings(&root);
        assert_eq!(v["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
        assert_eq!(v["hooks"]["SessionStart"][0]["hooks"][0]["command"], "my-start");
        assert!(v["hooks"]["UserPromptSubmit"].as_array().is_some() && v.get("statusLine").is_some());
        let states = check(&root, &pc(&root));
        assert_eq!(states[2], State::Missing);
        assert!(ready(&states), "the notes hook never blocks starting");
        assert!(!remove_notes_hook(&root).unwrap());
        // A broken one (pointing at a platoon that is gone) is repaired in place, not duplicated.
        let mut v = settings(&root);
        v["hooks"]["SessionStart"]
            .as_array_mut()
            .unwrap()
            .push(json!({"hooks":[{"type":"command","command":"/gone/platoon hook session-start"}]}));
        fs::write(root.join("settings.json"), v.to_string()).unwrap();
        assert!(matches!(check(&root, &pc(&root))[2], State::Broken(_)) && ready(&check(&root, &pc(&root))));
        assert_eq!(run(&root, &pc(&root)).unwrap().outcomes[2], Outcome::Repaired);
        assert_eq!(settings(&root)["hooks"]["SessionStart"].as_array().unwrap().len(), 2);
        // The Stop hook behaves exactly the same way, and is just as optional.
        assert!(remove_stop_hook(&root).unwrap());
        let states = check(&root, &pc(&root));
        assert_eq!(states[3], State::Missing);
        assert!(ready(&states), "the Stop hook never blocks starting either");
        assert!(!remove_stop_hook(&root).unwrap());
        assert_eq!(settings(&root)["hooks"]["Stop"].as_array().unwrap().len(), 1, "only ours went");
        assert_eq!(run(&root, &pc(&root)).unwrap().outcomes[3], Outcome::Added);
        assert!(stop_hook_installed(&root));
    }

    #[test]
    fn the_tickets_skill_is_written_into_the_claude_folder_told_when_out_of_date_and_removed_on_request() {
        let root = tmp("skill");
        let skill = root.join("skills/to-tickets/SKILL.md");
        assert_eq!(check(&root, &pc(&root))[4], State::Missing);
        assert!(render_blocked(&check(&root, &pc(&root)), false).contains("[–] Tickets skill"), "never a cross: it doesn't block starting");
        let r = run(&root, &pc(&root)).unwrap();
        assert_eq!(r.outcomes[4], Outcome::Added);
        assert_eq!(fs::read_to_string(&skill).unwrap(), crate::totickets::SKILL);
        let shown = render_report(&r, &root, false);
        assert!(shown.contains("[✓] Tickets skill") && shown.contains(&format!("Wrote {}", skill.display())), "{shown}");
        assert!(shown.contains("platoon setup --remove to-tickets"), "{shown}");
        // Again: already done, nothing written.
        let r = run(&root, &pc(&root)).unwrap();
        assert!(r.outcomes[4] == Outcome::AlreadyDone && r.skill.is_none());
        assert!(render_report(&r, &root, false).contains("Nothing to change"));
        // platoon's own copy changed since (here: the record says platoon last wrote the older one on disk): out of
        // date, never blocking, and setup brings it up to date quietly.
        fs::write(&skill, "an older copy").unwrap();
        fs::write(pc(&root).join("setup.json"), json!({"to-tickets": {"written": crate::totickets::hash("an older copy")}}).to_string())
            .unwrap();
        let states = check(&root, &pc(&root));
        assert_eq!(states[4], State::OutOfDate);
        assert!(ready(&states));
        assert!(render_blocked(&states, false).contains("out of date"), "{}", render_blocked(&states, false));
        let r = run(&root, &pc(&root)).unwrap();
        assert_eq!(r.outcomes[4], Outcome::Updated);
        assert!(render_report(&r, &root, false).contains("updated: platoon's copy had changed"), "{}", render_report(&r, &root, false));
        assert_eq!(fs::read_to_string(&skill).unwrap(), crate::totickets::SKILL);
        assert!(!root.join("skills/to-tickets/SKILL.md.platoon-bak").exists(), "platoon's own copy needs no backup");
        assert!(r.saved.is_none(), "settings.json is not touched for it");
        // Changed by the user: left alone, said plainly, never blocking, and replaced only on request.
        fs::write(&skill, "my own copy").unwrap();
        let states = check(&root, &pc(&root));
        assert_eq!(states[4], State::Changed);
        assert!(ready(&states));
        assert!(render_blocked(&states, false).contains("changed by you"), "{}", render_blocked(&states, false));
        let r = run(&root, &pc(&root)).unwrap();
        assert!(r.outcomes[4] == Outcome::Changed && r.skill.is_none());
        let shown = render_report(&r, &root, false);
        assert!(
            shown.contains("[–] Tickets skill") && shown.contains("changed by you") && shown.contains("platoon setup --replace to-tickets"),
            "{shown}"
        );
        assert_eq!(fs::read_to_string(&skill).unwrap(), "my own copy");
        assert!(replace_step(&root, &pc(&root), "to-tickets").unwrap().contains("yours is kept as"));
        assert_eq!(fs::read_to_string(root.join("skills/to-tickets/SKILL.md.platoon-bak")).unwrap(), "my own copy");
        assert_eq!(fs::read_to_string(&skill).unwrap(), crate::totickets::SKILL);
        // Removed on request, and only that; setup leaves it out until it is added again.
        let settings_before = fs::read_to_string(root.join("settings.json")).unwrap();
        let said = remove_step(&root, &pc(&root), "to-tickets").unwrap();
        assert!(said.starts_with("Removed the /to-tickets skill") && said.contains("platoon setup --add to-tickets"), "{said}");
        assert!(!root.join("skills/to-tickets").exists());
        assert_eq!(fs::read_to_string(root.join("settings.json")).unwrap(), settings_before);
        assert_eq!(check(&root, &pc(&root))[4], State::Removed);
        assert!(remove_step(&root, &pc(&root), "to-tickets").unwrap().contains("wasn't there"));
        assert!(remove_step(&root, &pc(&root), "guard").unwrap_err().contains("can remove: to-tickets"));
        assert!(add_step(&root, &pc(&root), "guard").unwrap_err().contains("can add: to-tickets"));
        assert!(replace_step(&root, &pc(&root), "x").unwrap_err().contains("can replace: to-tickets"));
        let r = run(&root, &pc(&root)).unwrap();
        assert!(r.outcomes[4] == Outcome::Removed && !skill.exists());
        let shown = render_report(&r, &root, false);
        assert!(
            shown.contains("removed by you") && shown.contains("platoon setup --add to-tickets") && shown.contains("Nothing to change"),
            "{shown}"
        );
        assert!(render_blocked(&check(&root, &pc(&root)), false).contains("removed by you"));
        assert!(add_step(&root, &pc(&root), "to-tickets").unwrap().starts_with("Added the /to-tickets skill"));
        assert_eq!(check(&root, &pc(&root))[4], State::Done);
        assert!(add_step(&root, &pc(&root), "to-tickets").unwrap().contains("already there"));
        // A skill folder that can't be written is said on its line; the other steps are still done.
        let blocked = tmp("skill-blocked");
        fs::write(blocked.join("skills"), "a file where the folder should be").unwrap();
        let r = run(&blocked, &pc(&blocked)).unwrap();
        assert!(matches!(&r.outcomes[4], Outcome::Skipped(why) if why.contains("couldn't make")), "{:?}", r.outcomes[4]);
        assert!(ready(&check(&blocked, &pc(&blocked))));
    }

    #[test]
    fn invalid_settings_are_reported_not_overwritten() {
        let root = tmp("invalid");
        fs::write(root.join("settings.json"), "{ not json").unwrap();
        assert!(!ready(&check(&root, &pc(&root))));
        assert!(run(&root, &pc(&root)).is_err());
        assert_eq!(fs::read_to_string(root.join("settings.json")).unwrap(), "{ not json");
    }

    // ---- the old code editor question ----

    /// setup doesn't ask about a code editor or show a line for one, and an old `setup.json` that remembers the
    /// question an older platoon asked reads as before and is kept as it is.
    #[test]
    fn setup_knows_no_editor_and_an_old_setup_json_still_reads() {
        let root = tmp("old-editor-choice");
        let config = pc(&root);
        fs::create_dir_all(&config).unwrap();
        let old = "{\n  \"to-tickets\": {\n    \"written\": \"925710a6a1025c4c\"\n  },\n  \"editor\": {\n    \"asked\": true,\n    \"wanted\": true\n  }\n}\n";
        fs::write(config.join("setup.json"), old).unwrap();
        let r = run(&root, &config).unwrap();
        let shown = render_report(&r, &root, false);
        assert!(!shown.contains("Code editor"), "{shown}");
        assert!(ready(&check(&root, &config)));
        let kept: Value = serde_json::from_str(&fs::read_to_string(config.join("setup.json")).unwrap()).unwrap();
        assert_eq!(kept["editor"], serde_json::json!({"asked": true, "wanted": true}), "the old record is kept");
        assert!(crate::totickets::choice(&config, "editor").get("wanted").is_some());
        assert!(!render_blocked(&check(&root, &config), false).contains("Code editor"));
        // `editor` is a step like any other setup doesn't know.
        assert_eq!(add_step(&root, &config, "editor").unwrap_err(), "platoon setup can't add \"editor\"; it can add: to-tickets");
        assert!(remove_step(&root, &config, "editor").unwrap_err().contains("can remove: to-tickets"));
        assert!(remove_step(&root, &config, "x").unwrap_err().ends_with("can remove: to-tickets"));
        assert!(replace_step(&root, &config, "editor").unwrap_err().contains("can replace: to-tickets"));
    }

    fn ask(config: &Path, interactive: bool, typed: &str) -> (TokenQuestion, String) {
        let mut out = Vec::new();
        let q = ask_about_tokens(config, interactive, &mut std::io::Cursor::new(typed.as_bytes().to_vec()), &mut out);
        (q, String::from_utf8(out).unwrap())
    }

    fn cfg(name: &str) -> PathBuf {
        tmp(name).join("config")
    }

    #[test]
    fn the_question_lists_the_features_in_plain_words_and_says_what_is_sent_and_how_to_switch_off() {
        let c = cfg("ask-text");
        let (q, shown) = ask(&c, true, "\n");
        assert_eq!(q, TokenQuestion::Answered(false));
        for f in ai::FEATURES {
            assert!(shown.contains(f.label) && shown.contains(f.description), "{}: {shown}", f.id);
        }
        for want in [
            "your own Claude plan",
            "the prompts you typed",
            "one line of Claude's last reply",
            "small batches",
            "press s in platoon",
            ai::MASTER_LABEL,
            "[y/N]",
        ] {
            assert!(shown.contains(want), "{want:?}: {shown}");
        }
        let lower = shown.split("[y/N]").next().unwrap().to_lowercase();
        for jargon in ["json", "api", "index", "stdin", "usd", "$"] {
            assert!(!lower.contains(jargon), "{jargon}: {shown}");
        }
    }

    #[test]
    fn yes_turns_the_master_and_the_listed_features_on_and_remembers_it_was_asked() {
        for typed in ["y\n", "yes\n", "  YES \n", "Y"] {
            let c = cfg("ask-yes");
            let (q, _) = ask(&c, true, typed);
            assert_eq!(q, TokenQuestion::Answered(true), "{typed:?}");
            let s = settings::load(&c).0;
            assert!(s.tokens_enabled && s.tokens_asked, "{typed:?}");
            assert!(ai::FEATURES.iter().all(|f| s.feature_on(f.id)));
        }
    }

    #[test]
    fn no_and_enter_and_anything_else_leave_everything_off_but_still_count_as_answered() {
        for typed in ["n\n", "no\n", "\n", "maybe\n", "yep\n", "y e s\n"] {
            let c = cfg("ask-no");
            let (q, shown) = ask(&c, true, typed);
            assert_eq!(q, TokenQuestion::Answered(false), "{typed:?}");
            let s = settings::load(&c).0;
            assert!(!s.tokens_enabled && s.tokens_asked && s.token_features.is_empty(), "{typed:?}");
            assert!(shown.contains("stay off") && shown.contains("tokens_enabled"), "{shown}");
        }
    }

    #[test]
    fn it_is_asked_only_once() {
        let c = cfg("ask-once");
        assert_eq!(ask(&c, true, "n\n").0, TokenQuestion::Answered(false));
        let (q, shown) = ask(&c, true, "y\n");
        assert_eq!(q, TokenQuestion::AlreadyAsked);
        assert_eq!(shown, "", "nothing is printed the second time");
        assert!(!settings::load(&c).0.tokens_enabled, "the second answer was never read");
        // A yes that was given stays given when setup runs again.
        let c = cfg("ask-once-yes");
        ask(&c, true, "y\n");
        assert_eq!(ask(&c, true, "n\n").0, TokenQuestion::AlreadyAsked);
        assert!(settings::load(&c).0.tokens_enabled);
    }

    #[test]
    fn without_a_terminal_nothing_is_asked_nothing_is_saved_and_one_line_says_how_to_enable() {
        let c = cfg("ask-noninteractive");
        let (q, shown) = ask(&c, false, "y\n");
        assert_eq!(q, TokenQuestion::NotInteractive);
        assert!(!c.exists(), "nothing written");
        assert_eq!(shown.trim().lines().count(), 1, "{shown}");
        assert!(shown.contains("off") && shown.contains("platoon setup") && shown.contains(ai::MASTER_LABEL), "{shown}");
        // ...so a later run in a terminal still asks.
        assert_eq!(ask(&c, true, "y\n").0, TokenQuestion::Answered(true));
    }

    #[test]
    fn an_input_that_ends_without_an_answer_saves_nothing() {
        let c = cfg("ask-eof");
        let (q, shown) = ask(&c, true, "");
        assert_eq!(q, TokenQuestion::NoAnswer);
        assert!(!c.exists() && shown.contains("ask again"), "{shown}");
    }

    #[test]
    fn an_unreadable_settings_file_is_left_alone_and_nothing_is_asked() {
        let c = cfg("ask-corrupt");
        fs::create_dir_all(&c).unwrap();
        fs::write(c.join("settings.json"), "{ nope").unwrap();
        let (q, shown) = ask(&c, true, "y\n");
        assert_eq!(q, TokenQuestion::SettingsUnreadable);
        assert_eq!(fs::read_to_string(c.join("settings.json")).unwrap(), "{ nope");
        assert!(shown.contains("off"), "{shown}");
    }

    #[test]
    fn an_answer_keeps_the_rest_of_the_settings_and_a_failed_save_is_reported_not_fatal() {
        let c = cfg("ask-keep");
        fs::create_dir_all(&c).unwrap();
        fs::write(c.join("settings.json"), r#"{"idle_stop_minutes":30,"mine":{"a":1}}"#).unwrap();
        ask(&c, true, "y\n");
        let v: Value = serde_json::from_str(&fs::read_to_string(c.join("settings.json")).unwrap()).unwrap();
        assert_eq!(
            (v["idle_stop_minutes"].as_u64(), v["mine"].clone(), v["tokens_enabled"].clone()),
            (Some(30), json!({"a":1}), json!(true))
        );
        // The settings file can't be replaced (it is a folder here): the question reports it and carries on.
        let c = cfg("ask-fail");
        fs::create_dir_all(c.join("settings.json")).unwrap();
        let (q, _) = ask(&c, true, "");
        assert_eq!(q, TokenQuestion::SettingsUnreadable, "a folder where the file should be reads as unreadable");
    }
}
