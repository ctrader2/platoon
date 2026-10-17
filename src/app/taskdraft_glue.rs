//! Tasks drafted from a conversation, inside the app. `t` on a session's row (the Sessions tab, a project's Sessions
//! pane) asks Claude once, in the background, to turn the conversation into one or several tasks; meanwhile the row
//! says "Drafting tasks…" and platoon keeps going. The draft opens in the popup **Tasks from this conversation** as soon
//! as the user is on platoon's own lists (in a session, a line says it is ready): `↑` `↓` move, `x` takes a task out,
//! `e` opens one in the user's editor, `enter` makes them all (Not started, each with the squad suggested for it),
//! `esc` makes nothing. Each made task works in the session's folder, remembers the session it came from and goes in
//! its project, or in none.
//!
//! What is sent, what a reply may be and what a task's text looks like are in `taskdraft.rs`; reading the
//! conversation is the hand-off's way (`notesrun::read_session`).

use super::*;
use crate::ai::{AiError, Reply, Request};
use crate::instructions;
use crate::taskdraft::{self, DraftTask, Kind, Material};
use crate::tickets::{self, NewTicket, Source, Tickets};
use std::sync::mpsc::{TryRecvError, channel};

/// How long the call may take before it is given up on. The user asked for it, and a draft is longer than a
/// hand-off, so it gets a little more.
const DRAFT_TIMEOUT: Duration = Duration::from_secs(150);

/// What the worker sends back.
pub(super) enum DraftAnswer {
    /// The conversation has nothing in it yet: nothing was asked.
    Nothing,
    Done(Result<Reply, AiError>),
}

/// A draft: the conversation it came from, where its tasks go, and the tasks (empty while the call is out).
pub struct TaskDraft {
    pub session: String,
    pub session_name: String,
    /// The project the tasks go in (`tickets::NO_PROJECT_ID`: none).
    pub project: String,
    /// Every task works in the session's own folder.
    pub folder: String,
    pub kinds: Vec<Kind>,
    pub tasks: Vec<DraftTask>,
    /// The task picked in the popup.
    pub selected: usize,
    /// The first line of the popup's list in view (kept so it scrolls only as far as the picked task needs).
    pub top: Cell<usize>,
    /// Said once, in a session, that it is ready.
    told: bool,
}

/// The call out, and the draft it is for.
struct Flight {
    rx: Receiver<DraftAnswer>,
    draft: TaskDraft,
}

/// Everything about drafting tasks the app keeps.
#[derive(Default)]
pub struct DraftState {
    flight: Option<Flight>,
    /// A draft ready: in the popup, or waiting for the user to be back on platoon's lists.
    pub draft: Option<TaskDraft>,
}

/// A conversation as the drafting gets it, read from its transcript the way the hand-off reads one: the first message
/// and the last few (each cut as the hand-off cuts them), and one line of Claude's last reply.
fn read_conversation(path: &Path, material: &mut Material) {
    let read = crate::notesrun::read_session(path, crate::handoff::MESSAGE_CHARS);
    let mut messages: Vec<String> = read.recent.iter().rev().take(crate::handoff::MESSAGES).rev().cloned().collect();
    if let Some(first) = &read.first
        && !messages.first().is_some_and(|x| x == first)
    {
        messages.insert(0, first.clone());
    }
    material.messages = messages;
    material.reply = read.reply;
}

fn plain(e: &io::Error) -> String {
    let why = e.to_string();
    why.split(" (os error").next().unwrap_or(&why).to_string()
}

