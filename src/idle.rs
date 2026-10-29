//! Idle stop's rules, shared by a session's host (`host::Watchdog`, for its own session) and the app (for sessions
//! whose host an older platoon started, which keeps that platoon's rules: `check_older`).
//!
//! Stopping an idle session loses nothing (the conversation is saved, and opening it resumes it), except what the agent
//! started that is still running: stopping the agent ends it. So the one question about what runs under the agent is
//! whether a command it started is still going. Claude Code runs every command of its Bash tool through a shell that
//! is a child of its own (`bash -c source <its shell snapshot> ... eval '<command>'`), and a background command stays
//! under that shell; Codex runs its commands as `bash -lc <command>` (`zsh` on macOS, the user's shell), also children
//! of its own. Either may start the shell inside a sandbox (`sandbox-exec` on macOS, bubblewrap on Linux, Codex's
//! `codex-linux-sandbox`). So a direct child of the agent that is a shell running a command (`-c`) or such a sandbox is
//! a command the agent started, and it keeps the session, whether or not Claude Code's snapshot is in it: unsure
//! counts as work. Everything else under the agent never does: its MCP servers (however they are launched: a script,
//! `npx`, `uvx`, a bare program, a script that `exec`s another), its own helpers, and what those start.
//!
//! Hooks and the status line are shells too (`/bin/sh -c <command>`), but only for a moment, while the agent is quiet;
//! a command the agent started was started while it worked, so it has been running at least as long as the session has
//! been quiet. A shell younger than `settle` (half a minute) is therefore not counted yet. Where a process's age can't
//! be read it counts.

use crate::agents::Agent;
use crate::host::HostInfo;
use crate::mcp::Tracked;
use crate::procs::{self, Row};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    time::{Duration, SystemTime},
};

/// Shells that run a command given with `-c`.
const SHELLS: [&str; 6] = ["sh", "bash", "zsh", "dash", "fish", "ksh"];
/// Sandboxes a command may be started inside: macOS's, bubblewrap, and Codex's own Linux helper (which becomes
/// bubblewrap).
const SANDBOXES: [&str; 3] = ["sandbox-exec", "bwrap", "codex-linux-sandbox"];
/// The program words a command's short form keeps: its program and at most this many words after it.
const BRIEF_WORDS: usize = 2;
/// The longest word a command's short form shows.
const BRIEF_WORD_MAX: usize = 24;

/// A program's own name: the last part of its path, without the `-` a login shell's name starts with.
fn program(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word).trim_start_matches('-')
}

/// A command line as words. Where only `ps` could say (macOS, another user's process) it comes as one string.
fn words(argv: &[String]) -> (Vec<String>, bool) {
    if argv.len() == 1 && argv[0].contains(' ') {
        (argv[0].split_whitespace().map(String::from).collect(), true)
    } else {
        (argv.to_vec(), false)
    }
}

/// Where the command text is, when `words[at]` is a shell started with `-c` (`-c`, `-lc`, `-ic`, ...): the index of the
/// first word after its options.
fn shell_text_at(words: &[String], at: usize) -> Option<usize> {
    if !SHELLS.contains(&program(&words[at])) {
        return None;
    }
    let mut has_c = false;
    for (i, w) in words.iter().enumerate().skip(at + 1) {
        if w.starts_with('-') && !w.starts_with("--") && w.len() > 1 {
            has_c |= w[1..].contains('c');
        } else if w.starts_with("--") {
            continue;
        } else {
            return has_c.then_some(i);
        }
    }
    None
}

/// Is this command line a command the agent started: a shell running one, or a sandbox around one?
pub fn is_command(argv: &[String]) -> bool {
    let (w, _) = words(argv);
    !w.is_empty() && (SANDBOXES.contains(&program(&w[0])) || shell_text_at(&w, 0).is_some())
}

/// A command's short form, for saying what keeps a session: its program and its first word or two (`npm run dev`),
/// never all its arguments (they can hold tokens). None when there is nothing to say it by.
pub fn summary(argv: &[String]) -> Option<String> {
    let (w, one_string) = words(argv);
    let text = (0..w.len()).find_map(|at| shell_text_at(&w, at)).map(|i| if one_string { w[i..].join(" ") } else { w[i].clone() });
    match text {
        Some(t) => brief(&t),
        // A sandbox whose command isn't a shell's: the program it runs is somewhere in its arguments; not guessed.
        None => None,
    }
}

/// A piece of shell text, read as words and the operators between commands.
#[derive(Debug, PartialEq)]
enum Token {
    Word(String),
    Op,
}

