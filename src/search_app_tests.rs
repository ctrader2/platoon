//! The search dialog (`/`) and renaming (`r`) at the level of the app: real key and mouse events in, what the
//! list, the files and the screen show out. Every directory is a temp dir (`World`).

use crate::app::{Action, App, Hit, Mode};
use crate::testkit::{World, iso, now_ms};
use crate::ui;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
    layout::Rect,
    style::Modifier,
};
use serde_json::json;

/// A conversation whose user typed `prompts` (with a reply after each), last active `mins_ago` minutes ago.
fn conv(w: &World, id: &str, cwd: &str, prompts: &[&str], mins_ago: u64) {
    let at = now_ms() - mins_ago * 60_000;
    let mut lines = vec![json!({"type":"mode","mode":"normal"}).to_string()];
    for (i, p) in prompts.iter().enumerate() {
        let t = at - (prompts.len() - i) as u64 * 1000;
        lines
            .push(json!({"type":"user","cwd":cwd,"gitBranch":"main","timestamp":iso(t),"message":{"role":"user","content":p}}).to_string());
        // A tool result: the words in it must never be found.
        lines.push(
            json!({"type":"user","cwd":cwd,"timestamp":iso(t + 100),"message":{"role":"user","content":[{"tool_use_id":"x","type":"tool_result","content":format!("tool output mentioning zebra {i}")}]}})
                .to_string(),
        );
        lines.push(json!({"type":"assistant","cwd":cwd,"gitBranch":"main","timestamp":iso(t + 200),"message":{"model":"claude-sonnet-5","usage":{"input_tokens":50_000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}).to_string());
    }
    w.transcript_lines(id, &lines);
    // `transcript_lines` uses one project folder; the time of the file matters little to these tests.
    let _ = at;
}

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn ctrl(app: &mut App, c: char) {
    app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
}

fn type_str(app: &mut App, text: &str) {
    for c in text.chars() {
        key(app, KeyCode::Char(c));
    }
}

fn click(app: &mut App, x: u16, y: u16) {
    app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: x, row: y, modifiers: KeyModifiers::NONE });
}

fn names(app: &App) -> Vec<String> {
    app.visible().iter().map(|&i| app.sessions[i].id.clone()).collect()
}

fn found(app: &App) -> Vec<String> {
    app.search.results.iter().map(|r| r.id.clone()).collect()
}

fn screen(term: &mut Terminal<TestBackend>, app: &App) -> String {
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

/// (column, row) of the first `needle` on the screen.
fn find(shown: &str, needle: &str) -> (u16, u16) {
    shown
        .lines()
        .enumerate()
        .find_map(|(row, l)| l.find(needle).map(|b| (l[..b].chars().count() as u16, row as u16)))
        .unwrap_or_else(|| panic!("{needle:?} not on the screen:\n{shown}"))
}

/// Where `needle` is inside the popup whose top-left corner is at column `left`: the list behind it shows at its sides.
fn find_in_popup(shown: &str, left: u16, needle: &str, from_row: u16) -> (u16, u16) {
    shown
        .lines()
        .enumerate()
        .skip(from_row as usize)
        .find_map(|(row, l)| {
            let chars: Vec<char> = l.chars().skip(left as usize).collect();
            let text: String = chars.iter().collect();
            text.find(needle).map(|b| (left + text[..b].chars().count() as u16, row as u16))
        })
        .unwrap_or_else(|| panic!("{needle:?} not in the popup:\n{shown}"))
}

fn hit_rect(app: &App, want: impl Fn(&Hit) -> bool) -> Rect {
    app.hits.borrow().iter().find(|(_, h)| want(h)).map(|(r, _)| *r).expect("not drawn")
}

/// Four conversations with distinct words; `archived` is put away.
fn four(name: &str) -> (World, App) {
    let w = World::new(name);
    conv(&w, "aaaaaaaa-0000", "/srv/alpha", &["Fix the Docker build", "add a migration for users"], 10);
    conv(&w, "bbbbbbbb-0000", "/srv/beta", &["docker is slow", "docker again", "why is docker slow"], 20);
    conv(&w, "cccccccc-0000", "/srv/gamma", &["write the docs", "docker docs please"], 30);
    conv(&w, "dddddddd-0000", "/srv/delta", &["kubernetes cluster setup"], 40);
    let app = w.app();
    (w, app)
}

#[test]
fn slash_opens_the_dialog_on_the_quick_filter_and_typing_narrows_the_list_at_once() {
    let (_w, mut app) = four("s-quick");
    assert_eq!(app.visible().len(), 4);
    key(&mut app, KeyCode::Char('/'));
    assert!(matches!(app.mode, Mode::Search) && app.search.kind == 0, "the quick filter is the default");
    type_str(&mut app, "delta");
    assert_eq!(app.filter, "delta", "narrowed as you type");
    assert_eq!(app.visible().len(), 1);
    // Digits and the letters that are keys elsewhere are just text.
    ctrl(&mut app, 'u');
    assert_eq!((app.filter.as_str(), app.visible().len()), ("", 4));
    type_str(&mut app, "s2rq1");
    assert_eq!(app.filter, "s2rq1");
    assert!(matches!(app.mode, Mode::Search) && app.tab == 0);
}

#[test]
fn enter_keeps_the_filter_esc_clears_it_and_the_dialog_opens_on_what_is_kept() {
    let (_w, mut app) = four("s-keep");
    key(&mut app, KeyCode::Char('/'));
    type_str(&mut app, "docker");
    key(&mut app, KeyCode::Char('x'));
    key(&mut app, KeyCode::Backspace);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List) && app.filter == "docker", "kept");
    assert_eq!(names(&app), ["aaaaaaaa-0000", "bbbbbbbb-0000"], "the quick filter looks at name, folder, branch and description");

    key(&mut app, KeyCode::Char('/'));
    assert_eq!(app.search.text, "docker", "typed in again, ready to edit");
    ctrl(&mut app, 'u');
    type_str(&mut app, "beta");
    key(&mut app, KeyCode::Enter);
    assert_eq!(names(&app), ["bbbbbbbb-0000"]);
    // Esc in the list clears a kept filter, as before.
    key(&mut app, KeyCode::Esc);
    assert!(app.filter.is_empty() && app.visible().len() == 4);
    // Esc in the dialog clears it and closes.
    key(&mut app, KeyCode::Char('/'));
    type_str(&mut app, "beta");
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List) && app.filter.is_empty() && app.visible().len() == 4);
    // Ctrl+C closes it the same way.
    key(&mut app, KeyCode::Char('/'));
    type_str(&mut app, "beta");
    ctrl(&mut app, 'c');
    assert!(matches!(app.mode, Mode::List) && app.filter.is_empty());
}

