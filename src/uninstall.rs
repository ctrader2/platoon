//! `platoon uninstall`: takes platoon back out of Claude Code and off the computer, all but the program itself (the
//! way it was installed decides how that goes: `scripts/uninstall.sh` does it, or the last line says how).
//!
//! What it does, in this order, so a "no" leaves everything as it was:
//! 1. Look: platoon's entries in Claude Code's `settings.json` (`setup::take_out`, exactly what setup writes) and in
//!    Codex's `hooks.json` (`setup::take_out_codex`), the `/to-tickets` skill (`skill::state`), the sessions platoon's hosts are running, the worktrees tasks
//!    recorded, and the folders.
//! 2. Ask: whether to stop the running sessions (no: stop here, nothing changed; `--stop-sessions` says yes), then
//!    whether to delete the config folder (the user's projects, notes, tasks and settings: default keep; `--all`
//!    deletes it without asking). Answers come from the terminal (`/dev/tty`, so `curl ... | sh` can still ask); with
//!    no terminal the sessions question is a no and the config folder is kept.
//! 3. Act: stop the sessions (SIGTERM to the `claude` each host runs, as the app's own stop does), take the entries out
//!    of settings.json (backup first, as setup does), have Codex forget it trusted platoon's hooks
//!    (`setup::forget_codex_trust`, through Codex's app-server: Codex's other records stay, and when Codex can't be
//!    asked they are left and said to be harmless) and take them out of `hooks.json`, have Codex forget its trust in
//!    the folders of tasks' worktrees platoon had it trust (`codex_server::forget_folders`, only those the tasks
//!    record), delete the `/to-tickets` skill when it is the copy platoon wrote (`skill::remove`; one of the user's
//!    stays, and is said), delete the cache folder, the config folder when asked, and older platoons' leftovers.
//! 4. Say what was removed and what was kept, with the git commands for the task worktrees (never deleted).
//!
//! Claude Code's and Codex's conversations are never touched.

