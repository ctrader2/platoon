//! Contrast, as drawn, in every built-in theme (dark and light): the words that say what a key does read at 4.5:1,
//! text on the selection fill meets the floors it meets on the page, the fill itself stands clear of the page, and
//! behind a popup every colored background is faded with the text. Every directory is a temp dir (`World`); nothing
//! here starts a process.

use crate::app::App;
use crate::testkit::World;
use crate::theme::{MIN_DIM, MIN_SUBTEXT, MIN_SURFACE, Theme, contrast};
use crate::ui;
use ratatui::{
    Terminal,
    backend::TestBackend,
    buffer::{Buffer, Cell},
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
};

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

/// Three sessions, the first in a project (so a badge shows), the app at `w` x `h`.
fn world(name: &str) -> (World, App) {
    let w = World::new(name);
    w.transcript("aaa", "/srv/alpha", "refactor the parser", 40);
    w.transcript("bbb", "/srv/beta", "add a login page", 30);
    w.transcript("ccc", "/srv/gamma", "write the docs", 3);
    let mut app = w.app();
    let project = app.projects.create("Web shop").unwrap();
    app.projects.assign("aaa", Some(&project)).unwrap();
    app.reload_for_test();
    app.notice = None;
    (w, app)
}

/// Every built-in with colors of its own: the `SPECS` themes and the theme files (Terminal colors has none to measure).
fn themes() -> Vec<Theme> {
    Theme::all().into_iter().filter(|t| t.ansi.is_some()).collect()
}

fn frame(app: &mut App, w: u16, h: u16) -> Buffer {
    app.sync_size(w, h);
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ui::draw(f, app)).unwrap();
    term.backend().buffer().clone()
}

fn row_text(buf: &Buffer, y: u16) -> String {
    (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect()
}

/// Where `word` first shows, in cells.
fn find(buf: &Buffer, word: &str) -> (u16, u16) {
    for y in 0..buf.area.height {
        let text = row_text(buf, y);
        if let Some(b) = text.find(word) {
            return (text[..b].chars().count() as u16, y);
        }
    }
    panic!("{word:?} is not on the screen")
}

/// The cells of the first place `word` shows, `word`'s length of them.
fn cells_of<'a>(buf: &'a Buffer, word: &str) -> Vec<&'a Cell> {
    let (x, y) = find(buf, word);
    (x..x + word.chars().count() as u16).map(|x| &buf[(x, y)]).collect()
}

fn letters(cells: &[&Cell]) -> Vec<(String, f64)> {
    cells.iter().filter(|c| c.symbol().chars().any(char::is_alphanumeric)).map(|c| (c.symbol().to_string(), contrast(c.fg, c.bg))).collect()
}

#[test]
fn hint_words_read_at_4_5_to_1_in_every_theme_in_the_footer_the_tabs_and_the_help() {
    let (_w, mut app) = world("contrast-hints");
    for t in themes() {
        app.theme = t.clone();
        let buf = frame(&mut app, 120, 30);
        let footer = buf.area.height - 1;
        let line = row_text(&buf, footer);
        assert!(line.contains("help"), "{}: {line}", t.id);
        // Every letter of the footer (the keys in the accent, their words in the hint color) reads as text.
        for x in 0..buf.area.width {
            let c = &buf[(x, footer)];
            if c.symbol().chars().any(char::is_alphanumeric) {
                assert!(
                    contrast(c.fg, c.bg) >= MIN_SUBTEXT,
                    "{}: {:?} at {x} in the footer is {:.2}:1: {line}",
                    t.id,
                    c.symbol(),
                    contrast(c.fg, c.bg)
                );
            }
        }
        assert_eq!(cells_of(&buf, "search")[0].fg, ui::hint_fg(&t), "{}: the hint words are the hint color", t.id);
        // The tabs' keys are hints too, on the current tab's fill as on the page.
        for name in ["1 Sessions", "2 Tasks", "3 Projects", "4 Squads"] {
            let k = cells_of(&buf, name)[0];
            assert!(contrast(k.fg, k.bg) >= MIN_SUBTEXT, "{}: the key of {name} is {:.2}:1", t.id, contrast(k.fg, k.bg));
        }
    }
    // The help popup's own keys, in two themes (light and dark).
    for id in ["catppuccin-latte", "nord"] {
        app.theme = Theme::by_id(id).unwrap();
        key(&mut app, KeyCode::Char('?'));
        assert!(app.help_showing());
        let buf = frame(&mut app, 80, 24);
        for (s, ratio) in letters(&cells_of(&buf, "esc close")) {
            assert!(ratio >= MIN_SUBTEXT, "{id}: {s:?} of `esc close` is {ratio:.2}:1");
        }
        key(&mut app, KeyCode::Esc);
        assert!(!app.help_showing());
    }
}

