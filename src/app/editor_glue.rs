//! Opening a task's or a session's folders in a code editor of this computer's (`v`): what it opens from where the
//! user is, the question before it opens (its session is still working), bringing a removed worktree back first, and the
//! request the event loop takes to hand the terminal to an editor that draws in it (`main.rs`). The folders and
//! running a terminal editor are `editor.rs`; finding the editors and starting a windowed one is `fileeditors.rs`.
//!
//! Where `v` opens what:
//! - a session's row (Sessions tab, a project's Sessions pane): its folder and the folders added to it;
//! - a task's row or its screen: its worktree, or its folder when it has none;
//! - anywhere else (the Projects list, the notes, an empty list): the folder picker asks.
//!
//! In the attached view every key is Claude's: the back key (ctrl+space) to the list first, then `v`. Typed into a box, `v` is
//! a letter (those modes never reach here).
//!
//! Which editor: `v` opens the favourite (`files_editor` in the settings; the first editor found until the user picks
//! one, and instead of one no longer installed, said once), or, with "Always ask which editor" on, the list with the
//! favourite starred and picked (picking another makes it the favourite). platoon installs no editor: with none found,
//! `v` says so (`NO_EDITOR`). Everything else is the same for every editor: the session-still-working question, bringing
//! a removed worktree back, the folder picker. An editor that draws in the terminal (Helix, the user's own Neovim) is
//! handed platoon's terminal (`main.rs`); one with a window of its own starts beside platoon, and a task's worktree
//! opened in one is kept after the work (platoon cannot tell when that window closes).

use super::*;
use crate::editor::{self, Request};
use crate::fileeditors::{self, FileEditor, Runs};
use crate::tickets::{State, Ticket, Tickets};

/// What `v` says when this computer has none of the editors it can open.
pub const NO_EDITOR: &str = "No editor found: install VS Code, Zed, Helix or a JetBrains IDE.";

/// What `v` was pressed on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EdTarget {
    /// A conversation, by id.
    Session(String),
    /// A ticket of a project.
    Ticket { project: String, number: u32 },
    /// A folder picked by hand.
    Folder(String),
}

/// A `v` on its way to opening the editor, through whatever has to happen first.
#[derive(Clone, Debug)]
struct Flow {
    target: EdTarget,
    /// The user said to open it although its session is still working.
    warned: bool,
    /// The editor it opens in; None while the list of editors is showing.
    editor: Option<FileEditor>,
}

/// The list of editors `v` shows with "Always ask which editor" on.
#[derive(Clone, Debug, Default)]
pub struct FilePick {
    pub options: Vec<FileEditor>,
    pub selected: usize,
    /// The favourite's id: the one with the star.
    pub starred: String,
}

pub struct EdState {
    flow: Option<Flow>,
    /// The editors `v` can offer: this computer's (`fileeditors::detect`, when platoon started).
    pub files: Vec<FileEditor>,
    /// The list showing (`Mode::PickFileEditor`).
    pub fpick: FilePick,
    /// A terminal editor, ready to open on these folders: the event loop takes it (`take_own_editor_request`).
    own: Option<(FileEditor, Vec<String>)>,
    /// The favourite that isn't installed, once it has been said (said once per run).
    said_missing: Option<String>,
    /// Said once the editor that has the terminal closes (what it left out).
    note: Option<String>,
}

impl EdState {
    pub fn new() -> Self {
        Self { flow: None, files: Self::files(), fpick: FilePick::default(), own: None, said_missing: None, note: None }
    }

    #[cfg(not(test))]
    fn files() -> Vec<FileEditor> {
        fileeditors::detect()
    }

    /// A unit test's app never looks at this computer's editors: none, until a test gives its own.
    #[cfg(test)]
    fn files() -> Vec<FileEditor> {
        Vec::new()
    }
}

impl App {
    /// `v`: open the editor on what is selected, or ask for a folder.
    pub(super) fn code_editor_key(&mut self) {
        match self.editor_target() {
            Some(t) => self.start_editor(t),
            None => self.open_editor_picker(),
        }
    }

