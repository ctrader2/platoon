//! The hand-off inside the app: `h` on a session that is in a project reads what git says about its folder, asks
//! Claude for its part of a hand-off document (summary, work completed, gotchas, next steps), shows the whole
//! document in a box the user can edit, and on accept starts a **new** session in the same folder and the same
//! project with that text as its first message. The old session is left exactly as it is: nothing is archived and
//! nothing is stopped.
//!
//! Git and the call run on a thread of their own, one after the other (git first, so the files it names can go to
//! Claude), while the screen keeps going. What is sent, what an answer may be and what the document reads like is
//! in `handoff.rs`; how the box wraps and scrolls is in `textbox.rs`; this file gathers the material and the facts,
//! runs the work, keeps the box and starts the session.

use super::*;
use crate::ai::{AiError, Reply, Request};
use crate::gitfacts::{self, GitFacts};
use crate::handoff::{self, Brief, Facts};
use crate::textbox;
use std::sync::mpsc::{channel, TryRecvError};

/// How long a hand-off call may take before it is given up on. It is one small call, but it is the user's own
/// key press, so it gets a little longer than a background one.
const HANDOFF_TIMEOUT: Duration = Duration::from_secs(90);

/// What the worker sends back: what git said (None: not a repository, or git couldn't say), and Claude's answer.
pub(super) type HandoffAnswer = (Option<GitFacts>, Result<Reply, AiError>);

/// What the hand-off box is showing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandoffState {
    /// Git is being read and the call is out.
    Asking,
    /// The document is there, and `text` is what will be sent.
    Ready,
    /// Why there is no document, in plain words.
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
    /// The document, as the user may edit it.
    pub text: String,
    /// Where the next character goes, as a character index into `text`.
    pub cursor: usize,
    /// The first row on screen, kept between frames so it only scrolls when it has to.
    pub top: Cell<usize>,
    /// The columns the text may take and the rows that show, as the last frame drew them (for ↑/↓ by row and a
    /// page's worth of PageUp/PageDown).
    pub view: Cell<(usize, usize)>,
    /// The column ↑/↓ aim for, so going through a short row doesn't lose it.
    goal: Option<usize>,
    /// The facts platoon knew when `h` was pressed; git's and the time are added when the answer is in.
    pub facts: Facts,
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
            view: Cell::new((80, 10)),
            goal: None,
            facts: Facts::default(),
        }
    }

    /// Where the cursor is in the lines of the document (line, column), in characters.
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

    /// The text the box shows: the document, or what is happening, or why there is no document.
    pub fn shown(&self) -> String {
        match &self.state {
            HandoffState::Asking => format!("Asking Claude what to hand on from \"{}\"…", self.session_name),
            HandoffState::Failed(why) => why.clone(),
            HandoffState::Ready => self.text.clone(),
        }
    }

    /// The rows the box shows at the width the last frame had.
    fn layout(&self) -> (Vec<char>, Vec<textbox::Row>) {
        let chars: Vec<char> = self.shown().chars().collect();
        let rows = textbox::layout(&chars, self.view.get().0);
        (chars, rows)
    }

    /// The cursor's row and column on screen (rows counted from the top of the text, not of the view).
    #[cfg(test)]
    pub fn cursor_row_col(&self) -> (usize, usize) {
        let (chars, rows) = self.layout();
        textbox::locate(&rows, &chars, self.cursor)
    }

    /// Put typed or pasted text in at the cursor. False (and nothing changes) if it would make the document longer
    /// than may be handed on.
    pub(super) fn insert(&mut self, text: &str) -> bool {
        let text = textbox::clean_paste(text);
        if self.text.chars().count() + text.chars().count() > handoff::TEXT_MAX {
            return false;
        }
        let at = self.byte_at(self.cursor);
        self.text.insert_str(at, &text);
        self.cursor += text.chars().count();
        self.goal = None;
        true
    }

    pub(super) fn backspace(&mut self) {
        self.goal = None;
        if self.cursor == 0 {
            return;
        }
        let at = self.byte_at(self.cursor - 1);
        let to = self.byte_at(self.cursor);
        self.text.replace_range(at..to, "");
        self.cursor -= 1;
    }

    pub(super) fn delete(&mut self) {
        self.goal = None;
        let len = self.text.chars().count();
        if self.cursor >= len {
            return;
        }
        let at = self.byte_at(self.cursor);
        let to = self.byte_at(self.cursor + 1);
        self.text.replace_range(at..to, "");
    }

    pub(super) fn move_cursor(&mut self, delta: i32) {
        self.goal = None;
        let len = self.text.chars().count() as i32;
        self.cursor = (self.cursor as i32 + delta).clamp(0, len) as usize;
    }

    /// Up or down by rows on screen (a long line that wraps is several rows), keeping the column it aims for.
    pub(super) fn move_row(&mut self, delta: i64) {
        let (chars, rows) = self.layout();
        let (row, col) = textbox::locate(&rows, &chars, self.cursor);
        let goal = *self.goal.get_or_insert(col);
        let to = (row as i64 + delta).clamp(0, rows.len().saturating_sub(1) as i64) as usize;
        self.cursor = textbox::index_at(&rows, &chars, to, goal);
    }

    /// PageUp/PageDown: the view moves by a page less one row, and the cursor with it.
    pub(super) fn page(&mut self, down: bool) {
        let step = self.view.get().1.saturating_sub(1).max(1) as i64;
        let delta = if down { step } else { -step };
        self.scroll_view(delta);
        if self.state == HandoffState::Ready {
            self.move_row(delta);
        }
    }

    /// Move the view by `delta` rows. While the document is being edited the cursor is kept in view: it is taken
    /// along to the nearest row that still shows.
    pub(super) fn scroll_view(&mut self, delta: i64) {
        let (chars, rows) = self.layout();
        let visible = self.view.get().1.max(1);
        let most = rows.len().saturating_sub(visible) as i64;
        let top = (self.top.get() as i64 + delta).clamp(0, most) as usize;
        self.top.set(top);
        if self.state != HandoffState::Ready {
            return;
        }
        let (row, col) = textbox::locate(&rows, &chars, self.cursor);
        let goal = *self.goal.get_or_insert(col);
        if row < top {
            self.cursor = textbox::index_at(&rows, &chars, top, goal);
        } else if row >= top + visible {
            self.cursor = textbox::index_at(&rows, &chars, top + visible - 1, goal);
        }
    }

    pub(super) fn to_line_start(&mut self) {
        self.goal = None;
        let (line, _) = self.cursor_at();
        self.cursor = self.index_of(line, 0);
    }

    pub(super) fn to_line_end(&mut self) {
        self.goal = None;
        let (line, _) = self.cursor_at();
        self.cursor = self.index_of(line, usize::MAX);
    }
}

