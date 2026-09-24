//! The hand-off inside the app: `H` on a session that is in a project asks Claude for a brief (what was done,
//! what was decided, what is next), shows it in a box the user can edit, and on accept starts a **new** session in
//! the same folder and the same project with that text as its first message. The old session is left exactly as
//! it is: nothing is archived and nothing is stopped.
//!
//! The call runs on a thread of its own, like the other token work, so the screen keeps going while it is out.
//! What is sent and what an answer may be is in `handoff.rs`; this file gathers the material, runs the call,
//! keeps the box and starts the session.

use super::*;
use crate::ai::{AiError, Request};
use crate::handoff::{self, Brief};
use std::sync::mpsc::TryRecvError;

/// How long a hand-off call may take before it is given up on. It is one small call, but it is the user's own
/// key press, so it gets a little longer than a background one.
const HANDOFF_TIMEOUT: Duration = Duration::from_secs(90);

/// What the hand-off box is showing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandoffState {
    /// The call is out.
    Asking,
    /// The brief is there, and `text` is what will be sent.
    Ready,
    /// Why there is no brief, in plain words.
    Failed(String),
}

/// The hand-off box while it is open.
pub struct HandoffBox {
    pub session: String,
    pub session_name: String,
    pub project: String,
    pub project_name: String,
    /// The folder the new session starts in: the old one's.
    pub cwd: String,
    pub state: HandoffState,
    /// The brief, as the user may edit it.
    pub text: String,
    /// Where the next character goes, as a character index into `text`.
    pub cursor: usize,
    /// The first line of the brief on screen, kept between frames so it only scrolls when it has to.
    pub top: Cell<usize>,
}

impl HandoffBox {
    pub fn empty() -> Self {
        HandoffBox {
            session: String::new(),
            session_name: String::new(),
            project: String::new(),
            project_name: String::new(),
            cwd: String::new(),
            state: HandoffState::Asking,
            text: String::new(),
            cursor: 0,
            top: Cell::new(0),
        }
    }

    /// The lines of the brief and where the cursor sits in them (line, column), in characters.
    pub fn lines(&self) -> Vec<String> {
        self.text.split('\n').map(str::to_string).collect()
    }

    pub fn cursor_at(&self) -> (usize, usize) {
        let mut seen = 0usize;
        for (n, line) in self.text.split('\n').enumerate() {
            let len = line.chars().count();
            if self.cursor <= seen + len {
                return (n, self.cursor - seen);
            }
            seen += len + 1; // the line break itself
        }
        (0, 0)
    }

    fn index_of(&self, line: usize, col: usize) -> usize {
        let mut seen = 0usize;
        for (n, text) in self.text.split('\n').enumerate() {
            let len = text.chars().count();
            if n == line {
                return seen + col.min(len);
            }
            seen += len + 1;
        }
        self.text.chars().count()
    }

    fn byte_at(&self, chars: usize) -> usize {
        self.text.char_indices().nth(chars).map_or(self.text.len(), |(i, _)| i)
    }

    pub(super) fn insert(&mut self, text: &str) {
        if self.text.chars().count() + text.chars().count() > handoff::TEXT_MAX {
            return;
        }
        let at = self.byte_at(self.cursor);
        self.text.insert_str(at, text);
        self.cursor += text.chars().count();
    }

    pub(super) fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let at = self.byte_at(self.cursor - 1);
        let to = self.byte_at(self.cursor);
        self.text.replace_range(at..to, "");
        self.cursor -= 1;
    }

    pub(super) fn delete(&mut self) {
        let len = self.text.chars().count();
        if self.cursor >= len {
            return;
        }
        let at = self.byte_at(self.cursor);
        let to = self.byte_at(self.cursor + 1);
        self.text.replace_range(at..to, "");
    }

    pub(super) fn move_cursor(&mut self, delta: i32) {
        let len = self.text.chars().count() as i32;
        self.cursor = (self.cursor as i32 + delta).clamp(0, len) as usize;
    }

    /// Up or down a line, keeping the column where it can.
    pub(super) fn move_line(&mut self, delta: i32) {
        let (line, col) = self.cursor_at();
        let last = self.text.split('\n').count().saturating_sub(1) as i32;
        let to = (line as i32 + delta).clamp(0, last) as usize;
        self.cursor = self.index_of(to, col);
    }

    pub(super) fn to_line_start(&mut self) {
        let (line, _) = self.cursor_at();
        self.cursor = self.index_of(line, 0);
    }

    pub(super) fn to_line_end(&mut self) {
        let (line, _) = self.cursor_at();
        self.cursor = self.index_of(line, usize::MAX);
    }
}

