//! Editing a project's notes from the project screen: the editor list (`e`), running the editor (in platoon's own
//! window for a terminal one, beside it for one with a window of its own), the before/after view of the last
//! change and undoing it. The file itself, its history and the diff are in `notes.rs`; finding editors and running
//! them is in `editors.rs`.
//!
//! It also works out what the notes pane and the header say about the state of things: whether the notes are
//! reaching the project's sessions, and why they are not being kept up to date (`poll_notes_status`).

use super::*;
use crate::editors::{self, Editor};
use crate::instructions;
use crate::notes;
use crate::notesrun;
use crate::notesupdate;
use crate::tickets::Tickets;

/// A terminal editor running inside platoon's window on a project's notes, or on the instructions of an AI action.
pub struct NotesEdit {
    pub project: String,
    /// Editing these instructions (`instructions::ACTIONS`) rather than a project's notes: `project` is empty.
    pub instructions: Option<&'static str>,
    /// Editing the body of this ticket of `project` rather than its notes.
    pub ticket: Option<u32>,
    /// Editing a file of its own (a task of a draft, a squad): `project` is empty.
    pub file: Option<FileEdit>,
    /// The instructions (or the ticket's body) as they were before the editor ran.
    before: String,
    /// The editor's plain name, for the status line.
    pub name: String,
    /// What the file looked like when the editor started, so platoon can tell whether anything came of it.
    stamp: notes::Stamp,
    pub run: editors::Running,
    /// The back key was pressed: the "Leave the editor?" question shows over it, and only `y` ends the editor.
    pub leaving: bool,
}

/// An editor with a window of its own that platoon opened on a project's notes. platoon carries on as usual and
/// reads the notes again when the editor is done with them, or as soon as the file changes.
pub struct NotesWindow {
    pub project: String,
    pub instructions: Option<&'static str>,
    pub ticket: Option<u32>,
    pub file: Option<FileEdit>,
    before: String,
    pub name: String,
    stamp: notes::Stamp,
    child: std::process::Child,
}

/// A file of its own open in an editor: what it is, the file, and what the file said before.
#[derive(Clone, Debug)]
pub struct FileEdit {
    pub of: FileOf,
    pub path: PathBuf,
    pub before: String,
}

/// What a `FileEdit` is the file of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileOf {
    /// A task of a draft (tasks drafted from a conversation, `app/taskdraft_glue.rs`): its place in the draft.
    Draft(usize),
    /// A squad (the Squads tab, `app/squadtab_glue.rs`); `copied`: platoon wrote the built-in's text there for the edit.
    Squad { id: String, copied: bool },
}

/// The list of editors, which `e` opens. The one used last is first and picked, so enter is one keystroke.
pub struct EditorPick {
    pub project: String,
    /// Opened from the settings, for these instructions: `project` is empty.
    pub instructions: Option<&'static str>,
    /// Opened for the body of this ticket of `project`.
    pub ticket: Option<u32>,
    /// Opened for a file of its own (a task of a draft, a squad).
    pub file: Option<FileEdit>,
    pub options: Vec<Editor>,
    pub selected: usize,
    /// The id of the editor used last: the one with the star in front of it (empty when none has been used).
    pub starred: String,
}

/// The before/after view of the last change to the notes.
pub struct Changes {
    pub lines: Vec<(notes::Mark, String)>,
    pub top: Cell<usize>,
}

