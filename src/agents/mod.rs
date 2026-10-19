//! The AI coding agents platoon runs, behind one seam. Each agent has a module of its own here (`claude.rs`: Claude
//! Code) that is the only place that knows its files, flags, hooks and models; the rest of platoon goes through this
//! one: the `Agent` it is dealing with, and the agent-neutral shapes below, which an agent's module produces.

use std::{
    fs,
    path::{Path, PathBuf},
};

pub mod claude;

/// An AI coding agent platoon runs. Closed on purpose: a new agent makes the compiler name every place that must
/// answer for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Agent {
    Claude,
}

impl Agent {
    /// A model id as a row shows it (`claude-sonnet-5` -> `Sonnet 5`).
    pub fn model_name(self, id: &str) -> String {
        match self {
            Agent::Claude => claude::pretty_model(id),
        }
    }

    /// How many tokens of context the conversation's model has room for.
    pub fn context_window(self, t: &Transcript) -> u64 {
        match self {
            Agent::Claude => claude::window_size(t.context_tokens, &mut t.large_window.clone()),
        }
    }
}

// ---- what platoon reads about conversations and the processes running them ----------------------------------------

/// A live agent process running a conversation, as its agent registers it.
#[derive(Debug)]
pub struct Live {
    pub pid: u32,
    pub id: String,
    pub cwd: String,
    /// Only a name the user chose; the agent's own placeholder is dropped.
    pub name: Option<String>,
    pub started_at: u64,
    pub busy: bool,
    /// Set for a background session: one run by the agent's own service (Claude Code's `claude --bg`, `claude
    /// agents`) rather than in a terminal. It is opened by attaching to it (`Start::Attach`), never by resuming it.
    /// Holds the id that attaching takes.
    pub background: Option<String>,
    /// Set for an interactive session that handed its conversation to a background session (Claude Code's
    /// `parkedJobId`): holds the id of that background session's job (the id attaching takes). Its process stays
    /// alive but no longer works on the conversation, and its registry status is never set back to idle, so a parked
    /// entry is never `busy` and never counts as a copy that writes to the conversation.
    pub parked: Option<String>,
}

impl Live {
    pub fn is_background(&self) -> bool {
        self.background.is_some()
    }

    pub fn is_parked(&self) -> bool {
        self.parked.is_some()
    }
}

/// What platoon knows about one saved conversation, read from its transcript.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transcript {
    /// Working directory the conversation started in (the first `cwd` recorded).
    pub cwd: Option<String>,
    /// Folders added beside it (`/add-dir`, `--add-dir`), as of the latest environment snapshot read. Empty when
    /// none were added, or the transcript is older than those snapshots.
    pub extra_dirs: Vec<String>,
    pub branch: Option<String>,
    /// Raw model id from the latest assistant message.
    pub model: Option<String>,
    pub context_tokens: u64,
    /// The model's large context window is in use (the agent's model id says so, or the conversation has been seen
    /// past the default window). Never flips back. See `Agent::context_window`.
    pub large_window: bool,
    pub ai_title: Option<String>,
    /// First real user prompt. None means nothing but slash-command echoes and injected blocks: junk.
    pub first_prompt: Option<String>,
    /// Timestamp of the latest user or assistant entry, ms since the epoch.
    pub last_active_ms: Option<u64>,
}

/// What one conversation spent, summed from the usage records in its transcript (and its subagents' transcripts).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Spent {
    /// Sent to the model and not read from its cache: `input_tokens` plus `cache_creation_input_tokens`.
    pub input: u64,
    /// Written by the model (`output_tokens`, thinking included).
    pub output: u64,
    /// Read back from the model's cache (`cache_read_input_tokens`): the conversation so far, sent again each turn.
    pub cache_read: u64,
    /// Some of it was spent by the conversation's subagents.
    pub subagents: bool,
}

