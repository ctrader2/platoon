//! The first naming pass, done inside `platoon setup` right after the person says yes to the smart features (or
//! when they said yes earlier and nothing has been named yet): a local estimate first, then the ACTIVE conversations
//! are named there and then, newest first, with a one-line progress display. Ctrl+C skips the rest: platoon carries
//! on in the background while its window is open. Then archived conversations are offered, default "skip".
//!
//! It never blocks starting platoon and never changes what setup did or its exit code: it is only offered in a
//! terminal, and everything that goes wrong (a refusal, a failed call, an unreadable file) ends in a plain line of
//! text and "platoon will carry on later". The work itself is the same code the app's worker uses (`smart.rs`).

use crate::ai::{self, AiError, Request, Runner};
use crate::archive::Archive;
use crate::index::Scanner;
use crate::instructions;
use crate::ledger::Ledger;
use crate::prompts::{FilePrompts, Store};
use crate::settings::{self, Settings, thousands};
use crate::smart::{self, Conv, Digest, Misses, Rules, Tier};
use crate::summaries::{Entry, Summaries};
use crate::usage;
use std::{
    collections::HashSet,
    io::{BufRead, Write},
    path::Path,
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

pub struct ScanEnv<'a> {
    pub config: &'a Path,
    pub cache: &'a Path,
    /// Claude's own folder (`~/.claude`), where the transcripts are.
    pub root: &'a Path,
    /// What runs a call: `claude`, or a stand-in.
    pub program: String,
    /// Both stdin and stdout are a terminal.
    pub interactive: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ScanOutcome {
    /// The features are off, or something was named already: nothing to offer, nothing printed.
    NotNeeded,
    /// No terminal: one line says platoon will do it in the background.
    NotInteractive,
    /// Nothing to name.
    Nothing,
    /// Every active conversation is named.
    Done { named: usize },
    /// Ctrl+C: platoon will carry on.
    Skipped { named: usize },
    /// A refusal or a failed call ended it early.
    Stopped { named: usize },
}

/// A conversation as setup sees it.
struct Held {
    id: String,
    fp: Arc<FilePrompts>,
    archived: bool,
    last_active_ms: u64,
}

/// Every conversation with something in it, from the transcripts (the same reading the app does, done here to
/// completion).
fn load_world(env: &ScanEnv, settings: &Settings) -> Vec<Held> {
    let mut scanner = Scanner::load(env.root, env.cache);
    scanner.scan(&mut |_| true);
    scanner.save();
    let entries = scanner.entries();
    let mut store = Store::load(env.cache, settings.search_prompt_chars as usize);
    store.scan(&entries, settings.search_prompt_chars as usize, true, &mut |_| true);
    let archive = Archive::open(env.config);
    // A squad's leader sessions belong to their tickets: never named with the user's own conversations.
    let leaders = crate::tickets::leader_ids(env.config);
    let mut by_id: std::collections::HashMap<String, Held> = std::collections::HashMap::new();
    for fp in store.snapshot() {
        if leaders.contains(&fp.id) {
            continue;
        }
        let Some(e) = entries.iter().find(|e| e.path == fp.path) else { continue };
        let held = Held { id: fp.id.clone(), archived: archive.contains(&fp.id), last_active_ms: e.last_active_ms(), fp };
        // The same id under two project folders: the most recently active file wins.
        match by_id.get(&held.id) {
            Some(old) if old.last_active_ms >= held.last_active_ms => {}
            _ => {
                by_id.insert(held.id.clone(), held);
            }
        }
    }
    by_id.into_values().collect()
}

fn convs(held: &[Held]) -> Vec<Conv<'_>> {
    held.iter().map(|h| Conv { id: &h.id, fp: &h.fp, archived: h.archived, last_active_ms: h.last_active_ms, busy: false }).collect()
}

