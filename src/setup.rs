//! `platoon setup`: the one-time changes platoon makes to Claude Code's settings, as a checklist.
//!
//! Every step's status is computed from the settings file itself, so "done" is always a fact about the
//! files as they are now: `platoon` refuses to start until every step is done, running `setup` again
//! is harmless, and it repairs a step whose command points at a platoon that no longer exists.

use serde_json::{json, Value};
use std::{fs, path::{Path, PathBuf}};

pub struct Step {
    pub name: &'static str,
    pub what: &'static str,
}

/// Same order as `check` and `Report::outcomes`.
pub const STEPS: [Step; 2] = [
    Step { name: "Status line", what: "plan usage, and Claude's own context % in the list" },
    Step { name: "Session guard", what: "stops two copies of a session overwriting each other" },
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

#[derive(Debug, PartialEq, Eq)]
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
    if Path::new(prog).is_absolute() && !Path::new(prog).exists() {
        State::Broken(format!("{prog} no longer exists"))
    } else {
        State::Done
    }
}

fn is_statusline(c: &str) -> bool {
    c.contains("platoon") && c.contains("statusline")
}

fn is_guard(c: &str) -> bool {
    c.contains("platoon") && c.contains("hook guard")
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
        Some(other) => State::Skipped(format!("you have your own status line ({other}); make it also run `platoon statusline` to see plan usage")),
        None => State::Missing,
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

/// Where each step stands, in `STEPS` order.
pub fn check(root: &Path) -> Vec<State> {
    let v = read_settings(root).unwrap_or(Value::Null);
    vec![check_statusline(&v), check_guard(&v)]
}

pub fn ready(states: &[State]) -> bool {
    states.iter().all(State::satisfied)
}

pub fn statusline_installed(root: &Path) -> bool {
    read_settings(root).is_some_and(|v| check_statusline(&v) == State::Done)
}

/// True when the guard hook is present and working, i.e. later copies of a running session get refused.
pub fn guard_installed(root: &Path) -> bool {
    read_settings(root).is_some_and(|v| check_guard(&v) == State::Done)
}

fn remove_guard(v: &mut Value) {
    // `get_mut`, not indexing: mutable indexing on a serde_json Value creates the missing keys as null.
    let groups = v.get_mut("hooks").and_then(|h| h.get_mut("UserPromptSubmit")).and_then(Value::as_array_mut);
    if let Some(groups) = groups {
        for g in groups.iter_mut() {
            if let Some(hs) = g["hooks"].as_array_mut() {
                hs.retain(|h| !h["command"].as_str().is_some_and(is_guard));
            }
        }
        groups.retain(|g| g["hooks"].as_array().is_none_or(|hs| !hs.is_empty()));
    }
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
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.to_string_lossy().into_owned();
    let quoted = if exe.contains(' ') { format!("'{exe}'") } else { exe.clone() };
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
            notes.push("That's a build directory path. Run `cargo install --path .` and then `platoon setup` again for a stable one.".to_string());
        }
    }
    Ok(Report { outcomes, saved, notes })
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
        out.push(format!("Saved {} (backup: settings.json.platoon-bak).", root.join("settings.json").display()));
        out.push("Claude Code picks up hook changes live. Restart running sessions for the status line.".into());
    } else {
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
        let d = std::env::temp_dir().join(format!("platoon-setup-{name}-{}", std::process::id()));
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
        assert_eq!(before, vec![State::Missing, State::Missing]);
        assert!(!ready(&before));
        let r = run(&root).unwrap();
        assert_eq!(r.outcomes, vec![Outcome::Added, Outcome::Added]);
        assert_eq!(check(&root), vec![State::Done, State::Done]);
        assert!(ready(&check(&root)) && statusline_installed(&root) && guard_installed(&root));
    }

    #[test]
    fn running_setup_again_marks_everything_done_and_changes_nothing() {
        let root = tmp("again");
        run(&root).unwrap();
        let text = fs::read_to_string(root.join("settings.json")).unwrap();
        let r = run(&root).unwrap();
        assert_eq!(r.outcomes, vec![Outcome::AlreadyDone, Outcome::AlreadyDone]);
        assert!(r.saved.is_none());
        assert_eq!(fs::read_to_string(root.join("settings.json")).unwrap(), text);
        let shown = render_report(&r, &root, false);
        assert_eq!(shown.matches("[✓]").count(), 2, "{shown}");
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
        assert_eq!(r.outcomes, vec![Outcome::Repaired, Outcome::Repaired]);
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
    }

    #[test]
    fn invalid_settings_are_reported_not_overwritten() {
        let root = tmp("invalid");
        fs::write(root.join("settings.json"), "{ not json").unwrap();
        assert!(!ready(&check(&root)));
        assert!(run(&root).is_err());
        assert_eq!(fs::read_to_string(root.join("settings.json")).unwrap(), "{ not json");
    }
}
