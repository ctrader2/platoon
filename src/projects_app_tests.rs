//! Projects at the level of the app: the tabs, the Projects tab and its keys, the project picker (`m`), badges on
//! session rows, a project's screen, the scoped search and what archiving, restoring and deleting a project does.
//! Every directory is a temp dir (`World`); the only processes signalled are the tests' own stand-ins.

use crate::app::{App, AssignOpt, Confirm, Hit, Mode, Pane, FIND_KIND};
use crate::host::HostInfo;
use crate::projects::{self, Projects};
use crate::testkit::{alive, now_ms, AiFake, World};
use crate::ui;
use ratatui::{
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
    Terminal,
};
use std::{fs, time::Duration};

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn type_str(app: &mut App, text: &str) {
    for c in text.chars() {
        key(app, KeyCode::Char(c));
    }
}

fn draw(term: &mut Terminal<TestBackend>, app: &App) -> String {
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

fn flat(text: &str) -> String {
    text.replace(['│', '╭', '╮', '╰', '╯', '─'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn click_hit(app: &mut App, want: impl Fn(&Hit) -> bool) {
    let r = app.hits.borrow().iter().find(|(_, h)| want(h)).map(|(r, _)| *r).expect("not drawn");
    app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: r.x + 1, row: r.y, modifiers: KeyModifiers::NONE });
}

fn ids(app: &App) -> Vec<String> {
    app.visible().iter().map(|&i| app.sessions[i].id.clone()).collect()
}

fn select(app: &mut App, id: &str) {
    app.selected = app.visible().iter().position(|&i| app.sessions[i].id == id).expect("listed");
}

fn hosted(pid: u32) -> HostInfo {
    HostInfo { claude_pid: pid, host_pid: 0, dir: "/tmp".into(), args: Vec::new(), sock: Default::default() }
}

/// Four conversations: alpha and beta in /srv/a, gamma and delta in /srv/b (each prompt names a fruit).
fn four(name: &str) -> World {
    let w = World::new(name);
    w.transcript("alpha-1", "/tmp", "alpha talks about apples", 10);
    w.transcript("beta-2", "/tmp", "beta talks about apples too", 20);
    w.transcript("gamma-3", "/tmp", "gamma talks about apples as well", 30);
    w.transcript("delta-4", "/tmp", "delta talks about pears", 40);
    w
}

/// Make a project through the keys, from wherever the app is: returns its id.
fn make_project(app: &mut App, name: &str) -> String {
    key(app, KeyCode::Char('2'));
    key(app, KeyCode::Char('n'));
    assert!(matches!(app.mode, Mode::ProjectName));
    type_str(app, name);
    key(app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));
    app.projects.all().iter().find(|p| p.name == name).expect("made").id.clone()
}

/// Put a session in a project with `m`, from the Sessions tab.
fn put_in(app: &mut App, session: &str, project: &str) {
    key(app, KeyCode::Char('1'));
    select(app, session);
    key(app, KeyCode::Char('m'));
    assert!(matches!(app.mode, Mode::Assign));
    app.assign.selected = app.assign.options.iter().position(|o| *o == AssignOpt::Project(project.to_string())).expect("offered");
    key(app, KeyCode::Enter);
    assert_eq!(app.projects.project_id_of(session), Some(project));
}

#[test]
fn a_project_is_made_renamed_and_deleted_from_the_projects_tab() {
    let w = four("p-crud");
    let mut app = w.app();
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    key(&mut app, KeyCode::Char('2'));
    assert!(flat(&draw(&mut term, &app)).contains("No projects yet. Press n to make one."));
    // An empty name is refused and the box stays open.
    key(&mut app, KeyCode::Char('n'));
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::ProjectName) && app.active_notice() == Some("A project needs a name"));
    type_str(&mut app, "Web shop");
    key(&mut app, KeyCode::Enter);
    let id = app.projects.all()[0].id.clone();
    assert_eq!(app.selected_project().map(|p| p.name.as_str()), Some("Web shop"));
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("1 Sessions 4 2 Projects 1") && shown.contains("WS Web shop") && shown.contains("no sessions"), "{shown}");
    assert!(shown.contains("run `platoon setup` once more"), "said once, since the notes hook is not installed: {shown}");
    // Saved where it belongs.
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(w.config.join("projects.json")).unwrap()).unwrap();
    assert_eq!(v["projects"][&id]["name"], "Web shop");

    key(&mut app, KeyCode::Char('r'));
    assert_eq!(app.name_box.input, "Web shop");
    for _ in 0..4 {
        key(&mut app, KeyCode::Backspace);
    }
    type_str(&mut app, "store");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.projects.get(&id).unwrap().name, "Web store");

    put_in(&mut app, "alpha-1", &id);
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Char('d'));
    let Mode::Confirm(Confirm::DeleteProject { sessions, .. }) = app.mode.clone() else { panic!("asks first") };
    assert_eq!(sessions, 1);
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("Its notes are deleted too") && shown.contains("no conversation is deleted"), "{shown}");
    key(&mut app, KeyCode::Char('n')); // no
    assert!(app.projects.get(&id).is_some());
    key(&mut app, KeyCode::Char('d'));
    key(&mut app, KeyCode::Char('y'));
    assert!(app.projects.all().is_empty() && app.projects.of("alpha-1").is_none());
    assert!(w.cache.join("deleted-projects").join(&id).join("project.json").exists(), "one backup copy");
    key(&mut app, KeyCode::Char('1'));
    assert_eq!(ids(&app).len(), 4, "every conversation is still there");
}

