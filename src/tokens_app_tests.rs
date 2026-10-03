//! The token settings and their notice at the level of the app: real key events in, the settings file, the ledger
//! reading and the screen out. Every directory is a temp dir (`World`); nothing here starts a `claude`.

use crate::ai::{self, FEATURES, MASTER_LABEL, TEST_FEATURE};
use crate::app::{App, Mode};
use crate::ledger::{FailKind, Ledger, Usage};
use crate::settings::{self, Settings};
use crate::testkit::World;
use crate::ui;
use crate::usage;
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
};

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn screen(term: &mut Terminal<TestBackend>, app: &App) -> String {
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

/// The settings screen, on the tab of the entry called `name`, with that entry selected.
fn select(app: &mut App, name: &str) {
    if !matches!(app.mode, Mode::Settings) {
        key(app, KeyCode::Char('s'));
    }
    let (tab, at) = settings::find_item(name).unwrap_or_else(|| panic!("no entry called {name}"));
    while app.settings_tab != tab {
        key(app, KeyCode::Tab);
    }
    key(app, KeyCode::Home);
    for _ in 0..at {
        key(app, KeyCode::Down);
    }
    assert_eq!(app.setting, at);
}

fn on_disk(w: &World) -> Settings {
    settings::load(&w.config).0
}

fn footer(app: &App) -> Option<String> {
    app.active_notice().map(str::to_string)
}

#[test]
fn the_master_switch_and_each_feature_switch_are_saved_at_once_and_start_off() {
    let w = World::new("tok-switches");
    let mut app = w.app();
    let d = on_disk(&w);
    assert!(!d.tokens_enabled && d.token_features.is_empty());

    select(&mut app, MASTER_LABEL);
    key(&mut app, KeyCode::Right);
    assert!(on_disk(&w).tokens_enabled, "turning it on is saved straight away");
    assert!(
        FEATURES.iter().filter(|f| !f.on_unless_switched_off).all(|f| !on_disk(&w).feature_on(f.id)),
        "the features that were here before anyone was asked are still off: they have switches of their own"
    );

    for f in FEATURES {
        select(&mut app, f.label);
        if on_disk(&w).feature_toggle(f.id) {
            key(&mut app, KeyCode::Left); // one that starts on: off first, so the step below is the same for all
        }
        assert!(!on_disk(&w).feature_on(f.id), "{}", f.id);
    }
    for f in FEATURES {
        select(&mut app, f.label);
        key(&mut app, KeyCode::Right);
        assert!(on_disk(&w).feature_on(f.id), "{}", f.id);
    }
    // Enter steps to the next choice and wraps: On -> Off.
    select(&mut app, FEATURES[0].label);
    key(&mut app, KeyCode::Enter);
    assert!(!on_disk(&w).feature_toggle(FEATURES[0].id));
    assert!(on_disk(&w).feature_toggle(FEATURES[1].id), "the other one is left alone");
    // The master off makes both off, and their own switches are remembered.
    select(&mut app, MASTER_LABEL);
    key(&mut app, KeyCode::Left);
    let d = on_disk(&w);
    assert!(!d.tokens_enabled && FEATURES.iter().all(|f| !d.feature_on(f.id)));
    assert!(d.feature_toggle(FEATURES[1].id));
}

#[test]
fn the_limits_step_through_their_choices_and_are_saved() {
    let w = World::new("tok-limits");
    let mut app = w.app();
    select(&mut app, "Daily token limit");
    key(&mut app, KeyCode::Right);
    assert_eq!(on_disk(&w).ai_daily_tokens, 1_000_000);
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Left);
    assert_eq!(on_disk(&w).ai_daily_tokens, 100_000, "the lowest step, and never zero");
    select(&mut app, "Pause above plan usage");
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Left);
    assert_eq!(on_disk(&w).ai_pause_percent, 0, "the first step is Never");
    key(&mut app, KeyCode::Right);
    assert_eq!(on_disk(&w).ai_pause_percent, 50);
    select(&mut app, "Model for smart features");
    key(&mut app, KeyCode::Right);
    assert_eq!(on_disk(&w).ai_model, "sonnet");
    select(&mut app, "Conversations per call");
    key(&mut app, KeyCode::Right);
    assert_eq!(on_disk(&w).ai_batch_size, 50);
    select(&mut app, "Refresh a summary after");
    key(&mut app, KeyCode::Right);
    assert_eq!(on_disk(&w).ai_refresh_messages, 10);
    // Nothing else moved.
    let d = on_disk(&w);
    assert_eq!((d.idle_stop_minutes, d.search_results, d.tokens_enabled), (120, 50, false));
}

