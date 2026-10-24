//! The token settings and their notice at the level of the app: real key events in, the settings file, the ledger
//! reading and the screen out. Every directory is a temp dir (`World`); nothing here starts a `claude`.

use crate::agents::Agent;
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
    // The models go as the catalog has them (opus, sonnet, haiku, fable): back from the default, Haiku, is Sonnet.
    select(&mut app, "Model for smart features");
    key(&mut app, KeyCode::Left);
    assert_eq!(on_disk(&w).ai_model.as_deref(), Some("sonnet"));
    select(&mut app, "Sessions per call");
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
        .record_call(
            usage::now(),
            Agent::Claude,
            "names",
            &Usage { input: 1000, cache_creation: 200, cache_read: 30_000, output: 5 },
            0.01,
            true,
        )
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
    Ledger::open(&w.cache)
        .record_call(usage::now(), Agent::Claude, "names", &Usage { input: 500_000, ..Usage::default() }, 0.0, true)
        .unwrap();
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
    assert!(flat.contains("your own plan (Claude's, or Codex's") && flat.contains("Off nothing is sent"), "{shown}");
    let fg_at = |term: &Terminal<TestBackend>, row_word: &str, word: &str| {
        let buf = term.backend().buffer();
        (0..buf.area.height).find_map(|y| {
            let text: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
            let start = text.find(row_word)?;
            let i = start + text[start..].find(word)?;
            Some(buf[(text[..i].chars().count() as u16, y)].fg)
        })
    };
    let t = &app.theme;
    assert_eq!(
        fg_at(&term, MASTER_LABEL, "Off"),
        Some(t.readable_on(t.dim, t.surface, crate::theme::MIN_DIM)),
        "Off is muted (on the selected row)"
    );
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
    ledger.note_failure(usage::now(), Agent::Claude, FailKind::RateLimited).unwrap();
    app.refresh_for_test();
    assert_eq!(footer(&app).as_deref(), Some("Smart features paused: your Claude plan limit was reached"));
    // Later failures of the same run, and every refresh, stay quiet.
    app.notice = None;
    ledger.note_failure(usage::now(), Agent::Claude, FailKind::RateLimited).unwrap();
    for _ in 0..3 {
        app.refresh_for_test();
    }
    assert_eq!(footer(&app), None, "one line per run, not one per failure");
    // The notice is drawn on the list footer, as one line.
    ledger.note_success(Agent::Claude).unwrap();
    ledger.note_failure(usage::now() + 5, Agent::Claude, FailKind::NotSignedIn).unwrap();
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
    Ledger::open(&w.cache).note_failure(usage::now(), Agent::Claude, FailKind::Network).unwrap();
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

// ---- the model settings -------------------------------------------------------------------------------------------

/// Claude Code (a stand-in, never run) and, with `codex`, a Codex folder holding the sample model list.
fn providers(claude: bool, codex: Option<&str>) -> crate::agents::Providers {
    crate::agents::Providers {
        claude: claude.then(|| "/nonexistent/platoon-unit-test/claude".to_string()),
        codex: codex.map(crate::agents::tests::codex_with_models),
        off: Vec::new(),
    }
}

/// The text and the foreground of the cell where `word` starts.
fn word_fg(term: &Terminal<TestBackend>, word: &str) -> Option<ratatui::style::Color> {
    let buf = term.backend().buffer();
    (0..buf.area.height).find_map(|y| {
        let text: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
        text.find(word).map(|i| buf[(text[..i].chars().count() as u16, y)].fg)
    })
}