#[test]
fn m_offers_the_active_projects_new_project_and_no_project() {
    let w = four("p-assign");
    let mut app = w.app();
    let a = make_project(&mut app, "Apples");
    let b = make_project(&mut app, "Bananas");
    let old = make_project(&mut app, "Old stuff");
    // Archive "Old stuff": `m` never offers it.
    app.pselected = app.project_rows().iter().position(|&i| app.projects.all()[i].id == old).unwrap();
    key(&mut app, KeyCode::Char('a'));
    assert!(app.projects.get(&old).unwrap().archived);
    key(&mut app, KeyCode::Char('1'));
    select(&mut app, "alpha-1");
    key(&mut app, KeyCode::Char('m'));
    assert_eq!(app.assign.options, [AssignOpt::Project(a.clone()), AssignOpt::Project(b.clone()), AssignOpt::New, AssignOpt::None]);
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    app.notice = None;
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("Put \"alpha\" in:") || shown.contains("Put \"tmp\" in:"), "{shown}");
    assert!(shown.contains("AP Apples") && shown.contains("New project…") && shown.contains("No project") && !shown.contains("Old stuff"), "{shown}");
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List) && app.projects.of("alpha-1").is_none());

    put_in(&mut app, "alpha-1", &a);
    // Moving it: one project at a time.
    put_in(&mut app, "alpha-1", &b);
    assert_eq!(app.projects.members_of(&a).len(), 0);
    // "New project…" asks for a name and puts the session in it.
    select(&mut app, "beta-2");
    key(&mut app, KeyCode::Char('m'));
    app.assign.selected = app.assign.options.iter().position(|o| *o == AssignOpt::New).unwrap();
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::ProjectName));
    type_str(&mut app, "Cherries");
    key(&mut app, KeyCode::Enter);
    let c = app.projects.all().iter().find(|p| p.name == "Cherries").unwrap().id.clone();
    assert_eq!(app.projects.project_id_of("beta-2"), Some(c.as_str()));
    assert_eq!(app.top, 0, "still on the Sessions tab");
    // "No project" takes it out.
    select(&mut app, "beta-2");
    key(&mut app, KeyCode::Char('m'));
    app.assign.selected = app.assign.options.len() - 1;
    key(&mut app, KeyCode::Enter);
    assert!(app.projects.of("beta-2").is_none());
    // A click on the highlighted entry takes it too.
    key(&mut app, KeyCode::Char('m'));
    draw(&mut term, &app);
    click_hit(&mut app, |h| matches!(h, Hit::AssignRow(0)));
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn a_session_in_a_project_carries_its_badge_in_the_projects_color() {
    let w = four("p-badge");
    let mut app = w.app();
    let a = make_project(&mut app, "Web shop");
    put_in(&mut app, "alpha-1", &a);
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    let shown = draw(&mut term, &app);
    let t = app.theme.clone();
    let buf = term.backend().buffer().clone();
    let row = shown.lines().position(|l| l.contains(" WS ")).expect("the badge on a row");
    let line: Vec<char> = shown.lines().nth(row).unwrap().chars().collect();
    let col = (0..line.len()).find(|&i| line[i] == 'W' && line[i + 1] == 'S').unwrap() as u16;
    let color = projects::slot_color(&t, &app.projects.get(&a).unwrap().color);
    assert_eq!((buf[(col, row as u16)].bg, buf[(col, row as u16)].fg), (color, projects::badge_text(&t)));
    assert_ne!(color, t.bg);
    // Only that row: the others have no badge.
    assert_eq!(shown.matches(" WS ").count(), 1, "{shown}");

    // Color and badge can be changed: the row follows.
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Char('c'));
    assert!(matches!(app.mode, Mode::Look));
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Right);
    for _ in 0..3 {
        key(&mut app, KeyCode::Backspace);
    }
    type_str(&mut app, "shop!"); // three at most
    assert_eq!(app.look.badge, "sho");
    let slot = app.look.slot;
    key(&mut app, KeyCode::Enter);
    let p = app.projects.get(&a).unwrap().clone();
    assert_eq!((p.badge.as_str(), p.color.as_str()), ("sho", projects::SLOTS[slot].0));
    key(&mut app, KeyCode::Char('1'));
    let shown = draw(&mut term, &app);
    let row = shown.lines().position(|l| l.contains(" sho ")).expect("the new badge");
    let line: Vec<char> = shown.lines().nth(row).unwrap().chars().collect();
    let col = (0..line.len()).find(|&i| line[i] == 's' && line[i + 1] == 'h').unwrap() as u16;
    assert_eq!(term.backend().buffer()[(col, row as u16)].bg, projects::slot_color(&t, &p.color));
}

