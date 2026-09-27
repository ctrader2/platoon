//! The code editor (`v`) at the level of the app: what it opens from each place, the questions before it opens (the
//! squad is working, it isn't installed), a removed worktree brought back first, and the worktree clean-up never
//! taking away a folder the editor is about to open or has open. Real git in temp repositories; nothing is run (the
//! request the event loop would take is looked at instead).

use crate::app::{App, Confirm, EdTarget, Mode};
use crate::editor_install::{self, testing as fake};
use crate::host::HostInfo;
use crate::testkit::World;
use crate::tickets::{Leader, NewTicket, State, Tickets, Worktree};
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

/// The folders the request the event loop would take opens.
fn opened(app: &mut App) -> Vec<String> {
    app.take_editor_request().map(|r| r.dirs()).unwrap_or_else(|| panic!("nothing to open; mode {:?}, notice {:?}", matches!(app.mode, Mode::List), app.notice))
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
    app.ed.stand_in = true;
    assert_eq!(app.editor_target(), Some(EdTarget::Session("s-1".into())));
    v(&mut app);
    let req = app.take_editor_request().expect("opened at once: installed (a stand-in), nothing to ask");
    assert_eq!(req.dirs(), vec![s(&main), s(&docs)]);
    assert_eq!(req.roots[0].repo.as_deref(), Some(s(&main).as_str()));
    assert_eq!(req.roots[0].base, None);
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn v_with_nothing_selected_asks_for_a_folder_and_typing_v_there_is_a_letter() {
    let w = World::new("ed-picker");
    let mut app = w.app();
    app.ed.stand_in = true;
    assert_eq!(app.editor_target(), None, "an empty list");
    v(&mut app);
    assert!(matches!(app.mode, Mode::Picker) && app.picker.editor);
    assert!(app.take_editor_request().is_none());
    // In the picker's box, v is typed.
    v(&mut app);
    assert_eq!(app.picker.input, "v");
    key(&mut app, KeyCode::Backspace);
    for c in s(&w.base).chars() {
        key(&mut app, KeyCode::Char(c));
    }
    key(&mut app, KeyCode::Enter);
    assert_eq!(opened(&mut app), vec![s(&w.base)]);

    // The Squads tab and the Projects list ask too.
    key(&mut app, KeyCode::Char('3'));
    v(&mut app);
    assert!(matches!(app.mode, Mode::Picker) && app.picker.editor);
    key(&mut app, KeyCode::Esc);
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
    assert!(app.take_editor_request().is_none());
}

/// A project "Web shop" (its screen open, the Tickets pane in focus) with ticket #1 in the temp repository `shop`,
/// handed to a squad (leader `lead-1`) whose worktree has one commit and one change not yet committed.
struct Lab {
    w: World,
    app: App,
    project: String,
    shop: PathBuf,
    made: Worktree,
    wt: PathBuf,
    base: String,
}

fn lab(name: &str, state: State) -> Lab {
    let w = World::new(name);
    w.transcript("user-1", "/tmp", "make a basket page", 10);
    let shop = w.base.join("repos/shop");
    let base = repo(&shop);
    let mut app = w.app();
    app.ed.stand_in = true;
    let project = app.projects.create("Web shop").unwrap();
    app.projects.assign("user-1", Some(&project)).unwrap();
    let folders = vec![s(&shop)];
    let mut t = Tickets::open(&w.config, &project);
    let n = t.add(NewTicket { title: "Basket".into(), folders: folders.clone(), ..Default::default() }).unwrap();
    let made = crate::squads::prepare_worktrees("git", &folders, n, &[]).unwrap().remove(0);
    let lead = Leader { squad: "coding".into(), session: "lead-1".into(), started: 1, names: Default::default(), snapshot: None };
    t.start_leader(n, &lead, std::slice::from_ref(&made)).unwrap();
    let wt = PathBuf::from(&made.path);
    fs::write(wt.join("basket.txt"), "basket\n").unwrap();
    git(&wt, &["add", "."]);
    git(&wt, &["commit", "-q", "-m", "basket"]);
    if state == State::Cancelled {
        t.set_state(n, &State::Cancelled).unwrap();
    } else {
        t.squad_says(n, &state, "", None).unwrap();
    }
    app.reload_for_test();
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Enter);
    assert!(app.open_project().is_some());
    // The Tickets pane, on #1.
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
    Lab { w, app, project, shop, made, wt, base }
}

