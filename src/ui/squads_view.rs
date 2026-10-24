//! The Squads tab (`4`): every squad in two-line rows (its name and where it comes from, then what it is for), and the
//! picked one beside the list from `TWO_PANES_FROM` columns (its description, models, whether it plans first, its steps
//! drawn from their Markdown, and what is wrong with its file). `enter` opens the picked one in the whole width, where
//! the setting `↑` `↓` picks is lit and `enter` changes it.

use super::{Hint, ROW_H, fit_end, hint_fg, wrap};
use crate::app::{Action, App, Hit, origin_words};
use crate::squads::{Origin, Setting, Squad};
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;

/// Below this width (of the area the tab is given) the tab is the list alone, and `enter` shows the picked squad. 78 is
/// what an 80-column terminal gives the tab (a column of air at each side): from 80 columns, the list and the squad side
/// by side.
pub const TWO_PANES_FROM: u16 = 78;

/// What a row's right side says about a squad, each piece with whether it is a warning: where it comes from, then
/// "needs fixing" when its file has problems. Where that leaves the name (`name_w` cells) too little of a `w`-cell row,
/// a squad that replaces a built-in says only "yours", and then "needs fixing" goes alone.
fn row_note(s: &Squad, w: usize, name_w: usize) -> Vec<(String, bool)> {
    let short = if s.origin == Origin::BuiltIn { "built in" } else { "yours" };
    let with_fix = |origin: &str| -> Vec<(String, bool)> {
        if s.usable() {
            vec![(origin.to_string(), false)]
        } else {
            vec![(format!("{origin} · "), false), ("needs fixing".to_string(), true)]
        }
    };
    let name_room = name_w.min(16);
    for pieces in [with_fix(origin_words(s.origin)), with_fix(short)] {
        let used: usize = pieces.iter().map(|(p, _)| p.width()).sum();
        if 2 + name_room + 2 + used <= w {
            return pieces;
        }
    }
    if s.usable() { vec![(short.to_string(), false)] } else { vec![("needs fixing".to_string(), true)] }
}

pub(super) fn draw_squads_tab(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let sqt = &app.sqt;
    if sqt.list.is_empty() {
        let p = Paragraph::new("No squads.").style(Style::new().fg(hint_fg(t))).alignment(Alignment::Center);
        f.render_widget(p, Rect::new(area.x, area.y + area.height / 3, area.width, 1));
        return;
    }
    if sqt.open {
        return draw_open_squad(f, app, area);
    }
    if area.width < TWO_PANES_FROM {
        return draw_squad_rows(f, app, area);
    }
    let left = (area.width * 2 / 5).max(30);
    draw_squad_rows(f, app, Rect::new(area.x, area.y, left.saturating_sub(1), area.height));
    for y in area.y..area.bottom() {
        f.render_widget(Paragraph::new("│").style(Style::new().fg(t.border)), Rect::new(area.x + left, y, 1, 1));
    }
    let pane = Rect::new(area.x + left + 2, area.y, area.width.saturating_sub(left + 2), area.height);
    if let Some(s) = sqt.picked() {
        let w = pane.width as usize;
        let mut lines = squad_lines(app, s, w, Some(pane.height as usize), None);
        lines.truncate(pane.height as usize);
        f.render_widget(Paragraph::new(lines), pane);
    }
}