#[test]
fn the_header_says_quietly_when_the_features_are_paused_shrinks_to_fit_and_goes_when_it_is_over() {
    let w = World::new("tok-header");
    let mut app = w.app();
    let mut term = Terminal::new(TestBackend::new(152, 20)).unwrap(); // (wide enough for four tabs and the whole note)
    // Nothing wrong: nothing in the header.
    assert_eq!(app.ai_header_note(200), None);
    assert!(!screen(&mut term, &app).contains("smart features"), "nothing to say while the features are simply off");
    // The features on and the plan above the pause point: the reason the user's notes stopped without a word.
    let mut s = on_disk(&w);
    s.tokens_enabled = true;
    settings::save(&w.config, &s).unwrap();
    let now = usage::now();
    std::fs::create_dir_all(w.cache.join("status")).unwrap();
    std::fs::write(
        w.cache.join("status/s1.json"),
        serde_json::json!({"ts": now, "context_pct": null, "five_hour": {"pct": 82, "resets_at": now + 3600}, "seven_day": null})
            .to_string(),
    )
    .unwrap();
    app.refresh_for_test();
    let full = "smart features paused · your Claude plan is 82% used (5-hour limit) · press s for the settings";
    let short = "smart features paused · your Claude plan is 82% used (5-hour limit)";
    assert_eq!(app.ai_header_note(200).unwrap(), full);
    assert_eq!(app.ai_header_note(full.chars().count()).unwrap(), full, "exactly enough room is enough");
    assert_eq!(app.ai_header_note(full.chars().count() - 1).unwrap(), short, "a little less: the reason without the way out");
    assert_eq!(app.ai_header_note(short.chars().count() - 1), None, "and with no room at all the tabs win");
    // On the real screen, in the header, beside the tabs.
    let shown = screen(&mut term, &app);
    let header = shown.lines().find(|l| l.contains("platoon")).unwrap_or("");
    assert!(header.contains("smart features paused"), "{shown}");
    assert!(header.contains(&format!("{} smart features", app.marker())), "with the same quiet mark as the rest: {header}");
    // The window resets: the line goes by itself.
    let _ = std::fs::remove_file(w.cache.join("status/s1.json"));
    app.refresh_for_test();
    assert_eq!(app.ai_header_note(200), None);
    assert!(!screen(&mut term, &app).contains("smart features paused"));
}

#[test]
fn how_long_to_wait_before_updating_a_projects_notes_is_on_the_projects_tab_and_saved_at_once() {
    let w = World::new("tok-quiet");
    let mut app = w.app();
    let mut term = Terminal::new(TestBackend::new(110, 40)).unwrap();
    select(&mut app, "Update notes after");
    assert_eq!(on_disk(&w).notes_quiet_minutes, 3, "three minutes to begin with");
    let shown = screen(&mut term, &app).split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(shown.contains("Update notes after") && shown.contains("3 minutes"), "{shown}");
    assert!(shown.contains("How long a project has to be quiet"), "in plain words: {shown}");
    key(&mut app, KeyCode::Right);
    assert_eq!(on_disk(&w).notes_quiet_minutes, 5, "saved as soon as it is stepped");
    for want in [15, 30] {
        key(&mut app, KeyCode::Right);
        assert_eq!(on_disk(&w).notes_quiet_minutes, want);
    }
    key(&mut app, KeyCode::Right);
    assert_eq!(on_disk(&w).notes_quiet_minutes, 30, "the longest wait it offers");
    for want in [15, 5, 3, 1] {
        key(&mut app, KeyCode::Left);
        assert_eq!(on_disk(&w).notes_quiet_minutes, want);
    }
    key(&mut app, KeyCode::Left);
    assert_eq!(on_disk(&w).notes_quiet_minutes, 1, "never no wait at all");
    // It sits on the Projects tab, with how much of Claude's replies the notes read.
    assert_eq!((settings::TABS[app.settings_tab].title, app.setting), ("Projects", 0));
    assert_eq!(settings::find_item("Read Claude's last reply up to"), Some((app.settings_tab, 1)));
}

