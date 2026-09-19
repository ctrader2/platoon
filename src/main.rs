mod app;
mod archive;
mod claude;
mod client;
mod dirs;
mod guard;
mod host;
mod index;
mod mcp;
mod model;
mod open;
mod procs;
mod setup;
mod settings;
mod theme;
mod ui;
#[cfg(test)]
mod perf;
#[cfg(test)]
mod testkit;
mod usage;

use ratatui::crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
        KeyEventKind,
    },
    execute,
};
use std::{io, time::{Duration, Instant}};

fn main() -> io::Result<()> {
    match std::env::args().nth(1).as_deref() {
        None => {}
        Some("statusline") => return statusline(),
        Some("setup") => return setup(),
        Some("open") => {
            require_setup();
            let args: Vec<String> = std::env::args().skip(2).collect();
            std::process::exit(open::run(&args));
        }
        Some("hook") if std::env::args().nth(2).as_deref() == Some("guard") => std::process::exit(guard::run_guard()),
        Some("host") => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            std::process::exit(host::run(&args));
        }
        Some(_) => {
            eprintln!("usage: platoon [setup | open [id]]\n  (no arguments)  open the session manager\n  open [id]       show a session live in this terminal (starts or resumes it if needed);\n                  with no id, start a new one here. Ctrl+Space detaches; it keeps running\n  setup           add platoon to Claude Code: plan usage in the status line, and a guard that\n                  stops two copies of one session from overwriting each other\n  statusline, hook guard   internal: run by Claude Code");
            std::process::exit(2);
        }
    }
    require_setup();
    let mut terminal = match ratatui::try_init() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("platoon needs an interactive terminal ({e}).");
            std::process::exit(1);
        }
    };
    let _ = execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste);
    let result = run(&mut terminal);
    let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    let running = result?;
    if running > 0 {
        println!(
            "platoon: {running} session{} still running in the background. Run `platoon` to pick {} up.",
            if running == 1 { "" } else { "s" },
            if running == 1 { "it" } else { "them" }
        );
    }
    Ok(())
}

/// Run by Claude Code as its status line command. Must be quick and must never fail loudly.
fn statusline() -> io::Result<()> {
    let mut input = String::new();
    io::Read::read_to_string(&mut io::stdin(), &mut input)?;
    println!("{}", usage::record(&input, &usage::cache_dir(), usage::now()));
    Ok(())
}

