//! Telling a session's MCP servers from the rest of what runs under it, for idle stop.
//!
//! Claude Code starts every configured stdio MCP server as a direct child at session start and keeps it
//! until it exits, so a real session nearly always has some. They are closed when claude shuts down and
//! started again on resume, so they shouldn't stop an idle session from being stopped. A dev server or a
//! long job that claude started is different: stopping claude would end it.
//!
//! Everything here reads formats Claude Code doesn't promise to keep, so it is best effort in one
//! direction only: when anything is unclear the process counts as work and blocks the stop. A process is
//! an MCP server only if it is a DIRECT child of claude (a server started by a Bash tool is deeper) and its
//! command line matches a server that is configured and not disabled. Wrapper launchers (`npx`, `uvx`,
//! `docker run`, `sh -c`) start further processes, so the whole subtree under a matched child is ignored.
//!
//! Where servers are configured (all read every time, so an edit applies at the next check):
//! - `mcpServers` and `projects[<cwd>].mcpServers` in Claude's user config (`~/.claude.json`, or
//!   `.claude.json` inside `CLAUDE_CONFIG_DIR`);
//! - `.mcp.json` in the session's directory and every parent directory;
//! - enabled plugins: `.mcp.json` and `.claude-plugin/plugin.json` in the plugin's install directory.
//!
//! Entries of another `type` than `stdio` (http, sse) have no local process and are skipped. Names in
//! `disabledMcpServers` / `disabledMcpjsonServers` and plugins switched off in a settings file are skipped
//! too: claude doesn't start them, so a process that looks like one is somebody else's.

use crate::procs::{self, Row};
use serde_json::{Map, Value};
use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Where the configuration is read from. Separate from the environment so tests can point it at temp dirs.
pub struct Sources {
    /// Claude's config directory: `settings.json`, `plugins/`.
    pub root: PathBuf,
    /// Claude's user config (`~/.claude.json`).
    pub claude_json: PathBuf,
    /// The session's working directory.
    pub cwd: PathBuf,
    pub home: String,
    /// Environment variables `${VAR}` in the configuration can refer to.
    pub vars: HashMap<String, String>,
}

impl Sources {
    /// The real locations for a session started in `cwd`, with this process's environment (which is the
    /// environment claude was started with).
    pub fn from_env(cwd: &str) -> Self {
        Self {
            root: crate::claude::claude_root(),
            claude_json: crate::claude::claude_json_path(),
            cwd: PathBuf::from(cwd),
            home: std::env::var("HOME").unwrap_or_default(),
            vars: std::env::vars().collect(),
        }
    }
}

/// One configured stdio server: what claude runs to start it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Server {
    pub name: String,
    /// The command, then its arguments, with `${VAR}` filled in as far as it could be.
    pub argv: Vec<String>,
}

#[derive(Default, Debug)]
pub struct Config {
    pub servers: Vec<Server>,
    /// A place that should have been read was there but couldn't be (unreadable, not JSON). Only says why
    /// nothing matched: fewer known servers already means more things block.
    pub unreadable: bool,
    home: String,
    path: Vec<PathBuf>,
}

enum Read {
    Missing,
    Bad,
    Json(Value),
}

/// A file that is simply absent is fine; one that is there but unreadable or not a JSON object is `Bad`.
/// `lenient_denied` treats "permission denied" as absent, for parent directories the user may not own.
fn read_json(path: &Path, lenient_denied: bool) -> Read {
    match fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(v) if v.is_object() => Read::Json(v),
            _ => Read::Bad,
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ENOTDIR) => Read::Missing,
        Err(e) if lenient_denied && e.kind() == io::ErrorKind::PermissionDenied => Read::Missing,
        Err(_) => Read::Bad,
    }
}

/// `${VAR}` and `${VAR:-default}`. Something that can't be filled in stays as it is, and then matches
/// anything in that place (see `wildcard`).
fn expand(s: &str, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[i..]);
            return out;
        };
        let inner = &after[..end];
        let (name, default) = match inner.split_once(":-") {
            Some((n, d)) => (n, Some(d)),
            None => (inner, None),
        };
        match lookup(name).filter(|v| !v.is_empty() || default.is_none()).or_else(|| default.map(String::from)) {
            Some(v) => out.push_str(&v),
            None => out.push_str(&rest[i..i + 2 + end + 1]),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The variables a piece of configuration can use: the environment plus a few Claude sets itself.
struct Vars<'a> {
    env: &'a HashMap<String, String>,
    extra: Vec<(&'static str, String)>,
}

impl Vars<'_> {
    fn get(&self, name: &str) -> Option<String> {
        self.extra.iter().find(|(k, _)| *k == name).map(|(_, v)| v.clone()).or_else(|| self.env.get(name).cloned())
    }
}

fn as_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// The stdio servers in a `{name: {command, args, ...}}` object. A malformed entry is skipped, never fatal.
fn servers_in(map: &Map<String, Value>, vars: &Vars, out: &mut Vec<Server>) {
    for (name, entry) in map {
        let Some(o) = entry.as_object() else { continue };
        // No type means stdio. http, sse, ws: nothing runs locally.
        if o.get("type").is_some_and(|t| t.as_str() != Some("stdio")) || o.get("disabled") == Some(&Value::Bool(true)) {
            continue;
        }
        let Some(command) = o.get("command").and_then(Value::as_str).filter(|c| !c.trim().is_empty()) else { continue };
        let args: Option<Vec<String>> = match o.get("args") {
            None | Some(Value::Null) => Some(Vec::new()),
            Some(Value::Array(a)) => a.iter().map(as_text).collect(),
            Some(_) => None,
        };
        let Some(args) = args else { continue };
        let lookup = |n: &str| vars.get(n);
        let argv = std::iter::once(command.to_string()).chain(args).map(|a| expand(&a, &lookup)).collect();
        out.push(Server { name: name.clone(), argv });
    }
}

fn names(v: Option<&Value>) -> impl Iterator<Item = String> + '_ {
    v.and_then(Value::as_array).into_iter().flatten().filter_map(|n| n.as_str().map(String::from))
}

/// `enabledPlugins`: `{"name@marketplace": true}`.
fn plugin_flag(v: &Value, id: &str) -> Option<bool> {
    v.get("enabledPlugins")?.get(id)?.as_bool()
}

struct Plugin {
    id: String,
    dir: PathBuf,
}

fn subdirs(p: &Path) -> Vec<(String, PathBuf)> {
    let mut v: Vec<_> = fs::read_dir(p)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| Some((e.file_name().to_str()?.to_string(), e.path())))
        .collect();
    v.sort();
    v
}

