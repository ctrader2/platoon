//! The app's side of the smart features: the background worker that names and summarizes conversations, and the
//! "Describe it" search. Both spend the user's Claude tokens, so both go through `ai::Runner` (which checks every
//! gate again just before a call starts) and both run a call on a thread of their own, one call at a time, while the
//! screen keeps going. What decides who is next and what is sent lives in `smart.rs`; this file only feeds it the
//! app's state, runs the call and takes the answer in.
//!
//! Indexing only happens while the platoon window is open: nothing here starts anything that outlives it.

use super::*;
use crate::ai::{AiError, Reply, Request, Runner};
use crate::instructions;
use crate::smart::{self, Digest, Estimate, FindItem, Misses, Rules};
use crate::summaries::{Entry as Summary, Summaries};
use std::collections::HashSet;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// After being refused or after an error the worker looks again this many seconds later (no busy loop, no repeated notices).
pub const IDLE_RECHECK_SECS: u64 = 30;
/// A description search that Claude has not answered after this long is given up on.
const FIND_TIMEOUT: Duration = Duration::from_secs(60);

type Answer = Result<Reply, AiError>;

/// One naming call in flight.
struct Flight {
    rx: Receiver<Answer>,
    /// The conversations in it, by the number they were sent under (1 is index 0).
    ids: Vec<String>,
    /// Their number of messages when they were sent: what a skipped one is not retried again until it changes from.
    totals: Vec<usize>,
    /// How many substantive messages each summary covers.
    covers: Vec<usize>,
    prev_names: Vec<Option<String>>,
    archived: bool,
}

/// One description search in flight.
struct FindFlight {
    rx: Receiver<Answer>,
    /// The candidates, by the number they were sent under.
    ids: Vec<String>,
}

/// A description search waiting for the naming call in flight to finish (one call at a time).
struct FindWaiting {
    req: Request,
    ids: Vec<String>,
}

type EstimateKey = (u64, u64, u64, u64, u64);

pub struct Engine {
    misses: Misses,
    /// Conversations that just became active again (taken out of the archive, or opened): done before the rest.
    boosted: HashSet<String>,
    /// The archived ids at the last look, so one that is no longer there was just restored.
    prev_archived: Option<HashSet<String>>,
    flight: Option<Flight>,
    find_flight: Option<FindFlight>,
    find_waiting: Option<FindWaiting>,
    /// Do nothing until the clock reaches this (seconds since the epoch).
    retry_at: u64,
    /// Why nothing is being done right now, in plain words, when that is a refusal or an error.
    idle: Option<String>,
    /// Conversations done in this run of work, and how many there are in all (done + still to do).
    done: usize,
    total: usize,
    /// The program that runs a call, when a test names a stand-in.
    program: Option<String>,
    /// Seconds added to the clock (tests cross midnight without waiting).
    offset: u64,
    estimate: RefCell<Option<(EstimateKey, Estimate)>>,
}

impl Engine {
    pub fn new() -> Self {
        Engine {
            misses: Misses::default(),
            boosted: HashSet::new(),
            prev_archived: None,
            flight: None,
            find_flight: None,
            find_waiting: None,
            retry_at: 0,
            idle: None,
            done: 0,
            total: 0,
            program: None,
            offset: 0,
            estimate: RefCell::new(None),
        }
    }
}

/// What the "Describe it" search is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FindState {
    /// Nothing asked yet for these words (or they changed since).
    Idle,
    /// Waiting for a naming call to finish before asking (one call at a time).
    Waiting,
    /// A call is out.
    Asking,
    /// About to ask again with more candidates: says what that costs and waits for enter.
    Confirm { estimate: Estimate, count: usize },
    /// Answered: the results are in the dialog (possibly none).
    Done,
    /// Refused or failed, in plain words.
    Failed(String),
}

pub struct FindData {
    pub state: FindState,
    /// The words that were asked (results belong to them).
    pub asked: String,
    /// What the state was before a confirmation, to go back to on esc.
    before_confirm: Option<FindState>,
}

