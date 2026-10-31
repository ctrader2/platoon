mod agents;
mod ai;
mod app;
mod archive;
mod askmatch;
mod board;
#[cfg(test)]
mod clarity_app_tests;
mod client;
mod clipboard;
#[cfg(test)]
mod codex_app_tests;
#[cfg(test)]
mod codex_tasks_app_tests;
mod colordepth;
#[cfg(test)]
mod contrast_app_tests;
#[cfg(test)]
mod density_app_tests;
mod describe;
mod diag;
mod dirs;
#[cfg(test)]
mod draft_app_tests;
mod editor;
#[cfg(test)]
mod editor_app_tests;
mod editors;
mod fileeditors;
#[cfg(test)]
mod frame_app_tests;
mod frames;
mod gitfacts;
mod guard;
mod handoff;
#[cfg(test)]
mod handoff_app_tests;
mod host;
mod idle;
#[cfg(test)]
mod idle_app_tests;
mod index;
mod instructions;
mod keys;
#[cfg(test)]
mod keys_app_tests;
#[cfg(test)]
mod layout_app_tests;
mod ledger;
#[cfg(test)]
mod light_theme_tests;
mod mcp;
mod mdview;
mod model;
mod names;
mod notes;
#[cfg(test)]
mod notes_app_tests;
mod notesrun;
mod notesupdate;
mod open;
#[cfg(test)]
mod perf;
#[cfg(test)]
mod polish_app_tests;
mod procs;
#[cfg(test)]
mod programs_app_tests;
mod projects;
#[cfg(test)]
mod projects_app_tests;
mod prompts;
mod report;
mod scan;
#[cfg(test)]
mod search_app_tests;
mod select;
#[cfg(test)]
mod select_app_tests;
mod sessionhook;
mod settings;
mod setup;
#[cfg(test)]
mod setup_notice_app_tests;
mod setupnotice;
mod skill;
mod smart;
#[cfg(test)]
mod smart_app_tests;
mod spent;
#[cfg(test)]
mod squad_settings_app_tests;
mod squads;
#[cfg(test)]
mod squads_app_tests;
mod steps;
#[cfg(test)]
mod stop_app_tests;
mod summaries;
mod taskcommit;
mod taskdraft;
mod taskrun;
#[cfg(test)]
mod tasks_app_tests;
#[cfg(test)]
mod testkit;
mod textbox;
mod theme;
mod tickets;
mod ticketshint;
#[cfg(test)]
mod tokens_app_tests;
mod ui;
mod uninstall;
mod usage;
mod usertheme;
mod verdict;
mod versions;
#[cfg(test)]
mod versions_app_tests;
#[cfg(test)]
mod wheel_app_tests;
#[cfg(test)]
mod worktree_app_tests;
mod worktrees;

use ratatui::crossterm::{
    cursor::Show,
    event::{
        self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste, EnableFocusChange, EnableMouseCapture,
        Event, KeyEventKind,
    },
    execute,
    terminal::{Clear, ClearType, EnterAlternateScreen, enable_raw_mode},
};
use std::{
    io,
    time::{Duration, Instant},
};

fn main() -> io::Result<()> {
    start_log(std::env::args().nth(1).as_deref());
    match std::env::args().nth(1).as_deref() {
        None => {}
        Some("--version" | "-V") => {
            println!("{}", version_line());
            return Ok(());
        }
        Some("statusline") => return statusline(),
        Some("setup") if std::env::args().nth(2).is_some() => {
            eprintln!("usage: platoon setup");
            std::process::exit(2);
        }
        Some("setup") => return setup(),
        Some("themes") => return themes(),
        Some("tokens") => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            let (mut out, mut err) = (io::stdout(), io::stderr());
            let providers = agents::Providers::here(&archive::config_dir());
            std::process::exit(ai::tokens_cli(&args, &usage::cache_dir(), &archive::config_dir(), &providers, &mut out, &mut err));
        }
        Some("report") => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            std::process::exit(report::cli(&args, &mut io::stdout(), &mut io::stderr()));
        }
        Some("open") => {
            require_setup();
            let args: Vec<String> = std::env::args().skip(2).collect();
            std::process::exit(open::run(&args));
        }
        // Claude Code's hooks name no agent; Codex's say `--agent codex`.
        Some("hook") if std::env::args().nth(2).as_deref() == Some("guard") => timed_hook("guard", guard::run_guard),
        Some("hook") if std::env::args().nth(2).as_deref() == Some("session-start") => timed_hook("session-start", sessionhook::run),
        Some("hook") if std::env::args().nth(2).as_deref() == Some("stop") => timed_hook("stop", notesrun::run_hook),
        // Kept as a silent no-op: Claude Code sessions an older platoon started (1.0.x) carry a `--settings` hook that runs
        // `hook offer-tasks` before their first edit, and must not see it fail after an upgrade. The offer to make tasks is
        // the /to-tasks skill now (`skill.rs`).
        Some("hook") if std::env::args().nth(2).as_deref() == Some("offer-tasks") => {
            use std::io::IsTerminal;
            if !io::stdin().is_terminal() {
                let _ = io::Read::read_to_end(&mut io::Read::take(io::stdin(), 1 << 20), &mut Vec::new());
            }
            std::process::exit(0)
        }
        // Claude Code's are given only to a task's session, with `--settings` (never installed in the user's own
        // settings); Codex's are in its hooks file for every session and act only in a task's (`PLATOON_TASK`).
        Some("hook") if task_hook().is_some() => {
            let name = task_hook().unwrap_or_default();
            timed_hook(name, |agent| taskrun::run_hook(name, agent))
        }
        Some("notes-update") => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            std::process::exit(notesrun::run_helper(&args));
        }
        Some("host") => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            std::process::exit(host::run(&args));
        }
        Some("uninstall") => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            std::process::exit(uninstall_cli(&args));
        }
        Some("task") => {
            use std::io::IsTerminal;
            let args: Vec<String> = std::env::args().skip(2).collect();
            let cwd = std::env::current_dir().unwrap_or_default();
            let (mut out, mut err) = (io::stdout(), io::stderr());
            let terminal = io::stdin().is_terminal();
            let (claude_root, cache, config) = (agents::claude::claude_root(), usage::cache_dir(), archive::config_dir());
            let roots = tickets::Roots { claude_root: &claude_root, cache: &cache };
            let task = std::env::var(agents::TASK_ENV).ok();
            std::process::exit(tickets::cli(
                &args,
                &config,
                task.as_deref(),
                Some(roots),
                &mut io::stdin().lock(),
                terminal,
                &cwd,
                &mut out,
                &mut err,
            ));
        }
        Some(_) => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    }
    require_setup();
    // ratatui sets the terminal up (raw mode, the alternate screen, and a panic hook that puts it back); frames are
    // written by `frames::Writer`, not by its `Terminal`.
    if let Err(e) = ratatui::try_init() {
        eprintln!("platoon needs an interactive terminal ({e}).");
        std::process::exit(1);
    }
    let put_back = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        stop_asking_the_terminal();
        put_back(info);
    }));
    ask_the_terminal();
    let result = run();
    stop_asking_the_terminal();
    let _ = execute!(io::stdout(), Show);
    ratatui::restore();
    if let Some(line) = quit_line(result?) {
        println!("{line}");
    }
    Ok(())
}

/// What platoon asks of the terminal while its screen shows: the mouse, a paste as one piece, and a word when the window
/// gains or loses focus (it comes back to the front: everything is drawn again, whatever the terminal skipped).
fn ask_the_terminal() {
    let _ = execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste, EnableFocusChange);
}

/// Undo `ask_the_terminal`: before platoon leaves the screen (quitting, a panic, an editor taking the terminal).
fn stop_asking_the_terminal() {
    let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste, DisableFocusChange);
}

/// Start this process's diagnostics log (`diag.rs`), named after what it is, and have it record a panic. Not for what
/// does nothing worth a line.
fn start_log(command: Option<&str>) {
    let kind = match command {
        None => "app",
        Some("host") => "host",
        Some("hook") => "hook",
        Some("statusline") => "statusline",
        Some("notes-update") => "notes",
        Some("setup") => "setup",
        Some("open") => "open",
        Some("task") => "task",
        Some("tokens") => "tokens",
        // Not `report`, which only reads, and `uninstall`, which deletes the cache the log is in.
        Some(_) => return,
    };
    diag::init(kind, &usage::cache_dir(), &archive::config_dir());
    diag::catch_panics();
}

/// Run hook `name` for the agent it is called for, note how long it took and how it ended, and exit with its code. The
/// two that run around every tool call of a task's session are noted only when slow or failing, so they don't fill the
/// log.
fn timed_hook(name: &'static str, run: impl FnOnce(agents::Agent) -> i32) -> ! {
    const EVERY_TOOL: [&str; 2] = ["task-before", "task-tool"];
    let (agent, start) = (hook_agent(), Instant::now());
    let code = run(agent);
    let took = start.elapsed();
    if !EVERY_TOOL.contains(&name) || code != 0 || took >= Duration::from_millis(250) {
        diag::decision("hook", "ran", &[("hook", name.into()), ("agent", agent.into()), ("took", took.into()), ("exit", code.into())]);
    }
    std::process::exit(code)
}

/// The task hook `platoon hook <name>` names (`taskrun::HOOKS`), if it names one.
fn task_hook() -> Option<&'static str> {
    std::env::args().nth(2).and_then(|h| taskrun::HOOKS.iter().copied().find(|k| *k == h))
}

/// The agent a `platoon hook <kind> [--agent <id>]` is for.
fn hook_agent() -> agents::Agent {
    agents::agent_arg(&std::env::args().skip(3).collect::<Vec<_>>())
}

/// What quitting says about the sessions platoon's hosts keep running: in the list they read `● idle` (or working),
/// and the next platoon picks them up. Nothing when none runs.
fn quit_line(running: usize) -> Option<String> {
    match running {
        0 => None,
        1 => Some("1 session keeps running: platoon picks it up next time.".into()),
        n => Some(format!("{n} sessions keep running: platoon picks them up next time.")),
    }
}

/// What `platoon --version` prints.
fn version_line() -> String {
    format!("platoon {}", versions::running())
}

