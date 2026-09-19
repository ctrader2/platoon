#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Busy,
    Idle,
}

pub struct Session {
    pub id: String,
    pub pid: u32,
    pub name: String,
    /// Home-abbreviated, for display.
    pub dir: String,
    /// Absolute working directory.
    pub cwd: String,
    pub branch: Option<String>,
    /// Short display name of the model in use, e.g. "Sonnet 5". None until the first reply.
    pub model: Option<String>,
    pub status: Status,
    /// Another live process is running this same conversation.
    pub shared: bool,
    /// This process must not write to the conversation any more: its memory can't be trusted to match it.
    pub stale: bool,
    /// Claude finished and the user hasn't looked at the output yet.
    pub unread: bool,
    /// Started outside platoon: listed and killable, but can't be jumped into.
    /// Everything is external until platoon hosts sessions itself.
    pub external: bool,
    pub context_pct: u8,
    /// True when the percentage comes from Claude Code itself, false when platoon estimated it.
    pub context_exact: bool,
    pub description: String,
    /// Start time in ms since the epoch, used for a stable ordering.
    pub started_at: u64,
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
