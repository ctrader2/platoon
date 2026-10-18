//! The built-in themes kept as theme files (`src/themes/`, once the Theme pack plugin) and light themes at the level of
//! the app: each of the twenty is exactly what the plugin made of it (`testdata/theme_pack_colors.tsv`, written by
//! the plugin route before it was removed), and reads well as it is, in 24-bit and in 256 colors; the screens drawn
//! in a light theme have the colors they should, cell by cell (the Sessions list with a selected row and a project
//! badge, a popup, the settings, a project's notes, the attached view's footer, a brief); a saved theme
//! id nobody has any more falls back to the default. Every folder is a temp one (`World`).

use crate::app::{Mode, Pane};
use crate::client::Client;
use crate::colordepth::rgb_to_256;
use crate::testkit::World;
use crate::theme::{MIN_DELTA_E, MIN_DIM, MIN_ROLE, MIN_SUBTEXT, MIN_TEXT, SELECTION_TEXT_CONTRAST, Theme, contrast, delta_e};
use crate::{ui, usertheme};
use ratatui::{
    Terminal,
    backend::TestBackend,
    buffer::Cell,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    style::Color,
};
use std::path::{Path, PathBuf};

/// The twenty, in the order the picker shows them.
const FILES: [&str; 20] = [
    "Harbor Night",
    "Moss Lantern",
    "Ayu Dark",
    "Doom One",
    "Embark",
    "Everblush",
    "GitHub Dark Dimmed",
    "Iceberg Dark",
    "Jellybeans",
    "Srcery",
    "Tomorrow Night",
    "Zenbones Dark",
    "Catppuccin Latte",
    "Dayfox",
    "Flexoki Light",
    "GitHub Light",
    "Gruvbox Light",
    "One Half Light",
    "Rosé Pine Dawn",
    "Solarized Light",
];
const LIGHT: [&str; 8] =
    ["Catppuccin Latte", "Dayfox", "Flexoki Light", "GitHub Light", "Gruvbox Light", "One Half Light", "Rosé Pine Dawn", "Solarized Light"];

fn themes_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/themes")
}

