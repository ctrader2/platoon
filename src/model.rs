#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Busy,
    Idle,
}

pub struct Session {
    pub id: String,
    pub pid: u32,
    pub name: String,
    pub dir: String,
    pub branch: Option<String>,
    pub status: Status,
    /// Claude finished and the user hasn't looked at the output yet.
    pub unread: bool,
    /// Started outside platoon: listed and killable, but can't be jumped into.
    /// Everything is external until platoon hosts sessions itself.
    pub external: bool,
    pub context_pct: u8,
    pub description: String,
    /// Start time in ms since the epoch, used for a stable ordering.
    pub started_at: u64,
}

#[allow(dead_code)] // Source for these numbers is still an open question.
pub struct PlanUsage {
    pub five_hour_pct: u8,
    pub seven_day_pct: u8,
}