#[test]
fn up_and_down_move_the_highlight_in_the_list_and_a_paste_lands_in_the_box() {
    let (_w, mut app) = four("s-move");
    key(&mut app, KeyCode::Char('/'));
    // Something has to be typed before there is anything listed to move over.
    type_str(&mut app, "s");
    assert_eq!(app.visible().len(), 4, "the letter is in every folder name, so all four are listed");
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    assert_eq!(app.selected, 2);
    key(&mut app, KeyCode::Up);
    assert_eq!(app.selected, 1);
    ctrl(&mut app, 'u');
    app.on_paste("alp\nha");
    assert_eq!((app.search.text.as_str(), app.filter.as_str(), app.selected), ("alpha", "alpha", 0), "line breaks dropped, list narrowed");
    assert_eq!(app.visible().len(), 1);
}

#[test]
fn tab_switches_the_kind_keeps_the_words_and_only_the_quick_filter_narrows_the_list() {
    let (_w, mut app) = four("s-tab");
    key(&mut app, KeyCode::Char('/'));
    type_str(&mut app, "docker");
    key(&mut app, KeyCode::Tab);
    assert_eq!((app.search.kind, app.search.text.as_str()), (1, "docker"));
    assert!(app.filter.is_empty() && app.visible().len() == 4, "the list is not narrowed by a search of the words");
    assert_eq!(found(&app).len(), 3, "three conversations mention docker");
    key(&mut app, KeyCode::Tab);
    assert_eq!((app.search.kind, app.filter.as_str()), (0, "docker"), "wraps to the first, narrowing again");
    key(&mut app, KeyCode::BackTab);
    assert_eq!(app.search.kind, 1);
    key(&mut app, KeyCode::BackTab);
    assert_eq!(app.search.kind, 0);
}

#[test]
fn a_search_finds_every_word_in_any_order_ignores_case_and_never_reads_tool_output() {
    let (_w, mut app) = four("s-words");
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Tab);
    type_str(&mut app, "MIGRATION docker");
    assert_eq!(found(&app), ["aaaaaaaa-0000"], "both words, either order, any case");
    ctrl(&mut app, 'u');
    type_str(&mut app, "zebra");
    assert!(found(&app).is_empty(), "words only in tool output are not what you typed");
    ctrl(&mut app, 'u');
    type_str(&mut app, "kube");
    assert_eq!(found(&app), ["dddddddd-0000"], "part of a word matches");
    ctrl(&mut app, 'u');
    assert!(found(&app).is_empty(), "nothing typed, nothing found");
    type_str(&mut app, "  ");
    assert!(found(&app).is_empty());
}

#[test]
fn results_are_ranked_by_one_message_having_all_the_words_then_by_how_many_mention_them_then_by_recency() {
    let w = World::new("s-rank");
    // All three words spread over different messages (older), all in one message (newer), and one word only in many.
    conv(&w, "spread00", "/srv/a", &["docker", "build", "cache"], 5);
    conv(&w, "one00000", "/srv/b", &["docker build cache all at once"], 500);
    conv(&w, "onemore0", "/srv/c", &["docker build cache in one message", "and docker build cache again"], 900);
    conv(&w, "older000", "/srv/d", &["docker build cache too"], 1000);
    let mut app = w.app();
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Tab);
    type_str(&mut app, "docker build cache");
    assert_eq!(
        found(&app),
        ["onemore0", "one00000", "older000", "spread00"],
        "one message with all the words first: more of those messages, then newer; words spread out last"
    );
}

#[test]
fn a_name_you_gave_ranks_first_and_is_searched_like_the_words_you_typed() {
    let (_w, mut app) = four("s-name-rank");
    // Name the conversation that never mentions docker.
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    assert_eq!(app.selected_session().unwrap().id, "dddddddd-0000");
    key(&mut app, KeyCode::Char('r'));
    ctrl(&mut app, 'u');
    type_str(&mut app, "Docker migration plan");
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Tab);
    type_str(&mut app, "docker");
    assert_eq!(found(&app)[0], "dddddddd-0000", "the name has the word: first, though nothing typed in it does");
    assert_eq!(app.search.results[0].detail, "kubernetes cluster setup", "with nothing typed to show, its description");
    ctrl(&mut app, 'u');
    type_str(&mut app, "kubernetes plan");
    assert_eq!(found(&app), ["dddddddd-0000"], "a word from the name and one from a message, together");
}

