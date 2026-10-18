//! A task's worktree: made when the task starts (`prepare`), and after the work, whether it has changes not yet
//! committed and removing it once its task is Done or Cancelled (`remove`). The branch is never touched: removing a
//! worktree keeps every commit, and opening the task's session again makes the worktree again from its branch.
//! When the app removes them is in `app/worktree_glue.rs`.
//!
//! A task's worktree is `<parent>/<repo>-worktrees/task-<N>` (`worktree_dir`), on the branch `task/<N>`, made from the
//! repository's default branch. What `remove` will touch, and nothing else: that folder, when the task's record
//! points there, git lists it as a worktree of that repository on the task's own branch, and it is not the
//! repository's main worktree. It uses `git worktree remove`, which refuses a worktree with changes by itself; `force`
//! (the user said yes to losing them) adds `--force` once, which still refuses a locked worktree.

use crate::tickets::{Worktree, task_branch};
use std::{
    fs,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(crate) fn git(program: &str, dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("couldn't run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let first = err.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("git said no").trim_start_matches("fatal: ").to_string();
        Err(first)
    }
}

/// `git worktree list --porcelain`: (path, branch ref or ""), the repository's main worktree first.
pub(crate) fn worktrees_of(program: &str, root: &Path) -> Vec<(PathBuf, String)> {
    let text = git(program, root, &["worktree", "list", "--porcelain"]).unwrap_or_default();
    let mut out = Vec::new();
    let (mut path, mut branch): (Option<PathBuf>, String) = (None, String::new());
    for line in text.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            if let Some(p) = path.take() {
                out.push((p, std::mem::take(&mut branch)));
            }
        } else if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(p));
        } else if let Some(b) = line.strip_prefix("branch ") {
            branch = b.to_string();
        }
    }
    out
}

pub(crate) fn same_place(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// Where the worktree of repository `root` on `branch` goes: `<parent>/<name>-worktrees/<branch>`, the branch's `/`
/// as `-` (`task-5`).
pub fn worktree_dir(root: &Path, branch: &str) -> PathBuf {
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "repo".into());
    root.parent().unwrap_or(root).join(format!("{name}-worktrees")).join(branch.replace('/', "-"))
}

