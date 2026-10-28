//! `platoon open [id]`: show a session live in a regular terminal.
//!
//! The session runs under a platoon host (see host.rs); this is just a window onto it, like tmux
//! attach. Output bytes go straight to your terminal, so its own scrollback and features keep
//! working, and the session keeps running when you detach with the back key (Ctrl+Space unless `keys.json` says
//! otherwise; see keys.rs, read once as this starts).

use crate::agents::claude::{Reader, claude_root, find_transcript, saved_session_ids, transcript_cwd};
use crate::agents::{Agent, Start, codex};
use crate::guard;
use crate::host::{self, F_INPUT, F_OUTPUT, F_RESIZE, HostInfo, read_frame, size_payload, write_frame};
use crate::projects::{self, Projects};
use ratatui::crossterm::terminal;
use std::{
    io::{self, Read, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// Undo anything claude switched on in the terminal (mouse reporting, bracketed paste, alt screen,
/// kitty keyboard mode, hidden cursor) so the shell you return to behaves normally.
const RESET_TERMINAL: &[u8] = b"\x1b[<u\x1b[?2004l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1049l\x1b[?25h\x1b[0m";

/// Match `arg` against known session ids: exact, or a unique prefix of at least 4 characters.
fn resolve_id(arg: &str, mut known: Vec<String>) -> Result<String, String> {
    known.sort();
    known.dedup();
    if known.iter().any(|k| k == arg) {
        return Ok(arg.to_string());
    }
    if arg.len() < 4 {
        return Err(format!("\"{arg}\" is too short to identify a session; give at least 4 characters of its id"));
    }
    let matches: Vec<&String> = known.iter().filter(|k| k.starts_with(arg)).collect();
    match matches.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(format!("no session matches \"{arg}\"")),
        many => Err(format!(
            "\"{arg}\" matches {} sessions: {}",
            many.len(),
            many.iter().take(5).map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        )),
    }
}

fn ask(question: &str) -> bool {
    eprint!("{question} [y/N] ");
    let _ = io::stderr().flush();
    let mut line = String::new();
    io::stdin().read_line(&mut line).is_ok() && matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
}

/// The agent a new session started here runs: the one the user last chose in platoon (`n`), while it is installed,
/// else the one installed (Claude Code first). There is no question to ask in a plain terminal.
fn new_session_agent() -> Result<Agent, String> {
    let remembered = crate::settings::load(&crate::archive::config_dir()).0.new_session_agent;
    crate::agents::Providers::here(&crate::archive::config_dir())
        .new_session_agent(remembered.as_deref())
        .ok_or_else(|| "neither Claude Code nor Codex is installed on this computer".to_string())
}

/// Find, resume, or start the session's host. Returns where to connect.
fn host_for(arg: Option<&str>, rows: u16, cols: u16) -> Result<HostInfo, String> {
    let hosts = host::discover();
    let Some(arg) = arg else {
        let agent = new_session_agent()?;
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        let args = agent.args(Start::New(None));
        return host::launch(&host::Dirs::from_env(), agent, &agent.program(), &cwd.to_string_lossy(), &args, &[], rows, cols)
            .map(|(h, _)| h);
    };
    // A saved conversation is Claude Code's unless it is Codex's.
    let agent = Agent::Claude;
    let root = claude_root();

    let live = guard::live_entries(&root);
    let all = Reader::with_root(root.clone()).live();
    // Sessions Claude's own service runs in the background: opened with `claude attach`, never `--resume`.
    let background: Vec<&crate::agents::Live> = all.iter().filter(|l| l.is_background()).collect();
    let codex = codex::Reader::new();
    let codex_live = codex.live();
    let codex_ids: Vec<String> = codex::rollout_paths(codex.root())
        .iter()
        .filter_map(|p| codex::id_of_path(p))
        .chain(codex_live.iter().map(|l| l.id.clone()))
        .collect();
    let mut known: Vec<String> = saved_session_ids(&root);
    known.extend(live.iter().map(|e| e.id.clone()));
    known.extend(background.iter().map(|l| l.id.clone()));
    known.extend(codex_ids.iter().cloned());
    let id = resolve_id(arg, known)?;
    // A conversation of a program switched off in platoon's settings is left alone, as if that program weren't installed.
    let of = if codex_ids.contains(&id) { Agent::Codex } else { Agent::Claude };
    if crate::agents::switched_off(&crate::archive::config_dir(), of) {
        return Err(switched_off_refusal(&id, of));
    }
    if codex_ids.contains(&id) {
        return codex_host_for(&id, &codex, &codex_live, &hosts, rows, cols);
    }

    // Copies that write to the conversation; a parked one handed it over to a background session (as in the list).
    let copies: Vec<&guard::Entry> = live.iter().filter(|e| e.id == id && !e.parked).collect();
    let parked: Vec<&guard::Entry> = live.iter().filter(|e| e.id == id && e.parked).collect();
    let own = background.iter().find(|l| l.id == id).copied();
    // Nothing else runs a parked conversation: open the background session it moved to, while that runs.
    let moved_to = || {
        let job = all.iter().find(|l| l.id == id && l.is_parked()).and_then(|l| l.parked.as_deref())?;
        background.iter().find(|b| b.background.as_deref() == Some(job)).copied()
    };
    if let Some(bg) = own.or_else(|| if copies.is_empty() { moved_to() } else { None }) {
        let attach_id = bg.background.clone().unwrap_or_default();
        if let Some(h) = hosts.iter().find(|h| h.attaches_to(&attach_id)) {
            return Ok(h.clone()); // already shown under platoon: just join it
        }
        let cwd = Some(bg.cwd.clone()).filter(|c| Path::new(c).is_dir());
        let cwd =
            cwd.map(Ok).unwrap_or_else(|| std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).map_err(|e| e.to_string()))?;
        let args = agent.args(Start::Attach(&attach_id));
        return host::launch(&host::Dirs::from_env(), agent, &agent.program(), &cwd, &args, &[], rows, cols).map(|(h, _)| h);
    }

    // A parked process platoon hosts is joined like an idle one; one outside platoon is left alone (never ended).
    let joinable = if copies.is_empty() { &parked } else { &copies };
    if let Some(h) = hosts.iter().find(|h| joinable.iter().any(|e| e.pid == h.claude_pid)) {
        return Ok(h.clone()); // already running under platoon: just join it
    }
    let cwd =
        find_transcript(&root, &id).and_then(|p| transcript_cwd(&p)).ok_or_else(|| format!("couldn't find where session {id} was run"))?;
    if !Path::new(&cwd).is_dir() {
        return Err(format!("session {id} was run in {cwd}, which no longer exists"));
    }
    if !copies.is_empty() {
        // Running in a plain terminal platoon doesn't own: its screen can't be shared, so it has to be ended first.
        let pids: Vec<u32> = copies.iter().map(|e| e.pid).collect();
        eprintln!("Session {id} is running in another terminal outside platoon (pid {}).", pids[0]);
        if !ask("End it and continue here? Nothing is lost: the conversation is reopened from its saved history.") {
            return Err("left it running".into());
        }
        host::end_processes(&pids)?;
    }
    // A session of a project keeps carrying the project, so the notes hook can tell.
    let projects = Projects::open(&crate::archive::config_dir());
    let env: Vec<(&str, &str)> = projects.project_id_of(&id).map(|p| ("PLATOON_PROJECT", p)).into_iter().collect();
    let args = agent.args(Start::Resume(&id));
    host::launch(&host::Dirs::from_env(), agent, &agent.program(), &cwd, &args, &env, rows, cols).map(|(h, _)| h)
}

