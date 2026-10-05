//! The test check platoon runs itself. A project may name a **test command** (`test_command` in `projects.json`, see
//! `projects.rs`): when a squad's leader stops at the end of the work, platoon runs it where the squad works and tells
//! the squad when it fails, so a squad's "done" is checked by something other than the squad.
//!
//! How it goes: the `squad-stop` hook (a short-lived process inside the leader's session) sets the task's state as the
//! leader's last line says and, when that line is the end of the work (`Stop`) and the project has a command, marks the
//! task `check_due`. It runs nothing. The platoon window that holds the squad line's lock runs the command
//! (`app/checks_glue.rs`), off the UI thread, with `run` here: `sh -c <command>` in the folder the squad works in (the
//! task's copy of its first git folder, or the user's checkout when the task works on a branch there and the checkout is
//! still on it: `place`), never where another session of the user's is open, with the leader's environment less
//! platoon's own variables (`env`), no terminal, its output kept as `<NN>-work/tests-<try>.log`, stopped with its
//! process group when its time is up. The result goes on the task (`tickets::CheckRun`), and `then` says what it means:
//! a pass leaves the task as the leader left it; a failure is typed into the squad's session (`typed_message`) and the
//! task is Working again, at most `FIXES` times in a row; the next failure leaves it Needs you.
//!
//! Also here: the suggestion on the project's screen (the first command of the "Build and test" block in the
//! `CLAUDE.md` of the project's first git folder; nothing is ever run from a suggestion).

use crate::squads::EndLine;
use crate::tickets::{CheckRun, CheckThen, Ticket};
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

/// The kind of check this platoon runs: the project's test command.
pub const TESTS: &str = "tests";

/// How many times in a row the squad is told its tests fail and given the work back; the next failure needs the user.
pub const FIXES: u32 = 2;

/// How many of the output's last lines are typed into the squad's session with a failure.
pub const TAIL_LINES: usize = 30;

/// The longest summary kept, in characters.
pub const SUMMARY_MAX: usize = 120;

/// What a task needs the user for when its tests still fail after the squad's tries.
pub const STILL_FAIL: &str = "The tests still fail after the squad tried twice";

/// ... and when they fail and the squad's session isn't there to be told.
pub const FAILED_NOT_RUNNING: &str = "The tests failed; the squad's session isn't running";

/// Why a check wasn't run: someone works in that folder.
pub const SESSION_OPEN: &str = "a session of yours is open there";

/// What a task's row says while its check runs (or waits to).
pub const RUNNING: &str = "running the tests";

// ---- when ---------------------------------------------------------------------------------------------------------

/// The leader stopped at the end of the work: one of the three stops a check follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// `SQUAD: question - Shall I close this task?`
    Close,
    /// `SQUAD: ready to publish - ...`
    Publish,
    /// `SQUAD: done`
    Done,
}

impl Stop {
    pub fn id(self) -> &'static str {
        match self {
            Stop::Close => "close",
            Stop::Publish => "publish",
            Stop::Done => "done",
        }
    }
}

/// Is the leader's question the one it asks when the work is finished ("Shall I close this task?"; "ticket" from
/// before the rename)?
pub fn is_close_question(question: &str) -> bool {
    let q = question.to_lowercase();
    q.contains("close this task") || q.contains("close this ticket")
}

/// The stop a check follows, from the leader's last line; None for any other (members still running, a question, stuck,
/// the plan, no line).
pub fn stop_of(line: &EndLine, members_running: bool) -> Option<Stop> {
    if members_running {
        return None;
    }
    match line {
        EndLine::Question(q) if is_close_question(q) => Some(Stop::Close),
        EndLine::ReadyToPublish(_) => Some(Stop::Publish),
        EndLine::Done => Some(Stop::Done),
        _ => None,
    }
}

/// Does task `t` have a folder a check can run in: a copy of a git folder (its worktree), or a branch in the user's
/// checkout? A task with only folders outside git, or none, has no test check.
pub fn has_place(t: &Ticket) -> bool {
    t.worktrees.iter().any(|w| w.removed.is_none()) || t.folders.iter().any(|f| t.branch_in_checkout(f))
}

// ---- what a result means ------------------------------------------------------------------------------------------

/// Which of the squad's tries the next run is: 1, plus one for each failure since the tests last passed (a run that
/// wasn't done counts neither way).
pub fn next_try(checks: &[CheckRun]) -> u32 {
    let failures = checks.iter().rev().filter(|c| c.kind == TESTS && !c.skipped).take_while(|c| !c.ok).count();
    1 + u32::try_from(failures).unwrap_or(u32::MAX - 1)
}

