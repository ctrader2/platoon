//! Tasks at the level of the app, as of the rename (2026-10): the hand-over popup (`S`: the squads, "On its own", one
//! line per folder, what the work lands on) at 120x40 and 80x24 with one and three folders, the branch option's three
//! conditions and their refusals, the task's screen's new lines, the Tasks pane's state words and their colors at wide
//! and narrow widths, and the move of a project's task files from their old names. Every directory is a temp dir
//! (`World`); git runs only in throwaway repositories made here; no session is ever started (the program would be the
//! test binary).

use crate::app::{App, Mode, NamePurpose, Pane};
use crate::testkit::World;
use crate::tickets::{Autonomy, How, NewTicket, State, Tickets};
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
};

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn typed(app: &mut App, text: &str) {
    for c in text.chars() {
        key(app, KeyCode::Char(c));
    }
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

/// Where `needle` starts on the drawn screen (row, column in cells).
fn find(term: &Terminal<TestBackend>, needle: &str) -> Option<(u16, u16)> {
    let buf = term.backend().buffer();
    for y in 0..buf.area.height {
        let line: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
        if let Some(at) = line.find(needle) {
            return Some((y, line[..at].chars().count() as u16));
        }
    }
    None
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A temp repository with one commit on `main`.
fn repo(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("README.md"), "# Shop\n").unwrap();
    git(dir, &["init", "-q"]);
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "first"]);
}

/// A project "Web shop" with one task "Basket" in `folders`, its screen open on the Tasks pane, the task picked, and a
/// stand-in for `claude` that says every squad fits.
fn with_task(name: &str, folders: &[PathBuf]) -> (World, App, String, crate::testkit::AiFake) {
    let w = World::new(name);
    w.transcript("user-1", "/tmp", "make a basket page", 10);
    let mut s = crate::settings::Settings { tokens_enabled: true, tokens_asked: true, ..Default::default() };
    s.auto_archive_days = 0;
    crate::settings::save(&w.config, &s).unwrap();
    let mut app = w.app();
    let fake = crate::testkit::AiFake::new(name);
    fake.reply_default(r#"{"fits":true,"reason":"It fits."}"#);
    app.set_ai_program_for_test(&fake.script);
    let id = app.projects.create("Web shop").unwrap();
    app.projects.assign("user-1", Some(&id)).unwrap();
    let folders: Vec<String> = folders.iter().map(|f| f.to_string_lossy().into_owned()).collect();
    Tickets::open(&w.config, &id).add(NewTicket { title: "Basket".into(), folders, ..Default::default() }).unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Char('3'));
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.pane, Pane::Tickets);
    (w, app, id, fake)
}

/// The popup's lines, as drawn (the rows between its top and bottom borders, each trimmed of the border).
fn popup_lines(shown: &str) -> Vec<String> {
    let lines: Vec<&str> = shown.lines().collect();
    let top = lines.iter().position(|l| l.contains("╭ Hand to a squad")).expect("the popup");
    let left = lines[top].chars().position(|c| c == '╭').unwrap();
    let right = left + lines[top].chars().skip(left + 1).position(|c| c == '╮').unwrap() + 1;
    lines[top + 1..]
        .iter()
        .take_while(|l| !l.chars().nth(left).is_some_and(|c| c == '╰'))
        .map(|l| l.chars().skip(left + 1).take(right - left - 1).collect::<String>().trim_end().to_string())
        .collect()
}

