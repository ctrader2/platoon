//! `platoon uninstall`: takes platoon back out of Claude Code and off the computer, all but the program itself (the
//! way it was installed decides how that goes: `scripts/uninstall.sh` does it, or the last line says how).
//!
//! What it does, in this order, so a "no" leaves everything as it was:
//! 1. Look: platoon's entries in Claude Code's `settings.json` (`setup::take_out`, exactly what setup writes), the
//!    sessions platoon's hosts are running, the worktrees tasks recorded, and the folders.
//! 2. Ask: whether to stop the running sessions (no: stop here, nothing changed; `--stop-sessions` says yes), then
//!    whether to delete the config folder (the user's projects, notes, tasks and settings: default keep; `--all`
//!    deletes it without asking). Answers come from the terminal (`/dev/tty`, so `curl ... | sh` can still ask); with
//!    no terminal the sessions question is a no and the config folder is kept.
//! 3. Act: stop the sessions (SIGTERM to the `claude` each host runs, as the app's own stop does), take the entries out
//!    of settings.json (backup first, as setup does), delete the cache folder, the config folder when asked, and
//!    older platoons' leftovers.
//! 4. Say what was removed and what was kept, with the git commands for the task worktrees (never deleted).
//!
//! Claude Code's conversations are never touched.

use crate::agents::claude;
use crate::{setup, tickets, versions};
use serde_json::Value;
use std::{
    fs,
    io::{self, BufRead, Write},
    path::{Component, Path, PathBuf},
};

pub const USAGE: &str = "usage: platoon uninstall [--all] [--stop-sessions]
  Takes platoon out of Claude Code (its status line and hooks), deletes its cache folder, and
  asks before deleting your projects, notes, tasks and settings. Claude Code's
  conversations are never touched. The program itself is removed the way you installed it.
  --all             also delete your projects, notes, tasks and settings, without asking
  --stop-sessions   stop the sessions platoon is running without asking (otherwise it asks, and
                    without a terminal it stops and changes nothing)";

/// Where the guide says all of this.
pub const GUIDE: &str = "https://github.com/ctrader2/platoon/blob/main/guide/install-and-setup.md#uninstall";

/// Set by `scripts/uninstall.sh` when it runs this: it removes the program itself afterwards, so the last line about
/// that is not said.
pub const FROM_SCRIPT_ENV: &str = "PLATOON_UNINSTALL_FROM_SCRIPT";
/// For tests: the file answers are read from instead of `/dev/tty`.
pub const TTY_ENV: &str = "PLATOON_UNINSTALL_TTY";
/// For tests: every folder uninstall may touch must be inside this one, or it refuses to run.
pub const TEST_HOME_ENV: &str = "PLATOON_UNINSTALL_TEST_HOME";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Options {
    pub all: bool,
    pub stop_sessions: bool,
}

/// The flags, or the message to print (Ok(None): `--help`).
pub fn parse_args(args: &[String]) -> Result<Option<Options>, String> {
    let mut o = Options::default();
    for a in args {
        match a.as_str() {
            "--all" => o.all = true,
            "--stop-sessions" => o.stop_sessions = true,
            "-h" | "--help" => return Ok(None),
            other => return Err(format!("platoon uninstall: don't know \"{other}\"\n{USAGE}")),
        }
    }
    Ok(Some(o))
}

/// Every place uninstall looks at.
#[derive(Debug, Clone)]
pub struct Places {
    /// Claude Code's folder (`CLAUDE_CONFIG_DIR`, else `~/.claude`).
    pub root: PathBuf,
    pub config: PathBuf,
    pub cache: PathBuf,
    /// Older platoons' folders, removed when they are there: (folder, what it was).
    pub leftovers: Vec<(PathBuf, &'static str)>,
    /// The home folder: nothing at or above it is ever deleted.
    pub home: PathBuf,
}

impl Places {
    /// The real ones, from the environment (not for unit tests: `claude_root`, `config_dir` and `cache_dir` panic there).
    pub fn from_env() -> Self {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let set = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let data = set("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share"));
        let mut leftovers =
            vec![(data.join("platoon"), "an older platoon's data folder"), (data.join("fonts/platoon"), "an older platoon's font")];
        if cfg!(target_os = "macos") {
            leftovers.push((home.join("Library/Application Support/platoon"), "an older platoon's data folder"));
        }
        Self { root: claude::claude_root(), config: crate::archive::config_dir(), cache: crate::usage::cache_dir(), leftovers, home }
    }

    fn all(&self) -> Vec<&Path> {
        let mut v = vec![self.root.as_path(), &self.config, &self.cache, &self.home];
        v.extend(self.leftovers.iter().map(|(p, _)| p.as_path()));
        v
    }
}

/// The belt-and-braces check for tests (`PLATOON_UNINSTALL_TEST_HOME`): every place must be inside it.
pub fn check_test_home(places: &Places, test_home: Option<&Path>) -> Result<(), String> {
    let Some(t) = test_home else { return Ok(()) };
    match places.all().into_iter().find(|p| !p.is_absolute() || !p.starts_with(t) || p.components().any(|c| c == Component::ParentDir)) {
        Some(p) => Err(format!("{TEST_HOME_ENV} is set to {}, but {} is outside it: refusing to run", t.display(), p.display())),
        None => Ok(()),
    }
}