/// What a task's session has used, and when it last wrote: what its transcript and its subagents' transcripts spent
/// (`spent`), and the latest time any of those files was written (ms since the epoch).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionLog {
    pub spent: Spent,
    pub last_at: Option<u64>,
}

/// A working folder whose sessions are not conversations to show: the folder platoon's own `claude -p` calls run in.
/// A process registered there, or a transcript recorded there, is platoon's own helper, never something the user
/// opened. Compared after resolving `.`, `..` and symlinks, so a different spelling of the same folder still counts,
/// while a folder that only has a similar name (`ai-work-2`, or `ai-work` somewhere else) does not.
#[derive(Clone, Debug)]
pub struct Hidden {
    resolved: PathBuf,
}

impl Hidden {
    pub fn new(dir: &Path) -> Hidden {
        Hidden { resolved: resolve_path(dir) }
    }

    /// Is `cwd` this folder, or somewhere inside it?
    pub fn covers(&self, cwd: &str) -> bool {
        !cwd.is_empty() && resolve_path(Path::new(cwd)).starts_with(&self.resolved)
    }
}

/// `p` with symlinks and `..` resolved as far as it exists; the part that doesn't exist (yet, or any more) is
/// tidied up in writing only.
fn resolve_path(p: &Path) -> PathBuf {
    use std::path::Component;
    if let Ok(c) = fs::canonicalize(p) {
        return c;
    }
    let comps: Vec<Component> = p.components().collect();
    let (mut out, from) = (0..comps.len())
        .rev()
        .find_map(|k| fs::canonicalize(comps[..k].iter().collect::<PathBuf>()).ok().filter(|_| k > 0).map(|c| (c, k)))
        .unwrap_or_default();
    for c in &comps[from..] {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn tmp(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-hidden-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_folder_is_hidden_however_it_is_spelled_and_a_similar_name_is_not() {
        let base = tmp("spell");
        let work = base.join("cache/ai-work");
        fs::create_dir_all(&work).unwrap();
        fs::create_dir_all(base.join("cache/ai-work-2")).unwrap();
        fs::create_dir_all(base.join("other/ai-work")).unwrap();
        fs::create_dir_all(base.join("cache/x")).unwrap();
        symlink(&work, base.join("link")).unwrap();
        let h = Hidden::new(&work);
        let s = |p: PathBuf| p.display().to_string();
        for yes in [
            s(work.clone()),
            format!("{}/", work.display()),
            s(base.join("cache/x/../ai-work")),
            s(base.join("./cache/./ai-work")),
            s(base.join("link")),
            s(work.join("inside")),
            s(base.join("link/deeper/still")),
        ] {
            assert!(h.covers(&yes), "{yes}");
        }
        for no in [
            "",
            "/",
            "relative/ai-work",
            &s(base.join("cache")),
            &s(base.join("cache/ai-work-2")),
            &s(base.join("other/ai-work")),
            &s(base.join("cache/ai-wor")),
        ] {
            assert!(!h.covers(no), "{no}");
        }
        // Hidden before the folder exists (nothing has been called yet).
        let ahead = Hidden::new(&base.join("cache/not-yet/ai-work"));
        assert!(ahead.covers(&s(base.join("cache/not-yet/ai-work"))) && ahead.covers(&s(base.join("cache/x/../not-yet/ai-work"))));
        assert!(!ahead.covers(&s(base.join("cache/not-yet/ai-work-2"))));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_claude_conversation_shows_its_model_by_name_and_its_window_from_what_its_transcript_says() {
        let a = Agent::Claude;
        assert_eq!(a.model_name("claude-opus-5-5"), "Opus 5.5");
        let t = Transcript { context_tokens: 120_000, ..Default::default() };
        assert_eq!(a.context_window(&t), 200_000);
        assert_eq!(a.context_window(&Transcript { large_window: true, ..t.clone() }), 1_000_000);
        assert_eq!(a.context_window(&Transcript { context_tokens: 250_000, ..t }), 1_000_000);
    }
}
