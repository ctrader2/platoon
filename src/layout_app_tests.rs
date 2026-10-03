//! How wide the table's columns come out: NAME and DIR sized to what the rows hold (capped), the fixed
//! columns and their drop rules, and the header lining up with the rows at every terminal width.
//! Every directory is a temp dir (`World`); nothing here starts a process.

use crate::app::{App, AssignOpt, Mode};
use crate::testkit::World;
use crate::ui;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
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

/// The column titles are the third line (the header, then the rule with the Active/Archived switch at its right, as
/// it is at every width these tests use); the first row's own line follows it.
const TITLES: usize = 2;
const FIRST_ROW: usize = 3;

/// How far the table reaches: the last cell written on the line.
fn table_end(line: &str) -> usize {
    line.trim_end_matches(' ').chars().count()
}

/// Two conversations with short names and short paths.
fn short(name: &str) -> World {
    let w = World::new(name);
    w.transcript("alpha-1", "/srv/alpha-one", "alpha talks about apples", 10);
    w.transcript("beta-2", "/srv/beta", "beta talks about pears", 20);
    w
}

/// Where the table's columns begin on a header line, in row order, skipping the ones not drawn.
fn column_starts(head: &str) -> Vec<usize> {
    ["NAME", "PROJECT", "DIR", "BRANCH", "MODEL", "STATUS", "CONTEXT", "LAST"].iter().filter_map(|t| col_of(head, t)).collect()
}

#[test]
fn on_a_wide_screen_the_columns_share_the_spare_width_and_the_row_fills_it() {
    let w = short("l-wide");
    let mut app = w.app();
    app.notice = None;
    for width in [200u16, 341] {
        let mut term = Terminal::new(TestBackend::new(width, 20)).unwrap();
        let lines = draw(&mut term, &app);
        let head = &lines[TITLES];
        let row = &lines[FIRST_ROW];
        // The row reaches the right edge: the one-cell side margin and the table's own trailing cell are all
        // that is left of the line.
        let end = table_end(head);
        assert_eq!(end, table_end(row), "{width}: the header and the row end together\n{head}\n{row}");
        assert_eq!(end, width as usize - 2, "{width}: the table reaches the right edge\n{head}");
        // Every column keeps the width its content needs, plus an equal share of what is over. "alpha-one"
        // is the longest name (9) and "/srv/alpha-one" the longest path (14), each plus a two-cell gap.
        let starts = column_starts(head);
        assert_eq!(starts.len(), 7, "{width}: NAME, DIR, BRANCH, MODEL, STATUS, CONTEXT and LAST are drawn\n{head}");
        // NAME, DIR, BRANCH, MODEL, STATUS: the distance to the next column is its own width plus its share.
        let wanted = [9 + 2, 14 + 2, 16, 12, 11];
        let shares: Vec<usize> = starts.windows(2).zip(wanted).map(|(x, want)| x[1] - x[0] - want).collect();
        let (lo, hi) = (shares.iter().min().unwrap(), shares.iter().max().unwrap());
        assert!(hi - lo <= 1 && *lo > 0, "{width}: the columns share what is over: {shares:?}\n{head}");
        // The context bar itself keeps its size; its column's share is the air after the percentage.
        let pct = col_of(row, "%").expect("the bar's percentage");
        assert_eq!(pct + 1, starts[5] + BAR + 5, "{width}: the bar is not stretched\n{head}\n{row}");
        // Nothing is cut: the text sits at the left of its column, the share is clear air after it.
        assert!(row.contains("alpha-one") && row.contains("/srv/alpha-one"), "{row}");
    }
}

/// The context bar keeps its own width: how full it looks is what it means.
const BAR: usize = 10;