#[test]
fn the_smart_features_model_offers_every_model_installed_in_one_order_by_name() {
    let w = World::new("tok-models-all");
    let mut app = w.app();
    app.set_providers_for_test(providers(true, Some("tok-models-all")));
    let mut term = Terminal::new(TestBackend::new(110, 34)).unwrap();
    select(&mut app, "Model for smart features");
    let shown = screen(&mut term, &app);
    assert!(shown.contains("‹     Haiku     ›"), "the default, by its name:\n{shown}");
    assert_eq!(on_disk(&w).ai_model, None, "nothing picked yet: nothing written");
    let mut seen = Vec::new();
    for _ in 0..5 {
        key(&mut app, KeyCode::Right);
        seen.push(on_disk(&w).ai_model.unwrap_or_default());
    }
    assert_eq!(
        seen,
        ["fable", "gpt-6-luna", "gpt-5.6-terra", "gpt-5.6-luna", "gpt-5.6-luna"],
        "Claude's, then Codex's; it stops at the end"
    );
    let shown = screen(&mut term, &app);
    assert!(shown.contains("GPT-5.6-Luna") && !shown.contains("gpt-5.6-luna"), "shown by its name:\n{shown}");
    assert_eq!(word_fg(&term, "GPT-5.6-Luna"), Some(app.theme.accent));
    key(&mut app, KeyCode::Enter);
    assert_eq!(on_disk(&w).ai_model.as_deref(), Some("opus"), "enter goes round to the first");
}

#[test]
fn a_saved_model_this_computer_cant_run_is_said_plainly_and_kept_until_another_is_picked() {
    let w = World::new("tok-models-missing");
    let mut s = on_disk(&w);
    s.ai_model = Some("gpt-6-luna".into());
    settings::save(&w.config, &s).unwrap();
    let mut app = w.app();
    let mut term = Terminal::new(TestBackend::new(110, 34)).unwrap();
    select(&mut app, "Model for smart features");
    let shown = screen(&mut term, &app);
    assert!(shown.contains("GPT-6-Luna (needs Codex, not installed)"), "{shown}");
    assert_eq!(word_fg(&term, "GPT-6-Luna (needs Codex"), Some(app.theme.accent));
    assert_eq!(on_disk(&w).ai_model.as_deref(), Some("gpt-6-luna"), "kept as written: never swapped for another");
    // The next step is the first model there is.
    key(&mut app, KeyCode::Right);
    assert_eq!(on_disk(&w).ai_model.as_deref(), Some("opus"));
    assert!(!screen(&mut term, &app).contains("not installed"));
}

#[test]
fn project_notes_have_a_model_setting_of_their_own_sonnet_unless_another_is_picked() {
    let w = World::new("tok-models-notes");
    let mut app = w.app();
    let mut term = Terminal::new(TestBackend::new(110, 34)).unwrap();
    select(&mut app, "Model for project notes");
    assert_eq!(app.settings_tab, settings::TAB_PROJECTS, "with the project notes settings");
    let shown = screen(&mut term, &app);
    assert!(shown.contains("Sonnet") && shown.contains("Model for project notes"), "{shown}");
    key(&mut app, KeyCode::Right);
    let d = on_disk(&w);
    assert_eq!((d.notes_model.as_deref(), d.ai_model.as_deref()), (Some("haiku"), None), "only the notes' own setting moved");
}

#[test]
fn on_a_computer_with_only_codex_the_models_are_codexs_and_the_defaults_its_first() {
    let w = World::new("tok-models-codex");
    let mut app = w.app();
    app.set_providers_for_test(providers(false, Some("tok-models-codex")));
    let mut term = Terminal::new(TestBackend::new(110, 34)).unwrap();
    select(&mut app, "Model for smart features");
    let shown = screen(&mut term, &app);
    assert!(shown.contains("‹  GPT-6-Luna   ›"), "{shown}");
    assert_eq!(app.model_setting(settings::ModelFor::Smart).0, "GPT-6-Luna");
    key(&mut app, KeyCode::Left);
    assert_eq!(on_disk(&w).ai_model, None, "the first already: nothing to go back to");
    key(&mut app, KeyCode::Right);
    assert_eq!(on_disk(&w).ai_model.as_deref(), Some("gpt-5.6-terra"));
    select(&mut app, "Model for project notes");
    assert!(screen(&mut term, &app).contains("GPT-6-Luna"));
}