#[test]
fn left_and_right_switch_active_and_archived_on_both_tabs() {
    let w = four("p-shelf");
    fs::create_dir_all(&w.config).unwrap();
    fs::write(w.config.join("archive.json"), r#"{"version":1,"archived":{"delta-4":5}}"#).unwrap();
    let mut app = w.app();
    let a = make_project(&mut app, "Apples");
    make_project(&mut app, "Bananas");
    app.pselected = app.project_rows().iter().position(|&i| app.projects.all()[i].id == a).unwrap();
    key(&mut app, KeyCode::Char('a'));
    assert_eq!(app.pcounts, [1, 1]);
    assert_eq!(app.project_rows().len(), 1);
    key(&mut app, KeyCode::Right);
    assert_eq!((app.ptab, app.selected_project().map(|p| p.name.clone())), (1, Some("Apples".into())));
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    app.notice = None;
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("Active 1 Archived 1") && shown.contains("a restore"), "{shown}");
    key(&mut app, KeyCode::Left);
    assert_eq!(app.ptab, 0);
    // The Sessions tab keeps its own.
    key(&mut app, KeyCode::Char('1'));
    assert_eq!(app.tab, 0);
    key(&mut app, KeyCode::Right);
    assert_eq!(ids(&app), ["delta-4"]);
    // And the chips click.
    draw(&mut term, &app);
    click_hit(&mut app, |h| matches!(h, Hit::Tab(0)));
    assert_eq!(app.tab, 0);
    key(&mut app, KeyCode::Char('2'));
    draw(&mut term, &app);
    click_hit(&mut app, |h| matches!(h, Hit::Tab(1)));
    assert_eq!(app.ptab, 1);
}

