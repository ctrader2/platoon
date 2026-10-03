//! The help popup (`?`): every key of the screen showing, from the same table its footer uses (so the two can't drift),
//! with the settings and the search where they work; the keys that work the same all over platoon (with the user's own
//! back and quit keys from `keys.json`); and a few things worth knowing. It scrolls when it is longer than the screen.

use super::{centered, hint_fg, modal_block, popup_ground, screen_hints, wrap};
use crate::app::{App, Hit, TOPS};
use ratatui::{
    Frame,
    layout::{Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

/// One line of the help before it is wrapped to the popup's width.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Entry {
    Heading(&'static str),
    Key(String, String),
    Note(String),
    Blank,
}

/// Hints the help lists once, under "Anywhere in platoon", rather than with the screen's own keys: the tabs, quit,
/// this help, and the screen's own esc "back" (the back key's line says esc does the same).
const GLOBAL: &[&str] = &["tabs", "quit", "help", "back"];

/// The keys that work the same on every one of platoon's own screens: the same list wherever the help is opened.
fn anywhere(app: &App) -> Vec<(String, String)> {
    vec![
        (format!("1-{}", TOPS.len()), format!("the tabs: {}", TOPS.join(", "))),
        (app.back_label().to_string(), "back one level (esc does the same)".into()),
        (app.quit_label().to_string(), "quit platoon, on the Sessions, Projects and Squads lists (elsewhere it goes back)".into()),
        ("?".into(), "this help".into()),
    ]
}

/// Everything the help says for the screen showing.
pub(super) fn entries(app: &App) -> Vec<Entry> {
    let mut out = vec![Entry::Heading("On this screen")];
    let mut screen: Vec<(String, String)> =
        screen_hints(app).into_iter().filter(|(_, d, _)| !GLOBAL.contains(d)).map(|(k, d, _)| (k.to_string(), d.to_string())).collect();
    // The settings and the search open from most screens but not all: listed here, on the screens where they work.
    for (works, k, d) in [(app.slash_searches(), "/", "search"), (app.s_opens_settings(), "s", "settings")] {
        if works && !screen.iter().any(|(_, have)| have == d) {
            screen.push((k.into(), d.into()));
        }
    }
    // The footer says only "tab next pane"; the arrows do it too, in the direction of the pane.
    if let Some(at) = screen.iter().position(|(_, d)| d == "next pane") {
        screen.insert(at + 1, ("shift+arrows".into(), "the pane that way (ctrl+arrows too)".into()));
    }
    out.extend(screen.into_iter().map(|(k, d)| Entry::Key(k, d)));
    out.push(Entry::Blank);
    out.push(Entry::Heading("Anywhere in platoon"));
    out.extend(anywhere(app).into_iter().map(|(k, d)| Entry::Key(k, d)));
    out.push(Entry::Blank);
    out.push(Entry::Heading("Good to know"));
    out.push(Entry::Note(
        "platoon uses the mouse itself: to select text with your terminal's own selection, hold Shift while you drag.".into(),
    ));
    out.push(Entry::Note("Ctrl+Z does nothing on platoon's own screens; in a session it goes to Claude Code like any other key.".into()));
    out.push(Entry::Note(
        "A ~ before a percentage means it is an estimate: platoon worked it out from the session's messages. Without it, the figure is Claude Code's own.".into(),
    ));
    out
}

/// The widest the key column gets; a longer key wraps its description under itself.
const KEY_MAX: usize = 18;

/// The help's lines wrapped to `width`: keys in a column of their own in the key color, descriptions beside them with
/// a hanging indent.
fn lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let t = app.effective_theme();
    let all = entries(app);
    let key_w =
        all.iter().filter_map(|e| if let Entry::Key(k, _) = e { Some(k.chars().count()) } else { None }).max().unwrap_or(0).min(KEY_MAX)
            + 2;
    let mut out = Vec::new();
    for e in all {
        match e {
            Entry::Heading(h) => out.push(Line::styled(h, Style::new().fg(t.text).add_modifier(Modifier::BOLD))),
            Entry::Blank => out.push(Line::raw("")),
            Entry::Note(n) => out.extend(wrap(&n, width).into_iter().map(|l| Line::styled(l, Style::new().fg(t.subtext)))),
            Entry::Key(k, d) => {
                let room = width.saturating_sub(key_w).max(10);
                let keyed = k.chars().count() + 2 <= key_w;
                let mut desc = wrap(&d, room).into_iter();
                let first = if keyed { desc.next().unwrap_or_default() } else { String::new() };
                let pad = key_w.saturating_sub(k.chars().count());
                out.push(Line::from(vec![
                    Span::styled(k, Style::new().fg(t.accent)),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(first, Style::new().fg(t.subtext)),
                ]));
                out.extend(desc.map(|l| Line::from(vec![Span::raw(" ".repeat(key_w)), Span::styled(l, Style::new().fg(t.subtext))])));
            }
        }
    }
    out
}

/// The popup, over the screen it is about: centered, as tall as its text or the screen, scrolled to `help_top`.
pub(super) fn draw_help(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    // Nothing behind it is clicked while it is open: a click outside closes it (`App::help_mouse`).
    app.hits.borrow_mut().clear();
    let w = area.width.saturating_sub(4).min(76);
    // Borders, a column of air each side.
    let text_w = w.saturating_sub(4) as usize;
    let text = lines(app, text_w);
    // The text, a blank line and the keys of the popup itself, inside the borders.
    let want = text.len() as u16 + 4;
    let r = centered(area, w, want.min(area.height.saturating_sub(2)));
    // The screen it is about stays in view behind it, faded as it is behind every popup.
    popup_ground(f, t, area, r, true);
    let block = modal_block(t, "Keys");
    let inner = block.inner(r).inner(Margin::new(1, 0));
    f.render_widget(block, r);
    app.hits.borrow_mut().push((r, Hit::Inert));
    let view_h = inner.height.saturating_sub(2) as usize;
    let max_top = text.len().saturating_sub(view_h);
    let top = app.help_top.get().min(max_top);
    app.help_top.set(top);
    let shown: Vec<Line> = text.into_iter().skip(top).take(view_h).collect();
    f.render_widget(Paragraph::new(shown), Rect::new(inner.x, inner.y, inner.width, view_h as u16));
    let mut keys = vec![Span::styled("esc", Style::new().fg(t.accent)), Span::styled(" close", Style::new().fg(hint_fg(t)))];
    if max_top > 0 {
        keys.extend([
            Span::raw("   "),
            Span::styled("↑↓", Style::new().fg(t.accent)),
            Span::styled(" scroll", Style::new().fg(hint_fg(t))),
        ]);
        let more = if top < max_top { format!("   {} more lines below", max_top - top) } else { String::new() };
        keys.push(Span::styled(more, Style::new().fg(t.dim)));
    }
    f.render_widget(Paragraph::new(Line::from(keys)), Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1));
}
