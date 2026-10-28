//! The code editor (`v`) at the level of the app: what it opens from each place, the question before it opens (the
//! task's session is working), a removed worktree brought back first, the worktree clean-up never taking away a folder an editor
//! is about to open or has open, and what it says when this computer has no editor. Real git in temp repositories;
//! nothing is run (the request the event loop would take is looked at instead, or a stand-in records its start).

use crate::app::{App, Confirm, EdTarget, Mode};
use crate::host::HostInfo;
use crate::testkit::World;
use crate::tickets::{Need, NewTicket, State, Tickets, Worktree};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn v(app: &mut App) {
    key(app, KeyCode::Char('v'));
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

/// The folders the request the event loop would take opens (a terminal editor: the event loop hands it the first).
fn opened(app: &mut App) -> Vec<String> {
    app.take_own_editor_request()
        .map(|(_, dirs)| dirs)
        .unwrap_or_else(|| panic!("nothing to open; mode {:?}, notice {:?}", matches!(app.mode, Mode::List), app.notice))
}

/// The app's editors: Helix alone (a terminal editor, so the request is there for the event loop to take).
fn with_helix(app: &mut App, w: &World) {
    app.set_file_editors_for_test(vec![helix(&w.base.join("bin"))]);
}

/// A conversation in `cwd` with `added` folders beside it (Claude Code's environment snapshot).
fn with_added(w: &World, id: &str, cwd: &str, added: &[String]) {
    let at = crate::testkit::now_ms() - 60_000;
    let (t0, t1) = (crate::testkit::iso(at - 5000), crate::testkit::iso(at));
    let lines = [
        serde_json::json!({"type":"user","cwd":cwd,"gitBranch":"main","timestamp":t0,"message":{"role":"user","content":"fix it"}}),
        serde_json::json!({"type":"attachment","isSidechain":false,"cwd":cwd,"timestamp":t0,
            "attachment":{"type":"environment","snapshot":{"workingDirectory":cwd,"additionalWorkingDirectories":added},"changes":null}}),
        serde_json::json!({"type":"assistant","cwd":cwd,"gitBranch":"main","timestamp":t1,"message":{"model":"claude-sonnet-5","usage":{"input_tokens":50}}}),
    ];
    w.transcript_lines(id, &lines.iter().map(|v| v.to_string()).collect::<Vec<_>>());
}

#[test]
fn v_on_a_session_opens_its_folder_and_its_added_folders() {
    let w = World::new("ed-session");
    let main = w.base.join("repos/app");
    repo(&main);
    let docs = w.base.join("docs");
    fs::create_dir_all(&docs).unwrap();
    with_added(&w, "s-1", &s(&main), &[s(&docs)]);
    let mut app = w.app();
    with_helix(&mut app, &w);
    assert_eq!(app.editor_target(), Some(EdTarget::Session("s-1".into())));
    v(&mut app);
    assert_eq!(opened(&mut app), vec![s(&main), s(&docs)], "opened at once: nothing to ask");
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn v_with_nothing_selected_asks_for_a_folder_and_typing_v_there_is_a_letter() {
    let w = World::new("ed-picker");
    let mut app = w.app();
    with_helix(&mut app, &w);
    assert_eq!(app.editor_target(), None, "an empty list");
    v(&mut app);
    assert!(matches!(app.mode, Mode::Picker) && app.picker.editor);
    assert!(app.take_own_editor_request().is_none());
    // In the picker's box, v is typed.
    v(&mut app);
    assert_eq!(app.picker.input, "v");
    key(&mut app, KeyCode::Backspace);
    for c in s(&w.base).chars() {
        key(&mut app, KeyCode::Char(c));
    }
    key(&mut app, KeyCode::Enter);
    assert_eq!(opened(&mut app), vec![s(&w.base)]);

    // The Projects list asks too.
    key(&mut app, KeyCode::Char('2'));
    v(&mut app);
    assert!(matches!(app.mode, Mode::Picker) && app.picker.editor);
    key(&mut app, KeyCode::Esc);

    // The search box and the rename box take v as a letter.
    key(&mut app, KeyCode::Char('1'));
    key(&mut app, KeyCode::Char('/'));
    v(&mut app);
    assert!(matches!(app.mode, Mode::Search));
    assert_eq!(app.search.text, "v");
    assert!(app.take_own_editor_request().is_none());
}

/// A project "Web shop" with task #1 in the temp repository `shop`, started as session `sess-1` whose worktree has one
/// commit; the task is in `state` and picked on the Tasks tab.
struct Lab {
    w: World,
    app: App,
    project: String,
    shop: PathBuf,
    made: Worktree,
    wt: PathBuf,
}

fn lab(name: &str, state: State) -> Lab {
    let w = World::new(name);
    w.transcript("user-1", "/tmp", "make a basket page", 10);
    let shop = w.base.join("repos/shop");
    repo(&shop);
    let mut app = w.app();
    with_helix(&mut app, &w);
    let project = app.projects.create("Web shop").unwrap();
    app.projects.assign("user-1", Some(&project)).unwrap();
    let mut t = Tickets::open(&w.config, &project);
    let n = t.add(NewTicket { title: "Basket".into(), folder: Some(s(&shop)), ..Default::default() }).unwrap();
    let made = crate::worktrees::prepare("git", &s(&shop), n, None).unwrap().unwrap();
    t.start(n, "feature", "sess-1", Some(made.clone()), &[]).unwrap();
    let wt = PathBuf::from(&made.path);
    fs::write(wt.join("basket.txt"), "basket\n").unwrap();
    git(&wt, &["add", "."]);
    git(&wt, &["commit", "-q", "-m", "basket"]);
    let mut l = Lab { w, app, project, shop, made, wt };
    l.set(state);
    key(&mut l.app, KeyCode::Char('3'));
    assert!(l.app.board_showing() && l.app.ticket(1).is_some());
    l
}

impl Lab {
    fn ticket(&self) -> crate::tickets::Ticket {
        Tickets::open(&self.w.config, &self.project).get(1).cloned().unwrap()
    }

    /// The task's state changes (as its session's hooks change it).
    fn set(&mut self, state: State) {
        Tickets::open(&self.w.config, &self.project)
            .update(1, |t| {
                t.state = state;
                true
            })
            .unwrap();
        self.app.reload_for_test();
    }

    /// The task's session runs in a platoon host.
    fn session_hosted(&mut self) {
        let host = HostInfo {
            agent: crate::agents::Agent::Claude,
            claude_pid: 4242,
            host_pid: 0,
            dir: s(&self.wt),
            args: vec!["--resume".into(), "sess-1".into()],
            sock: PathBuf::new(),
            task: None,
            version: None,
            kept: None,
        };
        self.app.refresh_with(vec![host], false);
    }

    /// Refresh for a while (longer than the clean-up's interval), then wait for anything it started to finish.
    fn refresh_for(&mut self, secs: f32) {
        let until = Instant::now() + Duration::from_secs_f32(secs);
        while Instant::now() < until {
            self.app.refresh_for_test();
            std::thread::sleep(Duration::from_millis(50));
        }
        self.app.refresh_until_for_test(10, "the removals to finish", |a| a.worktrees_idle());
    }

    fn removed(&self) -> Option<u64> {
        self.ticket().worktree.and_then(|w| w.removed)
    }
}

#[test]
fn v_on_a_task_opens_its_worktree_and_on_one_not_started_its_folder() {
    let mut l = lab("ed-ticket", State::NeedsYou(Need::Question("which page?".into())));
    assert_eq!(l.app.editor_target(), Some(EdTarget::Ticket { project: l.project.clone(), number: 1 }));
    v(&mut l.app);
    assert_eq!(opened(&mut l.app), vec![s(&l.wt)]);
    // A task that never started: its folder, from its own screen too.
    Tickets::open(&l.w.config, &l.project)
        .add(NewTicket { title: "Later".into(), folder: Some(s(&l.shop)), ..Default::default() })
        .unwrap();
    l.app.reload_for_test();
    key(&mut l.app, KeyCode::Char('G'));
    key(&mut l.app, KeyCode::Enter);
    assert_eq!(l.app.tk.open, Some(2));
    v(&mut l.app);
    assert_eq!(opened(&mut l.app), vec![s(&l.shop)]);
    let _ = &l.made;
}

#[test]
fn the_session_still_working_is_said_before_opening() {
    let mut l = lab("ed-working", State::Working);
    l.session_hosted();
    let t = l.app.every_ticket_for_test().into_iter().find(|(_, t)| t.number == 1).unwrap();
    assert!(l.app.task_running(&t.0, &t.1));
    v(&mut l.app);
    assert!(matches!(&l.app.mode, Mode::Confirm(Confirm::EditorWhileWorking { number: 1, .. })));
    assert!(l.app.take_own_editor_request().is_none());
    key(&mut l.app, KeyCode::Char('n'));
    assert!(matches!(l.app.mode, Mode::List));
    assert!(l.app.take_own_editor_request().is_none(), "no: nothing opens");
    assert_eq!(l.app.editor_flow_for_test(), None);
    v(&mut l.app);
    key(&mut l.app, KeyCode::Char('y'));
    assert_eq!(opened(&mut l.app), vec![s(&l.wt)], "yes: it opens");

    // Waiting on the user (not working): no question.
    l.set(State::NeedsYou(Need::Plan));
    v(&mut l.app);
    assert_eq!(opened(&mut l.app), vec![s(&l.wt)]);
}

#[test]
fn a_removed_worktree_is_brought_back_from_its_branch_before_the_editor_opens() {
    let mut l = lab("ed-back", State::Done);
    l.refresh_for(2.5);
    assert!(!l.wt.exists(), "removed after the work (the session isn't running)");
    assert!(l.removed().is_some());
    v(&mut l.app);
    assert!(l.app.take_own_editor_request().is_none(), "not yet: it is being made again");
    assert!(l.app.notice.as_ref().is_some_and(|(n, ..)| n.contains("back")), "{:?}", l.app.notice);
    l.app.refresh_until_for_test(10, "the worktree back and the editor asked for", |a| a.worktrees_idle());
    assert!(l.wt.is_dir());
    assert!(l.wt.join("basket.txt").is_file(), "from its branch: the session's commit is there");
    assert_eq!(l.removed(), None);
    assert_eq!(opened(&mut l.app), vec![s(&l.wt)]);
}

/// platoon starts removing a Done task's worktree (its session stopped) just before `v`: the editor must still open on
/// a worktree that is there (made again), never on one being taken away.
#[test]
fn v_just_after_the_clean_up_started_opens_on_a_worktree_that_stays() {
    let mut l = lab("ed-race", State::Working);
    // Done; past the clean-up's interval, one refresh starts the removal off the UI thread, then `v` at once.
    std::thread::sleep(Duration::from_millis(2100));
    l.set(State::Done);
    assert!(!l.app.worktrees_idle(), "the removal is under way");
    v(&mut l.app);
    l.app.refresh_until_for_test(10, "the worktree back", |a| a.worktrees_idle());
    assert!(l.wt.is_dir(), "the editor's folder is there");
    assert_eq!(opened(&mut l.app), vec![s(&l.wt)]);
    assert_eq!(l.removed(), None);
}

/// While a `v` waits on a question (here: which editor?), or while an editor (this window's or another's) has the
/// worktree open, the clean-up leaves it alone; once the editor has closed, it goes as before.
#[test]
fn the_clean_up_leaves_a_worktree_the_editor_is_opening_or_has_open() {
    let mut l = lab("ed-hold", State::Working);
    // The list of editors is open while the task turns Done and its session stops.
    with_settings(&l.w, |s| s.files_editor_ask = true);
    l.app.reload_for_test();
    v(&mut l.app);
    assert!(matches!(l.app.mode, Mode::PickFileEditor));
    l.set(State::Done);
    l.refresh_for(2.5);
    assert!(l.wt.is_dir(), "kept while v is on its way");
    key(&mut l.app, KeyCode::Esc);

    // An editor has it open (a mark, as main.rs writes while the editor runs; here another window's, this process).
    let mark = crate::editor::mark_open(&l.w.cache, std::process::id(), &[s(&l.wt)]).unwrap();
    let mut t = Tickets::open(&l.w.config, &l.project);
    t.touch(1).unwrap(); // a new version of the task: tried again
    l.app.reload_for_test();
    l.refresh_for(2.5);
    assert!(l.wt.is_dir(), "kept while the editor has it open");
    crate::editor::unmark(&mark);
    t.touch(1).unwrap();
    l.app.reload_for_test();
    l.refresh_for(2.5);
    assert!(!l.wt.exists(), "removed once the editor has closed");
}

// ---- the machine's own editors (`fileeditors.rs`) -------------------------------------------------------------------

use crate::fileeditors::{FileEditor, Runs, Several};
use crate::settings::{self, Settings};

/// A stand-in for an editor with a window of its own: every start appends its arguments, one line per start, to
/// `<dir>/<id>.calls`. Never a real editor.
fn window_stand_in(dir: &Path, id: &str, name: &str, args: &[&str], several: Several) -> FileEditor {
    fs::create_dir_all(dir).unwrap();
    let script = dir.join(id);
    let calls = dir.join(format!("{id}.calls"));
    fs::write(&script, format!("#!/bin/sh\necho \"$*\" >> '{}'\n", s(&calls))).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    FileEditor {
        id: id.into(),
        name: name.into(),
        program: s(&script),
        args: args.iter().map(|a| a.to_string()).collect(),
        runs: Runs::Window,
        several,
    }
}

fn helix(dir: &Path) -> FileEditor {
    FileEditor {
        id: "helix".into(),
        name: "Helix".into(),
        program: s(&dir.join("hx")),
        args: vec![],
        runs: Runs::Terminal,
        several: Several::First,
    }
}

/// What the stand-in `id` was started with, once it has been (it runs beside the app).
fn calls(dir: &Path, id: &str, n: usize) -> Vec<String> {
    let f = dir.join(format!("{id}.calls"));
    let start = Instant::now();
    loop {
        let got: Vec<String> = fs::read_to_string(&f).unwrap_or_default().lines().map(String::from).collect();
        if got.len() >= n {
            return got;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "{id} was started {} times, not {n}", got.len());
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn with_settings(w: &World, f: impl FnOnce(&mut Settings)) {
    let mut s = settings::load(&w.config).0;
    f(&mut s);
    settings::save(&w.config, &s).unwrap();
}

fn notice(app: &App) -> String {
    app.notice.as_ref().map(|(n, ..)| n.clone()).unwrap_or_default()
}

#[test]
fn v_goes_straight_to_the_favourite_when_always_ask_is_off() {
    let w = World::new("fe-straight");
    let main = w.base.join("repos/app");
    repo(&main);
    let docs = w.base.join("docs");
    fs::create_dir_all(&docs).unwrap();
    with_added(&w, "s-1", &s(&main), &[s(&docs)]);
    let bin = w.base.join("bin");
    let code = window_stand_in(&bin, "code", "VS Code", &["--new-window"], Several::OneWindow);

    // None chosen yet: the first one found (Helix here), no list.
    let mut app = w.app();
    app.set_file_editors_for_test(vec![helix(&bin), code.clone()]);
    v(&mut app);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(opened(&mut app), vec![s(&main), s(&docs)]);
    app.editor_closed(Ok(()));

    // VS Code the favourite: started beside platoon with both folders in one window, and platoon stays as it was.
    with_settings(&w, |s| s.files_editor = "code".into());
    app.reload_for_test();
    v(&mut app);
    assert!(matches!(app.mode, Mode::List), "no list, no question");
    assert!(app.take_own_editor_request().is_none(), "nothing takes the terminal");
    assert_eq!(calls(&bin, "code", 1), vec![format!("--new-window {} {}", s(&main), s(&docs))]);
    assert_eq!(notice(&app), "Opened in VS Code, in a window of its own.");

    // Helix the favourite: handed the terminal (by the event loop) on the first folder; it says what it left out
    // once it has closed.
    with_settings(&w, |s| s.files_editor = "helix".into());
    app.reload_for_test();
    v(&mut app);
    let (e, dirs) = app.take_own_editor_request().expect("a terminal editor of the user's");
    assert_eq!((e.id.as_str(), dirs), ("helix", vec![s(&main), s(&docs)]));
    app.editor_closed(Ok(()));
    assert!(
        notice(&app).starts_with("Helix opened ") && notice(&app).contains("only: it opens one folder at a time. Not opened:"),
        "{}",
        notice(&app)
    );
}

#[test]
fn always_ask_shows_the_list_with_the_favourite_starred_and_picked_and_another_choice_becomes_the_favourite() {
    let w = World::new("fe-ask");
    let main = w.base.join("repos/app");
    repo(&main);
    w.transcript("s-1", &s(&main), "hello", 5);
    let bin = w.base.join("bin");
    let code = window_stand_in(&bin, "code", "VS Code", &["--new-window"], Several::OneWindow);
    with_settings(&w, |s| s.files_editor_ask = true);
    let mut app = w.app();
    app.set_file_editors_for_test(vec![helix(&bin), code]);
    v(&mut app);
    assert!(matches!(app.mode, Mode::PickFileEditor));
    let ids: Vec<&str> = app.file_pick().options.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["helix", "code"], "the computer's, as found");
    assert_eq!((app.file_pick().starred.as_str(), app.file_pick().selected), ("helix", 0), "none chosen yet: the first");
    // Esc: nothing opens.
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List) && app.editor_flow_for_test().is_none());
    assert!(app.take_own_editor_request().is_none());

    // Down to VS Code, enter: it opens, and is the favourite from now on.
    v(&mut app);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(calls(&bin, "code", 1), vec![format!("--new-window {}", s(&main))]);
    assert_eq!(settings::load(&w.config).0.files_editor, "code");
    v(&mut app);
    assert_eq!((app.file_pick().starred.as_str(), app.file_pick().selected), ("code", 1), "starred and picked: enter opens it again");
    key(&mut app, KeyCode::Enter);
    assert_eq!(calls(&bin, "code", 2).len(), 2);

    // Back up to Helix: it opens in the terminal, and is the favourite now.
    v(&mut app);
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Enter);
    assert_eq!(opened(&mut app), vec![s(&main)]);
    assert_eq!(settings::load(&w.config).0.files_editor, "helix");
}

