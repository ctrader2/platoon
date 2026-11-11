//! The sign-in notice in the app (`signin.rs`, `App::news_showing`): an answer of `claude -p` (here a task's check,
//! from a stand-in) says the Claude Code sign-in ends within ten days, and the app says so once, on its own line above
//! the footer of the Sessions and Projects lists and the Tasks tab's, in the theme's warning color like the other
//! notices (the version and setup notices stay off the Tasks tab, and wait there for the Sessions list); put away by the
//! next key, never again for that sign-in, again for a new one, gone by itself when an answer has no date any more, and
//! never where Claude Code isn't in use. Drawn the way a terminal shows it (TestBackend). Every folder is a temp dir.

use crate::agents::{CodexAt, Providers};
use crate::ai::{Request, Runner};
use crate::app::{App, TOP_SESSIONS, TOP_TASKS};
use crate::settings::{self, Settings};
use crate::testkit::{AiFake, World, iso};
use crate::{signin, ui, usage};
use ratatui::{
    Terminal,
    backend::TestBackend,
    buffer::Buffer,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
};

const DAY: u64 = 86_400;
const NONE: [&str; 0] = [];

fn world(name: &str) -> World {
    let w = World::new(name);
    for i in 0..4u64 {
        w.transcript(&format!("conv-{i}"), &format!("/srv/project-{i}"), &format!("prompt number {i}"), 10 + i);
    }
    w
}

/// A task's check run as the `task-stop` hook runs it, on a stand-in `claude -p` that says the sign-in ends at `ends`
/// (None: says no date), with the world's cache folder.
fn check_answers(w: &World, ends: Option<u64>) {
    let fake = AiFake::new("signin-app");
    fake.reply_default("{}");
    fake.sign_in_ends(ends.map(|t| iso(t * 1000)).as_deref());
    let mut req = Request::new(crate::verdict::FEATURE, "sys", "prompt");
    req.model = Some("opus".into());
    Runner::with_program(&w.cache, &fake.script).run(&Settings::default(), &req).unwrap();
    assert_eq!(fake.calls(), 1);
}

/// What the notice says for a sign-in that ends at `ends`, in the user's own calendar.
fn between(ends: u64) -> String {
    format!(
        "Your Claude Code sign-in ends between {} and {}. Run `claude auth login` before then to keep sessions, task checks and smart features working.",
        signin::local_day(ends - 3 * DAY),
        signin::local_day(ends)
    )
}

fn drawn(app: &App, w: u16, h: u16) -> Buffer {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ui::draw(f, app)).unwrap();
    term.backend().buffer().clone()
}

fn lines(buf: &Buffer) -> Vec<String> {
    (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect()).collect()
}

