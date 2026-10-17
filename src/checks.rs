//! The project's tests, run where a task's session works when its turn ends (the `task-stop` hook, `taskrun.rs`): a
//! project may name a **test command** (`test_command` in `projects.json`, with `test_timeout_minutes`). `run` runs it
//! through `sh -c` in the task's folder, with platoon's environment less its own variables (`env`), no terminal, its
//! output kept in a log, its whole process group ended when its time is up. `summary` and `tail` say how it went.
//!
//! Also here: a task's "Done when" boxes as its text has them (`done_when`), and the suggestion on the project's
//! screen (the first command of the "Build and test" block in the `CLAUDE.md` of the project's first git folder;
//! nothing is ever run from a suggestion).

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

/// How many of the output's last lines go back to the session with a failure.
pub const TAIL_LINES: usize = 30;

/// The longest summary kept, in characters.
pub const SUMMARY_MAX: usize = 120;

// ---- "Done when" ----------------------------------------------------------------------------------------------------

/// A line's words as they are compared: lower case letters and digits, one space between.
fn words_of(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

/// A checkbox line's text and whether it is ticked: `- [ ] text`, `* [x] text` (any case, any of `-*+`, or numbered).
fn checkbox(line: &str) -> Option<(bool, &str)> {
    let t = line.trim_start();
    let t = t.strip_prefix(['-', '*', '+']).map(str::trim_start).or_else(|| {
        let digits = t.chars().take_while(char::is_ascii_digit).count();
        (digits > 0).then(|| t[digits..].strip_prefix(['.', ')'])).flatten().map(str::trim_start)
    })?;
    let ticked = if t.starts_with("[ ]") {
        false
    } else if t.starts_with("[x]") || t.starts_with("[X]") {
        true
    } else {
        return None;
    };
    Some((ticked, t[3..].trim()))
}

/// A task's "Done when" boxes, as written: the checkbox lines under a heading that says "done when" (until the next
/// heading of the same level or above), with whether each is ticked. Empty ones (the template's) are left out.
pub fn done_when(body: &str) -> Vec<(bool, String)> {
    let heading = |l: &str| {
        let t = l.trim_start();
        let level = t.chars().take_while(|c| *c == '#').count();
        ((1..=6).contains(&level) && t[level..].starts_with(' ')).then(|| (level, t[level..].trim().to_lowercase()))
    };
    let mut under: Option<usize> = None;
    let mut out = Vec::new();
    for l in body.lines() {
        if let Some((level, text)) = heading(l) {
            if text.contains("done when") {
                under = Some(level);
            } else if under.is_some_and(|u| level <= u) {
                under = None;
            }
            continue;
        }
        if under.is_some()
            && let Some((ticked, text)) = checkbox(l)
            && !words_of(text).is_empty()
        {
            out.push((ticked, text.to_string()));
        }
    }
    out
}

// ---- the summary ----------------------------------------------------------------------------------------------------

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

/// The longest line of output handed back to the task's session, in characters (a minified file on one line stays
/// short).
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

// ---- running it ---------------------------------------------------------------------------------------------------

/// The environment the command runs with: the one the task's session gives its Stop hook, with the variables that
/// describe a Claude Code session taken out (`claude::is_session_marker`) and every `PLATOON_*` variable too
/// (platoon's folders and test knobs are not the project's business).
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
    let first = folders.iter().map(Path::new).find(|f| crate::worktrees::in_git(f))?;
    let text = std::fs::read_to_string(claude_md_for(first)?).ok()?;
    suggested_command(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-ut-checks-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_done_when_boxes_are_the_ones_under_their_heading_as_written() {
        let body = "## What it should do\n\n- [ ] not a box of done when\n\n## Done when\n\n- [x] The page loads\n- [ ] A test covers it\n- [ ] \n\n## Notes\n- [ ] after\n";
        assert_eq!(done_when(body), [(true, "The page loads".to_string()), (false, "A test covers it".to_string())]);
        assert!(done_when(crate::tickets::TEMPLATE).is_empty(), "the template's empty boxes");
        assert!(done_when("- [ ] no heading").is_empty());
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
    fn the_environment_is_the_sessions_less_platoons_own_variables() {
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
