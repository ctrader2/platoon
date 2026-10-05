//! What `v` opens (a session's folders, a ticket's worktrees, a picked folder) and handing platoon's terminal to an
//! editor that draws in it (Helix, the user's own Neovim: `fileeditors.rs`) and back. The app decides when and in which
//! editor (`app/editor_glue.rs`); windowed editors start beside platoon (`fileeditors::launch_window`).
//!
//! The handoff: platoon leaves its screen (raw mode off, the alternate screen left), runs the editor in the foreground
//! on the same terminal and waits; nothing of platoon reads the keyboard or draws meanwhile (its loop is waiting here,
//! and its threads never touch the terminal). Ctrl+C typed at a program the editor started reaches that program, not
//! platoon. Then platoon takes the screen back and draws it again in full, where the user was. While it runs, the
//! folders it has open are marked (`mark_open`), so no platoon window removes a ticket's worktree from under it.

use crate::tickets::Ticket;
use serde_json::{Value, json};
use std::{
    fs, io,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

// ---- the folders --------------------------------------------------------------------------------------------------

/// One folder `v` opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
    /// Absolute.
    pub path: String,
    /// "folder", "worktree" or "work".
    pub kind: &'static str,
}

/// What `v` opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub roots: Vec<Root>,
}

impl Request {
    pub fn dirs(&self) -> Vec<String> {
        self.roots.iter().map(|r| r.path.clone()).collect()
    }
}

fn push_unique(out: &mut Vec<Root>, path: &str, kind: &'static str) {
    if !out.iter().any(|o| o.path == path) {
        out.push(Root { path: path.to_string(), kind });
    }
}

/// A conversation's folder and the folders added to it (`/add-dir`, home-abbreviated as the list shows them). Folders
/// that aren't there any more are left out.
pub fn session_roots(cwd: &str, extra: &[String], home: &str) -> Vec<Root> {
    let mut out = Vec::new();
    let all = std::iter::once(cwd.to_string()).chain(extra.iter().filter_map(|d| crate::dirs::expand(d, home)));
    for path in all {
        if Path::new(&path).is_dir() {
            push_unique(&mut out, &path, "folder");
        }
    }
    out
}

/// A folder picked by hand.
pub fn folder_roots(dir: &str) -> Vec<Root> {
    if !Path::new(dir).is_dir() {
        return Vec::new();
    }
    vec![Root { path: dir.to_string(), kind: "folder" }]
}

/// A ticket's worktrees, its folders that have no worktree (not in git, or not handed to a squad yet) as they are, and
/// its work folder when there is something in it. Worktrees that are gone are left out: the app brings them back first.
pub fn ticket_roots(t: &Ticket, work: &Path) -> Vec<Root> {
    let mut out = Vec::new();
    for f in &t.folders {
        match t.worktrees.iter().find(|w| w.folder == *f) {
            Some(w) if !w.is_gone() => push_unique(&mut out, &w.path, "worktree"),
            Some(_) => {}
            None if Path::new(f).is_dir() => push_unique(&mut out, f, "folder"),
            None => {}
        }
    }
    // A worktree whose folder was taken off the ticket later is still its work.
    for w in t.worktrees.iter().filter(|w| !t.folders.contains(&w.folder) && !w.is_gone()) {
        push_unique(&mut out, &w.path, "worktree");
    }
    let has_files = fs::read_dir(work).is_ok_and(|mut d| d.next().is_some());
    if has_files {
        push_unique(&mut out, &work.to_string_lossy(), "work");
    }
    out
}

// ---- starting it ----------------------------------------------------------------------------------------------------

/// Everything needed to run a terminal editor once.
#[derive(Debug)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, Option<String>)>,
    pub cwd: PathBuf,
}

/// While the editor runs, Ctrl+C, Ctrl+\ and Ctrl+Z typed at a program it started reach that program, not
/// platoon: platoon ignores them, and gives its old handling back afterwards.
struct Ignoring(Vec<(i32, libc::sigaction)>);

impl Ignoring {
    fn start() -> Ignoring {
        let mut old = Vec::new();
        for sig in [libc::SIGINT, libc::SIGQUIT, libc::SIGTSTP, libc::SIGTTOU, libc::SIGTTIN] {
            // SAFETY: plain sigaction calls with zeroed structs; the old handling is kept to be put back.
            unsafe {
                let mut ign: libc::sigaction = std::mem::zeroed();
                ign.sa_sigaction = libc::SIG_IGN;
                let mut was: libc::sigaction = std::mem::zeroed();
                if libc::sigaction(sig, &ign, &mut was) == 0 {
                    old.push((sig, was));
                }
            }
        }
        Ignoring(old)
    }
}

