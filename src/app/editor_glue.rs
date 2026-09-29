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
//! In the attached view every key is Claude's: the back key (ctrl+space) to the list first, then `v`. Typed into a box, `v` is
//! a letter (those modes never reach here).
//!
//! Which editor: platoon's own, or one of this computer's (`fileeditors.rs`). `v` opens the favourite
//! (`files_editor` in the settings; platoon's editor until the user picks another, and instead of one no longer
//! installed, said once), or, with "Always ask which editor" on, the list with the favourite starred and picked
//! (picking another makes it the favourite). Everything else is the same for every editor: the squad-still-working
//! question, bringing a removed worktree back, the folder picker. Only platoon's own is ever installed: it is a plugin
//! (`editorplugin.rs`), and `v` on it when it isn't there offers the plugin: installed from the catalog built into platoon, then
//! its downloads, with progress in the footer, then it opens. A terminal
//! editor of the user's own (Helix, their Neovim) is handed the terminal as platoon's is (`main.rs`); one with a
//! window of its own starts beside platoon, and a ticket's worktree opened in one is kept after the work until `w`
//! (platoon cannot tell when that window closes).

use super::*;
use crate::editor::{self, Request};
use crate::editor_install::{Dirs, Part, Progress, Report};
use crate::editorplugin;
use crate::plugins::Origin;
use crate::fileeditors::{self, FileEditor, Runs};
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
    /// platoon's own editor isn't installed yet (choosing it offers the install).
    pub platoon_missing: bool,
}

enum InstallMsg {
    /// What it is doing before the downloads: installing the plugin.
    Stage(String),
    Progress(Progress),
    Done(Result<Report, String>),
}

