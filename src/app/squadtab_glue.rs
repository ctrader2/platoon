//! The Squads tab (`4`): every squad's file to read, and the three things done to one: `e` opens it in the notes'
//! editors (a built-in's text written to the user's folder first, and taken away again when left as it was), `n` makes a
//! new one from a template, `d` deletes one of the user's (kept as `<id>.md.bak`). The files themselves are
//! `squads.rs`; the drawing is `ui/squads_view.rs`.

use super::*;
use crate::squads::{self, NewError, Origin, Squad};

/// The Squads tab: the squads as their files said when last read, the picked one, and whether it is open full width.
#[derive(Default)]
pub struct SquadsTab {
    pub list: Vec<Squad>,
    pub selected: usize,
    /// The picked squad is open in the tab's whole width (enter), scrolled with the arrows; esc goes back to the list.
    pub open: bool,
    /// The first line of the open squad in view.
    pub top: Cell<usize>,
    /// The first row of the list in view.
    pub list_top: Cell<usize>,
    /// How many rows (or, with a squad open, lines) a page is, as last drawn.
    pub page: Cell<usize>,
}

impl SquadsTab {
    pub fn picked(&self) -> Option<&Squad> {
        self.list.get(self.selected)
    }
}

/// Where a squad comes from, in the list's words.
pub fn origin_words(o: Origin) -> &'static str {
    match o {
        Origin::BuiltIn => "built in",
        Origin::Own => "yours",
        Origin::Replaces => "yours, replaces the built-in",
    }
}

impl App {
    /// The Squads tab's list (or a squad open from it) is showing.
    pub fn squads_showing(&self) -> bool {
        self.top == TOP_SQUADS && self.open_project.is_none()
    }

    /// Read every squad's file again, keeping the cursor on the same squad (or `pick`, when given) if it is still there.
    pub(super) fn reload_squads(&mut self, pick: Option<&str>) {
        let keep = pick.map(str::to_string).or_else(|| self.sqt.picked().map(|s| s.id.clone()));
        self.sqt.list = squads::all(&self.config);
        if let Some(i) = keep.and_then(|id| self.sqt.list.iter().position(|s| s.id == id)) {
            self.sqt.selected = i;
        }
        self.sqt.selected = self.sqt.selected.min(self.sqt.list.len().saturating_sub(1));
    }

