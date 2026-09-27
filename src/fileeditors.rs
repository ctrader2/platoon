//! The editors `v` can open a ticket's or a session's folders in: platoon's own editor (`editor.rs`, always offered,
//! installed on first use), then the editors **this computer has** that open a whole folder with a file tree. Only
//! what is really there is offered: a program found on `PATH` or in the places those editors install themselves
//! (JetBrains Toolbox's scripts, snaps, `/opt/<app>/bin`, `~/.local/bin`; on macOS the `.app` bundles), never a name
//! alone. Editors without a file tree (vi, vim, nano, micro, gedit, Mousepad) are not offered: they are the notes
//! editors of `editors.rs`.
//!
//! How each opens several folders (a ticket with worktrees in two repositories, a session with added folders):
//! - VS Code, Cursor, Zed, Sublime Text, Kate: every folder in one new window (`code --new-window a b` makes one
//!   window with both as its roots);
//! - the JetBrains IDEs: each folder is a project, in a window of its own, at most `JETBRAINS_MOST`;
//! - Helix and the user's own Neovim setup draw in the terminal: they are handed platoon's terminal exactly as
//!   platoon's editor is, on the first folder (they open one folder at a time).
//!
//! Editors with a window of their own start beside platoon and are left alone (their own session, no output, no
//! waiting): platoon cannot tell when they close.
//!
//! Knobs (the same as `editors.rs`): `PLATOON_EDITOR_PATH` is the only place looked in (instead of `PATH` and the
//! install places); `PLATOON_EDITOR_BIN` runs that program instead of the editor chosen; with neither set,
//! `PLATOON_FORBID_REAL_EDITOR` refuses to start one.

use std::{
    fs, io,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
};

/// The id of platoon's own editor, in the settings file and the list.
pub const PLATOON: &str = "platoon";

/// A JetBrains IDE opens this many folders at most, one window each.
pub const JETBRAINS_MOST: usize = 3;

/// Where an editor draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Runs {
    /// platoon's own editor: handed the terminal (`editor.rs`).
    Platoon,
    /// The machine's own editor that draws in the terminal: handed the terminal the same way.
    Terminal,
    /// A window of its own, beside platoon.
    Window,
}

/// What an editor does with several folders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Several {
    /// All of them in one window.
    OneWindow,
    /// One window each, at most `JETBRAINS_MOST`.
    EachWindow,
    /// The first only.
    First,
}

/// One editor `v` can offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEditor {
    /// What the settings file holds (`files_editor`).
    pub id: String,
    /// What a person calls it.
    pub name: String,
    /// The program, an absolute path where one was found (`open` for a macOS app bundle). Empty for platoon's.
    pub program: String,
    /// Arguments before the folder(s).
    pub args: Vec<String>,
    pub runs: Runs,
    pub several: Several,
}

impl FileEditor {
    pub fn platoon() -> Self {
        Self { id: PLATOON.into(), name: "platoon's editor".into(), program: String::new(), args: Vec::new(), runs: Runs::Platoon, several: Several::OneWindow }
    }

    pub fn is_platoon(&self) -> bool {
        self.runs == Runs::Platoon
    }
}

/// An editor platoon knows how to find and start.
struct Known {
    id: &'static str,
    name: &'static str,
    /// Program names, in the order they are looked for.
    commands: &'static [&'static str],
    /// Arguments before the folders.
    args: &'static [&'static str],
    runs: Runs,
    several: Several,
    /// On macOS: app bundles (in `/Applications`, `~/Applications`) and, for the ones with a command line tool of
    /// their own, where it is inside the bundle ("" to start the bundle with `open -na`).
    mac: &'static [(&'static str, &'static str)],
}

const fn window(id: &'static str, name: &'static str, commands: &'static [&'static str], args: &'static [&'static str], mac: &'static [(&'static str, &'static str)]) -> Known {
    Known { id, name, commands, args, runs: Runs::Window, several: Several::OneWindow, mac }
}

/// A JetBrains IDE: `<launcher> <folder>`, one call per folder. Its command line has no documented way to force a new
/// window (JetBrains' help, "Command-line interface" and "Open files from the command line", lists only `nosplash`,
/// `dontReopenProjects`, `disableNonBundledPlugins`, `--wait` and the diff/merge/format/inspect/installPlugins
/// commands), so a running IDE opens the folder as its own "Open project in" setting says (new window, this window, or
/// ask); platoon never changes that setting.
const fn jetbrains(id: &'static str, name: &'static str, commands: &'static [&'static str], mac: &'static [(&'static str, &'static str)]) -> Known {
    Known { id, name, commands, args: &[], runs: Runs::Window, several: Several::EachWindow, mac }
}