impl App {
    /// `H`: hand this conversation on to a new one. Everything that would make it pointless is said plainly and
    /// changes nothing: no conversation selected, it is in no project, its project is archived, its folder is
    /// gone, there is nothing in it yet, or the gate says no.
    pub(super) fn start_handoff(&mut self) {
        let Some(s) = self.selected_session() else {
            self.notify("No conversation selected");
            return;
        };
        let (session, session_name, cwd) = (s.id.clone(), s.name.clone(), s.cwd.clone());
        let Some(project) = self.projects.of(&session).cloned() else {
            self.notify("\"{name}\" is in no project. Press m to put it in one first.".replace("{name}", &session_name));
            return;
        };
        if project.archived {
            self.notify(format!("The project \"{}\" is archived. Restore it first.", project.name));
            return;
        }
        if !self.cwd_exists(&session_name, &cwd) {
            return;
        }
        let settings = self.settings.get();
        // The user asked for this themselves: the plan-usage pause doesn't apply, every other gate does.
        if let Err(r) = ai::gate_for(&settings, handoff::FEATURE, &self.cache, self.clock(), true) {
            let mut text = r.plain();
            if r.is_switch() {
                text.push_str(" Turn it on in the settings (press s, then Smart features).");
            }
            self.notify(format!("No hand-off: {text}"));
            return;
        }
        let material = self.handoff_material(&session, &session_name, &cwd, &project.name);
        if material.is_empty() {
            self.notify(format!("There is nothing in \"{session_name}\" yet to hand on."));
            return;
        }
        let mut req = Request::new(handoff::FEATURE, handoff::system(), handoff::prompt(&material));
        req.user_initiated = true;
        req.timeout = HANDOFF_TIMEOUT;
        req.cancel = self.switchboard.token(handoff::FEATURE);
        let rx = self.spawn_call(settings.clone(), req);
        self.handoff = HandoffBox {
            session,
            session_name,
            project: project.id.clone(),
            project_name: project.name.clone(),
            cwd,
            state: HandoffState::Asking,
            text: String::new(),
            cursor: 0,
            top: Cell::new(0),
        };
        self.handoff_flight = Some(rx);
        self.mode = Mode::Handoff;
    }

    /// What one hand-off call is about: what platoon already knows this conversation is, and the last few things
    /// said in it. The project's notes are deliberately not in it - the new session gets those from the hook.
    ///
    /// The conversation is read from its transcript there and then, not from the prompt index: the brief has to
    /// reflect what was said a moment ago, and a session platoon has only just noticed may not be indexed yet.
    fn handoff_material(&self, session: &str, name: &str, cwd: &str, project: &str) -> handoff::Material {
        let folder = Path::new(cwd).file_name().and_then(|n| n.to_str()).filter(|n| !n.is_empty()).unwrap_or(cwd).to_string();
        let mut m = handoff::Material {
            project: project.to_string(),
            folder,
            name: name.to_string(),
            summary: self.summaries.get(session).map(|e| e.summary.clone()).filter(|s| !s.is_empty()),
            ..handoff::Material::default()
        };
        let Some(path) = crate::claude::find_transcript(self.reader.root(), session) else { return m };
        let read = crate::notesrun::read_session(&path, handoff::MESSAGE_CHARS);
        let mut messages: Vec<String> = read.recent.iter().rev().take(handoff::MESSAGES).rev().cloned().collect();
        // What it set out to do matters as much as what was said last, so the first message comes too.
        if let Some(first) = &read.first {
            if !messages.first().is_some_and(|x| x == first) {
                messages.insert(0, first.clone());
            }
        }
        m.messages = messages;
        m.reply = read.reply;
        m
    }