/// Every theme kept as a file, as platoon has it: (name, what the file says, the theme).
fn file_themes() -> Vec<(String, usertheme::Parsed, Theme)> {
    FILES
        .iter()
        .map(|name| {
            let text = std::fs::read_to_string(themes_dir().join(name)).unwrap();
            let parsed = usertheme::parse(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
            let theme = Theme::by_id(&usertheme::slug(name)).unwrap_or_else(|| panic!("{name} is a built-in"));
            assert_eq!(theme.name, *name);
            (name.to_string(), parsed, theme)
        })
        .collect()
}

/// The RGB an xterm shows for one of the 256 colors (16 and up).
fn xterm(i: u8) -> Color {
    let cube = [0u8, 95, 135, 175, 215, 255];
    match i {
        16..=231 => Color::Rgb(cube[usize::from((i - 16) / 36)], cube[usize::from((i - 16) / 6 % 6)], cube[usize::from((i - 16) % 6)]),
        232..=255 => Color::Rgb(8 + 10 * (i - 232), 8 + 10 * (i - 232), 8 + 10 * (i - 232)),
        _ => panic!("platoon never draws with the terminal's own 16 colors in a fixed theme ({i})"),
    }
}

/// What a color looks like after the 256-color pass.
fn in_256(c: Color) -> Color {
    match c {
        Color::Rgb(r, g, b) => xterm(rgb_to_256(r, g, b)),
        Color::Indexed(i) => xterm(i),
        other => other,
    }
}

#[test]
fn the_twenty_are_built_ins_after_the_others_dark_then_light_before_terminal_colors() {
    let ids: Vec<String> = Theme::all().into_iter().map(|t| t.id).collect();
    let want: Vec<String> = FILES.iter().map(|n| usertheme::slug(n)).collect();
    let at = ids.iter().position(|i| *i == want[0]).unwrap();
    assert_eq!(ids[at..at + 20], want[..], "together, in order");
    assert_eq!(ids[at + 20], "terminal", "Terminal colors stays last");
    assert!(ids.contains(&"catppuccin-latte".to_string()) && ids.contains(&"rose-pine-dawn".to_string()));
    let all = file_themes();
    let light: Vec<&str> = all.iter().filter(|(_, _, t)| t.light).map(|(n, _, _)| n.as_str()).collect();
    assert_eq!(light, LIGHT.to_vec(), "the light ones, after the dark ones");
    // None looks like one of the other built-ins under another name: the background, the text and the five colors
    // platoon uses (Lab distances added up) are apart from every one's: Tokyo Night Moon, left out, is 48 from Tokyo
    // Night Storm; the closest kept, Everblush and Doom One to One Dark, about 70.
    let others: Vec<Theme> = Theme::all().into_iter().filter(|b| b.ansi.is_some() && !FILES.contains(&b.name.as_str())).collect();
    let looks = |t: &Theme| [t.bg, t.text, t.accent, t.busy, t.ok, t.warn, t.err];
    for (name, _, t) in &all {
        assert!(others.iter().all(|b| b.name.to_lowercase() != name.to_lowercase()), "{name} is another built-in's name");
        for b in &others {
            let apart: f64 = looks(t).iter().zip(looks(b)).map(|(x, y)| delta_e(*x, y)).sum();
            assert!(apart >= 60.0, "{name} looks like {} ({apart:.0})", b.name);
        }
    }
    // Where the colors come from is said in every file.
    for name in FILES {
        let text = std::fs::read_to_string(themes_dir().join(name)).unwrap();
        assert!(text.starts_with("# ") && text.lines().next().unwrap().contains("License: "), "{name} names its origin and license");
    }
    let on_disk = std::fs::read_dir(themes_dir()).unwrap().count();
    assert_eq!(on_disk, 20, "every file in src/themes is built in");
}

/// The colors the Theme pack plugin gave each of its themes, written down from the plugin route before it went: every
/// role, the sixteen session colors, the badge text and whether it is light, all the same now that they are built in.
/// One change since, on purpose (2026-10-03): the selection fill is held to `MIN_SURFACE` (it was at 1.08:1 and up),
/// so `surface`, and where it had to move to stay apart from it `border`, are the floor's; and dim is no longer lifted
/// for the selected row (that is done where it is drawn), which moved Doom One's dim by one step.
#[test]
fn each_built_in_has_exactly_the_colors_the_theme_pack_plugin_gave_it() {
    let hex = |c: Color| match c {
        Color::Rgb(r, g, b) => format!("{r:02x}{g:02x}{b:02x}"),
        other => format!("{other:?}"),
    };
    let fixture = include_str!("testdata/theme_pack_colors.tsv");
    let mut seen = Vec::new();
    for line in fixture.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        assert_eq!(f.len(), 6, "{line}");
        let old_id = f[0];
        let id = old_id.strip_prefix("plugin:theme-pack/").unwrap();
        let t = Theme::by_id(id).unwrap_or_else(|| panic!("{id} is a built-in"));
        let roles: Vec<String> = t.roles().iter().map(|(_, c)| hex(*c)).collect();
        let ansi: Vec<String> = t.ansi.unwrap().iter().map(|c| hex(*c)).collect();
        assert_eq!(
            (t.name.as_str(), t.light.to_string(), hex(t.on_accent), roles.join(" "), ansi.join(" ")),
            (f[1], f[2].to_string(), f[3].to_string(), f[4].to_string(), f[5].to_string()),
            "{id}"
        );
        seen.push(id.to_string());
    }
    let want: Vec<String> = FILES.iter().map(|n| usertheme::slug(n)).collect();
    assert_eq!(seen, want, "all twenty, in the plugin's order");
}