/// What a check's result does to its task. `failed`: it ran and failed. `attempt`: its try (`next_try`). `still`: the
/// task is still at the stop the check was for (Needs you or Done, nobody has written to it, no newer stop).
/// `can_type`: the squad's session is running where platoon can type into it.
///
/// A pass, a run that wasn't done, or a task that moved on: kept, nothing else. A failure on try 1 or 2: the squad is
/// told and the task is Working (or, with nobody to tell, Needs you). On try 3: Needs you, the tests still fail. Later
/// tries (the user, told already, has had the squad go on): kept, as the leader left the task.
pub fn then(failed: bool, attempt: u32, still: bool, can_type: bool) -> CheckThen {
    if !failed || !still {
        return CheckThen::Keep;
    }
    match attempt {
        n if n <= FIXES && can_type => CheckThen::Fix,
        n if n <= FIXES => CheckThen::NeedsYou(FAILED_NOT_RUNNING.to_string()),
        n if n == FIXES + 1 => CheckThen::NeedsYou(STILL_FAIL.to_string()),
        _ => CheckThen::Keep,
    }
}

// ---- the summary and the message ----------------------------------------------------------------------------------

/// `line` with terminal escape sequences and other control characters taken out.
fn plain(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // ESC [ ... a final letter (colors and the like); any other escape is two characters.
            if chars.peek() == Some(&'[') {
                chars.next();
                for x in chars.by_ref() {
                    if ('@'..='~').contains(&x) {
                        break;
                    }
                }
            } else {
                chars.next();
            }
        } else if c == '\t' {
            out.push(' ');
        } else if !c.is_control() {
            out.push(c);
        }
    }
    out
}

/// The number just before `word` in a `test result:` line ("3 passed;" is 3).
fn count_before(line: &str, word: &str) -> Option<u64> {
    let words: Vec<&str> = line.split_whitespace().collect();
    words.windows(2).find(|w| w[1].trim_end_matches([';', ',', '.']) == word).and_then(|w| w[0].parse().ok())
}

/// One line that says how a run went: with cargo's `test result:` lines, their passed and failed counts added up
/// ("214 passed", or "211 passed, 3 failed"); otherwise the output's last line that has something on it, cut to one
/// line; with no output at all, what the exit code says.
pub fn summary(output: &str, exit_code: Option<i32>) -> String {
    let (mut found, mut passed, mut failed) = (false, 0u64, 0u64);
    for line in output.lines().map(plain) {
        let Some(at) = line.find("test result:") else { continue };
        let rest = &line[at..];
        if let (Some(p), Some(f)) = (count_before(rest, "passed"), count_before(rest, "failed")) {
            found = true;
            passed += p;
            failed += f;
        }
    }
    if found {
        return if failed == 0 { format!("{passed} passed") } else { format!("{passed} passed, {failed} failed") };
    }
    match output.lines().map(plain).rfind(|l| !l.trim().is_empty()) {
        Some(last) => cut(last.trim()),
        None => match exit_code {
            Some(0) => "passed, with nothing to say".to_string(),
            Some(c) => format!("failed with exit code {c}, with nothing to say"),
            None => "failed, with nothing to say".to_string(),
        },
    }
}

/// One line of at most `SUMMARY_MAX` characters, "…" where it was cut.
fn cut(s: &str) -> String {
    let one = crate::tickets::clean(s, usize::MAX);
    if one.chars().count() <= SUMMARY_MAX {
        return one;
    }
    let mut out: String = one.chars().take(SUMMARY_MAX - 1).collect();
    out.push('…');
    out
}

/// The longest line of output typed into the squad's session, in characters (a minified file on one line stays short).
const LINE_MAX: usize = 400;

/// The last `n` lines of the output that have something on them at the end (blank ones before them stay), each cut to
/// `LINE_MAX` characters.
pub fn tail(output: &str, n: usize) -> String {
    let lines: Vec<String> = output
        .trim_end()
        .lines()
        .map(|l| {
            let l = plain(l);
            let l = l.trim_end();
            if l.chars().count() > LINE_MAX { format!("{}…", l.chars().take(LINE_MAX - 1).collect::<String>()) } else { l.to_string() }
        })
        .collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// "10 minutes", "1 minute", "45 seconds": a time limit in words.
pub fn duration_words(secs: u64) -> String {
    match secs {
        s if s % 60 == 0 && s >= 60 => {
            let m = s / 60;
            format!("{m} minute{}", if m == 1 { "" } else { "s" })
        }
        s => format!("{s} second{}", if s == 1 { "" } else { "s" }),
    }
}

/// What platoon types into the squad's session when the tests fail: what it ran and where, the output's last
/// `TAIL_LINES` lines (or what happened when there are none), and what to do about it.
pub fn typed_message(command: &str, folder: &str, ran: &Ran) -> String {
    let mut out = format!("The tests failed when platoon ran `{command}` in {folder}:\n\n");
    let tail = tail(&ran.output, TAIL_LINES);
    if tail.trim().is_empty() {
        out.push_str(&format!("({})\n", ran.summary));
    } else {
        out.push_str(&format!("```\n{tail}\n```\n"));
        if ran.timed_out || ran.not_started {
            out.push_str(&format!("({})\n", ran.summary));
        }
    }
    out.push_str("\nFix what makes them fail, then end your turn as before: platoon runs them again when you stop.");
    out
}

// ---- where --------------------------------------------------------------------------------------------------------

/// Where a task's check runs: the folder to run the command in, and the folder no other session may be open in (the
/// task's copy as a whole, or the checkout); or why it can't run there now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    Run { folder: PathBuf, root: PathBuf },
    Skip { folder: PathBuf, why: String },
}

