//! Plugins at the level of the app: the settings' Plugins tab listing every plugin (the catalog read when the settings
//! open) with the selected one's description, and the plugin screen it opens (installing with the confirm and the
//! footer line, the theme it adds in the theme picker with its plugin's name, an update, removing: a theme in use gives
//! way to the default; `1` `2` `3` to a tab), and back to the settings on the same plugin. A newer platoon's catalog
//! updating installed plugins when the app starts. Every folder is a temp one (the data folder sits beside the test's
//! config); the catalog is a temp folder standing in for the one built into platoon.

use crate::app::{App, Confirm, Mode, PlugState};
use crate::plugins::testing::{bump, catalog_dir, from, list_in_index, write_catalog, GOOD_THEME};
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
    let cat = catalog_dir(&w.base, &[("sea-themes", "1.0.0", &[("Deep Water", GOOD_THEME)]), ("dusk", "0.3.0", &[("Dusk", DUSK)])]);
    let mut app = w.app();
    app.pl.use_catalog(from(&cat));
    (w, app, cat)
}

/// `s`, the Plugins tab (the last: shift+tab from the first).
fn open_settings_plugins(app: &mut App) {
    ch(app, 's');
    assert!(matches!(app.mode, Mode::Settings));
    key(app, KeyCode::BackTab);
    assert_eq!(app.settings_tab, crate::settings::TAB_PLUGINS);
    assert!(!app.plugins_busy(), "nothing to wait for: the catalog comes with platoon");
}

/// The plugin screen, from the settings' Plugins tab, on its first plugin.
fn open_tab(app: &mut App) {
    open_settings_plugins(app);
    key(app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Plugins) && app.plugin_screen_showing(), "{:?}", app.active_notice());
}

fn install(app: &mut App, id: &str) {
    let at = app.pl.rows().iter().position(|r| r.manifest.id == id).unwrap();
    app.pl.selected = at;
    key(app, KeyCode::Enter);
    assert!(matches!(&app.mode, Mode::Confirm(Confirm::InstallPlugin { id: i, update: None, .. }) if i == id), "asked first");
    ch(app, 'y');
    assert!(matches!(app.mode, Mode::Plugins), "back on the plugin screen");
    app.refresh_until_for_test(20, "the install", |a| !a.plugins_busy());
}

#[test]
fn the_plugin_screen_installs_a_theme_plugin_from_the_catalog_whose_theme_is_labelled_in_the_picker() {
    let (w, mut app, _) = world("plug-tab");
    let mut term = Terminal::new(TestBackend::new(130, 30)).unwrap();
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("3 Squads") && !shown.contains("Plugins"), "three tabs: {shown}");
    open_tab(&mut app);
    let rows = app.pl.rows();
    assert_eq!(rows.iter().map(|r| (r.manifest.id.as_str(), r.state.clone())).collect::<Vec<_>>(), [("sea-themes", PlugState::Available), ("dusk", PlugState::Available)]);
    let shown = flat(&draw(&mut term, &app));
    for want in ["Settings · Plugins", "sea-themes name", "dusk name", "Not installed", "What sea-themes is.", "Adds 1 theme: Deep Water", "By Someone", "Needs platoon 0.1.0 or newer", "enter install", "ctrl+space back to settings"] {
        assert!(shown.contains(want), "{want}: {shown}");
    }
    // Where it comes from isn't a line of its own any more, and there is nothing to fetch.
    for gone in ["From ", "Catalog:", "check the catalog", "f check"] {
        assert!(!shown.contains(gone), "{gone}: {shown}");
    }
    // Nothing in the user's real places: the store is beside this test's config.
    assert!(app.pl.store.dir.starts_with(&w.base));

    install(&mut app, "dusk");
    assert!(app.active_notice().unwrap().contains("dusk name is installed: 1 theme added"), "{:?}", app.active_notice());
    assert_eq!(app.pl.rows()[1].state, PlugState::Installed);
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("d remove"), "{shown}");

    // The theme is in the list, after the built-ins, with its plugin's name beside it. (`s` does nothing on the plugin
    // screen: back to the settings, out of them, and in again on their first tab.)
    ch(&mut app, 's');
    assert!(matches!(app.mode, Mode::Plugins));
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List));
    ch(&mut app, 's');
    assert!(matches!(app.mode, Mode::Settings) && app.settings_tab == 0);
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
    key(&mut app, KeyCode::BackTab);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Plugins));

    // Removing it: asked, and the theme in use gives way to the default. The setting stays, so installing it again brings it back.
    let at = app.pl.rows().iter().position(|r| r.manifest.id == "dusk").unwrap();
    app.pl.selected = at;
    ch(&mut app, 'd');
    assert!(matches!(&app.mode, Mode::Confirm(Confirm::RemovePlugin { in_use: true, .. })));
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("Remove dusk name?") && shown.contains("default theme"), "{shown}");
    ch(&mut app, 'y');
    assert!(matches!(app.mode, Mode::Plugins));
    assert_eq!(app.theme.id, "catppuccin-mocha");
    assert!(app.active_notice().unwrap().contains("Removed dusk name. Its theme was in use"));
    assert_eq!(app.settings().get().theme, "plugin:dusk/dusk", "the choice is kept, as for a user theme whose file went");
    assert!(app.themes().by_id("plugin:dusk/dusk").is_none());
    install(&mut app, "dusk");
    assert_eq!(app.theme.id, "plugin:dusk/dusk", "back again");
}