#[test]
fn text_on_the_selected_row_meets_the_floors_it_meets_on_the_page_in_every_theme() {
    let (_w, mut app) = world("contrast-selection");
    for t in themes() {
        app.theme = t.clone();
        let buf = frame(&mut app, 140, 24);
        // The fill is clearly off the page.
        assert!(contrast(t.surface, t.bg) >= MIN_SURFACE, "{}", t.id);
        let sel = app.visible()[app.selected];
        let (name, desc) = (app.sessions[sel].name.clone(), app.sessions[sel].description.clone());
        let name_cells = cells_of(&buf, &name);
        assert_eq!(name_cells[0].bg, t.surface, "{}: the selected row is on the fill", t.id);
        for (s, ratio) in letters(&name_cells) {
            assert!(ratio >= MIN_SUBTEXT, "{}: the name's {s:?} is {ratio:.2}:1 on the fill", t.id);
        }
        // The description line under it is dim: at least dim's floor on the fill.
        let d = cells_of(&buf, &desc[..desc.len().min(12)]);
        assert_eq!(d[0].bg, t.surface, "{}", t.id);
        for (s, ratio) in letters(&d) {
            assert!(ratio >= MIN_DIM, "{}: the description's {s:?} is {ratio:.2}:1 on the fill", t.id);
        }
        // Every letter on the fill anywhere on the screen reads at least as well as dim text on the page, and every
        // one drawn in the subtext or the text color (or lifted from them) at 4.5:1.
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                let c = &buf[(x, y)];
                if c.bg != t.surface || !c.symbol().chars().any(char::is_alphanumeric) {
                    continue;
                }
                let ratio = contrast(c.fg, c.bg);
                assert!(ratio >= MIN_DIM, "{}: {:?} at {x},{y} is {ratio:.2}:1 on the fill: {}", t.id, c.symbol(), row_text(&buf, y));
                let readable_text = [t.subtext, t.text].map(|c| t.readable_on(c, t.surface, MIN_SUBTEXT));
                if readable_text.contains(&c.fg) {
                    assert!(ratio >= MIN_SUBTEXT, "{}: {:?} at {x},{y}", t.id, c.symbol());
                }
            }
        }
    }
}

#[test]
fn behind_a_popup_the_colored_backgrounds_fade_with_the_text() {
    let (_w, mut app) = world("contrast-fade");
    for id in ["catppuccin-mocha", "nord", "catppuccin-latte"] {
        let t = Theme::by_id(id).unwrap();
        app.theme = t.clone();
        let before = frame(&mut app, 120, 30);
        // The selected row's fill and the project's badge, before.
        let sel = app.visible()[app.selected];
        let name = app.sessions[sel].name.clone();
        let (nx, ny) = find(&before, &name);
        assert_eq!(before[(nx, ny)].bg, t.surface, "{id}");
        let badge = cells_of(&before, "Web shop")[1].bg;
        assert!(badge != t.bg && badge != t.surface, "{id}: the badge has a color of its own");

        // The rename popup sits in the middle; the selected row's start and the badge stay outside it, faded.
        key(&mut app, KeyCode::Char('r'));
        let after = frame(&mut app, 120, 30);
        assert!((0..after.area.height).any(|y| row_text(&after, y).contains("Rename")), "{id}: a popup is open");
        let behind = &after[(nx, ny)];
        assert_eq!(behind.fg, t.border, "{id}: the text behind is faded");
        assert!(behind.bg != t.surface && behind.bg != t.bg, "{id}: the fill is faded, not dropped: {:?}", behind.bg);
        assert!(contrast(behind.bg, t.bg) < contrast(t.surface, t.bg), "{id}: nearer the page than the fill was");
        // No background behind the popup stays as bright as it was: each is at most half as far from the page.
        for y in 0..after.area.height {
            for x in 0..after.area.width {
                let (b, a) = (&before[(x, y)], &after[(x, y)]);
                if b.bg == t.bg || a.bg == t.bg || a.symbol() != b.symbol() {
                    continue;
                }
                assert!(
                    contrast(a.bg, t.bg) <= 1.0 + (contrast(b.bg, t.bg) - 1.0) * 0.5,
                    "{id}: the background at {x},{y} is {:.2}:1 from the page, as bright as before ({:.2}:1)",
                    contrast(a.bg, t.bg),
                    contrast(b.bg, t.bg)
                );
            }
        }
        key(&mut app, KeyCode::Esc);
    }
}
