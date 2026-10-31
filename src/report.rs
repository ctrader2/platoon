//! `platoon report`: what a maintainer would ask for about a problem, gathered into one plain-text file the person reads
//! before sending it anywhere (an issue, an email). Nothing is sent: platoon writes the file and says where it is.
//!
//! What it holds: versions (platoon, Claude Code, Codex, the system, the terminal), where setup stands, platoon's own
//! settings, counts of the conversations and tasks, the sessions platoon runs and what runs under them (programs by
//! name), the usage meter's figures and what it chose, the smart features' spend, and the last lines of the diagnostics
//! log (`diag.rs`). What it never holds: anything said in a conversation (prompts, replies, transcripts, notes, task or
//! squad text, summaries, the names given to conversations), a command line or an environment variable's value (only
//! `TERM` and its kin, which name the terminal), anything from Claude Code's or Codex's own settings beyond whether
//! platoon's entries there are in place.
//!
//! As with the log, that is a matter of types, not care: the report is written through `Doc`, whose lines are made of
//! `Cell`s: words in platoon's code (`&'static str`), `diag::Safe` values (numbers, durations, checked ids, program
//! names, identifiers), folders (`Path`) and project names. Folders and project names go through `Names`: shown from
//! `~` as they are, or with `--hide-names` each folder name and each project name replaced by a placeholder
//! (`folder-1`, `project-2`), the same one wherever it comes up. There is no way to put a `String` into a line; the
//! diagnostics log's lines, which are made of the same safe values, are the one thing copied in as they are.
//!
//! Every probe is time-boxed (the programs asked for their version get `PROBE_WAIT`, all at once), and nothing needs a
//! terminal.

use crate::agents::{Agent, Providers};
use crate::diag::{self, Program, Safe};
use crate::host::HostInfo;
use crate::tickets::Ticket;
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

/// How long a program asked for its version (`claude --version`, `codex --version`, macOS's `sw_vers`) may take.
pub const PROBE_WAIT: Duration = Duration::from_secs(5);
/// How many of the diagnostics log's last lines the report carries.
const LOG_LINES: usize = 300;
/// How many days of the smart features' spend it shows.
const SPEND_DAYS: usize = 7;
/// How many of the status line's reports it lists.
const REPORTS_SHOWN: usize = 40;

pub const USAGE: &str = "usage: platoon report [--hide-names] [--out <file> | --stdout]
  Writes what is useful for a bug report into one plain-text file and says where it is: versions, setup, platoon's
  settings, counts of your sessions and tasks, the sessions platoon runs, the usage meter and the diagnostics log.
  It holds no conversation content, no command lines and nothing secret; read it before you send it.
  --hide-names   replace every project and folder name with a placeholder (project-1, folder-2)
  --out <file>   write it there instead of platoon-report-<date>.txt in the current folder
  --stdout       print it instead of writing a file";

// ---- names and folders --------------------------------------------------------------------------------------------

/// Folder names that are platoon's, Claude Code's, Codex's or the system's, not the person's: kept as they are even
/// with `--hide-names`.
const KEPT_FOLDERS: &[&str] =
    &[".cache", ".config", ".claude", ".codex", "platoon", "Library", "Caches", "Application Support", "tmp", "private", "var"];

/// How folders and project names are shown: as they are (folders from `~`), or each name as a placeholder, the same one
/// every time it comes up.
pub struct Names {
    hide: bool,
    home: Option<PathBuf>,
    folders: HashMap<String, String>,
    projects: HashMap<String, String>,
}

impl Names {
    pub fn new(hide: bool, home: Option<PathBuf>) -> Names {
        Names { hide, home: home.filter(|h| h.components().count() > 1), folders: HashMap::new(), projects: HashMap::new() }
    }

