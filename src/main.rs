mod app;
mod claude;
mod dirs;
mod host;
mod model;
mod theme;
mod ui;

use ratatui::crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
        KeyEventKind,
    },
    execute,
};
use std::{io, time::{Duration, Instant}};

fn main() -> io::Result<()> {
    let mut terminal = ratatui::init();
    let _ = execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste);
    let result = run(&mut terminal);
    let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal) -> io::Result<()> {
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
    Ok(())
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
        let app = app::App::new();
        println!("{}", draw(&mut term, &app));
    }

    #[test]
    fn footer_buttons_are_clickable() {
        let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
        let mut app = App::new();
        draw(&mut term, &app);

        // n -> picker opens, and shows candidate directories
        click_action(&mut app, Action::New);
        assert!(matches!(app.mode, Mode::Picker));
        let text = draw(&mut term, &app);
        assert!(text.contains("new session"), "{text}");
        // clicking cancel closes it
        click_action(&mut app, Action::Cancel);
        assert!(matches!(app.mode, Mode::List));

        // x -> confirm dialog (needs a session; this machine has at least the running one)
        draw(&mut term, &app);
        if !app.sessions.is_empty() {
            click_action(&mut app, Action::Kill);
            assert!(matches!(app.mode, Mode::Confirm(_)));
            assert!(draw(&mut term, &app).contains("Kill"));
            click_action(&mut app, Action::Cancel); // never confirm in a test
            assert!(matches!(app.mode, Mode::List));
        }

        // / -> filter mode, typing filters, esc-button clears
        draw(&mut term, &app);
        click_action(&mut app, Action::Filter);
        assert!(matches!(app.mode, Mode::Filter));
        for c in "zzzz-no-such".chars() {
            key(&mut app, KeyCode::Char(c));
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
    fn jumping_into_external_session_offers_takeover() {
        let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
        let mut app = App::new();
        if app.sessions.is_empty() {
            return; // nothing running on this machine to click on
        }
        assert!(app.sessions[0].external);
        key(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Confirm(crate::app::Confirm::Adopt { .. })));
        let text = draw(&mut term, &app);
        assert!(text.contains("Take over") && text.contains("y take over") && text.contains("n cancel"), "{text}");
        click_action(&mut app, Action::Cancel); // never confirm in a test: it would end a real session
        assert!(matches!(app.mode, Mode::List));
    }

    #[test]
    fn picker_filters_and_accepts_paths() {
        let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
        let mut app = App::new();
        key(&mut app, KeyCode::Char('n'));
        for c in "/tmp".chars() {
            key(&mut app, KeyCode::Char(c));
        }
        assert_eq!(app.picker.results.first().map(String::as_str), Some("/tmp"));
        assert!(draw(&mut term, &app).contains("/tmp"));
    }

    #[test]
    fn hosted_session_renders_and_takes_input() {
        let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
        let mut app = App::new();
        app.sync_size(100, 20);
        let mut h = host::Hosted::spawn("sh", &[], std::path::Path::new("/tmp"), 17, 98).unwrap();
        // Type into the shell through the same path real key presses take.
        h.write(b"echo platoon-$((40+2))\r");
        let pid = h.pid;
        app.hosted.push(h);
        app.mode = Mode::Attached(pid);
        let mut text = String::new();
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            text = draw(&mut term, &app);
            if text.contains("platoon-42") {
                break;
            }
        }
        assert!(text.contains("platoon-42"), "{text}");
        assert!(text.contains("back to list"));
        // ctrl+space leaves the attached view
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        assert!(matches!(app.mode, Mode::List));
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
            if app.hosted.is_empty() {
                break;
            }
        }
        assert!(app.hosted.is_empty(), "claude didn't exit after SIGTERM");
    }
}
