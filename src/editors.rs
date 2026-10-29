//! The editors this computer already has, and running one on a file.
//!
//! Nothing is installed and nothing is assumed: platoon looks at `$VISUAL` and `$EDITOR` first, then for a list of
//! editors it knows by name, and offers only the ones it found. Editors that live in a terminal (nano, vim, ...) are
//! run inside platoon's own window, in a pseudo-terminal, exactly as a session is; editors with a window of their own
//! (VS Code, Mousepad, the JetBrains IDEs, ...) are started beside platoon and asked to wait until the file is closed.
//!
//! Editors are looked for where `v`'s are (`fileeditors::places`): on `PATH`, then where editors install themselves
//! outside it (JetBrains Toolbox's scripts, `~/.local/bin`, snaps, `/opt/<app>/bin`); and the windowed ones `v` knows
//! too, also as macOS app bundles, by the program inside the bundle (`fileeditors::file_program`).
//!
//! Two knobs exist for tests, so a test never starts a real editor:
//!   `PLATOON_EDITOR_PATH`  the only place to look for editors, instead of `PATH`, the install places and app bundles
//!   `PLATOON_EDITOR_BIN`   run this program instead of whichever editor was chosen
//! With neither set, `PLATOON_FORBID_REAL_EDITOR` makes starting an editor fail instead.

use crate::fileeditors::{self, Places};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::{
    io::{self, Read, Write},
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// What `notes_editor` holds while no editor has been used yet, so nothing is starred and the list is in the
/// order editors were found. (It is the old "ask each time" value of the setting this replaced, so a settings
/// file written by an earlier platoon still reads.)
pub const ASK: &str = "ask";

/// An editor platoon knows how to name and how to run.
#[derive(Clone, Copy)]
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
    /// `v` knows it too, by this id (`fileeditors`): found where `v` finds it, its macOS app bundle included.
    shared: bool,
}

const fn terminal(command: &'static str, name: &'static str, args: &'static [&'static str]) -> Known {
    Known { command, name, args, terminal: true, macos: false, shared: false }
}

const fn window(command: &'static str, name: &'static str, args: &'static [&'static str], shared: bool) -> Known {
    Known { command, name, args, terminal: false, macos: false, shared }
}

/// Editors that live in a terminal first, then ones with a window of their own; in each group, the order here is
/// the order the settings and the picker show. The JetBrains IDEs and TextEdit come after these (`known`).
const KNOWN: &[Known] = &[
    terminal("nvim", "Neovim", &[]),
    terminal("vim", "Vim", &[]),
    terminal("hx", "Helix", &[]),
    terminal("helix", "Helix", &[]),
    terminal("micro", "Micro", &[]),
    terminal("nano", "nano", &[]),
    // Plain `emacs` opens a window when it can; `-nw` keeps it in the terminal, which is what this list means.
    terminal("emacs", "Emacs", &["-nw"]),
    terminal("vi", "vi", &[]),
    window("code", "VS Code", &["--wait"], true),
    window("cursor", "Cursor", &["--wait"], true),
    window("zed", "Zed", &["--wait"], true),
    window("subl", "Sublime Text", &["--wait"], true),
    window("kate", "Kate", &["--block"], true),
    window("gedit", "gedit", &["--wait"], false),
    window("gnome-text-editor", "GNOME Text Editor", &[], false),
    // Without this Mousepad hands the file to a copy that is already running and returns at once.
    window("mousepad", "Mousepad", &["--disable-server"], false),
];

/// `open -t -W`: the file in TextEdit, and `open` waits until TextEdit has quit.
const TEXTEDIT: Known = Known { command: "open", name: "TextEdit", args: &["-t", "-W"], terminal: false, macos: true, shared: false };

/// Every editor this list knows, in the order it offers them: `KNOWN`, then the JetBrains IDEs `v` knows, by `v`'s ids
/// and names (`<launcher> --wait <file>` returns once the file is closed), then TextEdit.
fn known() -> Vec<Known> {
    let jetbrains = fileeditors::jetbrains_ides().map(|(id, name)| window(id, name, &["--wait"], true));
    KNOWN.iter().copied().chain(jetbrains).chain([TEXTEDIT]).collect()
}

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
        Self {
            id: k.command.into(),
            name: k.name.into(),
            program,
            args: k.args.iter().map(|a| (*a).to_string()).collect(),
            terminal: k.terminal,
        }
    }
}

