//! Tickets' worktrees after the work, at the level of the app: removed once a Done or Cancelled ticket's squad has
//! stopped (never while it runs, never from under another session), kept and said on the ticket's screen when they
//! have changes not yet committed, `w` to remove one anyway after a yes, `c`'s question mentioning such changes, and
//! continuing a squad bringing its worktree back first. Real git in temp repositories only; nothing is started (a
//! unit test's leader is only noted).

use crate::app::{App, Confirm, Mode};
use crate::testkit::World;
use crate::tickets::{Leader, NewTicket, State, Tickets, Worktree};
use crate::ui;
use ratatui::{
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    Terminal,
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
    (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>()).collect::<Vec<_>>().join("\n")
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

/// A project "Web shop" (its screen open) with ticket #1 in the temp repository `shop`, handed to a squad whose
/// leader `lead-1` made one commit in its worktree; the ticket is in `state`.
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
    let folders = vec![shop.to_string_lossy().into_owned()];
    let mut t = Tickets::open(&w.config, &project);
    let n = t.add(NewTicket { title: "Basket".into(), folders: folders.clone(), ..Default::default() }).unwrap();
    let made = crate::squads::prepare_worktrees("git", &folders, n, &[]).unwrap().remove(0);
    let lead = Leader { squad: "coding".into(), session: "lead-1".into(), started: 1, names: Default::default(), snapshot: None };
    t.start_leader(n, &lead, std::slice::from_ref(&made)).unwrap();
    let wt = PathBuf::from(&made.path);
    fs::write(wt.join("basket.txt"), "basket\n").unwrap();
    git(&wt, &["add", "."]);
    git(&wt, &["commit", "-q", "-m", "basket"]);
    let tip = git(&wt, &["rev-parse", "HEAD"]);
    if state == State::Cancelled {
        t.set_state(n, &State::Cancelled).unwrap();
    } else {
        t.squad_says(n, &state, "", None).unwrap();
    }
    app.reload_for_test();
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Enter);
    assert!(app.open_project().is_some());
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
        git(&self.shop, &["rev-parse", "ticket/1"])
    }
}

#[test]
fn nothing_is_removed_while_the_squad_or_another_session_is_in_it_and_a_clean_worktree_goes_once_they_stop() {
    let mut l = lab("wt-running", State::Done);
    // The Done ticket's leader is still running (this test process stands in for it): the worktree stays.
    l.w.register(std::process::id(), "lead-1", &l.made.path, 1, "idle");
    assert!(l.stays_for(2.5), "removed while the squad's session runs");
    assert_eq!(l.ticket().worktrees[0], l.made, "nothing written");
    // The leader stopped, but some other session is working inside the worktree: it still stays.
    l.w.register(std::process::id(), "user-9", &l.wt.join("sub").to_string_lossy(), 1, "idle");
    assert!(l.stays_for(2.5), "removed from under another session");
    // Nobody is in it now: removed, its branch and commit kept, and the ticket says so.
    fs::remove_file(l.registry()).unwrap();
    l.app.refresh_until_for_test(15, "the worktree removed", |a| a.ticket(1).is_some_and(|t| t.worktrees[0].removed.is_some()));
    assert!(!l.wt.exists());
    assert_eq!(l.branch_tip(), l.tip);
    let t = l.ticket();
    assert_eq!((t.worktrees[0].base.clone(), t.worktrees[0].path.clone(), t.state), (l.made.base.clone(), l.made.path.clone(), State::Done), "path and base kept");
    assert_eq!(l.app.active_notice(), Some("Removed #1's worktree (its branch ticket/1 is kept)."), "said once, in one line");
    // The screen says so.
    l.app.tk.open = Some(1);
    let mut term = Terminal::new(TestBackend::new(140, 36)).unwrap();
    let f = flat(&draw(&mut term, &l.app));
    assert!(f.contains("shop-worktrees/ticket-1 Removed; its branch ticket/1 is kept."), "{f}");
    assert!(!f.contains("remove worktree"), "{f}");
}