/// Installed plugins: the list Claude keeps (`plugins/installed_plugins.json`), or, without it, whatever
/// sits in `plugins/cache/<marketplace>/<name>/<version>`.
fn installed_plugins(src: &Sources, unreadable: &mut bool) -> Vec<Plugin> {
    let mut out = Vec::new();
    match read_json(&src.root.join("plugins/installed_plugins.json"), false) {
        Read::Json(v) => {
            for (id, entries) in v.get("plugins").and_then(Value::as_object).into_iter().flatten() {
                let list: Vec<&Value> = match entries {
                    Value::Array(a) => a.iter().collect(),
                    one => vec![one],
                };
                for e in list {
                    let Some(dir) = e.get("installPath").and_then(Value::as_str) else { continue };
                    // A plugin installed for another project isn't loaded in this one.
                    let elsewhere = matches!(e.get("scope").and_then(Value::as_str), Some("project" | "local"))
                        && e.get("projectPath").and_then(Value::as_str).is_some_and(|p| Path::new(p) != src.cwd);
                    if !elsewhere {
                        out.push(Plugin { id: id.clone(), dir: PathBuf::from(dir) });
                    }
                }
            }
        }
        Read::Bad => *unreadable = true,
        Read::Missing => {
            for (market, mdir) in subdirs(&src.root.join("plugins/cache")) {
                for (name, pdir) in subdirs(&mdir) {
                    for (_, vdir) in subdirs(&pdir) {
                        out.push(Plugin { id: format!("{name}@{market}"), dir: vdir });
                    }
                }
            }
        }
    }
    out
}

/// Claude's `${CLAUDE_PLUGIN_DATA}`: `plugins/data/<name>-<marketplace>`, other characters turned to `-`.
fn plugin_data_dir(src: &Sources, id: &str) -> PathBuf {
    let clean: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' }).collect();
    src.root.join("plugins/data").join(clean)
}

/// Every stdio server that could be running under a session started in `src.cwd`.
pub fn load(src: &Sources) -> Config {
    let path = std::env::split_paths(src.vars.get("PATH").map(String::as_str).unwrap_or("")).collect();
    let mut cfg = Config { home: src.home.clone(), path, ..Config::default() };
    let project_dir = src.cwd.to_string_lossy().into_owned();
    let vars = Vars { env: &src.vars, extra: vec![("CLAUDE_PROJECT_DIR", project_dir.clone())] };
    let mut disabled: HashSet<String> = HashSet::new();
    let mut tables: Vec<Value> = Vec::new(); // settings-style files, most specific first

    // The directory as given and as the system resolves it (Claude keys its per-project entries by one of them).
    let mut dirs = vec![src.cwd.clone()];
    if let Ok(real) = fs::canonicalize(&src.cwd)
        && real != src.cwd
    {
        dirs.push(real);
    }

    // User config: user-scoped servers, and this directory's "local" ones.
    let mut claude_json = None;
    match read_json(&src.claude_json, false) {
        Read::Json(v) => {
            if let Some(m) = v.get("mcpServers").and_then(Value::as_object) {
                servers_in(m, &vars, &mut cfg.servers);
            }
            for d in &dirs {
                let key = d.to_string_lossy();
                let key = if key.len() > 1 { key.trim_end_matches('/') } else { &key };
                let Some(p) = v.get("projects").and_then(|p| p.get(key)) else { continue };
                if let Some(m) = p.get("mcpServers").and_then(Value::as_object) {
                    servers_in(m, &vars, &mut cfg.servers);
                }
                disabled.extend(names(p.get("disabledMcpServers")).chain(names(p.get("disabledMcpjsonServers"))));
            }
            claude_json = Some(v);
        }
        Read::Bad => cfg.unreadable = true,
        Read::Missing => {}
    }

    // Settings files: which plugins are on, and servers switched off.
    for path in [dirs[0].join(".claude/settings.local.json"), dirs[0].join(".claude/settings.json"), src.root.join("settings.json")] {
        match read_json(&path, false) {
            Read::Json(v) => tables.push(v),
            Read::Bad => cfg.unreadable = true,
            Read::Missing => {}
        }
    }
    tables.extend(claude_json);
    for t in &tables {
        disabled.extend(names(t.get("disabledMcpjsonServers")).chain(names(t.get("disabledMcpServers"))));
    }

    // Project scope: `.mcp.json` here and in every parent directory (the closest one wins in Claude; here
    // all count, which can only make a real server match, never a stranger).
    let mut seen = HashSet::new();
    for d in &dirs {
        for dir in d.ancestors() {
            if !seen.insert(dir.to_path_buf()) {
                continue;
            }
            match read_json(&dir.join(".mcp.json"), dir != d) {
                Read::Json(v) => {
                    if let Some(m) = v.get("mcpServers").and_then(Value::as_object) {
                        servers_in(m, &vars, &mut cfg.servers);
                    }
                }
                Read::Bad => cfg.unreadable = true,
                Read::Missing => {}
            }
        }
    }

    // Plugins. Switched off if the most specific settings file that mentions it says so; unmentioned
    // means on (Claude writes an entry when it installs one, so the gap only shows if the layout changed).
    for plugin in installed_plugins(src, &mut cfg.unreadable) {
        if tables.iter().find_map(|t| plugin_flag(t, &plugin.id)) == Some(false) {
            continue;
        }
        let pvars = Vars {
            env: &src.vars,
            extra: vec![
                ("CLAUDE_PROJECT_DIR", project_dir.clone()),
                ("CLAUDE_PLUGIN_ROOT", plugin.dir.to_string_lossy().into_owned()),
                ("CLAUDE_PLUGIN_DATA", plugin_data_dir(src, &plugin.id).to_string_lossy().into_owned()),
            ],
        };
        let name = plugin.id.split('@').next().unwrap_or(&plugin.id).to_string();
        let before = cfg.servers.len();
        plugin_servers(&plugin.dir, &pvars, &mut cfg);
        // A plugin's server that was switched off is named `plugin:<plugin>:<server>`.
        let mut added = cfg.servers.split_off(before);
        added.retain(|s| !disabled.contains(&format!("plugin:{name}:{}", s.name)));
        cfg.servers.extend(added);
    }

    cfg.servers.retain(|s| !disabled.contains(&s.name));
    cfg
}

/// A plugin's servers: its root `.mcp.json` (either `{"mcpServers": {...}}` or the bare `{name: {...}}` some
/// plugins use) and `mcpServers` in `.claude-plugin/plugin.json` (an object, or a path or list of paths).
fn plugin_servers(dir: &Path, vars: &Vars, cfg: &mut Config) {
    fn from_file(path: &Path, vars: &Vars, cfg: &mut Config) {
        match read_json(path, false) {
            Read::Json(v) => match v.get("mcpServers").and_then(Value::as_object) {
                Some(m) => servers_in(m, vars, &mut cfg.servers),
                None => servers_in(v.as_object().unwrap_or(&Map::new()), vars, &mut cfg.servers),
            },
            Read::Bad => cfg.unreadable = true,
            Read::Missing => {}
        }
    }
    from_file(&dir.join(".mcp.json"), vars, cfg);
    let manifest = match read_json(&dir.join(".claude-plugin/plugin.json"), false) {
        Read::Json(v) => v,
        Read::Bad => {
            cfg.unreadable = true;
            return;
        }
        Read::Missing => return,
    };
    let refs: Vec<&Value> = match manifest.get("mcpServers") {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(one) => vec![one],
        None => Vec::new(),
    };
    for r in refs {
        match r {
            Value::Object(m) => servers_in(m, vars, &mut cfg.servers),
            Value::String(rel) => from_file(&dir.join(rel), vars, cfg),
            _ => {}
        }
    }
}

