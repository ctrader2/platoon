//! `platoon hook session-start`: Claude Code runs it (a `SessionStart` hook) when a session starts, resumes, is
//! cleared or compacted. If the session is in a project that has notes (`project.md`), the notes are handed to
//! Claude as extra context, but only when they are new or changed for that session, or after a compaction (which
//! drops what Claude was told earlier). What was sent is remembered per session as a hash in
//! `<cache>/notes-sent/<session id>`: one small file per session, so any number of sessions can start at once.
//!
//! A session platoon started under a project carries `PLATOON_PROJECT=<id>`: a brand-new session (source
//! `startup` or `clear`) that is in no project yet is placed in that one here, before platoon itself has seen it.
//!
//! Codex's own hook gets the same variables from the process platoon started it in, and fires with the first message.
//! Nothing about making tasks is said here: that is the `/to-tasks` skill (`skill.rs`) and `t` in platoon.
//!
//! It must never get in a session's way: it is quick, prints nothing unless it has notes to give, and exits 0
//! whatever happens.

use crate::agents::{Agent, HookEvent, HookKind, HookReply};
use crate::projects::{self, Projects};
use std::{fs, io::Read, path::Path};

/// FNV-1a, 64 bits: stable across builds and Rust versions (a changed hash would only resend the notes once).
fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// A session id or project id that is safe to use as a file name.
pub fn safe_name(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn sent_path(cache: &Path, session: &str) -> std::path::PathBuf {
    cache.join("notes-sent").join(session)
}

/// Which version of a project's notes this is: the project's id and the text together, so notes moved between
/// projects still count as new. Both hooks use this one hash, which is why neither sends what the other just sent.
pub fn notes_hash(project: &str, notes: &str) -> String {
    format!("{:016x}", fnv(format!("{project}\n{notes}").as_bytes()))
}

/// The version of these notes this session has already been given, if any.
pub fn sent_hash(cache: &Path, session: &str) -> Option<String> {
    Some(fs::read_to_string(sent_path(cache, session)).ok()?.trim().to_string())
}

/// Remember that `session` has been given this version.
pub fn mark_sent(cache: &Path, session: &str, hash: &str) {
    let _ = projects::write_atomic(&sent_path(cache, session), hash);
}

/// The line that goes in front of a project's notes wherever they are handed to a session, then the notes, cut at
/// the cap (`notes::for_sessions`). The same words in both hooks, so a conversation that gets them twice reads the
/// same thing.
pub fn context_text(project_name: &str, notes: &str) -> String {
    format!(
        "These are the notes of the project \"{project_name}\" that this session belongs to (kept in platoon by the user):\n\n{}",
        crate::notes::for_sessions(notes).trim_end()
    )
}

/// What the hook prints for `input` (the hook's JSON), or None for nothing. Records what it sends.
pub fn session_start(agent: Agent, input: &str, env_project: Option<&str>, config: &Path, cache: &Path) -> Option<String> {
    let input = agent.hook_input(HookKind::SessionStart, input)?;
    let session = input.session.as_str();
    if !safe_name(session) {
        return None;
    }
    let HookEvent::SessionStart { source } = &input.event else { return None };
    let source = source.as_str();
    let mut projects = Projects::open(config);
    if projects.of(session).is_none() {
        let fresh = matches!(source, "startup" | "clear");
        if let Some(p) = env_project.filter(|p| fresh && !p.is_empty() && projects.get(p).is_some())
            && projects.place(session, p).unwrap_or(false)
        {
            // A session placed here has joined a project without any turn ending, so the notes are told: an
            // update runs a few minutes later instead of waiting for someone to resume it.
            crate::notesrun::mark_joined(config, cache, p, session, crate::notesrun::now_ms());
        }
    }
    let project = projects.of(session).cloned()?;
    let notes = notes_context(&projects, &project, session, source, cache)?;
    Some(agent.hook_answer(&HookReply::StartContext(notes)).stdout)
}

/// The project's notes for this session, when it hasn't been given this version yet (or after a compaction, which
/// drops what Claude was told earlier). Records what it gives.
fn notes_context(projects: &Projects, project: &projects::Project, session: &str, source: &str, cache: &Path) -> Option<String> {
    let notes = fs::read_to_string(projects.notes_path(&project.id)).ok()?;
    if notes.trim().is_empty() {
        return None;
    }
    let hash = notes_hash(&project.id, &notes);
    if source != "compact" && sent_hash(cache, session).as_deref() == Some(hash.as_str()) {
        return None;
    }
    mark_sent(cache, session, &hash);
    Some(context_text(&project.name, &notes))
}

/// `platoon hook session-start [--agent codex]`: the hook of `agent`'s sessions.
pub fn run(agent: Agent) -> i32 {
    // A program switched off in platoon's settings gets no notes.
    if crate::agents::switched_off(&crate::archive::config_dir(), agent) {
        return 0;
    }
    let _ = std::panic::catch_unwind(|| {
        let mut input = String::new();
        if std::io::stdin().take(1 << 20).read_to_string(&mut input).is_err() {
            return;
        }
        let env = std::env::var("PLATOON_PROJECT").ok();
        if let Some(out) = session_start(agent, &input, env.as_deref(), &crate::archive::config_dir(), &crate::usage::cache_dir()) {
            println!("{out}");
        }
    });
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::path::PathBuf;

    struct Rig {
        base: PathBuf,
        config: PathBuf,
        cache: PathBuf,
    }

    impl Rig {
        fn new(name: &str) -> Self {
            let base = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-sshook-{name}-{}", std::process::id())));
            let _ = fs::remove_dir_all(&base);
            Rig { config: base.join("config"), cache: base.join("cache"), base }
        }
        fn run(&self, session: &str, source: &str, env: Option<&str>) -> Option<String> {
            session_start(
                Agent::Claude,
                &json!({"session_id": session, "source": source, "cwd": "/tmp", "hook_event_name": "SessionStart"}).to_string(),
                env,
                &self.config,
                &self.cache,
            )
        }
        fn notes(&self, p: &Projects, id: &str, text: &str) {
            let path = p.notes_path(id);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    fn context(out: &str) -> String {
        let v: Value = serde_json::from_str(out).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
        v["hookSpecificOutput"]["additionalContext"].as_str().unwrap().to_string()
    }

    #[test]
    fn notes_are_sent_once_then_again_only_when_changed_or_after_a_compaction() {
        let r = Rig::new("once");
        let mut p = Projects::open(&r.config);
        let id = p.create("Web shop").unwrap();
        p.assign("sess-1", Some(&id)).unwrap();
        r.notes(&p, &id, "# Overview\nsell socks\n");
        let first = r.run("sess-1", "startup", None).expect("sent at the start");
        let text = context(&first);
        assert!(text.starts_with("These are the notes of the project \"Web shop\""), "{text}");
        assert!(text.ends_with("# Overview\nsell socks"), "{text}");
        assert_eq!(r.run("sess-1", "resume", None), None, "unchanged: nothing the second time");
        assert_eq!(r.run("sess-1", "clear", None), None);
        assert!(r.run("sess-1", "compact", None).is_some(), "a compaction drops it, so it goes again");
        r.notes(&p, &id, "# Overview\nsell socks and hats\n");
        assert!(context(&r.run("sess-1", "resume", None).unwrap()).contains("hats"), "changed: sent again");
        assert_eq!(r.run("sess-1", "resume", None), None);
        // Another session of the project gets its own copy.
        p.assign("sess-2", Some(&id)).unwrap();
        assert!(r.run("sess-2", "resume", None).is_some());
    }

    #[test]
    fn no_project_no_notes_or_bad_input_print_nothing() {
        let r = Rig::new("nothing");
        let mut p = Projects::open(&r.config);
        let id = p.create("a").unwrap();
        assert_eq!(r.run("loose", "startup", None), None, "in no project");
        p.assign("member", Some(&id)).unwrap();
        assert_eq!(r.run("member", "startup", None), None, "no notes file");
        r.notes(&p, &id, "  \n\n");
        assert_eq!(r.run("member", "startup", None), None, "empty notes");
        for bad in ["", "{", "[]", r#"{"source":"startup"}"#, r#"{"session_id":"../../x"}"#, r#"{"session_id":5}"#] {
            assert_eq!(session_start(Agent::Claude, bad, Some(&id), &r.config, &r.cache), None, "{bad}");
        }
        assert!(!r.cache.join("notes-sent").exists() || fs::read_dir(r.cache.join("notes-sent")).unwrap().count() == 0);
    }

    #[test]
    fn platoon_project_places_a_new_session_but_never_moves_one_or_revives_a_resume() {
        let r = Rig::new("place");
        let mut p = Projects::open(&r.config);
        let (a, b) = (p.create("a").unwrap(), p.create("b").unwrap());
        r.notes(&p, &a, "notes of a");
        assert!(context(&r.run("brand-new", "startup", Some(&a)).unwrap()).contains("notes of a"));
        assert_eq!(Projects::open(&r.config).project_id_of("brand-new"), Some(a.as_str()));
        // Already in b: the variable does not move it.
        p.assign("in-b", Some(&b)).unwrap();
        assert_eq!(r.run("in-b", "startup", Some(&a)), None);
        assert_eq!(Projects::open(&r.config).project_id_of("in-b"), Some(b.as_str()));
        // A resumed session the user took out of the project is not put back.
        assert_eq!(r.run("taken-out", "resume", Some(&a)), None);
        assert!(Projects::open(&r.config).of("taken-out").is_none());
        // A /clear starts a new id in the same process: that one is placed.
        assert!(r.run("after-clear", "clear", Some(&a)).is_some());
        // An unknown project id places nothing.
        assert_eq!(r.run("x", "startup", Some("pnope")), None);
        assert!(Projects::open(&r.config).of("x").is_none());
    }

    #[test]
    fn a_session_platoon_started_is_given_its_notes_and_nothing_about_making_tasks() {
        // Making tasks is the /to-tasks skill's and `t`'s: nothing of it at the start, Claude Code's or Codex's.
        let r = Rig::new("no-offer");
        let mut p = Projects::open(&r.config);
        let id = p.create("Web shop").unwrap();
        p.assign("in-shop", Some(&id)).unwrap();
        r.notes(&p, &id, "# Summary\nsell socks\n");
        let text = context(&r.run("in-shop", "startup", None).unwrap());
        assert!(text.contains("sell socks") && !text.contains("task add") && !text.contains("Ready to create tasks"), "{text}");
        let codex = json!({"session_id": "01a20000-1111-7000-8000-00000000c0de", "source": "startup", "cwd": "/home/u/shop",
            "hook_event_name": "SessionStart", "model": "gpt-6-luna", "permission_mode": "default"})
        .to_string();
        assert_eq!(session_start(Agent::Codex, &codex, None, &r.config, &r.cache), None, "in no project: nothing");
    }

    #[test]
    fn notes_over_the_cap_are_sent_cut_with_a_line_saying_so() {
        let r = Rig::new("cap");
        let mut p = Projects::open(&r.config);
        let id = p.create("Web shop").unwrap();
        p.assign("s", Some(&id)).unwrap();
        let line = format!("{}\n", "n".repeat(99));
        r.notes(&p, &id, &format!("# Summary\n{}", line.repeat(100)));
        let text = context(&r.run("s", "startup", None).unwrap());
        assert!(
            text.ends_with("[platoon cut the notes here: they are 10,010 characters and only the first 8,000 are given to sessions.]"),
            "{text}"
        );
        assert!(crate::notes::chars(&text) < crate::notes::CAP + 300);
    }

    #[test]
    fn many_hooks_at_once_leave_readable_records() {
        let r = Rig::new("many");
        let mut p = Projects::open(&r.config);
        let id = p.create("a").unwrap();
        r.notes(&p, &id, "notes");
        let (config, cache) = (r.config.clone(), r.cache.clone());
        let threads: Vec<_> = (0..24)
            .map(|i| {
                let (config, cache, id) = (config.clone(), cache.clone(), id.clone());
                std::thread::spawn(move || {
                    // Half of them on the same session, all at once.
                    let s = if i % 2 == 0 { "shared".to_string() } else { format!("s{i}") };
                    let input = json!({"session_id": s, "source": "startup"}).to_string();
                    session_start(Agent::Claude, &input, Some(&id), &config, &cache)
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let p = Projects::open(&r.config);
        assert!(!p.is_corrupt());
        assert_eq!(p.members_of(&id).len(), 13);
        for e in fs::read_dir(r.cache.join("notes-sent")).unwrap().flatten() {
            let text = fs::read_to_string(e.path()).unwrap();
            assert_eq!(text.len(), 16, "{:?}: {text:?}", e.file_name());
            assert!(!e.file_name().to_string_lossy().ends_with(".tmp"));
        }
        assert_eq!(r.run("shared", "resume", Some(&id)), None, "recorded as sent");
    }
}