    /// A folder: `~/repos/acme`, or with names hidden `~/folder-1/folder-2`.
    pub fn place(&mut self, p: &Path) -> String {
        let (start, rest) = match self.home.as_deref().and_then(|h| p.strip_prefix(h).ok()) {
            Some(rest) => ("~", rest),
            None => ("", p),
        };
        let names: Vec<String> = rest
            .components()
            .filter_map(|c| match c {
                std::path::Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        let parts: Vec<String> = names.iter().map(|n| self.folder(n)).collect();
        let joined = parts.join("/");
        match (start, joined.is_empty()) {
            ("~", true) => "~".into(),
            ("~", false) => format!("~/{joined}"),
            _ => format!("/{joined}"),
        }
    }

    fn folder(&mut self, name: &str) -> String {
        let task_folder = name.strip_prefix("task-").is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
        if !self.hide || KEPT_FOLDERS.contains(&name) || task_folder {
            return printable(name);
        }
        let next = self.folders.len() + 1;
        self.folders.entry(name.to_string()).or_insert_with(|| format!("folder-{next}")).clone()
    }

    /// A project's name, or with names hidden `project-1`.
    pub fn project(&mut self, name: &str) -> String {
        if !self.hide {
            return printable(name);
        }
        let next = self.projects.len() + 1;
        self.projects.entry(name.to_string()).or_insert_with(|| format!("project-{next}")).clone()
    }
}

/// A name as it may be shown: control characters out.
fn printable(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

// ---- the document -------------------------------------------------------------------------------------------------

/// A piece of a line: words in platoon's code, a safe value, a folder or a project's name.
pub enum Cell<'a> {
    T(&'static str),
    V(Safe),
    Place(&'a Path),
    Project(&'a str),
}

use Cell::{Place, Project, T, V};

/// The report as it is written: headings and lines of cells.
pub struct Doc {
    names: Names,
    text: String,
}

impl Doc {
    pub fn new(names: Names) -> Doc {
        Doc { names, text: String::new() }
    }

    fn heading(&mut self, title: &'static str) {
        self.text.push_str(&format!("\n== {title} ==\n"));
    }

    fn line(&mut self, cells: &[Cell]) {
        for c in cells {
            let piece = match c {
                T(t) => (*t).to_string(),
                V(v) => v.to_string(),
                Place(p) => self.names.place(p),
                Project(n) => self.names.project(n),
            };
            self.text.push_str(&piece);
        }
        self.text.push('\n');
    }

    /// `  label: value`.
    fn kv(&mut self, label: &'static str, v: impl Into<Safe>) {
        self.line(&[T("  "), T(label), T(": "), V(v.into())]);
    }

    /// The diagnostics log's lines (`diag::tail`), which are written of safe values only and read back as such.
    fn log_lines(&mut self, lines: Vec<String>) {
        for l in lines {
            self.text.push_str("  ");
            self.text.push_str(&l);
            self.text.push('\n');
        }
    }

    pub fn text(self) -> String {
        self.text
    }
}

fn num(n: impl Into<Safe>) -> Cell<'static> {
    V(n.into())
}

fn yes_no(b: bool) -> Cell<'static> {
    T(if b { "yes" } else { "no" })
}

/// How long ago `then` was from `now` (seconds since the epoch).
fn ago(then: u64, now: u64) -> Cell<'static> {
    V(Duration::from_secs(now.saturating_sub(then)).into())
}

/// How long from `now` until `then`.
fn until(then: u64, now: u64) -> Cell<'static> {
    V(Duration::from_secs(then.saturating_sub(now)).into())
}

// ---- where to look ------------------------------------------------------------------------------------------------

/// The folders the report reads.
pub struct Places {
    pub cache: PathBuf,
    pub config: PathBuf,
    pub claude_root: PathBuf,
    pub codex_root: PathBuf,
    pub home: Option<PathBuf>,
}

impl Places {
    /// The real ones, from the environment (not for unit tests).
    pub fn from_env() -> Places {
        Places {
            cache: crate::usage::cache_dir(),
            config: crate::archive::config_dir(),
            claude_root: crate::agents::claude::claude_root(),
            codex_root: crate::agents::codex::codex_root(),
            home: std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from),
        }
    }
}

/// What this computer has, so a test can give its own.
pub struct Probes {
    /// The programs installed, whatever the settings say.
    pub installed: Providers,
    /// How this platoon was installed.
    pub install: crate::versions::Install,
    /// How long a program asked for its version may take.
    pub wait: Duration,
}

impl Probes {
    pub fn from_env() -> Probes {
        Probes { installed: Providers::installed_here(), install: crate::versions::install_here(), wait: PROBE_WAIT }
    }
}

// ---- the command --------------------------------------------------------------------------------------------------

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub hide_names: bool,
    pub stdout: bool,
    pub out: Option<PathBuf>,
}

/// `platoon report`'s arguments; None for `--help`.
pub fn parse_args(args: &[String]) -> Result<Option<Options>, String> {
    let mut o = Options::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--hide-names" => o.hide_names = true,
            "--stdout" | "-" => o.stdout = true,
            "--out" => o.out = Some(PathBuf::from(it.next().ok_or("--out needs a file")?)),
            "-h" | "--help" => return Ok(None),
            _ => return Err(format!("unknown argument: {a}\n{USAGE}")),
        }
    }
    if o.stdout && o.out.is_some() {
        return Err(format!("--out and --stdout can't go together\n{USAGE}"));
    }
    Ok(Some(o))
}

/// `platoon report [--hide-names] [--out <file> | --stdout]`.
pub fn cli(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let opts = match parse_args(args) {
        Ok(Some(o)) => o,
        Ok(None) => {
            let _ = writeln!(out, "{USAGE}");
            return 0;
        }
        Err(e) => {
            let _ = writeln!(err, "{e}");
            return 2;
        }
    };
    let places = Places::from_env();
    let now = crate::usage::now();
    let text = make(&places, &Probes::from_env(), opts.hide_names, now);
    if opts.stdout {
        let _ = out.write_all(text.as_bytes());
        return 0;
    }
    let name = format!("platoon-report-{}.txt", file_stamp(now));
    // The current folder, where the person will look for it; else their home folder; else platoon's cache.
    let tries: Vec<PathBuf> = match &opts.out {
        Some(p) => vec![p.clone()],
        None => {
            let here = std::env::current_dir().ok().map(|d| d.join(&name));
            let home = places.home.as_ref().map(|h| h.join(&name));
            here.into_iter().chain(home).chain([places.cache.join("reports").join(&name)]).collect()
        }
    };
    for path in &tries {
        if path.starts_with(places.cache.join("reports")) {
            let _ = fs::create_dir_all(places.cache.join("reports"));
        }
        if fs::write(path, &text).is_ok() {
            let full = std::path::absolute(path).unwrap_or_else(|_| path.clone());
            let shown = Names::new(false, places.home.clone()).place(&full);
            let log_on = crate::settings::load(&places.config).0.diagnostics_log;
            let _ = writeln!(out, "{}", summary(&shown, text.len(), opts.hide_names, log_on));
            return 0;
        }
    }
    let _ = writeln!(err, "platoon report: couldn't write the report anywhere. `platoon report --stdout` prints it instead.");
    1
}