/// The folder platoon made for task `number`'s copy `path`: the `task-<N>` (or `ticket-<N>`) folder it is in, or the
/// path itself.
pub fn copy_root(path: &Path, number: u32) -> PathBuf {
    let names = [format!("task-{number}"), format!("ticket-{number}")];
    path.ancestors().find(|a| a.file_name().is_some_and(|n| names.iter().any(|x| *x == n.to_string_lossy()))).unwrap_or(path).to_path_buf()
}

/// Where task `t`'s check runs (`Place`), from its first folder that has a copy (a worktree not removed) or works on a
/// branch in the user's checkout; the checkout only while it is still on the task's branch (`branch_now` says which
/// branch a folder's checkout is on: read-only git, `gitfacts::current_branch`). None when it has no such folder.
pub fn place(t: &Ticket, branch_now: &dyn Fn(&Path) -> Option<String>) -> Option<Place> {
    for f in &t.folders {
        if let Some(w) = t.worktrees.iter().find(|w| w.folder == *f && w.removed.is_none()) {
            let folder = PathBuf::from(&w.path);
            if !folder.is_dir() {
                return Some(Place::Skip { folder, why: "its copy of the folder isn't there any more".into() });
            }
            let root = copy_root(&folder, t.branch_number());
            return Some(Place::Run { folder, root });
        }
        if t.branch_in_checkout(f) {
            let folder = PathBuf::from(f);
            let branch = t.branch();
            if branch_now(&folder).as_deref() != Some(branch.as_str()) {
                let name = crate::squads::folder_name(f);
                return Some(Place::Skip { folder, why: format!("your checkout of {name} isn't on {branch} any more") });
            }
            return Some(Place::Run { root: folder.clone(), folder });
        }
    }
    None
}

/// Is a session other than the squad's own open in `root` (or a folder inside it)? `others`: the folders the user's
/// other sessions work in.
pub fn open_there(root: &Path, others: &[PathBuf]) -> bool {
    others.iter().any(|d| d.starts_with(root))
}

// ---- running it ---------------------------------------------------------------------------------------------------

/// The environment the command runs with: platoon's own, as the squad's leader gets it (the variables that describe a
/// Claude Code session platoon was started from taken out, `claude::is_session_marker`), less every `PLATOON_*`
/// variable (platoon's folders and test knobs are not the project's business).
pub fn env() -> Vec<(String, String)> {
    env_from(std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))))
}

/// `env` from these variables.
fn env_from(vars: impl IntoIterator<Item = (String, String)>) -> Vec<(String, String)> {
    vars.into_iter().filter(|(k, _)| !k.starts_with("PLATOON_") && !crate::claude::is_session_marker(k)).collect()
}

/// How a run went.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ran {
    pub ok: bool,
    pub summary: String,
    /// The end of what it wrote (stdout and stderr together, as they came), as read back from its log.
    pub output: String,
    pub timed_out: bool,
    /// The command couldn't be started at all (no `sh`, the folder gone).
    pub not_started: bool,
    /// Stopped because platoon is closing: nothing is recorded, and the check is run again next time.
    pub cancelled: bool,
}

/// How much of the log's end is read back for the summary and the message.
const READ_BACK: u64 = 256 * 1024;

/// The end of the file at `path`.
fn read_end(path: &Path) -> String {
    let Ok(mut f) = fs::File::open(path) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(READ_BACK)));
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