#[test]
fn an_update_in_the_catalog_is_offered_and_installed() {
    let (_w, mut app, cat) = world("plug-update");
    open_tab(&mut app);
    install(&mut app, "sea-themes");
    // `f` fetches nothing (the catalog comes with platoon); a newer platoon brings the new one.
    write_catalog(&cat, &[("sea-themes", "1.1.0", &[("Deep Water", &GOOD_THEME.replace("#14181f", "#111820"))])]);
    ch(&mut app, 'f');
    assert!(!app.plugins_busy() && app.pl.rows().len() == 2 && app.pl.rows()[0].state == PlugState::Installed);
    app.pl.use_catalog(from(&cat));
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
fn an_empty_catalog_is_said_plainly_in_the_settings_and_f_does_nothing() {
    let w = World::new("plug-empty");
    let mut app = w.app();
    let mut term = Terminal::new(TestBackend::new(120, 26)).unwrap();
    open_settings_plugins(&mut app);
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("No plugins installed, and none from the catalog to show."), "{shown}");
    assert!(shown.contains("↑↓ move ctrl+space back") && !shown.contains("d remove"), "nothing to open or remove: {shown}");
    // Enter with nothing listed says why, rather than doing nothing, and stays in the settings.
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Settings) && app.active_notice() == Some("No plugins to show."));
    // f does nothing at all.
    app.notice = None;
    ch(&mut app, 'f');
    assert!(!app.plugins_busy() && matches!(app.mode, Mode::Settings) && app.active_notice().is_none());
    // An installed plugin is still listed when the catalog doesn't have it.
    let src = w.base.join("src");
    let dir = crate::plugins::testing::plugin(&src, "dusk", "0.3.0", &[("Dusk", DUSK)]);
    app.pl.store.install(&dir, crate::plugins::Origin::Folder { path: "/home/u/dusk".into() }).unwrap();
    app.plugins_changed_for_test();
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("dusk name 0.3.0 Installed from a folder") && shown.contains("↑↓ move enter open d remove ctrl+space back"), "{shown}");
    assert!(shown.contains("What dusk is."), "its description under the list: {shown}");
}

#[test]
fn the_settings_plugins_tab_lists_every_plugin_with_where_it_stands_and_removes_an_installed_one() {
    let (_w, mut app, cat) = world("plug-settings");
    open_tab(&mut app);
    install(&mut app, "sea-themes");
    install(&mut app, "dusk");
    // A newer sea-themes in the catalog, and a third plugin not installed.
    let newer = GOOD_THEME.replace("#14181f", "#111820");
    write_catalog(&cat, &[("sea-themes", "1.1.0", &[("Deep Water", newer.as_str())]), ("dusk", "0.3.0", &[("Dusk", DUSK)]), ("moss", "2.0.0", &[("Moss", GOOD_THEME)])]);
    app.pl.use_catalog(from(&cat));
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::Settings) && app.settings_tab == crate::settings::TAB_PLUGINS);
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let shown = draw(&mut term, &app);
    // One line each: the name, its version, and where it stands in plain words.
    let line = |name: &str| shown.lines().find(|l| l.contains(name)).unwrap_or_else(|| panic!("{name}: {shown}")).to_string();
    for (name, version, state) in [("sea-themes name", "1.1.0", "Update ready"), ("dusk name", "0.3.0", "Installed"), ("moss name", "2.0.0", "Available")] {
        let l = line(name);
        assert!(l.contains(&format!("{name}  {version}")) && l.contains(&format!(" {state} ")), "{name}: {l}");
    }
    let flat_shown = flat(&shown);
    // (dusk was selected last on the plugin screen, so it is here: its description under the list.)
    for want in ["What dusk is.", "Enter opens a plugin: what it adds, and install, update or remove.", "↑↓ move enter open d remove ctrl+space back"] {
        assert!(flat_shown.contains(want), "{want}: {flat_shown}");
    }
    assert!(!flat_shown.contains("press 4"), "{flat_shown}");
    // Keys that change a setting elsewhere do nothing here; the list stops at its ends.
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::End);
    assert_eq!(app.setting, 2);
    key(&mut app, KeyCode::Down);
    assert_eq!(app.setting, 2, "stops at the last");
    // d on one that isn't installed says so.
    ch(&mut app, 'd');
    assert!(matches!(app.mode, Mode::Settings));
    assert_eq!(app.active_notice(), Some("moss name isn't installed."));
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("↑↓ move enter open ctrl+space back"), "no remove on one not installed: {shown}");
    // d on an installed one asks, and both answers come back to the settings.
    key(&mut app, KeyCode::Home);
    ch(&mut app, 'd');
    assert!(matches!(&app.mode, Mode::Confirm(Confirm::RemovePlugin { id, .. }) if id == "sea-themes"));
    ch(&mut app, 'n');
    assert!(matches!(app.mode, Mode::Settings), "no goes back to the settings");
    ch(&mut app, 'd');
    ch(&mut app, 'y');
    assert!(matches!(app.mode, Mode::Settings), "and so does yes");
    assert_eq!(app.pl.installed.iter().map(|p| p.id()).collect::<Vec<_>>(), ["dusk"]);
    // It is still in the catalog: listed, available again, still selected.
    assert_eq!(app.setting, 0);
    let shown = draw(&mut term, &app);
    let l = shown.lines().find(|l| l.contains("sea-themes name")).unwrap().to_string();
    assert!(l.contains("▎ sea-themes name") && l.contains(" Available "), "{l}");
    app.on_mouse(ratatui::crossterm::event::MouseEvent {
        kind: ratatui::crossterm::event::MouseEventKind::ScrollDown,
        column: 60,
        row: 20,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.setting, 1);
}