/// Refuse to continue until setup is complete. Only the UI and `open` are gated; the commands Claude
/// Code and the session hosts run behind the scenes never are. PLATOON_SKIP_SETUP_CHECK is for tests and CI.
fn require_setup() {
    if std::env::var_os("PLATOON_SKIP_SETUP_CHECK").is_none() {
        let states = setup::check(&claude::claude_root());
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

fn setup() -> io::Result<()> {
    let root = claude::claude_root();
    match setup::run(&root) {
        Ok(report) => println!("{}", setup::render_report(&report, &root, use_color(false))),
        Err(e) => {
            eprintln!("platoon setup failed: {e}");
            std::process::exit(1);
        }
    }
    Ok(())
}

/// Runs the UI. Returns how many platoon-hosted sessions are still running when it exits.
fn run(terminal: &mut ratatui::DefaultTerminal) -> io::Result<usize> {
    let mut app = app::App::new();
    let start = Instant::now();
    let mut last_refresh = Instant::now();
    while !app.quit {
        app.tick = (start.elapsed().as_millis() / 100) as u64;
        // While the index is still reading transcripts, pick up its results more often so rows fill in smoothly
        // (and, likewise, a settings change that is about to take effect).
        let every = if app.indexing().is_some() || app.settling() { Duration::from_millis(250) } else { Duration::from_secs(1) };
        if last_refresh.elapsed() >= every {
            app.refresh();
            last_refresh = Instant::now();
        }
        let size = terminal.size()?;
        app.sync_size(size.width, size.height);
        terminal.draw(|f| ui::draw(f, &app))?;
        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                Event::Mouse(m) => app.on_mouse(m),
                Event::Paste(text) => app.on_paste(&text),
                _ => {}
            }
        }
    }
    Ok(app.hosts.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Action, AdoptKind, App, Confirm, Hit, JumpPlan, Mode};
    use crate::model::Status;
    use crate::testkit::{alive, World};
    use ratatui::{
        backend::TestBackend,
        crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
        Terminal,
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
        crate::host::HostInfo { claude_pid: pid, host_pid: 0, dir: "/tmp".into(), sock: Default::default() }
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
        w.transcript_lines("junk-4", &[r#"{"type":"user","cwd":"/srv/x","message":{"content":"<command-name>/clear</command-name>"}}"#.into()]);
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
        assert_eq!((old.branch.as_deref(), old.model.as_deref(), old.context_pct, old.context_exact), (Some("main"), Some("Sonnet 5"), 25, false));
        // Running conversations carry their process.
        assert_eq!(by("mid-2").procs.iter().map(|p| p.pid).collect::<Vec<_>>(), [pid]);
        assert_eq!(app.counts, [3, 0]);
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
        for want in ["NAME", "DIR", "LAST", "1 Active 3", "2 Archived 0", "working", "idle", "3m", "30m", "3d", "gamma", "write the docs", "Sonnet 5", "main", "~ 25%"] {
            assert!(text.contains(want), "missing {want:?}:\n{text}");
        }
        for banned in ["ext", "paused", "2 places", "kill", "x kill", "not open", "open elsewhere", "in platoon"] {
            assert!(!text.contains(banned), "{banned:?} must not appear:\n{text}");
        }
        assert!(text.contains("enter open") && text.contains("a archive") && text.contains("1-2 tabs"), "{text}");
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
        assert!(text.contains(" 61%") && !text.contains("~ 25%") && !text.contains("~"), "{text}");
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
        while alive(pid) {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
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
        assert_eq!(app.tab, 0);
        assert_eq!(ids(&app), ["new-3", "mid-2"]);
        assert!(draw(&mut term, &app).contains("1 Active 2   2 Archived 1"));

        ch(&mut app, '2');
        assert_eq!((app.tab, ids(&app)), (1, vec!["old-1"]));
        ch(&mut app, '3'); // no third tab yet
        assert_eq!(app.tab, 1);
        ch(&mut app, '1');
        assert_eq!((app.tab, app.selected), (0, 0));

        draw(&mut term, &app);
        click_tab(&mut app, 1);
        assert_eq!(app.tab, 1);
        draw(&mut term, &app);
        click_tab(&mut app, 0);
        assert_eq!(app.tab, 0);

        // Digits are plain text while typing a filter.
        ch(&mut app, '/');
        ch(&mut app, '2');
        assert_eq!((app.tab, app.filter.as_str()), (0, "2"));
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
        ch(&mut app, 'j');
        assert_eq!(app.selected_session().unwrap().id, "mid-2");
        // Filtering only searches the tab you're on.
        ch(&mut app, '/');
        for c in "alpha".chars() {
            ch(&mut app, c);
        }
        assert!(app.visible().is_empty(), "alpha is archived, so not on Active");
        key(&mut app, KeyCode::Esc);
        ch(&mut app, '2');
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

        ch(&mut app, '2');
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
        assert!(matches!(app.mode, Mode::Confirm(Confirm::Archive { outside: true, busy: false, .. })), "it isn't platoon's process to end unasked");
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let text = flat(&draw(&mut term, &app));
        for phrase in ["outside platoon", "Archiving ends that copy.", "The conversation is kept", "y archive", "n cancel"] {
            assert!(text.contains(phrase), "{phrase:?} cut off in:\n{text}");
        }
        assert!(!text.contains("working right now"), "it's idle: {text}");

        ch(&mut app, 'n');
        assert!(matches!(app.mode, Mode::List) && alive(pid) && w.archive_file().is_none(), "cancel touches nothing");

        ch(&mut app, 'a');
        ch(&mut app, 'y');
        assert!(matches!(app.mode, Mode::List));
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
            for phrase in ["Archive \"tmp\"?", "outside platoon", "working right now.", "interrupts what it is doing.", "press enter to pick it up again.", "y archive", "n cancel"] {
                assert!(text.contains(phrase), "{size:?}: {phrase:?} cut off in:\n{text}");
            }
        }
        ch(&mut app, 'n');
        assert!(matches!(app.mode, Mode::List) && alive(pid) && w.archive_file().is_none());

        ch(&mut app, 'a');
        ch(&mut app, 'y');
        assert!(matches!(app.mode, Mode::List));
        assert!(!alive(pid) && w.archive_file().unwrap()["archived"].get("busy-1").is_some());
        assert_eq!(app.counts, [0, 1]);
    }

    // --- automatic clean-up ---------------------------------------------------------------------------

    const THREE_DAYS: u64 = 3 * 24 * 60;

    /// The one-time notice, as it must read.
    fn notice_of(n: usize) -> String {
        format!("Auto-archived {n} conversations idle for 2+ days. Find them under Archived.")
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
            assert!(text.contains("1 Active 1") && text.contains("2 Archived 143"), "{text}");
        }
        // It covers the key hints, so it goes as soon as the user does anything.
        let mut app = app;
        let mut term = Terminal::new(TestBackend::new(110, 24)).unwrap();
        assert!(!draw(&mut term, &app).contains("enter open"));
        ch(&mut app, 'j');
        let text = draw(&mut term, &app);
        assert!(text.contains("enter open") && !text.contains("Auto-archived"), "{text}");
    }

    #[test]
    fn the_notice_is_singular_for_one_and_names_the_configured_age() {
        let w = World::new("auto-one");
        w.settings(120, 1);
        w.transcript("old", "/srv/old", "old work", 25 * 60);
        let app = w.app();
        assert_eq!(app.active_notice(), Some("Auto-archived 1 conversation idle for 1+ day. Find it under Archived."));
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
        assert_eq!(app.active_notice(), Some("Auto-archived 1 conversation idle for 2+ days. Find it under Archived."));

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

        ch(&mut app, '2');
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
        assert!(text.contains("new session"), "{text}");
        click_action(&mut app, Action::Cancel);
        assert!(matches!(app.mode, Mode::List));

        // a -> archives the selected conversation (it is idle and not running)
        draw(&mut term, &app);
        let first = app.selected_session().unwrap().id.clone();
        click_action(&mut app, Action::Archive);
        assert!(!ids(&app).contains(&first.as_str()));

        // / -> filter mode, typing filters, esc-button clears
        draw(&mut term, &app);
        click_action(&mut app, Action::Filter);
        assert!(matches!(app.mode, Mode::Filter));
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
        ch(&mut app, '2');
        draw(&mut term, &app);
        let buf = term.backend().buffer();
        let row = (0..buf.area.height).find(|&y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>().contains("hello")).unwrap();
        let name_cell = (0..buf.area.width).map(|x| &buf[(x, row - 1)]).find(|c| c.symbol() == "a").unwrap();
        assert_eq!(name_cell.fg, app.theme.dim);
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
        assert!(text.contains("session paused") && text.contains("not sent") && text.contains("reload"), "{text}");
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
        assert!(text.contains("Reload") && text.contains("open in 2 places") && text.contains("interrupts it."), "the whole warning must be readable: {text}");
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
            app.refresh();
            if app.hosts.is_empty() {
                break;
            }
        }
        assert!(app.hosts.is_empty(), "claude didn't exit after SIGTERM");
    }

    // --- the settings screen ------------------------------------------------------------------------

    use crate::settings::{self, Settings, ITEMS};

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

    /// A screen on which the settings are open, after `draw` so clicks have somewhere to land.
    fn settings_screen(w: &World, cols: u16, rows: u16) -> (App, Terminal<TestBackend>) {
        let mut app = w.app();
        ch(&mut app, 's');
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
        assert!(matches!(app.mode, Mode::Filter) && app.filter == "s", "an s typed into the filter is text");
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
        ch(&mut app, 's');
        assert_eq!(app.setting, 0);

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
        assert_eq!(app.setting, 1);
        key(&mut app, KeyCode::Right);
        assert_eq!(on_disk(&w), Settings { idle_stop_minutes: 0, auto_archive_days: 7, ..Settings::default() }, "the other setting is left alone");
        ch(&mut app, 'k');
        assert_eq!(app.setting, 0);
        ch(&mut app, 'j');
        key(&mut app, KeyCode::Up);
        assert_eq!(app.setting, 0);
        assert_eq!(ITEMS.len(), 3);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Down);
        assert_eq!(app.setting, 2, "no wrapping past the last");
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
        ch(&mut app, 's');
        assert_eq!(std::fs::read_to_string(config_file(&w)).unwrap(), text, "looking at the screen never rewrites the file");

        key(&mut app, KeyCode::Right);
        assert_eq!(on_disk(&w).idle_stop_minutes, 120, "90 goes up to the next step");
        std::fs::write(config_file(&w), text).unwrap();
        key(&mut app, KeyCode::Left);
        assert_eq!(on_disk(&w).idle_stop_minutes, 60, "and down to the previous one");
        std::fs::write(config_file(&w), text).unwrap();
        key(&mut app, KeyCode::Enter);
        assert_eq!(on_disk(&w).idle_stop_minutes, 120, "enter goes to the next step too");
        ch(&mut app, 'j');
        key(&mut app, KeyCode::Left);
        assert_eq!(on_disk(&w).auto_archive_days, 2, "5 days steps down to 2");
    }

    #[test]
    fn the_file_keeps_keys_platoon_does_not_know_including_nested_ones() {
        let w = World::new("set-unknown");
        std::fs::write(config_file(&w), r#"{"version":1,"theme":{"name":"latte","tweaks":[1,{"a":null}]},"idle_stop_minutes":120,"note":"hi"}"#).unwrap();
        let (mut app, _) = settings_screen(&w, 100, 30);
        key(&mut app, KeyCode::Right);
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(config_file(&w)).unwrap()).unwrap();
        assert_eq!(v["theme"], serde_json::json!({"name":"latte","tweaks":[1,{"a":null}]}));
        assert_eq!((v["note"].as_str(), v["idle_stop_minutes"].as_u64(), v["auto_archive_days"].as_u64()), (Some("hi"), Some(240), Some(2)));
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
                line.join("").find(word).map(|i| buf[(line[..i].len() as u16, y)].fg)
            })
        };
        assert_eq!(cell_fg("Off"), Some(app.theme.dim), "Off is muted");
        assert_eq!(cell_fg("2 days"), Some(app.theme.accent), "an active value stands out");
        // Every step reads as words a person would say.
        let labels: Vec<Vec<String>> = ITEMS.iter().map(|i| i.choices.iter().map(|&c| (i.label)(c)).collect()).collect();
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
        let row1 = hit_rect(&app, |h| matches!(h, Hit::Setting(1)));

        // Hover lights the row up; the selected row is lit too.
        assert_eq!(term.backend().buffer()[(row1.x + 5, row1.y)].bg, bg);
        hover(&mut app, row1.x + 5, row1.y);
        draw(&mut term, &app);
        assert_eq!(term.backend().buffer()[(row1.x + 5, row1.y)].bg, surface, "hovered row");
        let row0 = hit_rect(&app, |h| matches!(h, Hit::Setting(0)));
        assert_eq!(term.backend().buffer()[(row0.x + 5, row0.y)].bg, surface, "selected row");

        // A click on a row selects it and changes nothing.
        click(&mut app, row1.x + 5, row1.y);
        assert_eq!((app.setting, on_disk(&w)), (1, Settings { idle_stop_minutes: 120, auto_archive_days: 2, ..Settings::default() }));
        draw(&mut term, &app);

        // The buttons step; clicking one on another row selects that row and steps it.
        let next0 = hit_rect(&app, |h| matches!(h, Hit::SettingStep(0, settings::Step::Next)));
        click(&mut app, next0.x + 1, next0.y);
        assert_eq!((app.setting, on_disk(&w).idle_stop_minutes), (0, 240));
        draw(&mut term, &app);
        let prev1 = hit_rect(&app, |h| matches!(h, Hit::SettingStep(1, settings::Step::Prev)));
        click(&mut app, prev1.x + 1, prev1.y);
        assert_eq!((app.setting, on_disk(&w).auto_archive_days), (1, 1));
        draw(&mut term, &app);
        let prev1 = hit_rect(&app, |h| matches!(h, Hit::SettingStep(1, settings::Step::Prev)));
        click(&mut app, prev1.x + 1, prev1.y);
        assert_eq!(on_disk(&w).auto_archive_days, 0);
        draw(&mut term, &app);

        // Clicking the value cycles (and wraps).
        let value1 = hit_rect(&app, |h| matches!(h, Hit::SettingStep(1, settings::Step::Cycle)));
        click(&mut app, value1.x + 1, value1.y);
        assert_eq!(on_disk(&w).auto_archive_days, 1);
        draw(&mut term, &app);
        for _ in 0..5 {
            click(&mut app, value1.x + 1, value1.y);
        }
        assert_eq!(on_disk(&w).auto_archive_days, 0, "wrapped back to Off");

        // The wheel moves between the settings.
        app.on_mouse(MouseEvent { kind: MouseEventKind::ScrollUp, column: 5, row: 5, modifiers: KeyModifiers::NONE });
        assert_eq!(app.setting, 0);
    }

    #[test]
    fn the_step_buttons_of_the_first_and_last_choice_are_muted() {
        let w = World::new("set-ends");
        w.settings(0, 30);
        let (app, term) = settings_screen(&w, 100, 30);
        let border = app.theme.border;
        let accent = app.theme.accent;
        let buf = term.backend().buffer();
        let prev0 = hit_rect(&app, |h| matches!(h, Hit::SettingStep(0, settings::Step::Prev)));
        let next0 = hit_rect(&app, |h| matches!(h, Hit::SettingStep(0, settings::Step::Next)));
        let next1 = hit_rect(&app, |h| matches!(h, Hit::SettingStep(1, settings::Step::Next)));
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
        let mut app = App::with_dirs(crate::claude::Reader::with_root(w.root.clone()), w.cache.clone(), blocker.join("config"));
        ch(&mut app, 's');
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
            ch(&mut app, 's');
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
        ch(&mut app, 'j');
        key(&mut app, KeyCode::Right); // Off -> 1 day
        assert!(app.settling());
        app.refresh_for_test();
        assert_eq!(app.counts, [2, 0], "held while the value may still be changing");
        app.settle_for_test();
        app.refresh_for_test(); // the very next refresh, not a timed check a minute later
        assert_eq!(app.counts, [1, 1]);
        assert!(!app.settling());
        let shown = draw(&mut term, &app);
        assert!(flat(&shown).contains("Auto-archived 1 conversation idle for 1+ day. Find it under Archived."), "{shown}");
        key(&mut app, KeyCode::Esc);
        ch(&mut app, '2');
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
            ch(&mut app, 's');
            ch(&mut app, 'j');
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
        for (cols, rows) in [(80, 24), (100, 30), (140, 30), (60, 20), (80, 20)] {
            let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
            let shown = draw(&mut term, &app);
            let text = flat(&shown);
            let ctx = format!("{cols}x{rows}\n{shown}");
            // Where every explanation doesn't fit with room to breathe, the selected one does (and is whole).
            let roomy = rows >= 24;
            for (i, item) in ITEMS.iter().enumerate() {
                assert!(text.contains(item.name), "{ctx}");
                let must_explain = roomy || i == 0;
                assert!(!must_explain || text.contains(item.explain), "explanation cut off or mangled at {ctx}");
            }
            for want in ["2 hours", "2 days", "Ignore MCP servers", "‹", "›", "Changes are saved and apply immediately.", "enter next choice", "esc back", file] {
                assert!(text.contains(want), "{want:?} missing at {ctx}");
            }
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
    fn the_mcp_entry_reads_plainly_and_is_whole_at_80_100_and_140_columns_and_at_60_by_20() {
        let w = World::new("set-mcp-render");
        let mut app = w.app();
        ch(&mut app, 's');
        for (cols, rows) in [(80, 24), (100, 30), (140, 30), (60, 20), (60, 16)] {
            ch(&mut app, 'G'); // the last entry is the one under test, and (small terminals) the one explained
            assert_eq!(app.setting, ITEMS.len() - 1);
            let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
            let shown = draw(&mut term, &app);
            let text = flat(&shown);
            let ctx = format!("{cols}x{rows}\n{shown}");
            for want in ["Idle stop and MCP servers", "Ignore MCP servers", "helper programs", "they are ignored", "start again when you open it", "Count MCP servers", ITEMS[2].explain] {
                assert!(text.contains(want), "{want:?} missing at {ctx}");
            }
            let lower = text.to_lowercase();
            for jargon in ["sigterm", "pid", "descendant", "child", "json:", "stdio", "subprocess", "daemon"] {
                assert!(!lower.contains(jargon), "{jargon:?} in {ctx}");
            }
            for line in shown.lines().filter(|l| l.starts_with('│')) {
                assert!(line.trim_end_matches('│').ends_with(' '), "text touches the border at {ctx}");
            }
            assert!(text.contains("esc back") && text.contains("settings.json"), "{ctx}");
            ch(&mut app, 'g');
        }
    }

    #[test]
    fn the_mcp_entry_steps_with_keys_and_mouse_and_is_saved_as_a_boolean() {
        let w = World::new("set-mcp-input");
        let on_disk_flag = || serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(config_file(&w)).unwrap()).unwrap()["idle_stop_ignores_mcp"].clone();
        let (mut app, mut term) = settings_screen(&w, 100, 30);
        ch(&mut app, 'G');
        assert_eq!(app.setting, 2);
        assert!(draw(&mut term, &app).contains("Ignore MCP servers"), "the default");
        assert!(!config_file(&w).exists() || on_disk(&w).idle_stop_ignores_mcp, "looking at it changes nothing");

        key(&mut app, KeyCode::Left);
        assert_eq!((on_disk_flag(), on_disk(&w).idle_stop_ignores_mcp), (serde_json::json!(false), false));
        assert!(draw(&mut term, &app).contains("Count MCP servers"));
        key(&mut app, KeyCode::Left);
        assert_eq!(on_disk_flag(), serde_json::json!(false), "the left end holds");
        key(&mut app, KeyCode::Right);
        assert_eq!(on_disk_flag(), serde_json::json!(true));
        key(&mut app, KeyCode::Right);
        assert_eq!(on_disk_flag(), serde_json::json!(true), "and so does the right end");
        for (k, want) in [('h', false), ('l', true), ('-', false), ('+', true), ('=', true), (' ', false), (' ', true)] {
            ch(&mut app, k);
            assert_eq!(on_disk(&w).idle_stop_ignores_mcp, want, "{k:?}");
        }
        key(&mut app, KeyCode::Enter);
        assert!(!on_disk(&w).idle_stop_ignores_mcp, "enter goes to the other choice");
        key(&mut app, KeyCode::Enter);
        assert!(on_disk(&w).idle_stop_ignores_mcp, "and wraps back");
        assert_eq!(on_disk(&w).idle_stop_minutes, 120, "the other settings are untouched");

        // The mouse: the buttons step, the value toggles, a click on the row selects it.
        ch(&mut app, 'g');
        draw(&mut term, &app);
        let prev = hit_rect(&app, |h| matches!(h, Hit::SettingStep(2, settings::Step::Prev)));
        click(&mut app, prev.x + 1, prev.y);
        assert_eq!((app.setting, on_disk(&w).idle_stop_ignores_mcp), (2, false), "a button on another row selects it and steps");
        draw(&mut term, &app);
        let value = hit_rect(&app, |h| matches!(h, Hit::SettingStep(2, settings::Step::Cycle)));
        click(&mut app, value.x + 1, value.y);
        assert!(on_disk(&w).idle_stop_ignores_mcp);
        draw(&mut term, &app);
        let next = hit_rect(&app, |h| matches!(h, Hit::SettingStep(2, settings::Step::Next)));
        click(&mut app, next.x + 1, next.y);
        assert!(on_disk(&w).idle_stop_ignores_mcp, "already at the end");
        let buf = term.backend().buffer();
        assert_eq!(buf[(next.x + 1, next.y)].fg, app.theme.border, "the button that can't go further is muted");
        // Neither choice is drawn as "off": both read as a live value.
        let accent_of = |app: &App, term: &Terminal<TestBackend>| {
            let row = hit_rect(app, |h| matches!(h, Hit::SettingStep(2, settings::Step::Cycle)));
            term.backend().buffer()[(row.x + 2, row.y)].fg
        };
        assert_eq!(accent_of(&app, &term), app.theme.accent);
        click(&mut app, value.x + 1, value.y);
        draw(&mut term, &app);
        assert_eq!(accent_of(&app, &term), app.theme.accent, "Count MCP servers is a choice, not 'off'");
    }

    #[test]
    fn the_mcp_entry_survives_odd_files_and_keeps_what_else_is_in_them() {
        let w = World::new("set-mcp-odd");
        std::fs::write(config_file(&w), r#"{"idle_stop_ignores_mcp":"no","keep":{"a":[1]},"idle_stop_minutes":60}"#).unwrap();
        let (mut app, mut term) = settings_screen(&w, 100, 30);
        assert!(draw(&mut term, &app).contains("Ignore MCP servers"), "a wrong-typed value shows the default");
        ch(&mut app, 'G');
        key(&mut app, KeyCode::Left);
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(config_file(&w)).unwrap()).unwrap();
        assert_eq!((v["idle_stop_ignores_mcp"].clone(), v["keep"].clone(), v["idle_stop_minutes"].clone()), (serde_json::json!(false), serde_json::json!({"a": [1]}), serde_json::json!(60)));
        // An edit made elsewhere shows up on the open screen.
        std::fs::write(config_file(&w), r#"{"idle_stop_ignores_mcp":true}"#).unwrap();
        app.refresh_for_test();
        assert!(draw(&mut term, &app).contains("Ignore MCP servers"));
    }

    #[test]
    fn a_very_short_terminal_still_shows_the_settings_and_the_way_back() {
        let w = World::new("set-short");
        let mut app = w.app();
        ch(&mut app, 's');
        for rows in [8, 10, 12, 14, 16] {
            let mut term = Terminal::new(TestBackend::new(60, rows)).unwrap();
            let text = draw(&mut term, &app); // must not panic
            assert!(text.contains("Stop idle sessions after") && text.contains("esc back"), "60x{rows}\n{text}");
            if rows >= 12 {
                assert!(text.contains("Archive conversations after"), "60x{rows}\n{text}");
            }
        }
        // With little room only the selected setting explains itself, and moving on moves the explanation.
        let mut term = Terminal::new(TestBackend::new(60, 14)).unwrap();
        let first = flat(&draw(&mut term, &app));
        assert!(first.contains(ITEMS[0].explain) && !first.contains(ITEMS[1].explain), "{first}");
        ch(&mut app, 'j');
        let second = flat(&draw(&mut term, &app));
        assert!(second.contains(ITEMS[1].explain) && !second.contains(ITEMS[0].explain), "{second}");
        // The long MCP explanation needs a few more lines: the reassurance line steps aside for it at 16.
        ch(&mut app, 'j');
        let mut term = Terminal::new(TestBackend::new(60, 16)).unwrap();
        let third = draw(&mut term, &app);
        assert!(flat(&third).contains(ITEMS[2].explain) && !third.contains("apply immediately"), "{third}");
        let mut term = Terminal::new(TestBackend::new(60, 12)).unwrap();
        let cramped = flat(&draw(&mut term, &app));
        assert!(cramped.contains(ITEMS[2].name) && cramped.contains("esc back"), "{cramped}");
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
        ch(&mut app, 'j');
        ch(&mut app, 's');
        ch(&mut app, '3');
        assert!(matches!(app.mode, Mode::Settings));
        key(&mut app, KeyCode::Esc);
        assert_eq!((app.tab, app.selected, ids(&app)), (0, 1, vec!["aaa", "bbb"]));
    }
}
