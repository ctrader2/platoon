//! Committing a Codex task's work for it. Codex keeps a writable root's git data read-only in its sandbox (for a git
//! worktree also the main repository's `.git/worktrees/<name>` it points at, whatever `writable_roots` says), so a
//! Codex task's session can change files but can't commit them. platoon commits for it, from the task's `Stop` hook
//! (which runs outside the sandbox), before the check of the work: everything changed in the worktree (`git add -A`),
//! one commit per stop, its message `#<N> <title>`.
//!
//! - Only on the task's own branch: a worktree that has another branch checked out is never committed to.
//! - With the repository's own identity, and its own hooks (a `pre-commit` hook runs as it does for any commit, and one
//!   that refuses the commit is said with its first lines). Only when git knows no identity at all is one given, for
//!   that commit only (`-c user.name=... -c user.email=...`): platoon never writes git's config.
//! - Time-boxed: git and the hooks it runs are ended (their process group) when the box is up.
//! - A commit that doesn't happen loses nothing: the changes stay in the worktree (staged), and the caller says why.

use std::{
    io::Read,
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// How long committing a stop's work may take, the repository's own hooks included.
pub const BUDGET: Duration = Duration::from_secs(60);

/// The identity a commit gets when git knows none (no `user.name`/`user.email`, and none git could make up).
pub const FALLBACK_NAME: &str = "platoon";
pub const FALLBACK_EMAIL: &str = "platoon@localhost";

/// What became of committing a stop's work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Committed {
    /// Nothing had changed: no commit.
    Nothing,
    /// One commit, its short id.
    Made(String),
    /// No commit: why, in plain words (the changes are still in the worktree).
    Failed(String),
}

/// One git command in `dir` by `deadline`: (succeeded, its output and errors together, cut short).
fn run(program: &str, dir: &Path, args: &[&str], deadline: Instant) -> (bool, String) {
    if Instant::now() >= deadline {
        return (false, "it took too long".into());
    }
    let mut cmd = Command::new(program);
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Its own process group: a hook git runs ends with it when time is up, and nothing else is ever signaled.
        .process_group(0);
    let mut child = match crate::ai::spawn_retrying(&mut cmd) {
        Ok(c) => c,
        Err(e) => return (false, format!("couldn't run git: {e}")),
    };
    let read = |r: Option<Box<dyn Read + Send>>| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut r) = r {
                let _ = (&mut r).take(64 * 1024).read_to_end(&mut buf);
                let _ = std::io::copy(&mut r, &mut std::io::sink());
            }
            buf
        })
    };
    let out = read(child.stdout.take().map(|o| Box::new(o) as Box<dyn Read + Send>));
    let err = read(child.stderr.take().map(|e| Box::new(e) as Box<dyn Read + Send>));
    let ok = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status.success()),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => {
                unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
                let _ = child.wait();
                break None;
            }
        }
    };
    let text = [out.join().unwrap_or_default(), err.join().unwrap_or_default()]
        .iter()
        .map(|b| String::from_utf8_lossy(b).trim().to_string())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    match ok {
        Some(ok) => (ok, text),
        None => (false, "git didn't finish in time (its own hooks included), so it was stopped".into()),
    }
}

/// The first few lines of what git said, on one line.
fn first_lines(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).take(3).collect();
    crate::tickets::clean(&lines.join(" / "), 300)
}

