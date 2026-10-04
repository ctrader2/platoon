//! What git can say about a folder, for the hand-off document: its branch and how it stands against its upstream,
//! what is not committed yet, the last commit, and what was committed since a moment (the session's start); and for a
//! task's hand-over: the repository's default branch (`default_branch`), whether a branch exists and whether the
//! checkout is clean. Read-only: only `rev-parse`, `symbolic-ref`, `status`, `rev-list` and `log` are ever run (never
//! `fetch`: the upstream is as git last saw it), with `GIT_OPTIONAL_LOCKS=0` so not even git's own index
//! refresh is written, and every command shares one short time budget.
//!
//! Nothing here is ever guessed: no `git`, a folder that is not a repository, or a git that fails gives `None`,
//! and a single command that fails leaves just its own fact out.
//!
//! It can block (a huge repository, a network file system), so it never runs on the UI thread: the hand-off runs
//! it on the thread that then makes the Claude call.

use std::{
    io::{self, Read},
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// All the git commands of one hand-off together may take this long; whatever has not answered by then is left out.
pub const BUDGET: Duration = Duration::from_secs(5);
/// The most of one command's output that is read (a status of a huge tree is cut, not held).
const MAX_OUT: u64 = 1 << 20;
/// The most commits looked at for "committed since the session started".
const MAX_COMMITS: usize = 500;
/// The most file names kept from one list.
pub const MAX_FILES: usize = 500;
/// The longest path kept; a longer one keeps its end.
pub const PATH_MAX: usize = 120;
/// The longest commit subject kept.
pub const SUBJECT_MAX: usize = 150;

/// What kind of change a file not committed yet has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Changed,
    /// New and added to git, not committed.
    Added,
    Deleted,
    /// Renamed (or copied) from this path.
    Renamed(String),
    /// New, and not added to git at all.
    Untracked,
    /// Both sides of a merge changed it.
    Conflict,
}

