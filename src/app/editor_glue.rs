//! The code editor inside the app (`v`): what it opens from where the user is, the questions before it opens (the
//! squad is still working; it isn't installed), bringing a removed worktree back first, and the request the event
//! loop takes to hand the terminal over (`main.rs`). The editor itself - folders, config, colors, running it - is
//! `editor.rs`; installing it is `editor_install.rs`.
//!
//! Where `v` opens what:
//! - a session's row (Sessions tab, a project's Sessions pane): its folder and the folders added to it;
//! - a ticket's row or its screen: its worktrees, its folders not in git, its work folder when it has files;
//! - anywhere else (the Squads tab, the Projects list, the notes, an empty list): the folder picker asks.
//!
//! In the attached view every key is Claude's: ctrl+space back to the list first, then `v`. Typed into a box, `v` is
//! a letter (those modes never reach here).

use super::*;
use crate::editor::{self, Request};
use crate::editor_install::{self, Dirs, Part, Progress, Report};
use crate::tickets::{State, Ticket, Tickets};
use std::collections::BTreeMap;
use std::sync::mpsc::TryRecvError;

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
    /// The user said to open it although the squad is still working.
    warned: bool,
}

enum InstallMsg {
    Progress(Progress),
    Done(Result<Report, String>),
}

pub struct EdState {
    flow: Option<Flow>,
    /// Ready to open: the event loop takes it (`take_editor_request`) and hands the terminal over.
    request: Option<Request>,
    installing: Option<Receiver<InstallMsg>>,
    progress: BTreeMap<Part, Progress>,
    /// Where the editor is installed.
    pub dirs: Dirs,
    /// Installs from this instead of platoon's pins (unit tests: a mirror of fake downloads).
    #[cfg(test)]
    pub manifest: Option<editor_install::Manifest>,
    /// Run this instead of the editor (tests), as `PLATOON_NVIM_BIN` does.
    pub stand_in: bool,
}

impl EdState {
    #[cfg(not(test))]
    fn dirs() -> Dirs {
        Dirs::from_env()
    }

    /// A unit test's app never looks at the real editor folder: a folder that isn't there, until a test gives its own.
    #[cfg(test)]
    fn dirs() -> Dirs {
        let none = std::env::temp_dir().join(format!("platoon-unit-no-editor-{}", std::process::id()));
        Dirs { editor: none.join("editor"), fonts: none.join("fonts"), mirror: Some(none.join("mirror")), search_path: Vec::new(), refresh_fonts: false, platform: Some(editor_install::Platform::LinuxX64) }
    }

    pub fn new() -> Self {
        let env = |k: &str| std::env::var(k).ok();
        Self {
            flow: None,
            request: None,
            installing: None,
            progress: BTreeMap::new(),
            dirs: Self::dirs(),
            #[cfg(test)]
            manifest: None,
            stand_in: editor::stand_in(&env),
        }
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
        if self.squads_tab_showing() || self.projects_list_showing() {
            return None;
        }
        if let Some(project) = self.open_project.clone() {
            if let Some(number) = self.tk.open {
                return Some(EdTarget::Ticket { project, number });
            }
            return match self.pane {
                Pane::Tickets => self.selected_ticket().map(|t| EdTarget::Ticket { project, number: t.number }),
                Pane::Sessions => self.selected_session().map(|s| EdTarget::Session(s.id.clone())),
                Pane::Notes => None,
            };
        }
        self.selected_session().map(|s| EdTarget::Session(s.id.clone()))
    }

    /// The folder picker, for the editor: the project's folders first when one is open or selected.
    fn open_editor_picker(&mut self) {
        let project = self.open_project.clone().or_else(|| if self.projects_list_showing() { self.selected_project().map(|p| p.id.clone()) } else { None });
        self.open_picker_in(project);
        self.picker.editor = true;
    }

    /// Enter in the folder picker opened by `v`.
    pub(super) fn take_editor_folder(&mut self) {
        self.mode = Mode::List;
        if let Some(dir) = self.picker.results.get(self.picker.selected).cloned() {
            self.start_editor(EdTarget::Folder(dir));
        }
    }