/// In the order the list shows them (after platoon's own): windowed editors, the JetBrains family, then the ones in
/// the terminal.
const KNOWN: &[Known] = &[
    // `--new-window` with several folders: one window with each as a root (an untitled multi-root workspace).
    window("code", "VS Code", &["code"], &["--new-window"], &[("Visual Studio Code.app", "Contents/Resources/app/bin/code")]),
    window("cursor", "Cursor", &["cursor"], &["--new-window"], &[("Cursor.app", "Contents/Resources/app/bin/cursor")]),
    // `zed --new a b`: a new workspace holding both folders. Some Linux packages call it `zeditor`.
    window("zed", "Zed", &["zed", "zeditor"], &["--new"], &[("Zed.app", "Contents/MacOS/cli")]),
    // `subl --new-window a b`: a new window with both folders in its sidebar.
    window("subl", "Sublime Text", &["subl"], &["--new-window"], &[("Sublime Text.app", "Contents/SharedSupport/bin/subl")]),
    // `kate --new a b`: a Kate of its own with both folders (its Projects view).
    window("kate", "Kate", &["kate"], &["--new"], &[]),
    jetbrains("idea", "IntelliJ IDEA", &["idea", "idea.sh", "intellij-idea-ultimate"], &[("IntelliJ IDEA.app", ""), ("IntelliJ IDEA Ultimate.app", "")]),
    jetbrains("idea-community", "IntelliJ IDEA Community", &["idea-community", "intellij-idea-community"], &[("IntelliJ IDEA CE.app", ""), ("IntelliJ IDEA Community Edition.app", "")]),
    jetbrains("pycharm", "PyCharm", &["pycharm", "pycharm.sh", "pycharm-professional"], &[("PyCharm.app", ""), ("PyCharm Professional Edition.app", "")]),
    jetbrains("pycharm-community", "PyCharm Community", &["pycharm-community"], &[("PyCharm CE.app", ""), ("PyCharm Community Edition.app", "")]),
    jetbrains("webstorm", "WebStorm", &["webstorm", "webstorm.sh"], &[("WebStorm.app", "")]),
    jetbrains("goland", "GoLand", &["goland", "goland.sh"], &[("GoLand.app", "")]),
    jetbrains("rustrover", "RustRover", &["rustrover", "rustrover.sh"], &[("RustRover.app", "")]),
    jetbrains("clion", "CLion", &["clion", "clion.sh"], &[("CLion.app", "")]),
    jetbrains("phpstorm", "PhpStorm", &["phpstorm", "phpstorm.sh"], &[("PhpStorm.app", "")]),
    jetbrains("rider", "Rider", &["rider", "rider.sh"], &[("Rider.app", "")]),
    jetbrains("android-studio", "Android Studio", &["studio", "studio.sh", "android-studio"], &[("Android Studio.app", "")]),
    Known { id: "helix", name: "Helix", commands: &["hx", "helix"], args: &[], runs: Runs::Terminal, several: Several::First, mac: &[] },
    // Only with a config folder of the user's own: that is what "your setup" means (plain nvim is not a file-tree
    // editor, and platoon's own is first in the list anyway).
    Known { id: "nvim", name: "Neovim (your setup)", commands: &["nvim"], args: &[], runs: Runs::Terminal, several: Several::First, mac: &[] },
];

/// What a person calls the editor an id names (also one this computer no longer has).
pub fn name_of(id: &str) -> String {
    if id == PLATOON {
        return FileEditor::platoon().name;
    }
    KNOWN.iter().find(|k| k.id == id).map_or_else(|| id.to_string(), |k| k.name.to_string())
}

/// Where to look.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Places {
    /// `PATH` (or `PLATOON_EDITOR_PATH`), in order.
    pub path: Vec<PathBuf>,
    /// Then these: where editors install themselves outside `PATH`.
    pub extra: Vec<PathBuf>,
    /// macOS: folders holding `.app` bundles.
    pub apps: Vec<PathBuf>,
    /// The user's own Neovim config folder, when there is one.
    pub nvim_config: Option<PathBuf>,
}

