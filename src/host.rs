//! Session hosts: one small background process per session that owns claude's pseudo-terminal.
//!
//! Because the host is detached from platoon (own session, no controlling terminal), quitting
//! platoon, or closing its terminal, leaves claude running. platoon finds hosts again by scanning
//! `<cache>/hosts/`, and talks to them over a Unix socket using the framing below.
//!
//! Files per session, named by claude's pid, in a directory only the current user can enter:
//!   `<pid>.json`  who/where (removed when claude exits)
//!   `<pid>.sock`  the socket clients connect to
//!
//! A host also stops its own session when nobody has used it for a while (see `Watchdog`), so forgotten
//! sessions don't pile up. Nothing is lost by that: the conversation resumes from its saved history. Its file names the
//! platoon version that started it, so a newer platoon can tell a host that keeps older rules (`idle::check_older`), and
//! says why a quiet session is kept (`kept`), for the session's row.

use crate::agents::{Agent, Start};
use crate::archive::config_dir;
use crate::idle::{self, Kept};
use crate::mcp::{self, Tracked};
use crate::procs;
use crate::procs::pid_alive;
use crate::settings::SettingsStore;
use crate::usage::cache_dir;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Read, Write},
    net::Shutdown,
    os::unix::{
        fs::{DirBuilderExt, PermissionsExt},
        net::{UnixListener, UnixStream},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// How often a host asks whether its session has been idle long enough to stop.
const IDLE_CHECK_EVERY: Duration = Duration::from_secs(30);
/// After an idle stop, how long MCP servers that outlive claude get to exit by themselves before they are asked to.
const LEFTOVER_GRACE: Duration = Duration::from_secs(3);
/// How long to wait for claude to exit after SIGTERM before giving up on cleaning up after it (it may be ignoring it,
/// and then its MCP servers are still in use).
const CLAUDE_EXIT_WAIT: Duration = Duration::from_secs(20);

pub const SCROLLBACK: usize = 5000;

pub const F_INPUT: u8 = b'I';
pub const F_RESIZE: u8 = b'R';
pub const F_OUTPUT: u8 = b'O';

/// Frame: kind (1 byte) + payload length (u32, big endian) + payload.
pub fn write_frame(w: &mut impl Write, kind: u8, payload: &[u8]) -> io::Result<()> {
    let mut head = [kind, 0, 0, 0, 0];
    head[1..].copy_from_slice(&(payload.len() as u32).to_be_bytes());
    w.write_all(&head)?;
    w.write_all(payload)?;
    w.flush()
}

pub fn read_frame(r: &mut impl Read) -> io::Result<(u8, Vec<u8>)> {
    let mut head = [0u8; 5];
    r.read_exact(&mut head)?;
    let len = u32::from_be_bytes([head[1], head[2], head[3], head[4]]) as usize;
    if len > 16 << 20 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut buf = vec![0; len];
    r.read_exact(&mut buf)?;
    Ok((head[0], buf))
}

pub fn size_payload(rows: u16, cols: u16) -> [u8; 4] {
    let (r, c) = (rows.to_be_bytes(), cols.to_be_bytes());
    [r[0], r[1], c[0], c[1]]
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Where platoon keeps its own state: the cache (hosts live in `<cache>/hosts`) and the config. A platoon window has
/// its own (`App`'s; in a unit test, temporary ones), and the hosts it starts are given the same.
#[derive(Clone, Debug)]
pub struct Dirs {
    pub cache: PathBuf,
    pub config: PathBuf,
}

impl Dirs {
    /// The real ones, from the environment (`usage::cache_dir`, `archive::config_dir`).
    pub fn from_env() -> Self {
        Self { cache: cache_dir(), config: config_dir() }
    }

    /// What a host started with these folders adds to its environment, so it (and the hooks of the claude inside)
    /// uses them too: nothing when they are the ones the environment already names, so a real user's sessions start
    /// exactly as before; the overrides when they differ (a unit test's temporary ones).
    fn child_env(&self, from_env: &Dirs) -> Vec<(&'static str, PathBuf)> {
        let mut env = Vec::new();
        if self.cache != from_env.cache {
            env.push(("PLATOON_CACHE_DIR", self.cache.clone()));
        }
        if self.config != from_env.config {
            env.push(("PLATOON_CONFIG_DIR", self.config.clone()));
        }
        env
    }
}

/// The real `<cache>/hosts` (for `platoon host`, `platoon open` and the hooks). A window uses its own `hosts_dir_in`.
pub fn hosts_dir() -> io::Result<PathBuf> {
    hosts_dir_in(&cache_dir())
}

pub fn hosts_dir_in(cache: &Path) -> io::Result<PathBuf> {
    let d = cache.join("hosts");
    // Anyone who can open the socket can type into claude, so the directory is owner-only.
    fs::DirBuilder::new().recursive(true).mode(0o700).create(&d)?;
    fs::set_permissions(&d, fs::Permissions::from_mode(0o700))?;
    Ok(d)
}

#[derive(Clone, Debug)]
pub struct HostInfo {
    /// The agent running inside, as the host's file names it (a host of a platoon from before agents were named in
    /// it runs Claude Code).
    pub agent: Agent,
    pub claude_pid: u32,
    pub host_pid: u32,
    pub dir: String,
    /// What was run inside: the arguments after the program (`--resume <id>`, `attach <id>`, none for a new session).
    pub args: Vec<String>,
    pub sock: PathBuf,
    /// The task whose session this is, as its environment names it (`agents::TASK_ENV`, `tickets::task_tag`): how a
    /// Codex task's session is found before (and after) Codex says its id.
    pub task: Option<String>,
    /// The platoon version that started the host, as its file says (None: a platoon from before hosts said it).
    pub version: Option<String>,
    /// Why the host keeps a session that has been quiet for the idle-stop time, as its file says (None: it doesn't).
    pub kept: Option<Kept>,
}

impl HostInfo {
    /// True when this host is showing the background session `attach_id` (running `claude attach <id>`).
    /// Such a host's own process is only an attach client: it has no registry entry, and the session it shows
    /// is run by Claude's service, which nothing here ever signals.
    pub fn attaches_to(&self, attach_id: &str) -> bool {
        self.agent.attaches(&self.args) && self.args == self.agent.args(Start::Attach(attach_id))
    }

    /// The session this host runs, when its arguments name it (a task's session, a resumed one).
    pub fn session(&self) -> Option<String> {
        self.agent.session_of_args(&self.args)
    }
}

/// The agent a host's file (`<pid>.json`) says it runs. A host of a platoon from before agents were named there runs
/// Claude Code.
pub fn agent_of_info(v: &Value) -> Agent {
    v.get("agent").and_then(Value::as_str).map_or(Some(Agent::Claude), Agent::from_id).unwrap_or(Agent::Claude)
}

/// Live hosts in the real cache folder. Entries whose claude has gone are cleaned up.
pub fn discover() -> Vec<HostInfo> {
    discover_in(&cache_dir())
}

/// Live hosts in `<cache>/hosts`. Entries whose claude has gone are cleaned up.
pub fn discover_in(cache: &Path) -> Vec<HostInfo> {
    let Ok(dir) = hosts_dir_in(cache) else { return Vec::new() };
    let mut out = Vec::new();
    for e in fs::read_dir(&dir).into_iter().flatten().flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Some(v) = fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) else { continue };
        let (Some(claude_pid), Some(host_pid)) = (v["claude_pid"].as_u64(), v["host_pid"].as_u64()) else { continue };
        let sock = dir.join(format!("{claude_pid}.sock"));
        if !pid_alive(claude_pid as u32) {
            let _ = fs::remove_file(&path);
            let _ = fs::remove_file(&sock);
            continue;
        }
        let args = v["args"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        out.push(HostInfo {
            agent: agent_of_info(&v),
            claude_pid: claude_pid as u32,
            host_pid: host_pid as u32,
            dir: v["dir"].as_str().unwrap_or("").to_string(),
            args,
            sock,
            task: v["task"].as_str().filter(|t| !t.is_empty()).map(String::from),
            version: v.get("version").and_then(Value::as_str).map(String::from),
            kept: v.get("kept").and_then(Kept::from_json),
        });
    }
    out
}

/// Start a detached host running `program args` (a session of `agent`) in `dir` and wait until it is ready to connect
/// to.
/// The returned `Child` should be polled with `try_wait` so it is reaped when the host exits.
/// `env` is added to the environment claude runs with (the host passes its own on). `PLATOON_PROJECT` and
/// `PLATOON_TASK` are only ever what `env` says: ones platoon itself inherited (say, started from inside a project's
/// session) are not passed on, and neither is `PLATOON_OFFER_TASKS`, which only platoon 1.0 set (for its offer to make
/// tasks) and nothing reads now.
/// The host keeps its files in `dirs.cache`, and is told so when that is not the one its environment names.
#[allow(clippy::too_many_arguments)]
pub fn launch(
    dirs: &Dirs,
    agent: Agent,
    program: &str,
    dir: &str,
    args: &[String],
    env: &[(&str, &str)],
    rows: u16,
    cols: u16,
) -> Result<(HostInfo, Child), String> {
    // Known before anything starts, so a window whose folders can't be made starts nothing.
    let hdir = hosts_dir_in(&dirs.cache).map_err(|e| e.to_string())?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.env_remove("PLATOON_PROJECT");
    cmd.env_remove("PLATOON_OFFER_TASKS");
    cmd.env_remove(crate::agents::TASK_ENV);
    // A unit test's folders are never the real ones (and a test must not look the real ones up).
    #[cfg(not(test))]
    let real = Dirs::from_env();
    #[cfg(test)]
    let real = Dirs { cache: PathBuf::new(), config: PathBuf::new() };
    cmd.envs(dirs.child_env(&real));
    cmd.envs(env.iter().copied());
    cmd.args(["host", "--agent", agent.id(), "--program", program, "--dir", dir, "--rows", &rows.to_string(), "--cols", &cols.to_string()])
        .arg("--")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // New session: no controlling terminal, so closing platoon's terminal can't hang it up.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd.spawn().map_err(|e| format!("couldn't start host: {e}"))?;
    let err_file = hdir.join(format!("{}.err", child.id()));
    for _ in 0..100 {
        if let Some(info) = discover_in(&dirs.cache).into_iter().find(|h| h.host_pid == child.id()) {
            return Ok((info, child));
        }
        if let Ok(Some(_)) = child.try_wait() {
            let msg = fs::read_to_string(&err_file).unwrap_or_else(|_| "host exited immediately".into());
            let _ = fs::remove_file(&err_file);
            return Err(msg);
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    Err("timed out waiting for the session to start".into())
}

/// `platoon host ...`: runs until the program inside exits. Returns the process exit code.
pub fn run(args: &[String]) -> i32 {
    let (mut dir, mut program, mut rows, mut cols) = (".".to_string(), "claude".to_string(), 24u16, 80u16);
    let mut agent = Agent::Claude;
    let mut rest: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dir" => dir = it.next().cloned().unwrap_or(dir),
            "--program" => program = it.next().cloned().unwrap_or(program),
            "--agent" => agent = it.next().and_then(|a| Agent::from_id(a)).unwrap_or(agent),
            "--rows" => rows = it.next().and_then(|s| s.parse().ok()).unwrap_or(rows),
            "--cols" => cols = it.next().and_then(|s| s.parse().ok()).unwrap_or(cols),
            "--" => {
                rest = it.cloned().collect();
                break;
            }
            _ => {}
        }
    }
    let Ok(hdir) = hosts_dir() else { return 1 };
    let me = std::process::id();
    let fail = |msg: String| {
        let _ = fs::write(hdir.join(format!("{me}.err")), msg);
        1
    };

    let pair = match native_pty_system().openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }) {
        Ok(p) => p,
        Err(e) => return fail(e.to_string()),
    };
    let mut cmd = CommandBuilder::new(&program);
    cmd.args(&rest);
    cmd.cwd(&dir);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    // If platoon itself was started from inside Claude Code, don't leak that session's markers into the child (with
    // CLAUDE_CODE_CHILD_SESSION inherited, a session saves no transcript: no resume, no tokens, no list row). Which
    // ones, and why: `claude::SESSION_MARKERS`.
    for k in crate::agents::inherited_session_markers() {
        cmd.env_remove(k);
    }
    let mut child = match pair.slave.spawn_command(cmd) {
        Ok(c) => c,
        Err(e) => return fail(format!("couldn't start {program}: {e}")),
    };
    drop(pair.slave);
    let Some(claude_pid) = child.process_id() else { return fail("no process id".into()) };
    let mut reader = match pair.master.try_clone_reader() {
        Ok(r) => r,
        Err(e) => return fail(e.to_string()),
    };
    let writer: Box<dyn Write + Send> = match pair.master.take_writer() {
        Ok(w) => w,
        Err(e) => return fail(e.to_string()),
    };

    let sock_path = hdir.join(format!("{claude_pid}.sock"));
    let info_path = hdir.join(format!("{claude_pid}.json"));
    let _ = fs::remove_file(&sock_path);
    let listener = match UnixListener::bind(&sock_path) {
        Ok(l) => l,
        Err(e) => return fail(format!("couldn't create socket: {e}")),
    };
    let _ = fs::set_permissions(&sock_path, fs::Permissions::from_mode(0o600));

    let shared = Arc::new(Shared {
        cleanup: Mutex::new(None),
        parser: Mutex::new(vt100::Parser::new(rows, cols, SCROLLBACK)),
        clients: Mutex::new(Vec::new()),
        master: Mutex::new(pair.master),
        writer: Mutex::new(writer),
        size: Mutex::new((rows, cols)),
        next_id: AtomicU64::new(1),
        quiet_since: Mutex::new(Instant::now()),
        active_at: Mutex::new(Instant::now()),
        info: Mutex::new(None),
    });

    {
        let shared = shared.clone();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let shared = shared.clone();
                thread::spawn(move || serve_client(stream, &shared));
            }
        });
    }

    // Publish only once the socket is listening, so a discovered host is always connectable.
    let mut info = json!({
        "agent": agent.id(), "version": crate::versions::running(), "claude_pid": claude_pid, "host_pid": me, "dir": dir, "args": rest
    });
    if let (Ok(task), Some(o)) = (std::env::var(crate::agents::TASK_ENV), info.as_object_mut()) {
        o.insert("task".into(), json!(task));
    }
    let file = InfoFile { value: info, path: info_path.clone(), tmp: hdir.join(format!(".{claude_pid}.tmp")) };
    if file.write().is_err() {
        return fail("couldn't write host info".into());
    }
    *lock(&shared.info) = Some(file);

    {
        let shared = shared.clone();
        let dog = Watchdog::new(agent, claude_pid, me, &rest);
        thread::spawn(move || watch(&shared, dog));
    }

    // Output loop: feed the emulator, then fan out to every window that already has its snapshot.
    let mut buf = [0u8; 8192];
    while let Ok(n) = reader.read(&mut buf) {
        if n == 0 {
            break;
        }
        let now = Instant::now();
        *lock(&shared.quiet_since) = now;
        *lock(&shared.active_at) = now;
        let mut p = lock(&shared.parser);
        p.process(&buf[..n]);
        // A window that can't take this in time is cut off, never left waiting: see `cut_off`.
        lock(&shared.clients).retain_mut(|c| {
            let fed = !c.ready || write_frame(&mut c.stream, F_OUTPUT, &buf[..n]).is_ok();
            if !fed {
                cut_off(c);
            }
            fed
        });
    }
    let _ = child.wait();
    {
        // Under the lock, so the idle check can't write the file again once it is gone.
        let mut file = lock(&shared.info);
        *file = None;
        let _ = fs::remove_file(&info_path);
    }
    let _ = fs::remove_file(&sock_path);
    // Don't exit in the middle of cleaning up after an idle stop (see `watch`).
    if let Some(cleanup) = lock(&shared.cleanup).take() {
        let _ = cleanup.join();
    }
    0
}

