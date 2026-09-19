use crate::claude::{pid_alive, Reader};
use crate::client::{key_bytes, Client};
use crate::dirs;
use crate::guard;
use crate::setup;
use crate::host::{self, HostInfo};
use crate::model::{Session, Status};
use crate::usage::{self, Usage};
use crate::theme::Theme;
use fuzzy_matcher::{skim::SkimMatcherV2, FuzzyMatcher};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const NOTICE_FOR: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Jump,
    New,
    Kill,
    Filter,
    Quit,
    Back,
    Confirm,
    Cancel,
}

/// Clickable regions, recorded by the UI each frame.
#[derive(Clone, Copy, Debug)]
pub enum Hit {
    Row(usize),
    PickerRow(usize),
    Action(Action),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AdoptKind {
    /// One copy, running in another terminal.
    TakeOver,
    /// Several copies, or a copy that fell out of sync: end them all and reopen from the saved history.
    Reload,
}

/// What pressing Enter on a row should do.
#[derive(Debug, PartialEq, Eq)]
pub enum JumpPlan {
    Attach(u32),
    Adopt { pids: Vec<u32>, kind: AdoptKind, busy: bool },
}

#[derive(Clone)]
pub enum Confirm {
    Kill { pid: u32, name: String, dir: String },
    /// Take over a session started elsewhere: end it, then resume it inside platoon.
    Adopt { pids: Vec<u32>, id: String, name: String, cwd: String, busy: bool, kind: AdoptKind },
}

#[derive(Clone)]
pub enum Mode {
    List,
    /// Typing into the filter box.
    Filter,
    Picker,
    Confirm(Confirm),
    /// Full-screen view of a platoon-hosted session, by claude's pid.
    Attached(u32),
}

pub struct Picker {
    pub input: String,
    all: Vec<String>,
    /// Absolute paths, best match first.
    pub results: Vec<String>,
    pub selected: usize,
}

impl Picker {
    fn update(&mut self, home: &str) {
        let q = self.input.trim();
        let mut res: Vec<String> = Vec::new();
        if let Some(p) = dirs::expand(q, home) {
            if Path::new(&p).is_dir() {
                res.push(p);
            }
        }
        if q.is_empty() {
            res.extend(self.all.iter().take(200).cloned());
        } else {
            let m = SkimMatcherV2::default();
            let mut scored: Vec<(i64, &String)> = self
                .all
                .iter()
                .filter_map(|p| m.fuzzy_match(&dirs::tilde(p, home), q).map(|s| (s, p)))
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0));
            res.extend(scored.into_iter().take(200).map(|(_, p)| p.clone()));
        }
        let mut seen = std::collections::HashSet::new();
        res.retain(|p| seen.insert(p.clone()));
        self.results = res;
        self.selected = 0;
    }
}

pub struct App {
    pub sessions: Vec<Session>,
    pub usage: Usage,
    /// Index into `visible()`.
    pub selected: usize,
    pub filter: String,
    pub mode: Mode,
    pub picker: Picker,
    /// Sessions running under a platoon host. They outlive platoon.
    pub hosts: Vec<HostInfo>,
    /// Connection to the session being viewed, if any.
    pub client: Option<Client>,
    /// Host processes we started, kept only so they get reaped when they exit.
    spawned: Vec<std::process::Child>,
    pub notice: Option<(String, Instant)>,
    pub mouse: (u16, u16),
    pub home: String,
    /// Where platoon keeps its own state (`~/.cache/platoon`).
    pub cache: PathBuf,
    /// Animation frame counter, ~10 per second.
    pub tick: u64,
    pub theme: Theme,
    pub quit: bool,
    pub hits: RefCell<Vec<(Rect, Hit)>>,
    size: (u16, u16),
    reader: Reader,
}

impl App {
    pub fn new() -> Self {
        Self::with_reader(Reader::new(), usage::cache_dir())
    }