/// When a file was last changed and how big it is.
fn file_stamp(path: &Path) -> notes::Stamp {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// "The task" from "the task".
fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn plain(e: &io::Error) -> String {
    let why = e.to_string();
    why.split(" (os error").next().unwrap_or(&why).to_string()
}

impl App {
    #[cfg(test)]
    pub fn set_editors_for_test(&mut self, list: Vec<Editor>) {
        self.editors = list;
    }

    /// Whether the instructions for `id` are the built-in ones, the user's, or the user's with a newer built-in text.
    pub fn instructions_status(&self, id: &str) -> instructions::Status {
        instructions::status(&self.config, id)
    }

    /// An editor platoon started is open on some project's notes, so platoon must not write them itself.
    pub fn notes_busy(&self) -> bool {
        self.editing.is_some() || self.notes_window.is_some()
    }

    /// The name of the editor platoon has open on the notes of the project whose screen is showing, if any.
    pub fn notes_editor_open(&self) -> Option<&str> {
        let project = self.open_project.as_deref()?;
        if let Some(e) = self.editing.as_ref().filter(|e| e.project == project && e.ticket.is_none()) {
            return Some(&e.name);
        }
        self.notes_window.as_ref().filter(|w| w.project == project && w.ticket.is_none()).map(|w| w.name.as_str())
    }

    /// An automatic update is running on the open project's notes right now.
    pub fn notes_updating(&self) -> bool {
        self.open_project.as_deref().is_some_and(|p| notesrun::updating(&self.cache, p))
    }

    /// When platoon itself last changed the open project's notes, in seconds since the epoch.
    pub fn notes_updated_at(&self) -> Option<u64> {
        let p = self.open_project.as_deref()?;
        let at = notesupdate::State::load(&self.config, p).last_update_at;
        (at > 0).then_some(at)
    }

    /// Forget what the notes looked like, so a change platoon has just made itself isn't announced twice.
    fn forget_notes_stamp(&mut self) {
        self.notes_seen = None;
    }

    /// Notice the notes changing under the open project's screen - an automatic update finishing, another window,
    /// or an editor of the user's own - and say so once. Cheap: one `stat` a frame.
    fn poll_notes_changes(&mut self) {
        let Some(project) = self.open_project.clone() else {
            self.notes_seen = None;
            return;
        };
        let stamp = notes::stamp(&self.config, &project);
        if let Some((seen, was)) = self.notes_seen.take()
            && seen == project
            && was != stamp
            && !self.notes_busy()
        {
            self.notify("The notes changed. Press c to see what changed, u to undo it.");
        }
        self.notes_seen = Some((project, stamp));
    }

    /// Why these notes are not being kept up to date right now (the master switch, this feature's switch, the daily
    /// limit, the plan pause, a back-off), with where to change it. None while the updater can run.
    pub fn notes_stopped(&self) -> Option<&str> {
        self.notes_stopped.as_deref()
    }

    /// The dim line in the header while nothing that spends tokens can run at all: the long form when the header
    /// has `room` for it, else the short one, else nothing (the tabs matter more).
    pub fn ai_header_note(&self, room: usize) -> Option<&str> {
        let (short, full) = self.ai_note.as_ref()?;
        if full.chars().count() <= room {
            Some(full)
        } else if short.chars().count() <= room {
            Some(short)
        } else {
            None
        }
    }

    /// Work out, with the rest of the state, what the notes pane says about the open project: why its notes are
    /// not updating. A handful of small reads, and nothing at all while no project's screen is open.
    pub(super) fn poll_notes_status(&mut self) {
        let settings = self.settings.get();
        let now = self.clock();
        self.ai_note = ai::header_note(&settings, &self.cache, now);
        if self.open_project.is_none() {
            self.notes_stopped = None;
            return;
        }
        self.notes_stopped = ai::stopped_reason(&settings, notesrun::FEATURE, &self.cache, now);
    }

    // ---- starting an editor -----------------------------------------------------------------------------------

    /// `e`: the list of editors. The one used last is starred, first and already picked, so enter opens it again;
    /// anything else is one arrow away.
    pub(super) fn edit_notes(&mut self) {
        let Some(project) = self.open_project.clone() else { return };
        if let Some(name) = self.notes_editor_open().map(str::to_string) {
            self.notify(format!("The notes are already open in {name}."));
            return;
        }
        if let Some(name) = self
            .editing
            .as_ref()
            .filter(|e| e.ticket.is_some())
            .map(|e| e.name.clone())
            .or_else(|| self.notes_window.as_ref().filter(|w| w.ticket.is_some()).map(|w| w.name.clone()))
        {
            self.notify(format!("{name} is still open on a task. Close it first."));
            return;
        }
        if self.notes_busy() {
            self.notify("platoon is already editing another project's notes.");
            return;
        }
        let last = self.settings.get().notes_editor;
        let options = editors::in_order(&self.editors, &last);
        let starred = options.first().filter(|e| e.id == last).map(|e| e.id.clone()).unwrap_or_default();
        self.epick = EditorPick { project, instructions: None, ticket: None, file: None, options, selected: 0, starred };
        self.mode = Mode::PickEditor;
    }

    /// Enter on an instructions entry of the settings: the same list of editors as the notes.
    pub(super) fn edit_instructions(&mut self, id: &'static str) {
        if let Some(name) = self.editing.as_ref().map(|e| e.name.clone()).or_else(|| self.notes_window.as_ref().map(|w| w.name.clone())) {
            self.notify(format!("{name} is still open on something else. Close it first."));
            return;
        }
        let last = self.settings.get().notes_editor;
        let options = editors::in_order(&self.editors, &last);
        let starred = options.first().filter(|e| e.id == last).map(|e| e.id.clone()).unwrap_or_default();
        self.epick = EditorPick { project: String::new(), instructions: Some(id), ticket: None, file: None, options, selected: 0, starred };
        self.mode = Mode::PickEditor;
    }

    /// The same list of editors, for the body of task `number` of the tasks showing (the open project's, or the board's
    /// picked task's).
    pub(super) fn pick_ticket_editor(&mut self, number: u32) {
        let Some(project) = self.task_scope() else { return };
        if let Some(name) = self.editing.as_ref().map(|e| e.name.clone()).or_else(|| self.notes_window.as_ref().map(|w| w.name.clone())) {
            self.notify(format!("{name} is still open on something else. Close it first."));
            return;
        }
        let last = self.settings.get().notes_editor;
        let options = editors::in_order(&self.editors, &last);
        let starred = options.first().filter(|e| e.id == last).map(|e| e.id.clone()).unwrap_or_default();
        self.epick = EditorPick { project, instructions: None, ticket: Some(number), file: None, options, selected: 0, starred };
        self.mode = Mode::PickEditor;
    }

    /// The same list of editors, for a file of its own (already written: a task of a draft, a squad).
    pub(super) fn pick_file_editor(&mut self, edit: FileEdit) {
        if let Some(name) = self.editing.as_ref().map(|e| e.name.clone()).or_else(|| self.notes_window.as_ref().map(|w| w.name.clone())) {
            self.drop_file_edit(edit);
            self.notify(format!("{name} is still open on something else. Close it first."));
            return;
        }
        let last = self.settings.get().notes_editor;
        let options = editors::in_order(&self.editors, &last);
        let starred = options.first().filter(|e| e.id == last).map(|e| e.id.clone()).unwrap_or_default();
        self.epick =
            EditorPick { project: String::new(), instructions: None, ticket: None, file: Some(edit), options, selected: 0, starred };
        self.mode = Mode::PickEditor;
    }

    /// A file of its own that no editor opened after all: a task of a draft's file goes, and a built-in squad's copy
    /// left as it was goes.
    pub(super) fn drop_file_edit(&mut self, edit: FileEdit) {
        match &edit.of {
            FileOf::Draft(_) => {
                let _ = std::fs::remove_file(&edit.path);
            }
            FileOf::Squad { id, copied } => {
                crate::squads::after_edit(&self.config, id, &edit.before, *copied);
            }
        }
    }

    /// An editor has been and gone from a file of its own.
    fn after_file_edit(&mut self, edit: &FileEdit) {
        match &edit.of {
            FileOf::Draft(i) => self.after_draft_edit(*i, edit),
            FileOf::Squad { .. } => self.after_squad_edit(edit),
        }
    }

    /// Where the list of editors goes back to for a file of its own: the draft's popup, or the list.
    pub(super) fn file_edit_mode(edit: &FileEdit) -> Mode {
        if matches!(edit.of, FileOf::Draft(_)) { Mode::TaskDraft } else { Mode::List }
    }

    /// Open a file of its own in `e`.
    fn run_file_editor(&mut self, edit: FileEdit, e: &Editor) {
        let stamp = file_stamp(&edit.path);
        let what = if matches!(edit.of, FileOf::Draft(_)) { "the task" } else { "the squad" };
        if e.terminal {
            let (rows, cols) = self.pty_size();
            match editors::Running::spawn(e, &edit.path, rows, cols) {
                Ok(run) => {
                    self.editing = Some(NotesEdit {
                        project: String::new(),
                        instructions: None,
                        ticket: None,
                        file: Some(edit),
                        before: String::new(),
                        name: e.name.clone(),
                        stamp,
                        run,
                        leaving: false,
                    });
                    self.mode = Mode::EditNotes;
                    self.clear_selection();
                }
                Err(err) => {
                    self.drop_file_edit(edit);
                    self.notify(format!("Couldn't open {what} in {}: {}", e.name, plain(&err)));
                }
            }
            return;
        }
        match editors::launch_window(e, &edit.path) {
            Ok(child) => {
                self.notify(format!("{} opened in {}, in a window of its own.", capital(what), e.name));
                self.notes_window = Some(NotesWindow {
                    project: String::new(),
                    instructions: None,
                    ticket: None,
                    file: Some(edit),
                    before: String::new(),
                    name: e.name.clone(),
                    stamp,
                    child,
                });
            }
            Err(err) => {
                self.drop_file_edit(edit);
                self.notify(format!("Couldn't open {what} in {}: {}", e.name, plain(&err)));
            }
        }
    }

    /// Where esc, or the end of an edit, goes back to: the settings for instructions, else the list.
    pub(super) fn after_pick_mode(instructions: Option<&'static str>) -> Mode {
        if instructions.is_some() { Mode::Settings } else { Mode::List }
    }

    pub(super) fn move_editor_pick(&mut self, delta: i32) {
        let last = self.epick.options.len().saturating_sub(1) as i32;
        self.epick.selected = (self.epick.selected as i32 + delta).clamp(0, last) as usize;
    }

    /// Enter in the editor list: use that editor, and remember it as the one to offer first next time.
    pub(super) fn take_editor_pick(&mut self) {
        // A task of a draft goes back to the draft's popup, which stays open while a windowed editor has it.
        let back = match &self.epick.file {
            Some(f) => Self::file_edit_mode(f),
            None => Self::after_pick_mode(self.epick.instructions),
        };
        let Some(e) = self.epick.options.get(self.epick.selected).cloned() else {
            self.mode = back;
            return;
        };
        let project = self.epick.project.clone();
        self.mode = back;
        self.remember_editor(&e.id);
        if let Some(edit) = self.epick.file.take() {
            return self.run_file_editor(edit, &e);
        }
        match (self.epick.instructions, self.epick.ticket) {
            (Some(id), _) => self.run_instructions_editor(id, &e),
            (None, Some(n)) => self.run_ticket_editor(&project, n, &e),
            (None, None) => self.run_editor(&project, &e),
        }
    }

    /// Keep this editor as the last one used (`notes_editor` in the settings file). A save that fails is said
    /// plainly and changes nothing else: the editor still opens.
    fn remember_editor(&mut self, id: &str) {
        if self.settings.get().notes_editor == id {
            return;
        }
        let mut next = self.settings.get();
        next.notes_editor = id.to_string();
        let saved = self.settings.update(&next);
        if saved.is_err() {
            self.report_save(saved);
        }
    }

    /// Open `project`'s notes in `e`. A project with no notes yet gets the template first, and whatever is there
    /// is kept as an earlier version before the editor can change it.
    fn run_editor(&mut self, project: &str, e: &Editor) {
        let path = self.projects.notes_path(project);
        // Always from the file as it is on disk now: another window, or an editor of the user's own, may have
        // written it since platoon last looked.
        if !path.exists() {
            if let Err(err) = notes::write(&self.config, project, notes::TEMPLATE) {
                self.notify(format!("Couldn't start the notes file: {}", plain(&err)));
                return;
            }
        } else if let Err(err) = notes::keep_version(&self.config, project) {
            self.notify(format!("Couldn't keep a copy of the notes, so they weren't opened: {}", plain(&err)));
            return;
        }
        let stamp = notes::stamp(&self.config, project);
        // While platoon has an editor open on them, an automatic update must not write: the helper is another
        // process, so it is told through a small file (its own pid inside, so a platoon that was killed says nothing).
        notesrun::set_marker(&notesrun::editing_path(&self.cache, project));
        self.forget_notes_stamp();
        if e.terminal {
            let (rows, cols) = self.pty_size();
            match editors::Running::spawn(e, &path, rows, cols) {
                Ok(run) => {
                    self.editing = Some(NotesEdit {
                        project: project.to_string(),
                        instructions: None,
                        ticket: None,
                        file: None,
                        before: String::new(),
                        name: e.name.clone(),
                        stamp,
                        run,
                        leaving: false,
                    });
                    self.mode = Mode::EditNotes;
                    self.clear_selection();
                }
                Err(err) => {
                    notesrun::clear_marker(&notesrun::editing_path(&self.cache, project));
                    self.notify(format!("Couldn't open the notes in {}: {}", e.name, plain(&err)));
                }
            }
            return;
        }
        match editors::launch_window(e, &path) {
            Ok(child) => {
                self.notes_window = Some(NotesWindow {
                    project: project.to_string(),
                    instructions: None,
                    ticket: None,
                    file: None,
                    before: String::new(),
                    name: e.name.clone(),
                    stamp,
                    child,
                });
                self.notify(format!("The notes opened in {}, in a window of its own.", e.name));
            }
            Err(err) => {
                notesrun::clear_marker(&notesrun::editing_path(&self.cache, project));
                self.notify(format!("Couldn't open the notes in {}: {}", e.name, plain(&err)));
            }
        }
    }

    /// Open the user's own instructions for `id` in `e`, starting the file from the built-in text if there is none.
    fn run_instructions_editor(&mut self, id: &'static str, e: &Editor) {
        let (path, before) = match instructions::prepare_edit(&self.config, id) {
            Ok(v) => v,
            Err(err) => {
                self.notify(format!("Couldn't start the system prompt file: {}", plain(&err)));
                return;
            }
        };
        let stamp = instructions::stamp(&self.config, id);
        let what = instructions::action(id).what;
        if e.terminal {
            let (rows, cols) = self.pty_size();
            match editors::Running::spawn(e, &path, rows, cols) {
                Ok(run) => {
                    self.editing = Some(NotesEdit {
                        project: String::new(),
                        instructions: Some(id),
                        ticket: None,
                        file: None,
                        before,
                        name: e.name.clone(),
                        stamp,
                        run,
                        leaving: false,
                    });
                    self.mode = Mode::EditNotes;
                    self.clear_selection();
                }
                Err(err) => {
                    instructions::after_edit(&self.config, id, &before);
                    self.notify(format!("Couldn't open the system prompt in {}: {}", e.name, plain(&err)));
                }
            }
            return;
        }
        match editors::launch_window(e, &path) {
            Ok(child) => {
                self.notes_window = Some(NotesWindow {
                    project: String::new(),
                    instructions: Some(id),
                    ticket: None,
                    file: None,
                    before,
                    name: e.name.clone(),
                    stamp,
                    child,
                });
                self.notify(format!("The system prompt for {what} opened in {}, in a window of its own.", e.name));
            }
            Err(err) => {
                instructions::after_edit(&self.config, id, &before);
                self.notify(format!("Couldn't open the system prompt in {}: {}", e.name, plain(&err)));
            }
        }
    }

    /// Open the body of ticket `number` in `e`. A body file that has gone is started again from the template.
    fn run_ticket_editor(&mut self, project: &str, number: u32, e: &Editor) {
        let store = Tickets::open(&self.config, project);
        let Some((path, before)) = store.get(number).map(|t| (store.body_path(t), store.body(t))) else {
            return;
        };
        if store.get(number).is_some_and(|t| t.file.is_empty()) {
            self.notify(format!("#{number} has no body file, so there is nothing to open."));
            return;
        }
        if !path.exists()
            && let Err(err) = crate::projects::write_atomic(&path, crate::tickets::TEMPLATE)
        {
            self.notify(format!("Couldn't start the task's text: {}", plain(&err)));
            return;
        }
        let before = if path.exists() && before.is_empty() { std::fs::read_to_string(&path).unwrap_or_default() } else { before };
        let stamp = file_stamp(&path);
        if e.terminal {
            let (rows, cols) = self.pty_size();
            match editors::Running::spawn(e, &path, rows, cols) {
                Ok(run) => {
                    self.editing = Some(NotesEdit {
                        project: project.to_string(),
                        instructions: None,
                        ticket: Some(number),
                        file: None,
                        before,
                        name: e.name.clone(),
                        stamp,
                        run,
                        leaving: false,
                    });
                    self.mode = Mode::EditNotes;
                    self.clear_selection();
                }
                Err(err) => self.notify(format!("Couldn't open #{number} in {}: {}", e.name, plain(&err))),
            }
            return;
        }
        match editors::launch_window(e, &path) {
            Ok(child) => {
                self.notes_window = Some(NotesWindow {
                    project: project.to_string(),
                    instructions: None,
                    ticket: Some(number),
                    file: None,
                    before,
                    name: e.name.clone(),
                    stamp,
                    child,
                });
                self.notify(format!("#{number} opened in {}, in a window of its own.", e.name));
            }
            Err(err) => self.notify(format!("Couldn't open #{number} in {}: {}", e.name, plain(&err))),
        }
    }

    /// What to say once an editor has been and gone from a ticket's body: by its words, not the file's time.
    fn after_ticket_edit(&mut self, project: &str, number: u32, before: &str, name: &str) {
        let mut fresh = Tickets::open(&self.config, project);
        let Some(now) = fresh.get(number).map(|t| fresh.body(t)) else { return };
        if now == before {
            self.notify(format!("#{number} is unchanged ({name} left it as it was)."));
            return;
        }
        match self.tk.store.as_mut().filter(|s| s.project() == project) {
            Some(store) => {
                let _ = store.touch(number);
            }
            None => {
                let _ = fresh.touch(number);
            }
        }
        self.tk.body_top.set(0);
        self.notify(format!("Saved the body of #{number}."));
    }

    /// What to say once an editor has been and gone from the instructions (see `instructions::after_edit`).
    fn after_instructions_edit(&mut self, id: &'static str, before: &str) {
        let what = instructions::action(id).what;
        let said = match instructions::after_edit(&self.config, id, before) {
            // Nothing changed: nothing to say (the entry still reads "Enter to change" or "User Modified").
            instructions::Edited::Unchanged => return,
            instructions::Edited::BuiltIn => format!("The system prompt for {what} is platoon's default again."),
            instructions::Edited::Changed => {
                format!("Saved: platoon uses your system prompt for {what} from the next time it asks Claude.")
            }
        };
        self.notify(said);
    }

    /// `r` on an instructions entry: restore platoon's default, after asking. Only a changed prompt offers it, so on
    /// one that is platoon's default already the key does nothing.
    pub(super) fn ask_reset_instructions(&mut self, id: &'static str) {
        if !instructions::status(&self.config, id).restorable() {
            return;
        }
        self.mode = Mode::Confirm(Confirm::ResetInstructions { id });
    }

    pub(super) fn reset_instructions(&mut self, id: &'static str) {
        self.mode = Mode::Settings;
        match instructions::reset(&self.config, id) {
            // (The confirm said where the user's version is kept.)
            Ok(()) => self.notify(format!("Restored platoon's default system prompt for {}.", instructions::action(id).what)),
            Err(err) => self.notify(format!("Couldn't restore the default system prompt: {}", plain(&err))),
        }
    }

    /// Keys while a terminal editor has platoon's window: everything goes to the editor, except the back key, which
    /// asks whether to leave it (the editor may hold typing not yet saved). While that question shows, `y` leaves
    /// and every other key stays: enter, esc and the back key again too, so a key pressed twice loses nothing.
    pub(super) fn editor_key(&mut self, key: KeyEvent) {
        let Some(ed) = self.editing.as_mut() else { return };
        if ed.leaving {
            if matches!(key.code, KeyCode::Char('y' | 'Y')) && !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
                return self.stop_editor();
            }
            ed.leaving = false;
            return;
        }
        if self.keys.get().back.matches(&key) {
            return self.ask_leave_editor();
        }
        let Some(ed) = self.editing.as_mut() else { return };
        let app_cursor = ed.run.parser().lock().map(|p| p.screen().application_cursor()).unwrap_or(false);
        let bytes = key_bytes(key, app_cursor);
        ed.run.write(&bytes);
    }

    pub(super) fn editor_paste(&mut self, text: &str) {
        if let Some(ed) = self.editing.as_mut() {
            ed.run.write(format!("\x1b[200~{text}\x1b[201~").as_bytes());
        }
    }

    /// The back key (or a click on its hint) in the editor: ask first, since leaving ends the editor.
    pub(super) fn ask_leave_editor(&mut self) {
        if let Some(ed) = self.editing.as_mut() {
            ed.leaving = true;
        }
    }

    /// The question's answer by mouse: `leave` ends the editor, otherwise it stays.
    pub(super) fn answer_leave_editor(&mut self, leave: bool) {
        match self.editing.as_mut() {
            Some(_) if leave => self.stop_editor(),
            Some(ed) => ed.leaving = false,
            None => {}
        }
    }

    /// "Leave the editor?" answered yes: ask it to leave (it is platoon's own child) and go back to the project.
    fn stop_editor(&mut self) {
        if let Some(ed) = self.editing.as_mut() {
            ed.run.stop();
        }
        self.finish_edit();
    }

    /// Take in an editor that has finished, and notice when a window editor is done with the file. Cheap when
    /// nothing is being edited; the event loop calls it every turn.
    pub fn poll_editors(&mut self) {
        if self.editing.as_mut().is_some_and(|e| e.run.finished()) {
            self.finish_edit();
        }
        let done = match self.notes_window.as_mut() {
            // Either the editor is done with the file, or it has written it: read the notes again either way.
            Some(w) => {
                matches!(w.child.try_wait(), Ok(Some(_)))
                    || w.file.as_ref().is_some_and(|sp| file_stamp(&sp.path) != w.stamp)
                    || w.file.is_none()
                        && match (w.instructions, w.ticket) {
                            (Some(id), _) => instructions::stamp(&self.config, id) != w.stamp,
                            (None, Some(n)) => {
                                let store = Tickets::open(&self.config, &w.project);
                                let path = store.get(n).map(|t| store.body_path(t));
                                path.is_some_and(|p| file_stamp(&p) != w.stamp)
                            }
                            (None, None) => notes::changed_on_disk(&self.config, &w.project, w.stamp),
                        }
            }
            None => false,
        };
        if done && let Some(w) = self.notes_window.take() {
            if let Some(sp) = &w.file {
                self.after_file_edit(sp);
                return self.poll_notes_changes();
            }
            match (w.instructions, w.ticket) {
                (Some(id), _) => self.after_instructions_edit(id, &w.before),
                (None, Some(n)) => self.after_ticket_edit(&w.project, n, &w.before, &w.name),
                (None, None) => self.after_edit(&w.project, w.stamp, &w.name),
            }
        }
        self.poll_notes_changes();
    }

    fn finish_edit(&mut self) {
        let Some(ed) = self.editing.take() else { return };
        if matches!(self.mode, Mode::EditNotes) {
            self.mode = Self::after_pick_mode(ed.instructions);
        }
        if let Some(sp) = &ed.file {
            return self.after_file_edit(sp);
        }
        if let Some(id) = ed.instructions {
            return self.after_instructions_edit(id, &ed.before);
        }
        if let Some(n) = ed.ticket {
            return self.after_ticket_edit(&ed.project, n, &ed.before, &ed.name);
        }
        self.notes_top.set(0);
        self.after_edit(&ed.project, ed.stamp, &ed.name);
    }

    /// What to say once an editor has been and gone. The words follow the text, not the file's timestamp: an
    /// editor that saves the same words over again has changed nothing.
    fn after_edit(&mut self, project: &str, stamp: notes::Stamp, name: &str) {
        notesrun::clear_marker(&notesrun::editing_path(&self.cache, project));
        self.forget_notes_stamp();
        let changed = match notes::last_change(&self.config, project) {
            Some((before, after)) => before != after,
            // No earlier version: the notes are brand new (platoon wrote the template), so the file itself says.
            None => notes::changed_on_disk(&self.config, project, stamp),
        };
        if !changed {
            // A copy was kept before the editor ran; with nothing to show for it, it would only bury the change
            // before it, so it goes.
            notes::forget_unchanged_version(&self.config, project);
            self.notify(format!("The notes are unchanged ({name} left them as they were)."));
            return;
        }
        self.notify("The notes changed. Press c to see what changed, u to undo it.");
    }

    // ---- what changed, and undoing it -------------------------------------------------------------------------

    /// `c`: the before/after view of the last change.
    pub(super) fn show_changes(&mut self) {
        let Some(project) = self.open_project.clone() else { return };
        let Some((before, after)) = notes::last_change(&self.config, &project) else {
            self.notify("There is no earlier version of these notes yet, so there is nothing to compare.");
            return;
        };
        let lines = notes::diff(&before, &after);
        if lines.iter().all(|(m, _)| *m != notes::Mark::Removed && *m != notes::Mark::Added) {
            self.notify("Nothing has changed since the last copy platoon kept.");
            return;
        }
        self.changes = Changes { lines, top: Cell::new(0) };
        self.mode = Mode::Changes;
    }

    pub(super) fn scroll_changes(&mut self, delta: i32) {
        self.changes.top.set((self.changes.top.get() as i32 + delta).max(0) as usize);
    }

    /// `u`: put the previous version back. That counts as a change of its own, so it can be undone too.
    pub(super) fn undo_notes(&mut self) {
        let Some(project) = self.open_project.clone() else { return };
        if let Some(name) = self.notes_editor_open().map(str::to_string) {
            self.notify(format!("The notes are open in {name}. Close it first, so nothing you are writing is lost."));
            return;
        }
        match notes::undo(&self.config, &project) {
            Ok(()) => {
                self.notes_top.set(0);
                self.forget_notes_stamp();
                self.notify("The notes are back as they were. Press u again to undo that too.");
            }
            Err(why) => self.notify(why),
        }
    }
}
