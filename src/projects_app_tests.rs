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
    assert!(shown.contains("run `platoon setu"), "said once, since the notes hooks are not installed: {shown}");
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

/// The screen's cells, line by line.
fn cells(term: &Terminal<TestBackend>) -> Vec<Vec<ratatui::buffer::Cell>> {
    let buf = term.backend().buffer();
    (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].clone()).collect()).collect()
}

fn text_of(line: &[ratatui::buffer::Cell]) -> String {
    line.iter().map(|c| c.symbol()).collect()
}

/// The column (in cells) where `word` starts on a line of cells.
fn col_of(line: &[ratatui::buffer::Cell], word: &str) -> Option<usize> {
    let syms: Vec<String> = line.iter().map(|c| c.symbol().to_string()).collect();
    let w: Vec<String> = word.chars().map(|c| c.to_string()).collect();
    (0..syms.len().saturating_sub(w.len() - 1)).find(|&i| syms[i..i + w.len()] == w[..])
}

/// The Sessions table's title line.
fn header_row(lines: &[Vec<ratatui::buffer::Cell>]) -> usize {
    lines.iter().position(|l| col_of(l, "STATUS").is_some() && col_of(l, "CONTEXT").is_some()).expect("the column titles")
}

/// One column's share of a wide screen's spare width, read off the title line: STATUS is 11 cells, so
/// whatever stands between it and CONTEXT is a share. The odd cell that does not divide evenly goes to the
/// leftmost columns, so a column further left (PROJECT) has that share or one cell more.
fn share(line: &[ratatui::buffer::Cell]) -> usize {
    col_of(line, "CONTEXT").unwrap() - col_of(line, "STATUS").unwrap() - 11
}

/// The table's lines (below the column titles) with cells on `color`: the rows carrying that project's pill.
fn pill_rows(lines: &[Vec<ratatui::buffer::Cell>], color: ratatui::style::Color) -> Vec<usize> {
    let h = header_row(lines);
    lines.iter().enumerate().skip(h + 1).filter(|(_, l)| l.iter().any(|c| c.bg == color)).map(|(i, _)| i).collect()
}

/// The first line of a session's row: the one above its description (the prompt, in these tests).
fn row_of(lines: &[Vec<ratatui::buffer::Cell>], prompt: &str) -> usize {
    lines.iter().position(|l| text_of(l).contains(prompt)).expect("the row") - 1
}

#[test]
fn a_session_in_a_project_shows_the_project_as_a_pill_in_its_own_column() {
    let w = four("p-column");
    let mut app = w.app();
    let a = make_project(&mut app, "Web shop");
    put_in(&mut app, "alpha-1", &a);
    app.notice = None;
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    let shown = draw(&mut term, &app);
    let t = app.theme.clone();
    let lines = cells(&term);
    let h = header_row(&lines);
    // PROJECT comes right after NAME, before DIR.
    let (name_x, proj_x, dir_x) = (col_of(&lines[h], "NAME").unwrap(), col_of(&lines[h], "PROJECT").unwrap(), col_of(&lines[h], "DIR").unwrap());
    assert!(name_x < proj_x && proj_x < dir_x, "{}", text_of(&lines[h]));
    // As wide as the name needs, pill and gap included (and its share of the screen's spare width), never
    // narrower than its title and a space.
    let want = "Web shop".len() + 3 + share(&lines[h]);
    assert!([want, want + 1].contains(&(dir_x - proj_x)), "{}", text_of(&lines[h]));
    // The pill: the name on the project's color, the letters in the badge's text color, padded square (no round glyphs here).
    let color = projects::slot_color(&t, &app.projects.get(&a).unwrap().color);
    assert_ne!(color, t.bg);
    let rows = pill_rows(&lines, color);
    assert_eq!(rows.len(), 1, "only the one session in the project: {shown}");
    let row = &lines[rows[0]];
    assert_eq!(col_of(row, " Web shop ").unwrap(), proj_x, "{}", text_of(row));
    for c in &row[proj_x..proj_x + 10] {
        assert_eq!((c.bg, c.fg), (color, projects::badge_text(&t)));
    }
    assert_ne!(row[proj_x + 10].bg, color, "then a gap on the row's own color");
    // The badge letters are not in front of the name any more.
    assert!(col_of(row, " WS ").is_none(), "{}", text_of(row));
    assert!(row[..proj_x].iter().all(|c| c.bg != color));
    assert_eq!(rows[0], row_of(&lines, "alpha talks"));
    // Rows not in a project leave the column blank.
    let other = row_of(&lines, "delta talks");
    assert!(lines[other][proj_x..dir_x].iter().all(|c| c.symbol() == " "));

    // Color and badge can be changed: the pill follows the color; the name stays the name.
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
    draw(&mut term, &app);
    let lines = cells(&term);
    let rows = pill_rows(&lines, projects::slot_color(&t, &p.color));
    assert_eq!(rows.len(), 1);
    assert_eq!(col_of(&lines[rows[0]], " Web shop "), Some(proj_x));
}

