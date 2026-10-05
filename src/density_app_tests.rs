//! Density and narrow widths, drawn the way a terminal shows them (TestBackend) at 80x24, 60x24 and 60x14: the
//! Sessions list's two lines a conversation and its selection, the gaps between columns and the order they go in,
//! the CONTEXT column left blank when nothing is known, the Briefs tab as one pane on a narrow terminal, the settings'
//! tab strip, `S` handing a ticket to a squad, j/k and PageUp/PageDown in the lists, and which hints the Sessions
//! footer keeps at 120 and 80 columns. Every directory is a temp dir
//! (`World`); nothing here starts a process.

use crate::app::{App, Mode, Pane, SqFocus, SqView};
use crate::testkit::World;
use crate::tickets::{NewTicket, Tickets};
use crate::ui;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    style::Color,
};

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn ch(app: &mut App, c: char) {
    key(app, KeyCode::Char(c));
}

/// Draw at `w` x `h`: the screen as lines, and the terminal (for the cells' colors).
fn shown(app: &App, w: u16, h: u16) -> (Vec<String>, Terminal<TestBackend>) {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer();
    let lines = (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect()).collect();
    (lines, term)
}

fn flat(lines: &[String]) -> String {
    lines.join("\n").replace(['│', '╭', '╮', '╰', '╯', '─'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn row_of(lines: &[String], needle: &str) -> usize {
    lines.iter().position(|l| l.contains(needle)).unwrap_or_else(|| panic!("{needle:?} not on screen:\n{}", lines.join("\n")))
}

fn col_of(line: &str, needle: &str) -> usize {
    let at = line.find(needle).unwrap_or_else(|| panic!("{needle:?} not in {line:?}"));
    line[..at].chars().count()
}

fn bg(term: &Terminal<TestBackend>, x: usize, y: usize) -> Color {
    term.backend().buffer()[(x as u16, y as u16)].bg
}

/// Ten conversations, newest first: `conv-0` ... `conv-9`, each with a prompt line of its own.
fn ten(name: &str) -> World {
    let w = World::new(name);
    for i in 0..10u64 {
        w.transcript(&format!("conv-{i}"), &format!("/srv/project-{i}"), &format!("prompt number {i}"), 10 + i);
    }
    w
}

#[test]
fn a_conversation_is_two_lines_with_no_spacer_and_ten_fit_at_80_by_24() {
    let w = ten("d-rows");
    let mut app = w.app();
    app.notice = None;
    for (width, height, at_least) in [(80u16, 24u16, 10usize), (60, 24, 10), (60, 14, 5)] {
        let (lines, _) = shown(&app, width, height);
        println!("Sessions list at {width}x{height}:\n{}\n", lines.join("\n"));
        // Each name line is followed straight away by its prompt line, and that by the next name line.
        let names: Vec<usize> = (0..10).filter_map(|i| lines.iter().position(|l| l.contains(&format!("project-{i} ")))).collect();
        assert!(names.len() >= at_least, "{width}x{height}: {} rows fit:\n{}", names.len(), lines.join("\n"));
        for (k, &y) in names.iter().enumerate() {
            assert!(
                lines[y + 1].contains(&format!("prompt number {k}")),
                "{width}x{height}: the prompt line under its name\n{}",
                lines.join("\n")
            );
            if k + 1 < names.len() {
                assert_eq!(names[k + 1], y + 2, "{width}x{height}: no blank line between rows\n{}", lines.join("\n"));
            }
        }
    }
}

#[test]
fn the_selection_and_the_hover_cover_both_lines_of_a_row_and_nothing_else() {
    let w = ten("d-select");
    let mut app = w.app();
    app.notice = None;
    ch(&mut app, 'j'); // conv-1 selected
    let t = app.effective_theme().clone();
    let (lines, term) = shown(&app, 80, 24);
    let name = row_of(&lines, "project-1 ");
    let x = col_of(&lines[name], "project-1");
    for y in [name, name + 1] {
        assert_eq!(bg(&term, x, y), t.surface, "line {y} of the selected row is highlighted\n{}", lines.join("\n"));
        assert!(lines[y].contains('▎'), "the marker on both lines: {}", lines[y]);
    }
    for y in [name - 1, name + 2] {
        assert_eq!(bg(&term, x, y), t.bg, "line {y}, a neighbour's, is not\n{}", lines.join("\n"));
        assert!(!lines[y].contains('▎'), "{}", lines[y]);
    }
    // The mouse over the second line of another row lights both of that row's lines, as one row.
    let other = row_of(&lines, "project-3 ");
    app.mouse = (x as u16, other as u16 + 1);
    let (lines, term) = shown(&app, 80, 24);
    for y in [other, other + 1] {
        assert_eq!(bg(&term, x, y), t.surface, "hovered line {y}\n{}", lines.join("\n"));
    }
    assert_eq!(bg(&term, x, other + 2), t.bg);
}

#[test]
fn at_80_and_60_columns_the_columns_keep_apart_and_the_bar_goes_before_dir() {
    let w = ten("d-cols");
    let mut app = w.app();
    app.notice = None;
    let (lines, _) = shown(&app, 80, 24);
    let head = &lines[row_of(&lines, "STATUS")];
    for t in ["NAME", "DIR", "STATUS", "CONTEXT", "LAST"] {
        assert!(head.contains(t), "80: {t}: {head}");
    }
    assert!(!head.contains("BRANCH") && !head.contains("MODEL"), "80: {head}");
    let (lines, _) = shown(&app, 60, 24);
    let head = &lines[row_of(&lines, "STATUS")];
    for t in ["NAME", "DIR", "STATUS", "LAST"] {
        assert!(head.contains(t), "60: {t}: {head}");
    }
    assert!(!head.contains("CONTEXT"), "60: the bar goes before DIR and NAME: {head}");
    // A blank cell before every title, and before every value under it.
    let row = &lines[row_of(&lines, "project-0 ")];
    for t in ["DIR", "STATUS", "LAST"] {
        let x = col_of(head, t);
        assert_eq!(head.chars().nth(x - 1), Some(' '), "{head}");
        assert_eq!(row.chars().nth(x - 1), Some(' '), "{head}\n{row}");
    }
}

#[test]
fn the_context_column_is_blank_for_a_conversation_nothing_is_known_about() {
    let w = World::new("d-context");
    w.transcript("known-1", "/srv/known", "a conversation with usage", 10);
    // Only a prompt so far: no model, no usage.
    let at = crate::testkit::now_ms() - 20 * 60_000;
    w.transcript_lines("bare-2", &[serde_json::json!({"type":"user","cwd":"/srv/bare","timestamp":crate::testkit::iso(at),"message":{"role":"user","content":"nothing known yet"}}).to_string()]);
    let mut app = w.app();
    app.notice = None;
    let (lines, _) = shown(&app, 80, 24);
    let known = &lines[row_of(&lines, "known ")];
    let bare = &lines[row_of(&lines, "bare ")];
    assert!(known.contains('%') && known.contains('━'), "a bar where there is data: {known}");
    assert!(!bare.contains('%') && !bare.contains('━'), "blank, not an empty bar at 0%: {bare}");
    // Blank, but the row still lines up: LAST ends where it does on the other row.
    assert_eq!(bare.trim_end_matches(['│', ' ']).chars().count(), known.trim_end_matches(['│', ' ']).chars().count());
}

#[test]
fn the_squads_tab_is_one_pane_at_60_columns_and_two_at_100() {
    let w = World::new("d-squads");
    let mut app = w.app();
    ch(&mut app, '4');
    // 100 columns: the list and the details side by side.
    let (lines, _) = shown(&app, 100, 24);
    assert!(lines.iter().any(|l| l.contains("╮╭")), "two panes at 100:\n{}", lines.join("\n"));
    assert!(flat(&lines).contains("What it does"), "{}", lines.join("\n"));
    // From an 80-column terminal, two panes; below it, one.
    let (lines, _) = shown(&app, 80, 24);
    println!("Briefs tab at 80x24:\n{}\n", lines.join("\n"));
    assert!(lines.iter().any(|l| l.contains("╮╭")), "two panes at 80:\n{}", lines.join("\n"));
    let (lines, _) = shown(&app, 79, 24);
    assert!(!lines.iter().any(|l| l.contains("╮╭")), "one pane at 79:\n{}", lines.join("\n"));
    // 60 columns: only the list, every squad on it, and no "Built in" under each.
    let (lines, _) = shown(&app, 60, 24);
    println!("Briefs tab at 60x24, the list:\n{}\n", lines.join("\n"));
    let f = flat(&lines);
    assert!(!lines.iter().any(|l| l.contains("╮╭")), "one pane at 60:\n{}", lines.join("\n"));
    assert!(f.contains("▎ Coding 4 agents Research 4 agents Docs 3 agents Summarize 1 agent"), "{f}");
    assert!(!f.contains("Built in") && !f.contains("Name Coding") && !f.contains("next pane"), "{f}");
    // Down to Research, enter: its details take the whole width, with room for what they say.
    ch(&mut app, 'j');
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.sqt.focus, SqFocus::Detail);
    let (lines, _) = shown(&app, 60, 24);
    println!("Briefs tab at 60x24, Research's details:\n{}\n", lines.join("\n"));
    let f = flat(&lines);
    assert!(!f.contains("[Briefs]"), "the list is not drawn beside them: {f}");
    assert!(f.contains("Built in") && f.contains("What it does Finds things out"), "{f}");
    assert!(f.contains("esc back to the list"), "{f}");
    // Esc: the list again, on the same squad.
    key(&mut app, KeyCode::Esc);
    assert_eq!((app.sqt.focus, app.sqt.selected), (SqFocus::List, 1));
    let (lines, _) = shown(&app, 60, 24);
    assert!(flat(&lines).contains("▎ Research"), "{}", lines.join("\n"));
    // The Agents list too: one pane, enter opens the agent full width.
    key(&mut app, KeyCode::Right);
    assert_eq!(app.sqt.view, SqView::Agents);
    let (lines, _) = shown(&app, 60, 14);
    assert!(flat(&lines).contains("[Agents]") && !flat(&lines).contains("Built in"), "{}", lines.join("\n"));
    key(&mut app, KeyCode::Enter);
    let (lines, _) = shown(&app, 60, 14);
    let f = flat(&lines);
    assert!(f.contains("Name planner") && !f.contains("[Agents]"), "{f}");
}

/// The tab strip of the settings popup, at the top of its border: the line under "╭ Settings".
fn strip(lines: &[String]) -> String {
    lines[row_of(lines, "╭ Settings") + 1].clone()
}

#[test]
fn at_60_columns_the_settings_strip_keeps_the_showing_tab_in_view_and_marks_the_others() {
    let w = World::new("d-strip");
    let mut app = w.app();
    ch(&mut app, 's');
    assert!(matches!(app.mode, Mode::Settings));
    let titles = ["General", "Search", "Smart", "Sessions", "Projects", "Editors"];
    for height in [24u16, 14] {
        for (i, title) in titles.iter().enumerate() {
            app.switch_settings_tab(i);
            let (lines, _) = shown(&app, 60, height);
            let s = strip(&lines);
            if i == 0 && height == 24 {
                println!("Settings strip at 60 columns, on General:\n{s}\n");
            }
            if i == 5 && height == 24 {
                println!("Settings strip at 60 columns, on Editors:\n{s}\n");
            }
            assert!(s.contains(&format!(" {title} ")), "60x{height}: {title} is in view: {s}");
            // Where some tabs are out of view, a mark says on which side.
            let left_out = titles[..i].iter().any(|t| !s.contains(&format!(" {t} ")));
            let right_out = titles[i + 1..].iter().any(|t| !s.contains(&format!(" {t} ")));
            assert_eq!(s.contains('‹'), left_out, "60x{height} on {title}: {s}");
            assert_eq!(s.contains('›'), right_out, "60x{height} on {title}: {s}");
            assert!(left_out || right_out, "six tabs don't all fit in 60 columns: {s}");
        }
    }
    // Wide enough, every tab shows and there are no marks.
    let (lines, _) = shown(&app, 120, 24);
    let s = strip(&lines);
    assert!(titles.iter().all(|t| s.contains(t)) && !s.contains('‹') && !s.contains('›'), "{s}");
}

/// A project with two tickets and one session; its screen open, the Tickets pane in focus.
fn on_the_tickets(name: &str) -> (World, App) {
    let w = World::new(name);
    w.transcript("aaa-1", "/tmp", "a conversation", 10);
    let mut app = w.app();
    let id = app.projects.create("Web shop").unwrap();
    app.projects.assign("aaa-1", Some(&id)).unwrap();
    let mut store = Tickets::open(&w.config, &id);
    store.add(NewTicket { title: "Basket page".into(), ..Default::default() }).unwrap();
    store.add(NewTicket { title: "Checkout".into(), ..Default::default() }).unwrap();
    app.reload_for_test();
    ch(&mut app, '3');
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.pane, Pane::Tickets);
    (w, app)
}

#[test]
fn shift_s_hands_a_ticket_to_a_squad_and_a_says_so_instead() {
    let (_w, mut app) = on_the_tickets("d-shift-s");
    let (lines, _) = shown(&app, 160, 30);
    let f = flat(&lines);
    assert!(f.contains("S hand to a squad") && !f.contains("a hand to a squad"), "{f}");
    // `a` (which archives a session in the pane above) does nothing to a ticket, and says what does.
    ch(&mut app, 'a');
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.active_notice(), Some("S (shift+s) hands a task to a squad."));
    // Shift+s, as a terminal sends it.
    app.on_key(KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT));
    assert!(matches!(app.mode, Mode::SquadPick), "{:?}", app.active_notice());
    key(&mut app, KeyCode::Esc);
    // On the ticket's own screen, the same.
    key(&mut app, KeyCode::Enter);
    assert!(app.tk.open.is_some());
    let (lines, _) = shown(&app, 160, 30);
    assert!(flat(&lines).contains("S hand to a squad"), "{}", lines.join("\n"));
    app.notice = None;
    ch(&mut app, 'a');
    assert!(matches!(app.mode, Mode::List) && app.active_notice() == Some("S (shift+s) hands a task to a squad."));
    app.on_key(KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT));
    assert!(matches!(app.mode, Mode::SquadPick));
}