/// Why `platoon open` leaves conversation `id` of `agent` alone while `agent` is switched off.
fn switched_off_refusal(id: &str, agent: Agent) -> String {
    format!(
        "session {id} is a {0} conversation, and {0} is switched off in platoon's settings. Switch it back on there (Settings, General, \"Use {0}\") to open it.",
        agent.name()
    )
}

/// `host_for` a conversation of Codex's. Codex keeps one window per conversation itself: one platoon hosts is joined,
/// one open anywhere else (another terminal, Codex's background server) is left alone and said, never ended or opened
/// a second time. Otherwise it is resumed under a new host.
fn codex_host_for(
    id: &str,
    codex: &codex::Reader,
    live: &[crate::agents::Live],
    hosts: &[HostInfo],
    rows: u16,
    cols: u16,
) -> Result<HostInfo, String> {
    let agent = Agent::Codex;
    let ours = |pid: u32| hosts.iter().find(|h| h.claude_pid == pid || guard::ancestors(pid).contains(&h.claude_pid));
    if let Some(l) = live.iter().find(|l| l.id == id) {
        return match ours(l.pid) {
            Some(h) => Ok(h.clone()),
            None => Err(format!("session {id} is open in another Codex window (pid {}). Close it there to open it here.", l.pid)),
        };
    }
    // Started a moment ago by platoon, before Codex said it holds the conversation.
    if let Some(h) = hosts.iter().find(|h| h.session().as_deref() == Some(id)) {
        return Ok(h.clone());
    }
    let cwd = codex::find_rollout(codex.root(), id)
        .and_then(|p| codex::rollout_cwd(&p))
        .ok_or_else(|| format!("couldn't find where session {id} was run"))?;
    if !Path::new(&cwd).is_dir() {
        return Err(format!("session {id} was run in {cwd}, which no longer exists"));
    }
    let projects = Projects::open(&crate::archive::config_dir());
    let env: Vec<(&str, &str)> = projects.project_id_of(id).map(|p| ("PLATOON_PROJECT", p)).into_iter().collect();
    let args = agent.args(Start::Resume(id));
    host::launch(&host::Dirs::from_env(), agent, &agent.program(), &cwd, &args, &env, rows, cols).map(|(h, _)| h)
}

