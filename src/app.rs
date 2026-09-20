use crate::archive::{self, Archive};
use crate::claude::{self, Live, Reader};
use crate::client::{key_bytes, Client};
use crate::clipboard::Clipboard;
use crate::dirs;
use crate::guard;
use crate::host::{self, HostInfo};
use crate::index::{Entry, Indexer};
use crate::model::{Proc, Session, Status};
use crate::select::{click_count, extract, Click, Point, Selection, Unit};
use crate::settings::{Kind, SettingsStore, Step, ITEMS};
use crate::setup;
use crate::theme::Theme;
use crate::usertheme::Registry;
use crate::usage::{self, Usage};
use fuzzy_matcher::{skim::SkimMatcherV2, FuzzyMatcher};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

const NOTICE_FOR: Duration = Duration::from_secs(5);
/// Automatic archiving is something the user didn't do, so its notice stays long enough to be read.
const AUTO_ARCHIVE_NOTICE_FOR: Duration = Duration::from_secs(60);
/// Auto-archive is also re-checked this often, so a conversation that turns old while platoon stays open is caught.
const AUTO_ARCHIVE_EVERY: Duration = Duration::from_secs(60);
/// After a change on the settings screen, auto-archive waits this long for the next one before acting. The
/// steps are one key press apart, so getting from 2 days to Off passes through 1 day, and archiving at that
/// stop would put away conversations the user was on the way to keeping.
const SETTLE: Duration = Duration::from_millis(1200);

/// A tab in the header. Adding one is adding an entry here: the header, the number keys (`1` is the
/// first tab, `2` the second, ...) and the counts all follow.
pub struct Tab {
    pub label: &'static str,
    /// Shown when the tab has nothing in it.
    pub empty: &'static str,
    pub shows: fn(&Session) -> bool,
}

pub const TABS: &[Tab] = &[
    Tab { label: "Active", empty: "No conversations yet. Press n to start one.", shows: |s| !s.archived },
    Tab { label: "Archived", empty: "Nothing archived. Press a on a conversation to archive it.", shows: |s| s.archived },
];

/// Round badge caps are Powerline/Nerd Font glyphs. Terminals that bring their own symbol fallback (Ghostty, Kitty,
/// WezTerm) draw them with any font; elsewhere they can show as empty boxes, so those get square badges.
/// `PLATOON_BADGES=rounded` or `square` overrides the guess.
fn detect_rounded() -> bool {
    detect_rounded_with(&|k| std::env::var(k).ok())
}

pub fn detect_rounded_with(env: &dyn Fn(&str) -> Option<String>) -> bool {
    match env("PLATOON_BADGES").as_deref() {
        Some("rounded") => return true,
        Some("square") => return false,
        _ => {}
    }
    let (program, term) = (env("TERM_PROGRAM").unwrap_or_default(), env("TERM").unwrap_or_default());
    program.eq_ignore_ascii_case("ghostty")
        || program.eq_ignore_ascii_case("wezterm")
        || term.contains("ghostty")
        || term.contains("kitty")
        || ["KITTY_WINDOW_ID", "WEZTERM_EXECUTABLE", "GHOSTTY_RESOURCES_DIR"].iter().any(|k| env(k).is_some())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Jump,
    New,
    Archive,
    Filter,
    Settings,
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
    Tab(usize),
    /// A row of the settings screen: selects it.
    Setting(usize),
    /// A step button (or the value) of a setting: changes it.
    SettingStep(usize, Step),
    /// An entry of the theme list: the first click previews it, a click on the previewed one keeps it.
    ThemeRow(usize),
    /// Part of a panel that does nothing when clicked, but is not "outside" it either.
    Inert,
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
    /// A copy platoon hosts and that is in sync: just show it.
    Attach(u32),
    /// Nothing is running it: start it again from its saved history.
    Resume,
    /// Claude's own service runs it in the background and no platoon host is showing it yet: start one running
    /// `claude attach <id>` (with this id). Never `--resume`, which Claude refuses for a background session.
    AttachBackground(String),
    /// Something platoon doesn't own (or can't trust) is running it, and ending that is destructive.
    Adopt { pids: Vec<u32>, kind: AdoptKind, busy: bool },
}

#[derive(Clone)]
pub enum Confirm {
    /// Take over a conversation running elsewhere: end it, then resume it inside platoon.
    Adopt { pids: Vec<u32>, id: String, name: String, cwd: String, busy: bool, kind: AdoptKind, restore: bool },
    /// Archiving stops the process. Only asked when that is destructive: it is in the middle of working,
    /// or it runs in a terminal platoon doesn't own (so its owner didn't choose this).
    Archive { id: String, name: String, busy: bool, outside: bool },
}

