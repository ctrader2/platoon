//! Tasks drafted from a conversation, inside the app. `t` on a session's row (the Sessions tab, a project's Sessions
//! pane), or the chord inside an open session (`draft_tasks` in keys.json, which goes back to the list first), asks
//! Claude once, in the background, to turn the conversation into one or several tasks; meanwhile the row says
//! "Drafting tasks…" and platoon keeps going. The draft opens in the popup **Tasks from this conversation** as soon as
//! the user is on platoon's own lists (in a session, a line says it is ready): `↑` `↓` move, `x` takes a task out, `e`
//! opens one in the user's editor, `enter` makes them all and opens the hand-over popup for each in turn (the
//! suggested squad picked; `esc` there leaves that task as Can start and goes on to the next), `k` makes them and
//! stops, `esc` makes nothing. Each made task remembers the session it came from and goes in its project, or in none.
//!
//! What is sent, what a reply may be and what a task's text looks like are in `taskdraft.rs`; reading the
//! conversation is the hand-off's way (`notesrun::read_session`).

use super::squadtab_glue::{PromptOf, SquadPromptEdit};
use super::*;
use crate::ai::{AiError, Reply, Request};
use crate::instructions;
use crate::taskdraft::{self, DraftTask, Kind, Material};
use crate::tickets::{self, NewTicket, Source, Tickets};
use std::collections::VecDeque;
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
    /// Every task works in these: the session's own folder and the ones added to it.
    pub folders: Vec<String>,
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
    /// The tasks just made that are still to be offered to a squad, in turn: (project, number, suggested squad).
    chain: VecDeque<(String, u32, Option<String>)>,
    /// While the hand-overs go one after the other: the tasks made, the ones that still wait for another, and the first
    /// reason one couldn't be handed over, for the line said at the end.
    chaining: bool,
    made: Vec<u32>,
    made_in: String,
    waiting: Vec<u32>,
    refused: Option<String>,
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

    /// The chord inside an open session: back to the list (the session keeps running), then the same as `t` on its row.
    /// Not in a squad's own session, which belongs to its task.
    pub(super) fn draft_tasks_from_attached(&mut self, pid: u32) {
        if self.sq.split.is_some() {
            self.notify("This is a squad's session: tasks are drafted from your own conversations.");
            return;
        }
        let id = self.sessions.iter().find(|s| s.procs.iter().any(|p| p.pid == pid)).map(|s| s.id.clone());
        self.detach();
        match id {
            Some(id) => {
                if let Some(pos) = self.view.iter().position(|&i| self.sessions[i].id == id) {
                    self.selected = pos;
                }
                self.start_task_draft(&id);
            }
            None => self.notify("platoon doesn't know this session yet, so it can't draft tasks from it. Try again in a moment."),
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
        let (name, cwd, extra) = (s.name.clone(), s.cwd.clone(), s.extra_dirs.clone());
        let project = match self.projects.of(id) {
            Some(p) if p.archived => {
                let why = tickets::archived_refusal(&p.name);
                self.notify(why);
                return;
            }
            Some(p) => p.id.clone(),
            None => tickets::NO_PROJECT_ID.to_string(),
        };
        // Its own folder and the ones added to it, those that are there.
        let mut folders: Vec<String> = Vec::new();
        for f in std::iter::once(cwd).chain(extra.iter().filter_map(|d| dirs::expand(d, &self.home))) {
            if !f.is_empty() && Path::new(&f).is_dir() && !folders.contains(&f) {
                folders.push(f);
            }
        }
        if folders.is_empty() {
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
        let kinds: Vec<Kind> = crate::squadfile::load_all(&self.config)
            .into_iter()
            .filter_map(|l| l.squad)
            .map(|s| Kind { id: s.id.clone(), name: s.name.clone(), can_do: s.can_do.clone() })
            .collect();
        let material = Material {
            name: name.clone(),
            project: if project == tickets::NO_PROJECT_ID { String::new() } else { self.task_project_name(&project) },
            folders: folders.iter().map(|f| crate::squads::folder_name(f)).collect(),
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
            folders,
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
            KeyCode::Char('k') if !ctrl => self.make_drafted(false),
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

    /// `x`: the picked task goes from the draft (the others that waited for it don't any more). The last one gone, the
    /// popup closes with nothing made.
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
        self.pick_squad_editor(SquadPromptEdit { target: PromptOf::DraftTask(d.selected), path, before: text });
    }

    /// The editor is done with a drafted task's file: what it says now goes in the draft (the file goes).
    pub(super) fn after_draft_edit(&mut self, i: usize, edit: &SquadPromptEdit) {
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

    /// `enter` (`hand_over`) or `k` in the popup: every task left is made, each after the ones it waits for, in the
    /// session's project (or in none), remembering the session it came from; then (`enter`) the hand-over popup for
    /// each that can start, in turn.
    pub(super) fn make_drafted(&mut self, hand_over: bool) {
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
                folders: d.folders.clone(),
                blocked_by: t.waits_for.iter().filter_map(|j| made.get(*j).copied()).collect(),
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
        self.show_made(&d.project, first);
        let whose = if d.project == tickets::NO_PROJECT_ID {
            " (in no project)".to_string()
        } else {
            format!(" in {}", self.task_project_name(&d.project))
        };
        if made.len() < d.tasks.len() {
            // The line about what couldn't be made stays.
            return;
        }
        if !hand_over {
            let tasks = if made.len() == 1 { "task" } else { "tasks" };
            self.notify(format!("Made {} {tasks}{whose}: {}. S on each hands it to a squad.", made.len(), numbers(&made)));
            return;
        }
        self.td.chain = made.iter().zip(&d.tasks).map(|(n, t)| (d.project.clone(), *n, t.squad.clone())).collect();
        self.td.chaining = true;
        self.td.made = made;
        self.td.made_in = whose;
        self.td.waiting.clear();
        self.td.refused = None;
        self.next_in_chain();
    }

    /// Where the tasks just made can be seen, with `first` picked: the project's Tasks pane when that project's screen
    /// is showing, else the Tasks tab.
    fn show_made(&mut self, project: &str, first: u32) {
        if self.open_project.as_deref() == Some(project) {
            self.tk.open = None;
            if let Some(s) = self.tk.store.as_mut() {
                s.reload_if_changed();
            }
            self.focus_pane(Pane::Tickets);
            self.tk.selected = Some(TRow::Ticket(first));
            return;
        }
        if self.open_project.is_some() || self.top != TOP_TASKS || self.tk.open.is_some() {
            self.switch_top(TOP_TASKS);
        }
        self.bd.selected = Some(BRow::Task(project.to_string(), first));
        self.set_task_store(project);
    }

    /// Pick task `n` of `project` where the made tasks are shown.
    fn pick_made(&mut self, project: &str, n: u32) {
        if self.open_project.as_deref() == Some(project) {
            self.tk.selected = Some(TRow::Ticket(n));
        } else {
            self.bd.selected = Some(BRow::Task(project.to_string(), n));
        }
        self.set_task_store(project);
    }

    /// The hand-over popup for the next task just made that can start (its suggested squad picked); a task that
    /// waits for another is left for later. When none is left, one line says what was made.
    pub(super) fn next_in_chain(&mut self) {
        while let Some((project, n, squad)) = self.td.chain.pop_front() {
            self.pick_made(&project, n);
            let Some(t) = self.ticket(n).cloned() else { continue };
            if !tickets::waiting_for(&t, self.tickets()).is_empty() {
                self.td.waiting.push(n);
                continue;
            }
            let said = self.notice.as_ref().map(|(s, ..)| s.clone());
            self.ask_hand_to_squad(n);
            if !matches!(self.mode, Mode::SquadPick) {
                let now = self.notice.as_ref().map(|(s, ..)| s.clone());
                if now != said && self.td.refused.is_none() {
                    self.td.refused = now.map(|why| format!("#{n}: {why}"));
                }
                continue;
            }
            if let Some(i) = squad.and_then(|id| self.sq.pick.iter().position(|s| s.id == id)) {
                self.sq.pick_selected = i;
                self.sq.form.cursor = i;
            }
            // Handed over from here, it starts without taking the screen (another popup may be up by then).
            self.sq.quiet.insert((project, n));
            return;
        }
        if !self.td.chaining {
            return;
        }
        self.td.chaining = false;
        let made = std::mem::take(&mut self.td.made);
        let mut line = format!("Made {}{}.", numbers(&made), self.td.made_in);
        let waiting = std::mem::take(&mut self.td.waiting);
        if !waiting.is_empty() {
            let verb = if waiting.len() == 1 { "waits" } else { "wait" };
            line.push_str(&format!(" {} {verb} for another: S hands it over once that is done.", numbers(&waiting)));
        }
        if let Some(why) = self.td.refused.take() {
            line.push_str(&format!(" {why}"));
        }
        self.notify(line);
    }

    /// The hand-over popup closed (handed over, or `esc`): the next task of the ones just made, if they are being
    /// handed over in turn.
    pub(super) fn after_hand_over_popup(&mut self) {
        if self.td.chaining && matches!(self.mode, Mode::List) {
            self.next_in_chain();
        }
    }
}

/// Home and End in the popup: as far as it goes.
const MOST_I32: i32 = 1_000;