#[test]
fn a_saved_theme_pack_id_is_the_built_in_now() {
    let w = World::new("light-pack-id");
    w.transcript("aaa", "/srv/alpha", "refactor the parser", 40);
    let settings = w.config.join("settings.json");
    std::fs::write(&settings, r#"{"theme": "plugin:theme-pack/rose-pine-dawn", "idle_stop_minutes": 30}"#).unwrap();
    assert_eq!(crate::settings::load(&w.config).0.theme, "rose-pine-dawn");
    let app = w.app();
    assert_eq!((app.theme.id.as_str(), app.theme.name.as_str()), ("rose-pine-dawn", "Rosé Pine Dawn"));
    // Another plugin's theme, or a pack id that never was, is left as it was written (and shows as the default).
    std::fs::write(&settings, r#"{"theme": "plugin:other/rose-pine-dawn"}"#).unwrap();
    assert_eq!(crate::settings::load(&w.config).0.theme, "plugin:other/rose-pine-dawn");
}

#[test]
fn every_theme_file_reads_well_as_it_is_and_after_the_derivation() {
    for (name, parsed, t) in file_themes() {
        assert!(usertheme::problems(&parsed).is_empty(), "{name}: {:?}", usertheme::problems(&parsed));
        // What the file says is what platoon uses: nothing needed lifting.
        let picks = parsed.role_picks();
        assert_eq!([t.accent, t.busy, t.ok, t.warn, t.err].map(Some), picks, "{name}: the roles are the file's own, untouched");
        assert_eq!(t.text, parsed.foreground, "{name}: the text is the file's own");
        let on = |c: Color| contrast(c, t.bg);
        assert!(on(t.text) >= MIN_TEXT && on(t.subtext) >= MIN_SUBTEXT && on(t.dim) >= MIN_DIM, "{name}");
        assert!(on(t.text) > on(t.subtext) && on(t.subtext) > on(t.dim), "{name}: text over subtext over dim");
        for c in [t.accent, t.busy, t.ok, t.warn, t.err] {
            assert!(on(c) >= MIN_ROLE, "{name}: {c:?}");
        }
        let set = [t.accent, t.busy, t.ok, t.warn, t.err];
        for (i, a) in set.iter().enumerate() {
            for b in &set[i + 1..] {
                assert!(delta_e(*a, *b) >= MIN_DELTA_E, "{name}: {a:?} and {b:?}");
            }
        }
        // The same rules as the built-ins for what is drawn on what.
        assert!(
            contrast(t.surface, t.bg) >= crate::theme::MIN_SURFACE
                && contrast(t.border, t.bg) >= 1.25
                && contrast(t.border, t.surface) >= crate::theme::MIN_BORDER_ON_SURFACE,
            "{name}: surface and border"
        );
        // The text reads on a selected row as it is; dim and subtext are kept at their floors there where they are drawn.
        assert!(contrast(t.text, t.surface) >= MIN_SUBTEXT, "{name}: text on a selected row");
        assert!(contrast(t.readable_on(t.dim, t.surface, MIN_DIM), t.surface) >= MIN_DIM, "{name}: dim on a selected row");
        assert!(contrast(t.on_accent, t.accent) >= 4.5, "{name}: the count badge");
        let sel = t.selection_bg().unwrap();
        assert!(contrast(t.text, sel) >= SELECTION_TEXT_CONTRAST && delta_e(sel, t.bg) >= 15.0, "{name}: selected text");
        // Light or dark, the way round everything goes follows the background.
        if t.light {
            assert!(
                crate::theme::luminance(t.surface) < crate::theme::luminance(t.bg),
                "{name}: a light theme's selected row is a little darker"
            );
        }
    }
}

#[test]
fn every_theme_file_still_reads_in_256_colors() {
    for (name, _, t) in file_themes() {
        let q = |c: Color| in_256(c);
        let bg = q(t.bg);
        assert_ne!(q(t.surface), bg, "{name}: the selected row stands out");
        assert_ne!(q(t.selection_bg().unwrap()), bg, "{name}: selected text stands out");
        // 256 colors are coarse, and a light background becomes the grey ramp's lightest (#eeeeee): a little is
        // lost, as for the built-ins (whose lowest role is 3.7:1 there), but everything still reads.
        assert!(contrast(q(t.text), bg) >= 5.0, "{name}: text {:.2}", contrast(q(t.text), bg));
        // (4.1:1 at the least, Rosé Pine Dawn: the clearer selection fill costs a little here, like selected text.)
        assert!(contrast(q(t.text), q(t.surface)) >= 4.0, "{name}: text on the selected row");
        assert!(contrast(q(t.text), q(t.selection_bg().unwrap())) >= 4.0, "{name}: text on selected text");
        assert!(contrast(q(t.dim), bg) >= 2.5, "{name}: dim {:.2}", contrast(q(t.dim), bg));
        for c in [t.accent, t.busy, t.ok, t.warn, t.err] {
            assert!(contrast(q(c), bg) >= 3.5, "{name}: {c:?} is {:.2} in 256 colors", contrast(q(c), bg));
        }
        let set = [t.accent, t.busy, t.ok, t.warn, t.err].map(q);
        for (i, a) in set.iter().enumerate() {
            assert!(set[i + 1..].iter().all(|b| a != b), "{name}: two roles became one color in 256 colors");
        }
        assert!(contrast(q(t.on_accent), q(t.accent)) >= 3.5, "{name}: the count badge in 256 colors");
    }
}

// ---- the app in a light theme ---------------------------------------------------------------------------------

fn key(app: &mut crate::app::App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn shift(app: &mut crate::app::App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::SHIFT));
}

fn draw(term: &mut Terminal<TestBackend>, app: &crate::app::App) -> String {
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn lines(term: &Terminal<TestBackend>) -> Vec<Vec<Cell>> {
    let buf = term.backend().buffer();
    (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].clone()).collect()).collect()
}

