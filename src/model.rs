#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Busy,
    Idle,
    /// Claude's own service runs it (`claude --bg`): platoon shows it and can open it, but never stops, archives or
    /// otherwise touches it.
    Background,
}

/// A live Claude Code process running a conversation. Several can share one conversation (the guard
/// exists to stop that); the list still shows one row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub started_at: u64,
    /// Runs under a platoon host, so platoon can attach to it. For a background session: a platoon host is
    /// showing it (running `claude attach`), so opening the row joins that host.
    pub hosted: bool,
    /// Set for a background session (run by Claude's own service, not by platoon or a terminal); holds the id
    /// `claude attach` takes. Such a process is only ever attached to: never signalled, never counted as a
    /// copy that could fall out of sync.
    pub background: Option<String>,
    /// This process must not write to the conversation any more: its memory can't be trusted to match it.
    pub stale: bool,
}

/// One conversation (a session id), whether or not anything is running it.
pub struct Session {
    pub id: String,
    pub name: String,
    /// Home-abbreviated, for display.
    pub dir: String,
    /// Absolute working directory the conversation was run in.
    pub cwd: String,
    pub branch: Option<String>,
    /// Short display name of the model in use, e.g. "Sonnet 5". None until the first reply.
    pub model: Option<String>,
    /// Busy when a live process is working; a conversation nothing is running is idle.
    pub status: Status,
    /// Claude finished and the user hasn't looked at the output yet.
    pub unread: bool,
    pub context_pct: u8,
    /// True when the percentage comes from Claude Code itself, false when platoon estimated it.
    pub context_exact: bool,
    /// Platoon's own estimate from the transcript, used whenever Claude hasn't reported an exact figure.
    pub context_est: u8,
    pub description: String,
    /// Latest activity in the transcript, ms since the epoch. Rows are ordered by it.
    pub last_active: u64,
    pub archived: bool,
    /// Live processes running this conversation right now, oldest first. Usually none or one.
    pub procs: Vec<Proc>,
    /// Lowercased name, dir, branch and description, so filtering never allocates per row.
    pub haystack: String,
}

/// One rate-limit window as reported by Claude Code.
#[derive(Clone, Copy)]
pub struct Window {
    pub pct: u8,
    /// Unix epoch seconds when it resets.
    #[allow(dead_code)] // Will be shown as a countdown.
    pub resets_at: u64,
}

pub struct PlanUsage {
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
}