/// Shell text into words (quotes and backslashes taken off) and operators (`&&`, `||`, `;`, `|`, `&`, a new line,
/// brackets). Good enough to find the first command in it; nothing is run.
fn tokens(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = text.chars().peekable();
    let end = |out: &mut Vec<Token>, word: &mut String, in_word: &mut bool| {
        if *in_word {
            out.push(Token::Word(std::mem::take(word)));
            *in_word = false;
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    word.push(q);
                }
            }
            '"' => {
                in_word = true;
                while let Some(q) = chars.next() {
                    match q {
                        '"' => break,
                        '\\' => word.extend(chars.next()),
                        q => word.push(q),
                    }
                }
            }
            '\\' => {
                in_word = true;
                word.extend(chars.next());
            }
            c if c.is_whitespace() && c != '\n' => end(&mut out, &mut word, &mut in_word),
            '\n' | ';' | '|' | '&' | '(' | ')' | '{' | '}' => {
                end(&mut out, &mut word, &mut in_word);
                if out.last() != Some(&Token::Op) {
                    out.push(Token::Op);
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    end(&mut out, &mut word, &mut in_word);
    out
}

/// Commands that set things up rather than do the work: a command made only of these is passed over.
const SETUP: [&str; 12] = ["cd", "source", ".", "export", "set", "shopt", "true", "unalias", "unset", "builtin", "pushd", "pwd"];
/// Words that run the rest of the command: the program after them is the one that counts.
const RUNNERS: [&str; 4] = ["nohup", "exec", "env", "command"];

/// The short form of the first command in shell text that does the work. Claude Code's own wrapping
/// (`source <snapshot> ... && eval '<command>' && pwd -P >| ...`) is looked through to the command it evaluates.
fn brief(text: &str) -> Option<String> {
    let toks = tokens(text);
    if let Some(i) = toks.iter().position(|t| *t == Token::Word("eval".into()))
        && let Some(Token::Word(inner)) = toks.get(i + 1)
    {
        return brief(inner);
    }
    for cmd in toks.split(|t| *t == Token::Op) {
        let mut ws = cmd.iter().filter_map(|t| if let Token::Word(w) = t { Some(w.as_str()) } else { None }).peekable();
        while ws.peek().is_some_and(|w| RUNNERS.contains(w) || (w.contains('=') && !w.starts_with('='))) {
            ws.next();
        }
        let Some(first) = ws.next() else { continue };
        let name = program(first);
        if SETUP.contains(&name) || first.starts_with('\\') || name.is_empty() {
            continue;
        }
        let plain = |w: &&str| {
            w.len() <= BRIEF_WORD_MAX && !w.starts_with('-') && w.chars().all(|c| c.is_ascii_alphanumeric() || "._:@+-".contains(c))
        };
        let mut out = vec![name.to_string()];
        out.extend(ws.take_while(plain).take(BRIEF_WORDS).map(String::from));
        return Some(out.join(" "));
    }
    None
}

/// A command the agent started that is still running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub pid: u32,
    /// Its short form (`summary`), when it has one.
    pub what: Option<String>,
}

/// What runs under a session's agent, sorted for idle stop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Under {
    /// A command the agent started that is still running (the one with the lowest pid, if there are several): it keeps
    /// the session.
    pub command: Option<Command>,
    /// Everything else (its MCP servers and helpers, and what they started), as it was when looked at: what is asked to
    /// leave after a stop if it doesn't go with the agent (`mcp::end_leftovers`).
    pub leftovers: Vec<Tracked>,
}

/// What runs under the agent of `agent` whose process is `pid`, in the process table `rows` (`argv_of` reads a command
/// line). A shell younger than `settle` is a hook or the status line, for a moment: neither work nor left over. None
/// when `pid` isn't in the table: then nothing can be said.
pub fn under(agent: Agent, rows: &[Row], pid: u32, argv_of: &dyn Fn(u32) -> Option<Vec<String>>, settle: Duration) -> Option<Under> {
    if !rows.iter().any(|r| r.pid == pid) {
        return None;
    }
    let children = |p: u32| {
        let mut v: Vec<&Row> = rows.iter().filter(|r| r.ppid == p && r.pid != p && !r.zombie).collect();
        v.sort_by_key(|r| r.pid);
        v
    };
    // The agent itself: its process and, for Codex, the program its launcher runs (npm's `codex` starts the real one as
    // its child; the commands are that one's children).
    let mut agent_pids = vec![pid];
    if agent == Agent::Codex {
        for r in children(pid) {
            if argv_of(r.pid).is_some_and(|a| words(&a).0.first().is_some_and(|w| program(w) == "codex")) {
                agent_pids.push(r.pid);
            }
        }
    }
    let (mut command, mut leftovers) = (None, Vec::new());
    for &a in &agent_pids {
        for child in children(a).into_iter().filter(|r| !agent_pids.contains(&r.pid)) {
            let argv = argv_of(child.pid);
            // A command line that can't be read might be anything: counted as a command.
            let is_cmd = argv.as_deref().is_none_or(is_command);
            if is_cmd {
                let young = child.age.is_some_and(|age| age < settle);
                if !young && command.is_none() {
                    command = Some(Command { pid: child.pid, what: argv.as_deref().and_then(summary) });
                }
                continue;
            }
            leftovers.push(Tracked { pid: child.pid, start: child.start, argv });
            for d in procs::descendants(rows, child.pid).unwrap_or_default() {
                if let Some(r) = rows.iter().find(|r| r.pid == d) {
                    leftovers.push(Tracked { pid: d, start: r.start, argv: argv_of(d) });
                }
            }
        }
    }
    Some(Under { command, leftovers })
}

