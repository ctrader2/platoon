//! The small things that make platoon's screens read cleanly, at the level of the app: popups over a clean, faded
//! screen (a wide character can't eat a popup's border, no sliver of the screen at its sides), text that is cut says
//! so, notices that stay with the screen they are about, the help's sections, the words on screen, the search saying
//! why a row matched, and the settings' choosers lined up. Every directory is a temp dir (`World`); nothing here
//! starts a process.

use crate::app::{App, Mode};
use crate::testkit::World;
use crate::ui;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::Rect,
};
use unicode_width::UnicodeWidthStr;

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn ch(app: &mut App, c: char) {
    key(app, KeyCode::Char(c));
}

/// The app at a terminal size, as the event loop would have it.
fn sized(app: &mut App, w: u16, h: u16) -> Terminal<TestBackend> {
    app.sync_size(w, h);
    Terminal::new(TestBackend::new(w, h)).unwrap()
}

/// What the terminal shows after a frame, as lines: a wide character's second cell is part of it, not a cell of its own.
/// (The backend's buffer is what was sent to the terminal, so a cell the frame changed but never sent shows as it was.)
fn draw(term: &mut Terminal<TestBackend>, app: &App) -> Vec<String> {
    term.draw(|f| ui::draw(f, app)).unwrap();
    shown(term)
}

fn shown(term: &Terminal<TestBackend>) -> Vec<String> {
    let buf = term.backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            let mut line = String::new();
            let mut x = 0;
            while x < buf.area.width {
                let s = buf[(x, y)].symbol();
                line.push_str(s);
                x += (s.width() as u16).max(1);
            }
            line
        })
        .collect()
}

/// The terminal's cell at (x, y), as sent.
fn sym(term: &Terminal<TestBackend>, x: u16, y: u16) -> String {
    term.backend().buffer()[(x, y)].symbol().to_string()
}

/// Three conversations, one with Japanese text and an emoji in its folder and its first message.
fn three(name: &str) -> (World, App) {
    let w = World::new(name);
    w.transcript("alpha-1", "/srv/alpha", "fix the login page", 10);
    w.transcript("tokyo-2", "/srv/東京", "テストを書く 🚀 and ship it", 20);
    w.transcript("beta-3", "/srv/beta", "write the release notes", 30);
    let mut app = w.app();
    app.notice = None;
    (w, app)
}

/// The popup's box: the rows from its top corner `╭` to its bottom one, and the columns between, found on the screen.
fn popup_box(lines: &[String]) -> (usize, usize, usize, usize) {
    let top = lines.iter().position(|l| l.contains('╭')).expect("a popup");
    let bottom = (top..lines.len()).find(|&y| lines[y].contains('╰')).expect("its bottom");
    let left = lines[top].chars().position(|c| c == '╭').unwrap();
    let right = lines[top].chars().position(|c| c == '╮').unwrap();
    (top, bottom, left, right)
}

// ---- group 1: popups over the screen -------------------------------------------------------------------------------

#[test]
fn a_wide_character_under_a_popups_edge_never_covers_its_border() {
    let (_w, mut app) = three("p-wide");
    // At 120 columns the settings popup's left border falls on the emoji of the Japanese row's second line.
    let mut term = sized(&mut app, 120, 40);
    let before = draw(&mut term, &app);
    let tokyo = before.iter().position(|l| l.contains("テストを書く 🚀")).expect("the Japanese row");
    ch(&mut app, 's');
    assert!(matches!(app.mode, Mode::Settings));
    let lines = draw(&mut term, &app);
    let ctx = lines.join("\n");
    let (top, bottom, _, _) = popup_box(&lines);
    assert!((top..=bottom).contains(&(tokyo + 1)), "the popup is over the Japanese row: {ctx}");
    // On every row of the popup, the cells sent to the terminal hold its border at both edges (the left one is
    // found by its top corner's column, counted in cells).
    let buf = term.backend().buffer().clone();
    let corner = (0..120u16).find(|&x| buf[(x, top as u16)].symbol() == "╭").unwrap();
    let far = (0..120u16).find(|&x| buf[(x, top as u16)].symbol() == "╮").unwrap();
    for y in top as u16 + 1..bottom as u16 {
        assert_eq!(sym(&term, corner, y), "│", "row {y}: the left border is there: {ctx}");
        assert_eq!(sym(&term, far, y), "│", "row {y}: the right border is there: {ctx}");
        // Just outside, a plain blank: nothing wide can reach over the border.
        assert_eq!(sym(&term, corner - 1, y), " ", "row {y}: {ctx}");
        assert_eq!(sym(&term, far + 1, y), " ", "row {y}: {ctx}");
    }
    assert!(lines[tokyo + 1].contains("│"), "the Japanese row's line shows the border: {}", lines[tokyo + 1]);
    assert!(lines.iter().all(|l| l.width() == 120), "every line is the terminal's width: {ctx}");
}