    /// Take the answer in when it arrives. Called on every refresh; cheap when nothing is out.
    pub(super) fn poll_handoff(&mut self) {
        let Some(rx) = self.handoff_flight.take() else { return };
        let answer = match rx.try_recv() {
            Ok(a) => a,
            Err(TryRecvError::Empty) => {
                self.handoff_flight = Some(rx);
                return;
            }
            Err(TryRecvError::Disconnected) => Err(AiError::Failed { kind: crate::ledger::FailKind::Other, detail: "the call ended without an answer".into() }),
        };
        // The box was closed while it ran: nobody is waiting for this.
        if !matches!(self.mode, Mode::Handoff) || self.handoff.state != HandoffState::Asking {
            return;
        }
        match answer {
            Ok(reply) => match handoff::parse(&reply.text) {
                Some(brief) => self.show_brief(&brief),
                None => self.handoff.state = HandoffState::Failed("Claude's answer couldn't be understood, so there is no brief. Nothing has changed.".into()),
            },
            Err(AiError::Cancelled) => self.mode = Mode::List,
            Err(e) => self.handoff.state = HandoffState::Failed(format!("{} Nothing has changed.", e.plain())),
        }
    }

    fn show_brief(&mut self, brief: &Brief) {
        self.handoff.text = handoff::render(brief);
        self.handoff.cursor = self.handoff.text.chars().count();
        self.handoff.top.set(0);
        self.handoff.state = HandoffState::Ready;
    }

    /// Esc: call off whatever is out and leave everything as it was.
    pub(super) fn cancel_handoff(&mut self) {
        if self.handoff_flight.take().is_some() {
            self.switchboard.cancel(handoff::FEATURE);
        }
        self.mode = Mode::List;
    }

    /// Ctrl+S in the box: start the new session with this text.
    pub(super) fn take_handoff(&mut self) {
        if self.handoff.state != HandoffState::Ready {
            return;
        }
        let text = self.handoff.text.trim().to_string();
        if let Some(why) = handoff::why_not(&text) {
            self.notify(why);
            return; // the box stays open, with the text exactly as it was
        }
        let (project, cwd, name) = (self.handoff.project.clone(), self.handoff.cwd.clone(), self.handoff.project_name.clone());
        // It may have been moved out of the project in another window while the brief was being read: the new
        // session must not end up somewhere the user didn't ask for.
        if self.projects.project_id_of(&self.handoff.session) != Some(project.as_str()) {
            self.mode = Mode::List;
            self.notify(format!("\"{}\" is not in \"{name}\" any more, so nothing was started.", self.handoff.session_name));
            return;
        }
        self.mode = Mode::List;
        if self.launch_in(cwd, &[text.as_str()], Some(&project), true) {
            self.notify(format!("Started a new session in the project \"{name}\" with the brief as its first message."));
        }
    }

    /// Keys while the hand-off box is open.
    pub(super) fn handoff_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => return self.cancel_handoff(),
            KeyCode::Char('c') if ctrl => return self.cancel_handoff(),
            KeyCode::Char('s') if ctrl => return self.take_handoff(),
            _ => {}
        }
        if self.handoff.state != HandoffState::Ready {
            return;
        }
        match key.code {
            KeyCode::Enter => self.handoff.insert("\n"),
            KeyCode::Backspace => self.handoff.backspace(),
            KeyCode::Delete => self.handoff.delete(),
            KeyCode::Left => self.handoff.move_cursor(-1),
            KeyCode::Right => self.handoff.move_cursor(1),
            KeyCode::Up => self.handoff.move_line(-1),
            KeyCode::Down => self.handoff.move_line(1),
            KeyCode::Home => self.handoff.to_line_start(),
            KeyCode::End => self.handoff.to_line_end(),
            KeyCode::Char(c) if !ctrl && !alt => self.handoff.insert(&c.to_string()),
            _ => {}
        }
    }

    pub(super) fn handoff_paste(&mut self, text: &str) {
        if matches!(self.mode, Mode::Handoff) && self.handoff.state == HandoffState::Ready {
            self.handoff.insert(text);
        }
    }

    /// True while a hand-off call is out, so the screen is refreshed quickly.
    pub fn handoff_busy(&self) -> bool {
        self.handoff_flight.is_some()
    }
}