impl Drop for Ignoring {
    fn drop(&mut self) {
        for (sig, was) in &self.0 {
            // SAFETY: puts back exactly what `start` found.
            unsafe {
                libc::sigaction(*sig, was, std::ptr::null_mut());
            }
        }
    }
}

/// Run the editor in the foreground on platoon's terminal and wait until it has closed. The caller has handed the
/// terminal over (and takes it back after).
pub fn run(l: &Launch) -> io::Result<ExitStatus> {
    use std::os::unix::process::ExitStatusExt;
    let mut cmd = Command::new(&l.program);
    cmd.args(&l.args).current_dir(&l.cwd);
    for (k, v) in &l.env {
        match v {
            Some(v) => cmd.env(k, v),
            None => cmd.env_remove(k),
        };
    }
    // SAFETY: only async-signal-safe calls between fork and exec: the editor gets the usual signal handling back.
    unsafe {
        cmd.pre_exec(|| {
            for sig in [libc::SIGINT, libc::SIGQUIT, libc::SIGTSTP, libc::SIGTTOU, libc::SIGTTIN] {
                libc::signal(sig, libc::SIG_DFL);
            }
            Ok(())
        });
    }
    let _ignoring = Ignoring::start();
    let child = cmd.spawn().map_err(|e| io::Error::other(format!("couldn't start {}: {e}", l.program.display())))?;
    let pid = child.id() as libc::pid_t;
    loop {
        let mut status: libc::c_int = 0;
        // SAFETY: waits for our own child.
        let r = unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) };
        if r < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        if libc::WIFSTOPPED(status) {
            // It stopped itself (a suspend): platoon is waiting behind it and can't be the shell that resumes it, so
            // it carries on.
            // SAFETY: our own child.
            unsafe { libc::kill(pid, libc::SIGCONT) };
            continue;
        }
        return Ok(ExitStatus::from_raw(status));
    }
}

// ---- which folders an editor has open ---------------------------------------------------------------------------

/// Where platoon notes the folders an editor has open, one file per platoon (by pid), so that no platoon window
/// removes a ticket's worktree from under an editor another window opened.
fn marks_dir(cache: &Path) -> PathBuf {
    cache.join("editor-open")
}

/// Note that an editor this platoon started has `dirs` open.
pub fn mark_open(cache: &Path, pid: u32, dirs: &[String]) -> io::Result<PathBuf> {
    let p = marks_dir(cache).join(format!("{pid}.json"));
    crate::projects::write_atomic(&p, &json!({"pid": pid, "dirs": dirs}).to_string())?;
    Ok(p)
}

pub fn unmark(path: &Path) {
    let _ = fs::remove_file(path);
}