#[test]
fn the_settings_shape_the_results_how_many_and_whether_archived_conversations_are_included() {
    let (w, mut app) = four("s-settings");
    // Archive the newest.
    key(&mut app, KeyCode::Char('a'));
    assert_eq!(app.counts, [3, 1]);
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Tab);
    type_str(&mut app, "docker");
    assert_eq!(found(&app).len(), 3, "archived ones are searched by default");
    assert!(app.search.results.iter().any(|r| r.archived));

    let mut s = crate::settings::load(&w.config).0;
    s.search_archived = false;
    crate::settings::save(&w.config, &s).unwrap();
    app.refresh_for_test();
    assert_eq!(found(&app).len(), 2, "and skipped when the setting says so, without retyping");
    assert!(app.search.results.iter().all(|r| !r.archived));

    s.search_archived = true;
    s.search_results = 5;
    crate::settings::save(&w.config, &s).unwrap();
    app.refresh_for_test();
    assert_eq!(found(&app).len(), 3);
    // The limit: five is the smallest the settings allow; make more matches than that.
    for i in 0..8 {
        conv(&w, &format!("more{i:04}-0000"), "/srv/more", &["docker again and again"], 100 + i);
    }
    app.refresh_for_test();
    app.refresh_for_test();
    assert_eq!((app.search.results.len(), app.search.total), (5, 11), "at most the limit, and how many there were");
}

#[test]
fn enter_on_a_result_closes_the_dialog_and_selects_that_conversation_on_its_own_tab() {
    let (_w, mut app) = four("s-jump");
    // Put the oldest away, then look for it.
    for _ in 0..3 {
        key(&mut app, KeyCode::Down);
    }
    key(&mut app, KeyCode::Char('a'));
    assert_eq!(app.counts, [3, 1]);
    key(&mut app, KeyCode::Char('/'));
    type_str(&mut app, "alpha"); // a kept quick filter must not hide the target
    key(&mut app, KeyCode::Tab);
    ctrl(&mut app, 'u');
    type_str(&mut app, "kubernetes");
    assert_eq!(found(&app), ["dddddddd-0000"]);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List), "closed");
    assert_eq!(app.tab, 1, "the Archived tab, where it is");
    assert_eq!(app.selected_session().unwrap().id, "dddddddd-0000");
    assert!(app.filter.is_empty());

    // One on the Active tab: the tab switches back and the row is selected among the others.
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Tab);
    type_str(&mut app, "docs");
    key(&mut app, KeyCode::Enter);
    assert_eq!((app.tab, app.selected_session().unwrap().id.as_str()), (0, "cccccccc-0000"));
    assert_eq!(app.selected, 2, "third of the three by recency");
}

#[test]
fn with_nothing_found_enter_does_nothing_and_esc_closes_without_touching_the_list() {
    let (_w, mut app) = four("s-none");
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Tab);
    type_str(&mut app, "no such words anywhere");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Search), "still open");
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List) && app.visible().len() == 4 && app.tab == 0);
}

#[test]
fn up_down_and_page_keys_move_through_the_results() {
    let w = World::new("s-results-move");
    for i in 0..12 {
        conv(&w, &format!("id{i:02}0000-0000"), "/srv/x", &["the same words"], i as u64 + 1);
    }
    let mut app = w.app();
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Tab);
    type_str(&mut app, "same");
    assert_eq!((app.search.results.len(), app.search.selected), (12, 0));
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    assert_eq!(app.search.selected, 2);
    key(&mut app, KeyCode::PageDown);
    assert_eq!(app.search.selected, 7);
    key(&mut app, KeyCode::PageDown);
    key(&mut app, KeyCode::PageDown);
    assert_eq!(app.search.selected, 11, "stops at the last");
    key(&mut app, KeyCode::PageUp);
    key(&mut app, KeyCode::Up);
    assert_eq!(app.search.selected, 5);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.selected_session().unwrap().id, app.search.results[5].id);
    // Typing again starts from the top.
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Tab);
    type_str(&mut app, "s");
    assert_eq!(app.search.selected, 0);
}

#[test]
fn a_prompt_added_while_the_dialog_is_open_is_found_after_the_next_refresh() {
    let (w, mut app) = four("s-live");
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Tab);
    type_str(&mut app, "terraform");
    assert!(found(&app).is_empty());
    let path = w.root.join("projects/-some-project/dddddddd-0000.jsonl");
    let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
    use std::io::Write;
    writeln!(
        f,
        "{}",
        json!({"type":"user","cwd":"/srv/delta","timestamp":iso(now_ms()),"message":{"role":"user","content":"now use terraform for it"}})
    )
    .unwrap();
    drop(f);
    app.refresh_for_test();
    assert_eq!(found(&app), ["dddddddd-0000"]);
    assert_eq!(app.search.results[0].detail, "now use terraform for it");
}

#[test]
fn the_mouse_switches_kinds_selects_and_takes_a_result_and_a_click_outside_closes() {
    let (_w, mut app) = four("s-mouse");
    let mut term = Terminal::new(TestBackend::new(110, 32)).unwrap();
    key(&mut app, KeyCode::Char('/'));
    screen(&mut term, &app);
    let chip = hit_rect(&app, |h| matches!(h, Hit::SearchKind(1)));
    click(&mut app, chip.x + 1, chip.y);
    assert_eq!(app.search.kind, 1);
    type_str(&mut app, "docker");
    screen(&mut term, &app);
    let second = hit_rect(&app, |h| matches!(h, Hit::SearchRow(1)));
    click(&mut app, second.x + 3, second.y);
    assert_eq!((app.search.selected, matches!(app.mode, Mode::Search)), (1, true), "the first click selects");
    click(&mut app, second.x + 3, second.y);
    assert!(matches!(app.mode, Mode::List), "clicking the highlighted one again takes it");
    assert_eq!(app.selected_session().unwrap().id, app.search.results[1].id);
    // The wheel moves the highlight.
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Tab);
    type_str(&mut app, "docker");
    app.on_mouse(MouseEvent { kind: MouseEventKind::ScrollDown, column: 10, row: 10, modifiers: KeyModifiers::NONE });
    assert_eq!(app.search.selected, 1);
    // A click outside the dialog closes it like Esc.
    screen(&mut term, &app);
    click(&mut app, 2, 2);
    assert!(matches!(app.mode, Mode::List) && app.filter.is_empty());
}

