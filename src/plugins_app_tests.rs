//! Plugins at the level of the app: the Plugins tab (`4`) fetching the catalog from a local git repository, installing
//! from it with the confirm and the footer line, the theme it adds in the theme picker with its plugin's name, an
//! update, removing (a theme in use gives way to the default), and the settings screen's Plugins tab. Every folder is
//! a temp one (the data folder sits beside the test's config); the catalog is a temp git repository, never the network.

use crate::app::{App, Confirm, Mode, PlugState, TOP_PLUGINS};
use crate::plugins::testing::{catalog_repo, commit_catalog, source, GOOD_THEME};
use crate::testkit::World;
use crate::ui;
use ratatui::{
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    Terminal,
};

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn ch(app: &mut App, c: char) {
    key(app, KeyCode::Char(c));
}

fn draw(term: &mut Terminal<TestBackend>, app: &App) -> String {
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

fn flat(text: &str) -> String {
    text.replace(['│', '╭', '╮', '╰', '╯', '─'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

const DUSK: &str = "background = #1a1420\nforeground = #eee4f2\npalette = 1=#ff7a85\npalette = 2=#8bd49c\npalette = 3=#e6c170\npalette = 4=#7ab4ff\npalette = 5=#c89cf5\npalette = 9=#ff7a85\npalette = 10=#8bd49c\npalette = 11=#e6c170\npalette = 12=#7ab4ff\npalette = 13=#c89cf5\n";

/// A world with a catalog of two plugins, and its app pointed at it.
fn world(name: &str) -> (World, App, std::path::PathBuf) {
    let w = World::new(name);
    let (repo, _) = catalog_repo(&w.base, &[("sea-themes", "1.0.0", &[("Deep Water", GOOD_THEME)]), ("dusk", "0.3.0", &[("Dusk", DUSK)])]);
    let mut app = w.app();
    app.pl.source = source(&repo);
    (w, app, repo)
}

fn open_tab(app: &mut App) {
    ch(app, '4');
    assert!(app.top == TOP_PLUGINS && app.plugins_tab_showing());
    app.refresh_until_for_test(20, "the catalog", |a| !a.plugins_busy());
}

fn install(app: &mut App, id: &str) {
    let at = app.pl.rows().iter().position(|r| r.manifest.id == id).unwrap();
    app.pl.selected = at;
    key(app, KeyCode::Enter);
    assert!(matches!(&app.mode, Mode::Confirm(Confirm::InstallPlugin { id: i, update: None, .. }) if i == id), "asked first");
    ch(app, 'y');
    assert!(matches!(app.mode, Mode::List));
    app.refresh_until_for_test(20, "the install", |a| !a.plugins_busy());
}

#[test]
fn the_plugins_tab_fetches_the_catalog_and_installs_a_theme_plugin_whose_theme_is_labelled_in_the_picker() {
    let (w, mut app, _) = world("plug-tab");
    let mut term = Terminal::new(TestBackend::new(130, 30)).unwrap();
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("3 Squads") && shown.contains("4 Plugins") && !shown.contains("4 Plugins 0"), "a fourth tab, without a count: {shown}");
    open_tab(&mut app);
    let rows = app.pl.rows();
    assert_eq!(rows.iter().map(|r| (r.manifest.id.as_str(), r.state.clone())).collect::<Vec<_>>(), [("sea-themes", PlugState::Available), ("dusk", PlugState::Available)]);
    let shown = flat(&draw(&mut term, &app));
    for want in ["sea-themes name", "dusk name", "Not installed", "What sea-themes is.", "Adds 1 theme: Deep Water", "Catalog: commit", "enter install", "f check the catalog"] {
        assert!(shown.contains(want), "{want}: {shown}");
    }
    // Nothing in the user's real places: the store is beside this test's config.
    assert!(app.pl.store.dir.starts_with(&w.base));

    install(&mut app, "dusk");
    assert!(app.active_notice().unwrap().contains("dusk name is installed: 1 theme added"), "{:?}", app.active_notice());
    assert_eq!(app.pl.rows()[1].state, PlugState::Installed);
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("d remove"), "{shown}");

    // The theme is in the list, after the built-ins, with its plugin's name beside it.
    ch(&mut app, 's');
    assert!(matches!(app.mode, Mode::Settings));
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Themes));
    let at = app.tpick.list.iter().position(|t| t.id == "plugin:dusk/dusk").expect("the plugin's theme is offered");
    assert_eq!(app.tpick.list[at].name, "Dusk");
    for _ in 0..at {
        key(&mut app, KeyCode::Down);
    }
    let shown = draw(&mut term, &app);
    let line = shown.lines().find(|l| l.contains("▎ Dusk")).unwrap_or_else(|| panic!("{shown}"));
    assert!(line.contains("dusk name"), "labelled with its plugin: {line}");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.theme.id, "plugin:dusk/dusk");
    assert_eq!(app.settings().get().theme, "plugin:dusk/dusk");
    key(&mut app, KeyCode::Esc);

    // Removing it: asked, and the theme in use gives way to the default. The setting stays, so installing it again brings it back.
    let at = app.pl.rows().iter().position(|r| r.manifest.id == "dusk").unwrap();
    app.pl.selected = at;
    ch(&mut app, 'd');
    assert!(matches!(&app.mode, Mode::Confirm(Confirm::RemovePlugin { in_use: true, .. })));
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("Remove dusk name?") && shown.contains("default theme"), "{shown}");
    ch(&mut app, 'y');
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.theme.id, "catppuccin-mocha");
    assert!(app.active_notice().unwrap().contains("Removed dusk name. Its theme was in use"));
    assert_eq!(app.settings().get().theme, "plugin:dusk/dusk", "the choice is kept, as for a user theme whose file went");
    assert!(app.themes().by_id("plugin:dusk/dusk").is_none());
    install(&mut app, "dusk");
    assert_eq!(app.theme.id, "plugin:dusk/dusk", "back again");
}

