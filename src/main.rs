mod app;
mod archive;
mod claude;
mod client;
mod dirs;
mod guard;
mod host;
mod index;
mod model;
mod open;
mod setup;
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
        // While the index is still reading transcripts, pick up its results more often so rows fill in smoothly.
        let every = if app.indexing().is_some() { Duration::from_millis(250) } else { Duration::from_secs(1) };
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
    fn archiving_an_idle_running_conversation_stops_it_without_asking() {
        let (w, pid) = mixed("archive-idle");
        let mut app = w.app();
        app.selected = row_of(&app, "mid-2");
        ch(&mut app, 'a');
        assert!(matches!(app.mode, Mode::List), "idle: no confirmation");
        assert!(!alive(pid), "SIGTERM must have ended it");
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
            for phrase in ["Archive \"tmp\"?", "It is working right now.", "interrupts what it is doing.", "press enter to pick it up again.", "y archive", "n cancel"] {
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
}
