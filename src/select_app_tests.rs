//! Selecting text with the mouse in the attached view, at the level of the app: real mouse events in, the
//! drawn frame and what reached the (fake) clipboard out. Nothing here can reach the real clipboard or the
//! real terminal: `App::with_dirs` has no clipboard, and each test gives it a recording one.

use crate::app::{Action, App, Hit, Mode};
use crate::client::Client;
use crate::clipboard::fake::{Fake, Record, osc52_texts};
use crate::clipboard::{Clipboard, Target};
use crate::select::{Point, Unit};
use crate::testkit::World;
use crate::theme::Theme;
use crate::ui;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
    style::{Color, Modifier},
};
use std::{
    io::Read,
    os::unix::net::UnixStream,
    sync::{Arc, Mutex},
    time::Duration,
};

/// The screen the tests share: 80x24, so the session's screen is 23 rows by the whole 80 columns at (0, 0) (there
/// is no frame around the app), and the status bar is the row below it.
const ROWS: u16 = 23;
const COLS: u16 = 80;

struct Rig {
    _w: World,
    app: App,
    /// The host end of the connection: what the client sends to the session arrives here.
    host: UnixStream,
    rec: Arc<Mutex<Record>>,
    term: Terminal<TestBackend>,
}

fn rig_with(text: &str, target: Target, tweak: impl FnOnce(&mut Fake)) -> Rig {
    let w = World::new("select");
    let mut app = w.app();
    let (client, host) = Client::for_test(ROWS, COLS);
    host.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    client.parser.lock().unwrap().process(text.replace('\n', "\r\n").as_bytes());
    app.client = Some(client);
    app.mode = Mode::Attached(4242);
    let (mut fake, rec) = Fake::new();
    tweak(&mut fake);
    app.set_clipboard_for_test(Clipboard::new(target, Box::new(fake)));
    let mut r = Rig { _w: w, app, host, rec, term: Terminal::new(TestBackend::new(80, 24)).unwrap() };
    r.draw();
    r
}

fn rig(text: &str) -> Rig {
    rig_with(text, Target::System, |_| {})
}

impl Rig {
    fn draw(&mut self) -> String {
        let app = &self.app;
        self.term.draw(|f| ui::draw(f, app)).unwrap();
        let buf = self.term.backend().buffer();
        (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
    }

    fn mouse(&mut self, kind: MouseEventKind, x: u16, y: u16) {
        self.app.on_mouse(MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE });
    }

    /// Screen coordinates of a cell of the session's screen.
    fn cell(row: u16, col: u16) -> (u16, u16) {
        (col, row)
    }

    fn press(&mut self, row: u16, col: u16) {
        let (x, y) = Self::cell(row, col);
        self.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    }

    fn drag(&mut self, row: u16, col: u16) {
        let (x, y) = Self::cell(row, col);
        self.mouse(MouseEventKind::Drag(MouseButton::Left), x, y);
    }

    fn release(&mut self, row: u16, col: u16) {
        let (x, y) = Self::cell(row, col);
        self.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    }

    /// Press, drag and release: what selecting `from` to `to` looks like.
    fn select(&mut self, from: (u16, u16), to: (u16, u16)) {
        self.press(from.0, from.1);
        self.drag(to.0, to.1);
        self.release(to.0, to.1);
    }

    fn click(&mut self, row: u16, col: u16) {
        self.press(row, col);
        self.release(row, col);
    }

    fn system(&self) -> Vec<String> {
        self.rec.lock().unwrap().system.clone()
    }

    fn osc52(&self) -> Vec<String> {
        osc52_texts(&self.rec.lock().unwrap().terminal)
    }

