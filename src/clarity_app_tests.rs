//! The second round of small things that make platoon's screens read cleanly: a session's description line ending where
//! STATUS starts, a session that runs told apart from one that doesn't, `v` with no editor said in one short line, the
//! Squads tab keeping its main key and pointing at the right settings, one name for the editor, words that wrap
//! together, and a help heading never left alone at the fold. Every directory is a temp dir (`World`); the only process
//! started is a `sleep` standing in for a running claude.

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

fn sized(app: &mut App, w: u16, h: u16) -> Terminal<TestBackend> {
    app.sync_size(w, h);
    Terminal::new(TestBackend::new(w, h)).unwrap()
}

/// What the terminal shows, as lines (a wide character's second cell is part of it).
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

fn flat(lines: &[String]) -> String {
    lines.join(" ").replace(['│', '╭', '╮', '╰', '╯', '─'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Where `needle` starts in `line`, in cells.
fn cell_of(line: &str, needle: &str) -> Option<usize> {
    line.find(needle).map(|at| line[..at].width())
}

// ---- the description line ------------------------------------------------------------------------------------------

#[test]
fn a_sessions_description_ends_where_status_starts_at_every_width() {
    let w = World::new("c-desc");
    let long = "rewrite the whole checkout flow so that coupons, gift cards and store credit all add up the same way on every page";
    w.transcript("alpha-1", "/srv/alpha", &[long; 3].join(" "), 10);
    w.transcript("tokyo-2", "/srv/東京", &"テストを書く🚀".repeat(20), 20);
    let mut app = w.app();
    app.notice = None;
    for width in [60u16, 80, 120, 200] {
        let mut term = sized(&mut app, width, 24);
        let lines = draw(&mut term, &app);
        let ctx = format!("{width}\n{}", lines.join("\n"));
        let titles = lines.iter().find(|l| l.contains("NAME") && l.contains("STATUS")).expect("the column titles");
        let status = cell_of(titles, "STATUS").unwrap();
        for start in ["rewrite the whole", "テストを書く"] {
            let line = lines.iter().find(|l| l.contains(start)).unwrap_or_else(|| panic!("{start}: {ctx}"));
            let text = line.trim_end();
            assert!(text.ends_with('…'), "{width}: cut, and it says so: {line:?}\n{ctx}");
            // Its last cell before STATUS is blank, like every column's.
            assert!(text.width() < status, "{width}: ends before STATUS ({status}): {line:?}\n{ctx}");
            assert!(text.width() + 4 >= status, "{width}: and reaches it, spanning the columns before: {line:?}\n{ctx}");
        }
    }
}

// ---- running or not ------------------------------------------------------------------------------------------------

#[test]
fn a_running_idle_session_says_idle_with_a_filled_mark_and_one_nothing_runs_says_nothing() {
    let mut w = World::new("c-running");
    w.transcript("resting", "/srv/alpha", "a running one", 10);
    w.transcript("saved", "/srv/beta", "a saved one", 20);
    let _pid = w.live("resting", "/srv/alpha", 1, "idle");
    let mut app = w.app();
    app.notice = None;
    let mut term = sized(&mut app, 120, 24);
    let lines = draw(&mut term, &app);
    let ctx = lines.join("\n");
    let row = |name: &str| lines.iter().find(|l| l.contains(name)).unwrap_or_else(|| panic!("{name}: {ctx}")).clone();
    // (Each row's name line: the name, from its folder, and the folder.)
    let (running, saved) = (row("/srv/alpha"), row("/srv/beta"));
    assert!(running.contains("● alpha") && running.contains(" idle "), "{running}\n{ctx}");
    assert!(saved.contains("○ beta") && !saved.contains("idle"), "{saved}\n{ctx}");
    // A platoon host started on it runs it too, before its claude has said so.
    app.hosts.push(crate::host::HostInfo {
        claude_pid: 1,
        host_pid: 1,
        dir: "/srv/beta".into(),
        args: vec!["--resume".into(), "saved".into()],
        sock: w.base.join("sock"),
    });
    let lines = draw(&mut term, &app);
    let saved = lines.iter().find(|l| l.contains("/srv/beta")).unwrap();
    assert!(saved.contains("● beta") && saved.contains(" idle "), "{saved}");
    // The help says what the two marks mean, where sessions are listed.
    let help = ui::help_for_test(&app);
    assert!(help.iter().any(|(s, _, n)| s == "Good to know" && n.starts_with("● idle: the session is running")), "{help:?}");
    // Quitting says the same words.
    assert_eq!(crate::quit_line(0), None);
    assert_eq!(crate::quit_line(1).unwrap(), "1 session keeps running: platoon picks it up next time.");
    assert_eq!(crate::quit_line(3).unwrap(), "3 sessions keep running: platoon picks them up next time.");
}

// ---- `v` and the editor ---------------------------------------------------------------------------------------------

#[test]
fn v_with_no_editor_says_so_whole_beside_help_at_eighty_columns() {
    let w = World::new("c-noeditor");
    w.transcript("alpha-1", "/srv/alpha", "fix the login page", 10);
    let mut app = w.app();
    app.notice = None;
    ch(&mut app, 'v');
    assert_eq!(app.active_notice(), Some(crate::app::NO_EDITOR));
    let mut term = sized(&mut app, 80, 24);
    let lines = draw(&mut term, &app);
    let footer = lines.last().unwrap();
    assert!(footer.contains(crate::app::NO_EDITOR) && footer.contains("? help"), "{footer}");
}

#[test]
fn the_folder_picker_for_the_editor_says_so_and_never_picks_the_whole_home_folder() {
    let w = World::new("c-picker");
    let app0 = w.app();
    let home = app0.home.clone();
    drop(app0);
    // The home folder is the most recent place Claude ran, the world's own folder the one before.
    let history = [
        serde_json::json!({"project": home, "timestamp": 2}).to_string(),
        serde_json::json!({"project": w.base.to_string_lossy(), "timestamp": 1}).to_string(),
    ];
    std::fs::write(w.root.join("history.jsonl"), history.join("\n")).unwrap();
    let mut app = w.app();
    app.notice = None;
    ch(&mut app, 'v');
    assert!(matches!(app.mode, Mode::Picker) && app.picker.editor);
    assert_eq!(app.picker.results[0], home, "~ is still listed first");
    assert_eq!(app.picker.results[app.picker.selected], w.base.to_string_lossy(), "the first real folder is picked");
    let mut term = sized(&mut app, 100, 30);
    let f = flat(&draw(&mut term, &app));
    assert!(f.contains("Pick a folder to open in your editor") && !f.contains("Code editor"), "{f}");
    // A new session's picker is as it was: the first entry.
    key(&mut app, KeyCode::Esc);
    ch(&mut app, 'n');
    assert!(!app.picker.editor && app.picker.selected == 0);
}

#[test]
fn the_editor_has_one_name_in_the_settings() {
    let names: Vec<&str> = crate::settings::TABS.iter().flat_map(|t| t.items.iter().map(|i| i.name)).collect();
    assert!(names.contains(&"Editor for folders (v)") && names.contains(&"Editor for notes and tickets (e)"), "{names:?}");
    assert!(!names.iter().any(|n| n.contains("files") || n.contains("Code editor")), "{names:?}");
}

// ---- the Squads tab ------------------------------------------------------------------------------------------------

fn on_the_squads_tab(name: &str) -> (World, App) {
    let w = World::new(name);
    w.transcript("alpha-1", "/srv/alpha", "fix the login page", 10);
    let mut app = w.app();
    app.notice = None;
    ch(&mut app, '3');
    assert!(app.squads_tab_showing());
    (w, app)
}

#[test]
fn the_squads_tab_keeps_enter_details_at_eighty_columns() {
    let (_w, mut app) = on_the_squads_tab("c-sq80");
    let mut term = sized(&mut app, 80, 24);
    let lines = draw(&mut term, &app);
    let footer = lines.last().unwrap();
    assert!(footer.contains("enter details") && footer.contains("? help"), "{footer}");
}

#[test]
fn settings_s_projects_stays_on_one_line_and_s_opens_the_settings_there() {
    let (_w, mut app) = on_the_squads_tab("c-sqs");
    key(&mut app, KeyCode::Enter);
    for width in [80u16, 100, 120, 160, 200] {
        let mut term = sized(&mut app, width, 40);
        let lines = draw(&mut term, &app);
        let ctx = format!("{width}\n{}", lines.join("\n"));
        assert!(lines.iter().any(|l| l.contains("Settings (s) → Projects.")), "{ctx}");
    }
    key(&mut app, KeyCode::Esc);
    ch(&mut app, 's');
    assert!(matches!(app.mode, Mode::Settings));
    assert_eq!(app.settings_tab, crate::settings::TAB_PROJECTS, "on the tab with the leaders' prompt and model");
    key(&mut app, KeyCode::Esc);
    // Elsewhere the settings open on their first tab, as before.
    ch(&mut app, '1');
    ch(&mut app, 's');
    assert_eq!(app.settings_tab, 0);
}

// ---- wrapping ------------------------------------------------------------------------------------------------------

#[test]
fn a_wide_character_ending_in_a_popups_gap_goes_so_the_gap_shows() {
    let t = crate::theme::Theme::by_id("tokyo-night").unwrap();
    let mut term = Terminal::new(TestBackend::new(30, 6)).unwrap();
    let wide = |f: &mut ratatui::Frame| {
        for y in 0..6 {
            f.render_widget(ratatui::widgets::Paragraph::new("東".repeat(15)), Rect::new(0, y, 30, 1));
        }
    };
    term.draw(wide).unwrap();
    // The gap is column 7, the second half of the character at 6.
    term.draw(|f| {
        wide(f);
        ui::popup_for_test(f, &t, f.area(), Rect::new(8, 1, 14, 4));
    })
    .unwrap();
    let buf = term.backend().buffer().clone();
    for y in 1..5u16 {
        let l = shown(&term)[y as usize].clone();
        assert_eq!(buf[(6, y)].symbol(), " ", "{y}: the character over the gap goes: {l}");
        assert_eq!(buf[(7, y)].symbol(), " ", "{y}: the gap: {l}");
        assert!(["│", "╭", "╰"].contains(&buf[(8, y)].symbol()), "{y}: the left border: {l}");
        assert_eq!(buf[(4, y)].symbol(), "東", "{y}: the screen before it stays: {l}");
    }
}

#[test]
fn the_too_small_message_keeps_each_size_with_its_words() {
    let w = World::new("c-small");
    let mut app = w.app();
    app.notice = None;
    for width in 16u16..60 {
        let mut term = sized(&mut app, width, 12);
        let lines = draw(&mut term, &app);
        let ctx = format!("{width}\n{}", lines.join("\n"));
        let size = format!("{width}×12");
        let line = lines.iter().find(|l| l.contains(&size)).unwrap_or_else(|| panic!("{ctx}"));
        assert!(line.contains(&format!("is {size}")), "{width}: {line:?}\n{ctx}");
        assert!(lines.iter().any(|l| l.contains("least 60×14")), "{ctx}");
    }
}

/// The help popup's text lines in view: between its top border and the blank line above its keys.
fn help_text(lines: &[String]) -> Vec<String> {
    let top = lines.iter().position(|l| l.contains("╭") && l.contains("Keys")).expect("the help");
    let keys = (top..lines.len()).find(|&y| lines[y].contains("esc close")).expect("its keys");
    lines[top + 1..keys].iter().map(|l| l.replace('│', " ").trim().to_string()).collect()
}

#[test]
fn a_help_heading_is_never_the_last_line_in_view() {
    let w = World::new("c-help");
    w.transcript("aaa-1", "/srv/shop", "a session", 10);
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
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Enter);
    assert!(app.tk.open.is_some(), "a ticket's screen");
    let headings = ["On this screen", "Anywhere in platoon", "Good to know"];
    for height in 14u16..=30 {
        let mut term = sized(&mut app, 80, height);
        ch(&mut app, '?');
        assert!(app.help_showing());
        // Scrolled from the top to the end, a line at a time.
        for _ in 0..40 {
            let lines = draw(&mut term, &app);
            let text = help_text(&lines);
            let last = text.iter().rev().find(|l| !l.is_empty()).cloned().unwrap_or_default();
            assert!(!headings.contains(&last.as_str()), "80x{height}: {last:?} alone at the fold\n{}", lines.join("\n"));
            key(&mut app, KeyCode::Down);
        }
        key(&mut app, KeyCode::Esc);
    }
}

// ---- words ---------------------------------------------------------------------------------------------------------

#[test]
fn the_smart_features_say_session() {
    for f in crate::ai::FEATURES {
        assert!(!f.description.contains("onversation"), "{}: {}", f.id, f.description);
    }
    for a in crate::instructions::ACTIONS.iter() {
        assert!(!a.explain.contains("onversation"), "{}: {}", a.id, a.explain);
    }
}
