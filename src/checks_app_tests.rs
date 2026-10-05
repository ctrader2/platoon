//! The test check at the level of the app: the project's Tests line (with and without what its `CLAUDE.md` suggests)
//! and `T`'s box; the window that holds the squad line's lock running a due check off the UI thread, in the task's copy
//! or on its branch in the checkout, and what comes of it (typed into the squad's session twice, then Needs you; a
//! pass; nothing run where a session of the user's is open, or in a checkout that left the task's branch). Every
//! directory is a temp dir (`World`); the only commands run are stand-ins (`echo`, `exit 1`); no session is started;
//! the "squad's session" is a socket this test listens on.

use crate::app::{App, Mode, NamePurpose};
use crate::checks;
use crate::host::HostInfo;
use crate::testkit::World;
use crate::tickets::{State, Ticket, Tickets};
use crate::ui;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    style::Color,
};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn ctrl(app: &mut App, c: char) {
    app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
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

/// The cell where `needle` first starts on the line holding `row`.
fn fg_on(term: &Terminal<TestBackend>, row: &str, needle: &str) -> Color {
    let buf = term.backend().buffer();
    for y in 0..buf.area.height {
        let line: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
        if line.contains(row)
            && let Some(at) = line.find(needle)
        {
            return buf[(line[..at].chars().count() as u16, y)].fg;
        }
    }
    panic!("{needle:?} isn't on the line with {row:?}")
}

/// The line holding `text`, read flat.
fn line_with(shown: &str, text: &str) -> String {
    flat(shown.lines().find(|l| l.contains(text)).unwrap_or_else(|| panic!("no {text:?} in {shown}")))
}

/// "Web shop", whose one session works in `shop` (a folder in git, as far as a `.git` in it says); the project's
/// screen open.
fn shop_world(name: &str, claude_md: Option<&str>) -> (World, App, String, PathBuf) {
    let w = World::new(name);
    let shop = w.base.join("shop");
    fs::create_dir_all(shop.join(".git")).unwrap();
    if let Some(md) = claude_md {
        fs::write(shop.join("CLAUDE.md"), md).unwrap();
    }
    w.transcript("user-1", &shop.to_string_lossy(), "make a shop", 10);
    let mut app = w.app();
    let web = app.projects.create("Web shop").unwrap();
    app.projects.assign("user-1", Some(&web)).unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Char('3'));
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.open_project.as_deref(), Some(web.as_str()));
    app.notice = None;
    (w, app, web, shop)
}

const MD: &str = "# Shop\n\n## Build and test\n\n```\nexport PATH=\"$HOME/bin:$PATH\"   # each shell starts fresh\ncargo test                       # about a minute\n```\n";