fn words(lines: &[String]) -> String {
    lines.join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn key(app: &mut App, c: char) {
    app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
}

fn app_of(w: &World) -> App {
    let mut app = w.app();
    app.notice = None;
    app.refresh();
    app
}

#[test]
fn a_sign_in_that_ends_within_ten_days_is_said_on_its_own_lines_above_the_footer_in_the_warning_color() {
    let w = world("si-render");
    let ends = usage::now() + 6 * DAY;
    check_answers(&w, Some(ends));
    let app = app_of(&w);
    let want = between(ends);
    assert_eq!(app.news_showing(), [want.as_str()]);
    for width in [110u16, 80] {
        let buf = drawn(&app, width, 24);
        let shown = lines(&buf);
        println!("{width}x24:\n{}\n", shown.join("\n"));
        let footer = shown.len() - 1;
        assert!(shown[footer].contains("? help"), "{width}: the footer is the last line");
        let first = shown.iter().position(|l| l.trim_start().starts_with("Your Claude Code sign-in")).expect("the notice starts a line");
        assert_eq!(words(&shown[first..footer]), want, "{width}: whole, just above the footer");
        let warn = app.effective_theme().warn;
        let x = shown[first].find("Your").unwrap() as u16;
        assert_eq!(buf[(x, first as u16)].fg, warn, "{width}: in the theme's warning color");
    }
}

#[test]
fn said_once_per_sign_in_and_again_for_a_new_one() {
    let w = world("si-once");
    let ends = usage::now() + 5 * DAY;
    check_answers(&w, Some(ends));
    let mut app = app_of(&w);
    let want = between(ends);
    assert_eq!(app.news_showing(), [want.as_str()]);
    key(&mut app, 'j');
    assert_eq!(app.news_showing(), NONE, "a key puts it away");
    app.refresh();
    assert_eq!(app.news_showing(), NONE, "and it doesn't come back");
    assert_eq!(app_of(&w).news_showing(), NONE, "nor at the next start");
    check_answers(&w, Some(ends));
    assert_eq!(app_of(&w).news_showing(), NONE, "another answer about the same sign-in");
    // Signed in again: a month away, nothing to say.
    check_answers(&w, Some(usage::now() + 30 * DAY));
    app.refresh();
    assert_eq!(app.news_showing(), NONE);
    // A new sign-in that ends soon too is said again.
    let next = usage::now() + 8 * DAY;
    check_answers(&w, Some(next));
    app.refresh();
    let want = between(next);
    assert_eq!(app.news_showing(), [want.as_str()]);
}

#[test]
fn it_shows_on_the_tasks_tab_too_waits_while_the_help_is_open_and_goes_when_answers_have_no_date_any_more() {
    let w = world("si-wait");
    let ends = usage::now() + 4 * DAY;
    check_answers(&w, Some(ends));
    let mut app = w.app();
    app.notice = None;
    app.top = TOP_TASKS;
    app.set_news_for_test("platoon 9.9.9 is out.");
    app.help_open = true;
    app.refresh();
    assert_eq!(app.news_showing(), NONE, "not over the help");
    assert!(!w.cache.join("sign-in-notice.json").exists(), "not remembered as said before it shows");
    app.help_open = false;
    app.refresh();
    let want = between(ends);
    assert_eq!(app.news_showing(), [want.as_str()], "on the Tasks tab, without the version notice");
    let shown = lines(&drawn(&app, 110, 24));
    println!("{}", shown.join("\n"));
    let footer = shown.len() - 1;
    let first = shown.iter().position(|l| l.trim_start().starts_with("Your Claude Code sign-in")).expect("the notice starts a line");
    assert_eq!(words(&shown[first..footer]), want, "whole, just above the Tasks tab's footer");
    key(&mut app, 'j');
    assert_eq!(app.news_showing(), NONE, "a key puts it away");
    app.top = TOP_SESSIONS;
    app.refresh();
    assert_eq!(app.news_showing(), ["platoon 9.9.9 is out."], "the version notice kept for the Sessions list");
    // The user switched to an API key: the next answer has no date, and the notice goes.
    let w = world("si-gone");
    check_answers(&w, Some(ends));
    let mut app = app_of(&w);
    assert_eq!(app.news_showing(), [want.as_str()]);
    check_answers(&w, None);
    app.refresh();
    assert_eq!(app.news_showing(), NONE);
    assert!(!w.cache.join("sign-in.json").exists());
}

#[test]
fn nothing_without_a_date_or_with_claude_code_switched_off() {
    let w = world("si-none");
    check_answers(&w, None);
    assert_eq!(app_of(&w).news_showing(), NONE, "an API key: no date, nothing said");
    check_answers(&w, Some(usage::now() + 12 * DAY));
    assert_eq!(app_of(&w).news_showing(), NONE, "twelve days away: not yet");

    let w = world("si-off");
    check_answers(&w, Some(usage::now() + 2 * DAY));
    let mut app = w.app();
    app.notice = None;
    app.set_providers_for_test(Providers {
        claude: Some("claude".into()),
        codex: Some(CodexAt { program: "/nonexistent/platoon-unit-test/codex".into(), root: w.codex() }),
        off: Vec::new(),
    });
    let mut s = settings::load(&w.config).0;
    s.use_claude_code = false;
    settings::save(&w.config, &s).unwrap();
    app.refresh_for_test();
    app.refresh();
    assert_eq!(app.news_showing(), NONE);
    assert!(!w.cache.join("sign-in-notice.json").exists());
}