#[test]
fn a_popup_leaves_no_sliver_of_the_screen_at_its_sides() {
    let (_w, mut app) = three("p-sliver");
    ch(&mut app, 's');
    for (w, h) in [(80u16, 24u16), (60, 24), (120, 40), (200, 60)] {
        let mut term = sized(&mut app, w, h);
        let lines = draw(&mut term, &app);
        let ctx = format!("{w}x{h}\n{}", lines.join("\n"));
        let (top, bottom, _, _) = popup_box(&lines);
        let buf = term.backend().buffer().clone();
        let left = (0..w).find(|&x| buf[(x, top as u16)].symbol() == "╭").unwrap();
        let right = (0..w).find(|&x| buf[(x, top as u16)].symbol() == "╮").unwrap();
        for y in top as u16..=bottom as u16 {
            // A margin of four columns or fewer is blank; a wider one keeps the faded screen, with one blank column
            // against the border.
            let blank = |x: u16| buf[(x, y)].symbol() == " ";
            if left <= ui::POPUP_SLIVER {
                assert!((0..left).all(blank), "{y}: no sliver on the left: {ctx}");
            } else {
                assert!(blank(left - 1), "{y}: {ctx}");
            }
            if w - 1 - right <= ui::POPUP_SLIVER {
                assert!((right + 1..w).all(blank), "{y}: no sliver on the right: {ctx}");
            } else {
                assert!(blank(right + 1), "{y}: {ctx}");
            }
        }
    }
}

/// A popup, by the tab it opens on, the key that opens it and the mode it opens.
type Opens = (char, char, fn(&Mode) -> bool);

/// The popups opened from the Sessions list and the Projects tab, each with the key that opens it.
#[test]
fn every_popup_fades_the_screen_behind_it() {
    let (_w, mut app) = three("p-fade");
    let id = app.projects.create("Web shop").unwrap();
    app.projects.assign("alpha-1", Some(&id)).unwrap();
    app.reload_for_test();
    let faded = |app: &App, term: &Terminal<TestBackend>| {
        let t = app.effective_theme();
        // The title in the top left corner, never under a popup.
        let cell = &term.backend().buffer()[(1, 0)];
        assert_eq!(cell.symbol(), "p");
        cell.fg == t.border
    };
    let cases: [Opens; 7] = [
        ('1', 'r', |m| matches!(m, Mode::Rename)),
        ('1', 'm', |m| matches!(m, Mode::Assign)),
        ('1', 'n', |m| matches!(m, Mode::Picker)),
        ('1', 's', |m| matches!(m, Mode::Settings)),
        ('1', '?', |_| true),
        ('2', 'n', |m| matches!(m, Mode::ProjectName)),
        ('2', 'c', |m| matches!(m, Mode::Look)),
    ];
    // Wide enough that the top left corner is never under a popup.
    for (tab, k, is) in cases {
        ch(&mut app, tab);
        let mut term = sized(&mut app, 160, 40);
        draw(&mut term, &app);
        assert!(!faded(&app, &term), "{tab}{k}: nothing is faded before");
        ch(&mut app, k);
        assert!(is(&app.mode), "{tab}{k}: the popup opened");
        let lines = draw(&mut term, &app);
        assert!(faded(&app, &term), "{tab}{k}: the screen behind is faded:\n{}", lines.join("\n"));
        key(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::List) && !app.help_showing(), "{tab}{k}: closed");
    }
    // The search is the exception on purpose: the list behind narrows as you type, in its own colors.
    ch(&mut app, '1');
    let mut term = sized(&mut app, 160, 40);
    ch(&mut app, '/');
    draw(&mut term, &app);
    assert!(!faded(&app, &term), "the search leaves the list as it is");
}

