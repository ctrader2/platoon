//! Tickets at the level of the app: the Tickets pane of a project's screen and moving between the three panes, making
//! a ticket (the name box, then the editor list and a stand-in editor), the Done and cancelled group, the lists that
//! change a ticket, its own screen, and the pane's columns on a narrow terminal. Every directory is a temp dir
//! (`World`); the only processes started are the tests' own stand-in editors.

use crate::app::{App, Hit, Mode, Pane, PickKind, TRow};
use crate::editors::Editor;
use crate::testkit::World;
use crate::tickets::{self, HowFar, NewTicket, State, Tickets};
use crate::ui;
use ratatui::{
    backend::TestBackend,
    buffer::Buffer,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
    Terminal,
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    thread,
    time::{Duration, Instant},
};

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn typed(app: &mut App, text: &str) {
    for c in text.chars() {
        key(app, KeyCode::Char(c));
    }
}

fn draw(term: &mut Terminal<TestBackend>, app: &App) -> String {
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

fn buffer(term: &Terminal<TestBackend>) -> Buffer {
    term.backend().buffer().clone()
}

/// Where `needle` starts on the drawn screen (row, column in cells).
fn find(term: &Terminal<TestBackend>, needle: &str) -> Option<(u16, u16)> {
    let buf = term.backend().buffer();
    for y in 0..buf.area.height {
        let cells: Vec<String> = (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
        let line: String = cells.concat();
        if let Some(at) = line.find(needle) {
            let col = line[..at].chars().count();
            return Some((y, col as u16));
        }
    }
    None
}

fn flat(text: &str) -> String {
    text.replace(['│', '╭', '╮', '╰', '╯', '─'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A stand-in editor: a script in the world's own folder. `body` is the shell it runs (`$1` is the file).
fn fake_editor(w: &World, name: &str, terminal: bool, body: &str) -> Editor {
    let path = w.base.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    Editor { id: name.into(), name: name.into(), program: path.to_string_lossy().into_owned(), args: Vec::new(), terminal }
}

/// A project "Web shop" with one session in `/tmp`, its screen open. Returns the app and the project's id.
fn on_the_project(name: &str) -> (World, App, String) {
    let w = World::new(name);
    w.transcript("aaa-1", "/tmp", "a conversation", 10);
    let mut app = w.app();
    let id = app.projects.create("Web shop").unwrap();
    app.projects.assign("aaa-1", Some(&id)).unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Enter);
    assert!(app.open_project().is_some(), "the project's screen is open");
    (w, app, id)
}

/// Add tickets behind the app's back (as `platoon ticket add` would) and let it see them.
fn add(w: &World, app: &mut App, id: &str, title: &str, blocked_by: &[u32]) -> u32 {
    let n = Tickets::open(&w.config, id).add(NewTicket { title: title.into(), blocked_by: blocked_by.to_vec(), ..Default::default() }).unwrap();
    app.reload_for_test();
    n
}

fn set_state(w: &World, app: &mut App, id: &str, n: u32, s: State) {
    Tickets::open(&w.config, id).set_state(n, &s).unwrap();
    app.reload_for_test();
}

#[test]
fn the_project_screen_has_three_panes_and_tab_left_and_right_move_between_them() {
    let (_w, mut app, _id) = on_the_project("t-panes");
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    let shown = draw(&mut term, &app);
    let (s, tk, n) = (find(&term, "╭ Sessions ").unwrap(), find(&term, "╭ Tickets ").unwrap(), find(&term, "╭ Notes ").unwrap());
    assert!(s.1 == tk.1 && tk.0 > s.0, "Tickets under Sessions, on the left: {shown}");
    assert!(n.0 == s.0 && n.1 > s.1 + 20, "Notes on the right: {shown}");
    assert!(flat(&shown).contains("No tickets yet · n to make one"), "{shown}");
    assert_eq!(app.pane, Pane::Sessions);
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.pane, Pane::Tickets);
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.pane, Pane::Notes);
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.pane, Pane::Sessions, "round again");
    app.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(app.pane, Pane::Notes);
    // Left goes back to the one of the left column used last.
    key(&mut app, KeyCode::Left);
    assert_eq!(app.pane, Pane::Sessions);
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Right);
    assert_eq!(app.pane, Pane::Notes);
    key(&mut app, KeyCode::Left);
    assert_eq!(app.pane, Pane::Tickets, "the Tickets pane was the last of the left column");
    // The focused pane's border is the accent, the others' not.
    draw(&mut term, &app);
    let t = app.effective_theme().clone();
    let buf = buffer(&term);
    let tk = find(&term, "╭ Tickets ").unwrap();
    let s = find(&term, "╭ Sessions ").unwrap();
    assert_eq!(buf[(tk.1 + 3, tk.0)].fg, t.accent);
    assert_ne!(buf[(s.1 + 3, s.0)].fg, t.accent);
    // Clicking a pane focuses it.
    let r = app.hits.borrow().iter().find(|(_, h)| matches!(h, Hit::Pane(Pane::Sessions))).map(|(r, _)| *r).unwrap();
    app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: r.x + 1, row: r.y + r.height - 1, modifiers: KeyModifiers::NONE });
    assert_eq!(app.pane, Pane::Sessions);
}