// --- matching a command line against a configured server ---------------------------------------------

/// Leading `~` written out, one pair of quotes dropped.
fn norm(s: &str, home: &str) -> String {
    let t = s.trim();
    let t = match (t.chars().next(), t.chars().last()) {
        (Some(a @ ('"' | '\'')), Some(b)) if a == b && t.len() >= 2 => &t[1..t.len() - 1],
        _ => t,
    };
    match t.strip_prefix('~') {
        Some("") if !home.is_empty() => home.to_string(),
        Some(rest) if rest.starts_with('/') && !home.is_empty() => format!("{home}{rest}"),
        _ => t.to_string(),
    }
}

/// What a name amounts to without the decoration a launcher's script gets (`npx-cli.js` is `npx`).
fn stem(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    let file =
        ["js", "mjs", "cjs", "ts", "py", "sh", "exe", "cmd"].iter().find_map(|e| file.strip_suffix(&format!(".{e}"))).unwrap_or(file);
    file.strip_suffix("-cli").unwrap_or(file).to_string()
}

/// Filling in `${VAR}` failed for a piece of the configuration: that piece stands for any text.
fn has_wildcard(s: &str) -> bool {
    s.contains("${")
}

/// The pattern's own text, without its `${...}` holes.
fn literal_len(pattern: &str) -> usize {
    let mut n = 0;
    let mut rest = pattern;
    while let Some(i) = rest.find("${") {
        n += i;
        match rest[i..].find('}') {
            Some(j) => rest = &rest[i + j + 1..],
            None => return n + rest.len() - i,
        }
    }
    n + rest.len()
}

/// Does `text` fit `pattern`, where each leftover `${...}` stands for any text?
fn wildcard(pattern: &str, text: &str) -> bool {
    let mut parts = Vec::new();
    let mut rest = pattern;
    while let Some(i) = rest.find("${") {
        let Some(j) = rest[i..].find('}') else { break };
        parts.push(&rest[..i]);
        rest = &rest[i + j + 1..];
    }
    parts.push(rest);
    if parts.len() == 1 {
        return pattern == text;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if text.len() < first.len() + last.len() || !text.starts_with(first) || !text.ends_with(last) {
        return false;
    }
    let mut at = first.len();
    let end = text.len() - last.len();
    for mid in &parts[1..parts.len() - 1] {
        match text[at..end].find(mid) {
            Some(i) => at += i + mid.len(),
            None => return false,
        }
    }
    true
}

impl Config {
    /// The real file a command name stands for: bare names are looked up on PATH, then symlinks resolved.
    fn resolved(&self, cmd: &str) -> Option<PathBuf> {
        if cmd.contains('/') {
            return fs::canonicalize(cmd).ok();
        }
        self.path.iter().find_map(|d| fs::canonicalize(d.join(cmd)).ok().filter(|p| p.is_file()))
    }

    /// Same program: identical, or one is a bare name and the other a path to it, or both lead to one file.
    fn same_command(&self, want: &str, got: &str) -> bool {
        let (want, got) = (norm(want, &self.home), norm(got, &self.home));
        if has_wildcard(&want) {
            return wildcard(&want, &got);
        }
        if want == got {
            return true;
        }
        if (!want.contains('/') || !got.contains('/')) && stem(&want) == stem(&got) {
            return true;
        }
        matches!((self.resolved(&want), self.resolved(&got)), (Some(a), Some(b)) if a == b)
    }

    fn same_arg(&self, want: &str, got: &str) -> bool {
        let (want, got) = (norm(want, &self.home), norm(got, &self.home));
        if has_wildcard(&want) { wildcard(&want, &got) } else { want == got }
    }

    fn same_argv(&self, want: &[String], got: &[String]) -> bool {
        if has_wildcard(&want[0]) && literal_len(&want[0]) == 0 {
            return false; // a command nobody knows the name of says nothing about what runs
        }
        if want.len() == got.len()
            && self.same_command(&want[0], &got[0])
            && want[1..].iter().zip(&got[1..]).all(|(w, g)| self.same_arg(w, g))
        {
            return true;
        }
        // Where the arguments can't be told apart (one string from `ps`, or a config that puts the whole
        // command line in `command`), compare the line as text.
        let line = |v: &[String]| {
            v.iter().map(|a| norm(a, &self.home)).collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
        };
        let (w, g) = (line(want), line(got));
        if has_wildcard(&w) { wildcard(&w, &g) } else { w == g }
    }

    /// Is this command line one of the configured servers?
    pub fn matches(&self, argv: &[String]) -> bool {
        let cands = candidates(argv);
        self.servers.iter().any(|s| cands.iter().any(|c| self.same_argv(&s.argv, c) || npm_title(&s.argv, c)))
    }
}

/// The ways one command line can read. A script runs as `node /path/script args`, and the server that was
/// configured is the script. A process that rewrote its own title into one string (npm does) is read as words.
fn candidates(argv: &[String]) -> Vec<Vec<String>> {
    let words: Vec<String> = match argv {
        [one] if one.contains(' ') => one.split_whitespace().map(String::from).collect(),
        _ => argv.to_vec(),
    };
    let mut out = vec![argv.to_vec()];
    if words != argv {
        out.push(words.clone());
    }
    let Some(first) = words.first() else { return out };
    let prog = first.rsplit('/').next().unwrap_or(first);
    let runtime = ["node", "nodejs", "bun", "deno", "ruby", "perl"].contains(&prog) || prog.starts_with("python");
    let shell = ["sh", "bash", "dash", "zsh"].contains(&prog);
    if runtime || shell {
        // Options before the script (`node --no-warnings x.js`) are skipped for a runtime; a shell run
        // with options is `sh -c ...`, which is not a script.
        let rest: Vec<String> = words[1..].iter().skip_while(|w| runtime && w.starts_with('-')).cloned().collect();
        if rest.first().is_some_and(|w| !w.starts_with('-')) {
            out.push(rest);
        }
    }
    out
}

/// `npx -y pkg` shows up as `npm exec pkg` once npm renames itself.
fn npm_title(want: &[String], got: &[String]) -> bool {
    let launcher = stem(&want[0]);
    if launcher != "npx" && launcher != "pnpx" {
        return false;
    }
    let tail: Vec<&String> = want[1..].iter().filter(|a| !["-y", "--yes"].contains(&a.as_str())).collect();
    got.len() == 2 + tail.len() && got[0] == "npm" && got[1] == "exec" && got[2..].iter().zip(tail).all(|(g, w)| g == w)
}

// --- the process tree -----------------------------------------------------------------------------------

/// A process to remember: enough to be sure later that a pid is still this same process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tracked {
    pub pid: u32,
    pub start: u64,
    pub argv: Option<Vec<String>>,
}

/// Why something under claude counts as work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocker {
    /// A process that isn't a configured MCP server, or runs under something that isn't.
    Other(u32),
    /// A direct child whose command line couldn't be read: it might be anything.
    Unsure(u32),
}