#[test]
fn where_the_terminal_has_the_glyphs_the_project_pill_is_round() {
    let w = four("p-round");
    let mut app = w.app();
    let a = make_project(&mut app, "Web shop");
    put_in(&mut app, "alpha-1", &a);
    app.rounded = true;
    let t = app.theme.clone();
    let color = projects::slot_color(&t, &app.projects.get(&a).unwrap().color);
    for selected in [false, true] {
        if selected {
            select(&mut app, "alpha-1");
        } else {
            select(&mut app, "delta-4");
        }
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        draw(&mut term, &app);
        let lines = cells(&term);
        let proj_x = col_of(&lines[header_row(&lines)], "PROJECT").unwrap();
        let row = &lines[pill_rows(&lines, color)[0]];
        let row_bg = if selected { t.surface } else { t.bg };
        // The same pill as the counts: caps in the pill's color on the row's own background, the name between.
        assert_eq!(row[proj_x].symbol(), "\u{e0b6}", "{}", text_of(row));
        assert_eq!((row[proj_x].fg, row[proj_x].bg), (color, row_bg));
        assert_eq!(col_of(row, "Web shop"), Some(proj_x + 1));
        assert_eq!((row[proj_x + 1].bg, row[proj_x + 1].fg), (color, projects::badge_text(&t)));
        assert_eq!(row[proj_x + 9].symbol(), "\u{e0b4}");
        assert_eq!((row[proj_x + 9].fg, row[proj_x + 9].bg), (color, row_bg));
        assert_eq!(row[proj_x + 10].bg, row_bg);
    }
}

#[test]
fn a_long_project_name_is_cut_and_below_90_columns_the_pill_shows_the_badge() {
    let w = four("p-long");
    let mut app = w.app();
    let a = make_project(&mut app, "A project with a rather long name");
    put_in(&mut app, "alpha-1", &a);
    let b = make_project(&mut app, "Tiny");
    put_in(&mut app, "beta-2", &b);
    key(&mut app, KeyCode::Char('1'));
    app.notice = None;
    let t = app.theme.clone();
    let color = projects::slot_color(&t, &app.projects.get(&a).unwrap().color);
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    draw(&mut term, &app);
    let lines = cells(&term);
    let h = header_row(&lines);
    let (proj_x, dir_x) = (col_of(&lines[h], "PROJECT").unwrap(), col_of(&lines[h], "DIR").unwrap());
    // At most 18 columns of text: the pill holds 15 characters, the last one `…` (plus the column's share
    // of the screen's spare width, which is clear air after the pill).
    let want = 18 + share(&lines[h]);
    assert!([want, want + 1].contains(&(dir_x - proj_x)), "{}", text_of(&lines[h]));
    let row = &lines[pill_rows(&lines, color)[0]];
    assert_eq!(col_of(row, " A project with… "), Some(proj_x), "{}", text_of(row));
    // The short name sits in the same column.
    let tiny = projects::slot_color(&t, &app.projects.get(&b).unwrap().color);
    assert_ne!(tiny, color, "each project its own color");
    assert_eq!(col_of(&lines[pill_rows(&lines, tiny)[0]], " Tiny "), Some(proj_x));

    // Below 90 columns (where BRANCH goes): the badge letters, the column as wide as its title needs.
    let mut term = Terminal::new(TestBackend::new(89, 30)).unwrap();
    draw(&mut term, &app);
    let lines = cells(&term);
    let h = header_row(&lines);
    let head = text_of(&lines[h]);
    assert!(!head.contains("BRANCH") && !head.contains("MODEL"), "{head}");
    let (proj_x, dir_x) = (col_of(&lines[h], "PROJECT").unwrap(), col_of(&lines[h], "DIR").unwrap());
    let want = 8 + share(&lines[h]);
    assert!([want, want + 1].contains(&(dir_x - proj_x)), "{}", text_of(&lines[h]));
    let row = &lines[pill_rows(&lines, color)[0]];
    assert_eq!(col_of(row, " AP "), Some(proj_x), "{}", text_of(row));
    assert!(!text_of(row).contains("A project"));
    assert_eq!(col_of(&lines[pill_rows(&lines, tiny)[0]], " TI "), Some(proj_x));
}