#[test]
fn the_sessions_and_notes_keys_still_work_on_the_project_screen() {
    let (_w, mut app, _id) = on_the_project("t-oldkeys");
    // `n` in the Sessions pane (and in the Notes pane) is still a new session.
    key(&mut app, KeyCode::Char('n'));
    assert!(matches!(app.mode, Mode::Picker), "the folder picker for a new session");
    assert!(app.picker.ticket.is_none());
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Char('n'));
    assert!(matches!(app.mode, Mode::Picker));
    key(&mut app, KeyCode::Esc);
    // `e` on the notes is still the editor list for the notes.
    key(&mut app, KeyCode::Char('e'));
    assert!(matches!(app.mode, Mode::PickEditor) && app.epick.ticket.is_none() && app.epick.instructions.is_none());
    key(&mut app, KeyCode::Esc);
    // `s` outside the Tickets pane is still the settings.
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Char('s'));
    assert!(matches!(app.mode, Mode::Settings));
    key(&mut app, KeyCode::Esc);
    // `a` in the Sessions pane still archives the selected session.
    key(&mut app, KeyCode::Char('a'));
    assert!(app.sessions.iter().any(|s| s.id == "aaa-1" && s.archived), "archived");
}

#[test]
fn making_a_ticket_takes_a_title_then_opens_the_body_in_the_editor() {
    let (w, mut app, id) = on_the_project("t-new");
    let ed = fake_editor(&w, "fake-nano", true, r#"printf 'Login works with a magic link.\n' >> "$1""#);
    app.set_editors_for_test(vec![ed]);
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.pane, Pane::Tickets);
    // Esc in the name box makes nothing.
    key(&mut app, KeyCode::Char('n'));
    assert!(matches!(app.mode, Mode::ProjectName));
    typed(&mut app, "Nothing");
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List) && app.tickets().is_empty());
    assert!(!tickets::path_in(&w.config, &id).exists(), "no file either");
    // An empty title is refused and the box stays.
    key(&mut app, KeyCode::Char('n'));
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::ProjectName) && app.active_notice() == Some("A ticket needs a title"));
    typed(&mut app, "Log in with a link");
    key(&mut app, KeyCode::Enter);
    // Made: #1, New, "Local changes only", the folder of the project's session, the template.
    let t = app.ticket(1).expect("made").clone();
    assert_eq!((t.title.as_str(), &t.state, &t.how_far, t.folders.as_slice()), ("Log in with a link", &State::New, &HowFar::Preset(0), &["/tmp".to_string()][..]));
    assert_eq!(app.tickets_store().unwrap().body(&t), tickets::TEMPLATE);
    // Then the editor list, for the ticket; enter runs the (stand-in) editor on the body.
    assert!(matches!(app.mode, Mode::PickEditor) && app.epick.ticket == Some(1));
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    assert!(flat(&draw(&mut term, &app)).contains("edit ticket #1 with"));
    key(&mut app, KeyCode::Enter);
    let start = Instant::now();
    while app.editing.is_some() && start.elapsed() < Duration::from_secs(10) {
        app.poll_editors();
        thread::sleep(Duration::from_millis(20));
    }
    assert!(app.editing.is_none() && matches!(app.mode, Mode::List));
    assert_eq!(app.active_notice(), Some("Saved the body of #1."));
    let body = app.tickets_store().unwrap().body(&t);
    assert!(body.starts_with("## What it should do") && body.ends_with("Login works with a magic link.\n"), "{body}");
    // The row is picked, in the Tickets pane.
    assert_eq!((app.pane, app.selected_ticket().map(|t| t.number)), (Pane::Tickets, Some(1)));
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("#1 Log in with a link New local ready"), "{shown}");
}