/// What `platoon report` says once the file is written.
pub fn summary(path: &str, bytes: usize, hidden: bool, log_on: bool) -> String {
    let kb = bytes.div_ceil(1024);
    let names = if hidden {
        "Project and folder names are replaced by placeholders."
    } else {
        "Your project and folder names are in it: `platoon report --hide-names` replaces them with placeholders."
    };
    let log = if log_on {
        "The diagnostics log is on, so it shows what platoon decided and why."
    } else {
        "The diagnostics log is off, so the report has platoon's errors only. To add what platoon decides and why, turn on\n\
         \"Keep a diagnostics log\" in Settings (s), wait for the problem to happen again, then run platoon report again."
    };
    format!(
        "Wrote {path} ({kb} KB).\n\
         It holds platoon's versions, setup and settings, counts of your sessions and tasks, the sessions platoon runs,\n\
         the usage figures and the diagnostics log. It holds no conversation content (no prompts, replies, notes or names\n\
         you gave sessions), no command lines and no secrets. Read it before you send it.\n\
         {names}\n\
         {log}\n\
         To report a bug with it: https://github.com/ctrader2/platoon/issues/new/choose"
    )
}

/// The local time as part of a file name: `2026-10-28-0740`.
fn file_stamp(now: u64) -> String {
    let t = diag::local_time(now);
    if t.len() < 16 {
        return now.to_string();
    }
    // 2026-10-28T07:40:12-04:00
    format!("{}-{}{}", &t[..10], &t[11..13], &t[14..16])
}

// ---- the report ---------------------------------------------------------------------------------------------------

/// The whole report for `places` as it is at `now`.
pub fn make(places: &Places, probes: &Probes, hide: bool, now: u64) -> String {
    // The slow part first, all at once: each program's version.
    let asked = ask_versions(probes);
    let mut d = Doc::new(Names::new(hide, places.home.clone()));
    d.line(&[T("platoon report, made "), V(diag::at(now)), T(" by platoon "), V(diag::ident(crate::versions::running()))]);
    d.line(&[T("It holds no conversation content (prompts, replies, notes, task text, names given to sessions), no command")]);
    d.line(&[T("lines, no environment variable values and nothing from Claude Code's or Codex's own settings but platoon's")]);
    d.line(&[T("entries there.")]);
    d.line(&[T(if hide {
        "Project and folder names are replaced by placeholders."
    } else {
        "Project and folder names are shown as they are (platoon report --hide-names replaces them)."
    })]);
    let settings = crate::settings::load(&places.config);
    let providers = probes.installed.clone().switched(&settings.0.switched_off());
    let tasks = crate::tickets::every_ticket(&places.config);
    let hosts = crate::host::host_files(&places.cache.join("hosts"));
    versions(&mut d, probes, &providers, asked);
    folders(&mut d, places);
    setup(&mut d, places, probes);
    settings_section(&mut d, places, &settings);
    sessions(&mut d, places, &tasks);
    projects(&mut d, places, &tasks);
    running(&mut d, places, &hosts);
    usage(&mut d, places, probes, now);
    smart(&mut d, places, &settings.0, &providers, now);
    task_list(&mut d, places, probes, &tasks, &hosts, now);
    notes(&mut d, places);
    log(&mut d, places, settings.0.diagnostics_log);
    d.text()
}

/// What each installed program says its version is, asked all at once, each within `probes.wait`.
struct Asked {
    claude: Option<String>,
    codex: Option<String>,
    macos: Option<String>,
}

fn ask_versions(probes: &Probes) -> Asked {
    let wait = probes.wait;
    let ask = |program: Option<String>, args: &'static [&'static str], read: fn(&str) -> Option<String>| {
        std::thread::spawn(move || program.and_then(|p| crate::versions::output_of(&p, args, wait)).and_then(|o| read(&o)))
    };
    let claude = ask(probes.installed.claude.clone(), &["--version"], crate::versions::claude_version_in);
    let codex = ask(probes.installed.codex.as_ref().map(|c| c.program.clone()), &["--version"], crate::versions::codex_version_in);
    let sw_vers = cfg!(target_os = "macos").then(|| "sw_vers".to_string());
    let macos = ask(sw_vers, &["-productVersion"], |o| Some(o.trim().to_string()));
    Asked { claude: claude.join().ok().flatten(), codex: codex.join().ok().flatten(), macos: macos.join().ok().flatten() }
}