#[test]
fn codex_without_a_model_list_says_why_its_models_are_missing() {
    let w = World::new("tok-models-nolist");
    let mut app = w.app();
    let mut p = providers(true, Some("tok-models-nolist"));
    let root = p.codex.as_ref().unwrap().root.clone();
    std::fs::remove_file(root.join("models_cache.json")).unwrap();
    p.codex.as_mut().unwrap().root = root;
    app.set_providers_for_test(p);
    let mut term = Terminal::new(TestBackend::new(110, 40)).unwrap();
    select(&mut app, "Model for smart features");
    let shown = screen(&mut term, &app);
    assert!(shown.contains("Codex's models aren't listed"), "{shown}");
    assert_eq!(word_fg(&term, "Codex's models aren't listed"), Some(app.theme.subtext));
    select(&mut app, "Model for project notes");
    assert!(screen(&mut term, &app).contains("Codex's models aren't listed"));
    select(&mut app, "Sessions per call");
    assert!(!screen(&mut term, &app).contains("Codex's models"), "only where the model settings are");
}

#[test]
fn a_codex_model_for_the_smart_features_or_the_notes_says_what_each_call_costs_in_a_warning() {
    let w = World::new("tok-models-cost");
    let mut app = w.app();
    app.set_providers_for_test(providers(true, Some("tok-models-cost")));
    let mut term = Terminal::new(TestBackend::new(110, 44)).unwrap();
    let cost = "A call on Codex costs about 13,000 to 20,000 tokens of Codex's own instructions, however small the question.";
    // The popup's text with its borders out and every run of blanks one space (the line wraps).
    let flat = |shown: String| shown.replace(['│', '╭', '╮', '╰', '╯', '─', '▎'], " ").split_whitespace().collect::<Vec<_>>().join(" ");
    // Claude's models: nothing to say.
    select(&mut app, "Model for smart features");
    assert!(!screen(&mut term, &app).contains("A call on Codex"));
    select(&mut app, "Model for project notes");
    assert!(!screen(&mut term, &app).contains("A call on Codex"));
    // One of Codex's for the smart features: said under that tab's entries, as a warning; not on the notes' tab.
    select(&mut app, "Model for smart features");
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Right);
    assert_eq!(on_disk(&w).ai_model.as_deref(), Some("gpt-6-luna"));
    let shown = screen(&mut term, &app);
    assert!(flat(shown.clone()).contains(cost), "{shown}");
    assert_eq!(word_fg(&term, "A call on Codex"), Some(app.theme.warn));
    select(&mut app, "Model for project notes");
    assert!(!screen(&mut term, &app).contains("A call on Codex"), "the notes' model is still Sonnet");
    // And for the notes (the smart features back on Haiku).
    select(&mut app, "Model for smart features");
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Left);
    select(&mut app, "Model for project notes");
    for _ in 0..3 {
        key(&mut app, KeyCode::Right);
    }
    assert_eq!((on_disk(&w).ai_model.as_deref(), on_disk(&w).notes_model.as_deref()), (Some("haiku"), Some("gpt-6-luna")));
    let shown = screen(&mut term, &app);
    assert!(flat(shown.clone()).contains(cost), "{shown}");
    assert_eq!(word_fg(&term, "A call on Codex"), Some(app.theme.warn));
    select(&mut app, "Model for smart features");
    assert!(!screen(&mut term, &app).contains("A call on Codex"));
    // On a computer with only Codex, its first model is the default: said without anything picked.
    let w = World::new("tok-models-cost-codex");
    let mut app = w.app();
    app.set_providers_for_test(providers(false, Some("tok-models-cost-codex")));
    select(&mut app, "Model for smart features");
    assert!(flat(screen(&mut term, &app)).contains(cost));
}

// ---- each program's plan and spend ----------------------------------------------------------------------------------

/// The header's first line at `cols` columns.
fn header(app: &App, cols: u16) -> (String, Terminal<TestBackend>) {
    let mut term = Terminal::new(TestBackend::new(cols, 24)).unwrap();
    let shown = screen(&mut term, app);
    (shown.lines().next().unwrap().to_string(), term)
}

/// The usage readout at the header's right end: from the first program's name to the end of the line.
fn readout(h: &str) -> String {
    let start = [h.find("Claude"), h.find("Codex")].into_iter().flatten().min();
    start.map_or(String::new(), |i| h[i..].trim_end().to_string())
}