#[test]
fn a_projects_screen_lists_its_sessions_active_first_and_shows_its_notes() {
    let w = four("p-screen");
    fs::create_dir_all(&w.config).unwrap();
    fs::write(w.config.join("archive.json"), r#"{"version":1,"archived":{"delta-4":5}}"#).unwrap();
    let mut app = w.app();
    let a = make_project(&mut app, "Fruit");
    for s in ["alpha-1", "gamma-3"] {
        put_in(&mut app, s, &a);
    }
    // delta is archived: `m` works on the Archived tab too.
    key(&mut app, KeyCode::Right);
    select(&mut app, "delta-4");
    key(&mut app, KeyCode::Char('m'));
    app.assign.selected = 0;
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.open_project().map(|p| p.name.as_str()), Some("Fruit"));
    assert_eq!(ids(&app), ["alpha-1", "gamma-3", "delta-4"], "active first, then archived");
    assert_eq!(app.archived_rows, 1);
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    let shown = draw(&mut term, &app);
    let f = flat(&shown);
    assert!(f.contains("FR Fruit") && f.contains("2 active · 1 archived") && f.contains("Archived (1)"), "{shown}");
    assert!(f.contains("Sessions") && f.contains("Notes") && f.contains("No notes yet"), "{shown}");
    assert!(f.contains("They live in") && f.contains("project.md"), "says where they live: {shown}");
    assert!(!f.contains("beta talks"), "{shown}");
    // Focus: left and right move it between the panes; the focused pane has the accent border.
    assert_eq!(app.pane, Pane::Sessions);
    key(&mut app, KeyCode::Right);
    assert_eq!(app.pane, Pane::Notes);
    key(&mut app, KeyCode::Down); // scrolls the notes, not the sessions
    assert_eq!(app.selected, 0);
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Down);
    assert_eq!(app.selected, 1);
    // A click on a pane focuses it.
    draw(&mut term, &app);
    click_hit(&mut app, |h| matches!(h, Hit::Pane(Pane::Notes)));
    assert_eq!(app.pane, Pane::Notes);
    // Notes: read only, shown as written.
    let notes = app.projects.notes_path(&a);
    fs::create_dir_all(notes.parent().unwrap()).unwrap();
    fs::write(&notes, "# Overview\nA shop for fruit.\n\n## Decisions\n- apples first\n").unwrap();
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("# Overview") && f.contains("A shop for fruit.") && f.contains("- apples first") && !f.contains("No notes yet"), "{f}");
    // Esc goes back to the Projects tab with the project selected; the Projects tab shows the counts and notes.
    key(&mut app, KeyCode::Esc);
    assert!(app.open_project.is_none() && app.projects_list_showing());
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("2 active · 1 archived") && f.contains("notes now"), "{f}");
}

#[test]
fn search_on_a_projects_screen_only_finds_its_sessions() {
    let w = four("p-search");
    let mut app = w.app();
    let a = make_project(&mut app, "Fruit");
    put_in(&mut app, "alpha-1", &a);
    put_in(&mut app, "delta-4", &a);
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Enter);
    // Quick filter: narrows the project's own list.
    key(&mut app, KeyCode::Char('/'));
    type_str(&mut app, "talks");
    assert_eq!(ids(&app), ["alpha-1", "delta-4"]);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.filter, "talks");
    key(&mut app, KeyCode::Esc); // clears the filter first
    assert!(app.open_project.is_some() && app.filter.is_empty());
    // Full-text: "apples" is in three conversations, one of them in the project.
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Tab);
    for _ in 0..5 {
        key(&mut app, KeyCode::Backspace);
    }
    type_str(&mut app, "apples");
    let found: Vec<&str> = app.search.results.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(found, ["alpha-1"]);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List) && app.open_project.is_some(), "stays on the project's screen");
    assert_eq!(app.selected_session().map(|s| s.id.as_str()), Some("alpha-1"));
    // The same words on the Sessions tab find all three.
    key(&mut app, KeyCode::Char('1'));
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Tab);
    type_str(&mut app, "apples");
    assert_eq!(app.search.results.len(), 3);
    key(&mut app, KeyCode::Esc);
    // On the Projects tab the search narrows the projects (and is only the quick filter).
    make_project(&mut app, "Vegetables");
    key(&mut app, KeyCode::Char('/'));
    assert_eq!(app.search_kinds(), [0]);
    type_str(&mut app, "veg");
    assert_eq!(app.project_rows().len(), 1);
    assert_eq!(app.selected_project().map(|p| p.name.as_str()), Some("Vegetables"));
}

#[test]
fn describe_it_on_a_projects_screen_sends_only_its_sessions() {
    let w = four("p-find");
    let mut s = crate::settings::load(&w.config).0;
    s.tokens_enabled = true;
    s.set_feature("find", true);
    crate::settings::save(&w.config, &s).unwrap();
    let fake = AiFake::new("p-find");
    let mut app = w.app();
    app.set_ai_program_for_test(&fake.script);
    let a = make_project(&mut app, "Fruit");
    put_in(&mut app, "gamma-3", &a);
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.search.kind, FIND_KIND);
    type_str(&mut app, "the one about apples");
    key(&mut app, KeyCode::Enter);
    app.settle_ai_for_test(10);
    assert_eq!(fake.calls(), 1);
    let sent = fake.stdin(1);
    assert!(sent.contains("gamma talks") && !sent.contains("alpha talks") && !sent.contains("beta talks") && !sent.contains("pears"), "{sent}");
}