/// "#4", "#4 and #5", "#4, #5 and #6".
fn numbers(list: &[u32]) -> String {
    let words: Vec<String> = list.iter().map(|n| format!("#{n}")).collect();
    match words.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

impl App {
    /// The session whose tasks are being drafted right now (the call is out), if any: its row says so.
    pub fn drafting_from(&self) -> Option<&str> {
        self.td.flight.as_ref().map(|f| f.draft.session.as_str())
    }

    /// True while the call is out, so the screen is refreshed quickly.
    pub fn draft_busy(&self) -> bool {
        self.td.flight.is_some()
    }

    /// The draft the popup shows.
    pub fn task_draft(&self) -> Option<&TaskDraft> {
        self.td.draft.as_ref()
    }

    /// `t` on a session's row.
    pub(super) fn draft_tasks_key(&mut self) {
        match self.selected_session().map(|s| s.id.clone()) {
            Some(id) => self.start_task_draft(&id),
            None => self.notify("No session selected"),
        }
    }

    /// Ask Claude for tasks from conversation `id`, in the background. Everything that would make it pointless is
    /// said in one line and nothing is asked: another draft under way, a project that takes no new tasks, no folder to
    /// work in, or the gate says no.
    pub(super) fn start_task_draft(&mut self, id: &str) {
        if let Some(f) = &self.td.flight {
            let name = f.draft.session_name.clone();
            self.notify(format!("Tasks are already being drafted from \"{name}\": one conversation at a time."));
            return;
        }
        if self.td.draft.is_some() {
            // One drafted and not yet made or left: that one first.
            self.mode = Mode::TaskDraft;
            return;
        }
        let Some(s) = self.sessions.iter().find(|s| s.id == id) else {
            self.notify("That session isn't in platoon's list any more.");
            return;
        };
        let (name, cwd) = (s.name.clone(), s.cwd.clone());
        let project = match self.projects.of(id) {
            Some(p) if p.archived => {
                let why = tickets::archived_refusal(&p.name);
                self.notify(why);
                return;
            }
            Some(p) => p.id.clone(),
            None => tickets::NO_PROJECT_ID.to_string(),
        };
        if cwd.is_empty() || !Path::new(&cwd).is_dir() {
            self.notify(format!("\"{name}\" has no folder platoon can find, so no tasks can be drafted from it."));
            return;
        }
        let settings = self.settings.get();
        // The user asked for this themselves: the plan-usage pause doesn't apply, every other gate does.
        if let Err(r) = ai::gate_for(&settings, taskdraft::FEATURE, &self.cache, self.clock(), true) {
            let mut text = r.plain();
            if r.is_switch() {
                text.push_str(" Turn it on in the settings (press s, then the Smart features tab).");
            }
            self.notify(format!("No tasks drafted: {text}"));
            return;
        }
        let kinds: Vec<Kind> = crate::squads::all(&self.config)
            .into_iter()
            .filter(|s| s.usable())
            .map(|s| Kind { id: s.id, name: s.name, description: s.description })
            .collect();
        let material = Material {
            name: name.clone(),
            project: if project == tickets::NO_PROJECT_ID { String::new() } else { self.task_project_name(&project) },
            folders: vec![Path::new(&cwd).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()],
            kinds: kinds.clone(),
            ..Material::default()
        };
        let mut req = Request::new(
            taskdraft::FEATURE,
            taskdraft::system_with(&instructions::guidance(&self.config, instructions::TASK_DRAFT)),
            String::new(),
        );
        req.user_initiated = true;
        req.timeout = DRAFT_TIMEOUT;
        req.cancel = self.switchboard.token(taskdraft::FEATURE);
        let rx = self.spawn_task_draft(settings, req, material, (self.reader.root().to_path_buf(), id.to_string()));
        let draft = TaskDraft {
            session: id.to_string(),
            session_name: name.clone(),
            project,
            folder: cwd,
            kinds,
            tasks: Vec::new(),
            selected: 0,
            top: Cell::new(0),
            told: false,
        };
        self.td.flight = Some(Flight { rx, draft });
        self.notify(format!("Drafting tasks from \"{name}\"…"));
    }

    /// Read the conversation, then make the call, on a thread of their own (a long transcript takes a moment).
    fn spawn_task_draft(
        &self,
        settings: Settings,
        mut req: Request,
        mut material: Material,
        transcript: (PathBuf, String),
    ) -> Receiver<DraftAnswer> {
        let (tx, rx) = channel();
        let program = self.ai_program();
        let runner = self.ai_runner(&program);
        // A unit test that forgot to name a stand-in must never reach the real `claude`: the call is simply not made.
        let never = cfg!(test) && program == "claude";
        thread::spawn(move || {
            if let Some(path) = crate::claude::find_transcript(&transcript.0, &transcript.1) {
                read_conversation(&path, &mut material);
            }
            if material.is_empty() {
                let _ = tx.send(DraftAnswer::Nothing);
                return;
            }
            req.prompt = taskdraft::prompt(&material);
            let answer = if never || req.cancel.is_cancelled() { Err(AiError::Cancelled) } else { runner.run(&settings, &req) };
            let _ = tx.send(DraftAnswer::Done(answer));
        });
        rx
    }

    /// Take the answer in when it arrives, and show a draft that is ready when the user is on platoon's lists. Called
    /// on every refresh; cheap when nothing is out.
    pub(super) fn poll_task_draft(&mut self) {
        if let Some(f) = self.td.flight.as_ref() {
            let got = match f.rx.try_recv() {
                Ok(a) => Some(a),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(DraftAnswer::Done(Err(AiError::Failed {
                    kind: crate::ledger::FailKind::Other,
                    detail: "the call ended without an answer".into(),
                }))),
            };
            if let Some(answer) = got
                && let Some(Flight { mut draft, .. }) = self.td.flight.take()
            {
                match answer {
                    DraftAnswer::Nothing => {
                        self.notify(format!("There is nothing in \"{}\" yet to draft tasks from.", draft.session_name));
                    }
                    DraftAnswer::Done(Ok(reply)) => match taskdraft::parse(&reply.text, &draft.kinds) {
                        Ok(tasks) => {
                            draft.tasks = tasks;
                            self.td.draft = Some(draft);
                        }
                        Err(why) => self.notify(why.words()),
                    },
                    DraftAnswer::Done(Err(AiError::Cancelled)) => {}
                    DraftAnswer::Done(Err(e)) => self.notify(format!("No tasks drafted: {} Nothing was written.", e.plain())),
                }
            }
        }
        self.show_task_draft();
    }

    /// A draft that is ready opens as soon as the user is on platoon's own lists with nothing over them; in a session,
    /// one line says so (once).
    fn show_task_draft(&mut self) {
        if self.td.draft.is_none() || matches!(self.mode, Mode::TaskDraft) {
            return;
        }
        if matches!(self.mode, Mode::List) && !self.help_open {
            self.mode = Mode::TaskDraft;
            return;
        }
        let back = self.back_label().to_string();
        if let Some(d) = self.td.draft.as_mut().filter(|d| !d.told && matches!(self.mode, Mode::Attached(_))) {
            d.told = true;
            let name = d.session_name.clone();
            self.notify(format!("The tasks from \"{name}\" are drafted: {back} shows them."));
        }
    }

    // ---- the popup -----------------------------------------------------------------------------------------------

    pub(super) fn task_draft_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.act(Action::Cancel),
            KeyCode::Char('c') if ctrl => self.act(Action::Cancel),
            KeyCode::Enter => self.act(Action::Confirm),
            KeyCode::Char('x') | KeyCode::Delete => self.remove_drafted(),
            KeyCode::Char('e') => self.edit_drafted(),
            KeyCode::Up => self.move_draft(-1),
            KeyCode::Down | KeyCode::Tab => self.move_draft(1),
            KeyCode::BackTab => self.move_draft(-1),
            KeyCode::Home | KeyCode::Char('g') => self.move_draft(-(MOST_I32)),
            KeyCode::End | KeyCode::Char('G') => self.move_draft(MOST_I32),
            KeyCode::PageUp => self.scroll_draft(-(self.pages.tickets.get().max(1) as i64 * 2)),
            KeyCode::PageDown => self.scroll_draft(self.pages.tickets.get().max(1) as i64 * 2),
            _ => {}
        }
    }

    pub(super) fn move_draft(&mut self, delta: i32) {
        if let Some(d) = self.td.draft.as_mut() {
            let last = d.tasks.len().saturating_sub(1) as i32;
            d.selected = (d.selected as i32 + delta).clamp(0, last) as usize;
        }
    }

    /// PageUp/PageDown or the wheel: the list moves by lines (the picked task stays as it is).
    pub(super) fn scroll_draft(&mut self, delta: i64) {
        if let Some(d) = self.td.draft.as_ref() {
            d.top.set((d.top.get() as i64 + delta).max(0) as usize);
        }
    }

    /// A click on a task in the popup picks it.
    pub(super) fn click_draft(&mut self, i: usize) {
        if let Some(d) = self.td.draft.as_mut() {
            d.selected = i.min(d.tasks.len().saturating_sub(1));
        }
    }

    /// `x`: the picked task goes from the draft. The last one gone, the popup closes with nothing made.
    fn remove_drafted(&mut self) {
        let Some(d) = self.td.draft.as_mut() else { return };
        let i = d.selected;
        let title = d.tasks.get(i).map(|t| t.title.clone()).unwrap_or_default();
        taskdraft::remove(&mut d.tasks, i);
        d.selected = i.min(d.tasks.len().saturating_sub(1));
        if d.tasks.is_empty() {
            self.td.draft = None;
            self.mode = Mode::List;
            self.notify("Every task was taken out of the draft, so nothing was made.");
            return;
        }
        self.notify(format!("Took \"{title}\" out of the draft."));
    }

    /// `e`: the picked task in the user's editor (the notes' list of editors), as a file of its own: its title as a
    /// heading, then its text. What the file says when the editor is done replaces it in the draft.
    fn edit_drafted(&mut self) {
        let Some(d) = self.td.draft.as_ref() else { return };
        let Some(t) = d.tasks.get(d.selected) else { return };
        let dir = self.cache.join("task-drafts");
        let path = dir.join(format!("task-{}-{}.md", d.selected + 1, tickets::slug(&t.title)));
        let text = taskdraft::to_file(t);
        if let Err(e) = std::fs::create_dir_all(&dir).and_then(|()| crate::projects::write_atomic(&path, &text)) {
            self.notify(format!("Couldn't open the task for editing: {}", plain(&e)));
            return;
        }
        self.pick_file_editor(FileEdit { of: FileOf::Draft(d.selected), path, before: text });
    }

    /// The editor is done with a drafted task's file: what it says now goes in the draft (the file goes).
    pub(super) fn after_draft_edit(&mut self, i: usize, edit: &FileEdit) {
        let text = std::fs::read_to_string(&edit.path).unwrap_or_default();
        let _ = std::fs::remove_file(&edit.path);
        if matches!(self.mode, Mode::List) && self.td.draft.is_some() {
            self.mode = Mode::TaskDraft;
        }
        let Some(t) = self.td.draft.as_mut().and_then(|d| d.tasks.get_mut(i)) else { return };
        if text == edit.before {
            return self.notify("The task is unchanged.");
        }
        match taskdraft::from_file(&text, t) {
            Some(new) => {
                *t = new;
                let title = t.title.clone();
                self.notify(format!("Changed \"{title}\" in the draft."));
            }
            None => self.notify("The file was empty, so the task is as it was. x takes it out of the draft."),
        }
    }

    /// `esc` in the popup: nothing is made.
    pub(super) fn drop_draft(&mut self) {
        self.mode = Mode::List;
        if self.td.draft.take().is_some() {
            self.notify("No tasks were made.");
        }
    }

    /// `enter` in the popup: every task left is made, Not started, in the session's project (or in none), working in
    /// the session's folder, with the squad suggested for it, remembering the session it came from.
    pub(super) fn make_drafted(&mut self) {
        let Some(d) = self.td.draft.take() else {
            self.mode = Mode::List;
            return;
        };
        self.mode = Mode::List;
        if d.project != tickets::NO_PROJECT_ID {
            match self.projects.get(&d.project) {
                Some(p) if p.archived => {
                    let why = tickets::archived_refusal(&p.name);
                    self.notify(why);
                    return;
                }
                Some(_) => {}
                None => {
                    self.notify("The session's project isn't there any more, so no tasks were made.");
                    return;
                }
            }
        }
        let mut store = Tickets::open(&self.config, &d.project);
        let mut made: Vec<u32> = Vec::new();
        for t in &d.tasks {
            let new = NewTicket {
                title: t.title.clone(),
                body: Some(t.body.clone()),
                folder: Some(d.folder.clone()),
                squad: t.squad.clone().unwrap_or_default(),
                from: Some(Source { session: d.session.clone(), project: d.project.clone() }),
            };
            match store.add(new) {
                Ok(n) => made.push(n),
                Err(e) => {
                    let so_far = if made.is_empty() { "No tasks were made".to_string() } else { format!("Made {}", numbers(&made)) };
                    self.notify(format!("{so_far}; \"{}\" couldn't be made: {}", t.title, plain(&e)));
                    break;
                }
            }
        }
        let Some(&first) = made.first() else { return };
        self.reread_tickets();
        if self.open_project.is_some() || self.top != TOP_TASKS || self.tk.open.is_some() {
            self.switch_top(TOP_TASKS);
        }
        self.bd.selected = Some(BRow(d.project.clone(), first));
        self.set_task_store(&d.project);
        if made.len() < d.tasks.len() {
            // The line about what couldn't be made stays.
            return;
        }
        let whose = if d.project == tickets::NO_PROJECT_ID {
            " (in no project)".to_string()
        } else {
            format!(" in {}", self.task_project_name(&d.project))
        };
        let tasks = if made.len() == 1 { "task" } else { "tasks" };
        self.notify(format!("Made {} {tasks}{whose}: {}. s on each starts it.", made.len(), numbers(&made)));
    }
}

/// Home and End in the popup: as far as it goes.
const MOST_I32: i32 = 1_000;