impl Kind {
    /// In plain words, for the list of files.
    pub fn words(&self) -> String {
        match self {
            Kind::Changed => "changed".into(),
            Kind::Added => "new".into(),
            Kind::Deleted => "deleted".into(),
            Kind::Renamed(from) => format!("renamed from {from}"),
            Kind::Untracked => "new, not yet added to git".into(),
            Kind::Conflict => "merge conflict".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    pub kind: Kind,
}

/// What was committed since a moment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Since {
    pub commits: usize,
    /// The files those commits touched, newest first, each once.
    pub files: Vec<String>,
    /// Those commits' short ids and subjects (cleaned and cut), newest first.
    pub subjects: Vec<(String, String)>,
}

/// How the branch stands against the branch it tracks, as git last saw it (nothing is fetched).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Upstream {
    /// Its short name, e.g. `origin/main`.
    pub name: String,
    /// Commits here that the upstream doesn't have (not pushed yet).
    pub ahead: usize,
    /// Commits on the upstream that aren't here (not pulled yet).
    pub behind: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitFacts {
    /// The repository's top folder. Paths below are relative to it.
    pub top: String,
    /// None on a detached HEAD, or when it couldn't be read.
    pub branch: Option<String>,
    /// None with no branch, no upstream, or when it couldn't be read.
    pub upstream: Option<Upstream>,
    /// What is not committed yet. None when it couldn't be read - never taken to mean "nothing".
    pub changes: Option<Vec<Change>>,
    /// The last commit's short id and subject. None in a repository with no commits yet.
    pub last: Option<(String, String)>,
    /// Committed since the session started. None when there is no start time, or it couldn't be read.
    pub since: Option<Since>,
}

impl GitFacts {
    /// Git said which files changed, one way or the other, so Claude is not asked for its own list.
    pub fn knows_files(&self) -> bool {
        self.changes.is_some() || self.since.is_some()
    }
}

/// A path as it may be shown and sent: no control characters (a file name can hold an escape sequence), and not
/// endlessly long.
pub fn clean_path(p: &str) -> String {
    let p: String = p.chars().map(|c| if c.is_control() { '?' } else { c }).collect();
    let n = p.chars().count();
    if n <= PATH_MAX {
        return p;
    }
    let tail: String = p.chars().skip(n - (PATH_MAX - 1)).collect();
    format!("…{tail}")
}

/// Run one git command in `dir`, read-only, by `deadline`. Its output when it succeeded, else None.
pub(crate) fn run(program: &str, dir: &Path, args: &[&str], deadline: Instant) -> Option<String> {
    if Instant::now() >= deadline {
        return None;
    }
    let mut cmd = Command::new(program);
    cmd.arg("-C")
        .arg(dir)
        // The user's own settings must not change what is read (colors, signatures, paths relative to a subfolder,
        // quoted non-ASCII names).
        .args(["-c", "color.ui=false", "-c", "log.showSignature=false", "-c", "core.quotePath=false", "-c", "diff.relative=false"])
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // A script (a stand-in in the tests) that another thread has only just written can be "busy" for a moment.
    let mut child = crate::ai::spawn_retrying(&mut cmd).ok()?;
    let mut out = child.stdout.take()?;
    // Read on a thread of its own, so a big output can't fill the pipe and stall git while we wait for it.
    let reader = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = (&mut out).take(MAX_OUT).read_to_end(&mut buf);
        let _ = io::copy(&mut out, &mut io::sink());
        buf
    });
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let buf = reader.join().ok()?;
                return status.success().then(|| String::from_utf8_lossy(&buf).into_owned());
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            _ => {
                // Out of time: this is our own child, and only it is ended.
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// What git says about `dir`, within `budget`. None when git isn't there, `dir` is no repository or git fails.
/// `since_ms` is the session's start: what was committed after it is listed.
pub fn gather(program: &str, dir: &Path, since_ms: Option<u64>, budget: Duration) -> Option<GitFacts> {
    let deadline = Instant::now() + budget;
    let top = run(program, dir, &["rev-parse", "--show-toplevel"], deadline)?.trim().to_string();
    if top.is_empty() {
        return None;
    }
    let branch = run(program, dir, &["symbolic-ref", "--quiet", "--short", "HEAD"], deadline)
        .map(|b| clean_path(b.trim()))
        .filter(|b| !b.is_empty());
    // Only a branch can have an upstream; a detached HEAD, or a branch that tracks nothing, leaves it out.
    let upstream = branch.as_ref().and_then(|_| {
        let name = run(program, dir, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"], deadline)?;
        let name = clean_path(name.trim());
        if name.is_empty() {
            return None;
        }
        let counts = run(program, dir, &["rev-list", "--left-right", "--count", "@{upstream}...HEAD"], deadline)?;
        let (behind, ahead) = parse_counts(&counts)?;
        Some(Upstream { name, ahead, behind })
    });
    let changes = run(program, dir, &["status", "--porcelain=v1", "-z", "--untracked-files=normal"], deadline).map(|o| parse_status(&o));
    let last = run(program, dir, &["log", "-1", "--format=%h%x00%s"], deadline).and_then(|o| parse_last(&o));
    let since = since_ms.and_then(|ms| {
        let since = format!("--since=@{}", ms / 1000);
        let max = format!("--max-count={MAX_COMMITS}");
        run(program, dir, &["log", &since, &max, "--format=@@commit %h%x00%s", "--name-only"], deadline).map(|o| parse_log(&o))
    });
    Some(GitFacts { top, branch, upstream, changes, last, since })
}

/// The branch a repository's work is meant for, found read-only: the branch `origin/HEAD` points at, else `main` or
/// `master` when one of them exists, else the branch checked out. None when `dir` is no repository, git can't say in
/// time, or none of those is there (a detached HEAD in a repository with neither).
pub fn default_branch(program: &str, dir: &Path, deadline: Instant) -> Option<String> {
    if let Some(head) = run(program, dir, &["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"], deadline) {
        let head = head.trim();
        let name = head.strip_prefix("origin/").unwrap_or(head);
        if !name.is_empty() {
            return Some(clean_path(name));
        }
    }
    for name in ["main", "master"] {
        if branch_exists(program, dir, name, deadline) {
            return Some(name.into());
        }
    }
    run(program, dir, &["symbolic-ref", "--quiet", "--short", "HEAD"], deadline).map(|b| clean_path(b.trim())).filter(|b| !b.is_empty())
}

/// Is `name` a branch (or anything else git finds a commit by) in `dir`'s repository? Checked with
/// `git rev-parse --verify <name>^{commit}`; a name that could be taken for an option, or an empty one, is not.
pub fn branch_exists(program: &str, dir: &Path, name: &str, deadline: Instant) -> bool {
    let name = name.trim();
    if name.is_empty() || name.starts_with('-') || name.chars().any(char::is_whitespace) {
        return false;
    }
    run(program, dir, &["rev-parse", "--verify", "--quiet", &format!("{name}^{{commit}}")], deadline).is_some_and(|o| !o.trim().is_empty())
}

/// Has the checkout in `dir` nothing uncommitted (no changed, staged or new files git doesn't ignore)? None when git
/// can't say in time: never taken to mean clean.
pub fn is_clean(program: &str, dir: &Path, deadline: Instant) -> Option<bool> {
    run(program, dir, &["status", "--porcelain", "--untracked-files=normal"], deadline).map(|o| o.trim().is_empty())
}

/// `git status --porcelain=v1 -z`: `XY path\0`, and for a rename or copy `XY new\0old\0`.
pub fn parse_status(out: &str) -> Vec<Change> {
    let mut changes = Vec::new();
    let mut parts = out.split('\0');
    while let Some(entry) = parts.next() {
        let (Some(code), Some(path)) = (entry.get(..2), entry.get(3..)) else { continue };
        if path.is_empty() || changes.len() >= MAX_FILES {
            continue;
        }
        let (x, y) = (code.as_bytes()[0], code.as_bytes()[1]);
        let kind = if code == "??" {
            Kind::Untracked
        } else if x == b'U' || y == b'U' || code == "AA" || code == "DD" {
            Kind::Conflict
        } else if x == b'R' || x == b'C' {
            Kind::Renamed(clean_path(parts.next().unwrap_or("")))
        } else if x == b'D' || y == b'D' {
            Kind::Deleted
        } else if x == b'A' {
            Kind::Added
        } else {
            Kind::Changed
        };
        changes.push(Change { path: clean_path(path), kind });
    }
    changes
}

/// `git log -1 --format=%h%x00%s`.
fn parse_last(out: &str) -> Option<(String, String)> {
    let (id, subject) = out.trim_end_matches('\n').split_once('\0')?;
    let id = id.trim();
    (!id.is_empty()).then(|| (clean_path(id), crate::smart::clean_text(subject, SUBJECT_MAX)))
}

/// `git rev-list --left-right --count @{upstream}...HEAD`: "behind<tab>ahead".
fn parse_counts(out: &str) -> Option<(usize, usize)> {
    let mut it = out.split_whitespace().map(str::parse::<usize>);
    match (it.next(), it.next(), it.next()) {
        (Some(Ok(behind)), Some(Ok(ahead)), None) => Some((behind, ahead)),
        _ => None,
    }
}

/// `git log --format='@@commit %h%x00%s' --name-only`: a marker line per commit with its id and subject, then the
/// files it touched.
pub fn parse_log(out: &str) -> Since {
    let mut s = Since::default();
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("@@commit ") {
            s.commits += 1;
            let (id, subject) = rest.split_once('\0').unwrap_or((rest, ""));
            if s.subjects.len() < MAX_COMMITS {
                s.subjects.push((clean_path(id.trim()), crate::smart::clean_text(subject, SUBJECT_MAX)));
            }
        } else if !line.trim().is_empty() && s.files.len() < MAX_FILES {
            let path = clean_path(line);
            if !s.files.contains(&path) {
                s.files.push(path);
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

    /// A throwaway folder; git in it never reads the user's own configuration.
    struct Repo {
        dir: PathBuf,
    }

    impl Repo {
        fn new(name: &str) -> Repo {
            let dir = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-git-{name}-{}", std::process::id())));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Repo { dir }
        }

        /// A git command that writes (only ever in this temp folder), with a fixed author and no user config.
        fn git(&self, args: &[&str]) {
            self.git_at(args, None);
        }

        fn git_at(&self, args: &[&str], date: Option<&str>) {
            let mut c = Command::new("git");
            c.arg("-C").arg(&self.dir).args(args);
            c.env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_NOSYSTEM", "1");
            c.env("GIT_AUTHOR_NAME", "Test").env("GIT_AUTHOR_EMAIL", "test@example.com");
            c.env("GIT_COMMITTER_NAME", "Test").env("GIT_COMMITTER_EMAIL", "test@example.com");
            if let Some(d) = date {
                c.env("GIT_AUTHOR_DATE", d).env("GIT_COMMITTER_DATE", d);
            }
            let out = c.stdout(Stdio::null()).stderr(Stdio::piped()).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        }

        fn write(&self, path: &str, text: &str) {
            let p = self.dir.join(path);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }

        fn init(&self) {
            self.git(&["init", "-q", "-b", "main"]);
        }

        fn commit(&self, msg: &str, date: Option<&str>) {
            self.git(&["add", "-A"]);
            self.git_at(&["commit", "-q", "-m", msg], date);
        }

        fn facts(&self, since_ms: Option<u64>) -> Option<GitFacts> {
            gather("git", &self.dir, since_ms, BUDGET)
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn a_clean_repository_says_its_branch_and_last_commit_and_that_nothing_is_uncommitted() {
        let r = Repo::new("clean");
        r.init();
        r.write("a.txt", "one");
        r.commit("First commit", None);
        let f = r.facts(None).expect("a repository");
        assert_eq!(f.branch.as_deref(), Some("main"));
        assert_eq!(f.changes, Some(vec![]), "known to be clean, not unknown");
        let (id, subject) = f.last.clone().unwrap();
        assert!(id.len() >= 7 && id.chars().all(|c| c.is_ascii_hexdigit()), "{id}");
        assert_eq!(subject, "First commit");
        assert_eq!(f.since, None, "no start time, so nothing is said about it");
        assert_eq!(fs::canonicalize(&f.top).unwrap(), fs::canonicalize(&r.dir).unwrap());
        // Reading changed nothing in the repository: no lock left, the index as it was.
        assert!(!r.dir.join(".git/index.lock").exists());
    }

    #[test]
    fn changed_new_deleted_renamed_and_untracked_files_are_each_told_apart() {
        let r = Repo::new("dirty");
        r.init();
        r.write("keep.txt", "k");
        r.write("gone.txt", "g");
        r.write("old name.txt", "o");
        r.write("src/lib.rs", "fn a() {}");
        r.commit("Start", None);
        r.write("src/lib.rs", "fn b() {}");
        fs::remove_file(r.dir.join("gone.txt")).unwrap();
        r.git(&["mv", "old name.txt", "new name.txt"]);
        r.write("staged.txt", "s");
        r.git(&["add", "staged.txt"]);
        r.write("notes/über.md", "u");
        let f = r.facts(None).unwrap();
        let mut got: Vec<(String, String)> = f.changes.unwrap().into_iter().map(|c| (c.path, c.kind.words())).collect();
        got.sort();
        assert_eq!(
            got,
            [
                ("gone.txt".to_string(), "deleted".to_string()),
                ("new name.txt".into(), "renamed from old name.txt".into()),
                ("notes/".into(), "new, not yet added to git".into()),
                ("src/lib.rs".into(), "changed".into()),
                ("staged.txt".into(), "new".into()),
            ]
        );
    }

    #[test]
    fn what_was_committed_after_the_session_started_is_listed_and_what_came_before_is_not() {
        let r = Repo::new("since");
        r.init();
        r.write("before.txt", "b");
        r.commit("Before the session", Some("2026-09-20T10:00:00Z"));
        r.write("during.txt", "d");
        r.write("before.txt", "b2");
        r.commit("During the session", Some("2026-09-24T12:00:00Z"));
        r.write("later.txt", "l");
        r.commit("Also during", Some("2026-09-24T13:00:00Z"));
        let start = crate::claude::parse_iso_ms("2026-09-24T11:00:00.000Z").unwrap();
        let f = r.facts(Some(start)).unwrap();
        let since = f.since.unwrap();
        assert_eq!(since.commits, 2);
        assert_eq!(since.files, ["later.txt", "before.txt", "during.txt"], "newest commit first (git sorts within one), each once");
        assert_eq!(f.last.unwrap().1, "Also during");
        // A start after every commit: none.
        let late = crate::claude::parse_iso_ms("2026-09-25T00:00:00.000Z").unwrap();
        assert_eq!(r.facts(Some(late)).unwrap().since, Some(Since::default()));
    }

    #[test]
    fn a_repository_with_no_commits_yet_leaves_the_last_commit_out() {
        let r = Repo::new("empty");
        r.init();
        r.write("first.txt", "f");
        let f = r.facts(Some(1)).unwrap();
        assert_eq!(f.branch.as_deref(), Some("main"));
        assert_eq!(f.last, None);
        assert_eq!(f.changes.unwrap(), [Change { path: "first.txt".into(), kind: Kind::Untracked }]);
        assert_eq!(f.since, None, "git log fails with no commits, so it is left out rather than said to be none");
    }

    #[test]
    fn a_detached_head_has_no_branch_but_everything_else_is_there() {
        let r = Repo::new("detached");
        r.init();
        r.write("a", "1");
        r.commit("One", None);
        r.write("a", "2");
        r.commit("Two", None);
        r.git(&["checkout", "-q", "HEAD~1"]);
        let f = r.facts(None).unwrap();
        assert_eq!(f.branch, None);
        assert_eq!(f.last.unwrap().1, "One");
    }

    #[test]
    fn a_folder_that_is_no_repository_or_a_git_that_is_missing_or_failing_gives_nothing() {
        let r = Repo::new("plain");
        r.write("a.txt", "a");
        // GIT_CEILING_DIRECTORIES isn't needed: the temp dir itself is not inside a repository.
        assert_eq!(r.facts(None), None);
        assert_eq!(gather("/nonexistent/git-program", &r.dir, None, BUDGET), None);
        assert_eq!(gather("git", &r.dir.join("not-there"), None, BUDGET), None);
        // A git that fails every time.
        let bad = r.dir.join("bad-git");
        fs::write(&bad, "#!/bin/sh\necho 'fatal: broken' >&2\nexit 128\n").unwrap();
        fs::set_permissions(&bad, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(gather(bad.to_str().unwrap(), &r.dir, None, BUDGET), None);
    }

    #[test]
    fn a_git_that_answers_the_first_question_and_then_fails_leaves_each_later_fact_out() {
        let r = Repo::new("half");
        let half = r.dir.join("half-git");
        // Only `rev-parse` works.
        fs::write(&half, "#!/bin/sh\nfor a in \"$@\"; do [ \"$a\" = rev-parse ] && { echo /some/repo; exit 0; }; done\nexit 1\n").unwrap();
        fs::set_permissions(&half, fs::Permissions::from_mode(0o755)).unwrap();
        let f = gather(half.to_str().unwrap(), &r.dir, Some(1_000), BUDGET).unwrap();
        assert_eq!(f, GitFacts { top: "/some/repo".into(), ..GitFacts::default() });
        assert!(!f.knows_files(), "unknown is not the same as clean");
    }

    #[test]
    fn a_git_that_hangs_is_given_up_on_in_time_and_its_process_is_gone() {
        let r = Repo::new("slow");
        let slow = r.dir.join("slow-git");
        let pidfile = r.dir.join("pid");
        fs::write(&slow, format!("#!/bin/sh\necho $$ > '{}'\nexec sleep 30\n", pidfile.display())).unwrap();
        fs::set_permissions(&slow, fs::Permissions::from_mode(0o755)).unwrap();
        let t = Instant::now();
        assert_eq!(gather(slow.to_str().unwrap(), &r.dir, None, Duration::from_millis(400)), None);
        assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
        let pid: u32 = fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
        assert!(!crate::testkit::alive(pid), "the hung git was ended");
    }

    /// A clone of a bare "remote" in the same temp folder, with `main` tracking `origin/main`. Everything stays in
    /// the temp folder: the remote is a path, and nothing is ever fetched from anywhere else.
    fn cloned(name: &str) -> (Repo, Repo) {
        let remote = Repo::new(&format!("{name}-remote"));
        let seed = Repo::new(&format!("{name}-seed"));
        seed.init();
        seed.write("a.txt", "a");
        seed.commit("Seed", None);
        remote.git(&["init", "-q", "--bare", "-b", "main"]);
        seed.git(&["push", "-q", remote.dir.to_str().unwrap(), "main"]);
        let work = Repo::new(&format!("{name}-work"));
        let _ = fs::remove_dir_all(&work.dir);
        let out = Command::new("git")
            .args(["clone", "-q", remote.dir.to_str().unwrap(), work.dir.to_str().unwrap()])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        drop(seed);
        (remote, work)
    }

    /// Put a commit on the remote from a second clone, and fetch it into `work` (so `work` is behind by one).
    fn commit_on_remote(remote: &Repo, work: &Repo, name: &str, msg: &str) {
        let other = Repo::new(name);
        let _ = fs::remove_dir_all(&other.dir);
        let out = Command::new("git")
            .args(["clone", "-q", remote.dir.to_str().unwrap(), other.dir.to_str().unwrap()])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        other.write(&format!("{msg}.txt"), msg);
        other.commit(msg, None);
        other.git(&["push", "-q", "origin", "main"]);
        work.git(&["fetch", "-q", "origin"]);
    }

    #[test]
    fn a_branch_says_how_far_it_is_ahead_of_and_behind_its_upstream_and_nothing_when_it_has_none() {
        let (remote, work) = cloned("upstream");
        // Up to date.
        let up = work.facts(None).unwrap().upstream.expect("main tracks origin/main");
        assert_eq!(up, Upstream { name: "origin/main".into(), ahead: 0, behind: 0 });
        // Two commits not pushed yet.
        work.write("b.txt", "b");
        work.commit("Local one", None);
        work.write("c.txt", "c");
        work.commit("Local two", None);
        let up = work.facts(None).unwrap().upstream.unwrap();
        assert_eq!((up.ahead, up.behind), (2, 0));
        // And one on the remote not pulled yet: both.
        commit_on_remote(&remote, &work, "upstream-other", "Remote one");
        let up = work.facts(None).unwrap().upstream.unwrap();
        assert_eq!((up.ahead, up.behind), (2, 1));
        // Only behind.
        work.git(&["reset", "-q", "--hard", "origin/main~1"]);
        let up = work.facts(None).unwrap().upstream.unwrap();
        assert_eq!((up.ahead, up.behind), (0, 1));
        // A branch that tracks nothing, and a detached HEAD: left out, and the rest is still there.
        work.git(&["checkout", "-q", "-b", "loose"]);
        let f = work.facts(None).unwrap();
        assert_eq!((f.branch.as_deref(), f.upstream), (Some("loose"), None));
        work.git(&["checkout", "-q", "--detach"]);
        let f = work.facts(None).unwrap();
        assert_eq!((f.branch, f.upstream), (None, None));
        assert!(f.last.is_some());
        // Reading wrote nothing: no lock left.
        assert!(!work.dir.join(".git/index.lock").exists());
    }

    #[test]
    fn the_commits_made_during_the_session_come_with_their_subjects_newest_first_and_cleaned() {
        let r = Repo::new("subjects");
        r.init();
        r.write("a", "0");
        r.commit("Before", Some("2026-09-20T10:00:00Z"));
        for i in 1..=12 {
            r.write("a", &i.to_string());
            r.commit(&format!("Change number {i}"), Some(&format!("2026-09-24T12:{i:02}:00Z")));
        }
        r.write("a", "evil");
        r.commit(&format!("Evil \u{1b}[2J subject {}", "x".repeat(400)), Some("2026-09-24T13:00:00Z"));
        let start = crate::claude::parse_iso_ms("2026-09-24T11:00:00.000Z").unwrap();
        let since = r.facts(Some(start)).unwrap().since.unwrap();
        assert_eq!(since.commits, 13);
        assert_eq!(since.subjects.len(), 13, "all of them are kept here; the document decides how many to show");
        let subjects: Vec<&str> = since.subjects.iter().map(|(_, s)| s.as_str()).collect();
        assert!(subjects[0].starts_with("Evil subject xxx"), "cleaned: {:?}", subjects[0]);
        assert!(!subjects[0].contains('\u{1b}') && subjects[0].chars().count() <= SUBJECT_MAX, "{:?}", subjects[0]);
        assert_eq!(subjects[1..4], ["Change number 12", "Change number 11", "Change number 10"]);
        assert_eq!(*subjects.last().unwrap(), "Change number 1");
        assert!(!subjects.contains(&"Before"));
        assert!(since.subjects.iter().all(|(id, _)| id.len() >= 7 && id.chars().all(|c| c.is_ascii_hexdigit())));
        assert_eq!(since.files, ["a"]);
    }

    #[test]
    fn file_names_can_never_carry_a_control_character_or_run_on_forever() {
        let s = parse_status(" M evil\u{1b}[2J.txt\0?? ok.txt\0");
        assert_eq!(s[0].path, "evil?[2J.txt");
        assert_eq!(s[1], Change { path: "ok.txt".into(), kind: Kind::Untracked });
        let long = "d/".repeat(200) + "file.rs";
        let p = clean_path(&long);
        assert_eq!(p.chars().count(), PATH_MAX);
        assert!(p.starts_with('…') && p.ends_with("file.rs"));
        // Rubbish in the status output is skipped, not misread.
        assert_eq!(parse_status("x\0\0MM\0"), []);
        let log = parse_log("@@commit abc\0First\n\na.rs\nb.rs\n@@commit def\n\na.rs\n");
        assert_eq!((log.commits, log.files), (2, vec!["a.rs".to_string(), "b.rs".into()]));
        assert_eq!(
            log.subjects,
            [("abc".to_string(), "First".to_string()), ("def".into(), String::new())],
            "a line with no subject still counts"
        );
        // What `rev-list --count` says, and rubbish.
        assert_eq!(parse_counts("1\t3\n"), Some((1, 3)));
        for bad in ["", "1", "a\tb", "1\t2\t3", "-1\t2"] {
            assert_eq!(parse_counts(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_default_branch_is_origin_head_else_main_or_master_else_the_one_checked_out() {
        let d = Repo::new("default-branch").dir.clone();
        let run = |dir: &Path, args: &[&str]| {
            let out = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        let soon = || Instant::now() + Duration::from_secs(10);
        // Only `trunk`: the branch checked out.
        let a = d.join("a");
        fs::create_dir_all(&a).unwrap();
        run(&a, &["init", "-q", "--initial-branch=trunk"]);
        fs::write(a.join("f"), "1").unwrap();
        run(&a, &["add", "."]);
        run(&a, &["commit", "-q", "-m", "one"]);
        assert_eq!(default_branch("git", &a, soon()).as_deref(), Some("trunk"));
        assert!(branch_exists("git", &a, "trunk", soon()) && !branch_exists("git", &a, "nope", soon()));
        assert!(
            !branch_exists("git", &a, "", soon())
                && !branch_exists("git", &a, "--help", soon())
                && !branch_exists("git", &a, "a b", soon())
        );
        assert_eq!(is_clean("git", &a, soon()), Some(true));
        fs::write(a.join("g"), "new").unwrap();
        assert_eq!(is_clean("git", &a, soon()), Some(false), "a new file git doesn't ignore");
        // With a `main`: main, even from another branch.
        run(&a, &["branch", "main"]);
        assert_eq!(default_branch("git", &a, soon()).as_deref(), Some("main"));
        // A clone: what origin/HEAD points at wins, whatever is checked out.
        let b = d.join("b");
        run(&d, &["clone", "-q", "--branch", "trunk", a.to_str().unwrap(), b.to_str().unwrap()]);
        run(&b, &["remote", "set-head", "origin", "trunk"]);
        assert_eq!(default_branch("git", &b, soon()).as_deref(), Some("trunk"));
        run(&b, &["remote", "set-head", "origin", "main"]);
        assert_eq!(default_branch("git", &b, soon()).as_deref(), Some("main"));
        // Not a repository, or no time: nothing, never a guess.
        let plain = d.join("plain");
        fs::create_dir_all(&plain).unwrap();
        assert_eq!(default_branch("git", &plain, soon()), None);
        assert_eq!(default_branch("git", &a, Instant::now()), None);
        assert_eq!(is_clean("git", &plain, soon()), None);
    }
}