#[test]
fn archiving_a_project_archives_its_active_sessions_stopping_a_live_one_and_restoring_brings_back_exactly_those() {
    let mut w = four("p-archive");
    fs::create_dir_all(&w.config).unwrap();
    // gamma was archived before the project was.
    fs::write(w.config.join("archive.json"), r#"{"version":1,"archived":{"gamma-3":5}}"#).unwrap();
    let pid = w.live_slow_exit("alpha-1", "/tmp", 1, "idle", 0.3);
    let mut app = w.app();
    app.set_stop_times_for_test(Duration::from_secs(5), Duration::from_secs(5));
    app.refresh_with(vec![hosted(pid)], false);
    let a = make_project(&mut app, "Fruit");
    for s in ["alpha-1", "beta-2"] {
        put_in(&mut app, s, &a);
    }
    key(&mut app, KeyCode::Right);
    select(&mut app, "gamma-3");
    key(&mut app, KeyCode::Char('m'));
    app.assign.selected = 0;
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Left);
    // alpha runs under platoon and is idle: no question, it is stopped in the background.
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Char('a'));
    assert!(matches!(app.mode, Mode::List), "nothing destructive to ask about");
    assert!(app.projects.get(&a).unwrap().archived);
    assert_eq!(app.stops_pending(), 1);
    assert!(app.settle_stops_for_test(Duration::from_secs(8)), "the stop finished");
    assert!(!alive(pid), "our stand-in was stopped");
    let archived = |w: &World| w.archive_file().unwrap()["archived"].as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    assert_eq!(archived(&w), ["alpha-1", "beta-2", "gamma-3"]);
    let mut back = app.projects.get(&a).unwrap().archived_sessions.clone();
    back.sort();
    assert_eq!(back, ["alpha-1", "beta-2"]);
    assert_eq!(app.pcounts, [0, 1]);
    // Restore: exactly those two come back; gamma stays archived; all three stay in the project.
    app.refresh_for_test();
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Char('a'));
    assert!(!app.projects.get(&a).unwrap().archived);
    assert_eq!(archived(&w), ["gamma-3"]);
    assert_eq!(app.projects.members_of(&a).len(), 3);
    assert!(app.active_notice().unwrap().contains("Restored the project \"Fruit\" and 2 sessions"));
}

#[test]
fn a_session_of_an_archived_project_can_be_restored_on_its_own_and_stays_in_it() {
    let w = four("p-one");
    let mut app = w.app();
    let a = make_project(&mut app, "Fruit");
    for s in ["alpha-1", "beta-2"] {
        put_in(&mut app, s, &a);
    }
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Char('a'));
    // From the project's screen (on the Archived projects): restore beta alone.
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Enter);
    assert_eq!(ids(&app), ["alpha-1", "beta-2"]);
    assert_eq!(app.archived_rows, 2);
    select(&mut app, "beta-2");
    key(&mut app, KeyCode::Char('a'));
    assert!(app.projects.get(&a).unwrap().archived, "the project stays archived");
    assert_eq!(app.projects.project_id_of("beta-2"), Some(a.as_str()), "and beta stays in it");
    assert_eq!(ids(&app), ["beta-2", "alpha-1"], "beta is an active row now");
    assert_eq!(app.projects.get(&a).unwrap().archived_sessions, ["alpha-1"]);
    // Restoring the project later brings back alpha.
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Char('a'));
    assert!(w.archive_file().unwrap()["archived"].as_object().unwrap().is_empty());
}

#[test]
fn auto_archive_applies_to_sessions_in_projects_and_the_project_stays_active() {
    let w = World::new("p-auto");
    w.transcript("old-1", "/tmp", "an old one", 5 * 24 * 60);
    w.transcript("new-2", "/tmp", "a new one", 5);
    let mut app = w.app();
    let a = make_project(&mut app, "Mixed");
    put_in(&mut app, "old-1", &a);
    put_in(&mut app, "new-2", &a);
    w.settings(120, 2);
    app.auto_archive_at_for_test(crate::usage::now());
    assert!(w.archive_file().unwrap()["archived"]["old-1"].is_u64());
    assert!(!app.projects.get(&a).unwrap().archived);
    assert!(app.projects.get(&a).unwrap().archived_sessions.is_empty(), "archived by age, not by the project");
    assert_eq!(app.pstats[&a].archived, 1);
}

