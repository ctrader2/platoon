//! The frame and the help, at the level of the app: no frame around the screens (a column of air at each side), the
//! header in two rows with the Active/Archived switch on the rule, the `?` help popup built from the footer's own
//! tables, the attached view at the terminal's full width, and the message below the smallest size (60x14), which
//! lets no key act but quit. Pinned at 80x24, 60x24 and 60x14. Every directory is a temp dir (`World`); nothing here
//! starts a process.

use crate::app::{App, Hit, MIN_HEIGHT, MIN_WIDTH, Mode};
use crate::client::Client;
use crate::host::{F_RESIZE, read_frame};
use crate::testkit::World;
use crate::ui;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
    layout::Rect,
};
use std::{fs, os::unix::net::UnixStream, time::Duration};
use unicode_width::UnicodeWidthStr;

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn ch(app: &mut App, c: char) {
    key(app, KeyCode::Char(c));
}

fn click(app: &mut App, x: u16, y: u16) {
    app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: x, row: y, modifiers: KeyModifiers::NONE });
    app.on_mouse(MouseEvent { kind: MouseEventKind::Up(MouseButton::Left), column: x, row: y, modifiers: KeyModifiers::NONE });
}

/// The screen as lines of what a terminal shows: a wide character's second cell is part of it, not a cell of its own.
fn draw(term: &mut Terminal<TestBackend>, app: &App) -> Vec<String> {
    term.draw(|f| ui::draw(f, app)).unwrap();
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

fn flat(lines: &[String]) -> String {
    lines.join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The app at a terminal size, as the event loop would have it (the size is what the keys and the pty go by).
fn sized(app: &mut App, w: u16, h: u16) -> Terminal<TestBackend> {
    app.sync_size(w, h);
    Terminal::new(TestBackend::new(w, h)).unwrap()
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

const FRAME: [char; 6] = ['╭', '╮', '╰', '╯', '│', '┃'];

/// Every line is the terminal's width (wide characters counted as two), and its first and last cells are blank: no
/// frame, and nothing runs to the edge.
fn edges_clear(lines: &[String], w: u16, ctx: &str) {
    for (y, l) in lines.iter().enumerate() {
        assert_eq!(l.width(), w as usize, "{ctx}: line {y} is the terminal's width: {l:?}");
        let (first, last) = (l.chars().next().unwrap(), l.chars().last().unwrap());
        assert!(first == ' ' && last == ' ', "{ctx}: line {y} keeps clear of the edges: {l:?}");
    }
}

// ---- the Sessions list ---------------------------------------------------------------------------------------------

#[test]
fn the_sessions_list_has_no_frame_two_rows_of_header_and_keeps_wide_text_inside_its_row() {
    let (_w, mut app) = three("f-sessions");
    for (w, h) in [(80u16, 24u16), (60, 24), (60, 14)] {
        let mut term = sized(&mut app, w, h);
        let lines = draw(&mut term, &app);
        let ctx = format!("{w}x{h}\n{}", lines.join("\n"));
        edges_clear(&lines, w, &ctx);
        assert!(!lines.iter().any(|l| l.contains(FRAME)), "no box anywhere on the list: {ctx}");
        // Row 0: the title and the three tabs, whole, with no counts in them.
        assert!(lines[0].starts_with(" platoon    1 Sessions   2 Tasks   3 Projects   4 Squads "), "{ctx}");
        // Row 1: the rule, with the Active/Archived switch set into its right end.
        assert!(lines[1].starts_with(" ──────────━━━━━━━━━━━━──"), "the current tab is marked on the rule: {ctx}");
        assert!(lines[1].trim_end().ends_with("Active  3   Archived  0   ←→"), "{ctx}");
        // Row 2: the column titles. Then the rows.
        assert!(lines[2].contains("NAME") && lines[2].contains("LAST"), "{ctx}");
        assert!(lines[3].contains("alpha") && lines[4].contains("fix the login page"), "{ctx}");
        // The Japanese row: its folder and its message are there, inside the row, and its time is under LAST with the others.
        let tokyo = lines.iter().position(|l| l.contains("東京")).expect(&ctx);
        assert!(lines[tokyo + 1].contains("テストを書く 🚀"), "{ctx}");
        let last_col = |l: &str| l.trim_end().width();
        assert_eq!(last_col(&lines[tokyo]), last_col(&lines[3]), "the time ends where the other rows' do: {ctx}");
        // The footer: the last row, `? help` always in it.
        let footer = &lines[h as usize - 1];
        assert!(footer.starts_with(" enter open") && footer.contains("? help"), "{ctx}");
    }
}

#[test]
fn the_footer_keeps_help_however_narrow_and_drops_the_others_first() {
    let (_w, mut app) = three("f-footer");
    for w in [60u16, 70, 80, 120, 200] {
        let mut term = sized(&mut app, w, 24);
        let lines = draw(&mut term, &app);
        let footer = &lines[23];
        assert!(footer.contains("? help"), "{w}: {footer}");
        // Whole hints only: nothing is cut off at the right.
        assert!(footer.ends_with(' ') && !footer.contains('…'), "{w}: {footer:?}");
    }
    // On a ticket-less project screen and the Squads tab too.
    for top in ['2', '3'] {
        ch(&mut app, top);
        let mut term = sized(&mut app, 60, 14);
        let lines = draw(&mut term, &app);
        assert!(lines[13].contains("? help"), "tab {top}: {}", lines.join("\n"));
    }
}

// ---- the Projects tab ----------------------------------------------------------------------------------------------

#[test]
fn the_projects_tab_puts_its_switch_on_the_rule_where_it_fits_and_on_a_row_of_its_own_where_it_does_not() {
    let (_w, mut app) = three("f-projects");
    let id = app.projects.create("Web shop").unwrap();
    app.projects.assign("alpha-1", Some(&id)).unwrap();
    app.reload_for_test();
    ch(&mut app, '3');
    assert!(app.projects_list_showing());
    for (w, h) in [(80u16, 24u16), (60, 24), (60, 14)] {
        let mut term = sized(&mut app, w, h);
        let lines = draw(&mut term, &app);
        let ctx = format!("{w}x{h}\n{}", lines.join("\n"));
        edges_clear(&lines, w, &ctx);
        assert!(lines[0].contains("1 Sessions   2 Tasks   3 Projects   4 Squads"), "{ctx}");
        // The Projects tab's mark sits further right: at 80 columns the switch still fits beside it on the rule; at 60
        // it doesn't (the mark leaves 22 cells, the switch needs 26), so it has the row under the rule.
        let (switch_row, titles_row) = if w >= 80 { (1, 2) } else { (2, 3) };
        assert!(lines[switch_row].contains("Active  1   Archived  0"), "{ctx}");
        assert!(lines[titles_row].contains("PROJECT") && lines[titles_row].contains("SESSIONS"), "{ctx}");
        assert!(lines.iter().any(|l| l.contains(" WS ") && l.contains("Web shop")), "{ctx}");
        assert!(lines[h as usize - 1].contains("? help"), "{ctx}");
    }
}

// ---- the help popup -----------------------------------------------------------------------------------------------

/// The words inside the help popup only (not the screen around it), as one line.
fn popup_text(term: &Terminal<TestBackend>, app: &App) -> String {
    let r = app.hits.borrow().iter().find(|(_, h)| matches!(h, Hit::Inert)).map(|(r, _)| *r).expect("the popup");
    let buf = term.backend().buffer();
    let lines: Vec<String> = (r.y..r.bottom()).map(|y| (r.x..r.right()).map(|x| buf[(x, y)].symbol()).collect()).collect();
    flat(&lines)
}

#[test]
fn question_mark_lists_every_key_of_the_screen_even_those_the_footer_dropped() {
    let (_w, mut app) = three("f-help");
    let mut term = sized(&mut app, 80, 24);
    let footer = draw(&mut term, &app)[23].clone();
    // At 80 columns the footer has no room for these...
    for dropped in ["m project", "h hand-off", "v editor", "←→ active/archived", "1-4 tabs"] {
        assert!(!footer.contains(dropped), "{footer}");
    }
    ch(&mut app, '?');
    assert!(app.help_showing() && matches!(app.mode, Mode::List));
    let lines = draw(&mut term, &app);
    let first = flat(&lines);
    // At 80x24 it is longer than the screen: it scrolls, and says how much is below.
    assert!(first.contains("more lines below") && first.contains("esc close") && first.contains("↑↓ scroll"), "{}", lines.join("\n"));
    // Read it all, a line at a time down to its end.
    let mut pages = vec![lines.join("\n")];
    for _ in 0..40 {
        key(&mut app, KeyCode::Down);
        pages.push(draw(&mut term, &app).join("\n"));
    }
    let ctx = pages.join("\n");
    let f = flat(&pages);
    // ...the help has them, from the same table, and the keys that work everywhere, and the two things worth knowing.
    for want in [
        "Keys",
        "On this screen",
        "enter open",
        "m project",
        "h hand-off",
        "v editor",
        "←→ active/archived",
        "↑↓ move",
        "Anywhere in platoon",
        "1-4 the tabs: Sessions, Tasks, Projects, Squads",
        "s settings",
        "/ search",
        "ctrl+space back one level (esc does the same)",
        "q quit platoon",
        "? this help",
        "Good to know",
    ] {
        assert!(f.contains(want), "{want:?} missing:\n{ctx}");
    }
    // Once each: the global keys are not repeated under "On this screen" (on a screen tall enough for all of it).
    let mut tall_term = Terminal::new(TestBackend::new(80, 60)).unwrap();
    draw(&mut tall_term, &app);
    let tall = popup_text(&tall_term, &app);
    assert!(!tall.contains("more lines below") && tall.contains("Ctrl+Z"), "{tall}");
    for once in ["s settings", "/ search", "1-4", "q quit"] {
        assert_eq!(tall.matches(once).count(), 1, "{once}: {tall}");
    }
    key(&mut app, KeyCode::Home);
    assert_eq!(app.help_top.get(), 0);
    key(&mut app, KeyCode::End);
    let end = flat(&draw(&mut term, &app));
    assert!(end.contains("hold Shift while you drag") && end.contains("Ctrl+Z does nothing on platoon's own screens"), "{end}");
    assert!(!end.contains("more lines below"), "{end}");
    // esc closes it; nothing else happened meanwhile (End didn't move the list).
    key(&mut app, KeyCode::Esc);
    assert!(!app.help_showing() && app.selected == 0);
    let after = draw(&mut term, &app);
    assert!(!flat(&after).contains("On this screen"), "{}", after.join("\n"));
}

#[test]
fn the_help_fits_at_sixty_by_fourteen_and_closes_with_question_mark_back_or_a_click_outside() {
    let (w, mut app) = three("f-help-small");
    for (cw, chh) in [(80u16, 24u16), (60, 24), (60, 14)] {
        let mut term = sized(&mut app, cw, chh);
        ch(&mut app, '?');
        let lines = draw(&mut term, &app);
        let ctx = format!("{cw}x{chh}\n{}", lines.join("\n"));
        // A bordered popup inside the screen, with its own keys at the bottom.
        let top = lines.iter().position(|l| l.contains("╭ Keys ")).expect(&ctx);
        let bottom = lines.iter().position(|l| l.contains('╰')).expect(&ctx);
        assert!(top < bottom && lines[bottom - 1].contains("esc close"), "{ctx}");
        for l in &lines {
            assert_eq!(l.width(), cw as usize, "{ctx}");
        }
        // Scrolls to the end and back.
        key(&mut app, KeyCode::End);
        assert!(flat(&draw(&mut term, &app)).contains("Ctrl+Z"), "{ctx}");
        key(&mut app, KeyCode::Home);
        assert!(flat(&draw(&mut term, &app)).contains("On this screen"), "{ctx}");
        ch(&mut app, '?');
        assert!(!app.help_showing(), "? closes it again");
    }
    let mut term = sized(&mut app, 80, 24);
    // The back key closes it.
    ch(&mut app, '?');
    app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
    assert!(!app.help_showing() && matches!(app.mode, Mode::List) && app.open_project.is_none());
    // A click inside does nothing; one outside closes it, and doesn't reach the row under it.
    ch(&mut app, '?');
    draw(&mut term, &app);
    let popup = app.hits.borrow().iter().find(|(_, h)| matches!(h, Hit::Inert)).map(|(r, _)| *r).unwrap();
    click(&mut app, popup.x + 3, popup.y + 2);
    assert!(app.help_showing());
    // The wheel scrolls it.
    app.on_mouse(MouseEvent { kind: MouseEventKind::ScrollDown, column: popup.x + 3, row: popup.y + 2, modifiers: KeyModifiers::NONE });
    assert_eq!(app.help_top.get(), 3);
    click(&mut app, 1, 5);
    assert!(!app.help_showing() && app.selected == 0);
    // The back and quit keys are the user's own when keys.json says so.
    fs::create_dir_all(&w.config).unwrap();
    crate::projects::write_atomic(&w.config.join("keys.json"), r#"{ "back": "alt+x", "quit": "w" }"#).unwrap();
    app.refresh_for_test();
    ch(&mut app, '?');
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("alt+x back one level") && f.contains("w quit platoon"), "{f}");
    // The quit key closes the help rather than quitting.
    ch(&mut app, 'w');
    assert!(!app.help_showing() && !app.quit);
}

#[test]
fn the_help_uses_the_key_color_for_its_keys() {
    let (_w, mut app) = three("f-help-color");
    let mut term = sized(&mut app, 80, 24);
    ch(&mut app, '?');
    draw(&mut term, &app);
    let t = app.effective_theme().clone();
    let buf = term.backend().buffer().clone();
    let popup = app.hits.borrow().iter().find(|(_, h)| matches!(h, Hit::Inert)).map(|(r, _)| *r).unwrap();
    // The first key row: `enter` under "On this screen", its description beside it.
    let row = (popup.y..popup.bottom())
        .find(|&y| (popup.x..popup.right()).map(|x| buf[(x, y)].symbol()).collect::<String>().contains("enter"))
        .unwrap();
    let line: String = (popup.x..popup.right()).map(|x| buf[(x, row)].symbol()).collect();
    let kx = popup.x + line.find("enter").unwrap() as u16;
    let dx = popup.x + line.find("open").unwrap() as u16;
    assert_eq!((buf[(kx, row)].fg, buf[(kx, row)].bg), (t.accent, t.bg), "the key in the key color (the footer's)");
    assert_eq!(buf[(dx, row)].fg, t.subtext, "the description quieter");
    // The border is the popups' accent border, the heading bold text.
    assert_eq!(buf[(popup.x, popup.y)].fg, t.accent);
    let head = (popup.y..popup.bottom())
        .find(|&y| (popup.x..popup.right()).map(|x| buf[(x, y)].symbol()).collect::<String>().contains("On this screen"))
        .unwrap();
    let hl: String = (popup.x..popup.right()).map(|x| buf[(x, head)].symbol()).collect();
    let hx = popup.x + hl.find("On this").unwrap() as u16;
    assert_eq!(buf[(hx, head)].fg, t.text);
    assert!(buf[(hx, head)].modifier.contains(ratatui::style::Modifier::BOLD));
}

#[test]
fn question_mark_is_only_help_on_platoons_own_screens() {
    let (_w, mut app) = three("f-help-where");
    // Typed into the search box, it is a character.
    ch(&mut app, '/');
    ch(&mut app, '?');
    assert!(!app.help_showing() && matches!(app.mode, Mode::Search) && app.search.text == "?");
    key(&mut app, KeyCode::Esc);
    // In the attached view it goes to the session.
    let (client, host) = Client::for_test(23, 80);
    host.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    app.client = Some(client);
    app.mode = Mode::Attached(4242);
    ch(&mut app, '?');
    assert!(!app.help_showing());
    let mut r = &host;
    assert!(matches!(read_frame(&mut r), Ok((_, b)) if b == b"?"), "sent to the session");
    // On the project screen and the Squads tab it is the help, with that screen's keys.
    app.client = None;
    app.mode = Mode::List;
    ch(&mut app, '4');
    ch(&mut app, '?');
    let mut term = sized(&mut app, 100, 30);
    let f = flat(&draw(&mut term, &app));
    assert!(app.help_showing() && f.contains("n describe a new squad or agent"), "{f}");
    // Not `/ search` there: it doesn't search on the Squads tab.
    assert!(!f.contains("/ search"), "{f}");
    key(&mut app, KeyCode::Esc);
}

// ---- the attached view ---------------------------------------------------------------------------------------------

/// The resize messages the client has sent to its host so far, as (rows, cols).
fn resizes(host: &UnixStream) -> Vec<(u16, u16)> {
    let mut out = Vec::new();
    let mut r = host;
    while let Ok((kind, p)) = read_frame(&mut r) {
        if kind == F_RESIZE && p.len() == 4 {
            out.push((u16::from_be_bytes([p[0], p[1]]), u16::from_be_bytes([p[2], p[3]])));
        }
    }
    out
}

#[test]
fn the_attached_view_gives_the_session_the_whole_width_and_every_row_but_the_bar() {
    let (_w, mut app) = three("f-attached");
    let (client, host) = Client::for_test(23, 80);
    host.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    app.client = Some(client);
    app.mode = Mode::Attached(4242);
    for (w, h) in [(80u16, 24u16), (60, 24), (60, 14)] {
        let mut term = sized(&mut app, w, h);
        assert_eq!(app.term_area(), Rect::new(0, 0, w, h - 1), "{w}x{h}");
        let lines = draw(&mut term, &app);
        assert!(lines[h as usize - 1].contains("back to list"), "the bar is the last row: {}", lines.join("\n"));
        assert!(!lines.iter().take(h as usize - 1).any(|l| l.contains(['╭', '╰'])), "no frame around the session");
    }
    // Every size change reached the session, at the full width.
    assert_eq!(resizes(&host), [(23, 60), (13, 60)]);
    let mut term = sized(&mut app, 80, 24);
    assert_eq!(resizes(&host), [(23, 80)]);
    // A mark in the top left and the bottom right corner of the session's screen.
    app.client.as_ref().unwrap().parser.lock().unwrap().process(b"\x1b[1;1HA\x1b[23;80HZ");
    let lines = draw(&mut term, &app);
    assert!(lines[0].starts_with('A') && lines[22].ends_with('Z'), "the session's corners are the terminal's:\n{}", lines.join("\n"));
}

// ---- below the smallest size --------------------------------------------------------------------------------------

#[test]
fn below_sixty_by_fourteen_only_a_message_shows_and_resizing_back_brings_the_screen_back() {
    let (_w, mut app) = three("f-small");
    assert_eq!((MIN_WIDTH, MIN_HEIGHT), (60, 14));
    for (w, h) in [(59u16, 14u16), (60, 13), (20, 6)] {
        let mut term = sized(&mut app, w, h);
        assert!(app.too_small());
        let lines = draw(&mut term, &app);
        let ctx = format!("{w}x{h}\n{}", lines.join("\n"));
        let f = flat(&lines);
        assert!(f.contains("platoon needs at least 60×14") && f.contains(&format!("this window is {w}×{h}")), "{ctx}");
        assert!(f.contains("Make the window bigger, or press q to quit."), "{ctx}");
        assert!(!f.contains("1 Sessions") && !f.contains("alpha") && !f.contains("enter open"), "nothing else: {ctx}");
        for l in &lines {
            assert_eq!(l.width(), w as usize, "{ctx}");
        }
        // Centered: as many blank rows above it as below (or one fewer), and every line centered in the width.
        let text: Vec<usize> = (0..lines.len()).filter(|&y| !lines[y].trim().is_empty()).collect();
        let (above, below) = (text[0], lines.len() - 1 - text[text.len() - 1]);
        assert!(above.abs_diff(below) <= 1, "{ctx}");
        for &y in &text {
            let l = &lines[y];
            let (lead, trail) = (l.len() - l.trim_start().len(), l.chars().rev().take_while(|c| *c == ' ').count());
            assert!(lead.abs_diff(trail) <= 1, "line {y} is centered: {ctx}");
        }
    }
    // The exact lines at 59x14.
    let mut term = sized(&mut app, 59, 14);
    let lines = draw(&mut term, &app);
    assert_eq!(lines[5], "    platoon needs at least 60×14 — this window is 59×14    ");
    assert_eq!(lines[7], "        Make the window bigger, or press q to quit.        ");
}

#[test]
fn while_too_small_no_key_or_click_acts_but_quit() {
    let (_w, mut app) = three("f-small-keys");
    let mut term = sized(&mut app, 59, 14);
    draw(&mut term, &app);
    for c in ['n', '2', '3', 's', '/', '?', 'a', 'm', 'v'] {
        ch(&mut app, c);
    }
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Right);
    app.on_paste("pasted");
    click(&mut app, 10, 3);
    assert!(
        matches!(app.mode, Mode::List) && app.top == 0 && app.tab == 0 && app.selected == 0 && !app.help_showing() && app.filter.is_empty()
    );
    assert!(!app.quit);
    // Big enough again: the screen is back as it was, and the keys act again.
    let mut term = sized(&mut app, 60, 14);
    assert!(flat(&draw(&mut term, &app)).contains("1 Sessions"));
    key(&mut app, KeyCode::Down);
    assert_eq!(app.selected, 1);
    // Too small again: q quits (and ctrl+c does too).
    sized(&mut app, 60, 13);
    ch(&mut app, 'q');
    assert!(app.quit);
    let (_w, mut app) = three("f-small-ctrl-c");
    sized(&mut app, 30, 10);
    app.mode = Mode::Settings;
    app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(app.quit, "from any of platoon's own screens");
}

#[test]
fn while_too_small_a_session_hears_nothing_keeps_its_size_and_says_how_to_see_it_again() {
    let (_w, mut app) = three("f-small-attached");
    let (client, host) = Client::for_test(23, 80);
    host.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    app.client = Some(client);
    app.mode = Mode::Attached(4242);
    let mut term = sized(&mut app, 40, 12);
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("this window is 40×12") && f.contains("Make the window bigger to see it again."), "{f}");
    assert!(!f.contains("press q"), "q is the session's here: {f}");
    // Nothing typed reaches the session (it can't be seen), quit included, and nothing quits.
    for c in ['x', 'q'] {
        ch(&mut app, c);
    }
    app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    app.on_paste("pasted");
    assert!(!app.quit && matches!(app.mode, Mode::Attached(_)));
    // The session keeps the size it had: it isn't squeezed to 40x11 (and back) while nobody can see it.
    let mut r = &host;
    assert!(read_frame(&mut r).is_err(), "nothing was sent: no keys, no resize");
    // Big again: its new size, and the keys go to it again.
    sized(&mut app, 70, 20);
    ch(&mut app, 'y');
    let mut frames = Vec::new();
    while let Ok(fr) = read_frame(&mut r) {
        frames.push(fr);
    }
    assert_eq!(frames.len(), 2, "{frames:?}");
    assert_eq!(frames[0], (F_RESIZE, crate::host::size_payload(19, 70).to_vec()));
    assert_eq!(frames[1].1, b"y");
}