#[test]
fn a_project_with_no_sessions_makes_tickets_with_no_folder_yet() {
    let w = World::new("t-nofolder");
    let mut app = w.app();
    app.projects.create("Empty").unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Enter);
    app.set_editors_for_test(Vec::new());
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Char('n'));
    typed(&mut app, "First");
    key(&mut app, KeyCode::Enter);
    assert!(app.ticket(1).unwrap().folders.is_empty());
    key(&mut app, KeyCode::Esc); // the editor list
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.tk.open, Some(1));
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let shown = draw(&mut term, &app);
    assert!(flat(&shown).contains("Folders No folder yet"), "{shown}");
}

#[test]
fn done_and_cancelled_are_folded_under_a_divider_and_come_back_up_when_new_again() {
    let (w, mut app, id) = on_the_project("t-fold");
    for title in ["One", "Two", "Three"] {
        add(&w, &mut app, &id, title, &[]);
    }
    set_state(&w, &mut app, &id, 2, State::Done);
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.ticket_rows(), [TRow::Ticket(1), TRow::Ticket(3), TRow::Divider(1)], "folded by default");
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("▸ Done and cancelled · 1") && !shown.contains("#2 Two"), "{shown}");
    // Enter on the divider unfolds it, and again folds it.
    key(&mut app, KeyCode::Char('G'));
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.ticket_rows(), [TRow::Ticket(1), TRow::Ticket(3), TRow::Divider(1), TRow::Ticket(2)]);
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("▾ Done and cancelled · 1") && shown.contains("#2 Two Done"), "{shown}");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.ticket_rows().len(), 3);
    // `s` on #3: Cancelled. It goes under the (folded) divider, which is then the picked row.
    key(&mut app, KeyCode::Char('g'));
    key(&mut app, KeyCode::Down);
    assert_eq!(app.selected_ticket().map(|t| t.number), Some(3));
    key(&mut app, KeyCode::Char('s'));
    assert!(matches!(app.mode, Mode::TicketPick) && app.tk.pick.kind == PickKind::State);
    let shown = draw(&mut term, &app);
    assert!(shown.contains("state of #3") && shown.contains("✓ New") && shown.contains("  Done") && shown.contains("  Cancelled"), "{shown}");
    assert!(!shown.contains("Working") && !shown.contains("Waiting on you"), "only the three the user picks: {shown}");
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.ticket(3).unwrap().state, State::Cancelled);
    assert_eq!(app.ticket_rows(), [TRow::Ticket(1), TRow::Divider(2)]);
    assert!(matches!(app.ticket_rows()[app.ticket_selected()], TRow::Divider(_)));
    // Unfold, pick #3 and make it New again: back up with the open ones.
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('G'));
    assert_eq!(app.selected_ticket().map(|t| t.number), Some(3));
    key(&mut app, KeyCode::Char('s'));
    key(&mut app, KeyCode::Char('g'));
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.ticket(3).unwrap().state, State::New);
    assert_eq!(app.ticket_rows(), [TRow::Ticket(1), TRow::Ticket(3), TRow::Divider(1), TRow::Ticket(2)]);
    assert_eq!(app.selected_ticket().map(|t| t.number), Some(3), "still on it");
    assert_eq!(app.active_notice(), Some("#3 is New again, back with the open tickets."));
}

