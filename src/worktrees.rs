//! A ticket's worktrees after the work: what a branch is compared with (`diff_base`), whether one has changes not yet
//! committed, and removing one once its ticket is Done or Cancelled (`remove`). The branch is never touched: removing
//! a worktree keeps every commit, and continuing the squad makes the worktree again from its branch
//! (`squads::prepare_worktrees`). Making them is in `squads.rs`; when the app removes them is in `app/worktree_glue.rs`.
//!
//! What `remove` will touch, and nothing else: the folder platoon made for this ticket
//! (`<parent>/<repo>-worktrees/ticket-<N>`, `squads::worktree_dir`), when the ticket's record points there, git lists it
//! as a worktree of that repository on the ticket's own branch, and it is not the repository's main worktree. It uses
//! `git worktree remove`, which refuses a worktree with changes by itself; `force` (the user said yes to losing them)
//! adds `--force` once, which still refuses a locked worktree.

use crate::squads::{branch_name, git, same_place, worktree_dir, worktrees_of};
use crate::tickets::Worktree;
use std::{
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

/// What removing one of a ticket's worktrees came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Removal {
    /// Its folder is gone: removed now, or it was gone already (git's note of it tidied away). The branch is kept.
    Removed,
    /// Left in place: it has changes not yet committed (changed files, or new files git doesn't ignore).
    Uncommitted,
    /// Not touched, and why, in plain words.
    Refused(String),
}