/// `platoon open --project <name>`: a new session in this folder, in that project.
fn host_in_project(name: &str, rows: u16, cols: u16) -> Result<HostInfo, String> {
    let projects = Projects::open(&crate::archive::config_dir());
    let p = projects.find_by_name(name)?;
    if p.archived {
        return Err(format!("the project \"{}\" is archived; restore it in platoon first", p.name));
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let at_ms = crate::usage::now().saturating_sub(2) * 1000;
    let agent = new_session_agent()?;
    let env = vec![("PLATOON_PROJECT", p.id.as_str())];
    let args = agent.args(Start::New(None));
    let (info, _) = host::launch(&host::Dirs::from_env(), agent, &agent.program(), &cwd.to_string_lossy(), &args, &env, rows, cols)?;
    // Placed in the project once its session id shows up (by any platoon window, or by the notes hook).
    projects::note_launch(&crate::usage::cache_dir(), info.claude_pid, &p.id, at_ms)
        .map_err(|e| format!("couldn't note the project: {e}"))?;
    Ok(info)
}

/// The id argument and the project name of `platoon open [id] [--project <name>]`.
fn parse_args(args: &[String]) -> Result<(Option<&str>, Option<&str>), String> {
    let (mut id, mut project) = (None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--project" | "-p" => project = Some(it.next().map(String::as_str).ok_or("--project needs a project's name")?),
            other if other.starts_with("--project=") => project = Some(&other["--project=".len()..]),
            other if id.is_none() && !other.starts_with('-') => id = Some(other),
            other => return Err(format!("don't know what \"{other}\" means here")),
        }
    }
    if id.is_some() && project.is_some() {
        return Err("give a session id or --project, not both (`m` in platoon moves a session to a project)".into());
    }
    Ok((id, project))
}