fn text_of(line: &[Cell]) -> String {
    line.iter().map(|c| c.symbol()).collect()
}

/// Where `word` starts on the screen: (row, column), in cells.
fn find(ls: &[Vec<Cell>], word: &str) -> Option<(usize, usize)> {
    let w: Vec<String> = word.chars().map(|c| c.to_string()).collect();
    ls.iter().enumerate().find_map(|(y, l)| {
        let syms: Vec<&str> = l.iter().map(|c| c.symbol()).collect();
        (0..syms.len().saturating_sub(w.len() - 1)).find(|&i| syms[i..i + w.len()].iter().zip(&w).all(|(a, b)| *a == b)).map(|x| (y, x))
    })
}

fn at<'a>(ls: &'a [Vec<Cell>], word: &str) -> &'a Cell {
    let (y, x) = find(ls, word)
        .unwrap_or_else(|| panic!("{word} is not on the screen:\n{}", ls.iter().map(|l| text_of(l)).collect::<Vec<_>>().join("\n")));
    &ls[y][x]
}

/// Every letter and digit on the screen reads on the cell it is drawn on: at least 2.5:1 (dim text on a selected
/// row, the faintest text platoon draws on purpose), in 24-bit colors, and 2:1 after the 256-color pass. Rules,
/// bars and frames are lines, not text, and are left out, and so is what a popup mutes behind it on purpose (drawn
/// in the border color, on dark themes too).
fn assert_legible(term: &Terminal<TestBackend>, t: &Theme, what: &str) {
    for (y, l) in lines(term).iter().enumerate() {
        for (x, c) in l.iter().enumerate() {
            if !c.symbol().chars().any(char::is_alphanumeric) || c.fg == t.border {
                continue;
            }
            assert!(
                matches!(c.fg, Color::Rgb(..)) && matches!(c.bg, Color::Rgb(..)),
                "{what}: {:?} at {x},{y} is not in the theme's colors ({:?} on {:?})",
                c.symbol(),
                c.fg,
                c.bg
            );
            let ratio = contrast(c.fg, c.bg);
            assert!(
                ratio >= 2.5,
                "{what}: {:?} at {x},{y} is {ratio:.2}:1 ({:?} on {:?}) in the row: {}",
                c.symbol(),
                c.fg,
                c.bg,
                text_of(l)
            );
            let ratio = contrast(in_256(c.fg), in_256(c.bg));
            assert!(ratio >= 2.0, "{what}: {:?} at {x},{y} is {ratio:.2}:1 in 256 colors, in the row: {}", c.symbol(), text_of(l));
        }
    }
}

