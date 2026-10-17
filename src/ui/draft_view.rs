//! The popup **Tasks from this conversation**: the tasks Claude drafted from a session, each as a block (its title, the
//! squad suggested for it, the folder it works in, then its text drawn from its Markdown: what it should do, its "Done
//! when" boxes), the picked one marked, scrolled so it shows; and the keys that make them, change them or leave them.
//! It fits 80x24: the blocks scroll and the keys take two lines when one is too short.

use super::{Hint, badge_span, centered, draw_footer, draw_more_below, fit_end, hint_fg, hints_width, modal_block, popup_ground, wrap};
use crate::app::{Action, App, Hit};
use ratatui::{
    Frame,
    layout::{Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

/// The width of the labels in a block ("Works in" and a space).
const LABEL: usize = 10;

/// The keys the popup offers, the ones that make or leave the draft first (they stay when the line is short).
pub(super) fn draft_hints() -> Vec<Hint<'static>> {
    vec![
        ("enter", "make them", Some(Action::Confirm)),
        ("esc", "cancel", Some(Action::Cancel)),
        ("e", "edit", None),
        ("x", "remove", None),
        ("↑↓", "move", None),
    ]
}

/// A task's text drawn tighter than the notes are: no blank line between a heading and what is under it (a heading is
/// a line all in bold), so a block of a short task fits a small terminal.
fn compact(lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    let heading = |l: &Line| {
        !l.spans.is_empty()
            && l.spans.iter().any(|s| !s.content.trim().is_empty())
            && l.spans.iter().filter(|s| !s.content.trim().is_empty()).all(|s| s.style.add_modifier.contains(Modifier::BOLD))
    };
    let blank = |l: &Line| l.spans.iter().all(|s| s.content.trim().is_empty());
    let mut out: Vec<Line<'static>> = Vec::with_capacity(lines.len());
    for l in lines {
        if blank(&l) && out.last().is_some_and(heading) {
            continue;
        }
        out.push(l);
    }
    out
}

pub(super) fn draw_task_draft(f: &mut Frame, app: &App, area: Rect) {
    let t = app.effective_theme();
    app.hits.borrow_mut().clear();
    let Some(d) = app.task_draft() else { return };
    let w = area.width.saturating_sub(4).clamp(40, 100);
    let h = area.height.saturating_sub(2).max(10);
    let r = centered(area, w, h);
    popup_ground(f, t, area, r, true);
    let block = modal_block(t, "Tasks from this conversation");
    let inner = block.inner(r).inner(Margin::new(1, 0));
    f.render_widget(block, r);
    let iw = inner.width as usize;
    if iw < 20 || inner.height < 6 {
        app.hits.borrow_mut().push((r, Hit::Inert));
        return;
    }

    // Where it came from and where the tasks go.
    let mut head: Vec<Span> =
        vec![Span::styled(fit_end(&format!("From \"{}\" · ", d.session_name), iw.saturating_sub(20)), Style::new().fg(t.subtext))];
    match app.projects.get(&d.project) {
        Some(p) => {
            head.push(Span::styled("in ", Style::new().fg(t.subtext)));
            head.push(badge_span(t, p));
            head.push(Span::styled(format!(" {}", p.name), Style::new().fg(t.text)));
        }
        None => head.push(Span::styled("in no project", Style::new().fg(t.text))),
    }
    let n = d.tasks.len();
    let count = if n == 1 { "1 task".to_string() } else { format!("{n} tasks") };
    head.push(Span::styled(format!(" · {count}"), Style::new().fg(t.subtext)));
    f.render_widget(Paragraph::new(Line::from(head)), Rect::new(inner.x, inner.y, inner.width, 1));
    let note = "Change or take out what you like: nothing is made until you say so.";
    f.render_widget(
        Paragraph::new(Span::styled(fit_end(note, iw), Style::new().fg(hint_fg(t)))),
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
    );

    // The keys: on one line when they fit, else on two (the ones that make or leave the draft first).
    let hints = draft_hints();
    // (Two lines also while a key's line is said beside them.)
    let one = hints_width(&hints) <= iw && app.active_notice().is_none();
    let rows: Vec<Vec<Hint>> = if one { vec![hints] } else { vec![hints[..2].to_vec(), hints[2..].to_vec()] };
    let foot_h = rows.len() as u16;
    let foot_y = inner.bottom() - foot_h;
    // What a key just did ("Took ... out of the draft") is said here, beside the last row of keys, where it is seen.
    let last = rows.len().saturating_sub(1);
    for (k, row) in rows.iter().enumerate() {
        let said = if k == last { app.active_notice().map(|n| (n.to_string(), t.warn)) } else { None };
        draw_footer(f, app, Rect::new(inner.x, foot_y + k as u16, inner.width, 1), row, said);
    }

    // The tasks, as blocks, between a blank line under the head and one above the keys.
    let list = Rect::new(inner.x, inner.y + 3, inner.width, foot_y.saturating_sub(inner.y + 4));
    if list.height == 0 {
        app.hits.borrow_mut().push((r, Hit::Inert));
        return;
    }
    let lw = iw.saturating_sub(2);
    let mut lines: Vec<(Option<usize>, Line<'static>)> = Vec::new();
    let mut starts: Vec<(usize, usize)> = Vec::new();
    let label = |s: &str| Span::styled(format!("{s:<LABEL$}"), Style::new().fg(t.dim));
    for (i, task) in d.tasks.iter().enumerate() {
        let from = lines.len();
        let sel = i == d.selected;
        for l in wrap(&task.title, lw) {
            let style = Style::new().fg(if sel { t.accent } else { t.text }).add_modifier(Modifier::BOLD);
            lines.push((Some(i), Line::styled(l, style)));
        }
        let squad = match task.squad.as_ref().and_then(|id| d.kinds.iter().find(|k| k.id == *id)) {
            Some(k) => vec![
                label("Squad"),
                Span::styled(format!("{} ", k.name), Style::new().fg(t.text)),
                Span::styled("(suggested)", Style::new().fg(t.dim)),
            ],
            None => vec![label("Squad"), Span::styled("none suggested: you pick one when it starts", Style::new().fg(t.subtext))],
        };
        lines.push((Some(i), Line::from(squad)));
        lines.push((
            Some(i),
            Line::from(vec![
                label("Works in"),
                Span::styled(fit_end(&crate::dirs::tilde(&d.folder, &app.home), lw.saturating_sub(LABEL)), Style::new().fg(t.text)),
            ]),
        ));
        lines.push((Some(i), Line::raw("")));
        for l in compact(crate::mdview::render(task.body.trim(), lw, t, app.plain_marker)) {
            lines.push((Some(i), l));
        }
        starts.push((from, lines.len()));
        if i + 1 < d.tasks.len() {
            lines.push((None, Line::raw("")));
        }
    }
    let total = lines.len();
    let view = list.height as usize;
    app.pages.tickets.set((view / 2).max(1));
    // The picked task shows: from its start when it all fits, otherwise its start at the top.
    let (a, b) = starts.get(d.selected).copied().unwrap_or((0, 0));
    let mut top = d.top.get().min(total.saturating_sub(1));
    let rows_shown = if total > view { view.saturating_sub(1) } else { view };
    if a < top || b - a > rows_shown {
        top = a;
    } else if b > top + rows_shown {
        top = b - rows_shown;
    }
    let top = top.min(total.saturating_sub(rows_shown));
    d.top.set(top);
    for (k, (owner, line)) in lines.into_iter().skip(top).take(rows_shown).enumerate() {
        let y = list.y + k as u16;
        let sel = owner == Some(d.selected);
        let bar = Span::styled(if sel { "▎ " } else { "  " }, Style::new().fg(t.accent));
        let mut spans = vec![bar];
        spans.extend(line.spans);
        let rect = Rect::new(list.x, y, list.width, 1);
        if let Some(i) = owner {
            app.hits.borrow_mut().push((rect, Hit::DraftTask(i)));
        }
        f.render_widget(Paragraph::new(Line::from(spans).style(line.style)), rect);
    }
    let below = total.saturating_sub(top + rows_shown);
    draw_more_below(f, t, Rect::new(list.x + 2, list.bottom().saturating_sub(1), list.width.saturating_sub(2), 1), below);
    // Last: the rows and the keys above are found first by a click.
    app.hits.borrow_mut().push((r, Hit::Inert));
}
