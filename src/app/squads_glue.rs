//! Squads inside the app: handing a ticket to a squad (`a`, a small picker), starting its leader in a platoon host
//! (with a worktree for each git folder), the split view of a ticket beside its live leader session, continuing a
//! leader that isn't running, letting a squad publish (`p`), stopping a cancelled ticket's leader, and keeping
//! leader sessions out of every list of the user's own sessions (`leader_ids`, the one place that decides it).
//! The squads themselves and everything that can be worked out without the app are in `squads.rs`.

use super::*;
use crate::squads::{self, Squad};
use crate::tickets::{Leader, State};
use std::collections::HashSet;
use std::time::SystemTime;

/// Below this many columns there is no room for a ticket beside its session: the session gets the width and the
/// ticket one line above it.
pub const SPLIT_MIN_WIDTH: u16 = 100;

/// What the split view is drawn in, for a terminal of `w` by `h` (the frame around the whole app included).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SplitAreas {
    /// The ticket beside the session, or (narrow) the one line above it.
    pub ticket: Rect,
    pub wide: bool,
    /// The column of the line between the two (wide only).
    pub rule_x: u16,
    /// The session's own screen.
    pub term: Rect,
    /// The status line under the session.
    pub bar: Rect,
}

pub fn split_areas(w: u16, h: u16) -> SplitAreas {
    let inner = Rect::new(1, 1, w.saturating_sub(2).max(1), h.saturating_sub(2).max(2));
    if w >= SPLIT_MIN_WIDTH {
        let left = inner.width * 2 / 5;
        let right = Rect::new(inner.x + left + 1, inner.y, inner.width.saturating_sub(left + 1).max(1), inner.height);
        SplitAreas {
            ticket: Rect::new(inner.x + 1, inner.y, left.saturating_sub(2).max(1), inner.height),
            wide: true,
            rule_x: inner.x + left,
            term: Rect::new(right.x, right.y, right.width, right.height.saturating_sub(1).max(1)),
            bar: Rect::new(right.x, right.bottom().saturating_sub(1), right.width, 1),
        }
    } else {
        SplitAreas {
            ticket: Rect::new(inner.x, inner.y, inner.width, 1),
            wide: false,
            rule_x: 0,
            term: Rect::new(inner.x, inner.y + 1, inner.width, inner.height.saturating_sub(2).max(1)),
            bar: Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1),
        }
    }
}

/// Everything about squads the app keeps.
pub struct SquadState {
    /// The ticket the squad picker is open for.
    pub pick_ticket: u32,
    /// The picked entry of `squads::SQUADS`.
    pub pick_selected: usize,
    /// The ticket whose leader the attached view is showing beside it (the split view).
    pub split: Option<u32>,
    /// Every leader session of every ticket: never shown with the user's own sessions.
    leaders: HashSet<String>,
    /// The tickets files `leaders` was read from, as they were then.
    stamps: Vec<(PathBuf, Option<(SystemTime, u64)>)>,
}

impl SquadState {
    pub fn new() -> Self {
        Self { pick_ticket: 0, pick_selected: 0, split: None, leaders: HashSet::new(), stamps: Vec::new() }
    }
}

impl App {
    /// The sessions that are squads' leaders.
    #[cfg(test)]
    pub fn leader_ids(&self) -> &HashSet<String> {
        &self.sq.leaders
    }

    /// Read the leaders again if any project's tickets file changed (one `stat` per file when nothing did). True when
    /// the set changed.
    pub(super) fn refresh_leaders(&mut self) -> bool {
        let stamps: Vec<(PathBuf, Option<(SystemTime, u64)>)> = crate::tickets::all_files(&self.config)
            .into_iter()
            .map(|(_, p)| {
                let st = fs_stamp(&p);
                (p, st)
            })
            .collect();
        if stamps == self.sq.stamps {
            return false;
        }
        self.sq.stamps = stamps;
        let ids = crate::tickets::leader_ids(&self.config);
        if ids == self.sq.leaders {
            return false;
        }
        self.sq.leaders = ids;
        true
    }

