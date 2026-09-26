//! Keeps one conversation from being written by two Claude Code processes at once.
//!
//! Two live copies of a session each hold their own memory of it and both append to the same saved
//! history, so turns from one can be lost and the other's context goes stale. `platoon hook guard`
//! runs as a Claude Code `UserPromptSubmit` hook: the earliest live process for a session owns it,
//! and later copies have their prompts refused. A refused copy is marked stale for good, because
//! even after the owner exits its memory no longer matches the saved history.
//!
//! Every uncertain path allows the prompt: a guard that wrongly blocks is worse than none.
//!
//! The same hook also **carries a project's notes into the conversation** when they have changed since this
//! session last got them. The `SessionStart` hook (`sessionhook.rs`) hands them over at the start of a session and
//! after a compaction; this one keeps them up to date while the session runs, because notes that change an hour in
//! would otherwise never reach it. Both use the one hash store (`<cache>/notes-sent/<session id>`), so the same
//! version is never sent twice. It is context in the conversation, not a system prompt.
//!
//! Nothing about the guard itself changes for it: the notes are only ever looked at on a prompt that is allowed,
//! every step is a small read that swallows its own errors, and a prompt is never refused or delayed because of
//! them.

use crate::claude::{claude_root, pid_alive};
use crate::host;
use crate::procs::ppid;
use crate::sessionhook;
use crate::usage::cache_dir;
use serde_json::{json, Value};
use std::{collections::HashSet, fs, io::Read, path::{Path, PathBuf}};

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

/// The project notes to add to this prompt, or None. None is the answer to everything: a session in no project, an
/// archived session or project, notes that are missing or empty, a version this session already has, and the
/// runner's own `claude -p` calls (which work in `<cache>/ai-work` with `PLATOON_AI` set, so the helper can never
/// feed itself its own notes). What it returns is recorded as sent, so neither hook sends this version again.
pub fn notes_context(input: &Value, config: &Path, cache: &Path, ai_child: bool) -> Option<String> {
    if ai_child {
        return None;
    }
    let session = input.get("session_id")?.as_str()?;
    if !sessionhook::safe_name(session) {
        return None;
    }
    let cwd = input.get("cwd").and_then(Value::as_str).unwrap_or("");
    if !cwd.is_empty() && Path::new(cwd) == crate::ai::work_dir(cache) {
        return None;
    }
    let projects = crate::projects::Projects::open(config);
    // A squad's leader is in no project, but its ticket is: it gets that project's notes too.
    let leader = if projects.of(session).is_none() { crate::tickets::find_leader(config, session).and_then(|(p, _, _)| projects.get(&p).cloned()) } else { None };
    let project = match &leader {
        Some(p) => p,
        None => projects.of(session)?,
    };
    if project.archived {
        return None;
    }
    if crate::archive::Archive::open(config).contains(session) {
        return None;
    }
    let notes = fs::read_to_string(projects.notes_path(&project.id)).ok()?;
    if notes.trim().is_empty() {
        return None;
    }
    let hash = sessionhook::notes_hash(&project.id, &notes);
    if sessionhook::sent_hash(cache, session).as_deref() == Some(hash.as_str()) {
        return None;
    }
    sessionhook::mark_sent(cache, session, &hash);
    Some(if leader.is_some() { sessionhook::ticket_context_text(&project.name, &notes) } else { sessionhook::context_text(&project.name, &notes) })
}