/// Run `command` through `sh -c` in `folder`, with exactly `env`, no terminal (stdin is empty), its output (stdout
/// and stderr together) written to `log`, in a process group of its own. When `timeout` passes, or `stop` is set (platoon
/// is closing), the whole group is ended: TERM, then KILL a moment later. Only that group is ever signalled, and only
/// while its first process is still ours to wait for (so its number can't belong to anything else yet). Blocks until
/// it is over: run it on a thread of its own.
pub fn run(command: &str, folder: &Path, env: &[(String, String)], timeout: Duration, log: &Path, stop: &AtomicBool) -> Ran {
    let failed = |summary: String| Ran { summary, not_started: true, ..Ran::default() };
    if let Some(dir) = log.parent()
        && let Err(e) = fs::create_dir_all(dir)
    {
        return failed(format!("couldn't keep its output: {e}"));
    }
    let out = match fs::File::create(log) {
        Ok(f) => f,
        Err(e) => return failed(format!("couldn't keep its output: {e}")),
    };
    let err = match out.try_clone() {
        Ok(f) => f,
        Err(e) => return failed(format!("couldn't keep its output: {e}")),
    };
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .current_dir(folder)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .process_group(0);
    let mut child = match crate::ai::spawn_retrying(&mut cmd) {
        Ok(c) => c,
        Err(e) => {
            let why = e.to_string();
            return failed(format!("couldn't start it: {}", why.split(" (os error").next().unwrap_or(&why)));
        }
    };
    let group = child.id() as i32;
    let deadline = Instant::now() + timeout;
    let (status, timed_out, cancelled) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (Some(status), false, false),
            Ok(None) => {}
            Err(_) => break (None, false, false),
        }
        let late = Instant::now() >= deadline;
        let closing = stop.load(Ordering::Relaxed);
        if late || closing {
            // The group is ours: its first process hasn't been waited for, so the number is still its.
            // SAFETY: plain signals to the process group this function started.
            unsafe { libc::kill(-group, libc::SIGTERM) };
            let grace = Instant::now() + Duration::from_secs(2);
            while Instant::now() < grace && matches!(child.try_wait(), Ok(None)) {
                thread::sleep(Duration::from_millis(20));
            }
            if matches!(child.try_wait(), Ok(None)) {
                // SAFETY: as above; the first process is still running, so the group is still the one we made.
                unsafe { libc::kill(-group, libc::SIGKILL) };
            }
            let _ = child.wait();
            break (None, late, closing && !late);
        }
        thread::sleep(Duration::from_millis(50));
    };
    let output = read_end(log);
    if cancelled {
        return Ran { cancelled: true, output, ..Ran::default() };
    }
    if timed_out {
        return Ran {
            summary: format!("timed out after {}", duration_words(timeout.as_secs())),
            output,
            timed_out: true,
            ..Ran::default()
        };
    }
    let code = status.and_then(|s| s.code());
    let ok = status.is_some_and(|s| s.success());
    Ran { ok, summary: summary(&output, code), output, ..Ran::default() }
}

// ---- the suggestion -----------------------------------------------------------------------------------------------

/// The words of the heading whose block the suggestion comes from (any level, any case).
const SECTION: &str = "build and test";

/// Lines of a block that set the shell up rather than test anything: passed over for the first command.
const SETUP_WORDS: [&str; 8] = ["export", "cd", "source", ".", "set", "unset", "alias", "pushd"];

/// The `CLAUDE.md` that speaks for `folder`: the folder's own, else the one at the top of its git repository (the first
/// folder above it with a `.git`). Looked up on disk only: no process.
pub fn claude_md_for(folder: &Path) -> Option<PathBuf> {
    let own = folder.join("CLAUDE.md");
    if own.is_file() {
        return Some(own);
    }
    let top = folder.ancestors().find(|d| d.join(".git").exists())?;
    Some(top.join("CLAUDE.md")).filter(|p| p.is_file())
}

/// The heading level of a Markdown heading line (`## Build and test` is 2), with its text.
fn heading(line: &str) -> Option<(usize, &str)> {
    let t = line.trim_start();
    let level = t.chars().take_while(|c| *c == '#').count();
    let rest = &t[level..];
    ((1..=6).contains(&level) && (rest.is_empty() || rest.starts_with(' '))).then(|| (level, rest.trim()))
}

/// A line that opens or closes a fenced block: its fence (the run of three or more backticks or tildes).
fn fence(line: &str) -> Option<String> {
    let t = line.trim_start();
    let c = t.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let run: String = t.chars().take_while(|x| *x == c).collect();
    (run.chars().count() >= 3).then_some(run)
}

/// `line` without a shell comment at its end (`cargo test   # ~40s` is `cargo test`): a `#` after a space, outside
/// quotes.
fn without_comment(line: &str) -> &str {
    let (mut single, mut double, mut prev_space) = (false, false, true);
    for (i, c) in line.char_indices() {
        match c {
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            '#' if !single && !double && prev_space => return line[..i].trim_end(),
            _ => {}
        }
        prev_space = c.is_whitespace();
    }
    line
}