    pub(super) fn start_editor(&mut self, target: EdTarget) {
        self.ed.flow = Some(Flow { target, warned: false });
        self.editor_continue();
    }

    /// The ticket a flow is about, as it is now.
    fn flow_ticket(&self, project: &str, number: u32) -> Option<Ticket> {
        match (&self.open_project, &self.tk.store) {
            (Some(p), Some(store)) if p == project => store.get(number).cloned(),
            _ => Tickets::open(&self.config, project).get(number).cloned(),
        }
    }

    /// Take the flow one step further: each question or wait returns here once it is answered or done.
    pub(super) fn editor_continue(&mut self) {
        let Some(flow) = self.ed.flow.clone() else { return };
        if let EdTarget::Ticket { project, number } = &flow.target {
            let (project, number) = (project.clone(), *number);
            // platoon may have just started removing this ticket's worktree (it was Done, its squad stopped): let that
            // finish first, so the editor never opens on a folder being taken away. A removed one is brought back below.
            self.settle_removal(&project, number);
            let Some(t) = self.flow_ticket(&project, number) else {
                self.ed.flow = None;
                return;
            };
            if !flow.warned && t.state == State::Working && self.leader_running(number) {
                self.mode = Mode::Confirm(Confirm::EditorWhileWorking { number, title: t.title.clone() });
                return;
            }
        }
        if !self.editor_installed() {
            if self.ed.installing.is_some() {
                self.notify("The code editor is being installed. It opens when that is done.");
            } else {
                self.mode = Mode::Confirm(Confirm::InstallEditor);
            }
            return;
        }
        if let EdTarget::Ticket { project, number } = &flow.target {
            let (project, number) = (project.clone(), *number);
            let gone = self.flow_ticket(&project, number).is_some_and(|t| t.worktrees.iter().any(|w| w.is_gone()));
            if gone && self.open_project.as_deref() == Some(project.as_str()) {
                if self.bring_worktrees_back_then(number, worktree_glue::After::Editor) {
                    self.notify(format!("Bringing #{number}'s worktree back from its branch first…"));
                    return;
                }
            }
        }
        self.ed.flow = None;
        let Some(req) = self.editor_request(&flow.target) else { return };
        if req.roots.is_empty() {
            self.notify("There is nothing to open: its folders aren't there any more.");
            return;
        }
        self.ed.request = Some(req);
    }

    /// The folders and the title for a target.
    fn editor_request(&mut self, target: &EdTarget) -> Option<Request> {
        let git = self.git_program.clone();
        match target {
            EdTarget::Session(id) => {
                let s = self.sessions.iter().find(|s| s.id == *id)?;
                Some(Request { title: s.name.clone(), roots: editor::session_roots(&s.cwd, &s.extra_dirs, &self.home, &git) })
            }
            EdTarget::Ticket { project, number } => {
                let t = self.flow_ticket(project, *number)?;
                let work = crate::tickets::work_dir(&self.config, project, *number);
                Some(Request { title: format!("#{} {}", t.number, t.title), roots: editor::ticket_roots(&t, &work, &git) })
            }
            EdTarget::Folder(dir) => Some(Request { title: dirs::tilde(dir, &self.home), roots: editor::folder_roots(dir, &git) }),
        }
    }

    /// Enough of the editor is installed to open it (or a test named a stand-in).
    fn editor_installed(&self) -> bool {
        if self.ed.stand_in {
            return true;
        }
        #[cfg(test)]
        if let Some(m) = &self.ed.manifest {
            return editor::ready(&editor_install::status_with(&self.ed.dirs, m));
        }
        editor::ready(&editor_install::status(&self.ed.dirs))
    }

    /// Yes to "the squad is still working".
    pub(super) fn editor_anyway(&mut self) {
        self.mode = Mode::List;
        if let Some(f) = self.ed.flow.as_mut() {
            f.warned = true;
        }
        self.editor_continue();
    }

    /// No to one of the editor's questions.
    pub(super) fn editor_cancel(&mut self, installing: bool) {
        self.mode = Mode::List;
        self.ed.flow = None;
        if installing {
            self.notify("No code editor, then. `platoon setup --add editor` installs it later.");
        }
    }

