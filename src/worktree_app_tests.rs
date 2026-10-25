//! Tasks' worktrees after the work, at the level of the app: removed once a Done or Cancelled task's session has
//! stopped (never while it runs, never from under another session), kept and said when they have changes not yet
//! committed, `c`'s question mentioning such changes, and opening a Done task's session again bringing its worktree
//! back first. Real git in temp repositories only; nothing is started (a unit test's session is only noted).

use crate::app::{App, Confirm, Mode};
use crate::testkit::World;
use crate::tickets::{NewTicket, State, Tickets, Worktree};
use crate::ui;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn draw(term: &mut Terminal<TestBackend>, app: &App) -> String {
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn flat(text: &str) -> String {
    text.replace(['│', '╭', '╮', '╰', '╯', '─'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
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

/// A project "Web shop" with task #1 in the temp repository `shop`, started as session `sess-1`, which made one commit
/// in its worktree; the task is in `state`, and the Tasks tab shows it.
struct Lab {
    w: World,
    app: App,
    project: String,
    shop: PathBuf,
    made: Worktree,
    wt: PathBuf,
    tip: String,
}

fn lab(name: &str, state: State) -> Lab {
    let w = World::new(name);
    w.transcript("user-1", "/tmp", "make a basket page", 10);
    let shop = w.base.join("repos/shop");
    fs::create_dir_all(&shop).unwrap();
    fs::write(shop.join("README"), "hi\n").unwrap();
    git(&shop, &["init", "-q"]);
    git(&shop, &["add", "."]);
    git(&shop, &["commit", "-q", "-m", "first"]);
    let mut app = w.app();
    let project = app.projects.create("Web shop").unwrap();
    app.projects.assign("user-1", Some(&project)).unwrap();
    let folder = shop.to_string_lossy().into_owned();
    let mut t = Tickets::open(&w.config, &project);
    let n = t.add(NewTicket { title: "Basket".into(), folder: Some(folder.clone()), ..Default::default() }).unwrap();
    let made = crate::worktrees::prepare("git", &folder, n, None).unwrap().unwrap();
    t.start(n, "feature", "sess-1", Some(made.clone()), &[]).unwrap();
    let wt = PathBuf::from(&made.path);
    fs::write(wt.join("basket.txt"), "basket\n").unwrap();
    git(&wt, &["add", "."]);
    git(&wt, &["commit", "-q", "-m", "basket"]);
    let tip = git(&wt, &["rev-parse", "HEAD"]);
    t.update(n, |x| {
        x.state = state;
        true
    })
    .unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Char('3'));
    assert!(app.board_showing() && app.ticket(1).is_some());
    Lab { w, app, project, shop, made, wt, tip }
}

impl Lab {
    fn ticket(&self) -> crate::tickets::Ticket {
        Tickets::open(&self.w.config, &self.project).get(1).cloned().unwrap()
    }

    fn registry(&self) -> PathBuf {
        self.w.root.join("sessions").join(format!("{}.json", std::process::id()))
    }

    /// Refresh for `secs` (longer than the look-over's interval) and say whether the worktree is still there.
    fn stays_for(&mut self, secs: f32) -> bool {
        let until = Instant::now() + Duration::from_secs_f32(secs);
        while Instant::now() < until {
            self.app.refresh_for_test();
            std::thread::sleep(Duration::from_millis(50));
        }
        self.app.refresh_until_for_test(10, "the removals to finish", |a| a.worktrees_idle());
        self.wt.is_dir()
    }

    fn branch_tip(&self) -> String {
        git(&self.shop, &["rev-parse", "task/1"])
    }

    fn removed(&self) -> bool {
        self.ticket().worktree.is_some_and(|w| w.removed.is_some())
    }
}

#[test]
fn nothing_is_removed_while_the_session_or_another_one_is_in_it_and_a_clean_worktree_goes_once_they_stop() {
    let mut l = lab("wt-running", State::Done);
    // The Done task's session is still running (this test process stands in for it): the worktree stays.
    l.w.register(std::process::id(), "sess-1", &l.made.path, 1, "idle");
    assert!(l.stays_for(2.5), "removed while the task's session runs");
    assert_eq!(l.ticket().worktree.as_ref(), Some(&l.made), "nothing written");
    // The session stopped, but some other session is working inside the worktree: it still stays.
    l.w.register(std::process::id(), "user-9", &l.wt.join("sub").to_string_lossy(), 1, "idle");
    assert!(l.stays_for(2.5), "removed from under another session");
    // Nobody is in it now: removed, its branch and commit kept, and the task says so.
    fs::remove_file(l.registry()).unwrap();
    let (config, project) = (l.w.config.clone(), l.project.clone());
    l.app.refresh_until_for_test(15, "the worktree removed", |_| {
        Tickets::open(&config, &project).get(1).and_then(|t| t.worktree.clone()).is_some_and(|w| w.removed.is_some())
    });
    assert!(!l.wt.exists());
    assert_eq!(l.branch_tip(), l.tip);
    let t = l.ticket();
    let w = t.worktree.clone().unwrap();
    assert_eq!((w.base.clone(), w.path.clone(), t.state), (l.made.base.clone(), l.made.path.clone(), State::Done), "path and base kept");
    assert_eq!(l.app.active_notice(), Some("Removed #1's worktree (its branch task/1 is kept)."), "said once, in one line");
    // The details say so.
    let mut term = Terminal::new(TestBackend::new(140, 36)).unwrap();
    let f = flat(&draw(&mut term, &l.app));
    assert!(f.contains("Branch task/1 · its worktree was removed after the work"), "{f}");
}

#[test]
fn a_worktree_with_changes_not_yet_committed_is_kept_and_said_once() {
    let mut l = lab("wt-kept", State::Cancelled);
    fs::write(l.wt.join("draft.txt"), "not committed\n").unwrap();
    let (config, project) = (l.w.config.clone(), l.project.clone());
    l.app.refresh_until_for_test(15, "the worktree kept", |_| {
        Tickets::open(&config, &project).get(1).and_then(|t| t.worktree.clone()).is_some_and(|w| w.kept_uncommitted)
    });
    assert!(l.wt.join("draft.txt").exists());
    assert!(!l.removed());
    assert_eq!(l.app.active_notice(), Some("Kept #1's worktree: it has changes not yet committed."));
    // Looked at once for the task as it is: not over and over.
    assert!(l.stays_for(2.5));
    // A platoon started again looks at it again (the changes may have been committed meanwhile), but says nothing.
    let mut again = l.w.app();
    again.refresh_until_for_test(10, "the look-over done", |a| a.worktrees_tried_for_test() > 0 && a.worktrees_idle());
    assert!(again.active_notice().is_none(), "{:?}", again.active_notice());
    assert!(l.wt.join("draft.txt").exists());
    l.app.notice = None;
    let mut term = Terminal::new(TestBackend::new(140, 36)).unwrap();
    let f = flat(&draw(&mut term, &l.app));
    assert!(f.contains("kept: changes not yet committed"), "{f}");
}

#[test]
fn cancelling_a_task_whose_worktree_has_changes_says_platoon_keeps_it() {
    let mut l = lab("wt-cancel", State::Working);
    fs::write(l.wt.join("README"), "changed\n").unwrap();
    key(&mut l.app, KeyCode::Char('c'));
    assert!(matches!(l.app.mode, Mode::Confirm(Confirm::CancelTicket { uncommitted: true, .. })));
    let mut term = Terminal::new(TestBackend::new(140, 36)).unwrap();
    let f = flat(&draw(&mut term, &l.app));
    assert!(f.contains("Its worktree has changes not yet committed, so platoon keeps it."), "{f}");
    key(&mut l.app, KeyCode::Char('y'));
    assert_eq!(l.ticket().state, State::Cancelled);
    let (config, project) = (l.w.config.clone(), l.project.clone());
    l.app.refresh_until_for_test(15, "the worktree kept", |_| {
        Tickets::open(&config, &project).get(1).and_then(|t| t.worktree.clone()).is_some_and(|w| w.kept_uncommitted)
    });
    assert!(l.wt.join("README").exists());
    // A clean one: the question doesn't mention it.
    let mut l2 = lab("wt-cancel-clean", State::Working);
    key(&mut l2.app, KeyCode::Char('c'));
    assert!(matches!(l2.app.mode, Mode::Confirm(Confirm::CancelTicket { uncommitted: false, .. })));
}

#[test]
fn opening_a_done_tasks_session_whose_worktree_was_removed_makes_it_again_from_its_branch_first() {
    let mut l = lab("wt-continue", State::Done);
    let (config, project) = (l.w.config.clone(), l.project.clone());
    l.app.refresh_until_for_test(15, "the worktree removed", |_| {
        Tickets::open(&config, &project).get(1).and_then(|t| t.worktree.clone()).is_some_and(|w| w.removed.is_some())
    });
    assert!(!l.wt.exists());
    // The main repository moves on meanwhile.
    fs::write(l.shop.join("later.txt"), "later\n").unwrap();
    git(&l.shop, &["add", "."]);
    git(&l.shop, &["commit", "-q", "-m", "later"]);
    let mut term = Terminal::new(TestBackend::new(140, 36)).unwrap();
    assert!(flat(&draw(&mut term, &l.app)).contains("enter its session"));
    key(&mut l.app, KeyCode::Enter);
    assert!(l.app.sq.launched.is_empty(), "not before the worktree is back");
    l.app.refresh_until_for_test(15, "the session continued", |a| a.worktrees_idle() && !a.sq.launched.is_empty());
    assert_eq!(l.app.sq.launched, [1]);
    // Resumed (not a new session), in the worktree, which is back where it was with the session's commit.
    let (dir, args) = l.app.sq.launched_with[0].clone();
    assert_eq!(dir, l.made.path);
    assert!(args.windows(2).any(|a| a == ["--resume", "sess-1"]), "{args:?}");
    assert_eq!(git(&l.wt, &["rev-parse", "HEAD"]), l.tip);
    assert_eq!(git(&l.wt, &["rev-parse", "--abbrev-ref", "HEAD"]), "task/1");
    assert!(!l.wt.join("later.txt").exists(), "on its own branch, not main");
    let t = l.ticket();
    assert_eq!(t.worktree.as_ref(), Some(&l.made), "no longer marked removed; same path, branch and base");
    assert_eq!(t.state, State::Done, "Working again only when the user writes to it (the hook)");
}