    /// The platoon host running this leader session, if one is: the one started with its id (as a new session, or
    /// resumed).
    pub fn leader_host(&self, session: &str) -> Option<&HostInfo> {
        self.hosts.iter().find(|h| h.args.iter().any(|a| a == session))
    }

    /// Ticket `number`'s current leader is running under platoon.
    pub fn leader_running(&self, number: u32) -> bool {
        self.ticket(number).and_then(|t| t.leader()).is_some_and(|l| self.leader_host(&l.session).is_some())
    }

    // ---- handing a ticket to a squad --------------------------------------------------------------------------

    /// `a` on a ticket: the squad picker, unless the ticket can't go to a squad now (said in one plain line).
    pub(super) fn ask_hand_to_squad(&mut self, number: u32) {
        let Some(t) = self.ticket(number) else { return };
        let why = if self.open_project().is_some_and(|p| p.archived) {
            Some(crate::tickets::archived_refusal(&self.open_project().map(|p| p.name.clone()).unwrap_or_default()))
        } else if t.state != State::New {
            Some(format!("#{number} is {}. Only a New ticket can be handed to a squad.", t.state.label()))
        } else if let Some(first) = crate::tickets::waiting_for(t, self.tickets()).first() {
            Some(format!("#{number} waits for #{first}, which isn't Done yet."))
        } else if t.folders.is_empty() {
            Some(format!("#{number} has no folder yet. Press f to give it one."))
        } else {
            t.folders.iter().find(|f| !Path::new(f).is_dir()).map(|f| format!("{} doesn't exist any more. Press f to change #{number}'s folders.", dirs::tilde(f, &self.home)))
        };
        if let Some(why) = why {
            self.notify(why);
            return;
        }
        self.sq.pick_ticket = number;
        self.sq.pick_selected = 0;
        self.mode = Mode::SquadPick;
    }

    pub(super) fn move_squad_pick(&mut self, delta: i32) {
        let last = squads::SQUADS.len().saturating_sub(1) as i32;
        self.sq.pick_selected = (self.sq.pick_selected as i32 + delta).clamp(0, last) as usize;
    }

