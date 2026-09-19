//! Keeps one conversation from being written by two Claude Code processes at once.
//!
//! Two live copies of a session each hold their own memory of it and both append to the same saved
//! history, so turns from one can be lost and the other's context goes stale. `platoon hook guard`
//! runs as a Claude Code `UserPromptSubmit` hook: the earliest live process for a session owns it,
//! and later copies have their prompts refused. A refused copy is marked stale for good, because
//! even after the owner exits its memory no longer matches the saved history.
//!
//! Every uncertain path allows the prompt: a guard that wrongly blocks is worse than none.

use crate::claude::{claude_root, pid_alive};
use crate::host;
use crate::usage::cache_dir;
use serde_json::Value;
use std::{collections::HashSet, fs, io::Read, path::{Path, PathBuf}, process::Command};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub pid: u32,
    pub id: String,
    pub started_at: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    /// Another live copy owns the session.
    Blocked { owner: u32, owner_in_platoon: bool },
    /// This copy was refused before, or diverged; it must not write again.
    Stale,
}

/// Live interactive processes and the session each is running.
pub fn live_entries(root: &Path) -> Vec<Entry> {
    let mut out = Vec::new();
    for e in fs::read_dir(root.join("sessions")).into_iter().flatten().flatten() {
        let Some(v) = fs::read_to_string(e.path()).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) else { continue };
        if v["kind"].as_str() != Some("interactive") {
            continue;
        }
        let (Some(pid), Some(id)) = (v["pid"].as_u64(), v["sessionId"].as_str()) else { continue };
        if pid_alive(pid as u32) {
            out.push(Entry { pid: pid as u32, id: id.to_string(), started_at: v["startedAt"].as_u64().unwrap_or(0) });
        }
    }
    out
}

/// `entries` are the live processes on one session. The earliest owns it; ties go to a platoon-hosted
/// copy, then the lower pid.
pub fn decide(entries: &[Entry], caller: u32, in_platoon: &dyn Fn(u32) -> bool, stale: bool) -> Verdict {
    if stale {
        return Verdict::Stale;
    }
    let Some(owner) = entries.iter().min_by_key(|e| (e.started_at, !in_platoon(e.pid), e.pid)) else { return Verdict::Allow };
    if owner.pid == caller {
        Verdict::Allow
    } else {
        Verdict::Blocked { owner: owner.pid, owner_in_platoon: in_platoon(owner.pid) }
    }
}

fn stale_dir(cache: &Path) -> PathBuf {
    cache.join("stale")
}

/// Mark a process as no longer safe to write. Keyed by pid and start time so a reused pid isn't affected.
pub fn mark_stale(cache: &Path, pid: u32, started_at: u64) {
    let dir = stale_dir(cache);
    if fs::create_dir_all(&dir).is_ok() {
        let _ = fs::write(dir.join(pid.to_string()), started_at.to_string());
    }
}

pub fn is_stale(cache: &Path, pid: u32, started_at: u64) -> bool {
    let path = stale_dir(cache).join(pid.to_string());
    match fs::read_to_string(&path) {
        Ok(s) if s.trim() == started_at.to_string() => true,
        Ok(_) => {
            let _ = fs::remove_file(&path); // a different process that reused the pid
            false
        }
        Err(_) => false,
    }
}

/// Forget markers for processes that have exited.
pub fn prune_stale(cache: &Path) {
    for e in fs::read_dir(stale_dir(cache)).into_iter().flatten().flatten() {
        if let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) {
            if !pid_alive(pid) {
                let _ = fs::remove_file(e.path());
            }
        }
    }
}