fn versions(d: &mut Doc, probes: &Probes, providers: &Providers, asked: Asked) {
    use crate::versions::Install;
    d.heading("Versions");
    let how = match probes.install {
        Install::Homebrew => "Homebrew",
        Install::Cargo => "cargo install",
        Install::Script => "the install script",
        Install::Other => "something else (from source, Nix, by hand)",
    };
    d.line(&[T("  platoon: "), V(diag::ident(crate::versions::running())), T(", installed with "), T(how)]);
    for (agent, name, there, said, checked) in [
        (Agent::Claude, "Claude Code", probes.installed.claude.is_some(), &asked.claude, crate::versions::checked_with()),
        (Agent::Codex, "Codex", probes.installed.codex.is_some(), &asked.codex, crate::versions::codex_checked_with()),
    ] {
        let off = if providers.off.contains(&agent) { " (switched off in platoon's settings)" } else { "" };
        match (there, said) {
            (false, _) => d.line(&[T("  "), T(name), T(": not installed")]),
            (true, Some(v)) => {
                d.line(&[T("  "), T(name), T(": "), V(diag::ident(v)), T(off), T(", platoon was checked with "), V(diag::ident(checked))])
            }
            (true, None) => d.line(&[T("  "), T(name), T(": installed, but its version couldn't be read"), T(off)]),
        }
    }
    let (sys, release, machine) = uname();
    d.line(&[T("  system: "), V(diag::ident(&sys)), T(" "), V(diag::ident(&release)), T(" "), V(diag::ident(&machine))]);
    if let Some(v) = &asked.macos {
        d.line(&[T("  macOS: "), V(diag::ident(v))]);
    }
    let env_word = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty()).map_or(Safe::Word("(not set)"), |v| diag::ident(&v));
    d.line(&[
        T("  terminal: TERM="),
        V(env_word("TERM")),
        T(" TERM_PROGRAM="),
        V(env_word("TERM_PROGRAM")),
        T(" TERM_PROGRAM_VERSION="),
        V(env_word("TERM_PROGRAM_VERSION")),
        T(" COLORTERM="),
        V(env_word("COLORTERM")),
    ]);
    match terminal_size() {
        Some((cols, rows)) => d.line(&[T("  terminal size: "), num(cols), T(" x "), num(rows)]),
        None => d.line(&[T("  terminal size: (no terminal)")]),
    }
    // Which of the variables that change what platoon does are set: their names, never their values.
    let mut line = vec![T("  set in the environment:")];
    let mut any = false;
    for k in ENV_NAMES {
        if std::env::var_os(k).is_some_and(|v| !v.is_empty()) {
            any = true;
            line.extend([T(" "), T(k)]);
        }
    }
    if !any {
        line.push(T(" none of platoon's"));
    }
    d.line(&line);
}

/// The environment variables that change what platoon (or what it reads) does.
const ENV_NAMES: &[&str] = &[
    "PLATOON_CACHE_DIR",
    "PLATOON_CONFIG_DIR",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "PLATOON_CLAUDE_BIN",
    "PLATOON_CODEX_BIN",
    "PLATOON_COLORS",
    "PLATOON_BADGES",
    "PLATOON_GLYPHS",
    "PLATOON_CLIPBOARD",
    "PLATOON_NO_UPDATE_CHECK",
    "PLATOON_SKIP_SETUP_CHECK",
    "NO_COLOR",
    "CLAUDE_CODE_DISABLE_1M_CONTEXT",
];

/// The system's name, release and machine, as the kernel says.
fn uname() -> (String, String, String) {
    // SAFETY: `uname` fills the struct we hand it; each field is a NUL-terminated string.
    unsafe {
        let mut u: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut u) != 0 {
            return Default::default();
        }
        let s = |f: &[libc::c_char]| std::ffi::CStr::from_ptr(f.as_ptr()).to_string_lossy().into_owned();
        (s(&u.sysname), s(&u.release), s(&u.machine))
    }
}

/// The terminal's size, from whichever of stdout, stderr and stdin is one; None without a terminal.
fn terminal_size() -> Option<(u16, u16)> {
    [1, 2, 0].into_iter().find_map(|fd| {
        // SAFETY: TIOCGWINSZ only writes into the `winsize` we hand it.
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        (unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } == 0 && ws.ws_col > 0).then_some((ws.ws_col, ws.ws_row))
    })
}

fn folders(d: &mut Doc, places: &Places) {
    d.heading("Folders");
    for (label, p) in [
        ("platoon's settings", &places.config),
        ("platoon's cache", &places.cache),
        ("Claude Code's", &places.claude_root),
        ("Codex's", &places.codex_root),
    ] {
        d.line(&[T("  "), T(label), T(": "), Place(p), T(if p.is_dir() { "" } else { " (not there)" })]);
    }
}

fn setup(d: &mut Doc, places: &Places, probes: &Probes) {
    d.heading("Setup");
    if probes.installed.claude.is_some() {
        for (step, state) in crate::setup::STEPS.iter().zip(crate::setup::check(&places.claude_root)) {
            d.line(&[T("  Claude Code, "), T(step.name), T(": "), T(state.word())]);
        }
        let word = |state| match state {
            Ok(crate::skill::State::Missing) => "missing",
            Ok(crate::skill::State::Ours(_)) => "platoon's, unchanged",
            Ok(crate::skill::State::NotOurs) => "not platoon's (left alone)",
            Err(_) => "can't be read",
        };
        d.line(&[T("  Claude Code, "), T(crate::skill::COMMAND), T(" skill: "), T(word(crate::skill::state(&places.claude_root)))]);
        // The skill under each old name, only when something is there (setup not run since the rename, or one kept).
        for old in crate::skill::OLD_COMMANDS {
            let state = crate::skill::old_state(&places.claude_root, old);
            if !matches!(state, Ok(crate::skill::State::Missing)) {
                d.line(&[T("  Claude Code, "), T(old), T(" skill (an old name): "), T(word(state))]);
            }
        }
    }
    if probes.installed.codex.is_some() {
        match crate::setup::check_codex(&places.codex_root) {
            Some(states) => {
                for (step, state) in crate::setup::CODEX_STEPS.iter().zip(states) {
                    d.line(&[T("  Codex, "), T(step.name), T(": "), T(state.word())]);
                }
            }
            None => d.line(&[T("  Codex: its hooks file can't be read (setup leaves it alone)")]),
        }
        d.line(&[T("  Codex's trust in platoon's hooks: not asked here (platoon setup asks Codex)")]);
    }
    if probes.installed.claude.is_none() && probes.installed.codex.is_none() {
        d.line(&[T("  neither Claude Code nor Codex is installed")]);
    }
}