#[test]
fn the_hand_over_popup_lists_the_squads_then_on_its_own_then_one_line_per_folder_and_fits_small_screens() {
    let base = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-hand-popup-{}", std::process::id())));
    let _ = fs::remove_dir_all(&base);
    let (shop, api, docs) = (base.join("shop"), base.join("api"), base.join("docs"));
    repo(&shop);
    repo(&api);
    fs::create_dir_all(&docs).unwrap();
    let (_w, mut app, _id, _fake) = with_task("hand-popup", &[shop.clone(), api.clone(), docs.clone()]);
    key(&mut app, KeyCode::Char('S'));
    assert!(matches!(app.mode, Mode::SquadPick), "{:?}", app.active_notice());
    for (w, h) in [(120u16, 40u16), (80, 24)] {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let shown = draw(&mut term, &app);
        let lines = popup_lines(&shown);
        let f = flat(&lines.join("\n"));
        assert!(f.starts_with("#1 Basket"), "{w}x{h}: the task first: {shown}");
        // The squads by name; only the picked one says what it can do.
        assert!(f.contains("▎ ● Coding Makes a change to code in a git repository"), "{w}x{h}: {shown}");
        assert!(f.contains("Research") && f.contains("Docs") && f.contains("Summarize"), "{w}x{h}: {shown}");
        assert!(!f.contains("Finds things out"), "{w}x{h}: only the picked squad's text: {shown}");
        // Then the lines, pre-filled.
        assert!(f.contains("On its own: asks before anything others can see"), "{w}x{h}: {shown}");
        assert!(f.contains("shop: a copy on task/1, lands on main"), "{w}x{h}: {shown}");
        assert!(f.contains("api: a copy on task/1, lands on main"), "{w}x{h}: {shown}");
        assert!(f.contains("docs: used as it is (not in git)"), "{w}x{h}: {shown}");
        assert!(f.ends_with("enter hand it over space pick ↑↓ move esc cancel"), "{w}x{h}: the footer, on a squad: {shown}");
        // Nothing cut mid-word: every popup line ends with a whole word (or the border's space).
        for l in &lines {
            assert!(!l.ends_with('…'), "{w}x{h}: cut: {l:?}\n{shown}");
        }
        // The whole popup is on the screen, bottom border included.
        assert!(find(&term, "╰").is_some() && shown.lines().any(|l| l.contains("╰") && l.contains("╯")), "{w}x{h}: {shown}");
        // The cursor's row is drawn on the selection fill.
        let t = app.effective_theme().clone();
        let (y, x) = find(&term, "▎ ● Coding").unwrap();
        assert_eq!(term.backend().buffer()[(x + 4, y)].bg, t.surface, "{w}x{h}");
        assert_eq!(term.backend().buffer()[(x + 4, y)].fg, t.accent, "{w}x{h}: the picked squad's name stands out");
        let (y, x) = find(&term, "○ Research").unwrap();
        assert_eq!(term.backend().buffer()[(x + 2, y)].fg, t.text, "{w}x{h}");
        let (y, x) = find(&term, "On its own:").unwrap();
        assert_eq!(term.backend().buffer()[(x, y)].bg, t.bg, "{w}x{h}: not the cursor's row");
    }
    // Down through the squads to "On its own": the picked squad stays Coding (walking past the others picks none),
    // the footer offers the change, and space makes it.
    for _ in 0..4 {
        key(&mut app, KeyCode::Down);
    }
    assert!(app.hand_on_autonomy() && app.sq.pick_selected == 0);
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let shown = draw(&mut term, &app);
    let f = flat(&popup_lines(&shown).join("\n"));
    assert!(f.contains("● Coding Makes a change to code") && !f.contains("Finds things out"), "{shown}");
    assert!(f.ends_with("enter hand it over space change ↑↓ move esc cancel"), "{shown}");
    let t = app.effective_theme().clone();
    let (y, x) = find(&term, "On its own:").unwrap();
    assert_eq!(term.backend().buffer()[(x, y)].bg, t.surface, "the cursor's row now");
    key(&mut app, KeyCode::Char(' '));
    assert!(flat(&draw(&mut term, &app)).contains("On its own: works and publishes"));
    key(&mut app, KeyCode::Char(' '));
    assert!(flat(&draw(&mut term, &app)).contains("On its own: plans first, then waits for your yes"), "the third level");
    key(&mut app, KeyCode::Char(' '));
    assert!(flat(&draw(&mut term, &app)).contains("On its own: asks before anything others can see"), "round again");
    // Down to shop: space makes it a new branch in the checkout (nothing else works there), and again a copy.
    key(&mut app, KeyCode::Down);
    assert_eq!(app.hand_folder_at_cursor(), Some(0));
    let shown = draw(&mut term, &app);
    assert!(flat(&popup_lines(&shown).join("\n")).ends_with("enter hand it over space change l lands on esc cancel"), "{shown}");
    key(&mut app, KeyCode::Char(' '));
    assert!(flat(&draw(&mut term, &app)).contains("shop: a new branch task/1 in your checkout, lands on main"));
    key(&mut app, KeyCode::Char(' '));
    assert!(flat(&draw(&mut term, &app)).contains("shop: a copy on task/1, lands on main"));
    // `l`: the branch the work lands on, checked against git; a wrong one keeps the box open and says why.
    key(&mut app, KeyCode::Char('l'));
    assert!(matches!(app.mode, Mode::ProjectName) && matches!(app.name_box.purpose, NamePurpose::LandsOn(0)));
    assert_eq!(app.name_box.input, "main", "starts from what it is");
    let shown = draw(&mut term, &app);
    assert!(shown.contains("╭ Lands on") && flat(&shown).contains("The branch the work is meant for"), "{shown}");
    app.name_box.input.clear();
    typed(&mut app, "nope");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::ProjectName), "the box stays open");
    assert_eq!(app.active_notice(), Some("There is no branch nope in shop."));
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::SquadPick), "esc goes back to the popup");
    git(&shop, &["branch", "release"]);
    key(&mut app, KeyCode::Char('l'));
    app.name_box.input.clear();
    typed(&mut app, "release");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::SquadPick));
    assert!(flat(&draw(&mut term, &app)).contains("shop: a copy on task/1, lands on release"));
    // On docs (not in git): space and l only say so.
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    assert_eq!(app.hand_folder_at_cursor(), Some(2));
    key(&mut app, KeyCode::Char(' '));
    assert_eq!(app.active_notice(), Some("docs isn't in git: it is used as it is."));
    key(&mut app, KeyCode::Char('l'));
    assert!(matches!(app.mode, Mode::SquadPick));
    assert_eq!(app.active_notice(), Some("docs isn't in git, so its work lands on no branch."));
    // Up past the lines and the squads: the cursor stops at the first squad, and the first squad stays picked.
    for _ in 0..20 {
        key(&mut app, KeyCode::Up);
    }
    assert_eq!((app.sq.form.cursor, app.sq.pick_selected), (0, 0));
    // Space on a squad picks it: its text moves under it, and enter from any row would hand the task to it.
    key(&mut app, KeyCode::Down);
    assert_eq!(app.sq.pick_selected, 0, "moving onto Research doesn't pick it");
    key(&mut app, KeyCode::Char(' '));
    assert_eq!(app.sq.pick_selected, 1);
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("▎ ● Research Finds things out") && !f.contains("Makes a change to code"), "{f}");
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn enter_writes_the_terms_on_the_task_before_the_commander_and_the_leader_gets_them() {
    let base = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-hand-terms-{}", std::process::id())));
    let _ = fs::remove_dir_all(&base);
    let shop = base.join("shop");
    repo(&shop);
    git(&shop, &["branch", "release"]);
    let (w, mut app, id, fake) = with_task("hand-terms", std::slice::from_ref(&shop));
    // The commander says no the first time: the terms are kept for the next try.
    fake.reply_default(r#"{"fits":false,"reason":"Not this one."}"#);
    key(&mut app, KeyCode::Char('S'));
    for _ in 0..5 {
        key(&mut app, KeyCode::Down);
    }
    key(&mut app, KeyCode::Char(' '));
    key(&mut app, KeyCode::Char('l'));
    app.name_box.input.clear();
    typed(&mut app, "release");
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Char(' '));
    assert_eq!(app.sq.form.autonomy, Autonomy::Publish);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));
    let t = Tickets::open(&w.config, &id).get(1).cloned().unwrap();
    assert_eq!(t.autonomy, Some(Autonomy::Publish), "written before the check: {t:?}");
    assert_eq!((t.works_on[0].how, t.works_on[0].lands_on.as_str()), (How::Branch, "release"));
    app.refresh_until_for_test(20, "the commander's answer", |a| !a.squads_busy());
    assert_eq!(app.ticket(1).unwrap().state, State::New);
    // The screen says what was set, even now.
    key(&mut app, KeyCode::Enter);
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("On its own works and publishes"), "{f}");
    assert!(f.contains("Works in shop, on a new branch task/1 · lands on release"), "{f}");
    key(&mut app, KeyCode::Esc);
    // The next try starts from them; this time the commander says yes, and the leader is started with them.
    fake.reply_default(r#"{"fits":true,"reason":"Fine."}"#);
    key(&mut app, KeyCode::Char('S'));
    assert!(matches!(app.mode, Mode::SquadPick), "{:?}", app.active_notice());
    assert_eq!(app.sq.form.autonomy, Autonomy::Publish);
    assert_eq!((app.sq.form.folders[0].how, app.sq.form.folders[0].lands_on.as_str()), (How::Branch, "release"));
    let f = flat(&draw(&mut term, &app));
    assert!(
        f.contains("On its own: works and publishes") && f.contains("shop: a new branch task/1 in your checkout, lands on release"),
        "{f}"
    );
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('y')); // "Hand it anyway?" after the commander's earlier no
    app.refresh_until_for_test(20, "the leader to start", |a| !a.sq.launched.is_empty());
    let t = Tickets::open(&w.config, &id).get(1).cloned().unwrap();
    assert!(t.worktrees.is_empty(), "a branch in the checkout, no worktree: {t:?}");
    assert_eq!(git(&shop, &["symbolic-ref", "--short", "HEAD"]), "task/1");
    assert_eq!(git(&shop, &["rev-parse", "HEAD"]), git(&shop, &["rev-parse", "release"]));
    let (dir, args) = app.sq.launched_with[0].clone();
    assert_eq!(dir, shop.to_string_lossy(), "the squad starts in the user's own checkout");
    let agents: serde_json::Value = serde_json::from_str(&args[args.iter().position(|a| a == "--agents").unwrap() + 1]).unwrap();
    let prompt = agents["coding-leader"]["prompt"].as_str().unwrap();
    assert!(prompt.contains("The user has let this squad publish on its own"), "{prompt}");
    assert!(prompt.contains("In shop you work in the user's own checkout, on the branch task/1"), "{prompt}");
    assert!(prompt.contains("The work is meant for the branch release of shop"), "{prompt}");
    let first = args.last().unwrap();
    assert!(
        first.contains("(the user's own checkout, on the branch task/1 made for this task, made from release; the work lands on release"),
        "{first}"
    );
    // The gate lets a push through for this task.
    let sid = t.leader().unwrap().session.clone();
    let input = serde_json::json!({"session_id": sid, "tool_name": "Bash", "tool_input": {"command": "git push"}}).to_string();
    assert_eq!(crate::squads::squad_gate(&input, &w.config), None);
    // Its screen: the new lines, in the label/value shape, and the squad.
    key(&mut app, KeyCode::Enter);
    let f = flat(&draw(&mut term, &app));
    assert!(
        f.contains("Squad Coding squad Tests none set for this project On its own works and publishes Works in shop, on a new branch task/1 · lands on release"),
        "{f}"
    );
    assert!(f.contains("Its own folder"), "{f}");
}