/// What the hook prints when it lets a prompt through and the notes have changed: context for the conversation.
fn notes_output(context: &str) -> String {
    json!({"hookSpecificOutput": {"hookEventName": "UserPromptSubmit", "additionalContext": context}}).to_string()
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
    let Some(v) = serde_json::from_str::<Value>(&input).ok() else { return 0 };
    let Some(sid) = v["session_id"].as_str().map(String::from) else { return 0 };
    let cache = cache_dir();
    // The guard's own answer first, exactly as before: nothing below may change it.
    let same: Vec<Entry> = live_entries(&claude_root()).into_iter().filter(|e| e.id == sid).collect();
    // Which registered process is the one running this hook? It's an ancestor of the hook.
    let chain = ancestors(std::process::id());
    let me = same.iter().find(|e| chain.contains(&e.pid));
    let verdict = match me {
        // A process the registry doesn't know, or a hook that can't tell who called it: allowed, as before.
        None => Verdict::Allow,
        Some(me) => {
            let hosted: HashSet<u32> = host::discover().into_iter().map(|h| h.claude_pid).collect();
            decide(&same, me.pid, &|p| hosted.contains(&p), is_stale(&cache, me.pid, me.started_at))
        }
    };
    if verdict != Verdict::Allow {
        if let Some(me) = me {
            mark_stale(&cache, me.pid, me.started_at);
        }
        eprintln!("{}", message(&verdict, &sid));
        return 2;
    }
    // Allowed. Anything that goes wrong from here on is simply no notes; the prompt goes through either way.
    let _ = std::panic::catch_unwind(|| {
        let ai_child = std::env::var_os("PLATOON_AI").is_some();
        if let Some(context) = notes_context(&v, &crate::archive::config_dir(), &cache, ai_child) {
            println!("{}", notes_output(&context));
        }
    });
    0
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
    fn the_guard_only_sees_interactive_processes_never_a_background_session() {
        // Deliberate: Claude's own service owns a background session and refuses to resume it elsewhere, so it can
        // never be a second copy; counting it here could refuse the wrong process.
        let w = crate::testkit::World::new("guard-bg");
        w.register(std::process::id(), "sess", "/tmp", 5, "idle");
        let me = std::process::id();
        let child = std::process::Command::new("sleep").arg("30").spawn();
        let mut child = child.unwrap();
        w.register_bg(child.id(), "sess", "/tmp", 1, "sess", false);
        let entries = live_entries(&w.root);
        assert_eq!(entries.iter().map(|e| e.pid).collect::<Vec<_>>(), [me]);
        let _ = child.kill();
        let _ = child.wait();
    }

    // ---- the notes the guard carries ---------------------------------------------------------------------------

    struct NotesRig {
        base: PathBuf,
        config: PathBuf,
        cache: PathBuf,
    }

    impl NotesRig {
        fn new(name: &str) -> NotesRig {
            let base = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-guard-notes-{name}-{}", std::process::id())));
            let _ = fs::remove_dir_all(&base);
            let r = NotesRig { config: base.join("config"), cache: base.join("cache"), base };
            fs::create_dir_all(&r.config).unwrap();
            fs::create_dir_all(&r.cache).unwrap();
            r
        }

        /// One project with these notes, and `session` in it.
        fn project(&self, session: &str, notes: &str) -> String {
            let mut p = crate::projects::Projects::open(&self.config);
            let id = p.create("Web shop").unwrap();
            p.assign(session, Some(&id)).unwrap();
            self.notes(&id, notes);
            id
        }

        fn notes(&self, id: &str, text: &str) {
            let path = crate::projects::notes_path(&self.config, id);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        /// What the hook would carry for this prompt: the context, or None.
        fn prompt(&self, session: &str) -> Option<String> {
            self.prompt_in(session, "/home/x/work", false)
        }

        fn prompt_in(&self, session: &str, cwd: &str, ai_child: bool) -> Option<String> {
            let input = serde_json::json!({"session_id": session, "cwd": cwd, "hook_event_name": "UserPromptSubmit", "prompt": "carry on"});
            notes_context(&input, &self.config, &self.cache, ai_child)
        }
    }

    impl Drop for NotesRig {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    #[test]
    fn the_guard_carries_the_notes_once_and_again_only_when_they_change() {
        let r = NotesRig::new("carry");
        let id = r.project("sess-1", "# Overview\nSell socks.\n");
        let first = r.prompt("sess-1").expect("the first prompt carries them");
        assert!(first.starts_with("These are the notes of the project \"Web shop\" that this session belongs to"), "{first}");
        assert!(first.ends_with("# Overview\nSell socks."), "{first}");
        assert_eq!(r.prompt("sess-1"), None, "the same notes are not sent twice");
        assert_eq!(r.prompt("sess-1"), None);
        r.notes(&id, "# Overview\nSell socks and hats.\n");
        assert!(r.prompt("sess-1").expect("changed notes go again").contains("hats"));
        assert_eq!(r.prompt("sess-1"), None);
        // Another session of the same project gets its own copy, and its own record.
        crate::projects::Projects::open(&r.config).assign("sess-2", Some(&id)).unwrap();
        assert!(r.prompt("sess-2").is_some());
        assert_eq!(r.prompt("sess-2"), None);
        // What was written down is the hash, nothing else.
        let written = fs::read_to_string(crate::sessionhook::sent_path(&r.cache, "sess-1")).unwrap();
        assert_eq!(written.len(), 16, "{written:?}");
    }

    #[test]
    fn a_squads_leader_gets_its_tickets_project_notes_from_the_guard_too_without_joining() {
        let r = NotesRig::new("leader");
        let id = r.project("someone", "# Overview\nSell socks.\n");
        let mut t = crate::tickets::Tickets::open(&r.config, &id);
        let n = t.add(crate::tickets::NewTicket { title: "Basket".into(), ..Default::default() }).unwrap();
        t.start_leader(n, &crate::tickets::Leader { squad: "coding".into(), session: "lead-1".into(), started: 1, names: Default::default(), snapshot: None }, &[]).unwrap();
        let text = r.prompt("lead-1").expect("the leader gets them");
        assert!(text.starts_with("These are the notes of the project \"Web shop\" that your ticket belongs to") && text.ends_with("Sell socks."), "{text}");
        assert_eq!(r.prompt("lead-1"), None);
        assert!(crate::projects::Projects::open(&r.config).of("lead-1").is_none());
        assert_eq!(r.prompt("not-a-leader"), None);
    }

    #[test]
    fn the_two_hooks_never_send_the_same_version_twice_between_them() {
        let r = NotesRig::new("both");
        let id = r.project("sess-1", "# Overview\nSell socks.\n");
        let start = |source: &str| {
            let input = serde_json::json!({"session_id": "sess-1", "source": source, "cwd": "/home/x/work"}).to_string();
            crate::sessionhook::session_start(&input, None, &r.config, &r.cache)
        };
        // The session starts: the SessionStart hook hands them over, so the first prompt after it does not.
        assert!(start("startup").is_some());
        assert_eq!(r.prompt("sess-1"), None, "the session already has this version");
        // The notes change while the session runs: the prompt carries them, and a resume afterwards does not.
        r.notes(&id, "# Overview\nSell socks and hats.\n");
        assert!(r.prompt("sess-1").is_some());
        assert_eq!(start("resume"), None, "the SessionStart hook sees they are already there");
        // A compaction drops what Claude was told, so that hook sends them again - and the next prompt doesn't.
        assert!(start("compact").is_some());
        assert_eq!(r.prompt("sess-1"), None);
    }

    #[test]
    fn the_guard_carries_nothing_for_a_session_that_should_not_get_notes() {
        let r = NotesRig::new("refusals");
        let id = r.project("sess-1", "# Overview\nSell socks.\n");
        // In no project at all.
        assert_eq!(r.prompt("loose"), None);
        // The helper's own calls: by the environment, and by the folder they work in.
        assert_eq!(r.prompt_in("sess-1", "/home/x/work", true), None, "PLATOON_AI is set");
        let work = crate::ai::work_dir(&r.cache).display().to_string();
        assert_eq!(r.prompt_in("sess-1", &work, false), None, "the helper's own work folder");
        // An archived session of a live project.
        crate::archive::Archive::open(&r.config).archive("sess-1", crate::usage::now()).unwrap();
        assert_eq!(r.prompt("sess-1"), None);
        crate::archive::Archive::open(&r.config).restore("sess-1", crate::usage::now()).unwrap();
        // An archived project.
        let mut p = crate::projects::Projects::open(&r.config);
        p.archive(&id, &[]).unwrap();
        assert_eq!(r.prompt("sess-1"), None);
        p.restore(&id).unwrap();
        // Notes that are missing, or say nothing.
        r.notes(&id, "   \n\n");
        assert_eq!(r.prompt("sess-1"), None);
        let _ = fs::remove_file(crate::projects::notes_path(&r.config, &id));
        assert_eq!(r.prompt("sess-1"), None);
        // Input that makes no sense, and a session id that is not a file name.
        for bad in [serde_json::json!({}), serde_json::json!({"session_id": 5}), serde_json::json!({"session_id": "../../x"}), serde_json::json!(null)] {
            assert_eq!(notes_context(&bad, &r.config, &r.cache, false), None, "{bad}");
        }
        // Nothing was written down for any of them.
        let sent = r.cache.join("notes-sent");
        assert!(!sent.exists() || fs::read_dir(&sent).unwrap().count() == 0, "nothing was recorded as sent");
        // And with the notes back, the very next prompt still carries them.
        r.notes(&id, "# Overview\nSell socks.\n");
        assert!(r.prompt("sess-1").is_some());
    }

    #[test]
    fn what_the_guard_prints_for_a_prompt_is_context_for_that_conversation() {
        let out = notes_output("the notes");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
        assert_eq!(v["hookSpecificOutput"]["additionalContext"], "the notes");
        assert_eq!(v.as_object().unwrap().len(), 1, "nothing else: it must not decide anything about the prompt");
        // No "decision" or "continue" key anywhere: the guard's own answer is its exit code, as it always was.
        assert!(!out.contains("decision") && !out.contains("permission"), "{out}");
    }

    #[test]
    fn ancestry_finds_the_parent_chain() {
        let chain = ancestors(std::process::id());
        assert_eq!(chain[0], std::process::id());
        assert!(chain.len() >= 2, "should at least see the parent: {chain:?}");
    }
}