pub struct Split {
    /// The MCP servers and everything under them, as they were when looked at.
    pub ignored: Vec<Tracked>,
    /// The first thing that is not one of them, if any.
    pub blocker: Option<Blocker>,
}

/// Sort what runs under `claude` into MCP servers (a direct child that matches the configuration, with its
/// whole subtree) and the rest. None if claude isn't in the table at all.
pub fn split(rows: &[Row], claude: u32, cfg: &Config, argv_of: &dyn Fn(u32) -> Option<Vec<String>>) -> Option<Split> {
    let all = procs::descendants(rows, claude)?;
    let mut direct: Vec<&Row> = rows.iter().filter(|r| r.ppid == claude && r.pid != claude && !r.zombie).collect();
    direct.sort_by_key(|r| r.pid);

    let (mut ignored, mut blocker): (Vec<Tracked>, Option<Blocker>) = (Vec::new(), None);
    let mut covered: HashSet<u32> = HashSet::new();
    for child in direct {
        let argv = argv_of(child.pid);
        match argv.as_deref() {
            Some(a) if cfg.matches(a) => {
                ignored.push(Tracked { pid: child.pid, start: child.start, argv });
                covered.insert(child.pid);
                for pid in procs::descendants(rows, child.pid).unwrap_or_default() {
                    if let Some(r) = rows.iter().find(|r| r.pid == pid) {
                        ignored.push(Tracked { pid, start: r.start, argv: argv_of(pid) });
                        covered.insert(pid);
                    }
                }
            }
            Some(_) => blocker = blocker.or(Some(Blocker::Other(child.pid))),
            None => blocker = blocker.or(Some(Blocker::Unsure(child.pid))),
        }
    }
    // Anything else alive below claude, however it got there, is work too.
    let stray = all.iter().copied().filter(|p| !covered.contains(p)).min();
    Some(Split { ignored, blocker: blocker.or(stray.map(Blocker::Other)) })
}

/// SIGTERM `t` if that pid is still the process that was recorded: alive, and started at the same time (and
/// with the same command line, where that was recorded). Anything else, such as a pid reused since, is left
/// alone. True if a signal was sent.
pub fn terminate_if_same(t: &Tracked) -> bool {
    if t.pid <= 1 || t.start == 0 {
        return false; // a start time is the only proof of identity
    }
    let Some(now) = procs::table().and_then(|rows| rows.into_iter().find(|r| r.pid == t.pid)) else { return false };
    if now.zombie || now.start != t.start {
        return false;
    }
    if let (Some(before), Some(after)) = (&t.argv, procs::cmdline(t.pid))
        && *before != after
    {
        return false;
    }
    unsafe { libc::kill(t.pid as i32, libc::SIGTERM) == 0 }
}