#[test]
fn the_footer_button_and_the_click_open_the_dialog_and_quick_rows_can_be_clicked() {
    let (_w, mut app) = four("s-click");
    let mut term = Terminal::new(TestBackend::new(110, 32)).unwrap();
    screen(&mut term, &app);
    let btn = hit_rect(&app, |h| matches!(h, Hit::Action(Action::Search)));
    click(&mut app, btn.x + 1, btn.y);
    assert!(matches!(app.mode, Mode::Search));
    type_str(&mut app, "s");
    screen(&mut term, &app);
    let row = hit_rect(&app, |h| matches!(h, Hit::SearchRow(2)));
    click(&mut app, row.x + 3, row.y);
    assert_eq!(app.selected, 2);
    click(&mut app, row.x + 3, row.y);
    assert!(matches!(app.mode, Mode::List) && app.filter == "s", "a second click keeps the filter and closes");
}

// ---- the quick filter lists nothing until something is typed ----

/// The text of the dialog's results area (between its separator line and its footer line), one string per row, and
/// the rectangle of the area, for the screen the app draws now.
fn results_area(term: &mut Terminal<TestBackend>, app: &App) -> (Vec<String>, Rect) {
    let shown = screen(term, app);
    let (left, top) = find(&shown, "╭ Search");
    let lines: Vec<&str> = shown.lines().collect();
    let sep =
        (top as usize + 1..lines.len()).find(|&r| lines[r].chars().skip(left as usize + 3).take(10).all(|c| c == '─')).expect("separator");
    let foot = (sep + 1..lines.len()).find(|&r| lines[r].contains("esc ")).expect("footer");
    let width = lines[top as usize].chars().skip(left as usize).position(|c| c == '╮').expect("top right corner") as u16 + 1;
    let rows = (sep + 1..foot).map(|r| lines[r].chars().skip(left as usize + 1).take(width as usize - 2).collect::<String>()).collect();
    (rows, Rect::new(left + 1, sep as u16 + 1, width - 2, (foot - sep - 1) as u16))
}

/// Every cell of the results area has the plain dialog background and nothing but the hint is written in it.
fn assert_no_rows(term: &mut Terminal<TestBackend>, app: &App, hint: &str) {
    let (rows, area) = results_area(term, app);
    let text: Vec<String> = rows.iter().map(|r| r.trim().to_string()).filter(|r| !r.is_empty()).collect();
    assert_eq!(text, [hint], "only the hint is in the results area: {rows:?}");
    let buf = term.backend().buffer();
    let t = &app.theme;
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            let c = &buf[(x, y)];
            assert!(c.bg != t.surface && c.symbol() != "▎", "a highlighted row at ({x},{y})");
        }
    }
}

#[test]
fn the_quick_filter_lists_nothing_until_a_character_is_typed_and_nothing_again_when_the_box_is_emptied() {
    let (_w, mut app) = four("s-noprelist");
    let mut term = Terminal::new(TestBackend::new(110, 32)).unwrap();
    key(&mut app, KeyCode::Char('/'));
    assert!(app.search_rows_hidden());
    assert_no_rows(&mut term, &app, "Type to narrow the list.");
    let shown = screen(&mut term, &app);
    assert!(!shown.contains(" shown"), "no count of rows that are not listed\n{shown}");
    assert!(!app.hits.borrow().iter().any(|(_, h)| matches!(h, Hit::SearchRow(_))), "no row to click");
    assert_eq!(app.visible().len(), 4, "the list behind still shows every conversation: an empty filter narrows nothing");
    // The list behind is drawn as it always was (its rows are on the screen at the sides of the dialog).
    assert!(shown.contains("alpha") || shown.contains("/srv/alpha") || shown.contains("aaaaaaaa"), "{shown}");

    // One character: the narrowed rows, as before.
    type_str(&mut app, "d");
    let shown = screen(&mut term, &app);
    assert!(!app.search_rows_hidden() && shown.contains("4 shown"), "{shown}");
    let (rows, _) = results_area(&mut term, &app);
    assert_eq!(rows.iter().filter(|r| r.contains("/srv/")).count(), 4, "{rows:?}");
    assert!(app.hits.borrow().iter().any(|(_, h)| matches!(h, Hit::SearchRow(3))));
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    assert_eq!(app.selected, 2);

    // Deleting back to empty hides them again and the highlight starts over.
    key(&mut app, KeyCode::Backspace);
    assert_eq!((app.selected, app.search.selected, app.filter.as_str()), (0, 0, ""));
    assert_no_rows(&mut term, &app, "Type to narrow the list.");
    assert_eq!(app.visible().len(), 4);

    // Ctrl+U does the same.
    type_str(&mut app, "delta");
    let (rows, _) = results_area(&mut term, &app);
    assert_eq!(rows.iter().filter(|r| r.contains("/srv/")).count(), 1, "{rows:?}");
    ctrl(&mut app, 'u');
    assert_no_rows(&mut term, &app, "Type to narrow the list.");
    // A paste is a typing too.
    app.on_paste("beta");
    let (rows, _) = results_area(&mut term, &app);
    assert_eq!(rows.iter().filter(|r| r.contains("/srv/beta")).count(), 1, "{rows:?}");
}