impl Lab {
    /// Open #1's own screen from the Tickets pane (a Done one is under the folded "Done and cancelled").
    fn open_ticket_screen(&mut self) {
        if self.app.selected_ticket().is_none() {
            key(&mut self.app, KeyCode::Enter);
            key(&mut self.app, KeyCode::Down);
        }
        assert_eq!(self.app.selected_ticket().map(|t| t.number), Some(1));
        key(&mut self.app, KeyCode::Enter);
        assert_eq!(self.app.tk.open, Some(1));
    }

    fn ticket(&self) -> crate::tickets::Ticket {
        Tickets::open(&self.w.config, &self.project).get(1).cloned().unwrap()
    }

    /// The leader's session runs in a platoon host.
    fn leader_hosted(&mut self) {
        let host = HostInfo { claude_pid: 4242, host_pid: 0, dir: s(&self.wt), args: vec!["--resume".into(), "lead-1".into()], sock: PathBuf::new() };
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
}

#[test]
fn v_on_a_ticket_opens_its_worktree_counted_from_the_commit_it_was_made_from() {
    let mut l = lab("ed-ticket", State::Waiting);
    assert_eq!(l.app.editor_target(), Some(EdTarget::Ticket { project: l.project.clone(), number: 1 }));
    v(&mut l.app);
    let req = l.app.take_editor_request().expect("opened");
    assert_eq!(req.dirs(), vec![s(&l.wt)]);
    assert_eq!(req.roots[0].base.as_deref(), Some(l.base.as_str()));
    assert_eq!(req.title, "#1 Basket");
    // And from the ticket's own screen.
    key(&mut l.app, KeyCode::Enter);
    assert_eq!(l.app.tk.open, Some(1));
    v(&mut l.app);
    assert_eq!(opened(&mut l.app), vec![s(&l.wt)]);
    let _ = (&l.shop, &l.made);
}

#[test]
fn the_squad_still_working_is_said_before_opening() {
    let mut l = lab("ed-working", State::Working);
    l.leader_hosted();
    assert!(l.app.leader_running(1));
    v(&mut l.app);
    assert!(matches!(&l.app.mode, Mode::Confirm(Confirm::EditorWhileWorking { number: 1, .. })));
    assert!(l.app.take_editor_request().is_none());
    key(&mut l.app, KeyCode::Char('n'));
    assert!(matches!(l.app.mode, Mode::List));
    assert!(l.app.take_editor_request().is_none(), "no: nothing opens");
    assert_eq!(l.app.editor_flow_for_test(), None);
    v(&mut l.app);
    key(&mut l.app, KeyCode::Char('y'));
    assert_eq!(opened(&mut l.app), vec![s(&l.wt)], "yes: it opens");

    // Waiting on the user (not working): no question.
    let mut t = Tickets::open(&l.w.config, &l.project);
    t.squad_says(1, &State::Waiting, "a question", None).unwrap();
    l.app.reload_for_test();
    v(&mut l.app);
    assert_eq!(opened(&mut l.app), vec![s(&l.wt)]);
}

#[test]
fn a_removed_worktree_is_brought_back_from_its_branch_before_the_editor_opens() {
    let mut l = lab("ed-back", State::Done);
    l.refresh_for(2.5);
    assert!(!l.wt.exists(), "removed after the work (the squad isn't running)");
    assert!(l.ticket().worktrees[0].removed.is_some());
    l.open_ticket_screen();
    v(&mut l.app);
    assert!(l.app.take_editor_request().is_none(), "not yet: it is being made again");
    assert!(l.app.notice.as_ref().is_some_and(|(n, ..)| n.contains("back")), "{:?}", l.app.notice);
    l.app.refresh_until_for_test(10, "the worktree back and the editor asked for", |a| a.worktrees_idle());
    assert!(l.wt.is_dir());
    assert!(l.wt.join("basket.txt").is_file(), "from its branch: the squad's commit is there");
    assert_eq!(l.ticket().worktrees[0].removed, None);
    assert_eq!(opened(&mut l.app), vec![s(&l.wt)]);
}

/// The race the brief names: platoon starts removing a Done ticket's worktree (its squad stopped) just before `v`.
/// The editor must still open on a worktree that is there (made again), never on one being taken away.
#[test]
fn v_just_after_the_clean_up_started_opens_on_a_worktree_that_stays() {
    let mut l = lab("ed-race", State::Working);
    l.open_ticket_screen();
    // The squad says Done; past the clean-up's interval, one refresh starts the removal off the UI thread (it hasn't
    // finished yet), then `v` at once.
    Tickets::open(&l.w.config, &l.project).squad_says(1, &State::Done, "", None).unwrap();
    std::thread::sleep(Duration::from_millis(2100));
    l.app.reload_for_test();
    assert!(!l.app.worktrees_idle(), "the removal is under way");
    v(&mut l.app);
    l.app.refresh_until_for_test(10, "the worktree back", |a| a.worktrees_idle());
    assert!(l.wt.is_dir(), "the editor's folder is there");
    assert_eq!(opened(&mut l.app), vec![s(&l.wt)]);
    assert_eq!(l.ticket().worktrees[0].removed, None);
}

/// While a `v` waits on a question (here: install?), or while an editor (this window's or another's) has the
/// worktree open, the clean-up leaves it alone; once the editor has closed, it goes as before.
#[test]
fn the_clean_up_leaves_a_worktree_the_editor_is_opening_or_has_open() {
    let mut l = lab("ed-hold", State::Working);
    // The question is open while the ticket turns Done and the squad stops.
    l.app.ed.stand_in = false;
    v(&mut l.app);
    assert!(matches!(l.app.mode, Mode::Confirm(Confirm::InstallEditor)));
    let mut t = Tickets::open(&l.w.config, &l.project);
    t.squad_says(1, &State::Done, "", None).unwrap();
    l.app.reload_for_test();
    l.refresh_for(2.5);
    assert!(l.wt.is_dir(), "kept while v is on its way");
    key(&mut l.app, KeyCode::Char('n'));

    // An editor has it open (a mark, as main.rs writes while the editor runs; here another window's, this process).
    let mark = crate::editor::mark_open(&l.w.cache, std::process::id(), &[s(&l.wt)]).unwrap();
    t.touch(1).unwrap(); // a new version of the ticket: tried again
    l.app.reload_for_test();
    l.refresh_for(2.5);
    assert!(l.wt.is_dir(), "kept while the editor has it open");
    crate::editor::unmark(&mark);
    t.touch(1).unwrap();
    l.app.reload_for_test();
    l.refresh_for(2.5);
    assert!(!l.wt.exists(), "removed once the editor has closed");
}

#[test]
fn not_installed_it_offers_to_install_then_opens_and_no_says_how_to_do_it_later() {
    let w = World::new("ed-install");
    let root = w.base.join("proj");
    repo(&root);
    w.transcript("s-1", &s(&root), "hello", 5);
    let mut app = w.app();
    let fake_dir = fake::tmp("app-install");
    let dirs = fake::dirs(&fake_dir, vec![fake::fake_cc(&fake_dir)]);
    let manifest = fake::fake(dirs.mirror.as_ref().unwrap());
    app.ed.dirs = dirs.clone();
    app.ed.manifest = Some(manifest.clone());
    app.ed.stand_in = false;
    // Removed by the user before: an explicit v still asks.
    crate::totickets::change_choice(&w.config, crate::setup::EDITOR, &[("asked", Some(true.into())), ("wanted", Some(false.into())), ("removed", Some(true.into()))]).unwrap();

    v(&mut app);
    assert!(matches!(app.mode, Mode::Confirm(Confirm::InstallEditor)));
    key(&mut app, KeyCode::Char('n'));
    assert!(app.notice.as_ref().is_some_and(|(n, ..)| n.contains("platoon setup --add editor")), "{:?}", app.notice);
    assert!(app.take_editor_request().is_none());
    assert!(!dirs.editor.join("installed.json").exists(), "nothing installed on a no");

    v(&mut app);
    key(&mut app, KeyCode::Char('y'));
    assert!(app.editor_busy());
    app.refresh_until_for_test(20, "the install to finish", |a| !a.editor_busy());
    assert!(crate::editor::ready(&editor_install::status_with(&dirs, &manifest)), "installed");
    assert_eq!(opened(&mut app), vec![s(&root)], "and then opened");
    let choice = crate::totickets::choice(&w.config, crate::setup::EDITOR);
    assert_eq!(choice.get("wanted"), Some(&serde_json::Value::Bool(true)));
    assert_eq!(choice.get("removed"), None, "wanted again, like `platoon setup --add editor`");

    // Installed: v opens straight away.
    v(&mut app);
    assert_eq!(opened(&mut app), vec![s(&root)]);
}

#[test]
fn an_install_that_fails_says_so_and_opens_nothing() {
    let w = World::new("ed-install-fail");
    w.transcript("s-1", &s(&w.base), "hello", 5);
    let mut app = w.app();
    let fake_dir = fake::tmp("app-install-fail");
    // Platoon's real pins against an empty mirror: every download is missing, and the network is never used.
    app.ed.dirs = fake::dirs(&fake_dir, vec![]);
    fs::create_dir_all(app.ed.dirs.mirror.as_ref().unwrap()).unwrap();
    app.ed.stand_in = false;
    v(&mut app);
    key(&mut app, KeyCode::Char('y'));
    app.refresh_until_for_test(20, "the install to stop", |a| !a.editor_busy());
    assert!(app.notice.as_ref().is_some_and(|(n, ..)| n.contains("didn't finish installing")), "{:?}", app.notice);
    assert!(app.take_editor_request().is_none());
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
    FileEditor { id: id.into(), name: name.into(), program: s(&script), args: args.iter().map(|a| a.to_string()).collect(), runs: Runs::Window, several }
}

fn helix(dir: &Path) -> FileEditor {
    FileEditor { id: "helix".into(), name: "Helix".into(), program: s(&dir.join("hx")), args: vec![], runs: Runs::Terminal, several: Several::First }
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

    // The default: platoon's own, no list.
    let mut app = w.app();
    app.ed.stand_in = true;
    app.set_file_editors_for_test(vec![FileEditor::platoon(), code.clone(), helix(&bin)]);
    v(&mut app);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(opened(&mut app), vec![s(&main), s(&docs)]);

    // VS Code the favourite: started beside platoon with both folders in one window, and platoon stays as it was.
    with_settings(&w, |s| s.files_editor = "code".into());
    app.reload_for_test();
    v(&mut app);
    assert!(matches!(app.mode, Mode::List), "no list, no question");
    assert!(app.take_editor_request().is_none() && app.take_own_editor_request().is_none(), "nothing takes the terminal");
    assert_eq!(calls(&bin, "code", 1), vec![format!("--new-window {} {}", s(&main), s(&docs))]);
    assert_eq!(notice(&app), "Opened in VS Code, in a window of its own.");

    // Helix the favourite: handed the terminal (by the event loop) on the first folder; it says what it left out
    // once it has closed.
    with_settings(&w, |s| s.files_editor = "helix".into());
    app.reload_for_test();
    v(&mut app);
    let (e, dirs) = app.take_own_editor_request().expect("a terminal editor of the user's");
    assert_eq!((e.id.as_str(), dirs), ("helix", vec![s(&main), s(&docs)]));
    assert!(app.take_editor_request().is_none());
    app.editor_closed(Ok(()));
    assert!(notice(&app).starts_with("Helix opened ") && notice(&app).contains("only: it opens one folder at a time. Not opened:"), "{}", notice(&app));
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
    app.ed.stand_in = true;
    app.set_file_editors_for_test(vec![FileEditor::platoon(), code, helix(&bin)]);
    v(&mut app);
    assert!(matches!(app.mode, Mode::PickFileEditor));
    let ids: Vec<&str> = app.file_pick().options.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["platoon", "code", "helix"], "platoon's own first, then the computer's");
    assert_eq!((app.file_pick().starred.as_str(), app.file_pick().selected), ("platoon", 0));
    // Esc: nothing opens.
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List) && app.editor_flow_for_test().is_none());
    assert!(app.take_editor_request().is_none());

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

    // Platoon's own, chosen from the list, still asks to install it first when it isn't there.
    app.ed.stand_in = false;
    v(&mut app);
    assert!(app.file_pick().platoon_missing);
    key(&mut app, KeyCode::Char('g'));
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Confirm(Confirm::InstallEditor)));
    key(&mut app, KeyCode::Char('n'));
    assert_eq!(settings::load(&w.config).0.files_editor, "platoon");
}