/// The places for this environment. `root` is `/` except in tests; `macos` says which set of places applies.
/// With `PLATOON_EDITOR_PATH` set, that is the only place looked in (a test's world, whatever this machine has).
pub fn places(env: &dyn Fn(&str) -> Option<String>, root: &Path, macos: bool) -> Places {
    let set = |k: &str| env(k).filter(|v| !v.is_empty());
    let home = set("HOME").map(PathBuf::from);
    let appname = set("NVIM_APPNAME").unwrap_or_else(|| "nvim".into());
    let config_home = set("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| home.as_ref().map(|h| h.join(".config")));
    let nvim_config = config_home.map(|c| c.join(&appname)).filter(|d| d.is_dir());
    if let Some(only) = env("PLATOON_EDITOR_PATH") {
        let path = only.split(':').filter(|p| !p.is_empty()).map(PathBuf::from).collect();
        return Places { path, extra: Vec::new(), apps: Vec::new(), nvim_config };
    }
    let path = env("PATH").unwrap_or_default().split(':').filter(|p| !p.is_empty()).map(PathBuf::from).collect();
    let under = |p: &str| root.join(p.trim_start_matches('/'));
    let mut extra = Vec::new();
    if let Some(h) = &home {
        if macos {
            extra.push(h.join("Library/Application Support/JetBrains/Toolbox/scripts"));
        } else {
            extra.push(h.join(".local/share/JetBrains/Toolbox/scripts"));
        }
        extra.push(h.join(".local/bin"));
    }
    if !macos {
        extra.push(under("/snap/bin"));
        // A tarball unpacked into /opt (`/opt/idea-IU-253.1/bin/idea.sh`, `/opt/android-studio/bin/studio.sh`).
        let mut opt: Vec<PathBuf> = fs::read_dir(under("/opt")).into_iter().flatten().flatten().map(|e| e.path().join("bin")).filter(|b| b.is_dir()).collect();
        opt.sort();
        extra.extend(opt);
    }
    let mut apps = Vec::new();
    if macos {
        apps.push(under("/Applications"));
        if let Some(h) = &home {
            apps.push(h.join("Applications"));
        }
    }
    Places { path, extra, apps, nvim_config }
}

fn runnable(p: &Path) -> bool {
    p.is_file() && p.metadata().is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

fn find(commands: &[&str], dirs: &[PathBuf]) -> Option<String> {
    commands.iter().find_map(|c| dirs.iter().map(|d| d.join(c)).find(|p| runnable(p))).map(|p| p.to_string_lossy().into_owned())
}

/// The editors `v` offers here: platoon's own first, then each one found, in `KNOWN`'s order.
pub fn detect_in(places: &Places) -> Vec<FileEditor> {
    let mut out = vec![FileEditor::platoon()];
    let dirs: Vec<PathBuf> = places.path.iter().chain(&places.extra).cloned().collect();
    for k in KNOWN {
        if k.id == "nvim" && places.nvim_config.is_none() {
            continue;
        }
        let args = || k.args.iter().map(|a| (*a).to_string()).collect::<Vec<_>>();
        let of = |program: String, args: Vec<String>| FileEditor { id: k.id.into(), name: k.name.into(), program, args, runs: k.runs, several: k.several };
        if let Some(found) = find(k.commands, &dirs) {
            out.push(of(found, args()));
            continue;
        }
        // macOS: the app bundle, by its command line tool, or started with `open -na <bundle> --args`.
        let bundle = k.mac.iter().find_map(|(app, tool)| places.apps.iter().map(|d| (d.join(app), *tool)).find(|(b, _)| b.is_dir()));
        if let Some((b, tool)) = bundle {
            if tool.is_empty() {
                out.push(of("open".into(), vec!["-na".into(), b.to_string_lossy().into_owned(), "--args".into()]));
            } else if runnable(&b.join(tool)) {
                out.push(of(b.join(tool).to_string_lossy().into_owned(), args()));
            }
        }
    }
    out
}

/// The editors `v` offers on this computer. (A unit test's app never calls it: see `EdState::files`.)
#[cfg_attr(test, allow(dead_code))]
pub fn detect() -> Vec<FileEditor> {
    let env = |k: &str| std::env::var(k).ok();
    detect_in(&places(&env, Path::new("/"), cfg!(target_os = "macos")))
}

/// The editor an id names in `list`, if this computer has it.
pub fn by_id<'a>(list: &'a [FileEditor], id: &str) -> Option<&'a FileEditor> {
    list.iter().find(|e| e.id == id)
}