pub struct EdState {
    flow: Option<Flow>,
    /// Ready to open: the event loop takes it (`take_editor_request`) and hands the terminal over.
    request: Option<Request>,
    installing: Option<Receiver<InstallMsg>>,
    progress: BTreeMap<Part, Progress>,
    /// What the install is doing before its downloads start.
    stage: Option<String>,
    /// Where the editor is installed.
    pub dirs: Dirs,
    /// Run this instead of the editor (tests), as `PLATOON_NVIM_BIN` does.
    pub stand_in: bool,
    /// The editors `v` can offer: platoon's own, then this computer's (`fileeditors::detect`, when platoon started).
    pub files: Vec<FileEditor>,
    /// The list showing (`Mode::PickFileEditor`).
    pub fpick: FilePick,
    /// A terminal editor of the user's own, ready to open on these folders: the event loop takes it
    /// (`take_own_editor_request`).
    own: Option<(FileEditor, Vec<String>)>,
    /// The favourite that isn't installed, once it has been said (said once per run).
    said_missing: Option<String>,
    /// Said once the editor that has the terminal closes (what it left out).
    note: Option<String>,
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
        Dirs { editor: none.join("editor"), fonts: none.join("fonts"), mirror: Some(none.join("mirror")), search_path: Vec::new(), refresh_fonts: false, platform: Some(crate::editor_install::Platform::LinuxX64) }
    }

    pub fn new() -> Self {
        let env = |k: &str| std::env::var(k).ok();
        Self {
            flow: None,
            request: None,
            installing: None,
            progress: BTreeMap::new(),
            stage: None,
            dirs: Self::dirs(),
            stand_in: editor::stand_in(&env),
            files: Self::files(),
            fpick: FilePick::default(),
            own: None,
            said_missing: None,
            note: None,
        }
    }

    #[cfg(not(test))]
    fn files() -> Vec<FileEditor> {
        fileeditors::detect()
    }

    /// A unit test's app never looks at this computer's editors: platoon's own only, until a test gives its own.
    #[cfg(test)]
    fn files() -> Vec<FileEditor> {
        vec![FileEditor::platoon()]
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
        let favourite = self.files_favourite();
        if self.settings.get().files_editor_ask {
            let selected = self.ed.files.iter().position(|e| e.id == favourite.id).unwrap_or(0);
            let platoon_missing = !self.editor_installed();
            self.ed.fpick = FilePick { options: self.ed.files.clone(), selected, starred: favourite.id, platoon_missing };
            self.ed.flow = Some(Flow { target, warned: false, editor: None });
            self.mode = Mode::PickFileEditor;
            return;
        }
        self.ed.flow = Some(Flow { target, warned: false, editor: Some(favourite) });
        self.editor_continue();
    }

    /// The favourite editor for `v`, if this computer has it; else platoon's own, said once (the setting stays, in
    /// case it comes back).
    fn files_favourite(&mut self) -> FileEditor {
        let id = self.settings.get().files_editor;
        if let Some(e) = fileeditors::by_id(&self.ed.files, &id) {
            return e.clone();
        }
        if self.ed.said_missing.as_deref() != Some(id.as_str()) {
            let line = format!("{} isn't on this computer any more, so platoon's editor opens instead (Settings, Editors).", fileeditors::name_of(&id));
            self.notify(line.clone());
            self.ed.note = Some(line);
            self.ed.said_missing = Some(id);
        }
        FileEditor::platoon()
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
        match (&self.open_project, &self.tk.store) {
            (Some(p), Some(store)) if p == project => store.get(number).cloned(),
            _ => Tickets::open(&self.config, project).get(number).cloned(),
        }
    }

    /// Take the flow one step further: each question or wait returns here once it is answered or done.
    pub(super) fn editor_continue(&mut self) {
        let Some(flow) = self.ed.flow.clone() else { return };
        let chosen = flow.editor.clone().unwrap_or_else(FileEditor::platoon);
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
        if chosen.is_platoon() && !self.editor_installed() {
            if self.ed.installing.is_some() {
                self.notify("The code editor is being installed. It opens when that is done.");
            } else {
                self.ask_install_editor();
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
        match chosen.runs {
            Runs::Platoon => self.ed.request = Some(req),
            Runs::Terminal => {
                let dirs = req.dirs();
                if dirs.len() > 1 {
                    let others: Vec<String> = dirs[1..].iter().map(|d| dirs::tilde(d, &self.home)).collect();
                    let first = dirs::tilde(&dirs[0], &self.home);
                    self.ed.note = Some(format!("{} opened {first} only: it opens one folder at a time. Not opened: {}.", chosen.name, others.join(", ")));
                }
                self.ed.own = Some((chosen, dirs));
            }
            Runs::Window => self.open_in_window(&flow.target, &chosen, &req),
        }
    }

    /// Start an editor with a window of its own on `req`'s folders, beside platoon. A ticket's worktree opened in it
    /// is written down as kept (platoon can't tell when that window closes), until `w` on the ticket.
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
                format!("Opened {opened} of the {n} folders in {}, a window each (it opens at most {}).", e.name, fileeditors::JETBRAINS_MOST)
            }
            (fileeditors::Several::EachWindow, n) if n > 1 => format!("Opened the {n} folders in {}, a window each.", e.name),
            _ => format!("Opened in {}, in a window of its own.", e.name),
        };
        if let EdTarget::Ticket { project, number } = target {
            let mut store = Tickets::open(&self.config, project);
            let mut kept = 0;
            for r in req.roots.iter().take(shown).filter(|r| r.kind == "worktree") {
                if store.mark_opened_in(*number, &r.path, &e.name).is_ok() {
                    kept += 1;
                }
            }
            if kept > 0 {
                self.reread_all_tickets();
                line.push_str(&format!(" #{number}'s worktree stays after the work until w on the ticket removes it."));
            }
        }
        if let Some(n) = self.ed.note.take() {
            line = format!("{n} {line}");
        }
        self.notify(line);
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

    /// Enough of the editor is installed to open it (or a test named a stand-in): by the editor plugin's table, or
    /// (an editor installed before it was a plugin, not taken over yet) by the copy that came with platoon.
    fn editor_installed(&self) -> bool {
        self.ed.stand_in || editorplugin::ready(&self.pl.store, &self.ed.dirs)
    }

    /// The question before installing the editor plugin (or finishing its downloads), with what it downloads.
    pub(super) fn ask_install_editor(&mut self) {
        let plugin = editorplugin::installed(&self.pl.store).is_some();
        // Without the plugin: what the catalog's downloads, else the one built into platoon.
        if !plugin {
            self.pl.ensure_catalog();
        }
        let from_catalog = if plugin { None } else { self.pl.catalog.as_ref().and_then(|c| c.get(editorplugin::ID)).and_then(|e| e.download_bytes) };
        let bytes = from_catalog.unwrap_or_else(|| editorplugin::download_bytes(&self.pl.store));
        self.mode = Mode::Confirm(Confirm::InstallEditor { bytes, plugin });
    }

    /// An editor started by platoon (this window's or another's) has folders open now.
    pub(super) fn ed_open_now(&self) -> bool {
        !editor::open_now(&self.cache).is_empty()
    }

    /// The editor's settings files for this opening (the editor plugin's; see `editorplugin::config_files`).
    pub fn editor_config_files(&self) -> Vec<(String, String)> {
        editorplugin::config_files(&self.pl.store)
    }

    /// An editor installed before it was a plugin becomes the editor plugin (no network, nothing downloaded), said once.
    pub(crate) fn take_over_editor(&mut self) {
        match editorplugin::adopt(&self.pl.store, &self.ed.dirs) {
            Some(Ok(p)) => {
                self.plugins_changed();
                self.notify(editorplugin::adopted_words(&p));
            }
            Some(Err(e)) => self.notify(format!("Your code editor couldn't be made a plugin yet ({e}); it keeps working.")),
            None => {}
        }
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
        self.mode = self.plugins_base();
        self.ed.flow = None;
        self.ed.note = None;
        if installing {
            self.notify("No code editor, then. The settings (s, then Plugins) or `platoon plugin add editor` installs it later.");
        }
    }

    /// Yes to installing it: off the UI thread, with a progress line; it opens once it is there. Without the editor
    /// plugin: the plugin is installed from the catalog first; then whatever its table has that the editor folder
    /// doesn't is downloaded.
    pub(super) fn install_editor(&mut self) {
        self.mode = self.plugins_base();
        self.pl.ensure_catalog();
        let (dirs, store, catalog) = (self.ed.dirs.clone(), self.pl.store.clone(), self.pl.catalog.clone());
        let (tx, rx) = channel();
        thread::spawn(move || {
            let stage = |line: &str| {
                let _ = tx.send(InstallMsg::Stage(line.to_string()));
            };
            let plugin = (|| -> Result<(), String> {
                if editorplugin::installed(&store).is_some() {
                    return Ok(());
                }
                let entry = catalog.as_ref().and_then(|c| c.get(editorplugin::ID)).ok_or("platoon's plugin catalog has no code editor plugin")?;
                if !entry.problems.is_empty() {
                    return Err(format!("{} can't be installed here: {}", entry.manifest.name, entry.problems.join("; ")));
                }
                stage("Installing the code editor: its plugin…");
                let unpacked = entry.unpack()?;
                store.install(unpacked.dir(), Origin::Catalog).map(|_| ())
            })();
            let done = plugin.and_then(|()| {
                let progress = tx.clone();
                let report = move |p| {
                    let _ = progress.send(InstallMsg::Progress(p));
                };
                editorplugin::install_downloads(&store, &dirs, &report)
            });
            let _ = tx.send(InstallMsg::Done(done));
        });
        self.ed.installing = Some(rx);
        self.ed.progress.clear();
        self.ed.stage = None;
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
        match (&self.ed.stage, parts.is_empty()) {
            (_, false) => format!("Installing the code editor: {}", parts.join("  ")),
            (Some(stage), true) => stage.clone(),
            (None, true) => "Installing the code editor…".into(),
        }
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
                Ok(InstallMsg::Stage(line)) => self.ed.stage = Some(line),
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
        // The plugin may be new: the plugin list shows it.
        self.plugins_changed();
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
                self.notify(format!("The code editor didn't finish installing: {e} v, or `platoon plugin add editor`, tries again."));
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

    /// A terminal editor of the user's own is ready to open on these folders (it opens the first): the event loop
    /// hands it the terminal as it does platoon's.
    pub fn take_own_editor_request(&mut self) -> Option<(FileEditor, Vec<String>)> {
        self.ed.own.take()
    }

    pub fn editor_dirs(&self) -> &Dirs {
        &self.ed.dirs
    }

    /// The editor has closed (or couldn't start): say what went wrong, if anything, and look at everything again
    /// (sessions may have moved on, a ticket may be done).
    pub fn editor_closed(&mut self, result: Result<(), String>) {
        let note = self.ed.note.take();
        match (result, note) {
            (Err(e), _) => self.notify(format!("The code editor didn't open: {e}")),
            (Ok(()), Some(n)) => self.notify(n),
            (Ok(()), None) => {}
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