/// (conversations, estimate) of the pending work of one tier group, from what is on this computer.
fn estimate_of(held: &[Held], pending: &[(Tier, usize)], settings: &Settings, sums: &Summaries) -> smart::Estimate {
    let chars = pending.iter().map(|(_, i)| smart::digest_chars(&held[*i].fp, sums.get(&held[*i].id), settings)).collect::<Vec<_>>();
    smart::estimate(chars, settings.ai_batch_size as usize)
}

pub const NON_INTERACTIVE_LINE: &str = "Smart names haven't been made yet. platoon makes them in the background while it is open (run `platoon setup` in a terminal to make the first ones now).";

/// The offer. `interrupted` says whether Ctrl+C has been pressed (the caller arranges for that key to be seen: see
/// `ai::watch_interrupt`); it is polled while a call is out and between calls.
pub fn offer_scan(env: &ScanEnv, input: &mut dyn BufRead, out: &mut dyn Write, interrupted: &dyn Fn() -> bool) -> ScanOutcome {
    let (settings, corrupt) = settings::load(env.config);
    if corrupt || !settings.feature_on("names") {
        return ScanOutcome::NotNeeded;
    }
    let mut sums = Summaries::open(env.config);
    if !sums.is_empty() {
        return ScanOutcome::NotNeeded;
    }
    if !env.interactive {
        let _ = writeln!(out, "\n{NON_INTERACTIVE_LINE}");
        return ScanOutcome::NotInteractive;
    }
    let held = load_world(env, &settings);
    let all = convs(&held);
    let misses = Misses::default();
    let boosted = HashSet::new();
    let now = usage::now();
    let rules = Rules { settings: &settings, sums: &sums, misses: &misses, boosted: &boosted, now };
    let pending = smart::pending(&all, &rules, true);
    let active: Vec<(Tier, usize)> = pending.iter().copied().filter(|(t, _)| *t != Tier::Archived).collect();
    let archived: Vec<(Tier, usize)> = pending.iter().copied().filter(|(t, _)| *t == Tier::Archived).collect();
    let archived_estimate = estimate_of(&held, &archived, &settings, &sums);
    let (n_active, active_estimate) = (active.len(), estimate_of(&held, &active, &settings, &sums));
    drop(all);

    let mut outcome = ScanOutcome::Nothing;
    if n_active > 0 {
        let _ = writeln!(
            out,
            "\nSmart names: {n_active} active session{} to name, {} (an estimate, made on this computer).",
            if n_active == 1 { "" } else { "s" },
            active_estimate.plain()
        );
        let _ =
            writeln!(out, "Naming them now, newest first. Press Ctrl+C to skip: platoon carries on in the background while it is open.");
        ai::watch_interrupt();
        outcome = name_active(env, &held, &mut sums, &settings, n_active, out, interrupted);
        ai::stop_watching_interrupt();
    }

    // Archived conversations: a choice, and the default is to leave them.
    if archived_estimate.conversations > 0 && !settings.ai_archived {
        let _ = writeln!(
            out,
            "\nArchived: {} archived session{}, about {} tokens to name (an estimate).",
            archived_estimate.conversations,
            if archived_estimate.conversations == 1 { "" } else { "s" },
            thousands(archived_estimate.tokens)
        );
        let _ = write!(
            out,
            "Skip them, or name them slowly in the background while platoon is open (they may use at most {}% of what is left of the day's limit)? [S]kip / [b]ackground: ",
            settings.ai_archived_share
        );
        let _ = out.flush();
        let mut line = String::new();
        let answer = if input.read_line(&mut line).unwrap_or(0) == 0 { String::new() } else { line.trim().to_lowercase() };
        if matches!(answer.as_str(), "b" | "background") {
            let (mut current, corrupt) = settings::load(env.config);
            current.ai_archived = true;
            if !corrupt && settings::save(env.config, &current).is_ok() {
                let _ = writeln!(
                    out,
                    "Archived sessions will be named slowly in the background. Change it any time in the settings, on the Sessions tab."
                );
            } else {
                let _ = writeln!(out, "Couldn't save that choice, so archived sessions are skipped.");
            }
        } else {
            let _ = writeln!(out, "Skipped. You can change this any time in the settings, on the Sessions tab.");
        }
    }
    outcome
}