/// One connected window.
struct Viewer {
    id: u64,
    stream: UnixStream,
    rows: u16,
    cols: u16,
    /// Gets live output only once its snapshot has been sent, so nothing arrives out of order.
    ready: bool,
}

struct Shared {
    parser: Mutex<vt100::Parser>,
    clients: Mutex<Vec<Viewer>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    /// Size currently applied to the terminal.
    size: Mutex<(u16, u16)>,
    next_id: AtomicU64,
    /// The latest of: the last output from claude, the last window leaving, the last time claude was
    /// seen doing anything but idling. The idle-stop clock runs from here.
    quiet_since: Mutex<Instant>,
    /// The latest of: the last output from claude, the last window leaving. What "quiet" means for saying why a
    /// session is kept (`quiet_since` also moves while the agent isn't idle).
    active_at: Mutex<Instant>,
    /// After an idle stop: the thread that ends MCP servers which outlive claude. The host waits for it.
    cleanup: Mutex<Option<thread::JoinHandle<()>>>,
    /// The host's file (`<pid>.json`), while it is there.
    info: Mutex<Option<InfoFile>>,
}

/// The host's file, as written, so a change (why the session is kept) can be written again whole.
struct InfoFile {
    value: Value,
    path: PathBuf,
    tmp: PathBuf,
}