#[test]
fn a_copy_is_made_from_the_branch_the_work_lands_on_and_the_screen_says_so() {
    let base = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-hand-copy-{}", std::process::id())));
    let _ = fs::remove_dir_all(&base);
    let shop = base.join("shop");
    repo(&shop);
    git(&shop, &["switch", "-q", "-c", "develop"]);
    fs::write(shop.join("dev.txt"), "dev\n").unwrap();
    git(&shop, &["add", "."]);
    git(&shop, &["commit", "-q", "-m", "dev"]);
    git(&shop, &["switch", "-q", "main"]);
    let (w, mut app, id, _fake) = with_task("hand-copy", std::slice::from_ref(&shop));
    key(&mut app, KeyCode::Char('S'));
    for _ in 0..5 {
        key(&mut app, KeyCode::Down);
    }
    key(&mut app, KeyCode::Char('l'));
    app.name_box.input.clear();
    typed(&mut app, "develop");
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Enter);
    app.refresh_until_for_test(20, "the leader to start", |a| !a.sq.launched.is_empty());
    let t = Tickets::open(&w.config, &id).get(1).cloned().unwrap();
    let wt = base.join("shop-worktrees/task-1");
    assert_eq!(t.worktrees[0].path, wt.to_string_lossy());
    assert_eq!(t.worktrees[0].base.as_deref(), Some(git(&shop, &["rev-parse", "develop"]).as_str()), "from what the work lands on");
    assert!(wt.join("dev.txt").exists());
    assert_eq!(git(&shop, &["symbolic-ref", "--short", "HEAD"]), "main", "the user's checkout untouched");
    key(&mut app, KeyCode::Enter);
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let shown = draw(&mut term, &app);
    let f = flat(&shown);
    assert!(f.contains("On its own asks before anything others can see"), "{f}");
    assert!(f.contains("Works in a copy of shop (task/1) · lands on develop"), "{f}");
    assert!(f.contains("shop-worktrees/task-1"), "the copy's path under it: {f}");
    let t = app.effective_theme().clone();
    let (y, x) = find(&term, "On its own").unwrap();
    assert_eq!(term.backend().buffer()[(x, y)].fg, t.dim, "labels are quiet");
    let (y, x) = find(&term, "a copy of shop").unwrap();
    assert_eq!(term.backend().buffer()[(x, y)].fg, t.text);
    let (y, x) = find(&term, "shop-worktrees/task-1").unwrap();
    assert_eq!(term.backend().buffer()[(x, y)].fg, t.dim, "the path is secondary");
}