/// Why a session that has been quiet for the idle-stop time is still running, in words the session's row can show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kept {
    /// A command the agent started is still going (its short form, when it has one).
    Command(Option<String>),
    /// A window is open on it.
    Window,
    /// The agent doesn't say it is idle.
    NotIdle,
    /// A task's session whose task is being worked on.
    TaskAtWork,
    /// What runs under it couldn't be seen.
    Unknown,
}

impl Kept {
    /// The line a session's row shows: "Kept running: a command Claude started is still going (npm run dev)".
    pub fn words(&self, agent: Agent) -> String {
        let (who, program) = match agent {
            Agent::Claude => ("Claude", "Claude Code"),
            Agent::Codex => ("Codex", "Codex"),
        };
        let why = match self {
            Kept::Command(Some(what)) => format!("a command {who} started is still going ({what})"),
            Kept::Command(None) => format!("a command {who} started is still going"),
            Kept::Window => "a window is open on it".into(),
            Kept::NotIdle => format!("{program} doesn't say it's idle"),
            Kept::TaskAtWork => "its task is still being worked on".into(),
            Kept::Unknown => "platoon couldn't see what it is running".into(),
        };
        format!("Kept running: {why}")
    }

    /// A word for it, for the diagnostics log.
    pub fn word(&self) -> &'static str {
        match self {
            Kept::Command(_) => "command",
            Kept::Window => "window",
            Kept::NotIdle => "not-idle",
            Kept::TaskAtWork => "task-at-work",
            Kept::Unknown => "unknown",
        }
    }

    /// As a host's file keeps it (`"kept"`).
    pub fn to_json(&self) -> Value {
        match self {
            Kept::Command(what) => json!({"why": "command", "what": what}),
            Kept::Window => json!({"why": "window"}),
            Kept::NotIdle => json!({"why": "not-idle"}),
            Kept::TaskAtWork => json!({"why": "task"}),
            Kept::Unknown => json!({"why": "unknown"}),
        }
    }

    /// From a host's file. Anything else (a newer platoon's reason) is no reason platoon can say.
    pub fn from_json(v: &Value) -> Option<Self> {
        Some(match v.get("why")?.as_str()? {
            "command" => Kept::Command(v.get("what").and_then(Value::as_str).map(String::from)),
            "window" => Kept::Window,
            "not-idle" => Kept::NotIdle,
            "task" => Kept::TaskAtWork,
            "unknown" => Kept::Unknown,
            _ => return None,
        })
    }
}

// ---- sessions whose host an older platoon started ----------------------------------------------------------------

/// A host of a platoon older than this one: its file names no version (from before hosts said it), or an older one. Its
/// own idle stop keeps the rules it was built with, so the app applies these to it from outside (`check_older`). A
/// version that can't be read is not taken for older.
pub fn older(version: Option<&str>) -> bool {
    version.is_none_or(|v| crate::versions::newer(crate::versions::running(), v))
}

/// What the app decides about a session of an older host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Stop,
    /// Not now, and why when it is worth saying (None: it hasn't been idle for the idle-stop time yet).
    Keep(Option<Kept>),
}

/// The rules for a session of an older host, from what can be seen from outside it: how long its agent has said it is
/// idle (`idle_for`), whether a window is open on it, whether its task is being worked on, and what runs under it
/// (None: couldn't be seen).
pub fn judge(threshold: Duration, idle_for: Option<Duration>, window: bool, task_at_work: bool, under: Option<&Under>) -> Verdict {
    if idle_for.is_none_or(|d| d < threshold) {
        return Verdict::Keep(None);
    }
    if window {
        return Verdict::Keep(Some(Kept::Window));
    }
    if task_at_work {
        return Verdict::Keep(Some(Kept::TaskAtWork));
    }
    match under {
        None => Verdict::Keep(Some(Kept::Unknown)),
        Some(Under { command: Some(c), .. }) => Verdict::Keep(Some(Kept::Command(c.what.clone()))),
        Some(Under { command: None, .. }) => Verdict::Stop,
    }
}