/// Every readout the header shows as the terminal narrows from 200 columns to the smallest platoon draws, in order,
/// each once; the tabs whole at every width.
fn ladder(app: &App) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for cols in (crate::app::MIN_WIDTH..=200).rev() {
        let (h, _) = header(app, cols);
        assert!(h.contains("1 Sessions") && h.contains("4 Squads"), "{cols}: the tabs always win: {h}");
        let r = readout(&h);
        if seen.last() != Some(&r) {
            seen.push(r);
        }
    }
    seen
}

fn window(pct: u8) -> Option<crate::model::Window> {
    Some(crate::model::Window { pct, resets_at: 9_999_999_999 })
}

fn claude_plan(five_hour: Option<u8>, seven_day: Option<u8>) -> usage::Usage {
    usage::Usage::Ready(crate::model::PlanUsage { five_hour: five_hour.and_then(window), seven_day: seven_day.and_then(window) })
}

fn codex_limit(minutes: u64, pct: u8) -> usage::Limit {
    usage::Limit { minutes, pct, resets_at: 9_999_999_999 }
}

#[test]
fn the_header_names_each_programs_plan_and_shrinks_to_percentages_then_the_fullest_window_of_each() {
    let w = World::new("tok-header-both");
    let mut app = w.app();
    app.set_providers_for_test(providers(true, Some("tok-header-both")));
    app.usage = claude_plan(Some(47), Some(21));
    app.codex_limits = Some(vec![codex_limit(43_200, 12)]);
    // Bars, then bare percentages, then the fullest window of each, then nothing: never the tabs.
    assert_eq!(
        ladder(&app),
        [
            "Claude  5h ━━━━━━  47%  7d ━━━━━━  21%  │  Codex  30d ━━━━━━  12%",
            "Claude 5h 47% 7d 21%  │  Codex 30d 12%",
            "Claude 47%  Codex 12%",
            "",
        ]
    );
    // Each program's name in the text's color, the divider faint, the windows' labels dim, the figures in subtext,
    // and the bar filled as far as the plan is used in the color of how full it is, its track in the border's.
    let (h, term) = header(&app, 160);
    let t = app.effective_theme().clone();
    assert_eq!(word_fg(&term, "Claude"), Some(t.text));
    assert_eq!(word_fg(&term, "Codex"), Some(t.text));
    assert_eq!(word_fg(&term, "│"), Some(t.border));
    assert_eq!(word_fg(&term, "5h"), Some(t.dim));
    assert_eq!(word_fg(&term, "30d"), Some(t.dim));
    assert_eq!(word_fg(&term, " 12%"), Some(t.subtext));
    let buf = term.backend().buffer();
    let bar_at = h[..h.find("5h ━").unwrap()].chars().count() as u16 + 3;
    let cells: Vec<_> = (bar_at..bar_at + 6).map(|x| buf[(x, 0)].fg).collect();
    assert_eq!(cells, [t.ok, t.ok, t.ok, t.border, t.border, t.border], "47% of six cells, in the color of under 60%");
    // A plan nearly used up says so in its bar's color.
    app.usage = claude_plan(Some(90), Some(21));
    let (h, term) = header(&app, 160);
    let bar_at = h[..h.find("5h ━").unwrap()].chars().count() as u16 + 3;
    assert_eq!(term.backend().buffer()[(bar_at, 0)].fg, t.err);
}

#[test]
fn the_narrowest_header_shows_each_programs_fullest_window_the_one_that_stops_you_first() {
    let w = World::new("tok-header-fullest");
    let mut app = w.app();
    app.set_providers_for_test(providers(true, Some("tok-header-fullest")));
    app.codex_limits = Some(vec![codex_limit(300, 3), codex_limit(10_080, 30)]);
    for (five_hour, seven_day, fullest) in [(12, 40, "Claude 40%  Codex 30%"), (70, 40, "Claude 70%  Codex 30%")] {
        app.usage = claude_plan(Some(five_hour), Some(seven_day));
        assert_eq!(ladder(&app)[2], fullest);
    }
}