#[test]
fn a_favourite_no_longer_installed_opens_the_first_one_found_and_says_so_once() {
    let w = World::new("fe-gone");
    let main = w.base.join("repos/app");
    repo(&main);
    w.transcript("s-1", &s(&main), "hello", 5);
    with_settings(&w, |s| s.files_editor = "rustrover".into());
    let mut app = w.app();
    with_helix(&mut app, &w);
    v(&mut app);
    assert_eq!(opened(&mut app), vec![s(&main)]);
    assert_eq!(notice(&app), "RustRover isn't on this computer any more, so Helix opens instead (Settings, Editors).");
    app.editor_closed(Ok(()));
    assert!(notice(&app).starts_with("RustRover isn't"), "said again once the editor has closed (it had the screen)");
    app.notice = None;
    v(&mut app);
    assert_eq!(opened(&mut app), vec![s(&main)]);
    app.editor_closed(Ok(()));
    assert_eq!(notice(&app), "", "once is enough");
    assert_eq!(settings::load(&w.config).0.files_editor, "rustrover", "the setting stays, in case it comes back");
}

#[test]
fn jetbrains_opens_each_folder_in_a_window_of_its_own_at_most_three() {
    let w = World::new("fe-jb");
    let main = w.base.join("repos/app");
    repo(&main);
    let added: Vec<String> =
        (1..=4).map(|i| w.base.join(format!("more-{i}"))).inspect(|d| fs::create_dir_all(d).unwrap()).map(|d| s(&d)).collect();
    with_added(&w, "s-1", &s(&main), &added);
    let bin = w.base.join("bin");
    let idea = window_stand_in(&bin, "idea", "IntelliJ IDEA", &[], Several::EachWindow);
    with_settings(&w, |s| s.files_editor = "idea".into());
    let mut app = w.app();
    app.set_file_editors_for_test(vec![idea]);
    v(&mut app);
    let mut got = calls(&bin, "idea", 3);
    got.sort();
    let mut want = vec![s(&main), added[0].clone(), added[1].clone()];
    want.sort();
    assert_eq!(got, want, "one start per folder, the first three");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(calls(&bin, "idea", 3).len(), 3, "no fourth window");
    assert_eq!(notice(&app), "Opened 3 of the 5 folders in IntelliJ IDEA, a window each (it opens at most 3).");
}