/// platoon's own settings, the keys it knows: numbers, switches, and the models, editors and theme as identifiers.
fn settings_section(d: &mut Doc, places: &Places, (s, corrupt): &(crate::settings::Settings, bool)) {
    d.heading("Settings");
    if *corrupt {
        d.line(&[T("  the settings file can't be read: the defaults are in use")]);
    }
    d.kv("use Claude Code", s.use_claude_code);
    d.kv("use Codex", s.use_codex);
    let theme = if s.theme.starts_with("user:") { Safe::Word("(one of the user's own)") } else { diag::ident(&s.theme) };
    d.line(&[T("  theme: "), V(theme)]);
    d.kv("idle stop after (minutes, 0 = off)", s.idle_stop_minutes);
    d.kv("archive after (days, 0 = off)", s.auto_archive_days);
    d.kv("check for a newer platoon", s.update_check);
    d.kv("keep a diagnostics log", s.diagnostics_log);
    d.kv("smart features (master switch)", s.tokens_enabled);
    let model = |m: &Option<String>| m.as_deref().map_or(Safe::Word("(default)"), diag::ident);
    d.line(&[T("  model for smart features: "), V(model(&s.ai_model))]);
    d.line(&[T("  model for project notes: "), V(model(&s.notes_model))]);
    d.kv("daily token limit", s.ai_daily_tokens);
    d.kv("pause above plan percent (0 = never)", s.ai_pause_percent);
    d.kv("name archived sessions in the background", s.ai_archived);
    d.kv("archived share percent", s.ai_archived_share);
    d.kv("conversations per naming call", s.ai_batch_size);
    d.kv("refresh a name after messages", s.ai_refresh_messages);
    d.kv("quiet seconds before a refresh", s.ai_quiet_seconds);
    d.kv("notes quiet minutes", s.notes_quiet_minutes);
    d.kv("search archived", s.search_archived);
    d.kv("search results", s.search_results);
    d.line(&[T("  editor for notes and tasks: "), V(diag::ident(&s.notes_editor))]);
    let files = if s.files_editor.is_empty() { Safe::Word("(none chosen)") } else { diag::ident(&s.files_editor) };
    d.line(&[T("  editor for folders: "), V(files)]);
    d.kv("always ask which editor", s.files_editor_ask);
    let agent = s.new_session_agent.as_deref().map_or(Safe::Word("(not chosen yet)"), diag::ident);
    d.line(&[T("  new sessions start in: "), V(agent)]);
    let (keys, problems) = crate::keys::Keys::load(&places.config);
    d.line(&[
        T("  keys: back "),
        V(diag::ident(keys.back_label())),
        T(", quit "),
        V(diag::ident(keys.quit_label())),
        T(", problems in keys.json "),
        num(problems.len()),
    ]);
}

fn sessions(d: &mut Doc, places: &Places, tasks: &[(String, Ticket)]) {
    d.heading("Conversations (counts only)");
    let archive = crate::archive::Archive::open(&places.config);
    let names = crate::names::Names::open(&places.config);
    let summaries = crate::summaries::Summaries::open(&places.config);
    let task_sessions: Vec<&str> = tasks.iter().filter_map(|(_, t)| t.session_id()).collect();
    for (agent, label, root) in [(Agent::Claude, "Claude Code", &places.claude_root), (Agent::Codex, "Codex", &places.codex_root)] {
        let entries = crate::index::Scanner::load_for(agent, root, &places.cache).entries();
        let n = |f: &dyn Fn(&str) -> bool| entries.iter().filter(|e| f(&e.id)).count();
        let archived = n(&|id| archive.contains(id));
        let by_you = n(&|id| names.get(id).is_some());
        let by_platoon = n(&|id| names.get(id).is_none() && summaries.get(id).is_some());
        let of_tasks = n(&|id| task_sessions.contains(&id));
        d.line(&[
            T("  "),
            T(label),
            T(": "),
            num(entries.len()),
            T(" in platoon's index, "),
            num(archived),
            T(" archived, "),
            num(by_you),
            T(" named by you, "),
            num(by_platoon),
            T(" named by platoon, "),
            num(entries.len() - by_you - by_platoon),
            T(" not named, "),
            num(of_tasks),
            T(" tasks' sessions"),
        ]);
    }
    d.line(&[
        T("  archived by platoon's archive: "),
        num(archive.len()),
        T(if archive.is_corrupt() { " (its file can't be read)" } else { "" }),
    ]);
    if names.is_corrupt() {
        d.line(&[T("  the file of names you gave can't be read")]);
    }
}

