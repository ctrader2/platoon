//! `platoon open [id]`: show a session live in a regular terminal.
//!
//! The session runs under a platoon host (see host.rs); this is just a window onto it, like tmux
//! attach. Output bytes go straight to your terminal, so its own scrollback and features keep
//! working, and the session keeps running when you detach (Ctrl+Space).

use crate::claude::{self, claude_root, find_transcript, saved_session_ids, transcript_cwd, Reader};
use crate::guard;
use crate::projects::{self, Projects};
use crate::host::{self, read_frame, size_payload, write_frame, HostInfo, F_INPUT, F_OUTPUT, F_RESIZE};
use ratatui::crossterm::terminal;
use std::{
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

/// Undo anything claude switched on in the terminal (mouse reporting, bracketed paste, alt screen,
/// kitty keyboard mode, hidden cursor) so the shell you return to behaves normally.
const RESET_TERMINAL: &[u8] = b"\x1b[<u\x1b[?2004l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1049l\x1b[?25h\x1b[0m";

fn program() -> String {
    std::env::var("PLATOON_CLAUDE_BIN").unwrap_or_else(|_| "claude".into())
}

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
        many => Err(format!("\"{arg}\" matches {} sessions: {}", many.len(), many.iter().take(5).map(|s| s.as_str()).collect::<Vec<_>>().join(", "))),
    }
}

fn ask(question: &str) -> bool {
    eprint!("{question} [y/N] ");
    let _ = io::stderr().flush();
    let mut line = String::new();
    io::stdin().read_line(&mut line).is_ok() && matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Find, resume, or start the session's host. Returns where to connect.
fn host_for(arg: Option<&str>, rows: u16, cols: u16) -> Result<HostInfo, String> {
    let root = claude_root();
    let hosts = host::discover();
    let Some(arg) = arg else {
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        return host::launch(&program(), &cwd.to_string_lossy(), &[], &[], rows, cols).map(|(h, _)| h);
    };

    let live = guard::live_entries(&root);
    let all = Reader::with_root(root.clone()).live();
    // Sessions Claude's own service runs in the background: opened with `claude attach`, never `--resume`.
    let background: Vec<&claude::Live> = all.iter().filter(|l| l.is_background()).collect();
    let mut known: Vec<String> = saved_session_ids(&root);
    known.extend(live.iter().map(|e| e.id.clone()));
    known.extend(background.iter().map(|l| l.id.clone()));
    let id = resolve_id(arg, known)?;

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
        let cwd = cwd.map(Ok).unwrap_or_else(|| std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).map_err(|e| e.to_string()))?;
        let args = claude::attach_args(&attach_id);
        return host::launch(&program(), &cwd, &[args[0].as_str(), args[1].as_str()], &[], rows, cols).map(|(h, _)| h);
    }

    // A parked process platoon hosts is joined like an idle one; one outside platoon is left alone (never ended).
    let joinable = if copies.is_empty() { &parked } else { &copies };
    if let Some(h) = hosts.iter().find(|h| joinable.iter().any(|e| e.pid == h.claude_pid)) {
        return Ok(h.clone()); // already running under platoon: just join it
    }
    let cwd = find_transcript(&root, &id)
        .and_then(|p| transcript_cwd(&p))
        .ok_or_else(|| format!("couldn't find where session {id} was run"))?;
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
    host::launch(&program(), &cwd, &["--resume", &id], &env, rows, cols).map(|(h, _)| h)
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
    let (info, _) = host::launch(&program(), &cwd.to_string_lossy(), &[], &[("PLATOON_PROJECT", p.id.as_str())], rows, cols)?;
    // Placed in the project once its session id shows up (by any platoon window, or by the notes hook).
    projects::note_launch(&crate::usage::cache_dir(), info.claude_pid, &p.id, at_ms).map_err(|e| format!("couldn't note the project: {e}"))?;
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

fn attach(sock: &Path, rows: u16, cols: u16) -> io::Result<Ended> {
    let stream = UnixStream::connect(sock)?;
    let writer = Arc::new(Mutex::new(stream.try_clone()?));
    write_frame(&mut *writer.lock().unwrap(), F_RESIZE, &size_payload(rows, cols))?;

    let _raw = RawMode::enter()?;
    let mut out = io::stdout();
    out.write_all(b"\x1b[2J\x1b[H")?;
    out.flush()?;

    let host_gone = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    {
        let (gone, mut reader) = (host_gone.clone(), stream.try_clone()?);
        thread::spawn(move || {
            let mut out = io::stdout();
            while let Ok((kind, payload)) = read_frame(&mut reader) {
                if kind == F_OUTPUT {
                    let _ = out.write_all(&payload);
                    let _ = out.flush();
                }
            }
            gone.store(true, Ordering::Relaxed);
        });
    }
    {
        // Tell the host when this window is resized (it fits the smallest window attached).
        let (writer, stop) = (writer.clone(), stop.clone());
        thread::spawn(move || {
            let mut last = terminal::size().unwrap_or((cols, rows));
            while !stop.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(200));
                if let Ok(now) = terminal::size() {
                    if now != last {
                        last = now;
                        let _ = write_frame(&mut *writer.lock().unwrap(), F_RESIZE, &size_payload(now.1, now.0));
                    }
                }
            }
        });
    }

    let mut stdin = io::stdin();
    let mut buf = [0u8; 4096];
    let ended = loop {
        if host_gone.load(Ordering::Relaxed) {
            break Ended::SessionEnded;
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
        // Ctrl+Space arrives as a NUL byte: everything before it is still sent, then we detach.
        let cut = buf[..n].iter().position(|&b| b == 0);
        let send = &buf[..cut.unwrap_or(n)];
        if !send.is_empty() && write_frame(&mut *writer.lock().unwrap(), F_INPUT, send).is_err() {
            break Ended::SessionEnded;
        }
        if cut.is_some() {
            break Ended::Detached;
        }
    };
    stop.store(true, Ordering::Relaxed);
    let _ = stream.shutdown(std::net::Shutdown::Both);
    let _ = out.write_all(RESET_TERMINAL);
    let _ = out.flush();
    Ok(ended)
}

pub fn run(args: &[String]) -> i32 {
    let (id, project) = match parse_args(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("platoon open: {e}");
            return 2;
        }
    };
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
    match attach(&info.sock, rows, cols) {
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
        let a = |v: &[&str]| parse_args(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>()).map(|(i, p)| (i.map(String::from), p.map(String::from)));
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
