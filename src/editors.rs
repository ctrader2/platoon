//! The editors this computer already has, and running one on a file.
//!
//! Nothing is installed and nothing is assumed: platoon looks at `$VISUAL` and `$EDITOR` first, then for a list of
//! editors it knows by name, and offers only the ones it found. Editors that live in a terminal (nano, vim, ...) are
//! run inside platoon's own window, in a pseudo-terminal, exactly as a session is; editors with a window of their own
//! (VS Code, Mousepad, ...) are started beside platoon and left alone.
//!
//! Two knobs exist for tests, so a test never starts a real editor:
//!   `PLATOON_EDITOR_PATH`  where to look for editors, instead of `PATH`
//!   `PLATOON_EDITOR_BIN`   run this program instead of whichever editor was chosen
//! With neither set, `PLATOON_FORBID_REAL_EDITOR` makes starting an editor fail instead.

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::{
    io::{self, Read, Write},
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
    thread,
    time::{Duration, Instant},
};

/// The value of the setting that means "ask which editor every time".
pub const ASK: &str = "ask";

/// An editor platoon knows how to name and how to run.
struct Known {
    /// The command, which is also the editor's id in the settings file.
    command: &'static str,
    /// What a person calls it.
    name: &'static str,
    /// Arguments before the file. `--wait` and friends make a window app stay until the file is closed.
    args: &'static [&'static str],
    /// It draws in the terminal, so platoon runs it in its own window.
    terminal: bool,
    /// Only offered on macOS.
    macos: bool,
}

/// Editors that live in a terminal first, then ones with a window of their own; in each group, the order here is
/// the order the settings and the picker show.
const KNOWN: &[Known] = &[
    Known { command: "nvim", name: "Neovim", args: &[], terminal: true, macos: false },
    Known { command: "vim", name: "Vim", args: &[], terminal: true, macos: false },
    Known { command: "hx", name: "Helix", args: &[], terminal: true, macos: false },
    Known { command: "helix", name: "Helix", args: &[], terminal: true, macos: false },
    Known { command: "micro", name: "Micro", args: &[], terminal: true, macos: false },
    Known { command: "nano", name: "nano", args: &[], terminal: true, macos: false },
    // Plain `emacs` opens a window when it can; `-nw` keeps it in the terminal, which is what this list means.
    Known { command: "emacs", name: "Emacs", args: &["-nw"], terminal: true, macos: false },
    Known { command: "vi", name: "vi", args: &[], terminal: true, macos: false },
    Known { command: "code", name: "VS Code", args: &["--wait"], terminal: false, macos: false },
    Known { command: "cursor", name: "Cursor", args: &["--wait"], terminal: false, macos: false },
    Known { command: "zed", name: "Zed", args: &["--wait"], terminal: false, macos: false },
    Known { command: "subl", name: "Sublime Text", args: &["--wait"], terminal: false, macos: false },
    Known { command: "kate", name: "Kate", args: &["--block"], terminal: false, macos: false },
    Known { command: "gedit", name: "gedit", args: &["--wait"], terminal: false, macos: false },
    Known { command: "gnome-text-editor", name: "GNOME Text Editor", args: &[], terminal: false, macos: false },
    // Without this Mousepad hands the file to a copy that is already running and returns at once.
    Known { command: "mousepad", name: "Mousepad", args: &["--disable-server"], terminal: false, macos: false },
    Known { command: "open", name: "TextEdit", args: &["-t", "-W"], terminal: false, macos: true },
];

/// One editor platoon can offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Editor {
    /// What the settings file holds: the command's own name (`code`, `nano`).
    pub id: String,
    /// What a person calls it ("VS Code", "nano").
    pub name: String,
    /// The program to run, an absolute path where one was found.
    pub program: String,
    /// Arguments before the file.
    pub args: Vec<String>,
    /// It draws in the terminal, so it runs inside platoon's window.
    pub terminal: bool,
}