#[test]
fn a_favourite_no_longer_installed_opens_platoons_editor_and_says_so_once() {
    let w = World::new("fe-gone");
    let main = w.base.join("repos/app");
    repo(&main);
    w.transcript("s-1", &s(&main), "hello", 5);
    with_settings(&w, |s| s.files_editor = "rustrover".into());
    let mut app = w.app();
    app.ed.stand_in = true;
    v(&mut app);
    assert_eq!(opened(&mut app), vec![s(&main)]);
    assert_eq!(notice(&app), "RustRover isn't on this computer any more, so platoon's editor opens instead (Settings, Editors).");
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
    let added: Vec<String> = (1..=4).map(|i| w.base.join(format!("more-{i}"))).inspect(|d| fs::create_dir_all(d).unwrap()).map(|d| s(&d)).collect();
    with_added(&w, "s-1", &s(&main), &added);
    let bin = w.base.join("bin");
    let idea = window_stand_in(&bin, "idea", "IntelliJ IDEA", &[], Several::EachWindow);
    with_settings(&w, |s| s.files_editor = "idea".into());
    let mut app = w.app();
    app.set_file_editors_for_test(vec![FileEditor::platoon(), idea]);
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
fn a_worktree_opened_in_a_window_editor_is_kept_after_the_work_until_w() {
    let mut l = lab("fe-kept", State::Waiting);
    let bin = l.w.base.join("bin");
    let code = window_stand_in(&bin, "code", "VS Code", &["--new-window"], Several::OneWindow);
    with_settings(&l.w, |s| s.files_editor = "code".into());
    l.app.reload_for_test();
    l.app.set_file_editors_for_test(vec![FileEditor::platoon(), code]);
    v(&mut l.app);
    assert_eq!(calls(&bin, "code", 1), vec![format!("--new-window {}", s(&l.wt))]);
    assert_eq!(l.ticket().worktrees[0].opened_in.as_deref(), Some("VS Code"), "written on the ticket: it survives a restart");
    assert!(notice(&l.app).contains("#1's worktree stays after the work until w on the ticket removes it"), "{}", notice(&l.app));

    // Done, its squad not running: the clean-up leaves it (platoon can't tell when VS Code closes).
    Tickets::open(&l.w.config, &l.project).squad_says(1, &State::Done, "", None).unwrap();
    l.app.reload_for_test();
    l.refresh_for(2.5);
    assert!(l.wt.is_dir(), "kept");
    assert!(l.ticket().worktrees[0].removed.is_none());
    assert!(l.app.can_remove_worktree(1));

    // `w` asks, naming the editor; yes removes it and keeps the branch.
    l.open_ticket_screen();
    key(&mut l.app, KeyCode::Char('w'));
    match &l.app.mode {
        Mode::Confirm(Confirm::RemoveWorktree { number: 1, opened_in, uncommitted, .. }) => {
            assert_eq!(opened_in.as_deref(), Some("VS Code"));
            assert!(!uncommitted, "everything was committed");
        }
        _ => panic!("not asked"),
    }
    key(&mut l.app, KeyCode::Char('y'));
    l.app.refresh_until_for_test(10, "the removal", |a| a.worktrees_idle());
    assert!(!l.wt.exists());
    let wt = &l.ticket().worktrees[0];
    assert!(wt.removed.is_some() && wt.opened_in.is_none(), "{wt:?}");
    assert!(git(&l.shop, &["branch", "--list", "ticket/1"]).contains("ticket/1"), "the branch is kept");
}

#[test]
fn a_terminal_editor_keeps_nothing_after_the_work() {
    let mut l = lab("fe-term", State::Waiting);
    let bin = l.w.base.join("bin");
    with_settings(&l.w, |s| s.files_editor = "helix".into());
    l.app.reload_for_test();
    l.app.set_file_editors_for_test(vec![FileEditor::platoon(), helix(&bin)]);
    v(&mut l.app);
    let (_, dirs) = l.app.take_own_editor_request().expect("handed the terminal");
    assert_eq!(dirs, vec![s(&l.wt)]);
    l.app.editor_closed(Ok(()));
    assert_eq!(l.ticket().worktrees[0].opened_in, None, "platoon waits for a terminal editor: its open marks cover it");
}

#[test]
fn the_settings_step_through_the_editors_this_computer_has() {
    use crate::settings::EditorFor;
    let w = World::new("fe-settings");
    let bin = w.base.join("bin");
    let code = window_stand_in(&bin, "code", "VS Code", &["--new-window"], Several::OneWindow);
    let mut app = w.app();
    app.set_file_editors_for_test(vec![FileEditor::platoon(), code]);
    let nano = crate::editors::Editor { id: "nano".into(), name: "nano".into(), program: "/x/nano".into(), args: vec![], terminal: true };
    let kate = crate::editors::Editor { id: "kate".into(), name: "Kate".into(), program: "/x/kate".into(), args: vec![], terminal: false };
    app.set_editors_for_test(vec![nano, kate]);
    assert_eq!(app.editor_setting(EditorFor::Files).0, "platoon's editor");
    assert_eq!(app.editor_setting(EditorFor::Text).0, "None chosen yet");
    key(&mut app, KeyCode::Char('s'));
    assert!(matches!(app.mode, Mode::Settings));
    for _ in 0..5 {
        key(&mut app, KeyCode::Tab);
    }
    assert_eq!(app.items()[0].name, "Editor for notes and tickets");
    key(&mut app, KeyCode::Right);
    assert_eq!(settings::load(&w.config).0.notes_editor, "nano");
    key(&mut app, KeyCode::Right);
    assert_eq!(settings::load(&w.config).0.notes_editor, "kate");
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Right);
    assert_eq!(settings::load(&w.config).0.files_editor, "code");
    let (label, _, at_start, at_end) = app.editor_setting(EditorFor::Files);
    assert_eq!((label.as_str(), at_start, at_end), ("VS Code", false, true));
    key(&mut app, KeyCode::Enter); // round to the first
    assert_eq!(settings::load(&w.config).0.files_editor, "platoon");
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert!(settings::load(&w.config).0.files_editor_ask);
    // One that isn't here any more reads as such.
    with_settings(&w, |s| s.files_editor = "zed".into());
    app.reload_for_test();
    assert_eq!(app.editor_setting(EditorFor::Files).0, "Zed (not found)");
}
