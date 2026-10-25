//! Density and narrow widths, drawn the way a terminal shows them (TestBackend) at 80x24, 60x24 and 60x14: the
//! Sessions list's two lines a conversation and its selection, the gaps between columns and the order they go in,
//! the CONTEXT column left blank when nothing is known, the settings' tab strip, `s` starting a task from the Tasks
//! tab, j/k and PageUp/PageDown in the lists, and which hints the Sessions
//! footer keeps at 120 and 80 columns. Every directory is a temp dir
//! (`World`); nothing here starts a process.

use crate::app::{App, Mode};
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
fn at_80_and_60_columns_the_columns_keep_apart_and_context_goes_before_dir() {
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
    assert!(!head.contains("CONTEXT"), "60: CONTEXT goes before DIR and NAME: {head}");
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
    assert!(known.contains(" 25% "), "the figure where there is data: {known}");
    assert!(!bare.contains('%'), "blank, not 0%: {bare}");
    // Blank, but the row still lines up: LAST ends where it does on the other row.
    assert_eq!(bare.trim_end_matches(['│', ' ']).chars().count(), known.trim_end_matches(['│', ' ']).chars().count());
}

#[test]
fn the_context_column_says_how_full_each_conversation_is_and_warns_above_80_percent() {
    let w = World::new("d-context-pct");
    let at = crate::testkit::now_ms();
    // The last reply's input, from the cache and not: 120k of a 200k window (Haiku 4.5's) is 60%; 170k is 85%; 160k is 80% exactly.
    let talk = |id: &str, cwd: &str, mins: u64, input: u64, write: u64, read: u64| {
        let t = at - mins * 60_000;
        let lines = [
            serde_json::json!({"type":"user","cwd":cwd,"timestamp":crate::testkit::iso(t - 5000),"message":{"role":"user","content":format!("talk {id}")}}),
            serde_json::json!({"type":"assistant","cwd":cwd,"timestamp":crate::testkit::iso(t),"message":{"model":"claude-haiku-4-5",
                "usage":{"input_tokens":input,"cache_creation_input_tokens":write,"cache_read_input_tokens":read,"output_tokens":50}}}),
        ];
        w.transcript_lines(id, &lines.map(|l| l.to_string()));
    };
    talk("sixty-1", "/srv/sixty", 1, 1_000, 19_000, 100_000);
    talk("full-2", "/srv/full", 2, 0, 0, 170_000);
    talk("edge-3", "/srv/edge", 3, 0, 0, 160_000);
    let mut app = w.app();
    app.notice = None;
    let t = app.theme.clone();
    let (lines, term) = shown(&app, 80, 24);
    let head = &lines[row_of(&lines, "STATUS")];
    let x = col_of(head, "CONTEXT");
    let figure = |name: &str, want: &str| {
        let y = row_of(&lines, name);
        assert_eq!(lines[y].chars().skip(x).take(want.len()).collect::<String>(), want, "{name}: under CONTEXT\n{}", lines.join("\n"));
        term.backend().buffer()[(x as u16, y as u16)].fg
    };
    assert_eq!(figure("sixty ", "60%"), t.subtext);
    assert_eq!(figure("edge ", "80%"), t.subtext, "80% is not above 80%");
    assert_eq!(figure("full ", "85%"), t.warn, "above 80%: the warning colour");
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

/// A project with two tasks and one session; the Tasks tab showing.
fn on_the_tasks(name: &str) -> (World, App) {
    let w = World::new(name);
    w.transcript("aaa-1", "/tmp", "a conversation", 10);
    let mut app = w.app();
    let id = app.projects.create("Web shop").unwrap();
    app.projects.assign("aaa-1", Some(&id)).unwrap();
    let mut store = Tickets::open(&w.config, &id);
    store.add(NewTicket { title: "Basket page".into(), folder: Some("/tmp".into()), ..Default::default() }).unwrap();
    store.add(NewTicket { title: "Checkout".into(), folder: Some("/tmp".into()), ..Default::default() }).unwrap();
    app.reload_for_test();
    ch(&mut app, '3');
    assert!(app.board_showing());
    (w, app)
}

#[test]
fn s_on_the_tasks_tab_starts_the_picked_task_and_is_not_the_settings_there() {
    let (_w, mut app) = on_the_tasks("d-s");
    let (lines, _) = shown(&app, 160, 30);
    let f = flat(&lines);
    assert!(f.contains("s start") && !f.contains("s settings"), "{f}");
    ch(&mut app, 's');
    assert!(matches!(app.mode, Mode::SquadPick), "{:?}", app.active_notice());
    key(&mut app, KeyCode::Esc);
    // On the task's own screen, the same.
    key(&mut app, KeyCode::Enter);
    assert!(app.tk.open.is_some());
    let (lines, _) = shown(&app, 160, 30);
    assert!(flat(&lines).contains("s start"), "{}", lines.join("\n"));
    ch(&mut app, 's');
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
    ch(&mut app, '2');
    ch(&mut app, 'j');
    assert_eq!(app.pselected, 1);
    ch(&mut app, 'k');
    assert_eq!(app.pselected, 0);
    // The settings' entries.
    ch(&mut app, 's');
    let at = app.setting;
    ch(&mut app, 'j');
    assert_eq!(app.setting, at + 1);
    ch(&mut app, 'k');
    assert_eq!(app.setting, at);
    key(&mut app, KeyCode::Esc);
    // The Tasks tab.
    let (_w, mut app) = on_the_tasks("d-jk-tasks");
    let first = app.board_selected();
    ch(&mut app, 'j');
    assert_eq!(app.board_selected(), first + 1);
    ch(&mut app, 'k');
    assert_eq!(app.board_selected(), first);
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
