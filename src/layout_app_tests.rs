//! How wide the table's columns come out: NAME and DIR sized to what the rows hold (capped), the fixed
//! columns and their drop rules, and the header lining up with the rows at every terminal width.
//! Every directory is a temp dir (`World`); nothing here starts a process.

use crate::app::{App, AssignOpt, Mode};
use crate::testkit::World;
use crate::ui;
use ratatui::{
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    Terminal,
};

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn type_str(app: &mut App, text: &str) {
    for c in text.chars() {
        key(app, KeyCode::Char(c));
    }
}

/// The screen, one string per line.
fn draw(term: &mut Terminal<TestBackend>, app: &App) -> Vec<String> {
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect()).collect()
}

/// Where `word` starts on a line (in cells).
fn col_of(line: &str, word: &str) -> Option<usize> {
    let cells: Vec<char> = line.chars().collect();
    let w: Vec<char> = word.chars().collect();
    (0..cells.len().saturating_sub(w.len().saturating_sub(1))).find(|&i| cells[i..i + w.len()] == w[..])
}

/// The column titles are always the fourth line inside the frame; the first row's own line follows it.
const TITLES: usize = 4;
const FIRST_ROW: usize = 5;

/// How far the table reaches: the last cell written on the line, the frame's own border not counted.
fn table_end(line: &str) -> usize {
    line.trim_end_matches(['│', ' ']).chars().count()
}

/// Two conversations with short names and short paths.
fn short(name: &str) -> World {
    let w = World::new(name);
    w.transcript("alpha-1", "/srv/alpha-one", "alpha talks about apples", 10);
    w.transcript("beta-2", "/srv/beta", "beta talks about pears", 20);
    w
}

#[test]
fn on_a_wide_screen_name_and_dir_are_only_as_wide_as_the_rows_need() {
    let w = short("l-wide");
    let mut app = w.app();
    app.notice = None;
    for width in [200u16, 341] {
        let mut term = Terminal::new(TestBackend::new(width, 20)).unwrap();
        let lines = draw(&mut term, &app);
        let head = &lines[TITLES];
        let (name_x, dir_x, branch_x) = (col_of(head, "NAME").unwrap(), col_of(head, "DIR").unwrap(), col_of(head, "BRANCH").unwrap());
        // "alpha-one" is the longest name (9) and "/srv/alpha-one" the longest path (14), each plus a gap.
        assert_eq!(dir_x - name_x, 9 + 2, "{width}: NAME is as wide as its longest value\n{head}");
        assert_eq!(branch_x - dir_x, 14 + 2, "{width}: DIR is as wide as its longest value\n{head}");
        // Nothing is cut, and the table stops well short of the right edge: the spare cells stay empty.
        let row = &lines[FIRST_ROW];
        assert!(row.contains("alpha-one") && row.contains("/srv/alpha-one"), "{row}");
        let end = table_end(head);
        assert_eq!(end, table_end(row), "{width}: the header and the row end together\n{head}\n{row}");
        assert!(end + 20 < width as usize, "{width}: the table ends at {end}, leaving the right of the screen empty");
    }
}