#[test]
fn rows_show_waits_for_and_ready_in_their_colors_and_the_picked_row_is_highlighted() {
    let (w, mut app, id) = on_the_project("t-blocked");
    add(&w, &mut app, &id, "Base", &[]);
    add(&w, &mut app, &id, "Middle", &[]);
    add(&w, &mut app, &id, "Top", &[1, 2]);
    set_state(&w, &mut app, &id, 1, State::Done);
    key(&mut app, KeyCode::Tab);
    let mut term = Terminal::new(TestBackend::new(140, 30)).unwrap();
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("#2 Middle New local ready"), "{shown}");
    assert!(shown.contains("#3 Top New local waits for #2"), "only the unfinished blocker: {shown}");
    let t = app.effective_theme().clone();
    let buf = buffer(&term);
    let (ry, rx) = find(&term, "ready").unwrap();
    assert_eq!(buf[(rx, ry)].fg, t.ok, "ready is quiet green");
    let (by, bx) = find(&term, "waits for #2").unwrap();
    assert_eq!(buf[(bx, by)].fg, t.warn);
    // The picked row (the first) has the highlight and the bar; the other rows don't.
    let (my, mx) = find(&term, "#2 ").unwrap();
    assert_eq!(buf[(mx, my)].bg, t.surface, "the picked row");
    assert_eq!(buf[(mx - 1, my)].symbol(), "▎");
    assert_eq!(buf[(bx, by)].bg, t.bg, "another row");
    // The state column in its color.
    key(&mut app, KeyCode::Down);
    Tickets::open(&w.config, &id).set_state(2, &State::Waiting).unwrap();
    app.reload_for_test();
    draw(&mut term, &app);
    let buf = buffer(&term);
    let (wy, wx) = find(&term, "Waiting on you").unwrap();
    assert_eq!((buf[(wx, wy)].fg, buf[(wx, wy)].bg), (t.warn, t.bg), "a squad's state, shown with its words");
    // A click on a row picks it; a click on the picked row opens it.
    let click = |app: &mut App, i: usize| {
        let r = app.hits.borrow().iter().find(|(_, h)| matches!(h, Hit::TicketRow(x) if *x == i)).map(|(r, _)| *r).expect("drawn");
        app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: r.x + 2, row: r.y, modifiers: KeyModifiers::NONE });
    };
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.pane, Pane::Sessions);
    click(&mut app, 1);
    assert_eq!((app.pane, app.selected_ticket().map(|t| t.number), app.tk.open), (Pane::Tickets, Some(3), None));
    draw(&mut term, &app);
    click(&mut app, 1);
    assert_eq!(app.tk.open, Some(3));
    key(&mut app, KeyCode::Esc);
    draw(&mut term, &app);
    // Once #2 is Done too, #3 is ready.
    set_state(&w, &mut app, &id, 2, State::Done);
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("#3 Top New local ready"), "{shown}");
}