#[test]
fn four_does_nothing_and_the_header_has_three_tabs_even_when_narrow() {
    let (_w, mut app, _) = world("plug-four");
    for width in [60u16, 80, 200] {
        let mut term = Terminal::new(TestBackend::new(width, 24)).unwrap();
        let shown = draw(&mut term, &app);
        let header = shown.lines().nth(1).unwrap().to_string();
        assert!(header.contains("1 Sessions") && !header.contains("Plugins"), "{width}: {header}");
        if width >= 80 {
            assert!(header.contains("3 Squads"), "{width}: {header}");
        }
        if width == 200 {
            assert!(flat(&shown).contains("1-3 tabs"), "{width}: {}", flat(&shown));
        }
    }
    let top = app.top;
    ch(&mut app, '4');
    assert!(matches!(app.mode, Mode::List) && app.top == top && !app.plugin_screen_showing());
    assert!(!app.plugins_busy(), "nothing fetched either");
}

#[test]
fn enter_opens_the_plugin_screen_on_that_plugin_and_back_returns_to_the_settings_on_it() {
    let (_w, mut app, _) = world("plug-screen-back");
    // From a project's screen too: the plugin screen takes the whole body, and back comes back to it all.
    open_settings_plugins(&mut app);
    assert_eq!(app.pl.rows().iter().map(|r| r.manifest.id.as_str()).collect::<Vec<_>>(), ["sea-themes", "dusk"]);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Plugins));
    assert_eq!(app.pl.selected_row().unwrap().manifest.id, "dusk", "on the plugin that was selected");
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("Settings · Plugins") && shown.contains("▎ dusk name") && shown.contains("What dusk is."), "{shown}");
    // `s` does nothing here, nor keys of the list (`4` is no tab).
    for c in ['s', '4', 'v', '/', 'f'] {
        ch(&mut app, c);
        assert!(matches!(app.mode, Mode::Plugins), "{c}");
    }
    // The back key: the settings, on their Plugins tab, the same plugin selected.
    app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
    assert!(matches!(app.mode, Mode::Settings) && !app.plugin_screen_showing());
    assert_eq!((app.settings_tab, app.setting), (crate::settings::TAB_PLUGINS, 1));
    let shown = draw(&mut term, &app);
    assert!(shown.lines().any(|l| l.contains("▎ dusk name")), "{shown}");
    // Moving on the plugin screen moves what comes back selected; esc goes back as well.
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::Settings));
    assert_eq!(app.setting, 0);
    // q is not quit on the plugin screen: it goes back.
    key(&mut app, KeyCode::Enter);
    ch(&mut app, 'q');
    assert!(matches!(app.mode, Mode::Settings) && !app.quit);
    // A click on the selected plugin opens it, like enter; the mouse wheel moves on the plugin screen.
    let _ = draw(&mut term, &app);
    let row = app.hits.borrow().iter().find(|(_, h)| matches!(h, crate::app::Hit::Setting(0))).map(|(r, _)| *r).unwrap();
    let click = |app: &mut App, kind| {
        app.on_mouse(ratatui::crossterm::event::MouseEvent { kind, column: row.x + 2, row: row.y, modifiers: KeyModifiers::NONE });
    };
    click(&mut app, ratatui::crossterm::event::MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left));
    assert!(matches!(app.mode, Mode::Plugins));
    click(&mut app, ratatui::crossterm::event::MouseEventKind::ScrollDown);
    assert_eq!(app.pl.selected, 1);
    // The footer's back is clickable too.
    let _ = draw(&mut term, &app);
    let back = app.hits.borrow().iter().find(|(_, h)| matches!(h, crate::app::Hit::Action(crate::app::Action::Cancel))).map(|(r, _)| *r).unwrap();
    app.on_mouse(ratatui::crossterm::event::MouseEvent { kind: ratatui::crossterm::event::MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left), column: back.x, row: back.y, modifiers: KeyModifiers::NONE });
    assert!(matches!(app.mode, Mode::Settings) && app.setting == 1);
    // Out of the settings: the list, where q quits.
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List));
    ch(&mut app, 'q');
    assert!(app.quit);
}

#[test]
fn the_plugin_screen_opened_over_a_projects_screen_goes_back_to_it() {
    let (_w, mut app, _) = world("plug-over-project");
    app.projects.create("Web shop").unwrap();
    app.reload_for_test();
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Enter);
    assert!(app.open_project.is_some());
    open_tab(&mut app);
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("Settings · Plugins") && !shown.contains("Web shop"), "the plugin screen, not the project behind: {shown}");
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List) && app.open_project.is_some(), "back on the project's screen");
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("Web shop"), "{shown}");
}

// ---- plugins with skills, squads and agents ---------------------------------------------------------------------------

/// A catalog folder holding a plugin with a skill, a squad and two agents (`squadplugin::testing::plugin`).
fn instructions_catalog(base: &std::path::Path) -> std::path::PathBuf {
    let cat = base.join("catalog");
    crate::squadplugin::testing::plugin(&cat, "changelog", "1.0.0");
    list_in_index(&cat, "changelog", "1.0.0");
    cat
}