#[test]
fn with_nothing_listed_the_movement_keys_the_wheel_and_clicks_do_nothing_and_enter_closes_without_a_filter() {
    let (_w, mut app) = four("s-noprelist-keys");
    let mut term = Terminal::new(TestBackend::new(110, 32)).unwrap();
    // The list's own highlight (second row) is where the user left it and stays there.
    key(&mut app, KeyCode::Down);
    let before = app.selected_session().unwrap().id.clone();
    key(&mut app, KeyCode::Char('/'));
    screen(&mut term, &app);
    for code in [KeyCode::Down, KeyCode::Down, KeyCode::PageDown, KeyCode::PageDown, KeyCode::Up, KeyCode::PageUp] {
        key(&mut app, code);
        assert!(matches!(app.mode, Mode::Search));
        assert_eq!(app.selected_session().unwrap().id, before, "no hidden row was selected");
    }
    for kind in [MouseEventKind::ScrollDown, MouseEventKind::ScrollDown, MouseEventKind::ScrollUp] {
        app.on_mouse(MouseEvent { kind, column: 30, row: 14, modifiers: KeyModifiers::NONE });
    }
    assert_eq!(app.selected_session().unwrap().id, before);
    // Clicks all over the results area (and a stale row from an earlier frame) neither select, nor keep, nor close.
    let (_, area) = results_area(&mut term, &app);
    let list = hit_rect(&app, |h| matches!(h, Hit::Inert));
    assert!(area.contains(list.as_position()) && list.height == area.height, "the empty area is a panel that swallows clicks");
    for (dx, dy) in [(0, 0), (3, 0), (10, 1), (list.width - 1, list.height - 1)] {
        click(&mut app, list.x + dx, list.y + dy);
        assert!(matches!(app.mode, Mode::Search), "a click in the empty area keeps the dialog open");
        assert_eq!(app.selected_session().unwrap().id, before);
    }
    app.hits.borrow_mut().insert(0, (list, Hit::SearchRow(2)));
    click(&mut app, list.x + 3, list.y);
    assert!(matches!(app.mode, Mode::Search) && app.selected_session().unwrap().id == before);
    // Enter closes; nothing is kept, nothing is jumped to.
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List) && app.filter.is_empty() && app.tab == 0);
    assert_eq!((app.visible().len(), app.selected_session().unwrap().id.clone()), (4, before.clone()));
    // Esc closes just the same.
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List) && app.filter.is_empty());
    // A click outside the dialog still closes it.
    key(&mut app, KeyCode::Char('/'));
    screen(&mut term, &app);
    click(&mut app, 1, 1);
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn tab_to_full_text_and_back_keeps_the_rule_and_full_text_also_lists_nothing_until_typed() {
    let (_w, mut app) = four("s-noprelist-tab");
    let mut term = Terminal::new(TestBackend::new(110, 32)).unwrap();
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Tab);
    assert!(!app.search_rows_hidden(), "the rule is the quick filter's; Full-text has no results of its own to hide");
    assert!(app.search.results.is_empty());
    assert_no_rows(&mut term, &app, "Type words you asked Claude about, in any order.");
    key(&mut app, KeyCode::Tab);
    assert_no_rows(&mut term, &app, "Type to narrow the list.");
    // With words typed: Full-text lists what it found, the quick filter its rows; empty again on both.
    type_str(&mut app, "docker");
    let (rows, _) = results_area(&mut term, &app);
    assert_eq!(rows.iter().filter(|r| r.contains("/srv/")).count(), 2);
    key(&mut app, KeyCode::Tab);
    assert_eq!(found(&app).len(), 3);
    ctrl(&mut app, 'u');
    assert_no_rows(&mut term, &app, "Type words you asked Claude about, in any order.");
    key(&mut app, KeyCode::Tab);
    assert_no_rows(&mut term, &app, "Type to narrow the list.");
    // Full-text with the box empty: the keys and Enter do nothing harmful either.
    key(&mut app, KeyCode::Tab);
    for code in [KeyCode::Down, KeyCode::PageDown, KeyCode::Enter] {
        key(&mut app, code);
    }
    assert!(matches!(app.mode, Mode::Search) && app.search.selected == 0);
    // Typing straight into the quick filter after a Tab round trip narrows again.
    key(&mut app, KeyCode::Tab);
    type_str(&mut app, "kub");
    assert_eq!(names(&app), ["dddddddd-0000"]);
}

// ---- what is drawn ----