/// Commit everything changed in the worktree `dir`, on `branch` (its own; any other is refused), with `message`,
/// within `budget`.
pub fn commit_all(program: &str, dir: &Path, branch: &str, message: &str, budget: Duration) -> Committed {
    let deadline = Instant::now() + budget;
    let fail = |why: String| Committed::Failed(why);
    let (ok, head) = run(program, dir, &["symbolic-ref", "--quiet", "--short", "HEAD"], deadline);
    if !ok || head.trim() != branch {
        let on = if ok && !head.trim().is_empty() { format!("is on {}", head.trim()) } else { "has no branch checked out".into() };
        return fail(format!("its worktree {on}, not {branch}, and platoon commits only on the task's branch"));
    }
    let (ok, status) = run(program, dir, &["status", "--porcelain=v1", "--untracked-files=all"], deadline);
    if !ok {
        return fail(format!("git couldn't say what changed: {}", first_lines(&status)));
    }
    if status.trim().is_empty() {
        return Committed::Nothing;
    }
    let (ok, said) = run(program, dir, &["add", "-A"], deadline);
    if !ok {
        return fail(format!("git add failed: {}", first_lines(&said)));
    }
    // The repository's own identity; one only for this commit when git has none at all.
    let (known, _) = run(program, dir, &["var", "GIT_COMMITTER_IDENT"], deadline);
    let (known_author, _) = run(program, dir, &["var", "GIT_AUTHOR_IDENT"], deadline);
    let name = format!("user.name={FALLBACK_NAME}");
    let email = format!("user.email={FALLBACK_EMAIL}");
    let mut args: Vec<&str> = Vec::new();
    if !(known && known_author) {
        args.extend(["-c", &name, "-c", &email]);
    }
    args.extend(["commit", "--quiet", "-m", message]);
    let (ok, said) = run(program, dir, &args, deadline);
    if !ok {
        return fail(format!("git commit failed: {}", first_lines(&said)));
    }
    let (_, id) = run(program, dir, &["rev-parse", "--short", "HEAD"], deadline);
    Committed::Made(id.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A repository with one commit, its own identity, and a worktree of it on `task/5`.
    fn repo(name: &str) -> (PathBuf, PathBuf) {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-taskcommit-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        let shop = d.join("shop");
        fs::create_dir_all(&shop).unwrap();
        git(&shop, &["init", "-q"]);
        git(&shop, &["config", "user.name", "Shop Owner"]);
        git(&shop, &["config", "user.email", "owner@example.com"]);
        git(&shop, &["config", "commit.gpgsign", "false"]);
        fs::write(shop.join("README"), "hi\n").unwrap();
        git(&shop, &["add", "."]);
        git(&shop, &["commit", "-q", "-m", "first"]);
        let wt = d.join("shop-worktrees/task-5");
        git(&shop, &["worktree", "add", "-q", &wt.to_string_lossy(), "-b", "task/5"]);
        (shop, wt)
    }

    #[test]
    fn whats_changed_is_one_commit_on_the_tasks_branch_with_the_repos_identity_and_nothing_changed_is_none() {
        let (shop, wt) = repo("made");
        assert_eq!(commit_all("git", &wt, "task/5", "#5 Login", BUDGET), Committed::Nothing);
        fs::write(wt.join("README"), "hi\nthere\n").unwrap();
        fs::create_dir_all(wt.join("src")).unwrap();
        fs::write(wt.join("src/new.rs"), "fn x() {}\n").unwrap();
        let Committed::Made(id) = commit_all("git", &wt, "task/5", "#5 Login", BUDGET) else { panic!() };
        assert_eq!(git(&shop, &["rev-parse", "--short", "task/5"]), id);
        assert_eq!(git(&shop, &["log", "-1", "--format=%s|%an|%ae", "task/5"]), "#5 Login|Shop Owner|owner@example.com");
        assert_eq!(git(&shop, &["show", "--name-only", "--format=", "task/5"]), "README\nsrc/new.rs");
        assert_eq!(git(&wt, &["status", "--porcelain"]), "", "everything is in it");
        assert_eq!(git(&shop, &["rev-list", "--count", "main"]), "1", "the main branch is untouched");
        assert_eq!(commit_all("git", &wt, "task/5", "#5 Login", BUDGET), Committed::Nothing, "and nothing again");
    }

    #[test]
    fn a_commit_that_cant_happen_says_why_and_keeps_the_work() {
        let (shop, wt) = repo("fails");
        // The repository's own hook refuses it: its words come back, the change stays.
        let hooks = git(&shop, &["rev-parse", "--git-common-dir"]);
        let hook = shop.join(hooks).join("hooks/pre-commit");
        fs::write(&hook, "#!/bin/sh\necho 'lint: trailing spaces in README' >&2\nexit 1\n").unwrap();
        fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        fs::write(wt.join("README"), "hi \n").unwrap();
        let got = commit_all("git", &wt, "task/5", "#5 Login", BUDGET);
        assert!(
            matches!(&got, Committed::Failed(why) if why.contains("git commit failed") && why.contains("lint: trailing spaces")),
            "{got:?}"
        );
        assert_eq!(fs::read_to_string(wt.join("README")).unwrap(), "hi \n");
        assert_eq!(git(&shop, &["rev-list", "--count", "task/5"]), "1", "no commit");
        // A hook that never ends is ended with its time box.
        fs::write(&hook, "#!/bin/sh\nsleep 30\n").unwrap();
        let started = Instant::now();
        let got = commit_all("git", &wt, "task/5", "#5 Login", Duration::from_secs(2));
        assert!(matches!(&got, Committed::Failed(why) if why.contains("didn't finish")), "{got:?}");
        assert!(started.elapsed() < Duration::from_secs(10));
        // Another branch checked out: never committed to.
        fs::remove_file(&hook).unwrap();
        git(&wt, &["checkout", "-q", "-b", "elsewhere"]);
        let got = commit_all("git", &wt, "task/5", "#5 Login", BUDGET);
        assert!(matches!(&got, Committed::Failed(why) if why.contains("is on elsewhere, not task/5")), "{got:?}");
    }

    #[test]
    fn with_no_identity_at_all_the_commit_is_platoons_and_no_config_is_written() {
        let (shop, wt) = repo("no-identity");
        git(&shop, &["config", "--unset", "user.name"]);
        git(&shop, &["config", "--unset", "user.email"]);
        // No identity anywhere git looks (and git told not to make one up).
        git(&shop, &["config", "user.useConfigOnly", "true"]);
        let config_before = fs::read_to_string(shop.join(".git/config")).unwrap();
        fs::write(wt.join("README"), "changed\n").unwrap();
        let home = shop.parent().unwrap().join("empty-home");
        fs::create_dir_all(&home).unwrap();
        // git as if in a home without a ~/.gitconfig.
        let program = shop.parent().unwrap().join("git-no-user");
        fs::write(
            &program,
            format!("#!/bin/sh\nHOME='{}' XDG_CONFIG_HOME='{}' GIT_CONFIG_NOSYSTEM=1 exec git \"$@\"\n", home.display(), home.display()),
        )
        .unwrap();
        fs::set_permissions(&program, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let got = commit_all(&program.to_string_lossy(), &wt, "task/5", "#5 Login", BUDGET);
        assert!(matches!(got, Committed::Made(_)), "{got:?}");
        assert_eq!(git(&shop, &["log", "-1", "--format=%an|%ae", "task/5"]), format!("{FALLBACK_NAME}|{FALLBACK_EMAIL}"));
        assert_eq!(fs::read_to_string(shop.join(".git/config")).unwrap(), config_before, "git's config as it was");
    }
}
