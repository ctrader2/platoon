//! Projects inside the app: the top-level tabs, the Projects tab, a project's screen, the project picker (`m`),
//! the boxes for a project's name, color and badge, and what archiving, restoring and deleting a project does to
//! its sessions. The data itself is in `projects.rs`.

use super::*;
use crate::projects::{Project, SLOTS};
use std::fs;

/// The top-level tabs, on `1` to `4`.
pub const TOPS: &[&str] = &["Sessions", "Tasks", "Projects", "Squads"];
pub const TOP_SESSIONS: usize = 0;
pub const TOP_TASKS: usize = 1;
pub const TOP_PROJECTS: usize = 2;
pub const TOP_SQUADS: usize = 3;

/// Active and Archived on the Projects tab: the label and what the tab says when it has nothing in it.
pub const PTABS: &[(&str, &str)] =
    &[("Active", "No projects yet. Press n to make one."), ("Archived", "No archived projects. Press a on a project to archive it.")];

/// The two panes of a project's screen: Sessions on the left, Notes on the right.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Sessions,
    Notes,
}

/// What the name box is for.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum NamePurpose {
    New,
    /// A new project, with this session put in it (from the project picker).
    NewFor(String),
    Rename(String),
    /// A new task made on the Tasks tab, in this project (`NO_PROJECT_ID`: in none): its title.
    NewTask(String),
    /// A new squad (`n` on the Squads tab): its name.
    NewSquad,
    /// What the Tasks tab is narrowed to.
    TaskFilter,
}

pub struct NameBox {
    pub purpose: NamePurpose,
    pub input: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AssignOpt {
    Project(String),
    New,
    None,
}

/// The project picker of `m`: the active projects, then "New project…" and "No project".
pub struct AssignPick {
    pub session: String,
    pub session_name: String,
    pub options: Vec<AssignOpt>,
    pub selected: usize,
}

/// The color and badge box.
pub struct LookEdit {
    pub project: String,
    /// Index into `SLOTS`.
    pub slot: usize,
    pub badge: String,
}

/// What the Projects tab shows about a project.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct PStat {
    pub active: usize,
    pub archived: usize,
    /// The latest activity of any of its sessions, ms since the epoch (0: none).
    pub last_active: u64,
    /// Its non-archived sessions with a reply the user hasn't seen.
    pub unread: usize,
    /// Its tasks that aren't Done or Cancelled.
    pub open_tickets: usize,
}

impl App {
    /// The Projects tab's list is showing (not a project's screen).
    pub fn projects_list_showing(&self) -> bool {
        self.top == TOP_PROJECTS && self.open_project.is_none()
    }

    /// The project whose screen is showing.
    pub fn open_project(&self) -> Option<&Project> {
        self.open_project.as_deref().and_then(|p| self.projects.get(p))
    }

    /// A search only looks at the sessions of the project whose screen is showing.
    pub(super) fn in_scope(&self, s: &Session) -> bool {
        match &self.open_project {
            Some(p) => self.projects.project_id_of(&s.id) == Some(p.as_str()),
            None => true,
        }
    }

    pub(super) fn project_stats(&self) -> HashMap<String, PStat> {
        let mut out: HashMap<String, PStat> = self.projects.all().iter().map(|p| (p.id.clone(), PStat::default())).collect();
        for s in &self.sessions {
            if self.stopping.contains_key(&s.id) {
                continue;
            }
            let Some(st) = self.projects.project_id_of(&s.id).and_then(|p| out.get_mut(p)) else { continue };
            if s.archived {
                st.archived += 1;
            } else {
                st.active += 1;
                st.unread += usize::from(s.unread);
            }
            st.last_active = st.last_active.max(s.last_active);
        }
        // One small read per project: a missing or corrupt tasks file is simply no tasks, and nothing is written.
        for p in self.projects.all() {
            let store = crate::tickets::Tickets::open(&self.config, &p.id);
            let all = store.all();
            if let Some(st) = out.get_mut(&p.id) {
                st.open_tickets = all.iter().filter(|t| !t.state.closed()).count();
            }
        }
        out
    }

    /// Work the Projects tab's numbers out again from what is in memory, without re-reading Claude's state (which
    /// would put each session's unread mark back to what its processes say).
    #[cfg(test)]
    pub fn restat_for_test(&mut self) {
        self.rev += 1;
        self.update_view();
    }