#[test]
fn a_new_branch_in_the_checkout_is_offered_only_when_nothing_else_works_there() {
    let base = crate::testkit::removed_at_exit(std::env::temp_dir().join(format!("platoon-hand-blocked-{}", std::process::id())));
    let _ = fs::remove_dir_all(&base);
    let shop = base.join("shop");
    repo(&shop);
    let (mut w, mut app, id, _fake) = with_task("hand-blocked", std::slice::from_ref(&shop));
    let line = |app: &mut App| {
        // #1 (other tasks may come before it in the pane: a Working one does).
        app.tk.selected = Some(crate::app::TRow::Ticket(1));
        key(app, KeyCode::Char('S'));
        assert!(matches!(app.mode, Mode::SquadPick), "{:?}", app.active_notice());
        let l = app.sq.form.folders[0].clone();
        key(app, KeyCode::Esc);
        l
    };
    assert_eq!(line(&mut app).no_branch, None, "a clean checkout, nobody in it");
    // 1. Changes not yet committed.
    fs::write(shop.join("wip.txt"), "wip\n").unwrap();
    let l = line(&mut app);
    assert_eq!(l.no_branch.as_deref(), Some("shop has changes not yet committed"));
    assert_eq!(l.how, How::Worktree);
    key(&mut app, KeyCode::Char('S'));
    for _ in 0..5 {
        key(&mut app, KeyCode::Down);
    }
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let f = flat(&draw(&mut term, &app));
    // (The pane's rows behind the popup share its lines, so the two parts are looked for apart.)
    // (The reason wraps in the popup, and the pane's row behind it may sit between its two lines.)
    assert!(
        f.contains("shop: a copy on task/1, lands on main")
            && f.contains("a new branch in your checkout: not now, shop has changes not yet")
            && f.contains("committed"),
        "{f}"
    );
    key(&mut app, KeyCode::Char(' '));
    assert_eq!(
        app.active_notice(),
        Some("A new branch in your checkout isn't offered for shop just now: shop has changes not yet committed.")
    );
    assert_eq!(app.sq.form.folders[0].how, How::Worktree, "unchanged");
    key(&mut app, KeyCode::Esc);
    fs::remove_file(shop.join("wip.txt")).unwrap();
    assert_eq!(line(&mut app).no_branch, None);
    // 2. Another open task works there on a branch already (in any project).
    let mut other = Tickets::open(&w.config, &id);
    let n =
        other.add(NewTicket { title: "Other".into(), folders: vec![shop.to_string_lossy().into_owned()], ..Default::default() }).unwrap();
    other
        .set_terms(
            n,
            &Autonomy::Ask,
            &[crate::tickets::WorksOn { folder: shop.to_string_lossy().into_owned(), how: How::Branch, lands_on: "main".into() }],
        )
        .unwrap();
    other
        .start_leader(
            n,
            &crate::tickets::Leader {
                squad: "coding".into(),
                session: "lead-o".into(),
                started: 1,
                names: Default::default(),
                snapshot: None,
            },
            &[],
        )
        .unwrap();
    app.reload_for_test();
    assert_eq!(line(&mut app).no_branch.as_deref(), Some("#2 already works in shop on a branch"));
    Tickets::open(&w.config, &id).squad_says(n, &State::Done, "", Some("Done.")).unwrap();
    app.reload_for_test();
    assert_eq!(line(&mut app).no_branch, None, "Done: the checkout is free again");
    // 3. A session of the user's is running in the folder.
    w.live("mine-1", shop.to_str().unwrap(), 1, "idle");
    app.refresh_for_test();
    assert_eq!(line(&mut app).no_branch.as_deref(), Some("a session of yours is running in shop"));
    // A saved choice of "branch" that can't be offered any more falls back to a copy.
    Tickets::open(&w.config, &id)
        .set_terms(
            1,
            &Autonomy::Ask,
            &[crate::tickets::WorksOn { folder: shop.to_string_lossy().into_owned(), how: How::Branch, lands_on: "main".into() }],
        )
        .unwrap();
    app.reload_for_test();
    let l = line(&mut app);
    assert_eq!((l.how, l.no_branch.is_some()), (How::Worktree, true));
}