/// An older host the app looks at: what it runs, and the session (from its arguments, or the app's row for it).
#[derive(Clone, Debug)]
pub struct OldHost {
    pub info: HostInfo,
    pub session: Option<String>,
}

/// Where and by what rules the app looks at older hosts.
#[derive(Clone, Debug)]
pub struct Rules {
    /// The idle-stop time (the setting).
    pub threshold: Duration,
    /// How long a shell must have run to count (see the module's notes).
    pub settle: Duration,
    pub claude_root: PathBuf,
    /// Codex's folder, when Codex is used here.
    pub codex_root: Option<PathBuf>,
    /// platoon's config folder, where the tasks are.
    pub config: PathBuf,
    /// The session the app itself has open (its agent's pid).
    pub attached: Option<u32>,
}

/// `platoon open` windows running now: each one's pid and the words after `open` (a session id, or its start, when it
/// named one).
fn open_windows(rows: &[Row], argv_of: &dyn Fn(u32) -> Option<Vec<String>>) -> Vec<(u32, Vec<String>)> {
    rows.iter()
        .filter(|r| !r.zombie)
        .filter_map(|r| {
            let (w, _) = words(&argv_of(r.pid)?);
            (w.len() >= 2 && program(&w[0]).starts_with("platoon") && w[1] == "open").then(|| (r.pid, w[2..].to_vec()))
        })
        .collect()
}

/// Is a `platoon open` window on this host: one that started it (its parent), or one that names its session?
fn window_open(opens: &[(u32, Vec<String>)], host_parent: Option<u32>, session: Option<&str>) -> bool {
    opens.iter().any(|(pid, args)| {
        Some(*pid) == host_parent
            || session.is_some_and(|s| args.iter().any(|a| !a.starts_with('-') && a.len() >= 4 && s.starts_with(a.as_str())))
    })
}

/// Apply the rules to each older host, stop the sessions they say to stop, and say why each other one is kept. Stopping
/// is SIGTERM to the agent's process, and only when it is still the direct child of that host (a pid used again since
/// is never touched); nothing else is signaled. Blocks for the reads: run it off the UI thread.
pub fn check_older(hosts: &[OldHost], rules: &Rules) -> Vec<(u32, Option<Kept>)> {
    let rows = procs::table();
    let argv_of = |p: u32| procs::cmdline(p);
    // Every process is read here, so never through `ps` one at a time (a `platoon open` window is the user's own).
    let opens = rows.as_deref().map(|r| open_windows(r, &procs::cmdline_quick)).unwrap_or_default();
    let now = SystemTime::now();
    let mut out = Vec::new();
    for h in hosts {
        let (agent, pid) = (h.info.agent, h.info.claude_pid);
        let root = match agent {
            Agent::Claude => Some(rules.claude_root.as_path()),
            Agent::Codex => rules.codex_root.as_deref(),
        };
        let Some(root) = root else {
            out.push((pid, None));
            continue;
        };
        let session = h.session.as_deref();
        let idle_for = agent.idle_since(root, pid, session).and_then(|t| now.duration_since(t).ok());
        let parent = rows.as_deref().and_then(|r| r.iter().find(|r| r.pid == h.info.host_pid)).map(|r| r.ppid);
        let window = rules.attached == Some(pid) || window_open(&opens, parent, session);
        let at_work = session.is_some_and(|s| crate::tickets::session_at_work(&rules.config, s));
        let under = rows.as_deref().and_then(|r| under(agent, r, pid, &argv_of, rules.settle));
        let verdict = judge(rules.threshold, idle_for, window, at_work, under.as_ref());
        let (event, why) = match &verdict {
            Verdict::Stop => ("stop", "idle"),
            Verdict::Keep(None) => ("keep", "not-idle-long-enough"),
            Verdict::Keep(Some(k)) => ("keep", k.word()),
        };
        let program = match &verdict {
            Verdict::Keep(Some(Kept::Command(Some(w)))) => crate::diag::Program::named(w),
            _ => None,
        };
        let version = h.info.version.as_deref().map(crate::diag::ident);
        let fields =
            [("agent", agent.into()), ("pid", pid.into()), ("version", version.into()), ("why", why.into()), ("program", program.into())];
        crate::diag::changed("idle-older", u64::from(pid), event, &fields, &[("idle_for", idle_for.into())]);
        match verdict {
            Verdict::Stop => {
                let sent = stop_child(pid, h.info.host_pid);
                crate::diag::decision("idle-older", "stopped", &[("agent", agent.into()), ("pid", pid.into()), ("signalled", sent.into())]);
                out.push((pid, None));
            }
            Verdict::Keep(why) => out.push((pid, why)),
        }
    }
    out
}