/// The folders editors have open now, from every platoon whose process is still there (a mark left by one that is
/// gone is tidied away).
pub fn open_now(cache: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(marks_dir(cache)) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        let Some(v) = fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { continue };
        let pid = v.get("pid").and_then(Value::as_u64).unwrap_or(0) as i32;
        // SAFETY: signal 0 only asks whether the process is there.
        if pid <= 0 || unsafe { libc::kill(pid, 0) } != 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            let _ = fs::remove_file(&p);
            continue;
        }
        out.extend(v.get("dirs").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(PathBuf::from));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tickets::Worktree;
    use std::process::Stdio;

    fn tmp(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-ed-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn repo(at: &Path) -> String {
        fs::create_dir_all(at).unwrap();
        fs::write(at.join("README"), "hi\n").unwrap();
        git(at, &["init", "-q"]);
        git(at, &["add", "."]);
        git(at, &["commit", "-q", "-m", "first"]);
        git(at, &["rev-parse", "HEAD"])
    }

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn a_session_opens_its_folder_and_the_folders_added_to_it() {
        let d = tmp("session");
        let home = d.join("home");
        let main = home.join("repos/app");
        repo(&main);
        let docs = home.join("notes/docs");
        fs::create_dir_all(&docs).unwrap();
        let other = d.join("srv/app");
        fs::create_dir_all(&other).unwrap();
        let extra = vec!["~/notes/docs".to_string(), s(&other), "~/gone".to_string(), "~/repos/app".to_string()];
        let roots = session_roots(&s(&main), &extra, &s(&home));
        let paths: Vec<&str> = roots.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, vec![s(&main), s(&docs), s(&other)], "home expanded, missing and repeated folders left out");
        assert!(roots.iter().all(|r| r.kind == "folder"));
    }

    #[test]
    fn a_folder_picked_by_hand_is_opened_as_it_is() {
        let d = tmp("folder");
        assert_eq!(folder_roots(&s(&d.join("nope"))), vec![]);
        let r = folder_roots(&s(&d));
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].path.as_str(), r[0].kind), (s(&d).as_str(), "folder"));
    }

    fn ticket(number: u32, folders: Vec<String>, worktrees: Vec<Worktree>) -> Ticket {
        Ticket {
            number,
            title: "Basket".into(),
            file: String::new(),
            state: crate::tickets::State::Working,
            folders,
            blocked_by: vec![],
            created: 0,
            updated: 0,
            leaders: vec![],
            worktrees,
            reason: String::new(),
            go_ahead: false,
            handover: None,
            permission: false,
            busy: None,
            queued: None,
            from: None,
            aside: None,
            check_due: None,
            checks: Vec::new(),
            tests_failing: false,
            check_failing: None,
            publish_wanted: false,
            plan_ready: false,
            autonomy: None,
            works_on: vec![],
            old_number: None,
        }
    }

    #[test]
    fn a_ticket_opens_its_worktrees_its_plain_folders_and_its_work_folder() {
        let d = tmp("ticket");
        let shop = d.join("repos/shop");
        let base = repo(&shop);
        let plain = d.join("plain");
        fs::create_dir_all(&plain).unwrap();
        let folders = vec![s(&shop), s(&plain)];
        let made = crate::squads::prepare_worktrees("git", &folders, 4, &[]).unwrap();
        assert_eq!(made.len(), 1, "only the git folder gets a worktree");
        let w = made[0].clone();
        assert_eq!(w.base.as_deref(), Some(base.as_str()));
        fs::write(Path::new(&w.path).join("basket.txt"), "b\n").unwrap();
        git(Path::new(&w.path), &["add", "."]);
        git(Path::new(&w.path), &["commit", "-q", "-m", "basket"]);
        let work = d.join("04-work");
        fs::create_dir_all(&work).unwrap();
        let t = ticket(4, folders.clone(), made.clone());
        let roots = ticket_roots(&t, &work);
        assert_eq!(
            roots.iter().map(|r| r.path.clone()).collect::<Vec<_>>(),
            vec![w.path.clone(), s(&plain)],
            "an empty work folder is left out"
        );
        assert_eq!((roots[0].kind, roots[1].kind), ("worktree", "folder"));

        fs::write(work.join("notes.md"), "x").unwrap();
        let roots = ticket_roots(&t, &work);
        assert_eq!(roots.last().map(|r| (r.path.clone(), r.kind)), Some((s(&work), "work")));

        // A removed worktree is left out (the app brings it back before opening).
        let mut gone = w.clone();
        gone.removed = Some(1);
        let roots = ticket_roots(&ticket(4, folders.clone(), vec![gone]), &work);
        assert!(roots.iter().all(|r| r.kind != "worktree"), "{roots:?}");
        assert!(roots.iter().all(|r| r.path != s(&shop)), "nor is the repository itself opened in its place");
    }

    #[test]
    fn a_ticket_not_handed_to_a_squad_yet_opens_its_folders() {
        let d = tmp("new-ticket");
        let shop = d.join("shop");
        repo(&shop);
        let roots = ticket_roots(&ticket(2, vec![s(&shop)], vec![]), &d.join("none"));
        assert_eq!(roots, vec![Root { path: s(&shop), kind: "folder" }]);
    }

    #[test]
    fn open_folders_are_known_while_their_platoon_is_there() {
        let d = tmp("marks");
        let mine = mark_open(&d, std::process::id(), &["/a/task-1".into()]).unwrap();
        // A platoon that is gone (a pid no process has): its mark is tidied away.
        let gone = mark_open(&d, 0x3fff_fff0, &["/b".into()]).unwrap();
        assert_eq!(open_now(&d), vec![PathBuf::from("/a/task-1")]);
        assert!(!gone.exists());
        unmark(&mine);
        assert!(open_now(&d).is_empty());
    }
}