/// Wait up to `wait` for `claude` to exit, and only then clean up after it (see `end_leftovers`). While it is
/// still running its MCP servers are in use, so if it never goes nothing is signalled.
pub fn end_leftovers_after(claude: u32, tracked: &[Tracked], wait: Duration, grace: Duration) {
    let started = Instant::now();
    let running = || procs::table().is_none_or(|rows| rows.iter().any(|r| r.pid == claude && !r.zombie));
    while running() {
        if started.elapsed() >= wait {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    end_leftovers(tracked, grace);
}

/// Give leftovers `grace` to exit on their own, then SIGTERM each one that is still the same process.
pub fn end_leftovers(tracked: &[Tracked], grace: Duration) {
    let started = Instant::now();
    let any_running = || {
        procs::table().is_some_and(|rows| tracked.iter().any(|t| rows.iter().any(|r| r.pid == t.pid && !r.zombie && r.start == t.start)))
    };
    while started.elapsed() < grace && any_running() {
        std::thread::sleep(Duration::from_millis(50));
    }
    for t in tracked {
        terminate_if_same(t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::process::{Child, Command};

    /// A throwaway world: Claude's config dir, a fake home, and a project three levels below the home.
    struct World {
        base: PathBuf,
        src: Sources,
    }

    impl World {
        fn new(name: &str) -> Self {
            let base = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-mcp-{name}-{}", std::process::id())));
            let _ = fs::remove_dir_all(&base);
            let cwd = base.join("home/code/app/sub");
            fs::create_dir_all(&cwd).unwrap();
            fs::create_dir_all(base.join("claude")).unwrap();
            let src = Sources {
                root: base.join("claude"),
                claude_json: base.join("claude/.claude.json"),
                cwd,
                home: base.join("home").to_string_lossy().into_owned(),
                vars: [("PATH".to_string(), std::env::var("PATH").unwrap_or_default()), ("FOO".to_string(), "bar".to_string())].into(),
            };
            World { base, src }
        }

        fn write(&self, rel: &str, v: &Value) {
            let p = self.base.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, v.to_string()).unwrap();
        }

        fn write_text(&self, rel: &str, text: &str) {
            let p = self.base.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }

        fn cwd_key(&self) -> String {
            self.src.cwd.to_string_lossy().into_owned()
        }

        fn servers(&self) -> Vec<(String, Vec<String>)> {
            let mut v: Vec<_> = load(&self.src).servers.into_iter().map(|s| (s.name, s.argv)).collect();
            v.sort();
            v
        }

        fn names(&self) -> Vec<String> {
            self.servers().into_iter().map(|s| s.0).collect()
        }
    }

    impl Drop for World {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn cfg(servers: &[&[&str]]) -> Config {
        let servers = servers.iter().enumerate().map(|(i, a)| Server { name: format!("s{i}"), argv: strs(a) }).collect();
        Config { servers, unreadable: false, home: "/home/u".into(), path: vec!["/usr/bin".into(), "/bin".into()] }
    }

    // --- where servers come from ---------------------------------------------------------------------------

    #[test]
    fn user_scoped_servers_are_read_from_the_top_of_the_user_config() {
        let w = World::new("user");
        w.write(".claude.json.unused", &json!({}));
        w.write("claude/.claude.json", &json!({"mcpServers": {"fs": {"command": "npx", "args": ["-y", "server-fs", "/tmp"]}, "bare": {"command": "uvx"}}, "unrelated": 1}));
        assert_eq!(w.servers(), [("bare".to_string(), strs(&["uvx"])), ("fs".to_string(), strs(&["npx", "-y", "server-fs", "/tmp"]))]);
    }

    #[test]
    fn per_project_servers_belong_to_the_session_directory_only() {
        let w = World::new("project");
        let key = w.cwd_key();
        w.write(
            "claude/.claude.json",
            &json!({"projects": {
                key: {"mcpServers": {"mine": {"type": "stdio", "command": "node", "args": ["srv.js"]}}, "allowedTools": []},
                "/somewhere/else": {"mcpServers": {"theirs": {"command": "node", "args": ["other.js"]}}},
            }}),
        );
        assert_eq!(w.names(), ["mine"], "another project's servers aren't loaded in this session");
        // The key may carry a trailing slash in the session's directory; the projects entry never does.
        let mut src = World::new("project-slash");
        src.src.cwd = PathBuf::from(format!("{}/", w.cwd_key()));
        src.write("claude/.claude.json", &json!({"projects": {w.cwd_key(): {"mcpServers": {"mine": {"command": "x"}}}}}));
        assert_eq!(src.names(), ["mine"]);
    }

    #[test]
    fn project_files_are_read_in_the_directory_and_every_parent() {
        let w = World::new("dotmcp");
        w.write("home/code/app/sub/.mcp.json", &json!({"mcpServers": {"here": {"command": "a"}}}));
        w.write("home/code/app/.mcp.json", &json!({"mcpServers": {"parent": {"command": "b", "args": ["1"]}}}));
        w.write("home/.mcp.json", &json!({"mcpServers": {"grand": {"command": "c"}}}));
        w.write("home/code/.mcp.json", &json!({"mcpServers": {"web": {"type": "http", "url": "https://x"}}}));
        assert_eq!(w.names(), ["grand", "here", "parent"]);
        // A `.mcp.json` below the directory isn't this session's.
        w.write("home/code/app/sub/deeper/.mcp.json", &json!({"mcpServers": {"below": {"command": "d"}}}));
        assert!(!w.names().contains(&"below".to_string()));
        // The bare `{name: {...}}` shape is a plugin habit, not what a project file holds.
        w.write("home/code/app/sub/.mcp.json", &json!({"flat": {"command": "e"}}));
        assert!(!w.names().contains(&"flat".to_string()));
    }

    #[test]
    fn remote_disabled_and_malformed_entries_are_ignored_and_never_fatal() {
        let w = World::new("odd");
        w.write(
            "claude/.claude.json",
            &json!({"mcpServers": {
                "http": {"type": "http", "url": "https://x/mcp"},
                "sse": {"type": "sse", "url": "https://x/sse"},
                "ws": {"type": "ws", "command": "x"},
                "weird-type": {"type": 5, "command": "x"},
                "off": {"command": "x", "disabled": true},
                "no-command": {"args": ["a"]},
                "empty": {"command": "  "},
                "cmd-number": {"command": 5},
                "args-string": {"command": "x", "args": "a b"},
                "args-object": {"command": "x", "args": {"a": 1}},
                "args-nested": {"command": "x", "args": [["a"]]},
                "entry-string": "node server.js",
                "entry-null": null,
                "ok-null-args": {"command": "keep1", "args": null},
                "ok-numbers": {"command": "keep2", "args": [8080, true, "x"]},
                "ok-stdio": {"type": "stdio", "command": "keep3"},
            }}),
        );
        assert_eq!(w.names(), ["ok-null-args", "ok-numbers", "ok-stdio"]);
        assert_eq!(w.servers()[1].1, strs(&["keep2", "8080", "true", "x"]));
        // Wrong shapes higher up are skipped just the same.
        for junk in [json!({"mcpServers": []}), json!({"mcpServers": "x", "projects": 5}), json!({"projects": {"x": 1}}), json!([1, 2])] {
            w.write("claude/.claude.json", &junk);
            let _ = load(&w.src);
        }
    }

    #[test]
    fn servers_switched_off_for_the_project_are_dropped() {
        let w = World::new("disabled");
        let key = w.cwd_key();
        w.write("home/code/app/.mcp.json", &json!({"mcpServers": {"a": {"command": "a"}, "b": {"command": "b"}, "c": {"command": "c"}}}));
        w.write("claude/.claude.json", &json!({"mcpServers": {"u": {"command": "u"}, "v": {"command": "v"}}, "projects": {key: {"disabledMcpjsonServers": ["a"], "disabledMcpServers": ["u"]}}}));
        w.write("home/code/app/sub/.claude/settings.local.json", &json!({"disabledMcpjsonServers": ["b"]}));
        assert_eq!(w.names(), ["c", "v"]);
    }

    #[test]
    fn unreadable_configuration_is_flagged_and_reads_as_no_servers_from_that_file() {
        let w = World::new("bad");
        w.write_text("claude/.claude.json", "{ not json");
        w.write("home/code/app/.mcp.json", &json!({"mcpServers": {"a": {"command": "a"}}}));
        let c = load(&w.src);
        assert!(c.unreadable);
        assert_eq!(c.servers.len(), 1, "the other files are still used");
        assert!(!c.matches(&strs(&["b"])));
        // A directory where the file should be, and JSON that is not an object, are unreadable as well.
        let w = World::new("bad2");
        fs::create_dir_all(w.base.join("claude/.claude.json")).unwrap();
        assert!(load(&w.src).unreadable);
        w.write_text("home/code/app/.mcp.json", "[1]");
        fs::remove_dir(w.base.join("claude/.claude.json")).unwrap();
        assert!(load(&w.src).unreadable);
        // Nothing at all is fine, not a problem.
        let w = World::new("none");
        let c = load(&w.src);
        assert!(!c.unreadable && c.servers.is_empty());
    }

    fn install_plugin(w: &World, id: &str, rel: &str, files: &[(&str, Value)]) -> PathBuf {
        let dir = w.base.join("claude/plugins/cache").join(rel);
        for (name, v) in files {
            w.write(&format!("claude/plugins/cache/{rel}/{name}"), v);
        }
        w.write("claude/plugins/installed_plugins.json", &{
            let mut v: Value = fs::read_to_string(w.base.join("claude/plugins/installed_plugins.json"))
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or(json!({"version": 2, "plugins": {}}));
            v.get_mut("plugins")
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert(id.into(), json!([{"scope": "user", "installPath": dir, "version": "1"}]));
            v
        });
        dir
    }

    #[test]
    fn plugin_servers_come_from_dot_mcp_json_and_plugin_json_with_the_plugin_root_filled_in() {
        let w = World::new("plugin");
        let a = install_plugin(
            &w,
            "alpha@mk",
            "mk/alpha/1",
            &[(
                ".mcp.json",
                json!({"mcpServers": {"wrapped": {"command": "${CLAUDE_PLUGIN_ROOT}/bin/serve", "args": ["--data", "${CLAUDE_PLUGIN_DATA}", "--proj", "${CLAUDE_PROJECT_DIR}"]}}}),
            )],
        );
        let b = install_plugin(
            &w,
            "beta@mk",
            "mk/beta/2",
            &[(
                ".mcp.json",
                json!({"flat": {"command": "bun", "args": ["run", "--cwd", "${CLAUDE_PLUGIN_ROOT}", "start"]}, "remote": {"type": "http", "url": "https://x"}}),
            )],
        );
        let c = install_plugin(
            &w,
            "gamma@mk",
            "mk/gamma/3",
            &[(
                ".claude-plugin/plugin.json",
                json!({"name": "gamma", "mcpServers": {"inline": {"command": "node", "args": ["${CLAUDE_PLUGIN_ROOT}/s.js", "${FOO}", "${NOPE:-dflt}", "${NOPE}"]}}}),
            )],
        );
        let d = install_plugin(
            &w,
            "delta@mk",
            "mk/delta/4",
            &[
                (".claude-plugin/plugin.json", json!({"mcpServers": ["./servers.json", {"second": {"command": "py"}}, 5]})),
                ("servers.json", json!({"mcpServers": {"first": {"command": "sh", "args": ["x"]}}})),
            ],
        );
        let got: HashMap<String, Vec<String>> = load(&w.src).servers.into_iter().map(|s| (s.name, s.argv)).collect();
        let root = |p: &Path| p.to_string_lossy().into_owned();
        assert_eq!(
            got["wrapped"],
            strs(&[
                &format!("{}/bin/serve", root(&a)),
                "--data",
                &root(&w.base.join("claude/plugins/data/alpha-mk")),
                "--proj",
                &w.cwd_key()
            ])
        );
        assert_eq!(
            got["flat"],
            strs(&["bun", "run", "--cwd", &root(&b), "start"]),
            "the bare shape works, and the remote entry beside it is skipped"
        );
        assert!(!got.contains_key("remote"));
        assert_eq!(
            got["inline"],
            strs(&["node", &format!("{}/s.js", root(&c)), "bar", "dflt", "${NOPE}"]),
            "environment, defaults, and what can't be filled in stays"
        );
        assert!(got.contains_key("first") && got.contains_key("second"), "{got:?} ({})", root(&d));
    }

    #[test]
    fn a_plugin_that_is_switched_off_or_installed_for_another_project_adds_nothing() {
        let w = World::new("plugin-off");
        install_plugin(&w, "on@mk", "mk/on/1", &[(".mcp.json", json!({"mcpServers": {"on": {"command": "a"}}}))]);
        install_plugin(&w, "off@mk", "mk/off/1", &[(".mcp.json", json!({"mcpServers": {"off": {"command": "b"}}}))]);
        install_plugin(&w, "unmentioned@mk", "mk/un/1", &[(".mcp.json", json!({"mcpServers": {"unmentioned": {"command": "c"}}}))]);
        install_plugin(
            &w,
            "srv-off@mk",
            "mk/so/1",
            &[(".mcp.json", json!({"mcpServers": {"keep": {"command": "d"}, "drop": {"command": "e"}}}))],
        );
        let mut list: Value =
            serde_json::from_str(&fs::read_to_string(w.base.join("claude/plugins/installed_plugins.json")).unwrap()).unwrap();
        list["plugins"].as_object_mut().unwrap().insert(
            "other@mk".into(),
            json!([{"scope": "project", "projectPath": "/not/here", "installPath": w.base.join("claude/plugins/cache/mk/other/1")}]),
        );
        w.write("claude/plugins/installed_plugins.json", &list);
        w.write("claude/plugins/cache/mk/other/1/.mcp.json", &json!({"mcpServers": {"other": {"command": "f"}}}));
        w.write("claude/settings.json", &json!({"enabledPlugins": {"on@mk": true, "off@mk": false, "other@mk": true, "srv-off@mk": true}}));
        w.write("claude/.claude.json", &json!({"projects": {w.cwd_key(): {"disabledMcpServers": ["plugin:srv-off:drop"]}}}));
        assert_eq!(w.names(), ["keep", "on", "unmentioned"]);
        // A project's own settings beat the user's.
        w.write("home/code/app/sub/.claude/settings.json", &json!({"enabledPlugins": {"off@mk": true, "on@mk": false}}));
        assert_eq!(w.names(), ["keep", "off", "unmentioned"]);
    }

    #[test]
    fn without_the_plugin_list_the_plugin_cache_is_searched_and_a_broken_list_is_flagged() {
        let w = World::new("plugin-cache");
        w.write("claude/plugins/cache/mk/scan/1.0/.mcp.json", &json!({"mcpServers": {"scanned": {"command": "x"}}}));
        assert_eq!(w.names(), ["scanned"]);
        w.write_text("claude/plugins/installed_plugins.json", "{ oops");
        let c = load(&w.src);
        assert!(c.unreadable && c.servers.is_empty());
    }

    #[test]
    fn a_broken_plugin_file_is_skipped_without_hiding_the_others() {
        let w = World::new("plugin-broken");
        install_plugin(&w, "good@mk", "mk/good/1", &[(".mcp.json", json!({"mcpServers": {"good": {"command": "x"}}}))]);
        let dir = install_plugin(&w, "bad@mk", "mk/bad/1", &[]);
        fs::create_dir_all(dir.join(".claude-plugin")).unwrap();
        fs::write(dir.join(".claude-plugin/plugin.json"), "{ nope").unwrap();
        fs::write(dir.join(".mcp.json"), "").unwrap();
        let c = load(&w.src);
        assert_eq!(c.servers.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["good"]);
        assert!(c.unreadable);
    }

    // --- matching ------------------------------------------------------------------------------------------

    #[test]
    fn a_command_line_matches_its_configured_server_and_nothing_else() {
        let c = cfg(&[&["node", "/srv/a/server.js", "--port", "9"], &["sleep", "100"]]);
        assert!(c.matches(&strs(&["node", "/srv/a/server.js", "--port", "9"])));
        assert!(c.matches(&strs(&["sleep", "100"])));
        for other in [
            &["node", "/srv/a/server.js", "--port", "10"][..],
            &["node", "/srv/a/server.js"],
            &["node", "/srv/a/server.js", "--port", "9", "x"],
            &["sleep", "101"],
            &["sleep"],
            &["python", "/srv/a/server.js", "--port", "9"],
            &[],
        ] {
            assert!(!c.matches(&strs(other)), "{other:?}");
        }
    }

    #[test]
    fn a_bare_command_name_and_a_full_path_to_it_are_the_same_program() {
        let c = cfg(&[&["sleep", "100"]]);
        assert!(c.matches(&strs(&["/usr/bin/sleep", "100"])));
        assert!(c.matches(&strs(&["/bin/sleep", "100"])));
        let c = cfg(&[&["/opt/tools/serve", "-x"]]);
        assert!(c.matches(&strs(&["serve", "-x"])));
        assert!(c.matches(&strs(&["/opt/tools/serve", "-x"])));
        assert!(!c.matches(&strs(&["/opt/other/serve2", "-x"])));
        assert!(!c.matches(&strs(&["/opt/other/serve", "-y"])), "arguments still have to agree");
    }

    #[test]
    fn two_paths_to_one_file_are_the_same_program() {
        let w = World::new("symlink");
        fs::create_dir_all(w.base.join("bin")).unwrap();
        fs::write(w.base.join("bin/real-server"), "#!/bin/sh\n").unwrap();
        std::os::unix::fs::symlink(w.base.join("bin/real-server"), w.base.join("bin/alias")).unwrap();
        let c = cfg(&[&[w.base.join("bin/alias").to_str().unwrap(), "1"]]);
        assert!(c.matches(&strs(&[w.base.join("bin/real-server").to_str().unwrap(), "1"])));
        assert!(!c.matches(&strs(&[w.base.join("bin/nothing").to_str().unwrap(), "1"])));
    }

    #[test]
    fn a_home_shortcut_and_quotes_read_the_same_as_the_plain_text() {
        let c = cfg(&[&["~/bin/srv", "--dir", "~/data", "'quoted arg'"]]);
        assert!(c.matches(&strs(&["/home/u/bin/srv", "--dir", "/home/u/data", "quoted arg"])));
        assert!(c.matches(&strs(&["~/bin/srv", "--dir", "~/data", "\"quoted arg\""])));
        assert!(!c.matches(&strs(&["/home/other/bin/srv", "--dir", "/home/u/data", "quoted arg"])));
    }

    #[test]
    fn a_variable_that_could_not_be_filled_in_stands_for_any_text_but_not_for_the_whole_command() {
        let c = cfg(&[&["${PLUGIN}/bin/serve", "--token", "${TOKEN}", "--name=${X}-a"]]);
        assert!(c.matches(&strs(&["/any/where/bin/serve", "--token", "s3cret", "--name=q-a"])));
        assert!(!c.matches(&strs(&["/any/where/bin/other", "--token", "s3cret", "--name=q-a"])));
        assert!(!c.matches(&strs(&["/any/where/bin/serve", "--token", "s3cret", "--name=q-b"])));
        let nothing = cfg(&[&["${CMD}", "x"]]);
        assert!(!nothing.matches(&strs(&["anything", "x"])), "a command nobody knows the name of matches nothing");
        assert!(!cfg(&[&["${CMD}"]]).matches(&strs(&["sleep"])));
    }

    #[test]
    fn variables_are_filled_in_with_defaults_and_left_alone_when_unknown() {
        let look = |n: &str| (n == "A").then(|| "1".to_string()).or_else(|| (n == "E").then(String::new));
        assert_eq!(expand("x${A}y", &look), "x1y");
        assert_eq!(expand("${B:-def}", &look), "def");
        assert_eq!(expand("${A:-def}", &look), "1");
        assert_eq!(expand("${E:-def}", &look), "def", "empty counts as unset for a default");
        assert_eq!(expand("${E}", &look), "");
        assert_eq!(expand("${B}-${A}", &look), "${B}-1");
        assert_eq!(expand("${unclosed", &look), "${unclosed");
        assert_eq!(expand("$A plain", &look), "$A plain");
    }

    #[test]
    fn a_script_run_by_its_interpreter_is_the_configured_script() {
        let c = cfg(&[&["/usr/bin/npx", "-y", "@scope/server"], &["/opt/s/mcp-server", "--x"], &["/opt/s/run.sh"]]);
        assert!(c.matches(&strs(&["node", "/usr/bin/npx", "-y", "@scope/server"])), "npx is a node script");
        assert!(c.matches(&strs(&["/usr/bin/node", "/usr/bin/npx", "-y", "@scope/server"])));
        assert!(c.matches(&strs(&["node", "--no-warnings", "/opt/s/mcp-server", "--x"])));
        assert!(c.matches(&strs(&["python3.12", "/opt/s/mcp-server", "--x"])));
        assert!(c.matches(&strs(&["/bin/bash", "/opt/s/run.sh"])), "a shell script");
        assert!(!c.matches(&strs(&["node", "/usr/bin/npx", "-y", "@scope/other"])));
        assert!(!c.matches(&strs(&["bash", "-c", "/opt/s/run.sh"])), "a shell command line is not a script");
    }

    #[test]
    fn a_process_that_renamed_itself_is_read_back_as_words() {
        let c = cfg(&[&["npx", "-y", "@playwright/mcp@latest"], &["sh", "-c", "exec foo --bar"]]);
        assert!(c.matches(&strs(&["npm exec @playwright/mcp@latest"])), "npm renames its process this way");
        assert!(c.matches(&strs(&["npm", "exec", "@playwright/mcp@latest"])));
        assert!(!c.matches(&strs(&["npm exec @playwright/mcp@1.0"])));
        assert!(c.matches(&strs(&["sh -c exec foo --bar"])), "or a line from `ps`, which doesn't keep arguments apart");
        assert!(!c.matches(&strs(&["sh -c exec foo --baz"])));
    }

    // --- the process tree ----------------------------------------------------------------------------------

    fn row(pid: u32, ppid: u32) -> Row {
        Row { pid, ppid, zombie: false, start: 1000 + pid as u64 }
    }

    /// claude = 10. argv by pid.
    fn tree(rows: &[Row], argv: &[(u32, &[&str])], c: &Config) -> Split {
        let table: HashMap<u32, Vec<String>> = argv.iter().map(|(p, a)| (*p, strs(a))).collect();
        split(rows, 10, c, &|p| table.get(&p).cloned()).unwrap()
    }

    #[test]
    fn a_direct_child_that_is_a_configured_server_is_ignored() {
        let c = cfg(&[&["srv", "a"], &["srv", "b"]]);
        let s = tree(&[row(10, 1), row(11, 10), row(12, 10)], &[(11, &["srv", "a"]), (12, &["/usr/bin/srv", "b"])], &c);
        assert_eq!(s.blocker, None);
        assert_eq!(s.ignored.iter().map(|t| t.pid).collect::<Vec<_>>(), [11, 12]);
        assert_eq!(s.ignored[0].start, 1011, "identity is recorded with it");
    }

    #[test]
    fn a_wrapper_is_ignored_together_with_everything_it_started() {
        let c = cfg(&[&["npx", "-y", "pkg"]]);
        let rows = [row(10, 1), row(11, 10), row(12, 11), row(13, 12), row(14, 11)];
        let s = tree(&rows, &[(11, &["npx", "-y", "pkg"]), (12, &["node", "x"]), (13, &["sh", "-c", "y"]), (14, &["helper"])], &c);
        assert_eq!(s.blocker, None);
        let mut pids: Vec<u32> = s.ignored.iter().map(|t| t.pid).collect();
        pids.sort();
        assert_eq!(pids, [11, 12, 13, 14]);
    }

    #[test]
    fn only_a_direct_child_counts_so_the_same_command_line_deeper_down_still_blocks() {
        let c = cfg(&[&["srv", "a"]]);
        // claude -> bash (a tool call) -> srv a: identical to the configured server, but not started by claude itself.
        let rows = [row(10, 1), row(11, 10), row(12, 11)];
        let s = tree(&rows, &[(11, &["bash", "-c", "run"]), (12, &["srv", "a"])], &c);
        assert_eq!(s.blocker, Some(Blocker::Other(11)));
        assert!(s.ignored.is_empty(), "nothing under it is ignored either");
    }

    #[test]
    fn an_unrelated_child_blocks_even_when_an_mcp_server_is_there_too() {
        let c = cfg(&[&["srv", "a"]]);
        let rows = [row(10, 1), row(11, 10), row(12, 10), row(13, 12)];
        let s = tree(&rows, &[(11, &["srv", "a"]), (12, &["npm", "run", "dev"]), (13, &["node", "dev"])], &c);
        assert_eq!(s.blocker, Some(Blocker::Other(12)));
        assert_eq!(s.ignored.iter().map(|t| t.pid).collect::<Vec<_>>(), [11]);
        // A stray under the server itself is part of the server's subtree, not work.
        let s = tree(&[row(10, 1), row(11, 10), row(12, 11)], &[(11, &["srv", "a"]), (12, &["anything"])], &c);
        assert_eq!(s.blocker, None);
    }

    #[test]
    fn a_child_whose_command_line_cannot_be_read_is_unsure_and_blocks() {
        let c = cfg(&[&["srv", "a"]]);
        let s = tree(&[row(10, 1), row(11, 10), row(12, 10)], &[(12, &["srv", "a"])], &c);
        assert_eq!(s.blocker, Some(Blocker::Unsure(11)));
        assert_eq!(s.ignored.iter().map(|t| t.pid).collect::<Vec<_>>(), [12]);
    }

    #[test]
    fn with_nothing_configured_every_child_blocks_and_nothing_at_all_is_free() {
        let none = cfg(&[]);
        let rows = [row(10, 1), row(11, 10)];
        assert_eq!(tree(&rows, &[(11, &["srv", "a"])], &none).blocker, Some(Blocker::Other(11)));
        let s = tree(&[row(10, 1)], &[], &none);
        assert_eq!((s.blocker, s.ignored.len()), (None, 0));
        assert!(split(&rows, 99, &none, &|_| None).is_none(), "a claude that isn't in the table is unknown");
    }

    #[test]
    fn finished_children_do_not_count_and_the_lowest_blocking_pid_is_reported() {
        let c = cfg(&[]);
        let mut zombie = row(11, 10);
        zombie.zombie = true;
        let s = tree(&[row(10, 1), zombie], &[(11, &["x"])], &c);
        assert_eq!(s.blocker, None);
        let s = tree(&[row(10, 1), row(15, 10), row(12, 10)], &[(15, &["x"]), (12, &["y"])], &c);
        assert_eq!(s.blocker, Some(Blocker::Other(12)));
    }

    // --- signalling leftovers ------------------------------------------------------------------------------
    //
    // Every wait here is for something to happen, with a deadline far longer than it ever takes, so a loaded
    // machine is slow but never fails; only "nothing happened" (`still_running`) has to be a fixed pause.

    /// Longer than anything here takes even on a loaded machine, so only a real failure reaches it.
    const PATIENCE: Duration = Duration::from_secs(60);

    /// A `sleep` of the test's own, killed and reaped when it goes out of scope (a failing test leaves none behind).
    struct Kid(Child);

    impl Kid {
        fn id(&self) -> u32 {
            self.0.id()
        }
    }

    impl Drop for Kid {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// Long enough never to end by itself while a test runs, however slow the machine.
    fn spawn(secs: &str) -> Kid {
        Kid(Command::new("sleep").arg(secs).spawn().unwrap())
    }

    /// What `Tracked` records for `pid`. Its command line is waited for: right after a spawn returns, the new
    /// program's arguments can still be unset for a moment, which reads as no command line at all.
    fn tracked(pid: u32) -> Tracked {
        let start = procs::table().unwrap().into_iter().find(|r| r.pid == pid).unwrap().start;
        let since = Instant::now();
        let argv = loop {
            match procs::cmdline(pid) {
                Some(a) => break a,
                None => assert!(since.elapsed() < PATIENCE, "pid {pid} never showed its command line"),
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        Tracked { pid, start, argv: Some(argv) }
    }

    fn wait_gone(c: &mut Kid) -> bool {
        let since = Instant::now();
        while since.elapsed() < PATIENCE {
            if c.0.try_wait().unwrap().is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// Still there after a pause: what "not signalled" looks like (it would have ended within the pause).
    fn still_running(c: &mut Kid) -> bool {
        std::thread::sleep(Duration::from_millis(300));
        c.0.try_wait().unwrap().is_none()
    }

    #[test]
    fn only_the_recorded_process_is_signalled() {
        let (mut mine, mut bystander) = (spawn("641"), spawn("641"));
        let t = tracked(mine.id());
        assert!(terminate_if_same(&t));
        assert!(wait_gone(&mut mine));
        assert!(still_running(&mut bystander), "same command line, different process: untouched");
        // The process is gone: signalling it again does nothing (its pid could be anybody's by now).
        assert!(!terminate_if_same(&t));
    }

    #[test]
    fn a_pid_that_now_belongs_to_another_process_is_left_alone() {
        let mut other = spawn("642");
        let good = tracked(other.id());
        // Same pid, but a different start time: what a reused pid looks like.
        let reused = Tracked { start: good.start + 1, ..good.clone() };
        assert!(!terminate_if_same(&reused));
        // Same pid and start but not the command line that was recorded.
        let renamed = Tracked { argv: Some(strs(&["something", "else"])), ..good.clone() };
        assert!(!terminate_if_same(&renamed));
        // No start time recorded: no way to tell, so no signal. Nor to the pids nobody may signal.
        assert!(!terminate_if_same(&Tracked { start: 0, ..good.clone() }));
        for pid in [0, 1] {
            assert!(!terminate_if_same(&Tracked { pid, start: 5, argv: None }));
        }
        assert!(still_running(&mut other));
        assert!(terminate_if_same(&good), "the real one is still fine");
        assert!(wait_gone(&mut other));
    }

    #[test]
    fn leftovers_get_a_grace_period_then_only_the_recorded_ones_are_ended() {
        let (mut a, mut b, mut bystander, mut reused) = (spawn("643"), spawn("643"), spawn("643"), spawn("643"));
        let list = vec![tracked(a.id()), tracked(b.id()), Tracked { start: tracked(reused.id()).start + 7, ..tracked(reused.id()) }];
        let t = Instant::now();
        end_leftovers(&list, Duration::from_millis(300));
        assert!(t.elapsed() >= Duration::from_millis(300), "waited for them to leave by themselves first");
        assert!(wait_gone(&mut a) && wait_gone(&mut b));
        assert!(still_running(&mut bystander) && still_running(&mut reused));
    }

    #[test]
    fn nothing_is_signalled_while_claude_is_still_running_and_leftovers_are_ended_once_it_is_gone() {
        let (mut claude, mut server) = (spawn("644"), spawn("644"));
        let list = vec![tracked(server.id())];
        end_leftovers_after(claude.id(), &list, Duration::from_millis(400), Duration::from_millis(50));
        assert!(still_running(&mut server), "claude is still there, so its server is still in use");
        let id = claude.id();
        let killer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            unsafe { libc::kill(id as i32, libc::SIGKILL) };
        });
        // It returns as soon as claude is gone (a zombie counts): the long wait only matters on a loaded machine.
        end_leftovers_after(claude.id(), &list, PATIENCE, Duration::from_millis(50));
        killer.join().unwrap();
        assert!(wait_gone(&mut server), "claude exited (a zombie counts), so the server was asked to");
        let _ = claude.0.wait();
    }

    #[test]
    fn leftovers_that_leave_by_themselves_are_not_waited_for() {
        let mut gone = spawn("643");
        let list = vec![tracked(gone.id())];
        let _ = gone.0.kill();
        let _ = gone.0.wait();
        // The grace is far longer than the check may take even on a loaded machine: not waiting it out is the point.
        let t = Instant::now();
        end_leftovers(&list, 2 * PATIENCE);
        assert!(t.elapsed() < PATIENCE, "took {:?}", t.elapsed());
    }
}