impl Editor {
    fn of(k: &Known, program: String) -> Self {
        Self { id: k.command.into(), name: k.name.into(), program, args: k.args.iter().map(|a| (*a).to_string()).collect(), terminal: k.terminal }
    }
}

fn basename(program: &str) -> &str {
    program.rsplit('/').next().unwrap_or(program)
}

/// Where to look for editors: `PLATOON_EDITOR_PATH` if it is set, else `PATH`.
fn search_path(env: &dyn Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let list = env("PLATOON_EDITOR_PATH").or_else(|| env("PATH")).unwrap_or_default();
    list.split(':').filter(|p| !p.is_empty()).map(PathBuf::from).collect()
}

/// The absolute path of `program` if it is there and can be run. A program given with a path of its own is used
/// as it is.
fn look_up(program: &str, path: &[PathBuf]) -> Option<String> {
    let runnable = |p: &Path| p.is_file() && p.metadata().map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false);
    if program.contains('/') {
        let p = Path::new(program);
        return runnable(p).then(|| program.to_string());
    }
    path.iter().map(|d| d.join(program)).find(|p| runnable(p)).map(|p| p.to_string_lossy().into_owned())
}

/// The editors this computer has, in the order they are offered: whatever `$VISUAL`/`$EDITOR` names first, then the
/// terminal editors platoon knows, then the ones with a window of their own. Nothing found at all: `nano` and `vi`,
/// which nearly every machine has, so there is always something to edit with.
pub fn detect() -> Vec<Editor> {
    detect_with(&|k| std::env::var(k).ok())
}

pub fn detect_with(env: &dyn Fn(&str) -> Option<String>) -> Vec<Editor> {
    let path = search_path(env);
    let mut out: Vec<Editor> = Vec::new();
    for key in ["VISUAL", "EDITOR"] {
        let Some(line) = env(key).filter(|v| !v.trim().is_empty()) else { continue };
        let mut words = line.split_whitespace().map(String::from);
        let Some(program) = words.next() else { continue };
        let args: Vec<String> = words.collect();
        let Some(found) = look_up(&program, &path) else { continue };
        let known = KNOWN.iter().find(|k| k.command == basename(&program) && (!k.macos || cfg!(target_os = "macos")));
        let editor = match known {
            // The name and, when the variable gave none, the arguments platoon knows for it.
            Some(k) => Editor {
                id: k.command.into(),
                name: k.name.into(),
                program: found,
                terminal: k.terminal,
                args: if args.is_empty() { k.args.iter().map(|a| (*a).to_string()).collect() } else { args },
            },
            // Something platoon doesn't know: it is what the person chose, and $EDITOR means a terminal editor.
            None => Editor { id: basename(&program).to_string(), name: basename(&program).to_string(), program: found, args, terminal: true },
        };
        if !out.iter().any(|e| e.id == editor.id) {
            out.push(editor);
        }
    }
    for k in KNOWN.iter().filter(|k| !k.macos || cfg!(target_os = "macos")) {
        if out.iter().any(|e| e.id == k.command) {
            continue;
        }
        if let Some(found) = look_up(k.command, &path) {
            out.push(Editor::of(k, found));
        }
    }
    if out.is_empty() {
        out = ["nano", "vi"]
            .iter()
            .filter_map(|c| KNOWN.iter().find(|k| k.command == *c))
            .map(|k| Editor::of(k, k.command.to_string()))
            .collect();
    }
    out
}

/// The editor an id names, if this computer has it.
pub fn by_id<'a>(list: &'a [Editor], id: &str) -> Option<&'a Editor> {
    list.iter().find(|e| e.id == id)
}

/// What the settings screen shows for the value `id`.
pub fn label(list: &[Editor], id: &str) -> String {
    match by_id(list, id) {
        Some(e) => e.name.clone(),
        None => "Ask each time".into(),
    }
}