#[test]
fn the_popup_ground_blanks_the_column_outside_each_edge_and_slivers_whole() {
    // A screen of wide characters, a popup over it at an odd column: drawn twice, the way a terminal gets it.
    let t = crate::theme::Theme::by_id("tokyo-night").unwrap();
    let mut term = Terminal::new(TestBackend::new(30, 6)).unwrap();
    let wide = |f: &mut ratatui::Frame| {
        for y in 0..6 {
            f.render_widget(ratatui::widgets::Paragraph::new("東".repeat(15)), Rect::new(0, y, 30, 1));
        }
    };
    term.draw(wide).unwrap();
    term.draw(|f| {
        wide(f);
        ui::popup_for_test(f, &t, f.area(), Rect::new(7, 1, 15, 4));
    })
    .unwrap();
    for y in 1..5 {
        let l = shown(&term)[y as usize].clone();
        assert_eq!(sym(&term, 6, y), " ", "{y}: {l}");
        assert!(["│", "╭", "╰"].contains(&sym(&term, 7, y).as_str()), "{y}: the left border shows: {l}");
        assert!(["│", "╮", "╯"].contains(&sym(&term, 21, y).as_str()), "{y}: the right border shows: {l}");
        assert_eq!(sym(&term, 22, y), " ", "{y}: {l}");
    }
    // A popup four columns from each edge: the margins are blank, not two columns of the screen beside a blank one.
    term.draw(|f| {
        wide(f);
        ui::popup_for_test(f, &t, f.area(), Rect::new(4, 1, 22, 4));
    })
    .unwrap();
    for y in 1..5 {
        let l = shown(&term)[y as usize].clone();
        assert!(l.starts_with("    ") && l.ends_with("    "), "{y}: {l:?}");
    }
}

// ---- group 5: the help -------------------------------------------------------------------------------------------

/// A project "Web shop" with one session and a ticket, its screen open (the Sessions pane has the keys).
fn on_a_project(name: &str) -> (World, App, String) {
    let w = World::new(name);
    w.transcript("aaa-1", "/srv/shop", "a conversation", 10);
    let mut app = w.app();
    app.notice = None;
    let id = app.projects.create("Web shop").unwrap();
    app.projects.assign("aaa-1", Some(&id)).unwrap();
    crate::tickets::Tickets::open(&w.config, &id)
        .add(crate::tickets::NewTicket { title: "Coupon totals".into(), ..Default::default() })
        .unwrap();
    app.reload_for_test();
    ch(&mut app, '2');
    key(&mut app, KeyCode::Enter);
    assert!(app.open_project().is_some());
    (w, app, id)
}

fn section(help: &[(String, String, String)], name: &str) -> Vec<(String, String)> {
    help.iter().filter(|(s, _, _)| s == name).map(|(_, k, d)| (k.clone(), d.clone())).collect()
}

#[test]
fn the_help_says_the_same_about_platoon_everywhere_and_lists_settings_and_search_where_they_work() {
    let (_w, mut app, _) = on_a_project("h-sections");
    // (where, does `s` open the settings here, does `/` search here)
    let mut seen: Vec<(String, Vec<(String, String)>)> = Vec::new();
    let mut check = |app: &App, place: &str, s: bool, slash: bool| {
        let help = ui::help_for_test(app);
        let here = section(&help, "On this screen");
        let anywhere = section(&help, "Anywhere in platoon");
        assert!(!help.iter().any(|(s, _, _)| s == "Everywhere"), "{place}");
        let keys: Vec<&str> = anywhere.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["1-3", "ctrl+space", "q", "?"], "{place}: only the keys that work on every screen");
        assert_eq!(here.iter().any(|(k, d)| k == "s" && d == "settings"), s, "{place}: s {here:?}");
        assert_eq!(here.iter().any(|(k, d)| k == "/" && d == "search"), slash, "{place}: / {here:?}");
        // Never one action twice: the back key's line says esc does the same, so the screen's esc back isn't listed.
        assert!(!here.iter().any(|(k, d)| k == "esc" && d == "back"), "{place}: {here:?}");
        let mut all: Vec<(String, String)> = here.clone();
        all.extend(anywhere.iter().cloned());
        for (i, a) in all.iter().enumerate() {
            assert!(!all[i + 1..].contains(a), "{place}: {a:?} listed twice");
        }
        seen.push((place.to_string(), anywhere));
    };
    // The project screen, its Sessions pane, then its Tickets pane.
    check(&app, "project screen", true, true);
    key(&mut app, KeyCode::Tab);
    check(&app, "Tickets pane", false, true);
    // A ticket's own screen.
    key(&mut app, KeyCode::Enter);
    assert!(app.tk.open.is_some());
    check(&app, "ticket", false, false);
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Esc);
    assert!(app.open_project.is_none());
    // The three tabs.
    ch(&mut app, '1');
    check(&app, "Sessions", true, true);
    ch(&mut app, '2');
    check(&app, "Projects", true, true);
    ch(&mut app, '3');
    check(&app, "Squads", true, false);
    let first = seen[0].1.clone();
    for (place, anywhere) in &seen {
        assert_eq!(anywhere, &first, "{place}: the same everywhere");
    }
}