/// Where the list's pane ends on the Squads tab, and its text, flat.
fn squads_left(text: &str) -> String {
    let at = text.lines().find_map(|l| l.chars().collect::<String>().find("╮╭").map(|at| l[..at].chars().count() + 1)).expect("two panes");
    flat(&text.lines().skip(3).map(|l| l.chars().take(at).collect::<String>()).collect::<Vec<_>>().join("\n"))
}

#[test]
fn a_plugin_with_instructions_is_read_before_it_is_installed_and_its_squad_and_agents_are_read_only_on_the_squads_tab() {
    let w = World::new("plug-review");
    let cat = instructions_catalog(&w.base);
    let mut app = w.app();
    app.pl.use_catalog(from(&cat));
    let mut term = Terminal::new(TestBackend::new(130, 36)).unwrap();
    open_tab(&mut app);
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("enter read and install") && shown.contains("Enter shows each skill, squad and agent it adds"), "{shown}");
    assert!(shown.contains("Adds 1 skill, 1 squad, 2 agents: /changelog-entry, changelog, change-reader, changelog-writer"), "{shown}");

    // Enter: what it adds, each with its permissions, not a plain yes/no.
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::PluginReview));
    let shown = flat(&draw(&mut term, &app));
    for want in [
        "install plugin",
        "Install changelog name?",
        "It adds instructions Claude follows with your permissions. Read each one before you install it",
        "Skill changelog-entry",
        "– Claude may use it by itself when it fits; you can also type /changelog-entry",
        "Squad changelog squad",
        "– 3 steps, done by change-reader (this plugin), changelog-writer (this plugin) and editor",
        "(built in)",
        "Agent change-reader",
        "– Can run commands on this computer",
        "Agent changelog-writer",
        "– Starts with the skill changelog-entry (and can use other skills)",
        "enter read it",
        "y install",
        "n not now",
    ] {
        assert!(shown.contains(want), "{want}: {shown}");
    }
    // At 80 by 24 it still fits: the question, the items as far as there is room, the keys.
    let mut small = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let shown = flat(&draw(&mut small, &app));
    for want in ["Install changelog name?", "Skill changelog-entry", "enter read it", "y install", "n not now"] {
        assert!(shown.contains(want), "{want} at 80x24: {shown}");
    }
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    let shown = flat(&draw(&mut small, &app));
    assert!(shown.contains("▎ Agent changelog-writer"), "the selected item scrolls into view: {shown}");
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Up);
    // Reading one in full: the agent's whole system prompt, on the whole screen.
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.pl.review.as_ref().and_then(|r| r.reading), Some(2));
    let shown = flat(&draw(&mut term, &app));
    for want in ["Agent: change-reader · from changelog name", "This is what Claude will follow, with your permissions:", "Its system prompt", "Read the commits and the diff since the last release", "ctrl+space back to the list"] {
        assert!(shown.contains(want), "{want}: {shown}");
    }
    assert!(!shown.contains("y install"), "the full view only reads: {shown}");
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::PluginReview) && app.pl.review.as_ref().unwrap().reading.is_none());
    // The skill's own words.
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Enter);
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("Skill: changelog-entry") && shown.contains("Do the thing carefully, then say what you did."), "{shown}");
    key(&mut app, KeyCode::Esc);
    // No: nothing is installed.
    key(&mut app, KeyCode::Char('n'));
    assert!(matches!(app.mode, Mode::Plugins) && app.pl.review.is_none(), "back on the plugin screen");
    assert!(app.pl.store.installed().is_empty());
    let skill = w.root.join("skills/changelog-entry/SKILL.md");
    assert!(!skill.exists());
    // Yes: installed, its skill in the (test's) Claude folder, its squad and agents on the Squads tab.
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('y'));
    assert!(matches!(app.mode, Mode::Plugins));
    app.refresh_until_for_test(20, "the install", |a| !a.plugins_busy());
    let notice = app.active_notice().unwrap_or_default().to_string();
    assert!(notice.contains("changelog name is installed: 1 skill, 1 squad, 2 agents added. Its squads are on the Squads tab (3)"), "{notice}");
    assert!(skill.is_file());

    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Char('3'));
    assert!(app.squads_tab_showing());
    let at = app.sqt.list.iter().position(|l| l.id == "plugin:changelog/changelog").expect("the plugin's squad is listed");
    assert_eq!(app.sqt.list[at].label(), "From changelog name");
    for _ in 0..at {
        key(&mut app, KeyCode::Down);
    }
    let shown = draw(&mut term, &app);
    assert!(squads_left(&shown).contains("changelog squad 3 agents From changelog name"), "{}", squads_left(&shown));
    assert!(flat(&shown).contains("From changelog name · read only: c makes a copy of your own to change"), "{}", flat(&shown));
    assert!(!flat(&shown).contains("Add a step"), "read only: {}", flat(&shown));
    // Read only: its name can't be changed; nothing is written.
    key(&mut app, KeyCode::Enter);
    app.notice = None;
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("c copy to change") && !shown.contains("x remove") && !shown.contains("enter change"), "{shown}");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List), "no name box");
    assert_eq!(app.active_notice(), Some("changelog squad comes with the plugin changelog name, so it can't be changed here. c makes a copy of your own to change; the squad stays as the plugin has it."));
    key(&mut app, KeyCode::Char('x'));
    key(&mut app, KeyCode::Char('d'));
    assert!(matches!(app.mode, Mode::List));
    assert!(app.active_notice().unwrap().contains("comes with the plugin changelog name: removing the plugin (in the settings: s, then Plugins) takes it off"), "{:?}", app.active_notice());
    assert!(!crate::squadfile::dir(&w.config).exists(), "nothing written to the user's squads");
    // c: a copy of the user's own, which can be changed.
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Char('c'));
    assert!(matches!(app.mode, Mode::ProjectName));
    key(&mut app, KeyCode::Enter);
    let mine = app.sqt.listed().unwrap().clone();
    assert_eq!((mine.kind, mine.name()), (crate::squadfile::Kind::Own, "changelog squad copy"));
    assert!(crate::squadfile::path(&w.config, &mine.id).is_file());
    assert_eq!(mine.squad.as_ref().unwrap().agent_ids(), ["plugin:changelog/change-reader", "plugin:changelog/changelog-writer", "editor"], "the copy uses the plugin's agents");
    // The Agents list: the plugin's agents under its name, read only.
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Right);
    let at = app.sqt.agents.iter().position(|l| l.id == "plugin:changelog/change-reader").unwrap();
    for _ in 0..at {
        key(&mut app, KeyCode::Down);
    }
    let shown = draw(&mut term, &app);
    let left = squads_left(&shown);
    assert!(left.contains("From changelog name ▎ change-reader Sonnet"), "{left}");
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Char(' '));
    assert!(app.active_notice().unwrap().starts_with("change-reader comes with the plugin changelog name, so it can't be changed here."), "{:?}", app.active_notice());
    assert!(!crate::agentfile::dir(&w.config).exists(), "nothing written to the user's agents");

    // Removing the plugin: asked, naming the user's squad that uses its agents; afterwards that squad needs fixing.
    key(&mut app, KeyCode::Esc);
    open_tab(&mut app);
    ch(&mut app, 'd');
    let Mode::Confirm(Confirm::RemovePlugin { extra, .. }) = &app.mode else { panic!("asked first") };
    assert!(extra.contains("Its skills leave Claude Code's skills folder") && extra.contains("Tickets its squads already have keep working") && extra.contains("Your squad changelog squad copy uses its agents"), "{extra}");
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("Remove changelog name?") && shown.contains("Your squad changelog squad copy uses its agents"), "{shown}");
    ch(&mut app, 'y');
    assert_eq!(app.active_notice(), Some("Removed changelog name."));
    assert!(!skill.exists(), "its untouched skill went with it");
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Char('3'));
    assert!(app.sqt.list.iter().all(|l| l.kind != crate::squadfile::Kind::Plugin));
    key(&mut app, KeyCode::Left);
    let shown = draw(&mut term, &app);
    assert!(squads_left(&shown).contains("changelog squad copy 3 agents Your own · needs fixing"), "{}", squads_left(&shown));
}