#[test]
fn the_projects_tests_line_suggests_what_its_claude_md_says_and_t_sets_the_command() {
    let (w, mut app, web, _shop) = shop_world("ck-line", Some(MD));
    let t = app.effective_theme().clone();
    for (wide, high) in [(120, 40), (80, 24)] {
        let mut term = Terminal::new(TestBackend::new(wide, high)).unwrap();
        let shown = draw(&mut term, &app);
        assert_eq!(line_with(&shown, "Tests"), "Tests not set · T sets it · CLAUDE.md says: cargo test", "{wide}: {shown}");
        // Under the line saying how squads start, its label in the same quiet color, the key in the accent.
        let rows: Vec<&str> = shown.lines().collect();
        let at = rows.iter().position(|l| l.contains("Squads start")).unwrap();
        assert!(rows[at + 1].trim_start().starts_with("Tests"), "{wide}: {shown}");
        assert_eq!(rows[at + 1].find("not set"), rows[at].find("asks before"), "the values line up: {shown}");
        assert_eq!(fg_on(&term, "Tests", "Tests"), t.dim, "{wide}");
        assert_eq!(fg_on(&term, "Tests", "not set"), t.subtext, "{wide}");
        assert_eq!(fg_on(&term, "Tests", "T sets"), t.accent, "{wide}");
        assert_eq!(fg_on(&term, "Tests", "cargo test"), t.subtext, "{wide}");
    }
    // The help lists the key (the line itself says it, so the footer lets it go early).
    key(&mut app, KeyCode::Char('?'));
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    assert!(flat(&draw(&mut term, &app)).contains("T test command"));
    key(&mut app, KeyCode::Esc);
    // Narrow, the suggestion goes before the key does.
    let mut term = Terminal::new(TestBackend::new(60, 24)).unwrap();
    assert_eq!(line_with(&draw(&mut term, &app), "Tests"), "Tests not set · T sets it");
    // Nothing was run or written from the suggestion.
    let file: Value = serde_json::from_str(&fs::read_to_string(w.config.join("projects.json")).unwrap()).unwrap();
    assert!(file["projects"][&web].get("test_command").is_none(), "{file}");

    // T: the box, holding the suggestion for the user to check; enter saves it.
    key(&mut app, KeyCode::Char('T'));
    assert!(matches!(app.mode, Mode::ProjectName) && app.name_box.purpose == NamePurpose::TestCommand(web.clone()));
    assert_eq!(app.name_box.input, "cargo test");
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("Test command") && shown.contains("❯ cargo test"), "{shown}");
    assert!(shown.contains("Run with sh where the squad works, when it finishes."), "{shown}");
    typed(&mut app, " --quiet");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.active_notice(), Some("Tests for Web shop: platoon runs `cargo test --quiet` when a squad finishes."));
    let file: Value = serde_json::from_str(&fs::read_to_string(w.config.join("projects.json")).unwrap()).unwrap();
    assert_eq!(file["projects"][&web]["test_command"], "cargo test --quiet");
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let shown = draw(&mut term, &app);
    assert_eq!(line_with(&shown, "Tests"), "Tests cargo test --quiet T changes", "no suggestion once it is set: {shown}");
    assert_eq!(fg_on(&term, "Tests", "cargo test --quiet"), t.text);
    assert_eq!(fg_on(&term, "Tests", "T changes"), t.accent);

    // From the notes pane too; an empty box takes the command away.
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Char('T'));
    assert_eq!(app.name_box.input, "cargo test --quiet", "it holds the command it has");
    ctrl(&mut app, 'u');
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.active_notice(), Some("Web shop has no test command now: platoon runs no tests for its tasks."));
    let file: Value = serde_json::from_str(&fs::read_to_string(w.config.join("projects.json")).unwrap()).unwrap();
    assert!(file["projects"][&web].get("test_command").is_none(), "{file}");
    // Esc in the box changes nothing.
    key(&mut app, KeyCode::Char('T'));
    typed(&mut app, "x");
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List));
    assert!(app.projects.get(&web).unwrap().test_command.is_empty());
}

#[test]
fn without_a_claude_md_block_the_tests_line_suggests_nothing_and_the_box_starts_empty() {
    let (_w, mut app, _web, shop) = shop_world("ck-nomd", Some("# Shop\n\nRun the tests with cargo.\n"));
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let shown = draw(&mut term, &app);
    assert_eq!(line_with(&shown, "Tests"), "Tests not set · T sets it", "{shown}");
    assert!(!shown.contains("CLAUDE.md"), "{shown}");
    // A CLAUDE.md written now is read at the next refresh.
    fs::write(shop.join("CLAUDE.md"), MD).unwrap();
    app.reload_for_test();
    assert!(flat(&draw(&mut term, &app)).contains("CLAUDE.md says: cargo test"));
    fs::remove_file(shop.join("CLAUDE.md")).unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Char('T'));
    assert_eq!(app.name_box.input, "", "nothing to suggest");
}

// ---- running the check ----------------------------------------------------------------------------------------------

