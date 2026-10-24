//! A new session (`n`): once the folder is picked (and with it the project of the screen it was asked on), which agent
//! it starts in. With only Claude Code or only Codex installed that one starts at once. With both, a small question
//! "Start in Claude Code or Codex?" comes up with the agent chosen last time already picked (so `enter` is the one
//! extra key when that is right); the choice is remembered in the settings (`Settings::new_session_agent`), written only
//! once the user has chosen. `esc` starts nothing.

use super::*;

/// The question of a new session's agent, while it is open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentPick {
    /// Where the session starts, and the project it goes in.
    pub dir: String,
    pub project: Option<String>,
    /// The agents to choose from, in order (every one installed).
    pub options: Vec<Agent>,
    /// The one picked, an index into `options`: the one chosen last time, in advance.
    pub selected: usize,
    /// The one chosen last time, when there is one (it is starred).
    pub last: Option<Agent>,
}

impl App {
    /// Start a new session in `dir` (in `project`): in the one agent installed, or, with both, after asking which.
    pub(super) fn new_session_in(&mut self, dir: String, project: Option<String>) {
        let options = self.providers.installed();
        let remembered = self.settings.get().new_session_agent;
        let last = remembered.as_deref().and_then(Agent::from_id).filter(|a| options.contains(a));
        let Some(pick) = self.providers.new_session_agent(remembered.as_deref()) else {
            self.mode = Mode::List;
            self.notify("Neither Claude Code nor Codex is installed on this computer, so no session can be started.");
            return;
        };
        if options.len() < 2 {
            self.launch_in(pick, dir, Start::New(None), project.as_deref(), true);
            return;
        }
        let selected = options.iter().position(|&a| a == pick).unwrap_or(0);
        self.agent_pick = Some(AgentPick { dir, project, options, selected, last });
        self.mode = Mode::PickAgent;
    }

    /// The question while it is open.
    pub fn agent_pick(&self) -> Option<&AgentPick> {
        self.agent_pick.as_ref()
    }

    pub(super) fn move_agent_pick(&mut self, delta: i32) {
        if let Some(p) = self.agent_pick.as_mut() {
            p.selected = (p.selected as i32 + delta).clamp(0, p.options.len().saturating_sub(1) as i32) as usize;
        }
    }

    /// A click on an agent picks it; on the one picked already, starts it.
    pub(super) fn click_agent_pick(&mut self, i: usize) {
        match self.agent_pick.as_mut() {
            Some(p) if p.selected == i => self.act(Action::Confirm),
            Some(p) => p.selected = i.min(p.options.len().saturating_sub(1)),
            None => {}
        }
    }

    /// `enter`: start the session in the agent picked, and remember it for next time. A save that fails is said and
    /// the session still starts.
    pub(super) fn take_agent_pick(&mut self) {
        self.mode = Mode::List;
        let Some(p) = self.agent_pick.take() else { return };
        let Some(&agent) = p.options.get(p.selected) else { return };
        if self.settings.get().new_session_agent.as_deref() != Some(agent.id()) {
            let mut next = self.settings.get();
            next.new_session_agent = Some(agent.id().to_string());
            let saved = self.settings.update(&next);
            if saved.is_err() {
                self.report_save(saved);
            }
        }
        self.launch_in(agent, p.dir, Start::New(None), p.project.as_deref(), true);
    }

    /// Is the selected row a new Codex session with no saved conversation yet (`Session::unsaved`)? Then it can't be
    /// `what` ("archived", "renamed", ...): said, and true.
    pub(super) fn refuse_unsaved(&mut self, what: &str) -> bool {
        let Some(name) = self.selected_session().filter(|s| s.unsaved).map(|s| s.name.clone()) else { return false };
        self.notify(format!("\"{name}\" has no conversation yet: Codex saves it when you send your first message. Then it can be {what}."));
        true
    }

    /// `esc`: nothing starts, nothing is remembered.
    pub(super) fn cancel_agent_pick(&mut self) {
        self.agent_pick = None;
        self.mode = Mode::List;
    }
}