#[test]
fn a_worktree_with_changes_not_yet_committed_is_kept_said_on_its_screen_and_w_removes_it_after_a_yes() {
    let mut l = lab("wt-kept", State::Cancelled);
    fs::write(l.wt.join("draft.txt"), "not committed\n").unwrap();
    l.app.refresh_until_for_test(15, "the worktree kept", |a| a.ticket(1).is_some_and(|t| t.worktrees[0].kept_uncommitted));
    assert!(l.wt.join("draft.txt").exists());
    assert_eq!(l.ticket().worktrees[0].removed, None);
    assert_eq!(l.app.active_notice(), Some("Kept #1's worktree: it has changes not yet committed (w on the ticket removes it)."));
    // Looked at once for the ticket as it is: not over and over.
    assert!(l.stays_for(2.5));
    // A platoon started again looks at it again (the changes may have been committed meanwhile), but says nothing:
    // it was said the first time.
    let mut again = l.w.app();
    again.refresh_until_for_test(10, "the look-over done", |a| a.worktrees_tried_for_test() > 0 && a.worktrees_idle());
    assert!(again.active_notice().is_none(), "{:?}", again.active_notice());
    assert!(l.wt.join("draft.txt").exists());
    l.app.notice = None; // (it covers the footer's key hints while it shows)
    l.app.tk.open = Some(1);
    let mut term = Terminal::new(TestBackend::new(140, 36)).unwrap();
    let shown = draw(&mut term, &l.app);
    let f = flat(&shown);
    assert!(f.contains("Kept because it has changes not yet committed."), "{shown}");
    assert!(f.contains("w removes it anyway"), "{shown}");
    assert!(f.contains("w remove worktree"), "the footer too: {shown}");
    // Its key in the accent, like every key hint.
    let t = l.app.effective_theme();
    let buf = term.backend().buffer().clone();
    let (y, x) = shown.lines().enumerate().find_map(|(y, line)| line.find("w removes it anyway").map(|b| (y, line[..b].chars().count()))).unwrap();
    assert_eq!(buf[(x as u16, y as u16)].fg, t.accent);
    assert_eq!(buf[(x as u16 + 2, y as u16)].fg, t.dim);
    let (ky, kx) = shown.lines().enumerate().find_map(|(y, line)| line.find("Kept because").map(|b| (y, line[..b].chars().count()))).unwrap();
    assert_eq!(buf[(kx as u16, ky as u16)].fg, t.warn);
    // `w`: asked first, in plain words.
    key(&mut l.app, KeyCode::Char('w'));
    assert!(matches!(l.app.mode, Mode::Confirm(Confirm::RemoveWorktree { number: 1, .. })));
    let f = flat(&draw(&mut term, &l.app));
    assert!(f.contains("Remove #1's worktree anyway?"), "{f}");
    assert!(f.contains("It has changes not yet committed, and they will be lost. Its branch ticket/1 and everything committed on it are kept."), "{f}");
    assert!(f.contains("y remove it") && f.contains("n keep it"), "{f}");
    // No: nothing happens.
    key(&mut l.app, KeyCode::Char('n'));
    assert!(matches!(l.app.mode, Mode::List));
    assert!(l.stays_for(0.3));
    // Yes: removed with its changes, branch kept.
    key(&mut l.app, KeyCode::Char('w'));
    key(&mut l.app, KeyCode::Char('y'));
    l.app.refresh_until_for_test(15, "the worktree removed", |a| a.ticket(1).is_some_and(|t| t.worktrees[0].removed.is_some()));
    assert!(!l.wt.exists());
    assert_eq!(l.branch_tip(), l.tip);
    assert!(!l.ticket().worktrees[0].kept_uncommitted);
    assert_eq!(l.app.active_notice(), Some("#1's worktree is removed. Its branch ticket/1 is kept."));
}

#[test]
fn w_is_refused_while_the_squad_runs_or_when_nothing_was_kept() {
    let mut l = lab("wt-refuse", State::Done);
    l.app.tk.open = Some(1);
    l.w.register(std::process::id(), "lead-1", &l.made.path, 1, "idle");
    l.app.refresh_for_test();
    key(&mut l.app, KeyCode::Char('w'));
    assert!(matches!(l.app.mode, Mode::List));
    assert_eq!(l.app.active_notice(), Some("#1 has no worktree kept after the work."));
    // Kept, but the squad's session is running in it.
    fs::write(l.wt.join("draft.txt"), "x\n").unwrap();
    Tickets::open(&l.w.config, &l.project).mark_worktree(1, &l.made.path, crate::tickets::WtMark::KeptUncommitted).unwrap();
    l.app.refresh_for_test();
    key(&mut l.app, KeyCode::Char('w'));
    assert!(matches!(l.app.mode, Mode::List));
    assert_eq!(l.app.active_notice(), Some("#1's squad's session is running in its worktree. Stop it first."));
    assert!(l.wt.join("draft.txt").exists());
    let _ = fs::remove_file(l.registry());
}

#[test]
fn cancelling_a_ticket_whose_worktree_has_changes_says_platoon_keeps_it() {
    let mut l = lab("wt-cancel", State::Working);
    fs::write(l.wt.join("README"), "changed\n").unwrap();
    l.app.tk.open = Some(1);
    key(&mut l.app, KeyCode::Char('c'));
    assert!(matches!(l.app.mode, Mode::Confirm(Confirm::CancelTicket { uncommitted: true, .. })));
    let mut term = Terminal::new(TestBackend::new(140, 36)).unwrap();
    let f = flat(&draw(&mut term, &l.app));
    // Wrapped in the popup: each of its lines.
    assert!(f.contains("Its worktree has changes not yet committed, so") && f.contains("platoon keeps it: w on the ticket removes it anyway."), "{f}");
    key(&mut l.app, KeyCode::Char('y'));
    assert_eq!(l.ticket().state, State::Cancelled);
    l.app.refresh_until_for_test(15, "the worktree kept", |a| a.ticket(1).is_some_and(|t| t.worktrees[0].kept_uncommitted));
    assert!(l.wt.join("README").exists());
    // A clean one: the question doesn't mention it.
    git(&l.wt, &["checkout", "--", "README"]);
    let mut l2 = lab("wt-cancel-clean", State::Working);
    l2.app.tk.open = Some(1);
    key(&mut l2.app, KeyCode::Char('c'));
    assert!(matches!(l2.app.mode, Mode::Confirm(Confirm::CancelTicket { uncommitted: false, .. })));
}