use crate::agents::{claude, codex, codex_server};
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
    /// Codex's folder (`CODEX_HOME`, else `~/.codex`), there or not.
    pub codex: PathBuf,
    /// Codex's program (`PLATOON_CODEX_BIN`, else `codex`), asked to forget the hooks it trusted.
    pub codex_program: String,
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
        Self {
            root: claude::claude_root(),
            codex: codex::codex_root(),
            codex_program: codex::program(),
            config: crate::archive::config_dir(),
            cache: crate::usage::cache_dir(),
            leftovers,
            home,
        }
    }

    fn all(&self) -> Vec<&Path> {
        let mut v = vec![self.root.as_path(), &self.codex, &self.config, &self.cache, &self.home];
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
    for (other, what) in [(&places.home, "your home folder"), (&places.root, "Claude Code's folder"), (&places.codex, "Codex's folder")] {
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
    /// The agent running it.
    pub agent: crate::agents::Agent,
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
        let agent = crate::host::agent_of_info(&v);
        let attach = agent.attaches(&args);
        let id = if attach { args.get(1).cloned() } else { entry.map(|l| l.id.clone()).or_else(|| agent.session_of_args(&args)) };
        out.push(Running {
            claude_pid: pid,
            dir: v.get("dir").and_then(Value::as_str).unwrap_or("").to_string(),
            id,
            name: entry.and_then(|l| l.name.clone()),
            attach,
            agent,
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
    /// Codex's `hooks.json` with platoon's hooks taken out, when it is there and could be read.
    codex_hooks: Option<(PathBuf, Value, setup::TakenOut)>,
    /// Why Codex's `hooks.json` was left alone: it is there but can't be read or isn't a JSON object.
    codex_error: Option<String>,
    pub running: Vec<Running>,
    worktrees: Vec<KeptWorktree>,
    /// The task worktrees' folders platoon had Codex trust and that its task records still name (`codex_trust_left`).
    codex_trust: Vec<String>,
    config_there: bool,
    /// What is at the `/to-tickets` skill's place in Claude Code's folder.
    skill: Result<crate::skill::State, String>,
}

/// The folders platoon had Codex trust for tasks' worktrees, as the tasks in `config` record them, while those records
/// say the trust is still there: only folders of task worktrees platoon made (`codex_server::is_task_worktree`).
pub fn codex_trust_left(config: &Path) -> Vec<String> {
    let mut out: Vec<String> = tickets::every_ticket(config)
        .into_iter()
        .filter_map(|(_, t)| t.worktree?.codex_trust)
        .filter(|k| codex_server::is_task_worktree(k))
        .collect();
    out.sort();
    out.dedup();
    out
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
    let (mut codex_hooks, mut codex_error) = (None, None);
    let hooks = codex::hooks_path(&places.codex);
    match fs::read_to_string(&hooks) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => codex_error = Some(format!("couldn't read {}: {e}", hooks.display())),
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(mut v) if v.is_object() => {
                let taken = setup::take_out_codex(&mut v);
                codex_hooks = Some((hooks, v, taken));
            }
            Ok(_) => codex_error = Some(format!("{} isn't a JSON object", hooks.display())),
            Err(e) => codex_error = Some(format!("{} isn't valid JSON ({e})", hooks.display())),
        },
    }
    Plan {
        settings,
        settings_error,
        codex_hooks,
        codex_error,
        running: running_sessions(&places.cache, &places.root),
        worktrees: kept_worktrees(&places.config, git),
        codex_trust: codex_trust_left(&places.config),
        config_there: places.config.exists(),
        skill: crate::skill::state(&places.root),
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
    let again = match (&r.id, r.attach, r.agent) {
        (Some(id), true, _) => format!(": `claude attach {id}` shows it again"),
        (Some(id), false, crate::agents::Agent::Claude) => format!(": `claude --resume {id}` opens it again"),
        (Some(id), false, crate::agents::Agent::Codex) => format!(": `codex resume {id}` opens it again"),
        (None, _, _) => String::new(),
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
        if plan.running.iter().any(|r| r.agent == crate::agents::Agent::Codex) {
            let _ = writeln!(out, "Their conversations are saved: `claude --resume <id>` (or `codex resume <id>`) opens them again.");
        } else {
            let _ = writeln!(out, "Their conversations are saved: `claude --resume <id>` opens them again.");
        }
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

    if let Some((path, v, taken)) = &plan.codex_hooks {
        if !taken.removed.is_empty() {
            // Asked while platoon's hooks are still in the file: Codex names its records after their places there.
            match setup::forget_codex_trust(&places.codex, &places.codex_program, &places.home, codex_server::TIME_BOX) {
                Ok(0) => {}
                Ok(_) => removed.push(format!(
                    "from Codex's settings ({}), by Codex itself: its records that you trusted platoon's hooks",
                    codex::config_path(&places.codex).display()
                )),
                Err(why) => kept.push(format!(
                    "Codex's records of trusting platoon's hooks, in {}: Codex couldn't be asked to remove them ({why}). They \
                     are harmless: the hooks they name are gone",
                    codex::config_path(&places.codex).display()
                )),
            }
            let backup = places.codex.join("hooks.json.platoon-bak");
            let written = fs::copy(path, &backup)
                .map_err(|e| format!("couldn't back up {}: {e}", path.display()))
                .and_then(|_| serde_json::to_string_pretty(v).map_err(|e| e.to_string()))
                .and_then(|text| {
                    crate::projects::write_atomic(path, &(text + "\n")).map_err(|e| format!("couldn't write {}: {e}", path.display()))
                });
            match written {
                Ok(()) => removed.push(format!(
                    "from Codex's hooks ({}): {}. The file as it was is kept as hooks.json.platoon-bak",
                    path.display(),
                    taken.removed.join(", ")
                )),
                Err(e) => failed.push(e),
            }
        }
        for c in &taken.left {
            kept.push(format!("in Codex's hooks, your own command that runs platoon: {c} (take the platoon part out by hand)"));
        }
    }
    // The folders of tasks' worktrees platoon had Codex trust: Codex itself takes them out of its settings.
    if !plan.codex_trust.is_empty() {
        let n = plan.codex_trust.len();
        let folders = if n == 1 { "one task worktree's folder".to_string() } else { format!("{n} task worktrees' folders") };
        match codex_server::forget_folders(&places.codex_program, &places.codex, &plan.codex_trust, codex_server::TIME_BOX) {
            Ok(_) => removed.push(format!(
                "from Codex's settings ({}), by Codex itself: its trust in {folders} platoon had it trust",
                codex::config_path(&places.codex).display()
            )),
            Err(why) => kept.push(format!(
                "Codex's trust in {folders} platoon had it trust, in {}: Codex couldn't be asked to remove it ({why}). It is \
                 harmless; to take it out by hand, remove these [projects] entries: {}",
                codex::config_path(&places.codex).display(),
                plan.codex_trust.join(", ")
            )),
        }
    }

    // The /to-tickets skill: only the copy platoon wrote and nobody changed.
    match &plan.skill {
        Ok(crate::skill::State::Missing) => {}
        Ok(crate::skill::State::NotOurs) => kept.push(format!(
            "the /to-tickets skill at {}: platoon didn't write it (or it was changed since), so it is yours and stays",
            crate::skill::path(&places.root).display()
        )),
        Ok(crate::skill::State::Ours(_)) => match crate::skill::remove(&places.root) {
            crate::skill::Removal::Removed(p) => removed.push(format!("the /to-tickets skill ({})", p.display())),
            crate::skill::Removal::NotThere => {}
            crate::skill::Removal::NotOurs(p) => {
                kept.push(format!("the /to-tickets skill at {}: it changed while uninstalling, so it stays", p.display()))
            }
            crate::skill::Removal::Failed(why) => failed.push(why),
        },
        Err(why) => failed.push(format!("the /to-tickets skill was left alone: {why}")),
    }

    if let Some(e) = &plan.codex_error {
        failed.push(format!(
            "Codex's hooks were left alone, so platoon's may still be in them: {e}. Fix the file, or take them out by hand ({GUIDE})"
        ));
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
    if places.codex.is_dir() {
        kept.push("your Codex conversations and Codex's own settings: `codex resume` opens any of them".into());
    }

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
            codex: home.join(".codex"),
            // Never the real one: a test that wants Codex asked gives its stand-in.
            codex_program: "/nonexistent/platoon-test/codex".into(),
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
    fn platoons_codex_hooks_go_too_and_everything_else_of_codexs_stays() {
        let p = places(&tmp("codex"));
        set_up(&p, json!({}));
        fs::create_dir_all(&p.codex).unwrap();
        let hooks = json!({"hooks": {
            "SessionStart": [{"hooks": [{"type": "command", "command": "/opt/x/bin/platoon hook session-start --agent codex"}]}],
            "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "/opt/x/bin/platoon hook guard --agent codex"}]}],
            "Stop": [{"hooks": [{"type": "command", "command": "my-stop"}, {"type": "command", "command": "/opt/x/bin/platoon hook stop --agent codex"}]}]
        }});
        let before = serde_json::to_string_pretty(&hooks).unwrap();
        fs::write(p.codex.join("hooks.json"), &before).unwrap();
        fs::write(p.codex.join("config.toml"), "[hooks.state]\ntrusted = true\n").unwrap();
        let (code, out) = uninstall(&p, &Options::default(), None, &mut no_stop());
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("from Codex's hooks") && out.contains("the Codex notes updates hook"), "{out}");
        assert!(out.contains("your Codex conversations and Codex's own settings"), "{out}");
        // No Codex to ask: its records are left, said to be harmless, and nothing else waits for them.
        assert!(
            out.contains("Codex couldn't be asked to remove them (Codex's program (/nonexistent/platoon-test/codex) isn't there)"),
            "{out}"
        );
        assert!(out.contains("They are harmless: the hooks they name are gone"), "{out}");
        let v: Value = serde_json::from_str(&fs::read_to_string(p.codex.join("hooks.json")).unwrap()).unwrap();
        assert_eq!(v, json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "my-stop"}]}]}}));
        assert_eq!(fs::read_to_string(p.codex.join("hooks.json.platoon-bak")).unwrap(), before);
        assert_eq!(fs::read_to_string(p.codex.join("config.toml")).unwrap(), "[hooks.state]\ntrusted = true\n", "Codex's own records stay");
    }

    /// platoon's three hooks in Codex's hooks.json (as an installed platoon wrote them) beside the user's own; Codex's
    /// list of them, as a stand-in app-server gives it, with a hook of a project's that runs the same command.
    fn codex_with_trusted_hooks(p: &Places) -> (Vec<String>, Value) {
        fs::create_dir_all(&p.codex).unwrap();
        let file = p.codex.join("hooks.json");
        let hooks = json!({"hooks": {
            "SessionStart": [{"hooks": [{"type": "command", "command": "/opt/x/bin/platoon hook session-start --agent codex"}]}],
            "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "/opt/x/bin/platoon hook guard --agent codex"}]}],
            "Stop": [{"hooks": [{"type": "command", "command": "my-stop"}]}, {"hooks": [{"type": "command", "command": "/opt/x/bin/platoon hook stop --agent codex"}]}]
        }});
        fs::write(&file, serde_json::to_string_pretty(&hooks).unwrap()).unwrap();
        let k = |rest: &str| format!("{}:{rest}", file.display());
        let project = p.home.join("repo/.codex/hooks.json");
        let listed = vec![
            crate::testkit::codex_hook(
                &k("session_start:0:0"),
                "sessionStart",
                "/opt/x/bin/platoon hook session-start --agent codex",
                &file,
                "sha256:a",
                "trusted",
            ),
            crate::testkit::codex_hook(
                &k("user_prompt_submit:0:0"),
                "userPromptSubmit",
                "/opt/x/bin/platoon hook guard --agent codex",
                &file,
                "sha256:b",
                "trusted",
            ),
            crate::testkit::codex_hook(&k("stop:0:0"), "stop", "my-stop", &file, "sha256:c", "trusted"),
            crate::testkit::codex_hook(&k("stop:1:0"), "stop", "/opt/x/bin/platoon hook stop --agent codex", &file, "sha256:d", "modified"),
            crate::testkit::codex_hook(
                &format!("{}:stop:0:0", project.display()),
                "stop",
                "/opt/x/bin/platoon hook stop --agent codex",
                &project,
                "sha256:e",
                "trusted",
            ),
        ];
        let keys = vec![k("session_start:0:0"), k("user_prompt_submit:0:0"), k("stop:1:0")];
        (keys, json!({"data": [{"cwd": p.home, "hooks": listed, "warnings": [], "errors": []}]}))
    }

    #[test]
    fn codex_forgets_it_trusted_platoons_hooks_and_keeps_every_other_record() {
        let mut p = places(&tmp("codex-forget"));
        let (ours, listed) = codex_with_trusted_hooks(&p);
        let fake = crate::testkit::CodexServerFake::new("uninstall-forget", &p.codex);
        p.codex_program = fake.program.clone();
        fake.result("hooks/list", None, listed);
        let file = p.codex.join("hooks.json");
        let others = json!({
            format!("{}:stop:0:0", file.display()): {"trusted_hash": "sha256:c"},
            format!("{}:stop:0:0", p.home.join("repo/.codex/hooks.json").display()): {"trusted_hash": "sha256:e"},
            "/gone/hooks.json:stop:0:0": {"trusted_hash": "sha256:old", "enabled": false}
        });
        let mut state = others.as_object().unwrap().clone();
        for k in &ours {
            state.insert(k.clone(), json!({"trusted_hash": "sha256:x"}));
        }
        let user = json!({"name": {"type": "user", "file": codex::config_path(&p.codex), "profile": null}, "version": "sha256:v1",
            "config": {"model": "x", "hooks": {"state": state}}});
        let system = json!({"name": {"type": "system", "file": "/etc/codex/config.toml"}, "version": "s", "config": {"hooks": {"state": {(ours[0].clone()): {}}}}});
        fake.result("config/read", None, json!({"config": {}, "origins": {}, "layers": [system, user]}));
        fake.result("config/batchWrite", None, json!({"status": "ok", "version": "sha256:v2", "filePath": codex::config_path(&p.codex)}));
        let (code, out) = uninstall(&p, &Options::default(), None, &mut no_stop());
        assert_eq!(code, 0, "{out}");
        assert_eq!(fake.methods(), ["initialize", "initialized", "hooks/list", "config/read", "config/batchWrite"]);
        let sent = fake.requests();
        assert_eq!(sent[2]["params"], json!({"cwds": [p.home]}));
        assert_eq!(sent[3]["params"], json!({"includeLayers": true}));
        let edit = json!({"keyPath": "hooks.state", "value": others, "mergeStrategy": "replace"});
        assert_eq!(sent[4]["params"], json!({"edits": [edit], "reloadUserConfig": true, "expectedVersion": "sha256:v1"}));
        assert!(out.contains("by Codex itself: its records that you trusted platoon's hooks"), "{out}");
        assert!(!out.contains("couldn't be asked"), "{out}");
        let v: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v, json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "my-stop"}]}]}}), "and the hooks went too");
        assert!(!codex::config_path(&p.codex).exists(), "platoon never writes Codex's config.toml itself");

        // Nothing of platoon's recorded: nothing is written.
        let mut p = places(&tmp("codex-forget-none"));
        let (_, listed) = codex_with_trusted_hooks(&p);
        let fake = crate::testkit::CodexServerFake::new("uninstall-forget-none", &p.codex);
        p.codex_program = fake.program.clone();
        fake.result("hooks/list", None, listed);
        fake.result(
            "config/read",
            None,
            json!({"config": {}, "origins": {}, "layers": [{"name": {"type": "user"}, "version": "v", "config": {}}]}),
        );
        let (code, out) = uninstall(&p, &Options::default(), None, &mut no_stop());
        assert_eq!(code, 0, "{out}");
        assert_eq!(fake.methods(), ["initialize", "initialized", "hooks/list", "config/read"]);
        assert!(!out.contains("records"), "{out}");
    }

    #[test]
    fn when_codex_refuses_to_forget_the_records_are_said_to_be_left_and_the_rest_goes() {
        let mut p = places(&tmp("codex-forget-refused"));
        set_up(&p, json!({}));
        let (_, listed) = codex_with_trusted_hooks(&p);
        let fake = crate::testkit::CodexServerFake::new("uninstall-refused", &p.codex);
        p.codex_program = fake.program.clone();
        fake.result("hooks/list", None, listed);
        fake.answer("config/read", None, &crate::testkit::CodexServerFake::reply("error", json!({"message": "no config for you"})));
        let (code, out) = uninstall(&p, &Options::default(), None, &mut no_stop());
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("Codex couldn't be asked to remove them (Codex refused: no config for you)"), "{out}");
        assert!(out.contains("from Codex's hooks") && settings(&p).get("statusLine").is_none(), "{out}");
    }

    #[test]
    fn a_codex_hooks_file_that_cant_be_parsed_is_said_and_left_alone_and_the_rest_still_goes() {
        let p = places(&tmp("codex-bad"));
        set_up(&p, json!({}));
        fs::create_dir_all(&p.codex).unwrap();
        fs::write(p.codex.join("hooks.json"), "{ not json").unwrap();
        let (code, out) = uninstall(&p, &Options::default(), None, &mut no_stop());
        assert_eq!(code, 1, "{out}");
        assert!(out.contains("Codex's hooks were left alone") && out.contains("isn't valid JSON"), "{out}");
        assert_eq!(fs::read_to_string(p.codex.join("hooks.json")).unwrap(), "{ not json");
        assert!(!p.codex.join("hooks.json.platoon-bak").exists());
        assert!(settings(&p).get("statusLine").is_none(), "Claude Code's entries went all the same");
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

    #[test]
    fn codex_forgets_only_the_task_worktree_folders_platoon_had_it_trust() {
        let mut p = places(&tmp("codex-folders"));
        fs::create_dir_all(&p.codex).unwrap();
        // Two tasks whose worktrees platoon had Codex trust, one whose record names a folder that isn't a task
        // worktree (never taken away), and one with no such record.
        let project = crate::projects::Projects::open(&p.config).create("Shop").unwrap();
        let mut store = tickets::Tickets::open(&p.config, &project);
        let trusted = |k: Option<&str>| tickets::Worktree {
            path: k.unwrap_or("/home/u/shop-worktrees/task-9").into(),
            branch: "task/1".into(),
            codex_trust: k.map(String::from),
            ..Default::default()
        };
        for k in [Some("/home/u/shop-worktrees/task-1"), Some("/home/u/web-worktrees/task-2"), Some("/home/u/shop"), None] {
            let n = store.add(tickets::NewTicket { title: "T".into(), ..Default::default() }).unwrap();
            store.start(n, "feature", &format!("s-{n}"), Some(trusted(k)), &[]).unwrap();
        }
        assert_eq!(codex_trust_left(&p.config), ["/home/u/shop-worktrees/task-1", "/home/u/web-worktrees/task-2"]);
        let fake = crate::testkit::CodexServerFake::new("uninstall-folders", &p.codex);
        p.codex_program = fake.program.clone();
        let projects = json!({"/home/u/shop-worktrees/task-1": {"trust_level": "trusted"}, "/home/u/web-worktrees/task-2": {"trust_level": "trusted"},
            "/home/u/shop": {"trust_level": "trusted"}});
        fake.result("config/read", None, json!({"config": {}, "origins": {}, "layers": [{"name": {"type": "user"}, "version": "sha256:p1", "config": {"projects": projects}}]}));
        fake.result("config/batchWrite", None, json!({"status": "ok"}));
        let (code, out) = uninstall(&p, &Options::default(), None, &mut no_stop());
        assert_eq!(code, 0, "{out}");
        let sent = fake.requests();
        let write = sent.iter().find(|r| r["method"] == "config/batchWrite").expect("written");
        assert_eq!(
            write["params"],
            json!({"edits": [
                {"keyPath": "projects.\"/home/u/shop-worktrees/task-1\"", "value": null, "mergeStrategy": "replace"},
                {"keyPath": "projects.\"/home/u/web-worktrees/task-2\"", "value": null, "mergeStrategy": "replace"}],
                "reloadUserConfig": true, "expectedVersion": "sha256:p1"})
        );
        assert!(out.contains("its trust in 2 task worktrees' folders platoon had it trust"), "{out}");
        // Codex can't be asked: said, with what to take out by hand, and harmless.
        let mut p2 = places(&tmp("codex-folders-gone"));
        fs::create_dir_all(&p2.codex).unwrap();
        let project = crate::projects::Projects::open(&p2.config).create("Shop").unwrap();
        let mut store = tickets::Tickets::open(&p2.config, &project);
        let n = store.add(tickets::NewTicket { title: "T".into(), ..Default::default() }).unwrap();
        store.start(n, "feature", "s-1", Some(trusted(Some("/home/u/shop-worktrees/task-1"))), &[]).unwrap();
        p2.codex_program = "/nonexistent/platoon-test/codex".into();
        let (_, out) = uninstall(&p2, &Options::default(), None, &mut no_stop());
        assert!(
            out.contains("Codex's trust in one task worktree's folder platoon had it trust")
                && out.contains("remove these [projects] entries: /home/u/shop-worktrees/task-1"),
            "{out}"
        );
    }
}