impl FindData {
    pub fn new() -> Self {
        FindData { state: FindState::Idle, asked: String::new(), before_confirm: None }
    }
}

/// Index in `SEARCH_KINDS` of "Describe it".
pub const FIND_KIND: usize = 2;

fn folder_of(cwd: &str) -> String {
    Path::new(cwd).file_name().and_then(|n| n.to_str()).filter(|n| !n.is_empty()).unwrap_or(cwd).to_string()
}

impl App {
    // ---- clock, runner -------------------------------------------------------------------------------------

    /// Seconds since the epoch, as the smart features see it.
    pub(super) fn clock(&self) -> u64 {
        usage::now() + self.smart.offset
    }

    pub(super) fn ai_program(&self) -> String {
        self.smart.program.clone().unwrap_or_else(|| self.program.clone())
    }

    /// The runner every call of the app goes through: the stand-in `claude` in tests, and the test clock.
    pub(super) fn ai_runner(&self, program: &str) -> Runner {
        Runner::with_program(&self.cache, program).with_clock_offset(self.smart.offset)
    }

    /// Run `req` on a thread of its own. The answer comes back through the returned receiver.
    pub(super) fn spawn_call(&self, settings: Settings, req: Request) -> Receiver<Answer> {
        let (tx, rx) = channel();
        let program = self.ai_program();
        // A unit test that forgot to name a stand-in must never reach the real `claude`: the call is simply not made.
        if cfg!(test) && program == "claude" {
            let _ = tx.send(Err(AiError::Cancelled));
            return rx;
        }
        let runner = self.ai_runner(&program);
        thread::spawn(move || {
            let _ = tx.send(runner.run(&settings, &req));
        });
        rx
    }

    // ---- the naming worker ---------------------------------------------------------------------------------

    /// Called on every refresh, after the settings and the prompts were brought up to date. Never blocks.
    pub(super) fn drive_smart(&mut self) {
        let settings = self.settings.get();
        let now = self.clock();
        if self.summaries.reload_if_changed() {
            self.regenerate_rows();
        }
        self.watch_activation();
        self.poll_find(&settings);
        self.poll_names(now);
        if !settings.feature_on("names") {
            (self.smart.done, self.smart.total, self.smart.idle) = (0, 0, None);
        }
        if self.smart.flight.is_none()
            && self.smart.find_flight.is_none()
            && let Some(w) = self.smart.find_waiting.take()
        {
            self.launch_find(&settings, w.req, w.ids);
        }
        if settings.feature_on("names")
            && self.smart.flight.is_none()
            && self.smart.find_flight.is_none()
            && self.smart.find_waiting.is_none()
        {
            self.schedule_names(&settings, now);
        }
    }

    /// A conversation that is no longer archived was restored: it is treated as active and named at once.
    fn watch_activation(&mut self) {
        let now: HashSet<String> = self.sessions.iter().filter(|s| s.archived).map(|s| s.id.clone()).collect();
        if let Some(prev) = &self.smart.prev_archived {
            for id in prev.difference(&now) {
                if self.sessions.iter().any(|s| s.id == *id) {
                    self.smart.boosted.insert(id.clone());
                }
            }
        }
        self.smart.prev_archived = Some(now);
    }