#[test]
fn continuing_a_done_ticket_whose_worktree_was_removed_makes_it_again_from_its_branch_first() {
    let mut l = lab("wt-continue", State::Done);
    l.app.refresh_until_for_test(15, "the worktree removed", |a| a.ticket(1).is_some_and(|t| t.worktrees[0].removed.is_some()));
    assert!(!l.wt.exists());
    // The main repository moves on meanwhile.
    fs::write(l.shop.join("later.txt"), "later\n").unwrap();
    git(&l.shop, &["add", "."]);
    git(&l.shop, &["commit", "-q", "-m", "later"]);
    l.app.tk.open = Some(1);
    let mut term = Terminal::new(TestBackend::new(140, 36)).unwrap();
    assert!(flat(&draw(&mut term, &l.app)).contains("enter continue the squad"));
    key(&mut l.app, KeyCode::Enter);
    assert!(l.app.sq.launched.is_empty(), "not before the worktree is back");
    l.app.refresh_until_for_test(15, "the squad continued", |a| a.worktrees_idle() && !a.sq.launched.is_empty());
    assert_eq!(l.app.sq.launched, [1]);
    // Resumed (not a new session), in the worktree, which is back where it was with the squad's commit.
    let (dir, args) = l.app.sq.launched_with[0].clone();
    assert_eq!(dir, l.made.path);
    assert!(args.windows(2).any(|a| a == ["--resume", "lead-1"]), "{args:?}");
    assert_eq!(git(&l.wt, &["rev-parse", "HEAD"]), l.tip);
    assert_eq!(git(&l.wt, &["rev-parse", "--abbrev-ref", "HEAD"]), "ticket/1");
    assert!(!l.wt.join("later.txt").exists(), "on its own branch, not main");
    let t = l.ticket();
    assert_eq!(t.worktrees[0], l.made, "no longer marked removed; same path, branch and base");
    assert_eq!(t.state, State::Done, "Working again only when the user writes to it (the hook)");
    assert!(!l.app.ticket(1).unwrap().busy.is_some());
}

#[test]
fn the_notice_says_each_ticket_once_in_plain_words() {
    use crate::app::worktree_glue::removal_notice;
    assert_eq!(removal_notice(7, 0, 0), None);
    assert_eq!(removal_notice(7, 1, 0).as_deref(), Some("Removed #7's worktree (its branch ticket/7 is kept)."));
    assert_eq!(removal_notice(7, 2, 0).as_deref(), Some("Removed #7's worktrees (2); their branch ticket/7 is kept."));
    assert_eq!(removal_notice(7, 0, 1).as_deref(), Some("Kept #7's worktree: it has changes not yet committed (w on the ticket removes it)."));
    assert_eq!(removal_notice(7, 0, 2).as_deref(), Some("Kept #7's worktrees (2): they have changes not yet committed (w on the ticket removes them)."));
    assert_eq!(
        removal_notice(7, 1, 1).as_deref(),
        Some("Removed #7's worktree (its branch ticket/7 is kept); kept one with changes not yet committed (w on the ticket removes it).")
    );
}

#[test]
fn a_ticket_with_worktrees_in_two_repositories_gets_one_notice_for_both() {
    let w = World::new("wt-two");
    w.transcript("user-1", "/tmp", "make a basket page", 10);
    let mut folders = Vec::new();
    for name in ["shop", "api"] {
        let repo = w.base.join("repos").join(name);
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("README"), "hi\n").unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "first"]);
        folders.push(repo.to_string_lossy().into_owned());
    }
    let mut app = w.app();
    let project = app.projects.create("Web shop").unwrap();
    let mut t = Tickets::open(&w.config, &project);
    let n = t.add(NewTicket { title: "Basket".into(), folders: folders.clone(), ..Default::default() }).unwrap();
    let made = crate::squads::prepare_worktrees("git", &folders, n, &[]).unwrap();
    assert_eq!(made.len(), 2);
    let lead = Leader { squad: "coding".into(), session: "lead-1".into(), started: 1, names: Default::default(), snapshot: None };
    t.start_leader(n, &lead, &made).unwrap();
    t.squad_says(n, &State::Done, "", None).unwrap();
    app.reload_for_test();
    let config = w.config.clone();
    app.refresh_until_for_test(15, "both removed", |a| a.worktrees_idle() && Tickets::open(&config, &project).get(1).is_some_and(|t| t.worktrees.iter().all(|w| w.removed.is_some())));
    assert_eq!(app.active_notice(), Some("Removed #1's worktrees (2); their branch ticket/1 is kept."));
    for m in &made {
        assert!(!Path::new(&m.path).exists());
    }
}