#[test]
fn a_name_or_a_path_longer_than_the_cap_is_cut_and_a_path_keeps_its_end() {
    let w = World::new("l-cap");
    let long_name = "n".repeat(70);
    w.transcript("alpha-1", &format!("/tmp/{long_name}"), "alpha talks about apples", 10);
    w.transcript("beta-2", &format!("/srv/{}/repos/platoon", "x".repeat(70)), "beta talks about pears", 20);
    let mut app = w.app();
    app.notice = None;
    let mut term = Terminal::new(TestBackend::new(341, 20)).unwrap();
    let lines = draw(&mut term, &app);
    let head = &lines[TITLES];
    let (name_x, dir_x, branch_x) = (col_of(head, "NAME").unwrap(), col_of(head, "DIR").unwrap(), col_of(head, "BRANCH").unwrap());
    assert_eq!((dir_x - name_x, branch_x - dir_x), (60, 60), "both columns stop at the cap\n{head}");
    // The long name: cut with `…` at the end, inside its column.
    let name_row = lines.iter().find(|l| l.contains("nnnn")).expect("the long-named row");
    let shown_name: String = name_row.chars().skip(name_x).take(60).collect();
    assert_eq!(shown_name, format!("{}…", "n".repeat(59)), "the name is cut with an ellipsis\n{name_row}");
    assert!(!name_row.contains(&long_name), "the whole 70-character name does not fit");
    // The long path: cut at the front, so the folder itself still reads.
    let dir_row = lines.iter().find(|l| l.contains("xxxx")).expect("the long-path row");
    assert_eq!(dir_row.chars().nth(dir_x), Some('…'), "the path is cut at its start\n{dir_row}");
    let shown: String = dir_row.chars().skip(dir_x).take(60).collect();
    assert!(shown.ends_with("/repos/platoon"), "the end of the path is kept: {shown:?}");
    assert!(!shown.contains("/srv/"), "its start is gone: {shown:?}");
}

#[test]
fn the_header_and_the_rows_are_the_same_width_at_every_terminal_width() {
    let w = short("l-every");
    let mut app = w.app();
    app.notice = None;
    for width in 0..=240u16 {
        let mut term = Terminal::new(TestBackend::new(width.max(1), 20)).unwrap();
        let lines = draw(&mut term, &app);
        if lines.len() <= FIRST_ROW {
            continue;
        }
        let (head, row) = (&lines[TITLES], &lines[FIRST_ROW]);
        // Below about 12 columns there is no room for a table at all and only the frame is drawn.
        if head.contains("LAST") {
            assert_eq!(table_end(head), table_end(row), "{width}: the header and the row end together\n{head}\n{row}");
        }
        for line in &lines {
            assert!(line.chars().count() == width.max(1) as usize, "{width}: nothing is written past the terminal");
            if width >= 2 {
                assert!(line.ends_with('│') || line.ends_with('╮') || line.ends_with('╯') || line.ends_with('─'), "{width}: the frame is intact: {line}");
            }
        }
    }
}

#[test]
fn branch_and_model_still_drop_at_the_widths_they_always_did() {
    let w = short("l-drop");
    let mut app = w.app();
    app.notice = None;
    // The rules go by the table's width, which is the terminal less the frame and the margins (6 cells).
    for width in [80u16, 90, 95, 100, 120] {
        let mut term = Terminal::new(TestBackend::new(width, 20)).unwrap();
        let lines = draw(&mut term, &app);
        let head = &lines[TITLES];
        assert_eq!(head.contains("BRANCH"), width as usize - 6 >= 90, "{width}: {head}");
        assert_eq!(head.contains("MODEL"), width as usize - 6 >= 100, "{width}: {head}");
        assert!(head.contains("STATUS") && head.contains("CONTEXT") && head.contains("LAST"), "{width}: {head}");
        assert_eq!(table_end(head), table_end(&lines[FIRST_ROW]), "{width}: the header and the row end together\n{head}");
    }
}

/// Put `session` in `project` with the `m` key, from the Sessions tab.
fn put_in(app: &mut App, session: &str, project: &str) {
    key(app, KeyCode::Char('1'));
    app.selected = app.visible().iter().position(|&i| app.sessions[i].id == session).expect("listed");
    key(app, KeyCode::Char('m'));
    app.assign.selected = app.assign.options.iter().position(|o| *o == AssignOpt::Project(project.to_string())).expect("offered");
    key(app, KeyCode::Enter);
}

fn make_project(app: &mut App, name: &str) -> String {
    key(app, KeyCode::Char('2'));
    key(app, KeyCode::Char('n'));
    type_str(app, name);
    key(app, KeyCode::Enter);
    app.projects.all().iter().find(|p| p.name == name).expect("made").id.clone()
}

