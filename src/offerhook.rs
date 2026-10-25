//! The offer to make tasks: just before a session platoon started begins its work, it asks the user once whether to
//! turn that work into platoon tasks for a squad, or do it right there.
//!
//! An instruction given at the start of a session loses to everything else in the conversation (platoon gave one, and
//! the sessions never asked), so this is a `PreToolUse` hook instead. The first step that starts work
//! (`HookEvent::StartsWork`: an agent launched, a file edited, a worktree made; never a shell command, never an edit
//! outside the session's folder, `is_the_work`) is refused once, and the refusal tells the agent to ask (`refusal`):
//! Claude Code with its question tool, Codex in plain words at the end of its reply. The refusal is the mark
//! (`<cache>/offer-asked/<session id>`): whatever the user answers, every later step goes through.
//!
//! Where it runs: a session platoon starts or resumes that is not a task's own carries `PLATOON_OFFER_TASKS=1`
//! (`OFFER_ENV`). Claude Code's hook is given to that session alone, with `--settings` (`claude::offer_args`):
//! `platoon hook offer-tasks`. Codex's hooks are its `hooks.json`'s, the same for every session, so the one it already
//! runs before an edit (`hook task-before --agent codex`, `codex::TASK_HOOKS`) hands a session that is no task's to
//! this one (`taskrun::run_hook`): nothing new for setup to add or for Codex to trust.
//!
//! Off in the settings (Sessions, "Offer tasks before starting work"), or for a program switched off, it lets
//! everything through. It is quick: once the session has its mark, it reads its input and looks for one file.

use crate::agents::{Agent, HookEvent, HookKind, HookReply};
use crate::projects::Projects;
use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime},
};

/// The variable a session platoon hosts carries when it is to offer making tasks.
pub const OFFER_ENV: &str = "PLATOON_OFFER_TASKS";

/// The folder of the marks, one empty file per session that has been asked.
pub const MARKS: &str = "offer-asked";

/// A mark older than this is taken away when another session is asked: a conversation resumed after that long may
/// be asked again.
const KEEP: Duration = Duration::from_secs(90 * 24 * 3600);

/// Does a session with this environment offer making tasks: `PLATOON_OFFER_TASKS=1` (`offer`), and no task named
/// (`agents::TASK_ENV`, `task`)?
pub fn offering(offer: Option<&str>, task: Option<&str>) -> bool {
    offer == Some("1") && task.is_none_or(str::is_empty)
}

/// Where the hook finds what it needs.
pub struct Places<'a> {
    pub config: &'a Path,
    pub cache: &'a Path,
    /// The agent's own folder (`Agent::home`): an edit in it (Claude Code's memory, its plans) is not the work.
    pub agent_home: &'a Path,
    /// The platoon program the session runs to make tasks, by its full path.
    pub exe: &'a str,
    /// The project the session was started in (`PLATOON_PROJECT`), for one not placed in a project yet.
    pub env_project: Option<&'a str>,
}

/// The mark of `session`: there once it has been asked.
pub fn mark_path(cache: &Path, session: &str) -> PathBuf {
    cache.join(MARKS).join(session)
}

/// What the hook of `agent`'s says for `input` (the hook's JSON): the refusal, the first time this session starts
/// work, else None (the step goes through). `switched_off` says whether `agent` is switched off in platoon's settings;
/// it is asked only before a refusal.
pub fn hook(agent: Agent, input: &str, at: &Places, switched_off: impl FnOnce() -> bool) -> Option<String> {
    let input = agent.hook_input(HookKind::BeforeWork, input)?;
    let HookEvent::StartsWork { files } = &input.event else { return None };
    let session = input.session.as_str();
    if !crate::sessionhook::safe_name(session) || mark_path(at.cache, session).exists() {
        return None;
    }
    if !crate::settings::load(at.config).0.offer_tasks || switched_off() || !is_the_work(&input.cwd, files, at.agent_home) {
        return None;
    }
    // Two steps at once (parallel edits) race here: only the one that makes the mark refuses. A mark that can't be
    // made refuses nothing, or every step would be refused.
    if !take_mark(at.cache, session) {
        return None;
    }
    let projects = Projects::open(at.config);
    let project = projects
        .project_id_of(session)
        .map(String::from)
        .or_else(|| at.env_project.filter(|p| !p.is_empty() && projects.get(p).is_some()).map(String::from));
    let folder = Some(input.cwd.as_str()).filter(|c| !c.is_empty());
    let why = refusal(agent, at.exe, project.as_deref(), folder, &crate::squads::all(at.config));
    Some(agent.hook_answer(&HookReply::Deny(why)).stdout)
}