#[test]
fn a_plugin_squad_is_offered_for_a_ticket_and_its_leader_keeps_its_snapshot_after_the_plugin_is_removed() {
    use crate::tickets::{NewTicket, Tickets};
    let w = World::new("plug-squad-ticket");
    let s = crate::settings::Settings { tokens_enabled: true, tokens_asked: true, auto_archive_days: 0, squads_at_once: 1, ..Default::default() };
    crate::settings::save(&w.config, &s).unwrap();
    w.transcript("user-1", "/tmp", "make a basket page", 10);
    let src = crate::squadplugin::testing::plugin(&w.base.join("src"), "changelog", "1.0.0");
    crate::plugins::store_for(&w.config).install(&src, crate::plugins::Origin::Folder { path: src.to_string_lossy().into_owned() }).unwrap();
    let mut app = w.app();
    let fake = crate::testkit::AiFake::new("plug-squad-ticket");
    fake.reply_default(r#"{"fits":true,"reason":"It fits."}"#);
    app.set_ai_program_for_test(&fake.script);
    let id = app.projects.create("Web shop").unwrap();
    app.projects.assign("user-1", Some(&id)).unwrap();
    let mut t = Tickets::open(&w.config, &id);
    for n in 1..=3 {
        t.add(NewTicket { title: format!("Ticket {n}"), ..Default::default() }).unwrap();
    }
    app.reload_for_test();
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Tab);
    let hand = |app: &mut App, n: u32| {
        let at = app.ticket_rows().iter().position(|r| *r == crate::app::TRow::Ticket(n)).unwrap();
        app.tk.selected = Some(app.ticket_rows()[at]);
        ch(app, 'a');
        assert!(matches!(app.mode, Mode::SquadPick), "{:?}", app.active_notice());
        let to = app.sq.pick.iter().position(|s| s.id == "plugin:changelog/changelog").expect("the plugin's squad is offered");
        for _ in 0..to {
            key(app, KeyCode::Down);
        }
        key(app, KeyCode::Enter);
        app.refresh_until_for_test(20, "the commander's answer", |a| !a.squads_busy());
    };
    let mut term = Terminal::new(TestBackend::new(130, 36)).unwrap();
    // The picker says which plugin the squad comes with.
    let at = app.ticket_rows().iter().position(|r| *r == crate::app::TRow::Ticket(1)).unwrap();
    app.tk.selected = Some(app.ticket_rows()[at]);
    ch(&mut app, 'a');
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("changelog squad") && shown.contains("From changelog name. Writes the changelog entry for a release."), "{shown}");
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List));
    hand(&mut app, 1);
    assert_eq!(app.sq.launched, [1]);
    let (_, args) = app.sq.launched_with[0].clone();
    let after = |flag: &str| args[args.iter().position(|a| a == flag).unwrap() + 1].clone();
    assert_eq!(after("--agent"), "changelog-leader");
    assert_eq!(after("-n"), "changelog squad squad · #1 Ticket 1");
    let agents: serde_json::Value = serde_json::from_str(&after("--agents")).unwrap();
    assert!(agents.get("changelog-change-reader").is_some() && agents.get("changelog-editor").is_some(), "{agents}");
    // #2 waits in the line for the same squad (one squad at a time): a copy of the squad is kept beside it.
    hand(&mut app, 2);
    assert_eq!(app.sq.launched, [1]);
    let kept = crate::tickets::bodies_dir(&w.config, &id).join(crate::tickets::queued_crew_file(2));
    assert!(kept.is_file());
    let _ = draw(&mut term, &app);

    // The plugin goes while #1 works and #2 waits.
    crate::plugins::store_for(&w.config).remove("changelog").unwrap();
    app.refresh_for_test();
    assert!(app.sqt.list.iter().all(|l| l.kind != crate::squadfile::Kind::Plugin));
    // #1's leader continues from its snapshot, exactly as it started.
    let t1 = Tickets::open(&w.config, &id).get(1).cloned().unwrap();
    let launch = app.leader_launch(&id, t1.leader().unwrap(), &t1).expect("its snapshot is all it needs");
    assert_eq!((launch.agent.as_str(), launch.agents.as_str()), ("changelog-leader", after("--agents").as_str()));
    // #2 starts when the place frees, with the squad it waited for.
    Tickets::open(&w.config, &id).squad_says(1, &crate::tickets::State::Done, "", Some("Written.")).unwrap();
    app.refresh_until_for_test(20, "#2 to start", |a| a.sq.launched.len() == 2);
    let t2 = Tickets::open(&w.config, &id).get(2).cloned().unwrap();
    assert_eq!(t2.leaders[0].squad, "plugin:changelog/changelog");
    let (_, args2) = app.sq.launched_with[1].clone();
    let agents2 = &args2[args2.iter().position(|a| a == "--agents").unwrap() + 1];
    let v2: serde_json::Value = serde_json::from_str(agents2).unwrap();
    assert!(v2.get("changelog-change-reader").is_some() && v2.get("changelog-changelog-writer").is_some(), "{v2}");
    // A new hand-over can't pick it any more.
    ch(&mut app, 'a');
    if matches!(app.mode, Mode::SquadPick) {
        assert!(app.sq.pick.iter().all(|s| s.id != "plugin:changelog/changelog"));
        key(&mut app, KeyCode::Esc);
    }
}

