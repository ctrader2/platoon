//! `platoon setup`: the one-time changes platoon makes to Claude Code's settings, as a checklist, and, when Codex is
//! on this computer, the same notes hooks in Codex's `hooks.json` (`run_codex`).
//!
//! Every step's status is computed from the settings file itself, so "done" is always a fact about the
//! files as they are now: `platoon` refuses to start until every step is done, running `setup` again
//! is harmless, and it repairs a step whose command points at a platoon that no longer exists. Codex's steps never
//! block starting platoon. Codex runs a hook only once it is trusted: setup has Codex itself trust exactly the hooks it
//! wrote (`trust_codex`), and uninstall has it forget them (`forget_codex_trust`).

use crate::agents::{codex, codex_server};
use crate::{ai, settings};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, Write},
    path::{Path, PathBuf},
    time::Duration,
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
pub const STEPS: [Step; 4] = [
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
];

/// The steps for Codex, in `codex::HOOKS` order. None of them blocks starting platoon.
pub const CODEX_STEPS: [Step; 3] = [
    Step {
        name: "Project notes",
        what: "hands a project's notes to its Codex sessions when they start",
        blocking: false,
        missing_note: "",
    },
    Step { name: "Changed notes", what: "passes a project's changed notes to a running Codex session", blocking: false, missing_note: "" },
    Step {
        name: "Notes updates",
        what: "lets platoon keep a project's notes up to date after its Codex sessions stop",
        blocking: false,
        missing_note: "",
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
}

impl State {
    pub fn satisfied(&self) -> bool {
        matches!(self, State::Done | State::Skipped(_))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    AlreadyDone,
    Added,
    Repaired,
    Skipped(String),
}

pub struct Report {
    pub outcomes: Vec<Outcome>,
    /// Set when settings.json was rewritten.
    pub saved: Option<PathBuf>,
    /// The file there was is kept as `settings.json.platoon-bak` (there was none to keep when setup made it).
    pub backed_up: bool,
    pub notes: Vec<String>,
    /// Codex's part, when Codex is on this computer.
    pub codex: Option<CodexReport>,
}

/// What setup did in Codex's `hooks.json`.
#[derive(Debug, PartialEq, Eq)]
pub struct CodexReport {
    /// In `CODEX_STEPS` order; empty when the file couldn't be used (`error`).
    pub outcomes: Vec<Outcome>,
    /// Set when hooks.json was written.
    pub saved: Option<PathBuf>,
    /// The file there was is kept as `hooks.json.platoon-bak` (there was none to keep when setup made it).
    pub backed_up: bool,
    /// Why the file was left alone: it is there but can't be read, or isn't a JSON object.
    pub error: Option<String>,
    /// What Codex says about trusting these hooks, once it has been asked (`trust_codex`).
    pub trust: Option<Trust>,
}

/// What became of asking Codex to trust platoon's hooks.
#[derive(Debug, PartialEq, Eq)]
pub enum Trust {
    /// What Codex says of each hook now, in `codex::HOOKS` order.
    Asked(Vec<HookTrust>),
    /// Codex couldn't be asked, or its answers made no sense: why, plainly.
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookTrust {
    /// Codex trusted it before this run.
    Already,
    /// Codex trusts it now because setup asked.
    Now,
    /// Codex doesn't say it trusts it (or doesn't list it).
    Not,
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

fn hook_commands<'a>(v: &'a Value, event: &str, is_ours: impl Fn(&str) -> bool) -> Vec<&'a str> {
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

/// Where each step stands, in `STEPS` order. `root` is Claude Code's folder.
pub fn check(root: &Path) -> Vec<State> {
    let v = read_settings(root).unwrap_or(Value::Null);
    vec![check_statusline(&v), check_guard(&v), check_notes_hook(&v), check_stop_hook(&v)]
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
fn remove_hook(v: &mut Value, event: &str, is_ours: impl Fn(&str) -> bool) {
    // `get_mut`, not indexing: mutable indexing on a serde_json Value creates the missing keys as null.
    let groups = v.get_mut("hooks").and_then(|h| h.get_mut(event)).and_then(Value::as_array_mut);
    if let Some(groups) = groups {
        for g in groups.iter_mut() {
            if let Some(hs) = g.get_mut("hooks").and_then(Value::as_array_mut) {
                hs.retain(|h| !h.get("command").and_then(Value::as_str).is_some_and(&is_ours));
            }
        }
        groups.retain(|g| g.get("hooks").and_then(Value::as_array).is_none_or(|hs| !hs.is_empty()));
    }
}

// ---- platoon uninstall: taking setup's entries out again ---------------------------------------------------------

/// One entry setup adds to settings.json, as `platoon uninstall` names it, in `STEPS` order (the status line first).
pub const ENTRIES: [(&str, &str); 4] = [
    ("statusLine", "the status line"),
    ("UserPromptSubmit", "the session guard hook"),
    ("SessionStart", "the project notes hook"),
    ("Stop", "the notes updates hook"),
];

/// The part of a command after its program: `'/a b/platoon' hook guard` -> `hook guard`.
fn args_of(command: &str) -> &str {
    let c = command.trim_start();
    let rest = match c.strip_prefix('\'') {
        Some(rest) => rest.split_once('\'').map_or("", |(_, after)| after),
        None => c.split_once(char::is_whitespace).map_or("", |(_, after)| after),
    };
    rest.trim()
}

/// Exactly what setup writes: a program called `platoon` (by any path), then `args` and nothing else. Uninstall takes
/// out only these; a command that only mentions platoon (the user's own status line that also runs `platoon
/// statusline`) is the user's.
fn exactly(command: &str, args: &str) -> bool {
    Path::new(program_of(command)).file_name().is_some_and(|n| n == "platoon") && args_of(command) == args
}

/// What `take_out` did to settings.json, and what it left that mentions platoon.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TakenOut {
    /// The `ENTRIES` labels of what was taken out, in that order.
    pub removed: Vec<&'static str>,
    /// Commands left in place that mention platoon but aren't exactly what setup writes (the user's own).
    pub left: Vec<String>,
}

/// Take platoon's own entries out of Claude Code's settings: the status line when it is exactly setup's, and setup's
/// hook commands. A hook group left empty goes, an event left with no group goes, and `hooks` goes when nothing is left
/// in it, but only when platoon's entry was what emptied it. Every other key stays where it was.
pub fn take_out(v: &mut Value) -> TakenOut {
    let mut out = TakenOut::default();
    if let Some(c) = v.get("statusLine").and_then(|s| s.get("command")).and_then(Value::as_str) {
        if exactly(c, "statusline") {
            if let Some(o) = v.as_object_mut() {
                o.remove("statusLine");
            }
            out.removed.push(ENTRIES[0].1);
        } else if c.contains("platoon") {
            out.left.push(c.to_string());
        }
    }
    let ours: [(&str, &str); 3] = [("UserPromptSubmit", "hook guard"), ("SessionStart", "hook session-start"), ("Stop", "hook stop")];
    let hooks: Vec<(&str, &str, &'static str)> = ours.iter().zip(&ENTRIES[1..]).map(|((e, a), (_, label))| (*e, *a, *label)).collect();
    take_out_hooks(v, &hooks, &mut out);
    out
}

/// What Codex's hooks are called when `platoon uninstall` says what it took out, in `codex::HOOKS` order.
pub const CODEX_ENTRIES: [&str; 3] = ["the Codex project notes hook", "the Codex changed notes hook", "the Codex notes updates hook"];

/// Take platoon's own hooks out of Codex's `hooks.json` (read into `v`): exactly the commands setup writes there
/// (`codex::HOOKS`), and the groups, events and `hooks` they leave empty, as `take_out` does. Every other key stays.
pub fn take_out_codex(v: &mut Value) -> TakenOut {
    let mut out = TakenOut::default();
    let hooks: Vec<(&str, &str, &'static str)> = codex::HOOKS.iter().zip(CODEX_ENTRIES).map(|((e, a), label)| (*e, *a, label)).collect();
    take_out_hooks(v, &hooks, &mut out);
    out
}

/// Take out the hook commands that are exactly `(event, args)` (as setup writes them), saying `label` for each
/// event they were taken out of; commands that only mention platoon are left and listed.
fn take_out_hooks(v: &mut Value, hooks: &[(&str, &str, &'static str)], out: &mut TakenOut) {
    for (event, args, label) in hooks {
        let all: Vec<String> = hook_commands(v, event, |c| c.contains("platoon")).into_iter().map(String::from).collect();
        let (mine, theirs): (Vec<String>, Vec<String>) = all.into_iter().partition(|c| exactly(c, args));
        out.left.extend(theirs);
        if mine.is_empty() {
            continue;
        }
        let Some(hooks) = v.get_mut("hooks").and_then(Value::as_object_mut) else { continue };
        if let Some(groups) = hooks.get_mut(*event).and_then(Value::as_array_mut) {
            let mut emptied = Vec::new();
            for (i, g) in groups.iter_mut().enumerate() {
                if let Some(hs) = g.get_mut("hooks").and_then(Value::as_array_mut) {
                    let before = hs.len();
                    hs.retain(|h| !h.get("command").and_then(Value::as_str).is_some_and(|c| exactly(c, args)));
                    if hs.len() < before && hs.is_empty() {
                        emptied.push(i);
                    }
                }
            }
            // Only the groups this emptied: a group of the user's that was already empty stays as it was.
            let mut i = 0;
            groups.retain(|_| {
                i += 1;
                !emptied.contains(&(i - 1))
            });
            if groups.is_empty() {
                hooks.remove(*event);
            }
        }
        if hooks.is_empty()
            && let Some(o) = v.as_object_mut()
        {
            o.remove("hooks");
        }
        out.removed.push(label);
    }
}

/// Add one hook command to `event`, in a group of its own. `file` names the file for a message.
fn add_hook(v: &mut Value, event: &str, command: String, file: &str) -> Result<(), String> {
    let list = v
        .as_object_mut()
        .ok_or_else(|| format!("{file} isn't a JSON object"))?
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| format!("\"hooks\" in {file} isn't an object"))?
        .entry(event)
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| format!("\"hooks.{event}\" in {file} isn't a list"))?;
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
pub fn run(root: &Path) -> Result<Report, String> {
    let path = root.join("settings.json");
    let mut v: Value = match fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).map_err(|e| format!("{} isn't valid JSON: {e}", path.display()))?,
        Err(_) => json!({}),
    };
    if !v.is_object() {
        return Err(format!("{} isn't a JSON object", path.display()));
    }
    let (exe, quoted) = this_program()?;
    let mut outcomes = Vec::new();
    let mut changed = false;

    outcomes.push(match check_statusline(&v) {
        State::Done => Outcome::AlreadyDone,
        State::Skipped(why) => Outcome::Skipped(why),
        s @ (State::Missing | State::Broken(_)) => {
            v["statusLine"] = json!({ "type": "command", "command": format!("{quoted} statusline") });
            changed = true;
            if s == State::Missing { Outcome::Added } else { Outcome::Repaired }
        }
    });

    let guard = check_guard(&v);
    outcomes.push(match guard {
        State::Done | State::Skipped(_) => Outcome::AlreadyDone,
        State::Missing | State::Broken(_) => {
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
            State::Missing | State::Broken(_) => {
                remove_hook(&mut v, event, is_ours);
                add_hook(&mut v, event, format!("{quoted} {command}"), "settings.json")?;
                changed = true;
                if state == State::Missing { Outcome::Added } else { Outcome::Repaired }
            }
        });
    }

    let mut saved = None;
    let mut backed_up = false;
    let mut notes = Vec::new();
    if changed {
        if path.exists() {
            fs::copy(&path, root.join("settings.json.platoon-bak")).map_err(|e| format!("couldn't back up settings: {e}"))?;
            backed_up = true;
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

    Ok(Report { outcomes, saved, backed_up, notes, codex: None })
}

/// This platoon's full path, and as a hook command writes it (quoted when it holds a space).
fn this_program() -> Result<(String, String), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?.to_string_lossy().into_owned();
    let quoted = if exe.contains(' ') { format!("'{exe}'") } else { exe.clone() };
    Ok((exe, quoted))
}

/// Codex's steps in its `hooks.json` under `codex_root` (`codex::HOOKS`, each a fixed command running this platoon):
/// whatever is missing, or points at a platoon that no longer exists, is put in; everything else in the file stays as
/// it was. A file that is there but can't be read or isn't a JSON object is reported and left alone. Never written
/// when nothing changed; otherwise backed up first (`hooks.json.platoon-bak`).
pub fn run_codex(codex_root: &Path) -> CodexReport {
    let fail = |error: String| CodexReport { outcomes: Vec::new(), saved: None, backed_up: false, error: Some(error), trust: None };
    let path = codex::hooks_path(codex_root);
    let mut v: Value = match fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(v) if v.is_object() => v,
            Ok(_) => return fail(format!("{} isn't a JSON object", path.display())),
            Err(e) => return fail(format!("{} isn't valid JSON ({e})", path.display())),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(e) => return fail(format!("couldn't read {}: {e}", path.display())),
    };
    let (_, quoted) = match this_program() {
        Ok(p) => p,
        Err(e) => return fail(e),
    };
    let mut outcomes = Vec::new();
    for (event, args) in codex::HOOKS {
        let ours = |c: &str| c.contains("platoon") && args_of(c) == args;
        let state = commands_state(&hook_commands(&v, event, ours));
        outcomes.push(match state {
            State::Done | State::Skipped(_) => Outcome::AlreadyDone,
            State::Missing | State::Broken(_) => {
                remove_hook(&mut v, event, ours);
                if let Err(e) = add_hook(&mut v, event, format!("{quoted} {args}"), "hooks.json") {
                    return fail(e);
                }
                if state == State::Missing { Outcome::Added } else { Outcome::Repaired }
            }
        });
    }
    if outcomes.iter().all(|o| *o == Outcome::AlreadyDone) {
        return CodexReport { outcomes, saved: None, backed_up: false, error: None, trust: None };
    }
    let written = (|| -> Result<bool, String> {
        fs::create_dir_all(codex_root).map_err(|e| format!("couldn't make {}: {e}", codex_root.display()))?;
        let backed_up = path.exists();
        if backed_up {
            fs::copy(&path, codex_root.join("hooks.json.platoon-bak")).map_err(|e| format!("couldn't back up {}: {e}", path.display()))?;
        }
        let text = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())? + "\n";
        crate::projects::write_atomic(&path, &text).map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
        Ok(backed_up)
    })();
    match written {
        Ok(backed_up) => CodexReport { outcomes, saved: Some(path), backed_up, error: None, trust: None },
        Err(e) => fail(e),
    }
}

/// Have Codex trust platoon's own hooks in its `hooks.json` under `codex_root`, as Codex's own "Trust" does (running
/// setup is the user's say-so): Codex's app-server (`program`) lists the hooks a session in `cwd` would run, and those
/// in that very file whose event and command are exactly what this setup writes (`codex::HOOKS`, running this
/// platoon), and that Codex doesn't trust yet or that changed since, are trusted in one write. No other hook is ever
/// trusted, whoever wrote it. Codex is then asked again what it says of each. The whole exchange ends within `timeout`.
pub fn trust_codex(codex_root: &Path, program: &str, cwd: &Path, timeout: Duration) -> Trust {
    let asked = (|| -> Result<Vec<HookTrust>, String> {
        let (_, quoted) = this_program()?;
        let file = codex::hooks_path(codex_root);
        let mine = |hooks: &[codex_server::Hook], event: &str, args: &str| -> Vec<codex_server::Hook> {
            let command = format!("{quoted} {args}");
            hooks.iter().filter(|h| h.is_in(&file) && h.is_event(event) && h.command == command).cloned().collect()
        };
        let trusted = |hs: &[codex_server::Hook]| !hs.is_empty() && hs.iter().all(|h| h.trust == codex_server::TrustStatus::Trusted);
        let mut server = codex_server::Server::start(program, codex_root, timeout)?;
        let before = server.hooks(cwd)?;
        let to_trust: Vec<codex_server::Hook> = codex::HOOKS
            .iter()
            .flat_map(|(event, args)| mine(&before, event, args))
            .filter(|h| matches!(h.trust, codex_server::TrustStatus::Untrusted | codex_server::TrustStatus::Modified))
            .collect();
        let after = if to_trust.is_empty() {
            before.clone()
        } else {
            server.trust(&to_trust)?;
            server.hooks(cwd)?
        };
        Ok(codex::HOOKS
            .iter()
            .map(|(event, args)| match (trusted(&mine(&before, event, args)), trusted(&mine(&after, event, args))) {
                (true, true) => HookTrust::Already,
                (false, true) => HookTrust::Now,
                (_, false) => HookTrust::Not,
            })
            .collect())
    })();
    match asked {
        Ok(each) => Trust::Asked(each),
        Err(why) => Trust::Failed(why),
    }
}

/// Have Codex forget that it trusted platoon's hooks, before `platoon uninstall` takes them out of `hooks.json` under
/// `codex_root` (Codex names a hook's record after its place in that file, so it is asked while they are still
/// there): the hooks Codex lists in that file whose command is exactly one setup writes (by any platoon's path, as
/// `take_out_codex` takes them out) lose their records in `hooks.state`; every other record is written back as Codex
/// read it, only if the file hasn't changed since. Ok: how many records went.
pub fn forget_codex_trust(codex_root: &Path, program: &str, cwd: &Path, timeout: Duration) -> Result<usize, String> {
    let file = codex::hooks_path(codex_root);
    let mut server = codex_server::Server::start(program, codex_root, timeout)?;
    let ours: Vec<String> = server
        .hooks(cwd)?
        .into_iter()
        .filter(|h| h.is_in(&file) && codex::HOOKS.iter().any(|(event, args)| h.is_event(event) && exactly(&h.command, args)))
        .map(|h| h.key)
        .collect();
    if ours.is_empty() {
        return Ok(0);
    }
    let state = server.hook_state()?;
    let (gone, kept): (serde_json::Map<String, Value>, serde_json::Map<String, Value>) =
        state.records.into_iter().partition(|(key, _)| ours.contains(key));
    if gone.is_empty() {
        return Ok(0);
    }
    server.replace_hook_state(kept, state.version.as_deref())?;
    Ok(gone.len())
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
            Outcome::Skipped(why) => row(None, step, &format!("skipped: {why}"), color),
        });
    }
    out.push(String::new());
    if r.saved.is_some() {
        out.push(saved_line(&root.join("settings.json"), r.backed_up.then_some("settings.json.platoon-bak")));
        out.push("Claude Code picks up hook changes live. Restart running sessions for the status line.".into());
    }
    let codex_changed = r
        .codex
        .as_ref()
        .is_some_and(|c| c.saved.is_some() || matches!(&c.trust, Some(Trust::Asked(each)) if each.contains(&HookTrust::Now)));
    if r.saved.is_none() && !codex_changed {
        out.push("Nothing to change.".into());
    }
    if let Some(c) = &r.codex {
        out.extend(render_codex(c, color));
    }
    out.extend(r.notes.iter().cloned());
    out.push("All set. Run `platoon`.".into());
    out.join("\n")
}