    pub(super) fn squad_pick_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.act(Action::Cancel),
            KeyCode::Char('c') if ctrl => self.act(Action::Cancel),
            KeyCode::Enter => self.act(Action::Confirm),
            KeyCode::Up => self.move_squad_pick(-1),
            KeyCode::Down => self.move_squad_pick(1),
            _ => {}
        }
    }

    /// Enter in the squad picker: the ticket goes to that squad.
    pub(super) fn take_squad_pick(&mut self) {
        self.mode = Mode::List;
        let Some(squad) = squads::SQUADS.get(self.sq.pick_selected).copied() else { return };
        self.hand_to_squad(self.sq.pick_ticket, squad);
    }

    /// Make the ticket's worktrees, write its new leader on it (Working), and start that leader in a platoon host with
    /// the ticket as its first message; then show it beside the ticket. Anything that goes wrong on the way is said
    /// plainly and leaves the ticket as it was (worktrees already made stay, and are used again next time).
    fn hand_to_squad(&mut self, number: u32, squad: &'static Squad) {
        let Some(project) = self.open_project.clone() else { return };
        let Some(t) = self.ticket(number).cloned() else { return };
        let worktrees = match squads::prepare_worktrees(&self.git_program, &t.folders, number) {
            Ok(w) => w,
            Err(e) => {
                self.notify(format!("Couldn't hand #{number} to the {} squad: {e}", squad.name));
                return;
            }
        };
        let session = squads::new_session_id();
        let leader = Leader { squad: squad.id.to_string(), session: session.clone(), started: usage::now() };
        let Some(store) = self.tk.store.as_mut() else { return };
        if let Err(e) = store.start_leader(number, &leader, &worktrees) {
            self.report_squad_error(e);
            return;
        }
        // Written down before it starts, so the lists leave it out from its very first moment (and its hooks find it).
        if self.refresh_leaders() {
            self.built.0 = u64::MAX;
        }
        let Some(t) = self.ticket(number).cloned() else { return };
        let body = self.tk.store.as_ref().map(|s| s.body(&t)).unwrap_or_default();
        let project_name = self.projects.get(&project).map(|p| p.name.clone()).unwrap_or_default();
        let first = squads::first_message(&t, &project_name, &body);
        if self.launch_leader(number, Some(&first)) {
            self.notify(format!("#{number} is with the {} squad now. ctrl+space goes back to the project.", squad.name));
        } else if let Some(store) = self.tk.store.as_mut() {
            // Only a New ticket is ever handed to a squad: that is what it goes back to.
            let _ = store.forget_leader(number, &session, &State::New);
        }
    }

    fn report_squad_error(&mut self, e: io::Error) {
        let why = e.to_string();
        self.notify(format!("Couldn't save the ticket: {}", why.split(" (os error").next().unwrap_or(&why)));
    }

    /// Start ticket `number`'s current leader in a platoon host and show it in the split view: a new session with
    /// `first` as its first message, or (None) the same session resumed. The same squad flags either way, and never
    /// `PLATOON_PROJECT`: a leader belongs to its ticket, not to the project.
    fn launch_leader(&mut self, number: u32, first: Option<&str>) -> bool {
        let Some(t) = self.ticket(number).cloned() else { return false };
        let Some(leader) = t.leader().cloned() else { return false };
        let squad = squads::find(&leader.squad).unwrap_or(&squads::CODING);
        let folders = t.work_folders();
        let Some(dir) = folders.first().cloned() else {
            self.notify(format!("#{number} has no folder to work in."));
            return false;
        };
        if !Path::new(&dir).is_dir() {
            self.notify(format!("Can't start the squad: {} doesn't exist any more.", dirs::tilde(&dir, &self.home)));
            return false;
        }
        let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "platoon".into());
        let args = squads::leader_args(squad, &self.config, &exe, &leader.session, &squads::session_name(&t), &folders[1..], first);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.tk.open = Some(number);
        self.sq.split = Some(number);
        let (rows, cols) = self.pty_size();
        match host::launch(&self.program, &dir, &refs, &[], rows, cols) {
            Ok((info, child)) => {
                let pid = info.claude_pid;
                self.spawned.push(child);
                self.hosts.push(info);
                self.attach(pid);
                if !matches!(self.mode, Mode::Attached(_)) {
                    self.sq.split = None;
                }
                true
            }
            Err(e) => {
                self.sq.split = None;
                self.mode = Mode::List;
                self.notify(format!("Couldn't start the squad: {e}"));
                false
            }
        }
    }

    /// Enter on a ticket that has a leader: its session beside it, running or continued.
    pub(super) fn open_leader(&mut self, number: u32) {
        let Some(session) = self.ticket(number).and_then(|t| t.leader()).map(|l| l.session.clone()) else { return };
        self.tk.open = Some(number);
        match self.leader_host(&session).map(|h| h.claude_pid) {
            Some(pid) => {
                self.sq.split = Some(number);
                self.attach(pid);
                if !matches!(self.mode, Mode::Attached(_)) {
                    self.sq.split = None;
                }
            }
            None => {
                // Running somewhere platoon doesn't host (someone resumed it by hand): a second copy would fight it.
                if let Some(l) = self.reader.live().into_iter().find(|l| l.id == session) {
                    self.notify(format!("#{number}'s squad is running in another terminal (pid {}). Close it there first.", l.pid));
                    return;
                }
                self.launch_leader(number, None);
            }
        }
    }

    /// The split view is showing ticket `number` beside its session.
    pub fn split_showing(&self) -> Option<u32> {
        if matches!(self.mode, Mode::Attached(_)) { self.sq.split } else { None }
    }

    // ---- letting it publish -----------------------------------------------------------------------------------

    /// `p` on a ticket that is Ready to push/publish: the user's go-ahead is written on the ticket (Working again) and
    /// the leader is told to go on.
    pub(super) fn let_it_publish(&mut self, number: u32) {
        let Some(t) = self.ticket(number) else { return };
        if t.state != State::Ready {
            self.notify(format!("#{number} isn't waiting to publish anything."));
            return;
        }
        let session = t.leader().map(|l| l.session.clone()).unwrap_or_default();
        let Some(store) = self.tk.store.as_mut() else { return };
        if let Err(e) = store.give_go_ahead(number) {
            self.report_squad_error(e);
            return;
        }
        let Some(sock) = self.leader_host(&session).map(|h| h.sock.clone()) else {
            self.notify(format!("#{number} may publish now. Its squad's session isn't running: open it with enter and tell it to go ahead."));
            return;
        };
        let (rows, cols) = split_size(self.size);
        thread::spawn(move || type_into(&sock, rows, cols, "Go ahead and publish."));
        self.notify(format!("#{number} may publish now: platoon told its squad to go ahead."));
    }

    // ---- stopping a cancelled ticket's leader -----------------------------------------------------------------

    /// After the user made a ticket Cancelled: if its leader is running, ask whether to stop it too.
    pub(super) fn after_cancel(&mut self, number: u32) {
        let Some(session) = self.ticket(number).and_then(|t| t.leader()).map(|l| l.session.clone()) else { return };
        let Some(pid) = self.leader_host(&session).map(|h| h.claude_pid) else { return };
        let title = self.ticket(number).map(|t| t.title.clone()).unwrap_or_default();
        self.mode = Mode::Confirm(Confirm::StopSquad { number, title, pid });
    }

    /// Yes: stop only that leader's session (SIGTERM to the `claude` its host runs, on a thread of its own). Its
    /// conversation is kept, so choosing Working later continues it.
    pub(super) fn stop_squad(&mut self, number: u32, pid: u32) {
        self.mode = Mode::List;
        // The host of this ticket's leader, and nothing else.
        let Some(session) = self.ticket(number).and_then(|t| t.leader()).map(|l| l.session.clone()) else { return };
        if self.leader_host(&session).map(|h| h.claude_pid) != Some(pid) {
            return;
        }
        thread::spawn(move || {
            let _ = host::end_processes(&[pid]);
        });
        self.notify(format!("Stopping #{number}'s squad. Its conversation is kept."));
    }
}