#[test]
fn the_help_explains_the_estimate_mark_and_the_pane_arrows() {
    let (_w, mut app, _) = on_a_project("h-notes");
    let help = ui::help_for_test(&app);
    let here = section(&help, "On this screen");
    assert!(here.contains(&("tab".into(), "next pane".into())), "{here:?}");
    assert!(here.iter().any(|(k, _)| k == "shift+arrows"), "the arrows are kept in the help: {here:?}");
    let notes = section(&help, "Good to know");
    assert!(notes.iter().any(|(_, n)| n.starts_with("A ~ before a percentage means it is an estimate")), "{notes:?}");
    // The footer says only "tab next pane".
    let mut term = sized(&mut app, 120, 30);
    let lines = draw(&mut term, &app);
    let footer = lines.last().unwrap();
    assert!(footer.contains("tab next pane") && !footer.contains("shift+arrows"), "{footer}");
}

// ---- group 6: the words -------------------------------------------------------------------------------------------

fn flat(lines: &[String]) -> String {
    lines.join(" ").replace(['│', '╭', '╮', '╰', '╯', '─'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn a_ticket_that_can_be_handed_over_says_can_start_and_the_states_read_in_sentence_case() {
    use crate::tickets::State;
    let words: Vec<&str> =
        [State::New, State::Working, State::Waiting, State::Ready, State::Done, State::Cancelled].iter().map(|s| s.label()).collect();
    assert_eq!(words, ["New", "Working", "Waiting on you", "Ready to publish", "Done", "Cancelled"]);
    for w in &words {
        assert!(w.chars().next().unwrap().is_uppercase(), "{w}");
    }
    assert_eq!(crate::tickets::waits_for_words(&[1, 3]), "Waits for #1, #3");
    assert!(crate::tickets::QUEUED_WORDS.starts_with('W'));
    let (w, mut app, id) = on_a_project("w-states");
    crate::tickets::Tickets::open(&w.config, &id)
        .add(crate::tickets::NewTicket { title: "After it".into(), blocked_by: vec![1], ..Default::default() })
        .unwrap();
    app.reload_for_test();
    let mut term = sized(&mut app, 120, 30);
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("#1 Coupon totals New Can start"), "{f}");
    assert!(f.contains("#2 After it New Waits for #1"), "{f}");
    assert!(!f.contains("ready"), "the old word is gone: {f}");
    // On the Projects tab: how many can start.
    key(&mut app, KeyCode::Esc);
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("2 open · 1 can start"), "{f}");
    // On the ticket's own screen.
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Enter);
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("State New · Can start"), "{f}");
}

#[test]
fn the_screens_say_session_and_every_popup_title_starts_with_a_capital() {
    let (_w, mut app) = three("w-words");
    let mut term = sized(&mut app, 120, 40);
    // The rename box.
    ch(&mut app, 'r');
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("Rename session") && !f.contains("onversation"), "{f}");
    key(&mut app, KeyCode::Esc);
    // The settings: every entry's name and explanation on the General tab.
    ch(&mut app, 's');
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("Archive sessions after") && !f.contains("onversation"), "{f}");
    key(&mut app, KeyCode::Esc);
    // The folder picker: for a new session, enter starts it; for the code editor, enter opens it.
    ch(&mut app, 'n');
    let f = flat(&draw(&mut term, &app));
    let n = app.picker.results.len();
    assert!(f.contains("New session: pick a folder") && f.contains("enter start"), "{f}");
    assert!(f.contains(&format!("{n} folder")) && !f.contains(" dirs"), "{f}");
    app.picker.editor = true;
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("Pick a folder to open in your editor") && f.contains("enter open") && !f.contains("enter start"), "{f}");
    key(&mut app, KeyCode::Esc);
    // Every popup's title has a capital first letter, whatever it is.
    for k in ['r', 'm', 'n', 's', '?'] {
        ch(&mut app, k);
        let lines = draw(&mut term, &app);
        let top = lines.iter().find(|l| l.contains('╭')).unwrap().clone();
        let title = top.split('╭').nth(1).unwrap().trim_start_matches(['─', ' ']).chars().next().unwrap();
        assert!(title.is_uppercase(), "{k}: {top}");
        key(&mut app, KeyCode::Esc);
    }
}