#[test]
fn the_dialog_shows_the_chips_the_box_and_results_with_the_words_lit_in_the_accent_color() {
    let (_w, mut app) = four("s-draw");
    let mut term = Terminal::new(TestBackend::new(110, 32)).unwrap();
    key(&mut app, KeyCode::Char('/'));
    let shown = screen(&mut term, &app);
    assert!(shown.contains("╭ Search") && shown.contains("Quick filter") && shown.contains("Full-text"), "{shown}");
    assert!(shown.contains("Narrows the list by name, folder, branch and description."), "{shown}");
    let buf = term.backend().buffer().clone();
    let t = app.theme.clone();
    // The showing kind is lit, the other quiet.
    let (qx, qy) = find(&shown, "Quick filter");
    let (fx, fy) = find(&shown, "Full-text");
    assert_eq!((buf[(qx, qy)].fg, buf[(qx, qy)].bg), (t.on_accent, t.accent));
    assert!(buf[(qx, qy)].modifier.contains(Modifier::BOLD));
    assert_eq!((buf[(fx, fy)].fg, buf[(fx, fy)].bg), (t.dim, t.surface));
    // The box: a prompt mark in the accent color, the text after it.
    type_str(&mut app, "delta");
    let shown = screen(&mut term, &app);
    let (px, py) = find(&shown, "❯ delta");
    assert_eq!(term.backend().buffer()[(px, py)].fg, t.accent);
    assert_eq!(term.backend().buffer()[(px + 2, py)].fg, t.text);
    assert!(
        shown.contains("1 shown") && shown.contains("esc clear") && shown.contains("enter keep filter") && shown.contains("tab switch"),
        "{shown}"
    );

    // Full-text: the matching message under the name, the word in bold accent, the rest quiet.
    key(&mut app, KeyCode::Tab);
    ctrl(&mut app, 'u');
    type_str(&mut app, "docker");
    let shown = screen(&mut term, &app);
    assert!(shown.contains("3 found") && shown.contains("enter open") && shown.contains("esc close"), "{shown}");
    let buf = term.backend().buffer().clone();
    let (left, top) = find(&shown, "╭ Search");
    let (dx, dy) = find_in_popup(&shown, left, "why is docker slow", top);
    let (nx, ny) = find_in_popup(&shown, left, "beta", top);
    assert_eq!(ny + 1, dy, "the message is on the line under the name (best match first: the most messages mention it)");
    assert_eq!((buf[(nx, ny)].fg, buf[(nx, ny)].bg), (t.text, t.surface), "the name, in the highlighted result");
    assert!(buf[(nx, ny)].modifier.contains(Modifier::BOLD));
    assert_eq!((buf[(dx + 7, dy)].fg, buf[(dx + 7, dy)].bg), (t.accent, t.surface), "the word you typed");
    assert!(buf[(dx + 7, dy)].modifier.contains(Modifier::BOLD));
    assert_eq!(buf[(dx + 4, dy)].fg, t.subtext, "'is' is not one of them");
    assert_eq!(buf[(dx + 14, dy)].fg, t.subtext, "nor is 'slow'");
    let (ax, ay) = find_in_popup(&shown, left, "alpha", dy + 1);
    assert_eq!(ay, dy + 1, "the second result follows at once");
    assert_eq!(buf[(ax, ay)].bg, t.bg, "and is not lit");
    let (fx, fy) = find_in_popup(&shown, left, "Fix the Docker build", ay);
    assert_eq!(fy, ay + 1);
    assert_eq!((buf[(fx + 8, fy)].fg, buf[(fx, fy)].fg), (t.accent, t.subtext), "the match is found whatever its case");
}

#[test]
fn a_long_message_is_cut_around_the_match_so_the_match_stays_visible() {
    let w = World::new("s-snippet");
    let long = format!("{} the NEEDLE is here {}", "lead ".repeat(60), "tail ".repeat(60));
    conv(&w, "longlong-0000", "/srv/long", &[&long], 5);
    let mut app = w.app();
    let mut term = Terminal::new(TestBackend::new(90, 30)).unwrap();
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Tab);
    type_str(&mut app, "needle");
    let shown = screen(&mut term, &app);
    let (nx, ny) = find(&shown, "NEEDLE");
    let line: Vec<char> = shown.lines().nth(ny as usize).unwrap().chars().collect();
    assert!(line.contains(&'…'), "cut: {shown}");
    let buf = term.backend().buffer();
    assert_eq!(buf[(nx, ny)].fg, app.theme.accent);
    // The preview length setting is a ceiling: a smaller one shows less.
    let mut s = crate::settings::load(&w.config).0;
    s.search_snippet_chars = 40;
    crate::settings::save(&w.config, &s).unwrap();
    app.refresh_for_test();
    let shown = screen(&mut term, &app);
    let (left, top) = find(&shown, "╭ Search");
    let (_, ny) = find_in_popup(&shown, left, "NEEDLE", top);
    let row: Vec<char> = shown.lines().nth(ny as usize).unwrap().chars().collect();
    let right = row.iter().skip(left as usize + 1).position(|&c| c == '│').map(|p| p + left as usize + 1).unwrap();
    let cut: String = row[left as usize + 1..right].iter().collect();
    assert!(cut.trim().chars().count() <= 40, "{cut:?}");
}

#[test]
fn an_empty_dialog_says_what_to_type_and_a_search_with_no_match_says_so() {
    let (_w, mut app) = four("s-empty");
    let mut term = Terminal::new(TestBackend::new(110, 32)).unwrap();
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Tab);
    assert!(screen(&mut term, &app).contains("Type words you asked Claude about, in any order."));
    type_str(&mut app, "qqq");
    assert!(screen(&mut term, &app).contains("Nothing found"));
    key(&mut app, KeyCode::Tab);
    assert!(screen(&mut term, &app).contains("No sessions match the filter"));
}

#[test]
fn the_dialog_fits_a_small_terminal_and_tiny_ones_do_not_panic() {
    let (_w, mut app) = four("s-small");
    key(&mut app, KeyCode::Char('/'));
    type_str(&mut app, "d");
    for (w, h) in [(80, 24), (60, 16), (40, 12), (20, 6), (10, 3), (80, 5)] {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let shown = screen(&mut term, &app);
        if w >= 60 && h >= 16 {
            assert!(shown.contains("Quick filter") && shown.contains("esc clear"), "{w}x{h}\n{shown}");
        }
    }
    key(&mut app, KeyCode::Tab);
    for (w, h) in [(80, 24), (60, 16), (40, 12), (20, 6), (10, 3), (80, 5)] {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let _ = screen(&mut term, &app);
    }
}