    /// What `v` opens from where the user is, if anything is selected there.
    pub fn editor_target(&self) -> Option<EdTarget> {
        if self.projects_list_showing() {
            return None;
        }
        if self.board_showing() {
            let project = self.task_scope()?;
            return match self.tk.open {
                Some(number) => Some(EdTarget::Ticket { project, number }),
                None => self.board_task().map(|(project, number)| EdTarget::Ticket { project, number }),
            };
        }
        if self.open_project.is_some() {
            return match self.pane {
                Pane::Sessions => self.selected_session().map(|s| EdTarget::Session(s.id.clone())),
                Pane::Notes => None,
            };
        }
        self.selected_session().map(|s| EdTarget::Session(s.id.clone()))
    }

    /// The folder picker, for the editor: the project's folders first when one is open or selected.
    fn open_editor_picker(&mut self) {
        let project = self
            .open_project
            .clone()
            .or_else(|| if self.projects_list_showing() { self.selected_project().map(|p| p.id.clone()) } else { None });
        self.open_picker_in(project);
        self.picker.editor = true;
        // The whole home folder is rarely what an editor should open: the first real folder is picked instead (the
        // project's own first, where one is open), and `~` stays in the list for whoever wants it.
        if let Some(i) = self.picker.results.iter().position(|p| *p != self.home) {
            self.picker.selected = i;
        }
    }

    /// Enter in the folder picker opened by `v`.
    pub(super) fn take_editor_folder(&mut self) {
        self.mode = Mode::List;
        if let Some(dir) = self.picker.results.get(self.picker.selected).cloned() {
            self.start_editor(EdTarget::Folder(dir));
        }
    }

    pub(super) fn start_editor(&mut self, target: EdTarget) {
        let Some(favourite) = self.files_favourite() else {
            self.notify(NO_EDITOR);
            return;
        };
        if self.settings.get().files_editor_ask {
            let selected = self.ed.files.iter().position(|e| e.id == favourite.id).unwrap_or(0);
            self.ed.fpick = FilePick { options: self.ed.files.clone(), selected, starred: favourite.id };
            self.ed.flow = Some(Flow { target, warned: false, editor: None });
            self.mode = Mode::PickFileEditor;
            return;
        }
        self.ed.flow = Some(Flow { target, warned: false, editor: Some(favourite) });
        self.editor_continue();
    }

    /// The favourite editor for `v`, if this computer has it; else the first one found (said once when the favourite
    /// was a choice of the user's: the setting stays, in case it comes back). None: this computer has none.
    fn files_favourite(&mut self) -> Option<FileEditor> {
        let id = self.settings.get().files_editor;
        if let Some(e) = fileeditors::by_id(&self.ed.files, &id) {
            return Some(e.clone());
        }
        let first = self.ed.files.first()?.clone();
        if !id.is_empty() && self.ed.said_missing.as_deref() != Some(id.as_str()) {
            let line = format!(
                "{} isn't on this computer any more, so {} opens instead (Settings, Editors).",
                fileeditors::name_of(&id),
                first.name
            );
            self.notify(line.clone());
            self.ed.note = Some(line);
            self.ed.said_missing = Some(id);
        }
        Some(first)
    }

    /// Enter in `v`'s list: that editor, which becomes the favourite.
    pub(super) fn take_file_editor_pick(&mut self) {
        self.mode = Mode::List;
        let Some(e) = self.ed.fpick.options.get(self.ed.fpick.selected).cloned() else {
            self.ed.flow = None;
            return;
        };
        if self.settings.get().files_editor != e.id {
            let mut next = self.settings.get();
            next.files_editor = e.id.clone();
            let saved = self.settings.update(&next);
            if saved.is_err() {
                self.report_save(saved);
            }
        }
        match self.ed.flow.as_mut() {
            Some(f) => f.editor = Some(e),
            None => return,
        }
        self.editor_continue();
    }