#[test]
fn a_narrow_screen_still_shares_the_room_between_name_and_dir_as_it_did() {
    let w = World::new("l-narrow");
    w.transcript("alpha-1", &format!("/srv/{}", "a".repeat(60)), "alpha talks about apples", 10);
    w.transcript("beta-2", &format!("/srv/{}", "b".repeat(60)), "beta talks about pears", 20);
    let mut app = w.app();
    app.notice = None;
    for width in [80u16, 100, 120] {
        let mut term = Terminal::new(TestBackend::new(width, 20)).unwrap();
        let lines = draw(&mut term, &app);
        let head = &lines[TITLES];
        let table = width as usize - 2;
        let (branch, model) = (if table >= 90 { 16 } else { 0 }, if table >= 100 { 12 } else { 0 });
        let flex = table - (3 + BAR + 5 + 6 + 11 + branch + model);
        let starts = column_starts(head);
        // Two fifths of what there is to NAME, the rest to DIR, exactly as before: nothing is over to share.
        assert_eq!(starts[1] - starts[0], flex * 2 / 5, "{width}: NAME's old share\n{head}");
        assert_eq!(starts[2] - starts[1], flex - flex * 2 / 5, "{width}: DIR's old share\n{head}");
        assert_eq!(table_end(head), table_end(&lines[FIRST_ROW]), "{width}: header and row end together\n{head}");
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
    // The cap still holds: the columns are 60 cells of text each; what is past that is the equal share of
    // the spare width, which is the same for both.
    let share = (dir_x - name_x) - 60;
    assert!(share > 0, "and take a share of the spare width\n{head}");
    // (The odd cell that does not divide evenly goes to the leftmost columns, hence "or one less".)
    assert!((branch_x - dir_x) + 1 >= 60 + share && branch_x - dir_x <= 60 + share, "both columns stop at the cap\n{head}");
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
        // Below the smallest size platoon draws in, there is only the message saying so.
        if width < crate::app::MIN_WIDTH {
            if width >= 30 {
                assert!(lines.join(" ").contains("platoon needs at least 60×14"), "{width}: {lines:#?}");
            }
            assert!(!head.contains("LAST"), "{width}: no table: {head}");
            continue;
        }
        assert!(head.contains("LAST"), "{width}: {head}");
        assert_eq!(table_end(head), table_end(row), "{width}: the header and the row end together\n{head}\n{row}");
        for line in &lines {
            assert!(line.chars().count() == width.max(1) as usize, "{width}: nothing is written past the terminal");
            // No frame: a column of air at each side, so nothing touches the terminal's edges.
            assert!(line.starts_with(' ') && line.ends_with(' '), "{width}: the side margins are clear: {line:?}");
        }
    }
}

#[test]
fn branch_and_model_still_drop_at_the_widths_they_always_did() {
    let w = short("l-drop");
    let mut app = w.app();
    app.notice = None;
    // The rules go by the table's width, which is the terminal less the side margins (2 cells).
    for width in [80u16, 90, 91, 92, 95, 100, 101, 102, 120] {
        let mut term = Terminal::new(TestBackend::new(width, 20)).unwrap();
        let lines = draw(&mut term, &app);
        let head = &lines[TITLES];
        assert_eq!(head.contains("BRANCH"), width as usize - 2 >= 90, "{width}: {head}");
        assert_eq!(head.contains("MODEL"), width as usize - 2 >= 100, "{width}: {head}");
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
    // Every column takes the same share of the wide screen's spare width (the odd cell to the leftmost),
    // so the pill's own width is the distance to DIR less that share: STATUS (11 cells) shows what a share
    // is, and PROJECT, being further left, has it or one cell more.
    let pill = |head: &str| {
        let share = (col_of(head, "CONTEXT").unwrap() - col_of(head, "STATUS").unwrap()) - 11;
        let col = col_of(head, "DIR").unwrap() - col_of(head, "PROJECT").unwrap();
        assert!(col >= share, "{head}");
        col - share
    };
    let want = "Web shop".len() + 3;
    assert!([want, want + 1].contains(&pill(&head)), "{head}");

    // Narrow the list down to the row that is in no project: the column is sized from every row on the tab,
    // so it neither shrinks nor goes while the filter is typed.
    key(&mut app, KeyCode::Char('/'));
    type_str(&mut app, "beta");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List) && app.visible().len() == 1 && app.filter == "beta");
    let head = draw(&mut term, &app)[TITLES].clone();
    assert!(col_of(&head, "PROJECT").is_some(), "the column is still there\n{head}");
    assert!([want, want + 1].contains(&pill(&head)), "and still the same width\n{head}");
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
    // The name column is the badge (` WS `, four cells), a space and the longest name, plus the gap - and
    // then its share of the wide screen's spare width, the share SESSIONS (24 cells) gets or one cell more
    // (the odd cell goes to the leftmost columns).
    for width in [200u16, 341] {
        let mut term = Terminal::new(TestBackend::new(width, 20)).unwrap();
        let lines = draw(&mut term, &app);
        let head = &lines[TITLES];
        let (proj_x, sess_x, tickets_x) =
            (col_of(head, "PROJECT").unwrap(), col_of(head, "SESSIONS").unwrap(), col_of(head, "TICKETS").unwrap());
        let share = (tickets_x - sess_x) - 24;
        assert!(share > 0, "{width}: {head}");
        let want = 4 + 1 + "Web shop".len() + 2 + share;
        assert!([want, want + 1].contains(&(sess_x - proj_x)), "{width}: {head}");
        // The rows fill the width: they end where the header does, at the right edge.
        let row = lines.iter().find(|l| l.contains("Web shop")).expect("the row");
        assert_eq!(table_end(row), table_end(head), "{width}: the row and the header end together: {row}");
        assert_eq!(table_end(head), width as usize - 2, "{width}: the list reaches the right edge: {head}");
    }

    // A name past the cap stops there and is cut.
    let long = "L".repeat(crate::projects::NAME_MAX);
    make_project(&mut app, &long);
    key(&mut app, KeyCode::Char('2'));
    app.notice = None;
    let lines = draw(&mut term, &app);
    let head = &lines[TITLES];
    let (proj_x, sess_x, tickets_x) =
        (col_of(head, "PROJECT").unwrap(), col_of(head, "SESSIONS").unwrap(), col_of(head, "TICKETS").unwrap());
    let share = (tickets_x - sess_x) - 24;
    assert!([60 + share, 61 + share].contains(&(sess_x - proj_x)), "{head}");
    let row = lines.iter().find(|l| l.contains("LLLL")).expect("the long row");
    assert!(row.contains('…'), "the long name is cut: {row}");
    assert!(!row.contains(&long), "{row}");
}