#[test]
fn the_footer_offers_search_and_help_at_80_columns_and_rename_where_it_fits() {
    let (_w, app) = four("s-footer");
    for cols in [80, 100, 140] {
        let mut term = Terminal::new(TestBackend::new(cols, 24)).unwrap();
        let shown = screen(&mut term, &app);
        for want in ["/ search", "s settings", "q quit", "enter open", "a archive", "? help"] {
            assert!(shown.contains(want), "{want:?} at {cols}\n{shown}");
        }
        // At 80 columns `r rename` gives way to `? help`, which lists it.
        assert_eq!(shown.contains("r rename"), cols > 80, "at {cols}\n{shown}");
        assert!(!shown.contains("/ filter"));
    }
}

// ---- renaming ----

#[test]
fn r_opens_the_box_with_the_current_name_and_enter_saves_it_to_names_json_only() {
    let (w, mut app) = four("s-rename");
    let claude_before = std::fs::read(w.root.join("projects/-some-project/aaaaaaaa-0000.jsonl")).unwrap();
    key(&mut app, KeyCode::Char('r'));
    assert!(matches!(app.mode, Mode::Rename));
    assert_eq!(app.rename.input, "alpha", "what the row shows now");
    for _ in 0..5 {
        key(&mut app, KeyCode::Backspace);
    }
    type_str(&mut app, "Payments work");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.selected_session().unwrap().name, "Payments work", "on the row at once, and still selected");
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(w.config.join("names.json")).unwrap()).unwrap();
    assert_eq!(saved["names"], json!({"aaaaaaaa-0000": "Payments work"}));
    assert_eq!(app.active_notice(), Some("Renamed to \"Payments work\""));
    assert_eq!(
        std::fs::read(w.root.join("projects/-some-project/aaaaaaaa-0000.jsonl")).unwrap(),
        claude_before,
        "Claude's files are never written"
    );
    assert!(!w.root.join("names.json").exists() && !w.cache.join("names.json").exists());
    // A new run sees it, and the quick filter searches it.
    let mut again = w.app();
    assert_eq!(again.sessions.iter().find(|s| s.id == "aaaaaaaa-0000").unwrap().name, "Payments work");
    key(&mut again, KeyCode::Char('/'));
    type_str(&mut again, "payments");
    assert_eq!(names(&again), ["aaaaaaaa-0000"]);
}

#[test]
fn esc_cancels_an_unchanged_name_writes_nothing_and_an_empty_one_goes_back_to_the_automatic_name() {
    let (w, mut app) = four("s-rename-cancel");
    key(&mut app, KeyCode::Char('r'));
    type_str(&mut app, "zzz");
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List) && !w.config.join("names.json").exists());
    assert_eq!(app.selected_session().unwrap().name, "alpha");
    key(&mut app, KeyCode::Char('r'));
    key(&mut app, KeyCode::Enter);
    assert!(!w.config.join("names.json").exists(), "the same name changes nothing");
    key(&mut app, KeyCode::Char('r'));
    ctrl(&mut app, 'u');
    key(&mut app, KeyCode::Enter);
    assert!(!w.config.join("names.json").exists(), "and clearing a name that was never given writes nothing either");
    assert_eq!(
        app.selected_session().unwrap().name,
        "alpha",
        "an empty box on a conversation with no name of its own keeps the automatic one"
    );
}

#[test]
fn clearing_a_given_name_gives_the_automatic_one_back_and_other_names_stay() {
    let (w, mut app) = four("s-rename-clear");
    key(&mut app, KeyCode::Char('r'));
    type_str(&mut app, "!");
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Char('r'));
    type_str(&mut app, "?");
    key(&mut app, KeyCode::Enter);
    assert_eq!((app.sessions[0].name.as_str(), app.sessions[1].name.as_str()), ("alpha!", "beta?"));
    key(&mut app, KeyCode::Char('r'));
    assert_eq!(app.rename.input, "beta?");
    ctrl(&mut app, 'u');
    key(&mut app, KeyCode::Enter);
    assert_eq!((app.sessions[0].name.as_str(), app.sessions[1].name.as_str()), ("alpha!", "beta"));
    assert_eq!(app.active_notice(), Some("Name cleared"));
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(w.config.join("names.json")).unwrap()).unwrap();
    assert_eq!(saved["names"], json!({"aaaaaaaa-0000": "alpha!"}));
}

#[test]
fn a_new_name_keeps_whatever_else_names_json_holds_and_a_broken_file_is_backed_up() {
    let (w, mut app) = four("s-rename-keep");
    std::fs::create_dir_all(&w.config).unwrap();
    std::fs::write(w.config.join("names.json"), r#"{"note":"mine","names":{"other":"Elsewhere","obj":{"a":1}}}"#).unwrap();
    key(&mut app, KeyCode::Char('r'));
    type_str(&mut app, "x");
    key(&mut app, KeyCode::Enter);
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(w.config.join("names.json")).unwrap()).unwrap();
    assert_eq!(saved["note"], "mine");
    assert_eq!(saved["names"], json!({"other":"Elsewhere","obj":{"a":1},"aaaaaaaa-0000":"alphax"}));

    std::fs::write(w.config.join("names.json"), "{ broken").unwrap();
    app.refresh_for_test(); // the change is noticed; nothing is shown from it
    assert_eq!(app.sessions[0].name, "alpha");
    key(&mut app, KeyCode::Char('r'));
    type_str(&mut app, "y");
    key(&mut app, KeyCode::Enter);
    assert_eq!(std::fs::read_to_string(w.config.join("names.json.bak")).unwrap(), "{ broken");
    assert_eq!(app.sessions[0].name, "alphay");
}

