use crate::agents::claude::{self, Reader};
use crate::agents::{Agent, Catalog, CodexAt, Hidden, Live, Providers, Start, codex};
use crate::ai;
use crate::archive::{self, Archive};
use crate::client::{Client, key_bytes};
use crate::clipboard::Clipboard;
use crate::dirs;
use crate::editors::Editor;
use crate::guard;
use crate::host::{self, HostInfo};
use crate::index::{Entry, Indexer};
use crate::keys::KeysStore;
use crate::ledger::Ledger;
use crate::model::{NameSource, Proc, Session, Status};
use crate::names::{self, Names};
use crate::projects::{self as projects_mod, Projects};
use crate::prompts::{self, PromptIndex};
use crate::select::{Click, Drag, Past, Point, Selection, Unit, click_count, extract};
use crate::settings::{EditorFor, Item, Kind, ModelFor, Settings, SettingsStore, Step, TABS as SETTINGS_TABS};
use crate::setup;
use crate::summaries::Summaries;
use crate::theme::Theme;
use crate::usage::{self, Usage};
use crate::usertheme::Registry;
use crate::versions;
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::{
    cell::{Cell, RefCell},
    cmp::Reverse,
    collections::{HashMap, HashSet},
    io,
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{Receiver, Sender, channel},
    },
    thread,
    time::{Duration, Instant},
};

mod board_glue;
mod editor_glue;
mod handoff_glue;
mod idle_glue;
mod merged_glue;
mod newsession_glue;
mod notes_glue;
mod programs_glue;
mod projects_glue;
mod smart_glue;
mod spent_glue;
mod squads_glue;
mod squadtab_glue;
mod taskdraft_glue;
mod tickets_glue;
pub(crate) mod worktree_glue;
pub use board_glue::{BRow, BoardState, ListItem};
#[cfg(test)]
pub use editor_glue::{EdTarget, NO_EDITOR};
pub use handoff_glue::{HandoffBox, HandoffState};
pub use newsession_glue::AgentPick;
pub use notes_glue::{Changes, EditorPick, FileEdit, FileOf, NotesEdit, NotesWindow};
pub use programs_glue::ProgramState;
pub use projects_glue::{
    AssignOpt, AssignPick, LookEdit, NameBox, NamePurpose, PStat, PTABS, Pane, TOP_SESSIONS, TOP_SQUADS, TOP_TASKS, TOPS,
};
pub use smart_glue::{Engine, FIND_KIND, FindData, FindState};
pub use squads_glue::SquadState;
pub use squadtab_glue::{SquadsTab, origin_words};
pub use tickets_glue::TicketsState;

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
    Tab { label: "Active", empty: "No sessions yet. Press n to start one.", shows: |s| !s.archived },
    Tab { label: "Archived", empty: "Nothing archived. Press a on a session to archive it.", shows: |s| s.archived },
];

/// Round badge caps are Powerline/Nerd Font glyphs. Terminals that bring their own symbol fallback (Ghostty, Kitty,
/// WezTerm) draw them with any font; elsewhere they can show as empty boxes, so those get square badges.
/// `PLATOON_BADGES=rounded` or `square` overrides the guess.
fn detect_rounded() -> bool {
    detect_rounded_with(&|k| std::env::var(k).ok())
}

/// A screenful of each list, as last drawn (see `App::pages`). Before the first frame, a guess.
pub struct Pages {
    /// The Sessions list, or the Sessions pane of a project's screen (only one of them shows at a time).
    pub sessions: Cell<usize>,
    pub projects: Cell<usize>,
    pub tickets: Cell<usize>,
    /// The settings' entries.
    pub settings: Cell<usize>,
}

impl Default for Pages {
    fn default() -> Self {
        let c = || Cell::new(10);
        Pages { sessions: c(), projects: c(), tickets: c(), settings: c() }
    }
}

/// How far PageUp/PageDown move in a list that showed `rows` rows: a screenful, less one row kept in view.
pub fn page_of(rows: &Cell<usize>) -> i32 {
    (rows.get().saturating_sub(1)).max(1) as i32
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
    /// `v`: open the code editor on what is selected (or pick a folder).
    Editor,
    /// `?`: every key of the screen showing, in a popup (`help_open`).
    Help,
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
    /// A row of the Tasks tab.
    TicketRow(usize),
    /// A task in the popup of tasks drafted from a conversation.
    DraftTask(usize),
    /// An entry of the list of squads (`s` on a task).
    SquadRow(usize),
    /// A row of the Squads tab.
    SquadsTabRow(usize),
    /// A setting of the open squad on the Squads tab (`squads::Setting::ALL`).
    SquadSetting(usize),
    /// An entry of the project picker (`m`).
    AssignRow(usize),
    /// An entry of the project picker of a new task (`n` on the Tasks tab).
    ProjectPickRow(usize),
    /// An entry of the editor list.
    EditorRow(usize),
    /// A row of `v`'s list of editors.
    FileEditorRow(usize),
    /// An agent of a new session's question (`n` with Claude Code and Codex both installed).
    AgentRow(usize),
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
    /// An agent that keeps one window per conversation itself (Codex) has it open in a window platoon didn't start: it
    /// is never opened a second time, nor ended. The user is told where it is.
    Elsewhere,
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
    /// `c` on a task: cancel it? `pid`: its session's `claude`, when that is running (yes stops that session too).
    /// `uncommitted`: its worktree has changes not yet committed, so platoon will keep it.
    CancelTicket { number: u32, title: String, pid: Option<u32>, uncommitted: bool },
    /// `v` on a task whose session is still working in its files: open the editor anyway?
    EditorWhileWorking { number: u32, title: String },
    /// `d` on a squad of the user's: delete it? `replaces`: it replaces a built-in, which comes back.
    DeleteSquad { id: String, name: String, replaces: bool },
}

/// The smallest terminal platoon draws its screens in. Below it, `ui::draw` says so in one centered message instead of
/// squeezing the screens into fragments, and no key acts but quit (`App::too_small`).
pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 14;

/// Which screen is showing, as far as notices go (`App::screen_id`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenId {
    mode: std::mem::Discriminant<Mode>,
    attached: Option<u32>,
    top: usize,
    project: Option<String>,
    ticket: Option<u32>,
    squad_open: bool,
    help: bool,
}

/// A terminal of `w` by `h` is too small for platoon's screens.
pub fn too_small(w: u16, h: u16) -> bool {
    w < MIN_WIDTH || h < MIN_HEIGHT
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
    /// The list of editors `v` opens a ticket's or a session's files with (`ed.fpick`), when "Always ask" is on.
    PickFileEditor,
    /// The before/after view of the last change to the notes (`changes`).
    Changes,
    /// The hand-off brief, waiting for Claude or waiting to be edited and accepted (`handoff`).
    Handoff,
    /// The list of squads a task starts with (`s`, `sq`).
    SquadPick,
    /// Which project a new task goes in (`bd.pick`).
    TaskProject,
    /// The tasks drafted from a conversation, before any is made (`td.draft`).
    TaskDraft,
    /// Which agent a new session starts in, Claude Code or Codex (`agent_pick`).
    PickAgent,
}

pub struct Picker {
    pub input: String,
    all: Vec<String>,
    /// Absolute paths, best match first.
    pub results: Vec<String>,
    pub selected: usize,
    /// The project a new session started from here goes into.
    pub project: Option<String>,
    /// Picking the folder of this task (of the store the keys act on), before it starts.
    pub task_folder: Option<u32>,
    /// Picking a folder to open the code editor in (`v` with nothing selected).
    pub editor: bool,
    /// Picking the folder of a new task in no project, with this title (it is written once the folder is picked).
    pub new_task: Option<String>,
}