fn projects(d: &mut Doc, places: &Places, tasks: &[(String, Ticket)]) {
    let projects = crate::projects::Projects::open(&places.config);
    d.heading("Projects");
    if projects.is_corrupt() {
        d.line(&[T("  projects.json can't be read")]);
    }
    if projects.all().is_empty() {
        d.line(&[T("  none")]);
    }
    for p in projects.all() {
        let notes = fs::metadata(projects.notes_path(&p.id)).map(|m| m.len()).ok();
        let n_tasks = tasks.iter().filter(|(id, _)| *id == p.id).count();
        d.line(&[
            T("  "),
            Project(&p.name),
            T(": "),
            T(if p.archived { "archived, " } else { "" }),
            num(projects.members_of(&p.id).len()),
            T(" sessions, "),
            num(n_tasks),
            T(" tasks, notes "),
            V(notes.map_or(Safe::Word("none"), Safe::from)),
            T(if notes.is_some() { " bytes" } else { "" }),
            T(", description "),
            yes_no(!p.description.is_empty()),
        ]);
    }
}

fn running(d: &mut Doc, places: &Places, hosts: &[(PathBuf, HostInfo, serde_json::Value)]) {
    d.heading("Sessions platoon runs");
    if hosts.is_empty() {
        d.line(&[T("  none")]);
        return;
    }
    let rows = crate::procs::table().unwrap_or_default();
    let age = |pid: u32| rows.iter().find(|r| r.pid == pid).and_then(|r| r.age);
    for (_, h, v) in hosts {
        let (host_alive, agent_alive) = (crate::procs::pid_alive(h.host_pid), crate::procs::pid_alive(h.claude_pid));
        let root = match h.agent {
            Agent::Claude => &places.claude_root,
            Agent::Codex => &places.codex_root,
        };
        let session = h.session();
        let version = h.version.as_deref().map_or(Safe::Word("(a platoon older than 1.1.0)"), diag::ident);
        d.line(&[
            T("  "),
            V(h.agent.into()),
            T(" session, pid "),
            num(h.claude_pid),
            T(" (host "),
            num(h.host_pid),
            T("), started by platoon "),
            V(version),
        ]);
        if !agent_alive || !host_alive {
            let gone = if agent_alive { "the host's process" } else { "the agent's process" };
            d.line(&[T("    GONE: "), T(gone), T(" isn't running; its file is left over")]);
        }
        d.line(&[T("    running for "), V(age(h.host_pid).into()), T(", in "), Place(Path::new(&h.dir))]);
        let task = h.task.as_deref().and_then(crate::tickets::parse_tag).map(|(_, n)| n);
        let windows = v.get("windows").and_then(serde_json::Value::as_u64).map_or(Safe::Word("(not said)"), Safe::from);
        d.line(&[
            T("    session "),
            V(session.as_deref().map_or(Safe::Word("(new: no id in its arguments)"), diag::session)),
            T(", task "),
            V(task.map_or(Safe::Word("none"), Safe::from)),
            T(", windows open "),
            V(windows),
        ]);
        let idle = agent_alive && h.agent.known_idle(root, h.claude_pid, session.as_deref());
        let idle_for = h.agent.idle_since(root, h.claude_pid, session.as_deref()).and_then(|t| t.elapsed().ok());
        d.line(&[T("    idle as the agent says: "), yes_no(idle), T(", for "), V(idle_for.into())]);
        if let Some(k) = &h.kept {
            let program = if let crate::idle::Kept::Command(Some(w)) = k { Program::named(w) } else { None };
            d.line(&[T("    kept running because: "), T(k.word()), T(" "), V(program.into())]);
        }
        let under = crate::procs::descendants(&rows, h.claude_pid).unwrap_or_default();
        if under.is_empty() {
            d.line(&[T("    running under it: nothing")]);
        }
        for pid in under {
            let argv = crate::procs::cmdline_quick(pid);
            let direct = rows.iter().any(|r| r.pid == pid && r.ppid == h.claude_pid);
            let command = direct && argv.as_deref().is_some_and(crate::idle::is_command);
            d.line(&[
                T("    under it: "),
                V(argv.as_deref().and_then(Program::of).map_or(Safe::Word("(can't be read)"), Safe::from)),
                T(", pid "),
                num(pid),
                T(", running for "),
                V(age(pid).into()),
                T(if command { ", a command the agent started" } else { "" }),
            ]);
        }
    }
}