/// Make `session`'s mark; false when it was there already or can't be made. Old marks of other sessions go (`KEEP`).
fn take_mark(cache: &Path, session: &str) -> bool {
    let dir = cache.join(MARKS);
    if fs::create_dir_all(&dir).is_err() {
        return false;
    }
    let made = fs::OpenOptions::new().write(true).create_new(true).open(dir.join(session)).is_ok();
    if made {
        let now = SystemTime::now();
        for e in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let old = e.metadata().and_then(|m| m.modified()).is_ok_and(|t| now.duration_since(t).is_ok_and(|age| age > KEEP));
            if old {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    made
}

/// Is a step that changes `files` (none: a step that isn't about files) the start of the session's work? A step
/// without files is. An edit is when one of its files is in the session's folder `cwd` and not in the agent's own
/// folder `agent_home`, both as they really are on disk: a file reached through a link that leads elsewhere (a notes
/// folder linked into the repository) is where the link leads. A session whose folder isn't known counts every file
/// outside the agent's folder.
pub fn is_the_work(cwd: &str, files: &[String], agent_home: &Path) -> bool {
    if files.is_empty() {
        return true;
    }
    let folder = (!cwd.is_empty()).then(|| real_path(Path::new(cwd)));
    let home = (!agent_home.as_os_str().is_empty()).then(|| real_path(agent_home));
    files.iter().any(|f| {
        let f = Path::new(f);
        let path = match &folder {
            _ if f.is_absolute() => real_path(f),
            Some(_) => real_path(&Path::new(cwd).join(f)),
            None => return true,
        };
        folder.as_ref().is_none_or(|d| path.starts_with(d)) && !home.as_ref().is_some_and(|h| path.starts_with(h))
    })
}

/// `p` as it really is: `.` and `..` taken out, then the part of it that exists resolved (links followed), with the
/// rest (a file still to be written) after it.
fn real_path(p: &Path) -> PathBuf {
    let mut clean = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                clean.pop();
            }
            Component::CurDir => {}
            c => clean.push(c),
        }
    }
    let mut rest = Vec::new();
    let mut at = clean.as_path();
    loop {
        if let Ok(mut real) = at.canonicalize() {
            real.extend(rest.iter().rev());
            return real;
        }
        match (at.parent(), at.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                at = parent;
            }
            _ => return clean,
        }
    }
}