/// A world with the built-in `theme` chosen, three conversations (one working) and the first in a
/// project.
fn light_world(name: &str, theme: &str) -> (World, crate::app::App, String) {
    let mut w = World::new(name);
    w.transcript("aaa", "/srv/alpha", "refactor the parser", 40);
    w.transcript("bbb", "/srv/beta", "add a login page", 30);
    w.transcript("ccc", "/srv/gamma", "write the docs", 3);
    w.live("bbb", "/srv/beta", 1, "busy");
    let id = usertheme::slug(theme);
    crate::settings::save(&w.config, &crate::settings::Settings { theme: id.clone(), ..crate::settings::load(&w.config).0 }).unwrap();
    let mut app = w.app();
    assert_eq!(app.theme.id, id, "the theme is in effect");
    assert!(app.theme.light);
    let project = app.projects.create("Web shop").unwrap();
    app.projects.assign("aaa", Some(&project)).unwrap();
    app.reload_for_test();
    (w, app, project)
}

#[test]
fn the_sessions_list_in_a_light_theme_selected_row_badges_and_statuses() {
    let (_w, mut app, project) = light_world("light-list", "Catppuccin Latte");
    let t = app.theme.clone();
    let mut term = Terminal::new(TestBackend::new(130, 24)).unwrap();
    let shown = draw(&mut term, &app);
    assert!(shown.contains("refactor the parser"), "{shown}");
    let ls = lines(&term);
    assert_eq!((ls[1][1].fg, ls[1][1].bg), (t.border, t.bg), "the rule under the tabs on the light background");
    // The selected row: a surface band (a little darker than the background) with the accent marker and the text.
    let sel = app.visible()[app.selected];
    let name = app.sessions[sel].name.clone();
    let cell = at(&ls, &name);
    assert_eq!((cell.fg, cell.bg), (t.text, t.surface), "the selected row's name");
    let (y, _) = find(&ls, &name).unwrap();
    let mark = ls[y].iter().find(|c| c.symbol() == "▎").expect("the selected row's marker");
    assert_eq!((mark.fg, mark.bg), (t.accent, t.surface));
    // The working session, the current tab's count badge, and the project's pill.
    assert!(ls.iter().flatten().any(|c| c.fg == t.busy && c.symbol() == "w"), "working in the busy color");
    assert!(
        ls.iter().flatten().any(|c| c.bg == t.accent && c.fg == t.on_accent && c.symbol().chars().all(|ch| ch.is_ascii_digit())),
        "the count badge"
    );
    let fill = crate::projects::slot_color(&t, &app.projects.get(&project).unwrap().color);
    let pill = at(&ls, "Web shop");
    assert_eq!(pill.bg, fill);
    assert!(contrast(pill.fg, pill.bg) >= 4.5, "the pill's text reads: {:.2}", contrast(pill.fg, pill.bg));
    assert_legible(&term, &app.theme, "the Sessions list");

    // A project whose color is pale on a light background (yellow) gets dark text on its pill, not the background.
    app.projects.set_look(&project, "yellow", "WS").unwrap();
    app.reload_for_test();
    draw(&mut term, &app);
    let ls = lines(&term);
    let pill = at(&ls, "Web shop");
    assert_eq!(pill.bg, crate::projects::slot_color(&t, "yellow"));
    assert!(contrast(pill.fg, pill.bg) >= 4.5, "yellow pill: {:.2}", contrast(pill.fg, pill.bg));
    assert_legible(&term, &app.theme, "a yellow pill");
}

