//! Editing a project's notes from the project screen: the editor picker, running the editor (in platoon's own
//! window for a terminal one, beside it for one with a window of its own), the before/after view of the last
//! change and undoing it. The file itself, its history and the diff are in `notes.rs`; finding editors and running
//! them is in `editors.rs`.

use super::*;
use crate::editors::{self, Editor};
use crate::notes;

/// A terminal editor running inside platoon's window on a project's notes.
pub struct NotesEdit {
    pub project: String,
    /// The editor's plain name, for the status line.
    pub name: String,
    /// What the file looked like when the editor started, so platoon can tell whether anything came of it.
    stamp: notes::Stamp,
    pub run: editors::Running,
}

/// An editor with a window of its own that platoon opened on a project's notes. platoon carries on as usual and
/// reads the notes again when the editor is done with them, or as soon as the file changes.
pub struct NotesWindow {
    pub project: String,
    pub name: String,
    stamp: notes::Stamp,
    child: std::process::Child,
}

/// The list of editors, shown by `E` (and by `e` while the setting says "Ask each time").
pub struct EditorPick {
    pub project: String,
    pub options: Vec<Editor>,
    pub selected: usize,
}

/// The before/after view of the last change to the notes.
pub struct Changes {
    pub lines: Vec<(notes::Mark, String)>,
    pub top: Cell<usize>,
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

    /// An editor platoon started is open on some project's notes, so platoon must not write them itself.
    pub fn notes_busy(&self) -> bool {
        self.editing.is_some() || self.notes_window.is_some()
    }

    /// The name of the editor platoon has open on the notes of the project whose screen is showing, if any.
    pub fn notes_editor_open(&self) -> Option<&str> {
        let project = self.open_project.as_deref()?;
        if let Some(e) = self.editing.as_ref().filter(|e| e.project == project) {
            return Some(&e.name);
        }
        self.notes_window.as_ref().filter(|w| w.project == project).map(|w| w.name.as_str())
    }

    /// What the "Edit notes with" row of the settings shows, and how wide its choices need it to be.
    pub fn notes_editor_label(&self) -> String {
        editors::label(&self.editors, &self.settings.get().notes_editor)
    }

    pub fn notes_editor_width(&self) -> usize {
        editors::setting_ids(&self.editors).iter().map(|id| editors::label(&self.editors, id).chars().count()).max().unwrap_or(3)
    }

    /// True when the setting is already at the first (last) of its choices, so the arrow is drawn as spent.
    pub fn notes_editor_at_end(&self, last: bool) -> bool {
        let ids = editors::setting_ids(&self.editors);
        let at = ids.iter().position(|i| *i == self.settings.get().notes_editor).unwrap_or(0);
        if last { at + 1 >= ids.len() } else { at == 0 }
    }

    /// `←`/`→`/enter on the "Edit notes with" row: the next editor found, saved at once.
    pub(super) fn step_notes_editor(&mut self, step: Step) {
        let ids = editors::setting_ids(&self.editors);
        let at = ids.iter().position(|i| *i == self.settings.get().notes_editor).unwrap_or(0);
        let to = match step {
            Step::Prev => at.checked_sub(1),
            Step::Next => Some(at + 1).filter(|&n| n < ids.len()),
            Step::Cycle => Some(if at + 1 < ids.len() { at + 1 } else { 0 }),
        };
        let Some(to) = to else { return };
        let mut next = self.settings.get();
        next.notes_editor.clone_from(&ids[to]);
        let saved = self.settings.update(&next);
        self.report_save(saved);
    }

    /// The warning shown above big notes, or None.
    pub fn notes_size_warning(&self) -> Option<String> {
        let project = self.open_project.as_deref()?;
        notes::size_warning(notes::size(&self.config, project))
    }

    // ---- starting an editor -----------------------------------------------------------------------------------

    /// `e`: edit the open project's notes with the chosen editor (asking which one when that is the setting).
    /// `E` (`always_ask`): always ask, for this once.
    pub(super) fn edit_notes(&mut self, always_ask: bool) {
        let Some(project) = self.open_project.clone() else { return };
        if let Some(name) = self.notes_editor_open().map(str::to_string) {
            self.notify(format!("The notes are already open in {name}."));
            return;
        }
        if self.notes_busy() {
            self.notify("platoon is already editing another project's notes.");
            return;
        }
        let chosen = self.settings.get().notes_editor;
        match editors::by_id(&self.editors, &chosen).cloned() {
            Some(e) if !always_ask => self.run_editor(&project, &e),
            // "Ask each time", an editor this computer no longer has, or E: show the list.
            _ => {
                let selected = self.editors.iter().position(|e| e.id == chosen).unwrap_or(0);
                self.epick = EditorPick { project, options: self.editors.clone(), selected };
                self.mode = Mode::PickEditor;
            }
        }
    }

    pub(super) fn move_editor_pick(&mut self, delta: i32) {
        let last = self.epick.options.len().saturating_sub(1) as i32;
        self.epick.selected = (self.epick.selected as i32 + delta).clamp(0, last) as usize;
    }

    /// Enter in the editor list: use that editor this time.
    pub(super) fn take_editor_pick(&mut self) {
        let Some(e) = self.epick.options.get(self.epick.selected).cloned() else {
            self.mode = Mode::List;
            return;
        };
        let project = self.epick.project.clone();
        self.mode = Mode::List;
        self.run_editor(&project, &e);
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
        if e.terminal {
            let (rows, cols) = self.pty_size();
            match editors::Running::spawn(e, &path, rows, cols) {
                Ok(run) => {
                    self.editing = Some(NotesEdit { project: project.to_string(), name: e.name.clone(), stamp, run });
                    self.mode = Mode::EditNotes;
                    self.clear_selection();
                }
                Err(err) => self.notify(format!("Couldn't open the notes in {}: {}", e.name, plain(&err))),
            }
            return;
        }
        match editors::launch_window(e, &path) {
            Ok(child) => {
                self.notes_window = Some(NotesWindow { project: project.to_string(), name: e.name.clone(), stamp, child });
                self.notify(format!("The notes opened in {}, in a window of its own.", e.name));
            }
            Err(err) => self.notify(format!("Couldn't open the notes in {}: {}", e.name, plain(&err))),
        }
    }

    /// Keys while a terminal editor has platoon's window: everything goes to the editor, except the chord that
    /// ends it.
    pub(super) fn editor_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char(' ') {
            return self.stop_editor();
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

    /// Ctrl+Space in the editor: ask it to leave (it is platoon's own child) and go back to the project.
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
            Some(w) => matches!(w.child.try_wait(), Ok(Some(_))) || notes::changed_on_disk(&self.config, &w.project, w.stamp),
            None => false,
        };
        if done {
            if let Some(w) = self.notes_window.take() {
                self.after_edit(&w.project, w.stamp, &w.name);
            }
        }
    }

    fn finish_edit(&mut self) {
        let Some(ed) = self.editing.take() else { return };
        if matches!(self.mode, Mode::EditNotes) {
            self.mode = Mode::List;
        }
        self.notes_top.set(0);
        self.after_edit(&ed.project, ed.stamp, &ed.name);
    }

    /// What to say once an editor has been and gone. The words follow the text, not the file's timestamp: an
    /// editor that saves the same words over again has changed nothing.
    fn after_edit(&mut self, project: &str, stamp: notes::Stamp, name: &str) {
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
                self.notify("The notes are back as they were. Press u again to undo that too.");
            }
            Err(why) => self.notify(why),
        }
    }
}