impl InfoFile {
    /// Written whole and renamed into place, so a window never reads half of it.
    fn write(&self) -> io::Result<()> {
        fs::write(&self.tmp, self.value.to_string())?;
        fs::rename(&self.tmp, &self.path)
    }

    /// Say why the session is kept (None: it isn't, or nothing worth saying), writing the file only when that changed.
    fn set_kept(&mut self, kept: Option<&Kept>) {
        let want = kept.map(Kept::to_json);
        let Some(o) = self.value.as_object_mut() else { return };
        if o.get("kept") == want.as_ref() {
            return;
        }
        match want {
            Some(k) => o.insert("kept".into(), k),
            None => o.remove("kept"),
        };
        let _ = self.write();
    }
}

/// What the idle check decided, and if it is "not yet", the first reason why. The reason is also what the session's
/// row says about a quiet session that is kept (`Reason::kept`).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Decision {
    /// Stop the session. Holds what runs under claude that isn't a command it started (its MCP servers and what they
    /// started), to clean up if it outlives claude.
    Stop(Vec<Tracked>),
    Keep(Reason),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Reason {
    /// Idle stop is switched off (0 minutes).
    IdleStopOff,
    /// A window is attached.
    Viewer,
    /// The agent doesn't say the session is idle (`Agent::known_idle`).
    NotIdle,
    /// Not quiet for the whole threshold yet: recent output, or a window left recently.
    RecentOutput,
    /// A task's session whose task is Working: its subagents may be at work while nobody watches.
    TaskAtWork,
    /// The process table couldn't be read, so what runs under claude is unknown.
    ProcessesUnknown,
    /// A command claude started is still running (`idle::under`); its short form, when it has one.
    Command(Option<String>),
}

impl Reason {
    /// What the session's row says about it, once it has been `quiet` (no output, no window leaving) for the idle-stop
    /// time. None: nothing to say (still in its quiet time, or idle stop is off).
    fn kept(&self, quiet: bool) -> Option<Kept> {
        match self {
            Reason::IdleStopOff | Reason::RecentOutput => None,
            Reason::Viewer => quiet.then_some(Kept::Window),
            Reason::NotIdle => quiet.then_some(Kept::NotIdle),
            Reason::TaskAtWork => Some(Kept::TaskAtWork),
            Reason::ProcessesUnknown => Some(Kept::Unknown),
            Reason::Command(what) => Some(Kept::Command(what.clone())),
        }
    }
}

/// Decides when this host's session has been idle long enough to stop, and stops it.
///
/// Idle means all of: no window attached for the whole threshold, claude's own registry entry saying
/// `idle` the whole time (missing, unreadable or any other status is NOT idle, so uncertainty never
/// stops anything), no output from claude for the threshold, and no command claude started still running
/// (`idle::under`: its MCP servers and other helpers don't count).
/// Stopping is SIGTERM to claude, which shuts down cleanly; the host then exits like it always does.
/// The conversation is untouched, and Enter resumes it.
///
/// The threshold comes from `settings.json` (`idle_stop_minutes`, 0 = off) and is re-read as it changes.
/// Tests can shrink time with `PLATOON_IDLE_SECS_PER_MINUTE=N`: a "minute" of the setting then lasts N
/// seconds and the check runs every 200 ms instead of every 30 s.
struct Watchdog {
    agent: Agent,
    claude_pid: u32,
    host_pid: u32,
    /// The agent's own folder (`Agent::home`), where it says whether the session is idle.
    root: PathBuf,
    settings: SettingsStore,
    secs_per_minute: u64,
    check_every: Duration,
    /// How long a shell under claude must have run to count as a command it started: half a "minute" (see `idle`).
    settle: Duration,
    leftover_grace: Duration,
    /// The session this host runs, when its arguments name it (a task's session, a resumed one): a task's session is
    /// found by it.
    session: Option<String>,
    /// platoon's config folder, where the tasks are.
    config: PathBuf,
}

impl Watchdog {
    fn new(agent: Agent, claude_pid: u32, host_pid: u32, args: &[String]) -> Self {
        let scaled = std::env::var("PLATOON_IDLE_SECS_PER_MINUTE").ok().and_then(|v| v.parse::<u64>().ok());
        let secs_per_minute = scaled.unwrap_or(60);
        Self {
            agent,
            claude_pid,
            host_pid,
            root: agent.home(),
            settings: SettingsStore::open(&config_dir()),
            secs_per_minute,
            check_every: if scaled.is_some() { Duration::from_millis(200) } else { IDLE_CHECK_EVERY },
            settle: Duration::from_secs(secs_per_minute) / 2,
            leftover_grace: if scaled.is_some() { Duration::from_secs(1) } else { LEFTOVER_GRACE },
            session: agent.session_of_args(args),
            config: config_dir(),
        }
    }

    /// True when the session should be stopped now.
    #[cfg(test)]
    fn should_stop(&mut self, now: Instant, viewers: usize, quiet_since: &mut Instant) -> bool {
        matches!(self.decide(now, viewers, quiet_since), Decision::Stop(_))
    }

