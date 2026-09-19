#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Busy,
    Idle,
}

pub struct Session {
    pub name: String,
    pub dir: String,
    pub branch: Option<String>,
    pub status: Status,
    /// Claude finished and the user hasn't looked at the output yet.
    pub unread: bool,
    /// Started outside platoon: listed and killable, but can't be jumped into.
    pub external: bool,
    pub context_pct: u8,
    pub description: String,
}

pub struct PlanUsage {
    pub five_hour_pct: u8,
    pub seven_day_pct: u8,
}

pub fn mock_sessions() -> Vec<Session> {
    let s = |name: &str, dir: &str, branch: Option<&str>, status, unread, external, ctx, desc: &str| Session {
        name: name.into(),
        dir: dir.into(),
        branch: branch.map(Into::into),
        status,
        unread,
        external,
        context_pct: ctx,
        description: desc.into(),
    };
    vec![
        s("api-refactor", "~/repos/api", Some("feat/auth"), Status::Idle, true, false, 62, "Refactor the auth middleware to use the new session store and drop the legacy cookie path"),
        s("platoon", "~/repos/platoon", Some("main"), Status::Busy, false, false, 18, "Build a terminal app to manage Claude Code sessions across all directories"),
        s("billing-bug", "~/repos/billing", Some("fix/rounding"), Status::Idle, true, false, 91, "Invoices are off by one cent when tax is applied per line item"),
        s("dotfiles", "~/dotfiles", Some("main"), Status::Idle, false, false, 7, "Add a keybinding for the screenshot tool and sync it"),
        s("scratch", "~/tmp", None, Status::Busy, false, true, 35, "Write a script to dedupe photos by perceptual hash"),
        s("docs-site", "~/repos/docs", Some("main"), Status::Idle, false, true, 44, "Migrate the docs site from Jekyll to Astro"),
    ]
}

pub fn mock_usage() -> PlanUsage {
    PlanUsage { five_hour_pct: 47, seven_day_pct: 21 }
}
