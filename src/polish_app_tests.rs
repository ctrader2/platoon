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
            // A margin of two columns or fewer is blank; a wider one keeps the faded screen, with one blank column
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
    // A popup two columns from each edge: the margins are blank, not two columns of the screen.
    term.draw(|f| {
        wide(f);
        ui::popup_for_test(f, &t, f.area(), Rect::new(2, 1, 26, 4));
    })
    .unwrap();
    for y in 1..5 {
        let l = shown(&term)[y as usize].clone();
        assert!(l.starts_with("  ") && l.ends_with("  "), "{y}: {l:?}");
    }
}