/// A conversation in `cwd` with folders added beside it, as Claude Code records them (an environment snapshot).
fn with_added(w: &World, id: &str, cwd: &str, added: &[&str], prompt: &str, mins_ago: u64) {
    let at = crate::testkit::now_ms() - mins_ago * 60_000;
    let (t0, t1) = (crate::testkit::iso(at - 5000), crate::testkit::iso(at));
    let lines = [
        serde_json::json!({"type":"mode","mode":"normal"}),
        serde_json::json!({"type":"user","cwd":cwd,"gitBranch":"main","timestamp":t0,"message":{"role":"user","content":prompt}}),
        serde_json::json!({"type":"attachment","isSidechain":false,"cwd":cwd,"timestamp":t0,
            "attachment":{"type":"environment","snapshot":{"workingDirectory":cwd,"additionalWorkingDirectories":added},"changes":null}}),
        serde_json::json!({"type":"assistant","cwd":cwd,"gitBranch":"main","timestamp":t1,"message":{"model":"claude-sonnet-5","usage":{"input_tokens":50_000}}}),
    ];
    w.transcript_lines(id, &lines.iter().map(|v| v.to_string()).collect::<Vec<_>>());
}

#[test]
fn added_folders_follow_the_folder_in_the_dir_column_and_are_counted_when_they_dont_fit() {
    let w = World::new("l-added");
    // The paths are only text in the transcripts; the app writes them with `~` for its own home folder.
    let home = std::env::var("HOME").unwrap_or_default();
    assert!(home.len() > 1, "the test needs a home folder to write as ~");
    with_added(&w, "one-added", &format!("{home}/repos/platoon"), &["/"], "talks about apples", 10);
    // Longer, all told, than DIR's cap: the column stays at the cap and the added folder is counted.
    let long = format!("{home}/an/added/folder/with/a/rather/long/path/that/goes/on/for/a/while");
    with_added(&w, "long-added", &format!("{home}/repos/shop"), &[&long], "talks about pears", 20);
    w.transcript("none-added", &format!("{home}/repos/plain"), "talks about plums", 30);
    let mut app = w.app();
    assert_eq!(app.home, home);
    app.notice = None;

    let row_of =
        |lines: &[String], word: &str| lines.iter().find(|l| l.contains(word)).cloned().unwrap_or_else(|| panic!("{word}: {lines:#?}"));
    let mut term = Terminal::new(TestBackend::new(200, 20)).unwrap();
    let lines = draw(&mut term, &app);
    assert!(row_of(&lines, "~/repos/platoon").contains("~/repos/platoon, /  "), "in full where there is room\n{lines:#?}");
    let shop = row_of(&lines, "~/repos/shop");
    assert!(shop.contains("~/repos/shop, +1  "), "the main folder and a count\n{shop}");
    assert!(!shop.contains("~/an/"), "{shop}");
    assert!(row_of(&lines, "~/repos/plain").contains("~/repos/plain  "), "a conversation without added folders is as before");

    // Narrow: the main folder is cut at the front and the rest counted.
    let mut term = Terminal::new(TestBackend::new(60, 20)).unwrap();
    let lines = draw(&mut term, &app);
    assert!(!lines.iter().any(|l| l.contains("platoon, /")), "no room for the whole list at 60 wide\n{lines:#?}");
    assert_eq!(lines.iter().filter(|l| l.contains(", +1")).count(), 2, "both rows with added folders count them\n{lines:#?}");

    // The quick filter finds a conversation by an added folder.
    key(&mut app, KeyCode::Char('/'));
    type_str(&mut app, "rather/long");
    assert_eq!(app.visible().iter().map(|&i| app.sessions[i].id.as_str()).collect::<Vec<_>>(), ["long-added"]);
}