#[derive(Clone)]
pub enum Mode {
    List,
    /// Typing into the filter box.
    Filter,
    Picker,
    Confirm(Confirm),
    /// The settings screen, which replaces the list.
    Settings,
    /// The theme list, opened from the Theme row of the settings. Moving through it previews each theme on the
    /// whole app; only `Enter` keeps one.
    Themes,
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

/// The theme list while it is open.
pub struct ThemePick {
    /// Built-ins first, then the user's own, as they were when the list opened.
    pub list: Vec<Theme>,
    /// The highlighted entry: the one being previewed.
    pub selected: usize,
    /// The first entry on screen, kept between frames so that the list only scrolls when it has to.
    pub top: Cell<usize>,
}

pub struct App {
    /// Every conversation, newest activity first, across all tabs.
    pub sessions: Vec<Session>,
    /// Indices into `sessions` shown now: the current tab, narrowed by the filter.
    view: Vec<usize>,
    /// How many conversations each tab holds (unfiltered), parallel to `TABS`.
    pub counts: Vec<usize>,
    /// Which of `TABS` is showing.
    pub tab: usize,
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
    pub notice: Option<(String, Instant, Duration)>,
    pub mouse: (u16, u16),
    /// Text being selected with the mouse in the attached view: highlighted as it grows, copied when the button
    /// is released. Cleared by anything that could make it stale (a key, a paste, a resize, the wheel, leaving).
    pub selection: Option<Selection>,
    /// The left button is held and the selection follows the mouse.
    dragging: bool,
    /// The last press in the session's screen, for double and triple clicks.
    last_click: Option<Click>,
    /// Where copied text goes. Lives as long as the app: on X11 the program that set the clipboard is the one
    /// that hands it to whoever pastes, so it has to stay around for the text to stay pasteable.
    clipboard: Clipboard,
    /// Draw count badges with round caps. Needs glyphs only some terminals have, so it is off unless detected or asked for.
    pub rounded: bool,
    pub home: String,
    /// Where platoon keeps its own state (`~/.cache/platoon`).
    pub cache: PathBuf,
    /// Animation frame counter, ~10 per second.
    pub tick: u64,
    /// The theme the settings name: what the app looks like unless a preview is showing.
    pub theme: Theme,
    /// A theme being tried in the theme list. Everything is drawn with `effective_theme`, so this recolors the
    /// whole app at once; it is dropped as soon as the list closes, and the app is exactly as it was.
    pub preview: Option<Theme>,
    pub tpick: ThemePick,
    /// The user's own themes, from `<config dir>/themes`.
    themes: Registry,
    pub quit: bool,
    pub hits: RefCell<Vec<(Rect, Hit)>>,
    size: (u16, u16),
    reader: Reader,
    indexer: Indexer,
    archive: Archive,
    /// Automatic clean-up settings, re-read when the file changes.
    settings: SettingsStore,
    /// The selected line of the settings screen, an index into `ITEMS`.
    pub setting: usize,
    /// The conversation opened in the attached view: never auto-archived, even before its process registers.
    viewing: Option<String>,
    /// When auto-archive last ran (None: not yet).
    last_auto_archive: Option<Instant>,
    /// A settings change is waiting to take effect in auto-archive, which is on hold until then.
    apply_after: Option<Instant>,
    /// The program that runs a conversation: `claude`, or PLATOON_CLAUDE_BIN (a wrapper, or a stand-in in tests).
    program: String,
    guard_on: bool,
    /// What `view` and `counts` were computed from: (filter, tab, generation). Recomputed when it changes.
    view_for: (String, usize, u64),
    /// Bumped on every refresh.
    rev: u64,
    /// The index generation and live session ids `sessions` was built from.
    built: (u64, Vec<String>),
}

impl App {
    /// The real thing: rows appear as the background index reads them.
    pub fn new() -> Self {
        let reader = Reader::new();
        let cache = usage::cache_dir();
        let indexer = Indexer::spawn(reader.root(), &cache);
        let mut app = Self::build(reader, cache, archive::config_dir(), indexer);
        app.rounded = detect_rounded();
        app.clipboard = Clipboard::from_env();
        app
    }

    /// `cache` and `config` are where platoon keeps its own state; tests pass temp dirs so they never touch
    /// the real ones. The index is read on the calling thread, so tests see rows straight away.
    #[cfg(test)]
    pub fn with_dirs(reader: Reader, cache: PathBuf, config: PathBuf) -> Self {
        let indexer = Indexer::inline(reader.root(), &cache);
        Self::build(reader, cache, config, indexer)
    }

    fn build(reader: Reader, cache: PathBuf, config: PathBuf, indexer: Indexer) -> Self {
        let mut app = Self {
            sessions: Vec::new(),
            view: Vec::new(),
            counts: vec![0; TABS.len()],
            tab: 0,
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
            selection: None,
            dragging: false,
            last_click: None,
            // Nothing is ever copied by an App that was not asked to: `new` and tests choose.
            clipboard: Clipboard::off(),
            home: std::env::var("HOME").unwrap_or_default(),
            cache,
            tick: 0,
            theme: Theme::default(),
            preview: None,
            tpick: ThemePick { list: Vec::new(), selected: 0, top: Cell::new(0) },
            themes: Registry::open(&config),
            quit: false,
            rounded: false,
            hits: RefCell::new(Vec::new()),
            size: (80, 24),
            reader,
            indexer,
            archive: Archive::open(&config),
            settings: SettingsStore::open(&config),
            setting: 0,
            viewing: None,
            last_auto_archive: None,
            apply_after: None,
            program: std::env::var("PLATOON_CLAUDE_BIN").unwrap_or_else(|_| "claude".into()),
            guard_on: false,
            view_for: (String::new(), usize::MAX, 0),
            rev: 0,
            built: (0, Vec::new()),
        };
        app.theme = app.settings.get().theme(&app.themes);
        // Real inputs (running hosts, the guard setting) come with the first `refresh`; until then assume none.
        app.refresh_with(Vec::new(), false);
        app
    }

    pub fn settings(&self) -> &SettingsStore {
        &self.settings
    }

    /// Pick up an edit of settings.json and of the theme folder, and with them a changed theme. True when the
    /// settings file was re-read.
    fn refresh_settings(&mut self) -> bool {
        let changed = self.settings.refresh();
        // The theme list keeps the entries it opened with, so the folder is looked at again once it has closed.
        let themes_changed = !matches!(self.mode, Mode::Themes) && self.themes.refresh();
        if changed || themes_changed {
            self.sync_theme();
        }
        changed
    }

    /// The theme the settings name (the default when they name one platoon doesn't know, say a user theme whose
    /// file is gone). A preview, if there is one, is left alone: it wins until the list closes.
    fn sync_theme(&mut self) {
        let want = self.settings.get().theme(&self.themes);
        if want != self.theme {
            self.theme = want;
            // The highlighted entry is what is previewed, unless it is the theme now in effect.
            if matches!(self.mode, Mode::Themes) {
                self.select_theme(self.tpick.selected);
            }
        }
    }

    /// What everything is drawn with: the theme being previewed, else the one the settings name.
    pub fn effective_theme(&self) -> &Theme {
        self.preview.as_ref().unwrap_or(&self.theme)
    }

    pub fn themes(&self) -> &Registry {
        &self.themes
    }

    pub fn notify(&mut self, msg: impl Into<String>) {
        self.notice = Some((msg.into(), Instant::now(), NOTICE_FOR));
    }

    /// The auto-archive notice stays until the user does something (it covers the key hints while it shows).
    fn dismiss_long_notice(&mut self) {
        if self.notice.as_ref().is_some_and(|(_, _, ttl)| *ttl == AUTO_ARCHIVE_NOTICE_FOR) {
            self.notice = None;
        }
    }

    pub fn active_notice(&self) -> Option<&str> {
        self.notice.as_ref().filter(|(_, at, ttl)| at.elapsed() < *ttl).map(|(m, _, _)| m.as_str())
    }