#[test]
fn j_and_k_move_in_every_list_and_page_keys_move_a_screenful() {
    let w = ten("d-jk");
    let mut app = w.app();
    // Sessions: j/k, and a page is as many rows as the list showed.
    shown(&app, 80, 24);
    let page = app.pages.sessions.get();
    assert_eq!(page, 10, "ten two-line rows at 80x24");
    ch(&mut app, 'j');
    ch(&mut app, 'j');
    ch(&mut app, 'k');
    assert_eq!(app.selected, 1);
    // A page keeps one row of the last screenful in view; at the end of the list it stops at the last row.
    key(&mut app, KeyCode::PageDown);
    assert_eq!(app.selected, (1 + page - 1).min(9));
    key(&mut app, KeyCode::PageUp);
    assert_eq!(app.selected, (1 + page - 1).min(9) - (page - 1));
    // Projects.
    for name in ["One", "Two", "Three"] {
        app.projects.create(name).unwrap();
    }
    app.reload_for_test();
    ch(&mut app, '3');
    ch(&mut app, 'j');
    assert_eq!(app.pselected, 1);
    ch(&mut app, 'k');
    assert_eq!(app.pselected, 0);
    // Squads and Agents.
    ch(&mut app, '4');
    ch(&mut app, 'j');
    assert_eq!(app.sqt.selected, 1);
    key(&mut app, KeyCode::PageDown);
    assert_eq!(app.sqt.selected, 3, "to the last squad");
    ch(&mut app, 'k');
    assert_eq!(app.sqt.selected, 2);
    // The settings' entries.
    ch(&mut app, 's');
    ch(&mut app, 'j');
    assert_eq!(app.setting, 1);
    ch(&mut app, 'k');
    assert_eq!(app.setting, 0);
    key(&mut app, KeyCode::Esc);
    // The Tickets pane.
    let (_w, mut app) = on_the_tickets("d-jk-tickets");
    let first = app.ticket_selected();
    ch(&mut app, 'j');
    assert_eq!(app.ticket_selected(), first + 1);
    ch(&mut app, 'k');
    assert_eq!(app.ticket_selected(), first);
}