/// "Saved <file>.", naming the backup only when one was made.
fn saved_line(path: &Path, backup: Option<&str>) -> String {
    match backup {
        Some(b) => format!("Saved {} (backup: {b}).", path.display()),
        None => format!("Saved {}.", path.display()),
    }
}

/// Said when Codex will ask the user to trust platoon's hooks itself.
const CODEX_WILL_ASK: &str = "Codex asks you once to trust these hooks: the next time you start Codex it says they need \
                              review. Approve them there; until you do they don't run (nothing else in platoon waits for them).";

/// Codex's part of the report: its steps and whether Codex trusts each, the file written, and, when Codex couldn't be
/// made to trust them, the one thing it will ask of the user.
fn render_codex(c: &CodexReport, color: bool) -> Vec<String> {
    let mut out = vec![String::new(), "Codex".to_string()];
    if let Some(e) = &c.error {
        let step = Step { name: "Codex hooks", what: "carry a project's notes to its Codex sessions", blocking: false, missing_note: "" };
        out.push(row(None, &step, &format!("skipped: {e}. platoon left the file alone: fix it and run `platoon setup` again"), color));
        return out;
    }
    let each = match &c.trust {
        Some(Trust::Asked(each)) => each.as_slice(),
        _ => &[],
    };
    for (i, (step, o)) in CODEX_STEPS.iter().zip(&c.outcomes).enumerate() {
        let done = match o {
            Outcome::AlreadyDone => "already done".to_string(),
            Outcome::Added => "added".to_string(),
            Outcome::Repaired => "repaired: its command pointed at a platoon that no longer exists".to_string(),
            Outcome::Skipped(why) => format!("skipped: {why}"),
        };
        let trust = match each.get(i) {
            Some(HookTrust::Already) => "; Codex trusts it",
            Some(HookTrust::Now) => "; now trusted in Codex",
            Some(HookTrust::Not) => "; not trusted in Codex yet",
            None => "",
        };
        let ok = if matches!(o, Outcome::Skipped(_)) { None } else { Some(true) };
        out.push(row(ok, step, &format!("{done}{trust}"), color));
    }
    out.push(String::new());
    if let Some(path) = &c.saved {
        out.push(saved_line(path, c.backed_up.then_some("hooks.json.platoon-bak")));
    }
    match &c.trust {
        Some(Trust::Asked(each)) if each.contains(&HookTrust::Not) => {
            out.push("Codex didn't confirm it trusts every one of them.".into());
            out.push(CODEX_WILL_ASK.into());
        }
        Some(Trust::Asked(each)) if each.contains(&HookTrust::Now) => {
            out.push("Setup had Codex mark them trusted, so they run in your Codex sessions without asking.".into());
        }
        Some(Trust::Asked(_)) => {}
        Some(Trust::Failed(why)) if c.saved.is_some() => {
            out.push(format!("platoon couldn't ask Codex to trust them: {why}."));
            out.push(CODEX_WILL_ASK.into());
        }
        Some(Trust::Failed(why)) => {
            out.push(format!("platoon couldn't ask Codex whether it trusts them: {why}."));
            out.push(
                "If you haven't trusted them yet, Codex asks you once the next time it starts: approve them there; until \
                 you do they don't run (nothing else in platoon waits for them)."
                    .into(),
            );
        }
        None if c.saved.is_some() => out.push(CODEX_WILL_ASK.into()),
        None => {}
    }
    if out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out
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

    fn settings(root: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(root.join("settings.json")).unwrap()).unwrap()
    }

    #[test]
    fn nothing_is_done_at_first_and_everything_is_after_setup() {
        let root = tmp("fresh");
        let before = check(&root);
        assert_eq!(before, (0..STEPS.len()).map(|_| State::Missing).collect::<Vec<_>>());
        assert!(!ready(&before));
        let r = run(&root).unwrap();
        assert_eq!(r.outcomes, (0..STEPS.len()).map(|_| Outcome::Added).collect::<Vec<_>>());
        assert_eq!(check(&root), (0..STEPS.len()).map(|_| State::Done).collect::<Vec<_>>());
        assert!(notes_hook_installed(&root) && stop_hook_installed(&root));
        assert!(ready(&check(&root)) && statusline_installed(&root) && guard_installed(&root));
    }

    #[test]
    fn running_setup_again_marks_everything_done_and_changes_nothing() {
        let root = tmp("again");
        run(&root).unwrap();
        let text = fs::read_to_string(root.join("settings.json")).unwrap();
        let r = run(&root).unwrap();
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
        run(&root).unwrap();
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
        assert!(matches!(check(&root)[0], State::Skipped(_)));
        let r = run(&root).unwrap();
        assert!(matches!(r.outcomes[0], Outcome::Skipped(_)) && r.outcomes[1] == Outcome::Added);
        assert!(fs::read_to_string(root.join("settings.json")).unwrap().contains("my-script"));
        assert!(ready(&check(&root)), "a skipped step must not lock the user out");
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
        let before = check(&root);
        assert!(matches!(before[0], State::Broken(_)) && matches!(before[1], State::Broken(_)));
        assert!(!ready(&before));
        assert!(render_blocked(&before, false).contains("no longer exists"));
        let r = run(&root).unwrap();
        assert_eq!(r.outcomes, vec![Outcome::Repaired, Outcome::Repaired, Outcome::Added, Outcome::Added]);
        assert!(ready(&check(&root)));
        // Repaired in place, not duplicated.
        assert_eq!(settings(&root)["hooks"]["UserPromptSubmit"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn the_blocked_message_lists_every_step_with_its_state() {
        let root = tmp("blocked");
        let shown = render_blocked(&check(&root), false);
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
        run(&root).unwrap();
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
        let states = check(&root);
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
        assert!(matches!(check(&root)[2], State::Broken(_)) && ready(&check(&root)));
        assert_eq!(run(&root).unwrap().outcomes[2], Outcome::Repaired);
        assert_eq!(settings(&root)["hooks"]["SessionStart"].as_array().unwrap().len(), 2);
        // The Stop hook behaves exactly the same way, and is just as optional.
        assert!(remove_stop_hook(&root).unwrap());
        let states = check(&root);
        assert_eq!(states[3], State::Missing);
        assert!(ready(&states), "the Stop hook never blocks starting either");
        assert!(!remove_stop_hook(&root).unwrap());
        assert_eq!(settings(&root)["hooks"]["Stop"].as_array().unwrap().len(), 1, "only ours went");
        assert_eq!(run(&root).unwrap().outcomes[3], Outcome::Added);
        assert!(stop_hook_installed(&root));
    }

    #[test]
    fn invalid_settings_are_reported_not_overwritten() {
        let root = tmp("invalid");
        fs::write(root.join("settings.json"), "{ not json").unwrap();
        assert!(!ready(&check(&root)));
        assert!(run(&root).is_err());
        assert_eq!(fs::read_to_string(root.join("settings.json")).unwrap(), "{ not json");
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

    fn codex_hooks(codex: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(codex.join("hooks.json")).unwrap()).unwrap()
    }

    /// Every command in Codex's hooks for `event`.
    fn codex_commands(v: &Value, event: &str) -> Vec<String> {
        hook_commands(v, event, |_| true).into_iter().map(String::from).collect()
    }

    #[test]
    fn codexs_three_hooks_are_added_once_as_fixed_commands_and_a_second_run_changes_nothing() {
        let codex = tmp("codex-fresh").join("codex");
        let r = run_codex(&codex);
        assert_eq!((r.outcomes, r.error), (vec![Outcome::Added; 3], None));
        assert_eq!(r.saved.as_deref(), Some(codex.join("hooks.json").as_path()));
        assert!(!codex.join("hooks.json.platoon-bak").exists(), "there was nothing to back up");
        let v = codex_hooks(&codex);
        let (exe, _) = this_program().unwrap();
        for (event, args) in codex::HOOKS {
            assert_eq!(codex_commands(&v, event), [format!("{exe} {args}")], "{event}");
        }
        assert_eq!(v["hooks"]["SessionStart"][0]["hooks"][0]["type"], "command");
        let text = fs::read_to_string(codex.join("hooks.json")).unwrap();
        let again = run_codex(&codex);
        assert_eq!((again.outcomes, again.saved), (vec![Outcome::AlreadyDone; 3], None));
        assert_eq!(fs::read_to_string(codex.join("hooks.json")).unwrap(), text, "not written again");
        assert!(!codex.join("hooks.json.platoon-bak").exists());
    }

    #[test]
    fn codexs_hooks_file_keeps_every_key_and_hook_of_the_users_and_a_broken_one_of_ours_is_repaired() {
        let codex = tmp("codex-keep");
        let original = r#"{"version":2,"hooks":{"Stop":[{"hooks":[{"type":"command","command":"my-stop","timeout":5}]}],"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"check"}]}],"SessionStart":[{"hooks":[{"type":"command","command":"/gone/platoon hook session-start --agent codex"}]}]},"other":{"x":[1,2]}}"#;
        fs::write(codex.join("hooks.json"), original).unwrap();
        let r = run_codex(&codex);
        assert_eq!(r.outcomes, [Outcome::Repaired, Outcome::Added, Outcome::Added]);
        assert_eq!(fs::read_to_string(codex.join("hooks.json.platoon-bak")).unwrap(), original, "backed up first");
        let v = codex_hooks(&codex);
        assert_eq!((v["version"].clone(), v["other"].clone()), (json!(2), json!({"x": [1, 2]})));
        assert_eq!(v["hooks"]["PreToolUse"], json!([{"matcher":"Bash","hooks":[{"type":"command","command":"check"}]}]));
        let stops = codex_commands(&v, "Stop");
        assert_eq!(stops[0], "my-stop");
        assert!(stops.len() == 2 && stops[1].ends_with(" hook stop --agent codex"), "{stops:?}");
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["timeout"], 5);
        let starts = codex_commands(&v, "SessionStart");
        assert!(starts.len() == 1 && !starts[0].starts_with("/gone/"), "repaired in place, not doubled: {starts:?}");
        // The keys stay in the order they were in.
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["version", "hooks", "other"]);
    }

    #[test]
    fn a_codex_hooks_file_that_cant_be_parsed_is_reported_and_left_exactly_as_it_was() {
        for (name, bad) in [("codex-bad", "{ not json"), ("codex-list", "[1,2]")] {
            let codex = tmp(name);
            fs::write(codex.join("hooks.json"), bad).unwrap();
            let r = run_codex(&codex);
            assert!(r.outcomes.is_empty() && r.saved.is_none(), "{name}");
            assert!(r.error.as_deref().is_some_and(|e| e.contains("hooks.json")), "{:?}", r.error);
            assert_eq!(fs::read_to_string(codex.join("hooks.json")).unwrap(), bad);
            assert!(!codex.join("hooks.json.platoon-bak").exists());
            let report = Report { outcomes: vec![Outcome::AlreadyDone; 4], saved: None, backed_up: false, notes: vec![], codex: Some(r) };
            let shown = render_report(&report, &codex, false);
            assert!(shown.contains("[–] Codex hooks") && shown.contains("left the file alone"), "{shown}");
        }
    }

    #[test]
    fn the_report_says_what_codex_will_ask_and_a_claude_only_report_is_as_before() {
        let root = tmp("codex-report");
        let claude_only = Report { outcomes: vec![Outcome::AlreadyDone; 4], saved: None, backed_up: false, notes: vec![], codex: None };
        let shown = render_report(&claude_only, &root, false);
        assert!(!shown.contains("Codex") && shown.contains("Nothing to change"), "{shown}");
        let codex = run_codex(&root.join("codex"));
        let shown = render_report(&Report { codex: Some(codex), ..claude_only }, &root, false);
        assert_eq!(shown.matches("[✓]").count(), STEPS.len() + CODEX_STEPS.len(), "{shown}");
        // No hooks.json before: nothing was backed up, so no backup is named (decided 2026-10-23; it used to be).
        let saved = format!("Saved {}.", root.join("codex/hooks.json").display());
        assert!(shown.contains("\nCodex\n") && shown.contains(&saved) && !shown.contains("(backup:"), "{shown}");
        assert!(shown.contains("Codex asks you once to trust these hooks") && shown.contains("Approve them there"), "{shown}");
        assert!(!shown.contains("Nothing to change"), "Codex's file changed: {shown}");
    }

    #[test]
    fn uninstall_takes_out_exactly_platoons_codex_hooks() {
        let mut v = json!({"hooks": {
            "SessionStart": [{"hooks": [{"type": "command", "command": "/opt/x/bin/platoon hook session-start --agent codex"}]}],
            "UserPromptSubmit": [{"hooks": [
                {"type": "command", "command": "mine"},
                {"type": "command", "command": "/opt/x/bin/platoon hook guard --agent codex"}
            ]}],
            "Stop": [{"hooks": [{"type": "command", "command": "/opt/x/bin/platoon hook stop --agent codex"}]},
                     {"hooks": [{"type": "command", "command": "my-wrapper; platoon hook stop --agent codex"}]}],
            "PreToolUse": [{"hooks": [{"type": "command", "command": "/opt/x/bin/platoon hook guard"}]}]
        }, "keep": true});
        let taken = take_out_codex(&mut v);
        assert_eq!(taken.removed, CODEX_ENTRIES);
        assert_eq!(taken.left, ["my-wrapper; platoon hook stop --agent codex"]);
        assert_eq!(
            v,
            json!({"hooks": {
                "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "mine"}]}],
                "Stop": [{"hooks": [{"type": "command", "command": "my-wrapper; platoon hook stop --agent codex"}]}],
                "PreToolUse": [{"hooks": [{"type": "command", "command": "/opt/x/bin/platoon hook guard"}]}]
            }, "keep": true})
        );
        let mut only =
            json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "/opt/x/bin/platoon hook stop --agent codex"}]}]}});
        assert_eq!(take_out_codex(&mut only).removed, [CODEX_ENTRIES[2]]);
        assert_eq!(only, json!({}), "what platoon's hook left empty goes");
    }

    // ---- Codex trusting platoon's hooks, through a stand-in for its app-server ----

    use crate::testkit::{CodexServerFake, codex_hook};
    use std::time::Instant;

    const ENOUGH: Duration = Duration::from_secs(20);

    /// A Codex folder with setup's hooks.json just written in it (the report says so), the home folder sessions start
    /// in, and a stand-in app-server for that Codex folder.
    fn codex_world(name: &str) -> (PathBuf, PathBuf, CodexReport, CodexServerFake) {
        let base = tmp(name);
        let codex = base.join("codex");
        let report = run_codex(&codex);
        assert!(report.saved.is_some());
        let fake = CodexServerFake::new(name, &codex);
        (codex, base.join("home"), report, fake)
    }

    /// The command this setup writes for `args`, as Codex lists it.
    fn ours(args: &str) -> String {
        format!("{} {args}", this_program().unwrap().1)
    }

    /// Codex's list of platoon's three hooks, with these trust states.
    fn our_hooks(codex: &Path, trust: [&str; 3]) -> Vec<Value> {
        let file = codex.join("hooks.json");
        let events = [("sessionStart", "session_start"), ("userPromptSubmit", "user_prompt_submit"), ("stop", "stop")];
        codex::HOOKS
            .iter()
            .zip(events)
            .zip(trust)
            .enumerate()
            .map(|(i, (((_, args), (event, snake)), t))| {
                codex_hook(&format!("{}:{snake}:0:0", file.display()), event, &ours(args), &file, &format!("sha256:h{i}"), t)
            })
            .collect()
    }

    fn listed(hooks: &[Value], cwd: &Path) -> Value {
        json!({"data": [{"cwd": cwd, "hooks": hooks, "warnings": [], "errors": []}]})
    }

    fn trusted_now(hooks: &[Value], how_many: usize) -> Vec<Value> {
        let mut after = hooks.to_vec();
        for h in after.iter_mut().take(how_many) {
            *h.get_mut("trustStatus").unwrap() = json!("trusted");
        }
        after
    }

    #[test]
    fn setup_has_codex_trust_exactly_its_own_new_or_changed_hooks_and_says_so() {
        let (codex, cwd, mut report, fake) = codex_world("codex-trust");
        let file = codex.join("hooks.json");
        let mut before = our_hooks(&codex, ["untrusted", "modified", "trusted"]);
        // Never setup's to trust: the user's own hook in the same file, setup's very command in another file (a
        // project's), and setup's session-start command under another event.
        before.push(codex_hook(&format!("{}:stop:1:0", file.display()), "stop", "my-stop", &file, "sha256:u", "untrusted"));
        let project = cwd.join("repo/.codex/hooks.json");
        let start = ours("hook session-start --agent codex");
        before.push(codex_hook(
            &format!("{}:session_start:0:0", project.display()),
            "sessionStart",
            &start,
            &project,
            "sha256:p",
            "untrusted",
        ));
        before.push(codex_hook(&format!("{}:stop:2:0", file.display()), "stop", &start, &file, "sha256:w", "modified"));
        // Codex's own chatter before the answer, and an answer to another id, are passed over.
        let chatter = format!("{}\n{}", json!({"method": "configWarning", "params": {"summary": "x"}}), json!({"id": 99, "result": {}}));
        fake.answer("hooks/list", Some(1), &format!("{chatter}\n{}", CodexServerFake::reply("result", listed(&before, &cwd))));
        fake.result("hooks/list", Some(2), listed(&trusted_now(&before, 3), &cwd));
        fake.result("config/batchWrite", None, json!({"status": "ok", "version": "sha256:v2", "filePath": codex.join("config.toml")}));

        let trust = trust_codex(&codex, &fake.program, &cwd, ENOUGH);
        assert_eq!(trust, Trust::Asked(vec![HookTrust::Now, HookTrust::Now, HookTrust::Already]));
        let args: Vec<String> = ["app-server", "--listen", "stdio://"].map(String::from).to_vec();
        assert_eq!(fake.started_with(), (args, codex.display().to_string()), "Codex's own folder, by CODEX_HOME");
        assert_eq!(fake.methods(), ["initialize", "initialized", "hooks/list", "config/batchWrite", "hooks/list"]);
        let sent = fake.requests();
        let version = env!("CARGO_PKG_VERSION");
        assert_eq!(sent[0], json!({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "platoon", "version": version}}}));
        assert_eq!(sent[1], json!({"method": "initialized"}));
        assert_eq!(sent[2], json!({"id": 2, "method": "hooks/list", "params": {"cwds": [cwd]}}));
        let mut records = serde_json::Map::new();
        for h in &before[..2] {
            records.insert(h["key"].as_str().unwrap().to_string(), json!({"trusted_hash": h["currentHash"]}));
        }
        let edit = json!({"keyPath": "hooks.state", "value": records, "mergeStrategy": "upsert"});
        assert_eq!(sent[3], json!({"id": 3, "method": "config/batchWrite", "params": {"edits": [edit], "reloadUserConfig": true}}));
        assert_eq!(sent[4]["params"], sent[2]["params"], "asked again after");

        // The report: each step says what Codex says of it, and Codex won't ask.
        report.trust = Some(trust);
        let shown = render_report(
            &Report { outcomes: vec![Outcome::AlreadyDone; 4], saved: None, backed_up: false, notes: vec![], codex: Some(report) },
            &codex,
            false,
        );
        assert_eq!(shown.matches("added; now trusted in Codex").count(), 2, "{shown}");
        assert!(shown.contains("added; Codex trusts it"), "{shown}");
        assert!(shown.contains("Setup had Codex mark them trusted, so they run in your Codex sessions without asking."), "{shown}");
        assert!(!shown.contains("Codex asks you once") && !shown.contains("Nothing to change"), "{shown}");
    }

    #[test]
    fn hooks_codex_already_trusts_are_not_written_again_and_setup_says_nothing_changed() {
        let (codex, cwd, _, fake) = codex_world("codex-trusted");
        fake.result("hooks/list", None, listed(&our_hooks(&codex, ["trusted"; 3]), &cwd));
        let trust = trust_codex(&codex, &fake.program, &cwd, ENOUGH);
        assert_eq!(trust, Trust::Asked(vec![HookTrust::Already; 3]));
        assert_eq!(fake.methods(), ["initialize", "initialized", "hooks/list"], "nothing written");
        let mut again = run_codex(&codex);
        again.trust = Some(trust);
        let shown = render_report(
            &Report { outcomes: vec![Outcome::AlreadyDone; 4], saved: None, backed_up: false, notes: vec![], codex: Some(again) },
            &codex,
            false,
        );
        assert_eq!(shown.matches("already done; Codex trusts it").count(), 3, "{shown}");
        assert!(shown.contains("Nothing to change.") && !shown.contains("Codex asks you once") && !shown.contains("Saved"), "{shown}");
    }

    #[test]
    fn when_codex_cant_be_asked_or_doesnt_trust_them_setup_says_codex_will_ask_once() {
        type Arrange = fn(&CodexServerFake, &Path, &Path);
        let short = Duration::from_millis(500);
        let cases: [(&str, Arrange, Duration, &str); 7] = [
            ("gone", |f, _, _| fs::remove_file(f.dir.join("codex")).unwrap(), ENOUGH, "isn't there"),
            ("exits", |f, _, _| fs::write(f.dir.join("exit-at-once"), "").unwrap(), ENOUGH, "Codex stopped before it answered"),
            ("silent", |_, _, _| {}, short, "Codex didn't answer within"),
            ("garbage", |f, _, _| f.answer("hooks/list", None, "Welcome to Codex!\n{\"id\":"), short, "Codex didn't answer within"),
            ("strange", |f, _, _| f.result("hooks/list", None, json!({"nope": 1})), ENOUGH, "something platoon doesn't understand"),
            (
                "refuses",
                |f, codex, cwd| {
                    f.result("hooks/list", None, listed(&our_hooks(codex, ["untrusted"; 3]), cwd));
                    f.answer("config/batchWrite", None, &CodexServerFake::reply("error", json!({"message": "config.toml is read-only"})));
                },
                ENOUGH,
                "Codex refused: config.toml is read-only",
            ),
            (
                "elsewhere",
                |f, _, _| f.result("initialize", None, json!({"codexHome": "/somewhere/else", "userAgent": "x"})),
                ENOUGH,
                "Codex is using another folder (/somewhere/else)",
            ),
        ];
        for (name, arrange, timeout, want) in cases {
            let (codex, cwd, mut report, fake) = codex_world(&format!("codex-fail-{name}"));
            arrange(&fake, &codex, &cwd);
            let started = Instant::now();
            let trust = trust_codex(&codex, &fake.program, &cwd, timeout);
            assert!(started.elapsed() < timeout + Duration::from_secs(6), "{name}: {:?}", started.elapsed());
            assert!(matches!(&trust, Trust::Failed(why) if why.contains(want)), "{name}: {trust:?}");
            assert!(!fake.methods().contains(&"config/batchWrite".to_string()) || name == "refuses", "{name}");
            if let Ok(pid) = fs::read_to_string(fake.dir.join("pid")) {
                assert!(!crate::procs::pid_alive(pid.trim().parse().unwrap()), "{name}: the stand-in was left running");
            }
            report.trust = Some(trust);
            let shown = render_codex(&report, false).join("\n");
            assert!(shown.contains("platoon couldn't ask Codex to trust them: ") && shown.contains(want), "{name}: {shown}");
            assert!(shown.contains(CODEX_WILL_ASK), "{name}: {shown}");
        }
        // Asked, but Codex still doesn't say it trusts them.
        let (codex, cwd, mut report, fake) = codex_world("codex-fail-untrusted");
        fake.result("hooks/list", None, listed(&our_hooks(&codex, ["untrusted"; 3]), &cwd));
        fake.result("config/batchWrite", None, json!({"status": "ok"}));
        let trust = trust_codex(&codex, &fake.program, &cwd, ENOUGH);
        assert_eq!(trust, Trust::Asked(vec![HookTrust::Not; 3]));
        report.trust = Some(trust);
        let shown = render_codex(&report, false).join("\n");
        assert_eq!(shown.matches("added; not trusted in Codex yet").count(), 3, "{shown}");
        assert!(shown.contains("Codex didn't confirm it trusts every one of them.") && shown.contains(CODEX_WILL_ASK), "{shown}");
        // Codex couldn't be asked on a later run that changed nothing: said without claiming they are untrusted.
        let again = CodexReport {
            outcomes: vec![Outcome::AlreadyDone; 3],
            saved: None,
            backed_up: false,
            error: None,
            trust: Some(Trust::Failed("x".into())),
        };
        let shown = render_codex(&again, false).join("\n");
        assert!(
            shown.contains("couldn't ask Codex whether it trusts them: x.") && shown.contains("If you haven't trusted them yet"),
            "{shown}"
        );
    }

    #[test]
    fn saved_names_a_backup_only_when_there_was_a_file_to_back_up() {
        let root = tmp("backup-none");
        let r = run(&root).unwrap();
        assert!(!r.backed_up && !root.join("settings.json.platoon-bak").exists());
        let shown = render_report(&r, &root, false);
        assert!(shown.contains(&format!("Saved {}.\n", root.join("settings.json").display())) && !shown.contains("(backup:"), "{shown}");
        let root = tmp("backup-some");
        fs::write(root.join("settings.json"), "{}").unwrap();
        let r = run(&root).unwrap();
        assert!(r.backed_up && root.join("settings.json.platoon-bak").exists());
        let shown = render_report(&r, &root, false);
        assert!(shown.contains(&format!("Saved {} (backup: settings.json.platoon-bak).", root.join("settings.json").display())), "{shown}");
        // Codex's hooks.json the same way.
        let codex = tmp("backup-codex-none").join("codex");
        let c = run_codex(&codex);
        let shown = render_codex(&c, false).join("\n");
        assert!(!c.backed_up && shown.contains(&format!("Saved {}.", codex.join("hooks.json").display())), "{shown}");
        assert!(!shown.contains("(backup:"), "{shown}");
        let codex = tmp("backup-codex-some");
        fs::write(codex.join("hooks.json"), "{}").unwrap();
        let c = run_codex(&codex);
        let shown = render_codex(&c, false).join("\n");
        assert!(
            c.backed_up && shown.contains(&format!("Saved {} (backup: hooks.json.platoon-bak).", codex.join("hooks.json").display())),
            "{shown}"
        );
    }
}