/// The values the setting steps through: "Ask each time" first, then every editor found.
pub fn setting_ids(list: &[Editor]) -> Vec<String> {
    std::iter::once(ASK.to_string()).chain(list.iter().map(|e| e.id.clone())).collect()
}

/// The program and arguments to really run, with the test knobs applied.
fn command_of(e: &Editor, file: &Path, env: &dyn Fn(&str) -> Option<String>) -> io::Result<(String, Vec<String>)> {
    let stand_in = env("PLATOON_EDITOR_BIN").filter(|v| !v.is_empty());
    if stand_in.is_none() && env("PLATOON_EDITOR_PATH").is_none() && env("PLATOON_FORBID_REAL_EDITOR").is_some() {
        return Err(io::Error::other("PLATOON_FORBID_REAL_EDITOR is set: refusing to start a real editor"));
    }
    let program = stand_in.unwrap_or_else(|| e.program.clone());
    let mut args = e.args.clone();
    args.push(file.to_string_lossy().into_owned());
    Ok((program, args))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// How long a terminal editor gets to leave after it is asked to stop, before it is ended for good.
const STOP_WAIT: Duration = Duration::from_millis(1500);

/// A terminal editor running inside platoon's window: platoon owns its pseudo-terminal, draws its screen and
/// sends it what is typed, exactly as it does for a session.
pub struct Running {
    parser: Arc<Mutex<vt100::Parser>>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    ended: Arc<AtomicBool>,
    size: (u16, u16),
}

impl Running {
    /// Start `e` on `file` in a pseudo-terminal of `rows` x `cols`.
    pub fn spawn(e: &Editor, file: &Path, rows: u16, cols: u16) -> io::Result<Self> {
        Self::spawn_with(e, file, rows, cols, &|k| std::env::var(k).ok())
    }

    fn spawn_with(e: &Editor, file: &Path, rows: u16, cols: u16, env: &dyn Fn(&str) -> Option<String>) -> io::Result<Self> {
        let (program, args) = command_of(e, file, env)?;
        let pair = native_pty_system()
            .openpty(PtySize { rows: rows.max(1), cols: cols.max(1), pixel_width: 0, pixel_height: 0 })
            .map_err(io::Error::other)?;
        let mut cmd = CommandBuilder::new(&program);
        cmd.args(&args);
        if let Some(dir) = file.parent() {
            cmd.cwd(dir);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        // If platoon itself was started from inside Claude Code, don't leak that into the editor.
        cmd.env_remove("CLAUDECODE");
        cmd.env_remove("CLAUDE_CODE_ENTRYPOINT");
        let child = pair.slave.spawn_command(cmd).map_err(|e| io::Error::other(format!("couldn't start {program}: {e}")))?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(io::Error::other)?;
        let writer = pair.master.take_writer().map_err(io::Error::other)?;
        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows.max(1), cols.max(1), 0)));
        let ended = Arc::new(AtomicBool::new(false));
        let (p, done) = (parser.clone(), ended.clone());
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                lock(&p).process(&buf[..n]);
            }
            done.store(true, Ordering::Relaxed);
        });
        Ok(Self { parser, writer, master: pair.master, child, ended, size: (rows.max(1), cols.max(1)) })
    }

    pub fn parser(&self) -> &Arc<Mutex<vt100::Parser>> {
        &self.parser
    }

    pub fn write(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let (rows, cols) = (rows.max(1), cols.max(1));
        if self.size == (rows, cols) {
            return;
        }
        self.size = (rows, cols);
        let _ = self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
        lock(&self.parser).screen_mut().set_size(rows, cols);
    }

    /// True once the editor has exited (and its last output has been read).
    pub fn finished(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_))) && self.ended.load(Ordering::Relaxed)
    }

    /// Ask the editor to leave (it is platoon's own child), and end it for good if it won't.
    pub fn stop(&mut self) {
        if let Some(pid) = self.child.process_id() {
            if pid > 1 {
                unsafe { libc::kill(pid as i32, libc::SIGTERM) };
            }
        }
        let deadline = Instant::now() + STOP_WAIT;
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            self.stop();
        }
    }
}