fn usage(d: &mut Doc, places: &Places, probes: &Probes, now: u64) {
    d.heading("Plan usage");
    let saved = crate::usage::saved(&places.cache, now);
    if probes.installed.claude.is_some() || !saved.is_empty() {
        d.line(&[T("  Claude Code's status line reports (newest first): "), num(saved.len())]);
        for s in saved.iter().take(REPORTS_SHOWN) {
            let mut line = vec![T("    "), V(diag::session(&s.id)), T(" reported "), ago(s.ts, now), T(" ago:")];
            for (label, f) in [(" 5h ", s.five), (" 7d ", s.seven)] {
                line.push(T(label));
                match f {
                    Some((pct, r)) => {
                        line.extend([num(pct), T("% resets "), V(diag::at(r))]);
                        line.push(T(if r > now { " (running)" } else { " (over)" }));
                    }
                    None => line.push(T("not given")),
                }
            }
            d.line(&line);
        }
        match crate::usage::meter(&saved, now) {
            Some(p) => {
                let mut line = vec![T("  the meter shows:")];
                for (label, w) in [(" 5h ", p.five_hour), (" 7d ", p.seven_day)] {
                    line.push(T(label));
                    match w {
                        Some(w) if w.resets_at > 0 => line.extend([num(w.pct), T("% (resets in "), until(w.resets_at, now), T(")")]),
                        Some(w) => line.extend([num(w.pct), T("% (not started)")]),
                        None => line.push(T("n/a")),
                    }
                }
                d.line(&line);
            }
            None => d.line(&[T("  the meter shows: n/a (no report carries a plan's figures)")]),
        }
    }
    if probes.installed.codex.is_some() {
        match crate::agents::codex::rate_limits(&places.codex_root, now) {
            Some(limits) => {
                let mut line = vec![T("  Codex's limits:")];
                for l in limits {
                    line.extend([T(" "), num(l.minutes), T(" minutes "), num(l.pct), T("%")]);
                    line.push(T(if l.resets_at > now { " (running)" } else { " (over)" }));
                }
                d.line(&line);
            }
            None => d.line(&[T("  Codex's limits: none in its recent sessions")]),
        }
    }
}

fn smart(d: &mut Doc, places: &Places, s: &crate::settings::Settings, providers: &Providers, now: u64) {
    let catalog = providers.catalog();
    d.heading("Smart features");
    let mut line = vec![T("  switches: master "), yes_no(s.tokens_enabled)];
    for f in crate::ai::FEATURES {
        line.extend([T(", "), T(f.id), T(" "), yes_no(s.feature_toggle(f.id))]);
    }
    d.line(&line);
    let in_effect = |m: Option<String>| m.as_deref().map_or(Safe::Word("none here"), diag::ident);
    d.line(&[
        T("  models in effect: smart features "),
        V(in_effect(s.smart_model_in(&catalog))),
        T(", project notes "),
        V(in_effect(s.notes_model_in(&catalog))),
    ]);
    let data = crate::ledger::Ledger::open(&places.cache).load();
    for (agent, label) in [(Agent::Claude, "on Claude Code"), (Agent::Codex, "on Codex")] {
        let book = data.of(agent);
        if book.days.is_empty() && book.backoff.is_none() {
            d.line(&[T("  spent "), T(label), T(": nothing")]);
            continue;
        }
        d.line(&[T("  spent "), T(label), T(", the latest days (tokens, calls, failed calls; then by feature):")]);
        for (day, spent) in book.days.iter().rev().take(SPEND_DAYS) {
            let mut line =
                vec![T("    "), V(diag::ident(day)), T(": "), num(spent.tokens()), T(", "), num(spent.calls), T(", "), num(spent.failed)];
            let mut other = 0;
            for (id, f) in &spent.features {
                match known_feature(id) {
                    Some(id) => line.extend([T("; "), T(id), T(" "), num(f.tokens)]),
                    None => other += f.tokens,
                }
            }
            if other > 0 {
                line.extend([T("; other "), num(other)]);
            }
            d.line(&line);
        }
        if let Some(b) = book.backoff_at(now) {
            d.line(&[
                T("    backing off after "),
                num(b.failures),
                T(" failures ("),
                T(b.kind.name()),
                T("), for "),
                until(b.until, now),
                T(" more"),
            ]);
        }
    }
}

/// A feature id of the ledger's as platoon's own word, or None for one it doesn't know.
fn known_feature(id: &str) -> Option<&'static str> {
    crate::ai::FEATURES
        .iter()
        .map(|f| f.id)
        .chain(crate::ai::SWITCHLESS.iter().map(|(f, _)| *f))
        .chain(crate::ai::PART_OF_A_TASK.iter().map(|(f, _)| *f))
        .chain([crate::ledger::ARCHIVED_KEY])
        .find(|f| *f == id)
}

fn task_list(
    d: &mut Doc,
    places: &Places,
    probes: &Probes,
    tasks: &[(String, Ticket)],
    hosts: &[(PathBuf, HostInfo, serde_json::Value)],
    now: u64,
) {
    use crate::tickets::State;
    d.heading("Tasks (no titles, no text)");
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (_, t) in tasks {
        *counts.entry(t.state.id()).or_default() += 1;
    }
    let mut line = vec![T("  "), num(tasks.len()), T(" tasks")];
    for (state, n) in &counts {
        line.extend([T(", "), num(*n), T(" "), T(state)]);
    }
    d.line(&line);
    let projects = crate::projects::Projects::open(&places.config);
    let squads = crate::squads::all(&places.config);
    let catalog = probes.installed.catalog();
    for (project, t) in tasks {
        let need = if let State::NeedsYou(n) = &t.state { Some(n.id()) } else { None };
        let squad = squads.iter().find(|s| s.id == t.squad);
        let agent = match (t.session.as_deref(), squad) {
            (Some(s), _) if s.starts_with(crate::tickets::PENDING) => Some(Agent::Codex),
            (_, Some(s)) => catalog.agent_of(&s.as_run(&catalog).model),
            _ => None,
        };
        let tag = crate::tickets::task_tag(project, t.number);
        let runs = hosts.iter().any(|(_, h, _)| {
            let ours = h.task.as_deref() == Some(tag.as_str()) || (t.session_id().is_some() && h.session().as_deref() == t.session_id());
            ours && crate::procs::pid_alive(h.claude_pid)
        });
        let worktree = match &t.worktree {
            None => "none",
            Some(w) if w.removed.is_some() => "removed after the work",
            Some(w) if w.kept_uncommitted => "kept: changes not committed",
            Some(w) if w.opened_in.is_some() => "kept: opened in an editor",
            Some(w) if !Path::new(&w.path).exists() => "missing",
            Some(_) => "there",
        };
        let mut line = vec![T("  #"), num(t.number), T(" in ")];
        match projects.get(project) {
            Some(p) => line.push(Project(&p.name)),
            None => line.push(T(if project.is_empty() { "no project" } else { "(a project that is gone)" })),
        }
        line.extend([T(": "), T(t.state.id())]);
        if let Some(n) = need {
            line.extend([T(" ("), T(n), T(")")]);
        }
        line.extend([T(", squad "), V(if t.squad.is_empty() { Safe::Word("none") } else { diag::ident(&t.squad) })]);
        line.extend([T(", "), V(agent.into()), T(", session running "), yes_no(runs), T(", worktree "), T(worktree)]);
        if t.started > 0 {
            line.extend([T(", started "), ago(t.started, now), T(" ago")]);
        }
        let done = t.steps.list.iter().filter(|s| s.mark.finished()).count();
        line.extend([T(", steps "), num(done), T("/"), num(t.steps.list.len()), T(", checks refused "), num(t.checks_refused)]);
        d.line(&line);
        if let Some(w) = &t.worktree {
            d.line(&[T("    worktree "), Place(Path::new(&w.path))]);
        }
    }
}