#[test]
fn the_project_column_keeps_its_width_while_the_quick_filter_is_typed() {
    let w = short("l-filter");
    let mut app = w.app();
    let a = make_project(&mut app, "Web shop");
    put_in(&mut app, "alpha-1", &a);
    key(&mut app, KeyCode::Char('1'));
    app.notice = None;
    let mut term = Terminal::new(TestBackend::new(200, 20)).unwrap();
    let head = draw(&mut term, &app)[TITLES].clone();
    let (proj_x, dir_x) = (col_of(&head, "PROJECT").unwrap(), col_of(&head, "DIR").unwrap());
    assert_eq!(dir_x - proj_x, "Web shop".len() + 3);

    // Narrow the list down to the row that is in no project: the column is sized from every row on the tab,
    // so it neither shrinks nor goes while the filter is typed.
    key(&mut app, KeyCode::Char('/'));
    type_str(&mut app, "beta");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List) && app.visible().len() == 1 && app.filter == "beta");
    let head = draw(&mut term, &app)[TITLES].clone();
    let narrowed = col_of(&head, "PROJECT").expect("the column is still there");
    assert_eq!(col_of(&head, "DIR").unwrap() - narrowed, "Web shop".len() + 3, "and still the same width\n{head}");
}

#[test]
fn the_project_pill_shows_letters_below_ninety_terminal_columns_and_the_name_at_ninety() {
    let w = short("l-pill");
    let mut app = w.app();
    let a = make_project(&mut app, "Web shop");
    put_in(&mut app, "alpha-1", &a);
    key(&mut app, KeyCode::Char('1'));
    app.notice = None;
    for (width, name, letters) in [(89u16, false, true), (90, true, false)] {
        let mut term = Terminal::new(TestBackend::new(width, 20)).unwrap();
        let lines = draw(&mut term, &app);
        let row = lines.iter().find(|l| l.contains("alpha-one")).expect("the row").clone();
        assert_eq!(row.contains(" Web shop "), name, "{width}: {row}");
        assert_eq!(row.contains(" WS "), letters, "{width}: {row}");
    }
}

#[test]
fn the_projects_list_sizes_its_name_column_to_the_projects_on_show() {
    let w = short("l-plist");
    let mut app = w.app();
    make_project(&mut app, "Web shop");
    make_project(&mut app, "Tiny");
    app.notice = None;
    key(&mut app, KeyCode::Char('2'));
    let mut term = Terminal::new(TestBackend::new(200, 20)).unwrap();
    let lines = draw(&mut term, &app);
    let head = &lines[TITLES];
    let (proj_x, sess_x) = (col_of(head, "PROJECT").unwrap(), col_of(head, "SESSIONS").unwrap());
    // The badge (` WS `, four cells), a space and the longest name, plus the gap.
    assert_eq!(sess_x - proj_x, 4 + 1 + "Web shop".len() + 2, "{head}");
    let row = lines.iter().find(|l| l.contains("Web shop")).expect("the row");
    assert!(table_end(row) + 20 < 200, "the list ends well short of the right edge: {row}");

    // A name past the cap stops there and is cut.
    let long = "L".repeat(crate::projects::NAME_MAX);
    make_project(&mut app, &long);
    key(&mut app, KeyCode::Char('2'));
    app.notice = None;
    let lines = draw(&mut term, &app);
    let head = &lines[TITLES];
    let (proj_x, sess_x) = (col_of(head, "PROJECT").unwrap(), col_of(head, "SESSIONS").unwrap());
    assert_eq!(sess_x - proj_x, 60, "{head}");
    let row = lines.iter().find(|l| l.contains("LLLL")).expect("the long row");
    assert!(row.contains('…'), "the long name is cut: {row}");
    assert!(!row.contains(&long), "{row}");
}