#[test]
fn a_popup_and_the_settings_in_a_light_theme() {
    let (_w, mut app, _) = light_world("light-popup", "Solarized Light");
    let t = app.theme.clone();
    let mut term = Terminal::new(TestBackend::new(120, 32)).unwrap();
    // The new-session popup (the folder picker).
    key(&mut app, KeyCode::Char('n'));
    assert!(matches!(app.mode, Mode::Picker));
    let shown = draw(&mut term, &app);
    let ls = lines(&term);
    let corner = ls
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(y, l)| l.iter().skip(1).position(|c| c.symbol() == "╭").map(|x| (y, x + 1)))
        .unwrap_or_else(|| panic!("{shown}"));
    let c = &ls[corner.0][corner.1];
    assert_eq!(c.bg, t.bg, "the popup is on the background");
    assert!(c.fg == t.border || c.fg == t.accent, "its frame: {:?}", c.fg);
    assert_legible(&term, &app.theme, "the folder picker");
    key(&mut app, KeyCode::Esc);

    // The settings: the popup, its tabs, the Theme row selected on a surface band.
    key(&mut app, KeyCode::Char('s'));
    assert!(matches!(app.mode, Mode::Settings));
    let shown = draw(&mut term, &app);
    assert!(shown.contains("Solarized Light"), "{shown}");
    let ls = lines(&term);
    let row = at(&ls, "Solarized Light");
    assert_eq!(row.bg, t.surface, "the selected Theme row");
    assert!(
        ls.iter().flatten().any(|c| c.fg == t.accent && c.symbol().chars().any(char::is_alphabetic)),
        "the current settings tab or title in the accent"
    );
    assert_legible(&term, &app.theme, "the settings");
}

#[test]
fn a_projects_notes_in_a_light_theme() {
    let (w, mut app, project) = light_world("light-notes", "GitHub Light");
    let t = app.theme.clone();
    let path = crate::projects::notes_path(&w.config, &project);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "## Summary\n\nThe **shop** takes `orders` now.\n\n- first point\n- second point\n\n> a quote\n").unwrap();
    key(&mut app, KeyCode::Char('2'));
    key(&mut app, KeyCode::Enter);
    assert!(app.open_project().is_some());
    shift(&mut app, KeyCode::Right);
    assert_eq!(app.pane, Pane::Notes);
    let mut term = Terminal::new(TestBackend::new(130, 30)).unwrap();
    let shown = draw(&mut term, &app);
    assert!(shown.contains("first point"), "{shown}");
    let ls = lines(&term);
    let code = at(&ls, "orders");
    assert_eq!((code.fg, code.bg), (t.ok, t.surface), "inline code");
    assert!(contrast(code.fg, code.bg) >= 4.0);
    let heading = at(&ls, "Summary");
    assert!(contrast(heading.fg, heading.bg) >= MIN_ROLE && heading.bg == t.bg, "the heading: {:?} on {:?}", heading.fg, heading.bg);
    assert_eq!(at(&ls, "first point").fg, t.text);
    assert_legible(&term, &app.theme, "the project screen with its notes");
}