#[test]
fn a_worktree_opened_in_a_window_editor_is_kept_after_the_work() {
    let mut l = lab("fe-kept", State::NeedsYou(Need::Question("which page?".into())));
    let bin = l.w.base.join("bin");
    let code = window_stand_in(&bin, "code", "VS Code", &["--new-window"], Several::OneWindow);
    with_settings(&l.w, |s| s.files_editor = "code".into());
    l.app.reload_for_test();
    l.app.set_file_editors_for_test(vec![helix(&bin), code]);
    v(&mut l.app);
    assert_eq!(calls(&bin, "code", 1), vec![format!("--new-window {}", s(&l.wt))]);
    assert_eq!(l.ticket().worktree.and_then(|w| w.opened_in).as_deref(), Some("VS Code"), "written on the task: it survives a restart");
    assert!(notice(&l.app).contains("#1's worktree stays after the work"), "{}", notice(&l.app));
    // Done, its session not running: the clean-up leaves it (platoon can't tell when VS Code closes).
    l.set(State::Done);
    l.refresh_for(2.5);
    assert!(l.wt.is_dir(), "kept");
    assert!(l.removed().is_none());
}

#[test]
fn a_terminal_editor_keeps_nothing_after_the_work() {
    let mut l = lab("fe-term", State::NeedsYou(Need::Question("which page?".into())));
    let bin = l.w.base.join("bin");
    with_settings(&l.w, |s| s.files_editor = "helix".into());
    l.app.reload_for_test();
    l.app.set_file_editors_for_test(vec![helix(&bin)]);
    v(&mut l.app);
    let (_, dirs) = l.app.take_own_editor_request().expect("handed the terminal");
    assert_eq!(dirs, vec![s(&l.wt)]);
    l.app.editor_closed(Ok(()));
    assert_eq!(l.ticket().worktree.and_then(|w| w.opened_in), None, "platoon waits for a terminal editor: its open marks cover it");
}