fn fs_stamp(p: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// The size of the session's screen in the split view, for a terminal of this size.
pub fn split_size(size: (u16, u16)) -> (u16, u16) {
    let a = split_areas(size.0, size.1);
    (a.term.height, a.term.width)
}

/// Type `text` into a hosted session and press Enter, as a window of its own would: the words first, Enter a moment
/// later (sent together, Claude would take the Enter as part of a paste).
fn type_into(sock: &Path, rows: u16, cols: u16, text: &str) {
    use crate::host::{size_payload, write_frame, F_INPUT, F_RESIZE};
    let Ok(mut s) = std::os::unix::net::UnixStream::connect(sock) else { return };
    if write_frame(&mut s, F_RESIZE, &size_payload(rows, cols)).is_err() {
        return;
    }
    thread::sleep(Duration::from_millis(200));
    let _ = write_frame(&mut s, F_INPUT, text.as_bytes());
    thread::sleep(Duration::from_millis(400));
    let _ = write_frame(&mut s, F_INPUT, b"\r");
    thread::sleep(Duration::from_millis(200));
    let _ = s.shutdown(std::net::Shutdown::Both);
}

impl SquadState {
    /// Is this session a squad's leader?
    pub fn leaders_contains(&self, id: &str) -> bool {
        !self.leaders.is_empty() && self.leaders.contains(id)
    }
}