/// The commands that open `dirs` in `e` (a windowed or terminal editor), and how many folders were left out.
pub fn commands(e: &FileEditor, dirs: &[String]) -> (Vec<(String, Vec<String>)>, usize) {
    let with = |ds: &[String]| {
        let mut a = e.args.clone();
        a.extend(ds.iter().cloned());
        (e.program.clone(), a)
    };
    match e.several {
        _ if dirs.is_empty() || e.is_platoon() => (Vec::new(), 0),
        Several::OneWindow => (vec![with(dirs)], 0),
        Several::EachWindow => {
            let n = dirs.len().min(JETBRAINS_MOST);
            (dirs[..n].iter().map(|d| with(std::slice::from_ref(d))).collect(), dirs.len() - n)
        }
        Several::First => (vec![with(&dirs[..1])], dirs.len() - 1),
    }
}

/// The program to really run, with the test knobs applied.
pub fn program_of(program: &str, env: &dyn Fn(&str) -> Option<String>) -> io::Result<String> {
    let stand_in = env("PLATOON_EDITOR_BIN").filter(|v| !v.is_empty());
    if stand_in.is_none() && env("PLATOON_EDITOR_PATH").is_none() && env("PLATOON_FORBID_REAL_EDITOR").is_some() {
        return Err(io::Error::other("PLATOON_FORBID_REAL_EDITOR is set: refusing to start a real editor"));
    }
    Ok(stand_in.unwrap_or_else(|| program.to_string()))
}