#[test]
fn the_settings_step_through_the_editors_this_computer_has() {
    use crate::settings::EditorFor;
    let w = World::new("fe-settings");
    let bin = w.base.join("bin");
    let code = window_stand_in(&bin, "code", "VS Code", &["--new-window"], Several::OneWindow);
    let mut app = w.app();
    app.set_file_editors_for_test(vec![code, helix(&bin)]);
    let nano = crate::editors::Editor { id: "nano".into(), name: "nano".into(), program: "/x/nano".into(), args: vec![], terminal: true };
    let kate = crate::editors::Editor { id: "kate".into(), name: "Kate".into(), program: "/x/kate".into(), args: vec![], terminal: false };
    app.set_editors_for_test(vec![nano, kate]);
    assert_eq!(app.editor_setting(EditorFor::Files).0, "VS Code", "none chosen yet: the first one found, which v opens");
    assert_eq!(app.editor_setting(EditorFor::Text).0, "None chosen yet");
    key(&mut app, KeyCode::Char('s'));
    assert!(matches!(app.mode, Mode::Settings));
    for _ in 0..5 {
        key(&mut app, KeyCode::Tab);
    }
    assert_eq!(app.items()[0].name, "Editor for notes and tasks (e)");
    key(&mut app, KeyCode::Right);
    assert_eq!(settings::load(&w.config).0.notes_editor, "nano");
    key(&mut app, KeyCode::Right);
    assert_eq!(settings::load(&w.config).0.notes_editor, "kate");
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Right);
    assert_eq!(settings::load(&w.config).0.files_editor, "helix");
    let (label, _, at_start, at_end) = app.editor_setting(EditorFor::Files);
    assert_eq!((label.as_str(), at_start, at_end), ("Helix", false, true));
    key(&mut app, KeyCode::Enter); // round to the first
    assert_eq!(settings::load(&w.config).0.files_editor, "code");
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert!(settings::load(&w.config).0.files_editor_ask);
    // One that isn't here any more reads as such.
    with_settings(&w, |s| s.files_editor = "zed".into());
    app.reload_for_test();
    assert_eq!(app.editor_setting(EditorFor::Files).0, "Zed (not found)");
    // No editor on this computer, none chosen: said so.
    with_settings(&w, |s| s.files_editor = String::new());
    app.reload_for_test();
    app.set_file_editors_for_test(Vec::new());
    assert_eq!(app.editor_setting(EditorFor::Files).0, "None found");
}

