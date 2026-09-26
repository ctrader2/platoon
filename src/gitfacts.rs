//! What git can say about a folder, for the hand-off document: its branch, what is not committed yet, the last
//! commit, and what was committed since a moment (the session's start). Read-only: only `rev-parse`,
//! `symbolic-ref`, `status` and `log` are ever run, with `GIT_OPTIONAL_LOCKS=0` so not even git's own index
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
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitFacts {
    /// The repository's top folder. Paths below are relative to it.
    pub top: String,
    /// None on a detached HEAD, or when it couldn't be read.
    pub branch: Option<String>,
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
fn run(program: &str, dir: &Path, args: &[&str], deadline: Instant) -> Option<String> {
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
    let mut child = cmd.spawn().ok()?;
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
    let changes = run(program, dir, &["status", "--porcelain=v1", "-z", "--untracked-files=normal"], deadline).map(|o| parse_status(&o));
    let last = run(program, dir, &["log", "-1", "--format=%h%x00%s"], deadline).and_then(|o| parse_last(&o));
    let since = since_ms.and_then(|ms| {
        let since = format!("--since=@{}", ms / 1000);
        let max = format!("--max-count={MAX_COMMITS}");
        run(program, dir, &["log", &since, &max, "--format=@@commit %h", "--name-only"], deadline).map(|o| parse_log(&o))
    });
    Some(GitFacts { top, branch, changes, last, since })
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
    (!id.is_empty()).then(|| (clean_path(id), crate::smart::clean_text(subject, 150)))
}

/// `git log --format='@@commit %h' --name-only`: a marker line per commit, then the files it touched.
pub fn parse_log(out: &str) -> Since {
    let mut s = Since::default();
    for line in out.lines() {
        if line.starts_with("@@commit ") {
            s.commits += 1;
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
            let dir = std::env::temp_dir().join(format!("platoon-git-{name}-{}", std::process::id()));
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
        let log = parse_log("@@commit abc\n\na.rs\nb.rs\n@@commit def\n\na.rs\n");
        assert_eq!((log.commits, log.files), (2, vec!["a.rs".to_string(), "b.rs".into()]));
    }
}