    pub(super) fn squads_key(&mut self, key: KeyEvent) {
        let page = super::page_of(&self.sqt.page);
        match key.code {
            KeyCode::Char('e') => self.edit_squad(),
            KeyCode::Char('n') => self.open_name_box(NamePurpose::NewSquad),
            KeyCode::Char('d') => self.ask_delete_squad(),
            KeyCode::Enter if !self.sqt.open => self.open_squad(),
            KeyCode::Esc if self.sqt.open => self.close_squad(),
            KeyCode::Down | KeyCode::Char('j') => self.move_squads(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_squads(-1),
            KeyCode::PageDown => self.move_squads(page),
            KeyCode::PageUp => self.move_squads(-page),
            KeyCode::Char('g') | KeyCode::Home => self.move_squads(i32::MIN / 2),
            KeyCode::Char('G') | KeyCode::End => self.move_squads(i32::MAX / 2),
            _ => {}
        }
    }

    /// The arrows: through the list, or, with a squad open, through its lines.
    pub(super) fn move_squads(&mut self, delta: i32) {
        if self.sqt.open {
            self.sqt.top.set((self.sqt.top.get() as i64 + i64::from(delta)).clamp(0, u32::MAX as i64) as usize);
            return;
        }
        let last = self.sqt.list.len().saturating_sub(1) as i64;
        self.sqt.selected = (self.sqt.selected as i64 + i64::from(delta)).clamp(0, last) as usize;
    }

    pub(super) fn open_squad(&mut self) {
        if self.sqt.picked().is_some() {
            self.sqt.open = true;
            self.sqt.top.set(0);
        }
    }

    pub(super) fn close_squad(&mut self) {
        self.sqt.open = false;
    }

    /// A click on a row of the list: picks it; on the picked one, opens it.
    pub(super) fn click_squads_row(&mut self, i: usize) {
        if i == self.sqt.selected {
            self.open_squad();
        } else if i < self.sqt.list.len() {
            self.sqt.selected = i;
        }
    }

    /// `e`: the picked squad's file in the notes' editors. A built-in's text is written to the user's folder first.
    pub(super) fn edit_squad(&mut self) {
        let Some(s) = self.sqt.picked().cloned() else { return };
        if let Some(name) = self.editing.as_ref().map(|e| e.name.clone()).or_else(|| self.notes_window.as_ref().map(|w| w.name.clone())) {
            self.notify(format!("{name} is still open on something else. Close it first."));
            return;
        }
        match squads::prepare_edit(&self.config, &s.id) {
            Ok((path, before, copied)) => {
                self.reload_squads(None);
                self.pick_file_editor(FileEdit { of: FileOf::Squad { id: s.id.clone(), copied }, path, before });
            }
            Err(e) => self.notify(format!("Couldn't open the squad {}: {}", s.name_or_id(), plain_io(&e))),
        }
    }

    /// An editor has been and gone from a squad's file: read the squads again and say what came of it.
    pub(super) fn after_squad_edit(&mut self, edit: &FileEdit) {
        let FileOf::Squad { id, copied } = &edit.of else { return };
        let changed = squads::after_edit(&self.config, id, &edit.before, *copied);
        self.reload_squads(Some(id));
        let s = self.sqt.list.iter().find(|s| s.id == *id).cloned();
        let name = s.as_ref().map_or_else(|| id.clone(), |s| s.name_or_id());
        let said = match (changed, s) {
            (false, _) if *copied => format!("{name} is unchanged, so it stays built in."),
            (false, _) => format!("{name} is unchanged."),
            (true, Some(s)) if !s.usable() => format!("Saved {name}. It needs fixing: what is wrong is under its steps."),
            (true, _) => format!("Saved {name}. Tasks started from now on follow it."),
        };
        self.notify(said);
    }

    /// Enter in the name box of `n`: the new squad's file from the template, then open in the editors.
    pub(super) fn save_new_squad(&mut self) {
        let name = projects_mod::clean(&self.name_box.input, projects_mod::NAME_MAX);
        match squads::create(&self.config, &name) {
            Ok(id) => {
                self.mode = Mode::List;
                self.sqt.open = false;
                self.reload_squads(Some(&id));
                self.edit_squad();
            }
            // The box stays open for another name.
            Err(NewError::NoName) => self.notify("A squad needs a name with a letter or a digit in it"),
            Err(NewError::Taken(other)) => self.notify(format!("There is a squad called {other} already. Give this one another name.")),
            Err(NewError::Io(e)) => {
                self.mode = Mode::List;
                self.notify(format!("Couldn't make the squad: {}", plain_io(&e)));
            }
        }
    }

    /// `d`: delete the picked squad, after asking. Only the user's own: a built-in is changed with `e`, not deleted.
    pub(super) fn ask_delete_squad(&mut self) {
        let Some(s) = self.sqt.picked() else { return };
        if s.origin == Origin::BuiltIn {
            let name = s.name_or_id();
            self.notify(format!("{name} is built in, so it can't be deleted. e changes it."));
            return;
        }
        self.mode = Mode::Confirm(Confirm::DeleteSquad { id: s.id.clone(), name: s.name_or_id(), replaces: s.origin == Origin::Replaces });
    }

    pub(super) fn delete_squad(&mut self, id: &str, name: &str, replaces: bool) {
        self.mode = Mode::List;
        match squads::delete(&self.config, id) {
            Ok(()) => {
                self.sqt.open = false;
                self.reload_squads(None);
                self.notify(if replaces {
                    format!("Deleted your {name}: the built-in one is back. Your file is kept as {id}.md.bak.")
                } else {
                    format!("Deleted {name}. Its file is kept as {id}.md.bak.")
                });
            }
            Err(e) => self.notify(format!("Couldn't delete {name}: {}", plain_io(&e))),
        }
    }
}

fn plain_io(e: &io::Error) -> String {
    let why = e.to_string();
    why.split(" (os error").next().unwrap_or(&why).to_string()
}