    /// Yes to installing it: off the UI thread, with a progress line; it opens once it is there. Remembered as
    /// wanted, like `platoon setup --add editor` (also after the user removed it: pressing `v` asked for it).
    pub(super) fn install_editor(&mut self) {
        self.mode = Mode::List;
        let _ = crate::totickets::change_choice(
            &self.config,
            setup::EDITOR,
            &[("asked", Some(serde_json::Value::Bool(true))), ("wanted", Some(serde_json::Value::Bool(true))), ("removed", None)],
        );
        let dirs = self.ed.dirs.clone();
        #[cfg(test)]
        let manifest = self.ed.manifest.clone();
        let (tx, rx) = channel();
        thread::spawn(move || {
            let progress = tx.clone();
            let report = move |p| {
                let _ = progress.send(InstallMsg::Progress(p));
            };
            #[cfg(test)]
            let done = match manifest {
                Some(m) => editor_install::install_with(&dirs, &m, &report),
                None => editor_install::install(&dirs, &report),
            };
            #[cfg(not(test))]
            let done = editor_install::install(&dirs, &report);
            let _ = tx.send(InstallMsg::Done(done));
        });
        self.ed.installing = Some(rx);
        self.ed.progress.clear();
        self.notify("Installing the code editor…");
    }

    /// The install's progress, in the words of the footer line.
    fn install_line(&self) -> String {
        let parts: Vec<String> = self
            .ed
            .progress
            .values()
            .map(|p| if p.done + p.failed >= p.total && p.failed == 0 { format!("{} ✓", p.part.label()) } else { format!("{} {}/{}", p.part.label(), p.done, p.total) })
            .collect();
        if parts.is_empty() { "Installing the code editor…".into() } else { format!("Installing the code editor: {}", parts.join("  ")) }
    }

    /// Called on every refresh: the install's progress, and when it is done, the editor opened (unless the user is
    /// busy with something else by then: then it is said how to open it).
    pub(super) fn poll_editor_install(&mut self) {
        let Some(rx) = &self.ed.installing else { return };
        let mut done = None;
        loop {
            match rx.try_recv() {
                Ok(InstallMsg::Progress(p)) => {
                    self.ed.progress.insert(p.part, p);
                }
                Ok(InstallMsg::Done(r)) => {
                    done = Some(r);
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    done = Some(Err("it stopped half way".into()));
                    break;
                }
            }
        }
        let Some(r) = done else {
            let line = self.install_line();
            self.notify(line);
            return;
        };
        self.ed.installing = None;
        match r {
            Ok(_) if matches!(self.mode, Mode::List) && self.ed.flow.is_some() => {
                self.notify("The code editor is installed.");
                self.editor_continue();
            }
            Ok(_) => {
                self.ed.flow = None;
                self.notify("The code editor is installed. Press v to open it.");
            }
            Err(e) => {
                self.ed.flow = None;
                self.notify(format!("The code editor didn't finish installing: {e} `platoon setup --add editor` tries again."));
            }
        }
    }

    /// The install is running (the event loop refreshes more often meanwhile, for the progress line).
    pub fn editor_busy(&self) -> bool {
        self.ed.installing.is_some()
    }

    /// The editor is ready to open: the event loop hands it the terminal.
    pub fn take_editor_request(&mut self) -> Option<Request> {
        self.ed.request.take()
    }

    pub fn editor_dirs(&self) -> &Dirs {
        &self.ed.dirs
    }

    /// The editor has closed (or couldn't start): say what went wrong, if anything, and look at everything again
    /// (sessions may have moved on, a ticket may be done).
    pub fn editor_closed(&mut self, result: Result<(), String>) {
        if let Err(e) = result {
            self.notify(format!("The code editor didn't open: {e}"));
        }
        self.refresh();
    }

    /// A `v` on ticket `number` of `project` is under way: its worktrees are left alone until it has opened.
    pub(super) fn editor_holds(&self, project: &str, number: u32) -> bool {
        matches!(&self.ed.flow, Some(Flow { target: EdTarget::Ticket { project: p, number: n }, .. }) if p == project && *n == number)
    }

    #[cfg(test)]
    pub fn editor_flow_for_test(&self) -> Option<EdTarget> {
        self.ed.flow.as_ref().map(|f| f.target.clone())
    }
}