fn ppid(pid: u32) -> Option<u32> {
    if let Ok(s) = fs::read_to_string(format!("/proc/{pid}/stat")) {
        // "pid (comm) S ppid ...": comm may contain spaces or parens, so split after the last ')'.
        return s.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok();
    }
    let out = Command::new("ps").args(["-o", "ppid=", "-p", &pid.to_string()]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// `pid` and its ancestors, nearest first.
pub fn ancestors(pid: u32) -> Vec<u32> {
    let mut chain = vec![pid];
    let mut cur = pid;
    for _ in 0..32 {
        match ppid(cur) {
            Some(p) if p > 1 && !chain.contains(&p) => {
                chain.push(p);
                cur = p;
            }
            _ => break,
        }
    }
    chain
}

fn message(v: &Verdict, sid: &str) -> String {
    match v {
        Verdict::Blocked { owner, owner_in_platoon } => {
            let fork = format!("To work on a separate copy instead, run:\n  claude --resume {sid} --fork-session");
            if *owner_in_platoon {
                format!(
                    "This session is already open in platoon (pid {owner}). Two copies would overwrite each other's history, so this one is paused.\n\
                     Exit this copy (/exit). To see the live session in a terminal, run:\n  platoon open {}\n{fork}",
                    &sid[..sid.len().min(8)]
                )
            } else {
                format!(
                    "This session is already open in another terminal (pid {owner}). Two copies would overwrite each other's history, so this one is paused.\n\
                     Exit this copy (/exit), or close the other one first. {fork}"
                )
            }
        }
        Verdict::Stale => "This copy is out of sync with the saved conversation: it was opened while the session was running elsewhere. \
             Exit it (/exit) and resume the session again."
            .to_string(),
        Verdict::Allow => String::new(),
    }
}

/// `platoon hook guard`. Exit 0 allows the prompt; exit 2 refuses it and shows stderr to the user.
pub fn run_guard() -> i32 {
    std::panic::catch_unwind(guard_inner).unwrap_or(0)
}

fn guard_inner() -> i32 {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return 0;
    }
    let Some(sid) = serde_json::from_str::<Value>(&input).ok().and_then(|v| v["session_id"].as_str().map(String::from)) else { return 0 };
    let same: Vec<Entry> = live_entries(&claude_root()).into_iter().filter(|e| e.id == sid).collect();
    // Which registered process is the one running this hook? It's an ancestor of the hook.
    let chain = ancestors(std::process::id());
    let Some(me) = same.iter().find(|e| chain.contains(&e.pid)) else { return 0 };
    let hosted: HashSet<u32> = host::discover().into_iter().map(|h| h.claude_pid).collect();
    let cache = cache_dir();
    let verdict = decide(&same, me.pid, &|p| hosted.contains(&p), is_stale(&cache, me.pid, me.started_at));
    if verdict == Verdict::Allow {
        return 0;
    }
    mark_stale(&cache, me.pid, me.started_at);
    eprintln!("{}", message(&verdict, &sid));
    2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(pid: u32, started_at: u64) -> Entry {
        Entry { pid, id: "s".into(), started_at }
    }

    #[test]
    fn earliest_copy_owns_the_session() {
        let entries = [e(10, 100), e(20, 200)];
        let none = |_: u32| false;
        assert_eq!(decide(&entries, 10, &none, false), Verdict::Allow);
        assert_eq!(decide(&entries, 20, &none, false), Verdict::Blocked { owner: 10, owner_in_platoon: false });
    }

    #[test]
    fn a_lone_copy_is_always_allowed_and_ties_favour_platoon() {
        let none = |_: u32| false;
        assert_eq!(decide(&[e(10, 100)], 10, &none, false), Verdict::Allow);
        let tie = [e(10, 100), e(20, 100)];
        let hosted_20 = |p: u32| p == 20;
        assert_eq!(decide(&tie, 20, &hosted_20, false), Verdict::Allow);
        assert_eq!(decide(&tie, 10, &hosted_20, false), Verdict::Blocked { owner: 20, owner_in_platoon: true });
    }

    #[test]
    fn a_stale_copy_stays_blocked_even_when_alone() {
        assert_eq!(decide(&[e(10, 100)], 10, &|_| false, true), Verdict::Stale);
    }

    #[test]
    fn three_copies_only_the_first_may_write() {
        let entries = [e(30, 300), e(10, 100), e(20, 200)];
        let none = |_: u32| false;
        assert_eq!(decide(&entries, 10, &none, false), Verdict::Allow);
        assert!(matches!(decide(&entries, 20, &none, false), Verdict::Blocked { owner: 10, .. }));
        assert!(matches!(decide(&entries, 30, &none, false), Verdict::Blocked { owner: 10, .. }));
    }

    #[test]
    fn refusal_messages_say_what_to_do() {
        let in_platoon = message(&Verdict::Blocked { owner: 7, owner_in_platoon: true }, "abcdef12-3456");
        assert!(in_platoon.contains("open in platoon") && in_platoon.contains("platoon open abcdef12"), "{in_platoon}");
        assert!(in_platoon.contains("--fork-session"), "{in_platoon}");
        let elsewhere = message(&Verdict::Blocked { owner: 7, owner_in_platoon: false }, "abcdef12-3456");
        assert!(elsewhere.contains("another terminal") && !elsewhere.contains("platoon open"), "{elsewhere}");
    }

    #[test]
    fn ancestry_finds_the_parent_chain() {
        let chain = ancestors(std::process::id());
        assert_eq!(chain[0], std::process::id());
        assert!(chain.len() >= 2, "should at least see the parent: {chain:?}");
    }
}