#[test]
fn an_update_that_changes_an_agents_instructions_is_shown_again_with_the_changes_marked() {
    let w = World::new("plug-review-update");
    let cat = instructions_catalog(&w.base);
    let mut app = w.app();
    app.pl.use_catalog(from(&cat));
    open_tab(&mut app);
    key(&mut app, KeyCode::Enter);
    ch(&mut app, 'y');
    app.refresh_until_for_test(20, "the install", |a| !a.plugins_busy());
    assert!(app.pl.store.get("changelog").is_some());
    // 1.1.0 changes the reader's instructions.
    bump(&cat, "changelog", "1.1.0");
    let reader = cat.join("changelog/agents/change-reader.json");
    std::fs::write(&reader, std::fs::read_to_string(&reader).unwrap().replace("change nothing", "change nothing, not even a tag")).unwrap();
    app.pl.use_catalog(from(&cat));
    assert_eq!(app.pl.rows()[0].state, PlugState::Update);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::PluginReview), "asked again, with what changed: {:?}", app.active_notice());
    let mut term = Terminal::new(TestBackend::new(130, 36)).unwrap();
    let shown = flat(&draw(&mut term, &app));
    for want in ["update plugin", "Update changelog name from 1.0.0 to 1.1.0?", "Its instructions changed. Read the ones marked new or changed", "Agent change-reader · changed", "y update"] {
        assert!(shown.contains(want), "{want}: {shown}");
    }
    assert!(!shown.contains("changelog-writer · changed"), "only what changed is marked: {shown}");
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("Changed in this version. This is what Claude will follow:") && shown.contains("not even a tag"), "{shown}");
    key(&mut app, KeyCode::Esc);
    ch(&mut app, 'y');
    app.refresh_until_for_test(20, "the update", |a| !a.plugins_busy());
    assert!(app.active_notice().unwrap().contains("changelog name is updated from 1.0.0 to 1.1.0"), "{:?}", app.active_notice());
    assert!(crate::agentfile::get(&w.config, "plugin:changelog/change-reader").unwrap().guidance.contains("not even a tag"));
}

#[test]
fn the_back_key_goes_from_a_plugins_file_to_the_list_of_what_it_adds_and_then_closes_it() {
    let w = World::new("plug-review-back");
    let cat = instructions_catalog(&w.base);
    let mut app = w.app();
    app.pl.use_catalog(from(&cat));
    open_tab(&mut app);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::PluginReview));
    let ctrl_space = |app: &mut App| app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.pl.review.as_ref().and_then(|r| r.reading), Some(0));
    // (Not the space that pages down in the reader.)
    ctrl_space(&mut app);
    assert!(matches!(app.mode, Mode::PluginReview) && app.pl.review.as_ref().unwrap().reading.is_none(), "the file -> the list");
    ctrl_space(&mut app);
    assert!(matches!(app.mode, Mode::Plugins) && app.pl.review.is_none(), "the list -> closed, nothing installed");
    assert!(app.pl.store.installed().is_empty());
    // The plugin screen -> the settings -> the list, where q quits.
    ctrl_space(&mut app);
    assert!(matches!(app.mode, Mode::Settings));
    ctrl_space(&mut app);
    assert!(matches!(app.mode, Mode::List));
    ch(&mut app, 'q');
    assert!(app.quit);
}