#[test]
fn the_notes_and_the_squads_tab_say_it_plainly() {
    let (w, mut app, id) = on_a_project("w-notes");
    let _ = w;
    std::fs::write(app.projects.notes_path(&id), "# Overview\nSell socks.\n").unwrap();
    let mut term = sized(&mut app, 120, 30);
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("Every session in this project starts with these notes."), "{f}");
    assert!(!f.contains("context"), "{f}");
    key(&mut app, KeyCode::Esc);
    ch(&mut app, '3');
    key(&mut app, KeyCode::Enter);
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("its system prompt and model are in Settings (s) → Projects."), "{f}");
    assert!(!f.contains("on the Projects tab"), "{f}");
}

// ---- group 2: cut text says so ------------------------------------------------------------------------------------

/// The harness's shop: seven tickets in every state, one waiting for the first.
fn a_shop(name: &str) -> (World, App) {
    use crate::tickets::{NewTicket, State, Tickets};
    let (w, mut app, id) = on_a_project(name);
    let mut store = Tickets::open(&w.config, &id);
    for (title, blocked) in [
        ("Move prices to the server", vec![]),
        ("Pick a payment provider", vec![]),
        ("Checkout confirmation page", vec![]),
        ("Remove the old basket code", vec![1]),
        ("Ship the shop", vec![]),
        ("Old idea", vec![]),
    ] {
        store.add(NewTicket { title: title.into(), blocked_by: blocked, ..Default::default() }).unwrap();
    }
    for (n, s) in [(2, State::Working), (3, State::Waiting), (4, State::Ready), (6, State::Done), (7, State::Cancelled)] {
        store.set_state(n, &s).unwrap();
    }
    app.reload_for_test();
    app.tk.unfolded = true;
    (w, app)
}

/// The Tickets pane's rows: the lines between its top and bottom borders, without them.
fn tickets_pane(lines: &[String]) -> Vec<String> {
    let top = lines.iter().position(|l| l.contains("╭ Tickets")).expect("the Tickets pane");
    let left = lines[top].chars().position(|c| c == '╭').unwrap();
    let right = left + lines[top].chars().skip(left + 1).position(|c| c == '╮').unwrap() + 1;
    lines[top + 1..]
        .iter()
        .take_while(|l| !l.chars().nth(left).is_some_and(|c| c == '╰'))
        .map(|l| l.chars().skip(left + 1).take(right - left - 1).collect::<String>())
        .collect()
}

#[test]
fn the_tickets_pane_never_cuts_a_state_and_cuts_the_title_instead() {
    let (_w, mut app) = a_shop("c-tickets");
    // (size, the words every row must show whole)
    let full = ["Can start", "Working", "Waiting on you", "Ready to publish", "Waits for #1", "Done", "Cancelled"];
    let short = ["Can start", "Working", "Waiting", "To publish", "Waits #1", "Done", "Cancelled"];
    for (w, h, words) in [(120u16, 40u16, &full), (80, 24, &short), (60, 24, &short)] {
        let mut term = sized(&mut app, w, h);
        let lines = draw(&mut term, &app);
        let ctx = format!("{w}x{h}\n{}", lines.join("\n"));
        let pane = tickets_pane(&lines);
        // Every ticket row the pane has room for (a short pane scrolls).
        let mut seen = 0;
        for (n, word) in words.iter().enumerate() {
            let Some(row) = pane.iter().find(|l| l.contains(&format!("#{} ", n + 1))) else { continue };
            assert!(row.trim_end().ends_with(word), "#{}: {word:?} whole at the end of its row: {row:?}\n{ctx}", n + 1);
            seen += 1;
        }
        assert!(seen >= 5, "{ctx}");
        // What is cut is the title, and it says so.
        let first = pane.iter().find(|l| l.contains("#1 ")).unwrap();
        assert!(first.contains("Coupon totals") || first.contains('…'), "{ctx}");
        if w < 120 {
            assert!(pane.iter().any(|l| l.contains("#4 Checkout") && l.contains("…")), "a long title is cut with …: {ctx}");
        }
    }
    // Wide, the state has its own column and what a ticket waits for another.
    let mut term = sized(&mut app, 200, 60);
    let pane = tickets_pane(&draw(&mut term, &app));
    let five = pane.iter().find(|l| l.contains("#5 ")).unwrap();
    assert!(five.contains("New") && five.trim_end().ends_with("Waits for #1"), "{five}");
}