/// The list: two lines a squad, the picked one lit.
fn draw_squad_rows(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let sqt = &app.sqt;
    if area.height < ROW_H || area.width < 10 {
        return;
    }
    let fits = (area.height / ROW_H) as usize;
    let more = sqt.list.len() > fits;
    // When they don't all fit, the last line says how many more there are.
    let rows = if more { ((area.height - 1) / ROW_H).max(1) as usize } else { fits };
    sqt.page.set(rows);
    let mut top = sqt.list_top.get().min(sqt.list.len().saturating_sub(rows));
    if sqt.selected < top {
        top = sqt.selected;
    } else if sqt.selected >= top + rows {
        top = sqt.selected + 1 - rows;
    }
    sqt.list_top.set(top);
    let w = area.width as usize;
    for (i, s) in sqt.list.iter().enumerate().skip(top).take(rows) {
        let y = area.y + ((i - top) as u16) * ROW_H;
        let rect = Rect::new(area.x, y, area.width, ROW_H);
        let sel = i == sqt.selected;
        let lit = sel || rect.contains(app.mouse.into());
        let bg = if lit { t.surface } else { t.bg };
        let base = Style::new().bg(bg);
        let right = row_note(s, w, s.name_or_id().width());
        let right: Vec<Span> =
            right.into_iter().map(|(text, warn)| Span::styled(text, base.fg(if warn { t.warn } else { t.subtext }))).collect();
        let right_w: usize = right.iter().map(|s| s.content.width()).sum();
        let name_room = w.saturating_sub(2 + right_w + 2);
        let name = fit_end(&s.name_or_id(), name_room);
        let gap = w.saturating_sub(2 + name.width() + right_w);
        let mut first = vec![
            Span::styled(if sel { "▎ " } else { "  " }, base.fg(t.accent)),
            Span::styled(name, base.fg(t.text).add_modifier(Modifier::BOLD)),
            Span::styled(" ".repeat(gap), base),
        ];
        first.extend(right);
        let second =
            Line::from(vec![Span::styled("  ", base), Span::styled(fit_end(&s.description, w.saturating_sub(2)), base.fg(t.subtext))]);
        for (k, line) in [Line::from(first), second].into_iter().enumerate() {
            let at = Rect::new(area.x, y + k as u16, area.width, 1);
            app.hits.borrow_mut().push((at, Hit::SquadsTabRow(i)));
            f.render_widget(Paragraph::new(line).style(base), at);
        }
    }
    let below = sqt.list.len().saturating_sub(top + rows);
    if below > 0 {
        super::draw_more_below(f, t, Rect::new(area.x, area.bottom() - 1, area.width, 1), below);
    }
}

/// The picked squad in the tab's whole width, scrolled with the arrows.
fn draw_open_squad(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    let Some(s) = app.sqt.picked() else { return };
    let view = Rect::new(area.x + 1, area.y, area.width.saturating_sub(2), area.height);
    let lines = squad_lines(app, s, view.width as usize, None, app.sqt.field);
    let h = view.height as usize;
    app.sqt.page.set(h.saturating_sub(1).max(1));
    let top = app.sqt.top.get().min(lines.len().saturating_sub(h));
    app.sqt.top.set(top);
    // The settings' lines can be clicked: a click picks one, a click on the picked one changes it.
    let first = settings_at(s, view.width as usize);
    for i in 0..Setting::ALL.len() {
        if let Some(row) = (first + i).checked_sub(top).filter(|r| *r < h) {
            app.hits.borrow_mut().push((Rect::new(view.x, view.y + row as u16, view.width, 1), Hit::SquadSetting(i)));
        }
    }
    let below = lines.len().saturating_sub(top + h);
    let shown: Vec<Line> = lines.into_iter().skip(top).take(if below > 0 { h.saturating_sub(1) } else { h }).collect();
    f.render_widget(Paragraph::new(shown), view);
    if below > 0 {
        super::draw_more_below(f, t, Rect::new(view.x, view.bottom() - 1, view.width, 1), below);
    }
}

/// The line of a squad's lines (`squad_lines`, `w` cells wide) its first setting is on: under its name, what it is for
/// and a blank line.
fn settings_at(s: &Squad, w: usize) -> usize {
    2 + wrap(&s.description, w).len()
}