#[test]
fn a_name_given_in_another_window_shows_after_a_refresh() {
    let (w, mut app) = four("s-rename-other");
    let mut other = crate::names::Names::open(&w.config);
    other.set("bbbbbbbb-0000", "From the other window").unwrap();
    app.refresh_for_test();
    assert_eq!(app.sessions.iter().find(|s| s.id == "bbbbbbbb-0000").unwrap().name, "From the other window");
}

#[test]
fn a_name_wins_over_the_one_a_running_session_was_given_and_is_cut_to_length() {
    let mut w = World::new("s-rename-live");
    conv(&w, "aaaaaaaa-0000", "/srv/alpha", &["hello"], 5);
    let pid = w.live("aaaaaaaa-0000", "/srv/alpha", 1, "idle");
    // The registry entry carries a name the user gave inside Claude.
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(w.root.join(format!("sessions/{pid}.json"))).unwrap()).unwrap();
    v["name"] = json!("claude-side name");
    v["nameSource"] = json!("user");
    std::fs::write(w.root.join(format!("sessions/{pid}.json")), v.to_string()).unwrap();
    let mut app = w.app();
    assert_eq!(app.sessions[0].name, "claude-side name");
    key(&mut app, KeyCode::Char('r'));
    ctrl(&mut app, 'u');
    app.on_paste(&"long ".repeat(60));
    assert_eq!(app.rename.input.chars().count(), crate::names::NAME_MAX, "the box stops at the longest name");
    key(&mut app, KeyCode::Char('z'));
    assert_eq!(app.rename.input.chars().count(), crate::names::NAME_MAX);
    ctrl(&mut app, 'u');
    app.on_paste("My\nname");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.sessions[0].name, "Myname");
    app.refresh_for_test();
    assert_eq!(app.sessions[0].name, "Myname", "it stays across refreshes");
    // Clearing it lets the running session's own name show again.
    key(&mut app, KeyCode::Char('r'));
    ctrl(&mut app, 'u');
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.sessions[0].name, "claude-side name");
}

#[test]
fn rename_on_an_empty_list_says_so_and_the_confirm_dialogs_use_the_new_name() {
    let w = World::new("s-rename-empty");
    let mut app = w.app();
    key(&mut app, KeyCode::Char('r'));
    assert!(matches!(app.mode, Mode::List) && app.active_notice() == Some("No session selected"));
    conv(&w, "aaaaaaaa-0000", "/srv/alpha", &["hi"], 5);
    app.refresh_for_test();
    key(&mut app, KeyCode::Char('r'));
    type_str(&mut app, " two");
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('a'));
    assert_eq!(app.active_notice(), Some("Archived \"alpha two\""));
}

#[test]
fn the_rename_box_draws_the_name_the_hints_and_can_be_clicked() {
    let (_w, mut app) = four("s-rename-draw");
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    key(&mut app, KeyCode::Char('r'));
    let shown = screen(&mut term, &app);
    assert!(
        shown.contains("Rename session") && shown.contains("❯ alpha") && shown.contains("enter save") && shown.contains("esc cancel"),
        "{shown}"
    );
    assert!(shown.contains("Leave it empty to go back to the automatic name."), "{shown}");
    let (x, y) = find(&shown, "❯ alpha");
    let buf = term.backend().buffer();
    assert_eq!((buf[(x, y)].fg, buf[(x + 2, y)].fg), (app.theme.accent, app.theme.text));
    // Buttons: cancel by click, save by click.
    let cancel = hit_rect(&app, |h| matches!(h, Hit::Action(Action::Cancel)));
    click(&mut app, cancel.x + 1, cancel.y);
    assert!(matches!(app.mode, Mode::List));
    key(&mut app, KeyCode::Char('r'));
    type_str(&mut app, "!");
    screen(&mut term, &app);
    let save = hit_rect(&app, |h| matches!(h, Hit::Action(Action::Confirm)));
    click(&mut app, save.x + 1, save.y);
    assert_eq!(app.sessions[0].name, "alpha!");
    // Tiny terminals do not panic.
    key(&mut app, KeyCode::Char('r'));
    for (w, h) in [(30, 8), (20, 4), (10, 2)] {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let _ = screen(&mut term, &app);
    }
}

#[test]
fn a_new_prompt_length_reaches_the_index_only_once_the_settings_have_settled() {
    let (w, mut app) = four("s-cap");
    let long = "x ".repeat(1500); // 3000 characters
    conv(&w, "eeeeeeee-0000", "/srv/e", &[long.trim()], 50);
    app.refresh_for_test();
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Tab);
    type_str(&mut app, "x");
    let n = |app: &App| app.search.results.iter().find(|r| r.id == "eeeeeeee-0000").map(|r| r.detail.chars().count()).unwrap();
    assert_eq!(n(&app), 2000, "the default cap");
    // A change on the settings screen holds the rebuild until it stops moving.
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Char('s'));
    // It is on the Search tab, the second: tab goes there.
    let (tab, longest) = crate::settings::find_item("Longest message kept").unwrap();
    for _ in 0..tab {
        key(&mut app, KeyCode::Tab);
    }
    for _ in 0..longest {
        key(&mut app, KeyCode::Down);
    }
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Right);
    assert_eq!(crate::settings::load(&w.config).0.search_prompt_chars, 20_000);
    app.refresh_for_test();
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Tab);
    type_str(&mut app, "x");
    assert_eq!(n(&app), 2000, "still waiting for the setting to settle");
    app.settle_for_test();
    app.refresh_for_test();
    assert_eq!(n(&app), 2999, "read again with the new length (a trailing space is trimmed)");
}