#[test]
fn a_long_agent_name_is_cut_and_keeps_its_model_like_every_other_row() {
    let (_w, mut app) = three("c-agents");
    ch(&mut app, '3');
    key(&mut app, KeyCode::Right);
    for (w, h) in [(80u16, 24u16), (120, 40)] {
        let mut term = sized(&mut app, w, h);
        let lines = draw(&mut term, &app);
        let ctx = format!("{w}x{h}\n{}", lines.join("\n"));
        // The list's pane, left of the details.
        let rows: Vec<String> = lines
            .iter()
            .map(|l| l.split("││").next().unwrap_or_default().to_string())
            .filter(|l| l.starts_with(" │") && ["planner", "report-wr", "summarizer"].iter().any(|n| l.contains(n)))
            .collect();
        assert_eq!(rows.len(), 3, "{ctx}");
        // The same things on every row: the model, and at 120 columns how many squads use it.
        for r in &rows {
            assert!(["Opus", "Sonnet", "Haiku"].iter().any(|m| r.contains(m)), "{r}\n{ctx}");
            assert_eq!(r.contains("· 1 squad"), w >= 120, "{r}\n{ctx}");
        }
    }
}

#[test]
fn a_pane_cut_at_its_bottom_says_how_much_more_is_below() {
    let (w, mut app, id) = on_a_project("c-more");
    let notes: String = (1..=40).map(|i| format!("- point number {i}\n")).collect();
    std::fs::write(app.projects.notes_path(&id), format!("# Overview\n{notes}")).unwrap();
    let _ = w;
    let mut term = sized(&mut app, 80, 24);
    let lines = draw(&mut term, &app);
    let ctx = lines.join("\n");
    let notes_bottom = lines.iter().rposition(|l| l.contains("more lines below")).expect(&ctx);
    assert!(lines[notes_bottom + 1].contains('╰'), "on the notes pane's last row: {ctx}");
    // In the dim color.
    let t = app.effective_theme().clone();
    let buf = term.backend().buffer().clone();
    let x = (0..80).find(|&x| buf[(x, notes_bottom as u16)].symbol() == "↓").unwrap();
    assert_eq!(buf[(x, notes_bottom as u16)].fg, t.dim);
    // Scrolled to the end, nothing more is below.
    app.on_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
    assert_eq!(app.pane, crate::app::Pane::Notes);
    for _ in 0..60 {
        key(&mut app, KeyCode::Down);
    }
    let lines = draw(&mut term, &app);
    assert!(!lines.iter().any(|l| l.contains("more lines below")), "{}", lines.join("\n"));
    assert!(lines.iter().any(|l| l.contains("point number 40")), "{}", lines.join("\n"));
    // The Squads tab's workflow, at 80x24, is longer than its pane.
    key(&mut app, KeyCode::Esc);
    ch(&mut app, '3');
    let lines = draw(&mut term, &app);
    assert!(lines.iter().any(|l| l.contains("Workflow")), "{}", lines.join("\n"));
    let more = lines.iter().position(|l| l.contains("more lines below")).unwrap_or_else(|| panic!("{}", lines.join("\n")));
    assert!(lines[more + 1].contains('╰'), "{}", lines.join("\n"));
}

#[test]
fn a_session_folder_in_the_project_pane_is_cut_where_a_folder_starts() {
    let w = World::new("c-path");
    w.transcript("aaa-1", "/srv/clients/acme-corporation/shop", "a conversation", 10);
    let mut app = w.app();
    app.notice = None;
    let id = app.projects.create("Web shop").unwrap();
    app.projects.assign("aaa-1", Some(&id)).unwrap();
    app.reload_for_test();
    ch(&mut app, '2');
    key(&mut app, KeyCode::Enter);
    let mut term = sized(&mut app, 80, 24);
    let lines = draw(&mut term, &app);
    let row = lines.iter().find(|l| l.contains("…/")).unwrap_or_else(|| panic!("{}", lines.join("\n")));
    assert!(row.contains("/shop") && !row.contains("…c") && !row.contains("…o"), "{row}");
}