fn notes(d: &mut Doc, places: &Places) {
    d.heading("Notes updates");
    let count = |dir: &str| fs::read_dir(places.cache.join(dir)).map(|r| r.flatten().count()).unwrap_or(0);
    d.line(&[
        T("  waiting "),
        num(count("notes-due")),
        T(", updating "),
        num(count("notes-updating")),
        T(", being edited "),
        num(count("notes-editing")),
        T(", helpers "),
        num(count("notes-helper")),
    ]);
}

fn log(d: &mut Doc, places: &Places, on: bool) {
    d.heading("Diagnostics log");
    d.line(&[T(if on {
        "  on: decisions and errors"
    } else {
        "  off: errors only (Settings, \"Keep a diagnostics log\", adds what platoon decides and why)"
    })]);
    let lines = diag::tail(&places.cache, LOG_LINES);
    if lines.is_empty() {
        d.line(&[T("  (nothing written yet)")]);
    }
    d.log_lines(lines);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_shown_from_home_or_hidden_the_same_way_everywhere() {
        let home = PathBuf::from("/home/someone");
        let mut shown = Names::new(false, Some(home.clone()));
        assert_eq!(shown.place(Path::new("/home/someone/repos/acme")), "~/repos/acme");
        assert_eq!(shown.place(Path::new("/srv/acme")), "/srv/acme");
        assert_eq!(shown.project("Acme site"), "Acme site");

        let mut hidden = Names::new(true, Some(home));
        assert_eq!(hidden.place(Path::new("/home/someone/repos/acme")), "~/folder-1/folder-2");
        assert_eq!(hidden.place(Path::new("/home/someone/repos/acme-worktrees/task-3")), "~/folder-1/folder-3/task-3");
        assert_eq!(hidden.place(Path::new("/srv/acme")), "/folder-4/folder-2", "the same name, the same placeholder");
        assert_eq!(hidden.place(Path::new("/home/someone/.cache/platoon")), "~/.cache/platoon", "platoon's own folders stay");
        assert_eq!(hidden.place(Path::new("/home/someone")), "~");
        let projects = [hidden.project("Acme site"), hidden.project("Other"), hidden.project("Acme site")];
        assert_eq!(projects, ["project-1", "project-2", "project-1"]);
    }

    #[test]
    fn the_arguments() {
        let a = |v: &[&str]| parse_args(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(a(&[]), Ok(Some(Options::default())));
        assert_eq!(a(&["--hide-names", "--stdout"]), Ok(Some(Options { hide_names: true, stdout: true, out: None })));
        assert_eq!(a(&["--out", "r.txt"]), Ok(Some(Options { out: Some("r.txt".into()), ..Default::default() })));
        assert_eq!(a(&["--help"]), Ok(None));
        assert!(a(&["--out"]).is_err() && a(&["--frobnicate"]).is_err() && a(&["--out", "x", "--stdout"]).is_err());
    }

    #[test]
    fn the_summary_says_where_what_and_how_to_hide_names() {
        let s = summary("~/platoon-report-2026-10-28-0740.txt", 5000, false, false);
        assert!(s.starts_with("Wrote ~/platoon-report-2026-10-28-0740.txt (5 KB)."), "{s}");
        assert!(s.contains("no conversation content") && s.contains("no secrets") && s.contains("--hide-names"), "{s}");
        assert!(s.contains("Keep a diagnostics log"), "{s}");
        let s = summary("x", 10, true, true);
        assert!(s.contains("replaced by placeholders") && !s.contains("--hide-names") && s.contains("is on"), "{s}");
    }

    #[test]
    fn a_file_stamp_is_the_local_date_and_minute() {
        let s = file_stamp(1_793_186_731);
        assert_eq!(s.len(), 15, "{s}");
        assert!(s.starts_with("2026-10-2") && s.as_bytes()[10] == b'-', "{s}");
    }
}