#[test]
fn used_today_shows_the_ledger_against_the_limit_and_turns_to_a_warning_at_the_limit() {
    let w = World::new("tok-used");
    let mut app = w.app();
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    select(&mut app, "Daily token limit");
    let shown = screen(&mut term, &app);
    assert!(shown.contains("Used today: 0 of 500,000 tokens"), "{shown}");
    Ledger::open(&w.cache)
        .record_call(usage::now(), "names", &Usage { input: 1000, cache_creation: 200, cache_read: 30_000, output: 5 }, 0.01, true)
        .unwrap();
    app.refresh_for_test();
    let shown = screen(&mut term, &app);
    assert!(shown.contains("Used today: 31,205 of 500,000 tokens"), "{shown}");
    // The color: quiet while under, a warning at the limit.
    let color_of = |term: &Terminal<TestBackend>, word: &str| {
        let buf = term.backend().buffer();
        (0..buf.area.height).find_map(|y| {
            let text: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
            text.find(word).map(|i| buf[(text[..i].chars().count() as u16, y)].fg)
        })
    };
    assert_eq!(color_of(&term, "Used today"), Some(app.theme.subtext));
    Ledger::open(&w.cache).record_call(usage::now(), "names", &Usage { input: 500_000, ..Usage::default() }, 0.0, true).unwrap();
    app.refresh_for_test();
    screen(&mut term, &app);
    assert_eq!(color_of(&term, "Used today"), Some(app.theme.warn), "the limit is used up");
    // A raised limit is picked up from the file.
    select(&mut app, "Daily token limit");
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Right);
    let shown = screen(&mut term, &app);
    assert!(shown.contains("of 5,000,000 tokens"), "{shown}");
    assert_eq!(color_of(&term, "Used today"), Some(app.theme.subtext));
}

#[test]
fn the_token_entries_are_drawn_with_off_muted_and_on_in_the_accent_and_the_line_survives_a_short_terminal() {
    let w = World::new("tok-draw");
    let mut app = w.app();
    let mut term = Terminal::new(TestBackend::new(100, 40)).unwrap();
    select(&mut app, MASTER_LABEL);
    let shown = screen(&mut term, &app);
    for want in [
        MASTER_LABEL,
        "Smart names and summaries",
        "Find a session by description",
        "Daily token limit",
        "500K tokens",
        "Pause above plan usage",
        "80%",
        "Model for smart features",
        "Haiku",
        "Used today",
    ] {
        assert!(shown.contains(want), "{want}:\n{shown}");
    }
    // The master's explanation is shown while it is selected, in plain words.
    let flat = shown.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(flat.contains("your own Claude plan") && flat.contains("Off nothing is sent"), "{shown}");
    let fg_at = |term: &Terminal<TestBackend>, row_word: &str, word: &str| {
        let buf = term.backend().buffer();
        (0..buf.area.height).find_map(|y| {
            let text: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
            let start = text.find(row_word)?;
            let i = start + text[start..].find(word)?;
            Some(buf[(text[..i].chars().count() as u16, y)].fg)
        })
    };
    assert_eq!(fg_at(&term, MASTER_LABEL, "Off"), Some(app.theme.dim), "Off is muted");
    key(&mut app, KeyCode::Right);
    screen(&mut term, &app);
    assert_eq!(fg_at(&term, MASTER_LABEL, "On"), Some(app.theme.accent), "On stands out");
    // At 80x24 the selected entry is whole and the day's count is still there.
    let mut small = Terminal::new(TestBackend::new(80, 24)).unwrap();
    for name in [MASTER_LABEL, "Daily token limit", "Model for smart features"] {
        select(&mut app, name);
        let shown = screen(&mut small, &app);
        assert!(
            shown.contains(name) && shown.contains("Used today: 0 of 500,000 tokens") && shown.contains("esc back"),
            "{name}:\n{shown}"
        );
    }
}