/// A folder that may be deleted: absolute, plain, and neither the home folder, nor above it, nor Claude Code's folder or
/// above it. Err: why it is left alone.
fn deletable(dir: &Path, places: &Places) -> Result<(), String> {
    if !dir.is_absolute() || dir.components().any(|c| matches!(c, Component::ParentDir | Component::CurDir)) {
        return Err(format!("{} isn't a plain absolute path", dir.display()));
    }
    if dir.components().count() < 3 {
        return Err(format!("{} is too near the top of the disk", dir.display()));
    }
    for (other, what) in [(&places.home, "your home folder"), (&places.root, "Claude Code's folder")] {
        if !other.as_os_str().is_empty() && other.starts_with(dir) {
            return Err(format!("{} is {what} or holds it", dir.display()));
        }
    }
    Ok(())
}

// ---- looking --------------------------------------------------------------------------------------------------------

/// A session one of platoon's hosts is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub claude_pid: u32,
    pub dir: String,
    /// The conversation's id, when it is known.
    pub id: Option<String>,
    /// A name the user gave it, when Claude Code's registry has one.
    pub name: Option<String>,
    /// It only shows a background session (`claude attach <id>`): stopping it leaves that session running.
    pub attach: bool,
}

/// The sessions platoon's hosts run (`<cache>/hosts/*.json` whose `claude` is alive). Only reads: nothing is cleaned up
/// or made, so a "no" later leaves the folder as it was.
pub fn running_sessions(cache: &Path, root: &Path) -> Vec<Running> {
    let live = claude::Reader::with_root(root.to_path_buf()).live();
    let mut out = Vec::new();
    for e in fs::read_dir(cache.join("hosts")).into_iter().flatten().flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Some(v) = fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) else { continue };
        let Some(pid) = v.get("claude_pid").and_then(Value::as_u64).and_then(|p| u32::try_from(p).ok()) else { continue };
        if pid <= 1 || !crate::procs::pid_alive(pid) {
            continue;
        }
        let args: Vec<String> = v
            .get("args")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let entry = live.iter().find(|l| l.pid == pid);
        // A host runs Claude Code (`host::HostInfo::agent`).
        let agent = crate::agents::Agent::Claude;
        let attach = agent.attaches(&args);
        let id = if attach { args.get(1).cloned() } else { entry.map(|l| l.id.clone()).or_else(|| agent.session_of_args(&args)) };
        out.push(Running {
            claude_pid: pid,
            dir: v.get("dir").and_then(Value::as_str).unwrap_or("").to_string(),
            id,
            name: entry.and_then(|l| l.name.clone()),
            attach,
        });
    }
    out.sort_by_key(|r| r.claude_pid);
    out
}

/// A task's worktree or branch that is still there: listed with the git commands that remove it, never deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptWorktree {
    pub project: String,
    pub number: u32,
    /// The task's folder in the repository (where `git -C` runs).
    pub folder: String,
    /// The worktree's folder, when it is still there.
    pub dir: Option<PathBuf>,
    pub branch: String,
}

impl KeptWorktree {
    pub fn commands(&self) -> Vec<String> {
        let mut c = Vec::new();
        if let Some(d) = &self.dir {
            c.push(format!("git -C {} worktree remove {}", sh_quote(&self.folder), sh_quote(&d.to_string_lossy())));
        }
        c.push(format!("git -C {} branch -D {}", sh_quote(&self.folder), sh_quote(&self.branch)));
        c
    }
}

/// The worktree folder platoon made for task `number` that `path` (the session's place inside it) is in:
/// `<parent>/<repo>-worktrees/task-<N>` (`worktrees::worktree_dir`).
fn worktree_root(path: &Path, number: u32) -> Option<PathBuf> {
    let want = format!("task-{number}");
    path.ancestors()
        .find(|a| {
            a.file_name().is_some_and(|n| n.to_string_lossy() == want)
                && a.parent().and_then(Path::file_name).is_some_and(|p| p.to_string_lossy().ends_with("-worktrees"))
        })
        .map(Path::to_path_buf)
}