#[test]
fn an_update_in_the_catalog_is_offered_and_installed() {
    let (_w, mut app, repo) = world("plug-update");
    open_tab(&mut app);
    install(&mut app, "sea-themes");
    commit_catalog(&repo, &[("sea-themes", "1.1.0", &[("Deep Water", &GOOD_THEME.replace("#14181f", "#111820"))])]);
    ch(&mut app, 'f');
    app.refresh_until_for_test(20, "the catalog", |a| !a.plugins_busy());
    let rows = app.pl.rows();
    assert_eq!(rows.len(), 1, "dusk left the catalog and was never installed");
    assert_eq!(rows[0].state, PlugState::Update);
    app.pl.selected = 0;
    key(&mut app, KeyCode::Enter);
    assert!(matches!(&app.mode, Mode::Confirm(Confirm::InstallPlugin { update: Some((a, b)), .. }) if a == "1.0.0" && b == "1.1.0"));
    ch(&mut app, 'y');
    app.refresh_until_for_test(20, "the update", |a| !a.plugins_busy());
    assert!(app.active_notice().unwrap().contains("sea-themes name is updated from 1.0.0 to 1.1.0"));
    assert_eq!(app.pl.rows()[0].state, PlugState::Installed);
}

#[test]
fn a_catalog_that_cannot_be_reached_is_said_plainly_on_the_tab() {
    let w = World::new("plug-unreachable");
    let mut app = w.app();
    let mut term = Terminal::new(TestBackend::new(120, 26)).unwrap();
    open_tab(&mut app);
    assert!(app.pl.fetch_error.as_deref().unwrap().starts_with("platoon couldn't reach its plugin catalog:"));
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("platoon couldn't reach its plugin catalog") && shown.contains("f tries again"), "{shown}");
    // Enter with nothing listed says why, rather than doing nothing.
    key(&mut app, KeyCode::Enter);
    assert!(app.active_notice().is_some());
}

#[test]
fn the_settings_plugins_tab_lists_the_installed_plugins_and_removes_one() {
    let (_w, mut app, _) = world("plug-settings");
    open_tab(&mut app);
    install(&mut app, "sea-themes");
    install(&mut app, "dusk");
    ch(&mut app, '1');
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    ch(&mut app, 's');
    app.switch_settings_tab(crate::settings::TAB_PLUGINS);
    let shown = flat(&draw(&mut term, &app));
    for want in ["Plugins", "dusk name", "0.3.0 · 1 theme", "sea-themes name", "From the catalog (commit", "press 4 on the main screen", "d remove"] {
        assert!(shown.contains(want), "{want}: {shown}");
    }
    // Keys that change a setting elsewhere do nothing here; down and d remove the second one (by name: sea-themes).
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::End);
    assert_eq!(app.setting, 1);
    key(&mut app, KeyCode::Down);
    assert_eq!(app.setting, 1, "stops at the last");
    ch(&mut app, 'd');
    assert!(matches!(&app.mode, Mode::Confirm(Confirm::RemovePlugin { id, .. }) if id == "sea-themes"));
    ch(&mut app, 'n');
    assert!(matches!(app.mode, Mode::Settings), "no goes back to the settings");
    ch(&mut app, 'd');
    ch(&mut app, 'y');
    assert!(matches!(app.mode, Mode::Settings), "and so does yes");
    assert_eq!(app.pl.installed.iter().map(|p| p.id()).collect::<Vec<_>>(), ["dusk"]);
    assert_eq!(app.setting, 0);
    ch(&mut app, 'd');
    ch(&mut app, 'y');
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("No plugins installed."), "{shown}");
    // With none left, the keys are harmless.
    for k in [KeyCode::Down, KeyCode::Up, KeyCode::End, KeyCode::Char('d'), KeyCode::Enter] {
        key(&mut app, k);
        assert!(matches!(app.mode, Mode::Settings));
    }
    app.on_mouse(ratatui::crossterm::event::MouseEvent {
        kind: ratatui::crossterm::event::MouseEventKind::ScrollDown,
        column: 60,
        row: 20,
        modifiers: KeyModifiers::NONE,
    });
}