#[test]
fn blocked_by_is_picked_from_the_other_tickets_and_loops_are_refused() {
    let (w, mut app, id) = on_the_project("t-pickblock");
    add(&w, &mut app, &id, "A", &[]);
    add(&w, &mut app, &id, "B", &[1]);
    key(&mut app, KeyCode::Tab);
    // On #1: b lists #2 only; ticking it would make a loop.
    key(&mut app, KeyCode::Char('b'));
    assert!(matches!(app.mode, Mode::TicketPick) && app.tk.pick.kind == PickKind::BlockedBy);
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("[ ] #2 B · New") && !shown.contains("#1 A ·"), "{shown}");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.active_notice(), Some("Can't do that: #2 already waits for #1, so #1 can't wait for #2"));
    assert!(app.ticket(1).unwrap().blocked_by.is_empty());
    assert!(matches!(app.mode, Mode::TicketPick), "the list stays open");
    key(&mut app, KeyCode::Esc);
    // On #2: untick #1, tick it again.
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Char('b'));
    assert!(flat(&draw(&mut term, &app)).contains("[x] #1 A · New"));
    key(&mut app, KeyCode::Enter);
    assert!(app.ticket(2).unwrap().blocked_by.is_empty());
    key(&mut app, KeyCode::Char(' '));
    assert_eq!(app.ticket(2).unwrap().blocked_by, [1]);
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn the_ticket_screen_shows_every_field_and_scrolls_the_body() {
    let (w, mut app, id) = on_the_project("t-screen");
    add(&w, &mut app, &id, "Base", &[]);
    add(&w, &mut app, &id, "Basket page", &[1]);
    let mut store = Tickets::open(&w.config, &id);
    store.set_how_far(2, &HowFar::Preset(2)).unwrap();
    let home = app.home.clone();
    let folder = w.base.join("work");
    fs::create_dir_all(&folder).unwrap();
    store.set_folders(2, &[folder.to_string_lossy().into_owned(), format!("{home}/somewhere")]).unwrap();
    let t2 = store.get(2).unwrap().clone();
    let body: String = (1..=60).map(|i| format!("- step {i}\n")).collect();
    fs::write(store.body_path(&t2), format!("## What it should do\n\nThe **basket** keeps its items.\n\n{body}")).unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.tk.open, Some(2));
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let shown = draw(&mut term, &app);
    let f = flat(&shown);
    assert!(f.contains("#2 Basket page"), "{shown}");
    assert!(f.contains("State New · waits for #1"), "{shown}");
    assert!(f.contains("How far Commit, push and open a merge request"), "{shown}");
    assert!(f.contains(&format!("Folders {}", folder.to_string_lossy())) && f.contains("~/somewhere"), "home-abbreviated: {shown}");
    assert!(f.contains("Waits for #1 Base · New"), "{shown}");
    assert!(f.contains("What it should do") && f.contains("The basket keeps its items.") && !f.contains("**"), "the Markdown drawn: {shown}");
    assert!(f.contains("e edit") && f.contains("s state") && f.contains("h how far") && f.contains("esc back"), "the keys: {shown}");
    assert!(!f.contains("step 60"));
    // The bold word is bold.
    let buf = buffer(&term);
    let (y, x) = find(&term, "basket keeps").unwrap();
    assert!(buf[(x, y)].modifier.contains(ratatui::style::Modifier::BOLD));
    // Scrolling moves the body, and not past its end.
    for _ in 0..200 {
        key(&mut app, KeyCode::Down);
    }
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("step 60") && !f.contains("step 1 ") && f.contains("State New"), "the fields stay: {f}");
    key(&mut app, KeyCode::Char('g'));
    assert!(flat(&draw(&mut term, &app)).contains("The basket keeps its items."));
    // Rename, how far in own words, folders, from the screen.
    key(&mut app, KeyCode::Char('r'));
    assert_eq!(app.name_box.input, "Basket page");
    typed(&mut app, "s");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.ticket(2).unwrap().title, "Basket pages");
    key(&mut app, KeyCode::Char('h'));
    key(&mut app, KeyCode::Char('G'));
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::ProjectName));
    typed(&mut app, "Until the tests pass");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.ticket(2).unwrap().how_far, HowFar::Own("Until the tests pass".into()));
    assert!(flat(&draw(&mut term, &app)).contains("How far Until the tests pass"));
    key(&mut app, KeyCode::Char('f'));
    assert!(flat(&draw(&mut term, &app)).contains("Add a folder…"));
    key(&mut app, KeyCode::Char('d'));
    assert_eq!(app.ticket(2).unwrap().folders, [format!("{home}/somewhere")]);
    key(&mut app, KeyCode::Char('G'));
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Picker) && app.picker.ticket == Some(2));
    assert!(flat(&draw(&mut term, &app)).contains("a folder for ticket #2"));
    typed(&mut app, &folder.to_string_lossy());
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::TicketPick), "back in the folders");
    assert_eq!(app.ticket(2).unwrap().folders, [format!("{home}/somewhere"), folder.to_string_lossy().into_owned()]);
    key(&mut app, KeyCode::Esc);
    // Esc goes back to the project's screen with the same ticket picked.
    key(&mut app, KeyCode::Esc);
    assert_eq!((app.tk.open, app.pane, app.selected_ticket().map(|t| t.number)), (None, Pane::Tickets, Some(2)));
    assert!(app.open_project().is_some());
}