/// The command a project's `CLAUDE.md` gives for testing: in the first section whose heading says "Build and test",
/// the first fenced block's first command (a `$ ` prompt taken off, comments and lines that only set the shell up,
/// such as `export PATH=...` or `cd`, passed over). None when there is no such section, block or command.
pub fn suggested_command(text: &str) -> Option<String> {
    let mut lines = text.lines();
    // The section.
    let level = loop {
        let line = lines.next()?;
        if let Some((level, title)) = heading(line)
            && title.to_lowercase().contains(SECTION)
        {
            break level;
        }
    };
    // Its first fenced block (a heading of the same level or above ends the section first).
    let open = loop {
        let line = lines.next()?;
        if let Some(f) = fence(line) {
            break f;
        }
        if heading(line).is_some_and(|(l, _)| l <= level) {
            return None;
        }
    };
    for line in lines {
        if fence(line).is_some_and(|f| f.starts_with(&open[..1]) && f.len() >= open.len()) {
            return None;
        }
        let cmd = without_comment(line.trim().trim_start_matches("$ ").trim());
        if cmd.is_empty() || cmd.starts_with('#') {
            continue;
        }
        let first = cmd.split_whitespace().next().unwrap_or("");
        if SETUP_WORDS.contains(&first) || (first.contains('=') && !first.starts_with('=') && cmd.split_whitespace().count() == 1) {
            continue;
        }
        let cmd = crate::projects::clean_command(cmd);
        return (!cmd.is_empty()).then_some(cmd);
    }
    None
}