#[test]
fn a_failure_that_pauses_the_features_is_said_once_in_the_footer_per_run_of_failures() {
    let w = World::new("tok-notice");
    let mut app = w.app();
    let mut s = on_disk(&w);
    s.tokens_enabled = true;
    settings::save(&w.config, &s).unwrap();
    app.refresh_for_test();
    assert_eq!(footer(&app), None, "nothing to say before anything failed");
    let ledger = Ledger::open(&w.cache);
    ledger.note_failure(usage::now(), FailKind::RateLimited).unwrap();
    app.refresh_for_test();
    assert_eq!(footer(&app).as_deref(), Some("Smart features paused: your Claude plan limit was reached"));
    // Later failures of the same run, and every refresh, stay quiet.
    app.notice = None;
    ledger.note_failure(usage::now(), FailKind::RateLimited).unwrap();
    for _ in 0..3 {
        app.refresh_for_test();
    }
    assert_eq!(footer(&app), None, "one line per run, not one per failure");
    // The notice is drawn on the list footer, as one line.
    ledger.note_success().unwrap();
    ledger.note_failure(usage::now() + 5, FailKind::NotSignedIn).unwrap();
    app.refresh_for_test();
    let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
    let shown = screen(&mut term, &app);
    assert!(shown.contains("Smart features paused: Claude isn't signed in"), "a new run is a new notice:\n{shown}");
    assert_eq!(shown.matches("Smart features paused").count(), 1);
}

#[test]
fn no_notice_while_the_features_are_off() {
    let w = World::new("tok-notice-off");
    let mut app = w.app();
    Ledger::open(&w.cache).note_failure(usage::now(), FailKind::Network).unwrap();
    app.refresh_for_test();
    assert_eq!(footer(&app), None, "master off: nobody is waiting for the features");
    let mut s = on_disk(&w);
    s.tokens_enabled = true;
    settings::save(&w.config, &s).unwrap();
    app.refresh_for_test();
    assert_eq!(footer(&app).as_deref(), Some("Smart features paused: Claude can't be reached (is the internet down?)"));
}

#[test]
fn switching_a_feature_or_the_master_off_cancels_the_calls_in_flight() {
    let w = World::new("tok-cancel");
    let mut app = w.app();
    let board = app.switchboard_for_test();
    let mut s = Settings { tokens_enabled: true, ..on_disk(&w) };
    s.set_feature("names", true);
    s.set_feature("find", true);
    settings::save(&w.config, &s).unwrap();
    app.refresh_for_test();
    let (names, find, test) = (board.token("names"), board.token("find"), board.token(TEST_FEATURE));
    app.refresh_for_test();
    assert!(!names.is_cancelled() && !find.is_cancelled() && !test.is_cancelled());
    // One feature off, from the settings screen: only its call is cancelled.
    select(&mut app, "Smart names and summaries");
    key(&mut app, KeyCode::Left);
    assert!(names.is_cancelled() && !find.is_cancelled() && !test.is_cancelled());
    // The master off: everything left.
    select(&mut app, MASTER_LABEL);
    key(&mut app, KeyCode::Left);
    assert!(find.is_cancelled() && test.is_cancelled());
    // ...and a switch flipped by editing the file is noticed as well.
    let again = board.token("find");
    let mut s = on_disk(&w);
    s.tokens_enabled = true;
    settings::save(&w.config, &s).unwrap();
    app.refresh_for_test();
    assert!(!again.is_cancelled());
    s.tokens_enabled = false;
    settings::save(&w.config, &s).unwrap();
    app.refresh_for_test();
    assert!(again.is_cancelled(), "found on the next refresh");
    let _ = ai::FEATURES.len();
}