fn basename(program: &str) -> &str {
    program.rsplit('/').next().unwrap_or(program)
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

/// Where one of the editors this list knows is installed here.
fn find(k: &Known, places: &Places) -> Option<String> {
    if k.shared { fileeditors::file_program(k.command, places) } else { places.find(&[k.command]) }
}

/// The editors this computer has (`detect_in`).
#[cfg(not(test))]
pub fn detect() -> Vec<Editor> {
    let env = |k: &str| std::env::var(k).ok();
    let macos = cfg!(target_os = "macos");
    detect_in(&env, &fileeditors::places(&env, Path::new("/"), macos), macos)
}

/// A unit test never looks at this computer's editors: it gets the last resort, `nano` and `vi` by name.
#[cfg(test)]
pub fn detect() -> Vec<Editor> {
    detect_in(&|_| None, &Places::default(), false)
}

/// The editors found in `places`, in the order they are offered: whatever `$VISUAL`/`$EDITOR` names first (looked for
/// on `PATH` only), then the terminal editors platoon knows, then the ones with a window of their own, the JetBrains
/// IDEs last among those. Nothing found at all: `nano` and `vi`, which nearly every machine has, so there is always
/// something to edit with. `macos` says whether TextEdit is offered.
pub fn detect_in(env: &dyn Fn(&str) -> Option<String>, places: &Places, macos: bool) -> Vec<Editor> {
    let known: Vec<Known> = known().into_iter().filter(|k| !k.macos || macos).collect();
    let path = &places.path;
    let mut out: Vec<Editor> = Vec::new();
    for key in ["VISUAL", "EDITOR"] {
        let Some(line) = env(key).filter(|v| !v.trim().is_empty()) else { continue };
        let mut words = line.split_whitespace().map(String::from);
        let Some(program) = words.next() else { continue };
        let args: Vec<String> = words.collect();
        let Some(found) = look_up(&program, path) else { continue };
        let editor = match known.iter().find(|k| k.command == basename(&program)) {
            // The name and, when the variable gave none, the arguments platoon knows for it.
            Some(k) => Editor {
                id: k.command.into(),
                name: k.name.into(),
                program: found,
                terminal: k.terminal,
                args: if args.is_empty() { k.args.iter().map(|a| (*a).to_string()).collect() } else { args },
            },
            // Something platoon doesn't know: it is what the person chose, and $EDITOR means a terminal editor.
            None => {
                Editor { id: basename(&program).to_string(), name: basename(&program).to_string(), program: found, args, terminal: true }
            }
        };
        if !out.iter().any(|e| e.id == editor.id) {
            out.push(editor);
        }
    }
    for k in &known {
        if out.iter().any(|e| e.id == k.command) {
            continue;
        }
        if let Some(found) = find(k, places) {
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

/// The editors as the list offers them: the one used last (`last`) first, then the rest in the order they were
/// found. With no editor used yet - or one this computer no longer has - the order is simply the order found.
pub fn in_order(list: &[Editor], last: &str) -> Vec<Editor> {
    let mut out: Vec<Editor> = by_id(list, last).cloned().into_iter().collect();
    out.extend(list.iter().filter(|e| e.id != last).cloned());
    out
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
        let parser = Arc::new(Mutex::new(crate::host::emulator(rows, cols, 0)));
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
        let (r, c) = crate::host::vt_size(rows, cols);
        lock(&self.parser).screen_mut().set_size(r, c);
    }

    /// True once the editor has exited (and its last output has been read).
    pub fn finished(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_))) && self.ended.load(Ordering::Relaxed)
    }

    /// Ask the editor to leave (it is platoon's own child), and end it for good if it won't.
    pub fn stop(&mut self) {
        if let Some(pid) = self.child.process_id()
            && pid > 1
        {
            unsafe { libc::kill(pid as i32, libc::SIGTERM) };
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
            let dir = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-editors-{name}-{}", std::process::id())));
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
            self.list().into_iter().map(|e| e.name).collect()
        }

        fn list(&self) -> Vec<Editor> {
            let env = |k: &str| self.vars.get(k).cloned();
            detect_in(&env, &fileeditors::places(&env, &self.dir, false), false)
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
    fn the_list_puts_the_editor_used_last_first_and_otherwise_keeps_the_order_found() {
        let f = Fake::new("order", &["nano", "code"]);
        let list = f.list();
        let names = |last: &str| in_order(&list, last).into_iter().map(|e| e.name).collect::<Vec<_>>();
        assert_eq!(names(ASK), ["nano", "VS Code"], "nothing used yet: the order they were found in");
        assert_eq!(names("code"), ["VS Code", "nano"], "the one used last comes first");
        assert_eq!(names("nano"), ["nano", "VS Code"]);
        assert_eq!(names("one-i-uninstalled"), ["nano", "VS Code"], "an editor that is no longer here changes nothing");
        assert_eq!(in_order(&list, "code").len(), list.len(), "every editor is offered exactly once");
        assert_eq!(by_id(&list, "nano").map(|e| e.name.as_str()), Some("nano"));
    }

    /// A pretend computer with a home at `home/u` and `usr/bin` as its `PATH`: editors are placed where they install
    /// themselves, and looked for with Linux's places or macOS's.
    struct World {
        root: PathBuf,
        vars: HashMap<String, String>,
    }

    impl World {
        fn new(name: &str) -> Self {
            let root =
                crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-editors-w-{name}-{}", std::process::id())));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("home/u")).unwrap();
            fs::create_dir_all(root.join("usr/bin")).unwrap();
            let mut vars = HashMap::new();
            vars.insert("HOME".to_string(), s(&root.join("home/u")));
            vars.insert("PATH".to_string(), s(&root.join("usr/bin")));
            Self { root, vars }
        }

        /// A runnable program at `rel` (from the root).
        fn program(&self, rel: &str) -> &Self {
            let p = self.root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, "#!/bin/sh\n").unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
            self
        }

        fn dir(&self, rel: &str) -> &Self {
            fs::create_dir_all(self.root.join(rel)).unwrap();
            self
        }

        fn list(&self, macos: bool) -> Vec<Editor> {
            let env = |k: &str| self.vars.get(k).cloned();
            detect_in(&env, &fileeditors::places(&env, &self.root, macos), macos)
        }

        fn names(&self, macos: bool) -> Vec<String> {
            self.list(macos).into_iter().map(|e| e.name).collect()
        }

        fn at(&self, rel: &str) -> String {
            s(&self.root.join(rel))
        }
    }

    impl Drop for World {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn macos_app_bundles_are_offered_by_the_program_inside_them_and_asked_to_wait() {
        let w = World::new("mac");
        w.program("Applications/Visual Studio Code.app/Contents/Resources/app/bin/code");
        w.program("Applications/IntelliJ IDEA.app/Contents/MacOS/idea");
        w.program("home/u/Applications/PyCharm CE.app/Contents/MacOS/pycharm");
        w.dir("Applications/GoLand.app"); // no launcher inside: it couldn't be asked to wait
        w.dir("Applications/Zed.app"); // no tool inside
        let list = w.list(true);
        let code = by_id(&list, "code").expect("VS Code from its bundle");
        assert_eq!(
            (code.name.as_str(), code.program.clone(), code.args.clone(), code.terminal),
            ("VS Code", w.at("Applications/Visual Studio Code.app/Contents/Resources/app/bin/code"), vec!["--wait".to_string()], false)
        );
        let idea = by_id(&list, "idea").expect("IntelliJ IDEA from its bundle");
        assert_eq!(
            (idea.name.as_str(), idea.program.clone(), idea.args.clone(), idea.terminal),
            ("IntelliJ IDEA", w.at("Applications/IntelliJ IDEA.app/Contents/MacOS/idea"), vec!["--wait".to_string()], false),
            "the launcher itself, not `open -na`, which returns at once"
        );
        let pycharm = by_id(&list, "pycharm-community").expect("~/Applications too");
        assert_eq!(pycharm.program, w.at("home/u/Applications/PyCharm CE.app/Contents/MacOS/pycharm"));
        assert!(by_id(&list, "goland").is_none() && by_id(&list, "zed").is_none(), "a bundle without its program is not offered");
        assert_eq!(w.names(true), ["VS Code", "IntelliJ IDEA", "PyCharm Community"]);
        // Linux doesn't look in app bundles.
        assert_eq!(w.names(false), ["nano", "vi"]);
    }

    #[test]
    fn cursor_from_a_bundle_with_only_bin_code_is_offered_and_asked_to_wait() {
        let w = World::new("mac-cursor");
        w.program("Applications/Cursor.app/Contents/Resources/app/bin/code");
        let list = w.list(true);
        let cursor = by_id(&list, "cursor").expect("Cursor from its bundle");
        assert_eq!(
            (cursor.program.clone(), cursor.args.clone(), cursor.terminal),
            (w.at("Applications/Cursor.app/Contents/Resources/app/bin/code"), vec!["--wait".to_string()], false)
        );
    }

    #[test]
    fn jetbrains_ides_are_found_in_toolbox_scripts_opt_snaps_and_local_bin_on_linux() {
        let w = World::new("jb-linux");
        w.program("home/u/.local/share/JetBrains/Toolbox/scripts/pycharm");
        w.program("snap/bin/intellij-idea-community");
        w.program("opt/idea-IU-253.1/bin/idea.sh");
        w.program("opt/android-studio/bin/studio.sh");
        w.program("home/u/.local/bin/rustrover");
        let list = w.list(false);
        let found: Vec<(&str, &str, &str)> = list.iter().map(|e| (e.id.as_str(), e.name.as_str(), e.program.as_str())).collect();
        let (idea, community, pycharm, rust, studio) = (
            w.at("opt/idea-IU-253.1/bin/idea.sh"),
            w.at("snap/bin/intellij-idea-community"),
            w.at("home/u/.local/share/JetBrains/Toolbox/scripts/pycharm"),
            w.at("home/u/.local/bin/rustrover"),
            w.at("opt/android-studio/bin/studio.sh"),
        );
        assert_eq!(
            found,
            vec![
                ("idea", "IntelliJ IDEA", idea.as_str()),
                ("idea-community", "IntelliJ IDEA Community", community.as_str()),
                ("pycharm", "PyCharm", pycharm.as_str()),
                ("rustrover", "RustRover", rust.as_str()),
                ("android-studio", "Android Studio", studio.as_str()),
            ]
        );
        assert!(list.iter().all(|e| !e.terminal && e.args == ["--wait"]), "each opened with --wait: {list:?}");
        // The other windowed editors are found off the PATH too.
        w.program("snap/bin/code");
        assert_eq!(by_id(&w.list(false), "code").map(|e| e.program.clone()), Some(w.at("snap/bin/code")));
    }

    #[test]
    fn a_test_path_means_no_install_places_and_no_app_bundles() {
        let mut w = World::new("only");
        w.program("snap/bin/code");
        w.program("home/u/.local/share/JetBrains/Toolbox/scripts/idea");
        w.program("opt/goland/bin/goland.sh");
        w.program("Applications/IntelliJ IDEA.app/Contents/MacOS/idea");
        w.program("fake/mousepad");
        let only = w.at("fake");
        w.vars.insert("PLATOON_EDITOR_PATH".into(), only);
        assert_eq!(w.names(false), ["Mousepad"]);
        assert_eq!(w.names(true), ["Mousepad"]);
    }

    #[test]
    fn terminal_editors_first_then_windows_with_jetbrains_after_the_others_and_textedit_last() {
        let w = World::new("order");
        for p in ["usr/bin/open", "usr/bin/rider", "usr/bin/idea", "usr/bin/mousepad", "usr/bin/code", "usr/bin/vi", "usr/bin/nano"] {
            w.program(p);
        }
        assert_eq!(w.names(true), ["nano", "vi", "VS Code", "Mousepad", "IntelliJ IDEA", "Rider", "TextEdit"]);
        assert_eq!(w.names(false), ["nano", "vi", "VS Code", "Mousepad", "IntelliJ IDEA", "Rider"], "TextEdit on macOS only");
    }

    #[test]
    fn the_editor_variables_can_name_a_jetbrains_ide() {
        let mut w = World::new("visual-idea");
        w.program("usr/bin/idea");
        w.program("usr/bin/nano");
        w.vars.insert("VISUAL".into(), "idea".into());
        let list = w.list(false);
        assert_eq!(
            (list[0].id.as_str(), list[0].name.as_str(), list[0].terminal, list[0].args.clone()),
            ("idea", "IntelliJ IDEA", false, vec!["--wait".to_string()])
        );
        assert_eq!(w.names(false), ["IntelliJ IDEA", "nano"], "not listed twice");
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
        let dir = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-editors-run-{}", std::process::id())));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("project.md");
        fs::write(&file, "before\n").unwrap();
        let script = dir.join("ed");
        fs::write(&script, "#!/bin/sh\necho READY\nread line\nprintf '%s\\n' \"$line\" >> \"$1\"\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let e =
            Editor { id: "ed".into(), name: "ed".into(), program: script.to_string_lossy().into_owned(), args: Vec::new(), terminal: true };
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