impl App {
    /// `h`: hand this conversation on to a new one. Everything that would make it pointless is said plainly and
    /// changes nothing: no conversation selected, it is in no project, its project is archived, its folder is
    /// gone, there is nothing in it yet, or the gate says no.
    pub(super) fn start_handoff(&mut self) {
        let Some(s) = self.selected_session() else {
            self.notify("No conversation selected");
            return;
        };
        let (session, session_name, cwd) = (s.id.clone(), s.name.clone(), s.cwd.clone());
        // What the list already knows about it, for the facts. A figure it doesn't have is left out.
        let mut facts = Facts {
            session: session_name.clone(),
            folder: Some(s.dir.clone()).filter(|d| !d.is_empty()),
            home: self.home.clone(),
            last_ms: Some(s.last_active).filter(|&ms| ms > 0),
            model: s.model.clone().filter(|m| !m.is_empty()),
            context: (s.context_pct > 0).then_some((s.context_pct, s.context_exact)),
            ..Facts::default()
        };
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
        facts.project = project.name.clone();
        let material = self.handoff_material(&session, &session_name, &cwd, &project.name, &mut facts);
        if material.is_empty() {
            self.notify(format!("There is nothing in \"{session_name}\" yet to hand on."));
            return;
        }
        let mut req = Request::new(handoff::FEATURE, handoff::system(), String::new());
        req.user_initiated = true;
        req.timeout = HANDOFF_TIMEOUT;
        req.cancel = self.switchboard.token(handoff::FEATURE);
        let rx = self.spawn_handoff(settings.clone(), req, material, cwd.clone(), facts.started_ms);
        self.handoff = HandoffBox {
            session,
            session_name,
            project: project.id.clone(),
            project_name: project.name.clone(),
            cwd,
            facts,
            ..HandoffBox::empty()
        };
        self.handoff_flight = Some(rx);
        self.mode = Mode::Handoff;
    }

    /// Read git, then make the call, on a thread of their own: git can block (a huge repository, a network
    /// folder), and it goes first so the files it names can be given to Claude.
    fn spawn_handoff(&self, settings: Settings, mut req: Request, material: handoff::Material, cwd: String, since_ms: Option<u64>) -> Receiver<HandoffAnswer> {
        let (tx, rx) = channel();
        let program = self.ai_program();
        let git_program = self.git_program.clone();
        let runner = self.ai_runner(&program);
        // A unit test that forgot to name a stand-in must never reach the real `claude`: the call is simply not made.
        let never = cfg!(test) && program == "claude";
        thread::spawn(move || {
            let git = if req.cancel.is_cancelled() { None } else { gitfacts::gather(&git_program, Path::new(&cwd), since_ms, gitfacts::BUDGET) };
            req.prompt = handoff::prompt(&material, git.as_ref());
            let answer = if never { Err(AiError::Cancelled) } else { runner.run(&settings, &req) };
            let _ = tx.send((git, answer));
        });
        rx
    }