impl Picker {
    fn update(&mut self, home: &str) {
        let q = self.input.trim();
        let mut res: Vec<String> = Vec::new();
        if let Some(p) = dirs::expand(q, home)
            && Path::new(&p).is_dir()
        {
            res.push(p);
        }
        if q.is_empty() {
            res.extend(self.all.iter().take(200).cloned());
        } else {
            let m = SkimMatcherV2::default();
            let mut scored: Vec<(i64, &String)> =
                self.all.iter().filter_map(|p| m.fuzzy_match(&dirs::tilde(p, home), q).map(|s| (s, p))).collect();
            scored.sort_by_key(|s| std::cmp::Reverse(s.0));
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
        explain: "Finds sessions by the words you typed in them, archived ones too.",
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
    /// Folders added beside `dir`, as on the row.
    pub extra_dirs: Vec<String>,
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

/// A version notice (`versions.rs`) waiting to show, or showing.
struct News {
    text: String,
    /// What to remember once it has shown: the versions it announced.
    state: versions::State,
    /// It has been on screen, and its versions are remembered as announced.
    shown: bool,
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
    /// Claude Code's plan usage, from its status line.
    pub usage: Usage,
    /// Codex's plan windows still running, as its saved sessions last reported them: None when Codex isn't installed,
    /// empty while no session has said.
    pub codex_limits: Option<Vec<crate::usage::Limit>>,
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
    /// What happened, when, for how long it shows, and the screen it belongs to (`settle_notice`).
    pub notice: Option<(String, Instant, Duration, Option<ScreenId>)>,
    /// A newer Claude Code or platoon (`versions.rs`): shown above the footer of the top-level lists until a key or a click.
    news: Option<News>,
    /// The version checks started with the app, until they answer.
    news_rx: Option<Receiver<Option<versions::Outcome>>>,
    pub mouse: (u16, u16),
    /// Text being selected with the mouse in the attached view: highlighted as it grows, copied when the button
    /// is released. Cleared by anything that could make it stale (a key, a paste, a resize, the wheel, leaving).
    pub selection: Option<Selection>,
    /// The left button is held and the selection follows the mouse (and the view scrolls past an edge).
    drag: Option<Drag>,
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
    /// The task store the keys act on, and a task's own screen.
    pub tk: TicketsState,
    /// The Tasks tab.
    pub bd: BoardState,
    /// Starting tasks: the list of squads, the worktrees being made, and which sessions are tasks'.
    pub sq: SquadState,
    /// The Squads tab.
    pub sqt: SquadsTab,
    /// What tasks' sessions have spent, read off the UI thread.
    pub sw: spent_glue::WorkState,
    /// Which Done tasks' branches are not merged yet (`app/merged_glue.rs`).
    merged: merged_glue::MergeState,
    /// Sessions whose host an older platoon started: the rules applied from outside, and why each is kept
    /// (`app/idle_glue.rs`).
    idle: idle_glue::IdleState,
    /// Tasks' worktrees after the work: removing them, and bringing them back to continue a session.
    pub wt: worktree_glue::WtState,
    /// Tasks drafted from a conversation (`t`): the call out, the draft in its popup.
    pub td: taskdraft_glue::DraftState,
    /// `n`'s question of the agent a new session starts in, while it is open.
    agent_pick: Option<AgentPick>,
    /// The code editor (`v`): the questions before it opens, the request the event loop takes.
    pub ed: editor_glue::EdState,
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
    /// The keys the user can change (`<config dir>/keys.json`): back and quit.
    pub keys: KeysStore,
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
    /// The help popup (`?`) is open over the screen showing: every key of that screen. It opens only on platoon's own
    /// screens (`Mode::List`), which stay as they are under it.
    pub help_open: bool,
    /// The first line of the help popup in view, when it is longer than the screen (kept in range by the drawing).
    pub help_top: Cell<usize>,
    pub hits: RefCell<Vec<(Rect, Hit)>>,
    /// How many rows each list had room for when it was last drawn: what PageUp and PageDown move it by.
    pub pages: Pages,
    size: (u16, u16),
    reader: Reader,
    /// Codex's conversations open right now (its writer locks), when Codex's folder is read (None in a unit test that
    /// gave it none).
    codex: Option<codex::Reader>,
    indexer: Indexer,
    /// What the user typed in every conversation, for the full-text search; the smart names and summaries are made from it too.
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
    /// The cancel flags of the smart features' calls in flight (Claude's or Codex's): a switch turned off in settings cancels them.
    switchboard: Arc<ai::Switchboard>,
    /// Tokens the token features have used today on each AI program installed (on Claude Code alone when there is
    /// none), as the ledger says (shown in the settings).
    pub tokens_today: Vec<(Agent, u64)>,
    /// The run of failures whose "paused" notice was already shown, so it appears once.
    ai_notice_seen: Option<String>,
    /// The conversation opened in the attached view: never auto-archived, even before its process registers.
    viewing: Option<String>,
    /// When the attached view last connected again by itself (`keep_attached`), since it was opened.
    reconnected: Option<Instant>,
    /// When auto-archive last ran (None: not yet).
    last_auto_archive: Option<Instant>,
    /// A settings change is waiting to take effect in auto-archive, which is on hold until then.
    apply_after: Option<Instant>,
    /// The program that runs a conversation of Claude Code's (`Agent::program`, read once).
    program: String,
    /// The program that runs a conversation of Codex's, read once too.
    codex_program: String,
    /// Claude Code's 1M window is switched off (`claude::large_window_off`, read once): its rows are measured against 200k.
    claude_large_off: bool,
    /// The AI programs this computer has (read once), whatever the settings say: what the settings screen's Programs
    /// show, and where a running task of a program switched off is still found.
    installed: Providers,
    /// The AI programs platoon uses: `installed` less the ones switched off in the settings (`sync_providers`). What
    /// decides the rows listed, the models offered and what a call runs on.
    providers: Providers,
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
    /// A unit test's sessions are never started: what `launch_in` would have started is noted here instead (the
    /// agent, the folder, the arguments and the environment it adds).
    #[cfg(test)]
    pub launched: Vec<Launched>,
}

/// A new Codex session goes through three stages, and has one row in each: before Codex holds a conversation (its own
/// update and folder-trust questions), Codex holding one it hasn't saved (no rollout until the first message), and the
/// saved conversation. `live` (with platoon's hosts' Codex holders already told by the host's child, `App::live_now`)
/// gets, for each new Codex session of `hosts` that holds nothing yet, a stand-in whose id is `model::unsaved_id`, its
/// folder the host's and its start `started(host)`; and a conversation a host's child holds and hasn't saved takes the
/// host's folder (there is no rollout to say it). Both are `unsaved`. A stand-in goes once its process holds a
/// conversation, so no session is ever two rows.
pub(crate) fn new_codex_sessions(mut live: Vec<Live>, hosts: &[HostInfo], started: impl Fn(&HostInfo) -> u64) -> Vec<Live> {
    for l in live.iter_mut().filter(|l| l.agent == Agent::Codex && l.unsaved) {
        if let Some(h) = hosts.iter().find(|h| h.claude_pid == l.pid) {
            l.cwd.clone_from(&h.dir);
        }
    }
    let new: Vec<Live> = hosts
        .iter()
        .filter(|h| h.agent == Agent::Codex && h.session().is_none() && !live.iter().any(|l| l.pid == h.claude_pid))
        .map(|h| Live {
            agent: Agent::Codex,
            pid: h.claude_pid,
            id: crate::model::unsaved_id(h.claude_pid),
            cwd: h.dir.clone(),
            name: None,
            started_at: started(h),
            busy: false,
            background: None,
            parked: None,
            unsaved: true,
        })
        .collect();
    live.extend(new);
    live
}

/// One session a unit test's app would have started (see `App::launched`).
#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launched {
    pub agent: Agent,
    pub dir: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// The option `step` leads to from `id` among `options` (id, name): back and on stop at the ends, `Cycle` goes round;
/// an id that isn't one of them goes to the first. None when that is where it is already, or there are none.
fn step_among(options: &[(String, String)], id: &str, step: Step) -> Option<String> {
    let last = options.len().checked_sub(1)?;
    let next = match (options.iter().position(|(o, _)| o == id), step) {
        (None, _) => 0,
        (Some(i), Step::Prev) => i.saturating_sub(1),
        (Some(i), Step::Next) => (i + 1).min(last),
        (Some(i), Step::Cycle) => {
            if i >= last {
                0
            } else {
                i + 1
            }
        }
    };
    Some(options[next].0.clone()).filter(|chosen| chosen != id)
}

impl App {
    /// The real thing: rows appear as the background index reads them.
    pub fn new() -> Self {
        let reader = Reader::new();
        let codex = codex::Reader::new();
        let cache = usage::cache_dir();
        let indexer = Indexer::spawn(reader.root(), Some(codex.root()), &cache);
        let large_off = std::env::var("CLAUDE_CODE_DISABLE_1M_CONTEXT").ok();
        let mut app = Self::build(reader, Some(codex), cache, archive::config_dir(), indexer, true, large_off.as_deref());
        app.installed = Providers::installed_here();
        app.sync_providers();
        app.rounded = detect_rounded();
        app.plain_marker = detect_plain_marker_with(&|k| std::env::var(k).ok());
        app.start_version_checks();
        // Know the running hosts before the first frame: until then every session platoon hosts looks like a copy
        // running in another terminal, and Enter on it would offer to take it over instead of opening it.
        app.refresh();
        app
    }

    /// `cache` and `config` are where platoon keeps its own state; tests pass temp dirs so they never touch
    /// the real ones. The index is read on the calling thread, so tests see rows straight away.
    #[cfg(test)]
    pub fn with_dirs(reader: Reader, cache: PathBuf, config: PathBuf) -> Self {
        let indexer = Indexer::inline(reader.root(), None, &cache);
        Self::build(reader, None, cache, config, indexer, false, None)
    }

    /// `with_dirs` as the app is right after it starts: the rows from the saved index, the first scan of the disk not
    /// finished until `release_index_for_test`.
    #[cfg(test)]
    pub fn with_dirs_index_held(reader: Reader, cache: PathBuf, config: PathBuf) -> Self {
        let indexer = Indexer::held(reader.root(), None, &cache);
        Self::build(reader, None, cache, config, indexer, false, None)
    }

    /// `with_dirs`, reading Codex's conversations too, from its folder `codex`.
    #[cfg(test)]
    pub fn with_codex_dirs(reader: Reader, codex: codex::Reader, cache: PathBuf, config: PathBuf) -> Self {
        let indexer = Indexer::inline(reader.root(), Some(codex.root()), &cache);
        Self::build(reader, Some(codex), cache, config, indexer, false, None)
    }

    /// `threaded`: the prompt index reads in the background (the real thing) rather than on the calling thread (tests).
    /// `large_off`: `CLAUDE_CODE_DISABLE_1M_CONTEXT` in platoon's environment (`claude::large_window_off`), known before
    /// the first rows are made.
    fn build(
        mut reader: Reader,
        codex: Option<codex::Reader>,
        cache: PathBuf,
        config: PathBuf,
        mut indexer: Indexer,
        threaded: bool,
        large_off: Option<&str>,
    ) -> Self {
        // platoon's own `claude -p` helper runs in one folder of ours. It registers itself like any session, but it is
        // not a conversation: dropped here, at the two doors (running processes and saved transcripts), so nothing
        // downstream (rows, counts, search, naming, archive) ever sees it.
        let helper = Hidden::new(&ai::work_dir(&cache));
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
        let claude_large_off = claude::large_window_off(reader.root(), large_off);
        let mut app = Self {
            sessions: Vec::new(),
            view: Vec::new(),
            tab_view: Vec::new(),
            counts: vec![0; TABS.len()],
            tab: 0,
            usage: Usage::NotSetUp,
            codex_limits: None,
            selected: 0,
            filter: String::new(),
            mode: Mode::List,
            search: Search {
                kind: 0,
                text: String::new(),
                results: Vec::new(),
                total: 0,
                selected: 0,
                top: Cell::new(0),
                computed_for: None,
                find: FindData::new(),
            },
            rename: Rename { id: String::new(), input: String::new(), original: String::new() },
            picker: Picker {
                input: String::new(),
                all: Vec::new(),
                results: Vec::new(),
                selected: 0,
                project: None,
                task_folder: None,
                editor: false,
                new_task: None,
            },
            hosts: Vec::new(),
            client: None,
            spawned: Vec::new(),
            notice: None,
            news: None,
            news_rx: None,
            mouse: (0, 0),
            selection: None,
            drag: None,
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
            tk: TicketsState::new(),
            bd: BoardState::new(),
            sq: SquadState::new(),
            sqt: SquadsTab::default(),
            sw: spent_glue::WorkState::default(),
            merged: merged_glue::MergeState::default(),
            idle: idle_glue::IdleState::default(),
            wt: worktree_glue::WtState::default(),
            td: taskdraft_glue::DraftState::default(),
            ed: editor_glue::EdState::new(),
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
            agent_pick: None,
            epick: EditorPick {
                project: String::new(),
                instructions: None,
                ticket: None,
                file: None,
                options: Vec::new(),
                selected: 0,
                starred: String::new(),
            },
            changes: Changes { lines: Vec::new(), top: Cell::new(0) },
            handoff: HandoffBox::empty(),
            handoff_flight: None,
            git_program: "git".into(),
            home: std::env::var("HOME").unwrap_or_default(),
            cache,
            config: config.clone(),
            keys: KeysStore::open(&config),
            tick: 0,
            theme: Theme::default(),
            preview: None,
            tpick: ThemePick { list: Vec::new(), selected: 0, top: Cell::new(0) },
            themes: Registry::open(&config),
            quit: false,
            rounded: false,
            plain_marker: false,
            hits: RefCell::new(Vec::new()),
            help_open: false,
            help_top: Cell::new(0),
            pages: Pages::default(),
            size: (80, 24),
            reader,
            codex,
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
            tokens_today: Vec::new(),
            ai_notice_seen: None,
            viewing: None,
            reconnected: None,
            last_auto_archive: None,
            apply_after: None,
            program: Agent::Claude.program(),
            codex_program: Agent::Codex.program(),
            claude_large_off,
            installed: Providers::default(),
            providers: Providers::default(),
            guard_on: false,
            view_for: (String::new(), usize::MAX, 0, 0, 0, None),
            rev: 0,
            built: (0, Vec::new()),
            stopping: HashMap::new(),
            stops_tx,
            stops_rx,
            stop_times: StopTimes { stop: host::STOP_WAIT, quit: QUIT_WAITS_FOR_STOPS },
            #[cfg(test)]
            launched: Vec::new(),
        };
        app.theme = app.settings.get().theme(&app.themes);
        // A unit test's app has Claude Code, and Codex when it was given a Codex folder that is there; never the real
        // `codex` (`set_providers_for_test` gives it a stand-in).
        app.installed = Providers {
            claude: Some(app.program.clone()),
            codex: app.codex.as_ref().filter(|c| c.root().is_dir()).map(|c| CodexAt {
                program: if cfg!(test) { "/nonexistent/platoon-unit-test/codex".into() } else { app.codex_program.clone() },
                root: c.root().to_path_buf(),
            }),
            off: Vec::new(),
        };
        app.sync_providers();
        // Real inputs (running hosts, the guard setting) come with `refresh` (`new` calls it at once); until then assume none.
        app.refresh_with(Vec::new(), false);
        app.say_keys_problems();
        app
    }

    pub fn settings(&self) -> &SettingsStore {
        &self.settings
    }

    /// The AI programs platoon uses: the ones this computer has, less the ones switched off in the settings.
    pub fn providers(&self) -> &Providers {
        &self.providers
    }

    /// The models of the AI programs this computer has.
    pub fn catalog(&self) -> Catalog {
        self.providers.catalog()
    }

    /// This window's own folders, which the hosts it looks for and starts use too (a unit test's are temporary).
    pub fn host_dirs(&self) -> host::Dirs {
        host::Dirs { cache: self.cache.clone(), config: self.config.clone() }
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
        let catalog = self.catalog();
        let mut agents: Vec<Agent> = Agent::ALL.into_iter().filter(|a| catalog.installed(*a)).collect();
        if agents.is_empty() {
            agents.push(Agent::Claude);
        }
        let data = Ledger::open(&self.cache).load();
        self.tokens_today = agents.into_iter().map(|a| (a, data.of(a).day(now).tokens())).collect();
        if let Some((run, text)) = ai::backoff_notice(&settings, &self.cache, now)
            && self.ai_notice_seen.as_deref() != Some(run.as_str())
        {
            self.ai_notice_seen = Some(run);
            self.notify(text);
        }
    }

    /// Pick up an edit of settings.json and of the theme folder, and with them a changed theme. True when the
    /// settings file was re-read.
    fn refresh_settings(&mut self) -> bool {
        if self.keys.refresh() {
            self.say_keys_problems();
        }
        let changed = self.settings.refresh();
        // The theme list keeps the entries it opened with, so the folder is looked at again once it has closed.
        let themes_changed = !matches!(self.mode, Mode::Themes) && self.themes.refresh();
        if changed || themes_changed {
            self.sync_theme();
        }
        if changed {
            self.sync_providers();
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

    /// An entry of keys.json that was ignored (or needs saying) is said once, in the footer.
    fn say_keys_problems(&mut self) {
        if let Some(n) = self.keys.take_notice() {
            self.notify(n);
        }
    }

    /// The back key as the hints show it (`ctrl+space` unless keys.json says otherwise). Only where Esc belongs to
    /// someone else: a session (the attached view) and the notes editor. On platoon's own screens, where the back key
    /// is a second Esc, the hints say `esc back`.
    pub fn back_label(&self) -> &str {
        self.keys.get().back_label()
    }

    /// The quit key as the hints show it (`q` unless keys.json says otherwise).
    pub fn quit_label(&self) -> &str {
        self.keys.get().quit_label()
    }

    /// Is something running this session now: a Claude Code process seen for it (`Session::running`), or a platoon
    /// host started on it (`--resume`/`--session-id`) that is still alive, whether or not its claude has said so yet.
    pub fn session_running(&self, s: &Session) -> bool {
        s.running() || self.hosts.iter().any(|h| h.session().as_deref() == Some(s.id.as_str()))
    }

    pub fn notify(&mut self, msg: impl Into<String>) {
        self.notice = Some((msg.into(), Instant::now(), NOTICE_FOR, None));
    }

    /// The screen showing, as far as notices go: the mode, the session attached, the tab, the project and task open,
    /// and the help.
    pub fn screen_id(&self) -> ScreenId {
        ScreenId {
            mode: std::mem::discriminant(&self.mode),
            attached: if let Mode::Attached(pid) = self.mode { Some(pid) } else { None },
            top: self.top,
            project: self.open_project.clone(),
            ticket: self.tk.open,
            squad_open: self.squads_showing() && self.sqt.open,
            help: self.help_showing(),
        }
    }

    /// A notice belongs to the screen it was given on: the one showing at the end of the key, click or refresh that gave
    /// it (so a notice about a change of screen, given with it, stays with the new screen). Once the screen changes
    /// again it goes, rather than follow the user onto another screen or into a session. Called after every event and
    /// refresh.
    pub fn settle_notice(&mut self) {
        let now = self.screen_id();
        if let Some((.., on)) = self.notice.as_mut() {
            match on {
                None => *on = Some(now),
                Some(s) if *s != now => self.notice = None,
                Some(_) => {}
            }
        }
    }

    /// The auto-archive notice stays until the user does something (it covers the key hints while it shows).
    fn dismiss_long_notice(&mut self) {
        if self.notice.as_ref().is_some_and(|(_, _, ttl, _)| *ttl == AUTO_ARCHIVE_NOTICE_FOR) {
            self.notice = None;
        }
    }

    /// Start the version checks off the UI thread (`versions.rs`). Nothing at all with `PLATOON_NO_UPDATE_CHECK` set.
    fn start_version_checks(&mut self) {
        // Claude Code's and Codex's versions only where each is installed.
        let checks = versions::checks_for(
            &self.providers,
            versions::claude_program(),
            self.settings.get().update_check,
            versions::off_by_env(),
            &self.cache,
            versions::install_here,
        );
        self.news_rx = checks.map(versions::spawn);
    }

    /// How a model setting reads on the settings screen: the model in effect (the one picked, else this computer's
    /// default) by its name, said plainly when nothing here runs it; the widest value it can take; and whether it can
    /// go no further back or on.
    pub fn model_setting(&self, which: ModelFor) -> (String, usize, bool, bool) {
        let catalog = self.catalog();
        let current = which.in_effect(&self.settings.get(), &catalog);
        let options = catalog.models();
        let at = current.as_deref().and_then(|m| options.iter().position(|o| o.id == m));
        let label = match &current {
            Some(m) => catalog.label(m),
            None => "None installed".to_string(),
        };
        let widest = options.iter().map(|m| m.name.chars().count()).chain([label.chars().count()]).max().unwrap_or(0);
        let n = options.len();
        (label, widest, at == Some(0) || n == 0, at.is_some_and(|i| i + 1 >= n) || n == 0)
    }

    /// Step a model setting through the models this computer has, and save it at once. A model nothing here runs
    /// goes to the first one.
    fn step_model_setting(&mut self, which: ModelFor, step: Step) {
        let catalog = self.catalog();
        let options: Vec<(String, String)> = catalog.models().iter().map(|m| (m.id.clone(), m.name.clone())).collect();
        let current = which.in_effect(&self.settings.get(), &catalog).unwrap_or_default();
        let Some(chosen) = step_among(&options, &current, step) else { return };
        let mut s = self.settings.get();
        which.set(&mut s, chosen);
        let saved = self.settings.update(&s);
        self.sync_ai();
        self.report_save(saved);
    }

    /// Pick up the checks' answer, and put a notice on screen once one of the top-level lists shows: only then are its
    /// versions remembered as announced (so one that never showed is said next time).
    fn poll_news(&mut self) {
        if let Some(rx) = &self.news_rx {
            match rx.try_recv() {
                Ok(found) => {
                    self.news_rx = None;
                    if let Some(o) = found {
                        self.news = Some(News { text: o.text, state: o.state, shown: false });
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.news_rx = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.news_place()
            && let Some(n) = self.news.as_mut().filter(|n| !n.shown)
        {
            n.shown = true;
            let _ = n.state.save(&self.cache);
        }
    }

    /// Where a version notice shows: the Sessions and Projects lists, with nothing over them.
    fn news_place(&self) -> bool {
        matches!(self.mode, Mode::List) && !self.help_open && self.open_project.is_none() && !self.board_showing()
    }

    /// The version notice on screen now, if any.
    pub fn news_showing(&self) -> Option<&str> {
        self.news.as_ref().filter(|n| n.shown && self.news_place()).map(|n| n.text.as_str())
    }

    /// A key or a click while the version notice shows puts it away (the key still does what it does).
    fn dismiss_news(&mut self) {
        if self.news_showing().is_some() {
            self.news = None;
        }
    }

    /// A version notice as the checks would give it, on screen at the next refresh.
    #[cfg(test)]
    pub fn set_news_for_test(&mut self, text: &str) {
        self.news = Some(News { text: text.into(), state: versions::State::default(), shown: false });
    }

    /// The notice to show now: given lately, and on this screen (or given since the last event, not yet placed).
    pub fn active_notice(&self) -> Option<&str> {
        self.notice
            .as_ref()
            .filter(|(_, at, ttl, on)| at.elapsed() < *ttl && on.as_ref().is_none_or(|s| *s == self.screen_id()))
            .map(|(m, ..)| m.as_str())
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
        // A task's session is on neither tab, nor counted, nor on its project's screen among the sessions: it is reached
        // through its task.
        let task: Vec<bool> = self.sessions.iter().map(|s| self.is_task_session(s)).collect();
        let on_tab = |i: usize, s: &Session| listed(s) && !task[i];
        self.pstats = self.project_stats();
        self.archived_rows = 0;
        if let Some(open) = self.open_project.clone() {
            // A project's screen: its sessions, the active ones first and then the archived ones. A task's session is
            // listed as its task, under them (`project_tasks`).
            let mine = |i: usize, s: &Session| self.projects.project_id_of(&s.id) == Some(open.as_str()) && on_tab(i, s);
            let (active, archived): (Vec<usize>, Vec<usize>) =
                self.sessions.iter().enumerate().filter(|(i, s)| mine(*i, s)).map(|(i, _)| i).partition(|&i| !self.sessions[i].archived);
            self.tab_view = active.into_iter().chain(archived).collect();
            let view: Vec<usize> =
                self.tab_view.iter().copied().filter(|&i| q.is_empty() || self.sessions[i].haystack.contains(&q)).collect();
            self.archived_rows = view.iter().filter(|&&i| self.sessions[i].archived).count();
            self.view = view;
        } else if self.top != TOP_SESSIONS {
            self.tab_view.clear();
            self.view.clear();
        } else {
            self.tab_view = self.sessions.iter().enumerate().filter(|(i, s)| on_tab(*i, s) && shows(s)).map(|(i, _)| i).collect();
            let view: Vec<usize> =
                self.tab_view.iter().copied().filter(|&i| q.is_empty() || self.sessions[i].haystack.contains(&q)).collect();
            self.view = view;
        }
        self.counts =
            TABS.iter().map(|t| self.sessions.iter().enumerate().filter(|(i, s)| on_tab(*i, s) && (t.shows)(s)).count()).collect();
        self.update_project_rows(&q);
        self.clamp_selected();
    }

    pub fn selected_session(&self) -> Option<&Session> {
        self.view.get(self.selected).map(|&i| &self.sessions[i])
    }

    fn clamp_selected(&mut self) {
        self.selected = self.selected.min(self.row_count().saturating_sub(1));
        self.pselected = self.pselected.min(self.pview.len().saturating_sub(1));
    }

    /// Is anything on the screen moving with the clock: a spinner (a working conversation, the index, a call out to
    /// Claude, a squad starting, an install), an unread row's pulse, or a popup (which may hold one of those)? The
    /// event loop draws a new frame on each tick only then (`main.rs`, `Screen`). When unsure, it says yes.
    pub fn animating(&self) -> bool {
        !matches!(self.mode, Mode::List)
            || self.sessions.iter().any(|s| s.status == Status::Busy || s.unread)
            || self.indexing().is_some()
            || self.settling()
            || self.smart_busy()
            || self.handoff_busy()
            || self.draft_busy()
            || self.squads_busy()
            || !self.stopping.is_empty()
            || self.board_clock_running()
    }

    /// The sessions with a reply not yet looked at, as the header counts them (a task's session is its task's news).
    pub fn unread_count(&self) -> usize {
        self.sessions.iter().filter(|s| s.unread && !self.is_task_session(s)).count()
    }

    /// Transcripts still being read by the background index, or Some(0) while the very first pass is
    /// still listing files. None once it's caught up.
    pub fn indexing(&self) -> Option<usize> {
        match self.indexer.remaining {
            // The first pass only: every later one (once a second) is running whenever a refresh has just started it.
            0 if self.sessions.is_empty() && !self.indexer.first_scan_done => Some(0),
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
        // The whole width, and every row but the status line under it.
        (self.size.1.saturating_sub(1).max(1), self.size.0.max(1))
    }

    /// The terminal is below the smallest size platoon draws in (`MIN_WIDTH` x `MIN_HEIGHT`).
    pub fn too_small(&self) -> bool {
        too_small(self.size.0, self.size.1)
    }

    pub fn sync_size(&mut self, w: u16, h: u16) {
        if self.size == (w, h) {
            return;
        }
        self.size = (w, h);
        self.clear_selection();
        // Too small, the session isn't shown (a message is): it keeps the size it had rather than being squeezed into
        // fragments it would have to redraw, and gets the new size when the window is big enough again.
        if self.too_small() {
            return;
        }
        let (rows, cols) = self.pty_size();
        if let Some(c) = &mut self.client {
            c.resize(rows, cols);
        }
        if let Some(e) = &mut self.editing {
            e.run.resize(rows, cols);
        }
    }

    /// Where the session's screen is drawn in the attached view: the whole width, above the status bar.
    /// The same numbers as `pty_size` and `ui::draw_attached`.
    pub fn term_area(&self) -> Rect {
        let (rows, cols) = self.pty_size();
        Rect::new(0, 0, cols, rows)
    }

    /// Re-read Claude Code's state, carrying unread flags and the selection across.
    pub fn refresh(&mut self) {
        self.poll_news();
        self.spawned.retain_mut(|c| c.try_wait().ok().flatten().is_none());
        let guard_on = setup::guard_installed(self.reader.root());
        self.refresh_with(host::discover_in(&self.cache), guard_on);
        self.settle_notice();
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
        // The squads' files are small and few: read again while their tab shows, so a change made by hand is seen.
        if self.squads_showing() {
            self.reload_squads(None);
        }
        self.keep_attached();
        if let Some(pid) = self.attached_pid()
            && (self.client.is_none() || !self.hosts.iter().any(|h| h.claude_pid == pid))
        {
            self.session_ended();
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
        // Tasks' sessions are ordinary rows, marked with their task's number.
        if self.reread_tickets() {
            self.built.0 = u64::MAX;
        }
        let live = self.live_now();
        self.note_running_tasks(&live);
        // Where sessions are working now: no worktree is removed from under one.
        self.note_busy_dirs(&live);
        self.projects.reload_if_changed();
        // Sessions started under a project are placed in it once their id shows up.
        let ids: Vec<(u32, String, u64)> =
            live.iter().filter(|l| !l.is_background()).map(|l| (l.pid, l.id.clone(), l.started_at)).collect();
        // A session platoon started under a project is placed here; that is a membership change like any other,
        // so its project is told there is something new (the same as pressing `m`).
        for (session, project) in projects_mod::resolve_launches(&self.cache, &mut self.projects, &ids, &crate::procs::pid_alive) {
            self.joined_project(&project, &session);
        }
        // A new Codex session holds no conversation until it has opened (Codex's own questions first): its row stands for
        // its host meanwhile, never placed in a project above. Once it holds one, it is placed above by its real id.
        let live = self.with_unsaved(live);
        if self.open_project.as_ref().is_some_and(|p| self.projects.get(p).is_none()) {
            self.open_project = None; // deleted in another window
        }
        // A task added by `platoon task add` shows without a restart.
        self.refresh_tickets();

        // The background sessions running now, by the job id a parked session names (`Proc::moved_to`).
        let jobs: HashSet<&str> = live.iter().filter_map(|l| l.background.as_deref()).collect();
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
            // Saved since the row was built (the first message): the index brings the rest at its next pass.
            s.unsaved &= ls.iter().all(|l| l.unsaved);
            // Only interactive copies can fall out of sync with each other. A background session is run by
            // Claude's own service: it is never marked stale and never counts toward "several copies". Nor does a
            // parked one: it handed the conversation over and no longer writes to it.
            let shared = ls.iter().filter(|l| !l.is_background() && !l.is_parked()).count() > 1;
            for l in ls {
                if let Some(attach_id) = &l.background {
                    // `hosted`: a platoon host is showing it right now (its process is a `claude attach`, which has no
                    // registry entry of its own, so it never appears among `live`).
                    let hosted = self.hosts.iter().any(|h| h.attaches_to(attach_id));
                    s.procs.push(Proc {
                        pid: l.pid,
                        started_at: l.started_at,
                        hosted,
                        background: Some(attach_id.clone()),
                        stale: false,
                        parked: false,
                        moved_to: None,
                    });
                    continue;
                }
                let hosted = self.hosts.iter().any(|h| h.claude_pid == l.pid);
                if let Some(job) = &l.parked {
                    // Where it went, while that background session still runs.
                    let moved_to = jobs.contains(job.as_str()).then(|| job.clone());
                    s.procs.push(Proc {
                        pid: l.pid,
                        started_at: l.started_at,
                        hosted,
                        background: None,
                        stale: false,
                        parked: true,
                        moved_to,
                    });
                    continue;
                }
                if shared && hosted && !guard_on {
                    guard::mark_stale(&self.cache, l.pid, l.started_at);
                }
                let stale = guard::is_stale(&self.cache, l.pid, l.started_at);
                s.procs.push(Proc { pid: l.pid, started_at: l.started_at, hosted, background: None, stale, parked: false, moved_to: None });
            }
            // The name the user gave it in platoon wins over the one Claude was told.
            if let (Some(n), None) = (ls.iter().find_map(|l| l.name.as_ref()), self.names.get(&s.id)) {
                s.name.clone_from(n);
                s.name_source = NameSource::Claude;
                s.rehaystack();
            }
            // (A parked entry is never `busy`.) Moved only while nothing else runs the conversation: a copy opened
            // again since is what the row is about then.
            s.status = if ls.iter().any(|l| l.busy && !l.is_background()) {
                Status::Busy
            } else if ls.iter().any(|l| l.is_background()) {
                Status::Background
            } else if s.procs.iter().all(|p| p.parked) && s.procs.iter().any(|p| p.moved_to.is_some()) {
                Status::Moved
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
        self.poll_task_draft();
        self.poll_squad_starts();
        self.poll_squad_work();
        self.poll_merged();
        self.poll_older_hosts();
        self.poll_worktrees();
        self.drive_smart();
        self.usage = match snap.plan {
            Some(p) => Usage::Ready(p),
            None if setup::statusline_installed(self.reader.root()) => Usage::Waiting,
            None => Usage::NotSetUp,
        };
        let now = usage::now();
        self.codex_limits = self.providers.codex_root().map(|root| {
            crate::agents::codex::rate_limits(root, now).unwrap_or_default().into_iter().filter(|l| l.resets_at > now).collect()
        });
        self.rev += 1;
        self.update_view();
        if let Some(id) = selected_id
            && let Some(pos) = self.view.iter().position(|&i| self.sessions[i].id == id)
        {
            self.selected = pos;
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
        let due =
            settled || sessions_changed || settings_changed || self.last_auto_archive.is_none_or(|t| t.elapsed() >= AUTO_ARCHIVE_EVERY);
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
        // One being stopped for the archive key is archived by that (once its process is gone), not counted here. A
        // task's session is on neither tab: it goes with its task.
        ids.retain(|id| !self.stopping.contains_key(id) && self.sq.task_of(id).is_none());
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
                    format!("Auto-archived {n} session{plural} idle for {days}+ {day}. Find {them} in Archived."),
                    Instant::now(),
                    AUTO_ARCHIVE_NOTICE_FOR,
                    None,
                ));
            }
            Err(e) => self.notify(format!("Couldn't auto-archive: {e}")),
        }
    }

    /// Every process running a conversation now, Claude Code's and Codex's. A Codex conversation is held by the process
    /// that has its writer lock: when that runs under one of platoon's hosts (the host's child, or one of its children),
    /// it is told by the host's child, which is the process platoon attaches to and stops. Anything else holding it
    /// (another terminal, Codex's shared background server) is never platoon's.
    fn live_now(&self) -> Vec<Live> {
        let mut live = self.reader.live();
        let codex = self.codex.as_ref().map(codex::Reader::live).unwrap_or_default();
        for mut l in codex {
            if !self.hosts.iter().any(|h| h.claude_pid == l.pid)
                && let Some(h) = self.hosts.iter().find(|h| crate::guard::ancestors(l.pid).contains(&h.claude_pid))
            {
                l.pid = h.claude_pid;
            }
            live.push(l);
        }
        live
    }

    /// `live` with the new Codex sessions of platoon's hosts as `new_codex_sessions` makes them.
    fn with_unsaved(&self, live: Vec<Live>) -> Vec<Live> {
        let started = |h: &HostInfo| {
            std::fs::metadata(&h.sock)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or_else(|| usage::now() * 1000, |d| d.as_millis() as u64)
        };
        new_codex_sessions(live, &self.hosts, started)
    }

    /// The conversations to list: every saved one that has a real prompt, plus anything running (even before
    /// its first prompt is saved), newest activity first. Running-state fields are filled in by the caller.
    fn build_sessions(&self, live: &HashMap<&str, Vec<&Live>>) -> Vec<Session> {
        // The same id can be saved under two project folders; the most recently active file wins.
        let mut latest: HashMap<&str, &Arc<Entry>> = HashMap::new();
        // A program switched off in the settings is left out as if it weren't installed: its conversations aren't listed,
        // running or not (a running task of its stays on the Tasks tab).
        for e in self.indexer.entries.values().filter(|e| !self.providers.is_off(e.agent)) {
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
            if !latest.contains_key(id) && !self.providers.is_off(ls[0].agent) {
                out.push(self.session_from_live(ls[0]));
            }
        }
        out.sort_by(|a, b| b.last_active.cmp(&a.last_active).then_with(|| a.id.cmp(&b.id)));
        out
    }

    fn session_from(&self, e: &Entry) -> Session {
        let t = &e.t;
        let cwd = t.cwd.clone().unwrap_or_default();
        let agent = e.agent;
        let est = (t.context_tokens * 100 / agent.context_window(t, self.claude_large_off)).min(100) as u8;
        let description = t.ai_title.clone().or_else(|| t.first_prompt.clone()).unwrap_or_default();
        let branch = t.branch.clone().filter(|b| b != "HEAD");
        let model = t.model.as_deref().map(|m| agent.model_name(m));
        let mut s = self.session(agent, e.id.clone(), cwd, branch, model, est, description, e.last_active_ms());
        if !t.extra_dirs.is_empty() {
            s.extra_dirs = t.extra_dirs.iter().map(|d| dirs::tilde(d, &self.home)).collect();
            s.rehaystack();
        }
        s
    }

    /// A running conversation whose transcript isn't saved yet (nothing sent), described by what its agent says of
    /// the process running it.
    fn session_from_live(&self, l: &Live) -> Session {
        let s = self.session(l.agent, l.id.clone(), l.cwd.clone(), None, None, 0, String::new(), l.started_at);
        Session { unsaved: l.unsaved, ..s }
    }

    #[allow(clippy::too_many_arguments)]
    fn session(
        &self,
        agent: Agent,
        id: String,
        cwd: String,
        branch: Option<String>,
        model: Option<String>,
        est: u8,
        description: String,
        last_active: u64,
    ) -> Session {
        // Claude Code derives a placeholder name; only a name the user chose (set from the registry) replaces this, and
        // a name the user gave it in platoon replaces both.
        let generated = self.summaries.get(&id);
        let (name, name_source) = match (self.names.get(&id), generated) {
            (Some(n), _) => (n.to_string(), NameSource::Platoon),
            (None, Some(g)) => (g.name.clone(), NameSource::Generated),
            (None, None) => (
                Path::new(&cwd).file_name().and_then(|n| n.to_str()).filter(|n| !n.is_empty()).unwrap_or(&cwd).to_string(),
                NameSource::Folder,
            ),
        };
        let summary = generated.map(|g| g.summary.clone()).unwrap_or_default();
        let dir = dirs::tilde(&cwd, &self.home);
        let haystack = Session::make_haystack(&name, &dir, &[], branch.as_deref(), &description, &summary);
        Session {
            id,
            unsaved: false,
            agent,
            name,
            name_source,
            summary,
            dir,
            extra_dirs: Vec::new(),
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
            (Action::Jump, Mode::List) if self.project_task_picked().is_some() => self.open_project_task(),
            (Action::Jump, Mode::List) => self.jump(),
            (Action::Assign, Mode::List) if !self.projects_list_showing() => self.open_assign(),
            (Action::Delete, Mode::List) if self.projects_list_showing() => self.ask_delete_project(),
            (Action::Look, Mode::List) if self.projects_list_showing() => self.open_look(),
            (Action::Leave, Mode::List) if self.tk.open.is_some() => self.close_ticket_screen(),
            (Action::Leave, Mode::List) if self.squads_showing() => self.close_squad(),
            (Action::Leave, Mode::List) => self.close_project_screen(),
            (Action::Confirm, Mode::TaskProject) => self.take_task_project(),
            (Action::Confirm, Mode::SquadPick) => self.take_squad_pick(),
            (Action::Confirm, Mode::TaskDraft) => self.make_drafted(),
            (Action::Cancel, Mode::TaskDraft) => self.drop_draft(),
            (Action::Cancel, Mode::PickEditor) if self.epick.file.is_some() => {
                if let Some(e) = self.epick.file.take() {
                    self.mode = Self::file_edit_mode(&e);
                    self.drop_file_edit(e);
                }
            }
            (Action::Confirm, Mode::Confirm(Confirm::CancelTicket { number, pid, .. })) => self.cancel_ticket(number, pid),
            (Action::Edit, Mode::List) => self.edit_notes(),
            (Action::ShowChanges, Mode::List) => self.show_changes(),
            (Action::Undo, Mode::List) => self.undo_notes(),
            (Action::HandOff, Mode::List) if !self.projects_list_showing() => self.start_handoff(),
            (Action::Editor, Mode::List) => self.code_editor_key(),
            (Action::Help, Mode::List) => self.open_help(),
            (Action::Confirm, Mode::Confirm(Confirm::EditorWhileWorking { .. })) => self.editor_anyway(),
            (Action::Cancel, Mode::Confirm(Confirm::EditorWhileWorking { .. })) => self.editor_cancel(),
            (Action::Confirm, Mode::Handoff) => self.take_handoff(),
            (Action::Cancel, Mode::Handoff) => self.cancel_handoff(),
            (Action::Confirm, Mode::PickEditor) => self.take_editor_pick(),
            (Action::Confirm, Mode::PickFileEditor) => self.take_file_editor_pick(),
            (Action::Cancel, Mode::PickFileEditor) => self.cancel_file_editor_pick(),
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
            (Action::Confirm, Mode::Confirm(Confirm::DeleteSquad { id, name, replaces })) => self.delete_squad(&id, &name, replaces),
            (Action::Confirm, Mode::Confirm(Confirm::DeleteProject { id, .. })) => {
                self.mode = Mode::List;
                self.delete_project(&id);
            }
            (Action::Back, Mode::Attached(_)) => self.detach(),
            // The notes editor's hint and its "Leave the editor?" question, by mouse.
            (Action::Back, Mode::EditNotes) => self.ask_leave_editor(),
            (Action::Confirm, Mode::EditNotes) => self.answer_leave_editor(true),
            (Action::Cancel, Mode::EditNotes) => self.answer_leave_editor(false),
            (Action::Confirm, Mode::Confirm(Confirm::Archive { id, name, .. })) => {
                self.mode = Mode::List;
                self.archive_conversation(&id, &name);
            }
            (Action::Confirm, Mode::Confirm(Confirm::Adopt { pids, id, name, cwd, restore, .. })) => {
                self.mode = Mode::List;
                self.adopt(&pids, &id, &name, cwd, restore);
            }
            (Action::Confirm, Mode::Picker) if self.picker.editor => self.take_editor_folder(),
            (Action::Confirm, Mode::Picker) if self.picker.new_task.is_some() => {
                if let (Some(dir), Some(title)) = (self.picker.results.get(self.picker.selected).cloned(), self.picker.new_task.clone()) {
                    self.new_task_folder(&title, dir);
                }
            }
            // Leaving the folder picker of a new task in no project: nothing is written (a task needs a folder).
            (Action::Cancel, Mode::Picker) if self.picker.new_task.is_some() => {
                self.mode = Mode::List;
                self.notify("No task was made: a task in no project needs a folder to work in.");
            }
            (Action::Confirm, Mode::Picker) if self.picker.task_folder.is_some() => {
                if let (Some(dir), Some(n)) = (self.picker.results.get(self.picker.selected).cloned(), self.picker.task_folder) {
                    self.take_task_folder(n, dir);
                }
            }
            (Action::Confirm, Mode::Picker) => {
                if let Some(dir) = self.picker.results.get(self.picker.selected).cloned() {
                    let project = self.picker.project.clone();
                    self.new_session_in(dir, project);
                }
            }
            (Action::Confirm, Mode::PickAgent) => self.take_agent_pick(),
            (Action::Cancel, Mode::PickAgent) => self.cancel_agent_pick(),
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
        let key = (
            self.search.kind,
            self.search.text.clone(),
            self.prompts.generation,
            self.rev,
            settings.search_archived,
            settings.search_results,
        );
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
            let name = format!("{} {}", self.names.get(&s.id).unwrap_or(""), self.summaries.get(&s.id).map_or("", |g| g.name.as_str()))
                .to_lowercase();
            let summary = s.summary.to_lowercase();
            let data = self.prompts.get(&s.id);
            if let Some(f) = prompts::find_with(data.map(|d| &**d), &name, &summary, &words) {
                found.push((s, f, data));
            }
        }
        found.sort_by(|a, b| {
            let key = |x: &(&Session, prompts::Found, _)| {
                (Reverse(x.1.in_name), Reverse(x.1.in_one_prompt), Reverse(x.1.hits), Reverse(x.0.last_active))
            };
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
                SearchResult {
                    id: s.id.clone(),
                    name: s.name.clone(),
                    dir: s.dir.clone(),
                    extra_dirs: s.extra_dirs.clone(),
                    archived: s.archived,
                    last_active: s.last_active,
                    detail,
                    marks,
                }
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
        self.pane = Pane::Sessions;
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
        if self.refuse_unsaved("renamed") {
            return;
        }
        let Some(s) = self.selected_session() else {
            self.notify("No session selected");
            return;
        };
        self.rename = Rename { id: s.id.clone(), input: s.name.clone(), original: s.name.clone() };
        self.mode = Mode::Rename;
    }

    /// Enter in the rename box. The same text as before changes nothing; an empty one gives the conversation back its
    /// automatic name.
    fn save_rename(&mut self) {
        self.mode = Mode::List;
        let Rename { id, input, original } =
            std::mem::replace(&mut self.rename, Rename { id: String::new(), input: String::new(), original: String::new() });
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
        self.picker = Picker {
            input: String::new(),
            all,
            results: Vec::new(),
            selected: 0,
            project,
            task_folder: None,
            editor: false,
            new_task: None,
        };
        self.picker.update(&self.home);
        self.mode = Mode::Picker;
    }

    /// Show the settings screen, starting from what is on disk now, on the Theme entry (below the Programs, so a
    /// habitual `s` then `enter` never switches a program off).
    fn open_settings(&mut self) {
        self.refresh_settings();
        self.settings_tab = 0;
        self.setting = SETTINGS_TABS[0].items.iter().position(|i| i.kind == Kind::Theme).unwrap_or(0);
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
        if let Kind::Editor(which) = item.kind {
            self.step_editor_setting(which, step);
            return;
        }
        if let Kind::Model(which) = item.kind {
            self.step_model_setting(which, step);
            return;
        }
        if let Kind::Program(agent) = item.kind {
            self.step_program_setting(agent, step);
            return;
        }
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

    /// The editors an editor setting chooses among, as (id, name), and the id it holds now.
    fn editor_setting_options(&self, which: EditorFor) -> (Vec<(String, String)>, String) {
        let s = self.settings.get();
        match which {
            EditorFor::Text => (self.editors.iter().map(|e| (e.id.clone(), e.name.clone())).collect(), s.notes_editor),
            EditorFor::Files => {
                let options: Vec<(String, String)> = self.file_editors().iter().map(|e| (e.id.clone(), e.name.clone())).collect();
                // None chosen yet: `v` opens the first one found, so that is the one shown.
                let id = if s.files_editor.is_empty() { options.first().map(|o| o.0.clone()).unwrap_or_default() } else { s.files_editor };
                (options, id)
            }
        }
    }

    /// How an editor setting reads on the settings screen: its value, the widest value it can take, and whether it
    /// can go no further back or on.
    pub fn editor_setting(&self, which: EditorFor) -> (String, usize, bool, bool) {
        let (options, id) = self.editor_setting_options(which);
        let at = options.iter().position(|(o, _)| *o == id);
        let label = match at {
            Some(i) => options[i].1.clone(),
            None if which == EditorFor::Text && id == crate::editors::ASK => "None chosen yet".to_string(),
            None if which == EditorFor::Text => format!("{id} (not found)"),
            None if id.is_empty() => "None found".to_string(),
            None => format!("{} (not found)", crate::fileeditors::name_of(&id)),
        };
        let widest = options.iter().map(|(_, n)| n.chars().count()).chain([label.chars().count()]).max().unwrap_or(0);
        let n = options.len();
        (label, widest, at == Some(0), at.is_some_and(|i| i + 1 >= n) || n == 0)
    }

    /// Step an editor setting through the editors this computer has, and save it at once.
    fn step_editor_setting(&mut self, which: EditorFor, step: Step) {
        let (options, id) = self.editor_setting_options(which);
        let Some(chosen) = step_among(&options, &id, step) else { return };
        let mut s = self.settings.get();
        match which {
            EditorFor::Text => s.notes_editor = chosen,
            EditorFor::Files => s.files_editor = chosen,
        }
        let saved = self.settings.update(&s);
        self.report_save(saved);
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
        if !self.hosts.iter().any(|h| h.claude_pid == pid) {
            self.notify("That session's host isn't running");
            return;
        }
        self.reconnected = None;
        if let Err(e) = self.connect(pid) {
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

    /// Show the session `pid` runs in the attached view, connected to its host at the view's size.
    fn connect(&mut self, pid: u32) -> io::Result<()> {
        let sock = self.hosts.iter().find(|h| h.claude_pid == pid).map(|h| h.sock.clone()).ok_or(io::ErrorKind::NotFound)?;
        let (rows, cols) = self.pty_size();
        self.client = Some(Client::connect(&sock, rows, cols)?);
        self.mode = Mode::Attached(pid);
        self.clear_selection();
        if let Some(s) = self.sessions.iter_mut().find(|s| s.procs.iter().any(|p| p.pid == pid)) {
            s.unread = false;
        }
        Ok(())
    }

    /// Keep the attached view connected (the event loop asks on every pass). A host closes the connection of a window
    /// it can't feed in time (`host::cut_off`): while the host still runs, connect again at once, which brings the
    /// screen up to date just as leaving and coming back would. Only when that fails (the host is gone), or the
    /// connection closes again right after it was made, does the view end: "Session ended".
    pub fn keep_attached(&mut self) {
        let Some(pid) = self.attached_pid() else { return };
        if !self.client.as_ref().is_some_and(Client::closed) {
            return;
        }
        let again_at_once = self.reconnected.is_some_and(|t| t.elapsed() < crate::client::RECONNECT_GAP);
        if again_at_once || self.connect(pid).is_err() {
            self.session_ended();
        } else {
            self.reconnected = Some(Instant::now());
        }
    }

    /// Back to the list: the attached session has ended.
    fn session_ended(&mut self) {
        self.mode = Mode::List;
        self.client = None;
        self.clear_selection();
        self.notify("Session ended");
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
            // A host platoon just started for it, whose process hasn't said which conversation it runs yet: that one.
            return Some(match self.hosts.iter().find(|h| h.session().as_deref() == Some(s.id.as_str())) {
                Some(h) => JumpPlan::Attach(h.claude_pid),
                None => JumpPlan::Resume,
            });
        }
        if s.held_elsewhere() {
            return Some(JumpPlan::Elsewhere);
        }
        // Claude's own service runs it: open its screen with `claude attach`, in a platoon host of our own (or
        // the one already showing it). Ending or resuming it is not on offer.
        if let Some(attach_id) = s.procs.iter().find_map(|p| p.background.as_ref()) {
            return Some(match self.hosts.iter().find(|h| h.attaches_to(attach_id)) {
                Some(h) => JumpPlan::Attach(h.claude_pid),
                None => JumpPlan::AttachBackground(attach_id.clone()),
            });
        }
        // Only copies that still write to the conversation count below. A parked process handed it over to a
        // background session: that session is where it went on, so opening the row opens it (joining a host already
        // showing it). With that session gone, the row is an idle conversation: its own platoon host if it has one,
        // else reopened from the saved history. A parked process is never ended or taken over.
        let writers: Vec<&Proc> = s.procs.iter().filter(|p| !p.parked).collect();
        if writers.is_empty() {
            if let Some(job) = s.procs.iter().find_map(|p| p.moved_to.as_ref()) {
                return Some(match self.hosts.iter().find(|h| h.attaches_to(job)) {
                    Some(h) => JumpPlan::Attach(h.claude_pid),
                    None => JumpPlan::AttachBackground(job.clone()),
                });
            }
            return Some(match s.procs.iter().find(|p| p.hosted) {
                Some(p) => JumpPlan::Attach(p.pid),
                None => JumpPlan::Resume,
            });
        }
        if let Some(p) = writers.iter().find(|p| p.hosted && !p.stale) {
            return Some(JumpPlan::Attach(p.pid));
        }
        let kind = if writers.len() == 1 && !writers[0].hosted { AdoptKind::TakeOver } else { AdoptKind::Reload };
        Some(JumpPlan::Adopt { pids: writers.iter().map(|p| p.pid).collect(), kind, busy: s.status == Status::Busy })
    }

    fn jump(&mut self) {
        let Some(id) = self.selected_session().map(|s| s.id.clone()) else { return };
        self.jump_to(&id);
    }

    /// Enter on a conversation, wherever it was picked (a row, or the "From" line of a ticket).
    pub(crate) fn jump_to(&mut self, id: &str) {
        let Some(s) = self.sessions.iter().find(|s| s.id == id) else { return };
        let Some(plan) = self.jump_plan_of(s) else { return };
        let (id, name, cwd, restore, agent) = (s.id.clone(), s.name.clone(), s.cwd.clone(), s.archived, s.agent);
        match plan {
            JumpPlan::Attach(pid) => {
                self.attach(pid);
                if self.attached_pid().is_some() {
                    self.note_opened(&id, restore);
                }
            }
            JumpPlan::Elsewhere => {
                self.notify(format!("\"{name}\" is open in another {} window. Close it there to open it here.", agent.name()))
            }
            JumpPlan::Resume => {
                if !self.cwd_exists(&name, &cwd) {
                    return;
                }
                let project = self.projects.project_id_of(&id).map(String::from);
                if self.launch_in(agent, cwd, Start::Resume(&id), project.as_deref(), false) {
                    self.note_opened(&id, restore);
                }
            }
            JumpPlan::AttachBackground(attach_id) => {
                if !self.cwd_exists(&name, &cwd) {
                    return;
                }
                if self.launch(agent, cwd, Start::Attach(&attach_id)) {
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
        // The row it was picked on, unless a refresh took it away meanwhile: then Claude Code's, like every row.
        let agent = self.sessions.iter().find(|s| s.id == id).map_or(Agent::Claude, |s| s.agent);
        if self.launch_in(agent, cwd, Start::Resume(id), project.as_deref(), false) {
            self.note_opened(id, restore);
        }
    }

    fn toggle_archive(&mut self) {
        if self.refuse_unsaved("archived") {
            return;
        }
        let Some(s) = self.selected_session() else {
            self.notify("No session selected");
            return;
        };
        let (id, name) = (s.id.clone(), s.name.clone());
        if s.procs.iter().any(|p| p.background.is_some()) {
            // Archiving stops what runs the conversation, and this one is run by Claude's own service.
            self.notify(format!("\"{name}\" runs in the background, so platoon can't archive it"));
            return;
        }
        if s.held_elsewhere() {
            // Archiving stops what runs the conversation, and this window isn't platoon's to stop.
            self.notify(format!("\"{name}\" is open in another {} window. Close it there to archive it.", s.agent.name()));
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
        // Never a background session's process: Claude's own service owns it (the archive key refuses those rows). Nor a
        // window of an agent that keeps one per conversation, unless platoon started it (`Session::held_elsewhere`).
        let pids: Vec<u32> = self
            .sessions
            .iter()
            .filter(|s| s.id == id)
            .flat_map(|s| s.procs.iter().filter(|p| p.background.is_none() && (p.hosted || !s.agent.keeps_one_writer())).map(|p| p.pid))
            .collect();
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
                let again = self.live_now().iter().any(|l| l.id == done.id && l.background.is_none() && !pids.contains(&l.pid));
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

    /// Start a session of `agent` as `start` says under a new host in `dir` and attach to it. False if it couldn't be
    /// started.
    fn launch(&mut self, agent: Agent, dir: String, start: Start) -> bool {
        self.launch_in(agent, dir, start, None, false)
    }

    /// The program that runs `agent`'s sessions. (A unit test starts none: `launched`.)
    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn program_of(&self, agent: Agent) -> String {
        match agent {
            Agent::Claude => self.program.clone(),
            Agent::Codex => self.codex_program.clone(),
        }
    }

    /// Why a new session of `agent`'s can't be started for the conversation named `name`, in plain words: the agent
    /// isn't installed here (any more), or is switched off in the settings. None when it can be.
    pub(crate) fn agent_gone(&self, agent: Agent, name: &str) -> Option<String> {
        self.providers.program(agent).is_none().then(|| {
            let why =
                if self.providers.is_off(agent) { "is switched off in platoon's settings" } else { "isn't installed on this computer" };
            format!("\"{name}\" is a {0} conversation, and {0} {why}, so nothing was started.", agent.name())
        })
    }

    /// The folder `agent` keeps its saved conversations under, as this window reads it; None for Codex when this
    /// window reads no Codex folder (a unit test that gave it none).
    pub(crate) fn agent_root(&self, agent: Agent) -> Option<PathBuf> {
        match agent {
            Agent::Claude => Some(self.reader.root().to_path_buf()),
            Agent::Codex => self.codex.as_ref().map(|c| c.root().to_path_buf()),
        }
    }

    /// `launch`, for a session of `project`: claude gets `PLATOON_PROJECT` (so the notes hook knows), and with
    /// `place` (a new session) the process is remembered until its session id shows up and is put in the project.
    fn launch_in(&mut self, agent: Agent, dir: String, start: Start, project: Option<&str>, place: bool) -> bool {
        let (rows, cols) = self.pty_size();
        let env: Vec<(&str, &str)> = project.map(|p| ("PLATOON_PROJECT", p)).into_iter().collect();
        let args = agent.args(start);
        let at_ms = usage::now().saturating_sub(2) * 1000;
        // A unit test's program would be the real one: nothing is started, what would have been is noted.
        #[cfg(test)]
        {
            let _ = (rows, cols, at_ms, place);
            let env = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            self.launched.push(Launched { agent, dir, args, env });
            self.mode = Mode::List;
            true
        }
        #[cfg(not(test))]
        match host::launch(&self.host_dirs(), agent, &self.program_of(agent), &dir, &args, &env, rows, cols) {
            Ok((info, child)) => {
                let pid = info.claude_pid;
                if let (Some(p), true) = (project, place)
                    && let Err(e) = projects_mod::note_launch(&self.cache, pid, p, at_ms)
                {
                    self.notify(format!("Couldn't note the new session's project: {e}"));
                }
                self.spawned.push(child);
                self.hosts.push(info);
                self.filter.clear();
                self.attach(pid);
                true
            }
            Err(e) => {
                self.mode = Mode::List;
                self.notify(format!("Couldn't start {}: {e}", agent.id()));
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

    /// The index read on a background thread, as `App::new` has it (`with_dirs` reads it on the calling thread).
    #[cfg(test)]
    pub fn index_in_background_for_test(&mut self) {
        self.indexer = Indexer::spawn(self.reader.root(), self.codex.as_ref().map(codex::Reader::root), &self.cache);
    }

    #[cfg(test)]
    pub fn release_index_for_test(&mut self) {
        self.indexer.release_for_test(self.reader.root(), self.codex.as_ref().map(codex::Reader::root), &self.cache);
    }

    #[cfg(test)]
    pub fn first_scan_done_for_test(&self) -> bool {
        self.indexer.first_scan_done
    }

    /// What a refresh does to the index first: take in what it has read and start its next pass.
    #[cfg(test)]
    pub fn poll_index_for_test(&mut self) {
        self.indexer.poll();
    }

    #[cfg(test)]
    pub fn reload_for_test(&mut self) {
        self.reload();
    }

    #[cfg(test)]
    pub fn picker_launch_for_test(&mut self, dir: String) {
        self.launch(Agent::Claude, dir, Start::New(None));
    }

    #[cfg(test)]
    pub fn cache_for_test(&self) -> &Path {
        &self.cache
    }

    fn move_selection(&mut self, delta: i32) {
        let last = self.row_count().saturating_sub(1) as i32;
        self.selected = (self.selected as i32 + delta).clamp(0, last) as usize;
    }

    fn move_picker(&mut self, delta: i32) {
        let last = self.picker.results.len().saturating_sub(1) as i32;
        self.picker.selected = (self.picker.selected as i32 + delta).clamp(0, last) as usize;
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        self.key(key);
        self.settle_notice();
    }

    fn key(&mut self, key: KeyEvent) {
        let own_screen = !matches!(self.mode, Mode::Attached(_) | Mode::EditNotes);
        if self.too_small() {
            return self.too_small_key(key, own_screen);
        }
        self.dismiss_long_notice();
        self.dismiss_news();
        // Something else took the screen while the help was open: it doesn't come back with the screen later.
        if self.help_open && !self.help_showing() {
            self.help_open = false;
        }
        let back = self.keys.get().back.matches(&key);
        // On platoon's own screens and popups the back key is a second esc: one level back, or cancel. (In a session
        // and in the notes editor it is theirs to handle, below: everything else there is typing.)
        let key = if back && own_screen { KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE) } else { key };
        // Ctrl+Space when it isn't the back key is nothing here (not the space that switches or picks things).
        if own_screen && key.code == KeyCode::Char(' ') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if self.help_showing() {
            return self.help_key(key);
        }
        // `?` on platoon's own screens (never where it is typed: a session, the notes editor, a box or a list being
        // typed into are other modes) shows every key of the screen.
        if key.code == KeyCode::Char('?') && !ctrl && !alt && matches!(self.mode, Mode::List) {
            return self.act(Action::Help);
        }
        match self.mode.clone() {
            Mode::Attached(_) => {
                // Typing moves the screen on (and the highlight would no longer be over what was selected).
                self.clear_selection();
                // The back key: everything else goes to Claude.
                if back {
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
                KeyCode::Up | KeyCode::Char('k') => self.move_setting(-1),
                KeyCode::Down | KeyCode::Char('j') => self.move_setting(1),
                KeyCode::PageUp => self.move_setting(-page_of(&self.pages.settings)),
                KeyCode::PageDown => self.move_setting(page_of(&self.pages.settings)),
                KeyCode::Tab => self.step_settings_tab(1),
                KeyCode::BackTab => self.step_settings_tab(-1),
                KeyCode::Home | KeyCode::Char('g') => self.setting = 0,
                KeyCode::End | KeyCode::Char('G') => self.setting = self.items().len().saturating_sub(1),
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
            Mode::TaskProject => self.task_project_key(key),
            Mode::TaskDraft => self.task_draft_key(key),
            Mode::SquadPick => self.squad_pick_key(key),
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
            Mode::PickAgent => match key.code {
                KeyCode::Esc => self.act(Action::Cancel),
                KeyCode::Enter => self.act(Action::Confirm),
                KeyCode::Char('c') if ctrl => self.act(Action::Cancel),
                KeyCode::Up | KeyCode::Left | KeyCode::BackTab => self.move_agent_pick(-1),
                KeyCode::Down | KeyCode::Right | KeyCode::Tab => self.move_agent_pick(1),
                _ => {}
            },
            Mode::PickFileEditor => match key.code {
                KeyCode::Esc => self.act(Action::Cancel),
                KeyCode::Enter => self.act(Action::Confirm),
                KeyCode::Char('c') if ctrl => self.act(Action::Cancel),
                KeyCode::Up => self.move_file_editor_pick(-1),
                KeyCode::Down => self.move_file_editor_pick(1),
                KeyCode::Char('g') | KeyCode::Home => self.move_file_editor_pick(-1000),
                KeyCode::Char('G') | KeyCode::End => self.move_file_editor_pick(1000),
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

    /// A key while the terminal is too small to show anything but the message saying so: nothing happens unseen. On
    /// platoon's own screens the quit key and ctrl+c quit (the message says so); in a session or the notes editor every
    /// key is theirs, so none is sent while they can't be seen.
    fn too_small_key(&mut self, key: KeyEvent, own_screen: bool) {
        let ctrl_c = key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
        if own_screen && (ctrl_c || self.keys.get().quit.matches(&key)) {
            self.quit = true;
        }
    }

    /// The help popup is showing: open, and over one of the screens it opens on (a question that pops up over the
    /// screen meanwhile takes the keys, and closes it).
    pub fn help_showing(&self) -> bool {
        self.help_open && matches!(self.mode, Mode::List)
    }

    fn open_help(&mut self) {
        self.help_open = true;
        self.help_top.set(0);
    }

    /// Keys while the help popup is open: it scrolls, and esc (or the back key), `?` or the quit key close it.
    fn help_key(&mut self, key: KeyEvent) {
        let top = self.help_top.get();
        match key.code {
            KeyCode::Esc | KeyCode::Char('?') => self.help_open = false,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => self.help_open = false,
            _ if self.keys.get().quit.matches(&key) => self.help_open = false,
            KeyCode::Down | KeyCode::Char('j') => self.help_top.set(top + 1),
            KeyCode::Up | KeyCode::Char('k') => self.help_top.set(top.saturating_sub(1)),
            KeyCode::PageDown | KeyCode::Char(' ') => self.help_top.set(top + 10),
            KeyCode::PageUp => self.help_top.set(top.saturating_sub(10)),
            KeyCode::Home | KeyCode::Char('g') => self.help_top.set(0),
            // As far as it goes: the drawing keeps it in range.
            KeyCode::End | KeyCode::Char('G') => self.help_top.set(usize::MAX / 2),
            _ => {}
        }
    }

    pub fn on_paste(&mut self, text: &str) {
        self.paste(text);
        self.settle_notice();
    }

    fn paste(&mut self, text: &str) {
        if self.too_small() || self.help_showing() {
            return;
        }
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
        self.mouse_event(m);
        self.settle_notice();
    }

    fn mouse_event(&mut self, m: MouseEvent) {
        self.mouse = (m.column, m.row);
        // Too small, nothing is on screen to click or scroll.
        if self.too_small() {
            return;
        }
        if matches!(m.kind, MouseEventKind::Down(_)) {
            self.dismiss_long_notice();
            self.dismiss_news();
        }
        if self.help_showing() {
            return self.help_mouse(m);
        }
        match m.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let up = m.kind == MouseEventKind::ScrollUp;
                match self.mode.clone() {
                    Mode::Attached(_) => {
                        // The wheel drops the selection, as typing does (a drag scrolls by itself past an edge).
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
                    Mode::SquadPick => self.move_squad_pick(if up { -1 } else { 1 }),
                    Mode::TaskProject => self.move_task_project_pick(if up { -1 } else { 1 }),
                    Mode::TaskDraft => self.scroll_draft(if up { -3 } else { 3 }),
                    Mode::PickEditor => self.move_editor_pick(if up { -1 } else { 1 }),
                    Mode::PickFileEditor => self.move_file_editor_pick(if up { -1 } else { 1 }),
                    Mode::PickAgent => self.move_agent_pick(if up { -1 } else { 1 }),
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
                let hit = self.hits.borrow().iter().find(|(r, _)| r.contains((m.column, m.row).into())).map(|(_, h)| *h);
                match hit {
                    Some(Hit::Action(a)) => self.act(a),
                    Some(Hit::Tab(i)) if self.projects_list_showing() => self.switch_ptab(i),
                    Some(Hit::Tab(i)) if self.board_showing() => self.switch_board_shelf(i),
                    Some(Hit::Tab(i)) => self.switch_tab(i),
                    Some(Hit::Top(i)) if matches!(self.mode, Mode::List) => self.switch_top(i),
                    Some(Hit::Top(_)) => {}
                    Some(Hit::ProjectRow(i)) if i == self.pselected => self.act(Action::Jump),
                    Some(Hit::ProjectRow(i)) => self.pselected = i,
                    Some(Hit::AssignRow(i)) if i == self.assign.selected => self.act(Action::Confirm),
                    Some(Hit::AssignRow(i)) => self.assign.selected = i,
                    Some(Hit::EditorRow(i)) if i == self.epick.selected => self.act(Action::Confirm),
                    Some(Hit::EditorRow(i)) => self.epick.selected = i,
                    Some(Hit::FileEditorRow(i)) if i == self.file_pick().selected => self.act(Action::Confirm),
                    Some(Hit::FileEditorRow(i)) => self.move_file_editor_pick(i as i32 - self.file_pick().selected as i32),
                    Some(Hit::AgentRow(i)) => self.click_agent_pick(i),
                    Some(Hit::Slot(i)) => self.look.slot = i,
                    Some(Hit::Pane(p)) => self.pane = p,
                    Some(Hit::TicketRow(i)) if self.board_showing() && self.tk.open.is_none() => self.click_board_row(i),
                    Some(Hit::TicketRow(_)) => {}
                    Some(Hit::ProjectPickRow(i)) if i == self.bd.pick_selected => self.act(Action::Confirm),
                    Some(Hit::ProjectPickRow(i)) => self.bd.pick_selected = i,
                    Some(Hit::SquadRow(i)) => self.click_squad_row(i),
                    Some(Hit::SquadsTabRow(i)) if self.squads_showing() && matches!(self.mode, Mode::List) => self.click_squads_row(i),
                    Some(Hit::SquadsTabRow(_)) => {}
                    Some(Hit::SquadSetting(i)) if self.squads_showing() && matches!(self.mode, Mode::List) => self.click_squad_setting(i),
                    Some(Hit::SquadSetting(_)) => {}
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
                    Some(Hit::Row(i)) if i == self.selected && (self.open_project.is_none() || self.pane == Pane::Sessions) => {
                        self.act(Action::Jump)
                    }
                    Some(Hit::Row(i)) => {
                        self.selected = i;
                        self.pane = Pane::Sessions;
                    }
                    Some(Hit::PickerRow(i)) if i == self.picker.selected => self.act(Action::Confirm),
                    Some(Hit::PickerRow(i)) => self.picker.selected = i,
                    // The previewed entry is the one that gets kept: clicking it again is "use this".
                    Some(Hit::ThemeRow(i)) if i == self.tpick.selected => self.act(Action::Confirm),
                    Some(Hit::ThemeRow(i)) => self.select_theme(i),
                    Some(Hit::DraftTask(i)) => self.click_draft(i),
                    Some(Hit::Inert) => {}
                    // Nothing clickable here: in the attached view that is the session's own text.
                    None => self.begin_selection(m),
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => self.extend_selection(m),
            MouseEventKind::Up(MouseButton::Left) => self.finish_selection(),
            // The pointer moving with no button held: a release the terminal never reported (it was let go outside
            // the window). Whatever happens to the selection, the view stops scrolling.
            MouseEventKind::Moved => {
                if let Some(d) = &mut self.drag {
                    d.past = None;
                }
            }
            _ => {}
        }
        self.update_view();
    }

    /// The wheel scrolls the help popup; a click outside it closes it (one inside does nothing).
    fn help_mouse(&mut self, m: MouseEvent) {
        let top = self.help_top.get();
        match m.kind {
            MouseEventKind::ScrollUp => self.help_top.set(top.saturating_sub(3)),
            MouseEventKind::ScrollDown => self.help_top.set(top + 3),
            MouseEventKind::Down(MouseButton::Left) => {
                let inside = self.hits.borrow().iter().any(|(r, h)| matches!(h, Hit::Inert) && r.contains((m.column, m.row).into()));
                if !inside {
                    self.help_open = false;
                }
            }
            _ => {}
        }
    }

    fn clear_selection(&mut self) {
        self.selection = None;
        self.drag = None;
    }

    /// A press on the session's text starts a selection (a double or triple click one by word or line). It only
    /// counts as a selection once it covers something: a plain click just clears the old one. Never on the status
    /// bar, the frame, or a session that is paused (its text is under the banner).
    fn begin_selection(&mut self, m: MouseEvent) {
        self.clear_selection();
        if !matches!(self.mode, Mode::Attached(_)) || self.attached_paused() {
            return;
        }
        let Some(client) = &self.client else { return };
        let area = self.term_area();
        if !area.contains((m.column, m.row).into()) {
            return;
        }
        let row = m.row - area.y;
        let at = Point::new(client.history(|h| h.top()) + usize::from(row), m.column - area.x);
        let now = Instant::now();
        let count = click_count(self.last_click, now, at);
        self.last_click = Some(Click { at: now, point: at, count });
        let unit = match count {
            1 => Unit::Char,
            2 => Unit::Word,
            _ => Unit::Line,
        };
        self.selection = Some(Selection::new(at, unit));
        self.drag = Some(Drag::new(row));
    }

    /// The head follows the pointer. Past the top or bottom edge of the session's screen it stays on the edge line
    /// and the view scrolls (`drag_scroll`), back through the history above or on toward the live screen below;
    /// where the session has asked to hear the mouse there is no history of platoon's to scroll, and it just stays
    /// on the edge.
    fn extend_selection(&mut self, m: MouseEvent) {
        let area = self.term_area();
        let scrolls = self.client.as_ref().is_some_and(|c| !c.wants_mouse());
        let Some(drag) = &mut self.drag else { return };
        let col = m.column.clamp(area.x, area.right().saturating_sub(1)) - area.x;
        let row = m.row.clamp(area.y, area.bottom().saturating_sub(1)) - area.y;
        drag.left_row |= row != drag.press_row;
        let past = if !scrolls {
            None
        } else if m.row >= area.bottom() {
            Some((false, m.row - area.bottom() + 1))
        } else if m.row <= area.y && drag.left_row {
            Some((true, area.y - m.row + 1))
        } else {
            None
        };
        let now = Instant::now();
        drag.past = past.map(|(up, rows)| match drag.past {
            Some(p) if p.up == up => Past { rows, col, ..p },
            _ => Past { up, rows, col, since: now, last_step: None },
        });
        self.move_head(row, col);
        self.drag_scroll(now);
    }

    /// Put the selection's head on a cell of the view.
    fn move_head(&mut self, row: u16, col: u16) {
        let Some(client) = &self.client else { return };
        let top = client.history(|h| h.top());
        if let Some(sel) = &mut self.selection {
            sel.head = Point::new(top + usize::from(row), col);
        }
    }

    /// While a drag is held past the top or bottom edge, scroll the view a step when one is due and keep the head on
    /// the edge line, so the selection grows over the text coming into view. Called with every event and on every
    /// pass of the event loop: the terminal says nothing while the mouse stands still.
    pub fn drag_scroll(&mut self, now: Instant) {
        let Some(Drag { past: Some(past), .. }) = &mut self.drag else { return };
        let (up, col) = (past.up, past.col);
        let Some(lines) = past.step(now) else { return };
        let Some(client) = &self.client else { return };
        if client.wants_mouse() {
            return;
        }
        let lines = i32::try_from(lines).unwrap_or(i32::MAX);
        client.scroll(if up { lines } else { -lines });
        let edge = if up { 0 } else { self.term_area().height.saturating_sub(1) };
        self.move_head(edge, col);
    }

    /// Letting go copies what is selected, once: all of it, the lines scrolled out of view included. A press that
    /// never moved (a click) copies nothing, and neither does a selection of only blanks: both are dropped.
    fn finish_selection(&mut self) {
        if self.drag.take().is_none() {
            return;
        }
        let (Some(sel), Some(client)) = (self.selection, &self.client) else { return };
        let text = client.history(|h| sel.span(h).map(|span| extract(h, span)));
        match text {
            Some(text) if !text.is_empty() => {
                let outcome = self.clipboard.copy(&text);
                self.notify(outcome.notice());
            }
            _ => self.selection = None,
        }
    }

    /// Where copies go (`Clipboard::off` until given one: the real one, or a test's).
    pub fn set_clipboard(&mut self, clipboard: Clipboard) {
        self.clipboard = clipboard;
    }
}