#[test]
fn at_120_columns_the_sessions_footer_keeps_t_draft_tasks_and_lets_rename_go_first() {
    let w = ten("dense-footer");
    let mut app = w.app();
    app.reload_for_test();
    let footer_at = |app: &App, cols: u16| shown(app, cols, 30).0.last().cloned().unwrap();
    let (lines, term) = shown(&app, 120, 30);
    let footer = lines.last().unwrap();
    for want in ["t draft tasks", "h hand-off", "m project", "s settings", "? help", "q quit"] {
        assert!(footer.contains(want), "{want}: {footer}");
    }
    assert!(!footer.contains("r rename"), "rename goes before it: {footer}");
    let y = lines.len() - 1;
    assert_eq!(term.backend().buffer()[(col_of(footer, "t draft tasks") as u16, y as u16)].fg, app.effective_theme().accent, "the key");
    // Wide enough, all of them; at 80 columns the settings are still there (t, h and m have gone by then).
    let wide = footer_at(&app, 160);
    assert!(wide.contains("t draft tasks") && wide.contains("r rename") && wide.contains("s settings"), "{wide}");
    let narrow = footer_at(&app, 80);
    assert!(narrow.contains("s settings") && !narrow.contains("t draft tasks") && !narrow.contains("r rename"), "{narrow}");
    // The help lists every one at any width.
    ch(&mut app, '?');
    let (lines, _) = shown(&app, 80, 40);
    let f = flat(&lines);
    assert!(f.contains("t draft tasks") && f.contains("r rename") && f.contains("s settings"), "{f}");
}