    /// What one hand-off call is about: what platoon already knows this conversation is, and the last few things
    /// said in it. The project's notes are deliberately not in it - the new session gets those from the hook.
    /// The facts the transcript gives (when it started, how many messages) are put in `facts` on the way.
    ///
    /// The conversation is read from its transcript there and then, not from the prompt index: the document has to
    /// reflect what was said a moment ago, and a session platoon has only just noticed may not be indexed yet.
    fn handoff_material(&self, session: &str, name: &str, cwd: &str, project: &str, facts: &mut Facts) -> handoff::Material {
        let folder = Path::new(cwd).file_name().and_then(|n| n.to_str()).filter(|n| !n.is_empty()).unwrap_or(cwd).to_string();
        let mut m = handoff::Material {
            project: project.to_string(),
            folder,
            name: name.to_string(),
            summary: self.summaries.get(session).map(|e| e.summary.clone()).filter(|s| !s.is_empty()),
            ..handoff::Material::default()
        };
        let Some(path) = crate::claude::find_transcript(self.reader.root(), session) else { return m };
        facts.started_ms = crate::claude::started_ms(&path);
        let read = crate::notesrun::read_session(&path, handoff::MESSAGE_CHARS);
        facts.messages = Some(read.total).filter(|&n| n > 0);
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
        let (git, answer) = match rx.try_recv() {
            Ok(a) => a,
            Err(TryRecvError::Empty) => {
                self.handoff_flight = Some(rx);
                return;
            }
            Err(TryRecvError::Disconnected) => (None, Err(AiError::Failed { kind: crate::ledger::FailKind::Other, detail: "the call ended without an answer".into() })),
        };
        // The box was closed while it ran: nobody is waiting for this.
        if !matches!(self.mode, Mode::Handoff) || self.handoff.state != HandoffState::Asking {
            return;
        }
        self.handoff.top.set(0);
        match answer {
            Ok(reply) => match handoff::parse(&reply.text, !handoff::git_lists_files(git.as_ref())) {
                Some(brief) => self.show_brief(&brief, git),
                None => self.handoff.state = HandoffState::Failed("Claude's answer couldn't be understood, so there is no brief. Nothing has changed.".into()),
            },
            Err(AiError::Cancelled) => self.mode = Mode::List,
            Err(e) => self.handoff.state = HandoffState::Failed(format!("{} Nothing has changed.", e.plain())),
        }
    }

    fn show_brief(&mut self, brief: &Brief, git: Option<GitFacts>) {
        let mut facts = self.handoff.facts.clone();
        facts.git = git;
        facts.written = Some(self.clock());
        self.handoff.text = handoff::document(&facts, brief);
        self.handoff.facts = facts;
        // The cursor starts at the top, where the document starts: the user reads it from there.
        self.handoff.cursor = 0;
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
            self.notify(format!("Started a new session in the project \"{name}\" with the hand-off as its first message."));
        }
    }

    /// Keys while the hand-off box is open. ↑/↓ move by rows on screen, Home/End to the ends of the line as typed,
    /// PageUp/PageDown by a page. While there is no document yet, the arrows and pages scroll what is shown.
    pub(super) fn handoff_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => return self.cancel_handoff(),
            KeyCode::Char('c') if ctrl => return self.cancel_handoff(),
            KeyCode::Char('s') if ctrl => return self.take_handoff(),
            KeyCode::PageUp => return self.handoff.page(false),
            KeyCode::PageDown => return self.handoff.page(true),
            _ => {}
        }
        if self.handoff.state != HandoffState::Ready {
            match key.code {
                KeyCode::Up => self.handoff.scroll_view(-1),
                KeyCode::Down => self.handoff.scroll_view(1),
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Enter => {
                self.handoff_type("\n");
            }
            KeyCode::Backspace => self.handoff.backspace(),
            KeyCode::Delete => self.handoff.delete(),
            KeyCode::Left => self.handoff.move_cursor(-1),
            KeyCode::Right => self.handoff.move_cursor(1),
            KeyCode::Up => self.handoff.move_row(-1),
            KeyCode::Down => self.handoff.move_row(1),
            KeyCode::Home => self.handoff.to_line_start(),
            KeyCode::End => self.handoff.to_line_end(),
            KeyCode::Char(c) if !ctrl && !alt => {
                self.handoff_type(&c.to_string());
            }
            _ => {}
        }
    }

    /// Put text in at the cursor, or say why it can't go in.
    fn handoff_type(&mut self, text: &str) -> bool {
        let ok = self.handoff.insert(text);
        if !ok {
            self.notify(format!("That doesn't fit: the hand-off can be at most {} characters.", handoff::TEXT_MAX));
        }
        ok
    }

    pub(super) fn handoff_paste(&mut self, text: &str) {
        if matches!(self.mode, Mode::Handoff) && self.handoff.state == HandoffState::Ready {
            self.handoff_type(text);
        }
    }

    /// The mouse wheel over the box: three rows at a time.
    pub(super) fn handoff_wheel(&mut self, up: bool) {
        self.handoff.scroll_view(if up { -3 } else { 3 });
    }

    /// True while a hand-off call is out, so the screen is refreshed quickly.
    pub fn handoff_busy(&self) -> bool {
        self.handoff_flight.is_some()
    }

    /// Read git with a stand-in instead of the real `git`.
    #[cfg(test)]
    pub fn set_git_program_for_test(&mut self, program: &str) {
        self.git_program = program.to_string();
    }
}