#[test]
fn a_session_started_under_a_project_is_put_in_it_once_its_id_shows_up() {
    let w = World::new("p-launch");
    let mut app = w.app();
    let a = make_project(&mut app, "Fruit");
    // What `launch_in` records for a new session, then the process registers with its new id.
    let child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
    let pid = child.id();
    projects::note_launch(app.cache_for_test(), pid, &a, now_ms() - 2000).unwrap();
    w.register(pid, "brand-new", "/tmp", now_ms(), "idle");
    app.refresh_for_test();
    assert_eq!(app.projects.project_id_of("brand-new"), Some(a.as_str()));
    // It shows up on the project's screen.
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Enter);
    assert_eq!(ids(&app), ["brand-new"]);
    let mut child = child;
    let _ = child.kill();
    let _ = child.wait();
    app.refresh_for_test();
    assert!(projects::launches(app.cache_for_test()).is_empty(), "forgotten once the process has gone");
}

#[test]
fn n_on_a_projects_screen_offers_the_projects_folders_first() {
    let w = World::new("p-folders");
    let (d1, d2) = (w.base.join("one"), w.base.join("two"));
    fs::create_dir_all(&d1).unwrap();
    fs::create_dir_all(&d2).unwrap();
    w.transcript("s1", &d1.to_string_lossy(), "one", 10);
    w.transcript("s2", &d2.to_string_lossy(), "two", 5);
    w.transcript("s3", "/tmp", "other", 1);
    let mut app = w.app();
    let a = make_project(&mut app, "Two folders");
    put_in(&mut app, "s1", &a);
    put_in(&mut app, "s2", &a);
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('n'));
    assert!(matches!(app.mode, Mode::Picker));
    assert_eq!(app.picker.project.as_deref(), Some(a.as_str()));
    let first: Vec<String> = app.picker.results.iter().take(2).cloned().collect();
    assert_eq!(first, [d2.to_string_lossy().into_owned(), d1.to_string_lossy().into_owned()], "the project's folders, most recent first");
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    assert!(draw(&mut term, &app).contains("new session in Two folders"));
}

#[test]
fn the_projects_file_is_shared_with_other_windows_and_the_hook() {
    let w = four("p-shared");
    let mut app = w.app();
    // Another window (or the hook) makes a project and places a session.
    let mut other = Projects::open(&w.config);
    let id = other.create("Elsewhere").unwrap();
    other.assign("beta-2", Some(&id)).unwrap();
    app.refresh_for_test();
    assert_eq!(app.pcounts, [1, 0]);
    assert_eq!(app.projects.project_id_of("beta-2"), Some(id.as_str()));
    // Deleted there while its screen is open here: the screen closes.
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Enter);
    assert!(app.open_project.is_some());
    other.delete(&id, &w.cache).unwrap();
    app.refresh_for_test();
    assert!(app.open_project.is_none() && app.pcounts == [0, 0]);
}

#[test]
fn the_new_screens_fit_small_terminals() {
    let w = four("p-small");
    let mut app = w.app();
    let a = make_project(&mut app, "A project with a rather long name indeed");
    put_in(&mut app, "alpha-1", &a);
    for (cols, rows) in [(40u16, 12u16), (60, 16), (80, 24)] {
        let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
        key(&mut app, KeyCode::Char('1'));
        assert!(draw(&mut term, &app).contains("platoon"));
        key(&mut app, KeyCode::Char('2'));
        draw(&mut term, &app);
        key(&mut app, KeyCode::Enter);
        draw(&mut term, &app);
        key(&mut app, KeyCode::Right);
        draw(&mut term, &app);
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Char('c'));
        draw(&mut term, &app);
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Char('n'));
        draw(&mut term, &app);
        key(&mut app, KeyCode::Esc);
    }
    // At 80 columns the Sessions tab's footer keeps its main keys.
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    key(&mut app, KeyCode::Char('1'));
    app.notice = None;
    let shown = draw(&mut term, &app);
    for want in ["enter open", "a archive", "/ search", "q quit"] {
        assert!(shown.contains(want), "{want}: {shown}");
    }
}
