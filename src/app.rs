use crate::model::{mock_sessions, mock_usage, PlanUsage, Session};
use crate::theme::Theme;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub struct App {
    pub sessions: Vec<Session>,
    pub usage: PlanUsage,
    pub selected: usize,
    /// Animation frame counter, ~10 per second.
    pub tick: u64,
    pub theme: Theme,
    pub quit: bool,
}

impl App {
    pub fn new() -> Self {
        Self {
            sessions: mock_sessions(),
            usage: mock_usage(),
            selected: 0,
            tick: 0,
            theme: Theme::catppuccin_mocha(),
            quit: false,
        }
    }

    pub fn unread_count(&self) -> usize {
        self.sessions.iter().filter(|s| s.unread).count()
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        let last = self.sessions.len().saturating_sub(1);
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => self.quit = true,
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('j') | KeyCode::Down => self.selected = (self.selected + 1).min(last),
            KeyCode::Char('k') | KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Char('g') | KeyCode::Home => self.selected = 0,
            KeyCode::Char('G') | KeyCode::End => self.selected = last,
            // Mock: jumping in marks the session as read (real jump-in comes later).
            KeyCode::Enter => {
                if let Some(s) = self.sessions.get_mut(self.selected) {
                    if !s.external {
                        s.unread = false;
                    }
                }
            }
            KeyCode::Char('x') => {
                if self.selected < self.sessions.len() {
                    self.sessions.remove(self.selected);
                    self.selected = self.selected.min(self.sessions.len().saturating_sub(1));
                }
            }
            _ => {}
        }
    }
}