    /// Everything the scheduler looks at: the conversations that have a transcript with something in it (of an agent
    /// whose conversations the smart features read: `Agent::smart_reads`).
    fn convs(&self) -> Vec<smart::Conv<'_>> {
        self.sessions
            .iter()
            .filter(|s| s.agent.smart_reads())
            .filter_map(|s| {
                let fp = self.prompts.get(&s.id)?;
                Some(smart::Conv { id: &s.id, fp, archived: s.archived, last_active_ms: s.last_active, busy: s.status == Status::Busy })
            })
            .collect()
    }

    fn schedule_names(&mut self, settings: &Settings, now: u64) {
        if self.prompts.catching_up() {
            return;
        }
        // Worked out first and owned, so the borrow of the app ends before anything is changed.
        struct Picked {
            ids: Vec<String>,
            digests: Vec<Digest>,
            totals: Vec<usize>,
            covers: Vec<usize>,
            prev_names: Vec<Option<String>>,
            archived: bool,
        }
        let (pending_active, pending_archived, picked) = {
            let convs = self.convs();
            let rules = Rules { settings, sums: &self.summaries, misses: &self.smart.misses, boosted: &self.smart.boosted, now };
            let pending = smart::pending(&convs, &rules, settings.ai_archived);
            let n_arch = pending.iter().filter(|(t, _)| *t == smart::Tier::Archived).count();
            let n_active = pending.len() - n_arch;
            let picked = smart::next_batch(&pending, settings.ai_batch_size as usize).map(|(idx, archived)| {
                let mut p = Picked { ids: vec![], digests: vec![], totals: vec![], covers: vec![], prev_names: vec![], archived };
                for i in idx {
                    let c = &convs[i];
                    if !c.fp.path.exists() {
                        continue; // its transcript is gone: nothing to read
                    }
                    let prev = self.summaries.get(c.id);
                    p.ids.push(c.id.to_string());
                    p.digests.push(smart::digest(c.fp, prev, settings));
                    p.totals.push(c.fp.prompts.len());
                    p.covers.push(c.fp.substantive);
                    p.prev_names.push(prev.map(|e| e.name.clone()));
                }
                p
            });
            (n_active, n_arch, picked)
        };
        // Progress: what has been done in this run and what is still to do. Archived work that isn't allowed to run
        // now (its share is used up) is not counted, so the indicator doesn't sit there for hours.
        let ledger = Ledger::open(&self.cache);
        let (used, archived_used) = (ledger.today(now).tokens(), ledger.archived_today(now));
        let mut counted = pending_active;
        let archived_open = settings.ai_archived
            && smart::archived_allowed(
                settings.ai_daily_tokens,
                used,
                archived_used,
                settings.ai_archived_share,
                smart::CALL_OVERHEAD_TOKENS,
            );
        if archived_open {
            counted += pending_archived;
        }
        if counted == 0 {
            (self.smart.done, self.smart.total) = (0, 0);
        } else {
            self.smart.total = self.smart.done + counted;
        }
        let Some(p) = picked.filter(|p| !p.ids.is_empty()) else { return };
        if now < self.smart.retry_at {
            return;
        }
        if let Err(r) = ai::gate(settings, "names", &self.cache, now) {
            self.smart.idle = Some(r.plain());
            self.smart.retry_at = now + IDLE_RECHECK_SECS;
            return;
        }
        let system = smart::names_system_with(&instructions::guidance(&self.config, instructions::NAMES));
        let prompt = smart::names_prompt(&p.digests);
        if p.archived {
            let est = smart::estimate([system.len() + prompt.len()], 1).tokens;
            if !smart::archived_allowed(settings.ai_daily_tokens, used, archived_used, settings.ai_archived_share, est) {
                self.smart.retry_at = now + IDLE_RECHECK_SECS;
                self.smart.idle = None;
                return;
            }
        }
        self.smart.idle = None;
        let mut req = Request::new("names", system, prompt);
        req.cancel = self.switchboard.token("names");
        let rx = self.spawn_call(settings.clone(), req);
        self.smart.flight =
            Some(Flight { rx, ids: p.ids, totals: p.totals, covers: p.covers, prev_names: p.prev_names, archived: p.archived });
    }

    fn poll_names(&mut self, now: u64) {
        let Some(fl) = self.smart.flight.take() else { return };
        let answer = match fl.rx.try_recv() {
            Ok(a) => a,
            Err(TryRecvError::Empty) => {
                self.smart.flight = Some(fl);
                return;
            }
            Err(TryRecvError::Disconnected) => {
                Err(AiError::Failed { kind: crate::ledger::FailKind::Other, detail: "the call ended without an answer".into() })
            }
        };
        match answer {
            Ok(reply) => self.take_names(fl, reply, now),
            // A switch was turned off while it ran: whatever came back is thrown away and nothing is kept against the
            // conversations, which are simply due again when it is switched back on.
            Err(AiError::Cancelled) => {}
            Err(e) => {
                self.smart.idle = Some(e.plain());
                self.smart.retry_at = now + IDLE_RECHECK_SECS;
            }
        }
    }

    /// Validate what came back, keep the good entries, remember the missing ones.
    fn take_names(&mut self, fl: Flight, reply: Reply, now: u64) {
        if fl.archived {
            let _ = Ledger::open(&self.cache).note_archived(now, reply.usage.total());
        }
        let named = smart::parse_names(&reply.text, fl.ids.len());
        let mut changes: Vec<(String, Summary)> = Vec::new();
        let mut got = vec![false; fl.ids.len()];
        for r in named {
            let i = r.n - 1;
            // "Same" needs a name to keep.
            let Some(name) = r.name.or_else(|| fl.prev_names[i].clone()) else { continue };
            got[i] = true;
            changes
                .push((fl.ids[i].clone(), Summary { name, summary: r.summary, covers: fl.covers[i], model: reply.model.clone(), at: now }));
        }
        for (i, id) in fl.ids.iter().enumerate() {
            if got[i] {
                self.smart.misses.clear(id);
                self.smart.boosted.remove(id);
            } else {
                self.smart.misses.miss(id, fl.totals[i]);
            }
        }
        if let Err(e) = self.summaries.set_many(&changes) {
            let why = e.to_string();
            self.notify(format!("Couldn't save the names: {}", why.split(" (os error").next().unwrap_or(&why)));
            self.smart.retry_at = now + IDLE_RECHECK_SECS;
            return;
        }
        self.smart.done += changes.len();
        let ids: Vec<String> = changes.into_iter().map(|(id, _)| id).collect();
        self.patch_rows(&ids);
    }

    /// Put the generated names and summaries of just these conversations on their rows.
    fn patch_rows(&mut self, ids: &[String]) {
        let sums = &self.summaries;
        for s in self.sessions.iter_mut().filter(|s| ids.contains(&s.id)) {
            apply_generated(s, sums);
        }
        self.rev += 1;
        self.update_view();
    }

    /// The names and summaries changed: put them on the rows (a name from the user or from Claude still wins).
    pub(super) fn regenerate_rows(&mut self) {
        let sums = &self.summaries;
        for s in &mut self.sessions {
            apply_generated(s, sums);
        }
        self.rev += 1;
        self.update_view();
    }

    /// How far the naming has got and how much there is, while there is some. The flag says it is waiting rather than
    /// working. The screen no longer shows it (the count was noise); the tests read it.
    #[cfg(test)]
    pub fn smart_progress(&self) -> Option<(usize, usize, bool)> {
        (self.smart.total > 0 && self.settings.get().feature_on("names")).then_some((
            self.smart.done,
            self.smart.total,
            self.smart.flight.is_none(),
        ))
    }

    /// Why the naming work is waiting, in plain words, when a gate or a failure is the reason.
    pub fn smart_idle(&self) -> Option<&str> {
        self.smart.idle.as_deref().filter(|_| self.settings.get().feature_on("names"))
    }

    /// True while a call is out or one is waiting to go, so the screen should be refreshed quickly.
    pub fn smart_busy(&self) -> bool {
        self.smart.flight.is_some() || self.smart.find_flight.is_some() || self.smart.find_waiting.is_some()
    }

    /// Archived conversations that still have no name, and what naming them would roughly cost (an estimate made
    /// on this computer, from what is in the prompt index).
    pub fn archived_estimate(&self) -> Estimate {
        let s = self.settings.get();
        let key = (self.prompts.generation, self.rev, s.ai_prompts_per_session, s.ai_prompt_chars, s.ai_batch_size);
        if let Some((k, e)) = &*self.smart.estimate.borrow()
            && *k == key
        {
            return *e;
        }
        let chars = self
            .sessions
            .iter()
            .filter(|c| c.archived && self.summaries.get(&c.id).is_none())
            .filter_map(|c| self.prompts.get(&c.id))
            .filter(|fp| fp.substantive > 0)
            .map(|fp| smart::digest_chars(fp, None, &s))
            .collect::<Vec<_>>();
        let e = smart::estimate(chars, s.ai_batch_size as usize);
        *self.smart.estimate.borrow_mut() = Some((key, e));
        e
    }

    /// The user opened a conversation: it counts as active, so it is named at once if it has no name yet.
    pub(super) fn boost(&mut self, id: &str) {
        self.smart.boosted.insert(id.to_string());
    }

    // ---- "Describe it" --------------------------------------------------------------------------------------

    /// The kinds of search on offer: "Describe it" only while the master switch and its own switch are on.
    pub fn search_kinds(&self) -> Vec<usize> {
        // The Projects tab's search narrows its list of projects: the other kinds are about sessions.
        if self.projects_list_showing() {
            return vec![0];
        }
        let s = self.settings.get();
        (0..SEARCH_KINDS.len()).filter(|&i| SEARCH_KINDS[i].feature.is_none_or(|f| s.feature_on(f))).collect()
    }

    pub fn find_state(&self) -> &FindState {
        &self.search.find.state
    }

    /// Typing is held while a description search is out: what was typed is what was asked.
    pub(super) fn find_locked(&self) -> bool {
        self.search.kind == FIND_KIND
            && matches!(self.search.find.state, FindState::Waiting | FindState::Asking | FindState::Confirm { .. })
    }

    /// The words changed in the description search: the answer on screen was for other words.
    pub(super) fn find_text_changed(&mut self) {
        if matches!(self.search.find.state, FindState::Waiting | FindState::Asking | FindState::Confirm { .. }) {
            return;
        }
        self.search.find.state = FindState::Idle;
        self.search.results.clear();
        self.search.total = 0;
        self.search.selected = 0;
    }

    /// Stop whatever the description search has out: the call is cancelled (its process killed, its answer thrown
    /// away) or its wait dropped.
    pub(super) fn cancel_find(&mut self) {
        if self.smart.find_flight.take().is_some() {
            self.switchboard.cancel("find");
        }
        self.smart.find_waiting = None;
        self.search.find.before_confirm = None;
        if matches!(self.search.find.state, FindState::Waiting | FindState::Asking | FindState::Confirm { .. }) {
            self.search.find.state = FindState::Idle;
        }
    }

    /// Enter in "Describe it".
    pub(super) fn confirm_find(&mut self) {
        match self.search.find.state.clone() {
            FindState::Waiting | FindState::Asking => {}
            FindState::Confirm { .. } => self.start_find(true),
            FindState::Done if self.search.text == self.search.find.asked => {
                if let Some(id) = self.search.results.get(self.search.selected).map(|r| r.id.clone()) {
                    self.show_conversation(&id);
                }
            }
            _ => self.start_find(false),
        }
    }

    /// Esc in "Describe it". True when it was used up (a call cancelled, a question dismissed); false when the
    /// dialog should close as usual.
    pub(super) fn cancel_find_step(&mut self) -> bool {
        match self.search.find.state.clone() {
            FindState::Waiting | FindState::Asking => {
                self.cancel_find();
                true
            }
            FindState::Confirm { .. } => {
                self.search.find.state = self.search.find.before_confirm.take().unwrap_or(FindState::Idle);
                true
            }
            _ => false,
        }
    }

    /// The conversations a description search may pick from, the local way: by index into `sessions`.
    fn find_pool(&self, settings: &Settings) -> Vec<usize> {
        (0..self.sessions.len())
            .filter(|&i| {
                let s = &self.sessions[i];
                !(s.archived && !settings.search_archived) && self.in_scope(s) && s.agent.smart_reads()
            })
            .collect()
    }

    /// Ctrl+W: search wider. Says what that costs first and waits for enter.
    pub(super) fn ask_wider(&mut self) {
        if self.search.kind != FIND_KIND
            || self.search.text.trim().is_empty()
            || matches!(self.search.find.state, FindState::Waiting | FindState::Asking | FindState::Confirm { .. })
        {
            return;
        }
        let settings = self.settings.get();
        let (_, texts) = self.find_texts(&settings, true);
        let system = smart::find_system_with(&instructions::guidance(&self.config, instructions::FIND), settings.ai_find_results as usize);
        let chars = texts.iter().map(String::len).sum::<usize>() + system.len() + self.search.text.len() + 60;
        let estimate = smart::estimate([chars], 1);
        self.search.find.before_confirm = Some(self.search.find.state.clone());
        self.search.find.state = FindState::Confirm { estimate: Estimate { conversations: texts.len(), ..estimate }, count: texts.len() };
    }

    /// The candidates for the words typed and how each is written in the request.
    fn find_texts(&self, settings: &Settings, wide: bool) -> (Vec<String>, Vec<String>) {
        let pool = self.find_pool(settings);
        let folders: Vec<String> = pool.iter().map(|&i| folder_of(&self.sessions[i].cwd)).collect();
        let items: Vec<FindItem> = pool
            .iter()
            .zip(&folders)
            .map(|(&i, folder)| {
                let s = &self.sessions[i];
                let summary = self.summaries.get(&s.id).map(|e| e.summary.as_str()).filter(|t| !t.is_empty());
                FindItem { name: &s.name, folder, summary, fp: self.prompts.get(&s.id).map(|a| &**a), last_active_ms: s.last_active }
            })
            .collect();
        let limit = if wide { settings.ai_find_wide_candidates } else { settings.ai_find_candidates } as usize;
        let chosen = smart::find_candidates(&self.search.text, &items, limit);
        let ids = chosen.iter().map(|&c| self.sessions[pool[c]].id.clone()).collect();
        let texts = chosen.iter().enumerate().map(|(n, &c)| smart::candidate_text(n + 1, &items[c])).collect();
        (ids, texts)
    }

    /// Ask Claude which of the candidates the words describe. Only ever from a key press (never per keystroke).
    fn start_find(&mut self, wide: bool) {
        let query = self.search.text.trim().to_string();
        if query.is_empty() {
            return;
        }
        let settings = self.settings.get();
        let now = self.clock();
        self.search.results.clear();
        self.search.total = 0;
        self.search.selected = 0;
        self.search.find.asked = self.search.text.clone();
        self.search.find.before_confirm = None;
        // The user asked for this themselves: the plan pause doesn't apply, everything else does.
        if let Err(r) = ai::gate_for(&settings, "find", &self.cache, now, true) {
            let mut text = r.plain();
            if r.is_switch() {
                text.push(' ');
                text.push_str("Turn it on in the settings (press s, then the Smart features tab).");
            }
            self.search.find.state = FindState::Failed(text);
            return;
        }
        let (ids, texts) = self.find_texts(&settings, wide);
        if ids.is_empty() {
            self.search.find.state = FindState::Failed("There are no sessions to search yet.".into());
            return;
        }
        let mut req = Request::new(
            "find",
            smart::find_system_with(&instructions::guidance(&self.config, instructions::FIND), settings.ai_find_results as usize),
            smart::find_prompt(&query, &texts),
        );
        req.user_initiated = true;
        req.timeout = FIND_TIMEOUT;
        req.cancel = self.switchboard.token("find");
        if self.smart.flight.is_some() {
            self.smart.find_waiting = Some(FindWaiting { req, ids });
            self.search.find.state = FindState::Waiting;
        } else {
            self.launch_find(&settings, req, ids);
        }
    }

    fn launch_find(&mut self, settings: &Settings, req: Request, ids: Vec<String>) {
        let rx = self.spawn_call(settings.clone(), req);
        self.smart.find_flight = Some(FindFlight { rx, ids });
        self.search.find.state = FindState::Asking;
    }

    fn poll_find(&mut self, settings: &Settings) {
        let Some(fl) = self.smart.find_flight.take() else { return };
        let answer = match fl.rx.try_recv() {
            Ok(a) => a,
            Err(TryRecvError::Empty) => {
                self.smart.find_flight = Some(fl);
                return;
            }
            Err(TryRecvError::Disconnected) => {
                Err(AiError::Failed { kind: crate::ledger::FailKind::Other, detail: "the call ended without an answer".into() })
            }
        };
        // The dialog was closed or the words changed while it ran: nobody is waiting for the answer any more.
        if self.search.find.state != FindState::Asking {
            return;
        }
        match answer {
            Ok(reply) => {
                let ranked = smart::parse_ranking(&reply.text, fl.ids.len(), settings.ai_find_results as usize);
                let results: Vec<SearchResult> = ranked
                    .iter()
                    .filter_map(|r| {
                        let s = self.sessions.iter().find(|s| s.id == fl.ids[r.n - 1])?;
                        let detail = if !r.reason.is_empty() { r.reason.clone() } else { s.summary.clone() };
                        Some(SearchResult {
                            id: s.id.clone(),
                            name: s.name.clone(),
                            dir: s.dir.clone(),
                            extra_dirs: s.extra_dirs.clone(),
                            archived: s.archived,
                            last_active: s.last_active,
                            detail,
                            marks: Vec::new(),
                        })
                    })
                    .collect();
                self.search.total = results.len();
                self.search.results = results;
                self.search.selected = 0;
                self.search.find.state = FindState::Done;
            }
            Err(AiError::Cancelled) => self.search.find.state = FindState::Idle,
            Err(e) => self.search.find.state = FindState::Failed(e.plain()),
        }
    }

    // ---- for tests ---------------------------------------------------------------------------------------------

    /// Run calls through this program (a fake `claude`) instead of the real one.
    #[cfg(test)]
    pub fn set_ai_program_for_test(&mut self, program: &str) {
        self.smart.program = Some(program.to_string());
    }

    /// Move the smart features' clock forward.
    #[cfg(test)]
    pub fn advance_clock_for_test(&mut self, secs: u64) {
        self.smart.offset += secs;
    }

    /// Refresh until no call is out and no work is due (or `secs` seconds pass).
    #[cfg(test)]
    pub fn settle_ai_for_test(&mut self, secs: u64) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(secs) {
            self.refresh_with(self.hosts.clone(), self.guard_on);
            if !self.smart_busy() {
                // One more turn: finishing a call may make the next batch due.
                self.refresh_with(self.hosts.clone(), self.guard_on);
                if !self.smart_busy() {
                    return;
                }
            }
            thread::sleep(Duration::from_millis(15));
        }
        panic!("the smart features were still busy after {secs} seconds");
    }

    /// Refresh until `cond` holds (or panic after `secs` seconds).
    #[cfg(test)]
    pub fn refresh_until_for_test(&mut self, secs: u64, what: &str, cond: impl Fn(&App) -> bool) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(secs) {
            self.refresh_with(self.hosts.clone(), self.guard_on);
            if cond(self) {
                return;
            }
            thread::sleep(Duration::from_millis(15));
        }
        panic!("timed out waiting for: {what}");
    }

    #[cfg(test)]
    pub fn misses_for_test(&self, id: &str) -> u32 {
        self.smart.misses.count(id)
    }
}

/// Put the generated name and summary on a row, unless the user's own name or one typed inside Claude is there.
fn apply_generated(s: &mut Session, sums: &Summaries) {
    let g = sums.get(&s.id);
    s.summary = g.map(|e| e.summary.clone()).unwrap_or_default();
    match (s.name_source, g) {
        (NameSource::Folder | NameSource::Generated, Some(e)) => {
            s.name = e.name.clone();
            s.name_source = NameSource::Generated;
        }
        (NameSource::Generated, None) => {
            s.name = folder_of(&s.cwd);
            s.name_source = NameSource::Folder;
        }
        _ => {}
    }
    s.rehaystack();
}
