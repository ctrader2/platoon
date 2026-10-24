use crate::agents::Agent;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Busy,
    Idle,
    /// Claude's own service runs it (`claude --bg`): platoon shows it and can open it, but never stops, archives or
    /// otherwise touches it.
    Background,
    /// The process that ran it handed the conversation over to a background session that still runs (Claude Code
    /// "parks" it). Nothing works on this conversation now; opening it opens that background session.
    Moved,
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
    /// An interactive process that handed the conversation to a background session (`agents::Live::parked`). It is
    /// never working, never a copy that writes to the conversation, and platoon never offers to take it over.
    pub parked: bool,
    /// For a parked process whose background session is still running: that session's job id (what
    /// `claude attach` takes). Opening the row opens it.
    pub moved_to: Option<String>,
}

/// Where a row's name came from, in the order that wins: the user's own platoon name, a name typed with `/rename`
/// inside Claude, the name platoon generated, and the folder the conversation ran in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameSource {
    Folder,
    Generated,
    Claude,
    Platoon,
}

/// What the id of a row begins with when the row is a new Codex session platoon started that has no conversation yet:
/// Codex saves a new conversation, and so gives it its id, only with the first message, so until then the row stands
/// for the host's process (`unsaved_id`). Nothing is ever kept under such an id (archive, names, projects): the row's
/// actions that would keep something say why not, and once Codex saves the conversation its real row takes over.
pub const UNSAVED: &str = "unsaved:";

/// The row id of the new session platoon started as process `pid`, before it has a conversation (`UNSAVED`).
pub fn unsaved_id(pid: u32) -> String {
    format!("{UNSAVED}{pid}")
}

/// One conversation (a session id), whether or not anything is running it.
pub struct Session {
    pub id: String,
    /// The agent the conversation is with: what resumes it, and how its model and window read.
    pub agent: Agent,
    pub name: String,
    pub name_source: NameSource,
    /// The summary platoon generated for it, empty if there is none.
    pub summary: String,
    /// Home-abbreviated, for display.
    pub dir: String,
    /// Folders added beside `dir` (`/add-dir`), home-abbreviated like it. Shown after it: `~/repos/platoon, /`.
    pub extra_dirs: Vec<String>,
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
    /// Lowercased name, dir (and the added ones), branch, description and summary, so filtering never allocates per row.
    pub haystack: String,
}

impl Session {
    /// What the quick filter looks in: one lower-case text.
    pub fn make_haystack(name: &str, dir: &str, extra_dirs: &[String], branch: Option<&str>, description: &str, summary: &str) -> String {
        let dirs = std::iter::once(dir).chain(extra_dirs.iter().map(String::as_str)).collect::<Vec<_>>().join("\n");
        [name, &dirs, branch.unwrap_or(""), description, summary].join("\n").to_lowercase()
    }

    /// Is a Claude Code process running this conversation right now (in a platoon host or a terminal of its own)? A
    /// process that only waits after handing the conversation to a background session doesn't count.
    pub fn running(&self) -> bool {
        self.procs.iter().any(|p| !p.parked)
    }

    /// Is it open in a window of an agent that keeps one window per conversation itself (Codex), which platoon didn't
    /// start? Then platoon never opens it a second time, never stops it and never archives it: the user closes it there.
    pub fn held_elsewhere(&self) -> bool {
        self.agent.keeps_one_writer() && self.procs.iter().any(|p| !p.hosted)
    }

    /// Is this the row of a new session that has no conversation of its own yet (`UNSAVED`)?
    pub fn unsaved(&self) -> bool {
        self.id.starts_with(UNSAVED)
    }

    pub fn rehaystack(&mut self) {
        self.haystack =
            Self::make_haystack(&self.name, &self.dir, &self.extra_dirs, self.branch.as_deref(), &self.description, &self.summary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_finds_a_conversation_by_a_folder_added_to_it() {
        let extra = vec!["~/repos/Project-Docs".to_string(), "/srv/data".to_string()];
        let h = Session::make_haystack("Parser fix", "~/repos/platoon", &extra, Some("main"), "fix it", "");
        for q in ["platoon", "project-docs", "/srv/data", "parser"] {
            assert!(h.contains(q), "{q}");
        }
        assert!(!Session::make_haystack("x", "~/a", &[], None, "", "").contains("srv"));
    }
}

/// One rate-limit window as reported by Claude Code.
#[derive(Clone, Copy)]
pub struct Window {
    pub pct: u8,
    /// Unix epoch seconds when it resets; 0 when unknown (a 5-hour window that hasn't started, read as 0%).
    #[allow(dead_code)] // Will be shown as a countdown.
    pub resets_at: u64,
}

pub struct PlanUsage {
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
}