struct RawMode;

impl RawMode {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        Ok(RawMode)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

enum Ended {
    Detached,
    SessionEnded,
}

/// `back`: the byte forms of the back key (`keys::Chord::raw_forms`).
fn attach(sock: &Path, rows: u16, cols: u16, back: &[Vec<u8>]) -> io::Result<Ended> {
    let _raw = RawMode::enter()?;
    let mut out = io::stdout();
    let (stream, mut host_gone) = connect(sock, (rows, cols), b"", &mut out)?;
    // Swapped for the new connection's when the window connects again, so the resize watcher always tells the host.
    let writer = Arc::new(Mutex::new(stream));
    let stop = Arc::new(AtomicBool::new(false));
    {
        // Tell the host when this window is resized (it fits the smallest window attached).
        let (writer, stop) = (writer.clone(), stop.clone());
        thread::spawn(move || {
            let mut last = terminal::size().unwrap_or((cols, rows));
            while !stop.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(200));
                if let Ok(now) = terminal::size()
                    && now != last
                {
                    last = now;
                    let _ = write_frame(&mut *writer.lock().unwrap(), F_RESIZE, &size_payload(now.1, now.0));
                }
            }
        });
    }

    let mut stdin = io::stdin();
    let mut buf = [0u8; 4096];
    let mut reconnected: Option<Instant> = None;
    let ended = loop {
        if host_gone.load(Ordering::Relaxed) {
            // The host closes the connection of a window it can't feed in time (`host::cut_off`). While it still runs,
            // connect again, which redraws the screen as leaving and coming back would; once it has gone, the session
            // has ended (and a connection that closes again right after it was made is not chased in a loop).
            let again_at_once = reconnected.is_some_and(|t| t.elapsed() < crate::client::RECONNECT_GAP);
            let size = terminal::size().map(|(c, r)| (r, c)).unwrap_or((rows, cols));
            let fresh = if again_at_once { None } else { connect(sock, size, RESET_TERMINAL, &mut out).ok() };
            let Some((stream, gone)) = fresh else { break Ended::SessionEnded };
            let _ = std::mem::replace(&mut *writer.lock().unwrap(), stream).shutdown(Shutdown::Both);
            host_gone = gone;
            reconnected = Some(Instant::now());
        }
        // Wake regularly so we notice when the session ends while we're waiting for a key.
        let mut fd = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
        if unsafe { libc::poll(&mut fd, 1, 100) } <= 0 {
            continue;
        }
        let n = stdin.read(&mut buf)?;
        if n == 0 {
            break Ended::Detached;
        }
        // The back key (Ctrl+Space arrives as a NUL byte): everything before it is still sent, then we detach.
        let cut = crate::keys::find_back(&buf[..n], back).map(|(at, _)| at);
        let send = &buf[..cut.unwrap_or(n)];
        // A failed write means the connection closed: the reader sees that too, and the top of the loop deals with it.
        if !send.is_empty() {
            let _ = write_frame(&mut *writer.lock().unwrap(), F_INPUT, send);
        }
        if cut.is_some() {
            break Ended::Detached;
        }
    };
    stop.store(true, Ordering::Relaxed);
    let _ = writer.lock().unwrap().shutdown(Shutdown::Both);
    let _ = out.write_all(RESET_TERMINAL);
    let _ = out.flush();
    Ok(ended)
}

