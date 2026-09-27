use crate::ai;
use crate::archive::{self, Archive};
use crate::claude::{self, Live, Reader};
use crate::client::{key_bytes, Client};
use crate::clipboard::Clipboard;
use crate::dirs;
use crate::editors::Editor;
use crate::guard;
use crate::host::{self, HostInfo};
use crate::index::{Entry, Indexer};
use crate::ledger::Ledger;
use crate::model::{NameSource, Proc, Session, Status};
use crate::names::{self, Names};
use crate::projects::{self as projects_mod, Projects};
use crate::prompts::{self, PromptIndex};
use crate::select::{click_count, extract, Click, Point, Selection, Unit};
use crate::settings::{Item, Kind, Settings, SettingsStore, Step, TABS as SETTINGS_TABS};
use crate::setup;
use crate::summaries::Summaries;
use crate::theme::Theme;
use crate::usertheme::Registry;
use crate::usage::{self, Usage};
use fuzzy_matcher::{skim::SkimMatcherV2, FuzzyMatcher};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::{
    cell::{Cell, RefCell},
    cmp::Reverse,
    collections::HashMap,
    io,
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        mpsc::{channel, Receiver, Sender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

mod handoff_glue;
mod notes_glue;
mod projects_glue;
mod smart_glue;
mod squads_glue;
mod squadwork_glue;
mod squadtab_glue;
mod tickets_glue;
pub use handoff_glue::{HandoffBox, HandoffState};
pub use notes_glue::{Changes, EditorPick, NotesEdit, NotesWindow};
pub use projects_glue::{AssignOpt, AssignPick, LookEdit, NameBox, NamePurpose, PStat, Pane, PTABS, TOPS, TOP_PROJECTS, TOP_SESSIONS, TOP_SQUADS};
pub use smart_glue::{Engine, FindData, FindState, FIND_KIND};
pub use squads_glue::{split_areas, SquadState, SPLIT_MIN_WIDTH};
pub use squadtab_glue::{agent_line, agent_rows, back_words, detail_rows, ARow, ChoiceFor, DRow, Drafting, SqFocus, SqView, SquadName, SquadTab, TextFor, NEW_AGENT};
pub use tickets_glue::{PickKind, PickOpt, TRow, TicketsState};

const NOTICE_FOR: Duration = Duration::from_secs(5);
/// Automatic archiving is something the user didn't do, so its notice stays long enough to be read.
const AUTO_ARCHIVE_NOTICE_FOR: Duration = Duration::from_secs(60);
/// Auto-archive is also re-checked this often, so a conversation that turns old while platoon stays open is caught.
const AUTO_ARCHIVE_EVERY: Duration = Duration::from_secs(60);
/// After a change on the settings screen, auto-archive waits this long for the next one before acting. The
/// steps are one key press apart, so getting from 2 days to Off passes through 1 day, and archiving at that
/// stop would put away conversations the user was on the way to keeping.
const SETTLE: Duration = Duration::from_millis(1200);

/// How long quitting waits for stops that are still running (so their archive records get written) before leaving.
const QUIT_WAITS_FOR_STOPS: Duration = Duration::from_secs(3);

/// A conversation whose process is being stopped in the background because the user archived it.
struct Stopping {
    name: String,
    /// The processes signalled: exactly this conversation's own, never a background session's.
    pids: Vec<u32>,
}

/// What a stop worker reports back: the conversation and whether every one of its processes exited.
struct StopDone {
    id: String,
    result: Result<(), String>,
}

/// The two waiting times of stopping, held apart from the constants so tests can shorten them.
#[derive(Clone, Copy)]
struct StopTimes {
    /// How long a process gets to exit after SIGTERM.
    stop: Duration,
    /// How long quitting waits for stops in flight.
    quit: Duration,
}

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

/// The mark of a generated name is `✦`, which nearly every font has; on the Linux console and when
/// `PLATOON_GLYPHS=plain` it is a plain `*` (`PLATOON_GLYPHS=fancy` forces the `✦`).
pub fn detect_plain_marker_with(env: &dyn Fn(&str) -> Option<String>) -> bool {
    match env("PLATOON_GLYPHS").as_deref() {
        Some("plain" | "ascii") => true,
        Some("fancy") => false,
        _ => env("TERM").as_deref() == Some("linux"),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Jump,
    New,
    Archive,
    /// Open the search dialog (`/`).
    Search,
    Rename,
    Settings,
    Quit,
    Back,
    Confirm,
    Cancel,
    /// `m`: put the selected session in a project, another one, or none.
    Assign,
    /// `d` on the Projects tab: delete the selected project (asks first).
    Delete,
    /// `c` on the Projects tab: the project's color and badge.
    Look,
    /// Esc on a project's screen: back to the Projects tab.
    Leave,
    /// `e` in the notes pane: edit the project's notes.
    Edit,
    /// `E` in the notes pane: choose an editor for this once.
    /// `c` in the notes pane: what changed last.
    ShowChanges,
    /// `u` in the notes pane: undo the last change.
    Undo,
    /// `H` on a session that is in a project: write a brief and start a new session from it.
    HandOff,
}

/// Clickable regions, recorded by the UI each frame.
#[derive(Clone, Copy, Debug)]
pub enum Hit {
    Row(usize),
    PickerRow(usize),
    Action(Action),
    /// Active or Archived (of the Sessions tab, or of the Projects tab when that is showing).
    Tab(usize),
    /// A top-level tab: Sessions or Projects.
    Top(usize),
    /// A row of the Projects tab.
    ProjectRow(usize),
    /// A row of the Tickets pane of a project's screen.
    TicketRow(usize),
    /// An entry of a list open over a ticket (its state, folders, what it waits for).
    TicketPickRow(usize),
    /// An entry of the squad picker.
    SquadRow(usize),
    /// An entry of the Squads tab's list (of squads or of agents, whichever it shows).
    SquadEntry(usize),
    /// The Squads tab's list title: Squads (false) or Agents (true).
    SquadView(bool),
    /// A row of the selected squad's details.
    SquadDetail(usize),
    /// An entry of the small list open over the Squads tab.
    SquadChoiceRow(usize),
    /// An entry of the project picker (`m`).
    AssignRow(usize),
    /// An entry of the editor list.
    EditorRow(usize),
    /// A color of the color and badge box.
    Slot(usize),
    /// One of the two panes of a project's screen: gives it the focus.
    Pane(Pane),
    /// One kind of search in the search dialog: switches to it.
    SearchKind(usize),
    /// A row of the search dialog's list: selects it, and a click on the selected one takes it.
    SearchRow(usize),
    /// A tab of the settings screen: shows it.
    SettingsTab(usize),
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
    /// Archiving a project stops sessions of it that are running, and one of those is working or runs outside platoon.
    ArchiveProject { id: String, name: String, running: usize, busy: bool, outside: bool },
    /// Deleting a project always asks: its notes go too.
    DeleteProject { id: String, name: String, sessions: usize },
    /// Restoring platoon's default system prompt of an AI action (`r` in the settings) replaces the user's own.
    ResetInstructions { id: &'static str },
    /// `c` on a ticket: cancel it? `squad`: a squad has had it; `pid`: its leader's `claude`, when that is running
    /// (yes stops that session too).
    CancelTicket { number: u32, title: String, squad: bool, pid: Option<u32> },
    /// The commander said this squad doesn't fit the ticket, and the user picked it again: hand it over anyway?
    HandAnyway { number: u32, squad: String, title: String, says: String },
    /// Delete one of the user's own squads.
    DeleteSquad { id: String, name: String },
    /// Put a built-in squad the user changed back as platoon has it.
    RestoreSquad { id: String, name: String },
    /// Throw away the drafted squad that was never saved.
    DiscardDraft { name: String },
    /// Delete one of the user's own agents (no squad uses it).
    DeleteAgent { id: String, name: String },
    /// Put a built-in agent the user changed back as platoon has it.
    RestoreAgent { id: String, name: String },
    /// Throw away the drafted agent that was never saved.
    DiscardAgentDraft { name: String },
}

#[derive(Clone)]
pub enum Mode {
    List,
    /// The search dialog (`/`): a kind of search (`SEARCH_KINDS`), the words being typed and what they find.
    Search,
    /// Typing a new name for the selected conversation.
    Rename,
    Picker,
    Confirm(Confirm),
    /// The settings screen, which replaces the list.
    Settings,
    /// The theme list, opened from the Theme row of the settings. Moving through it previews each theme on the
    /// whole app; only `Enter` keeps one.
    Themes,
    /// Full-screen view of a platoon-hosted session, by claude's pid.
    Attached(u32),
    /// Typing the name of a new project, or a new name for one (`name_box`).
    ProjectName,
    /// The project picker of `m` (`assign`).
    Assign,
    /// The color and badge of a project (`look`).
    Look,
    /// A terminal editor running inside platoon's window on a project's notes (`editing`).
    EditNotes,
    /// The list of editors to open the notes with (`epick`).
    PickEditor,
    /// The before/after view of the last change to the notes (`changes`).
    Changes,
    /// The hand-off brief, waiting for Claude or waiting to be edited and accepted (`handoff`).
    Handoff,
    /// A list open over a ticket: its state, its folders or what it waits for (`tk.pick`).
    TicketPick,
    /// The squads a ticket can be handed to (`sq`).
    SquadPick,
    /// A small list over the Squads tab: a model, an agent, where a step sends the work back, what to describe
    /// (`sqt.choice`).
    SquadChoice,
    /// A paragraph typed on the Squads tab: a new squad described for Claude, what a squad can do, what a step does
    /// (`sqt.text`).
    SquadText,
}

pub struct Picker {
    pub input: String,
    all: Vec<String>,
    /// Absolute paths, best match first.
    pub results: Vec<String>,
    pub selected: usize,
    /// The project a new session started from here goes into.
    pub project: Option<String>,
    /// Picking a folder to add to this ticket (of the open project), rather than one to start a session in.
    pub ticket: Option<u32>,
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

/// One kind of search in the `/` dialog. A new kind is a new entry here (and, if it lists its own results, a new arm
/// in `App::run_search`): the dialog's chips, the Tab key and the clicks all follow.
pub struct SearchKind {
    pub label: &'static str,
    /// One line under the chips: what this search looks at.
    pub explain: &'static str,
    /// What the results area says before anything is typed.
    pub prompt: &'static str,
    /// It narrows the list behind the dialog as you type (and Enter keeps that), rather than listing results of its own.
    pub narrows_list: bool,
    /// The token feature this kind needs (its id in `ai::FEATURES`): the kind is only offered while the master switch
    /// and that feature's own switch are both on.
    pub feature: Option<&'static str>,
}

pub const SEARCH_KINDS: &[SearchKind] = &[
    SearchKind {
        label: "Quick filter",
        explain: "Narrows the list by name, folder, branch and description.",
        prompt: "Type to narrow the list.",
        narrows_list: true,
        feature: None,
    },
    SearchKind {
        label: "Full-text",
        explain: "Finds conversations by the words you typed in them, archived ones too.",
        prompt: "Type words you asked Claude about, in any order.",
        narrows_list: false,
        feature: None,
    },
    SearchKind {
        label: "Describe it",
        explain: "Describe it in your own words; Claude picks the likely ones.",
        prompt: "Describe what you remember, then press enter.",
        narrows_list: false,
        feature: Some("find"),
    },
];

/// One conversation a search found.
pub struct SearchResult {
    pub id: String,
    pub name: String,
    pub dir: String,
    pub archived: bool,
    pub last_active: u64,
    /// The text to show under the name: the matching message, or (when only the name matched) the description.
    pub detail: String,
    /// Where the words are in `detail`, as character ranges.
    pub marks: Vec<Range<usize>>,
}

/// The search dialog while it is open.
pub struct Search {
    /// Which of `SEARCH_KINDS` is showing.
    pub kind: usize,
    /// What has been typed. Kept when the kind changes.
    pub text: String,
    /// The conversations a kind that lists its own results found, best first.
    pub results: Vec<SearchResult>,
    /// How many matched before the limit on results was applied.
    pub total: usize,
    /// The highlighted result.
    pub selected: usize,
    /// The first result on screen, kept between frames so that the list only scrolls when it has to.
    pub top: Cell<usize>,
    /// What `results` was worked out from, so nothing is redone when nothing changed.
    computed_for: Option<(usize, String, u64, u64, bool, u64)>,
    /// The state of the "Describe it" search.
    pub find: FindData,
}

/// A conversation being renamed.
pub struct Rename {
    pub id: String,
    pub input: String,
    /// The name it had when the box opened: saving the same text changes nothing.
    original: String,
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
    /// The same rows before the filter: every conversation on this tab. The PROJECT column is sized from
    /// these, so its width doesn't jump about while the filter is being typed.
    tab_view: Vec<usize>,
    /// How many conversations each tab holds (unfiltered), parallel to `TABS`.
    pub counts: Vec<usize>,
    /// Which of `TABS` is showing.
    pub tab: usize,
    pub usage: Usage,
    /// Index into `visible()`.
    pub selected: usize,
    /// The words the list is narrowed by (the quick filter). Kept after the dialog closes, until Esc clears it.
    pub filter: String,
    pub mode: Mode,
    pub search: Search,
    pub rename: Rename,
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
    /// Projects and which sessions are in them (`projects.json`).
    pub projects: Projects,
    /// Which of `TOPS` is showing: Sessions or Projects.
    pub top: usize,
    /// Active (0) or Archived (1) on the Projects tab.
    pub ptab: usize,
    /// Indices into `projects.all()` shown on the Projects tab now.
    pview: Vec<usize>,
    /// Active and archived projects.
    pub pcounts: [usize; 2],
    /// Index into `project_rows()`.
    pub pselected: usize,
    /// Per project: its sessions, when it was last used, when its notes changed.
    pub pstats: HashMap<String, PStat>,
    /// The project whose screen is showing, if any. Its sessions are then what `visible()` lists.
    pub open_project: Option<String>,
    /// Which pane of the project's screen has the focus.
    pub pane: Pane,
    /// Sessions or Tickets: the one of the left column used last, which `←` goes back to.
    pub left_pane: Pane,
    /// The open project's tickets, the Tickets pane and a ticket's own screen.
    pub tk: TicketsState,
    /// Squads: the picker, the split view, and which sessions are squads' leaders.
    pub sq: SquadState,
    /// What the open project's squads have done (tokens, members' runs), read off the UI thread.
    pub sw: squadwork_glue::WorkState,
    /// The Squads tab: every squad, the one selected, a draft, and what is being changed.
    pub sqt: SquadTab,
    /// The first line of the notes in view.
    pub notes_top: Cell<usize>,
    /// The open project and the state of its notes file when it was last looked at, so a change made by the
    /// automatic update (or by anything else) while the screen is open is noticed and said once.
    pub notes_seen: Option<(String, crate::notes::Stamp)>,
    /// Why the open project's notes are not being kept up to date, in the words the pane shows. None while they are.
    notes_stopped: Option<String>,
    /// What the header says while nothing that spends tokens can run: the short form and the one with where to
    /// change it (`ai::header_note`).
    ai_note: Option<(String, String)>,
    /// How many of the rows of a project's screen are archived ones (they come last).
    pub archived_rows: usize,
    pub name_box: NameBox,
    pub assign: AssignPick,
    pub look: LookEdit,
    /// The editors this computer has (`editors.rs`), as they were when platoon started.
    editors: Vec<Editor>,
    /// A terminal editor platoon is running on a project's notes, inside its own window.
    pub editing: Option<NotesEdit>,
    /// An editor with a window of its own that platoon opened on a project's notes.
    notes_window: Option<NotesWindow>,
    pub epick: EditorPick,
    pub changes: Changes,
    /// The hand-off box (`h`), and the git reading and the call it is waiting for.
    pub handoff: HandoffBox,
    handoff_flight: Option<Receiver<handoff_glue::HandoffAnswer>>,
    /// The `git` the hand-off reads the folder with (a stand-in in tests).
    git_program: String,
    /// Draw the mark of a generated name (and the summary strip) as a plain `*`: for terminals whose font may lack `✦`.
    pub plain_marker: bool,
    /// Draw count badges with round caps. Needs glyphs only some terminals have, so it is off unless detected or asked for.
    pub rounded: bool,
    pub home: String,
    /// Where platoon keeps its own state (`~/.cache/platoon`).
    pub cache: PathBuf,
    /// Where platoon's own settings, archive, projects and notes live (`~/.config/platoon`).
    pub config: PathBuf,
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
    /// What the user typed in every conversation, for the full-text search. Step C's names and summaries come from it.
    prompts: PromptIndex,
    /// The prompt length `prompts` was last asked to keep. It follows the setting once the setting has stopped changing.
    prompt_cap: usize,
    archive: Archive,
    /// The names the user gave conversations (`names.json`).
    names: Names,
    /// Automatic clean-up settings, re-read when the file changes.
    settings: SettingsStore,
    /// The names and summaries platoon generated (`summaries.json`).
    summaries: Summaries,
    /// The naming worker and the description search's calls (see `smart_glue.rs`).
    smart: Engine,
    /// The tab of the settings screen showing, an index into `settings::TABS`.
    pub settings_tab: usize,
    /// The selected line of the settings screen, an index into the entries of that tab (`items()`).
    pub setting: usize,
    /// The first entry of the settings screen in view, when the entries don't all fit. Kept between frames so the list
    /// scrolls only as far as the selection needs, like the other lists.
    pub setting_top: Cell<usize>,
    /// The cancel flags of calls to Claude in flight (step C's features): a switch turned off in settings cancels them.
    switchboard: Arc<ai::Switchboard>,
    /// Tokens the smart features have used today, as the ledger says (shown in the settings).
    pub tokens_today: u64,
    /// The run of failures whose "paused" notice was already shown, so it appears once.
    ai_notice_seen: Option<String>,
    /// The conversation opened in the attached view: never auto-archived, even before its process registers.
    viewing: Option<String>,
    /// When auto-archive last ran (None: not yet).
    last_auto_archive: Option<Instant>,
    /// A settings change is waiting to take effect in auto-archive, which is on hold until then.
    apply_after: Option<Instant>,
    /// The program that runs a conversation: `claude`, or PLATOON_CLAUDE_BIN (a wrapper, or a stand-in in tests).
    program: String,
    guard_on: bool,
    /// What `view` and `counts` were computed from: (filter, tab, generation, top tab, projects tab, open
    /// project). Recomputed when it changes.
    view_for: (String, usize, u64, usize, usize, Option<String>),
    /// Bumped on every refresh.
    rev: u64,
    /// The index generation and live session ids `sessions` was built from.
    built: (u64, Vec<String>),
    /// Conversations the user archived whose process is still being stopped, by a worker thread each. They are left
    /// out of the list and of the counts at once, and archived only when the process has really exited.
    stopping: HashMap<String, Stopping>,
    stops_tx: Sender<StopDone>,
    stops_rx: Receiver<StopDone>,
    stop_times: StopTimes,
}

impl App {
    /// The real thing: rows appear as the background index reads them.
    pub fn new() -> Self {
        let reader = Reader::new();
        let cache = usage::cache_dir();
        let indexer = Indexer::spawn(reader.root(), &cache);
        let mut app = Self::build(reader, cache, archive::config_dir(), indexer, true);
        app.rounded = detect_rounded();
        app.plain_marker = detect_plain_marker_with(&|k| std::env::var(k).ok());
        app.clipboard = Clipboard::from_env();
        // Know the running hosts before the first frame: until then every session platoon hosts looks like a copy
        // running in another terminal, and Enter on it would offer to take it over instead of opening it.
        app.refresh();
        app
    }

    /// `cache` and `config` are where platoon keeps its own state; tests pass temp dirs so they never touch
    /// the real ones. The index is read on the calling thread, so tests see rows straight away.
    #[cfg(test)]
    pub fn with_dirs(reader: Reader, cache: PathBuf, config: PathBuf) -> Self {
        let indexer = Indexer::inline(reader.root(), &cache);
        Self::build(reader, cache, config, indexer, false)
    }

    /// `threaded`: the prompt index reads in the background (the real thing) rather than on the calling thread (tests).
    fn build(mut reader: Reader, cache: PathBuf, config: PathBuf, mut indexer: Indexer, threaded: bool) -> Self {
        // platoon's own `claude -p` helper runs in one folder of ours. It registers itself like any session, but it is
        // not a conversation: dropped here, at the two doors (running processes and saved transcripts), so nothing
        // downstream (rows, counts, search, naming, archive) ever sees it.
        let helper = claude::Hidden::new(&ai::work_dir(&cache));
        reader.hide(helper.clone());
        indexer.hide(helper);
        let settings = SettingsStore::open(&config);
        let prompt_cap = settings.get().search_prompt_chars as usize;
        #[cfg(test)]
        let prompts = if threaded { PromptIndex::spawn(&cache, prompt_cap) } else { PromptIndex::inline(&cache, prompt_cap) };
        #[cfg(not(test))]
        let prompts = PromptIndex::spawn(&cache, prompt_cap);
        let _ = threaded;
        let (stops_tx, stops_rx) = channel();
        let mut app = Self {
            sessions: Vec::new(),
            view: Vec::new(),
            tab_view: Vec::new(),
            counts: vec![0; TABS.len()],
            tab: 0,
            usage: Usage::NotSetUp,
            selected: 0,
            filter: String::new(),
            mode: Mode::List,
            search: Search { kind: 0, text: String::new(), results: Vec::new(), total: 0, selected: 0, top: Cell::new(0), computed_for: None, find: FindData::new() },
            rename: Rename { id: String::new(), input: String::new(), original: String::new() },
            picker: Picker { input: String::new(), all: Vec::new(), results: Vec::new(), selected: 0, project: None, ticket: None },
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
            projects: Projects::open(&config),
            top: TOP_SESSIONS,
            ptab: 0,
            pview: Vec::new(),
            pcounts: [0, 0],
            pselected: 0,
            pstats: HashMap::new(),
            open_project: None,
            pane: Pane::Sessions,
            left_pane: Pane::Sessions,
            tk: TicketsState::new(),
            sq: SquadState::new(),
            sw: squadwork_glue::WorkState::default(),
            sqt: SquadTab::new(&config),
            notes_top: Cell::new(0),
            notes_seen: None,
            notes_stopped: None,
            ai_note: None,
            archived_rows: 0,
            name_box: NameBox { purpose: NamePurpose::New, input: String::new() },
            assign: AssignPick { session: String::new(), session_name: String::new(), options: Vec::new(), selected: 0 },
            look: LookEdit { project: String::new(), slot: 0, badge: String::new() },
            editors: crate::editors::detect(),
            editing: None,
            notes_window: None,
            epick: EditorPick { project: String::new(), instructions: None, ticket: None, squad: None, options: Vec::new(), selected: 0, starred: String::new() },
            changes: Changes { lines: Vec::new(), top: Cell::new(0) },
            handoff: HandoffBox::empty(),
            handoff_flight: None,
            git_program: "git".into(),
            home: std::env::var("HOME").unwrap_or_default(),
            cache,
            config: config.clone(),
            tick: 0,
            theme: Theme::default(),
            preview: None,
            tpick: ThemePick { list: Vec::new(), selected: 0, top: Cell::new(0) },
            themes: Registry::open(&config),
            quit: false,
            rounded: false,
            plain_marker: false,
            hits: RefCell::new(Vec::new()),
            size: (80, 24),
            reader,
            indexer,
            prompts,
            prompt_cap,
            archive: Archive::open(&config),
            names: Names::open(&config),
            summaries: Summaries::open(&config),
            smart: Engine::new(),
            settings_tab: 0,
            settings,
            setting: 0,
            setting_top: Cell::new(0),
            switchboard: Arc::new(ai::Switchboard::new()),
            tokens_today: 0,
            ai_notice_seen: None,
            viewing: None,
            last_auto_archive: None,
            apply_after: None,
            program: std::env::var("PLATOON_CLAUDE_BIN").unwrap_or_else(|_| "claude".into()),
            guard_on: false,
            view_for: (String::new(), usize::MAX, 0, 0, 0, None),
            rev: 0,
            built: (0, Vec::new()),
            stopping: HashMap::new(),
            stops_tx,
            stops_rx,
            stop_times: StopTimes { stop: host::STOP_WAIT, quit: QUIT_WAITS_FOR_STOPS },
        };
        app.theme = app.settings.get().theme(&app.themes);
        // Real inputs (running hosts, the guard setting) come with `refresh` (`new` calls it at once); until then assume none.
        app.refresh_with(Vec::new(), false);
        app
    }

    pub fn settings(&self) -> &SettingsStore {
        &self.settings
    }

    /// The mark of a generated name.
    pub fn marker(&self) -> &'static str {
        if self.plain_marker { "*" } else { "✦" }
    }

    /// The mark on the editor used last. A plain `*` where `✦` may be missing from the font, for the same reason.
    pub fn star(&self) -> &'static str {
        if self.plain_marker { "*" } else { "★" }
    }

    /// Everything about the token features that follows the settings and the ledger: calls whose switch was turned
    /// off are cancelled, today's count is read for the settings screen, and a failure that paused the features is
    /// said once in the footer.
    fn sync_ai(&mut self) {
        let settings = self.settings.get();
        self.switchboard.apply(&settings);
        let now = usage::now();
        self.tokens_today = Ledger::open(&self.cache).today(now).tokens();
        if let Some((run, text)) = ai::backoff_notice(&settings, &self.cache, now) {
            if self.ai_notice_seen.as_deref() != Some(run.as_str()) {
                self.ai_notice_seen = Some(run);
                self.notify(text);
            }
        }
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

    /// Indices into `sessions` for every row on this tab (or project screen) before the quick filter.
    pub fn tab_rows(&self) -> &[usize] {
        &self.tab_view
    }

    /// Recompute the rows on screen if the filter, tab or data changed. Called after every event and
    /// refresh, and does nothing (one comparison) when nothing moved.
    fn update_view(&mut self) {
        let key = (self.filter.clone(), self.tab, self.rev, self.top, self.ptab, self.open_project.clone());
        if self.view_for == key {
            return;
        }
        self.view_for = key;
        let q = self.filter.to_lowercase();
        let shows = TABS[self.tab.min(TABS.len() - 1)].shows;
        // A conversation being stopped is on neither tab: it is on its way to Archived and can't be acted on meanwhile.
        let listed = |s: &Session| self.stopping.is_empty() || !self.stopping.contains_key(&s.id);
        self.pstats = self.project_stats();
        self.archived_rows = 0;
        if let Some(open) = self.open_project.clone() {
            // A project's screen: its sessions, the active ones first and then the archived ones.
            let mine = |s: &Session| self.projects.project_id_of(&s.id) == Some(open.as_str()) && listed(s);
            let (active, archived): (Vec<usize>, Vec<usize>) =
                self.sessions.iter().enumerate().filter(|(_, s)| mine(s)).map(|(i, _)| i).partition(|&i| !self.sessions[i].archived);
            self.tab_view = active.into_iter().chain(archived).collect();
            let view: Vec<usize> = self.tab_view.iter().copied().filter(|&i| q.is_empty() || self.sessions[i].haystack.contains(&q)).collect();
            self.archived_rows = view.iter().filter(|&&i| self.sessions[i].archived).count();
            self.view = view;
        } else if self.top == TOP_PROJECTS || self.top == TOP_SQUADS {
            self.tab_view.clear();
            self.view.clear();
        } else {
            self.tab_view = self.sessions.iter().enumerate().filter(|(_, s)| listed(s) && shows(s)).map(|(i, _)| i).collect();
            let view: Vec<usize> = self.tab_view.iter().copied().filter(|&i| q.is_empty() || self.sessions[i].haystack.contains(&q)).collect();
            self.view = view;
        }
        self.counts = TABS.iter().map(|t| self.sessions.iter().filter(|s| listed(s) && (t.shows)(s)).count()).collect();
        self.update_project_rows(&q);
        self.clamp_selected();
    }

    pub fn selected_session(&self) -> Option<&Session> {
        self.view.get(self.selected).map(|&i| &self.sessions[i])
    }

    fn clamp_selected(&mut self) {
        self.selected = self.selected.min(self.view.len().saturating_sub(1));
        self.pselected = self.pselected.min(self.pview.len().saturating_sub(1));
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

    /// (rows, cols) of the terminal area inside the attached view (beside a ticket, in the split view).
    fn pty_size(&self) -> (u16, u16) {
        if self.sq.split.is_some() {
            return squads_glue::split_size(self.size);
        }
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
        if let Some(e) = &mut self.editing {
            e.run.resize(rows, cols);
        }
        self.clear_selection();
    }

    /// Where the session's screen is drawn in the attached view: inside the frame and above the status bar.
    /// The same numbers as `pty_size` and `ui::draw_attached`.
    pub fn term_area(&self) -> Rect {
        if self.sq.split.is_some() {
            return split_areas(self.size.0, self.size.1).term;
        }
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
                // A squad's session that ended leaves its ticket showing, with the way to continue it.
                if self.sq.split.take().is_some() {
                    self.notify("The squad's session ended.");
                } else {
                    self.notify("Session ended");
                }
            }
        }

        // What we knew about running conversations, for spotting busy -> idle across the rebuild.
        let prev: HashMap<String, (Status, bool)> =
            self.sessions.iter().filter(|s| !s.procs.is_empty()).map(|s| (s.id.clone(), (s.status, s.unread))).collect();
        let selected_id = self.selected_session().map(|s| s.id.clone());

        let index_changed = self.indexer.poll();
        self.archive.reload_if_changed();
        // A name given in another platoon window shows here too.
        if self.names.reload_if_changed() {
            self.built.0 = u64::MAX;
        }
        // Squads' leader sessions belong to their tickets: from here on nothing sees them (rows, counts, search,
        // naming, archiving, "Take over?"), because they are never in `live` or `sessions`.
        if self.refresh_leaders() {
            self.built.0 = u64::MAX;
        }
        let all_live = self.reader.live();
        // Which leaders run, for the squads at work at once; then they are left out like everywhere else.
        self.note_running_leaders(&all_live);
        let live: Vec<Live> = all_live.into_iter().filter(|l| !self.sq.leaders_contains(&l.id)).collect();
        self.projects.reload_if_changed();
        // Sessions started under a project are placed in it once their id shows up.
        let ids: Vec<(u32, String, u64)> = live.iter().filter(|l| !l.is_background()).map(|l| (l.pid, l.id.clone(), l.started_at)).collect();
        // A session platoon started under a project is placed here; that is a membership change like any other,
        // so its project is told there is something new (the same as pressing `m`).
        for (session, project) in projects_mod::resolve_launches(&self.cache, &mut self.projects, &ids, &claude::pid_alive) {
            self.joined_project(&project, &session);
        }
        if self.open_project.as_ref().is_some_and(|p| self.projects.get(p).is_none()) {
            self.open_project = None; // deleted in another window
        }
        // A ticket added by `platoon ticket add` shows without a restart.
        self.refresh_tickets();

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
            // The name the user gave it in platoon wins over the one Claude was told.
            if let (Some(n), None) = (ls.iter().find_map(|l| l.name.as_ref()), self.names.get(&s.id)) {
                s.name.clone_from(n);
                s.name_source = NameSource::Claude;
                s.rehaystack();
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
        // After the settings were re-read (auto_archive does that first), so an edit of the file is seen at once.
        self.sync_ai();
        self.poll_notes_status();
        self.refresh_prompts();
        self.poll_handoff();
        self.poll_squad_starts();
        self.start_from_line();
        self.poll_squad_work();
        self.poll_squad_files();
        self.poll_squad_draft();
        self.drive_smart();
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

    /// Tell the prompt index which transcripts there are (when the conversation index has news), and take in what it
    /// has read. A new prompt length is only handed on once the settings screen has stopped changing it: every
    /// step on the way would otherwise read everything again.
    fn refresh_prompts(&mut self) {
        if self.apply_after.is_none() {
            self.prompt_cap = self.settings.get().search_prompt_chars as usize;
        }
        let complete = self.indexer.first_scan_done && self.indexer.remaining == 0;
        self.prompts.refresh(&self.indexer.entries, self.indexer.generation, self.prompt_cap, complete);
        if matches!(self.mode, Mode::Search) {
            self.update_search();
        }
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
        let mut ids = archive::auto_archive_ids(&self.sessions, &self.archive, now_secs, age, viewing);
        // One being stopped for the archive key is archived by that (once its process is gone), not counted here.
        ids.retain(|id| !self.stopping.contains_key(id));
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
        for e in self.indexer.entries.values().filter(|e| !self.sq.leaders_contains(&e.id)) {
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
        // Claude Code derives a placeholder name; only a name the user chose (set from the registry) replaces this, and
        // a name the user gave it in platoon replaces both.
        let generated = self.summaries.get(&id);
        let (name, name_source) = match (self.names.get(&id), generated) {
            (Some(n), _) => (n.to_string(), NameSource::Platoon),
            (None, Some(g)) => (g.name.clone(), NameSource::Generated),
            (None, None) => (Path::new(&cwd).file_name().and_then(|n| n.to_str()).filter(|n| !n.is_empty()).unwrap_or(&cwd).to_string(), NameSource::Folder),
        };
        let summary = generated.map(|g| g.summary.clone()).unwrap_or_default();
        let dir = dirs::tilde(&cwd, &self.home);
        let haystack = Session::make_haystack(&name, &dir, branch.as_deref(), &description, &summary);
        Session {
            id,
            name,
            name_source,
            summary,
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
            (Action::New, Mode::List) if self.projects_list_showing() => self.open_name_box(NamePurpose::New),
            (Action::New, Mode::List) => {
                let project = self.open_project.clone();
                self.open_picker_in(project);
            }
            (Action::Archive, Mode::List) if self.projects_list_showing() => self.toggle_project_archive(),
            (Action::Archive, Mode::List) => self.toggle_archive(),
            (Action::Search, Mode::List) => self.open_search(),
            (Action::Rename, Mode::List) if self.projects_list_showing() => self.open_project_rename(),
            (Action::Rename, Mode::List) => self.open_rename(),
            (Action::Settings, Mode::List) => self.open_settings(),
            (Action::Jump, Mode::List) if self.projects_list_showing() => self.open_project_screen(),
            (Action::Jump, Mode::List) => self.jump(),
            (Action::Assign, Mode::List) if !self.projects_list_showing() => self.open_assign(),
            (Action::Delete, Mode::List) if self.projects_list_showing() => self.ask_delete_project(),
            (Action::Look, Mode::List) if self.projects_list_showing() => self.open_look(),
            (Action::Leave, Mode::List) if self.tk.open.is_some() => self.close_ticket_screen(),
            (Action::Leave, Mode::List) => self.close_project_screen(),
            (Action::Confirm, Mode::TicketPick) => self.take_ticket_pick(),
            (Action::Confirm, Mode::SquadPick) => self.take_squad_pick(),
            (Action::Confirm, Mode::Confirm(Confirm::CancelTicket { number, pid, .. })) => self.cancel_ticket(number, pid),
            (Action::Confirm, Mode::Confirm(Confirm::HandAnyway { number, squad, .. })) => self.hand_anyway(number, &squad),
            (Action::Confirm, Mode::Confirm(Confirm::DeleteSquad { id, .. })) => self.remove_squad(&id, false),
            (Action::Confirm, Mode::Confirm(Confirm::RestoreSquad { id, .. })) => self.remove_squad(&id, true),
            (Action::Confirm, Mode::Confirm(Confirm::DiscardDraft { .. })) => self.discard_draft(),
            (Action::Confirm, Mode::Confirm(Confirm::DeleteAgent { id, .. })) => self.remove_agent(&id, false),
            (Action::Confirm, Mode::Confirm(Confirm::RestoreAgent { id, .. })) => self.remove_agent(&id, true),
            (Action::Confirm, Mode::Confirm(Confirm::DiscardAgentDraft { .. })) => self.discard_agent_draft(),
            (Action::Confirm, Mode::SquadChoice) => self.take_squad_choice(),
            (Action::Edit, Mode::List) => self.edit_notes(),
            (Action::ShowChanges, Mode::List) => self.show_changes(),
            (Action::Undo, Mode::List) => self.undo_notes(),
            (Action::HandOff, Mode::List) if !self.projects_list_showing() => self.start_handoff(),
            (Action::Confirm, Mode::Handoff) => self.take_handoff(),
            (Action::Cancel, Mode::Handoff) => self.cancel_handoff(),
            (Action::Confirm, Mode::PickEditor) => self.take_editor_pick(),
            (Action::Confirm, Mode::ProjectName) => self.save_name_box(),
            (Action::Confirm, Mode::Assign) => self.take_assign(),
            (Action::Confirm, Mode::Look) => self.save_look(),
            (Action::Confirm, Mode::Confirm(Confirm::ArchiveProject { id, .. })) => {
                self.mode = Mode::List;
                self.archive_project(&id);
            }
            (Action::Confirm, Mode::Confirm(Confirm::ResetInstructions { id })) => self.reset_instructions(id),
            (Action::Cancel, Mode::Confirm(Confirm::ResetInstructions { .. })) => self.mode = Mode::Settings,
            (Action::Cancel, Mode::PickEditor) if self.epick.instructions.is_some() => self.mode = Mode::Settings,
            (Action::Confirm, Mode::Confirm(Confirm::DeleteProject { id, .. })) => {
                self.mode = Mode::List;
                self.delete_project(&id);
            }
            (Action::Back, Mode::Attached(_)) => self.detach(),
            (Action::Confirm, Mode::Confirm(Confirm::Archive { id, name, .. })) => {
                self.mode = Mode::List;
                self.archive_conversation(&id, &name);
            }
            (Action::Confirm, Mode::Confirm(Confirm::Adopt { pids, id, name, cwd, restore, .. })) => {
                self.mode = Mode::List;
                self.adopt(&pids, &id, &name, cwd, restore);
            }
            (Action::Confirm, Mode::Picker) if self.picker.ticket.is_some() => {
                if let (Some(dir), Some(n)) = (self.picker.results.get(self.picker.selected).cloned(), self.picker.ticket) {
                    self.add_ticket_folder(n, dir);
                }
            }
            // Leaving the folder picker of a ticket goes back to its folders.
            (Action::Cancel, Mode::Picker) if self.picker.ticket.is_some() => self.mode = Mode::TicketPick,
            (Action::Confirm, Mode::Picker) => {
                if let Some(dir) = self.picker.results.get(self.picker.selected).cloned() {
                    let project = self.picker.project.clone();
                    self.launch_in(dir, &[], project.as_deref(), true);
                }
            }
            (Action::Confirm, Mode::Themes) => self.keep_theme(),
            (Action::Cancel, Mode::Themes) => self.close_theme_list(),
            (Action::Confirm, Mode::Search) => self.confirm_search(),
            (Action::Cancel, Mode::Search) => self.cancel_search(),
            (Action::Confirm, Mode::Rename) => self.save_rename(),
            (Action::Cancel, _) => self.mode = Mode::List,
            _ => {}
        }
        self.update_view();
    }

    /// Open the search dialog on the quick filter, with what the list is already narrowed by (if anything) typed in.
    fn open_search(&mut self) {
        self.search = Search {
            kind: 0,
            text: self.filter.clone(),
            results: Vec::new(),
            total: 0,
            selected: 0,
            top: Cell::new(0),
            computed_for: None,
            find: FindData::new(),
        };
        self.mode = Mode::Search;
        self.update_search();
    }

    /// Show another kind of search with the same words. The list behind is narrowed only while a kind that narrows
    /// it is showing.
    pub fn set_search_kind(&mut self, kind: usize) {
        if kind >= SEARCH_KINDS.len() || kind == self.search.kind {
            return;
        }
        // What the description search asked belongs to it: leaving it drops the question, and its answers with it.
        if self.search.kind == FIND_KIND || kind == FIND_KIND {
            self.cancel_find();
            self.search.find = FindData::new();
            self.search.results.clear();
            self.search.total = 0;
            self.search.computed_for = None;
        }
        self.search.kind = kind;
        self.search.selected = 0;
        self.selected = 0;
        self.search.top.set(0);
        self.update_search();
    }

    fn step_search_kind(&mut self, delta: i32) {
        let kinds = self.search_kinds();
        let at = kinds.iter().position(|&k| k == self.search.kind).unwrap_or(0) as i32;
        self.set_search_kind(kinds[(at + delta).rem_euclid(kinds.len() as i32) as usize]);
    }

    /// The words changed: narrow the list, or work out the results.
    fn search_text_changed(&mut self) {
        self.selected = 0;
        self.search.selected = 0;
        self.search.top.set(0);
        if self.search.kind == FIND_KIND {
            self.find_text_changed();
        }
        self.update_search();
    }

    /// Bring what the dialog shows up to date with its words, the conversations, and the settings that shape it.
    fn update_search(&mut self) {
        let kind = &SEARCH_KINDS[self.search.kind.min(SEARCH_KINDS.len() - 1)];
        if kind.narrows_list {
            self.filter.clone_from(&self.search.text);
            self.search.computed_for = None;
            return;
        }
        self.filter.clear();
        // "Describe it" lists what Claude answered, never something worked out on every keystroke.
        if self.search.kind == FIND_KIND {
            return;
        }
        let settings = self.settings.get();
        let key = (self.search.kind, self.search.text.clone(), self.prompts.generation, self.rev, settings.search_archived, settings.search_results);
        if self.search.computed_for.as_ref() == Some(&key) {
            return;
        }
        self.search.computed_for = Some(key);
        let (results, total) = self.run_search(&settings);
        self.search.results = results;
        self.search.total = total;
        self.search.selected = self.search.selected.min(self.search.results.len().saturating_sub(1));
    }

    /// The conversations whose words match, best first, at most `search_results` of them, and how many there were.
    /// Best: the name the user gave has all the words, then one message has all of them, then more messages
    /// mention them, then the most recently used.
    fn run_search(&self, settings: &crate::settings::Settings) -> (Vec<SearchResult>, usize) {
        let words = prompts::words(&self.search.text);
        if words.is_empty() {
            return (Vec::new(), 0);
        }
        let mut found: Vec<(&Session, prompts::Found, Option<&Arc<prompts::FilePrompts>>)> = Vec::new();
        for s in &self.sessions {
            if (s.archived && !settings.search_archived) || self.stopping.contains_key(&s.id) || !self.in_scope(s) {
                continue;
            }
            // The name you gave and the one platoon made up both count as the name; the summary is searched too.
            let name = format!("{} {}", self.names.get(&s.id).unwrap_or(""), self.summaries.get(&s.id).map_or("", |g| g.name.as_str())).to_lowercase();
            let summary = s.summary.to_lowercase();
            let data = self.prompts.get(&s.id);
            if let Some(f) = prompts::find_with(data.map(|d| &**d), &name, &summary, &words) {
                found.push((s, f, data));
            }
        }
        found.sort_by(|a, b| {
            let key = |x: &(&Session, prompts::Found, _)| (Reverse(x.1.in_name), Reverse(x.1.in_one_prompt), Reverse(x.1.hits), Reverse(x.0.last_active));
            key(a).cmp(&key(b)).then_with(|| a.0.id.cmp(&b.0.id))
        });
        let total = found.len();
        let results = found
            .into_iter()
            .take(settings.search_results as usize)
            .map(|(s, f, data)| {
                let detail = match (f.prompt, data) {
                    (Some(i), Some(d)) => d.prompts[i].clone(),
                    _ if !s.summary.is_empty() => s.summary.clone(),
                    _ => s.description.clone(),
                };
                let marks = prompts::spans(&detail, &words);
                SearchResult { id: s.id.clone(), name: s.name.clone(), dir: s.dir.clone(), archived: s.archived, last_active: s.last_active, detail, marks }
            })
            .collect();
        (results, total)
    }

    /// Enter in the dialog: the quick filter keeps its words and closes; a kind with results takes the highlighted one.
    fn confirm_search(&mut self) {
        if SEARCH_KINDS[self.search.kind].narrows_list {
            self.mode = Mode::List;
        } else if self.search.kind == FIND_KIND {
            self.confirm_find();
        } else if let Some(id) = self.search.results.get(self.search.selected).map(|r| r.id.clone()) {
            self.show_conversation(&id);
        }
    }

    /// Esc in the dialog: closes it, and nothing stays narrowed.
    fn cancel_search(&mut self) {
        // In the description search the first esc calls off what it has out; only then does it close the dialog.
        if self.search.kind == FIND_KIND && self.cancel_find_step() {
            return;
        }
        self.cancel_find();
        self.filter.clear();
        self.selected = 0;
        self.pselected = 0;
        self.mode = Mode::List;
    }

    /// Close the dialog and select a conversation in the list: on the tab it belongs to, with nothing narrowing the
    /// list (a filter would hide it).
    fn show_conversation(&mut self, id: &str) {
        self.mode = Mode::List;
        let Some(s) = self.sessions.iter().find(|s| s.id == id && !self.stopping.contains_key(&s.id)) else { return };
        let Some(tab) = TABS.iter().position(|t| (t.shows)(s)) else { return };
        self.filter.clear();
        // On a project's screen the search only looked at its sessions, and both groups are on that screen.
        if self.open_project.is_none() {
            self.tab = tab;
            self.top = TOP_SESSIONS;
        }
        self.focus_pane(Pane::Sessions);
        self.tk.open = None;
        self.update_view();
        if let Some(pos) = self.view.iter().position(|&i| self.sessions[i].id == id) {
            self.selected = pos;
        }
    }

    /// The word index is still being filled in, so a search may miss things for now.
    pub fn search_catching_up(&self) -> bool {
        self.prompts.catching_up()
    }

    /// The highlighted row of the dialog: in the list behind for a kind that narrows it, else among its results.
    pub fn search_selected(&self) -> usize {
        if self.projects_list_showing() {
            self.pselected
        } else if SEARCH_KINDS[self.search.kind].narrows_list {
            self.selected
        } else {
            self.search.selected
        }
    }

    /// The quick filter lists nothing until something is typed (an empty filter narrows nothing, so the rows would
    /// only be the whole list again). The results of the other kinds are empty by themselves then.
    pub fn search_rows_hidden(&self) -> bool {
        SEARCH_KINDS[self.search.kind.min(SEARCH_KINDS.len() - 1)].narrows_list && self.search.text.is_empty()
    }

    fn select_search_row(&mut self, i: usize) {
        if self.projects_list_showing() {
            self.pselected = i;
        } else if SEARCH_KINDS[self.search.kind].narrows_list {
            self.selected = i;
        } else {
            self.search.selected = i;
        }
    }

    fn move_search(&mut self, delta: i32) {
        // Nothing is listed, so there is nothing to move over (and the list behind must not move unseen).
        if self.search_rows_hidden() {
            return;
        }
        if self.projects_list_showing() {
            self.move_project(delta);
        } else if SEARCH_KINDS[self.search.kind].narrows_list {
            self.move_selection(delta);
        } else {
            let last = self.search.results.len().saturating_sub(1) as i32;
            self.search.selected = (self.search.selected as i32 + delta).clamp(0, last) as usize;
        }
    }

    /// Open the box for renaming the selected conversation, with the name it shows now.
    fn open_rename(&mut self) {
        let Some(s) = self.selected_session() else {
            self.notify("No conversation selected");
            return;
        };
        self.rename = Rename { id: s.id.clone(), input: s.name.clone(), original: s.name.clone() };
        self.mode = Mode::Rename;
    }

    /// Enter in the rename box. The same text as before changes nothing; an empty one gives the conversation back its
    /// automatic name.
    fn save_rename(&mut self) {
        self.mode = Mode::List;
        let Rename { id, input, original } = std::mem::replace(&mut self.rename, Rename { id: String::new(), input: String::new(), original: String::new() });
        let text = names::clean(&input);
        let has_own = self.names.get(&id).is_some();
        if text == names::clean(&original) || (text.is_empty() && !has_own) {
            return;
        }
        match self.names.set(&id, &text) {
            Ok(()) if text.is_empty() => self.notify("Name cleared"),
            Ok(()) => self.notify(format!("Renamed to \"{text}\"")),
            Err(e) => {
                let why = e.to_string();
                self.notify(format!("Couldn't save the name: {}", why.split(" (os error").next().unwrap_or(&why)));
            }
        }
        self.built.0 = u64::MAX; // rows are built again with the new name
        self.reload();
    }

    /// The folder picker for a new session; under a project, the folders its other sessions use come first.
    fn open_picker_in(&mut self, project: Option<String>) {
        let mut all = project.as_deref().map(|p| self.project_folders(p)).unwrap_or_default();
        all.extend(dirs::candidates(self.reader.root(), &self.home));
        let mut seen = std::collections::HashSet::new();
        all.retain(|p| seen.insert(p.clone()));
        self.picker = Picker { input: String::new(), all, results: Vec::new(), selected: 0, project, ticket: None };
        self.picker.update(&self.home);
        self.mode = Mode::Picker;
    }

    /// Show the settings screen, starting from what is on disk now.
    fn open_settings(&mut self) {
        self.refresh_settings();
        self.settings_tab = 0;
        self.setting = 0;
        self.setting_top.set(0);
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

    /// The entries of the settings tab showing.
    pub fn items(&self) -> &'static [Item] {
        &SETTINGS_TABS[self.settings_tab.min(SETTINGS_TABS.len() - 1)].items
    }

    /// Show another tab of the settings, on its first entry.
    pub(crate) fn switch_settings_tab(&mut self, i: usize) {
        if i >= SETTINGS_TABS.len() || i == self.settings_tab {
            return;
        }
        self.settings_tab = i;
        self.setting = 0;
        self.setting_top.set(0);
        self.notice = None;
    }

    /// Tab and Shift+Tab: the next or the previous tab, round from the last to the first.
    fn step_settings_tab(&mut self, delta: i32) {
        let n = SETTINGS_TABS.len() as i32;
        self.switch_settings_tab((self.settings_tab as i32 + delta).rem_euclid(n) as usize);
    }

    /// Move to another line of the settings screen.
    fn move_setting(&mut self, delta: i32) {
        self.setting = (self.setting as i32 + delta).clamp(0, self.items().len() as i32 - 1) as usize;
    }

    /// Change one setting a step and save it at once. What the screen shows afterwards is read back from
    /// the file, so a save that failed leaves the old value on screen with a notice saying why.
    fn change_setting(&mut self, i: usize, step: Step) {
        let Some(item) = self.items().get(i) else { return };
        self.notice = None;
        if let Kind::Instructions(id) = item.kind {
            // Enter, space and a click on the value open the editor list; the arrows have nothing to step through.
            if step == Step::Cycle {
                self.edit_instructions(id);
            }
            return;
        }
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
        let old = item.value(&next);
        let new = item.step(old, step);
        if new == old {
            return;
        }
        item.assign(&mut next, new);
        let saved = self.settings.update(&next);
        self.sync_theme();
        self.sync_ai();
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

    /// Leave the attached view. Only our connection closes; the session keeps running. From the split view, that is
    /// back to the project's screen, on the ticket.
    fn detach(&mut self) {
        self.client = None;
        self.mode = Mode::List;
        self.clear_selection();
        if self.sq.split.take().is_some() {
            self.close_ticket_screen();
        }
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

    /// What Enter does on the selected row (see `jump_plan_of`).
    #[cfg(test)]
    pub fn jump_plan(&self) -> Option<JumpPlan> {
        self.jump_plan_of(self.selected_session()?)
    }

    /// Decide what Enter does on a conversation. Nothing running it: resume it from the saved history.
    /// A copy that platoon hosts and that is still in sync is simply attached to. Anything else (another
    /// terminal's copy, several copies, or a copy that fell out of sync) means ending the process(es)
    /// and reopening from the saved history, which is destructive, so it is confirmed first.
    fn jump_plan_of(&self, s: &Session) -> Option<JumpPlan> {
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
        let Some(id) = self.selected_session().map(|s| s.id.clone()) else { return };
        self.jump_to(&id);
    }

    /// Enter on a conversation, wherever it was picked (a row, or the "From" line of a ticket).
    pub(crate) fn jump_to(&mut self, id: &str) {
        let Some(s) = self.sessions.iter().find(|s| s.id == id) else { return };
        let Some(plan) = self.jump_plan_of(s) else { return };
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
                let project = self.projects.project_id_of(&id).map(String::from);
                if self.launch_in(cwd, &["--resume", &id], project.as_deref(), false) {
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
        let project = self.projects.project_id_of(id).map(String::from);
        if self.launch_in(cwd, &["--resume", id], project.as_deref(), false) {
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
            // It stays in its project; the project, archived or not, stays as it is.
            let _ = self.projects.session_restored(&id);
            self.reload();
        } else if (s.status == Status::Busy && !s.procs.is_empty()) || s.procs.iter().any(|p| !p.hosted) {
            // Stopping it is destructive if it is mid-turn, or if it lives in a terminal platoon didn't start.
            let (busy, outside) = (s.status == Status::Busy, s.procs.iter().any(|p| !p.hosted));
            self.mode = Mode::Confirm(Confirm::Archive { id, name, busy, outside });
        } else {
            self.archive_conversation(&id, &name);
        }
    }

    /// Move the conversation to Archived, stopping whatever runs it first (SIGTERM, so Claude shuts down cleanly).
    /// The transcript stays, so restoring it resumes from the saved history.
    ///
    /// Nothing running it: archived at once. Something running it: the row leaves the list and the counts at once, a
    /// worker thread stops the process(es) and waits for them to exit, and only then (`stop_done`) is the archive
    /// written. So a conversation is never archived while its process still runs, and the screen never waits for one.
    fn archive_conversation(&mut self, id: &str, name: &str) {
        if self.stopping.contains_key(id) {
            return; // already on its way
        }
        // Never a background session's process: Claude's own service owns it (the archive key refuses those rows).
        let pids: Vec<u32> =
            self.sessions.iter().filter(|s| s.id == id).flat_map(|s| s.procs.iter().filter(|p| p.background.is_none()).map(|p| p.pid)).collect();
        if pids.is_empty() {
            self.record_archived(id, name);
            self.reload();
            return;
        }
        let (tx, wait, worker_pids) = (self.stops_tx.clone(), self.stop_times.stop, pids.clone());
        let worker_id = id.to_string();
        let started = thread::Builder::new().name("platoon-stop".into()).spawn(move || {
            let result = host::end_processes_within(&worker_pids, wait);
            let _ = tx.send(StopDone { id: worker_id, result });
        });
        if let Err(e) = started {
            self.notify(format!("Couldn't stop \"{name}\": {e}. Try again in a moment."));
            return;
        }
        self.stopping.insert(id.to_string(), Stopping { name: name.to_string(), pids });
        self.rev += 1;
        self.update_view();
    }

    /// Write the archive record and say so (or say why it couldn't be saved).
    fn record_archived(&mut self, id: &str, name: &str) {
        match self.archive.archive(id, usage::now()) {
            Ok(()) => self.notify(format!("Archived \"{name}\"")),
            Err(e) => self.notify(format!("Couldn't save the archive: {e}")),
        }
    }

    /// Take in what the stop workers have finished. Cheap when none are running; the event loop calls it every turn.
    pub fn poll_stops(&mut self) {
        if self.stopping.is_empty() {
            return;
        }
        let mut any = false;
        while let Ok(done) = self.stops_rx.try_recv() {
            any |= self.stop_done(done);
        }
        if any {
            self.reload();
        }
    }

    /// One worker finished. Its process(es) exited: write the archive record (unless the conversation was opened
    /// again in the meantime, from another terminal, which makes it in use: it stays in the list). They didn't:
    /// the row comes back with the plain notice. True when something changed that the list must be rebuilt for.
    fn stop_done(&mut self, done: StopDone) -> bool {
        let Some(Stopping { name, pids }) = self.stopping.remove(&done.id) else { return false };
        match done.result {
            Ok(()) => {
                let again = self.reader.live().iter().any(|l| l.id == done.id && l.background.is_none() && !pids.contains(&l.pid));
                if again {
                    self.notify(format!("\"{name}\" was opened again, so it stays in the list."));
                } else {
                    self.record_archived(&done.id, &name);
                }
            }
            Err(e) => self.notify(format!("Couldn't stop \"{name}\": {e}. Try again in a moment.")),
        }
        true
    }

    /// Called once when platoon is leaving: give the stops still running a few seconds to finish, so their archive
    /// records are written, and then leave regardless. A stop that hasn't finished by then leaves its conversation
    /// unarchived (its process may still be shutting down): never archived while running.
    pub fn finish_stops(&mut self) {
        let deadline = Instant::now() + self.stop_times.quit;
        while !self.stopping.is_empty() {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.stops_rx.recv_timeout(left) {
                Ok(done) => {
                    self.stop_done(done);
                }
                Err(_) => break,
            }
        }
    }

    /// How many conversations are being stopped for the archive key right now.
    #[cfg(test)]
    pub fn stops_pending(&self) -> usize {
        self.stopping.len()
    }

    /// Take in finished stops until none is pending (or `max` has passed). True when none is left.
    #[cfg(test)]
    pub fn settle_stops_for_test(&mut self, max: Duration) -> bool {
        let deadline = Instant::now() + max;
        while !self.stopping.is_empty() && Instant::now() < deadline {
            self.poll_stops();
            thread::sleep(Duration::from_millis(5));
        }
        self.stopping.is_empty()
    }

    /// Archive `id` as the archive key does after any confirmation (for tests that go round the selection).
    #[cfg(test)]
    pub fn archive_for_test(&mut self, id: &str, name: &str) {
        self.archive_conversation(id, name);
    }

    #[cfg(test)]
    pub fn set_stop_times_for_test(&mut self, stop: Duration, quit: Duration) {
        self.stop_times = StopTimes { stop, quit };
    }

    /// The user opened a conversation. That counts as activity for auto-archive (opening one doesn't always
    /// write to it), and an archived one comes back to the Active tab.
    fn note_opened(&mut self, id: &str, archived: bool) {
        self.viewing = Some(id.to_string());
        self.boost(id);
        let now = usage::now();
        let saved = if archived { self.archive.restore(id, now) } else { self.archive.touch(id, now) };
        if archived {
            let _ = self.projects.session_restored(id);
        }
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
        self.launch_in(dir, args, None, false)
    }

    /// `launch`, for a session of `project`: claude gets `PLATOON_PROJECT` (so the notes hook knows), and with
    /// `place` (a new session) the process is remembered until its session id shows up and is put in the project.
    fn launch_in(&mut self, dir: String, args: &[&str], project: Option<&str>, place: bool) -> bool {
        let (rows, cols) = self.pty_size();
        let env: Vec<(&str, &str)> = project.map(|p| ("PLATOON_PROJECT", p)).into_iter().collect();
        let at_ms = usage::now().saturating_sub(2) * 1000;
        match host::launch(&self.program, &dir, args, &env, rows, cols) {
            Ok((info, child)) => {
                let pid = info.claude_pid;
                if let (Some(p), true) = (project, place) {
                    if let Err(e) = projects_mod::note_launch(&self.cache, pid, p, at_ms) {
                        self.notify(format!("Couldn't note the new session's project: {e}"));
                    }
                }
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
    pub fn switchboard_for_test(&self) -> Arc<ai::Switchboard> {
        self.switchboard.clone()
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

    #[cfg(test)]
    pub fn cache_for_test(&self) -> &Path {
        &self.cache
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
                KeyCode::Tab => self.step_settings_tab(1),
                KeyCode::BackTab => self.step_settings_tab(-1),
                KeyCode::Home | KeyCode::Char('g') => self.setting = 0,
                KeyCode::End | KeyCode::Char('G') => self.setting = self.items().len() - 1,
                KeyCode::Left | KeyCode::Char('h' | '-') => self.change_setting(self.setting, Step::Prev),
                KeyCode::Right | KeyCode::Char('l' | '+' | '=') => self.change_setting(self.setting, Step::Next),
                KeyCode::Enter | KeyCode::Char(' ') => self.change_setting(self.setting, Step::Cycle),
                // Restore platoon's default system prompt (only a changed one has one to go back to).
                KeyCode::Char('r') => {
                    if let Some(Kind::Instructions(id)) = self.items().get(self.setting).map(|i| i.kind) {
                        self.ask_reset_instructions(id);
                    }
                }
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
            Mode::Search => match key.code {
                KeyCode::Esc => self.act(Action::Cancel),
                KeyCode::Enter => self.act(Action::Confirm),
                KeyCode::Tab => self.step_search_kind(1),
                KeyCode::BackTab => self.step_search_kind(-1),
                KeyCode::Up => self.move_search(-1),
                KeyCode::Down => self.move_search(1),
                KeyCode::PageUp => self.move_search(-5),
                KeyCode::PageDown => self.move_search(5),
                KeyCode::Char('c') if ctrl => self.act(Action::Cancel),
                KeyCode::Char('w') if ctrl => self.ask_wider(),
                // While a description search is out what was typed is what was asked: esc calls it off first.
                _ if self.find_locked() => {}
                KeyCode::Char('u') if ctrl => {
                    self.search.text.clear();
                    self.search_text_changed();
                }
                KeyCode::Backspace => {
                    self.search.text.pop();
                    self.search_text_changed();
                }
                KeyCode::Char(c) if !ctrl && !alt => {
                    self.search.text.push(c);
                    self.search_text_changed();
                }
                _ => {}
            },
            Mode::Rename => match key.code {
                KeyCode::Esc => self.act(Action::Cancel),
                KeyCode::Enter => self.act(Action::Confirm),
                KeyCode::Char('c') if ctrl => self.act(Action::Cancel),
                KeyCode::Char('u') if ctrl => self.rename.input.clear(),
                KeyCode::Backspace => {
                    self.rename.input.pop();
                }
                KeyCode::Char(c) if !ctrl && !alt => self.type_name(&c.to_string()),
                _ => {}
            },
            Mode::List => self.list_key(key),
            Mode::Handoff => self.handoff_key(key),
            Mode::ProjectName => match key.code {
                KeyCode::Esc => self.act(Action::Cancel),
                KeyCode::Enter => self.act(Action::Confirm),
                KeyCode::Char('c') if ctrl => self.act(Action::Cancel),
                KeyCode::Char('u') if ctrl => self.name_box.input.clear(),
                KeyCode::Backspace => {
                    self.name_box.input.pop();
                }
                KeyCode::Char(c) if !ctrl && !alt => self.type_project_name(&c.to_string()),
                _ => {}
            },
            Mode::TicketPick => self.ticket_pick_key(key),
            Mode::SquadPick => self.squad_pick_key(key),
            Mode::SquadChoice => self.squad_choice_key(key),
            Mode::SquadText => self.squad_text_key(key),
            Mode::Assign => match key.code {
                KeyCode::Esc => self.act(Action::Cancel),
                KeyCode::Enter => self.act(Action::Confirm),
                KeyCode::Char('c') if ctrl => self.act(Action::Cancel),
                KeyCode::Up => self.move_assign(-1),
                KeyCode::Down => self.move_assign(1),
                _ => {}
            },
            Mode::EditNotes => self.editor_key(key),
            Mode::PickEditor => match key.code {
                KeyCode::Esc => self.act(Action::Cancel),
                KeyCode::Enter => self.act(Action::Confirm),
                KeyCode::Char('c') if ctrl => self.act(Action::Cancel),
                KeyCode::Up => self.move_editor_pick(-1),
                KeyCode::Down => self.move_editor_pick(1),
                KeyCode::Char('g') | KeyCode::Home => self.epick.selected = 0,
                KeyCode::Char('G') | KeyCode::End => self.epick.selected = self.epick.options.len().saturating_sub(1),
                _ => {}
            },
            Mode::Changes => match key.code {
                KeyCode::Esc | KeyCode::Char('q' | 'c') => self.act(Action::Cancel),
                KeyCode::Down => self.scroll_changes(1),
                KeyCode::Up => self.scroll_changes(-1),
                KeyCode::PageDown => self.scroll_changes(10),
                KeyCode::PageUp => self.scroll_changes(-10),
                KeyCode::Home | KeyCode::Char('g') => self.changes.top.set(0),
                _ => {}
            },
            Mode::Look => match key.code {
                KeyCode::Esc => self.act(Action::Cancel),
                KeyCode::Enter => self.act(Action::Confirm),
                KeyCode::Char('c') if ctrl => self.act(Action::Cancel),
                KeyCode::Left | KeyCode::Up => self.step_slot(-1),
                KeyCode::Right | KeyCode::Down | KeyCode::Tab => self.step_slot(1),
                KeyCode::Backspace => {
                    self.look.badge.pop();
                }
                KeyCode::Char(c) if !ctrl && !alt => self.type_badge(&c.to_string()),
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
            Mode::Search if self.find_locked() => {}
            Mode::Search => {
                self.search.text.push_str(&text.replace(['\n', '\r'], ""));
                self.search_text_changed();
            }
            Mode::Rename => self.type_name(&text.replace(['\n', '\r'], "")),
            Mode::ProjectName => self.type_project_name(&text.replace(['\n', '\r'], "")),
            Mode::Look => self.type_badge(&text.replace(['\n', '\r'], "")),
            Mode::EditNotes => self.editor_paste(text),
            Mode::Handoff => self.handoff_paste(text),
            Mode::SquadText => self.squad_type(text),
            _ => {}
        }
        self.update_view();
    }

    /// Add typed or pasted text to the name being written, up to the longest a name may be.
    fn type_name(&mut self, text: &str) {
        let room = names::NAME_MAX.saturating_sub(self.rename.input.chars().count());
        self.rename.input.extend(text.chars().take(room));
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
                    Mode::Attached(_) if self.sq.split.is_some() && split_areas(self.size.0, self.size.1).wide && split_areas(self.size.0, self.size.1).ticket.contains((m.column, m.row).into()) => {
                        self.wheel_ticket(up);
                    }
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
                    Mode::List => self.wheel_list(up),
                    Mode::Assign => self.move_assign(if up { -1 } else { 1 }),
                    Mode::TicketPick => self.move_ticket_pick(if up { -1 } else { 1 }),
                    Mode::SquadPick => self.move_squad_pick(if up { -1 } else { 1 }),
                    Mode::SquadChoice => self.move_squad_choice(if up { -1 } else { 1 }),
                    Mode::SquadText => {}
                    Mode::PickEditor => self.move_editor_pick(if up { -1 } else { 1 }),
                    Mode::Changes => self.scroll_changes(if up { -3 } else { 3 }),
                    Mode::EditNotes => {}
                    Mode::Handoff => self.handoff_wheel(up),
                    Mode::ProjectName | Mode::Look => {}
                    Mode::Search => self.move_search(if up { -1 } else { 1 }),
                    Mode::Rename => {}
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
                    Some(Hit::Tab(i)) if self.projects_list_showing() => self.switch_ptab(i),
                    Some(Hit::Tab(i)) => self.switch_tab(i),
                    Some(Hit::Top(i)) => self.switch_top(i),
                    Some(Hit::ProjectRow(i)) if i == self.pselected => self.act(Action::Jump),
                    Some(Hit::ProjectRow(i)) => self.pselected = i,
                    Some(Hit::AssignRow(i)) if i == self.assign.selected => self.act(Action::Confirm),
                    Some(Hit::AssignRow(i)) => self.assign.selected = i,
                    Some(Hit::EditorRow(i)) if i == self.epick.selected => self.act(Action::Confirm),
                    Some(Hit::EditorRow(i)) => self.epick.selected = i,
                    Some(Hit::Slot(i)) => self.look.slot = i,
                    Some(Hit::Pane(p)) => self.focus_pane(p),
                    Some(Hit::TicketRow(i)) => self.click_ticket_row(i),
                    Some(Hit::TicketPickRow(i)) if i == self.tk.pick.selected => self.act(Action::Confirm),
                    Some(Hit::TicketPickRow(i)) => self.tk.pick.selected = i,
                    Some(Hit::SquadRow(i)) if i == self.sq.pick_selected => self.act(Action::Confirm),
                    Some(Hit::SquadRow(i)) => self.sq.pick_selected = i,
                    Some(Hit::SquadEntry(i)) => self.click_squad(i),
                    Some(Hit::SquadView(agents)) => self.show_squad_view(if agents { SqView::Agents } else { SqView::Squads }),
                    Some(Hit::SquadDetail(i)) => self.click_squad_row(i),
                    Some(Hit::SquadChoiceRow(i)) if i == self.sqt.choice.selected => self.act(Action::Confirm),
                    Some(Hit::SquadChoiceRow(i)) => self.sqt.choice.selected = i,
                    Some(Hit::SearchKind(i)) => self.set_search_kind(i),
                    // A row that is no longer listed (the box was emptied since the last frame) is not there to click.
                    Some(Hit::SearchRow(_)) if self.search_rows_hidden() => {}
                    // As in the list: clicking the highlighted result again takes it.
                    Some(Hit::SearchRow(i)) if i == self.search_selected() => self.act(Action::Confirm),
                    Some(Hit::SearchRow(i)) => self.select_search_row(i),
                    Some(Hit::Setting(i)) => self.setting = i,
                    Some(Hit::SettingsTab(i)) => self.switch_settings_tab(i),
                    Some(Hit::SettingStep(i, step)) => {
                        self.setting = i;
                        self.change_setting(i, step);
                    }
                    // Clicking the selected row again acts like Enter.
                    Some(Hit::Row(i)) if i == self.selected && (self.open_project.is_none() || self.pane == Pane::Sessions) => self.act(Action::Jump),
                    Some(Hit::Row(i)) => {
                        self.selected = i;
                        self.focus_pane(Pane::Sessions);
                    }
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