/// Name the active conversations, a batch at a time, with a one-line progress display.
fn name_active(
    env: &ScanEnv,
    held: &[Held],
    sums: &mut Summaries,
    settings: &Settings,
    total: usize,
    out: &mut dyn Write,
    interrupted: &dyn Fn() -> bool,
) -> ScanOutcome {
    let ledger = Ledger::open(env.cache);
    let used_before = ledger.today(usage::now()).tokens();
    let mut misses = Misses::default();
    let boosted = HashSet::new();
    let mut named = 0usize;
    let progress = |out: &mut dyn Write, named: usize| {
        let _ = write!(out, "\r  Naming sessions: {named} of {total}   ");
        let _ = out.flush();
    };
    progress(out, 0);
    let finish = |out: &mut dyn Write, outcome: ScanOutcome, note: &str| {
        let _ = writeln!(out);
        if !note.is_empty() {
            let _ = writeln!(out, "{note}");
        }
        let used = Ledger::open(env.cache).today(usage::now()).tokens().saturating_sub(used_before);
        let _ = writeln!(out, "Used about {} tokens.", thousands(used));
        outcome
    };
    loop {
        if interrupted() {
            return finish(out, ScanOutcome::Skipped { named }, "Skipped. platoon will name the rest in the background while it is open.");
        }
        let now = usage::now();
        let batch = {
            let all = convs(held);
            let rules = Rules { settings, sums, misses: &misses, boosted: &boosted, now };
            let pending = smart::pending(&all, &rules, false);
            smart::next_batch(&pending, settings.ai_batch_size as usize).map(|(idx, _)| {
                idx.into_iter()
                    .filter(|&i| all[i].fp.path.exists())
                    .map(|i| (i, smart::digest(all[i].fp, sums.get(all[i].id), settings)))
                    .collect::<Vec<(usize, Digest)>>()
            })
        };
        let Some(batch) = batch else { break };
        if batch.is_empty() {
            break;
        }
        if let Err(r) = ai::gate(settings, "names", env.cache, now) {
            return finish(
                out,
                ScanOutcome::Stopped { named },
                &format!("Stopping: {} platoon will carry on later while it is open.", r.plain()),
            );
        }
        let digests: Vec<Digest> = batch.iter().map(|(_, d)| d.clone()).collect();
        let mut req = Request::new(
            "names",
            smart::names_system_with(&instructions::guidance(env.config, instructions::NAMES)),
            smart::names_prompt(&digests),
        );
        let cancel = ai::Cancel::new();
        req.cancel = cancel.clone();
        let (tx, rx) = mpsc::channel();
        let (runner_settings, runner2) = (settings.clone(), Runner::with_program(env.cache, &env.program));
        thread::spawn(move || {
            let _ = tx.send(runner2.run(&runner_settings, &req));
        });
        let answer = loop {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(a) => break Some(a),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if interrupted() {
                        cancel.cancel();
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break None,
            }
        };
        let reply = match answer {
            Some(Ok(r)) => r,
            Some(Err(AiError::Cancelled)) => {
                return finish(
                    out,
                    ScanOutcome::Skipped { named },
                    "Skipped. platoon will name the rest in the background while it is open.",
                );
            }
            Some(Err(e)) => {
                return finish(
                    out,
                    ScanOutcome::Stopped { named },
                    &format!("Stopping: {} platoon will carry on later while it is open.", e.plain()),
                );
            }
            None => {
                return finish(
                    out,
                    ScanOutcome::Stopped { named },
                    "Stopping: the call ended without an answer. platoon will carry on later while it is open.",
                );
            }
        };
        let mut changes = Vec::new();
        let mut got = vec![false; batch.len()];
        for r in smart::parse_names(&reply.text, batch.len()) {
            let i = r.n - 1;
            let h = &held[batch[i].0];
            let Some(name) = r.name.or_else(|| sums.get(&h.id).map(|e| e.name.clone())) else { continue };
            got[i] = true;
            changes.push((h.id.clone(), Entry { name, summary: r.summary, covers: h.fp.substantive, model: reply.model.clone(), at: now }));
        }
        for (i, (idx, _)) in batch.iter().enumerate() {
            let h = &held[*idx];
            if got[i] {
                misses.clear(&h.id);
            } else {
                misses.miss(&h.id, h.fp.prompts.len());
            }
        }
        if let Err(e) = sums.set_many(&changes) {
            return finish(
                out,
                ScanOutcome::Stopped { named },
                &format!("Couldn't save the names ({e}). platoon will carry on later while it is open."),
            );
        }
        named += changes.len();
        progress(out, named);
    }
    finish(out, ScanOutcome::Done { named }, "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{AiFake, World};
    use std::fs;

    fn on(w: &World, change: impl FnOnce(&mut Settings)) {
        let mut s = settings::load(&w.config).0;
        s.tokens_enabled = true;
        s.tokens_asked = true;
        s.set_feature("names", true);
        change(&mut s);
        settings::save(&w.config, &s).unwrap();
    }

    fn fill(w: &World, active: usize, archived: usize) {
        for i in 1..=active {
            w.transcript(
                &format!("act{i}"),
                &format!("/work/act{i}"),
                &format!("please look into the active topic number {i} for me"),
                10 * i as u64,
            );
        }
        let mut a = Archive::open(&w.config);
        for i in 1..=archived {
            w.transcript(
                &format!("arc{i}"),
                &format!("/work/arc{i}"),
                &format!("please look into the archived topic number {i} for me"),
                5000 + i as u64,
            );
            a.archive(&format!("arc{i}"), usage::now()).unwrap();
        }
    }

    fn run(w: &World, fake: &AiFake, typed: &str, interactive: bool, interrupted: &dyn Fn() -> bool) -> (ScanOutcome, String) {
        let env = ScanEnv { config: &w.config, cache: &w.cache, root: &w.root, program: fake.script.clone(), interactive };
        let mut out = Vec::new();
        let outcome = offer_scan(&env, &mut std::io::Cursor::new(typed.as_bytes().to_vec()), &mut out, interrupted);
        (outcome, String::from_utf8(out).unwrap())
    }

    fn never() -> bool {
        false
    }

    fn summaries(w: &World) -> Summaries {
        Summaries::open(&w.config)
    }

    #[test]
    fn after_yes_the_estimate_is_shown_the_active_sessions_are_named_with_progress_and_archived_are_offered() {
        let w = World::new("sc-yes");
        fill(&w, 3, 2);
        on(&w, |_| {});
        let fake = AiFake::new("sc-yes");
        let (outcome, shown) = run(&w, &fake, "\n", true, &never);
        assert_eq!(outcome, ScanOutcome::Done { named: 3 });
        // The local estimate first, labelled as one, in plain words.
        assert!(
            shown.contains("Smart names: 3 active sessions to name, about ")
                && shown.contains("tokens, roughly 1 call (an estimate, made on this computer)."),
            "{shown}"
        );
        assert!(
            shown.contains("newest first") && shown.contains("Ctrl+C") && shown.contains("carries on in the background while it is open"),
            "{shown}"
        );
        // The one-line progress display: rewritten in place, ending at all of them.
        assert!(shown.contains("\r  Naming sessions: 0 of 3") && shown.contains("Naming sessions: 3 of 3"), "{shown}");
        assert!(shown.contains("Used about 450 tokens."), "{shown}");
        // One call for the three, the newest first.
        assert_eq!(fake.calls(), 1);
        let sent = fake.stdin(1);
        assert!(sent.find("active topic number 1 ").unwrap() < sent.find("active topic number 3 ").unwrap(), "{sent}");
        assert!(!sent.contains("archived topic"));
        assert_eq!(summaries(&w).len(), 3);
        // Archived: an estimate, and a default that leaves them alone.
        assert!(shown.contains("Archived: 2 archived sessions, about ") && shown.contains("tokens to name (an estimate)."), "{shown}");
        assert!(shown.contains("[S]kip / [b]ackground") && shown.contains("50% of what is left of the day's limit"), "{shown}");
        assert!(shown.contains("Skipped. You can change this any time in the settings, on the Sessions tab."), "{shown}");
        assert!(!settings::load(&w.config).0.ai_archived);
        let lower = shown.to_lowercase();
        for jargon in ["json", "stdin", "batch", "index", "api", "conversation"] {
            assert!(!lower.contains(jargon), "{jargon}: {shown}");
        }
    }

    #[test]
    fn the_first_pass_sends_the_users_own_guidance_for_naming() {
        let w = World::new("sc-own");
        fill(&w, 2, 0);
        on(&w, |_| {});
        fs::create_dir_all(w.config.join("prompts")).unwrap();
        fs::write(w.config.join("prompts/names.md"), "Name them after the file they are about.").unwrap();
        let fake = AiFake::new("sc-own");
        let (outcome, _) = run(&w, &fake, "\n", true, &never);
        assert_eq!(outcome, ScanOutcome::Done { named: 2 });
        assert!(
            fake.has_arg(1, &crate::instructions::compose("Name them after the file they are about.", smart::NAMES_FIXED)),
            "{:?}",
            fake.argv(1)
        );
    }

    #[test]
    fn the_estimate_is_the_local_formula_and_is_shown_before_anything_is_sent() {
        let w = World::new("sc-est");
        fill(&w, 61, 0);
        on(&w, |s| s.ai_batch_size = 30);
        let fake = AiFake::new("sc-est");
        fake.hang(true); // nothing gets an answer: we only look at what was said first
        let stop = std::sync::atomic::AtomicBool::new(false);
        let interrupted = || {
            let n = fake.calls() >= 1;
            if n {
                stop.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            n
        };
        let (_, shown) = run(&w, &fake, "", true, &interrupted);
        let first_line = shown.lines().find(|l| l.contains("active sessions to name")).unwrap();
        assert!(first_line.contains("61 active sessions") && first_line.contains("roughly 3 calls"), "{first_line}");
        // The same figure the function gives for those digests.
        let held = {
            let env = ScanEnv { config: &w.config, cache: &w.cache, root: &w.root, program: fake.script.clone(), interactive: true };
            load_world(&env, &settings::load(&w.config).0)
        };
        let s = settings::load(&w.config).0;
        let chars: Vec<usize> = held.iter().map(|h| smart::digest_chars(&h.fp, None, &s)).collect();
        let want = smart::estimate(chars, 30);
        assert!(first_line.contains(&format!("about {} tokens", thousands(want.tokens))), "{first_line} vs {want:?}");
    }

    #[test]
    fn ctrl_c_skips_the_rest_and_says_platoon_carries_on() {
        let w = World::new("sc-int");
        fill(&w, 4, 0);
        on(&w, |s| s.ai_batch_size = 1);
        let fake = AiFake::new("sc-int");
        // Pressed after the first call has been answered and its names saved (not while it is out).
        let (outcome, shown) = run(&w, &fake, "", true, &|| !summaries(&w).is_empty());
        assert_eq!(outcome, ScanOutcome::Skipped { named: 1 });
        assert_eq!((fake.calls(), summaries(&w).len()), (1, 1));
        assert!(shown.contains("Skipped. platoon will name the rest in the background while it is open."), "{shown}");
        assert!(!shown.contains("Archived:"), "no archived conversations, no question");
    }

    #[test]
    fn ctrl_c_during_a_call_cancels_it_and_kills_only_its_process() {
        let w = World::new("sc-int-call");
        fill(&w, 2, 0);
        on(&w, |_| {});
        let fake = AiFake::new("sc-int-call");
        fake.hang(true);
        let (outcome, shown) = run(&w, &fake, "", true, &|| fake.pids().len() == 2);
        assert_eq!(outcome, ScanOutcome::Skipped { named: 0 });
        assert!(shown.contains("Skipped."), "{shown}");
        let pids = fake.pids();
        for _ in 0..200 {
            if pids.iter().all(|&p| !crate::testkit::alive(p)) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(pids.iter().all(|&p| !crate::testkit::alive(p)), "gone: {pids:?}");
        assert!(!w.config.join("summaries.json").exists(), "the answer that never came is not kept");
    }

    #[test]
    fn choosing_background_for_archived_sessions_is_saved_and_nothing_archived_is_sent_now() {
        let w = World::new("sc-bg");
        fill(&w, 1, 3);
        on(&w, |s| s.idle_stop_minutes = 45);
        let fake = AiFake::new("sc-bg");
        let (outcome, shown) = run(&w, &fake, "b\n", true, &never);
        assert_eq!(outcome, ScanOutcome::Done { named: 1 });
        let s = settings::load(&w.config).0;
        assert!(s.ai_archived && s.tokens_enabled && s.idle_stop_minutes == 45, "the choice is saved and the rest is kept");
        assert!(shown.contains("Archived sessions will be named slowly in the background."), "{shown}");
        assert_eq!(fake.calls(), 1, "not now: while platoon is open, and within their share");
        assert!(!fake.stdin(1).contains("archived topic"));
        // "background" spelled out works; anything else is skip.
        for (typed, want) in [("background\n", true), ("BACKGROUND\n", true), ("skip\n", false), ("y\n", false), ("", false)] {
            let w = World::new("sc-bg2");
            fill(&w, 1, 1);
            on(&w, |_| {});
            let fake = AiFake::new("sc-bg2");
            run(&w, &fake, typed, true, &never);
            assert_eq!(settings::load(&w.config).0.ai_archived, want, "{typed:?}");
        }
    }

    #[test]
    fn without_a_terminal_nothing_is_scanned_and_one_line_says_platoon_will_do_it_in_the_background() {
        let w = World::new("sc-nontty");
        fill(&w, 2, 1);
        on(&w, |_| {});
        let fake = AiFake::new("sc-nontty");
        let (outcome, shown) = run(&w, &fake, "y\n", false, &never);
        assert_eq!(outcome, ScanOutcome::NotInteractive);
        assert_eq!(shown.trim().lines().count(), 1, "{shown}");
        assert!(shown.contains("in the background while it is open") && shown.contains("platoon setup"), "{shown}");
        assert_eq!(fake.calls(), 0);
        assert!(!w.config.join("summaries.json").exists());
    }

    #[test]
    fn it_is_offered_only_when_the_features_are_on_and_nothing_has_been_named_yet() {
        // Off: nothing at all.
        let w = World::new("sc-off");
        fill(&w, 2, 0);
        let fake = AiFake::new("sc-off");
        let (outcome, shown) = run(&w, &fake, "\n", true, &never);
        assert_eq!((outcome, shown.as_str(), fake.calls()), (ScanOutcome::NotNeeded, "", 0));
        // The master on but the names switch off: nothing.
        on(&w, |s| s.set_feature("names", false));
        assert_eq!(run(&w, &fake, "\n", true, &never).0, ScanOutcome::NotNeeded);
        // Said yes earlier and nothing named: it runs.
        on(&w, |_| {});
        assert_eq!(run(&w, &fake, "\n", true, &never).0, ScanOutcome::Done { named: 2 });
        // Named already: not again, and not a word.
        fake.hang(false);
        let (outcome, shown) = run(&w, &fake, "\n", true, &never);
        assert_eq!((outcome, shown.as_str(), fake.calls()), (ScanOutcome::NotNeeded, "", 1));
        // Nothing to name at all: nothing said.
        let w = World::new("sc-empty");
        on(&w, |_| {});
        let (outcome, shown) = run(&w, &AiFake::new("sc-empty"), "\n", true, &never);
        assert_eq!((outcome, shown.as_str()), (ScanOutcome::Nothing, ""));
    }

    #[test]
    fn a_refusal_or_a_failure_ends_it_in_a_plain_line_and_never_blocks_or_panics() {
        // The daily limit is used up.
        let w = World::new("sc-limit");
        fill(&w, 2, 0);
        on(&w, |s| s.ai_daily_tokens = 20_000);
        Ledger::open(&w.cache)
            .record_call(usage::now(), "names", &crate::ledger::Usage { input: 25_000, ..Default::default() }, 0.0, true)
            .unwrap();
        let fake = AiFake::new("sc-limit");
        let (outcome, shown) = run(&w, &fake, "\n", true, &never);
        assert_eq!(outcome, ScanOutcome::Stopped { named: 0 });
        assert!(
            shown.contains("Stopping: Today's token limit is used up") && shown.contains("platoon will carry on later while it is open."),
            "{shown}"
        );
        assert_eq!(fake.calls(), 0);
        // A failed call: the plain message, the back-off started, nothing kept.
        let w = World::new("sc-fail");
        fill(&w, 2, 0);
        on(&w, |_| {});
        let fake = AiFake::new("sc-fail");
        fake.fail(Some("Not logged in · Please run /login"));
        let (outcome, shown) = run(&w, &fake, "\n", true, &never);
        assert_eq!(outcome, ScanOutcome::Stopped { named: 0 });
        assert!(
            shown.contains("Stopping: Claude isn't signed in.") && !shown.contains("Not logged in"),
            "Claude's own words are not shown: {shown}"
        );
        assert!(Ledger::open(&w.cache).load().backoff.is_some());
        assert!(!w.config.join("summaries.json").exists());
        // A program that isn't there.
        let w = World::new("sc-missing");
        fill(&w, 1, 0);
        on(&w, |_| {});
        let env = ScanEnv {
            config: &w.config,
            cache: &w.cache,
            root: &w.root,
            program: "/nonexistent/claude-not-here".into(),
            interactive: true,
        };
        let mut out = Vec::new();
        let outcome = offer_scan(&env, &mut std::io::Cursor::new(Vec::new()), &mut out, &never);
        assert_eq!(outcome, ScanOutcome::Stopped { named: 0 });
        assert!(String::from_utf8(out).unwrap().contains("Stopping:"));
    }

    #[test]
    fn it_only_writes_where_platoon_keeps_its_own_things_and_never_into_claudes_files() {
        let w = World::new("sc-writes");
        fill(&w, 2, 1);
        on(&w, |_| {});
        let before: Vec<_> = fs::read_dir(w.root.join("projects/-some-project"))
            .unwrap()
            .flatten()
            .map(|e| (e.file_name(), fs::read(e.path()).unwrap()))
            .collect();
        let fake = AiFake::new("sc-writes");
        run(&w, &fake, "\n", true, &never);
        let after: Vec<_> = fs::read_dir(w.root.join("projects/-some-project"))
            .unwrap()
            .flatten()
            .map(|e| (e.file_name(), fs::read(e.path()).unwrap()))
            .collect();
        let sorted = |mut v: Vec<(std::ffi::OsString, Vec<u8>)>| {
            v.sort();
            v
        };
        assert_eq!(sorted(before), sorted(after), "Claude's transcripts are untouched");
        let mut in_config: Vec<String> =
            fs::read_dir(&w.config).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        in_config.sort();
        assert_eq!(in_config, ["archive.json", "settings.json", "summaries.json"]);
        let text = fs::read_to_string(w.config.join("summaries.json")).unwrap();
        assert!(!text.contains("please look into"), "no message text in it: {text}");
    }
}