    /// Esc in `v`'s list.
    pub(super) fn cancel_file_editor_pick(&mut self) {
        self.mode = Mode::List;
        self.ed.flow = None;
        self.ed.note = None;
    }

    pub(super) fn move_file_editor_pick(&mut self, delta: i32) {
        let last = self.ed.fpick.options.len().saturating_sub(1) as i32;
        self.ed.fpick.selected = (self.ed.fpick.selected as i32 + delta).clamp(0, last) as usize;
    }

    pub fn file_pick(&self) -> &FilePick {
        &self.ed.fpick
    }

    /// The editors `v` can offer (the settings screen steps through them).
    pub fn file_editors(&self) -> &[FileEditor] {
        &self.ed.files
    }

    #[cfg(test)]
    pub fn set_file_editors_for_test(&mut self, list: Vec<FileEditor>) {
        self.ed.files = list;
    }

    /// The ticket a flow is about, as it is now.
    fn flow_ticket(&self, project: &str, number: u32) -> Option<Ticket> {
        match &self.tk.store {
            Some(store) if store.project() == project => store.get(number).cloned(),
            _ => Tickets::open(&self.config, project).get(number).cloned(),
        }
    }

    /// Take the flow one step further: each question or wait returns here once it is answered or done.
    pub(super) fn editor_continue(&mut self) {
        let Some(flow) = self.ed.flow.clone() else { return };
        let Some(chosen) = flow.editor.clone() else {
            self.ed.flow = None;
            return;
        };
        if let EdTarget::Ticket { project, number } = &flow.target {
            let (project, number) = (project.clone(), *number);
            // platoon may have just started removing this ticket's worktree (it was Done, its squad stopped): let that
            // finish first, so the editor never opens on a folder being taken away. A removed one is brought back below.
            self.settle_removal(&project, number);
            let Some(t) = self.flow_ticket(&project, number) else {
                self.ed.flow = None;
                return;
            };
            if !flow.warned && t.state == State::Working && t.session.as_deref().is_some_and(|s| self.task_session_running(s)) {
                self.mode = Mode::Confirm(Confirm::EditorWhileWorking { number, title: t.title.clone() });
                return;
            }
        }
        if let EdTarget::Ticket { project, number } = &flow.target {
            let (project, number) = (project.clone(), *number);
            let gone = self.flow_ticket(&project, number).is_some_and(|t| t.worktree.as_ref().is_some_and(|w| w.is_gone()));
            if gone
                && self.task_scope().as_deref() == Some(project.as_str())
                && self.bring_worktree_back_then(number, worktree_glue::After::Editor)
            {
                return;
            }
        }
        self.ed.flow = None;
        let Some(req) = self.editor_request(&flow.target) else { return };
        if req.roots.is_empty() {
            self.notify("There is nothing to open: its folders aren't there any more.");
            return;
        }
        match chosen.runs {
            Runs::Terminal => {
                let dirs = req.dirs();
                if dirs.len() > 1 {
                    let others: Vec<String> = dirs[1..].iter().map(|d| dirs::tilde(d, &self.home)).collect();
                    let first = dirs::tilde(&dirs[0], &self.home);
                    self.ed.note = Some(format!(
                        "{} opened {first} only: it opens one folder at a time. Not opened: {}.",
                        chosen.name,
                        others.join(", ")
                    ));
                }
                self.ed.own = Some((chosen, dirs));
            }
            Runs::Window => self.open_in_window(&flow.target, &chosen, &req),
        }
    }