#[test]
fn the_attached_views_footer_and_session_in_a_light_theme() {
    let (_w, mut app, _) = light_world("light-attached", "Gruvbox Light");
    let t = app.theme.clone();
    let (client, _host) = Client::for_test(23, 80);
    client.parser.lock().unwrap().process(b"plain text\r\n\x1b[31mred words\x1b[0m\r\n");
    app.client = Some(client);
    app.mode = Mode::Attached(4242);
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let shown = draw(&mut term, &app);
    let ls = lines(&term);
    assert!(shown.contains("back to list"), "{shown}");
    let footer = &ls[23];
    let hint = find(&ls, "back to list").unwrap();
    assert_eq!(hint.0, 23, "the footer is the last line: there is no frame");
    let label = &footer[hint.1];
    assert_eq!((label.fg, label.bg), (ui::hint_fg(&t), t.bg), "hint words read like body text");
    assert!(contrast(label.fg, label.bg) >= MIN_SUBTEXT);
    let back = app.back_label().to_string();
    let k = at(&ls, &back);
    assert_eq!((k.fg, k.bg), (t.accent, t.bg), "the back key in the accent");
    // The session: its default colors are the theme's, its red the theme's red.
    let plain = at(&ls, "plain text");
    assert_eq!((plain.fg, plain.bg), (t.text, t.bg));
    assert_eq!(at(&ls, "red words").fg, t.ansi.unwrap()[1]);
    assert_legible(&term, &app.theme, "the attached view");
}

#[test]
fn every_light_built_in_draws_every_main_screen_legibly() {
    let (w, mut app, project) = light_world("light-sweep", "Catppuccin Latte");
    let path = crate::projects::notes_path(&w.config, &project);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "## Summary\n\nThe **shop** takes `orders` now.\n\n- a point\n").unwrap();
    for name in LIGHT {
        let id = usertheme::slug(name);
        crate::settings::save(&w.config, &crate::settings::Settings { theme: id.clone(), ..crate::settings::load(&w.config).0 }).unwrap();
        app.refresh_for_test();
        assert_eq!(app.theme.id, id);
        for slot in ["accent", "blue", "magenta", "cyan", "green", "yellow", "red", "busy"] {
            app.projects.set_look(&project, slot, "WS").unwrap();
            app.reload_for_test();
            app.mode = Mode::List;
            key(&mut app, KeyCode::Char('1'));
            let mut term = Terminal::new(TestBackend::new(130, 30)).unwrap();
            draw(&mut term, &app);
            assert_legible(&term, &app.theme, &format!("{name}, list, {slot} project"));
        }
        let c = KeyCode::Char;
        // (the keys from the Sessions tab, what it is, something only that screen shows)
        let screens: [(&[KeyCode], &str, &str); 10] = [
            (&[c('s')], "settings", "╭ Settings"),
            (&[c('s'), KeyCode::Enter], "the theme list", "╭ Theme"),
            (&[c('n')], "the folder picker", "pick a folder"),
            (&[c('m')], "the project picker", "╭ Project"),
            (&[c('r')], "rename", "╭ Rename session"),
            (&[c('/')], "search", "╭ Search"),
            (&[c('2')], "projects", "Web shop"),
            (&[c('2'), c('c')], "a project's color", "╭ Color and badge"),
            (&[c('2'), c('d')], "deleting a project", "╭ Delete project"),
            (&[c('3')], "tasks", "No tasks yet"),
        ];
        for (keys, what, sign) in screens {
            app.mode = Mode::List;
            key(&mut app, KeyCode::Char('1'));
            for k in keys {
                key(&mut app, *k);
            }
            let mut term = Terminal::new(TestBackend::new(130, 30)).unwrap();
            let shown = draw(&mut term, &app);
            assert!(shown.contains(sign), "{name}, {what}: {shown}");
            assert_legible(&term, &app.theme, &format!("{name}, {what}"));
            for _ in 0..3 {
                key(&mut app, KeyCode::Esc);
            }
        }
        app.mode = Mode::List;
        key(&mut app, KeyCode::Char('2'));
        key(&mut app, KeyCode::Enter);
        shift(&mut app, KeyCode::Right);
        let mut term = Terminal::new(TestBackend::new(130, 30)).unwrap();
        let shown = draw(&mut term, &app);
        assert!(shown.contains("a point"), "{name}: {shown}");
        assert_legible(&term, &app.theme, &format!("{name}, notes"));
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Char('1'));
    }
}