/// The worktrees and branches the tasks in `config` recorded that are still there (git is asked, read-only, whether
/// a branch is; when it can't say, the branch is listed).
pub fn kept_worktrees(config: &Path, git: &str) -> Vec<KeptWorktree> {
    let mut out: Vec<KeptWorktree> = Vec::new();
    for (project, t) in tickets::every_ticket(config) {
        let (Some(w), Some(folder)) = (&t.worktree, &t.folder) else { continue };
        if !Path::new(folder).is_dir() || w.branch.is_empty() {
            continue;
        }
        let dir = worktree_root(Path::new(&w.path), t.number).filter(|d| d.is_dir());
        let branch_there = || {
            crate::worktrees::git(git, Path::new(folder), &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{}", w.branch)])
                .map(|_| true)
                // No git to ask: listed, in case it is there.
                .unwrap_or_else(|e| e.starts_with("couldn't run git"))
        };
        if dir.is_none() && !branch_there() {
            continue;
        }
        out.push(KeptWorktree { project: project.clone(), number: t.number, folder: folder.clone(), dir, branch: w.branch.clone() });
    }
    out
}

// ---- the plan, asked, then carried out --------------------------------------------------------------------------------

/// What there is to do, found before anything is asked or changed.
pub struct Plan {
    settings: Option<(PathBuf, Value, setup::TakenOut)>,
    settings_error: Option<String>,
    pub running: Vec<Running>,
    worktrees: Vec<KeptWorktree>,
    config_there: bool,
}

pub fn look(places: &Places, git: &str) -> Plan {
    let path = places.root.join("settings.json");
    let (mut settings, mut settings_error) = (None, None);
    match fs::read_to_string(&path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => settings_error = Some(format!("couldn't read {}: {e}", path.display())),
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(mut v) if v.is_object() => {
                let taken = setup::take_out(&mut v);
                settings = Some((path, v, taken));
            }
            Ok(_) => settings_error = Some(format!("{} isn't a JSON object", path.display())),
            Err(e) => settings_error = Some(format!("{} isn't valid JSON ({e})", path.display())),
        },
    }
    Plan {
        settings,
        settings_error,
        running: running_sessions(&places.cache, &places.root),
        worktrees: kept_worktrees(&places.config, git),
        config_there: places.config.exists(),
    }
}

/// How to stop the sessions: the app's own way (SIGTERM, wait for them to exit). A parameter so tests can see it.
pub type Stopper<'a> = &'a mut dyn FnMut(&[u32]) -> Result<(), String>;

/// Read one answer: Some(true) for a clear yes, Some(false) for anything else, None when the input ended.
fn answer(input: &mut dyn BufRead) -> Option<bool> {
    let mut line = String::new();
    match input.read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(matches!(line.trim().to_lowercase().as_str(), "y" | "yes")),
    }
}

fn session_line(r: &Running) -> String {
    let what = match (&r.name, r.attach) {
        (_, true) => "a background session, shown".to_string(),
        (Some(n), _) => format!("\"{n}\""),
        (None, _) => "a session".to_string(),
    };
    let dir = if r.dir.is_empty() { String::new() } else { format!(" in {}", r.dir) };
    let again = match (&r.id, r.attach) {
        (Some(id), true) => format!(": `claude attach {id}` shows it again"),
        (Some(id), false) => format!(": `claude --resume {id}` opens it again"),
        (None, _) => String::new(),
    };
    format!("  - {what}{dir} (pid {}){again}", r.claude_pid)
}

/// Single-quote for a shell when needed.
pub fn sh_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+:@%=,".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// How the program itself goes, for the last line (not said when the uninstall script runs this).
pub fn program_line(install: &versions::Install, exe: &Path) -> String {
    let exe = sh_quote(&exe.to_string_lossy());
    match install {
        versions::Install::Homebrew => "Last step, remove the program: brew uninstall platoon".into(),
        versions::Install::Cargo => "Last step, remove the program: cargo uninstall platoon-tui".into(),
        versions::Install::Script => format!(
            "Last step, remove the program: rm {exe}, and the install script's folder ~/.config/platoon-tui (or run the uninstall script, which does both: see {GUIDE})"
        ),
        versions::Install::Other => {
            format!("Last step, remove the program: rm {exe} (with Nix, take it out of your profile or configuration instead)")
        }
    }
}