    /// The rows of the Projects tab: its Active or Archived projects, narrowed by the filter, the most recently used
    /// first (then the newest).
    pub(super) fn update_project_rows(&mut self, q: &str) {
        let all = self.projects.all();
        let archived = self.ptab == 1;
        self.pcounts = [all.iter().filter(|p| !p.archived).count(), all.iter().filter(|p| p.archived).count()];
        let filtering = self.top == TOP_PROJECTS && self.open_project.is_none() && !q.is_empty();
        let mut rows: Vec<usize> = (0..all.len())
            .filter(|&i| all[i].archived == archived)
            .filter(|&i| !filtering || all[i].name.to_lowercase().contains(q) || all[i].badge.to_lowercase().contains(q))
            .collect();
        let last = |i: usize| self.pstats.get(&all[i].id).map_or(0, |s| s.last_active);
        rows.sort_by(|&a, &b| last(b).cmp(&last(a)).then(all[b].created.cmp(&all[a].created)).then(all[a].id.cmp(&all[b].id)));
        self.pview = rows;
    }

    /// Indices into `projects.all()` of the Projects tab's rows.
    pub fn project_rows(&self) -> &[usize] {
        &self.pview
    }

    pub fn selected_project(&self) -> Option<&Project> {
        self.pview.get(self.pselected).map(|&i| &self.projects.all()[i])
    }

    /// `1` to `4`: a top-level tab. From a project's screen, `3` goes back to the Projects tab's list; from a task
    /// opened on the Tasks tab, `2` goes back to the board; from a squad opened on the Squads tab, `4` to its list.
    pub fn switch_top(&mut self, i: usize) {
        if i >= TOPS.len() {
            return;
        }
        if self.open_project.is_some() {
            self.close_project_screen();
        } else if self.board_showing() && self.tk.open.is_some() {
            self.close_ticket_screen();
        } else if self.squads_showing() && self.sqt.open {
            self.close_squad();
        } else if i == self.top {
            return;
        }
        self.top = i;
        self.filter.clear();
        if self.board_showing() {
            self.sync_board_store();
        }
        if self.squads_showing() {
            self.reload_squads(None);
        }
        self.update_view();
    }

    /// Left and right on the Projects tab: Active or Archived.
    pub fn switch_ptab(&mut self, i: usize) {
        if i < PTABS.len() && i != self.ptab {
            self.ptab = i;
            self.pselected = 0;
            self.update_view();
        }
    }

    pub(super) fn move_project(&mut self, delta: i32) {
        let last = self.pview.len().saturating_sub(1) as i32;
        self.pselected = (self.pselected as i32 + delta).clamp(0, last) as usize;
    }

    pub(super) fn open_project_screen(&mut self) {
        let Some(p) = self.selected_project().map(|p| p.id.clone()) else {
            self.notify("No project selected");
            return;
        };
        self.open_project = Some(p.clone());
        self.filter.clear();
        self.selected = 0;
        self.pane = Pane::Sessions;
        self.notes_top.set(0);
        self.update_view();
        self.poll_notes_status();
    }

    pub(super) fn close_project_screen(&mut self) {
        let Some(id) = self.open_project.take() else { return };
        self.filter.clear();
        self.selected = 0;
        self.update_view();
        // Back on the row of the project that was open.
        if let Some(pos) = self.pview.iter().position(|&i| self.projects.all()[i].id == id) {
            self.pselected = pos;
        } else if let Some(p) = self.projects.get(&id) {
            // It was archived or restored meanwhile: to the tab it is on now.
            self.ptab = usize::from(p.archived);
            self.update_view();
            if let Some(pos) = self.pview.iter().position(|&i| self.projects.all()[i].id == id) {
                self.pselected = pos;
            }
        }
    }

    /// The folders the project's sessions ran in, most recently used first, those that still exist.
    pub(super) fn project_folders(&self, project: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for s in self.sessions.iter().filter(|s| self.projects.project_id_of(&s.id) == Some(project)) {
            if !s.cwd.is_empty() && !out.contains(&s.cwd) && Path::new(&s.cwd).is_dir() {
                out.push(s.cwd.clone());
            }
        }
        out
    }