/// A squad as the pane shows it, `w` cells wide: its name and what it is for, its settings, its steps, then what is
/// wrong with its file. With `fit` (the pane beside the list, that many lines tall) the steps are cut so that what is
/// wrong still shows. `picked`: the setting `enter` changes (the open squad), lit, with that said at its end.
pub(super) fn squad_lines(app: &App, s: &Squad, w: usize, fit: Option<usize>, picked: Option<usize>) -> Vec<Line<'static>> {
    let t = app.effective_theme();
    let mut head: Vec<Line<'static>> =
        vec![Line::styled(fit_end(&s.name_or_id(), w), Style::new().fg(t.text).add_modifier(Modifier::BOLD))];
    head.extend(wrap(&s.description, w).into_iter().map(|l| Line::styled(l, Style::new().fg(t.subtext))));
    head.push(Line::raw(""));
    const LABEL: usize = 15;
    const CHANGES: &str = "   enter changes";
    let catalog = app.catalog();
    // The three settings, one line each (the one picked lit, with what enter does).
    for (i, setting) in Setting::ALL.into_iter().enumerate() {
        let lit = picked == Some(i);
        let base = if lit { Style::new().bg(t.surface) } else { Style::new() };
        let room = w.saturating_sub(LABEL + if lit { CHANGES.width() } else { 0 });
        let value = Some(setting.words(s, &catalog)).filter(|v| !v.is_empty()).unwrap_or_else(|| "none".into());
        let value = fit_end(&value, room);
        let mut spans =
            vec![Span::styled(format!("{:<LABEL$}", setting.label()), base.fg(t.subtext)), Span::styled(value.clone(), base.fg(t.text))];
        if lit {
            // What enter does, just after the value; the light runs on to the line's end.
            let gap = room.saturating_sub(value.width());
            spans.push(Span::styled(CHANGES, base.fg(hint_fg(t))));
            spans.push(Span::styled(" ".repeat(gap), base));
        }
        head.push(Line::from(spans));
    }
    head.push(Line::raw(""));
    head.push(Line::styled("Steps", Style::new().fg(t.text).add_modifier(Modifier::BOLD)));
    let mut steps = if s.body.trim().is_empty() {
        vec![Line::styled("No steps yet: e opens the file.", Style::new().fg(hint_fg(t)))]
    } else {
        crate::mdview::render_joined(s.body.trim(), w, t, app.plain_marker)
    };
    let mut tail: Vec<Line<'static>> = Vec::new();
    if !s.problems.is_empty() {
        tail.push(Line::raw(""));
        tail.push(Line::styled("Needs fixing", Style::new().fg(t.warn).add_modifier(Modifier::BOLD)));
        for p in &s.problems {
            for (k, row) in wrap(p, w.saturating_sub(2).max(10)).into_iter().enumerate() {
                tail.push(Line::styled(format!("{}{row}", if k == 0 { "· " } else { "  " }), Style::new().fg(t.warn)));
            }
        }
    }
    if let Some(h) = fit {
        let room = h.saturating_sub(head.len() + tail.len());
        if steps.len() > room {
            let more = steps.len() + 1 - room.max(1);
            steps.truncate(room.saturating_sub(1));
            steps.push(Line::styled(fit_end(&format!("… {more} more lines: enter shows them all"), w), Style::new().fg(t.dim)));
        }
    }
    head.extend(steps);
    head.extend(tail);
    head
}

/// The keys of the Squads tab: the list's, or the open squad's.
pub(super) fn squads_hints(app: &App) -> Vec<Hint<'_>> {
    let sqt = &app.sqt;
    let own = sqt.picked().is_some_and(|s| s.origin != Origin::BuiltIn);
    let mut out: Vec<Hint> = Vec::new();
    if sqt.open {
        if sqt.field.is_some() {
            out.push(("enter", "change", None));
        }
        out.push(("e", "edit", None));
        if own {
            out.push(("d", "delete", None));
        }
        out.extend([("↑↓", "move", None), ("?", "help", Some(Action::Help)), ("esc", "back", Some(Action::Leave))]);
        return out;
    }
    out.push(("enter", "open", None));
    out.push(("e", "edit", None));
    out.push(("n", "new squad", None));
    if own {
        out.push(("d", "delete", None));
    }
    out.extend([
        ("1-4", "tabs", None),
        ("↑↓", "move", None),
        ("s", "settings", Some(Action::Settings)),
        ("?", "help", Some(Action::Help)),
        (app.quit_label(), "quit", Some(Action::Quit)),
    ]);
    out
}