    /// Indices into `sessions` for the rows on screen: current tab, narrowed by the filter.
    pub fn visible(&self) -> &[usize] {
        &self.view
    }

    /// Recompute the rows on screen if the filter, tab or data changed. Called after every event and
    /// refresh, and does nothing (one comparison) when nothing moved.
    fn update_view(&mut self) {
        if self.view_for.0 == self.filter && self.view_for.1 == self.tab && self.view_for.2 == self.rev {
            return;
        }
        self.view_for = (self.filter.clone(), self.tab, self.rev);
        let q = self.filter.to_lowercase();
        let shows = TABS[self.tab.min(TABS.len() - 1)].shows;
        self.view = self
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| shows(s) && (q.is_empty() || s.haystack.contains(&q)))
            .map(|(i, _)| i)
            .collect();
        self.counts = TABS.iter().map(|t| self.sessions.iter().filter(|s| (t.shows)(s)).count()).collect();
        self.clamp_selected();
    }

    pub fn selected_session(&self) -> Option<&Session> {
        self.view.get(self.selected).map(|&i| &self.sessions[i])
    }

    fn clamp_selected(&mut self) {
        self.selected = self.selected.min(self.view.len().saturating_sub(1));
    }

    pub fn unread_count(&self) -> usize {
        self.sessions.iter().filter(|s| s.unread).count()
    }

    /// Transcripts still being read by the background index, or Some(0) while the very first pass is
    /// still listing files. None once it's caught up.
    pub fn indexing(&self) -> Option<usize> {
        match self.indexer.remaining {
            0 if self.sessions.is_empty() && self.indexer.scanning() => Some(0),
            0 => None,
            n => Some(n),
        }
    }

    pub fn switch_tab(&mut self, i: usize) {
        if i < TABS.len() && i != self.tab {
            self.tab = i;
            self.selected = 0;
            self.update_view();
        }
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
        self.clear_selection();
    }

    /// Where the session's screen is drawn in the attached view: inside the frame and above the status bar.
    /// The same numbers as `pty_size` and `ui::draw_attached`.
    pub fn term_area(&self) -> Rect {
        let (rows, cols) = self.pty_size();
        Rect::new(1, 1, cols, rows)
    }

    /// Re-read Claude Code's state, carrying unread flags and the selection across.
    pub fn refresh(&mut self) {
        self.spawned.retain_mut(|c| c.try_wait().ok().flatten().is_none());
        let guard_on = setup::guard_installed(self.reader.root());
        self.refresh_with(host::discover(), guard_on);
    }

    /// `refresh` with the same outside inputs as last time (after an action that changed our own state).
    fn reload(&mut self) {
        self.refresh_with(self.hosts.clone(), self.guard_on);
    }

    fn attached_pid(&self) -> Option<u32> {
        if let Mode::Attached(p) = self.mode { Some(p) } else { None }
    }

    /// `refresh` with its outside inputs supplied: which sessions platoon hosts, and whether the
    /// guard hook is installed (so a second copy of a session gets refused at the source).
    pub fn refresh_with(&mut self, hosts: Vec<HostInfo>, guard_on: bool) {
        let attached = self.attached_pid();
        self.hosts = hosts;
        self.guard_on = guard_on;
        if let Some(pid) = attached {
            if self.client.as_ref().map_or(true, |c| c.closed()) || !self.hosts.iter().any(|h| h.claude_pid == pid) {
                self.mode = Mode::List;
                self.client = None;
                self.clear_selection();
                self.notify("Session ended");
            }
        }

        // What we knew about running conversations, for spotting busy -> idle across the rebuild.
        let prev: HashMap<String, (Status, bool)> =
            self.sessions.iter().filter(|s| !s.procs.is_empty()).map(|s| (s.id.clone(), (s.status, s.unread))).collect();
        let selected_id = self.selected_session().map(|s| s.id.clone());

        let index_changed = self.indexer.poll();
        self.archive.reload_if_changed();
        let live = self.reader.live();

        // One row per conversation, however many processes run it.
        let mut by_id: HashMap<&str, Vec<&Live>> = HashMap::new();
        for l in &live {
            by_id.entry(l.id.as_str()).or_default().push(l);
        }
        let mut live_ids: Vec<String> = by_id.keys().map(|k| k.to_string()).collect();
        live_ids.sort();
        if index_changed || self.built.0 != self.indexer.generation || self.built.1 != live_ids || self.rev == 0 {
            self.sessions = self.build_sessions(&by_id);
            self.built = (self.indexer.generation, live_ids);
        }

        // Two live processes on one conversation each hold their own memory of it and write to the same
        // history. With the guard installed the later copy is refused at its first prompt; without it,
        // all platoon can do is stop writing through its own copy, and require a reload from the history.
        guard::prune_stale(&self.cache);
        // Plan limits and exact context figures, saved by `platoon statusline`.
        let snap = usage::load(&self.cache, usage::now());
        for s in &mut self.sessions {
            s.archived = self.archive.contains(&s.id);
            s.context_exact = snap.context.contains_key(&s.id);
            s.context_pct = snap.context.get(&s.id).copied().unwrap_or(s.context_est);
            s.procs.clear();
            let Some(ls) = by_id.get(s.id.as_str()) else {
                s.status = Status::Idle;
                s.unread = false;
                continue;
            };
            // Only interactive copies can fall out of sync with each other. A background session is run by
            // Claude's own service: it is never marked stale and never counts toward "several copies".
            let shared = ls.iter().filter(|l| !l.is_background()).count() > 1;
            for l in ls {
                if let Some(attach_id) = &l.background {
                    // `hosted`: a platoon host is showing it right now (its process is a `claude attach`, which has no
                    // registry entry of its own, so it never appears among `live`).
                    let hosted = self.hosts.iter().any(|h| h.attaches_to(attach_id));
                    s.procs.push(Proc { pid: l.pid, started_at: l.started_at, hosted, background: Some(attach_id.clone()), stale: false });
                    continue;
                }
                let hosted = self.hosts.iter().any(|h| h.claude_pid == l.pid);
                if shared && hosted && !guard_on {
                    guard::mark_stale(&self.cache, l.pid, l.started_at);
                }
                let stale = guard::is_stale(&self.cache, l.pid, l.started_at);
                s.procs.push(Proc { pid: l.pid, started_at: l.started_at, hosted, background: None, stale });
            }
            if let Some(n) = ls.iter().find_map(|l| l.name.as_ref()) {
                s.name.clone_from(n);
            }
            s.status = if ls.iter().any(|l| l.busy && !l.is_background()) {
                Status::Busy
            } else if ls.iter().any(|l| l.is_background()) {
                Status::Background
            } else {
                Status::Idle
            };
            // A background session is on the Active tab whatever the archive says: it is running right now.
            if s.status == Status::Background {
                s.archived = false;
            }
            // Unread = Claude went busy -> idle while we were watching. Starting work again clears it.
            // Sessions already idle when first seen count as read; so do ones we're looking at.
            s.unread = s.status == Status::Idle && prev.get(&s.id).is_some_and(|(st, unread)| *unread || *st == Status::Busy);
            if attached.is_some_and(|pid| s.procs.iter().any(|p| p.pid == pid)) {
                s.unread = false;
            }
        }
        self.auto_archive(index_changed, usage::now());
        self.usage = match snap.plan {
            Some(p) => Usage::Ready(p),
            None if setup::statusline_installed(self.reader.root()) => Usage::Waiting,
            None => Usage::NotSetUp,
        };
        self.rev += 1;
        self.update_view();
        if let Some(id) = selected_id {
            if let Some(pos) = self.view.iter().position(|&i| self.sessions[i].id == id) {
                self.selected = pos;
            }
        }
        self.clamp_selected();
    }

