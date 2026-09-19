//! Session hosts: one small background process per session that owns claude's pseudo-terminal.
//!
//! Because the host is detached from platoon (own session, no controlling terminal), quitting
//! platoon, or closing its terminal, leaves claude running. platoon finds hosts again by scanning
//! `<cache>/hosts/`, and talks to them over a Unix socket using the framing below.
//!
//! Files per session, named by claude's pid, in a directory only the current user can enter:
//!   `<pid>.json`  who/where (removed when claude exits)
//!   `<pid>.sock`  the socket clients connect to

use crate::claude::pid_alive;
use crate::usage::cache_dir;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde_json::{json, Value};
use std::{
    fs,
    io::{self, Read, Write},
    os::unix::{
        fs::{DirBuilderExt, PermissionsExt},
        net::{UnixListener, UnixStream},
        process::CommandExt,
    },
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, MutexGuard},
    thread,
    time::Duration,
};

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

pub fn hosts_dir() -> io::Result<PathBuf> {
    let d = cache_dir().join("hosts");
    // Anyone who can open the socket can type into claude, so the directory is owner-only.
    fs::DirBuilder::new().recursive(true).mode(0o700).create(&d)?;
    fs::set_permissions(&d, fs::Permissions::from_mode(0o700))?;
    Ok(d)
}

#[derive(Clone, Debug)]
pub struct HostInfo {
    pub claude_pid: u32,
    pub host_pid: u32,
    pub dir: String,
    pub sock: PathBuf,
}

/// Live hosts. Entries whose claude has gone are cleaned up.
pub fn discover() -> Vec<HostInfo> {
    let Ok(dir) = hosts_dir() else { return Vec::new() };
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
        out.push(HostInfo { claude_pid: claude_pid as u32, host_pid: host_pid as u32, dir: v["dir"].as_str().unwrap_or("").to_string(), sock });
    }
    out
}

/// Start a detached host running `program args` in `dir` and wait until it is ready to connect to.
/// The returned `Child` should be polled with `try_wait` so it is reaped when the host exits.
pub fn launch(program: &str, dir: &str, args: &[&str], rows: u16, cols: u16) -> Result<(HostInfo, Child), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.args(["host", "--program", program, "--dir", dir, "--rows", &rows.to_string(), "--cols", &cols.to_string(), "--"])
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
    let hdir = hosts_dir().map_err(|e| e.to_string())?;
    let err_file = hdir.join(format!("{}.err", child.id()));
    for _ in 0..100 {
        if let Some(info) = discover().into_iter().find(|h| h.host_pid == child.id()) {
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
    let mut rest: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dir" => dir = it.next().cloned().unwrap_or(dir),
            "--program" => program = it.next().cloned().unwrap_or(program),
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
    // If platoon itself was started from inside Claude Code, don't leak that into the child.
    cmd.env_remove("CLAUDECODE");
    cmd.env_remove("CLAUDE_CODE_ENTRYPOINT");
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

    let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, SCROLLBACK)));
    let clients: Arc<Mutex<Vec<UnixStream>>> = Arc::new(Mutex::new(Vec::new()));
    let master: Arc<Mutex<Box<dyn MasterPty + Send>>> = Arc::new(Mutex::new(pair.master));
    let writer = Arc::new(Mutex::new(writer));

    {
        let (parser, clients, master, writer) = (parser.clone(), clients.clone(), master.clone(), writer.clone());
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (parser, clients, master, writer) = (parser.clone(), clients.clone(), master.clone(), writer.clone());
                thread::spawn(move || serve_client(stream, parser, clients, master, writer));
            }
        });
    }

    // Publish only once the socket is listening, so a discovered host is always connectable.
    let info = json!({ "claude_pid": claude_pid, "host_pid": me, "dir": dir });
    let tmp = hdir.join(format!(".{claude_pid}.tmp"));
    if fs::write(&tmp, info.to_string()).and_then(|_| fs::rename(&tmp, &info_path)).is_err() {
        return fail("couldn't write host info".into());
    }

    // Output loop: feed the emulator, then fan out to whoever is attached.
    let mut buf = [0u8; 8192];
    while let Ok(n) = reader.read(&mut buf) {
        if n == 0 {
            break;
        }
        let mut p = lock(&parser);
        p.process(&buf[..n]);
        lock(&clients).retain_mut(|c| write_frame(c, F_OUTPUT, &buf[..n]).is_ok());
    }
    let _ = child.wait();
    let _ = fs::remove_file(&info_path);
    let _ = fs::remove_file(&sock_path);
    0
}

fn apply_resize(payload: &[u8], parser: &Mutex<vt100::Parser>, master: &Mutex<Box<dyn MasterPty + Send>>) {
    if payload.len() != 4 {
        return;
    }
    let rows = u16::from_be_bytes([payload[0], payload[1]]).max(1);
    let cols = u16::from_be_bytes([payload[2], payload[3]]).max(1);
    let _ = lock(master).resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
    lock(parser).screen_mut().set_size(rows, cols);
}

fn serve_client(
    mut stream: UnixStream,
    parser: Arc<Mutex<vt100::Parser>>,
    clients: Arc<Mutex<Vec<UnixStream>>>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
) {
    // The first frame is the client's size, so the snapshot below is made at that size.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    match read_frame(&mut stream) {
        Ok((F_RESIZE, payload)) => apply_resize(&payload, &parser, &master),
        _ => return,
    }
    let _ = stream.set_read_timeout(None);
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    {
        // Snapshot and registration under the parser lock: no output can slip in between.
        let p = lock(&parser);
        let snapshot = p.screen().state_formatted();
        if write_frame(&mut stream, F_OUTPUT, &snapshot).is_err() {
            return;
        }
        match stream.try_clone() {
            Ok(c) => lock(&clients).push(c),
            Err(_) => return,
        }
    }
    while let Ok((kind, payload)) = read_frame(&mut stream) {
        match kind {
            F_INPUT => {
                let mut w = lock(&writer);
                let _ = w.write_all(&payload);
                let _ = w.flush();
            }
            F_RESIZE => apply_resize(&payload, &parser, &master),
            _ => {}
        }
    }
}
