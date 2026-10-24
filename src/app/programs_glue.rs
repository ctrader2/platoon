//! Switching Claude Code or Codex off (Settings, General, Programs): "Use Claude Code" and "Use Codex", each On or Off,
//! or "Not installed", which can't be changed. A program switched off is left out exactly as if it weren't installed
//! (`Providers::switched`): its conversations leave the lists, its plan usage the header, its models every model list,
//! and `n` stops asking. The last program on can't be switched off: platoon says why. Nothing running is stopped: a
//! running task of a program switched off stays on the Tasks tab, marked (`task_switched_off`), and can be opened or
//! cancelled.

use super::*;
use crate::tickets::Ticket;

/// How a program's switch reads on the settings screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramState {
    On,
    Off,
    NotInstalled,
}

impl ProgramState {
    pub fn label(self) -> &'static str {
        match self {
            ProgramState::On => "On",
            ProgramState::Off => "Off",
            ProgramState::NotInstalled => "Not installed",
        }
    }
}

impl App {
    /// Make the programs in use follow the settings: what this computer has, less what is switched off. When that
    /// changes, the rows are built again (a program's conversations come or go) and the naming worker looks again.
    pub(super) fn sync_providers(&mut self) {
        let next = self.installed.clone().switched(&self.settings.get().switched_off());
        if next != self.providers {
            self.providers = next;
            self.built.0 = u64::MAX;
            self.smart.retry_at = 0;
        }
    }

    /// `agent`'s switch as the settings screen shows it.
    pub fn program_state(&self, agent: Agent) -> ProgramState {
        if self.installed.program(agent).is_none() {
            ProgramState::NotInstalled
        } else if self.providers.is_off(agent) {
            ProgramState::Off
        } else {
            ProgramState::On
        }
    }

    /// `←` `→` (Off, On), `enter` and a click (the other one) on a program's switch: saved at once. Not installed: nothing
    /// to switch, said. The last program on: refused, said.
    pub(super) fn step_program_setting(&mut self, agent: Agent, step: Step) {
        let now = self.program_state(agent);
        let want = match (now, step) {
            (ProgramState::NotInstalled, _) => {
                self.notify(format!("{} isn't installed on this computer, so there is nothing to switch.", agent.name()));
                return;
            }
            (_, Step::Prev) => ProgramState::Off,
            (_, Step::Next) => ProgramState::On,
            (ProgramState::On, Step::Cycle) => ProgramState::Off,
            (ProgramState::Off, Step::Cycle) => ProgramState::On,
        };
        if want == now {
            return;
        }
        if want == ProgramState::Off
            && let Some(why) = self.last_on(agent)
        {
            self.notify(why);
            return;
        }
        let mut next = self.settings.get();
        next.set_uses(agent, want == ProgramState::On);
        let saved = self.settings.update(&next);
        if saved.is_err() {
            self.report_save(saved);
            return;
        }
        self.sync_providers();
        self.sync_ai();
        self.reload();
        if want == ProgramState::Off {
            self.notify(format!(
                "{} is switched off: its sessions and models are left out. Anything of it already running keeps running.",
                agent.name()
            ));
        }
    }

    /// Why `agent` can't be switched off, when it is the last program on: platoon needs one. None when another is on.
    fn last_on(&self, agent: Agent) -> Option<String> {
        let other = Agent::ALL.into_iter().find(|&a| a != agent)?;
        let name = agent.name();
        match self.program_state(other) {
            ProgramState::On => None,
            ProgramState::Off => Some(format!(
                "{name} can't be switched off while {0} is off too: platoon needs one of them. Switch {0} on first.",
                other.name()
            )),
            ProgramState::NotInstalled => {
                Some(format!("{name} can't be switched off: {} isn't installed, and platoon needs one of them.", other.name()))
            }
        }
    }

    /// The program task `t` of `project` runs in when that program is switched off: its session is still found and
    /// can be opened (while it runs) or cancelled, but nothing new starts in it. None for a task not started, or one
    /// whose program is on.
    pub fn task_switched_off(&self, project: &str, t: &Ticket) -> Option<Agent> {
        if !t.started() || self.providers.off.is_empty() {
            return None;
        }
        let squad = self.task_squad(project, t).ok()?;
        let agent = self.installed.catalog().agent_of(&squad.as_run(&self.installed.catalog()).model)?;
        self.providers.is_off(agent).then_some(agent)
    }
}
