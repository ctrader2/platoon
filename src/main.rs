mod app;
mod claude;
mod client;
mod dirs;
mod guard;
mod host;
mod model;
mod open;
mod setup;
mod theme;
mod ui;
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
        if last_refresh.elapsed() >= Duration::from_secs(1) {
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
    use crate::app::{Action, App, Hit, Mode};
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

    fn click(app: &mut App, x: u16, y: u16) {
        app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: x, row: y, modifiers: KeyModifiers::NONE });
    }

    fn click_action(app: &mut App, a: Action) {
        let r = app.hits.borrow().iter().find(|(_, h)| matches!(h, Hit::Action(x) if *x == a)).map(|(r, _)| *r).expect("button not drawn");
        click(app, r.x + 1, r.y);
    }

    fn key(app: &mut App, c: KeyCode) {
        app.on_key(KeyEvent::new(c, KeyModifiers::NONE));
    }

    #[test]
    fn render_snapshot() {
        let mut term = Terminal::new(TestBackend::new(110, 16)).unwrap();
        let d = dup("render", false);
        let text = draw(&mut term, &d.app);
        assert!(text.contains("MODEL"), "{text}");
        println!("{text}");
    }

    #[test]
    fn footer_buttons_are_clickable() {
        let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
        let mut d = dup("footer", false);
        let app = &mut d.app;
        draw(&mut term, app);

        // n -> picker opens, and shows candidate directories
        click_action(app, Action::New);
        assert!(matches!(app.mode, Mode::Picker));
        let text = draw(&mut term, app);
        assert!(text.contains("new session"), "{text}");
        // clicking cancel closes it
        click_action(app, Action::Cancel);
        assert!(matches!(app.mode, Mode::List));

        // x -> confirm dialog (needs a session; this machine has at least the running one)
        draw(&mut term, app);
        if !app.sessions.is_empty() {
            click_action(app, Action::Kill);
            assert!(matches!(app.mode, Mode::Confirm(_)));
            let text = draw(&mut term, app);
            assert!(text.contains("Kill") && text.contains("inactive") && text.contains("claude --resume"), "{text}");
            println!("{text}");
            click_action(app, Action::Cancel); // never confirm in a test
            assert!(matches!(app.mode, Mode::List));
        }

        // / -> filter mode, typing filters, esc-button clears
        draw(&mut term, app);
        click_action(app, Action::Filter);
        assert!(matches!(app.mode, Mode::Filter));
        for c in "zzzz-no-such".chars() {
            key(app, KeyCode::Char(c));
        }
        assert!(app.visible().is_empty());
        assert!(draw(&mut term, app).contains("No sessions match"));
        click_action(app, Action::Cancel);
        assert!(matches!(app.mode, Mode::List) && app.filter.is_empty());

        // q with nothing hosted quits
        draw(&mut term, app);
        click_action(app, Action::Quit);
        assert!(app.quit);
    }

    #[test]
    fn jumping_into_external_session_offers_takeover() {
        let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
        let mut d = dup("takeover", false);
        d.app.refresh_with(vec![], true);
        key(&mut d.app, KeyCode::Enter);
        assert!(matches!(d.app.mode, Mode::Confirm(crate::app::Confirm::Adopt { .. })));
        let text = draw(&mut term, &d.app);
        assert!(text.contains("Take over") && text.contains("y take over") && text.contains("n cancel"), "{text}");
        click_action(&mut d.app, Action::Cancel); // never confirm in a test: it would end a real session
        assert!(matches!(d.app.mode, Mode::List));
    }

    // --- two copies of one conversation ---------------------------------------------------------

    struct Dup {
        app: App,
        a: std::process::Child, // started first
        b: std::process::Child,
        cache: std::path::PathBuf,
    }

    impl Drop for Dup {
        fn drop(&mut self) {
            let _ = self.a.kill();
            let _ = self.b.kill();
            let _ = self.a.wait();
            let _ = self.b.wait();
        }
    }

    /// Two live processes registered under the same session id, or just `b` when `both` is false.
    fn dup(name: &str, both: bool) -> Dup {
        use std::process::Command;
        let base = std::env::temp_dir().join(format!("platoon-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (root, cache) = (base.join("claude"), base.join("cache"));
        std::fs::create_dir_all(root.join("sessions")).unwrap();
        std::fs::create_dir_all(&cache).unwrap();
        let a = Command::new("sleep").arg("60").spawn().unwrap();
        let b = Command::new("sleep").arg("60").spawn().unwrap();
        let reg = |pid: u32, started: u64| {
            std::fs::write(
                root.join("sessions").join(format!("{pid}.json")),
                format!(r#"{{"pid":{pid},"sessionId":"same-id","cwd":"/tmp","kind":"interactive","startedAt":{started},"status":"idle"}}"#),
            )
            .unwrap();
        };
        if both {
            reg(a.id(), 1);
        }
        reg(b.id(), 2);
        let app = App::with_reader(crate::claude::Reader::with_root(root), cache.clone());
        Dup { app, a, b, cache }
    }

    fn hosted(pid: u32) -> crate::host::HostInfo {
        crate::host::HostInfo { claude_pid: pid, host_pid: 0, dir: "/tmp".into(), sock: Default::default() }
    }

    fn row_of(app: &App, pid: u32) -> usize {
        app.visible().iter().position(|&i| app.sessions[i].pid == pid).unwrap()
    }

    #[test]
    fn without_the_guard_platoon_pauses_its_own_copy_and_reload_is_offered_from_either_row() {
        use crate::app::{AdoptKind, JumpPlan};
        let mut d = dup("noguard", true);
        d.app.refresh_with(vec![hosted(d.a.id())], false);
        let (a, b) = (d.a.id(), d.b.id());
        let sa = d.app.sessions.iter().find(|s| s.pid == a).unwrap();
        let sb = d.app.sessions.iter().find(|s| s.pid == b).unwrap();
        assert!(sa.shared && sb.shared);
        assert!(sa.stale && !sa.external, "platoon's copy must be paused");
        assert!(!sb.stale && sb.external, "platoon can't pause another terminal's copy");
        assert!(d.cache.join("stale").join(a.to_string()).exists());
        for pid in [a, b] {
            d.app.selected = row_of(&d.app, pid);
            match d.app.jump_plan().unwrap() {
                JumpPlan::Adopt { mut pids, kind, .. } => {
                    pids.sort();
                    let mut want = vec![a, b];
                    want.sort();
                    assert_eq!((pids, kind), (want, AdoptKind::Reload));
                }
                other => panic!("expected a reload, got {other:?}"),
            }
        }
        // The pause survives another refresh and even a restart of platoon (it is a marker on disk).
        d.app.refresh_with(vec![hosted(a)], false);
        assert!(d.app.sessions.iter().find(|s| s.pid == a).unwrap().stale);
    }

    #[test]
    fn with_the_guard_the_platoon_copy_keeps_working_and_enter_on_either_row_attaches_to_it() {
        use crate::app::JumpPlan;
        let mut d = dup("guard", true);
        d.app.refresh_with(vec![hosted(d.a.id())], true);
        assert!(!d.app.sessions.iter().find(|s| s.pid == d.a.id()).unwrap().stale);
        for pid in [d.a.id(), d.b.id()] {
            d.app.selected = row_of(&d.app, pid);
            assert_eq!(d.app.jump_plan(), Some(JumpPlan::Attach(d.a.id())));
        }
    }

    #[test]
    fn single_copies_behave_as_before() {
        use crate::app::{AdoptKind, JumpPlan};
        let mut d = dup("single", false);
        d.app.refresh_with(vec![], true);
        assert_eq!(d.app.jump_plan(), Some(JumpPlan::Adopt { pids: vec![d.b.id()], kind: AdoptKind::TakeOver, busy: false }));
        d.app.refresh_with(vec![hosted(d.b.id())], true);
        assert_eq!(d.app.jump_plan(), Some(JumpPlan::Attach(d.b.id())));
    }

    #[test]
    fn paused_copy_shows_a_banner_and_the_reload_dialog_explains_itself() {
        use crate::app::{AdoptKind, Confirm};
        let mut d = dup("banner", true);
        d.app.refresh_with(vec![hosted(d.a.id())], false);
        d.app.mode = Mode::Attached(d.a.id());
        assert!(d.app.attached_paused());
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| ui::draw_paused_banner(f, &d.app, f.area())).unwrap();
        let text = screen_text(&term);
        assert!(text.contains("session paused") && text.contains("not sent") && text.contains("reload"), "{text}");
        // The other copy isn't paused.
        d.app.mode = Mode::Attached(d.b.id());
        assert!(!d.app.attached_paused());

        d.app.mode = Mode::Confirm(Confirm::Adopt {
            pids: vec![d.a.id(), d.b.id()],
            id: "same-id".into(),
            name: "x".into(),
            cwd: "/tmp".into(),
            busy: true,
            kind: AdoptKind::Reload,
        });
        let text = draw(&mut term, &d.app);
        assert!(text.contains("Reload") && text.contains("open in 2 places") && text.contains("interrupts it."), "the whole warning must be readable: {text}");
    }

    #[test]
    fn picker_filters_and_accepts_paths() {
        let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
        let mut d = dup("picker", false);
        let app = &mut d.app;
        key(app, KeyCode::Char('n'));
        for c in "/tmp".chars() {
            key(app, KeyCode::Char(c));
        }
        assert_eq!(app.picker.results.first().map(String::as_str), Some("/tmp"));
        assert!(draw(&mut term, app).contains("/tmp"));
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
            hosted_row = app.sessions.iter().any(|s| !s.external);
            if hosted_row {
                break;
            }
        }
        println!("{}", draw(&mut term, &app));
        assert!(hosted_row, "hosted claude never appeared in the session list");
        // leave, then kill it through the real confirm flow
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        let idx = app.visible().iter().position(|&i| !app.sessions[i].external).unwrap();
        app.selected = idx;
        key(&mut app, KeyCode::Char('x'));
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