#[test]
fn one_two_three_on_the_plugin_screen_go_straight_to_a_tab_and_no_hint_says_so() {
    let (_w, mut app, _) = world("plug-digits");
    app.projects.create("Web shop").unwrap();
    app.reload_for_test();
    let mut term = Terminal::new(TestBackend::new(130, 30)).unwrap();
    for (c, top) in [('1', 0), ('2', 1), ('3', 2)] {
        open_tab(&mut app);
        let shown = flat(&draw(&mut term, &app));
        assert!(!shown.contains("1-3") && !shown.contains("tabs"), "no hint for them: {shown}");
        ch(&mut app, c);
        assert!(matches!(app.mode, Mode::List) && !app.plugin_screen_showing(), "{c}: the plugin screen and the settings are closed");
        assert_eq!(app.top, top, "{c}");
        let shown = flat(&draw(&mut term, &app));
        assert!(!shown.contains("Settings · Plugins") && !shown.contains("Settings file:"), "{c}: {shown}");
    }
    // From over a project's screen: the tab, not the project.
    ch(&mut app, '2');
    key(&mut app, KeyCode::Enter);
    assert!(app.open_project.is_some());
    open_tab(&mut app);
    ch(&mut app, '1');
    assert!(matches!(app.mode, Mode::List) && app.top == 0 && app.open_project.is_none());
}

#[test]
fn the_settings_show_the_selected_plugins_description_cut_to_a_few_lines() {
    let w = World::new("plug-description");
    let cat = catalog_dir(&w.base, &[("sea-themes", "1.0.0", &[("Deep Water", GOOD_THEME)]), ("dusk", "0.3.0", &[("Dusk", DUSK)])]);
    // A long description for dusk: more than three lines at this width.
    let path = cat.join("dusk/plugin.json");
    let long = "Dusk has a long story to tell. ".repeat(12);
    let mut m: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    m.as_object_mut().unwrap().insert("description".into(), serde_json::json!(long.trim()));
    std::fs::write(&path, m.to_string()).unwrap();
    let mut app = w.app();
    app.pl.use_catalog(from(&cat));
    open_settings_plugins(&mut app);
    let mut term = Terminal::new(TestBackend::new(100, 40)).unwrap();
    let shown = draw(&mut term, &app);
    assert!(flat(&shown).contains("What sea-themes is."), "{shown}");
    let at = |text: &str, what: &str| text.lines().position(|l| l.contains(what)).unwrap_or_else(|| panic!("{what}: {text}"));
    assert!(at(&shown, "What sea-themes is.") > at(&shown, "dusk name"), "under the list: {shown}");
    let height = shown.lines().filter(|l| l.contains('│')).count();
    key(&mut app, KeyCode::Down);
    let shown = draw(&mut term, &app);
    assert!(!shown.contains("What sea-themes is."));
    let told: Vec<&str> = shown.lines().filter(|l| l.contains("Dusk has a long story")).collect();
    assert_eq!(told.len(), 3, "three lines at most: {shown}");
    assert!(told[2].trim_end_matches(['│', ' ']).ends_with('…'), "cut with an ellipsis: {}", told[2]);
    assert_eq!(shown.lines().filter(|l| l.contains('│')).count(), height, "the popup keeps its size");
    assert!(flat(&shown).contains("Enter opens a plugin"), "{shown}");
}

/// A world whose app starts with plugins installed from its catalog, then a newer platoon (a newer catalog) starts.
fn installed_then_newer(name: &str) -> (World, std::path::PathBuf) {
    let w = World::new(name);
    let cat = catalog_dir(&w.base, &[("sea-themes", "1.0.0", &[("Deep Water", GOOD_THEME)]), ("dusk", "0.3.0", &[("Dusk", DUSK)])]);
    crate::squadplugin::testing::plugin(&cat, "changelog", "1.0.0");
    list_in_index(&cat, "changelog", "1.0.0");
    let store = crate::plugins::store_for(&w.config);
    let c = crate::plugins::load_catalog(&from(&cat));
    for id in ["sea-themes", "dusk", "changelog"] {
        store.install(c.get(id).unwrap().unpack().unwrap().dir(), crate::plugins::Origin::Catalog).unwrap();
    }
    // One of the user's own, from a folder: never touched.
    let mine = crate::plugins::testing::plugin(&w.base.join("mine"), "moss", "1.0.0", &[("Moss", GOOD_THEME)]);
    store.install(&mine, crate::plugins::Origin::Folder { path: "/home/u/moss".into() }).unwrap();
    (w, cat)
}

/// The app as it starts with catalog `cat` (standing in for the one built into it).
fn start(w: &World, cat: &std::path::Path) -> App {
    let mut app = w.app();
    app.pl.use_catalog(from(cat));
    app.update_with_platoon();
    app.refresh_until_for_test(20, "the updates at start", |a| !a.plugins_busy());
    app
}