/// Start a windowed editor beside platoon and leave it alone: in `cwd`, its own session (closing platoon's terminal
/// can't hang it up), no input, its output thrown away. A thread waits for the program to end (a launcher usually
/// hands over to a running copy and ends at once), so nothing is left behind as a zombie.
pub fn launch_window(program: &str, args: &[String], cwd: &Path, env: &dyn Fn(&str) -> Option<String>) -> io::Result<()> {
    let program = program_of(program, env)?;
    let mut cmd = Command::new(&program);
    cmd.args(args);
    if cwd.is_dir() {
        cmd.current_dir(cwd);
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // If platoon itself was started from inside Claude Code, don't leak that into the editor's terminals.
    cmd.env_remove("CLAUDECODE").env_remove("CLAUDE_CODE_ENTRYPOINT");
    // SAFETY: only setsid between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd.spawn().map_err(|err| io::Error::other(format!("couldn't start {program}: {err}")))?;
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// How a terminal editor of the user's own is run on platoon's terminal (`editor::run`): in the folder, with the
/// user's environment as it is (not platoon's editor's world), only Claude Code's marks taken out.
pub fn terminal_launch(e: &FileEditor, dir: &str, env: &dyn Fn(&str) -> Option<String>) -> io::Result<crate::editor::Launch> {
    let (cmds, _) = commands(e, &[dir.to_string()]);
    let (program, args) = cmds.into_iter().next().ok_or_else(|| io::Error::other("nothing to open"))?;
    Ok(crate::editor::Launch {
        program: PathBuf::from(program_of(&program, env)?),
        args,
        env: vec![("CLAUDECODE".into(), None), ("CLAUDE_CODE_ENTRYPOINT".into(), None)],
        cwd: PathBuf::from(dir),
        session: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A pretend computer: a root folder with a home in it, programs placed where a test says.
    struct World {
        root: PathBuf,
        vars: HashMap<String, String>,
    }

    impl World {
        fn new(name: &str) -> Self {
            let root = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-fileeditors-{name}-{}", std::process::id())));
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

        fn set(&mut self, k: &str, v: &str) -> &mut Self {
            self.vars.insert(k.into(), v.into());
            self
        }

        fn places(&self, macos: bool) -> Places {
            places(&|k| self.vars.get(k).cloned(), &self.root, macos)
        }

        fn list(&self) -> Vec<FileEditor> {
            detect_in(&self.places(false))
        }

        fn ids(&self) -> Vec<String> {
            self.list().into_iter().map(|e| e.id).collect()
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
    fn nothing_installed_offers_platoons_editor_alone() {
        let w = World::new("none");
        let list = w.list();
        assert_eq!(list, vec![FileEditor::platoon()]);
        assert_eq!(list[0].name, "platoon's editor");
        assert_eq!(list[0].runs, Runs::Platoon);
    }

    #[test]
    fn each_editor_is_listed_only_when_its_program_is_there() {
        for (program, id, name) in [
            ("usr/bin/code", "code", "VS Code"),
            ("usr/bin/cursor", "cursor", "Cursor"),
            ("usr/bin/zed", "zed", "Zed"),
            ("usr/bin/zeditor", "zed", "Zed"),
            ("usr/bin/subl", "subl", "Sublime Text"),
            ("usr/bin/kate", "kate", "Kate"),
            ("usr/bin/idea", "idea", "IntelliJ IDEA"),
            ("usr/bin/idea.sh", "idea", "IntelliJ IDEA"),
            ("usr/bin/pycharm", "pycharm", "PyCharm"),
            ("usr/bin/webstorm", "webstorm", "WebStorm"),
            ("usr/bin/goland", "goland", "GoLand"),
            ("usr/bin/rustrover", "rustrover", "RustRover"),
            ("usr/bin/clion", "clion", "CLion"),
            ("usr/bin/phpstorm", "phpstorm", "PhpStorm"),
            ("usr/bin/rider", "rider", "Rider"),
            ("usr/bin/studio.sh", "android-studio", "Android Studio"),
            ("usr/bin/hx", "helix", "Helix"),
            ("usr/bin/helix", "helix", "Helix"),
        ] {
            let w = World::new(&format!("one-{}", program.replace('/', "-")));
            w.program(program);
            let list = w.list();
            assert_eq!(list.len(), 2, "{program}: {list:?}");
            assert_eq!((list[1].id.as_str(), list[1].name.as_str()), (id, name), "{program}");
            assert_eq!(list[1].program, w.at(program), "the program found, by its full path");
        }
    }

    #[test]
    fn a_file_that_cannot_be_run_or_a_name_alone_is_not_an_editor() {
        let w = World::new("not-runnable");
        fs::write(w.root.join("usr/bin/code"), "not a program").unwrap();
        w.dir("usr/bin/cursor"); // a folder called cursor
        assert_eq!(w.ids(), ["platoon"]);
    }

    #[test]
    fn editors_without_a_file_tree_are_never_listed() {
        let w = World::new("no-tree");
        for p in ["vi", "vim", "nano", "micro", "gedit", "mousepad", "emacs", "gnome-text-editor"] {
            w.program(&format!("usr/bin/{p}"));
        }
        assert_eq!(w.ids(), ["platoon"]);
    }

    #[test]
    fn your_own_neovim_only_with_a_config_folder_of_yours() {
        let mut w = World::new("nvim");
        w.program("usr/bin/nvim");
        assert_eq!(w.ids(), ["platoon"], "nvim alone is not 'your setup'");
        w.dir("home/u/.config/nvim");
        assert_eq!(w.ids(), ["platoon", "nvim"]);
        assert_eq!(w.list()[1].name, "Neovim (your setup)");
        assert_eq!(w.list()[1].runs, Runs::Terminal);
        // XDG_CONFIG_HOME wins over ~/.config, and NVIM_APPNAME names the folder.
        let other = w.at("xdg");
        w.set("XDG_CONFIG_HOME", &other);
        assert_eq!(w.ids(), ["platoon"], "no nvim folder in XDG_CONFIG_HOME");
        w.dir("xdg/mine");
        w.set("NVIM_APPNAME", "mine");
        assert_eq!(w.ids(), ["platoon", "nvim"]);
        // A config folder without nvim: nothing.
        let mut v = World::new("nvim-config-only");
        v.dir("home/u/.config/nvim");
        v.set("PATH", "");
        assert_eq!(v.ids(), ["platoon"]);
    }

    #[test]
    fn jetbrains_toolbox_scripts_snaps_and_opt_are_found_off_the_path() {
        let w = World::new("jb-places");
        w.program("home/u/.local/share/JetBrains/Toolbox/scripts/pycharm");
        w.program("snap/bin/intellij-idea-community");
        w.program("opt/idea-IU-253.1/bin/idea.sh");
        w.program("opt/android-studio/bin/studio.sh");
        w.program("home/u/.local/bin/rustrover");
        let list = w.list();
        let found: Vec<(&str, &str)> = list.iter().map(|e| (e.id.as_str(), e.program.as_str())).collect();
        let (idea, community, pycharm, rust, studio) = (
            w.at("opt/idea-IU-253.1/bin/idea.sh"),
            w.at("snap/bin/intellij-idea-community"),
            w.at("home/u/.local/share/JetBrains/Toolbox/scripts/pycharm"),
            w.at("home/u/.local/bin/rustrover"),
            w.at("opt/android-studio/bin/studio.sh"),
        );
        assert_eq!(
            found,
            vec![("platoon", ""), ("idea", idea.as_str()), ("idea-community", community.as_str()), ("pycharm", pycharm.as_str()), ("rustrover", rust.as_str()), ("android-studio", studio.as_str())]
        );
        assert!(list[1..].iter().all(|e| e.runs == Runs::Window && e.several == Several::EachWindow));
        // PATH comes first when both have it.
        w.program("usr/bin/pycharm");
        assert_eq!(by_id(&w.list(), "pycharm").unwrap().program, w.at("usr/bin/pycharm"));
    }

    #[test]
    fn a_test_path_is_the_only_place_looked_in() {
        let mut w = World::new("test-path");
        w.program("snap/bin/code");
        w.program("fake/zed");
        let only = w.at("fake");
        w.set("PLATOON_EDITOR_PATH", &only);
        assert_eq!(w.ids(), ["platoon", "zed"]);
    }

    #[test]
    fn macos_app_bundles_by_their_own_tool_or_by_open() {
        let w = World::new("mac");
        w.dir("Applications/IntelliJ IDEA CE.app");
        w.program("Applications/Visual Studio Code.app/Contents/Resources/app/bin/code");
        w.dir("home/u/Applications/GoLand.app");
        w.dir("Applications/Zed.app"); // no tool inside: not offered
        let list = detect_in(&w.places(true));
        let code = by_id(&list, "code").unwrap();
        assert_eq!(code.program, w.at("Applications/Visual Studio Code.app/Contents/Resources/app/bin/code"));
        assert_eq!(code.args, ["--new-window"]);
        let idea = by_id(&list, "idea-community").unwrap();
        assert_eq!((idea.program.as_str(), idea.args.clone()), ("open", vec!["-na".to_string(), w.at("Applications/IntelliJ IDEA CE.app"), "--args".to_string()]));
        assert!(by_id(&list, "goland").is_some(), "~/Applications too");
        assert!(by_id(&list, "zed").is_none());
        let (cmds, _) = commands(idea, &["/r/a".into()]);
        assert_eq!(cmds[0].1, vec!["-na".to_string(), w.at("Applications/IntelliJ IDEA CE.app"), "--args".into(), "/r/a".into()]);
        // Linux places are not macOS ones.
        assert_eq!(w.ids(), ["platoon"]);
    }

    #[test]
    fn the_order_is_platoons_then_windows_then_jetbrains_then_the_terminal() {
        let w = World::new("order");
        for p in ["hx", "rider", "code", "kate", "idea", "subl", "nvim", "zed", "cursor"] {
            w.program(&format!("usr/bin/{p}"));
        }
        w.dir("home/u/.config/nvim");
        assert_eq!(w.ids(), ["platoon", "code", "cursor", "zed", "subl", "kate", "idea", "rider", "helix", "nvim"]);
    }

    #[test]
    fn how_each_editor_opens_one_folder_and_several() {
        let w = World::new("argv");
        for p in ["code", "cursor", "zed", "subl", "kate", "idea", "hx", "nvim"] {
            w.program(&format!("usr/bin/{p}"));
        }
        w.dir("home/u/.config/nvim");
        let list = w.list();
        let one = vec!["/r/a".to_string()];
        let four: Vec<String> = ["/r/a", "/r/b", "/r/c", "/r/d"].iter().map(|s| s.to_string()).collect();
        let argv = |id: &str, dirs: &[String]| {
            let (cmds, left) = commands(by_id(&list, id).unwrap(), dirs);
            (cmds.into_iter().map(|(p, a)| format!("{} {}", p.rsplit('/').next().unwrap(), a.join(" "))).collect::<Vec<_>>(), left)
        };
        assert_eq!(argv("code", &one), (vec!["code --new-window /r/a".to_string()], 0));
        assert_eq!(argv("code", &four), (vec!["code --new-window /r/a /r/b /r/c /r/d".to_string()], 0), "one window, every folder a root");
        assert_eq!(argv("cursor", &four).0, vec!["cursor --new-window /r/a /r/b /r/c /r/d"]);
        assert_eq!(argv("zed", &four).0, vec!["zed --new /r/a /r/b /r/c /r/d"]);
        assert_eq!(argv("subl", &four).0, vec!["subl --new-window /r/a /r/b /r/c /r/d"]);
        assert_eq!(argv("kate", &four).0, vec!["kate --new /r/a /r/b /r/c /r/d"]);
        assert_eq!(argv("idea", &one), (vec!["idea /r/a".to_string()], 0));
        assert_eq!(argv("idea", &four), (vec!["idea /r/a".to_string(), "idea /r/b".into(), "idea /r/c".into()], 1), "a window each, at most three");
        assert_eq!(argv("helix", &four), (vec!["hx /r/a".to_string()], 3), "the first folder");
        assert_eq!(argv("nvim", &four), (vec!["nvim /r/a".to_string()], 3));
        assert_eq!(commands(&FileEditor::platoon(), &four), (Vec::new(), 0), "platoon's own is run by editor.rs");
    }

    #[test]
    fn names_of_editors_this_computer_may_not_have() {
        assert_eq!(name_of("code"), "VS Code");
        assert_eq!(name_of("platoon"), "platoon's editor");
        assert_eq!(name_of("rustrover"), "RustRover");
        assert_eq!(name_of("something-else"), "something-else");
    }

    #[test]
    fn a_stand_in_replaces_the_program_and_a_real_one_can_be_forbidden() {
        let env = |vars: Vec<(&str, &str)>| {
            let map: HashMap<String, String> = vars.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            move |k: &str| map.get(k).cloned()
        };
        assert_eq!(program_of("/usr/bin/code", &env(vec![])).unwrap(), "/usr/bin/code");
        assert_eq!(program_of("/usr/bin/code", &env(vec![("PLATOON_EDITOR_BIN", "/bin/true")])).unwrap(), "/bin/true");
        assert!(program_of("/usr/bin/code", &env(vec![("PLATOON_FORBID_REAL_EDITOR", "1")])).is_err());
        assert!(program_of("/x/code", &env(vec![("PLATOON_FORBID_REAL_EDITOR", "1"), ("PLATOON_EDITOR_PATH", "/x")])).is_ok());
        assert!(launch_window("/usr/bin/code", &[], Path::new("/"), &env(vec![("PLATOON_FORBID_REAL_EDITOR", "1")])).is_err(), "refused before anything starts");
    }

    #[test]
    fn a_window_editor_starts_beside_platoon_with_its_folders_and_is_left_alone() {
        let w = World::new("launch");
        let out = w.root.join("out");
        let script = w.root.join("usr/bin/code");
        fs::write(&script, format!("#!/bin/sh\npwd > '{0}.cwd'\nprintf '%s\\n' \"$@\" > '{0}.tmp'\nmv '{0}.tmp' '{0}'\n", s(&out))).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let e = by_id(&w.list(), "code").unwrap().clone();
        let (cmds, _) = commands(&e, &[s(&w.root.join("home")), s(&w.root.join("usr"))]);
        let (program, args) = &cmds[0];
        launch_window(program, args, &w.root.join("home"), &|_| None).unwrap();
        let start = std::time::Instant::now();
        while !out.is_file() {
            assert!(start.elapsed() < std::time::Duration::from_secs(10), "the stand-in never ran");
            thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(fs::read_to_string(&out).unwrap(), format!("--new-window\n{}\n{}\n", s(&w.root.join("home")), s(&w.root.join("usr"))));
        assert_eq!(fs::read_to_string(w.root.join("out.cwd")).unwrap().trim(), s(&w.root.join("home")));
    }

    #[test]
    fn a_terminal_editor_of_yours_runs_in_the_folder_with_your_own_environment() {
        let e = FileEditor { id: "helix".into(), name: "Helix".into(), program: "/x/hx".into(), args: vec![], runs: Runs::Terminal, several: Several::First };
        let l = terminal_launch(&e, "/r/a", &|_| None).unwrap();
        assert_eq!(l.program, PathBuf::from("/x/hx"));
        assert_eq!(l.args, ["/r/a"]);
        assert_eq!(l.cwd, PathBuf::from("/r/a"));
        assert_eq!(l.session, None);
        assert!(l.env.iter().all(|(k, v)| v.is_none() && k.starts_with("CLAUDE")), "nothing of platoon's editor's world: {:?}", l.env);
    }
}