    fn key(&mut self, code: KeyCode) {
        self.app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    /// What the client has sent to the session so far.
    fn sent(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut buf = [0u8; 256];
        while let Ok(n) = self.host.read(&mut buf) {
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        out
    }

    fn notice(&self) -> Option<String> {
        self.app.active_notice().map(str::to_string)
    }

    /// The cells the selection covers now.
    fn span(&self) -> Option<crate::select::Span> {
        let parser = self.app.client.as_ref().unwrap().parser.lock().unwrap();
        self.app.selection.and_then(|s| s.span(parser.screen()))
    }

    fn buffer(&self) -> &ratatui::buffer::Buffer {
        self.term.backend().buffer()
    }
}

fn bg_of(r: &Rig, row: u16, col: u16) -> Color {
    r.buffer()[Rig::cell(row, col)].bg
}

const TEXT: &str = "hello brave world\nsecond line here\nthird";

// --- the basics ---------------------------------------------------------------------------------------

#[test]
fn dragging_highlights_and_releasing_copies_exactly_once_with_the_selected_text() {
    let mut r = rig(TEXT);
    r.press(0, 6);
    r.drag(0, 8);
    r.drag(0, 10);
    assert!(r.system().is_empty(), "nothing is copied while the button is down");
    let sel = r.app.selection.expect("a selection is showing");
    assert_eq!((sel.anchor, sel.head, sel.unit), (Point::new(0, 6), Point::new(0, 10), Unit::Char));
    let shown = r.draw();
    let theme = r.app.effective_theme().clone();
    let selection_bg = theme.selection_bg().unwrap();
    for col in 6..=10 {
        assert_eq!(bg_of(&r, 0, col), selection_bg, "col {col} is highlighted while dragging");
    }
    assert_eq!((bg_of(&r, 0, 5), bg_of(&r, 0, 11)), (theme.bg, theme.bg), "and only those");
    assert!(shown.contains("hello brave world"), "the text is still there");

    r.release(0, 10);
    assert_eq!(r.system(), ["brave"], "the system clipboard got it once");
    assert_eq!(r.osc52(), ["brave"], "and so did the terminal, as OSC 52");
    assert_eq!(r.notice().as_deref(), Some("Copied 5 characters"));
    assert!(r.draw().contains("Copied 5 characters"), "the status bar says so");
    assert!(r.app.selection.is_some(), "the highlight stays after the copy");
    assert_eq!(bg_of(&r, 0, 8), selection_bg);

    // Nothing more happens on further events: no second copy from a stray release or from moving about.
    r.release(0, 10);
    r.mouse(MouseEventKind::Moved, 30, 5);
    r.mouse(MouseEventKind::Drag(MouseButton::Left), 30, 5);
    assert_eq!(r.system(), ["brave"]);
    assert_eq!(r.osc52(), ["brave"]);
}

#[test]
fn a_multi_line_selection_reports_lines_and_copies_the_text_with_its_breaks() {
    let mut r = rig(TEXT);
    r.select((0, 6), (2, 4));
    assert_eq!(r.system(), ["brave world\nsecond line here\nthird"]);
    assert_eq!(r.notice().as_deref(), Some("Copied 3 lines"));
}

#[test]
fn selecting_backwards_copies_the_same_text() {
    let mut r = rig(TEXT);
    r.select((1, 10), (0, 6));
    assert_eq!(r.system(), ["brave world\nsecond line"]);
}

#[test]
fn a_plain_click_copies_nothing_and_clears_an_earlier_selection() {
    let mut r = rig(TEXT);
    r.select((0, 0), (0, 4));
    assert!(r.app.selection.is_some());
    r.press(1, 3);
    assert_eq!(r.span(), None, "a press alone shows no highlight");
    r.draw();
    assert_eq!(bg_of(&r, 0, 2), r.app.effective_theme().bg, "the old highlight is gone at once");
    r.release(1, 3);
    assert!(r.app.selection.is_none(), "a click leaves nothing selected");
    assert_eq!(r.system(), ["hello"], "only the first drag copied");
    assert_eq!(r.osc52(), ["hello"]);
}

#[test]
fn a_selection_of_only_blanks_copies_nothing_and_goes_away() {
    let mut r = rig(TEXT);
    r.select((5, 0), (9, 30));
    assert!(r.system().is_empty() && r.osc52().is_empty());
    assert!(r.app.selection.is_none());
    assert_eq!(r.notice(), None);
}

#[test]
fn a_drag_that_ends_where_it_started_is_a_click() {
    let mut r = rig(TEXT);
    r.press(0, 3);
    r.drag(0, 6);
    r.drag(0, 3);
    r.release(0, 3);
    assert!(r.system().is_empty());
    assert!(r.app.selection.is_none());
}

#[test]
fn other_buttons_do_not_select_and_do_not_disturb_a_selection() {
    let mut r = rig(TEXT);
    r.select((0, 0), (0, 4));
    for button in [MouseButton::Right, MouseButton::Middle] {
        let (x, y) = Rig::cell(0, 8);
        r.mouse(MouseEventKind::Down(button), x, y);
        r.mouse(MouseEventKind::Drag(button), x + 3, y);
        r.mouse(MouseEventKind::Up(button), x + 3, y);
    }
    assert_eq!(r.system(), ["hello"]);
    assert!(r.app.selection.is_some());
}

// --- where it applies ---------------------------------------------------------------------------------

#[test]
fn the_back_hint_still_goes_back_and_leaves_no_selection_behind() {
    let mut r = rig(TEXT);
    r.select((0, 0), (0, 4));
    r.draw();
    let hint =
        r.app.hits.borrow().iter().find(|(_, h)| matches!(h, Hit::Action(Action::Back))).map(|(rect, _)| *rect).expect("the hint is drawn");
    r.mouse(MouseEventKind::Down(MouseButton::Left), hint.x + 1, hint.y);
    assert!(matches!(r.app.mode, Mode::List), "clicked the hint");
    assert!(r.app.selection.is_none() && r.app.client.is_none());
    r.mouse(MouseEventKind::Up(MouseButton::Left), hint.x + 1, hint.y);
    assert_eq!(r.system(), ["hello"], "and the release copied nothing more");
}

#[test]
fn a_press_on_the_status_bar_does_not_start_a_selection() {
    let mut r = rig(TEXT);
    let bar = 23; // the row below the session's screen: the last row
    for (x, y) in [(40, bar), (0, bar), (79, bar), (5, bar)] {
        r.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
        assert!(r.app.selection.is_none(), "({x},{y})");
        r.mouse(MouseEventKind::Drag(MouseButton::Left), x + 2, y);
        r.mouse(MouseEventKind::Up(MouseButton::Left), x + 2, y);
    }
    assert!(r.system().is_empty());
}

#[test]
fn it_only_works_in_the_attached_view() {
    let mut r = rig(TEXT);
    r.app.mode = Mode::List;
    r.press(0, 0);
    r.drag(0, 8);
    r.release(0, 8);
    assert!(r.app.selection.is_none() && r.system().is_empty());
}

#[test]
fn dragging_past_an_edge_sticks_to_that_edge() {
    let mut r = rig(TEXT);
    r.press(1, 7);
    r.mouse(MouseEventKind::Drag(MouseButton::Left), 500, 300);
    assert_eq!(r.app.selection.unwrap().head, Point::new(ROWS - 1, COLS - 1), "bottom right");
    r.mouse(MouseEventKind::Drag(MouseButton::Left), 0, 0);
    assert_eq!(r.app.selection.unwrap().head, Point::new(0, 0), "top left");
    r.mouse(MouseEventKind::Drag(MouseButton::Left), 40, 23);
    assert_eq!(r.app.selection.unwrap().head, Point::new(ROWS - 1, 40), "onto the status bar: the last row of the screen");
}

#[test]
fn dragging_out_of_the_screen_and_releasing_there_still_copies_up_to_the_edge() {
    let mut r = rig(TEXT);
    r.press(1, 7);
    r.mouse(MouseEventKind::Drag(MouseButton::Left), 500, 300);
    r.mouse(MouseEventKind::Up(MouseButton::Left), 500, 300);
    assert_eq!(r.system(), ["line here\nthird"]);
}

// --- the selection's lifetime -------------------------------------------------------------------------

fn selected_rig() -> Rig {
    let mut r = rig(TEXT);
    r.select((0, 6), (0, 10));
    assert!(r.app.selection.is_some());
    r
}

#[test]
fn a_key_forwarded_to_the_session_clears_the_selection() {
    let mut r = selected_rig();
    r.key(KeyCode::Char('x'));
    assert!(r.app.selection.is_none());
    assert!(r.sent().ends_with(b"x"), "and it went to the session");
    r.draw();
    assert_eq!(bg_of(&r, 0, 8), r.app.effective_theme().bg);
}

#[test]
fn pasted_text_clears_the_selection() {
    let mut r = selected_rig();
    r.app.on_paste("pasted");
    assert!(r.app.selection.is_none());
    assert!(String::from_utf8_lossy(&r.sent()).contains("pasted"));
}

#[test]
fn a_resize_clears_the_selection_but_the_same_size_does_not() {
    let mut r = selected_rig();
    r.app.sync_size(80, 24);
    assert!(r.app.selection.is_some(), "not a change");
    r.app.sync_size(100, 30);
    assert!(r.app.selection.is_none());
}

#[test]
fn leaving_the_attached_view_clears_it_and_coming_back_starts_clean() {
    let mut r = selected_rig();
    r.app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
    assert!(matches!(r.app.mode, Mode::List) && r.app.selection.is_none());
}

#[test]
fn the_wheel_scrolls_and_drops_the_selection() {
    let mut r = rig(&(0..60).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n"));
    r.select((0, 0), (0, 5));
    assert!(r.app.selection.is_some());
    let before = r.app.client.as_ref().unwrap().parser.lock().unwrap().screen().scrollback();
    r.mouse(MouseEventKind::ScrollUp, 10, 10);
    let after = r.app.client.as_ref().unwrap().parser.lock().unwrap().screen().scrollback();
    assert_eq!((before, after), (0, 3), "the wheel still scrolls the session");
    assert!(r.app.selection.is_none(), "and what was selected is no longer where it was");
    r.mouse(MouseEventKind::ScrollDown, 10, 10);
    assert_eq!(r.app.client.as_ref().unwrap().parser.lock().unwrap().screen().scrollback(), 0);
}

#[test]
fn a_wheel_turn_in_the_middle_of_a_drag_ends_it() {
    let mut r = rig(TEXT);
    r.press(0, 6);
    r.drag(0, 8);
    r.mouse(MouseEventKind::ScrollUp, 10, 10);
    r.drag(0, 10);
    r.release(0, 10);
    assert!(r.system().is_empty() && r.app.selection.is_none());
}

#[test]
fn selecting_while_scrolled_back_copies_what_is_on_screen() {
    let mut r = rig(&(0..60).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n"));
    r.mouse(MouseEventKind::ScrollUp, 10, 10);
    r.mouse(MouseEventKind::ScrollUp, 10, 10);
    let rows: Vec<String> = r.app.client.as_ref().unwrap().parser.lock().unwrap().screen().rows(0, COLS).collect();
    assert_eq!((rows[0].as_str(), rows[1].as_str()), ("line 31", "line 32"), "six rows further back");
    r.select((0, 0), (1, 20));
    assert_eq!(r.system(), ["line 31\nline 32"]);
    // The highlight is on the same cells.
    r.draw();
    let sel = r.app.effective_theme().selection_bg().unwrap();
    assert_eq!((bg_of(&r, 0, 3), bg_of(&r, 1, 20)), (sel, sel));
    assert_eq!(bg_of(&r, 2, 3), r.app.effective_theme().bg);
}

#[test]
fn a_session_that_is_paused_cannot_be_selected() {
    let mut w = World::new("select-paused");
    w.transcript("aaa", "/tmp", "a prompt", 5);
    let pid = w.live("aaa", "/tmp", 1, "idle");
    crate::guard::mark_stale(&w.cache, pid, 1);
    let mut app = w.app();
    assert!(app.sessions.iter().any(|s| s.procs.iter().any(|p| p.pid == pid)));
    let (client, _host) = Client::for_test(ROWS, COLS);
    client.parser.lock().unwrap().process(b"hello brave world");
    app.client = Some(client);
    app.mode = Mode::Attached(pid);
    let (fake, rec) = Fake::new();
    app.set_clipboard_for_test(Clipboard::new(Target::System, Box::new(fake)));
    assert!(app.attached_paused(), "the fixture is paused");
    let ev = |kind, x, y| MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE };
    app.on_mouse(ev(MouseEventKind::Down(MouseButton::Left), 2, 1));
    app.on_mouse(ev(MouseEventKind::Drag(MouseButton::Left), 9, 1));
    app.on_mouse(ev(MouseEventKind::Up(MouseButton::Left), 9, 1));
    assert!(app.selection.is_none() && rec.lock().unwrap().system.is_empty());
}

// --- double and triple clicks -------------------------------------------------------------------------

#[test]
fn a_double_click_selects_and_copies_the_word_and_a_triple_click_the_line() {
    let mut r = rig("see https://ex.com/a_b?x=1 now\nnext line");
    r.click(0, 10);
    assert!(r.system().is_empty(), "one click copies nothing");
    r.click(0, 10);
    assert_eq!(r.system(), ["https://ex.com/a_b?x=1"]);
    assert_eq!(r.app.selection.unwrap().unit, Unit::Word);
    r.draw();
    assert_eq!(bg_of(&r, 0, 4), r.app.effective_theme().selection_bg().unwrap(), "the word is highlighted");
    assert_eq!(bg_of(&r, 0, 0), r.app.effective_theme().bg);
    r.click(0, 10);
    assert_eq!(r.system(), ["https://ex.com/a_b?x=1", "see https://ex.com/a_b?x=1 now"]);
    assert_eq!(r.notice().as_deref(), Some("Copied 30 characters"));
    // A fourth click starts over: a plain click, so the selection goes.
    r.click(0, 10);
    assert!(r.app.selection.is_none());
    assert_eq!(r.system().len(), 2);
}

#[test]
fn a_double_click_can_be_dragged_out_to_more_words() {
    let mut r = rig("one two three four");
    r.click(0, 5);
    r.press(0, 5);
    r.drag(0, 10);
    r.release(0, 10);
    assert_eq!(r.system().last().unwrap(), "two three");
}

#[test]
fn two_clicks_too_far_apart_in_time_are_two_single_clicks() {
    let mut r = rig("one two three");
    r.click(0, 5);
    std::thread::sleep(crate::select::MULTI_CLICK + Duration::from_millis(80));
    r.click(0, 5);
    assert!(r.system().is_empty() && r.app.selection.is_none());
}

#[test]
fn two_clicks_in_different_places_are_two_single_clicks() {
    let mut r = rig("one two three");
    r.click(0, 1);
    r.click(0, 9);
    assert!(r.system().is_empty() && r.app.selection.is_none());
}

// --- where the text goes ------------------------------------------------------------------------------

#[test]
fn osc52_mode_sends_only_the_terminal_sequence() {
    let mut r = rig_with(TEXT, Target::Osc52, |_| {});
    r.select((0, 6), (0, 10));
    assert!(r.system().is_empty());
    assert_eq!(r.osc52(), ["brave"]);
    assert_eq!(r.notice().as_deref(), Some("Copied 5 characters"));
}

#[test]
fn with_the_clipboard_off_it_still_highlights_and_says_what_was_selected() {
    let mut r = rig_with(TEXT, Target::Off, |_| {});
    r.select((0, 6), (0, 10));
    assert!(r.system().is_empty() && r.rec.lock().unwrap().terminal.is_empty());
    assert_eq!(r.notice().as_deref(), Some("Selected 5 characters (clipboard off)"));
    r.draw();
    assert_eq!(bg_of(&r, 0, 8), r.app.effective_theme().selection_bg().unwrap());
}

#[test]
fn when_every_route_fails_the_status_bar_points_to_shift_drag() {
    let mut r = rig_with(TEXT, Target::System, |f| {
        f.system_error = Some("no display".into());
        f.terminal_error = true;
    });
    r.select((0, 6), (0, 10));
    assert_eq!(r.notice().as_deref(), Some("Copy failed: hold Shift and drag to select natively"));
    let bar = r.draw().lines().nth(23).unwrap().to_string();
    assert!(bar.contains("Copy failed: hold Shift and drag"), "{bar}");
}

#[test]
fn when_only_the_system_clipboard_fails_the_terminal_still_gets_it() {
    let mut r = rig_with(TEXT, Target::System, |f| f.system_error = Some("no display".into()));
    r.select((0, 6), (0, 10));
    assert_eq!(r.osc52(), ["brave"]);
    assert_eq!(r.notice().as_deref(), Some("Copied 5 characters (terminal only)"));
}

// --- what it looks like -------------------------------------------------------------------------------

#[test]
fn the_highlight_keeps_the_sessions_text_color_where_it_reads_and_changes_it_where_it_would_not() {
    let mut r = rig("\x1b[38;2;255;120;120mred\x1b[0m \x1b[38;2;40;40;60mdark\x1b[0m plain");
    r.select((0, 0), (0, 14));
    r.draw();
    let theme = r.app.effective_theme().clone();
    let sel = theme.selection_bg().unwrap();
    let cell = |col: u16| r.buffer()[Rig::cell(0, col)].clone();
    assert_eq!((cell(0).fg, cell(0).bg), (Color::Rgb(255, 120, 120), sel), "readable: kept");
    // Would vanish: moved toward the theme's text just until it reads (the rule for text on platoon's own selection).
    let dark = Color::Rgb(40, 40, 60);
    assert_eq!((cell(4).fg, cell(4).bg), (theme.readable_on(dark, sel, 3.0), sel), "would vanish: lifted");
    assert!(cell(4).fg != dark && crate::theme::contrast(cell(4).fg, sel) >= 3.0);
    assert_eq!((cell(10).fg, cell(10).bg), (theme.text, sel), "default text is the theme's text color anyway");
}

#[test]
fn terminal_colors_has_no_colors_to_mix_so_selected_cells_are_reversed_and_inverse_ones_toggle() {
    let mut r = rig("plain \x1b[7minverse\x1b[0m");
    r.app.preview = Some(Theme::by_id("terminal").unwrap());
    r.select((0, 0), (0, 12));
    r.draw();
    let mods = |col: u16| r.buffer()[Rig::cell(0, col)].modifier;
    assert!(mods(1).contains(Modifier::REVERSED), "selected plain text is reversed");
    assert!(!mods(8).contains(Modifier::REVERSED), "selected inverse text is the other way round");
    assert!(!mods(14).contains(Modifier::REVERSED), "unselected cells are untouched");
    assert_eq!(r.buffer()[Rig::cell(0, 1)].bg, Color::Reset, "no color is invented");
}

#[test]
fn the_highlight_survives_the_256_color_pass() {
    use crate::colordepth::Depth;
    let mut r = rig(TEXT);
    r.select((0, 6), (0, 10));
    let app = &r.app;
    r.term
        .draw(|f| {
            ui::draw(f, app);
            Depth::Indexed256.apply(f.buffer_mut());
        })
        .unwrap();
    let theme = r.app.effective_theme().clone();
    let indexed = |c: Color| Depth::Indexed256.color(c);
    assert!(matches!(r.buffer()[Rig::cell(0, 8)].bg, Color::Indexed(_)));
    assert_eq!(r.buffer()[Rig::cell(0, 8)].bg, indexed(theme.selection_bg().unwrap()));
    assert_ne!(r.buffer()[Rig::cell(0, 8)].bg, r.buffer()[Rig::cell(0, 2)].bg, "still different from the background after quantizing");
}

#[test]
fn while_a_theme_is_previewed_the_selection_is_drawn_in_that_theme() {
    let mut r = rig(TEXT);
    r.select((0, 6), (0, 10));
    let nord = Theme::by_id("nord").unwrap();
    r.app.preview = Some(nord.clone());
    r.draw();
    assert_eq!(bg_of(&r, 0, 8), nord.selection_bg().unwrap());
}