    /// The text of the open project's notes: None when there is no file (or it can't be read).
    pub fn open_notes(&self) -> Option<String> {
        let p = self.open_project.as_deref()?;
        fs::read_to_string(self.projects.notes_path(p)).ok()
    }

    /// Where the open project's notes live, home-abbreviated.
    pub fn open_notes_path(&self) -> String {
        self.open_project.as_deref().map(|p| dirs::tilde(&self.projects.notes_path(p).to_string_lossy(), &self.home)).unwrap_or_default()
    }

    // ---- keys -------------------------------------------------------------------------------------------------

    pub(super) fn list_key(&mut self, key: KeyEvent) {
        // The quit key quits from the top-level lists only; on a project's or a task's screen it goes back one level,
        // as esc does.
        let key = match self.keys.get().quit.matches(&key) {
            true if self.at_top_level() => return self.act(Action::Quit),
            true => KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            false => key,
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char('c') if ctrl => return self.act(Action::Quit),
            KeyCode::Char('/') if self.slash_searches() => return self.act(Action::Search),
            KeyCode::Char('s') if !ctrl && self.s_opens_settings() => return self.act(Action::Settings),
            // The code editor, from anywhere: on what is selected, or on a folder the picker asks for.
            KeyCode::Char('v') if !ctrl && !alt => return self.act(Action::Editor),
            // 1 is the first top-level tab, 2 the second; a number with no tab does nothing.
            KeyCode::Char(c @ '1'..='9') if !ctrl && !alt => return self.switch_top(c as usize - '1' as usize),
            _ => {}
        }
        if self.task_screen_showing() {
            self.ticket_screen_key(key);
        } else if self.open_project.is_some() {
            self.project_screen_key(key);
        } else if self.top == TOP_TASKS {
            self.board_key(key);
        } else if self.top == TOP_PROJECTS {
            self.projects_key(key);
        } else if self.top == TOP_SQUADS {
            self.squads_key(key);
        } else {
            self.sessions_key(key);
        }
    }

    /// One of the top-level lists (Sessions, Tasks, Projects, Squads) with nothing open over it: where quit quits.
    pub fn at_top_level(&self) -> bool {
        self.open_project.is_none() && !(self.top == TOP_TASKS && self.tk.open.is_some()) && !(self.top == TOP_SQUADS && self.sqt.open)
    }