/// The commit ticket `w`'s branch is compared with to show what the ticket changed: the commit its worktree was made
/// from, as written on the ticket. A worktree made before platoon wrote that down: where its branch left `main`
/// (else `master`), else where it left the repository's HEAD as it is now (HEAD itself if they share nothing). None
/// when its branch or its repository isn't there any more. `program` is git.
pub fn diff_base(w: &Worktree, program: &str) -> Option<String> {
    if let Some(b) = &w.base {
        return Some(b.clone());
    }
    let dir = [&w.folder, &w.path].into_iter().map(Path::new).find(|p| p.is_dir())?;
    // The repository's main worktree: its HEAD is the one "the repository's HEAD" means (the ticket's own worktree
    // has the branch itself as its HEAD).
    let main = worktrees_of(program, dir).into_iter().next().map(|(p, _)| p).filter(|p| p.is_dir()).unwrap_or_else(|| dir.to_path_buf());
    let one = |args: &[&str]| git(program, &main, args).ok().map(|o| o.trim().to_string()).filter(|o| !o.is_empty());
    let branch = format!("refs/heads/{}", w.branch);
    one(&["rev-parse", "--verify", "--quiet", &format!("{branch}^{{commit}}")])?;
    for trunk in ["refs/heads/main", "refs/heads/master"] {
        if one(&["rev-parse", "--verify", "--quiet", trunk]).is_some()
            && let Some(mb) = one(&["merge-base", &branch, trunk])
        {
            return Some(mb);
        }
    }
    let head = one(&["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])?;
    one(&["merge-base", &branch, &head]).or(Some(head))
}

/// Where ticket `number`'s worktree `w` is, checked against its record: (its repository's root, the worktree's
/// folder). Err: why platoon won't touch it.
fn located(program: &str, w: &Worktree, number: u32) -> Result<(PathBuf, PathBuf), String> {
    if w.branch != branch_name(number) {
        return Err(format!("its branch {} isn't this ticket's", w.branch));
    }
    let path = Path::new(&w.path);
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!("{} isn't a place platoon makes worktrees", w.path));
    }
    if !Path::new(&w.folder).is_dir() {
        return Err(format!("its repository's folder {} isn't there any more", w.folder));
    }
    let top = git(program, Path::new(&w.folder), &["rev-parse", "--show-toplevel"])
        .map_err(|e| format!("{} isn't in a git repository any more: {e}", w.folder))?;
    let root = PathBuf::from(top.trim());
    let dir = worktree_dir(&root, number);
    if !path.starts_with(&dir) || same_place(&dir, &root) {
        return Err(format!("{} isn't where platoon made this ticket's worktree ({})", w.path, dir.display()));
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

/// Remove ticket `number`'s worktree `w`, keeping its branch. Only a worktree whose record is sound (see the top of
/// this file) and that git lists for its repository on the ticket's branch is ever touched. One with changes not yet
/// committed is left in place unless `force` (the user said yes to losing them). Never while a session is working in
/// it: the caller makes sure of that.
pub fn remove(program: &str, w: &Worktree, number: u32, force: bool) -> Removal {
    let (root, dir) = match located(program, w, number) {
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
    use crate::squads::prepare_worktrees;
    use crate::tickets::{NewTicket, Tickets, WtMark};
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

    #[test]
    fn the_worktree_records_the_commit_it_was_made_from_and_keeps_it_when_used_again() {
        let base = dir("base");
        let shop = repo(&base, "shop");
        let head = run(&shop, &["rev-parse", "HEAD"]);
        let folders = vec![shop.to_string_lossy().into_owned()];
        let got = prepare_worktrees("git", &folders, 3, &[]).unwrap();
        assert_eq!(got[0].base.as_deref(), Some(head.as_str()));
        assert_eq!(diff_base(&got[0], "git").as_deref(), Some(head.as_str()));
        // The main repository moves on; the worktree is used again: the base stays where the branch started.
        commit_in(&shop, "later.txt", "later on main");
        let again = prepare_worktrees("git", &folders, 3, &got).unwrap();
        assert_eq!(again, got);
        // Written on the ticket and read back.
        let config = dir("base-config");
        let mut t = Tickets::open(&config, "p1");
        let n = t.add(NewTicket { title: "Basket".into(), folders: folders.clone(), ..Default::default() }).unwrap();
        let lead =
            crate::tickets::Leader { squad: "coding".into(), session: "s".into(), started: 1, names: Default::default(), snapshot: None };
        t.start_leader(n, &lead, &got).unwrap();
        assert_eq!(Tickets::open(&config, "p1").get(n).unwrap().worktrees, got);
        let text = fs::read_to_string(crate::tickets::path_in(&config, "p1")).unwrap();
        assert!(text.contains(&format!("\"base\": \"{head}\"")), "{text}");
        assert!(!text.contains("removed") && !text.contains("kept_uncommitted"), "nothing more until there is something to say: {text}");
        let _ = fs::remove_dir_all(&base);
        let _ = fs::remove_dir_all(&config);
    }

    #[test]
    fn a_ticket_from_before_the_base_reads_and_writes_as_it_was_and_keeps_keys_it_doesnt_know() {
        let config = dir("old");
        let file = crate::tickets::path_in(&config, "p1");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        let old = serde_json::json!({"next": 2, "tickets": [{"number": 1, "title": "Old", "file": "01-old.md", "state": "done", "folders": ["/w/shop"], "blocked_by": [], "created": 1, "updated": 5,
            "worktrees": [{"folder": "/w/shop", "path": "/w/shop-worktrees/ticket-1", "branch": "ticket/1", "colour": "teal"}], "later": {"x": 1}}]});
        fs::write(&file, serde_json::to_string_pretty(&old).unwrap()).unwrap();
        let mut t = Tickets::open(&config, "p1");
        let w = t.get(1).unwrap().worktrees[0].clone();
        assert_eq!(
            w,
            Worktree {
                folder: "/w/shop".into(),
                path: "/w/shop-worktrees/ticket-1".into(),
                branch: "ticket/1".into(),
                ..Default::default()
            }
        );
        // A rename rewrites the file: the worktree's entry, and everything platoon doesn't know, as they were.
        t.rename(1, "Old one").unwrap();
        let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v["tickets"][0]["worktrees"], old["tickets"][0]["worktrees"]);
        assert_eq!(v["tickets"][0]["later"], serde_json::json!({"x": 1}));
        // Marks go on that entry alone, keep its unknown key, and never touch `updated`.
        assert!(t.mark_worktree(1, "/w/shop-worktrees/ticket-1", WtMark::KeptUncommitted).unwrap());
        assert!(!t.mark_worktree(1, "/w/shop-worktrees/ticket-1", WtMark::KeptUncommitted).unwrap(), "said already: nothing written");
        assert!(!t.mark_worktree(1, "/elsewhere", WtMark::Removed).unwrap(), "no worktree there");
        let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v["tickets"][0]["worktrees"][0]["colour"], "teal");
        assert_eq!(v["tickets"][0]["worktrees"][0]["kept_uncommitted"], true);
        let updated = v["tickets"][0]["updated"].clone();
        assert!(t.mark_worktree(1, "/w/shop-worktrees/ticket-1", WtMark::Removed).unwrap());
        let w = t.get(1).unwrap().worktrees[0].clone();
        assert!(w.removed.is_some() && !w.kept_uncommitted && w.is_gone(), "{w:?}");
        assert!(t.mark_worktree(1, "/w/shop-worktrees/ticket-1", WtMark::Present).unwrap());
        let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v["tickets"][0]["worktrees"], old["tickets"][0]["worktrees"], "back as it was");
        assert_eq!(v["tickets"][0]["updated"], updated);
        // A base that isn't a commit id is not taken.
        let mut bad = old.clone();
        if let Some(w) = bad.pointer_mut("/tickets/0/worktrees/0").and_then(|w| w.as_object_mut()) {
            w.insert("base".into(), serde_json::json!("; rm -rf /"));
        }
        fs::write(&file, bad.to_string()).unwrap();
        assert_eq!(Tickets::open(&config, "p1").get(1).unwrap().worktrees[0].base, None);
        let _ = fs::remove_dir_all(&config);
    }

    #[test]
    fn a_worktree_without_a_base_is_compared_with_where_it_left_main_else_master_else_head() {
        let base = dir("fallback");
        let shop = repo(&base, "shop");
        let start = run(&shop, &["rev-parse", "HEAD"]);
        let got = prepare_worktrees("git", &[shop.to_string_lossy().into_owned()], 5, &[]).unwrap();
        let wt = PathBuf::from(&got[0].path);
        commit_in(&wt, "basket.txt", "basket on the branch");
        commit_in(&shop, "other.txt", "main moves on");
        let old = Worktree { base: None, ..got[0].clone() };
        assert_eq!(diff_base(&old, "git").as_deref(), Some(start.as_str()), "where it left main");
        // A repository whose trunk is `master`.
        let legacy = repo(&base, "legacy");
        run(&legacy, &["branch", "-m", "main", "master"]);
        let start = run(&legacy, &["rev-parse", "HEAD"]);
        let got = prepare_worktrees("git", &[legacy.to_string_lossy().into_owned()], 6, &[]).unwrap();
        commit_in(Path::new(&got[0].path), "x.txt", "on the branch");
        commit_in(&legacy, "y.txt", "master moves on");
        assert_eq!(diff_base(&Worktree { base: None, ..got[0].clone() }, "git").as_deref(), Some(start.as_str()), "where it left master");
        // Neither: where it left the repository's HEAD (the main worktree's, not the branch's own).
        let odd = repo(&base, "odd");
        run(&odd, &["branch", "-m", "main", "trunk"]);
        let start = run(&odd, &["rev-parse", "HEAD"]);
        let got = prepare_worktrees("git", &[odd.to_string_lossy().into_owned()], 7, &[]).unwrap();
        commit_in(Path::new(&got[0].path), "x.txt", "on the branch");
        commit_in(&odd, "y.txt", "trunk moves on");
        assert_eq!(diff_base(&Worktree { base: None, ..got[0].clone() }, "git").as_deref(), Some(start.as_str()), "where it left HEAD");
        // Its branch deleted, or its repository gone: no base.
        let gone = Worktree { base: None, branch: "ticket/99".into(), ..got[0].clone() };
        assert_eq!(diff_base(&gone, "git"), None);
        let nowhere =
            Worktree { folder: "/nowhere/at/all".into(), path: "/nowhere/else".into(), branch: "ticket/1".into(), ..Default::default() };
        assert_eq!(diff_base(&nowhere, "git"), None);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_clean_worktree_is_removed_and_its_branch_and_commits_kept() {
        let base = dir("clean");
        let shop = repo(&base, "shop");
        let got = prepare_worktrees("git", &[shop.join("src").to_string_lossy().into_owned()], 2, &[]).unwrap();
        let wt = worktree_dir(&shop, 2);
        let tip = commit_in(&wt, "basket.txt", "the basket");
        // Ignored files (a build) don't count as changes, and go with the folder.
        fs::create_dir_all(wt.join("target")).unwrap();
        fs::write(wt.join("target/out"), "built").unwrap();
        assert_eq!(uncommitted("git", &wt), Ok(false));
        assert_eq!(remove("git", &got[0], 2, false), Removal::Removed);
        assert!(!wt.exists());
        assert_eq!(listed(&shop).len(), 1, "git no longer lists it");
        assert!(branches(&shop).lines().any(|b| b == "ticket/2"));
        assert_eq!(run(&shop, &["rev-parse", "ticket/2"]), tip, "its commits are on the branch");
        // Again: gone already, still fine.
        assert_eq!(remove("git", &got[0], 2, false), Removal::Removed);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_worktree_with_changed_or_new_files_is_kept_unless_the_user_said_yes() {
        let base = dir("dirty");
        let shop = repo(&base, "shop");
        let folders = vec![shop.to_string_lossy().into_owned()];
        let got = prepare_worktrees("git", &folders, 4, &[]).unwrap();
        let wt = worktree_dir(&shop, 4);
        fs::write(wt.join("README"), "changed\n").unwrap();
        assert_eq!(remove("git", &got[0], 4, false), Removal::Uncommitted);
        assert!(wt.join("README").exists());
        run(&wt, &["checkout", "--", "README"]);
        // A new file git doesn't ignore.
        fs::write(wt.join("notes.txt"), "mine\n").unwrap();
        assert_eq!(uncommitted("git", &wt), Ok(true));
        assert!(uncommitted_quick("git", &wt, Duration::from_secs(5)));
        assert_eq!(remove("git", &got[0], 4, false), Removal::Uncommitted);
        assert!(wt.join("notes.txt").exists());
        // Staged only.
        run(&wt, &["add", "notes.txt"]);
        assert_eq!(remove("git", &got[0], 4, false), Removal::Uncommitted);
        // The user said yes: removed, changes lost, branch kept.
        assert_eq!(remove("git", &got[0], 4, true), Removal::Removed);
        assert!(!wt.exists());
        assert!(branches(&shop).lines().any(|b| b == "ticket/4"));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn nothing_is_touched_that_doesnt_match_the_tickets_record() {
        let base = dir("guard");
        let shop = repo(&base, "shop");
        let folders = vec![shop.to_string_lossy().into_owned()];
        let got = prepare_worktrees("git", &folders, 8, &[]).unwrap();
        let wt = worktree_dir(&shop, 8);
        let refused = |w: &Worktree, n: u32| matches!(remove("git", w, n, true), Removal::Refused(_));
        // Another ticket's number, another branch, a path elsewhere, a path that climbs out, the repository itself.
        assert!(refused(&got[0], 9));
        assert!(refused(&Worktree { branch: "main".into(), ..got[0].clone() }, 8));
        assert!(refused(&Worktree { path: shop.to_string_lossy().into_owned(), ..got[0].clone() }, 8));
        assert!(refused(&Worktree { path: format!("{}/../../shop", wt.display()), ..got[0].clone() }, 8));
        assert!(refused(&Worktree { path: "relative/ticket-8".into(), ..got[0].clone() }, 8));
        assert!(wt.is_dir() && shop.join("README").exists());
        // The right place, but not a worktree of this repository (a plain folder someone made): left alone.
        let fake = worktree_dir(&shop, 11);
        fs::create_dir_all(&fake).unwrap();
        fs::write(fake.join("keep.txt"), "x").unwrap();
        let w11 = Worktree { path: fake.to_string_lossy().into_owned(), branch: "ticket/11".into(), ..got[0].clone() };
        assert!(refused(&w11, 11));
        assert!(fake.join("keep.txt").exists());
        // A worktree at the right place that has been switched to another branch: left alone.
        run(&wt, &["switch", "-q", "-c", "elsewhere"]);
        assert!(refused(&got[0], 8));
        assert!(wt.is_dir());
        // Its repository gone: nothing to check against, so nothing is touched.
        let w = Worktree { folder: base.join("gone").to_string_lossy().into_owned(), ..got[0].clone() };
        assert!(refused(&w, 8));
        assert!(wt.is_dir());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_removed_worktree_comes_back_from_its_branch_at_the_same_place_with_its_commits_and_base() {
        let base = dir("back");
        let shop = repo(&base, "shop");
        let folders = vec![shop.join("src").to_string_lossy().into_owned()];
        let got = prepare_worktrees("git", &folders, 6, &[]).unwrap();
        let wt = worktree_dir(&shop, 6);
        let tip = commit_in(&wt, "basket.txt", "the basket");
        assert_eq!(remove("git", &got[0], 6, false), Removal::Removed);
        commit_in(&shop, "later.txt", "main moves on");
        // Without the ticket's record the branch name is taken, as before.
        let err = prepare_worktrees("git", &folders, 6, &[]).unwrap_err();
        assert!(err.contains("ticket/6 is already taken"), "{err}");
        assert!(!wt.exists());
        // With it: the same place, on the branch, with its commit and the base it was made from.
        let removed = Worktree { removed: Some(9), ..got[0].clone() };
        let back = prepare_worktrees("git", &folders, 6, &[removed]).unwrap();
        assert_eq!(back, got, "same path, branch and base");
        assert_eq!(run(&wt, &["rev-parse", "HEAD"]), tip);
        assert_eq!(run(&wt, &["rev-parse", "--abbrev-ref", "HEAD"]), "ticket/6");
        assert!(wt.join("basket.txt").exists());
        // Removed by hand (git still has a note of it): made again too.
        fs::remove_dir_all(&wt).unwrap();
        let back = prepare_worktrees("git", &folders, 6, &got).unwrap();
        assert_eq!(back, got);
        assert!(wt.join("basket.txt").exists());
        let _ = fs::remove_dir_all(&base);
    }
}