#[test]
fn a_ticket_added_from_outside_shows_on_the_next_refresh_and_a_corrupt_file_is_said_and_left_alone() {
    let (w, mut app, id) = on_the_project("t-outside");
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    assert!(flat(&draw(&mut term, &app)).contains("No tickets yet"));
    Tickets::open(&w.config, &id).add(NewTicket { title: "From the CLI".into(), ..Default::default() }).unwrap();
    app.refresh_for_test();
    assert!(flat(&draw(&mut term, &app)).contains("#1 From the CLI"));
    fs::write(tickets::path_in(&w.config, &id), "{ broken").unwrap();
    app.refresh_for_test();
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("tickets.json can't be read"), "{shown}");
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Char('n'));
    typed(&mut app, "x");
    key(&mut app, KeyCode::Enter);
    assert!(app.active_notice().unwrap_or("").contains("can't be read"), "{:?}", app.active_notice());
    assert_eq!(fs::read_to_string(tickets::path_in(&w.config, &id)).unwrap(), "{ broken");
}

#[test]
fn eighty_columns_keep_the_number_title_and_what_matters_most_and_drop_how_far_first() {
    let (w, mut app, id) = on_the_project("t-80");
    add(&w, &mut app, &id, "A rather long title for a ticket", &[]);
    add(&w, &mut app, &id, "Second", &[1]);
    add(&w, &mut app, &id, "Third", &[]);
    set_state(&w, &mut app, &id, 3, State::Cancelled);
    app.tk.unfolded = true;
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let shown = draw(&mut term, &app);
    let f = flat(&shown);
    assert!(f.contains("Sessions") && f.contains("Tickets") && f.contains("Notes"), "{shown}");
    assert!(f.contains("#1 A rather long") && f.contains("… ready"), "{shown}");
    assert!(f.contains("#2 Second waits for #1"), "{shown}");
    assert!(!f.contains("local") && !f.contains(" New"), "how far goes first, then the state's own column: {shown}");
    assert!(f.contains("#3 Third Cancelled"), "a row with nothing else to say says its state: {shown}");
    // No row runs past its pane.
    let (y, x) = find(&term, "#2").unwrap();
    let buf = buffer(&term);
    let border = (x..80).find(|c| buf[(*c, y)].symbol() == "│").expect("the pane's right border");
    assert!(border < 40, "{shown}");
}

#[test]
fn ticket_columns_share_the_width_and_drop_how_far_then_the_tail_then_the_state() {
    for width in 0..200usize {
        let c = ui::ticket_cols(width, 2, 21, 9, 16);
        let used = 1 + c.num + 1 + c.title + [c.state, c.how, c.tail].iter().filter(|w| **w > 0).map(|w| w + 2).sum::<usize>();
        assert!(used <= width.max(4) || c.title == 0, "{width}: {c:?}");
        if c.how > 0 {
            assert!(c.tail > 0 && c.state > 0, "how far goes first: {width} {c:?}");
        }
        if c.state > 0 {
            assert!(c.tail > 0, "then the state, then waits for / ready: {width} {c:?}");
        }
        if c.state > 0 || c.how > 0 || c.tail > 0 {
            assert!(c.title >= 12, "{width}: {c:?}");
        }
    }
    let c = ui::ticket_cols(120, 2, 3, 5, 5);
    assert_eq!((c.state, c.how, c.tail, c.title), (3, 5, 5, 120 - 4 - 5 - 7 - 7));
}


#[test]
fn an_archived_project_takes_no_new_ticket_but_its_tickets_can_still_be_changed() {
    let (w, mut app, id) = on_the_project("t-archived");
    add(&w, &mut app, &id, "Old work", &[]);
    app.projects.archive(&id, &[]).unwrap();
    app.reload_for_test();
    assert!(app.open_project().is_some_and(|p| p.archived), "still on its screen");
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.pane, Pane::Tickets);
    key(&mut app, KeyCode::Char('n'));
    assert!(matches!(app.mode, Mode::List), "no name box");
    assert_eq!(app.active_notice(), Some("\"Web shop\" is archived; restore it first"));
    assert_eq!(app.tickets().len(), 1, "nothing made");
    // The ticket it has: opened, and its state changed.
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.tk.open, Some(1));
    key(&mut app, KeyCode::Char('s'));
    assert!(matches!(app.mode, Mode::TicketPick));
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.ticket(1).unwrap().state, State::Done);
    assert_eq!(Tickets::open(&w.config, &id).get(1).unwrap().state, State::Done, "saved");
}