    /// `cache` is where platoon keeps its own state; tests pass a temp dir so they never touch the real one.
    pub fn with_reader(reader: Reader, cache: PathBuf) -> Self {
        let mut app = Self {
            sessions: Vec::new(),
            usage: Usage::NotSetUp,
            selected: 0,
            filter: String::new(),
            mode: Mode::List,
            picker: Picker { input: String::new(), all: Vec::new(), results: Vec::new(), selected: 0 },
            hosts: Vec::new(),
            client: None,
            spawned: Vec::new(),
            notice: None,
            mouse: (0, 0),
            home: std::env::var("HOME").unwrap_or_default(),
            cache,
            tick: 0,
            theme: Theme::catppuccin_mocha(),
            quit: false,
            hits: RefCell::new(Vec::new()),
            size: (80, 24),
            reader,
        };
        app.refresh();
        app
    }

    pub fn notify(&mut self, msg: impl Into<String>) {
        self.notice = Some((msg.into(), Instant::now()));
    }

    pub fn active_notice(&self) -> Option<&str> {
        self.notice.as_ref().filter(|(_, t)| t.elapsed() < NOTICE_FOR).map(|(m, _)| m.as_str())
    }

    /// Indices into `sessions` that match the filter.
    pub fn visible(&self) -> Vec<usize> {
        let q = self.filter.to_lowercase();
        self.sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                q.is_empty()
                    || [s.name.as_str(), s.dir.as_str(), s.branch.as_deref().unwrap_or(""), s.description.as_str()]
                        .iter()
                        .any(|f| f.to_lowercase().contains(&q))
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn selected_session(&self) -> Option<&Session> {
        self.visible().get(self.selected).map(|&i| &self.sessions[i])
    }

    fn clamp_selected(&mut self) {
        self.selected = self.selected.min(self.visible().len().saturating_sub(1));
    }

    pub fn unread_count(&self) -> usize {
        self.sessions.iter().filter(|s| s.unread).count()
    }

    /// (rows, cols) of the terminal area inside the attached view.
    fn pty_size(&self) -> (u16, u16) {
        (self.size.1.saturating_sub(3).max(1), self.size.0.saturating_sub(2).max(1))
    }

    pub fn sync_size(&mut self, w: u16, h: u16) {
        if self.size == (w, h) {
            return;
        }
        self.size = (w, h);
        let (rows, cols) = self.pty_size();
        if let Some(c) = &mut self.client {
            c.resize(rows, cols);
        }
    }

    /// Re-read Claude Code's state, carrying unread flags and the selection across.
    pub fn refresh(&mut self) {
        self.spawned.retain_mut(|c| c.try_wait().ok().flatten().is_none());
        let guard_on = setup::guard_installed(self.reader.root());
        self.refresh_with(host::discover(), guard_on);
    }

    /// `refresh` with its outside inputs supplied: which sessions platoon hosts, and whether the
    /// guard hook is installed (so a second copy of a session gets refused at the source).
    pub fn refresh_with(&mut self, hosts: Vec<HostInfo>, guard_on: bool) {
        let attached = if let Mode::Attached(p) = self.mode { Some(p) } else { None };
        self.hosts = hosts;
        if let Some(pid) = attached {
            if self.client.as_ref().map_or(true, |c| c.closed()) || !self.hosts.iter().any(|h| h.claude_pid == pid) {
                self.mode = Mode::List;
                self.client = None;
                self.notify("Session ended");
            }
        }

        // Rows are identified by pid, not session id: resuming a conversation elsewhere while it's
        // still running gives two live processes the same session id.
        let selected_pid = self.selected_session().map(|s| s.pid);
        let mut fresh = self.reader.load();
        for s in &mut fresh {
            // Unread = Claude went busy -> idle while we were watching. Starting work again clears it.
            // Sessions already idle when first seen count as read; so do ones we're looking at.
            if let Some(old) = self.sessions.iter().find(|o| o.pid == s.pid) {
                s.unread = s.status == Status::Idle && (old.unread || old.status == Status::Busy);
            }
            if attached == Some(s.pid) {
                s.unread = false;
            }
            s.external = !self.hosts.iter().any(|h| h.claude_pid == s.pid);
        }

        // Two live processes on one conversation each hold their own memory of it and write to the same
        // history. With the guard installed the later copy is refused at its first prompt; without it,
        // all platoon can do is stop writing through its own copy, and require a reload from the history.
        let mut copies: HashMap<String, usize> = HashMap::new();
        for s in &fresh {
            *copies.entry(s.id.clone()).or_default() += 1;
        }
        guard::prune_stale(&self.cache);
        for s in &mut fresh {
            s.shared = copies[&s.id] > 1;
            if s.shared && !s.external && !guard_on {
                guard::mark_stale(&self.cache, s.pid, s.started_at);
            }
            s.stale = guard::is_stale(&self.cache, s.pid, s.started_at);
        }

        // Plan limits and exact context figures, saved by `platoon statusline`.
        let snap = usage::load(&self.cache, usage::now());
        for s in &mut fresh {
            if let Some(p) = snap.context.get(&s.id) {
                s.context_pct = *p;
                s.context_exact = true;
            }
        }
        self.usage = match snap.plan {
            Some(p) => Usage::Ready(p),
            None if setup::statusline_installed(self.reader.root()) => Usage::Waiting,
            None => Usage::NotSetUp,
        };
        self.sessions = fresh;
        if let Some(pid) = selected_pid {
            if let Some(pos) = self.visible().iter().position(|&i| self.sessions[i].pid == pid) {
                self.selected = pos;
            }
        }
        self.clamp_selected();
    }

    pub fn act(&mut self, action: Action) {
        match (action, self.mode.clone()) {
            // Sessions started here keep running in their hosts, so quitting never ends them.
            (Action::Quit, Mode::List) => self.quit = true,
            (Action::New, Mode::List) => self.open_picker(),
            (Action::Kill, Mode::List) => match self.selected_session() {
                Some(s) => {
                    self.mode = Mode::Confirm(Confirm::Kill { pid: s.pid, name: s.name.clone(), dir: s.dir.clone() })
                }
                None => self.notify("No session selected"),
            },
            (Action::Filter, Mode::List) => self.mode = Mode::Filter,
            (Action::Jump, Mode::List | Mode::Filter) => self.jump(),
            (Action::Back, Mode::Attached(_)) => self.detach(),
            (Action::Confirm, Mode::Confirm(Confirm::Kill { pid, name, dir })) => {
                self.mode = Mode::List;
                self.kill(pid, &name, &dir);
            }
            (Action::Confirm, Mode::Confirm(Confirm::Adopt { pids, id, name, cwd, .. })) => {
                self.mode = Mode::List;
                self.adopt(&pids, &id, &name, cwd);
            }
            (Action::Confirm, Mode::Picker) => {
                if let Some(dir) = self.picker.results.get(self.picker.selected).cloned() {
                    self.launch(dir, &[]);
                }
            }
            (Action::Confirm, Mode::Filter) => self.mode = Mode::List,
            (Action::Cancel, Mode::Filter) => {
                self.filter.clear();
                self.mode = Mode::List;
            }
            (Action::Cancel, _) => self.mode = Mode::List,
            _ => {}
        }
    }

    fn open_picker(&mut self) {
        let all = dirs::candidates(self.reader.root(), &self.home);
        self.picker = Picker { input: String::new(), all, results: Vec::new(), selected: 0 };
        self.picker.update(&self.home);
        self.mode = Mode::Picker;
    }

    /// Leave the attached view. Only our connection closes; the session keeps running.
    fn detach(&mut self) {
        self.client = None;
        self.mode = Mode::List;
    }

    fn attach(&mut self, pid: u32) {
        let Some(info) = self.hosts.iter().find(|h| h.claude_pid == pid).cloned() else {
            self.notify("That session's host isn't running");
            return;
        };
        let (rows, cols) = self.pty_size();
        match Client::connect(&info.sock, rows, cols) {
            Ok(c) => {
                self.client = Some(c);
                self.mode = Mode::Attached(pid);
                if let Some(s) = self.sessions.iter_mut().find(|s| s.pid == pid) {
                    s.unread = false;
                }
            }
            Err(e) => self.notify(format!("Couldn't connect to the session: {e}")),
        }
    }

    /// Decide what Enter does on the selected row. A copy that platoon hosts and that is still in sync
    /// is simply attached to. Anything else (another terminal's copy, several copies, or a copy that
    /// fell out of sync) means ending the process(es) and reopening from the saved history.
    pub fn jump_plan(&self) -> Option<JumpPlan> {
        let s = self.selected_session()?;
        let group: Vec<&Session> = self.sessions.iter().filter(|o| o.id == s.id).collect();
        if let Some(h) = group.iter().find(|o| !o.external && !o.stale) {
            return Some(JumpPlan::Attach(h.pid));
        }
        let kind = if group.len() == 1 && s.external { AdoptKind::TakeOver } else { AdoptKind::Reload };
        Some(JumpPlan::Adopt {
            pids: group.iter().map(|o| o.pid).collect(),
            kind,
            busy: group.iter().any(|o| o.status == Status::Busy),
        })
    }

    fn jump(&mut self) {
        match self.jump_plan() {
            Some(JumpPlan::Attach(pid)) => self.attach(pid),
            Some(JumpPlan::Adopt { pids, kind, busy }) => {
                let Some(s) = self.selected_session() else { return };
                self.mode = Mode::Confirm(Confirm::Adopt {
                    pids,
                    id: s.id.clone(),
                    name: s.name.clone(),
                    cwd: s.cwd.clone(),
                    busy,
                    kind,
                });
            }
            None => {}
        }
    }

    /// End every live copy, wait for them to go, then resume the conversation under a platoon host.
    /// Reopening from the saved history means the new copy's memory matches it exactly.
    fn adopt(&mut self, pids: &[u32], id: &str, name: &str, cwd: String) {
        for &pid in pids {
            if pid <= 1 || unsafe { libc::kill(pid as i32, libc::SIGTERM) } != 0 {
                self.notify(format!("Couldn't stop {name}"));
                return;
            }
        }
        for _ in 0..100 {
            if !pids.iter().any(|&p| pid_alive(p)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if pids.iter().any(|&p| pid_alive(p)) {
            self.notify(format!("{name} didn't exit; try kill (x) first"));
            return;
        }
        self.launch(cwd, &["--resume", id]);
    }

    /// True when the attached session must not receive input: it fell out of sync with its conversation.
    pub fn attached_paused(&self) -> bool {
        let Mode::Attached(pid) = self.mode else { return false };
        // Check the marker directly too, so a copy refused a moment ago is paused before the next refresh.
        self.sessions.iter().find(|s| s.pid == pid).is_some_and(|s| s.stale || guard::is_stale(&self.cache, s.pid, s.started_at))
    }

    fn launch(&mut self, dir: String, args: &[&str]) {
        let (rows, cols) = self.pty_size();
        // PLATOON_CLAUDE_BIN overrides which program runs (a wrapper script, or a stand-in for tests).
        let program = std::env::var("PLATOON_CLAUDE_BIN").unwrap_or_else(|_| "claude".into());
        match host::launch(&program, &dir, args, rows, cols) {
            Ok((info, child)) => {
                let pid = info.claude_pid;
                self.spawned.push(child);
                self.hosts.push(info);
                self.filter.clear();
                self.attach(pid);
            }
            Err(e) => {
                self.mode = Mode::List;
                self.notify(format!("Couldn't start claude: {e}"));
            }
        }
    }

    #[cfg(test)]
    pub fn picker_launch_for_test(&mut self, dir: String) {
        self.launch(dir, &[]);
    }

    /// Stop the process. The conversation stays on disk, so the session is inactive, not deleted.
    fn kill(&mut self, pid: u32, name: &str, dir: &str) {
        // SIGTERM lets Claude Code shut down cleanly. Guard against nonsense pids.
        if pid > 1 && unsafe { libc::kill(pid as i32, libc::SIGTERM) } == 0 {
            self.notify(format!("\"{name}\" is now inactive. Resume it from {dir} with `claude --resume`."));
        } else {
            self.notify(format!("Couldn't signal {name}"));
        }
    }

    fn move_selection(&mut self, delta: i32) {
        let last = self.visible().len().saturating_sub(1) as i32;
        self.selected = (self.selected as i32 + delta).clamp(0, last) as usize;
    }

    fn move_picker(&mut self, delta: i32) {
        let last = self.picker.results.len().saturating_sub(1) as i32;
        self.picker.selected = (self.picker.selected as i32 + delta).clamp(0, last) as usize;
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match self.mode.clone() {
            Mode::Attached(_) => {
                // The prefix chord: everything else goes to Claude.
                if ctrl && key.code == KeyCode::Char(' ') {
                    self.detach();
                } else if self.attached_paused() {
                    // Paused: nothing is forwarded, so this copy can't write to the conversation.
                } else if let Some(c) = self.client.as_mut() {
                    c.reset_scroll();
                    let bytes = key_bytes(key, c.app_cursor());
                    c.write(&bytes);
                }
            }
            Mode::Picker => match key.code {
                KeyCode::Esc => self.act(Action::Cancel),
                KeyCode::Enter => self.act(Action::Confirm),
                KeyCode::Up => self.move_picker(-1),
                KeyCode::Down => self.move_picker(1),
                KeyCode::Char('p') if ctrl => self.move_picker(-1),
                KeyCode::Char('n') if ctrl => self.move_picker(1),
                KeyCode::Char('c') if ctrl => self.act(Action::Cancel),
                KeyCode::Backspace => {
                    self.picker.input.pop();
                    self.picker.update(&self.home);
                }
                KeyCode::Char(c) if !ctrl && !alt => {
                    self.picker.input.push(c);
                    self.picker.update(&self.home);
                }
                _ => {}
            },
            Mode::Confirm(_) => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => self.act(Action::Confirm),
                KeyCode::Char('n') | KeyCode::Esc => self.act(Action::Cancel),
                KeyCode::Char('c') if ctrl => self.act(Action::Cancel),
                _ => {}
            },
            Mode::Filter => match key.code {
                KeyCode::Esc => self.act(Action::Cancel),
                KeyCode::Enter => self.act(Action::Confirm),
                KeyCode::Up => self.move_selection(-1),
                KeyCode::Down => self.move_selection(1),
                KeyCode::Char('c') if ctrl => self.act(Action::Cancel),
                KeyCode::Backspace => {
                    self.filter.pop();
                    self.selected = 0;
                }
                KeyCode::Char(c) if !ctrl && !alt => {
                    self.filter.push(c);
                    self.selected = 0;
                }
                _ => {}
            },
            Mode::List => match key.code {
                KeyCode::Char('c') if ctrl => self.act(Action::Quit),
                KeyCode::Char('q') => self.act(Action::Quit),
                KeyCode::Char('n') => self.act(Action::New),
                KeyCode::Char('x') => self.act(Action::Kill),
                KeyCode::Char('/') => self.act(Action::Filter),
                KeyCode::Enter => self.act(Action::Jump),
                KeyCode::Esc => {
                    self.filter.clear();
                    self.selected = 0;
                }
                KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
                KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
                KeyCode::Char('g') | KeyCode::Home => self.selected = 0,
                KeyCode::Char('G') | KeyCode::End => self.selected = self.visible().len().saturating_sub(1),
                _ => {}
            },
        }
        self.clamp_selected();
    }

    pub fn on_paste(&mut self, text: &str) {
        match self.mode.clone() {
            Mode::Attached(_) if self.attached_paused() => {}
            Mode::Attached(_) => {
                if let Some(c) = self.client.as_mut() {
                    c.reset_scroll();
                    c.write(format!("\x1b[200~{text}\x1b[201~").as_bytes());
                }
            }
            Mode::Picker => {
                self.picker.input.push_str(&text.replace('\n', ""));
                self.picker.update(&self.home);
            }
            Mode::Filter => {
                self.filter.push_str(&text.replace('\n', ""));
                self.selected = 0;
            }
            _ => {}
        }
    }

    pub fn on_mouse(&mut self, m: MouseEvent) {
        self.mouse = (m.column, m.row);
        match m.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let up = m.kind == MouseEventKind::ScrollUp;
                match self.mode.clone() {
                    Mode::Attached(_) => {
                        if let Some(c) = &self.client {
                            c.scroll(if up { 3 } else { -3 });
                        }
                    }
                    Mode::Picker => self.move_picker(if up { -1 } else { 1 }),
                    Mode::List | Mode::Filter => self.move_selection(if up { -1 } else { 1 }),
                    Mode::Confirm(_) => {}
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let hit = self
                    .hits
                    .borrow()
                    .iter()
                    .find(|(r, _)| r.contains((m.column, m.row).into()))
                    .map(|(_, h)| *h);
                match hit {
                    Some(Hit::Action(a)) => self.act(a),
                    // Clicking the selected row again acts like Enter.
                    Some(Hit::Row(i)) if i == self.selected => self.act(Action::Jump),
                    Some(Hit::Row(i)) => self.selected = i,
                    Some(Hit::PickerRow(i)) if i == self.picker.selected => self.act(Action::Confirm),
                    Some(Hit::PickerRow(i)) => self.picker.selected = i,
                    None => {}
                }
            }
            _ => {}
        }
    }
}