// ---- group 3: notices --------------------------------------------------------------------------------------------

#[test]
fn a_notice_stays_with_the_screen_it_was_given_on() {
    let (_w, mut app) = three("n-screen");
    let mut term = sized(&mut app, 120, 30);
    // Renamed: the box closes and the notice is about that, so it shows on the list.
    ch(&mut app, 'r');
    key(&mut app, KeyCode::End);
    for c in "-fix".chars() {
        ch(&mut app, c);
    }
    key(&mut app, KeyCode::Enter);
    let said = app.active_notice().expect("the rename says so").to_string();
    assert!(said.starts_with("Renamed to"), "{said}");
    assert!(draw(&mut term, &app).last().unwrap().contains(&said));
    // Another tab: it doesn't follow.
    ch(&mut app, '2');
    assert_eq!(app.active_notice(), None);
    let lines = draw(&mut term, &app);
    assert!(!lines.iter().any(|l| l.contains("Renamed to")), "{}", lines.join("\n"));
    // Nor does it come back with the first tab.
    ch(&mut app, '1');
    assert_eq!(app.active_notice(), None);
    // A popup is another screen too.
    app.notify("Copied 3 lines");
    app.settle_notice();
    ch(&mut app, 's');
    assert_eq!(app.active_notice(), None, "not in the settings");
    key(&mut app, KeyCode::Esc);
}

#[test]
fn a_notice_from_before_attaching_never_shows_in_the_sessions_bar() {
    let (_w, mut app) = three("n-attach");
    app.notify("Renamed to \"alpha-fix\"");
    app.settle_notice();
    assert!(app.active_notice().is_some());
    // Attached by something other than a key (as a hand-off or a squad does, from a refresh).
    let (client, _host) = crate::client::Client::for_test(29, 120);
    app.client = Some(client);
    app.mode = Mode::Attached(4242);
    assert_eq!(app.active_notice(), None, "before anything else ran");
    app.settle_notice();
    assert!(app.notice.is_none(), "and it is gone for good");
    // One given with the attaching itself is about it: it shows there.
    app.notify("Copied 2 lines");
    app.settle_notice();
    assert_eq!(app.active_notice(), Some("Copied 2 lines"));
}

#[test]
fn a_long_notice_is_cut_beside_help_rather_than_take_its_place() {
    let (_w, mut app) = three("n-footer");
    app.notify("Couldn't save the name: the config folder is read only, so nothing was written and the old name stays");
    for w in [60u16, 80, 120] {
        let mut term = sized(&mut app, w, 24);
        let lines = draw(&mut term, &app);
        let footer = lines.last().unwrap();
        assert!(footer.contains("? help"), "{w}: {footer}");
        // Whole where it fits beside `? help`; cut with "…" where it doesn't.
        assert!(footer.contains("Couldn't save the name") && footer.contains('…') == (w < 120), "{w}: {footer}");
        assert!(footer.width() == w as usize, "{w}: {footer:?}");
    }
}

// ---- group 7: the search and the settings ------------------------------------------------------------------------