/// "Web shop" with `command` as its test command and task #1, whose Coding squad (leader `L1`) works in its copy of
/// `shop` (`<base>/shop-worktrees/task-1`, a plain folder: no git is asked about a copy) and has just asked to close it;
/// its check is due. The app has read it all, and has run nothing yet.
fn due_world(name: &str, command: &str) -> (World, App, String, PathBuf) {
    let w = World::new(name);
    w.transcript("user-1", "/tmp", "make a shop", 10);
    let copy = w.base.join("shop-worktrees/task-1");
    fs::create_dir_all(&copy).unwrap();
    let mut app = w.app();
    let web = app.projects.create("Web shop").unwrap();
    app.projects.set_test_command(&web, command).unwrap();
    let shop = w.base.join("shop").to_string_lossy().into_owned();
    let t = json!({"number": 1, "title": "Basket", "file": "01-basket.md", "state": "waiting", "folders": [shop],
        "blocked_by": [], "created": 1, "updated": 1, "reason": "Asks: Shall I close this task?",
        "leaders": [{"squad": "coding", "session": "L1", "started": 1, "names": "squad-role"}],
        "worktrees": [{"folder": shop, "path": copy.to_string_lossy(), "branch": "task/1"}],
        "check_due": {"stop": "close", "at": 1_000}});
    let path = crate::tickets::path_in(&w.config, &web);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, json!({"next": 2, "tickets": [t]}).to_string()).unwrap();
    (w, app, web, copy)
}

fn task(w: &World, project: &str) -> Ticket {
    Tickets::open(&w.config, project).get(1).cloned().unwrap()
}