#[test]
fn the_tasks_pane_says_the_five_states_in_their_colors_wide_and_narrow() {
    let w = World::new("tasks-pane-words");
    w.transcript("aaa-1", "/tmp", "a conversation", 10);
    let mut app = w.app();
    let id = app.projects.create("Web shop").unwrap();
    app.projects.assign("aaa-1", Some(&id)).unwrap();
    let mut store = Tickets::open(&w.config, &id);
    let f = vec!["/tmp".to_string()];
    for (title, folders, blocked) in [
        ("Can start one", f.clone(), vec![]),
        ("Working one", f.clone(), vec![]),
        ("Needs you one", f.clone(), vec![]),
        ("Publish one", f.clone(), vec![]),
        ("Waits one", f.clone(), vec![1]),
        ("No folder one", vec![], vec![]),
        ("Done one", f.clone(), vec![]),
    ] {
        store.add(NewTicket { title: title.into(), folders, blocked_by: blocked, ..Default::default() }).unwrap();
    }
    let lead = |s: &str| crate::tickets::Leader {
        squad: "coding".into(),
        session: s.into(),
        started: 1,
        names: Default::default(),
        snapshot: None,
    };
    store.start_leader(2, &lead("l2"), &[]).unwrap();
    store.start_leader(3, &lead("l3"), &[]).unwrap();
    store.squad_says(3, &State::Waiting, "Asks: red or blue?", None).unwrap();
    store.start_leader(4, &lead("l4"), &[]).unwrap();
    store.squad_wants_to_publish(4, "Wants to publish: push task/4").unwrap();
    store.set_state(7, &State::Done).unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Char('3'));
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Tab);
    app.tk.unfolded = true;
    // (The picked row is drawn on the selection fill, with its colors held readable: the divider is picked instead.)
    app.tk.selected = Some(crate::app::TRow::Divider(1));
    let t = app.effective_theme().clone();
    // The board's rows: under each section's heading in its color, each row's word at its right in its color.
    let color_at = |term: &Terminal<TestBackend>, shown: &str, row_has: &str, word: &str| {
        let y = shown.lines().position(|l| l.contains(row_has)).unwrap_or_else(|| panic!("{row_has}: {shown}"));
        let row = shown.lines().nth(y).unwrap();
        let x = row[..row.rfind(word).unwrap_or_else(|| panic!("{word} on {row_has}: {shown}"))].chars().count() as u16;
        term.backend().buffer()[(x, y as u16)].fg
    };
    for (w, h) in [(140u16, 60u16), (60, 60)] {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let shown = draw(&mut term, &app);
        let f = flat(&shown);
        for (heading, color) in [("Needs you · 2", t.warn), ("Working · 1", t.busy), ("Can start · 2", t.ok), ("Waits for · 1", t.subtext)]
        {
            assert!(f.contains(heading), "{w}: {heading}: {shown}");
            assert_eq!(color_at(&term, &shown, heading, heading), color, "{w}: {heading}");
        }
        // (Narrow, the titles are cut with "…"; the words at the right never are.)
        for (row, word, color) in
            [("#3 ", "a question", t.warn), ("#4 ", "a go-ahead", t.warn), ("#6 ", "Needs a folder", t.subtext), ("#7 ", "Done", t.ok)]
        {
            assert_eq!(color_at(&term, &shown, row, word), color, "{w}: {row}");
        }
        if w >= 100 {
            // Their second lines: what each needs, in plain words.
            assert!(f.contains("#5 Waits one Waits for #1 Can start one"), "{w}: {shown}");
            assert!(f.contains("#1 Can start one New · S hands it to a squad"), "{w}: {shown}");
            assert!(f.contains("Coding squad wants to publish: push task/4"), "{w}: {shown}");
        }
        assert!(f.contains("▸ Done and cancelled · 1") || f.contains("▾ Done and cancelled · 1"), "{w}: {shown}");
        assert!(!f.contains("Ready") && !f.contains("Waiting on you"), "{f}");
    }
    // `y` offered on the one that wants to publish (the go-ahead), `S` on the one that can start.
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    app.tk.selected = Some(crate::app::TRow::Ticket(4));
    assert!(flat(&draw(&mut term, &app)).contains("y go ahead"));
    app.tk.selected = Some(crate::app::TRow::Ticket(1));
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("S hand to a squad") && !f.contains("go ahead"), "{f}");
    key(&mut app, KeyCode::Enter);
    let f = flat(&draw(&mut Terminal::new(TestBackend::new(120, 30)).unwrap(), &app));
    assert!(f.contains("State Can start") && f.contains("Works in /tmp") && !f.contains("On its own"), "nothing set yet: {f}");
}