    /// Archive conversations that have sat unused for the configured number of days (default 2, 0 = off).
    /// Only when something changed or a minute has passed, and only once the index has caught up with the
    /// disk: a conversation used in another terminal since the last run must not look old just because
    /// the saved index says so. All of them go into one write, and one notice says how many.
    fn auto_archive(&mut self, sessions_changed: bool, now_secs: u64) {
        let settings_changed = self.refresh_settings();
        // A value is still being chosen on the settings screen: hold everything until it stops changing.
        let settled = match self.apply_after {
            Some(t) if Instant::now() < t => return,
            Some(_) => {
                self.apply_after = None;
                true
            }
            None => false,
        };
        let due = settled || sessions_changed || settings_changed || self.last_auto_archive.is_none_or(|t| t.elapsed() >= AUTO_ARCHIVE_EVERY);
        if !due || !self.indexer.first_scan_done || self.indexer.remaining > 0 {
            return;
        }
        self.last_auto_archive = Some(Instant::now());
        let days = self.settings.get().auto_archive_days;
        let Some(age) = self.settings.get().auto_archive_secs() else { return };
        // A file we couldn't read holds something unknown; only the user's own action may replace it.
        if self.archive.is_corrupt() {
            return;
        }
        let viewing = self.attached_pid().and(self.viewing.as_deref());
        let ids = archive::auto_archive_ids(&self.sessions, &self.archive, now_secs, age, viewing);
        if ids.is_empty() {
            return;
        }
        match self.archive.archive_many(&ids, now_secs) {
            Ok(()) => {
                for s in self.sessions.iter_mut().filter(|s| ids.contains(&s.id)) {
                    s.archived = true;
                }
                let (n, plural) = (ids.len(), if ids.len() == 1 { "" } else { "s" });
                let (them, day) = (if n == 1 { "it" } else { "them" }, if days == 1 { "day" } else { "days" });
                self.notice = Some((
                    format!("Auto-archived {n} conversation{plural} idle for {days}+ {day}. Find {them} under Archived."),
                    Instant::now(),
                    AUTO_ARCHIVE_NOTICE_FOR,
                ));
            }
            Err(e) => self.notify(format!("Couldn't auto-archive: {e}")),
        }
    }

    /// The conversations to list: every saved one that has a real prompt, plus anything running (even before
    /// its first prompt is saved), newest activity first. Running-state fields are filled in by the caller.
    fn build_sessions(&self, live: &HashMap<&str, Vec<&Live>>) -> Vec<Session> {
        // The same id can be saved under two project folders; the most recently active file wins.
        let mut latest: HashMap<&str, &Arc<Entry>> = HashMap::new();
        for e in self.indexer.entries.values() {
            let slot = latest.entry(e.id.as_str()).or_insert(e);
            if (e.last_active_ms(), e.mtime) > (slot.last_active_ms(), slot.mtime) {
                *slot = e;
            }
        }
        let mut out: Vec<Session> = latest
            .values()
            .filter(|e| e.t.first_prompt.is_some() || live.contains_key(e.id.as_str()))
            .map(|e| self.session_from(e))
            .collect();
        for (id, ls) in live {
            if !latest.contains_key(id) {
                out.push(self.session_from_live(ls[0]));
            }
        }
        out.sort_by(|a, b| b.last_active.cmp(&a.last_active).then_with(|| a.id.cmp(&b.id)));
        out
    }

    fn session_from(&self, e: &Entry) -> Session {
        let t = &e.t;
        let cwd = t.cwd.clone().unwrap_or_default();
        let mut large = t.large_window;
        let window = claude::window_size(t.context_tokens, &mut large);
        let est = (t.context_tokens * 100 / window).min(100) as u8;
        let description = t.ai_title.clone().or_else(|| t.first_prompt.clone()).unwrap_or_default();
        let branch = t.branch.clone().filter(|b| b != "HEAD");
        self.session(e.id.clone(), cwd, branch, t.model.as_deref().map(claude::pretty_model), est, description, e.last_active_ms())
    }

    /// A running conversation whose transcript isn't saved yet (nothing sent), described by its registry entry.
    fn session_from_live(&self, l: &Live) -> Session {
        self.session(l.id.clone(), l.cwd.clone(), None, None, 0, String::new(), l.started_at)
    }