    /// Whether to stop the session now. `quiet_since` is moved forward whenever claude is seen not idle.
    /// `now` and the window count are passed in so the decision can be tested.
    fn decide(&mut self, now: Instant, viewers: usize, quiet_since: &mut Instant) -> Decision {
        self.settings.refresh();
        let idle = self.agent.known_idle(&self.root, self.claude_pid, self.session.as_deref());
        if !idle {
            *quiet_since = (*quiet_since).max(now);
        }
        let settings = self.settings.get();
        let Some(threshold) = settings.idle_stop_after(self.secs_per_minute) else { return Decision::Keep(Reason::IdleStopOff) };
        if viewers > 0 {
            return Decision::Keep(Reason::Viewer);
        }
        if !idle {
            return Decision::Keep(Reason::NotIdle);
        }
        if now.saturating_duration_since(*quiet_since) < threshold {
            return Decision::Keep(Reason::RecentOutput);
        }
        // Only now, when everything else says stop: a few small reads of the tasks files.
        if self.session.as_deref().is_some_and(|s| crate::tickets::session_at_work(&self.config, s)) {
            return Decision::Keep(Reason::TaskAtWork);
        }
        self.decide_processes()
    }

    /// Stopping claude would end a command it started (a dev server, a watcher, ...), so one still running keeps the
    /// session, and so does not knowing what runs under it. Everything else under it (its MCP servers, ...) is
    /// recorded for the clean-up after the stop.
    fn decide_processes(&self) -> Decision {
        let unknown = Decision::Keep(Reason::ProcessesUnknown);
        let Some(rows) = procs::table() else { return unknown };
        let Some(under) = idle::under(self.agent, &rows, self.claude_pid, &procs::cmdline, self.settle) else { return unknown };
        match under.command {
            Some(c) => Decision::Keep(Reason::Command(c.what)),
            None => Decision::Stop(under.leftovers),
        }
    }

    /// What the session's row is to say about the decision: why a session quiet since `active_at` is kept.
    fn kept(&self, decision: &Decision, now: Instant, active_at: Instant) -> Option<Kept> {
        let Decision::Keep(reason) = decision else { return None };
        let threshold = self.settings.get().idle_stop_after(self.secs_per_minute)?;
        reason.kept(now.saturating_duration_since(active_at) >= threshold)
    }
}

fn watch(sh: &Shared, mut dog: Watchdog) {
    loop {
        thread::sleep(dog.check_every);
        let viewers = lock(&sh.clients).len();
        let mut since = *lock(&sh.quiet_since);
        let now = Instant::now();
        let decision = dog.decide(now, viewers, &mut since);
        // Why a quiet session is kept, in the host's file for the session's row.
        let kept = dog.kept(&decision, now, *lock(&sh.active_at));
        if let Some(file) = lock(&sh.info).as_mut() {
            file.set_kept(kept.as_ref());
        }
        // Output or a window leaving during the decision means it is not idle after all.
        let stop = {
            let mut shared_since = lock(&sh.quiet_since);
            let moved = *shared_since > since;
            *shared_since = (*shared_since).max(since);
            !moved
        };
        let (stop, tracked) = match decision {
            Decision::Stop(t) => (stop, t),
            Decision::Keep(_) => (false, Vec::new()),
        };
        // A window may have joined while we were deciding. Only ever signal our own child: pid must still
        // be a direct child of this host, so a reused pid is never touched.
        let ours = dog.claude_pid > 1 && procs::ppid(dog.claude_pid) == Some(dog.host_pid);
        if stop && ours && lock(&sh.clients).is_empty() {
            unsafe { libc::kill(dog.claude_pid as i32, libc::SIGTERM) };
            // Some MCP servers don't notice that claude has gone and would run on with nobody to talk to. Once
            // it has gone, those recorded before the stop (and still the same process) are asked to exit too.
            // Its own thread: an MCP server holding the terminal open also keeps this host's output loop going.
            if !tracked.is_empty() {
                let (claude, grace) = (dog.claude_pid, dog.leftover_grace);
                *lock(&sh.cleanup) = Some(thread::spawn(move || mcp::end_leftovers_after(claude, &tracked, CLAUDE_EXIT_WAIT, grace)));
            }
            // If it doesn't go (something ignoring SIGTERM), try again after another full quiet period, not every check.
            *lock(&sh.quiet_since) = Instant::now();
        }
    }
}

/// One terminal can only be one size. Fit the smallest attached window so every window shows everything
/// (this is what tmux does). With nobody attached, the last size stays.
fn fit_smallest_viewer(sh: &Shared) {
    let want = {
        let clients = lock(&sh.clients);
        clients.iter().map(|c| (c.rows, c.cols)).reduce(|a, b| (a.0.min(b.0), a.1.min(b.1)))
    };
    let Some((rows, cols)) = want else { return };
    let (rows, cols) = (rows.max(1), cols.max(1));
    let mut cur = lock(&sh.size);
    if *cur == (rows, cols) {
        return; // resizing to the same size would make claude redraw for nothing
    }
    *cur = (rows, cols);
    let _ = lock(&sh.master).resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
    lock(&sh.parser).screen_mut().set_size(rows, cols);
}

fn parse_size(payload: &[u8]) -> Option<(u16, u16)> {
    (payload.len() == 4).then(|| (u16::from_be_bytes([payload[0], payload[1]]), u16::from_be_bytes([payload[2], payload[3]])))
}

/// Give up on a window that didn't take its output in time (it is slow, or stuck): close its connection, both ways.
/// Only dropping it from the list would leave it connected but sent nothing ever again, its screen frozen while its keys
/// still reach the session; closed, the window sees the end, connects again and gets a fresh snapshot, and this
/// window's `serve_client` sees it too and finishes. The caller takes it off the list.
fn cut_off(viewer: &Viewer) {
    let _ = viewer.stream.shutdown(Shutdown::Both);
}

fn serve_client(mut stream: UnixStream, sh: &Shared) {
    // The first frame is the window's size, so the snapshot below is made at the right size.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let Ok((F_RESIZE, payload)) = read_frame(&mut stream) else { return };
    let Some((rows, cols)) = parse_size(&payload) else { return };
    let _ = stream.set_read_timeout(None);
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    let Ok(writer_half) = stream.try_clone() else { return };

    let id = sh.next_id.fetch_add(1, Ordering::Relaxed);
    lock(&sh.clients).push(Viewer { id, stream: writer_half, rows, cols, ready: false });
    fit_smallest_viewer(sh);
    {
        // Snapshot, then start live output, under the parser lock: no output can slip in between.
        let p = lock(&sh.parser);
        let snapshot = p.screen().state_formatted();
        let mut clients = lock(&sh.clients);
        let Some(me) = clients.iter_mut().find(|c| c.id == id) else { return };
        if write_frame(&mut me.stream, F_OUTPUT, &snapshot).is_err() {
            cut_off(me);
            clients.retain(|c| c.id != id);
            return;
        }
        me.ready = true;
    }
    while let Ok((kind, payload)) = read_frame(&mut stream) {
        match kind {
            F_INPUT => {
                let mut w = lock(&sh.writer);
                let _ = w.write_all(&payload);
                let _ = w.flush();
            }
            F_RESIZE => {
                if let Some((r, c)) = parse_size(&payload) {
                    if let Some(me) = lock(&sh.clients).iter_mut().find(|c| c.id == id) {
                        me.rows = r;
                        me.cols = c;
                    }
                    fit_smallest_viewer(sh);
                }
            }
            _ => {}
        }
    }
    // This window left: forget it, and let the terminal grow back if it was the smallest.
    lock(&sh.clients).retain(|c| c.id != id);
    let now = Instant::now();
    *lock(&sh.quiet_since) = now;
    *lock(&sh.active_at) = now;
    fit_smallest_viewer(sh);
}