/// Refresh with these hosts until the check that is due has run and been written down.
fn settle(app: &mut App, w: &World, project: &str, hosts: &[HostInfo]) {
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        app.refresh_with(hosts.to_vec(), false);
        if task(w, project).check_due.is_none() && !app.check_running(project, 1) {
            return;
        }
        assert!(Instant::now() < until, "the check never finished: {:?}", task(w, project));
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The squad's session: a host whose socket this test listens on. What one window typed into it comes back once that
/// window is done with it.
struct Squad {
    host: HostInfo,
    listener: UnixListener,
}

impl Squad {
    fn new(w: &World, k: usize) -> Squad {
        let sock = w.base.join(format!("lead-{k}.sock"));
        let listener = UnixListener::bind(&sock).unwrap();
        listener.set_nonblocking(true).unwrap();
        Squad {
            host: HostInfo { claude_pid: 1, host_pid: 1, dir: "/tmp".into(), args: vec!["--resume".into(), "L1".into()], sock },
            listener,
        }
    }

    /// What was typed, waiting up to a few seconds for it.
    fn typed(&self) -> String {
        let until = Instant::now() + Duration::from_secs(10);
        let mut s = loop {
            match self.listener.accept() {
                Ok((s, _)) => break s,
                Err(_) if Instant::now() < until => std::thread::sleep(Duration::from_millis(20)),
                Err(e) => panic!("nothing was typed: {e}"),
            }
        };
        s.set_nonblocking(false).unwrap();
        let mut typed = Vec::new();
        while let Ok((kind, payload)) = crate::host::read_frame(&mut s) {
            if kind == crate::host::F_INPUT {
                typed.extend(payload);
            }
        }
        String::from_utf8_lossy(&typed).into_owned()
    }

    /// Nothing was typed (within a moment).
    fn nothing_typed(&self) -> bool {
        std::thread::sleep(Duration::from_millis(300));
        self.listener.accept().is_err()
    }
}

/// The leader stops again asking to close the task, and the hook marks a check due.
fn stops_again(w: &World, project: &str, at: u64) {
    let mut store = Tickets::open(&w.config, project);
    store.user_typed(1).unwrap();
    store.squad_says(1, &State::Waiting, "Asks: Shall I close this task?", None).unwrap();
    store.mark_check_due(1, "close", at).unwrap();
}

#[test]
fn a_failure_is_typed_into_the_squads_session_twice_then_the_task_needs_you_and_a_pass_starts_again() {
    let (w, mut app, web, copy) = due_world("ck-fail", "echo running; echo \"test result: FAILED. 2 passed; 1 failed\"; exit 1");
    let squad = Squad::new(&w, 1);
    settle(&mut app, &w, &web, std::slice::from_ref(&squad.host));
    // 1. Working again, the failure typed into the squad's session as one pasted message, then Enter.
    let typed = squad.typed();
    let folder = copy.to_string_lossy();
    assert_eq!(
        typed,
        format!(
            "\x1b[200~The tests failed when platoon ran `echo running; echo \"test result: FAILED. 2 passed; 1 failed\"; exit 1` in {folder}:\n\n```\nrunning\ntest result: FAILED. 2 passed; 1 failed\n```\n\nFix what makes them fail, then end your turn as before: platoon runs them again when you stop.\x1b[201~\r"
        )
    );
    let t = task(&w, &web);
    assert_eq!((&t.state, t.reason.as_str(), t.check_due.as_ref()), (&State::Working, "", None), "{t:?}");
    let c = t.checks.last().unwrap();
    assert_eq!(
        (c.kind.as_str(), c.ok, c.summary.as_str(), c.attempt, c.folder.as_str()),
        ("tests", false, "2 passed, 1 failed", 1, &*folder)
    );
    let log = crate::tickets::work_dir(&w.config, &web, 1).join("tests-1.log");
    assert_eq!(fs::read_to_string(&log).unwrap(), "running\ntest result: FAILED. 2 passed; 1 failed\n", "the output, kept for the user");
    assert_eq!(
        app.active_notice(),
        Some("#1's tests failed (2 passed, 1 failed): platoon told its squad, which is fixing them (try 1 of 2).")
    );
    // 2. It stops again, and they fail again: told once more.
    stops_again(&w, &web, 2_000);
    let squad = Squad::new(&w, 2);
    settle(&mut app, &w, &web, std::slice::from_ref(&squad.host));
    assert!(squad.typed().contains("The tests failed when platoon ran"));
    let t = task(&w, &web);
    assert_eq!((&t.state, t.checks.last().unwrap().attempt), (&State::Working, 2));
    assert!(crate::tickets::work_dir(&w.config, &web, 1).join("tests-2.log").exists());
    // 3. The third failure: the user, and nothing typed.
    stops_again(&w, &web, 3_000);
    let squad = Squad::new(&w, 3);
    settle(&mut app, &w, &web, std::slice::from_ref(&squad.host));
    assert!(squad.nothing_typed());
    let t = task(&w, &web);
    assert_eq!((&t.state, t.reason.as_str(), t.tests_failing), (&State::Waiting, checks::STILL_FAIL, true), "{t:?}");
    assert_eq!(app.active_notice(), Some("#1 needs you: The tests still fail after the squad tried twice (2 passed, 1 failed)."));
    // A pass leaves the task as the leader left it, and the next failure is a first try again.
    app.projects.set_test_command(&web, "echo \"test result: ok. 3 passed; 0 failed\"").unwrap();
    stops_again(&w, &web, 4_000);
    settle(&mut app, &w, &web, std::slice::from_ref(&squad.host));
    let t = task(&w, &web);
    assert_eq!((&t.state, t.reason.as_str(), t.tests_failing), (&State::Waiting, "Asks: Shall I close this task?", false));
    assert_eq!((t.checks.last().unwrap().ok, t.checks.last().unwrap().summary.as_str()), (true, "3 passed"));
    assert_eq!(app.active_notice(), Some("#1's tests passed: 3 passed."));
    assert_eq!(checks::next_try(&t.checks), 1);
}

#[test]
fn with_its_session_gone_a_failure_needs_you_and_a_session_of_yours_in_the_copy_keeps_the_check_out() {
    let (w, mut app, web, copy) = due_world("ck-alone", "exit 1");
    settle(&mut app, &w, &web, &[]);
    let t = task(&w, &web);
    assert_eq!((&t.state, t.reason.as_str(), t.tests_failing), (&State::Waiting, checks::FAILED_NOT_RUNNING, true), "{t:?}");
    assert_eq!(t.checks.last().unwrap().summary, "failed with exit code 1, with nothing to say");
    // A session of the user's open in the copy: not run, said why, and not one of the squad's tries.
    stops_again(&w, &web, 2_000);
    let yours = HostInfo {
        claude_pid: 2,
        host_pid: 2,
        dir: copy.join("src").to_string_lossy().into(),
        args: vec!["--resume".into(), "user-1".into()],
        sock: PathBuf::new(),
    };
    settle(&mut app, &w, &web, &[yours]);
    let t = task(&w, &web);
    let c = t.checks.last().unwrap();
    assert_eq!((c.ok, c.skipped, c.summary.as_str()), (false, true, checks::SESSION_OPEN));
    assert_eq!((&t.state, t.reason.as_str()), (&State::Waiting, "Asks: Shall I close this task?"), "left as the leader left it");
    assert_eq!(checks::next_try(&t.checks), 2, "the skip counts neither way");
    assert!(!crate::tickets::work_dir(&w.config, &web, 1).join("tests-2.log").exists(), "nothing ran");
    assert_eq!(app.active_notice(), Some("#1's tests weren't run: a session of yours is open there."));
    // The squad's own session there is no reason not to.
    stops_again(&w, &web, 3_000);
    let own = HostInfo {
        claude_pid: 3,
        host_pid: 3,
        dir: copy.to_string_lossy().into(),
        args: vec!["--resume".into(), "L1".into()],
        sock: w.base.join("none.sock"),
    };
    settle(&mut app, &w, &web, &[own]);
    assert!(!task(&w, &web).checks.last().unwrap().skipped);
}

#[test]
fn a_task_that_moved_on_before_its_check_ran_has_its_mark_taken_away_and_nothing_run() {
    let (w, mut app, web, _copy) = due_world("ck-moved", "exit 1");
    Tickets::open(&w.config, &web).set_state(1, &State::Cancelled).unwrap();
    let mut store = Tickets::open(&w.config, &web);
    store.mark_check_due(1, "close", 5_000).unwrap();
    settle(&mut app, &w, &web, &[]);
    let t = task(&w, &web);
    assert!(t.checks.is_empty() && t.check_due.is_none(), "{t:?}");
    // A project whose command was taken away meanwhile: the same.
    store.set_state(1, &State::Done).unwrap();
    store.mark_check_due(1, "done", 6_000).unwrap();
    app.projects.set_test_command(&web, "").unwrap();
    settle(&mut app, &w, &web, &[]);
    assert!(task(&w, &web).checks.is_empty());
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn on_a_branch_in_the_checkout_the_check_runs_only_while_the_checkout_is_on_the_tasks_branch() {
    let (w, mut app, web, _copy) = due_world("ck-branch", "echo \"test result: ok. 1 passed; 0 failed\"");
    let shop = w.base.join("shop");
    fs::create_dir_all(&shop).unwrap();
    fs::write(shop.join("README.md"), "# Shop\n").unwrap();
    git(&shop, &["init", "-q"]);
    git(&shop, &["add", "."]);
    git(&shop, &["commit", "-q", "-m", "first"]);
    // The task works on task/1 in the user's own checkout (no copy); the checkout is on main just now.
    let path = crate::tickets::path_in(&w.config, &web);
    let mut v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let t = v["tickets"][0].as_object_mut().unwrap();
    t.remove("worktrees");
    t.insert("works_on".into(), json!([{"folder": shop.to_string_lossy(), "how": "branch", "lands_on": "main"}]));
    fs::write(&path, v.to_string()).unwrap();
    settle(&mut app, &w, &web, &[]);
    let c = task(&w, &web).checks.last().cloned().unwrap();
    assert_eq!((c.skipped, c.summary.as_str()), (true, "your checkout of shop isn't on task/1 any more"));
    // On the task's branch: run there.
    git(&shop, &["switch", "-q", "-c", "task/1"]);
    stops_again(&w, &web, 2_000);
    settle(&mut app, &w, &web, &[]);
    let c = task(&w, &web).checks.last().cloned().unwrap();
    assert_eq!((c.ok, c.skipped, c.summary.as_str(), c.folder.as_str()), (true, false, "1 passed", &*shop.to_string_lossy()));
}

#[test]
fn only_the_window_that_holds_the_squad_lines_lock_runs_checks() {
    let (w, mut app, web, _copy) = due_world("ck-lock", "true");
    settle(&mut app, &w, &web, &[]);
    assert_eq!(task(&w, &web).checks.len(), 1, "the first window took the lock and ran it");
    // A second window on the same folders sees the next due check and leaves it alone.
    let mut other = w.app();
    stops_again(&w, &web, 2_000);
    for _ in 0..5 {
        other.refresh_with(Vec::new(), false);
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!other.check_running(&web, 1));
    assert_eq!(task(&w, &web).checks.len(), 1, "{:?}", task(&w, &web));
    settle(&mut app, &w, &web, &[]);
    assert_eq!(task(&w, &web).checks.len(), 2);
}