#[test]
fn a_projects_old_task_files_are_moved_once_no_leader_of_it_runs_and_said_once() {
    let mut w = World::new("tasks-move");
    w.transcript("aaa-1", "/tmp", "a conversation", 10);
    let mut app = w.app();
    let id = app.projects.create("Web shop").unwrap();
    app.projects.assign("aaa-1", Some(&id)).unwrap();
    let dir = w.config.join("projects").join(&id);
    fs::create_dir_all(dir.join("tickets/01-work")).unwrap();
    fs::write(dir.join("tickets/01-basket.md"), "body").unwrap();
    fs::write(
        dir.join("tickets.json"),
        serde_json::json!({"next": 2, "tickets": [{"number": 1, "title": "Basket", "file": "01-basket.md", "state": "working", "folders": ["/tmp"], "blocked_by": [], "created": 1, "updated": 1,
            "leaders": [{"squad": "coding", "session": "lead-1", "started": 1}]}]})
        .to_string(),
    )
    .unwrap();
    // Its leader runs: nothing moves, and the old names are read.
    let pid = w.live("lead-1", "/tmp", 1, "idle");
    app.refresh_for_test();
    assert!(dir.join("tickets.json").exists() && !dir.join("tasks.json").exists(), "a running leader was given the old paths");
    key(&mut app, KeyCode::Char('3'));
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.ticket(1).map(|t| t.title.as_str()), Some("Basket"));
    assert!(app.active_notice().is_none_or(|n| !n.contains("Moved")), "{:?}", app.active_notice());
    // The leader ends: moved on the next refresh, said once, and the screen goes on from the new file.
    fs::remove_file(w.root.join("sessions").join(format!("{pid}.json"))).unwrap();
    app.refresh_for_test();
    assert!(!dir.join("tickets.json").exists() && dir.join("tickets.json.bak").exists() && dir.join("tasks.json").exists());
    assert!(dir.join("tasks/01-basket.md").exists() && dir.join("tasks/01-work").is_dir() && !dir.join("tickets").exists());
    assert_eq!(app.active_notice(), Some("Moved Web shop's tickets to tasks.json."));
    assert_eq!(app.ticket(1).map(|t| t.title.as_str()), Some("Basket"));
    assert_eq!(app.tickets_store().unwrap().path(), dir.join("tasks.json"));
    app.notice = None;
    app.refresh_for_test();
    assert_eq!(app.active_notice(), None, "said once");
    // A change through the screen lands in the new file.
    Tickets::open(&w.config, &id).rename(1, "Basket page").unwrap();
    app.refresh_for_test();
    assert_eq!(app.ticket(1).map(|t| t.title.as_str()), Some("Basket page"));
    assert!(fs::read_to_string(dir.join("tasks.json")).unwrap().contains("Basket page"));
    assert!(!fs::read_to_string(dir.join("tickets.json.bak")).unwrap().contains("Basket page"), "the backup is the file as it was");
}