#[test]
fn a_light_theme_of_the_users_own_draws_the_main_screens_legibly() {
    let mut w = World::new("light-user");
    w.transcript("aaa", "/srv/alpha", "refactor the parser", 40);
    w.transcript("bbb", "/srv/beta", "add a login page", 30);
    w.live("bbb", "/srv/beta", 1, "busy");
    // The user's own copy of a light theme, under a name with an accent: `user:creme-brulee`.
    let dir = w.config.join("themes");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(themes_dir().join("Catppuccin Latte"), dir.join("Crème Brûlée")).unwrap();
    std::fs::write(dir.join("Too Pale"), "background = #ffffff\nforeground = #cccccc\n").unwrap();
    crate::settings::save(
        &w.config,
        &crate::settings::Settings { theme: "user:creme-brulee".into(), ..crate::settings::load(&w.config).0 },
    )
    .unwrap();
    let mut app = w.app();
    assert_eq!(app.theme.id, "user:creme-brulee", "the user's light theme is in effect");
    assert!(app.theme.light && app.theme.name == "Crème Brûlée");
    let skipped = app.themes().skipped().reasons();
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    assert!(skipped[0].starts_with("Too Pale: a light theme has to read as it is: the text is hard to read"), "{skipped:?}");
    let t = app.theme.clone();
    let c = KeyCode::Char;
    let screens: [(&[KeyCode], &str, &str); 5] = [
        (&[], "the Sessions list", "refactor the parser"),
        (&[c('s')], "settings", "╭ Settings"),
        (&[c('s'), KeyCode::Enter], "the theme list", "Crème Brûlée"),
        (&[c('n')], "the folder picker", "pick a folder"),
        (&[c('3')], "tasks", "No tasks yet"),
    ];
    for (keys, what, sign) in screens {
        app.mode = Mode::List;
        key(&mut app, KeyCode::Char('1'));
        for k in keys {
            key(&mut app, *k);
        }
        let mut term = Terminal::new(TestBackend::new(130, 30)).unwrap();
        let shown = draw(&mut term, &app);
        assert!(shown.contains(sign), "{what}: {shown}");
        assert_eq!(lines(&term)[1][1].bg, t.bg, "{what}: on the light background");
        assert_legible(&term, &t, what);
        if what == "the theme list" {
            assert!(shown.contains("1 theme file skipped"), "{shown}");
        }
        for _ in 0..3 {
            key(&mut app, KeyCode::Esc);
        }
    }
}

#[test]
fn rose_pine_dawn_is_rose_pine_dawn() {
    let all = file_themes();
    let dawn = &all.iter().find(|(n, _, _)| n == "Rosé Pine Dawn").unwrap().2;
    assert_eq!(dawn.id, "rose-pine-dawn");
    assert!(Theme::by_id("ros-pine-dawn").is_none(), "the accented letter is its plain letter, not dropped");
}

#[test]
fn a_saved_theme_id_nobody_has_any_more_shows_the_default_and_is_kept() {
    let w = World::new("light-gone");
    w.transcript("aaa", "/srv/alpha", "refactor the parser", 40);
    // The example plugin's old id: its Harbor Night is a built-in now, under another id.
    let gone = "plugin:example-themes/harbor-night";
    crate::settings::save(&w.config, &crate::settings::Settings { theme: gone.into(), ..crate::settings::load(&w.config).0 }).unwrap();
    let mut app = w.app();
    assert_eq!(app.theme.id, crate::theme::DEFAULT_ID);
    assert!(Theme::by_id("harbor-night").is_some(), "the built-in Harbor Night is there");
    let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
    draw(&mut term, &app);
    assert_eq!(term.backend().buffer()[(1, 1)].bg, Theme::default().bg);
    app.refresh_for_test();
    assert_eq!(app.theme.id, crate::theme::DEFAULT_ID);
    assert_eq!(crate::settings::load(&w.config).0.theme, gone, "the id stays in the file");
}