/// Where task `number` works in `folder`: a git worktree on `task/<N>`, made from the repository's default branch (its
/// commit written down as the worktree's `base`). None when `folder` is not in git (the task works in the folder as
/// it is). One already made for this task is used again, keeping its base; one platoon removed after the work (its
/// branch kept) is made again from that branch at the same place, when `recorded` (what the task says about its
/// worktree) shows it is this task's. A branch of that name the task doesn't account for is refused. Says why in plain
/// words when it can't. Nothing here ever removes a worktree.
pub fn prepare(program: &str, folder: &str, number: u32, recorded: Option<&Worktree>) -> Result<Option<Worktree>, String> {
    let at = Path::new(folder);
    let Ok(top) = git(program, at, &["rev-parse", "--show-toplevel"]) else { return Ok(None) };
    let root = PathBuf::from(top.trim());
    let branch = task_branch(number);
    let full = format!("refs/heads/{branch}");
    let sub = fs::canonicalize(at)
        .ok()
        .and_then(|c| c.strip_prefix(fs::canonicalize(&root).ok()?).ok().map(Path::to_path_buf))
        .unwrap_or_default();
    let dir = worktree_dir(&root, &branch);
    let shown = dir.display();
    let before = recorded.filter(|w| w.branch == branch && Path::new(&w.path).starts_with(&dir));
    let mut base = before.and_then(|w| w.base.clone());
    let make_parent = || -> Result<(), String> {
        match dir.parent() {
            Some(parent) => fs::create_dir_all(parent).map_err(|e| format!("couldn't make {}: {e}", parent.display())),
            None => Ok(()),
        }
    };
    let from_branch = || -> Result<(), String> {
        make_parent()?;
        let d = dir.to_string_lossy().into_owned();
        git(program, &root, &["worktree", "add", &d, &branch])
            .map(|_| ())
            .map_err(|e| format!("couldn't make the worktree of {} again from its branch: {e}", root.display()))
    };
    match worktrees_of(program, &root).iter().find(|(p, _)| same_place(p, &dir)) {
        Some((_, b)) if *b == full && dir.is_dir() => {}
        // Git still lists it but its folder is gone (removed by hand): forget that, and make it again.
        Some((_, b)) if *b == full && !dir.exists() => {
            git(program, &root, &["worktree", "prune"]).map_err(|e| format!("couldn't tidy the worktrees of {}: {e}", root.display()))?;
            from_branch()?;
        }
        Some(_) => return Err(format!("{shown} is already a worktree on another branch; move it away, then try again")),
        None if dir.exists() => {
            return Err(format!("{shown} is already there and isn't this task's worktree; move it away, then try again"));
        }
        None if git(program, &root, &["rev-parse", "--verify", "--quiet", &full]).is_ok() => {
            if before.is_none() {
                return Err(format!("the branch name {branch} is already taken in {}; rename that branch, then try again", root.display()));
            }
            from_branch()?;
        }
        None => {
            make_parent()?;
            let deadline = Instant::now() + Duration::from_secs(5);
            let commit = |name: &str| {
                git(program, &root, &["rev-parse", "--verify", "--quiet", &format!("{name}^{{commit}}")])
                    .ok()
                    .map(|h| h.trim().to_string())
                    .filter(|h| !h.is_empty())
            };
            let start = crate::gitfacts::default_branch(program, &root, deadline).and_then(|b| commit(&b)).or_else(|| commit("HEAD"));
            let d = dir.to_string_lossy().into_owned();
            let mut args = vec!["worktree", "add", d.as_str(), "-b", branch.as_str()];
            if let Some(h) = &start {
                args.push(h);
            }
            git(program, &root, &args).map_err(|e| format!("couldn't make a worktree of {}: {e}", root.display()))?;
            base = start;
        }
    }
    let path = if sub.as_os_str().is_empty() { dir.clone() } else { dir.join(&sub) };
    // A folder git doesn't track (empty, or ignored) isn't in the worktree: made, so the session can start there.
    if !path.is_dir() {
        fs::create_dir_all(&path).map_err(|e| format!("couldn't make {}: {e}", path.display()))?;
    }
    Ok(Some(Worktree { path: path.to_string_lossy().into_owned(), branch, base, ..Default::default() }))
}

/// What removing a task's worktree came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Removal {
    /// Its folder is gone: removed now, or it was gone already (git's note of it tidied away). The branch is kept.
    Removed,
    /// Left in place: it has changes not yet committed (changed files, or new files git doesn't ignore).
    Uncommitted,
    /// Not touched, and why, in plain words.
    Refused(String),
}

/// Where task `number`'s worktree `w` (of its `folder`) is, checked against its record: (its repository's root, the
/// worktree's folder). Err: why platoon won't touch it.
fn located(program: &str, folder: &str, w: &Worktree, number: u32) -> Result<(PathBuf, PathBuf), String> {
    if w.branch != task_branch(number) {
        return Err(format!("its branch {} isn't this task's", w.branch));
    }
    let path = Path::new(&w.path);
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!("{} isn't a place platoon makes worktrees", w.path));
    }
    if !Path::new(folder).is_dir() {
        return Err(format!("its repository's folder {folder} isn't there any more"));
    }
    let top = git(program, Path::new(folder), &["rev-parse", "--show-toplevel"])
        .map_err(|e| format!("{folder} isn't in a git repository any more: {e}"))?;
    let root = PathBuf::from(top.trim());
    let dir = worktree_dir(&root, &w.branch);
    if !path.starts_with(&dir) || same_place(&dir, &root) {
        return Err(format!("{} isn't where platoon made this task's worktree ({})", w.path, dir.display()));
    }
    Ok((root, dir))
}

/// Does the worktree at `dir` have changes not yet committed: changed or staged files, or new files git doesn't
/// ignore? Err when git can't say.
pub fn uncommitted(program: &str, dir: &Path) -> Result<bool, String> {
    git(program, dir, &["status", "--porcelain", "--untracked-files=normal", "--ignore-submodules=none"]).map(|out| !out.trim().is_empty())
}