/// Connect this window to the host at `sock`: say its size (rows, cols), write `reset` and clear the screen for the
/// snapshot the host sends first, then show everything the host sends on a thread of its own. The flag returned is set
/// when that connection ends (the host closed it, or went).
fn connect(sock: &Path, (rows, cols): (u16, u16), reset: &[u8], out: &mut impl Write) -> io::Result<(UnixStream, Arc<AtomicBool>)> {
    let mut stream = UnixStream::connect(sock)?;
    write_frame(&mut stream, F_RESIZE, &size_payload(rows, cols))?;
    out.write_all(reset)?;
    out.write_all(b"\x1b[2J\x1b[H")?;
    out.flush()?;
    let gone = Arc::new(AtomicBool::new(false));
    let (said, mut reader) = (gone.clone(), stream.try_clone()?);
    // It ends with its connection, after the last of what it was sent, so nothing it writes can land on the screen of
    // a connection made after it.
    thread::spawn(move || {
        let mut out = io::stdout();
        while let Ok((kind, payload)) = read_frame(&mut reader) {
            if kind == F_OUTPUT {
                let _ = out.write_all(&payload);
                let _ = out.flush();
            }
        }
        said.store(true, Ordering::Relaxed);
    });
    Ok((stream, gone))
}

pub fn run(args: &[String]) -> i32 {
    let (id, project) = match parse_args(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("platoon open: {e}");
            return 2;
        }
    };
    // The back key, from keys.json (a problem with it is said here, before the session takes the screen).
    let (keys, problems) = crate::keys::Keys::load(&crate::archive::config_dir());
    for p in &problems {
        eprintln!("platoon: {p}");
    }
    let back = keys.back.raw_forms();
    let Ok((cols, rows)) = terminal::size() else {
        eprintln!("platoon open needs an interactive terminal.");
        return 1;
    };
    let found = match project {
        Some(name) => host_in_project(name, rows, cols),
        None => host_for(id, rows, cols),
    };
    let info = match found {
        Ok(h) => h,
        Err(e) => {
            eprintln!("platoon open: {e}");
            return 1;
        }
    };
    let ended = attach(&info.sock, rows, cols, &back);
    // Said again once the session has given the screen back (it cleared the screen as it started).
    for p in &problems {
        eprintln!("\r\nplatoon: {p}");
    }
    match ended {
        Ok(Ended::Detached) => {
            eprintln!("\r\nplatoon: detached. The session is still running; run `platoon` to see it.");
            0
        }
        Ok(Ended::SessionEnded) => {
            eprintln!("\r\nplatoon: the session ended.");
            0
        }
        Err(e) => {
            eprintln!("platoon open: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> Vec<String> {
        vec!["abcd1111-aaaa".into(), "abcd2222-bbbb".into(), "ffff0000-cccc".into()]
    }

    #[test]
    fn ids_resolve_by_exact_match_or_unique_prefix() {
        assert_eq!(resolve_id("ffff", ids()).unwrap(), "ffff0000-cccc");
        assert_eq!(resolve_id("abcd1", ids()).unwrap(), "abcd1111-aaaa");
        assert_eq!(resolve_id("abcd2222-bbbb", ids()).unwrap(), "abcd2222-bbbb");
    }

    #[test]
    fn the_arguments_are_an_id_or_a_project_never_both() {
        let a = |v: &[&str]| {
            parse_args(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>()).map(|(i, p)| (i.map(String::from), p.map(String::from)))
        };
        assert_eq!(a(&[]).unwrap(), (None, None));
        assert_eq!(a(&["abcd"]).unwrap(), (Some("abcd".into()), None));
        assert_eq!(a(&["--project", "Web shop"]).unwrap(), (None, Some("Web shop".into())));
        assert_eq!(a(&["--project=shop"]).unwrap(), (None, Some("shop".into())));
        assert!(a(&["--project"]).is_err());
        assert!(a(&["abcd", "--project", "x"]).is_err());
        assert!(a(&["--nope"]).is_err());
    }

    #[test]
    fn ambiguous_short_and_unknown_ids_are_rejected_with_a_reason() {
        assert!(resolve_id("abcd", ids()).unwrap_err().contains("matches 2 sessions"));
        assert!(resolve_id("ab", ids()).unwrap_err().contains("too short"));
        assert!(resolve_id("zzzz", ids()).unwrap_err().contains("no session matches"));
    }
}