#[test]
fn a_newer_platoon_updates_its_theme_plugins_quietly_says_so_once_and_asks_about_one_with_instructions() {
    let (w, cat) = installed_then_newer("plug-auto");
    // Nothing newer: nothing done, nothing said, nothing asked.
    let app = start(&w, &cat);
    assert!(matches!(app.mode, Mode::List) && app.active_notice().is_none(), "{:?}", app.active_notice());
    drop(app);
    // A newer platoon: sea-themes and the instructions plugin are newer; moss (from a folder) too, but it is the user's.
    bump(&cat, "sea-themes", "1.1.0");
    bump(&cat, "changelog", "1.1.0");
    crate::plugins::testing::plugin(&cat, "moss", "9.0.0", &[("Moss", GOOD_THEME)]);
    list_in_index(&cat, "moss", "9.0.0");
    let reader = cat.join("changelog/agents/change-reader.json");
    std::fs::write(&reader, std::fs::read_to_string(&reader).unwrap().replace("change nothing", "change nothing, not even a tag")).unwrap();
    let mut app = start(&w, &cat);
    assert_eq!(app.active_notice(), Some("Updated with platoon: sea-themes name 1.1.0."), "one line");
    let store = crate::plugins::store_for(&w.config);
    assert_eq!(store.get("sea-themes").unwrap().manifest.version, "1.1.0");
    assert_eq!(store.get("dusk").unwrap().manifest.version, "0.3.0");
    assert_eq!(store.get("moss").unwrap().manifest.version, "1.0.0", "a folder's plugin is never touched");
    // The one with instructions: shown, with what changed, and asked.
    assert!(matches!(app.mode, Mode::PluginReview), "{:?}", app.active_notice());
    let mut term = Terminal::new(TestBackend::new(130, 36)).unwrap();
    let shown = flat(&draw(&mut term, &app));
    for want in ["Update changelog name from 1.0.0 to 1.1.0?", "Agent change-reader · changed", "y update", "n not now"] {
        assert!(shown.contains(want), "{want}: {shown}");
    }
    assert_eq!(store.get("changelog").unwrap().manifest.version, "1.0.0", "nothing until asked");
    // No: it stays, and that version isn't asked about again; the settings still say an update is ready.
    ch(&mut app, 'n');
    assert!(matches!(app.mode, Mode::List));
    assert!(app.active_notice().unwrap().starts_with("changelog name stays at 1.0.0; platoon won't ask about 1.1.0 again."), "{:?}", app.active_notice());
    assert_eq!(store.get("changelog").unwrap().manifest.version, "1.0.0");
    let row = app.pl.rows().into_iter().find(|r| r.manifest.id == "changelog").unwrap();
    assert_eq!(row.state, PlugState::Update);
    assert_eq!(row.state.short_words(), "Update ready");
    drop(app);
    let app = start(&w, &cat);
    assert!(matches!(app.mode, Mode::List) && app.active_notice().is_none(), "not asked again, nothing said: {:?}", app.active_notice());
    drop(app);
    // A still newer version asks again, and yes updates it.
    bump(&cat, "changelog", "1.2.0");
    let mut app = start(&w, &cat);
    assert!(matches!(app.mode, Mode::PluginReview));
    assert!(flat(&draw(&mut term, &app)).contains("Update changelog name from 1.0.0 to 1.2.0?"));
    ch(&mut app, 'y');
    app.refresh_until_for_test(20, "the update", |a| !a.plugins_busy());
    assert!(app.active_notice().unwrap().contains("changelog name is updated from 1.0.0 to 1.2.0"), "{:?}", app.active_notice());
    assert!(crate::agentfile::get(&w.config, "plugin:changelog/change-reader").unwrap().guidance.contains("not even a tag"));
    drop(app);
    // Everything up to date: nothing said, nothing asked.
    let app = start(&w, &cat);
    assert!(matches!(app.mode, Mode::List) && app.active_notice().is_none(), "{:?}", app.active_notice());
}

#[test]
fn a_question_at_start_waits_for_the_list_and_esc_is_a_no() {
    let (w, cat) = installed_then_newer("plug-auto-wait");
    bump(&cat, "changelog", "1.1.0");
    let mut app = w.app();
    app.pl.use_catalog(from(&cat));
    // The user is in the settings when the question comes: it waits.
    ch(&mut app, 's');
    app.update_with_platoon();
    app.refresh_for_test();
    assert!(matches!(app.mode, Mode::Settings));
    key(&mut app, KeyCode::Esc);
    app.refresh_for_test();
    assert!(matches!(app.mode, Mode::PluginReview), "asked once the list shows");
    // Esc (or the back key) is no, too.
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(crate::plugins::store_for(&w.config).get("changelog").unwrap().declined.as_deref(), Some("1.1.0"));
}

#[test]
fn the_old_catalog_an_earlier_platoon_fetched_goes_when_the_app_starts() {
    let w = World::new("plug-old-catalog");
    let store = crate::plugins::store_for(&w.config);
    std::fs::create_dir_all(store.dir.join(".catalog/editor")).unwrap();
    std::fs::write(store.dir.join(".catalog.json"), "{}").unwrap();
    std::fs::write(store.dir.join("keep-me"), "x").unwrap();
    let _app = w.app();
    assert!(!store.dir.join(".catalog").exists() && !store.dir.join(".catalog.json").exists());
    assert!(store.dir.join("keep-me").exists());
}