/// SIGTERM `pid` if it is still the direct child of `host` (as the host's own idle stop checks): a pid used again since
/// belongs to somebody else and is left alone. True if the signal was sent.
pub fn stop_child(pid: u32, host: u32) -> bool {
    pid > 1 && pid != std::process::id() && procs::ppid(pid) == Some(host) && unsafe { libc::kill(pid as i32, libc::SIGTERM) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, fs};

    const SETTLE: Duration = Duration::from_secs(30);

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// A process of the table: running for `age` seconds.
    fn row(pid: u32, ppid: u32, age: u64) -> Row {
        Row { pid, ppid, zombie: false, start: 1000 + u64::from(pid), age: Some(Duration::from_secs(age)) }
    }

    /// The agent is pid 10; `argv` by pid.
    fn look(agent: Agent, rows: &[Row], argv: &[(u32, &[&str])]) -> Under {
        let table: HashMap<u32, Vec<String>> = argv.iter().map(|(p, a)| (*p, strs(a))).collect();
        under(agent, rows, 10, &|p| table.get(&p).cloned(), SETTLE).unwrap()
    }

    fn pids(u: &Under) -> Vec<u32> {
        let mut v: Vec<u32> = u.leftovers.iter().map(|t| t.pid).collect();
        v.sort();
        v
    }

    /// What Claude Code's Bash tool runs (seen with Claude Code 2.1.312), around `command`.
    fn claude_tool(command: &str) -> Vec<String> {
        let quoted = command.replace('\'', r#"'"'"'"#);
        vec![
            "/bin/bash".into(),
            "-c".into(),
            format!(
                "source /home/u/.claude/shell-snapshots/snapshot-bash-1793150317529-pdysz3.sh 2>/dev/null || true && shopt -u extglob \
                 2>/dev/null || true && {{ \\builtin unalias -- 'unsetenv'; \\builtin unset -f -- 'unsetenv'; }} >/dev/null 2>&1 || \
                 true && eval '{quoted}' && pwd -P >| /tmp/claude-a8de-cwd"
            ),
        ]
    }

    #[test]
    fn mcp_servers_however_they_are_launched_never_keep_a_session() {
        // A wrapper script that `exec`s the server (it runs as node + the server's script), `npm exec` for `npx -y`
        // (which runs the package through `sh -c` of its own), `uvx`, a bare server program.
        let rows = [
            row(10, 1, 9000),
            row(11, 10, 9000),
            row(12, 10, 9000),
            row(13, 12, 9000),
            row(14, 13, 9000),
            row(15, 10, 9000),
            row(16, 15, 9000),
            row(17, 10, 9000),
        ];
        let u = look(
            Agent::Claude,
            &rows,
            &[
                (11, &["node", "/opt/homebrew/bin/mcp-gitlab"]),
                (12, &["npm", "exec", "@vertica/mcp-server"]),
                (13, &["sh", "-c", "mcp-server-vertica"]),
                (14, &["node", "/home/u/.npm/_npx/1/node_modules/.bin/mcp-server-vertica"]),
                (15, &["/home/u/.local/bin/uv", "tool", "uvx", "mcp-server-fetch"]),
                (16, &["/home/u/.cache/uv/archive-v0/x/bin/python", "/home/u/.cache/uv/archive-v0/x/bin/mcp-server-fetch"]),
                (17, &["mcp-grafana"]),
            ],
        );
        assert_eq!(u.command, None);
        assert_eq!(pids(&u), [11, 12, 13, 14, 15, 16, 17], "each recorded, with what it started, for the clean-up after a stop");
        assert!(u.leftovers.iter().all(|t| t.start != 0 && t.argv.is_some()));
        // A wrapper script that doesn't `exec`: the script's shell has no `-c`, so it is the server too.
        let u = look(
            Agent::Claude,
            &[row(10, 1, 9000), row(11, 10, 9000), row(12, 11, 9000)],
            &[(11, &["/bin/bash", "/repo/scripts/gitlab-mcp.sh"]), (12, &["node", "x"])],
        );
        assert_eq!((u.command.clone(), pids(&u)), (None, vec![11, 12]));
    }

    #[test]
    fn a_tool_shell_with_a_background_command_under_it_keeps_the_session() {
        let rows = [row(10, 1, 9000), row(11, 10, 9000), row(12, 11, 8000), row(13, 12, 8000), row(14, 10, 9000)];
        let tool = claude_tool("npm run dev --port 4000 &");
        let tool: Vec<&str> = tool.iter().map(String::as_str).collect();
        let u = look(Agent::Claude, &rows, &[(11, &tool), (12, &["npm", "run", "dev"]), (13, &["node", "vite"]), (14, &["mcp-grafana"])]);
        assert_eq!(u.command, Some(Command { pid: 11, what: Some("npm run dev".into()) }));
        assert_eq!(pids(&u), [14], "the MCP server is still recorded");
        // A shell running a command without Claude Code's snapshot counts too (Codex's way, or anything else's).
        let u = look(Agent::Claude, &rows[..2], &[(11, &["zsh", "-lc", "cargo watch -x test"])]);
        assert_eq!(u.command, Some(Command { pid: 11, what: Some("cargo watch".into()) }));
    }

    #[test]
    fn a_command_in_a_sandbox_keeps_the_session() {
        let rows = [row(10, 1, 9000), row(11, 10, 9000), row(12, 11, 9000)];
        let mac =
            ["/usr/bin/sandbox-exec", "-p", "(version 1)", "-D", "TMPDIR=/tmp", "--", "/bin/zsh", "-lc", "python3 -m http.server 8000"];
        let u = look(Agent::Codex, &rows, &[(11, &mac), (12, &["python3", "-m", "http.server"])]);
        assert_eq!(u.command, Some(Command { pid: 11, what: Some("python3".into()) }));
        let linux = ["bwrap", "--ro-bind", "/", "/", "--", "/bin/bash", "-c", "make serve"];
        assert_eq!(look(Agent::Claude, &rows[..2], &[(11, &linux)]).command, Some(Command { pid: 11, what: Some("make serve".into()) }));
        let codex = ["codex-linux-sandbox", "--sandbox-policy", "{}", "--", "bash", "-lc", "x"];
        assert_eq!(look(Agent::Codex, &rows[..2], &[(11, &codex)]).command.map(|c| c.pid), Some(11));
        assert_eq!(
            look(Agent::Codex, &rows[..2], &[(11, &["/usr/bin/sandbox-exec", "-f", "p.sb", "server"])]).command,
            Some(Command { pid: 11, what: None })
        );
    }

    #[test]
    fn a_short_lived_hook_or_status_line_does_not_keep_the_session() {
        // Claude Code runs a hook as `/bin/sh -c <its command>` (seen with 2.1.312), and the status line likewise.
        let rows = [row(10, 1, 9000), row(11, 10, 1), row(12, 11, 1), row(13, 10, 0)];
        let u = look(
            Agent::Claude,
            &rows,
            &[
                (11, &["/bin/sh", "-c", "/usr/local/bin/platoon hook stop"]),
                (12, &["platoon", "hook", "stop"]),
                (13, &["/bin/sh", "-c", "platoon statusline"]),
            ],
        );
        assert_eq!(u.command, None);
        assert_eq!(pids(&u), Vec::<u32>::new(), "nor is it a leftover");
        // One that has run for half a minute is not a hook any more, as far as platoon can tell: it counts.
        let rows = [row(10, 1, 9000), row(11, 10, 30)];
        assert_eq!(look(Agent::Claude, &rows, &[(11, &["/bin/sh", "-c", "sleep 9999"])]).command.map(|c| c.pid), Some(11));
        // An age that couldn't be read counts as old.
        let rows = [row(10, 1, 9000), Row { age: None, ..row(11, 10, 0) }];
        assert_eq!(look(Agent::Claude, &rows, &[(11, &["/bin/sh", "-c", "x"])]).command.map(|c| c.pid), Some(11));
    }

    #[test]
    fn only_a_direct_child_of_the_agent_is_its_command_and_codexs_launcher_is_looked_through() {
        // A shell running a command under an MCP server is the server's own business.
        let rows = [row(10, 1, 9000), row(11, 10, 9000), row(12, 11, 9000)];
        let u = look(Agent::Claude, &rows, &[(11, &["uvx", "srv"]), (12, &["bash", "-c", "serve"])]);
        assert_eq!((u.command.clone(), pids(&u)), (None, vec![11, 12]));
        // npm's `codex` is node running a script that starts the real `codex`, whose children the commands are.
        let rows = [row(10, 1, 9000), row(11, 10, 9000), row(12, 11, 9000), row(13, 11, 9000)];
        let u = look(
            Agent::Codex,
            &rows,
            &[
                (11, &["/usr/lib/node_modules/@openai/codex/vendor/x86_64-unknown-linux-musl/codex/codex", "resume"]),
                (12, &["/bin/bash", "-lc", "npm start"]),
                (13, &["docs-server"]),
            ],
        );
        assert_eq!(u.command, Some(Command { pid: 12, what: Some("npm start".into()) }));
        assert_eq!(pids(&u), [13]);
        // Claude Code's own process is never looked through.
        let u = look(Agent::Claude, &rows, &[(11, &["codex"]), (12, &["/bin/bash", "-lc", "npm start"]), (13, &["docs-server"])]);
        assert_eq!((u.command.clone(), pids(&u)), (None, vec![11, 12, 13]));
    }

    #[test]
    fn an_unreadable_child_counts_finished_ones_do_not_and_an_agent_not_in_the_table_is_unknown() {
        let rows = [row(10, 1, 9000), row(11, 10, 9000)];
        assert_eq!(look(Agent::Claude, &rows, &[]).command, Some(Command { pid: 11, what: None }));
        let zombie = Row { zombie: true, ..row(11, 10, 9000) };
        let u = look(Agent::Claude, &[row(10, 1, 9000), zombie], &[(11, &["bash", "-c", "x"])]);
        assert_eq!((u.command, u.leftovers.len()), (None, 0));
        assert_eq!(under(Agent::Claude, &rows, 99, &|_| None, SETTLE), None);
        // The first command by pid is the one named.
        let rows = [row(10, 1, 9000), row(15, 10, 9000), row(12, 10, 9000)];
        assert_eq!(look(Agent::Claude, &rows, &[(15, &["sh", "-c", "a"]), (12, &["sh", "-c", "b"])]).command.map(|c| c.pid), Some(12));
    }

    #[test]
    fn a_command_is_named_by_its_program_and_first_words_never_its_arguments() {
        let say = |argv: Vec<String>| summary(&argv);
        assert_eq!(say(claude_tool("npm run dev")), Some("npm run dev".into()));
        assert_eq!(say(claude_tool("cd /x/app && PORT=3000 npm run dev -- --host")), Some("npm run dev".into()));
        assert_eq!(say(claude_tool("nohup ./bin/server --token sk-secret-123 &")), Some("server".into()));
        assert_eq!(say(claude_tool("curl -H 'Authorization: Bearer abc' https://x")), Some("curl".into()));
        assert_eq!(say(claude_tool("docker compose up -d")), Some("docker compose up".into()));
        assert_eq!(say(claude_tool("python3 -I - <<'EOF'\nprint(1)\nEOF")), Some("python3".into()));
        assert_eq!(say(claude_tool("tail -f log | grep x")), Some("tail".into()));
        assert_eq!(say(strs(&["bash", "-lc", "export A=1; cargo   run"])), Some("cargo run".into()));
        assert_eq!(say(strs(&["sh", "-c", "cd /x"])), None, "nothing but set-up");
        assert_eq!(say(strs(&["sh", "-c", "  "])), None);
        assert_eq!(
            say(strs(&["sh", "-c", "a-very-long-word-that-could-be-a-secret-token-0123456789 --x"])),
            Some("a-very-long-word-that-could-be-a-secret-token-0123456789".into()),
            "the program's own name is always said"
        );
        assert_eq!(say(strs(&["sh", "-c", "run AKIAIOSFODNN7EXAMPLEKEYTHATISLONG"])), Some("run".into()), "a long word after it is not");
        // macOS's `ps` gives one string when the kernel won't say.
        assert_eq!(say(strs(&["/bin/zsh -lc npm run dev"])), Some("npm run dev".into()));
        assert!(is_command(&strs(&["/bin/zsh -lc npm run dev"])));
        assert!(!is_command(&strs(&["node /opt/homebrew/bin/mcp-gitlab"])));
        assert!(!is_command(&strs(&["bash", "script.sh", "-c"])), "an option after the script is the script's");
        assert!(is_command(&strs(&["-bash", "-c", "x"])));
        assert!(!is_command(&strs(&["bash"])));
        assert!(!is_command(&[]));
    }

    #[test]
    fn the_reason_is_said_in_plain_words_and_kept_in_a_hosts_file_as_it_is() {
        let all =
            [Kept::Command(Some("npm run dev".into())), Kept::Command(None), Kept::Window, Kept::NotIdle, Kept::TaskAtWork, Kept::Unknown];
        for k in &all {
            assert_eq!(Kept::from_json(&k.to_json()).as_ref(), Some(k));
            for agent in Agent::ALL {
                let w = k.words(agent);
                assert!(w.starts_with("Kept running: "), "{w}");
                for jargon in ["process", "pid", "descendant", "MCP"] {
                    assert!(!w.contains(jargon), "{w}");
                }
            }
        }
        assert_eq!(all[0].words(Agent::Claude), "Kept running: a command Claude started is still going (npm run dev)");
        assert_eq!(all[0].words(Agent::Codex), "Kept running: a command Codex started is still going (npm run dev)");
        assert_eq!(Kept::NotIdle.words(Agent::Claude), "Kept running: Claude Code doesn't say it's idle");
        assert_eq!(Kept::from_json(&json!({"why": "something newer"})), None);
        assert_eq!(Kept::from_json(&json!(5)), None);
    }

    #[test]
    fn a_host_is_older_without_a_version_or_with_an_older_one() {
        assert!(older(None));
        assert!(older(Some("0.9.0")));
        assert!(!older(Some(crate::versions::running())));
        assert!(!older(Some("999.0.0")), "a newer platoon's host keeps its own rules");
        assert!(!older(Some("not a version")));
    }

    #[test]
    fn an_older_hosts_session_is_stopped_only_when_every_rule_says_so() {
        let hour = Duration::from_secs(3600);
        let two = 2 * hour;
        let free = Under { command: None, leftovers: Vec::new() };
        let busy = Under { command: Some(Command { pid: 5, what: Some("npm run dev".into()) }), leftovers: Vec::new() };
        assert_eq!(judge(two, Some(two), false, false, Some(&free)), Verdict::Stop);
        assert_eq!(judge(two, Some(hour), false, false, Some(&free)), Verdict::Keep(None), "not idle long enough");
        assert_eq!(judge(two, None, false, false, Some(&free)), Verdict::Keep(None), "not known to be idle");
        assert_eq!(judge(two, Some(two), true, false, Some(&free)), Verdict::Keep(Some(Kept::Window)));
        assert_eq!(judge(two, Some(two), false, true, Some(&free)), Verdict::Keep(Some(Kept::TaskAtWork)));
        assert_eq!(judge(two, Some(two), false, false, Some(&busy)), Verdict::Keep(Some(Kept::Command(Some("npm run dev".into())))));
        assert_eq!(judge(two, Some(two), false, false, None), Verdict::Keep(Some(Kept::Unknown)));
    }

    #[test]
    fn a_platoon_open_window_on_a_host_is_found_by_its_session_or_as_the_hosts_parent() {
        let rows = [row(20, 1, 9), row(21, 1, 9), row(22, 1, 9), row(23, 1, 9)];
        let table: HashMap<u32, Vec<String>> = [
            (20, strs(&["/usr/local/bin/platoon", "open", "abcd1234"])),
            (21, strs(&["platoon", "open"])),
            (22, strs(&["platoon", "host", "--agent", "claude"])),
            (23, strs(&["/x/platoon open --project shop"])),
        ]
        .into();
        let opens = open_windows(&rows, &|p| table.get(&p).cloned());
        assert_eq!(opens.iter().map(|o| o.0).collect::<Vec<_>>(), [20, 21, 23]);
        assert!(window_open(&opens, None, Some("abcd1234-5678")), "named by the start of its id");
        assert!(!window_open(&opens, None, Some("ffff0000-5678")));
        assert!(window_open(&opens, Some(21), None), "the window that started the host");
        assert!(!window_open(&opens, Some(1), None));
        assert!(!window_open(&opens, None, None));
    }

    /// `check_older` on a real process of the test's own: a host file's worth of facts, Claude Code's registry in a
    /// temporary folder, the test itself as the host.
    #[test]
    fn the_app_stops_an_older_hosts_quiet_session_only_while_no_window_is_open_on_it() {
        let dir = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-idle-older-{}", std::process::id())));
        fs::create_dir_all(dir.join("claude/sessions")).unwrap();
        let mut child = std::process::Command::new("sleep").arg("600").spawn().unwrap();
        let pid = child.id();
        let id = "cccc0001-old";
        let entry = json!({"pid": pid, "sessionId": id, "kind": "interactive", "status": "idle", "statusUpdatedAt": 1000});
        fs::write(dir.join(format!("claude/sessions/{pid}.json")), entry.to_string()).unwrap();
        let info = HostInfo {
            agent: Agent::Claude,
            claude_pid: pid,
            host_pid: std::process::id(),
            dir: "/tmp".into(),
            args: strs(&["--resume", id]),
            sock: PathBuf::new(),
            task: None,
            version: None,
            kept: None,
        };
        let hosts = [OldHost { info: info.clone(), session: info.session() }];
        let mut rules = Rules {
            threshold: Duration::from_secs(7200),
            settle: SETTLE,
            claude_root: dir.join("claude"),
            codex_root: None,
            config: dir.join("config"),
            attached: Some(pid),
        };
        assert_eq!(check_older(&hosts, &rules), [(pid, Some(Kept::Window))], "the app has it open");
        // A host that isn't its parent: never signaled, whatever the rules say.
        rules.attached = None;
        let stranger = [OldHost { info: HostInfo { host_pid: 1, ..info.clone() }, session: info.session() }];
        assert_eq!(check_older(&stranger, &rules), [(pid, None)]);
        assert!(child.try_wait().unwrap().is_none(), "not the host's child: left alone");
        assert_eq!(check_older(&hosts, &rules), [(pid, None)]);
        let start = std::time::Instant::now();
        while child.try_wait().unwrap().is_none() {
            assert!(start.elapsed() < Duration::from_secs(30), "the session was never stopped");
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