/// How long a stop waits for a process to exit after SIGTERM. Claude shuts down cleanly in about a second.
pub const STOP_WAIT: Duration = Duration::from_secs(5);

/// SIGTERM each process and wait for all of them to exit. Used when ending copies before reopening.
/// Blocks for as long as the processes take (up to `STOP_WAIT`): never call it from the UI's own thread when the
/// screen would freeze for that long (the archive key runs it on a worker: see `App::start_stop`).
pub fn end_processes(pids: &[u32]) -> Result<(), String> {
    end_processes_within(pids, STOP_WAIT)
}

/// `end_processes` with the waiting time given, so a test does not have to wait for the real one.
pub fn end_processes_within(pids: &[u32], wait: Duration) -> Result<(), String> {
    for &pid in pids {
        if pid <= 1 {
            return Err(format!("couldn't stop pid {pid}"));
        }
        // Already gone (it ended by itself since the list was last read) is what we wanted: nothing to signal.
        if unsafe { libc::kill(pid as i32, libc::SIGTERM) } != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
            return Err(format!("couldn't stop pid {pid}"));
        }
    }
    let deadline = Instant::now() + wait;
    loop {
        if !pids.iter().any(|&p| pid_alive(p)) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("it didn't exit in time".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::claude::{claude_root, registry_status, session_of_args};
    use crate::settings::{Settings, save};
    use std::process::Child;

    /// A watchdog watching a real `sleep` (standing in for claude) with a fake registry and settings.
    struct Rig {
        dog: Watchdog,
        dir: PathBuf,
        child: Child,
    }

    impl Rig {
        fn new(name: &str, settings: Settings) -> Self {
            let dir = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-host-{name}-{}", std::process::id())));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("claude/sessions")).unwrap();
            save(&dir.join("config"), &settings).unwrap();
            let child = Command::new("sleep").arg("60").spawn().unwrap();
            let dog = Watchdog {
                agent: Agent::Claude,
                claude_pid: child.id(),
                host_pid: std::process::id(),
                root: dir.join("claude"),
                settings: SettingsStore::open(&dir.join("config")),
                secs_per_minute: 60,
                check_every: IDLE_CHECK_EVERY,
                // Every shell counts at once, however young (see `a_hook_running_for_a_moment_does_not_prevent_a_stop`).
                settle: Duration::ZERO,
                leftover_grace: Duration::from_millis(300),
                session: None,
                config: dir.join("config"),
            };
            Rig { dog, dir, child }
        }

        fn status(&self, status: &str) {
            let v = json!({"pid": self.child.id(), "sessionId": "s", "kind": "interactive", "status": status});
            fs::write(self.dir.join("claude/sessions").join(format!("{}.json", self.child.id())), v.to_string()).unwrap();
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    const MIN: Duration = Duration::from_secs(60);

    #[test]
    fn stops_only_after_the_whole_threshold_of_quiet_idle_time_with_nobody_attached() {
        let mut rig = Rig::new("decide", Settings { idle_stop_minutes: 120, auto_archive_days: 2, ..Settings::default() });
        rig.status("idle");
        let t0 = Instant::now();
        let mut since = t0;
        assert!(!rig.dog.should_stop(t0 + 119 * MIN, 0, &mut since), "one minute short");
        assert!(rig.dog.should_stop(t0 + 120 * MIN, 0, &mut since), "exactly the threshold");
        assert_eq!(since, t0, "being idle doesn't move the clock");
    }

    #[test]
    fn each_blocker_alone_prevents_a_stop() {
        let mut rig = Rig::new("blockers", Settings { idle_stop_minutes: 120, auto_archive_days: 2, ..Settings::default() });
        let t0 = Instant::now();
        let later = t0 + 500 * MIN;

        rig.status("idle");
        assert!(!rig.dog.should_stop(later, 1, &mut { t0 }), "a window is attached");

        for other in ["busy", "waiting", "", "IDLE"] {
            rig.status(other);
            let mut since = t0;
            assert!(!rig.dog.should_stop(later, 0, &mut since), "status {other:?} is not idle");
            assert_eq!(since, later, "and it restarts the quiet period");
        }

        // No registry entry, or one that can't be read: not known to be idle.
        let file = rig.dir.join("claude/sessions").join(format!("{}.json", rig.child.id()));
        fs::write(&file, "{ not json").unwrap();
        assert!(!rig.dog.should_stop(later, 0, &mut { t0 }));
        fs::remove_file(&file).unwrap();
        assert!(!rig.dog.should_stop(later, 0, &mut { t0 }));
        // An entry that belongs to a different pid isn't this process's status.
        fs::write(&file, json!({"pid": rig.child.id() + 1, "status": "idle"}).to_string()).unwrap();
        assert!(!rig.dog.should_stop(later, 0, &mut { t0 }));

        // Recent output/detach: the quiet period hasn't run its course.
        rig.status("idle");
        assert!(!rig.dog.should_stop(later, 0, &mut { later - MIN }));
        // Everything clear: it does stop.
        assert!(rig.dog.should_stop(later, 0, &mut { t0 }));
    }

    #[test]
    fn a_host_showing_a_background_session_is_never_stopped_and_only_ever_targets_its_own_child() {
        // The host's child is a `claude attach`: no registry entry of its own. Claude's service runs the session,
        // and its entry (kind "bg", idle) belongs to another pid. Neither may make the host stop anything.
        let mut rig = Rig::new("bg-attach", Settings { idle_stop_minutes: 1, auto_archive_days: 2, ..Settings::default() });
        let mut daemon_child = Command::new("sleep").arg("60").spawn().unwrap(); // stands in for the service's process
        let v = json!({"pid": daemon_child.id(), "sessionId": "feedf00d-x", "kind": "bg", "status": "idle", "jobId": "feedf00d"});
        fs::write(rig.dir.join("claude/sessions").join(format!("{}.json", daemon_child.id())), v.to_string()).unwrap();
        let t0 = Instant::now();
        let mut since = t0;
        assert_eq!(
            rig.dog.decide(t0 + 10_000 * MIN, 0, &mut since),
            Decision::Keep(Reason::NotIdle),
            "no entry of its own: not known to be idle"
        );
        // Even a stray entry naming the service's pid is not this host's status: the lookup is by the host's own child.
        assert_eq!(registry_status(&rig.dir.join("claude"), rig.child.id()), None);
        let _ = daemon_child.kill();
        let _ = daemon_child.wait();
    }

    #[test]
    fn a_hosted_session_that_moved_to_the_background_is_never_idle_stopped() {
        // Claude Code 2.1.283 parks the interactive process when it hands the conversation to a background session:
        // alive, `parkedJobId` set, its status left as it was. Stopping it is not platoon's call, whatever that says.
        let mut rig = Rig::new("parked", Settings { idle_stop_minutes: 1, auto_archive_days: 2, ..Settings::default() });
        let file = rig.dir.join("claude/sessions").join(format!("{}.json", rig.child.id()));
        let t0 = Instant::now();
        for status in ["busy", "idle"] {
            let v = json!({"pid": rig.child.id(), "sessionId": "s", "kind": "interactive", "status": status, "parkedJobId": "beefcafe"});
            fs::write(&file, v.to_string()).unwrap();
            assert_eq!(registry_status(&rig.dir.join("claude"), rig.child.id()).as_deref(), Some("parked"));
            let mut since = t0;
            assert_eq!(rig.dog.decide(t0 + 10_000 * MIN, 0, &mut since), Decision::Keep(Reason::NotIdle), "status {status:?}");
        }
        // Once it is no longer parked, it is an ordinary session again.
        rig.status("idle");
        assert!(rig.dog.should_stop(t0 + 10_000 * MIN, 0, &mut { t0 }));
    }

    #[test]
    fn a_host_knows_which_background_session_it_shows_from_its_arguments() {
        let info = |args: &[&str]| HostInfo {
            agent: crate::agents::Agent::Claude,
            claude_pid: 1,
            host_pid: 2,
            dir: String::new(),
            args: args.iter().map(|a| a.to_string()).collect(),
            sock: PathBuf::new(),
            task: None,
            version: None,
            kept: None,
        };
        assert!(info(&["attach", "feedf00d"]).attaches_to("feedf00d"));
        assert!(!info(&["attach", "feedf00d"]).attaches_to("feedf00"));
        assert!(!info(&["--resume", "feedf00d"]).attaches_to("feedf00d"));
        assert!(!info(&[]).attaches_to("feedf00d"));
    }

    #[test]
    fn a_command_claude_started_prevents_a_stop_and_a_finished_one_does_not() {
        let mut rig = Rig::new("children", Settings { idle_stop_minutes: 1, auto_archive_days: 2, ..Settings::default() });
        // `sleep` has no children, so watch a shell that starts a command in a shell of its own (as Claude Code's Bash
        // tool does) in the background and then waits for input.
        let mut shell = Command::new("sh").args(["-c", "sh -c 'sleep 60; true' & read x"]).stdin(Stdio::piped()).spawn().unwrap();
        rig.dog.claude_pid = shell.id();
        let v = json!({"pid": shell.id(), "status": "idle"});
        fs::write(rig.dir.join(format!("claude/sessions/{}.json", shell.id())), v.to_string()).unwrap();

        let kids = loop {
            match procs::live_descendants(shell.id()) {
                Some(k) if k.len() >= 2 => break k,
                _ => thread::sleep(Duration::from_millis(20)), // the shell needs a moment to start its job
            }
        };
        let t0 = Instant::now();
        let blocked = rig.dog.decide(t0 + 10 * MIN, 0, &mut { t0 });

        // The command ends (it is ours): now nothing runs under the shell, so it may be stopped.
        for k in kids {
            unsafe { libc::kill(k as i32, libc::SIGKILL) };
        }
        let mut freed = false;
        for _ in 0..100 {
            freed = rig.dog.should_stop(t0 + 10 * MIN, 0, &mut { t0 });
            if freed {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        drop(shell.stdin.take()); // the shell's `read` ends and it exits
        let _ = shell.wait();
        assert_eq!(blocked, Decision::Keep(Reason::Command(Some("sleep 60".into()))), "stopping claude would end it");
        assert!(freed, "once the command is gone (a zombie doesn't count) the session may stop");
    }

    #[test]
    fn threshold_zero_is_off_and_an_edit_to_the_settings_applies_at_once() {
        let mut rig = Rig::new("off", Settings { idle_stop_minutes: 0, auto_archive_days: 2, ..Settings::default() });
        rig.status("idle");
        let t0 = Instant::now();
        assert!(!rig.dog.should_stop(t0 + 10_000 * MIN, 0, &mut { t0 }), "0 = never stop");
        save(&rig.dir.join("config"), &Settings { idle_stop_minutes: 30, auto_archive_days: 2, ..Settings::default() }).unwrap();
        assert!(!rig.dog.should_stop(t0 + 29 * MIN, 0, &mut { t0 }));
        assert!(rig.dog.should_stop(t0 + 30 * MIN, 0, &mut { t0 }), "the new value is used without restarting the host");
        save(&rig.dir.join("config"), &Settings { idle_stop_minutes: 0, auto_archive_days: 2, ..Settings::default() }).unwrap();
        assert!(!rig.dog.should_stop(t0 + 30 * MIN, 0, &mut { t0 }), "and switching it off applies too");
    }

    #[test]
    fn a_missing_or_corrupt_settings_file_means_the_default_two_hours() {
        let mut rig = Rig::new("default", Settings::default());
        fs::write(rig.dir.join("config/settings.json"), "{ nope").unwrap();
        rig.status("idle");
        let t0 = Instant::now();
        assert!(!rig.dog.should_stop(t0 + 119 * MIN, 0, &mut { t0 }));
        assert!(rig.dog.should_stop(t0 + 120 * MIN, 0, &mut { t0 }));
    }

    #[test]
    fn a_tasks_session_is_not_stopped_while_its_task_is_working_and_is_like_any_session_after() {
        use crate::tickets::{Need, NewTicket, State, Tickets};
        let mut rig = Rig::new("task", Settings { idle_stop_minutes: 1, auto_archive_days: 2, ..Settings::default() });
        rig.status("idle");
        rig.dog.session =
            session_of_args(&["--add-dir".into(), "/x".into(), "--session-id".into(), "task-1".into(), "-n".into(), "#1 x".into()]);
        assert_eq!(rig.dog.session.as_deref(), Some("task-1"));
        let config = rig.dir.join("config");
        let mut t = Tickets::open(&config, "p1");
        let n = t.add(NewTicket { title: "Basket".into(), ..Default::default() }).unwrap();
        t.start(n, "feature", "task-1", None, &[]).unwrap();
        let t0 = Instant::now();
        assert_eq!(rig.dog.decide(t0 + 500 * MIN, 0, &mut { t0 }), Decision::Keep(Reason::TaskAtWork));
        for s in [State::NeedsYou(Need::Plan), State::Done, State::Cancelled] {
            Tickets::open(&config, "p1")
                .update(n, |t| {
                    t.state = s.clone();
                    true
                })
                .unwrap();
            assert!(rig.dog.should_stop(t0 + 500 * MIN, 0, &mut { t0 }), "{s:?}");
        }
        // Any other session, and a resumed one.
        assert_eq!(session_of_args(&["--resume".into(), "task-1".into(), "--model".into()]).as_deref(), Some("task-1"));
        assert_eq!(session_of_args(&[]), None);
        assert_eq!(session_of_args(&["attach".into(), "abc".into()]), None);
    }

    // --- MCP servers -------------------------------------------------------------------------------------

    /// A stand-in claude: a shell running `script` (which starts its children and then waits for input), with
    /// an `idle` registry entry, watched by the rig's watchdog. Everything under it is ours and is killed on drop.
    struct Session {
        shell: Child,
    }

    impl Session {
        fn start(rig: &mut Rig, script: &str, children: usize) -> Self {
            // Held in `Self` from the start, so a panic below still kills and reaps the shell and its children (Drop).
            let session = Self { shell: Command::new("sh").args(["-c", script]).stdin(Stdio::piped()).spawn().unwrap() };
            let pid = session.shell.id();
            rig.dog.claude_pid = pid;
            let v = json!({"pid": pid, "sessionId": "s", "kind": "interactive", "status": "idle"});
            fs::write(rig.dir.join(format!("claude/sessions/{pid}.json")), v.to_string()).unwrap();
            for _ in 0..250 {
                if procs::live_descendants(pid).is_some_and(|k| k.len() >= children) {
                    return session;
                }
                thread::sleep(Duration::from_millis(20));
            }
            panic!("the stand-in claude never started {children} children");
        }

        /// The pid of the process under the shell whose command line starts with `word`.
        fn pid_with(&self, word: &str) -> u32 {
            // The process table can be momentarily unreadable when the machine is busy (the whole suite runs at
            // once), so look again for a moment before deciding it isn't there.
            for _ in 0..100 {
                let found = procs::live_descendants(self.shell.id())
                    .and_then(|kids| kids.into_iter().find(|&p| procs::cmdline(p).is_some_and(|a| a.join(" ").starts_with(word))));
                if let Some(pid) = found {
                    return pid;
                }
                thread::sleep(Duration::from_millis(20));
            }
            panic!("nothing running with {word:?}")
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            for pid in procs::live_descendants(self.shell.id()).unwrap_or_default() {
                unsafe { libc::kill(pid as i32, libc::SIGKILL) };
            }
            let _ = self.shell.kill();
            let _ = self.shell.wait();
        }
    }

    impl Rig {
        /// The decision with everything else ready to stop: idle, long quiet, nobody attached.
        fn decide(&mut self) -> Decision {
            let t0 = Instant::now();
            self.dog.decide(t0 + 500 * MIN, 0, &mut { t0 })
        }
    }

    fn pids(d: &Decision) -> Vec<u32> {
        match d {
            Decision::Stop(t) => {
                let mut v: Vec<u32> = t.iter().map(|t| t.pid).collect();
                v.sort();
                v
            }
            other => panic!("expected a stop, got {other:?}"),
        }
    }

    /// A Codex session platoon hosts: idle is what its rollout's last turn record says, and a command it started keeps
    /// it as a command of Claude's does.
    #[test]
    fn a_codex_session_stops_only_when_its_rollout_says_idle_and_no_command_of_its_runs() {
        let mut rig = Rig::new("codex", Settings::default());
        let codex = rig.dir.join("codex");
        let id = "01a1517c-414d-7902-93d1-efa74830493d";
        let day = codex.join("sessions/2026/10/18");
        fs::create_dir_all(&day).unwrap();
        let rollout = day.join(format!("rollout-2026-10-18T20-07-40-{id}.jsonl"));
        let turn =
            |kind: &str| json!({"timestamp": "2026-10-19T00:07:40.187Z", "type": "event_msg", "payload": {"type": kind}}).to_string();
        fs::write(&rollout, [turn("task_started"), turn("task_complete")].join("\n") + "\n").unwrap();
        rig.dog.agent = Agent::Codex;
        rig.dog.root = codex.clone();
        rig.dog.session = Agent::Codex.session_of_args(&Agent::Codex.args(Start::Resume(id)));

        let s = Session::start(&mut rig, "sleep 621 & sh -c 'sleep 622; true' & read x", 3);
        assert_eq!(rig.decide(), Decision::Keep(Reason::Command(Some("sleep 622".into()))), "a command Codex started");
        drop(s);
        let s = Session::start(&mut rig, "sleep 621 & read x", 1);
        assert_eq!(pids(&rig.decide()), [s.pid_with("sleep 621")], "its MCP server alone doesn't keep it");

        // In the middle of a turn: not idle.
        fs::write(&rollout, [turn("task_complete"), turn("task_started")].join("\n") + "\n").unwrap();
        assert_eq!(rig.decide(), Decision::Keep(Reason::NotIdle));
        // An interrupted turn has ended.
        fs::write(&rollout, [turn("task_started"), turn("turn_aborted")].join("\n") + "\n").unwrap();
        assert!(matches!(rig.decide(), Decision::Stop(_)));
        // Nothing to tell from: no turn record, no rollout, no session named.
        fs::write(&rollout, "{\"type\":\"session_meta\",\"payload\":{}}\n").unwrap();
        assert_eq!(rig.decide(), Decision::Keep(Reason::NotIdle));
        fs::write(&rollout, [turn("task_complete")].join("\n") + "\n").unwrap();
        rig.dog.session = Some("01a1517c-0000-7000-8000-000000000000".into());
        assert_eq!(rig.decide(), Decision::Keep(Reason::NotIdle));
        rig.dog.session = None;
        assert_eq!(rig.decide(), Decision::Keep(Reason::NotIdle));
    }

    #[test]
    fn what_claude_runs_that_is_not_a_command_does_not_prevent_a_stop_and_is_recorded_for_clean_up() {
        // MCP servers: platoon reads no configuration for them; they are simply not a command claude started.
        let mut rig = Rig::new("mcp-ignored", Settings::default());
        let s = Session::start(&mut rig, "sleep 611 & sleep 612 & read x", 2);
        let d = rig.decide();
        let mut want = vec![s.pid_with("sleep 611"), s.pid_with("sleep 612")];
        want.sort();
        assert_eq!(pids(&d), want);
        let Decision::Stop(tracked) = d else { unreachable!() };
        assert!(tracked.iter().all(|t| t.start != 0 && t.argv.is_some()), "each is recorded so it can be recognised again");
    }

    #[test]
    fn a_command_prevents_the_stop_even_beside_mcp_servers() {
        let mut rig = Rig::new("mcp-other", Settings::default());
        let _s = Session::start(&mut rig, "sleep 611 & sh -c 'sleep 612; true' & read x", 3);
        assert_eq!(rig.decide(), Decision::Keep(Reason::Command(Some("sleep 612".into()))));
        assert!(!rig.dog.should_stop(Instant::now() + 500 * MIN, 0, &mut Instant::now()));
    }

    #[test]
    fn a_hook_running_for_a_moment_does_not_prevent_a_stop() {
        let mut rig = Rig::new("hook", Settings::default());
        rig.dog.settle = Duration::from_secs(600);
        // A shell started just now, as a hook or the status line is: not counted yet, and not a leftover either.
        let s = Session::start(&mut rig, "sleep 611 & sh -c 'sleep 613; true' & read x", 3);
        assert_eq!(pids(&rig.decide()), [s.pid_with("sleep 611")]);
    }

    #[test]
    fn why_a_quiet_session_is_kept_is_said_once_it_has_been_quiet_for_the_threshold() {
        let rig = Rig::new("kept", Settings::default());
        let (t0, later) = (Instant::now(), Instant::now() + 500 * MIN);
        let keep = |r: Reason| Decision::Keep(r);
        assert_eq!(
            rig.dog.kept(&keep(Reason::Command(Some("npm run dev".into()))), later, t0),
            Some(Kept::Command(Some("npm run dev".into())))
        );
        assert_eq!(rig.dog.kept(&keep(Reason::Viewer), later, t0), Some(Kept::Window));
        assert_eq!(rig.dog.kept(&keep(Reason::Viewer), later, later - MIN), None, "a window that is in use");
        assert_eq!(rig.dog.kept(&keep(Reason::NotIdle), later, t0), Some(Kept::NotIdle));
        assert_eq!(rig.dog.kept(&keep(Reason::NotIdle), later, later - MIN), None, "working away");
        assert_eq!(rig.dog.kept(&keep(Reason::TaskAtWork), later, t0), Some(Kept::TaskAtWork));
        assert_eq!(rig.dog.kept(&keep(Reason::ProcessesUnknown), later, t0), Some(Kept::Unknown));
        assert_eq!(rig.dog.kept(&keep(Reason::RecentOutput), later, t0), None);
        assert_eq!(rig.dog.kept(&Decision::Stop(Vec::new()), later, t0), None);
        save(&rig.dir.join("config"), &Settings { idle_stop_minutes: 0, ..Settings::default() }).unwrap();
        let mut off = rig;
        off.dog.settings.refresh();
        assert_eq!(off.dog.kept(&keep(Reason::Command(None)), later, t0), None, "idle stop is off: nothing to explain");
    }

    #[test]
    fn every_reason_not_to_stop_is_reported_as_itself() {
        let mut rig = Rig::new("reasons", Settings::default());
        let (t0, later) = (Instant::now(), Instant::now() + 500 * MIN);
        rig.status("idle");
        assert_eq!(rig.dog.decide(later, 1, &mut { t0 }), Decision::Keep(Reason::Viewer));
        rig.status("busy");
        assert_eq!(rig.dog.decide(later, 0, &mut { t0 }), Decision::Keep(Reason::NotIdle));
        rig.status("idle");
        assert_eq!(rig.dog.decide(later, 0, &mut { later - MIN }), Decision::Keep(Reason::RecentOutput));
        assert_eq!(rig.dog.decide(later, 0, &mut { t0 }), Decision::Stop(Vec::new()));
        save(&rig.dir.join("config"), &Settings { idle_stop_minutes: 0, ..Settings::default() }).unwrap();
        assert_eq!(rig.dog.decide(later, 0, &mut { t0 }), Decision::Keep(Reason::IdleStopOff));
        save(&rig.dir.join("config"), &Settings::default()).unwrap();
        // A process table that can't say anything about claude.
        let mut gone = Command::new("true").spawn().unwrap();
        let dead = gone.id();
        gone.wait().unwrap();
        rig.dog.claude_pid = dead;
        assert_eq!(rig.dog.decide(later, 0, &mut { t0 }), Decision::Keep(Reason::NotIdle), "not registered either, so not known idle");
        fs::write(rig.dir.join(format!("claude/sessions/{dead}.json")), json!({"pid": dead, "status": "idle"}).to_string()).unwrap();
        assert_eq!(rig.dog.decide(later, 0, &mut { t0 }), Decision::Keep(Reason::ProcessesUnknown));
    }

    // --- ending processes ----------------------------------------------------------------------------

    /// A shell of the test's own that ends `secs` seconds after SIGTERM, or ignores it (`None`). Returned once it has
    /// set that up (a SIGTERM before then would just end it).
    fn shell(after_term: Option<f32>) -> Child {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let ready = std::env::temp_dir().join(format!(
            "platoon-host-ready-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let trap = after_term.map_or(String::new(), |secs| format!("sleep {secs}; exit 0"));
        let script = format!("trap '{trap}' TERM; : > '{}'; while :; do sleep 0.05; done", ready.display());
        let child = Command::new("sh").arg("-c").arg(script).spawn().unwrap();
        let start = Instant::now();
        while !ready.exists() {
            assert!(start.elapsed() < Duration::from_secs(10));
            thread::sleep(Duration::from_millis(5));
        }
        let _ = fs::remove_file(&ready);
        child
    }

    #[test]
    fn end_processes_waits_for_the_exit_and_gives_up_after_the_time_it_was_given() {
        let mut slow = shell(Some(0.4));
        let t = Instant::now();
        assert_eq!(end_processes_within(&[slow.id()], Duration::from_secs(10)), Ok(()));
        assert!(t.elapsed() >= Duration::from_millis(350), "it waited for the exit: {:?}", t.elapsed());
        slow.wait().unwrap();

        let mut deaf = shell(None);
        let t = Instant::now();
        assert_eq!(end_processes_within(&[deaf.id()], Duration::from_millis(250)), Err("it didn't exit in time".into()));
        let took = t.elapsed();
        assert!(took >= Duration::from_millis(250) && took < Duration::from_secs(3), "{took:?}");
        deaf.kill().unwrap(); // our own child, by handle
        deaf.wait().unwrap();
    }

    #[test]
    fn a_process_that_is_already_gone_counts_as_stopped_and_pid_zero_and_one_are_never_signalled() {
        let mut gone = Command::new("true").spawn().unwrap();
        let dead = gone.id();
        gone.wait().unwrap();
        assert_eq!(end_processes_within(&[dead], Duration::from_millis(200)), Ok(()));
        for bad in [0u32, 1] {
            assert_eq!(end_processes_within(&[bad], Duration::from_millis(200)), Err(format!("couldn't stop pid {bad}")));
        }
    }

    /// The user's live sessions keep their sockets in the real `<cache>/hosts`, and `discover` deletes the entries it
    /// takes for gone: no unit test may ever reach it. Every way to the real folders panics in a unit test, before
    /// anything is made, read or removed.
    #[test]
    fn a_unit_test_cannot_reach_the_real_folders() {
        let reached: Vec<(&str, bool)> = vec![
            ("cache_dir", std::panic::catch_unwind(cache_dir).is_ok()),
            ("config_dir", std::panic::catch_unwind(config_dir).is_ok()),
            ("claude_root", std::panic::catch_unwind(claude_root).is_ok()),
            ("hosts_dir", std::panic::catch_unwind(hosts_dir).is_ok()),
            ("discover", std::panic::catch_unwind(discover).is_ok()),
            ("Dirs::from_env", std::panic::catch_unwind(Dirs::from_env).is_ok()),
        ];
        assert_eq!(reached.iter().filter(|(_, ok)| *ok).collect::<Vec<_>>(), Vec::<&(&str, bool)>::new());
    }

    /// A window looks for its hosts in its own cache folder (a unit test's is temporary), owner-only, as before.
    #[test]
    fn a_window_finds_its_hosts_in_its_own_cache_folder() {
        let w = crate::testkit::World::new("own-hosts");
        let mut app = w.app();
        let me = std::process::id(); // alive, standing in for a claude
        let hosts = w.cache.join("hosts");
        fs::create_dir_all(&hosts).unwrap();
        fs::write(hosts.join(format!("{me}.json")), json!({"claude_pid": me, "host_pid": me, "dir": "/tmp"}).to_string()).unwrap();
        fs::write(hosts.join("4000000123.json"), json!({"claude_pid": 4_000_000_123u32, "host_pid": 1}).to_string()).unwrap();
        app.refresh();
        assert_eq!(app.hosts.iter().map(|h| h.claude_pid).collect::<Vec<_>>(), vec![me]);
        assert_eq!(app.hosts[0].sock, hosts.join(format!("{me}.sock")));
        assert!(!hosts.join("4000000123.json").exists(), "a gone host's entry is cleaned up, in the window's own folder");
        assert_eq!(fs::metadata(&hosts).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(app.host_dirs().cache, w.cache);
        assert_eq!(app.host_dirs().config, w.config);
    }

    /// A host started by a window uses the window's folders: told only when they differ from what the environment
    /// names, so a real user's sessions start with exactly the environment they always had.
    #[test]
    fn a_host_is_told_the_windows_folders_only_when_they_differ() {
        let real = Dirs { cache: "/home/u/.cache/platoon".into(), config: "/home/u/.config/platoon".into() };
        assert_eq!(real.child_env(&real), Vec::new());
        let test = Dirs { cache: "/tmp/w/cache".into(), config: "/tmp/w/config".into() };
        assert_eq!(
            test.child_env(&real),
            vec![("PLATOON_CACHE_DIR", PathBuf::from("/tmp/w/cache")), ("PLATOON_CONFIG_DIR", PathBuf::from("/tmp/w/config"))]
        );
        let own_cache = Dirs { cache: "/tmp/w/cache".into(), config: real.config.clone() };
        assert_eq!(own_cache.child_env(&real), vec![("PLATOON_CACHE_DIR", PathBuf::from("/tmp/w/cache"))]);
    }
}
