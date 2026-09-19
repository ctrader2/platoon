mod app;
mod model;
mod theme;
mod ui;

use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::{io, time::{Duration, Instant}};

fn main() -> io::Result<()> {
    let mut terminal = ratatui::init();
    let result = run(&mut terminal);
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal) -> io::Result<()> {
    let mut app = app::App::new();
    let start = Instant::now();
    while !app.quit {
        app.tick = (start.elapsed().as_millis() / 100) as u64;
        terminal.draw(|f| ui::draw(f, &app))?;
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    app.on_key(key);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn render_snapshot() {
        let mut term = Terminal::new(TestBackend::new(110, 28)).unwrap();
        let app = app::App::new();
        term.draw(|f| ui::draw(f, &app)).unwrap();
        let buf = term.backend().buffer().clone();
        let text: Vec<String> = (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect())
            .collect();
        println!("{}", text.join("\n"));
    }
}