    /// Start an editor with a window of its own on `req`'s folders, beside platoon. A task's worktree opened in it is
    /// written down as kept (platoon can't tell when that window closes).
    fn open_in_window(&mut self, target: &EdTarget, e: &FileEditor, req: &Request) {
        let dirs = req.dirs();
        let (commands, left) = fileeditors::commands(e, &dirs);
        let env = |k: &str| std::env::var(k).ok();
        let mut opened = 0;
        for (program, args) in &commands {
            // In the folder it opens (the first of them when it opens several).
            let cwd = args.iter().find(|a| dirs.contains(a)).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(&dirs[0]));
            if let Err(err) = fileeditors::launch_window(program, args, &cwd, &env) {
                let why = err.to_string();
                let why = why.split(" (os error").next().unwrap_or(&why).to_string();
                let prior = self.ed.note.take().map(|n| format!("{n} ")).unwrap_or_default();
                self.notify(format!("{prior}Couldn't open it in {}: {why}", e.name));
                if opened == 0 {
                    return;
                }
                break;
            }
            opened += 1;
        }
        let shown = if e.several == fileeditors::Several::EachWindow { opened } else { dirs.len() };
        let mut line = match (e.several, dirs.len()) {
            (fileeditors::Several::EachWindow, n) if n > 1 && left > 0 => {
                format!(
                    "Opened {opened} of the {n} folders in {}, a window each (it opens at most {}).",
                    e.name,
                    fileeditors::JETBRAINS_MOST
                )
            }
            (fileeditors::Several::EachWindow, n) if n > 1 => format!("Opened the {n} folders in {}, a window each.", e.name),
            _ => format!("Opened in {}, in a window of its own.", e.name),
        };
        if let EdTarget::Ticket { project, number } = target {
            let mut store = Tickets::open(&self.config, project);
            if req.roots.iter().take(shown).any(|r| r.kind == "worktree") && store.mark_opened_in(*number, &e.name).unwrap_or(false) {
                self.reread_tickets();
                line.push_str(&format!(" #{number}'s worktree stays after the work: remove it with git when you no longer need it."));
            }
        }
        if let Some(n) = self.ed.note.take() {
            line = format!("{n} {line}");
        }
        self.notify(line);
    }

    /// The folders for a target.
    fn editor_request(&mut self, target: &EdTarget) -> Option<Request> {
        match target {
            EdTarget::Session(id) => {
                let s = self.sessions.iter().find(|s| s.id == *id)?;
                Some(Request { roots: editor::session_roots(&s.cwd, &s.extra_dirs, &self.home) })
            }
            EdTarget::Ticket { project, number } => {
                let t = self.flow_ticket(project, *number)?;
                Some(Request { roots: editor::ticket_roots(&t) })
            }
            EdTarget::Folder(dir) => Some(Request { roots: editor::folder_roots(dir) }),
        }
    }

    /// Yes to "its session is still working".
    pub(super) fn editor_anyway(&mut self) {
        self.mode = Mode::List;
        if let Some(f) = self.ed.flow.as_mut() {
            f.warned = true;
        }
        self.editor_continue();
    }

    /// No to "its session is still working".
    pub(super) fn editor_cancel(&mut self) {
        self.mode = Mode::List;
        self.ed.flow = None;
        self.ed.note = None;
    }

    /// A terminal editor is ready to open on these folders (it opens the first): the event loop hands it the terminal.
    pub fn take_own_editor_request(&mut self) -> Option<(FileEditor, Vec<String>)> {
        self.ed.own.take()
    }

    /// The editor has closed (or couldn't start): say what went wrong, if anything, and look at everything again
    /// (sessions may have moved on, a task may be done).
    pub fn editor_closed(&mut self, result: Result<(), String>) {
        let note = self.ed.note.take();
        match (result, note) {
            (Err(e), _) => self.notify(format!("The editor didn't open: {e}")),
            (Ok(()), Some(n)) => self.notify(n),
            (Ok(()), None) => {}
        }
        self.refresh();
    }

    /// A `v` on task `number` of `project` is under way: its worktree is left alone until it has opened.
    pub(super) fn editor_holds(&self, project: &str, number: u32) -> bool {
        matches!(&self.ed.flow, Some(Flow { target: EdTarget::Ticket { project: p, number: n }, .. }) if p == project && *n == number)
    }

    #[cfg(test)]
    pub fn editor_flow_for_test(&self) -> Option<EdTarget> {
        self.ed.flow.as_ref().map(|f| f.target.clone())
    }
}
