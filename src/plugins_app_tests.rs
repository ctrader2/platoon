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


// ---- plugins with skills, squads and agents ---------------------------------------------------------------------------

/// A catalog repository holding the plugin with a skill, a squad and two agents (`squadplugin::testing::plugin`).
fn instructions_repo(base: &std::path::Path) -> std::path::PathBuf {
    let repo = base.join("source");
    let cat = repo.join(crate::plugins::CATALOG_FOLDER);
    crate::squadplugin::testing::plugin(&cat, "changelog", "1.0.0");
    std::fs::write(cat.join(crate::plugins::INDEX), serde_json::json!({"plugins": [{"id": "changelog", "version": "1.0.0"}]}).to_string()).unwrap();
    crate::plugins::testing::git(&repo, &["init", "--quiet"]);
    crate::plugins::testing::git(&repo, &["add", "-A"]);
    crate::plugins::testing::git(&repo, &["commit", "--quiet", "-m", "catalog"]);
    repo
}

/// Where the list's pane ends on the Squads tab, and its text, flat.
fn squads_left(text: &str) -> String {
    let at = text.lines().find_map(|l| l.chars().collect::<String>().find("╮╭").map(|at| l[..at].chars().count() + 1)).expect("two panes");
    flat(&text.lines().skip(3).map(|l| l.chars().take(at).collect::<String>()).collect::<Vec<_>>().join("\n"))
}

#[test]
fn a_plugin_with_instructions_is_read_before_it_is_installed_and_its_squad_and_agents_are_read_only_on_the_squads_tab() {
    let w = World::new("plug-review");
    let repo = instructions_repo(&w.base);
    let mut app = w.app();
    app.pl.source = source(&repo);
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
        "– 3 steps, done by change-reader (this plugin), changelog-writer (this plugin) and editor (built in)",
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
    for want in ["Agent: change-reader · from changelog name", "This is what Claude will follow, with your permissions:", "Its system prompt", "Read the commits and the diff since the last release", "esc back to the list"] {
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
    assert!(matches!(app.mode, Mode::List) && app.pl.review.is_none());
    assert!(app.pl.store.installed().is_empty());
    let skill = w.root.join("skills/changelog-entry/SKILL.md");
    assert!(!skill.exists());
    // Yes: installed, its skill in the (test's) Claude folder, its squad and agents on the Squads tab.
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('y'));
    assert!(matches!(app.mode, Mode::List));
    app.refresh_until_for_test(20, "the install", |a| !a.plugins_busy());
    let notice = app.active_notice().unwrap_or_default().to_string();
    assert!(notice.contains("changelog name is installed: 1 skill, 1 squad, 2 agents added. Its squads are on the Squads tab (3)"), "{notice}");
    assert!(skill.is_file());

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
    assert!(app.active_notice().unwrap().contains("comes with the plugin changelog name: removing the plugin (the Plugins tab, 4) takes it off"), "{:?}", app.active_notice());
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
    key(&mut app, KeyCode::Char('4'));
    ch(&mut app, 'd');
    let Mode::Confirm(Confirm::RemovePlugin { extra, .. }) = &app.mode else { panic!("asked first") };
    assert!(extra.contains("Its skills leave Claude Code's skills folder") && extra.contains("Tickets its squads already have keep working") && extra.contains("Your squad changelog squad copy uses its agents"), "{extra}");
    let shown = flat(&draw(&mut term, &app));
    assert!(shown.contains("Remove changelog name?") && shown.contains("Your squad changelog squad copy uses its agents"), "{shown}");
    ch(&mut app, 'y');
    assert_eq!(app.active_notice(), Some("Removed changelog name."));
    assert!(!skill.exists(), "its untouched skill went with it");
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
    let repo = instructions_repo(&w.base);
    let mut app = w.app();
    app.pl.source = source(&repo);
    open_tab(&mut app);
    key(&mut app, KeyCode::Enter);
    ch(&mut app, 'y');
    app.refresh_until_for_test(20, "the install", |a| !a.plugins_busy());
    assert!(app.pl.store.get("changelog").is_some());
    // 1.1.0 changes the reader's instructions.
    let cat = repo.join(crate::plugins::CATALOG_FOLDER);
    for f in ["changelog/plugin.json", "index.json"] {
        let p = cat.join(f);
        std::fs::write(&p, std::fs::read_to_string(&p).unwrap().replace("1.0.0", "1.1.0")).unwrap();
    }
    let reader = cat.join("changelog/agents/change-reader.json");
    std::fs::write(&reader, std::fs::read_to_string(&reader).unwrap().replace("change nothing", "change nothing, not even a tag")).unwrap();
    crate::plugins::testing::git(&repo, &["commit", "--quiet", "-am", "1.1.0"]);
    ch(&mut app, 'f');
    app.refresh_until_for_test(20, "the catalog", |a| !a.plugins_busy());
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