#[test]
fn with_only_claude_the_header_names_claudes_plan_alone_and_says_plainly_when_there_is_no_figure() {
    let w = World::new("tok-header-claude");
    let mut app = w.app();
    app.set_providers_for_test(providers(true, None));
    app.codex_limits = None;
    app.usage = claude_plan(Some(12), Some(40));
    assert_eq!(ladder(&app), ["Claude  5h ━━━━━━  12%  7d ━━━━━━  40%", "Claude 5h 12% 7d 40%", "Claude 40%", ""]);
    // A window with no figure of its own: a dash, never "n/a".
    app.usage = claude_plan(None, Some(40));
    assert_eq!(ladder(&app), ["Claude  5h –  7d ━━━━━━  40%", "Claude 5h – 7d 40%", "Claude 40%", ""]);
    let (_, term) = header(&app, 160);
    assert_eq!(word_fg(&term, "–"), Some(app.effective_theme().dim));
    // No session has reported the plan yet, or the status line isn't set up: said in words, then a dash.
    app.usage = usage::Usage::Waiting;
    assert_eq!(ladder(&app), ["Claude  waiting for a session", "Claude –", ""]);
    app.usage = usage::Usage::NotSetUp;
    assert_eq!(ladder(&app), ["Claude  off · run `platoon setup`", "Claude –", ""]);
    let (_, term) = header(&app, 160);
    let t = app.effective_theme().clone();
    assert_eq!((word_fg(&term, "Claude"), word_fg(&term, "off ·")), (Some(t.text), Some(t.dim)));
}

#[test]
fn claude_waiting_beside_codexs_figures_is_said_in_words_and_then_a_dash() {
    let w = World::new("tok-header-waiting");
    let mut app = w.app();
    app.set_providers_for_test(providers(true, Some("tok-header-waiting")));
    app.usage = usage::Usage::Waiting;
    app.codex_limits = Some(vec![codex_limit(43_200, 2)]);
    assert_eq!(
        ladder(&app),
        ["Claude  waiting for a session  │  Codex  30d ━━━━━━   2%", "Claude –  │  Codex 30d 2%", "Claude –  Codex 2%", ""]
    );
}

#[test]
fn with_only_codex_the_header_names_codexs_plan_alone_and_says_when_there_is_no_figure_yet() {
    let w = World::new("tok-header-codex");
    let mut app = w.app();
    app.set_providers_for_test(providers(false, Some("tok-header-codex")));
    app.usage = usage::Usage::NotSetUp;
    app.codex_limits = Some(Vec::new());
    assert_eq!(ladder(&app), ["Codex  no figure yet", "Codex –", ""]);
    let (_, term) = header(&app, 120);
    let t = app.effective_theme().clone();
    assert_eq!((word_fg(&term, "Codex"), word_fg(&term, "no figure yet")), (Some(t.text), Some(t.dim)));
    // A plan with two windows: both said, each with its own label.
    app.codex_limits = Some(vec![codex_limit(300, 3), codex_limit(10_080, 30)]);
    assert_eq!(ladder(&app), ["Codex  5h ━━━━━━   3%  7d ━━━━━━  30%", "Codex 5h 3% 7d 30%", "Codex 30%", ""]);
}

#[test]
fn the_settings_say_what_each_program_used_today_against_its_own_limit() {
    let w = World::new("tok-used-both");
    let mut app = w.app();
    app.set_providers_for_test(providers(true, Some("tok-used-both")));
    let now = usage::now();
    let l = Ledger::open(&w.cache);
    l.record_call(now, Agent::Claude, "names", &Usage { input: 1000, ..Usage::default() }, 0.0, true).unwrap();
    l.record_call(now, Agent::Codex, "names", &Usage { input: 21_040, ..Usage::default() }, 0.0, true).unwrap();
    app.refresh_for_test();
    let mut term = Terminal::new(TestBackend::new(110, 40)).unwrap();
    select(&mut app, "Daily token limit");
    let shown = screen(&mut term, &app);
    assert!(shown.contains("Used today: 1,000 on Claude Code, 21,040 on Codex, of 500,000 tokens each"), "{shown}");
    assert_eq!(word_fg(&term, "Used today"), Some(app.theme.subtext));
    l.record_call(now, Agent::Codex, "names", &Usage { input: 500_000, ..Usage::default() }, 0.0, true).unwrap();
    app.refresh_for_test();
    screen(&mut term, &app);
    assert_eq!(word_fg(&term, "Used today"), Some(app.theme.warn), "one of them at its limit");
}