#[test]
fn the_quick_filter_marks_the_words_and_shows_a_hidden_field_that_matched() {
    let w = World::new("s-why");
    w.transcript("alpha-1", "/srv/alpha", "fix the login page", 10);
    w.transcript("beta-2", "/srv/beta", "write the release notes for the basket", 20);
    let mut app = w.app();
    app.notice = None;
    let mut term = sized(&mut app, 100, 30);
    let t = app.effective_theme().clone();
    ch(&mut app, '/');
    // A word in the name: marked there, in the accent color.
    for c in "alph".chars() {
        ch(&mut app, c);
    }
    let lines = draw(&mut term, &app);
    let buf = term.backend().buffer().clone();
    let y = lines.iter().rposition(|l| l.contains("▎ alpha")).unwrap_or_else(|| panic!("{}", lines.join("\n")));
    let x = (0..100).find(|&x| buf[(x, y as u16)].symbol() == "a" && buf[(x + 1, y as u16)].symbol() == "l").unwrap();
    assert_eq!(buf[(x, y as u16)].fg, t.accent, "the typed words are marked");
    assert!(buf[(x, y as u16)].modifier.contains(ratatui::style::Modifier::BOLD));
    assert_ne!(buf[(x + 4, y as u16)].fg, t.accent, "the rest of the name is not");
    // A word only in what the row doesn't show (its first message): that part shows, dim, the word marked.
    for _ in 0..4 {
        key(&mut app, KeyCode::Backspace);
    }
    for c in "basket".chars() {
        ch(&mut app, c);
    }
    let lines = draw(&mut term, &app);
    let row = lines.iter().find(|l| l.contains("beta") && l.contains("basket")).unwrap_or_else(|| panic!("{}", lines.join("\n")));
    assert!(!row.contains("/srv/beta"), "the matching text takes the folder's place: {row}");
    let y = lines.iter().position(|l| l == row).unwrap() as u16;
    let bx = (0..100).find(|&x| buf_text(&term, x, y, 6) == "basket").unwrap();
    let buf = term.backend().buffer().clone();
    assert_eq!(buf[(bx, y)].fg, t.accent);
    // Dim, kept readable on the selected row's fill.
    assert_eq!(buf[(bx - 2, y)].fg, t.readable_on(t.dim, t.surface, crate::theme::MIN_DIM), "the rest of it dim");
    // A folder that matches is marked where it is.
    for _ in 0..6 {
        key(&mut app, KeyCode::Backspace);
    }
    for c in "srv/al".chars() {
        ch(&mut app, c);
    }
    let lines = draw(&mut term, &app);
    // (The list behind shows above the popup: the popup's row is the last one.)
    let y = lines.iter().rposition(|l| l.contains("/srv/alpha")).unwrap() as u16;
    let sx = (0..100).find(|&x| buf_text(&term, x, y, 6) == "srv/al").unwrap();
    assert_eq!(term.backend().buffer()[(sx, y)].fg, t.accent);
}

/// The `n` cells from (x, y), as text.
fn buf_text(term: &Terminal<TestBackend>, x: u16, y: u16, n: u16) -> String {
    let buf = term.backend().buffer();
    (x..(x + n).min(buf.area.width)).map(|x| buf[(x, y)].symbol()).collect()
}

#[test]
fn the_settings_choosers_of_a_tab_line_up() {
    let (_w, mut app) = three("s-align");
    ch(&mut app, 's');
    for (w, h) in [(120u16, 40u16), (80, 24), (60, 24)] {
        for tab in 0..crate::settings::TABS.len() {
            app.switch_settings_tab(tab);
            let mut term = sized(&mut app, w, h);
            let lines = draw(&mut term, &app);
            let ctx = format!("{w}x{h} tab {tab}\n{}", lines.join("\n"));
            // Where each chooser's `‹` and `›` are, in cells.
            let buf = term.backend().buffer().clone();
            // Below the popup's own tab strip (which has `‹ ›` too, at narrow widths) and the rule under it.
            let top = lines.iter().position(|l| l.contains("╭ Settings")).unwrap() as u16;
            let marks: Vec<(u16, u16)> = (top + 3..h)
                .filter_map(|y| {
                    let l = (0..w).find(|&x| buf[(x, y)].symbol() == "‹")?;
                    let r = (0..w).find(|&x| buf[(x, y)].symbol() == "›")?;
                    Some((l, r))
                })
                .collect();
            assert!(!marks.is_empty() || tab != 0, "{ctx}");
            // From 80 columns every chooser lines up. At 60 the names come first: a value too wide to share the width
            // that leaves every name whole keeps its own (the others still line up), and none runs out of the popup.
            let right = marks.iter().map(|m| m.1).max().unwrap_or(0);
            assert!(marks.iter().all(|m| m.1 == right), "every chooser ends in one column: {marks:?}\n{ctx}");
            if w >= 80 {
                assert!(marks.iter().all(|m| m.0 == marks[0].0), "and starts in one: {marks:?}\n{ctx}");
            }
            // And every name beside a chooser is whole.
            for item in crate::settings::TABS[tab].items.iter().filter(|it| !matches!(it.kind, crate::settings::Kind::Instructions(_))) {
                let shown = lines.iter().any(|l| l.contains(item.name));
                let cut = lines.iter().any(|l| l.contains(&item.name[..item.name.len().min(10)]) && l.contains('…'));
                assert!(shown || !cut, "{}: {ctx}", item.name);
            }
        }
    }
}