/// Start an editor that has a window of its own, beside platoon, and leave it alone. The returned child is only
/// kept so platoon can notice when the editor is done with the file.
pub fn launch_window(e: &Editor, file: &Path) -> io::Result<Child> {
    launch_window_with(e, file, &|k| std::env::var(k).ok())
}

fn launch_window_with(e: &Editor, file: &Path, env: &dyn Fn(&str) -> Option<String>) -> io::Result<Child> {
    let (program, args) = command_of(e, file, env)?;
    let mut cmd = Command::new(&program);
    cmd.args(&args);
    if let Some(dir) = file.parent() {
        cmd.current_dir(dir);
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // Its own session: closing platoon's terminal can't hang it up.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn().map_err(|err| io::Error::other(format!("couldn't start {program}: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;

    /// A folder of programs that "exist", and an environment to go with it.
    struct Fake {
        dir: PathBuf,
        vars: HashMap<String, String>,
    }

    impl Fake {
        fn new(name: &str, programs: &[&str]) -> Self {
            let dir = std::env::temp_dir().join(format!("platoon-editors-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            for p in programs {
                let f = dir.join(p);
                fs::write(&f, "#!/bin/sh\n").unwrap();
                fs::set_permissions(&f, fs::Permissions::from_mode(0o755)).unwrap();
            }
            let mut vars = HashMap::new();
            vars.insert("PLATOON_EDITOR_PATH".to_string(), dir.to_string_lossy().into_owned());
            Self { dir, vars }
        }

        fn set(&mut self, key: &str, value: &str) -> &mut Self {
            self.vars.insert(key.into(), value.into());
            self
        }

        fn names(&self) -> Vec<String> {
            detect_with(&|k| self.vars.get(k).cloned()).into_iter().map(|e| e.name).collect()
        }

        fn list(&self) -> Vec<Editor> {
            detect_with(&|k| self.vars.get(k).cloned())
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn only_editors_that_are_there_are_offered_by_their_plain_names_terminal_ones_first() {
        let f = Fake::new("found", &["nano", "vi", "code", "mousepad", "notanedior"]);
        assert_eq!(f.names(), ["nano", "vi", "VS Code", "Mousepad"]);
        let list = f.list();
        assert!(list[0].terminal && list[1].terminal, "nano and vi live in the terminal");
        assert!(!list[2].terminal && !list[3].terminal, "VS Code and Mousepad have windows of their own");
        assert_eq!(list[2].args, ["--wait"], "a window app is asked to wait for the file to be closed");
        assert_eq!(list[3].args, ["--disable-server"]);
        assert!(list[0].program.ends_with("/nano"), "the one that was found, not just a name: {}", list[0].program);
        assert_eq!(list.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["nano", "vi", "code", "mousepad"]);
    }

    #[test]
    fn the_editor_variables_come_first_and_are_not_listed_twice() {
        let mut f = Fake::new("env", &["nano", "vim", "code"]);
        f.set("EDITOR", "nano");
        assert_eq!(f.names(), ["nano", "Vim", "VS Code"], "$EDITOR's own editor leads");
        f.set("VISUAL", "code");
        assert_eq!(f.names(), ["VS Code", "nano", "Vim"], "$VISUAL wins over $EDITOR");
        // A program that isn't there is no use.
        let mut g = Fake::new("env-missing", &["nano"]);
        g.set("EDITOR", "/usr/bin/nothing-here");
        assert_eq!(g.names(), ["nano"]);
        // Something platoon doesn't know is still offered, by its own name, as a terminal editor.
        let mut h = Fake::new("env-unknown", &["myed", "nano"]);
        h.set("EDITOR", "myed --quick");
        let list = h.list();
        assert_eq!((list[0].name.as_str(), list[0].terminal, list[0].args.clone()), ("myed", true, vec!["--quick".to_string()]));
        assert_eq!(h.names(), ["myed", "nano"]);
    }

    #[test]
    fn with_nothing_found_there_is_still_nano_and_vi_to_fall_back_on() {
        let f = Fake::new("none", &[]);
        assert_eq!(f.names(), ["nano", "vi"]);
        assert!(f.list().iter().all(|e| e.terminal));
        assert_eq!(f.list()[0].program, "nano", "by name: it is a last resort, not something we found");
    }

    #[test]
    fn the_setting_offers_ask_each_time_and_then_what_was_found() {
        let f = Fake::new("setting", &["nano", "code"]);
        let list = f.list();
        assert_eq!(setting_ids(&list), ["ask", "nano", "code"]);
        assert_eq!(label(&list, "ask"), "Ask each time");
        assert_eq!(label(&list, "code"), "VS Code");
        assert_eq!(label(&list, "one-i-uninstalled"), "Ask each time", "an editor that is no longer here");
        assert_eq!(by_id(&list, "nano").map(|e| e.name.as_str()), Some("nano"));
    }

    #[test]
    fn a_stand_in_program_replaces_the_editor_and_a_real_one_can_be_forbidden() {
        let f = Fake::new("knobs", &["nano"]);
        let e = f.list()[0].clone();
        let file = Path::new("/tmp/platoon-editors-test/project.md");
        let env = |vars: Vec<(&str, &str)>| {
            let map: HashMap<String, String> = vars.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            move |k: &str| map.get(k).cloned()
        };
        let plain = command_of(&e, file, &env(vec![])).unwrap();
        assert_eq!(plain.0, e.program);
        assert_eq!(plain.1, [file.to_string_lossy().into_owned()]);
        let swapped = command_of(&e, file, &env(vec![("PLATOON_EDITOR_BIN", "/bin/sh")])).unwrap();
        assert_eq!(swapped.0, "/bin/sh");
        assert!(command_of(&e, file, &env(vec![("PLATOON_FORBID_REAL_EDITOR", "1")])).is_err(), "no stand-in: refused");
        assert!(command_of(&e, file, &env(vec![("PLATOON_FORBID_REAL_EDITOR", "1"), ("PLATOON_EDITOR_BIN", "/bin/sh")])).is_ok());
        assert!(command_of(&e, file, &env(vec![("PLATOON_FORBID_REAL_EDITOR", "1"), ("PLATOON_EDITOR_PATH", "/x")])).is_ok());
    }

    #[test]
    fn a_terminal_editor_runs_in_platoons_own_window_and_ends_when_it_exits() {
        let dir = std::env::temp_dir().join(format!("platoon-editors-run-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("project.md");
        fs::write(&file, "before\n").unwrap();
        let script = dir.join("ed");
        fs::write(&script, "#!/bin/sh\necho READY\nread line\nprintf '%s\\n' \"$line\" >> \"$1\"\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let e = Editor { id: "ed".into(), name: "ed".into(), program: script.to_string_lossy().into_owned(), args: Vec::new(), terminal: true };
        let mut run = Running::spawn_with(&e, &file, 10, 40, &|_| None).unwrap();
        let start = Instant::now();
        while !run.parser().lock().unwrap().screen().contents().contains("READY") {
            assert!(start.elapsed() < Duration::from_secs(10), "the editor never drew anything");
            thread::sleep(Duration::from_millis(20));
        }
        run.write(b"typed inside platoon\r");
        while !run.finished() {
            assert!(start.elapsed() < Duration::from_secs(10), "the editor never exited");
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(fs::read_to_string(&file).unwrap(), "before\ntyped inside platoon\n");
        let _ = fs::remove_dir_all(&dir);
    }
}