/// The suggestion for a project whose folders are `folders` (most recently used first): from the `CLAUDE.md` of the
/// first one in git, as `suggested_command` reads it.
pub fn suggestion_for(folders: &[String]) -> Option<String> {
    let first = folders.iter().map(Path::new).find(|f| crate::squads::in_git(f))?;
    let text = std::fs::read_to_string(claude_md_for(first)?).ok()?;
    suggested_command(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tickets::{How, WorksOn, Worktree};
    use std::fs;

    fn dir(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-ut-checks-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn check(ok: bool, skipped: bool) -> CheckRun {
        CheckRun { kind: TESTS.into(), ok, skipped, ..CheckRun::default() }
    }

    #[test]
    fn only_the_three_stops_at_the_end_of_the_work_are_followed_by_a_check() {
        let stop = |msg: &str, running: bool| stop_of(&crate::squads::parse_end_line(msg), running);
        assert_eq!(stop("Done on task/3.\nSQUAD: question - Shall I close this task?", false), Some(Stop::Close));
        assert_eq!(stop("SQUAD: question - shall i close this ticket", false), Some(Stop::Close), "the old word, any case");
        assert_eq!(stop("SQUAD: ready to publish - push task/3", false), Some(Stop::Publish));
        assert_eq!(stop("Pushed.\nSQUAD: done", false), Some(Stop::Done));
        for other in ["SQUAD: question - which page?", "SQUAD: blocked - no database", "SQUAD: working", "SQUAD: plan ready", "no line"] {
            assert_eq!(stop(other, false), None, "{other}");
        }
        assert_eq!(stop("SQUAD: done", true), None, "members still running: not the end");
        assert_eq!((Stop::Close.id(), Stop::Publish.id(), Stop::Done.id()), ("close", "publish", "done"));
    }

    #[test]
    fn the_summary_adds_up_cargos_lines_else_says_the_last_line_cut_to_one() {
        let cargo = "running 3 tests\n\u{1b}[32mtest result: ok\u{1b}[0m. 200 passed; 0 failed; 1 ignored\n\n     Running tests/b.rs\ntest result: ok. 14 passed; 0 failed; 0 ignored; 0 measured\n   Doc-tests x\n";
        assert_eq!(summary(cargo, Some(0)), "214 passed");
        let failing = "test result: FAILED. 10 passed; 2 failed; 0 ignored\ntest result: ok. 201 passed; 1 failed\nerror: test failed, to rerun pass `--lib`\n";
        assert_eq!(summary(failing, Some(101)), "211 passed, 3 failed");
        assert_eq!(summary("compiling\n\nerror: could not compile `shop`\n\n", Some(101)), "error: could not compile `shop`");
        assert_eq!(summary("sh: 1: cargo: not found\n", Some(127)), "sh: 1: cargo: not found");
        let long = format!("{}\n", "word ".repeat(60));
        let s = summary(&long, Some(1));
        assert!(s.chars().count() == SUMMARY_MAX && s.ends_with('…') && !s.contains('\n'), "{s}");
        assert_eq!(summary("", Some(0)), "passed, with nothing to say");
        assert_eq!(summary("  \n", Some(2)), "failed with exit code 2, with nothing to say");
    }

    #[test]
    fn the_retry_rule_tells_the_squad_twice_then_needs_you_then_leaves_it_to_the_user() {
        assert_eq!(next_try(&[]), 1);
        assert_eq!(next_try(&[check(true, false)]), 1, "after a pass");
        assert_eq!(next_try(&[check(false, false)]), 2, "after one failure");
        assert_eq!(
            next_try(&[check(true, false), check(false, false), check(false, true), check(false, false)]),
            3,
            "a skip counts neither way"
        );
        assert_eq!(next_try(&[check(false, false), check(true, false)]), 1, "a pass starts again");
        // A pass, and a run that wasn't done, change nothing.
        assert_eq!(then(false, 1, true, true), CheckThen::Keep);
        // Fail once, twice: the squad is told and works again; with nobody to tell, the user is.
        assert_eq!(then(true, 1, true, true), CheckThen::Fix);
        assert_eq!(then(true, 2, true, true), CheckThen::Fix);
        assert_eq!(then(true, 1, true, false), CheckThen::NeedsYou(FAILED_NOT_RUNNING.into()));
        // The third failure: the user.
        assert_eq!(then(true, 3, true, true), CheckThen::NeedsYou(STILL_FAIL.into()));
        assert_eq!(then(true, 3, true, false), CheckThen::NeedsYou(STILL_FAIL.into()));
        // After that the user has been told: the task stays as the leader left it.
        assert_eq!(then(true, 4, true, true), CheckThen::Keep);
        // A task that moved on (the user wrote to it, or the squad stopped again) only keeps the result.
        assert_eq!(then(true, 1, false, true), CheckThen::Keep);
    }

    #[test]
    fn the_typed_message_says_what_ran_where_and_the_last_thirty_lines() {
        let output: String = (1..=50).map(|i| format!("line {i}\n")).collect();
        let ran = Ran { summary: "line 50".into(), output, ..Ran::default() };
        let m = typed_message("cargo test", "/home/u/code/shop-worktrees/task-5", &ran);
        let mut lines = m.lines();
        assert_eq!(lines.next(), Some("The tests failed when platoon ran `cargo test` in /home/u/code/shop-worktrees/task-5:"));
        assert!(m.contains("```\nline 21\n") && m.contains("line 50\n```"), "{m}");
        assert!(!m.contains("line 20\n"), "only the last 30 lines: {m}");
        assert_eq!(m.lines().filter(|l| l.starts_with("line ")).count(), TAIL_LINES);
        assert!(m.ends_with("Fix what makes them fail, then end your turn as before: platoon runs them again when you stop."), "{m}");
        // A timeout says so after what it wrote; with no output, the summary stands in for it.
        let late = Ran { summary: "timed out after 10 minutes".into(), output: "waiting\n".into(), timed_out: true, ..Ran::default() };
        assert!(typed_message("make", "/w", &late).contains("```\nwaiting\n```\n(timed out after 10 minutes)\n"));
        let quiet = Ran { summary: "failed with exit code 1, with nothing to say".into(), ..Ran::default() };
        assert!(typed_message("make", "/w", &quiet).contains(":\n\n(failed with exit code 1, with nothing to say)\n"));
        // A very long line is cut; colors are taken out.
        let long = Ran { output: format!("\u{1b}[31m{}\u{1b}[0m\n", "x".repeat(5000)), ..Ran::default() };
        let m = typed_message("make", "/w", &long);
        assert!(m.contains(&format!("```\n{}…\n```", "x".repeat(399))), "{}", m.len());
    }

    fn task(folders: &[&str]) -> Ticket {
        Ticket { number: 5, folders: folders.iter().map(|f| f.to_string()).collect(), ..Ticket::default() }
    }

    #[test]
    fn the_check_runs_in_the_tasks_copy_or_its_branch_in_the_checkout_while_head_is_still_on_it() {
        let d = dir("place");
        let copy = d.join("shop-worktrees/task-5/web");
        fs::create_dir_all(&copy).unwrap();
        let none = |_: &Path| -> Option<String> { panic!("no git asked for a copy") };
        // The first folder with a copy: its copy, and the task-5 folder as the one no other session may be in.
        let mut t = task(&["/home/u/notes", "/home/u/shop/web"]);
        t.worktrees.push(Worktree {
            folder: "/home/u/shop/web".into(),
            path: copy.to_string_lossy().into(),
            branch: "task/5".into(),
            ..Worktree::default()
        });
        assert!(has_place(&t));
        assert_eq!(place(&t, &none), Some(Place::Run { folder: copy.clone(), root: d.join("shop-worktrees/task-5") }));
        // A copy that is gone: not run, said why.
        fs::remove_dir_all(d.join("shop-worktrees")).unwrap();
        assert!(matches!(place(&t, &none), Some(Place::Skip { why, .. }) if why == "its copy of the folder isn't there any more"));
        // A removed copy is no place; a folder outside git is none either.
        t.worktrees[0].removed = Some(1);
        assert!(!has_place(&t));
        assert_eq!(place(&t, &none), None);
        // A branch in the user's own checkout: only while it is still on the task's branch.
        let shop = d.join("shop");
        fs::create_dir_all(&shop).unwrap();
        let mut t = task(&[&shop.to_string_lossy()]);
        t.works_on.push(WorksOn { folder: shop.to_string_lossy().into(), how: How::Branch, lands_on: "main".into() });
        assert!(has_place(&t));
        let on = |b: &'static str| move |_: &Path| Some(b.to_string());
        assert_eq!(place(&t, &on("task/5")), Some(Place::Run { folder: shop.clone(), root: shop.clone() }));
        assert_eq!(
            place(&t, &on("main")),
            Some(Place::Skip { folder: shop.clone(), why: "your checkout of shop isn't on task/5 any more".into() })
        );
        assert!(matches!(place(&t, &|_: &Path| None), Some(Place::Skip { .. })), "a detached HEAD, or git can't say");
        // Another session open in the copy (or a folder inside it) keeps the check out; one in a folder above doesn't.
        let root = Path::new("/home/u/shop-worktrees/task-5");
        assert!(open_there(root, &[PathBuf::from("/home/u/shop-worktrees/task-5/web")]));
        assert!(!open_there(root, &[PathBuf::from("/home/u"), PathBuf::from("/home/u/shop-worktrees/task-50")]));
    }

    #[test]
    fn the_environment_is_the_leaders_less_platoons_own_variables() {
        let vars = [("PATH", "/usr/bin"), ("HOME", "/home/u"), ("PLATOON_CONFIG_DIR", "/x"), ("PLATOON_IDLE_SECS_PER_MINUTE", "1")]
            .into_iter()
            .chain([("CLAUDE_CODE_SESSION_ID", "s"), ("CLAUDECODE", "1"), ("ANTHROPIC_API_KEY", "k"), ("CLAUDE_CONFIG_DIR", "/c")])
            .map(|(k, v)| (k.to_string(), v.to_string()));
        let names: Vec<String> = env_from(vars).into_iter().map(|(k, _)| k).collect();
        assert_eq!(names, ["PATH", "HOME", "ANTHROPIC_API_KEY", "CLAUDE_CONFIG_DIR"]);
    }

    fn path_env() -> Vec<(String, String)> {
        vec![("PATH".into(), std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())), ("FOO".into(), "bar".into())]
    }

    #[test]
    fn a_run_writes_its_log_and_says_how_it_went() {
        let d = dir("run");
        let never = AtomicBool::new(false);
        let log = d.join("work/tests-1.log");
        let ran =
            run("echo \"test result: ok. 3 passed; 0 failed\"; echo $FOO >&2", &d, &path_env(), Duration::from_secs(20), &log, &never);
        assert!(ran.ok && !ran.timed_out, "{ran:?}");
        assert_eq!(ran.summary, "3 passed");
        assert_eq!(
            fs::read_to_string(&log).unwrap(),
            "test result: ok. 3 passed; 0 failed\nbar\n",
            "stdout and stderr together, in the env given"
        );
        let ran = run("echo nope; exit 1", &d, &path_env(), Duration::from_secs(20), &log, &never);
        assert_eq!((ran.ok, ran.summary.as_str()), (false, "nope"));
        assert_eq!(fs::read_to_string(&log).unwrap(), "nope\n", "a run's log replaces the last one of that try");
        // In the folder it is given, with no terminal: stdin is empty.
        let ran = run("pwd; cat; echo [$PLATOON_CONFIG_DIR]", &d, &path_env(), Duration::from_secs(20), &log, &never);
        assert!(ran.ok && ran.output.starts_with(&format!("{}\n[]", d.to_string_lossy())), "{ran:?}");
    }

    #[test]
    fn a_command_that_isnt_there_fails_with_what_sh_says_and_a_folder_that_isnt_there_says_so() {
        let d = dir("notfound");
        let never = AtomicBool::new(false);
        let ran = run("no-such-command-for-platoon-tests", &d, &path_env(), Duration::from_secs(20), &d.join("t.log"), &never);
        assert!(!ran.ok && ran.summary.contains("no-such-command-for-platoon-tests") && ran.summary.contains("not found"), "{ran:?}");
        let ran = run("true", &d.join("gone"), &path_env(), Duration::from_secs(20), &d.join("t.log"), &never);
        assert!(!ran.ok && ran.not_started && ran.summary.starts_with("couldn't start it: "), "{ran:?}");
    }

    #[test]
    fn a_run_past_its_time_is_stopped_with_everything_it_started() {
        let d = dir("timeout");
        let never = AtomicBool::new(false);
        let started = Instant::now();
        // A child of the command, in the background: it goes with the group.
        let ran =
            run("sleep 60 & echo $! > child; echo started; sleep 60", &d, &path_env(), Duration::from_secs(1), &d.join("t.log"), &never);
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
        assert!(!ran.ok && ran.timed_out, "{ran:?}");
        assert_eq!(ran.summary, "timed out after 1 second");
        assert_eq!(ran.output, "started\n");
        let child: i32 = fs::read_to_string(d.join("child")).unwrap().trim().parse().unwrap();
        let gone = Instant::now() + Duration::from_secs(5);
        // SAFETY: signal 0 only asks whether the process is there.
        while unsafe { libc::kill(child, 0) } == 0 && Instant::now() < gone {
            thread::sleep(Duration::from_millis(20));
        }
        assert_ne!(unsafe { libc::kill(child, 0) }, 0, "the background child was ended too");
        assert_eq!(
            (duration_words(600), duration_words(60), duration_words(90)),
            ("10 minutes".into(), "1 minute".into(), "90 seconds".into())
        );
        // Closing platoon stops it too, and that is no result at all.
        let stop = AtomicBool::new(true);
        let ran = run("sleep 60", &d, &path_env(), Duration::from_secs(30), &d.join("t.log"), &stop);
        assert!(ran.cancelled && !ran.timed_out, "{ran:?}");
    }

    #[test]
    fn the_suggestion_is_the_first_command_of_the_build_and_test_block() {
        // Shaped like a real project's: a line that sets the shell up first, comments after the commands.
        let md = "# A project\n\nSome words.\n\n## Build and test\n\n```\nexport PATH=\"$HOME/.cargo/bin:$PATH\"     # each shell call starts fresh\ncargo test                               # ~40s\ncargo build --release\n```\n\n## Next\n\n```\nmake\n```\n";
        assert_eq!(suggested_command(md).as_deref(), Some("cargo test"));
        // Any level, any case; a `$ ` prompt and a `sh` fence; spaces inside quotes stay.
        let md = "### BUILD AND TEST\n\n~~~sh\n$ npm test -- --grep \"two  words\"\n~~~\n";
        assert_eq!(suggested_command(md).as_deref(), Some("npm test -- --grep \"two  words\""));
        // A `#` inside quotes is not a comment.
        let md = "## Build and test\n```\npytest -k 'not #slow'  # quick\n```\n";
        assert_eq!(suggested_command(md).as_deref(), Some("pytest -k 'not #slow'"));
    }

    #[test]
    fn no_section_no_block_or_an_empty_block_suggests_nothing() {
        assert_eq!(suggested_command("# Project\n\n```\ncargo test\n```\n"), None, "no Build and test section");
        assert_eq!(
            suggested_command("## Build and test\n\nRun the tests with cargo.\n\n## Next\n\n```\nmake\n```\n"),
            None,
            "no block in it"
        );
        assert_eq!(suggested_command("## Build and test\n\n```\n# only a comment\nexport X=1\n```\n"), None, "nothing to run in it");
        assert_eq!(suggested_command(""), None);
    }

    #[test]
    fn the_claude_md_is_the_folders_own_else_the_repositorys() {
        let base = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-ut-checks-md-{}", std::process::id())));
        let _ = fs::remove_dir_all(&base);
        let repo = base.join("shop");
        let sub = repo.join("web");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(&sub).unwrap();
        assert_eq!(claude_md_for(&sub), None, "none yet");
        fs::write(repo.join("CLAUDE.md"), "## Build and test\n\n```\nmake check\n```\n").unwrap();
        assert_eq!(claude_md_for(&sub), Some(repo.join("CLAUDE.md")), "the repository's");
        assert_eq!(suggestion_for(&[sub.to_string_lossy().into_owned()]).as_deref(), Some("make check"));
        fs::write(sub.join("CLAUDE.md"), "## Build and test\n\n```\nnpm test\n```\n").unwrap();
        assert_eq!(claude_md_for(&sub), Some(sub.join("CLAUDE.md")), "the folder's own first");
        // Only a folder in git is asked: the first one there is.
        let loose = base.join("notes");
        fs::create_dir_all(&loose).unwrap();
        fs::write(loose.join("CLAUDE.md"), "## Build and test\n\n```\nrm -rf /\n```\n").unwrap();
        let folders = [loose.to_string_lossy().into_owned(), sub.to_string_lossy().into_owned()];
        assert_eq!(suggestion_for(&folders).as_deref(), Some("npm test"));
        assert_eq!(suggestion_for(&folders[..1]), None, "a folder outside git suggests nothing");
        let _ = fs::remove_dir_all(&base);
    }
}