/// `text` as one word for `sh`: as it is when nothing in it is special, else in single quotes.
fn sh_word(text: &str) -> String {
    if !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+:,@%=".contains(c)) {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

/// The question, as the session asks it.
pub const QUESTION: &str = "Turn this into platoon tasks for a squad?";

/// Why the step was held back, worded for `agent`: platoon asks (not an error) that the agent ask the user `QUESTION`
/// first (Claude Code with its question tool, options "Make tasks" and "Do it here"; Codex plainly at the end of its
/// reply, then wait). On "Make tasks": draft them, show them, ask "Make these tasks?", then make each with `exe task
/// add` (the session's project and folder, the squad that suits it: the usable ones of `squads`, each with its
/// one-line description), say how many and where, and neither start them nor do their work. On "Do it here": say in
/// a line that it won't ask again and that `t` makes tasks later, and carry on with the step.
pub fn refusal(agent: Agent, exe: &str, project: Option<&str>, folder: Option<&str>, squads: &[crate::squads::Squad]) -> String {
    let mut cmd = format!("{} task add", sh_word(exe));
    if let Some(p) = project {
        cmd.push_str(&format!(" --project {}", sh_word(p)));
    }
    if let Some(f) = folder {
        cmd.push_str(&format!(" --folder {}", sh_word(f)));
    }
    cmd.push_str(" --squad <squad> --title '<short title>' <<'TASK'");
    let (ask, confirm) = match agent {
        Agent::Claude => (
            format!("Ask with the AskUserQuestion tool: question \"{QUESTION}\", options \"Make tasks\" and \"Do it here\"."),
            "ask with the AskUserQuestion tool \"Make these tasks?\", options \"Make them\" and \"Change them\", before making \
             any (on \"Change them\", ask what to change and show the list again)."
                .to_string(),
        ),
        // Codex's question tool is there in plan mode only: the question goes in the reply, and the answer comes as
        // the user's next message.
        Agent::Codex => (
            format!(
                "Ask at the end of your reply, in plain words: \"{QUESTION} Make tasks, or do it here?\" Then stop and wait for \
                 their answer."
            ),
            "ask at the end of your reply \"Make these tasks?\" and wait for their answer before making any.".to_string(),
        ),
    };
    let mut t = format!(
        "platoon is asking, not an error. platoon (the app that started this session) held back this one step so you can \
         ask the user first whether to turn the work into platoon tasks, each done by a squad in a session of its own, or \
         do it here. {ask} platoon holds back nothing more in this conversation: whatever they answer, the same step goes \
         through when you take it again.\n\n\
         If they choose \"Make tasks\": don't do the work here. Draft the tasks, one for each piece of the agreed work that \
         can be done on its own. Show them to the user as a list (each one's title, squad and Done when) and {confirm} \
         Make each task with one command:\n\n"
    );
    t.push_str(&format!(
        "{cmd}\n## What it should do\n\n<what to do and why, enough for someone who hasn't seen this conversation>\n\n\
         ## Done when\n\n- [ ] <a check anyone can make>\n- [ ] <another>\nTASK\n\n"
    ));
    t.push_str("Pick the squad that suits each task:\n");
    for s in squads.iter().filter(|s| s.usable()) {
        t.push_str(&format!("- {}: {}\n", s.id, s.description));
    }
    t.push_str(
        "\nThen tell the user how many tasks you made and that they are on platoon's Tasks tab. Don't start them, and \
         don't do their work in this conversation.\n\n\
         If they choose \"Do it here\": tell them in one line that you won't ask again in this conversation, and that `t` \
         on this session's row in platoon makes tasks from it later. Then carry on with the step you were about to take.",
    );
    t
}

/// This platoon by its full path, as a session runs it (its hook, `task add`): it works without platoon on the PATH.
pub fn this_platoon() -> String {
    std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "platoon".into())
}