/// The whole of `platoon uninstall` once the places are known. `input`: the terminal's answers, None with no terminal.
/// `program`: the last line about removing the program itself (None: the script does that). Returns the exit code:
/// 0 done, 1 stopped or something couldn't be removed.
pub fn run(
    places: &Places,
    opts: &Options,
    input: Option<&mut dyn BufRead>,
    out: &mut dyn Write,
    stop: Stopper,
    git: &str,
    program: Option<String>,
) -> i32 {
    let plan = look(places, git);
    let _ = writeln!(out, "platoon uninstall\n");
    if let Some(e) = &plan.settings_error {
        let _ = writeln!(
            out,
            "Claude Code's settings can't be read: {e}.\nNothing was changed. Fix the file, or take platoon's entries out by hand ({GUIDE}), then run this again."
        );
        return 1;
    }
    let mut input = input;

    // ---- ask everything first ----
    let interactive = input.is_some();
    if !plan.running.is_empty() {
        let n = plan.running.len();
        let _ = writeln!(out, "platoon is running {}:", if n == 1 { "1 session".to_string() } else { format!("{n} sessions") });
        for r in &plan.running {
            let _ = writeln!(out, "{}", session_line(r));
        }
        let _ = writeln!(out, "Their conversations are saved: `claude --resume <id>` opens them again.");
        let yes = if opts.stop_sessions {
            let _ = writeln!(out, "Stopping them (--stop-sessions).");
            true
        } else if let Some(i) = input.as_deref_mut() {
            let _ = write!(out, "Stop {} and go on? [y/N] ", if n == 1 { "it" } else { "them" });
            let _ = out.flush();
            let a = answer(i);
            if a.is_none() {
                let _ = writeln!(out);
            }
            a == Some(true)
        } else {
            let _ = writeln!(out, "There is no terminal to ask in, so platoon left them running and changed nothing.");
            let _ = writeln!(out, "Run this again in a terminal, or add --stop-sessions to stop them without asking.");
            return 1;
        };
        if !yes {
            let _ = writeln!(out, "Left them running. Nothing was changed.");
            return 1;
        }
    }
    let delete_config = if !plan.config_there {
        false
    } else if opts.all {
        true
    } else if let Some(i) = input {
        let _ = writeln!(out, "Your projects, notes, tasks, squads and settings are in {}.", places.config.display());
        let _ = write!(out, "Also delete your projects, notes, tasks and settings? [y/N] ");
        let _ = out.flush();
        let a = answer(i);
        if a.is_none() {
            let _ = writeln!(out);
        }
        a == Some(true)
    } else {
        false
    };
    let _ = writeln!(out);

    // ---- then act ----
    let mut removed: Vec<String> = Vec::new();
    let mut kept: Vec<String> = Vec::new();
    let mut failed: Vec<String> = Vec::new();

    if !plan.running.is_empty() {
        let pids: Vec<u32> = plan.running.iter().map(|r| r.claude_pid).collect();
        match stop(&pids) {
            Ok(()) => removed.push(format!(
                "{} platoon was running: stopped (their conversations are saved)",
                if pids.len() == 1 { "the session".to_string() } else { format!("the {} sessions", pids.len()) }
            )),
            Err(e) => {
                let _ = writeln!(out, "Couldn't stop the sessions ({e}). Nothing else was changed: run this again in a moment.");
                return 1;
            }
        }
    }

    if let Some((path, v, taken)) = &plan.settings {
        if !taken.removed.is_empty() {
            let backup = places.root.join("settings.json.platoon-bak");
            let written = fs::copy(path, &backup)
                .map_err(|e| format!("couldn't back up {}: {e}", path.display()))
                .and_then(|_| serde_json::to_string_pretty(v).map_err(|e| e.to_string()))
                .and_then(|text| {
                    crate::projects::write_atomic(path, &(text + "\n")).map_err(|e| format!("couldn't write {}: {e}", path.display()))
                });
            match written {
                Ok(()) => removed.push(format!(
                    "from Claude Code's settings ({}): {}. The file as it was is kept as settings.json.platoon-bak",
                    path.display(),
                    taken.removed.join(", ")
                )),
                Err(e) => failed.push(e),
            }
        }
        for c in &taken.left {
            kept.push(format!("in Claude Code's settings, your own command that runs platoon: {c} (take the platoon part out by hand)"));
        }
    }

    let remove_dir = |dir: &Path, what: &str, removed: &mut Vec<String>, failed: &mut Vec<String>| -> bool {
        if !dir.exists() {
            return false;
        }
        if let Err(why) = deletable(dir, places) {
            failed.push(format!("{what} ({}) was left alone: {why}", dir.display()));
            return false;
        }
        match fs::remove_dir_all(dir) {
            Ok(()) => {
                removed.push(format!("{what} ({})", dir.display()));
                true
            }
            Err(e) => {
                failed.push(format!("couldn't delete {what} ({}): {e}", dir.display()));
                false
            }
        }
    };
    remove_dir(&places.cache, "platoon's cache folder", &mut removed, &mut failed);
    if delete_config {
        remove_dir(&places.config, "your projects, notes, tasks and settings", &mut removed, &mut failed);
    } else if plan.config_there {
        let why = if interactive || opts.all { "" } else { " (there was no terminal to ask in; --all deletes it)" };
        kept.push(format!(
            "your projects, notes, tasks and settings in {}{why}. To delete them later: rm -rf {}",
            places.config.display(),
            sh_quote(&places.config.to_string_lossy())
        ));
    }
    let mut fonts_gone = false;
    for (dir, what) in &places.leftovers {
        if remove_dir(dir, what, &mut removed, &mut failed) && dir.ends_with("fonts/platoon") {
            fonts_gone = true;
        }
    }
    if fonts_gone {
        // Let the system forget the font; nothing is said when there is no fc-cache.
        let _ = std::process::Command::new("fc-cache")
            .arg("-f")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    kept.push("your conversations: they are Claude Code's, and `claude --resume` opens any of them".into());

    // ---- say what happened ----
    if removed.is_empty() {
        let _ = writeln!(out, "Nothing of platoon's was left to remove.");
    } else {
        let _ = writeln!(out, "Removed:");
        for r in &removed {
            let _ = writeln!(out, "  - {r}");
        }
    }
    if !failed.is_empty() {
        let _ = writeln!(out, "Couldn't remove:");
        for f in &failed {
            let _ = writeln!(out, "  - {f}");
        }
    }
    let _ = writeln!(out, "Kept:");
    for k in &kept {
        let _ = writeln!(out, "  - {k}");
    }
    if !plan.worktrees.is_empty() {
        let _ = writeln!(out, "Task worktrees and branches (never deleted: remove each once you no longer need its work):");
        for w in &plan.worktrees {
            let _ = writeln!(out, "  #{} ({}):", w.number, w.project);
            for c in w.commands() {
                let _ = writeln!(out, "    {c}");
            }
        }
    }
    if failed.is_empty() {
        if let Some(p) = program {
            let _ = writeln!(out, "{p}");
        }
        0
    } else {
        let _ = writeln!(out, "Fix what couldn't be removed and run this again: what is gone stays gone.");
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::process::{Command, Stdio};

    fn tmp(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-uninstall-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn places(base: &Path) -> Places {
        let home = base.join("home");
        fs::create_dir_all(&home).unwrap();
        Places {
            root: home.join(".claude"),
            config: home.join(".config/platoon"),
            cache: home.join(".cache/platoon"),
            leftovers: vec![(home.join(".local/share/platoon"), "an older platoon's data folder")],
            home,
        }
    }

    fn no_stop() -> impl FnMut(&[u32]) -> Result<(), String> {
        |_: &[u32]| panic!("nothing should be stopped")
    }

    /// Runs it with `typed` as the terminal's answers (None: no terminal).
    fn uninstall(p: &Places, opts: &Options, typed: Option<&str>, stop: Stopper) -> (i32, String) {
        let mut out = Vec::new();
        let mut cursor = typed.map(|t| io::Cursor::new(t.as_bytes().to_vec()));
        let code = run(p, opts, cursor.as_mut().map(|c| c as &mut dyn BufRead), &mut out, stop, "git", None);
        (code, String::from_utf8(out).unwrap())
    }

    fn settings(p: &Places) -> Value {
        serde_json::from_str(&fs::read_to_string(p.root.join("settings.json")).unwrap()).unwrap()
    }

    /// What setup writes, beside the user's own entries, as setup writes it (a unit test's own program isn't called
    /// `platoon`, so the commands name an installed one; `tests/uninstall.rs` runs the real setup and uninstall).
    fn set_up(p: &Places, user: Value) {
        fs::create_dir_all(&p.root).unwrap();
        let mut v = user;
        let o = v.as_object_mut().unwrap();
        o.insert("statusLine".into(), json!({"type": "command", "command": "/opt/x/bin/platoon statusline"}));
        let hooks = o.entry("hooks").or_insert_with(|| json!({})).as_object_mut().unwrap();
        for (event, args) in [("UserPromptSubmit", "hook guard"), ("SessionStart", "hook session-start"), ("Stop", "hook stop")] {
            let list = hooks.entry(event).or_insert_with(|| json!([])).as_array_mut().unwrap();
            list.push(json!({"hooks": [{"type": "command", "command": format!("/opt/x/bin/platoon {args}")}]}));
        }
        fs::write(p.root.join("settings.json"), serde_json::to_string_pretty(&v).unwrap() + "\n").unwrap();
        // setup knows each one as its own (pointing at a platoon that isn't on this computer).
        assert!(setup::check(&p.root).iter().all(|s| matches!(s, setup::State::Broken(_))));
    }

    #[test]
    fn only_platoons_entries_go_and_the_users_own_stay_in_their_place() {
        let p = places(&tmp("only-ours"));
        let user = json!({
            "model": "opus",
            "hooks": {
                "Stop": [{"hooks": [{"type": "command", "command": "my-stop"}]}],
                "UserPromptSubmit": [{"matcher": "", "hooks": [{"type": "command", "command": "mine"}]}],
                "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "check"}]}]
            },
            "permissions": {"allow": ["Bash(ls)"]}
        });
        set_up(&p, user.clone());
        let before = fs::read_to_string(p.root.join("settings.json")).unwrap();
        assert!(before.contains("hook guard") && before.contains("statusline"));
        let (code, said) = uninstall(&p, &Options::default(), Some("n\n"), &mut no_stop());
        assert_eq!(code, 0, "{said}");
        // Exactly the user's file again: same keys, same order, nothing left of platoon's.
        assert_eq!(settings(&p), user, "{said}");
        let keys: Vec<String> = settings(&p).as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["model", "hooks", "permissions"]);
        assert_eq!(fs::read_to_string(p.root.join("settings.json.platoon-bak")).unwrap(), before, "the backup is the file as it was");
        assert!(fs::read_to_string(p.root.join("settings.json")).unwrap().ends_with("}\n"));
        for want in ["the status line", "the session guard hook", "the project notes hook", "the notes updates hook", "Removed:"] {
            assert!(said.contains(want), "{want}: {said}");
        }
    }

    #[test]
    fn events_and_hooks_left_empty_by_platoon_go_but_ones_the_user_left_empty_stay() {
        let p = places(&tmp("empties"));
        set_up(&p, json!({"theme": "dark"}));
        uninstall(&p, &Options::default(), None, &mut no_stop());
        assert_eq!(settings(&p), json!({"theme": "dark"}), "platoon made `hooks`, so it goes with its last entry");
        // An event of the user's that was already empty, and an empty group of theirs, stay.
        let p = places(&tmp("empties-user"));
        set_up(&p, json!({"hooks": {"Notification": [], "Stop": [{"hooks": []}]}}));
        uninstall(&p, &Options::default(), None, &mut no_stop());
        assert_eq!(settings(&p), json!({"hooks": {"Notification": [], "Stop": [{"hooks": []}]}}));
    }

    #[test]
    fn a_hook_group_shared_with_the_users_own_command_keeps_theirs() {
        let p = places(&tmp("shared-group"));
        fs::create_dir_all(&p.root).unwrap();
        let v = json!({"hooks": {"Stop": [{"hooks": [
            {"type": "command", "command": "/opt/x/platoon hook stop"},
            {"type": "command", "command": "notify-send done"}
        ]}]}});
        fs::write(p.root.join("settings.json"), v.to_string()).unwrap();
        uninstall(&p, &Options::default(), None, &mut no_stop());
        assert_eq!(settings(&p), json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "notify-send done"}]}]}}));
    }

    #[test]
    fn a_status_line_of_the_users_own_that_also_runs_platoon_is_kept_and_said() {
        let p = places(&tmp("own-statusline"));
        fs::create_dir_all(&p.root).unwrap();
        let v = json!({
            "statusLine": {"type": "command", "command": "sh -c 'my-line; platoon statusline'"},
            "hooks": {"UserPromptSubmit": [{"hooks": [{"type": "command", "command": "'/my dir/platoon' hook guard"}]}],
                      "SessionStart": [{"hooks": [{"type": "command", "command": "platoon hook session-start --verbose"}]}]}
        });
        fs::write(p.root.join("settings.json"), v.to_string()).unwrap();
        let (code, said) = uninstall(&p, &Options::default(), None, &mut no_stop());
        assert_eq!(code, 0);
        assert_eq!(
            settings(&p),
            json!({
                "statusLine": {"type": "command", "command": "sh -c 'my-line; platoon statusline'"},
                "hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "platoon hook session-start --verbose"}]}]}
            }),
            "the quoted path is setup's own guard; the other two are the user's"
        );
        assert!(said.contains("your own command that runs platoon: sh -c 'my-line; platoon statusline'"), "{said}");
        assert!(said.contains("platoon hook session-start --verbose"), "{said}");
        assert!(said.contains("the session guard hook") && !said.contains("the status line,"), "{said}");
    }

    #[test]
    fn a_second_run_finds_nothing_left_and_writes_nothing() {
        let p = places(&tmp("again"));
        set_up(&p, json!({"theme": "dark"}));
        assert_eq!(uninstall(&p, &Options::default(), None, &mut no_stop()).0, 0);
        let after = fs::read_to_string(p.root.join("settings.json")).unwrap();
        let backup = fs::read_to_string(p.root.join("settings.json.platoon-bak")).unwrap();
        let (code, said) = uninstall(&p, &Options::default(), None, &mut no_stop());
        assert_eq!(code, 0, "{said}");
        assert!(said.contains("Nothing of platoon's was left to remove."), "{said}");
        assert!(!said.contains("Removed:"), "{said}");
        assert_eq!(fs::read_to_string(p.root.join("settings.json")).unwrap(), after);
        assert_eq!(fs::read_to_string(p.root.join("settings.json.platoon-bak")).unwrap(), backup, "the backup isn't overwritten");
        // No settings file at all: nothing to do, nothing made.
        let p = places(&tmp("no-settings"));
        let (code, said) = uninstall(&p, &Options::default(), None, &mut no_stop());
        assert!(code == 0 && said.contains("Nothing of platoon's was left to remove."), "{said}");
        assert!(!p.root.exists(), "Claude Code's folder isn't made");
    }

    #[test]
    fn settings_that_cant_be_read_stop_everything_before_anything_changes() {
        let p = places(&tmp("bad-json"));
        fs::create_dir_all(&p.root).unwrap();
        fs::create_dir_all(&p.cache).unwrap();
        fs::write(p.root.join("settings.json"), "{ not json").unwrap();
        let (code, said) = uninstall(&p, &Options { all: true, stop_sessions: true }, Some("y\ny\n"), &mut no_stop());
        assert_eq!(code, 1);
        assert!(said.contains("isn't valid JSON") && said.contains("Nothing was changed"), "{said}");
        assert_eq!(fs::read_to_string(p.root.join("settings.json")).unwrap(), "{ not json");
        assert!(p.cache.exists());
    }

    #[test]
    fn the_cache_always_goes_and_the_config_only_after_a_yes_or_all() {
        for (opts, typed, gone) in [
            (Options::default(), Some("y\n"), true),
            (Options::default(), Some("n\n"), false),
            (Options::default(), Some("\n"), false),
            (Options::default(), Some(""), false),
            (Options::default(), None, false),
            (Options { all: true, stop_sessions: false }, None, true),
        ] {
            let p = places(&tmp("folders"));
            for d in [&p.cache, &p.config, &p.leftovers[0].0] {
                fs::create_dir_all(d.join("x")).unwrap();
            }
            let (code, said) = uninstall(&p, &opts, typed, &mut no_stop());
            assert_eq!(code, 0, "{said}");
            assert!(!p.cache.exists() && !p.leftovers[0].0.exists(), "{said}");
            assert_eq!(!p.config.exists(), gone, "{opts:?} {typed:?}: {said}");
            let asked = said.contains("Also delete your projects, notes, tasks and settings? [y/N]");
            assert_eq!(asked, typed.is_some() && !opts.all, "{said}");
            if !gone {
                assert!(said.contains("To delete them later: rm -rf"), "{said}");
                assert_eq!(said.contains("there was no terminal to ask in; --all deletes it"), typed.is_none(), "{said}");
            }
        }
    }

    #[test]
    fn a_folder_that_is_home_or_holds_claude_codes_is_never_deleted() {
        let base = tmp("guard-dirs");
        let mut p = places(&base);
        p.config = p.home.clone();
        p.cache = base.clone();
        fs::create_dir_all(p.home.join("keep")).unwrap();
        let (code, said) = uninstall(&p, &Options { all: true, stop_sessions: false }, None, &mut no_stop());
        assert_eq!(code, 1, "{said}");
        assert!(p.home.join("keep").exists(), "{said}");
        assert!(said.contains("is your home folder or holds it") && said.contains("Couldn't remove:"), "{said}");
        assert!(deletable(Path::new("relative/platoon"), &p).is_err() && deletable(Path::new("/tmp"), &p).is_err());
        assert!(deletable(&p.root, &p).is_err() && deletable(&p.home.join(".cache/platoon"), &p).is_ok());
    }

    #[test]
    fn the_test_home_guard_refuses_any_place_outside_it() {
        let p = places(&tmp("test-home"));
        let inside = p.home.parent().unwrap();
        assert!(check_test_home(&p, None).is_ok() && check_test_home(&p, Some(inside)).is_ok());
        let mut out = p.clone();
        out.cache = PathBuf::from("/var/tmp/elsewhere");
        assert!(check_test_home(&out, Some(inside)).unwrap_err().contains("/var/tmp/elsewhere"));
        let mut sneaky = p.clone();
        sneaky.config = inside.join("../x");
        assert!(check_test_home(&sneaky, Some(inside)).is_err());
    }

    /// A stand-in "claude": a process this test started, never anyone else's.
    struct Sleeper(std::process::Child);
    impl Drop for Sleeper {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn host_entry(p: &Places, pid: u32, args: Value) {
        fs::create_dir_all(p.cache.join("hosts")).unwrap();
        fs::write(
            p.cache.join(format!("hosts/{pid}.json")),
            json!({"claude_pid": pid, "host_pid": pid, "dir": "/work/x", "args": args}).to_string(),
        )
        .unwrap();
    }

    #[test]
    fn running_sessions_are_listed_and_a_no_or_no_terminal_changes_nothing() {
        let p = places(&tmp("sessions-no"));
        set_up(&p, json!({}));
        let s = Sleeper(Command::new("sleep").arg("60").stdin(Stdio::null()).spawn().unwrap());
        let pid = s.0.id();
        host_entry(&p, pid, json!(["--resume", "abc-123"]));
        host_entry(&p, 4_000_000_001, json!([])); // gone: not listed
        fs::create_dir_all(&p.config).unwrap();
        let before = fs::read_to_string(p.root.join("settings.json")).unwrap();
        for typed in [Some("n\n"), Some(""), None] {
            let (code, said) = uninstall(&p, &Options { all: true, stop_sessions: false }, typed, &mut no_stop());
            assert_eq!(code, 1, "{said}");
            assert!(said.contains(&format!("(pid {pid}): `claude --resume abc-123` opens it again")), "{said}");
            assert!(!said.contains("4000000001"), "{said}");
            assert!(said.contains("Their conversations are saved: `claude --resume <id>` opens them again."), "{said}");
            if typed.is_some() {
                assert!(said.contains("Stop it and go on? [y/N]") && said.contains("Nothing was changed"), "{said}");
            } else {
                assert!(said.contains("--stop-sessions") && said.contains("changed nothing"), "{said}");
            }
            assert!(!said.contains("Also delete"), "nothing more is asked after a no: {said}");
            assert_eq!(fs::read_to_string(p.root.join("settings.json")).unwrap(), before);
            assert!(p.cache.join(format!("hosts/{pid}.json")).exists() && p.config.exists());
            assert!(p.cache.join("hosts/4000000001.json").exists(), "looking cleans nothing up");
        }
    }

    #[test]
    fn a_yes_or_stop_sessions_stops_them_the_apps_way_then_removes_everything() {
        for (opts, typed) in [(Options::default(), Some("y\nn\n")), (Options { all: false, stop_sessions: true }, None)] {
            let p = places(&tmp("sessions-yes"));
            set_up(&p, json!({}));
            let s = Sleeper(Command::new("sleep").arg("60").stdin(Stdio::null()).spawn().unwrap());
            let pid = s.0.id();
            host_entry(&p, pid, json!(["attach", "job7"]));
            let mut stopped = Vec::new();
            let mut stop = |pids: &[u32]| {
                stopped.extend_from_slice(pids);
                Ok(())
            };
            let (code, said) = uninstall(&p, &opts, typed, &mut stop);
            assert_eq!(code, 0, "{said}");
            assert_eq!(stopped, vec![pid], "{said}");
            assert!(
                said.contains("a background session, shown in /work/x") && said.contains("`claude attach job7` shows it again"),
                "{said}"
            );
            assert!(said.contains("the session platoon was running: stopped"), "{said}");
            assert!(!p.cache.exists() && !settings(&p).to_string().contains("platoon"), "{said}");
        }
        // The stop failing: nothing else is changed.
        let p = places(&tmp("sessions-fail"));
        set_up(&p, json!({}));
        let s = Sleeper(Command::new("sleep").arg("60").stdin(Stdio::null()).spawn().unwrap());
        host_entry(&p, s.0.id(), json!([]));
        let (code, said) =
            uninstall(&p, &Options { all: true, stop_sessions: true }, None, &mut |_: &[u32]| Err("it didn't exit in time".into()));
        assert_eq!(code, 1);
        assert!(said.contains("Couldn't stop the sessions (it didn't exit in time). Nothing else was changed"), "{said}");
        assert!(settings(&p).to_string().contains("platoon") && p.cache.exists());
    }

    #[test]
    fn the_real_stop_ends_a_process_with_sigterm() {
        let mut s = Sleeper(Command::new("sleep").arg("60").stdin(Stdio::null()).spawn().unwrap());
        let pid = s.0.id();
        let reaper = std::thread::spawn(move || {
            let st = s.0.wait();
            std::mem::forget(s);
            st
        });
        crate::host::end_processes(&[pid]).unwrap();
        let st = reaper.join().unwrap().unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(st.signal(), Some(libc::SIGTERM));
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git").arg("-C").arg(dir).args(args).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn task_worktrees_and_branches_are_listed_with_their_commands_and_never_touched() {
        let base = tmp("worktrees");
        let p = places(&base);
        let repo = base.join("repo");
        fs::create_dir_all(repo.join("sub")).unwrap();
        git(&repo, &["init", "-q"]);
        fs::write(repo.join("sub/a"), "a").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "a"]);
        let wt = base.join("repo-worktrees/task-3");
        git(&repo, &["worktree", "add", "-q", "-b", "task/3", wt.to_str().unwrap()]);
        git(&repo, &["branch", "task/5"]); // task 5's worktree was removed; its branch is kept
        let project_dir = p.config.join("projects/p1");
        fs::create_dir_all(&project_dir).unwrap();
        let t = |n: u32, folder: &Path, path: &Path| {
            json!({"number": n, "title": format!("t{n}"), "file": format!("{n:02}.md"), "state": "done", "folder": folder,
                "worktree": {"path": path, "branch": format!("task/{n}")}})
        };
        fs::write(
            project_dir.join("tasks.json"),
            json!({"next": 8, "tasks": [
                t(3, &repo.join("sub"), &wt.join("sub")),
                t(5, &repo, &base.join("repo-worktrees/task-5")),
                t(6, &repo, &base.join("repo-worktrees/task-6")),
                t(7, &base.join("gone"), &base.join("gone-worktrees/task-7"))
            ]})
            .to_string(),
        )
        .unwrap();
        let (code, said) = uninstall(&p, &Options { all: true, stop_sessions: false }, None, &mut no_stop());
        assert_eq!(code, 0, "{said}");
        let r = sh_quote(&repo.join("sub").to_string_lossy());
        assert!(said.contains("#3 (p1):"), "{said}");
        assert!(said.contains(&format!("git -C {r} worktree remove {}", sh_quote(&wt.to_string_lossy()))), "{said}");
        assert!(said.contains(&format!("git -C {r} branch -D task/3")), "{said}");
        assert_eq!(said.matches("worktree remove").count(), 1, "{said}");
        assert!(said.contains("#5 (p1):") && said.contains("branch -D task/5"), "{said}");
        assert!(!said.contains("#6") && !said.contains("#7"), "no branch, no folder: nothing to say: {said}");
        assert!(wt.join("sub/a").exists(), "never deleted");
        assert!(!p.config.exists(), "read before the config folder went");
    }

    #[test]
    fn flags_and_quoting() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(parse_args(&s(&["--all", "--stop-sessions"])).unwrap(), Some(Options { all: true, stop_sessions: true }));
        assert_eq!(parse_args(&s(&["--help"])).unwrap(), None);
        assert!(parse_args(&s(&["--yes"])).unwrap_err().contains("don't know \"--yes\""));
        assert_eq!(sh_quote("/a/b-c_d.e"), "/a/b-c_d.e");
        assert_eq!(sh_quote("/a b/it's"), r"'/a b/it'\''s'");
        assert!(program_line(&versions::Install::Cargo, Path::new("/x")).ends_with("cargo uninstall platoon-tui"));
        assert!(program_line(&versions::Install::Homebrew, Path::new("/x")).ends_with("brew uninstall platoon"));
        assert!(program_line(&versions::Install::Other, Path::new("/a b/platoon")).contains("rm '/a b/platoon'"));
        assert_eq!(worktree_root(Path::new("/r-worktrees/task-3/sub/x"), 3), Some(PathBuf::from("/r-worktrees/task-3")));
        assert_eq!(worktree_root(Path::new("/r/task-3/sub"), 3), None);
    }
}