const USAGE: &str = "usage: platoon [setup | open [id] | open --project <name> | task add | task list | task squads | task step |
               themes | tokens | report | uninstall | --version]
  (no arguments)  open the session manager
  open [id]       show a session live in this terminal (starts or resumes it if needed);
                  with no id, start a new one here. The back key (Ctrl+Space unless
                  keys.json says otherwise) detaches; it keeps running
  open --project <name>   start a new session here, in that project
  setup           add platoon to Claude Code: plan usage in the status line, a guard that
                  stops two copies of one session from overwriting each other, two hooks
                  that hand a project's notes to its sessions and keep them up to date, and
                  the /to-tasks skill; and, when Codex is installed, the notes and task
                  hooks to Codex's hooks.json
  task add [--project <name>] --title <text> [--body-file <path>] [--folder <path>] [--squad <id>]
                  make a task and print its number (#N); with no --folder it works in the
                  current folder; --squad suggests the squad that starts it
  task list [--project <name>] [--json]   the tasks, open ones first
  task squads     the squads a task can start with: each one's id and what it is for
  task step <n> started|done|skipped [\"<one line>\"]   inside a task's session: mark a step of its
                  squad (skipped needs a reason)
  themes          list the color themes: the built-in ones and your own
  tokens [status | test]   show whether the smart features are on and how many tokens they used
                  today; `test` makes one tiny call on their model (Claude's or Codex's)
  report [--hide-names] [--out <file> | --stdout]   write what a bug report needs into one plain-text
                  file you can read first: versions, setup, settings, counts of your sessions and
                  tasks, the sessions platoon runs, the usage figures and the diagnostics log; no
                  conversation content and no secrets (--hide-names: no project or folder names)
  uninstall [--all] [--stop-sessions]   take platoon's settings and hooks out of Claude Code and
                  Codex (and Codex's trust in them) and delete its cache;
                  asks before deleting your projects, notes, tasks and settings (--all: without
                  asking) and before stopping the sessions it runs (--stop-sessions: without asking)
  --version, -V   print platoon's version
  statusline, hook guard, hook session-start, hook stop, hook task-plan, hook task-question,
                  hook task-notify, hook task-tool, hook task-prompt, hook task-stop, hook task-start,
                  hook task-before, notes-update   internal: run by Claude Code (and by Codex, with
                  --agent codex: hook guard, hook session-start, hook stop and the task hooks),
                  or by platoon itself";

/// Run by Claude Code as its status line command. Must be quick and must never fail loudly. Every session's usage
/// figures are recorded; a task's session (its task in its environment) shows the task's line, the figures at its end.
/// Any other session's figures end with the `/to-tasks` hint while its conversation has agreed on work
/// (`ticketshint.rs`); a task's session never gets it, whichever line it shows.
fn statusline() -> io::Result<()> {
    let mut input = String::new();
    io::Read::read_to_string(&mut io::stdin(), &mut input)?;
    let now = usage::now();
    let (config, cache) = (archive::config_dir(), usage::cache_dir());
    let figures = usage::record(&input, &cache, now);
    let line = if std::env::var_os(agents::TASK_ENV).is_some_and(|t| !t.is_empty()) {
        taskrun::status_line(&input, &config, &figures, status_width(), now).unwrap_or(figures)
    } else {
        let root = agents::claude::claude_root();
        let at = ticketshint::Places { config: &config, cache: &cache, claude_root: &root };
        match ticketshint::wanted(&input, &at, now) {
            Some(command) => ticketshint::with_hint(&figures, command, status_width()),
            None => figures,
        }
    };
    println!("{line}");
    Ok(())
}

/// How wide a status line may be: the terminal's columns when the environment says (`COLUMNS`), else 120.
fn status_width() -> usize {
    std::env::var("COLUMNS").ok().and_then(|c| c.trim().parse::<usize>().ok()).filter(|c| *c >= 20).map_or(120, |c| c - 1)
}

/// Refuse to continue until setup is complete. Only the UI and `open` are gated; the commands Claude
/// Code and the session hosts run behind the scenes never are. PLATOON_SKIP_SETUP_CHECK is for tests and CI.
/// Claude Code's steps are asked for only where Claude Code is installed and not switched off in platoon's settings
/// (Codex's never block); with neither Claude Code nor Codex there is nothing to manage, and platoon says what to install.
fn require_setup() {
    if std::env::var_os("PLATOON_SKIP_SETUP_CHECK").is_some() {
        return;
    }
    let providers = agents::Providers::here(&archive::config_dir());
    if providers.claude.is_none() && providers.codex.is_none() {
        eprintln!("{}", setup::NOTHING_INSTALLED);
        std::process::exit(1);
    }
    if providers.claude.is_some() {
        let states = setup::check(&agents::claude::claude_root());
        if !setup::ready(&states) {
            eprintln!("{}", setup::render_blocked(&states, use_color(true)));
            std::process::exit(1);
        }
    }
}

fn use_color(stderr: bool) -> bool {
    use std::io::IsTerminal;
    std::env::var_os("NO_COLOR").is_none() && if stderr { io::stderr().is_terminal() } else { io::stdout().is_terminal() }
}

/// `platoon themes`: the built-in themes and the user's own with their ids, with a swatch of each when stdout shows
/// colors, and the theme files that were skipped, with the reason.
fn themes() -> io::Result<()> {
    let depth = use_color(false).then(colordepth::Depth::detect);
    let registry = usertheme::Registry::open(&archive::config_dir());
    print!("{}", theme::render_list(depth, &registry));
    Ok(())
}

fn setup() -> io::Result<()> {
    let root = agents::claude::claude_root();
    // A program switched off in platoon's settings is skipped as if it weren't installed: what setup put in it before
    // stays as it is (taking it out is `platoon uninstall`'s job), and the report says why it was skipped.
    let providers = agents::Providers::here(&archive::config_dir());
    if providers.claude.is_none() && providers.codex.is_none() {
        eprintln!("{}", setup::NOTHING_INSTALLED);
        std::process::exit(1);
    }
    // Claude Code's steps only where Claude Code is installed: its settings are left alone otherwise.
    let claude = if providers.claude.is_some() { setup::run(&root) } else { Ok(setup::Report::without_claude()) };
    match claude {
        Ok(mut report) => {
            report.off.clone_from(&providers.off);
            // The /to-tasks skill, where Claude Code is: not part of the start-up gate, so it never blocks platoon. Then
            // platoon's copy under its old name, /to-tickets, goes once the new one is in.
            if providers.claude.is_some() {
                let exe = setup::this_program().map(|(exe, _)| exe);
                report.skill = Some(exe.map_or_else(skill::Installed::Failed, |exe| skill::install(&root, &exe)));
                report.old_skill = Some(skill::retire_old(&root));
            }
            // Codex's hooks, when Codex is on this computer. They never block anything, Claude Code's steps included.
            if let Some(agents::CodexAt { program, root: codex }) = &providers.codex {
                let mut c = setup::run_codex(codex);
                // Running setup is the user's say-so: Codex itself is asked to trust the hooks setup wrote.
                if c.error.is_none() {
                    let home = std::env::var_os("HOME").filter(|h| !h.is_empty()).map_or_else(|| codex.clone(), std::path::PathBuf::from);
                    c.trust = Some(setup::trust_codex(codex, program, &home, agents::codex_server::TIME_BOX));
                }
                report.codex = Some(c);
            }
            // A Claude Code or Codex newer than the one this platoon was checked with is said here every time (each where it
            // is installed).
            let claude_program = providers.claude.as_ref().and(versions::claude_program());
            report.notes.extend(versions::setup_line(claude_program.as_deref(), versions::off_by_env()));
            let codex_program = providers.codex.as_ref().map(|c| c.program.as_str());
            report.notes.extend(versions::codex_setup_line(codex_program, versions::off_by_env()));
            setup::log_report(&report);
            println!("{}", setup::render_report(&report, &root, use_color(false)));
        }
        Err(e) => {
            diag::error("setup", "failed", &[]);
            eprintln!("platoon setup failed: {e}");
            std::process::exit(1);
        }
    }
    // The one extra question. It never changes what setup did or the exit code.
    use std::io::IsTerminal;
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    let agent = ai::smart_agent(&settings::load(&archive::config_dir()).0, &providers.catalog());
    let answer = setup::ask_about_tokens(&archive::config_dir(), interactive, agent, &mut io::stdin().lock(), &mut io::stdout());
    // Yes now, or yes before and nothing named yet: the first pass over the active conversations, right here.
    // (It reads Claude Code's conversations: none to read where Claude Code isn't installed or is switched off.)
    if providers.claude.is_some() && matches!(answer, setup::TokenQuestion::Answered(true) | setup::TokenQuestion::AlreadyAsked) {
        let (config, cache) = (archive::config_dir(), usage::cache_dir());
        let env = scan::ScanEnv { config: &config, cache: &cache, root: &root, providers, interactive };
        scan::offer_scan(&env, &mut io::stdin().lock(), &mut io::stdout(), &ai::interrupted);
    }
    Ok(())
}

/// `platoon uninstall [--all] [--stop-sessions]`: see `uninstall.rs`. Answers come from the terminal itself
/// (`/dev/tty`), so it can still ask when the uninstall script was piped into `sh`.
fn uninstall_cli(args: &[String]) -> i32 {
    let opts = match uninstall::parse_args(args) {
        Ok(Some(o)) => o,
        Ok(None) => {
            println!("{}", uninstall::USAGE);
            return 0;
        }
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let places = uninstall::Places::from_env();
    let test_home = std::env::var_os(uninstall::TEST_HOME_ENV).filter(|v| !v.is_empty()).map(std::path::PathBuf::from);
    if let Err(e) = uninstall::check_test_home(&places, test_home.as_deref()) {
        eprintln!("{e}");
        return 1;
    }
    let tty = std::env::var_os(uninstall::TTY_ENV).map_or_else(|| std::path::PathBuf::from("/dev/tty"), std::path::PathBuf::from);
    let mut input = std::fs::File::open(tty).ok().map(io::BufReader::new);
    let program = if std::env::var_os(uninstall::FROM_SCRIPT_ENV).is_some() {
        None
    } else {
        let exe = std::env::current_exe().ok().and_then(|e| std::fs::canonicalize(e).ok()).unwrap_or_default();
        Some(uninstall::program_line(&versions::install_here(), &exe))
    };
    let mut stop = |pids: &[u32]| host::end_processes(pids);
    uninstall::run(&places, &opts, input.as_mut().map(|i| i as &mut dyn io::BufRead), &mut io::stdout(), &mut stop, "git", program)
}

/// Runs the UI. Returns how many platoon-hosted sessions are still running when it exits.
fn run() -> io::Result<usize> {
    let mut app = app::App::new();
    let mut screen = Screen::new(frames::Writer::spawn(io::stdout()));
    // A copy to the terminal's clipboard goes with the frames, so it neither waits for the terminal nor lands in a frame.
    app.set_clipboard(clipboard::Clipboard::from_env(screen.out.bytes()));
    let depth = colordepth::Depth::detect();
    // NO_COLOR: platoon takes the colors out itself (`colordepth::strip`), so bold and underline survive.
    let no_color = colordepth::no_color();
    if no_color {
        ratatui::crossterm::style::force_color_output(true);
    }
    let start = Instant::now();
    let mut last_refresh = Instant::now();
    while !app.quit {
        if let Some((e, dirs)) = app.take_own_editor_request() {
            open_own_editor(&mut screen, &mut app, &e, &dirs);
        }
        app.poll_stops();
        app.poll_editors();
        let tick = (start.elapsed().as_millis() / 100) as u64;
        // A spinner or a pulse moves on with the tick: only then does a new tick need a new frame.
        if tick != app.tick && app.animating() {
            screen.dirty = true;
        }
        app.tick = tick;
        // While the index is still reading transcripts, pick up its results more often so rows fill in smoothly
        // (and, likewise, a settings change that is about to take effect).
        let every = if app.indexing().is_some() || app.settling() || app.smart_busy() || app.handoff_busy() || app.squads_busy() {
            Duration::from_millis(250)
        } else {
            Duration::from_secs(1)
        };
        if last_refresh.elapsed() >= every {
            app.refresh();
            last_refresh = Instant::now();
            // Anything may have changed (sessions, tickets, usage, the time a row says); the frame says whether it did.
            screen.dirty = true;
        }
        // What the session asked to copy, before a connection that closed is replaced by a new one.
        app.take_session_copy();
        // A session's host may have closed its connection: connect again (or leave the session) before drawing.
        app.keep_attached();
        // A selection dragged past the top or bottom of a session keeps scrolling while the mouse stands still.
        app.drag_scroll(Instant::now());
        // A notice that appears or runs out changes the footer. (One given by the work above belongs to this screen.)
        app.settle_notice();
        let notice = app.active_notice().map(str::to_string);
        if notice != screen.notice {
            screen.notice = notice;
            screen.dirty = true;
        }
        // A session's screen (or a notes editor's, running inside platoon) changes whenever its program writes, which
        // this loop isn't told about.
        if matches!(app.mode, app::Mode::Attached(_) | app::Mode::EditNotes) {
            screen.dirty = true;
        }
        // Asked of the terminal's device, never written to it: a terminal that reads nothing doesn't hold this up.
        let (cols, rows) = ratatui::crossterm::terminal::size()?;
        let size = ratatui::layout::Size::new(cols, rows);
        if screen.shown.as_ref().is_none_or(|f| f.cells.area.as_size() != size) {
            screen.dirty = true;
        }
        app.sync_size(size.width, size.height);
        if screen.dirty {
            screen.dirty = false;
            screen.show(size, |f| {
                ui::draw(f, &app);
                if no_color {
                    colordepth::strip(f.buffer_mut());
                } else {
                    depth.apply(f.buffer_mut());
                }
            })?;
        }
        if event::poll(Duration::from_millis(50))? {
            on_event(&mut app, &mut screen, event::read()?);
        }
    }
    // Stops still running for the archive key get a few seconds to finish, so their records are written.
    app.finish_stops();
    // The last frame is written before the terminal is put back.
    screen.out.finish()?;
    Ok(app.hosts.len())
}

/// Hand what came in from the terminal to the app.
fn on_event(app: &mut app::App, screen: &mut Screen, event: Event) {
    // Whatever came in (a key, the mouse, a paste, a new size) may change what is on the screen.
    screen.dirty = true;
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
        Event::Mouse(m) => app.on_mouse(m),
        Event::Paste(text) => app.on_paste(&text),
        // The window is back in front: every cell is drawn again, whatever the terminal skipped or lost while it was
        // behind another one. (Never handed to a session: it is platoon's, not something the user typed.)
        Event::FocusGained => screen.forget(),
        _ => {}
    }
}

/// What is on the terminal, so a frame is only sent to it when it differs. The loop marks the screen `dirty` when
/// something may have changed (an event, a refresh, a notice, a spinner's tick, a session's output); the frame is then
/// drawn off screen first and handed to the writer only if it is not the one already showing. An idle platoon writes
/// nothing at all, and a terminal that reads nothing never holds the loop up (`frames::Writer`).
struct Screen {
    /// Something may have changed since the last frame: draw one (the first frame is always drawn).
    dirty: bool,
    /// The frame showing (handed to the writer last). None: unknown (draw in full).
    shown: Option<frames::Frame>,
    /// The notice the footer showed in that frame.
    notice: Option<String>,
    /// Where frames are drawn before they are compared.
    off: Option<ratatui::Terminal<ratatui::backend::TestBackend>>,
    /// The terminal's writer.
    out: frames::Writer,
}

impl Screen {
    fn new(out: frames::Writer) -> Self {
        Self { dirty: true, shown: None, notice: None, off: None, out }
    }

    /// Forget what is showing (another program had the terminal, or its window was behind another one): the next frame
    /// is drawn in full, every cell.
    fn forget(&mut self) {
        self.shown = None;
        self.dirty = true;
        self.out.repaint();
    }

    /// Draw a frame of `size` with `render`, off screen, and hand it to the writer if it is not what is showing already.
    /// An error: the writer could not write to the terminal (it has gone).
    fn show(&mut self, size: ratatui::layout::Size, render: impl Fn(&mut ratatui::Frame)) -> io::Result<()> {
        self.out.check()?;
        let off = match &mut self.off {
            Some(t) if t.backend().buffer().area.as_size() == size => t,
            slot => {
                let Ok(t) = ratatui::Terminal::new(ratatui::backend::TestBackend::new(size.width, size.height));
                slot.insert(t)
            }
        };
        let Ok(_) = off.draw(|f| render(f));
        let backend = off.backend();
        let frame = frames::Frame { cells: backend.buffer().clone(), cursor: backend.cursor_visible().then(|| backend.cursor_position()) };
        if self.shown.as_ref() == Some(&frame) {
            return Ok(());
        }
        self.out.send(frame.clone());
        self.shown = Some(frame);
        Ok(())
    }
}

/// Hand the terminal to an editor of the user's own that draws in the terminal (Helix, their Neovim setup), on the
/// first folder, wait for it to close, and take the terminal back: platoon's screen goes (raw mode off, the alternate
/// screen left) so the editor draws on the real terminal, then comes back drawn in full, where the user was. Nothing
/// of platoon reads the keyboard or draws in between (this loop is waiting).
fn open_own_editor(screen: &mut Screen, app: &mut app::App, e: &fileeditors::FileEditor, dirs: &[String]) {
    let env = |k: &str| std::env::var(k).ok();
    let Some(first) = dirs.first() else { return };
    let launch = match fileeditors::terminal_launch(e, first, &env) {
        Ok(l) => l,
        Err(err) => return app.editor_closed(Err(err.to_string())),
    };
    let result = hand_over(screen, app, &launch, std::slice::from_ref(first));
    app.editor_closed(result);
}

/// Leave platoon's screen, run the editor in the foreground until it closes, and take the screen back, drawn in full.
/// Meanwhile `dirs` are marked as open, so no platoon window removes a worktree from under it.
fn hand_over(screen: &mut Screen, app: &mut app::App, launch: &editor::Launch, dirs: &[String]) -> Result<(), String> {
    // Other platoon windows must not remove a worktree the editor has open (`worktree_glue`).
    let mark = editor::mark_open(&app.cache, std::process::id(), dirs).ok();
    // The writer finishes the frame it is writing and then leaves the terminal alone until the editor has closed.
    screen.out.pause();
    stop_asking_the_terminal();
    ratatui::restore();
    let ran = editor::run(launch);
    let _ = enable_raw_mode();
    let _ = execute!(io::stdout(), EnterAlternateScreen, Clear(ClearType::All));
    ask_the_terminal();
    // Everything is drawn again (the frame drawn on the cleared screen, every cell; the cursor hidden again).
    screen.out.resume();
    screen.forget();
    if let Some(m) = mark {
        editor::unmark(&m);
    }
    match ran {
        Ok(st) if st.success() => Ok(()),
        // Closed by a signal or with an error of its own: said, and platoon carries on.
        Ok(st) => Err(match st.code() {
            Some(c) => format!("it stopped with exit code {c}"),
            None => "it was stopped".to_string(),
        }),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_usage_text_names_tasks_and_the_task_hooks() {
        assert!(USAGE.contains("task add | task list"), "{USAGE}");
        // No `platoon squads` or `platoon ticket` any more: `task squads` lists them, `task add --squad` names one.
        assert!(!USAGE.contains("platoon squads") && !USAGE.contains("ticket ") && !USAGE.contains("| squads"), "{USAGE}");
        assert!(USAGE.contains("task squads") && USAGE.contains("the /to-tasks skill"), "{USAGE}");
        assert!(!USAGE.contains("offer-tasks"), "kept only so older sessions don't fail: {USAGE}");
        assert!(USAGE.contains("[--squad <id>]"), "{USAGE}");
        assert!(USAGE.contains("  task add [--project") && USAGE.contains("  task list [--project"));
        for h in crate::taskrun::HOOKS {
            assert!(USAGE.contains(&format!("hook {h}")), "{h}");
        }
    }
    use crate::app::{Action, AdoptKind, App, Confirm, Hit, JumpPlan, Mode};
    use crate::model::Status;
    use crate::testkit::{World, alive};
    use crate::theme::Theme;
    use ratatui::{
        Terminal,
        backend::TestBackend,
        crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
        style::Color,
    };

    fn screen_text(term: &Terminal<TestBackend>) -> String {
        let buf = term.backend().buffer();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn draw(term: &mut Terminal<TestBackend>, app: &App) -> String {
        term.draw(|f| ui::draw(f, app)).unwrap();
        screen_text(term)
    }

    /// A dialog's text with its borders removed and line breaks collapsed, so a phrase can be looked for
    /// even where word wrapping split it across lines.
    fn flat(text: &str) -> String {
        text.replace(['│', '╭', '╮', '╰', '╯', '─'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// Only what is inside the settings popup, with line breaks collapsed: the list behind it shows at its sides
    /// on a wide terminal, and its words would otherwise get mixed into the popup's wrapped lines.
    fn popup_flat(shown: &str) -> String {
        let lines: Vec<Vec<char>> = shown.lines().map(|l| l.chars().collect()).collect();
        let top = lines.iter().position(|l| l.iter().collect::<String>().contains("╭ Settings")).expect("the popup");
        let left = lines[top]
            .iter()
            .collect::<String>()
            .find("╭ Settings")
            .map(|b| lines[top].iter().collect::<String>()[..b].chars().count())
            .unwrap();
        let right = lines[top].iter().rposition(|&c| c == '╮').unwrap();
        let mut out = String::new();
        for line in &lines[top + 1..] {
            if line.get(left) == Some(&'╰') {
                break;
            }
            out.extend(line.iter().skip(left + 1).take(right - left - 1));
            out.push('\n');
        }
        flat(&out)
    }

    fn click(app: &mut App, x: u16, y: u16) {
        app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: x, row: y, modifiers: KeyModifiers::NONE });
    }

    fn click_action(app: &mut App, a: Action) {
        let r = app.hits.borrow().iter().find(|(_, h)| matches!(h, Hit::Action(x) if *x == a)).map(|(r, _)| *r).expect("button not drawn");
        click(app, r.x + 1, r.y);
    }

    fn click_tab(app: &mut App, i: usize) {
        let r = app.hits.borrow().iter().find(|(_, h)| matches!(h, Hit::Tab(x) if *x == i)).map(|(r, _)| *r).expect("tab not drawn");
        click(app, r.x + 1, r.y);
    }

    fn key(app: &mut App, c: KeyCode) {
        app.on_key(KeyEvent::new(c, KeyModifiers::NONE));
    }

    fn ch(app: &mut App, c: char) {
        key(app, KeyCode::Char(c));
    }

    fn hosted(pid: u32) -> crate::host::HostInfo {
        crate::host::HostInfo {
            agent: crate::agents::Agent::Claude,
            claude_pid: pid,
            host_pid: 0,
            dir: "/tmp".into(),
            args: Vec::new(),
            sock: Default::default(),
            task: None,
            version: None,
            kept: None,
        }
    }

    fn row_of(app: &App, id: &str) -> usize {
        app.visible().iter().position(|&i| app.sessions[i].id == id).unwrap()
    }

    fn ids(app: &App) -> Vec<&str> {
        app.visible().iter().map(|&i| app.sessions[i].id.as_str()).collect()
    }

    // --- the list ---------------------------------------------------------------------------------

    /// Three saved conversations (one with nothing but a slash command), one of them running.
    fn mixed(name: &str) -> (World, u32) {
        let mut w = World::new(name);
        w.transcript("old-1", "/srv/alpha", "refactor the parser", 3 * 24 * 60);
        w.transcript("mid-2", "/srv/beta", "add a login page", 30);
        w.transcript("new-3", "/srv/gamma", "write the docs", 3);
        w.transcript_lines(
            "junk-4",
            &[r#"{"type":"user","cwd":"/srv/x","message":{"content":"<command-name>/clear</command-name>"}}"#.into()],
        );
        let pid = w.live("mid-2", "/srv/beta", 1, "idle");
        (w, pid)
    }

    #[test]
    fn every_saved_conversation_is_listed_newest_first_running_or_not_and_junk_is_hidden() {
        let (w, pid) = mixed("all");
        let app = w.app();
        assert_eq!(ids(&app), ["new-3", "mid-2", "old-1"], "junk-4 has no real prompt");
        let by = |id: &str| &app.sessions[app.sessions.iter().position(|s| s.id == id).unwrap()];
        // Not running: still a full row, and simply "idle".
        let old = by("old-1");
        assert_eq!((old.name.as_str(), old.cwd.as_str(), old.description.as_str()), ("alpha", "/srv/alpha", "refactor the parser"));
        assert!(old.procs.is_empty() && old.status == Status::Idle && !old.unread);
        assert_eq!(
            (old.branch.as_deref(), old.model.as_deref(), old.context_pct, old.context_exact),
            (Some("main"), Some("Sonnet 5"), 25, false)
        );
        // Running conversations carry their process.
        assert_eq!(by("mid-2").procs.iter().map(|p| p.pid).collect::<Vec<_>>(), [pid]);
        assert_eq!(app.counts, [3, 0]);
    }

    #[test]
    fn with_no_sessions_the_list_says_so_once_the_first_pass_is_done_not_indexing_at_every_later_one() {
        let w = World::new("empty-index");
        let mut app = w.app();
        app.index_in_background_for_test();
        let start = std::time::Instant::now();
        while !app.first_scan_done_for_test() {
            assert!(start.elapsed() < Duration::from_secs(10), "the first pass never finished");
            app.refresh_for_test();
            std::thread::sleep(Duration::from_millis(5));
        }
        // Each refresh starts the next once-a-second pass. A frame drawn before that pass ends (on a busy computer,
        // nearly every one) is not "indexing", and says there are no sessions.
        let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
        for _ in 0..20 {
            app.poll_index_for_test();
            assert_eq!(app.indexing(), None, "the first pass is done");
            let shown = draw(&mut term, &app);
            assert!(shown.contains("No sessions yet") && !shown.contains("indexing"), "{shown}");
        }
    }

    #[test]
    fn a_running_conversation_without_a_saved_transcript_is_still_listed() {
        let mut w = World::new("nofile");
        w.live("fresh-id", "/tmp", 5, "idle");
        let app = w.app();
        assert_eq!(ids(&app), ["fresh-id"]);
    }

    #[test]
    fn rows_show_status_last_activity_and_never_process_vocabulary() {
        let (mut w, _) = mixed("render");
        let busy = w.live("new-3", "/srv/gamma", 2, "busy");
        let mut app = w.app();
        let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
        let text = draw(&mut term, &app);
        println!("{text}");
        for want in [
            "NAME",
            "DIR",
            "LAST",
            "Active 3",
            "Archived 0",
            "working",
            "idle",
            "3m",
            "30m",
            "3d",
            "gamma",
            "write the docs",
            "Sonnet 5",
            "main",
            "25%",
        ] {
            assert!(flat(&text).contains(want), "missing {want:?}:\n{text}");
        }
        for banned in ["ext", "paused", "2 places", "kill", "x kill", "not open", "open elsewhere", "in platoon"] {
            assert!(!text.contains(banned), "{banned:?} must not appear:\n{text}");
        }
        assert!(
            text.contains("enter open") && text.contains("a archive") && flat(&text).contains("1 Sessions 2 Projects 3 Tasks"),
            "{text}"
        );
        // Working then finished while unseen -> "done".
        w.register(busy, "new-3", "/srv/gamma", 2, "idle");
        app.reload_for_test();
        assert!(app.sessions.iter().find(|s| s.id == "new-3").unwrap().unread);
        assert!(draw(&mut term, &app).contains("done"));
    }

    #[test]
    fn exact_context_from_the_statusline_wins_over_the_estimate() {
        let w = World::new("exact");
        w.transcript("aaa", "/srv/a", "hi", 1);
        std::fs::create_dir_all(w.cache.join("status")).unwrap();
        std::fs::write(w.cache.join("status/aaa.json"), format!(r#"{{"ts":{},"context_pct":61}}"#, crate::usage::now())).unwrap();
        let app = w.app();
        let s = &app.sessions[0];
        assert_eq!((s.context_pct, s.context_exact), (61, true));
        let mut term = Terminal::new(TestBackend::new(120, 20)).unwrap();
        let text = draw(&mut term, &app);
        assert!(text.contains(" 61%") && !text.contains("25%") && !text.contains("~"), "{text}");
    }

    /// A conversation at 150,000 tokens, on `model`, as Claude Code saves it.
    fn at_150k(w: &World, id: &str, model: &str, mins_ago: u64) {
        let at = crate::testkit::now_ms() - mins_ago * 60_000;
        let ts = crate::testkit::iso;
        w.transcript_lines(
            id,
            &[
                serde_json::json!({"type":"user","cwd":"/srv/a","timestamp":ts(at - 5000),"message":{"role":"user","content":model}})
                    .to_string(),
                serde_json::json!({"type":"assistant","cwd":"/srv/a","timestamp":ts(at),"message":{"model":model,
                    "usage":{"input_tokens":2_000,"cache_creation_input_tokens":8_000,"cache_read_input_tokens":140_000}}})
                .to_string(),
            ],
        );
    }

    #[test]
    fn a_models_own_window_decides_its_context_figure() {
        let w = World::new("window");
        let rows = [
            ("aaa", "claude-opus-5-5", 15),
            ("bbb", "claude-sonnet-5-5", 15),
            ("ccc", "claude-haiku-5-5", 15),
            ("ddd", "claude-haiku-4-5", 75),
            ("eee", "claude-sonnet-5", 75),
        ];
        for (i, (id, model, _)) in rows.iter().enumerate() {
            at_150k(&w, id, model, i as u64 + 1);
        }
        let app = w.app();
        for (id, model, want) in rows {
            let s = app.sessions.iter().find(|s| s.id == id).unwrap();
            assert_eq!((s.context_pct, s.context_exact), (want, false), "{model} at 150,000 tokens");
        }
        let mut term = Terminal::new(TestBackend::new(120, 20)).unwrap();
        let text = draw(&mut term, &app);
        assert_eq!((text.matches(" 15%").count(), text.matches(" 75%").count()), (3, 2), "{text}");
    }

    #[test]
    fn with_claude_codes_1m_window_switched_off_a_1m_model_is_measured_against_200k() {
        let w = World::new("window-off");
        at_150k(&w, "aaa", "claude-opus-5-5", 1);
        std::fs::write(w.root.join("settings.json"), r#"{"env":{"CLAUDE_CODE_DISABLE_1M_CONTEXT":"1"}}"#).unwrap();
        assert_eq!(w.app().sessions[0].context_pct, 75);
    }

    // --- one row per conversation (the earlier pid-per-row bug) ------------------------------------

    #[test]
    fn two_live_processes_on_one_conversation_are_one_row_with_stable_unread_and_selection() {
        let mut w = World::new("dup");
        w.transcript("same-id", "/srv/a", "shared conversation", 5);
        w.transcript("other", "/srv/b", "another one", 20);
        let a = w.live("same-id", "/srv/a", 1, "busy");
        let b = w.live("same-id", "/srv/a", 2, "busy");
        let mut app = w.app();
        assert_eq!(ids(&app), ["same-id", "other"], "one row for two processes");
        assert_eq!(app.sessions[0].procs.len(), 2);
        assert_eq!(app.sessions[0].status, Status::Busy);

        // Select the conversation, then let both go busy -> idle: unread is set once, on that one row.
        app.selected = row_of(&app, "same-id");
        w.register(a, "same-id", "/srv/a", 1, "idle");
        w.register(b, "same-id", "/srv/a", 2, "idle");
        app.reload_for_test();
        assert_eq!(ids(&app), ["same-id", "other"]);
        assert!(app.sessions[0].unread);
        app.reload_for_test();
        assert!(app.sessions[0].unread, "stays unread until looked at");
        assert_eq!(app.selected_session().unwrap().id, "same-id");

        // A newer conversation appears above it: the selection follows the conversation, not the position.
        w.transcript("newest", "/srv/c", "brand new", 0);
        app.reload_for_test();
        assert_eq!(ids(&app), ["newest", "same-id", "other"]);
        assert_eq!(app.selected_session().unwrap().id, "same-id");
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn without_the_guard_platoon_pauses_its_own_copy_and_reload_is_offered_whichever_row_is_selected() {
        let mut w = World::new("noguard");
        w.transcript("same-id", "/tmp", "shared", 1);
        let (a, b) = (w.live("same-id", "/tmp", 1, "idle"), w.live("same-id", "/tmp", 2, "idle"));
        let mut app = w.app();
        app.refresh_with(vec![hosted(a)], false);
        let s = &app.sessions[0];
        let (pa, pb) = (s.procs.iter().find(|p| p.pid == a).unwrap(), s.procs.iter().find(|p| p.pid == b).unwrap());
        assert!(pa.stale && pa.hosted, "platoon's copy must be paused");
        assert!(!pb.stale && !pb.hosted, "platoon can't pause another terminal's copy");
        assert!(w.cache.join("stale").join(a.to_string()).exists());
        match app.jump_plan().unwrap() {
            JumpPlan::Adopt { mut pids, kind, .. } => {
                pids.sort();
                let mut want = vec![a, b];
                want.sort();
                assert_eq!((pids, kind), (want, AdoptKind::Reload));
            }
            other => panic!("expected a reload, got {other:?}"),
        }
        // The pause survives another refresh (it is a marker on disk).
        app.refresh_with(vec![hosted(a)], false);
        assert!(app.sessions[0].procs.iter().find(|p| p.pid == a).unwrap().stale);
    }

    #[test]
    fn with_the_guard_the_platoon_copy_keeps_working_and_enter_attaches_to_it() {
        let mut w = World::new("guard");
        w.transcript("same-id", "/tmp", "shared", 1);
        let (a, _b) = (w.live("same-id", "/tmp", 1, "idle"), w.live("same-id", "/tmp", 2, "idle"));
        let mut app = w.app();
        app.refresh_with(vec![hosted(a)], true);
        assert!(!app.sessions[0].procs.iter().any(|p| p.stale));
        assert_eq!(app.jump_plan(), Some(JumpPlan::Attach(a)));
    }

    #[test]
    fn enter_plans_by_what_is_running_without_showing_process_states() {
        let mut w = World::new("plan");
        w.transcript("rest", "/tmp", "resting", 10);
        w.transcript("out", "/tmp", "outside", 5);
        w.transcript("mine", "/tmp", "hosted", 1);
        let (out, mine) = (w.live("out", "/tmp", 1, "busy"), w.live("mine", "/tmp", 2, "idle"));
        let mut app = w.app();
        app.refresh_with(vec![hosted(mine)], true);
        app.selected = row_of(&app, "rest");
        assert_eq!(app.jump_plan(), Some(JumpPlan::Resume), "nothing running: just open it");
        app.selected = row_of(&app, "mine");
        assert_eq!(app.jump_plan(), Some(JumpPlan::Attach(mine)));
        app.selected = row_of(&app, "out");
        assert_eq!(app.jump_plan(), Some(JumpPlan::Adopt { pids: vec![out], kind: AdoptKind::TakeOver, busy: true }));
    }

    #[test]
    fn enter_on_a_session_whose_host_just_stopped_says_so_and_the_next_enter_resumes_it() {
        let mut w = World::new("just-stopped");
        w.transcript("stopped-1", "/tmp", "hello", 5);
        let pid = w.live("stopped-1", "/tmp", 1, "idle");
        let mut app = w.app();
        let gone = crate::host::HostInfo { sock: w.cache.join("hosts/gone.sock"), ..hosted(pid) };
        app.refresh_with(vec![gone], true); // platoon still believes in the host...
        unsafe { libc::kill(pid as i32, libc::SIGKILL) }; // ...but it has just ended (our own stand-in process)
        crate::testkit::wait_ended(pid);
        key(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::List));
        assert_eq!(app.active_notice(), Some("That session just stopped. Press enter to open it again."));
        assert_eq!(app.jump_plan(), Some(JumpPlan::Resume), "the row is an ordinary conversation again");
        assert!(app.hosts.is_empty() && app.sessions[0].procs.is_empty());
    }

    #[test]
    fn take_over_dialog_is_shown_for_a_plain_terminal_copy_and_readable() {
        let mut w = World::new("takeover");
        w.transcript("out", "/tmp", "outside", 5);
        w.live("out", "/tmp", 1, "idle");
        let mut app = w.app();
        let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
        key(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Confirm(Confirm::Adopt { .. })));
        let text = draw(&mut term, &app);
        assert!(text.contains("Take over") && text.contains("y take over") && text.contains("n cancel"), "{text}");
        click_action(&mut app, Action::Cancel); // never confirm in a test: it would end a process
        assert!(matches!(app.mode, Mode::List));
    }

    #[test]
    fn enter_on_a_conversation_whose_directory_is_gone_says_so_and_starts_nothing() {
        let w = World::new("gone");
        w.transcript("aaa", "/no/such/dir/anywhere", "old work", 5);
        let mut app = w.app();
        let mut term = Terminal::new(TestBackend::new(110, 24)).unwrap();
        key(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::List));
        assert!(app.hosts.is_empty());
        let text = draw(&mut term, &app);
        assert!(text.contains("no longer exists") && text.contains("/no/such/dir/anywhere"), "{text}");
        assert!(w.archive_file().is_none());
    }

    // --- tabs ---------------------------------------------------------------------------------------

    #[test]
    fn number_keys_and_clicks_switch_tabs_and_a_missing_tab_does_nothing() {
        let (w, _) = mixed("tabs");
        std::fs::create_dir_all(&w.config).unwrap();
        std::fs::write(w.config.join("archive.json"), r#"{"version":1,"archived":{"old-1":5}}"#).unwrap();
        let mut app = w.app();
        let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
        assert_eq!((app.top, app.tab), (0, 0));
        assert_eq!(ids(&app), ["new-3", "mid-2"]);
        let shown = flat(&draw(&mut term, &app));
        assert!(
            shown.contains("1 Sessions 2 Projects 3 Tasks ") && !shown.contains("Plugins") && shown.contains("Active 2 Archived 1"),
            "{shown}"
        );

        // Left and right: Active and Archived.
        key(&mut app, KeyCode::Right);
        assert_eq!((app.tab, ids(&app)), (1, vec!["old-1"]));
        key(&mut app, KeyCode::Right); // nothing further right
        assert_eq!(app.tab, 1);
        key(&mut app, KeyCode::Left);
        assert_eq!((app.tab, app.selected), (0, 0));

        // 1 to 4: the top-level tabs. A number with no tab does nothing.
        ch(&mut app, '3');
        assert!(app.top == 2 && app.board_showing() && app.selected_session().is_none());
        assert!(flat(&draw(&mut term, &app)).contains(&crate::board::Shelf::Active.empty(Some("/to-tasks"))));
        ch(&mut app, '2');
        assert!(app.top == 1 && app.projects_list_showing() && app.selected_session().is_none());
        assert!(flat(&draw(&mut term, &app)).contains("No projects yet. Press n to make one."));
        ch(&mut app, '4');
        assert!(app.top == 3 && app.squads_showing() && app.selected_session().is_none());
        assert!(flat(&draw(&mut term, &app)).contains("Feature"));
        ch(&mut app, '6');
        assert!(app.top == 3 && matches!(app.mode, Mode::List));
        ch(&mut app, '1');
        assert_eq!(app.top, 0);

        // Clicks: the top-level tabs and the Active/Archived chips.
        draw(&mut term, &app);
        click_tab(&mut app, 1);
        assert_eq!(app.tab, 1);
        draw(&mut term, &app);
        click_tab(&mut app, 0);
        assert_eq!(app.tab, 0);
        draw(&mut term, &app);
        let r = app.hits.borrow().iter().find(|(_, h)| matches!(h, Hit::Top(1))).map(|(r, _)| *r).unwrap();
        click(&mut app, r.x + 1, r.y);
        assert_eq!(app.top, 1);
        draw(&mut term, &app);
        let r = app.hits.borrow().iter().find(|(_, h)| matches!(h, Hit::Top(0))).map(|(r, _)| *r).unwrap();
        click(&mut app, r.x + 1, r.y);
        assert_eq!(app.top, 0);

        // Digits are plain text while typing a filter.
        ch(&mut app, '/');
        ch(&mut app, '2');
        assert_eq!((app.top, app.tab, app.filter.as_str()), (0, 0, "2"));
    }

    #[test]
    fn navigation_and_the_filter_work_within_the_current_tab() {
        let (w, _) = mixed("nav");
        std::fs::create_dir_all(&w.config).unwrap();
        std::fs::write(w.config.join("archive.json"), r#"{"archived":{"old-1":5}}"#).unwrap();
        let mut app = w.app();
        ch(&mut app, 'G');
        assert_eq!(app.selected_session().unwrap().id, "mid-2");
        ch(&mut app, 'g');
        key(&mut app, KeyCode::Down);
        assert_eq!(app.selected_session().unwrap().id, "mid-2");
        // Filtering only searches the tab you're on.
        ch(&mut app, '/');
        for c in "alpha".chars() {
            ch(&mut app, c);
        }
        assert!(app.visible().is_empty(), "alpha is archived, so not on Active");
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Right);
        ch(&mut app, '/');
        for c in "ALPHA".chars() {
            ch(&mut app, c);
        }
        assert_eq!(ids(&app), ["old-1"]);
        assert_eq!(app.counts, [2, 1], "counts ignore the filter");
    }

    // --- archiving ----------------------------------------------------------------------------------

    #[test]
    fn archiving_a_resting_conversation_moves_it_and_persists_and_a_restores_it() {
        let (w, _) = mixed("archive");
        let mut app = w.app();
        app.selected = row_of(&app, "old-1");
        ch(&mut app, 'a');
        assert_eq!(ids(&app), ["new-3", "mid-2"]);
        assert_eq!(app.counts, [2, 1]);
        let file = w.archive_file().unwrap();
        assert!(file["archived"]["old-1"].as_u64().unwrap() > 0, "{file}");
        assert!(w.root.join("projects/-some-project/old-1.jsonl").exists(), "the transcript is kept");
        assert!(app.active_notice().unwrap().contains("Archived"));

        // A new run of platoon still has it archived.
        let app2 = w.app();
        assert_eq!(app2.counts, [2, 1]);

        key(&mut app, KeyCode::Right);
        assert_eq!(ids(&app), ["old-1"]);
        let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
        let text = draw(&mut term, &app);
        assert!(text.contains("a restore") && text.contains("refactor the parser"), "{text}");
        ch(&mut app, 'a');
        assert_eq!(app.counts, [3, 0]);
        assert!(w.archive_file().unwrap()["archived"].as_object().unwrap().is_empty());
        assert!(draw(&mut term, &app).contains("Nothing archived"));
    }

    #[test]
    fn archiving_an_idle_conversation_running_in_a_plain_terminal_asks_first() {
        let (w, pid) = mixed("archive-outside");
        let mut app = w.app();
        app.selected = row_of(&app, "mid-2");
        ch(&mut app, 'a');
        assert!(
            matches!(app.mode, Mode::Confirm(Confirm::Archive { outside: true, busy: false, .. })),
            "it isn't platoon's process to end unasked"
        );
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let text = flat(&draw(&mut term, &app));
        for phrase in ["outside platoon", "Archiving ends that copy.", "The session is kept", "y archive", "n cancel"] {
            assert!(text.contains(phrase), "{phrase:?} cut off in:\n{text}");
        }
        assert!(!text.contains("working right now"), "it's idle: {text}");

        ch(&mut app, 'n');
        assert!(matches!(app.mode, Mode::List) && alive(pid) && w.archive_file().is_none(), "cancel touches nothing");

        ch(&mut app, 'a');
        ch(&mut app, 'y');
        assert!(matches!(app.mode, Mode::List));
        assert!(app.settle_stops_for_test(std::time::Duration::from_secs(5)), "the stop finishes");
        assert!(!alive(pid), "SIGTERM must have ended it");
        assert!(w.archive_file().unwrap()["archived"].get("mid-2").is_some());
    }

    #[test]
    fn archiving_an_idle_conversation_platoon_runs_stops_it_without_asking() {
        let (w, pid) = mixed("archive-hosted");
        let mut app = w.app();
        app.refresh_with(vec![hosted(pid)], true); // now it runs under a platoon host
        app.selected = row_of(&app, "mid-2");
        ch(&mut app, 'a');
        assert!(matches!(app.mode, Mode::List), "platoon's own idle process: nothing to protect");
        assert!(app.settle_stops_for_test(std::time::Duration::from_secs(5)), "the stop finishes");
        assert!(!alive(pid));
        assert!(w.archive_file().unwrap()["archived"].get("mid-2").is_some());
    }

    #[test]
    fn archiving_a_working_conversation_asks_first_and_cancel_touches_nothing() {
        let mut w = World::new("archive-busy");
        w.transcript("busy-1", "/tmp", "long task", 1);
        let pid = w.live("busy-1", "/tmp", 1, "busy");
        let mut app = w.app();
        ch(&mut app, 'a');
        assert!(matches!(app.mode, Mode::Confirm(Confirm::Archive { .. })));
        for size in [(110, 30), (60, 24)] {
            let mut term = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
            let text = flat(&draw(&mut term, &app));
            for phrase in [
                "Archive \"tmp\"?",
                "outside platoon",
                "working right now.",
                "interrupts what it is doing.",
                "press enter to pick it up again.",
                "y archive",
                "n cancel",
            ] {
                assert!(text.contains(phrase), "{size:?}: {phrase:?} cut off in:\n{text}");
            }
        }
        ch(&mut app, 'n');
        assert!(matches!(app.mode, Mode::List) && alive(pid) && w.archive_file().is_none());

        ch(&mut app, 'a');
        ch(&mut app, 'y');
        assert!(matches!(app.mode, Mode::List));
        assert!(app.settle_stops_for_test(std::time::Duration::from_secs(5)), "the stop finishes");
        assert!(!alive(pid) && w.archive_file().unwrap()["archived"].get("busy-1").is_some());
        assert_eq!(app.counts, [0, 1]);
    }

    // --- automatic clean-up ---------------------------------------------------------------------------

    const THREE_DAYS: u64 = 3 * 24 * 60;

    /// The one-time notice, as it must read.
    fn notice_of(n: usize) -> String {
        format!("Auto-archived {n} sessions idle for 2+ days. Find them in Archived.")
    }

    #[test]
    fn a_first_run_archives_every_old_idle_conversation_in_one_write_and_says_so_once() {
        let mut w = World::new("auto-first");
        w.settings(120, 2); // the defaults
        for i in 0..143 {
            w.transcript(&format!("old-{i:03}"), "/srv/old", &format!("old work {i}"), THREE_DAYS + i);
        }
        w.transcript("fresh", "/srv/fresh", "today's work", 30);
        w.transcript("runs", "/srv/run", "long ago but still running", THREE_DAYS * 2);
        let pid = w.live("runs", "/srv/run", 1, "idle");
        let app = w.app();

        assert_eq!(app.counts, [2, 143], "only the fresh one and the running one stay");
        assert_eq!(ids(&app), ["fresh", "runs"]);
        assert_eq!(app.archive_writes_for_test(), 1, "143 conversations must be one write, not 143");
        assert_eq!(w.archive_file().unwrap()["archived"].as_object().unwrap().len(), 143);
        assert!(!w.config.join("archive.json.tmp").exists());
        assert!(alive(pid), "archiving never touches a process");
        assert_eq!(app.active_notice(), Some(notice_of(143).as_str()));
        // Nothing was deleted: the transcripts are all still there.
        assert!(w.root.join("projects/-some-project/old-000.jsonl").exists());
    }

    #[test]
    fn the_notice_is_readable_in_full_on_ordinary_terminals() {
        let w = World::new("auto-render");
        w.settings(120, 2);
        for i in 0..143 {
            w.transcript(&format!("old-{i:03}"), "/srv/old", "old work", THREE_DAYS);
        }
        w.transcript("fresh", "/srv/fresh", "today's work", 30);
        let app = w.app();
        for (cols, rows) in [(80, 24), (100, 24), (140, 30)] {
            let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
            let text = draw(&mut term, &app);
            assert!(text.contains(&notice_of(143)), "{cols}x{rows}: notice cut off:\n{text}");
            assert!(flat(&text).contains("Active 1") && flat(&text).contains("Archived 143"), "{text}");
        }
        // It takes the place of the key hints it can't share the line with (never `? help`), so it goes as soon as the
        // user does anything.
        let mut app = app;
        let mut term = Terminal::new(TestBackend::new(110, 24)).unwrap();
        let text = draw(&mut term, &app);
        assert!(text.contains("? help") && !text.contains("s settings") && text.contains(&notice_of(143)), "{text}");
        key(&mut app, KeyCode::Down);
        let text = draw(&mut term, &app);
        assert!(text.contains("enter open") && text.contains("s settings") && !text.contains("Auto-archived"), "{text}");
    }

    #[test]
    fn the_notice_is_singular_for_one_and_names_the_configured_age() {
        let w = World::new("auto-one");
        w.settings(120, 1);
        w.transcript("old", "/srv/old", "old work", 25 * 60);
        let app = w.app();
        assert_eq!(app.active_notice(), Some("Auto-archived 1 session idle for 1+ day. Find it in Archived."));
    }

    #[test]
    fn nothing_is_archived_and_nothing_said_when_nothing_qualifies_or_it_is_off() {
        let w = World::new("auto-none");
        w.settings(120, 2);
        w.transcript("recent", "/srv/a", "hello", 47 * 60);
        let app = w.app();
        assert_eq!((app.counts.clone(), app.active_notice(), w.archive_file().is_none()), (vec![1, 0], None, true));

        w.settings(120, 0); // 0 = off
        w.transcript("ancient", "/srv/a", "hello", 400 * 24 * 60);
        let app = w.app();
        assert_eq!((app.counts.clone(), app.active_notice(), w.archive_file().is_none()), (vec![2, 0], None, true));
    }

    #[test]
    fn a_settings_change_applies_to_a_running_platoon_without_a_restart() {
        let w = World::new("auto-live");
        w.settings(120, 0);
        w.transcript("old", "/srv/a", "old work", THREE_DAYS);
        let mut app = w.app();
        app.refresh_for_test();
        assert_eq!(app.counts, [1, 0]);

        w.settings(120, 2);
        app.refresh_for_test();
        assert_eq!(app.counts, [0, 1], "the edited file is picked up on the next refresh");
        assert_eq!(app.active_notice(), Some("Auto-archived 1 session idle for 2+ days. Find it in Archived."));

        // A longer age puts nothing back (archiving is not undone by settings), but archives nothing more.
        w.settings(120, 30);
        w.transcript("newer", "/srv/a", "work", THREE_DAYS);
        app.refresh_for_test();
        assert_eq!(app.counts, [1, 1]);
    }

    #[test]
    fn a_conversation_that_turns_old_while_platoon_is_open_is_caught_by_the_periodic_check() {
        let w = World::new("auto-periodic");
        w.settings(120, 2);
        w.transcript("aging", "/srv/a", "work", 47 * 60 + 59); // 2 minutes short of 2 days
        let mut app = w.app();
        assert_eq!(app.counts, [1, 0]);
        // Time moving on is simulated by the injected clock: the same list, evaluated 3 minutes later.
        app.auto_archive_at_for_test(crate::usage::now() + 180);
        assert_eq!(app.counts, [0, 1]);
    }

    #[test]
    fn restoring_an_old_conversation_sticks_across_evaluations_and_restarts() {
        let w = World::new("auto-restore");
        w.settings(120, 2);
        w.transcript("old", "/srv/a", "old work", THREE_DAYS);
        w.transcript("keep", "/srv/a", "recent work", 10);
        let mut app = w.app();
        assert_eq!(app.counts, [1, 1]);

        key(&mut app, KeyCode::Right);
        assert_eq!(ids(&app), ["old"]);
        ch(&mut app, 'a'); // restore without opening it
        assert_eq!(app.counts, [2, 0]);
        for _ in 0..3 {
            app.refresh_for_test();
            app.auto_archive_at_for_test(crate::usage::now() + 3600);
        }
        assert_eq!(app.counts.clone(), [2, 0], "restored means restored");

        let app2 = w.app(); // a fresh start reads the remembered restore time from disk
        assert_eq!(app2.counts, [2, 0]);
        assert!(w.archive_file().unwrap()["touched"]["old"].as_u64().is_some());

        // ...until it has again gone unused for the full age from the restore.
        let mut app3 = w.app();
        app3.auto_archive_at_for_test(crate::usage::now() + 2 * 86_400 + 60);
        assert_eq!(app3.counts.clone(), [0, 2]);
    }

    #[test]
    fn a_conversation_with_a_running_process_is_never_auto_archived_however_old() {
        let mut w = World::new("auto-running");
        w.settings(120, 2);
        w.transcript("busy-old", "/srv/a", "work", THREE_DAYS);
        w.transcript("idle-old", "/srv/a", "work", THREE_DAYS);
        let (a, b) = (w.live("busy-old", "/srv/a", 1, "busy"), w.live("idle-old", "/srv/a", 2, "idle"));
        let mut app = w.app();
        assert_eq!(app.counts, [2, 0]);
        app.auto_archive_at_for_test(crate::usage::now() + 30 * 86_400);
        assert_eq!(app.counts.clone(), [2, 0]);
        assert!(alive(a) && alive(b));
        assert!(w.archive_file().is_none());
    }

    #[test]
    fn a_conversation_being_viewed_is_never_auto_archived_even_before_its_process_registers() {
        let w = World::new("auto-viewing");
        w.settings(120, 0);
        w.transcript("resumed", "/srv/a", "work", THREE_DAYS * 10);
        w.transcript("other", "/srv/a", "work", THREE_DAYS * 10);
        let mut app = w.app();
        assert_eq!(app.counts, [2, 0]);
        // Just resumed: platoon knows the conversation, but claude hasn't registered a process yet.
        app.view_conversation_for_test("resumed", 4242);
        w.settings(120, 2);
        app.auto_archive_at_for_test(crate::usage::now());
        assert_eq!(app.counts, [1, 1]);
        assert_eq!(ids(&app), ["resumed"], "only the one being viewed stays");
    }

    #[test]
    fn an_unreadable_archive_file_is_never_overwritten_automatically() {
        let w = World::new("auto-corrupt");
        w.settings(120, 2);
        std::fs::create_dir_all(&w.config).unwrap();
        std::fs::write(w.config.join("archive.json"), "garbage{").unwrap();
        w.transcript("old", "/srv/a", "old work", THREE_DAYS);
        let app = w.app();
        assert_eq!(app.counts, [1, 0]);
        assert_eq!(std::fs::read_to_string(w.config.join("archive.json")).unwrap(), "garbage{");
        assert!(!w.config.join("archive.json.bak").exists());
    }

    #[test]
    fn opening_a_conversation_counts_as_activity() {
        let w = World::new("auto-open");
        w.settings(120, 0); // off while we set up, so it shows in the list
        w.transcript("old", "/srv/a", "old work", THREE_DAYS);
        let mut app = w.app();
        app.opened_for_test("old"); // what a successful Enter records
        w.settings(120, 2);
        app.refresh_for_test();
        assert_eq!(app.counts, [1, 0], "opened just now, so not idle for 2 days");
        assert!(w.archive_file().unwrap()["touched"]["old"].as_u64().is_some());
    }

    #[test]
    fn a_corrupt_archive_file_is_tolerated_and_kept_as_a_backup() {
        let w = World::new("corrupt");
        w.transcript("aaa", "/tmp", "hello", 1);
        std::fs::create_dir_all(&w.config).unwrap();
        std::fs::write(w.config.join("archive.json"), "garbage{").unwrap();
        let mut app = w.app();
        assert_eq!(ids(&app), ["aaa"]);
        ch(&mut app, 'a');
        assert_eq!(std::fs::read_to_string(w.config.join("archive.json.bak")).unwrap(), "garbage{");
        assert_eq!(app.counts, [0, 1]);
    }

    #[test]
    fn there_is_no_kill_key_or_kill_hint() {
        let (w, pid) = mixed("nokill");
        let mut app = w.app();
        let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
        app.selected = row_of(&app, "mid-2");
        ch(&mut app, 'x');
        assert!(matches!(app.mode, Mode::List));
        assert!(alive(pid));
        assert!(!draw(&mut term, &app).contains("kill"));
    }

    // --- chrome ----------------------------------------------------------------------------------------

    #[test]
    fn footer_buttons_are_clickable() {
        let (w, _) = mixed("footer");
        let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
        let mut app = w.app();
        draw(&mut term, &app);

        // n -> picker opens, and shows candidate directories
        click_action(&mut app, Action::New);
        assert!(matches!(app.mode, Mode::Picker));
        let text = draw(&mut term, &app);
        assert!(text.contains("New session"), "{text}");
        click_action(&mut app, Action::Cancel);
        assert!(matches!(app.mode, Mode::List));

        // a -> archives the selected conversation (it is idle and not running)
        draw(&mut term, &app);
        let first = app.selected_session().unwrap().id.clone();
        click_action(&mut app, Action::Archive);
        assert!(!ids(&app).contains(&first.as_str()));

        // / -> filter mode, typing filters, esc-button clears
        draw(&mut term, &app);
        click_action(&mut app, Action::Search);
        assert!(matches!(app.mode, Mode::Search));
        for c in "zzzz-no-such".chars() {
            ch(&mut app, c);
        }
        assert!(app.visible().is_empty());
        assert!(draw(&mut term, &app).contains("No sessions match"));
        click_action(&mut app, Action::Cancel);
        assert!(matches!(app.mode, Mode::List) && app.filter.is_empty());

        // q with nothing hosted quits
        draw(&mut term, &app);
        click_action(&mut app, Action::Quit);
        assert!(app.quit);
    }

    #[test]
    fn rows_highlight_on_hover_and_scroll_and_click_select_within_the_tab() {
        let w = World::new("mouse");
        for i in 0..5 {
            w.transcript(&format!("id{i}"), "/srv/a", &format!("prompt {i}"), i + 1);
        }
        let mut app = w.app();
        let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
        draw(&mut term, &app);
        let second = app.hits.borrow().iter().find(|(_, h)| matches!(h, Hit::Row(1))).map(|(r, _)| *r).unwrap();
        app.on_mouse(MouseEvent { kind: MouseEventKind::Moved, column: second.x + 2, row: second.y, modifiers: KeyModifiers::NONE });
        draw(&mut term, &app);
        let surface = app.theme.surface;
        assert_eq!(term.backend().buffer()[(second.x + 30, second.y)].bg, surface, "hovered row is highlighted");
        click(&mut app, second.x + 2, second.y);
        assert_eq!(app.selected, 1);
        app.on_mouse(MouseEvent { kind: MouseEventKind::ScrollDown, column: 5, row: 5, modifiers: KeyModifiers::NONE });
        assert_eq!(app.selected, 2);
    }

    #[test]
    fn archived_rows_are_dimmed() {
        let w = World::new("dim");
        w.transcript("aaa", "/srv/a", "hello", 1);
        std::fs::create_dir_all(&w.config).unwrap();
        std::fs::write(w.config.join("archive.json"), r#"{"archived":{"aaa":1}}"#).unwrap();
        let mut app = w.app();
        let mut term = Terminal::new(TestBackend::new(110, 20)).unwrap();
        key(&mut app, KeyCode::Right);
        draw(&mut term, &app);
        let buf = term.backend().buffer();
        let row = (0..buf.area.height)
            .find(|&y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>().contains("hello"))
            .unwrap();
        let name_cell = (0..buf.area.width).map(|x| &buf[(x, row - 1)]).find(|c| c.symbol() == "a").unwrap();
        // Dim, as dim is drawn on the selected row's fill.
        let t = &app.theme;
        assert_eq!((name_cell.fg, name_cell.bg), (t.readable_on(t.dim, t.surface, crate::theme::MIN_DIM), t.surface));
    }

    #[test]
    fn nothing_is_cut_off_on_a_narrow_terminal() {
        let (w, _) = mixed("narrow");
        let app = w.app();
        for (cols, rows) in [(60, 16), (80, 24), (40, 12)] {
            let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
            let text = draw(&mut term, &app); // must not panic
            assert!(text.contains("platoon"), "{cols}x{rows}\n{text}");
        }
    }

    #[test]
    fn paused_copy_shows_a_banner_and_the_reload_dialog_explains_itself() {
        let mut w = World::new("banner");
        w.transcript("same-id", "/tmp", "shared", 1);
        let (a, b) = (w.live("same-id", "/tmp", 1, "idle"), w.live("same-id", "/tmp", 2, "idle"));
        let mut app = w.app();
        app.refresh_with(vec![hosted(a)], false);
        app.mode = Mode::Attached(a);
        assert!(app.attached_paused());
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| ui::draw_paused_banner(f, &app, f.area())).unwrap();
        let text = screen_text(&term);
        assert!(text.contains("Session paused") && text.contains("not sent") && text.contains("reload"), "{text}");
        // The other copy isn't paused.
        app.mode = Mode::Attached(b);
        assert!(!app.attached_paused());

        app.mode = Mode::Confirm(Confirm::Adopt {
            pids: vec![a, b],
            id: "same-id".into(),
            name: "x".into(),
            cwd: "/tmp".into(),
            busy: true,
            kind: AdoptKind::Reload,
            restore: false,
        });
        let text = flat(&draw(&mut term, &app));
        assert!(
            text.contains("Reload") && text.contains("open in 2 places") && text.contains("interrupts it."),
            "the whole warning must be readable: {text}"
        );
    }

    #[test]
    fn picker_filters_and_accepts_paths() {
        let (w, _) = mixed("picker");
        let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
        let mut app = w.app();
        ch(&mut app, 'n');
        for c in "/tmp".chars() {
            ch(&mut app, c);
        }
        assert_eq!(app.picker.results.first().map(String::as_str), Some("/tmp"));
        assert!(draw(&mut term, &app).contains("/tmp"));
    }

    #[test]
    fn a_fresh_start_with_nothing_saved_explains_itself() {
        let w = World::new("empty");
        let app = w.app();
        let mut term = Terminal::new(TestBackend::new(110, 20)).unwrap();
        assert!(draw(&mut term, &app).contains("Press n to start one"));
    }

    /// Starts a real `claude` (no prompt is sent). Needs a folder Claude already trusts (it registers
    /// itself only after the trust prompt), so it's manual: `cargo test -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_claude_is_hosted_and_listed() {
        let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap());
        let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
        let mut app = App::new();
        app.sync_size(110, 30);
        app.act(Action::New);
        app.picker_launch_for_test(dir.to_string_lossy().into_owned());
        assert!(matches!(app.mode, Mode::Attached(_)), "claude failed to start");
        let mut hosted_row = false;
        for _ in 0..60 {
            std::thread::sleep(std::time::Duration::from_millis(250));
            app.refresh();
            hosted_row = app.sessions.iter().any(|s| s.procs.iter().any(|p| p.hosted));
            if hosted_row {
                break;
            }
        }
        println!("{}", draw(&mut term, &app));
        assert!(hosted_row, "hosted claude never appeared in the session list");
        // leave, then archive it (which stops it) through the real flow
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        app.selected = app.visible().iter().position(|&i| app.sessions[i].procs.iter().any(|p| p.hosted)).unwrap();
        key(&mut app, KeyCode::Char('a'));
        key(&mut app, KeyCode::Char('y'));
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(250));
            app.poll_stops();
            app.poll_editors();
            app.refresh();
            if app.hosts.is_empty() {
                break;
            }
        }
        assert!(app.hosts.is_empty(), "claude didn't exit after SIGTERM");
    }

    // --- background sessions (run by Claude's own service) -------------------------------------------

    const BG_ID: &str = "feedf00d-9999-4aaa-8bbb-cccccccccccc";

    /// A saved conversation that Claude's service is running in the background (attach id `feedf00d`), an ordinary
    /// idle one next to it, and the real process standing in for the background one.
    fn bg_world(name: &str) -> (World, u32) {
        let mut w = World::new(name);
        w.transcript(BG_ID, "/srv/alpha", "continue the release", 30);
        w.transcript("plain-1", "/srv/beta", "write the docs", 40);
        let pid = w.live_bg(BG_ID, "/srv/alpha", 1);
        (w, pid)
    }

    fn session<'a>(app: &'a App, id: &str) -> &'a crate::model::Session {
        &app.sessions[app.sessions.iter().position(|s| s.id == id).unwrap()]
    }

    #[test]
    fn a_background_session_is_listed_as_background_on_the_active_tab_instead_of_idle() {
        let (w, pid) = bg_world("bg-listed");
        let app = w.app();
        let s = session(&app, BG_ID);
        assert_eq!(s.status, Status::Background);
        assert_eq!(
            s.procs.iter().map(|p| (p.pid, p.background.as_deref(), p.hosted, p.stale)).collect::<Vec<_>>(),
            [(pid, Some("feedf00d"), false, false)]
        );
        assert!(!s.unread && !s.archived);
        assert_eq!(session(&app, "plain-1").status, Status::Idle, "everything else is as before");
        assert_eq!(ids(&app), [BG_ID, "plain-1"]);
        assert_eq!(app.counts, [2, 0]);
        assert!(alive(pid));
    }

    #[test]
    fn a_background_session_stays_on_the_active_tab_even_if_the_archive_lists_it() {
        let (w, _pid) = bg_world("bg-archived");
        std::fs::create_dir_all(&w.config).unwrap();
        std::fs::write(w.config.join("archive.json"), format!(r#"{{"version":1,"archived":{{"{BG_ID}":5,"plain-1":5}}}}"#)).unwrap();
        let app = w.app();
        assert!(!session(&app, BG_ID).archived && session(&app, "plain-1").archived);
        assert_eq!((app.counts.clone(), ids(&app)), (vec![1, 1], vec![BG_ID]));
    }

    #[test]
    fn the_services_idle_spare_processes_are_not_conversations_and_get_no_row() {
        let mut w = World::new("bg-spare");
        w.transcript("plain-1", "/srv/beta", "write the docs", 40);
        let _keep = w.live("plain-1", "/srv/beta", 1, "idle");
        let child = std::process::Command::new("sleep").arg("30").spawn();
        let mut child = child.unwrap();
        w.register_bg(child.id(), "5ca1ab1e-0000-4000-8000-000000000bb1", "/srv/beta", 2, "5ca1ab1e", true);
        let app = w.app();
        assert_eq!(ids(&app), ["plain-1"]);
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn a_background_row_opens_with_attach_and_never_resume() {
        let (w, _pid) = bg_world("bg-plan");
        let mut app = w.app();
        app.selected = row_of(&app, BG_ID);
        assert_eq!(app.jump_plan(), Some(JumpPlan::AttachBackground("feedf00d".into())));
        // Nothing platoon would confirm or end: no dialog, no process to take over.
        app.selected = row_of(&app, "plain-1");
        assert_eq!(app.jump_plan(), Some(JumpPlan::Resume), "an idle conversation is resumed as always");
    }

    #[test]
    fn once_a_platoon_host_shows_it_the_row_joins_that_host_and_the_attach_client_is_not_a_second_copy() {
        let (w, pid) = bg_world("bg-host");
        let mut app = w.app();
        // The host's own process is a plain `claude attach feedf00d`: it has no registry entry (not among the live
        // processes) but its arguments say which background session it shows.
        let host = crate::host::HostInfo { args: vec!["attach".into(), "feedf00d".into()], ..hosted(4_000_000) };
        app.refresh_with(vec![host], false);
        app.selected = row_of(&app, BG_ID);
        let s = session(&app, BG_ID);
        assert_eq!(
            (s.status, s.procs.len(), s.procs[0].hosted, s.procs[0].stale, s.procs[0].pid),
            (Status::Background, 1, true, false, pid)
        );
        assert_eq!(app.jump_plan(), Some(JumpPlan::Attach(4_000_000)));
        assert_eq!(ids(&app), [BG_ID, "plain-1"], "the attach client adds no row of its own");

        // A host showing some other background session, or resuming one, is not this one.
        let other = crate::host::HostInfo { args: vec!["attach".into(), "12345678".into()], ..hosted(4_000_001) };
        let resumed = crate::host::HostInfo { args: vec!["--resume".into(), BG_ID.into()], ..hosted(4_000_002) };
        app.refresh_with(vec![other, resumed], false);
        assert_eq!(app.jump_plan(), Some(JumpPlan::AttachBackground("feedf00d".into())));
        assert!(!session(&app, BG_ID).procs[0].hosted);
    }

    #[test]
    fn a_background_session_never_marks_a_terminal_copy_of_the_same_conversation_out_of_sync() {
        let mut w = World::new("bg-shared");
        w.transcript(BG_ID, "/srv/alpha", "continue the release", 30);
        let (bg, term) = (w.live_bg(BG_ID, "/srv/alpha", 1), w.live(BG_ID, "/srv/alpha", 2, "idle"));
        let mut app = w.app();
        app.refresh_with(vec![hosted(term)], false); // no guard installed: two interactive copies WOULD be marked
        let s = session(&app, BG_ID);
        assert_eq!(s.procs.len(), 2);
        assert!(s.procs.iter().all(|p| !p.stale), "{:?}", s.procs);
        assert!(!w.cache.join("stale").join(term.to_string()).exists() && !w.cache.join("stale").join(bg.to_string()).exists());
    }

    #[test]
    fn archiving_a_background_row_is_refused_and_touches_nothing() {
        let (w, pid) = bg_world("bg-archive-key");
        let mut app = w.app();
        app.selected = row_of(&app, BG_ID);
        ch(&mut app, 'a');
        assert!(matches!(app.mode, Mode::List), "no dialog either: there is nothing to confirm");
        assert!(alive(pid), "Claude's own process must not be signaled");
        assert!(w.archive_file().is_none(), "and nothing is archived");
        assert!(
            app.notice.as_ref().is_some_and(|(m, ..)| m.contains("runs in the background") && m.contains("can't archive it")),
            "{:?}",
            app.notice.as_ref().map(|n| &n.0)
        );
        assert!(!session(&app, BG_ID).archived);
    }

    #[test]
    fn auto_archive_leaves_a_background_session_alone_however_old_and_never_signals_it() {
        let mut w = World::new("bg-auto");
        w.settings(120, 2);
        w.transcript(BG_ID, "/srv/alpha", "started long ago", THREE_DAYS * 3);
        w.transcript("old-plain", "/srv/beta", "also old", THREE_DAYS);
        let pid = w.live_bg(BG_ID, "/srv/alpha", 1);
        let mut app = w.app();
        assert_eq!(ids(&app), [BG_ID], "the old idle conversation went to Archived; the background one stayed");
        assert!(w.archive_file().unwrap()["archived"].get("old-plain").is_some(), "auto-archive did run");
        assert!(w.archive_file().unwrap()["archived"].get(BG_ID).is_none());
        // Later passes do not change their mind.
        app.auto_archive_at_for_test(crate::usage::now() + 30 * 24 * 3600);
        assert!(w.archive_file().unwrap()["archived"].get(BG_ID).is_none());
        assert!(alive(pid));
        assert!(!session(&app, BG_ID).archived && session(&app, BG_ID).status == Status::Background);
    }

    fn find_text(term: &Terminal<TestBackend>, needle: &str) -> Vec<ratatui::buffer::Cell> {
        let buf = term.backend().buffer();
        for y in 0..buf.area.height {
            let row: Vec<String> = (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
            let line = row.concat();
            if let Some(at) = line.find(needle) {
                let x0 = line[..at].chars().count();
                return (x0..x0 + needle.chars().count()).map(|x| buf[(x as u16, y)].clone()).collect();
            }
        }
        panic!("{needle:?} is not on screen");
    }

    #[test]
    fn the_background_status_is_drawn_in_its_own_color_in_every_theme() {
        let (w, _pid) = bg_world("bg-colors");
        let mut w = w;
        w.transcript("working", "/srv/gamma", "busy one", 3);
        let _busy = w.live("working", "/srv/gamma", 3, "busy");
        // A running one that waits: `idle` (one that nothing runs has no word).
        w.transcript("resting", "/srv/delta", "idle one", 4);
        let _idle = w.live("resting", "/srv/delta", 4, "idle");
        for t in Theme::all() {
            choose_theme(&w, &t.id);
            let app = w.app();
            let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
            let shown = draw(&mut term, &app);
            assert!(shown.contains("Background"), "{}: {shown}", t.id);
            // The word and the marker before the name are in the accent, which every theme already keeps apart from
            // the working, done and warning colors and readable on the background.
            for c in find_text(&term, "Background") {
                assert_eq!(c.fg, t.accent, "{}: {:?}", t.id, c.symbol());
            }
            let marker = find_text(&term, "◆");
            assert_eq!(marker[0].fg, t.accent, "{}", t.id);
            // Not mistaken for the other statuses on the same screen.
            for (word, color) in [("working", t.busy), ("idle", t.dim)] {
                let cell = &find_text(&term, word)[0];
                assert_eq!(cell.fg, color, "{}: {word}", t.id);
                assert_ne!(cell.fg, t.accent, "{}: {word} must differ from Background", t.id);
            }
            if t.ansi.is_some() {
                assert!(crate::theme::contrast(t.accent, t.bg) >= crate::theme::MIN_ROLE, "{}", t.id);
                for (name, other) in [("busy", t.busy), ("ok", t.ok), ("warn", t.warn), ("err", t.err), ("dim", t.dim)] {
                    assert!(crate::theme::delta_e(t.accent, other) >= crate::theme::MIN_DELTA_E, "{}: Background (accent) vs {name}", t.id);
                }
            }
        }
    }

    #[test]
    fn the_word_background_fits_whole_on_narrow_terminals_and_the_header_still_lines_up() {
        let (w, _pid) = bg_world("bg-narrow");
        let app = w.app();
        for (cols, rows) in [(110, 24), (80, 24), (60, 16)] {
            let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
            let shown = draw(&mut term, &app);
            assert!(shown.contains("Background"), "{cols}x{rows}\n{shown}");
            let header = shown.lines().find(|l| l.contains("STATUS")).unwrap();
            let row = shown.lines().find(|l| l.contains("Background")).unwrap();
            let col = |l: &str, needle: &str| l[..l.find(needle).unwrap()].chars().count();
            assert_eq!(col(header, "STATUS"), col(row, "Background"), "{cols}x{rows}: the word sits under its heading");
        }
    }

    // --- a session parked when it moved to the background (Claude Code's `parkedJobId`) ------------------

    const PARKED_ID: &str = "c0ffee00-1111-4222-8333-444444444444";
    const JOB_ID: &str = "beefcafe-5555-4666-8777-888888888888";

    /// A conversation whose interactive process handed it to a background session (a fork with its own id, job
    /// `beefcafe`) and stays alive, parked, its registry status stuck at "busy"; an ordinary idle one beside it.
    /// With `job_runs`, the background session is still running. Returns the parked process's pid.
    fn parked_world(name: &str, job_runs: bool) -> (World, u32) {
        let mut w = World::new(name);
        w.transcript(PARKED_ID, "/tmp", "plan the release", 30);
        w.transcript("plain-1", "/srv/beta", "write the docs", 40);
        if job_runs {
            w.transcript(JOB_ID, "/tmp", "plan the release", 20);
            w.live_bg(JOB_ID, "/tmp", 2);
        }
        let pid = w.live_parked(PARKED_ID, "/tmp", 1, "beefcafe");
        (w, pid)
    }

    #[test]
    fn a_parked_session_is_not_working_and_says_it_moved_away() {
        let (w, pid) = parked_world("parked-listed", true);
        let app = w.app();
        let s = session(&app, PARKED_ID);
        assert_eq!(s.status, Status::Moved, "never Busy, whatever its registry status says");
        assert_eq!(
            s.procs.iter().map(|p| (p.pid, p.parked, p.moved_to.as_deref(), p.background.as_deref(), p.stale)).collect::<Vec<_>>(),
            [(pid, true, Some("beefcafe"), None, false)]
        );
        assert!(!s.unread && !s.archived);
        assert_eq!(session(&app, JOB_ID).status, Status::Background, "the background session it moved to has its own row");
        assert_eq!(session(&app, "plain-1").status, Status::Idle);
        assert!(alive(pid));
    }

    #[test]
    fn opening_a_parked_session_opens_the_background_session_it_moved_to() {
        let (w, pid) = parked_world("parked-plan", true);
        let mut app = w.app();
        app.selected = row_of(&app, PARKED_ID);
        assert_eq!(app.jump_plan(), Some(JumpPlan::AttachBackground("beefcafe".into())), "no take-over, no resume");
        // A platoon host already showing that background session is joined.
        let host = crate::host::HostInfo { args: vec!["attach".into(), "beefcafe".into()], ..hosted(4_000_000) };
        app.refresh_with(vec![host], false);
        app.selected = row_of(&app, PARKED_ID);
        assert_eq!(app.jump_plan(), Some(JumpPlan::Attach(4_000_000)));
        // Even when platoon hosts the parked process itself (the case seen for real), the row goes where the work went.
        app.refresh_with(vec![hosted(pid)], false);
        app.selected = row_of(&app, PARKED_ID);
        assert_eq!(session(&app, PARKED_ID).status, Status::Moved);
        assert_eq!(app.jump_plan(), Some(JumpPlan::AttachBackground("beefcafe".into())));
        assert!(alive(pid));
    }

    #[test]
    fn once_the_background_session_has_gone_a_parked_session_is_an_idle_conversation() {
        let (w, pid) = parked_world("parked-gone", false);
        let mut app = w.app();
        app.selected = row_of(&app, PARKED_ID);
        let s = session(&app, PARKED_ID);
        assert_eq!((s.status, s.unread), (Status::Idle, false));
        assert_eq!(s.procs.iter().map(|p| (p.parked, p.moved_to.clone())).collect::<Vec<_>>(), [(true, None)]);
        // Outside platoon: reopened from the saved history, never "Take over?" (it no longer writes to it).
        assert_eq!(app.jump_plan(), Some(JumpPlan::Resume));
        // Under a platoon host: joined, like any idle session platoon hosts.
        app.refresh_with(vec![hosted(pid)], false);
        app.selected = row_of(&app, PARKED_ID);
        assert_eq!(app.jump_plan(), Some(JumpPlan::Attach(pid)));
        assert!(alive(pid));
    }

    #[test]
    fn a_parked_process_beside_a_copy_opened_again_is_not_a_second_copy_and_marks_nothing_stale() {
        let (mut w, parked) = parked_world("parked-shared", true);
        let copy = w.live(PARKED_ID, "/tmp", 5, "idle");
        let mut app = w.app();
        app.refresh_with(vec![hosted(copy)], false); // no guard installed: two writing copies WOULD be marked
        let s = session(&app, PARKED_ID);
        assert_eq!(s.procs.len(), 2);
        assert!(s.procs.iter().all(|p| !p.stale), "{:?}", s.procs);
        assert!(!w.cache.join("stale").join(copy.to_string()).exists() && !w.cache.join("stale").join(parked.to_string()).exists());
        assert_eq!(s.status, Status::Idle, "the copy opened again is what the row is about");
        app.selected = row_of(&app, PARKED_ID);
        assert_eq!(app.jump_plan(), Some(JumpPlan::Attach(copy)));

        // A terminal copy outside platoon: taking it over names only that copy, never the parked process.
        app.refresh_with(vec![], false);
        app.selected = row_of(&app, PARKED_ID);
        assert_eq!(app.jump_plan(), Some(JumpPlan::Adopt { pids: vec![copy], kind: AdoptKind::TakeOver, busy: false }));
        // And if that copy is working, the row says so.
        w.register(copy, PARKED_ID, "/tmp", 5, "busy");
        app.refresh_with(vec![], false);
        assert_eq!(session(&app, PARKED_ID).status, Status::Busy);
        assert!(alive(parked) && alive(copy));
    }

    #[test]
    fn a_parked_row_is_drawn_quietly_not_in_the_working_color_or_with_the_spinner() {
        let (w, _pid) = parked_world("parked-colors", true);
        let mut w = w;
        w.transcript("working", "/srv/gamma", "busy one", 3);
        let _busy = w.live("working", "/srv/gamma", 3, "busy");
        for t in Theme::all() {
            choose_theme(&w, &t.id);
            let app = w.app();
            let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
            let shown = draw(&mut term, &app);
            let row = shown.lines().find(|l| l.contains("moved away")).unwrap_or_else(|| panic!("{}: {shown}", t.id));
            assert!(!SPIN.iter().any(|g| row.contains(g)), "{}: {row}", t.id);
            assert!(row.contains('◇'), "{}: {row}", t.id);
            for c in find_text(&term, "moved away") {
                assert_eq!(c.fg, t.dim, "{}: {:?}", t.id, c.symbol());
                assert_ne!(c.fg, t.busy, "{}", t.id);
            }
            assert_eq!(find_text(&term, "◇")[0].fg, t.accent, "{}", t.id);
            // Exactly one working row: the real working one.
            assert_eq!(shown.lines().filter(|l| l.contains("working")).count(), 1, "{}: {shown}", t.id);
        }
    }

    #[test]
    fn the_words_moved_away_fit_whole_on_narrow_terminals_and_the_header_still_lines_up() {
        let (w, _pid) = parked_world("parked-narrow", true);
        let app = w.app();
        for (cols, rows) in [(110, 24), (80, 24), (60, 16)] {
            let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
            let shown = draw(&mut term, &app);
            let header = shown.lines().find(|l| l.contains("STATUS")).unwrap();
            let row = shown.lines().find(|l| l.contains("moved away")).unwrap_or_else(|| panic!("{cols}x{rows}\n{shown}"));
            let col = |l: &str, needle: &str| l[..l.find(needle).unwrap()].chars().count();
            assert_eq!(col(header, "STATUS"), col(row, "moved away"), "{cols}x{rows}: the words sit under their heading");
        }
    }

    const SPIN: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

    // --- the settings screen ------------------------------------------------------------------------

    use crate::settings::{self, Settings};

    /// The General tab: the one the settings open on.
    static ITEMS: std::sync::LazyLock<&'static [settings::Item]> = std::sync::LazyLock::new(|| &settings::TABS[0].items);
    /// The General tab's Programs entries ("Use Claude Code", "Use Codex"), above the Theme entry.
    const P: usize = 2;

    fn config_file(w: &World) -> std::path::PathBuf {
        w.config.join("settings.json")
    }

    fn on_disk(w: &World) -> Settings {
        settings::load(&w.config).0
    }

    fn hit_rect(app: &App, want: impl Fn(&Hit) -> bool) -> ratatui::layout::Rect {
        app.hits.borrow().iter().find(|(_, h)| want(h)).map(|(r, _)| *r).expect("not drawn")
    }

    fn hover(app: &mut App, x: u16, y: u16) {
        app.on_mouse(MouseEvent { kind: MouseEventKind::Moved, column: x, row: y, modifiers: KeyModifiers::NONE });
    }

    /// The settings open on "Stop idle sessions after", the first of the numeric entries (the Theme entry above
    /// it, below the Programs, is what the settings open on).
    fn open_numeric_settings(app: &mut App) {
        ch(app, 's');
        assert_eq!(app.setting, P, "the settings open on the Theme entry");
        key(app, KeyCode::Down);
        assert_eq!(app.setting, P + 1);
    }

    /// A screen on which the settings are open on the first numeric entry, after `draw` so clicks have
    /// somewhere to land.
    fn settings_screen(w: &World, cols: u16, rows: u16) -> (App, Terminal<TestBackend>) {
        let mut app = w.app();
        open_numeric_settings(&mut app);
        let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
        draw(&mut term, &app);
        (app, term)
    }

    #[test]
    fn s_opens_the_settings_and_esc_s_q_and_ctrl_c_go_back_without_quitting() {
        let w = World::new("set-open");
        let mut app = w.app();
        assert!(matches!(app.mode, Mode::List));
        ch(&mut app, 's');
        assert!(matches!(app.mode, Mode::Settings));
        key(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::List));
        ch(&mut app, 's');
        ch(&mut app, 's');
        assert!(matches!(app.mode, Mode::List), "s again goes back");
        ch(&mut app, 's');
        ch(&mut app, 'q');
        assert!(matches!(app.mode, Mode::List) && !app.quit, "q goes back, it does not quit");
        ch(&mut app, 's');
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(matches!(app.mode, Mode::List) && !app.quit, "ctrl+c goes back");
        ch(&mut app, 'q');
        assert!(app.quit, "and on the list q still quits");
    }

    #[test]
    fn the_footer_hint_opens_the_settings_and_the_screen_has_a_back_button() {
        let w = World::new("set-footer");
        let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
        let mut app = w.app();
        draw(&mut term, &app);
        click_action(&mut app, Action::Settings);
        assert!(matches!(app.mode, Mode::Settings));
        assert!(draw(&mut term, &app).contains("Settings"));
        click_action(&mut app, Action::Cancel);
        assert!(matches!(app.mode, Mode::List));
        assert!(draw(&mut term, &app).contains("s settings"));
    }

    #[test]
    fn the_settings_are_not_reachable_while_filtering_in_a_dialog_the_picker_or_an_attached_session() {
        let w = World::new("set-unreachable");
        w.transcript("aaa", "/srv/a", "hello", 5);
        let mut app = w.app();

        ch(&mut app, '/');
        ch(&mut app, 's');
        assert!(matches!(app.mode, Mode::Search) && app.filter == "s", "an s typed into the filter is text");
        key(&mut app, KeyCode::Esc);

        ch(&mut app, 'n');
        assert!(matches!(app.mode, Mode::Picker));
        ch(&mut app, 's');
        assert!(matches!(app.mode, Mode::Picker) && app.picker.input == "s");
        key(&mut app, KeyCode::Esc);

        app.mode = Mode::Confirm(Confirm::Archive { id: "aaa".into(), name: "a".into(), busy: false, outside: false });
        ch(&mut app, 's');
        assert!(matches!(app.mode, Mode::Confirm(_)));
        key(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::List));

        app.view_conversation_for_test("aaa", 1);
        ch(&mut app, 's');
        assert!(matches!(app.mode, Mode::Attached(_)), "in a session the key belongs to Claude");
        // The footer button can't be clicked from a dialog either: the dialog owns the clicks.
        assert!(app.hits.borrow().iter().all(|(_, h)| !matches!(h, Hit::Action(Action::Settings))));
    }

    #[test]
    fn keys_step_through_the_choices_and_each_change_is_saved_at_once() {
        let w = World::new("set-keys");
        w.settings(120, 2);
        let mut app = w.app();
        open_numeric_settings(&mut app);

        key(&mut app, KeyCode::Right);
        assert_eq!(on_disk(&w), Settings { idle_stop_minutes: 240, auto_archive_days: 2, ..Settings::default() });
        ch(&mut app, 'l');
        ch(&mut app, '+');
        ch(&mut app, '=');
        assert_eq!(on_disk(&w).idle_stop_minutes, 1440, "right, l, + and = all step forward, and stop at the last choice");
        key(&mut app, KeyCode::Left);
        ch(&mut app, 'h');
        ch(&mut app, '-');
        assert_eq!(on_disk(&w).idle_stop_minutes, 120, "left, h and - step back");
        for _ in 0..9 {
            key(&mut app, KeyCode::Left);
        }
        assert_eq!(on_disk(&w).idle_stop_minutes, 0, "Off is the first choice; the arrows stop there");

        // Enter goes to the next choice and wraps from the last back to Off.
        key(&mut app, KeyCode::Enter);
        assert_eq!(on_disk(&w).idle_stop_minutes, 30);
        for _ in 0..5 {
            key(&mut app, KeyCode::Enter);
        }
        assert_eq!(on_disk(&w).idle_stop_minutes, 1440);
        key(&mut app, KeyCode::Enter);
        assert_eq!(on_disk(&w).idle_stop_minutes, 0, "enter wraps");

        // Down / j select the next setting, up / k the previous; the second one changes the other value.
        key(&mut app, KeyCode::Down);
        assert_eq!(app.setting, P + 2);
        key(&mut app, KeyCode::Right);
        assert_eq!(
            on_disk(&w),
            Settings { idle_stop_minutes: 0, auto_archive_days: 7, ..Settings::default() },
            "the other setting is left alone"
        );
        key(&mut app, KeyCode::Up);
        assert_eq!(app.setting, P + 1);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Up);
        assert_eq!(app.setting, P + 1);
        assert_eq!(
            ITEMS.len(),
            7,
            "the General tab: the two programs, the theme, two numbers, the check for a newer platoon and the diagnostics log"
        );
        for _ in 0..ITEMS.len() + 2 {
            key(&mut app, KeyCode::Down);
        }
        assert_eq!(app.setting, ITEMS.len() - 1, "no wrapping past the last");
        for _ in 0..ITEMS.len() + 2 {
            key(&mut app, KeyCode::Up);
        }
        assert_eq!(app.setting, 0, "nor before the first");
    }

    #[test]
    fn a_value_that_is_not_a_step_is_shown_as_it_is_kept_on_open_and_stepped_to_the_nearest() {
        let w = World::new("set-odd");
        let text = "{\n  \"idle_stop_minutes\": 90,\n  \"auto_archive_days\": 5,\n  \"mine\": true\n}\n";
        std::fs::write(config_file(&w), text).unwrap();
        let (mut app, mut term) = settings_screen(&w, 100, 30);
        let shown = draw(&mut term, &app);
        assert!(shown.contains("90 minutes") && shown.contains("5 days"), "{shown}");
        for k in [KeyCode::Down, KeyCode::Up, KeyCode::Esc] {
            key(&mut app, k);
        }
        open_numeric_settings(&mut app);
        assert_eq!(std::fs::read_to_string(config_file(&w)).unwrap(), text, "looking at the screen never rewrites the file");

        key(&mut app, KeyCode::Right);
        assert_eq!(on_disk(&w).idle_stop_minutes, 120, "90 goes up to the next step");
        std::fs::write(config_file(&w), text).unwrap();
        key(&mut app, KeyCode::Left);
        assert_eq!(on_disk(&w).idle_stop_minutes, 60, "and down to the previous one");
        std::fs::write(config_file(&w), text).unwrap();
        key(&mut app, KeyCode::Enter);
        assert_eq!(on_disk(&w).idle_stop_minutes, 120, "enter goes to the next step too");
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Left);
        assert_eq!(on_disk(&w).auto_archive_days, 2, "5 days steps down to 2");
    }

    #[test]
    fn the_file_keeps_keys_platoon_does_not_know_including_nested_ones() {
        let w = World::new("set-unknown");
        std::fs::write(
            config_file(&w),
            r#"{"version":1,"theme":{"name":"latte","tweaks":[1,{"a":null}]},"idle_stop_minutes":120,"note":"hi"}"#,
        )
        .unwrap();
        let (mut app, _) = settings_screen(&w, 100, 30);
        key(&mut app, KeyCode::Right);
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(config_file(&w)).unwrap()).unwrap();
        assert_eq!(v["theme"], serde_json::json!({"name":"latte","tweaks":[1,{"a":null}]}));
        assert_eq!(
            (v["note"].as_str(), v["idle_stop_minutes"].as_u64(), v["auto_archive_days"].as_u64()),
            (Some("hi"), Some(240), Some(2))
        );
    }

    // --- tabs as chips, and settings as a popup ---------------------------------------------------

    fn mixed_app(name: &str) -> (World, App) {
        let (w, _) = mixed(name);
        let app = w.app();
        (w, app)
    }

    /// Column ranges of the heavy underline (`━`) on the rule row.
    fn underline(shown: &str) -> Option<(usize, usize)> {
        let row = shown.lines().nth(1)?; // header, rule
        let cells: Vec<char> = row.chars().collect();
        let first = cells.iter().position(|&c| c == '━')?;
        let last = cells.iter().rposition(|&c| c == '━')?;
        Some((first, last))
    }

    #[test]
    fn the_current_tab_is_underlined_and_the_underline_follows_the_tab() {
        let (_w, mut app) = mixed_app("tabs-rule");
        let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
        let shown = draw(&mut term, &app);
        assert!(flat(&shown).contains("1 Sessions 2 Projects 3 Tasks"), "{shown}");
        let rule = shown.lines().nth(1).unwrap().to_string();
        assert!(rule.contains('─') && rule.contains('━'), "a quiet rule with a heavy underline: {rule}");
        let (a_first, a_last) = underline(&shown).unwrap();
        // The underline covers the whole first tab, and not the other tab.
        let header: Vec<char> = shown.lines().next().unwrap().chars().collect();
        let col_of = |needle: &str| {
            let text: String = header.iter().collect();
            text[..text.find(needle).unwrap()].chars().count()
        };
        assert!(a_first <= col_of("1 Sessions") && a_last >= col_of("1 Sessions") + "1 Sessions".len(), "{shown}");
        assert!(a_last < col_of("2 Projects"), "the other tab is not underlined: {shown}");

        ch(&mut app, '2');
        let shown2 = draw(&mut term, &app);
        let (b_first, _) = underline(&shown2).unwrap();
        assert!(b_first >= col_of("2 Projects") - 1, "the underline moved to the Projects tab: {shown2}");
        assert!(b_first > a_last, "{shown2}");
    }

    /// The cells of one row of the screen.
    fn row_cells(term: &Terminal<TestBackend>, y: u16) -> Vec<ratatui::buffer::Cell> {
        let buf = term.backend().buffer();
        (0..buf.area.width).map(|x| buf[(x, y)].clone()).collect()
    }

    fn cell_index(cells: &[ratatui::buffer::Cell], text: &str) -> usize {
        let joined: String = cells.iter().map(|c| c.symbol().to_string()).collect();
        joined[..joined.find(text).unwrap()].chars().count()
    }

    #[test]
    fn the_tabs_carry_no_count_and_their_key_reads_as_a_hint() {
        let (_w, app) = mixed_app("tabs-plain");
        let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
        draw(&mut term, &app);
        let t = app.theme.clone();
        let cells = row_cells(&term, 0);
        let header: String = cells.iter().map(|c| c.symbol()).collect();
        // The counts are on the Active/Archived switch under them, so the tabs are just a key and a name.
        assert!(header.contains("1 Sessions   2 Projects   3 Tasks"), "{header}");
        for (key, name) in [("1", "1 Sessions"), ("2", "2 Projects"), ("3", "3 Tasks")] {
            let at = cell_index(&cells, name);
            let hint = if cells[at].bg == t.surface { t.readable_on(t.subtext, t.surface, crate::theme::MIN_SUBTEXT) } else { t.subtext };
            assert_eq!((cells[at].symbol(), cells[at].fg), (key, hint), "{name}: the key reads as a hint, current or not");
            assert!(crate::theme::contrast(cells[at].fg, cells[at].bg) >= crate::theme::MIN_SUBTEXT, "{name}");
        }
        let current = cell_index(&cells, "Sessions");
        assert!(
            cells[current].modifier.contains(ratatui::style::Modifier::BOLD) && cells[current].fg == t.text,
            "the current tab's name is lit"
        );
    }

    #[test]
    fn the_count_is_a_badge_set_apart_from_the_name_and_the_switch_still_clicks() {
        let (_w, mut app) = mixed_app("tabs-badge");
        let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
        draw(&mut term, &app);
        let t = app.theme.clone();
        // The Active/Archived switch is at the right of the rule under the tabs.
        let cells = row_cells(&term, 1);
        let name_end = cell_index(&cells, "Active") + "Active".len();
        // Between the name and the badge there is air: cells on the chip's own background, not the badge's.
        let badge_at = (name_end..cells.len()).find(|&i| cells[i].bg == t.accent).expect("the badge");
        assert!(badge_at - name_end >= 1, "a gap between the name and the badge");
        assert!(cells[name_end..badge_at].iter().all(|c| c.symbol() == " " && c.bg != t.accent));
        // Square badge: a padded block, the count dark on the accent color.
        let three = (name_end..cells.len()).find(|&i| cells[i].symbol() == "3").unwrap();
        assert_eq!((cells[three].bg, cells[three].fg), (t.accent, t.bg));
        assert_eq!((cells[three - 1].symbol(), cells[three + 1].symbol()), (" ", " "), "padded");
        // The badge of the one you're not on is quieter.
        let arch_end = cell_index(&cells, "Archived") + "Archived".len();
        let zero = (arch_end..cells.len()).find(|&i| cells[i].symbol() == "0").unwrap();
        assert_eq!(cells[zero].bg, t.surface);
        // Clicking anywhere on a chip, badge included, switches to it.
        click(&mut app, zero as u16, 1);
        assert_eq!(app.tab, 1);
    }

    #[test]
    fn where_the_terminal_can_draw_them_badges_are_round() {
        let (_w, mut app) = mixed_app("tabs-round");
        app.rounded = true;
        let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
        draw(&mut term, &app);
        let t = app.theme.clone();
        let cells = row_cells(&term, 1);
        let (left, right) = ('\u{e0b6}'.to_string(), '\u{e0b4}'.to_string());
        let three = cells.iter().position(|c| c.symbol() == "3").unwrap();
        // Round caps hug the count: the left cap is accent on the chip's color, the right one accent on the page.
        assert_eq!((cells[three - 1].symbol(), cells[three + 1].symbol()), (left.as_str(), right.as_str()));
        assert_eq!((cells[three - 1].fg, cells[three + 1].fg, cells[three + 1].bg), (t.accent, t.accent, t.bg));
        assert_eq!((cells[three].bg, cells[three].fg), (t.accent, t.bg), "the count sits on the badge color");
        // Same air before the badge, and the text still reads as before.
        let name_end = cell_index(&cells, "Active") + "Active".len();
        assert!(three - 1 - name_end >= 1 && cells[name_end].symbol() == " ");
        let plain: String = cells.iter().map(|c| c.symbol()).collect::<String>().replace(['\u{e0b6}', '\u{e0b4}'], "");
        assert!(flat(&plain).contains("Active 3") && flat(&plain).contains("Archived 0"), "{plain}");
        // A click on the cap switches.
        let caps: Vec<usize> = cells.iter().enumerate().filter(|(_, c)| c.symbol() == left).map(|(i, _)| i).collect();
        click(&mut app, caps[1] as u16, 1);
        assert_eq!(app.tab, 1, "the Archived chip's cap");
        draw(&mut term, &app);
        click(&mut app, caps[0] as u16, 1);
        assert_eq!(app.tab, 0, "the Active chip's cap");
    }

    #[test]
    fn terminals_that_bring_their_own_symbols_get_round_badges_and_others_do_not() {
        // The detection is a pure function of the environment; check its rules without touching ours.
        let g = |pairs: &[(&str, &str)]| {
            crate::app::detect_rounded_with(&|k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string()))
        };
        assert!(g(&[("TERM_PROGRAM", "ghostty")]) && g(&[("TERM", "xterm-kitty")]) && g(&[("WEZTERM_EXECUTABLE", "/x")]));
        assert!(!g(&[("TERM_PROGRAM", "Apple_Terminal"), ("TERM", "xterm-256color")]) && !g(&[]));
        assert!(g(&[("PLATOON_BADGES", "rounded")]) && !g(&[("PLATOON_BADGES", "square"), ("TERM_PROGRAM", "ghostty")]));
    }

    #[test]
    fn j_and_k_move_like_the_arrows_and_are_letters_in_the_filter() {
        let (_w, mut app) = mixed_app("jk");
        assert_eq!(app.selected, 0);
        ch(&mut app, 'j');
        assert_eq!(app.selected, 1, "j moves down");
        ch(&mut app, 'k');
        assert_eq!(app.selected, 0, "k moves up");
        key(&mut app, KeyCode::Down);
        assert_eq!(app.selected, 1);
        key(&mut app, KeyCode::Up);
        assert_eq!(app.selected, 0);
        // Typing them into the filter still works: they are just letters there.
        ch(&mut app, '/');
        ch(&mut app, 'j');
        assert_eq!(app.filter, "j");
        // The footer keeps the arrows' hint (j/k are the arrows' aliases, not a hint of their own).
        key(&mut app, KeyCode::Esc);
        let mut term = Terminal::new(TestBackend::new(160, 24)).unwrap();
        let shown = draw(&mut term, &app);
        assert!(!shown.contains("j/k") && shown.contains("↑↓"), "{shown}");
    }

    #[test]
    fn big_counts_and_the_usage_readout_share_a_narrow_header_without_clipping_either_tab() {
        use crate::model::{PlanUsage, Window};
        let (_w, mut app) = mixed_app("tabs-narrow");
        app.counts = vec![1234, 3];
        app.pcounts = [5678, 0];
        // The tabs carry no counts (the Active/Archived switch on the rule under them does), so big counts never
        // crowd the readout: it shrinks to bare percentages beside the tabs, and has its bars where there is room.
        for (cols, bars) in [(80u16, false), (100, true), (120, true), (140, true)] {
            app.usage = crate::usage::Usage::Ready(PlanUsage {
                five_hour: Some(Window { pct: 47, resets_at: 9_999_999_999 }),
                seven_day: Some(Window { pct: 21, resets_at: 9_999_999_999 }),
            });
            let mut term = Terminal::new(TestBackend::new(cols, 24)).unwrap();
            let shown = draw(&mut term, &app);
            let header = shown.lines().next().unwrap();
            assert!(flat(header).contains("1 Sessions 2 Projects 3 Tasks"), "{cols}: {header}");
            assert!(!header.contains("1234") && !header.contains("5678"), "{cols}: no counts in the tabs: {header}");
            assert!(flat(shown.lines().nth(1).unwrap()).contains("Active 1234 Archived 3"), "{cols}: the counts are on the rule: {shown}");
            assert!(
                header.contains("5h") && header.contains("47%") && header.contains("7d") && header.contains("21%"),
                "{cols}: usage still readable: {header}"
            );
            assert_eq!(header.contains('━'), bars, "{cols}: full bars only when there is room: {header}");
        }
        // With nothing to spare the readout gives way, never the tabs.
        app.usage = crate::usage::Usage::NotSetUp;
        let mut term = Terminal::new(TestBackend::new(60, 24)).unwrap();
        let header = draw(&mut term, &app).lines().next().unwrap().to_string();
        assert!(flat(&header).contains("1 Sessions 2 Projects 3 Tasks"), "{header}");
    }

    #[test]
    fn a_five_hour_window_that_has_not_started_shows_zero_not_n_a() {
        // Right after a reset Claude Code reports the weekly figure but leaves the 5-hour one out.
        let (w, mut app) = mixed_app("usage-zero");
        let cache = w.base.join("usage-zero-cache");
        std::fs::create_dir_all(cache.join("status")).unwrap();
        let json = serde_json::json!({"ts": 1_000, "context_pct": 5.0, "five_hour": null, "seven_day": {"pct": 3, "resets_at": 9_000}});
        std::fs::write(cache.join("status/aaaa.json"), json.to_string()).unwrap();
        // (At 80 columns, beside three tabs, both figures fit without their bars.)
        for (cols, bars, weekly) in [(80u16, false, true), (100, true, true), (140, true, true)] {
            app.usage = crate::usage::Usage::Ready(crate::usage::load(&cache, 1_001).plan.unwrap());
            let mut term = Terminal::new(TestBackend::new(cols, 24)).unwrap();
            let header = draw(&mut term, &app).lines().next().unwrap().to_string();
            assert!(header.contains("5h") && header.contains("0%"), "{cols}: {header}");
            assert_eq!(header.contains("7d") && header.contains("3%"), weekly, "{cols}: {header}");
            assert!(!header.contains("n/a"), "{cols}: {header}");
            assert_eq!(header.contains('━'), bars, "{cols}: {header}");
        }
    }

    #[test]
    fn settings_float_over_a_faded_list_and_a_click_outside_closes_them() {
        let (_w, mut app) = mixed_app("popup");
        ch(&mut app, 's');
        let mut term = Terminal::new(TestBackend::new(120, 100)).unwrap();
        let shown = draw(&mut term, &app);
        // The list is still there behind it...
        assert!(shown.contains("platoon") && flat(&shown).contains("Active 3") && shown.contains("Settings"), "{shown}");
        // ...the popup is a window with room around it, not the whole screen...
        let top = shown.lines().position(|l| l.contains("Settings")).unwrap();
        assert!(top >= 1, "{shown}");
        let border = shown.lines().nth(top).unwrap();
        assert!(border.starts_with("  "), "the popup has room around it:\n{shown}");
        assert!(shown.lines().any(|l| l.trim_start().starts_with('╰')), "{shown}");
        // ...and what is behind it has faded: the header's accent title is now the muted border color.
        let buf = term.backend().buffer().clone();
        let row: String = (0..buf.area.width).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        let x = row[..row.find("platoon").unwrap()].chars().count() as u16;
        assert_eq!(buf[(x, 0)].fg, app.theme.border, "the backdrop is muted");
        // Clicking the faded list behind the popup closes it; clicking inside does not.
        let inside = app.hits.borrow().iter().find(|(_, h)| matches!(h, Hit::Setting(0))).map(|(r, _)| *r).unwrap();
        click(&mut app, inside.x + 2, inside.y);
        assert!(matches!(app.mode, Mode::Settings), "a click inside the popup keeps it open");
        click(&mut app, 3, 1);
        assert!(matches!(app.mode, Mode::List), "a click outside closes it");
    }

    #[test]
    fn the_popup_is_sized_to_its_contents_and_uses_every_row_on_a_short_terminal() {
        let (_w, mut app) = mixed_app("popup-size");
        ch(&mut app, 's');
        // Tall terminal: floats with air above and below.
        let mut tall = Terminal::new(TestBackend::new(110, 110)).unwrap();
        let shown = draw(&mut tall, &app);
        let (first, last) = (
            shown.lines().position(|l| l.contains("Settings")).unwrap(),
            shown.lines().enumerate().filter(|(_, l)| l.contains("esc back")).map(|(i, _)| i).last().unwrap(),
        );
        assert!(first > 1 && shown.lines().count() - last > 3, "not stretched to the screen:\n{shown}");
        // 100x100 and 140x100: every explanation fits.
        for cols in [100, 140] {
            let mut term = Terminal::new(TestBackend::new(cols, 100)).unwrap();
            let text = popup_flat(&draw(&mut term, &app));
            for item in ITEMS.iter() {
                assert!(text.contains(item.explain), "{}: cut off at {cols}x100", item.name);
            }
        }
        // 80x24 has room for the selected entry and its explanation, whole, and a window of the others: the list
        // scrolls as the selection moves, and only the selected entry is explained.
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        ch(&mut app, 'g');
        for (i, item) in ITEMS.iter().enumerate() {
            while app.setting < i {
                key(&mut app, KeyCode::Down);
            }
            let shown = draw(&mut term, &app);
            let text = popup_flat(&shown);
            assert!(
                text.contains(item.name) && text.contains(item.explain),
                "{} is shown with its explanation at 80x24:\n{shown}",
                item.name
            );
            assert!(text.contains("esc back") && !text.contains("Used today"), "{shown}");
            for (j, other) in ITEMS.iter().enumerate().filter(|(j, _)| *j != i) {
                assert!(!text.contains(other.explain), "{} is explained only while selected ({}, {j})", other.name, item.name);
            }
            assert!(shown.matches('›').count() >= ITEMS.len(), "every entry of the tab stays in view:\n{shown}");
        }
    }

    #[test]
    fn values_read_plainly_and_off_is_muted() {
        let w = World::new("set-values");
        w.settings(0, 2);
        let (mut app, mut term) = settings_screen(&w, 100, 30);
        let shown = draw(&mut term, &app);
        assert!(shown.contains("Off") && shown.contains("2 days"), "{shown}");
        let buf = term.backend().buffer().clone();
        let cell_fg = |word: &str| {
            (0..buf.area.height).find_map(|y| {
                let line: Vec<String> = (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
                // `find` gives a byte offset; a cell is one char, and box-drawing characters take three bytes.
                let text = line.join("");
                text.find(word).map(|i| buf[(text[..i].chars().count() as u16, y)].fg)
            })
        };
        let t = &app.theme;
        assert_eq!(cell_fg("Off"), Some(t.readable_on(t.dim, t.surface, crate::theme::MIN_DIM)), "Off is muted (on the selected row)");
        assert_eq!(cell_fg("2 days"), Some(app.theme.accent), "an active value stands out");
        // Every step reads as words a person would say.
        let labels: Vec<Vec<String>> = ITEMS[P + 1..].iter().map(|i| i.choices.iter().map(|&c| (i.label)(c)).collect()).collect();
        assert_eq!(labels[0], ["Off", "30 minutes", "1 hour", "2 hours", "4 hours", "8 hours", "1 day"]);
        assert_eq!(labels[1], ["Off", "1 day", "2 days", "7 days", "14 days", "30 days"]);
        w.settings(1440 * 3, 1);
        app.refresh_for_test();
        assert!(draw(&mut term, &app).contains("3 days"), "a value from the file in other units still reads naturally");
    }

    #[test]
    fn the_mouse_hovers_selects_and_steps() {
        let w = World::new("set-mouse");
        w.settings(120, 2);
        let (mut app, mut term) = settings_screen(&w, 100, 30);
        let surface = app.theme.surface;
        let bg = app.theme.bg;
        let row1 = hit_rect(&app, |h| matches!(h, Hit::Setting(i) if *i == P + 2));

        // Hover lights the row up; the selected row is lit too.
        assert_eq!(term.backend().buffer()[(row1.x + 5, row1.y)].bg, bg);
        hover(&mut app, row1.x + 5, row1.y);
        draw(&mut term, &app);
        assert_eq!(term.backend().buffer()[(row1.x + 5, row1.y)].bg, surface, "hovered row");
        let row0 = hit_rect(&app, |h| matches!(h, Hit::Setting(i) if *i == P + 1));
        assert_eq!(term.backend().buffer()[(row0.x + 5, row0.y)].bg, surface, "selected row");

        // A click on a row selects it and changes nothing.
        click(&mut app, row1.x + 5, row1.y);
        assert_eq!((app.setting, on_disk(&w)), (P + 2, Settings { idle_stop_minutes: 120, auto_archive_days: 2, ..Settings::default() }));
        draw(&mut term, &app);

        // The buttons step; clicking one on another row selects that row and steps it.
        let next0 = hit_rect(&app, |h| matches!(h, Hit::SettingStep(i, settings::Step::Next) if *i == P + 1));
        click(&mut app, next0.x + 1, next0.y);
        assert_eq!((app.setting, on_disk(&w).idle_stop_minutes), (P + 1, 240));
        draw(&mut term, &app);
        let prev1 = hit_rect(&app, |h| matches!(h, Hit::SettingStep(i, settings::Step::Prev) if *i == P + 2));
        click(&mut app, prev1.x + 1, prev1.y);
        assert_eq!((app.setting, on_disk(&w).auto_archive_days), (P + 2, 1));
        draw(&mut term, &app);
        let prev1 = hit_rect(&app, |h| matches!(h, Hit::SettingStep(i, settings::Step::Prev) if *i == P + 2));
        click(&mut app, prev1.x + 1, prev1.y);
        assert_eq!(on_disk(&w).auto_archive_days, 0);
        draw(&mut term, &app);

        // Clicking the value cycles (and wraps).
        let value1 = hit_rect(&app, |h| matches!(h, Hit::SettingStep(i, settings::Step::Cycle) if *i == P + 2));
        click(&mut app, value1.x + 1, value1.y);
        assert_eq!(on_disk(&w).auto_archive_days, 1);
        draw(&mut term, &app);
        for _ in 0..5 {
            click(&mut app, value1.x + 1, value1.y);
        }
        assert_eq!(on_disk(&w).auto_archive_days, 0, "wrapped back to Off");

        // The wheel moves between the settings.
        app.on_mouse(MouseEvent { kind: MouseEventKind::ScrollUp, column: 5, row: 5, modifiers: KeyModifiers::NONE });
        assert_eq!(app.setting, P + 1);
    }

    #[test]
    fn the_step_buttons_of_the_first_and_last_choice_are_muted() {
        let w = World::new("set-ends");
        w.settings(0, 30);
        let (app, term) = settings_screen(&w, 100, 30);
        let border = app.theme.border;
        let accent = app.theme.accent;
        let buf = term.backend().buffer();
        let prev0 = hit_rect(&app, |h| matches!(h, Hit::SettingStep(i, settings::Step::Prev) if *i == P + 1));
        let next0 = hit_rect(&app, |h| matches!(h, Hit::SettingStep(i, settings::Step::Next) if *i == P + 1));
        let next1 = hit_rect(&app, |h| matches!(h, Hit::SettingStep(i, settings::Step::Next) if *i == P + 2));
        assert_eq!(buf[(prev0.x + 1, prev0.y)].fg, border, "nothing below Off");
        assert_eq!(buf[(next0.x + 1, next0.y)].fg, accent);
        assert_eq!(buf[(next1.x + 1, next1.y)].fg, border, "nothing above 30 days");
    }

    #[test]
    fn a_failed_save_says_why_and_keeps_showing_what_is_on_disk() {
        let w = World::new("set-fail");
        // The settings directory can't be created: something that isn't a directory is in the way.
        let blocker = w.base.join("blocker");
        std::fs::write(&blocker, "x").unwrap();
        let mut app = App::with_dirs(crate::agents::claude::Reader::with_root(w.root.clone()), w.cache.clone(), blocker.join("config"));
        open_numeric_settings(&mut app);
        key(&mut app, KeyCode::Right);
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let shown = draw(&mut term, &app);
        assert!(flat(&shown).contains("Couldn't save the settings: Not a directory"), "{shown}");
        assert!(shown.contains("2 hours") && !shown.contains("4 hours"), "the value that is really in effect stays on screen:\n{shown}");
        assert!(!blocker.join("config").exists());

        // A read-only directory with a file in it: the old value is what stays.
        if unsafe { libc::geteuid() } != 0 {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(config_file(&w), r#"{"idle_stop_minutes":60}"#).unwrap();
            let mut app = w.app();
            open_numeric_settings(&mut app);
            std::fs::set_permissions(&w.config, std::fs::Permissions::from_mode(0o555)).unwrap();
            key(&mut app, KeyCode::Right);
            std::fs::set_permissions(&w.config, std::fs::Permissions::from_mode(0o755)).unwrap();
            let shown = draw(&mut term, &app);
            assert!(flat(&shown).contains("Couldn't save the settings: Permission denied"), "{shown}");
            assert!(shown.contains("1 hour") && !shown.contains("2 hours"), "{shown}");
            assert_eq!(on_disk(&w).idle_stop_minutes, 60);
            assert!(!w.config.join("settings.json.tmp").exists());
            // The next change that can be saved clears the message.
            key(&mut app, KeyCode::Right);
            assert_eq!(on_disk(&w).idle_stop_minutes, 120);
            assert!(!draw(&mut term, &app).contains("Couldn't save"));
        }
    }

    #[test]
    fn a_corrupt_file_opens_on_the_standard_values_and_the_first_change_keeps_a_copy() {
        let w = World::new("set-corrupt");
        std::fs::write(config_file(&w), "{ this is not json").unwrap();
        let (mut app, mut term) = settings_screen(&w, 100, 30);
        let shown = draw(&mut term, &app);
        assert!(shown.contains("2 hours") && shown.contains("2 days"), "{shown}");
        assert!(flat(&shown).contains("settings file couldn't be read"), "the user is told why:\n{shown}");
        assert_eq!(std::fs::read_to_string(config_file(&w)).unwrap(), "{ this is not json", "opening changes nothing");
        assert!(!w.config.join("settings.json.bak").exists());

        key(&mut app, KeyCode::Right);
        assert_eq!(std::fs::read_to_string(w.config.join("settings.json.bak")).unwrap(), "{ this is not json");
        assert_eq!(settings::load(&w.config), (Settings { idle_stop_minutes: 240, auto_archive_days: 2, ..Settings::default() }, false));
        assert!(!flat(&draw(&mut term, &app)).contains("couldn't be read"), "healthy again");
    }

    #[test]
    fn an_edit_made_elsewhere_shows_up_while_the_screen_is_open() {
        let w = World::new("set-external");
        w.settings(120, 2);
        let (mut app, mut term) = settings_screen(&w, 100, 30);
        w.settings(480, 14);
        app.refresh_for_test();
        let shown = draw(&mut term, &app);
        assert!(shown.contains("8 hours") && shown.contains("14 days"), "{shown}");
    }

    const THREE_DAYS_MINS: u64 = 3 * 24 * 60;

    #[test]
    fn turning_auto_archive_on_from_the_screen_takes_effect_without_a_restart_and_says_so_there() {
        let w = World::new("set-live-on");
        w.settings(120, 0);
        w.transcript("old", "/srv/a", "old work", THREE_DAYS_MINS);
        w.transcript("fresh", "/srv/a", "new work", 30);
        let (mut app, mut term) = settings_screen(&w, 100, 30);
        assert_eq!(app.counts, [2, 0]);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Right); // Off -> 1 day
        assert!(app.settling());
        app.refresh_for_test();
        assert_eq!(app.counts, [2, 0], "held while the value may still be changing");
        app.settle_for_test();
        app.refresh_for_test(); // the very next refresh, not a timed check a minute later
        assert_eq!(app.counts, [1, 1]);
        assert!(!app.settling());
        let shown = draw(&mut term, &app);
        assert!(flat(&shown).contains("Auto-archived 1 session idle for 1+ day. Find it in Archived."), "{shown}");
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Right);
        assert_eq!(ids(&app), ["old"]);
    }

    #[test]
    fn turning_auto_archive_off_from_the_screen_stops_it_archiving_even_though_the_way_passes_through_1_day() {
        let later = crate::usage::now() + 5 * 86_400;
        let run = |steps: &[KeyCode]| {
            let w = World::new("set-live-off");
            w.settings(120, 2);
            w.transcript("recent", "/srv/a", "work", 25 * 60); // over a day old: 1 day would archive it, 2 days not yet
            let mut app = w.app();
            assert_eq!(app.counts, [1, 0]);
            open_numeric_settings(&mut app);
            key(&mut app, KeyCode::Down);
            for k in steps {
                key(&mut app, *k);
                app.refresh_for_test(); // platoon refreshes every second while the user is choosing
            }
            app.settle_for_test();
            app.auto_archive_at_for_test(later);
            (app.counts.clone(), on_disk(&w).auto_archive_days)
        };
        assert_eq!(run(&[]), (vec![0, 1], 2), "control: time passes and the conversation is archived");
        assert_eq!(run(&[KeyCode::Left, KeyCode::Left]), (vec![1, 0], 0), "Off: nothing is archived, not even at the stop on the way");
        assert_eq!(run(&[KeyCode::Left]), (vec![0, 1], 1), "stopping at 1 day does archive, once it has settled");
    }

    #[test]
    fn the_settings_screen_reads_well_at_every_size() {
        let w = World::new("set-render");
        w.settings(120, 2);
        let app = {
            let mut a = w.app();
            ch(&mut a, 's');
            a
        };
        let file = "settings.json";
        for (cols, rows) in [(80, 24), (100, 30), (140, 30), (100, 100), (140, 100), (60, 20), (80, 20)] {
            let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
            let shown = draw(&mut term, &app);
            let text = popup_flat(&shown);
            let ctx = format!("{cols}x{rows}\n{shown}");
            // Where every explanation doesn't fit with room to breathe, the selected one does (and is whole).
            let roomy = rows >= 100;
            // Below 30 rows the list scrolls: the selected entry (Theme, which the settings open on) is whole, the others
            // come as far as there is room.
            let all_names = rows >= 30;
            for (i, item) in ITEMS.iter().enumerate() {
                assert!(text.contains(item.name) || !(all_names || i == app.setting), "{ctx}");
                let must_explain = roomy || i == app.setting;
                assert!(!must_explain || text.contains(item.explain), "explanation cut off or mangled at {ctx}");
            }
            for want in ["2 hours", "2 days", "Once a day", "‹", "›", "enter next choice", "esc back", file] {
                // At 60 columns the back key's longer name leaves no room for this one (it goes first).
                if want == "enter next choice" && cols < 70 {
                    continue;
                }
                assert!(text.contains(want), "{want:?} missing at {ctx}");
            }
            assert!(rows < 30 || text.contains("Changes are saved and apply immediately."), "{ctx}");
            assert!(!text.contains("Used today"), "the day's token count is on the smart features tab: {ctx}");
            let lower = text.to_lowercase();
            for jargon in ["sigterm", "host", "index", "pid", "json:"] {
                assert!(!lower.contains(jargon), "{jargon:?} in {ctx}");
            }
            // Nothing runs into the frame: every text line keeps a blank before the right border.
            for line in shown.lines().filter(|l| l.starts_with('│')) {
                assert!(line.trim_end_matches('│').ends_with(' '), "text touches the border at {ctx}");
            }
        }
        // The path is written with ~ for the home directory.
        let mut under_home = w.app();
        under_home.home = w.config.parent().unwrap().to_string_lossy().into_owned();
        ch(&mut under_home, 's');
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let text = draw(&mut term, &under_home);
        assert!(text.contains("Settings file: ~/config/settings.json"), "{text}");
    }

    #[test]
    fn a_two_choice_entry_steps_with_keys_and_mouse_and_is_saved_as_a_boolean() {
        let w = World::new("set-two-input");
        let on_disk_flag = || {
            serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(config_file(&w)).unwrap()).unwrap()["update_check"].clone()
        };
        let (mut app, mut term) = settings_screen(&w, 100, 30);
        for _ in 0..2 {
            key(&mut app, KeyCode::Down);
        }
        assert_eq!(app.setting, P + 3);
        assert!(draw(&mut term, &app).contains("Once a day"), "the default");
        assert!(!config_file(&w).exists() || on_disk(&w).update_check, "looking at it changes nothing");

        key(&mut app, KeyCode::Left);
        assert_eq!((on_disk_flag(), on_disk(&w).update_check), (serde_json::json!(false), false));
        assert!(draw(&mut term, &app).contains("Off"));
        key(&mut app, KeyCode::Left);
        assert_eq!(on_disk_flag(), serde_json::json!(false), "the left end holds");
        key(&mut app, KeyCode::Right);
        assert_eq!(on_disk_flag(), serde_json::json!(true));
        key(&mut app, KeyCode::Right);
        assert_eq!(on_disk_flag(), serde_json::json!(true), "and so does the right end");
        for (k, want) in [('h', false), ('l', true), ('-', false), ('+', true), ('=', true), (' ', false), (' ', true)] {
            ch(&mut app, k);
            assert_eq!(on_disk(&w).update_check, want, "{k:?}");
        }
        key(&mut app, KeyCode::Enter);
        assert!(!on_disk(&w).update_check, "enter goes to the other choice");
        key(&mut app, KeyCode::Enter);
        assert!(on_disk(&w).update_check, "and wraps back");
        assert_eq!(on_disk(&w).idle_stop_minutes, 120, "the other settings are untouched");

        // The mouse: the buttons step, the value toggles, a click on the row selects it.
        ch(&mut app, 'g');
        draw(&mut term, &app);
        let prev = hit_rect(&app, |h| matches!(h, Hit::SettingStep(i, settings::Step::Prev) if *i == P + 3));
        click(&mut app, prev.x + 1, prev.y);
        assert_eq!((app.setting, on_disk(&w).update_check), (P + 3, false), "a button on another row selects it and steps");
        draw(&mut term, &app);
        let value = hit_rect(&app, |h| matches!(h, Hit::SettingStep(i, settings::Step::Cycle) if *i == P + 3));
        click(&mut app, value.x + 1, value.y);
        assert!(on_disk(&w).update_check);
        draw(&mut term, &app);
        let next = hit_rect(&app, |h| matches!(h, Hit::SettingStep(i, settings::Step::Next) if *i == P + 3));
        click(&mut app, next.x + 1, next.y);
        assert!(on_disk(&w).update_check, "already at the end");
        let buf = term.backend().buffer();
        assert_eq!(buf[(next.x + 1, next.y)].fg, app.theme.border, "the button that can't go further is muted");
    }

    #[test]
    fn the_diagnostics_log_switch_is_drawn_steps_and_is_saved_like_the_other_switches() {
        let w = World::new("set-diag-log");
        let (mut app, mut term) = settings_screen(&w, 100, 30);
        for _ in 0..3 {
            key(&mut app, KeyCode::Down);
        }
        assert_eq!(app.setting, P + 4);
        let shown = draw(&mut term, &app);
        assert!(shown.contains("Keep a diagnostics log") && shown.contains("platoon report"), "{shown}");
        assert!(!config_file(&w).exists() || !on_disk(&w).diagnostics_log, "off until switched on");
        key(&mut app, KeyCode::Right);
        assert!(on_disk(&w).diagnostics_log);
        let shown = draw(&mut term, &app);
        let row = shown.lines().find(|l| l.contains("Keep a diagnostics log")).unwrap_or_default();
        assert!(row.split_whitespace().collect::<Vec<_>>().ends_with(&["‹", "On", "›", "│"]), "{shown}");
        key(&mut app, KeyCode::Enter);
        assert!(!on_disk(&w).diagnostics_log, "enter goes to the other choice");
        draw(&mut term, &app);
        let value = hit_rect(&app, |h| matches!(h, Hit::SettingStep(i, settings::Step::Cycle) if *i == P + 4));
        click(&mut app, value.x + 1, value.y);
        assert!(on_disk(&w).diagnostics_log, "a click on the value too");
        assert!(on_disk(&w).update_check && on_disk(&w).idle_stop_minutes == 120, "the other settings are untouched");
    }

    #[test]
    fn a_two_choice_entry_survives_odd_files_and_keeps_what_else_is_in_them() {
        let w = World::new("set-two-odd");
        std::fs::write(config_file(&w), r#"{"update_check":"no","keep":{"a":[1]},"idle_stop_minutes":60}"#).unwrap();
        let (mut app, mut term) = settings_screen(&w, 100, 30);
        assert!(draw(&mut term, &app).contains("Once a day"), "a wrong-typed value shows the default");
        for _ in 0..2 {
            key(&mut app, KeyCode::Down);
        }
        key(&mut app, KeyCode::Left);
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(config_file(&w)).unwrap()).unwrap();
        assert_eq!(
            (v["update_check"].clone(), v["keep"].clone(), v["idle_stop_minutes"].clone()),
            (serde_json::json!(false), serde_json::json!({"a": [1]}), serde_json::json!(60))
        );
        // An edit made elsewhere shows up on the open screen.
        std::fs::write(config_file(&w), r#"{"update_check":true}"#).unwrap();
        app.refresh_for_test();
        assert!(draw(&mut term, &app).contains("Once a day"));
    }

    #[test]
    fn a_very_short_terminal_still_shows_the_settings_and_the_way_back() {
        let w = World::new("set-short");
        let mut app = w.app();
        ch(&mut app, 's');
        for rows in [8, 10, 12, 13, 14, 15, 16] {
            let mut term = Terminal::new(TestBackend::new(60, rows)).unwrap();
            let text = draw(&mut term, &app); // must not panic
            // Below the smallest size platoon draws in, only the message saying so.
            if rows < crate::app::MIN_HEIGHT {
                assert!(flat(&text).contains(&format!("platoon needs at least 60×14 — this window is 60×{rows}")), "60x{rows}\n{text}");
                continue;
            }
            assert!(text.contains("Theme") && text.contains("esc back"), "the selected entry and the way back: 60x{rows}\n{text}");
            if rows >= 16 {
                assert!(text.contains("Use Claude Code") && text.contains("Stop idle sessions after"), "60x{rows}\n{text}");
            }
        }
        // With little room only the selected setting explains itself, and moving on moves the explanation.
        to_theme_row(&mut app);
        let mut term = Terminal::new(TestBackend::new(60, 18)).unwrap();
        let first = flat(&draw(&mut term, &app));
        assert!(first.contains(ITEMS[P].explain) && !first.contains(ITEMS[P + 1].explain), "{first}");
        key(&mut app, KeyCode::Down);
        let mut term = Terminal::new(TestBackend::new(60, 19)).unwrap();
        let second = flat(&draw(&mut term, &app));
        assert!(second.contains(ITEMS[P + 1].explain) && !second.contains(ITEMS[P].explain), "{second}");
        // The update check's explanation (and the tab's two headings) fits whole at 21 rows.
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Down);
        let mut term = Terminal::new(TestBackend::new(60, 21)).unwrap();
        let third = draw(&mut term, &app);
        assert!(flat(&third).contains(ITEMS[P + 3].explain), "{third}");
        // Too cramped to explain it (the smallest size platoon draws in): the entries and the way back are still whole.
        let mut term = Terminal::new(TestBackend::new(60, 14)).unwrap();
        let cramped = flat(&draw(&mut term, &app));
        assert!(cramped.contains(ITEMS[P + 3].name) && cramped.contains("esc back"), "{cramped}");
        let mut term = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let cramped = draw(&mut term, &app);
        assert!(cramped.contains(ITEMS[P + 3].name) && cramped.contains("Once a day") && cramped.contains("esc back"), "{cramped}");
    }

    #[test]
    fn the_list_footer_offers_settings_and_stays_whole_at_80_columns() {
        let w = World::new("set-footer80");
        w.transcript("aaa", "/srv/a", "hello", 5);
        let app = w.app();
        for cols in [80, 100, 140] {
            let mut term = Terminal::new(TestBackend::new(cols, 24)).unwrap();
            let text = draw(&mut term, &app);
            assert!(text.contains("s settings") && text.contains("q quit") && text.contains("enter open"), "{cols}\n{text}");
        }
    }

    #[test]
    fn three_still_does_nothing_and_the_list_is_unchanged_by_visiting_the_settings() {
        let w = World::new("set-list");
        w.transcript("aaa", "/srv/a", "hello", 5);
        w.transcript("bbb", "/srv/a", "again", 9);
        let mut app = w.app();
        key(&mut app, KeyCode::Down);
        ch(&mut app, 's');
        ch(&mut app, '3');
        assert!(matches!(app.mode, Mode::Settings));
        key(&mut app, KeyCode::Esc);
        assert_eq!((app.tab, app.selected, ids(&app)), (0, 1, vec!["aaa", "bbb"]));
    }

    // --- themes -------------------------------------------------------------------------------------

    fn choose_theme(w: &World, id: &str) {
        settings::save(&w.config, &Settings { theme: id.into(), ..on_disk(w) }).unwrap();
    }

    fn cells(term: &Terminal<TestBackend>) -> Vec<ratatui::buffer::Cell> {
        term.backend().buffer().content.clone()
    }

    #[test]
    fn the_theme_follows_the_settings_file_within_a_normal_refresh() {
        let w = World::new("theme-follow");
        w.transcript("aaa", "/srv/a", "hello", 5);
        let mut app = w.app();
        assert_eq!(app.theme.id, "catppuccin-mocha");
        choose_theme(&w, "nord");
        assert_eq!(app.theme.id, "catppuccin-mocha", "nothing changes until the next refresh");
        app.refresh_for_test();
        assert_eq!(app.theme.id, "nord");
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        draw(&mut term, &app);
        let buf = term.backend().buffer();
        assert_eq!((buf[(1, 1)].fg, buf[(1, 1)].bg), (app.theme.border, app.theme.bg), "drawn with the new theme: the rule under the tabs");
        assert_ne!(app.theme.bg, Theme::default().bg);

        choose_theme(&w, "terminal");
        app.refresh_for_test();
        assert_eq!(app.theme.id, "terminal");
        draw(&mut term, &app);
        assert_eq!(term.backend().buffer()[(1, 1)].bg, Color::Reset);

        // A hand-edited file that says something else (or garbage) shows the default, never an error.
        std::fs::write(config_file(&w), r#"{"theme": "tokyo-night"}"#).unwrap();
        app.refresh_for_test();
        assert_eq!(app.theme.id, "tokyo-night");
        std::fs::write(config_file(&w), "{ not json").unwrap();
        app.refresh_for_test();
        assert_eq!(app.theme.id, "catppuccin-mocha");
    }

    #[test]
    fn an_unknown_theme_shows_the_default_and_survives_changes_made_on_the_settings_screen() {
        let w = World::new("theme-unknown");
        std::fs::write(config_file(&w), r#"{"theme": "from-the-future", "idle_stop_minutes": 120}"#).unwrap();
        let (mut app, mut term) = settings_screen(&w, 100, 30);
        assert_eq!(app.theme.id, "catppuccin-mocha");
        assert!(draw(&mut term, &app).contains("Settings"), "the screen works as before");
        key(&mut app, KeyCode::Right);
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(config_file(&w)).unwrap()).unwrap();
        assert_eq!((v["theme"].as_str(), v["idle_stop_minutes"].as_u64()), (Some("from-the-future"), Some(240)));
        assert_eq!(app.theme.id, "catppuccin-mocha");
    }

    #[test]
    fn every_theme_draws_the_list_the_settings_and_the_dialogs_with_its_own_colors() {
        let mut w = World::new("theme-sweep");
        w.transcript("aaa", "/srv/alpha", "refactor the parser", 3 * 24 * 60);
        w.transcript("bbb", "/srv/beta", "add a login page", 30);
        w.live("bbb", "/srv/beta", 1, "busy");
        for t in Theme::all() {
            choose_theme(&w, &t.id);
            let mut app = w.app();
            assert_eq!(app.theme.id, t.id);
            let fixed = t.ansi.is_some();
            let check = |term: &Terminal<TestBackend>, what: &str| {
                for c in cells(term) {
                    if fixed {
                        assert!(
                            matches!(c.bg, Color::Rgb(..)) && !matches!(c.fg, Color::Indexed(_)),
                            "{}: {what}: {:?}/{:?}",
                            t.id,
                            c.fg,
                            c.bg
                        );
                        assert!(
                            c.symbol().trim().is_empty() || matches!(c.fg, Color::Rgb(..)),
                            "{}: {what}: {:?} is unthemed text",
                            t.id,
                            c.symbol()
                        );
                    } else {
                        assert!(
                            !matches!(c.fg, Color::Rgb(..)) && !matches!(c.bg, Color::Rgb(..)),
                            "{}: {what}: {:?}/{:?}",
                            t.id,
                            c.fg,
                            c.bg
                        );
                    }
                }
            };
            // Down to the smallest size platoon draws in, and below it (the message saying so, in the theme's colors).
            for (cols, rows) in [(100, 24), (80, 24), (60, 16), (60, 14), (44, 12)] {
                let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
                app.mode = Mode::List;
                let text = draw(&mut term, &app);
                assert!(text.contains("platoon"), "{}: {cols}x{rows}\n{text}", t.id);
                check(&term, "list");
                if cols >= 60 {
                    assert_eq!(term.backend().buffer()[(1, 1)].fg, t.border, "{}: the rule under the tabs", t.id);
                }
                ch(&mut app, 's');
                draw(&mut term, &app);
                check(&term, "settings");
                key(&mut app, KeyCode::Esc);
                ch(&mut app, 'n');
                draw(&mut term, &app);
                check(&term, "picker");
                key(&mut app, KeyCode::Esc);
                ch(&mut app, '/');
                draw(&mut term, &app);
                check(&term, "filter");
                key(&mut app, KeyCode::Esc);
            }
        }
    }

    #[test]
    fn the_selected_row_the_current_tab_and_the_bars_use_the_themes_own_roles() {
        let mut w = World::new("theme-roles");
        w.transcript("aaa", "/srv/alpha", "refactor the parser", 40);
        w.transcript("bbb", "/srv/beta", "add a login page", 30);
        w.live("bbb", "/srv/beta", 1, "busy");
        for t in Theme::all() {
            choose_theme(&w, &t.id);
            let app = w.app();
            let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
            draw(&mut term, &app);
            let all = cells(&term);
            let has = |f: &dyn Fn(&ratatui::buffer::Cell) -> bool| all.iter().any(f);
            assert!(has(&|c| c.bg == t.surface && c.symbol() != " "), "{}: the selected row has a surface band with text on it", t.id);
            assert!(has(&|c| c.bg == t.accent && c.fg == t.on_accent), "{}: the current tab's count badge is on_accent on accent", t.id);
            assert!(has(&|c| c.fg == t.busy), "{}: the working session is in the busy color", t.id);
            assert!(has(&|c| c.fg == t.subtext && c.symbol() == "%"), "{}: a context figure under the warning line is quiet", t.id);
            assert!(has(&|c| c.fg == t.dim), "{}", t.id);
            assert!(has(&|c| c.fg == t.subtext), "{}", t.id);
            assert!(has(&|c| c.fg == t.accent && c.symbol() == "━"), "{}: the current tab is underlined", t.id);
        }
    }

    #[test]
    fn the_paused_banner_and_the_dialogs_are_drawn_in_theme_colors() {
        let mut w = World::new("theme-banner");
        w.transcript("same-id", "/tmp", "shared", 1);
        let (a, _b) = (w.live("same-id", "/tmp", 1, "idle"), w.live("same-id", "/tmp", 2, "idle"));
        for id in ["tokyo-night", "terminal"] {
            choose_theme(&w, id);
            let mut app = w.app();
            app.refresh_with(vec![hosted(a)], false);
            app.mode = Mode::Attached(a);
            assert!(app.attached_paused());
            let t = app.theme.clone();
            let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
            term.draw(|f| ui::draw_paused_banner(f, &app, f.area())).unwrap();
            let buf = term.backend().buffer();
            let corner = (0..30)
                .flat_map(|y| (0..100).map(move |x| (x, y)))
                .find(|&(x, y)| buf[(x, y)].symbol() == "╭")
                .expect("the banner's frame");
            assert_eq!((buf[corner].fg, buf[corner].bg), (t.warn, t.bg), "{id}: the frame is the warning color");
            let title = (0..100).find(|&x| buf[(x, corner.1)].symbol() == "s" && buf[(x + 1, corner.1)].symbol() == "e").expect("title");
            assert_eq!(buf[(title, corner.1)].fg, t.warn, "{id}");
            let body = (0..100).find(|&x| buf[(x, corner.1 + 1)].symbol() == "T").expect("first line");
            assert_eq!(
                (buf[(body, corner.1 + 1)].fg, buf[(body, corner.1 + 1)].bg),
                (t.text, t.bg),
                "{id}: text on the theme's background"
            );
        }
    }

    #[test]
    fn a_256_color_terminal_gets_only_indexed_colors_and_the_same_text() {
        use crate::colordepth::Depth;
        let w = World::new("theme-256");
        w.transcript("aaa", "/srv/alpha", "refactor the parser", 40);
        let app = w.app();
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        term.draw(|f| ui::draw(f, &app)).unwrap();
        let truecolor = screen_text(&term);
        term.draw(|f| {
            ui::draw(f, &app);
            Depth::Indexed256.apply(f.buffer_mut());
        })
        .unwrap();
        assert_eq!(screen_text(&term), truecolor);
        assert!(cells(&term).iter().all(|c| !matches!(c.fg, Color::Rgb(..)) && !matches!(c.bg, Color::Rgb(..))));
        assert!(cells(&term).iter().any(|c| matches!(c.bg, Color::Indexed(_))), "the theme's background is an index now");
    }

    // --- the theme list ------------------------------------------------------------------------------

    /// Three conversations (one working) and two theme files of the user's: one that loads, one that is light.
    fn theme_world(name: &str) -> World {
        let mut w = World::new(name);
        w.transcript("aaa", "/srv/alpha", "refactor the parser", 3 * 24 * 60);
        w.transcript("bbb", "/srv/beta", "add a login page", 30);
        w.transcript("ccc", "/srv/gamma", "write the docs", 3);
        w.live("bbb", "/srv/beta", 1, "busy");
        put_theme(
            &w,
            "My Theme",
            "background = #101820\nforeground = #e6e6e6\npalette = 12=#5fb3ff\npalette = 13=#ff8fd0\npalette = 10=#7fe28a\npalette = 11=#ffd866\npalette = 9=#ff6b6b\n",
        );
        put_theme(&w, "Nord Light", "background = #eceff4\nforeground = #2e3440\n");
        w
    }

    fn put_theme(w: &World, name: &str, text: &str) {
        std::fs::create_dir_all(w.config.join("themes")).unwrap();
        std::fs::write(w.config.join("themes").join(name), text).unwrap();
    }

    /// The settings open on the Theme entry: check it.
    fn to_theme_row(app: &mut App) {
        assert_eq!(app.items()[app.setting].name, "Theme", "the settings open on the Theme entry");
    }

    fn open_themes(app: &mut App) {
        if matches!(app.mode, Mode::List) {
            ch(app, 's');
            to_theme_row(app);
        }
        assert!(matches!(app.mode, Mode::Settings) && app.setting == P, "the settings are open on the Theme entry");
        key(app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Themes));
    }

    fn snapshot(term: &Terminal<TestBackend>) -> ratatui::buffer::Buffer {
        term.backend().buffer().clone()
    }

    /// (column, row) of the first `needle` in the buffer.
    fn locate(buf: &ratatui::buffer::Buffer, needle: &str) -> (u16, u16) {
        (0..buf.area.height)
            .find_map(|y| {
                let text: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
                text.find(needle).map(|b| (text[..b].chars().count() as u16, y))
            })
            .unwrap_or_else(|| panic!("no {needle:?} on the screen"))
    }

    fn saved_theme(w: &World) -> Option<String> {
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(config_file(w)).ok()?).ok()?;
        v.get("theme").and_then(|t| t.as_str()).map(str::to_string)
    }

    fn by_id(id: &str) -> Theme {
        Theme::by_id(id).unwrap()
    }

    #[test]
    fn enter_on_the_theme_row_opens_the_list_and_esc_closes_it_changing_nothing() {
        let w = theme_world("tl-open");
        let before = std::fs::read(config_file(&w)).unwrap();
        let mut app = w.app();
        open_themes(&mut app);
        assert_eq!(app.tpick.list.len(), Theme::all().len() + 1, "the built-ins, then the one user theme that loaded");
        assert_eq!(app.tpick.list.last().unwrap().id, "user:my-theme");
        assert_eq!((app.tpick.selected, app.preview.is_none()), (0, true), "it opens on the theme in effect, previewing nothing");
        key(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Settings), "esc goes back to the settings, not out of them");
        key(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::List));
        assert_eq!(std::fs::read(config_file(&w)).unwrap(), before, "looking never writes");

        // Space opens it too, so does ctrl+c close it, and so does clicking the value.
        ch(&mut app, 's');
        to_theme_row(&mut app);
        ch(&mut app, ' ');
        assert!(matches!(app.mode, Mode::Themes));
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(matches!(app.mode, Mode::Settings) && !app.quit);
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        draw(&mut term, &app);
        let value = hit_rect(&app, |h| matches!(h, Hit::SettingStep(P, settings::Step::Cycle)));
        click(&mut app, value.x + 2, value.y);
        assert!(matches!(app.mode, Mode::Themes), "the value is a button that opens the list");
        // The other rows' Enter still steps their own value and never opens the list.
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Settings));
    }

    #[test]
    fn the_theme_list_cannot_be_reached_from_any_other_mode() {
        let w = theme_world("tl-unreachable");
        let mut app = w.app();
        // The list: Enter opens a conversation, nothing else.
        key(&mut app, KeyCode::Enter);
        assert!(!matches!(app.mode, Mode::Themes));
        app.mode = Mode::List;
        ch(&mut app, '/');
        key(&mut app, KeyCode::Enter);
        assert!(!matches!(app.mode, Mode::Themes));
        app.mode = Mode::List;
        ch(&mut app, 'n');
        key(&mut app, KeyCode::Enter);
        assert!(!matches!(app.mode, Mode::Themes));
        app.mode = Mode::Confirm(Confirm::Archive { id: "aaa".into(), name: "a".into(), busy: false, outside: false });
        key(&mut app, KeyCode::Enter);
        assert!(!matches!(app.mode, Mode::Themes));
        app.mode = Mode::List;
        app.view_conversation_for_test("aaa", 1);
        key(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Attached(_)));
        // Even the step that opens it does nothing outside the settings.
        app.mode = Mode::List;
        let opened = |app: &mut App| matches!(app.mode, Mode::Themes);
        app.act(Action::Confirm);
        assert!(!opened(&mut app));
        // And a stale click on the theme row's value can't open it from the list either.
        app.hits.borrow_mut().push((ratatui::layout::Rect::new(0, 0, 200, 50), Hit::SettingStep(P, settings::Step::Cycle)));
        click(&mut app, 3, 3);
        assert!(!opened(&mut app));
        // From the settings, only the Theme row opens it.
        app.mode = Mode::List;
        ch(&mut app, 's');
        to_theme_row(&mut app);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        assert!(!opened(&mut app));
        key(&mut app, KeyCode::Up);
        key(&mut app, KeyCode::Enter);
        assert!(opened(&mut app));
    }

    #[test]
    fn moving_previews_the_theme_on_the_whole_app_and_esc_puts_back_every_cell() {
        let w = theme_world("tl-preview");
        let mut app = w.app();
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let (mocha, macchiato, frappe) = (by_id("catppuccin-mocha"), by_id("catppuccin-macchiato"), by_id("catppuccin-frappe"));

        let list_before = {
            draw(&mut term, &app);
            snapshot(&term)
        };
        ch(&mut app, 's');
        to_theme_row(&mut app);
        let settings_before = {
            draw(&mut term, &app);
            snapshot(&term)
        };
        key(&mut app, KeyCode::Enter);
        let picker_before = {
            draw(&mut term, &app);
            snapshot(&term)
        };

        for (moves, want) in [(1, &macchiato), (1, &frappe)] {
            for _ in 0..moves {
                key(&mut app, KeyCode::Down);
            }
            assert_eq!(app.effective_theme().id, want.id);
            draw(&mut term, &app);
            let buf = snapshot(&term);
            // The real list behind the panel, and the frame, in the previewed theme's colors...
            let (x, y) = locate(&buf, "platoon");
            assert_eq!(buf[(x, y)].fg, want.accent, "{}: the header title", want.id);
            assert_eq!(buf[(1, 1)].fg, want.border, "{}: the rule under the tabs", want.id);
            let (gx, gy) = locate(&buf, "gamma");
            assert_eq!((buf[(gx, gy)].fg, buf[(gx, gy)].bg), (want.text, want.surface), "{}: the selected row", want.id);
            let (bx, by) = locate(&buf, "beta");
            assert_eq!((buf[(bx, by)].fg, buf[(bx, by)].bg), (want.text, want.bg), "{}: another row", want.id);
            let (wx, wy) = locate(&buf, "⠋");
            assert_eq!(buf[(wx, wy)].fg, want.busy, "{}: the working marker", want.id);
            // ...and the panel itself is drawn in them too, including its sample.
            let panel = hit_rect(&app, |h| matches!(h, Hit::Inert));
            assert_eq!(buf[(panel.x, panel.y)].fg, want.accent, "{}: the panel's border", want.id);
            let (sx, sy) = locate(&buf, "docs-site");
            assert_eq!(buf[(sx, sy)].fg, want.ok, "{}: the unread sample row", want.id);
            assert_ne!(buf, picker_before, "the screen really changed");
        }
        // Not a byte of the old colors is left behind after esc: the settings popup is exactly as it was,
        // and so is the list.
        key(&mut app, KeyCode::Esc);
        assert!(app.preview.is_none());
        draw(&mut term, &app);
        assert_eq!(snapshot(&term), settings_before, "cell for cell, colors included");
        key(&mut app, KeyCode::Esc);
        draw(&mut term, &app);
        assert_eq!(snapshot(&term), list_before);
        assert_eq!(app.effective_theme(), &mocha);
        assert_eq!(saved_theme(&w).as_deref(), Some("catppuccin-mocha"));

        // Reopening starts from the same picture as the first time.
        ch(&mut app, 's');
        to_theme_row(&mut app);
        key(&mut app, KeyCode::Enter);
        draw(&mut term, &app);
        assert_eq!(snapshot(&term), picker_before);
    }

    #[test]
    fn enter_keeps_the_theme_by_saving_its_id_and_the_rest_of_the_file_is_untouched() {
        let w = theme_world("tl-keep");
        std::fs::write(
            config_file(&w),
            r#"{"version":1,"note":"mine","nested":{"a":[1,{"b":null}]},"idle_stop_minutes":60,"theme":"catppuccin-mocha"}"#,
        )
        .unwrap();
        let mut app = w.app();
        open_themes(&mut app);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Down);
        assert_eq!(saved_theme(&w).as_deref(), Some("catppuccin-mocha"), "moving saves nothing");
        key(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Settings), "enter returns to the settings");
        assert!(app.preview.is_none());
        assert_eq!((app.theme.id.as_str(), saved_theme(&w).as_deref()), ("catppuccin-frappe", Some("catppuccin-frappe")));
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(config_file(&w)).unwrap()).unwrap();
        assert_eq!(v["note"], "mine");
        assert_eq!(v["nested"], serde_json::json!({"a":[1,{"b":null}]}));
        assert_eq!(v["idle_stop_minutes"], 60);
        assert!(!w.config.join("settings.json.bak").exists() && !w.config.join("settings.json.tmp").exists());
        // The settings row shows what was kept, and the list marks it as the saved one.
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        assert!(draw(&mut term, &app).contains("Catppuccin Frappé"));
        key(&mut app, KeyCode::Enter);
        let shown = draw(&mut term, &app);
        let line = shown.lines().find(|l| l.contains("Catppuccin Frappé")).unwrap();
        assert!(line.contains("✓"), "the saved theme has a check: {line}");
        assert_eq!(shown.matches('✓').count(), 1);

        // Enter on the entry that is already in effect writes nothing at all.
        let stamp = std::fs::metadata(config_file(&w)).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        key(&mut app, KeyCode::Enter);
        assert_eq!(std::fs::metadata(config_file(&w)).unwrap().modified().unwrap(), stamp);
        // A damaged file is backed up first, like any other save.
        std::fs::write(config_file(&w), "{ broken").unwrap();
        let mut app = w.app();
        open_themes(&mut app);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        assert_eq!(std::fs::read_to_string(w.config.join("settings.json.bak")).unwrap(), "{ broken");
        assert_eq!(saved_theme(&w).as_deref(), Some("catppuccin-macchiato"));
    }

    #[test]
    fn esc_after_wandering_leaves_the_file_byte_for_byte_and_moving_back_needs_no_preview() {
        let w = theme_world("tl-revert");
        let before = std::fs::read(config_file(&w)).unwrap();
        let mut app = w.app();
        open_themes(&mut app);
        key(&mut app, KeyCode::End);
        assert_eq!(app.effective_theme().id, "user:my-theme", "End jumps to the last entry and previews it");
        key(&mut app, KeyCode::Home);
        assert!(app.preview.is_none(), "back on the theme in effect there is nothing to preview");
        ch(&mut app, 'G');
        key(&mut app, KeyCode::Up);
        key(&mut app, KeyCode::PageUp);
        assert!(app.preview.is_some());
        key(&mut app, KeyCode::Esc);
        assert!(app.preview.is_none());
        assert_eq!(app.effective_theme().id, "catppuccin-mocha");
        assert_eq!(std::fs::read(config_file(&w)).unwrap(), before);
        // Enter after coming back to where it started keeps that, i.e. changes nothing.
        open_themes(&mut app);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Up);
        key(&mut app, KeyCode::Enter);
        assert_eq!(std::fs::read(config_file(&w)).unwrap(), before);
        // The ends hold.
        open_themes(&mut app);
        for _ in 0..80 {
            key(&mut app, KeyCode::Down);
        }
        assert_eq!(app.tpick.selected, app.tpick.list.len() - 1);
        for _ in 0..80 {
            key(&mut app, KeyCode::Up);
        }
        assert_eq!(app.tpick.selected, 0);
    }

    #[test]
    fn a_failed_save_puts_the_saved_theme_back_and_says_why() {
        let w = theme_world("tl-fail");
        let blocker = w.base.join("blocker");
        std::fs::write(&blocker, "x").unwrap();
        let mut app = App::with_dirs(crate::agents::claude::Reader::with_root(w.root.clone()), w.cache.clone(), blocker.join("config"));
        open_themes(&mut app);
        key(&mut app, KeyCode::Down);
        assert_eq!(app.effective_theme().id, "catppuccin-macchiato");
        key(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Settings));
        assert_eq!(
            (app.theme.id.as_str(), app.effective_theme().id.as_str()),
            ("catppuccin-mocha", "catppuccin-mocha"),
            "back to what is really saved"
        );
        assert!(app.preview.is_none());
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let shown = draw(&mut term, &app);
        assert!(flat(&shown).contains("Couldn't save the settings: Not a directory") && shown.contains("Catppuccin Mocha"), "{shown}");
        assert!(!blocker.join("config").exists());
    }

    #[test]
    fn the_arrow_keys_on_the_theme_row_save_the_neighboring_theme_at_once() {
        let w = theme_world("tl-arrows");
        let mut app = w.app();
        ch(&mut app, 's');
        to_theme_row(&mut app);
        let stamp = std::fs::metadata(config_file(&w)).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        key(&mut app, KeyCode::Left);
        assert_eq!(
            std::fs::metadata(config_file(&w)).unwrap().modified().unwrap(),
            stamp,
            "nothing before the first theme, so nothing is written"
        );
        key(&mut app, KeyCode::Right);
        assert_eq!((saved_theme(&w).as_deref(), app.theme.id.as_str()), (Some("catppuccin-macchiato"), "catppuccin-macchiato"));
        assert!(matches!(app.mode, Mode::Settings) && app.preview.is_none(), "no preview and no list on this path");
        for (k, want) in
            [('l', "catppuccin-frappe"), ('+', "tokyo-night"), ('=', "tokyo-night-storm"), ('h', "tokyo-night"), ('-', "catppuccin-frappe")]
        {
            ch(&mut app, k);
            assert_eq!(saved_theme(&w).as_deref(), Some(want), "{k:?}");
        }
        key(&mut app, KeyCode::Left);
        key(&mut app, KeyCode::Left);
        assert_eq!(saved_theme(&w).as_deref(), Some("catppuccin-mocha"));
        // All the way to the end of the list, which is where the user's own themes are; and it stops there.
        for _ in 0..80 {
            key(&mut app, KeyCode::Right);
        }
        assert_eq!((saved_theme(&w).as_deref(), app.theme.name.as_str()), (Some("user:my-theme"), "My Theme"));
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let shown = draw(&mut term, &app);
        assert!(shown.contains("My Theme"), "{shown}");
        let (_, row) = locate(term.backend().buffer(), "My Theme");
        assert_eq!(term.backend().buffer()[(locate(term.backend().buffer(), "My Theme").0, row)].fg, app.theme.accent);
        // The buttons on the row do the same, and Enter still opens the list rather than stepping.
        let prev = hit_rect(&app, |h| matches!(h, Hit::SettingStep(P, settings::Step::Prev)));
        click(&mut app, prev.x + 1, prev.y);
        assert_eq!(saved_theme(&w).as_deref(), Some("terminal"));
        let next = {
            draw(&mut term, &app);
            hit_rect(&app, |h| matches!(h, Hit::SettingStep(P, settings::Step::Next)))
        };
        click(&mut app, next.x + 1, next.y);
        assert_eq!(saved_theme(&w).as_deref(), Some("user:my-theme"));
        assert!(matches!(app.mode, Mode::Settings));
    }

    #[test]
    fn an_id_platoon_does_not_know_is_kept_by_the_arrows_until_a_theme_is_actually_chosen() {
        let w = theme_world("tl-unknown-arrows");
        std::fs::write(config_file(&w), r#"{"theme":"from-the-future","idle_stop_minutes":120}"#).unwrap();
        let mut app = w.app();
        assert_eq!(app.theme.id, "catppuccin-mocha", "shown as the default");
        ch(&mut app, 's');
        to_theme_row(&mut app);
        key(&mut app, KeyCode::Left);
        assert_eq!(saved_theme(&w).as_deref(), Some("from-the-future"), "nothing lies before the default");
        // Opening the list and leaving it, or keeping what it opened on, is not a choice either.
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Enter);
        assert_eq!(saved_theme(&w).as_deref(), Some("from-the-future"));
        key(&mut app, KeyCode::Right);
        assert_eq!(saved_theme(&w).as_deref(), Some("catppuccin-macchiato"));
    }

    #[test]
    fn the_settings_popup_still_fades_the_list_but_the_theme_list_does_not() {
        let w = theme_world("tl-fade");
        let mut app = w.app();
        let mut term = Terminal::new(TestBackend::new(120, 32)).unwrap();
        let t = app.theme.clone();
        ch(&mut app, 's');
        to_theme_row(&mut app);
        draw(&mut term, &app);
        let buf = snapshot(&term);
        let (gx, gy) = locate(&buf, "gamma");
        let (px, py) = locate(&buf, "platoon");
        assert_eq!((buf[(gx, gy)].fg, buf[(px, py)].fg), (t.border, t.border), "behind the ordinary popup everything is muted");

        key(&mut app, KeyCode::Enter);
        draw(&mut term, &app);
        let buf = snapshot(&term);
        let (gx, gy) = locate(&buf, "gamma");
        let (px, py) = locate(&buf, "platoon");
        assert_eq!((buf[(gx, gy)].fg, buf[(gx, gy)].bg), (t.text, t.surface), "behind the theme list a row keeps its colors");
        assert_eq!(buf[(px, py)].fg, t.accent, "and the title its accent");
        let (dx, dy) = locate(&buf, "/srv/gamma");
        assert_eq!(buf[(dx, dy)].fg, t.subtext);
        // Bold and underline survive too: the current tab is still underlined and bold.
        let (ax, ay) = locate(&buf, "Active");
        assert!(buf[(ax, ay)].modifier.contains(ratatui::style::Modifier::BOLD));
        assert!(!draw(&mut term, &app).contains("Settings file:"), "the settings popup gives way to the list");
        // The rule under the tabs is the same one the list has.
        assert_eq!(buf[(1, 1)].fg, t.border);
    }

    #[test]
    fn the_sample_shows_every_color_role_of_the_theme_in_view_even_with_no_conversations() {
        let w = World::new("tl-sample"); // nothing in the list at all
        let mut app = w.app();
        assert!(app.visible().is_empty());
        open_themes(&mut app);
        for (cols, rows) in [(80, 24), (100, 30), (140, 30), (60, 20)] {
            let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
            key(&mut app, KeyCode::Home);
            for k in 0..app.tpick.list.len() {
                let t = app.tpick.list[k].clone();
                draw(&mut term, &app);
                assert_eq!(app.effective_theme().id, t.id);
                let panel = hit_rect(&app, |h| matches!(h, Hit::Inert));
                let buf = snapshot(&term);
                let cells: Vec<&ratatui::buffer::Cell> = (panel.y..panel.bottom())
                    .flat_map(|y| (panel.x..panel.right()).map(move |x| (x, y)))
                    .map(|(x, y)| &buf[(x, y)])
                    .collect();
                let has_fg = |c: Color| cells.iter().any(|cell| cell.fg == c && cell.symbol().trim() != "");
                let has_bg = |c: Color| cells.iter().any(|cell| cell.bg == c);
                let ctx = format!("{} at {cols}x{rows}", t.id);
                assert!(has_bg(t.bg) && has_bg(t.surface), "{ctx}: background and the selected row's surface");
                assert!(has_fg(t.text) && has_fg(t.subtext) && has_fg(t.dim), "{ctx}: the three text colors");
                assert!(has_fg(t.border), "{ctx}: the border of the frame and of the bars' tracks");
                assert!(
                    has_fg(t.accent) && has_fg(t.busy) && has_fg(t.ok) && has_fg(t.warn) && has_fg(t.err),
                    "{ctx}: accent and the four status colors"
                );
                // The sample's parts are there in words, not only in colors.
                let text: String = cells.iter().map(|c| c.symbol().to_string()).collect();
                for want in ["Active", "docs-site", "notes", "5h", "enter open", "Archived"] {
                    assert!(text.contains(want), "{ctx}: {want}");
                }
                if panel.width >= 40 {
                    assert!(text.contains("working") && text.contains("done") && text.contains("idle"), "{ctx}: the status words");
                }
                assert!(text.contains('━') && text.contains("sample"), "{ctx}");
                key(&mut app, KeyCode::Down);
            }
        }
        // On the real screen behind it, an empty list previews nothing, which is why the sample exists.
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let shown = draw(&mut term, &app);
        assert!(shown.contains("No sessions yet"), "{shown}");
    }

    #[test]
    fn the_theme_list_shows_names_swatches_a_check_the_users_marks_and_what_was_skipped() {
        let w = theme_world("tl-content");
        let mut app = w.app();
        open_themes(&mut app);
        // Tall enough for every built-in, the user's themes and the note at once.
        let mut term = Terminal::new(TestBackend::new(140, 80)).unwrap();
        let shown = draw(&mut term, &app);
        let buf = snapshot(&term);
        for t in Theme::all() {
            assert!(shown.contains(&t.name), "{} is listed\n{shown}", t.name);
        }
        let (mx, my) = locate(&buf, "My Theme");
        let mine = app.tpick.list.last().unwrap().clone();
        let row: String = (0..buf.area.width).map(|x| buf[(x, my)].symbol().to_string()).collect();
        assert!(row.contains("yours"), "the user's own theme is marked: {row}");
        // The swatch strip is that entry's own five colors, whatever theme is in effect.
        let strip: Vec<Color> = (0..buf.area.width).filter(|&x| buf[(x, my)].symbol() == "█").map(|x| buf[(x, my)].fg).collect();
        assert_eq!(strip, [mine.accent, mine.busy, mine.ok, mine.warn, mine.err]);
        assert_eq!(buf[(mx, my)].fg, app.theme.subtext, "an entry that is not highlighted is quiet");
        let (nx, ny) = locate(&buf, "Nord");
        let nord = by_id("nord");
        let nstrip: Vec<Color> = (0..buf.area.width).filter(|&x| buf[(x, ny)].symbol() == "█").map(|x| buf[(x, ny)].fg).collect();
        assert_eq!(nstrip, [nord.accent, nord.busy, nord.ok, nord.warn, nord.err]);
        assert_eq!(buf[(nx, ny)].bg, app.theme.bg);
        // Exactly one check, on the saved theme.
        assert_eq!(shown.matches('✓').count(), 1);
        assert!(shown.lines().find(|l| l.contains("Catppuccin Mocha")).unwrap().contains('✓'));
        // The light theme file was skipped, and the reason is in a dim note.
        assert!(flat(&shown).contains("1 theme file skipped. Nord Light: a light theme has to read as it is: palette 12"), "{shown}");
        let (kx, ky) = locate(&buf, "1 theme file skipped");
        assert_eq!(buf[(kx, ky)].fg, app.theme.dim);
        // With nothing skipped there is no note.
        std::fs::remove_file(w.config.join("themes/Nord Light")).unwrap();
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Esc);
        app.refresh_for_test();
        open_themes(&mut app);
        assert!(!draw(&mut term, &app).contains("skipped"));
    }

    #[test]
    fn the_theme_list_fits_and_never_panics_at_every_size() {
        let w = theme_world("tl-sizes");
        put_theme(&w, "Broken", "nothing here");
        put_theme(&w, "Also Broken", "background = nope");
        put_theme(&w, "Third", "");
        let mut app = w.app();
        open_themes(&mut app);
        for (cols, rows) in [
            (80, 24),
            (100, 30),
            (140, 30),
            (60, 20),
            (80, 20),
            (44, 12),
            (60, 10),
            (40, 8),
            (30, 6),
            (20, 5),
            (12, 4),
            (8, 3),
            (3, 3),
            (1, 1),
        ] {
            for sel in [0, 1, 7, 14] {
                app.select_theme_for_test(sel);
                let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
                let shown = draw(&mut term, &app); // must not panic
                let ctx = format!("{cols}x{rows} selecting {sel}\n{shown}");
                if crate::app::too_small(cols, rows) {
                    assert!(cols < 30 || flat(&shown).contains("platoon needs at least"), "{ctx}");
                    continue;
                }
                assert!(shown.contains("esc cancel") && shown.contains("enter use"), "{ctx}");
                assert!(shown.contains(&app.tpick.list[sel].name) || rows < 12, "the highlighted entry is in view, whole: {ctx}");
                // Nothing touches the panel's border on its right.
                if cols >= 60 {
                    let panel = hit_rect(&app, |h| matches!(h, Hit::Inert));
                    for y in panel.y + 1..panel.bottom() - 1 {
                        assert_eq!(
                            term.backend().buffer()[(panel.right() - 2, y)].symbol(),
                            " ",
                            "text touches the panel's border at row {y}: {ctx}"
                        );
                    }
                }
                if cols >= 60 && rows >= 18 {
                    assert!(shown.contains("sample") && shown.contains("Active") && shown.contains("docs-site"), "{ctx}");
                    assert!(flat(&shown).contains("3 theme files skipped") || flat(&shown).contains("4 theme files skipped"), "{ctx}");
                }
            }
        }
        // Ten skipped files can't take the panel over: at most three lines, the last one cut with an ellipsis.
        for n in 0..10 {
            put_theme(&w, &format!("Skip {n}"), "");
        }
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Esc);
        app.refresh_for_test();
        open_themes(&mut app);
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let shown = draw(&mut term, &app);
        assert!(flat(&shown).contains("theme files skipped") && shown.contains("…"), "{shown}");
        assert!(shown.contains("sample") && shown.contains("esc cancel"), "{shown}");
    }

    #[test]
    fn the_mouse_hovers_previews_keeps_cancels_and_scrolls_the_theme_list() {
        let w = theme_world("tl-mouse");
        let mut app = w.app();
        open_themes(&mut app);
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        draw(&mut term, &app);
        let (mocha, macchiato, frappe) = (by_id("catppuccin-mocha"), by_id("catppuccin-macchiato"), by_id("catppuccin-frappe"));
        let row = |app: &App, i: usize| hit_rect(app, |h| matches!(h, Hit::ThemeRow(x) if *x == i));

        // Hovering lights an entry up but previews nothing.
        let r2 = row(&app, 2);
        hover(&mut app, r2.x + 4, r2.y);
        draw(&mut term, &app);
        assert_eq!(term.backend().buffer()[(r2.x + 4, r2.y)].bg, mocha.surface, "hovered entry");
        assert!(app.preview.is_none() && app.tpick.selected == 0);

        // A click previews it and saves nothing; the whole app is recolored.
        click(&mut app, r2.x + 4, r2.y);
        assert_eq!((app.tpick.selected, app.effective_theme().id.as_str()), (2, "catppuccin-frappe"));
        assert_eq!(saved_theme(&w).as_deref(), Some("catppuccin-mocha"));
        draw(&mut term, &app);
        let (px, py) = locate(term.backend().buffer(), "platoon");
        assert_eq!(term.backend().buffer()[(px, py)].fg, frappe.accent);

        // The wheel moves the highlight one entry at a time, previewing as it goes.
        app.on_mouse(MouseEvent { kind: MouseEventKind::ScrollUp, column: 5, row: 5, modifiers: KeyModifiers::NONE });
        assert_eq!(app.effective_theme().id, macchiato.id);
        app.on_mouse(MouseEvent { kind: MouseEventKind::ScrollDown, column: 5, row: 5, modifiers: KeyModifiers::NONE });
        app.on_mouse(MouseEvent { kind: MouseEventKind::ScrollDown, column: 5, row: 5, modifiers: KeyModifiers::NONE });
        assert_eq!(app.effective_theme().id, "tokyo-night");

        // A click on the panel that is not an entry does nothing; an entry other than the previewed one previews it.
        draw(&mut term, &app);
        let panel = hit_rect(&app, |h| matches!(h, Hit::Inert));
        click(&mut app, panel.x + 1, panel.y);
        assert!(matches!(app.mode, Mode::Themes) && app.tpick.selected == 3);
        // Clicking the previewed entry again keeps it, and returns to the settings.
        let r = row(&app, 3);
        click(&mut app, r.x + 4, r.y);
        assert!(matches!(app.mode, Mode::Settings) && app.preview.is_none());
        assert_eq!((saved_theme(&w).as_deref(), app.theme.id.as_str()), (Some("tokyo-night"), "tokyo-night"));
    }

    #[test]
    fn the_use_and_cancel_hints_and_a_click_outside_the_theme_list_are_buttons() {
        let w = theme_world("tl-mouse2");
        let mut app = w.app();
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let start = |app: &mut App, term: &mut Terminal<TestBackend>| {
            app.mode = Mode::Settings;
            app.setting = P;
            open_themes(app);
            draw(term, app);
        };
        start(&mut app, &mut term);
        let r1 = hit_rect(&app, |h| matches!(h, Hit::ThemeRow(1)));
        click(&mut app, r1.x + 4, r1.y);
        assert_eq!(app.effective_theme().id, "catppuccin-macchiato");
        draw(&mut term, &app);

        // "enter use" is a visible control for the same thing as Enter.
        click_action(&mut app, Action::Confirm);
        assert!(matches!(app.mode, Mode::Settings));
        assert_eq!((saved_theme(&w).as_deref(), app.theme.id.as_str()), (Some("catppuccin-macchiato"), "catppuccin-macchiato"));

        // "esc cancel" reverts.
        start(&mut app, &mut term);
        let r5 = hit_rect(&app, |h| matches!(h, Hit::ThemeRow(5)));
        click(&mut app, r5.x + 4, r5.y);
        assert_eq!(app.effective_theme().id, "dracula");
        draw(&mut term, &app);
        click_action(&mut app, Action::Cancel);
        assert!(matches!(app.mode, Mode::Settings) && app.preview.is_none());
        assert_eq!((saved_theme(&w).as_deref(), app.effective_theme().id.as_str()), (Some("catppuccin-macchiato"), "catppuccin-macchiato"));

        // A click outside the panel, on the real screen behind it, reverts and closes the list (not the settings).
        start(&mut app, &mut term);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Down);
        draw(&mut term, &app);
        assert_ne!(app.effective_theme().id, "catppuccin-macchiato");
        click(&mut app, 4, 4);
        assert!(matches!(app.mode, Mode::Settings), "closed the list, still in the settings");
        assert!(app.preview.is_none());
        assert_eq!(saved_theme(&w).as_deref(), Some("catppuccin-macchiato"));
        assert_eq!(app.effective_theme().id, "catppuccin-macchiato");
        // Hovering while a list is open never moves the highlight.
        start(&mut app, &mut term);
        let sel = app.tpick.selected;
        hover(&mut app, 30, 10);
        assert_eq!(app.tpick.selected, sel);
    }

    #[test]
    fn a_change_of_the_file_on_disk_never_fights_a_preview_and_applies_when_the_list_closes() {
        let w = theme_world("tl-disk");
        let mut app = w.app();
        open_themes(&mut app);
        key(&mut app, KeyCode::Down);
        assert_eq!(app.effective_theme().id, "catppuccin-macchiato");
        std::fs::write(config_file(&w), r#"{"theme":"nord","note":"edited by hand"}"#).unwrap();
        app.refresh_for_test();
        assert_eq!(app.theme.id, "nord", "what the file says is tracked");
        assert_eq!(app.effective_theme().id, "catppuccin-macchiato", "but the preview wins while the list is open");
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        draw(&mut term, &app);
        assert_eq!(term.backend().buffer()[(1, 1)].fg, by_id("catppuccin-macchiato").border);
        key(&mut app, KeyCode::Esc);
        assert_eq!(app.effective_theme().id, "nord", "an edited id applies as usual once the list is closed");
        // Keeping works on top of the edited file, keeping its other keys.
        open_themes(&mut app);
        key(&mut app, KeyCode::Down);
        std::fs::write(config_file(&w), r#"{"theme":"dracula","note":"edited again"}"#).unwrap();
        app.refresh_for_test();
        key(&mut app, KeyCode::Enter);
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(config_file(&w)).unwrap()).unwrap();
        assert_eq!((v["theme"].as_str(), v["note"].as_str()), (Some("gruvbox-dark"), Some("edited again")));
    }

    #[test]
    fn a_previewed_theme_recolors_an_attached_session_too() {
        use std::os::unix::net::UnixListener;
        let w = World::new("tl-attached");
        let sock = w.base.join("host.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = crate::host::read_frame(&mut stream); // the client's size
            crate::host::write_frame(&mut stream, crate::host::F_OUTPUT, b"\x1b[31mred\x1b[0m plain\x1b[44m blue").unwrap();
            // Held open until the test is over.
            let _ = crate::host::read_frame(&mut stream);
        });
        let mut app = w.app();
        let client = crate::client::Client::connect(&sock, 20, 60).unwrap();
        let until = std::time::Instant::now();
        while client.parser.lock().unwrap().screen().contents().is_empty() && until.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        app.client = Some(client);
        app.mode = Mode::Attached(4242);
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let cell = |term: &Terminal<TestBackend>, x: u16, y: u16| term.backend().buffer()[(x, y)].clone();
        // The terminal area starts at the top left: there is no frame around it.
        draw(&mut term, &app);
        let mocha = by_id("catppuccin-mocha");
        assert_eq!((cell(&term, 0, 0).fg, cell(&term, 0, 0).bg), (mocha.ansi.unwrap()[1], mocha.bg), "red is the saved theme's red");
        for id in ["tokyo-night", "dracula", "nord"] {
            let t = by_id(id);
            app.preview = Some(t.clone());
            draw(&mut term, &app);
            assert_eq!(cell(&term, 0, 0).fg, t.ansi.unwrap()[1], "{id}: SGR 31 is the previewed theme's red");
            assert_eq!(cell(&term, 5, 0).fg, t.text, "{id}: default text");
            assert_eq!(cell(&term, 5, 0).bg, t.bg, "{id}: default background");
            assert_eq!(cell(&term, 11, 0).bg, t.ansi.unwrap()[4], "{id}: SGR 44");
            assert_eq!(cell(&term, 0, 23).bg, t.bg, "{id}: the status bar");
        }
        app.preview = None;
        draw(&mut term, &app);
        assert_eq!(cell(&term, 0, 0).fg, mocha.ansi.unwrap()[1], "back to the saved theme");
        drop(app); // closes the connection, which lets the server thread finish
        server.join().unwrap();
    }

    /// A stand-in for a session's host at `sock`: each window that connects is sent a screen naming its connection
    /// ("screen of connection 1", ...), and is then cut off at once when `cut_off` says so for its number, else served
    /// until it leaves. Says how many have connected.
    fn stand_in_host(
        sock: &std::path::Path,
        cut_off: impl Fn(usize) -> bool + Send + 'static,
    ) -> std::sync::Arc<std::sync::atomic::AtomicUsize> {
        let listener = std::os::unix::net::UnixListener::bind(sock).unwrap();
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = count.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let n = counted.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                std::thread::spawn({
                    let cut = cut_off(n);
                    move || {
                        let _ = crate::host::read_frame(&mut stream); // the window's size
                        let _ =
                            crate::host::write_frame(&mut stream, crate::host::F_OUTPUT, format!("screen of connection {n}").as_bytes());
                        if cut {
                            let _ = stream.shutdown(std::net::Shutdown::Both);
                        }
                        while crate::host::read_frame(&mut stream).is_ok() {}
                    }
                });
            }
        });
        count
    }

    /// An app attached to the session `pid` whose host is at `sock`, once its window has been sent its first screen.
    fn attached_to(w: &World, pid: u32, sock: &std::path::Path) -> App {
        let mut app = w.app();
        app.hosts = vec![crate::host::HostInfo { sock: sock.to_path_buf(), ..hosted(pid) }];
        let client = crate::client::Client::connect(sock, 20, 60).unwrap();
        wait_until("the first screen", || !client.parser.lock().unwrap().screen().contents().is_empty() || client.closed());
        app.client = Some(client);
        app.mode = Mode::Attached(pid);
        app
    }

    fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let start = std::time::Instant::now();
        while !done() {
            assert!(start.elapsed() < std::time::Duration::from_secs(10), "timed out waiting for {what}");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn connection_closed(app: &App) -> bool {
        app.client.as_ref().is_some_and(|c| c.closed())
    }

    #[test]
    fn a_window_its_host_cuts_off_connects_again_by_itself_and_catches_up() {
        let w = World::new("reconnect");
        let sock = w.base.join("host.sock");
        let connections = stand_in_host(&sock, |n| n == 1);
        let mut app = attached_to(&w, 4242, &sock);
        wait_until("the host to cut the window off", || connection_closed(&app));

        app.keep_attached();
        assert!(matches!(app.mode, Mode::Attached(4242)), "still in the session");
        assert_eq!(app.active_notice(), None, "nothing to tell: it is as if the window had never been cut off");
        let shows =
            |app: &App, text: &str| app.client.as_ref().is_some_and(|c| c.parser.lock().unwrap().screen().contents().contains(text));
        wait_until("the new connection's screen", || shows(&app, "screen of connection 2"));
        assert_eq!(connections.load(std::sync::atomic::Ordering::Relaxed), 2);
        // A connection that stays open is left alone.
        app.keep_attached();
        assert!(matches!(app.mode, Mode::Attached(4242)));
        assert_eq!(connections.load(std::sync::atomic::Ordering::Relaxed), 2);
    }

    #[test]
    fn a_window_whose_host_has_gone_still_says_the_session_ended() {
        let w = World::new("host-gone");
        let sock = w.base.join("host.sock");
        let (gone, sock_file) = (std::sync::Arc::new(std::sync::Barrier::new(2)), sock.clone());
        // The host ends: its socket goes, then the window's connection closes.
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let host = std::thread::spawn({
            let gone = gone.clone();
            move || {
                let (mut stream, _) = listener.accept().unwrap();
                let _ = crate::host::read_frame(&mut stream);
                crate::host::write_frame(&mut stream, crate::host::F_OUTPUT, b"last screen").unwrap();
                gone.wait();
                drop(listener);
                std::fs::remove_file(&sock_file).unwrap();
                stream.shutdown(std::net::Shutdown::Both).unwrap();
            }
        });
        let mut app = attached_to(&w, 4242, &sock);
        gone.wait();
        host.join().unwrap();
        wait_until("the connection to close", || connection_closed(&app));

        app.keep_attached();
        assert!(matches!(app.mode, Mode::List));
        assert_eq!(app.active_notice(), Some("Session ended"));
        assert!(app.client.is_none());
    }

    #[test]
    fn a_host_that_keeps_cutting_the_window_off_is_not_chased_in_a_loop() {
        let w = World::new("cut-again");
        let sock = w.base.join("host.sock");
        let connections = stand_in_host(&sock, |_| true);
        let mut app = attached_to(&w, 4242, &sock);
        // As the event loop does, about every 50 ms.
        wait_until("the window to give up", || {
            app.keep_attached();
            std::thread::sleep(std::time::Duration::from_millis(50));
            matches!(app.mode, Mode::List)
        });
        assert_eq!(app.active_notice(), Some("Session ended"));
        assert!(connections.load(std::sync::atomic::Ordering::Relaxed) < 5, "it connected again and again");
    }

    /// A terminal for `Screen`'s writer that keeps everything it is sent.
    #[derive(Clone, Default)]
    struct Recorded(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl io::Write for Recorded {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Recorded {
        /// What was sent since the last time, once something was and nothing more comes for a moment.
        fn take(&self) -> Vec<u8> {
            wait_until("a frame", || !self.0.lock().unwrap().is_empty());
            std::thread::sleep(std::time::Duration::from_millis(100));
            std::mem::take(&mut *self.0.lock().unwrap())
        }

        fn nothing_sent(&self) -> bool {
            std::thread::sleep(std::time::Duration::from_millis(100));
            self.0.lock().unwrap().is_empty()
        }
    }

    /// The screen a terminal of `size` shows after `bytes`, starting blank.
    fn shown_by(bytes: &[u8], size: ratatui::layout::Size) -> String {
        let mut p = crate::host::emulator(size.height, size.width, 0);
        p.process(bytes);
        p.screen().contents()
    }

    #[test]
    fn coming_back_to_the_window_draws_every_cell_again_and_otherwise_nothing_is_sent_twice() {
        let w = World::new("focus");
        let mut app = w.app();
        let term = Recorded::default();
        let mut screen = Screen::new(frames::Writer::spawn(term.clone()));
        let size = ratatui::layout::Size::new(100, 20);
        screen.show(size, |f| ui::draw(f, &app)).unwrap();
        let first = term.take();
        assert!(shown_by(&first, size).contains("Sessions"), "{}", shown_by(&first, size));
        // The same frame again, or the window going behind another one: nothing is sent.
        screen.show(size, |f| ui::draw(f, &app)).unwrap();
        on_event(&mut app, &mut screen, Event::FocusLost);
        screen.show(size, |f| ui::draw(f, &app)).unwrap();
        assert!(term.nothing_sent());
        // Back in front: the next frame is every cell, on its own a whole screen.
        on_event(&mut app, &mut screen, Event::FocusGained);
        assert!(screen.dirty);
        screen.show(size, |f| ui::draw(f, &app)).unwrap();
        let again = term.take();
        assert!(String::from_utf8_lossy(&again).contains("\x1b[2J"), "the screen is cleared first");
        assert_eq!(shown_by(&again, size), shown_by(&first, size));
        // Then differences again.
        screen.show(size, |f| ui::draw(f, &app)).unwrap();
        assert!(term.nothing_sent());
    }

    #[test]
    fn the_window_gaining_or_losing_focus_is_never_typed_into_an_open_session() {
        let w = World::new("focus-attached");
        let sock = w.base.join("host.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let (typed, input) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = crate::host::read_frame(&mut stream); // the window's size
            crate::host::write_frame(&mut stream, crate::host::F_OUTPUT, b"a session").unwrap();
            while let Ok((kind, payload)) = crate::host::read_frame(&mut stream) {
                if kind == crate::host::F_INPUT && typed.send(payload).is_err() {
                    return;
                }
            }
        });
        let mut app = attached_to(&w, 4242, &sock);
        let mut screen = Screen::new(frames::Writer::spawn(io::sink()));
        on_event(&mut app, &mut screen, Event::FocusGained);
        on_event(&mut app, &mut screen, Event::FocusLost);
        on_event(&mut app, &mut screen, Event::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)));
        // The key is the first thing the session is sent.
        assert_eq!(input.recv_timeout(std::time::Duration::from_secs(10)).unwrap(), b"x");
        assert!(matches!(app.mode, Mode::Attached(4242)), "still in the session");
    }

    #[test]
    fn a_user_theme_is_chosen_by_its_id_which_survives_the_file_going_and_coming_back() {
        let w = theme_world("tl-user-id");
        let mut app = w.app();
        open_themes(&mut app);
        key(&mut app, KeyCode::End);
        key(&mut app, KeyCode::Enter);
        assert_eq!(
            (saved_theme(&w).as_deref(), app.theme.id.as_str(), app.theme.name.as_str()),
            (Some("user:my-theme"), "user:my-theme", "My Theme")
        );
        // A new run reads it back.
        assert_eq!(w.app().theme.id, "user:my-theme");
        // The file goes: the default is shown, and the id is still on disk, even through another change.
        std::fs::remove_file(w.config.join("themes/My Theme")).unwrap();
        app.refresh_for_test();
        assert_eq!(app.theme.id, "catppuccin-mocha");
        assert_eq!(saved_theme(&w).as_deref(), Some("user:my-theme"));
        key(&mut app, KeyCode::Esc);
        ch(&mut app, 's');
        to_theme_row(&mut app);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Right);
        assert_eq!(saved_theme(&w).as_deref(), Some("user:my-theme"), "changing another setting keeps the id of a theme that is gone");
        assert_eq!(on_disk(&w).idle_stop_minutes, 240);
        // It comes back with the file, without a restart.
        put_theme(&w, "My Theme", "background = #101820\nforeground = #e6e6e6\n");
        app.refresh_for_test();
        assert_eq!(app.theme.id, "user:my-theme");
        // An edit to the file recolors a running platoon.
        put_theme(&w, "My Theme", "background = #200000\nforeground = #e6e6e6\n");
        app.refresh_for_test();
        assert_eq!(app.theme.bg, Color::Rgb(0x20, 0, 0));
        // A name that a built-in has gets its own id and is marked in the row.
        put_theme(&w, "Nord", "background = #001020\nforeground = #e6e6e6\n");
        app.refresh_for_test();
        let ids: Vec<String> = app.themes().all().into_iter().map(|t| t.id).collect();
        assert!(ids.contains(&"nord".to_string()) && ids.contains(&"user:nord".to_string()));
        assert_eq!(app.themes().by_id("user:nord").unwrap().name, "Nord (yours)");
    }

    #[test]
    fn every_theme_previewed_draws_the_list_in_its_own_colors_and_only_in_256_colors_when_asked() {
        use crate::colordepth::Depth;
        let w = theme_world("tl-sweep");
        let mut app = w.app();
        open_themes(&mut app);
        for k in 0..app.tpick.list.len() {
            app.select_theme_for_test(k);
            let t = app.effective_theme().clone();
            for (cols, rows) in [(100, 24), (80, 24), (60, 20), (60, 14)] {
                let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
                draw(&mut term, &app);
                for c in cells(&term) {
                    if t.ansi.is_some() {
                        // (the entries' swatches are other themes' colors, but still fixed ones)
                        assert!(matches!(c.bg, Color::Rgb(..)) && !matches!(c.fg, Color::Indexed(_)), "{}: {:?}/{:?}", t.id, c.fg, c.bg);
                        assert!(
                            c.symbol().trim().is_empty()
                                || matches!(
                                    c.fg,
                                    Color::Rgb(..)
                                        | Color::Reset
                                        | Color::LightBlue
                                        | Color::LightMagenta
                                        | Color::LightGreen
                                        | Color::LightYellow
                                        | Color::LightRed
                                ),
                            "{}: {:?} is unthemed text",
                            t.id,
                            c.symbol()
                        );
                    } else {
                        assert!(!matches!(c.bg, Color::Rgb(..)), "{}: {:?}", t.id, c.bg);
                    }
                }
                if t.ansi.is_some() {
                    assert_eq!(term.backend().buffer()[(1, 1)].fg, t.border, "{}: the rule under the tabs", t.id);
                }
                term.draw(|f| {
                    ui::draw(f, &app);
                    Depth::Indexed256.apply(f.buffer_mut());
                })
                .unwrap();
                assert!(
                    cells(&term).iter().all(|c| !matches!(c.fg, Color::Rgb(..)) && !matches!(c.bg, Color::Rgb(..))),
                    "{}: {cols}x{rows} in 256 colors",
                    t.id
                );
            }
        }
    }

    #[test]
    fn if_the_file_changes_under_an_untouched_list_the_highlighted_theme_is_still_what_enter_keeps() {
        let w = theme_world("tl-disk2");
        let mut app = w.app();
        open_themes(&mut app);
        std::fs::write(config_file(&w), r#"{"theme":"nord"}"#).unwrap();
        app.refresh_for_test();
        // The highlight is on Catppuccin Mocha, so that is what shows (the preview wins) and what Enter keeps.
        assert_eq!((app.theme.id.as_str(), app.effective_theme().id.as_str()), ("nord", "catppuccin-mocha"));
        key(&mut app, KeyCode::Enter);
        assert_eq!(saved_theme(&w).as_deref(), Some("catppuccin-mocha"));
        assert_eq!(app.effective_theme().id, "catppuccin-mocha");
    }
}