#[test]
fn without_a_project_on_the_tab_the_table_is_unchanged() {
    let table = |term: &Terminal<TestBackend>| -> Vec<String> {
        let lines = cells(term);
        lines[header_row(&lines)..].iter().map(|l| text_of(l)).collect()
    };
    for width in [120u16, 80] {
        let w = four(&format!("p-nocol-{width}"));
        fs::create_dir_all(&w.config).unwrap();
        fs::write(w.config.join("archive.json"), r#"{"version":1,"archived":{"delta-4":5}}"#).unwrap();
        let mut app = w.app();
        let mut term = Terminal::new(TestBackend::new(width, 30)).unwrap();
        draw(&mut term, &app);
        let before = table(&term);
        assert!(!before[0].contains("PROJECT"));
        // A project with nobody in it, then one holding only an archived session: the Active tab has no column.
        let a = make_project(&mut app, "Web shop");
        key(&mut app, KeyCode::Char('1'));
        app.notice = None;
        draw(&mut term, &app);
        assert_eq!(table(&term), before, "{width}");
        app.projects.assign("delta-4", Some(&a)).unwrap();
        app.refresh_for_test();
        draw(&mut term, &app);
        assert_eq!(table(&term), before, "{width}: the archived session's project is not on this tab");
        // On the Archived tab it is.
        key(&mut app, KeyCode::Right);
        draw(&mut term, &app);
        assert!(table(&term)[0].contains("PROJECT"), "{}", table(&term)[0]);
    }
}

#[test]
fn with_a_project_column_every_row_still_fits_the_width() {
    let w = four("p-fit");
    let mut app = w.app();
    let a = make_project(&mut app, "A project with a rather long name");
    put_in(&mut app, "alpha-1", &a);
    key(&mut app, KeyCode::Char('1'));
    app.notice = None;
    let t = app.theme.clone();
    let color = projects::slot_color(&t, &app.projects.get(&a).unwrap().color);
    for width in [120u16, 100, 96, 95, 90, 89, 80, 69, 68, 60, 50, 40, 30] {
        let mut term = Terminal::new(TestBackend::new(width, 30)).unwrap();
        draw(&mut term, &app);
        let lines = cells(&term);
        let h = header_row(&lines);
        // The table's right edge: LAST ends where the rows' times end, with a column to spare before the frame.
        let head = text_of(&lines[h]);
        let last_end = col_of(&lines[h], "LAST").map(|x| x + 4);
        let rows: Vec<usize> = (h + 1..lines.len()).filter(|&y| text_of(&lines[y]).contains("talks")).map(|y| y - 1).collect();
        assert_eq!(rows.len(), 4, "{width}: every row drawn");
        for &y in &rows {
            let line = text_of(&lines[y]);
            let end = line.trim_end_matches(['│', ' ']).chars().count();
            if let Some(e) = last_end {
                assert_eq!(end, e, "{width}: the row ends under LAST:\n{head}\n{line}");
            }
            assert!(line.ends_with('│'), "{width}: the frame is not overwritten: {line}");
        }
        let has_col = head.contains("PROJECT");
        assert_eq!(has_col, width >= 69, "{width}: {head}");
        assert_eq!(pill_rows(&lines, color).is_empty(), !has_col, "{width}");
        // The pill switches to the badge letters below 90 real terminal columns.
        if has_col {
            let row = text_of(&lines[pill_rows(&lines, color)[0]]);
            assert_eq!(row.contains(" AP "), width < 90, "{width}: {row}");
            assert_eq!(row.contains(" A project"), width >= 90, "{width}: {row}");
        }
    }
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