    #[allow(clippy::too_many_arguments)]
    fn session(&self, id: String, cwd: String, branch: Option<String>, model: Option<String>, est: u8, description: String, last_active: u64) -> Session {
        // Claude Code derives a placeholder name; only a name the user chose (set from the registry) replaces this.
        let name = Path::new(&cwd).file_name().and_then(|n| n.to_str()).filter(|n| !n.is_empty()).unwrap_or(&cwd).to_string();
        let dir = dirs::tilde(&cwd, &self.home);
        let haystack = [name.as_str(), dir.as_str(), branch.as_deref().unwrap_or(""), description.as_str()].join("\n").to_lowercase();
        Session {
            id,
            name,
            dir,
            cwd,
            branch,
            model,
            status: Status::Idle,
            unread: false,
            context_pct: est,
            context_exact: false,
            context_est: est,
            description,
            last_active,
            archived: false,
            procs: Vec::new(),
            haystack,
        }
    }

    pub fn act(&mut self, action: Action) {
        match (action, self.mode.clone()) {
            // Sessions started here keep running in their hosts, so quitting never ends them.
            (Action::Quit, Mode::List) => self.quit = true,
            (Action::New, Mode::List) => self.open_picker(),
            (Action::Archive, Mode::List) => self.toggle_archive(),
            (Action::Filter, Mode::List) => self.mode = Mode::Filter,
            (Action::Settings, Mode::List) => self.open_settings(),
            (Action::Jump, Mode::List | Mode::Filter) => self.jump(),
            (Action::Back, Mode::Attached(_)) => self.detach(),
            (Action::Confirm, Mode::Confirm(Confirm::Archive { id, name, .. })) => {
                self.mode = Mode::List;
                self.archive_conversation(&id, &name);
            }
            (Action::Confirm, Mode::Confirm(Confirm::Adopt { pids, id, name, cwd, restore, .. })) => {
                self.mode = Mode::List;
                self.adopt(&pids, &id, &name, cwd, restore);
            }
            (Action::Confirm, Mode::Picker) => {
                if let Some(dir) = self.picker.results.get(self.picker.selected).cloned() {
                    self.launch(dir, &[]);
                }
            }
            (Action::Confirm, Mode::Themes) => self.keep_theme(),
            (Action::Cancel, Mode::Themes) => self.close_theme_list(),
            (Action::Confirm, Mode::Filter) => self.mode = Mode::List,
            (Action::Cancel, Mode::Filter) => {
                self.filter.clear();
                self.mode = Mode::List;
            }
            (Action::Cancel, _) => self.mode = Mode::List,
            _ => {}
        }
        self.update_view();
    }

    fn open_picker(&mut self) {
        let all = dirs::candidates(self.reader.root(), &self.home);
        self.picker = Picker { input: String::new(), all, results: Vec::new(), selected: 0 };
        self.picker.update(&self.home);
        self.mode = Mode::Picker;
    }

    /// Show the settings screen, starting from what is on disk now.
    fn open_settings(&mut self) {
        self.refresh_settings();
        self.setting = 0;
        self.mode = Mode::Settings;
    }

    /// Open the theme list on the theme in effect. Nothing changes until the highlight moves.
    fn open_theme_list(&mut self) {
        if !matches!(self.mode, Mode::Settings) {
            return;
        }
        self.refresh_settings();
        let list = self.themes.all();
        let selected = list.iter().position(|t| t.id == self.theme.id).unwrap_or(0);
        self.tpick = ThemePick { list, selected, top: Cell::new(0) };
        self.preview = None;
        self.mode = Mode::Themes;
    }

    /// Highlight entry `i` and preview it (the theme in effect needs no preview: the app already looks like it).
    fn select_theme(&mut self, i: usize) {
        let Some(t) = self.tpick.list.get(i) else { return };
        self.preview = (t.id != self.theme.id).then(|| t.clone());
        self.tpick.selected = i;
    }

    fn move_theme(&mut self, delta: i32) {
        let last = self.tpick.list.len().saturating_sub(1) as i32;
        self.select_theme((self.tpick.selected as i32 + delta).clamp(0, last) as usize);
    }

    /// Leave the theme list for the settings, with the app back in the theme it was in.
    fn close_theme_list(&mut self) {
        self.preview = None;
        self.mode = Mode::Settings;
    }

    /// Enter: keep the previewed theme by saving it like any other setting. If the file can't be written the app
    /// goes back to the theme that is really saved, and says so.
    fn keep_theme(&mut self) {
        let keep = self.preview.take().map(|t| t.id);
        self.mode = Mode::Settings;
        self.notice = None;
        let Some(id) = keep else { return };
        let mut next = self.settings.get();
        next.theme = id;
        let saved = self.settings.update(&next);
        self.sync_theme();
        self.report_save(saved);
    }

    /// `←` and `→` on the Theme row: the neighbouring theme in list order, saved at once, no preview.
    fn step_theme(&mut self, step: Step) {
        let list = self.themes.all();
        let at = list.iter().position(|t| t.id == self.theme.id).unwrap_or(0);
        let to = match step {
            Step::Prev => at.checked_sub(1),
            _ => Some(at + 1).filter(|&n| n < list.len()),
        };
        let Some(to) = to else { return };
        let mut next = self.settings.get();
        next.theme = list[to].id.clone();
        let saved = self.settings.update(&next);
        self.sync_theme();
        self.report_save(saved);
    }

    /// A save that failed leaves the old value in effect: say why.
    fn report_save(&mut self, saved: io::Result<()>) {
        if let Err(e) = saved {
            let why = e.to_string();
            let why = why.split(" (os error").next().unwrap_or(&why);
            self.notify(format!("Couldn't save the settings: {why}"));
        }
    }

    /// Move to another line of the settings screen.
    fn move_setting(&mut self, delta: i32) {
        self.setting = (self.setting as i32 + delta).clamp(0, ITEMS.len() as i32 - 1) as usize;
    }

    /// Change one setting a step and save it at once. What the screen shows afterwards is read back from
    /// the file, so a save that failed leaves the old value on screen with a notice saying why.
    fn change_setting(&mut self, i: usize, step: Step) {
        let Some(item) = ITEMS.get(i) else { return };
        self.notice = None;
        // Start from the file as it is now, in case it was edited since the screen last looked.
        self.refresh_settings();
        if item.kind == Kind::Theme {
            match step {
                Step::Cycle => self.open_theme_list(),
                _ => self.step_theme(step),
            }
            return;
        }
        let mut next = self.settings.get();
        let old = (item.get)(&next);
        let new = item.step(old, step);
        if new == old {
            return;
        }
        (item.set)(&mut next, new);
        let saved = self.settings.update(&next);
        self.sync_theme();
        if saved.is_err() {
            self.report_save(saved);
            return;
        }
        // The next refresh acts on it (auto-archive, the only thing platoon itself does with these, once the
        // value has settled); sessions pick it up from the file on their own.
        self.apply_after = Some(Instant::now() + SETTLE);
    }