/// No editor on this computer: `v` says so in one plain line, in the footer, and nothing opens.
#[test]
fn v_with_no_editor_found_says_so() {
    use ratatui::{Terminal, backend::TestBackend};
    let w = World::new("fe-none");
    let main = w.base.join("repos/app");
    repo(&main);
    w.transcript("s-1", &s(&main), "hello", 5);
    let mut app = w.app();
    assert!(app.file_editors().is_empty(), "a unit test's app finds none");
    v(&mut app);
    assert!(matches!(app.mode, Mode::List) && app.editor_flow_for_test().is_none());
    assert!(app.take_own_editor_request().is_none());
    assert_eq!(notice(&app), crate::app::NO_EDITOR);
    let mut term = Terminal::new(TestBackend::new(110, 24)).unwrap();
    term.draw(|f| crate::ui::draw(f, &app)).unwrap();
    let buf = term.backend().buffer();
    let rows: Vec<String> = (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()).collect();
    assert!(rows.iter().any(|r| r.contains(crate::app::NO_EDITOR)), "{}", rows.join("\n"));
    assert!(!rows.join("\n").to_lowercase().contains("plugin"), "nothing offers to install one");
    // The same with "Always ask" on: no empty list.
    with_settings(&w, |s| s.files_editor_ask = true);
    app.reload_for_test();
    v(&mut app);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(notice(&app), crate::app::NO_EDITOR);
}