    fn sessions_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('n') => self.act(Action::New),
            KeyCode::Char('a') => self.act(Action::Archive),
            KeyCode::Char('r') => self.act(Action::Rename),
            KeyCode::Char('m') => self.act(Action::Assign),
            KeyCode::Char('h') => self.act(Action::HandOff),
            KeyCode::Char('t') => self.draft_tasks_key(),
            KeyCode::Enter => self.act(Action::Jump),
            KeyCode::Esc => {
                self.filter.clear();
                self.selected = 0;
            }
            KeyCode::Left => self.switch_tab(0),
            KeyCode::Right => self.switch_tab(1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::PageDown => self.move_selection(super::page_of(&self.pages.sessions)),
            KeyCode::PageUp => self.move_selection(-super::page_of(&self.pages.sessions)),
            KeyCode::Char('g') | KeyCode::Home => self.selected = 0,
            KeyCode::Char('G') | KeyCode::End => self.selected = self.view.len().saturating_sub(1),
            _ => {}
        }
    }

    fn projects_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('n') => self.act(Action::New),
            KeyCode::Char('a') => self.act(Action::Archive),
            KeyCode::Char('r') => self.act(Action::Rename),
            KeyCode::Char('d') => self.act(Action::Delete),
            KeyCode::Char('c') => self.act(Action::Look),
            KeyCode::Enter => self.act(Action::Jump),
            KeyCode::Esc => {
                self.filter.clear();
                self.pselected = 0;
            }
            KeyCode::Left => self.switch_ptab(0),
            KeyCode::Right => self.switch_ptab(1),
            KeyCode::Down | KeyCode::Char('j') => self.move_project(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_project(-1),
            KeyCode::PageDown => self.move_project(super::page_of(&self.pages.projects)),
            KeyCode::PageUp => self.move_project(-super::page_of(&self.pages.projects)),
            KeyCode::Char('g') | KeyCode::Home => self.pselected = 0,
            KeyCode::Char('G') | KeyCode::End => self.pselected = self.pview.len().saturating_sub(1),
            _ => {}
        }
    }

    /// `/` opens the search on the screen showing: everywhere on platoon's own screens but the Tasks tab (where it
    /// narrows the list of tasks) and the Squads tab (no sessions there to find).
    pub fn slash_searches(&self) -> bool {
        matches!(self.mode, Mode::List) && !self.board_showing() && !self.squads_showing()
    }

    /// `s` opens the settings on the screen showing. On the Tasks tab it starts the picked task instead.
    pub fn s_opens_settings(&self) -> bool {
        matches!(self.mode, Mode::List) && !self.ticket_has_keys()
    }

    /// The Tasks tab is showing: `s` there starts a task, it is not the settings.
    pub(crate) fn ticket_has_keys(&self) -> bool {
        self.board_showing()
    }

    fn project_screen_key(&mut self, key: KeyEvent) {
        // Plain arrows move inside a pane. With Shift or Ctrl (both, because some terminals keep one of them for
        // themselves) they move between the panes: left to the sessions, right to the notes.
        let across = key.modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Left if across => self.pane = Pane::Sessions,
            KeyCode::Right if across => self.pane = Pane::Notes,
            KeyCode::Left | KeyCode::Right => {}
            KeyCode::Tab | KeyCode::BackTab => {
                self.pane = if self.pane == Pane::Sessions { Pane::Notes } else { Pane::Sessions };
            }
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.clear();
                self.selected = 0;
            }
            KeyCode::Esc => self.act(Action::Leave),
            KeyCode::Char('n') => self.act(Action::New),
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown | KeyCode::Char('j' | 'k') if self.pane == Pane::Notes => {
                let d: i64 = match key.code {
                    KeyCode::Up | KeyCode::Char('k') => -1,
                    KeyCode::Down | KeyCode::Char('j') => 1,
                    KeyCode::PageUp => -10,
                    _ => 10,
                };
                self.notes_top.set((self.notes_top.get() as i64 + d).max(0) as usize);
            }
            KeyCode::Home if self.pane == Pane::Notes => self.notes_top.set(0),
            // The notes' own keys: edit them, see what changed, undo it.
            KeyCode::Char('e') if self.pane == Pane::Notes => self.act(Action::Edit),
            KeyCode::Char('c') if self.pane == Pane::Notes => self.act(Action::ShowChanges),
            KeyCode::Char('u') if self.pane == Pane::Notes => self.act(Action::Undo),
            // Everything else is about the selected session, as on the Sessions tab.
            _ if self.pane == Pane::Sessions => match key.code {
                KeyCode::Char('a') => self.act(Action::Archive),
                KeyCode::Char('r') => self.act(Action::Rename),
                KeyCode::Char('m') => self.act(Action::Assign),
                KeyCode::Char('h') => self.act(Action::HandOff),
                KeyCode::Char('t') => self.draft_tasks_key(),
                KeyCode::Enter => self.act(Action::Jump),
                KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
                KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
                KeyCode::PageDown => self.move_selection(super::page_of(&self.pages.sessions)),
                KeyCode::PageUp => self.move_selection(-super::page_of(&self.pages.sessions)),
                KeyCode::Char('g') | KeyCode::Home => self.selected = 0,
                KeyCode::Char('G') | KeyCode::End => self.selected = self.view.len().saturating_sub(1),
                _ => {}
            },
            _ => {}
        }
    }

    pub(super) fn wheel_list(&mut self, up: bool) {
        let d = if up { -1 } else { 1 };
        if self.board_showing() && self.tk.open.is_some() {
            self.wheel_ticket(up);
        } else if self.board_showing() {
            self.board_key(KeyEvent::new(if up { KeyCode::Up } else { KeyCode::Down }, KeyModifiers::NONE));
        } else if self.projects_list_showing() {
            self.move_project(d);
        } else if self.squads_showing() {
            self.move_squads(if self.sqt.open { d * 3 } else { d });
        } else if self.open_project.is_some() && self.pane == Pane::Notes {
            self.notes_top.set((self.notes_top.get() as i32 + d * 3).max(0) as usize);
        } else {
            self.move_selection(d);
        }
    }

    // ---- the name box -----------------------------------------------------------------------------------------

    pub(super) fn open_name_box(&mut self, purpose: NamePurpose) {
        let input = match &purpose {
            NamePurpose::Rename(id) => self.projects.get(id).map(|p| p.name.clone()).unwrap_or_default(),
            NamePurpose::TaskFilter => self.filter.clone(),
            _ => String::new(),
        };
        self.name_box = NameBox { purpose, input };
        self.mode = Mode::ProjectName;
    }

    pub(super) fn open_project_rename(&mut self) {
        match self.selected_project().map(|p| p.id.clone()) {
            Some(id) => self.open_name_box(NamePurpose::Rename(id)),
            None => self.notify("No project selected"),
        }
    }

    pub(super) fn type_project_name(&mut self, text: &str) {
        let max = match &self.name_box.purpose {
            NamePurpose::NewTask(_) => crate::tickets::TITLE_MAX,
            NamePurpose::TaskFilter => 200,
            _ => projects_mod::NAME_MAX,
        };
        let room = max.saturating_sub(self.name_box.input.chars().count());
        self.name_box.input.extend(text.chars().take(room));
    }

    /// Enter in the name box.
    pub(super) fn save_name_box(&mut self) {
        match self.name_box.purpose.clone() {
            NamePurpose::TaskFilter => {
                self.filter = self.name_box.input.trim().to_string();
                self.mode = Mode::List;
                return;
            }
            NamePurpose::NewSquad => return self.save_new_squad(),
            NamePurpose::NewTask(project) => {
                let title = crate::tickets::clean(&self.name_box.input, crate::tickets::TITLE_MAX);
                if title.is_empty() {
                    self.notify("A task needs a title");
                    return; // the box stays open
                }
                self.mode = Mode::List;
                return self.make_board_task(&project, &title);
            }
            _ => {}
        }
        let name = projects_mod::clean(&self.name_box.input, projects_mod::NAME_MAX);
        if name.is_empty() {
            self.notify("A project needs a name");
            return; // the box stays open
        }
        self.mode = Mode::List;
        let purpose = self.name_box.purpose.clone();
        match purpose {
            NamePurpose::Rename(id) => match self.projects.rename(&id, &name) {
                Ok(()) => self.notify(format!("Renamed the project to \"{name}\"")),
                Err(e) => self.report_projects_error(e),
            },
            NamePurpose::NewTask(_) | NamePurpose::TaskFilter | NamePurpose::NewSquad => {}
            NamePurpose::New | NamePurpose::NewFor(_) => match self.projects.create(&name) {
                Ok(id) => {
                    let mut msg = format!("Made the project \"{name}\"");
                    if let NamePurpose::NewFor(session) = &purpose {
                        match self.projects.assign(session, Some(&id)) {
                            Ok(()) => {
                                let s = session.clone();
                                self.joined_project(&id, &s);
                                msg = format!("Made the project \"{name}\" and put the session in it");
                            }
                            Err(e) => return self.report_projects_error(e),
                        }
                    } else {
                        // Show it: the Active projects, with the new one selected.
                        self.ptab = 0;
                        self.filter.clear();
                        self.rev += 1;
                        self.update_view();
                        if let Some(pos) = self.pview.iter().position(|&i| self.projects.all()[i].id == id) {
                            self.pselected = pos;
                        }
                    }
                    // Said once: without the hook, the notes never reach a session.
                    let hooks_missing = !setup::notes_hook_installed(self.reader.root()) || !setup::stop_hook_installed(self.reader.root());
                    if hooks_missing && !self.projects.hook_told() {
                        let _ = self.projects.set_hook_told();
                        msg.push_str(
                            ". To hand a project's notes to its sessions, and keep them up to date, run `platoon setup` once more.",
                        );
                        self.notice = Some((msg, Instant::now(), AUTO_ARCHIVE_NOTICE_FOR, None));
                    } else {
                        self.notify(msg);
                    }
                }
                Err(e) => self.report_projects_error(e),
            },
        }
        self.reload();
    }

    /// A session has just joined a project: tell the notes helper, so an update runs a few minutes later without
    /// the user having to resume anything. It never blocks (the helper is a detached process of its own) and it
    /// does nothing at all when the feature is off, the project is archived or there are no notes yet.
    pub(super) fn joined_project(&self, project: &str, session: &str) {
        crate::notesrun::mark_joined(&self.config, &self.cache, project, session, crate::notesrun::now_ms());
    }

    fn report_projects_error(&mut self, e: io::Error) {
        let why = e.to_string();
        self.notify(format!("Couldn't save the projects: {}", why.split(" (os error").next().unwrap_or(&why)));
    }

    // ---- the project picker (`m`) -----------------------------------------------------------------------------

    pub(super) fn open_assign(&mut self) {
        let Some(s) = self.selected_session() else {
            self.notify("No session selected");
            return;
        };
        let (session, session_name) = (s.id.clone(), s.name.clone());
        let current = self.projects.project_id_of(&session).map(String::from);
        let mut active: Vec<&Project> = self.projects.all().iter().filter(|p| !p.archived).collect();
        active.sort_by_key(|p| p.name.to_lowercase());
        let mut options: Vec<AssignOpt> = active.iter().map(|p| AssignOpt::Project(p.id.clone())).collect();
        options.push(AssignOpt::New);
        options.push(AssignOpt::None);
        let selected = options.iter().position(|o| matches!((o, &current), (AssignOpt::Project(p), Some(c)) if p == c)).unwrap_or(0);
        self.assign = AssignPick { session, session_name, options, selected };
        self.mode = Mode::Assign;
    }

    pub(super) fn move_assign(&mut self, delta: i32) {
        let last = self.assign.options.len().saturating_sub(1) as i32;
        self.assign.selected = (self.assign.selected as i32 + delta).clamp(0, last) as usize;
    }

    pub(super) fn take_assign(&mut self) {
        let Some(choice) = self.assign.options.get(self.assign.selected).cloned() else { return };
        let (session, name) = (self.assign.session.clone(), self.assign.session_name.clone());
        self.mode = Mode::List;
        let current = self.projects.project_id_of(&session).map(String::from);
        match choice {
            AssignOpt::New => return self.open_name_box(NamePurpose::NewFor(session)),
            AssignOpt::Project(p) if current.as_deref() == Some(p.as_str()) => return,
            AssignOpt::Project(p) => {
                let pname = self.projects.get(&p).map(|x| x.name.clone()).unwrap_or_default();
                match self.projects.assign(&session, Some(&p)) {
                    Ok(()) => {
                        // Nothing ends a turn when a session is added by hand, so the notes are told here instead.
                        self.joined_project(&p, &session);
                        self.notify(format!("Put \"{name}\" in the project \"{pname}\""));
                    }
                    Err(e) => self.report_projects_error(e),
                }
            }
            AssignOpt::None if current.is_none() => return,
            AssignOpt::None => match self.projects.assign(&session, None) {
                Ok(()) => self.notify(format!("\"{name}\" is in no project now")),
                Err(e) => self.report_projects_error(e),
            },
        }
        self.reload();
    }

    // ---- color and badge --------------------------------------------------------------------------------------

    pub(super) fn open_look(&mut self) {
        let Some(p) = self.selected_project() else {
            self.notify("No project selected");
            return;
        };
        let slot = SLOTS.iter().position(|(s, _)| *s == p.color).unwrap_or(0);
        self.look = LookEdit { project: p.id.clone(), slot, badge: p.badge.clone() };
        self.mode = Mode::Look;
    }

    pub(super) fn step_slot(&mut self, delta: i32) {
        self.look.slot = (self.look.slot as i32 + delta).rem_euclid(SLOTS.len() as i32) as usize;
    }

    pub(super) fn type_badge(&mut self, text: &str) {
        let add = projects_mod::clean_badge(text);
        let room = projects_mod::BADGE_MAX.saturating_sub(self.look.badge.chars().count());
        self.look.badge.extend(add.chars().take(room));
    }

    pub(super) fn save_look(&mut self) {
        self.mode = Mode::List;
        let (id, color, badge) = (self.look.project.clone(), SLOTS[self.look.slot].0, self.look.badge.clone());
        if let Err(e) = self.projects.set_look(&id, color, &badge) {
            self.report_projects_error(e);
        }
        self.reload();
    }

    // ---- archive, restore, delete -----------------------------------------------------------------------------

    /// The active sessions of a project that archiving it would archive (never a background one: Claude's own
    /// service runs it, and platoon doesn't archive those).
    fn archivable_members(&self, project: &str) -> Vec<&Session> {
        self.sessions
            .iter()
            .filter(|s| self.projects.project_id_of(&s.id) == Some(project))
            .filter(|s| !s.archived && !self.stopping.contains_key(&s.id) && s.status != Status::Background)
            .collect()
    }

    pub(super) fn toggle_project_archive(&mut self) {
        let Some(p) = self.selected_project().cloned() else {
            self.notify("No project selected");
            return;
        };
        if p.archived {
            return self.restore_project(&p.id);
        }
        let members = self.archivable_members(&p.id);
        let running: Vec<&&Session> = members.iter().filter(|s| !s.procs.is_empty()).collect();
        let busy = running.iter().any(|s| s.status == Status::Busy);
        let outside = running.iter().any(|s| s.procs.iter().any(|p| !p.hosted));
        if busy || outside {
            self.mode =
                Mode::Confirm(Confirm::ArchiveProject { id: p.id.clone(), name: p.name.clone(), running: running.len(), busy, outside });
        } else {
            self.archive_project(&p.id);
        }
    }

    /// Archive the project and its active sessions: the resting ones at once (one write), the running ones the way
    /// the archive key does it (stopped on a worker thread, archived once their process has exited).
    pub(super) fn archive_project(&mut self, id: &str) {
        let Some(name) = self.projects.get(id).map(|p| p.name.clone()) else { return };
        let members: Vec<(String, String, bool)> =
            self.archivable_members(id).iter().map(|s| (s.id.clone(), s.name.clone(), !s.procs.is_empty())).collect();
        let ids: Vec<String> = members.iter().map(|m| m.0.clone()).collect();
        if let Err(e) = self.projects.archive(id, &ids) {
            return self.report_projects_error(e);
        }
        let resting: Vec<String> = members.iter().filter(|m| !m.2).map(|m| m.0.clone()).collect();
        if !resting.is_empty()
            && let Err(e) = self.archive.archive_many(&resting, usage::now())
        {
            self.notify(format!("Couldn't save the archive: {e}"));
        }
        for (sid, sname, _) in members.iter().filter(|m| m.2) {
            self.archive_conversation(sid, sname);
        }
        let n = ids.len();
        self.notify(format!("Archived the project \"{name}\" and {n} session{}", if n == 1 { "" } else { "s" }));
        self.reload();
    }

    /// Restore the project and exactly the sessions its archiving archived; the others stay archived.
    pub(super) fn restore_project(&mut self, id: &str) {
        let Some(name) = self.projects.get(id).map(|p| p.name.clone()) else { return };
        let back = match self.projects.restore(id) {
            Ok(b) => b,
            Err(e) => return self.report_projects_error(e),
        };
        let back: Vec<String> = back.into_iter().filter(|s| self.archive.contains(s)).collect();
        if !back.is_empty()
            && let Err(e) = self.archive.restore_many(&back, usage::now())
        {
            self.notify(format!("Couldn't save the archive: {e}"));
        }
        let n = back.len();
        self.notify(format!("Restored the project \"{name}\" and {n} session{}", if n == 1 { "" } else { "s" }));
        self.reload();
    }

    pub(super) fn ask_delete_project(&mut self) {
        let Some(p) = self.selected_project() else {
            self.notify("No project selected");
            return;
        };
        let sessions = self.projects.members_of(&p.id).len();
        self.mode = Mode::Confirm(Confirm::DeleteProject { id: p.id.clone(), name: p.name.clone(), sessions });
    }

    pub(super) fn delete_project(&mut self, id: &str) {
        let Some(name) = self.projects.get(id).map(|p| p.name.clone()) else { return };
        match self.projects.delete(id, &self.cache) {
            Ok(()) => self.notify(format!("Deleted the project \"{name}\". Its sessions are still in the Sessions tab.")),
            Err(e) => self.report_projects_error(e),
        }
        if self.open_project.as_deref() == Some(id) {
            self.open_project = None;
        }
        self.reload();
    }
}