    /// A change is waiting to take effect, so the next refresh should come soon rather than in a second.
    pub fn settling(&self) -> bool {
        self.apply_after.is_some()
    }

    /// Leave the attached view. Only our connection closes; the session keeps running.
    fn detach(&mut self) {
        self.client = None;
        self.mode = Mode::List;
        self.clear_selection();
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
                self.clear_selection();
                if let Some(s) = self.sessions.iter_mut().find(|s| s.procs.iter().any(|p| p.pid == pid)) {
                    s.unread = false;
                }
            }
            Err(e) => {
                // Most likely the host just went (an idle session stopped since the last refresh). Forget it now,
                // so pressing enter again opens the conversation from its saved history instead of failing again.
                self.hosts.retain(|h| h.claude_pid != pid);
                self.reload();
                if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) {
                    self.notify("That session just stopped. Press enter to open it again.");
                } else {
                    self.notify(format!("Couldn't connect to the session: {e}"));
                }
            }
        }
    }

    /// Decide what Enter does on the selected row. Nothing running it: resume it from the saved history.
    /// A copy that platoon hosts and that is still in sync is simply attached to. Anything else (another
    /// terminal's copy, several copies, or a copy that fell out of sync) means ending the process(es)
    /// and reopening from the saved history, which is destructive, so it is confirmed first.
    pub fn jump_plan(&self) -> Option<JumpPlan> {
        let s = self.selected_session()?;
        if s.procs.is_empty() {
            return Some(JumpPlan::Resume);
        }
        // Claude's own service runs it: open its screen with `claude attach`, in a platoon host of our own (or
        // the one already showing it). Ending or resuming it is not on offer.
        if let Some(attach_id) = s.procs.iter().find_map(|p| p.background.as_ref()) {
            return Some(match self.hosts.iter().find(|h| h.attaches_to(attach_id)) {
                Some(h) => JumpPlan::Attach(h.claude_pid),
                None => JumpPlan::AttachBackground(attach_id.clone()),
            });
        }
        if let Some(p) = s.procs.iter().find(|p| p.hosted && !p.stale) {
            return Some(JumpPlan::Attach(p.pid));
        }
        let kind = if s.procs.len() == 1 && !s.procs[0].hosted { AdoptKind::TakeOver } else { AdoptKind::Reload };
        Some(JumpPlan::Adopt { pids: s.procs.iter().map(|p| p.pid).collect(), kind, busy: s.status == Status::Busy })
    }

    fn jump(&mut self) {
        let Some(plan) = self.jump_plan() else { return };
        let Some(s) = self.selected_session() else { return };
        let (id, name, cwd, restore) = (s.id.clone(), s.name.clone(), s.cwd.clone(), s.archived);
        match plan {
            JumpPlan::Attach(pid) => {
                self.attach(pid);
                if self.attached_pid().is_some() {
                    self.note_opened(&id, restore);
                }
            }
            JumpPlan::Resume => {
                if !self.cwd_exists(&name, &cwd) {
                    return;
                }
                if self.launch(cwd, &["--resume", &id]) {
                    self.note_opened(&id, restore);
                }
            }
            JumpPlan::AttachBackground(attach_id) => {
                if !self.cwd_exists(&name, &cwd) {
                    return;
                }
                let args = claude::attach_args(&attach_id);
                if self.launch(cwd, &[args[0].as_str(), args[1].as_str()]) {
                    self.note_opened(&id, restore);
                }
            }
            JumpPlan::Adopt { pids, kind, busy } => {
                self.mode = Mode::Confirm(Confirm::Adopt { pids, id, name, cwd, busy, kind, restore });
            }
        }
    }

    /// A conversation can only be resumed where it was run. Says so, rather than failing later, if that's gone.
    fn cwd_exists(&mut self, name: &str, cwd: &str) -> bool {
        if !cwd.is_empty() && Path::new(cwd).is_dir() {
            return true;
        }
        let msg = if cwd.is_empty() {
            format!("Can't open \"{name}\": its working directory isn't recorded")
        } else {
            format!("Can't open \"{name}\": {cwd} no longer exists")
        };
        self.notify(msg);
        false
    }

    /// End every live copy, wait for them to go, then resume the conversation under a platoon host.
    /// Reopening from the saved history means the new copy's memory matches it exactly.
    fn adopt(&mut self, pids: &[u32], id: &str, name: &str, cwd: String, restore: bool) {
        // Check first: ending the process and then failing to reopen it would be the worst outcome.
        if !self.cwd_exists(name, &cwd) {
            return;
        }
        if let Err(e) = host::end_processes(pids) {
            self.notify(format!("Couldn't end {name}: {e}. Try again in a moment."));
            return;
        }
        if self.launch(cwd, &["--resume", id]) {
            self.note_opened(id, restore);
        }
    }

    fn toggle_archive(&mut self) {
        let Some(s) = self.selected_session() else {
            self.notify("No conversation selected");
            return;
        };
        let (id, name) = (s.id.clone(), s.name.clone());
        if s.procs.iter().any(|p| p.background.is_some()) {
            // Archiving stops what runs the conversation, and this one is run by Claude's own service.
            self.notify(format!("\"{name}\" runs in the background, so platoon can't archive it"));
            return;
        }
        if s.archived {
            match self.archive.restore(&id, usage::now()) {
                Ok(()) => self.notify(format!("Restored \"{name}\"")),
                Err(e) => self.notify(format!("Couldn't save the archive: {e}")),
            }
            self.reload();
        } else if (s.status == Status::Busy && !s.procs.is_empty()) || s.procs.iter().any(|p| !p.hosted) {
            // Stopping it is destructive if it is mid-turn, or if it lives in a terminal platoon didn't start.
            let (busy, outside) = (s.status == Status::Busy, s.procs.iter().any(|p| !p.hosted));
            self.mode = Mode::Confirm(Confirm::Archive { id, name, busy, outside });
        } else {
            self.archive_conversation(&id, &name);
        }
    }

    /// Stop whatever runs the conversation (SIGTERM, so Claude shuts down cleanly) and move it to Archived.
    /// The transcript stays, so restoring it resumes from the saved history.
    fn archive_conversation(&mut self, id: &str, name: &str) {
        // Never a background session's process: Claude's own service owns it (the archive key refuses those rows).
        let pids: Vec<u32> =
            self.sessions.iter().filter(|s| s.id == id).flat_map(|s| s.procs.iter().filter(|p| p.background.is_none()).map(|p| p.pid)).collect();
        if !pids.is_empty() {
            if let Err(e) = host::end_processes(&pids) {
                self.notify(format!("Couldn't stop \"{name}\": {e}. Try again in a moment."));
                return;
            }
        }
        match self.archive.archive(id, usage::now()) {
            Ok(()) => self.notify(format!("Archived \"{name}\"")),
            Err(e) => self.notify(format!("Couldn't save the archive: {e}")),
        }
        self.reload();
    }

    /// The user opened a conversation. That counts as activity for auto-archive (opening one doesn't always
    /// write to it), and an archived one comes back to the Active tab.
    fn note_opened(&mut self, id: &str, archived: bool) {
        self.viewing = Some(id.to_string());
        let now = usage::now();
        let saved = if archived { self.archive.restore(id, now) } else { self.archive.touch(id, now) };
        if let Err(e) = saved {
            self.notify(format!("Couldn't save the archive: {e}"));
        }
        self.reload();
    }

    /// True when the attached session must not receive input: it fell out of sync with its conversation.
    pub fn attached_paused(&self) -> bool {
        let Some(pid) = self.attached_pid() else { return false };
        // Check the marker directly too, so a copy refused a moment ago is paused before the next refresh.
        self.sessions
            .iter()
            .flat_map(|s| &s.procs)
            .find(|p| p.pid == pid)
            .is_some_and(|p| p.stale || guard::is_stale(&self.cache, p.pid, p.started_at))
    }

    /// Start `args` under a new host in `dir` and attach to it. False if it couldn't be started.
    fn launch(&mut self, dir: String, args: &[&str]) -> bool {
        let (rows, cols) = self.pty_size();
        match host::launch(&self.program, &dir, args, rows, cols) {
            Ok((info, child)) => {
                let pid = info.claude_pid;
                self.spawned.push(child);
                self.hosts.push(info);
                self.filter.clear();
                self.attach(pid);
                true
            }
            Err(e) => {
                self.mode = Mode::List;
                self.notify(format!("Couldn't start claude: {e}"));
                false
            }
        }
    }

    #[cfg(test)]
    pub fn archive_writes_for_test(&self) -> usize {
        self.archive.saves()
    }

    /// Run auto-archive as if it were `now`, and update the tabs.
    #[cfg(test)]
    pub fn auto_archive_at_for_test(&mut self, now_secs: u64) {
        self.auto_archive(true, now_secs);
        self.rev += 1;
        self.update_view();
    }

    /// Let a pending settings change take effect at the next refresh, as if the settle time had passed.
    #[cfg(test)]
    pub fn settle_for_test(&mut self) {
        if self.apply_after.is_some() {
            self.apply_after = Some(Instant::now());
        }
    }

    #[cfg(test)]
    pub fn view_conversation_for_test(&mut self, id: &str, pid: u32) {
        self.viewing = Some(id.to_string());
        self.mode = Mode::Attached(pid);
    }

    #[cfg(test)]
    pub fn opened_for_test(&mut self, id: &str) {
        self.note_opened(id, false);
    }

    #[cfg(test)]
    pub fn select_theme_for_test(&mut self, i: usize) {
        self.select_theme(i);
    }

    #[cfg(test)]
    pub fn refresh_for_test(&mut self) {
        self.refresh_with(Vec::new(), false);
    }

    #[cfg(test)]
    pub fn reload_for_test(&mut self) {
        self.reload();
    }

    #[cfg(test)]
    pub fn picker_launch_for_test(&mut self, dir: String) {
        self.launch(dir, &[]);
    }

    fn move_selection(&mut self, delta: i32) {
        let last = self.view.len().saturating_sub(1) as i32;
        self.selected = (self.selected as i32 + delta).clamp(0, last) as usize;
    }

    fn move_picker(&mut self, delta: i32) {
        let last = self.picker.results.len().saturating_sub(1) as i32;
        self.picker.selected = (self.picker.selected as i32 + delta).clamp(0, last) as usize;
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        self.dismiss_long_notice();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match self.mode.clone() {
            Mode::Attached(_) => {
                // Typing moves the screen on (and the highlight would no longer be over what was selected).
                self.clear_selection();
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
            Mode::Settings => match key.code {
                KeyCode::Esc | KeyCode::Char('s' | 'q') => self.act(Action::Cancel),
                KeyCode::Char('c') if ctrl => self.act(Action::Cancel),
                KeyCode::Up => self.move_setting(-1),
                KeyCode::Down => self.move_setting(1),
                KeyCode::Home | KeyCode::Char('g') => self.setting = 0,
                KeyCode::End | KeyCode::Char('G') => self.setting = ITEMS.len() - 1,
                KeyCode::Left | KeyCode::Char('h' | '-') => self.change_setting(self.setting, Step::Prev),
                KeyCode::Right | KeyCode::Char('l' | '+' | '=') => self.change_setting(self.setting, Step::Next),
                KeyCode::Enter | KeyCode::Char(' ') => self.change_setting(self.setting, Step::Cycle),
                _ => {}
            },
            Mode::Themes => match key.code {
                KeyCode::Esc | KeyCode::Char('s' | 'q') => self.act(Action::Cancel),
                KeyCode::Char('c') if ctrl => self.act(Action::Cancel),
                KeyCode::Enter | KeyCode::Char(' ') => self.act(Action::Confirm),
                KeyCode::Up => self.move_theme(-1),
                KeyCode::Down => self.move_theme(1),
                KeyCode::PageUp => self.move_theme(-5),
                KeyCode::PageDown => self.move_theme(5),
                KeyCode::Home | KeyCode::Char('g') => self.select_theme(0),
                KeyCode::End | KeyCode::Char('G') => self.select_theme(self.tpick.list.len().saturating_sub(1)),
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
                KeyCode::Char('a') => self.act(Action::Archive),
                KeyCode::Char('/') => self.act(Action::Filter),
                KeyCode::Char('s') => self.act(Action::Settings),
                // 1 is the first tab, 2 the second; a number with no tab does nothing.
                KeyCode::Char(c @ '1'..='9') if !ctrl && !alt => self.switch_tab(c as usize - '1' as usize),
                KeyCode::Enter => self.act(Action::Jump),
                KeyCode::Esc => {
                    self.filter.clear();
                    self.selected = 0;
                }
                KeyCode::Down => self.move_selection(1),
                KeyCode::Up => self.move_selection(-1),
                KeyCode::Char('g') | KeyCode::Home => self.selected = 0,
                KeyCode::Char('G') | KeyCode::End => self.selected = self.view.len().saturating_sub(1),
                _ => {}
            },
        }
        self.update_view();
        self.clamp_selected();
    }

    pub fn on_paste(&mut self, text: &str) {
        match self.mode.clone() {
            Mode::Attached(_) if self.attached_paused() => {}
            Mode::Attached(_) => {
                self.clear_selection();
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
        self.update_view();
    }

    pub fn on_mouse(&mut self, m: MouseEvent) {
        self.mouse = (m.column, m.row);
        if matches!(m.kind, MouseEventKind::Down(_)) {
            self.dismiss_long_notice();
        }
        match m.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let up = m.kind == MouseEventKind::ScrollUp;
                match self.mode.clone() {
                    Mode::Attached(_) => {
                        // Scrolling changes which text is under the highlight, and a selection is only of what is
                        // on screen, so it is dropped.
                        self.clear_selection();
                        // A session that asked for mouse reports (Claude's full screen mode does) gets the wheel
                        // as a report and scrolls itself; ours would have nothing to scroll there. Never a paused
                        // one, which must not be written to.
                        let area = self.term_area();
                        let over_screen = area.contains((m.column, m.row).into());
                        let report = match &self.client {
                            Some(c) if over_screen && !self.attached_paused() => {
                                c.wheel_report(up, m.column - area.x + 1, m.row - area.y + 1)
                            }
                            _ => None,
                        };
                        if let Some(c) = self.client.as_mut() {
                            match report {
                                Some(bytes) => {
                                    c.reset_scroll();
                                    if !bytes.is_empty() {
                                        c.write(&bytes);
                                    }
                                }
                                None => c.scroll(if up { 3 } else { -3 }),
                            }
                        }
                    }
                    Mode::Picker => self.move_picker(if up { -1 } else { 1 }),
                    Mode::List | Mode::Filter => self.move_selection(if up { -1 } else { 1 }),
                    Mode::Settings => self.move_setting(if up { -1 } else { 1 }),
                    Mode::Themes => self.move_theme(if up { -1 } else { 1 }),
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
                    Some(Hit::Tab(i)) => self.switch_tab(i),
                    Some(Hit::Setting(i)) => self.setting = i,
                    Some(Hit::SettingStep(i, step)) => {
                        self.setting = i;
                        self.change_setting(i, step);
                    }
                    // Clicking the selected row again acts like Enter.
                    Some(Hit::Row(i)) if i == self.selected => self.act(Action::Jump),
                    Some(Hit::Row(i)) => self.selected = i,
                    Some(Hit::PickerRow(i)) if i == self.picker.selected => self.act(Action::Confirm),
                    Some(Hit::PickerRow(i)) => self.picker.selected = i,
                    // The previewed entry is the one that gets kept: clicking it again is "use this".
                    Some(Hit::ThemeRow(i)) if i == self.tpick.selected => self.act(Action::Confirm),
                    Some(Hit::ThemeRow(i)) => self.select_theme(i),
                    Some(Hit::Inert) => {}
                    // Nothing clickable here: in the attached view that is the session's own text.
                    None => self.begin_selection(m),
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => self.extend_selection(m),
            MouseEventKind::Up(MouseButton::Left) => self.finish_selection(),
            _ => {}
        }
        self.update_view();
    }

    fn clear_selection(&mut self) {
        self.selection = None;
        self.dragging = false;
    }

    /// A press on the session's text starts a selection (a double or triple click one by word or line). It only
    /// counts as a selection once it covers something: a plain click just clears the old one. Never on the status
    /// bar, the frame, or a session that is paused (its text is under the banner).
    fn begin_selection(&mut self, m: MouseEvent) {
        self.clear_selection();
        if !matches!(self.mode, Mode::Attached(_)) || self.client.is_none() || self.attached_paused() {
            return;
        }
        let area = self.term_area();
        if !area.contains((m.column, m.row).into()) {
            return;
        }
        let at = Point::new(m.row - area.y, m.column - area.x);
        let now = Instant::now();
        let count = click_count(self.last_click, now, at);
        self.last_click = Some(Click { at: now, point: at, count });
        let unit = match count {
            1 => Unit::Char,
            2 => Unit::Word,
            _ => Unit::Line,
        };
        self.selection = Some(Selection::new(at, unit));
        self.dragging = true;
    }

    /// Dragging past an edge of the session's screen sticks to that edge (no scrolling in v1).
    fn extend_selection(&mut self, m: MouseEvent) {
        if !self.dragging {
            return;
        }
        let area = self.term_area();
        let col = m.column.clamp(area.x, area.right().saturating_sub(1)) - area.x;
        let row = m.row.clamp(area.y, area.bottom().saturating_sub(1)) - area.y;
        if let Some(sel) = &mut self.selection {
            sel.head = Point::new(row, col);
        }
    }

    /// Letting go copies what is selected, once. A press that never moved (a click) copies nothing, and neither
    /// does a selection of only blanks: both are dropped.
    fn finish_selection(&mut self) {
        if !std::mem::take(&mut self.dragging) {
            return;
        }
        let (Some(sel), Some(client)) = (self.selection, &self.client) else { return };
        let text = {
            let parser = client.parser.lock().unwrap_or_else(|e| e.into_inner());
            sel.span(parser.screen()).map(|span| extract(parser.screen(), span))
        };
        match text {
            Some(text) if !text.is_empty() => {
                let outcome = self.clipboard.copy(&text);
                self.notify(outcome.notice());
            }
            _ => self.selection = None,
        }
    }

    #[cfg(test)]
    pub fn set_clipboard_for_test(&mut self, clipboard: Clipboard) {
        self.clipboard = clipboard;
    }
}