/// `platoon hook offer-tasks` (Claude Code's, `--settings`), and Codex's `hook task-before --agent codex` in a session
/// that is no task's: always 0, with the refusal on stdout when there is one. A session that doesn't offer ends at once,
/// without reading anything.
pub fn run(agent: Agent) -> i32 {
    let task = std::env::var(crate::agents::TASK_ENV).ok();
    if !offering(std::env::var(OFFER_ENV).ok().as_deref(), task.as_deref()) {
        return 0;
    }
    let _ = std::panic::catch_unwind(|| {
        let mut input = String::new();
        if std::io::stdin().take(1 << 20).read_to_string(&mut input).is_err() {
            return;
        }
        let (config, cache, home) = (crate::archive::config_dir(), crate::usage::cache_dir(), agent.home());
        let exe = this_platoon();
        let env_project = std::env::var("PLATOON_PROJECT").ok();
        let at = Places { config: &config, cache: &cache, agent_home: &home, exe: &exe, env_project: env_project.as_deref() };
        if let Some(out) = hook(agent, &input, &at, || crate::agents::switched_off(&config, agent)) {
            println!("{out}");
        }
    });
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    struct Rig {
        base: PathBuf,
        config: PathBuf,
        cache: PathBuf,
        home: PathBuf,
        work: PathBuf,
    }

    impl Rig {
        fn new(name: &str) -> Self {
            let base = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-offer-{name}-{}", std::process::id())));
            let _ = fs::remove_dir_all(&base);
            let r = Rig {
                config: base.join("config"),
                cache: base.join("cache"),
                home: base.join("agent-home"),
                work: base.join("work"),
                base,
            };
            for d in [&r.config, &r.cache, &r.home, &r.work] {
                fs::create_dir_all(d).unwrap();
            }
            r
        }

        fn at(&self) -> Places<'_> {
            Places { config: &self.config, cache: &self.cache, agent_home: &self.home, exe: "/opt/my tools/platoon", env_project: None }
        }

        /// Claude Code's hook input for `tool` with `tool_input`, in the work folder.
        fn claude(&self, session: &str, tool: &str, tool_input: Value) -> String {
            json!({"session_id": session, "cwd": self.work, "hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": tool_input})
                .to_string()
        }

        fn ask(&self, input: &str) -> Option<String> {
            hook(Agent::Claude, input, &self.at(), || false)
        }

        fn settings(&self, text: &str) {
            fs::write(crate::settings::path_in(&self.config), text).unwrap();
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    /// The reason of a refusal, checking the shape both agents read.
    fn reason(out: &str) -> String {
        let v: Value = serde_json::from_str(out).unwrap();
        let o = &v["hookSpecificOutput"];
        assert_eq!((o["hookEventName"].as_str(), o["permissionDecision"].as_str()), (Some("PreToolUse"), Some("deny")), "{out}");
        assert_eq!(v.as_object().unwrap().len(), 1, "nothing else: {out}");
        o["permissionDecisionReason"].as_str().unwrap().to_string()
    }

    #[test]
    fn only_a_session_platoon_started_that_is_no_tasks_offers() {
        assert!(offering(Some("1"), None));
        assert!(offering(Some("1"), Some("")));
        assert!(!offering(None, None), "a session started outside platoon");
        assert!(!offering(Some("0"), None));
        assert!(!offering(Some("1"), Some("pweb/3")), "a task's own session");
    }

    #[test]
    fn the_first_start_of_work_is_refused_once_and_everything_after_goes_through() {
        let r = Rig::new("once");
        let edit = r.claude("s-1", "Edit", json!({"file_path": r.work.join("src/main.rs"), "old_string": "a", "new_string": "b"}));
        // A shell command or a read is not the start of the work: nothing, and no mark.
        for (tool, input) in [("Bash", json!({"command": "cargo test"})), ("Read", json!({"file_path": r.work.join("a")}))] {
            assert_eq!(r.ask(&r.claude("s-1", tool, input)), None, "{tool}");
        }
        assert!(!mark_path(&r.cache, "s-1").exists());
        let why = reason(&r.ask(&edit).expect("the first edit is held back"));
        assert!(why.starts_with("platoon is asking, not an error."), "{why}");
        assert!(mark_path(&r.cache, "s-1").exists(), "the refusal is the mark");
        // Whatever the answer: the same step, and every other, goes through.
        assert_eq!(r.ask(&edit), None);
        assert_eq!(r.ask(&r.claude("s-1", "Agent", json!({"prompt": "build it"}))), None);
        // Another session is asked in its turn, by each kind of step that starts work.
        for (i, (tool, input)) in [
            ("Agent", json!({"description": "x", "prompt": "build it"})),
            ("Task", json!({"prompt": "build it"})),
            ("Write", json!({"file_path": r.work.join("new/file.rs"), "content": ""})),
            ("MultiEdit", json!({"file_path": "src/lib.rs", "edits": []})),
            ("NotebookEdit", json!({"notebook_path": r.work.join("a.ipynb")})),
            ("EnterWorktree", json!({"name": "feature"})),
        ]
        .into_iter()
        .enumerate()
        {
            let session = format!("other-{i}");
            assert!(r.ask(&r.claude(&session, tool, input.clone())).is_some(), "{tool} starts the work");
            assert_eq!(r.ask(&r.claude(&session, tool, input)), None, "{tool}: once");
        }
        // Input that isn't a hook's, or a session id that can't be a file name: nothing.
        for bad in ["", "{", "[]", r#"{"tool_name":"Edit"}"#, &r.claude("../x", "Agent", json!({}))] {
            assert_eq!(r.ask(bad), None, "{bad}");
        }
    }

    #[test]
    fn an_edit_outside_the_sessions_folder_is_not_the_work_even_through_a_link() {
        let r = Rig::new("folder");
        let elsewhere = r.base.join("notes-repo");
        fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, r.work.join("docs")).unwrap();
        fs::create_dir_all(r.work.join("src")).unwrap();
        let home = r.home.to_string_lossy().into_owned();
        let cwd = r.work.to_string_lossy().into_owned();
        let files = |f: &[&str]| f.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // Through the linked notes folder, in the agent's own folder (its memory, its plans), or anywhere else.
        for outside in [
            "docs/requirements.md".to_string(),
            r.work.join("docs/new.md").to_string_lossy().into_owned(),
            format!("{home}/projects/x/memory/MEMORY.md"),
            format!("{home}/plans/plan.md"),
            "../other/file.rs".to_string(),
            "/etc/hosts".to_string(),
        ] {
            assert!(!is_the_work(&cwd, &files(&[&outside]), &r.home), "{outside}");
            assert_eq!(r.ask(&r.claude("s", "Write", json!({"file_path": outside, "content": ""}))), None, "{outside}");
        }
        assert!(!mark_path(&r.cache, "s").exists(), "nothing asked, nothing marked");
        // In the folder: relative or absolute, a file that exists or one still to be written, through `.` and `..`.
        for inside in ["src/main.rs", "./new/deep/file.rs", "docs/../src/lib.rs"] {
            assert!(is_the_work(&cwd, &files(&[inside]), &r.home), "{inside}");
        }
        assert!(is_the_work(&cwd, &files(&[&r.work.join("src").to_string_lossy()]), &r.home));
        // A patch with one file inside is the work; no files at all (an agent, a worktree) is too.
        assert!(is_the_work(&cwd, &files(&["docs/a.md", "src/x.rs"]), &r.home));
        assert!(is_the_work(&cwd, &[], &r.home));
        // A session in the home folder: the agent's own folder below it still doesn't count.
        assert!(!is_the_work(&r.base.to_string_lossy(), &files(&[&format!("{home}/memory.md")]), &r.home));
        assert!(is_the_work(&r.base.to_string_lossy(), &files(&["work/src/main.rs"]), &r.home));
        // A folder that isn't known: anything but the agent's own.
        assert!(is_the_work("", &files(&["src/main.rs"]), &r.home));
        // The session's folder itself reached through a link: its files are its own.
        let link = r.base.join("work-link");
        std::os::unix::fs::symlink(&r.work, &link).unwrap();
        assert!(is_the_work(&link.to_string_lossy(), &files(&[&r.work.join("src/a.rs").to_string_lossy()]), &r.home));
        assert!(r.ask(&r.claude("s", "Edit", json!({"file_path": "src/main.rs"}))).is_some());
    }

    #[test]
    fn the_setting_off_or_the_program_switched_off_lets_everything_through() {
        let r = Rig::new("off");
        let agent = r.claude("s", "Agent", json!({"prompt": "go"}));
        r.settings(r#"{"offer_tasks": false}"#);
        assert_eq!(r.ask(&agent), None);
        assert!(!mark_path(&r.cache, "s").exists(), "not asked: asked later, once it is on again");
        r.settings(r#"{"offer_tasks": true}"#);
        assert_eq!(hook(Agent::Claude, &agent, &r.at(), || true), None, "Claude Code switched off");
        assert!(!mark_path(&r.cache, "s").exists());
        // The check of the switches is made only on the way to a refusal.
        let mut asked = false;
        assert!(
            hook(Agent::Claude, &agent, &r.at(), || {
                asked = true;
                false
            })
            .is_some()
        );
        assert!(asked);
        assert_eq!(hook(Agent::Claude, &agent, &r.at(), || panic!("not asked after the mark")), None);
    }

    #[test]
    fn two_steps_at_once_are_refused_once() {
        let r = Rig::new("race");
        let input = r.claude("shared", "Edit", json!({"file_path": "a.rs"}));
        let (config, cache, home) = (r.config.clone(), r.cache.clone(), r.home.clone());
        let threads: Vec<_> = (0..16)
            .map(|_| {
                let (config, cache, home, input) = (config.clone(), cache.clone(), home.clone(), input.clone());
                std::thread::spawn(move || {
                    let at = Places { config: &config, cache: &cache, agent_home: &home, exe: "platoon", env_project: None };
                    hook(Agent::Claude, &input, &at, || false).is_some()
                })
            })
            .collect();
        let refused = threads.into_iter().map(|t| t.join().unwrap()).filter(|&r| r).count();
        assert_eq!(refused, 1);
    }

    #[test]
    fn old_marks_go_when_another_session_is_asked() {
        let r = Rig::new("prune");
        let old = mark_path(&r.cache, "old-one");
        fs::create_dir_all(old.parent().unwrap()).unwrap();
        fs::write(&old, "").unwrap();
        let f = fs::File::options().write(true).open(&old).unwrap();
        f.set_modified(SystemTime::now() - KEEP - Duration::from_secs(60)).unwrap();
        let recent = mark_path(&r.cache, "recent");
        fs::write(&recent, "").unwrap();
        assert!(r.ask(&r.claude("new", "Agent", json!({}))).is_some());
        assert!(!old.exists() && recent.exists() && mark_path(&r.cache, "new").exists());
    }

    #[test]
    fn claude_is_told_to_ask_with_its_question_tool_and_how_to_make_the_tasks() {
        let r = Rig::new("claude-text");
        let mut p = Projects::open(&r.config);
        let id = p.create("Web shop").unwrap();
        p.assign("in-shop", Some(&id)).unwrap();
        let why = reason(&r.ask(&r.claude("in-shop", "Agent", json!({}))).unwrap());
        let cwd = crate::agents::claude::shell_word(&r.work.to_string_lossy());
        for want in [
            "platoon is asking, not an error. platoon (the app that started this session) held back this one step",
            "Ask with the AskUserQuestion tool: question \"Turn this into platoon tasks for a squad?\", options \"Make tasks\" and \"Do it here\".",
            "whatever they answer, the same step goes through when you take it again.",
            "If they choose \"Make tasks\": don't do the work here.",
            "ask with the AskUserQuestion tool \"Make these tasks?\", options \"Make them\" and \"Change them\", before making any",
            &format!("'/opt/my tools/platoon' task add --project {id} --folder {cwd} --squad <squad> --title '<short title>' <<'TASK'"),
            "## What it should do",
            "## Done when\n\n- [ ] <a check anyone can make>",
            "- feature: New or changed behaviour in code",
            "- bug-fix: ",
            "- docs: ",
            "how many tasks you made and that they are on platoon's Tasks tab. Don't start them",
            "If they choose \"Do it here\": tell them in one line that you won't ask again in this conversation, and that `t` on \
             this session's row in platoon makes tasks from it later. Then carry on with the step you were about to take.",
        ] {
            assert!(why.contains(want), "{want:?} missing from:\n{why}");
        }
        // A new session platoon started in a project, not placed in it yet: the project it was started in.
        let at = Places { env_project: Some(&id), ..r.at() };
        let why = reason(&hook(Agent::Claude, &r.claude("brand-new", "Agent", json!({})), &at, || false).unwrap());
        assert!(why.contains(&format!("task add --project {id} --folder")), "{why}");
        // In no project (an unknown one named is none): no --project, and a folder with a quote in it stays one word.
        let at = Places { env_project: Some("pnope"), ..r.at() };
        let input = json!({"session_id": "loose", "cwd": "/home/u/bob's shop", "tool_name": "Agent", "tool_input": {}}).to_string();
        let why = reason(&hook(Agent::Claude, &input, &at, || false).unwrap());
        assert!(why.contains("'/opt/my tools/platoon' task add --folder '/home/u/bob'\\''s shop' --squad"), "{why}");
        // A squad of the user's that can't be used is left out; their own good one is listed.
        fs::create_dir_all(crate::squads::dir(&r.config)).unwrap();
        fs::write(crate::squads::path(&r.config, "broken"), "no front matter").unwrap();
        fs::write(
            crate::squads::path(&r.config, "triage"),
            "---\nname: Triage\ndescription: Sort the bug reports.\nmodel: sonnet\nverdict_model: sonnet\nplan_first: false\n---\n1. Read them.\n",
        )
        .unwrap();
        let why = refusal(Agent::Claude, "platoon", None, None, &crate::squads::all(&r.config));
        assert!(why.contains("- triage: Sort the bug reports.") && !why.contains("- broken"), "{why}");
        assert!(why.contains("platoon task add --squad <squad>"), "no project, no folder: {why}");
    }

    #[test]
    fn codex_is_told_to_ask_in_its_reply_on_its_edit_tool_only() {
        let r = Rig::new("codex");
        let sample = include_str!("testdata/codex/hooks/PreToolUse-apply_patch.from-source.json");
        let cwd = r.work.to_string_lossy().into_owned();
        let patch = sample.replace("/work/repo", &cwd);
        let at = r.at();
        // Its shell command is not the work (Codex's hook is on `Bash` too).
        let shell = include_str!("testdata/codex/hooks/PreToolUse-Bash.from-source.json").replace("/work/repo", &cwd);
        assert_eq!(hook(Agent::Codex, &shell, &at, || false), None);
        let out = hook(Agent::Codex, &patch, &at, || false).expect("its first edit is held back");
        let why = reason(&out);
        for want in [
            "platoon is asking, not an error.",
            "Ask at the end of your reply, in plain words: \"Turn this into platoon tasks for a squad? Make tasks, or do it here?\" \
             Then stop and wait for their answer.",
            "ask at the end of your reply \"Make these tasks?\" and wait for their answer before making any.",
            &format!("'/opt/my tools/platoon' task add --folder {} --squad <squad>", crate::agents::claude::shell_word(&cwd)),
            "## Done when",
            "If they choose \"Do it here\"",
        ] {
            assert!(why.contains(want), "{want:?} missing from:\n{why}");
        }
        assert!(!why.contains("AskUserQuestion"), "Codex has no such tool: {why}");
        assert_eq!(hook(Agent::Codex, &patch, &at, || false), None, "once");
        // A patch only to files outside the folder (a linked notes folder) is not the work.
        let elsewhere = r.base.join("notes-repo");
        fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, r.work.join("docs")).unwrap();
        let notes = patch
            .replace("01a15181-dc6f-7b73-81cf-c041b14f6432", "01a15181-dc6f-7b73-81cf-00000000beef")
            .replace("notes.txt", "docs/notes.md");
        assert_eq!(hook(Agent::Codex, &notes, &at, || false), None);
    }

    #[test]
    fn claude_codes_hook_is_given_to_the_session_with_its_arguments() {
        use crate::agents::Start;
        let v: Value = serde_json::from_str(&crate::agents::claude::offer_settings_json("/opt/my tools/platoon")).unwrap();
        assert_eq!(
            v,
            json!({"hooks": {"PreToolUse": [{
                "matcher": "Agent|Task|Edit|Write|MultiEdit|NotebookEdit|EnterWorktree",
                "hooks": [{"type": "command", "command": "'/opt/my tools/platoon' hook offer-tasks"}]
            }]}})
        );
        let settings = crate::agents::claude::offer_settings_json("/p/platoon");
        let args = |start| Agent::Claude.offering_args(start, "/p/platoon");
        assert_eq!(args(Start::New(None)), ["--settings", settings.as_str()]);
        assert_eq!(args(Start::New(Some("hello"))), ["--settings", settings.as_str(), "hello"], "the first message stays last");
        assert_eq!(args(Start::Resume("abc")), ["--resume", "abc", "--settings", settings.as_str()]);
        assert_eq!(Agent::Claude.session_of_args(&args(Start::Resume("abc"))).as_deref(), Some("abc"));
        // Codex's hook is its hooks file's: its arguments are as ever.
        assert_eq!(Agent::Codex.offering_args(Start::Resume("abc"), "/p/platoon"), Agent::Codex.args(Start::Resume("abc")));
    }
}