/// `uncommitted`, for a question to the user on the UI thread: git gets `budget`; false when it can't say in time.
pub fn uncommitted_quick(program: &str, dir: &Path, budget: Duration) -> bool {
    crate::gitfacts::run(program, dir, &["status", "--porcelain", "--untracked-files=normal"], Instant::now() + budget)
        .is_some_and(|out| !out.trim().is_empty())
}

/// Remove task `number`'s worktree `w` (of its `folder`), keeping its branch. Only a worktree whose record is sound (see
/// the top of this file) and that git lists for its repository on the task's branch is ever touched. One with changes
/// not yet committed is left in place unless `force` (the user said yes to losing them). Never while a session is
/// working in it: the caller makes sure of that.
pub fn remove(program: &str, folder: &str, w: &Worktree, number: u32, force: bool) -> Removal {
    let (root, dir) = match located(program, folder, w, number) {
        Ok(x) => x,
        Err(e) => return Removal::Refused(e),
    };
    let full = format!("refs/heads/{}", w.branch);
    let listed = worktrees_of(program, &root);
    if listed.is_empty() {
        return Removal::Refused(format!("git didn't list the worktrees of {}", root.display()));
    }
    match listed.iter().enumerate().find(|(_, (p, _))| same_place(p, &dir)) {
        None if dir.exists() => {
            return Removal::Refused(format!("{} isn't a git worktree of {}, so platoon leaves it alone", dir.display(), root.display()));
        }
        // Gone, and git knows nothing of it: nothing to do.
        None => return Removal::Removed,
        Some((0, _)) => return Removal::Refused(format!("{} is the repository itself", dir.display())),
        Some((_, (_, b))) if *b != full => {
            return Removal::Refused(format!("{} is on another branch now, so platoon leaves it alone", dir.display()));
        }
        // Gone already (removed by hand): only git's note of it is left.
        Some(_) if !dir.exists() => {
            return match git(program, &root, &["worktree", "prune"]) {
                Ok(_) => Removal::Removed,
                Err(e) => Removal::Refused(format!("couldn't tidy the worktrees of {}: {e}", root.display())),
            };
        }
        Some(_) => {}
    }
    if !force {
        match uncommitted(program, &dir) {
            Ok(true) => return Removal::Uncommitted,
            Ok(false) => {}
            Err(e) => return Removal::Refused(format!("git couldn't say whether {} has changes: {e}", dir.display())),
        }
    }
    let d = dir.to_string_lossy().into_owned();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&d);
    match git(program, &root, &args) {
        Ok(_) => Removal::Removed,
        // Changed between the look and the removal: git said no by itself.
        Err(_) if !force && uncommitted(program, &dir) == Ok(true) => Removal::Uncommitted,
        Err(e) => Removal::Refused(format!("git didn't remove {}: {e}", dir.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tickets::{NewTicket, Tickets};
    use std::{fs, process::Command, process::Stdio};

    fn dir(name: &str) -> PathBuf {
        let d = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-wtc-{name}-{}", std::process::id())));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn run(dir: &Path, args: &[&str]) -> String {
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

    /// A repository `name` in `base` with one commit on `main` (and `target/` ignored).
    fn repo(base: &Path, name: &str) -> PathBuf {
        let r = base.join(name);
        fs::create_dir_all(r.join("src")).unwrap();
        fs::write(r.join("README"), "hi\n").unwrap();
        fs::write(r.join(".gitignore"), "target/\n").unwrap();
        fs::write(r.join("src/main.rs"), "fn main() {}\n").unwrap();
        run(&r, &["init", "-q"]);
        run(&r, &["add", "."]);
        run(&r, &["commit", "-q", "-m", "first"]);
        r
    }

    fn commit_in(dir: &Path, file: &str, msg: &str) -> String {
        fs::write(dir.join(file), format!("{msg}\n")).unwrap();
        run(dir, &["add", "."]);
        run(dir, &["commit", "-q", "-m", msg]);
        run(dir, &["rev-parse", "HEAD"])
    }

    fn branches(r: &Path) -> String {
        run(r, &["branch", "--format=%(refname:short)"])
    }

    fn listed(r: &Path) -> Vec<PathBuf> {
        worktrees_of("git", r).into_iter().map(|(p, _)| p).collect()
    }

    fn prep(folder: &Path, n: u32, recorded: Option<&Worktree>) -> Result<Worktree, String> {
        prepare("git", &folder.to_string_lossy(), n, recorded).map(|w| w.expect("in git"))
    }

    #[test]
    fn the_worktree_is_made_from_the_default_branch_and_keeps_its_base_when_used_again() {
        let base = dir("base");
        let shop = repo(&base, "shop");
        let head = run(&shop, &["rev-parse", "HEAD"]);
        // The checkout is on another branch with a commit of its own: the worktree still starts from main.
        run(&shop, &["switch", "-q", "-c", "side"]);
        commit_in(&shop, "side.txt", "on the side");
        let got = prep(&shop, 3, None).unwrap();
        assert_eq!((got.base.as_deref(), got.branch.as_str()), (Some(head.as_str()), "task/3"));
        assert_eq!(Path::new(&got.path), worktree_dir(&shop, "task/3"));
        assert!(!Path::new(&got.path).join("side.txt").exists(), "made from main, not from the checkout's branch");
        commit_in(&shop, "later.txt", "later");
        assert_eq!(prep(&shop, 3, Some(&got)).unwrap(), got, "used again: the base stays where the branch started");
        let config = dir("base-config");
        let mut t = Tickets::open(&config, "p1");
        let n =
            t.add(NewTicket { title: "Basket".into(), folder: Some(shop.to_string_lossy().into_owned()), ..Default::default() }).unwrap();
        t.start(n, "feature", "s", Some(got.clone())).unwrap();
        assert_eq!(Tickets::open(&config, "p1").get(n).unwrap().worktree, Some(got));
        let text = fs::read_to_string(crate::tickets::path_in(&config, "p1")).unwrap();
        assert!(text.contains(&format!("\"base\": \"{head}\"")), "{text}");
        assert!(!text.contains("removed") && !text.contains("kept_uncommitted"), "{text}");
        // A folder outside git: no worktree.
        let plain = base.join("plain");
        fs::create_dir_all(&plain).unwrap();
        assert_eq!(prepare("git", &plain.to_string_lossy(), 1, None), Ok(None));
        let _ = fs::remove_dir_all(&base);
        let _ = fs::remove_dir_all(&config);
    }

    #[test]
    fn a_clean_worktree_is_removed_and_its_branch_and_commits_kept() {
        let base = dir("clean");
        let shop = repo(&base, "shop");
        let src = shop.join("src");
        let got = prep(&src, 2, None).unwrap();
        let f = src.to_string_lossy().into_owned();
        let wt = worktree_dir(&shop, "task/2");
        let tip = commit_in(&wt, "basket.txt", "the basket");
        // Ignored files (a build) don't count as changes, and go with the folder.
        fs::create_dir_all(wt.join("target")).unwrap();
        fs::write(wt.join("target/out"), "built").unwrap();
        assert_eq!(uncommitted("git", &wt), Ok(false));
        assert_eq!(remove("git", &f, &got, 2, false), Removal::Removed);
        assert!(!wt.exists());
        assert_eq!(listed(&shop).len(), 1, "git no longer lists it");
        assert!(branches(&shop).lines().any(|b| b == "task/2"));
        assert_eq!(run(&shop, &["rev-parse", "task/2"]), tip, "its commits are on the branch");
        // Again: gone already, still fine.
        assert_eq!(remove("git", &f, &got, 2, false), Removal::Removed);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_worktree_with_changed_or_new_files_is_kept_unless_the_user_said_yes() {
        let base = dir("dirty");
        let shop = repo(&base, "shop");
        let f = shop.to_string_lossy().into_owned();
        let got = prep(&shop, 4, None).unwrap();
        let wt = worktree_dir(&shop, "task/4");
        fs::write(wt.join("README"), "changed\n").unwrap();
        assert_eq!(remove("git", &f, &got, 4, false), Removal::Uncommitted);
        assert!(wt.join("README").exists());
        run(&wt, &["checkout", "--", "README"]);
        // A new file git doesn't ignore.
        fs::write(wt.join("notes.txt"), "mine\n").unwrap();
        assert_eq!(uncommitted("git", &wt), Ok(true));
        assert!(uncommitted_quick("git", &wt, Duration::from_secs(5)));
        assert_eq!(remove("git", &f, &got, 4, false), Removal::Uncommitted);
        assert!(wt.join("notes.txt").exists());
        // Staged only.
        run(&wt, &["add", "notes.txt"]);
        assert_eq!(remove("git", &f, &got, 4, false), Removal::Uncommitted);
        // The user said yes: removed, changes lost, branch kept.
        assert_eq!(remove("git", &f, &got, 4, true), Removal::Removed);
        assert!(!wt.exists());
        assert!(branches(&shop).lines().any(|b| b == "task/4"));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn nothing_is_touched_that_doesnt_match_the_tickets_record() {
        let base = dir("guard");
        let shop = repo(&base, "shop");
        let f = shop.to_string_lossy().into_owned();
        let got = prep(&shop, 8, None).unwrap();
        let wt = worktree_dir(&shop, "task/8");
        let refused = |w: &Worktree, n: u32| matches!(remove("git", &f, w, n, true), Removal::Refused(_));
        // Another ticket's number, another branch, a path elsewhere, a path that climbs out, the repository itself.
        assert!(refused(&got, 9));
        assert!(refused(&Worktree { branch: "main".into(), ..got.clone() }, 8));
        assert!(refused(&Worktree { path: shop.to_string_lossy().into_owned(), ..got.clone() }, 8));
        assert!(refused(&Worktree { path: format!("{}/../../shop", wt.display()), ..got.clone() }, 8));
        assert!(refused(&Worktree { path: "relative/task-8".into(), ..got.clone() }, 8));
        assert!(wt.is_dir() && shop.join("README").exists());
        // The right place, but not a worktree of this repository (a plain folder someone made): left alone.
        let fake = worktree_dir(&shop, "task/11");
        fs::create_dir_all(&fake).unwrap();
        fs::write(fake.join("keep.txt"), "x").unwrap();
        let w11 = Worktree { path: fake.to_string_lossy().into_owned(), branch: "task/11".into(), ..got.clone() };
        assert!(refused(&w11, 11));
        assert!(fake.join("keep.txt").exists());
        // A worktree at the right place that has been switched to another branch: left alone.
        run(&wt, &["switch", "-q", "-c", "elsewhere"]);
        assert!(refused(&got, 8));
        assert!(wt.is_dir());
        // Its repository gone: nothing to check against, so nothing is touched.
        assert!(matches!(remove("git", &base.join("gone").to_string_lossy(), &got, 8, true), Removal::Refused(_)));
        assert!(wt.is_dir());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_removed_worktree_comes_back_from_its_branch_at_the_same_place_with_its_commits_and_base() {
        let base = dir("back");
        let shop = repo(&base, "shop");
        let src = shop.join("src");
        let f = src.to_string_lossy().into_owned();
        let got = prep(&src, 6, None).unwrap();
        let wt = worktree_dir(&shop, "task/6");
        let tip = commit_in(&wt, "basket.txt", "the basket");
        assert_eq!(remove("git", &f, &got, 6, false), Removal::Removed);
        commit_in(&shop, "later.txt", "main moves on");
        // Without the ticket's record the branch name is taken, as before.
        let err = prep(&src, 6, None).unwrap_err();
        assert!(err.contains("task/6 is already taken"), "{err}");
        assert!(!wt.exists());
        // With it: the same place, on the branch, with its commit and the base it was made from.
        let removed = Worktree { removed: Some(9), ..got.clone() };
        let back = prep(&src, 6, Some(&removed)).unwrap();
        assert_eq!(back, got, "same path, branch and base");
        assert_eq!(run(&wt, &["rev-parse", "HEAD"]), tip);
        assert_eq!(run(&wt, &["rev-parse", "--abbrev-ref", "HEAD"]), "task/6");
        assert!(wt.join("basket.txt").exists());
        // Removed by hand (git still has a note of it): made again too.
        fs::remove_dir_all(&wt).unwrap();
        let back = prep(&src, 6, Some(&got)).unwrap();
        assert_eq!(back, got);
        assert!(wt.join("basket.txt").exists());
        let _ = fs::remove_dir_all(&base);
    }
}
